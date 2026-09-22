use amcli_model::{ConceptKind, EditError, ElementType, FolderType, Model, RelType};

fn corpus(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus").join(name)
}

fn open(name: &str) -> Model {
    Model::open(corpus(name)).unwrap()
}

fn text(m: &Model) -> String {
    String::from_utf8(m.to_bytes().unwrap()).unwrap()
}

#[test]
fn a_new_element_lands_in_the_folder_archi_would_have_chosen() {
    let mut m = open("testmodel1.archimate");
    let before = text(&m);

    let c = m.add_element(ElementType::ApplicationComponent, "Refund Service", None, None).unwrap();
    assert_eq!(m.concept(c).name, "Refund Service");
    assert_eq!(m.folder(m.concept(c).folder).folder_type, FolderType::Application);

    // Attribute order matches Archi's own output.
    let after = text(&m);
    let line =
        after.lines().find(|l| l.contains("Refund Service")).expect("the element was written");
    assert!(
        line.trim().starts_with(
            r#"<element xsi:type="archimate:ApplicationComponent" name="Refund Service" id="id-"#
        ),
        "{line}"
    );

    // The Application folder was self-closing, so it has to reopen: three lines
    // where there was one. Everything else in the file is untouched.
    let changed: Vec<&str> = after.lines().filter(|l| !before.lines().any(|b| b == *l)).collect();
    assert_eq!(changed.len(), 2, "the reopened folder tag and the new element");
    assert_eq!(after.lines().count(), before.lines().count() + 2);

    // Into a folder that already has children, a new element really is one line.
    let mut m2 = open("testmodel1.archimate");
    let before2 = text(&m2);
    let business = m2.folder_by_path("/Business").unwrap();
    m2.add_element(ElementType::BusinessActor, "Extra", Some(business), None).unwrap();
    let after2 = text(&m2);
    assert_eq!(after2.lines().count(), before2.lines().count() + 1);
    assert_eq!(
        after2.lines().filter(|l| !before2.lines().any(|b| b == *l)).count(),
        1,
        "one added line, nothing else disturbed"
    );
}

#[test]
fn adding_then_deleting_restores_the_original_bytes() {
    let mut m = open("testmodel1.archimate");
    let before = m.to_bytes().unwrap();
    let c = m.add_element(ElementType::ApplicationComponent, "Temp", None, None).unwrap();
    assert_ne!(m.to_bytes().unwrap(), before);
    m.delete_concept(c).unwrap();
    assert_eq!(m.to_bytes().unwrap(), before, "an edit and its inverse leave no trace");
}

#[test]
fn an_illegal_relationship_is_refused_and_the_message_says_what_is_legal() {
    let mut m = open("testmodel1.archimate");
    let data = m.add_element(ElementType::DataObject, "Record", None, None).unwrap();
    let comp = m.add_element(ElementType::ApplicationComponent, "Svc", None, None).unwrap();

    // ArchiMate permits only Association from a DataObject to a Component.
    let err = m.add_relation(RelType::Serving, data, comp, None, None, None).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("does not permit Serving"), "{msg}");
    assert!(msg.contains("permitted here: Association"), "the error has to teach: {msg}");
    assert!(matches!(err, EditError::InvalidRelationship { .. }));

    // And the legal one goes through.
    assert!(m.add_relation(RelType::Association, data, comp, None, None, None).is_ok());
}

#[test]
fn a_duplicate_relationship_is_refused() {
    let mut m = open("testmodel1.archimate");
    let f = m.add_element(ElementType::ApplicationFunction, "F", None, None).unwrap();
    let d = m.add_element(ElementType::DataObject, "D", None, None).unwrap();
    m.add_relation(RelType::Access, f, d, Some(3), None, None).unwrap();

    let err = m.add_relation(RelType::Access, f, d, Some(1), None, None).unwrap_err();
    assert!(matches!(err, EditError::DuplicateRelationship { .. }), "{err}");
    // A different type between the same pair is a different relationship.
    assert!(m.add_relation(RelType::Association, f, d, None, None, None).is_ok());
}

#[test]
fn every_relationship_at_a_junction_must_share_its_type() {
    let mut m = open("testmodel1.archimate");
    let j = m.add_element(ElementType::Junction, "J", None, None).unwrap();
    let a = m.add_element(ElementType::ApplicationProcess, "A", None, None).unwrap();
    let b = m.add_element(ElementType::ApplicationProcess, "B", None, None).unwrap();

    m.add_relation(RelType::Triggering, a, j, None, None, None).unwrap();
    let err = m.add_relation(RelType::Flow, j, b, None, None, None).unwrap_err();
    assert!(matches!(err, EditError::MixedJunction(_, "Triggering")), "{err}");
    assert!(m.add_relation(RelType::Triggering, j, b, None, None, None).is_ok());
}

#[test]
fn access_type_zero_is_left_out_because_archi_leaves_it_out() {
    let mut m = open("testmodel1.archimate");
    let f = m.add_element(ElementType::ApplicationFunction, "F", None, None).unwrap();
    let d = m.add_element(ElementType::DataObject, "D", None, None).unwrap();

    // 0 is the schema default (write), and EMF omits defaults. Writing it
    // explicitly would differ from a file Archi produced.
    m.add_relation(RelType::Access, f, d, Some(0), None, None).unwrap();
    assert!(!text(&m).contains("accessType"));

    let d2 = m.add_element(ElementType::DataObject, "D2", None, None).unwrap();
    m.add_relation(RelType::Access, f, d2, Some(3), None, None).unwrap();
    assert!(text(&m).contains(r#"accessType="3""#));

    // A fresh target, so this is rejected for the access type rather than for
    // duplicating the relationship above.
    let d3 = m.add_element(ElementType::DataObject, "D3", None, None).unwrap();
    assert!(matches!(
        m.add_relation(RelType::Access, f, d3, Some(9), None, None),
        Err(EditError::BadAccessType(9))
    ));
}

/// The headline fix. Deleting a concept that appears on views used to leave
/// `archimateElement` and `archimateRelationship` pointing at nothing, and Archi
/// then refuses to open the model.
#[test]
fn deleting_a_concept_cleans_up_every_view_that_showed_it() {
    let mut m = open("testmodel1.archimate");
    let actor = m.concept_by_id("59fa6c90").expect("Business Actor");

    let plan = m.delete_plan(actor);
    assert_eq!(plan.concepts, ["59fa6c90"]);
    assert_eq!(plan.relationships, ["ffdc8ea9"], "the assignment it was part of");
    assert_eq!(plan.diagram_objects.len(), 2, "it appears on two views");
    assert_eq!(plan.connections.len(), 2);
    assert_eq!(plan.views.len(), 2);
    assert_eq!(plan.total(), 6);

    // Planning changes nothing.
    assert!(m.is_unmodified());

    let done = m.delete_concept(actor).unwrap();
    assert_eq!(done.total(), plan.total());

    let out = text(&m);
    for gone in ["59fa6c90", "ffdc8ea9", "eac5adf1", "6e21f397", "f408e9d0", "6cb40cfb"] {
        assert!(!out.contains(gone), "{gone} survived the delete");
    }
    // The mirror attribute is recomputed, not patched: the objects those
    // connections pointed at no longer claim an incoming connection.
    assert!(!out.contains("targetConnections"), "stale mirrors left behind:\n{out}");

    // And the result is still a model that loads.
    let reopened = Model::from_bytes(m.to_bytes().unwrap(), "x.archimate").unwrap();
    assert!(reopened.concept_by_id("59fa6c90").is_none());
    assert_eq!(reopened.views().count(), 4, "the views themselves survive");
}

#[test]
fn deleting_cascades_through_relationships_that_point_at_relationships() {
    let mut m = open("testmodel1.archimate");
    let a = m.add_element(ElementType::ApplicationComponent, "A", None, None).unwrap();
    let b = m.add_element(ElementType::ApplicationComponent, "B", None, None).unwrap();
    let note = m.add_element(ElementType::ApplicationComponent, "N", None, None).unwrap();
    let r1 = m.add_relation(RelType::Serving, a, b, None, None, None).unwrap();
    // ArchiMate lets an association target a relationship.
    m.add_relation(RelType::Association, note, r1, None, None, None).unwrap();

    let plan = m.delete_plan(a);
    assert_eq!(plan.relationships.len(), 2, "the serving, and the association to it");
}

#[test]
fn a_junction_left_with_one_connection_is_flagged_not_removed() {
    let mut m = open("testmodel1.archimate");
    let j = m.add_element(ElementType::Junction, "J", None, None).unwrap();
    let a = m.add_element(ElementType::ApplicationProcess, "A", None, None).unwrap();
    let b = m.add_element(ElementType::ApplicationProcess, "B", None, None).unwrap();
    m.add_relation(RelType::Triggering, a, j, None, None, None).unwrap();
    m.add_relation(RelType::Triggering, j, b, None, None, None).unwrap();

    let plan = m.delete_plan(a);
    assert_eq!(plan.degenerate_junctions, ["J"]);
    // Flagged only: a junction is a modelling decision, not debris to sweep up.
    m.delete_concept(a).unwrap();
    assert!(m.concept_by_id(&m.concept(j).id.clone()).is_some());
}

/// Deleting a view takes the drawing and nothing else — but a box on *another*
/// view standing for this one has to go too, or `model="…"` points at nothing
/// and Archi refuses to open the file.
#[test]
fn deleting_a_view_takes_its_drawing_and_any_reference_to_it() {
    let mut m = open("testDeleteHandler.archimate");
    let concepts_before = m.concepts().count();

    let v = m.view_by_id("12917bec").expect("the fixture's second view");
    let refs = m.view_references(v);
    assert_eq!(refs.len(), 1, "one view draws a reference to this one");
    assert_eq!(refs[0].1, "99a52921");

    let plan = m.delete_view(v).unwrap();
    assert!(plan.diagram_objects.len() > 1, "the objects on it are reported: {plan:?}");

    let after = text(&m);
    assert!(!after.contains("12917bec"), "the view survived");
    assert!(!after.contains("99a52921"), "the reference box survived and now dangles");
    assert!(m.view_by_id("12917bec").is_none());

    // A view is a drawing of the model, not part of it.
    assert_eq!(m.concepts().count(), concepts_before, "no concept was touched");

    // And the file still parses as a model.
    Model::from_bytes(after.into_bytes(), "x.archimate").unwrap();
}

#[test]
fn renaming_a_view_changes_one_attribute_and_nothing_else() {
    let mut m = open("testmodel1.archimate");
    let before = text(&m);
    let v = m.view_by_id("17cdf396").unwrap();

    m.rename_view(v, "Renamed View");
    assert_eq!(m.view(v).name, "Renamed View");
    assert_eq!(text(&m), before.replace(r#"name="0 Blank View""#, r#"name="Renamed View""#));
}

#[test]
fn a_view_moves_between_folders_without_changing_its_id() {
    let mut m = open("testmodel1.archimate");
    let v = m.view_by_id("17cdf396").unwrap();
    let id = m.view(v).id.clone();
    let views = m.top_folder(FolderType::Diagrams).unwrap();
    let sub = m.add_folder(views, "Motivation").unwrap();

    m.move_view_to_folder(v, sub).unwrap();
    assert_eq!(m.folder(m.view(v).folder).path, "/Views/Motivation");
    // The id is what every diagram reference and every git diff hangs on, so
    // re-filing must not reissue it.
    assert_eq!(m.view(v).id, id);

    // Moving it where it already is is a no-op rather than a duplicate.
    let before = text(&m);
    m.move_view_to_folder(v, sub).unwrap();
    assert_eq!(text(&m), before);

    // The drawing travelled with the view, rather than being left behind.
    assert!(text(&m).contains(&format!(r#"id="{id}""#)));
    assert_eq!(m.views().filter(|w| w.id == id).count(), 1);
}

#[test]
fn a_view_cannot_be_filed_outside_the_views_tree() {
    let mut m = open("testmodel1.archimate");
    let v = m.view_by_id("17cdf396").unwrap();
    let before = text(&m);
    let business = m.top_folder(FolderType::Business).unwrap();

    // Archi shows nothing filed here, so the refusal is the whole point: a
    // model that loads with a view missing is worse than an error.
    let err = m.move_view_to_folder(v, business).unwrap_err();
    assert!(matches!(err, EditError::NotAViewsFolder(_)), "{err}");
    assert_eq!(text(&m), before, "a refused move changes nothing");
}

#[test]
fn documentation_and_properties_round_trip_through_edits() {
    let mut m = open("testmodel1.archimate");
    let c =
        m.add_element(ElementType::ApplicationComponent, "Svc", None, Some("First draft")).unwrap();
    assert_eq!(m.documentation(m.concept(c).node).as_deref(), Some("First draft"));

    m.set_documentation(c, "Second draft & more").unwrap();
    assert_eq!(m.documentation(m.concept(c).node).as_deref(), Some("Second draft & more"));
    assert!(text(&m).contains("Second draft &amp; more"));

    m.set_property(c, "owner", "team-a").unwrap();
    m.set_property(c, "tier", "1").unwrap();
    m.set_property(c, "owner", "team-b").unwrap();
    assert_eq!(
        m.properties(m.concept(c).node),
        vec![("owner".into(), "team-b".into()), ("tier".into(), "1".into())]
    );

    m.remove_property(c, "tier");
    assert_eq!(m.properties(m.concept(c).node).len(), 1);

    // Documentation comes before properties, as Archi writes it.
    let out = text(&m);
    let doc_at = out.find("<documentation>Second").unwrap();
    let prop_at = out.find(r#"<property key="owner""#).unwrap();
    assert!(doc_at < prop_at);

    m.set_documentation(c, "").unwrap();
    assert_eq!(m.documentation(m.concept(c).node), None);
}

#[test]
fn moving_a_concept_keeps_attributes_this_build_does_not_understand() {
    // compatibility_test3 carries Bogus types; re-filing one must not quietly
    // rebuild it from the fields we happen to know about.
    let mut m = open("compatibility_test3.archimate");
    let c = m.concepts_with_ids().find(|(_, c)| c.name == "E1").map(|(i, _)| i).unwrap();
    let target = m.folder_by_path("/Business").unwrap();

    m.move_to_folder(c, target).unwrap();

    let out = text(&m);
    assert!(out.contains(r#"<element xsi:type="archimate:Bogus1" name="E1""#), "{out}");
    let reopened = Model::from_bytes(m.to_bytes().unwrap(), "x.archimate").unwrap();
    let moved = reopened.concepts().find(|c| c.name == "E1").unwrap();
    assert_eq!(reopened.folder_path_of(moved), "/Business");
    assert!(matches!(&moved.kind, ConceptKind::Unknown { xsi, .. } if xsi == "Bogus1"));
}

#[test]
fn folders_can_be_created_and_are_written_before_elements() {
    let mut m = open("testmodel1.archimate");
    let app = m.folder_by_path("/Application").unwrap();
    let sub = m.add_folder(app, "Payments").unwrap();
    assert_eq!(m.folder(sub).path, "/Application/Payments");
    // A nested folder inherits its ancestor's type but writes no `type`
    // attribute, because `user` is the schema default.
    assert_eq!(m.folder(sub).folder_type, FolderType::Application);
    let out = text(&m);
    let line =
        out.lines().find(|l| l.contains(r#"name="Payments""#)).expect("the folder was written");
    assert!(line.trim().starts_with(r#"<folder name="Payments" id="id-"#), "{line}");
    assert!(!line.contains("type="), "a nested folder writes no type attribute: {line}");

    let c = m.add_element(ElementType::ApplicationComponent, "Pay", Some(sub), None).unwrap();
    assert_eq!(m.folder_path_of(m.concept(c)), "/Application/Payments");
}

#[test]
fn every_edit_leaves_a_model_that_still_loads() {
    let mut m = open("modelimporter_test.archimate");
    let before_views = m.views().count();

    let c =
        m.add_element(ElementType::ApplicationService, "New Service", None, Some("Docs")).unwrap();
    m.set_property(c, "k", "v").unwrap();
    let other = m.concepts_with_ids().find(|(_, x)| x.name == "BA1").map(|(i, _)| i).unwrap();
    m.add_relation(RelType::Serving, c, other, None, None, None).unwrap();
    m.rename(c, "Renamed Service");

    let reopened = Model::from_bytes(m.to_bytes().unwrap(), "x.archimate").unwrap();
    assert!(reopened.concepts().any(|x| x.name == "Renamed Service"));
    assert_eq!(reopened.views().count(), before_views);
    assert!(reopened.duplicate_ids().is_empty());
}

const BA1: &str = "5dde26f7-9d5e-4685-aada-5d66ad27bdb0";
const BR1: &str = "bfb40b14-442f-4ba2-a7e3-c2339093692c";
const BR2: &str = "97b99ae7-8742-4c92-a984-b2e6ea82fb07";
const REL1: &str = "e3ae7a88-0a0e-4431-82af-c89405bd3196";

/// A type chosen wrong is one attribute changed, not an element deleted and
/// remade: the id, the documentation, the relationships and every box stay,
/// and the element is re-filed only when its folder is of another tree.
#[test]
fn retyping_changes_the_type_in_place_and_files_it_where_archi_would() {
    let mut m = open("modelimporter_test.archimate");
    let before = text(&m);

    // BR1 is assigned from BA1, sits in a user subfolder and is on two views.
    let br1 = m.concept_by_id(BR1).unwrap();
    assert!(m.retype_check(br1, ElementType::BusinessInterface).unwrap().is_empty());
    assert!(m.is_unmodified(), "checking changes nothing");
    let moved = m.retype(br1, ElementType::BusinessInterface).unwrap();
    assert!(!moved, "a user subfolder under the right top folder is kept");
    let c = m.concept(br1);
    assert_eq!(c.kind, ConceptKind::Element(ElementType::BusinessInterface));
    assert_eq!(c.id, BR1);
    assert_eq!(m.folder_path_of(c), "/Business/Folder1");
    assert_eq!(
        text(&m),
        before.replace(
            r#"xsi:type="archimate:BusinessRole" name="BR1""#,
            r#"xsi:type="archimate:BusinessInterface" name="BR1""#
        ),
        "one attribute, nothing else"
    );

    // Into another layer the element moves to that layer's top folder, as one
    // block: its documentation and properties travel with it.
    let br2 = m.concept_by_id(BR2).unwrap();
    assert!(m.retype(br2, ElementType::ApplicationComponent).unwrap());
    assert_eq!(m.folder_path_of(m.concept(br2)), "/Application");
    let after = text(&m);
    assert!(
        after.contains(
            r#"<element xsi:type="archimate:ApplicationComponent" name="BR2" id="97b99ae7-8742-4c92-a984-b2e6ea82fb07">"#
        ),
        "{after}"
    );
    assert!(after.contains("<documentation>BR2 Documentation</documentation>"), "{after}");
    assert_eq!(after.matches(BR2).count(), before.matches(BR2).count(), "every reference stays");

    let reopened = Model::from_bytes(m.to_bytes().unwrap(), "x.archimate").unwrap();
    let r = reopened.concept_by_id(REL1).map(|r| reopened.concept(r)).unwrap();
    assert_eq!((r.source.as_deref(), r.target.as_deref()), (Some(BA1), Some(BR1)));
}

#[test]
fn retyping_is_refused_when_a_relationship_would_break_the_matrix() {
    let mut m = open("modelimporter_test.archimate");
    let before = m.to_bytes().unwrap();
    let ba1 = m.concept_by_id(BA1).unwrap();

    // The check names the relationship, its other end and what would be legal.
    let illegal = m.retype_check(ba1, ElementType::ApplicationComponent).unwrap();
    assert_eq!(illegal.len(), 1);
    let r = &illegal[0];
    assert_eq!((r.id.as_str(), r.rel, r.direction), (REL1, "Assignment", "out"));
    assert_eq!(
        (r.other.as_str(), r.other_name.as_str(), r.other_type.as_str()),
        (BR1, "BR1", "BusinessRole")
    );
    assert!(r.permitted.contains(&"Serving") && !r.permitted.contains(&"Assignment"), "{r:?}");

    let err = m.retype(ba1, ElementType::ApplicationComponent).unwrap_err();
    assert!(matches!(&err, EditError::IllegalRelationships(l) if l.len() == 1), "{err}");
    assert!(err.to_string().contains("BR1") && err.to_string().contains("permitted here"), "{err}");
    assert_eq!(m.to_bytes().unwrap(), before, "a refusal writes nothing");

    // A relationship, a junction and a junction-to-be are refused outright.
    let rel = m.concept_by_id(REL1).unwrap();
    assert!(matches!(m.retype(rel, ElementType::BusinessActor), Err(EditError::NotAnElement(..))));
    assert!(matches!(m.retype(ba1, ElementType::Junction), Err(EditError::JunctionFixed(..))));
    let j = m.add_element(ElementType::Junction, "", None, None).unwrap();
    assert!(matches!(m.retype(j, ElementType::BusinessActor), Err(EditError::JunctionFixed(..))));
}

/// Two names for one thing become one: relationships and boxes move to the
/// survivor, twins and would-be loops go, the words and properties are
/// carried over, and what is left loads.
#[test]
fn merging_folds_one_element_into_another_and_leaves_a_model_that_loads() {
    let mut m = open("modelimporter_test.archimate");
    let (ba1, br1, br2) = (
        m.concept_by_id(BA1).unwrap(),
        m.concept_by_id(BR1).unwrap(),
        m.concept_by_id(BR2).unwrap(),
    );
    let svc = m.add_element(ElementType::ApplicationComponent, "Svc", None, None).unwrap();
    let svc_id = m.concept(svc).id.clone();
    // To be repointed: BR2 → Svc. A twin: BR2 → BA1, which BR1 → BA1 already
    // says. A loop-to-be: BR2 – BR1. The last two are drawn on View 1, where
    // all three elements are.
    let kept = m.add_relation(RelType::Association, br2, svc, None, None, None).unwrap();
    let twin = m.add_relation(RelType::Serving, br1, ba1, None, None, None).unwrap();
    let dup = m.add_relation(RelType::Serving, br2, ba1, None, None, None).unwrap();
    let cycle = m.add_relation(RelType::Association, br2, br1, None, None, None).unwrap();
    for r in [dup, cycle] {
        assert_eq!(m.draw_relation_on_views(r).unwrap().len(), 1);
    }
    let (kept_id, twin_id, dup_id, cycle_id) = (
        m.concept(kept).id.clone(),
        m.concept(twin).id.clone(),
        m.concept(dup).id.clone(),
        m.concept(cycle).id.clone(),
    );
    m.set_property(br2, "src", "notes-2026").unwrap();
    m.set_property(br1, "src", "memo-2025").unwrap();
    m.set_property(br2, "extra", "only-on-br2").unwrap();

    assert!(m.merge_check(br2, br1).unwrap().is_empty());
    let done = m.merge_elements(br2, br1, true).unwrap();
    assert_eq!(done.relationships, [kept_id]);
    assert_eq!(done.dropped, [dup_id.clone(), cycle_id.clone()]);
    assert_eq!(done.objects, ["191ff954-5491-41cd-abc2-a410af3527e0"]);
    assert_eq!(done.duplicated_on, ["View 1"]);

    let out = text(&m);
    assert!(!out.contains(BR2), "no element, endpoint or box refers to it:\n{out}");
    assert!(!out.contains(&dup_id) && !out.contains(&cycle_id), "{out}");
    assert!(
        out.contains("<documentation>BR1 Documentation\n\nBR2 Documentation</documentation>"),
        "{out}"
    );
    let props = m.properties(m.concept(br1).node);
    for (k, v) in [
        ("p1", "v1"),
        ("src", "memo-2025"),
        ("extra", "only-on-br2"),
        ("supporting-source", "notes-2026"),
    ] {
        assert!(props.iter().any(|(pk, pv)| pk == k && pv == v), "{k}={v} in {props:?}");
    }
    let r = m.concept(kept);
    assert_eq!((r.source.as_deref(), r.target.as_deref()), (Some(BR1), Some(svc_id.as_str())));
    // The twin's line draws the twin now, and the loop's line is gone with
    // its mirror on the box it pointed at.
    assert!(out.contains(&format!(r#"archimateRelationship="{twin_id}""#)), "{out}");
    let reopened = Model::from_bytes(m.to_bytes().unwrap(), "x.archimate").unwrap();
    assert!(reopened.concept_by_id(BR2).is_none());
    for c in reopened.concepts().filter(|c| c.kind.is_relationship()) {
        for end in [&c.source, &c.target] {
            assert!(reopened.concept_by_id(end.as_deref().unwrap()).is_some(), "{c:?} dangles");
        }
    }
    for v in reopened.views() {
        for n in reopened.doc.descendants(v.node) {
            for attr in ["archimateElement", "archimateRelationship"] {
                if let Some(id) = reopened.doc.attr(n, attr) {
                    assert!(
                        reopened.concept_by_id(&id).is_some(),
                        "{attr}={id} dangles on {}",
                        v.name
                    );
                }
            }
        }
    }
}

#[test]
fn merging_is_refused_when_a_repointed_relationship_would_break_the_matrix() {
    let mut m = open("modelimporter_test.archimate");
    let (ba1, br1, br2) = (
        m.concept_by_id(BA1).unwrap(),
        m.concept_by_id(BR1).unwrap(),
        m.concept_by_id(BR2).unwrap(),
    );
    let data = m.add_element(ElementType::DataObject, "Record", None, None).unwrap();
    // BR2 serves BA1; a DataObject may not.
    let serving = m.add_relation(RelType::Serving, br2, ba1, None, None, None).unwrap();
    let before = m.to_bytes().unwrap();

    let illegal = m.merge_check(br2, data).unwrap();
    assert_eq!(illegal.len(), 1);
    assert_eq!(illegal[0].id, m.concept(serving).id);
    assert_eq!(
        (illegal[0].rel, illegal[0].direction, illegal[0].other_name.as_str()),
        ("Serving", "out", "BA1")
    );
    let err = m.merge_elements(br2, data, true).unwrap_err();
    assert!(matches!(err, EditError::IllegalRelationships(_)), "{err}");
    assert_eq!(m.to_bytes().unwrap(), before, "a refusal writes nothing");

    // The two must be two elements.
    assert!(matches!(m.merge_elements(br1, br1, true), Err(EditError::SameElement(_))));
    let rel = m.concept_by_id(REL1).unwrap();
    assert!(matches!(m.merge_elements(rel, br1, true), Err(EditError::NotAnElement(..))));
    assert!(matches!(m.merge_elements(br1, rel, true), Err(EditError::NotAnElement(..))));

    // And without the documentation, the survivor's words are untouched.
    let doc_before = m.documentation(m.concept(br1).node);
    m.delete_concept(serving).unwrap();
    m.merge_elements(br2, br1, false).unwrap();
    assert_eq!(m.documentation(m.concept(br1).node), doc_before);
}
