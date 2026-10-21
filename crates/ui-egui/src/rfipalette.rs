//! The RFI List palette (Window ▸ RFI List, RFILIST typed alone): where the prints are silent
//! or disagree and what was assumed meanwhile (`rfi.list`), most severe first, filtered by
//! severity and level. A click zooms the plan to an item (`rfi.zoom`); Walk to It stands in Walk
//! looking at it; each item is tracked (not sent, sent, answered, closed: `rfi.set`), and the list
//! exported as XLSX or PDF with pictures of the model (`rfi.export`).

use egui::{Color32, RichText};
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "rfilist";

/// What the palette keeps between frames.
#[derive(Clone, Default)]
struct State {
    severity: Option<String>,
    level: Option<String>,
    open_only: bool,
    selected: Option<String>,
}

fn badge(sev: &str) -> (&'static str, Color32) {
    match sev {
        "high" => ("H", Color32::from_rgb(210, 60, 50)),
        "low" => ("L", Color32::from_rgb(90, 150, 90)),
        _ => ("M", Color32::from_rgb(220, 160, 40)),
    }
}

fn status_label(s: &str) -> &'static str {
    match s {
        "sent" => crate::tl!("Sent"),
        "answered" => crate::tl!("Answered"),
        "closed" => crate::tl!("Closed"),
        _ => crate::tl!("Not sent"),
    }
}

fn severity_label(s: &str) -> &'static str {
    match s {
        "high" => crate::tl!("High"),
        "low" => crate::tl!("Low"),
        _ => crate::tl!("Medium"),
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let id = egui::Id::new("rfi-palette-state");
    let mut st: State = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let mut q = json!({});
    if let Some(s) = &st.severity {
        q["severity"] = json!(s);
    }
    if let Some(l) = &st.level {
        q["level"] = json!(l);
    }
    if st.open_only {
        q["status"] = json!("open");
    }
    let r = app.session.execute("rfi.list", &q).unwrap_or(Value::Null);
    let list: Vec<Value> = r["items"].as_array().cloned().unwrap_or_default();
    let levels: Vec<String> = app
        .session
        .execute("bspec.get", &json!({}))
        .ok()
        .and_then(|v| v["spec"]["levels"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|l| l["name"].as_str().map(String::from))
        .collect();
    let mut zoom: Option<String> = None;
    let mut walk: Option<(String, [f64; 2])> = None;
    let mut set: Option<(String, String)> = None;
    let mut export: Option<&str> = None;
    egui::Window::new(crate::tl!("RFI List"))
        .id(egui::Id::new("RFI List"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([420.0, 520.0])
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                let shown = st.severity.as_deref().map_or(crate::tl!("All"), severity_label);
                egui::ComboBox::from_id_salt("rfi-severity").selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut st.severity, None, crate::tl!("All"));
                    for s in ["high", "medium", "low"] {
                        ui.selectable_value(&mut st.severity, Some(s.to_string()), severity_label(s));
                    }
                });
                let shown = st.level.clone().unwrap_or_else(|| crate::tl!("All levels").to_string());
                egui::ComboBox::from_id_salt("rfi-level").selected_text(shown).show_ui(ui, |ui| {
                    ui.selectable_value(&mut st.level, None, crate::tl!("All levels"));
                    for l in &levels {
                        ui.selectable_value(&mut st.level, Some(l.clone()), l);
                    }
                });
                ui.checkbox(&mut st.open_only, crate::tl!("Open items only"));
            });
            let n = |k: &str| r["bySeverity"][k].as_u64().unwrap_or(0);
            ui.label(
                RichText::new(crate::tf!("{high} high, {medium} medium, {low} low", high = n("high"), medium = n("medium"), low = n("low")))
                    .small()
                    .color(t.text_dim),
            );
            ui.separator();
            egui::ScrollArea::vertical().id_salt("rfi-list").max_height(260.0).show(ui, |ui| {
                if list.is_empty() {
                    ui.label(RichText::new(crate::tl!("No gaps or conflicts in the building spec.")).color(t.text_dim));
                }
                for item in &list {
                    let rid = item["id"].as_str().unwrap_or_default().to_string();
                    let (b, colour) = badge(item["severity"].as_str().unwrap_or_default());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(b).strong().color(colour));
                        let lv = item["level"].as_str().unwrap_or_default();
                        let text = format!(
                            "{rid}  {}{}",
                            item["title"].as_str().unwrap_or_default(),
                            if lv.is_empty() { String::new() } else { format!("  ({lv})") }
                        );
                        let done = matches!(item["status"].as_str(), Some("answered" | "closed"));
                        let rich = if done { RichText::new(text).strikethrough().color(t.text_dim) } else { RichText::new(text) };
                        let picked = st.selected.as_deref() == Some(rid.as_str());
                        if ui.selectable_label(picked, rich).on_hover_text(item["issue"].as_str().unwrap_or_default()).clicked() {
                            st.selected = Some(rid.clone());
                            zoom = Some(rid.clone());
                        }
                    });
                }
            });
            ui.separator();
            let sel = st.selected.as_ref().and_then(|s| list.iter().find(|i| i["id"].as_str() == Some(s.as_str())));
            if let Some(item) = sel {
                let rid = item["id"].as_str().unwrap_or_default().to_string();
                ui.label(RichText::new(format!("{rid}  {}", item["title"].as_str().unwrap_or_default())).strong());
                if let Some(a) = item["about"].as_str().filter(|a| !a.is_empty()) {
                    ui.label(RichText::new(a).color(t.text_dim));
                }
                ui.label(item["issue"].as_str().unwrap_or_default());
                ui.label(RichText::new(crate::tl!("Assumed meanwhile:")).small().color(t.text_dim));
                ui.label(item["assumed"].as_str().unwrap_or_default());
                if let Some(p) = item["proposed"].as_str() {
                    ui.label(RichText::new(crate::tl!("Proposed fix:")).small().color(t.text_dim));
                    ui.label(p);
                }
                let sheets: Vec<&str> = item["sheets"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                if !sheets.is_empty() {
                    ui.label(RichText::new(format!("{} {}", crate::tl!("Sheets:"), sheets.join(", "))).small().color(t.text_dim));
                }
                ui.horizontal(|ui| {
                    let status = item["status"].as_str().unwrap_or("open").to_string();
                    egui::ComboBox::from_id_salt("rfi-status").selected_text(status_label(&status)).show_ui(ui, |ui| {
                        for s in ["open", "sent", "answered", "closed"] {
                            if ui.selectable_label(status == s, status_label(s)).clicked() && status != s {
                                set = Some((rid.clone(), s.to_string()));
                            }
                        }
                    });
                    let lv = item["level"].as_str().unwrap_or_default();
                    let at = item["at"]
                        .as_array()
                        .map(|a| [a.first().and_then(Value::as_f64).unwrap_or(f64::NAN), a.get(1).and_then(Value::as_f64).unwrap_or(f64::NAN)]);
                    if !lv.is_empty()
                        && let Some(at) = at
                    {
                        if ui.button(crate::tl!("Zoom")).clicked() {
                            zoom = Some(rid.clone());
                        }
                        if ui.button(crate::tl!("Walk to It")).clicked() {
                            walk = Some((lv.to_string(), at));
                        }
                    }
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Export XLSX...")).clicked() {
                    export = Some("xlsx");
                }
                if ui.button(crate::tl!("Export PDF...")).clicked() {
                    export = Some("pdf");
                }
            });
        });
    ctx.data_mut(|d| d.insert_temp(id, st));
    if let Some(rid) = zoom {
        let _ = app.run("rfi.zoom", json!({ "id": rid }));
        app.canvas.list = None;
    }
    if let Some((rid, status)) = set {
        let _ = app.run("rfi.set", json!({ "id": rid, "status": status }));
    }
    if let Some((level, at)) = walk
        && let Err(e) = crate::walk::look_at(app, &level, at)
    {
        app.set_status(format!("Walk: {e}"));
    }
    if let Some(format) = export {
        let name = format!("RFI list.{format}");
        let picked = app.services.pick_save.as_ref().and_then(|f| f(&name));
        if let Some(path) = picked {
            match app.run("rfi.export", json!({ "path": path, "format": format })) {
                Ok(r) => app.set_status(format!("RFI list: {} items, {} pictures → {path}", r["items"], r["pictures"])),
                Err(e) => app.set_status(format!("RFI list: {e}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_gaps() -> CadApp {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        let spec = json!({
            "version": 1, "units_per_foot": 12,
            "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}],
            "wall_types": [{"name": "P1", "thickness_in": 5, "assembly": {"tag": "P1", "height": "ceiling", "fire_rating": "1 HR", "layers": []}}],
            "walls": [{"level": "1", "a": [0, 0], "b": [240, 0], "wall_type": "P1", "guess": "the only type that thick"}],
            "rfis": [{"kind": "sleeve_no_route", "severity": "high", "level": "1", "at": [120, 60], "sheets": ["A-101"], "issue": "\"3\" SLEEVE\" gives no route", "assumed": "Not routed."}]
        });
        app.run("bspec.set", json!({ "replace": true, "spec": spec })).unwrap();
        app
    }

    fn frame(app: &mut CadApp) {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(app, ui.ctx()));
        out.textures_delta.clear();
    }

    #[test]
    fn rfilist_opens_the_palette_and_renders() {
        let mut app = app_with_gaps();
        app.cmdline("rfilist");
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        frame(&mut app);
        let r = app.run("rfi.list", json!({})).unwrap();
        let kinds: Vec<&str> = r["items"].as_array().unwrap().iter().filter_map(|i| i["kind"].as_str()).collect();
        assert!(kinds.contains(&"sleeve_no_route") && kinds.contains(&"rated_to_ceiling") && kinds.contains(&"untagged_wall"), "{kinds:?}");
    }

    #[test]
    fn the_palette_renders_in_arabic_and_with_nothing_to_list() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.ui.dialog = Some(DIALOG.into());
        frame(&mut app);
        let mut app = app_with_gaps();
        let _ = crate::menus::run_ui_command(&mut app, "ui.language", &json!({ "lang": "ar" }));
        app.ui.dialog = Some(DIALOG.into());
        frame(&mut app);
    }
}
