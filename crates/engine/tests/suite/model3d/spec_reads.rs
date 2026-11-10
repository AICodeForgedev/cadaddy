//! The small settings the screen reads out of the building spec every frame (section box,
//! coordination mode, BIM look, fidelity rules, units) are read once per spec text, not parsed out
//! of the whole spec on each read (Q6481): on a large job the spec is tens of megabytes.

use std::time::Instant;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::{bimlook, coordination, fidelity};
use serde_json::{Value, json};

/// A building whose spec runs to megabytes: `rooms` rooms on one level, each with its own
/// finishes, the section box and coordination mode on.
fn big_building(rooms: usize) -> Session {
    let mut s = Session::new();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9}],
            "rooms": [{"name": "Office", "ceiling_ft": 8}],
        }}),
    )
    .unwrap();
    s.execute("coordination.sectionbox", &json!({"on": true})).unwrap();
    s.execute("coordination.mode", &json!({"on": true})).unwrap();
    let mut spec: Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    let list: Vec<Value> =
        (0..rooms).map(|i| json!({"name": format!("Room {i:06}"), "number": format!("{i}"), "ceiling_ft": 8 + i % 3, "level": "Level 1"})).collect();
    spec["rooms"] = Value::Array(list);
    s.doc_mut().unwrap().building_spec = Some(spec.to_string()).into();
    s
}

/// What one frame of the 3D view, the BIM view and the Properties palette read of the spec.
fn frame(s: &Session) -> (bool, bool, f64) {
    let d = s.doc().unwrap();
    let crs = coordination::coord_render_state(d);
    let look = bimlook::Look::of(d);
    let upf = fidelity::units_per_foot(d);
    let _ = fidelity::Rules::of(d);
    let _ = fidelity::plan_rules(d);
    (crs.sbox_on && crs.mode_on, look.off, upf)
}

#[test]
fn a_frame_parses_the_building_spec_once_per_spec_text() {
    let s = big_building(150_000);
    let mb = s.doc().unwrap().building_spec.as_deref().map_or(0, str::len) as f64 / 1e6;
    let t = Instant::now();
    let first = frame(&s);
    let first_ms = t.elapsed().as_secs_f64() * 1e3;
    assert_eq!(first, (true, false, 12.0), "the settings read as the spec has them");
    let before = cadaddy_engine::specread::whole_parses();
    const FRAMES: u32 = 20;
    let t = Instant::now();
    for _ in 0..FRAMES {
        assert_eq!(frame(&s), first);
    }
    let per = t.elapsed().as_secs_f64() * 1e3 / f64::from(FRAMES);
    println!("spec {mb:.1} MB: first frame {first_ms:.1} ms, then {per:.3} ms a frame");
    assert_eq!(cadaddy_engine::specread::whole_parses() - before, 0, "no frame after the first parses the spec");

    // An edit that changes the spec is seen on the next frame.
    let mut s = s;
    s.execute("coordination.sectionbox", &json!({"on": false})).unwrap();
    assert_eq!(frame(&s).0, false, "the section box is off after the edit");
    // An edit that leaves the spec alone doesn't read it again.
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    let before = cadaddy_engine::specread::whole_parses();
    let _ = frame(&s);
    assert_eq!(cadaddy_engine::specread::whole_parses() - before, 0, "a line drawn doesn't change the spec");
}
