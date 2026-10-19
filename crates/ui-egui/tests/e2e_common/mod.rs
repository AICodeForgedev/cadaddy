//! The end-to-end harness: a whole CadApp driven as an agent or user would, through the control
//! channel (`control::handle`: `cmdline.input`, `engine.execute`, `ui.menu.invoke`, `ui.click`,
//! `ui.key`…), with real frames run between calls, and the visible result checked: the controls on
//! screen (`ui.elements`), the interface state (`ui.inspect`), the drawing (`drawing.inspect`) or a
//! render (`ui.render`, decoded and its pixels counted).

#![allow(dead_code)]

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_ui_egui::control::{ControlRequest, Outcome, handle};
use cadaddy_ui_egui::{CadApp, Services};
use serde_json::{Value, json};

pub const W: f32 = 1280.0;
pub const H: f32 = 800.0;

pub struct E2e {
    pub app: CadApp,
    pub ctx: egui::Context,
    /// Synthetic input for the next frame (touch, pointer and keys).
    pub events: Vec<egui::Event>,
}

/// The real file formats, as the app installs them (apps/cadaddy `install_io`).
pub fn io_hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
    });
    cadaddy_engine::cmd::pointcloud::set_reader(cadaddy_io::pointcloud_io::read);
    cadaddy_engine::cmd::markup::set_reader(cadaddy_io::pdf_markups::read);
    cadaddy_engine::cmd::file::set_pdf_merge(|parts| cadaddy_io::pdf_merge::merge(parts).map_err(|e| e.to_string()));
    cadaddy_engine::cmd::file::set_write_as(|d, name, o| {
        cadaddy_io::write_with(d, name, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
    cadaddy_engine::cmd::file::set_entity_codec(cadaddy_engine::cmd::file::EntityCodec {
        groups: cadaddy_io::entity_groups,
        entity: cadaddy_io::entity_from_groups,
        records: cadaddy_io::entity_records,
        attrib: cadaddy_io::attrib_from_groups,
    });
}

/// A fresh folder for one test's files.
pub fn folder(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-e2e-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

impl E2e {
    pub fn new() -> E2e {
        io_hooks();
        let mut e = E2e { app: CadApp::new(Session::new(), Services::default()), ctx: egui::Context::default(), events: Vec::new() };
        e.pump();
        e
    }

    /// One frame of the whole interface, with any queued synthetic input.
    pub fn frame(&mut self) {
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W, H))),
            events: std::mem::take(&mut self.events),
            ..Default::default()
        };
        self.app.raw_input_hook(&mut raw);
        cadaddy_ui_egui::cmdline::capture_tab(&self.ctx, &mut raw);
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
        out.textures_delta.clear();
    }

    /// A few frames, so layouts settle (windows size themselves, palettes open).
    pub fn pump(&mut self) {
        for _ in 0..4 {
            self.frame();
        }
    }

    /// A control-channel call; the reply's `result`, or the error as `Err`. Frames run after it,
    /// as in the app.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let (req, _reply) = ControlRequest::new(method, params);
        let v = match handle(&mut self.app, &self.ctx, &req) {
            Outcome::Done(v) => v,
            Outcome::Screenshot { .. } => json!({ "ok": true, "result": null }),
        };
        self.pump();
        if v["ok"] == true { Ok(v["result"].clone()) } else { Err(v["error"].as_str().map_or_else(|| v.to_string(), str::to_string)) }
    }

    /// `call` that must succeed.
    pub fn ok(&mut self, method: &str, params: Value) -> Value {
        self.call(method, params.clone()).unwrap_or_else(|e| panic!("{method} {params}: {e}"))
    }

    /// Run a command by id with JSON parameters, as an agent does (`engine.execute`).
    pub fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({ "command": command, "params": params }))
    }

    /// Type a line at the command line (Enter at the end), as a user does.
    pub fn typed(&mut self, text: &str) -> Value {
        self.ok("cmdline.input", json!({ "text": text }))
    }

    /// The menu item that runs `command` (`ui.menu.invoke`), as a click on it does.
    pub fn menu(&mut self, command: &str) -> Value {
        self.ok("ui.menu.invoke", json!({ "command": command }))
    }

    /// The controls on screen now (dialogs and menus included).
    pub fn elements(&mut self) -> Vec<Value> {
        let v = self.ok("ui.elements", json!({}));
        v.as_array().cloned().or_else(|| v["elements"].as_array().cloned()).unwrap_or_default()
    }

    /// Whether a control labelled `label` (exactly) is on screen.
    pub fn shows(&mut self, label: &str) -> bool {
        self.elements().iter().any(|e| e["label"].as_str() == Some(label))
    }

    /// Click the control labelled `label` (its centre, on screen); false when none is shown.
    pub fn click(&mut self, label: &str) -> bool {
        let Some(e) = self.elements().into_iter().find(|e| e["label"].as_str() == Some(label)) else { return false };
        let r: Vec<f64> = e["rect"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
        let [x, y, w, h] = r.as_slice() else { return false };
        self.ok("ui.click", json!({ "x": x + w / 2.0, "y": y + h / 2.0, "space": "screen" }));
        true
    }

    /// The open dialog's id (`ui.inspect`'s `ui.dialog`).
    pub fn dialog(&mut self) -> Option<String> {
        self.ok("ui.inspect", json!({}))["ui"]["dialog"].as_str().map(str::to_string)
    }

    /// The drawing's summary (`drawing.inspect`).
    pub fn drawing(&mut self) -> Value {
        self.ok("drawing.inspect", json!({ "entities": false }))
    }

    pub fn entity_count(&mut self) -> u64 {
        self.drawing()["entityCount"].as_u64().unwrap_or(0)
    }

    /// The drawing rendered (`ui.render`), decoded.
    pub fn render(&mut self, width: u32, height: u32) -> image::RgbaImage {
        let r = self.ok("ui.render", json!({ "width": width, "height": height }));
        let b64 = r["pngBase64"].as_str().unwrap_or_default();
        let png = cadaddy_engine::cmd::file::base64_decode(b64).expect("ui.render's PNG");
        image::load_from_memory(&png).expect("a PNG").to_rgba8()
    }

    /// Pixels of a render that differ from its background (its most common colour).
    pub fn inked(&mut self, width: u32, height: u32) -> usize {
        inked(&self.render(width, height))
    }

    /// The last command-line lines.
    pub fn history(&mut self) -> Vec<String> {
        self.app.session.log.iter().rev().take(40).rev().cloned().collect()
    }
}

/// Pixels that differ from the image's most common colour.
pub fn inked(img: &image::RgbaImage) -> usize {
    let mut counts = std::collections::HashMap::new();
    for p in img.pixels() {
        *counts.entry(p.0).or_insert(0usize) += 1;
    }
    let bg = counts.iter().max_by_key(|(_, n)| **n).map(|(c, _)| *c).unwrap_or([0, 0, 0, 0]);
    img.pixels().filter(|p| p.0 != bg).count()
}
