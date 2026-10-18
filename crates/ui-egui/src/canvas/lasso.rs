//! Lasso selection and selection cycling on the drawing area.
//!
//! A press-and-drag that opens a selection window becomes a lasso once its path encloses an area
//! (a straight drag stays the rectangle window): dragged rightward a window lasso, leftward a
//! crossing lasso; Space while dragging switches Window, Crossing, Fence. Selection cycling
//! (SELECTIONCYCLING): Shift+Space steps the highlight through the objects under the cursor and
//! the next click picks the highlighted one; with SELECTIONCYCLING 2 a click on overlapping objects
//! lists them to choose from instead.

use cadaddy_doc::Handle;
use cadaddy_engine::select::LassoMode;
use cadaddy_geom::Vec2;
use egui::{Color32, Pos2, Rect, Shape, Stroke, vec2};

use super::Xf;
use crate::CadApp;
use crate::theme::Tokens;

/// Screen points closer than this (pixels) to the last one aren't added to the path.
const STEP_PX: f32 = 2.0;
/// Most screen points a lasso path keeps.
const MAX_POINTS: usize = 4000;
/// How far (pixels) the drag must go sideways before its direction picks window or crossing.
const DIRECTION_PX: f32 = 3.0;
/// The least area (square pixels) a path must enclose to be a lasso.
const MIN_AREA_PX: f32 = 400.0;
/// The least share of its bounding box a path must enclose to be a lasso: a straight or gently
/// bowed drag from corner to corner stays a rectangle window.
const MIN_FILL: f32 = 0.25;

/// A press-and-drag selection in progress: the path the pointer took on screen.
#[derive(Clone, Debug, Default)]
pub(crate) struct Lasso {
    pts: Vec<Pos2>,
    /// Chosen by the drag's direction, then by Space.
    mode: Option<LassoMode>,
    /// Space was pressed: a lasso however little area the path encloses (a fence encloses none).
    forced: bool,
}

impl Lasso {
    pub(crate) fn new(origin: Pos2) -> Self {
        Lasso { pts: vec![origin], mode: None, forced: false }
    }

    /// Follow the pointer to `p`.
    pub(crate) fn track(&mut self, p: Pos2) {
        let (Some(first), Some(last)) = (self.pts.first().copied(), self.pts.last().copied()) else { return };
        if self.mode.is_none() && (p.x - first.x).abs() > DIRECTION_PX {
            self.mode = Some(if p.x > first.x { LassoMode::Window } else { LassoMode::Crossing });
        }
        if last.distance(p) >= STEP_PX && self.pts.len() < MAX_POINTS && p.x.is_finite() && p.y.is_finite() {
            self.pts.push(p);
        }
    }

    pub(crate) fn mode(&self) -> LassoMode {
        self.mode.unwrap_or(LassoMode::Window)
    }

    /// Space: the next mode (Window, Crossing, Fence).
    pub(crate) fn cycle_mode(&mut self) {
        self.mode = Some(self.mode().next());
        self.forced = true;
    }

    /// Whether the drag selects as a lasso rather than as the rectangle between its ends.
    pub(crate) fn is_lasso(&self) -> bool {
        if self.forced {
            return true;
        }
        if self.pts.len() < 3 {
            return false;
        }
        let area = polygon_area(&self.pts).abs();
        let bb = Rect::from_points(&self.pts);
        area >= MIN_AREA_PX && area >= MIN_FILL * bb.area()
    }

    /// The path in drawing coordinates, ending at `end`.
    fn world_path(&self, xf: &Xf, end: Pos2) -> Vec<Vec2> {
        self.pts.iter().copied().chain(std::iter::once(end)).map(|p| xf.s2w(p)).collect()
    }
}

/// The signed area of the polygon closing `pts` (shoelace).
fn polygon_area(pts: &[Pos2]) -> f32 {
    let n = pts.len();
    let mut twice = 0.0;
    for (i, a) in pts.iter().enumerate() {
        if let Some(b) = pts.get((i + 1) % n) {
            twice += a.x * b.y - b.x * a.y;
        }
    }
    twice / 2.0
}

/// Whether the press-and-drag in progress is drawing a lasso.
pub(crate) fn dragging(app: &CadApp) -> bool {
    app.canvas.drag_window && app.canvas.lasso.as_ref().is_some_and(Lasso::is_lasso)
}

/// The press-and-drag ended at `end`: a lasso selects along its path (true); a drag that isn't
/// one is left to close the rectangle window (false).
pub(crate) fn finish(app: &mut CadApp, xf: &Xf, end: Pos2, shift: bool) -> bool {
    let Some(l) = app.canvas.lasso.take().filter(Lasso::is_lasso) else { return false };
    let path = l.world_path(xf, end);
    if let Err(e) = app.session.lasso(&path, l.mode(), shift) {
        app.session.echo(e.to_string());
    }
    true
}

/// The lasso outline: a window lasso solid on its area tint, a crossing lasso dashed on its tint
/// (SELECTIONAREA, WINDOWAREACOLOR, CROSSINGAREACOLOR), a fence a dashed open path. Its mode is
/// named beside the cursor.
pub(crate) fn draw(app: &CadApp, p: &egui::Painter, cursor: Pos2) {
    let Some(l) = app.canvas.lasso.as_ref().filter(|l| l.is_lasso()) else { return };
    let t = Tokens::get();
    let mode = l.mode();
    let mut pts = l.pts.clone();
    pts.push(cursor);
    if mode != LassoMode::Fence {
        if let Some(fill) = super::selection_area_fill(&app.session.settings, mode == LassoMode::Crossing) {
            fill_polygon(p, &pts, fill);
        }
        if let Some(first) = pts.first().copied() {
            pts.push(first);
        }
    }
    match mode {
        LassoMode::Window => {
            p.add(Shape::line(pts, Stroke::new(1.0, t.window_stroke)));
        }
        LassoMode::Crossing | LassoMode::Fence => {
            p.extend(Shape::dashed_line(&pts, Stroke::new(1.0, t.crossing_stroke), 5.0, 3.0));
        }
    }
    let label = match mode {
        LassoMode::Window => crate::tl!("Window Lasso"),
        LassoMode::Crossing => crate::tl!("Crossing Lasso"),
        LassoMode::Fence => crate::tl!("Fence Lasso"),
    };
    let hint = format!("{label}  ·  {}", crate::tl!("Press Space to cycle options"));
    super::tooltip(p, cursor + vec2(16.0, 16.0), &hint);
}

/// Fill the (possibly concave or self-crossing) polygon `pts`, even-odd.
fn fill_polygon(p: &egui::Painter, pts: &[Pos2], color: Color32) {
    let ring: Vec<Vec2> = pts.iter().map(|q| Vec2::new(f64::from(q.x), f64::from(q.y))).collect();
    let tris = cadaddy_render::triangulate_evenodd(&[ring]);
    let mut mesh = egui::Mesh::default();
    for tri in tris.as_chunks::<3>().0 {
        let Ok(i) = u32::try_from(mesh.vertices.len()) else { break };
        for v in tri {
            mesh.colored_vertex(egui::pos2(v.x as f32, v.y as f32), color);
        }
        mesh.add_triangle(i, i + 1, i + 2);
    }
    if !mesh.is_empty() {
        p.add(Shape::mesh(mesh));
    }
}

/// Selection cycling in progress at one spot.
#[derive(Clone, Debug, Default)]
pub(crate) struct Cycle {
    /// Where (screen) the objects were found; moving the cursor out of the pick box there ends it.
    anchor: Pos2,
    /// The objects under the pick there, nearest first.
    cands: Vec<Handle>,
    /// The highlighted one.
    index: usize,
    /// The candidate list is open (SELECTIONCYCLING 2); the row under the pointer.
    list: bool,
    list_hover: Option<Handle>,
    /// The list opened this frame (its own click isn't a click elsewhere).
    fresh: bool,
}

/// Whether selection cycling is on (SELECTIONCYCLING 1 or 2) and objects are being selected.
fn cycling(app: &CadApp) -> bool {
    app.session.settings.selectioncycling > 0 && app.session.selecting_objects()
}

/// The pick box radius in pixels.
fn pick_px(app: &CadApp) -> f32 {
    (app.session.settings.pickbox.max(1.0) * 1.5) as f32
}

/// Keys the drawing area takes before the command line sees them: Space while a lasso is dragged
/// (the next mode), Shift+Space over the drawing (the next object under the cursor), Escape while
/// the candidate list is open (closes it).
pub(crate) fn keys(app: &mut CadApp, ctx: &egui::Context, hover: Option<Pos2>, world: Option<Vec2>) {
    let (spaces, shift_spaces, escape) = ctx.input(|i| {
        let mut n = (0, 0, false);
        for e in &i.events {
            if let egui::Event::Key { key, pressed: true, modifiers, .. } = e {
                match key {
                    egui::Key::Space if modifiers.shift => n.1 += 1,
                    egui::Key::Space => n.0 += 1,
                    egui::Key::Escape => n.2 = true,
                    _ => {}
                }
            }
        }
        n
    });
    let lasso = app.canvas.drag_window && app.canvas.lasso.is_some();
    let list = app.canvas.cycle.as_ref().is_some_and(|c| c.list);
    let take_space = if lasso {
        if let Some(l) = app.canvas.lasso.as_mut() {
            for _ in 0..spaces + shift_spaces {
                l.cycle_mode();
            }
        }
        spaces + shift_spaces > 0
    } else if shift_spaces > 0 && app.cmd.buffer.is_empty() && cycling(app) {
        if let (Some(h), Some(w)) = (hover, world) {
            for _ in 0..shift_spaces {
                step(app, h, w);
            }
        }
        true
    } else {
        false
    };
    let take_escape = escape && list;
    if take_escape {
        app.canvas.cycle = None;
    }
    if take_space || take_escape {
        ctx.request_repaint();
        ctx.input_mut(|i| {
            i.events.retain(|e| match e {
                egui::Event::Text(t) => !(take_space && t == " "),
                egui::Event::Key { key: egui::Key::Space, .. } => !take_space,
                egui::Event::Key { key: egui::Key::Escape, .. } => !take_escape,
                _ => true,
            });
        });
    }
}

/// Shift+Space at `hover`: highlight the next object under the cursor, starting over where the
/// cursor has moved away.
fn step(app: &mut CadApp, hover: Pos2, world: Vec2) {
    let px = pick_px(app);
    if let Some(c) = app.canvas.cycle.as_mut().filter(|c| !c.list && c.anchor.distance(hover) <= px && !c.cands.is_empty()) {
        c.index = (c.index + 1) % c.cands.len();
        return;
    }
    let cands = app.session.pick_candidates(world).unwrap_or_default();
    app.canvas.cycle = (!cands.is_empty()).then(|| Cycle { anchor: hover, index: 1 % cands.len(), cands, ..Cycle::default() });
}

/// The highlight under the cursor while cycling: the stepped-to object, or the list row under the
/// pointer. Moving out of the pick box (with no list open) ends the cycling.
pub(crate) fn hover(app: &mut CadApp, hover: Option<Pos2>) {
    let px = pick_px(app);
    let Some(c) = app.canvas.cycle.as_ref() else { return };
    if c.list {
        if let Some(h) = c.list_hover {
            app.canvas.hover = Some(h);
        }
        return;
    }
    if !cycling(app) || hover.is_none_or(|h| c.anchor.distance(h) > px) {
        app.canvas.cycle = None;
        return;
    }
    app.canvas.hover = c.cands.get(c.index).copied();
}

/// A click on the drawing area at `hover` while objects are being selected. Returns whether
/// selection cycling used it: the stepped-to object is picked, an open list closes, or (with
/// SELECTIONCYCLING 2) a click on overlapping objects opens the list of them instead of picking.
/// `on_grip`: the click is on a grip (grips win).
pub(crate) fn click(app: &mut CadApp, hover: Pos2, world: Vec2, shift: bool, on_grip: bool) -> bool {
    if let Some(c) = app.canvas.cycle.take() {
        if c.list {
            return true;
        }
        if c.anchor.distance(hover) <= pick_px(app)
            && let Some(h) = c.cands.get(c.index).copied()
        {
            if let Err(e) = app.session.pick_handle(h, shift) {
                app.session.echo(e.to_string());
            }
            return true;
        }
    }
    if on_grip || app.session.settings.selectioncycling != 2 || !cycling(app) || app.session.pending_window.is_some() {
        return false;
    }
    let cands = app.session.pick_candidates(world).unwrap_or_default();
    if cands.len() < 2 {
        return false;
    }
    app.canvas.cycle = Some(Cycle { anchor: hover, cands, list: true, fresh: true, ..Cycle::default() });
    true
}

/// The candidate list (SELECTIONCYCLING 2) beside where it was opened: each object's type and
/// layer; the row under the pointer is highlighted on the drawing, a click picks it, a click
/// elsewhere closes the list.
pub(crate) fn list(app: &mut CadApp, ctx: &egui::Context) {
    let Some(c) = app.canvas.cycle.as_ref().filter(|c| c.list) else { return };
    let rows: Vec<(Handle, String)> = match app.session.state() {
        Ok(st) => c
            .cands
            .iter()
            .filter_map(|h| {
                let e = st.doc.entity(*h)?;
                Some((*h, format!("{}  ·  {}", crate::i18n::t(e.kind.type_name()), e.common.layer)))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    if rows.is_empty() {
        app.canvas.cycle = None;
        return;
    }
    let at = c.anchor + vec2(14.0, 14.0);
    let mut hovered = None;
    let mut chosen = None;
    let area = egui::Area::new(egui::Id::new("selection-cycling-list")).order(egui::Order::Foreground).fixed_pos(at).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.label(egui::RichText::new(crate::tl!("Selection")).strong());
            ui.separator();
            for (h, text) in &rows {
                let r = ui.selectable_label(false, text.as_str());
                if r.hovered() {
                    hovered = Some(*h);
                }
                if r.clicked() {
                    chosen = Some(*h);
                }
            }
        });
    });
    let fresh = app.canvas.cycle.as_mut().is_some_and(|c| std::mem::take(&mut c.fresh));
    let clicked_elsewhere = !fresh && ctx.input(|i| i.pointer.any_pressed()) && !area.response.contains_pointer();
    if let Some(h) = chosen {
        app.canvas.cycle = None;
        let shift = ctx.input(|i| i.modifiers.shift);
        if let Err(e) = app.session.pick_handle(h, shift) {
            app.session.echo(e.to_string());
        }
    } else if clicked_elsewhere {
        app.canvas.cycle = None;
    } else if let Some(c) = app.canvas.cycle.as_mut() {
        c.list_hover = hovered;
        if let Some(h) = hovered {
            app.canvas.hover = Some(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag(pts: &[(f32, f32)]) -> Lasso {
        let mut it = pts.iter().map(|(x, y)| egui::pos2(*x, *y));
        let mut l = Lasso::new(it.next().unwrap_or_default());
        for p in it {
            l.track(p);
        }
        l
    }

    #[test]
    fn a_straight_drag_stays_a_rectangle_and_a_loop_is_a_lasso() {
        let straight = drag(&[(0.0, 0.0), (20.0, 15.0), (40.0, 31.0), (60.0, 44.0), (80.0, 60.0)]);
        assert!(!straight.is_lasso());
        let lasso = drag(&[(0.0, 0.0), (80.0, 0.0), (80.0, 60.0), (40.0, 80.0), (0.0, 60.0)]);
        assert!(lasso.is_lasso());
        assert_eq!(lasso.mode(), LassoMode::Window, "it headed right");
        let left = drag(&[(100.0, 0.0), (20.0, 0.0), (20.0, 60.0), (60.0, 80.0), (100.0, 60.0)]);
        assert_eq!(left.mode(), LassoMode::Crossing, "it headed left");
        // A tiny loop is a click-and-wobble, not a lasso.
        assert!(!drag(&[(0.0, 0.0), (6.0, 0.0), (6.0, 6.0), (0.0, 6.0)]).is_lasso());
    }

    #[test]
    fn space_makes_any_drag_a_lasso_and_steps_its_mode() {
        let mut l = drag(&[(0.0, 0.0), (30.0, 2.0)]);
        assert!(!l.is_lasso());
        l.cycle_mode();
        assert!(l.is_lasso());
        assert_eq!(l.mode(), LassoMode::Crossing);
        l.cycle_mode();
        assert_eq!(l.mode(), LassoMode::Fence);
        l.cycle_mode();
        assert_eq!(l.mode(), LassoMode::Window);
    }

    #[test]
    fn the_path_skips_jitter_and_is_capped() {
        let mut l = Lasso::new(egui::pos2(0.0, 0.0));
        l.track(egui::pos2(0.5, 0.5));
        assert_eq!(l.pts.len(), 1, "under a pixel or two");
        l.track(egui::pos2(f32::NAN, 3.0));
        assert_eq!(l.pts.len(), 1);
        for i in 0..(MAX_POINTS as u32 + 100) {
            l.track(egui::pos2((i % 2) as f32 * 10.0, i as f32 * 3.0));
        }
        assert_eq!(l.pts.len(), MAX_POINTS);
    }
}
