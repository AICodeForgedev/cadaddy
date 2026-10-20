//! The Drawing Properties dialog (DWGPROPS typed or File ▸ Drawing Properties...): General (name,
//! folder, size), Summary (title, subject, author, keywords, comments, hyperlink base, revision
//! number), Statistics (created, modified, last saved by, editing time, objects) and Custom (name
//! and value rows). It edits a copy; OK runs DWGPROPS with it and Cancel changes nothing. And the
//! System Variable Monitor (SYSVARMONITOR): the watched variables with their values now, Add,
//! Remove, notify, Reset All.

use egui::vec2;
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "dwgprops";
pub const MONITOR: &str = "sysvarmonitor";

const SUMMARY: [(&str, &str); 6] = [
    ("title", "Title"),
    ("subject", "Subject"),
    ("author", "Author"),
    ("keywords", "Keywords"),
    ("comments", "Comments"),
    ("revisionNumber", "Revision number"),
];

#[derive(Clone, Default)]
struct State {
    report: Value,
    tab: u8,
    summary: Vec<(&'static str, String)>,
    base: String,
    custom: Vec<(String, String)>,
    removed: Vec<String>,
    status: String,
}

fn id() -> egui::Id {
    egui::Id::new("dwgprops_dialog_state")
}

fn current(app: &CadApp) -> State {
    let report = cadaddy_engine::cmd::dwgprops::report(&app.session).unwrap_or(Value::Null);
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    State {
        summary: SUMMARY.iter().map(|(k, _)| (*k, text(&report["summary"][*k]))).collect(),
        base: text(&report["hyperlinkBase"]),
        custom: report["custom"].as_array().into_iter().flatten().map(|c| (text(&c["name"]), text(&c["value"]))).collect(),
        report,
        ..State::default()
    }
}

pub fn open(app: &mut CadApp) {
    app.ui.dialog = Some(DIALOG.into());
}

/// DWGPROPS's parameters for the edited copy: every summary field, the hyperlink base, the
/// custom rows (removed ones as null).
fn params(st: &State) -> Value {
    let mut p = serde_json::Map::new();
    for (k, v) in &st.summary {
        p.insert((*k).to_string(), json!(v));
    }
    p.insert("hyperlinkBase".into(), json!(st.base));
    let mut custom = serde_json::Map::new();
    for n in &st.removed {
        custom.insert(n.clone(), Value::Null);
    }
    for (n, v) in st.custom.iter().filter(|(n, _)| !n.trim().is_empty()) {
        custom.insert(n.trim().to_string(), json!(v));
    }
    p.insert("custom".into(), Value::Object(custom));
    Value::Object(p)
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let mut st = ctx.data_mut(|d| d.get_temp::<State>(id())).unwrap_or_else(|| current(app));
    let (mut ok, mut cancel) = (false, false);
    let title = st.report["general"]["name"].as_str().unwrap_or_default().to_string();
    egui::Window::new(crate::tf!("{name} Properties", name = title))
        .id(egui::Id::new("dwgprops_window"))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(420.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            let mut body = crate::rtl::Body::new(ui);
            let ui = &mut *body;
            ui.horizontal(|ui| {
                for (i, label) in
                    [crate::tl!("General"), crate::tl!("Summary"), crate::tl!("Statistics"), crate::tl!("Custom")].into_iter().enumerate()
                {
                    if ui.selectable_label(st.tab == i as u8, label).clicked() {
                        st.tab = i as u8;
                    }
                }
            });
            ui.separator();
            let r = st.report.clone();
            let fact = |v: &Value| match v {
                Value::Null => "—".to_string(),
                Value::String(s) if s.is_empty() => "—".to_string(),
                Value::String(s) => s.clone(),
                v => v.to_string(),
            };
            let grid = egui::Grid::new("dwgprops_grid").num_columns(2).spacing(vec2(8.0, 4.0));
            match st.tab {
                0 => {
                    crate::rtl::show_grid(grid, ui, |ui| {
                        for (label, v) in [
                            (crate::tl!("Name"), &r["general"]["name"]),
                            (crate::tl!("Location"), &r["general"]["folder"]),
                            (crate::tl!("Size"), &r["general"]["size"]),
                        ] {
                            ui.label(label);
                            ui.label(fact(v));
                            ui.end_row();
                        }
                    });
                }
                1 => {
                    crate::rtl::show_grid(grid, ui, |ui| {
                        for (i, (_, label)) in SUMMARY.iter().enumerate() {
                            ui.label(crate::i18n::t(label));
                            if let Some((_, v)) = st.summary.get_mut(i) {
                                if i == 4 {
                                    ui.add(egui::TextEdit::multiline(v).desired_rows(3).desired_width(260.0));
                                } else {
                                    ui.add(egui::TextEdit::singleline(v).desired_width(260.0));
                                }
                            }
                            ui.end_row();
                        }
                        ui.label(crate::tl!("Hyperlink base"));
                        ui.add(egui::TextEdit::singleline(&mut st.base).desired_width(260.0));
                        ui.end_row();
                    });
                }
                2 => {
                    crate::rtl::show_grid(grid, ui, |ui| {
                        for (label, v) in [
                            (crate::tl!("Created"), &r["statistics"]["created"]),
                            (crate::tl!("Modified"), &r["statistics"]["modified"]),
                            (crate::tl!("Last saved by"), &r["summary"]["lastSavedBy"]),
                            (crate::tl!("Total editing time (minutes)"), &r["statistics"]["editingMinutes"]),
                            (crate::tl!("Objects"), &r["statistics"]["objects"]),
                        ] {
                            ui.label(label);
                            ui.label(fact(v));
                            ui.end_row();
                        }
                    });
                }
                _ => {
                    let mut drop = None;
                    crate::rtl::show_grid(egui::Grid::new("dwgprops_custom").num_columns(3).spacing(vec2(6.0, 4.0)), ui, |ui| {
                        ui.strong(crate::tl!("Name"));
                        ui.strong(crate::tl!("Value"));
                        ui.label("");
                        ui.end_row();
                        for (i, (n, v)) in st.custom.iter_mut().enumerate() {
                            ui.add(egui::TextEdit::singleline(n).desired_width(130.0));
                            ui.add(egui::TextEdit::singleline(v).desired_width(190.0));
                            if ui.button(crate::tl!("Delete")).clicked() {
                                drop = Some(i);
                            }
                            ui.end_row();
                        }
                    });
                    if let Some(i) = drop
                        && i < st.custom.len()
                    {
                        let (n, _) = st.custom.remove(i);
                        if !n.trim().is_empty() {
                            st.removed.push(n.trim().to_string());
                        }
                    }
                    if ui.button(crate::tl!("Add...")).clicked() {
                        st.custom.push((format!("{} {}", crate::tl!("Property"), st.custom.len() + 1), String::new()));
                    }
                }
            }
            if !st.status.is_empty() {
                ui.colored_label(crate::theme::Tokens::get().warn, &st.status);
            }
            ui.separator();
            crate::rtl::row(ui, |ui| {
                ok = ui.button(crate::tl!("OK")).clicked();
                cancel = ui.button(crate::tl!("Cancel")).clicked();
            });
        });
    if ok {
        match app.run("dwgprops", params(&st)) {
            Ok(_) => *open = false,
            Err(e) => st.status = e,
        }
    }
    if cancel || !*open {
        ctx.data_mut(|d| d.remove::<State>(id()));
        *open = false;
    } else {
        ctx.data_mut(|d| d.insert_temp(id(), st));
    }
}

/// The System Variable Monitor: each change runs SYSVARMONITOR at once.
pub fn monitor(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let add_id = egui::Id::new("sysvarmonitor_add");
    let mut add = ctx.data_mut(|d| d.get_temp::<String>(add_id)).unwrap_or_default();
    let state = cadaddy_engine::script_tools::state(&app.session);
    let mut call: Option<Value> = None;
    egui::Window::new(crate::tl!("System Variable Monitor"))
        .id(egui::Id::new("sysvarmonitor_window"))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_width(380.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            let mut body = crate::rtl::Body::new(ui);
            let ui = &mut *body;
            crate::rtl::show_grid(egui::Grid::new("sysvarmonitor_grid").num_columns(4).spacing(vec2(8.0, 4.0)), ui, |ui| {
                ui.strong(crate::tl!("Variable"));
                ui.strong(crate::tl!("Watched value"));
                ui.strong(crate::tl!("Current value"));
                ui.label("");
                ui.end_row();
                for v in state["variables"].as_array().into_iter().flatten() {
                    let name = v["name"].as_str().unwrap_or_default().to_string();
                    let changed = v["watched"] != v["now"];
                    ui.label(&name);
                    ui.label(v["watched"].to_string());
                    let now = egui::RichText::new(v["now"].to_string());
                    ui.label(if changed { now.color(crate::theme::Tokens::get().warn) } else { now });
                    if ui.button(crate::tl!("Remove")).clicked() {
                        call = Some(json!({ "remove": [name] }));
                    }
                    ui.end_row();
                }
            });
            crate::rtl::row(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut add).hint_text("FILLETRAD").desired_width(140.0));
                if ui.button(crate::tl!("Add")).clicked() && !add.trim().is_empty() {
                    call = Some(json!({ "add": [add.trim()] }));
                }
            });
            let mut notify = state["notify"].as_bool().unwrap_or(false);
            if ui.checkbox(&mut notify, crate::tl!("Notify when a watched variable changes")).changed() {
                call = Some(json!({ "notify": notify }));
            }
            ui.separator();
            crate::rtl::row(ui, |ui| {
                if ui.button(crate::tl!("Reset All")).clicked() {
                    call = Some(json!({ "reset": true }));
                }
                if ui.button(crate::tl!("Close")).clicked() {
                    call = Some(Value::Null);
                }
            });
        });
    match call {
        Some(Value::Null) => *open = false,
        Some(p) => match app.run("sysvarmonitor", p) {
            Ok(_) => add.clear(),
            Err(e) => app.set_status(e),
        },
        None => {}
    }
    ctx.data_mut(|d| d.insert_temp(add_id, add));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_carry_the_summary_and_the_custom_rows() {
        let st = State {
            summary: SUMMARY.iter().map(|(k, _)| (*k, format!("{k}!"))).collect(),
            base: "https://example.com/".into(),
            custom: vec![("Phase".into(), "Design".into()), ("  ".into(), "dropped".into())],
            removed: vec!["Old".into()],
            ..State::default()
        };
        let p = params(&st);
        assert_eq!(p["title"], "title!");
        assert_eq!(p["hyperlinkBase"], "https://example.com/");
        assert_eq!(p["custom"], json!({ "Old": null, "Phase": "Design" }));
    }
}
