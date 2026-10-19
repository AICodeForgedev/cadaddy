//! Plans from vector PDFs (written by our own PDF exporter, read back without layer names):
//! walls by double lines and line weight, doors by swing arcs, room names by the areas round
//! them, the scale from the scale note or from a dimension, the sheet from its title block,
//! a drafted spec; a page that is only a picture is reported unsupported.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Space;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str, lw: u16) {
    s.execute("layer.new", &json!({"name": name, "current": true, "lineweight": f64::from(lw) / 100.0})).unwrap();
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": at, "text": t, "height": h})).unwrap();
}

/// A 40' × 20' plan in inches: two rooms in 6" walls (heavy pen), a door swing and its leaf, a
/// desk (light pen), room names, a title block; a scale note, or a dimension instead.
fn plan(note: bool) -> Session {
    let mut s = Session::new();
    layer(&mut s, "WALLS", 70);
    rect(&mut s, [0.0, 0.0], [480.0, 240.0]);
    rect(&mut s, [6.0, 6.0], [234.0, 234.0]);
    rect(&mut s, [246.0, 6.0], [474.0, 234.0]);
    layer(&mut s, "THIN", 18);
    s.execute("arc", &json!({"center": [60, 6], "radius": 36, "start": 0, "end": 90})).unwrap();
    s.execute("line", &json!({"points": [[60, 6], [60, 42]]})).unwrap();
    rect(&mut s, [150.0, 150.0], [210.0, 180.0]);
    layer(&mut s, "TEXT", 25);
    text(&mut s, [90.0, 120.0], "OFFICE", 9.0);
    text(&mut s, [330.0, 120.0], "LOBBY", 9.0);
    text(&mut s, [380.0, -70.0], "A101", 12.0);
    text(&mut s, [380.0, -90.0], "FLOOR PLAN", 6.0);
    if note {
        text(&mut s, [0.0, -50.0], "SCALE: 1/4\" = 1'-0\"", 6.0);
    } else {
        s.execute("line", &json!({"points": [[0, 280], [480, 280]]})).unwrap();
        text(&mut s, [220.0, 284.0], "40'-0\"", 6.0);
    }
    s
}

/// The plan through PDF: plotted at 1/4" = 1'-0" without PDF layers, read back at paper size
/// with everything on PDF_Geometry / PDF_Text.
fn through_pdf(s: &Session) -> Session {
    let opts =
        json!({"layers": false, "fit": false, "center": true, "scale": 0.25 / 12.0, "paper": "ANSI B", "landscape": true, "lineweights": true});
    let bytes = cadaddy_io::pdf::plot(s.doc().unwrap(), &Space::Model, &opts).unwrap();
    let (d, _) = cadaddy_io::pdf_read::read_json(&bytes, &json!({"scale": 1, "layers": "object", "layout": false})).unwrap();
    assert!(d.layers.iter().all(|l| !["WALLS", "THIN", "TEXT"].contains(&l.name.as_str())), "no layer names came through");
    let mut out = Session::empty();
    out.open_drawing(d, "Imported", None);
    out
}

#[test]
fn a_pdf_plan_is_read_by_what_it_draws() {
    let mut s = through_pdf(&plan(true));
    let r = s.execute("plan.classify", &json!({})).unwrap();
    assert_eq!(r["raster_only"], json!(false));
    let f = r["scale"]["factor"].as_f64().unwrap();
    assert!((f - 48.0).abs() < 1e-9, "{r}");
    assert!(r["scale"]["source"].as_str().unwrap().starts_with("scale note"));
    assert!(r["walls"]["objects"].as_u64().unwrap() >= 3, "{r}");
    assert!(r["walls"]["pairs"].as_u64().unwrap() >= 4, "{r}");
    assert_eq!(r["walls"]["lineweight_mm"], json!(0.7));
    // The swing and its leaf (read back as one polyline, or as two pieces).
    assert!((1..=2).contains(&r["doors"].as_u64().unwrap()), "{r}");
    let mut rooms: Vec<&str> = r["rooms"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    rooms.sort_unstable();
    assert_eq!(rooms, vec!["LOBBY", "OFFICE"]);
    assert_eq!(r["sheet"]["number"], json!("A101"), "{r}");
    // Applied and drafted: real inches, NCS layers, a spec with the walls and rooms.
    let r = s.execute("plan.classify", &json!({"draft": true})).unwrap();
    let spec = &r["spec"];
    assert!(spec["layers"].as_array().unwrap().iter().any(|l| l["layer"] == "A-WALL" && l["role"] == "wall"), "{spec}");
    let t = spec["wall_types"].as_array().unwrap().iter().find(|t| t["from"] == "pairs").unwrap();
    assert!((t["thickness_in"].as_f64().unwrap() - 6.0).abs() < 0.13, "{t}");
    let names: Vec<&str> = spec["rooms"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Office") && names.contains(&"Lobby"), "{names:?}");
    let ext = s.doc().unwrap().extents(&Space::Model);
    assert!((ext.width() - 480.0).abs() < 2.0, "back at real size: {}", ext.width());
    // It builds.
    let b = s.execute("model.build", &json!({})).unwrap();
    assert!(b["walls"].as_u64().unwrap() >= 4, "{b}");
}

#[test]
fn the_scale_comes_from_a_dimension_without_a_note() {
    let mut s = through_pdf(&plan(false));
    let r = s.execute("plan.classify", &json!({})).unwrap();
    let f = r["scale"]["factor"].as_f64().unwrap();
    assert!((f - 48.0).abs() < 0.05, "{r}");
    assert!(r["scale"]["source"].as_str().unwrap().contains("dimension"), "{r}");
}

#[test]
fn a_picture_page_is_unsupported() {
    let dir = std::env::temp_dir().join(format!("cadaddy-pdfplans-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("scan.png");
    let raster = cadaddy_engine::render::images::Raster { width: 4, height: 4, rgba: vec![255; 64] };
    std::fs::write(&png, raster.encode_png().unwrap()).unwrap();
    let mut s = Session::new();
    s.execute("imageattach", &json!({"path": png.to_string_lossy(), "at": [0, 0], "scale": 11})).unwrap();
    let r: Value = s.execute("plan.classify", &json!({"apply": true})).unwrap();
    assert_eq!(r["raster_only"], json!(true), "{r}");
    assert_eq!(r["applied"], json!(false));
    let _ = std::fs::remove_dir_all(&dir);
}
