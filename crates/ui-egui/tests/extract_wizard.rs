//! The Data Extraction wizard: typed DATAEXTRACTION opens it (-DATAEXTRACTION keeps the command
//! line), its pages build the command's parameters, Preview shows rows, and Finish inserts a
//! table that DATAEXTRACTION Update can fill again.

use cadaddy_engine::Session;
use cadaddy_engine::doc::EntityKind;
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn harness() -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::default(), Services::default());
    for (a, b) in [([0, 0], [10, 0]), ([0, 5], [10, 5]), ([0, 9], [20, 9])] {
        app.run("line", json!({"points": [a, b]})).unwrap();
    }
    app.run("circle", json!({"center": [40, 40], "radius": 2})).unwrap();
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

fn tables(app: &CadApp) -> Vec<cadaddy_engine::doc::Table> {
    app.session.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Table(t) = &e.kind { Some(t.clone()) } else { None }).collect()
}

#[test]
fn typed_dataextraction_opens_the_wizard_and_finish_inserts_a_table() {
    let mut h = harness();
    h.state_mut().cmdline("dataextraction");
    h.run_steps(3);
    assert_eq!(h.state().ui.dialog.as_deref(), Some("dataextraction"));
    assert!(h.state().session.current_prompt().is_none(), "the wizard, not the command line");
    // The drawing's object types are offered.
    h.get_by_label("2. Objects").click();
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Line").click();
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Blocks").click();
    h.run_steps(2);
    h.get_by_label("3. Properties").click();
    h.run_steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::CheckBox, "Length").click();
    h.run_steps(2);
    h.get_by_label("Next >").click();
    h.run_steps(3);
    h.get_by_label("Preview").click();
    h.run_steps(4);
    // Two rows: the two 10-unit lines counted once, the 20-unit line.
    assert!(h.query_by_label("Count").is_some() && h.query_by_label("20").is_some());
    let p = h.state().extract_dlg.as_ref().unwrap().params(true);
    assert_eq!(p["types"], json!(["Line"]));
    assert_eq!(p["properties"], json!(["Length"]));
    assert_eq!(p["includeBlocks"], false);
    h.get_by_label("Finish").click();
    h.run_steps(4);
    assert!(h.state().ui.dialog.is_none() && h.state().extract_dlg.is_none());
    let t = tables(h.state());
    assert_eq!(t.len(), 1);
    // Header and two rows; Update after a new line adds its row.
    assert_eq!(t[0].cells.len(), 3);
    h.state_mut().run("line", json!({"points": [[0, 30], [7, 30]]})).unwrap();
    h.state_mut().run("dataextraction", json!({"update": true})).unwrap();
    assert_eq!(tables(h.state())[0].cells.len(), 4);
}

#[test]
fn minus_dataextraction_keeps_the_command_line_and_cancel_changes_nothing() {
    let mut h = harness();
    h.state_mut().cmdline("-dataextraction");
    h.run_steps(2);
    assert!(h.state().ui.dialog.is_none());
    assert!(h.state().session.prompt_text().contains("Enter block names to extract"), "{}", h.state().session.prompt_text());
    h.state_mut().session.cancel();
    let before = h.state().session.doc().unwrap().model.len();
    h.state_mut().cmdline("eattext");
    h.run_steps(3);
    assert_eq!(h.state().ui.dialog.as_deref(), Some("dataextraction"));
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(h.state().ui.dialog.is_none());
    assert_eq!(h.state().session.doc().unwrap().model.len(), before);
}
