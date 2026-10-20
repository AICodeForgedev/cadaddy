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

/// The Purge dialog's state between frames.
#[derive(Clone, Default)]
struct PurgeState {
    /// Find Non-Purgeable Items instead of the purgeable ones.
    held_view: bool,
    /// Items left out: (kind key, name); a kind without names (zero-length…) as (key, "").
    off: std::collections::HashSet<(String, String)>,
    nested: bool,
    confirm: bool,
    /// Confirm each item: what is still to be asked about, as (type, kind key, name).
    queue: Vec<(String, String, String)>,
    /// Items purged so far in this run.
    purged: usize,
    /// The non-purgeable item shown: (kind key, name).
    shown: Option<(String, String)>,
}

/// The names of a kind in the list result.
fn names_of<'a>(list: &'a Value, key: &str) -> Vec<&'a str> {
    list[key].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
}

/// Purge one kind's `names` (empty: the whole kind, for zero-length geometry and empty text).
fn purge_names(app: &mut CadApp, ty: &str, names: &[String], nested: bool) -> usize {
    let mut p = json!({ "type": ty, "nested": nested });
    if !names.is_empty() {
        p["exact"] = json!(names);
    }
    app.run("purge", p).map_or(0, |r| r["purged"].as_object().map_or(0, |m| m.values().map(|v| v.as_array().map_or(0, Vec::len)).sum::<usize>()))
}

/// Select the objects behind a reason, and zoom to them (in their layout).
fn show_holders(app: &mut CadApp, handles: &[String]) {
    let hs: Vec<cadaddy_doc::Handle> = handles.iter().filter_map(|h| cadaddy_doc::Handle::parse_hex(h)).collect();
    let Some(first) = hs.first().copied() else { return };
    let space = app.session.doc().ok().and_then(|d| {
        if d.model.get(first).is_some() {
            Some("Model".to_string())
        } else {
            d.layouts.iter().find(|l| l.entities.get(first).is_some()).map(|l| l.name.clone())
        }
    });
    if let Some(sp) = space {
        crate::finddlg::zoom_to(app, &json!({ "handle": first.hex(), "space": sp }));
    }
    let _ = app.run("zoom.object", json!({ "handles": handles }));
    app.session.set_selection(hs);
}

/// PURGE: the unused items in a tree by kind, each with its own check box, Purge nested items and
/// Confirm each item; or the items that can't be purged, each with why, the objects that hold
/// it selected and zoomed to.
pub fn purge(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = Tokens::get();
    // The lists are worked out again only when the drawing changes (a purge, an undo).
    let key = app.session.doc().map(|d| std::ptr::from_ref(d) as usize).unwrap_or(0);
    let state_id = egui::Id::new("purge_state");
    let mut st: PurgeState = ctx.data_mut(|d| d.get_temp(state_id)).unwrap_or_default();
    let cache = egui::Id::new(("purge_list", st.held_view));
    let list = match ctx.data_mut(|d| d.get_temp::<(usize, Value)>(cache)) {
        Some((k, v)) if k == key => v,
        _ => {
            let v = app.session.execute("purge", &json!({ "list": true, "reasons": st.held_view })).unwrap_or(Value::Null);
            ctx.data_mut(|d| d.insert_temp(cache, (key, v.clone())));
            v
        }
    };
    let mut run: Option<Vec<(String, String, String)>> = None;
    let mut answer: Option<&str> = None;
    let mut holders: Option<Vec<String>> = None;
    let mut close = false;
    egui::Window::new(crate::tl!("Purge"))
        .id(egui::Id::new("purge_dialog"))
        .open(open)
        .resizable(true)
        .collapsible(false)
        .default_width(380.0)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            // Right to left in Arabic (rtl.rs).
            let mut body = crate::rtl::Body::new(ui);
            let ui = &mut *body;
            ui.horizontal(|ui| {
                ui.selectable_value(&mut st.held_view, false, crate::tl!("Purgeable Items"));
                ui.selectable_value(&mut st.held_view, true, crate::tl!("Find Non-Purgeable Items"));
            });
            ui.separator();
            if let Some((_, kkey, name)) = st.queue.first().cloned() {
                // Confirm each item.
                let label = PURGE_KINDS.iter().find(|(k, _, _)| *k == kkey).map_or("", |(_, _, l)| *l);
                ui.label(crate::tf!("Purge {kind} \"{name}\"?", kind = crate::i18n::t(label), name = name));
                ui.horizontal(|ui| {
                    for (k, l) in [("yes", "Yes"), ("all", "Yes to All"), ("no", "No"), ("cancel", "Cancel")] {
                        if ui.button(crate::i18n::t(l)).clicked() {
                            answer = Some(k);
                        }
                    }
                });
                return;
            }
            if st.held_view {
                ui.label(RichText::new(crate::tl!("Items that can't be purged, and why")).strong());
                egui::ScrollArea::vertical().id_salt("purge_held").max_height(260.0).show(ui, |ui| {
                    for (key, _, label) in PURGE_KINDS.iter() {
                        let Some(items) = list["nonPurgeable"][*key].as_array().filter(|a| !a.is_empty()) else { continue };
                        egui::CollapsingHeader::new(format!("{} ({})", crate::i18n::t(label), items.len())).id_salt(("purge_held_kind", *key)).show(
                            ui,
                            |ui| {
                                for it in items {
                                    let name = it["name"].as_str().unwrap_or_default();
                                    let sel = st.shown.as_ref().is_some_and(|(k, n)| k == key && n == name);
                                    if ui.selectable_label(sel, name).clicked() {
                                        st.shown = Some((key.to_string(), name.to_string()));
                                    }
                                }
                            },
                        );
                    }
                });
                ui.separator();
                let item = st.shown.as_ref().and_then(|(k, n)| list["nonPurgeable"][k.as_str()].as_array()?.iter().find(|i| i["name"] == n.as_str()));
                match item {
                    Some(it) => {
                        ui.label(RichText::new(it["name"].as_str().unwrap_or_default()).strong());
                        for r in it["reasons"].as_array().into_iter().flatten() {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(r["text"].as_str().unwrap_or_default()).color(t.text_dim));
                                let hs: Vec<String> =
                                    r["handles"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
                                if !hs.is_empty() && ui.small_button(crate::tl!("Select and Zoom")).clicked() {
                                    holders = Some(hs);
                                }
                            });
                        }
                    }
                    None => {
                        ui.label(RichText::new(crate::tl!("Pick an item to see why it can't be purged.")).small().color(t.text_dim));
                    }
                }
            } else {
                ui.label(RichText::new(crate::tl!("Unused items in this drawing")).strong());
                egui::ScrollArea::vertical().id_salt("purge_items").max_height(300.0).show(ui, |ui| {
                    for (key, _, label) in PURGE_KINDS.iter() {
                        let n = purgeable(&list, key);
                        let names = names_of(&list, key);
                        if names.is_empty() {
                            // A count only (zero-length geometry, empty text) or nothing.
                            let mut on = n > 0 && !st.off.contains(&(key.to_string(), String::new()));
                            ui.add_enabled_ui(n > 0, |ui| {
                                if ui.checkbox(&mut on, format!("{} ({n})", crate::i18n::t(label))).changed() {
                                    if on {
                                        st.off.remove(&(key.to_string(), String::new()));
                                    } else {
                                        st.off.insert((key.to_string(), String::new()));
                                    }
                                }
                            });
                            continue;
                        }
                        let all_on = names.iter().all(|nm| !st.off.contains(&(key.to_string(), nm.to_string())));
                        let id = ui.make_persistent_id(("purge_kind", *key));
                        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
                            .show_header(ui, |ui| {
                                let mut on = all_on;
                                if ui.checkbox(&mut on, format!("{} ({n})", crate::i18n::t(label))).changed() {
                                    for nm in &names {
                                        let k = (key.to_string(), nm.to_string());
                                        if on {
                                            st.off.remove(&k);
                                        } else {
                                            st.off.insert(k);
                                        }
                                    }
                                }
                            })
                            .body(|ui| {
                                for nm in &names {
                                    let k = (key.to_string(), nm.to_string());
                                    let mut on = !st.off.contains(&k);
                                    if ui.checkbox(&mut on, *nm).changed() {
                                        if on {
                                            st.off.remove(&k);
                                        } else {
                                            st.off.insert(k);
                                        }
                                    }
                                }
                            });
                    }
                });
                ui.separator();
                ui.checkbox(&mut st.confirm, crate::tl!("Confirm each item to be purged"));
                ui.checkbox(&mut st.nested, crate::tl!("Purge nested items"));
            }
            ui.separator();
            ui.horizontal(|ui| {
                let items = |only_checked: bool| -> Vec<(String, String, String)> {
                    let mut v = Vec::new();
                    for (key, ty, _) in PURGE_KINDS.iter() {
                        let names = names_of(&list, key);
                        if names.is_empty() {
                            if purgeable(&list, key) > 0 && !(only_checked && st.off.contains(&(key.to_string(), String::new()))) {
                                v.push((ty.to_string(), key.to_string(), String::new()));
                            }
                            continue;
                        }
                        for nm in names {
                            if !(only_checked && st.off.contains(&(key.to_string(), nm.to_string()))) {
                                v.push((ty.to_string(), key.to_string(), nm.to_string()));
                            }
                        }
                    }
                    v
                };
                let checked = if st.held_view { Vec::new() } else { items(true) };
                let all = if st.held_view { Vec::new() } else { items(false) };
                if ui.add_enabled(!checked.is_empty(), egui::Button::new(crate::tl!("Purge Checked Items"))).clicked() {
                    run = Some(checked);
                }
                if ui.add_enabled(!all.is_empty(), egui::Button::new(crate::tl!("Purge All"))).clicked() {
                    run = Some(all);
                }
                if ui.button(crate::tl!("Close")).clicked() {
                    close = true;
                }
            });
        });
    if let Some(items) = run {
        st.purged = 0;
        if st.confirm {
            st.queue = items;
        } else {
            st.purged = purge_items(app, &items, st.nested);
            app.session.echo(crate::tf!("Items purged: {count}", count = st.purged));
        }
    }
    if let Some(a) = answer {
        match a {
            "yes" => {
                if !st.queue.is_empty() {
                    let it = st.queue.remove(0);
                    st.purged += purge_items(app, &[it], st.nested);
                }
            }
            "all" => {
                let rest = std::mem::take(&mut st.queue);
                st.purged += purge_items(app, &rest, st.nested);
            }
            "no" => {
                if !st.queue.is_empty() {
                    st.queue.remove(0);
                }
            }
            _ => st.queue.clear(),
        }
        if st.queue.is_empty() {
            app.session.echo(crate::tf!("Items purged: {count}", count = st.purged));
        }
    }
    if let Some(hs) = holders {
        show_holders(app, &hs);
    }
    if close {
        *open = false;
    }
    ctx.data_mut(|d| d.insert_temp(state_id, st));
}

/// Purge `items` ((type, kind key, name)), one PURGE per kind; named kinds in PURGE All's order
/// (blocks free layers…) since the list keeps it.
fn purge_items(app: &mut CadApp, items: &[(String, String, String)], nested: bool) -> usize {
    let mut n = 0;
    for (_, ty, _) in PURGE_KINDS.iter() {
        let names: Vec<String> = items.iter().filter(|(t, _, _)| t == ty).map(|(_, _, nm)| nm.clone()).collect();
        if names.is_empty() {
            continue;
        }
        let whole = names.iter().all(String::is_empty);
        n += purge_names(app, ty, if whole { &[] } else { &names }, nested);
    }
    n
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
            // Right to left in Arabic (rtl.rs).
            let mut body = crate::rtl::Body::new(ui);
            let ui = &mut *body;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn purge_dialog_purges_the_checked_names_and_shows_why_others_stay() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        for n in ["Spare1", "Spare2"] {
            app.run("layer.new", json!({ "name": n })).unwrap();
        }
        {
            let d = app.session.doc_mut().unwrap();
            let h = d.new_handle();
            let zero = cadaddy_doc::Line { a: cadaddy_geom::Vec3::ZERO, b: cadaddy_geom::Vec3::ZERO };
            d.model.push(cadaddy_doc::Entity::new(h, cadaddy_doc::EntityKind::Line(zero)));
        }
        // Only Spare1 and the zero-length line.
        let items = vec![
            ("layers".to_string(), "layers".to_string(), "Spare1".to_string()),
            ("zerolength".to_string(), "zeroLength".to_string(), String::new()),
        ];
        assert_eq!(purge_items(&mut app, &items, true), 2);
        let d = app.session.doc().unwrap();
        assert!(d.layer("Spare1").is_none() && d.layer("Spare2").is_some() && d.model.is_empty());
        // Holders are selected and zoomed to.
        app.run("line", json!({ "points": [[0, 0], [10, 0]] })).unwrap();
        let h = app.session.doc().unwrap().model.iter().next().unwrap().handle.hex();
        show_holders(&mut app, std::slice::from_ref(&h));
        assert_eq!(app.session.selection().iter().map(|h| h.hex()).collect::<Vec<_>>(), vec![h]);
        // Both views render.
        app.ui.dialog = Some("purge".into());
        let ctx = egui::Context::default();
        for held in [false, true] {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("purge_state"), PurgeState { held_view: held, ..PurgeState::default() }));
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
            out.textures_delta.clear();
        }
        // Confirm each item: the question shows instead of the tree.
        let q = vec![("layers".to_string(), "layers".to_string(), "Spare2".to_string())];
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("purge_state"), PurgeState { queue: q, confirm: true, ..PurgeState::default() }));
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
        assert!(app.session.doc().unwrap().layer("Spare2").is_some(), "nothing goes before Yes");
    }
}
