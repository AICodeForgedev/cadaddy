//! The Data Extraction wizard (DATAEXTRACTION and EATTEXT typed or from a menu; -DATAEXTRACTION
//! keeps the command line): pick the drawings, the objects and their properties, then the output
//! (a table, a spreadsheet file, saved settings), with a preview of the rows. Every step builds
//! the command's JSON parameters; Finish runs it.

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::CadApp;

/// Rows the preview shows.
const PREVIEW_ROWS: usize = 30;

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    page: usize,
    include_current: bool,
    drawings: Vec<String>,
    include_blocks: bool,
    blocks: String,
    nested: bool,
    /// Object types in the drawing, and whether each is extracted.
    types: Vec<(String, bool)>,
    properties: Vec<(&'static str, bool)>,
    tags: String,
    combine: bool,
    table: bool,
    at: [f64; 2],
    title: String,
    to_file: bool,
    file: String,
    /// The last preview: columns and rows (as text), or an error.
    preview: Option<Result<(Vec<String>, Vec<Vec<String>>), String>>,
}

const PAGES: [&str; 4] = ["Data source", "Objects", "Properties", "Output"];

/// Open the wizard, with the object types the drawing has and the view's centre for the table.
pub fn open(app: &mut CadApp) {
    let mut types: Vec<String> = Vec::new();
    let mut at = [0.0, 0.0];
    if let Ok(st) = app.session.state() {
        let c = st.view().center;
        at = [c.x, c.y];
        for e in st.doc.model.iter().chain(st.doc.layouts.iter().flat_map(|l| l.entities.iter())) {
            let t = e.kind.type_name();
            if t != "Block Reference" && !types.iter().any(|x| x == t) {
                types.push(t.to_string());
            }
            if types.len() > 64 {
                break;
            }
        }
    }
    types.sort();
    app.extract_dlg = Some(State {
        page: 0,
        include_current: true,
        drawings: Vec::new(),
        include_blocks: true,
        blocks: "*".into(),
        nested: true,
        types: types.into_iter().map(|t| (t, false)).collect(),
        properties: cadaddy_engine::EXTRACT_PROPERTIES.iter().map(|p| (*p, false)).collect(),
        tags: "*".into(),
        combine: true,
        table: true,
        at,
        title: String::new(),
        to_file: false,
        file: String::new(),
        preview: None,
    });
    app.ui.dialog = Some("dataextraction".into());
}

impl State {
    /// The command's parameters for what is chosen; with `output`, the table and file too.
    pub fn params(&self, output: bool) -> Value {
        let mut p = json!({
            "includeCurrent": self.include_current,
            "drawings": self.drawings,
            "includeBlocks": self.include_blocks,
            "blocks": self.blocks,
            "nested": self.nested,
            "types": self.types.iter().filter(|t| t.1).map(|t| t.0.clone()).collect::<Vec<_>>(),
            "properties": self.properties.iter().filter(|p| p.1).map(|p| p.0).collect::<Vec<_>>(),
            "tags": self.tags,
            "combine": self.combine,
        });
        if output {
            if self.table {
                p["at"] = json!(self.at);
                if !self.title.trim().is_empty() {
                    p["title"] = json!(self.title);
                }
            }
            if self.to_file && !self.file.trim().is_empty() {
                p["file"] = json!(self.file.trim());
            }
        }
        p
    }

    /// Fill the choices from saved settings (as the command reports them).
    fn load(&mut self, st: &Value) {
        let b = |k: &str, d: bool| st.get(k).and_then(Value::as_bool).unwrap_or(d);
        let t = |k: &str| st.get(k).and_then(Value::as_str).map(str::to_string);
        let list = |k: &str| -> Vec<String> {
            st.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        self.include_current = b("includeCurrent", true);
        self.drawings = list("drawings");
        self.include_blocks = b("includeBlocks", true);
        self.blocks = t("blocks").unwrap_or_else(|| "*".into());
        self.nested = b("nested", true);
        self.tags = t("tags").unwrap_or_else(|| "*".into());
        self.combine = b("combine", true);
        let types = list("types");
        for ty in &types {
            if !self.types.iter().any(|(n, _)| n.eq_ignore_ascii_case(ty)) {
                self.types.push((ty.clone(), false));
            }
        }
        for (n, on) in &mut self.types {
            *on = types.iter().any(|t| t.eq_ignore_ascii_case(n));
        }
        let props = list("properties");
        for (n, on) in &mut self.properties {
            *on = props.iter().any(|p| p.eq_ignore_ascii_case(n));
        }
        if let Some(f) = t("file") {
            self.file = f;
            self.to_file = true;
        }
        if let Some(f) = t("title") {
            self.title = f;
        }
    }
}

fn cells(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| a.iter().map(|c| c.as_str().map(str::to_string).or_else(|| c.as_f64().map(|n| format!("{n}"))).unwrap_or_default()).collect())
        .unwrap_or_default()
}

/// Run the extraction without output for the preview.
fn preview(app: &mut CadApp, st: &State) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let r = app.run("dataextraction", st.params(false)).map_err(|e| e.to_string())?;
    let rows = r["rows"].as_array().map(|a| a.iter().take(PREVIEW_ROWS).map(cells).collect()).unwrap_or_default();
    Ok((cells(&r["columns"]), rows))
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(mut st) = app.extract_dlg.clone() else {
        *open = false;
        return;
    };
    let mut finish = false;
    let mut cancel = false;
    let mut load: Option<String> = None;
    let mut save: Option<String> = None;
    let mut do_preview = false;
    egui::Window::new(crate::tl!("Data Extraction"))
        .id(egui::Id::new("Data Extraction"))
        .open(open)
        .collapsible(false)
        .default_size(vec2(560.0, 440.0))
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (i, name) in PAGES.iter().enumerate() {
                    let label = RichText::new(format!("{}. {}", i + 1, crate::i18n::t(name)));
                    if ui.selectable_label(st.page == i, label).clicked() {
                        st.page = i;
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| match st.page {
                0 => {
                    ui.checkbox(&mut st.include_current, crate::tl!("This drawing"));
                    ui.label(crate::tl!("Other drawings:"));
                    let mut gone = None;
                    for (i, d) in st.drawings.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.small_button("✕").on_hover_text(crate::tl!("Remove")).clicked() {
                                gone = Some(i);
                            }
                            ui.label(d);
                        });
                    }
                    if let Some(i) = gone {
                        st.drawings.remove(i);
                    }
                    ui.horizontal(|ui| {
                        if ui.button(crate::tl!("Add Drawings...")).clicked()
                            && let Some(p) = app.services.pick_open.as_ref().and_then(|f| f())
                            && !st.drawings.contains(&p)
                        {
                            st.drawings.push(p);
                        }
                        if ui.button(crate::tl!("Open Settings...")).on_hover_text(crate::tl!("Start from a saved .dxe settings file")).clicked() {
                            load = app.services.pick_open.as_ref().and_then(|f| f());
                        }
                    });
                }
                1 => {
                    ui.checkbox(&mut st.include_blocks, crate::tl!("Blocks"));
                    ui.add_enabled_ui(st.include_blocks, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(crate::tl!("Block names"));
                            ui.text_edit_singleline(&mut st.blocks);
                        });
                        ui.checkbox(&mut st.nested, crate::tl!("Include blocks inside blocks"));
                    });
                    ui.separator();
                    ui.label(crate::tl!("Other objects:"));
                    if st.types.is_empty() {
                        ui.weak(crate::tl!("The drawing has no other objects."));
                    }
                    for (name, on) in &mut st.types {
                        ui.checkbox(on, crate::i18n::t(name));
                    }
                }
                2 => {
                    ui.label(crate::tl!("Properties to extract:"));
                    egui::Grid::new("dx-props").num_columns(3).show(ui, |ui| {
                        for (i, (name, on)) in st.properties.iter_mut().enumerate() {
                            ui.checkbox(on, crate::i18n::t(name));
                            if i % 3 == 2 {
                                ui.end_row();
                            }
                        }
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(crate::tl!("Attribute tags"));
                        ui.text_edit_singleline(&mut st.tags);
                    });
                }
                _ => {
                    ui.checkbox(&mut st.combine, crate::tl!("Combine identical rows"));
                    ui.checkbox(&mut st.table, crate::tl!("Insert a table"));
                    ui.add_enabled_ui(st.table, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(crate::tl!("Insertion point"));
                            ui.add(egui::DragValue::new(&mut st.at[0]).prefix("X ").speed(1.0));
                            ui.add(egui::DragValue::new(&mut st.at[1]).prefix("Y ").speed(1.0));
                        });
                        ui.horizontal(|ui| {
                            ui.label(crate::tl!("Title"));
                            ui.text_edit_singleline(&mut st.title);
                        });
                    });
                    ui.checkbox(&mut st.to_file, crate::tl!("Write a file (.csv for spreadsheets, .txt tab-separated)"));
                    ui.add_enabled_ui(st.to_file, |ui| {
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut st.file);
                            if ui.button(crate::tl!("Browse...")).clicked()
                                && let Some(p) = app.services.pick_save.as_ref().and_then(|f| f("extraction.csv"))
                            {
                                st.file = p;
                            }
                        });
                    });
                    if ui.button(crate::tl!("Save Settings...")).on_hover_text(crate::tl!("Save these choices to a .dxe file to use again")).clicked()
                    {
                        save = app.services.pick_save.as_ref().and_then(|f| f("extraction.dxe"));
                    }
                    ui.separator();
                    if ui.button(crate::tl!("Preview")).clicked() {
                        do_preview = true;
                    }
                    match &st.preview {
                        Some(Ok((cols, rows))) => {
                            egui::ScrollArea::horizontal().id_salt("dx-preview").show(ui, |ui| {
                                egui::Grid::new("dx-preview-grid").striped(true).show(ui, |ui| {
                                    for c in cols {
                                        ui.label(RichText::new(c).strong());
                                    }
                                    ui.end_row();
                                    for r in rows {
                                        for c in r {
                                            ui.label(c);
                                        }
                                        ui.end_row();
                                    }
                                });
                            });
                        }
                        Some(Err(e)) => {
                            ui.colored_label(crate::theme::Tokens::get().warn, e);
                        }
                        None => {}
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add_enabled(st.page > 0, egui::Button::new(crate::tl!("< Back"))).clicked() {
                    st.page -= 1;
                }
                if ui.add_enabled(st.page + 1 < PAGES.len(), egui::Button::new(crate::tl!("Next >"))).clicked() {
                    st.page += 1;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(crate::tl!("Cancel")).clicked() {
                        cancel = true;
                    }
                    if ui.add_enabled(st.table || st.to_file, egui::Button::new(crate::tl!("Finish"))).clicked() {
                        finish = true;
                    }
                });
            });
        });
    if let Some(path) = load {
        match app.run("dataextraction", json!({ "settings": path })) {
            Ok(r) => st.load(&r["settings"]),
            Err(e) => st.preview = Some(Err(e.to_string())),
        }
    }
    if let Some(path) = save {
        let mut p = st.params(false);
        p["saveSettings"] = json!(path);
        if let Err(e) = app.run("dataextraction", p) {
            st.preview = Some(Err(e.to_string()));
        }
    }
    if do_preview {
        st.preview = Some(preview(app, &st));
    }
    if finish {
        match app.run("dataextraction", st.params(true)) {
            Ok(r) => {
                let n = r["rows"].as_array().map_or(0, Vec::len);
                app.session.echo(format!("{n} row{} extracted.", if n == 1 { "" } else { "s" }));
                cancel = true;
            }
            Err(e) => {
                st.page = PAGES.len() - 1;
                st.preview = Some(Err(e.to_string()));
            }
        }
    }
    if cancel || !*open {
        app.extract_dlg = None;
        *open = false;
    } else {
        app.extract_dlg = Some(st);
    }
}
