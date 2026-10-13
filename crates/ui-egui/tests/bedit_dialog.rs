//! The Edit Block Definition dialog: BEDIT typed or from a menu with no block reference selected
//! opens it; OK opens the Block Editor on the block named, or on a new block for a new name.

use cadcraft_engine::Session;
use cadcraft_ui_egui::{CadApp, Services, bedit};
use serde_json::json;

fn app_with_blocks() -> CadApp {
    let mut app = CadApp::new(Session::new(), Services::default());
    for (name, y) in [("Window", 0.0), ("door", 5.0)] {
        let l = app.run("line", json!({ "points": [[0, y], [2, y]] })).unwrap()["handles"][0].clone();
        app.run("block", json!({ "name": name, "base": [0, y], "handles": [l], "description": format!("{name} symbol") })).unwrap();
    }
    app.session.set_selection(Vec::new());
    app
}

/// The text painted in one frame.
fn painted(app: &mut CadApp, ctx: &egui::Context) -> String {
    fn collect(shape: &egui::Shape, text: &mut String) {
        match shape {
            egui::Shape::Text(s) => {
                text.push_str(s.galley.text());
                text.push('\n');
            }
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, text)),
            _ => {}
        }
    }
    let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), ..Default::default() };
    let mut output = ctx.run_ui(input, |ui| app.ui(ui));
    output.textures_delta.clear();
    let mut text = String::new();
    for s in &output.shapes {
        collect(&s.shape, &mut text);
    }
    text
}

#[test]
fn bedit_without_a_selected_reference_opens_the_dialog() {
    let mut app = app_with_blocks();
    app.cmdline("BEDIT");
    assert_eq!(app.ui.dialog.as_deref(), Some("bedit"));
    // Blocks sorted by name, case-insensitively; the first is offered.
    assert_eq!(bedit::editable_blocks(app.session.doc().unwrap()), ["door", "Window"]);
    assert_eq!(app.bedit.name, "door");
    assert!(app.session.state().unwrap().block_editor().is_none(), "the dialog changes nothing by itself");

    let ctx = egui::Context::default();
    app.logic(&ctx);
    painted(&mut app, &ctx);
    let text = painted(&mut app, &ctx);
    for s in ["Edit Block Definition", "Block to create or edit", "Window", "Preview", "door symbol"] {
        assert!(text.contains(s), "{s} missing from:\n{text}");
    }

    // OK edits the block named, in any case.
    app.bedit.name = "WINDOW".into();
    assert!(bedit::confirm(&mut app));
    assert_eq!(app.session.state().unwrap().block_editor(), Some("Window"));
    app.run("bclose", json!({})).unwrap();

    // Opened again, it starts on the block last edited.
    app.start("be");
    assert_eq!((app.ui.dialog.as_deref(), app.bedit.name.as_str()), (Some("bedit"), "WINDOW"));
}

#[test]
fn a_new_name_starts_a_new_block_and_a_bad_one_keeps_the_dialog() {
    let mut app = app_with_blocks();
    app.start("bedit");
    app.bedit.name = "a*b".into();
    assert!(!bedit::confirm(&mut app));
    assert!(app.bedit.error.as_deref().is_some_and(|e| e.contains("not a valid block name")), "{:?}", app.bedit.error);

    app.bedit.name = " Skylight ".into();
    let ctx = egui::Context::default();
    app.logic(&ctx);
    painted(&mut app, &ctx);
    assert!(painted(&mut app, &ctx).contains("New block"));
    assert!(bedit::confirm(&mut app));
    assert_eq!(app.session.state().unwrap().block_editor(), Some("Skylight"));
    app.run("line", json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    app.run("bclose", json!({})).unwrap();
    assert_eq!(app.session.doc().unwrap().block("Skylight").unwrap().entities.len(), 1);
}

#[test]
fn a_selected_reference_and_json_calls_skip_the_dialog() {
    let mut app = app_with_blocks();
    let refs: Vec<_> = app.session.doc().unwrap().model.handles();
    app.session.set_selection(vec![refs[0]]);
    app.start("bedit");
    assert!(app.ui.dialog.is_none());
    assert_eq!(app.session.state().unwrap().block_editor(), Some("Window"));
    // In the editor BEDIT says why it can't run instead of opening the dialog.
    app.start("bedit");
    assert!(app.ui.dialog.is_none());
    app.run("bclose", json!({})).unwrap();

    app.run("bedit", json!({ "name": "door" })).unwrap();
    assert!(app.ui.dialog.is_none(), "JSON calls never open dialogs");
    assert_eq!(app.session.state().unwrap().block_editor(), Some("door"));
}
