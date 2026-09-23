//! Where a label sits. A group's name belongs at the top of the group, under
//! its tab, unless its text position says otherwise: centred in a tall group it
//! sat behind the boxes the group holds, and a poster whose lanes are groups
//! showed no lane names at all. A line's label goes where the connection's
//! text position puts it — by the source, halfway, or by the target.

use amcli_model::Model;
use amcli_view::compile;

const VIEW: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<archimate:model xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:archimate="http://www.archimatetool.com/archimate" name="T" id="id-m" version="5.0.0">
  <folder name="Application" id="id-f1" type="application">
    <element xsi:type="archimate:ApplicationComponent" name="A" id="id-a"/>
    <element xsi:type="archimate:ApplicationComponent" name="B" id="id-b"/>
  </folder>
  <folder name="Relations" id="id-f2" type="relations">
    <element xsi:type="archimate:FlowRelationship" name="near the source" id="id-r1" source="id-a" target="id-b"/>
    <element xsi:type="archimate:FlowRelationship" name="near the target" id="id-r2" source="id-a" target="id-b"/>
    <element xsi:type="archimate:FlowRelationship" name="halfway" id="id-r3" source="id-a" target="id-b"/>
  </folder>
  <folder name="Views" id="id-f3" type="diagrams">
    <element xsi:type="archimate:ArchimateDiagramModel" name="Lanes" id="id-v1">
      <child xsi:type="archimate:Group" id="id-g1" name="TOP LANE">
        <bounds x="0" y="0" width="300" height="800"/>
      </child>
      <child xsi:type="archimate:Group" id="id-g2" name="MIDDLE LANE" textPosition="1">
        <bounds x="400" y="0" width="300" height="800"/>
      </child>
      <child xsi:type="archimate:Group" id="id-g3" name="BOTTOM LANE" textPosition="2">
        <bounds x="800" y="0" width="300" height="800"/>
      </child>
      <child xsi:type="archimate:DiagramObject" id="id-oa" archimateElement="id-a">
        <bounds x="0" y="900" width="100" height="50"/>
        <sourceConnection xsi:type="archimate:Connection" id="id-c1" source="id-oa" target="id-ob" archimateRelationship="id-r1" textPosition="0"/>
        <sourceConnection xsi:type="archimate:Connection" id="id-c2" source="id-oa" target="id-ob" archimateRelationship="id-r2" textPosition="2"/>
        <sourceConnection xsi:type="archimate:Connection" id="id-c3" source="id-oa" target="id-ob" archimateRelationship="id-r3"/>
      </child>
      <child xsi:type="archimate:DiagramObject" id="id-ob" archimateElement="id-b">
        <bounds x="1000" y="900" width="100" height="50"/>
      </child>
    </element>
  </folder>
</archimate:model>
"##;

/// The x and y of the `<text>` whose content is `label`.
fn at(svg: &str, label: &str) -> (f64, f64) {
    let end = svg.find(&format!(">{label}</text>")).unwrap_or_else(|| panic!("no {label}: {svg}"));
    let open = svg[..end].rfind("<text ").unwrap();
    let tag = &svg[open..end];
    let num = |attr: &str| -> f64 {
        let v = tag.split(&format!("{attr}=\"")).nth(1).unwrap();
        v[..v.find('"').unwrap()].parse().unwrap()
    };
    (num("x"), num("y"))
}

#[test]
fn a_group_is_named_at_the_top_unless_its_text_position_says_otherwise() {
    let m = Model::from_bytes(VIEW.as_bytes().to_vec(), "t.archimate").unwrap();
    let scene = compile(&m, m.views_with_ids().next().unwrap().0);
    let pos = |id: &str| scene.nodes.iter().find(|n| n.id == id).unwrap().text_position;
    assert_eq!(pos("id-g1"), 0, "Archi leaves a group's default, the top, unwritten");
    assert_eq!((pos("id-g2"), pos("id-g3")), (1, 2));
    assert_eq!(pos("id-oa"), 1, "an element keeps the middle");

    let svg = amcli_render::svg(&scene, &amcli_render::Options::default());
    let (_, top) = at(&svg, "TOP LANE");
    let (_, middle) = at(&svg, "MIDDLE LANE");
    let (_, bottom) = at(&svg, "BOTTOM LANE");
    assert!(top < 60.0, "under the tab, not halfway down: {top}");
    assert!((350.0..450.0).contains(&middle), "centred in the body: {middle}");
    assert!(bottom > 750.0, "along the bottom: {bottom}");
}

#[test]
fn a_line_label_sits_where_its_text_position_puts_it() {
    let m = Model::from_bytes(VIEW.as_bytes().to_vec(), "t.archimate").unwrap();
    let scene = compile(&m, m.views_with_ids().next().unwrap().0);
    let pos = |id: &str| scene.edges.iter().find(|e| e.id == id).unwrap().text_position;
    assert_eq!((pos("id-c1"), pos("id-c2"), pos("id-c3")), (0, 2, 1));

    let svg = amcli_render::svg(&scene, &amcli_render::Options::default());
    let (source, _) = at(&svg, "near the source");
    let (target, _) = at(&svg, "near the target");
    let (half, _) = at(&svg, "halfway");
    assert!(source < half && half < target, "{source} {half} {target}");
    assert!((half - 550.0).abs() < 1.0, "halfway is the midpoint, as before: {half}");
}
