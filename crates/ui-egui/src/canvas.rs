//! The drawing area: grid, entities, selection highlighting, grips, rubber bands, object-snap
//! markers, crosshair cursor, ViewCube, UCS icon and mouse handling.

use cadcraft_color::Rgb;
use cadcraft_doc::Handle;
use cadcraft_engine::Input;
use cadcraft_engine::snap::{self, SnapHit};
use cadcraft_geom::{Bounds2, Vec2};
use cadcraft_render::{DisplayList, Kind};
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};

use crate::CadApp;
use crate::theme::Tokens;

/// World ↔ screen transform for the canvas rect.
#[derive(Clone, Copy, Debug)]
pub struct Xf {
    pub rect: Rect,
    pub center: Vec2,
    /// Pixels (points) per drawing unit.
    pub scale: f64,
}

impl Xf {
    pub fn to_screen(&self, p: Vec2) -> Pos2 {
        let c = self.rect.center();
        pos2(c.x + ((p.x - self.center.x) * self.scale) as f32, c.y - ((p.y - self.center.y) * self.scale) as f32)
    }
    pub fn to_world(&self, p: Pos2) -> Vec2 {
        let c = self.rect.center();
        Vec2::new(self.center.x + f64::from(p.x - c.x) / self.scale, self.center.y - f64::from(p.y - c.y) / self.scale)
    }
    pub fn world_bounds(&self) -> Bounds2 {
        Bounds2::new(self.to_world(self.rect.left_bottom()), self.to_world(self.rect.right_top()))
    }
}

#[derive(Default)]
pub struct CanvasState {
    pub list: Option<DisplayList>,
    key: (u64, u64, i32, i32, bool, usize),
    /// The area `list` was built for (hatch patterns exist only inside it); a view leaving it
    /// rebuilds the list.
    window: Option<Bounds2>,
    /// Bumped whenever `list` is rebuilt (identifies it for [`PrimIndex`]).
    list_gen: u64,
    prim_index: PrimIndex,
    /// Set when the app runs on wgpu: entities are drawn by [`crate::gpu`]; otherwise on the CPU.
    pub gpu: Option<crate::gpu::GpuTarget>,
    /// The mesh last handed to the GPU (see [`crate::gpu::CanvasCallback::key`]).
    mesh_key: Option<u64>,
    mesh_slot: crate::gpu::MeshSlot,
    mesh_origin: Vec2,
    /// The dragged objects of a MOVE/COPY/ROTATE/SCALE preview, uploaded once per drag (see
    /// [`draw_drag_gpu`]).
    drag_key: Option<u64>,
    drag_slot: crate::gpu::MeshSlot,
    drag_origin: Vec2,
    /// The selection highlight on the GPU (see [`draw_highlight_gpu`]).
    hl_key: Option<u64>,
    hl_slot: crate::gpu::MeshSlot,
    hl_origin: Vec2,
    /// Selected infinite lines (clipped and drawn on the CPU each frame).
    hl_infinite: Vec<usize>,
    /// The highlight geometry of the active viewport's model space, with the key it was built for
    /// (see [`viewport_highlight_tolerance`]).
    vp_hl: Option<(u64, DisplayList)>,
    /// Indices of infinite-line primitives in `list` (drawn on the CPU in GPU mode).
    infinite: Vec<usize>,
    /// Milliseconds spent converting the display list to vertex data (last rebuild).
    pub mesh_ms: f64,
    pub rect: Option<Rect>,
    pub xf: Option<Xf>,
    pub hover: Option<Handle>,
    pub snap: Option<SnapHit>,
    /// Effective cursor point after snaps/ortho/polar.
    pub cursor: Option<Vec2>,
    /// The polar or object snap tracking path(s) the cursor is on.
    pub track: Option<snap::tracking::Track>,
    pan_last: Option<Pos2>,
    /// True when the latest scroll came from a trackpad (or other precise-scrolling device):
    /// scrolling then pans and pinch zooms. A notched mouse wheel clears it so the wheel zooms.
    scroll_pans: bool,
    /// Hot grips (object, grip index): the ones Shift+clicked while idle, then the ones the
    /// running GRIP edit works on (the engine's grip modes take them from there).
    pub hot_grips: Vec<(Handle, usize)>,
    /// Grip dimensional input: the value fields of the grip being stretched.
    pub grip_dyn: crate::grip_dyn::GripDyn,
    /// A GRIP edit was running last frame (its grips go cold when it ends).
    grip_editing: bool,
    /// The grip whose menu (Stretch, Add Vertex, Lengthen…) a right-click opened.
    grip_menu: Option<(Handle, usize)>,
    /// Zoom to extents once the canvas size is known (after opening a drawing).
    pub zoom_pending: bool,
    pub build_ms: f64,
    pub draw_ms: f64,
    hover_at: Option<Pos2>,
    /// Constraint glyphs for the parametric overlay (cached per drawing revision).
    pub param: crate::parametric::Cache,
    /// A selection window opened by a press-and-drag, to close where the button is let go.
    drag_window: bool,
    /// The right-click that opened the shortcut menu held Shift or Ctrl: it is the object snap
    /// menu ([`crate::context_menu`]).
    pub snap_menu: bool,
    /// Tab was pressed over a snap marker: show the next candidate ([`SnapCycle`]).
    pub snap_tab: bool,
    pub snap_cycle: Option<SnapCycle>,
}

/// Tab cycling through the candidate snaps under the cursor: where it started and which
/// candidate shows. Moving the cursor out of the aperture ends it.
#[derive(Clone, Copy, Debug)]
pub struct SnapCycle {
    anchor: Vec2,
    index: usize,
}

/// Primitives of `list` sorted by entity handle, built on the first highlight after each list
/// rebuild. Highlighting the hovered entity used to scan every primitive on every frame: ~2.5 ms
/// per frame on a 270k-entity drawing, whether or not anything changed.
#[derive(Default)]
struct PrimIndex {
    list_gen: Option<u64>,
    by_handle: Vec<(Handle, usize)>,
}

impl PrimIndex {
    /// Indices of the primitives drawn for `handles` (in list order).
    fn prims(&mut self, list_gen: u64, list: &DisplayList, handles: &[Handle]) -> Vec<usize> {
        if handles.is_empty() {
            return Vec::new();
        }
        if self.list_gen != Some(list_gen) {
            self.by_handle = list.prims.iter().enumerate().map(|(i, p)| (p.handle, i)).collect();
            self.by_handle.sort_unstable();
            self.list_gen = Some(list_gen);
        }
        let mut out = Vec::new();
        for h in handles {
            let from = self.by_handle.partition_point(|(k, _)| k < h);
            out.extend(self.by_handle.get(from..).unwrap_or(&[]).iter().take_while(|(k, _)| k == h).map(|(_, i)| *i));
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// Indices of the primitives in `list` drawn for `handles` (a scan; for small lists).
fn prims_of(list: &DisplayList, handles: &[Handle]) -> Vec<usize> {
    if handles.is_empty() {
        return Vec::new();
    }
    let set: std::collections::HashSet<Handle> = handles.iter().copied().collect();
    list.prims.iter().enumerate().filter(|(_, pr)| set.contains(&pr.handle)).map(|(i, _)| i).collect()
}

fn color32(c: Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

/// Rebuild the display list when needed. `px`: drawing units per pixel in the current view;
/// `views`: every area on screen that shows the list (the current view, other model tiles) with
/// its drawing units per pixel.
fn ensure_list(app: &mut CadApp, px: f64, views: &[(Bounds2, f64)]) {
    let Ok(st) = app.session.state() else { return };
    // Rebuild when the drawing changes, the zoom moves by more than 2x (tessellation band) or a
    // view leaves the area the list was built for.
    let band = |px: f64| px.max(1e-300).log2().floor() as i32;
    // Hatch level of detail follows the finest view showing the list.
    let lod_band = views.iter().map(|v| band(v.1)).fold(band(px), i32::min);
    let key = (st.uid, st.revision, band(px), lod_band, app.session.settings.lwdisplay, std::sync::Arc::as_ptr(&st.doc) as usize);
    let visible = views.iter().fold(Bounds2::EMPTY, |acc, v| acc.union(&v.0));
    let inside = app.canvas.window.is_none_or(|w| visible.is_empty() || w.contains_box(&visible));
    if app.canvas.list.is_some() && app.canvas.key == key && inside {
        return;
    }
    let t0 = crate::now_ms();
    let opts = cadcraft_render::Options {
        tolerance: 2f64.powi(band(px)) * 0.5,
        // Two pixels at the band's finest zoom: the list serves every zoom in the band, so its
        // level of detail must hold at the finest one.
        min_dash: 2f64.powi(lod_band) * 2.0,
        text: true,
        fill: true,
        lineweights: app.session.settings.lwdisplay,
        view_height: st.paper_view().height,
        plot_style_table: None,
        view: build_window(views),
    };
    let space = st.space.clone();
    app.canvas.list = Some(cadcraft_render::build(&st.doc, &space, &opts));
    app.canvas.key = key;
    app.canvas.window = opts.view;
    app.canvas.list_gen = app.canvas.list_gen.wrapping_add(1);
    app.canvas.build_ms = crate::now_ms() - t0;
}

/// Tessellation tolerance for the highlights inside the active viewport: `scale` (pixels per
/// drawing unit) snapped to its 2x zoom band, as in [`ensure_list`], so zooming and panning
/// inside the band doesn't rebuild the geometry.
fn viewport_highlight_tolerance(scale: f64) -> f64 {
    2f64.powi((1.0 / scale.max(1e-300)).log2().floor() as i32) * 0.5
}

/// The area a display list is built for: each view with half its larger side of margin all
/// round (panning by up to that much doesn't rebuild), joined. `None` (everything) when a view
/// isn't usable.
fn build_window(views: &[(Bounds2, f64)]) -> Option<Bounds2> {
    let mut w = Bounds2::EMPTY;
    for (v, _) in views {
        let side = v.width().max(v.height());
        if v.is_empty() || !side.is_finite() {
            return None;
        }
        w = w.union(&v.expand(side * 0.5));
    }
    (!w.is_empty()).then_some(w)
}

/// Grid spacing adapted to the zoom: multiply the unit until lines are at least `min_px` apart.
fn adaptive(unit: f64, scale: f64, min_px: f64) -> f64 {
    let mut u = unit.max(1e-12);
    let mut guard = 0;
    while u * scale < min_px && guard < 40 {
        u *= 5.0;
        guard += 1;
    }
    u
}

fn draw_grid(app: &CadApp, p: &egui::Painter, xf: &Xf) {
    let t = Tokens::get();
    let s = app.session.drafting_settings();
    let wb = xf.world_bounds();
    if s.gridmode && s.isodraft {
        draw_iso_grid(app, p, xf);
    } else if s.gridmode {
        let major_n = f64::from(s.gridmajor.max(1));
        let unit = if s.gridunit.x == 0.0 { s.snapunit.x } else { s.gridunit.x };
        let minor = adaptive(unit, xf.scale, 8.0);
        let major = minor * major_n;
        let mut lines = Vec::new();
        let x0 = (wb.min.x / minor).floor() as i64;
        let x1 = (wb.max.x / minor).ceil() as i64;
        let y0 = (wb.min.y / minor).floor() as i64;
        let y1 = (wb.max.y / minor).ceil() as i64;
        if (x1 - x0) < 2000 && (y1 - y0) < 2000 {
            for i in x0..=x1 {
                let x = i as f64 * minor;
                let is_major = (x / major).round() * major - x == 0.0 || ((x / major) - (x / major).round()).abs() < 1e-6;
                let sx = xf.to_screen(Vec2::new(x, 0.0)).x;
                lines.push(Shape::line_segment(
                    [pos2(sx, xf.rect.top()), pos2(sx, xf.rect.bottom())],
                    Stroke::new(1.0, if is_major { t.grid_major } else { t.grid_minor }),
                ));
            }
            for j in y0..=y1 {
                let y = j as f64 * minor;
                let is_major = ((y / major) - (y / major).round()).abs() < 1e-6;
                let sy = xf.to_screen(Vec2::new(0.0, y)).y;
                lines.push(Shape::line_segment(
                    [pos2(xf.rect.left(), sy), pos2(xf.rect.right(), sy)],
                    Stroke::new(1.0, if is_major { t.grid_major } else { t.grid_minor }),
                ));
            }
        }
        p.extend(lines);
    }
    // Axes through the origin.
    let o = xf.to_screen(Vec2::ZERO);
    if xf.rect.y_range().contains(o.y) {
        p.line_segment([pos2(xf.rect.left(), o.y), pos2(xf.rect.right(), o.y)], Stroke::new(1.0, t.axis_x));
    }
    if xf.rect.x_range().contains(o.x) {
        p.line_segment([pos2(o.x, xf.rect.top()), pos2(o.x, xf.rect.bottom())], Stroke::new(1.0, t.axis_y));
    }
}

/// The isometric grid (SNAPSTYL 1): lines at 30°, 90° and 150° through the snap lattice, GRIDUNIT
/// Y apart along the axes, coarsened while they would crowd closer than 8 pixels.
fn draw_iso_grid(app: &CadApp, p: &egui::Painter, xf: &Xf) {
    use cadcraft_engine::snap::iso;
    let t = Tokens::get();
    let s = app.session.drafting_settings();
    let mut d = s.gridunit.y.max(1e-12);
    for _ in 0..40 {
        if d * 0.866 * xf.scale >= 8.0 {
            break;
        }
        d *= 5.0;
    }
    let (origin, angle) = app.session.doc().map(cadcraft_engine::snap::grid_frame).unwrap_or((Vec2::ZERO, 0.0));
    let lines = iso::grid_lines(xf.world_bounds(), d, origin, angle, s.gridmajor);
    p.extend(
        lines.into_iter().map(|l| {
            Shape::line_segment([xf.to_screen(l.a), xf.to_screen(l.b)], Stroke::new(1.0, if l.major { t.grid_major } else { t.grid_minor }))
        }),
    );
}

fn clip_infinite(xf: &Xf, base: Vec2, dir: Vec2, ray: bool) -> Option<[Pos2; 2]> {
    let wb = xf.world_bounds();
    let far = (wb.width() + wb.height()) * 2.0 + base.dist(wb.center());
    let a = if ray { base } else { base - dir * far };
    let b = base + dir * far;
    Some([xf.to_screen(a), xf.to_screen(b)])
}

/// Draw a display list. `highlight`: draw these handles brighter (hover) or dashed (selected).
fn draw_list(p: &egui::Painter, xf: &Xf, list: &DisplayList, bg: Rgb, lwdisplay: bool) {
    let vis = xf.world_bounds();
    let mut shapes: Vec<Shape> = Vec::with_capacity(list.prims.len());
    let px = 1.0 / xf.scale;
    for prim in &list.prims {
        let col = color32(prim.display_rgb(bg));
        let pts = list.points(prim);
        match prim.kind {
            Kind::Polyline => {
                let bb = Bounds2::from_points(pts.iter().copied());
                if !bb.intersects(&vis) {
                    continue;
                }
                let w = if lwdisplay && prim.lw > 0.0 { (prim.lw * 3.78).clamp(1.0, 12.0) } else { 1.0 };
                // Skip sub-pixel objects except as a dot.
                if bb.width() < px && bb.height() < px {
                    shapes.push(Shape::rect_filled(Rect::from_center_size(xf.to_screen(bb.center()), vec2(1.0, 1.0)), 0.0, col));
                    continue;
                }
                let screen: Vec<Pos2> = pts.iter().map(|q| xf.to_screen(*q)).collect();
                if screen.len() == 2 {
                    if let [a, b] = screen.as_slice() {
                        shapes.push(Shape::line_segment([*a, *b], Stroke::new(w, col)));
                    }
                } else {
                    shapes.push(Shape::line(screen, Stroke::new(w, col)));
                }
            }
            Kind::Tris | Kind::Mask => {
                // A wipeout's mask hides what was drawn before it with the background colour.
                let col = if prim.kind == Kind::Mask { color32(bg) } else { col };
                let mut mesh = egui::Mesh::default();
                for tri in pts.chunks(3) {
                    if let [a, b, c] = tri {
                        let i = mesh.vertices.len() as u32;
                        for q in [a, b, c] {
                            mesh.colored_vertex(xf.to_screen(*q), col);
                        }
                        mesh.add_triangle(i, i + 1, i + 2);
                    }
                }
                if !mesh.vertices.is_empty() {
                    shapes.push(Shape::mesh(mesh));
                }
            }
            Kind::Point => {
                if let Some(q) = pts.first()
                    && vis.contains(*q)
                {
                    shapes.push(Shape::rect_filled(Rect::from_center_size(xf.to_screen(*q), vec2(1.5, 1.5)), 0.0, col));
                }
            }
            Kind::Infinite { ray } => {
                if let (Some(b), Some(d)) = (pts.first(), pts.get(1))
                    && let Some(seg) = clip_infinite(xf, *b, *d, ray)
                {
                    shapes.push(Shape::line_segment(seg, Stroke::new(1.0, col)));
                }
            }
        }
    }
    p.extend(shapes);
}

/// Draw the selection highlight (dashed, `width` points) through the GPU: its vertex data is
/// rebuilt only when the display list, the selection or the vertex origin changes. Dashing every
/// selected line and tessellating it as egui shapes each frame made large selections sluggish.
#[allow(clippy::too_many_arguments)]
fn draw_highlight_gpu(
    p: &egui::Painter,
    xf: &Xf,
    list: &DisplayList,
    list_gen: u64,
    prim_index: &mut PrimIndex,
    sel: &[Handle],
    color: Color32,
    width: f32,
    hl: (&mut Option<u64>, &crate::gpu::MeshSlot, &mut Vec2, &mut Vec<usize>),
) {
    let (hl_key, hl_slot, hl_origin, hl_infinite) = hl;
    let origin = crate::gpu::choose_origin(&list.bounds, xf.center, f64::from(xf.rect.height()) / xf.scale.max(1e-300));
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (list_gen, sel, origin.x.to_bits(), origin.y.to_bits(), color.to_array(), width.to_bits()).hash(&mut h);
        h.finish()
    };
    if *hl_key != Some(key) {
        let prims = prim_index.prims(list_gen, list, sel);
        let mesh = crate::gpu::build_highlight_mesh(list, &prims, origin, color.to_array(), width);
        *hl_infinite = prims.into_iter().filter(|&i| list.prims.get(i).is_some_and(|p| matches!(p.kind, Kind::Infinite { .. }))).collect();
        if let Ok(mut slot) = hl_slot.lock() {
            *slot = Some(mesh);
        }
        *hl_key = Some(key);
        *hl_origin = origin;
    }
    let center = xf.rect.center();
    p.add(egui_wgpu::Callback::new_paint_callback(
        xf.rect,
        crate::gpu::CanvasCallback {
            key,
            slot: hl_slot.clone(),
            offset: [(hl_origin.x - xf.center.x) as f32, (hl_origin.y - xf.center.y) as f32],
            scale: xf.scale as f32,
            center: [center.x, center.y],
            layer: crate::gpu::LAYER_HIGHLIGHT,
            xform: crate::gpu::IDENTITY_XFORM,
            dash: 10.0,
        },
    ));
}

/// Draw a MOVE/COPY/ROTATE/SCALE preview the way the drawing itself is drawn: the dragged
/// objects are tessellated and uploaded once (again only when the drawing, the selection or the
/// zoom band changes), and each frame only the transform in the view uniform moves. Rebuilding
/// the transformed copies as egui shapes every frame was slow with heavy selections and could
/// overflow the GPU buffer limit.
/// `clipboard` is the session's clipboard and its serial (dragged when `drag.clipboard`, by PASTE).
fn draw_drag_gpu(
    c: &mut CanvasState,
    p: &egui::Painter,
    xf: &Xf,
    doc: &cadcraft_doc::Drawing,
    drag: &cadcraft_engine::Drag,
    clipboard: (&[cadcraft_doc::Entity], u64),
    bg: Rgb,
) {
    // Tessellate for a power-of-two zoom band so zooming mid-drag rebuilds only every 2x.
    let band = xf.scale.max(1e-300).log2().round() as i32;
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (c.key, band, bg.0, bg.1, bg.2, &drag.handles).hash(&mut h);
        if drag.clipboard {
            clipboard.1.hash(&mut h);
        }
        h.finish()
    };
    if c.drag_key != Some(key) {
        let band_scale = 2f64.powi(band);
        let opts = cadcraft_render::Options { tolerance: 0.5 / band_scale, min_dash: 2.0 / band_scale, ..Default::default() };
        let list = if drag.clipboard {
            cadcraft_render::build_entities(doc, clipboard.0.iter(), &opts)
        } else {
            cadcraft_render::build_entities(doc, drag.handles.iter().filter_map(|h| doc.entity(*h)).map(|e| e.as_ref()), &opts)
        };
        let origin = if list.bounds.is_empty() { Vec2::ZERO } else { list.bounds.center() };
        let mesh = crate::gpu::build_mesh(&list, origin, bg, false);
        if let Ok(mut slot) = c.drag_slot.lock() {
            *slot = Some(mesh);
        }
        c.drag_key = Some(key);
        c.drag_origin = origin;
    }
    // Positions are relative to the origin o: M(o + p) = L·p + (M(o) − o).
    let m = drag.matrix;
    let o = c.drag_origin;
    let t = m.apply(o) - o;
    let center = xf.rect.center();
    p.add(egui_wgpu::Callback::new_paint_callback(
        xf.rect,
        crate::gpu::CanvasCallback {
            key,
            slot: c.drag_slot.clone(),
            offset: [(o.x - xf.center.x) as f32, (o.y - xf.center.y) as f32],
            scale: xf.scale as f32,
            center: [center.x, center.y],
            layer: crate::gpu::LAYER_DRAG,
            xform: [m.a as f32, m.b as f32, m.c as f32, m.d as f32, t.x as f32, t.y as f32],
            dash: 0.0,
        },
    ));
}

/// Draw the display list through the GPU paint callback (uploading it when it changed), plus
/// the infinite lines, which are clipped to the view on the CPU every frame.
fn draw_list_gpu(c: &mut CanvasState, p: &egui::Painter, xf: &Xf, bg: Rgb, lwdisplay: bool) {
    let Some(list) = &c.list else { return };
    let origin = crate::gpu::choose_origin(&list.bounds, xf.center, f64::from(xf.rect.height()) / xf.scale.max(1e-300));
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (c.key, c.list_gen, origin.x.to_bits(), origin.y.to_bits(), bg.0, bg.1, bg.2, lwdisplay).hash(&mut h);
        h.finish()
    };
    if c.mesh_key != Some(key) {
        let t0 = crate::now_ms();
        let mesh = crate::gpu::build_mesh(list, origin, bg, lwdisplay);
        if let Ok(mut slot) = c.mesh_slot.lock() {
            *slot = Some(mesh);
        }
        c.infinite = list.prims.iter().enumerate().filter(|(_, pr)| matches!(pr.kind, Kind::Infinite { .. })).map(|(i, _)| i).collect();
        c.mesh_key = Some(key);
        c.mesh_origin = origin;
        c.mesh_ms = crate::now_ms() - t0;
    }
    let center = xf.rect.center();
    p.add(egui_wgpu::Callback::new_paint_callback(
        xf.rect,
        crate::gpu::CanvasCallback {
            key,
            slot: c.mesh_slot.clone(),
            offset: [(c.mesh_origin.x - xf.center.x) as f32, (c.mesh_origin.y - xf.center.y) as f32],
            scale: xf.scale as f32,
            center: [center.x, center.y],
            layer: crate::gpu::LAYER_DRAWING,
            xform: crate::gpu::IDENTITY_XFORM,
            dash: 0.0,
        },
    ));
    let mut shapes = Vec::new();
    for prim in c.infinite.iter().filter_map(|i| list.prims.get(*i)) {
        let pts = list.points(prim);
        if let (Kind::Infinite { ray }, Some(b), Some(d)) = (prim.kind, pts.first(), pts.get(1))
            && let Some(seg) = clip_infinite(xf, *b, *d, ray)
        {
            shapes.push(Shape::line_segment(seg, Stroke::new(1.0, color32(prim.display_rgb(bg)))));
        }
    }
    p.extend(shapes);
}

/// Draw selected/hovered entities as highlight overlays: `prims` are indices into `list` (from
/// [`PrimIndex::prims`] or [`prims_of`]).
fn draw_highlight(p: &egui::Painter, xf: &Xf, list: &DisplayList, prims: &[usize], color: Color32, width: f32, dashed: bool) {
    let mut shapes = Vec::new();
    for prim in prims.iter().filter_map(|&i| list.prims.get(i)) {
        let pts = list.points(prim);
        match prim.kind {
            Kind::Polyline => {
                let screen: Vec<Pos2> = pts.iter().map(|q| xf.to_screen(*q)).collect();
                if dashed {
                    shapes.extend(Shape::dashed_line(&screen, Stroke::new(width, color), 6.0, 4.0));
                } else {
                    shapes.push(Shape::line(screen, Stroke::new(width, color)));
                }
            }
            Kind::Infinite { ray } => {
                if let (Some(b), Some(d)) = (pts.first(), pts.get(1))
                    && let Some(seg) = clip_infinite(xf, *b, *d, ray)
                {
                    if dashed {
                        shapes.extend(Shape::dashed_line(&seg, Stroke::new(width, color), 6.0, 4.0));
                    } else {
                        shapes.push(Shape::line_segment(seg, Stroke::new(width, color)));
                    }
                }
            }
            Kind::Point => {
                if let Some(q) = pts.first() {
                    shapes.push(Shape::circle_stroke(xf.to_screen(*q), 3.0, Stroke::new(1.0, color)));
                }
            }
            Kind::Tris | Kind::Mask => {
                // Outline triangles' bounding region lightly.
                let bb = Bounds2::from_points(pts.iter().copied());
                let r = Rect::from_two_pos(xf.to_screen(bb.min), xf.to_screen(bb.max));
                shapes.push(Shape::rect_stroke(r, 0.0, Stroke::new(1.0, color), egui::StrokeKind::Middle));
            }
        }
    }
    p.extend(shapes);
}

fn draw_snap_marker(p: &egui::Painter, at: Pos2, hit: &SnapHit) {
    let t = Tokens::get();
    let s = crate::theme::snap_marker_half();
    let st = Stroke::new(2.0, t.snap);
    use snap::mode;
    match hit.mode {
        mode::END => {
            p.rect_stroke(Rect::from_center_size(at, vec2(2.0 * s, 2.0 * s)), 0.0, st, egui::StrokeKind::Middle);
        }
        mode::MID => {
            p.add(Shape::closed_line(vec![at + vec2(0.0, -s), at + vec2(s, s * 0.8), at + vec2(-s, s * 0.8)], st));
        }
        mode::CEN | mode::GCEN => {
            p.circle_stroke(at, s, st);
        }
        mode::QUA => {
            p.add(Shape::closed_line(vec![at + vec2(0.0, -s), at + vec2(s, 0.0), at + vec2(0.0, s), at + vec2(-s, 0.0)], st));
        }
        mode::INT | mode::APP => {
            p.line_segment([at + vec2(-s, -s), at + vec2(s, s)], st);
            p.line_segment([at + vec2(s, -s), at + vec2(-s, s)], st);
            if hit.mode == mode::APP {
                p.rect_stroke(Rect::from_center_size(at, vec2(2.0 * s, 2.0 * s)), 0.0, st, egui::StrokeKind::Middle);
            }
        }
        mode::PER => {
            p.line(vec![at + vec2(-s, -s), at + vec2(-s, s), at + vec2(s, s)], st);
            p.line(vec![at + vec2(-s, 0.0), at + vec2(0.0, 0.0), at + vec2(0.0, s)], st);
        }
        mode::TAN => {
            p.circle_stroke(at, s * 0.8, st);
            p.line_segment([at + vec2(-s, -s), at + vec2(s, -s)], st);
        }
        mode::NOD => {
            p.circle_stroke(at, s, st);
            p.line_segment([at + vec2(-s, -s), at + vec2(s, s)], st);
            p.line_segment([at + vec2(s, -s), at + vec2(-s, s)], st);
        }
        mode::INS => {
            p.line(
                vec![
                    at + vec2(-s, -s),
                    at + vec2(0.0, -s),
                    at + vec2(0.0, 0.0),
                    at + vec2(s, 0.0),
                    at + vec2(s, s),
                    at + vec2(-s, s),
                    at + vec2(-s, -s),
                ],
                st,
            );
        }
        _ => {
            p.line(vec![at + vec2(-s, -s), at + vec2(s, -s), at + vec2(-s, s), at + vec2(s, s), at + vec2(-s, -s)], st);
        }
    }
    p.text(at + vec2(12.0, 12.0), egui::Align2::LEFT_TOP, crate::i18n::t(hit.name), crate::theme::small(), Color32::BLACK);
    let galley = p.layout_no_wrap(crate::i18n::t(hit.name).to_string(), crate::theme::small(), Color32::BLACK);
    let r = Rect::from_min_size(at + vec2(10.0, 10.0), galley.size() + vec2(6.0, 2.0));
    p.rect_filled(r, 2.0, Color32::from_rgb(0xff, 0xff, 0xe1));
    p.galley(r.min + vec2(3.0, 1.0), galley, Color32::BLACK);
}

/// Acquired tracking points (small `+`, `//` for a Parallel line), the dotted paths the cursor
/// is on and the tooltip naming them ("Endpoint: 5.0000 < 90°").
fn draw_tracking(p: &egui::Painter, xf: &Xf, acquired: &[snap::tracking::Acquired], track: Option<&snap::tracking::Track>, far: f64) {
    use snap::tracking::{Acquired, PathShape};
    let t = Tokens::get();
    let st = Stroke::new(1.0, t.snap);
    for a in acquired {
        let c = xf.to_screen(a.at());
        if let Acquired::Parallel { .. } = a {
            p.line_segment([c + vec2(-5.0, 3.0), c + vec2(-1.0, -5.0)], st);
            p.line_segment([c + vec2(1.0, 3.0), c + vec2(5.0, -5.0)], st);
        } else {
            p.line_segment([c + vec2(-4.0, 0.0), c + vec2(4.0, 0.0)], st);
            p.line_segment([c + vec2(0.0, -4.0), c + vec2(0.0, 4.0)], st);
        }
    }
    let Some(tr) = track else { return };
    let mut tips = Vec::new();
    for path in &tr.paths {
        // Around the cursor only (a path's origin may be far off screen).
        let pts: Vec<Pos2> = match path.shape {
            PathShape::Ray { origin, dir } => {
                let back = (tr.point - origin).dot(dir).clamp(0.0, far);
                vec![xf.to_screen(tr.point - dir * back), xf.to_screen(tr.point + dir * far)]
            }
            PathShape::Line { dir, .. } => vec![xf.to_screen(tr.point - dir * far), xf.to_screen(tr.point + dir * far)],
            PathShape::Circle { center, radius, .. } => {
                let (a0, span) = (center.angle_to(tr.point), (far / radius).min(std::f64::consts::PI));
                (0..=96).map(|i| xf.to_screen(Vec2::polar(center, radius, a0 - span + span * f64::from(i) / 48.0))).collect()
            }
            PathShape::Ellipse { center, major, ratio, .. } => {
                let e = cadcraft_geom::Ellipse::full(center, major, ratio);
                let (t0, span) = (e.param_of(tr.point), (far / (major.len() * ratio.min(1.0))).min(std::f64::consts::PI));
                (0..=96).map(|i| xf.to_screen(e.at_param(t0 - span + span * f64::from(i) / 48.0))).collect()
            }
        };
        p.extend(Shape::dotted_line(&pts, t.snap, 6.0, 1.0));
        let (distance, angle) = (
            cadcraft_engine::units::format_distance(path.from.dist(tr.point), 2, 4),
            (path.from.angle_to(tr.point).to_degrees().round() as i64).rem_euclid(360),
        );
        tips.push(if path.name == "Polar" {
            crate::tf!("Polar: {distance} < {angle}°", distance = distance, angle = angle)
        } else {
            crate::tf!("{name}: {distance} < {angle}°", name = crate::i18n::t(path.name), distance = distance, angle = angle)
        });
    }
    if let Some(o) = tr.object {
        tips.push(crate::i18n::t(o).to_string());
    }
    let at = xf.to_screen(tr.point);
    if tr.paths.len() > 1 || tr.object.is_some() {
        p.line_segment([at + vec2(-4.0, -4.0), at + vec2(4.0, 4.0)], st);
        p.line_segment([at + vec2(4.0, -4.0), at + vec2(-4.0, 4.0)], st);
    }
    tooltip(p, at + vec2(16.0, 18.0), &tips.join(", "));
}

/// The rubber-band and snap-adjusted cursor point for the current prompt (the engine's
/// `Session::snap_cursor`: object snaps, acquiring tracking points, grid, tracking paths, ortho).
fn effective_point(app: &mut CadApp, raw: Vec2, xf: &Xf, ctx: &egui::Context) -> Vec2 {
    let prompt = app.session.current_prompt();
    let wants_point = prompt.as_ref().is_some_and(cadcraft_engine::pointmod::accepts);
    let base = prompt.as_ref().and_then(|p| p.base);
    let deferred = prompt.as_ref().is_some_and(|p| p.deferred);
    app.canvas.snap = None;
    app.canvas.track = None;
    if !wants_point || app.session.pending_window.is_some() {
        if !wants_point {
            app.session.tracking.clear();
        }
        return raw;
    }
    let (now, shift) = ctx.input(|i| (i.time, i.modifiers.shift));
    let q = snap::tracking::CursorQuery { base, deferred, aperture: app.session.settings.aperture / xf.scale, tol: 6.0 / xf.scale, now, shift };
    let r = app.session.snap_cursor(raw, &q);
    if r.wait {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(snap::tracking::DWELL / 4.0));
    }
    app.canvas.snap = r.snap;
    app.canvas.track = r.track;
    r.point
}

/// The vertical wheel scroll this frame that zooms, in points, read from the raw wheel events so
/// each notch zooms by its whole step at once, as in standard CAD (#660). egui's smoothed delta
/// spreads a notch over several frames, so quick notches lag behind the wheel. Units convert to
/// points as egui does (a line is `line_scroll_speed` points, a page the view's height), and
/// events egui turns into a pinch zoom (⌘/Ctrl) or a sideways scroll (⇧) are left out, as it does.
fn wheel_zoom_points(events: &[egui::Event], opts: &egui::InputOptions, page: f32) -> f32 {
    let points: f32 = events
        .iter()
        .filter_map(|e| match e {
            egui::Event::MouseWheel { unit, delta, modifiers, .. }
                if !modifiers.matches_any(opts.zoom_modifier) && !modifiers.matches_any(opts.horizontal_scroll_modifier) =>
            {
                let per = match unit {
                    egui::MouseWheelUnit::Point => 1.0,
                    egui::MouseWheelUnit::Line => opts.line_scroll_speed,
                    egui::MouseWheelUnit::Page => page,
                };
                Some(delta.y * per)
            }
            _ => None,
        })
        .sum();
    if points.is_finite() { points } else { 0.0 }
}

pub fn show(app: &mut CadApp, ui: &mut egui::Ui) {
    let t = Tokens::get();
    let full = ui.available_rect_before_wrap();
    // Tiled model viewports (VPORTS): the current tile is the drawing area below; `draw_tiles`
    // draws the others.
    let tiles = tile_rects(app, full);
    let rect = tiles.iter().find(|(_, tile)| tile.active).map_or(full, |(r, _)| *r);
    let resp = ui.interact(full, ui.id().with("canvas"), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    app.canvas.rect = Some(rect);
    app.session.viewport_px = (f64::from(rect.width()).max(1.0), f64::from(rect.height()).max(1.0));
    if app.canvas.zoom_pending {
        app.canvas.zoom_pending = false;
        let _ = app.session.zoom_extents();
    }
    let Ok(st) = app.session.state() else { return };
    let view = st.view();
    let scale = f64::from(rect.height()) / view.height.max(1e-12);
    let xf = Xf { rect, center: view.center, scale };
    app.canvas.xf = Some(xf);

    // ---------- input ----------
    let (hover_pos, scroll, mods, middle_down, pressed_primary, pressed_secondary, dbl_middle, dbl_primary) = ui.input(|i| {
        (
            i.pointer.hover_pos(),
            i.smooth_scroll_delta,
            i.modifiers,
            i.pointer.middle_down(),
            i.pointer.primary_clicked(),
            i.pointer.secondary_clicked(),
            i.pointer.button_double_clicked(egui::PointerButton::Middle),
            i.pointer.button_double_clicked(egui::PointerButton::Primary),
        )
    });
    let inside = hover_pos.is_some_and(|p| rect.contains(p)) && resp.hovered();
    // Which device is scrolling? On macOS, trackpads (and Magic Mouse) report precise point
    // deltas; notched mouse wheels report lines. The answer is sticky because egui spreads a wheel
    // notch over several frames. Elsewhere (browsers, Wayland smooth-scroll mice) ordinary wheels
    // also report points, so the wheel keeps zooming there.
    ui.input(|i| {
        for e in &i.raw.events {
            if let egui::Event::MouseWheel { unit, .. } = e {
                app.canvas.scroll_pans = cfg!(target_os = "macos") && *unit == egui::MouseWheelUnit::Point;
            }
        }
    });
    if inside && app.canvas.scroll_pans && scroll != egui::Vec2::ZERO {
        // Two-finger swipe pans: the drawing follows the fingers, like a middle-drag.
        // Pinch (and Cmd+scroll) still zoom via `zoom_delta` below.
        if let Ok(st) = app.session.state_mut() {
            let v = st.view();
            st.set_view_quiet(cadcraft_engine::View {
                center: v.center - Vec2::new(f64::from(scroll.x) / scale, -f64::from(scroll.y) / scale),
                height: v.height,
            });
        }
    }
    let opts = ui.ctx().options(|o| o.input_options);
    let wheel = ui.input(|i| wheel_zoom_points(&i.raw.events, &opts, rect.height()));
    if inside
        && !app.canvas.scroll_pans
        && wheel != 0.0
        && let Some(hp) = hover_pos
    {
        // Zoom with the mouse wheel about the cursor, each notch at once.
        let f = app.session.wheel_zoom_factor(f64::from(wheel));
        let about = xf.to_world(hp);
        let _ = app.session.zoom_about(f, about);
    }
    let zoom_pinch = ui.input(|i| i.zoom_delta());
    if inside
        && (zoom_pinch - 1.0).abs() > 1e-4
        && let Some(hp) = hover_pos
    {
        let _ = app.session.zoom_about(f64::from(zoom_pinch), xf.to_world(hp));
    }
    // Pan with the middle button (or Shift+right-drag / two-finger drag on trackpads is scroll).
    let middle_pressed = ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Middle));
    let focused = ui.input(|i| i.focused);
    if middle_down
        && focused
        && (app.canvas.pan_last.is_some() || (middle_pressed && inside))
        && let Some(hp) = hover_pos
    {
        if let Some(last) = app.canvas.pan_last {
            let d = hp - last;
            if let Ok(st) = app.session.state_mut() {
                let v = st.view();
                st.set_view_quiet(cadcraft_engine::View {
                    center: v.center - Vec2::new(f64::from(d.x) / scale, -f64::from(d.y) / scale),
                    height: v.height,
                });
            }
        }
        app.canvas.pan_last = Some(hp);
    } else {
        app.canvas.pan_last = None;
    }
    if dbl_middle && inside {
        let _ = app.session.zoom_extents();
    }
    // ZOOM Realtime: dragging up zooms in, down zooms out.
    if resp.dragged_by(egui::PointerButton::Primary) && app.session.realtime_zoom_active() {
        let _ = app.session.realtime_zoom(f64::from(resp.drag_delta().y));
    }
    // A click in another tile makes it current (or answers -VPORTS Join's prompts).
    if pressed_primary
        && let Some(hp) = hover_pos
        && !rect.contains(hp)
        && let Some(i) = tiles.iter().position(|(r, _)| r.contains(hp))
    {
        if let Err(e) = app.session.click_tile(i) {
            app.session.echo(e.to_string());
        }
        ui.ctx().request_repaint();
        return;
    }
    // Re-read the view after navigation.
    let view = app.session.state().map(|s| s.view()).unwrap_or(view);
    let scale = f64::from(rect.height()) / view.height.max(1e-12);
    let xf = Xf { rect, center: view.center, scale };
    app.canvas.xf = Some(xf);
    // The paper (display) transform; equal to `xf` except inside an MSPACE viewport.
    let (xf_paper, active_vp) = match app.session.state() {
        Ok(st) => {
            let pv = st.paper_view();
            let xp = Xf { rect, center: pv.center, scale: f64::from(rect.height()) / pv.height.max(1e-12) };
            (if st.mspace.is_some() { xp } else { xf }, st.active_viewport().map(|(_, v)| v))
        }
        Err(_) => (xf, None),
    };
    // A clipped active viewport's outline (picks and its heavy border follow it).
    let active_clip = app.session.state().ok().and_then(|st| st.active_viewport().and_then(|(h, v)| cadcraft_render::viewport_clip(&st.doc, h, &v)));
    // Double-click a viewport to work inside it; double-click the sheet outside it to return.
    if inside
        && dbl_primary
        && app.session.running.is_none()
        && matches!(app.session.layout_space(), cadcraft_doc::Space::Paper(_))
        && let Some(hp) = hover_pos
    {
        let pp = xf_paper.to_world(hp);
        let in_rect = |v: &cadcraft_doc::Viewport| (pp.x - v.center.x).abs() <= v.width / 2.0 && (pp.y - v.center.y).abs() <= v.height / 2.0;
        let in_active = active_vp.as_ref().is_some_and(|v| active_clip.as_ref().map_or(in_rect(v), |c| c.contains(pp)));
        if !in_active {
            app.canvas.hot_grips.clear();
            if app.run("mspace", serde_json::json!({"at": [pp.x, pp.y]})).is_err() && active_vp.is_some() {
                let _ = app.run("pspace", serde_json::json!({}));
            }
            app.canvas.list = None;
            return;
        }
    }

    let raw_world = hover_pos.filter(|_| inside).map(|p| xf.to_world(p));
    let dyn_frame = crate::cmdline::dyn_frame(app);
    if let Some(w) = raw_world {
        let mut eff = effective_point(app, w, &xf, ui.ctx());
        // A locked Dynamic Input value (`40,` or `40<` typed) holds the cursor and overrides snaps.
        if let Some(f) = dyn_frame
            && let Some(e) = crate::dyninput::parse(&app.cmd.buffer, &f)
            && e.first_locked().is_some()
        {
            eff = crate::dyninput::constrain(&e, &f, eff, app.cmd.second_locked());
            app.canvas.snap = None;
            app.canvas.track = None;
        }
        // Grip dimensional input: typed lengths and angles hold the grip and override snaps.
        let grip_held = crate::grip_dyn::constrain(app, eff);
        if let Some(q) = grip_held {
            eff = q;
            app.canvas.snap = None;
            app.canvas.track = None;
        }
        app.canvas.cursor = Some(eff);
        app.session.cursor = eff;
        app.session.cursor_deferred = app.canvas.snap.and_then(|h| h.deferred);
        // Typed or menu snap overrides (END, NON…) and Tab cycling; a locked Dynamic Input value wins.
        if dyn_frame.and_then(|f| crate::dyninput::parse(&app.cmd.buffer, &f)).is_none_or(|e| e.first_locked().is_none()) {
            override_cursor(app, w, &xf);
        }
        // Typed grip dimensions win over snap overrides too.
        if let Some(q) = grip_held {
            app.canvas.cursor = Some(q);
            app.session.cursor = q;
            app.canvas.snap = None;
            app.session.cursor_deferred = None;
        }
    } else {
        app.canvas.cursor = None;
        app.canvas.snap = None;
        app.session.cursor_deferred = None;
    }

    // A click on the Dynamic Input options menu picks the keyword; elsewhere it closes the menu.
    let mut picked = false;
    if pressed_primary && app.cmd.options.is_some() {
        match hover_pos.and_then(|h| app.cmd.option_at(h)) {
            Some(k) => {
                crate::cmdline::pick_option(app, k);
                picked = true;
            }
            None => app.cmd.options = None,
        }
    }
    let pressed_primary = pressed_primary && !picked;
    // Clicks.
    if inside
        && pressed_primary
        && !middle_down
        && let Some(p) = app.canvas.cursor
    {
        if app.session.running.is_some() {
            // Ctrl held while picking the point of a grip edit makes copies (the Copy option).
            if mods.ctrl && app.session.running.as_ref().is_some_and(|r| r.id == "grip") {
                let _ = app.session.input(Input::Keyword("Copy".into()));
            }
            // Clicking places the point; a value typed into the Dynamic Input boxes is used up.
            if let Some(f) = dyn_frame
                && crate::dyninput::parse(&app.cmd.buffer, &f).is_some()
            {
                app.cmd.buffer.clear();
            }
            // Preserve a snap's viewport origin for DIST, and its deferred curve where needed.
            let result = match app.canvas.snap {
                Some(hit) => app.session.input_snapped(hit),
                None => app.session.input(Input::Point(p)),
            };
            if let Err(e) = result {
                app.session.echo(e.to_string());
            }
        } else if let Some(g) = grip_at(app, &xf, hover_pos) {
            click_grip(app, g, mods.shift);
        } else if let Err(e) = app.session.idle_click(raw_world.unwrap_or(p), mods.shift) {
            app.session.echo(e.to_string());
        }
    }
    // Press-and-drag selection window (AutoCAD's PICKDRAG = 2; click-click still works): a drag
    // that starts while objects are being selected opens a window where the button went down,
    // and letting go closes it. Drags on grips and drags while drawing are left alone.
    if resp.drag_started_by(egui::PointerButton::Primary)
        && !middle_down
        && app.session.pending_window.is_none()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        && rect.contains(origin)
        && grip_at(app, &xf, Some(origin)).is_none()
    {
        app.canvas.drag_window = app.session.begin_window(xf.to_world(origin));
    }
    if app.canvas.drag_window && resp.drag_stopped_by(egui::PointerButton::Primary) {
        app.canvas.drag_window = false;
        if app.session.pending_window.is_some()
            && let Some(end) = ui.input(|i| i.pointer.interact_pos())
        {
            let end = xf.to_world(end);
            let done = if app.session.running.is_some() { app.session.input(Input::Point(end)) } else { app.session.idle_click(end, mods.shift) };
            if let Err(e) = done {
                app.session.echo(e.to_string());
            }
        }
    }
    // Right-click acts as Enter while a command runs (the classic CAD default; held, it opens the
    // command menu); otherwise it opens the shortcut menu (Repeat, Clipboard, …).
    // Shift/Ctrl+right-click opens the object snap menu.
    let idle = app.session.running.is_none();
    if inside && pressed_secondary {
        app.canvas.snap_menu = mods.shift || mods.ctrl;
    }
    if crate::context_menu::in_command(app, ui, inside, pressed_secondary) {
        let _ = app.session.input(Input::Enter);
    }
    // Right-click on a grip with options (polyline vertex, line end…) opens the grip menu.
    if inside && pressed_secondary && idle && !app.canvas.snap_menu {
        app.canvas.grip_menu = grip_at(app, &xf, hover_pos).filter(|g| !grip_options(app, *g).is_empty());
    }
    if idle && !app.canvas.snap_menu && app.canvas.grip_menu.is_some() {
        grip_menu(app, &resp);
    } else if idle || app.canvas.snap_menu {
        crate::context_menu::show(app, &resp);
    }

    // Hover highlight (throttled to cursor movement).
    // The cursor picks objects (pick box, rollover highlight): "Select objects" and single-object
    // prompts, or no command with PICKFIRST on.
    let selecting = app.session.picking_objects();
    if inside && selecting && app.session.pending_window.is_none() {
        if hover_pos != app.canvas.hover_at
            && let (Some(w), Ok(st)) = (raw_world, app.session.state())
        {
            app.canvas.hover = cadcraft_engine::select::pick(&st.doc, &st.edit_space(), w, app.session.settings.pickbox.max(1.0) * 1.5 / scale);
            app.canvas.hover_at = hover_pos;
        }
    } else {
        app.canvas.hover = None;
        app.canvas.hover_at = None;
    }

    // ---------- paint ----------
    painter.rect_filled(rect, 0.0, t.canvas);
    let t0 = crate::now_ms();
    let mut views = vec![(xf_paper.world_bounds(), 1.0 / xf_paper.scale)];
    views.extend(tiles.iter().filter(|(r, tile)| !tile.active && r.width() >= 1.0 && r.height() >= 1.0).map(|(r, tile)| {
        let xf = tile_xf(*r, tile);
        (xf.world_bounds(), 1.0 / xf.scale)
    }));
    ensure_list(app, 1.0 / xf_paper.scale, &views);
    let sheet = app.canvas.list.as_ref().and_then(|l| l.sheet);
    let bg = match sheet {
        Some(sh) => {
            // Paper space: grey surround, the sheet with a shadow, the printable area dashed.
            painter.rect_filled(rect, 0.0, Color32::from_rgb(0x50, 0x57, 0x63));
            let a = xf_paper.to_screen(Vec2::new(0.0, sh.size.y));
            let b = xf_paper.to_screen(Vec2::new(sh.size.x, 0.0));
            let paper = Rect::from_two_pos(a, b);
            painter.rect_filled(paper.translate(vec2(5.0, 5.0)), 0.0, Color32::from_black_alpha(110));
            painter.rect_filled(paper, 0.0, t.paper);
            let pa = Rect::from_two_pos(
                xf_paper.to_screen(Vec2::new(sh.printable.min.x, sh.printable.max.y)),
                xf_paper.to_screen(Vec2::new(sh.printable.max.x, sh.printable.min.y)),
            );
            let pts = [pa.left_top(), pa.right_top(), pa.right_bottom(), pa.left_bottom(), pa.left_top()];
            painter.extend(Shape::dashed_line(&pts, Stroke::new(1.0, Color32::from_gray(150)), 4.0, 4.0));
            Rgb(t.paper.r(), t.paper.g(), t.paper.b())
        }
        None => {
            draw_grid(app, &painter, &xf);
            Rgb(t.canvas.r(), t.canvas.g(), t.canvas.b())
        }
    };
    let sel = app.session.selection();
    if app.canvas.gpu.is_some() {
        draw_list_gpu(&mut app.canvas, &painter, &xf_paper, bg, app.session.settings.lwdisplay);
    } else if let Some(list) = &app.canvas.list {
        draw_list(&painter, &xf_paper, list, bg, app.session.settings.lwdisplay);
    }
    if let Some(vp) = &active_vp {
        // The active viewport: a heavy border, highlights in model units clipped to it.
        let r = Rect::from_two_pos(
            xf_paper.to_screen(Vec2::new(vp.center.x - vp.width / 2.0, vp.center.y + vp.height / 2.0)),
            xf_paper.to_screen(Vec2::new(vp.center.x + vp.width / 2.0, vp.center.y - vp.height / 2.0)),
        );
        let heavy = Stroke::new(3.0, Color32::from_rgb(0x2a, 0x2a, 0x2a));
        match &active_clip {
            Some(c) => {
                let mut pts: Vec<egui::Pos2> = c.polygon().iter().map(|q| xf_paper.to_screen(*q)).collect();
                pts.extend(pts.first().copied());
                painter.add(egui::Shape::line(pts, heavy));
            }
            None => {
                painter.rect_stroke(r, 0.0, heavy, egui::StrokeKind::Middle);
            }
        }
        if let Ok(st) = app.session.state() {
            let mut hs = sel.clone();
            hs.extend(app.canvas.hover.filter(|h| !sel.contains(h)));
            let ents: Vec<_> = hs.iter().filter_map(|h| st.doc.model.get(*h)).map(|e| e.as_ref()).collect();
            if !ents.is_empty() {
                let tolerance = viewport_highlight_tolerance(scale);
                let key = {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    (st.uid, st.revision, std::sync::Arc::as_ptr(&st.doc) as usize, tolerance.to_bits(), &hs).hash(&mut h);
                    h.finish()
                };
                if app.canvas.vp_hl.as_ref().is_none_or(|(k, _)| *k != key) {
                    let built = cadcraft_render::build_entities(&st.doc, ents, &cadcraft_render::Options { tolerance, ..Default::default() });
                    app.canvas.vp_hl = Some((key, built));
                }
                if let Some((_, list)) = &app.canvas.vp_hl {
                    let clipped = painter.with_clip_rect(r.intersect(rect));
                    if let Some(h) = app.canvas.hover
                        && !sel.contains(&h)
                    {
                        draw_highlight(&clipped, &xf, list, &prims_of(list, &[h]), t.hover, 2.0, false);
                    }
                    draw_highlight(&clipped, &xf, list, &prims_of(list, &sel), t.selection, 1.5, true);
                }
            }
        }
    } else if let CanvasState { list: Some(list), list_gen, prim_index, hover, gpu, hl_key, hl_slot, hl_origin, hl_infinite, .. } = &mut app.canvas {
        if let Some(h) = *hover
            && !sel.contains(&h)
        {
            draw_highlight(&painter, &xf, list, &prim_index.prims(*list_gen, list, &[h]), t.hover, 2.0, false);
        }
        if gpu.is_some() && !sel.is_empty() {
            draw_highlight_gpu(&painter, &xf, list, *list_gen, prim_index, &sel, t.selection, 1.5, (hl_key, hl_slot, hl_origin, &mut *hl_infinite));
            draw_highlight(&painter, &xf, list, hl_infinite, t.selection, 1.5, true);
        } else {
            draw_highlight(&painter, &xf, list, &prim_index.prims(*list_gen, list, &sel), t.selection, 1.5, true);
        }
    }
    app.canvas.draw_ms = crate::now_ms() - t0;
    compare::paint(app, ui.ctx(), &painter, &xf_paper, rect);
    // Constraint bars and dynamic dimensional constraints (model space, or inside a viewport).
    if sheet.is_none() || active_vp.is_some() {
        let clip = match &active_vp {
            Some(vp) => Rect::from_two_pos(
                xf_paper.to_screen(Vec2::new(vp.center.x - vp.width / 2.0, vp.center.y + vp.height / 2.0)),
                xf_paper.to_screen(Vec2::new(vp.center.x + vp.width / 2.0, vp.center.y - vp.height / 2.0)),
            )
            .intersect(rect),
            None => rect,
        };
        let p = painter.with_clip_rect(clip);
        crate::parametric::draw_overlay(app, ui, &p, &xf);
    }
    // Grips on selected objects (when idle or editing grips).
    let grip_edit = app.session.running.as_ref().is_some_and(|r| r.id == "grip");
    if (!grip_edit && (app.canvas.grip_editing || app.session.running.is_some())) || sel.is_empty() {
        app.canvas.hot_grips.clear();
    }
    app.canvas.grip_editing = grip_edit;
    if (app.session.running.is_none() || grip_edit)
        && let Ok(st) = app.session.state()
    {
        let gs = app.session.settings.gripsize as f32;
        for h in sel.iter().take(200) {
            if let Some(e) = st.doc.entity(*h) {
                for (gi, g) in e.kind.grips().into_iter().enumerate() {
                    let sp = xf.to_screen(g);
                    if rect.contains(sp) {
                        let r = Rect::from_center_size(sp, vec2(gs * 2.0, gs * 2.0));
                        let hot = app.canvas.hot_grips.contains(&(*h, gi));
                        let warm = !hot && hover_pos.is_some_and(|hp| r.expand(2.0).contains(hp));
                        painter.rect_filled(
                            r,
                            0.0,
                            if hot {
                                t.grip_hot
                            } else if warm {
                                Color32::from_rgb(0xff, 0x7f, 0x9f)
                            } else {
                                t.grip
                            },
                        );
                        painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::from_rgb(0x10, 0x30, 0x80)), egui::StrokeKind::Middle);
                    }
                }
            }
        }
    }
    // Rubber band preview of the active command.
    // `xf` is the space being edited: paper space, or model space inside the active viewport.
    let drag = match (app.canvas.cursor, app.canvas.gpu.is_some()) {
        (Some(c), true) => app.session.drag(c),
        _ => None,
    };
    if let Some((drag, extra)) = drag {
        if let Ok(st) = app.session.state() {
            let serial = app.session.clipboard_source.as_ref().map_or(0, |c| c.serial);
            draw_drag_gpu(&mut app.canvas, &painter, &xf, &st.doc, &drag, (&app.session.clipboard, serial), bg);
            let list = cadcraft_render::build_entities(
                &st.doc,
                extra.iter(),
                &cadcraft_render::Options { tolerance: 0.5 / scale, min_dash: 2.0 / scale, ..Default::default() },
            );
            draw_list(&painter, &xf, &list, bg, false);
        }
    } else if let (Some(c), true) = (app.canvas.cursor, app.session.running.is_some()) {
        let ents = app.session.preview(c);
        if !ents.is_empty()
            && let Ok(st) = app.session.state()
        {
            let list = cadcraft_render::build_entities(
                &st.doc,
                ents.iter(),
                &cadcraft_render::Options { tolerance: 0.5 / scale, min_dash: 2.0 / scale, ..Default::default() },
            );
            draw_list(&painter, &xf, &list, bg, false);
        }
    }
    // Pending selection window.
    if let (Some(pw), Some(c)) = (app.session.pending_window, raw_world) {
        let a = xf.to_screen(pw.corner);
        let b = xf.to_screen(c);
        let r = Rect::from_two_pos(a, b);
        let crossing = c.x < pw.corner.x;
        painter.rect_filled(r, 0.0, if crossing { t.crossing_fill } else { t.window_fill });
        let st = Stroke::new(1.0, if crossing { t.crossing_stroke } else { t.window_stroke });
        if crossing {
            let pts = vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
            painter.extend(Shape::dashed_line(&pts, st, 5.0, 3.0));
        } else {
            painter.rect_stroke(r, 0.0, st, egui::StrokeKind::Middle);
        }
    }
    // Acquired tracking points and the polar / tracking paths under the cursor.
    if app.canvas.cursor.is_some() {
        draw_tracking(&painter, &xf, &app.session.tracking.acquired, app.canvas.track.as_ref(), view.height * 4.0);
    }
    if let (Some(hit), Some(_)) = (app.canvas.snap, app.canvas.cursor) {
        draw_snap_marker(&painter, xf.to_screen(hit.point), &hit);
    }
    draw_tiles(app, ui, &tiles);
    // UCS icon, ViewCube and viewport label.
    if app.ui.show_ucs_icon {
        draw_ucs_icon(&painter, rect);
        crate::viewcube::ucs_icon(app, ui, rect);
    }
    if app.ui.show_viewcube && sheet.is_none() {
        draw_viewcube(app, ui, rect);
    }
    viewport_label(&painter, rect);
    // Typing and Escape reach the command line even when its overlay is hidden.
    crate::cmdline::keyboard(app, ui.ctx());
    // Command line overlay.
    if app.ui.show_command_line {
        crate::cmdline::show(app, ui, full);
    }
    // Crosshair cursor.
    if inside {
        // Hide the OS cursor under the drawn crosshair, unless assistive tech needs it (#39).
        ui.ctx().set_cursor_icon(if app.ui.system_cursor { egui::CursorIcon::Crosshair } else { egui::CursorIcon::None });
        if let Some(hp) = hover_pos {
            draw_crosshair(app, &painter, rect, hp, selecting);
        }
        if app.session.settings.dynmode
            && let Some(hp) = hover_pos
        {
            // A stretched grip shows its dimensions instead of the pointer boxes.
            if !crate::grip_dyn::draw(app, &painter) {
                dynamic_input(app, &painter, hp);
            }
        }
    }
    // No continuous repaint while hovering: input events repaint by themselves. Repainting every
    // frame kept a vsync swapchain's queue full (every frame waited behind queued ones) and would
    // spin the CPU with the low-latency present mode (`gpu::surface_config`).
}

/// The model space tiles on screen (none without tiling): each tile's rectangle in `full`.
fn tile_rects(app: &CadApp, full: Rect) -> Vec<(Rect, cadcraft_engine::ModelTile)> {
    let x = |f: f64| full.left() + full.width() * f.clamp(0.0, 1.0) as f32;
    let y = |f: f64| full.bottom() - full.height() * f.clamp(0.0, 1.0) as f32;
    app.session
        .model_tiles()
        .into_iter()
        .map(|t| (Rect::from_min_max(pos2(x(t.min.x), y(t.max.y)), pos2(x(t.max.x), y(t.min.y))).shrink(1.0), t))
        .collect()
}

/// The world ↔ screen transform of a model tile drawn in `r`.
fn tile_xf(r: Rect, tile: &cadcraft_engine::ModelTile) -> Xf {
    Xf { rect: r, center: tile.view.center, scale: f64::from(r.height()) / tile.view.height.max(1e-12) }
}

/// The tiles other than the current one (each with its own view, clipped to it), then the
/// tile borders: thin, and heavy around the current tile.
fn draw_tiles(app: &CadApp, ui: &egui::Ui, tiles: &[(Rect, cadcraft_engine::ModelTile)]) {
    let t = Tokens::get();
    let bg = Rgb(t.canvas.r(), t.canvas.g(), t.canvas.b());
    for (r, tile) in tiles.iter().filter(|(r, tile)| !tile.active && r.width() >= 1.0 && r.height() >= 1.0) {
        let p = ui.painter_at(*r);
        let xf = tile_xf(*r, tile);
        p.rect_filled(*r, 0.0, t.canvas);
        draw_grid(app, &p, &xf);
        if let Some(list) = &app.canvas.list {
            draw_list(&p, &xf, list, bg, app.session.settings.lwdisplay);
        }
        viewport_label(&p, *r);
    }
    let p = ui.painter();
    for (r, tile) in tiles {
        let (w, c) = if tile.active { (2.0, t.accent) } else { (1.0, t.border) };
        p.rect_stroke(*r, 0.0, Stroke::new(w, c), egui::StrokeKind::Outside);
    }
}

fn tooltip(p: &egui::Painter, at: Pos2, text: &str) {
    let galley = p.layout_no_wrap(text.to_string(), crate::theme::small(), Color32::from_rgb(0x20, 0x20, 0x20));
    let r = Rect::from_min_size(at, galley.size() + vec2(8.0, 4.0));
    p.rect_filled(r, 2.0, Color32::from_rgb(0xf4, 0xf4, 0xf4));
    p.galley(r.min + vec2(4.0, 2.0), galley, Color32::BLACK);
}

/// Half the side of the pick box: the pick aperture (PICKBOX × 1.5 pixels, as `select::pick` is
/// called with), so the square shows what a click hits. `None` when PICKBOX is 0 (no box).
/// The candidate Tab has cycled to, if cycling: a Tab press starts or advances it, and the
/// cursor leaving the aperture where it started ends it.
fn snap_cycle(app: &mut CadApp, raw: Vec2, ap: f64) -> Option<usize> {
    let tab = std::mem::take(&mut app.canvas.snap_tab);
    let c = &mut app.canvas.snap_cycle;
    if c.is_some_and(|c| c.anchor.dist(raw) > ap) {
        *c = None;
    }
    if tab {
        *c = Some(c.map_or(SnapCycle { anchor: raw, index: 1 }, |c| SnapCycle { index: c.index + 1, ..c }));
    }
    c.map(|c| c.index)
}

/// The cursor under a one-pick object snap override (`Session::snap_override`: END, NON…) or
/// while Tab cycles the candidates: the snap of that mode (or the cycled candidate) replaces
/// what the running snaps found; with none, a running snap no longer holds the cursor.
fn override_cursor(app: &mut CadApp, raw: Vec2, xf: &Xf) {
    let over = app.session.snap_override();
    let ap = app.session.settings.aperture / xf.scale;
    let cycle = snap_cycle(app, raw, ap);
    let prompt = app.session.current_prompt();
    let wants_point = prompt.as_ref().is_some_and(cadcraft_engine::pointmod::accepts);
    if (over.is_none() && cycle.is_none()) || !wants_point || app.session.pending_window.is_some() {
        return;
    }
    let base = prompt.as_ref().and_then(|p| p.base);
    let deferred = prompt.as_ref().is_some_and(|p| p.deferred);
    let s = &app.session.settings;
    let osmode = over.unwrap_or(s.osmode) | if s.osnaphatch { snap::mode::HATCH } else { 0 };
    let Ok(st) = app.session.state() else { return };
    let hit = match cycle {
        Some(i) => {
            let cands = cadcraft_engine::pointmod::snap_candidates(&st.doc, &st.edit_space(), raw, ap, osmode, base, deferred);
            cands.get(i % cands.len().max(1)).copied()
        }
        None => snap::osnap(&st.doc, &st.edit_space(), raw, ap, osmode, base, deferred),
    };
    let p = match (hit, app.canvas.snap) {
        (Some(h), _) => {
            app.canvas.snap = Some(h);
            h.point
        }
        (None, Some(_)) => {
            app.canvas.snap = None;
            raw
        }
        (None, None) => return,
    };
    app.canvas.cursor = Some(p);
    app.session.cursor = p;
    app.session.cursor_deferred = app.canvas.snap.and_then(|h| h.deferred);
}

fn pickbox_half(pickbox: f64) -> Option<f32> {
    let half = (pickbox * 1.5) as f32;
    (half.is_finite() && half >= 1.0).then_some(half)
}

fn draw_crosshair(app: &CadApp, p: &egui::Painter, rect: Rect, at: Pos2, pickbox: bool) {
    let st = Stroke::new(1.0, Tokens::get().crosshair);
    let len = (rect.height() * app.session.settings.cursorsize as f32 / 100.0).max(12.0);
    let pb = if pickbox { pickbox_half(app.session.settings.pickbox) } else { None };
    let gap = pb.unwrap_or(0.0);
    // Along the isometric plane's axes in isometric drafting.
    if app.session.settings.isodraft {
        for a in cadcraft_engine::snap::iso::cursor_axes(&app.session.settings) {
            let dir = vec2(a.cos() as f32, -(a.sin() as f32));
            p.line_segment([at - dir * len, at - dir * gap], st);
            p.line_segment([at + dir * gap, at + dir * len], st);
        }
    } else {
        p.line_segment([pos2(at.x - len, at.y), pos2(at.x - gap, at.y)], st);
        p.line_segment([pos2(at.x + gap, at.y), pos2(at.x + len, at.y)], st);
        p.line_segment([pos2(at.x, at.y - len), pos2(at.x, at.y - gap)], st);
        p.line_segment([pos2(at.x, at.y + gap), pos2(at.x, at.y + len)], st);
    }
    if let Some(pb) = pb {
        p.rect_stroke(Rect::from_center_size(at, vec2(pb * 2.0, pb * 2.0)), 0.0, st, egui::StrokeKind::Middle);
    }
}

/// The Dynamic Input tooltip beside the cursor, styled like the command line. At a point prompt
/// it shows the pair of value boxes, a view of the command-line text (see [`crate::dyninput`]);
/// otherwise the prompt with what is being typed, or the live distance and angle.
fn dynamic_input(app: &mut CadApp, p: &egui::Painter, hover: Pos2) {
    app.cmd.option_rows.clear();
    // While the options menu is open the tooltip stays where it opened, so the menu can be reached.
    let at = match app.cmd.options {
        Some(_) => *app.cmd.options_anchor.get_or_insert(hover),
        None => {
            app.cmd.options_anchor = None;
            hover
        }
    };
    let Some(prompt) = app.session.current_prompt() else { return };
    // DYNPROMPT = 0: no prompt beside the cursor (the value boxes stay).
    let show_prompt = app.session.settings.dynprompt;
    let options = show_prompt && !prompt.keywords.is_empty();
    if let Some(f) = crate::cmdline::dyn_frame(app)
        && let Some(e) = crate::dyninput::parse(&app.cmd.buffer, &f)
        && let Some(c) = app.canvas.cursor
    {
        let message = if show_prompt { crate::i18n::t(&prompt.message) } else { "" };
        let r = dynamic_input_boxes(p, at, message, &f, &e, c, app.cmd.second_locked(), options);
        options_menu(app, p, r, hover);
        return;
    }
    if !show_prompt {
        return;
    }
    let mut text = crate::i18n::t(&prompt.message).to_string();
    if !app.cmd.buffer.is_empty() {
        text = format!("{}: {}", crate::i18n::t(&prompt.message), app.cmd.buffer);
    } else if let (Some(base), Some(c)) = (prompt.base, app.canvas.cursor) {
        text = format!(
            "{}   {}  <  {}°",
            crate::i18n::t(&prompt.message),
            cadcraft_engine::units::format_distance(base.dist(c), 2, 4),
            (base.angle_to(c).to_degrees() * 10.0).round() / 10.0
        );
    } else if let Some(c) = app.canvas.cursor {
        text = format!(
            "{}   {}, {}",
            crate::i18n::t(&prompt.message),
            cadcraft_engine::units::format_distance(c.x, 2, 4),
            cadcraft_engine::units::format_distance(c.y, 2, 4)
        );
    }
    let t = Tokens::get();
    let galley = p.layout_no_wrap(text, crate::theme::small(), t.text);
    let hint = if options { OPTIONS_HINT_W } else { 0.0 };
    let r = tooltip_frame(p, at, galley.size() + vec2(12.0 + hint, 8.0));
    p.galley(r.min + vec2(6.0, 4.0), galley, t.text);
    if options {
        options_hint(p, r);
    }
    options_menu(app, p, r, hover);
}

/// Room for the down-arrow sign that a prompt has options.
const OPTIONS_HINT_W: f32 = 14.0;

/// The down-arrow sign at the right end of the tooltip `r`: the prompt has options, and Down
/// Arrow lists them.
fn options_hint(p: &egui::Painter, r: Rect) {
    let t = Tokens::get();
    let c = pos2(r.right() - OPTIONS_HINT_W / 2.0 - 2.0, r.center().y + 1.0);
    p.add(Shape::convex_polygon(vec![pos2(c.x - 4.0, c.y - 2.5), pos2(c.x + 4.0, c.y - 2.5), pos2(c.x, c.y + 2.5)], t.text_dim, Stroke::NONE));
}

/// The options menu under the tooltip `r` (DYNPROMPT): one row per keyword, the highlighted one
/// marked; the rows are kept for clicks.
fn options_menu(app: &mut CadApp, p: &egui::Painter, r: Rect, hover: Pos2) {
    let Some(mut menu) = app.cmd.options.clone() else { return };
    let t = Tokens::get();
    let font = crate::theme::small();
    let rows: Vec<_> = menu.keywords.iter().map(|k| p.layout_no_wrap(k.clone(), font.clone(), t.text)).collect();
    let row_h = rows.iter().map(|g| g.size().y).fold(12.0, f32::max) + 6.0;
    let w = rows.iter().map(|g| g.size().x).fold(60.0, f32::max) + 28.0;
    let h = row_h * rows.len() as f32 + 4.0;
    let m = Rect::from_min_size(pos2(r.left(), r.bottom() + 2.0), vec2(w, h));
    p.rect_filled(m, 3.0, t.cmd_bg);
    p.rect_stroke(m, 3.0, Stroke::new(1.0, t.cmd_border), egui::StrokeKind::Inside);
    let row_at = |i: usize| Rect::from_min_size(pos2(m.left() + 2.0, m.top() + 2.0 + row_h * i as f32), vec2(w - 4.0, row_h));
    // The row under the pointer is the highlighted one.
    if let Some(i) = (0..menu.keywords.len()).find(|i| row_at(*i).contains(hover)) {
        menu.index = i;
        app.cmd.options = Some(menu.clone());
    }
    for (i, (g, k)) in rows.into_iter().zip(&menu.keywords).enumerate() {
        let row = row_at(i);
        if i == menu.index {
            p.rect_filled(row, 2.0, t.cmd_keyword_hover);
            p.circle_filled(pos2(row.left() + 9.0, row.center().y), 2.5, t.accent);
        }
        p.galley(pos2(row.left() + 20.0, row.center().y - g.size().y / 2.0), g, t.text);
        app.cmd.option_rows.push((row, k.clone()));
    }
}

/// The tooltip background, matching the command line bar; returns its rectangle.
fn tooltip_frame(p: &egui::Painter, at: Pos2, size: egui::Vec2) -> Rect {
    let t = Tokens::get();
    let r = Rect::from_min_size(at + vec2(18.0, -size.y - 14.0), size);
    p.rect_filled(r, 3.0, t.cmd_bg);
    p.rect_stroke(r, 3.0, Stroke::new(1.0, t.cmd_border), egui::StrokeKind::Inside);
    r
}

/// The Dynamic Input value boxes: X and Y, or distance and angle. Typed values show in full
/// text colour, a locked value (a separator typed after it) gets a lock, the active box gets the
/// accent outline, and open boxes show the cursor's live value, dimmed. Values measured from the
/// last point are marked with `@`.
#[allow(clippy::too_many_arguments)]
fn dynamic_input_boxes(
    p: &egui::Painter,
    at: Pos2,
    message: &str,
    f: &crate::dyninput::Frame,
    e: &crate::dyninput::Entry<'_>,
    cursor: Vec2,
    second_locked: bool,
    options: bool,
) -> Rect {
    use crate::dyninput::Format;
    let t = Tokens::get();
    let font = crate::theme::small();
    let dist = |v: f64| cadcraft_engine::units::format_distance(v, 2, 4);
    let (live_first, live_second) = f.live(e.format, cursor);
    let live_second = match e.format {
        Format::Cartesian => dist(live_second),
        Format::Polar => format!("{}°", (live_second.to_degrees() * 10.0).round() / 10.0),
    };
    let (label_first, label_second) = match e.format {
        Format::Cartesian => ("X", "Y"),
        Format::Polar => ("", "<"),
    };
    // (label, text, typed, locked, active)
    let cells = [
        (
            label_first,
            if e.first.is_empty() { dist(live_first) } else { e.first.to_owned() },
            !e.first.is_empty(),
            e.first_locked().is_some(),
            !e.split,
        ),
        (
            label_second,
            if e.second.is_empty() { live_second } else { e.second.to_owned() },
            !e.second.is_empty(),
            second_locked,
            e.split && !second_locked,
        ),
    ];

    let msg = p.layout_no_wrap(message.to_owned(), font.clone(), t.text_dim);
    let marker = p.layout_no_wrap(if f.relative() { "@".to_owned() } else { String::new() }, font.clone(), t.text_faint);
    let lock_w = 10.0;
    let laid: Vec<_> = cells
        .into_iter()
        .map(|(label, text, typed, locked, active)| {
            let label = p.layout_no_wrap(label.to_owned(), font.clone(), t.text_faint);
            let value = p.layout_no_wrap(text, font.clone(), if typed { t.text } else { t.text_faint });
            let w = (value.size().x + 10.0 + if locked { lock_w } else { 0.0 }).max(48.0);
            (label, value, w, locked, active)
        })
        .collect();

    // Measure, then draw.
    let pad = 6.0;
    let gap = 6.0;
    let box_h = msg.size().y + 6.0;
    let mut w = pad + msg.size().x + gap + marker.size().x;
    for (label, _, bw, _, _) in &laid {
        w += gap + label.size().x + 3.0 + bw;
    }
    let hint = if options { OPTIONS_HINT_W } else { 0.0 };
    let r = tooltip_frame(p, at, vec2(w + pad + hint, box_h + 2.0 * pad - 2.0));
    if options {
        options_hint(p, r);
    }
    let cy = r.center().y;
    let mut x = r.left() + pad;
    let (msg_w, msg_h) = (msg.size().x, msg.size().y);
    p.galley(pos2(x, cy - msg_h / 2.0), msg, t.text_dim);
    x += msg_w + gap;
    let mw = marker.size().x;
    p.galley(pos2(x, cy - marker.size().y / 2.0), marker, t.text_faint);
    x += mw;
    for (label, value, bw, locked, active) in laid {
        x += gap;
        let lw = label.size().x;
        p.galley(pos2(x, cy - label.size().y / 2.0), label, t.text_faint);
        x += lw + 3.0;
        let b = Rect::from_min_size(pos2(x, cy - box_h / 2.0), vec2(bw, box_h));
        p.rect_filled(b, 2.0, t.chrome_dark);
        let stroke = if active { Stroke::new(1.5, t.accent) } else { Stroke::new(1.0, t.border) };
        p.rect_stroke(b, 2.0, stroke, egui::StrokeKind::Inside);
        let vw = value.size().x;
        p.galley(pos2(b.left() + 5.0, cy - value.size().y / 2.0), value, t.text);
        if active {
            let cx = b.left() + 6.0 + vw;
            p.line_segment([pos2(cx, b.top() + 3.0), pos2(cx, b.bottom() - 3.0)], Stroke::new(1.0, t.text));
        }
        if locked {
            draw_lock(p, pos2(b.right() - lock_w / 2.0 - 3.0, cy), t.accent);
        }
        x += bw;
    }
    r
}

/// A small padlock, drawn in code: body plus shackle.
fn draw_lock(p: &egui::Painter, c: Pos2, color: Color32) {
    p.rect_filled(Rect::from_center_size(c + vec2(0.0, 1.5), vec2(7.0, 5.0)), 1.0, color);
    p.add(Shape::line(
        vec![pos2(c.x - 2.0, c.y - 1.0), pos2(c.x - 2.0, c.y - 4.0), pos2(c.x + 2.0, c.y - 4.0), pos2(c.x + 2.0, c.y - 1.0)],
        Stroke::new(1.2, color),
    ));
}

fn draw_ucs_icon(p: &egui::Painter, rect: Rect) {
    let t = Tokens::get();
    let o = pos2(rect.left() + 36.0, rect.bottom() - 34.0);
    let st = Stroke::new(1.0, t.canvas_ink);
    p.line_segment([o, o + vec2(60.0, 0.0)], st);
    p.line_segment([o, o + vec2(0.0, -60.0)], st);
    p.rect_stroke(Rect::from_center_size(o, vec2(9.0, 9.0)), 0.0, st, egui::StrokeKind::Middle);
    let f = egui::FontId::proportional(13.0);
    p.text(o + vec2(70.0, 0.0), egui::Align2::LEFT_CENTER, crate::tl!("X"), f.clone(), t.canvas_ink);
    p.text(o + vec2(0.0, -70.0), egui::Align2::CENTER_BOTTOM, crate::tl!("Y"), f, t.canvas_ink);
}

fn viewport_label(p: &egui::Painter, rect: Rect) {
    let t = Tokens::get();
    let at = pos2(rect.left() + 10.0, rect.top() + 8.0);
    p.text(at, egui::Align2::LEFT_TOP, crate::tl!("+  |  Top  |  2D Wireframe"), crate::theme::small(), t.canvas_ink);
}

fn draw_viewcube(app: &mut CadApp, ui: &mut egui::Ui, rect: Rect) {
    let l = crate::viewcube::Layout::new(rect);
    let (c, ring) = (l.center, l.ring);
    let p = ui.painter_at(rect);
    crate::viewcube::show(app, ui, &l);
    // WCS pill.
    let pill = Rect::from_center_size(c + vec2(0.0, ring + 26.0), vec2(56.0, 16.0));
    p.rect_filled(pill, 8.0, Color32::from_rgb(0x48, 0x50, 0x5c));
    p.text(pill.center(), egui::Align2::CENTER_CENTER, crate::tl!("WCS ⌄"), crate::theme::small(), Tokens::DARK.canvas_ink);
}

/// A click on grip `g` (idle): Shift+click makes it hot or cold again; a plain click starts
/// editing it, with the other hot grips when it is one of them.
fn click_grip(app: &mut CadApp, g: (Handle, usize), shift: bool) {
    let hot = &mut app.canvas.hot_grips;
    if shift {
        match hot.iter().position(|x| *x == g) {
            Some(i) => {
                hot.remove(i);
            }
            None => hot.push(g),
        }
        return;
    }
    if !hot.contains(&g) {
        hot.clear();
    }
    hot.retain(|x| *x != g);
    hot.insert(0, g);
    let hs = hot.clone();
    if let Err(e) = app.session.start_grip_edit(&hs) {
        app.canvas.hot_grips.clear();
        app.session.echo(e.to_string());
    }
}

/// The multifunctional options of grip `g` (Add Vertex, Lengthen…; empty: Stretch only).
fn grip_options(app: &CadApp, (h, i): (Handle, usize)) -> Vec<cadcraft_engine::grips::GripAction> {
    let Ok(st) = app.session.state() else { return Vec::new() };
    st.doc.entity(h).map(|e| cadcraft_engine::grips::grip_actions(&e.kind, i)).unwrap_or_default()
}

/// The menu of a multifunctional grip: Stretch and its options. A choice makes the grip hot and
/// runs the option at the grip prompt, as typing its keyword would.
fn grip_menu(app: &mut CadApp, resp: &egui::Response) {
    let Some(g) = app.canvas.grip_menu else { return };
    let options = grip_options(app, g);
    let mut chosen: Option<Option<cadcraft_engine::grips::GripAction>> = None;
    let open = resp
        .context_menu(|ui| {
            let vertex = options.contains(&cadcraft_engine::grips::GripAction::RemoveVertex);
            if ui.button(crate::i18n::t(if vertex { "Stretch Vertex" } else { "Stretch" })).clicked() {
                chosen = Some(None);
                ui.close();
            }
            for a in &options {
                if ui.button(crate::i18n::t(a.label())).clicked() {
                    chosen = Some(Some(*a));
                    ui.close();
                }
            }
        })
        .is_some();
    if !open || chosen.is_some() {
        app.canvas.grip_menu = None;
    }
    let Some(choice) = chosen else { return };
    app.canvas.hot_grips = vec![g];
    let r = app.session.start_grip_edit(&[g]).and_then(|()| match choice {
        Some(a) => app.session.input(Input::Keyword(a.keyword().to_string())),
        None => Ok(()),
    });
    if let Err(e) = r {
        app.canvas.hot_grips.clear();
        app.session.echo(e.to_string());
    }
}

/// The grip of a selected object under the cursor, if any: (object, grip index).
fn grip_at(app: &CadApp, xf: &Xf, hover: Option<Pos2>) -> Option<(Handle, usize)> {
    let hp = hover?;
    let st = app.session.state().ok()?;
    let gs = app.session.settings.gripsize as f32 + 3.0;
    for h in app.session.selection().iter().take(200) {
        let e = st.doc.entity(*h)?;
        if st.doc.layer(&e.common.layer).is_some_and(|l| l.locked) {
            continue;
        }
        for (i, g) in e.kind.grips().into_iter().enumerate() {
            let sp = xf.to_screen(g);
            if (sp.x - hp.x).abs() <= gs && (sp.y - hp.y).abs() <= gs {
                return Some((*h, i));
            }
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use cadcraft_render::DPrim;

    fn list(handles: &[u64]) -> DisplayList {
        let prim = |h| DPrim { handle: Handle(h), color: Rgb(255, 255, 255), aci7: true, lw: 0.0, kind: Kind::Polyline, start: 0, len: 0 };
        DisplayList { prims: handles.iter().map(|&h| prim(h)).collect(), ..Default::default() }
    }

    fn hs(v: &[u64]) -> Vec<Handle> {
        v.iter().map(|&h| Handle(h)).collect()
    }

    #[test]
    fn viewport_highlight_tolerance_is_constant_within_a_zoom_band() {
        assert_eq!(viewport_highlight_tolerance(4.0), viewport_highlight_tolerance(3.0));
        assert_ne!(viewport_highlight_tolerance(4.0), viewport_highlight_tolerance(1.5));
        for s in [0.3, 1.0, 3.0, 4.0, 250.0] {
            assert!(viewport_highlight_tolerance(s) <= 0.5 / s);
        }
    }

    #[test]
    fn prim_index_matches_a_scan_and_follows_list_rebuilds() {
        let a = list(&[3, 1, 3, 2, 7, 3]);
        let mut ix = PrimIndex::default();
        assert_eq!(ix.prims(1, &a, &hs(&[3])), vec![0, 2, 5]);
        assert_eq!(ix.prims(1, &a, &hs(&[3])), prims_of(&a, &hs(&[3])));
        assert_eq!(ix.prims(1, &a, &hs(&[2, 1, 2])), vec![1, 3]);
        assert!(ix.prims(1, &a, &hs(&[9])).is_empty());
        assert!(ix.prims(1, &a, &[]).is_empty());
        // A rebuilt list (new generation) is re-indexed.
        let b = list(&[3, 9]);
        assert_eq!(ix.prims(2, &b, &hs(&[9, 3])), vec![0, 1]);
    }

    fn wheel(unit: egui::MouseWheelUnit, dy: f32, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::MouseWheel { unit, delta: vec2(0.0, dy), phase: egui::TouchPhase::Move, modifiers }
    }

    /// Wheel events convert to points as egui does; ⌘/Ctrl (egui's pinch zoom) and ⇧ (a sideways
    /// scroll) don't zoom here, and every notch of the frame counts (#660).
    #[test]
    fn wheel_zoom_reads_every_notch_of_the_frame() {
        use egui::{Modifiers, MouseWheelUnit as U};
        let o = egui::InputOptions::default();
        let none = Modifiers::NONE;
        assert_eq!(wheel_zoom_points(&[wheel(U::Line, 1.0, none)], &o, 500.0), o.line_scroll_speed);
        assert_eq!(wheel_zoom_points(&[wheel(U::Line, -2.0, none)], &o, 500.0), -2.0 * o.line_scroll_speed);
        assert_eq!(wheel_zoom_points(&[wheel(U::Point, 7.5, none)], &o, 500.0), 7.5);
        assert_eq!(wheel_zoom_points(&[wheel(U::Page, 1.0, none)], &o, 500.0), 500.0);
        // Three quick notches in one frame zoom three steps at once.
        let three = [wheel(U::Line, 1.0, none), wheel(U::Line, 1.0, none), wheel(U::Line, 1.0, none)];
        assert_eq!(wheel_zoom_points(&three, &o, 500.0), 3.0 * o.line_scroll_speed);
        assert_eq!(wheel_zoom_points(&[wheel(U::Line, 1.0, Modifiers::COMMAND)], &o, 500.0), 0.0);
        assert_eq!(wheel_zoom_points(&[wheel(U::Line, 1.0, Modifiers::SHIFT)], &o, 500.0), 0.0);
        assert_eq!(wheel_zoom_points(&[wheel(U::Line, f32::NAN, none)], &o, 500.0), 0.0);
        assert_eq!(wheel_zoom_points(&[egui::Event::PointerGone], &o, 500.0), 0.0);
    }

    /// The frame a notch arrives, egui's smoothed scroll holds only part of it (the rest trickles
    /// in over later frames, which made quick notches lag), while the raw reading zooms the
    /// whole notch at once (#660).
    #[test]
    fn a_wheel_notch_zooms_whole_in_its_own_frame() {
        let ctx = egui::Context::default();
        let o = ctx.options(|o| o.input_options);
        let input = egui::RawInput { events: vec![wheel(egui::MouseWheelUnit::Line, 1.0, egui::Modifiers::NONE)], ..Default::default() };
        let mut seen = (0.0, 0.0);
        ctx.run_ui(input, |ui| {
            seen = ui.input(|i| (i.smooth_scroll_delta.y, wheel_zoom_points(&i.raw.events, &o, 500.0)));
        })
        .textures_delta
        .clear();
        let (smoothed, raw) = seen;
        assert_eq!(raw, o.line_scroll_speed, "the whole notch");
        assert!(smoothed < raw, "egui's smoothed delta lags behind: {smoothed} of {raw}");
    }
}

/// The COMPARE overlay.
mod compare;
