//! The Symbols palette: CADaddy's built-in symbol library (plumbing fixtures and piping symbols)
//! as pictures, grouped by category, with a search box. Clicking a symbol starts SYMBOL with it:
//! the user then picks the insertion point and rotation and fills in its tag.

use std::sync::Arc;

use cadaddy_engine::Input;
use cadaddy_engine::cmd::symbol::{SymbolPreview, previews};
use cadaddy_engine::geom::Vec2;
use egui::{Rect, Sense, Stroke, pos2, vec2};

use crate::CadApp;
use crate::theme::Tokens;

/// A tile's side, in points.
const CELL: f32 = 60.0;
/// Space between tiles.
const GAP: f32 = 6.0;

thread_local! {
    static PREVIEWS: Arc<Vec<SymbolPreview>> = Arc::new(previews());
}

fn search_id() -> egui::Id {
    egui::Id::new("cc_symbols_search")
}

/// Draw a symbol's preview to fit in `r` (y up in the drawing, down on screen).
fn paint(p: &egui::Painter, r: Rect, s: &SymbolPreview, hovered: bool) {
    let t = Tokens::get();
    p.rect_filled(r, 4.0, if hovered { t.control_hover } else { t.field });
    let inner = r.shrink(7.0);
    let b = s.bounds;
    if b.is_empty() {
        return;
    }
    let k = (f64::from(inner.width()) / b.width().max(1e-9)).min(f64::from(inner.height()) / b.height().max(1e-9));
    let c = b.center();
    let at = |v: &Vec2| pos2(inner.center().x + ((v.x - c.x) * k) as f32, inner.center().y - ((v.y - c.y) * k) as f32);
    let stroke = Stroke::new(1.0, t.text);
    for run in &s.lines {
        if run.len() >= 2 {
            p.add(egui::Shape::line(run.iter().map(at).collect(), stroke));
        }
    }
    for tri in &s.fills {
        p.add(egui::Shape::convex_polygon(tri.iter().map(at).collect(), t.text, Stroke::NONE));
    }
}

/// Start SYMBOL with `name`, ending a command still waiting for input first.
fn place(app: &mut CadApp, name: &str) {
    if app.session.running.is_some() {
        app.session.cancel();
    }
    app.start("symbol");
    if let Err(e) = app.session.input(Input::Text(name.to_string())) {
        app.session.echo(e.to_string());
    }
}

pub(crate) fn symbols_section(app: &mut CadApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let all = PREVIEWS.with(Arc::clone);
    let mut query: String = ui.data(|d| d.get_temp(search_id())).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let edit = egui::TextEdit::singleline(&mut query).hint_text(crate::tl!("Search symbols")).desired_width(ui.available_width() - 10.0);
        if ui.add(edit).changed() {
            ui.data_mut(|d| d.insert_temp(search_id(), query.clone()));
        }
    });
    ui.add_space(4.0);
    let words = query.trim().to_ascii_lowercase();
    let shown: Vec<&SymbolPreview> = all
        .iter()
        .filter(|s| {
            let hay = format!("{} {} {} {}", s.name, s.label, s.category, s.keywords).to_ascii_lowercase();
            words.split_whitespace().all(|w| hay.contains(w))
        })
        .collect();
    if shown.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(10.0);
            ui.label(egui::RichText::new(crate::tl!("No symbols match.")).color(t.text_dim));
        });
        return;
    }
    let mut clicked = None;
    let mut category = "";
    let width = ui.available_width() - 16.0;
    let per_row = ((width + GAP) / (CELL + GAP)).floor().max(1.0) as usize;
    let mut i = 0;
    while i < shown.len() {
        let Some(first) = shown.get(i) else { break };
        if first.category != category {
            category = first.category;
            ui.horizontal(|ui| {
                ui.add_space(10.0);
                ui.label(egui::RichText::new(crate::i18n::t(category)).color(t.text_dim).size(11.0));
            });
        }
        // One row: tiles of this category only.
        let row: Vec<&SymbolPreview> = shown.iter().skip(i).take(per_row).take_while(|s| s.category == category).copied().collect();
        i += row.len().max(1);
        let (rr, _) = ui.allocate_exact_size(vec2(ui.available_width(), CELL + GAP), Sense::hover());
        for (j, s) in row.iter().enumerate() {
            let r = Rect::from_min_size(pos2(rr.left() + 8.0 + j as f32 * (CELL + GAP), rr.top()), vec2(CELL, CELL));
            let resp = ui.interact(r, ui.id().with(("symbol", s.name)), Sense::click());
            paint(ui.painter(), r, s, resp.hovered());
            crate::icons::describe_control(ui, &resp, s.label, None, false);
            if resp.on_hover_text(crate::i18n::t(s.label)).clicked() {
                clicked = Some(s.name);
            }
        }
    }
    if let Some(name) = clicked {
        place(app, name);
    }
}
