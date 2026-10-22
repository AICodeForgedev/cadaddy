//! The 4D Schedule palette (Window ▸ 4D Schedule, FOURD typed alone): the drawing's construction
//! schedule (`fourd.list`), imported from CSV or Excel (`fourd.import`), and a date slider that
//! shows the model as it stands that day in the 3D view and in Walk: built work in its own
//! colours, work in progress orange, work not yet started hidden (or pale). Play steps through
//! the days; a task's elements are picked by hand from the selection (`fourd.link`); Export Video
//! renders the sequence day by day to frames and an MP4 (`fourd.export`).

use egui::{Color32, RichText};
use serde_json::{Value, json};

use crate::CadApp;
use cadaddy_walk3d::fourd::{self, Phase, Schedule4d, day, iso};

pub const DIALOG: &str = "fourd";

/// The date the model is shown at, and whether not-yet work shows pale instead of hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    /// Days since 1970-01-01.
    pub day: i32,
    pub ghost: bool,
}

/// Walk's scene as it stands on the shown date (elements of the open drawing).
pub fn stage_walk(scene: &cadaddy_walk3d::Scene, d: &cadaddy_doc::Drawing, f: Shown) -> cadaddy_walk3d::Scene {
    let Ok(sched) = Schedule4d::from_json(d.schedule4d.as_deref()) else { return scene.clone() };
    if sched.tasks.is_empty() {
        return scene.clone();
    }
    // Walk's own parts (walls, doors, devices), and the model-space objects those came from.
    let (owners, mut els) = fourd::walk_elements(scene, d);
    for e in &mut els {
        if let Some(ent) = d.entity(e.handle) {
            let m = fourd::element_of(ent);
            if m.class != "IfcBuildingElementProxy" {
                *e = m;
            }
        }
    }
    let links = sched.links(&els);
    let phases = sched.phases(&links, f.day);
    // Only the open drawing's (source 0) objects are elements.
    let by: std::collections::HashMap<cadaddy_doc::Handle, Phase> = owners.iter().zip(phases).filter_map(|(o, p)| p.map(|p| (o.handle, p))).collect();
    fourd::stage_scene(scene, f.ghost, |o| if o.src == 0 { by.get(&o.handle).copied() } else { None })
}

/// What the palette keeps between frames.
#[derive(Clone, Default)]
struct State {
    /// The drawing revision `list` was read at.
    revision: Option<u64>,
    list: Value,
    counts: Value,
    counts_for: Option<(u64, i32)>,
    selected: Option<String>,
    playing: bool,
    last_step: f64,
    /// Days a second while playing.
    speed: f64,
}

fn phase_colour(p: &str) -> Color32 {
    match p {
        "built" => Color32::from_rgb(90, 160, 90),
        "in_progress" => Color32::from_rgb(fourd::IN_PROGRESS_RGB[0], fourd::IN_PROGRESS_RGB[1], fourd::IN_PROGRESS_RGB[2]),
        _ => Color32::from_rgb(150, 155, 165),
    }
}

fn phase_label(p: Phase) -> &'static str {
    match p {
        Phase::Built => crate::tl!("Built"),
        Phase::InProgress => crate::tl!("In progress"),
        Phase::NotYet => crate::tl!("Not yet"),
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let id = egui::Id::new("fourd-palette-state");
    let mut st: State = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    if st.speed <= 0.0 {
        st.speed = 2.0;
    }
    let revision = app.session.state().map(|s| s.revision).unwrap_or(0);
    if st.revision != Some(revision) {
        st.list = app.session.execute("fourd.list", &json!({})).unwrap_or(Value::Null);
        st.revision = Some(revision);
        st.counts_for = None;
    }
    let tasks: Vec<Value> = st.list["tasks"].as_array().cloned().unwrap_or_default();
    let span = st.list["span"].as_array().and_then(|a| Some((day(a.first()?.as_str()?)?, day(a.get(1)?.as_str()?)?)));
    let mut shown = app.fourd;
    // Play: a day every 1/speed seconds, stopping after the last finish.
    if st.playing
        && let (Some((a, b)), Some(f)) = (span, shown.as_mut())
    {
        let now = ctx.input(|i| i.time);
        if now - st.last_step >= 1.0 / st.speed {
            st.last_step = now;
            if f.day > b {
                st.playing = false;
            } else {
                f.day = (f.day + 1).clamp(a - 1, b + 1);
            }
        }
        ctx.request_repaint();
    }
    if let Some(f) = shown
        && st.counts_for != Some((revision, f.day))
    {
        st.counts = app.session.execute("fourd.state", &json!({"date": iso(f.day), "handles": false})).unwrap_or(Value::Null);
        st.counts_for = Some((revision, f.day));
    }
    let mut import = false;
    let mut export = false;
    let mut link: Option<(String, bool)> = None;
    egui::Window::new(crate::tl!("4D Schedule"))
        .id(egui::Id::new("4D Schedule"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([440.0, 520.0])
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(crate::tl!("Import Schedule..."))
                    .on_hover_text(crate::tl!("CSV or Excel: task, start, finish, and level, system, class or element columns"))
                    .clicked()
                {
                    import = true;
                }
                if ui.add_enabled(span.is_some(), egui::Button::new(crate::tl!("Export Video..."))).clicked() {
                    export = true;
                }
            });
            let Some((a, b)) = span else {
                ui.label(RichText::new(crate::tl!("No schedule yet. Import one to show the model on a date.")).color(t.text_dim));
                return;
            };
            ui.label(
                RichText::new(crate::tf!(
                    "{tasks} tasks, {start} to {finish}; {linked} of {elements} objects scheduled",
                    tasks = tasks.len(),
                    start = iso(a),
                    finish = iso(b),
                    linked = st.list["linked"],
                    elements = st.list["elements"]
                ))
                .small()
                .color(t.text_dim),
            );
            ui.separator();
            let mut on = shown.is_some();
            if ui.checkbox(&mut on, crate::tl!("Show the model on a date")).changed() {
                shown = on.then_some(Shown { day: a, ghost: false });
                st.playing = false;
            }
            if let Some(f) = shown.as_mut() {
                ui.horizontal(|ui| {
                    ui.add(egui::Slider::new(&mut f.day, (a - 1)..=(b + 1)).show_value(false));
                    ui.label(RichText::new(iso(f.day)).monospace().strong());
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button(if st.playing { crate::tl!("Pause") } else { crate::tl!("Play") }).clicked() {
                        st.playing = !st.playing;
                        if st.playing && f.day > b {
                            f.day = a;
                        }
                        st.last_step = ctx.input(|i| i.time);
                    }
                    if ui.small_button("<").on_hover_text(crate::tl!("A day earlier")).clicked() {
                        f.day = (f.day - 1).max(a - 1);
                    }
                    if ui.small_button(">").on_hover_text(crate::tl!("A day later")).clicked() {
                        f.day = (f.day + 1).min(b + 1);
                    }
                    ui.add(egui::DragValue::new(&mut st.speed).range(0.25..=30.0).speed(0.1).suffix(crate::tl!(" days/s")));
                    ui.checkbox(&mut f.ghost, crate::tl!("Show work not started, pale"));
                });
                let c = &st.counts["counts"];
                ui.horizontal_wrapped(|ui| {
                    for (k, p) in [("built", Phase::Built), ("inProgress", Phase::InProgress), ("notYet", Phase::NotYet)] {
                        ui.label(RichText::new("■").color(phase_colour(p.name())));
                        ui.label(format!("{} {}", phase_label(p), c[k].as_u64().unwrap_or(0)));
                    }
                });
            }
            ui.separator();
            let phase_of: std::collections::HashMap<String, String> = st.counts["tasks"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| Some((x["id"].as_str()?.to_string(), x["phase"].as_str()?.to_string()))).collect())
                .unwrap_or_default();
            egui::ScrollArea::vertical().id_salt("fourd-tasks").max_height(240.0).show(ui, |ui| {
                for task in &tasks {
                    let tid = task["id"].as_str().unwrap_or_default().to_string();
                    ui.horizontal(|ui| {
                        let p = phase_of.get(&tid).map_or("not_yet", String::as_str);
                        ui.label(RichText::new("■").color(if shown.is_some() { phase_colour(p) } else { t.text_dim }));
                        let text = format!(
                            "{tid}  {}  ({} – {}, {})",
                            task["name"].as_str().unwrap_or_default(),
                            task["start"].as_str().unwrap_or_default(),
                            task["finish"].as_str().unwrap_or_default(),
                            task["elements"].as_u64().unwrap_or(0)
                        );
                        let picked = st.selected.as_deref() == Some(tid.as_str());
                        let empty = task["elements"].as_u64() == Some(0);
                        let rich = if empty { RichText::new(text).color(t.text_dim) } else { RichText::new(text) };
                        if ui.selectable_label(picked, rich).on_hover_text(task["filter"].as_str().unwrap_or_default()).clicked() {
                            st.selected = Some(tid.clone());
                            if let (Some(f), Some(s)) = (shown.as_mut(), task["start"].as_str().and_then(day)) {
                                f.day = s;
                            }
                        }
                    });
                }
            });
            if let Some(sel) = st.selected.clone() {
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&sel).strong());
                    if ui.button(crate::tl!("Add Selected Objects")).clicked() {
                        link = Some((sel.clone(), false));
                    }
                    if ui.button(crate::tl!("Remove Selected Objects")).clicked() {
                        link = Some((sel.clone(), true));
                    }
                });
            }
        });
    if !*open {
        shown = None;
        st.playing = false;
    }
    app.fourd = shown;
    ctx.data_mut(|d| d.insert_temp(id, st));
    if import {
        match app.services.pick_open.as_ref().map(|p| p()) {
            Some(Some(path)) => match app.run("fourd.import", json!({ "path": path })) {
                Ok(r) => app.set_status(format!("4D: {} tasks imported, {} objects scheduled.", r["tasks"], r["linked"])),
                Err(e) => app.set_status(format!("4D: {e}")),
            },
            Some(None) => {}
            None => app.set_status("4D: there is no file dialog here; run fourd.import {\"path\": \"schedule.csv\"}".to_string()),
        }
    }
    if let Some((task, remove)) = link {
        match app.run("fourd.link", json!({ "task": task, "remove": remove })) {
            Ok(r) => app.set_status(format!("4D: task {} covers {} objects.", r["task"].as_str().unwrap_or_default(), r["elements"])),
            Err(e) => app.set_status(format!("4D: {e}")),
        }
    }
    if export {
        let folder = app.services.pick_folder.as_ref().and_then(|f| f());
        if let Some(folder) = folder {
            let mp4 = std::path::Path::new(&folder).join("4D sequence.mp4").display().to_string();
            let ghost = app.fourd.is_some_and(|f| f.ghost);
            match app.run("fourd.export", json!({ "folder": folder, "mp4": mp4, "ghost": ghost })) {
                Ok(r) if r.get("mp4").is_some() => app.set_status(format!("4D: {} frames → {mp4}", r["frames"])),
                Ok(r) => app.set_status(format!(
                    "4D: {} frames in {folder}; ffmpeg isn't installed, run: {}",
                    r["frames"],
                    r["ffmpeg"].as_str().unwrap_or_default()
                )),
                Err(e) => app.set_status(format!("4D: {e}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_schedule() -> CadApp {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.run("layer.new", json!({"name": "A-WALL", "current": true})).unwrap();
        app.run("line", json!({"points": [[0, 0], [100, 0]]})).unwrap();
        app.run(
            "fourd.import",
            json!({"rows": [["Task", "Start", "Finish", "Element"], ["Walls", "2026-03-02", "2026-03-06", "A-WALL"], ["Paint", "2026-03-09", "2026-03-10", ""]]}),
        )
        .unwrap();
        app
    }

    fn frame(app: &mut CadApp) {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(app, ui.ctx()));
        out.textures_delta.clear();
    }

    #[test]
    fn fourd_opens_the_palette_and_renders() {
        let mut app = app_with_schedule();
        app.cmdline("fourd");
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        frame(&mut app);
        assert_eq!(app.fourd, None, "the model shows as drawn until a date is picked");
        app.fourd = Some(Shown { day: day("2026-03-03").unwrap_or_default(), ghost: false });
        frame(&mut app);
        assert!(app.fourd.is_some(), "the date stays while the palette is open");
        // Closed, the model shows as drawn again.
        app.ui.dialog = None;
        frame(&mut app);
    }

    #[test]
    fn the_palette_renders_in_arabic_and_with_no_schedule() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.ui.dialog = Some(DIALOG.into());
        frame(&mut app);
        let mut app = app_with_schedule();
        let _ = crate::menus::run_ui_command(&mut app, "ui.language", &json!({ "lang": "ar" }));
        app.ui.dialog = Some(DIALOG.into());
        app.fourd = Some(Shown { day: day("2026-03-09").unwrap_or_default(), ghost: true });
        frame(&mut app);
    }

    #[test]
    fn walk_shows_the_model_on_the_date() {
        use cadaddy_walk3d::{Owner, Part, Scene, Vertex};
        let mut d = cadaddy_doc::Drawing::new_imperial();
        d.schedule4d = Some(r#"{"version":1,"tasks":[{"id":"1","start":"2026-03-02","finish":"2026-03-06","add":["2A"]}]}"#.into());
        let v = Vertex { p: [0.0, 0.0, 0.0], n: [0.0, 0.0, 1.0], c: [9, 9, 9, 255] };
        let mut sc = Scene::default();
        for h in [0x2a_u64, 0x2b] {
            sc.tris.extend([v, v, v]);
            sc.owners.push(Owner { src: 0, handle: cadaddy_doc::Handle(h) });
            sc.parts.push(Part::Wall);
        }
        let at = |s: &str, ghost| stage_walk(&sc, &d, Shown { day: day(s).unwrap_or_default(), ghost });
        assert_eq!(at("2026-03-01", false).tris.len(), 3, "the picked wall isn't up yet; the other is on no task");
        assert_eq!(at("2026-03-01", true).tris.len(), 6);
        assert_eq!(at("2026-03-03", false).tris[0].c, [240, 140, 20, 255]);
        assert_eq!(at("2026-03-07", false).tris[0].c, [9, 9, 9, 255]);
    }
}
