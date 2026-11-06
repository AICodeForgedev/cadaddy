//! The Block Authoring Palettes window and the test window's bar in the Block Editor.

use cadaddy_engine::{Input, Session};
use cadaddy_geom::Vec2;
use cadaddy_ui_egui::{CadApp, Services, authorpalette};
use serde_json::json;

fn editing_app() -> CadApp {
    let mut app = CadApp::new(Session::new(), Services::default());
    let l = app.run("line", json!({ "points": [[0, 0], [10, 0]] })).unwrap()["handles"][0].clone();
    app.run("block", json!({ "name": "Bar", "base": [0, 0], "handles": [l] })).unwrap();
    app.session.set_selection(Vec::new());
    app.run("bedit", json!({ "name": "Bar" })).unwrap();
    app
}

/// The text painted in a frame (after a first one, so windows have faded in).
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
    let mut text = String::new();
    for _ in 0..2 {
        let mut output = ctx.run_ui(input.clone(), |ui| app.ui(ui));
        output.textures_delta.clear();
        text.clear();
        for s in &output.shapes {
            collect(&s.shape, &mut text);
        }
    }
    text
}

#[test]
fn the_palette_window_lists_the_tools_of_the_tab_and_hides_with_the_command() {
    let mut app = editing_app();
    let ctx = egui::Context::default();
    app.logic(&ctx);
    let text = painted(&mut app, &ctx);
    for s in ["Block Authoring Palettes", "Parameters", "Actions", "Linear", "Polar", "Alignment", "Base Point", "Test Block"] {
        assert!(text.contains(s), "{s} missing from:\n{text}");
    }
    app.run("bauthorpalette", json!({ "action": "hide" })).unwrap();
    assert!(!painted(&mut app, &ctx).contains("Block Authoring Palettes"));
    app.run("bauthorpalette", json!({ "action": "show" })).unwrap();
    assert!(painted(&mut app, &ctx).contains("Block Authoring Palettes"));
    // Outside the editor there are none.
    app.run("bclose", json!({})).unwrap();
    assert!(!painted(&mut app, &ctx).contains("Block Authoring Palettes"));
}

#[test]
fn a_tool_starts_its_command_with_the_type_and_the_prompt_asks_for_the_picks() {
    let mut app = editing_app();
    authorpalette::start_tool(&mut app, "bparameter", "polar");
    let r = app.session.running.as_ref().expect("BPARAMETER is running");
    assert_eq!(r.id, "bparameter");
    assert_eq!(app.session.current_prompt().map(|p| p.message), Some("Specify start point".to_string()));
    app.session.input(Input::Point(Vec2::new(0.0, 0.0))).unwrap();
    app.session.input(Input::Point(Vec2::new(4.0, 0.0))).unwrap();
    assert!(app.session.running.is_none());
    let d = app.run("blocks.dynamic", json!({ "name": "Bar" })).unwrap();
    assert_eq!(d["parameters"][0]["kind"]["type"], "polar");
    // An action tool: the parameter it can follow is the only one, so it asks for the objects.
    authorpalette::start_tool(&mut app, "bactiontool", "scale");
    assert_eq!(app.session.current_prompt().map(|p| p.message), Some("Select objects".to_string()));
    app.session.cancel();
}

#[test]
fn the_test_window_shows_a_bar_that_closes_it() {
    let mut app = editing_app();
    let ctx = egui::Context::default();
    app.logic(&ctx);
    app.run("btestblock", json!({})).unwrap();
    let text = painted(&mut app, &ctx);
    assert!(text.contains("Close Test Block"), "{text}");
    assert!(!text.contains("Block Authoring Palettes"), "the palettes wait for the editor");
    app.run("btestblock", json!({ "action": "close" })).unwrap();
    let text = painted(&mut app, &ctx);
    assert!(!text.contains("Close Test Block") && text.contains("Block Authoring Palettes"), "{text}");
}
