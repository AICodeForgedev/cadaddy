//! The MEP Review palette (MEPREVIEW typed alone, or Model > MEP Review): every run, fitting and
//! item the reader made, grouped by system and level, with confidence and the source of its
//! height and size; filter "guessed only"; click to zoom to it in plan and 3D; edit
//! size/height/system inline or pick a tag; mark reviewed; rebuild (`model.revise`) keeps edits.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "mepreview";

#[derive(Clone, Default)]
struct State {
    /// Filter by system name (empty = all).
    system_filter: String,
    /// Filter by level name (empty = all).
    level_filter: String,
    /// Show only guessed (default) items.
    guessed_only: bool,
    /// Status message at the bottom.
    status: String,
    /// Which item is selected (by stable run id).
    selected: Option<String>,
    /// Editing state: the item being edited.
    editing: Option<EditState>,
}

#[derive(Clone)]
struct EditState {
    id: String,
    size_w: String,
    size_h: String,
    bottom_ft: String,
    system: String,
}

fn state_id() -> egui::Id {
    egui::Id::new("mep_palette_state")
}

/// Fetch the items list from the engine.
fn fetch_items(app: &mut CadApp, st: &State) -> Value {
    let mut params = json!({});
    if !st.system_filter.is_empty() {
        params["system"] = json!(st.system_filter);
    }
    if !st.level_filter.is_empty() {
        params["level"] = json!(st.level_filter);
    }
    if st.guessed_only {
        params["guessed_only"] = json!(true);
    }
    app.session.execute("mep.list", &params).unwrap_or(Value::Null)
}

/// Zoom to a run's plan objects.
fn zoom_to_item(app: &mut CadApp, item: &Value) {
    let handles: Vec<Value> = item["objects"].as_array().into_iter().flatten().cloned().collect();
    if !handles.is_empty() {
        let _ = app.run("zoom.object", json!({ "handles": handles }));
    }
}

/// Confidence as a colored label.
fn confidence_label(confidence: &str) -> (egui::Color32, &str) {
    match confidence {
        "high" => (egui::Color32::from_rgb(80, 200, 80), "H"),
        "medium" => (egui::Color32::from_rgb(220, 180, 50), "M"),
        _ => (egui::Color32::from_rgb(200, 80, 80), "L"),
    }
}

/// Format a size as a readable string.
fn format_size(size: &Value) -> String {
    let w = size.get(0).and_then(Value::as_f64).unwrap_or(0.0);
    let h = size.get(1).and_then(Value::as_f64).unwrap_or(0.0);
    if h > 0.0 { format!("{w}x{h}\"") } else { format!("{w}\"") }
}

/// Format a bottom height in feet-inches.
fn format_bottom(bottoms: &Value) -> String {
    let first = bottoms.get(0).and_then(Value::as_f64).unwrap_or(0.0);
    let ft = first.floor() as i64;
    let inches = ((first - first.floor()) * 12.0).round() as i64;
    if inches == 0 { format!("{ft}'-0\"") } else { format!("{ft}'-{inches}\"") }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(state_id())).unwrap_or_default();
    let data = fetch_items(app, &st);
    let items = data["items"].as_array().cloned().unwrap_or_default();
    let systems: Vec<String> = data["systems"].as_array().into_iter().flatten().filter_map(|s| s["name"].as_str().map(str::to_string)).collect();
    let levels: Vec<String> = {
        let mut v: Vec<String> = items.iter().filter_map(|it| it["level"].as_str().map(str::to_string)).collect();
        v.sort();
        v.dedup();
        v
    };

    let mut pick: Option<String> = None;
    let mut toggle_reviewed: Option<String> = None;
    let mut apply_edit: Option<EditState> = None;
    let mut start_edit: Option<String> = None;
    let mut cancel_edit = false;
    let mut do_rebuild = false;

    egui::Window::new(crate::tl!("MEP Review"))
        .id(egui::Id::new("mep_palette"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([460.0, 520.0])
        .show(ctx, |ui| {
            // Filter bar.
            ui.horizontal(|ui| {
                ui.label(crate::tl!("System:"));
                egui::ComboBox::from_id_salt("mep_sys_filter")
                    .selected_text(if st.system_filter.is_empty() { crate::tl!("All").to_string() } else { st.system_filter.clone() })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut st.system_filter, String::new(), crate::tl!("All"));
                        for s in &systems {
                            ui.selectable_value(&mut st.system_filter, s.clone(), s.as_str());
                        }
                    });
                ui.label(crate::tl!("Level:"));
                egui::ComboBox::from_id_salt("mep_lvl_filter")
                    .selected_text(if st.level_filter.is_empty() { crate::tl!("All").to_string() } else { st.level_filter.clone() })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut st.level_filter, String::new(), crate::tl!("All"));
                        for l in &levels {
                            ui.selectable_value(&mut st.level_filter, l.clone(), l.as_str());
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut st.guessed_only, crate::tl!("Guessed only"));
                ui.label(RichText::new(crate::tf!("{n} items", n = items.len())).small().color(t.text_dim));
            });
            ui.separator();

            // Items table.
            egui::ScrollArea::vertical().id_salt("mep_items").max_height(360.0).show(ui, |ui| {
                if items.is_empty() {
                    ui.label(RichText::new(crate::tl!("No MEP items found. Run mep.read first.")).color(t.text_dim));
                    return;
                }
                // Group by system then level.
                let mut current_system = String::new();
                let mut current_level = String::new();
                for item in &items {
                    let sys = item["system"].as_str().unwrap_or_default();
                    let lvl = item["level"].as_str().unwrap_or_default();
                    let id = item["id"].as_str().unwrap_or_default().to_string();

                    if sys != current_system {
                        current_system = sys.to_string();
                        current_level.clear();
                        ui.add_space(4.0);
                        ui.strong(sys);
                    }
                    if lvl != current_level {
                        current_level = lvl.to_string();
                        ui.label(RichText::new(format!("  {lvl}")).small().color(t.text_dim));
                    }

                    let selected = st.selected.as_deref() == Some(id.as_str());
                    let is_editing = st.editing.as_ref().is_some_and(|e| e.id == id);
                    let reviewed = item["reviewed"].as_bool() == Some(true);
                    let (conf_color, conf_char) = confidence_label(item["confidence"].as_str().unwrap_or("low"));
                    let kind = item["kind"].as_str().unwrap_or("?");
                    let segs = item["segments"].as_u64().unwrap_or(0);
                    let size_src = item["size_source"].as_str().unwrap_or("default");
                    let ht_src = item["height_source"].as_str().unwrap_or("default");

                    ui.horizontal(|ui| {
                        // Confidence badge.
                        ui.label(RichText::new(conf_char).color(conf_color).strong());
                        // Reviewed checkbox.
                        let mut rev = reviewed;
                        if ui.checkbox(&mut rev, "").on_hover_text(crate::tl!("Mark reviewed")).changed() {
                            toggle_reviewed = Some(id.clone());
                        }
                        // Main label (clickable).
                        let label = format!("{kind} {segs}seg {}  {}", format_size(&item["size_in"]), format_bottom(&item["bottoms_ft"]),);
                        let resp = ui.selectable_label(selected, label);
                        if resp.on_hover_text(crate::tf!("Size: {ss}, Height: {hs}", ss = size_src, hs = ht_src)).clicked() {
                            pick = Some(id.clone());
                        }
                        // Edit button.
                        if !is_editing && ui.small_button(crate::tl!("Edit")).on_hover_text(crate::tl!("Edit size, height or system")).clicked() {
                            start_edit = Some(id.clone());
                        }
                    });

                    // Inline editor.
                    if is_editing && let Some(ed) = &mut st.editing {
                        ui.indent(format!("edit_{id}"), |ui| {
                            ui.horizontal(|ui| {
                                ui.label(crate::tl!("Size W:"));
                                ui.add(egui::TextEdit::singleline(&mut ed.size_w).desired_width(50.0));
                                ui.label(crate::tl!("H:"));
                                ui.add(egui::TextEdit::singleline(&mut ed.size_h).desired_width(50.0));
                            });
                            ui.horizontal(|ui| {
                                ui.label(crate::tl!("Bottom ft:"));
                                ui.add(egui::TextEdit::singleline(&mut ed.bottom_ft).desired_width(60.0));
                            });
                            ui.horizontal(|ui| {
                                ui.label(crate::tl!("System:"));
                                egui::ComboBox::from_id_salt(format!("edit_sys_{id}")).selected_text(&ed.system).show_ui(ui, |ui| {
                                    for s in &systems {
                                        ui.selectable_value(&mut ed.system, s.clone(), s.as_str());
                                    }
                                });
                            });
                            ui.horizontal(|ui| {
                                if ui.button(crate::tl!("Apply")).clicked() {
                                    apply_edit = Some(ed.clone());
                                }
                                if ui.button(crate::tl!("Cancel")).clicked() {
                                    cancel_edit = true;
                                }
                            });
                        });
                    }
                }
            });

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Rebuild")).on_hover_text(crate::tl!("Rebuild 3D model keeping hand edits")).clicked() {
                    do_rebuild = true;
                }
                if !st.status.is_empty() {
                    ui.label(RichText::new(&st.status).small());
                }
            });
        });

    // Actions.
    if let Some(id) = pick
        && let Some(item) = items.iter().find(|it| it["id"].as_str() == Some(id.as_str()))
    {
        zoom_to_item(app, item);
        st.selected = Some(id);
        let sys = item["system"].as_str().unwrap_or_default();
        let lvl = item["level"].as_str().unwrap_or_default();
        st.status = crate::tf!("Zoomed to {sys} run on {lvl}.", sys = sys, lvl = lvl);
    }
    if let Some(id) = toggle_reviewed {
        let reviewed = items.iter().find(|it| it["id"].as_str() == Some(id.as_str())).and_then(|it| it["reviewed"].as_bool()).unwrap_or(false);
        match app.run("mep.edit", json!({ "id": id, "reviewed": !reviewed })) {
            Ok(_) => {
                st.status = if !reviewed { crate::tl!("Marked as reviewed.").to_string() } else { crate::tl!("Unmarked.").to_string() };
            }
            Err(e) => st.status = e,
        }
    }
    if let Some(id) = start_edit
        && let Some(item) = items.iter().find(|it| it["id"].as_str() == Some(id.as_str()))
    {
        let w = item["size_in"].get(0).and_then(Value::as_f64).unwrap_or(0.0);
        let h = item["size_in"].get(1).and_then(Value::as_f64).unwrap_or(0.0);
        let b = item["bottoms_ft"].get(0).and_then(Value::as_f64).unwrap_or(0.0);
        st.editing = Some(EditState {
            id,
            size_w: format!("{w}"),
            size_h: format!("{h}"),
            bottom_ft: format!("{b}"),
            system: item["system"].as_str().unwrap_or_default().to_string(),
        });
    }
    if cancel_edit {
        st.editing = None;
    }
    if let Some(ed) = apply_edit {
        let w: f64 = ed.size_w.parse().unwrap_or(0.0);
        let h: f64 = ed.size_h.parse().unwrap_or(0.0);
        let b: f64 = ed.bottom_ft.parse().unwrap_or(f64::NAN);
        let mut params = json!({ "id": &ed.id });
        if w > 0.0 {
            params["size_in"] = json!([w, h]);
        }
        if b.is_finite() {
            params["bottom_ft"] = json!(b);
        }
        if !ed.system.is_empty() {
            params["system"] = json!(ed.system);
        }
        match app.run("mep.edit", params) {
            Ok(_) => {
                st.status = crate::tl!("Edit applied.").to_string();
                st.editing = None;
            }
            Err(e) => st.status = e,
        }
    }
    if do_rebuild {
        st.status = match app.run("model.build", json!({})) {
            Ok(r) => {
                let runs = r["mepRuns"].as_u64().unwrap_or(0);
                crate::tf!("Rebuilt: {n} MEP runs.", n = runs)
            }
            Err(e) => e,
        };
    }
    ctx.data_mut(|d| d.insert_temp(state_id(), st));
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_engine::Session;
    use serde_json::json;

    /// Set up a session with some MEP runs in the building spec.
    fn mep_session() -> CadApp {
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        // Set the building spec directly on the drawing.
        let spec = json!({
            "levels": [{"name": "Level 1", "elevation_ft": 0.0, "height_ft": 12.0, "ceiling_ft": 9.0}],
            "units_per_foot": 12.0,
            "mep": {
                "systems": [
                    {"name": "SA-1", "layer": "M-DUCT-SUPP", "kind": "duct", "size_in": [24.0, 12.0], "bottom_ft": 10.0, "show": true},
                    {"name": "CW-1", "layer": "P-DOMW-CW", "kind": "pipe", "size_in": [2.0, 0.0], "bottom_ft": 10.0, "show": true}
                ],
                "runs": [
                    {"system": "SA-1", "level": "Level 1", "points": [[0.0,0.0],[120.0,0.0],[120.0,60.0]], "size_in": [24.0, 12.0], "bottoms_ft": [10.0, 10.0], "objects": ["1A", "1B"], "size_source": "default", "height_source": "default"},
                    {"system": "SA-1", "level": "Level 1", "points": [[0.0,60.0],[60.0,60.0]], "size_in": [18.0, 10.0], "bottoms_ft": [9.5], "objects": ["2A"], "size_source": "tag", "height_source": "tag"},
                    {"system": "CW-1", "level": "Level 1", "points": [[0.0,0.0],[0.0,120.0]], "size_in": [2.0, 0.0], "bottoms_ft": [10.0], "objects": ["3A"], "size_source": "default", "height_source": "default"}
                ]
            }
        });
        app.session.doc_mut().unwrap().building_spec = Some(serde_json::to_string(&spec).unwrap_or_default());
        app
    }

    #[test]
    fn mep_list_returns_all_runs() {
        let mut app = mep_session();
        let r = app.session.execute("mep.list", &json!({})).unwrap();
        assert_eq!(r["total"].as_u64(), Some(3));
        let items = r["items"].as_array().unwrap();
        // A default run (size = system default) has low confidence.
        let default_run = items.iter().find(|it| it["objects"] == json!(["1A", "1B"])).unwrap();
        assert_eq!(default_run["confidence"].as_str(), Some("low"));
        assert_eq!(default_run["size_source"].as_str(), Some("default"));
        // The tagged run (different size and height) has high confidence.
        let tagged_run = items.iter().find(|it| it["objects"] == json!(["2A"])).unwrap();
        assert_eq!(tagged_run["confidence"].as_str(), Some("high"));
        assert_eq!(tagged_run["size_source"].as_str(), Some("tag"));
        assert_eq!(tagged_run["height_source"].as_str(), Some("tag"));
    }

    #[test]
    fn mep_list_filters_by_system() {
        let mut app = mep_session();
        let r = app.session.execute("mep.list", &json!({"system": "CW-1"})).unwrap();
        assert_eq!(r["total"].as_u64(), Some(1));
        assert_eq!(r["items"][0]["system"].as_str(), Some("CW-1"));
    }

    #[test]
    fn mep_list_guessed_only_shows_defaults() {
        let mut app = mep_session();
        let r = app.session.execute("mep.list", &json!({"guessed_only": true})).unwrap();
        // Only items with all defaults (confidence=low).
        let items = r["items"].as_array().unwrap();
        for it in items {
            assert_eq!(it["confidence"].as_str(), Some("low"));
        }
    }

    #[test]
    fn mep_edit_changes_size_and_marks_reviewed() {
        let mut app = mep_session();
        // Trigger id assignment via mep.list.
        let list = app.session.execute("mep.list", &json!({})).unwrap();
        let id0 = list["items"][0]["id"].as_str().unwrap().to_string();
        let _ = app.session.execute("mep.edit", &json!({"id": &id0, "size_in": [30.0, 14.0], "reviewed": true})).unwrap();
        let r = app.session.execute("mep.list", &json!({})).unwrap();
        let item = r["items"].as_array().unwrap().iter().find(|it| it["id"].as_str() == Some(id0.as_str())).unwrap();
        assert_eq!(item["size_in"], json!([30.0, 14.0]));
        assert_eq!(item["reviewed"].as_bool(), Some(true));
        assert_eq!(item["size_source"].as_str(), Some("override"), "an edit sets source to override");
    }

    #[test]
    fn mep_edit_changes_bottom() {
        let mut app = mep_session();
        let list = app.session.execute("mep.list", &json!({})).unwrap();
        let id2 = list["items"][2]["id"].as_str().unwrap().to_string();
        let _ = app.session.execute("mep.edit", &json!({"id": &id2, "bottom_ft": 8.5})).unwrap();
        let r = app.session.execute("mep.list", &json!({})).unwrap();
        let item = r["items"].as_array().unwrap().iter().find(|it| it["id"].as_str() == Some(id2.as_str())).unwrap();
        let b = item["bottoms_ft"][0].as_f64().unwrap();
        assert!((b - 8.5).abs() < 1e-6);
        assert_eq!(item["height_source"].as_str(), Some("override"), "an edit sets source to override");
    }

    #[test]
    fn mepreview_opens_from_command_and_renders() {
        let mut app = mep_session();
        crate::menus::run_ui_command(&mut app, "mepreview", &Value::Null);
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
    }

    #[test]
    fn mepreview_renders_in_arabic() {
        let mut app = mep_session();
        let _ = app.run("ui.language", json!({"lang": "ar"}));
        app.ui.dialog = Some(DIALOG.into());
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        // It rendered without panic (RTL layout exercised).
    }
}
