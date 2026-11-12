//! The 3D BIM view of a print set whose sheets load on demand (Q6418): its frames load no sheet.
//! The view reads its own state (`bim.view`) whenever the drawing changes; when that read loaded
//! the whole set (which changes the drawing) and the idle window put the sheets out again (which
//! changes it too), every frame loaded the set on the UI thread and started the 3D model again:
//! the view never finished preparing.

use cadaddy_engine::doc::{EntityKind, Line, RawTag, Space, geom::Vec3};
use cadaddy_engine::{Session, sample};
use cadaddy_ui_egui::{CadApp, Services, dialogs, docking::Panel};
use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;

/// A made-up print set with a few built walls on its sheets (an element carries its build's
/// extended data, so it never leaves memory), open as the interface opens one: on its first
/// sheet, the others out of memory. With it, the objects in memory.
fn on_demand() -> (Harness<'static, CadApp>, usize) {
    let mut set = sample::job_print_set(8, 300);
    for i in 0..30 {
        let x = 100.0 + f64::from(i) * 40.0;
        let line = Line { a: Vec3::new(x, 100.0, 0.0), b: Vec3::new(x + 30.0, 100.0, 0.0) };
        let h = set.add(&Space::Model, Default::default(), EntityKind::Line(line)).unwrap();
        let app = cadaddy_engine::cmd::wallgen::APP;
        set.model.modify(h, |e| {
            e.common.xdata = vec![
                RawTag { code: 1001, value: app.into() },
                RawTag { code: 1000, value: "wall".into() },
                RawTag { code: 1000, value: "LEVEL 1".into() },
            ]
            .into()
        });
    }
    let total = set.model.len();
    let mut s = Session::new();
    s.sheets_on_demand = true;
    s.open_drawing(set, "Sample Set", None);
    assert!(s.open_first_sheet());
    assert!(s.has_parked_sheets(), "the sheets it isn't showing are out of memory");
    let held = s.doc().unwrap().model.len();
    assert!(held < total);
    let mut app = CadApp::new(s, Services::default());
    app.canvas.zoom_pending = true;
    let mut h = Harness::builder().with_size(vec2(1200.0, 800.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    (h, held)
}

/// The palettes and dialogs that read sheet linework as they are drawn, so opening one loads the
/// set (once): a count and a purge look at every object, a plot preview and a transmittal at
/// everything they will write, a standards check at every object's properties, the room list
/// traces rooms from the plans when none are built, the 4D schedule counts every object not yet
/// linked, and Validate and the agent's work list check the model against the prints.
const WHOLE_SET: &[&str] = &[
    "dialog countpalette",
    "dialog purge",
    "dialog preview",
    "dialog etransmit",
    "dialog checkstandards",
    "dialog rooms",
    "dialog fourd",
    "palette validate",
    "dialog agentwork",
];

/// No palette and no dialog loads the whole set by being open unless it reads sheet linework
/// (Q6418), and none loads it more than once: what each reads on its own, as it is drawn, it
/// reads from what is in memory. A whole-set load on the UI thread is seconds on a large set;
/// when the idle window put the sheets out again while the dialog still asked every frame, each
/// frame loaded them again.
#[test]
fn an_open_palette_or_dialog_loads_the_whole_set_at_most_once_and_only_for_sheet_linework() {
    let (mut h, held) = on_demand();
    let mut items: Vec<(String, Option<String>)> = Panel::RAIL.iter().map(|p| (format!("palette {}", p.id()), None)).collect();
    items.extend(dialogs::IDS.iter().map(|id| (format!("dialog {id}"), Some((*id).to_string()))));
    let (mut needless, mut again) = (Vec::new(), Vec::new());
    for (what, dialog) in items {
        assert!(h.state().session.has_parked_sheets(), "before {what}: the sheets are out of memory");
        match &dialog {
            Some(id) => h.state_mut().ui.dialog = Some(id.clone()),
            None => {
                let panel = what.trim_start_matches("palette ").to_string();
                h.state_mut().run("ui.dock", json!({ "operation": "open", "panel": panel })).unwrap();
            }
        }
        let before = h.state().session.whole_set_loads.0;
        h.state_mut().session.whole_set_asked.clear();
        h.run_steps(12);
        let (n, last) = h.state().session.whole_set_loads.clone();
        let by = last.last().cloned().unwrap_or_default();
        // Every command it ran that reads the whole set, whether or not that one loaded it.
        let asked = h.state().session.whole_set_asked.join(", ");
        if !asked.is_empty() && !WHOLE_SET.contains(&what.as_str()) {
            needless.push(format!("{what}: {asked}"));
        }
        if n > before + 1 {
            again.push(format!("{what}: {by} × {}", n - before));
        }
        // Closed: nothing reads the set any more, and it goes out of memory again.
        match &dialog {
            Some(_) => h.state_mut().ui.dialog = None,
            None => {
                let panel = what.trim_start_matches("palette ").to_string();
                h.state_mut().run("ui.dock", json!({ "operation": "close", "panel": panel })).unwrap();
            }
        }
        // (Opened within milliseconds of each other here: no read is taken for an answer to
        // the sheets going out.)
        cadaddy_ui_egui::sheetload::forget_holds();
        h.run_steps(4);
    }
    assert!(needless.is_empty(), "opened, these ran whole-set commands and read no sheet linework:\n{}", needless.join("\n"));
    assert!(again.is_empty(), "open, these loaded the whole set again and again:\n{}", again.join("\n"));
    assert!(h.state().session.has_parked_sheets());
    assert_eq!(h.state().session.doc().unwrap().model.len(), held);
}

/// Something that reads the whole set whenever the drawing changes (a made-up palette: `entities`
/// run when the drawing isn't the one it last read) doesn't make the set load for ever: putting
/// the sheets out changes the drawing, so the second time it is read right after they went out
/// they stay in memory.
#[test]
fn a_read_that_follows_every_change_of_the_drawing_loads_the_set_twice_at_most() {
    let (mut h, _) = on_demand();
    let mut read: *const cadaddy_engine::doc::Drawing = std::ptr::null();
    for _ in 0..40 {
        let now = std::sync::Arc::as_ptr(&h.state().session.state().unwrap().doc);
        if now != read {
            h.state_mut().session.execute("entities", &json!({ "limit": 1 })).unwrap();
            read = std::sync::Arc::as_ptr(&h.state().session.state().unwrap().doc);
        }
        h.run_steps(1);
    }
    let (n, _) = h.state().session.whole_set_loads.clone();
    assert!((1..=2).contains(&n), "the set was loaded {n} times in 40 frames");
}

#[test]
fn frames_of_the_3d_bim_view_load_no_sheet_and_the_model_finishes() {
    let (mut h, held) = on_demand();
    h.state_mut().run("bim.look", json!({ "on": true })).unwrap();
    h.state_mut().run("vscurrent", json!({ "style": "shaded" })).unwrap();
    assert_eq!(h.state().session.doc().unwrap().model.len(), held, "turning the view on loaded no sheet");
    // Frames until the model (built off the UI thread) is on screen.
    let mut ready = false;
    for _ in 0..400 {
        h.run_steps(1);
        let st = &h.state().canvas.shade.stats;
        if st.building.is_none() && st.build_ms > 0.0 {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(ready, "the 3D model never finished: {:?}", h.state().canvas.shade.stats.building);
    // Still frames after it: the same drawing, nothing loaded, no new model.
    let doc = std::sync::Arc::as_ptr(&h.state().session.state().unwrap().doc);
    for _ in 0..20 {
        h.run_steps(1);
        let app = h.state();
        assert_eq!(std::sync::Arc::as_ptr(&app.session.state().unwrap().doc), doc, "a frame changed the drawing");
        assert_eq!(app.canvas.shade.stats.building, None, "a frame started the 3D model again");
    }
    assert!(h.state().session.has_parked_sheets());
    assert_eq!(h.state().session.doc().unwrap().model.len(), held, "frames of the 3D view loaded sheets");
}
