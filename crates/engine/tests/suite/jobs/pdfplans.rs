//! Plans from vector PDFs (written by our own PDF exporter, read back without layer names):
//! walls by double lines and line weight, doors by swing arcs, room names by the areas round
//! them, the scale from the scale note or from a dimension, the sheet from its title block,
//! a drafted spec; walls drawn only as filled poché in one pen; a page that is only a picture
//! is reported unsupported.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Space, entity_bounds};
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
    // The three wall rectangles (the outer one and the two rooms' insides), and nothing else: the
    // desk is a light pen and too wide for a wall. Seven pairs: the four outer sides with the
    // rooms' outer sides, and the partition's two faces.
    assert_eq!(r["walls"]["objects"], json!(3), "{r}");
    assert_eq!(r["walls"]["pairs"], json!(7), "{r}");
    assert_eq!(r["walls"]["lineweight_mm"], json!(0.7));
    // The swing and its leaf come back as one polyline (chords, then the leaf).
    assert_eq!(r["doors"], json!(1), "{r}");
    let mut rooms: Vec<&str> = r["rooms"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    rooms.sort_unstable();
    assert_eq!(rooms, vec!["LOBBY", "OFFICE"]);
    assert_eq!(r["sheet"]["number"], json!("A101"), "{r}");
    assert_eq!(r["sheet"]["title"], json!("FLOOR PLAN"), "{r}");
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
    // The desk (60" × 30") stays where it was; the swing (rebuilt as an arc) and its leaf are doors.
    let d = s.doc().unwrap();
    let desk: Vec<&str> = d
        .model
        .iter()
        .filter(|e| {
            let b = entity_bounds(d, e, 0);
            (b.width() - 60.0).abs() < 1.0 && (b.height() - 30.0).abs() < 1.0
        })
        .map(|e| e.common.layer.as_str())
        .collect();
    assert_eq!(desk.len(), 1, "{desk:?}");
    assert!(desk.iter().all(|l| *l != "A-WALL" && *l != "A-DOOR"), "{desk:?}");
    let on = |layer: &str| d.model.iter().filter(|e| e.common.layer == layer).map(|e| e.kind.clone()).collect::<Vec<_>>();
    let doors = on("A-DOOR");
    assert!(doors.iter().any(|k| matches!(k, EntityKind::Arc(a) if (a.radius - 36.0).abs() < 0.5)), "{doors:?}");
    assert!(doors.iter().any(|k| matches!(k, EntityKind::Line(l) if (l.a.xy().dist(l.b.xy()) - 36.0).abs() < 0.5)), "{doors:?}");
    assert_eq!(doors.len(), 2, "{doors:?}");
    assert_eq!(on("A-WALL").len(), 3);
    // It builds.
    let b = s.execute("model.build", &json!({})).unwrap();
    assert!(b["walls"].as_u64().unwrap() >= 4, "{b}");
}

/// The plan with its walls drawn only as poché, in the one pen the whole sheet uses (no double
/// lines, no heavier pen): the four outer walls as SOLID hatches (their boundaries erased) and
/// the partition as a 2D solid, all 6" thick; the swing, its leaf and the desk as before. The
/// title block has a field label and the scale note nearer the number than the title.
fn poche() -> Session {
    let mut s = Session::new();
    layer(&mut s, "PLAN", 25);
    for (a, b) in [([0.0, 0.0], [480.0, 6.0]), ([0.0, 234.0], [480.0, 240.0]), ([0.0, 6.0], [6.0, 234.0]), ([474.0, 6.0], [480.0, 234.0])] {
        let r = s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
        let h = r["handle"].as_str().unwrap().to_string();
        s.execute("hatch", &json!({"handles": [h], "pattern": "SOLID"})).unwrap();
        s.execute("erase", &json!({"handles": [h]})).unwrap();
    }
    s.execute("solid", &json!({"points": [[237, 6], [243, 6], [237, 234], [243, 234]]})).unwrap();
    s.execute("arc", &json!({"center": [60, 6], "radius": 36, "start": 0, "end": 90})).unwrap();
    s.execute("line", &json!({"points": [[60, 6], [60, 42]]})).unwrap();
    rect(&mut s, [150.0, 150.0], [210.0, 180.0]);
    text(&mut s, [90.0, 120.0], "OFFICE", 9.0);
    text(&mut s, [330.0, 120.0], "LOBBY", 9.0);
    text(&mut s, [380.0, -56.0], "SHEET", 4.0);
    text(&mut s, [380.0, -70.0], "A102", 12.0);
    text(&mut s, [380.0, -94.0], "FIRST FLOOR PLAN", 6.0);
    text(&mut s, [330.0, -70.0], "SCALE: 1/4\" = 1'-0\"", 6.0);
    s
}

#[test]
fn walls_drawn_only_as_poche_are_walls() {
    let mut s = through_pdf(&poche());
    let fills = s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Hatch(_) | EntityKind::Solid(_))).count();
    assert_eq!(fills, 5, "the five fills come back as fills");
    let r = s.execute("plan.classify", &json!({})).unwrap();
    assert!((r["scale"]["factor"].as_f64().unwrap() - 48.0).abs() < 1e-9, "{r}");
    // Exactly the five fills, by poché alone: no pairs, and one pen (no wall line weight).
    assert_eq!(r["walls"]["objects"], json!(5), "{r}");
    assert_eq!(r["walls"]["pairs"], json!(0), "{r}");
    assert_eq!(r["walls"]["lineweight_mm"], Value::Null, "{r}");
    assert_eq!(r["doors"], json!(1), "{r}");
    assert_eq!(r["sheet"]["number"], json!("A102"), "{r}");
    assert_eq!(r["sheet"]["title"], json!("FIRST FLOOR PLAN"), "{r}");
    let r = s.execute("plan.classify", &json!({"apply": true})).unwrap();
    assert_eq!(r["applied"], json!(true));
    let d = s.doc().unwrap();
    let walls: Vec<_> = d.model.iter().filter(|e| e.common.layer == "A-WALL").collect();
    assert_eq!(walls.len(), 5);
    assert!(walls.iter().all(|e| matches!(e.kind, EntityKind::Hatch(_) | EntityKind::Solid(_))));
    // At real size: each 6" thick, the four outer walls 480" or 228" long, the partition 228".
    let mut sizes: Vec<(i64, i64)> = walls
        .iter()
        .map(|e| {
            let b = entity_bounds(d, e, 0);
            let (w, h) = (b.width().round() as i64, b.height().round() as i64);
            (w.min(h), w.max(h))
        })
        .collect();
    sizes.sort_unstable();
    assert_eq!(sizes, vec![(6, 228), (6, 228), (6, 228), (6, 480), (6, 480)]);
    // Nothing else on A-WALL: the desk stays put, the swing and leaf are doors.
    assert!(d.model.iter().filter(|e| e.common.layer != "A-WALL").all(|e| !matches!(e.kind, EntityKind::Hatch(_) | EntityKind::Solid(_))));
    let desk = d.model.iter().find(|e| {
        let b = entity_bounds(d, e, 0);
        (b.width() - 60.0).abs() < 1.0 && (b.height() - 30.0).abs() < 1.0
    });
    assert!(desk.is_some_and(|e| e.common.layer != "A-WALL"));
    assert_eq!(d.model.iter().filter(|e| e.common.layer == "A-DOOR").count(), 2);
}

/// The plan with what a real sheet has in its rooms (Q251), in light pens: a 14" duct across the
/// office with its label between the duct's sides, an exhaust fan's mark, a stair in the lobby
/// (stringers, ten 11" treads, its note on the landing), room numbers by the names.
fn cluttered() -> Session {
    let mut s = plan(true);
    layer(&mut s, "DUCT", 18);
    s.execute("line", &json!({"points": [[6, 60], [234, 60]]})).unwrap();
    s.execute("line", &json!({"points": [[6, 74], [234, 74]]})).unwrap();
    text(&mut s, [60.0, 65.0], "SUPPLY AIR", 4.0);
    text(&mut s, [180.0, 200.0], "EF-1", 4.0);
    layer(&mut s, "STAIR", 18);
    rect(&mut s, [420.0, 60.0], [464.0, 230.0]);
    for i in 1..11 {
        let y = 60.0 + 11.0 * f64::from(i);
        s.execute("line", &json!({"points": [[420, y], [464, y]]})).unwrap();
    }
    // The note on the landing (44" × 60": a room's size when the treads bound it).
    text(&mut s, [424.0, 195.0], "STAIR", 3.0);
    layer(&mut s, "TAGS", 25);
    text(&mut s, [90.0, 40.0], "101", 6.0);
    text(&mut s, [380.0, 105.0], "102", 6.0);
    s
}

#[test]
fn ducts_stairs_and_marks_never_make_or_split_rooms() {
    let mut s = through_pdf(&cluttered());
    let r = s.execute("plan.classify", &json!({})).unwrap();
    let mut rooms: Vec<&str> = r["rooms"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
    rooms.sort_unstable();
    assert_eq!(rooms, vec!["LOBBY", "OFFICE"], "{r}");
    // Drafted: two rooms, numbered from the tags inside them, and the office whole (its number
    // is below the duct, its name above).
    let r = s.execute("plan.classify", &json!({"draft": true})).unwrap();
    let rooms = r["spec"]["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 2, "{}", r["spec"]);
    let office = rooms.iter().find(|x| x["name"] == "Office").unwrap();
    assert_eq!(office["number"], json!("101"), "{office}");
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
