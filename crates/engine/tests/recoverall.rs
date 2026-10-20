//! RECOVER and RECOVERALL with damaged DWG files (Q183): a drawing cut short opens with what could
//! be read and the recovery report says so; RECOVERALL recovers the drawing and the drawings it
//! references, saving each repaired reference back to its file with the original kept as .bak.

use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::doc::{Drawing, EntityKind};
use serde_json::json;

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-recoverall-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A made-up part drawing of `n` lines as a DWG file, cut to `keep` of its bytes.
fn damaged_part(path: &Path, n: usize, keep: f64) {
    let mut src = String::from("0\nSECTION\n2\nENTITIES\n");
    for i in 0..n {
        src += &format!("0\nLINE\n8\nPART\n10\n{i}\n20\n0\n30\n0\n11\n{i}\n21\n3\n31\n0\n");
    }
    src += "0\nENDSEC\n0\nEOF\n";
    let d = cadaddy_io::read(src.as_bytes(), "part.dxf").unwrap();
    let bytes = cadaddy_io::write(&d, "part.dwg").unwrap();
    std::fs::write(path, &bytes[..(bytes.len() as f64 * keep) as usize]).unwrap();
}

fn lines(d: &Drawing) -> usize {
    d.model.iter().filter(|e| matches!(e.kind, EntityKind::Line(_))).count()
}

#[test]
fn recover_opens_a_dwg_cut_short_with_a_report() {
    hooks();
    let dir = folder("recover");
    let part = dir.join("part.dwg");
    damaged_part(&part, 1500, 0.85);
    let mut s = Session::new();
    let r = s.execute("recover", &json!({ "path": part.to_string_lossy() })).unwrap();
    let kept = lines(s.doc().unwrap());
    assert!(kept > 1000, "{kept} lines kept: {r}");
    let warnings = r["warnings"].to_string();
    assert!(warnings.contains("damaged") && warnings.contains("object map was rebuilt"), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recoverall_recovers_the_drawing_and_its_xrefs() {
    hooks();
    let dir = folder("all");
    let part = dir.join("part.dwg");
    damaged_part(&part, 1500, 0.85);
    let original = std::fs::read(&part).unwrap();
    // A host drawing referencing the part (attached while sound enough to read).
    let host = dir.join("host.dxf");
    let mut s = Session::new();
    s.execute("xattach", &json!({ "path": part.to_string_lossy(), "name": "PART" })).unwrap();
    s.execute("saveas", &json!({ "path": host.to_string_lossy() })).unwrap();
    drop(s);
    let mut s = Session::new();
    let r = s.execute("recoverall", &json!({ "path": host.to_string_lossy() })).unwrap();
    let x = &r["xrefs"][0];
    assert_eq!(x["name"], "PART", "{r}");
    assert_eq!(x["saved"], true, "{r}");
    assert!(x["warnings"].to_string().contains("damaged"), "{r}");
    // The original is kept beside it; the repaired file now reads without damage.
    let bak = dir.join("part.bak");
    assert_eq!(std::fs::read(&bak).unwrap(), original);
    let repaired = cadaddy_io::read(&std::fs::read(&part).unwrap(), "part.dwg").unwrap();
    assert!(repaired.read_notes.is_empty(), "{:?}", repaired.read_notes);
    assert!(lines(&repaired) > 1000);
    // Report only: nothing written.
    let before = std::fs::read(&part).unwrap();
    let r = s.execute("recoverall", &json!({ "path": host.to_string_lossy(), "save": false })).unwrap();
    assert!(r["xrefs"][0].get("saved").is_none(), "{r}");
    assert_eq!(std::fs::read(&part).unwrap(), before);
    // A missing reference is reported, not an error.
    std::fs::remove_file(&part).unwrap();
    let r = s.execute("recoverall", &json!({ "path": host.to_string_lossy() })).unwrap();
    assert_eq!(r["xrefs"][0]["error"], "file not found", "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}
