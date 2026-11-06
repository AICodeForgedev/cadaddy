//! An agent watching hears of a dialog once, when it opens.

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services, menus};
use serde_json::Value;

fn frames(app: &mut CadApp, ctx: &egui::Context, n: usize) {
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), ..Default::default() };
    for _ in 0..n {
        app.logic(ctx);
        let mut output = ctx.run_ui(input.clone(), |ui| app.ui(ui));
        output.textures_delta.clear();
    }
}

fn dialog_events(app: &mut CadApp, since: u64) -> (Vec<Value>, u64) {
    let r = app.session.events_poll(since);
    let next = r["next"].as_u64().unwrap();
    let evs = r["events"].as_array().unwrap().iter().filter(|e| e["type"] == "dialog.opened").cloned().collect();
    (evs, next)
}

#[test]
fn opening_a_dialog_is_announced_once() {
    let mut app = CadApp::new(Session::new(), Services::default());
    let ctx = egui::Context::default();
    frames(&mut app, &ctx, 2);
    let since = app.session.events_subscribe(true)["seq"].as_u64().unwrap();
    frames(&mut app, &ctx, 2);
    assert!(dialog_events(&mut app, since).0.is_empty(), "no dialog is open yet");

    menus::activate(&mut app, "dsettings");
    assert_eq!(app.ui.dialog.as_deref(), Some("dsettings"));
    frames(&mut app, &ctx, 3);
    let (evs, next) = dialog_events(&mut app, since);
    assert_eq!(evs.len(), 1, "{evs:?}");
    assert_eq!(evs[0]["dialog"], "dsettings");

    // More frames with the same dialog open say nothing more.
    frames(&mut app, &ctx, 5);
    assert!(dialog_events(&mut app, next).0.is_empty());
}
