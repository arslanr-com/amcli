//! The contract this crate exists to keep: parse then write is the identity
//! function, and an edit changes only the bytes it has to.

use amcli_xml::{Doc, NodeBuilder};

fn rt(src: &str) {
    let doc = Doc::parse(src.as_bytes().to_vec()).expect("parse");
    let out = doc.to_bytes();
    assert_eq!(String::from_utf8_lossy(&out), src, "round trip changed the bytes");
    assert!(doc.is_unmodified());
}

#[test]
fn identity_minimal() {
    rt("<a/>");
    rt("<a></a>");
    rt("<a>text</a>");
    rt("<?xml version=\"1.0\"?><a/>");
}

#[test]
fn identity_preserves_declaration_quoting() {
    // ElementTree rewrites this to single quotes; we must not.
    rt("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<a/>\n");
    rt("<?xml version='1.0' encoding='UTF-8'?>\n<a/>\n");
}

#[test]
fn identity_comments_pi_doctype_cdata() {
    rt("<!-- leading --><a/><!-- trailing -->");
    rt("<?xml version=\"1.0\"?>\n<!DOCTYPE a>\n<a/>");
    rt("<a><?pi target?><b/><!-- between --><c/></a>");
    rt("<a><![CDATA[ raw <not markup> & stuff ]]></a>");
}

#[test]
fn identity_whitespace_and_eol() {
    rt("<a>\n    <b/>\n    <c/>\n</a>\n");
    rt("<a>\r\n    <b/>\r\n</a>\r\n");
    rt("<a   x = \"1\"    y='2'   />");
    rt("<a\n  x=\"1\"\n  y=\"2\"\n/>");
}

#[test]
fn identity_unicode_and_entities() {
    rt("<a name=\"Ünïcode Ñáme\"/>");
    rt("<a name=\"日本語 &amp; emoji 🚀\"><b>&lt;tag&gt;</b></a>");
    rt("<a name=\"&#x41;&#66;\"/>");
    // A BOM is part of the prologue and must survive.
    rt("\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?><a/>");
}

#[test]
fn identity_archi_shaped() {
    rt(concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<archimate:model xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"",
        " xmlns:archimate=\"http://www.archimatetool.com/archimate\"",
        " name=\"Test\" id=\"be0eecc1\" version=\"5.0.0\">\n",
        "  <folder name=\"Business\" id=\"272c8c4d\" type=\"business\">\n",
        "    <element xsi:type=\"archimate:BusinessActor\" name=\"Actor\" id=\"59fa6c90\">\n",
        "      <documentation>Some docs</documentation>\n",
        "      <property key=\"owner\" value=\"team\"/>\n",
        "    </element>\n",
        "  </folder>\n",
        "</archimate:model>\n"
    ));
}

/// Every plain-XML file we ship, including the 229 KB relationship matrix.
#[test]
fn identity_over_corpus() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut checked = 0;
    for dir in ["tests/corpus", "assets/archi"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if !matches!(ext, "archimate" | "xml") {
                continue;
            }
            let bytes = std::fs::read(&path).expect("read");
            // Zipped .archimate files are a container concern, not an XML one.
            if bytes.starts_with(b"PK") {
                continue;
            }
            let doc = Doc::parse(bytes.clone()).unwrap_or_else(|e| panic!("{path:?}: {e}"));
            assert_eq!(doc.to_bytes(), bytes, "round trip changed {path:?}");
            checked += 1;
        }
    }
    assert!(checked >= 9, "expected a real corpus, only checked {checked} files");
}

// ---- involution: an edit and its inverse leave no trace -------------------

#[test]
fn setting_an_attribute_to_its_current_value_is_not_a_change() {
    let src = br#"<a x="1" y="2"/>"#.to_vec();
    let mut doc = Doc::parse(src.clone()).unwrap();
    let root = doc.root();
    doc.set_attr(root, "x", "1");
    assert!(doc.is_unmodified());
    assert_eq!(doc.to_bytes(), src);
}

#[test]
fn changing_an_attribute_and_changing_it_back_restores_the_bytes() {
    let src = br#"<a   x = 'one'   y="two"/>"#.to_vec();
    let mut doc = Doc::parse(src.clone()).unwrap();
    let root = doc.root();
    doc.set_attr(root, "x", "changed");
    assert_ne!(doc.to_bytes(), src);
    doc.set_attr(root, "x", "one");
    // Odd spacing and single quotes survive because the tag is spliced, not rebuilt.
    assert_eq!(doc.to_bytes(), src);
}

#[test]
fn inserting_then_removing_a_child_restores_the_bytes() {
    let src = b"<a>\n    <b/>\n</a>\n".to_vec();
    let mut doc = Doc::parse(src.clone()).unwrap();
    let root = doc.root();
    let new = doc.append_child(root, NodeBuilder::new("c").attr("id", "x")).unwrap();
    assert_ne!(doc.to_bytes(), src);
    doc.remove_subtree(new);
    assert_eq!(doc.to_bytes(), src);
}

// ---- minimal diffs --------------------------------------------------------

#[test]
fn an_edit_touches_only_its_own_tag() {
    let src = concat!(
        "<root>\n",
        "  <keep a=\"1\">   <!-- odd   spacing preserved -->  </keep>\n",
        "  <edit name=\"before\" other=\"untouched\"/>\n",
        "  <keep2><deep><deeper/></deep></keep2>\n",
        "</root>\n"
    );
    let mut doc = Doc::parse(src.as_bytes().to_vec()).unwrap();
    let target = doc.child_named(doc.root(), "edit").unwrap();
    doc.set_attr(target, "name", "after");
    let out = String::from_utf8(doc.to_bytes()).unwrap();

    assert_eq!(out, src.replace("before", "after"));
    let diff = out.lines().zip(src.lines()).filter(|(a, b)| a != b).count();
    assert_eq!(diff, 1, "exactly one line should differ");
}

#[test]
fn removing_a_child_takes_its_leading_whitespace() {
    let src = "<a>\n    <b/>\n    <c/>\n</a>\n";
    let mut doc = Doc::parse(src.as_bytes().to_vec()).unwrap();
    let b = doc.child_named(doc.root(), "b").unwrap();
    doc.remove_subtree(b);
    assert_eq!(String::from_utf8(doc.to_bytes()).unwrap(), "<a>\n    <c/>\n</a>\n");
}

#[test]
fn inserted_nodes_pick_up_the_surrounding_indentation() {
    let src = "<a>\n    <b/>\n</a>\n";
    let mut doc = Doc::parse(src.as_bytes().to_vec()).unwrap();
    let root = doc.root();
    doc.append_child(root, NodeBuilder::new("c").attr("id", "id-1")).unwrap();
    assert_eq!(
        String::from_utf8(doc.to_bytes()).unwrap(),
        "<a>\n    <b/>\n    <c id=\"id-1\"/>\n</a>\n"
    );
}

/// Regression: an element with no attributes in the source has no attribute
/// span to splice against, so the cursor has to start after the element name or
/// the new attribute lands in front of the tag.
#[test]
fn an_element_with_no_attributes_can_gain_one() {
    let src = "<a><concept>$Macro$</concept></a>";
    let mut doc = Doc::parse(src.as_bytes().to_vec()).unwrap();
    let c = doc.child_named(doc.root(), "concept").unwrap();
    doc.set_attr(c, "id", "id-1");
    assert_eq!(
        String::from_utf8(doc.to_bytes()).unwrap(),
        "<a><concept id=\"id-1\">$Macro$</concept></a>"
    );
    let re = Doc::parse(doc.to_bytes()).unwrap();
    let rc = re.child_named(re.root(), "concept").unwrap();
    assert_eq!(re.attr(rc, "id").as_deref(), Some("id-1"));
    assert_eq!(re.text(rc), "$Macro$");
}

#[test]
fn a_self_closing_element_that_gains_a_child_is_reopened() {
    let src = "<a>\n    <b/>\n</a>\n";
    let mut doc = Doc::parse(src.as_bytes().to_vec()).unwrap();
    let b = doc.child_named(doc.root(), "b").unwrap();
    doc.append_child(b, NodeBuilder::new("c")).unwrap();
    assert_eq!(
        String::from_utf8(doc.to_bytes()).unwrap(),
        "<a>\n    <b>\n        <c/>\n    </b>\n</a>\n"
    );
}

// ---- reading --------------------------------------------------------------

#[test]
fn attributes_and_text_are_unescaped_on_read() {
    let doc = Doc::parse(br#"<a name="A &amp; B &lt;x&gt;"><t>&lt;raw&gt; &#65;</t></a>"#.to_vec())
        .unwrap();
    let root = doc.root();
    assert_eq!(doc.attr(root, "name").as_deref(), Some("A & B <x>"));
    assert_eq!(doc.attr_raw(root, "name"), Some("A &amp; B &lt;x&gt;"));
    let t = doc.child_named(root, "t").unwrap();
    assert_eq!(doc.text(t), "<raw> A");
}

#[test]
fn namespace_prefixes_are_read_but_never_rewritten() {
    let src = br#"<archimate:model xmlns:archimate="u" xsi:type="archimate:X"/>"#.to_vec();
    let doc = Doc::parse(src.clone()).unwrap();
    assert_eq!(doc.name(doc.root()), "archimate:model");
    assert_eq!(doc.local_name(doc.root()), "model");
    assert_eq!(doc.attr(doc.root(), "xsi:type").as_deref(), Some("archimate:X"));
    assert_eq!(doc.to_bytes(), src);
}

#[test]
fn text_can_be_replaced_and_is_escaped() {
    let src = "<a><doc>old</doc></a>";
    let mut doc = Doc::parse(src.as_bytes().to_vec()).unwrap();
    let d = doc.child_named(doc.root(), "doc").unwrap();
    doc.set_text(d, "new & <improved>").unwrap();
    assert_eq!(
        String::from_utf8(doc.to_bytes()).unwrap(),
        "<a><doc>new &amp; &lt;improved&gt;</doc></a>"
    );
}

#[test]
fn malformed_input_is_an_error_not_a_panic() {
    assert!(Doc::parse(b"<a><b></a>".to_vec()).is_err());
    assert!(Doc::parse(b"not xml at all".to_vec()).is_err());
    assert!(Doc::parse(b"<a>".to_vec()).is_err());
    assert!(Doc::parse(vec![0xff, 0xfe, 0x00]).is_err());
}

/// An element that loses its last child closes itself again.
///
/// Adding a documentation and then clearing it is two runs of amcli: the first
/// expands `<element …/>` to hold the child, the second parses a file where
/// the `/>` is already gone. Without this the undone edit left
/// `<element …></element>` behind — a scar on a file nothing had changed.
#[test]
fn an_emptied_element_closes_itself_again() {
    let src = "<a><b/></a>";
    let mut doc = Doc::parse(src.as_bytes().to_vec()).unwrap();
    let b = doc.child_named(doc.root(), "b").unwrap();
    doc.remove_subtree(b);
    assert_eq!(String::from_utf8(doc.to_bytes()).unwrap(), "<a/>");

    // Indentation the children stood in is not text, and the document must
    // report what it is about to write: proptest caught these two disagreeing.
    let src = "<a>\n  <b/>\n</a>";
    let mut doc = Doc::parse(src.as_bytes().to_vec()).unwrap();
    let b = doc.child_named(doc.root(), "b").unwrap();
    doc.remove_subtree(b);
    assert_eq!(doc.text(doc.root()), "");
    assert_eq!(String::from_utf8(doc.to_bytes()).unwrap(), "<a/>");

    // An element that never had children keeps its text, whitespace and all.
    let src = "<a>   </a>";
    let mut doc = Doc::parse(src.as_bytes().to_vec()).unwrap();
    doc.set_attr(doc.root(), "k", "v");
    assert_eq!(doc.text(doc.root()), "   ");
    assert_eq!(String::from_utf8(doc.to_bytes()).unwrap(), r#"<a k="v">   </a>"#);
}
