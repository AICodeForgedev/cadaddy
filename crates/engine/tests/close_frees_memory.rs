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

#[test]
fn closing_a_drawing_drops_its_images_only() {
    use cadaddy_engine::doc::{EntityKind, Image};
    use cadaddy_render::images;
    let pixels = || images::Raster::new(1, 1, vec![255, 0, 0, 255]).unwrap();
    let mut s = Session::new();
    // Registered pixels with no file behind them: once dropped they can't come back.
    let (mine, theirs) = ("close-test-kept.png", "close-test-dropped.png");
    images::insert(mine, pixels());
    images::insert(theirs, pixels());
    s.add_entity(EntityKind::Image(Image { path: mine.into(), ..Default::default() })).unwrap();
    s.execute("new", &json!({})).unwrap();
    s.add_entity(EntityKind::Image(Image { path: theirs.into(), ..Default::default() })).unwrap();
    s.execute("close", &json!({})).unwrap();
    assert!(images::get(mine).is_some(), "an open drawing's image stays");
    assert!(images::get(theirs).is_none(), "a closed drawing's image is let go");
}
