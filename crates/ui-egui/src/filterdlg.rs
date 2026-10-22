//! Object Selection Filters (FILTER typed or picked): the filter as one line ("type = Line and
//! (layer = Walls or color = red)"), a row that adds a condition to it, the saved filters (pick
//! one to edit or apply it, Save As, Delete) and Apply, which selects the objects that pass. All
//! through the FILTER command: saved filters are the user's, shared by every drawing.

use egui::vec2;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "filter";

/// The properties offered in the condition row (any property QSELECT knows can be typed).
const PROPERTIES: &[&str] = &["type", "layer", "color", "linetype", "lineweight", "radius", "length", "area", "contents", "name"];
const OPERATORS: &[&str] = &["=", "!=", "<", ">", "<=", ">=", "*"];

/// What the dialog holds between frames.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct State {
    filter: String,
    property: String,
    op: String,
    value: String,
    /// The saved filter picked, and the name typed for Save As.
    saved: String,
    save_as: String,
    selection_only: bool,
    /// The saved filters as the command lists them (name, filter as text), read when the
    /// dialog opens and after a save or delete.
    list: Option<Vec<(String, String)>>,
    message: String,
}

impl Default for State {
    fn default() -> Self {
        State {
            filter: String::new(),
            property: "type".into(),
            op: "=".into(),
            value: String::new(),
            saved: String::new(),
            save_as: String::new(),
            selection_only: false,
            list: None,
            message: String::new(),
        }
    }
}

/// A saved filter as text: its line, or the JSON of a tree saved by an agent.
fn as_text(f: &Value) -> String {
    f.as_str().map(str::to_string).unwrap_or_else(|| f.to_string())
}

/// The filter to send: a JSON tree when the text is one, else the line.
fn as_param(text: &str) -> Value {
    let t = text.trim();
    if t.starts_with('{') { serde_json::from_str(t).unwrap_or_else(|_| json!(t)) } else { json!(t) }
}

/// `value` quoted when it has spaces or operators in it.
fn quoted(value: &str) -> String {
    let v = value.trim();
    if v.is_empty() || v.chars().any(|c| c.is_whitespace() || "()=<>!\"".contains(c)) { format!("\"{}\"", v.replace('"', "")) } else { v.to_string() }
}

fn load_list(app: &mut CadApp) -> Vec<(String, String)> {
    let r = app.session.execute("filter", &json!({ "list": true })).unwrap_or(Value::Null);
    r.get("filters")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|f| Some((f.get("name")?.as_str()?.to_string(), as_text(f.get("filter")?)))).collect())
        .unwrap_or_default()
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let sid = egui::Id::new("filter_dialog_state");
    let mut st: State = ctx.data_mut(|d| d.get_temp(sid)).unwrap_or_default();
    if st.list.is_none() {
        st.list = Some(load_list(app));
    }
    let has_sel = !app.session.selection().is_empty();
    let mut apply = false;
    let mut close = false;
    egui::Window::new(crate::tl!("Object Selection Filters"))
        .id(egui::Id::new("Object Selection Filters"))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(520.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            let mut body = crate::rtl::Body::new(ui);
            let ui = &mut *body;
            ui.label(crate::tl!("Filter:"));
            ui.add(
                egui::TextEdit::multiline(&mut st.filter)
                    .desired_rows(3)
                    .desired_width(500.0)
                    .hint_text("type = Line and (layer = Walls or color = red)")
                    .id_salt("filter_text"),
            );
            ui.add_space(6.0);
            // A condition to add: property, operator, value.
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("filter_property").width(120.0).selected_text(st.property.clone()).show_ui(ui, |ui| {
                    for p in PROPERTIES {
                        ui.selectable_value(&mut st.property, (*p).to_string(), *p);
                    }
                });
                egui::ComboBox::from_id_salt("filter_op").width(50.0).selected_text(st.op.clone()).show_ui(ui, |ui| {
                    for o in OPERATORS {
                        ui.selectable_value(&mut st.op, (*o).to_string(), *o);
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut st.value).desired_width(140.0).hint_text(crate::tl!("Value")));
                for (label, join) in [(crate::tl!("Add (AND)"), "and"), (crate::tl!("Add (OR)"), "or")] {
                    if ui.add_enabled(!st.value.trim().is_empty(), egui::Button::new(label)).clicked() {
                        let cond = format!("{} {} {}", st.property, st.op, quoted(&st.value));
                        st.filter = if st.filter.trim().is_empty() { cond } else { format!("({}) {join} {cond}", st.filter.trim()) };
                        st.value.clear();
                    }
                }
            });
            ui.separator();
            // The saved filters.
            let list = st.list.clone().unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Saved filters:"));
                let shown = if st.saved.is_empty() { crate::i18n::t("(none)").to_owned() } else { st.saved.clone() };
                egui::ComboBox::from_id_salt("filter_saved").width(200.0).selected_text(shown).show_ui(ui, |ui| {
                    for (n, f) in &list {
                        if ui.selectable_label(st.saved == *n, n).clicked() {
                            st.saved = n.clone();
                            st.filter = f.clone();
                            st.message.clear();
                        }
                    }
                });
                if ui.add_enabled(!st.saved.is_empty(), egui::Button::new(crate::tl!("Delete"))).clicked() {
                    match app.session.execute("filter", &json!({ "delete": st.saved })) {
                        Ok(_) => {
                            st.saved.clear();
                            st.list = None;
                        }
                        Err(e) => st.message = e.user_message(),
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut st.save_as).desired_width(200.0).hint_text(crate::tl!("Name")));
                let can = !st.save_as.trim().is_empty() && !st.filter.trim().is_empty();
                if ui.add_enabled(can, egui::Button::new(crate::tl!("Save As"))).clicked() {
                    let p = json!({ "filter": as_param(&st.filter), "save": st.save_as.trim(), "apply": false });
                    match app.session.execute("filter", &p) {
                        Ok(_) => {
                            st.saved = st.save_as.trim().to_string();
                            st.save_as.clear();
                            st.list = None;
                            st.message.clear();
                        }
                        Err(e) => st.message = e.user_message(),
                    }
                }
            });
            ui.separator();
            ui.add_enabled_ui(has_sel, |ui| ui.checkbox(&mut st.selection_only, crate::tl!("Apply to current selection")));
            if !st.message.is_empty() {
                ui.colored_label(crate::theme::Tokens::get().warn, st.message.clone());
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!st.filter.trim().is_empty(), egui::Button::new(crate::tl!("Apply"))).clicked() {
                    apply = true;
                }
                if ui.button(crate::tl!("Cancel")).clicked() {
                    close = true;
                }
            });
            ui.allocate_space(vec2(0.0, 2.0));
        });
    if apply {
        let p = json!({ "filter": as_param(&st.filter), "applyTo": if st.selection_only && has_sel { "selection" } else { "drawing" } });
        match app.session.execute("filter", &p) {
            Ok(r) => {
                if let Some(m) = r.get("message").and_then(Value::as_str) {
                    app.session.echo(m.to_string());
                }
                let sel = app.session.selection();
                if !sel.is_empty() {
                    app.session.remember_selection(&sel);
                }
                st.message.clear();
                close = true;
            }
            Err(e) => st.message = e.user_message(),
        }
    }
    if close {
        *open = false;
    }
    if *open {
        ctx.data_mut(|d| d.insert_temp(sid, st));
    } else {
        // Closed: the next opening reads the saved filters again; the line typed is kept.
        st.list = None;
        st.message.clear();
        ctx.data_mut(|d| d.insert_temp(sid, st));
    }
}
