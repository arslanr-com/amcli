//! A format-preserving XML tree.
//!
//! The one invariant that makes this crate worth existing:
//!
//! > **An untouched node is written back as the exact bytes it was parsed from.**
//!
//! Nothing is normalised, canonicalised or re-indented on the way out. Comments,
//! processing instructions, CDATA, DOCTYPE, unknown attributes, exotic whitespace
//! and the precise quoting of the XML declaration all survive a round trip for
//! free, because nothing ever parsed them semantically — they live inside byte
//! spans that get copied verbatim.
//!
//! Dirtiness does not propagate. Editing one attribute re-emits exactly one start
//! tag; that node's children, its siblings and the other 99.9% of the file are
//! still blitted straight out of the source buffer. The practical consequence is
//! that `git diff` after an edit shows only the lines that actually changed.
//!
//! This crate knows nothing about ArchiMate.

use std::io::{self, Write};
use std::sync::Arc;

mod emit;
mod parse;

pub use emit::escape_attr;
pub use parse::XmlError;

/// A byte range into the source buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    #[inline]
    pub fn new(start: usize, end: usize) -> Span {
        debug_assert!(start <= end);
        Span { start: start as u32, end: end as u32 }
    }

    #[inline]
    pub fn len(self) -> usize {
        (self.end - self.start) as usize
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.end == self.start
    }

    #[inline]
    fn slice(self, src: &[u8]) -> &[u8] {
        &src[self.start as usize..self.end as usize]
    }
}

/// Index of a node in the document arena.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct NodeId(u32);

impl NodeId {
    #[inline]
    fn idx(self) -> usize {
        self.0 as usize
    }
}

/// Whether a node's own start tag still matches the source bytes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeState {
    /// Untouched; emitted as a raw slice of the source.
    Pristine,
    /// An existing node whose attributes or text changed. Its start tag is
    /// re-emitted by splicing the original bytes, so unchanged attributes keep
    /// their original spacing and quoting.
    Dirty,
    /// A node created by this library; emitted from scratch.
    Inserted,
}

/// How the source formats things, so newly built nodes look like their neighbours.
#[derive(Clone, Debug)]
pub struct EmitStyle {
    /// One level of indentation, e.g. `b"    "` or `b"\t"`.
    pub indent: Vec<u8>,
    /// Line ending in use, `b"\n"` or `b"\r\n"`.
    pub eol: Vec<u8>,
}

impl Default for EmitStyle {
    fn default() -> Self {
        EmitStyle { indent: b"    ".to_vec(), eol: b"\n".to_vec() }
    }
}

#[derive(Clone, Debug)]
enum AttrOrigin {
    /// An attribute that came from the source.
    Src {
        /// ` name="value"` including the leading whitespace, so removal is clean.
        full: Span,
        name: Span,
        /// The raw value bytes between the quotes, entities intact.
        value: Span,
    },
    /// An attribute added after parsing.
    New { name: Box<str> },
}

#[derive(Clone, Debug)]
struct Attr {
    origin: AttrOrigin,
    /// `Some` when the value was replaced; needs escaping on the way out.
    value_override: Option<Box<str>>,
    removed: bool,
}

#[derive(Clone, Debug)]
struct NodeData {
    /// Qualified name as it appears in the source, e.g. `archimate:model`.
    name: Name,
    attrs: Vec<Attr>,
    children: Vec<NodeId>,
    parent: Option<NodeId>,
    /// Bytes between the previous sibling's end (or the parent's start tag) and
    /// this node's first `<`. Carries whitespace, comments and text.
    lead: Span,
    /// The whole element, `<` through the `>` of its end tag.
    span: Span,
    /// The start tag alone. For an empty element this is the same as `span`.
    open: Span,
    /// Bytes between the last child's end (or the start tag) and the end tag.
    /// For an element with no element children this is its character content.
    tail: Span,
    /// Written as `<a/>` in the source.
    self_closing: bool,
    /// Replacement character content; only valid when there are no children.
    text_override: Option<Box<str>>,
    state: NodeState,
    /// Set on this node and every ancestor when anything below changes, so the
    /// emitter knows it cannot blit the whole subtree. It does *not* force the
    /// ancestor's own start tag to be rebuilt — that is what `state` is for.
    subtree_dirty: bool,
    /// Detached by `remove_subtree`; skipped by the emitter and by traversal.
    removed: bool,
    /// The whitespace before this node has to be invented, because the node was
    /// created here or moved here and its original surroundings do not apply.
    /// Its own bytes are still valid, which is why a moved subtree stays
    /// byte-identical.
    lead_synthetic: bool,
}

#[derive(Clone, Debug)]
enum Name {
    Src(Span),
    New(Box<str>),
}

/// A parsed XML document that remembers its own bytes.
pub struct Doc {
    src: Arc<[u8]>,
    nodes: Vec<NodeData>,
    root: NodeId,
    /// Everything before the root's `<`: the declaration, leading comments, DOCTYPE.
    prologue: Span,
    /// Everything after the root's end tag.
    epilogue: Span,
    style: EmitStyle,
}

/// An edit was refused because it would have produced mixed content — an element
/// holding both character data and element children. Refusing is deliberate:
/// silently keeping one and dropping the other loses data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("element would end up with both text and element children")]
pub struct MixedContent;

/// A node to be created, built up before insertion.
pub struct NodeBuilder {
    name: Box<str>,
    attrs: Vec<(Box<str>, Box<str>)>,
    text: Option<Box<str>>,
}

impl NodeBuilder {
    pub fn new(name: impl Into<Box<str>>) -> NodeBuilder {
        NodeBuilder { name: name.into(), attrs: Vec::new(), text: None }
    }

    /// Attribute order is preserved exactly as pushed — callers are expected to
    /// push in the order the target schema writes them.
    pub fn attr(mut self, name: impl Into<Box<str>>, value: impl Into<Box<str>>) -> Self {
        self.attrs.push((name.into(), value.into()));
        self
    }

    pub fn text(mut self, text: impl Into<Box<str>>) -> Self {
        self.text = Some(text.into());
        self
    }
}

impl Doc {
    /// Parse a document, recording a byte span for every node and attribute.
    pub fn parse(src: impl Into<Arc<[u8]>>) -> Result<Doc, XmlError> {
        parse::parse(src.into())
    }

    /// Write the document out. Untouched regions are copied byte for byte.
    pub fn write_to(&self, w: &mut impl Write) -> io::Result<()> {
        emit::write_doc(self, w)
    }

    /// Convenience wrapper over [`Doc::write_to`].
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.src.len() + 64);
        self.write_to(&mut out).expect("writing to a Vec cannot fail");
        out
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn style(&self) -> &EmitStyle {
        &self.style
    }

    /// The original bytes this document was parsed from.
    pub fn source(&self) -> &[u8] {
        &self.src
    }

    #[inline]
    fn node(&self, id: NodeId) -> &NodeData {
        &self.nodes[id.idx()]
    }

    /// Qualified element name, e.g. `archimate:model`.
    pub fn name(&self, id: NodeId) -> &str {
        match &self.node(id).name {
            Name::Src(s) => str_of(s.slice(&self.src)),
            Name::New(s) => s,
        }
    }

    /// The local part of the name, with any namespace prefix stripped.
    pub fn local_name(&self, id: NodeId) -> &str {
        let n = self.name(id);
        match n.rfind(':') {
            Some(i) => &n[i + 1..],
            None => n,
        }
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).parent
    }

    /// Element children in document order. Text, comments and PIs are not nodes;
    /// they live in the byte spans between children and are preserved implicitly.
    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.node(id).children.iter().copied().filter(move |c| !self.node(*c).removed)
    }

    /// Translate a position among a node's *live* children into a position in
    /// the raw child vector.
    ///
    /// A removed node keeps its seat in that vector — `remove_subtree` only
    /// marks it — so the two numbering schemes drift apart the moment anything
    /// is deleted. Every caller counts with `children()`, which skips the
    /// removed, so insertion must translate or the new node lands as many
    /// places early as there are removed siblings before it. That is invisible
    /// until something is deleted and re-added in one session, which is exactly
    /// what a batch does.
    fn raw_child_index(&self, parent: NodeId, live_at: usize) -> usize {
        let kids = &self.node(parent).children;
        let mut live = 0;
        for (raw, c) in kids.iter().enumerate() {
            if live == live_at {
                return raw;
            }
            if !self.node(*c).removed {
                live += 1;
            }
        }
        kids.len()
    }

    /// First child whose qualified name matches.
    pub fn child_named(&self, id: NodeId, name: &str) -> Option<NodeId> {
        self.children(id).find(|c| self.name(*c) == name)
    }

    /// Depth-first pre-order walk of the subtree rooted at `id`, including `id`.
    pub fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            if self.node(n).removed {
                continue;
            }
            out.push(n);
            // Reversed so the walk comes out in document order.
            stack.extend(self.node(n).children.iter().rev().copied());
        }
        out
    }

    /// Raw attribute value, with XML entities left as they appear in the source.
    /// Use [`Doc::attr`] unless you specifically want the unescaped bytes.
    pub fn attr_raw(&self, id: NodeId, name: &str) -> Option<&str> {
        let a = self.find_attr(id, name)?;
        match (&a.value_override, &a.origin) {
            (Some(v), _) => Some(v),
            (None, AttrOrigin::Src { value, .. }) => Some(str_of(value.slice(&self.src))),
            (None, AttrOrigin::New { .. }) => Some(""),
        }
    }

    /// Attribute value with entities resolved.
    pub fn attr(&self, id: NodeId, name: &str) -> Option<String> {
        self.attr_raw(id, name).map(unescape)
    }

    /// Attribute names in document order, skipping removed ones.
    pub fn attr_names(&self, id: NodeId) -> Vec<&str> {
        self.node(id).attrs.iter().filter(|a| !a.removed).map(|a| self.attr_name(a)).collect()
    }

    /// Character content, with entities resolved. Empty for elements that have
    /// element children.
    pub fn text(&self, id: NodeId) -> String {
        let n = self.node(id);
        if let Some(t) = &n.text_override {
            return t.to_string();
        }
        if !n.children.iter().any(|c| !self.node(*c).removed) && !n.self_closing {
            // Children that have all been removed leave their indentation
            // behind, and that whitespace is not content: `to_bytes` writes
            // the element as the empty one it now is, so reporting the
            // indentation as text here would make the document say one thing
            // and write another. `has_text` has always taken this view — it
            // trims before deciding — and a node that never had children keeps
            // its text, whitespace and all.
            if !n.children.is_empty() {
                return String::new();
            }
            return unescape(str_of(n.tail.slice(&self.src)));
        }
        String::new()
    }

    /// The byte range this node occupies in the original source. Empty for nodes
    /// this library created.
    pub fn span(&self, id: NodeId) -> Span {
        let n = self.node(id);
        if n.state == NodeState::Inserted { Span::default() } else { n.span }
    }

    /// The node's original bytes, when it came from the source.
    pub fn source_bytes(&self, id: NodeId) -> Option<&[u8]> {
        let s = self.span(id);
        if s.is_empty() && self.node(id).state == NodeState::Inserted {
            return None;
        }
        Some(s.slice(&self.src))
    }

    /// 1-based line and column of a byte offset in the source. This is what lets
    /// a finding point at a place in the file instead of just naming an id.
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let upto = &self.src[..(offset as usize).min(self.src.len())];
        let line = 1 + upto.iter().filter(|b| **b == b'\n').count() as u32;
        let col = match upto.iter().rposition(|b| *b == b'\n') {
            Some(i) => (upto.len() - i) as u32,
            None => upto.len() as u32 + 1,
        };
        (line, col)
    }

    /// True if this node and everything under it is byte-identical to the source.
    pub fn is_pristine(&self, id: NodeId) -> bool {
        let n = self.node(id);
        n.state == NodeState::Pristine && !n.subtree_dirty && !n.removed
    }

    /// True if nothing in the document has been modified.
    pub fn is_unmodified(&self) -> bool {
        self.is_pristine(self.root)
    }

    fn attr_name<'a>(&'a self, a: &'a Attr) -> &'a str {
        match &a.origin {
            AttrOrigin::Src { name, .. } => str_of(name.slice(&self.src)),
            AttrOrigin::New { name } => name,
        }
    }

    fn find_attr(&self, id: NodeId, name: &str) -> Option<&Attr> {
        self.node(id).attrs.iter().find(|a| !a.removed && self.attr_name(a) == name)
    }

    fn find_attr_pos(&self, id: NodeId, name: &str) -> Option<usize> {
        self.node(id).attrs.iter().position(|a| !a.removed && self.attr_name(a) == name)
    }

    // ---- mutation -------------------------------------------------------

    /// Set an attribute. If it already exists the value is replaced in place, so
    /// the attribute keeps its position, and setting it back to its previous
    /// value restores byte identity. Otherwise it is appended.
    pub fn set_attr(&mut self, id: NodeId, name: &str, value: &str) {
        match self.find_attr_pos(id, name) {
            Some(i) => {
                // Writing the same value back is not a change.
                if self.attr_raw(id, name) == Some(value) {
                    return;
                }
                self.nodes[id.idx()].attrs[i].value_override = Some(value.into());
            }
            None => {
                self.nodes[id.idx()].attrs.push(Attr {
                    origin: AttrOrigin::New { name: name.into() },
                    value_override: Some(value.into()),
                    removed: false,
                });
            }
        }
        self.mark_dirty(id);
    }

    pub fn remove_attr(&mut self, id: NodeId, name: &str) {
        if let Some(i) = self.find_attr_pos(id, name) {
            self.nodes[id.idx()].attrs[i].removed = true;
            self.mark_dirty(id);
        }
    }

    /// Replace an element's character content.
    ///
    /// Refuses on an element that has element children rather than silently
    /// dropping one or the other: mixed content does not occur in ArchiMate
    /// models, so hitting this means a caller is confused about the node.
    pub fn set_text(&mut self, id: NodeId, text: &str) -> Result<(), MixedContent> {
        if self.children(id).next().is_some() {
            return Err(MixedContent);
        }
        if self.text(id) == text && !self.node(id).self_closing {
            return Ok(());
        }
        self.nodes[id.idx()].text_override = Some(text.into());
        self.mark_dirty(id);
        Ok(())
    }

    /// True if the element carries character content of its own.
    fn has_text(&self, id: NodeId) -> bool {
        let n = self.node(id);
        match &n.text_override {
            Some(t) => !t.is_empty(),
            None => {
                !n.self_closing
                    && self.children(id).next().is_none()
                    && !str_of(n.tail.slice(&self.src)).trim().is_empty()
            }
        }
    }

    /// Insert a child at `at` (clamped to the child count). Returns the new node.
    ///
    /// Refuses on an element that carries character content, for the same reason
    /// [`Doc::set_text`] refuses on an element with children.
    pub fn insert_child(
        &mut self,
        parent: NodeId,
        at: usize,
        b: NodeBuilder,
    ) -> Result<NodeId, MixedContent> {
        if self.has_text(parent) {
            return Err(MixedContent);
        }
        Ok(self.insert_child_unchecked(parent, at, b))
    }

    fn insert_child_unchecked(&mut self, parent: NodeId, at: usize, b: NodeBuilder) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        let attrs = b
            .attrs
            .into_iter()
            .map(|(name, value)| Attr {
                origin: AttrOrigin::New { name },
                value_override: Some(value),
                removed: false,
            })
            .collect();
        self.nodes.push(NodeData {
            name: Name::New(b.name),
            attrs,
            children: Vec::new(),
            parent: Some(parent),
            lead: Span::default(),
            span: Span::default(),
            open: Span::default(),
            tail: Span::default(),
            self_closing: b.text.is_none(),
            text_override: b.text,
            state: NodeState::Inserted,
            subtree_dirty: false,
            removed: false,
            lead_synthetic: true,
        });
        let at = self.raw_child_index(parent, at);
        self.nodes[parent.idx()].children.insert(at, id);
        self.mark_subtree_dirty(parent);
        id
    }

    /// Append a child. Equivalent to [`Doc::insert_child`] at the end.
    pub fn append_child(&mut self, parent: NodeId, b: NodeBuilder) -> Result<NodeId, MixedContent> {
        let at = self.nodes[parent.idx()].children.len();
        self.insert_child(parent, at, b)
    }

    /// Move a node, with everything under it, to a new parent.
    ///
    /// The node keeps its own bytes — only the whitespace around it is
    /// reinvented — so re-filing a concept cannot lose unknown attributes or
    /// unknown children the way rebuilding it from known fields would.
    pub fn move_child(&mut self, id: NodeId, new_parent: NodeId, at: usize) {
        if id == self.root || self.is_ancestor(id, new_parent) {
            return;
        }
        if let Some(old) = self.nodes[id.idx()].parent {
            self.nodes[old.idx()].children.retain(|c| *c != id);
            self.mark_subtree_dirty(old);
        }
        // After the removal above, so that moving within one parent indexes the
        // list the node is no longer in.
        let at = self.raw_child_index(new_parent, at);
        self.nodes[new_parent.idx()].children.insert(at, id);
        self.nodes[id.idx()].parent = Some(new_parent);
        self.nodes[id.idx()].lead_synthetic = true;
        self.mark_subtree_dirty(new_parent);
    }

    /// Guard against re-parenting a node under its own descendant.
    fn is_ancestor(&self, maybe_ancestor: NodeId, of: NodeId) -> bool {
        let mut cur = Some(of);
        while let Some(n) = cur {
            if n == maybe_ancestor {
                return true;
            }
            cur = self.nodes[n.idx()].parent;
        }
        false
    }

    /// Detach a node and everything under it. The whitespace that preceded it
    /// goes too, so removal never leaves an orphaned blank line behind.
    pub fn remove_subtree(&mut self, id: NodeId) {
        if id == self.root {
            panic!("cannot remove the root element");
        }
        self.nodes[id.idx()].removed = true;
        if let Some(p) = self.nodes[id.idx()].parent {
            self.mark_subtree_dirty(p);
        }
    }

    fn mark_dirty(&mut self, id: NodeId) {
        if self.nodes[id.idx()].state == NodeState::Pristine {
            self.nodes[id.idx()].state = NodeState::Dirty;
        }
        self.mark_subtree_dirty(id);
    }

    fn mark_subtree_dirty(&mut self, id: NodeId) {
        let mut cur = Some(id);
        while let Some(n) = cur {
            if self.nodes[n.idx()].subtree_dirty {
                break; // ancestors are already flagged
            }
            self.nodes[n.idx()].subtree_dirty = true;
            cur = self.nodes[n.idx()].parent;
        }
    }
}

/// Source bytes are validated as UTF-8 at parse time, so this is infallible in
/// practice; the fallback keeps a malformed slice from panicking mid-write.
#[inline]
fn str_of(b: &[u8]) -> &str {
    std::str::from_utf8(b).unwrap_or("")
}

/// Resolve the five predefined XML entities plus numeric character references.
fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(semi) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        let ent = &rest[1..semi];
        match ent {
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "amp" => out.push('&'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let cp = ent
                    .strip_prefix("#x")
                    .or_else(|| ent.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse::<u32>().ok()))
                    .and_then(char::from_u32);
                match cp {
                    Some(c) => out.push(c),
                    // Not an entity we know: pass it through untouched rather
                    // than silently dropping content.
                    None => out.push_str(&rest[..=semi]),
                }
            }
        }
        rest = &rest[semi + 1..];
    }
    out.push_str(rest);
    out
}
