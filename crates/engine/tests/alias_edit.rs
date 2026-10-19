//! ALIASEDIT as the alias editor uses it: the built-in aliases listed beside the user's, a user
//! alias that takes over a built-in one flagged, aliases saved to a `.pgp` file that REINIT reads
//! back, and clear going back to the built-in aliases. One test: the user aliases are
//! process-wide.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn aliases(r: &Value) -> Vec<(String, String)> {
    r["aliases"].as_object().unwrap().iter().map(|(a, c)| (a.clone(), c.as_str().unwrap().to_string())).collect()
}

#[test]
fn aliases_are_listed_flagged_saved_and_reset() {
    let dir = std::env::temp_dir().join(format!("cadaddy-alias-edit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::new();
    s.set_lisp_support_dir(Some(dir.clone()));

    let r = s.execute("aliasedit", &json!({ "clear": true })).unwrap();
    let builtin = r["builtIn"].as_array().unwrap();
    assert!(builtin.iter().any(|b| b["alias"] == "l" && b["command"] == "line"), "L is LINE");
    assert!(r["conflicts"].as_array().unwrap().is_empty());

    // L for LINE taken over by CIRCLE: flagged; a fresh alias isn't.
    let r = s.execute("aliasedit", &json!({ "set": { "l": "circle", "zz": "zoom" } })).unwrap();
    assert_eq!(r["conflicts"], json!([{ "alias": "l", "command": "circle", "overrides": "line" }]));
    assert!(s.execute("aliasedit", &json!({ "set": { "line": "circle" } })).is_err(), "a command name");
    assert!(s.execute("aliasedit", &json!({ "set": { "q q": "line" } })).is_err());
    assert!(s.execute("aliasedit", &json!({ "set": { "qq": "nosuchcommand" } })).is_err());

    // Saved to cadaddy.pgp in the support folder, in the .pgp format.
    let r = s.execute("aliasedit", &json!({ "save": true })).unwrap();
    let path = dir.join("cadaddy.pgp");
    assert_eq!(r["saved"], json!(path.to_string_lossy()));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("L, *CIRCLE") && text.contains("ZZ, *ZOOM"), "{text}");

    // Removed and cleared, then REINIT reads the saved file back.
    let r = s.execute("aliasedit", &json!({ "remove": ["zz"] })).unwrap();
    assert_eq!(aliases(&r), [("l".to_string(), "circle".to_string())]);
    s.execute("aliasedit", &json!({ "clear": true })).unwrap();
    let r = s.execute("reinit", &json!({ "file": path.to_string_lossy() })).unwrap();
    assert_eq!(aliases(&r), [("l".to_string(), "circle".to_string()), ("zz".to_string(), "zoom".to_string())]);

    // An explicit path; a bad `save`.
    let other = dir.join("other.pgp");
    s.execute("aliasedit", &json!({ "save": other.to_string_lossy() })).unwrap();
    assert!(std::fs::read_to_string(&other).unwrap().contains("ZZ, *ZOOM"));
    assert!(s.execute("aliasedit", &json!({ "save": 5 })).is_err());

    // Reset to defaults: only the built-in aliases.
    let r = s.execute("aliasedit", &json!({ "clear": true })).unwrap();
    assert!(aliases(&r).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
