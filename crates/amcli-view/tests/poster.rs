//! A drawing someone laid out by hand in Archi — a poster — carries fonts,
//! text colours, border types and label expressions that a generated view
//! never has. Compiling it must read them, and rendering it must draw them,
//! or the poster comes out as a sheet of unreadable nine-pixel captions.

use amcli_model::Model;
use amcli_view::{Figure, Font, compile};

const POSTER: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<archimate:model xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns:archimate="http://www.archimatetool.com/archimate" name="P" id="id-m" version="5.0.0">
  <folder name="Technology &amp; Physical" id="id-f1" type="technology">
    <element xsi:type="archimate:Node" name="Cluster" id="id-n1"/>
    <element xsi:type="archimate:Node" name="Database" id="id-n2"/>
  </folder>
  <folder name="Relations" id="id-f2" type="relations">
    <element xsi:type="archimate:FlowRelationship" name="sql" id="id-r1" source="id-n1" target="id-n2"/>
  </folder>
  <folder name="Views" id="id-f3" type="diagrams">
    <element xsi:type="archimate:ArchimateDiagramModel" name="Poster" id="id-v1">
      <child xsi:type="archimate:Group" id="id-g1" name="REGION" font="1|Arial|20.0|1|COCOA|1|" fontColor="#183047" borderType="1" fillColor="#eef5fc">
        <bounds x="0" y="0" width="600" height="300"/>
        <child xsi:type="archimate:DiagramObject" id="id-o1" font="1|Arial|16.0|0|COCOA|1|" archimateElement="id-n1">
          <bounds x="20" y="60" width="200" height="80"/>
          <feature name="labelExpression" value="${name}&#xA;three nodes"/>
          <feature name="iconVisible" value="2"/>
          <sourceConnection xsi:type="archimate:Connection" id="id-c1" font="1|Arial|14.0|2|COCOA|1|" fontColor="#2563a6" source="id-o1" target="id-o2" archimateRelationship="id-r1">
            <feature name="labelExpression" value="1 · SQL / TLS"/>
          </sourceConnection>
        </child>
        <child xsi:type="archimate:DiagramObject" id="id-o2" archimateElement="id-n2" textPosition="2">
          <bounds x="360" y="60" width="200" height="80"/>
        </child>
        <child xsi:type="archimate:Note" id="id-t1" borderType="2" font="1|Arial|12.0|0|COCOA|1|">
          <bounds x="20" y="200" width="300" height="60"/>
          <content>Legend: numbers are flows</content>
        </child>
      </child>
    </element>
  </folder>
</archimate:model>
"##;

#[test]
fn a_hand_drawn_view_keeps_its_fonts_colours_borders_and_expressions() {
    let m = Model::from_bytes(POSTER.as_bytes().to_vec(), "poster.archimate").unwrap();
    let v = m.views_with_ids().next().unwrap().0;
    let scene = compile(&m, v);

    let group = scene.nodes.iter().find(|n| n.id == "id-g1").unwrap();
    assert_eq!(group.figure, Figure::Tabbed);
    assert_eq!(group.border, 1, "a rectangle group");
    assert_eq!(group.font, Some(Font { size: 20.0, bold: true, italic: false }));
    assert_eq!(group.font_color.map(|c| c.hex()), Some("#183047".to_string()));

    let cluster = scene.nodes.iter().find(|n| n.id == "id-o1").unwrap();
    assert_eq!(cluster.label, "Cluster\nthree nodes", "the label expression expands");
    assert!(!cluster.icon_visible, "iconVisible 2 hides the type icon");
    assert_eq!(cluster.font.map(|f| f.size), Some(16.0));
    assert_eq!(cluster.parent_id.as_deref(), Some("id-g1"));

    let db = scene.nodes.iter().find(|n| n.id == "id-o2").unwrap();
    assert_eq!(db.text_position, 2);

    let note = scene.nodes.iter().find(|n| n.id == "id-t1").unwrap();
    assert_eq!(note.figure, Figure::Note);
    assert_eq!(note.border, 2, "a borderless note");
    assert_eq!(note.label, "Legend: numbers are flows");

    let edge = &scene.edges[0];
    assert_eq!(
        edge.label, "1 · SQL / TLS",
        "the line's expression replaces the relationship's name"
    );
    assert_eq!(edge.font, Some(Font { size: 14.0, bold: false, italic: true }));

    let svg = amcli_render::svg(&scene, &amcli_render::Options::default());
    assert!(svg.contains(r##"font-size="20" font-weight="bold" fill="#183047">REGION"##), "{svg}");
    assert!(svg.contains(r#"font-size="16">Cluster"#) && svg.contains(">three nodes<"), "{svg}");
    assert!(
        svg.contains(r##"font-size="14" font-style="italic" fill="#2563a6">1 · SQL / TLS"##),
        "{svg}"
    );
    assert!(svg.contains(r#"stroke="none""#), "the borderless note has no stroke: {svg}");
    let block = |id: &str| -> String {
        svg.split(&format!("data-id=\"{id}\""))
            .nth(1)
            .unwrap()
            .split("</g>")
            .next()
            .unwrap()
            .to_string()
    };
    assert!(!block("id-o1").contains("<use "), "no type icon on the hidden one: {svg}");
    assert!(block("id-o2").contains("<use "), "the other node keeps its icon: {svg}");
    // A rectangle group is one rect, not a tab and a body.
    assert_eq!(block("id-g1").matches("<rect").count(), 1, "{svg}");
}

#[test]
fn archi_font_strings_parse_and_garbage_does_not() {
    assert_eq!(
        Font::parse("1|Arial|19.0|0|COCOA|1|"),
        Some(Font { size: 19.0, bold: false, italic: false })
    );
    assert_eq!(
        Font::parse("1|Segoe UI|9.0|3|WINDOWS|1|"),
        Some(Font { size: 12.0, bold: true, italic: true }),
        "a Windows point is four thirds of a pixel"
    );
    assert_eq!(Font::parse("1|Sans|9.0|0|GTK|1|").map(|f| f.size), Some(12.0));
    assert_eq!(Font::parse("1|Arial|0|0|"), None);
    assert_eq!(Font::parse("Arial"), None);
    assert_eq!(Font::parse(""), None);
}
