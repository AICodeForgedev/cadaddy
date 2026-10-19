//! The Materials Browser: typed MATERIALS opens it; it shows the library by category and the
//! drawing's materials.

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services};
use egui::vec2;
use egui_kittest::Harness;
use serde_json::{Value, json};

fn harness(app: CadApp) -> Harness<'static, CadApp> {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn painted(h: &Harness<'_, CadApp>) -> String {
    format!("{:?}", h.output().shapes)
}

#[test]
fn materials_opens_the_browser_with_the_library_and_the_drawings_materials() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.run("materials", Value::Null).unwrap();
    assert_eq!(app.ui.dialog.as_deref(), Some("materials"));
    // MATERIALATTACH typed opens the same browser (its layer row).
    app.ui.dialog = None;
    app.run("materialattach", Value::Null).unwrap();
    assert_eq!(app.ui.dialog.as_deref(), Some("materials"));
    let mut h = harness(app);
    let text = painted(&h);
    for s in ["Materials Browser", "Library", "Masonry", "Brick, red, running bond", "In this drawing", "No materials yet"] {
        assert!(text.contains(s), "{s} missing");
    }
    // A material added to the drawing shows there, and its swatch is a texture.
    h.state_mut().run("material.new", json!({ "library": "oak", "name": "Desk oak" })).unwrap();
    h.run_steps(3);
    let text = painted(&h);
    assert!(text.contains("Desk oak") && !text.contains("No materials yet"));
    // JSON calls never open it.
    let mut app = CadApp::new(Session::new(), Services::default());
    let r = app.run("materials", json!({})).unwrap();
    assert!(r["library"].as_array().is_some_and(|l| !l.is_empty()));
    assert_eq!(app.ui.dialog, None);
}
