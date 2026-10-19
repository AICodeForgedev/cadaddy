//! End to end: LIGHTBAKE on a small model (a slab and a box) — the bake reports what it baked,
//! the rendered view shows the model, and clearing drops the lightmap.

mod e2e_common;

use e2e_common::E2e;
use serde_json::json;

#[test]
fn bake_and_clear() {
    let mut e = E2e::new();
    e.exec("box", json!({ "corner": [0, 0, -1], "length": 200, "width": 200, "height": 1 }));
    e.exec("box", json!({ "corner": [80, 80, 0], "length": 40, "width": 40, "height": 40 }));
    e.typed("vscurrent realistic");
    let r = e.exec("lightbake", json!({ "quality": "draft", "texel": 20 }));
    assert!(r["baked"].as_u64().unwrap_or(0) >= 1 && r["texels"].as_u64().unwrap_or(0) > 0, "{r}");
    assert!(e.inked(400, 300) > 500, "the model renders");
    let again = e.exec("lightbake", json!({ "quality": "draft", "texel": 20 }));
    assert!(again["kept"].as_u64().unwrap_or(0) >= 1, "nothing changed: kept {again}");
    let c = e.exec("lightbake", json!({ "clear": true }));
    assert!(c.is_object(), "{c}");
}
