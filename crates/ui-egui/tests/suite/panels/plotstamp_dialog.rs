//! The Plot Stamp dialog: File ▸ Plot Stamp... and typed PLOTSTAMP open it; OK writes the settings
//! PLOTSTAMP reads back; Cancel changes nothing; JSON calls keep running the command.

use cadaddy_engine::Session;
use cadaddy_ui_egui::menus::{self, Entry};
use cadaddy_ui_egui::{CadApp, Services};
use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{Queryable, by};
use serde_json::{Value, json};

fn harness() -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::default(), Services::default());
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

/// File ▸ Plot Stamp...'s command id.
fn menu_item(app: &CadApp) -> String {
    let file = menus::tree(app).into_iter().find(|(m, _)| m == "File").map(|(_, e)| e).unwrap_or_default();
    file.into_iter()
        .find_map(|e| match e {
            Entry::Item { label, id, .. } if label == "Plot Stamp..." => Some(id),
            _ => None,
        })
        .expect("File ▸ Plot Stamp...")
}

/// What PLOTSTAMP reports now (no options: nothing changes).
fn readback(app: &mut CadApp) -> Value {
    app.run("plotstamp", json!({})).unwrap()
}

#[test]
fn the_menu_opens_the_dialog_and_ok_writes_the_settings() {
    let mut h = harness();
    let id = menu_item(h.state());
    menus::activate(h.state_mut(), &id);
    h.run_steps(4);
    assert_eq!(h.state().ui.dialog.as_deref(), Some("plotstamp"));
    assert!(h.query_by_label("Plot stamp fields").is_some(), "the dialog is shown");

    h.get_by_role_and_label(Role::CheckBox, "Paper size").click();
    h.get_by_role_and_label(Role::CheckBox, "Device name").click();
    h.get_by_role_and_label(Role::CheckBox, "Single line plot stamp").click();
    h.get_by_role_and_label(Role::CheckBox, "Upside-down").click();
    h.get_by_role_and_label(Role::RadioButton, "Paper border").click();
    h.get_by_role_and_label(Role::CheckBox, "Stamp model space plots").click();
    h.run_steps(2);
    let user = || by().role(Role::MultilineTextInput);
    h.get(user()).focus();
    h.run_steps(2);
    h.get(user()).type_text("Issued for review");
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(4);
    assert!(h.state().ui.dialog.is_none(), "OK closes the dialog");

    let r = readback(h.state_mut());
    let s = &r["settings"];
    assert_eq!((s["paperSize"].as_bool(), s["deviceName"].as_bool(), s["singleLine"].as_bool()), (Some(true), Some(false), Some(false)), "{r}");
    assert_eq!((s["upsideDown"].as_bool(), s["fromPaperEdge"].as_bool(), s["model"].as_bool()), (Some(true), Some(true), Some(true)), "{r}");
    assert_eq!(s["userFields"], json!(["Issued for review"]), "{r}");
    assert_eq!((r["on"].as_bool(), r["layouts"].clone()), (Some(true), json!(["Model"])), "{r}");
    // Untouched settings keep their values.
    assert_eq!((s["font"].as_str(), s["heightMm"].as_f64(), s["offsetMm"].clone()), (Some("Helvetica"), Some(2.0), json!([2.0, 2.0])), "{r}");
}

#[test]
fn typed_plotstamp_opens_it_and_cancel_changes_nothing() {
    let mut h = harness();
    h.state_mut().cmdline("PLOTSTAMP");
    h.run_steps(4);
    assert_eq!(h.state().ui.dialog.as_deref(), Some("plotstamp"));
    h.get_by_role_and_label(Role::CheckBox, "Paper size").click();
    h.get_by_role_and_label(Role::CheckBox, "Drawing name").click();
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "Cancel").click();
    h.run_steps(4);
    assert!(h.state().ui.dialog.is_none());
    assert!(h.state().session.doc().unwrap().plot_stamp.is_none(), "Cancel wrote nothing");

    // Opened again, it shows the drawing's settings, not the cancelled edits.
    h.state_mut().cmdline("PLOTSTAMP");
    h.run_steps(4);
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(4);
    let r = readback(h.state_mut());
    assert_eq!((r["settings"]["paperSize"].as_bool(), r["settings"]["drawingName"].as_bool()), (Some(false), Some(true)), "{r}");
}

#[test]
fn json_calls_keep_running_the_command() {
    let mut h = harness();
    let r = h.state_mut().run("plotstamp", json!({ "plotScale": true, "units": "inches", "height": 0.1 })).unwrap();
    assert!(h.state().ui.dialog.is_none(), "JSON calls never open dialogs");
    assert_eq!((r["settings"]["plotScale"].as_bool(), r["settings"]["inches"].as_bool()), (Some(true), Some(true)), "{r}");
    assert!((r["settings"]["heightMm"].as_f64().unwrap() - 2.54).abs() < 1e-9, "{r}");
    // The dialog then shows the height in inches; OK keeps it.
    h.state_mut().cmdline("PLOTSTAMP");
    h.run_steps(4);
    assert!(h.query(by().value("Inches")).is_some() || h.query_by_label("Inches").is_some(), "the units shown");
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(4);
    let r = readback(h.state_mut());
    assert!((r["settings"]["heightMm"].as_f64().unwrap() - 2.54).abs() < 1e-6, "{r}");
}
