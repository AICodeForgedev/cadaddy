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

fn fetch(s: &mut cadaddy_engine::Session) -> (Value, Value) {
    let log = s.execute("review.log", &json!({"author": "agent", "approved": false, "limit": 50})).unwrap_or(Value::Null);
    let brief = s.execute("project.briefing", &json!({})).unwrap_or(Value::Null);
    (log, brief)
}

/// The document and its revision key the cache: any edit refreshes it.
fn key(app: &CadApp) -> (usize, usize) {
    let doc_key = app.session.doc().ok().map_or(0, |d| d as *const _ as usize);
    let rev = app.session.doc().ok().and_then(|d| d.agent.as_ref().map(String::len)).unwrap_or(0);
    (doc_key, rev)
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    // The briefing reads the model against the whole set: worked out on a worker thread, the
    // window open meanwhile (Q6585).
    let fetched = crate::asked::worked_out(app, ctx, cache_id(), "project.briefing", key, fetch);
    let mut approve: Option<Value> = None;
    egui::Window::new(crate::tl!("Agent Work")).id(egui::Id::new("agentwork_palette")).default_size([460.0, 520.0]).open(open).show(ctx, |ui| {
        let Some(fetched) = &fetched else {
            crate::asked::working(ui);
            return;
        };
        let (log, brief) = &**fetched;
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
    // Accepting is an edit: the drawing the list keys on changes, so it is worked out again.
    if let Some(p) = approve
        && let Err(e) = app.session.execute("review.approve", &p)
    {
        app.status = Some((e.to_string(), crate::now_ms()));
    }
}
