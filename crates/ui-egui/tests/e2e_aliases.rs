//! End to end: command aliases. ALIASEDIT typed alone opens the alias editor; an alias added through
//! its JSON form is typed at the command line and runs its command (a circle appears); removed, it
//! is unknown again; an alias that would replace a built-in one is reported as a conflict.

mod e2e_common;

use e2e_common::E2e;
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
