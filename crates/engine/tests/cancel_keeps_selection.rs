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
