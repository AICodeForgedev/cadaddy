//! The dialogs typed PURGE, FIND and COLOR open, as in AutoCAD (UI review 0.7.7b). -PURGE,
//! scripts and JSON calls keep the command line and the JSON form.

use cadaddy_color::{Color, Rgb};
use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;
use crate::theme::Tokens;

/// The Purge dialog's kinds: (key in `purge {list: true}`, `type` to purge, label).
const PURGE_KINDS: [(&str, &str, &str); 11] = [
    ("blocks", "blocks", "Blocks"),
    ("dimstyles", "dimstyles", "Dimension styles"),
    ("groups", "groups", "Groups"),
    ("layers", "layers", "Layers"),
    ("linetypes", "linetypes", "Linetypes"),
    ("mleaderstyles", "mleaderstyles", "Multileader styles"),
    ("mlinestyles", "mlinestyles", "Multiline styles"),
    ("tablestyles", "tablestyles", "Table styles"),
    ("textstyles", "textstyles", "Text styles"),
    ("zeroLength", "zerolength", "Zero-length geometry"),
    ("emptyText", "emptytext", "Empty text objects"),
];

/// How many items of a kind can go, from the `purge {list: true}` result.
fn purgeable(list: &Value, key: &str) -> usize {
    match &list[key] {
        Value::Array(a) => a.len(),
        v => v.as_u64().and_then(|n| usize::try_from(n).ok()).unwrap_or(0),
    }
}

/// PURGE: the unused items by kind; purge the checked kinds or all of them.
pub fn purge(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    // The list is worked out again only when the drawing changes (a purge, an undo).
    let key = app.session.doc().map(|d| std::ptr::from_ref(d) as usize).unwrap_or(0);
    let cache = egui::Id::new("purge_list");
    let list = match ctx.data_mut(|d| d.get_temp::<(usize, Value)>(cache)) {
        Some((k, v)) if k == key => v,
        _ => {
            let v = app.session.execute("purge", &json!({ "list": true })).unwrap_or(Value::Null);
            ctx.data_mut(|d| d.insert_temp(cache, (key, v.clone())));
            v
        }
    };
    let checked_id = egui::Id::new("purge_checked");
    let mut checked: Vec<bool> = ctx.data_mut(|d| d.get_temp(checked_id)).unwrap_or_else(|| vec![true; PURGE_KINDS.len()]);
    checked.resize(PURGE_KINDS.len(), true);
    let mut run: Option<Vec<&str>> = None;
    let mut close = false;
    egui::Window::new(crate::tl!("Purge"))
        .id(egui::Id::new("purge_dialog"))
        .open(open)
        .resizable(false)
        .collapsible(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.label(RichText::new(crate::tl!("Unused items in this drawing")).strong());
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for (i, (key, _, label)) in PURGE_KINDS.iter().enumerate() {
                    let n = purgeable(&list, key);
                    let Some(on) = checked.get_mut(i) else { continue };
                    ui.add_enabled_ui(n > 0, |ui| {
                        ui.checkbox(on, format!("{} ({n})", crate::i18n::t(label)));
                    });
                    if let Some(names) = list[*key].as_array().filter(|a| !a.is_empty()) {
                        ui.indent(*key, |ui| {
                            let shown: Vec<&str> = names.iter().filter_map(Value::as_str).take(12).collect();
                            let more = names.len().saturating_sub(shown.len());
                            let mut text = shown.join(", ");
                            if more > 0 {
                                text.push_str(&format!(" … (+{more})"));
                            }
                            ui.label(RichText::new(text).small().color(t.text_dim));
                        });
                    }
                }
            });
            let total: usize = PURGE_KINDS.iter().map(|(k, _, _)| purgeable(&list, k)).sum();
            ui.separator();
            ui.horizontal(|ui| {
                let picked: Vec<&str> =
                    PURGE_KINDS.iter().zip(&checked).filter(|((k, _, _), on)| **on && purgeable(&list, k) > 0).map(|((_, ty, _), _)| *ty).collect();
                if ui.add_enabled(!picked.is_empty(), egui::Button::new(crate::tl!("Purge Checked Items"))).clicked() {
                    run = Some(picked);
                }
                if ui.add_enabled(total > 0, egui::Button::new(crate::tl!("Purge All"))).clicked() {
                    run = Some(PURGE_KINDS.iter().map(|(_, ty, _)| *ty).collect());
                }
                if ui.button(crate::tl!("Close")).clicked() {
                    close = true;
                }
            });
        });
    ctx.data_mut(|d| d.insert_temp(checked_id, checked));
    if let Some(types) = run {
        // Named kinds first (blocks free layers), as PURGE All orders them; then the geometry.
        let mut purged = 0;
        for ty in types {
            if let Ok(r) = app.run("purge", json!({ "type": ty })) {
                purged += r["purged"].as_object().map_or(0, |m| m.values().map(|v| v.as_array().map_or(0, Vec::len)).sum::<usize>());
            }
        }
        app.session.echo(crate::tf!("Items purged: {count}", count = purged));
    }
    if close {
        *open = false;
    }
}

/// FIND: Find and Replace over the current space's text, dimensions, attributes and tables.
pub fn find(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let id = egui::Id::new("find_dialog_state");
    let (mut what, mut with, mut case, mut whole, mut status) =
        ctx.data_mut(|d| d.get_temp::<(String, String, bool, bool, String)>(id)).unwrap_or_default();
    let mut action: Option<Value> = None;
    let mut close = false;
    egui::Window::new(crate::tl!("Find and Replace"))
        .id(egui::Id::new("find_dialog"))
        .open(open)
        .resizable(false)
        .collapsible(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            egui::Grid::new("find_grid").num_columns(2).spacing(egui::vec2(10.0, 6.0)).show(ui, |ui| {
                ui.label(crate::tl!("Find what:"));
                ui.add(egui::TextEdit::singleline(&mut what).desired_width(240.0));
                ui.end_row();
                ui.label(crate::tl!("Replace with:"));
                ui.add(egui::TextEdit::singleline(&mut with).desired_width(240.0));
                ui.end_row();
            });
            ui.checkbox(&mut case, crate::tl!("Match case"));
            ui.checkbox(&mut whole, crate::tl!("Find whole words only"));
            if !status.is_empty() {
                ui.label(RichText::new(&status).small());
            }
            ui.separator();
            ui.horizontal(|ui| {
                let some = !what.is_empty();
                if ui.add_enabled(some, egui::Button::new(crate::tl!("Find"))).clicked() {
                    action = Some(json!({ "find": what, "matchCase": case, "wholeWord": whole }));
                }
                if ui.add_enabled(some, egui::Button::new(crate::tl!("Replace All"))).clicked() {
                    action = Some(json!({ "find": what, "replace": with, "matchCase": case, "wholeWord": whole }));
                }
                if ui.button(crate::tl!("Close")).clicked() {
                    close = true;
                }
            });
        });
    if let Some(p) = action {
        let replacing = p.get("replace").is_some();
        status = match app.run("find", p) {
            Ok(r) => {
                let n = r["total"].as_u64().unwrap_or(0);
                let objects = r["matches"].as_array().map_or(0, Vec::len);
                if replacing {
                    crate::tf!("Matches replaced: {count} (objects: {objects})", count = n, objects = objects)
                } else {
                    crate::tf!("Matches found and selected: {count} (objects: {objects})", count = n, objects = objects)
                }
            }
            Err(e) => e.to_string(),
        };
    }
    ctx.data_mut(|d| d.insert_temp(id, (what, with, case, whole, status)));
    if close {
        *open = false;
    }
}

/// COLOR: Select Color for new objects (CECOLOR).
pub fn color(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let current = app
        .session
        .doc()
        .map(|d| match d.header.get("CECOLOR_RGB").map(|_| d.header.i64("CECOLOR_RGB", 0)) {
            Some(v) => Color::True(Rgb::from_u32(u32::try_from(v).unwrap_or(0))),
            None => Color::from_aci(i16::try_from(d.header.i64("CECOLOR", 256)).unwrap_or(256)),
        })
        .unwrap_or(Color::ByLayer);
    let mut picked = None;
    egui::Window::new(crate::tl!("Select Color"))
        .id(egui::Id::new("color_dialog"))
        .open(open)
        .resizable(false)
        .collapsible(false)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            picked = crate::layers::color_palette(ui, egui::Id::new("color_dialog_palette"), current, true);
        });
    if let Some(c) = picked {
        let value = match c {
            Color::True(Rgb(r, g, b)) => json!(format!("{r},{g},{b}")),
            Color::ByLayer => json!("ByLayer"),
            Color::ByBlock => json!("ByBlock"),
            Color::Index(i) => json!(i),
        };
        let _ = app.run("color", json!({ "color": value }));
        *open = false;
    }
}
