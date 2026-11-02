//! BOUNDARY with associative arrays of a small hatch in the drawing: the hatch's pattern lines
//! (inside the array's block references) are never boundary candidates, so a pick neither pays
//! for drawing them nor traces along them.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn handle(r: &Value) -> Value {
    r["handle"].clone()
}

/// A hatched rectangle, an associative polar array of it (its items reach all around the
/// origin), `copies` copies of the array, and a far-off 30 x 20 rectangle.
fn fixture(copies: usize) -> Session {
    let mut s = Session::new();
    s.execute("rectang", &json!({"p1": [985, 106], "p2": [1015, 126]})).unwrap();
    let h = handle(&s.execute("hatch", &json!({"points": [[1000, 116]], "pattern": "ANSI37", "associative": false})).unwrap());
    let a = handle(&s.execute("arraypolar", &json!({"handles": [h], "center": [0, 0], "count": 4, "angle": 360, "associative": true})).unwrap());
    for i in 0..copies {
        s.execute("copy", &json!({"handles": [a], "delta": [i as f64 * 7.0, i as f64 * 5.0]})).unwrap();
    }
    s.execute("rectang", &json!({"p1": [-391.6, -310.6], "p2": [-361.6, -290.6]})).unwrap();
    s
}

fn areas(s: &mut Session, r: &Value) -> Vec<f64> {
    let mut v: Vec<f64> =
        r["handles"].as_array().unwrap().iter().map(|h| s.execute("area", &json!({"handle": h})).unwrap()["area"].as_f64().unwrap()).collect();
    v.sort_by(f64::total_cmp);
    v
}

#[test]
fn boundary_beside_array_copies_of_a_hatch_is_fast_and_unchanged() {
    let mut s = fixture(9);
    let t = std::time::Instant::now();
    let r = s.execute("boundary", &json!({"points": [[-376.6, -300.6]]})).unwrap();
    // Was 6 s on a laptop (27 s here): every array's box spans the pick, and each was drawn
    // with its pattern lines.
    assert!(t.elapsed().as_secs_f64() < 0.5, "{:?}", t.elapsed());
    let a = areas(&mut s, &r);
    assert_eq!(a.len(), 1);
    assert!((a[0] - 600.0).abs() < 1e-6, "{a:?}");
}

#[test]
fn pattern_lines_inside_arrays_are_not_boundary_candidates() {
    // A rectangle around the whole drawing: the outer loop and the two real rectangles as
    // islands, nothing from the pattern lines.
    let mut with = fixture(2);
    with.execute("rectang", &json!({"p1": [-1200, -1200], "p2": [1200, 1200]})).unwrap();
    let r = with.execute("boundary", &json!({"points": [[-1100, 1100]]})).unwrap();
    let a = areas(&mut with, &r);
    assert_eq!(a.len(), 3, "{a:?}");
    assert!((a[0] - 600.0).abs() < 1e-6 && (a[1] - 600.0).abs() < 1e-6, "{a:?}");
    assert!((a[2] - 2400.0 * 2400.0).abs() < 1e-3, "{a:?}");
}
