//! The Rooms palette: typed ROOMS opens it, listing the plan's rooms with Go to.

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;

#[test]
fn rooms_opens_the_palette_with_the_rooms() {
    let mut app = CadApp::new(Session::new(), Services::default());
    let rect = |a: [f64; 2], b: [f64; 2]| json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true});
    app.session.execute("pline", &rect([0.0, 0.0], [300.0, 240.0])).unwrap();
    app.session.execute("pline", &rect([6.0, 6.0], [294.0, 234.0])).unwrap();
    app.session.execute("text", &json!({"at": [120, 120], "text": "KITCHEN", "height": 6})).unwrap();
    app.cmdline("rooms");
    assert_eq!(app.ui.dialog.as_deref(), Some("rooms"));
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    let painted = format!("{:?}", h.output().shapes);
    for s in ["Rooms", "KITCHEN", "Go to", "Build Volumes"] {
        assert!(painted.contains(s), "{s} missing");
    }
}
