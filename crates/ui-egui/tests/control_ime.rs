//! `ui.ime` drives input method composition through the control channel, as a Chinese,
//! Japanese or Korean IME would: the pre-edit shows at the command line, the commit is typed.

use cadaddy_engine::Session;
use cadaddy_ui_egui::control::{ControlRequest, Outcome, handle};
use cadaddy_ui_egui::{CadApp, Services};
use serde_json::{Value, json};

fn call(app: &mut CadApp, ctx: &egui::Context, method: &str, params: Value) -> Value {
    let (req, _reply) = ControlRequest::new(method, params);
    match handle(app, ctx, &req) {
        Outcome::Done(v) => v,
        _ => panic!("unexpected screenshot"),
    }
}

fn pump(app: &mut CadApp, ctx: &egui::Context) {
    for _ in 0..3 {
        let mut raw =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))), ..Default::default() };
        app.raw_input_hook(&mut raw);
        let mut out = ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }
}

#[test]
fn ui_ime_composes_and_commits_at_the_command_line() {
    let ctx = egui::Context::default();
    let mut app = CadApp::new(Session::new(), Services::default());
    pump(&mut app, &ctx);
    assert_eq!(call(&mut app, &ctx, "ui.ime", json!({"preedit": "えん"}))["ok"], true);
    pump(&mut app, &ctx);
    assert_eq!((app.cmd.preedit.as_str(), app.cmd.buffer.as_str()), ("えん", ""));
    call(&mut app, &ctx, "ui.ime", json!({"preedit": ""}));
    call(&mut app, &ctx, "ui.ime", json!({"commit": "円"}));
    pump(&mut app, &ctx);
    assert_eq!(call(&mut app, &ctx, "cmdline.state", json!({}))["result"]["buffer"], "円");
    // Neither or both: an error, nothing sent.
    assert_eq!(call(&mut app, &ctx, "ui.ime", json!({}))["ok"], false);
    assert_eq!(call(&mut app, &ctx, "ui.ime", json!({"preedit": "a", "commit": "b"}))["ok"], false);
}
