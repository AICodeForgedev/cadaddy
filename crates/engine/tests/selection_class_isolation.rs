//! Selection leftovers: the `CLass` option at "Select objects" (objects by class, DXF type or type
//! name, with wildcards) and OBJECTISOLATIONMODE 1 (hidden objects saved with the drawing).

use cadaddy_engine::doc::Handle;
use cadaddy_engine::{Input, Session};
use serde_json::{Value, json};

/// Two lines, a circle, a text and an mtext.
fn scene() -> (Session, Vec<Handle>) {
    let mut s = Session::new();
    let last = |s: &Session| s.doc().unwrap().model.last().unwrap().handle;
    let mut hs = Vec::new();
    s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap();
    hs.push(last(&s));
    s.execute("line", &json!({ "points": [[0, 5], [10, 5]] })).unwrap();
    hs.push(last(&s));
    s.execute("circle", &json!({ "center": [20, 0], "radius": 2 })).unwrap();
    hs.push(last(&s));
    s.execute("text", &json!({ "at": [0, 10], "text": "Hello", "height": 1 })).unwrap();
    hs.push(last(&s));
    s.execute("mtext", &json!({ "at": [0, 20], "text": "World", "height": 1 })).unwrap();
    hs.push(last(&s));
    (s, hs)
}

fn model(s: &Session) -> Vec<Handle> {
    s.doc().unwrap().model.iter().map(|e| e.handle).collect()
}

#[test]
fn class_at_select_objects_takes_every_object_of_that_type() {
    let (mut s, hs) = scene();
    s.start("erase").unwrap();
    s.cmdline("cl").unwrap();
    assert_eq!(s.current_prompt().unwrap().message, "Enter class name");
    s.cmdline("LINE").unwrap();
    s.input(Input::Enter).unwrap();
    assert_eq!(model(&s), hs[2..].to_vec(), "both lines erased");
}

#[test]
fn class_names_dxf_types_type_names_and_wildcards() {
    let (mut s, hs) = scene();
    for (pattern, want) in [
        ("AcDbCircle", vec![hs[2]]),
        ("circle", vec![hs[2]]),
        ("*TEXT", vec![hs[3], hs[4]]),
        ("AcDbLine,AcDbCircle", vec![hs[0], hs[1], hs[2]]),
        ("MText", vec![hs[4]]),
        ("Line, Text", vec![hs[0], hs[1], hs[3]]),
    ] {
        let r = s.execute("select", &json!({ "class": pattern })).unwrap();
        assert_eq!(r["handles"], json!(want.iter().map(|h| h.hex()).collect::<Vec<_>>()), "{pattern}");
    }
    let r = s.execute("select", &json!({ "class": "AcDbSpline" })).unwrap();
    assert_eq!(r["selected"], 0);
}

#[test]
fn class_question_mark_lists_and_bad_names_keep_asking() {
    let (mut s, hs) = scene();
    s.start("erase").unwrap();
    s.cmdline("class").unwrap();
    s.cmdline("?").unwrap();
    assert_eq!(s.current_prompt().unwrap().message, "Enter class name", "? lists and asks again");
    s.cmdline("NOSUCHTHING").unwrap();
    // No objects of that class: back at Select objects, nothing picked.
    assert!(s.current_prompt().unwrap().message.contains("Select objects"));
    s.cmdline("cl").unwrap();
    s.cmdline("AcDbCircle").unwrap();
    s.input(Input::Enter).unwrap();
    assert!(!model(&s).contains(&hs[2]));
    assert_eq!(model(&s).len(), 4);
}

#[test]
fn class_skips_hidden_objects_and_follows_group_selection() {
    let (mut s, hs) = scene();
    s.execute("hideobjects", &json!({ "handles": [hs[0].hex()] })).unwrap();
    let r = s.execute("select", &json!({ "class": "LINE" })).unwrap();
    assert_eq!(r["handles"], json!([hs[1].hex()]), "the hidden line isn't selected");
    // At Select objects, a class pick brings the rest of a selectable group.
    s.execute("unisolateobjects", &json!({})).unwrap();
    s.execute("group", &json!({ "name": "G", "handles": [hs[2].hex(), hs[3].hex()] })).unwrap();
    s.set_selection(Vec::new());
    s.start("erase").unwrap();
    s.cmdline("cl").unwrap();
    s.cmdline("CIRCLE").unwrap();
    s.input(Input::Enter).unwrap();
    assert!(!model(&s).contains(&hs[3]), "the circle's group came with it");
}

fn getvar(s: &mut Session, name: &str) -> Value {
    s.execute("getvar", &json!({ "name": name })).unwrap()["value"].clone()
}

#[test]
fn objectisolationmode_1_saves_hidden_objects_with_the_drawing() {
    let (mut s, hs) = scene();
    assert_eq!(getvar(&mut s, "OBJECTISOLATIONMODE"), json!(0));
    s.execute("hideobjects", &json!({ "handles": [hs[0].hex()] })).unwrap();
    assert!(!s.doc().unwrap().hidden_saved, "mode 0: for this session");
    s.execute("setvar", &json!({ "name": "OBJECTISOLATIONMODE", "value": 1 })).unwrap();
    assert_eq!(getvar(&mut s, "OBJECTISOLATIONMODE"), json!(1));
    assert!(s.doc().unwrap().hidden_saved, "the open drawing saves its hidden objects from now on");
    s.execute("isolateobjects", &json!({ "handles": [hs[2].hex()] })).unwrap();
    assert!(s.doc().unwrap().hidden_saved);
    assert_eq!(s.doc().unwrap().hidden_objects.len(), 4);
    s.execute("setvar", &json!({ "name": "OBJECTISOLATIONMODE", "value": 0 })).unwrap();
    assert!(!s.doc().unwrap().hidden_saved);
    assert!(s.execute("setvar", &json!({ "name": "OBJECTISOLATIONMODE", "value": 2 })).is_err());
    // A user profile setting, like AutoCAD's.
    s.execute("setvar", &json!({ "name": "OBJECTISOLATIONMODE", "value": 1 })).unwrap();
    let profile = s.execute("options", &json!({})).unwrap();
    assert_eq!(profile["OBJECTISOLATIONMODE"], json!(1));
}
