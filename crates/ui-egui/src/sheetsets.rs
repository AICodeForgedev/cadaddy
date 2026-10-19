//! The Sheet Set Manager (SHEETSET): the current sheet set's sheets by subset, and the SHEETSET
//! command's actions on them — new and open a set, add a drawing's layouts, edit a sheet's
//! number, title and subset, move and remove sheets, renumber, rename the layouts to match
//! (for the sheet fields), and publish the set to one PDF. The set's file is remembered.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

#[derive(Clone, Default)]
struct State {
    /// The listing of the current set (SHEETSET list), or why it couldn't be read.
    listing: Option<Result<Value, String>>,
    selected: Option<usize>,
    number: String,
    title: String,
    subset: String,
    prefix: String,
    start: String,
    digits: String,
}

fn id() -> egui::Id {
    egui::Id::new("cadaddy-sheetset-dialog")
}

/// Run a SHEETSET action on the current set; a failure is echoed and returned.
fn act(app: &mut CadApp, mut p: Value) -> Option<Value> {
    let set = app.ui.sheet_set.clone()?;
    p["set"] = json!(set);
    app.run("sheetset", p).ok()
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_else(|| State { start: "1".into(), ..Default::default() });
    if st.listing.is_none()
        && let Some(set) = app.ui.sheet_set.clone()
    {
        st.listing = Some(app.session.execute("sheetset", &json!({ "set": set })).map_err(|e| e.user_message()));
    }
    let mut todo: Option<Value> = None;
    let mut pick: Option<&'static str> = None;
    egui::Window::new(crate::tl!("Sheet Set Manager")).id(egui::Id::new("Sheet Set Manager")).open(open).resizable(true).default_width(460.0).show(
        ctx,
        |ui| {
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("New Sheet Set...")).clicked() {
                    pick = Some("new");
                }
                if ui.button(crate::tl!("Open Sheet Set...")).clicked() {
                    pick = Some("open");
                }
            });
            let Some(Ok(v)) = &st.listing else {
                match &st.listing {
                    Some(Err(e)) => {
                        ui.label(RichText::new(e).color(t.warn));
                    }
                    _ => {
                        ui.label(RichText::new(crate::tl!("No sheet set is open.")).color(t.text_dim));
                    }
                }
                return;
            };
            ui.label(RichText::new(v["name"].as_str().unwrap_or_default()).strong());
            let sheets = v["sheets"].as_array().cloned().unwrap_or_default();
            let mut subsets: Vec<String> = vec![String::new()];
            subsets.extend(v["subsets"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
            egui::ScrollArea::vertical().id_salt("sheetset-sheets").max_height(260.0).show(ui, |ui| {
                for sub in &subsets {
                    let in_sub: Vec<&Value> = sheets.iter().filter(|s| s["subset"].as_str().unwrap_or_default() == sub).collect();
                    let mut rows = |ui: &mut egui::Ui| {
                        for s in &in_sub {
                            let i = s["index"].as_u64().map_or(0, |i| i as usize);
                            let label = format!("{}  {}", s["number"].as_str().unwrap_or_default(), s["title"].as_str().unwrap_or_default());
                            let text = if s["found"] == true { RichText::new(label) } else { RichText::new(label).color(t.warn) };
                            let r = ui.selectable_label(st.selected == Some(i), text).on_hover_text(format!(
                                "{} — {}",
                                s["drawing"].as_str().unwrap_or_default(),
                                s["layout"].as_str().unwrap_or_default()
                            ));
                            if r.clicked() {
                                st.selected = Some(i);
                                st.number = s["number"].as_str().unwrap_or_default().into();
                                st.title = s["title"].as_str().unwrap_or_default().into();
                                st.subset = s["subset"].as_str().unwrap_or_default().into();
                            }
                        }
                    };
                    if sub.is_empty() {
                        rows(ui);
                    } else {
                        egui::CollapsingHeader::new(format!("📁 {sub}")).id_salt(("sheetset-subset", sub)).default_open(true).show(ui, rows);
                    }
                }
            });
            ui.separator();
            if let Some(i) = st.selected.filter(|i| *i < sheets.len()) {
                egui::Grid::new("sheetset-edit").num_columns(2).show(ui, |ui| {
                    ui.label(crate::tl!("Number"));
                    ui.text_edit_singleline(&mut st.number);
                    ui.end_row();
                    ui.label(crate::tl!("Title"));
                    ui.text_edit_singleline(&mut st.title);
                    ui.end_row();
                    ui.label(crate::tl!("Subset"));
                    ui.text_edit_singleline(&mut st.subset);
                    ui.end_row();
                });
                ui.horizontal(|ui| {
                    if ui.button(crate::tl!("Apply")).clicked() {
                        todo = Some(json!({ "action": "set", "sheet": i, "number": st.number, "title": st.title, "subset": st.subset }));
                    }
                    if ui.add_enabled(i > 0, egui::Button::new("▲")).clicked() {
                        todo = Some(json!({ "action": "move", "sheet": i, "to": i - 1 }));
                        st.selected = Some(i - 1);
                    }
                    if ui.add_enabled(i + 1 < sheets.len(), egui::Button::new("▼")).clicked() {
                        todo = Some(json!({ "action": "move", "sheet": i, "to": i + 1 }));
                        st.selected = Some(i + 1);
                    }
                    if ui.button(crate::tl!("Remove Sheet")).clicked() {
                        todo = Some(json!({ "action": "remove", "sheets": [i] }));
                        st.selected = None;
                    }
                });
                ui.separator();
            }
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Add Drawing...")).clicked() {
                    pick = Some("add");
                }
                if ui
                    .button(crate::tl!("Rename Layouts to Match"))
                    .on_hover_text(crate::tl!("Each sheet's layout becomes \"number title\", which the sheet number and title fields show"))
                    .clicked()
                {
                    todo = Some(json!({ "action": "sync" }));
                }
                if ui.add_enabled(!sheets.is_empty(), egui::Button::new(crate::tl!("Publish to PDF..."))).clicked() {
                    pick = Some("publish");
                }
            });
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Prefix"));
                ui.add(egui::TextEdit::singleline(&mut st.prefix).desired_width(50.0));
                ui.label(crate::tl!("Start"));
                ui.add(egui::TextEdit::singleline(&mut st.start).desired_width(40.0));
                ui.label(crate::tl!("Digits"));
                ui.add(egui::TextEdit::singleline(&mut st.digits).desired_width(30.0));
                if ui.button(crate::tl!("Renumber")).clicked() {
                    let start = st.start.trim().parse::<i64>().unwrap_or(1);
                    let digits = st.digits.trim().parse::<i64>().unwrap_or(0);
                    todo = Some(json!({ "action": "renumber", "prefix": st.prefix, "start": start, "digits": digits }));
                }
            });
        },
    );
    let name = app
        .ui
        .sheet_set
        .as_deref()
        .and_then(|s| std::path::Path::new(s).file_stem())
        .map(|s| s.to_string_lossy().trim_end_matches(".sheetset").to_string())
        .unwrap_or_else(|| "Sheets".into());
    match pick {
        Some("new") => {
            if let Some(path) = app.services.pick_save.as_ref().and_then(|p| p("Sheets.sheetset.json")) {
                if app.run("sheetset", json!({ "set": path, "action": "new", "overwrite": true })).is_ok() {
                    app.ui.sheet_set = Some(path);
                }
                st.listing = None;
            }
        }
        Some("open") => {
            if let Some(path) = app.services.pick_open.as_ref().and_then(|p| p()) {
                app.ui.sheet_set = Some(path);
                st.listing = None;
                st.selected = None;
            }
        }
        Some("add") => {
            if let Some(path) = app.services.pick_open.as_ref().and_then(|p| p()) {
                todo = Some(json!({ "action": "add", "drawing": path }));
            }
        }
        Some("publish") => {
            if let Some(path) = app.services.pick_save.as_ref().and_then(|p| p(&format!("{name}.pdf")))
                && let Some(r) = act(app, json!({ "action": "publish", "path": path }))
            {
                app.session.echo(format!("Published {} sheets to {}.", r["pages"], r["path"].as_str().unwrap_or_default()));
            }
        }
        _ => {}
    }
    if let Some(p) = todo
        && let Some(r) = act(app, p)
    {
        st.listing = Some(Ok(r));
    }
    if *open {
        ctx.data_mut(|d| d.insert_temp(id(), st));
    } else {
        ctx.data_mut(|d| d.remove::<State>(id()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheetset_opens_the_manager_and_without_a_set_says_so() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        crate::menus::run_ui_command(&mut app, "sheetset", &Value::Null);
        assert_eq!(app.ui.dialog.as_deref(), Some("sheetset"));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert_eq!(app.ui.dialog.as_deref(), Some("sheetset"), "it stays open");
        // A set that can't be read shows why.
        app.ui.sheet_set = Some("/no/such/folder/x.sheetset.json".into());
        ctx.data_mut(|d| d.remove::<State>(id()));
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        let st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
        assert!(matches!(st.listing, Some(Err(_))));
    }
}
