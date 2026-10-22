//! `?field`: the field viewer (Q312) on its own — the building model on a tablet or phone, with no
//! drawing and no engine loaded. A package comes from Open, a dropped file or `?field=URL`; it and
//! the notes made are written to the browser's storage as soon as they change, so a reload with
//! no connection comes back as it was. Send Notes downloads the notes file for `fieldview.sync`.

use cadaddy_ui_egui::fieldview::{FieldViewer, STORAGE_KEY};
use wasm_bindgen::JsCast as _;

struct FieldShell(FieldViewer, crate::upload::Inbox);

impl eframe::App for FieldShell {
    fn logic(&mut self, _ctx: &egui::Context, frame: &mut eframe::Frame) {
        let viewer = &mut self.0;
        self.1.drain_with(|upload| match upload {
            Ok((name, bytes)) => {
                if let Err(e) = viewer.open(&bytes) {
                    viewer.status = format!("{name}: {e}");
                }
            }
            Err(e) => viewer.status = e,
        });
        if std::mem::take(&mut self.0.open_requested) {
            self.1.pick();
        }
        for (name, bytes) in std::mem::take(&mut self.0.outbox) {
            if let Err(e) = crate::download::download(&name, "application/json", &bytes) {
                self.0.status = e;
            }
        }
        // Kept at once: a tablet's battery may die before the next autosave.
        if self.0.dirty
            && let Some(storage) = frame.storage_mut()
            && let Some(text) = self.0.to_storage()
        {
            storage.set_string(STORAGE_KEY, text);
            storage.flush();
            self.0.dirty = false;
        }
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some(text) = self.0.to_storage() {
            storage.set_string(STORAGE_KEY, text);
        }
    }
    fn persist_egui_memory(&self) -> bool {
        false
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }
}

/// A tag for this device in note ids, kept with the package's storage.
fn device_tag() -> String {
    let ms = web_sys::js_sys::Date::now();
    let n = if ms.is_finite() && ms > 0.0 { (ms as u64) % 0xff_ffff } else { 0 };
    format!("d{n:x}")
}

pub fn start(canvas_id: &'static str, loading_id: &'static str, url: Option<String>) {
    wasm_bindgen_futures::spawn_local(async move {
        let Some(document) = web_sys::window().and_then(|w| w.document()) else { return };
        let Some(canvas) = document.get_element_by_id(canvas_id).and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok()) else {
            log::error!("missing <canvas id=\"{canvas_id}\">");
            return;
        };
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |cc| {
                    cadaddy_ui_egui::theme::apply(&cc.egui_ctx, egui::Theme::Dark, false);
                    let device = cc.storage.and_then(|s| s.get_string("cadaddy.fieldview.device")).unwrap_or_else(device_tag);
                    let mut viewer = FieldViewer::new(&device);
                    if let Some(text) = cc.storage.and_then(|s| s.get_string(STORAGE_KEY))
                        && let Err(e) = viewer.from_storage(&text)
                    {
                        viewer.status = e;
                    }
                    // The device tag is written with the first package.
                    viewer.dirty = viewer.pkg.is_none();
                    let inbox = crate::upload::Inbox::new(&cc.egui_ctx);
                    if let Some(url) = url {
                        inbox.fetch(url);
                    }
                    let shell = FieldShell(viewer, inbox);
                    Ok(Box::new(DeviceSaved(shell, device)))
                }),
            )
            .await;
        if let Some(el) = document.get_element_by_id(loading_id) {
            match result {
                Ok(()) => el.remove(),
                Err(e) => {
                    el.set_inner_html(&format!("<p>The field viewer failed to start: {e:?}</p><p>A browser with WebGPU or WebGL2 is required.</p>"))
                }
            }
        }
    });
}

/// The shell, writing this device's tag to storage once.
struct DeviceSaved(FieldShell, String);

impl eframe::App for DeviceSaved {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !self.1.is_empty()
            && let Some(storage) = frame.storage_mut()
        {
            storage.set_string("cadaddy.fieldview.device", std::mem::take(&mut self.1));
        }
        self.0.logic(ctx, frame);
    }
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.0.save(storage);
    }
    fn persist_egui_memory(&self) -> bool {
        false
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.0.ui(ui, frame);
    }
}
