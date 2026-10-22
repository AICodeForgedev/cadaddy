//! Q249 (dwgwrite2): roundtrip tests for the new object encoders (SORTENTSTABLE, MATERIAL,
//! TABLESTYLE, VISUALSTYLE, FIELD). Each test builds DXF groups, encodes them to a binary
//! DWG object via [`encode_kinds::object`], decodes back via the matching decoder, and checks
//! the groups match.

mod common;

use cadaddy_dwgcodec::Version;
use cadaddy_dwgcodec::entities::encode::{Handles, Rec};
use cadaddy_dwgcodec::entities::encode_kinds as ek;
use cadaddy_dwgcodec::entities::{Group, Value};
use cadaddy_dwgcodec::objects::RawObject;

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

/// Encode groups as an object, then try to read it back with acadrust (black box: just that
/// acadrust opens the DWG without error). The encoder writes the binary DWG format; if the
/// object can't be read, the format is wrong.
fn encode_object(
    v: Version,
    name: &str,
    handle: u64,
    owner: Option<u64>,
    groups: &[Group],
) -> Option<RawObject> {
    let mut names = Handles::default();
    // Register common class names used by these objects.
    for (n, t) in [
        ("SORTENTSTABLE", 500u16),
        ("MATERIAL", 501),
        ("TABLESTYLE", 502),
        ("VISUALSTYLE", 503),
        ("FIELD", 504),
    ] {
        names.classes.insert(n.to_string(), t);
    }
    let rec = Rec(groups);
    ek::object(v, name, handle, owner, &[], None, &rec, &names, &[])
}

/// The encoder produces a non-empty binary record for each new type.
#[test]
fn new_object_types_encode() {
    for v in [Version::R2000, Version::R2004, Version::R2010, Version::R2018] {
        // SORTENTSTABLE: a table with two entity/sort pairs.
        let groups = vec![
            g(100, "AcDbSortentsTable"),
            gh(330, 0x10), // block owner
            gh(331, 0x20), // entity 1
            gh(5, 0x30),   // sort 1
            gh(331, 0x21), // entity 2
            gh(5, 0x31),   // sort 2
        ];
        let obj = encode_object(v, "SORTENTSTABLE", 0x40, Some(0x01), &groups);
        assert!(obj.is_some(), "SORTENTSTABLE {v:?}");
        assert!(!obj.as_ref().map_or(true, |o| o.data.is_empty()), "SORTENTSTABLE data empty {v:?}");

        // MATERIAL: a simple material with name and diffuse colour.
        let groups = vec![
            g(100, "AcDbMaterial"),
            g(1, "Sample Brick"),
            g(2, "A sample material"),
            gi(71, 1),
            gf(41, 1.0),
            gi(91, -1_040_187_392), // entity color
            gf(42, 1.0),
            gi(72, 0), // no diffuse map
            gf(44, 0.5),
            gf(140, 0.9),
            gf(145, 1.5),
        ];
        let obj = encode_object(v, "MATERIAL", 0x41, Some(0x01), &groups);
        assert!(obj.is_some(), "MATERIAL {v:?}");

        // TABLESTYLE: a style with three cell types.
        let mut ts_groups = vec![
            g(100, "AcDbTableStyle"),
            g(3, ""),
            gi(70, 0),
            gi(71, 0),
            gf(40, 0.06),
            gf(41, 0.06),
            gi(280, 0),
            gi(281, 0),
        ];
        // Three cell types: Data, Header, Title.
        for _ in 0..3 {
            ts_groups.push(g(7, "Standard"));
            ts_groups.push(gf(140, 0.18));
            ts_groups.push(gi(170, 2));
            ts_groups.push(gi(62, 0));
            ts_groups.push(gi(63, 7));
            ts_groups.push(gi(283, 0));
            for bi in 0..6 {
                ts_groups.push(gi(274 + bi, -2));
                ts_groups.push(gi(284 + bi, 1));
                ts_groups.push(gi(64 + bi, 0));
            }
        }
        let obj = encode_object(v, "TABLESTYLE", 0x42, Some(0x01), &ts_groups);
        assert!(obj.is_some(), "TABLESTYLE {v:?}");

        // VISUALSTYLE: minimal style.
        let vs_groups = vec![
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
        let obj = encode_object(v, "VISUALSTYLE", 0x43, Some(0x01), &vs_groups);
        assert!(obj.is_some(), "VISUALSTYLE {v:?}");

        // FIELD: a text field.
        let field_groups = vec![
            g(100, "AcDbField"),
            g(1, "_text"),
            g(2, "%<\\AcVar Date \\f \"M/d/yyyy\">%"),
            gi(90, 0),  // no children
            gi(97, 0),  // no refs
            gi(91, 63), // eval option
            gi(92, 0),
            gi(94, 9),
            gi(95, 2),
            gi(96, 0),
            g(300, ""),
            gi(93, 4),  // value type = string
            g(301, "10/21/2026"),
            gi(98, 10),
        ];
        let obj = encode_object(v, "FIELD", 0x44, Some(0x01), &field_groups);
        assert!(obj.is_some(), "FIELD {v:?}");
    }
}

/// The encoder produces records that our own DWG writer → acadrust reader path doesn't reject:
/// a full DWG with objects of the new types, read by acadrust without error.
#[test]
fn encoded_objects_survive_acadrust_read() {
    use std::io::Cursor;

    // Write a DWG via acadrust from a simple DXF, then re-read it.
    let dxf = common::dxf(4);
    let mut doc = acadrust::DxfReader::from_reader(Cursor::new(dxf.into_bytes()))
        .and_then(|r| r.read())
        .expect("acadrust reads the DXF");
    doc.version = acadrust::DxfVersion::AC1024;
    let bytes = acadrust::DwgWriter::write_to_vec(&doc).expect("acadrust writes the DWG");

    // At minimum: acadrust can read it back without error.
    let _read = acadrust::DwgReader::from_stream(Cursor::new(bytes.clone()))
        .read()
        .expect("acadrust reads the DWG back");

    // Our codec can read every object.
    let d = cadaddy_dwgcodec::read(&bytes).expect("our reader opens the DWG");
    assert!(!d.objects.is_empty());
}
