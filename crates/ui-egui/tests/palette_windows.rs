//! Palettes float in windows of their own (Window ▸ Palette Windows, the Properties menu, or
//! `ui.dock` detach/dock/moveDetached), remember where that window was and on which screen, and
//! dock back to where they were. Where there is only one window (these tests, the web) the
//! palette's window shows inside the main one.

use cadaddy_engine::Session;
use cadaddy_ui_egui::docking::{Detached, Panel, Workspace};
use cadaddy_ui_egui::{CadApp, Services, UiState};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn harness(state: Option<UiState>) -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::default(), Services::default());
    app.run("circle", json!({"center":[0,0],"radius":5})).unwrap();
    if let Some(state) = state {
        app.ui = state;
    }
    app.ui.in_window_menu = false;
    let mut harness = Harness::builder().with_size(vec2(1280.0, 760.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(5);
    harness
}

fn dock(h: &mut Harness<'_, CadApp>, params: serde_json::Value) -> Result<serde_json::Value, String> {
    let r = h.state_mut().run("ui.dock", params);
    h.run_steps(4);
    r
}

#[test]
fn detach_and_dock_back_to_the_same_place() {
    let mut h = harness(None);
    let before = h.state().ui.docking.layout.location(&Panel::Layers).unwrap();
    dock(&mut h, json!({"operation":"detach","panel":"layers","rect":[1900,200,340,600]})).unwrap();
    let ws = &h.state().ui.docking;
    assert!(!ws.layout.contains(&Panel::Layers) && ws.is_detached(Panel::Layers));
    assert_eq!(ws.detached[0].rect, [1900.0, 200.0, 340.0, 600.0]);
    assert!(h.state().ui.show_palettes, "a palette in its own window is still shown");
    // Its window shows (inside the main one here) with a Dock button.
    assert!(h.query_by_label("Dock").is_some());
    h.get_by_label("Dock").click();
    h.run_steps(5);
    let ws = &h.state().ui.docking;
    assert!(!ws.is_detached(Panel::Layers) && ws.detached.is_empty());
    assert_eq!(ws.layout.location(&Panel::Layers).unwrap(), before);
}

#[test]
fn palette_window_place_and_screen_are_remembered_across_a_reload() {
    let mut h = harness(None);
    h.state_mut().run("ui.float.properties", json!(null)).unwrap();
    h.run_steps(3);
    dock(&mut h, json!({"operation":"moveDetached","panel":"properties","rect":[-1500,40,300,700]})).unwrap();
    let mut ws = h.state().ui.docking.clone();
    ws.move_detached(Panel::Properties, [-1500.0, 40.0, 300.0, 700.0], Some([1920.0, 1080.0])).unwrap();
    let text = serde_json::to_string(&ws).unwrap();
    let back: Workspace = serde_json::from_str(&text).unwrap();
    back.validate().unwrap();
    assert_eq!(back.detached, vec![Detached { panel: Panel::Properties, rect: [-1500.0, 40.0, 300.0, 700.0], monitor: Some([1920.0, 1080.0]) }]);
    // A saved layout from before palette windows existed still loads.
    let old: serde_json::Value = serde_json::to_value(Workspace::default()).unwrap();
    assert!(old.get("detached").is_none());
    let loaded: Workspace = serde_json::from_value(old).unwrap();
    assert!(loaded.detached.is_empty());

    let mut state = h.state().ui.clone();
    state.docking = back;
    let mut h2 = harness(Some(state));
    assert!(h2.state().ui.docking.is_detached(Panel::Properties));
    // The menu item toggles it back.
    h2.state_mut().run("ui.float.properties", json!(null)).unwrap();
    h2.run_steps(3);
    assert!(h2.state().ui.docking.layout.contains(&Panel::Properties));
}

#[test]
fn open_close_and_the_palette_switches_respect_palette_windows() {
    let mut h = harness(None);
    dock(&mut h, json!({"operation":"detach","panel":"properties"})).unwrap();
    // Opening it again just leaves its window open; arranging it in the main window needs a dock.
    dock(&mut h, json!({"operation":"open","panel":"properties"})).unwrap();
    assert!(h.state().ui.docking.is_detached(Panel::Properties));
    assert!(dock(&mut h, json!({"operation":"setStackOpen","panel":"properties","open":true})).is_err());
    // Closing the window keeps its place: opening restores it in the main window.
    dock(&mut h, json!({"operation":"close","panel":"properties"})).unwrap();
    assert!(!h.state().ui.docking.is_detached(Panel::Properties));
    dock(&mut h, json!({"operation":"open","panel":"properties"})).unwrap();
    assert!(h.state().ui.docking.layout.contains(&Panel::Properties));
    // Turning the palettes off closes their windows too.
    dock(&mut h, json!({"operation":"detach","panel":"layers"})).unwrap();
    h.state_mut().ui.show_palettes = false;
    dock(&mut h, json!({"operation":"activate","panel":"canvas"})).unwrap();
    let ws = &h.state().ui.docking;
    assert!(ws.detached.is_empty() && !ws.layout.contains(&Panel::Layers) && !ws.layout.contains(&Panel::Properties));
}

#[test]
fn hostile_palette_window_requests_and_saved_settings_are_refused() {
    let mut h = harness(None);
    let before = h.state().ui.docking.clone();
    for bad in [
        json!({"operation":"detach","panel":"canvas"}),
        json!({"operation":"detach","panel":"layers","rect":[0,0,1e30,400]}),
        json!({"operation":"detach","panel":"layers","rect":[0,0,10,10]}),
        json!({"operation":"detach","panel":"layers","rect":[0,0,300]}),
        json!({"operation":"detach","panel":"layers","rect":"x"}),
        json!({"operation":"dock","panel":"layers"}),
        json!({"operation":"moveDetached","panel":"layers","rect":[0,0,300,300]}),
        json!({"operation":"detach","panel":"nope"}),
    ] {
        assert!(dock(&mut h, bad.clone()).is_err(), "{bad}");
    }
    assert_eq!(serde_json::to_value(&h.state().ui.docking).unwrap(), serde_json::to_value(&before).unwrap());
    let mut ws = Workspace::default();
    ws.detached.push(Detached { panel: Panel::Layers, rect: [0.0, 0.0, 300.0, 300.0], monitor: None });
    assert!(ws.validate().is_err(), "in the layout and in a window at once");
    let mut ws = Workspace::default();
    ws.detach(Panel::Layers, [0.0, 0.0, 300.0, 300.0], None).unwrap();
    ws.detached.push(ws.detached[0].clone());
    assert!(ws.validate().is_err(), "twice");
    let mut ws = Workspace::default();
    ws.detach(Panel::Layers, [0.0, 0.0, 300.0, 300.0], None).unwrap();
    ws.detached[0].rect[0] = f32::NAN;
    assert!(ws.validate().is_err(), "not finite");
}
