//! End to end: touch gestures from synthetic touch events, as a tablet sends them. Two fingers
//! spreading zoom in about their midpoint and moving together pan; the touch bar (Enter, Esc…)
//! appears after the first touch; a finger held still is a right-click (the shortcut menu).

mod e2e_common;

use e2e_common::{E2e, H, W};
use egui::{Event, TouchDeviceId, TouchId, TouchPhase, pos2};
use serde_json::{Value, json};

fn touch(id: u64, phase: TouchPhase, x: f32, y: f32) -> Event {
    Event::Touch { device_id: TouchDeviceId(1), id: TouchId(id), phase, pos: pos2(x, y), force: None }
}

/// One frame at time `t` (seconds) with `events` (the clock matters for a long press).
fn frame_at(e: &mut E2e, t: f64, events: Vec<Event>) {
    let mut raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W, H))),
        time: Some(t),
        events,
        ..Default::default()
    };
    e.app.raw_input_hook(&mut raw);
    let app = &mut e.app;
    let mut out = e.ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
}

fn view(e: &mut E2e) -> (f64, f64, f64) {
    let v = e.ok("ui.inspect", json!({}))["view"].clone();
    let c = |i: usize| v["center"][i].as_f64().unwrap_or(f64::NAN);
    (c(0), c(1), v["height"].as_f64().unwrap_or(f64::NAN))
}

/// The middle of the drawing area on screen.
fn canvas_mid(e: &mut E2e) -> (f32, f32) {
    let r: Vec<f64> = e.ok("ui.inspect", json!({}))["canvasRect"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
    match r.as_slice() {
        [x, y, w, h] => ((x + w / 2.0) as f32, (y + h / 2.0) as f32),
        _ => (500.0, 400.0),
    }
}

#[test]
fn pinch_zooms_two_fingers_pan_and_a_long_press_opens_the_menu() {
    let mut e = E2e::new();
    e.typed("rectang 0,0 100,60");
    e.exec("zoom", json!({ "mode": "extents" }));
    let (mx, my) = canvas_mid(&mut e);
    let (_, _, h0) = view(&mut e);
    assert!(!e.shows("Esc"), "no touch bar before a touch");

    // Pinch out: the fingers' spread doubles.
    let mut t = 10.0;
    frame_at(&mut e, t, vec![touch(1, TouchPhase::Start, mx - 50.0, my), touch(2, TouchPhase::Start, mx + 50.0, my)]);
    t += 0.05;
    frame_at(&mut e, t, vec![touch(1, TouchPhase::Move, mx - 100.0, my), touch(2, TouchPhase::Move, mx + 100.0, my)]);
    let (cx1, cy1, h1) = view(&mut e);
    assert!((h1 - h0 / 2.0).abs() < h0 * 0.05, "zoomed in about 2x: {h0} → {h1}");

    // Both fingers move right together: the drawing follows (the view's centre moves left).
    t += 0.05;
    frame_at(&mut e, t, vec![touch(1, TouchPhase::Move, mx - 60.0, my), touch(2, TouchPhase::Move, mx + 140.0, my)]);
    let (cx2, cy2, h2) = view(&mut e);
    assert!(cx2 < cx1 - 1e-6 && (cy2 - cy1).abs() < 1e-6 && (h2 - h1).abs() < h1 * 1e-3, "panned: {cx1},{cy1} → {cx2},{cy2}");
    t += 0.05;
    frame_at(&mut e, t, vec![touch(1, TouchPhase::End, mx - 60.0, my), touch(2, TouchPhase::End, mx + 140.0, my)]);
    // The touch bar shows now.
    e.pump();
    assert!(e.shows("Esc") && e.shows("Enter"), "the touch bar");

    // A finger held still: the shortcut menu, as a right-click.
    assert!(!egui::Popup::is_any_open(&e.ctx), "no menu before the long press");
    t += 1.0;
    // As the windowing layer does, the first finger also moves the pointer.
    frame_at(&mut e, t, vec![Event::PointerMoved(pos2(mx, my + 40.0)), touch(3, TouchPhase::Start, mx, my + 40.0)]);
    for k in 1..=8 {
        frame_at(&mut e, t + 0.1 * f64::from(k), vec![]);
    }
    frame_at(&mut e, t + 1.0, vec![touch(3, TouchPhase::End, mx, my + 40.0)]);
    for k in 1..=3 {
        frame_at(&mut e, t + 1.0 + 0.05 * f64::from(k), vec![]);
    }
    assert!(egui::Popup::is_any_open(&e.ctx), "the shortcut menu after a long press");
    assert!(e.app.synthetic.is_empty(), "the right-click was delivered");
}
