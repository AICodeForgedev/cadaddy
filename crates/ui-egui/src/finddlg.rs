//! The Find and Replace dialog (FIND, Edit ▸ Find...): what to find and what with, where (the
//! whole drawing, this space or the selection), what to look in, match case, whole words,
//! wildcards or regular expressions; a list of the hits with their space and text, each one
//! zoomed to (switching layout as needed) or replaced on its own; Replace All; and the hits made
//! the selection. Everything runs the FIND command.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;

/// What FIND can look in: (FIND's name, label).
const LOOK_IN: [(&str, &str); 7] = [
    ("text", "Text and multiline text"),
    ("attributes", "Block attribute values"),
    ("dimensions", "Dimension text"),
    ("tables", "Tables"),
    ("mleaders", "Multileaders"),
    ("hyperlinks", "Hyperlinks"),
    ("names", "Layer and block names"),
];

#[derive(Clone)]
struct State {
    what: String,
    with: String,
    case: bool,
    whole: bool,
    /// 0 plain, 1 wildcards, 2 regular expressions.
    mode: usize,
    /// 0 the whole drawing, 1 this space, 2 the selection.
    scope: usize,
    look_in: [bool; 7],
    results: Vec<Value>,
    status: String,
}

impl Default for State {
    fn default() -> Self {
        State {
            what: String::new(),
            with: String::new(),
            case: false,
            whole: false,
            mode: 0,
            scope: 0,
            look_in: [true, true, true, true, true, true, false],
            results: Vec::new(),
            status: String::new(),
        }
    }
}

fn id() -> egui::Id {
    egui::Id::new("find_dialog_state")
}

/// FIND's parameters for the dialog's choices.
fn params(st: &State) -> Value {
    let include: Vec<&str> = LOOK_IN.iter().zip(st.look_in).filter(|(_, on)| *on).map(|((k, _), _)| *k).collect();
    let mode = ["plain", "wildcard", "regex"].get(st.mode).copied().unwrap_or("plain");
    let scope = ["drawing", "space", "selection"].get(st.scope).copied().unwrap_or("drawing");
    json!({
        "find": st.what,
        "matchCase": st.case,
        "wholeWord": st.whole,
        "mode": mode,
        "in": scope,
        "include": include,
    })
}

/// Show hit `m`: its layout made current, then zoomed to.
pub fn zoom_to(app: &mut CadApp, m: &Value) {
    let Some(h) = m["handle"].as_str() else { return };
    let here = match app.session.space() {
        cadaddy_engine::doc::Space::Model => "Model".to_string(),
        cadaddy_engine::doc::Space::Paper(n) => n,
    };
    if let Some(sp) = m["space"].as_str().filter(|sp| *sp != here) {
        let _ = app.run("layout", json!({ "option": "set", "name": sp }));
    }
    let _ = app.run("zoom.object", json!({ "handles": [h] }));
    app.canvas.list = None;
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
    let mut action: Option<Value> = None;
    let mut zoom: Option<Value> = None;
    let mut close = false;
    egui::Window::new(crate::tl!("Find and Replace"))
        .id(egui::Id::new("find_dialog"))
        .open(open)
        .resizable(true)
        .collapsible(false)
        .default_width(460.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            // Right to left in Arabic (rtl.rs).
            let mut body = crate::rtl::Body::new(ui);
            let ui = &mut *body;
            crate::rtl::show_grid(egui::Grid::new("find_grid").num_columns(2).spacing(egui::vec2(10.0, 6.0)), ui, |ui| {
                crate::rtl::lead(ui, crate::tl!("Find what:"));
                // Named by its label (in either reading order), so screen readers and agents find it.
                let name = crate::tl!("Find what:");
                ui.add(egui::TextEdit::singleline(&mut st.what).desired_width(280.0))
                    .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, name.trim_end_matches(':')));
                crate::rtl::trail(ui, crate::tl!("Find what:"));
                ui.end_row();
                crate::rtl::lead(ui, crate::tl!("Replace with:"));
                // Named by its label (in either reading order), so screen readers and agents find it.
                let name = crate::tl!("Replace with:");
                ui.add(egui::TextEdit::singleline(&mut st.with).desired_width(280.0))
                    .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, name.trim_end_matches(':')));
                crate::rtl::trail(ui, crate::tl!("Replace with:"));
                ui.end_row();
                crate::rtl::lead(ui, crate::tl!("Find where:"));
                egui::ComboBox::from_id_salt("find_scope")
                    .selected_text(crate::i18n::t(["Entire drawing", "Current space", "Selected objects"][st.scope.min(2)]))
                    .show_ui(ui, |ui| {
                        for (i, l) in ["Entire drawing", "Current space", "Selected objects"].iter().enumerate() {
                            ui.selectable_value(&mut st.scope, i, crate::i18n::t(l));
                        }
                    });
                crate::rtl::trail(ui, crate::tl!("Find where:"));
                ui.end_row();
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut st.case, crate::tl!("Match case"));
                ui.checkbox(&mut st.whole, crate::tl!("Find whole words only"));
            });
            ui.horizontal(|ui| {
                for (i, l) in ["Plain text", "Use wildcards", "Regular expressions"].iter().enumerate() {
                    ui.radio_value(&mut st.mode, i, crate::i18n::t(l));
                }
            });
            egui::CollapsingHeader::new(crate::tl!("Look in")).id_salt("find_look_in").show(ui, |ui| {
                for (i, (_, l)) in LOOK_IN.iter().enumerate() {
                    if let Some(on) = st.look_in.get_mut(i) {
                        ui.checkbox(on, crate::i18n::t(l));
                    }
                }
            });
            if !st.status.is_empty() {
                ui.label(RichText::new(&st.status).small());
            }
            if !st.results.is_empty() {
                ui.separator();
                egui::ScrollArea::vertical().id_salt("find_results").max_height(200.0).show(ui, |ui| {
                    crate::rtl::show_grid(egui::Grid::new("find_results_grid").striped(true).num_columns(4), ui, |ui| {
                        for m in st.results.clone() {
                            ui.label(RichText::new(m["space"].as_str().unwrap_or("—")).small().color(t.text_dim));
                            ui.label(crate::i18n::t(m["type"].as_str().unwrap_or_default()));
                            ui.label(m["text"].as_str().unwrap_or_default());
                            ui.horizontal(|ui| {
                                if m["handle"].is_string() && ui.small_button(crate::tl!("Zoom To")).clicked() {
                                    zoom = Some(m.clone());
                                }
                                if ui.small_button(crate::tl!("Replace")).clicked() {
                                    let mut p = params(&st);
                                    p["replace"] = json!(st.with);
                                    match (m["handle"].as_str(), m["name"].as_str()) {
                                        (Some(h), _) => p["handles"] = json!([h]),
                                        (_, Some(n)) => {
                                            p["handles"] = json!([]);
                                            p["names"] = json!([n]);
                                        }
                                        _ => {}
                                    }
                                    action = Some(p);
                                }
                            });
                            ui.end_row();
                        }
                    });
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                let some = !st.what.is_empty();
                if ui.add_enabled(some, egui::Button::new(crate::tl!("Find"))).clicked() {
                    let mut p = params(&st);
                    p["select"] = json!(false);
                    action = Some(p);
                }
                if ui.add_enabled(some, egui::Button::new(crate::tl!("Replace All"))).clicked() {
                    let mut p = params(&st);
                    p["replace"] = json!(st.with);
                    action = Some(p);
                }
                if ui.add_enabled(some, egui::Button::new(crate::tl!("Create Selection Set"))).clicked() {
                    let mut p = params(&st);
                    p["select"] = json!(true);
                    action = Some(p);
                }
                if ui.button(crate::tl!("Close")).clicked() {
                    close = true;
                }
            });
        });
    if let Some(p) = action {
        let replacing = p.get("replace").is_some();
        let selecting = p["select"] == true;
        match app.run("find", p) {
            Ok(r) => {
                let n = r["total"].as_u64().unwrap_or(0);
                let objects = r["matches"].as_array().map_or(0, Vec::len);
                st.status = if replacing {
                    crate::tf!("Matches replaced: {count} (objects: {objects})", count = n, objects = objects)
                } else if selecting {
                    crate::tf!("Matches found and selected: {count} (objects: {objects})", count = n, objects = objects)
                } else {
                    crate::tf!("Matches found: {count} (objects: {objects})", count = n, objects = objects)
                };
                if replacing {
                    // What is left to find.
                    let mut again = params(&st);
                    again["select"] = json!(false);
                    st.results = app.session.execute("find", &again).ok().and_then(|v| v["matches"].as_array().cloned()).unwrap_or_default();
                } else {
                    st.results = r["matches"].as_array().cloned().unwrap_or_default();
                }
            }
            Err(e) => st.status = e,
        }
    }
    if let Some(m) = zoom {
        zoom_to(app, &m);
    }
    ctx.data_mut(|d| d.insert_temp(id(), st));
    if close {
        *open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_are_listed_and_zoomed_to_across_layouts() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.run("text", json!({ "at": [0, 0], "height": 1, "text": "door" })).unwrap();
        app.run("layout", json!({ "option": "new", "name": "Sheet" })).unwrap();
        app.run("layout", json!({ "option": "set", "name": "Sheet" })).unwrap();
        app.run("text", json!({ "at": [1, 1], "height": 1, "text": "door 2" })).unwrap();
        app.run("layout", json!({ "option": "set", "name": "Model" })).unwrap();
        let st = State { what: "door".into(), ..State::default() };
        let r = app.run("find", params(&st)).unwrap();
        let hits = r["matches"].as_array().unwrap().clone();
        assert_eq!(hits.len(), 2);
        let sheet = hits.iter().find(|m| m["space"] == "Sheet").unwrap();
        zoom_to(&mut app, sheet);
        assert!(matches!(app.session.space(), cadaddy_engine::doc::Space::Paper(n) if n == "Sheet"), "switched to the hit's layout");
        // The dialog renders.
        app.ui.dialog = Some("find".into());
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
    }
}
