//! The palette rail: a slim strip of icon buttons down the window's edge (the left, the right in
//! a right-to-left interface), one for every palette. The drawing gets the window; a button
//! opens its palette beside the rail and closes the one that was open, unless that one is
//! pinned; the button again, or the palette's hide button, puts it away.
//! It drives the `ui.dock` command (`toggle`, `pin`), so agents and scripts do the same.

use crate::CadApp;
use crate::docking::Panel;
use crate::icons::{self, Icon};
use crate::theme::Tokens;
use serde_json::json;

/// The rail's width, points.
pub const WIDTH: f32 = 34.0;
/// A rail button's size, points.
const BUTTON: f32 = 28.0;

/// The icon a palette's rail button shows.
pub fn icon(panel: Panel) -> Icon {
    match panel {
        Panel::Canvas => Icon::Workspace,
        Panel::PlansToModel => Icon::PlansToModel,
        Panel::ToolSets => Icon::ToolSets,
        Panel::QuickCalc => Icon::Calculator,
        Panel::Layers => Icon::Layers,
        Panel::Properties => Icon::Properties,
        Panel::ExternalRefs => Icon::Attach,
        Panel::Symbols => Icon::Blocks,
        Panel::SheetSets => Icon::SheetSet,
        Panel::Coordination => Icon::Coordination,
        Panel::Validate => Icon::Validate,
        Panel::Job => Icon::Job,
    }
}

/// Draw the rail at the window's edge (call before the central area).
pub fn show(app: &mut CadApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let side = if app.rtl() { egui::Panel::right("cc_palette_rail") } else { egui::Panel::left("cc_palette_rail") };
    let mut clicked = None;
    side.exact_size(WIDTH).resizable(false).frame(egui::Frame::NONE.fill(t.chrome)).show(ui, |ui| {
        let r = ui.max_rect();
        // A hairline between the rail and the drawing or palette beside it.
        let edge = if app.rtl() { r.left() } else { r.right() - 0.5 };
        ui.painter().line_segment([egui::pos2(edge, r.top()), egui::pos2(edge, r.bottom())], egui::Stroke::new(1.0, t.border));
        egui::ScrollArea::vertical().id_salt("palette-rail").scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(
            ui,
            |ui| {
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 4.0);
                    let ws = &app.ui.docking;
                    for &p in Panel::RAIL {
                        let shown = ws.layout.contains(&p) || ws.is_detached(p);
                        let pinned = ws.pinned.contains(&p);
                        let resp = icons::button(ui, icon(p), BUTTON, p.label(), shown);
                        if pinned {
                            // A pinned palette's button carries a dot at its corner.
                            let c = egui::pos2(resp.rect.right() - 4.0, resp.rect.top() + 4.0);
                            ui.painter().circle_filled(c, 2.5, t.accent);
                        }
                        if resp.clicked() {
                            clicked = Some(p);
                        }
                    }
                });
            },
        );
    });
    if let Some(p) = clicked
        && let Err(e) = app.run("ui.dock", json!({ "operation": "toggle", "panel": p.id() }))
    {
        app.set_status(format!("Palette: {e}"));
    }
}
