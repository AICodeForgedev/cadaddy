//! Selected groups as GROUPDISPLAYMODE shows them: with 1, one grip at the group's centre instead
//! of every member's grips; with 2 (the default), that grip and the group's bounding box. Dragging
//! the grip moves the group ([`cadaddy_engine::Session::start_group_grip`]).

use egui::{Color32, Painter, Pos2, Rect, Stroke, vec2};

use crate::CadApp;
use crate::canvas::Xf;
use crate::theme::Tokens;
use cadaddy_engine::cmd::groups::display::{ShownGroup, shown_groups};

/// The grip's screen square (half side `gs`).
fn grip_rect(xf: &Xf, g: &ShownGroup, gs: f32) -> Rect {
    Rect::from_center_size(xf.w2s(g.center), vec2(gs * 2.0, gs * 2.0))
}

/// The group whose grip is under `hover`, if any (its index in the drawing's groups).
pub(crate) fn at(app: &CadApp, xf: &Xf, hover: Option<Pos2>) -> Option<usize> {
    let p = hover?;
    let gs = app.session.settings.gripsize as f32;
    shown_groups(&app.session).into_iter().find(|g| grip_rect(xf, g, gs).expand(3.0).contains(p)).map(|g| g.index)
}

/// A click on the grip of group `index`: drag the group.
pub(crate) fn click(app: &mut CadApp, index: usize) {
    app.canvas.hot_grips.clear();
    if let Err(e) = app.session.start_group_grip(index) {
        app.session.echo(e.to_string());
    }
}

/// Draw the shown groups: the bounding box (GROUPDISPLAYMODE 2) and the grip at the centre, warm
/// under the cursor.
pub(crate) fn draw(app: &CadApp, painter: &Painter, xf: &Xf, rect: Rect, hover: Option<Pos2>) {
    let t = Tokens::get();
    let gs = app.session.settings.gripsize as f32;
    let boxes = app.session.settings.groupdisplaymode == 2;
    for g in shown_groups(&app.session) {
        if boxes {
            let r = Rect::from_two_pos(xf.w2s(g.bounds.min), xf.w2s(g.bounds.max));
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, t.grip), egui::StrokeKind::Middle);
        }
        let gr = grip_rect(xf, &g, gs);
        if !rect.contains(gr.center()) {
            continue;
        }
        let warm = hover.is_some_and(|p| gr.expand(3.0).contains(p));
        painter.rect_filled(gr, 0.0, if warm { Color32::from_rgb(0xff, 0x7f, 0x9f) } else { t.grip });
        painter.rect_stroke(gr, 0.0, Stroke::new(1.0, Color32::from_rgb(0x10, 0x30, 0x80)), egui::StrokeKind::Middle);
    }
}
