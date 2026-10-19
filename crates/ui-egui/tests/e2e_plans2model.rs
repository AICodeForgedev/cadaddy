//! End to end: Plans to Model as a person does it — a plan drawn, the palette opened from its
//! menu, the spec drafted from the plan, every part approved, the model built (3D walls and
//! rooms in the drawing), then the plan changed and the model revised.

mod e2e_common;

use e2e_common::{E2e, folder};
use serde_json::{Value, json};

fn walls(e: &mut E2e) -> u64 {
    e.exec("entities", json!({ "layer": "3D-Walls", "limit": 0 }))["count"].as_u64().unwrap_or(0)
}

#[test]
fn draft_approve_build_revise() {
    let mut e = E2e::new();
    let p = |x: f64, y: f64| json!([x * 12.0, y * 12.0]);
    let rect = |a: (f64, f64), b: (f64, f64)| json!({ "vertices": [p(a.0, a.1), p(b.0, a.1), p(b.0, b.1), p(a.0, b.1)], "closed": true });
    e.exec("layer.new", json!({ "name": "A-WALL", "current": true }));
    e.exec("pline", rect((0.0, 0.0), (30.0, 12.0)));
    e.exec("pline", rect((0.5, 0.5), (29.5, 11.5)));
    e.exec("layer.new", json!({ "name": "A-ROOM-TAG", "current": true }));
    e.exec("text", json!({ "at": p(10.0, 6.0), "text": "OFFICE", "height": 6 }));
    // The palette from its menu.
    e.menu("ui.panel.planstomodel");
    assert!(e.ok("ui.inspect", json!({}))["ui"]["docking"].to_string().contains("plansToModel") || e.shows("Draft from Plan"), "the palette is open");
    // Draft, approve all.
    let d = e.exec("bspec.draft", json!({ "store": true }));
    assert!(d["stored"] == true && d["parts"].as_u64().unwrap_or(1) >= 1, "{d}");
    let parts = e.exec("bspec.get", json!({}))["parts"].as_array().cloned().unwrap_or_default();
    assert!(!parts.is_empty());
    let approve_all = e.click("Approve All");
    let got = e.exec("bspec.get", json!({}));
    let approved = got["parts"].as_array().unwrap().iter().filter(|p| p["approved"] == true).count();
    assert!(approve_all && approved == parts.len(), "Approve All in the palette: {approved}/{}", parts.len());
    // Build: 3D walls in the drawing and a model in the render.
    let b = e.exec("model.build", json!({}));
    assert!(b["walls"].as_u64().unwrap_or(0) >= 4, "{b}");
    let n = walls(&mut e);
    assert!(n >= 4, "3D walls: {n}");
    // Revise: a partition added to the plan; the model follows it (auto), and REVISE against the
    // plan as it was before lists the partition as added.
    let dir = folder("plans2model");
    let before = dir.join("Sample Plan v1.dxf");
    e.exec("saveas", json!({ "path": before }));
    e.exec("layer.current", json!({ "name": "A-WALL" }));
    e.exec("pline", rect((15.0, 0.5), (15.4, 11.5)));
    assert!(walls(&mut e) > n, "the new partition is built");
    let r: Value = e.exec("model.revise", json!({ "against": before }));
    assert!(r["added"].as_array().is_some_and(|a| !a.is_empty()), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}
