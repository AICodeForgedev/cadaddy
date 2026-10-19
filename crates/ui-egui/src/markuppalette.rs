//! The Markups palette (MARKUPS typed alone, Window ▸ Markups): the review markups imported from
//! PDFs (MARKUPIMPORT), each with its comment, author and kind, ticked done or open again, and
//! zoomed to on a click (in its layout).

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;

/// Show markup `m`'s objects: its layout made current, then zoomed to.
fn zoom_to(app: &mut CadApp, m: &Value) {
    let hs: Vec<cadaddy_engine::doc::Handle> =
        m["handles"].as_array().into_iter().flatten().filter_map(|h| h.as_str().and_then(cadaddy_engine::doc::Handle::parse_hex)).collect();
    let Some(first) = hs.first() else { return };
    let space = app.session.doc().ok().and_then(|d| d.space_of(*first));
    if let Some(sp) = space
        && sp != app.session.space()
    {
        let name = match sp {
            cadaddy_engine::doc::Space::Model => "Model".to_string(),
            cadaddy_engine::doc::Space::Paper(n) => n,
        };
        let _ = app.run("layout", json!({ "option": "set", "name": name }));
    }
    let _ = app.run("zoom.object", json!({ "handles": hs.iter().map(|h| h.hex()).collect::<Vec<_>>() }));
    app.canvas.list = None;
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let list: Vec<Value> = app.session.execute("markups", &json!({})).ok().and_then(|v| v["markups"].as_array().cloned()).unwrap_or_default();
    let mut toggle: Option<(String, bool)> = None;
    let mut zoom: Option<Value> = None;
    let mut import = false;
    egui::Window::new(crate::tl!("Markups")).id(egui::Id::new("Markups")).open(open).resizable(true).default_size([360.0, 400.0]).show(ctx, |ui| {
        let done = list.iter().filter(|m| m["status"] == "done").count();
        ui.label(RichText::new(crate::tf!("{done} of {total} done", done = done, total = list.len())).small().color(t.text_dim));
        egui::ScrollArea::vertical().id_salt("markup-list").max_height(300.0).show(ui, |ui| {
            if list.is_empty() {
                ui.label(RichText::new(crate::tl!("No markups. Import them from a reviewed PDF.")).color(t.text_dim));
            }
            for m in &list {
                let id = m["id"].as_str().unwrap_or_default().to_string();
                ui.horizontal(|ui| {
                    let mut is_done = m["status"] == "done";
                    if ui.checkbox(&mut is_done, "").on_hover_text(crate::tl!("Done")).changed() {
                        toggle = Some((id.clone(), is_done));
                    }
                    let text = m["text"].as_str().filter(|s| !s.is_empty()).unwrap_or_else(|| m["kind"].as_str().unwrap_or_default());
                    let label = format!("{id}  {text}");
                    let rich = if is_done { RichText::new(label).strikethrough().color(t.text_dim) } else { RichText::new(label) };
                    if ui.selectable_label(false, rich).on_hover_text(m["author"].as_str().unwrap_or_default()).clicked() {
                        zoom = Some(m.clone());
                    }
                });
            }
        });
        ui.separator();
        import = ui.button(crate::tl!("Import Markups...")).clicked();
    });
    if let Some((id, done)) = toggle {
        let _ = app.run("markupstatus", json!({ "id": id, "status": if done { "done" } else { "open" } }));
    }
    if let Some(m) = zoom {
        zoom_to(app, &m);
    }
    if import {
        let _ = crate::menus::run_ui_command(app, "markupimport", &Value::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_opens_and_lists() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.cmdline("markups");
        assert_eq!(app.ui.dialog.as_deref(), Some("markups"));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert!(app.run("markups", json!({})).unwrap()["markups"].as_array().unwrap().is_empty());
    }
}
