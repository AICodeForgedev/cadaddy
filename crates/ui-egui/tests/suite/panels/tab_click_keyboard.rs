//! Clicking a drawing or layout tab leaves the keyboard with the command line: a command typed
//! next starts (it used to go nowhere, the tab holding keyboard focus).

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

#[test]
fn typing_after_a_tab_click_reaches_the_command_line() {
    let ctx = egui::Context::default();
    let mut app = CadApp::new(Session::new(), Services::default());
    pump(&mut app, &ctx);
    let tabs = call(&mut app, &ctx, "ui.elements", json!({ "role": "tab" }));
    let tabs = tabs.as_array().cloned().unwrap_or_default();
    assert!(!tabs.is_empty(), "no tabs observed");
    // Drawing and layout tabs (the Start tab shows the start page, with no drawing to type into).
    for tab in tabs.iter().filter(|t| t["label"] != "Start").take(4) {
        let r = tab["rect"].as_array().cloned().unwrap_or_default();
        let f = |i: usize| r.get(i).and_then(Value::as_f64).unwrap_or(0.0);
        let (x, y) = (f(0) + f(2) / 2.0, f(1) + f(3) / 2.0);
        call(&mut app, &ctx, "ui.click", json!({ "x": x, "y": y }));
        pump(&mut app, &ctx);
        assert!(!ctx.egui_wants_keyboard_input(), "the tab {} kept keyboard focus", tab["label"]);
        call(&mut app, &ctx, "ui.text", json!({ "text": "line" }));
        call(&mut app, &ctx, "ui.key", json!({ "key": "Enter" }));
        pump(&mut app, &ctx);
        let st = call(&mut app, &ctx, "cmdline.state", json!({}));
        assert_eq!(st["running"], "line", "after clicking tab {}: {st}", tab["label"]);
        call(&mut app, &ctx, "cmdline.key", json!({ "key": "escape" }));
        pump(&mut app, &ctx);
    }
}

#[test]
fn escape_closes_a_dialog_without_cancelling_the_command_behind_it() {
    let ctx = egui::Context::default();
    let mut app = CadApp::new(Session::new(), Services::default());
    pump(&mut app, &ctx);
    call(&mut app, &ctx, "cmdline.input", json!({ "text": "line" }));
    app.ui.dialog = Some("units".into());
    pump(&mut app, &ctx);
    call(&mut app, &ctx, "ui.key", json!({ "key": "Escape" }));
    pump(&mut app, &ctx);
    assert!(app.ui.dialog.is_none(), "Esc left the dialog open");
    let st = call(&mut app, &ctx, "cmdline.state", json!({}));
    assert_eq!(st["running"], "line", "Esc in the dialog also cancelled the command: {st}");
    // With no dialog, Esc cancels the command as usual.
    call(&mut app, &ctx, "ui.key", json!({ "key": "Escape" }));
    pump(&mut app, &ctx);
    assert_eq!(call(&mut app, &ctx, "cmdline.state", json!({}))["running"], Value::Null);
}
