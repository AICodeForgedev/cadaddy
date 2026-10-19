//! 3DWALK and 3DFLY: walk mode opens in the drawing's own tab (the palettes stay), walking or
//! flying, switching between the two; room names take the walker there; Esc goes back.

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn harness() -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::default(), Services::default());
    app.run("layer.new", json!({"name": "A-WALL", "current": true})).unwrap();
    for (a, b) in [([120, 120], [600, 120]), ([600, 120], [600, 240]), ([600, 240], [120, 240]), ([120, 240], [120, 120])] {
        app.run("line", json!({"points": [a, b]})).unwrap();
    }
    app.run("layer.new", json!({"name": "A-ANNO", "current": true})).unwrap();
    app.run("text", json!({"at": [480, 180], "text": "OFFICE", "height": 6})).unwrap();
    app.ui.in_window_menu = false;
    app.ui.show_palettes = true;
    let mut h = Harness::builder().with_size(vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

#[test]
fn walk_and_fly_open_in_the_drawing_tab_and_switch() {
    let mut h = harness();
    h.state_mut().cmdline("3dwalk");
    h.run_steps(3);
    let w = h.state().walk.as_ref().expect("walk mode is open");
    assert!(!w.walker.fly);
    // The drawing's tab holds it: the palettes are still there beside it.
    assert!(h.query_all_by_label("Properties").count() > 0, "palettes stay docked");
    assert!(h.query_by_label("Go to room").is_some());
    // 3DFLY switches to flying without leaving.
    h.state_mut().cmdline("3dfly");
    h.run_steps(2);
    assert!(h.state().walk.as_ref().unwrap().walker.fly);
    h.state_mut().cmdline("3dwalk");
    h.run_steps(2);
    assert!(!h.state().walk.as_ref().unwrap().walker.fly);
    // JSON calls report the mode and speed settings.
    let r = h.state_mut().session.execute("3dwalk", &json!({})).unwrap();
    assert_eq!((r["mode"].as_str(), r["stepSize"].as_f64(), r["stepsPerSec"].as_f64()), (Some("walk"), Some(6.0), Some(2.0)));
}

#[test]
fn room_names_take_the_walker_there_and_speed_follows_stepsize() {
    let mut h = harness();
    h.state_mut().run("setvar", json!({"name": "STEPSIZE", "value": 24})).unwrap();
    h.state_mut().run("setvar", json!({"name": "STEPSPERSEC", "value": 3})).unwrap();
    h.state_mut().cmdline("3dwalk");
    h.run_steps(3);
    let w = h.state_mut().walk.as_mut().unwrap();
    // 24" × 3 a second = 6 ft/s.
    assert!((w.speed_ft - 6.0).abs() < 1e-9, "{}", w.speed_ft);
    let i = w.rooms.iter().position(|r| r.name == "OFFICE").expect("the room name is offered");
    w.go_to_room(i);
    assert!((w.walker.eye.x - 30.0).abs() < 1e-6 && (w.walker.eye.y - 5.0).abs() < 1e-6, "{:?}", w.walker.eye);
    // Clicking the room in the panel does the same.
    h.get_by_label("OFFICE  ·  Level 1").click();
    h.run_steps(3);
    assert!((h.state().walk.as_ref().unwrap().walker.eye.x - 30.0).abs() < 1e-6);
}

#[test]
fn escape_goes_back_to_the_drawing() {
    let mut h = harness();
    h.state_mut().cmdline("3dwalk");
    h.run_steps(3);
    assert!(h.state().walk.is_some());
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.state().walk.is_none());
}
