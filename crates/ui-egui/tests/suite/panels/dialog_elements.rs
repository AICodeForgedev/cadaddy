//! `ui.elements` names the controls of dialogs too (UI review 0.7.7b: agents couldn't find a
//! dialog's buttons by label), and Drafting Settings shows its sections as tabs.

use cadaddy_engine::Session;
use cadaddy_ui_egui::control::{ControlRequest, Outcome, handle};
use cadaddy_ui_egui::{CadApp, Services};
use serde_json::{Value, json};

fn call(app: &mut CadApp, ctx: &egui::Context, method: &str, params: Value) -> Value {
    let (req, _reply) = ControlRequest::new(method, params);
    match handle(app, ctx, &req) {
        Outcome::Done(v) => {
            assert_eq!(v["ok"], true, "{v}");
            v["result"].clone()
        }
        _ => panic!("unexpected screenshot"),
    }
}

fn frame(app: &mut CadApp, ctx: &egui::Context) {
    let mut raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))), ..Default::default() };
    app.raw_input_hook(&mut raw);
    cadaddy_ui_egui::cmdline::capture_tab(ctx, &mut raw);
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
}

fn pump(app: &mut CadApp, ctx: &egui::Context) {
    for _ in 0..4 {
        frame(app, ctx);
    }
}

fn centre(w: &Value) -> (f64, f64) {
    let r = w["rect"].as_array().cloned().unwrap_or_default();
    let f = |i: usize| r.get(i).and_then(Value::as_f64).unwrap_or(0.0);
    (f(0) + f(2) / 2.0, f(1) + f(3) / 2.0)
}

#[test]
fn dialog_controls_are_reported_and_drafting_settings_has_tabs() {
    let ctx = egui::Context::default();
    let mut app = CadApp::new(Session::new(), Services::default());
    call(&mut app, &ctx, "cmdline.state", json!({}));
    app.ui.dialog = Some("dsettings".into());
    pump(&mut app, &ctx);
    let tab = call(&mut app, &ctx, "ui.elements", json!({ "label": "Object Snap", "role": "button" }));
    // The dialog's tab, not a status bar or ribbon control of the same name: it sits mid-window.
    let Some(tab) = tab.as_array().into_iter().flatten().find(|w| (150.0..650.0).contains(&centre(w).1)).cloned() else {
        panic!("no Object Snap tab observed: {tab}")
    };
    // The first tab shows snap and grid only.
    assert_eq!(call(&mut app, &ctx, "ui.elements", json!({ "label": "Object Snap On (F3)" })), json!([]));
    let (x, y) = centre(&tab);
    call(&mut app, &ctx, "ui.click", json!({ "x": x, "y": y }));
    pump(&mut app, &ctx);
    let on = call(&mut app, &ctx, "ui.elements", json!({ "label": "Object Snap On (F3)", "role": "checkbox" }));
    assert_eq!(on.as_array().map(Vec::len), Some(1), "{on}");
}
