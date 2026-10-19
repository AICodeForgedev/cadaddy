//! The Layer States Manager (LAYERSTATE typed or from Format ▸ Layer States Manager…, or its
//! button in the Layer Properties Manager): the saved layer states with their descriptions; New,
//! Save, Edit, Rename, Delete, Import, Export and Restore; the layer properties restoring sets and
//! "Turn off layers not found in layer state". Every change is a `layerstate.*` call.

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

/// The mask's properties: (JSON key, label).
const PROPS: [(&str, &str); 10] = [
    ("on", "On / Off"),
    ("frozen", "Frozen / Thawed"),
    ("locked", "Locked / Unlocked"),
    ("plot", "Plot / No Plot"),
    ("vpFreezeNew", "New VP Freeze"),
    ("color", "Color"),
    ("linetype", "Linetype"),
    ("lineweight", "Lineweight"),
    ("transparency", "Transparency"),
    ("plotStyle", "Plot Style"),
];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// The dialog to go back to on Close (the Layer Properties Manager, when opened from it).
    back: Option<String>,
    selected: Option<String>,
    /// New state: (name, description).
    new: Option<(String, String)>,
    /// Rename: the new name.
    rename: Option<String>,
    /// Edit: the state's layers are listed.
    editing: bool,
    error: Option<String>,
}

impl State {
    /// The dialog to return to when this one closes.
    pub fn back(self) -> Option<String> {
        self.back
    }
}

pub fn open(app: &mut CadApp, back: Option<&str>) {
    app.layer_states_dlg = Some(State { back: back.map(str::to_string), ..State::default() });
    app.ui.dialog = Some("layerstates".into());
}

fn run(app: &mut CadApp, st: &mut State, cmd: &str, p: Value) -> bool {
    match app.run(cmd, p) {
        Ok(_) => {
            st.error = None;
            true
        }
        Err(e) => {
            st.error = Some(e);
            false
        }
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.layer_states_dlg.clone() else {
        *open = false;
        return;
    };
    let Ok(d) = app.session.doc() else {
        *open = false;
        return;
    };
    let states = d.layer_states.clone();
    if st.selected.as_ref().is_some_and(|n| !states.iter().any(|s| &s.name == n)) {
        st.selected = None;
    }
    let current = st.selected.as_ref().and_then(|n| states.iter().find(|s| &s.name == n)).cloned();
    let mut calls: Vec<(&str, Value)> = Vec::new();
    let mut close = false;
    egui::Window::new(crate::tl!("Layer States Manager"))
        .id(egui::Id::new("layer_states_manager"))
        .open(open)
        .collapsible(false)
        .default_size(vec2(620.0, 380.0))
        // Centred, so it fits on the screen whatever opened it.
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_min_width(380.0);
                    ui.label(RichText::new(crate::tl!("Layer states")).strong());
                    egui::ScrollArea::vertical().id_salt("lsm_list").max_height(140.0).show(ui, |ui| {
                        egui::Grid::new("lsm_grid").striped(true).num_columns(3).show(ui, |ui| {
                            ui.label(RichText::new(crate::tl!("Name")).strong());
                            ui.label(RichText::new(crate::tl!("Description")).strong());
                            ui.label(RichText::new(crate::tl!("Layers")).strong());
                            ui.end_row();
                            for s in &states {
                                if ui.selectable_label(st.selected.as_deref() == Some(s.name.as_str()), &s.name).clicked() {
                                    st.selected = Some(s.name.clone());
                                    st.rename = None;
                                }
                                ui.label(&s.description);
                                ui.label(s.layers.len().to_string());
                                ui.end_row();
                            }
                        });
                        if states.is_empty() {
                            ui.weak(crate::tl!("No saved layer states"));
                        }
                    });
                    if let Some(cur) = &current {
                        ui.separator();
                        let mut off = cur.off_others;
                        if ui.checkbox(&mut off, crate::tl!("Turn off layers not found in layer state")).changed() {
                            calls.push(("layerstate.edit", json!({ "name": cur.name, "offOthers": off })));
                        }
                        ui.label(crate::tl!("Layer properties to restore"));
                        let mask = serde_json::to_value(cur.restore).unwrap_or(Value::Null);
                        egui::Grid::new("lsm_mask").num_columns(3).show(ui, |ui| {
                            for (i, (key, label)) in PROPS.iter().enumerate() {
                                let mut on = mask[*key].as_bool().unwrap_or(true);
                                if ui.checkbox(&mut on, crate::i18n::t(label)).changed() {
                                    let mut m = mask.clone();
                                    m[*key] = json!(on);
                                    calls.push(("layerstate.edit", json!({ "name": cur.name, "restore": m })));
                                }
                                if i % 3 == 2 {
                                    ui.end_row();
                                }
                            }
                        });
                        if st.editing {
                            ui.separator();
                            ui.label(RichText::new(crate::tf!("Layers in \"{name}\"", name = cur.name)).strong());
                            egui::ScrollArea::vertical().id_salt("lsm_layers").max_height(160.0).show(ui, |ui| {
                                egui::Grid::new("lsm_layers_grid").striped(true).num_columns(6).show(ui, |ui| {
                                    for h in ["Name", "On", "Freeze", "Lock", "Plot", ""] {
                                        ui.label(RichText::new(crate::i18n::t(h)).strong());
                                    }
                                    ui.end_row();
                                    for l in &cur.layers {
                                        ui.label(&l.name);
                                        for (key, v) in [("on", l.on), ("frozen", l.frozen), ("locked", l.locked), ("plot", l.plot)] {
                                            let mut b = v;
                                            if ui.checkbox(&mut b, "").changed() {
                                                calls.push(("layerstate.edit", json!({ "name": cur.name, "layers": [{ "name": l.name, key: b }] })));
                                            }
                                        }
                                        if ui.small_button("✕").on_hover_text(crate::tl!("Remove from the layer state")).clicked() {
                                            calls.push(("layerstate.edit", json!({ "name": cur.name, "removeLayers": [l.name] })));
                                        }
                                        ui.end_row();
                                    }
                                });
                            });
                        }
                    }
                    if let Some(e) = &st.error {
                        ui.colored_label(Tokens::get().warn, e);
                    }
                });
                ui.vertical(|ui| {
                    let has = current.is_some();
                    let w = [130.0, 22.0];
                    if ui.add_sized(w, egui::Button::new(crate::tl!("New..."))).clicked() {
                        st.new = Some((String::new(), String::new()));
                    }
                    if ui
                        .add_enabled(has, egui::Button::new(crate::tl!("Save")).min_size(w.into()))
                        .on_hover_text(crate::tl!("Save the layers as they are now in this state"))
                        .clicked()
                        && let Some(c) = &current
                    {
                        calls.push(("layerstate.save", json!({ "name": c.name })));
                    }
                    if ui.add_enabled(has, egui::Button::new(crate::tl!("Edit...")).min_size(w.into())).clicked() {
                        st.editing = !st.editing;
                    }
                    if ui.add_enabled(has, egui::Button::new(crate::tl!("Rename")).min_size(w.into())).clicked() {
                        st.rename = current.as_ref().map(|c| c.name.clone());
                    }
                    if ui.add_enabled(has, egui::Button::new(crate::tl!("Delete")).min_size(w.into())).clicked()
                        && let Some(c) = &current
                    {
                        calls.push(("layerstate.delete", json!({ "name": c.name })));
                    }
                    ui.add_space(6.0);
                    if ui.add_sized(w, egui::Button::new(crate::tl!("Import..."))).clicked()
                        && let Some(path) = app.services.pick_open.as_ref().and_then(|f| f())
                    {
                        calls.push(("layerstate.import", json!({ "path": path })));
                    }
                    if ui.add_enabled(has, egui::Button::new(crate::tl!("Export...")).min_size(w.into())).clicked()
                        && let Some(c) = &current
                        && let Some(path) = app.services.pick_save.as_ref().and_then(|f| f(&format!("{}.las", c.name)))
                    {
                        calls.push(("layerstate.export", json!({ "name": c.name, "path": path })));
                    }
                    ui.add_space(6.0);
                    if ui.add_enabled(has, egui::Button::new(crate::tl!("Restore")).min_size(w.into())).clicked()
                        && let Some(c) = &current
                    {
                        calls.push(("layerstate.restore", json!({ "name": c.name })));
                    }
                    if ui.add_sized(w, egui::Button::new(crate::tl!("Close"))).clicked() {
                        close = true;
                    }
                });
            });
            let (mut new_ok, mut new_cancel) = (false, false);
            if let Some((name, desc)) = &mut st.new {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("New layer state name"));
                    ui.text_edit_singleline(name);
                });
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Description"));
                    ui.text_edit_singleline(desc);
                });
                ui.horizontal(|ui| {
                    new_ok = ui.add_enabled(!name.trim().is_empty(), egui::Button::new(crate::tl!("OK"))).clicked();
                    new_cancel = ui.button(crate::tl!("Cancel")).clicked();
                });
            }
            if new_ok && let Some((name, desc)) = st.new.take() {
                calls.push(("layerstate.save", json!({ "name": name.trim(), "description": desc })));
                st.selected = Some(name.trim().to_string());
            } else if new_cancel {
                st.new = None;
            }
            let mut rename_ok = false;
            if let (Some(to), Some(_)) = (&mut st.rename, &current) {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("New name"));
                    ui.text_edit_singleline(to);
                    rename_ok = ui.button(crate::tl!("OK")).clicked();
                });
            }
            if rename_ok && let (Some(to), Some(cur)) = (st.rename.take(), &current) {
                calls.push(("layerstate.rename", json!({ "from": cur.name, "to": to.trim() })));
                st.selected = Some(to.trim().to_string());
            }
        });
    for (cmd, p) in calls {
        if !run(app, &mut st, cmd, p) {
            break;
        }
    }
    if close || !*open {
        *open = false;
        app.layer_states_dlg = None;
        app.ui.dialog = st.back.clone();
        return;
    }
    app.layer_states_dlg = Some(st);
}
