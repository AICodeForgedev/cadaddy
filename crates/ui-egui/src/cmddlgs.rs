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

/// COLOR: Select Color for new objects (CECOLOR).
/// Where the Select Color dialog's choice goes: the objects given (the Properties palette's
/// "More Colors…" for a selection), else the colour of new objects.
fn color_target_id() -> egui::Id {
    egui::Id::new("color_dialog_target")
}

/// Open the Select Color dialog for `handles` (`None`: the current colour, as COLOR does).
pub fn open_color_dialog(ctx: &egui::Context, handles: Option<Vec<String>>) {
    ctx.data_mut(|d| match handles {
        Some(h) => {
            d.insert_temp(color_target_id(), h);
        }
        None => d.remove::<Vec<String>>(color_target_id()),
    });
    // Opened on the next frame ([`crate::dialogs::show`]).
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("color_dialog_open"), true));
}

/// Give the picked colour to `target`'s objects, else make it the colour of new objects.
pub fn apply_color(app: &mut CadApp, target: Option<&[String]>, c: Color) {
    let value = match c {
        Color::True(Rgb(r, g, b)) => json!(format!("{r},{g},{b}")),
        Color::ByLayer => json!("ByLayer"),
        Color::ByBlock => json!("ByBlock"),
        Color::Index(i) => json!(i),
    };
    let _ = match target {
        Some(hs) => app.run("properties.set", json!({ "handles": hs, "color": value })),
        None => app.run("color", json!({ "color": value })),
    };
}

pub fn color(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let target: Option<Vec<String>> = ctx.data(|d| d.get_temp(color_target_id()));
    // For objects: the colour they share (else ByLayer to start from).
    let shared = target.as_ref().and_then(|hs| {
        let d = app.session.doc().ok()?;
        let cs: Vec<Color> =
            hs.iter().filter_map(|h| cadaddy_doc::Handle::parse_hex(h)).filter_map(|h| d.entity(h)).map(|e| e.common.color).collect();
        cs.first().copied().filter(|c| cs.iter().all(|x| x == c))
    });
    let current = match (&target, shared) {
        (Some(_), Some(c)) => c,
        (Some(_), None) => Color::ByLayer,
        (None, _) => app
            .session
            .doc()
            .map(|d| match d.header.get("CECOLOR_RGB").map(|_| d.header.i64("CECOLOR_RGB", 0)) {
                Some(v) => Color::True(Rgb::from_u32(u32::try_from(v).unwrap_or(0))),
                None => Color::from_aci(i16::try_from(d.header.i64("CECOLOR", 256)).unwrap_or(256)),
            })
            .unwrap_or(Color::ByLayer),
    };
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
        apply_color(app, target.as_deref(), c);
        *open = false;
    }
    if !*open {
        ctx.data_mut(|d| d.remove::<Vec<String>>(color_target_id()));
    }
}

#[cfg(test)]
mod color_tests {
    use super::*;
    use cadaddy_engine::Session;

    #[test]
    fn select_color_goes_to_the_objects_it_was_opened_for_and_every_tab_draws() {
        let mut app = CadApp::new(Session::new(), crate::Services::default());
        let a = app.run("circle", json!({"center": [0, 0], "radius": 2})).unwrap()["handle"].as_str().unwrap().to_string();
        let b = app.run("circle", json!({"center": [9, 0], "radius": 2})).unwrap()["handle"].as_str().unwrap().to_string();
        // For a selection: only those objects; new objects keep their colour.
        apply_color(&mut app, Some(std::slice::from_ref(&a)), Color::True(Rgb(10, 200, 30)));
        let colour = |app: &CadApp, h: &str| app.session.doc().unwrap().entity(cadaddy_doc::Handle::parse_hex(h).unwrap()).unwrap().common.color;
        assert_eq!(colour(&app, &a), Color::True(Rgb(10, 200, 30)));
        assert_eq!(colour(&app, &b), Color::ByLayer);
        assert_eq!(app.session.doc().unwrap().header.i64("CECOLOR", 256), 256);
        // Without: the colour of new objects (COLOR).
        apply_color(&mut app, None, Color::Index(3));
        assert_eq!(app.session.doc().unwrap().header.i64("CECOLOR", 256), 3);
        // A colour from a book is its true colour.
        let brick = cadaddy_engine::cmd::colorbook::lookup("CADaddy Basics", "brick").unwrap();
        apply_color(&mut app, Some(std::slice::from_ref(&b)), Color::True(brick));
        assert_eq!(colour(&app, &b), Color::True(brick));
        // The dialog with each of its tabs, for objects and for new objects, draws without a crash.
        let ctx = egui::Context::default();
        let tab_id = egui::Id::new("color_dialog_palette").with("tab");
        for target in [Some(vec![a.clone(), b.clone()]), None] {
            open_color_dialog(&ctx, target);
            for tab in 0u8..3 {
                ctx.data_mut(|d| d.insert_temp(tab_id, tab));
                let mut open = true;
                ctx.run_ui(egui::RawInput::default(), |ui| color(&mut app, ui.ctx(), &mut open)).textures_delta.clear();
                assert!(open);
            }
        }
    }
}
