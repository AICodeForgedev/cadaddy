//! CADaddy's own format (.cdy) through every file path: SAVEAS, QSAVE, OPEN (by path and by
//! data), RECOVER, the DWG copy (DWGCOPY), document.bytes, and the notice when saving into DWG or
//! DXF would lose part of a drawing.

use std::path::PathBuf;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, base64_encode, set_dwg_copy, set_io};
use serde_json::{Value, json};

/// DWGCOPY is process-wide: the tests that save .cdy files take turns.
static DWGCOPY: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

/// A fresh folder for one test.
fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-cdy-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn path(dir: &std::path::Path, name: &str) -> String {
    dir.join(name).to_string_lossy().into_owned()
}

fn drawn() -> Session {
    hooks();
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [10, 5]]})).unwrap();
    s.execute("circle", &json!({"center": [3, 3], "radius": 1.25})).unwrap();
    s.execute("layer", &json!({"name": "Walls", "color": 3})).unwrap();
    s
}

#[test]
fn saveas_cdy_then_open_gives_the_drawing_exactly() {
    let _turn = DWGCOPY.lock().unwrap_or_else(|e| e.into_inner());
    let dir = folder("saveas");
    let mut s = drawn();
    let p = path(&dir, "Plan.cdy");
    let r = s.execute("saveas", &json!({"path": p})).unwrap();
    // Saving stamps TDCREATE/TDUPDATE into the drawing; the file holds the stamped drawing.
    let before = s.doc().unwrap().clone();
    assert_eq!(r["path"], json!(p), "{r}");
    assert!(r.get("version").is_none(), "a .cdy file has no DWG version: {r}");
    assert!(r.get("notKept").is_none(), "{r}");
    let bytes = std::fs::read(&p).unwrap();
    assert!(cadaddy_io::cdy::is_cdy(&bytes));
    assert!(cadaddy_io::cdy::thumbnail(&bytes).is_some(), "a preview for file browsers");
    let st = s.state().unwrap();
    assert_eq!((st.title.as_str(), st.is_dirty()), ("Plan.cdy", false));
    // Opened again in a second session: exactly what was saved.
    let mut t = Session::new();
    let r = t.execute("open", &json!({"path": p})).unwrap();
    assert_eq!(r["warnings"], json!([]), "{r}");
    assert_eq!(t.doc().unwrap(), &before);
    // QSAVE writes .cdy again and keeps the previous file as .bak.
    s.execute("line", &json!({"points": [[0, 0], [0, 9]]})).unwrap();
    let r = s.execute("qsave", &json!({})).unwrap();
    assert_eq!(r["path"], json!(p));
    assert!(cadaddy_io::cdy::is_cdy(&std::fs::read(&p).unwrap()));
    assert!(cadaddy_io::cdy::is_cdy(&std::fs::read(path(&dir, "Plan.bak")).unwrap()));
    // Opening by data (the web, agents) reads .cdy by its content whatever the name says.
    let mut u = Session::new();
    u.execute("open", &json!({"data": base64_encode(&bytes), "name": "upload.dxf"})).unwrap();
    assert_eq!(u.doc().unwrap(), &before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dwg_copy_is_saved_beside_a_cdy_file_when_asked() {
    let _turn = DWGCOPY.lock().unwrap_or_else(|e| e.into_inner());
    let dir = folder("copy");
    let mut s = drawn();
    let p = path(&dir, "Site.cdy");
    let copy = path(&dir, "Site.dwg");
    set_dwg_copy(false);
    assert_eq!(s.execute("getvar", &json!({"name": "DWGCOPY"})).unwrap()["value"], json!(0));
    let r = s.execute("saveas", &json!({"path": p})).unwrap();
    assert!(r.get("dwgCopy").is_none() && !std::path::Path::new(&copy).exists(), "{r}");
    s.execute("setvar", &json!({"name": "DWGCOPY", "value": 1})).unwrap();
    let r = s.execute("qsave", &json!({})).unwrap();
    assert_eq!(r["dwgCopy"], json!(copy), "{r}");
    assert!(std::fs::read(&copy).unwrap().starts_with(b"AC10"), "DWG content");
    // Saving again replaces the copy without a .bak of its own: Site.bak stays the drawing's.
    s.execute("line", &json!({"points": [[0, 0], [2, 2]]})).unwrap();
    s.execute("qsave", &json!({})).unwrap();
    assert!(cadaddy_io::cdy::is_cdy(&std::fs::read(path(&dir, "Site.bak")).unwrap()));
    // The copy doesn't become the drawing's file.
    assert_eq!(s.state().unwrap().path.as_deref(), Some(p.as_str()));
    // Saving into DWG itself makes no copy of a copy.
    let r = s.execute("saveas", &json!({"path": path(&dir, "Other.dwg")})).unwrap();
    assert!(r.get("dwgCopy").is_none(), "{r}");
    set_dwg_copy(false);
    let _ = std::fs::remove_dir_all(&dir);
}

/// DXF text from lines of `code value`.
fn dxf(src: &str) -> String {
    src.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| {
            let (c, v) = l.split_once(' ').unwrap_or((l, ""));
            format!("{c}\n{v}\n")
        })
        .collect()
}

#[test]
fn saving_into_dwg_says_once_what_the_file_would_not_keep() {
    hooks();
    let dir = folder("notkept");
    // Another program's object type, which DWG doesn't keep (DXF does, and .cdy does).
    let src = dxf("0 SECTION\n2 ENTITIES\n0 LINE\n8 0\n10 0\n20 0\n11 1\n21 1\n0 ACME_WIDGET\n8 0\n5 3A0\n10 1\n20 1\n0 ENDSEC\n0 EOF");
    let mut s = Session::new();
    s.execute("open", &json!({"data": base64_encode(src.as_bytes()), "name": "in.dxf"})).unwrap();
    let r = s.execute("saveas", &json!({"path": path(&dir, "a.dxf")})).unwrap();
    assert!(r.get("notKept").is_none(), "DXF keeps it: {r}");
    let mut s = Session::new();
    s.execute("open", &json!({"data": base64_encode(src.as_bytes()), "name": "in.dxf"})).unwrap();
    let r = s.execute("saveas", &json!({"path": path(&dir, "a.dwg")})).unwrap();
    assert_eq!(r["notKept"], json!(["objects"]), "{r}");
    assert!(s.log.iter().any(|l| l.contains("Not kept in the DWG file: objects") && l.contains(".cdy")), "{:?}", s.log);
    // Once per drawing.
    let r = s.execute("qsave", &json!({})).unwrap();
    assert!(r.get("notKept").is_none(), "{r}");
    // A drawing CADaddy made itself loses nothing in DXF or DWG: no notice.
    let mut t = drawn();
    for name in ["b.dxf", "b.dwg"] {
        let r = t.execute("saveas", &json!({"path": path(&dir, name)})).unwrap();
        assert!(r.get("notKept").is_none(), "{name}: {r}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recover_and_bytes_read_and_write_cdy() {
    let dir = folder("recover");
    let mut s = drawn();
    let before = s.doc().unwrap().clone();
    let r = s.execute("document.bytes", &json!({"format": "cdy"})).unwrap();
    let data = r["data"].as_str().unwrap();
    let bytes = cadaddy_engine::cmd::file::base64_decode(data).unwrap();
    assert!(cadaddy_io::cdy::is_cdy(&bytes));
    let p = path(&dir, "Plan_1-a_2.sv$");
    std::fs::write(&p, &bytes).unwrap();
    let mut t = Session::new();
    let r: Value = t.execute("recover", &json!({"path": p, "copy": true, "title": "Plan"})).unwrap();
    assert!(r.get("uid").is_some(), "{r}");
    assert_eq!(t.doc().unwrap().model, before.model);
    let _ = std::fs::remove_dir_all(&dir);
}
