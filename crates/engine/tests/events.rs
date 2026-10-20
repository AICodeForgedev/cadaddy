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

    // Reading again gives nothing new; unsubscribing stops recording.
    let next = r["next"].as_u64().unwrap();
    assert!(s.events_poll(next)["events"].as_array().unwrap().is_empty());
    s.events_subscribe(false);
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    assert_eq!(s.events_poll(0)["events"], json!([]));
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
