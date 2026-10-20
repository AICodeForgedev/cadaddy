//! DWGCONVERT: drawings converted in a batch between .dwg, .cdy and .dxf — round trips keep the
//! objects, a bad file doesn't stop the rest, existing outputs are skipped, renamed or overwritten
//! (with a .bak), names ignore case, and a cancel stops after the current drawing.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::convert::{self, Status};
use serde_json::{Value, json};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-dwgconvert-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A made-up drawing with the objects the round-trip tests know, saved at `path`.
fn drawing(path: &Path, n: usize) {
    let mut s = Session::new();
    s.execute("layer", &json!({ "option": "new", "name": "WALLS", "color": 3 })).unwrap();
    s.execute("line", &json!({ "points": [[0, 0], [100, 0], [100, 50]] })).unwrap();
    s.execute("circle", &json!({ "center": [20, 20], "radius": 5 + n })).unwrap();
    s.execute("arc", &json!({ "center": [50, 20], "radius": 8, "start": 0, "end": 90 })).unwrap();
    s.execute("pline", &json!({ "points": [[0, 60], [30, 60], [30, 90]], "closed": true })).unwrap();
    s.execute("text", &json!({ "at": [0, 100], "height": 2.5, "text": format!("Sheet {n}") })).unwrap();
    s.execute("saveas", &json!({ "path": path.to_string_lossy() })).unwrap();
}

/// The model's object kinds, sorted.
fn kinds(path: &Path) -> Vec<&'static str> {
    let d = cadaddy_io::read(&std::fs::read(path).unwrap(), &path.to_string_lossy()).unwrap();
    let mut v: Vec<&'static str> = d.model.iter().map(|e| e.kind.type_name()).collect();
    v.sort_unstable();
    v
}

fn run(s: &mut Session, p: Value) -> Value {
    s.execute("dwgconvert", &p).unwrap_or_else(|e| panic!("{p}: {e}"))
}

fn files(r: &Value, key: &str, field: &str) -> Vec<String> {
    r[key].as_array().unwrap().iter().map(|x| x[field].as_str().unwrap_or_default().to_string()).collect()
}

#[test]
fn dwg_to_cdy_to_dwg_keeps_every_object() {
    hooks();
    let dir = folder("roundtrip");
    let input = dir.join("in");
    std::fs::create_dir_all(input.join("sub/inner")).unwrap();
    drawing(&input.join("plan.dwg"), 1);
    drawing(&input.join("sub/inner/detail.dwg"), 2);
    // An upper-case extension is still a drawing (pathcase).
    std::fs::rename(input.join("sub/inner/detail.dwg"), input.join("sub/inner/detail.DWG")).unwrap();
    let mut s = Session::new();
    let cdy = dir.join("cdy");
    let r = run(&mut s, json!({ "folder": input, "recurse": true, "to": "cdy", "out": cdy }));
    assert_eq!(r["converted"].as_array().unwrap().len(), 2, "{r}");
    assert!(cdy.join("plan.cdy").is_file() && cdy.join("sub/inner/detail.cdy").is_file(), "subfolders kept: {r}");
    // Without recurse, only the folder's own drawings.
    let r = run(&mut s, json!({ "folder": input, "to": "cdy", "out": dir.join("flat") }));
    assert_eq!(files(&r, "converted", "from").len(), 1, "{r}");

    let back = dir.join("back");
    let r = run(&mut s, json!({ "folder": cdy, "recurse": true, "to": ".DWG", "out": back }));
    assert_eq!(r["converted"].as_array().unwrap().len(), 2, "{r}");
    assert!(r["converted"].as_array().unwrap().iter().all(|c| c.get("losses").is_none()), "a current DWG keeps it all: {r}");
    assert_eq!(kinds(&back.join("plan.dwg")), kinds(&input.join("plan.dwg")));
    assert_eq!(kinds(&back.join("sub/inner/detail.dwg")), kinds(&input.join("sub/inner/detail.DWG")));
    assert!(kinds(&back.join("plan.dwg")).len() >= 5);
    // And to DXF, a version chosen.
    let r = run(&mut s, json!({ "files": [back.join("plan.dwg")], "to": "dxf", "version": 2004, "out": dir.join("dxf") }));
    assert_eq!(kinds(&dir.join("dxf/plan.dxf")), kinds(&input.join("plan.dwg")), "{r}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_bad_file_is_reported_and_the_rest_convert() {
    hooks();
    let dir = folder("bad");
    drawing(&dir.join("a.dxf"), 1);
    std::fs::write(dir.join("broken.dwg"), b"AC1032 garbage").unwrap();
    drawing(&dir.join("c.dxf"), 3);
    std::fs::write(dir.join("notes.txt"), "not a drawing").unwrap();
    let mut s = Session::new();
    let r = run(&mut s, json!({ "folder": dir, "to": "cdy", "out": dir.join("out"), "log": true }));
    assert_eq!(files(&r, "converted", "from").len(), 2, "{r}");
    let failed = files(&r, "failed", "file");
    assert!(failed.len() == 1 && failed[0].ends_with("broken.dwg"), "{r}");
    assert!(!r["failed"][0]["error"].as_str().unwrap().is_empty());
    assert!(!dir.join("out/notes.cdy").exists(), "only drawings");
    let log = std::fs::read_to_string(r["log"].as_str().unwrap()).unwrap();
    assert!(log.contains("FAILED") && log.contains("2 converted, 0 skipped, 1 failed of 3"), "{log}");
    // A file that isn't there fails alone.
    let r = run(&mut s, json!({ "files": [dir.join("a.dxf"), dir.join("missing.dwg")], "to": "dwg", "out": dir.join("o2") }));
    assert_eq!((files(&r, "converted", "from").len(), files(&r, "failed", "file").len()), (1, 1), "{r}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn existing_outputs_are_skipped_renamed_or_overwritten() {
    hooks();
    let dir = folder("exists");
    drawing(&dir.join("plan.dxf"), 1);
    let mut s = Session::new();
    let out = dir.join("out");
    let p = |o: Value| json!({ "files": [dir.join("plan.dxf")], "to": "dwg", "out": out, "overwrite": o });
    run(&mut s, p(Value::Null));
    // Default: skip.
    let r = run(&mut s, p(Value::Null));
    assert_eq!(files(&r, "skipped", "file").len(), 1, "{r}");
    let r = run(&mut s, p(json!("rename")));
    assert!(files(&r, "converted", "to")[0].ends_with("plan (2).dwg"), "{r}");
    assert!(out.join("plan (2).dwg").is_file());
    let r = run(&mut s, p(json!(true)));
    assert!(files(&r, "converted", "to")[0].ends_with("plan.dwg"), "{r}");
    assert!(out.join("plan.bak").is_file(), "the file written over is kept as .bak");

    // Next to the original, the same format: only over itself with a .bak.
    let here = json!({ "files": [dir.join("plan.dxf")], "to": "dxf", "version": 2000 });
    let r = run(&mut s, here.clone());
    assert_eq!(files(&r, "skipped", "file").len(), 1, "{r}");
    let mut over = here.clone();
    over["overwrite"] = json!("overwrite");
    let r = run(&mut s, over.clone());
    assert_eq!(files(&r, "converted", "to").len(), 1, "{r}");
    assert!(dir.join("plan.bak").is_file(), "the original is the .bak");
    over["bak"] = json!(false);
    let r = run(&mut s, over);
    assert_eq!(files(&r, "skipped", "file").len(), 1, "never over itself without a .bak: {r}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn names_ignore_case() {
    hooks();
    let dir = folder("case");
    drawing(&dir.join("PLAN.dwg"), 1);
    drawing(&dir.join("plan.dxf"), 2);
    let mut s = Session::new();
    // PLAN.dwg and plan.dxf both become plan.cdy: one file on Windows and macOS.
    let r = run(&mut s, json!({ "folder": dir, "to": "CDY", "out": dir.join("out") }));
    assert_eq!(files(&r, "converted", "from").len(), 1, "{r}");
    assert!(r["failed"][0]["error"].as_str().unwrap().contains("already became"), "{r}");
    // Renaming takes the next free name instead.
    let r = run(&mut s, json!({ "folder": dir, "to": "cdy", "out": dir.join("out2"), "overwrite": "rename" }));
    assert_eq!(files(&r, "converted", "from").len(), 2, "{r}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn cancel_stops_after_the_current_drawing() {
    hooks();
    let dir = folder("cancel");
    for i in 0..4 {
        drawing(&dir.join(format!("d{i}.dxf")), i);
    }
    let inputs = convert::folder_inputs(&dir, false).unwrap();
    let job = convert::Job { inputs, to: "cdy".into(), out: Some(dir.join("out")), ..convert::Job::default() };
    let cancel = AtomicBool::new(false);
    let mut seen = Vec::new();
    let sum = convert::run(&job, None, &cancel, &mut |i, o| {
        seen.push((i, o.status));
        cancel.store(true, Ordering::Relaxed);
    });
    assert!(sum.cancelled);
    assert_eq!(seen, [(0, Status::Converted)]);
    assert_eq!(sum.to_json()["total"], 4);
    assert!(dir.join("out/d0.cdy").is_file() && !dir.join("out/d1.cdy").exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_older_dwg_lists_what_it_did_not_keep() {
    hooks();
    let dir = folder("losses");
    let mut s = Session::new();
    let h = s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap()["handles"][0].clone();
    s.execute("chprop", &json!({ "handles": [h], "transparency": 50 })).unwrap();
    s.execute("saveas", &json!({ "path": dir.join("t.cdy") })).unwrap();
    let r = run(&mut s, json!({ "files": [dir.join("t.cdy")], "to": "dwg", "version": "2000" }));
    let losses = r["converted"][0]["losses"].as_array().unwrap_or_else(|| panic!("{r}"));
    assert!(losses.iter().any(|l| l["feature"] == "object transparency"), "{r}");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn bad_parameters_are_errors_not_panics() {
    hooks();
    let mut s = Session::new();
    for p in [
        json!(null),
        json!({}),
        json!({ "to": "cdy" }),
        json!({ "files": ["x.dwg"] }),
        json!({ "files": ["x.dwg"], "to": "docx" }),
        json!({ "files": 5, "to": "dwg" }),
        json!({ "files": [5], "to": "dwg" }),
        json!({ "files": ["x.dwg"], "to": "dwg", "version": 12 }),
        json!({ "files": ["x.dwg"], "to": "dwg", "overwrite": "maybe" }),
        json!({ "folder": "/no/such/folder/anywhere", "to": "dwg" }),
    ] {
        assert!(s.execute("dwgconvert", &p).is_err(), "{p}");
    }
    // The aliases reach it.
    assert!(s.execute("convert", &json!({ "files": ["/no/such.dwg"], "to": "cdy" })).unwrap()["failed"].as_array().unwrap().len() == 1);
}

/// A drawing keeps its own DWG version through .cdy when none is asked for, 2000 included
/// (REPORT-0.7.8-battle #16).
#[test]
fn each_drawing_keeps_its_own_dwg_version() {
    hooks();
    let dir = folder("ownversion");
    let input = dir.join("in");
    std::fs::create_dir_all(&input).unwrap();
    for (name, version) in [("old.dwg", 2000), ("new.dwg", 2018)] {
        let mut s = Session::new();
        s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap();
        s.execute("saveas", &json!({ "path": input.join(name).to_string_lossy(), "version": version })).unwrap();
    }
    let id = |p: &Path| String::from_utf8_lossy(&std::fs::read(p).unwrap()[..6]).to_string();
    assert_eq!(id(&input.join("old.dwg")), "AC1015");
    let mut s = Session::new();
    let cdy = dir.join("cdy");
    run(&mut s, json!({ "folder": input, "to": "cdy", "out": cdy }));
    let back = dir.join("back");
    let r = run(&mut s, json!({ "folder": cdy, "to": "dwg", "out": back }));
    assert_eq!(r["converted"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(id(&back.join("old.dwg")), "AC1015", "{r}");
    assert_eq!(id(&back.join("new.dwg")), "AC1032", "{r}");
    std::fs::remove_dir_all(&dir).ok();
}
