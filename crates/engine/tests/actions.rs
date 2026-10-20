//! The Action Recorder: ACTRECORD … ACTSTOP records the command-line work as a macro; playing it
//! (by name or ACTPLAY) reproduces the drawing; asked inputs, base points and messages; macro
//! files in the actions folder; the manager's list, ask, rename and delete.

use cadaddy_engine::Session;
use cadaddy_engine::doc::EntityKind;
use serde_json::{Value, json};

/// The model's objects as comparable text (kind and geometry).
fn shapes(s: &Session) -> Vec<String> {
    let mut v: Vec<String> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .map(|e| match &e.kind {
            EntityKind::Circle(c) => format!("circle {:.3},{:.3} r{:.3}", c.center.x, c.center.y, c.radius),
            EntityKind::Line(l) => format!("line {:.3},{:.3} {:.3},{:.3}", l.a.x, l.a.y, l.b.x, l.b.y),
            k => k.type_name().to_string(),
        })
        .collect();
    v.sort();
    v
}

fn record(s: &mut Session, lines: &[&str]) {
    s.execute("actrecord", &json!({})).unwrap();
    for l in lines {
        s.cmdline(l).unwrap();
    }
}

#[test]
fn a_recorded_macro_plays_back_the_same_drawing() {
    let mut s = Session::new();
    record(&mut s, &["circle", "*10,10", "5", "line", "*0,0", "*20,0", "*20,20", ""]);
    let r = s.execute("actstop", &json!({ "name": "Outline" })).unwrap();
    assert_eq!(r["asks"], 0, "{r}");
    let before = shapes(&s);
    assert_eq!(before.len(), 3, "{before:?}");
    assert!(
        r["steps"].as_array().unwrap().iter().all(|st| !st["input"].as_str().unwrap_or_default().starts_with("act")),
        "the recorder's own commands are left out: {r}"
    );
    // Played into a new drawing: the same objects.
    s.execute("new", &json!({})).unwrap();
    s.execute("actplay", &json!({ "name": "outline" })).unwrap();
    assert_eq!(shapes(&s), before);
    // Typing its name plays it too.
    s.execute("new", &json!({})).unwrap();
    s.cmdline("OUTLINE").unwrap();
    assert_eq!(shapes(&s), before);
}

#[test]
fn asked_inputs_take_new_values_or_keep_the_recorded_ones() {
    let mut s = Session::new();
    s.execute("actrecord", &json!({})).unwrap();
    s.cmdline("circle").unwrap();
    // Asked for: the centre (marked while CIRCLE prompts for it, the prompt kept).
    s.cmdline("'actuserinput").unwrap();
    s.cmdline("*10,10").unwrap();
    s.cmdline("5").unwrap();
    let r = s.execute("actstop", &json!({ "name": "Ring" })).unwrap();
    assert_eq!(r["asks"], 1, "{r}");
    let asked: Vec<&Value> = r["steps"].as_array().unwrap().iter().filter(|st| st["ask"] == true).collect();
    assert_eq!(asked[0]["input"], "*10,10", "{r}");
    assert!(asked[0]["prompt"].as_str().unwrap().to_lowercase().contains("center"), "{r}");
    // A new centre given.
    s.execute("new", &json!({})).unwrap();
    s.execute("actplay", &json!({ "name": "Ring", "inputs": [[50, 60]] })).unwrap();
    assert_eq!(shapes(&s), ["circle 50.000,60.000 r5.000"]);
    // Nothing given: the recorded one.
    s.execute("new", &json!({})).unwrap();
    s.execute("actplay", &json!({ "name": "Ring" })).unwrap();
    assert_eq!(shapes(&s), ["circle 10.000,10.000 r5.000"]);
    // Typed: the macro asks, Enter keeps, a point replaces.
    s.execute("new", &json!({})).unwrap();
    s.cmdline("ring").unwrap();
    assert!(s.prompt_text().contains("Ring"), "{}", s.prompt_text());
    s.cmdline("30,40").unwrap();
    assert!(s.running.is_none());
    assert_eq!(shapes(&s), ["circle 30.000,40.000 r5.000"]);
}

#[test]
fn a_base_point_moves_the_points_after_it_and_messages_are_shown() {
    let mut s = Session::new();
    s.execute("actrecord", &json!({})).unwrap();
    s.execute("actusermessage", &json!({ "text": "Pick where the door goes" })).unwrap();
    s.execute("actbasepoint", &json!({ "point": [0, 0] })).unwrap();
    for l in ["line", "*0,0", "*10,0", ""] {
        s.cmdline(l).unwrap();
    }
    let r = s.execute("actstop", &json!({ "name": "Sill" })).unwrap();
    assert_eq!(r["asks"], 1, "the base point is asked for: {r}");
    s.execute("new", &json!({})).unwrap();
    let from = s.log.len();
    s.execute("actplay", &json!({ "name": "Sill", "inputs": [[100, 50]] })).unwrap();
    assert_eq!(shapes(&s), ["line 100.000,50.000 110.000,50.000"]);
    assert!(s.log[from..].iter().any(|l| l.contains("Pick where the door goes")), "{:?}", &s.log[from..]);
}

#[test]
fn macros_are_saved_in_the_actions_folder_and_managed() {
    let dir = std::env::temp_dir().join(format!("cadaddy-actions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut s = Session::new();
    s.set_action_dir(Some(dir.clone()));
    record(&mut s, &["circle", "*0,0", "2"]);
    let r = s.execute("actstop", &json!({ "description": "a small circle" })).unwrap();
    assert_eq!(r["name"], "ActMacro1", "a default name");
    let file = dir.join("actmacro1.cdyact");
    assert!(file.is_file(), "{r}");
    let saved: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(saved["format"], "cadaddy-action-macro");
    // Another session finds it in the folder.
    let mut t = Session::new();
    t.set_action_dir(Some(dir.clone()));
    let list = t.execute("actions", &json!({})).unwrap();
    assert_eq!(list["macros"].as_array().unwrap().len(), 1, "{list}");
    t.execute("actplay", &json!({ "name": "ActMacro1" })).unwrap();
    assert_eq!(shapes(&t), ["circle 0.000,0.000 r2.000"]);
    // Mark the centre as asked for, rename, delete.
    let step = saved["steps"].as_array().unwrap().iter().position(|st| st["input"] == "*0,0").unwrap();
    let r = t.execute("actions", &json!({ "action": "ask", "name": "ActMacro1", "step": step })).unwrap();
    assert_eq!(r["asks"], 1);
    t.execute("actions", &json!({ "action": "rename", "name": "ActMacro1", "to": "Dot" })).unwrap();
    assert!(dir.join("dot.cdyact").is_file() && !file.exists());
    t.execute("actions", &json!({ "action": "delete", "name": "Dot" })).unwrap();
    assert!(!dir.join("dot.cdyact").exists());
    assert!(t.execute("actplay", &json!({ "name": "Dot" })).is_err());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn mistakes_are_errors() {
    let mut s = Session::new();
    assert!(s.execute("actstop", &json!({})).is_err(), "not recording");
    assert!(s.execute("actuserinput", &json!({})).is_err());
    s.execute("actrecord", &json!({})).unwrap();
    assert!(s.execute("actrecord", &json!({})).is_err(), "already recording");
    assert!(s.execute("actstop", &json!({})).is_err(), "nothing recorded");
    record(&mut s, &["circle", "*0,0", "1"]);
    assert!(s.execute("actstop", &json!({ "name": "line" })).is_err(), "a command's name");
    assert!(s.execute("actstop", &json!({ "name": "bad name!" })).is_err());
    s.execute("actstop", &json!({ "name": "ok" })).unwrap();
    for p in [json!({}), json!({ "name": "nope" }), json!({ "path": "/no/such.cdyact" })] {
        assert!(s.execute("actplay", &p).is_err(), "{p}");
    }
    assert!(s.execute("actbasepoint", &json!({ "point": "x" })).is_err());
}
