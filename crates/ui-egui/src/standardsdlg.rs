//! The CAD standards dialogs: Configure Standards (STANDARDS: the standards files associated
//! with the drawing, Add and Remove), Check Standards (CHECKSTANDARDS: each problem with the
//! standard's names to replace it with, Fix, Fix All and Save Report) and the Layer Translator
//! (LAYTRANS: the drawing's layers mapped onto a standards file's, with the map saved and loaded).

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

/// Configure Standards.
pub fn configure(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let files: Vec<String> =
        app.session.execute("standards", &json!({})).ok().and_then(|v| serde_json::from_value(v["files"].clone()).ok()).unwrap_or_default();
    let mut action: Option<Value> = None;
    let mut check = false;
    egui::Window::new(crate::tl!("Configure Standards")).id(egui::Id::new("Configure Standards")).default_pos(crate::palette_pos(app)).open(open).resizable(true).default_width(460.0).show(ctx, |ui| {
        ui.label(RichText::new(crate::tl!("Standards files associated with the current drawing")).strong());
        egui::ScrollArea::vertical().id_salt("standards-files").max_height(200.0).show(ui, |ui| {
            if files.is_empty() {
                ui.label(RichText::new(crate::tl!("None")).color(t.text_dim));
            }
            for f in &files {
                ui.horizontal(|ui| {
                    if ui.small_button("✖").on_hover_text(crate::tl!("Remove")).clicked() {
                        action = Some(json!({ "action": "remove", "path": f }));
                    }
                    ui.label(f);
                });
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button(crate::tl!("Add...")).clicked() {
                match app.services.pick_open.as_ref().map(|p| p()) {
                    Some(Some(path)) => action = Some(json!({ "action": "add", "path": path })),
                    Some(None) => {}
                    None => app.session.echo("STANDARDS: there is no file dialog here; run it with parameters, e.g. standards {\"action\": \"add\", \"path\": \"office.dws\"}"),
                }
            }
            if ui.add_enabled(!files.is_empty(), egui::Button::new(crate::tl!("Check Standards..."))).clicked() {
                check = true;
            }
        });
    });
    if let Some(p) = action {
        let _ = app.run("standards", p);
    }
    if check {
        *open = false;
        app.ui.dialog = Some("checkstandards".into());
    }
}

#[derive(Clone, Default)]
struct CheckState {
    /// The last check, or why it failed.
    result: Option<Result<Value, String>>,
    /// The replacement chosen per problem id.
    choice: Vec<(String, String)>,
}

fn check_id() -> egui::Id {
    egui::Id::new("cadaddy-checkstandards-dialog")
}

/// Check Standards.
pub fn check(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let mut st: CheckState = ctx.data(|d| d.get_temp(check_id())).unwrap_or_default();
    if st.result.is_none() {
        st.result = Some(app.session.execute("checkstandards", &json!({})).map_err(|e| e.user_message()));
    }
    let mut run: Option<Value> = None;
    let mut report = false;
    egui::Window::new(crate::tl!("Check Standards"))
        .id(egui::Id::new("Check Standards"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_width(560.0)
        .show(ctx, |ui| match &st.result {
            Some(Ok(v)) => {
                let problems = v["problems"].as_array().cloned().unwrap_or_default();
                if problems.is_empty() {
                    ui.label(crate::tl!("No standards violations were found."));
                } else {
                    ui.label(RichText::new(format!("{} {}", problems.len(), crate::tl!("problem(s)"))).strong());
                }
                egui::ScrollArea::vertical().id_salt("standards-problems").max_height(320.0).show(ui, |ui| {
                    for p in &problems {
                        let id = p["id"].as_str().unwrap_or_default().to_string();
                        ui.horizontal(|ui| {
                            let kind = match p["kind"].as_str().unwrap_or_default() {
                                "layer" => crate::tl!("Layer"),
                                "linetype" => crate::tl!("Linetype"),
                                "style" => crate::tl!("Text Style"),
                                _ => crate::tl!("Dimension Style"),
                            };
                            ui.label(RichText::new(kind).color(t.text_dim));
                            ui.label(RichText::new(p["name"].as_str().unwrap_or_default()).strong());
                            ui.label(if p["standard"].is_null() {
                                crate::tl!("Not in the standards")
                            } else {
                                crate::tl!("Properties differ from the standards")
                            });
                        });
                        ui.horizontal(|ui| {
                            let names: Vec<String> = serde_json::from_value(p["replacements"].clone()).unwrap_or_default();
                            if names.is_empty() {
                                if ui.button(crate::tl!("Fix")).on_hover_text(crate::tl!("Take the standard's properties")).clicked() {
                                    run = Some(json!({ "fix": [{ "id": id }] }));
                                }
                            } else {
                                let mut chosen = st.choice.iter().find(|(i, _)| *i == id).map(|(_, n)| n.clone()).unwrap_or_else(|| names[0].clone());
                                egui::ComboBox::from_id_salt(("standards-replace", &id)).selected_text(&chosen).show_ui(ui, |ui| {
                                    for n in &names {
                                        ui.selectable_value(&mut chosen, n.clone(), n);
                                    }
                                });
                                st.choice.retain(|(i, _)| *i != id);
                                st.choice.push((id.clone(), chosen.clone()));
                                if ui.button(crate::tl!("Fix")).on_hover_text(crate::tl!("Replace it with the standard one")).clicked() {
                                    run = Some(json!({ "fix": [{ "id": id, "replaceWith": chosen }] }));
                                }
                            }
                        });
                        ui.separator();
                    }
                });
                ui.horizontal(|ui| {
                    let differs = problems.iter().any(|p| p["replacements"].as_array().is_none_or(Vec::is_empty));
                    if ui
                        .add_enabled(differs, egui::Button::new(crate::tl!("Fix All")))
                        .on_hover_text(crate::tl!("Give everything that differs the standard's properties"))
                        .clicked()
                    {
                        run = Some(json!({ "fixAll": true }));
                    }
                    if ui.button(crate::tl!("Save Report...")).clicked() {
                        report = true;
                    }
                    if ui.button(crate::tl!("Check Again")).clicked() {
                        run = Some(json!({}));
                    }
                });
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e).color(t.warn));
            }
            None => {}
        });
    // A fix that fails is told on the command line; the list stays.
    if let Some(p) = run
        && let Ok(v) = app.run("checkstandards", p)
    {
        st.result = Some(Ok(v));
    }
    if report {
        match app.services.pick_save.as_ref().map(|pick| pick("standards report.html")) {
            Some(Some(path)) => {
                if app.run("checkstandards", json!({ "report": path })).is_ok() {
                    app.session.echo(format!("Standards report saved to {path}."));
                }
            }
            Some(None) => {}
            None => app
                .session
                .echo("CHECKSTANDARDS: there is no file dialog here; run it with parameters, e.g. checkstandards {\"report\": \"report.html\"}"),
        }
    }
    if *open {
        ctx.data_mut(|d| d.insert_temp(check_id(), st));
    } else {
        ctx.data_mut(|d| d.remove::<CheckState>(check_id()));
    }
}

#[derive(Clone, Default)]
struct TransState {
    /// The drawing the layers are translated to.
    standards: Option<String>,
    targets: Vec<String>,
    from: Option<String>,
    to: Option<String>,
    map: Vec<(String, String)>,
    error: Option<String>,
}

fn trans_id() -> egui::Id {
    egui::Id::new("cadaddy-laytrans-dialog")
}

fn map_json(map: &[(String, String)]) -> Value {
    json!(map.iter().map(|(f, t)| json!({ "from": f, "to": t })).collect::<Vec<_>>())
}

/// The Layer Translator.
pub fn laytrans(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let mut st: TransState = ctx.data(|d| d.get_temp(trans_id())).unwrap_or_default();
    let layers: Vec<String> =
        app.session.doc().map(|d| d.layers.iter().map(|l| l.name.clone()).filter(|n| !n.contains('|')).collect()).unwrap_or_default();
    let (mut load_std, mut load_map, mut save_map, mut translate) = (false, false, false, false);
    egui::Window::new(crate::tl!("Layer Translator"))
        .id(egui::Id::new("Layer Translator"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_width(560.0)
        .show(ctx, |ui| {
            ui.columns(2, |cols| {
                cols[0].label(RichText::new(crate::tl!("Translate From")).strong());
                egui::ScrollArea::vertical().id_salt("laytrans-from").max_height(220.0).show(&mut cols[0], |ui| {
                    for l in &layers {
                        let mapped = st.map.iter().any(|(f, _)| f == l);
                        let text = if mapped { RichText::new(l).color(t.text_dim) } else { RichText::new(l) };
                        if ui.selectable_label(st.from.as_deref() == Some(l.as_str()), text).clicked() {
                            st.from = Some(l.clone());
                        }
                    }
                });
                cols[1].label(RichText::new(crate::tl!("Translate To")).strong());
                egui::ScrollArea::vertical().id_salt("laytrans-to").max_height(220.0).show(&mut cols[1], |ui| {
                    if st.targets.is_empty() {
                        ui.label(RichText::new(crate::tl!("Load a drawing whose layers to translate to.")).color(t.text_dim));
                    }
                    for l in &st.targets {
                        if ui.selectable_label(st.to.as_deref() == Some(l.as_str()), l).clicked() {
                            st.to = Some(l.clone());
                        }
                    }
                });
                if cols[1].button(crate::tl!("Load...")).clicked() {
                    load_std = true;
                }
            });
            ui.horizontal(|ui| {
                let ready = st.from.is_some() && st.to.is_some();
                if ui.add_enabled(ready, egui::Button::new(crate::tl!("Map"))).clicked()
                    && let (Some(f), Some(to)) = (st.from.clone(), st.to.clone())
                {
                    st.map.retain(|(m, _)| *m != f);
                    st.map.push((f, to));
                }
            });
            ui.separator();
            ui.label(RichText::new(crate::tl!("Layer Translation Mappings")).strong());
            let mut remove = None;
            egui::ScrollArea::vertical().id_salt("laytrans-map").max_height(160.0).show(ui, |ui| {
                for (i, (f, to)) in st.map.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("✖").on_hover_text(crate::tl!("Remove")).clicked() {
                            remove = Some(i);
                        }
                        ui.label(format!("{f}  →  {to}"));
                    });
                }
            });
            if let Some(i) = remove {
                st.map.remove(i);
            }
            if let Some(e) = &st.error {
                ui.label(RichText::new(e).color(t.warn));
            }
            ui.horizontal(|ui| {
                load_map = ui.button(crate::tl!("Load Map...")).clicked();
                save_map = ui.add_enabled(!st.map.is_empty(), egui::Button::new(crate::tl!("Save Map..."))).clicked();
                translate = ui.add_enabled(!st.map.is_empty(), egui::Button::new(crate::tl!("Translate"))).clicked();
            });
        });
    let no_dialog = |app: &mut CadApp| {
        app.session
            .echo("LAYTRANS: there is no file dialog here; run it with parameters, e.g. laytrans {\"map\": [{\"from\": \"A\", \"to\": \"B\"}]}")
    };
    if load_std {
        match app.services.pick_open.as_ref().map(|p| p()) {
            Some(Some(path)) => match app.session.execute("laytrans", &json!({ "standards": path })) {
                Ok(v) => {
                    st.targets = serde_json::from_value(v["targets"].clone()).unwrap_or_default();
                    st.standards = Some(path);
                    st.to = None;
                    st.error = None;
                }
                Err(e) => st.error = Some(e.user_message()),
            },
            Some(None) => {}
            None => no_dialog(app),
        }
    }
    if load_map {
        match app.services.pick_open.as_ref().map(|p| p()) {
            Some(Some(path)) => match app.session.execute("laytrans", &json!({ "load": path })) {
                Ok(v) => {
                    st.map = v["map"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|m| Some((m["from"].as_str()?.to_string(), m["to"].as_str()?.to_string()))).collect())
                        .unwrap_or_default();
                    st.error = None;
                }
                Err(e) => st.error = Some(e.user_message()),
            },
            Some(None) => {}
            None => no_dialog(app),
        }
    }
    if save_map {
        match app.services.pick_save.as_ref().map(|pick| pick("layer map.json")) {
            Some(Some(path)) => {
                if let Err(e) = app.session.execute("laytrans", &json!({ "map": map_json(&st.map), "save": path, "translate": false })) {
                    st.error = Some(e.user_message());
                }
            }
            Some(None) => {}
            None => no_dialog(app),
        }
    }
    if translate {
        let mut p = json!({ "map": map_json(&st.map) });
        if let Some(s) = &st.standards {
            p["standards"] = json!(s);
        }
        match app.run("laytrans", p) {
            Ok(v) => {
                app.session.echo(format!("LAYTRANS: {} layer(s) translated.", v["translated"]));
                st.map.clear();
                st.from = None;
                st.error = None;
            }
            Err(e) => st.error = Some(e),
        }
    }
    if *open {
        ctx.data_mut(|d| d.insert_temp(trans_id(), st));
    } else {
        ctx.data_mut(|d| d.remove::<TransState>(trans_id()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(app: &mut CadApp, ctx: &egui::Context) {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(app, ui.ctx()));
        out.textures_delta.clear();
    }

    #[test]
    fn the_menus_open_the_standards_dialogs() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.run("line", json!({ "points": [[0, 0], [1, 1]] })).unwrap();
        let ctx = egui::Context::default();
        for id in ["standards", "checkstandards", "laytrans"] {
            crate::menus::run_ui_command(&mut app, id, &Value::Null);
            assert_eq!(app.ui.dialog.as_deref(), Some(id));
            frame(&mut app, &ctx);
        }
        // Nothing associated: Check Standards says so.
        app.ui.dialog = Some("checkstandards".into());
        frame(&mut app, &ctx);
        let st: CheckState = ctx.data(|d| d.get_temp(check_id())).unwrap_or_default();
        assert!(matches!(st.result, Some(Err(_))));
    }
}
