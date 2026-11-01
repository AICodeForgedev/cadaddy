//! Output commands never write over the open drawing or onto a path of the wrong kind, and
//! REDLINE / WALK.PUBLISH keep a file that is already beside the drawing.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use serde_json::json;
use std::path::PathBuf;

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    cadaddy_engine::cmd::file::set_pdf_merge(|parts| cadaddy_io::pdf_merge::merge(parts).map_err(|e| e.to_string()));
    cadaddy_engine::cmd::redline::set_page_count(cadaddy_io::pdf_markups::page_count);
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-plotover-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A saved drawing `name` in `d`.
fn saved(d: &std::path::Path, name: &str) -> (Session, String) {
    hooks();
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [100, 50]]})).unwrap();
    let path = d.join(name).to_string_lossy().into_owned();
    s.execute("saveas", &json!({ "path": path })).unwrap();
    (s, path)
}

fn err(s: &mut Session, cmd: &str, p: serde_json::Value) -> String {
    s.execute(cmd, &p).expect_err(cmd).to_string()
}

#[test]
fn plot_over_the_open_drawing_is_refused_and_the_file_is_unchanged() {
    let d = dir("open");
    let (mut s, path) = saved(&d, "victim.dxf");
    let before = std::fs::read(&path).unwrap();
    let e = err(&mut s, "plot", json!({ "path": path }));
    assert!(e.contains(".dxf") && e.contains(".pdf"), "names both: {e}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn a_path_of_the_wrong_kind_is_refused() {
    let d = dir("kind");
    let (mut s, _) = saved(&d, "plan.dxf");
    let png = d.join("x.png").to_string_lossy().into_owned();
    let e = err(&mut s, "plot", json!({ "path": png }));
    assert!(e.contains(".png") && e.contains(".pdf"), "{e}");
    assert!(!d.join("x.png").exists());
    // The right kind works, and a name without an extension gets one.
    let r = s.execute("plot", &json!({ "path": d.join("a.pdf").to_string_lossy() })).unwrap();
    assert!(r["path"].as_str().unwrap().ends_with("a.pdf"));
    let r = s.execute("plot", &json!({ "path": d.join("b").to_string_lossy() })).unwrap();
    assert!(r["path"].as_str().unwrap().ends_with("b.pdf"), "{r}");
    assert!(d.join("b.pdf").exists());
}

#[test]
fn export_png_onto_a_dxf_name_is_refused() {
    let d = dir("export");
    let (mut s, path) = saved(&d, "plan.dxf");
    let before = std::fs::read(&path).unwrap();
    let e = err(&mut s, "export", json!({ "path": path, "format": "png" }));
    assert!(e.contains(".dxf") && e.contains(".png"), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // Exporting the drawing's own format onto itself is refused too.
    let e = err(&mut s, "export", json!({ "path": path }));
    assert!(e.contains("open drawing"), "{e}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn redline_keeps_an_existing_pdf_beside_the_drawing() {
    let d = dir("redline");
    let (mut s, _) = saved(&d, "plan.dxf");
    let original = d.join("plan.pdf");
    std::fs::write(&original, b"%PDF-1.4 the original print").unwrap();
    let r = s.execute("redline", &json!({})).unwrap();
    assert_eq!(std::fs::read(&original).unwrap(), b"%PDF-1.4 the original print");
    assert!(r["path"].as_str().unwrap().ends_with("plan-redline.pdf"), "{r}");
    assert_eq!(r["keptExisting"], true);
    assert!(d.join("plan-redline.pdf").exists());
    // Again: -redline-2, never a replacement.
    let r = s.execute("redline", &json!({})).unwrap();
    assert!(r["path"].as_str().unwrap().ends_with("plan-redline-2.pdf"), "{r}");
    // An explicit existing path needs force.
    let e = err(&mut s, "redline", json!({ "path": original.to_string_lossy() }));
    assert!(e.contains("force"), "{e}");
    assert_eq!(std::fs::read(&original).unwrap(), b"%PDF-1.4 the original print");
    s.execute("redline", &json!({ "path": original.to_string_lossy(), "force": true })).unwrap();
    assert_ne!(std::fs::read(&original).unwrap(), b"%PDF-1.4 the original print");
}

#[test]
fn walk_publish_keeps_an_existing_package() {
    let d = dir("walk");
    let (mut s, _) = saved(&d, "plan.dxf");
    let original = d.join("plan.cdywalk");
    std::fs::write(&original, b"earlier package").unwrap();
    let r = s.execute("walk.publish", &json!({})).unwrap();
    assert_eq!(std::fs::read(&original).unwrap(), b"earlier package");
    assert!(r["path"].as_str().unwrap().ends_with("plan-walk.cdywalk"), "{r}");
    assert!(d.join("plan-walk.cdywalk").exists());
    let e = err(&mut s, "walk.publish", json!({ "path": original.to_string_lossy() }));
    assert!(e.contains("force"), "{e}");
    let e = err(&mut s, "walk.publish", json!({ "path": d.join("plan.dxf").to_string_lossy() }));
    assert!(e.contains(".cdywalk"), "{e}");
    s.execute("walk.publish", &json!({ "path": original.to_string_lossy(), "force": true })).unwrap();
    assert_ne!(std::fs::read(&original).unwrap(), b"earlier package");
}
