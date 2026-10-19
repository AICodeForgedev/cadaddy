//! Touch screens and pens (tablets in the web build, Windows touch screens).
//!
//! - Two fingers pan (the drawing follows their midpoint) and pinch-zoom about it; while two
//!   fingers are down the single-finger pointer does nothing else (no selection window).
//! - A finger held still for [`LONG_PRESS_SECS`] is a right-click: the shortcut menu, or Enter /
//!   the command menu while a command runs — whatever the right button does there.
//! - A pen (a touch that reports pressure) rejects the palm: other touches are ignored while it is
//!   down. Pressure itself is ignored; a hovering pen moves the cursor, so snaps show.
//! - After the first touch the touch bar shows over the drawing: large Enter, Esc, Undo and Redo
//!   buttons, the current prompt's options as buttons, and an input field that brings up the
//!   on-screen keyboard (focused by itself when a prompt asks for text or a number).

use std::collections::BTreeMap;

use egui::{Pos2, Rect, Vec2, pos2, vec2};

use crate::CadApp;

/// How long a still finger takes to become a right-click.
pub const LONG_PRESS_SECS: f64 = 0.55;
/// How far a finger may wander and still be "still" (points).
const STILL: f32 = 10.0;
/// Touch bar buttons are at least this big (a fingertip).
const BUTTON: f32 = 44.0;

#[derive(Clone, Copy, Debug)]
struct Finger {
    pos: Pos2,
    start: Pos2,
    since: f64,
    pen: bool,
}

/// What a frame's touches ask of the drawing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gesture {
    /// Move the drawing by this much (screen points).
    pub pan: Vec2,
    /// Zoom by this factor about `about` (1 = none).
    pub zoom: f32,
    pub about: Pos2,
    /// A long press here: a right-click.
    pub long_press: Option<Pos2>,
    /// Two fingers are down: the pointer's own actions are off.
    pub active: bool,
}

/// The fingers and pens on the screen.
#[derive(Clone, Debug, Default)]
pub struct Tracker {
    fingers: BTreeMap<u64, Finger>,
    /// The two-finger midpoint and spread last frame.
    last: Option<(Pos2, f32)>,
    /// The long press already fired for the touch down now.
    pressed: bool,
    /// A touch has been seen: the touch bar shows.
    pub used: bool,
}

impl Tracker {
    /// Take this frame's touch events (at time `now`, seconds) and say what they do.
    pub fn update(&mut self, events: &[egui::Event], now: f64) -> Gesture {
        for e in events {
            let egui::Event::Touch { id, phase, pos, force, .. } = e else { continue };
            self.used = true;
            let key = id.0;
            match phase {
                egui::TouchPhase::Start => {
                    let pen = force.is_some_and(|f| f > 0.0);
                    // Palm rejection: a pen is down, other touches don't count.
                    if !pen && self.fingers.values().any(|f| f.pen) {
                        continue;
                    }
                    if pen {
                        self.fingers.retain(|_, f| f.pen);
                    }
                    self.fingers.insert(key, Finger { pos: *pos, start: *pos, since: now, pen });
                }
                egui::TouchPhase::Move => {
                    if let Some(f) = self.fingers.get_mut(&key) {
                        f.pos = *pos;
                    }
                }
                egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                    self.fingers.remove(&key);
                }
            }
        }
        if self.fingers.is_empty() {
            self.pressed = false;
        }
        let mut g = Gesture { zoom: 1.0, ..Gesture::default() };
        let pts: Vec<Pos2> = self.fingers.values().map(|f| f.pos).collect();
        if let [a, b, ..] = pts[..] {
            let mid = pos2((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
            let spread = a.distance(b).max(1.0);
            if let Some((m0, s0)) = self.last {
                g.pan = mid - m0;
                g.zoom = spread / s0.max(1.0);
            }
            g.about = mid;
            g.active = true;
            self.last = Some((mid, spread));
            // Two fingers never make a long press.
            self.pressed = true;
        } else {
            self.last = None;
        }
        if let [f] = self.fingers.values().copied().collect::<Vec<_>>()[..]
            && !self.pressed
            && !f.pen
            && f.pos.distance(f.start) <= STILL
            && now - f.since >= LONG_PRESS_SECS
        {
            self.pressed = true;
            g.long_press = Some(f.pos);
        }
        g
    }

    /// A finger is down (the clock has to keep running for a long press).
    pub fn touching(&self) -> bool {
        !self.fingers.is_empty()
    }
}

/// Apply this frame's touches to the drawing area `rect` (from the canvas, before its own
/// pointer handling). Returns the gesture: while it is active the canvas leaves the pointer be.
pub fn canvas_input(app: &mut CadApp, ui: &egui::Ui, rect: Rect) -> Gesture {
    let (events, now) = ui.input(|i| (i.raw.events.clone(), i.time));
    let g = app.canvas.touch.update(&events, now);
    if app.canvas.touch.touching() {
        ui.ctx().request_repaint();
    }
    if g.active {
        if let (Some(xf), Ok(st)) = (app.canvas.xf, app.session.state_mut()) {
            let v = st.view();
            st.set_view_quiet(cadaddy_engine::View {
                center: v.center - cadaddy_engine::geom::Vec2::new(f64::from(g.pan.x) / xf.scale, -f64::from(g.pan.y) / xf.scale),
                height: v.height,
            });
        }
        if (g.zoom - 1.0).abs() > 1e-4
            && let Some(xf) = app.canvas.xf
            && rect.contains(g.about)
        {
            let about = xf.to_world(g.about);
            let _ = app.session.zoom_about(f64::from(g.zoom), about);
        }
        // No selection window or lasso from the first finger.
        app.canvas.lasso = None;
        app.canvas.list = None;
    }
    if let Some(p) = g.long_press.filter(|p| rect.contains(*p)) {
        // A right-click at the finger, through the usual path (shortcut menu, Enter in a command).
        let click =
            |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Secondary, pressed, modifiers: egui::Modifiers::NONE };
        app.synthetic.extend([click(true), click(false)]);
    }
    g
}

#[derive(Clone, Default)]
struct BarState {
    text: String,
    /// The prompt the input field was focused for (focus once per prompt).
    focused_for: String,
}

fn bar_id() -> egui::Id {
    egui::Id::new("cadaddy-touch-bar")
}

/// The touch bar over the bottom of the drawing area, above the command line.
pub fn bar(app: &mut CadApp, ui: &mut egui::Ui, canvas: Rect) {
    if !app.canvas.touch.used || !app.ui.touch_bar {
        return;
    }
    let ctx = ui.ctx().clone();
    let mut st: BarState = ctx.data(|d| d.get_temp(bar_id())).unwrap_or_default();
    let prompt = app.session.current_prompt();
    let keywords: Vec<String> = prompt.as_ref().map(|p| p.keywords.clone()).unwrap_or_default();
    let wants_typing = prompt.as_ref().is_some_and(|p| p.accept.text || p.accept.number);
    let message = prompt.as_ref().map(|p| p.message.clone()).unwrap_or_default();
    let mut action: Option<cadaddy_engine::Input> = None;
    let mut start: Option<&str> = None;
    let mut submit = false;
    let mut close = false;
    let width = (canvas.width() - 24.0).min(760.0);
    let at = pos2(canvas.center().x - width / 2.0, canvas.bottom() - 2.0 * BUTTON - 64.0);
    egui::Area::new(bar_id()).fixed_pos(at).order(egui::Order::Foreground).show(&ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_max_width(width);
            ui.horizontal_wrapped(|ui| {
                let big = |ui: &mut egui::Ui, label: &str| {
                    ui.add(egui::Button::new(egui::RichText::new(label).size(16.0)).min_size(vec2(BUTTON * 1.4, BUTTON)))
                };
                if big(ui, crate::tl!("Enter")).clicked() {
                    action = Some(cadaddy_engine::Input::Enter);
                }
                if big(ui, crate::tl!("Esc")).clicked() {
                    action = Some(cadaddy_engine::Input::Cancel);
                }
                if big(ui, crate::tl!("Undo")).clicked() {
                    start = Some("u");
                }
                if big(ui, crate::tl!("Redo")).clicked() {
                    start = Some("redo");
                }
                for k in &keywords {
                    if big(ui, k).clicked() {
                        action = Some(cadaddy_engine::Input::Keyword(k.clone()));
                    }
                }
                if ui.add(egui::Button::new("✖").min_size(vec2(BUTTON, BUTTON))).on_hover_text(crate::tl!("Hide the touch bar")).clicked() {
                    close = true;
                }
            });
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut st.text)
                        .hint_text(crate::tl!("Type a command or value"))
                        .desired_width(width - BUTTON * 2.0)
                        .min_size(vec2(0.0, BUTTON)),
                );
                crate::icons::describe_area(&r, "Type a command or value");
                // The on-screen keyboard comes with the field's focus: given once per prompt that
                // asks for text or a number.
                if wants_typing && st.focused_for != message {
                    st.focused_for = message.clone();
                    r.request_focus();
                }
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    submit = true;
                }
                if ui.add(egui::Button::new("⏎").min_size(vec2(BUTTON, BUTTON))).clicked() {
                    submit = true;
                }
            });
        });
    });
    if let Some(i) = action {
        let _ = app.session.input(i);
    }
    if let Some(c) = start {
        app.start(c);
    }
    if submit && !st.text.trim().is_empty() {
        let line = std::mem::take(&mut st.text);
        app.cmdline(&line);
    }
    if close {
        app.ui.touch_bar = false;
    }
    ctx.data_mut(|d| d.insert_temp(bar_id(), st));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(id: u64, phase: egui::TouchPhase, x: f32, y: f32, force: Option<f32>) -> egui::Event {
        egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(id), phase, pos: pos2(x, y), force }
    }

    #[test]
    fn two_fingers_pan_and_pinch() {
        let mut t = Tracker::default();
        let g = t.update(&[touch(1, egui::TouchPhase::Start, 100.0, 100.0, None), touch(2, egui::TouchPhase::Start, 200.0, 100.0, None)], 0.0);
        assert!(g.active && g.zoom == 1.0 && g.pan == Vec2::ZERO, "the first frame sets the baseline");
        // Both move right 30: a pan.
        let g = t.update(&[touch(1, egui::TouchPhase::Move, 130.0, 100.0, None), touch(2, egui::TouchPhase::Move, 230.0, 100.0, None)], 0.1);
        assert_eq!(g.pan, vec2(30.0, 0.0));
        assert!((g.zoom - 1.0).abs() < 1e-6);
        // Spread doubles about the midpoint: a 2× zoom.
        let g = t.update(&[touch(1, egui::TouchPhase::Move, 80.0, 100.0, None), touch(2, egui::TouchPhase::Move, 280.0, 100.0, None)], 0.2);
        assert!((g.zoom - 2.0).abs() < 1e-6 && g.about == pos2(180.0, 100.0));
        let g = t.update(&[touch(1, egui::TouchPhase::End, 80.0, 100.0, None), touch(2, egui::TouchPhase::End, 280.0, 100.0, None)], 0.3);
        assert!(!g.active && !t.touching());
    }

    #[test]
    fn a_held_finger_is_a_right_click_once() {
        let mut t = Tracker::default();
        assert_eq!(t.update(&[touch(1, egui::TouchPhase::Start, 50.0, 50.0, None)], 1.0).long_press, None);
        assert_eq!(t.update(&[touch(1, egui::TouchPhase::Move, 53.0, 52.0, None)], 1.3).long_press, None, "not yet");
        assert_eq!(t.update(&[], 1.6).long_press, Some(pos2(53.0, 52.0)));
        assert_eq!(t.update(&[], 2.5).long_press, None, "once per touch");
        t.update(&[touch(1, egui::TouchPhase::End, 53.0, 52.0, None)], 2.6);
        // A finger that moves away is a drag, not a press.
        t.update(&[touch(2, egui::TouchPhase::Start, 50.0, 50.0, None)], 3.0);
        t.update(&[touch(2, egui::TouchPhase::Move, 90.0, 50.0, None)], 3.1);
        assert_eq!(t.update(&[], 4.0).long_press, None);
    }

    #[test]
    fn the_pen_rejects_the_palm() {
        let mut t = Tracker::default();
        t.update(&[touch(1, egui::TouchPhase::Start, 50.0, 50.0, Some(0.4))], 0.0);
        // The palm lands: ignored, no two-finger gesture.
        let g = t.update(&[touch(2, egui::TouchPhase::Start, 300.0, 400.0, None), touch(2, egui::TouchPhase::Move, 310.0, 410.0, None)], 0.1);
        assert!(!g.active);
        // A pen held still doesn't open the menu either (it draws).
        assert_eq!(t.update(&[], 2.0).long_press, None);
        t.update(&[touch(1, egui::TouchPhase::End, 50.0, 50.0, Some(0.0))], 2.1);
        assert!(!t.touching());
    }
}
