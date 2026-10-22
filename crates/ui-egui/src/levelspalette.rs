//! The Levels palette (Window ▸ Levels, LEVELS typed alone): the building's levels lowest first
//! with their elevations, floor to floor and ceiling heights (edited in place: `levels.set`), the
//! datums above them (the roof, a parapet), the sheets on each level, and one storey at a time in
//! 3D (`levels.isolate`). Read from Prints (`levels.read`), Attach Sheets (`levels.sheets`) and
//! Check Shafts (`levels.shafts`) act on the drawing.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "levels";

/// What the palette keeps between frames.
#[derive(Clone, Default)]
struct State {
    /// The level being edited and its fields as typed: name, elevation, height, ceiling.
    editing: Option<(String, [String; 4])>,
    isolated: Option<String>,
    /// The last action's result, in words.
    report: Vec<String>,
}

/// Feet as typed: `14'-6"`, `14' 6"`, `14.5`.
fn parse_feet(t: &str) -> Option<f64> {
    let t = t.trim();
    t.parse::<f64>().ok().filter(|v| v.is_finite()).or_else(|| cadaddy_walk3d::levels::parse_elevation(t).map(|e| e.0))
}

enum Act {
    Read,
    Sheets,
    Shafts,
    Isolate(Option<String>),
    Save(String, [String; 4]),
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let id = egui::Id::new("levels-palette-state");
    let mut st: State = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let r = app.session.execute("levels.list", &json!({})).unwrap_or(Value::Null);
    let levels: Vec<Value> = r["levels"].as_array().cloned().unwrap_or_default();
    let datums: Vec<Value> = r["datums"].as_array().cloned().unwrap_or_default();
    let mut act: Option<Act> = None;
    egui::Window::new(crate::tl!("Levels"))
        .id(egui::Id::new("Levels"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([440.0, 460.0])
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(crate::tl!("Read from Prints"))
                    .on_hover_text(crate::tl!("Level names and elevations from the sections, elevations and level markers"))
                    .clicked()
                {
                    act = Some(Act::Read);
                }
                if ui.button(crate::tl!("Attach Sheets")).clicked() {
                    act = Some(Act::Sheets);
                }
                if ui
                    .button(crate::tl!("Check Shafts"))
                    .on_hover_text(crate::tl!("Shafts, chases and risers must carry on through the levels between their ends"))
                    .clicked()
                {
                    act = Some(Act::Shafts);
                }
            });
            ui.separator();
            if levels.is_empty() {
                ui.label(RichText::new(crate::tl!("No levels in the building spec.")).color(t.text_dim));
            }
            egui::Grid::new("levels-grid").striped(true).num_columns(6).show(ui, |ui| {
                for h in [crate::tl!("Level"), crate::tl!("Elevation"), crate::tl!("Floor to floor"), crate::tl!("Ceiling"), crate::tl!("Sheets"), ""]
                {
                    ui.label(RichText::new(h).small().color(t.text_dim));
                }
                ui.end_row();
                // Highest first, as a section reads.
                for l in levels.iter().rev() {
                    let name = l["name"].as_str().unwrap_or_default().to_string();
                    let ft = |k: &str| cadaddy_walk3d::feet(l[k].as_f64().unwrap_or(f64::NAN));
                    let sheets: Vec<&str> = l["sheets"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                    match &mut st.editing {
                        Some((who, f)) if *who == name => {
                            for (i, w) in [90.0, 70.0, 70.0, 60.0].into_iter().enumerate() {
                                if let Some(s) = f.get_mut(i) {
                                    ui.add(egui::TextEdit::singleline(s).desired_width(w));
                                }
                            }
                            ui.label(sheets.join(", "));
                            ui.horizontal(|ui| {
                                if ui.small_button(crate::tl!("OK")).clicked() {
                                    act = Some(Act::Save(name.clone(), f.clone()));
                                }
                                if ui.small_button(crate::tl!("Cancel")).clicked() {
                                    act = Some(Act::Isolate(st.isolated.clone()));
                                }
                            });
                        }
                        _ => {
                            ui.label(RichText::new(&name).strong());
                            ui.label(ft("elevation_ft"));
                            ui.label(ft("height_ft"));
                            ui.label(ft("ceiling_ft"));
                            ui.label(RichText::new(sheets.join(", ")).small());
                            ui.horizontal(|ui| {
                                if ui.small_button(crate::tl!("Edit")).clicked() {
                                    st.editing = Some((name.clone(), [name.clone(), ft("elevation_ft"), ft("height_ft"), ft("ceiling_ft")]));
                                }
                                let on = st.isolated.as_deref() == Some(name.as_str());
                                if ui.selectable_label(on, crate::tl!("Isolate")).on_hover_text(crate::tl!("Show only this storey in 3D")).clicked() {
                                    act = Some(Act::Isolate(if on { None } else { Some(name.clone()) }));
                                }
                            });
                        }
                    }
                    ui.end_row();
                }
            });
            if !datums.is_empty() {
                ui.separator();
                ui.label(RichText::new(crate::tl!("Datums (no plan on this drawing):")).small().color(t.text_dim));
                for d in datums.iter().rev() {
                    ui.label(format!(
                        "{}  {}",
                        d["name"].as_str().unwrap_or_default(),
                        cadaddy_walk3d::feet(d["elevation_ft"].as_f64().unwrap_or(f64::NAN))
                    ));
                }
            }
            let loose: Vec<&str> = r["unattached"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            if !loose.is_empty() {
                ui.label(RichText::new(format!("{} {}", crate::tl!("Sheets on no level:"), loose.join(", "))).small().color(t.text_dim));
            }
            if st.isolated.is_some() && ui.button(crate::tl!("Show All Levels")).clicked() {
                act = Some(Act::Isolate(None));
            }
            if !st.report.is_empty() {
                ui.separator();
                egui::ScrollArea::vertical().id_salt("levels-report").max_height(140.0).show(ui, |ui| {
                    for line in &st.report {
                        ui.label(RichText::new(line).small());
                    }
                });
            }
        });
    match act {
        Some(Act::Read) => {
            st.report = match app.run("levels.read", json!({ "apply": true })) {
                Ok(v) => {
                    let mut out: Vec<String> = v["changes"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
                    out.extend(v["conflicts"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from));
                    if let Some(m) = v["message"].as_str() {
                        out.push(m.to_string());
                    }
                    if out.is_empty() {
                        out.push(crate::tl!("The levels already match the prints.").to_string());
                    }
                    out
                }
                Err(e) => vec![e.to_string()],
            };
        }
        Some(Act::Sheets) => {
            let folder = app.services.pick_folder.as_ref().and_then(|f| f());
            let mut p = json!({ "apply": true });
            if let Some(f) = folder {
                p["folder"] = json!(f);
            }
            st.report = match app.run("levels.sheets", p) {
                Ok(v) => {
                    let sheets = v["sheets"].as_array().map_or(0, Vec::len);
                    let loose = v["unattached"].as_array().map_or(0, Vec::len);
                    vec![crate::tf!("{sheets} sheets, {loose} on no level", sheets = sheets, loose = loose)]
                }
                Err(e) => vec![e.to_string()],
            };
        }
        Some(Act::Shafts) => {
            st.report = match app.run("levels.shafts", json!({})) {
                Ok(v) => {
                    let mut out: Vec<String> = v["runs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|r| r["missing"].as_array().is_some_and(|m| !m.is_empty()))
                        .map(|r| {
                            let miss: Vec<&str> = r["missing"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                            crate::tf!("{name}: not shown on {levels}", name = r["name"].as_str().unwrap_or_default(), levels = miss.join(", "))
                        })
                        .collect();
                    if out.is_empty() {
                        out.push(crate::tl!("Every shaft and riser carries on through its levels.").to_string());
                    }
                    out
                }
                Err(e) => vec![e.to_string()],
            };
        }
        Some(Act::Isolate(level)) => {
            st.editing = None;
            if level != st.isolated {
                let p = level.as_ref().map_or(json!({}), |l| json!({ "level": l }));
                match app.run("levels.isolate", p) {
                    Ok(_) => st.isolated = level,
                    Err(e) => st.report = vec![e.to_string()],
                }
            }
        }
        Some(Act::Save(name, f)) => {
            let mut p = json!({ "name": name });
            let [new_name, elev, height, ceiling] = &f;
            if new_name.trim() != name {
                p["rename"] = json!(new_name.trim());
            }
            for (k, v) in [("elevation_ft", elev), ("height_ft", height), ("ceiling_ft", ceiling)] {
                match parse_feet(v) {
                    Some(x) => p[k] = json!(x),
                    None => {
                        st.report = vec![crate::tf!("Not a height: {text}", text = v)];
                        ctx.data_mut(|d| d.insert_temp(id, st));
                        return;
                    }
                }
            }
            match app.run("levels.set", p) {
                Ok(_) => st.editing = None,
                Err(e) => st.report = vec![e.to_string()],
            }
        }
        None => {}
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_levels() -> CadApp {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        let spec = json!({
            "version": 1, "units_per_foot": 12,
            "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}, {"name": "2", "elevation_ft": 12, "height_ft": 12}],
            "datums": [{"name": "ROOF", "elevation_ft": 24, "kind": "roof"}],
            "sheets": [{"number": "M-201", "level": "2"}, {"number": "M-601"}]
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
    fn levels_opens_the_palette_and_renders() {
        let mut app = app_with_levels();
        app.cmdline("levels");
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        frame(&mut app);
        frame(&mut app);
    }

    #[test]
    fn the_palette_renders_in_arabic_and_with_no_spec() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.ui.dialog = Some(DIALOG.into());
        frame(&mut app);
        let mut app = app_with_levels();
        let _ = crate::menus::run_ui_command(&mut app, "ui.language", &json!({ "lang": "ar" }));
        app.ui.dialog = Some(DIALOG.into());
        frame(&mut app);
    }

    #[test]
    fn heights_parse_as_typed() {
        assert_eq!(parse_feet("14.5"), Some(14.5));
        assert_eq!(parse_feet("14'-6\""), Some(14.5));
        assert_eq!(parse_feet("abc"), None);
    }
}
