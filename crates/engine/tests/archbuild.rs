//! Architecture from the floor plans (Q5263): plans drawn side by side become one stacked
//! building, a level per plan; what pairs up outside the building (the border, dimension and
//! grid lines) builds no walls, nor does another discipline's copy of the plan; a door swing
//! drawn twice is one door; rooms keep their level; slabs end at the building; one sheet alone
//! still builds as before.

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

/// One 40' × 30' floor at `ox`: 6" outside walls (both faces), a 4" partition with a 3' door,
/// a room each side. `junk`: the sheet's border (double ruled), a dimension pair below the
/// building and a grid pair across it, all on the wall pen. `twice`: the door's swing drawn
/// twice (as prints often stroke it).
fn walls(s: &mut Session, ox: f64, junk: bool) {
    rect(s, ox, 0.0, ox + 480.0, 360.0);
    rect(s, ox + 6.0, 6.0, ox + 474.0, 354.0);
    for x in [238.0, 242.0] {
        line(s, [ox + x, 6.0], [ox + x, 150.0]);
        line(s, [ox + x, 186.0], [ox + x, 354.0]);
    }
    if junk {
        rect(s, ox - 180.0, -180.0, ox + 780.0, 580.0);
        rect(s, ox - 174.0, -174.0, ox + 774.0, 574.0);
        line(s, [ox, -60.0], [ox + 480.0, -60.0]);
        line(s, [ox, -50.0], [ox + 480.0, -50.0]);
        line(s, [ox + 120.0, -100.0], [ox + 120.0, 460.0]);
        line(s, [ox + 128.0, -100.0], [ox + 128.0, 460.0]);
    }
}

fn door(s: &mut Session, ox: f64, twice: bool) {
    for _ in 0..if twice { 2 } else { 1 } {
        s.execute("arc", &json!({"center": [ox + 240.0, 150.0], "radius": 36.0, "start": 0.0, "end": 90.0})).unwrap();
    }
}

fn tags(s: &mut Session, ox: f64, level: usize) {
    s.execute("text", &json!({"at": [ox + 100.0, 180.0], "text": "OFFICE", "height": 8.0})).unwrap();
    s.execute("text", &json!({"at": [ox + 100.0, 160.0], "text": format!("{level}01"), "height": 6.0})).unwrap();
    s.execute("text", &json!({"at": [ox + 340.0, 180.0], "text": "LOBBY", "height": 8.0})).unwrap();
    s.execute("text", &json!({"at": [ox + 340.0, 160.0], "text": format!("{level}02"), "height": 6.0})).unwrap();
}

/// `plans` floors side by side (the first's door drawn twice when `twice`), and an electrical
/// sheet's copy of the walls after them when `electrical`.
fn sheet(plans: usize, junk: bool, twice: bool, electrical: bool) -> Session {
    let mut s = Session::new();
    let copies = plans + usize::from(electrical);
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    for i in 0..copies {
        walls(&mut s, i as f64 * STEP, junk);
    }
    s.execute("layer.new", &json!({"name": "A-DOOR", "current": true})).unwrap();
    for i in 0..copies {
        door(&mut s, i as f64 * STEP, twice && i == 0);
    }
    s.execute("layer.new", &json!({"name": "A-AREA-IDEN", "current": true})).unwrap();
    for i in 0..copies {
        tags(&mut s, i as f64 * STEP, i + 1);
    }
    s
}

fn levels(plans: usize) -> Value {
    json!((0..plans)
        .map(|i| json!({"name": format!("Level {}", i + 1), "elevation_ft": 10.0 * i as f64, "height_ft": 10.0, "ceiling_ft": 8.0, "region": region(i)}))
        .collect::<Vec<_>>())
}

/// A built object's level (its WALLGEN tag after the part).
fn level_of(e: &Entity) -> Option<String> {
    made_as(e)?;
    e.common.xdata.iter().filter(|t| t.code == 1000).nth(1).map(|t| t.value.clone())
}

/// The built `part`s of `level`: how many, and their extents (x, y, z).
fn built(s: &Session, part: &str, level: &str) -> (usize, Bounds2, (f64, f64)) {
    let d = s.doc().unwrap();
    let mut b = Bounds2::EMPTY;
    let (mut z0, mut z1) = (f64::INFINITY, f64::NEG_INFINITY);
    let mut n = 0;
    for e in d.model.iter().filter(|e| made_as(e).as_deref() == Some(part) && level_of(e).as_deref() == Some(level)) {
        n += 1;
        b = b.union(&entity_bounds(d, e, 0));
        if let cadaddy_engine::doc::EntityKind::Mesh(m) = &e.kind {
            for v in &m.vertices {
                z0 = z0.min(v.z);
                z1 = z1.max(v.z);
            }
        }
    }
    (n, b, (z0, z1))
}

fn build(s: &mut Session, plans: usize, stack: bool) -> Value {
    s.execute("bspec.draft", &json!({"store": true, "levels": levels(plans), "stack": stack})).unwrap();
    s.execute("model.build", &json!({})).unwrap()
}

#[test]
fn side_by_side_plans_build_one_stacked_building() {
    // One clean floor: what a level should build.
    let mut one = sheet(1, false, false, false);
    let r1 = build(&mut one, 1, false);
    let (walls1, doors1) = (r1["walls"].as_u64().unwrap(), r1["doors"].as_u64().unwrap());
    assert!(walls1 >= 5, "{r1}");
    assert_eq!(doors1, 1, "{r1}");

    // Three floors with border, dimension and grid pairs, the first door drawn twice, and an
    // electrical copy of the plan beside them.
    let mut s = sheet(3, true, true, true);
    let r = build(&mut s, 3, true);
    assert_eq!(r["levels"], json!(3), "{r}");
    assert_eq!(r["walls"].as_u64(), Some(3 * walls1), "each floor builds its walls once, no border/grid/dimension walls, none from the E copy: {r}");
    assert_eq!(r["doors"].as_u64(), Some(3 * doors1), "one door per swing: {r}");
    assert_eq!(r["stacked"], json!(2), "{r}");

    let (n1, b1, z1) = built(&s, "wall", "Level 1");
    for level in ["Level 2", "Level 3"] {
        let (n, b, z) = built(&s, "wall", level);
        assert_eq!(n, n1, "{level}");
        // Stacked over the first floor's plan, a storey up.
        assert!(b.min.dist(b1.min) < 1e-6 && b.max.dist(b1.max) < 1e-6, "{level}: {b:?} vs {b1:?}");
        assert!(z.0 >= z1.1 - 1.0, "{level} sits above level 1: {z:?} vs {z1:?}");
    }
    // Slabs end at the building, not at the sheet.
    for level in ["Level 1", "Level 2", "Level 3"] {
        let (n, b, _) = built(&s, "floor", level);
        assert!(n >= 1, "{level} has a floor");
        assert!(b.width() <= 480.0 + 24.0 && b.height() <= 360.0 + 24.0, "{level} floor {b:?}");
        assert!(b.min.dist(b1.min) < 30.0, "{level} floor under the stacked walls: {b:?} vs {b1:?}");
    }
    // The plans themselves stay where they were drawn.
    let ext = s.doc().unwrap().extents(&cadaddy_engine::doc::Space::Model);
    assert!(ext.max.x > 2.0 * STEP, "{ext:?}");

    // Rooms: each floor's, with its level; same names on every floor are rooms of their own.
    assert_eq!(r["rooms"].as_u64(), Some(6), "{r}");
    let spec = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    let rooms = spec["rooms"].as_array().cloned().unwrap_or_default();
    assert_eq!(rooms.len(), 6, "{rooms:?}");
    for i in 1..=3 {
        let on: Vec<&Value> = rooms.iter().filter(|r| r["level"] == format!("Level {i}").as_str()).collect();
        assert_eq!(on.len(), 2, "Level {i}: {rooms:?}");
    }
    assert_eq!(spec["stack"], json!(true));

    // The takeoff groups by the built levels.
    let bom = s.execute("bim.bom", &json!({"group_by": "level"})).unwrap();
    let lv: std::collections::BTreeSet<String> =
        bom["lines"].as_array().into_iter().flatten().filter_map(|l| l["level"].as_str()).filter(|l| !l.is_empty()).map(str::to_string).collect();
    assert!(lv.len() >= 3, "lines on each level: {lv:?} {}", bom["lines"]);
}

#[test]
fn without_stacking_each_level_stands_over_its_plan() {
    let mut s = sheet(2, false, false, false);
    let r = build(&mut s, 2, false);
    assert!(r.get("stacked").is_none(), "{r}");
    let (_, b1, _) = built(&s, "wall", "Level 1");
    let (_, b2, _) = built(&s, "wall", "Level 2");
    assert!((b2.min.x - b1.min.x - STEP).abs() < 1e-6, "{b1:?} {b2:?}");
    // model.build {stack} stacks the same spec.
    let r = s.execute("model.build", &json!({"stack": true})).unwrap();
    assert_eq!(r["stacked"], json!(1), "{r}");
    let (_, b2, _) = built(&s, "wall", "Level 2");
    assert!(b2.min.dist(b1.min) < 1e-6, "{b1:?} {b2:?}");
}

#[test]
fn one_sheet_drafts_one_level_as_before() {
    let mut s = sheet(1, false, false, false);
    let r = s.execute("bspec.draft", &json!({"store": true})).unwrap();
    let levels = r["spec"]["levels"].as_array().cloned().unwrap_or_default();
    assert_eq!(levels.len(), 1, "{levels:?}");
    assert_eq!(levels[0]["name"], "Level 1");
    assert!(levels[0]["region"].is_null());
    assert!(r["spec"].get("stack").is_none(), "{}", r["spec"]);
    let b = s.execute("model.build", &json!({})).unwrap();
    assert!(b["walls"].as_u64().unwrap() >= 5, "{b}");
    assert_eq!(b["doors"], json!(1), "{b}");
}

#[test]
fn a_level_heights_guess_is_listed() {
    let mut s = sheet(1, false, false, false);
    let r = s
        .execute(
            "bspec.draft",
            &json!({"levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "region": region(0), "guess": "floor to floor assumed"}]}),
        )
        .unwrap();
    let guesses = r["guesses"].as_array().cloned().unwrap_or_default();
    assert!(guesses.iter().any(|g| g["key"] == "levels/Level 1" && g["why"] == "floor to floor assumed"), "{guesses:?}");
}

#[test]
fn hostile_draft_levels_never_panic() {
    let mut s = sheet(1, false, false, false);
    let many: Vec<Value> = (0..5_000).map(|i| json!({"name": format!("L{i}"), "elevation_ft": i, "height_ft": 10})).collect();
    for bad in [
        json!({"levels": "x"}),
        json!({"levels": [1, 2]}),
        json!({"levels": [{"name": "", "elevation_ft": 0, "height_ft": 10}]}),
        json!({"levels": [{"name": "A", "elevation_ft": 0, "height_ft": 0}]}),
        json!({"levels": [{"name": "A", "elevation_ft": 0, "height_ft": -3}]}),
        json!({"levels": [{"name": "A", "elevation_ft": 0, "height_ft": 10}, {"name": "a", "elevation_ft": 10, "height_ft": 10}]}),
        json!({"levels": [{"name": "A", "elevation_ft": 1e308, "height_ft": 1e308, "region": [0, 0, 1e308, 1e308]}]}),
        json!({"levels": [{"name": "A", "elevation_ft": 0, "height_ft": 10, "region": [1, 2]}]}),
        json!({"levels": many}),
    ] {
        let _ = s.execute("bspec.draft", &bad);
    }
    // A region with nothing in it: an empty level, no failure.
    let r = s.execute(
        "bspec.draft",
        &json!({"store": true, "levels": [{"name": "A", "elevation_ft": 0, "height_ft": 10, "region": [5e5, 5e5, 6e5, 6e5]}], "stack": true}),
    );
    assert!(r.is_ok(), "{r:?}");
    let _ = s.execute("model.build", &json!({}));
    let _ = s.execute("model.build", &json!({"stack": "yes"}));
}

/// One 40' × 30' floor whose 4" partition has a 6' opening (y 150-222),
/// swung by `arcs` (centre, start, end).
fn opening_plan(arcs: &[([f64; 2], f64, f64)]) -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    rect(&mut s, 0.0, 0.0, 480.0, 360.0);
    rect(&mut s, 6.0, 6.0, 474.0, 354.0);
    for x in [238.0, 242.0] {
        line(&mut s, [x, 6.0], [x, 150.0]);
        line(&mut s, [x, 222.0], [x, 354.0]);
    }
    s.execute("layer.new", &json!({"name": "A-DOOR", "current": true})).unwrap();
    for (c, a0, a1) in arcs {
        s.execute("arc", &json!({"center": c, "radius": 36.0, "start": a0, "end": a1})).unwrap();
    }
    s
}

#[test]
fn a_double_door_is_one_door_and_connecting_doors_are_two() {
    // A pair of 3' leaves meeting in the middle of the 6' opening, both swinging east.
    let mut s = opening_plan(&[([240.0, 150.0], 0.0, 90.0), ([240.0, 222.0], 270.0, 360.0)]);
    let r = build(&mut s, 1, false);
    assert_eq!(r["doors"], json!(1), "a double door is one door: {r}");
    // Its one door object carries both leaves (spans the whole opening).
    let (n, b, _) = built(&s, "door", "Level 1");
    assert_eq!(n, 1);
    assert!(b.height() > 60.0, "both leaves: {b:?}");

    // The same pair drawn twice is still one door.
    let mut s =
        opening_plan(&[([240.0, 150.0], 0.0, 90.0), ([240.0, 222.0], 270.0, 360.0), ([240.0, 150.0], 0.0, 90.0), ([240.0, 222.0], 270.0, 360.0)]);
    let r = build(&mut s, 1, false);
    assert_eq!(r["doors"], json!(1), "{r}");

    // Back-to-back connecting doors (one leaf swinging into each room) stay two doors.
    let mut s = opening_plan(&[([240.0, 150.0], 0.0, 90.0), ([240.0, 150.0], 90.0, 180.0)]);
    let r = build(&mut s, 1, false);
    assert_eq!(r["doors"], json!(2), "{r}");

    // Two single doors side by side in one wall, both swinging east, hinges apart: two doors
    // (the wall stops for each: openings 150-186 and 264-300).
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    rect(&mut s, 0.0, 0.0, 480.0, 360.0);
    rect(&mut s, 6.0, 6.0, 474.0, 354.0);
    for x in [238.0, 242.0] {
        line(&mut s, [x, 6.0], [x, 150.0]);
        line(&mut s, [x, 186.0], [x, 264.0]);
        line(&mut s, [x, 300.0], [x, 354.0]);
    }
    s.execute("layer.new", &json!({"name": "A-DOOR", "current": true})).unwrap();
    s.execute("arc", &json!({"center": [240.0, 150.0], "radius": 36.0, "start": 0.0, "end": 90.0})).unwrap();
    s.execute("arc", &json!({"center": [240.0, 300.0], "radius": 36.0, "start": 270.0, "end": 360.0})).unwrap();
    let r = build(&mut s, 1, false);
    assert_eq!(r["doors"], json!(2), "{r}");
}
