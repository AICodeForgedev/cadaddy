//! End to end: the Blocks palette and tool palettes used through the interface. A block's tile is
//! clicked in the Blocks palette and placed with a typed point; a circle on a layer becomes a tool
//! (Add Selected), and clicking that tool draws a new circle on the tool's layer.

mod e2e_common;

use e2e_common::E2e;
use serde_json::{Value, json};

/// Click the first control whose label starts with `prefix` (its centre, on screen).
fn click_starting(e: &mut E2e, prefix: &str) -> bool {
    let Some(x) = e.elements().into_iter().find(|x| x["label"].as_str().is_some_and(|l| l.starts_with(prefix))) else { return false };
    let r: Vec<f64> = x["rect"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
    let [x, y, w, h] = r.as_slice() else { return false };
    e.ok("ui.click", json!({ "x": x + w / 2.0, "y": y + h / 2.0, "space": "screen" }));
    true
}

/// Answer the user's own command as the user does: keys typed at the command line, then Enter.
fn keyed(e: &mut E2e, text: &str) {
    e.ok("ui.text", json!({ "text": text }));
    e.ok("ui.key", json!({ "key": "enter" }));
}

fn on_layer(e: &mut E2e, kind: &str, layer: &str) -> usize {
    e.exec("entities", json!({ "type": kind, "layer": layer }))["count"].as_u64().unwrap_or(0) as usize
}

#[test]
fn a_block_tile_is_clicked_and_placed() {
    let mut e = E2e::new();
    e.typed("circle 0,0 2");
    e.exec("selectall", json!({}));
    e.exec("block", json!({ "name": "TAG", "base": [0, 0], "keep": "delete" }));
    e.menu("ui.dialog.blocks");
    assert_eq!(e.dialog().as_deref(), Some("blocks"));
    assert!(
        e.click("TAG"),
        "the TAG tile is shown: {:?}",
        e.elements().iter().filter_map(|x| x["label"].as_str().map(str::to_string)).collect::<Vec<_>>()
    );
    keyed(&mut e, "30,40");
    let r = e.exec("entities", json!({ "type": "Block Reference" }));
    assert_eq!(r["count"], 1, "{r}");
    let at = &r["entities"][0]["geometry"];
    assert!(at.to_string().contains("30") && at.to_string().contains("40"), "placed at 30,40: {at}");
    assert!(e.inked(300, 300) > 0);
}

#[test]
fn a_tool_made_from_a_circle_draws_on_its_layer() {
    let mut e = E2e::new();
    e.exec("layer.new", json!({ "name": "A-FURN", "color": "green", "current": true }));
    e.typed("circle 0,0 3");
    e.exec("layer.current", json!({ "name": "0" }));
    e.exec("selectall", json!({}));
    let r = e.exec("toolpalettes", json!({ "action": "add", "palette": "Project A", "selected": true }));
    assert_eq!(r["added"], 1, "{r}");
    e.exec("select", json!({ "clear": true }));
    e.menu("toolpalettes");
    assert_eq!(e.dialog().as_deref(), Some("toolpalettes"));
    assert!(e.click("Project A"), "the palette's tab");
    assert!(click_starting(&mut e, "CIRCLE"), "the circle tool is shown");
    keyed(&mut e, "50,50");
    keyed(&mut e, "4");
    assert_eq!(on_layer(&mut e, "Circle", "A-FURN"), 2, "the tool drew on its layer: {:?}", e.history());
}
