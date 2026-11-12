//! The 3D BIM view of a print set whose sheets load on demand (Q6418): its frames load no sheet.
//! The view reads its own state (`bim.view`) whenever the drawing changes; when that read loaded
//! the whole set (which changes the drawing) and the idle window put the sheets out again (which
//! changes it too), every frame loaded the set on the UI thread and started the 3D model again:
//! the view never finished preparing.

use cadaddy_engine::doc::{EntityKind, Line, RawTag, Space, geom::Vec3};
use cadaddy_engine::{Session, sample};
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;

#[test]
fn frames_of_the_3d_bim_view_load_no_sheet_and_the_model_finishes() {
    // A made-up print set with a few built walls on its sheets (an element carries its build's
    // extended data, so it never leaves memory).
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
