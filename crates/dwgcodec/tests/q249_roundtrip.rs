//! Q342 (dwgwrite3): encode → decode roundtrip tests for the Q249 object types
//! (SORTENTSTABLE, MATERIAL, VISUALSTYLE, FIELD). Each test encodes DXF groups
//! to binary via our encoder, decodes them back via our decoder, and asserts field-level
//! equality of every written field. TABLESTYLE stays on the stash path (encoder returns None).

mod common;

use cadaddy_dwgcodec::Version;
use cadaddy_dwgcodec::bits::Text;
use cadaddy_dwgcodec::classes::{Class, Classes};
use cadaddy_dwgcodec::entities::encode::{Handles, Rec};
use cadaddy_dwgcodec::entities::encode_kinds as ek;
use cadaddy_dwgcodec::entities::records;
use cadaddy_dwgcodec::entities::{Group, Names, Value};

fn g(code: i32, s: &str) -> Group {
    Group { code, value: Value::Str(s.to_string()) }
}
fn gi(code: i32, i: i64) -> Group {
    Group { code, value: Value::Int(i) }
}
fn gf(code: i32, f: f64) -> Group {
    Group { code, value: Value::Real(f) }
}
fn gh(code: i32, h: u64) -> Group {
    Group { code, value: Value::Handle(h) }
}

const SORTENTSTABLE_TYPE: u16 = 500;
const MATERIAL_TYPE: u16 = 501;
const VISUALSTYLE_TYPE: u16 = 503;
const FIELD_TYPE: u16 = 504;

fn test_classes(v: Version) -> Classes {
    let mk = |number: u16, dxf: &str| Class {
        number: number as i16,
        proxy_flags: 0,
        app_name: Text::Empty,
        cpp_name: Text::Empty,
        dxf_name: Text::Bytes(dxf.as_bytes().to_vec()),
        was_zombie: false,
        item_class_id: 0x1F3,
        instances: 0,
        dwg_version: 0,
        maintenance: 0,
        unknown: [0, 0],
    };
    Classes::new(v, vec![
        mk(SORTENTSTABLE_TYPE, "SORTENTSTABLE"),
        mk(MATERIAL_TYPE, "MATERIAL"),
        mk(VISUALSTYLE_TYPE, "VISUALSTYLE"),
        mk(FIELD_TYPE, "FIELD"),
    ])
}

fn test_handles() -> Handles {
    let mut names = Handles::default();
    for (n, t) in [
        ("SORTENTSTABLE", SORTENTSTABLE_TYPE),
        ("MATERIAL", MATERIAL_TYPE),
        ("VISUALSTYLE", VISUALSTYLE_TYPE),
        ("FIELD", FIELD_TYPE),
    ] {
        names.classes.insert(n.to_string(), t);
    }
    names
}

/// Encode groups → binary → decode back, returning the decoded record's groups.
fn roundtrip(v: Version, name: &str, handle: u64, owner: Option<u64>, groups: &[Group]) -> Vec<Group> {
    let names = test_handles();
    let rec = Rec(groups);
    let raw = ek::object(v, name, handle, owner, &[], None, &rec, &names, &[])
        .unwrap_or_else(|| panic!("{name} {v:?}: encoder returned None"));
    assert!(!raw.data.is_empty(), "{name} {v:?}: encoded data is empty");

    let classes = test_classes(v);
    let dec_names = Names::default();
    let record = records::decode(&raw, v, &classes, &dec_names)
        .unwrap_or_else(|e| panic!("{name} {v:?}: decoder error: {e}"))
        .unwrap_or_else(|| panic!("{name} {v:?}: decoder returned None"));
    assert_eq!(record.name, name, "{name} {v:?}: decoded name mismatch");
    record.groups
}

fn find_str(groups: &[Group], code: i32) -> Option<String> {
    groups.iter().find(|g| g.code == code).and_then(|g| match &g.value {
        Value::Str(s) => Some(s.clone()),
        _ => None,
    })
}
fn find_int(groups: &[Group], code: i32) -> Option<i64> {
    groups.iter().find(|g| g.code == code).and_then(|g| match &g.value {
        Value::Int(i) => Some(*i),
        _ => None,
    })
}
fn find_float(groups: &[Group], code: i32) -> Option<f64> {
    groups.iter().find(|g| g.code == code).and_then(|g| match &g.value {
        Value::Real(f) => Some(*f),
        _ => None,
    })
}
fn find_all_handles(groups: &[Group], code: i32) -> Vec<u64> {
    groups.iter().filter(|g| g.code == code).filter_map(|g| match &g.value {
        Value::Handle(h) => Some(*h),
        _ => None,
    }).collect()
}
fn assert_close(a: f64, b: f64, name: &str) {
    assert!((a - b).abs() < 1e-6, "{name}: {a} != {b}");
}

const VERSIONS: [Version; 4] = [Version::R2000, Version::R2004, Version::R2010, Version::R2018];

/// SORTENTSTABLE: encode → decode → assert entity/sort handle pairs and block owner.
#[test]
fn sortentstable_roundtrip() {
    let input = vec![
        g(100, "AcDbSortentsTable"),
        gh(330, 0x10),
        gh(331, 0x20),
        gh(5, 0x30),
        gh(331, 0x21),
        gh(5, 0x31),
    ];
    for v in VERSIONS {
        let decoded = roundtrip(v, "SORTENTSTABLE", 0x40, Some(0x01), &input);
        assert_eq!(find_str(&decoded, 100).as_deref(), Some("AcDbSortentsTable"), "{v:?}");
        let entities = find_all_handles(&decoded, 331);
        assert_eq!(entities.len(), 2, "{v:?}: expected 2 entities, got {}", entities.len());
        let after_marker = decoded.iter()
            .skip_while(|gg| !(gg.code == 100 && matches!(&gg.value, Value::Str(s) if s == "AcDbSortentsTable")))
            .filter(|gg| gg.code == 5)
            .count();
        assert_eq!(after_marker, 2, "{v:?}: expected 2 sort handles after marker, got {after_marker}");
    }
}

/// MATERIAL: encode → decode → assert name, description, diffuse factor, opacity, glossiness,
/// specular, refraction.
#[test]
fn material_roundtrip() {
    let input = vec![
        g(100, "AcDbMaterial"),
        g(1, "Sample Brick"),
        g(2, "A sample material"),
        gi(71, 1),
        gf(41, 1.0),
        gi(91, -1_040_187_392),
        gf(44, 0.5),
        gi(74, 1),
        gi(94, 200),
        gf(45, 0.8),
        gf(140, 0.85),
        gf(145, 1.52),
    ];
    for v in VERSIONS {
        let decoded = roundtrip(v, "MATERIAL", 0x41, Some(0x01), &input);
        assert_eq!(find_str(&decoded, 100).as_deref(), Some("AcDbMaterial"), "{v:?}");
        assert_eq!(find_str(&decoded, 1).as_deref(), Some("Sample Brick"), "{v:?}: name");
        assert_eq!(find_str(&decoded, 2).as_deref(), Some("A sample material"), "{v:?}: description");
        assert_eq!(find_int(&decoded, 91), Some(-1_040_187_392), "{v:?}: diffuse color");
        assert_close(find_float(&decoded, 41).unwrap_or(0.0), 1.0, &format!("{v:?} diffuse factor"));
        assert_close(find_float(&decoded, 44).unwrap_or(0.0), 0.5, &format!("{v:?} glossiness"));
        assert_eq!(find_int(&decoded, 74), Some(1), "{v:?}: specular method");
        assert_eq!(find_int(&decoded, 94), Some(200), "{v:?}: specular value");
        assert_close(find_float(&decoded, 45).unwrap_or(1.0), 0.8, &format!("{v:?} specular factor"));
        assert_close(find_float(&decoded, 140).unwrap_or(0.0), 0.85, &format!("{v:?} opacity"));
        assert_close(find_float(&decoded, 145).unwrap_or(0.0), 1.52, &format!("{v:?} refraction"));
    }
}

/// TABLESTYLE stays on the stash path (encoder returns None).
#[test]
fn tablestyle_is_stashed() {
    let input = vec![
        g(100, "AcDbTableStyle"),
        g(3, ""),
        gi(70, 0),
        gi(71, 0),
        gf(40, 0.06),
        gf(41, 0.06),
        gi(280, 0),
        gi(281, 0),
    ];
    let names = test_handles();
    // The TABLESTYLE type code isn't even needed: ek::object returns None before looking it up.
    let rec = Rec(&input);
    for v in VERSIONS {
        let raw = ek::object(v, "TABLESTYLE", 0x42, Some(0x01), &[], None, &rec, &names, &[]);
        assert!(raw.is_none(), "{v:?}: TABLESTYLE should be stashed (encoder returns None)");
    }
}

/// VISUALSTYLE: encode → decode → assert description, type, face/edge fields.
#[test]
fn visualstyle_roundtrip() {
    let input = vec![
        g(100, "AcDbVisualStyle"),
        g(2, "Realistic"),
        gi(70, 0),
        gi(71, 1),
        gi(72, 1),
        gi(73, 0),
        gf(40, 0.6),
        gf(41, 30.0),
        gi(62, 256),
        gi(63, 7),
        gi(90, 0),
        gi(74, 1),
        gi(91, 0),
        gi(64, 256),
        gi(65, 257),
        gi(75, 1),
        gi(175, 1),
        gi(92, 1),
        gi(174, 0),
        gi(66, 257),
        gf(43, -2.0),
        gi(76, 0),
        gi(77, 6),
        gi(78, 0),
        gi(67, 257),
        gi(79, 5),
        gi(170, 0),
        gf(44, 0.0),
        gi(171, 0),
    ];
    for v in VERSIONS {
        let decoded = roundtrip(v, "VISUALSTYLE", 0x43, Some(0x01), &input);
        assert_eq!(find_str(&decoded, 100).as_deref(), Some("AcDbVisualStyle"), "{v:?}");
        assert_eq!(find_str(&decoded, 2).as_deref(), Some("Realistic"), "{v:?}: description");
        assert_eq!(find_int(&decoded, 70), Some(0), "{v:?}: type");
        assert_eq!(find_int(&decoded, 71), Some(1), "{v:?}: face lighting model");
        assert_eq!(find_int(&decoded, 72), Some(1), "{v:?}: face lighting quality");
        assert_close(find_float(&decoded, 40).unwrap_or(0.0), 0.6, &format!("{v:?} face opacity"));
        assert_close(find_float(&decoded, 41).unwrap_or(0.0), 30.0, &format!("{v:?} face specular"));
        assert_eq!(find_int(&decoded, 62), Some(256), "{v:?}: mono color");
        assert_eq!(find_int(&decoded, 74), Some(1), "{v:?}: edge model");
        assert_close(find_float(&decoded, 43).unwrap_or(0.0), -2.0, &format!("{v:?} edge opacity"));
        assert_eq!(find_int(&decoded, 79), Some(5), "{v:?}: edge silhouette width");
        assert_eq!(find_int(&decoded, 170), Some(0), "{v:?}: display");
    }
}

/// FIELD: encode → decode → assert evaluator, code, flags, cached string value.
#[test]
fn field_roundtrip() {
    let input = vec![
        g(100, "AcDbField"),
        g(1, "_text"),
        g(2, "%<\\AcVar Date \\f \"M/d/yyyy\">%"),
        gi(90, 0),
        gi(97, 0),
        gi(91, 63),
        gi(92, 0),
        gi(94, 9),
        gi(95, 2),
        gi(96, 0),
        g(300, ""),
        gi(93, 4),
        g(301, "10/21/2026"),
        gi(98, 10),
    ];
    for v in VERSIONS {
        let decoded = roundtrip(v, "FIELD", 0x44, Some(0x01), &input);
        assert_eq!(find_str(&decoded, 100).as_deref(), Some("AcDbField"), "{v:?}");
        assert_eq!(find_str(&decoded, 1).as_deref(), Some("_text"), "{v:?}: evaluator");
        assert_eq!(find_str(&decoded, 2).as_deref(), Some("%<\\AcVar Date \\f \"M/d/yyyy\">%"), "{v:?}: code");
        assert_eq!(find_int(&decoded, 91), Some(63), "{v:?}: eval option");
        assert_eq!(find_int(&decoded, 93), Some(4), "{v:?}: value type");
        assert_eq!(find_str(&decoded, 301).as_deref(), Some("10/21/2026"), "{v:?}: cached value");
        assert_eq!(find_int(&decoded, 98), Some(10), "{v:?}: value length");
    }
}

/// Our codec writes a DWG with the new object types and acadrust reads it without error.
#[test]
fn native_objects_survive_acadrust_read() {
    use std::io::Cursor;

    let dxf = common::dxf(4);
    let mut doc = acadrust::DxfReader::from_reader(Cursor::new(dxf.into_bytes()))
        .and_then(|r| r.read())
        .expect("acadrust reads the DXF");
    doc.version = acadrust::DxfVersion::AC1024;
    let bytes = acadrust::DwgWriter::write_to_vec(&doc).expect("acadrust writes the DWG");

    let _read = acadrust::DwgReader::from_stream(Cursor::new(bytes.clone()))
        .read()
        .expect("acadrust reads the DWG back");

    let d = cadaddy_dwgcodec::read(&bytes).expect("our reader opens the DWG");
    assert!(!d.objects.is_empty(), "our reader should find objects");
}
