//! ETRANSMIT: a drawing with nested xrefs and images packed into a ZIP archive or a folder, with
//! every path rewritten relative so the package opens anywhere.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io, zip};
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Block, Drawing, Entity, EntityKind, Insert, Line, XrefStatus};
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-etransmit-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save(d: &Drawing, path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, cadaddy_io::write(d, &path.to_string_lossy()).unwrap()).unwrap();
}

fn line(d: &mut Drawing, y: f64) {
    let h = d.new_handle();
    d.model.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(0.0, y, 0.0), b: Vec3::new(10.0, y, 0.0) })));
}

fn image(d: &mut Drawing, path: &str) {
    let im = serde_json::from_value(
        json!({ "insert": {"x":0,"y":0,"z":0}, "u": {"x":1,"y":0,"z":0}, "v": {"x":0,"y":1,"z":0}, "size": {"x":4,"y":4}, "path": path }),
    )
    .unwrap();
    let h = d.new_handle();
    d.model.push(Entity::new(h, EntityKind::Image(im)));
}

fn xref(d: &mut Drawing, name: &str, path: &str) {
    let mut b = Block::new(name);
    b.xref_path = Some(path.into());
    d.blocks.insert(name.into(), Arc::new(b));
    let h = d.new_handle();
    d.model.push(Entity::new(
        h,
        EntityKind::Insert(Insert {
            block: name.into(),
            insert: Vec3::ZERO,
            scale: Vec3::new(1.0, 1.0, 1.0),
            rotation: 0.0,
            attribs: Vec::new(),
            cols: 1,
            rows: 1,
            col_spacing: 0.0,
            row_spacing: 0.0,
            clip: None,
        }),
    ));
}

/// proj/host.dxf → parts/a.dxf (→ b.dxf beside it, images ../img/photo.png and other.png),
/// img/photo.png, and a missing gone.dxf.
fn project(tag: &str) -> (PathBuf, Session) {
    hooks();
    let dir = folder(tag);
    let mut b = Drawing::new_metric();
    line(&mut b, 0.0);
    save(&b, &dir.join("parts/b.dxf"));
    let mut a = Drawing::new_metric();
    line(&mut a, 1.0);
    xref(&mut a, "b", "b.dxf");
    image(&mut a, "../img/photo.png");
    image(&mut a, "../img/other.png");
    save(&a, &dir.join("parts/a.dxf"));
    std::fs::create_dir_all(dir.join("img")).unwrap();
    std::fs::write(dir.join("img/photo.png"), b"photo bytes").unwrap();
    std::fs::write(dir.join("img/other.png"), b"other bytes").unwrap();
    let mut host = Drawing::new_metric();
    xref(&mut host, "a", "parts/a.dxf");
    xref(&mut host, "gone", "gone.dxf");
    image(&mut host, "img/photo.png");
    save(&host, &dir.join("host.dxf"));
    let mut s = Session::new();
    s.execute("open", &json!({ "path": dir.join("host.dxf").to_string_lossy() })).unwrap();
    (dir, s)
}

fn entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    zip::read(&std::fs::read(path).unwrap()).unwrap()
}

fn drawing(entries: &[(String, Vec<u8>)], name: &str) -> Drawing {
    let (_, b) = entries.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("{name} not in the package"));
    cadaddy_io::read(b, name).unwrap()
}

fn image_paths(d: &Drawing) -> Vec<String> {
    let mut v: Vec<String> = d.model.iter().filter_map(|e| if let EntityKind::Image(i) = &e.kind { Some(i.path.clone()) } else { None }).collect();
    v.sort();
    v
}

#[test]
fn a_zip_holds_every_file_with_relative_paths() {
    let (dir, mut s) = project("zip");
    let out = dir.join("out.zip");
    let r = s.execute("etransmit", &json!({ "path": out.to_string_lossy() })).unwrap();
    let e = entries(&out);
    let mut names: Vec<&str> = e.iter().map(|(n, _)| n.as_str()).collect();
    names.sort();
    assert_eq!(names, ["Images/other.png", "Images/photo.png", "Xrefs/a.dxf", "Xrefs/b.dxf", "host.dxf", "transmittal.html", "transmittal.txt"]);
    // The host points into the package.
    let host = drawing(&e, "host.dxf");
    assert_eq!(host.block("a").unwrap().xref_path.as_deref(), Some("Xrefs\\a.dxf"));
    assert_eq!(image_paths(&host), ["Images\\photo.png"]);
    // The nested xref's own paths, relative to where it landed; the shared image is packed once.
    let a = drawing(&e, "Xrefs/a.dxf");
    assert_eq!(a.block("b").unwrap().xref_path.as_deref(), Some("b.dxf"));
    assert_eq!(image_paths(&a), ["..\\Images\\other.png", "..\\Images\\photo.png"]);
    // A file whose paths didn't change is copied as it is.
    let b = e.iter().find(|(n, _)| n == "Xrefs/b.dxf").unwrap();
    assert_eq!(b.1, std::fs::read(dir.join("parts/b.dxf")).unwrap());
    // The missing xref is reported, and the report lists the files.
    assert!(r["left"].as_array().unwrap().iter().any(|l| l["name"] == "gone.dxf" && l["note"] == "not found"), "{}", r["left"]);
    let report = String::from_utf8(e.iter().find(|(n, _)| n == "transmittal.txt").unwrap().1.clone()).unwrap();
    assert!(report.contains("Xrefs/b.dxf") && report.contains("gone.dxf"), "{report}");
}

#[test]
fn a_packed_folder_opens_with_its_xrefs_loaded() {
    let (dir, mut s) = project("folder");
    let out = dir.join("package");
    s.execute("etransmit", &json!({ "path": out.to_string_lossy() })).unwrap();
    // Move the package away from the originals: everything still resolves.
    let moved = folder("folder-moved").join("pkg");
    std::fs::rename(&out, &moved).unwrap();
    let mut t = Session::new();
    t.execute("open", &json!({ "path": moved.join("host.dxf").to_string_lossy() })).unwrap();
    let d = t.doc().unwrap();
    let a = d.block("a").unwrap();
    assert_eq!((a.xref_status, a.entities.len() >= 2), (XrefStatus::Loaded, true), "{:?}", a.xref_reason);
    assert!(d.block("a|b").is_some() || d.blocks.keys().any(|k| k.to_ascii_lowercase().contains("|b")), "the nested xref loads too");
    // A folder that isn't empty is refused.
    let e = s.execute("etransmit", &json!({ "path": moved.to_string_lossy() })).unwrap_err();
    assert!(e.to_string().contains("empty folder"), "{e}");
}

#[test]
fn flat_layout_listing_and_bad_options() {
    let (dir, mut s) = project("flat");
    let out = dir.join("flat.zip");
    s.execute("etransmit", &json!({ "path": out.to_string_lossy(), "layout": "flat", "report": false })).unwrap();
    let e = entries(&out);
    let mut names: Vec<&str> = e.iter().map(|(n, _)| n.as_str()).collect();
    names.sort();
    assert_eq!(names, ["a.dxf", "b.dxf", "host.dxf", "other.png", "photo.png"]);
    assert_eq!(drawing(&e, "host.dxf").block("a").unwrap().xref_path.as_deref(), Some("a.dxf"));
    assert_eq!(image_paths(&drawing(&e, "a.dxf")), ["other.png", "photo.png"]);
    // Without a path: only the list.
    let r = s.execute("etransmit", &json!({})).unwrap();
    assert_eq!(r["files"].as_array().unwrap().len(), 7);
    assert!(r.get("path").is_none());
    for p in [json!({ "layout": "tree" }), json!({ "path": "/dev/null/x.zip" })] {
        assert!(s.execute("etransmit", &p).is_err(), "{p}");
    }
    let _: Value = r;
}

#[test]
fn an_unsaved_drawing_is_packed_under_its_title() {
    hooks();
    let mut s = Session::new();
    s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    let r = s.execute("etransmit", &json!({ "report": false })).unwrap();
    let files = r["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert!(files[0]["target"].as_str().unwrap().ends_with(".dwg"), "{}", files[0]);
}
