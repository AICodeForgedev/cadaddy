//! The steering wheel (NAVSWHEEL) on the drawing area: a round menu of navigation tools that
//! stays where it was opened (drag its hub to move it). The outer ring holds ZOOM, ORBIT, REWIND
//! and PAN, the inner ring CENTER, WALK and LOOK. Dragging a wedge zooms about the point pressed,
//! orbits, pans or looks around; clicking REWIND goes back a view; CENTER is dragged onto the
//! point to centre on (and orbit about); WALK starts walk mode. Everything goes through the
//! `navswheel.*` commands; Esc, the × or a right click closes the wheel.

use egui::{Color32, CursorIcon, Pos2, Rect, Sense, Stroke, pos2, vec2};
use serde_json::{Value, json};

use crate::CadApp;
use crate::canvas::Xf;

/// The wheel while it is open.
#[derive(Clone, Debug)]
pub struct WheelUi {
    /// The wheel's centre on screen.
    pub center: Pos2,
    /// The wedge being dragged, from where, and the pointer at the last step.
    drag: Option<(Wedge, Pos2, Pos2)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wedge {
    Zoom,
    Orbit,
    Rewind,
    Pan,
    Center,
    Walk,
    Look,
    Hub,
}

impl Wedge {
    pub fn label(self) -> &'static str {
        match self {
            Wedge::Zoom => "ZOOM",
            Wedge::Orbit => "ORBIT",
            Wedge::Rewind => "REWIND",
            Wedge::Pan => "PAN",
            Wedge::Center => "CENTER",
            Wedge::Walk => "WALK",
            Wedge::Look => "LOOK",
            Wedge::Hub => "",
        }
    }
}

/// The outer ring's wedges, each a quarter turn centred on its angle (degrees, counterclockwise
/// from screen right), and the inner ring's, each a third.
const OUTER: [(Wedge, f32); 4] = [(Wedge::Zoom, 90.0), (Wedge::Pan, 180.0), (Wedge::Rewind, 270.0), (Wedge::Orbit, 0.0)];
const INNER: [(Wedge, f32); 3] = [(Wedge::Center, 90.0), (Wedge::Look, 210.0), (Wedge::Walk, 330.0)];

/// The wheel's outer radius for NAVSWHEELSIZEBIG (0 small … 3 extra large).
pub fn radius(app: &CadApp) -> f32 {
    [52.0, 68.0, 84.0, 104.0][usize::from(app.session.settings.navswheel_size.min(3))]
}

/// The ring radii: hub, inner ring's outer edge, outer edge.
fn rings(r: f32) -> (f32, f32, f32) {
    (r * 0.2, r * 0.55, r)
}

/// The wedge at screen point `p` of a wheel at `c` with radius `r`.
pub fn wedge_at(c: Pos2, r: f32, p: Pos2) -> Option<Wedge> {
    let d = p - c;
    let dist = d.length();
    let (hub, inner, outer) = rings(r);
    if dist > outer {
        return None;
    }
    if dist <= hub {
        return Some(Wedge::Hub);
    }
    // Screen y points down: counterclockwise angles come from -y.
    let a = (-d.y).atan2(d.x).to_degrees().rem_euclid(360.0);
    let off = |centre: f32| ((a - centre + 540.0).rem_euclid(360.0) - 180.0).abs();
    if dist <= inner {
        INNER.iter().find(|(_, at)| off(*at) <= 60.0).map(|(w, _)| *w)
    } else {
        OUTER.iter().find(|(_, at)| off(*at) <= 45.0).map(|(w, _)| *w)
    }
}

/// Where a wedge's label sits.
pub fn wedge_center(c: Pos2, r: f32, w: Wedge) -> Pos2 {
    let (hub, inner, outer) = rings(r);
    let (at, rad) = match (OUTER.iter().find(|(x, _)| *x == w), INNER.iter().find(|(x, _)| *x == w)) {
        (Some((_, a)), _) => (*a, (inner + outer) / 2.0),
        (_, Some((_, a))) => (*a, (hub + inner) / 2.0),
        _ => return c,
    };
    let t = at.to_radians();
    c + vec2(t.cos(), -t.sin()) * rad
}

/// The × that closes the wheel.
fn close_rect(c: Pos2, r: f32) -> Rect {
    Rect::from_center_size(c + vec2(r * 0.82, -r * 0.82), vec2(16.0, 16.0))
}

/// Open the wheel at the pointer (else the middle of the drawing area).
pub fn open(app: &mut CadApp, ctx: &egui::Context) {
    let canvas = app.canvas.xf.map(|x| x.rect);
    let at = ctx.pointer_latest_pos().filter(|p| canvas.is_none_or(|r| r.contains(*p))).or(canvas.map(|r| r.center())).unwrap_or(pos2(400.0, 300.0));
    app.wheel = Some(WheelUi { center: at, drag: None });
}

/// NAVSWHEEL and the Walk wedge from anywhere (menus, command line, control channel).
pub fn route(app: &mut CadApp, ctx: Option<&egui::Context>, id: &str, params: &Value) -> Option<Result<Value, String>> {
    match id {
        "navswheel" | "wheel" => {
            let r = app.session.execute("navswheel", params).map_err(|e| e.user_message());
            if r.is_ok() {
                match ctx {
                    Some(ctx) => open(app, ctx),
                    None => open(app, &egui::Context::default()),
                }
            }
            Some(r)
        }
        "navswheel.walk" => {
            app.wheel = None;
            Some(app.run("3dwalk", json!({})))
        }
        _ => None,
    }
}

fn run(app: &mut CadApp, id: &str, params: Value) {
    if let Err(e) = app.run(id, params) {
        app.session.echo(e);
    }
}

/// The drawing point under screen point `p` (on the XY plane; in a 3D view the view-plane point
/// where the XY plane is seen edge-on).
fn world(xf: &Xf, p: Pos2) -> [f64; 2] {
    let w = xf.s2w(p);
    [w.x, w.y]
}

/// Show the wheel and act on it. Returns whether it holds the pointer.
pub fn show(app: &mut CadApp, ui: &mut egui::Ui, rect: Rect, xf: &Xf) {
    let Some(mut w) = app.wheel.clone() else { return };
    let r = radius(app);
    let ctx = ui.ctx().clone();
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.wheel = None;
        return;
    }
    // Keep the wheel on the drawing area.
    w.center = pos2(w.center.x.clamp(rect.left() + r, rect.right() - r), w.center.y.clamp(rect.top() + r, rect.bottom() - r));
    let bounds = Rect::from_center_size(w.center, vec2(2.0 * r + 24.0, 2.0 * r + 24.0));
    let resp = ui.interact(bounds, ui.id().with("navswheel"), Sense::click_and_drag());
    crate::icons::describe_control(ui, &resp, "SteeringWheel", None, false);
    let hover = resp.hover_pos().and_then(|p| wedge_at(w.center, r, p));
    let close = close_rect(w.center, r);
    // Pressing a wedge starts it; dragging works it; letting go ends it.
    // egui starts a drag once the pointer has moved: the wedge and the drag start where the
    // button went down.
    if resp.drag_started()
        && let Some(p) = ctx.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos())
        && let Some(wedge) = wedge_at(w.center, r, p)
    {
        w.drag = Some((wedge, p, p));
        let first = |app: &mut CadApp, id: &str, params: Value| run(app, id, params);
        // The view each drag starts from is kept for Rewind: the drag's first step records it.
        match wedge {
            Wedge::Zoom => first(app, "navswheel.zoom", json!({ "factor": 1.0, "at": world(xf, p) })),
            Wedge::Pan => first(app, "navswheel.pan", json!({ "by": [0.0, 0.0] })),
            Wedge::Orbit if xf.proj.is_some() || app.session.layout_space() == cadaddy_doc::Space::Model => {
                first(app, "navswheel.orbit", json!({ "by": [0.0, 0.0] }))
            }
            Wedge::Look => first(app, "navswheel.look", json!({ "by": [0.0, 0.0] })),
            _ => {}
        }
    }
    if let Some((wedge, start, last)) = w.drag
        && let Some(now) = resp.interact_pointer_pos()
        && resp.dragged()
    {
        let d = now - last;
        if d != egui::Vec2::ZERO {
            let (dx, dy) = (f64::from(d.x), f64::from(d.y));
            match wedge {
                // Up zooms in, about the point pressed.
                Wedge::Zoom => run(app, "navswheel.zoom", json!({ "factor": (-dy * 0.01).exp(), "at": world(xf, start), "record": false })),
                // The drawing follows the pointer.
                Wedge::Pan => run(app, "navswheel.pan", json!({ "by": [dx / xf.scale, -dy / xf.scale], "record": false })),
                // Right turns the model right (the viewer goes left); up tips it towards the viewer.
                Wedge::Orbit => run(app, "navswheel.orbit", json!({ "by": [-dx * 0.4, dy * 0.4], "record": false })),
                Wedge::Look => run(app, "navswheel.look", json!({ "by": [-dx * 0.2, -dy * 0.2], "record": false })),
                // Moving the wheel by its hub.
                Wedge::Hub => w.center += d,
                _ => {}
            }
            w.drag = Some((wedge, start, now));
        }
    }
    if resp.drag_stopped() {
        if let Some((Wedge::Center, _, last)) = w.drag {
            run(app, "navswheel.center", json!({ "point": world(xf, last) }));
        }
        w.drag = None;
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        if close.contains(p) {
            app.wheel = None;
            return;
        }
        match wedge_at(w.center, r, p) {
            Some(Wedge::Rewind) => run(app, "navswheel.rewind", json!({})),
            Some(Wedge::Walk) => {
                run(app, "navswheel.walk", json!({}));
                return;
            }
            Some(Wedge::Center) => run(app, "navswheel.center", json!({ "point": world(xf, p) })),
            _ => {}
        }
    }
    if resp.secondary_clicked() {
        app.wheel = None;
        return;
    }
    let resp = match hover {
        Some(Wedge::Hub) => resp.on_hover_cursor(CursorIcon::Grab),
        Some(x) => resp.on_hover_cursor(CursorIcon::PointingHand).on_hover_text(crate::i18n::t(match x {
            Wedge::Zoom => "Zoom: drag up or down",
            Wedge::Orbit => "Orbit: drag to turn the model",
            Wedge::Rewind => "Rewind: back to the view before",
            Wedge::Pan => "Pan: drag the drawing",
            Wedge::Center => "Center: click or drag onto a point",
            Wedge::Walk => "Walk",
            Wedge::Look => "Look: drag to look around",
            Wedge::Hub => "",
        })),
        None => resp,
    };
    let _ = resp;
    let active = w.drag.map(|(x, ..)| x).or(hover);
    paint(app, ui, &w, r, active);
    app.wheel = Some(w);
}

fn paint(app: &CadApp, ui: &egui::Ui, w: &WheelUi, r: f32, active: Option<Wedge>) {
    let p = ui.painter();
    let alpha = (f32::from(app.session.settings.navswheel_opacity.clamp(25, 90)) / 100.0 * 255.0) as u8;
    let (hub, inner, outer) = rings(r);
    let c = w.center;
    let ring = |rad_in: f32, rad_out: f32, from: f32, to: f32| -> Vec<Pos2> {
        let n = 16;
        let mut pts = Vec::with_capacity(2 * n + 2);
        for i in 0..=n {
            let t = (from + (to - from) * i as f32 / n as f32).to_radians();
            pts.push(c + vec2(t.cos(), -t.sin()) * rad_out);
        }
        for i in (0..=n).rev() {
            let t = (from + (to - from) * i as f32 / n as f32).to_radians();
            pts.push(c + vec2(t.cos(), -t.sin()) * rad_in);
        }
        pts
    };
    let base = Color32::from_rgba_unmultiplied(0x2b, 0x31, 0x3a, alpha);
    let lit = Color32::from_rgba_unmultiplied(0x3d, 0x7f, 0xd0, alpha.max(200));
    let inner_col = Color32::from_rgba_unmultiplied(0x3a, 0x42, 0x4e, alpha);
    let edge = Stroke::new(1.0, Color32::from_rgba_unmultiplied(0x9a, 0xa4, 0xb0, alpha));
    // A wedge isn't convex: it is filled as a strip of quadrilaterals, then outlined.
    let wedge = |rad_in: f32, rad_out: f32, from: f32, to: f32, fill: Color32| {
        let pts = ring(rad_in, rad_out, from, to);
        let n = pts.len() / 2;
        for i in 0..n.saturating_sub(1) {
            let (Some(a), Some(b), Some(c2), Some(d)) = (pts.get(i), pts.get(i + 1), pts.get(pts.len() - 2 - i), pts.get(pts.len() - 1 - i)) else {
                continue;
            };
            p.add(egui::Shape::convex_polygon(vec![*a, *b, *c2, *d], fill, Stroke::NONE));
        }
        p.add(egui::Shape::closed_line(pts, edge));
    };
    for (w_, at) in OUTER {
        wedge(inner, outer, at - 45.0, at + 45.0, if active == Some(w_) { lit } else { base });
    }
    for (w_, at) in INNER {
        wedge(hub, inner, at - 60.0, at + 60.0, if active == Some(w_) { lit } else { inner_col });
    }
    p.circle(c, hub, if active == Some(Wedge::Hub) { lit } else { base }, edge);
    let text = Color32::from_rgb(0xe6, 0xea, 0xef);
    for (wedge, _) in OUTER.iter().chain(INNER.iter()) {
        let size = if OUTER.iter().any(|(x, _)| x == wedge) { 11.0 } else { 9.0 } * (r / 68.0).clamp(0.8, 1.3);
        p.text(wedge_center(c, r, *wedge), egui::Align2::CENTER_CENTER, crate::i18n::t(wedge.label()), egui::FontId::proportional(size), text);
    }
    // The ×.
    let x = close_rect(c, r);
    p.circle_filled(x.center(), 8.0, Color32::from_rgba_unmultiplied(0x2b, 0x31, 0x3a, alpha.max(160)));
    let s = Stroke::new(1.4, text);
    p.line_segment([x.center() + vec2(-3.5, -3.5), x.center() + vec2(3.5, 3.5)], s);
    p.line_segment([x.center() + vec2(-3.5, 3.5), x.center() + vec2(3.5, -3.5)], s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wedges_are_where_they_are_drawn() {
        let c = pos2(300.0, 300.0);
        let r = 68.0;
        for w in [Wedge::Zoom, Wedge::Orbit, Wedge::Rewind, Wedge::Pan, Wedge::Center, Wedge::Walk, Wedge::Look] {
            assert_eq!(wedge_at(c, r, wedge_center(c, r, w)), Some(w), "{w:?}");
        }
        assert_eq!(wedge_at(c, r, c), Some(Wedge::Hub));
        assert_eq!(wedge_at(c, r, c + vec2(0.0, -60.0)), Some(Wedge::Zoom), "up is ZOOM");
        assert_eq!(wedge_at(c, r, c + vec2(r + 5.0, 0.0)), None);
    }
}
