//! A frame of the BIM view with the Properties and BIM (Coordination) palettes open doesn't parse
//! the building spec (Q6481): the small settings the screen reads out of it every frame (section
//! box, coordination mode, BIM look, units) are read once per spec text. On a large job the spec
//! is tens of megabytes and each whole parse cost most of a frame.

use std::time::Instant;

use crate::e2e_common::E2e;
use cadaddy_engine::doc::{Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// A made-up 20' × 10' room of 6" walls on A-WALL, built; its spec padded with `rooms` more
/// rooms (a big job's spec). One wall's handle.
fn built(e: &mut E2e, rooms: usize) -> Handle {
    {
        let d = e.app.session.doc_mut().unwrap();
        let o = [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
        let i = [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
        d.ensure_layer("A-WALL");
        for ((ax, ay), (bx, by)) in o.into_iter().chain(i) {
            let p = |x: f64, y: f64| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
            let h = d.new_handle();
            let mut l = Entity::new(h, EntityKind::Line(Line { a: p(ax, ay), b: p(bx, by) }));
            l.common.layer = "A-WALL".into();
            d.model.push(l);
        }
    }
    let rules = json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "kind": "wall"}]
    });
    e.exec("wallgen", json!({ "rules": rules, "auto": true }));
    e.exec("coordination.sectionbox", json!({ "on": true }));
    e.exec("coordination.mode", json!({ "on": true }));
    {
        let d = e.app.session.doc_mut().unwrap();
        let mut spec: Value = d.building_spec.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or_else(|| json!({}));
        let list: Vec<Value> = (0..rooms).map(|i| json!({"name": format!("Room {i:06}"), "ceiling_ft": 8 + i % 3, "level": "L1"})).collect();
        spec["rooms"] = Value::Array(list);
        d.building_spec = Some(spec.to_string()).into();
    }
    let walls = e.exec("bim.props.find", json!({ "kind": "wall", "limit": 1 }));
    Handle::parse_hex(walls["handles"][0].as_str().unwrap()).unwrap()
}

/// The BIM view in 3D, one wall selected, the Properties and Coordination palettes open in
/// windows of their own; then the spec parses and the time of `frames` more frames.
fn frames(rooms: usize, frames: u32) -> (f64, u64, f64) {
    let mut e = E2e::new();
    let w = built(&mut e, rooms);
    let mb = e.app.session.doc().unwrap().building_spec.as_deref().map_or(0, str::len) as f64 / 1e6;
    e.exec("view.dir", json!({ "dir": [-1, -1.5, 1], "fit": true }));
    e.exec("vscurrent", json!({ "style": "shaded" }));
    // The architectural view draws the walls (the default MEP view hides them, and what the
    // view hides leaves the selection, Q6419).
    e.exec("bim.view", json!({ "view": "architectural" }));
    e.app.session.set_selection(vec![w]);
    e.app.run("ui.dock", json!({ "operation": "detach", "panel": "properties", "rect": [40, 80, 380, 560] })).unwrap();
    e.app.run("ui.dock", json!({ "operation": "detach", "panel": "coordination", "rect": [440, 80, 380, 560] })).unwrap();
    for _ in 0..10 {
        e.frame();
    }
    assert!(e.shows("Kind: wall"), "the Properties palette shows the wall's BIM properties");
    let before = cadaddy_engine::specread::whole_parses();
    let t = Instant::now();
    for _ in 0..frames {
        e.frame();
    }
    let ms = t.elapsed().as_secs_f64() * 1e3 / f64::from(frames);
    (mb, cadaddy_engine::specread::whole_parses() - before, ms)
}

#[test]
fn bim_view_and_palette_frames_dont_parse_the_building_spec() {
    let (small_mb, small_parses, small_ms) = frames(0, 30);
    let (big_mb, big_parses, big_ms) = frames(100_000, 30);
    println!("spec {small_mb:.2} MB: {small_ms:.1} ms a frame; spec {big_mb:.1} MB: {big_ms:.1} ms a frame");
    assert_eq!((small_parses, big_parses), (0, 0), "frames after the first parse no building spec");
}
