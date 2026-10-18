//! Closing a drawing lets go of it: nothing process-wide keeps its objects alive.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Space;
use cadaddy_engine::geom::Vec2;
use serde_json::json;

fn big_session() -> Session {
    let mut s = Session::new();
    for i in 0..(cadaddy_engine::spatial::MIN_INDEXED + 50) {
        let x = i as f64;
        s.execute("line", &json!({"points": [[x, 0], [x, 5]]})).unwrap();
    }
    s
}

fn first_entity(s: &Session) -> std::sync::Weak<cadaddy_engine::doc::Entity> {
    let store = s.doc().unwrap().space(&Space::Model).unwrap();
    std::sync::Arc::downgrade(store.iter().next().unwrap())
}

#[test]
fn closing_a_picked_drawing_frees_it() {
    for all in [false, true] {
        let mut s = big_session();
        // A pick builds and caches the selection index of model space.
        let picked = cadaddy_engine::select::pick(s.doc().unwrap(), &Space::Model, Vec2::new(0.0, 2.0), 0.1);
        assert!(picked.is_some());
        let weak = first_entity(&s);
        s.execute(if all { "closeall" } else { "close" }, &json!({})).unwrap();
        assert!(weak.upgrade().is_none(), "a closed drawing's objects are still held (closeall: {all})");
    }
}
