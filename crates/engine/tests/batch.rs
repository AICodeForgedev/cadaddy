//! Batch calls (MCP execute_many, control engine.executeMany): many commands, one undo step;
//! stopOnError rolls back.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn circles(s: &Session) -> usize {
    s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, cadaddy_doc::EntityKind::Circle(_))).count()
}

fn circle(i: usize) -> Value {
    json!({"command": "circle", "params": {"center": [i as f64 * 3.0, 0.0], "radius": 1}})
}

#[test]
fn hundreds_of_objects_in_one_call_and_one_undo_reverts_them_all() {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
    let undo = s.state().unwrap().undo.len();
    let cmds: Vec<Value> = (0..450).map(circle).collect();
    let r = s.execute_many(&cmds, true).unwrap();
    assert_eq!(r["ran"], 450);
    assert_eq!(r["results"].as_array().unwrap().len(), 450);
    assert!(r["results"][0]["handle"].is_string());
    assert_eq!(circles(&s), 450);
    assert_eq!(s.state().unwrap().undo.len(), undo + 1, "one undo step");
    assert!(s.state().unwrap().undo.last().unwrap().label.starts_with("Batch (450"));
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(circles(&s), 0);
    assert_eq!(s.doc().unwrap().model.len(), 1, "the line from before stays");
}

#[test]
fn stop_on_error_rolls_everything_back_and_says_where() {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
    let before = s.doc().unwrap().model.len();
    let undo = s.state().unwrap().undo.len();
    let cmds = vec![circle(0), circle(1), json!({"command": "circle", "params": {"center": "nowhere"}}), circle(3)];
    let r = s.execute_many(&cmds, true).unwrap();
    assert_eq!(r["error"]["index"], 2);
    assert_eq!(r["error"]["command"], "circle");
    assert_eq!(r["undone"], 2);
    assert_eq!(s.doc().unwrap().model.len(), before, "nothing the batch did is left");
    assert_eq!(s.state().unwrap().undo.len(), undo, "nor any undo step of it");
    // Without stopOnError: the failure is reported in its place and the rest still run.
    let r = s.execute_many(&cmds, false).unwrap();
    assert_eq!(r["ran"], 4);
    assert!(r["results"][2]["error"].is_string());
    assert_eq!(circles(&s), 3);
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(circles(&s), 0, "one undo for the three that ran");
}

#[test]
fn bad_batches_are_refused_before_anything_runs() {
    let mut s = Session::new();
    let too_many: Vec<Value> = (0..10_001).map(circle).collect();
    assert!(s.execute_many(&too_many, false).is_err());
    assert!(s.execute_many(&[circle(0), json!({"params": {}})], false).is_err(), "a command without `command`");
    assert_eq!(circles(&s), 0);
    assert_eq!(s.execute_many(&[], true).unwrap()["ran"], 0);
}
