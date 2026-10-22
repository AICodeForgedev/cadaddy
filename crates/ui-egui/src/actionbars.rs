//! The Block Editor's action bars (BACTIONBARMODE 1): under each parameter, a bar with one cell
//! per action that follows it. Right-click a bar to hide it, or to show every bar again
//! (BACTIONBAR). The engine reports the bars (`cadaddy_engine::blockedit::action_bars`).

use egui::{Color32, Rect, Sense, Stroke, vec2};
use serde_json::{Value, json};

use crate::CadApp;
use crate::canvas::Xf;
use crate::theme::Tokens;

/// The short label of an action type in its cell.
fn abbreviation(kind: &str) -> &'static str {
    match kind {
        "move" => "Mv",
        "stretch" => "St",
        "polarStretch" => "PS",
        "scale" => "Sc",
        "rotate" => "Ro",
        "flip" => "Fl",
        "array" => "Ar",
        "lookup" => "Lk",
        _ => "Ac",
    }
}

/// Draw the action bars and handle their menus.
pub fn draw_overlay(app: &mut CadApp, ui: &mut egui::Ui, painter: &egui::Painter, xf: &Xf) {
    let editing = app.session.state().ok().and_then(|st| st.block_edit.as_ref().map(|b| !b.current)).unwrap_or(false);
    if !editing {
        return;
    }
    let Ok(r) = cadaddy_engine::blockedit::action_bars(&app.session) else { return };
    if r.get("mode").and_then(Value::as_i64) != Some(1) {
        return;
    }
    let t = Tokens::get();
    let mut chosen: Option<Value> = None;
    let bars = r.get("bars").and_then(Value::as_array).cloned().unwrap_or_default();
    for b in bars.iter().take(500) {
        let actions = b.get("actions").and_then(Value::as_array).cloned().unwrap_or_default();
        if actions.is_empty() || b.get("shown").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let at = b.get("at").and_then(Value::as_array);
        let (Some(x), Some(y)) = (at.and_then(|a| a.first()).and_then(Value::as_f64), at.and_then(|a| a.get(1)).and_then(Value::as_f64)) else {
            continue;
        };
        let node = b.get("node").cloned().unwrap_or(Value::Null);
        let anchor = xf.to_screen(cadaddy_geom::Vec2::new(x, y)) + vec2(0.0, 22.0);
        let cell = vec2(24.0, 18.0);
        let n = actions.len().min(20);
        let width = cell.x * n as f32 + 4.0;
        let bar = Rect::from_min_size(anchor - vec2(width / 2.0, 0.0), vec2(width, cell.y + 4.0));
        if !painter.clip_rect().intersects(bar) {
            continue;
        }
        painter.rect_filled(bar, 3.0, t.panel);
        painter.rect_stroke(bar, 3.0, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
        for (i, a) in actions.iter().take(n).enumerate() {
            let r = Rect::from_min_size(bar.min + vec2(2.0 + cell.x * i as f32, 2.0), cell);
            let kind = a.get("type").and_then(Value::as_str).unwrap_or("");
            let name = a.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            painter.rect_filled(r.shrink(1.0), 2.0, Color32::from_rgb(0x3a, 0x6e, 0xa5));
            painter.text(r.center(), egui::Align2::CENTER_CENTER, abbreviation(kind), crate::theme::small(), Color32::WHITE);
            let resp = ui.interact(r, ui.id().with(("actionbar", node.to_string(), i)), Sense::click());
            resp.on_hover_text(name);
        }
        let resp = ui.interact(bar, ui.id().with(("actionbar", node.to_string())), Sense::click());
        resp.context_menu(|ui| {
            if ui.button(crate::tl!("Hide Action Bar")).clicked() {
                chosen = Some(json!({ "action": "hide", "parameters": [node.clone()] }));
                ui.close();
            }
            if ui.button(crate::tl!("Reset Action Bars")).clicked() {
                chosen = Some(json!({ "action": "reset" }));
                ui.close();
            }
        });
    }
    if let Some(p) = chosen {
        let _ = app.run("bactionbar", p);
    }
}
