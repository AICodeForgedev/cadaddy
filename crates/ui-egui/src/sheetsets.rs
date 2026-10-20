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
    /// The last publish's outcome: where it went and the sheets warned about.
    published: Option<(String, Vec<String>)>,
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
    egui::Window::new(crate::tl!("Sheet Set Manager"))
        .id(egui::Id::new("Sheet Set Manager"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_width(460.0)
        .show(ctx, |ui| {
            // Right to left in Arabic (rtl.rs).
            let mut body = crate::rtl::Body::new(ui);
            let ui = &mut *body;
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
                crate::rtl::show_grid(egui::Grid::new("sheetset-edit").num_columns(2), ui, |ui| {
                    crate::rtl::lead(ui, crate::tl!("Number"));
                    ui.text_edit_singleline(&mut st.number);
                    crate::rtl::trail(ui, crate::tl!("Number"));
                    ui.end_row();
                    crate::rtl::lead(ui, crate::tl!("Title"));
                    ui.text_edit_singleline(&mut st.title);
                    crate::rtl::trail(ui, crate::tl!("Title"));
                    ui.end_row();
                    crate::rtl::lead(ui, crate::tl!("Subset"));
                    ui.text_edit_singleline(&mut st.subset);
                    crate::rtl::trail(ui, crate::tl!("Subset"));
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
            if let Some((path, warnings)) = &st.published {
                ui.label(RichText::new(crate::tf!("Published to {path}", path = path)).color(t.text_dim));
                for w in warnings {
                    ui.label(RichText::new(format!("⚠ {w}")).color(t.warn));
                }
            }
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
        });
    match pick {
        Some("new") => {
            if let Some(path) = app.services.pick_save.as_ref().and_then(|p| p("Sheets.sheetset.json")) {
                // The command names the file `<name>.sheetset.json` whatever the dialog added.
                if let Ok(r) = app.run("sheetset", json!({ "set": path, "action": "new", "overwrite": true })) {
                    app.ui.sheet_set = Some(r["set"].as_str().map_or(path, str::to_string));
                }
                st.listing = None;
                st.selected = None;
                st.published = None;
            }
        }
        Some("open") => {
            if let Some(path) = app.services.pick_open.as_ref().and_then(|p| p()) {
                app.ui.sheet_set = Some(path);
                st.listing = None;
                st.selected = None;
                st.published = None;
            }
        }
        Some("add") => {
            if let Some(path) = app.services.pick_open.as_ref().and_then(|p| p()) {
                todo = Some(json!({ "action": "add", "drawing": path }));
            }
        }
        Some("publish") => {
            let suggested = app.ui.sheet_set.as_deref().map_or_else(|| "Sheets.pdf".to_string(), publish_suggestion);
            if let Some(path) = app.services.pick_save.as_ref().and_then(|p| p(&suggested))
                && let Some(r) = act(app, json!({ "action": "publish", "path": path }))
            {
                // The sheets that plot blank were warned about on the command line first.
                app.session.echo(format!("Published {} sheets to {}.", r["pages"], r["path"].as_str().unwrap_or_default()));
                let warnings = r["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
                st.published = Some((r["path"].as_str().unwrap_or_default().to_string(), warnings));
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

/// The PDF a sheet set is published to by default: named after the set, in the set's own folder.
fn publish_suggestion(set: &str) -> String {
    let path = std::path::Path::new(set);
    let file = path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
    let lower = file.to_ascii_lowercase();
    let stem =
        [".sheetset.json", ".json"].iter().find(|e| lower.ends_with(*e)).map_or(file.as_str(), |e| file.get(..file.len() - e.len()).unwrap_or(""));
    let name = format!("{}.pdf", if stem.trim().is_empty() { "Sheets" } else { stem });
    let dir = std::path::absolute(path).ok().and_then(|p| p.parent().map(std::path::Path::to_path_buf)).filter(|d| !d.as_os_str().is_empty());
    match dir {
        Some(d) => d.join(name).to_string_lossy().into_owned(),
        None => name,
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

    /// Every text the frame painted.
    fn painted(out: &egui::FullOutput) -> Vec<String> {
        out.shapes.iter().filter_map(|c| if let egui::Shape::Text(t) = &c.shape { Some(t.galley.text().to_string()) } else { None }).collect()
    }

    #[test]
    fn the_last_publish_and_its_blank_sheets_show_in_the_manager() {
        let dir = std::env::temp_dir().join(format!("cadaddy-ssm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let set = dir.join("Quarry Hill.sheetset.json");
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.run("sheetset", json!({ "set": set.to_string_lossy(), "action": "new" })).unwrap();
        app.ui.sheet_set = Some(set.to_string_lossy().into_owned());
        let ctx = egui::Context::default();
        let warning = "Sheet 2 Spare (empty.dxf, Layout1) has nothing on it: its page is blank.";
        ctx.data_mut(|d| {
            d.insert_temp(id(), State { published: Some(("out.pdf".into(), vec![warning.into()])), start: "1".into(), ..Default::default() })
        });
        let mut open = true;
        let mut texts = Vec::new();
        // A window is laid out unseen on its first frame.
        for _ in 0..3 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| dialog(&mut app, ui.ctx(), &mut open));
            out.textures_delta.clear();
            texts = painted(&out);
        }
        assert!(texts.iter().any(|t| t.contains(warning)), "{texts:?}");
        assert!(texts.iter().any(|t| t == "Quarry Hill"), "the set's name: {texts:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn publishing_suggests_the_sheet_sets_own_folder() {
        let dir = std::env::temp_dir().join("Some Jobs").join("Quarry Hill");
        let set = dir.join("Quarry Hill.sheetset.json");
        let want = std::path::absolute(dir.join("Quarry Hill.pdf")).unwrap();
        assert_eq!(std::path::PathBuf::from(publish_suggestion(&set.to_string_lossy())), want);
        let odd = dir.join(".sheetset.json");
        assert!(publish_suggestion(&odd.to_string_lossy()).ends_with("Sheets.pdf"));
    }
}
