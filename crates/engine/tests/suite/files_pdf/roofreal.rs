//! The roof on a real set (Q6006): a roof plan comes in as a level of its own, its edge,
//! parapet and curbs as loose lines with the sheet's border round them. `model.build` reads the
//! roof off it (never the border, never walls), stacks it over the storey under it however the
//! plan sits on its sheet, and the takeoff carries roof area, parapet lf, drains and rooftop
//! units. A roof plan with no outline gets a flat roof over the top floor's plate.

use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::cmd::bspec::made_level;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Entity, EntityKind};
use cadaddy_geom::Bounds2;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

/// A rectangle as four loose lines (as a PDF's edges come in).
fn loose(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    for (p, q) in [(a, [b[0], a[1]]), ([b[0], a[1]], b), (b, [a[0], b[1]]), ([a[0], b[1]], a)] {
        line(s, p, q);
    }
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).unwrap();
}

/// A 40' × 30' box of 6" walls with its lower-left outside corner at `at`.
fn walls(s: &mut Session, at: [f64; 2]) {
    layer(s, "A-WALL");
    rect(s, at, [at[0] + 480.0, at[1] + 360.0]);
    rect(s, [at[0] + 6.0, at[1] + 6.0], [at[0] + 474.0, at[1] + 354.0]);
}

/// The roof plan's sheet: [`ROOF_X`] over, bordered on the wall pen as a printed sheet's comes in.
const ROOF_X: f64 = 2800.0;

/// Two floors side by side, each 40' × 30' near its sheet's corner, and the roof plan's sheet
/// with its border and title. `roof` draws the roof plan itself; the levels are stacked.
fn building(roof: impl FnOnce(&mut Session)) -> Session {
    let mut s = Session::new();
    walls(&mut s, [100.0, 100.0]);
    walls(&mut s, [1500.0, 100.0]);
    layer(&mut s, "A-WALL");
    rect(&mut s, [ROOF_X + 10.0, 10.0], [ROOF_X + 1190.0, 890.0]);
    line(&mut s, [ROOF_X + 10.0, 80.0], [ROOF_X + 1190.0, 80.0]);
    layer(&mut s, "A-ANNO");
    text(&mut s, [ROOF_X + 900.0, 40.0], "ROOF PLAN");
    roof(&mut s);
    let region = |x: f64| json!([x, 0, x + 1200.0, 900]);
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "version": 1, "units_per_foot": 12, "stack": true,
            "levels": [
                {"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "region": region(0.0)},
                {"name": "Level 2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "region": region(1400.0)},
                {"name": "Roof", "elevation_ft": 24, "height_ft": 4, "ceiling_ft": 4, "region": region(ROOF_X)}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s
}

/// The roof edge's lower-left corner on its sheet: placed differently on its sheet than the
/// floors are on theirs, 6" past the walls all round (41' × 31').
const EDGE: [f64; 2] = [ROOF_X + 394.0, 294.0];

/// A roof plan as it prints: the edge and an 8" parapet ring as loose lines, grid lines across
/// it, a curb on a layer of its own named by the notes under it, drains with their `RD` notes,
/// slope arrows to the drains both ways (tapered insulation) and a legend off the roof.
fn roof_plan(s: &mut Session) {
    let [x, y] = EDGE;
    layer(s, "A-WALL");
    loose(s, [x, y], [x + 492.0, y + 372.0]);
    loose(s, [x + 8.0, y + 8.0], [x + 484.0, y + 364.0]);
    layer(s, "A-GRID");
    line(s, [x + 150.0, y - 80.0], [x + 150.0, y + 450.0]);
    line(s, [x - 80.0, y + 186.0], [x + 570.0, y + 186.0]);
    layer(s, "A-ROOF-CURB");
    rect(s, [x + 300.0, y + 250.0], [x + 348.0, y + 286.0]);
    layer(s, "A-ROOF");
    text(s, [x + 305.0, y + 238.0], "RTU-1");
    text(s, [x + 305.0, y + 230.0], "ON 14\" CURB");
    text(s, [x + 30.0, y + 340.0], "PARAPET, 42\" ABOVE ROOF");
    for (cx, cy) in [(x + 100.0, y + 120.0), (x + 400.0, y + 120.0)] {
        s.execute("circle", &json!({"center": [cx, cy], "radius": 9})).unwrap();
        text(s, [cx + 14.0, cy - 14.0], "RD");
    }
    for (tail, head) in [([x + 100.0, y + 330.0], [x + 100.0, y + 140.0]), ([x + 400.0, y + 20.0], [x + 400.0, y + 100.0])] {
        line(s, tail, head);
        let back = if head[1] < tail[1] { 8.0 } else { -8.0 };
        line(s, head, [head[0] - 5.0, head[1] + back]);
        line(s, head, [head[0] + 5.0, head[1] + back]);
        text(s, [tail[0] + 8.0, (tail[1] + head[1]) / 2.0], "SLOPE 1/8\"/FT");
    }
    text(s, [ROOF_X + 40.0, 120.0], "RD = ROOF DRAIN");
}

fn made<'a>(s: &'a Session, part: &str, level: &str) -> Vec<&'a Entity> {
    s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some(part) && made_level(e).as_deref() == Some(level)).map(|e| &**e).collect()
}

fn bounds(s: &Session, es: &[&Entity]) -> Bounds2 {
    let d = s.doc().unwrap();
    es.iter().fold(Bounds2::EMPTY, |b, e| b.union(&cadaddy_engine::doc::entity_bounds(d, e, 0)))
}

fn z_range(es: &[&Entity]) -> (f64, f64) {
    let mut z = (f64::INFINITY, f64::NEG_INFINITY);
    for e in es {
        if let EntityKind::Mesh(m) = &e.kind {
            for v in &m.vertices {
                z = (z.0.min(v.z), z.1.max(v.z));
            }
        }
    }
    z
}

fn near(a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
    (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol
}

/// The model takeoff's roof lines: (item, size, quantity).
fn roof_lines(s: &mut Session) -> Vec<(String, String, f64)> {
    let t = s.execute("bim.bom", &json!({})).unwrap();
    t["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["category"] == "roofs")
        .map(|l| (l["item"].as_str().unwrap_or("").to_string(), l["size"].as_str().unwrap_or("").to_string(), l["net"].as_f64().unwrap_or(0.0)))
        .collect()
}

fn qty(lines: &[(String, String, f64)], item: &str) -> f64 {
    lines.iter().filter(|l| l.0 == item).map(|l| l.2).sum()
}

#[test]
fn a_roof_plan_off_its_sheets_corner_is_read_and_stacked_over_the_walls() {
    let mut s = building(roof_plan);
    let r = s.execute("model.build", &json!({})).unwrap();
    // The roof plan's lines are no walls, its title block no room, and it has no floor.
    for part in ["wall", "floor", "ceiling", "door"] {
        assert!(made(&s, part, "Roof").is_empty(), "{part} on the roof: {r}");
    }
    assert_eq!(made(&s, "wall", "Level 2").len(), made(&s, "wall", "Level 1").len(), "{r}");
    assert_eq!([&r["roofs"], &r["parapets"], &r["equipment"], &r["roofDrains"]].map(|v| v.as_u64().unwrap_or(0)), [1, 1, 1, 2], "{r}");
    let notes = r["notes"].to_string();
    assert!(notes.contains("Roof: roof read from its plan: 1271 sq ft, parapet 3'-6"), "{notes}");
    // Stacked: the edge 6" past Level 1's walls all round (the sheet border is not the roof).
    let roof = made(&s, "roof", "Roof");
    let b = bounds(&s, &roof);
    assert!(near([b.min.x, b.min.y], [94.0, 94.0], 0.5) && near([b.max.x, b.max.y], [586.0, 466.0], 0.5), "roof at {b:?}");
    // Flat (the arrows run both ways to the drains), its top on the roof level.
    let (lo, hi) = z_range(&roof);
    assert!((hi - 288.0).abs() < 1e-6 && (lo - 276.0).abs() < 1e-6, "roof {lo}..{hi}");
    let (_, top) = z_range(&made(&s, "parapet", "Roof"));
    assert!((top - (288.0 + 42.0)).abs() < 1e-6, "parapet top {top}");
    // The unit on its curb's outline at the roof, the drains where their circles are.
    let unit = made(&s, "equipment", "Roof");
    let (u0, u1) = (bounds(&s, &unit), z_range(&unit));
    assert!(near([u0.min.x, u0.min.y], [394.0, 344.0], 0.5) && (u1.0 - 288.0).abs() < 1e-6 && (u1.1 - 336.0).abs() < 1e-6, "{u0:?} {u1:?}");
    let drains = made(&s, "roof-drain", "Roof");
    let centres: Vec<[f64; 2]> = drains.iter().map(|e| bounds(&s, &[*e]).center()).map(|c| [c.x, c.y]).collect();
    assert!(centres.iter().any(|c| near(*c, [194.0, 214.0], 0.5)) && centres.iter().any(|c| near(*c, [494.0, 214.0], 0.5)), "{centres:?}");
    // In the takeoff: area, parapet lf, drains and the unit.
    let lines = roof_lines(&mut s);
    assert!((qty(&lines, "Roof area") - 1271.0).abs() < 0.01, "{lines:?}");
    assert!((qty(&lines, "Parapet") - 144.0).abs() < 0.01, "{lines:?}");
    assert_eq!(qty(&lines, "Roof drain"), 2.0, "{lines:?}");
    assert!(lines.iter().any(|l| l.0 == "Rooftop equipment" && l.1 == "RTU-1" && l.2 == 1.0), "{lines:?}");
}

#[test]
fn roof_read_takes_the_roof_off_loose_lines_not_the_sheet_border() {
    let mut s = building(roof_plan);
    let r = s.execute("roof.read", &json!({"region": [ROOF_X, 0, ROOF_X + 1200.0, 900], "level": "Roof", "apply": true})).unwrap();
    let outline: Vec<[f64; 2]> = serde_json::from_value(r["roofs"][0]["outline"].clone()).unwrap();
    let b = Bounds2::from_points(outline.iter().map(|p| cadaddy_geom::Vec2::new(p[0], p[1])));
    assert!(near([b.min.x, b.min.y], EDGE, 1e-6) && near([b.max.x, b.max.y], [EDGE[0] + 492.0, EDGE[1] + 372.0], 1e-6), "{r}");
    assert_eq!(r["roofs"][0]["parapet_ft"], json!(3.5), "{r}");
    assert_eq!(r["roofs"][0]["slope"], Value::Null, "two ways to the drains: flat, {r}");
    assert_eq!(r["drains"].as_array().map(Vec::len), Some(2), "{r}");
    assert_eq!(r["equipment"][0]["name"], json!("RTU-1"), "{r}");
    // Applied, the build takes the spec's roof as it is.
    let b = s.execute("model.build", &json!({})).unwrap();
    assert_eq!([&b["roofs"], &b["roofDrains"]].map(|v| v.as_u64().unwrap_or(0)), [1, 2], "{b}");
    assert!(!b["notes"].to_string().contains("read from its plan"), "{b}");
}

#[test]
fn a_roof_plan_with_no_outline_gets_a_flat_roof_over_the_top_floor() {
    // Only the sheet's border and title: the roof is the top floor's outline, over it.
    let mut s = building(|_| {});
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["roofs"], json!(1), "{r}");
    assert!(r["notes"].to_string().contains("Roof: no roof outline read on its plan; a flat roof over Level 2's floor plate."), "{r}");
    let roof = made(&s, "roof", "Roof");
    let b = bounds(&s, &roof);
    assert!(near([b.min.x, b.min.y], [100.0, 100.0], 1e-6) && near([b.max.x, b.max.y], [580.0, 460.0], 1e-6), "{b:?}");
    assert!((z_range(&roof).1 - 288.0).abs() < 1e-6);
    let lines = roof_lines(&mut s);
    assert!((qty(&lines, "Roof area") - 1200.0).abs() < 0.01, "{lines:?}");
}

// ---- the guided job on a made-up print set ----

fn real_pdf_reader() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

#[test]
fn the_job_builds_the_roof_from_the_printed_roof_plan() {
    let _g = serial();
    real_pdf_reader();
    // A made-up office's two floor plans and its roof plan, printed and merged into one PDF.
    let mut set = cadaddy_mepfix::generate(3).expect("set");
    set.truth.sheets.retain(|s| ["A-101", "A-102", "A-103"].contains(&s.number.as_str()));
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_roofreal_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pages: Vec<(Vec<u8>, String, String)> =
        set.truth.sheets.iter().map(|sh| (set.file(&sh.pdf).expect("pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new())).collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).unwrap()).unwrap();
    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false})).unwrap();
    assert_eq!(st["done"], json!(true), "{st}");
    let _ = std::fs::remove_dir_all(&dir);
    // The job's own takeoff stage has the roof.
    let job: Value = serde_json::from_str(s.doc().ok().and_then(|d| d.agent.clone()).as_deref().unwrap_or("{}")).unwrap();
    let takeoff =
        job["guided_job"]["stages"].as_array().and_then(|a| a.iter().find(|x| x["name"] == "takeoff")).map(|x| x["result"]["csv"].to_string());
    assert!(takeoff.as_deref().is_some_and(|c| c.contains("Roof,architecture,roofs,roofs,Roof area")), "{takeoff:?}");
    let roof = &set.truth.arch.as_ref().expect("the set's architecture").roof;
    // No walls or floor slab from the roof plan's lines; the roof over the walls.
    assert!(made(&s, "wall", "Roof").is_empty() && made(&s, "floor", "Roof").is_empty());
    let walls = bounds(&s, &made(&s, "wall", "Level 1"));
    let spec = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    let roof_ft = spec["levels"].as_array().unwrap().iter().find(|l| l["name"] == "Roof").and_then(|l| l["elevation_ft"].as_f64()).unwrap();
    let built = made(&s, "roof", "Roof");
    let b = bounds(&s, &built);
    assert!(near([b.center().x, b.center().y], [walls.center().x, walls.center().y], 6.0), "roof {b:?} walls {walls:?}");
    assert!((z_range(&built).1 - roof_ft * 12.0).abs() < 1e-6);
    // The takeoff: the roof's area as printed, its parapet, every drain and curb.
    let lines = roof_lines(&mut s);
    let printed = (roof.max_ft[0] - roof.min_ft[0]) * (roof.max_ft[1] - roof.min_ft[1]);
    assert!((qty(&lines, "Roof area") - printed).abs() <= 0.02 * printed, "{printed} sq ft printed: {lines:?}");
    assert!(qty(&lines, "Parapet") > 0.0 && roof.parapet_in.is_some(), "{lines:?}");
    assert_eq!(qty(&lines, "Roof drain") as usize, roof.drains.len(), "{lines:?}");
    assert_eq!(qty(&lines, "Rooftop equipment") as usize, roof.curbs.len(), "{lines:?}");
    for c in &roof.curbs {
        let name = c.tag.split_whitespace().next().unwrap_or("");
        assert!(lines.iter().any(|l| l.0 == "Rooftop equipment" && l.1.contains(name)), "{} missing: {lines:?}", c.tag);
    }
}
