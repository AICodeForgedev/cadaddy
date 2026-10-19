//! Cancelling a command leaves the selection as it was before the command: objects picked at its
//! "Select objects" prompt never become the next command's selection (ERASE would take them unasked).

use cadaddy_engine::{Input, Session};
use serde_json::json;

#[test]
fn escape_at_select_objects_leaves_nothing_selected() {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    let h = s.doc().unwrap().model.iter().next().unwrap().handle;
    s.cmdline("move").unwrap();
    s.input(Input::Pick(vec![h])).unwrap();
    s.input(Input::Cancel).unwrap();
    assert!(s.running.is_none());
    assert!(s.selection().is_empty(), "picked objects stayed selected: {:?}", s.selection());
    // ERASE now asks for objects instead of erasing the line.
    s.cmdline("erase").unwrap();
    assert!(s.running.is_some(), "ERASE should prompt");
    s.input(Input::Cancel).unwrap();
    assert_eq!(s.doc().unwrap().entity_count(), 1);
}

#[test]
fn a_selection_made_before_the_command_survives_its_cancel() {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    s.execute("select", &json!({"all": true})).unwrap();
    let before = s.selection();
    s.cmdline("rotate").unwrap();
    s.input(Input::Cancel).unwrap();
    assert_eq!(s.selection(), before);
}

#[test]
fn saveas_without_an_extension_adds_dwg() {
    use cadaddy_engine::cmd::file::{IoHooks, set_io};
    // The app's "Save new drawings as" format (.cdy unless chosen otherwise): DWG here.
    cadaddy_engine::cmd::file::set_default_ext("dwg");
    set_io(IoHooks { read: |_, _| Ok(cadaddy_engine::doc::Drawing::new_metric()), write: |_, _| Ok(b"saved".to_vec()), plot: None, read_pdf: None });
    let dir = std::env::temp_dir().join(format!("cadaddy-saveas-ext-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = Session::new();
    s.execute("saveas", &json!({"path": dir.join("plan").to_string_lossy()})).unwrap();
    assert!(dir.join("plan.dwg").is_file());
    assert!(s.state().unwrap().path.as_deref().is_some_and(|p| p.ends_with("plan.dwg")));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn image_commands_list_detach_and_set_transparency() {
    use cadaddy_engine::doc::{EntityKind, Image};
    let mut s = Session::new();
    let h = s.add_entity(EntityKind::Image(Image { path: "c:/nowhere/logo.png".into(), name: "logo".into(), ..Default::default() })).unwrap();
    let list = s.execute("image", &json!({})).unwrap();
    assert_eq!(list["images"][0]["name"], json!("logo"), "{list}");
    s.execute("transparency", &json!({"handles": [h.hex()], "on": true})).unwrap();
    let EntityKind::Image(i) = &s.doc().unwrap().entity(h).unwrap().kind else { panic!() };
    assert_eq!(i.display & 8, 8);
    s.execute("-image", &json!({"option": "path", "names": "LOGO", "path": "pics/logo.png"})).unwrap();
    let EntityKind::Image(i) = &s.doc().unwrap().entity(h).unwrap().kind else { panic!() };
    assert_eq!(i.path, "pics/logo.png");
    s.execute("-image", &json!({"option": "detach", "names": "lo*"})).unwrap();
    assert!(s.doc().unwrap().entity(h).is_none());
    assert!(s.execute("-image", &json!({"option": "detach", "names": "nothing"})).is_err());
    // IMAGEFRAME is a variable now (typing the name runs SETVAR).
    s.execute("setvar", &json!({"name": "IMAGEFRAME", "value": 2})).unwrap();
    assert!(s.execute("setvar", &json!({"name": "IMAGEFRAME", "value": 7})).is_err());
}

#[test]
fn recover_of_an_open_file_replaces_it_instead_of_a_second_tab() {
    use cadaddy_engine::cmd::file::{IoHooks, set_io};
    set_io(IoHooks { read: |_, _| Ok(cadaddy_engine::doc::Drawing::new_metric()), write: |_, _| Ok(b"saved".to_vec()), plot: None, read_pdf: None });
    let dir = std::env::temp_dir().join(format!("cadaddy-recover-open-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("plan.dxf");
    std::fs::write(&path, b"0\nEOF\n").unwrap();
    let path = path.to_string_lossy().to_string();
    let mut s = Session::new();
    s.execute("open", &json!({ "path": path })).unwrap();
    let tabs = s.docs.len();
    s.execute("recover", &json!({ "path": path })).unwrap();
    assert_eq!(s.docs.len(), tabs, "RECOVER of an open file opened a second tab");
    // With unsaved changes in the open copy, it asks to close that first.
    s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    let e = s.execute("recover", &json!({ "path": path })).unwrap_err().to_string();
    assert!(e.contains("unsaved changes"), "{e}");
    assert_eq!(s.docs.len(), tabs);
    std::fs::remove_dir_all(&dir).ok();
}

/// EXPLODE of an object that can't be exploded is still an undo step: U then undoes it, not the
/// command before (battle test 0.7.7e: U removed the circle drawn before it).
#[test]
fn u_after_a_failed_explode_keeps_the_circle() {
    let mut s = Session::new();
    s.cmdline("circle 0,0 5").unwrap();
    let n = s.doc().unwrap().model.len();
    s.cmdline("explode l").unwrap();
    s.input(cadaddy_engine::Input::Enter).ok();
    assert!(s.log.iter().any(|l| l.contains("not able to be exploded")), "{:?}", s.log);
    s.cmdline("u").unwrap();
    assert_eq!(s.doc().unwrap().model.len(), n, "U undid the CIRCLE");
}

/// A JSON parameter a command doesn't take is reported in `warnings` (tester 0.7.7e: a misspelt
/// `raduis` was silently ignored); correct ones give none.
#[test]
fn unknown_json_parameters_are_warned_about() {
    let mut s = Session::new();
    let r = s.execute("circle", &json!({"center": [0, 0], "radius": 2, "raduis": 5})).unwrap();
    let w = r["warnings"].as_array().cloned().unwrap_or_default();
    assert!(w.iter().any(|m| m.as_str().unwrap_or("").contains("raduis")), "{r}");
    let r = s.execute("circle", &json!({"center": [0, 0], "radius": 2})).unwrap();
    assert!(r.get("warnings").is_none(), "{r}");
}

/// Parameter names written with a note in brackets are known (`{name (e.g. …), at: …}`).
#[test]
fn documented_names_with_notes_are_not_warned_about() {
    let spec = cadaddy_engine::command_specs().iter().find(|c| c.id == "models3d.place").unwrap();
    assert!(spec.params.contains("{name ("), "the doc changed: {}", spec.params);
    let mut s = Session::new();
    if let Ok(r) = s.execute("models3d.place", &json!({"name": "M3_DESK", "at": [0, 0]})) {
        assert!(r.get("warnings").is_none(), "{r}");
    }
}
