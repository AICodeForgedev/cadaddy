//! End to end (0.7.8): the Purge dialog's tree. Two unused layers and an unused block: the tree
//! lists them under their kinds with counts; one layer unchecked, Purge Checked Items purges the
//! rest and keeps it; the tree updates; a layer in use is never offered.

mod e2e_common;

use e2e_common::E2e;
use serde_json::{Value, json};

fn rect(el: &Value) -> Vec<f64> {
    el["rect"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect()
}

fn el(e: &mut E2e, label: &str) -> Value {
    e.elements().into_iter().find(|x| x["label"] == label).unwrap_or_else(|| panic!("no `{label}` on screen"))
}

/// Open a kind's branch: its arrow is just left of its checkbox.
fn expand(e: &mut E2e, header: &str) {
    let r = rect(&el(e, header));
    e.ok("ui.click", json!({ "x": r[0] - 9.0, "y": r[1] + r[3] / 2.0, "space": "screen" }));
    e.pump();
}

fn layers(e: &mut E2e) -> Vec<String> {
    e.app.session.doc().map(|d| d.layers.iter().map(|l| l.name.clone()).collect()).unwrap_or_default()
}

#[test]
fn the_purge_tree_purges_what_is_checked_and_keeps_the_rest() {
    let mut e = E2e::new();
    for name in ["Spare1", "Spare2", "InUse"] {
        e.exec("layer.new", json!({ "name": name }));
    }
    let l = e.exec("line", json!({ "points": [[0, 0], [10, 0]] }))["handles"][0].clone();
    e.exec("chprop", json!({ "handles": [l], "layer": "InUse" }));
    let c = e.exec("circle", json!({ "center": [0, 0], "radius": 1 }))["handle"].clone();
    e.exec("block", json!({ "name": "OLDBLOCK", "base": [0, 0], "handles": [c], "keep": "delete" }));

    e.typed("purge");
    assert_eq!(e.dialog().as_deref(), Some("purge"));
    assert!(
        e.shows("Layers (2)"),
        "two unused layers: {:?}",
        e.elements().iter().filter(|x| x["role"] == "checkbox").map(|x| x["label"].clone()).collect::<Vec<_>>()
    );
    assert!(e.shows("Blocks (1)"));
    expand(&mut e, "Layers (2)");
    assert!(e.shows("Spare1") && e.shows("Spare2"), "the branch lists them");
    assert!(!e.shows("InUse"), "a layer in use isn't offered");

    // Uncheck Spare2, purge the checked items.
    let spare2 = el(&mut e, "Spare2");
    assert_eq!(spare2["selected"], true, "everything starts checked");
    assert!(e.click("Spare2"));
    e.pump();
    assert_eq!(el(&mut e, "Spare2")["selected"], false);
    assert!(e.click("Purge Checked Items"));
    e.pump();
    // A confirmation, if asked for.
    if e.shows("Yes to All") {
        e.click("Yes to All");
        e.pump();
    }
    let now = layers(&mut e);
    assert!(!now.iter().any(|n| n == "Spare1"), "Spare1 purged: {now:?}");
    assert!(now.iter().any(|n| n == "Spare2") && now.iter().any(|n| n == "InUse"), "Spare2 kept: {now:?}");
    assert!(e.exec("purge", json!({ "list": true })).to_string().contains("Spare2"));
    assert!(!e.exec("purge", json!({ "list": true })).to_string().contains("OLDBLOCK"), "the block went too");
    assert!(e.history().iter().any(|h| h.contains("purged")), "{:?}", e.history());
    // The tree shows what is left.
    assert_eq!(e.dialog().as_deref(), Some("purge"), "the dialog stays open");
    assert!(e.shows("Layers (1)") && !e.shows("Blocks (1)"));
}
