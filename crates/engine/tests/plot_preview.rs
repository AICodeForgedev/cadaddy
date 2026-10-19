//! PREVIEW is the plot itself as an image (plot styles applied), and PUBLISH takes a sheet list —
//! sheets from other drawings, page setup overrides per sheet — into one PDF with a bookmark per
//! sheet; publish lists are saved and loaded.

use std::path::{Path, PathBuf};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, base64_decode, set_io, set_pdf_merge};
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Layout, Line, Space};
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: None,
    });
    set_pdf_merge(|parts| cadaddy_io::pdf_merge::merge(parts).map_err(|e| e.to_string()));
}

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-plotpreview-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A session whose Layout1 has a thick red line.
fn session() -> Session {
    hooks();
    let mut s = Session::new();
    s.execute("layout.set", &json!({ "name": "Layout1" })).unwrap();
    let h = s.execute("line", &json!({ "points": [[1, 1], [8, 6]] })).unwrap()["handles"][0].clone();
    s.execute("properties.set", &json!({ "handles": [h], "color": 1, "lineweight": 1.0 })).unwrap();
    s
}

fn data(r: &Value) -> Vec<u8> {
    base64_decode(r["data"].as_str().unwrap()).unwrap()
}

#[test]
fn the_preview_is_the_plot_raster() {
    let mut s = session();
    let r = s.execute("preview", &json!({ "layout": "Layout1", "width": 640 })).unwrap();
    let preview = data(&r);
    assert!(preview.starts_with(b"\x89PNG"));
    assert_eq!(r["width"], 640, "{r}");
    let dpi = r["resolution"].as_f64().unwrap();
    let plot = s.execute("plot", &json!({ "layout": "Layout1", "output": "png", "resolution": dpi })).unwrap();
    assert_eq!(preview, data(&plot), "pixel for pixel what PLOT makes");
    // Plot style colours show: with monochrome.ctb the red line plots black.
    s.execute("pagesetup", &json!({ "layout": "Layout1", "plotStyleTable": "monochrome.ctb" })).unwrap();
    let mono = data(&s.execute("preview", &json!({ "layout": "Layout1", "width": 640 })).unwrap());
    assert_ne!(mono, preview);
    let plot = s.execute("plot", &json!({ "layout": "Layout1", "output": "png", "resolution": dpi })).unwrap();
    assert_eq!(mono, data(&plot));
}

fn other_drawing(path: &Path) {
    let mut d = Drawing::new_metric();
    d.layouts = vec![Layout::new("Plan", 1), Layout::new("Section", 2)];
    for n in ["Plan", "Section"] {
        let h = d.new_handle();
        d.space_mut(&Space::Paper(n.into()))
            .unwrap()
            .push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(10.0, 10.0, 0.0), b: Vec3::new(90.0, 50.0, 0.0) })));
    }
    std::fs::write(path, cadaddy_io::write(&d, &path.to_string_lossy()).unwrap()).unwrap();
}

#[test]
fn a_sheet_list_from_two_drawings_publishes_to_one_pdf() {
    let mut s = session();
    let dir = folder("publish");
    let other = dir.join("site.dxf");
    other_drawing(&other);
    // The other drawing's sheets, as the dialog lists them.
    let r = s.execute("publishlist", &json!({ "action": "layouts", "drawing": other.to_string_lossy() })).unwrap();
    let theirs = r["sheets"].as_array().unwrap().clone();
    assert_eq!(theirs.iter().map(|x| x["layout"].as_str().unwrap()).collect::<Vec<_>>(), ["Plan", "Section"]);
    let mut sheets = vec![json!({ "layout": "Layout1", "title": "A-1 Cover", "page": { "paper": "ISO A3", "landscape": true } })];
    sheets.extend(theirs.into_iter().rev());
    // Saved and loaded again as a list.
    let list = dir.join("job.publish.json");
    s.execute("publishlist", &json!({ "action": "save", "path": list.to_string_lossy(), "sheets": sheets })).unwrap();
    let back = s.execute("publishlist", &json!({ "action": "load", "path": list.to_string_lossy() })).unwrap();
    assert_eq!(back["sheets"].as_array().unwrap().len(), 3);
    assert_eq!(back["sheets"][0]["page"]["paper"], "ISO A3");

    let out = dir.join("job.pdf");
    let r = s.execute("publish", &json!({ "list": list.to_string_lossy(), "single": true, "path": out.to_string_lossy() })).unwrap();
    assert_eq!(r["pages"], 3);
    let marks = cadaddy_io::pdf_merge::bookmarks(&std::fs::read(&out).unwrap()).unwrap();
    let titles: Vec<(String, usize)> = marks.into_iter().map(|(t, _, p)| (t, p)).collect();
    assert_eq!(titles, vec![("A-1 Cover".to_string(), 1), ("site - Section".to_string(), 2), ("site - Plan".to_string(), 3)]);

    // Without `single`, one PDF per sheet, as before; `single` alone publishes this drawing's sheets.
    let r = s.execute("publish", &json!({ "single": true })).unwrap();
    assert!(r["data"].is_string() && r["pages"].as_u64() >= Some(2), "{r}");
    for p in [
        json!({ "sheets": [{ "layout": "Nope" }], "single": true }),
        json!({ "sheets": [{ "drawing": dir.join("missing.dxf").to_string_lossy(), "layout": "Plan" }] }),
        json!({ "sheets": [{ "drawing": other.to_string_lossy(), "layout": "Nope" }] }),
        json!({ "sheets": [{}] }),
        json!({ "sheets": "x" }),
        json!({ "list": dir.join("missing.json").to_string_lossy() }),
    ] {
        assert!(s.execute("publish", &p).is_err(), "{p}");
    }
    assert!(s.execute("publishlist", &json!({ "action": "load", "path": other.to_string_lossy() })).is_err(), "a drawing isn't a list");
}
