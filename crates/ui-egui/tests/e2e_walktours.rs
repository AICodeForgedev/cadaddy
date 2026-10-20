//! End to end: Walk on and off through its menu command (the Walk panel shows, then the drawing
//! again), viewpoints and a camera tour saved, and the walkthrough published as a .cdywalk.
//! (MP4 export runs in the headless walk renderer with ffmpeg, not through the control channel.)

mod e2e_common;

use e2e_common::{E2e, folder};
use serde_json::json;

#[test]
fn walk_viewpoints_tour_and_publish() {
    let mut e = E2e::new();
    let dir = folder("walktours");
    e.exec("layer.new", json!({ "name": "A-WALL", "current": true }));
    e.exec("pline", json!({ "vertices": [[0, 0], [240, 0], [240, 120], [0, 120]], "closed": true }));
    e.exec("pline", json!({ "vertices": [[6, 6], [234, 6], [234, 114], [6, 114]], "closed": true }));
    e.exec("saveas", json!({ "path": dir.join("Sample Plan.dxf") }));
    // Walk on: its panel is on screen; off: back to the drawing.
    e.menu("ui.walk");
    assert!(e.shows("Back to the drawing (Esc)"), "the Walk panel");
    e.menu("ui.walk");
    assert!(!e.shows("Back to the drawing (Esc)"));
    // Viewpoints and a tour between them.
    e.exec("tour.view", json!({ "name": "Door", "eye": [2, 2, 5.5], "yaw": 45 }));
    e.exec("tour.view", json!({ "name": "Corner", "eye": [18, 8, 5.5], "yaw": 225 }));
    let t = e.exec("tour.create", json!({ "name": "Project A Tour", "stops": ["Door", "Corner"] }));
    assert!(t["duration"].as_f64().unwrap() > 0.0, "{t}");
    let l = e.exec("tour.list", json!({}));
    assert_eq!((l["views"].as_array().unwrap().len(), l["tours"][0]["name"].as_str()), (2, Some("Project A Tour")));
    // Published as a walkthrough package.
    let out = dir.join("Sample Plan.cdywalk");
    let p = e.exec("walk.publish", json!({ "path": out }));
    assert!(std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0) > 0, "{p}");
    let _ = std::fs::remove_dir_all(&dir);
}
