//! The BIM Coordination palette: a dockable panel for reviewing MEP and structural
//! coordination in 3D. Tabs: Model Tree, Trades, Clashes, Views, Section Box, Measure.
//!
//! The palette drives the `coordination.*` engine commands and reads their results to show
//! a live, interactive coordination viewer that matches or beats Navisworks for the
//! coordination review workflow.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

pub const DIALOG: &str = "bimcoordination";

/// Which tab the palette shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Tree,
    Trades,
    Clashes,
    Views,
    SectionBox,
    Measure,
}

#[derive(Clone, Default)]
struct State {
    tab: Tab,
    tree_data: Option<Value>,
    trades_data: Option<Value>,
    clash_data: Option<Value>,
    views_data: Option<Value>,
    sectionbox: Option<Value>,
    search: String,
    measure_result: Option<Value>,
    /// Name typed into the view save field.
    view_name: String,
    /// The selected clash index for navigation.
    selected_clash: Option<usize>,
    /// Expanded levels in the tree.
    expanded: std::collections::HashSet<String>,
    /// Coordination mode state.
    coord_mode: Option<Value>,
    /// Level filter for trades.
    level_filter: String,
}

fn id() -> egui::Id {
    egui::Id::new("bim_palette_state")
}

fn state(ctx: &egui::Context) -> State {
    ctx.data_mut(|d| d.get_temp::<State>(id()).unwrap_or_default())
}

fn set_state(ctx: &egui::Context, s: State) {
    ctx.data_mut(|d| d.insert_temp(id(), s));
}

pub fn show(app: &mut CadApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let mut st = state(ui.ctx());

    // Tab bar.
    ui.horizontal(|ui| {
        let tabs = [
            (Tab::Tree, "Tree"),
            (Tab::Trades, "Trades"),
            (Tab::Clashes, "Clashes"),
            (Tab::Views, "Views"),
            (Tab::SectionBox, "Section"),
            (Tab::Measure, "Measure"),
        ];
        for (tab, label) in tabs {
            let selected = st.tab == tab;
            let text =
                if selected { RichText::new(crate::i18n::t(label)).color(t.accent) } else { RichText::new(crate::i18n::t(label)).color(t.text_dim) };
            if ui.selectable_label(selected, text).clicked() {
                st.tab = tab;
            }
        }
    });
    ui.separator();

    match st.tab {
        Tab::Tree => tree_tab(app, ui, &mut st),
        Tab::Trades => trades_tab(app, ui, &mut st),
        Tab::Clashes => clashes_tab(app, ui, &mut st),
        Tab::Views => views_tab(app, ui, &mut st),
        Tab::SectionBox => sectionbox_tab(app, ui, &mut st),
        Tab::Measure => measure_tab(app, ui, &mut st),
    }

    set_state(ui.ctx(), st);
}

// ─── Model Tree tab ───────────────────────────────────────────────────────────

fn tree_tab(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State) {
    // Refresh button.
    ui.horizontal(|ui| {
        if ui.button(crate::i18n::t("Refresh")).clicked() {
            st.tree_data = app.session.execute("coordination.tree", &json!({})).ok();
        }
        ui.text_edit_singleline(&mut st.search);
    });

    // Auto-load on first show.
    if st.tree_data.is_none() {
        st.tree_data = app.session.execute("coordination.tree", &json!({})).ok();
    }

    let Some(data) = &st.tree_data else {
        ui.label(crate::i18n::t("No building spec. Use Plans to Model first."));
        return;
    };

    let levels = data.get("levels").and_then(Value::as_array);
    let Some(levels) = levels else {
        ui.label(crate::i18n::t("No levels found."));
        return;
    };

    let filter = st.search.to_ascii_lowercase();
    egui::ScrollArea::vertical().id_salt("bim-tree").auto_shrink([false, false]).show(ui, |ui| {
        for level in levels {
            let name = level.get("name").and_then(Value::as_str).unwrap_or("?");
            let elev = level.get("elevation_ft").and_then(Value::as_f64).unwrap_or(0.0);
            let elements = level.get("elements").and_then(Value::as_u64).unwrap_or(0);

            if !filter.is_empty() && !name.to_ascii_lowercase().contains(&filter) {
                continue;
            }

            let expanded = st.expanded.contains(name);
            let header = format!("{name}  ({elev:.1} ft, {elements} elements)");
            let resp = ui.selectable_label(expanded, RichText::new(&header).strong());
            if resp.clicked() {
                if expanded {
                    st.expanded.remove(name);
                } else {
                    st.expanded.insert(name.to_string());
                }
            }

            if expanded {
                ui.indent(name, |ui| {
                    // Rooms.
                    if let Some(rooms) = level.get("rooms").and_then(Value::as_array)
                        && !rooms.is_empty()
                    {
                        ui.label(RichText::new(crate::i18n::t("Rooms")).color(Tokens::get().text_dim));
                        for room in rooms {
                            let rn = room.get("name").and_then(Value::as_str).unwrap_or("?");
                            let cf = room.get("ceiling_ft").and_then(Value::as_f64).unwrap_or(0.0);
                            ui.label(format!("  {rn} (ceiling {cf:.1} ft)"));
                        }
                    }
                    // Systems.
                    if let Some(systems) = level.get("systems").and_then(Value::as_array)
                        && !systems.is_empty()
                    {
                        ui.label(RichText::new(crate::i18n::t("Systems")).color(Tokens::get().text_dim));
                        for sys in systems {
                            let sn = sys.get("name").and_then(Value::as_str).unwrap_or("?");
                            let kind = sys.get("kind").and_then(Value::as_str).unwrap_or("?");
                            let count = sys.get("count").and_then(Value::as_u64).unwrap_or(0);
                            ui.label(format!("  {sn} ({kind}, {count} runs)"));
                        }
                    }
                });
            }
        }
    });
}

// ─── Trades tab ───────────────────────────────────────────────────────────────

fn trades_tab(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State) {
    // Coordination mode toggle.
    if st.coord_mode.is_none() {
        st.coord_mode = app.session.execute("coordination.mode", &json!({})).ok();
    }
    let mode_on = st.coord_mode.as_ref().and_then(|v| v.get("on")).and_then(Value::as_bool).unwrap_or(false);

    ui.horizontal(|ui| {
        let mode_label = if mode_on { "Coordination: ON" } else { "Coordination: OFF" };
        if ui.button(crate::i18n::t(mode_label)).clicked() {
            st.coord_mode = app.session.execute("coordination.mode", &json!({ "on": !mode_on })).ok();
        }
    });
    ui.separator();

    // Level filter.
    ui.horizontal(|ui| {
        ui.label(crate::i18n::t("Level:"));
        let changed = ui.text_edit_singleline(&mut st.level_filter).changed();
        if ui.button(crate::i18n::t("Refresh")).clicked() || st.trades_data.is_none() || changed {
            let params = if st.level_filter.is_empty() { json!({}) } else { json!({ "level": st.level_filter }) };
            st.trades_data = app.session.execute("coordination.trades", &params).ok();
        }
        // Hide the MEP objects of the other levels (undo or Show All brings them back).
        if ui.add_enabled(!st.level_filter.trim().is_empty(), egui::Button::new(crate::i18n::t("Isolate Level"))).clicked()
            && let Err(e) = app.session.execute("coordination.isolate", &json!({ "level": st.level_filter.trim() }))
        {
            app.set_status(e.to_string());
        }
    });

    let Some(data) = &st.trades_data.clone() else {
        ui.label(crate::i18n::t("No MEP systems found."));
        return;
    };

    let trades = data.get("trades").and_then(Value::as_array);
    let Some(trades) = trades else {
        ui.label(crate::i18n::t("No trades."));
        return;
    };

    let t = Tokens::get();
    ui.horizontal(|ui| {
        if ui.button(crate::i18n::t("Show All")).clicked() {
            let sets: Vec<Value> = trades.iter().map(|tr| json!({ "system": tr.get("system"), "visible": true })).collect();
            let _ = app.session.execute("coordination.isolate", &json!({ "reset": true }));
            st.trades_data = app.session.execute("coordination.trades", &json!({ "set": sets })).ok();
        }
        if ui.button(crate::i18n::t("Hide All")).clicked() {
            let sets: Vec<Value> = trades.iter().map(|tr| json!({ "system": tr.get("system"), "visible": false })).collect();
            st.trades_data = app.session.execute("coordination.trades", &json!({ "set": sets })).ok();
        }
    });
    ui.separator();

    egui::ScrollArea::vertical().id_salt("bim-trades").auto_shrink([false, false]).show(ui, |ui| {
        for trade in trades {
            let system = trade.get("system").and_then(Value::as_str).unwrap_or("?");
            let kind = trade.get("kind").and_then(Value::as_str).unwrap_or("?");
            let visible = trade.get("visible").and_then(Value::as_bool).unwrap_or(true);
            let count = trade.get("count").and_then(Value::as_u64).unwrap_or(0);

            ui.horizontal(|ui| {
                let mut vis = visible;
                if ui.checkbox(&mut vis, "").changed() {
                    st.trades_data = app.session.execute("coordination.trades", &json!({ "set": [{ "system": system, "visible": vis }] })).ok();
                }
                let color = if visible { t.text } else { t.text_faint };
                ui.label(RichText::new(format!("{system} ({kind}, {count} runs)")).color(color));
                // Isolate button.
                // Isolate button: this system (on the level typed above, if any).
                if ui.small_button(crate::i18n::t("Isolate")).clicked() {
                    let level = st.level_filter.trim();
                    let params = if level.is_empty() { json!({ "system": system }) } else { json!({ "system": system, "level": level }) };
                    if let Err(e) = app.session.execute("coordination.isolate", &params) {
                        app.set_status(e.to_string());
                    }
                    st.trades_data = app.session.execute("coordination.trades", &json!({})).ok();
                }
            });
        }
    });
}

// ─── Clashes tab ──────────────────────────────────────────────────────────────

fn clashes_tab(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State) {
    ui.horizontal(|ui| {
        if ui.button(crate::i18n::t("Run Clash Check")).clicked() {
            st.clash_data = app.session.execute("coordination.clashlist", &json!({})).ok();
            st.selected_clash = None;
        }
        if ui.button(crate::i18n::t("Export BCF")).clicked() {
            // Export to a default path next to the drawing.
            let path = std::env::temp_dir().join("cadaddy_clashes.bcf");
            if let Some(p) = path.to_str() {
                let _ = app.session.execute("coordination.bcf", &json!({ "path": p }));
                app.set_status(format!("BCF exported to {p}"));
            }
        }
    });

    let Some(data) = &st.clash_data else {
        ui.label(crate::i18n::t("Press Run Clash Check to detect clashes."));
        return;
    };

    let total = data.get("total").and_then(Value::as_u64).unwrap_or(0);
    let t = Tokens::get();
    ui.label(RichText::new(format!("{total} clashes found")).color(if total > 0 { t.warn } else { t.text }));
    ui.separator();

    let groups = data.get("groups").and_then(Value::as_array);
    let Some(groups) = groups else { return };

    egui::ScrollArea::vertical().id_salt("bim-clashes").auto_shrink([false, false]).show(ui, |ui| {
        let mut global_idx = 0usize;
        for group in groups {
            let key = group.get("key").and_then(Value::as_str).unwrap_or("?");
            let clashes = group.get("clashes").and_then(Value::as_array);
            let Some(clashes) = clashes else { continue };

            ui.label(RichText::new(format!("{key} ({} clashes)", clashes.len())).strong());
            for clash in clashes {
                let id_str = clash.get("id").and_then(Value::as_str).unwrap_or("?");
                let msg = clash.get("message").and_then(Value::as_str).unwrap_or("");
                let kind = clash.get("kind").and_then(Value::as_str).unwrap_or("?");
                let status = clash.get("status").and_then(Value::as_str).unwrap_or("new");

                let selected = st.selected_clash == Some(global_idx);
                let status_icon = match status {
                    "resolved" => " [resolved]",
                    "approved" => " [approved]",
                    "active" => " [active]",
                    _ => "",
                };
                let label = format!("{id_str} {kind}{status_icon}: {msg}");
                let resp = ui.selectable_label(selected, &label);
                if resp.clicked() {
                    st.selected_clash = Some(global_idx);
                    // Navigate to the clash location.
                    if let Some(at) = clash.get("at").and_then(Value::as_array) {
                        let x = at.first().and_then(Value::as_f64).unwrap_or(0.0);
                        let y = at.get(1).and_then(Value::as_f64).unwrap_or(0.0);
                        let _ = app.run("zoom", json!({ "mode": "center", "center": [x, y], "height": 200.0 }));
                    }
                    // Select the clashing elements.
                    if let Some(handles) = clash.get("handles").and_then(Value::as_array) {
                        let hs: Vec<cadaddy_engine::doc::Handle> =
                            handles.iter().filter_map(|h| h.as_str().and_then(cadaddy_engine::doc::Handle::parse_hex)).collect();
                        app.session.set_selection(hs);
                    }
                }
                global_idx += 1;
            }
        }
    });
}

// ─── Views tab (coordination views: camera + filters + section box + scheme) ─

fn views_tab(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State) {
    // Load views.
    if st.views_data.is_none() {
        st.views_data = app.session.execute("coordination.views", &json!({})).ok();
    }

    ui.horizontal(|ui| {
        ui.label(crate::i18n::t("Name:"));
        ui.text_edit_singleline(&mut st.view_name);
        if ui.button(crate::i18n::t("Save")).clicked() && !st.view_name.is_empty() {
            st.views_data = app.session.execute("coordination.views", &json!({ "save": { "name": st.view_name } })).ok();
        }
    });
    ui.add_space(4.0);
    ui.label(RichText::new(crate::i18n::t("Saves camera, section box, trade visibility and colour scheme.")).color(Tokens::get().text_dim));
    ui.separator();

    let views = st.views_data.as_ref().and_then(|d| d.get("views")).and_then(Value::as_array).cloned().unwrap_or_default();

    if views.is_empty() {
        ui.label(crate::i18n::t("No saved coordination views."));
        return;
    }

    egui::ScrollArea::vertical().id_salt("bim-views").auto_shrink([false, false]).show(ui, |ui| {
        for view in &views {
            let name = view.get("name").and_then(Value::as_str).unwrap_or("?");
            let vstyle = view.get("visual_style").and_then(Value::as_str).unwrap_or("");
            let has_sbox = view.get("sectionbox").and_then(|s| s.get("on")).and_then(Value::as_bool).unwrap_or(false);
            let detail = format!("{vstyle}{}", if has_sbox { " [section box]" } else { "" });

            ui.horizontal(|ui| {
                if ui.button(name).clicked() {
                    match app.session.execute("coordination.views", &json!({ "restore": name })) {
                        Ok(r) => {
                            let warnings: Vec<&str> =
                                r.get("warnings").and_then(Value::as_array).map(|w| w.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                            if !warnings.is_empty() {
                                app.set_status(warnings.join("; "));
                            }
                        }
                        Err(e) => app.set_status(e.to_string()),
                    }
                    // The list again (restoring answers with what it did, not the views).
                    st.views_data = None;
                    st.coord_mode = None;
                    st.sectionbox = None;
                    st.trades_data = None;
                }
                let t = Tokens::get();
                ui.label(RichText::new(detail).color(t.text_dim));
                if ui.small_button("X").clicked() {
                    st.views_data = app.session.execute("coordination.views", &json!({ "delete": name })).ok();
                }
            });
        }
    });
}

// ─── Section Box tab ──────────────────────────────────────────────────────────

fn sectionbox_tab(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State) {
    if st.sectionbox.is_none() {
        st.sectionbox = app.session.execute("coordination.sectionbox", &json!({})).ok();
    }

    let sbox = st.sectionbox.clone().unwrap_or(json!({"on": false}));
    let on = sbox.get("on").and_then(Value::as_bool).unwrap_or(false);

    ui.horizontal(|ui| {
        let label = if on { "Section Box: ON" } else { "Section Box: OFF" };
        if ui.button(crate::i18n::t(label)).clicked() {
            st.sectionbox = app.session.execute("coordination.sectionbox", &json!({ "on": !on })).ok();
        }
        if ui.button(crate::i18n::t("Fit to Model")).clicked() {
            st.sectionbox = app.session.execute("coordination.sectionbox", &json!({ "fit": true })).ok();
        }
    });

    if on {
        ui.add_space(4.0);
        ui.label(RichText::new(crate::i18n::t("The section box clips the 3D view to its bounds.")).color(Tokens::get().text_dim));

        if let Some(lo) = sbox.get("min").and_then(Value::as_array) {
            let vals: Vec<f64> = lo.iter().filter_map(Value::as_f64).collect();
            if vals.len() == 3 {
                ui.label(format!("Min: ({:.1}, {:.1}, {:.1})", vals[0], vals[1], vals[2]));
            }
        }
        if let Some(hi) = sbox.get("max").and_then(Value::as_array) {
            let vals: Vec<f64> = hi.iter().filter_map(Value::as_f64).collect();
            if vals.len() == 3 {
                ui.label(format!("Max: ({:.1}, {:.1}, {:.1})", vals[0], vals[1], vals[2]));
            }
        }
    }
}

// ─── Measure tab ──────────────────────────────────────────────────────────────

fn measure_tab(_app: &mut CadApp, ui: &mut egui::Ui, st: &mut State) {
    ui.label(crate::i18n::t("3D point-to-point measurement"));
    ui.separator();

    if let Some(r) = &st.measure_result {
        let dist = r.get("distance").and_then(Value::as_f64).unwrap_or(0.0);
        let dx = r.get("dx").and_then(Value::as_f64).unwrap_or(0.0);
        let dy = r.get("dy").and_then(Value::as_f64).unwrap_or(0.0);
        let dz = r.get("dz").and_then(Value::as_f64).unwrap_or(0.0);

        let t = Tokens::get();
        ui.label(RichText::new(format!("Distance: {dist:.3}")).color(t.accent).strong());
        ui.label(format!("dX: {dx:.3}  dY: {dy:.3}  dZ: {dz:.3}"));
    } else {
        ui.label(crate::i18n::t("Use DIST or the control channel to measure between two 3D points."));
    }
}
