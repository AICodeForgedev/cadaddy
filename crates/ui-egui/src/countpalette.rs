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
    /// The space a selection the status line reports was made in: the line goes when the
    /// selection is cleared or the space changes (battle test 0.7.8 #18).
    selected_in: Option<String>,
}

fn id() -> egui::Id {
    egui::Id::new("count_palette_state")
}

/// COUNT's list for the palette's scope (with the similar-copies check).
fn list(app: &mut CadApp, all: bool) -> Value {
    let space = if all { "all" } else { "current" };
    app.session.execute("count", &json!({ "space": space, "similar": true })).unwrap_or(Value::Null)
}

/// Select `handles` and zoom to them (the first one's layout made current), in the part of the
/// drawing area the palette at `covered` leaves free.
fn show(app: &mut CadApp, handles: &[String], covered: Option<egui::Rect>) {
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
    clear_of(app, covered);
    app.session.set_selection(hs);
}

/// Margins around the free part of the drawing area (points): the ViewCube and navigation bar
/// on the right, the floating command line at the bottom.
const MARGIN: [f32; 4] = [40.0, 40.0, 110.0, 80.0];

/// The part of `canvas` that a palette at `covered` leaves free (its wider side), inside
/// [`MARGIN`]; `None` when too little is left to show anything.
fn free_area(canvas: egui::Rect, covered: Option<egui::Rect>) -> Option<egui::Rect> {
    let mut f = canvas;
    if let Some(c) = covered.filter(|c| c.intersects(canvas)) {
        if canvas.right() - c.right() >= c.left() - canvas.left() {
            f.min.x = c.right().max(canvas.left());
        } else {
            f.max.x = c.left().min(canvas.right());
        }
    }
    let [l, t, r, b] = MARGIN;
    let f = egui::Rect::from_min_max(f.min + egui::vec2(l, t), f.max - egui::vec2(r, b));
    (f.width() >= 60.0 && f.height() >= 60.0).then_some(f)
}

/// After a zoom that fills the drawing area: zoom out and pan so what it showed fits the free
/// part (the references under the palette, the ViewCube or the command line before: battle test
/// 0.7.8 #18). Not inside a layout viewport, which is only part of the drawing area.
fn clear_of(app: &mut CadApp, covered: Option<egui::Rect>) {
    let Some(canvas) = app.canvas.rect.filter(|r| r.width() > 1.0 && r.height() > 1.0) else { return };
    let Some(free) = free_area(canvas, covered) else { return };
    let Ok(st) = app.session.state() else { return };
    if st.zoom_viewport().is_some() {
        return;
    }
    let v = st.zoom_frame();
    let k = f64::from((canvas.width() / free.width()).max(canvas.height() / free.height()));
    let height = v.height * k;
    // World units per point at the new height; the screen's y goes down.
    let per = height / f64::from(canvas.height());
    let d = free.center() - canvas.center();
    let center = [v.center.x - f64::from(d.x) * per, v.center.y + f64::from(d.y) * per];
    let _ = app.run("zoom", json!({ "mode": "center", "center": center, "height": height }));
}

/// Highlight block `name`'s references in the palette's scope.
fn highlight(app: &mut CadApp, name: &str, all: bool, covered: Option<egui::Rect>) -> usize {
    let space = if all { "all" } else { "current" };
    let r = app.session.execute("count", &json!({ "block": name, "space": space })).unwrap_or(Value::Null);
    let hs: Vec<String> = r["handles"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
    show(app, &hs, covered);
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
    // The status line reports a selection: it goes with the selection, or when the space changes.
    let selected = app.session.state().is_ok_and(|s| !s.selection.is_empty());
    if let Some(space) = &st.selected_in
        && (!selected || *space != format!("{:?}", app.session.space()))
    {
        st.status.clear();
        st.picked = None;
        st.selected_in = None;
    }
    let window = egui::Window::new(crate::tl!("Count"))
        .id(egui::Id::new("count_palette"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([320.0, 420.0])
        .show(ctx, |ui| {
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
        });
    let covered = window.map(|w| w.response.rect);
    if let Some(name) = pick {
        let n = highlight(app, &name, st.all_spaces, covered);
        st.status = crate::tf!("{n} reference(s) of {name} selected.", n = n, name = name);
        st.picked = Some(name);
        st.selected_in = (n > 0).then(|| format!("{:?}", app.session.space()));
    }
    if let Some(hs) = similar {
        st.status = crate::tf!("{n} similar object(s) selected.", n = hs.len());
        show(app, &hs, covered);
        st.selected_in = (!hs.is_empty()).then(|| format!("{:?}", app.session.space()));
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
        assert_eq!(highlight(&mut app, "chair", false, None), 3);
        assert_eq!(app.session.selection().len(), 3);
        // COUNTLIST typed alone opens it, and it renders.
        crate::menus::run_ui_command(&mut app, "countlist", &Value::Null);
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(&mut app, ui.ctx()));
        out.textures_delta.clear();
    }

    /// The CHAIR drawing of the test above: references at x = 0, 10, 20.
    fn chairs() -> CadApp {
        let mut app = CadApp::new(cadaddy_engine::Session::new(), crate::Services::default());
        let d = app.session.doc_mut().unwrap();
        let mut b = Block::new("CHAIR");
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 1.0 })));
        d.blocks.insert("CHAIR".into(), std::sync::Arc::new(b));
        for x in [0.0, 10.0, 20.0] {
            let h = d.new_handle();
            let i: Insert = serde_json::from_value(json!({"block": "CHAIR", "insert": Vec3::new(x, 0.0, 0.0)})).unwrap();
            d.model.push(Entity::new(h, EntityKind::Insert(i)));
        }
        app
    }

    #[test]
    fn tools_count_opens_the_palette() {
        let spec = cadaddy_engine::cmd::command_specs().iter().find(|c| c.menu == ["Tools", "Count"]).unwrap();
        assert_eq!(spec.id, "countlist");
        let mut app = chairs();
        app.start(spec.id);
        assert_eq!(app.ui.dialog.as_deref(), Some(DIALOG));
        assert!(app.session.running.is_none(), "not COUNT's prompts");
    }

    #[test]
    fn a_row_click_zooms_clear_of_the_palette_with_a_margin() {
        let mut app = chairs();
        let canvas = egui::Rect::from_min_size(egui::pos2(0.0, 150.0), egui::vec2(1200.0, 700.0));
        app.canvas.rect = Some(canvas);
        app.session.viewport_px = (1200.0, 700.0);
        // The palette over the drawing area's left part.
        let palette = egui::Rect::from_min_size(egui::pos2(12.0, 162.0), egui::vec2(320.0, 420.0));
        assert_eq!(highlight(&mut app, "CHAIR", false, Some(palette)), 3);
        let free = free_area(canvas, Some(palette)).unwrap();
        assert!(free.left() >= palette.right() + MARGIN[0] - 1e-3 && free.bottom() <= canvas.bottom() - MARGIN[3] + 1e-3, "{free:?}");
        // Every reference (x -1..21, y -1..1) lands inside the free part, on screen.
        let v = app.session.state().unwrap().zoom_frame();
        let per = v.height / f64::from(canvas.height());
        let screen =
            |x: f64, y: f64| egui::pos2(canvas.center().x + ((x - v.center.x) / per) as f32, canvas.center().y - ((y - v.center.y) / per) as f32);
        for (x, y) in [(-1.0, -1.0), (21.0, 1.0)] {
            assert!(free.contains(screen(x, y)), "{:?} outside {free:?}", screen(x, y));
        }
    }

    #[test]
    fn the_status_line_goes_with_the_selection() {
        let mut app = chairs();
        let ctx = egui::Context::default();
        app.ui.dialog = Some(DIALOG.into());
        let frame = |app: &mut CadApp| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| crate::dialogs::show(app, ui.ctx()));
            out.textures_delta.clear();
        };
        frame(&mut app);
        let n = highlight(&mut app, "CHAIR", false, None);
        ctx.data_mut(|d| {
            let st: &mut State = d.get_temp_mut_or_default(id());
            st.status = format!("{n} reference(s) of CHAIR selected.");
            st.picked = Some("CHAIR".into());
            st.selected_in = Some(format!("{:?}", app.session.space()));
        });
        frame(&mut app);
        assert!(!ctx.data(|d| d.get_temp::<State>(id())).unwrap().status.is_empty(), "still selected");
        app.session.set_selection(Vec::new());
        frame(&mut app);
        let st = ctx.data(|d| d.get_temp::<State>(id())).unwrap();
        assert!(st.status.is_empty() && st.picked.is_none());
    }
}
