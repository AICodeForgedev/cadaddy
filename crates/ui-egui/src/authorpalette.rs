//! The Block Authoring Palettes (BAUTHORPALETTE) and the test window's bar (BTESTBLOCK): while
//! the Block Editor is open on a block, a floating window with a Parameters tab and an Actions
//! tab offers a tool for each parameter and action type. A tool starts BPARAMETER or BACTIONTOOL
//! with its type, and the prompts take the picks in the drawing. While the test window is open a
//! bar says so and closes it. The engine lists the tools (`bauthorpalette`).

use cadaddy_engine::Input;
use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

/// The tab shown, kept between frames.
fn tab_id() -> egui::Id {
    egui::Id::new("authorpalette-tab")
}

/// Start `command` (BPARAMETER or BACTIONTOOL) for the palette tool `tool`.
pub fn start_tool(app: &mut CadApp, command: &str, tool: &str) {
    if app.session.running.is_some() {
        app.session.cancel();
    }
    app.start(command);
    if app.session.running.as_ref().is_none_or(|r| !r.id.eq_ignore_ascii_case(command)) {
        return;
    }
    if let Err(e) = app.session.input(Input::Keyword(tool.to_string())) {
        app.session.echo(e.to_string());
    }
}

/// Draw the palettes (or the test window's bar) for the Block Editor, when it is open.
pub fn show(app: &mut CadApp, ctx: &egui::Context) {
    let Ok(st) = app.session.state() else { return };
    if st.block_testing() {
        return test_bar(app, ctx);
    }
    let Some(be) = st.block_edit.as_ref().filter(|b| !b.current) else { return };
    if !be.palette {
        return;
    }
    let tabs = cadaddy_engine::blockedit::palette_tabs().as_array().cloned().unwrap_or_default();
    let mut tab = ctx.data(|d| d.get_temp::<usize>(tab_id())).unwrap_or(0).min(tabs.len().saturating_sub(1));
    let mut chosen: Option<(String, String)> = None;
    let mut test = false;
    let mut open = true;
    egui::Window::new(crate::tl!("Block Authoring Palettes"))
        .id(egui::Id::new("Block Authoring Palettes"))
        .open(&mut open)
        .default_pos(ctx.content_rect().left_top() + vec2(24.0, 120.0))
        .resizable(false)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (i, t) in tabs.iter().enumerate() {
                    let label = t.get("label").and_then(Value::as_str).unwrap_or("");
                    if ui.selectable_label(tab == i, crate::i18n::t(label)).clicked() {
                        tab = i;
                    }
                }
            });
            ui.separator();
            let tools = tabs.get(tab).and_then(|t| t.get("tools")).and_then(Value::as_array).cloned().unwrap_or_default();
            for t in tools {
                let (Some(id), Some(label), Some(command)) =
                    (t.get("id").and_then(Value::as_str), t.get("label").and_then(Value::as_str), t.get("command").and_then(Value::as_str))
                else {
                    continue;
                };
                // A lookup table is entered as parameters, not at the prompt.
                let typed = t.get("typed").and_then(Value::as_bool).unwrap_or(true);
                let b = ui.add_enabled(typed, egui::Button::new(crate::i18n::t(label)).min_size(vec2(150.0, 0.0)));
                if b.clicked() {
                    chosen = Some((command.to_string(), id.to_string()));
                }
            }
            ui.separator();
            if ui.button(crate::tl!("Test Block")).clicked() {
                test = true;
            }
        });
    ctx.data_mut(|d| d.insert_temp(tab_id(), tab));
    if let Some((command, tool)) = chosen {
        start_tool(app, &command, &tool);
    }
    if test && let Err(e) = app.run("btestblock", json!({})) {
        app.session.echo(e);
    }
    if !open {
        let _ = app.run("bauthorpalette", json!({ "action": "hide" }));
    }
}

/// The bar over the test window: what it is, and the button that throws it away.
fn test_bar(app: &mut CadApp, ctx: &egui::Context) {
    let t = Tokens::get();
    let mut close = false;
    egui::Area::new(egui::Id::new("Test Block bar")).anchor(egui::Align2::CENTER_TOP, vec2(0.0, 96.0)).show(ctx, |ui| {
        egui::Frame::NONE.fill(t.panel).stroke(egui::Stroke::new(1.0, t.border)).inner_margin(6.0).corner_radius(3.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(crate::tl!("Test Block")).strong());
                ui.label(crate::tl!("Changes made in the test window are thrown away when it closes."));
                if ui.button(crate::tl!("Close Test Block")).clicked() {
                    close = true;
                }
            });
        });
    });
    if close && let Err(e) = app.run("btestblock", json!({ "action": "close" })) {
        app.session.echo(e);
    }
}
