//! End to end (0.7.8): touch and pen. A pen held on the screen rejects the palm (a second touch
//! doesn't pinch the view); a pen never long-presses; the touch bar's Esc cancels a command, its
//! keyword buttons answer the prompt, and ✖ hides it; a finger held still while a command runs is
//! Enter.

mod e2e_common;

use e2e_common::{E2e, H, W};
use egui::{Event, TouchDeviceId, TouchId, TouchPhase, pos2};
use serde_json::{Value, json};

fn touch(id: u64, phase: TouchPhase, x: f32, y: f32, force: Option<f32>) -> Event {
    Event::Touch { device_id: TouchDeviceId(1), id: TouchId(id), phase, pos: pos2(x, y), force }
}

/// One frame at time `t` (seconds; never before the last frame, which the harness's own frames
/// move on) with `events`.
/// A finger's events as egui-winit sends them on a touch screen: the touch and the pointer.
fn finger(phase: TouchPhase, x: f32, y: f32) -> Vec<Event> {
    let pos = pos2(x, y);
    let mut v = vec![touch(1, phase, x, y, None)];
    match phase {
        TouchPhase::Start => {
            v.push(Event::PointerMoved(pos));
            v.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() });
        }
        TouchPhase::Move => v.push(Event::PointerMoved(pos)),
        _ => {
            v.push(Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() });
            v.push(Event::PointerGone);
        }
    }
    v
}

fn frame_at(e: &mut E2e, t: f64, events: Vec<Event>) {
    let t = t.max(e.ctx.input(|i| i.time) + 0.001);
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

fn canvas_mid(e: &mut E2e) -> (f32, f32) {
    let r: Vec<f64> = e.ok("ui.inspect", json!({}))["canvasRect"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
    match r.as_slice() {
        [x, y, w, h] => ((x + w / 2.0) as f32, (y + h / 2.0) as f32),
        _ => (500.0, 400.0),
    }
}

fn menu_open(e: &E2e) -> bool {
    egui::Popup::is_any_open(&e.ctx)
}

fn command_running(e: &mut E2e) -> bool {
    e.app.session.running.is_some()
}

#[test]
fn the_pen_rejects_the_palm_and_never_long_presses() {
    let mut e = E2e::new();
    e.typed("rectang 0,0 100,60");
    e.exec("zoom", json!({ "mode": "extents" }));
    let (mx, my) = canvas_mid(&mut e);
    let before = view(&mut e);
    // The pen goes down, then a palm lands and spreads away from it.
    let mut t = 10.0;
    frame_at(&mut e, t, vec![touch(1, TouchPhase::Start, mx, my, Some(0.6))]);
    t += 0.05;
    frame_at(&mut e, t, vec![touch(2, TouchPhase::Start, mx + 80.0, my + 40.0, None)]);
    for k in 1..8 {
        t += 0.05;
        let d = 20.0 * k as f32;
        frame_at(&mut e, t, vec![touch(1, TouchPhase::Move, mx, my, Some(0.6)), touch(2, TouchPhase::Move, mx + 80.0 + d, my + 40.0 + d, None)]);
    }
    let during = view(&mut e);
    assert!((during.2 - before.2).abs() < 1e-9 * before.2.max(1.0), "the palm didn't zoom: {before:?} → {during:?}");
    // Held still a whole second: a pen doesn't long-press.
    for _ in 0..25 {
        t += 0.05;
        frame_at(&mut e, t, vec![touch(1, TouchPhase::Move, mx, my, Some(0.6))]);
    }
    assert!(!menu_open(&e), "no shortcut menu from the pen");
    frame_at(&mut e, t + 0.05, vec![touch(1, TouchPhase::End, mx, my, Some(0.6)), touch(2, TouchPhase::End, mx + 240.0, my + 180.0, None)]);
    e.pump();
}

#[test]
fn the_touch_bar_answers_prompts_and_a_held_finger_is_enter() {
    let mut e = E2e::new();
    let (mx, my) = canvas_mid(&mut e);
    // A tap: the touch bar appears.
    let mut t = 5.0;
    frame_at(&mut e, t, finger(TouchPhase::Start, mx, my));
    t += 0.05;
    frame_at(&mut e, t, finger(TouchPhase::End, mx, my));
    e.pump();
    assert!(e.shows("Esc") && e.shows("Enter") && e.shows("Undo"), "the touch bar");

    // Esc cancels a running command.
    e.ok("cmdline.input", json!({ "text": "circle", "enter": true }));
    assert!(command_running(&mut e), "CIRCLE is asking for its centre");
    assert!(e.click("Esc"));
    e.pump();
    assert!(!command_running(&mut e), "Esc on the touch bar cancelled it");

    // A prompt's keywords show as buttons; one answers it.
    e.ok("cmdline.input", json!({ "text": "circle", "enter": true }));
    let kw = ["3P", "2P", "Ttr"].into_iter().find(|k| e.shows(k));
    assert!(
        kw.is_some(),
        "CIRCLE's keywords on the touch bar: {:?}",
        e.elements().iter().filter(|x| x["role"] == "button").map(|x| x["label"].clone()).collect::<Vec<_>>()
    );
    assert!(e.click(kw.unwrap()));
    e.pump();
    let h = e.history().join("\n");
    assert!(h.to_ascii_lowercase().contains("point") || command_running(&mut e), "the keyword was taken: {h}");
    e.click("Esc");
    e.pump();

    // LINE: two taps for points, then a finger held still is Enter (the line ends, no point added).
    let n = e.entity_count();
    e.ok("cmdline.input", json!({ "text": "line", "enter": true }));
    for (dx, dy) in [(-100.0, 0.0), (100.0, 0.0)] {
        t += 1.0;
        frame_at(&mut e, t, finger(TouchPhase::Start, mx + dx, my + dy));
        t += 0.05;
        frame_at(&mut e, t, finger(TouchPhase::End, mx + dx, my + dy));
        e.pump();
    }
    // (A lifted finger is PointerGone in the same frame as its release: the tap still picks.)
    assert_eq!(e.entity_count(), n + 1, "a line from the two taps");
    assert!(command_running(&mut e), "LINE asks for the next point");
    t += 1.0;
    frame_at(&mut e, t, finger(TouchPhase::Start, mx, my + 80.0));
    for _ in 0..15 {
        t += 0.05;
        frame_at(&mut e, t, finger(TouchPhase::Move, mx, my + 80.0));
    }
    frame_at(&mut e, t + 0.05, finger(TouchPhase::End, mx, my + 80.0));
    e.pump();
    assert!(!command_running(&mut e), "the held finger was Enter: LINE ended");
    assert_eq!(e.entity_count(), n + 1, "and its press didn't add a segment");

    // ✖ hides the bar (and remembers it).
    assert!(e.click("✖"));
    e.pump();
    assert!(!e.shows("Esc"));
    assert_eq!(e.ok("ui.inspect", json!({}))["ui"]["touchBar"], false);
}
