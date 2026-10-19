//! End to end: Find and Replace. FIND typed alone opens the dialog with its controls (Find, Replace
//! All, Match case, the search modes); the JSON form, as an agent uses it, replaces "ROOM" with
//! "SUITE" in the drawing's texts (read back with TEXTS), whole words and case as asked.
//!
//! Known gap (reported): the dialog's "Find what" and "Replace with" boxes are not listed by
//! `ui.elements` (unlabelled text boxes), so an agent can't type into them; the test types
//! through the JSON form instead.

mod e2e_common;

use e2e_common::E2e;
use serde_json::json;

fn texts(e: &mut E2e) -> Vec<String> {
    let r = e.exec("texts", json!({}));
    r["texts"].as_array().into_iter().flatten().filter_map(|t| t["text"].as_str().map(str::to_string)).collect()
}

#[test]
fn the_dialog_opens_and_replacing_changes_the_texts() {
    let mut e = E2e::new();
    for (i, t) in ["ROOM 101", "ROOM 102", "CORRIDOR", "Room key"].iter().enumerate() {
        e.exec("text", json!({ "at": [0, i as f64 * 10.0], "height": 2.5, "text": t }));
    }
    e.typed("find");
    assert_eq!(e.dialog().as_deref(), Some("find"));
    for label in ["Find", "Replace All", "Match case", "Find whole words only", "Plain text", "Use wildcards"] {
        assert!(e.shows(label), "the Find and Replace dialog shows {label}");
    }
    assert!(e.click("Close"));
    assert_eq!(e.dialog(), None, "Close closes it");

    // Matching case: "Room key" stays.
    let r = e.exec("find", json!({ "find": "ROOM", "replace": "SUITE", "matchCase": true }));
    assert!(r.to_string().contains('2'), "two replaced: {r}");
    let t = texts(&mut e);
    for want in ["SUITE 101", "SUITE 102", "CORRIDOR", "Room key"] {
        assert!(t.iter().any(|x| x == want), "{want} in {t:?}");
    }
    // Wildcards, whole words.
    e.exec("find", json!({ "find": "SUITE 10?", "replace": "UNIT", "mode": "wildcard" }));
    let t = texts(&mut e);
    assert_eq!(t.iter().filter(|x| *x == "UNIT").count(), 2, "{t:?}");
    assert!(e.inked(300, 300) > 0);
}
