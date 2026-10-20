//! End to end: CAD standards. A standards drawing (.dws) is associated (STANDARDS), CHECKSTANDARDS
//! reports a standard layer with the wrong colour and a layer the standards don't have, fixes the
//! colour, LAYTRANS moves the stray layer's objects onto the standard one; each dialog opens from
//! its command, and the drawing shows the result.

mod e2e_common;

use e2e_common::{E2e, folder};
use serde_json::{Value, json};

fn layer(e: &mut E2e, name: &str) -> Option<Value> {
    e.drawing()["layers"].as_array().and_then(|ls| ls.iter().find(|l| l["name"] == name).cloned())
}

#[test]
fn a_drawing_is_checked_fixed_and_translated_to_its_standards() {
    let mut e = E2e::new();
    let dir = folder("standards");
    // The office standard: layer A-WALL is red.
    e.exec("layer.new", json!({ "name": "A-WALL", "color": "red" }));
    let dws = dir.join("Office Standard.dws");
    e.exec("saveas", json!({ "path": dws }));

    // A drawing that strays: A-WALL yellow, walls drawn on "old walls".
    e.exec("new", json!({}));
    e.exec("layer.new", json!({ "name": "A-WALL", "color": "yellow" }));
    e.exec("layer.new", json!({ "name": "old walls", "current": true }));
    e.typed("line 0,0 100,0");
    e.typed("");
    let r = e.exec("standards", json!({ "action": "add", "path": dws }));
    assert_eq!(r["files"].as_array().map(Vec::len), Some(1), "{r}");

    let check = e.exec("checkstandards", json!({}));
    let problems = check["problems"].as_array().cloned().unwrap_or_default();
    let colour = problems.iter().find(|p| p["name"] == "A-WALL").unwrap_or_else(|| panic!("A-WALL's colour: {check}"));
    assert!(problems.iter().any(|p| p["name"] == "old walls"), "the stray layer: {check}");
    e.exec("checkstandards", json!({ "fix": [{ "id": colour["id"] }] }));
    assert_eq!(layer(&mut e, "A-WALL").unwrap()["color"], "Red", "fixed from the standards");

    let t = e.exec("laytrans", json!({ "map": [{ "from": "old walls", "to": "A-WALL" }] }));
    assert!(t["translated"].as_u64().unwrap_or(0) >= 1, "{t}");
    let lines = e.exec("entities", json!({ "type": "Line" }));
    let on: Vec<Value> = lines["entities"].as_array().cloned().unwrap_or_default();
    assert!(!on.is_empty() && on.iter().all(|o| o["layer"] == "A-WALL"), "{lines}");
    let after = e.exec("checkstandards", json!({}));
    assert!(!after["problems"].as_array().is_some_and(|p| p.iter().any(|x| x["name"] == "A-WALL" || x["name"] == "old walls")), "{after}");

    // The dialogs, from their commands.
    for d in ["standards", "checkstandards", "laytrans"] {
        e.menu(d);
        assert_eq!(e.dialog().as_deref(), Some(d));
        e.exec("ui.dialog.close", json!({}));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
