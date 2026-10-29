//! MTEXT columns and paragraph codes survive a DXF round trip (#532): the column groups
//! 75/76/78/79/48/49/50, tabs as `^I`, carets as `^ `.

use cadcraft_doc::{Common, Drawing, EntityKind, MText, MTextColumnType, MTextColumns, Space};
use cadcraft_geom::Vec3;
use cadcraft_io::{SaveOptions, Version, entity_from_groups, read_dxf, write_dxf, write_with};

fn mtext(contents: &str, rotation: f64, columns: Option<MTextColumns>) -> MText {
    MText {
        insert: Vec3::new(1.0, 2.0, 0.0),
        height: 0.5,
        width: 6.0,
        attach: 1,
        rotation,
        style: "Standard".into(),
        contents: contents.into(),
        line_spacing: 1.0,
        line_spacing_exact: false,
        columns,
    }
}

fn round_trip(t: MText) -> (MText, String) {
    let mut d = Drawing::new_metric();
    d.add(&Space::Model, Common::default(), EntityKind::MText(t)).unwrap();
    let dxf = write_dxf(&d);
    let back = read_dxf(dxf.as_bytes()).unwrap();
    match back.model.iter().find_map(|e| match &e.kind {
        EntityKind::MText(t) => Some(t.clone()),
        _ => None,
    }) {
        Some(t) => (t, dxf),
        None => panic!("no mtext"),
    }
}

#[test]
fn static_columns_round_trip() {
    let c = MTextColumns { kind: MTextColumnType::Static, count: 3, gutter: 1.25, height: 7.5, ..Default::default() };
    let (t, dxf) = round_trip(mtext("one\\Ntwo", 0.0, Some(c.clone())));
    assert_eq!(t.columns, Some(c));
    assert_eq!(t.width, 6.0);
    // Group 50 after the column type is a column height, not the rotation.
    assert_eq!(t.rotation, 0.0);
    assert!(dxf.contains(" 75\r\n1\r\n"), "column type written");
}

#[test]
fn dynamic_manual_heights_and_rotation_round_trip() {
    let c = MTextColumns {
        kind: MTextColumnType::Dynamic,
        count: 3,
        gutter: 0.5,
        height: 5.0,
        auto_height: false,
        heights: vec![5.0, 3.0, 4.0],
        flow_reversed: true,
    };
    let (t, _) = round_trip(mtext("a b c", 0.5, Some(c.clone())));
    assert_eq!(t.columns, Some(c));
    assert!((t.rotation - 0.5).abs() < 1e-9);
    let auto = MTextColumns { kind: MTextColumnType::Dynamic, count: 1, gutter: 0.5, height: 4.0, auto_height: true, ..Default::default() };
    assert_eq!(round_trip(mtext("x", 0.0, Some(auto.clone()))).0.columns, Some(auto));
}

#[test]
fn paragraph_codes_tabs_and_carets_round_trip() {
    let contents = "\\pxi-3,l3,t3;1.\tFirst\\P2.\tSecond {\\H0.7x;\\S+0.1^-0.2;}";
    let (t, dxf) = round_trip(mtext(contents, 0.0, None));
    assert_eq!(t.contents, contents);
    assert!(t.columns.is_none());
    assert!(dxf.contains("1.^IFirst") && dxf.contains("^ -0.2"), "tab as ^I, caret as ^ ");
    // Older files with a bare caret keep it.
    let old = dxf.replace("^ -0.2", "^-0.2");
    let back = read_dxf(old.as_bytes()).unwrap();
    assert!(back.model.iter().any(|e| matches!(&e.kind, EntityKind::MText(m) if m.contents == contents)));
}

#[test]
fn hostile_column_groups_do_not_panic() {
    let mut d = Drawing::new_metric();
    d.add(&Space::Model, Common::default(), EntityKind::MText(mtext("x", 0.0, None))).unwrap();
    let dxf = write_dxf(&d);
    for extra in [
        " 75\r\n2\r\n 76\r\n-5\r\n 50\r\nnan\r\n",
        " 75\r\n1\r\n 76\r\n99999999\r\n 48\r\n-1\r\n 49\r\ninf\r\n",
        " 75\r\n7\r\n",
        &" 75\r\n2\r\n 50\r\n1e308\r\n".repeat(3000),
    ] {
        let bad = dxf.replacen(" 44\r\n", &format!("{extra} 44\r\n"), 1);
        let _ = read_dxf(bad.as_bytes());
    }
}

// ---- #2248: R2018 embedded column object ----

fn embedded_tags(count: i64, width: f64, gutter: f64, heights: &[f64]) -> Vec<cadcraft_dxf::Tag> {
    use cadcraft_dxf::Tag;
    let mut v = vec![
        Tag::s(8, "0"),
        Tag::f(10, 10.0),
        Tag::f(20, 10.0),
        Tag::f(30, 0.0),
        Tag::f(40, 2.5),
        Tag::f(41, width),
        Tag::i(71, 5),
        Tag::i(72, 5),
        Tag::s(1, "Hello"),
        Tag::s(101, "Embedded Object"),
        Tag::i(70, 1),
        Tag::f(10, 1.0),
        Tag::f(20, 0.0),
        Tag::f(30, 0.0),
        Tag::f(11, 10.0),
        Tag::f(21, 10.0),
        Tag::f(31, 0.0),
        Tag::f(40, width),
        Tag::f(41, 0.0),
        Tag::f(42, width),
        Tag::f(43, 0.0),
        Tag::i(71, 1),
        Tag::i(72, count),
        Tag::f(44, width),
        Tag::f(45, gutter),
        Tag::i(73, 0),
        Tag::i(74, 0),
    ];
    v.extend(heights.iter().map(|h| Tag::f(46, *h)));
    v
}

fn mtext_from_groups(tags: &[cadcraft_dxf::Tag]) -> (Common, MText) {
    let Some((common, EntityKind::MText(t))) = entity_from_groups("MTEXT", tags) else { panic!("MTEXT was not read") };
    (common, t)
}

fn dxf_as(t: &MText, version: Version) -> String {
    let mut d = Drawing::new_metric();
    d.add(&Space::Model, Common::default(), EntityKind::MText(t.clone())).unwrap();
    let bytes = write_with(&d, "x.dxf", &SaveOptions { version: Some(version), binary: false }).unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn embedded_object_does_not_leak_into_the_entity() {
    // A single column (the usual R2018 block of a plain MTEXT): 44 is the column width, not the
    // line spacing factor, and the columns are kept.
    let (_, t) = mtext_from_groups(&embedded_tags(1, 38.8, 12.5, &[0.0]));
    assert_eq!(t.line_spacing, 1.0, "44 of the embedded object is the column width");
    assert_eq!(t.attach, 5);
    assert_eq!(t.width, 38.8);
    let c = MTextColumns { kind: MTextColumnType::Static, count: 1, gutter: 12.5, height: 0.0, ..Default::default() };
    assert_eq!(t.columns, Some(c));
}

#[test]
fn embedded_static_columns_are_read() {
    let (_, t) = mtext_from_groups(&embedded_tags(3, 12.5, 2.5, &[20.0, 20.0, 20.0]));
    let c = MTextColumns { kind: MTextColumnType::Static, count: 3, gutter: 2.5, height: 20.0, ..Default::default() };
    assert_eq!(t.columns, Some(c));
}

#[test]
fn r2018_writes_the_embedded_object_and_older_versions_the_legacy_groups() {
    let (_, t) = mtext_from_groups(&embedded_tags(3, 12.5, 2.5, &[20.0, 20.0, 20.0]));
    let r2018 = dxf_as(&t, Version::R2018);
    assert!(r2018.contains("101\r\nEmbedded Object\r\n"), "R2018 keeps the embedded object");
    assert!(!r2018.contains(" 75\r\n"), "no legacy column groups in R2018");
    assert!(r2018.contains(" 72\r\n5\r\n"), "the flow direction (72) is kept");
    assert!(r2018.contains(" 44\r\n12.5\r\n 45\r\n2.5\r\n"), "column width and gutter");
    assert!(!r2018.contains(" 44\r\n1.0\r\n"), "the entity's line spacing is not the column width");
    let back = read_dxf(r2018.as_bytes()).unwrap();
    let Some(EntityKind::MText(b)) = back.model.iter().map(|e| e.kind.clone()).find(|k| matches!(k, EntityKind::MText(_))) else { panic!("no mtext") };
    assert_eq!(b.columns, t.columns);
    let r2000 = dxf_as(&t, Version::R2000);
    assert!(!r2000.contains("Embedded Object"), "R2000 has no embedded object");
    assert!(r2000.contains(" 75\r\n1\r\n"), "R2000 keeps the legacy column groups");
}
