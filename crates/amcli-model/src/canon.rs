//! A canonical form for comparing subtrees across files.
//!
//! Two saves of the same model are rarely the same bytes: Archi writes
//! attributes in metamodel order where a hand or a tool may not, omits an
//! attribute whose value is the schema default, spells `>` as `&gt;` in one
//! place and not another, and indents however it indents. None of that is a
//! change to the model, and a diff or a merge that counted it would report
//! every block as changed after the first save in Archi.
//!
//! [`Canon`] is the shape a node has once that noise is gone: the element
//! name, its attributes sorted by name with entities resolved and defaults
//! dropped, its character content resolved the same way, and its children in
//! order — order is meaning for a view's objects and a concept's properties,
//! so it stays. Two nodes with equal canonical forms mean the same thing to
//! Archi; two with different ones differ in something a reader would want
//! to hear about, and [`describe`] says what, in a phrase.

use amcli_xml::{Doc, NodeId};

/// A node with the serialisation noise removed. See the module notes.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Canon {
    pub name: String,
    /// Sorted by attribute name; values with entities resolved.
    pub attrs: Vec<(String, String)>,
    pub text: String,
    pub children: Vec<Canon>,
}

/// Attributes EMF leaves out when they hold the schema default, so a file
/// that writes one explicitly and a file that omits it mean the same thing.
/// `(element local name, attribute, default)`.
const DEFAULTS: &[(&str, &str, &str)] = &[
    ("bounds", "x", "0"),
    ("bounds", "y", "0"),
    ("bounds", "width", "-1"),
    ("bounds", "height", "-1"),
    ("bendpoint", "startX", "0"),
    ("bendpoint", "startY", "0"),
    ("bendpoint", "endX", "0"),
    ("bendpoint", "endY", "0"),
    // 0 is Write, and Write is what an Access relationship is unless said.
    ("element", "accessType", "0"),
    ("folder", "type", "user"),
];

impl Canon {
    /// The canonical form of `node` and everything under it.
    pub fn of(doc: &Doc, node: NodeId) -> Canon {
        let name = doc.name(node).to_string();
        let local = doc.local_name(node);
        let mut attrs: Vec<(String, String)> = doc
            .attr_names(node)
            .into_iter()
            .map(|a| (a.to_string(), doc.attr(node, a).unwrap_or_default()))
            .filter(|(a, v)| {
                !DEFAULTS.iter().any(|(el, at, def)| *el == local && at == a && def == v)
            })
            .collect();
        attrs.sort();
        let children: Vec<Canon> = doc.children(node).map(|c| Canon::of(doc, c)).collect();
        let text = if children.is_empty() { doc.text(node) } else { String::new() };
        Canon { name, attrs, text, children }
    }

    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(a, _)| a == name).map(|(_, v)| v.as_str())
    }

    fn local(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    /// What a child is matched on when two subtrees are compared: a property
    /// by its key, a feature by its name, anything with an id by that, and
    /// the rest by its element name alone.
    fn key(&self) -> (String, String) {
        let local = self.local();
        let k = match local {
            "property" => self.attr("key"),
            "feature" => self.attr("name"),
            _ => self.attr("id"),
        };
        (local.to_string(), k.unwrap_or("").to_string())
    }
}

/// The first difference between two canonical forms, as a short phrase a
/// diff row can carry — `name X → Y`, `documentation`, `property owner`,
/// `geometry`, `objects` — or `None` when they are equal.
pub fn describe(a: &Canon, b: &Canon) -> Option<String> {
    if a == b {
        return None;
    }
    if a.name != b.name {
        return Some(format!("{} → {}", a.name, b.name));
    }
    let local = a.local();
    // Leaves whose whole difference is best named as one thing.
    if matches!(local, "bounds" | "bendpoint" | "property" | "feature") {
        return Some(one_sided(a));
    }

    // An attribute that differs, or is on one side only.
    let mut names: Vec<&str> = a.attrs.iter().chain(&b.attrs).map(|(n, _)| n.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    for n in names {
        let (x, y) = (a.attr(n), b.attr(n));
        if x != y {
            let shown = match n {
                "xsi:type" => "type",
                other => other,
            };
            return Some(format!(
                "{shown} {} → {}",
                x.map(|s| s.trim_start_matches("archimate:")).unwrap_or(""),
                y.map(|s| s.trim_start_matches("archimate:")).unwrap_or("")
            ));
        }
    }

    if a.text != b.text {
        return Some(match local {
            "documentation" | "purpose" | "content" => local.to_string(),
            _ => "text".to_string(),
        });
    }

    // Children: matched by key, in order. The first child that is on one
    // side only, or that differs inside, names the difference.
    let mut i = 0;
    let mut j = 0;
    while i < a.children.len() || j < b.children.len() {
        match (a.children.get(i), b.children.get(j)) {
            (Some(x), Some(y)) if x.key() == y.key() => {
                if let Some(d) = describe(x, y) {
                    return Some(d);
                }
                i += 1;
                j += 1;
            }
            (Some(x), Some(y)) => {
                // Out of step: whichever side's child the other has later is
                // the one to skip over, the other is added or removed.
                let x_later = b.children[j..].iter().any(|c| c.key() == x.key());
                let y_later = a.children[i..].iter().any(|c| c.key() == y.key());
                return Some(match (x_later, y_later) {
                    (true, _) => one_sided(y),
                    (false, true) => one_sided(x),
                    (false, false) => one_sided(x),
                });
            }
            (Some(x), None) => return Some(one_sided(x)),
            (None, Some(y)) => return Some(one_sided(y)),
            (None, None) => break,
        }
    }
    Some("changed".to_string())
}

/// The phrase for a child present on one side only.
fn one_sided(c: &Canon) -> String {
    match c.local() {
        "property" => format!("property {}", c.attr("key").unwrap_or("")),
        "feature" => format!("feature {}", c.attr("name").unwrap_or("")),
        "documentation" | "purpose" => c.local().to_string(),
        "bounds" | "bendpoint" => "geometry".to_string(),
        "child" | "sourceConnection" => "objects".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canon(xml: &str) -> Canon {
        let doc = Doc::parse(xml.as_bytes().to_vec()).unwrap();
        Canon::of(&doc, doc.root())
    }

    #[test]
    fn serialisation_noise_is_not_a_difference() {
        let a = canon(concat!(
            "<element xsi:type=\"archimate:Node\" name=\"N\" id=\"n1\">\n",
            "  <documentation>a &gt; b</documentation>\n",
            "  <property key=\"k\" value=\"v\"/>\n",
            "  <child xsi:type=\"archimate:DiagramObject\" id=\"o1\">\n",
            "    <bounds x=\"10\" y=\"0\" width=\"120\" height=\"55\"/>\n",
            "  </child>\n",
            "</element>\n"
        ));
        // Attributes reordered, `y="0"` dropped, `>` spelled bare, no whitespace.
        let b = canon(concat!(
            "<element id=\"n1\" name=\"N\" xsi:type=\"archimate:Node\">",
            "<documentation>a > b</documentation>",
            "<property value=\"v\" key=\"k\"/>",
            "<child id=\"o1\" xsi:type=\"archimate:DiagramObject\">",
            "<bounds height=\"55\" width=\"120\" x=\"10\"/>",
            "</child></element>"
        ));
        assert_eq!(a, b);
        assert_eq!(describe(&a, &b), None);
    }

    #[test]
    fn every_real_change_is_a_difference_with_a_name() {
        let base = "<element xsi:type=\"archimate:Node\" name=\"N\" id=\"n1\">\
            <documentation>doc</documentation>\
            <property key=\"owner\" value=\"v\"/>\
            <child id=\"o1\"><bounds x=\"10\" y=\"20\"/></child>\
            </element>";
        let a = canon(base);
        let cases = [
            (base.replace("name=\"N\"", "name=\"M\""), "name N → M"),
            (base.replace(">doc<", ">docs<"), "documentation"),
            (base.replace("value=\"v\"", "value=\"w\""), "property owner"),
            (base.replace("x=\"10\"", "x=\"11\""), "geometry"),
            (base.replace("<child id=\"o1\">", "<child id=\"o2\">"), "objects"),
            (base.replace("archimate:Node", "archimate:Device"), "type Node → Device"),
            (base.replace("<property key=\"owner\" value=\"v\"/>", ""), "property owner"),
        ];
        for (xml, want) in cases {
            let b = canon(&xml);
            assert_ne!(a, b, "{xml}");
            assert_eq!(describe(&a, &b).as_deref(), Some(want), "{xml}");
        }
    }

    #[test]
    fn a_nested_object_added_to_a_view_is_a_difference() {
        let a = canon("<element id=\"v\"><child id=\"o1\"/></element>");
        let b = canon("<element id=\"v\"><child id=\"o1\"/><child id=\"o2\"/></element>");
        assert_ne!(a, b);
        assert_eq!(describe(&a, &b).as_deref(), Some("objects"));
        // And so is one taken away, or reordered.
        assert_eq!(describe(&b, &a).as_deref(), Some("objects"));
        let c = canon("<element id=\"v\"><child id=\"o2\"/><child id=\"o1\"/></element>");
        assert_ne!(b, c);
    }
}
