//! UNDOHISTORY (going to a step equals repeated U and REDO) and COMMANDHISTORY (the session saved
//! as a script replays to the same drawing).

use cadaddy_engine::geom::Vec2;
use cadaddy_engine::{Input, Session};
use serde_json::{Value, json};

/// The model space as comparable values (geometry and properties, no handles).
fn shapes(s: &Session) -> Vec<String> {
    let mut v: Vec<String> =
        s.doc().unwrap().model.iter().map(|e| format!("{}|{}", e.common.layer, serde_json::to_string(&e.kind).unwrap())).collect();
    v.sort();
    v
}

fn work(s: &mut Session) {
    s.cmdline("line 0,0 10,0 10,10 ").unwrap();
    s.execute("circle", &json!({ "center": [5, 5], "radius": 2 })).unwrap();
    s.cmdline("rectang 20,0 30,10").unwrap();
    s.execute("circle", &json!({ "center": [40, 5], "radius": 1 })).unwrap();
    s.cmdline("line 0,20 5,25 ").unwrap();
}

#[test]
fn going_to_a_step_equals_repeated_u_and_redo() {
    let mut a = Session::new();
    work(&mut a);
    let l = a.execute("undohistory", &json!({})).unwrap();
    let n = l["current"].as_u64().unwrap();
    assert!(n >= 5, "{l}");
    assert!(l["steps"].as_array().unwrap().iter().all(|s| s["done"] == true));
    let full = shapes(&a);

    let mut b = Session::new();
    work(&mut b);
    for _ in 0..3 {
        b.cmdline("u").unwrap();
    }
    let r = a.execute("undohistory", &json!({ "action": "to", "step": n - 3 })).unwrap();
    assert_eq!(shapes(&a), shapes(&b), "to step n-3 = U three times");
    assert_eq!(r["current"], json!(n - 3));
    let steps = r["steps"].as_array().unwrap();
    assert_eq!(steps.iter().filter(|s| s["done"] == false).count(), 3, "three steps to redo");

    b.cmdline("redo").unwrap();
    b.cmdline("redo").unwrap();
    a.execute("undohistory", &json!({ "action": "to", "step": n - 1 })).unwrap();
    assert_eq!(shapes(&a), shapes(&b), "forward two = REDO twice");

    a.execute("undohistory", &json!({ "action": "to", "step": n })).unwrap();
    assert_eq!(shapes(&a), full);
    assert!(a.execute("undohistory", &json!({ "action": "to", "step": n + 10 })).is_err());
    a.execute("undohistory", &json!({ "action": "to", "step": 0 })).unwrap();
    assert!(shapes(&a).is_empty(), "everything undone");
}

#[test]
fn the_session_saved_as_a_script_replays_to_the_same_drawing() {
    let dir = std::env::temp_dir().join(format!("cadaddy-history-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::new();
    // Typed, picked, a call with parameters, a cancelled command, Enter repeating a command.
    s.cmdline("line 0,0 10,0 ").unwrap();
    s.start("circle").unwrap();
    s.input(Input::Point(Vec2::new(3.5, 4.25))).unwrap();
    s.input(Input::Text("2".into())).unwrap();
    s.execute("layer", &json!({ "action": "new", "name": "NOTES" })).ok();
    s.execute("text", &json!({ "at": [0, -5], "height": 1, "text": "Hello world" })).unwrap();
    s.start("line").unwrap();
    s.input(Input::Point(Vec2::new(100.0, 100.0))).unwrap();
    s.input(Input::Cancel).unwrap();
    s.input(Input::Enter).unwrap();
    s.input(Input::Point(Vec2::new(0.0, 30.0))).unwrap();
    s.input(Input::Point(Vec2::new(8.0, 30.0))).unwrap();
    s.input(Input::Enter).unwrap();

    let lines: Vec<String> =
        s.execute("commandhistory", &json!({})).unwrap()["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect();
    assert!(!lines.iter().any(|l| l.contains("100")), "the cancelled LINE left nothing: {lines:?}");
    assert!(lines.contains(&"*3.5,4.25".to_string()), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with("text {")), "{lines:?}");
    let filtered = s.execute("commandhistory", &json!({ "filter": "circle" })).unwrap();
    assert_eq!(filtered["lines"], json!(["circle"]));

    let path = dir.join("session.scr");
    let r = s.execute("commandhistory", &json!({ "action": "save", "path": path.to_string_lossy() })).unwrap();
    assert_eq!(r["lines"].as_u64().unwrap() as usize, lines.len());

    let mut replay = Session::new();
    replay.execute("script", &json!({ "path": path.to_string_lossy() })).unwrap();
    assert_eq!(shapes(&replay), shapes(&s));
    assert_eq!(shapes(&s).len(), 4);

    s.execute("commandhistory", &json!({ "action": "clear" })).unwrap();
    assert_eq!(s.execute("commandhistory", &json!({})).unwrap()["lines"], json!([]));
    assert!(s.execute("commandhistory", &json!({ "action": "save" })).is_err());
    let _ = std::fs::remove_dir_all(&dir);
    let _: Value = Value::Null;
}
