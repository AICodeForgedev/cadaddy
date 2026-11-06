//! Battle test 4 file-format cleanups: a .cdy padded or extended after its end (B4-021, B4-031),
//! a file whose content isn't what its name says (B4-029) and a DXF with a byte-order mark.

use cadaddy_doc::{Common, Drawing, EntityKind, Space};
use cadaddy_geom::Vec2;

fn drawing() -> Drawing {
    let mut d = Drawing::new_metric();
    for i in 0..20 {
        let (a, b) = (Vec2::new(f64::from(i), 0.0).to3(0.0), Vec2::new(f64::from(i), 5.0).to3(0.0));
        d.add(&Space::Model, Common::default(), EntityKind::Line(cadaddy_doc::Line { a, b })).unwrap();
    }
    d
}

/// B4-021: zeros after a sound file are not a million empty chunks. Checking and reading list
/// what the file holds, whatever follows it.
#[test]
fn zero_padding_is_not_read_as_chunks() {
    let good = cadaddy_io::cdy::write_cdy(&drawing()).unwrap();
    let mut padded = good.clone();
    padded.extend(std::iter::repeat_n(0u8, 8 << 20));
    let check = cadaddy_io::cdy::verify(&padded).unwrap();
    let plain = cadaddy_io::cdy::verify(&good).unwrap();
    // The padding adds no chunks to the report, whatever its length (it was 1 chunk per 28 bytes).
    assert_eq!(check["chunksTotal"], plain["chunksTotal"], "{}", check["chunksTotal"]);
    assert!(check["chunks"].as_array().unwrap().len() <= 100);
    assert_eq!(check["trailing"], 8 << 20);
    assert_eq!(check["ok"], false);
    // The drawing still reads, whole.
    let back = cadaddy_io::cdy::read_cdy(&padded).unwrap();
    assert_eq!(back.model.len(), 20);
}

/// B4-031: bytes appended to a sound file are named as that.
#[test]
fn trailing_bytes_are_reported_as_such() {
    let good = cadaddy_io::cdy::write_cdy(&drawing()).unwrap();
    assert_eq!(cadaddy_io::cdy::verify(&good).unwrap()["trailing"], serde_json::Value::Null);
    let mut longer = good.clone();
    longer.extend_from_slice(b"appended by another program");
    let check = cadaddy_io::cdy::verify(&longer).unwrap();
    assert_eq!(check["trailing"], 27);
    assert_eq!(check["ok"], false);
    assert!(check["chunks"].as_array().unwrap().iter().all(|c| c.get("damage").is_none()), "{check}");
    // A cut-short file is damaged, not "trailing".
    let cut = &good[..good.len() / 2];
    assert_eq!(cadaddy_io::cdy::verify(cut).unwrap()["trailing"], serde_json::Value::Null);
}

/// B4-029: the error says what the file isn't.
#[test]
fn wrong_extension_errors_name_the_format() {
    let e = cadaddy_io::read(b"hello world\nhello world\n", "x.dwg").unwrap_err().to_string();
    assert_eq!(e, "not a DWG file (no AC10xx header)");
    let e = cadaddy_io::read(b"hello", "x.cdy").unwrap_err().to_string();
    assert_eq!(e, "not a CADaddy .cdy file");
    let e = cadaddy_io::read(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n1 0 obj", "x.dwg").unwrap_err().to_string();
    assert!(e.contains("not a DWG file"), "{e}");
    // A name nobody reads is still "unsupported".
    let e = cadaddy_io::read(b"hello world", "x.zzz").unwrap_err().to_string();
    assert!(e.contains("unsupported file type"), "{e}");
    // A DXF named .dwg still opens as the DXF it is.
    let dxf = cadaddy_io::write(&drawing(), "a.dxf").unwrap();
    assert_eq!(cadaddy_io::read(&dxf, "oops.dwg").unwrap().model.len(), 20);
}

/// A DXF that starts with a UTF-8 byte-order mark opens.
#[test]
fn a_dxf_with_a_byte_order_mark_opens() {
    let dxf = cadaddy_io::write(&drawing(), "a.dxf").unwrap();
    let mut with_bom = vec![0xEF, 0xBB, 0xBF];
    with_bom.extend_from_slice(&dxf);
    for name in ["a.dxf", "a", "a.txt"] {
        let d = cadaddy_io::read(&with_bom, name).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(d.model.len(), 20, "{name}");
    }
}
