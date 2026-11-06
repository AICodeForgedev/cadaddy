//! Hatch and region loops keep elliptical and spline boundaries as exact curves: HATCH (select
//! objects or a point inside), REGION, AREA, transforms and associative updates.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle, HatchLoop};
use cadaddy_engine::geom::{LoopEdge, Vec2};
use serde_json::{Value, json};

const PI: f64 = std::f64::consts::PI;

fn last(s: &Session) -> Handle {
    s.doc().unwrap().model.last().unwrap().handle
}

fn hatch_loops(s: &Session) -> Vec<HatchLoop> {
    s.doc().unwrap().model.iter().find_map(|e| if let EntityKind::Hatch(h) = &e.kind { Some(h.loops.clone()) } else { None }).expect("a hatch")
}

fn hatch_handle(s: &Session) -> Handle {
    s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::Hatch(_))).unwrap().handle
}

/// An ellipse at the origin, semi-axes 5 and 2.
fn ellipse(s: &mut Session) -> Handle {
    s.execute("ellipse", &json!({ "center": [0, 0], "major": [5, 0], "ratio": 0.4 })).unwrap();
    last(s)
}

fn area(s: &mut Session, h: Handle) -> f64 {
    let r: Value = s.execute("area", &json!({ "handle": h.hex() })).unwrap();
    r["area"].as_f64().unwrap_or(f64::NAN)
}

#[test]
fn a_hatch_of_a_selected_ellipse_keeps_the_ellipse_exactly() {
    let mut s = Session::new();
    let e = ellipse(&mut s);
    s.execute("hatch", &json!({ "handles": [e.hex()], "pattern": "SOLID" })).unwrap();
    let loops = hatch_loops(&s);
    assert_eq!(loops.len(), 1);
    assert!(matches!(loops[0].curves.as_slice(), [LoopEdge::Ellipse { .. }]), "{:?}", loops[0].curves);
    // AREA: exactly π·a·b, not the polygon's.
    let h = hatch_handle(&s);
    let a = area(&mut s, h);
    assert!((a - PI * 10.0).abs() < 1e-9, "{a}");
}

#[test]
fn a_point_inside_an_ellipse_gives_its_exact_boundary() {
    let mut s = Session::new();
    ellipse(&mut s);
    s.execute("hatch", &json!({ "points": [[0.5, 0.2]], "pattern": "ANSI31" })).unwrap();
    let loops = hatch_loops(&s);
    assert!(matches!(loops[0].curves.as_slice(), [LoopEdge::Ellipse { .. }]), "{:?}", loops[0].curves);
    assert!((loops[0].area().abs() - PI * 10.0).abs() < 1e-9);
}

#[test]
fn an_island_ellipse_keeps_its_curve_too() {
    let mut s = Session::new();
    s.execute("rectang", &json!({ "p1": [-10, -10], "p2": [10, 10] })).unwrap();
    ellipse(&mut s);
    s.execute("hatch", &json!({ "points": [[-9, -9]], "pattern": "ANSI31" })).unwrap();
    let loops = hatch_loops(&s);
    assert_eq!(loops.len(), 2, "outer square and the ellipse island");
    assert!(loops[0].curves.is_empty(), "lines: a plain polyline loop");
    assert!(matches!(loops[1].curves.as_slice(), [LoopEdge::Ellipse { .. }]));
    let h = hatch_handle(&s);
    let a = area(&mut s, h);
    assert!((a - (400.0 - PI * 10.0)).abs() < 1e-9, "{a}");
}

#[test]
fn a_region_of_a_closed_spline_keeps_the_spline() {
    let mut s = Session::new();
    s.execute("spline", &json!({ "fit": [[0, 0], [4, 1], [5, 4], [1, 5]], "closed": true })).unwrap();
    let sp = last(&s);
    let exact = match &s.doc().unwrap().entity(sp).unwrap().kind {
        EntityKind::Spline(g) => cadaddy_engine::geom::loop_area(&[LoopEdge::Spline(g.as_ref().clone())]).abs(),
        k => panic!("{k:?}"),
    };
    s.execute("region", &json!({ "handles": [sp.hex()] })).unwrap();
    let EntityKind::Region(r) = &s.doc().unwrap().entity(sp).unwrap().kind else { panic!("no region") };
    assert!(matches!(r.loops[0].curves.as_slice(), [LoopEdge::Spline(_)]));
    assert!((r.area() - exact).abs() < 1e-9, "{} vs {exact}", r.area());
    let a = area(&mut s, sp);
    assert!((a - exact).abs() < 1e-9);
}

#[test]
fn transforms_keep_the_curves_and_scale_the_area() {
    let mut s = Session::new();
    let e = ellipse(&mut s);
    s.execute("hatch", &json!({ "handles": [e.hex()], "pattern": "SOLID" })).unwrap();
    let h = hatch_handle(&s);
    s.execute("rotate", &json!({ "handles": [h.hex()], "base": [0, 0], "angle": 30 })).unwrap();
    s.execute("move", &json!({ "handles": [h.hex()], "delta": [7, -3] })).unwrap();
    s.execute("scale", &json!({ "handles": [h.hex()], "base": [0, 0], "factor": 2 })).unwrap();
    s.execute("mirror", &json!({ "handles": [h.hex()], "p1": [0, 0], "p2": [0, 1], "erase": true })).unwrap();
    let loops = hatch_loops(&s);
    assert!(matches!(loops[0].curves.as_slice(), [LoopEdge::Ellipse { .. }]));
    let a = area(&mut s, h);
    assert!((a - PI * 40.0).abs() < 1e-9, "{a}");
    // The vertices follow the curve (they're made from it).
    let c = match &loops[0].curves[0] {
        LoopEdge::Ellipse { center, .. } => *center,
        _ => Vec2::ZERO,
    };
    assert!(c.near(Vec2::new(-14.0, -6.0), 1e-9), "{c:?}");
    assert!(loops[0].vertices.iter().all(|v| v.p.dist(c) <= 10.0 + 1e-6));
}

#[test]
fn an_associative_hatch_follows_its_ellipse_exactly() {
    let mut s = Session::new();
    let e = ellipse(&mut s);
    s.execute("hatch", &json!({ "handles": [e.hex()], "pattern": "SOLID", "associative": true })).unwrap();
    s.execute("move", &json!({ "handles": [e.hex()], "delta": [3, 4] })).unwrap();
    let loops = hatch_loops(&s);
    match loops[0].curves.as_slice() {
        [LoopEdge::Ellipse { center, .. }] => assert!(center.near(Vec2::new(3.0, 4.0), 1e-9), "{center:?}"),
        other => panic!("{other:?}"),
    }
}

/// A hatch by a point inside lines and one partial ellipse, or a spline: the outer loop keeps the
/// true curve between the lines, and its area is the analytic one (the traced chords' is off by
/// the flattening).
#[test]
fn a_point_inside_lines_and_a_partial_ellipse_keeps_the_ellipse_edge() {
    let mut s = Session::new();
    s.execute("ellipse", &json!({ "center": [0, 0], "major": [5, 0], "ratio": 0.4, "start": 0, "end": 90 })).unwrap();
    s.execute("line", &json!({ "points": [[0, 0], [5, 0]] })).unwrap();
    s.execute("line", &json!({ "points": [[0, 0], [0, 2]] })).unwrap();
    s.execute("hatch", &json!({ "points": [[1.0, 0.5]], "pattern": "SOLID" })).unwrap();
    let loops = hatch_loops(&s);
    assert_eq!(loops.len(), 1);
    let ellipses = loops[0].curves.iter().filter(|c| matches!(c, LoopEdge::Ellipse { .. })).count();
    assert_eq!(ellipses, 1, "{:?}", loops[0].curves);
    assert_eq!(loops[0].curves.len(), 3, "{:?}", loops[0].curves);
    // A quarter of π·5·2.
    assert!((loops[0].area().abs() - PI * 10.0 / 4.0).abs() < 1e-9, "{}", loops[0].area());
    let h = hatch_handle(&s);
    let a = area(&mut s, h);
    assert!((a - PI * 10.0 / 4.0).abs() < 1e-9, "{a}");
}

#[test]
fn a_point_inside_a_line_and_a_spline_keeps_the_spline_edge() {
    let mut s = Session::new();
    // A cubic Bezier from (0,0) to (4,0) and the line back: the area is 9.6.
    s.execute("spline", &json!({ "control": [[0, 0], [0, 4], [4, 4], [4, 0]], "degree": 3 })).unwrap();
    s.execute("line", &json!({ "points": [[4, 0], [0, 0]] })).unwrap();
    s.execute("hatch", &json!({ "points": [[2.0, 1.0]], "pattern": "SOLID" })).unwrap();
    let loops = hatch_loops(&s);
    assert_eq!(loops.len(), 1);
    assert!(loops[0].curves.iter().any(|c| matches!(c, LoopEdge::Spline(_))), "{:?}", loops[0].curves);
    assert!((loops[0].area().abs() - 9.6).abs() < 1e-9, "{}", loops[0].area());
    let h = hatch_handle(&s);
    let a = area(&mut s, h);
    assert!((a - 9.6).abs() < 1e-9, "{a}");
}

/// A spline that crosses an arc and a line: its piece in the boundary ends exactly on both (the
/// chord crossing the tessellated arc would stop up to the flattening tolerance off it).
#[test]
fn a_spline_crossing_an_arc_ends_exactly_on_it() {
    let mut s = Session::new();
    s.execute("arc", &json!({ "center": [0, 0], "radius": 5, "start": 0, "end": 180 })).unwrap();
    s.execute("line", &json!({ "points": [[-5, 0], [5, 0]] })).unwrap();
    s.execute("spline", &json!({ "fit": [[1, -2], [2, 2], [3.5, 6]] })).unwrap();
    s.execute("hatch", &json!({ "points": [[3.5, 1.0]], "pattern": "SOLID" })).unwrap();
    let loops = hatch_loops(&s);
    assert_eq!(loops.len(), 1);
    let curves = &loops[0].curves;
    let sp = curves.iter().find(|c| matches!(c, LoopEdge::Spline(_))).unwrap_or_else(|| panic!("no spline edge: {curves:?}"));
    assert!(curves.iter().any(|c| matches!(c, LoopEdge::Arc { .. })), "{curves:?}");
    let ends = [sp.start_point(), sp.end_point()];
    let on_arc = ends.iter().filter(|p| (p.len() - 5.0).abs() <= 1e-9).count();
    let on_line = ends.iter().filter(|p| p.y.abs() <= 1e-9).count();
    assert_eq!((on_arc, on_line), (1, 1), "{ends:?}");
    // The loop closes up: every edge starts where the one before ends.
    for (a, b) in curves.iter().zip(curves.iter().cycle().skip(1)) {
        assert!(a.end_point().near(b.start_point(), 1e-9), "{a:?} then {b:?}");
    }
    // And encloses what the fine points do.
    let poly = cadaddy_engine::geom::Polyline { vertices: loops[0].vertices.clone(), closed: true }.area();
    assert!((loops[0].area() - poly).abs() < 1e-3 * poly.abs(), "{} vs {poly}", loops[0].area());
}
