//! The Publish dialog (PUBLISH typed or from the menu): the sheet list — this drawing's sheets,
//! and others added from other drawings — with each sheet's bookmark title, paper and
//! orientation overrides, reorder and remove, the list saved and loaded (PUBLISHLIST), and
//! Publish to one PDF with a bookmark per sheet; and batch plotting a folder of drawings to one
//! PDF each (into a `PDF` folder inside it), with or without the plot stamp.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

#[derive(Clone, Debug, Default, PartialEq)]
struct Row {
    drawing: Option<String>,
    layout: String,
    title: String,
    include: bool,
    /// "" = the sheet's own page setup.
    paper: String,
    /// None = the page setup's orientation.
    landscape: Option<bool>,
}

#[derive(Clone, Default)]
struct State {
    rows: Vec<Row>,
    loaded: bool,
    /// Batch plot: stamp every sheet.
    stamp: bool,
}

fn id() -> egui::Id {
    egui::Id::new("cadaddy-publish-dialog")
}

fn rows_from(v: &Value) -> Vec<Row> {
    v["sheets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| Row {
            drawing: s["drawing"].as_str().map(str::to_string),
            layout: s["layout"].as_str().unwrap_or_default().to_string(),
            title: s["title"].as_str().unwrap_or_default().to_string(),
            include: true,
            paper: s["page"]["paper"].as_str().unwrap_or_default().to_string(),
            landscape: s["page"]["landscape"].as_bool(),
        })
        .collect()
}

fn sheets_json(rows: &[Row], only_included: bool) -> Value {
    let list: Vec<Value> = rows
        .iter()
        .filter(|r| r.include || !only_included)
        .map(|r| {
            let mut v = json!({ "layout": r.layout });
            if let Some(d) = &r.drawing {
                v["drawing"] = json!(d);
            }
            if !r.title.trim().is_empty() {
                v["title"] = json!(r.title.trim());
            }
            let mut page = serde_json::Map::new();
            if !r.paper.is_empty() {
                page.insert("paper".into(), json!(r.paper));
            }
            if let Some(l) = r.landscape {
                page.insert("landscape".into(), json!(l));
            }
            if !page.is_empty() {
                v["page"] = Value::Object(page);
            }
            v
        })
        .collect();
    Value::Array(list)
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
    if !st.loaded {
        st.loaded = true;
        if let Ok(v) = app.session.execute("publishlist", &json!({ "action": "layouts" })) {
            st.rows = rows_from(&v);
        }
    }
    let title = app.session.state().map(|s| s.title.clone()).unwrap_or_default();
    let stem = std::path::Path::new(&title)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Drawing".into());
    let mut action: Option<&'static str> = None;
    egui::Window::new(crate::tl!("Publish")).id(egui::Id::new("Publish")).open(open).resizable(true).default_width(640.0).show(ctx, |ui| {
        ui.label(RichText::new(crate::tl!("Sheets to publish, in order")).strong());
        let mut swap: Option<(usize, usize)> = None;
        let mut remove: Option<usize> = None;
        let n = st.rows.len();
        egui::ScrollArea::vertical().id_salt("publish-rows").max_height(320.0).show(ui, |ui| {
            egui::Grid::new("publish-grid").num_columns(6).striped(true).show(ui, |ui| {
                ui.label("");
                ui.label(RichText::new(crate::tl!("Sheet")).color(t.text_dim));
                ui.label(RichText::new(crate::tl!("Bookmark")).color(t.text_dim));
                ui.label(RichText::new(crate::tl!("Paper")).color(t.text_dim));
                ui.label(RichText::new(crate::tl!("Orientation")).color(t.text_dim));
                ui.label("");
                ui.end_row();
                for (i, r) in st.rows.iter_mut().enumerate() {
                    ui.checkbox(&mut r.include, "");
                    let from = r.drawing.as_deref().and_then(|d| std::path::Path::new(d).file_stem()).map(|f| f.to_string_lossy().into_owned());
                    let name = match &from {
                        Some(f) => format!("{f} - {}", r.layout),
                        None => r.layout.clone(),
                    };
                    ui.label(name).on_hover_text(r.drawing.clone().unwrap_or_else(|| crate::tl!("This drawing").to_string()));
                    ui.add(egui::TextEdit::singleline(&mut r.title).hint_text(crate::tl!("(sheet name)")).desired_width(150.0));
                    let own = crate::tl!("Page setup").to_string();
                    egui::ComboBox::from_id_salt(("publish-paper", i))
                        .width(150.0)
                        .selected_text(if r.paper.is_empty() { own.clone() } else { r.paper.clone() })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut r.paper, String::new(), own.as_str());
                            for p in cadaddy_render::PAPER_SIZES {
                                ui.selectable_value(&mut r.paper, p.name.to_string(), p.name);
                            }
                        });
                    let orient = match r.landscape {
                        None => crate::tl!("Page setup"),
                        Some(true) => crate::tl!("Landscape"),
                        Some(false) => crate::tl!("Portrait"),
                    };
                    egui::ComboBox::from_id_salt(("publish-orient", i)).selected_text(orient).show_ui(ui, |ui| {
                        ui.selectable_value(&mut r.landscape, None, crate::tl!("Page setup"));
                        ui.selectable_value(&mut r.landscape, Some(false), crate::tl!("Portrait"));
                        ui.selectable_value(&mut r.landscape, Some(true), crate::tl!("Landscape"));
                    });
                    ui.horizontal(|ui| {
                        if ui.add_enabled(i > 0, egui::Button::new("▲")).clicked() {
                            swap = Some((i, i - 1));
                        }
                        if ui.add_enabled(i + 1 < n, egui::Button::new("▼")).clicked() {
                            swap = Some((i, i + 1));
                        }
                        if ui.button("✕").on_hover_text(crate::tl!("Remove Sheet")).clicked() {
                            remove = Some(i);
                        }
                    });
                    ui.end_row();
                }
            });
        });
        if let Some((a, b)) = swap {
            st.rows.swap(a, b);
        }
        if let Some(i) = remove.filter(|i| *i < st.rows.len()) {
            st.rows.remove(i);
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(crate::tl!("Add Sheets from Drawing...")).clicked() {
                action = Some("add");
            }
            if ui.button(crate::tl!("Load List...")).clicked() {
                action = Some("load");
            }
            if ui.button(crate::tl!("Save List...")).clicked() {
                action = Some("save");
            }
        });
        ui.add_space(4.0);
        let any = st.rows.iter().any(|r| r.include);
        if ui.add_enabled(any, egui::Button::new(RichText::new(crate::tl!("Publish to PDF...")).strong())).clicked() {
            action = Some("publish");
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .button(crate::tl!("Plot a Folder of Drawings..."))
                .on_hover_text(crate::tl!("Every drawing in a folder to its own PDF, in a PDF folder inside it"))
                .clicked()
            {
                action = Some("folder");
            }
            ui.checkbox(&mut st.stamp, crate::tl!("Plot stamp"));
        });
    });
    match action {
        Some("add") => {
            if let Some(path) = app.services.pick_open.as_ref().and_then(|p| p())
                && let Ok(v) = app.run("publishlist", json!({ "action": "layouts", "drawing": path }))
            {
                st.rows.extend(rows_from(&v));
            }
        }
        Some("load") => {
            if let Some(path) = app.services.pick_open.as_ref().and_then(|p| p())
                && let Ok(v) = app.run("publishlist", json!({ "action": "load", "path": path }))
            {
                st.rows = rows_from(&v);
            }
        }
        Some("save") => {
            if let Some(path) = app.services.pick_save.as_ref().and_then(|p| p(&format!("{stem}.publish.json"))) {
                let _ = app.run("publishlist", json!({ "action": "save", "path": path, "sheets": sheets_json(&st.rows, false) }));
            }
        }
        Some("folder") => {
            if let Some(dir) = app.services.pick_folder.as_ref().and_then(|p| p()) {
                let out = std::path::Path::new(&dir).join("PDF").to_string_lossy().into_owned();
                let mut p = json!({ "folder": dir, "out": out });
                if st.stamp {
                    p["stamp"] = json!(true);
                }
                let _ = app.run("publish", p);
            }
        }
        Some("publish") => {
            if let Some(path) = app.services.pick_save.as_ref().and_then(|p| p(&format!("{stem}.pdf")))
                && app.run("publish", json!({ "sheets": sheets_json(&st.rows, true), "single": true, "path": path })).is_ok()
            {
                *open = false;
            }
        }
        _ => {}
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
    fn the_list_round_trips_through_its_json() {
        let rows = vec![
            Row { drawing: None, layout: "Layout1".into(), title: String::new(), include: true, paper: String::new(), landscape: None },
            Row {
                drawing: Some("/x/b.dwg".into()),
                layout: "Plan".into(),
                title: "A-1".into(),
                include: false,
                paper: "ISO A3".into(),
                landscape: Some(true),
            },
        ];
        let all = sheets_json(&rows, false);
        assert_eq!(
            rows_from(&json!({ "sheets": all })).iter().map(|r| (r.layout.clone(), r.paper.clone(), r.landscape)).collect::<Vec<_>>(),
            vec![("Layout1".to_string(), String::new(), None), ("Plan".to_string(), "ISO A3".to_string(), Some(true))]
        );
        assert_eq!(sheets_json(&rows, true).as_array().map_or(0, Vec::len), 1, "only the included ones are published");
    }

    #[test]
    fn publish_opens_the_dialog_with_this_drawings_sheets() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.cmdline("PUBLISH");
        assert_eq!(app.ui.dialog.as_deref(), Some("publish"));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        let st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
        assert_eq!(st.rows.iter().map(|r| r.layout.as_str()).collect::<Vec<_>>(), ["Layout1", "Layout2"]);
    }
}
