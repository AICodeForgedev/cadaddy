//! The exchange notice: after the first DWG or DXF save of a drawing that loses something, a
//! dialog lists what that file didn't keep — feature, count, dropped or changed — with Select for
//! the objects concerned. Shown once per drawing, for saves the user started (scripts, the control
//! channel and MCP get the same list in the save reply's `notKeptDetail` instead).

use egui::{RichText, vec2};
use serde_json::Value;

use crate::CadApp;
use crate::theme::Tokens;

pub const DIALOG: &str = "exchangeloss";

/// Take the notice the last save left; with `show`, open the dialog on it.
pub fn take(app: &mut CadApp, show: bool) {
    let Some(notice) = app.session.state_mut().ok().and_then(|st| st.exchange_notice.take()) else { return };
    if show {
        app.exchange_notice = Some(notice);
        app.ui.dialog = Some(DIALOG.into());
    }
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let Some(notice) = app.exchange_notice.clone() else {
        *open = false;
        return;
    };
    let t = Tokens::get();
    let format = notice["format"].as_str().unwrap_or("DWG").to_string();
    let losses = notice["losses"].as_array().cloned().unwrap_or_default();
    let mut select: Option<Vec<String>> = None;
    let mut close = false;
    egui::Window::new(crate::tf!("Not kept in the {format} file", format = format))
        .id(egui::Id::new("exchange notice"))
        .open(open)
        .collapsible(false)
        .default_size(vec2(520.0, 320.0))
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.label(crate::tl!("The drawing was saved. This file format can't keep everything in it:"));
            ui.add_space(4.0);
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                egui::Grid::new("exchange_losses").striped(true).num_columns(4).show(ui, |ui| {
                    for h in ["What", "Count", "", ""] {
                        ui.label(RichText::new(crate::i18n::t(h)).strong());
                    }
                    ui.end_row();
                    for l in &losses {
                        ui.label(l["feature"].as_str().unwrap_or_default());
                        ui.label(l["count"].to_string());
                        ui.label(if l["what"] == "dropped" { crate::tl!("left out") } else { crate::tl!("changed") });
                        let handles: Vec<String> =
                            l["handles"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
                        if !handles.is_empty() && l["what"] != "dropped" {
                            if ui.small_button(crate::tl!("Select")).clicked() {
                                select = Some(handles);
                            }
                        } else if let Some(names) = l["names"].as_array().filter(|n| !n.is_empty()) {
                            let list: Vec<&str> = names.iter().filter_map(Value::as_str).collect();
                            ui.label(RichText::new(list.join(", ")).color(t.text_dim));
                        } else {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                });
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new(crate::tl!("A CADaddy drawing (.cdy) keeps everything. Check Exchange lists this again without saving."))
                    .color(t.text_dim),
            );
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("OK")).clicked() {
                    close = true;
                }
            });
        });
    if let Some(hs) = select
        && let Ok(st) = app.session.state_mut()
    {
        st.selection = hs.iter().filter_map(|h| cadaddy_doc::Handle::parse_hex(h)).filter(|h| st.doc.model.get(*h).is_some()).collect();
    }
    if close {
        *open = false;
    }
    if !*open {
        app.exchange_notice = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_engine::Session;
    use serde_json::json;

    fn notice(h: &str) -> Value {
        json!({"path": "a.dwg", "format": "DWG", "losses": [
            {"feature": "object transparency", "what": "changed", "count": 1, "handles": [h]},
            {"feature": "layouts", "what": "dropped", "count": 1, "names": ["Layout2"]}
        ]})
    }

    #[test]
    fn a_save_the_user_started_shows_the_notice_once_programmatic_ones_never_do() {
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        let h = app.run("line", json!({"points": [[0, 0], [1, 0]]})).unwrap()["handles"][0].as_str().unwrap().to_string();
        // Programmatic: the notice is dropped (the reply carried it), no dialog.
        app.session.state_mut().unwrap().exchange_notice = Some(notice(&h));
        let _ = app.run("regen", json!({}));
        assert!(app.ui.dialog.is_none() && app.exchange_notice.is_none());
        assert!(app.session.state().unwrap().exchange_notice.is_none());
        // Started from the menu or typed: the dialog opens on it.
        app.session.state_mut().unwrap().exchange_notice = Some(notice(&h));
        app.start("regen");
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        assert_eq!(app.exchange_notice, Some(notice(&h)));
        assert!(app.session.state().unwrap().exchange_notice.is_none(), "taken: shown once");
        // Drawn without a crash; closing forgets it.
        let ctx = egui::Context::default();
        let mut open = true;
        ctx.run_ui(egui::RawInput::default(), |ui| dialog(&mut app, ui.ctx(), &mut open)).textures_delta.clear();
        assert!(open);
        let mut shut = false;
        ctx.run_ui(egui::RawInput::default(), |ui| dialog(&mut app, ui.ctx(), &mut shut)).textures_delta.clear();
        assert!(app.exchange_notice.is_none());
        app.session.state_mut().unwrap().exchange_notice = Some(notice(&h));
        app.cmdline("regen");
        assert_eq!(app.exchange_notice, Some(notice(&h)), "typed commands show it too");
    }
}
