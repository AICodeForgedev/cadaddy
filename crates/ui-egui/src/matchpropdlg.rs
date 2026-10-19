//! MATCHPROP's Property Settings dialog: shown while MATCHPROP asks for its settings
//! ([`Picker::MatchProps`]), a checkbox for every property it can copy. OK keeps the choice for
//! the session ([`cadaddy_engine::LastUsed::matchprop`]); OK and Cancel both answer the prompt,
//! back to "Select destination object(s)". Typed keywords keep working meanwhile.

use cadaddy_engine::cmd::MatchProps;
use cadaddy_engine::{Input, Picker};

use crate::CadApp;

/// The dialog's rows: (label, field), basic properties then special ones, as AutoCAD groups them.
type Field = fn(&mut MatchProps) -> &mut bool;
pub const BASIC: [(&str, Field); 8] = [
    ("Color", |m| &mut m.color),
    ("Layer", |m| &mut m.layer),
    ("Linetype", |m| &mut m.linetype),
    ("Linetype Scale", |m| &mut m.ltscale),
    ("Lineweight", |m| &mut m.lineweight),
    ("Transparency", |m| &mut m.transparency),
    ("Thickness", |m| &mut m.thickness),
    ("Plot Style", |m| &mut m.plot_style),
];
pub const SPECIAL: [(&str, Field); 10] = [
    ("Dimension", |m| &mut m.dimension),
    ("Text", |m| &mut m.text),
    ("Hatch", |m| &mut m.hatch),
    ("Polyline", |m| &mut m.polyline),
    ("Viewport", |m| &mut m.viewport),
    ("Table", |m| &mut m.table),
    ("Material", |m| &mut m.material),
    ("Shadow display", |m| &mut m.shadow),
    ("Multileader", |m| &mut m.multileader),
    ("Center object", |m| &mut m.center),
];

/// The settings being edited, kept between frames while the dialog is open.
fn draft_id() -> egui::Id {
    egui::Id::new("matchprop_settings_draft")
}

/// What the dialog's buttons do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Ok,
    Cancel,
}

/// Apply a button: OK keeps `draft` for the session; either answers the settings prompt.
pub fn close(app: &mut CadApp, draft: MatchProps, choice: Choice) {
    if choice == Choice::Ok {
        app.session.last_used.matchprop = draft;
    }
    app.cmd.buffer.clear();
    if let Err(e) = app.session.input(Input::Enter) {
        app.session.echo(e.to_string());
    }
}

/// Every frame: the dialog while MATCHPROP's settings prompt is showing.
pub fn show(app: &mut CadApp, ctx: &egui::Context) {
    if !matches!(app.session.current_prompt().and_then(|p| p.picker), Some(Picker::MatchProps)) {
        ctx.data_mut(|d| {
            d.remove::<MatchProps>(draft_id());
        });
        return;
    }
    let mut draft: MatchProps = ctx.data_mut(|d| d.get_temp(draft_id())).unwrap_or(app.session.last_used.matchprop);
    let mut choice = None;
    egui::Window::new(crate::tl!("Property Settings"))
        .id(egui::Id::new("matchprop_settings"))
        .collapsible(false)
        .resizable(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.strong(crate::tl!("Basic Properties"));
                    for (label, f) in BASIC {
                        ui.checkbox(f(&mut draft), crate::i18n::t(label));
                    }
                });
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    ui.strong(crate::tl!("Special Properties"));
                    for (label, f) in SPECIAL {
                        ui.checkbox(f(&mut draft), crate::i18n::t(label));
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("OK")).clicked() {
                    choice = Some(Choice::Ok);
                }
                if ui.button(crate::tl!("Cancel")).clicked() {
                    choice = Some(Choice::Cancel);
                }
            });
        });
    if let Some(c) = choice {
        ctx.data_mut(|d| {
            d.remove::<MatchProps>(draft_id());
        });
        close(app, draft, c);
    } else {
        ctx.data_mut(|d| d.insert_temp(draft_id(), draft));
    }
}

#[cfg(test)]
mod tests {
    use cadaddy_engine::Session;
    use serde_json::json;

    use super::*;
    use crate::Services;

    fn at_settings() -> CadApp {
        let mut s = Session::new();
        s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap();
        let mut app = CadApp::new(s, Services::default());
        let h = app.session.doc().unwrap().model.iter().next().unwrap().handle;
        app.session.cmdline("matchprop").unwrap();
        app.session.input(Input::Pick(vec![h])).unwrap();
        app.session.input(Input::Keyword("Settings".into())).unwrap();
        assert!(matches!(app.session.current_prompt().and_then(|p| p.picker), Some(Picker::MatchProps)));
        app
    }

    #[test]
    fn ok_keeps_the_settings_and_returns_to_the_destination_prompt() {
        let mut app = at_settings();
        let mut m = MatchProps::all(true);
        m.color = false;
        m.center = false;
        close(&mut app, m, Choice::Ok);
        assert_eq!(app.session.last_used.matchprop, m);
        assert!(app.session.prompt_text().contains("Select destination object"));
        assert!(app.session.current_prompt().and_then(|p| p.picker).is_none());
    }

    #[test]
    fn cancel_keeps_the_old_settings() {
        let mut app = at_settings();
        let before = app.session.last_used.matchprop;
        close(&mut app, MatchProps::all(false), Choice::Cancel);
        assert_eq!(app.session.last_used.matchprop, before);
        assert!(app.session.prompt_text().contains("Select destination object"));
    }

    #[test]
    fn every_setting_has_a_row() {
        let mut m = MatchProps::all(false);
        for (_, f) in BASIC.iter().chain(SPECIAL.iter()) {
            *f(&mut m) = true;
        }
        assert_eq!(m, MatchProps::all(true));
    }

    #[test]
    fn the_dialog_draws_while_the_settings_prompt_shows() {
        let mut app = at_settings();
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui.ctx()));
            out.textures_delta.clear();
        }
        assert!(ctx.data_mut(|d| d.get_temp::<MatchProps>(draft_id())).is_some());
        // Gone with the prompt.
        app.session.input(Input::Enter).unwrap();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert!(ctx.data_mut(|d| d.get_temp::<MatchProps>(draft_id())).is_none());
    }
}
