//! The Model Tree palette (Window ▸ Model Tree): a parametric part's parameters (name, expression,
//! value; edit an expression and the part rebuilds) and its feature tree (sketches, pads, pockets,
//! revolves, fillets, chamfers) with each feature's state. Suppress, rename, move up or down,
//! delete, rebuild. Everything goes through the engine's `part.*` commands.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::icons::{self, Icon};

pub const DIALOG: &str = "parttree";

/// What the palette keeps between frames.
#[derive(Clone, Default)]
struct State {
    part: String,
    /// The tree, and the drawing revision it was read at.
    tree: Option<(u64, String, Value)>,
    /// A parameter or feature being edited: its key and the text so far.
    editing: Option<(String, String)>,
    new_name: String,
    new_expr: String,
    message: String,
}

/// What a click asks for, run after the window is drawn.
struct Act(&'static str, Value);

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let id = egui::Id::new("part-tree-state");
    let mut st: State = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let parts: Vec<String> = app
        .session
        .execute("part.list", &json!({}))
        .ok()
        .and_then(|v| v["parts"].as_array().map(|a| a.iter().filter_map(|p| p["name"].as_str().map(str::to_string)).collect()))
        .unwrap_or_default();
    if !parts.contains(&st.part) {
        st.part = parts.last().cloned().unwrap_or_default();
    }
    let revision = app.session.state().map(|s| s.revision).unwrap_or(0);
    if st.tree.as_ref().is_none_or(|(r, p, _)| *r != revision || *p != st.part) {
        let tree =
            if st.part.is_empty() { Value::Null } else { app.session.execute("part.tree", &json!({ "part": st.part })).unwrap_or(Value::Null) };
        st.tree = Some((revision, st.part.clone(), tree));
    }
    let tree = st.tree.as_ref().map(|(_, _, v)| v.clone()).unwrap_or(Value::Null);
    let part = st.part.clone();
    let mut acts: Vec<Act> = Vec::new();
    egui::Window::new(crate::tl!("Model Tree"))
        .id(egui::Id::new("Model Tree"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([380.0, 520.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if parts.len() > 1 {
                    egui::ComboBox::from_id_salt("part-tree-part").selected_text(&st.part).show_ui(ui, |ui| {
                        for p in &parts {
                            ui.selectable_value(&mut st.part, p.clone(), p);
                        }
                    });
                } else if let Some(p) = parts.first() {
                    ui.strong(p);
                }
                if ui.button(crate::tl!("New Part")).clicked() {
                    acts.push(Act("part.new", json!({})));
                }
                if !part.is_empty() && ui.button(crate::tl!("Rebuild Part")).clicked() {
                    acts.push(Act("part.rebuild", json!({ "part": part })));
                }
            });
            if part.is_empty() {
                ui.label(
                    RichText::new(crate::tl!("No part yet: New Part starts one; sketches and features come from the part commands."))
                        .color(t.text_dim),
                );
                return;
            }
            if let Some(v) = tree["volume"].as_f64() {
                ui.label(RichText::new(crate::tf!("Volume {v}", v = format!("{v:.4}"))).color(t.text_dim));
            }
            ui.separator();
            ui.strong(crate::tl!("Parameters"));
            egui::Grid::new("part-tree-params").striped(true).num_columns(3).show(ui, |ui| {
                for p in tree["parameters"].as_array().into_iter().flatten() {
                    let name = p["name"].as_str().unwrap_or_default();
                    let expr = p["expr"].as_str().unwrap_or_default();
                    ui.label(name);
                    let key = format!("param:{name}");
                    let mut text = match &st.editing {
                        Some((k, v)) if *k == key => v.clone(),
                        _ => expr.to_string(),
                    };
                    let r = ui.add_sized([150.0, 20.0], egui::TextEdit::singleline(&mut text));
                    if r.changed() {
                        st.editing = Some((key.clone(), text.clone()));
                    }
                    if r.lost_focus() && st.editing.as_ref().is_some_and(|(k, _)| *k == key) {
                        if text.trim() != expr {
                            acts.push(Act("part.parameter", json!({ "part": part, "name": name, "expr": text.trim() })));
                        }
                        st.editing = None;
                    }
                    ui.label(RichText::new(p["value"].as_f64().map(|v| format!("= {v}")).unwrap_or_else(|| "?".into())).color(t.text_dim));
                    ui.end_row();
                }
                ui.add_sized([70.0, 20.0], egui::TextEdit::singleline(&mut st.new_name).hint_text(crate::tl!("name")));
                ui.add_sized([150.0, 20.0], egui::TextEdit::singleline(&mut st.new_expr).hint_text(crate::tl!("expression")));
                if ui.button(crate::tl!("Add")).clicked() && !st.new_name.trim().is_empty() {
                    acts.push(Act("part.parameter", json!({ "part": part, "name": st.new_name.trim(), "expr": st.new_expr.trim() })));
                    st.new_name.clear();
                    st.new_expr.clear();
                }
                ui.end_row();
            });
            ui.separator();
            ui.strong(crate::tl!("Features"));
            let features = tree["features"].as_array().cloned().unwrap_or_default();
            let n = features.len();
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                for (i, f) in features.iter().enumerate() {
                    let fid = f["id"].clone();
                    let name = f["name"].as_str().unwrap_or_default();
                    let state = f["state"].as_str().unwrap_or_default();
                    ui.horizontal(|ui| {
                        // The state: a filled dot (built), a ring (suppressed), a warning-coloured dot
                        // (failed).
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                        match state {
                            "ok" => ui.painter().circle_filled(rect.center(), 4.0, t.accent),
                            "suppressed" => ui.painter().circle_stroke(rect.center(), 3.5, egui::Stroke::new(1.2, t.text_dim)),
                            _ => ui.painter().circle_filled(rect.center(), 4.0, t.warn),
                        };
                        let mut on = state != "suppressed";
                        if ui.checkbox(&mut on, "").on_hover_text(crate::tl!("Suppress")).changed() {
                            acts.push(Act("part.edit", json!({ "part": part, "feature": fid, "suppressed": !on })));
                        }
                        let key = format!("feature:{fid}");
                        match &mut st.editing {
                            Some((k, text)) if *k == key => {
                                let r = ui.add(egui::TextEdit::singleline(text).desired_width(120.0));
                                if r.lost_focus() {
                                    if !text.trim().is_empty() && text.trim() != name {
                                        acts.push(Act("part.edit", json!({ "part": part, "feature": fid, "name": text.trim() })));
                                    }
                                    st.editing = None;
                                } else {
                                    r.request_focus();
                                }
                            }
                            _ => {
                                if ui.selectable_label(false, name).on_hover_text(crate::tl!("Double-click to rename")).double_clicked() {
                                    st.editing = Some((key, name.to_string()));
                                }
                            }
                        }
                        ui.label(RichText::new(f["type"].as_str().unwrap_or_default()).color(t.text_dim));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icons::button(ui, Icon::Erase, 18.0, crate::tl!("Delete"), false).clicked() {
                                acts.push(Act("part.delete", json!({ "part": part, "feature": fid })));
                            }
                            if ui
                                .add_enabled_ui(i + 1 < n, |ui| icons::button(ui, Icon::ChevronDown, 18.0, crate::tl!("Move Down"), false))
                                .inner
                                .clicked()
                            {
                                acts.push(Act("part.reorder", json!({ "part": part, "feature": fid, "to": i + 1 })));
                            }
                            if ui.add_enabled_ui(i > 0, |ui| icons::button(ui, Icon::ChevronUp, 18.0, crate::tl!("Move Up"), false)).inner.clicked() {
                                acts.push(Act("part.reorder", json!({ "part": part, "feature": fid, "to": i - 1 })));
                            }
                        });
                    });
                    if let Some(e) = f["error"].as_str() {
                        ui.label(RichText::new(e).color(t.warn).small());
                    }
                }
            });
            if !st.message.is_empty() {
                ui.separator();
                ui.label(RichText::new(&st.message).color(t.warn));
            }
        });
    for Act(cmd, p) in acts {
        match app.run(cmd, p) {
            Ok(v) => {
                st.message.clear();
                if cmd == "part.new"
                    && let Some(n) = v["part"].as_str()
                {
                    st.part = n.to_string();
                }
            }
            Err(e) => st.message = e,
        }
        st.tree = None;
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
}
