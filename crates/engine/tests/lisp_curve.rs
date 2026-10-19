//! The `vlax-curve-` functions on each curve type against values worked out by hand: points,
//! parameters as AutoCAD defines them per type, distances, derivatives, areas, closest points
//! and the closed/periodic/planar tests.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{EntityCodec, set_entity_codec};
use serde_json::{Value, json};
use std::f64::consts::PI;

/// A session whose LISP can make objects from group lists (`entmake`).
fn session() -> Session {
    set_entity_codec(EntityCodec {
        groups: cadaddy_io::entity_groups,
        entity: cadaddy_io::entity_from_groups,
        records: cadaddy_io::entity_records,
        attrib: cadaddy_io::attrib_from_groups,
    });
    Session::new()
}

fn eval(s: &mut Session, code: &str) -> Result<String, String> {
    let r: Value = s.execute("lisp", &json!({ "code": code })).map_err(|e| e.to_string())?;
    match r.get("error").and_then(Value::as_str) {
        Some(e) => Err(e.to_string()),
        None => Ok(r["value"].as_str().unwrap_or_default().to_string()),
    }
}

fn ok(s: &mut Session, code: &str) -> String {
    eval(s, code).unwrap_or_else(|e| panic!("{code}: {e}"))
}

/// Make an object with `entmake` and keep its name in `e`.
fn make(s: &mut Session, list: &str) {
    let made = ok(s, &format!("(entmake '({list}))"));
    assert_ne!(made, "nil", "entmake {list}");
    ok(s, "(setq e (entlast))");
}

/// A real as the LISP reader reads it back exactly.
fn lisp_real(x: f64) -> String {
    format!("{x:e}")
}

#[track_caller]
fn assert_num(s: &mut Session, code: &str, want: f64, tol: f64) {
    let same = ok(s, &format!("(equal {code} {} {})", lisp_real(want), lisp_real(tol)));
    assert_eq!(same, "T", "{code}: {}, wanted {want}", ok(s, code));
}

#[track_caller]
fn assert_pt(s: &mut Session, code: &str, want: [f64; 3], tol: f64) {
    let w = format!("'({} {} {})", lisp_real(want[0]), lisp_real(want[1]), lisp_real(want[2]));
    let same = ok(s, &format!("(equal {code} {w} {})", lisp_real(tol)));
    assert_eq!(same, "T", "{code}: {}, wanted {want:?}", ok(s, code));
}

#[test]
fn line_parameters_are_distances() {
    let mut s = session();
    make(&mut s, r#"(0 . "LINE") (10 0.0 0.0 0.0) (11 3.0 4.0 0.0)"#);
    assert_num(&mut s, "(vlax-curve-getStartParam e)", 0.0, 1e-12);
    assert_num(&mut s, "(vlax-curve-getEndParam e)", 5.0, 1e-12);
    assert_pt(&mut s, "(vlax-curve-getStartPoint e)", [0.0, 0.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getEndPoint e)", [3.0, 4.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getPointAtParam e 2.5)", [1.5, 2.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getPointAtDist e 2.5)", [1.5, 2.0, 0.0], 1e-12);
    assert_num(&mut s, "(vlax-curve-getDistAtParam e 2)", 2.0, 1e-12);
    assert_num(&mut s, "(vlax-curve-getParamAtDist e 2)", 2.0, 1e-12);
    assert_num(&mut s, "(vlax-curve-getDistAtPoint e '(3 4))", 5.0, 1e-9);
    assert_num(&mut s, "(vlax-curve-getParamAtPoint e '(1.5 2 0))", 2.5, 1e-9);
    assert_eq!(ok(&mut s, "(vlax-curve-getParamAtPoint e '(1 1))"), "nil", "not on the line");
    assert_pt(&mut s, "(vlax-curve-getFirstDeriv e 1)", [0.6, 0.8, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getSecondDeriv e 1)", [0.0, 0.0, 0.0], 1e-12);
    assert_num(&mut s, "(vlax-curve-getArea e)", 0.0, 1e-12);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(0 5))", [2.4, 3.2, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(6 8))", [3.0, 4.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(6 8) T)", [6.0, 8.0, 0.0], 1e-12);
    assert_eq!(ok(&mut s, "(vlax-curve-getPointAtParam e 6)"), "nil", "past the end");
    assert_eq!(ok(&mut s, "(vlax-curve-getPointAtDist e -1)"), "nil");
    assert_eq!(ok(&mut s, "(vlax-curve-isClosed e)"), "nil");
    assert_eq!(ok(&mut s, "(vlax-curve-isPeriodic e)"), "nil");
    assert_eq!(ok(&mut s, "(vlax-curve-isPlanar e)"), "T");
    // A 3D line seen from above: the projection meets (5,3) in plan at (5,0).
    make(&mut s, r#"(0 . "LINE") (10 0.0 0.0 0.0) (11 10.0 0.0 10.0)"#);
    assert_pt(&mut s, "(vlax-curve-getClosestPointToProjection e '(5 3 0) '(0 0 1))", [5.0, 0.0, 5.0], 1e-9);
    assert_num(&mut s, "(vlax-curve-getEndParam e)", 200f64.sqrt(), 1e-12);
}

#[test]
fn arc_and_circle_parameters_are_angles() {
    let mut s = session();
    make(&mut s, r#"(0 . "ARC") (10 0.0 0.0 0.0) (40 . 2.0) (50 . 0.0) (51 . 1.5707963267948966)"#);
    assert_num(&mut s, "(vlax-curve-getStartParam e)", 0.0, 1e-12);
    assert_num(&mut s, "(vlax-curve-getEndParam e)", PI / 2.0, 1e-12);
    assert_pt(&mut s, "(vlax-curve-getEndPoint e)", [0.0, 2.0, 0.0], 1e-12);
    assert_num(&mut s, "(vlax-curve-getDistAtParam e (vlax-curve-getEndParam e))", PI, 1e-12);
    assert_num(&mut s, "(vlax-curve-getParamAtDist e 1)", 0.5, 1e-12);
    assert_pt(&mut s, "(vlax-curve-getFirstDeriv e 0)", [0.0, 2.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getSecondDeriv e 0)", [-2.0, 0.0, 0.0], 1e-12);
    assert_num(&mut s, "(vlax-curve-getArea e)", PI - 2.0, 1e-12);
    assert_num(&mut s, "(vlax-curve-getParamAtPoint e '(0 2))", PI / 2.0, 1e-9);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(-3 0))", [0.0, 2.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(-3 0) T)", [-2.0, 0.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(3 3 7))", [2f64.sqrt(), 2f64.sqrt(), 0.0], 1e-12);
    // An arc across 0°: from 270° to 90° (group lists hold angles in radians).
    make(&mut s, r#"(0 . "ARC") (10 0.0 0.0 0.0) (40 . 1.0) (50 . 4.71238898038469) (51 . 1.5707963267948966)"#);
    assert_num(&mut s, "(- (vlax-curve-getEndParam e) (vlax-curve-getStartParam e))", PI, 1e-12);
    assert_pt(&mut s, "(vlax-curve-getPointAtDist e (/ pi 2))", [1.0, 0.0, 0.0], 1e-12);

    make(&mut s, r#"(0 . "CIRCLE") (10 1.0 1.0 0.0) (40 . 1.0)"#);
    assert_num(&mut s, "(vlax-curve-getEndParam e)", 2.0 * PI, 1e-12);
    assert_pt(&mut s, "(vlax-curve-getStartPoint e)", [2.0, 1.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getEndPoint e)", [2.0, 1.0, 0.0], 1e-12);
    assert_num(&mut s, "(vlax-curve-getArea e)", PI, 1e-12);
    assert_num(&mut s, "(vlax-curve-getDistAtParam e pi)", PI, 1e-12);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(1 5))", [1.0, 2.0, 0.0], 1e-12);
    assert_eq!(ok(&mut s, "(vlax-curve-isClosed e)"), "T");
    assert_eq!(ok(&mut s, "(vlax-curve-isPeriodic e)"), "T");
}

#[test]
fn ellipse_parameters_are_parametric_angles() {
    let mut s = session();
    make(&mut s, r#"(0 . "ELLIPSE") (10 0.0 0.0 0.0) (11 4.0 0.0 0.0) (40 . 0.5) (41 . 0.0) (42 . 6.283185307179586)"#);
    assert_num(&mut s, "(vlax-curve-getEndParam e)", 2.0 * PI, 1e-9);
    assert_pt(&mut s, "(vlax-curve-getPointAtParam e (/ pi 2))", [0.0, 2.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getFirstDeriv e 0)", [0.0, 2.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getSecondDeriv e 0)", [-4.0, 0.0, 0.0], 1e-12);
    assert_num(&mut s, "(vlax-curve-getArea e)", 8.0 * PI, 1e-9);
    // The perimeter (Ramanujan's second approximation is good to ~1e-7 here).
    let (a, b) = (4.0f64, 2.0f64);
    let h = ((a - b) / (a + b)).powi(2);
    let perimeter = PI * (a + b) * (1.0 + 3.0 * h / (10.0 + (4.0 - 3.0 * h).sqrt()));
    assert_num(&mut s, "(vlax-curve-getDistAtParam e (vlax-curve-getEndParam e))", perimeter, 1e-5);
    // A quarter of the way round by length is at a quarter of the perimeter.
    assert_num(&mut s, "(vlax-curve-getDistAtParam e (vlax-curve-getParamAtDist e 5.0))", 5.0, 1e-7);
    assert_num(&mut s, "(vlax-curve-getParamAtPoint e '(0 -2))", 1.5 * PI, 1e-6);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(0 7))", [0.0, 2.0, 0.0], 1e-6);
    assert_eq!(ok(&mut s, "(vlax-curve-isClosed e)"), "T");
    assert_eq!(ok(&mut s, "(vlax-curve-isPeriodic e)"), "T");
    // Half an ellipse: the area to its chord.
    make(&mut s, r#"(0 . "ELLIPSE") (10 0.0 0.0 0.0) (11 4.0 0.0 0.0) (40 . 0.5) (41 . 0.0) (42 . 3.141592653589793)"#);
    assert_num(&mut s, "(vlax-curve-getArea e)", 4.0 * PI, 1e-9);
    assert_eq!(ok(&mut s, "(vlax-curve-isClosed e)"), "nil");
}

#[test]
fn polyline_parameters_count_vertices() {
    let mut s = session();
    // A half circle below (0,0)–(10,0), then up to (10,10).
    make(
        &mut s,
        r#"(0 . "LWPOLYLINE") (100 . "AcDbEntity") (100 . "AcDbPolyline") (90 . 3) (70 . 0) (10 0.0 0.0) (42 . 1.0) (10 10.0 0.0) (10 10.0 10.0)"#,
    );
    assert_num(&mut s, "(vlax-curve-getEndParam e)", 2.0, 1e-12);
    assert_pt(&mut s, "(vlax-curve-getPointAtParam e 0.5)", [5.0, -5.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getPointAtParam e 1.5)", [10.0, 5.0, 0.0], 1e-12);
    assert_num(&mut s, "(vlax-curve-getDistAtParam e 1)", 5.0 * PI, 1e-12);
    assert_num(&mut s, "(vlax-curve-getDistAtParam e 2)", 5.0 * PI + 10.0, 1e-12);
    assert_num(&mut s, "(vlax-curve-getParamAtDist e (+ (* 5 pi) 5))", 1.5, 1e-12);
    assert_pt(&mut s, "(vlax-curve-getFirstDeriv e 1.5)", [0.0, 10.0, 0.0], 1e-12);
    // On the arc: speed = sweep × radius.
    assert_pt(&mut s, "(vlax-curve-getFirstDeriv e 0.5)", [5.0 * PI, 0.0, 0.0], 1e-9);
    // The triangle plus the half disc below its base.
    assert_num(&mut s, "(vlax-curve-getArea e)", 50.0 + 12.5 * PI, 1e-9);
    assert_num(&mut s, "(vlax-curve-getParamAtPoint e '(5 -5))", 0.5, 1e-9);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(5 -9))", [5.0, -5.0, 0.0], 1e-9);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(13 20))", [10.0, 10.0, 0.0], 1e-9);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(13 20) T)", [10.0, 20.0, 0.0], 1e-9);
    assert_eq!(ok(&mut s, "(vlax-curve-isClosed e)"), "nil");
    // Closed: one more segment back to the start.
    make(
        &mut s,
        r#"(0 . "LWPOLYLINE") (100 . "AcDbEntity") (100 . "AcDbPolyline") (90 . 4) (70 . 1) (38 . 2.0) (10 0.0 0.0) (10 4.0 0.0) (10 4.0 3.0) (10 0.0 3.0)"#,
    );
    assert_num(&mut s, "(vlax-curve-getEndParam e)", 4.0, 1e-12);
    assert_pt(&mut s, "(vlax-curve-getEndPoint e)", [0.0, 0.0, 2.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getPointAtParam e 3.5)", [0.0, 1.5, 2.0], 1e-12);
    assert_num(&mut s, "(vlax-curve-getArea e)", 12.0, 1e-12);
    assert_num(&mut s, "(vlax-curve-getDistAtParam e 4)", 14.0, 1e-12);
    assert_eq!(ok(&mut s, "(vlax-curve-isClosed e)"), "T");
    assert_eq!(ok(&mut s, "(vlax-curve-isPeriodic e)"), "nil");

    // A 3D polyline.
    s.execute("3dpoly", &json!({"points": [[0, 0, 0], [10, 0, 0], [10, 10, 5]]})).unwrap();
    ok(&mut s, "(setq e (entlast))");
    assert_num(&mut s, "(vlax-curve-getEndParam e)", 2.0, 1e-12);
    assert_pt(&mut s, "(vlax-curve-getPointAtParam e 1.5)", [10.0, 5.0, 2.5], 1e-12);
    assert_num(&mut s, "(vlax-curve-getDistAtParam e 2)", 10.0 + 125f64.sqrt(), 1e-12);
    assert_eq!(ok(&mut s, "(vlax-curve-isPlanar e)"), "T");
    s.execute("3dpoly", &json!({"points": [[0, 0, 0], [10, 0, 0], [10, 10, 5], [0, 10, 0]]})).unwrap();
    ok(&mut s, "(setq e (entlast))");
    assert_eq!(ok(&mut s, "(vlax-curve-isPlanar e)"), "nil");
}

#[test]
fn spline_parameters_are_knots() {
    let mut s = session();
    // A cubic Bézier: (0,0) (1,2) (3,2) (4,0).
    make(
        &mut s,
        r#"(0 . "SPLINE") (100 . "AcDbEntity") (100 . "AcDbSpline") (70 . 8) (71 . 3) (72 . 8) (73 . 4) (40 . 0.0) (40 . 0.0) (40 . 0.0) (40 . 0.0) (40 . 1.0) (40 . 1.0) (40 . 1.0) (40 . 1.0) (10 0.0 0.0 0.0) (10 1.0 2.0 0.0) (10 3.0 2.0 0.0) (10 4.0 0.0 0.0)"#,
    );
    assert_num(&mut s, "(vlax-curve-getStartParam e)", 0.0, 1e-12);
    assert_num(&mut s, "(vlax-curve-getEndParam e)", 1.0, 1e-12);
    assert_pt(&mut s, "(vlax-curve-getPointAtParam e 0.5)", [2.0, 1.5, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getFirstDeriv e 0.5)", [4.5, 0.0, 0.0], 1e-6);
    assert_pt(&mut s, "(vlax-curve-getFirstDeriv e 0)", [3.0, 6.0, 0.0], 1e-6);
    assert_pt(&mut s, "(vlax-curve-getSecondDeriv e 0.5)", [0.0, -12.0, 0.0], 1e-4);
    // Its length by a fine polyline.
    let mut len = 0.0;
    let bez = |t: f64| {
        let m = 1.0 - t;
        (3.0 * m * m * t + 9.0 * m * t * t + 4.0 * t * t * t, 6.0 * m * m * t + 6.0 * m * t * t)
    };
    let mut prev = bez(0.0);
    for k in 1..=100_000 {
        let p = bez(f64::from(k) / 100_000.0);
        len += ((p.0 - prev.0).powi(2) + (p.1 - prev.1).powi(2)).sqrt();
        prev = p;
    }
    assert_num(&mut s, "(vlax-curve-getDistAtParam e 1)", len, 1e-6);
    assert_num(&mut s, &format!("(vlax-curve-getParamAtDist e {})", len / 2.0), 0.5, 1e-6);
    assert_num(&mut s, "(vlax-curve-getParamAtPoint e '(2 1.5))", 0.5, 1e-6);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(2 5))", [2.0, 1.5, 0.0], 1e-6);
    assert_eq!(ok(&mut s, "(vlax-curve-isClosed e)"), "nil");
    assert_eq!(ok(&mut s, "(vlax-curve-isPeriodic e)"), "nil");
    // Closed by its chord (the X axis): ∫ y dx = 4.2 for this Bézier.
    assert_num(&mut s, "(vlax-curve-getArea e)", 4.2, 1e-5);
}

#[test]
fn rays_and_xlines_run_to_infinity() {
    let mut s = session();
    make(&mut s, r#"(0 . "RAY") (100 . "AcDbEntity") (100 . "AcDbRay") (10 1.0 1.0 0.0) (11 1.0 0.0 0.0)"#);
    assert_num(&mut s, "(vlax-curve-getStartParam e)", 0.0, 1e-12);
    assert_num(&mut s, "(vlax-curve-getEndParam e)", 1e99, 1e84);
    assert_pt(&mut s, "(vlax-curve-getStartPoint e)", [1.0, 1.0, 0.0], 1e-12);
    assert_eq!(ok(&mut s, "(vlax-curve-getEndPoint e)"), "nil");
    assert_pt(&mut s, "(vlax-curve-getPointAtParam e 3)", [4.0, 1.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(-5 3))", [1.0, 1.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(-5 3) T)", [-5.0, 1.0, 0.0], 1e-12);
    assert_eq!(ok(&mut s, "(vlax-curve-getPointAtParam e -1)"), "nil");
    make(&mut s, r#"(0 . "XLINE") (100 . "AcDbEntity") (100 . "AcDbXline") (10 0.0 0.0 0.0) (11 0.0 1.0 0.0)"#);
    assert_num(&mut s, "(vlax-curve-getStartParam e)", -1e99, 1e84);
    assert_eq!(ok(&mut s, "(vlax-curve-getStartPoint e)"), "nil");
    assert_pt(&mut s, "(vlax-curve-getPointAtParam e -2)", [0.0, -2.0, 0.0], 1e-12);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(3 -7))", [0.0, -7.0, 0.0], 1e-12);
    assert_num(&mut s, "(vlax-curve-getArea e)", 0.0, 1e-12);
}

#[test]
fn bad_arguments_are_errors_not_crashes() {
    let mut s = session();
    let fails = |s: &mut Session, code: &str, want: &str| match eval(s, code) {
        Ok(v) => panic!("{code} gave {v}"),
        Err(e) => assert!(e.contains(want), "{code}: {e}"),
    };
    fails(&mut s, "(vlax-curve-getStartPoint \"x\")", "lentityp");
    fails(&mut s, "(vlax-curve-getStartPoint)", "too few arguments");
    make(&mut s, r#"(0 . "TEXT") (10 0.0 0.0 0.0) (40 . 1.0) (1 . "hi")"#);
    fails(&mut s, "(vlax-curve-getStartPoint e)", "not a curve");
    make(&mut s, r#"(0 . "LINE") (10 0.0 0.0 0.0) (11 1.0 0.0 0.0)"#);
    fails(&mut s, "(vlax-curve-getPointAtParam e \"a\")", "numberp");
    fails(&mut s, "(vlax-curve-getClosestPointTo e 5)", "point");
    fails(&mut s, "(vlax-curve-getClosestPointToProjection e '(0 0) '(0 0 0))", "no length");
    assert_eq!(ok(&mut s, "(vlax-curve-getPointAtParam e 1e400)"), "nil");
    // A degenerate line and a zero-radius circle answer without crashing.
    make(&mut s, r#"(0 . "LINE") (10 2.0 2.0 0.0) (11 2.0 2.0 0.0)"#);
    assert_pt(&mut s, "(vlax-curve-getClosestPointTo e '(5 5))", [2.0, 2.0, 0.0], 1e-12);
    assert_eq!(ok(&mut s, "(vlax-curve-getParamAtDist e 1)"), "nil");
}
