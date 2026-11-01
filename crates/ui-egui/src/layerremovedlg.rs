//! The Layer Merge and Layer Delete dialogs (LAYMRG, LAYDEL; Format ▸ Layer Tools, the ribbon):
//! the drawing's layers in a list with a name filter, a check box and an object count on each,
//! a target layer for LAYMRG, and a summary of what Confirm will move or erase. Confirm runs
//! the same `laymrg {from, to}` / `laydel {names}` commands a script or an agent calls; those
//! never open the dialog.

use std::collections::{BTreeSet, HashMap};

use egui::RichText;
use serde_json::json;

use crate::CadApp;

pub const MERGE: &str = "laymrg";
pub const DELETE: &str = "laydel";

#[derive(Clone, Default)]
struct State {
    /// Layer names checked in the list.
    picked: BTreeSet<String>,
    filter: String,
    /// LAYMRG: the layer the checked layers merge into ("" until chosen).
    target: String,
    /// Objects per layer (lower-case name), counted when the dialog opened.
    counts: HashMap<String, usize>,
    status: String,
}

/// One row of the list: the layer's name, its object count and why it can't be checked.
struct Row {
    name: String,
    count: usize,
    refusal: Option<String>,
}

fn id(mode: &str) -> egui::Id {
    egui::Id::new(("layer_remove_dialog_state", mode))
}

fn rows(app: &CadApp, st: &State, delete: bool) -> Vec<Row> {
    let Ok(d) = app.session.doc() else { return Vec::new() };
    d.layers
        .iter()
        .map(|l| Row {
            name: l.name.clone(),
            count: st.counts.get(&l.name.to_ascii_lowercase()).copied().unwrap_or(0),
            refusal: cadaddy_engine::cmd::layer_removal_refusal(d, &l.name, delete),
        })
        .collect()
}

/// Whether `name` passes the filter: wildcards (`*`, `?`) match the whole name, anything else is
/// a case-insensitive part of it.
fn passes(filter: &str, name: &str) -> bool {
    let f = filter.trim().to_lowercase();
    if f.is_empty() {
        return true;
    }
    let n = name.to_lowercase();
    if f.contains(['*', '?']) { cadaddy_engine::cmd::wildcard(&f, &n) } else { n.contains(&f) }
}

/// The line under the list: what Confirm will do, or `None` while nothing is checked.
fn summary(rows: &[Row], st: &State, delete: bool) -> Option<String> {
    let chosen: Vec<&Row> = rows.iter().filter(|r| st.picked.contains(&r.name)).collect();
    if chosen.is_empty() {
        return None;
    }
    let objects: usize = chosen.iter().map(|r| r.count).sum();
    Some(if delete {
        crate::tf!("Deleting {layers} layers erases {objects} objects.", layers = chosen.len(), objects = objects)
    } else if st.target.is_empty() {
        crate::tf!("Merging {layers} layers moves {objects} objects. Choose the target layer.", layers = chosen.len(), objects = objects)
    } else {
        crate::tf!(
            "Merging {layers} layers moves {objects} objects to \"{target}\" and deletes the layers.",
            layers = chosen.len(),
            objects = objects,
            target = st.target.as_str()
        )
    })
}

/// Whether Confirm can run now.
fn ready(st: &State, delete: bool) -> bool {
    !st.picked.is_empty() && (delete || (!st.target.is_empty() && !st.picked.contains(&st.target)))
}

/// Run the command for the checked layers.
fn confirm(app: &mut CadApp, st: &State, delete: bool) -> Result<String, String> {
    let names: Vec<&String> = st.picked.iter().collect();
    let r = if delete { app.run("laydel", json!({ "names": names })) } else { app.run("laymrg", json!({ "from": names, "to": st.target })) };
    r.map(|v| v["message"].as_str().unwrap_or_default().to_string())
}

/// Open the dialog for LAYMRG (`delete` false) or LAYDEL.
pub fn open(app: &mut CadApp, delete: bool) {
    app.ui.dialog = Some(if delete { DELETE } else { MERGE }.into());
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool, delete: bool) {
    let t = crate::theme::Tokens::get();
    let key = id(if delete { DELETE } else { MERGE });
    let mut st: State = ctx
        .data(|d| d.get_temp(key))
        .unwrap_or_else(|| State { counts: app.session.doc().map(cadaddy_engine::cmd::objects_per_layer).unwrap_or_default(), ..State::default() });
    let list = rows(app, &st, delete);
    let mut done = false;
    let mut cancel = false;
    let title = if delete { crate::tl!("Layer Delete") } else { crate::tl!("Layer Merge") };
    egui::Window::new(title)
        .id(egui::Id::new(("layer_remove_dialog", delete)))
        .open(open)
        .resizable(true)
        .collapsible(false)
        .default_width(340.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Filter:"));
                ui.add(egui::TextEdit::singleline(&mut st.filter).desired_width(160.0).hint_text("A*"));
            });
            egui::ScrollArea::vertical().id_salt("layer_remove_list").max_height(280.0).auto_shrink([false, true]).show(ui, |ui| {
                for r in list.iter().filter(|r| passes(&st.filter, &r.name)) {
                    ui.horizontal(|ui| {
                        let mut on = st.picked.contains(&r.name);
                        let resp = ui.add_enabled(r.refusal.is_none(), egui::Checkbox::new(&mut on, r.name.as_str()));
                        if let Some(why) = &r.refusal {
                            resp.on_disabled_hover_text(why);
                        } else if resp.changed() {
                            if on {
                                st.picked.insert(r.name.clone());
                            } else {
                                st.picked.remove(&r.name);
                            }
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(r.count.to_string()).small().color(t.text_dim));
                        });
                    });
                }
            });
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Select All")).clicked() {
                    let shown = list.iter().filter(|r| r.refusal.is_none() && passes(&st.filter, &r.name));
                    st.picked.extend(shown.map(|r| r.name.clone()));
                }
                if ui.button(crate::tl!("Clear")).clicked() {
                    st.picked.clear();
                }
            });
            if !delete {
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Target layer:"));
                    let shown = if st.target.is_empty() { "—".to_string() } else { st.target.clone() };
                    egui::ComboBox::from_id_salt("layer_remove_target").selected_text(shown).show_ui(ui, |ui| {
                        for r in list.iter().filter(|r| !st.picked.contains(&r.name) && !r.name.contains('|')) {
                            ui.selectable_value(&mut st.target, r.name.clone(), r.name.as_str());
                        }
                    });
                });
            }
            ui.separator();
            match summary(&list, &st, delete) {
                Some(s) => ui.label(s),
                None => ui.label(RichText::new(crate::tl!("Check the layers to change.")).color(t.text_dim)),
            };
            if !st.status.is_empty() {
                ui.label(RichText::new(&st.status).small());
            }
            ui.horizontal(|ui| {
                let label = if delete { crate::tl!("Delete") } else { crate::tl!("Merge") };
                if ui.add_enabled(ready(&st, delete), egui::Button::new(label)).clicked() {
                    match confirm(app, &st, delete) {
                        Ok(_) => done = true,
                        Err(e) => st.status = e,
                    }
                }
                if ui.button(crate::tl!("Cancel")).clicked() {
                    cancel = true;
                }
            });
        });
    if done || cancel {
        *open = false;
    }
    if *open {
        ctx.data_mut(|d| d.insert_temp(key, st));
    } else {
        ctx.data_mut(|d| d.remove::<State>(key));
    }
}

#[cfg(test)]
mod tests;
