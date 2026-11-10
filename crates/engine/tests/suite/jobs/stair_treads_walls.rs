//! A stair drawn as linework (exploded, no stair block) on the wall layer: its tread lines, a
//! tread apart, are a flight's treads and never a wall's faces (Q6429). The wall tracer used to
//! pair each two neighbouring treads into an 11" wall, filling the stair well with walls.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// A line in inches on `layer`.
fn line_in(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: Vec3 { x: a.0, y: a.1, z: 0.0 }, b: Vec3 { x: b.0, y: b.1, z: 0.0 } }));
    e.common.layer = layer.into();
    d.model.push(e);
}

/// A rectangle's four sides (inches) on `layer`.
fn rect(d: &mut Drawing, layer: &str, (x0, y0): (f64, f64), (x1, y1): (f64, f64)) {
    for (a, b) in [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))] {
        line_in(d, layer, a, b);
    }
}

/// A walled room: outside `lo..hi` (inches), walls `t` thick.
fn walled(d: &mut Drawing, layer: &str, lo: (f64, f64), hi: (f64, f64), t: f64) {
    rect(d, layer, lo, hi);
    rect(d, layer, (lo.0 + t, lo.1 + t), (hi.0 - t, hi.1 - t));
}

/// The test's stair enclosure: 9' × 18' outside of 6" walls, a switchback inside it: two runs of
/// 8 tread lines 11" apart and 4' long, from the enclosure's faces to a centre line between the
/// runs, an UP note at the foot. Everything on `layer` (one-layer linework, as printed).
fn switchback(d: &mut Drawing, layer: &str, ox: f64, oy: f64) {
    walled(d, layer, (ox, oy), (ox + 108.0, oy + 216.0), 6.0);
    let (west, mid, east) = (ox + 6.0, ox + 54.0, ox + 102.0);
    for i in 0..8 {
        let y = oy + 48.0 + 11.0 * f64::from(i);
        line_in(d, layer, (west, y), (mid, y));
        line_in(d, layer, (mid, y), (east, y));
    }
    // The centre line between the runs (a handrail or the stringers back to back).
    line_in(d, layer, (mid, oy + 30.0), (mid, oy + 150.0));
}

/// A straight flight of `n` tread lines `step` apart and `width` wide, turned `deg` about its
/// foot at `at`, between its two stringers.
fn flight(d: &mut Drawing, layer: &str, at: (f64, f64), n: u32, step: f64, width: f64, deg: f64) {
    let (c, s) = (deg.to_radians().cos(), deg.to_radians().sin());
    let p = |x: f64, y: f64| (at.0 + x * c - y * s, at.1 + x * s + y * c);
    let run = step * f64::from(n - 1);
    for i in 0..n {
        let x = step * f64::from(i);
        line_in(d, layer, p(x, 0.0), p(x, width));
    }
    line_in(d, layer, p(-6.0, 0.0), p(run + 6.0, 0.0));
    line_in(d, layer, p(-6.0, width), p(run + 6.0, width));
}

fn open(d: Drawing) -> Session {
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    s
}

fn spec(s: &mut Session, layer: &str) {
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": layer, "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
}

/// The walls the tracer lists for L1: (thickness, length), inches.
fn planned(s: &mut Session) -> (Vec<(f64, f64)>, Value) {
    let plan = s.execute("wallgen.plan", &json!({"level": "L1"})).expect("plan");
    let walls = plan["walls"]
        .as_array()
        .expect("walls")
        .iter()
        .map(|w| {
            let (a, b) = ((w["a"][0].as_f64().unwrap(), w["a"][1].as_f64().unwrap()), (w["b"][0].as_f64().unwrap(), w["b"][1].as_f64().unwrap()));
            (w["thickness"].as_f64().unwrap(), ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt())
        })
        .collect();
    (walls, plan)
}

fn tread_thick(walls: &[(f64, f64)], step: f64) -> usize {
    walls.iter().filter(|(t, _)| (t - step).abs() < 0.5).count()
}

#[test]
fn a_switchbacks_tread_lines_in_its_enclosure_are_no_walls() {
    let mut d = Drawing::new_imperial();
    switchback(&mut d, "A-WALL", 0.0, 0.0);
    let mut s = open(d);
    s.execute("text", &json!({"at": [20.0, 30.0], "text": "UP", "height": 6})).expect("note");
    spec(&mut s, "A-WALL");
    let (walls, plan) = planned(&mut s);
    // The enclosure's four walls, 6" thick; not one 11" "wall" between two treads.
    assert_eq!(tread_thick(&walls, 11.0), 0, "{plan}");
    assert_eq!(walls.len(), 4, "{plan}");
    assert!(walls.iter().all(|(t, _)| (t - 6.0).abs() < 0.5), "{plan}");
    let notes = plan["notes"].to_string();
    assert!(notes.contains("14 line pairs of stair treads"), "{notes}");
    // model.build builds the enclosure and nothing in the well.
    let built = s.execute("model.build", &json!({})).expect("build");
    let made_walls = s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).count();
    assert_eq!(made_walls, 4, "{built}");
}

#[test]
fn a_straight_flight_at_any_angle_and_tread_is_no_walls_and_the_room_beside_it_stays() {
    // A 12-tread flight 9½" a tread and 44" wide between stringers, turned 30°, on a stair layer
    // traced as walls; a 20' × 10' room of 5" walls beside it.
    for (deg, step) in [(30.0, 9.5), (90.0, 12.0), (0.0, 10.0)] {
        let mut d = Drawing::new_imperial();
        flight(&mut d, "STAIRS", (400.0, 50.0), 12, step, 44.0, deg);
        walled(&mut d, "STAIRS", (0.0, 0.0), (240.0, 120.0), 5.0);
        let mut s = open(d);
        spec(&mut s, "STAIRS");
        let (walls, plan) = planned(&mut s);
        assert_eq!(tread_thick(&walls, step), 0, "{deg}° {step}\": {plan}");
        assert_eq!(walls.len(), 4, "{deg}° {step}\": the room's walls: {plan}");
        // The stair reader reads the same lines as the flight they are.
        let found = s.execute("stairs.detect", &json!({})).expect("detect");
        let treads: Vec<u64> = found["stairs"]
            .as_array()
            .expect("stairs")
            .iter()
            .flat_map(|st| st["runs"].as_array().cloned().unwrap_or_default())
            .filter_map(|r| r["treads"].as_u64())
            .collect();
        assert_eq!(treads, vec![11], "{deg}° {step}\": {found}");
    }
}

#[test]
fn walls_a_tread_apart_that_dont_stop_on_a_stringer_stay_walls() {
    // Two 11" walls side by side with an 11" chase between (four faces evenly 11" apart), long
    // and free at their ends: no flight (no line across them at their ends), so walls.
    let mut d = Drawing::new_imperial();
    for i in 0..4 {
        let y = 11.0 * f64::from(i);
        line_in(&mut d, "A-WALL", (0.0, y), (96.0, y));
    }
    // And three lines a tread apart between two cross lines: too few for a flight.
    for i in 0..3 {
        let y = 300.0 + 11.0 * f64::from(i);
        line_in(&mut d, "A-WALL", (0.0, y), (48.0, y));
    }
    line_in(&mut d, "A-WALL", (0.0, 290.0), (0.0, 330.0));
    line_in(&mut d, "A-WALL", (48.0, 290.0), (48.0, 330.0));
    let mut s = open(d);
    spec(&mut s, "A-WALL");
    let (walls, plan) = planned(&mut s);
    assert_eq!(tread_thick(&walls, 11.0), 5, "{plan}");
}
