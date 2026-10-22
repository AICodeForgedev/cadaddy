//! The annotation monitor's badges (ANNOMONITOR): a dimension that lost its association
//! because its object was erased shows a badge by its text. Clicking the badge offers
//! Reassociate (DIMREASSOCIATE on it) and Disassociate (DIMDISASSOCIATE: accept it as it is).
//!
//! The engine keeps the flag (`DimMarks::orphaned`, `cadaddy_engine::assoc::orphans`); this
//! module only draws the badges and runs the commands.

use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use serde_json::json;

use crate::CadApp;
use crate::canvas::Xf;

/// Badges drawn at most (a drawing that lost thousands of associations shows the first ones).
const MAX_BADGES: usize = 500;

/// The badge: an amber triangle with an exclamation mark, drawn in code.
pub fn badge(painter: &egui::Painter, r: Rect) {
    let amber = Color32::from_rgb(0xf2, 0xb1, 0x34);
    let ink = Color32::from_rgb(0x20, 0x20, 0x20);
    let (c, h) = (r.center(), r.height());
    let tri = vec![pos2(c.x, r.top()), pos2(r.right(), r.bottom()), pos2(r.left(), r.bottom())];
    painter.add(Shape::convex_polygon(tri, amber, Stroke::new(1.0, ink)));
    painter.line_segment([pos2(c.x, r.top() + h * 0.35), pos2(c.x, r.top() + h * 0.68)], Stroke::new(1.6, ink));
    painter.circle_filled(pos2(c.x, r.top() + h * 0.82), 1.1, ink);
}

/// Draw the badges of the current space and handle clicks on them.
pub fn draw_overlay(app: &mut CadApp, ui: &mut egui::Ui, painter: &egui::Painter, xf: &Xf) {
    let Ok(d) = app.session.doc() else { return };
    if !cadaddy_engine::assoc::monitor_on(d) {
        return;
    }
    let orphans = cadaddy_engine::assoc::orphans(d, &app.session.space());
    let mut chosen: Option<(&'static str, cadaddy_engine::doc::Handle)> = None;
    for (h, at) in orphans.into_iter().take(MAX_BADGES) {
        let p: Pos2 = xf.to_screen(at) + vec2(10.0, -14.0);
        if !painter.clip_rect().contains(p) {
            continue;
        }
        let r = Rect::from_center_size(p, vec2(14.0, 13.0));
        badge(painter, r);
        let resp = ui.interact(r.expand(2.0), ui.id().with(("annomon", h.0)), Sense::click());
        let resp = resp.on_hover_text(crate::tl!("This dimension lost its association: an object it measured was erased. Click to reassociate it."));
        resp.context_menu(|ui| {
            if ui.button(crate::tl!("Reassociate")).clicked() {
                chosen = Some(("dimreassociate", h));
                ui.close();
            }
            if ui.button(crate::tl!("Disassociate")).clicked() {
                chosen = Some(("dimdisassociate", h));
                ui.close();
            }
        });
        if resp.clicked() {
            chosen = Some(("dimreassociate", h));
        }
    }
    if let Some((id, h)) = chosen {
        let _ = app.run(id, json!({ "handles": [h.hex()] }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badges_follow_the_monitor() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.run("line", json!({ "points": [[0, 0], [10, 0]] })).unwrap();
        let l = app.session.doc().unwrap().model.last().unwrap().handle;
        app.run("dimlinear", json!({ "p1": [0, 0], "p2": [10, 0], "at": [5, -2] })).unwrap();
        app.run("erase", json!({ "handles": [l.hex()] })).unwrap();
        let d = app.session.doc().unwrap();
        assert!(cadaddy_engine::assoc::monitor_on(d));
        assert_eq!(cadaddy_engine::assoc::orphans(d, &app.session.space()).len(), 1);
    }
}
