//! End to end: command aliases. ALIASEDIT typed alone opens the alias editor; an alias added through
//! its JSON form is typed at the command line and runs its command (a circle appears); removed, it
//! is unknown again; an alias that would replace a built-in one is reported as a conflict.

use crate::e2e_common::E2e;
use serde_json::json;

fn circles(e: &mut E2e) -> u64 {
    e.exec("entities", json!({ "type": "Circle" }))["count"].as_u64().unwrap_or(0)
}

#[test]
fn an_alias_is_added_typed_and_removed() {
    let mut e = E2e::new();
    e.typed("aliasedit");
    assert_eq!(e.dialog().as_deref(), Some("aliasedit"));
    e.ok("ui.key", json!({ "key": "escape" }));

    let r = e.exec("aliasedit", json!({ "set": { "QQC": "circle" } }));
    assert_eq!(r["aliases"]["qqc"], "circle", "kept in lower case: {}", r["aliases"]);
    e.typed("qqc 10,10 5");
    assert_eq!(circles(&mut e), 1, "the alias ran CIRCLE: {:?}", e.history());
    assert!(e.inked(200, 200) > 0, "the circle is drawn");

    e.exec("aliasedit", json!({ "remove": ["QQC"] }));
    let before = e.history().len();
    let _ = e.call("cmdline.input", json!({ "text": "qqc" }));
    let said: Vec<String> = e.history().into_iter().skip(before.saturating_sub(1)).collect();
    assert!(said.iter().any(|l| l.to_lowercase().contains("unknown")), "removed: {said:?}");
    assert_eq!(circles(&mut e), 1);

    // Replacing a built-in alias (C = CIRCLE) is a conflict to report.
    let r = e.exec("aliasedit", json!({ "set": { "C": "copy" } }));
    assert!(r["conflicts"].as_array().is_some_and(|c| c.iter().any(|x| x["alias"] == "c")), "{}", r["conflicts"]);
    e.exec("aliasedit", json!({ "clear": true }));
}

#[test]
fn the_open_editor_adds_nothing_to_the_journal_or_the_event_queue() {
    let mut e = E2e::new();
    e.typed("aliasedit");
    assert_eq!(e.dialog().as_deref(), Some("aliasedit"));
    e.ok("events.subscribe", json!({ "on": true }));
    let since = e.ok("events.poll", json!({}))["next"].as_u64().unwrap_or(0);
    let said = |e: &mut E2e| e.app.session.log.iter().filter(|l| l.to_lowercase().contains("aliasedit")).count();
    let journal = |e: &mut E2e| e.exec("commandhistory", json!({}))["lines"].to_string().to_lowercase().matches("aliasedit").count();
    let (log_before, journal_before) = (said(&mut e), journal(&mut e));
    for _ in 0..5 {
        e.frame();
    }
    assert_eq!(e.dialog().as_deref(), Some("aliasedit"), "still open");
    assert_eq!((said(&mut e), journal(&mut e)), (log_before, journal_before), "drawing the editor ran no command");
    let polled = e.ok("events.poll", json!({ "since": since }));
    let runs: Vec<&serde_json::Value> =
        polled["events"].as_array().into_iter().flatten().filter(|ev| ev["type"] == "command.run" && ev["command"] == "aliasedit").collect();
    assert!(runs.is_empty(), "no command.run for aliasedit while merely drawn: {runs:?}");

    // Using it still runs the command: Add or Change goes through ALIASEDIT.
    e.exec("aliasedit", json!({ "set": { "qqe": "circle" } }));
    let polled = e.ok("events.poll", json!({ "since": since }));
    assert!(polled["events"].as_array().is_some_and(|a| a.iter().any(|ev| ev["command"] == "aliasedit")), "{polled}");
    e.exec("aliasedit", json!({ "clear": true }));
}
