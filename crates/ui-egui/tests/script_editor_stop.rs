//! The Script Editor's Stop: a script that never ends runs on its worker thread while the window
//! keeps drawing, and Stop ends it with a "stopped" line in the output.

use std::time::{Duration, Instant};

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

/// Click the Script Editor's button `label`.
fn click(app: &mut CadApp, ctx: &egui::Context, label: &str) {
    let found = call(app, ctx, "ui.elements", json!({ "label": label, "role": "button" }));
    let w = found.as_array().and_then(|a| a.last()).unwrap_or_else(|| panic!("no {label} button: {found}")).clone();
    let r: Vec<f64> = w["rect"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    call(app, ctx, "ui.click", json!({ "x": r[0] + r[2] / 2.0, "y": r[1] + r[3] / 2.0 }));
    pump(app, ctx);
}

fn output(ctx: &egui::Context) -> Vec<String> {
    ctx.data(|d| d.get_temp::<Vec<String>>(egui::Id::new("rhai_editor_output"))).unwrap_or_default()
}

#[test]
fn stop_ends_a_runaway_script_and_the_window_keeps_drawing() {
    let ctx = egui::Context::default();
    let mut app = CadApp::new(Session::new(), Services::default());
    call(&mut app, &ctx, "cmdline.state", json!({}));
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("rhai_editor_code"), r#"print("started"); loop { getvar("CLAYER"); }"#.to_string()));
    app.ui.dialog = Some("scripteditor".into());
    pump(&mut app, &ctx);
    click(&mut app, &ctx, "Run");
    assert!(app.session.rhai_running(), "the script runs on after Run returned");
    // Frames go on while it runs, each quickly.
    let t = Instant::now();
    let mut frames = 0;
    while t.elapsed() < Duration::from_millis(300) {
        let f = Instant::now();
        frame(&mut app, &ctx);
        assert!(f.elapsed() < Duration::from_millis(250), "a frame took {:?}", f.elapsed());
        frames += 1;
    }
    assert!(frames >= 5, "{frames} frames in 300 ms");
    assert!(app.session.rhai_running());
    click(&mut app, &ctx, "Stop");
    let t = Instant::now();
    while app.session.rhai_running() {
        assert!(t.elapsed() < Duration::from_secs(2), "Stop didn't end the script");
        frame(&mut app, &ctx);
    }
    pump(&mut app, &ctx);
    assert_eq!(output(&ctx), ["started", "Error: script stopped by the user"]);
    // Run is back: a second script runs to its value.
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("rhai_editor_code"), "6 * 7".to_string()));
    click(&mut app, &ctx, "Run");
    let t = Instant::now();
    while app.session.rhai_running() {
        assert!(t.elapsed() < Duration::from_secs(2));
        frame(&mut app, &ctx);
    }
    pump(&mut app, &ctx);
    assert_eq!(output(&ctx).last().map(String::as_str), Some("=> 42"));
}
