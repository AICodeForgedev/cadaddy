//! The Plans to Model palette: the drawing's building spec in plain language ("L2 = sheet A112
//! at 10'-0"", "A-WALL = full height walls 4 7/8" thick (line pairs)"), each part approved or
//! edited (its JSON) or removed by a person, the model's issues with Zoom to each, and Build,
//! Rebuild and Walk. A thin view over the engine's `bspec.get`, `bspec.set`, `model.build` and
//! `model.issues`: it reads their results and acts through [`CadApp::run`].

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::docking::Panel;

/// PLANSTOMODEL (`ui.panel.planstomodel`) opens the palette, floating beside the drawing the
/// first time.
pub fn open(app: &mut CadApp) -> Result<Value, String> {
    let panel = Panel::PlansToModel;
    let first = !app.ui.docking.layout.contains(&panel) && !app.ui.docking.hidden.iter().any(|(p, _)| *p == panel);
    let r = crate::docking::command(app, false, &json!({ "operation": "open", "panel": panel.id() }))?;
    if !first {
        return Ok(r);
    }
    let area = app.canvas.rect.unwrap_or(egui::Rect::from_min_size(egui::pos2(220.0, 80.0), egui::vec2(1000.0, 800.0)));
    let (w, h) = (420.0, (area.height() - 40.0).clamp(240.0, 720.0));
    let rect = [area.right() - w - 16.0, area.top() + 24.0, w, h];
    crate::docking::command(app, false, &json!({ "operation": "float", "panel": panel.id(), "rect": rect }))
}

/// What the palette shows, read from `bspec.get` once per drawing revision.
#[derive(Clone, Debug, Default)]
struct View {
    key: (u64, u64),
    spec: Value,
    stored: bool,
    built: bool,
    parts: Vec<Part>,
    error: Option<String>,
}

#[derive(Clone, Debug)]
struct Part {
    section: String,
    key: String,
    text: String,
    approved: bool,
    /// Why `bspec.draft` guessed it (shown until approved).
    guess: Option<String>,
}

/// The issues last checked (`model.issues`), for the revision they were checked at.
#[derive(Clone, Debug, Default)]
struct Issues {
    key: (u64, u64),
    items: Vec<Value>,
}

/// The part being edited: its key and the JSON text.
#[derive(Clone, Debug, Default)]
struct Editing {
    key: String,
    text: String,
    error: Option<String>,
}

fn key(app: &CadApp) -> (u64, u64) {
    app.session.state().map_or((0, 0), |st| (st.uid, st.revision))
}

fn view(app: &mut CadApp, ui: &egui::Ui) -> View {
    let id = egui::Id::new("p2m-view");
    let k = key(app);
    if let Some(v) = ui.data(|d| d.get_temp::<View>(id)).filter(|v| v.key == k) {
        return v;
    }
    let v = match app.run("bspec.get", json!({})) {
        Ok(r) => View {
            key: k,
            stored: r["stored"].as_bool().unwrap_or(false),
            built: r["built"].as_bool().unwrap_or(false),
            parts: r["parts"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|p| Part {
                    section: p["section"].as_str().unwrap_or_default().to_string(),
                    key: p["key"].as_str().unwrap_or_default().to_string(),
                    text: p["text"].as_str().unwrap_or_default().to_string(),
                    approved: p["approved"].as_bool().unwrap_or(false),
                    guess: p["guess"].as_str().map(str::to_string),
                })
                .collect(),
            spec: r["spec"].clone(),
            error: None,
        },
        Err(e) => View { key: k, error: Some(e), ..View::default() },
    };
    ui.data_mut(|d| d.insert_temp(id, v.clone()));
    v
}

/// A part's JSON in the spec: the list item named by its key, or the whole object.
fn item(spec: &Value, key: &str) -> Value {
    match key.split_once('/') {
        Some((section, name)) => {
            let field = if section == "layers" { "layer" } else { "name" };
            spec[section]
                .as_array()
                .into_iter()
                .flatten()
                .find(|x| x[field].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
                .cloned()
                .unwrap_or(Value::Null)
        }
        None => spec[key].clone(),
    }
}

/// The `bspec.set` patch that puts an edited part back (or removes it: `edited` null).
fn patch(key: &str, old: &Value, edited: Option<Value>) -> Value {
    match key.split_once('/') {
        Some((section, name)) => {
            let field = if section == "layers" { "layer" } else { "name" };
            let mut items = Vec::new();
            let renamed = edited.as_ref().and_then(|e| e[field].as_str()).is_some_and(|n| !n.eq_ignore_ascii_case(name));
            if edited.is_none() || renamed {
                items.push(json!({ field: old[field].clone(), "remove": true }));
            }
            items.extend(edited);
            json!({ section: items })
        }
        // Materials, site and openings are replaced whole (`null` removes the site).
        None => json!({ key: edited.unwrap_or(Value::Null) }),
    }
}

/// What a click asks for, done once the palette is drawn.
enum Act {
    Set(Value),
    Draft,
    Build,
    Walk,
    Check,
    Zoom(Value),
}

fn section_title(s: &str) -> &'static str {
    match s {
        "levels" => "Levels",
        "layers" => "Layers",
        "wall_types" => "Wall Types",
        "rooms" => "Rooms",
        "families" => "Door and Window Families",
        "materials" => "Materials",
        "site" => "Site",
        "stairs" => "Stairs and Ramps",
        "exterior" => "Exterior",
        _ => "Openings",
    }
}

pub fn show(app: &mut CadApp, ui: &mut egui::Ui) {
    let t = crate::theme::Tokens::get();
    let v = view(app, ui);
    let edit_id = egui::Id::new("p2m-edit");
    let issues_id = egui::Id::new("p2m-issues");
    let mut editing: Option<Editing> = ui.data(|d| d.get_temp(edit_id));
    let mut act: Option<Act> = None;
    egui::ScrollArea::vertical().id_salt("p2m-scroll").auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if ui.add_enabled(!v.built, egui::Button::new(crate::tl!("Build"))).clicked() {
                act = Some(Act::Build);
            }
            if ui.add_enabled(v.built, egui::Button::new(crate::tl!("Rebuild"))).clicked() {
                act = Some(Act::Build);
            }
            if ui.add_enabled(v.built, egui::Button::new(crate::tl!("Walk"))).clicked() {
                act = Some(Act::Walk);
            }
            if ui.add_enabled(v.built, egui::Button::new(crate::tl!("Check"))).clicked() {
                act = Some(Act::Check);
            }
        });
        if let Some(e) = &v.error {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(e).color(t.warn));
            });
            return;
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            let n = v.parts.iter().filter(|p| p.approved).count();
            let source = if v.stored { crate::tl!("Kept in the drawing.") } else { crate::tl!("Drafted from the walk rules; not saved yet.") };
            ui.label(RichText::new(source).color(t.text_dim));
            ui.label(RichText::new(crate::tf!("{n} of {total} approved", n = n, total = v.parts.len())).color(t.text_dim));
        });
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if !v.stored
                && ui
                    .button(crate::tl!("Draft from Plan"))
                    .on_hover_text(crate::tl!("Read the plan's layers, walls, doors, rooms and notes; guesses are marked."))
                    .clicked()
            {
                act = Some(Act::Draft);
            }
            if !v.stored && ui.button(crate::tl!("Save Spec")).clicked() {
                act = Some(Act::Set(json!({})));
            }
            if v.parts.iter().any(|p| !p.approved) && ui.button(crate::tl!("Approve All")).clicked() {
                act = Some(Act::Set(json!({ "approved": v.parts.iter().map(|p| p.key.clone()).collect::<Vec<_>>() })));
            }
        });
        let mut last = String::new();
        for p in &v.parts {
            if p.section != last {
                last = p.section.clone();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new(crate::i18n::t(section_title(&p.section))).strong());
                });
            }
            ui.horizontal(|ui| {
                ui.add_space(12.0);
                let mut on = p.approved;
                if ui.checkbox(&mut on, "").on_hover_text(crate::tl!("Approved")).changed() {
                    let mut list: Vec<String> = v.parts.iter().filter(|x| x.approved && x.key != p.key).map(|x| x.key.clone()).collect();
                    if on {
                        list.push(p.key.clone());
                    }
                    act = Some(Act::Set(json!({ "approved": list })));
                }
                if let Some(why) = p.guess.as_ref().filter(|_| !p.approved) {
                    ui.label(RichText::new(crate::tl!("guess")).color(t.warn).small()).on_hover_text(why);
                }
                ui.add(egui::Label::new(RichText::new(&p.text).color(if p.approved { t.text } else { t.text_dim })).wrap());
            });
            ui.horizontal(|ui| {
                ui.add_space(36.0);
                if ui.small_button(crate::tl!("Edit")).on_hover_text(&p.key).clicked() {
                    let text = serde_json::to_string_pretty(&item(&v.spec, &p.key)).unwrap_or_default();
                    editing = Some(Editing { key: p.key.clone(), text, error: None });
                }
                if p.section != "openings" && ui.small_button(crate::tl!("Remove")).on_hover_text(&p.key).clicked() {
                    act = Some(Act::Set(patch(&p.key, &item(&v.spec, &p.key), None)));
                }
            });
            if let Some(ed) = editing.as_mut().filter(|e| e.key == p.key) {
                ui.horizontal(|ui| {
                    ui.add_space(36.0);
                    ui.add(egui::TextEdit::multiline(&mut ed.text).code_editor().desired_rows(6).desired_width(f32::INFINITY));
                });
                if let Some(e) = &ed.error {
                    ui.horizontal(|ui| {
                        ui.add_space(36.0);
                        ui.label(RichText::new(e).color(t.warn));
                    });
                }
                let mut close = false;
                ui.horizontal(|ui| {
                    ui.add_space(36.0);
                    if ui.button(crate::tl!("Apply")).clicked() {
                        match serde_json::from_str::<Value>(&ed.text) {
                            Ok(new) => act = Some(Act::Set(patch(&p.key, &item(&v.spec, &p.key), Some(new)))),
                            Err(e) => ed.error = Some(e.to_string()),
                        }
                    }
                    if ui.button(crate::tl!("Cancel")).clicked() {
                        close = true;
                    }
                });
                if close {
                    editing = None;
                }
            }
        }
        // The model's issues, as last checked.
        let issues: Option<Issues> = ui.data(|d| d.get_temp(issues_id));
        if let Some(is) = issues {
            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(crate::tf!("Issues ({n})", n = is.items.len())).strong());
                if is.key != v.key {
                    ui.label(RichText::new(crate::tl!("The drawing changed since; check again.")).color(t.text_dim));
                }
            });
            if is.items.is_empty() {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    ui.label(RichText::new(crate::tl!("No issues found.")).color(t.text_dim));
                });
            }
            for i in &is.items {
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let has_place = i.get("at").is_some() || i["handles"].as_array().is_some_and(|h| !h.is_empty());
                    if ui.add_enabled(has_place, egui::Button::new(crate::tl!("Zoom")).small()).clicked() {
                        act = Some(Act::Zoom(i.clone()));
                    }
                    ui.add(egui::Label::new(RichText::new(i["message"].as_str().unwrap_or_default()).color(t.warn)).wrap());
                });
            }
        }
    });
    ui.data_mut(|d| match &editing {
        Some(e) => {
            d.insert_temp(edit_id, e.clone());
        }
        None => d.remove::<Editing>(edit_id),
    });
    let check = |app: &mut CadApp, ui: &egui::Ui| match app.run("model.issues", json!({})) {
        Ok(r) => {
            let items = r["issues"].as_array().cloned().unwrap_or_default();
            ui.data_mut(|d| d.insert_temp(issues_id, Issues { key: key(app), items }));
        }
        Err(e) => app.set_status(e),
    };
    match act {
        Some(Act::Set(p)) => match app.run("bspec.set", json!({ "spec": p })) {
            Ok(_) => ui.data_mut(|d| d.remove::<Editing>(edit_id)),
            Err(e) => {
                if let Some(mut ed) = editing {
                    ed.error = Some(e.clone());
                    ui.data_mut(|d| d.insert_temp(edit_id, ed));
                }
                app.set_status(e);
            }
        },
        Some(Act::Draft) => {
            if let Err(e) = app.run("bspec.draft", json!({ "store": true })) {
                app.set_status(e);
            }
        }
        Some(Act::Build) => match app.run("model.build", json!({})) {
            Ok(_) => check(app, ui),
            Err(e) => app.set_status(e),
        },
        Some(Act::Walk) => {
            if let Err(e) = app.run("ui.walk", Value::Null) {
                app.set_status(e);
            }
        }
        Some(Act::Check) => check(app, ui),
        Some(Act::Zoom(i)) => zoom_to(app, &i, v.spec["units_per_foot"].as_f64().filter(|u| u.is_finite() && *u > 0.0).unwrap_or(12.0)),
        None => {}
    }
}

/// Zoom to an issue (its point, else its objects) and select its objects.
fn zoom_to(app: &mut CadApp, issue: &Value, upf: f64) {
    let handles: Vec<Value> = issue["handles"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|h| h.as_str().is_some_and(|h| app.session.doc().is_ok_and(|d| d.model.iter().any(|e| e.handle.hex() == h))))
        .cloned()
        .collect();
    let r = match issue.get("at") {
        Some(at) => app.run("zoom", json!({ "mode": "center", "center": at, "height": 30.0 * upf })),
        None if !handles.is_empty() => app.run("zoom", json!({ "mode": "object", "handles": handles })),
        None => Ok(Value::Null),
    };
    if let Err(e) = r {
        app.set_status(e);
    }
    if !handles.is_empty() {
        let _ = app.run("select", json!({ "handles": handles }));
    }
}
