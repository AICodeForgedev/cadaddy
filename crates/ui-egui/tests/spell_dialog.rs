//! The Check Spelling dialog: typed SPELL opens it (-SPELL keeps the command line), Start finds
//! the words not in the dictionary, Change takes the suggestion shown, Ignore moves on, and the
//! check ends with a message.

use cadaddy_engine::Session;
use cadaddy_engine::doc::EntityKind;
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn harness(text: &str) -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::default(), Services::default());
    let dir = std::env::temp_dir().join(format!("cadaddy-spelldlg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    app.run("setvar", json!({"name": "DCTCUST", "value": dir.join("custom.cus").to_str().unwrap()})).unwrap();
    app.run("text", json!({"at": [0, 0], "text": text, "height": 1})).unwrap();
    app.ui.in_window_menu = false;
    let mut h = Harness::builder().with_size(vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn value(app: &CadApp) -> String {
    app.session.doc().unwrap().model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.value.clone()) } else { None }).unwrap()
}

#[test]
fn spell_dialog_changes_and_ignores_words() {
    let mut h = harness("Wirring for the trnasformer pad");
    h.state_mut().cmdline("spell");
    h.run_steps(3);
    assert_eq!(h.state().ui.dialog.as_deref(), Some("spell"));
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Start").click();
    h.run_steps(4);
    assert!(h.query_by_label("Wirring").is_some(), "the first word not in the dictionary is shown");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Change").click();
    h.run_steps(4);
    assert_eq!(value(h.state()), "Wiring for the trnasformer pad");
    assert!(h.query_by_label("trnasformer").is_some());
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Ignore").click();
    h.run_steps(4);
    assert!(h.query_by_label("Spelling check complete.").is_some());
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Close").click();
    h.run_steps(3);
    assert!(h.state().ui.dialog.is_none() && h.state().spell_dlg.is_none());
}

#[test]
fn minus_spell_keeps_the_command_line() {
    let mut h = harness("Wirring");
    h.state_mut().cmdline("-spell");
    h.run_steps(2);
    assert!(h.state().ui.dialog.is_none());
    assert!(h.state().session.prompt_text().contains("Select objects"), "{}", h.state().session.prompt_text());
}

/// The main dictionary is chosen in the dialog (Q146): the languages are listed, one not
/// installed says so, and choosing it sets the drawing's DCTMAIN.
#[test]
fn spell_dialog_chooses_the_main_dictionary() {
    let mut h = harness("Wirring");
    h.state_mut().cmdline("spell");
    h.run_steps(3);
    // The combo box shows the current language as its value.
    h.get_by(|n| n.value().as_deref() == Some("American English")).click();
    h.run_steps(3);
    h.get_by_label("German (not installed)").click();
    h.run_steps(3);
    assert_eq!(h.state_mut().run("getvar", json!({"name": "DCTMAIN"})).unwrap()["value"], "de");
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Start").click();
    h.run_steps(4);
    assert!(h.query_by_label_contains("isn't installed").is_some(), "Start says where the German list goes");
}
