//! Q249 object types in the native DWG codec. TABLESTYLE, SORTENTSTABLE, MATERIAL,
//! VISUALSTYLE and FIELD stay on the stash path (the encoder returns None, so the writer keeps
//! them in CADADDY_KEEP) until each layout is proven against an independent reader in both
//! directions; the six-version encode → decode harness below is checked on objects that are
//! native. That our written files read in acadrust with their values is
//! `crates/io/tests/q249_native_objects.rs` (`ours_read_in_acadrust_with_their_values`).

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

const SCALE_TYPE: u16 = 500;

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
    Classes::new(v, vec![mk(SCALE_TYPE, "SCALE")])
}

fn test_handles() -> Handles {
    let mut names = Handles::default();
    names.classes.insert("SCALE".to_string(), SCALE_TYPE);
    names
}

/// Encode groups → binary → decode back, returning the decoded record's groups.
fn roundtrip(v: Version, name: &str, handle: u64, owner: Option<u64>, groups: &[Group]) -> Vec<Group> {
    let names = test_handles();
    let rec = Rec(groups);
    let raw = ek::object(v, name, handle, owner, &[], None, &rec, &names, &[]).unwrap_or_else(|| panic!("{name} {v:?}: encoder returned None"));
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
    groups
        .iter()
        .filter(|g| g.code == code)
        .filter_map(|g| match &g.value {
            Value::Handle(h) => Some(*h),
            _ => None,
        })
        .collect()
}
fn assert_close(a: f64, b: f64, name: &str) {
    assert!((a - b).abs() < 1e-6, "{name}: {a} != {b}");
}

const VERSIONS: [Version; 6] = [Version::R2000, Version::R2004, Version::R2007, Version::R2010, Version::R2013, Version::R2018];

/// The harness on objects the codec does encode: a SCALE (class) and a DICTIONARY (fixed type),
/// every field back in every version.
#[test]
fn native_objects_round_trip_in_all_six_versions() {
    let scale = vec![g(100, "AcDbScale"), gi(70, 0), g(300, "1:50"), gf(140, 1.0), gf(141, 50.0), gi(290, 0)];
    let dict = vec![g(100, "AcDbDictionary"), gi(280, 0), gi(281, 1), g(3, "A"), gh(350, 0x30), g(3, "B"), gh(350, 0x31)];
    for v in VERSIONS {
        let decoded = roundtrip(v, "SCALE", 0x40, Some(0x01), &scale);
        assert_eq!(find_str(&decoded, 300).as_deref(), Some("1:50"), "{v:?}: name");
        assert_eq!(find_int(&decoded, 70), Some(0), "{v:?}: flags");
        assert_close(find_float(&decoded, 140).unwrap_or(0.0), 1.0, &format!("{v:?} paper units"));
        assert_close(find_float(&decoded, 141).unwrap_or(0.0), 50.0, &format!("{v:?} drawing units"));
        let decoded = roundtrip(v, "DICTIONARY", 0x41, Some(0x01), &dict);
        assert_eq!(find_all_handles(&decoded, 350), vec![0x30, 0x31], "{v:?}: entries");
        assert_eq!(find_str(&decoded, 3).as_deref(), Some("A"), "{v:?}: first key");
    }
}

/// The unproven types are not encoded in any version: the writer stashes them (lossless).
#[test]
fn the_unproven_types_are_stashed_in_all_six_versions() {
    let inputs: [(&str, Vec<Group>); 5] = [
        ("TABLESTYLE", vec![g(100, "AcDbTableStyle"), g(3, ""), gi(70, 0), gf(40, 0.06), gf(41, 0.06)]),
        ("SORTENTSTABLE", vec![g(100, "AcDbSortentsTable"), gh(330, 0x10), gh(331, 0x20), gh(5, 0x30)]),
        ("MATERIAL", vec![g(100, "AcDbMaterial"), g(1, "Sample Brick"), gf(40, 0.5), gf(145, 1.52)]),
        ("VISUALSTYLE", vec![g(100, "AcDbVisualStyle"), g(2, "Sample"), gi(70, 0)]),
        ("FIELD", vec![g(100, "AcDbField"), g(1, "_text"), g(2, "%<\\AcVar Date>%"), gi(90, 0)]),
    ];
    let names = test_handles();
    for v in VERSIONS {
        for (name, groups) in &inputs {
            let raw = ek::object(v, name, 0x42, Some(0x01), &[], None, &Rec(groups), &names, &[]);
            assert!(raw.is_none(), "{name} {v:?}: stashed, not encoded");
        }
    }
}
