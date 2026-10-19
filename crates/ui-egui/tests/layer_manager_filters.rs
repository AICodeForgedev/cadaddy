//! The Layer Properties Manager's filter tree (a group filter from the selected layer, a property
//! filter from the Layer Filter Properties window, Invert filter) and the Layer States Manager
//! (opened from it: restore mask, "turn off layers not in state", Restore, Close back).

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services};
use egui::accesskit::Role;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn harness() -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::default(), Services::default());
    for n in ["WALLS", "E-POWER"] {
        app.run("layer.new", json!({"name": n})).unwrap();
    }
    app.ui.in_window_menu = false;
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.state_mut().cmdline("layer");
    h.run_steps(4);
    h
}

/// Whether the layer table shows `name` (its row label).
fn shows(h: &Harness<'_, CadApp>, name: &str) -> bool {
    h.query_all_by_label(name).count() > 0
}

#[test]
fn group_and_property_filters_and_invert_in_the_layer_manager() {
    let mut h = harness();
    assert_eq!(h.state().ui.dialog.as_deref(), Some("layers"));
    // Select WALLS, make a group filter of it.
    h.get_by_label("WALLS").click();
    h.run_steps(3);
    h.get_by_label("New Group Filter").click();
    h.run_steps(4);
    let f = h.state().session.doc().unwrap().layer_filters.clone();
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].kind, cadaddy_engine::doc::LayerFilterKind::Group { layers: vec!["WALLS".into()] });
    // The new filter is chosen: only WALLS shows; inverted, everything else.
    assert!(shows(&h, "WALLS") && !shows(&h, "E-POWER"));
    h.get_by_role_and_label(Role::CheckBox, "Invert filter").click();
    h.run_steps(4);
    assert!(!shows(&h, "WALLS") && shows(&h, "E-POWER"));
    h.get_by_role_and_label(Role::CheckBox, "Invert filter").click();
    h.run_steps(3);
    // A property filter from the form (defaults: every layer), nested in the group filter.
    h.get_by_label("New Property Filter...").click();
    h.run_steps(3);
    assert!(h.query_by_label("Layer Filter Properties").is_some());
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.run_steps(4);
    let f = h.state().session.doc().unwrap().layer_filters.clone();
    assert_eq!(f.len(), 2, "{f:?}");
    assert_eq!(f[1].parent.as_deref(), Some("Group1"));
}

#[test]
fn layer_states_manager_from_the_layer_manager() {
    let mut h = harness();
    h.state_mut().run("layerstate.save", json!({"name": "Plan", "description": "plot set"})).unwrap();
    h.run_steps(2);
    h.get_by_label("Layer States Manager...").click();
    h.run_steps(4);
    assert_eq!(h.state().ui.dialog.as_deref(), Some("layerstates"));
    assert!(h.query_by_label("plot set").is_some(), "the description is listed");
    h.get_by_label("Plan").click();
    // The window grows to show the state's settings; let it settle.
    h.run_steps(10);
    // Don't restore colours; turn off the layers the state doesn't have.
    h.get_by_role_and_label(Role::CheckBox, "Color").click();
    h.run_steps(3);
    h.get_by_role_and_label(Role::CheckBox, "Turn off layers not found in layer state").click();
    h.run_steps(3);
    let st = h.state().session.doc().unwrap().layer_states[0].clone();
    assert!(!st.restore.color && st.off_others, "{st:?} {:?}", h.state().layer_states_dlg);
    // A layer made after saving goes off on Restore.
    h.state_mut().run("layer.new", json!({"name": "LATER"})).unwrap();
    h.run_steps(2);
    h.get_by_role_and_label(Role::Button, "Restore").click();
    h.run_steps(3);
    assert!(!h.state().session.doc().unwrap().layer("LATER").unwrap().on);
    // Close goes back to the Layer Properties Manager.
    h.get_by_role_and_label(Role::Button, "Close").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.dialog.as_deref(), Some("layers"));
    assert!(h.state().layer_states_dlg.is_none());
}

#[test]
fn typed_layerstate_opens_the_manager_and_escape_closes_it() {
    let mut h = harness();
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
    h.state_mut().cmdline("layerstate");
    h.run_steps(3);
    assert_eq!(h.state().ui.dialog.as_deref(), Some("layerstates"));
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.state().ui.dialog.is_none() && h.state().layer_states_dlg.is_none());
}
