//! The Sheet Set Manager palette (SHEETSET, docked like the other palettes): the current sheet
//! set's sheets by subset, and the SHEETSET command's actions on them — new and open a set, add a
//! drawing's layouts, edit a sheet's number, title, description and subset, move and remove
//! sheets, renumber, sync the drawings (layout names, the `\AcSm` fields' data, view labels and
//! callouts), custom properties of the set and its sheets, a page setup override for publishing,
//! named views placed on sheets with callouts to them, sheet set fields, and publish to one PDF.
//! The set's file is remembered.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::docking::Panel;
use crate::theme::Tokens;

#[derive(Clone, Default)]
struct State {
    /// The listing of the current set (SHEETSET list), or why it couldn't be read.
    listing: Option<Result<Value, String>>,
    selected: Option<usize>,
    number: String,
    title: String,
    description: String,
    subset: String,
    prefix: String,
    start: String,
    digits: String,
    /// The last publish's outcome: where it went and the sheets warned about.
    published: Option<(String, Vec<String>)>,
    /// A custom property being added (name, value), to the set or to the selected sheet.
    set_key: String,
    set_value: String,
    sheet_key: String,
    sheet_value: String,
    /// A page setup taken from the selected sheet's layout: its name.
    setup_name: String,
    /// Where on the sheet a view, callout or field goes, and the named view to place.
    at_x: String,
    at_y: String,
    scale: String,
    view: String,
    /// The view a callout points at (its id).
    target: Option<u64>,
    field: String,
}

fn id() -> egui::Id {
    egui::Id::new("cadaddy-sheetset-palette")
}

/// Open the palette (SHEETSET, SSM): floating beside the drawing the first time, afterwards
/// where it was closed.
pub fn open(app: &mut CadApp) -> Result<Value, String> {
    let panel = Panel::SheetSets;
    let first = !app.ui.docking.layout.contains(&panel) && !app.ui.docking.hidden.iter().any(|(p, _)| *p == panel);
    let r = crate::docking::command(app, false, &json!({ "operation": "open", "panel": panel.id() }))?;
    if !first {
        return Ok(r);
    }
    let area = app.canvas.rect.unwrap_or(egui::Rect::from_min_size(egui::pos2(220.0, 80.0), egui::vec2(1000.0, 800.0)));
    let (w, h) = (380.0, (area.height() - 40.0).clamp(240.0, 720.0));
    let rect = [area.right() - w - 16.0, area.top() + 24.0, w, h];
    crate::docking::command(app, false, &json!({ "operation": "float", "panel": panel.id(), "rect": rect }))
}

/// Run a SHEETSET action on the current set; a failure is echoed and returned.
fn act(app: &mut CadApp, mut p: Value) -> Option<Value> {
    let set = app.ui.sheet_set.clone()?;
    p["set"] = json!(set);
    app.run("sheetset", p).ok()
}

fn str_of(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

/// A position typed on the palette, as [x, y] (0 where it isn't a number).
fn at(st: &State) -> Value {
    let n = |s: &str| s.trim().parse::<f64>().ok().filter(|v| v.is_finite()).unwrap_or(0.0);
    json!([n(&st.at_x), n(&st.at_y)])
}

/// Custom properties as rows with a remove button, and a row adding one. Returns the change:
/// {name: value} or {name: null}.
fn properties(ui: &mut egui::Ui, salt: &str, props: &Value, key: &mut String, value: &mut String) -> Option<Value> {
    let mut change = None;
    crate::rtl::show_grid(egui::Grid::new(("sheetset-props", salt)).num_columns(3), ui, |ui| {
        for (k, v) in props.as_object().into_iter().flatten() {
            ui.label(k);
            ui.label(v.as_str().map_or_else(|| v.to_string(), str::to_string));
            if ui.small_button("✕").on_hover_text(crate::tl!("Remove Property")).clicked() {
                change = Some(json!({ k: null }));
            }
            ui.end_row();
        }
        ui.add(egui::TextEdit::singleline(key).hint_text(crate::tl!("Name")).desired_width(90.0));
        ui.add(egui::TextEdit::singleline(value).hint_text(crate::tl!("Value")).desired_width(120.0));
        if ui.add_enabled(!key.trim().is_empty(), egui::Button::new(crate::tl!("Add"))).clicked() {
            change = Some(json!({ key.trim(): value.clone() }));
            key.clear();
            value.clear();
        }
        ui.end_row();
    });
    change
}

pub fn show(app: &mut CadApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_else(|| State { start: "1".into(), scale: "1".into(), ..Default::default() });
    if st.listing.is_none()
        && let Some(set) = app.ui.sheet_set.clone()
    {
        st.listing = Some(app.session.execute("sheetset", &json!({ "set": set })).map_err(|e| e.user_message()));
    }
    let mut todo: Option<Value> = None;
    let mut pick: Option<&'static str> = None;
    egui::ScrollArea::vertical().id_salt("sheetset-palette").auto_shrink([false, false]).show(ui, |ui| {
        // Right to left in Arabic (rtl.rs).
        let mut body = crate::rtl::Body::new(ui);
        let ui = &mut *body;
        let t = Tokens::get();
        ui.add_space(4.0);
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
        let v = v.clone();
        ui.label(RichText::new(str_of(&v["name"])).strong()).on_hover_text(str_of(&v["description"]));
        let sheets = v["sheets"].as_array().cloned().unwrap_or_default();
        let mut subsets: Vec<String> = vec![String::new()];
        subsets.extend(v["subsets"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
        egui::ScrollArea::vertical().id_salt("sheetset-sheets").max_height(240.0).show(ui, |ui| {
            for sub in &subsets {
                let in_sub: Vec<&Value> = sheets.iter().filter(|s| str_of(&s["subset"]) == sub).collect();
                let mut rows = |ui: &mut egui::Ui| {
                    for s in &in_sub {
                        let i = s["index"].as_u64().map_or(0, |i| i as usize);
                        let label = format!("{}  {}", str_of(&s["number"]), str_of(&s["title"]));
                        let text = if s["found"] == true { RichText::new(label) } else { RichText::new(label).color(t.warn) };
                        let r = ui
                            .selectable_label(st.selected == Some(i), text)
                            .on_hover_text(format!("{} — {}", str_of(&s["drawing"]), str_of(&s["layout"])));
                        if r.clicked() {
                            st.selected = Some(i);
                            st.number = str_of(&s["number"]).into();
                            st.title = str_of(&s["title"]).into();
                            st.description = str_of(&s["description"]).into();
                            st.subset = str_of(&s["subset"]).into();
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
        let selected = st.selected.filter(|i| *i < sheets.len());
        if let Some(i) = selected {
            crate::rtl::show_grid(egui::Grid::new("sheetset-edit").num_columns(2), ui, |ui| {
                for (label, text) in [
                    (crate::tl!("Number"), &mut st.number),
                    (crate::tl!("Title"), &mut st.title),
                    (crate::tl!("Description"), &mut st.description),
                    (crate::tl!("Subset"), &mut st.subset),
                ] {
                    crate::rtl::lead(ui, label);
                    ui.text_edit_singleline(text);
                    crate::rtl::trail(ui, label);
                    ui.end_row();
                }
            });
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Apply")).clicked() {
                    todo = Some(json!({ "action": "set", "sheet": i, "number": st.number, "title": st.title, "description": st.description, "subset": st.subset }));
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
            egui::CollapsingHeader::new(crate::tl!("Sheet Properties")).id_salt("sheetset-sheet-props").show(ui, |ui| {
                let props = sheets.get(i).map(|s| s["properties"].clone()).unwrap_or(Value::Null);
                if let Some(values) = properties(ui, "sheet", &props, &mut st.sheet_key, &mut st.sheet_value) {
                    todo = Some(json!({ "action": "properties", "sheet": i, "values": values }));
                }
            });
            ui.separator();
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button(crate::tl!("Add Drawing...")).clicked() {
                pick = Some("add");
            }
            if ui
                .button(crate::tl!("Sync Drawings"))
                .on_hover_text(crate::tl!(
                    "Each sheet's layout becomes \"number title\"; the sheet set fields, view labels and callouts take the set's current values"
                ))
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
        egui::CollapsingHeader::new(crate::tl!("Sheet Set Properties")).id_salt("sheetset-set-props").show(ui, |ui| {
            if let Some(values) = properties(ui, "set", &v["properties"], &mut st.set_key, &mut st.set_value) {
                todo = Some(json!({ "action": "properties", "values": values }));
            }
        });
        egui::CollapsingHeader::new(crate::tl!("Page Setup Override")).id_salt("sheetset-pagesetup").show(ui, |ui| {
            let names: Vec<String> = v["pageSetups"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            let current = str_of(&v["pageSetup"]).to_string();
            let own = crate::tl!("(each layout's own)");
            let shown = if current.is_empty() { own.to_string() } else { current.clone() };
            egui::ComboBox::from_id_salt("sheetset-pagesetup-use").selected_text(shown).show_ui(ui, |ui| {
                if ui.selectable_label(current.is_empty(), own).clicked() && !current.is_empty() {
                    todo = Some(json!({ "action": "pagesetup", "use": "" }));
                }
                for n in &names {
                    if ui.selectable_label(*n == current, n).clicked() && *n != current {
                        todo = Some(json!({ "action": "pagesetup", "use": n }));
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut st.setup_name).hint_text(crate::tl!("Name")).desired_width(110.0));
                let can = selected.is_some() && !st.setup_name.trim().is_empty();
                if ui
                    .add_enabled(can, egui::Button::new(crate::tl!("From Selected Sheet")))
                    .on_hover_text(crate::tl!("A page setup copied from the selected sheet's layout"))
                    .clicked()
                {
                    todo = Some(json!({ "action": "pagesetup", "name": st.setup_name.trim(), "from": selected }));
                    st.setup_name.clear();
                }
                if ui.add_enabled(!current.is_empty(), egui::Button::new(crate::tl!("Remove"))).clicked() {
                    todo = Some(json!({ "action": "pagesetup", "remove": current }));
                }
            });
        });
        egui::CollapsingHeader::new(crate::tl!("Views and Callouts")).id_salt("sheetset-views").show(ui, |ui| {
            for view in v["views"].as_array().into_iter().flatten() {
                let vid = view["id"].as_u64();
                let label = format!("{} {} — {}", str_of(&view["number"]), str_of(&view["title"]), str_of(&view["sheet"]));
                if ui.selectable_label(st.target.is_some() && st.target == vid, label).clicked() {
                    st.target = vid;
                }
            }
            let callouts = v["callouts"].as_array().map_or(0, Vec::len);
            if callouts > 0 {
                ui.label(RichText::new(crate::tl!("Callouts: {}").replace("{}", &callouts.to_string())).color(t.text_dim));
            }
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Position on sheet"));
                ui.add(egui::TextEdit::singleline(&mut st.at_x).hint_text("X").desired_width(50.0));
                ui.add(egui::TextEdit::singleline(&mut st.at_y).hint_text("Y").desired_width(50.0));
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut st.view).hint_text(crate::tl!("Named view")).desired_width(110.0));
                ui.label(crate::tl!("Scale"));
                ui.add(egui::TextEdit::singleline(&mut st.scale).desired_width(40.0));
                if ui.add_enabled(selected.is_some() && !st.view.trim().is_empty(), egui::Button::new(crate::tl!("Place View"))).clicked() {
                    let scale = st.scale.trim().parse::<f64>().ok().filter(|s| s.is_finite() && *s > 0.0).unwrap_or(1.0);
                    todo = Some(json!({ "action": "view", "sheet": selected, "view": st.view.trim(), "at": at(&st), "scale": scale }));
                }
            });
            if ui
                .add_enabled(selected.is_some() && st.target.is_some(), egui::Button::new(crate::tl!("Add Callout")))
                .on_hover_text(crate::tl!("A callout on the selected sheet pointing at the selected view"))
                .clicked()
            {
                todo = Some(json!({ "action": "callout", "sheet": selected, "view": st.target, "at": at(&st) }));
            }
        });
        egui::CollapsingHeader::new(crate::tl!("Sheet Set Fields")).id_salt("sheetset-fields").show(ui, |ui| {
            let fields = app.session.execute("sheetset", &json!({ "set": app.ui.sheet_set, "action": "fields" })).ok();
            for f in fields.as_ref().and_then(|f| f["fields"].as_array()).into_iter().flatten() {
                let name = str_of(&f["name"]);
                if ui.selectable_label(st.field == name, name).on_hover_text(str_of(&f["code"])).clicked() {
                    st.field = name.to_string();
                }
            }
            if ui
                .add_enabled(selected.is_some() && !st.field.is_empty(), egui::Button::new(crate::tl!("Insert on Selected Sheet")))
                .on_hover_text(crate::tl!("Text holding the field, at the position on sheet"))
                .clicked()
            {
                todo = Some(json!({ "action": "insertField", "sheet": selected, "field": st.field, "at": at(&st) }));
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
                app.session.echo(format!("Published {} sheets to {}.", r["pages"], str_of(&r["path"])));
                let warnings = r["warnings"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
                st.published = Some((str_of(&r["path"]).to_string(), warnings));
            }
        }
        _ => {}
    }
    if let Some(p) = todo
        && let Some(r) = act(app, p)
    {
        st.listing = Some(Ok(r));
    }
    ctx.data_mut(|d| d.insert_temp(id(), st));
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

    fn frame(app: &mut CadApp, ctx: &egui::Context) {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(app, ui));
        out.textures_delta.clear();
    }

    #[test]
    fn sheetset_docks_the_manager_and_without_a_set_says_so() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        crate::menus::run_ui_command(&mut app, "sheetset", &Value::Null);
        assert!(app.ui.dialog.is_none(), "a palette, not a dialog");
        let open = |app: &CadApp| app.ui.docking.layout.contains(&Panel::SheetSets);
        assert!(open(&app), "the palette is open");
        // SSM again keeps it open where it is.
        crate::menus::run_ui_command(&mut app, "ssm", &Value::Null);
        assert!(open(&app));
        let ctx = egui::Context::default();
        frame(&mut app, &ctx);
        let st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
        assert!(st.listing.is_none(), "no set yet");
        // A set that can't be read shows why.
        app.ui.sheet_set = Some("/no/such/folder/x.sheetset.json".into());
        ctx.data_mut(|d| d.remove::<State>(id()));
        frame(&mut app, &ctx);
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
        let mut texts = Vec::new();
        for _ in 0..2 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui));
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
