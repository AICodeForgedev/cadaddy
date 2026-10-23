//! The Clashes palette (CLASHDETECT): runs clash detection on the 3D model's MEP runs and
//! structural elements, lists every clash grouped by location (one beam crossing many pipes =
//! one group), lets the user walk to each clash in 3D, set its status (new / active / resolved),
//! assign a trade, add notes, and export to BCF 2.1 or CSV. Re-running keeps statuses.

use egui::RichText;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::CadApp;

pub const DIALOG: &str = "clashpalette";

/// A single clash item from the engine's `model.clashes`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClashItem {
    pub id: String,
    pub kind: String,
    pub message: String,
    pub systems: Vec<String>,
    pub at: [f64; 2],
    pub by_in: f64,
    pub handles: Vec<String>,
    /// The level the clash is on (part of the key a re-run matches it by).
    #[serde(default)]
    pub level: String,
    pub status: ClashStatus,
    pub trade: String,
    pub notes: String,
    /// For grouping: the structural element handle that many runs clash into.
    pub group_key: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClashStatus {
    #[default]
    New,
    Active,
    Resolved,
}

impl ClashStatus {
    fn label(self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Active => "Active",
            Self::Resolved => "Resolved",
        }
    }
    fn all() -> &'static [Self] {
        &[Self::New, Self::Active, Self::Resolved]
    }
}

/// A group of clashes that share a structural element or close location.
#[derive(Clone, Debug)]
struct ClashGroup {
    label: String,
    clash_ids: Vec<usize>,
}

/// Persistent state for the clash palette, stored in egui's temp data per-drawing.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct State {
    clashes: Vec<ClashItem>,
    /// Which clash index is selected.
    selected: Option<usize>,
    /// Status filter: show only these statuses (empty = show all).
    show_statuses: Vec<ClashStatus>,
    /// Message from the last action.
    status: String,
    /// Tolerance for clearance violations (inches).
    tolerance_in: f64,
    /// Configurable clearance per trade pair (trade_a, trade_b) -> inches.
    clearances: BTreeMap<(String, String), f64>,
    /// The serial number of the last run (bumped on re-run to detect staleness).
    run_serial: u64,
}

fn id() -> egui::Id {
    egui::Id::new("clash_palette_state")
}

/// Run clash detection via the engine's `model.clashes` command.
fn detect(app: &mut CadApp, tolerance_in: f64) -> Vec<ClashItem> {
    let result = app.session.execute("model.clashes", &json!({ "tolerance_in": tolerance_in }));
    let data = match result {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    items_from(data.get("clashes").and_then(Value::as_array).cloned().unwrap_or_default(), 0)
}

/// The engine's clashes as palette items; a clash without an id (a new one) is numbered after
/// `last_id`. Status, trade and notes the engine carried over from a previous run are kept.
fn items_from(raw: Vec<Value>, last_id: usize) -> Vec<ClashItem> {
    let mut next_id = last_id;
    let mut items = Vec::new();
    for c in raw {
        let kind = c.get("kind").and_then(Value::as_str).unwrap_or("unknown").to_string();
        let message = c.get("message").and_then(Value::as_str).unwrap_or("").to_string();
        let systems: Vec<String> =
            c.get("systems").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        let at_arr = c.get("at").and_then(Value::as_array).unwrap_or(&Vec::new()).clone();
        let at = [at_arr.first().and_then(Value::as_f64).unwrap_or(0.0), at_arr.get(1).and_then(Value::as_f64).unwrap_or(0.0)];
        let by_in = c.get("by_in").and_then(Value::as_f64).unwrap_or(0.0);
        let handles: Vec<String> =
            c.get("handles").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        // Group key: for column/structure clashes, use the last handle (the structural element).
        let group_key = if kind == "column" || kind == "into_structure" { handles.last().cloned().unwrap_or_default() } else { String::new() };
        let level =
            c.get("level").and_then(Value::as_str).or_else(|| c.pointer("/reference/level").and_then(Value::as_str)).unwrap_or("").to_string();
        let id = match c.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            Some(id) => id.to_string(),
            None => {
                next_id += 1;
                format!("C{next_id:04}")
            }
        };
        let status = match c.get("status").and_then(Value::as_str) {
            Some("Active") => ClashStatus::Active,
            Some("Resolved") => ClashStatus::Resolved,
            _ => ClashStatus::New,
        };
        let text = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        items.push(ClashItem {
            id,
            kind,
            message,
            systems,
            at,
            by_in,
            handles,
            level,
            status,
            trade: text("trade"),
            notes: text("notes"),
            group_key,
        });
    }
    items
}

/// Group clashes that share the same structural element or are within 2 feet of each other.
fn group_clashes(clashes: &[ClashItem]) -> Vec<ClashGroup> {
    let mut groups: Vec<ClashGroup> = Vec::new();
    let mut assigned: Vec<bool> = vec![false; clashes.len()];

    // First pass: group by structural element (column/structure clashes with same group_key).
    let mut key_groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, c) in clashes.iter().enumerate() {
        if !c.group_key.is_empty() {
            key_groups.entry(c.group_key.clone()).or_default().push(i);
        }
    }
    for (key, indices) in &key_groups {
        if indices.len() > 1 {
            let first = indices.first().and_then(|&i| clashes.get(i));
            let label = if let Some(c) = first {
                format!("{} ({} clashes at {})", c.kind, indices.len(), key.get(..6).unwrap_or(key))
            } else {
                format!("Group {key}",)
            };
            groups.push(ClashGroup { label, clash_ids: indices.clone() });
            for &i in indices {
                if let Some(a) = assigned.get_mut(i) {
                    *a = true;
                }
            }
        }
    }

    // Second pass: proximity-group remaining clashes (within 2 ft).
    let proximity = 2.0;
    for i in 0..clashes.len() {
        if assigned.get(i).copied().unwrap_or(true) {
            continue;
        }
        let Some(ci) = clashes.get(i) else { continue };
        let mut nearby = vec![i];
        for j in i + 1..clashes.len() {
            if assigned.get(j).copied().unwrap_or(true) {
                continue;
            }
            if let Some(cj) = clashes.get(j) {
                let dx = ci.at[0] - cj.at[0];
                let dy = ci.at[1] - cj.at[1];
                if (dx * dx + dy * dy).sqrt() < proximity {
                    nearby.push(j);
                }
            }
        }
        let label = if nearby.len() > 1 {
            format!("{} ({} clashes near [{:.1}, {:.1}])", ci.kind, nearby.len(), ci.at[0], ci.at[1])
        } else {
            format!("{}: {}", ci.id, ci.message)
        };
        groups.push(ClashGroup { label, clash_ids: nearby.clone() });
        for &idx in &nearby {
            if let Some(a) = assigned.get_mut(idx) {
                *a = true;
            }
        }
    }

    groups
}

/// The number in a palette id (`C0042` → 42).
fn id_number(id: &str) -> usize {
    id.trim_start_matches('C').parse().unwrap_or(0)
}

/// Re-run detection keeping statuses, trades and notes: the engine matches each clash to the
/// previous run's by kind, level, systems and plan position (not by handles, which rebuilding the
/// model changes), and a matched clash keeps its id. A clash someone had triaged that is no
/// longer found stays in the list as resolved; untouched ones that went away are dropped.
fn rerun(app: &mut CadApp, state: &mut State) {
    let previous: Vec<Value> = state
        .clashes
        .iter()
        .map(|c| {
            json!({
                "id": c.id, "kind": c.kind, "level": c.level, "systems": c.systems, "at": c.at,
                "status": c.status.label(), "trade": c.trade, "notes": c.notes,
            })
        })
        .collect();
    let triaged: BTreeMap<String, ClashItem> = state
        .clashes
        .iter()
        .filter(|c| c.status != ClashStatus::New || !c.trade.is_empty() || !c.notes.is_empty())
        .map(|c| (c.id.clone(), c.clone()))
        .collect();
    let last_id = state.clashes.iter().map(|c| id_number(&c.id)).max().unwrap_or(0);
    let Ok(data) = app.session.execute("model.clashes", &json!({ "tolerance_in": state.tolerance_in, "previous": previous })) else {
        state.status = crate::tl!("Clash detection failed.").to_string();
        return;
    };
    let mut new_clashes = items_from(data.get("clashes").and_then(Value::as_array).cloned().unwrap_or_default(), last_id);
    let gone = data.get("gone").and_then(Value::as_array).into_iter().flatten().filter_map(|g| g.get("id").and_then(Value::as_str));
    for id in gone {
        if let Some(mut c) = triaged.get(id).cloned() {
            c.status = ClashStatus::Resolved;
            new_clashes.push(c);
        }
    }
    state.clashes = new_clashes;
    state.run_serial = state.run_serial.wrapping_add(1);
    state.selected = None;
    state.status = crate::tf!("{n} clashes found.", n = state.clashes.len());
}

/// Select the clash's handles and zoom to them.
fn goto_clash(app: &mut CadApp, clash: &ClashItem, covered: Option<egui::Rect>) {
    if clash.handles.is_empty() {
        return;
    }
    let hs: Vec<cadaddy_engine::doc::Handle> = clash.handles.iter().filter_map(|h| cadaddy_engine::doc::Handle::parse_hex(h)).collect();
    let _ = app.run("zoom.object", json!({ "handles": clash.handles }));
    // Clear of the palette, as the count palette does.
    if let Some(canvas) = app.canvas.rect.filter(|r| r.width() > 1.0 && r.height() > 1.0)
        && let Some(free) = free_area(canvas, covered)
        && let Ok(st) = app.session.state()
        && st.zoom_viewport().is_none()
    {
        let v = st.zoom_frame();
        let k = f64::from((canvas.width() / free.width()).max(canvas.height() / free.height()));
        let height = v.height * k;
        let per = height / f64::from(canvas.height());
        let d = free.center() - canvas.center();
        let center = [v.center.x - f64::from(d.x) * per, v.center.y + f64::from(d.y) * per];
        let _ = app.run("zoom", json!({ "mode": "center", "center": center, "height": height }));
    }
    app.session.set_selection(hs);
}

/// The free area of the canvas not covered by the palette (same as countpalette).
const MARGIN: [f32; 4] = [40.0, 40.0, 110.0, 80.0];

fn free_area(canvas: egui::Rect, covered: Option<egui::Rect>) -> Option<egui::Rect> {
    let mut f = canvas;
    if let Some(c) = covered.filter(|c| c.intersects(canvas)) {
        if canvas.right() - c.right() >= c.left() - canvas.left() {
            f.min.x = c.right().max(canvas.left());
        } else {
            f.max.x = c.left().min(canvas.right());
        }
    }
    let [l, t, r, b] = MARGIN;
    let f = egui::Rect::from_min_max(f.min + egui::vec2(l, t), f.max - egui::vec2(r, b));
    (f.width() >= 60.0 && f.height() >= 60.0).then_some(f)
}

/// Export clashes to BCF 2.1 XML (a ZIP containing bcf.version, markup/*.bcf, viewpoint/*.bcfv).
pub fn export_bcf(clashes: &[ClashItem]) -> Vec<u8> {
    bcf::write_bcf(clashes)
}

/// Export clashes to CSV.
pub fn export_csv(clashes: &[ClashItem]) -> String {
    let mut out = String::from("ID,Kind,Status,Trade,Systems,Location X,Location Y,Depth (in),Message,Notes\n");
    for c in clashes {
        let systems = c.systems.join("; ");
        let notes = c.notes.replace('"', "\"\"");
        let message = c.message.replace('"', "\"\"");
        out.push_str(&format!(
            "{},{},{},{},\"{}\",{:.3},{:.3},{:.2},\"{}\",\"{}\"\n",
            c.id,
            c.kind,
            c.status.label(),
            c.trade,
            systems,
            c.at[0],
            c.at[1],
            c.by_in,
            message,
            notes,
        ));
    }
    out
}

/// The clashes as the engine's report command takes them: each with its palette group's label.
fn report_clashes(clashes: &[ClashItem]) -> Vec<Value> {
    let mut group_of: Vec<String> = vec![String::new(); clashes.len()];
    for g in group_clashes(clashes) {
        if g.clash_ids.len() > 1 {
            for &i in &g.clash_ids {
                if let Some(slot) = group_of.get_mut(i) {
                    slot.clone_from(&g.label);
                }
            }
        }
    }
    clashes
        .iter()
        .zip(group_of)
        .map(|(c, group)| {
            json!({
                "id": c.id, "kind": c.kind, "level": c.level, "systems": c.systems, "message": c.message,
                "status": c.status.label(), "trade": c.trade, "notes": c.notes, "group": group,
            })
        })
        .collect()
}

/// Export the grouped clash report to PDF through `model.clashes.report`.
fn export_pdf(app: &mut CadApp, clashes: &[ClashItem], path: &str) -> Result<(), String> {
    app.session.execute("model.clashes.report", &json!({ "path": path, "clashes": report_clashes(clashes) })).map(|_| ()).map_err(|e| e.to_string())
}

/// The directory beside the current drawing (or the current directory if unsaved).
fn doc_dir(app: &CadApp) -> std::path::PathBuf {
    app.session
        .docs
        .get(app.session.active)
        .and_then(|d| d.path.as_deref())
        .and_then(|p| std::path::Path::new(p).parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")))
}

/// Kind label with an icon for the palette's list.
fn kind_label(kind: &str) -> &str {
    match kind {
        "below_ceiling" => "Below Ceiling",
        "into_structure" => "Into Structure",
        "crossing" => "Crossing",
        "column" => "Through Column",
        _ => kind,
    }
}

fn status_color(status: ClashStatus, t: &crate::theme::Tokens) -> egui::Color32 {
    match status {
        ClashStatus::New => t.warn,
        ClashStatus::Active => egui::Color32::from_rgb(70, 130, 220),
        ClashStatus::Resolved => egui::Color32::from_rgb(80, 180, 80),
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
    let mut action: Option<Action> = None;

    let window = egui::Window::new(crate::tl!("Clashes"))
        .id(egui::Id::new("clash_palette"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([380.0, 500.0])
        .show(ctx, |ui| {
            // Toolbar.
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Run")).on_hover_text(crate::tl!("Run clash detection")).clicked() {
                    action = Some(Action::Run);
                }
                if ui.button(crate::tl!("Re-run")).on_hover_text(crate::tl!("Re-run keeping statuses")).clicked() {
                    action = Some(Action::Rerun);
                }
                ui.separator();
                ui.label(crate::tl!("Tolerance:"));
                let mut tol_str = if st.tolerance_in == 0.0 { String::new() } else { format!("{:.1}", st.tolerance_in) };
                let resp = ui.add(egui::TextEdit::singleline(&mut tol_str).desired_width(40.0).hint_text("0"));
                if resp.changed() {
                    st.tolerance_in = tol_str.parse::<f64>().unwrap_or(0.0).max(0.0);
                }
                ui.label(crate::tl!("in"));
            });

            // Status filter.
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::tl!("Show:")).small());
                for &status in ClashStatus::all() {
                    let active = st.show_statuses.is_empty() || st.show_statuses.contains(&status);
                    let color = if active { status_color(status, &t) } else { t.text_dim };
                    if ui.add(egui::Button::new(RichText::new(status.label()).color(color)).small()).clicked() {
                        if st.show_statuses.is_empty() {
                            // First click: show only this status.
                            st.show_statuses = vec![status];
                        } else if st.show_statuses.contains(&status) && st.show_statuses.len() == 1 {
                            // Only this one was shown, clear filter (show all).
                            st.show_statuses.clear();
                        } else if st.show_statuses.contains(&status) {
                            st.show_statuses.retain(|s| *s != status);
                        } else {
                            st.show_statuses.push(status);
                        }
                    }
                }
            });

            ui.separator();

            // Summary.
            let total = st.clashes.len();
            let new_count = st.clashes.iter().filter(|c| c.status == ClashStatus::New).count();
            let active_count = st.clashes.iter().filter(|c| c.status == ClashStatus::Active).count();
            let resolved_count = st.clashes.iter().filter(|c| c.status == ClashStatus::Resolved).count();
            ui.label(
                RichText::new(crate::tf!(
                    "Total: {total} | New: {new} | Active: {active} | Resolved: {resolved}",
                    total = total,
                    new = new_count,
                    active = active_count,
                    resolved = resolved_count,
                ))
                .small()
                .color(t.text_dim),
            );

            // Grouped clash list.
            let groups = group_clashes(&st.clashes);
            let visible: Vec<usize> = if st.show_statuses.is_empty() {
                (0..st.clashes.len()).collect()
            } else {
                (0..st.clashes.len()).filter(|&i| st.clashes.get(i).is_some_and(|c| st.show_statuses.contains(&c.status))).collect()
            };

            egui::ScrollArea::vertical().id_salt("clash_list").max_height(280.0).show(ui, |ui| {
                for group in &groups {
                    let group_visible: Vec<usize> = group.clash_ids.iter().filter(|i| visible.contains(i)).copied().collect();
                    if group_visible.is_empty() {
                        continue;
                    }
                    if group.clash_ids.len() == 1
                        && let Some(&idx) = group.clash_ids.first()
                        && let Some(c) = st.clashes.get(idx)
                    {
                        // Single clash, no group header.
                        let sel = st.selected == Some(idx);
                        let color = status_color(c.status, &t);
                        let label = format!("{} [{}] {}", c.id, kind_label(&c.kind), c.message);
                        let resp = ui.selectable_label(sel, RichText::new(&label).color(color));
                        if resp.clicked() {
                            st.selected = Some(idx);
                            action = Some(Action::Goto(idx));
                        }
                    } else {
                        // Group header.
                        let header_id = egui::Id::new(&group.label);
                        egui::CollapsingHeader::new(RichText::new(&group.label).strong()).id_salt(header_id).default_open(true).show(ui, |ui| {
                            for &idx in &group_visible {
                                if let Some(c) = st.clashes.get(idx) {
                                    let sel = st.selected == Some(idx);
                                    let color = status_color(c.status, &t);
                                    let label = format!("{} {}", c.id, c.message);
                                    let resp = ui.selectable_label(sel, RichText::new(&label).color(color));
                                    if resp.clicked() {
                                        st.selected = Some(idx);
                                        action = Some(Action::Goto(idx));
                                    }
                                }
                            }
                        });
                    }
                }
                if st.clashes.is_empty() {
                    ui.label(RichText::new(crate::tl!("No clashes. Run detection first.")).color(t.text_dim));
                }
            });

            ui.separator();

            // Detail panel for selected clash.
            if let Some(idx) = st.selected
                && let Some(c) = st.clashes.get_mut(idx)
            {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(&c.id).strong());
                    ui.label(kind_label(&c.kind));
                });
                ui.label(&c.message);
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Status:"));
                    for &status in ClashStatus::all() {
                        if ui.selectable_label(c.status == status, status.label()).clicked() {
                            c.status = status;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Trade:"));
                    ui.text_edit_singleline(&mut c.trade);
                });
                ui.horizontal(|ui| {
                    ui.label(crate::tl!("Notes:"));
                });
                ui.text_edit_multiline(&mut c.notes);

                ui.horizontal(|ui| {
                    if ui.button(crate::tl!("Go To")).on_hover_text(crate::tl!("Zoom to clash")).clicked() {
                        action = Some(Action::Goto(idx));
                    }
                });
            }

            ui.separator();

            // Export buttons.
            ui.horizontal_wrapped(|ui| {
                if ui.button(crate::tl!("Export BCF")).on_hover_text(crate::tl!("Export to BCF 2.1 (ZIP)")).clicked() {
                    action = Some(Action::ExportBcf);
                }
                if ui.button(crate::tl!("Export CSV")).on_hover_text(crate::tl!("Export clash report to CSV")).clicked() {
                    action = Some(Action::ExportCsv);
                }
                if ui.button(crate::tl!("Export PDF")).on_hover_text(crate::tl!("Export the grouped clash report to PDF")).clicked() {
                    action = Some(Action::ExportPdf);
                }
            });

            if !st.status.is_empty() {
                ui.label(RichText::new(&st.status).small());
            }
        });

    let covered = window.map(|w| w.response.rect);

    match action {
        Some(Action::Run) => {
            st.clashes = detect(app, st.tolerance_in);
            st.run_serial = st.run_serial.wrapping_add(1);
            st.selected = None;
            st.status = crate::tf!("{n} clashes found.", n = st.clashes.len());
        }
        Some(Action::Rerun) => {
            rerun(app, &mut st);
        }
        Some(Action::Goto(idx)) => {
            if let Some(c) = st.clashes.get(idx) {
                goto_clash(app, c, covered);
                st.status = crate::tf!("Showing clash {id}.", id = c.id);
            }
        }
        Some(Action::ExportBcf) => {
            let data = export_bcf(&st.clashes);
            let dir = doc_dir(app);
            let path = dir.join("clashes.bcf");
            match std::fs::write(&path, &data) {
                Ok(()) => st.status = crate::tf!("BCF exported to {path}.", path = path.display()),
                Err(e) => st.status = format!("BCF export failed: {e}"),
            }
        }
        Some(Action::ExportCsv) => {
            let csv = export_csv(&st.clashes);
            let dir = doc_dir(app);
            let path = dir.join("clashes.csv");
            match std::fs::write(&path, csv.as_bytes()) {
                Ok(()) => st.status = crate::tf!("CSV exported to {path}.", path = path.display()),
                Err(e) => st.status = format!("CSV export failed: {e}"),
            }
        }
        Some(Action::ExportPdf) => {
            let path = doc_dir(app).join("clashes.pdf");
            st.status = match export_pdf(app, &st.clashes, &path.display().to_string()) {
                Ok(()) => crate::tf!("PDF exported to {path}.", path = path.display()),
                Err(e) => e,
            };
        }
        None => {}
    }

    ctx.data_mut(|d| d.insert_temp(id(), st));
}

enum Action {
    Run,
    Rerun,
    Goto(usize),
    ExportBcf,
    ExportCsv,
    ExportPdf,
}

/// BCF 2.1 export: a minimal ZIP containing the required XML files.
mod bcf {
    use super::*;

    /// Generate a deterministic GUID from a clash ID string.
    fn clash_guid(id: &str) -> String {
        // Simple hash-based GUID generation for BCF topic IDs.
        let mut h: u64 = 0x517C_C1B7_2722_0A95;
        for b in id.as_bytes() {
            h = h.wrapping_mul(0x0100_0000_01B3).wrapping_add(u64::from(*b));
        }
        let h2 = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            h >> 32,
            (h >> 16) & 0xFFFF,
            h & 0x0FFF,
            0x8000 | ((h2 >> 48) & 0x3FFF),
            h2 & 0xFFFF_FFFF_FFFF,
        )
    }

    /// Write a minimal BCF 2.1 ZIP archive.
    pub fn write_bcf(clashes: &[ClashItem]) -> Vec<u8> {
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();

        // bcf.version
        entries.push((
            "bcf.version".into(),
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
              <Version VersionId=\"2.1\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n\
              </Version>\n"
                .to_vec(),
        ));

        // project.bcfp
        entries.push((
            "project.bcfp".into(),
            b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
              <ProjectInfo xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n\
              <Project ProjectId=\"cadaddy-clashes\"><Name>Clash Detection Report</Name></Project>\n\
              </ProjectInfo>\n"
                .to_vec(),
        ));

        for clash in clashes {
            let guid = clash_guid(&clash.id);
            let dir = &guid;

            // markup.bcf
            let status_text = clash.status.label();
            let description = xml_escape(&clash.message);
            let notes = if clash.notes.is_empty() { String::new() } else { xml_escape(&clash.notes) };
            let trade = xml_escape(&clash.trade);

            let mut markup = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <Markup xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n\
                 <Topic Guid=\"{guid}\" TopicType=\"Clash\" TopicStatus=\"{status_text}\">\n\
                 <Title>{description}</Title>\n"
            );
            if !trade.is_empty() {
                markup.push_str(&format!("<AssignedTo>{trade}</AssignedTo>\n"));
            }
            if !notes.is_empty() {
                markup.push_str(&format!("<Description>{notes}</Description>\n"));
            }
            markup.push_str(&format!("<Labels><Label>{}</Label></Labels>\n", xml_escape(&clash.kind)));
            markup.push_str("</Topic>\n</Markup>\n");
            entries.push((format!("{dir}/markup.bcf"), markup.into_bytes()));

            // viewpoint (minimal, with the clash location as camera target).
            let vp_guid = clash_guid(&format!("{}-vp", clash.id));
            let viewpoint = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <VisualizationInfo Guid=\"{vp_guid}\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\n\
                 <OrthogonalCamera>\n\
                 <CameraViewPoint><X>{x}</X><Y>{y}</Y><Z>0</Z></CameraViewPoint>\n\
                 <CameraDirection><X>0</X><Y>0</Y><Z>-1</Z></CameraDirection>\n\
                 <CameraUpVector><X>0</X><Y>1</Y><Z>0</Z></CameraUpVector>\n\
                 <ViewToWorldScale>1</ViewToWorldScale>\n\
                 </OrthogonalCamera>\n\
                 </VisualizationInfo>\n",
                x = clash.at[0],
                y = clash.at[1],
            );
            entries.push((format!("{dir}/viewpoint.bcfv"), viewpoint.into_bytes()));
        }

        zip_entries(&entries)
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
    }

    /// Minimal ZIP archive (store, no compression — BCF files are small XML).
    fn zip_entries(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let name_bytes = name.as_bytes();
            let offset = buf.len() as u32;
            let crc = crc32(data);
            let size = data.len() as u32;

            // Local file header.
            buf.extend_from_slice(&0x04034B50u32.to_le_bytes()); // signature
            buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
            buf.extend_from_slice(&0u16.to_le_bytes()); // flags
            buf.extend_from_slice(&0u16.to_le_bytes()); // compression (store)
            buf.extend_from_slice(&0u16.to_le_bytes()); // mod time
            buf.extend_from_slice(&0u16.to_le_bytes()); // mod date
            buf.extend_from_slice(&crc.to_le_bytes());
            buf.extend_from_slice(&size.to_le_bytes()); // compressed
            buf.extend_from_slice(&size.to_le_bytes()); // uncompressed
            buf.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            buf.extend_from_slice(&0u16.to_le_bytes()); // extra len
            buf.extend_from_slice(name_bytes);
            buf.extend_from_slice(data);

            // Central directory entry.
            central.extend_from_slice(&0x02014B50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            central.extend_from_slice(&20u16.to_le_bytes()); // version needed
            central.extend_from_slice(&0u16.to_le_bytes()); // flags
            central.extend_from_slice(&0u16.to_le_bytes()); // compression
            central.extend_from_slice(&0u16.to_le_bytes()); // mod time
            central.extend_from_slice(&0u16.to_le_bytes()); // mod date
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra len
            central.extend_from_slice(&0u16.to_le_bytes()); // comment len
            central.extend_from_slice(&0u16.to_le_bytes()); // disk start
            central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name_bytes);
        }

        let cd_offset = buf.len() as u32;
        buf.extend_from_slice(&central);
        let cd_size = central.len() as u32;
        let count = entries.len() as u16;

        // End of central directory.
        buf.extend_from_slice(&0x06054B50u32.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // disk
        buf.extend_from_slice(&0u16.to_le_bytes()); // disk with cd
        buf.extend_from_slice(&count.to_le_bytes());
        buf.extend_from_slice(&count.to_le_bytes());
        buf.extend_from_slice(&cd_size.to_le_bytes());
        buf.extend_from_slice(&cd_offset.to_le_bytes());
        buf.extend_from_slice(&0u16.to_le_bytes()); // comment len

        buf
    }

    /// CRC-32 (ISO 3309 / ZIP).
    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_engine::Session;
    use serde_json::Value;

    #[test]
    fn csv_export_escapes_correctly() {
        let clashes = vec![ClashItem {
            id: "C0001".into(),
            kind: "crossing".into(),
            message: "Supply crosses return, 3\" overlap".into(),
            systems: vec!["M-DUCT-SUPP".into(), "M-DUCT-RETN".into()],
            at: [12.5, 34.75],
            by_in: 3.0,
            handles: vec!["1A".into()],
            level: String::new(),
            status: ClashStatus::New,
            trade: "Mechanical".into(),
            notes: "Check with engineer".into(),
            group_key: String::new(),
        }];
        let csv = export_csv(&clashes);
        assert!(csv.contains("C0001"));
        assert!(csv.contains("crossing"));
        assert!(csv.contains("New"));
        assert!(csv.contains("Mechanical"));
        assert!(csv.contains("12.500"));
        assert!(csv.contains("34.750"));
    }

    #[test]
    fn bcf_export_produces_valid_zip() {
        let clashes = vec![
            ClashItem {
                id: "C0001".into(),
                kind: "crossing".into(),
                message: "Supply crosses return".into(),
                systems: vec!["HVAC-Supply".into()],
                at: [10.0, 20.0],
                by_in: 2.0,
                handles: vec![],
                level: String::new(),
                status: ClashStatus::Active,
                trade: "HVAC".into(),
                notes: String::new(),
                group_key: String::new(),
            },
            ClashItem {
                id: "C0002".into(),
                kind: "column".into(),
                message: "Pipe through column".into(),
                systems: vec!["Plumbing".into()],
                at: [5.0, 5.0],
                by_in: 0.0,
                handles: vec![],
                level: String::new(),
                status: ClashStatus::Resolved,
                trade: "Plumbing".into(),
                notes: "Rerouted".into(),
                group_key: String::new(),
            },
        ];
        let zip = export_bcf(&clashes);
        // Verify it starts with the ZIP local file header signature.
        assert!(zip.len() > 4);
        assert_eq!(&zip[..4], &0x04034B50u32.to_le_bytes());
        // Contains bcf.version.
        let s = String::from_utf8_lossy(&zip);
        assert!(s.contains("bcf.version"));
        assert!(s.contains("markup.bcf"));
        assert!(s.contains("viewpoint.bcfv"));
        // Contains our clash data.
        assert!(s.contains("Supply crosses return"));
        assert!(s.contains("Active"));
        assert!(s.contains("Rerouted"));
    }

    #[test]
    fn grouping_clusters_nearby_clashes() {
        let make = |id: &str, kind: &str, at: [f64; 2], group_key: &str| ClashItem {
            id: id.into(),
            kind: kind.into(),
            message: format!("Clash {id}"),
            systems: vec!["sys".into()],
            at,
            by_in: 1.0,
            handles: vec![],
            level: String::new(),
            status: ClashStatus::New,
            trade: String::new(),
            notes: String::new(),
            group_key: group_key.into(),
        };
        let clashes = vec![
            make("C1", "column", [10.0, 10.0], "COL1"),
            make("C2", "column", [10.0, 10.5], "COL1"),
            make("C3", "column", [10.0, 11.0], "COL1"),
            make("C4", "crossing", [50.0, 50.0], ""),
            make("C5", "crossing", [50.5, 50.0], ""),
            make("C6", "crossing", [100.0, 100.0], ""),
        ];
        let groups = group_clashes(&clashes);
        // COL1 group: 3 clashes.
        let col_group = groups.iter().find(|g| g.clash_ids.len() == 3);
        assert!(col_group.is_some(), "Expected a group of 3 for COL1");
        // C4 and C5 are within 2 ft, should be grouped.
        let nearby = groups.iter().find(|g| g.clash_ids.len() == 2 && g.clash_ids.contains(&3));
        assert!(nearby.is_some(), "Expected C4+C5 grouped by proximity");
        // C6 is alone.
        let solo = groups.iter().find(|g| g.clash_ids.len() == 1 && g.clash_ids.contains(&5));
        assert!(solo.is_some(), "Expected C6 as a solo group");
    }

    /// A session with two made-up systems on one level: a supply duct crossing a water pipe at
    /// the same height (a crossing clash), above a 9 ft ceiling.
    fn crossing_session() -> Session {
        let mut s = Session::new();
        s.execute(
            "bspec.set",
            &json!({"replace": true, "spec": {
                "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 13, "ceiling_ft": 9, "region": [-1000, -1000, 1000, 1000], "base": [0, 0]}],
                "mep": {
                    "systems": [
                        {"name": "M-DUCT-SUPP", "layer": "M-DUCT-SUPP", "kind": "duct", "size_in": [12, 12], "bottom_ft": 10},
                        {"name": "P-DCW", "layer": "P-DCW", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 10}
                    ],
                    "runs": [
                        {"system": "M-DUCT-SUPP", "level": "L1", "points": [[0, 120], [480, 120]], "size_in": [12, 12], "bottoms_ft": [10]},
                        {"system": "P-DCW", "level": "L1", "points": [[240, 0], [240, 240]], "size_in": [2, 0], "bottoms_ft": [10.3]}
                    ]
                }
            }}),
        )
        .unwrap();
        s
    }

    /// The palette's Re-run (the shipped `rerun`): a triaged clash keeps its id, status, trade and
    /// notes; after an edit that fixes it, it stays in the list as resolved; a clash the edit makes
    /// is new, with an id of its own.
    #[test]
    fn rerun_keeps_triage_and_marks_fixed_clashes_resolved() {
        let mut app = crate::CadApp::new(crossing_session(), crate::Services::default());
        let mut st = State::default();
        rerun(&mut app, &mut st);
        let crossing = st.clashes.iter().position(|c| c.kind == "crossing").unwrap();
        assert_eq!(st.clashes[crossing].level, "L1");
        let id = st.clashes[crossing].id.clone();
        st.clashes[crossing].status = ClashStatus::Active;
        st.clashes[crossing].trade = "Plumbing".into();
        st.clashes[crossing].notes = "Drop the pipe under the duct".into();
        // Nothing changed: the same clash, same id, same triage.
        rerun(&mut app, &mut st);
        let c = st.clashes.iter().find(|c| c.kind == "crossing").unwrap();
        assert_eq!(
            (c.id.as_str(), c.status, c.trade.as_str(), c.notes.as_str()),
            (id.as_str(), ClashStatus::Active, "Plumbing", "Drop the pipe under the duct")
        );
        // The pipe drops to 7 ft: the crossing is fixed and it now hangs below the ceiling.
        app.session.execute("mep.edit", &json!({"id": 1, "bottom_ft": 7.0})).unwrap();
        rerun(&mut app, &mut st);
        let fixed = st.clashes.iter().find(|c| c.id == id).unwrap();
        assert_eq!(fixed.status, ClashStatus::Resolved, "{:?}", st.clashes);
        assert_eq!(fixed.notes, "Drop the pipe under the duct");
        let below = st.clashes.iter().find(|c| c.kind == "below_ceiling" && c.systems == ["P-DCW"]).unwrap();
        assert_eq!(below.status, ClashStatus::New);
        assert_ne!(below.id, id);
    }

    /// The palette's PDF export goes through the engine's report command (a build without a
    /// plotter says so instead of writing nothing silently).
    #[test]
    fn pdf_export_uses_the_report_command() {
        let mut app = crate::CadApp::new(crossing_session(), crate::Services::default());
        let mut st = State::default();
        rerun(&mut app, &mut st);
        let rows = report_clashes(&st.clashes);
        assert_eq!(rows.len(), st.clashes.len());
        assert!(rows.iter().all(|r| r["id"].as_str().is_some_and(|i| i.starts_with('C')) && r["status"] == "New"));
        let out = cadaddy_engine::cmd::clashreport::report_pdf(&mut app.session, &json!({"clashes": rows})).unwrap();
        assert_eq!(out.clashes, st.clashes.len());
        if cadaddy_engine::cmd::file::io().and_then(|h| h.plot).is_none() {
            let e = export_pdf(&mut app, &st.clashes, "clashes.pdf").unwrap_err();
            assert!(e.contains("PDF export isn't available"), "{e}");
        }
    }

    #[test]
    fn clashdetect_opens_the_palette() {
        let mut app = crate::CadApp::new(Session::new(), crate::Services::default());
        crate::menus::run_ui_command(&mut app, "clashdetect", &Value::Null);
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
    }

    #[test]
    fn palette_renders_without_crash() {
        let mut app = crate::CadApp::new(Session::new(), crate::Services::default());
        app.ui.dialog = Some(DIALOG.into());
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
    }
}
