//! End to end: sheet sets through the control channel — a set made, drawings added, published as
//! one PDF; and the Sheet Set Manager palette opened from its menu shows the sheets.

mod e2e_common;

use e2e_common::{E2e, folder};
use serde_json::json;

#[test]
fn a_sheet_set_is_made_published_and_shown() {
    let mut e = E2e::new();
    let dir = folder("sheetsets");
    e.typed("rectang 0,0 100,60");
    e.typed("circle 50,30 10");
    let a = dir.join("A-101.dwg");
    e.exec("saveas", json!({ "path": a }));
    let set = dir.join("Project A.sheetset.json");
    e.exec("sheetset", json!({ "set": set, "action": "new", "name": "Project A" }));
    let r = e.exec("sheetset", json!({ "set": set, "action": "add", "drawing": a, "number": "A-101", "title": "Plan" }));
    assert!(r["sheets"].as_array().is_some_and(|s| !s.is_empty()), "{r}");
    let book = dir.join("Project A.pdf");
    let p = e.exec("sheetset", json!({ "set": set, "action": "publish", "path": book }));
    assert!(p["pages"].as_u64().unwrap_or(0) >= 1, "{p}");
    assert!(std::fs::read(&book).unwrap().starts_with(b"%PDF"));
    // The palette from its menu.
    e.menu("sheetset");
    assert_eq!(e.dialog().as_deref(), Some("sheetset"));
    let _ = std::fs::remove_dir_all(&dir);
}
