//! Release 12 DXF output (SAVEAS version 12): `$ACADVER` AC1009, no CLASSES or OBJECTS section,
//! no handles, lightweight polylines as POLYLINE/VERTEX/SEQEND, block references with their
//! attributes, and the file's code page kept. The drawing reads back the same.

use cadaddy_color::{Color, Rgb};
use cadaddy_doc::*;
use cadaddy_geom::{PolyVertex, Vec2, Vec3};
use cadaddy_io::{SaveOptions, Version, read, write_with};

fn text(value: &str, at: Vec3) -> Text {
    Text {
        insert: at,
        align_pt: None,
        height: 2.5,
        value: value.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: HAlign::Left,
        valign: VAlign::Baseline,
    }
}

fn attrib(tag: &str, value: &str, prompt: &str) -> Attrib {
    Attrib {
        tag: tag.into(),
        text: text(value, Vec3::ZERO),
        invisible: false,
        constant: false,
        prompt: prompt.into(),
        props: Default::default(),
        multiline: None,
        modes: Default::default(),
    }
}

fn r12(d: &Drawing) -> Vec<u8> {
    write_with(d, "old.dxf", &SaveOptions { version: Some(Version::R12), binary: false }).unwrap()
}

fn sample() -> Drawing {
    let mut d = Drawing::new_metric();
    d.layers.push(Layer { name: "Walls".into(), color: Color::True(Rgb(250, 0, 0)), linetype: "DASHED".into(), ..Layer::default() });
    d.linetypes.push(Linetype {
        name: "DASHED".into(),
        description: "Dashed __ __".into(),
        pattern: serde_json::from_value(serde_json::json!([{"length": 0.5}, {"length": -0.25}])).unwrap(),
    });
    let walls = Common { layer: "Walls".into(), ..Common::default() };
    let m = Space::Model;
    d.add(&m, walls.clone(), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(10.0, 5.0, 0.0) })).unwrap();
    d.add(&m, Common::default(), EntityKind::Circle(Circle { center: Vec3::new(3.0, 4.0, 0.0), radius: 2.0 })).unwrap();
    d.add(&m, Common::default(), EntityKind::Arc(Arc { center: Vec3::ZERO, radius: 5.0, start: 0.0, end: 1.5 })).unwrap();
    d.add(&m, Common::default(), EntityKind::Text(text("NOTE 1", Vec3::new(1.0, 1.0, 0.0)))).unwrap();
    let mut v = PolyVertex::new(Vec2::new(0.0, 0.0));
    v.bulge = 0.5;
    let pl = LwPolyline {
        vertices: vec![v, PolyVertex::new(Vec2::new(10.0, 0.0)), PolyVertex::new(Vec2::new(10.0, 10.0))],
        closed: true,
        const_width: 0.0,
        elevation: 2.0,
        plinegen: false,
    };
    d.add(&m, walls, EntityKind::LwPolyline(pl)).unwrap();
    d.add(
        &m,
        Common::default(),
        EntityKind::Polyline3d(Polyline3d { points: vec![Vec3::ZERO, Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 5.0, 6.0)], closed: false }),
    )
    .unwrap();
    d.add(
        &m,
        Common::default(),
        EntityKind::Face3d(Face3d {
            corners: [Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(0.0, 1.0, 1.0)],
            hidden_edges: 2,
        }),
    )
    .unwrap();
    let mesh = Mesh {
        vertices: vec![Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)],
        faces: vec![MeshFace { v: vec![0, 1, 2, 3], ..MeshFace::default() }],
        kind: MeshKind::Polyface,
    };
    d.add(&m, Common::default(), EntityKind::Mesh(mesh)).unwrap();
    let mut tag = Block::new("TAG");
    tag.entities.push(Entity::new(Handle(0x40), EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 1.0 })));
    tag.entities.push(Entity::new(Handle(0x41), EntityKind::AttDef(Box::new(attrib("NUM", "?", "Number")))));
    d.blocks.insert("TAG".into(), std::sync::Arc::new(tag));
    let ins = Insert {
        block: "TAG".into(),
        insert: Vec3::new(20.0, 20.0, 0.0),
        scale: Vec3::new(2.0, 2.0, 2.0),
        rotation: 0.5,
        attribs: vec![attrib("NUM", "101", "")],
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    };
    d.add(&m, Common::default(), EntityKind::Insert(ins)).unwrap();
    d
}

fn count(d: &Drawing, f: impl Fn(&EntityKind) -> bool) -> usize {
    d.model.iter().filter(|e| f(&e.kind)).count()
}

#[test]
fn r12_has_only_r12_sections_and_entities() {
    let text = String::from_utf8(r12(&sample())).unwrap();
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    assert!(lines.windows(2).any(|w| w == ["$ACADVER", "1"] || w[0] == "$ACADVER"));
    assert!(text.contains("AC1009"));
    for absent in ["OBJECTS", "CLASSES", "LWPOLYLINE", "AcDb", "$HANDSEED"] {
        assert!(!lines.contains(&absent) && !text.contains(absent), "{absent} in an R12 file");
    }
    for present in ["POLYLINE", "VERTEX", "SEQEND", "ATTRIB", "ATTDEF", "3DFACE", "INSERT", "ENDBLK"] {
        assert!(lines.contains(&present), "no {present}");
    }
    // No handle groups (code 5) on entities: $HANDLING is 0.
    let pairs: Vec<(&str, &str)> = lines.chunks(2).filter_map(|c| Some((*c.first()?, *c.get(1)?))).collect();
    assert!(!pairs.iter().any(|(c, _)| *c == "5"), "handles in an R12 file");
    assert!(pairs.contains(&("62", "1")), "true colour layer written as its nearest ACI");
}

#[test]
fn r12_reads_back_the_same() {
    let d = sample();
    let back = read(&r12(&d), "old.dxf").unwrap();
    assert_eq!(back.header.get("ACADVER").and_then(HVal::as_str), Some("AC1009"));
    assert_eq!(count(&back, |k| matches!(k, EntityKind::Line(_))), 1);
    assert_eq!(count(&back, |k| matches!(k, EntityKind::Circle(_))), 1);
    assert_eq!(count(&back, |k| matches!(k, EntityKind::Arc(_))), 1);
    assert_eq!(count(&back, |k| matches!(k, EntityKind::Face3d(_))), 1);
    let pl = back
        .model
        .iter()
        .find_map(|e| if let EntityKind::LwPolyline(p) = &e.kind { Some((e.common.layer.clone(), p.clone())) } else { None })
        .unwrap();
    assert_eq!(pl.0, "Walls");
    assert!(pl.1.closed && pl.1.vertices.len() == 3 && (pl.1.vertices[0].bulge - 0.5).abs() < 1e-9 && (pl.1.elevation - 2.0).abs() < 1e-9);
    let p3 = back.model.iter().find_map(|e| if let EntityKind::Polyline3d(p) = &e.kind { Some(p.clone()) } else { None }).unwrap();
    assert_eq!(p3.points.len(), 3);
    assert!((p3.points[2].z - 6.0).abs() < 1e-9);
    let mesh = back.model.iter().find_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None }).unwrap();
    assert_eq!((mesh.vertices.len(), mesh.faces.len(), mesh.kind), (4, 1, MeshKind::Polyface));
    let ins = back.model.iter().find_map(|e| if let EntityKind::Insert(i) = &e.kind { Some(i.clone()) } else { None }).unwrap();
    assert_eq!(ins.block, "TAG");
    assert!((ins.scale.x - 2.0).abs() < 1e-9 && (ins.rotation - 0.5).abs() < 1e-9);
    assert_eq!(ins.attribs.iter().map(|a| (a.tag.as_str(), a.text.value.as_str())).collect::<Vec<_>>(), [("NUM", "101")]);
    let blk = back.block("TAG").unwrap();
    assert!(blk.entities.iter().any(|e| matches!(&e.kind, EntityKind::AttDef(a) if a.prompt == "Number")));
    let t = back.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.value.clone()) } else { None });
    assert_eq!(t.as_deref(), Some("NOTE 1"));
    let walls = back.layer("Walls").unwrap();
    assert_eq!((walls.color, walls.linetype.as_str()), (Color::Index(1), "DASHED"));
    assert_eq!(back.linetype("DASHED").map(|l| l.pattern.len()), Some(2));
    // Binary R12 reads back too.
    let bin = write_with(&d, "old.dxf", &SaveOptions { version: Some(Version::R12), binary: true }).unwrap();
    assert_eq!(read(&bin, "old.dxf").unwrap().model.len(), back.model.len());
}

#[test]
fn r12_keeps_the_code_page_and_is_dxf_only() {
    let mut d = sample();
    d.header.set_str("DWGCODEPAGE", "ANSI_1251");
    assert!(String::from_utf8(r12(&d)).unwrap().contains("ANSI_1251"));
    assert!(write_with(&d, "old.dwg", &SaveOptions { version: Some(Version::R12), binary: false }).is_err());
    assert_eq!(Version::parse("12"), Some(Version::R12));
    assert_eq!(Version::parse("R12"), Some(Version::R12));
    assert_eq!(Version::parse("AC1009"), Some(Version::R12));
    assert!(!Version::ALL.contains(&Version::R12));
}

/// Found by opening our R12 files in other programs (Q229): a dimension's text brought down to
/// TEXT reached them as `45\U+00B0` and `\U+230016`, escapes R12 has no meaning for. The signs
/// are R12's control codes.
#[test]
fn r12_writes_degree_plus_minus_and_diameter_as_control_codes() {
    let mut d = Drawing::new_metric();
    for (i, v) in ["45°", "±0.5", "⌀16", "∅8", "Ünïcødé"].iter().enumerate() {
        let h = d.new_handle();
        d.model.push(Entity::new(h, EntityKind::Text(text(v, Vec3::new(0.0, i as f64 * 5.0, 0.0)))));
    }
    let file = String::from_utf8(r12(&d)).unwrap();
    for want in ["45%%d", "%%p0.5", "%%c16", "%%c8"] {
        assert!(file.lines().any(|l| l == want), "{want} missing");
    }
    for gone in ["\\U+00B0", "\\U+00B1", "\\U+2300", "\\U+2205"] {
        assert!(!file.contains(gone), "{gone} written");
    }
    // Other characters keep their escapes, and read back.
    let back = read(file.as_bytes(), "old.dxf").unwrap();
    assert!(back.model.iter().any(|e| matches!(&e.kind, EntityKind::Text(t) if t.value == "Ünïcødé")));
}

/// Found the same way (Q229): R12 files named the built-in font "CADaddy Stroke", which other
/// programs don't have (QCAD reported it missing); the other DXF versions already write `txt`.
#[test]
fn r12_names_the_built_in_font_txt() {
    let d = Drawing::new_metric();
    let file = String::from_utf8(r12(&d)).unwrap();
    assert!(!file.contains("CADaddy Stroke"));
    let lines: Vec<&str> = file.lines().map(str::trim).collect();
    let std = lines.windows(2).position(|w| w == ["2", "Standard"] || w == ["2", "STANDARD"]).unwrap();
    let font = lines[std..].windows(2).find(|w| w[0] == "3").unwrap()[1];
    assert_eq!(font, "txt");
}
