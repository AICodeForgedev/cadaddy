//! The Undo History palette (Edit ▸ Undo History, UNDOHISTORY) and the Command History palette
//! (COMMANDHISTORY): every undo step by label, the steps that can be redone greyed, the UNDO
//! marks and an open BEgin group shown, a click going to that step; and the session as a script,
//! searchable, a line run again by a double-click, the lines shown copied as a script or the
//! session saved as a `.scr` file.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;

/// The Undo History window.
pub fn undo_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let list = app.session.execute("undohistory", &json!({})).unwrap_or(Value::Null);
    let mut go: Option<u64> = None;
    egui::Window::new(crate::tl!("Undo History")).id(egui::Id::new("Undo History")).open(open).resizable(true).default_size([280.0, 360.0]).show(
        ctx,
        |ui| {
            let marks: Vec<u64> = list["marks"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
            let group = list["group"].as_u64();
            let current = list["current"].as_u64().unwrap_or(0);
            egui::ScrollArea::vertical().id_salt("undo-steps").stick_to_bottom(true).max_height(300.0).show(ui, |ui| {
                if ui.selectable_label(current == 0, RichText::new(crate::tl!("(Start)")).color(t.text_dim)).clicked() {
                    go = Some(0);
                }
                for step in list["steps"].as_array().into_iter().flatten() {
                    let n = step["step"].as_u64().unwrap_or(0);
                    let done = step["done"] == true;
                    let mut label = crate::i18n::t(step["label"].as_str().unwrap_or_default()).to_string();
                    if marks.contains(&n) {
                        label.push_str("  ⚑");
                    }
                    if group.is_some_and(|g| n > g) {
                        label = format!("┃ {label}");
                    }
                    let text = if done { RichText::new(label) } else { RichText::new(label).color(t.text_faint).italics() };
                    let r = ui.selectable_label(n == current, text);
                    if r.clicked() {
                        go = Some(n);
                    }
                }
            });
            ui.label(RichText::new(crate::tl!("Click a step to undo or redo to it.")).small().color(t.text_dim));
        },
    );
    if let Some(step) = go
        && let Err(e) = app.run("undohistory", json!({ "action": "to", "step": step }))
    {
        app.session.echo(e);
    }
}

#[derive(Clone, Default)]
struct State {
    filter: String,
}

fn id() -> egui::Id {
    egui::Id::new("cadaddy-command-history")
}

/// Save the session as a script (asks where).
pub fn save_script(app: &mut CadApp) {
    match app.services.pick_save.as_ref().map(|p| p("session.scr")) {
        Some(Some(path)) => {
            if let Ok(r) = app.run("commandhistory", json!({ "action": "save", "path": path })) {
                app.session.echo(format!("{} lines saved to {}.", r["lines"], r["path"].as_str().unwrap_or_default()));
            }
        }
        Some(None) => {}
        None => app.session.echo("There is no file dialog here: COMMANDHISTORY {\"action\": \"save\", \"path\": \"session.scr\"} saves the script."),
    }
}

/// The Command History window.
pub fn commands_dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
    let lines: Vec<String> = app
        .session
        .execute("commandhistory", &json!({ "filter": st.filter }))
        .ok()
        .and_then(|v| serde_json::from_value(v["lines"].clone()).ok())
        .unwrap_or_default();
    let mut rerun: Option<String> = None;
    let (mut copy, mut save, mut clear) = (false, false, false);
    egui::Window::new(crate::tl!("Command History"))
        .id(egui::Id::new("Command History"))
        .open(open)
        .resizable(true)
        .default_size([360.0, 400.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Search"));
                let r = ui.text_edit_singleline(&mut st.filter);
                crate::icons::describe_area(&r, "Search");
            });
            egui::ScrollArea::vertical().id_salt("command-lines").stick_to_bottom(true).max_height(300.0).show(ui, |ui| {
                if lines.is_empty() {
                    ui.label(RichText::new(crate::tl!("Nothing yet.")).color(t.text_dim));
                }
                for l in &lines {
                    let shown = if l.is_empty() { "⏎".to_string() } else { l.clone() };
                    let r = ui.add(egui::Label::new(RichText::new(shown).monospace()).sense(egui::Sense::click()));
                    if r.double_clicked() {
                        rerun = Some(l.clone());
                    }
                    r.on_hover_text(crate::tl!("Double-click to run it again"));
                }
            });
            ui.horizontal(|ui| {
                copy = ui.button(crate::tl!("Copy as Script")).clicked();
                save = ui.button(crate::tl!("Save Session as Script...")).clicked();
                clear = ui.button(crate::tl!("Clear")).clicked();
            });
        });
    if let Some(l) = rerun {
        app.cmdline(&l);
    }
    if copy {
        let mut text = lines.join("\n");
        text.push('\n');
        ctx.copy_text(text);
    }
    if save {
        save_script(app);
    }
    if clear {
        let _ = app.run("commandhistory", json!({ "action": "clear" }));
    }
    if *open {
        ctx.data_mut(|d| d.insert_temp(id(), st));
    } else {
        ctx.data_mut(|d| d.remove::<State>(id()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_history_palettes_open_and_go_to_a_step() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.cmdline("line 0,0 10,0 ");
        app.cmdline("circle 0,0 5");
        app.start("undohistory");
        assert_eq!(app.ui.dialog.as_deref(), Some("undohistory"));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        app.run("undohistory", json!({ "action": "to", "step": 0 })).unwrap();
        assert_eq!(app.session.doc().unwrap().model.len(), 0);
        app.start("commandhistory");
        assert_eq!(app.ui.dialog.as_deref(), Some("commandhistory"));
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
    }
}
