//! A jamb: a short piece of wall between an opening and a crossing wall (Q6614). Drawn as two
//! faces it used to be dropped (pairs overlapping under 12" were no wall), so the opening read
//! wider than drawn; drawn as a centreline it was stretched past both ends, into the opening and
//! through the crossing wall. A pair of faces between an opening and a crossing wall is a wall
//! however short; the 12" minimum is for lone fragments with nothing at either end, and a
//! centreline wall is built to its drawn ends, extended only to meet the wall it runs into.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// A plan turned `deg` about the origin: inches in, inches out.
#[derive(Clone, Copy)]
struct Turn(f64);

impl Turn {
    fn to(self, (x, y): (f64, f64)) -> (f64, f64) {
        let (s, c) = self.0.to_radians().sin_cos();
        (x * c - y * s, x * s + y * c)
    }
    fn back(self, (x, y): (f64, f64)) -> (f64, f64) {
        Turn(-self.0).to((x, y))
    }
}

/// A line in inches on `layer`, turned.
fn line(d: &mut Drawing, t: Turn, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let (a, b) = (t.to(a), t.to(b));
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: Vec3 { x: a.0, y: a.1, z: 0.0 }, b: Vec3 { x: b.0, y: b.1, z: 0.0 } }));
    e.common.layer = layer.into();
    d.model.push(e);
}

fn open(d: Drawing, layers: Value, wall_types: Value) -> Session {
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": layers,
        "wall_types": wall_types,
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s
}

/// The walls the tracer lists for L1, turned back: (a, b, thickness), inches.
fn planned(s: &mut Session, t: Turn) -> (Vec<((f64, f64), (f64, f64), f64)>, Value) {
    let plan = s.execute("wallgen.plan", &json!({"level": "L1"})).expect("plan");
    let pt = |v: &Value| t.back((v[0].as_f64().unwrap(), v[1].as_f64().unwrap()));
    let walls = plan["walls"].as_array().expect("walls").iter().map(|w| (pt(&w["a"]), pt(&w["b"]), w["thickness"].as_f64().unwrap())).collect();
    (walls, plan)
}

/// The extent along x of the walls of thickness `th` lying on the line y = `y` (turned back):
/// each (x0, x1).
fn on_line(walls: &[((f64, f64), (f64, f64), f64)], y: f64, th: f64) -> Vec<(f64, f64)> {
    let mut v: Vec<(f64, f64)> = walls
        .iter()
        .filter(|(a, b, w)| (w - th).abs() < 0.3 && (a.1 - y).abs() < 0.3 && (b.1 - y).abs() < 0.3)
        .map(|(a, b, _)| (a.0.min(b.0), a.0.max(b.0)))
        .collect();
    v.sort_by(|p, q| p.0.total_cmp(&q.0));
    v
}

/// The test's plan, both faces drawn: a crossing wall `th` thick (faces x = 0 and x = th, from
/// y −60 to 120) that a wall `th` thick on y 0..th meets in a tee; that wall's first piece is a
/// jamb `jamb` long (from the crossing wall's face to the jamb line), then an opening `gap` wide,
/// then the wall runs on to x 200 where a second crossing wall stands.
fn faces(d: &mut Drawing, t: Turn, th: f64, jamb: f64, gap: f64) {
    let l = "A-WALL";
    // The crossing wall, its east face broken where the wall tees in.
    line(d, t, l, (0.0, -60.0), (0.0, 120.0));
    line(d, t, l, (th, -60.0), (th, 0.0));
    line(d, t, l, (th, th), (th, 120.0));
    // The jamb and its line across at the opening.
    let j = th + jamb;
    line(d, t, l, (th, 0.0), (j, 0.0));
    line(d, t, l, (th, th), (j, th));
    line(d, t, l, (j, 0.0), (j, th));
    // The wall beyond the opening, to the second crossing wall.
    let w = j + gap;
    line(d, t, l, (w, 0.0), (w, th));
    line(d, t, l, (w, 0.0), (200.0, 0.0));
    line(d, t, l, (w, th), (200.0, th));
    line(d, t, l, (200.0, -60.0), (200.0, 0.0));
    line(d, t, l, (200.0, th), (200.0, 120.0));
    line(d, t, l, (200.0 + th, -60.0), (200.0 + th, 120.0));
}

fn pairs_spec() -> (Value, Value) {
    (json!([{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}]), json!([{"name": "Partition", "thickness_in": 6}]))
}

#[test]
fn a_short_jamb_between_an_opening_and_a_crossing_wall_is_built() {
    for (deg, th, jamb) in [(0.0, 9.0, 6.0), (0.0, 9.0, 2.75), (90.0, 9.0, 6.0), (30.0, 6.0, 4.0), (0.0, 6.0, 8.0)] {
        let t = Turn(deg);
        let mut d = Drawing::new_imperial();
        faces(&mut d, t, th, jamb, 50.0);
        let (layers, types) = pairs_spec();
        let mut s = open(d, layers, types);
        let (walls, plan) = planned(&mut s, t);
        let run = on_line(&walls, th / 2.0, th);
        let j = th + jamb;
        // The jamb: a wall from the crossing wall to the jamb line, not past it into the opening.
        let jamb_wall = run.iter().find(|(x0, x1)| *x0 < th + 0.1 && *x1 > th + 0.5 && *x1 < j + 5.0);
        let Some(&(x0, x1)) = jamb_wall else { panic!("{deg}° {th}\" wall, {jamb}\" jamb: no jamb wall in {run:?}: {plan}") };
        assert!((x1 - j).abs() < 0.1, "{deg}° {th}\"/{jamb}\": the jamb ends at its line {j}: {run:?}");
        assert!(x0 > -0.1, "{deg}° {th}\"/{jamb}\": the jamb stops at the crossing wall's far face: {run:?}");
        // The opening stays as wide as drawn.
        assert!(!run.iter().any(|(a, b)| *b > j + 0.1 && *a < j + 50.0 - 0.1), "{deg}° {th}\"/{jamb}\": a wall in the opening: {run:?}");
    }
}

#[test]
fn a_lone_short_pair_is_still_no_wall() {
    // Two 6" lines 9" apart with nothing at either end (a symbol's bit, a mark), and a 6"
    // piece 9" thick against a crossing wall with nothing at its other end: no walls.
    let t = Turn(0.0);
    let mut d = Drawing::new_imperial();
    line(&mut d, t, "A-WALL", (300.0, 0.0), (306.0, 0.0));
    line(&mut d, t, "A-WALL", (300.0, 9.0), (306.0, 9.0));
    line(&mut d, t, "A-WALL", (0.0, -60.0), (0.0, 120.0));
    line(&mut d, t, "A-WALL", (9.0, -60.0), (9.0, 120.0));
    line(&mut d, t, "A-WALL", (9.0, 30.0), (15.0, 30.0));
    line(&mut d, t, "A-WALL", (9.0, 39.0), (15.0, 39.0));
    let (layers, types) = pairs_spec();
    let mut s = open(d, layers, types);
    let (walls, plan) = planned(&mut s, t);
    assert_eq!(walls.len(), 1, "only the crossing wall: {plan}");
}

#[test]
fn a_short_centreline_jamb_keeps_its_drawn_ends() {
    // Centrelines on a centreline wall layer, 9" walls: the crossing wall at x 4.5, the jamb
    // from the crossing wall to the opening at x 15 (drawn from its face, or from its
    // centreline), the opening 50" wide, the wall on to a second crossing wall at x 204.5.
    for (deg, from) in [(0.0, 9.0), (0.0, 4.5), (90.0, 9.0), (30.0, 4.5)] {
        let t = Turn(deg);
        let l = "A-WALL-CORE";
        let mut d = Drawing::new_imperial();
        line(&mut d, t, l, (4.5, -60.0), (4.5, 120.0));
        line(&mut d, t, l, (from, 4.5), (15.0, 4.5));
        line(&mut d, t, l, (65.0, 4.5), (204.5, 4.5));
        line(&mut d, t, l, (204.5, -60.0), (204.5, 120.0));
        let layers = json!([{"layer": l, "role": "wall", "wall_type": "Core"}]);
        let types = json!([{"name": "Core", "thickness_in": 9, "from": "centerline"}]);
        let mut s = open(d, layers, types);
        let (walls, plan) = planned(&mut s, t);
        let run = on_line(&walls, 4.5, 9.0);
        let Some(&(x0, x1)) = run.first() else { panic!("{deg}° from {from}: no jamb: {plan}") };
        assert!((x1 - 15.0).abs() < 0.1, "{deg}° from {from}: the jamb ends where drawn, at the opening: {run:?}");
        assert!(x0 > -0.1 && x0 <= 4.6, "{deg}° from {from}: the jamb meets the crossing wall and stops at its far face: {run:?}");
        // The wall beyond the opening keeps its opening end; its far end meets the second wall.
        let Some(&(y0, y1)) = run.get(1) else { panic!("{deg}° from {from}: no wall beyond: {plan}") };
        assert!((y0 - 65.0).abs() < 0.1 && (204.4..=209.1).contains(&y1), "{deg}° from {from}: {run:?}");
    }
}
