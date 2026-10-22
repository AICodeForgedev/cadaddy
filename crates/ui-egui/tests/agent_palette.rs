//! The Agent Work palette (Q319): what agents decided, waiting for a person to accept.

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services, menus};
use serde_json::json;

/// A drawing where an agent has worked: a spec with untagged duct runs, built, one deferred, one
/// raised as a concern, a note.
fn worked() -> CadApp {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "M-DUCT-SUPP", "current": true})).unwrap();
    for k in 0..4 {
        let x = 24.0 + 60.0 * f64::from(k);
        s.execute("line", &json!({"points": [[x, 24], [x, 240]]})).unwrap();
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": {"levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}]}})).unwrap();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("bim.build", &json!({})).unwrap();
    let ids: Vec<String> =
        s.execute("review.list", &json!({})).unwrap()["items"].as_array().unwrap().iter().map(|i| i["id"].as_str().unwrap().to_string()).collect();
    s.execute("review.defer", &json!({"id": ids[0], "reason": "field to check the beam depth at grid 2"})).unwrap();
    s.execute(
        "review.resolve",
        &json!({"id": ids[1], "answer": {"size_in": [14, 10]}, "citation": {"kind": "drawing", "document": "M-101", "ref": "tag at B-2"}}),
    )
    .unwrap();
    s.execute("project.note", &json!({"text": "L1 supply runs reviewed to grid 3."})).unwrap();
    CadApp::new(s, Services::default())
}

#[test]
fn the_palette_opens_from_its_menu_and_accepts_all() {
    let mut app = worked();
    // Tools ▸ Agent Work opens the palette (the command without parameters).
    let _ = menus::run_ui_command(&mut app, "review.log", &serde_json::Value::Null);
    assert_eq!(app.ui.dialog.as_deref(), Some(cadaddy_ui_egui::agentpalette::DIALOG));
    let mut harness = egui_kittest::Harness::builder().with_size(egui::vec2(1100.0, 760.0)).wgpu().build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    harness.run_steps(6);
    let img = harness.render().unwrap();
    if let Some(dir) = std::env::var_os("AGENT_PALETTE_SHOT") {
        img.save(std::path::Path::new(&dir).join("agent_palette.png")).unwrap();
    }
    // Accept all: nothing left waiting.
    let app = harness.state_mut();
    let before = app.session.execute("review.log", &json!({"author": "agent", "approved": false})).unwrap()["total"].as_u64().unwrap();
    assert!(before >= 3);
    app.session.execute("review.approve", &json!({"all": true})).unwrap();
    assert_eq!(app.session.execute("review.log", &json!({"author": "agent", "approved": false})).unwrap()["total"], 0);
}
