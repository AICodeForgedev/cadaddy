//! End to end (0.7.8): the Count palette. Three references of a made-up CHAIR block and one TABLE:
//! the palette lists both; clicking CHAIR selects (highlights) its three references and zooms to
//! them; inserting another CHAIR updates the list at once; a count table follows when updated
//! from the palette's button.

mod e2e_common;

use e2e_common::E2e;
use serde_json::{Value, json};

fn block(e: &mut E2e, name: &str, at: [f64; 2]) {
    let c = e.exec("circle", json!({ "center": at, "radius": 1 }))["handle"].clone();
    e.exec("block", json!({ "name": name, "base": at, "handles": [c], "keep": "delete" }));
}

fn insert(e: &mut E2e, name: &str, at: [f64; 2]) {
    e.exec("insert", json!({ "name": name, "at": at }));
}

fn selection(e: &mut E2e) -> usize {
    let s = e.ok("ui.inspect", json!({}))["session"]["selection"].clone();
    s.as_array().map_or(0, Vec::len)
}

fn view(e: &mut E2e) -> Value {
    e.ok("ui.inspect", json!({}))["view"].clone()
}

/// Every text in the drawing's tables (the count table's cells).
fn table_texts(e: &mut E2e) -> String {
    e.ok("drawing.inspect", json!({ "entities": true })).to_string()
}

#[test]
fn the_count_palette_counts_highlights_zooms_and_follows_edits() {
    let mut e = E2e::new();
    block(&mut e, "CHAIR", [0.0, 0.0]);
    block(&mut e, "TABLE", [0.0, 0.0]);
    for x in [0.0, 50.0, 100.0] {
        insert(&mut e, "CHAIR", [x, 0.0]);
    }
    insert(&mut e, "TABLE", [500.0, 500.0]);
    e.exec("zoom", json!({ "mode": "extents" }));

    e.typed("countlist");
    assert_eq!(e.dialog().as_deref(), Some("countpalette"));
    assert!(e.shows("CHAIR") && e.shows("TABLE"), "a row per block");
    let counts = e.exec("count", json!({ "space": "current" }));
    assert_eq!((counts["blocks"]["CHAIR"].as_u64(), counts["blocks"]["TABLE"].as_u64()), (Some(3), Some(1)), "{counts}");

    // Click CHAIR: its references selected (the palette's highlight) and the view on them.
    let before = view(&mut e);
    assert!(e.click("CHAIR"));
    e.pump();
    assert_eq!(selection(&mut e), 3, "the three chairs highlighted");
    let after = view(&mut e);
    assert_ne!(before, after, "zoomed to them");
    assert!(after["height"].as_f64().unwrap() < before["height"].as_f64().unwrap(), "closer in: {before} → {after}");
    let chair = e.elements().into_iter().find(|x| x["label"] == "CHAIR").unwrap();
    assert_eq!(chair["selected"], true, "the row shows as picked");

    // A count table, then one more chair: the list follows at once, the table when updated.
    e.exec("counttable", json!({ "at": [0, -100] }));
    assert!(table_texts(&mut e).contains("Block Count"));
    insert(&mut e, "CHAIR", [150.0, 0.0]);
    e.pump();
    assert_eq!(e.exec("count", json!({ "block": "CHAIR" }))["count"].as_u64(), Some(4));
    assert!(e.click("CHAIR"));
    e.pump();
    assert_eq!(selection(&mut e), 4, "the palette's list was rebuilt after the edit");
    assert!(e.click("Update Count Tables"));
    e.pump();
    let all = e.exec("entities", json!({ "type": "Table" }));
    assert!(all.to_string().contains("CHAIR") && all.to_string().contains("\"4\""), "the table counts 4 chairs: {all}");

    // Erasing a block's last reference takes its row away.
    let tables = e.exec("count", json!({ "block": "TABLE" }));
    e.exec("erase", json!({ "handles": tables["handles"] }));
    e.pump();
    assert!(!e.shows("TABLE") || e.exec("count", json!({ "block": "TABLE" }))["count"].as_u64() == Some(0));
    assert!(!e.shows("TABLE"), "no row for a block with no references");
}
