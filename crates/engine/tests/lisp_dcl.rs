//! AutoLISP dialogs and reactors: a DCL dialog runs headless through scripted tile actions
//! (`lisp {code, dialog}`), waits for the interface when typed (`lisp.dialog`, `lisp.tile`), and
//! reactors call their functions when a command ends or an object changes.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{EntityCodec, set_entity_codec};
use serde_json::{Value, json};

fn session() -> Session {
    set_entity_codec(EntityCodec {
        groups: cadaddy_io::entity_groups,
        entity: cadaddy_io::entity_from_groups,
        records: cadaddy_io::entity_records,
        attrib: cadaddy_io::attrib_from_groups,
    });
    let mut s = Session::new();
    s.execute("appload", &json!({ "name": "demo.dcl", "text": DCL })).unwrap();
    s
}

const DCL: &str = r#"
// A made-up dialog for a wall tag: every common tile.
demo : dialog {
    label = "Wall Tag";
    : boxed_column {
        label = "Tag";
        : edit_box { key = "name"; label = "Name:"; edit_width = 20; value = "W1"; }
        : toggle { key = "bold"; label = "Bold"; }
        : popup_list { key = "size"; label = "Size"; list = "Small\nLarge"; value = "0"; }
        : list_box { key = "items"; height = 4; }
        : radio_row { key = "side"; : radio_button { key = "left"; label = "Left"; value = "1"; } : radio_button { key = "right"; label = "Right"; } }
    }
    : text { key = "msg"; label = ""; }
    : image { key = "pic"; width = 10; height = 4; }
    : button { key = "go"; label = "Go"; }
    : button { key = "more"; label = "More..."; }
    ok_cancel;
}
confirm : dialog { label = "Sure?"; : text { label = "Really?"; } ok_cancel; }
"#;

const SETUP: &str = r#"
(defun run-demo (/ id st)
  (setq id (load_dialog "demo.dcl") clicks 0 result nil)
  (if (not (new_dialog "demo" id)) (exit))
  (start_list "items") (mapcar 'add_list '("one" "two" "three")) (end_list)
  (start_image "pic") (fill_image 0 0 (dimx_tile "pic") (dimy_tile "pic") 1) (vector_image 0 0 10 10 7) (end_image)
  (action_tile "go" "(setq clicks (1+ clicks)) (set_tile \"msg\" (strcat \"go \" (get_tile \"name\")))")
  (action_tile "more" "(setq inner (done-confirm id))")
  (action_tile "accept" "(setq result (mapcar 'get_tile '(\"name\" \"bold\" \"size\" \"items\" \"side\" \"msg\"))) (done_dialog 7)")
  (setq st (start_dialog))
  (unload_dialog id)
  (list st clicks result))
(defun done-confirm (id)
  (new_dialog "confirm" id)
  (start_dialog))
"#;

fn lisp(s: &mut Session, code: &str, dialog: Value) -> Value {
    s.execute("lisp", &json!({ "code": code, "dialog": dialog })).unwrap()
}

fn value(s: &mut Session, code: &str, dialog: Value) -> String {
    let r = lisp(s, code, dialog);
    assert!(r.get("error").is_none(), "{code}: {r}");
    r["value"].as_str().unwrap_or_default().to_string()
}

#[test]
fn a_dialog_runs_headless_through_scripted_tile_actions() {
    let mut s = session();
    value(&mut s, SETUP, json!([]));
    let script = json!([
        { "key": "name", "value": "Wall-B" },
        { "key": "go" },
        { "key": "bold" },
        { "key": "items", "value": "2" },
        { "key": "size", "value": "1" },
        { "key": "right" },
        { "key": "go" },
        { "key": "accept" }
    ]);
    assert_eq!(value(&mut s, "(run-demo)", script), r#"(7 2 ("Wall-B" "1" "1" "2" "right" "go Wall-B"))"#);
}

#[test]
fn defaults_ok_cancel_and_an_unfinished_script() {
    let mut s = session();
    value(&mut s, SETUP, json!([]));
    // Cancel's default action ends the dialog with 0; the tiles keep their DCL values.
    let code = r#"(progn (setq id (load_dialog "demo")) (new_dialog "demo" id)
        (list (start_dialog) (get_tile "name")))"#;
    let r = lisp(&mut s, code, json!([{ "key": "cancel" }]));
    assert!(r["error"].as_str().is_some_and(|e| e.contains("no dialog")), "get_tile after the dialog closed: {r}");
    let code = r#"(progn (setq id (load_dialog "demo")) (new_dialog "demo" id)
        (action_tile "go" "(setq seen (list (get_tile \"name\") (get_tile \"size\") (get_tile \"side\") (get_attr \"name\" \"edit_width\") $key $reason)) (done_dialog)")
        (list (start_dialog) seen))"#;
    assert_eq!(value(&mut s, code, json!([{ "key": "go" }])), r#"(1 ("W1" "0" "left" "20" "go" 1))"#);
    assert_eq!(value(&mut s, r#"(progn (new_dialog "demo" id) (start_dialog))"#, json!([{ "key": "cancel" }])), "0");
    // OK without an action of its own: status 1.
    assert_eq!(value(&mut s, r#"(progn (new_dialog "demo" id) (start_dialog))"#, json!([{ "key": "accept" }])), "1");
    // Nobody answers: cancelled, status 0, and the routine carries on.
    assert_eq!(value(&mut s, r#"(progn (new_dialog "demo" id) (list (start_dialog) 'after))"#, json!([{ "key": "go" }])), "(0 AFTER)");
    // A nested dialog from an action, then the outer one ends.
    let r = value(&mut s, "(progn (setq inner nil) (list (run-demo) inner))", json!([{ "key": "more" }, { "key": "accept" }, { "key": "accept" }]));
    assert_eq!(r, r#"((7 0 ("W1" "0" "0" "" "left" "")) 1)"#);
    // A disabled tile can't be clicked; term_dialog ends with -1.
    let code = r#"(progn (new_dialog "demo" id) (mode_tile "go" 1) (action_tile "go" "(setq hit t)") (action_tile "bold" "(term_dialog)") (setq hit nil) (list (start_dialog) hit))"#;
    let r = lisp(&mut s, code, json!([{ "key": "go" }, { "key": "bold" }]));
    assert_eq!(r["value"], "(-1 nil)", "{r}");
    assert!(r["output"].to_string().contains("disabled"), "{r}");
}

#[test]
fn a_typed_routine_waits_for_the_interface() {
    let mut s = session();
    value(&mut s, SETUP, json!([]));
    assert!(s.lisp_eval("(setq outcome (run-demo))", false, false, Vec::new()).unwrap().is_none(), "waits in the dialog");
    let st = s.execute("lisp.dialog", &json!({})).unwrap();
    assert_eq!(st["dialog"], "demo");
    let text = st.to_string();
    assert!(text.contains(r#""items":["one","two","three"]"#), "{text}");
    assert!(text.contains(r#""key":"name""#) && text.contains(r#""value":"W1""#), "{text}");
    let d = s.lisp_dialog().unwrap();
    assert_eq!(d.images["pic"].len(), 2);
    s.execute("lisp.tile", &json!({ "key": "name", "value": "Typed" })).unwrap();
    let after = s.execute("lisp.tile", &json!({ "key": "go" })).unwrap();
    assert!(after.to_string().contains("go Typed"), "{after}");
    assert!(s.execute("lisp.tile", &json!({ "key": "nothing" })).is_err());
    let done = s.execute("lisp.tile", &json!({ "key": "accept" })).unwrap();
    assert_eq!(done["dialog"], Value::Null);
    assert!(!s.lisp_running());
    assert_eq!(cadaddy_engine::lisp::value::to_text(&s.lisp_var("OUTCOME"), true), r#"(7 1 ("Typed" "0" "0" "" "left" "go Typed"))"#);
    // Esc in the dialog runs Cancel.
    assert!(s.lisp_eval("(setq outcome (run-demo))", false, false, Vec::new()).unwrap().is_none());
    s.dcl_cancel();
    assert!(s.lisp_dialog().is_none());
    assert_eq!(cadaddy_engine::lisp::value::to_text(&s.lisp_var("OUTCOME"), true), "(0 0 nil)");
}

#[test]
fn errors_close_the_dialog_and_bad_files_are_refused() {
    let mut s = session();
    let r = lisp(
        &mut s,
        r#"(progn (setq id (load_dialog "demo")) (new_dialog "demo" id) (action_tile "go" "(car 1)") (start_dialog))"#,
        json!([{ "key": "go" }]),
    );
    assert!(r["error"].as_str().is_some_and(|e| e.contains("bad argument")), "{r}");
    assert!(s.lisp_dialog().is_none());
    assert_eq!(value(&mut s, r#"(load_dialog "missing-file")"#, json!([])), "-1");
    assert_eq!(value(&mut s, r#"(new_dialog "nope" id)"#, json!([])), "nil");
    // A hostile file: deep nesting and self-reference are errors, not a crash.
    let deep = format!("d : dialog {{ {} {} }}", ": row {".repeat(500), "}".repeat(500));
    s.execute("appload", &json!({ "name": "deep.dcl", "text": deep })).unwrap();
    assert_eq!(value(&mut s, r#"(load_dialog "deep.dcl")"#, json!([])), "-1");
    s.execute("appload", &json!({ "name": "self.dcl", "text": "x : x { }\nd : dialog { x; }" })).unwrap();
    assert_eq!(value(&mut s, r#"(new_dialog "d" (load_dialog "self"))"#, json!([])), "nil");
    // Tile actions in a script that isn't a list of objects are refused.
    assert!(s.execute("lisp", &json!({ "code": "1", "dialog": [{ "value": "x" }] })).is_err());
}

#[test]
fn a_command_reactor_fires_when_a_command_ends() {
    let mut s = session();
    value(
        &mut s,
        r#"(progn (setq log nil)
            (defun on-end (r cmds) (setq log (cons (list 'end (car cmds) (vlr-data r)) log)))
            (defun on-start (r cmds) (setq log (cons (list 'start (car cmds)) log)))
            (setq cr (vlr-command-reactor "tag" '((:vlr-commandEnded . on-end) (:vlr-commandWillStart . on-start))))
            (list (vlr-type cr) (vlr-added-p cr) (length (vlr-reactions cr))))"#,
        json!([]),
    );
    s.start("regen").unwrap();
    assert_eq!(cadaddy_engine::lisp::value::to_text(&s.lisp_var("LOG"), true), r#"((END "REGEN" "tag") (START "REGEN"))"#);
    // An interactive command: when it ends, not when it starts taking input.
    value(&mut s, "(setq log nil)", json!([]));
    s.start("line").unwrap();
    s.input(cadaddy_engine::Input::Point(cadaddy_geom::Vec2::new(0.0, 0.0))).unwrap();
    s.input(cadaddy_engine::Input::Point(cadaddy_geom::Vec2::new(10.0, 0.0))).unwrap();
    assert_eq!(cadaddy_engine::lisp::value::to_text(&s.lisp_var("LOG"), true), r#"((START "LINE"))"#);
    s.input(cadaddy_engine::Input::Enter).unwrap();
    assert_eq!(cadaddy_engine::lisp::value::to_text(&s.lisp_var("LOG"), true), r#"((END "LINE" "tag") (START "LINE"))"#);
    // Removed, it is quiet; commands a routine runs don't set it off; `command` in a callback is refused.
    value(&mut s, "(progn (vlr-remove cr) (setq log nil))", json!([]));
    s.start("regen").unwrap();
    assert!(s.lisp_var("LOG").is_nil());
    value(&mut s, r#"(progn (vlr-add cr) (command "_regen") log)"#, json!([]));
    assert!(s.lisp_var("LOG").is_nil(), "a routine's own commands don't fire reactors");
    value(
        &mut s,
        r#"(progn (vlr-remove-all) (defun bad (r c) (setq err (vl-catch-all-error-message (vl-catch-all-apply 'command '("_regen"))))) (vlr-command-reactor nil '((:vlr-commandEnded . bad))))"#,
        json!([]),
    );
    s.start("regen").unwrap();
    assert!(cadaddy_engine::lisp::value::to_text(&s.lisp_var("ERR"), true).contains("reactor"));
}

#[test]
fn an_object_reactor_sees_its_object_modified_and_erased() {
    let mut s = session();
    let h = s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap();
    let handle = h["handle"].as_str().or_else(|| h["handles"][0].as_str()).unwrap().to_string();
    value(
        &mut s,
        r#"(progn (setq seen nil)
            (defun on-change (obj r args) (setq seen (cons (list (cdr (assoc 0 (entget obj))) (vlr-data r)) seen)))
            (defun on-erase (obj r args) (setq seen (cons 'erased seen)))
            (vlr-object-reactor (list (entlast)) "watch" '((:vlr-modified . on-change) (:vlr-erased . on-erase))))"#,
        json!([]),
    );
    s.execute("move", &json!({ "handles": [handle], "from": [0, 0], "to": [5, 5] })).unwrap();
    assert_eq!(cadaddy_engine::lisp::value::to_text(&s.lisp_var("SEEN"), true), r#"(("LINE" "watch"))"#);
    // A command that leaves it alone: nothing.
    s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    assert_eq!(cadaddy_engine::lisp::value::to_text(&s.lisp_var("SEEN"), true), r#"(("LINE" "watch"))"#);
    s.execute("erase", &json!({ "handles": [handle] })).unwrap();
    assert_eq!(cadaddy_engine::lisp::value::to_text(&s.lisp_var("SEEN"), true), r#"(ERASED ("LINE" "watch"))"#);
}
