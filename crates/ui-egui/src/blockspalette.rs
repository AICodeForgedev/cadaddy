//! The Blocks palette (BLOCKSPALETTE, Window ▸ Blocks): the current drawing's blocks, the ones
//! inserted lately (Recent), the user's Favourites and a Libraries folder (each drawing in it is a
//! block, inserted as INSERT's `NAME=file`), as tiles with previews. Insertion options: scale,
//! rotation, Auto-rotate (the block turns with the current UCS) and Repeat Placement (INSERT
//! starts again after each placement until cancelled). Clicking a tile starts INSERT with the
//! options preset; dragging it onto the drawing area inserts it where it is dropped.

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

/// Start INSERT for `b` with the palette's scale and rotation preset, so the next prompt asks for
/// the insertion point.
pub fn start_insert(app: &mut CadApp, b: &BlockRef) {
    use cadaddy_engine::Input;
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

/// Polylines in a unit box for a preview of `ents` in `d`.
fn preview<'a>(d: &cadaddy_engine::doc::Drawing, ents: impl IntoIterator<Item = &'a cadaddy_engine::doc::Entity>) -> Vec<Vec<[f32; 2]>> {
    let list = cadaddy_render::build_entities(d, ents, &cadaddy_render::Options::default());
    let b = list.bounds;
    if b.is_empty() {
        return Vec::new();
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
    out
}

/// The preview of `b`: a block of the current drawing, or a drawing file's model space.
fn preview_of(app: &CadApp, b: &BlockRef) -> Vec<Vec<[f32; 2]>> {
    match &b.file {
        Some(f) => read_file(f).map(|d| preview(&d, d.model.iter().map(|e| &**e))).unwrap_or_default(),
        None => app.session.doc().ok().and_then(|d| Some(preview(d, d.block(&b.name)?.entities.iter().map(|e| &**e)))).unwrap_or_default(),
    }
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
    previews: Vec<(BlockRef, Vec<Vec<[f32; 2]>>)>,
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
    let window =
        egui::Window::new(crate::tl!("Blocks")).id(egui::Id::new("Blocks")).open(open).resizable(true).default_size([360.0, 480.0]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (i, l) in ["Current Drawing", "Recent", "Favourites", "Libraries"].iter().enumerate() {
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
            let q = st.filter.trim().to_lowercase();
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
                        let pic = Rect::from_min_size(rect.min + vec2(8.0, 6.0), vec2(68.0, 68.0));
                        for line in &pv {
                            let pts: Vec<egui::Pos2> =
                                line.iter().map(|q| pos2(pic.left() + q[0] * pic.width(), pic.bottom() - q[1] * pic.height())).collect();
                            p.add(egui::Shape::line(pts, Stroke::new(1.0, t.text)));
                        }
                        let favourite = app.ui.blocks_palette.favourites.contains(b);
                        p.text(
                            rect.right_top() + vec2(-8.0, 8.0),
                            egui::Align2::CENTER_CENTER,
                            if favourite { "★" } else { "☆" },
                            crate::theme::small(),
                            t.accent,
                        );
                        p.text(pos2(rect.center().x, rect.bottom() - 10.0), egui::Align2::CENTER_CENTER, &b.name, crate::theme::small(), t.text);
                        crate::icons::describe_control(ui, &resp, &b.name, None, false);
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
