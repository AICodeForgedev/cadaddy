//! Doors not inflated (Q5273): an arc is a door only where a wall stops for it. An arc hinged on
//! a solid wall (a fixture or a piece of equipment against it), or one touching a wall but
//! shutting off its line, builds no door; a view of part of the plan drawn at a larger scale
//! beside it (an enlarged plan) builds no walls or doors; a second wing at the plan's own scale
//! still builds.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, x0: f64, y0: f64, x1: f64, y1: f64) {
    line(s, [x0, y0], [x1, y0]);
    line(s, [x1, y0], [x1, y1]);
    line(s, [x1, y1], [x0, y1]);
    line(s, [x0, y1], [x0, y0]);
}

fn arc(s: &mut Session, c: [f64; 2], r: f64, start: f64, end: f64) {
    s.execute("arc", &json!({"center": c, "radius": r, "start": start, "end": end})).unwrap();
}

/// A floor at (`ox`, `oy`), `scale` times a 40' × 30' plan in inches: outside walls 6" thick, a
/// 4" partition with a door opening `door` wide (at scale 1) and its swing, hinged at the jamb.
fn floor(s: &mut Session, ox: f64, oy: f64, scale: f64, door: f64) {
    let p = |x: f64, y: f64| [ox + x * scale, oy + y * scale];
    s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    let r = |s: &mut Session, a: [f64; 2], b: [f64; 2]| rect(s, a[0], a[1], b[0], b[1]);
    r(s, p(0.0, 0.0), p(480.0, 360.0));
    r(s, p(6.0, 6.0), p(474.0, 354.0));
    for x in [238.0, 242.0] {
        line(s, p(x, 6.0), p(x, 150.0));
        line(s, p(x, 150.0 + door), p(x, 354.0));
    }
    s.execute("layer.current", &json!({"name": "A-DOOR"})).unwrap();
    arc(s, p(240.0, 150.0), door * scale, 0.0, 90.0);
}

/// A 20' x 15' room with a closet partition at twice the scale at (`ox`, `oy`): 6" walls drawn
/// 12" thick, a 4" partition drawn 8" with a 24" door (48" on the sheet).
fn enlarged_room(s: &mut Session, ox: f64, oy: f64) {
    let p = |x: f64, y: f64| [ox + 2.0 * x, oy + 2.0 * y];
    s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    let r = |s: &mut Session, a: [f64; 2], b: [f64; 2]| rect(s, a[0], a[1], b[0], b[1]);
    r(s, p(0.0, 0.0), p(240.0, 180.0));
    r(s, p(6.0, 6.0), p(234.0, 174.0));
    for x in [158.0, 162.0] {
        line(s, p(x, 6.0), p(x, 80.0));
        line(s, p(x, 104.0), p(x, 174.0));
    }
    s.execute("layer.current", &json!({"name": "A-DOOR"})).unwrap();
    arc(s, p(160.0, 80.0), 48.0, 0.0, 90.0);
}

fn session() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("layer.new", &json!({"name": "A-DOOR"})).unwrap();
    s
}

fn build(s: &mut Session, region: [f64; 4]) -> Value {
    let levels = json!([{"name": "Level 1", "elevation_ft": 0.0, "height_ft": 10.0, "ceiling_ft": 8.0, "region": region}]);
    s.execute("bspec.draft", &json!({"store": true, "levels": levels})).unwrap();
    s.execute("model.build", &json!({})).unwrap()
}

const REGION: [f64; 4] = [-200.0, -200.0, 2400.0, 1200.0];

fn count(r: &Value, k: &str) -> u64 {
    r[k].as_u64().unwrap_or_else(|| panic!("{k} in {r}"))
}

#[test]
fn arcs_on_solid_walls_or_off_the_wall_line_are_no_doors() {
    let mut clean = session();
    floor(&mut clean, 0.0, 0.0, 1.0, 36.0);
    let r0 = build(&mut clean, REGION);
    assert_eq!(count(&r0, "doors"), 1, "{r0}");

    let mut s = session();
    floor(&mut s, 0.0, 0.0, 1.0, 36.0);
    s.execute("layer.new", &json!({"name": "P-FIXT", "current": true})).unwrap();
    // Hinged on the solid south wall's centreline, shutting along it: a fixture, not a door.
    arc(&mut s, [100.0, 3.0], 30.0, 0.0, 90.0);
    // The same on the solid north wall, swung the other way.
    arc(&mut s, [360.0, 357.0], 24.0, 180.0, 270.0);
    // Touching the south wall at its centre, both ends off the wall line.
    arc(&mut s, [300.0, 3.0], 30.0, 45.0, 135.0);
    let r = build(&mut s, REGION);
    assert_eq!(count(&r, "doors"), 1, "only the partition's door: {r}");
    assert_eq!(count(&r, "walls"), count(&r0, "walls"), "{r}");
    let notes = r["notes"].to_string();
    assert!(notes.contains("3 arcs not taken for doors"), "{notes}");
}

#[test]
fn a_swing_with_its_leaf_on_an_unbroken_wall_is_still_a_door() {
    let mut s = session();
    floor(&mut s, 0.0, 0.0, 1.0, 36.0);
    // A door symbol (arc and leaf) on the solid south wall, as drawings with unbroken walls have.
    s.execute("layer.current", &json!({"name": "A-DOOR"})).unwrap();
    arc(&mut s, [100.0, 6.0], 30.0, 0.0, 90.0);
    line(&mut s, [100.0, 6.0], [100.0, 36.0]);
    let r = build(&mut s, REGION);
    assert_eq!(count(&r, "doors"), 2, "{r}");
}

#[test]
fn a_door_in_an_opening_with_a_header_drawn_across_is_still_a_door() {
    let mut s = session();
    floor(&mut s, 0.0, 0.0, 1.0, 36.0);
    // A header pair drawn across the opening only (jamb to jamb): no solid wall.
    s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    line(&mut s, [238.0, 150.0], [238.0, 186.0]);
    line(&mut s, [242.0, 150.0], [242.0, 186.0]);
    let r = build(&mut s, REGION);
    assert_eq!(count(&r, "doors"), 1, "{r}");
}

#[test]
fn an_enlarged_view_beside_the_plan_builds_nothing() {
    let mut one = session();
    floor(&mut one, 0.0, 0.0, 1.0, 36.0);
    let r0 = build(&mut one, REGION);

    // The plan, and its part (the west room, 20' x 15') drawn at twice the scale beside it: on
    // the sheet as long as the plan, its walls twice as thick, its 24" closet door reading 48".
    let mut s = session();
    floor(&mut s, 0.0, 0.0, 1.0, 36.0);
    enlarged_room(&mut s, 900.0, 0.0);
    let r = build(&mut s, REGION);
    assert_eq!(count(&r, "walls"), count(&r0, "walls"), "the enlarged view's walls go: {r}");
    assert_eq!(count(&r, "doors"), 1, "and its door: {r}");
}

#[test]
fn a_second_wing_at_the_plans_scale_still_builds() {
    let mut one = session();
    floor(&mut one, 0.0, 0.0, 1.0, 36.0);
    let r0 = build(&mut one, REGION);

    let mut s = session();
    floor(&mut s, 0.0, 0.0, 1.0, 36.0);
    floor(&mut s, 700.0, 0.0, 1.0, 36.0);
    let r = build(&mut s, REGION);
    assert_eq!(count(&r, "walls"), 2 * count(&r0, "walls"), "{r}");
    assert_eq!(count(&r, "doors"), 2, "{r}");
}

#[test]
fn hostile_arcs_never_panic() {
    let mut s = session();
    floor(&mut s, 0.0, 0.0, 1.0, 36.0);
    s.execute("layer.current", &json!({"name": "A-DOOR"})).unwrap();
    for (c, r) in [([240.0, 150.0], 1e-12), ([1e300, 3.0], 30.0), ([240.0, 150.0], 54.0), ([0.0, 0.0], 16.0)] {
        let _ = s.execute("arc", &json!({"center": c, "radius": r, "start": 0.0, "end": 90.0}));
    }
    let r = build(&mut s, REGION);
    assert!(r["doors"].as_u64().is_some(), "{r}");
}

fn pline(s: &mut Session, pts: &[[f64; 2]]) {
    s.execute("pline", &json!({"points": pts})).unwrap();
}

/// A 4" partition at `x` across the floor of [`floor`], with a 36" opening from y 150 and its
/// swing hinged at the jamb.
fn partition(s: &mut Session, x: f64) {
    s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    for x in [x - 2.0, x + 2.0] {
        line(s, [x, 6.0], [x, 150.0]);
        line(s, [x, 186.0], [x, 354.0]);
    }
    s.execute("layer.current", &json!({"name": "A-DOOR"})).unwrap();
    arc(s, [x, 150.0], 36.0, 0.0, 90.0);
}

/// [`floor`] with the partition's opening and no swing.
fn open_floor(s: &mut Session) {
    s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    rect(s, 0.0, 0.0, 480.0, 360.0);
    rect(s, 6.0, 6.0, 474.0, 354.0);
    for x in [238.0, 242.0] {
        line(s, [x, 6.0], [x, 150.0]);
        line(s, [x, 186.0], [x, 354.0]);
    }
}

#[test]
fn a_curve_by_an_opening_hangs_no_door_there() {
    // The opening with no swing: a cased opening, no door.
    let mut bare = session();
    open_floor(&mut bare);
    let r0 = build(&mut bare, REGION);
    assert_eq!(count(&r0, "doors"), 0, "{r0}");

    // A piece of furniture beside it, its corners about a leaf's width from the jamb (30", 40",
    // 34"): no swing hung from the jamb, no door.
    let mut s = session();
    open_floor(&mut s);
    s.execute("layer.new", &json!({"name": "I-FURN", "current": true})).unwrap();
    pline(&mut s, &[[270.0, 150.0], [268.0, 178.6], [240.0, 184.0]]);
    let r = build(&mut s, REGION);
    assert_eq!(count(&r, "doors"), 0, "furniture by an opening is no door: {r}");
    assert_eq!(count(&r, "walls"), count(&r0, "walls"), "{r}");

    // The swing hung from the jamb, flattened to chords as prints arrive: a door.
    let mut flat = session();
    open_floor(&mut flat);
    flat.execute("layer.current", &json!({"name": "A-DOOR"})).unwrap();
    let pts: Vec<[f64; 2]> = (0..=8)
        .map(|i| {
            let a = f64::from(i) * std::f64::consts::FRAC_PI_2 / 8.0;
            [242.0 + 34.0 * a.cos(), 150.0 + 34.0 * a.sin()]
        })
        .collect();
    pline(&mut flat, &pts);
    let r = build(&mut flat, REGION);
    assert_eq!(count(&r, "doors"), 1, "{r}");
}

#[test]
fn swings_on_solid_walls_go_when_the_plan_breaks_its_walls_for_doors() {
    // Three partitions, each with its opening and swing: this plan breaks its walls for doors.
    let mut plan = session();
    floor(&mut plan, 0.0, 0.0, 1.0, 36.0);
    partition(&mut plan, 120.0);
    partition(&mut plan, 360.0);
    let r0 = build(&mut plan, REGION);
    assert_eq!(count(&r0, "doors"), 3, "{r0}");

    // A swing with its leaf on the solid south wall (a cabinet's, a shower's): no door here.
    let mut s = session();
    floor(&mut s, 0.0, 0.0, 1.0, 36.0);
    partition(&mut s, 120.0);
    partition(&mut s, 360.0);
    s.execute("layer.current", &json!({"name": "A-DOOR"})).unwrap();
    arc(&mut s, [60.0, 6.0], 30.0, 0.0, 90.0);
    line(&mut s, [60.0, 6.0], [60.0, 36.0]);
    let r = build(&mut s, REGION);
    assert_eq!(count(&r, "doors"), 3, "{r}");
    assert_eq!(count(&r, "walls"), count(&r0, "walls"), "{r}");
    let notes = r["notes"].to_string();
    assert!(notes.contains("1 arcs not taken for doors"), "{notes}");
}
