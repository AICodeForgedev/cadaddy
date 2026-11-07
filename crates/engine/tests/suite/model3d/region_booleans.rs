//! UNION / SUBTRACT / INTERSECT of regions whose boundaries cross, touch or share edges (gaps
//! F21): exact on lines and arcs (arcs stay arcs), holes kept, results as regions that AREA and
//! MASSPROP measure right away.

use cadaddy_doc::{EntityKind, Handle, Region};
use cadaddy_engine::Session;
use cadaddy_geom::PI;
use serde_json::{Value, json};

fn h(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap()).unwrap()
}

fn region(s: &Session, hd: Handle) -> Region {
    match &s.doc().unwrap().entity(hd).unwrap().kind {
        EntityKind::Region(r) => r.clone(),
        k => panic!("not a region: {k:?}"),
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6 * b.abs().max(1.0)
}

fn rect_region(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Handle {
    let r = h(&s.execute("rectang", &json!({"p1": a, "p2": b})).unwrap());
    s.execute("region", &json!({"handles": [r.hex()]})).unwrap();
    r
}

fn circle_region(s: &mut Session, c: [f64; 2], r: f64) -> Handle {
    let hd = h(&s.execute("circle", &json!({"center": c, "radius": r})).unwrap());
    s.execute("region", &json!({"handles": [hd.hex()]})).unwrap();
    hd
}

/// The area MASSPROP reports for `hd`.
fn massprop_area(s: &mut Session, hd: Handle) -> f64 {
    let r = s.execute("massprop", &json!({"handles": [hd.hex()]})).unwrap();
    r["regions"][0]["area"].as_f64().unwrap()
}

#[test]
fn overlapping_squares_union_subtract_intersect() {
    for (op, want) in [("union", 7.0), ("intersect", 1.0)] {
        let mut s = Session::new();
        let a = rect_region(&mut s, [0.0, 0.0], [2.0, 2.0]);
        let b = rect_region(&mut s, [1.0, 1.0], [3.0, 3.0]);
        s.execute(op, &json!({"handles": [a.hex(), b.hex()]})).unwrap();
        assert!(near(region(&s, a).area(), want), "{op}: {}", region(&s, a).area());
        assert!(near(massprop_area(&mut s, a), want));
        assert_eq!(s.doc().unwrap().model.len(), 1, "{op}: the other region is used up");
        // One undo step.
        s.undo().unwrap();
        assert_eq!(s.doc().unwrap().model.len(), 2);
    }
    let mut s = Session::new();
    let a = rect_region(&mut s, [0.0, 0.0], [2.0, 2.0]);
    let b = rect_region(&mut s, [1.0, 1.0], [3.0, 3.0]);
    s.execute("subtract", &json!({"from": [a.hex()], "remove": [b.hex()]})).unwrap();
    assert!(near(region(&s, a).area(), 3.0));
    // AREA measures it right away.
    let r = s.execute("area", &json!({"handle": a.hex()})).unwrap();
    assert!(near(r["area"].as_f64().unwrap(), 3.0), "{r}");
}

#[test]
fn circles_stay_arcs() {
    let mut s = Session::new();
    let a = circle_region(&mut s, [0.0, 0.0], 1.0);
    let b = circle_region(&mut s, [1.0, 0.0], 1.0);
    s.execute("intersect", &json!({"handles": [a.hex(), b.hex()]})).unwrap();
    let r = region(&s, a);
    assert_eq!(r.loops.len(), 1);
    assert!(r.loops[0].vertices.iter().all(|v| v.bulge != 0.0), "a lens of two arcs: {:?}", r.loops[0].vertices);
    let lens = 2.0 * PI / 3.0 - 3f64.sqrt() / 2.0;
    assert!(near(r.area(), lens), "{} vs {lens}", r.area());
    assert!(near(massprop_area(&mut s, a), lens));
}

#[test]
fn holes_are_kept_and_cut() {
    // A plate with a round hole; a bar unioned across the hole cuts it into two.
    let mut s = Session::new();
    let plate = rect_region(&mut s, [0.0, 0.0], [10.0, 10.0]);
    let hole = circle_region(&mut s, [5.0, 5.0], 2.0);
    s.execute("subtract", &json!({"from": [plate.hex()], "remove": [hole.hex()]})).unwrap();
    assert_eq!(region(&s, plate).loops.len(), 2);
    let bar = rect_region(&mut s, [2.0, 4.5], [8.0, 5.5]);
    s.execute("union", &json!({"handles": [plate.hex(), bar.hex()]})).unwrap();
    let r = region(&s, plate);
    assert_eq!(r.loops.len(), 3, "outer and two half holes");
    assert_eq!(r.loops.iter().filter(|l| l.outer).count(), 1);
    // The plate less the hole, plus the part of the hole the bar fills.
    let hole_in_bar = {
        // Strip |y - 5| < 0.5 of a circle of radius 2.
        let f = |y: f64| y * (4.0 - y * y).sqrt() + 4.0 * (y / 2.0).asin();
        f(0.5) - f(-0.5)
    };
    assert!(near(r.area(), 100.0 - 4.0 * PI + hole_in_bar), "{}", r.area());
}

#[test]
fn shared_edges_and_touching_corners() {
    let mut s = Session::new();
    let a = rect_region(&mut s, [0.0, 0.0], [1.0, 1.0]);
    let b = rect_region(&mut s, [1.0, 0.0], [2.0, 1.0]);
    s.execute("union", &json!({"handles": [a.hex(), b.hex()]})).unwrap();
    let r = region(&s, a);
    assert_eq!(r.loops.len(), 1);
    assert_eq!(r.loops[0].vertices.len(), 4, "no seam where they met");
    // Corners touching: one region of two loops, both outer.
    let mut s = Session::new();
    let a = rect_region(&mut s, [0.0, 0.0], [1.0, 1.0]);
    let b = rect_region(&mut s, [1.0, 1.0], [2.0, 2.0]);
    s.execute("union", &json!({"handles": [a.hex(), b.hex()]})).unwrap();
    let r = region(&s, a);
    assert_eq!(r.loops.len(), 2);
    assert!(r.loops.iter().all(|l| l.outer));
    assert!(near(r.area(), 2.0));
    // Their intersection is empty: the regions go.
    let mut s = Session::new();
    let a = rect_region(&mut s, [0.0, 0.0], [1.0, 1.0]);
    let b = rect_region(&mut s, [1.0, 0.0], [2.0, 1.0]);
    let r = s.execute("intersect", &json!({"handles": [a.hex(), b.hex()]})).unwrap();
    assert!(r["handle"].is_null());
    assert_eq!(s.doc().unwrap().model.len(), 0);
}

#[test]
fn several_regions_fold_in_order() {
    let mut s = Session::new();
    let hs: Vec<Handle> = (0..5).map(|i| rect_region(&mut s, [i as f64 * 1.5, 0.0], [i as f64 * 1.5 + 2.0, 2.0])).collect();
    s.execute("union", &json!({"handles": hs.iter().map(|h| h.hex()).collect::<Vec<_>>()})).unwrap();
    // A strip from 0 to 8 by 2.
    assert!(near(region(&s, hs[0]).area(), 16.0));
    assert_eq!(region(&s, hs[0]).loops.len(), 1);
}
