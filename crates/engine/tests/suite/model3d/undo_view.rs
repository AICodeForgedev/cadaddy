//! Undo gives back the view a command changed (S5-007): a model build leaves model space in a
//! shaded SW iso view; undoing it returns the plan view and visual style it had, and redo the
//! build's again. Plain pans and zooms are not undo steps.

use cadaddy_engine::Session;
use cadaddy_render::shade::VisualStyle;
use serde_json::json;

/// A room's walls on A-WALL, as a plan to build from.
fn plan() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[-12, -12], [492, -12], [492, 372], [-12, 372]], "closed": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[-6, -6], [486, -6], [486, 366], [-6, 366]], "closed": true})).unwrap();
    s
}

#[test]
fn undoing_a_build_gives_back_the_plan_view() {
    let mut s = plan();
    let st = s.state().unwrap();
    let (dir, style, view) = (st.view_dir, st.visual_style, st.view());
    assert_eq!(style, VisualStyle::Wireframe2d);
    s.execute("model.build", &json!({})).unwrap();
    let built = s.state().unwrap().model_look();
    assert!(built.dir != dir && built.style == VisualStyle::Realistic, "{built:?}");
    s.undo().unwrap();
    let st = s.state().unwrap();
    assert_eq!((st.view_dir, st.visual_style, st.view()), (dir, style, view));
    s.redo().unwrap();
    assert_eq!(s.state().unwrap().model_look(), built);
    // A zoom between steps is no step of its own: undo leaves it.
    s.undo().unwrap();
    s.execute("zoom", &json!({"mode": "scale", "factor": 0.5})).unwrap();
    let zoomed = s.state().unwrap().view();
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    s.undo().unwrap();
    assert_eq!(s.state().unwrap().view(), zoomed);
}
