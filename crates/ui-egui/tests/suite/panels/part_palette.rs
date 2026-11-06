//! The Model Tree palette on a made-up part: its parameters and feature tree, one feature
//! suppressed and one in error, in light and dark; a parameter edited through the palette's
//! command rebuilds the part. `CADADDY_PART_SHOTS=<dir>` saves the renders (to look at).

use cadaddy_ui_egui::{CadApp, Services, theme};
use egui_kittest::Harness;
use serde_json::{Value, json};

fn app() -> CadApp {
    let mut app = CadApp::new(cadaddy_engine::Session::new(), Services::default());
    app.run("part.new", json!({ "name": "Bracket", "parameters": {"thick": 4, "width": "10*thick", "hole": 3} })).unwrap();
    let rect = |w: &str| {
        json!({ "name": "Base", "geometry": [
            {"type": "line", "a": [0, 0], "b": [40, 0]}, {"type": "line", "a": [40, 0], "b": [40, 20]},
            {"type": "line", "a": [40, 20], "b": [0, 20]}, {"type": "line", "a": [0, 20], "b": [0, 0]},
            {"type": "circle", "center": [20, 10], "radius": 3}],
            "constraints": [{"kind": "radius", "refs": [{"geom": 4}], "value": w}] })
    };
    app.run("part.sketch", rect("hole")).unwrap();
    app.run("part.pad", json!({ "sketch": "Base", "length": "thick" })).unwrap();
    app.run("part.fillet", json!({ "edges": [[40, 20, 2]], "radius": 2 })).unwrap();
    app.run("part.chamfer", json!({ "edges": [[20, 0, 4]], "distance": 1 })).unwrap();
    app.run("part.edit", json!({ "feature": "Chamfer001", "suppressed": true })).unwrap();
    app.run("part.sketch", json!({ "name": "Open", "geometry": [{"type": "line", "a": [0, 0], "b": [5, 0]}] })).unwrap();
    app.run("part.pocket", json!({ "sketch": "Open", "depth": 1 })).unwrap();
    app.run("parttree", Value::Null).unwrap();
    app
}

#[test]
fn the_model_tree_renders_light_and_dark() {
    for (name, dark) in [("part-tree-light.png", false), ("part-tree-dark.png", true)] {
        let mut h = Harness::builder().with_size(egui::vec2(460.0, 620.0)).wgpu().build_ui_state(
            move |ui, app: &mut CadApp| {
                theme::apply(ui.ctx(), if dark { egui::Theme::Dark } else { egui::Theme::Light }, false);
                cadaddy_ui_egui::dialogs::show(app, ui.ctx());
            },
            app(),
        );
        h.run_steps(4);
        assert_eq!(h.state().ui.dialog.as_deref(), Some(cadaddy_ui_egui::partpalette::DIALOG), "the palette stays open");
        if let Some(dir) = std::env::var_os("CADADDY_PART_SHOTS") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            h.render().unwrap().save(dir.join(name)).unwrap();
        }
    }
}

#[test]
fn the_tree_reads_what_the_palette_shows() {
    let mut app = app();
    let t = app.session.execute("part.tree", &json!({})).unwrap();
    let states: Vec<&str> = t["features"].as_array().unwrap().iter().map(|f| f["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["ok", "ok", "ok", "suppressed", "ok", "error"]);
    let before = t["volume"].as_f64().unwrap();
    // What the palette runs when an expression is edited.
    app.run("part.parameter", json!({ "part": "Bracket", "name": "thick", "expr": "5" })).unwrap();
    let after = app.session.execute("part.tree", &json!({})).unwrap()["volume"].as_f64().unwrap();
    // Only the pad's length follows thick here (and the corner fillet with it): × 5/4 exactly.
    assert!((after / before - 1.25).abs() < 1e-9, "{before} → {after}");
}
