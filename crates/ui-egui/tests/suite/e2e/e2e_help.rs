//! End to end: help in the app. F1 over a menu item or toolbar button opens that command's page
//! in the Command Reference, F1 during a command opens the running command's page, and HELP / ?
//! typed at the command line opens the page of the command named (or the running one). Agents
//! still get the engine's JSON page from `engine.execute`.

use crate::e2e_common::E2e;
use serde_json::{Value, json};

/// The command whose page the Command Reference shows (`ui.inspect`'s `helpPage`).
fn help_page(e: &mut E2e) -> Option<String> {
    e.ok("ui.inspect", json!({}))["helpPage"].as_str().map(str::to_string)
}

/// The centre of the first control labelled `label` whose role is `role`, on screen.
fn centre(e: &mut E2e, label: &str, role: Option<&str>) -> Option<(f64, f64)> {
    let x = e.elements().into_iter().find(|x| x["label"].as_str() == Some(label) && role.is_none_or(|r| x["role"].as_str() == Some(r)))?;
    let r: Vec<f64> = x["rect"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
    let [x, y, w, h] = r.as_slice() else { return None };
    Some((x + w / 2.0, y + h / 2.0))
}

fn labels(e: &mut E2e) -> Vec<String> {
    e.elements().iter().filter_map(|x| Some(format!("{}:{}", x["role"].as_str()?, x["label"].as_str()?))).collect()
}

fn f1(e: &mut E2e) {
    e.ok("ui.key", json!({ "key": "F1" }));
}

/// The pointer over the drawing, away from every menu and button.
fn pointer_on_canvas(e: &mut E2e) {
    e.ok("ui.move", json!({ "x": 640.0, "y": 420.0 }));
}

#[test]
fn f1_over_a_menu_item_opens_its_page() {
    let mut e = E2e::new();
    e.ok("ui.set", json!({ "inWindowMenu": true }));
    assert!(e.click("Modify"), "the Modify menu is shown: {:?}", labels(&mut e));
    let Some((x, y)) = centre(&mut e, "Erase", None) else { panic!("Modify > Erase is shown: {:?}", labels(&mut e)) };
    e.ok("ui.move", json!({ "x": x, "y": y }));
    f1(&mut e);
    assert_eq!(e.dialog().as_deref(), Some("commands"));
    assert_eq!(help_page(&mut e).as_deref(), Some("erase"));
    // Nothing was started by F1.
    assert!(e.ok("cmdline.state", json!({}))["running"].is_null());
}

#[test]
fn f1_over_a_toolbar_button_opens_its_page() {
    let mut e = E2e::new();
    // A button of the Home tab's Draw panel (its tooltip names it).
    let Some((x, y)) = centre(&mut e, "Circle", None).or_else(|| centre(&mut e, "Center, Radius", None)) else {
        panic!("a Circle button is shown: {:?}", labels(&mut e))
    };
    e.ok("ui.move", json!({ "x": x, "y": y }));
    f1(&mut e);
    assert_eq!(e.dialog().as_deref(), Some("commands"));
    let page = help_page(&mut e).unwrap_or_default();
    assert!(page.starts_with("circle"), "{page}");
}

#[test]
fn f1_during_line_opens_the_line_page() {
    let mut e = E2e::new();
    e.typed("line");
    e.typed("0,0");
    pointer_on_canvas(&mut e);
    f1(&mut e);
    assert_eq!(e.dialog().as_deref(), Some("commands"));
    assert_eq!(help_page(&mut e).as_deref(), Some("line"));
    // LINE keeps running behind the page.
    assert_eq!(e.ok("cmdline.state", json!({}))["running"], "line");
}

#[test]
fn f1_with_nothing_running_or_hovered_opens_the_list() {
    let mut e = E2e::new();
    pointer_on_canvas(&mut e);
    f1(&mut e);
    assert_eq!(e.dialog().as_deref(), Some("commands"));
    assert_eq!(help_page(&mut e), None);
}

#[test]
fn typed_help_opens_the_named_page() {
    let mut e = E2e::new();
    e.typed("help circle");
    assert_eq!(e.dialog().as_deref(), Some("commands"));
    assert_eq!(help_page(&mut e).as_deref(), Some("circle"));
    e.menu("ui.dialog.close");

    // `?` is HELP; an alias names its command.
    e.typed("? l");
    assert_eq!(help_page(&mut e).as_deref(), Some("line"));
    e.menu("ui.dialog.close");

    // HELP alone with nothing running: the list, still on the page looked at last.
    e.typed("help");
    assert_eq!(e.dialog().as_deref(), Some("commands"));
    assert_eq!(help_page(&mut e).as_deref(), Some("line"));
    e.menu("ui.dialog.close");

    // A name that is no command: the list, and the command line says so.
    e.typed("help nosuch");
    assert_eq!(e.dialog().as_deref(), Some("commands"));
    assert!(e.history().iter().any(|l| l == "Unknown command \"NOSUCH\"."), "{:?}", e.history());
    e.menu("ui.dialog.close");

    // Help > Help in the menus opens it too.
    e.menu("help");
    assert_eq!(e.dialog().as_deref(), Some("commands"));
}

#[test]
fn transparent_help_opens_the_running_commands_page() {
    let mut e = E2e::new();
    e.typed("circle");
    e.typed("'help");
    assert_eq!(help_page(&mut e).as_deref(), Some("circle"));
    assert_eq!(e.ok("cmdline.state", json!({}))["running"], "circle", "CIRCLE goes on");
    e.menu("ui.dialog.close");
    // 'HELP with a name during a command: that page.
    e.typed("'? zoom");
    assert_eq!(help_page(&mut e).as_deref(), Some("zoom"));
}

/// During a command a bare ? is an answer to its prompt (-LAYER's list option), not help.
#[test]
fn a_bare_question_mark_answers_the_prompt() {
    let mut e = E2e::new();
    e.typed("-layer");
    e.typed("?");
    assert_eq!(e.dialog(), None);
}

/// The JSON form stays the engine's: agents get the page as data, no dialog opens.
#[test]
fn agents_get_the_page_as_json() {
    let mut e = E2e::new();
    let v = e.exec("help", json!({ "command": "c" }));
    assert_eq!(v["id"], "CIRCLE");
    assert_eq!(v["options"], json!(["3P", "2P", "Ttr (tan tan radius)"]));
    assert_eq!(e.dialog(), None);
    e.typed(r#"help {"command": "zoom"}"#);
    assert_eq!(e.dialog(), None);
}

/// The page shows the options and first prompt the engine's page has.
#[test]
fn the_page_shows_options_and_the_first_prompt() {
    let mut e = E2e::new();
    e.typed("help circle");
    let shown = labels(&mut e);
    let has = |t: &str| shown.iter().any(|l| l == t);
    assert!(has("text:3P / 2P / Ttr (tan tan radius)"), "{shown:?}");
    assert!(has("text:Specify center point for circle or [3P/2P/Ttr (tan tan radius)]:"), "{shown:?}");
    assert!(has("text:C"), "the alias: {shown:?}");
}

/// The CIRCLE page drawn for real (saved as help_page.png when HELP_SHOT names a folder).
#[test]
fn the_circle_page_renders() {
    let mut app = cadaddy_ui_egui::CadApp::new(cadaddy_engine::Session::new(), cadaddy_ui_egui::Services::default());
    app.cmdline("help circle");
    assert_eq!(app.ui.dialog.as_deref(), Some("commands:circle"));
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).wgpu().build_ui_state(
        |ui, app: &mut cadaddy_ui_egui::CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(6);
    let img = harness.render().unwrap();
    assert!(img.width() == 1280 || img.width() == 2560, "{}", img.width());
    if let Some(dir) = std::env::var_os("HELP_SHOT") {
        img.save(std::path::Path::new(&dir).join("help_page.png")).unwrap();
    }
}
