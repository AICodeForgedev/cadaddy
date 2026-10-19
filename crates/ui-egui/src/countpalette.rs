//! The Count palette (COUNTLIST typed alone, Tools ▸ Count): every block in the drawing with how
//! many references it has (nested ones included), in this space or all of them; a click
//! highlights a block's references and zooms to them; a flag marks blocks with similar but not
//! identical copies (scaled references, exploded copies) that can be selected to check; a count
//! table that keeps itself up to date; and counting inside an area. Everything runs COUNT,
//! COUNTTABLE and the selection and zoom commands.

use egui::RichText;
use serde_json::{Value, json};

use crate::CadApp;

pub const DIALOG: &str = "countpalette";

#[derive(Clone, Default)]
struct State {
    all_spaces: bool,
    picked: Option<String>,
    status: String,
}

fn id() -> egui::Id {
    egui::Id::new("count_palette_state")
}

/// COUNT's list for the palette's scope (with the similar-copies check).
fn list(app: &mut CadApp, all: bool) -> Value {
    let space = if all { "all" } else { "current" };
    app.session.execute("count", &json!({ "space": space, "similar": true })).unwrap_or(Value::Null)
}

/// Select `handles` and zoom to them (the first one's layout made current).
fn show(app: &mut CadApp, handles: &[String]) {
    if handles.is_empty() {
        return;
    }
    let hs: Vec<cadaddy_engine::doc::Handle> = handles.iter().filter_map(|h| cadaddy_engine::doc::Handle::parse_hex(h)).collect();
    if let Some(first) = hs.first() {
        let space = app.session.doc().ok().and_then(|d| d.space_of(*first));
        if let Some(sp) = space
            && sp != app.session.space()
        {
            let name = match sp {
                cadaddy_engine::doc::Space::Model => "Model".to_string(),
                cadaddy_engine::doc::Space::Paper(n) => n,
            };
            let _ = app.run("layout", json!({ "option": "set", "name": name }));
        }
    }
    let _ = app.run("zoom.object", json!({ "handles": handles }));
    app.session.set_selection(hs);
}

/// Highlight block `name`'s references in the palette's scope.
fn highlight(app: &mut CadApp, name: &str, all: bool) -> usize {
    let space = if all { "all" } else { "current" };
    let r = app.session.execute("count", &json!({ "block": name, "space": space })).unwrap_or(Value::Null);
    let hs: Vec<String> = r["handles"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    show(app, &hs);
    hs.len()
}

pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
    // Worked out again only when the drawing or the scope changes.
    let key = (app.session.doc().map(|d| std::ptr::from_ref(d) as usize).unwrap_or(0), st.all_spaces, format!("{:?}", app.session.space()));
    let cache = egui::Id::new("count_palette_list");
    let data = match ctx.data_mut(|d| d.get_temp::<((usize, bool, String), Value)>(cache)) {
        Some((k, v)) if k == key => v,
        _ => {
            let v = list(app, st.all_spaces);
            ctx.data_mut(|d| d.insert_temp(cache, (key.clone(), v.clone())));
            v
        }
    };
    let mut pick: Option<String> = None;
    let mut similar: Option<Vec<String>> = None;
    let mut action: Option<&str> = None;
    egui::Window::new(crate::tl!("Count")).id(egui::Id::new("count_palette")).open(open).resizable(true).default_size([320.0, 420.0]).show(
        ctx,
        |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut st.all_spaces, false, crate::tl!("Current space"));
                ui.selectable_value(&mut st.all_spaces, true, crate::tl!("All spaces"));
            });
            let blocks = data["blocks"].as_object().cloned().unwrap_or_default();
            ui.label(RichText::new(crate::tf!("Total: {n}", n = data["total"].as_u64().unwrap_or(0))).small().color(t.text_dim));
            egui::ScrollArea::vertical().id_salt("count_list").max_height(300.0).show(ui, |ui| {
                if blocks.is_empty() {
                    ui.label(RichText::new(crate::tl!("No block references here.")).color(t.text_dim));
                }
                egui::Grid::new("count_grid").striped(true).num_columns(3).show(ui, |ui| {
                    for (name, n) in &blocks {
                        let sel = st.picked.as_deref() == Some(name.as_str());
                        if ui.selectable_label(sel, name).on_hover_text(crate::tl!("Highlight and zoom to its references")).clicked() {
                            pick = Some(name.clone());
                        }
                        ui.label(n.to_string());
                        let sim = &data["similar"][name.as_str()];
                        let scaled: Vec<String> =
                            sim["scaled"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
                        let exploded: Vec<String> = sim["exploded"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .flat_map(|g| g.as_array().into_iter().flatten())
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect();
                        let groups = sim["exploded"].as_array().map_or(0, Vec::len);
                        if !scaled.is_empty() || groups > 0 {
                            let tip = crate::tf!(
                                "Similar but not identical: {scaled} scaled, {exploded} exploded",
                                scaled = scaled.len(),
                                exploded = groups
                            );
                            if ui.button(RichText::new("⚠").color(t.warn)).on_hover_text(tip).clicked() {
                                similar = Some(scaled.into_iter().chain(exploded).collect());
                            }
                        } else {
                            ui.label("");
                        }
                        ui.end_row();
                    }
                });
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if ui.button(crate::tl!("Insert Count Table")).clicked() {
                    action = Some("table");
                }
                if ui.button(crate::tl!("Update Count Tables")).clicked() {
                    action = Some("update");
                }
                if ui.button(crate::tl!("Count in Area")).clicked() {
                    action = Some("area");
                }
            });
            if !st.status.is_empty() {
                ui.label(RichText::new(&st.status).small());
            }
        },
    );
    if let Some(name) = pick {
        let n = highlight(app, &name, st.all_spaces);
        st.status = crate::tf!("{n} reference(s) of {name} selected.", n = n, name = name);
        st.picked = Some(name);
    }
    if let Some(hs) = similar {
        st.status = crate::tf!("{n} similar object(s) selected.", n = hs.len());
        show(app, &hs);
    }
    match action {
        Some("table") => app.start("counttable"),
        Some("update") => {
            st.status = match app.run("counttable", json!({ "update": true })) {
                Ok(r) => r["message"].as_str().unwrap_or_default().to_string(),
                Err(e) => e,
            }
        }
        Some("area") => {
            app.cmdline("count");
            app.cmdline("a");
        }
        _ => {}
    }
    ctx.data_mut(|d| d.insert_temp(id(), st));
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_engine::doc::{Block, Circle, Entity, EntityKind, Insert};
    use cadaddy_engine::geom::Vec3;

    #[test]
    fn the_palette_lists_counts_and_highlights_references() {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        {
            let d = app.session.doc_mut().unwrap();
            let mut b = Block::new("CHAIR");
            let h = d.new_handle();
            b.entities.push(Entity::new(h, EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 1.0 })));
            d.blocks.insert("CHAIR".into(), std::sync::Arc::new(b));
            for x in [0.0, 10.0, 20.0] {
                let h = d.new_handle();
                let i = Insert {
                    block: "CHAIR".into(),
                    insert: Vec3::new(x, 0.0, 0.0),
                    scale: Vec3::new(1.0, 1.0, 1.0),
                    rotation: 0.0,
                    attribs: Vec::new(),
                    cols: 1,
                    rows: 1,
                    col_spacing: 0.0,
                    row_spacing: 0.0,
                    clip: None,
                };
                d.model.push(Entity::new(h, EntityKind::Insert(i)));
            }
        }
        assert_eq!(list(&mut app, false)["blocks"]["CHAIR"], 3);
        assert_eq!(highlight(&mut app, "chair", false), 3);
        assert_eq!(app.session.selection().len(), 3);
        // COUNTLIST typed alone opens it, and it renders.
        crate::menus::run_ui_command(&mut app, "countlist", &Value::Null);
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
    }
}
