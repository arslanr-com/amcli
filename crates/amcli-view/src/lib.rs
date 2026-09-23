//! Turning a stored view into something drawable.
//!
//! The geometry is already in the file — Archi records every bound and every
//! bendpoint — so nothing here lays anything out. Compiling a view is resolving
//! what is written into absolute coordinates: parent-relative origins summed,
//! `-1` sizes replaced by the figure default, and relative bendpoints turned
//! into a polyline.
//!
//! Laying out a *new* view is a separate job; see [`layout`].

pub mod geometry;
pub mod icons;
pub mod layout;
pub mod notation;

use amcli_model::{ConceptKind, ElementType, Model, RelType, ViewId};
use amcli_xml::NodeId;

pub use geometry::{Bendpoint, Pt, Rect};
pub use notation::{Deco, Figure, Rgb};

/// A view resolved into absolute coordinates and concrete styling. Nothing
/// downstream needs the model or the document again.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub view_id: String,
    pub view_name: String,
    pub viewpoint: String,
    /// Bounding box of everything drawn.
    pub content: Rect,
    /// Painter's order: tree pre-order, so a child covers its parent.
    pub nodes: Vec<Node>,
    /// Drawn after every node. In GEF the connection layer sits above the
    /// primary layer, so an edge is never hidden behind a box it crosses.
    pub edges: Vec<Edge>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: String,
    /// The concept this displays, when it displays one.
    pub concept_id: Option<String>,
    /// The object this one is drawn inside, when it is nested.
    pub parent_id: Option<String>,
    pub figure: Figure,
    /// What is written on the figure: the concept's name, unless a label
    /// expression on the object says otherwise.
    pub label: String,
    /// A note's body text.
    pub content: String,
    pub abs: Rect,
    pub depth: usize,
    pub fill: Rgb,
    pub line: Rgb,
    /// 0..255; Archi tracks fill and line opacity separately.
    pub alpha: u8,
    pub line_alpha: u8,
    pub line_width: u32,
    /// 1 left, 2 centre, 4 right.
    pub text_align: u8,
    /// 0 top, 1 centre, 2 bottom — where the label sits in the figure.
    pub text_position: u8,
    /// False when the object's `iconVisible` feature hides the type icon.
    pub icon_visible: bool,
    /// The font the author chose for this object, when they chose one.
    pub font: Option<Font>,
    /// The colour of its text, when the author set one.
    pub font_color: Option<Rgb>,
    /// Archi's `borderType`: on a Group 0 is tabbed and 1 a plain rectangle;
    /// on a Note 0 is the dog-eared corner, 1 a rectangle, 2 no border.
    pub border: u8,
    pub type_name: String,
}

/// A font as Archi stores it on an object: `1|Arial|19.0|1|COCOA|1|` is
/// version, face, height in points, SWT style (1 bold, 2 italic, 3 both),
/// the platform it was set on, and a flag. Only the size and the style
/// matter to a drawing; the face is whatever the machine has.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    /// Height in drawing pixels — what the author saw. Archi stores points,
    /// and a point is a pixel on macOS (72 dpi) but four thirds of one on
    /// Windows and GTK (96 dpi): Segoe UI 9 on Windows and Lucida Grande 12
    /// on a Mac are the same twelve pixels, which is also Archi's default
    /// on both. The platform field says which arithmetic applies.
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
}

impl Font {
    /// Parse Archi's font string; `None` when it is absent or unreadable.
    pub fn parse(s: &str) -> Option<Font> {
        let mut parts = s.split('|');
        let _version = parts.next()?;
        let _face = parts.next()?;
        let points: f64 = parts.next()?.trim().parse().ok()?;
        let style: u8 = parts.next().and_then(|v| v.trim().parse().ok()).unwrap_or(0);
        let platform = parts.next().unwrap_or("").trim().to_ascii_uppercase();
        let size = match platform.as_str() {
            "WINDOWS" | "GTK" | "WIN32" => (points * 96.0 / 72.0).round(),
            _ => points,
        };
        (size > 0.0).then_some(Font { size, bold: style & 1 != 0, italic: style & 2 != 0 })
    }
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub id: String,
    pub relationship_id: Option<String>,
    pub label: String,
    /// The font and colour the author gave the label, when they did.
    pub font: Option<Font>,
    pub font_color: Option<Rgb>,
    /// Where along the line the label sits, as Archi's connection text
    /// position says: 0 by the source, 1 halfway, 2 by the target.
    pub text_position: u8,
    pub points: Vec<Pt>,
    pub dash: Option<&'static str>,
    pub source_deco: Deco,
    pub target_deco: Deco,
    pub line: Rgb,
    pub line_width: u32,
}

/// Resolve a view into a scene.
pub fn compile(m: &Model, view: ViewId) -> Scene {
    let v = m.view(view);
    let mut scene = Scene {
        view_id: v.id.clone(),
        view_name: v.name.clone(),
        viewpoint: v.viewpoint.clone(),
        ..Default::default()
    };

    // Objects are indexed by id while walking, because a connection names its
    // endpoints by id and may be declared before either of them.
    let mut bounds_of: std::collections::HashMap<String, Rect> = Default::default();
    let children: Vec<NodeId> = m.doc.children(v.node).collect();
    for c in children {
        walk(m, c, 0, 0, 0, None, &mut scene, &mut bounds_of);
    }

    collect_edges(m, v.node, &bounds_of, &mut scene);

    scene.content = scene
        .nodes
        .iter()
        .map(|n| n.abs)
        .chain(
            scene
                .edges
                .iter()
                .flat_map(|e| e.points.iter().map(|p| Rect { x: p.x, y: p.y, w: 0, h: 0 })),
        )
        .reduce(|a, b| a.union(b))
        .unwrap_or_default();

    scene
}

#[allow(clippy::too_many_arguments)] // a tree walk carries its origin, depth and parent
fn walk(
    m: &Model,
    node: NodeId,
    ox: i32,
    oy: i32,
    depth: usize,
    parent_id: Option<&str>,
    scene: &mut Scene,
    bounds_of: &mut std::collections::HashMap<String, Rect>,
) {
    if m.doc.local_name(node) != "child" {
        return;
    }
    let id = m.doc.attr(node, "id").unwrap_or_default();
    let xsi = m.doc.attr(node, "xsi:type").unwrap_or_default();
    let bare = xsi.trim_start_matches("archimate:");
    let concept_id = m.doc.attr(node, "archimateElement");
    let concept = concept_id.as_deref().and_then(|c| m.concept_by_id(c)).map(|c| m.concept(c));
    let features = m.features(node);
    let feature = |name: &str| features.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());

    let (dw, dh) = default_size(bare, concept.map(|c| &c.kind));
    let b = read_bounds(m, node, dw, dh);
    // Child coordinates are relative to the parent's origin.
    let abs = Rect { x: ox + b.x, y: oy + b.y, w: b.w, h: b.h };

    let (figure, fill) = match (bare, concept.map(|c| &c.kind)) {
        (_, Some(ConceptKind::Element(e))) => {
            let f = if *e == ElementType::Junction {
                notation::BLACK
            } else {
                notation::layer_fill(e.info().layer)
            };
            (notation::figure_of(*e), f)
        }
        ("Note", _) => (Figure::Note, notation::WHITE),
        ("Group", _) => (Figure::Tabbed, notation::WHITE),
        _ => (Figure::Rect, notation::WHITE),
    };

    let explicit_fill = m.doc.attr(node, "fillColor").and_then(|s| parse_hex(&s));
    let fill = explicit_fill.unwrap_or(fill);
    // An element's border is derived from its fill unless the object's
    // `deriveElementLineColor` feature says otherwise — Archi ignores an
    // explicit `lineColor` on an element until that feature is `false`. A
    // note or a group has no derived colour, so its own is always used.
    let explicit_line = m.doc.attr(node, "lineColor").and_then(|s| parse_hex(&s)).filter(|_| {
        concept.is_none()
            || matches!(figure, Figure::Note | Figure::Tabbed)
            || feature("deriveElementLineColor") == Some("false")
    });
    let line = explicit_line.unwrap_or_else(|| match figure {
        Figure::Note | Figure::Tabbed => notation::DEFAULT_LINE,
        _ => fill.derived_line(),
    });

    let content = m.doc.child_named(node, "content").map(|n| m.doc.text(n)).unwrap_or_default();
    // A note has no name; what it says is its label.
    let name = concept
        .map(|c| c.name.clone())
        .or_else(|| m.doc.attr(node, "name"))
        .or_else(|| (!content.is_empty()).then(|| content.clone()));
    // A label expression, as jArchi and Archi 4.8+ write it, replaces the
    // name on the figure: `${name}` and friends expand, anything else is the
    // literal text the author typed. Only an absent or empty expression falls
    // back to the name — a blank one is a blank label, exactly as Archi draws
    // it, which is how a poster hides a name it does not want printed.
    let label = match feature("labelExpression").filter(|e| !e.is_empty()) {
        Some(expr) => expand_label(
            expr,
            &LabelContext {
                name: name.as_deref().unwrap_or_default(),
                documentation: concept.and_then(|c| m.documentation(c.node)).unwrap_or_default(),
                type_name: concept.map(|c| c.kind.name()).unwrap_or(bare),
                properties: concept.map(|c| m.properties(c.node)).unwrap_or_default(),
                view_name: &scene.view_name,
                model_name: &m.name(),
            },
        ),
        None => name.unwrap_or_default(),
    };

    scene.nodes.push(Node {
        id: id.clone(),
        concept_id: concept_id.clone(),
        parent_id: parent_id.map(str::to_string),
        figure,
        label,
        content,
        abs,
        depth,
        fill,
        line,
        alpha: m.doc.attr(node, "alpha").and_then(|s| s.parse().ok()).unwrap_or(255),
        line_alpha: m.doc.attr(node, "lineAlpha").and_then(|s| s.parse().ok()).unwrap_or(255),
        line_width: m.doc.attr(node, "lineWidth").and_then(|s| s.parse().ok()).unwrap_or(1),
        text_align: m.doc.attr(node, "textAlignment").and_then(|s| s.parse().ok()).unwrap_or(2),
        // Archi leaves the attribute out at its default. For a group that is
        // the top — its name sits under the tab — and for anything else this
        // renderer keeps the middle it has always drawn.
        text_position: m
            .doc
            .attr(node, "textPosition")
            .and_then(|s| s.parse().ok())
            .unwrap_or(if figure == Figure::Tabbed { 0 } else { 1 }),
        // 0 shows the icon unless an image replaces it, 1 always, 2 never.
        icon_visible: feature("iconVisible") != Some("2"),
        font: m.doc.attr(node, "font").and_then(|f| Font::parse(&f)),
        font_color: m.doc.attr(node, "fontColor").and_then(|s| parse_hex(&s)),
        border: m.doc.attr(node, "borderType").and_then(|s| s.parse().ok()).unwrap_or(0),
        type_name: concept.map(|c| c.kind.name().to_string()).unwrap_or_else(|| bare.to_string()),
    });
    bounds_of.insert(id.clone(), abs);

    for c in m.doc.children(node).collect::<Vec<_>>() {
        walk(m, c, abs.x, abs.y, depth + 1, Some(&id), scene, bounds_of);
    }
}

/// What a label expression may refer to.
pub struct LabelContext<'a> {
    pub name: &'a str,
    pub documentation: String,
    pub type_name: &'a str,
    pub properties: Vec<(String, String)>,
    pub view_name: &'a str,
    pub model_name: &'a str,
}

/// Expand an Archi label expression.
///
/// The expressions Archi's own label editor offers are all `${…}`: `name`,
/// `documentation`, `type`, `property:KEY`, `view:name`, `model:name`, and
/// `viewpoint`. Those are resolved; a reference this build does not know is
/// left as written, because a label that shows `${something}` is a label the
/// author can see went wrong, and one silently blanked is not. Text outside a
/// reference is kept verbatim, newlines included — that is how a poster's
/// multi-line captions are written.
pub fn expand_label(expr: &str, ctx: &LabelContext<'_>) -> String {
    let mut out = String::with_capacity(expr.len());
    let mut rest = expr;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[start..]);
            return out;
        };
        let key = &after[..end];
        let value = match key {
            "name" => Some(ctx.name.to_string()),
            "documentation" | "doc" => Some(ctx.documentation.clone()),
            "type" => Some(ctx.type_name.to_string()),
            "view:name" => Some(ctx.view_name.to_string()),
            "model:name" => Some(ctx.model_name.to_string()),
            k => k.strip_prefix("property:").map(|p| {
                ctx.properties
                    .iter()
                    .find(|(k, _)| k == p)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            }),
        };
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[start..start + 2 + end + 1]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn collect_edges(
    m: &Model,
    view_node: NodeId,
    bounds_of: &std::collections::HashMap<String, Rect>,
    scene: &mut Scene,
) {
    // Who holds whom, so a line between a container and what it holds can be
    // left undrawn, as Archi leaves it: the nesting already says it.
    let parent_of: std::collections::HashMap<&str, &str> =
        scene.nodes.iter().filter_map(|n| Some((n.id.as_str(), n.parent_id.as_deref()?))).collect();
    for n in m.doc.descendants(view_node) {
        if m.doc.local_name(n) != "sourceConnection" {
            continue;
        }
        let id = m.doc.attr(n, "id").unwrap_or_default();
        let src = m.doc.attr(n, "source").unwrap_or_default();
        let tgt = m.doc.attr(n, "target").unwrap_or_default();
        if parent_of.get(src.as_str()) == Some(&tgt.as_str())
            || parent_of.get(tgt.as_str()) == Some(&src.as_str())
        {
            continue;
        }
        let rel_id = m.doc.attr(n, "archimateRelationship");
        let expression = m
            .features(n)
            .into_iter()
            .find(|(k, _)| k == "labelExpression")
            .map(|(_, v)| v)
            .filter(|e| !e.is_empty());

        // A connection may end on another connection. Those have no bounds of
        // their own, so the edge is skipped rather than drawn to the origin.
        let (Some(sb), Some(tb)) = (bounds_of.get(&src), bounds_of.get(&tgt)) else {
            scene
                .warnings
                .push(format!("connection {id} ends on something with no bounds; not drawn"));
            continue;
        };

        let bendpoints: Vec<Bendpoint> = m
            .doc
            .children(n)
            .filter(|c| m.doc.local_name(*c) == "bendpoint")
            .map(|c| Bendpoint {
                start_x: attr_i32(m, c, "startX"),
                start_y: attr_i32(m, c, "startY"),
                end_x: attr_i32(m, c, "endX"),
                end_y: attr_i32(m, c, "endY"),
            })
            .collect();

        let points = geometry::route(*sb, *tb, &bendpoints);
        if points.first() == points.last() && bendpoints.is_empty() {
            scene
                .warnings
                .push(format!("connection {id} is a self-loop with no bendpoints; it draws as a point, which is what Archi does"));
        }

        let rel = rel_id.as_deref().and_then(|r| m.concept_by_id(r)).map(|c| m.concept(c));
        let (rel_type, access, directed, label) = match rel {
            Some(c) => (
                match &c.kind {
                    ConceptKind::Relationship(r) => Some(*r),
                    _ => None,
                },
                m.doc.attr(c.node, "accessType").and_then(|s| s.parse::<i64>().ok()),
                m.doc.attr(c.node, "directed").as_deref() == Some("true"),
                c.name.clone(),
            ),
            None => (None, None, false, String::new()),
        };

        let style = rel_type
            .map(|r| notation::rel_style(r, access, directed))
            .unwrap_or(notation::RelStyle { dash: None, source: Deco::None, target: Deco::None });
        let label = match expression {
            Some(expr) => expand_label(
                &expr,
                &LabelContext {
                    name: &label,
                    documentation: rel.and_then(|c| m.documentation(c.node)).unwrap_or_default(),
                    type_name: rel.map(|c| c.kind.name()).unwrap_or_default(),
                    properties: rel.map(|c| m.properties(c.node)).unwrap_or_default(),
                    view_name: &scene.view_name,
                    model_name: &m.name(),
                },
            ),
            None => label,
        };

        scene.edges.push(Edge {
            id,
            relationship_id: rel_id,
            label,
            font: m.doc.attr(n, "font").and_then(|f| Font::parse(&f)),
            font_color: m.doc.attr(n, "fontColor").and_then(|s| parse_hex(&s)),
            text_position: m.doc.attr(n, "textPosition").and_then(|s| s.parse().ok()).unwrap_or(1),
            points,
            dash: style.dash,
            source_deco: style.source,
            target_deco: style.target,
            line: m
                .doc
                .attr(n, "lineColor")
                .and_then(|s| parse_hex(&s))
                .unwrap_or(notation::DEFAULT_LINE),
            line_width: m.doc.attr(n, "lineWidth").and_then(|s| s.parse().ok()).unwrap_or(1),
        });
    }
}

fn attr_i32(m: &Model, n: NodeId, name: &str) -> i32 {
    m.doc.attr(n, name).and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// `<bounds>` is a child element, and `-1` means "the figure's default size".
fn read_bounds(m: &Model, node: NodeId, dw: i32, dh: i32) -> Rect {
    let Some(b) = m.doc.child_named(node, "bounds") else {
        return Rect { x: 0, y: 0, w: dw, h: dh };
    };
    let w = attr_or(m, b, "width", -1);
    let h = attr_or(m, b, "height", -1);
    Rect {
        x: attr_or(m, b, "x", 0),
        y: attr_or(m, b, "y", 0),
        w: if w >= 0 { w } else { dw },
        h: if h >= 0 { h } else { dh },
    }
}

fn attr_or(m: &Model, n: NodeId, name: &str, default: i32) -> i32 {
    m.doc.attr(n, name).and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn default_size(bare: &str, kind: Option<&ConceptKind>) -> (i32, i32) {
    if let Some(ConceptKind::Element(e)) = kind {
        return e.info().default_wh;
    }
    match bare {
        "Note" => geometry::NOTE_SIZE,
        "Group" => geometry::GROUP_SIZE,
        "DiagramModelImage" => geometry::IMAGE_SIZE,
        _ => geometry::ELEMENT_SIZE,
    }
}

fn parse_hex(s: &str) -> Option<Rgb> {
    let s = s.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    Some(Rgb(
        u8::from_str_radix(&s[0..2], 16).ok()?,
        u8::from_str_radix(&s[2..4], 16).ok()?,
        u8::from_str_radix(&s[4..6], 16).ok()?,
    ))
}

/// Relationship types that carry containment, used when deciding what a
/// generated view should nest.
pub const NESTING: [RelType; 2] = [RelType::Composition, RelType::Aggregation];

#[cfg(test)]
mod label_tests {
    use super::{LabelContext, expand_label};

    fn ctx() -> LabelContext<'static> {
        LabelContext {
            name: "Payments",
            documentation: "Takes the money.".into(),
            type_name: "ApplicationComponent",
            properties: vec![("owner".into(), "Team A".into())],
            view_name: "V",
            model_name: "M",
        }
    }

    #[test]
    fn the_references_archi_offers_expand_and_the_rest_stays_literal() {
        assert_eq!(expand_label("${name}", &ctx()), "Payments");
        assert_eq!(expand_label("${name}\n${type}", &ctx()), "Payments\nApplicationComponent");
        assert_eq!(expand_label("Owner: ${property:owner}", &ctx()), "Owner: Team A");
        assert_eq!(expand_label("${property:none}!", &ctx()), "!");
        assert_eq!(
            expand_label("${documentation} (${view:name}/${model:name})", &ctx()),
            "Takes the money. (V/M)"
        );
        assert_eq!(expand_label("${specialization}", &ctx()), "${specialization}");
        assert_eq!(expand_label("plain text", &ctx()), "plain text");
        assert_eq!(expand_label("${unclosed", &ctx()), "${unclosed");
    }
}
