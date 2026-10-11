//! TRIM, EXTEND, JOIN and LENGTHEN on elliptical arcs and splines (#466).

use std::f64::consts::{FRAC_PI_2, PI};

use cadcraft_engine::Session;
use cadcraft_engine::doc::{EntityKind, Handle};
use cadcraft_engine::geom::{Ellipse, Spline, Vec2};
use serde_json::{Value, json};

fn h(r: &Value) -> String {
    r["handle"].as_str().or_else(|| r["handles"][0].as_str()).unwrap().to_string()
}

fn kind(s: &Session, h: &str) -> EntityKind {
    s.doc().unwrap().entity(Handle::parse_hex(h).unwrap()).unwrap().kind.clone()
}

fn spline(s: &Session, h: &str) -> Spline {
    match kind(s, h) {
        EntityKind::Spline(sp) => sp,
        k => panic!("not a spline: {k:?}"),
    }
}

fn ellipse(s: &Session, h: &str) -> Ellipse {
    match kind(s, h) {
        EntityKind::Ellipse(e) => Ellipse { center: e.center.xy(), major: e.major.xy(), ratio: e.ratio, start: e.start, end: e.end },
        k => panic!("not an ellipse: {k:?}"),
    }
}

fn ends(sp: &Spline) -> (Vec2, Vec2) {
    let (lo, hi) = sp.domain();
    (sp.eval(lo), sp.eval(hi))
}

fn samples(sp: &Spline, n: usize) -> Vec<Vec2> {
    let (lo, hi) = sp.domain();
    (0..=n).map(|i| sp.eval(lo + (hi - lo) * i as f64 / n as f64)).collect()
}

/// Distance from `p` to the spline (dense sampling, then a ternary search round the best sample).
fn dist_to(sp: &Spline, p: Vec2) -> f64 {
    let (lo, hi) = sp.domain();
    let n = 4000;
    let step = (hi - lo) / n as f64;
    let best = (0..=n).map(|i| lo + step * i as f64).min_by(|a, b| sp.eval(*a).dist(p).total_cmp(&sp.eval(*b).dist(p))).unwrap();
    let (mut a, mut b) = ((best - step).max(lo), (best + step).min(hi));
    for _ in 0..100 {
        let (m1, m2) = (a + (b - a) / 3.0, b - (b - a) / 3.0);
        if sp.eval(m1).dist(p) < sp.eval(m2).dist(p) {
            b = m2;
        } else {
            a = m1;
        }
    }
    sp.eval((a + b) / 2.0).dist(p)
}

fn count(s: &Session) -> usize {
    s.doc().unwrap().model.iter().count()
}

fn length(s: &mut Session, h: &str) -> f64 {
    s.execute("lengthen", &json!({ "handle": h })).unwrap()["length"].as_f64().unwrap()
}

/// An S-shaped open cubic from (0,0) to (9,0); it leaves its end heading along (1,1).
fn s_curve(s: &mut Session) -> String {
    h(&s.execute("spline", &json!({ "control": [[0, 0], [3, 3], [6, -3], [9, 0]] })).unwrap())
}

// ---------------- EXTEND ----------------

#[test]
fn extend_runs_a_spline_on_along_its_end_tangent() {
    let mut s = Session::new();
    let sp = s_curve(&mut s);
    s.execute("line", &json!({ "points": [[12, -10], [12, 20]] })).unwrap();
    let before = spline(&s, &sp);
    s.execute("extend", &json!({ "handle": sp, "pick": [8.9, 0] })).unwrap();
    let after = spline(&s, &sp);
    let (a, b) = ends(&after);
    assert!(a.near(Vec2::ZERO, 1e-9), "{a:?}");
    assert!(b.near(Vec2::new(12.0, 3.0), 1e-9), "{b:?}");
    // The original curve is still there, unchanged.
    for p in samples(&before, 20) {
        assert!(dist_to(&after, p) < 1e-9, "{p:?}");
    }
    // The start end extends backwards along its tangent, (-1,-1) here.
    s.execute("line", &json!({ "points": [[-10, -3], [10, -3]] })).unwrap();
    s.execute("extend", &json!({ "handle": sp, "pick": [0.1, 0.1] })).unwrap();
    assert!(ends(&spline(&s, &sp)).0.near(Vec2::new(-3.0, -3.0), 1e-9));
    // Nothing ahead: an error, the spline unchanged.
    let now = spline(&s, &sp);
    assert!(s.execute("extend", &json!({ "handle": sp, "pick": [11.9, 2.9] })).is_err());
    assert_eq!(spline(&s, &sp), now);
}

#[test]
fn extend_refuses_closed_splines() {
    let mut s = Session::new();
    let sp = h(&s.execute("spline", &json!({ "fit": [[0, 0], [4, 0], [4, 4], [0, 4]], "closed": true })).unwrap());
    s.execute("line", &json!({ "points": [[10, -10], [10, 10]] })).unwrap();
    assert!(s.execute("extend", &json!({ "handle": sp, "pick": [4, 2] })).is_err());
}

// ---------------- TRIM ----------------

#[test]
fn trimming_a_closed_spline_leaves_one_open_spline() {
    for (pick, right) in [([10.0, 5.0], false), ([0.0, 5.0], true)] {
        let mut s = Session::new();
        let sp = h(&s.execute("spline", &json!({ "fit": [[0, 0], [10, 0], [10, 10], [0, 10]], "closed": true })).unwrap());
        s.execute("line", &json!({ "points": [[5, -5], [5, 15]] })).unwrap();
        let r = s.execute("trim", &json!({ "handle": sp, "pick": pick })).unwrap();
        let hs = r["handles"].as_array().unwrap();
        assert_eq!(hs.len(), 1, "{pick:?}: {r}");
        let piece = spline(&s, hs[0].as_str().unwrap());
        assert!(!piece.closed);
        let (a, b) = ends(&piece);
        assert!((a.x - 5.0).abs() < 1e-6 && (b.x - 5.0).abs() < 1e-6, "{a:?} {b:?}");
        // Only the half away from the pick is left.
        for p in samples(&piece, 100) {
            if right {
                assert!(p.x > 5.0 - 1e-6, "{pick:?}: {p:?}");
            } else {
                assert!(p.x < 5.0 + 1e-6, "{pick:?}: {p:?}");
            }
        }
    }
}

// ---------------- LENGTHEN ----------------

/// A quarter of the ellipse x²/16 + y²/4 = 1, from (4,0) to (0,2).
fn quarter_ellipse(s: &mut Session) -> String {
    h(&s.execute("ellipse", &json!({ "center": [0, 0], "major": [4, 0], "ratio": 0.5, "start": 0, "end": 90 })).unwrap())
}

#[test]
fn lengthen_elliptical_arcs_along_the_ellipse() {
    let mut s = Session::new();
    let e = quarter_ellipse(&mut s);
    let l0 = length(&mut s, &e);
    assert!((l0 - ellipse(&s, &e).length()).abs() < 1e-9);
    // DElta at the end nearest the pick (the (0,2) end); the start stays.
    s.execute("lengthen", &json!({ "handle": e, "pick": [0, 2], "delta": 1.0 })).unwrap();
    assert!((length(&mut s, &e) - (l0 + 1.0)).abs() < 1e-6);
    assert!(ellipse(&s, &e).start.abs() < 1e-12);
    // Total, Percent; picking the start end moves the start.
    s.execute("lengthen", &json!({ "handle": e, "pick": [0, 2], "total": l0 })).unwrap();
    assert!((ellipse(&s, &e).end - FRAC_PI_2).abs() < 1e-6);
    s.execute("lengthen", &json!({ "handle": e, "pick": [4, 0], "percent": 200 })).unwrap();
    assert!((length(&mut s, &e) - 2.0 * l0).abs() < 1e-6);
    assert!((ellipse(&s, &e).end - FRAC_PI_2).abs() < 1e-6);
    assert!((ellipse(&s, &e).start - 1.5 * PI).abs() < 1e-6, "{}", ellipse(&s, &e).start);
    // DYnamic / To: the end follows the point round the ellipse.
    s.execute("lengthen", &json!({ "handle": e, "pick": [0, 2], "to": [-4, 0] })).unwrap();
    assert!((ellipse(&s, &e).end - PI).abs() < 1e-9);
    // Too long (a full ellipse), too short, angle options: errors, the arc unchanged.
    let before = ellipse(&s, &e);
    for p in [json!({ "total": 1000.0 }), json!({ "delta": -1000.0 }), json!({ "deltaAngle": 10 }), json!({ "percent": f64::MIN_POSITIVE })] {
        let mut q = p.clone();
        q["handle"] = json!(e);
        q["pick"] = json!([0, 2]);
        assert!(s.execute("lengthen", &q).is_err(), "{q}");
        assert_eq!(ellipse(&s, &e), before);
    }
}

#[test]
fn lengthen_open_splines() {
    let mut s = Session::new();
    let sp = s_curve(&mut s);
    let before = spline(&s, &sp);
    let l0 = length(&mut s, &sp);
    // Longer: runs on along the end tangent.
    s.execute("lengthen", &json!({ "handle": sp, "pick": [9, 0], "delta": 2f64.sqrt() })).unwrap();
    assert!(ends(&spline(&s, &sp)).1.near(Vec2::new(10.0, 1.0), 1e-9));
    assert!((length(&mut s, &sp) - l0 - 2f64.sqrt()).abs() < 1e-6);
    // Shorter from the start: the far end stays put.
    s.execute("lengthen", &json!({ "handle": sp, "pick": [0, 0], "total": 5.0 })).unwrap();
    let now = spline(&s, &sp);
    assert!((length(&mut s, &sp) - 5.0).abs() < 1e-6);
    assert!(ends(&now).1.near(Vec2::new(10.0, 1.0), 1e-9));
    assert!(dist_to(&before, ends(&now).0) < 1e-9, "the new start lies on the old curve");
    // To a point on the curve: cut there.
    let mut s = Session::new();
    let sp = s_curve(&mut s);
    let mid = before.eval(0.5);
    s.execute("lengthen", &json!({ "handle": sp, "pick": [9, 0], "to": [mid.x, mid.y] })).unwrap();
    assert!(ends(&spline(&s, &sp)).1.near(mid, 1e-6));
    // Closed splines and nonsense lengths are refused.
    let c = h(&s.execute("spline", &json!({ "fit": [[0, 0], [4, 0], [4, 4]], "closed": true })).unwrap());
    assert!(s.execute("lengthen", &json!({ "handle": c, "delta": 1 })).is_err());
    assert!(s.execute("lengthen", &json!({ "handle": sp, "pick": [9, 0], "delta": -1e9 })).is_err());
}

#[test]
fn lengthen_at_the_command_line_on_an_elliptical_arc() {
    let mut s = Session::new();
    let e = quarter_ellipse(&mut s);
    let l0 = ellipse(&s, &e).length();
    s.script("LENGTHEN DE 1 0,2\n\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert!((ellipse(&s, &e).length() - l0 - 1.0).abs() < 1e-6);
}

// ---------------- JOIN ----------------

fn arc_on_ellipse(s: &mut Session, start: f64, end: f64) -> String {
    h(&s.execute("ellipse", &json!({ "center": [1, 1], "major": [4, 0], "ratio": 0.5, "start": start, "end": end })).unwrap())
}

#[test]
fn join_merges_elliptical_arcs_on_one_ellipse() {
    // Touching arcs: one larger arc.
    let mut s = Session::new();
    let a = arc_on_ellipse(&mut s, 0.0, 90.0);
    let b = arc_on_ellipse(&mut s, 90.0, 180.0);
    let r = s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    assert_eq!(h(&r), a, "the source arc is kept");
    let m = ellipse(&s, &a);
    assert!(m.start.abs() < 1e-9 && (m.end - PI).abs() < 1e-9, "{m:?}");
    assert_eq!(count(&s), 1);
    // A gap closes; the widest gap stays open.
    let mut s = Session::new();
    let a = arc_on_ellipse(&mut s, 0.0, 90.0);
    let b = arc_on_ellipse(&mut s, 120.0, 270.0);
    s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    let m = ellipse(&s, &a);
    assert!(m.start.abs() < 1e-9 && (m.end - 1.5 * PI).abs() < 1e-9, "{m:?}");
    // Two halves: the full ellipse. The same ellipse with its major axis the other way round
    // (parameters shifted by half a turn) counts.
    let mut s = Session::new();
    let a = arc_on_ellipse(&mut s, 0.0, 180.0);
    let b = h(&s.execute("ellipse", &json!({ "center": [1, 1], "major": [-4, 0], "ratio": 0.5, "start": 0, "end": 180 })).unwrap());
    s.execute("join", &json!({ "handles": [a, b] })).unwrap();
    assert!(ellipse(&s, &a).is_full());
    assert_eq!(count(&s), 1);
}

#[test]
fn join_makes_one_spline_from_lines_arcs_polylines_and_splines() {
    let mut s = Session::new();
    let sp = s_curve(&mut s);
    let before = spline(&s, &sp);
    let line = h(&s.execute("line", &json!({ "points": [[-5, 0], [0, 0]] })).unwrap());
    // An arc leaving (9,0) and a polyline (with an arc segment) after it, picked in any order.
    let arc = h(&s.execute("arc", &json!({ "center": [9, 3], "radius": 3, "start": 270, "end": 0 })).unwrap());
    let pl = h(&s.execute("pline", &json!({ "vertices": [[12, 3], { "p": [12, 6], "bulge": 0.5 }, [14, 6]] })).unwrap());
    let far = h(&s.execute("line", &json!({ "points": [[50, 50], [60, 60]] })).unwrap());
    s.execute("join", &json!({ "handles": [line, pl, sp, arc, far] })).unwrap();
    let j = spline(&s, &line);
    let (a, b) = ends(&j);
    assert!(a.near(Vec2::new(-5.0, 0.0), 1e-9), "{a:?}");
    assert!(b.near(Vec2::new(14.0, 6.0), 1e-9), "{b:?}");
    // Every joined piece lies on the result, exactly for the arc.
    for p in samples(&before, 20) {
        assert!(dist_to(&j, p) < 1e-9, "{p:?}");
    }
    for k in 0..=8 {
        let t = -FRAC_PI_2 + FRAC_PI_2 * k as f64 / 8.0;
        assert!(dist_to(&j, Vec2::new(9.0 + 3.0 * t.cos(), 3.0 + 3.0 * t.sin())) < 1e-9);
    }
    // The unconnected line stays; the others are consumed.
    assert_eq!(count(&s), 2);
    assert!(matches!(kind(&s, &far), EntityKind::Line(_)));
    assert!(s.doc().unwrap().entity(Handle::parse_hex(&sp).unwrap()).is_none());
}

#[test]
fn join_at_the_command_line_names_the_result() {
    let mut s = Session::new();
    s_curve(&mut s);
    s.execute("line", &json!({ "points": [[9, 0], [12, 0]] })).unwrap();
    s.script("JOIN ALL\n\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert!(s.log.iter().any(|l| l.contains("2 objects joined into 1 spline")), "{:?}", s.log);
    assert!(matches!(s.doc().unwrap().model.last().unwrap().kind, EntityKind::Spline(_)));
    assert_eq!(count(&s), 1);
}

#[test]
fn join_refuses_unconnected_curves() {
    let mut s = Session::new();
    let sp = s_curve(&mut s);
    let l = h(&s.execute("line", &json!({ "points": [[20, 0], [30, 0]] })).unwrap());
    assert!(s.execute("join", &json!({ "handles": [sp, l] })).is_err());
    assert_eq!(count(&s), 2);
    // Arcs on different ellipses that do not touch.
    let a = arc_on_ellipse(&mut s, 0.0, 90.0);
    let b = h(&s.execute("ellipse", &json!({ "center": [40, 40], "major": [4, 0], "ratio": 0.5, "start": 0, "end": 90 })).unwrap());
    assert!(s.execute("join", &json!({ "handles": [a, b] })).is_err());
}

#[test]
fn hostile_curve_edits_do_not_panic() {
    let mut s = Session::new();
    let sp = s_curve(&mut s);
    let e = quarter_ellipse(&mut s);
    // A degenerate spline (all control points equal) and a huge-degree one.
    let d = h(&s.execute("spline", &json!({ "control": [[1, 1], [1, 1], [1, 1], [1, 1]] })).unwrap());
    s.execute("line", &json!({ "points": [[0, -50], [0, 50]] })).unwrap();
    for target in [&sp, &e, &d] {
        for p in [
            json!({ "delta": 1e308 }),
            json!({ "delta": f64::MAX }),
            json!({ "total": 1e-300 }),
            json!({ "percent": 1e300 }),
            json!({ "to": [1e308, -1e308] }),
            json!({ "to": [0, 0] }),
        ] {
            let mut q = p.clone();
            q["handle"] = json!(target);
            let _ = s.execute("lengthen", &q);
        }
        let _ = s.execute("extend", &json!({ "handle": target, "pick": [1e308, 1e308] }));
        let _ = s.execute("trim", &json!({ "handle": target, "pick": [-1e308, 0] }));
        let _ = s.execute("join", &json!({ "handles": [target, sp, e, d] }));
    }
}
