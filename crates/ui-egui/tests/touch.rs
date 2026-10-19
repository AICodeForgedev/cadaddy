//! Touch screens: simulated touch sequences into the app — a pinch zooms about the fingers, two
//! fingers pan, a held finger becomes a right-click, and the touch bar appears with a text field
//! for the on-screen keyboard.

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services};
use egui::{Event, Pos2, TouchDeviceId, TouchId, TouchPhase, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

fn touch(id: u64, phase: TouchPhase, p: Pos2) -> Event {
    Event::Touch { device_id: TouchDeviceId(7), id: TouchId(id), phase, pos: p, force: None }
}

fn harness() -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::default(), Services::default());
    app.run("new", json!({})).unwrap();
    app.run("line", json!({ "points": [[0, 0], [100, 100]] })).unwrap();
    let mut h = Harness::builder().with_size(vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

fn view(h: &Harness<'_, CadApp>) -> cadaddy_engine::View {
    h.state().session.state().unwrap().view()
}

fn send(h: &mut Harness<'_, CadApp>, events: Vec<Event>) {
    for e in events {
        h.event(e);
    }
    h.step();
}

#[test]
fn pinch_and_two_finger_pan_move_the_view() {
    let mut h = harness();
    let rect = h.state().canvas.rect.unwrap();
    let c = rect.center();
    let before = view(&h);
    send(&mut h, vec![touch(1, TouchPhase::Start, c - vec2(50.0, 0.0)), touch(2, TouchPhase::Start, c + vec2(50.0, 0.0))]);
    // Spread to twice as far apart about the same midpoint: twice as big.
    send(&mut h, vec![touch(1, TouchPhase::Move, c - vec2(100.0, 0.0)), touch(2, TouchPhase::Move, c + vec2(100.0, 0.0))]);
    let zoomed = view(&h);
    assert!((zoomed.height - before.height / 2.0).abs() < before.height * 0.01, "{} → {}", before.height, zoomed.height);
    // Both fingers move right: the drawing follows (its centre moves left in drawing units).
    send(&mut h, vec![touch(1, TouchPhase::Move, c - vec2(60.0, 0.0)), touch(2, TouchPhase::Move, c + vec2(140.0, 0.0))]);
    let panned = view(&h);
    assert!(panned.center.x < zoomed.center.x, "{:?} → {:?}", zoomed.center, panned.center);
    assert!((panned.height - zoomed.height).abs() < zoomed.height * 1e-6, "a pan doesn't zoom: {} vs {}", zoomed.height, panned.height);
    send(&mut h, vec![touch(1, TouchPhase::End, c), touch(2, TouchPhase::End, c)]);
    assert!(!h.state().canvas.touch.touching());
}

#[test]
fn a_long_press_right_clicks_and_the_touch_bar_shows() {
    let mut h = harness();
    let p = h.state().canvas.rect.unwrap().center() + vec2(-120.0, 40.0);
    assert!(h.query_by_label("Type a command or value").is_none(), "no touch bar before a touch");
    send(&mut h, vec![touch(1, TouchPhase::Start, p)]);
    // Held still past the long-press time.
    for _ in 0..40 {
        h.step();
        if !h.state().synthetic.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let right: Vec<_> = h
        .state()
        .synthetic
        .iter()
        .filter(|e| matches!(e, Event::PointerButton { button: egui::PointerButton::Secondary, pos, .. } if *pos == p))
        .collect();
    assert_eq!(right.len(), 2, "a right press and release at the finger");
    send(&mut h, vec![touch(1, TouchPhase::End, p)]);
    h.run_steps(2);
    assert!(h.query_by_label("Type a command or value").is_some(), "the touch bar's field for the on-screen keyboard");
    // The bar's Undo button works.
    h.get_all_by_label("Undo").last().unwrap().click();
    h.run_steps(2);
    assert!(h.state().session.doc().unwrap().model.is_empty(), "the line was undone");
}
