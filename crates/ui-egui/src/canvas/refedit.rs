//! Editing a reference in place (REFEDIT) on the canvas: a bar with the block's name and Add to
//! Working Set, Save Reference Edits and Discard Reference Edits (the rest of the drawing is
//! drawn faded by the renderer).

use egui::{Rect, RichText, Stroke, vec2};
use serde_json::json;

use crate::CadApp;
use crate::theme::Tokens;

fn id() -> egui::Id {
    egui::Id::new("cadaddy-refedit-bar")
}

/// The block being edited in place in the drawing shown, and whether it is an xref.
fn editing(app: &CadApp) -> Option<(String, bool)> {
    let d = &app.session.state().ok()?.doc;
    let r = d.refedit.as_ref()?;
    Some((r.block.clone(), d.block(&r.block).is_some_and(|b| b.xref_path.is_some())))
}

/// The REFEDIT bar (over the drawing).
pub(super) fn paint(app: &mut CadApp, ctx: &egui::Context, rect: Rect) {
    let Some((name, xref)) = editing(app) else { return };
    let t = Tokens::get();
    let mut action: Option<&str> = None;
    egui::Area::new(id()).order(egui::Order::Foreground).pivot(egui::Align2::CENTER_TOP).fixed_pos(rect.center_top() + vec2(0.0, 8.0)).show(
        ctx,
        |ui| {
            egui::Frame::new().fill(t.cmd_bg).stroke(Stroke::new(1.0, t.accent)).corner_radius(4.0).inner_margin(6.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(crate::tl!("Edit Reference")).color(t.accent).strong());
                    ui.label(RichText::new(&name).color(t.text));
                    if xref {
                        ui.label(RichText::new(crate::tl!("Xref")).color(t.text_dim));
                    }
                    ui.separator();
                    if ui.button(crate::tl!("Add to Working Set")).clicked() {
                        action = Some("refset");
                    }
                    if ui.button(crate::tl!("Save Reference Edits")).clicked() {
                        action = Some("save");
                    }
                    if ui.button(crate::tl!("Discard Reference Edits")).clicked() {
                        action = Some("discard");
                    }
                });
            });
        },
    );
    match action {
        Some("refset") => app.start("refset"),
        Some(option) => {
            let _ = app.run("refclose", json!({ "option": option }));
        }
        None => {}
    }
}
