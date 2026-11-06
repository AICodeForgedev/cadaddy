//! The event stream for agents watching a user: a scripted session produces the expected events.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn kinds(r: &Value) -> Vec<String> {
    r["events"].as_array().unwrap().iter().map(|e| e["type"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_scripted_session_produces_the_expected_events() {
    let mut s = Session::new();
    // Nothing is recorded before someone subscribes.
    s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
    assert_eq!(s.events_poll(0)["subscribed"], false);
    let sub = s.events_subscribe(true);
    let mut since = sub["seq"].as_u64().unwrap();
    assert!(s.events_poll(since)["events"].as_array().unwrap().is_empty(), "what is open now is not news");

    // A command run (JSON): command.run, then the change with its object.
    let c = s.execute("circle", &json!({"center": [5, 5], "radius": 2})).unwrap()["handle"].as_str().unwrap().to_string();
    let r = s.events_poll(since);
    assert_eq!(kinds(&r), ["command.run", "drawing.changed"], "{r}");
    let ev = &r["events"][1];
    assert_eq!(ev["added"], json!([c]));
    assert_eq!(ev["counts"], json!([1, 0, 0]));
    since = r["next"].as_u64().unwrap();

    // Selection.
    s.execute("select", &json!({"handles": [c]})).unwrap();
    let r = s.events_poll(since);
    assert!(kinds(&r).contains(&"selection.changed".to_string()), "{r}");
    assert_eq!(r["events"].as_array().unwrap().iter().find(|e| e["type"] == "selection.changed").unwrap()["count"], 1);
    since = r["next"].as_u64().unwrap();

    // A typed command: started, prompt changes, ended (and the change, coalesced at the read).
    s.cmdline("line").unwrap();
    s.cmdline("0,0").unwrap();
    s.cmdline("10,0").unwrap();
    s.cancel();
    let r = s.events_poll(since);
    let k = kinds(&r);
    assert_eq!(k.first().map(String::as_str), Some("command.started"), "{k:?}");
    assert!(k.contains(&"command.ended".to_string()), "{k:?}");
    assert_eq!(k.iter().filter(|x| *x == "drawing.changed").count(), 1, "coalesced: {k:?}");
    since = r["next"].as_u64().unwrap();

    // A typed LINE waiting for its next point: the prompt changed, with its text.
    s.cmdline("line").unwrap();
    s.cmdline("0,0").unwrap();
    let want = s.prompt_text();
    assert!(!want.is_empty());
    let r = s.events_poll(since);
    let p = r["events"].as_array().unwrap().iter().find(|e| e["type"] == "prompt.changed").unwrap_or_else(|| panic!("no prompt.changed: {r}"));
    assert_eq!(p["prompt"], json!(want));
    s.cancel();
    since = s.events_poll(since)["next"].as_u64().unwrap();

    // Undo changes the drawing again (the circle's line goes).
    s.execute("undo", &json!({})).unwrap();
    let r = s.events_poll(since);
    assert!(r["events"].as_array().unwrap().iter().any(|e| e["type"] == "drawing.changed" && e["counts"][1] == 1), "{r}");
    since = r["next"].as_u64().unwrap();

    // Documents: a new one opened (and switched to), then back.
    s.execute("new", &json!({})).unwrap();
    let r = s.events_poll(since);
    assert!(kinds(&r).contains(&"document.opened".to_string()), "{r}");
    since = r["next"].as_u64().unwrap();
    s.execute("document.switch", &json!({"index": 0})).unwrap();
    let r = s.events_poll(since);
    assert!(kinds(&r).contains(&"document.switched".to_string()), "{r}");

    // Closing a drawing is told with its uid.
    s.execute("document.close", &json!({"index": 1, "discard": true})).unwrap();
    let r = s.events_poll(r["next"].as_u64().unwrap());
    let closed = r["events"].as_array().unwrap().iter().find(|e| e["type"] == "document.closed").unwrap_or_else(|| panic!("no document.closed: {r}"));
    assert!(closed["uid"].is_u64(), "{closed}");

    // Reading again gives nothing new; unsubscribing stops recording.
    let next = r["next"].as_u64().unwrap();
    assert!(s.events_poll(next)["events"].as_array().unwrap().is_empty());
    s.events_subscribe(false);
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    assert_eq!(s.events_poll(0)["events"], json!([]));
}

#[test]
fn closing_every_drawing_then_opening_one_is_announced() {
    let mut s = Session::new();
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    let mut since = s.events_subscribe(true)["seq"].as_u64().unwrap();
    s.execute("document.closeall", &json!({"discard": true})).unwrap();
    let r = s.events_poll(since);
    assert!(kinds(&r).contains(&"document.closed".to_string()), "{r}");
    since = r["next"].as_u64().unwrap();
    s.execute("new", &json!({})).unwrap();
    let r = s.events_poll(since);
    assert!(kinds(&r).contains(&"document.opened".to_string()), "{r}");
}

#[test]
fn subscribing_with_no_drawing_open_still_announces_the_next_one() {
    let mut s = Session::new();
    s.execute("document.closeall", &json!({"discard": true})).unwrap();
    let mut since = s.events_subscribe(true)["seq"].as_u64().unwrap();
    assert!(s.events_poll(since)["events"].as_array().unwrap().is_empty());
    s.execute("new", &json!({})).unwrap();
    let r = s.events_poll(since);
    assert!(kinds(&r).contains(&"document.opened".to_string()), "{r}");
    since = r["next"].as_u64().unwrap();
    // Selection changes in the window after a zero-drawing state are not swallowed.
    let c = s.execute("circle", &json!({"center": [1, 1], "radius": 1})).unwrap()["handle"].as_str().unwrap().to_string();
    s.execute("select", &json!({"handles": [c]})).unwrap();
    let r = s.events_poll(since);
    assert!(kinds(&r).contains(&"selection.changed".to_string()), "{r}");
}

#[test]
fn a_moved_object_is_reported_as_changed() {
    let mut s = Session::new();
    let c = s.execute("circle", &json!({"center": [5, 5], "radius": 2})).unwrap()["handle"].as_str().unwrap().to_string();
    let since = s.events_subscribe(true)["seq"].as_u64().unwrap();
    s.execute("move", &json!({"handles": [c], "from": [0, 0], "to": [3, 0]})).unwrap();
    let r = s.events_poll(since);
    let d = r["events"].as_array().unwrap().iter().find(|e| e["type"] == "drawing.changed").unwrap_or_else(|| panic!("{r}"));
    assert_eq!(d["changed"], json!([c]));
    assert_eq!(d["counts"], json!([0, 0, 1]));
}

#[test]
fn a_flood_of_events_is_capped_and_counted() {
    let mut s = Session::new();
    s.events_subscribe(true);
    for i in 0..1_200 {
        s.event("dialog.opened", json!({"dialog": format!("d{i}")}));
    }
    let r = s.events_poll(0);
    // The thousand kept, after a notice of the 200 the reader missed.
    let evs = r["events"].as_array().unwrap();
    assert_eq!(evs.len(), 1_001);
    assert_eq!((evs[0]["type"].as_str(), evs[0]["count"].as_u64()), (Some("events.dropped"), Some(200)));
    assert_eq!(evs[1]["dialog"], "d200");
    assert_eq!(r["dropped"], 200);
    // Read up to date: nothing missed, no notice.
    let next = r["next"].as_u64().unwrap();
    s.event("dialog.opened", json!({"dialog": "late"}));
    let r = s.events_poll(next);
    assert_eq!(r["events"].as_array().unwrap().len(), 1, "{r}");
}
