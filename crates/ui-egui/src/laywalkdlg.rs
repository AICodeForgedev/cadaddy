//! The LayerWalk dialog (LAYWALK, Format ▸ Layer Tools ▸ Layer Walk...): the drawing's layers in
//! a list; the ones picked (click, Ctrl/Cmd+click to add, Shift+click for a range) are the only
//! ones shown while the dialog is open, and a wildcard filter picks them by name. Closing puts
//! the layers back unless "Restore on exit" is cleared. Everything runs the LAYWALK command.

use egui::RichText;
use serde_json::json;

use crate::CadApp;

pub const DIALOG: &str = "laywalk";

#[derive(Clone)]
struct State {
    picked: Vec<String>,
    anchor: Option<usize>,
    filter: String,
    restore: bool,
    status: String,
}

impl Default for State {
    fn default() -> Self {
        State { picked: Vec::new(), anchor: None, filter: String::new(), restore: true, status: String::new() }
    }
}

fn id() -> egui::Id {
    egui::Id::new("laywalk_dialog_state")
}

/// Show only `picked` (none picked: every layer as it was).
fn apply(app: &mut CadApp, st: &mut State) {
    let r = if st.picked.is_empty() {
        app.run("laywalk", json!({ "end": true, "restore": true }))
    } else {
        app.run("laywalk", json!({ "show": st.picked }))
    };
    st.status = match r {
        Ok(v) => v["message"].as_str().unwrap_or_default().to_string(),
        Err(e) => e,
    };
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
    let names: Vec<String> = app.session.doc().map(|d| d.layers.iter().map(|l| l.name.clone()).collect()).unwrap_or_default();
    let mut changed = false;
    let mut close = false;
    egui::Window::new(crate::tl!("LayerWalk"))
        .id(egui::Id::new("laywalk_dialog"))
        .open(open)
        .resizable(true)
        .collapsible(false)
        .default_width(300.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.label(RichText::new(crate::tf!("Layers: {n}", n = names.len())).small().color(t.text_dim));
            egui::ScrollArea::vertical().id_salt("laywalk_list").max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    let on = st.picked.iter().any(|p| p == n);
                    let r = ui.selectable_label(on, n);
                    if r.clicked() {
                        let m = ui.input(|i| i.modifiers);
                        if m.shift
                            && let Some(a) = st.anchor
                        {
                            let (lo, hi) = (a.min(i), a.max(i));
                            st.picked = names.get(lo..=hi).map(<[String]>::to_vec).unwrap_or_default();
                        } else if m.command {
                            if on {
                                st.picked.retain(|p| p != n);
                            } else {
                                st.picked.push(n.clone());
                            }
                            st.anchor = Some(i);
                        } else {
                            st.picked = vec![n.clone()];
                            st.anchor = Some(i);
                        }
                        changed = true;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Filter:"));
                let r = ui.add(egui::TextEdit::singleline(&mut st.filter).desired_width(150.0).hint_text("A*"));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !st.filter.trim().is_empty() {
                    let pats: Vec<String> = st.filter.split(',').map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()).collect();
                    st.picked = names.iter().filter(|n| pats.iter().any(|p| cadaddy_engine::cmd::wildcard(p, &n.to_lowercase()))).cloned().collect();
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Select All")).clicked() {
                    st.picked = names.clone();
                    changed = true;
                }
                if ui.button(crate::tl!("Clear")).clicked() {
                    st.picked.clear();
                    changed = true;
                }
            });
            ui.checkbox(&mut st.restore, crate::tl!("Restore on exit"));
            if !st.status.is_empty() {
                ui.label(RichText::new(&st.status).small());
            }
            ui.separator();
            if ui.button(crate::tl!("Close")).clicked() {
                close = true;
            }
        });
    if changed {
        apply(app, &mut st);
    }
    if close {
        *open = false;
    }
    if !*open {
        let _ = app.run("laywalk", json!({ "end": true, "restore": st.restore }));
        ctx.data_mut(|d| d.remove::<State>(id()));
    } else {
        ctx.data_mut(|d| d.insert_temp(id(), st));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walking_shows_the_picked_layers_and_closing_restores() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.run("layer.new", json!({ "name": "Walls" })).unwrap();
        app.run("layer.new", json!({ "name": "Doors" })).unwrap();
        let mut st = State { picked: vec!["Walls".into()], ..State::default() };
        apply(&mut app, &mut st);
        let d = app.session.doc().unwrap();
        assert!(d.layer("Walls").unwrap().on && !d.layer("Doors").unwrap().on && !d.layer("0").unwrap().on);
        // The dialog renders, then closes: the layers come back.
        app.ui.dialog = Some(DIALOG.into());
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        let mut open = false;
        dialog(&mut app, &ctx, &mut open);
        let d = app.session.doc().unwrap();
        assert!(d.layers.iter().all(|l| l.on), "Restore on exit");
    }
}
