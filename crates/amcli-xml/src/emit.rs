use std::io::{self, Write};

use crate::{AttrOrigin, Doc, Name, NodeData, NodeId, NodeState, Span};

pub(crate) fn write_doc(doc: &Doc, w: &mut impl Write) -> io::Result<()> {
    write_span(doc, w, doc.prologue)?;
    write_node(doc, w, doc.root, 0)?;
    write_span(doc, w, doc.epilogue)
}

#[inline]
fn write_span(doc: &Doc, w: &mut impl Write, s: Span) -> io::Result<()> {
    if s.is_empty() {
        return Ok(());
    }
    w.write_all(&doc.src[s.start as usize..s.end as usize])
}

fn write_node(doc: &Doc, w: &mut impl Write, id: NodeId, depth: usize) -> io::Result<()> {
    let n = doc.node(id);
    if n.removed {
        return Ok(());
    }

    // The fast path, and the whole point of this crate: an untouched subtree is
    // one contiguous slice of the original file.
    if doc.is_pristine(id) {
        return write_span(doc, w, n.span);
    }

    let live: Vec<NodeId> = n.children.iter().copied().filter(|c| !doc.node(*c).removed).collect();
    // Every element child removed. What is left between the tags is the
    // indentation they sat in, not content — and EMF writes an element with
    // neither children nor text as `<a/>`, which is how this one was written
    // before anything was put in it. Without this, adding a documentation and
    // then clearing it left `<element …></element>` behind: the two edits are
    // two runs of amcli, and the second one parses a file where the `/>` is
    // already gone, so nothing else can know to put it back. `n.children` is
    // the test rather than `live`, so an element that was `<a></a>` in the
    // source and is dirty for some other reason keeps the shape it had.
    let emptied = !n.children.is_empty() && live.is_empty() && n.text_override.is_none();
    let has_text = n.text_override.as_deref().is_some_and(|t| !t.is_empty())
        || (live.is_empty() && !n.self_closing && !n.tail.is_empty() && !emptied);
    let has_content = !live.is_empty() || has_text;

    write_open_tag(doc, w, id, has_content, emptied)?;
    if !has_content && (n.self_closing || emptied) {
        return Ok(()); // the open tag already closed it
    }

    if live.is_empty() {
        match &n.text_override {
            Some(t) => w.write_all(escape_text(t).as_bytes())?,
            None => write_span(doc, w, n.tail)?,
        }
    } else {
        write_children(doc, w, n, &live, depth)?;
    }

    w.write_all(b"</")?;
    write_name(doc, w, n)?;
    w.write_all(b">")
}

fn write_children(
    doc: &Doc,
    w: &mut impl Write,
    n: &NodeData,
    live: &[NodeId],
    depth: usize,
) -> io::Result<()> {
    // Consecutive untouched children are adjacent in the source, so their leads
    // and spans collapse into a single write.
    let mut run: Option<Span> = None;
    for &c in live {
        let cn = doc.node(c);
        // A moved node's bytes are still good but its surroundings are not, so
        // it cannot join a contiguous run.
        if doc.is_pristine(c) && !cn.lead_synthetic {
            let s = Span { start: cn.lead.start, end: cn.span.end };
            run = match run {
                Some(prev) if prev.end == s.start => Some(Span { start: prev.start, end: s.end }),
                Some(prev) => {
                    write_span(doc, w, prev)?;
                    Some(s)
                }
                None => Some(s),
            };
            continue;
        }
        if let Some(prev) = run.take() {
            write_span(doc, w, prev)?;
        }
        // An existing child's lead is authoritative even when it is empty —
        // `<a><b/></a>` has no whitespace and must not grow any. Only a node
        // created or moved here has no source whitespace to speak of.
        if cn.lead_synthetic {
            write_break(doc, w, depth + 1)?;
        } else {
            write_span(doc, w, cn.lead)?;
        }
        write_node(doc, w, c, depth + 1)?;
    }
    if let Some(prev) = run.take() {
        write_span(doc, w, prev)?;
    }

    // A node that was self-closing has no tail to speak of, so an element that
    // just gained its first child needs a synthesised break before its end tag.
    if n.state == NodeState::Inserted || n.self_closing {
        write_break(doc, w, depth)
    } else {
        write_span(doc, w, n.tail)
    }
}

fn write_break(doc: &Doc, w: &mut impl Write, depth: usize) -> io::Result<()> {
    w.write_all(&doc.style.eol)?;
    for _ in 0..depth {
        w.write_all(&doc.style.indent)?;
    }
    Ok(())
}

fn write_name(doc: &Doc, w: &mut impl Write, n: &NodeData) -> io::Result<()> {
    match &n.name {
        Name::Src(s) => w.write_all(&doc.src[s.start as usize..s.end as usize]),
        Name::New(s) => w.write_all(s.as_bytes()),
    }
}

fn write_open_tag(
    doc: &Doc,
    w: &mut impl Write,
    id: NodeId,
    has_content: bool,
    emptied: bool,
) -> io::Result<()> {
    let n = doc.node(id);

    if n.state == NodeState::Inserted {
        w.write_all(b"<")?;
        write_name(doc, w, n)?;
        for a in n.attrs.iter().filter(|a| !a.removed) {
            let name = doc.attr_name(a);
            let value = a.value_override.as_deref().unwrap_or("");
            write!(w, " {}=\"{}\"", name, escape_attr(value))?;
        }
        return w.write_all(if has_content { b">" } else { b"/>" });
    }

    // An existing tag is spliced, not rebuilt: untouched attributes keep their
    // original spacing, quoting and escaping, so setting a value back to what it
    // was restores byte identity. The name is written first so that an element
    // with no attributes in the source still has somewhere to append one.
    w.write_all(b"<")?;
    write_name(doc, w, n)?;
    let mut pos = match &n.name {
        Name::Src(s) => s.end,
        Name::New(_) => n.open.start,
    };
    for a in &n.attrs {
        let AttrOrigin::Src { full, value, .. } = &a.origin else { continue };
        write_span(doc, w, Span { start: pos, end: full.start })?;
        if !a.removed {
            match &a.value_override {
                Some(v) => {
                    write_span(doc, w, Span { start: full.start, end: value.start })?;
                    w.write_all(escape_attr(v).as_bytes())?;
                    // The closing quote, whichever style the source used.
                    write_span(doc, w, Span { start: value.end, end: full.end })?;
                }
                None => write_span(doc, w, *full)?,
            }
        }
        pos = full.end;
    }

    for a in &n.attrs {
        let AttrOrigin::New { name } = &a.origin else { continue };
        if a.removed {
            continue;
        }
        let value = a.value_override.as_deref().unwrap_or("");
        write!(w, " {}=\"{}\"", name, escape_attr(value))?;
    }

    // Whatever closed the tag: optional whitespace then `>` or `/>`.
    let tail = &doc.src[pos as usize..n.open.end as usize];
    if n.self_closing && has_content {
        // Gained children, so it can no longer be self-closing.
        let trimmed: Vec<u8> = tail.iter().copied().filter(|b| *b != b'/').collect();
        w.write_all(&trimmed)
    } else if emptied && !has_content && !n.self_closing {
        // Lost its last child, so it closes itself again — the `>` becomes `/>`
        // and whatever spacing the source had before it is kept.
        w.write_all(&tail[..tail.len().saturating_sub(1)])?;
        w.write_all(b"/>")
    } else {
        w.write_all(tail)
    }
}

fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a string for use inside a double-quoted attribute value.
///
/// Public because anything that writes an Archi file from scratch has to escape
/// exactly the way this emitter does; a second copy of these rules elsewhere is
/// a file that Archi mis-reads waiting to happen.
pub fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            // Whitespace in an attribute value is normalised away by any
            // conforming parser unless it is written as a character reference.
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            '\t' => out.push_str("&#x9;"),
            _ => out.push(c),
        }
    }
    out
}
