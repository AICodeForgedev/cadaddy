//! Every built object belongs to a level (Q5295): a stacked build's storeys stand once, one over
//! the other, in the BIM view and the IFC export (never moved a second time by the plans' base
//! points); the default render frames the building, not the sheets around it; a room ceiling is
//! never traced round a grid bubble, a title block or the sheet's border; what a reader built off
//! the building is listed with where it came from; and what lies on the set's other sheets is
//! that sheet's, not "outside every level's region".

use std::sync::{Mutex, MutexGuard};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Entity, entity_bounds};
use cadaddy_engine::geom::Bounds2;
use serde_json::{Value, json};

/// The plans' spacing on the sheet (inches).
const STEP: f64 = 1200.0;

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, x0: f64, y0: f64, x1: f64, y1: f64) {
    line(s, [x0, y0], [x1, y0]);
    line(s, [x1, y0], [x1, y1]);
    line(s, [x1, y1], [x0, y1]);
    line(s, [x0, y1], [x0, y0]);
}

/// A plan's sheet rectangle.
fn region(i: usize) -> [f64; 4] {
    let ox = i as f64 * STEP;
    [ox - 200.0, -200.0, ox + 800.0, 600.0]
}

/// `plans` 40' × 30' floors side by side, each with a partition, a door and two named rooms;
/// below each, a grid bubble ("A" in a 2' circle) and a 10' × 5' box holding a sheet note
/// (neither is a room).
fn sheet(plans: usize) -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    for i in 0..plans {
        let ox = i as f64 * STEP;
        rect(&mut s, ox, 0.0, ox + 480.0, 360.0);
        rect(&mut s, ox + 6.0, 6.0, ox + 474.0, 354.0);
        for x in [238.0, 242.0] {
            line(&mut s, [ox + x, 6.0], [ox + x, 150.0]);
            line(&mut s, [ox + x, 186.0], [ox + x, 354.0]);
        }
    }
    s.execute("layer.new", &json!({"name": "A-DOOR", "current": true})).unwrap();
    for i in 0..plans {
        let ox = i as f64 * STEP;
        s.execute("arc", &json!({"center": [ox + 240.0, 150.0], "radius": 36.0, "start": 0.0, "end": 90.0})).unwrap();
    }
    s.execute("layer.new", &json!({"name": "A-AREA-IDEN", "current": true})).unwrap();
    for i in 0..plans {
        let ox = i as f64 * STEP;
        let level = i + 1;
        s.execute("text", &json!({"at": [ox + 100.0, 180.0], "text": "OFFICE", "height": 8.0})).unwrap();
        s.execute("text", &json!({"at": [ox + 100.0, 160.0], "text": format!("{level}01"), "height": 6.0})).unwrap();
        s.execute("text", &json!({"at": [ox + 340.0, 180.0], "text": "LOBBY", "height": 8.0})).unwrap();
        s.execute("text", &json!({"at": [ox + 340.0, 160.0], "text": format!("{level}02"), "height": 6.0})).unwrap();
        // The grid bubble's letter and the note, under the building.
        s.execute("text", &json!({"at": [ox + 116.0, -104.0], "text": "A", "height": 8.0})).unwrap();
        s.execute("text", &json!({"at": [ox + 210.0, -124.0], "text": "SHEET NOTES", "height": 8.0})).unwrap();
    }
    // Their circle and box on the wall pen, as an imported print puts them.
    s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    for i in 0..plans {
        let ox = i as f64 * STEP;
        s.execute("circle", &json!({"center": [ox + 120.0, -100.0], "radius": 12.0})).unwrap();
        rect(&mut s, ox + 200.0, -150.0, ox + 320.0, -90.0);
    }
    s
}

fn levels(plans: usize) -> Value {
    json!((0..plans)
        .map(|i| json!({"name": format!("Level {}", i + 1), "elevation_ft": 10.0 * i as f64, "height_ft": 10.0, "ceiling_ft": 8.0, "region": region(i)}))
        .collect::<Vec<_>>())
}

fn build(s: &mut Session, plans: usize, stack: bool) -> Value {
    s.execute("bspec.draft", &json!({"store": true, "levels": levels(plans), "stack": stack})).unwrap();
    s.execute("model.build", &json!({})).unwrap()
}

/// A built object's level (its WALLGEN tag after the part).
fn level_of(e: &Entity) -> Option<String> {
    made_as(e)?;
    e.common.xdata.iter().filter(|t| t.code == 1000).nth(1).map(|t| t.value.clone())
}

/// The plan extents of `d`'s built `part`s on `level`.
fn extents(d: &cadaddy_engine::doc::Drawing, part: &str, level: &str) -> Bounds2 {
    let mut b = Bounds2::EMPTY;
    for e in d.model.iter().filter(|e| made_as(e).as_deref() == Some(part) && level_of(e).as_deref() == Some(level)) {
        b = b.union(&entity_bounds(d, e, 0));
    }
    b
}

fn same(a: &Bounds2, b: &Bounds2, tol: f64) -> bool {
    !a.is_empty() && !b.is_empty() && a.min.dist(b.min) < tol && a.max.dist(b.max) < tol
}

/// The BIM view's whole-building composition (storeys stacked).
fn stacked_view(s: &Session) -> cadaddy_engine::doc::Drawing {
    let d = s.doc().unwrap();
    let look = cadaddy_engine::cmd::bimlook::Look::of(d);
    cadaddy_engine::cmd::bimlook::compose(d, &look, false, true, &|_, _| true)
}

#[test]
fn a_stacked_build_stands_once_in_the_bim_view() {
    let mut s = sheet(2);
    let r = build(&mut s, 2, true);
    assert_eq!(r["stacked"], json!(1), "{r}");
    let d = s.doc().unwrap();
    let (w1, w2) = (extents(d, "wall", "Level 1"), extents(d, "wall", "Level 2"));
    assert!(same(&w1, &w2, 1e-6), "the build stacked Level 2 over Level 1: {w1:?} {w2:?}");
    // The view takes the build's move into account: Level 2 stays over Level 1 (it used to move
    // again by the plans' spacing, a whole sheet away).
    let v = stacked_view(&s);
    let (v1, v2) = (extents(&v, "wall", "Level 1"), extents(&v, "wall", "Level 2"));
    assert!(same(&v1, &w1, 1e-6) && same(&v2, &w1, 1e-6), "stacked once: {v1:?} {v2:?} (built {w1:?})");
    // Each object the build moved says by how much.
    let moved = d.model.iter().filter(|e| level_of(e).as_deref() == Some("Level 2")).filter(|e| tag(e, "stacked:").is_some()).count();
    assert!(moved > 0);
    assert!(d.model.iter().filter(|e| level_of(e).as_deref() == Some("Level 1")).all(|e| tag(e, "stacked:").is_none()));

    // Built over their own plans (no stacking), the view still stacks them, as before.
    let mut s = sheet(2);
    build(&mut s, 2, false);
    let d = s.doc().unwrap();
    let (w1, w2) = (extents(d, "wall", "Level 1"), extents(d, "wall", "Level 2"));
    assert!((w2.min.x - w1.min.x - STEP).abs() < 1e-6, "{w1:?} {w2:?}");
    let v = stacked_view(&s);
    let (v1, v2) = (extents(&v, "wall", "Level 1"), extents(&v, "wall", "Level 2"));
    assert!(same(&v1, &v2, 1e-6), "{v1:?} {v2:?}");
}

fn tag<'a>(e: &'a Entity, key: &str) -> Option<&'a str> {
    e.common.xdata.iter().filter(|t| t.code == 1000).find_map(|t| t.value.strip_prefix(key))
}

#[test]
fn a_stacked_build_exports_its_storeys_over_each_other() {
    // The IFC walls of a stacked build and of one built over its plans are the same building:
    // one 40' × 30' footprint (12.192 m × 9.144 m), two storeys high.
    for stack in [true, false] {
        let mut s = sheet(2);
        build(&mut s, 2, stack);
        let bytes = cadaddy_io::ifc::write_ifc(s.doc().unwrap(), "Sample Building.ifc").unwrap();
        let back = cadaddy_io::ifc::read_ifc(&bytes).unwrap();
        let mut plan = Bounds2::EMPTY;
        let (mut z0, mut z1) = (f64::INFINITY, f64::NEG_INFINITY);
        for e in back.model.iter().filter(|e| cadaddy_io::ifc::element_info(e).class == "IfcWall") {
            plan = plan.union(&entity_bounds(&back, e, 0));
            if let cadaddy_engine::doc::EntityKind::Mesh(m) = &e.kind {
                for v in &m.vertices {
                    z0 = z0.min(v.z);
                    z1 = z1.max(v.z);
                }
            }
        }
        assert!((plan.width() - 12.192).abs() < 0.05 && (plan.height() - 9.144).abs() < 0.05, "stack {stack}: one footprint, {plan:?}");
        assert!(z1 - z0 > 6.0, "stack {stack}: two storeys, {z0}..{z1}");
    }
}

#[test]
fn room_ceilings_stay_inside_the_walls() {
    let mut s = sheet(2);
    build(&mut s, 2, true);
    let d = s.doc().unwrap();
    for level in ["Level 1", "Level 2"] {
        let walls = extents(d, "wall", level);
        let mut rooms = 0;
        for e in d.model.iter().filter(|e| made_as(e).as_deref() == Some("ceiling") && level_of(e).as_deref() == Some(level)) {
            let b = entity_bounds(d, e, 0);
            let w = walls.expand(12.0);
            assert!(
                b.min.x >= w.min.x && b.min.y >= w.min.y && b.max.x <= w.max.x && b.max.y <= w.max.y,
                "{level}: a ceiling {b:?} outside the walls {walls:?} ({:?})",
                tag(e, "room:")
            );
            rooms += usize::from(tag(e, "room:").is_some());
        }
        // The office and the lobby (one outline: the door's gap joins them), no bubble's or note
        // box's.
        assert_eq!(rooms, 1, "{level}");
    }
    // Nothing the build made stands off the building.
    let issues = s.execute("model.issues", &json!({"kind": "outside_building"})).unwrap();
    assert_eq!(issues["total"], json!(0), "{issues}");
}

#[test]
fn what_a_reader_built_off_the_building_is_listed_with_its_source() {
    let mut s = sheet(1);
    build(&mut s, 1, false);
    // A legend's line read as equipment from a mechanical sheet, 60' to the right of the
    // building; and a pump inside it.
    let box_at = |x: f64, y: f64, tag: &str| {
        json!({"tag": tag, "kind": "ahu", "level": "Level 1", "min": [x, y], "max": [x + 40.0, y + 40.0], "bottom_ft": 0.0, "height_ft": 4.0,
               "properties": {"SOURCE": "sheet=M-101"}})
    };
    let equipment = json!([box_at(480.0 + 720.0, 160.0, "AHU = AIR HANDLING UNIT"), box_at(300.0, 200.0, "P-1")]);
    s.execute("bspec.set", &json!({"spec": {"mep": {"equipment": equipment}}})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    let made = s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("mep")).count();
    assert!(made >= 2, "both boxes built: {r}");
    let issues = s.execute("model.issues", &json!({"kind": "outside_building"})).unwrap();
    let list = issues["issues"].as_array().cloned().unwrap_or_default();
    assert_eq!(list.len(), 1, "only the stray equipment: {issues}");
    let msg = list[0]["message"].as_str().unwrap_or("");
    assert!(msg.contains("equipment") && msg.contains("AIR HANDLING UNIT") && msg.contains("outside Level 1"), "{msg}");
    assert_eq!(list[0]["level"], json!("Level 1"));
    assert_eq!(list[0]["handles"].as_array().map(Vec::len), Some(1), "{issues}");

    // The default render frames the building: the stray is out of the picture, so the building
    // fills it.
    let dir = std::env::temp_dir().join(format!("cadaddy_levelregions_png_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("full.png").display().to_string();
    s.execute("model.render", &json!({"width": 400, "height": 300, "path": path})).unwrap();
    let full = ink(&path);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(full.0 > 0.6 * 400.0 || full.1 > 0.6 * 300.0, "the building fills the view: {full:?}");
}

/// The width and height of what a rendered PNG drew (its non-white pixels).
fn ink(path: &str) -> (f64, f64) {
    let img = image::open(path).unwrap().to_rgb8();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p.0.iter().any(|c| *c < 240) {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x1 < x0 { (0.0, 0.0) } else { (f64::from(x1 - x0), f64::from(y1 - y0)) }
}

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

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn a_jobs_other_sheets_are_theirs_not_outside_every_level() {
    let _g = serial();
    real_pdf_reader();
    let mut set = cadaddy_mepfix::generate_hotel(42, cadaddy_mepfix::hotel::HotelSize::Small).expect("hotel");
    let only = ["A-101", "A-102", "E-101", "M-101", "P-101"];
    set.truth.sheets.retain(|s| only.contains(&s.number.as_str()));
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_levelregions_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pages: Vec<(Vec<u8>, String, String)> =
        set.truth.sheets.iter().map(|sh| (set.file(&sh.pdf).unwrap().to_vec(), format!("{} {}", sh.number, sh.title), String::new())).collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).unwrap()).unwrap();
    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false})).unwrap();
    assert_eq!(st["done"], json!(true), "{st}");
    let _ = std::fs::remove_dir_all(&dir);

    // The spec knows where each imported sheet sits and its level.
    let spec = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    let sheets = spec["sheets"].as_array().cloned().unwrap_or_default();
    for n in only {
        let sh = sheets.iter().find(|x| x["number"] == n).unwrap_or_else(|| panic!("{n} in {sheets:?}"));
        assert!(sh["rect"].as_array().is_some_and(|r| r.len() == 4), "{sh}");
        let want = if n.ends_with("02") { "Level 2" } else { "Level 1" };
        assert_eq!(sh["level"], json!(want), "{sh}");
    }
    // Nothing on the electrical, mechanical and plumbing plans counts as outside every level.
    let v = s.execute("bspec.validate", &json!({})).unwrap();
    let outside: Vec<&Value> = v["warnings"].as_array().into_iter().flatten().filter(|w| w["path"] == "levels").collect();
    assert!(outside.is_empty(), "{outside:?}");

    // The built model: every object on a level of the spec, Level 2 over Level 1 in the view.
    let d = s.doc().unwrap();
    let names: Vec<&str> = spec["levels"].as_array().unwrap().iter().filter_map(|l| l["name"].as_str()).collect();
    assert_eq!(names, ["Level 1", "Level 2"]);
    assert!(d.model.iter().filter(|e| made_as(e).is_some()).all(|e| level_of(e).is_some_and(|l| names.contains(&l.as_str()))));
    let v = stacked_view(&s);
    let (v1, v2) = (extents(&v, "wall", "Level 1"), extents(&v, "wall", "Level 2"));
    assert!(v1.min.dist(v2.min) < 24.0, "Level 2 stands on Level 1: {v1:?} {v2:?}");
    // Every room ceiling inside its level's walls (none round a grid bubble or the border).
    for level in names {
        let w = extents(d, "wall", level).expand(12.0);
        for e in d.model.iter().filter(|e| made_as(e).as_deref() == Some("ceiling") && level_of(e).as_deref() == Some(level)) {
            let b = entity_bounds(d, e, 0);
            assert!(b.min.x >= w.min.x && b.min.y >= w.min.y && b.max.x <= w.max.x && b.max.y <= w.max.y, "{level}: ceiling {b:?} walls {w:?}");
        }
    }
    // What the readers built off the building is listed, each with where it came from.
    let issues = s.execute("model.issues", &json!({"kind": "outside_building", "limit": 1000})).unwrap();
    for i in issues["issues"].as_array().into_iter().flatten() {
        assert!(i["level"].is_string() && i["handles"].as_array().is_some_and(|h| !h.is_empty()), "{i}");
        assert!(i["message"].as_str().is_some_and(|m| m.contains("outside")), "{i}");
    }
}
