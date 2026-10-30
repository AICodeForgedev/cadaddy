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
    assert_eq!(n, 5, "{l}");
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
    s.execute("layer.new", &json!({ "name": "NOTES" })).ok();
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
    replay.settings.trustedpaths = dir.to_string_lossy().into_owned();
    replay.execute("script", &json!({ "path": path.to_string_lossy() })).unwrap();
    assert_eq!(shapes(&replay), shapes(&s));
    assert_eq!(shapes(&s).len(), 4);

    s.execute("commandhistory", &json!({ "action": "clear" })).unwrap();
    assert_eq!(s.execute("commandhistory", &json!({})).unwrap()["lines"], json!([]));
    assert!(s.execute("commandhistory", &json!({ "action": "save" })).is_err());
    let _ = std::fs::remove_dir_all(&dir);
    let _: Value = Value::Null;
}

// ---- the saved script replays what really happened (Esc, start over a running command, nesting) ----

/// The session saved as a script and run in a new session; the saved result.
fn replay(s: &mut Session, tag: &str) -> (Session, Value) {
    let dir = std::env::temp_dir().join(format!("cadaddy-history-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.scr");
    let saved = s.execute("commandhistory", &json!({ "action": "save", "path": path.to_string_lossy() })).unwrap();
    let mut r = Session::new();
    r.settings.trustedpaths = dir.to_string_lossy().into_owned();
    // A script with a gap in it may fail where the gap is (ERASE with nothing selected).
    let _ = r.execute("script", &json!({ "path": path.to_string_lossy() }));
    let _ = std::fs::remove_dir_all(&dir);
    (r, saved)
}

fn pt(s: &mut Session, x: f64, y: f64) {
    s.input(Input::Point(Vec2::new(x, y))).unwrap();
}

fn lines(s: &mut Session) -> Vec<String> {
    s.execute("commandhistory", &json!({})).unwrap()["lines"].as_array().unwrap().iter().map(|l| l.as_str().unwrap().to_string()).collect()
}

#[test]
fn esc_after_a_command_drew_replays_the_same_drawing() {
    let mut s = Session::new();
    s.start("line").unwrap();
    pt(&mut s, 0.0, 0.0);
    pt(&mut s, 10.0, 0.0);
    pt(&mut s, 10.0, 10.0);
    s.input(Input::Cancel).unwrap();
    assert_eq!(shapes(&s).len(), 2, "Esc keeps the two segments");
    assert_eq!(lines(&mut s).last().map(String::as_str), Some("^C"));
    let (r, saved) = replay(&mut s, "esc");
    assert_eq!(shapes(&r), shapes(&s));
    assert_eq!(saved["unscriptable"], json!(0));

    // The same through the app's Esc (`cancel`), which is not an input.
    let mut s = Session::new();
    s.start("line").unwrap();
    pt(&mut s, 0.0, 0.0);
    pt(&mut s, 5.0, 5.0);
    s.cancel();
    let (r, _) = replay(&mut s, "esc2");
    assert_eq!(shapes(&r), shapes(&s));
    assert_eq!(shapes(&r).len(), 1);
}

#[test]
fn esc_that_changed_nothing_leaves_no_lines() {
    let mut s = Session::new();
    s.cmdline("line 0,0 10,0 ").unwrap();
    let before = lines(&mut s);
    s.start("line").unwrap();
    pt(&mut s, 100.0, 100.0);
    s.input(Input::Cancel).unwrap();
    assert_eq!(lines(&mut s), before, "the cancelled LINE left nothing");
    s.start("circle").unwrap();
    s.cancel();
    assert_eq!(lines(&mut s), before);
}

#[test]
fn an_offset_loop_ended_with_esc_replays_the_same_drawing() {
    let mut s = Session::new();
    s.cmdline("line 0,0 10,0 ").unwrap();
    s.start("offset").unwrap();
    s.input(Input::Text("2".into())).unwrap();
    // First copy: pick the line, then the side.
    pt(&mut s, 5.0, 0.0);
    pt(&mut s, 5.0, 5.0);
    // Second copy, from the first one.
    pt(&mut s, 5.0, 2.0);
    pt(&mut s, 5.0, 9.0);
    s.input(Input::Cancel).unwrap();
    assert_eq!(shapes(&s).len(), 3, "the line and two copies: {:?}", shapes(&s));
    let (r, _) = replay(&mut s, "offset");
    assert_eq!(shapes(&r), shapes(&s));
}

#[test]
fn starting_a_command_over_a_running_one_replays_the_same_drawing() {
    let mut s = Session::new();
    s.start("line").unwrap();
    pt(&mut s, 0.0, 0.0);
    pt(&mut s, 10.0, 0.0);
    s.start("circle").unwrap();
    pt(&mut s, 3.0, 3.0);
    s.input(Input::Text("2".into())).unwrap();
    let l = lines(&mut s);
    let at = l.iter().position(|x| x == "circle").unwrap();
    assert_eq!(l.get(at - 1).map(String::as_str), Some("^C"), "the cancelled LINE is ended before CIRCLE: {l:?}");
    let (r, _) = replay(&mut s, "over");
    assert_eq!(shapes(&r), shapes(&s));
    assert_eq!(shapes(&s).len(), 2);

    // A LINE that drew nothing, replaced by another command, leaves nothing behind.
    let mut s = Session::new();
    s.start("line").unwrap();
    pt(&mut s, 0.0, 0.0);
    s.start("circle").unwrap();
    pt(&mut s, 3.0, 3.0);
    s.input(Input::Text("2".into())).unwrap();
    assert_eq!(lines(&mut s), ["circle", "*3,3", "2"]);
    let (r, _) = replay(&mut s, "over2");
    assert_eq!(shapes(&r), shapes(&s));
}

#[test]
fn an_interactive_transparent_command_does_not_lose_the_outer_start() {
    let mut s = Session::new();
    s.start("line").unwrap();
    pt(&mut s, 0.0, 0.0);
    pt(&mut s, 10.0, 0.0);
    s.start("'zoom").unwrap();
    pt(&mut s, -5.0, -5.0);
    pt(&mut s, 20.0, 20.0);
    pt(&mut s, 10.0, 10.0);
    s.input(Input::Cancel).unwrap();
    assert_eq!(shapes(&s).len(), 2);
    let l = lines(&mut s);
    assert_eq!(l.first().map(String::as_str), Some("line"), "{l:?}");
    assert!(l.iter().any(|x| x == "'zoom"), "{l:?}");
    let (r, _) = replay(&mut s, "zoom");
    assert_eq!(shapes(&r), shapes(&s));

    // Esc inside the transparent command alone, then the outer one goes on and ends.
    let mut s = Session::new();
    s.start("line").unwrap();
    pt(&mut s, 0.0, 0.0);
    s.start("'zoom").unwrap();
    s.input(Input::Cancel).unwrap();
    pt(&mut s, 4.0, 0.0);
    s.input(Input::Enter).unwrap();
    assert_eq!(lines(&mut s), ["line", "*0,0", "*4,0", ""], "the cancelled ZOOM left nothing");
    let (r, _) = replay(&mut s, "zoom2");
    assert_eq!(shapes(&r), shapes(&s));
}

#[test]
fn a_transparent_command_that_runs_at_once_is_a_line_of_the_outer_command() {
    let mut s = Session::new();
    s.start("line").unwrap();
    pt(&mut s, 0.0, 0.0);
    s.start("'pan").ok();
    pt(&mut s, 3.0, 0.0);
    s.input(Input::Cancel).unwrap();
    let (r, _) = replay(&mut s, "pan");
    assert_eq!(shapes(&r), shapes(&s));
}

// ---- selections the script can't hold are reported ----

#[test]
fn a_selection_made_before_the_command_replays_or_is_reported() {
    // Select one object with no command running, then ERASE it by the Delete key's command.
    let mut s = Session::new();
    s.cmdline("line 0,0 10,0 ").unwrap();
    s.cmdline("line 0,5 10,5 ").unwrap();
    let h = s.doc().unwrap().model.iter().next().unwrap().handle;
    s.set_selection(vec![h]);
    s.execute("erase.selection", &json!({})).unwrap();
    assert_eq!(shapes(&s).len(), 1);
    let (r, saved) = replay(&mut s, "erase");
    let unscriptable = saved["unscriptable"].as_u64().unwrap();
    assert!(shapes(&r) == shapes(&s) || unscriptable >= 1, "a silent clean script with a different replay: {saved}");
    assert!(unscriptable >= 1, "{saved}");
    assert!(saved["first_at"].as_u64().is_some() && saved["message"].is_string(), "{saved}");

    // Select, then MOVE it (pickfirst).
    let mut s = Session::new();
    s.cmdline("line 0,0 10,0 ").unwrap();
    let h = s.doc().unwrap().model.iter().next().unwrap().handle;
    s.set_selection(vec![h]);
    s.cmdline("move 0,0 5,5").ok();
    let moved = shapes(&s);
    let listed = s.execute("commandhistory", &json!({})).unwrap();
    let (r, _) = replay(&mut s, "move");
    assert!(shapes(&r) == moved || listed["unscriptable"].as_u64().unwrap() >= 1, "{listed}");
    assert!(listed["unscriptable"].as_u64().unwrap() >= 1, "{listed}");
    // The gap is a comment line the runner skips.
    assert!(lines(&mut s).iter().any(|l| l.starts_with("; cannot be replayed")));
}

#[test]
fn picked_objects_are_counted_and_scripted_selections_are_not() {
    let mut s = Session::new();
    s.cmdline("line 0,0 10,0 ").unwrap();
    s.start("erase").unwrap();
    // A click on the object is a point: the script holds it.
    pt(&mut s, 5.0, 0.0);
    s.input(Input::Enter).unwrap();
    assert!(shapes(&s).is_empty());
    let r = s.execute("commandhistory", &json!({})).unwrap();
    assert_eq!(r["unscriptable"], json!(0), "{r}");
    let (r, _) = replay(&mut s, "click");
    assert_eq!(shapes(&r), shapes(&s));

    // The same by handle (a lasso, selection cycling, a dialog): no script form, so it is counted.
    let mut s = Session::new();
    s.cmdline("line 0,0 10,0 ").unwrap();
    let h = s.doc().unwrap().model.iter().next().unwrap().handle;
    s.start("erase").unwrap();
    s.input(Input::Pick(vec![h])).unwrap();
    s.input(Input::Enter).unwrap();
    assert!(shapes(&s).is_empty());
    let r = s.execute("commandhistory", &json!({})).unwrap();
    assert_eq!(r["unscriptable"], json!(1), "{r}");
    let c = s.execute("commandhistory", &json!({ "action": "clear" })).unwrap();
    assert_eq!(c["unscriptable"], json!(0));
}

// ---- undo to a step with an open BEgin group ----

/// Two steps, UNDO Begin, three steps (a group still open).
fn grouped() -> Session {
    let mut s = Session::new();
    for i in 0..2 {
        s.execute("circle", &json!({ "center": [i * 10, 0], "radius": 1 })).unwrap();
    }
    s.cmdline("undo begin").unwrap();
    for i in 0..3 {
        s.execute("circle", &json!({ "center": [i * 10, 20], "radius": 1 })).unwrap();
    }
    s
}

#[test]
fn going_to_a_step_with_an_open_group_never_undoes_past_it() {
    let l = grouped().execute("undohistory", &json!({})).unwrap();
    assert_eq!(l["current"], json!(5), "{l}");
    assert_eq!(l["group"], json!(2), "the group starts after step 2: {l}");
    assert_eq!(l["marks"], json!([]));
    assert_eq!(l["steps"].as_array().unwrap().len(), 5);

    // Steps 3-5 are inside the group; the group is one step (3) once collapsed. A step inside it
    // is the group step: nothing is undone, and the drawing is never taken back past it.
    for inside in [3u64, 4, 5] {
        let mut a = grouped();
        let r = a.execute("undohistory", &json!({ "action": "to", "step": inside })).unwrap();
        assert_eq!(r["current"], json!(3), "step {inside}: {r}");
        assert_eq!(shapes(&a).len(), 5, "step {inside}: the group is whole");
        assert_eq!(r["group"], json!(null), "the group ended");
    }
    // Step 2 = what U twice does after the group collapsed (the group, then one more step).
    let mut a = grouped();
    let r = a.execute("undohistory", &json!({ "action": "to", "step": 2 })).unwrap();
    let mut b = grouped();
    b.cmdline("u").unwrap();
    assert_eq!((r["current"].clone(), shapes(&a)), (json!(2), shapes(&b)));
    assert_eq!(shapes(&a).len(), 2);
    // Step 1.
    let mut a = grouped();
    let r = a.execute("undohistory", &json!({ "action": "to", "step": 1 })).unwrap();
    let mut b = grouped();
    b.cmdline("u").unwrap();
    b.cmdline("u").unwrap();
    assert_eq!((r["current"].clone(), shapes(&a)), (json!(1), shapes(&b)));
    assert_eq!(shapes(&a).len(), 1);
    // Then forward again: to the end is REDO, group step included.
    let r = a.execute("undohistory", &json!({ "action": "to", "step": 3 })).unwrap();
    assert_eq!((r["current"].clone(), shapes(&a).len()), (json!(3), 5));
    // Step 4 is inside the group, so it was one of the five circles: the collapsed list has 3 steps.
    assert_eq!(r["steps"].as_array().unwrap().len(), 3, "{r}");
}

#[test]
fn the_undo_list_reports_marks_and_the_open_group() {
    let mut s = Session::new();
    s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    s.cmdline("undo mark").unwrap();
    s.execute("circle", &json!({ "center": [10, 0], "radius": 1 })).unwrap();
    s.cmdline("undo begin").unwrap();
    s.execute("circle", &json!({ "center": [20, 0], "radius": 1 })).unwrap();
    let l = s.execute("undohistory", &json!({})).unwrap();
    assert_eq!(l["marks"], json!([1]), "{l}");
    assert_eq!(l["group"], json!(2), "{l}");
    assert_eq!(l["current"], json!(3));
}
