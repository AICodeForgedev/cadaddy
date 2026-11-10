//! WALLGEN and callout boundaries: the rounded rectangle a plan draws round an enlarged-plan
//! callout (a stair, a core, a typical unit) is an annotation, never a wall, however it is drawn
//! (one closed polyline with arcs or with faceted corners, or a dash-dot line a print flattens
//! into dashes, dots and corner pieces), while a wall drawn with rounded corners on both faces is
//! still built.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line, LwPolyline, Mesh};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn rules() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "kind": "wall", "skip_curves": true}]
    })
}

fn next_handle(d: &mut Drawing) -> Handle {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    h
}

/// A line in inches on `layer`.
fn line_in(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let h = next_handle(d);
    let mut e = Entity::new(h, EntityKind::Line(Line { a: Vec3 { x: a.0, y: a.1, z: 0.0 }, b: Vec3 { x: b.0, y: b.1, z: 0.0 } }));
    e.common.layer = layer.into();
    d.model.push(e);
}

/// A polyline in inches on A-WALL: `(x, y, bulge)` vertices.
fn poly(d: &mut Drawing, pts: &[(f64, f64, f64)], closed: bool) {
    let vertices: Vec<Value> =
        pts.iter().map(|&(x, y, b)| json!({"p": {"x": x, "y": y}, "bulge": b, "start_width": 0.0, "end_width": 0.0})).collect();
    let pl: LwPolyline = serde_json::from_value(json!({"vertices": vertices, "closed": closed})).unwrap();
    let h = next_handle(d);
    let mut e = Entity::new(h, EntityKind::LwPolyline(pl));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
}

/// A 30' × 12' room of 6" walls (outside faces x 0–360", y 0–144"), its south wall broken by a
/// 6' window gap at x 144–216" with the glazing drawn across it.
fn room() -> Drawing {
    let mut d = Drawing::new_imperial();
    let w = |d: &mut Drawing, a: (f64, f64), b: (f64, f64)| line_in(d, "A-WALL", a, b);
    for (a, b) in [
        ((0., 0.), (144., 0.)),
        ((216., 0.), (360., 0.)),
        ((360., 0.), (360., 144.)),
        ((360., 144.), (0., 144.)),
        ((0., 144.), (0., 0.)),
        ((6., 6.), (144., 6.)),
        ((216., 6.), (354., 6.)),
        ((354., 6.), (354., 138.)),
        ((354., 138.), (6., 138.)),
        ((6., 138.), (6., 6.)),
        ((144., 0.), (144., 6.)),
        ((216., 0.), (216., 6.)),
    ] {
        w(&mut d, a, b);
    }
    line_in(&mut d, "A-GLAZ", (144., 3.), (216., 3.));
    d
}

/// The points of a quarter circle round `c` from angle `a0` (degrees) turning +90°, in `n` facets.
fn quarter(c: (f64, f64), r: f64, a0: f64, n: usize) -> Vec<(f64, f64)> {
    (0..=n)
        .map(|i| (a0 + 90.0 * i as f64 / n as f64).to_radians())
        .map(|a| (c.0 + r * a.cos(), c.1 + r * a.sin()))
        .collect()
}

/// The callout's box: x0, y0, x1, y1 in inches, corner radius 10". South 4" outside the south
/// wall's face; the other sides `margin` out.
fn callout_box(margin: f64) -> (f64, f64, f64, f64) {
    (-margin, -4.0, 360.0 + margin, 144.0 + margin)
}

/// The rounded rectangle's outline counter-clockwise, its corners faceted in six (as a print
/// flattens an arc).
fn faceted_outline((x0, y0, x1, y1): (f64, f64, f64, f64), r: f64) -> Vec<(f64, f64)> {
    let mut pts = Vec::new();
    pts.extend(quarter((x1 - r, y0 + r), r, 270.0, 6));
    pts.extend(quarter((x1 - r, y1 - r), r, 0.0, 6));
    pts.extend(quarter((x0 + r, y1 - r), r, 90.0, 6));
    pts.extend(quarter((x0 + r, y0 + r), r, 180.0, 6));
    pts
}

/// The callout drawn as a print flattens a dash-dot line: 18" dashes, 2" dots, 4" spaces; each
/// corner one open polyline (the dash running round it: straight, the faceted arc, straight), the
/// sides' dashes and dots separate lines.
fn dash_dot_callout(d: &mut Drawing, (x0, y0, x1, y1): (f64, f64, f64, f64), r: f64) {
    let (dash, dot, gap) = (18.0, 2.0, 4.0);
    // Each corner: 10" of the dash before the arc, the arc, 8" after.
    let corners = [((x1 - r, y0 + r), 270.0), ((x1 - r, y1 - r), 0.0), ((x0 + r, y1 - r), 90.0), ((x0 + r, y0 + r), 180.0)];
    for (c, a0) in corners {
        let arc = quarter(c, r, a0, 6);
        let (Some(&first), Some(&last)) = (arc.first(), arc.last()) else { continue };
        let t0 = (a0 + 90.0).to_radians();
        let t1 = (a0 + 180.0).to_radians();
        let start = (first.0 - 10.0 * t0.cos(), first.1 - 10.0 * t0.sin());
        let end = (last.0 + 8.0 * t1.cos(), last.1 + 8.0 * t1.sin());
        let mut pts = vec![(start.0, start.1, 0.0)];
        pts.extend(arc.iter().map(|&(x, y)| (x, y, 0.0)));
        pts.push((end.0, end.1, 0.0));
        poly(d, &pts, false);
    }
    // The sides between the corner pieces: a space, then dot, space, dash, space, ... up to the
    // next corner piece's space.
    let sides = [((x0 + r + 8.0, y0), (x1 - r - 10.0, y0)), ((x1, y0 + r + 8.0), (x1, y1 - r - 10.0)), ((x1 - r - 8.0, y1), (x0 + r + 10.0, y1)), ((x0, y1 - r - 8.0), (x0, y0 + r + 10.0))];
    for (a, b) in sides {
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let u = ((b.0 - a.0) / len, (b.1 - a.1) / len);
        let at = |s: f64| (a.0 + u.0 * s, a.1 + u.1 * s);
        let mut s = gap;
        let mut is_dot = true;
        while s + gap < len {
            let piece = if is_dot { dot } else { dash };
            let e = (s + piece).min(len - gap);
            line_in(d, "A-WALL", at(s), at(e));
            s = e + gap;
            is_dot = !is_dot;
        }
    }
}

fn made(s: &Session, part: &str) -> Vec<Mesh> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some(part))
        .filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None })
        .collect()
}

/// The plan boxes of the built walls' and windows' 8-vertex boxes.
fn boxes(s: &Session, part: &str) -> Vec<(f64, f64, f64, f64)> {
    made(s, part)
        .iter()
        .flat_map(|m| {
            m.vertices
                .chunks(8)
                .map(|b| b.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |(x0, y0, x1, y1), v| (x0.min(v.x), y0.min(v.y), x1.max(v.x), y1.max(v.y))))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn build(d: Drawing) -> (Session, Value) {
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    let r = s.execute("wallgen", &json!({"rules": rules()})).unwrap();
    (s, r)
}

/// The room's five walls (the gap splits the south one) and its one window, with nothing south of the south wall's face and the
/// window glass across the drawn gap (x 144–216").
fn assert_room_alone(s: &Session, r: &Value, what: &str) {
    assert_eq!((r["walls"].clone(), r["windows"].clone()), (json!(5), json!(1)), "{what}: {r}");
    let walls = boxes(s, "wall");
    assert!(walls.iter().all(|b| b.1 >= -0.01), "{what}: a wall south of the south wall's face: {walls:?}");
    let glass = made(s, "window");
    let g = glass.first().map(|m| m.vertices.iter().take(8).fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v.x), hi.max(v.x))));
    assert!(g.is_some_and(|(lo, hi)| (lo - 144.0).abs() < 0.5 && (hi - 216.0).abs() < 0.5), "{what}: window {g:?}");
}

#[test]
fn the_room_alone_has_four_walls_and_one_window() {
    let (s, r) = build(room());
    assert_room_alone(&s, &r, "no callout");
}

#[test]
fn a_closed_rounded_callout_is_no_wall() {
    // Faceted corners (a print's arcs), the other sides 2' out and, tighter, 4" out all round.
    for margin in [24.0, 4.0] {
        let mut d = room();
        let pts: Vec<(f64, f64, f64)> = faceted_outline(callout_box(margin), 10.0).into_iter().map(|(x, y)| (x, y, 0.0)).collect();
        poly(&mut d, &pts, true);
        let (s, r) = build(d);
        assert_room_alone(&s, &r, &format!("faceted callout {margin}\" out"));
    }
    // True arcs at the corners (bulge tan(90°/4)).
    let (x0, y0, x1, y1) = callout_box(4.0);
    let b = (std::f64::consts::FRAC_PI_8).tan();
    let r = 10.0;
    let pts = [
        (x0 + r, y0, 0.0),
        (x1 - r, y0, b),
        (x1, y0 + r, 0.0),
        (x1, y1 - r, b),
        (x1 - r, y1, 0.0),
        (x0 + r, y1, b),
        (x0, y1 - r, 0.0),
        (x0, y0 + r, b),
    ];
    let mut d = room();
    poly(&mut d, &pts, true);
    let (s, r) = build(d);
    assert_room_alone(&s, &r, "arc-cornered callout");
}

#[test]
fn a_dash_dot_callout_in_pieces_is_no_wall() {
    for margin in [24.0, 4.0] {
        let mut d = room();
        dash_dot_callout(&mut d, callout_box(margin), 10.0);
        let (s, r) = build(d);
        assert_room_alone(&s, &r, &format!("dash-dot callout {margin}\" out"));
    }
}

#[test]
fn a_wall_with_rounded_corners_on_both_faces_is_still_built() {
    // A 30' × 12' room whose 6" walls are two closed loops with rounded corners: the outside face
    // on a 16" radius, the inside one on 10" (one centre per corner).
    let mut d = Drawing::new_imperial();
    let outer: Vec<(f64, f64, f64)> = faceted_outline((0.0, 0.0, 360.0, 144.0), 16.0).into_iter().map(|(x, y)| (x, y, 0.0)).collect();
    let inner: Vec<(f64, f64, f64)> = faceted_outline((6.0, 6.0, 354.0, 138.0), 10.0).into_iter().map(|(x, y)| (x, y, 0.0)).collect();
    poly(&mut d, &outer, true);
    poly(&mut d, &inner, true);
    let (s, r) = build(d);
    let walls = r["walls"].as_u64().unwrap_or(0);
    assert!(walls >= 4, "{r}");
    // Each straight side is built.
    let b = boxes(&s, "wall");
    for (x, y) in [(180.0, 3.0), (180.0, 141.0), (3.0, 72.0), (357.0, 72.0)] {
        assert!(b.iter().any(|w| x > w.0 && x < w.2 && y > w.1 && y < w.3), "no wall at ({x}, {y}): {b:?}");
    }
}
