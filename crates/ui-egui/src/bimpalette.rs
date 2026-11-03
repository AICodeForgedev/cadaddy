//! The BIM Coordination palette: a dockable panel for reviewing MEP and structural
//! coordination in 3D. Tabs: Model Tree (Level ▸ Trade ▸ Category ▸ Type ▸ element, with the
//! selected element's properties), Colours (the trade colour table), Systems, Clashes, Views,
//! Section Box, Measure.
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
    Colours,
    Trades,
    Clashes,
    Views,
    SectionBox,
    Measure,
}

#[derive(Clone, Default)]
struct State {
    tab: Tab,
    /// The drawing the cached results are of (title and path): another tab's are read afresh.
    doc_key: String,
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
    /// Coordination mode state.
    coord_mode: Option<Value>,
    /// Level filter for trades.
    level_filter: String,
    /// The BIM view's settings (`bim.look`).
    look: Option<Value>,
    /// The trade colour table (`bim.trades`).
    colours: Option<Value>,
    /// The selected element and its properties (`bim.element`).
    element: Option<(cadaddy_doc::Handle, Option<Value>)>,
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
            (Tab::Colours, "Colours"),
            (Tab::Trades, "Systems"),
            (Tab::Clashes, "Clashes"),
            (Tab::Views, "Views"),
            (Tab::SectionBox, "Section"),
            (Tab::Measure, "Measure"),
        ];
        for (tab, label) in tabs {
            let selected = st.tab == tab;
            // Selected: the selection's own text colour (accent text on the accent fill vanished).
            let text = if selected { RichText::new(crate::i18n::t(label)).strong() } else { RichText::new(crate::i18n::t(label)).color(t.text_dim) };
            if ui.selectable_label(selected, text).clicked() {
                st.tab = tab;
            }
        }
    });
    ui.separator();

    match st.tab {
        Tab::Tree => tree_tab(app, ui, &mut st),
        Tab::Colours => colours_tab(app, ui, &mut st),
        Tab::Trades => trades_tab(app, ui, &mut st),
        Tab::Clashes => clashes_tab(app, ui, &mut st),
        Tab::Views => views_tab(app, ui, &mut st),
        Tab::SectionBox => sectionbox_tab(app, ui, &mut st),
        Tab::Measure => measure_tab(app, ui, &mut st),
    }

    set_state(ui.ctx(), st);
}

// ─── Model Tree tab ───────────────────────────────────────────────────────────

/// The BIM view's switches: on, the shell ghosted, the sheet's own lines in 3D.
fn look_row(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State) {
    if st.look.is_none() {
        st.look = app.session.execute("bim.look", &json!({})).ok();
    }
    let get = |k: &str| st.look.as_ref().and_then(|v| v.get(k)).and_then(Value::as_bool).unwrap_or(false);
    let (mut on, mut ghost, mut sheet) = (get("on"), get("ghost"), get("sheet"));
    let mut set: Option<Value> = None;
    ui.horizontal_wrapped(|ui| {
        if ui.checkbox(&mut on, crate::i18n::t("BIM View")).changed() {
            set = Some(json!({ "on": on }));
        }
        if ui.add_enabled(on, egui::Checkbox::new(&mut ghost, crate::i18n::t("Ghost Architecture"))).changed() {
            set = Some(json!({ "ghost": ghost }));
        }
        if ui.add_enabled(on, egui::Checkbox::new(&mut sheet, crate::i18n::t("Sheet Lines in 3D"))).changed() {
            set = Some(json!({ "sheet": sheet }));
        }
    });
    if let Some(p) = set {
        match app.session.execute("bim.look", &p) {
            Ok(v) => st.look = Some(v),
            Err(e) => app.set_status(e.to_string()),
        }
    }
}

/// A branch's header: a checkbox (shown in the view) and its name (click: select it and zoom).
fn branch_header(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State, path: &[&str], node: &Value, strong: bool) {
    let name = node.get("name").and_then(Value::as_str).unwrap_or("?");
    let count = node.get("count").and_then(Value::as_u64).unwrap_or(0);
    let mut vis = node.get("visible").and_then(Value::as_bool).unwrap_or(true);
    if ui.checkbox(&mut vis, "").changed() {
        st.tree_data = app.session.execute("bim.tree", &json!({ "path": path, "visible": vis })).ok();
    }
    let text = RichText::new(format!("{name}  ({count})"));
    let text = if strong { text.strong() } else { text };
    if ui.selectable_label(false, text).clicked()
        && let Err(e) = app.session.execute("bim.tree", &json!({ "path": path, "select": true, "zoom": true }))
    {
        app.set_status(e.to_string());
    }
}

/// The children of a tree node under `key`.
fn kids<'a>(n: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    n.get(key).and_then(Value::as_array).into_iter().flatten()
}

fn name_of(n: &Value) -> String {
    n.get("name").and_then(Value::as_str).unwrap_or("?").to_string()
}

/// One branch of the tree and what is under it (Level ▸ Trade ▸ Category ▸ Type ▸ element).
fn branch(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State, path: &[String], node: &Value, filter: &str) {
    const LEVELS: [&str; 4] = ["trades", "categories", "types", "elements"];
    let depth = path.len();
    let id = ui.make_persistent_id(("bim-tree", path));
    let refs: Vec<&str> = path.iter().map(String::as_str).collect();
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
        .show_header(ui, |ui| branch_header(app, ui, st, &refs, node, depth == 1))
        .body(|ui| {
            let key = LEVELS.get(depth - 1).copied().unwrap_or("elements");
            for child in kids(node, key) {
                if !filter.is_empty() && !child.to_string().to_ascii_lowercase().contains(filter) {
                    continue;
                }
                if key == "elements" {
                    let h = child.get("handle").and_then(Value::as_str).unwrap_or("");
                    let name = child.get("name").and_then(Value::as_str).unwrap_or(h);
                    if ui.selectable_label(false, name).clicked() {
                        let _ = app.session.execute("zoom", &json!({ "mode": "object", "handles": [h] }));
                        if let Some(hh) = cadaddy_doc::Handle::parse_hex(h) {
                            app.session.set_selection(vec![hh]);
                        }
                    }
                } else {
                    let mut p = path.to_vec();
                    p.push(name_of(child));
                    branch(app, ui, st, &p, child, filter);
                }
            }
        });
}

fn tree_tab(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State) {
    let key = app.session.state().map(|d| format!("{}|{:?}", d.title, d.path)).unwrap_or_default();
    if st.doc_key != key {
        st.doc_key = key;
        st.tree_data = None;
        st.look = None;
    }
    look_row(app, ui, st);
    ui.horizontal(|ui| {
        if ui.button(crate::i18n::t("Refresh")).clicked() {
            st.tree_data = app.session.execute("bim.tree", &json!({})).ok();
            st.look = None;
        }
        ui.text_edit_singleline(&mut st.search);
    });
    if st.tree_data.is_none() {
        st.tree_data = app.session.execute("bim.tree", &json!({})).ok();
    }
    let data = st.tree_data.clone().unwrap_or(Value::Null);
    let levels: Vec<Value> = kids(&data, "levels").cloned().collect();
    if levels.is_empty() {
        ui.label(crate::i18n::t("Nothing is built yet. Use Plans to Model first."));
        return;
    }
    trades_row(app, ui, st, &levels);
    let filter = st.search.to_ascii_lowercase();
    let height = (ui.available_height() * 0.6).max(120.0);
    egui::ScrollArea::vertical().id_salt("bim-tree").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
        for level in &levels {
            if !filter.is_empty() && !level.to_string().to_ascii_lowercase().contains(&filter) {
                continue;
            }
            branch(app, ui, st, &[name_of(level)], level, &filter);
        }
    });
    ui.separator();
    properties(app, ui, st);
}

/// One switch per trade for the whole building (every level's branch of it at once): ticked when
/// it shows on every level that has it.
fn trades_row(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State, levels: &[Value]) {
    let mut trades: Vec<(String, bool, Vec<String>)> = Vec::new();
    for level in levels {
        for trade in kids(level, "trades") {
            let (name, vis) = (name_of(trade), trade.get("visible").and_then(Value::as_bool).unwrap_or(true));
            match trades.iter_mut().find(|(n, _, _)| *n == name) {
                Some(t) => {
                    t.1 &= vis;
                    t.2.push(name_of(level));
                }
                None => trades.push((name, vis, vec![name_of(level)])),
            }
        }
    }
    if trades.is_empty() {
        return;
    }
    let mut changed: Option<(String, bool, Vec<String>)> = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(crate::i18n::t("Whole building:")).color(Tokens::get().text_dim));
        for (name, vis, on) in &trades {
            let mut v = *vis;
            if ui.checkbox(&mut v, crate::i18n::t(name)).changed() {
                changed = Some((name.clone(), v, on.clone()));
            }
        }
    });
    if let Some((trade, vis, on)) = changed {
        for level in on {
            if let Err(e) = app.session.execute("bim.tree", &json!({ "path": [level, trade], "visible": vis })) {
                app.set_status(e.to_string());
            }
        }
        st.tree_data = app.session.execute("bim.tree", &json!({})).ok();
    }
}

/// The one selected element's properties (type, size, level, system, circuit, quantity,
/// source, issues) and a button to its spot on the sheet.
fn properties(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State) {
    let sel = app.session.selection();
    let [h] = sel.as_slice() else {
        ui.label(RichText::new(crate::i18n::t("Select one element to see its properties.")).color(Tokens::get().text_dim));
        return;
    };
    if st.element.as_ref().is_none_or(|(k, _)| k != h) {
        st.element = Some((*h, app.session.execute("bim.element", &json!({ "handle": h.hex() })).ok()));
    }
    let Some((_, Some(el))) = st.element.clone() else {
        ui.label(RichText::new(crate::i18n::t("Not part of the built model.")).color(Tokens::get().text_dim));
        return;
    };
    let text = |k: &str| el.get(k).and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string);
    let q = el.get("quantity").cloned().unwrap_or(Value::Null);
    let num = |k: &str| q.get(k).and_then(Value::as_f64);
    let src = el.get("source").cloned().unwrap_or(Value::Null);
    let issues = el.get("issues").and_then(Value::as_array).map_or(0, Vec::len);
    // A piece of a run (Q6157): what it is bought as, its run and what it joins; a device: its
    // heights, its wall, its box and what feeds it.
    let piece = el.get("piece").cloned().unwrap_or(Value::Null);
    let items = |v: Option<&Value>| -> Option<String> {
        let list: Vec<String> = v?
            .as_array()?
            .iter()
            .take(8)
            .filter_map(|n| n.get("item").and_then(Value::as_str).filter(|t| !t.is_empty()).or_else(|| n.get("kind").and_then(Value::as_str)))
            .map(str::to_string)
            .collect();
        (!list.is_empty()).then(|| list.join(", "))
    };
    let aff = match (
        el.get("aff_in").and_then(Value::as_f64),
        el.get("center_aff_in").and_then(Value::as_f64),
        el.get("top_aff_in").and_then(Value::as_f64),
    ) {
        (Some(b), Some(c), Some(t)) => Some(format!("{b:.1} / {c:.1} / {t:.1} in")),
        _ => None,
    };
    let face = el.get("wall_face").filter(|v| !v.is_null());
    let wall = face.and_then(|f| f.get("wall")).or_else(|| el.get("host").and_then(|h| h.get("wall"))).and_then(Value::as_str).map(str::to_string);
    let face_text = face.and_then(|f| Some(format!("{:+.2} in {}", f.get("inches")?.as_f64()?, f.get("as")?.as_str()?)));
    let boxed = el.get("box").filter(|b| !b.is_null()).and_then(|b| {
        let item = b.get("item").and_then(Value::as_str)?;
        Some(format!("{item} {}", b.get("handle").and_then(Value::as_str).unwrap_or_default()).trim().to_string())
    });
    let rows: Vec<(&str, Option<String>)> = vec![
        ("Piece", piece.get("item").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string)),
        ("Type", text("type")),
        ("Category", text("category")),
        ("Trade", text("trade_label")),
        ("Size", text("size")),
        ("Level", text("level")),
        ("System", text("system")),
        ("Circuit", el.get("circuit").filter(|v| !v.is_null()).map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))),
        ("Length", num("length_ft").map(|v| format!("{v:.2} ft"))),
        ("Area", num("area_sf").map(|v| format!("{v:.2} sf"))),
        ("Height", num("height_ft").map(|v| format!("{v:.2} ft"))),
        ("Count", q.get("count").and_then(Value::as_u64).map(|v| v.to_string())),
        ("Sheet", src.get("sheet").and_then(Value::as_str).or(src.get("sheet_file").and_then(Value::as_str)).map(str::to_string)),
        ("Source", src.get("kind").and_then(Value::as_str).map(|k| format!("{k} {}", src.get("handle").and_then(Value::as_str).unwrap_or("")))),
        ("Issues", (issues > 0).then(|| issues.to_string())),
        ("Raceway run", piece.get("run").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string)),
        ("Connects to", items(piece.get("connects"))),
        ("Height AFF", aff),
        ("Host wall", wall),
        ("Wall face", face_text),
        ("Outlet box", boxed),
        ("Fed by", items(el.get("fed_by"))),
    ];
    egui::Grid::new("bim-element").num_columns(2).striped(true).show(ui, |ui| {
        for (label, v) in rows {
            if let Some(v) = v {
                ui.label(RichText::new(crate::i18n::t(label)).color(Tokens::get().text_dim));
                ui.label(v);
                ui.end_row();
            }
        }
    });
    if !src.is_null() && ui.button(crate::i18n::t("Show on Sheet")).clicked() {
        if let Err(e) = app.session.execute("bim.element", &json!({ "handle": h.hex(), "open_source": true })) {
            app.set_status(e.to_string());
        }
        st.element = None;
    }
}

// ─── Colours tab ──────────────────────────────────────────────────────────────

/// The trade colour table: each trade's colour (editable) and whether it shows.
fn colours_tab(app: &mut CadApp, ui: &mut egui::Ui, st: &mut State) {
    if st.colours.is_none() {
        st.colours = app.session.execute("bim.trades", &json!({})).ok();
    }
    if ui.button(crate::i18n::t("Reset Colours")).clicked() {
        st.colours = app.session.execute("bim.trades", &json!({ "reset": true })).ok();
    }
    let rows: Vec<Value> = st.colours.as_ref().map(|v| kids(v, "trades").cloned().collect()).unwrap_or_default();
    let mut set: Option<Value> = None;
    egui::ScrollArea::vertical().id_salt("bim-colours").auto_shrink([false, false]).show(ui, |ui| {
        let mut group = String::new();
        for r in &rows {
            let g = r.get("group").and_then(Value::as_str).unwrap_or("");
            if g != group {
                group = g.to_string();
                ui.label(RichText::new(crate::i18n::t(g)).strong());
            }
            let key = r.get("trade").and_then(Value::as_str).unwrap_or("");
            let label = r.get("label").and_then(Value::as_str).unwrap_or(key);
            let count = r.get("count").and_then(Value::as_u64).unwrap_or(0);
            let c: Vec<u8> = kids(r, "color").filter_map(Value::as_u64).map(|v| v.min(255) as u8).collect();
            let mut rgb = [c.first().copied().unwrap_or(200), c.get(1).copied().unwrap_or(200), c.get(2).copied().unwrap_or(200)];
            let mut vis = r.get("visible").and_then(Value::as_bool).unwrap_or(true);
            ui.horizontal(|ui| {
                if ui.checkbox(&mut vis, "").changed() {
                    set = Some(json!({ "visible": { key: vis } }));
                }
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    set = Some(json!({ "colors": { key: rgb } }));
                }
                ui.label(format!("{}  ({count})", crate::i18n::t(label)));
            });
        }
    });
    if let Some(p) = set {
        match app.session.execute("bim.trades", &p) {
            Ok(v) => st.colours = Some(v),
            Err(e) => app.set_status(e.to_string()),
        }
    }
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
