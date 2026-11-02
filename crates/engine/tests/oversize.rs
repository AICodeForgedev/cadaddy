//! Objects whose size puts them beyond ±1e12 (a circle of radius 1e300, an insert scaled 1e13)
//! are dropped with a counted note when a file is opened, inserted or attached, as objects with
//! coordinates there are; a design file whose elements lie outside their own ranges attaches
//! with a warning; `drawing.inspect` names the drawing's units (battle test 7).

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-oversize-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A millimetre drawing of a line and a circle of radius 1e300.
const HUGE_CIRCLE: &str = "0\nSECTION\n2\nHEADER\n9\n$INSUNITS\n70\n4\n0\nENDSEC\n0\nSECTION\n2\nENTITIES\n\
0\nLINE\n8\n0\n10\n0\n20\n0\n11\n10\n21\n0\n0\nCIRCLE\n8\n0\n10\n0\n20\n0\n40\n1e300\n0\nENDSEC\n0\nEOF\n";

const NOTE: &str = "1 object had coordinates or a size beyond \u{b1}1e12";

fn huge_file(tag: &str) -> String {
    let path = dir(tag).join("Sample Part.dxf");
    std::fs::write(&path, HUGE_CIRCLE).unwrap();
    path.display().to_string()
}

fn warned(v: &Value, key: &str) -> bool {
    v[key].as_array().is_some_and(|a| a.iter().any(|w| w.as_str().is_some_and(|w| w.contains(NOTE))))
}

#[test]
fn opening_drops_the_oversize_object_says_so_and_inspect_names_the_units() {
    hooks();
    let path = huge_file("open");
    let mut s = Session::new();
    let r = s.execute("open", &json!({ "path": path })).unwrap();
    assert!(warned(&r, "warnings"), "{r}");
    let v = s.execute("drawing.inspect", &json!({ "entities": false })).unwrap();
    assert_eq!(v["entityCount"], json!(1), "{v}");
    assert_eq!(v["units"], json!("Millimeters"), "{v}");
    let ext = v["extents"].as_array().unwrap();
    assert_eq!(ext[1], json!([10.0, 0.0]), "{v}");
    // A drawing that declares no units says so.
    let mut t = Session::new();
    t.doc_mut().unwrap().header.set_i64("INSUNITS", 0);
    assert_eq!(t.execute("drawing.inspect", &json!({ "entities": false })).unwrap()["units"], json!("Unitless"));
}

#[test]
fn inserting_or_attaching_the_file_drops_it_with_the_note() {
    hooks();
    let path = huge_file("insert");
    let mut s = Session::new();
    let r = s.execute("insert", &json!({ "file": path, "at": [0, 0] })).unwrap();
    assert!(warned(&r, "warnings"), "{r}");
    let block = s.doc().unwrap().block("Sample Part").map(|b| b.entities.len());
    assert_eq!(block, Some(1), "only the line made it into the block");
    let r = s.execute("xattach", &json!({ "path": path, "at": [0, 0], "name": "Part Ref" })).unwrap();
    assert!(r["xref"]["reason"].as_str().is_some_and(|w| w.contains(NOTE)), "{r}");
}

#[test]
fn a_design_file_with_elements_outside_their_ranges_attaches_with_a_warning() {
    hooks();
    let d = dir("range");
    let path = d.join("site.dgn").display().to_string();
    let mut s = Session::new();
    s.execute("line", &json!({ "points": [[0, 0], [40, 0]] })).unwrap();
    s.execute("line", &json!({ "points": [[0, 10], [40, 10]] })).unwrap();
    s.execute("dgnexport", &json!({ "path": path })).unwrap();
    let whole = s.execute("dgnattach", &json!({ "path": path })).unwrap();
    assert_eq!(whole["warnings"], json!([]), "{whole}");
    // A flipped bit in the second line's end point (the elements follow the TCB and colour table).
    let mut bytes = std::fs::read(&path).unwrap();
    let mut at = 0usize;
    let mut lines = Vec::new();
    while at + 4 <= bytes.len() && bytes[at..at + 2] != [0xff, 0xff] {
        if bytes[at + 1] & 0x7f == 3 {
            lines.push(at);
        }
        at += 4 + 2 * usize::from(u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]));
    }
    let line = *lines.get(1).expect("two line elements");
    bytes[line + 44 + 1] ^= 0x08;
    let hurt = d.join("hurt.dgn").display().to_string();
    std::fs::write(&hurt, &bytes).unwrap();
    let mut t = Session::new();
    let r = t.execute("dgnattach", &json!({ "path": hurt })).unwrap();
    let said = r["warnings"].as_array().unwrap();
    assert!(said.iter().any(|w| w.as_str().unwrap().contains("1 element lies outside the range its header gives")), "{r}");
}
