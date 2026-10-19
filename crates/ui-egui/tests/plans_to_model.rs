//! The Plans to Model palette, driven through the control channel and clicks: it opens from
//! its UI command, shows the building spec in plain language, approves, edits and removes parts
//! through `bspec.set`, builds and rebuilds the model, lists `model.issues` and zooms to one.

use cadaddy_engine::Session;
use cadaddy_ui_egui::control::{ControlRequest, Outcome, handle};
use cadaddy_ui_egui::docking::Panel;
use cadaddy_ui_egui::{CadApp, Services};
use egui::Key;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable, by};
use serde_json::{Value, json};

fn call(app: &mut CadApp, ctx: &egui::Context, method: &str, params: Value) -> Value {
    let (req, _reply) = ControlRequest::new(method, params);
    match handle(app, ctx, &req) {
        Outcome::Done(v) => {
            assert_eq!(v["ok"], json!(true), "{method}: {v}");
            v["result"].clone()
        }
        Outcome::Screenshot { .. } => Value::Null,
    }
}

fn exec(app: &mut CadApp, command: &str, params: Value) -> Value {
    call(app, &egui::Context::default(), "engine.execute", json!({ "command": command, "params": params }))
}

/// A 30' × 12' building in inches: OFFICE and STORAGE split by a centreline partition that stops
/// short of the north wall when `leak`.
fn plan(leak: bool) -> CadApp {
    let mut app = CadApp::new(Session::new(), Services::default());
    let p = |x: f64, y: f64| json!([x * 12.0, y * 12.0]);
    let rect = |a: (f64, f64), b: (f64, f64)| json!({"vertices": [p(a.0, a.1), p(b.0, a.1), p(b.0, b.1), p(a.0, b.1)], "closed": true});
    exec(&mut app, "layer.new", json!({"name": "A-WALL", "current": true}));
    exec(&mut app, "pline", rect((0.0, 0.0), (30.0, 12.0)));
    exec(&mut app, "pline", rect((0.5, 0.5), (29.5, 11.5)));
    exec(&mut app, "layer.new", json!({"name": "A-WALL-CL", "current": true}));
    exec(&mut app, "line", json!({"points": [p(15.0, 0.5), p(15.0, if leak { 8.0 } else { 11.5 })]}));
    exec(&mut app, "layer.new", json!({"name": "A-ROOM-TAG", "current": true}));
    exec(&mut app, "text", json!({"at": p(5.0, 6.0), "text": "OFFICE", "height": 6}));
    exec(&mut app, "text", json!({"at": p(20.0, 6.0), "text": "STORAGE", "height": 6}));
    exec(
        &mut app,
        "bspec.set",
        json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9}],
            "layers": [
                {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
                {"layer": "A-WALL", "role": "wall"},
                {"layer": "A-ROOM-TAG", "role": "room_tag"}
            ],
            "wall_types": [{"name": "Partition", "thickness_in": 4.875, "from": "centerline", "height_ft": 9}],
            "rooms": [{"name": "Office", "ceiling_ft": 8.5}, {"name": "Storage"}]
        }}),
    );
    app
}

fn harness(mut app: CadApp) -> Harness<'static, CadApp> {
    let ctx = egui::Context::default();
    // Opened through the control channel, as an agent or the menu would.
    call(&mut app, &ctx, "ui.menu.invoke", json!({"command": "ui.panel.planstomodel"}));
    assert!(app.ui.docking.layout.contains(&Panel::PlansToModel));
    app.ui.in_window_menu = false;
    let mut h = Harness::builder().with_size(vec2(1500.0, 1000.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

fn spec(h: &mut Harness<'static, CadApp>) -> Value {
    exec(h.state_mut(), "bspec.get", json!({}))
}

fn built(h: &mut Harness<'static, CadApp>) -> bool {
    spec(h)["built"] == json!(true)
}

#[test]
fn the_palette_shows_approves_edits_and_builds_the_spec() {
    let mut h = harness(plan(false));
    // Plain language.
    h.get_by_label("L1 = this sheet at 0'-0\"; 10'-0\" floor to floor, 9'-0\" ceilings");
    h.get_by_label("A-WALL-CL = walls 9'-0\" high, 4 7/8\" thick (centrelines)");
    h.get_by_label("A-WALL = full height walls (line pairs)");
    h.get_by_label("Office: ceiling 8'-6\"");
    h.get_by_label("Kept in the drawing.");
    h.get_by_label("0 of 8 approved");
    // Approve one part, then all.
    let row = h.get_by_label("L1 = this sheet at 0'-0\"; 10'-0\" floor to floor, 9'-0\" ceilings").rect();
    h.get_all_by_role(egui::accesskit::Role::CheckBox)
        .min_by(|a, b| (a.rect().center().y - row.center().y).abs().total_cmp(&(b.rect().center().y - row.center().y).abs()))
        .unwrap()
        .click();
    h.run_steps(3);
    assert_eq!(spec(&mut h)["spec"]["approved"], json!(["levels/L1"]));
    h.get_by_label("1 of 8 approved");
    h.get_by_label("Approve All").click();
    h.run_steps(3);
    assert_eq!(spec(&mut h)["spec"]["approved"].as_array().unwrap().len(), 8);
    h.get_by_label("8 of 8 approved");
    // Edit the level: its JSON, changed and applied through bspec.set.
    let row = h.get_by_label("L1 = this sheet at 0'-0\"; 10'-0\" floor to floor, 9'-0\" ceilings").rect();
    h.get_all_by_label("Edit").filter(|e| e.rect().top() > row.top()).min_by(|a, b| a.rect().top().total_cmp(&b.rect().top())).unwrap().click();
    h.run_steps(3);
    let editor = || by().role(egui::accesskit::Role::MultilineTextInput);
    h.get(editor()).focus();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.run_steps(1);
    h.get(editor()).type_text(r#"{"name": "Ground", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9}"#);
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(4);
    let s = spec(&mut h);
    assert_eq!(s["spec"]["levels"].as_array().unwrap().len(), 1, "{s}");
    assert_eq!(s["spec"]["levels"][0]["name"], json!("Ground"));
    assert_eq!(s["spec"]["levels"][0]["height_ft"], json!(12.0));
    h.get_by_label("Ground = this sheet at 0'-0\"; 12'-0\" floor to floor, 9'-0\" ceilings");
    assert!(h.query_by_role(egui::accesskit::Role::MultilineTextInput).is_none(), "the editor closes");
    // Bad JSON stays in the editor with the error.
    let row = h.get_by_label("Ground = this sheet at 0'-0\"; 12'-0\" floor to floor, 9'-0\" ceilings").rect();
    h.get_all_by_label("Edit").filter(|e| e.rect().top() > row.top()).min_by(|a, b| a.rect().top().total_cmp(&b.rect().top())).unwrap().click();
    h.run_steps(3);
    h.get(editor()).focus();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    h.get(editor()).type_text("{nope");
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(3);
    assert!(h.query_by_role(egui::accesskit::Role::MultilineTextInput).is_some());
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(h.query_by_role(egui::accesskit::Role::MultilineTextInput).is_none());
    // Remove a room.
    let rooms = h.get_all_by_label("Remove").count();
    assert!(rooms >= 5);
    let storage = h.get_by_label("Storage: the level's ceiling").rect().top();
    h.get_all_by_label("Remove").min_by(|a, b| (a.rect().top() - storage).abs().total_cmp(&(b.rect().top() - storage).abs())).unwrap().click();
    h.run_steps(3);
    assert_eq!(spec(&mut h)["spec"]["rooms"].as_array().unwrap().len(), 1);
    // Build: the model is made, the issues are checked (none), and Rebuild and Walk come on.
    assert!(!built(&mut h));
    h.get_by_label("Build").click();
    h.run_steps(4);
    assert!(built(&mut h));
    h.get_by_label("Issues (0)");
    h.get_by_label("No issues found.");
    assert!(h.get_by_label("Build").accesskit_node().is_disabled());
    assert!(!h.get_by_label("Rebuild").accesskit_node().is_disabled());
    assert!(!h.get_by_label("Walk").accesskit_node().is_disabled());
    let walls = h.state().session.doc().unwrap().model.iter().filter(|e| e.common.layer == "3D-Walls").count();
    h.get_by_label("Rebuild").click();
    h.run_steps(4);
    assert_eq!(h.state().session.doc().unwrap().model.iter().filter(|e| e.common.layer == "3D-Walls").count(), walls);
}

#[test]
fn issues_list_and_zoom_to_one() {
    let mut h = harness(plan(true));
    h.get_by_label("Build").click();
    h.run_steps(4);
    // The short partition lets OFFICE run into STORAGE.
    h.get_by_label("OFFICE runs into STORAGE: their outline is one (a gap joins them)");
    h.get_by_label("Issues (1)");
    // One space, one ceiling (not one per tag).
    assert_eq!(h.state().session.doc().unwrap().model.iter().filter(|e| e.common.layer == "3D-Ceilings").count(), 1);
    let before = h.state().session.state().unwrap().view().center;
    h.get_by_label("Zoom").click();
    h.run_steps(3);
    let st = h.state().session.state().unwrap();
    assert!((st.view().center.x - 60.0).abs() < 1e-6 && (st.view().center.y - 72.0).abs() < 1e-6, "{:?} → {:?}", before, st.view().center);
    assert_eq!(h.state().session.selection().len(), 2, "both room tags are selected");
    // Closing the gap and checking again clears it.
    let partition = h.state().session.doc().unwrap().model.iter().find(|e| e.common.layer == "A-WALL-CL").unwrap().handle.hex();
    exec(h.state_mut(), "erase", json!({"handles": [partition]}));
    exec(h.state_mut(), "layer.current", json!({"name": "A-WALL-CL"}));
    exec(h.state_mut(), "line", json!({"points": [[180, 6], [180, 138]]}));
    h.run_steps(3);
    h.get_by_label("The drawing changed since; check again.");
    h.get_by_label("Check").click();
    h.run_steps(3);
    h.get_by_label("Issues (0)");
}

#[test]
fn a_drawing_without_a_spec_offers_to_save_the_draft() {
    let mut app = CadApp::new(Session::new(), Services::default());
    exec(&mut app, "layer.new", json!({"name": "A-WALL", "current": true}));
    exec(&mut app, "line", json!({"points": [[0, 0], [120, 0]]}));
    let mut h = harness(app);
    h.get_by_label("Drafted from the walk rules; not saved yet.");
    h.get_by_label("*WALL* = full height walls (line pairs)");
    h.get_by_label("Save Spec").click();
    h.run_steps(3);
    assert_eq!(spec(&mut h)["stored"], json!(true));
    h.get_by_label("Kept in the drawing.");
    assert!(h.query_by_label("Save Spec").is_none());
}

#[test]
fn draft_from_plan_marks_its_guesses() {
    let mut app = CadApp::new(Session::new(), Services::default());
    exec(&mut app, "layer.new", json!({"name": "A-WALL", "current": true}));
    exec(&mut app, "pline", json!({"vertices": [[0, 0], [240, 0], [240, 120], [0, 120]], "closed": true}));
    exec(&mut app, "pline", json!({"vertices": [[6, 6], [234, 6], [234, 114], [6, 114]], "closed": true}));
    let mut h = harness(app);
    h.get_by_label("Draft from Plan").click();
    h.run_steps(3);
    assert_eq!(spec(&mut h)["stored"], json!(true));
    h.get_by_label("A-WALL = full height walls 6\" thick (line pairs)");
    let guesses = h.get_all_by_label("guess").count();
    assert!(guesses >= 3, "{guesses}");
    // Approving a guessed part takes its mark away.
    h.get_by_label("Approve All").click();
    h.run_steps(3);
    assert!(h.query_by_label("guess").is_none());
}
