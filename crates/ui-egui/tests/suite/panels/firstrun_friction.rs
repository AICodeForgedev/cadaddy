//! Friction a new user met in their first ten minutes (Q234): each fixed one stays fixed.

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services};
use egui::{Rect, Shape, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

fn harness() -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::default(), Services::default());
    app.run("new", serde_json::json!({})).unwrap();
    app.ui.start_tab = false;
    app.ui.ribbon = true;
    let mut h = Harness::builder().with_size(vec2(1600.0, 900.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    h
}

fn texts(shape: &Shape, clip: Rect, out: &mut Vec<(String, Rect)>) {
    match shape {
        Shape::Text(t) => {
            let r = t.visual_bounding_rect().intersect(clip);
            if r.is_positive() {
                out.push((t.galley.text().to_string(), r));
            }
        }
        Shape::Vec(v) => v.iter().for_each(|s| texts(s, clip, out)),
        _ => {}
    }
}

/// The Measure split button's list (Distance / Area / Measure) opened as a popup wider than the
/// screen, its names far to the left of the button; it is now as wide as its names.
#[test]
fn ribbon_flyout_is_as_wide_as_its_names_and_under_its_button() {
    let mut h = harness();
    let arrow = h.get_by_label("Measure options").rect();
    h.get_by_label("Measure options").click();
    h.run_steps(3);
    let mut shown = Vec::new();
    for s in &h.output().shapes {
        texts(&s.shape, s.clip_rect, &mut shown);
    }
    let names: Vec<&(String, Rect)> = shown.iter().filter(|(t, _)| t == "Distance" || t == "Area").collect();
    assert_eq!(names.len(), 2, "both entries drawn: {shown:?}");
    for (name, r) in names {
        assert!(r.left() >= 0.0 && r.right() <= 1600.0, "{name} on screen: {r:?}");
        assert!((r.left() - arrow.left()).abs() < 260.0, "{name} at {r:?} sits by its button {arrow:?}");
    }
}

/// The Plot dialog's device "None" plots to a PDF file, which nobody guessed: it says so.
#[test]
fn plot_dialog_says_none_plots_to_a_pdf_file() {
    let mut h = harness();
    h.state_mut().ui.dialog = Some("plot".into());
    h.run_steps(4);
    let mut shown = Vec::new();
    for s in &h.output().shapes {
        texts(&s.shape, s.clip_rect, &mut shown);
    }
    assert!(shown.iter().any(|(t, _)| t == "None (PDF file)"), "{:?}", shown.iter().map(|(t, _)| t).collect::<Vec<_>>());
}

/// The floating command line hid the bottom of a freshly opened drawing: the drawing area tells
/// the engine how much of it the command line covers, and zooming to the extents stays above it.
#[test]
fn zoom_extents_stays_above_the_floating_command_line() {
    let mut h = harness();
    h.state_mut().run("rectang", serde_json::json!({ "p1": [0, 0], "p2": [360, 240] })).unwrap();
    h.run_steps(2);
    let (_, covered) = h.state().session.viewport_inset_px;
    assert!(covered >= 34.0, "the bar and its margin at least: {covered}");
    h.state_mut().run("zoom", serde_json::json!({ "mode": "extents" })).unwrap();
    h.run_steps(2);
    let app = h.state();
    let v = app.session.state().unwrap().view();
    let (_, px_h) = app.session.viewport_px;
    let bottom_px = px_h / 2.0 + (v.center.y - 0.0) * px_h / v.height;
    assert!(bottom_px < px_h - covered, "the drawing's bottom edge ({bottom_px}) is above the command line ({})", px_h - covered);
    // Hidden command line: nothing covered.
    h.state_mut().ui.show_command_line = false;
    h.run_steps(2);
    assert_eq!(h.state().session.viewport_inset_px, (0.0, 0.0));
}
