//! The Agent Work palette (Tools ▸ Agent Work, or REVIEW.LOG at the command line): what agents
//! decided on the project (review answers with their citations, deferrals, RFIs drafted, notes),
//! for a person to accept one by one or all at once. Accepting is an undoable edit
//! (`review.approve`); the project's one-page briefing sits on top.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "agentwork";

fn cache_id() -> egui::Id {
    egui::Id::new("agentwork_cache")
}

fn fetch(app: &mut CadApp) -> (Value, Value) {
    let log = app.session.execute("review.log", &json!({"author": "agent", "approved": false, "limit": 50})).unwrap_or(Value::Null);
    let brief = app.session.execute("project.briefing", &json!({})).unwrap_or(Value::Null);
    (log, brief)
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    // The document and its revision key the cache: any edit refreshes it.
    let doc_key = app.session.doc().ok().map_or(0, |d| d as *const _ as usize);
    let rev = app.session.doc().ok().and_then(|d| d.agent.as_ref().map(String::len)).unwrap_or(0);
    let key = (doc_key, rev);
    let (log, brief) = match ctx.data_mut(|d| d.get_temp::<((usize, usize), (Value, Value))>(cache_id())) {
        Some((k, v)) if k == key => v,
        _ => {
            let v = fetch(app);
            ctx.data_mut(|d| d.insert_temp(cache_id(), (key, v.clone())));
            v
        }
    };
    let mut approve: Option<Value> = None;
    egui::Window::new(crate::tl!("Agent Work")).id(egui::Id::new("agentwork_palette")).default_size([460.0, 520.0]).open(open).show(ctx, |ui| {
        if let Some(t) = brief["text"].as_str() {
            ui.label(RichText::new(t).small());
            ui.separator();
        }
        let entries = log["entries"].as_array().cloned().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(format!("{} {}", log["total"].as_u64().unwrap_or(0), crate::tl!("to accept")));
            if !entries.is_empty() && ui.button(crate::tl!("Accept all")).clicked() {
                approve = Some(json!({"all": true}));
            }
        });
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            if entries.is_empty() {
                ui.label(crate::tl!("Nothing waiting: agents' work is all accepted."));
            }
            for e in &entries {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(e["action"].as_str().unwrap_or_default()).strong());
                        ui.label(RichText::new(e["item"].as_str().unwrap_or_default()).monospace());
                        let item = e["item"].as_str().unwrap_or_default();
                        if !item.is_empty() && ui.button(crate::tl!("Accept")).clicked() {
                            approve = Some(json!({"ids": [item]}));
                        }
                    });
                    ui.label(e["summary"].as_str().unwrap_or_default());
                    if let Some(c) = e["citation"].as_str() {
                        ui.label(RichText::new(format!("{} {c}", crate::tl!("Cited:"))).small());
                    }
                });
            }
        });
    });
    if let Some(p) = approve {
        if let Err(e) = app.session.execute("review.approve", &p) {
            app.status = Some((e.to_string(), crate::now_ms()));
        }
        ctx.data_mut(|d| d.remove::<((usize, usize), (Value, Value))>(cache_id()));
    }
}
