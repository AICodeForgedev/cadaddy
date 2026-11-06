//! Workspaces (WSCURRENT, WORKSPACE, Tools ▸ Workspaces, the status bar's workspace menu) and
//! the navigation bar (NAVBARDISPLAY, NAVBAR).

use cadaddy_engine::{Session, sysvars};
use cadaddy_ui_egui::workspaces::{self, built_in};
use cadaddy_ui_egui::{CadApp, Services, menus};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};

fn app() -> CadApp {
    let mut app = CadApp::new(Session::default(), Services::default());
    app.run("new", json!({})).unwrap();
    app
}

fn wscurrent(app: &CadApp) -> String {
    sysvars::get(&app.session, "WSCURRENT").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

fn docking(app: &CadApp) -> Value {
    serde_json::to_value(&app.ui.docking).unwrap()
}

#[test]
fn built_in_workspaces_arrange_the_interface() {
    let mut app = app();
    assert_eq!(wscurrent(&app), "Drafting & Annotation");
    let r = app.run("workspace", Value::Null).unwrap();
    let names: Vec<&str> = r["workspaces"].as_array().unwrap().iter().map(|w| w["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Drafting & Annotation", "3D Modeling", "Electrical"]);

    menus::run_ui_command(&mut app, "ui.workspace.electrical", &Value::Null).unwrap().unwrap();
    assert_eq!(wscurrent(&app), "Electrical");
    assert!(!app.ui.show_toolsets);
    assert_eq!(app.ui.ribbon_tab, "Insert");
    assert!(workspaces::tab_shown(&app, "Annotate") && !workspaces::tab_shown(&app, "Parametric"));
    let electrical = built_in().into_iter().find(|d| d.name == "Electrical").unwrap();
    assert_eq!(docking(&app), serde_json::to_value(electrical.docking.unwrap()).unwrap(), "the Symbols palette out");

    app.run("workspace", json!({ "name": "3D Modeling" })).unwrap();
    assert!(app.ui.show_toolsets);
    assert_eq!(app.ui.toolset_tab, "Modeling");
    assert!(workspaces::tab_shown(&app, "Parametric"), "every tab again");
    assert!(app.run("workspace", json!({ "name": "Nope" })).is_err());
    assert_eq!(wscurrent(&app), "3D Modeling");
}

#[test]
fn user_workspaces_are_saved_switched_and_deleted() {
    let mut app = app();
    app.ui.show_toolsets = true;
    app.ui.ribbon_tab = "View".into();
    app.ui.ribbon_tabs = vec!["Home".into(), "View".into()];
    app.run("workspace", json!({ "action": "save", "name": "Mine" })).unwrap();
    assert_eq!(wscurrent(&app), "Mine");
    assert!(app.run("workspace", json!({ "action": "save", "name": "Electrical" })).is_err(), "a built-in name");
    assert!(app.run("workspace", json!({ "action": "save", "name": "  " })).is_err());

    app.run("workspace", json!({ "name": "Drafting & Annotation" })).unwrap();
    assert!(!app.ui.show_toolsets && app.ui.ribbon_tabs.is_empty() && app.ui.ribbon_tab == "Home");
    app.run("workspace", json!({ "name": "mine" })).unwrap();
    assert!(app.ui.show_toolsets && app.ui.ribbon_tab == "View" && app.ui.ribbon_tabs == ["Home", "View"]);

    // Kept with the interface state.
    let saved: cadaddy_ui_egui::UiState = serde_json::from_value(serde_json::to_value(&app.ui).unwrap()).unwrap();
    assert_eq!(saved.user_workspaces.len(), 1);
    assert_eq!(saved.user_workspaces[0].name, "Mine");

    assert!(app.run("workspace", json!({ "action": "delete", "name": "Electrical" })).is_err());
    app.run("workspace", json!({ "action": "delete", "name": "Mine" })).unwrap();
    assert_eq!(wscurrent(&app), "Drafting & Annotation", "the current one deleted: the default");
    assert!(app.ui.user_workspaces.is_empty());
}

#[test]
fn setting_wscurrent_applies_the_workspace() {
    let app = app();
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(2);
    // At start-up the interface stays as it was left.
    assert_eq!(h.state().ui.ribbon_tab, "Home");
    sysvars::set(&mut h.state_mut().session, "WSCURRENT", &json!("Electrical")).unwrap();
    h.run_steps(2);
    assert_eq!(h.state().ui.ribbon_tab, "Insert");
    // A name that isn't a workspace: it stays.
    sysvars::set(&mut h.state_mut().session, "WSCURRENT", &json!("No Such")).unwrap();
    h.run_steps(2);
    assert_eq!(wscurrent(h.state()), "Electrical");
    assert!(sysvars::set(&mut h.state_mut().session, "WSCURRENT", &json!("")).is_err());
}

#[test]
fn the_navigation_bar_shows_and_closes() {
    let mut app = app();
    assert!(workspaces::navbar_shown(&app), "NAVBARDISPLAY is 1 by default");
    app.run("navbar", json!({ "on": false })).unwrap();
    assert_eq!(sysvars::get(&app.session, "NAVBARDISPLAY"), Some(json!(0)));
    app.run("navbar", Value::Null).unwrap();
    assert!(workspaces::navbar_shown(&app), "no value: toggles");
    assert!(app.run("navbar", json!({ "on": 3 })).is_err());

    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    for label in ["Pan", "Zoom Extents", "Zoom Window", "Zoom Previous", "Orbit"] {
        assert!(h.query_all_by_label(label).count() >= 1, "{label}");
    }
    h.get_by_label("Close Navigation Bar").click();
    h.run_steps(3);
    assert!(!workspaces::navbar_shown(h.state()));
    assert!(h.query_by_label("Close Navigation Bar").is_none());
}

#[test]
fn the_profile_keeps_the_workspace_and_the_navigation_bar() {
    let mut app = app();
    app.run("workspace", json!({ "name": "Electrical" })).unwrap();
    app.run("navbar", json!({ "on": false })).unwrap();
    let profile = app.profile_json();
    let mut other = CadApp::new(Session::default(), Services::default());
    other.load_profile(&profile);
    assert_eq!(wscurrent(&other), "Electrical");
    assert!(!workspaces::navbar_shown(&other));
}
