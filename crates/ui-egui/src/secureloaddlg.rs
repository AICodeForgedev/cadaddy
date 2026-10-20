//! "Load this file?": a LISP file or script outside the trusted folders that the user tried to
//! load (SECURELOAD 1, [`cadaddy_engine::secureload`]). Load Once allows it this session, Always
//! Load adds its folder to TRUSTEDPATHS, Don't Load (the default) leaves it; an allowed APPLOAD or
//! SCRIPT runs again. Programmatic calls get the refusal in their reply instead.

use egui::{RichText, vec2};

use crate::CadApp;
use crate::theme::Tokens;

pub const DIALOG: &str = "secureload";

/// Take the question the last command left; with `show`, open the dialog on it.
pub fn take(app: &mut CadApp, show: bool) {
    let Some(ask) = app.session.secureload_ask.take() else { return };
    if show {
        app.secureload_ask = Some(ask);
        app.ui.dialog = Some(DIALOG.into());
    }
}

/// Allow the file (once, or its folder always) and run the command that wanted it again.
fn allow(app: &mut CadApp, always: bool) {
    let Some(ask) = app.secureload_ask.take() else { return };
    let path = ask.path.to_string_lossy().into_owned();
    if let Err(e) = app.run("secureload.allow", serde_json::json!({ "path": path, "always": always })) {
        app.session.echo(e);
        return;
    }
    match ask.retry {
        Some((cmd, params)) if cmd == "appload" => {
            // As APPLOAD from the menu: the routine may wait for the user.
            match cadaddy_engine::lisp::builtins::read_text_file(&path) {
                Some(text) => {
                    if let Err(e) = app.session.lisp_load(&path, &text, false) {
                        app.session.echo(e.to_string());
                    }
                }
                None => app.session.echo(format!("APPLOAD: can't read {path}")),
            }
            let _ = params;
        }
        Some((cmd, params)) => {
            let _ = app.run(&cmd, params);
        }
        None => app.session.echo(format!("{path} may now be loaded; run the routine again.")),
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(ask) = app.secureload_ask.clone() else {
        *open = false;
        return;
    };
    let t = Tokens::get();
    let (mut once, mut always, mut no) = (false, false, false);
    egui::Window::new(crate::tl!("Load This File?"))
        .id(egui::Id::new("secureload window"))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .default_size(vec2(480.0, 200.0))
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.label(crate::tl!("This file isn't in a trusted folder:"));
            ui.add(egui::Label::new(RichText::new(ask.path.display().to_string()).monospace()).wrap());
            ui.add_space(4.0);
            ui.label(
                RichText::new(crate::tl!("LISP routines and scripts can change drawings and files. Load it only if you trust where it came from."))
                    .color(t.text_dim),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let dont = ui.button(crate::tl!("Don't Load"));
                if ui.ctx().memory(|m| m.focused().is_none()) {
                    dont.request_focus();
                }
                no = dont.clicked();
                once = ui.button(crate::tl!("Load Once")).clicked();
                always = ui
                    .button(crate::tl!("Always Load from This Folder"))
                    .on_hover_text(crate::tf!("Adds {folder} to TRUSTEDPATHS", folder = ask.folder.display()))
                    .clicked();
            });
        });
    if once || always {
        *open = false;
        // Close first: the routine may open its own prompts.
        app.ui.dialog = None;
        app.secureload_ask = Some(ask);
        allow(app, always);
        return;
    }
    if no {
        *open = false;
    }
    if !*open {
        app.secureload_ask = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_engine::Session;
    use serde_json::json;

    fn script_in_untrusted_folder() -> String {
        let dir = std::env::temp_dir().join(format!("cadaddy-secureload-ui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("draw.scr");
        std::fs::write(&p, "circle 0,0 3\n").unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn typing_an_untrusted_script_asks_and_load_once_runs_it() {
        let path = script_in_untrusted_folder();
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        // Programmatic: refused, no dialog.
        assert!(app.run("script", json!({ "path": path })).is_err());
        assert!(app.ui.dialog.is_none() && app.secureload_ask.is_none());
        // Typed: refused, then asked.
        app.cmdline("script");
        app.cmdline(&path);
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        assert_eq!(app.session.doc().unwrap().model.len(), 0, "nothing ran yet");
        let ctx = egui::Context::default();
        let mut open = true;
        ctx.run_ui(egui::RawInput::default(), |ui| dialog(&mut app, ui.ctx(), &mut open)).textures_delta.clear();
        assert!(open, "waits for the user");
        // Load Once: allowed for the session and run again.
        allow(&mut app, false);
        assert_eq!(app.session.doc().unwrap().model.len(), 1, "the script ran");
        assert!(app.session.settings.trustedpaths.is_empty(), "once, not always");
        assert!(app.run("script", json!({ "path": path })).is_ok(), "allowed for the rest of the session");
    }
}
