use cadaddy_engine::Session;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;

use crate::{CadApp, Services};

/// Layers Walls (2 lines), Doors (1 line) and Notes (1 line, current layer), plus layer 0.
fn app() -> CadApp {
    let mut app = CadApp::new(Session::default(), Services::default());
    for n in ["Walls", "Doors", "Notes"] {
        app.run("layer.new", json!({ "name": n, "current": true })).unwrap();
        let count = if n == "Walls" { 2 } else { 1 };
        for i in 0..count {
            app.run("line", json!({ "points": [[0, i], [10, i]] })).unwrap();
        }
    }
    app
}

fn harness(app: CadApp) -> Harness<'static, CadApp> {
    Harness::builder().with_size(vec2(500.0, 600.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            crate::dialogs::show(app, ui.ctx());
        },
        app,
    )
}

fn on_layer(app: &CadApp, layer: &str) -> usize {
    app.session.doc().unwrap().model.iter().filter(|e| e.common.layer.eq_ignore_ascii_case(layer)).count()
}

#[test]
fn the_menu_opens_the_dialogs_but_programmatic_calls_never_do() {
    let mut app = app();
    app.start("laymrg");
    assert_eq!(app.ui.dialog.as_deref(), Some(super::MERGE));
    app.ui.dialog = None;
    app.start("laydel");
    assert_eq!(app.ui.dialog.as_deref(), Some(super::DELETE));
    app.ui.dialog = None;
    // With parameters the command just runs; typed on the command line it keeps its prompts.
    app.run("laymrg", json!({ "from": ["Doors"], "to": "Walls" })).unwrap();
    app.run("laydel", json!({ "names": ["Notes"] })).ok();
    assert_eq!(app.ui.dialog, None);
    app.cmdline("LAYDEL");
    assert_eq!(app.ui.dialog, None);
    assert!(app.session.prompting(), "the typed command asks its own questions");
}

#[test]
fn the_merge_dialog_moves_the_checked_layers_into_the_target() {
    let mut app = app();
    app.session.execute("layer.current", &json!({ "name": "0" })).unwrap();
    super::open(&mut app, false);
    let mut h = harness(app);
    h.run_steps(3);
    // Nothing checked yet: Merge is disabled.
    assert!(h.get_by_label("Merge").accesskit_node().is_disabled());
    h.get_by_label("Doors").click();
    h.get_by_label("Notes").click();
    h.run_steps(2);
    h.get_by_label_contains("Merging 2 layers moves 2 objects. Choose the target layer.");
    assert!(h.get_by_label("Merge").accesskit_node().is_disabled(), "no target yet");
    // Pick Walls as the target from the drop-down.
    h.get_by_role(egui::accesskit::Role::ComboBox).click();
    h.run_steps(2);
    // "Walls" is in the list (a check box) and in the open drop-down (a choice): click the choice.
    let choice = h.get_all_by_label("Walls").find(|n| n.accesskit_node().role() != egui::accesskit::Role::CheckBox).unwrap();
    choice.click();
    h.run_steps(2);
    h.get_by_label_contains("Merging 2 layers moves 2 objects to \"Walls\" and deletes the layers.");
    h.get_by_label("Merge").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(on_layer(app, "Walls"), 4, "Walls had 2; Doors' 1 and Notes' 1 moved in");
    assert_eq!(on_layer(app, "Doors") + on_layer(app, "Notes"), 0);
    let d = app.session.doc().unwrap();
    assert!(d.layer("Doors").is_none() && d.layer("Notes").is_none(), "source layers are gone");
    assert!(d.layer("Walls").is_some());
    assert_eq!(app.ui.dialog, None, "Confirm closes the dialog");
}

#[test]
fn the_delete_dialog_erases_the_checked_layers_and_their_objects() {
    let mut app = app();
    app.session.execute("layer.current", &json!({ "name": "0" })).unwrap();
    super::open(&mut app, true);
    let mut h = harness(app);
    h.run_steps(3);
    h.get_by_label("Walls").click();
    h.run_steps(2);
    h.get_by_label_contains("Deleting 1 layers erases 2 objects.");
    h.get_by_label("Delete").click();
    h.run_steps(3);
    let app = h.state();
    let d = app.session.doc().unwrap();
    assert!(d.layer("Walls").is_none());
    assert_eq!(on_layer(app, "Walls"), 0);
    assert!(d.layer("Doors").is_some() && d.layer("Notes").is_some(), "layers that weren't checked stay");
    assert_eq!((on_layer(app, "Doors"), on_layer(app, "Notes")), (1, 1));
    assert_eq!(d.model.iter().count(), 2, "only the two other lines are left");
}

#[test]
fn cancel_changes_nothing_and_the_filter_narrows_the_list() {
    let mut app = app();
    app.session.execute("layer.current", &json!({ "name": "0" })).unwrap();
    super::open(&mut app, true);
    let mut h = harness(app);
    h.run_steps(3);
    h.get_by_label("Doors").click();
    h.run_steps(2);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    let app = h.state();
    assert_eq!(app.ui.dialog, None);
    assert!(app.session.doc().unwrap().layer("Doors").is_some());
    assert_eq!(app.session.doc().unwrap().model.iter().count(), 4);

    // The filter hides layers that don't match; the current layer can't be checked for deletion.
    assert!(super::passes("wal", "Walls") && !super::passes("wal", "Doors"));
    assert!(super::passes("d*", "Doors") && !super::passes("d*", "Walls"));
    assert!(super::passes("", "Anything"));
}

#[test]
fn layers_that_cannot_be_removed_are_listed_but_disabled() {
    let mut app = app();
    // Notes is current after the setup: LAYDEL keeps it; layer 0 is never removed.
    super::open(&mut app, true);
    let mut h = harness(app);
    h.run_steps(3);
    assert!(h.get_by_label("Notes").accesskit_node().is_disabled());
    assert!(!h.get_by_label("Walls").accesskit_node().is_disabled());
    let d = h.state().session.doc().unwrap();
    assert!(cadaddy_engine::cmd::layer_removal_refusal(d, "0", true).is_some(), "layer 0 always stays");
    assert!(cadaddy_engine::cmd::layer_removal_refusal(d, "Walls", true).is_none());
}
