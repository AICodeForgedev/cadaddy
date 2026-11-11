//! The offer on the plan of a drawing opened holding a built model (Q6310): one click shows it
//! in 3D BIM.

use cadaddy_engine::Session;
use cadaddy_engine::render::shade::VisualStyle;
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::{CadApp, Services};

fn opened(built: bool, reply: serde_json::Value) -> CadApp {
    let mut app = CadApp::new(Session::empty(), Services::default());
    let d = if built { cadaddy_engine::sample::bim_building(2, 4) } else { cadaddy_engine::doc::Drawing::new_imperial() };
    app.session.open_drawing(d, "Sample Building", None);
    app.opened(Ok(reply));
    app
}

fn harness(app: CadApp) -> Harness<'static, CadApp> {
    Harness::builder().with_size(vec2(900.0, 500.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            let xf = crate::canvas::Xf { rect: ui.max_rect(), center: cadaddy_engine::geom::Vec2::ZERO, scale: 1.0, proj: None };
            super::offer_bim3d(app, ui, &xf);
        },
        app,
    )
}

#[test]
fn a_built_model_opens_on_its_plan_offering_3d_bim_one_click_away() {
    let app = opened(true, json!({ "index": 0, "builtModel": true, "offer": "view.bim3d" }));
    let uid = app.session.state().unwrap().uid;
    assert_eq!(app.canvas.shade.offer_bim3d, Some(uid));
    assert_eq!(app.session.state().unwrap().visual_style, VisualStyle::Wireframe2d, "it opens on the plan");
    let mut h = harness(app);
    h.run_steps(2);
    h.get_by_label("This drawing holds a built model.");
    h.get_by_label("Show 3D BIM").click();
    h.run_steps(2);
    let app = h.state();
    assert_eq!(app.canvas.shade.offer_bim3d, None);
    let st = app.session.state().unwrap();
    assert_eq!(st.visual_style, VisualStyle::ShadedEdges);
    assert!(!cadaddy_engine::cmd::bimlook::Look::of(&st.doc).off);
    assert!(st.projection().is_some(), "a 3D view");
}

#[test]
fn the_offer_is_for_a_newly_opened_built_model_only_and_closes() {
    // A drawing switched to (already open), or one with no built model: no offer.
    let app = opened(true, json!({ "index": 0, "builtModel": true, "offer": "view.bim3d", "alreadyOpen": true }));
    assert_eq!(app.canvas.shade.offer_bim3d, None);
    let app = opened(false, json!({ "index": 0 }));
    assert_eq!(app.canvas.shade.offer_bim3d, None);
    // Closed: gone, and the drawing stays on its plan.
    let app = opened(true, json!({ "index": 0, "builtModel": true, "offer": "view.bim3d" }));
    let mut h = harness(app);
    h.run_steps(2);
    h.get_by_label("×").click();
    h.run_steps(2);
    assert_eq!(h.state().canvas.shade.offer_bim3d, None);
    assert_eq!(h.state().session.state().unwrap().visual_style, VisualStyle::Wireframe2d);
    assert!(h.query_by_label("Show 3D BIM").is_none());
    // Another drawing current: the card isn't drawn over it.
    let mut app = opened(true, json!({ "index": 0, "builtModel": true, "offer": "view.bim3d" }));
    app.session.open_drawing(cadaddy_engine::doc::Drawing::new_imperial(), "Plan", None);
    let mut h = harness(app);
    h.run_steps(2);
    assert!(h.query_by_label("Show 3D BIM").is_none());
}
