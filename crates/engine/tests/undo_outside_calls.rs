//! A call from outside while a command runs (an agent's `engine.execute` while the user is in
//! TEXT) is an undo step of its own: undoing the command never takes the call's work away, and
//! U undoes in the order things happened.

use cadaddy_engine::Session;
use cadaddy_engine::doc::EntityKind;
use serde_json::json;

fn kinds(s: &Session) -> Vec<&'static str> {
    let mut k: Vec<&'static str> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .map(|e| match &e.kind {
            EntityKind::Circle(_) => "circle",
            EntityKind::Text(t) if t.value == "first" => "first",
            EntityKind::Text(_) => "second",
            _ => "other",
        })
        .collect();
    k.sort_unstable();
    k
}

#[test]
fn undo_keeps_a_call_made_during_a_command_apart() {
    let mut s = Session::new();
    s.cmdline("TEXT 0,0 1 0").unwrap();
    s.cmdline("first").unwrap();
    s.execute("circle", &json!({ "center": [5, 5], "radius": 1 })).unwrap();
    s.cmdline("second").unwrap();
    s.cmdline("").unwrap();
    assert!(s.running.is_none());
    let all = vec!["circle", "first", "second"];
    assert_eq!(kinds(&s), all);

    // Newest first: the second line, the circle, the first line.
    s.undo().unwrap();
    assert_eq!(kinds(&s), ["circle", "first"], "the first U takes only the text typed after the call");
    s.undo().unwrap();
    assert_eq!(kinds(&s), ["first"]);
    s.undo().unwrap();
    assert!(kinds(&s).is_empty());
    for _ in 0..3 {
        s.redo().unwrap();
    }
    assert_eq!(kinds(&s), all);
}

#[test]
fn cancelling_the_command_keeps_the_call() {
    let mut s = Session::new();
    s.cmdline("LINE 0,0 10,0").unwrap();
    s.execute("circle", &json!({ "center": [5, 5], "radius": 1 })).unwrap();
    s.cancel();
    assert!(s.running.is_none());
    s.undo().unwrap();
    assert_eq!(kinds(&s), ["other"], "U takes the circle (the newest) and leaves the line");
    s.undo().unwrap();
    assert!(kinds(&s).is_empty());
}

#[test]
fn a_command_running_its_own_steps_is_still_one_undo_step() {
    // Inside a command, its own JSON calls are part of it (not an outside call).
    let mut s = Session::new();
    s.cmdline("RECTANG 0,0 10,5").unwrap();
    s.cmdline("CIRCLE 0,0 2").unwrap();
    s.undo().unwrap();
    assert_eq!(s.doc().unwrap().model.len(), 1, "one U takes the circle only");
}
