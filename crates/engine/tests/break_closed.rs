//! BREAK on closed polylines (RECTANG, closed PLINE, with arcs), ellipses and splines, typed and
//! JSON; BREAKATPOINT opening a closed outline; and an object that can't be broken ends the
//! command with its message instead of re-asking the second point.

use cadaddy_engine::doc::{EntityKind, LwPolyline};
use cadaddy_engine::geom::{Polyline, Vec2};
use cadaddy_engine::{Input, Session};
use serde_json::{Value, json};

fn h(r: &Value) -> String {
    r["handle"].as_str().or_else(|| r["handles"][0].as_str()).unwrap().to_string()
}

fn polys(s: &Session) -> Vec<LwPolyline> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::LwPolyline(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

fn length(p: &LwPolyline) -> f64 {
    Polyline { vertices: p.vertices.clone(), closed: p.closed }.segments().iter().map(|s| s.len()).sum()
}

fn ends(p: &LwPolyline) -> (Vec2, Vec2) {
    (p.vertices.first().unwrap().p, p.vertices.last().unwrap().p)
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

fn near2(a: Vec2, x: f64, y: f64) -> bool {
    near(a.x, x) && near(a.y, y)
}

/// The rectangle of the battle test: a 100 × 100 square, bottom edge from 1100,0 to 1200,0.
fn rect(s: &mut Session) -> String {
    h(&s.execute("rectang", &json!({"p1": [1100, 0], "p2": [1200, 100]})).unwrap())
}

/// One open polyline left, `len` long, running from `a` to `b`.
fn assert_open(s: &Session, len: f64, a: (f64, f64), b: (f64, f64)) {
    let ps = polys(s);
    assert_eq!(ps.len(), 1, "{ps:?}");
    let p = &ps[0];
    assert!(!p.closed);
    assert!(near(length(p), len), "length {} want {len}", length(p));
    let (pa, pb) = ends(p);
    assert!(near2(pa, a.0, a.1) && near2(pb, b.0, b.1), "ends {pa:?} {pb:?}");
}

#[test]
fn json_break_opens_a_rectangle_in_both_point_orders() {
    // First 1150,0 then 1170,0: the counter-clockwise part between them (20 long) goes.
    let mut s = Session::new();
    let r = rect(&mut s);
    s.execute("break", &json!({"handle": r, "p1": [1150, 0], "p2": [1170, 0]})).unwrap();
    assert_open(&s, 380.0, (1170.0, 0.0), (1150.0, 0.0));
    // Swapped: counter-clockwise from 1170,0 round to 1150,0 is the long way; 20 is left.
    let mut s = Session::new();
    let r = rect(&mut s);
    s.execute("break", &json!({"handle": r, "p1": [1170, 0], "p2": [1150, 0]})).unwrap();
    assert_open(&s, 20.0, (1150.0, 0.0), (1170.0, 0.0));
}

#[test]
fn json_break_on_a_clockwise_outline_removes_the_counter_clockwise_part() {
    // Vertices in clockwise order: the bottom edge runs from 10,0 to 0,0.
    let mut s = Session::new();
    let p = h(&s.execute("pline", &json!({"vertices": [[0, 0], [0, 10], [10, 10], [10, 0]], "closed": true})).unwrap());
    s.execute("break", &json!({"handle": p, "p1": [2, 0], "p2": [6, 0]})).unwrap();
    let ps = polys(&s);
    assert_eq!(ps.len(), 1);
    assert!(near(length(&ps[0]), 36.0), "{}", length(&ps[0]));
    let (a, b) = ends(&ps[0]);
    // Runs in the polyline's own direction: from the first point round to the second.
    assert!(near2(a, 2.0, 0.0) && near2(b, 6.0, 0.0), "{a:?} {b:?}");
}

#[test]
fn json_break_on_a_closed_polyline_with_an_arc() {
    // A slot: bottom edge, a half circle round 10,5 (radius 5) on the right, top edge back.
    let mut s = Session::new();
    let p = h(&s.execute("pline", &json!({"vertices": [[0, 0], {"p": [10, 0], "bulge": 1.0}, [10, 10], [0, 10]], "closed": true})).unwrap());
    let total = 10.0 + 5.0 * std::f64::consts::PI + 10.0 + 10.0;
    let q = Vec2::new(10.0 + 5.0 * std::f64::consts::FRAC_1_SQRT_2, 5.0 + 5.0 * std::f64::consts::FRAC_1_SQRT_2);
    s.execute("break", &json!({"handle": p, "p1": [15, 5], "p2": [q.x, q.y]})).unwrap();
    let ps = polys(&s);
    assert_eq!(ps.len(), 1);
    // A quarter of the half circle (45°) is gone; both ends are on the arc, still arcs.
    assert!(near(length(&ps[0]), total - 5.0 * std::f64::consts::FRAC_PI_4), "{}", length(&ps[0]));
    let (a, b) = ends(&ps[0]);
    assert!(near2(a, q.x, q.y) && near2(b, 15.0, 5.0), "{a:?} {b:?}");
    assert!(ps[0].vertices.first().unwrap().bulge.abs() > 1e-6, "{:?}", ps[0].vertices);
    assert!(ps[0].vertices.iter().rev().nth(1).unwrap().bulge.abs() > 1e-6, "{:?}", ps[0].vertices);
}

#[test]
fn breakatpoint_opens_a_closed_polyline() {
    let mut s = Session::new();
    let r = rect(&mut s);
    s.execute("breakatpoint", &json!({"handle": r, "at": [1150, 0]})).unwrap();
    assert_open(&s, 400.0, (1150.0, 0.0), (1150.0, 0.0));
    // At a corner (the start vertex) too.
    let mut s = Session::new();
    let r = rect(&mut s);
    s.execute("breakatpoint", &json!({"handle": r, "at": [1100, 0]})).unwrap();
    assert_open(&s, 400.0, (1100.0, 0.0), (1100.0, 0.0));
}

#[test]
fn typed_break_opens_a_rectangle_and_ends() {
    let mut s = Session::new();
    s.cmdline("RECTANG 1100,0 1200,100").unwrap();
    s.cmdline("BREAK 1150,0 1170,0").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_open(&s, 380.0, (1170.0, 0.0), (1150.0, 0.0));
}

#[test]
fn typed_break_first_point_option_on_a_rectangle() {
    let mut s = Session::new();
    s.cmdline("RECTANG 1100,0 1200,100").unwrap();
    // Pick the right edge, then F: the break runs from 1150,0 to 1170,0.
    s.cmdline("BREAK 1200,50 F 1150,0 1170,0").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_open(&s, 380.0, (1170.0, 0.0), (1150.0, 0.0));
}

#[test]
fn typed_break_at_one_point_with_at_sign_opens_a_closed_polyline() {
    let mut s = Session::new();
    s.cmdline("PLINE 0,0 10,0 10,10 0,10 C").unwrap();
    // `@` for the second point: the same point as the first.
    s.cmdline("BREAK 5,0 @").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_open(&s, 40.0, (5.0, 0.0), (5.0, 0.0));
    // BREAKATPOINT typed.
    let mut s = Session::new();
    s.cmdline("PLINE 0,0 10,0 10,10 0,10 C").unwrap();
    s.cmdline("BREAKATPOINT 10,5 10,5").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_open(&s, 40.0, (10.0, 5.0), (10.0, 5.0));
}

#[test]
fn an_object_that_cannot_be_broken_ends_the_command() {
    let mut s = Session::new();
    s.execute("text", &json!({"at": [0, 0], "text": "NOTE", "height": 2.5})).unwrap();
    let before = s.doc().unwrap().model.len();
    s.start("break").unwrap();
    s.input(Input::Point(Vec2::new(0.2, 0.2))).unwrap();
    assert!(s.running.is_some(), "the text was not picked");
    s.input(Input::Point(Vec2::new(1.0, 0.2))).unwrap();
    assert!(s.running.is_none(), "still asking: {}", s.prompt_text());
    assert!(s.log.iter().rev().take(3).any(|l| l.contains("Cannot break")), "{:?}", s.log.iter().rev().take(3).collect::<Vec<_>>());
    // What's typed next is a new command, not a second break point.
    s.cmdline("LINE 0,10 5,10").unwrap();
    s.cmdline("").unwrap();
    assert_eq!(s.doc().unwrap().model.len(), before + 1);

    // A locked closed polyline: refused, and the command ends too.
    let mut s = Session::new();
    s.cmdline("RECTANG 0,0 10,10").unwrap();
    s.execute("layer.set", &json!({"name": "0", "locked": true})).unwrap();
    s.cmdline("BREAK 5,0 7,0").unwrap();
    assert!(s.running.is_none(), "still asking: {}", s.prompt_text());
    assert!(s.log.iter().rev().take(3).any(|l| l.contains("locked layer")));
    assert_eq!(polys(&s).len(), 1);
    assert!(polys(&s)[0].closed);
}

#[test]
fn breakatpoint_on_an_object_that_cannot_be_broken_ends() {
    let mut s = Session::new();
    s.cmdline("CIRCLE 0,0 5").unwrap();
    s.cmdline("BREAKATPOINT 5,0 5,0").unwrap();
    assert!(s.running.is_none(), "still asking: {}", s.prompt_text());
    assert!(s.log.iter().rev().take(3).any(|l| l.contains("360")), "{:?}", s.log.last());
}

#[test]
fn the_other_targets_still_break() {
    let mut s = Session::new();
    let l = h(&s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap());
    assert_eq!(s.execute("break", &json!({"handle": l, "p1": [7, 0], "p2": [3, 0]})).unwrap()["handles"].as_array().unwrap().len(), 2);
    let a = h(&s.execute("arc", &json!({"center": [0, 20], "radius": 5, "start": 0, "end": 180})).unwrap());
    assert_eq!(s.execute("break", &json!({"handle": a, "p1": [5, 20], "p2": [0, 25]})).unwrap()["handles"].as_array().unwrap().len(), 1);
    let c = h(&s.execute("circle", &json!({"center": [0, 40], "radius": 5})).unwrap());
    assert_eq!(s.execute("break", &json!({"handle": c, "p1": [5, 40], "p2": [0, 45]})).unwrap()["handles"].as_array().unwrap().len(), 1);
    let p = h(&s.execute("pline", &json!({"vertices": [[0, 60], [10, 60], [10, 70]]})).unwrap());
    assert_eq!(s.execute("break", &json!({"handle": p, "p1": [4, 60], "p2": [10, 65]})).unwrap()["handles"].as_array().unwrap().len(), 2);
    let e = h(&s.execute("ellipse", &json!({"center": [0, 80], "major": [10, 0], "ratio": 0.5})).unwrap());
    let r = s.execute("break", &json!({"handle": e, "p1": [10, 80], "p2": [0, 85]})).unwrap();
    let eh = cadaddy_engine::doc::Handle::parse_hex(r["handles"][0].as_str().unwrap()).unwrap();
    match &s.doc().unwrap().entity(eh).unwrap().kind {
        // A quarter (0 → 90°) is gone: the rest runs from 90° round to 0.
        EntityKind::Ellipse(el) => assert!(near(el.start, std::f64::consts::FRAC_PI_2) && near(el.end, 0.0), "{el:?}"),
        k => panic!("{k:?}"),
    }
    let sp = h(&s.execute("spline", &json!({"fit": [[0, 100], [5, 104], [10, 100], [15, 104]]})).unwrap());
    assert_eq!(s.execute("break", &json!({"handle": sp, "p1": [5, 104], "p2": [10, 100]})).unwrap()["handles"].as_array().unwrap().len(), 2);
}

#[test]
fn a_closed_spline_opens_into_one_spline() {
    let mut s = Session::new();
    let sp = h(&s.execute("spline", &json!({"fit": [[0, 0], [10, 0], [10, 10], [0, 10]], "closed": true})).unwrap());
    let r = s.execute("break", &json!({"handle": sp, "p1": [10, 5], "p2": [5, 10.5]})).unwrap();
    let hs = r["handles"].as_array().unwrap();
    assert_eq!(hs.len(), 1);
    let nh = cadaddy_engine::doc::Handle::parse_hex(hs[0].as_str().unwrap()).unwrap();
    let EntityKind::Spline(o) = &s.doc().unwrap().entity(nh).unwrap().kind else { panic!() };
    assert!(!o.closed);
    // Counter-clockwise from the right side to the top is the short way: it is gone, so the
    // open spline runs from the top round the left and bottom to the right side.
    let (lo, hi) = o.domain();
    let (a, b) = (o.eval(lo), o.eval(hi));
    assert!(a.y > 9.0 && b.x > 9.0, "{a:?} {b:?}");
    let mid = o.eval((lo + hi) / 2.0);
    assert!(mid.x < 5.0 && mid.y < 5.0, "{mid:?}");
}
