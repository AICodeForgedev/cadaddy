//! 3D object snaps and the 3D edit handles on the canvas: the snap's marker where the point in
//! space it found shows on screen, and at a 3DMOVE / 3DROTATE / 3DSCALE base point the X, Y and
//! Z handles (red, green, blue) — clicking one holds the edit to that axis (it types X, Y or Z).

use cadaddy_engine::Input;
use cadaddy_engine::prompt::{Gizmo, GizmoKind};
use cadaddy_engine::snap3::{Snap3, Snap3Kind};
use cadaddy_geom::Vec3;
use egui::{Color32, Painter, Pos2, Shape, Stroke, pos2, vec2};

use crate::CadApp;
use crate::canvas::Xf;

/// Where a point in space shows on screen.
pub fn w3s(xf: &Xf, p: Vec3) -> Pos2 {
    match xf.proj {
        Some(pr) => xf.to_screen(pr.project(p)),
        None => xf.to_screen(p.xy()),
    }
}

/// The 3D snap under the cursor (`hover`), when no plan snap holds it.
pub fn update(app: &mut CadApp, xf: &Xf, hover: Option<Pos2>) {
    app.canvas.snap3 = match (hover, app.canvas.snap) {
        (Some(h), None) => app.session.osnap3_at(xf.to_world(h)),
        _ => None,
    };
}

/// The point a click places: a plan snap, else a 3D snap's point in space, else the plain point.
pub fn click_input(app: &CadApp, p: cadaddy_geom::Vec2) -> Option<Input> {
    if app.canvas.snap.is_some() {
        return None;
    }
    Some(match app.canvas.snap3 {
        Some(s) => Input::Point3(s.at),
        None => Input::Point(p),
    })
}

pub fn draw_marker(p: &Painter, xf: &Xf, s: &Snap3) {
    let at = w3s(xf, s.at);
    let c = Color32::from_rgb(64, 200, 255);
    let st = Stroke::new(2.0, c);
    let r = 6.0;
    match s.kind {
        Snap3Kind::Vertex => {
            p.rect_stroke(egui::Rect::from_center_size(at, vec2(2.0 * r, 2.0 * r)), 0.0, st, egui::StrokeKind::Middle);
        }
        Snap3Kind::Midpoint => {
            p.add(Shape::closed_line(vec![pos2(at.x, at.y - r), pos2(at.x + r, at.y + r), pos2(at.x - r, at.y + r)], st));
        }
        Snap3Kind::Center => {
            p.circle_stroke(at, r, st);
            p.circle_filled(at, 1.5, c);
        }
        Snap3Kind::Perpendicular => {
            p.line_segment([pos2(at.x - r, at.y + r), pos2(at.x + r, at.y + r)], st);
            p.line_segment([pos2(at.x - r, at.y + r), pos2(at.x - r, at.y - r)], st);
            p.line_segment([pos2(at.x - r, at.y), pos2(at.x, at.y)], st);
        }
        Snap3Kind::Nearest => {
            p.line_segment([pos2(at.x - r, at.y - r), pos2(at.x + r, at.y + r)], st);
            p.line_segment([pos2(at.x - r, at.y + r), pos2(at.x + r, at.y - r)], st);
        }
        // The face dynamic UCS draws on: a small square lying in it.
        Snap3Kind::OnFace => {
            if let Some(n) = s.normal {
                let (u, v) = basis(n);
                let k = f64::from(r) / xf.scale.max(1e-12);
                let q = [u + v, u - v, v * -1.0 - u, v - u].map(|d| w3s(xf, s.at + d * k));
                p.add(Shape::closed_line(q.to_vec(), Stroke::new(1.5, c)));
            }
            p.circle_filled(at, 2.0, c);
        }
    }
}

fn basis(n: Vec3) -> (Vec3, Vec3) {
    let a = if n.x.abs() < 0.9 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
    let u = a.cross(n).normalized();
    (u, n.cross(u).normalized())
}

const AXES: [(Vec3, Color32, &str); 3] = [
    (Vec3 { x: 1.0, y: 0.0, z: 0.0 }, Color32::from_rgb(230, 70, 70), "X"),
    (Vec3 { x: 0.0, y: 1.0, z: 0.0 }, Color32::from_rgb(80, 200, 90), "Y"),
    (Vec3 { x: 0.0, y: 0.0, z: 1.0 }, Color32::from_rgb(80, 130, 240), "Z"),
];

/// Handle length on screen, points.
const LEN: f32 = 70.0;

/// Each axis handle's ends on screen (`None`: the axis points at the viewer).
fn handles(xf: &Xf, g: &Gizmo) -> [Option<(Pos2, Pos2)>; 3] {
    let o = w3s(xf, g.at);
    AXES.map(|(a, _, _)| {
        let tip = w3s(xf, g.at + a * (1.0 / xf.scale.max(1e-12)));
        let d = tip - o;
        (d.length() > 1e-3).then(|| (o, o + d.normalized() * LEN))
    })
}

pub fn draw_gizmo(p: &Painter, xf: &Xf, g: &Gizmo) {
    let o = w3s(xf, g.at);
    for (i, h) in handles(xf, g).into_iter().enumerate() {
        let (_, c, label) = AXES[i];
        let held = g.axis == Some(i as u8);
        let st = Stroke::new(if held { 4.0 } else { 2.0 }, c);
        let Some((a, b)) = h else {
            // Seen end on: a ring around the base point.
            p.circle_stroke(o, 10.0 + i as f32 * 3.0, Stroke::new(1.0, c));
            continue;
        };
        p.line_segment([a, b], st);
        let d = (b - a).normalized();
        let side = vec2(-d.y, d.x);
        match g.kind {
            GizmoKind::Move => {
                p.add(Shape::convex_polygon(vec![b + d * 10.0, b + side * 5.0, b - side * 5.0], c, Stroke::NONE));
            }
            GizmoKind::Scale => {
                p.rect_filled(egui::Rect::from_center_size(b, vec2(9.0, 9.0)), 0.0, c);
            }
            GizmoKind::Rotate => {
                // The turn about this axis: a ring in the plane square to it.
                let (u, v) = basis(AXES[i].0);
                let k = 30.0 / xf.scale.max(1e-12);
                let ring: Vec<Pos2> = (0..=32)
                    .map(|j| {
                        let t = std::f64::consts::TAU * f64::from(j) / 32.0;
                        w3s(xf, g.at + (u * t.cos() + v * t.sin()) * k * (1.0 + i as f64 * 0.12))
                    })
                    .collect();
                p.add(Shape::line(ring, Stroke::new(if held { 3.0 } else { 1.5 }, c)));
            }
        }
        p.text(b + d * 16.0, egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(12.0), c);
    }
}

/// The axis whose handle is under `at` (screen), within a few points.
pub fn handle_at(xf: &Xf, g: &Gizmo, at: Pos2) -> Option<&'static str> {
    handles(xf, g).into_iter().enumerate().find_map(|(i, h)| {
        let (a, b) = h?;
        let ab = b - a;
        let t = ((at - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
        // The shaft's outer part only: the base point itself stays pickable.
        (t > 0.3 && (a + ab * t - at).length() <= 7.0).then_some(AXES[i].2)
    })
}
