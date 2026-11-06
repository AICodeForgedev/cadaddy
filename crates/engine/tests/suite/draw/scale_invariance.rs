//! A drawing in millimetres, in miles, tiny, or far from the origin edits the same way: the same
//! session — TRIM, EXTEND (to a circle and to an infinite line), FILLET, CHAMFER, BREAK, OFFSET
//! (a polyline with an arc, an ellipse, a spline), JOIN and LENGTHEN — at scales from 1e-7 to 1e7
//! and a billion units from the origin gives the same objects, scaled. (Tolerances are relative:
//! JOIN used an absolute 1e-6, which closed a polyline in a tiny drawing.)

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// Every number of every object (in the order the objects were made), the session run at
/// `scale`, moved by `shift`.
fn session(scale: f64, shift: f64) -> Vec<(String, Vec<f64>)> {
    let p = |x: f64, y: f64| json!([x * scale + shift, y * scale + shift]);
    let len = |v: f64| v * scale;
    let mut s = Session::new();
    let mut run = |id: &str, v: Value| -> String {
        let r = s.execute(id, &v).unwrap_or_else(|e| panic!("{id} at scale {scale} shift {shift}: {e}"));
        r["handle"].as_str().or_else(|| r["handles"][0].as_str()).unwrap_or_default().to_string()
    };
    let a = run("line", json!({"points": [p(0.0, 0.0), p(10.0, 0.0)]}));
    run("line", json!({"points": [p(5.0, -5.0), p(5.0, 5.0)]}));
    run("trim", json!({"handle": a, "pick": p(8.0, 0.0)}));
    run("circle", json!({"center": p(30.0, 0.0), "radius": len(3.0)}));
    let e = run("line", json!({"points": [p(15.0, 0.2), p(20.0, 0.2)]}));
    run("extend", json!({"handle": e, "pick": p(19.0, 0.2)}));
    run("xline", json!({"base": p(80.0, -50.0), "angle": 90}));
    let x = run("line", json!({"points": [p(70.0, 3.0), p(75.0, 3.0)]}));
    run("extend", json!({"handle": x, "pick": p(74.0, 3.0)}));
    let f1 = run("line", json!({"points": [p(0.0, 20.0), p(10.0, 20.0)]}));
    let f2 = run("line", json!({"points": [p(12.0, 22.0), p(12.0, 30.0)]}));
    run("fillet", json!({"h1": f1, "p1": p(5.0, 20.0), "h2": f2, "p2": p(12.0, 25.0), "radius": len(1.0)}));
    let c1 = run("line", json!({"points": [p(20.0, 20.0), p(30.0, 20.0)]}));
    let c2 = run("line", json!({"points": [p(30.0, 20.0), p(30.0, 30.0)]}));
    run("chamfer", json!({"h1": c1, "p1": p(25.0, 20.0), "h2": c2, "p2": p(30.0, 25.0), "d1": len(1.0), "d2": len(2.0)}));
    let br = run("circle", json!({"center": p(50.0, 0.0), "radius": len(4.0)}));
    run("break", json!({"handle": br, "p1": p(54.0, 0.0), "p2": p(50.0, 4.0)}));
    let pl = run("pline", json!({"vertices": [p(0.0, 40.0), {"p": p(10.0, 40.0), "bulge": 1.0}, p(10.0, 46.0), p(0.0, 46.0)]}));
    run("offset", json!({"handle": pl, "distance": len(0.5), "side": p(5.0, 43.0)}));
    let el = run("ellipse", json!({"center": p(20.0, 60.0), "major": [len(6.0), len(2.0)], "ratio": 0.4}));
    run("offset", json!({"handle": el, "distance": len(0.5), "side": p(20.0, 60.0)}));
    let sp = run("spline", json!({"fit": [p(40.0, 60.0), p(44.0, 64.0), p(48.0, 58.0), p(52.0, 63.0)]}));
    run("offset", json!({"handle": sp, "distance": len(0.3), "side": p(46.0, 66.0)}));
    let j1 = run("line", json!({"points": [p(40.0, 40.0), p(45.0, 40.0)]}));
    let j2 = run("line", json!({"points": [p(45.0, 40.0), p(50.0, 40.0)]}));
    run("join", json!({"handles": [j1, j2]}));
    let l = run("line", json!({"points": [p(60.0, 0.0), p(65.0, 0.0)]}));
    run("lengthen", json!({"handle": l, "pick": p(65.0, 0.0), "delta": len(2.0)}));
    s.doc()
        .unwrap()
        .model
        .iter()
        .map(|e| {
            let v = serde_json::to_value(&e.kind).unwrap();
            let mut nums = Vec::new();
            numbers(&v, &mut nums);
            (v["type"].as_str().unwrap_or_default().to_string(), nums)
        })
        .collect()
}

fn numbers(v: &Value, out: &mut Vec<f64>) {
    match v {
        Value::Number(n) => out.push(n.as_f64().unwrap_or_default()),
        Value::Array(a) => a.iter().for_each(|x| numbers(x, out)),
        Value::Object(m) => m.values().for_each(|x| numbers(x, out)),
        _ => {}
    }
}

#[test]
fn huge_tiny_and_far_drawings_edit_the_same() {
    let base = session(1.0, 0.0);
    assert!(base.len() >= 15 && base.iter().any(|(k, _)| k == "spline"));
    for (scale, shift) in [(1e-7, 0.0), (1e-4, 0.0), (1e-3, 1e5), (1e4, 0.0), (1e7, 0.0), (1.0, 1e6), (1.0, 1e9), (1e3, 1e8)] {
        let got = session(scale, shift);
        assert_eq!(got.len(), base.len(), "objects at scale {scale} shift {shift}");
        for ((gk, g), (bk, b)) in got.iter().zip(&base) {
            assert_eq!((gk, g.len()), (bk, b.len()), "at scale {scale} shift {shift}");
            for (x, y) in g.iter().zip(b) {
                // A coordinate (scaled and moved), a length (scaled), or an angle, ratio or bulge (as
                // it was): one of them maps it back.
                let tol = 1e-6 * (1.0 + y.abs()) + 1e-12 * shift / scale;
                let back = [(x - shift) / scale, x / scale, *x];
                assert!(back.iter().any(|m| (m - y).abs() <= tol), "{gk}: {x} at scale {scale} shift {shift}, expected {y}");
            }
        }
    }
}
