//! The Action Macro Manager (ACTMANAGER): the action macros of this session and the actions
//! folder, each with its steps; Play (asks for the inputs marked "ask"), Delete, Rename, and a
//! box per input step for asking for it when the macro plays. Record and Stop are here too.

use egui::{RichText, vec2};
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

pub const DIALOG: &str = "actmanager";

/// The window's choices between frames.
#[derive(Clone, Default)]
struct State {
    open: Option<String>,
    rename: String,
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    let id = egui::Id::new("actmanager state");
    let mut st: State = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    let list = app.run("actions", json!({})).unwrap_or(Value::Null);
    let macros = list["macros"].as_array().cloned().unwrap_or_default();
    let recording = list["recording"] == true;
    let mut act: Option<(&'static str, Value)> = None;
    let mut play: Option<String> = None;
    egui::Window::new(crate::tl!("Action Macro Manager"))
        .id(egui::Id::new("actmanager window"))
        .open(open)
        .collapsible(false)
        .default_size(vec2(520.0, 420.0))
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                if recording {
                    ui.label(RichText::new(crate::tl!("Recording…")).color(egui::Color32::from_rgb(230, 90, 80)));
                    if ui.button(crate::tl!("Stop")).clicked() {
                        act = Some(("actstop", json!({})));
                    }
                } else if ui.button(crate::tl!("Record")).clicked() {
                    act = Some(("actrecord", json!({})));
                }
            });
            ui.separator();
            if macros.is_empty() {
                ui.label(RichText::new(crate::tl!("No action macros yet: Record, do the work, then Stop.")).color(t.text_dim));
            }
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for m in &macros {
                    let name = m["name"].as_str().unwrap_or_default().to_string();
                    let steps = m["steps"].as_array().cloned().unwrap_or_default();
                    let expanded = st.open.as_deref() == Some(name.as_str());
                    ui.horizontal(|ui| {
                        if ui.selectable_label(expanded, RichText::new(&name).strong()).clicked() {
                            st.open = if expanded { None } else { Some(name.clone()) };
                            st.rename = name.clone();
                        }
                        ui.label(RichText::new(crate::tf!("{n} steps", n = steps.len())).color(t.text_dim));
                        if ui.button(crate::tl!("Play")).clicked() {
                            play = Some(name.clone());
                        }
                        if ui.button(crate::tl!("Delete")).clicked() {
                            act = Some(("actions", json!({ "action": "delete", "name": name })));
                        }
                    });
                    if let Some(d) = m["description"].as_str().filter(|d| !d.is_empty()) {
                        ui.label(RichText::new(d).color(t.text_dim));
                    }
                    if !expanded {
                        continue;
                    }
                    ui.indent(("steps", &name), |ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut st.rename).desired_width(160.0));
                            if ui.button(crate::tl!("Rename")).clicked() && st.rename.trim() != name {
                                act = Some(("actions", json!({ "action": "rename", "name": name, "to": st.rename.trim() })));
                            }
                        });
                        for (i, s) in steps.iter().enumerate() {
                            if let Some(msg) = s["message"].as_str() {
                                ui.label(RichText::new(crate::tf!("Message: {text}", text = msg)).italics());
                            } else if s.get("basepoint").is_some_and(|b| !b.is_null()) {
                                ui.label(crate::tl!("Base point (asked for)"));
                            } else if let Some(input) = s["input"].as_str() {
                                let mut ask = s["ask"] == true;
                                let shown = if input.is_empty() { crate::i18n::t("(Enter)").to_string() } else { input.to_string() };
                                if ui.checkbox(&mut ask, shown).on_hover_text(crate::tl!("Ask for this input when the macro plays")).changed() {
                                    act = Some(("actions", json!({ "action": "ask", "name": name, "step": i, "ask": ask })));
                                }
                            }
                        }
                    });
                }
            });
        });
    if let Some((cmd, p)) = act
        && let Err(e) = app.run(cmd, p)
    {
        app.session.echo(e);
    }
    if let Some(name) = play {
        // As typed: the macro asks for its inputs at the command line.
        *open = false;
        app.ui.dialog = None;
        app.cmdline(&name);
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_engine::Session;

    #[test]
    fn the_manager_opens_from_the_menu_lists_and_plays() {
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        app.run("actrecord", json!({})).unwrap();
        for l in ["circle", "*0,0", "3"] {
            app.cmdline(l);
        }
        app.run("actstop", json!({ "name": "Dot" })).unwrap();
        app.start("actmanager");
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        let ctx = egui::Context::default();
        let mut open = true;
        ctx.run_ui(egui::RawInput::default(), |ui| dialog(&mut app, ui.ctx(), &mut open)).textures_delta.clear();
        assert!(open);
        let list = app.run("actions", json!({})).unwrap();
        assert_eq!(list["macros"][0]["name"], "Dot");
    }
}
