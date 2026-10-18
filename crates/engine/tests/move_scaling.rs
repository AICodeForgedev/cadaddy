//! Moving every object of a large drawing scales with the drawing (perf1): each object is found
//! by handle without walking the store.

use cadaddy_engine::Session;
use cadaddy_engine::doc::EntityKind;
use cadaddy_engine::geom::Vec3;
use serde_json::json;

#[test]
fn select_all_and_move_scales_to_a_hundred_thousand_objects() {
    let mut s = Session::new();
    let n = 100_000;
    for i in 0..n {
        let x = (i % 1000) as f64;
        let y = (i / 1000) as f64;
        s.add_entity(EntityKind::Line(cadaddy_engine::doc::Line { a: Vec3::new(x, y, 0.0), b: Vec3::new(x + 0.5, y, 0.0) })).unwrap();
    }
    let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    let t = std::time::Instant::now();
    s.execute("move", &json!({ "handles": hs, "delta": [2, 3] })).unwrap();
    let took = t.elapsed();
    let d = s.doc().unwrap();
    assert!(d.model.iter().enumerate().all(|(i, e)| matches!(&e.kind, EntityKind::Line(l)
        if l.a.x == (i % 1000) as f64 + 2.0 && l.a.y == (i / 1000) as f64 + 3.0)));
    assert!(took.as_secs_f64() < 20.0, "move took {took:?}");
    // Undo puts every one back.
    s.undo().unwrap();
    assert!(s.doc().unwrap().model.iter().enumerate().all(|(i, e)| matches!(&e.kind, EntityKind::Line(l) if l.a.x == (i % 1000) as f64)));
}
