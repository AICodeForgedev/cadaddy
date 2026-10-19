//! The Activity palette (ACTIVITY typed alone, Tools ▸ Activity History): the drawing's steps,
//! newest first, each with its command, when, and how many objects it added, changed and erased;
//! filtered by command; a click selects the objects a step added or changed (those that still
//! exist) and zooms to them. Everything runs the ACTIVITY command.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "activity";

fn id() -> egui::Id {
    egui::Id::new("activity_palette_filter")
}

/// "12 s ago", "5 min ago", "3 h ago", "2 d ago" from seconds since 1970.
pub fn ago(time: u64, now: u64) -> String {
    if time == 0 {
        return String::new();
    }
    let d = now.saturating_sub(time);
    match d {
        0..60 => crate::tf!("{n} s ago", n = d),
        60..3600 => crate::tf!("{n} min ago", n = d / 60),
        3600..86_400 => crate::tf!("{n} h ago", n = d / 3600),
        _ => crate::tf!("{n} d ago", n = d / 86_400),
    }
}

fn now() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// Select step `seq`'s objects and zoom to them; how many there were.
fn show(app: &mut CadApp, seq: u64) -> Result<(usize, usize), String> {
    let r = app.run("activity", json!({ "action": "show", "seq": seq }))?;
    let hs: Vec<Value> = r["handles"].as_array().cloned().unwrap_or_default();
    if !hs.is_empty() {
        let _ = app.run("zoom.object", json!({ "handles": hs }));
    }
    Ok((hs.len(), r["missing"].as_u64().unwrap_or(0) as usize))
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut filter: String = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
    let status_id = egui::Id::new("activity_palette_status");
    let mut status: String = ctx.data(|d| d.get_temp(status_id)).unwrap_or_default();
    let params = if filter.trim().is_empty() { json!({}) } else { json!({ "command": filter.trim() }) };
    let list = app.session.execute("activity", &params).unwrap_or(Value::Null);
    let mut pick: Option<u64> = None;
    let mut clear = false;
    let now = now();
    egui::Window::new(crate::tl!("Activity History"))
        .id(egui::Id::new("activity_palette"))
        .open(open)
        .resizable(true)
        .default_size([360.0, 420.0])
        .show(ctx, |ui| {
            if list["enabled"] == false {
                ui.label(RichText::new(crate::tl!("Activity history is off (Options ▸ Open and Save).")).color(t.warn));
            }
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Filter:"));
                ui.add(egui::TextEdit::singleline(&mut filter).desired_width(160.0).hint_text(crate::tl!("command")));
            });
            ui.label(RichText::new(crate::tf!("{n} steps", n = list["count"].as_u64().unwrap_or(0))).small().color(t.text_dim));
            egui::ScrollArea::vertical().id_salt("activity_list").max_height(300.0).show(ui, |ui| {
                for e in list["entries"].as_array().into_iter().flatten() {
                    let seq = e["seq"].as_u64().unwrap_or(0);
                    let counts = crate::tf!(
                        "+{a} ~{m} −{e}",
                        a = e["added"].as_u64().unwrap_or(0),
                        m = e["modified"].as_u64().unwrap_or(0),
                        e = e["erased"].as_u64().unwrap_or(0)
                    );
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(ago(e["time"].as_u64().unwrap_or(0), now)).small().color(t.text_dim));
                        let label = format!("{}  {counts}", crate::i18n::t(e["command"].as_str().unwrap_or_default()));
                        if ui.selectable_label(false, label).on_hover_text(crate::tl!("Select the objects this step added or changed")).clicked() {
                            pick = Some(seq);
                        }
                    });
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button(crate::tl!("Clear")).clicked() {
                    clear = true;
                }
                if !status.is_empty() {
                    ui.label(RichText::new(&status).small());
                }
            });
        });
    if let Some(seq) = pick {
        status = match show(app, seq) {
            Ok((0, _)) => crate::tl!("The objects of this step are gone.").to_string(),
            Ok((n, 0)) => crate::tf!("{n} object(s) selected.", n = n),
            Ok((n, gone)) => crate::tf!("{n} object(s) selected ({gone} since erased).", n = n, gone = gone),
            Err(e) => e,
        };
    }
    if clear {
        let _ = app.run("activity", json!({ "action": "clear" }));
        status.clear();
    }
    ctx.data_mut(|d| {
        d.insert_temp(id(), filter);
        d.insert_temp(status_id, status);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_palette_lists_steps_and_shows_their_objects() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        app.run("line", json!({ "points": [[0, 0], [10, 0]] })).unwrap();
        app.run("circle", json!({ "center": [5, 5], "radius": 2 })).unwrap();
        let r = app.run("activity", json!({})).unwrap();
        let seq = r["entries"][0]["seq"].as_u64().unwrap();
        assert_eq!(show(&mut app, seq).unwrap(), (1, 0));
        assert_eq!(app.session.selection().len(), 1);
        assert_eq!(ago(100, 160), "1 min ago");
        crate::menus::run_ui_command(&mut app, "activity", &Value::Null);
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
    }
}
