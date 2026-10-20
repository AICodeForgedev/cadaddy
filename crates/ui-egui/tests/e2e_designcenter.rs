//! End to end: DesignCenter. A library drawing (a block and a layer) is browsed from another
//! drawing: DESIGNCENTER opens from its menu, its folder and contents are listed, and the block is
//! inserted and the layer brought in — both then in the drawing and on screen.

mod e2e_common;

use e2e_common::{E2e, folder};
use serde_json::json;

#[test]
fn a_block_and_a_layer_come_from_another_drawing() {
    let mut e = E2e::new();
    let dir = folder("designcenter");
    // The library: block CHAIR (a circle and a line) and layer A-FURN.
    e.exec("layer.new", json!({ "name": "A-FURN", "color": "green" }));
    e.typed("circle 0,0 9");
    e.typed("line -9,0 9,0");
    e.typed("");
    e.exec("selectall", json!({}));
    e.exec("block", json!({ "name": "CHAIR", "base": [0, 0], "keep": "delete" }));
    let lib = dir.join("Sample Library.dwg");
    e.exec("saveas", json!({ "path": lib }));

    // A new drawing, then DesignCenter from its menu.
    e.exec("new", json!({}));
    assert!(e.exec("blocks.list", json!({})).to_string().find("CHAIR").is_none(), "a fresh drawing");
    e.menu("adcenter");
    assert_eq!(e.dialog().as_deref(), Some("adcenter"));
    let f = e.exec("adcenter", json!({ "action": "folder", "path": dir }));
    assert!(f["drawings"].to_string().contains("Sample Library.dwg"), "{f}");
    let l = e.exec("adcenter", json!({ "path": lib }));
    assert!(l["blocks"].as_array().unwrap().iter().any(|b| b["name"] == "CHAIR"), "{l}");
    assert!(l["layers"].to_string().contains("A-FURN"), "{l}");

    let before = e.entity_count();
    let r = e.exec("adcenter", json!({ "action": "import", "path": lib, "kind": "blocks", "names": ["CHAIR"], "at": [50, 50] }));
    assert!(r["handle"].is_string(), "{r}");
    e.exec("adcenter", json!({ "action": "import", "path": lib, "kind": "layers", "names": ["A-FURN"] }));
    assert_eq!(e.entity_count(), before + 1, "the reference is placed");
    let d = e.drawing();
    assert!(d["layers"].as_array().unwrap().iter().any(|x| x["name"] == "A-FURN"), "{}", d["layers"]);
    assert!(d["blocks"].as_array().unwrap().iter().any(|b| b == "CHAIR"));
    e.exec("zoom", json!({ "mode": "extents" }));
    assert!(e.inked(300, 300) > 0, "the chair is drawn");
    let _ = std::fs::remove_dir_all(&dir);
}
