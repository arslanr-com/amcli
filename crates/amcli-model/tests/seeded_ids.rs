//! Ids derived from content instead of drawn at random.
//!
//! This lives in its own file because the seed is process-wide by design —
//! `new_id` is called from deep inside the edit layer, where threading a
//! parameter through every signature would be a poor trade for a flag most
//! callers never set. Cargo gives each test file its own binary, so setting it
//! here cannot reach into the tests that expect random ids.

use amcli_model::{ElementType, Model, RelType, ids};

fn open() -> Model {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/testmodel1.archimate");
    Model::open(path).unwrap()
}

fn text(m: &Model) -> String {
    String::from_utf8(m.to_bytes().unwrap()).unwrap()
}

/// The point of the whole feature: rebuilding a model from the same edits has to
/// produce the same file, or a regenerate-everything workflow shows a whole-file
/// diff and there is nothing left to review.
#[test]
fn a_seed_makes_the_same_edits_produce_the_same_file() {
    ids::set_seed(Some("test-seed".to_string()));
    assert!(ids::is_seeded());

    let build = || {
        let mut m = open();
        let a =
            m.add_element(ElementType::ApplicationComponent, "Payment API", None, None).unwrap();
        let b = m.add_element(ElementType::DataObject, "Payment Record", None, None).unwrap();
        m.add_relation(RelType::Access, a, b, None, None, None).unwrap();
        // Two elements sharing a type and a name: the second cannot reuse the
        // first's id, and which of them gets the bumped one must not vary.
        m.add_element(ElementType::ApplicationComponent, "Payment API", None, None).unwrap();
        let v = m.add_view("Payments", None).unwrap();
        let obj = m.add_view_object(v, a, 0, 0, 120, 55).unwrap();
        let other = m.add_view_object(v, b, 0, 100, 120, 55).unwrap();
        let rel = m.concepts_with_ids().find(|(_, c)| c.kind.is_relationship()).unwrap().0;
        m.add_view_connection(v, rel, &obj, &other, &[]).unwrap();
        text(&m)
    };

    let first = build();
    assert_eq!(first, build(), "the same edits produced a different file");

    // Elements, relationships, views, objects and connections all take part:
    // one random id anywhere would break the property above.
    let ids: Vec<&str> =
        first.split(r#" id=""#).skip(1).filter_map(|s| s.split('"').next()).collect();
    let unique: std::collections::HashSet<&&str> = ids.iter().collect();
    assert_eq!(ids.len(), unique.len(), "a derived id collided: {ids:?}");
    assert!(ids.len() > 12, "expected the whole model to be covered, saw {}", ids.len());
}
