//! The Blocks palette (BLOCKSPALETTE, Window ▸ Blocks): the current drawing's blocks, the ones
//! inserted lately (Recent), the user's Favourites, a Libraries folder (each drawing in it is a
//! block, inserted as INSERT's `NAME=file`) and CADaddy's Parts Library (over a thousand parts by
//! discipline and category, placed with PART at the drawing's units and scale), as tiles with
//! previews. Insertion options: scale,
//! rotation, Auto-rotate (the block turns with the current UCS) and Repeat Placement (INSERT
//! starts again after each placement until cancelled). Clicking a tile starts INSERT with the
//! options preset; dragging it onto the drawing area inserts it where it is dropped.

use std::cell::RefCell;
use std::collections::HashMap;

use cadaddy_engine::doc::library::parts::{self, Discipline, Part};
use egui::{Rect, RichText, Sense, Stroke, pos2, vec2};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::CadApp;

/// Most recent blocks kept.
pub const MAX_RECENT: usize = 30;
/// Most favourites kept.
pub const MAX_FAVOURITES: usize = 200;
/// Most drawings shown from a library folder.
const MAX_LIBRARY: usize = 500;
/// Most Parts Library tiles shown at once (the filter narrows the rest).
const MAX_PART_TILES: usize = 150;

/// A block the palette can insert: one of the current drawing's, or a drawing file as a block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// The palette's settings, kept with the interface state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub recent: Vec<BlockRef>,
    pub favourites: Vec<BlockRef>,
    /// The Libraries folder.
    pub library: Option<String>,
    pub scale: f64,
    /// Degrees.
    pub rotation: f64,
    pub auto_rotate: bool,
    pub repeat: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { recent: Vec::new(), favourites: Vec::new(), library: None, scale: 1.0, rotation: 0.0, auto_rotate: false, repeat: false }
    }
}

/// The rotation an insertion gets: the set angle, plus the current UCS's when Auto-rotate is on.
fn rotation(app: &CadApp) -> f64 {
    let s = &app.ui.blocks_palette;
    let ucs = if s.auto_rotate { app.session.ucs().angle.to_degrees() } else { 0.0 };
    let r = s.rotation + ucs;
    if r.is_finite() { r } else { 0.0 }
}

fn scale(app: &CadApp) -> f64 {
    let s = app.ui.blocks_palette.scale;
    if s.is_finite() && s != 0.0 { s } else { 1.0 }
}

fn remember(app: &mut CadApp, b: &BlockRef) {
    let r = &mut app.ui.blocks_palette.recent;
    r.retain(|x| x != b);
    r.insert(0, b.clone());
    r.truncate(MAX_RECENT);
}

/// What INSERT is given as the block name: the name, or `NAME=file` for a drawing file.
fn insert_name(b: &BlockRef) -> String {
    match &b.file {
        Some(f) => format!("{}={f}", b.name),
        None => b.name.clone(),
    }
}

/// Whether `b` is a part of the parts library: placed with PART, which defines its block (with its
/// dynamic parameters) and scales it to the drawing's units, so the palette's scale isn't used.
fn is_part(b: &BlockRef) -> bool {
    b.file.is_none() && parts::part(&b.name).is_some()
}

/// Start PART for library part `b` with the palette's rotation preset.
fn start_part(app: &mut CadApp, b: &BlockRef) {
    use cadaddy_engine::Input;
    if app.session.running.is_some() {
        app.session.cancel();
    }
    app.start("part");
    if app.session.running.as_ref().is_none_or(|r| r.id != "part") {
        return;
    }
    let mut steps = vec![Input::Text(b.name.clone())];
    let rot = rotation(app);
    if rot != 0.0 {
        steps.extend([Input::Keyword("Rotate".into()), Input::Text(format!("{rot}"))]);
    }
    for i in steps {
        if let Err(e) = app.session.input(i) {
            app.session.echo(e.to_string());
            return;
        }
    }
    remember(app, b);
    app.placing = Some((b.clone(), inserts(app)));
}

/// Start INSERT for `b` with the palette's scale and rotation preset, so the next prompt asks for
/// the insertion point (PART for a library part).
pub fn start_insert(app: &mut CadApp, b: &BlockRef) {
    use cadaddy_engine::Input;
    if is_part(b) {
        return start_part(app, b);
    }
    app.start("insert");
    if app.session.running.as_ref().is_none_or(|r| r.id != "insert") {
        return;
    }
    let steps = [
        Input::Text(insert_name(b)),
        Input::Keyword("Scale".into()),
        Input::Text(format!("{}", scale(app))),
        Input::Keyword("Rotate".into()),
        Input::Text(format!("{}", rotation(app))),
    ];
    for i in steps {
        if let Err(e) = app.session.input(i) {
            app.session.echo(e.to_string());
            return;
        }
    }
    remember(app, b);
    app.placing = Some((b.clone(), inserts(app)));
}

/// How many block references the current space holds (a placement adds one).
fn inserts(app: &CadApp) -> usize {
    app.session.doc().map_or(0, |d| d.model.iter().filter(|e| matches!(e.kind, cadaddy_engine::doc::EntityKind::Insert(_))).count())
}

/// Each frame: with Repeat Placement on, INSERT starts again after a placement; a cancelled one
/// ends it.
pub fn follow(app: &mut CadApp) {
    let Some((b, before)) = app.placing.clone() else { return };
    if app.session.running.is_some() {
        return;
    }
    app.placing = None;
    if app.ui.blocks_palette.repeat && inserts(app) > before {
        start_insert(app, &b);
    }
}

/// Insert `b` at `at` (a drop on the drawing area) with the palette's options.
pub fn insert_at(app: &mut CadApp, b: &BlockRef, at: [f64; 2]) -> Result<serde_json::Value, String> {
    if is_part(b) {
        let r = app.run("part", json!({ "name": b.name, "at": at, "rotation": rotation(app) }))?;
        remember(app, b);
        return Ok(r);
    }
    let mut p = json!({ "name": b.name, "at": at, "scale": scale(app), "rotation": rotation(app) });
    if let Some(f) = &b.file {
        p["file"] = json!(f);
    }
    let r = app.run("insert", p)?;
    remember(app, b);
    Ok(r)
}

/// Toggle `b` among the favourites.
pub fn toggle_favourite(app: &mut CadApp, b: &BlockRef) {
    let f = &mut app.ui.blocks_palette.favourites;
    if f.contains(b) {
        f.retain(|x| x != b);
    } else if f.len() < MAX_FAVOURITES {
        f.push(b.clone());
    }
}

/// A drawing file's model space, read for a preview.
fn read_file(path: &str) -> Option<cadaddy_engine::doc::Drawing> {
    let hooks = cadaddy_engine::cmd::file::io()?;
    let bytes = std::fs::read(path).ok().filter(|b| b.len() < 64 << 20)?;
    (hooks.read)(&bytes, path).ok()
}

/// A tile's picture in a unit box: polylines and filled triangles.
#[derive(Clone, Default)]
struct Pic {
    lines: Vec<Vec<[f32; 2]>>,
    fills: Vec<[[f32; 2]; 3]>,
}

/// Polylines in a unit box for a preview of `ents` in `d`.
fn preview<'a>(d: &cadaddy_engine::doc::Drawing, ents: impl IntoIterator<Item = &'a cadaddy_engine::doc::Entity>) -> Pic {
    let list = cadaddy_render::build_entities(d, ents, &cadaddy_render::Options::default());
    let b = list.bounds;
    if b.is_empty() {
        return Pic::default();
    }
    let size = b.width().max(b.height()).max(1e-9);
    let c = b.center();
    let mut out = Vec::new();
    let mut n = 0usize;
    for prim in list.prims.iter().filter(|p| matches!(p.kind, cadaddy_render::Kind::Polyline)) {
        let (s, l) = (prim.start as usize, prim.len as usize);
        let Some(pts) = list.verts.get(s..s.saturating_add(l)) else { continue };
        let step = (pts.len() / 100).max(1);
        let line: Vec<[f32; 2]> = pts.iter().step_by(step).map(|v| [((v.x - c.x) / size + 0.5) as f32, ((v.y - c.y) / size + 0.5) as f32]).collect();
        n += line.len();
        if n > 1500 {
            break;
        }
        if line.len() >= 2 {
            out.push(line);
        }
    }
    Pic { lines: out, fills: Vec::new() }
}

thread_local! {
    /// Library part pictures made so far (each part's drawing never changes).
    static PART_PICS: RefCell<HashMap<&'static str, Pic>> = RefCell::new(HashMap::new());
}

/// The picture of library part `p`, its lines and fills in a unit box.
fn part_pic(p: &'static Part) -> Pic {
    if let Some(pic) = PART_PICS.with(|c| c.borrow().get(p.name.as_str()).cloned()) {
        return pic;
    }
    let pv = cadaddy_engine::cmd::parts::preview(p);
    let b = pv.bounds;
    let pic = if b.is_empty() {
        Pic::default()
    } else {
        let size = b.width().max(b.height()).max(1e-9);
        let c = b.center();
        let at = |v: &cadaddy_engine::geom::Vec2| [((v.x - c.x) / size + 0.5) as f32, ((v.y - c.y) / size + 0.5) as f32];
        let mut n = 0usize;
        let mut lines = Vec::new();
        for run in &pv.lines {
            let step = (run.len() / 100).max(1);
            let line: Vec<[f32; 2]> = run.iter().step_by(step).map(at).collect();
            n += line.len();
            if n > 3000 {
                break;
            }
            if line.len() >= 2 {
                lines.push(line);
            }
        }
        let fills = pv.fills.iter().take(2000).map(|t| [at(&t[0]), at(&t[1]), at(&t[2])]).collect();
        Pic { lines, fills }
    };
    PART_PICS.with(|c| c.borrow_mut().insert(p.name.as_str(), pic.clone()));
    pic
}

/// The preview of `b`: a block of the current drawing, a library part not placed yet, or a
/// drawing file's model space.
fn preview_of(app: &CadApp, b: &BlockRef) -> Pic {
    match &b.file {
        Some(f) => read_file(f).map(|d| preview(&d, d.model.iter().map(|e| &**e))).unwrap_or_default(),
        None => app
            .session
            .doc()
            .ok()
            .and_then(|d| Some(preview(d, d.block(&b.name)?.entities.iter().map(|e| &**e))))
            .or_else(|| parts::part(&b.name).map(part_pic))
            .unwrap_or_default(),
    }
}

/// The parts library's parts in discipline `discipline` (0 every one, else `Discipline::ALL`
/// index + 1) and category (empty: every one) that `query` finds.
fn library_parts(discipline: usize, category: &str, query: &str) -> Vec<&'static Part> {
    let d = discipline.checked_sub(1).and_then(|i| Discipline::ALL.get(i).copied());
    parts::parts()
        .iter()
        .filter(|p| d.is_none_or(|d| p.discipline == d) && (category.is_empty() || p.category == category) && p.matches(query))
        .collect()
}

/// The drawings of the Libraries folder, each a block named after the file.
fn library_blocks(folder: &str) -> Vec<BlockRef> {
    let Ok(rd) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut v: Vec<BlockRef> = rd
        .flatten()
        .take(MAX_LIBRARY)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| ["dwg", "dxf", "cdy"].iter().any(|k| x.eq_ignore_ascii_case(k))))
        .filter_map(|p| Some(BlockRef { name: p.file_stem()?.to_string_lossy().into_owned(), file: Some(p.to_string_lossy().into_owned()) }))
        .collect();
    v.sort_by_key(|b| b.name.to_lowercase());
    v
}

#[derive(Clone, Default)]
struct State {
    tab: u8,
    filter: String,
    dragging: Option<BlockRef>,
    /// Previews made so far, by tile.
    previews: Vec<(BlockRef, Pic)>,
    /// The Parts Library's discipline (0 every one, else `Discipline::ALL` index + 1) and
    /// category (empty: every one).
    discipline: usize,
    category: String,
}

fn id() -> egui::Id {
    egui::Id::new("cadaddy-blocks-palette")
}

/// The palette window.
pub fn dialog(app: &mut CadApp, ctx: &egui::Context, open: &mut bool) {
    let t = crate::theme::Tokens::get();
    let mut st: State = ctx.data(|d| d.get_temp(id())).unwrap_or_default();
    let mut insert: Option<BlockRef> = None;
    let mut fav: Option<BlockRef> = None;
    let mut pick_library = false;
    let window = egui::Window::new(crate::tl!("Blocks"))
        .id(egui::Id::new("Blocks"))
        .default_pos(crate::palette_pos(app))
        .open(open)
        .resizable(true)
        .default_size([360.0, 480.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (i, l) in ["Current Drawing", "Recent", "Favourites", "Libraries", "Parts Library"].iter().enumerate() {
                    if ui.selectable_label(st.tab == i as u8, crate::i18n::t(l)).clicked() {
                        st.tab = i as u8;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Filter"));
                let r = ui.text_edit_singleline(&mut st.filter);
                crate::icons::describe_area(&r, "Filter");
            });
            let mut more = None;
            let tiles: Vec<BlockRef> = match st.tab {
                0 => app
                    .session
                    .doc()
                    .map(|d| {
                        d.blocks
                            .iter()
                            .filter(|(n, b)| !n.starts_with('*') && !b.anonymous && b.xref_path.is_none() && !n.contains('|'))
                            .map(|(n, _)| BlockRef { name: n.clone(), file: None })
                            .collect()
                    })
                    .unwrap_or_default(),
                1 => app.ui.blocks_palette.recent.clone(),
                2 => app.ui.blocks_palette.favourites.clone(),
                4 => {
                    ui.horizontal(|ui| {
                        let all = crate::tl!("All");
                        let shown = st
                            .discipline
                            .checked_sub(1)
                            .and_then(|i| Discipline::ALL.get(i))
                            .map_or(all.to_string(), |d| crate::i18n::t(d.label()).to_string());
                        egui::ComboBox::from_id_salt("parts-discipline").selected_text(shown).width(130.0).show_ui(ui, |ui| {
                            if ui.selectable_label(st.discipline == 0, all).clicked() {
                                st.discipline = 0;
                                st.category.clear();
                            }
                            for (i, d) in Discipline::ALL.iter().enumerate() {
                                if ui.selectable_label(st.discipline == i + 1, crate::i18n::t(d.label())).clicked() {
                                    st.discipline = i + 1;
                                    st.category.clear();
                                }
                            }
                        });
                        let cats: Vec<&'static str> = match st.discipline.checked_sub(1).and_then(|i| Discipline::ALL.get(i)) {
                            Some(d) => parts::categories(*d),
                            None => Discipline::ALL.iter().flat_map(|d| parts::categories(*d)).collect(),
                        };
                        let shown = if st.category.is_empty() { all.to_string() } else { crate::i18n::t(&st.category).to_string() };
                        egui::ComboBox::from_id_salt("parts-category").selected_text(shown).width(170.0).show_ui(ui, |ui| {
                            if ui.selectable_label(st.category.is_empty(), all).clicked() {
                                st.category.clear();
                            }
                            for c in cats {
                                if ui.selectable_label(st.category == c, crate::i18n::t(c)).clicked() {
                                    st.category = c.to_string();
                                }
                            }
                        });
                    });
                    let found = library_parts(st.discipline, &st.category, &st.filter);
                    if found.len() > MAX_PART_TILES {
                        more = Some(found.len());
                    }
                    found.into_iter().take(MAX_PART_TILES).map(|p| BlockRef { name: p.name.clone(), file: None }).collect()
                }
                _ => {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(app.ui.blocks_palette.library.clone().unwrap_or_else(|| crate::tl!("No library folder").to_string()))
                                .small()
                                .color(t.text_dim),
                        );
                        pick_library = ui.button(crate::tl!("Choose Folder...")).clicked();
                    });
                    app.ui.blocks_palette.library.as_deref().map(library_blocks).unwrap_or_default()
                }
            };
            // The Parts Library's search is its own (labels and keywords too).
            let q = if st.tab == 4 { String::new() } else { st.filter.trim().to_lowercase() };
            if let Some(total) = more {
                ui.label(
                    RichText::new(crate::tf!("Showing {shown} of {total} parts; type in Filter to narrow.", shown = MAX_PART_TILES, total = total))
                        .small()
                        .color(t.text_dim),
                );
            }
            egui::ScrollArea::vertical().id_salt("blocks-tiles").max_height(300.0).show(ui, |ui| {
                if tiles.is_empty() {
                    ui.label(RichText::new(crate::tl!("No blocks here.")).color(t.text_dim));
                }
                ui.horizontal_wrapped(|ui| {
                    for b in tiles.iter().filter(|b| q.is_empty() || b.name.to_lowercase().contains(&q)) {
                        let pv = match st.previews.iter().find(|(k, _)| k == b) {
                            Some((_, p)) => p.clone(),
                            None => {
                                let p = preview_of(app, b);
                                st.previews.push((b.clone(), p.clone()));
                                p
                            }
                        };
                        let (rect, resp) = ui.allocate_exact_size(vec2(84.0, 96.0), Sense::click_and_drag());
                        let p = ui.painter();
                        p.rect_filled(rect, 3.0, if resp.hovered() { t.control_hover } else { t.panel });
                        let pic = Rect::from_min_size(rect.min + vec2(11.0, 4.0), vec2(62.0, 62.0));
                        let at = |q: &[f32; 2]| pos2(pic.left() + q[0] * pic.width(), pic.bottom() - q[1] * pic.height());
                        for line in &pv.lines {
                            p.add(egui::Shape::line(line.iter().map(at).collect(), Stroke::new(1.0, t.text)));
                        }
                        for tri in &pv.fills {
                            p.add(egui::Shape::convex_polygon(tri.iter().map(at).collect(), t.text, Stroke::NONE));
                        }
                        let favourite = app.ui.blocks_palette.favourites.contains(b);
                        p.text(
                            rect.right_top() + vec2(-8.0, 8.0),
                            egui::Align2::CENTER_CENTER,
                            if favourite { "★" } else { "☆" },
                            crate::theme::small(),
                            t.accent,
                        );
                        // The name on up to two lines, cut to the tile with an ellipsis (hover shows
                        // it whole); a library part without its discipline letter.
                        let shown = match parts::part(&b.name) {
                            Some(part) if b.file.is_none() => {
                                b.name.strip_prefix(&format!("{}_", part.discipline.prefix())).unwrap_or(&b.name).to_string()
                            }
                            _ => b.name.clone(),
                        };
                        let mut job = egui::text::LayoutJob::simple_singleline(shown, crate::theme::small(), t.text);
                        job.wrap = egui::text::TextWrapping {
                            max_width: rect.width() - 4.0,
                            max_rows: 2,
                            break_anywhere: true,
                            overflow_character: Some('…'),
                        };
                        job.halign = egui::Align::Center;
                        let galley = p.layout_job(job);
                        p.galley(pos2(rect.center().x, rect.top() + 68.0), galley, t.text);
                        crate::icons::describe_control(ui, &resp, &b.name, None, false);
                        let resp = match parts::part(&b.name) {
                            Some(part) => resp.on_hover_text(&part.label),
                            None => resp,
                        };
                        if resp.clicked() {
                            insert = Some(b.clone());
                        }
                        if resp.drag_started() {
                            st.dragging = Some(b.clone());
                        }
                        resp.context_menu(|ui| {
                            if ui.button(if favourite { crate::tl!("Remove from Favourites") } else { crate::tl!("Add to Favourites") }).clicked() {
                                fav = Some(b.clone());
                                ui.close();
                            }
                        });
                    }
                });
            });
            ui.separator();
            ui.label(RichText::new(crate::tl!("Insertion Options")).strong());
            let s = &mut app.ui.blocks_palette;
            ui.horizontal(|ui| {
                ui.label(crate::tl!("Scale"));
                ui.add(egui::DragValue::new(&mut s.scale).speed(0.05).range(0.0001..=1.0e6));
                ui.label(crate::tl!("Rotation"));
                ui.add(egui::DragValue::new(&mut s.rotation).speed(1.0).suffix("°"));
            });
            ui.checkbox(&mut s.auto_rotate, crate::tl!("Auto-rotate (follow the current UCS)"));
            ui.checkbox(&mut s.repeat, crate::tl!("Repeat Placement"));
        });
    if let Some(b) = insert {
        start_insert(app, &b);
    }
    if let Some(b) = fav {
        toggle_favourite(app, &b);
    }
    if pick_library && let Some(Some(path)) = app.services.pick_open.as_ref().map(|p| p()) {
        let p = std::path::Path::new(&path);
        let folder = if p.is_dir() { path.clone() } else { p.parent().map(|d| d.to_string_lossy().into_owned()).unwrap_or(path.clone()) };
        app.ui.blocks_palette.library = Some(folder);
        st.previews.clear();
    }
    // A tile dropped on the drawing area: inserted there.
    if let Some(b) = st.dragging.clone() {
        if ctx.input(|i| i.pointer.any_released()) {
            st.dragging = None;
            let over_palette = window.as_ref().is_some_and(|w| ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| w.response.rect.contains(p)));
            if !over_palette
                && let Some(at) = app.canvas.cursor
                && let Err(e) = insert_at(app, &b, [at.x, at.y])
            {
                app.session.echo(e);
            }
        } else if let Some(p) = ctx.input(|i| i.pointer.hover_pos()) {
            egui::Area::new(egui::Id::new("blocks-drag")).fixed_pos(p + vec2(12.0, 12.0)).interactable(false).show(ctx, |ui| {
                ui.label(RichText::new(&b.name).strong());
            });
        }
    }
    if *open {
        ctx.data_mut(|d| d.insert_temp(id(), st));
    } else {
        ctx.data_mut(|d| d.remove::<State>(id()));
    }
}
