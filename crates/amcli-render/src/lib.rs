//! SVG output.
//!
//! **What this promises.** Every figure lands on the bounds Archi recorded and
//! every connection on the polyline Archi computes. That is verifiable, and it
//! is what guarantees the picture reflects the file.
//!
//! **What it does not promise.** Pixel identity with Archi. That is not
//! achievable even in principle: Archi's default view font is the platform
//! system font, so its own export differs between macOS and Windows. Glyphs,
//! text wrapping and antialiasing will differ, and saying so plainly is better
//! than pretending otherwise.
//!
//! Output is byte-stable: fixed attribute order, two decimal places, LF
//! endings, no timestamps.

use std::fmt::Write;

use amcli_model::ElementType;
use amcli_view::geometry::{GROUP_HEADER, NOTE_DOG_EAR, Pt, Rect};
use amcli_view::icons::{ICON_BOX, ICON_RIGHT, ICON_TOP};
use amcli_view::notation::{BALL_RADIUS, deco_points};
use amcli_view::{Deco, Figure, Node, Scene, icons};

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Blank space around the content, in view units.
    pub margin: i32,
    /// Multiplies the pixel size; the viewBox is untouched, so this is purely a
    /// resolution knob.
    pub scale: f64,
    /// Pixel size for labels that carry no font of their own. Twelve is what
    /// Archi draws by default on every platform — Lucida Grande 12 on a Mac,
    /// Segoe UI 9 at 96 dpi on Windows — and what the layout sizes boxes for.
    pub font_size: f64,
    /// Emit `width`/`height`, so the file has an intrinsic size in a browser.
    pub sized: bool,
    /// Draw each element's type icon in its top-right corner, as Archi does.
    pub icons: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { margin: 10, scale: 1.0, font_size: 12.0, sized: true, icons: true }
    }
}

pub fn svg(scene: &Scene, o: &Options) -> String {
    let c = scene.content;
    let m = o.margin;
    // Negative coordinates need no normalising: the viewBox handles them, and
    // keeping model coordinates means an agent can line the SVG up against the
    // file it came from.
    let (vx, vy) = (c.x - m, c.y - m);
    let (vw, vh) = ((c.w + 2 * m).max(1), (c.h + 2 * m).max(1));

    let mut s = String::with_capacity(4096);
    s.push_str(r#"<svg xmlns="http://www.w3.org/2000/svg""#);
    if o.sized {
        let _ = write!(
            s,
            r#" width="{}" height="{}""#,
            num(vw as f64 * o.scale),
            num(vh as f64 * o.scale)
        );
    }
    let _ = write!(s, r#" viewBox="{vx} {vy} {vw} {vh}">"#);
    s.push('\n');
    let _ = writeln!(s, "  <title>{}</title>", esc(&scene.view_name));
    let _ = write!(
        s,
        r#"  <style>text{{font-family:Arial,Helvetica,sans-serif;font-size:{}px}}</style>"#,
        num(o.font_size)
    );
    s.push('\n');

    // One symbol per element type the scene draws, sorted, so the same scene
    // gives the same bytes and a figure's icon is a `<use>` rather than a copy.
    if o.icons {
        let mut types: Vec<ElementType> = scene
            .nodes
            .iter()
            .filter(|n| shows_icon(n))
            .filter_map(|n| ElementType::from_str(&n.type_name))
            .collect();
        types.sort();
        types.dedup();
        let symbols: Vec<String> = types.into_iter().filter_map(icons::symbol).collect();
        if !symbols.is_empty() {
            s.push_str("  <defs>\n");
            for sym in symbols {
                let _ = writeln!(s, "    {sym}");
            }
            s.push_str("  </defs>\n");
        }
    }

    // Nodes first, in tree pre-order, then every edge. In GEF the connection
    // layer sits above the primary layer, so an edge is never hidden behind a
    // box it happens to cross — getting this backwards is the most visible
    // possible mistake and costs nothing to get right.
    s.push_str("  <g class=\"nodes\">\n");
    for n in &scene.nodes {
        node(&mut s, n, o);
    }
    s.push_str("  </g>\n  <g class=\"edges\">\n");
    for e in &scene.edges {
        edge(&mut s, e, o);
    }
    s.push_str("  </g>\n</svg>\n");
    s
}

fn node(s: &mut String, n: &Node, o: &Options) {
    let r = n.abs;
    let fill = n.fill.hex();
    let line = n.line.hex();
    let opacity = n.alpha as f64 / 255.0;
    let line_opacity = n.line_alpha as f64 / 255.0;
    let common = format!(
        r#"fill="{fill}" fill-opacity="{}" stroke="{line}" stroke-opacity="{}" stroke-width="{}""#,
        num(opacity),
        num(line_opacity),
        n.line_width
    );

    let _ = write!(s, "    <g data-id=\"{}\"", esc(&n.id));
    if let Some(c) = &n.concept_id {
        let _ = write!(s, " data-concept=\"{}\"", esc(c));
    }
    let _ = writeln!(s, " data-type=\"{}\">", esc(&n.type_name));

    match n.figure {
        Figure::Rect => {
            let _ = write!(
                s,
                r#"      <rect x="{}" y="{}" width="{}" height="{}" {common}/>"#,
                r.x, r.y, r.w, r.h
            );
        }
        Figure::RoundedRect => {
            // Archi's behaviour figures use a 20x20 arc, which is rx=10.
            let _ = write!(
                s,
                r#"      <rect x="{}" y="{}" width="{}" height="{}" rx="10" ry="10" {common}/>"#,
                r.x, r.y, r.w, r.h
            );
        }
        Figure::Circle => {
            let _ = write!(
                s,
                r#"      <circle cx="{}" cy="{}" r="{}" {common}/>"#,
                r.x + r.w / 2,
                r.y + r.h / 2,
                r.w.min(r.h) / 2
            );
        }
        Figure::Octagon => {
            // INSET is 10 in Archi's motivation figure.
            const I: i32 = 10;
            let pts = [
                (r.x + I, r.y),
                (r.x + r.w - I, r.y),
                (r.x + r.w, r.y + I),
                (r.x + r.w, r.y + r.h - I),
                (r.x + r.w - I, r.y + r.h),
                (r.x + I, r.y + r.h),
                (r.x, r.y + r.h - I),
                (r.x, r.y + I),
            ];
            let _ = write!(s, r#"      <polygon points="{}" {common}/>"#, points(&pts));
        }
        // A note's border is the author's choice: the dog-eared corner,
        // a plain rectangle, or none at all.
        Figure::Note if n.border == 1 => {
            let _ = write!(
                s,
                r#"      <rect x="{}" y="{}" width="{}" height="{}" {common}/>"#,
                r.x, r.y, r.w, r.h
            );
        }
        Figure::Note if n.border == 2 => {
            let _ = write!(
                s,
                r#"      <rect x="{}" y="{}" width="{}" height="{}" fill="{fill}" fill-opacity="{}" stroke="none"/>"#,
                r.x,
                r.y,
                r.w,
                r.h,
                num(opacity)
            );
        }
        Figure::Note => {
            const D: i32 = NOTE_DOG_EAR;
            let pts = [
                (r.x, r.y),
                (r.x + r.w, r.y),
                (r.x + r.w, r.y + r.h - D),
                (r.x + r.w - D, r.y + r.h),
                (r.x, r.y + r.h),
            ];
            let _ = write!(s, r#"      <polygon points="{}" {common}/>"#, points(&pts));
        }
        // A group drawn as a plain rectangle, its name across the top.
        Figure::Tabbed if n.border == 1 => {
            let _ = write!(
                s,
                r#"      <rect x="{}" y="{}" width="{}" height="{}" {common}/>"#,
                r.x, r.y, r.w, r.h
            );
        }
        Figure::Tabbed => {
            // A tab across the top-left, then the body. Tab width is half the
            // figure, as Archi's GroupFigure computes it.
            let tab_w = (r.w / 2).max(40);
            let header = n.fill.darker().hex();
            let _ = writeln!(
                s,
                "      <rect x=\"{}\" y=\"{}\" width=\"{tab_w}\" height=\"{GROUP_HEADER}\" fill=\"{header}\" stroke=\"{line}\"/>",
                r.x, r.y
            );
            let _ = write!(
                s,
                r#"      <rect x="{}" y="{}" width="{}" height="{}" {common}/>"#,
                r.x,
                r.y + GROUP_HEADER,
                r.w,
                r.h - GROUP_HEADER
            );
        }
    }
    s.push('\n');

    if o.icons
        && shows_icon(n)
        && let Some(t) = ElementType::from_str(&n.type_name).filter(|t| icons::icon(*t).is_some())
    {
        let _ = writeln!(
            s,
            "      <use href=\"#i-{}\" x=\"{}\" y=\"{}\" width=\"{ICON_BOX}\" height=\"{ICON_BOX}\" color=\"{line}\"/>",
            t.info().short,
            r.x + r.w - ICON_RIGHT,
            r.y + ICON_TOP
        );
    }

    let text = if n.content.is_empty() { &n.label } else { &n.content };
    if !text.is_empty() {
        label(s, n, text, o);
    }
    s.push_str("    </g>\n");
}

/// Whether Archi would draw a type icon on this figure: an element with room
/// for one. Notes and groups have no type; a junction is its own icon; and a
/// figure smaller than the icon plus its margins would just be smudged.
fn shows_icon(n: &Node) -> bool {
    !matches!(n.figure, Figure::Note | Figure::Tabbed | Figure::Circle)
        && n.concept_id.is_some()
        && n.icon_visible
        && n.abs.w >= ICON_RIGHT + ICON_BOX
        && n.abs.h >= ICON_TOP + ICON_BOX + 4
}

/// Wrapped, clipped label text.
///
/// Character advance is approximated at 0.52 em rather than measured. That is
/// honest about the fidelity contract: exact text metrics would still not match
/// Archi, whose own output differs by platform, so an approximation that is
/// deterministic beats one that is merely more elaborate.
///
/// Where the line *breaks* is not an approximation, though, because that is
/// geometry rather than typography: Archi gives a label the box less its
/// margin, and less the type icon's width off both sides when the icon shows.
/// Wrapping inside the same width is what makes this drawing agree with the
/// one in Archi about how many lines a name takes.
/// The `font-size`, weight, style and colour attributes a text element gets
/// when the author chose them; nothing when the sheet's defaults apply.
fn font_attrs(font: Option<amcli_view::Font>, color: Option<amcli_view::Rgb>) -> String {
    let mut a = String::new();
    if let Some(f) = font {
        let _ = write!(a, r#" font-size="{}""#, num(f.size));
        if f.bold {
            a.push_str(r#" font-weight="bold""#);
        }
        if f.italic {
            a.push_str(r#" font-style="italic""#);
        }
    }
    if let Some(c) = color {
        let _ = write!(a, r#" fill="{}""#, c.hex());
    }
    a
}

fn label(s: &mut String, n: &Node, text: &str, o: &Options) {
    let r = n.abs;
    let pad = 5.0;
    // The author's font, when they chose one, sizes both the wrap and the
    // line height: a poster's twenty-point group titles are what make it
    // readable when it is fitted to a screen.
    let font_size = n.font.map(|f| f.size).unwrap_or(o.font_size);
    let attrs = font_attrs(n.font, n.font_color);
    // A Group's name belongs to its tab, and the tab is half the figure wide
    // and one line tall — so that, not the figure, is the box the text has to
    // fit. Measured against the whole width it wrapped to six lines and ran
    // out through the right edge of the tab and down across the body.
    let box_w = r.w as f64;
    let inset = match n.figure {
        Figure::Note | Figure::Tabbed => pad,
        _ => amcli_view::layout::ICON_INSET as f64,
    };
    let usable = (box_w - 2.0 * inset).max(10.0);
    let per_char = font_size * 0.52;
    let max_chars = (usable / per_char).floor().max(1.0) as usize;
    let line_h = font_size * 1.25;
    // Six lines is the cap for an ordinary box, whose text is allowed to run
    // a little past its edges rather than vanish; a tall figure — a poster's
    // note or legend — shows every line that fits its height.
    let fits = ((r.h as f64 - 2.0 * pad) / line_h).floor().max(0.0) as usize;
    let lines = wrap(text, max_chars, fits.max(6));

    let (anchor, tx) = match n.text_align {
        1 => ("start", r.x as f64 + pad),
        4 => ("end", (r.x + r.w) as f64 - pad),
        _ => ("middle", r.x as f64 + box_w / 2.0),
    };

    // A Group's text sits in its tab; a Note's starts at the top; everything
    // else is vertically centred in the figure.
    let top = match n.figure {
        // A Group's name goes in the body, not in the tab: the tab is half the
        // figure wide and one line tall, and a name of any length either ran
        // out through its right edge or had to be cut down to one word. The
        // body is the whole width and  makes it tall enough.
        // A rectangle group carries its name along the top, as Archi draws
        // it; a tabbed one in the body.
        Figure::Tabbed if n.border == 1 => r.y as f64 + pad,
        Figure::Tabbed => {
            let block = lines.len() as f64 * line_h;
            r.y as f64 + GROUP_HEADER as f64 + ((r.h - GROUP_HEADER) as f64 - block) / 2.0
        }
        Figure::Note => r.y as f64 + pad,
        // Where Archi's text position puts it: at the top, in the middle, or
        // along the bottom of the figure.
        _ => {
            let block = lines.len() as f64 * line_h;
            match n.text_position {
                0 => r.y as f64 + pad,
                2 => (r.y + r.h) as f64 - pad - block,
                _ => r.y as f64 + (r.h as f64 - block) / 2.0,
            }
        }
    };

    for (i, l) in lines.iter().enumerate() {
        let y = top + (i as f64 + 0.8) * line_h;
        let _ = writeln!(
            s,
            "      <text x=\"{}\" y=\"{}\" text-anchor=\"{anchor}\"{attrs}>{}</text>",
            num(tx),
            num(y),
            esc(l)
        );
    }
}

fn wrap(text: &str, max_chars: usize, max_lines: usize) -> Vec<String> {
    let max = max_lines;
    let mut out: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if line.is_empty() {
                line = word.to_string();
            } else if line.chars().count() + 1 + word.chars().count() <= max_chars {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            }
            if out.len() >= max {
                break;
            }
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    if out.len() > max {
        out.truncate(max);
        if let Some(last) = out.last_mut() {
            last.push('…');
        }
    }
    out
}

fn edge(s: &mut String, e: &amcli_view::Edge, o: &Options) {
    if e.points.len() < 2 {
        return;
    }
    let line = e.line.hex();
    let dash = match e.dash {
        Some(d) => format!(r#" stroke-dasharray="{d}""#),
        None => String::new(),
    };
    let _ = write!(s, "    <g data-id=\"{}\"", esc(&e.id));
    if let Some(r) = &e.relationship_id {
        let _ = write!(s, " data-relationship=\"{}\"", esc(r));
    }
    s.push_str(">\n");
    let _ = write!(
        s,
        r#"      <polyline points="{}" fill="none" stroke="{line}" stroke-width="{}"{dash}/>"#,
        points_pt(&e.points),
        e.line_width
    );
    s.push('\n');

    // A decoration points along the line it terminates, so it needs the
    // neighbouring point to know which way is "back".
    let n = e.points.len();
    decoration(s, e.source_deco, e.points[0], e.points[1], &line);
    decoration(s, e.target_deco, e.points[n - 1], e.points[n - 2], &line);

    if !e.label.is_empty() {
        // Halfway along the line, one text per line of the label; an author's
        // multi-line caption stays multi-line.
        let (a, b) = (e.points[(n - 1) / 2], e.points[n / 2]);
        let (mx, my) = ((a.x + b.x) as f64 / 2.0, (a.y + b.y) as f64 / 2.0);
        let attrs = font_attrs(e.font, e.font_color);
        let line_h = e.font.map(|f| f.size).unwrap_or(o.font_size) * 1.25;
        for (i, l) in e.label.split('\n').enumerate() {
            let _ = writeln!(
                s,
                "      <text x=\"{}\" y=\"{}\" text-anchor=\"middle\"{attrs}>{}</text>",
                num(mx),
                num(my - 4.0 + i as f64 * line_h),
                esc(l)
            );
        }
    }
    let _ = writeln!(s, "    </g>");
}

fn decoration(s: &mut String, d: Deco, tip: Pt, back: Pt, line: &str) {
    if d == Deco::None {
        return;
    }
    if d == Deco::Ball {
        let _ = writeln!(
            s,
            "      <circle cx=\"{}\" cy=\"{}\" r=\"{}\" fill=\"{line}\" stroke=\"{line}\"/>",
            tip.x,
            tip.y,
            num(BALL_RADIUS)
        );
        return;
    }

    let (template, filled) = deco_points(d);
    if template.is_empty() {
        return;
    }
    // Rotate the template so +x runs back along the line.
    let dx = (back.x - tip.x) as f64;
    let dy = (back.y - tip.y) as f64;
    let len = (dx * dx + dy * dy).sqrt();
    if len < f64::EPSILON {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let mapped: Vec<(f64, f64)> = template
        .iter()
        .map(|(px, py)| {
            // The template points back along -x, so negating puts it on the
            // incoming direction.
            (tip.x as f64 - px * ux + py * uy, tip.y as f64 - px * uy - py * ux)
        })
        .collect();

    let fill = match d {
        Deco::HollowDiamond | Deco::HollowTriangle => "#ffffff",
        _ => line,
    };
    if filled {
        let _ = writeln!(
            s,
            "      <polygon points=\"{}\" fill=\"{fill}\" stroke=\"{line}\"/>",
            points_f(&mapped)
        );
    } else {
        let _ = writeln!(
            s,
            "      <polyline points=\"{}\" fill=\"none\" stroke=\"{line}\"/>",
            points_f(&mapped)
        );
    }
}

fn points(p: &[(i32, i32)]) -> String {
    p.iter().map(|(x, y)| format!("{x},{y}")).collect::<Vec<_>>().join(" ")
}

fn points_pt(p: &[Pt]) -> String {
    p.iter().map(|p| format!("{},{}", p.x, p.y)).collect::<Vec<_>>().join(" ")
}

fn points_f(p: &[(f64, f64)]) -> String {
    p.iter().map(|(x, y)| format!("{},{}", num(*x), num(*y))).collect::<Vec<_>>().join(" ")
}

/// Two decimals, with `-0` normalised, so output is byte-stable.
fn num(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "-0" { "0".to_string() } else { s }
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// A machine-readable dump of everything the renderer used.
///
/// Cheap to produce and the most useful output of the lot for an agent: it can
/// answer "do these two boxes overlap" without rasterising anything.
pub fn scene_json(scene: &Scene) -> String {
    let mut s = String::from("{");
    let _ =
        write!(s, r#""view":{{"id":{},"name":{}"#, jstr(&scene.view_id), jstr(&scene.view_name));
    let _ = write!(s, r#","viewpoint":{}}}"#, jstr(&scene.viewpoint));
    let _ = write!(
        s,
        r#","content":{{"x":{},"y":{},"w":{},"h":{}}}"#,
        scene.content.x, scene.content.y, scene.content.w, scene.content.h
    );
    s.push_str(r#","nodes":["#);
    for (i, n) in scene.nodes.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            r#"{{"id":{},"concept":{},"type":{},"label":{},"x":{},"y":{},"w":{},"h":{},"depth":{},"fill":{}}}"#,
            jstr(&n.id),
            n.concept_id.as_deref().map(jstr).unwrap_or_else(|| "null".into()),
            jstr(&n.type_name),
            jstr(&n.label),
            n.abs.x,
            n.abs.y,
            n.abs.w,
            n.abs.h,
            n.depth,
            jstr(&n.fill.hex())
        );
    }
    s.push_str(r#"],"edges":["#);
    for (i, e) in scene.edges.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let pts: Vec<String> = e.points.iter().map(|p| format!("[{},{}]", p.x, p.y)).collect();
        let _ = write!(
            s,
            r#"{{"id":{},"relationship":{},"points":[{}]}}"#,
            jstr(&e.id),
            e.relationship_id.as_deref().map(jstr).unwrap_or_else(|| "null".into()),
            pts.join(",")
        );
    }
    s.push_str("],\"warnings\":[");
    for (i, w) in scene.warnings.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(&jstr(w));
    }
    s.push_str("]}");
    s
}

fn jstr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Bounds of a rectangle, for callers that want to reason about the scene.
pub fn bbox(scene: &Scene) -> Rect {
    scene.content
}

/// The same drawing as [`svg`], rasterised. `o.scale` is the resolution:
/// 2.0 gives a picture twice the size of the view in pixels.
///
/// Labels are set in whatever sans-serif the machine has (Arial or Helvetica
/// where they exist, else the system's own), which is also what Archi does;
/// a machine with no fonts at all gets boxes and lines but no text, and the
/// error says so rather than pretending.
pub fn png(scene: &Scene, o: &Options) -> Result<Vec<u8>, String> {
    let markup = svg(scene, &Options { sized: true, scale: 1.0, ..*o });
    let mut opt = resvg::usvg::Options::default();
    {
        let db = opt.fontdb_mut();
        db.load_system_fonts();
        db.set_sans_serif_family("Arial");
        if db.faces().next().is_none() {
            return Err(
                "no fonts are installed, so labels cannot be drawn; render to SVG instead".into()
            );
        }
    }
    opt.font_family = "Arial".into();
    let tree = resvg::usvg::Tree::from_data(markup.as_bytes(), &opt).map_err(|e| e.to_string())?;
    let size = tree.size();
    let scale = if o.scale > 0.0 { o.scale as f32 } else { 1.0 };
    let (w, h) = ((size.width() * scale).ceil() as u32, (size.height() * scale).ceil() as u32);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w.max(1), h.max(1))
        .ok_or_else(|| format!("{w}x{h} is too large to rasterise"))?;
    pixmap.fill(resvg::tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().map_err(|e| e.to_string())
}

/// Every type icon as one `<svg><defs>…</defs></svg>`, for a page that draws
/// its own figures and wants them to carry the same icons a rendered view does.
pub fn icon_defs() -> String {
    let mut s = String::from(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"0\" height=\"0\">\n  <defs>\n",
    );
    for e in ElementType::ALL {
        if let Some(sym) = icons::symbol(e) {
            let _ = writeln!(s, "    {sym}");
        }
    }
    s.push_str("  </defs>\n</svg>\n");
    s
}
