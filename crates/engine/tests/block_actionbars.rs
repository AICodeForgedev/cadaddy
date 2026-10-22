//! The Block Editor's action bars (BACTIONBAR, BACTIONBARMODE) and visibility state editing
//! (BVSTATE current, BVSHOW, BVHIDE, BVMODE).

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Value {
    s.execute("line", &json!({ "points": [a, b] })).unwrap()["handles"][0].clone()
}

fn block_handles(s: &Session, name: &str) -> Vec<String> {
    s.doc().unwrap().block(name).unwrap().entities.handles().iter().map(|h| h.hex()).collect()
}

/// Block "Box" (a 10 × 5 rectangle, base point (2,0)) with a linear parameter "Width" across its
/// bottom, a stretch and a move following it, and a point parameter without actions.
fn authored() -> Session {
    let mut s = Session::new();
    let ls = vec![
        line(&mut s, [0.0, 0.0], [10.0, 0.0]),
        line(&mut s, [10.0, 0.0], [10.0, 5.0]),
        line(&mut s, [10.0, 5.0], [0.0, 5.0]),
        line(&mut s, [0.0, 5.0], [0.0, 0.0]),
    ];
    s.execute("block", &json!({ "name": "Box", "base": [2, 0], "handles": ls })).unwrap();
    s.execute("bparameter", &json!({ "block": "Box", "type": "linear", "base": [0, 0], "end": [10, 0], "label": "Width" })).unwrap();
    let hs = block_handles(&s, "Box");
    s.execute(
        "bactiontool",
        &json!({ "block": "Box", "type": "stretch", "parameter": "Width", "handles": hs, "frame": [[8, -1], [12, -1], [12, 6], [8, 6]] }),
    )
    .unwrap();
    s.execute("bactiontool", &json!({ "block": "Box", "type": "move", "parameter": "Width", "handles": [hs[0]] })).unwrap();
    s.execute("bparameter", &json!({ "block": "Box", "type": "point", "at": [4, 7], "label": "Tag" })).unwrap();
    s
}

fn bar<'a>(r: &'a Value, parameter: &str) -> &'a Value {
    r["bars"].as_array().unwrap().iter().find(|b| b["parameter"] == parameter).unwrap_or_else(|| panic!("no bar for {parameter}: {r}"))
}

#[test]
fn action_bars_list_each_parameters_actions_in_the_editor() {
    let mut s = authored();
    assert!(s.execute("bactionbar", &json!({})).is_err(), "only in the Block Editor");
    s.execute("bedit", &json!({ "name": "Box" })).unwrap();
    let r = s.execute("bactionbar", &json!({})).unwrap();
    assert_eq!(r["mode"], 1, "BACTIONBARMODE defaults to action bars");
    let w = bar(&r, "Width");
    let names: Vec<&str> = w["actions"].as_array().unwrap().iter().map(|a| a["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 2, "{w}");
    assert!(names.iter().any(|n| n.starts_with("Stretch")) && names.iter().any(|n| n.starts_with("Move")), "{names:?}");
    // Under the parameter's middle, in editor coordinates (the base point (2,0) at the origin).
    assert_eq!(w["at"], json!([3.0, 0.0]));
    assert_eq!(w["shown"], true);
    let t = bar(&r, "Tag");
    assert!(t["actions"].as_array().unwrap().is_empty());
    assert_eq!(t["at"], json!([2.0, 7.0]));
}

#[test]
fn bars_are_hidden_shown_and_reset() {
    let mut s = authored();
    s.execute("bedit", &json!({ "name": "Box" })).unwrap();
    let r = s.execute("bactionbar", &json!({ "action": "hide", "parameters": ["Width"] })).unwrap();
    assert_eq!(bar(&r, "Width")["shown"], false);
    assert_eq!(bar(&r, "Tag")["shown"], true);
    let r = s.execute("bactionbar", &json!({ "action": "hide" })).unwrap();
    assert!(r["bars"].as_array().unwrap().iter().all(|b| b["shown"] == false));
    let r = s.execute("bactionbar", &json!({ "action": "show", "parameters": ["tag"] })).unwrap();
    assert_eq!(bar(&r, "Tag")["shown"], true);
    let r = s.execute("bactionbar", &json!({ "action": "reset" })).unwrap();
    assert!(r["bars"].as_array().unwrap().iter().all(|b| b["shown"] == true));
    assert!(s.execute("bactionbar", &json!({ "action": "hide", "parameters": ["Nope"] })).is_err());
    assert!(s.execute("bactionbar", &json!({ "action": "spin" })).is_err());
    s.execute("setvar", &json!({ "name": "BACTIONBARMODE", "value": 0 })).unwrap();
    assert_eq!(s.execute("bactionbar", &json!({})).unwrap()["mode"], 0);
    assert!(s.execute("setvar", &json!({ "name": "BACTIONBARMODE", "value": 2 })).is_err());
}

/// Block "Sign": lines a (short) and b (long), states "Long" (both) and "Short" (a only).
fn sign() -> (Session, String, String) {
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [4.0, 0.0]);
    let b = line(&mut s, [0.0, 1.0], [8.0, 1.0]);
    s.execute("block", &json!({ "name": "Sign", "base": [0, 0], "handles": [a, b] })).unwrap();
    let hs = block_handles(&s, "Sign");
    s.execute("bparameter", &json!({ "block": "Sign", "type": "visibility", "at": [0, -1] })).unwrap();
    s.execute("bvstate", &json!({ "block": "Sign", "action": "rename", "name": "VisibilityState0", "newName": "Long" })).unwrap();
    s.execute("bvstate", &json!({ "block": "Sign", "action": "new", "name": "Short", "handles": [hs[0]] })).unwrap();
    (s, hs[0].clone(), hs[1].clone())
}

fn visible(r: &Value, state: &str) -> Vec<String> {
    let st = r["states"].as_array().unwrap().iter().find(|x| x["name"] == state).unwrap();
    let mut v: Vec<String> = st["visible"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_string()).collect();
    v.sort();
    v
}

fn hidden(s: &Session) -> Vec<String> {
    s.doc().unwrap().hidden_objects.iter().map(|h| h.hex()).collect()
}

#[test]
fn the_current_state_hides_its_invisible_objects_in_the_editor() {
    let (mut s, a, b) = sign();
    s.execute("bedit", &json!({ "name": "Sign" })).unwrap();
    let r = s.execute("bvstate", &json!({})).unwrap();
    assert_eq!(r["current"], "Long", "the first state until one is made current");
    let r = s.execute("bvstate", &json!({ "action": "current", "name": "short" })).unwrap();
    assert_eq!(r["current"], "Short");
    assert_eq!(hidden(&s), vec![b.clone()]);
    // BVMODE 1 shows the hidden objects too.
    s.execute("setvar", &json!({ "name": "BVMODE", "value": 1 })).unwrap();
    s.execute("bvstate", &json!({ "action": "current", "name": "Short" })).unwrap();
    assert!(hidden(&s).is_empty());
    s.execute("setvar", &json!({ "name": "BVMODE", "value": 0 })).unwrap();
    s.execute("bvstate", &json!({ "action": "current", "name": "Long" })).unwrap();
    assert!(hidden(&s).is_empty());
    assert!(s.execute("bvstate", &json!({ "action": "current", "name": "Medium" })).is_err());
    let _ = a;
}

#[test]
fn bvhide_and_bvshow_change_the_current_state_and_save_into_the_block() {
    let (mut s, a, b) = sign();
    s.execute("bedit", &json!({ "name": "Sign" })).unwrap();
    s.execute("bvstate", &json!({ "action": "current", "name": "Short" })).unwrap();
    // Show the long line in "Short" too, then hide the short one there.
    let r = s.execute("bvshow", &json!({ "handles": [b] })).unwrap();
    assert_eq!(r["state"], "Short");
    let mut both = vec![a.clone(), b.clone()];
    both.sort();
    assert_eq!(visible(&r, "Short"), both);
    assert!(hidden(&s).is_empty());
    let r = s.execute("bvhide", &json!({ "handles": [a] })).unwrap();
    assert_eq!(visible(&r, "Short"), vec![b.clone()]);
    assert_eq!(visible(&r, "Long"), both, "other states keep their objects");
    assert_eq!(hidden(&s), vec![a.clone()]);
    // In every state at once.
    let r = s.execute("bvhide", &json!({ "handles": [b], "allStates": true })).unwrap();
    assert!(visible(&r, "Short").is_empty());
    assert_eq!(visible(&r, "Long"), vec![a.clone()]);
    s.execute("undo", &json!({})).unwrap();
    // Saved into the block: the drawing's block has the states as edited.
    s.execute("bclose", &json!({ "save": true })).unwrap();
    assert!(hidden(&s).is_empty(), "the editor's hiding doesn't leak into the drawing");
    let r = s.execute("bvstate", &json!({ "block": "Sign" })).unwrap();
    assert_eq!(visible(&r, "Short"), vec![b.clone()]);
    assert_eq!(visible(&r, "Long"), both);
}

#[test]
fn bvhide_refuses_bad_input() {
    let (mut s, _, _) = sign();
    assert!(s.execute("bvhide", &json!({ "handles": [] })).is_err(), "not in the Block Editor");
    s.execute("bedit", &json!({ "name": "Sign" })).unwrap();
    assert!(s.execute("bvhide", &json!({ "handles": [] })).is_err(), "nothing selected");
    assert!(s.execute("bvhide", &json!({ "handles": ["FFFFFF"] })).is_err(), "not an object of the block");
    assert!(s.execute("bvhide", &json!({ "handles": "x" })).is_err());
    let hs = block_handles(&s, "Sign");
    assert!(s.execute("bvhide", &json!({ "handles": [hs[0]], "state": "Nope" })).is_err());
}
