//! GPU canvas: draws the drawing's display list with an `egui_wgpu` paint callback.
//!
//! The display list is converted once per (document, revision, tessellation band, background,
//! lineweight display, vertex origin) into vertex data ([`MeshData`]) and uploaded to wgpu
//! buffers; each frame only a small view uniform changes. Positions are stored as f32 relative to
//! a per-build origin so large world coordinates keep their precision; at deep zoom the origin
//! follows the view (see [`choose_origin`]).
//!
//! Batches: filled triangles (TriangleList), 1-px hairlines (LineList), and instanced screen-space
//! quads for wide lineweights and point markers. Fills are drawn first, then lines on top. A
//! wipeout starts a new draw-order group (see [`MeshData::breaks`]): its background-coloured mask
//! is drawn after everything before it and under everything after it. The shader only uses
//! vertex buffers and one uniform buffer, so it runs on WebGPU and WebGL2.
//!
//! Infinite lines (xline/ray) and all overlays stay egui shapes (see `canvas.rs`), and the
//! canvas falls back to the CPU path when no wgpu render state was handed to the app.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cadaddy_color::Rgb;
use cadaddy_geom::{Bounds2, Vec2};
use cadaddy_render::{DisplayList, Kind};
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;

/// Line/triangle vertex: position (2 × f32, relative to the origin) + sRGB colour (4 × u8,
/// premultiplied by its alpha: transparent objects blend with what is under them).
const VERTEX_SIZE: usize = 12;
/// Wide-line / point instance: a, b (2 × f32 each) + colour (4 × u8) + width in points (f32) +
/// length along its polyline before `a`, in world units (f32; dashing).
const INSTANCE_SIZE: usize = 28;
/// Vertices (or instances) per GPU buffer; divisible by 2 and 3 so no primitive straddles two.
const CHUNK: usize = 4_194_300;
/// Point markers are 1.5 × 1.5 points (as on the CPU path).
const POINT_SIZE: f32 = 1.5;
/// Image vertex: position (2 × f32, relative to the origin) + texture coordinates (2 × f32) +
/// opacity (f32).
const IMAGE_VERTEX_SIZE: usize = 20;

/// How the native window presents frames (`NativeOptions::wgpu_options.surface`).
///
/// The canvas hides the OS pointer and draws its own crosshair, so every frame the swapchain holds
/// back is visible as cursor lag. With FIFO (vsync) wgpu asks for one queued frame, but Vulkan
/// clamps that to the driver's minimum swapchain size, which is three images on X11 (two frames
/// queued behind the one on screen; Windows DX12 honours one). `low_latency` (the
/// app passes it on Linux/BSD) prefers a mode that never queues: `AutoNoVsync` picks Immediate,
/// then Mailbox, then falls back to FIFO, so it is valid on every surface; under a compositor the
/// compositor still syncs to the display. `vsync` is the `CADADDY_VSYNC` override: `1`/`on`
/// forces FIFO (tear-free without a compositor), `0`/`off` forces the low-latency mode.
pub fn surface_config(vsync: Option<&str>, low_latency: bool) -> egui_wgpu::SurfaceConfig {
    let no_vsync = match vsync.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("1" | "on" | "true" | "yes") => false,
        Some("0" | "off" | "false" | "no") => true,
        _ => low_latency,
    };
    egui_wgpu::SurfaceConfig {
        present_mode: if no_vsync { wgpu::PresentMode::AutoNoVsync } else { wgpu::PresentMode::AutoVsync },
        desired_maximum_frame_latency: Some(1),
    }
}

/// Frame-rate cap for presenting without vsync. Nothing else paces frames then (on X11; Wayland
/// still throttles to the compositor), so a 1000 Hz mouse made the app render ~900 frames per
/// second and keep a CPU core busy. Waiting out the rest of a short minimum interval adds at most
/// that interval of input age, far less than a vsync queue.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy, Debug)]
pub struct FrameCap {
    min: std::time::Duration,
    next: Option<web_time::Instant>,
}

#[cfg(not(target_arch = "wasm32"))]
impl FrameCap {
    /// At most `max_fps` frames per second.
    pub fn new(max_fps: u32) -> Self {
        FrameCap { min: std::time::Duration::from_secs(1) / max_fps.max(1), next: None }
    }

    /// How long a frame starting at `now` waits; records when the following one may start.
    pub fn wait_at(&mut self, now: web_time::Instant) -> std::time::Duration {
        let wait = self.next.map_or(std::time::Duration::ZERO, |n| n.saturating_duration_since(now));
        self.next = now.checked_add(wait.saturating_add(self.min));
        wait
    }

    /// Block until this frame may start (call once per frame, at its start).
    pub fn wait(&mut self) {
        let wait = self.wait_at(web_time::Instant::now());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
}

/// The wgpu target the app renders into, recorded by [`install`].
#[derive(Clone, Copy, Debug)]
pub struct GpuTarget {
    pub format: wgpu::TextureFormat,
    /// The stencil buffer format of the render pass, when it has one (the native window asks
    /// for one): clipped viewports are clipped with it.
    pub stencil: Option<wgpu::TextureFormat>,
}

/// CPU-side vertex data for one display list, waiting to be uploaded.
#[derive(Default)]
pub struct MeshData {
    pub lines: Vec<u8>,
    pub tris: Vec<u8>,
    pub quads: Vec<u8>,
    /// Draw-order breaks, one before each wipeout mask: the fill vertices, line vertices and
    /// quad instances drawn before it. Each group is drawn fills, lines, quads in turn.
    pub breaks: Vec<[usize; 3]>,
    /// Raster images, each drawn between two draw-order groups (see [`MeshImage::group`]).
    pub images: Vec<MeshImage>,
    /// What was drawn simpler than the list ([`build_screen_mesh`]).
    pub simplified: Simplified,
    /// The vertex buffers already created from this data (see [`MeshData::upload_now`]); the
    /// paint callback then only swaps them in.
    uploaded: Option<Uploaded>,
}

/// A raster image in a [`MeshData`]: its triangles with texture coordinates, painted with its
/// pixels after draw-order group `group` (the group ending at `breaks[group]`) and before the
/// next, so it covers what was drawn before it and lies under what comes after.
pub struct MeshImage {
    pub raster: Arc<cadaddy_render::images::Raster>,
    pub verts: Vec<u8>,
    pub group: usize,
}

/// GPU buffers made ahead of the frame that draws them.
struct Uploaded {
    lines: Vec<Batch>,
    tris: Vec<Batch>,
    quads: Vec<Batch>,
    images: Vec<LayerImage>,
}

/// An uploaded [`MeshImage`].
struct LayerImage {
    raster: Arc<cadaddy_render::images::Raster>,
    vertices: Option<Batch>,
    group: usize,
}

impl MeshData {
    /// Create the vertex buffers now, on the calling thread, and let the CPU copy go. Making a
    /// buffer zero-fills and copies it: about 0.35 s for a million-primitive drawing, which a
    /// background rebuild does off the UI thread (see `canvas::rebuild`).
    pub fn upload_now(&mut self, device: &wgpu::Device) {
        let usage = wgpu::BufferUsages::VERTEX;
        let images = std::mem::take(&mut self.images)
            .into_iter()
            .map(|im| LayerImage {
                vertices: upload(device, "cad_image", &im.verts, IMAGE_VERTEX_SIZE, usage).into_iter().next(),
                raster: im.raster,
                group: im.group,
            })
            .collect();
        self.uploaded = Some(Uploaded {
            lines: upload(device, "cad_lines", &self.lines, VERTEX_SIZE, usage),
            tris: upload(device, "cad_fills", &self.tris, VERTEX_SIZE, usage),
            quads: upload(device, "cad_quads", &self.quads, INSTANCE_SIZE, usage),
            images,
        });
        self.lines = Vec::new();
        self.tris = Vec::new();
        self.quads = Vec::new();
    }

    pub fn line_vertices(&self) -> usize {
        self.lines.len() / VERTEX_SIZE
    }
    pub fn tri_vertices(&self) -> usize {
        self.tris.len() / VERTEX_SIZE
    }
    pub fn quad_instances(&self) -> usize {
        self.quads.len() / INSTANCE_SIZE
    }
    /// Bytes of vertex data waiting to be uploaded.
    pub fn bytes(&self) -> usize {
        self.lines.len() + self.tris.len() + self.quads.len() + self.images.iter().map(|im| im.verts.len()).sum::<usize>()
    }
}

fn rel(p: Vec2, origin: Vec2) -> [f32; 2] {
    [(p.x - origin.x) as f32, (p.y - origin.y) as f32]
}

fn push_vertex(out: &mut Vec<u8>, p: [f32; 2], c: [u8; 4]) {
    out.extend_from_slice(&p[0].to_le_bytes());
    out.extend_from_slice(&p[1].to_le_bytes());
    out.extend_from_slice(&c);
}

fn push_instance(out: &mut Vec<u8>, a: [f32; 2], b: [f32; 2], c: [u8; 4], width: f32, start: f32) {
    for v in [a[0], a[1], b[0], b[1]] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&c);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&start.to_le_bytes());
}

/// Lineweight in screen points for a primitive (as on the CPU path), or `None` for a hairline.
pub fn wide_width(lw: f32, lwdisplay: bool) -> Option<f32> {
    if !lwdisplay || lw <= 0.0 {
        return None;
    }
    let w = (lw * 3.78).clamp(1.0, 12.0);
    (w > 1.0).then_some(w)
}

/// `c` with alpha `a`, premultiplied (as the pipelines blend).
fn premultiplied(c: Rgb, a: u8) -> [u8; 4] {
    let k = |v: u8| u8::try_from((u32::from(v) * u32::from(a) + 127) / 255).unwrap_or(v);
    [k(c.0), k(c.1), k(c.2), a]
}

/// Screen detail of the vertex data drawn at one zoom band (see [`build_screen_mesh`]). An object
/// (a top-level entity: a block reference with all its contents) or a primitive whose box is under
/// this many pixels each way at the band's finest zoom is drawn as one dot, as the CPU canvas
/// draws one under a pixel, and the dots falling in one pixel cell as one.
pub const DOT_BELOW_PX: f64 = 1.0;
/// An object under this many pixels each way whose lines and fills cost more than its box is
/// drawn as the box's outline: a block reference's contents, a short text, a small circle.
pub const BOX_BELOW_PX: f64 = 4.0;
/// A dot's size in points (the CPU canvas's sub-pixel dot).
const DOT_POINTS: f32 = 1.0;
/// GPU vertices a dot costs: one quad instance (two triangles).
const DOT_COST: usize = 6;

/// What [`build_screen_mesh`] drew simpler than the list: objects and primitives drawn as dots,
/// the dots drawn for them (one per pixel cell), and objects drawn as their box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Simplified {
    pub dot_objects: usize,
    pub dot_prims: usize,
    pub dots: usize,
    pub boxes: usize,
}

/// Convert a display list into vertex data relative to `origin`, colours resolved against `bg`,
/// with every object and primitive drawn as it is (selection highlights, previews, masks).
pub fn build_mesh(list: &DisplayList, origin: Vec2, bg: Rgb, lwdisplay: bool) -> MeshData {
    build_screen_mesh(list, origin, bg, lwdisplay, None)
}

/// [`build_mesh`] for the screen at a zoom band whose finest zoom shows `pixel` drawing units in
/// a pixel (`None`: everything as it is). Objects and primitives under [`DOT_BELOW_PX`] pixels
/// draw as one dot per pixel cell, small objects under [`BOX_BELOW_PX`] as their box, where that
/// is cheaper than their own lines and fills (see [`Simplified`]). The list itself is untouched:
/// plots, exports, picks, snaps and counts see every object, and nothing is left undrawn (a
/// simplified object still lights its pixels). The band is rebuilt every 2x of zoom, so the detail
/// comes back as the view zooms in.
pub fn build_screen_mesh(list: &DisplayList, origin: Vec2, bg: Rgb, lwdisplay: bool, pixel: Option<f64>) -> MeshData {
    let mut m = MeshData {
        lines: Vec::with_capacity(list.segment_count().saturating_mul(2 * VERTEX_SIZE)),
        tris: Vec::with_capacity(list.tris.len().saturating_mul(VERTEX_SIZE)),
        ..MeshData::default()
    };
    let px = pixel.filter(|p| p.is_finite() && *p > 0.0);
    let mut cells = Cells { pixel: px.unwrap_or(1.0), origin, ..Cells::default() };
    let Some(px) = px else {
        (0..list.prims.len()).for_each(|k| emit_prim(&mut m, &mut cells, list, k, origin, bg, lwdisplay));
        return m;
    };
    let (prims, mut i, mut scratch) = (&list.prims, 0, Vec::new());
    while let Some(first) = prims.get(i) {
        // One top-level object's primitives come one after another under its handle.
        let end = prims.get(i + 1..).and_then(|rest| rest.iter().position(|p| p.handle != first.handle)).map_or(prims.len(), |n| i + 1 + n);
        screen_object(&mut m, &mut cells, &mut scratch, list, i..end, px, bg, lwdisplay);
        i = end;
    }
    cells.flush(&mut m, list, bg, lwdisplay);
    m
}

/// Primitives that can be drawn simpler: lines and fills (masks, images and markers stay).
fn simplifiable(kind: Kind) -> bool {
    matches!(kind, Kind::Polyline | Kind::Tris)
}

/// GPU vertices a primitive costs as it is (as `drawstats` counts them).
fn prim_cost(list: &DisplayList, prim: &cadaddy_render::DPrim, lwdisplay: bool) -> usize {
    let n = list.points(prim).len();
    match prim.kind {
        Kind::Polyline if wide_width(prim.lw, lwdisplay).is_some() => n.saturating_sub(1) * DOT_COST,
        Kind::Polyline => n.saturating_sub(1) * 2,
        Kind::Tris => n,
        _ => DOT_COST,
    }
}

/// The dot width (points) of a primitive: its lineweight's, at least [`DOT_POINTS`].
fn dot_width(prim: &cadaddy_render::DPrim, lwdisplay: bool) -> f32 {
    wide_width(prim.lw, lwdisplay).unwrap_or(DOT_POINTS)
}

/// Primitives costing this many GPU vertices or fewer (a single hairline segment, a lone fill
/// triangle) are drawn as they are even under a pixel: a dot would cost more, so only a crowd of
/// them in one cell could save anything, and finding the crowds would cost more time than it saves.
const CHEAP: usize = 3;

/// The box of `pts` while it is under `limit` each way (`None` once it is not, or with no points).
fn small_box(pts: &[Vec2], limit: f64) -> Option<Bounds2> {
    let (first, rest) = pts.split_first()?;
    let mut b = Bounds2 { min: *first, max: *first };
    for q in rest {
        b.min = Vec2::new(b.min.x.min(q.x), b.min.y.min(q.y));
        b.max = Vec2::new(b.max.x.max(q.x), b.max.y.max(q.y));
        if b.max.x - b.min.x >= limit || b.max.y - b.min.y >= limit {
            return None;
        }
    }
    (first.x.is_finite() && first.y.is_finite()).then_some(b)
}

/// Draw object `run` (primitives of one handle) at a band of `px` drawing units a pixel;
/// `scratch` holds its primitives' boxes under [`BOX_BELOW_PX`] and costs (reused from object to
/// object).
#[allow(clippy::too_many_arguments)]
fn screen_object(
    m: &mut MeshData,
    cells: &mut Cells,
    scratch: &mut Vec<(Option<Bounds2>, usize)>,
    list: &DisplayList,
    run: std::ops::Range<usize>,
    px: f64,
    bg: Rgb,
    lwdisplay: bool,
) {
    let origin = cells.origin;
    let prims = list.prims.get(run.clone()).unwrap_or(&[]);
    let under = |b: &Bounds2, limit: f64| b.width() < px * limit && b.height() < px * limit;
    let boxed = |p: &cadaddy_render::DPrim| if simplifiable(p.kind) { small_box(list.points(p), px * BOX_BELOW_PX) } else { None };
    // Most objects are one primitive bigger than the box limit (a wall, a door swing): drawn as
    // they are with no more work than finding out.
    if let [p] = prims
        && boxed(p).is_none()
    {
        emit_prim(m, cells, list, run.start, origin, bg, lwdisplay);
        return;
    }
    scratch.clear();
    // The object's box while it is small, what it costs, and the primitive with the most
    // geometry, which stands for it (its colour and lineweight). Once the object is too big to
    // be drawn simpler, its primitives are drawn one by one as they come.
    let (mut bb, mut cost) = (Some(Bounds2::EMPTY), 0);
    let mut main: Option<(&cadaddy_render::DPrim, usize)> = None;
    for (k, p) in run.clone().zip(prims) {
        let (b, c) = (boxed(p), prim_cost(list, p, lwdisplay));
        if bb.is_none() {
            screen_prim(m, cells, list, k, b, c, px, bg, lwdisplay);
            continue;
        }
        bb = bb.zip(b).map(|(a, b)| a.union(&b)).filter(|a| under(a, BOX_BELOW_PX));
        cost += c;
        if main.is_none_or(|(_, mc)| c > mc) {
            main = Some((p, c));
        }
        scratch.push((b, c));
        if bb.is_none() {
            for (j, (b, c)) in (run.start..).zip(scratch.drain(..)) {
                screen_prim(m, cells, list, j, b, c, px, bg, lwdisplay);
            }
        }
    }
    let (Some(bb), Some((main, _))) = (bb, main) else { return };
    let col = premultiplied(main.display_rgb(bg), main.alpha());
    if under(&bb, DOT_BELOW_PX) && cost > CHEAP {
        let width = prims.iter().map(|p| dot_width(p, lwdisplay)).fold(DOT_POINTS, f32::max);
        cells.add(bb.center(), run, true, cost, col, width);
        return;
    }
    let wide = wide_width(main.lw, lwdisplay);
    let box_cost = 4 * if wide.is_some() { DOT_COST } else { 2 };
    if cost > box_cost {
        let corners = [bb.min, Vec2::new(bb.max.x, bb.min.y), bb.max, Vec2::new(bb.min.x, bb.max.y), bb.min];
        for side in corners.windows(2) {
            let [a, b] = side else { continue };
            let (a, b) = (rel(*a, origin), rel(*b, origin));
            match wide {
                Some(w) => push_instance(&mut m.quads, a, b, col, w, 0.0),
                None => {
                    push_vertex(&mut m.lines, a, col);
                    push_vertex(&mut m.lines, b, col);
                }
            }
        }
        m.simplified.boxes += 1;
        return;
    }
    for (j, (b, c)) in (run.start..).zip(scratch.drain(..)) {
        screen_prim(m, cells, list, j, b, c, px, bg, lwdisplay);
    }
}

/// Draw primitive `k` (box `b` while under [`BOX_BELOW_PX`], cost `c`) of an object not drawn
/// simpler as a whole: as a dot when it is under a pixel and costs more than a [`CHEAP`] one, else
/// as it is.
#[allow(clippy::too_many_arguments)]
fn screen_prim(m: &mut MeshData, cells: &mut Cells, list: &DisplayList, k: usize, b: Option<Bounds2>, c: usize, px: f64, bg: Rgb, lwdisplay: bool) {
    let small = |b: &Bounds2| c > CHEAP && b.width() < px * DOT_BELOW_PX && b.height() < px * DOT_BELOW_PX;
    if let (Some(b), Some(prim)) = (b.filter(small), list.prims.get(k)) {
        let col = premultiplied(prim.display_rgb(bg), prim.alpha());
        cells.add(b.center(), k..k + 1, false, c, col, dot_width(prim, lwdisplay));
        return;
    }
    let origin = cells.origin;
    emit_prim(m, cells, list, k, origin, bg, lwdisplay);
}

/// A fast hash for pixel cells (a multiply-rotate over the two coordinates).
#[derive(Default)]
struct CellHasher(u64);

impl std::hash::Hasher for CellHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        bytes.iter().for_each(|b| self.write_u64(u64::from(*b)));
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write_i64(&mut self, v: i64) {
        self.write_u64(v as u64);
    }
}

/// Sub-pixel objects and primitives of the current draw-order group by pixel cell (cells of the
/// band's finest pixel from the vertex origin), drawn as one dot a cell when the group ends.
#[derive(Default)]
struct Cells {
    pixel: f64,
    origin: Vec2,
    index: HashMap<(i64, i64), usize, std::hash::BuildHasherDefault<CellHasher>>,
    /// In the order first reached (the vertex data comes out the same every time).
    cells: Vec<Cell>,
}

/// What a cell holds: how many objects and primitives, what drawing them as they are costs,
/// and while that is no more than a dot, which primitives they are (at most [`DOT_COST`] / 2:
/// every line or fill costs two vertices or more, and those costing none draw nothing).
struct Cell {
    at: (i64, i64),
    count: usize,
    objects: usize,
    cost: usize,
    parts: [(u32, u32); 3],
    nparts: usize,
    /// The last one's colour (drawn on top) and the widest lineweight.
    color: [u8; 4],
    width: f32,
}

impl Cells {
    fn add(&mut self, p: Vec2, prims: std::ops::Range<usize>, object: bool, cost: usize, color: [u8; 4], width: f32) {
        // Float to int casts saturate (and NaN is 0): a far point lands in an edge cell.
        let at = (((p.x - self.origin.x) / self.pixel).floor() as i64, ((p.y - self.origin.y) / self.pixel).floor() as i64);
        let part = (u32::try_from(prims.start).unwrap_or(u32::MAX), u32::try_from(prims.end).unwrap_or(u32::MAX));
        let cell = match self.index.get(&at).copied() {
            Some(i) => self.cells.get_mut(i),
            None => {
                self.index.insert(at, self.cells.len());
                self.cells.push(Cell { at, count: 0, objects: 0, cost: 0, parts: [(0, 0); 3], nparts: 0, color, width });
                self.cells.last_mut()
            }
        };
        let Some(c) = cell else { return };
        if let Some(slot) = c.parts.get_mut(c.nparts).filter(|_| cost > 0) {
            *slot = part;
            c.nparts += 1;
        }
        c.count += 1;
        c.objects += usize::from(object);
        c.cost = c.cost.saturating_add(cost);
        c.color = color;
        c.width = c.width.max(width);
    }

    /// Draw the cells: one dot each, or what they hold as it is where that costs no more.
    fn flush(&mut self, m: &mut MeshData, list: &DisplayList, bg: Rgb, lwdisplay: bool) {
        self.index.clear();
        let origin = self.origin;
        for c in std::mem::take(&mut self.cells) {
            if c.cost <= DOT_COST {
                for &(a, b) in c.parts.get(..c.nparts).unwrap_or(&[]) {
                    for k in a as usize..b as usize {
                        emit_prim(m, self, list, k, origin, bg, lwdisplay);
                    }
                }
                continue;
            }
            let centre = |i: i64| (i as f64 + 0.5) * self.pixel;
            let p = [centre(c.at.0) as f32, centre(c.at.1) as f32];
            push_instance(&mut m.quads, p, p, c.color, c.width, 0.0);
            m.simplified.dots += 1;
            m.simplified.dot_objects += c.objects;
            m.simplified.dot_prims += c.count - c.objects;
        }
    }
}

/// Append primitive `k` of `list` as it is. A wipeout mask or an image starts a draw-order group,
/// so the dots of the group before are drawn first.
fn emit_prim(m: &mut MeshData, cells: &mut Cells, list: &DisplayList, k: usize, origin: Vec2, bg: Rgb, lwdisplay: bool) {
    let Some(prim) = list.prims.get(k) else { return };
    if matches!(prim.kind, Kind::InkMask | Kind::Mask | Kind::Image(_)) && !cells.cells.is_empty() {
        cells.flush(m, list, bg, lwdisplay);
    }
    let col = premultiplied(prim.display_rgb(bg), prim.alpha());
    let pts = list.points(prim);
    match prim.kind {
        Kind::Polyline => match wide_width(prim.lw, lwdisplay) {
            Some(w) => {
                for s in pts.windows(2) {
                    if let [a, b] = s {
                        push_instance(&mut m.quads, rel(*a, origin), rel(*b, origin), col, w, 0.0);
                    }
                }
            }
            None => {
                for s in pts.windows(2) {
                    if let [a, b] = s {
                        push_vertex(&mut m.lines, rel(*a, origin), col);
                        push_vertex(&mut m.lines, rel(*b, origin), col);
                    }
                }
            }
        },
        Kind::Tris | Kind::InkMask => {
            if prim.kind == Kind::InkMask {
                m.breaks.push([m.tri_vertices(), m.line_vertices(), m.quad_instances()]);
            }
            for t in pts.as_chunks::<3>().0 {
                for q in t {
                    push_vertex(&mut m.tris, rel(*q, origin), col);
                }
            }
        }
        Kind::Mask => {
            m.breaks.push([m.tri_vertices(), m.line_vertices(), m.quad_instances()]);
            for t in pts.as_chunks::<3>().0 {
                for q in t {
                    push_vertex(&mut m.tris, rel(*q, origin), [bg.0, bg.1, bg.2, 255]);
                }
            }
        }
        Kind::Point => {
            if let Some(q) = pts.first() {
                let p = rel(*q, origin);
                push_instance(&mut m.quads, p, p, col, POINT_SIZE, 0.0);
            }
        }
        // Drawn on the CPU each frame (clipped to the view).
        Kind::Infinite { .. } => {}
        Kind::Image(_) => {
            let Some(img) = list.image(prim) else { return };
            // drawn between the groups before and after it
            m.breaks.push([m.tri_vertices(), m.line_vertices(), m.quad_instances()]);
            let opacity = f32::from(img.alpha) / 255.0 * f32::from(prim.alpha()) / 255.0;
            let mut verts = Vec::with_capacity(pts.len() * IMAGE_VERTEX_SIZE);
            for q in pts.as_chunks::<3>().0.iter().flatten() {
                let (u, v) = img.uv(*q);
                let [x, y] = rel(*q, origin);
                for f in [x, y, u as f32, v as f32, opacity] {
                    verts.extend_from_slice(&f.to_le_bytes());
                }
            }
            m.images.push(MeshImage { raster: img.raster_on(bg), verts, group: m.breaks.len() - 1 });
        }
    }
}

/// Vertex data drawing primitives `prims` of `list` in `color` (the selection highlight), as on
/// the CPU path: lines `width` points wide, and fills filled, or with `dashed` (SELECTIONEFFECT 0;
/// the callback's `dash` dashes the lines) outlined. A wipeout is always outlined, since filling it
/// would hide the drawing; infinite lines are left to the CPU (they are clipped per frame).
pub fn build_highlight_mesh(list: &DisplayList, prims: &[usize], origin: Vec2, color: [u8; 4], width: f32, dashed: bool) -> MeshData {
    let mut m = MeshData::default();
    for prim in prims.iter().filter_map(|&i| list.prims.get(i)) {
        let pts = list.points(prim);
        match prim.kind {
            Kind::Polyline => {
                let mut run = 0.0;
                for s in pts.windows(2) {
                    if let [a, b] = s {
                        push_instance(&mut m.quads, rel(*a, origin), rel(*b, origin), color, width, run as f32);
                        run += a.dist(*b);
                    }
                }
            }
            Kind::Point => {
                if let Some(q) = pts.first() {
                    let p = rel(*q, origin);
                    push_instance(&mut m.quads, p, p, color, 3.0, 0.0);
                }
            }
            Kind::Tris if !dashed => {
                let fill = translucent(color, HIGHLIGHT_FILL_ALPHA);
                for t in pts.as_chunks::<3>().0 {
                    for q in t {
                        push_vertex(&mut m.tris, rel(*q, origin), fill);
                    }
                }
            }
            Kind::Tris | Kind::InkMask | Kind::Mask | Kind::Image(_) => {
                for line in list.fill_outlines(prim) {
                    let mut run = 0.0;
                    for s in line.windows(2) {
                        if let [a, b] = s {
                            push_instance(&mut m.quads, rel(*a, origin), rel(*b, origin), color, width, run as f32);
                            run += a.dist(*b);
                        }
                    }
                }
            }
            Kind::Infinite { .. } => {}
        }
    }
    m
}

/// The vertex origin for a display list seen at `view_center` / `view_height`: the list's centre
/// normally; at deep zoom (the view much smaller than the drawing) a point near the view, snapped
/// to a coarse grid so panning re-uploads only every few dozen screens.
pub fn choose_origin(bounds: &Bounds2, view_center: Vec2, view_height: f64) -> Vec2 {
    let size = bounds.width().max(bounds.height());
    if bounds.is_empty()
        || !view_center.is_finite()
        || view_height.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
        || view_height * 2000.0 >= size
    {
        return if bounds.is_empty() { Vec2::ZERO } else { bounds.center() };
    }
    let step = view_height * 64.0;
    Vec2::new((view_center.x / step).round() * step, (view_center.y / step).round() * step)
}

/// Pending upload shared between the canvas (producer) and the paint callback (consumer).
pub type MeshSlot = Arc<Mutex<Option<MeshData>>>;

/// Per-frame parameters for the paint callback.
pub struct CanvasCallback {
    /// Identifies the mesh in `slot`; the GPU buffers are rebuilt when it changes.
    pub key: u64,
    pub slot: MeshSlot,
    /// (origin − view centre) in world units.
    pub offset: [f32; 2],
    /// Points per world unit.
    pub scale: f32,
    /// Canvas rect centre in points.
    pub center: [f32; 2],
    /// Which buffers to draw: [`LAYER_DRAWING`], [`LAYER_DRAG`], [`LAYER_HIGHLIGHT`] or
    /// [`LAYER_VIEWPORT`].
    pub layer: usize,
    /// Affine transform applied to the origin-relative positions before `offset`: the linear
    /// part (a, b, c, d as in `Mat3`) and the translation. [`IDENTITY_XFORM`] for the drawing.
    pub xform: [f32; 6],
    /// Dash period in points for wide lines (0 = solid): the selection highlight.
    pub dash: f32,
    /// How the draw uses the stencil buffer.
    pub stencil: Stencil,
    /// The view is moving (a pan or a zoom in the last [`crate::dynres::SETTLE_MS`]): the
    /// drawing may be drawn at a lower resolution ([`crate::dynres`]). Set by
    /// [`CanvasCallback::track_motion`].
    pub moving: bool,
}

thread_local! {
    /// Per drawing layer: the view it was last drawn at and when that changed (ms).
    static VIEWS: std::cell::RefCell<std::collections::HashMap<usize, (Vec<u32>, f64)>> = std::cell::RefCell::new(std::collections::HashMap::new());
}

impl CanvasCallback {
    /// Whether this is a drawing's own layer (the drawing, or model space through the active
    /// viewport): the layers drawn at a lower resolution while the view moves. Highlight, drag
    /// previews, the crosshair and every overlay stay at full resolution.
    pub fn reducible(&self) -> bool {
        matches!(self.layer % LAYERS_PER_CANVAS, LAYER_DRAWING | LAYER_VIEWPORT) && self.stencil == Stencil::Off
    }

    /// Note this frame's view of the layer: [`Self::moving`] is set while it changed in the last
    /// settle time, and a frame is asked for when the settle time is over, so the still view is
    /// drawn at full resolution even when nothing else repaints.
    pub fn track_motion(mut self, ctx: &egui::Context) -> Self {
        let now = crate::now_ms();
        let view: Vec<u32> = self.offset.iter().chain(&[self.scale]).chain(&self.center).chain(&self.xform).map(|v| v.to_bits()).collect();
        let since = VIEWS.with(|m| {
            let mut m = m.borrow_mut();
            let e = m.entry(self.layer).or_insert_with(|| (view.clone(), f64::NEG_INFINITY));
            // (A layer's first frame is not a move: its entry starts at this view.)
            if e.0 != view {
                e.1 = now;
                e.0 = view;
            }
            now - e.1
        });
        let settle = crate::dynres::SETTLE_MS;
        self.moving = since < settle;
        if self.moving {
            ctx.request_repaint_after(std::time::Duration::from_millis((settle - since).max(1.0) as u64 + 5));
        } else if self.reducible() && since < settle * 4.0 {
            // The pan is over: what it learnt about steps that didn't pay is forgotten.
            crate::dynres::with(crate::dynres::Controller::still);
        }
        self
    }
}

/// How a [`CanvasCallback`] uses the stencil buffer ([`GpuTarget::stencil`]): a clipped
/// viewport's outline is drawn as a triangle fan that flips a stencil bit ([`Stencil::Mask`],
/// even-odd, as the outline clips on the CPU), then its contents draw only where the bit is set
/// ([`Stencil::Inside`]). The stencil is cleared every frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stencil {
    #[default]
    Off,
    Mask,
    Inside,
}

/// The drawing's display list.
pub const LAYER_DRAWING: usize = 0;
/// The objects a MOVE/COPY/ROTATE/SCALE preview drags, uploaded once per drag.
pub const LAYER_DRAG: usize = 1;
/// The selection highlight, rebuilt only when the selection or the drawing changes.
pub const LAYER_HIGHLIGHT: usize = 2;
/// Model space seen through the active layout viewport, moved to its current view each frame.
pub const LAYER_VIEWPORT: usize = 3;
/// The outline of the active viewport when it is clipped (stencil only, see [`Stencil`]).
pub const LAYER_MASK: usize = 4;
/// Layers one canvas uses; each drawing window (a torn-off tab) has a set of its own.
pub const LAYERS_PER_CANVAS: usize = LAYER_MASK + 1;
/// Drawing windows with GPU layers of their own (the main one included).
pub const MAX_CANVASES: usize = 16;

thread_local! {
    /// The first layer of the canvas being drawn: 0 for the main window, then one set per
    /// torn-off drawing window ([`crate::tearoff`]).
    static LAYER_BASE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Layer `l` ([`LAYER_DRAWING`]…) of the canvas being drawn.
pub fn layer(l: usize) -> usize {
    LAYER_BASE.with(|b| b.get()) + l
}

/// Draw the canvas of window `slot` (0: the main window) while `f` runs.
pub fn with_canvas_slot<R>(slot: usize, f: impl FnOnce() -> R) -> R {
    let before = LAYER_BASE.with(|b| b.replace(slot.min(MAX_CANVASES - 1) * LAYERS_PER_CANVAS));
    let r = f();
    LAYER_BASE.with(|b| b.set(before));
    r
}

/// Opacity of filled areas in the selection and hover highlight: see-through, so the lines and
/// text on (and under) a highlighted hatch stay visible, as the reference app's dot pattern does.
pub const HIGHLIGHT_FILL_ALPHA: f32 = 0.5;

/// `c` at `alpha` opacity, premultiplied (as the pipelines blend).
fn translucent(c: [u8; 4], alpha: f32) -> [u8; 4] {
    let a = alpha.clamp(0.0, 1.0);
    let m = |v: u8| (f32::from(v) * a).round() as u8;
    [m(c[0]), m(c[1]), m(c[2]), m(c[3])]
}
pub const IDENTITY_XFORM: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Bytes of GPU memory alive now (the canvas's, the shaded view's and Walk's buffers and
/// textures, and the canvas's image textures): what `ui.inspect` reports as `perf.gpuBytes`, and
/// what the soak test watches for a leak. Transient readback buffers (screenshots, frame export)
/// are exempt: they are mapped, read and dropped inside one call.
static GPU_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn gpu_bytes() -> u64 {
    GPU_BYTES.load(Ordering::Relaxed)
}

/// A GPU resource with a size to count.
pub(crate) trait Counted {
    fn counted_bytes(&self) -> u64;
}

impl Counted for wgpu::Buffer {
    fn counted_bytes(&self) -> u64 {
        self.size()
    }
}

impl Counted for wgpu::Texture {
    /// Width × height × layers × bytes per pixel × samples (mip levels are not added: every
    /// texture the app keeps has one).
    fn counted_bytes(&self) -> u64 {
        let per_pixel = u64::from(self.format().block_copy_size(None).unwrap_or(4));
        u64::from(self.width()) * u64::from(self.height()) * u64::from(self.depth_or_array_layers()) * per_pixel * u64::from(self.sample_count())
    }
}

/// A GPU buffer or texture counted in [`GPU_BYTES`] while it lives.
pub(crate) struct Tracked<T: Counted = wgpu::Buffer> {
    inner: T,
    bytes: u64,
    counter: &'static AtomicU64,
}

impl<T: Counted> Tracked<T> {
    pub(crate) fn new(inner: T) -> Tracked<T> {
        Tracked::counting(&GPU_BYTES, inner)
    }
    /// Counted in `counter` instead of the app-wide total (tests, which run side by side).
    pub(crate) fn counting(counter: &'static AtomicU64, inner: T) -> Tracked<T> {
        let bytes = inner.counted_bytes();
        counter.fetch_add(bytes, Ordering::Relaxed);
        Tracked { inner, bytes, counter }
    }
}

impl<T: Counted> Drop for Tracked<T> {
    fn drop(&mut self) {
        self.counter.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}

impl<T: Counted> std::ops::Deref for Tracked<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl Tracked<wgpu::Texture> {
    /// A view that keeps the texture (and its count) alive as long as it lives.
    pub(crate) fn into_view(self, desc: &wgpu::TextureViewDescriptor) -> TrackedView {
        TrackedView { view: self.inner.create_view(desc), _texture: self }
    }
}

/// A texture view that owns its counted texture.
pub(crate) struct TrackedView {
    view: wgpu::TextureView,
    _texture: Tracked<wgpu::Texture>,
}

impl std::ops::Deref for TrackedView {
    type Target = wgpu::TextureView;
    fn deref(&self) -> &wgpu::TextureView {
        &self.view
    }
}

#[derive(Clone)]
struct Batch {
    buffer: Arc<Tracked>,
    count: u32,
}

/// One uploaded mesh with its own view uniform (each callback's transform differs).
struct Layer {
    uniform: Tracked,
    bind_group: wgpu::BindGroup,
    key: Option<u64>,
    lines: Vec<Batch>,
    tris: Vec<Batch>,
    quads: Vec<Batch>,
    breaks: Vec<[usize; 3]>,
    images: Vec<LayerImage>,
    /// The layer's last frame as a picture (large layers only, see [`FRAME_CACHE_MIN`]), and
    /// whether this frame shows it instead of drawing the layer again.
    cache: Option<FrameCache>,
    cached: bool,
}

/// A layer's drawing as it was last drawn, in a texture the size of the window: while the mesh
/// and the view stay as they were, the frame shows the texture instead of running every vertex
/// of the drawing again (the pointer moving, a rubber band, a hover, a blinking caret).
struct FrameCache {
    view: TrackedView,
    /// The pass's stencil buffer, when the canvas pipelines name one.
    stencil: Option<TrackedView>,
    group: wgpu::BindGroup,
    /// The texture's size: the window's, or less while the view moves ([`crate::dynres`]).
    size: [u32; 2],
    /// The window's size the picture covers.
    window: [u32; 2],
    /// What the texture shows: the mesh, the view uniform's bytes and the texture's size.
    shows: Option<(u64, Vec<u8>, [u32; 2])>,
    used: web_time::Instant,
}

/// Vertices and instances a layer holds before its frame is kept: below this a frame costs less
/// to draw than the texture costs to keep, and small drawings draw exactly as before.
const FRAME_CACHE_MIN: u64 = 200_000;
/// A kept frame nothing has shown for this long is freed (a closed or hidden drawing's).
const FRAME_CACHE_IDLE: std::time::Duration = std::time::Duration::from_secs(3);

/// Frames a kept layer was drawn again for, and frames shown from the kept picture.
static FRAMES_DRAWN: AtomicU64 = AtomicU64::new(0);
static FRAMES_KEPT: AtomicU64 = AtomicU64::new(0);

/// `(drawn, kept)`: frames of large layers drawn into their kept picture, and frames shown from
/// it unchanged (`ui.inspect`'s `perf.frameCache`).
pub fn frame_cache_counts() -> (u64, u64) {
    (FRAMES_DRAWN.load(Ordering::Relaxed), FRAMES_KEPT.load(Ordering::Relaxed))
}

/// The scale the drawing was last drawn at (f32 bits): 1 at rest ([`crate::dynres`]).
static SHOWN_SCALE: AtomicU32 = AtomicU32::new(0x3f80_0000);
/// The GPU times the drawing's pass (timestamp queries).
static PASS_TIMER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The scale the drawing was last drawn at each way: 1 is full resolution.
pub fn shown_scale() -> f32 {
    f32::from_bits(SHOWN_SCALE.load(Ordering::Relaxed))
}

/// Whether the GPU times the drawing's pass, so the moving scale can follow it.
pub fn pass_timer_on() -> bool {
    PASS_TIMER.load(Ordering::Relaxed)
}

/// Device features the canvas uses when the adapter has them: timestamp queries time the
/// drawing's pass for [`crate::dynres`]. Ask for them when the device is made.
pub fn wanted_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    adapter.features() & wgpu::Features::TIMESTAMP_QUERY
}

/// Once per app frame, after the last frame went to the GPU: read the drawing passes' timings
/// that have arrived, and ask for the ones written last frame.
pub fn read_pass_times(rs: &egui_wgpu::RenderState) {
    if let Some(res) = rs.renderer.read().callback_resources.get::<Resources>()
        && let Some(t) = &res.timer
    {
        t.read(&rs.device, &rs.queue);
    }
}

/// Free the kept frames of layers that are no longer drawn (a closed drawing, a view switched to
/// 3D): called once a frame by the app, since a layer that isn't drawn has no callback to do it.
pub fn free_idle_frames(rs: &egui_wgpu::RenderState) {
    if let Some(res) = rs.renderer.write().callback_resources.get_mut::<Resources>() {
        res.free_idle_frames(web_time::Instant::now());
    }
}

/// `CADADDY_FRAME_CACHE=0` draws every frame directly, as before the cache.
fn frame_cache_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !std::env::var("CADADDY_FRAME_CACHE").is_ok_and(|v| matches!(v.trim(), "0" | "off" | "false" | "no")))
}

struct Resources {
    line_pipeline: wgpu::RenderPipeline,
    tri_pipeline: wgpu::RenderPipeline,
    quad_pipeline: wgpu::RenderPipeline,
    /// Raster images: textured triangles (group 1 holds the texture and sampler).
    image_pipeline: wgpu::RenderPipeline,
    image_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Image textures by their pixels (each keeps its pixels alive, so a key isn't reused while
    /// it is here); dropped when no layer draws them.
    textures: std::collections::HashMap<usize, (Arc<cadaddy_render::images::Raster>, wgpu::BindGroup, Tracked<wgpu::Texture>)>,
    /// With a stencil buffer: the pipelines of [`Stencil::Mask`] and [`Stencil::Inside`].
    clipped: Option<ClipPipelines>,
    /// Shows a layer's kept frame ([`FrameCache`]) in the window's pass.
    blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    /// Shows a smaller kept frame (drawn while the view moved) scaled up, filtered.
    scaled_pipeline: wgpu::RenderPipeline,
    scaled_layout: wgpu::BindGroupLayout,
    /// Times the passes that draw kept frames, when the device has timestamp queries.
    timer: Option<PassTimer>,
    target: GpuTarget,
    /// Vertices a layer holds before its frame is kept ([`FRAME_CACHE_MIN`]; tests lower it).
    frame_cache_min: u64,
    linear_out: bool,
    layers: Vec<Layer>,
    /// For the layers of more drawing windows, made when one first draws.
    layer_layout: wgpu::BindGroupLayout,
}

/// One canvas layer's buffers.
fn new_layer(device: &wgpu::Device, bgl: &wgpu::BindGroupLayout) -> Layer {
    let uniform = Tracked::new(device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("cad_canvas_view"),
        size: UNIFORM_SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    }));
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cad_canvas"),
        layout: bgl,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
    });
    Layer {
        uniform,
        bind_group,
        key: None,
        lines: Vec::new(),
        tris: Vec::new(),
        quads: Vec::new(),
        breaks: Vec::new(),
        images: Vec::new(),
        cache: None,
        cached: false,
    }
}

struct ClipPipelines {
    /// Flips the stencil bit under each triangle; draws no colour.
    mask: wgpu::RenderPipeline,
    line: wgpu::RenderPipeline,
    tri: wgpu::RenderPipeline,
    quad: wgpu::RenderPipeline,
    image: wgpu::RenderPipeline,
}

const UNIFORM_SIZE: u64 = 80;

const SHADER: &str = r#"
struct View {
    a: vec4<f32>, // offset x, offset y (world), scale (device px per unit), 1 = linear output
    b: vec4<f32>, // canvas centre x, y (device px), screen w, h (device px)
    c: vec4<f32>, // pixels per point, dash period in points (0 = solid)
    d: vec4<f32>, // transform linear part: a, b, c, d (x' = a x + c y, y' = b x + d y)
    e: vec4<f32>, // transform translation x, y (world, origin-relative)
};
@group(0) @binding(0) var<uniform> view: View;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    // Device pixels along a wide line from its start (dashing).
    @location(1) along: f32,
};

fn to_px(p: vec2<f32>) -> vec2<f32> {
    let q = vec2<f32>(view.d.x * p.x + view.d.z * p.y, view.d.y * p.x + view.d.w * p.y) + view.e.xy;
    let w = (q + view.a.xy) * view.a.z;
    return vec2<f32>(view.b.x + w.x, view.b.y - w.y);
}

fn px_to_clip(px: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(px.x / view.b.z * 2.0 - 1.0, 1.0 - px.y / view.b.w * 2.0, 0.0, 1.0);
}

// `c` is premultiplied sRGB; a linear target gets it linearised, still premultiplied.
fn out_color(c: vec4<f32>) -> vec4<f32> {
    if (view.a.w > 0.5) {
        let s = c.rgb / max(c.a, 0.0001);
        let lo = s / 12.92;
        let hi = pow((s + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
        return vec4<f32>(select(hi, lo, s <= vec3<f32>(0.04045)) * c.a, c.a);
    }
    return c;
}

@vertex
fn vs_line(@location(0) pos: vec2<f32>, @location(1) color: vec4<f32>) -> VOut {
    var o: VOut;
    o.pos = px_to_clip(to_px(pos));
    o.color = out_color(color);
    o.along = 0.0;
    return o;
}

@vertex
fn vs_quad(
    @builtin(vertex_index) vi: u32,
    @location(0) a: vec2<f32>,
    @location(1) b: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) width: f32,
    @location(4) start: f32,
) -> VOut {
    let pa = to_px(a);
    let pb = to_px(b);
    let d = pb - pa;
    let len = length(d);
    var dir = vec2<f32>(1.0, 0.0);
    if (len > 0.0001) {
        dir = d / len;
    }
    let n = vec2<f32>(-dir.y, dir.x);
    // Width arrives in points.
    let hw = 0.5 * width * view.c.x;
    let t = select(0.0, 1.0, vi == 1u || vi == 4u || vi == 5u);
    let s = select(-1.0, 1.0, vi == 2u || vi == 3u || vi == 5u);
    let along = mix(pa - dir * hw, pb + dir * hw, t);
    var o: VOut;
    o.pos = px_to_clip(along + n * hw * s);
    o.color = out_color(color);
    o.along = start * view.a.z + mix(-hw, len + hw, t);
    return o;
}

@group(1) @binding(0) var image_tex: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;

struct IOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
};

@vertex
fn vs_image(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) alpha: f32) -> IOut {
    var o: IOut;
    o.pos = px_to_clip(to_px(pos));
    o.uv = uv;
    o.alpha = alpha;
    return o;
}

// The texture holds sRGB values with straight alpha: premultiplied here like vertex colours.
@fragment
fn fs_image(v: IOut) -> @location(0) vec4<f32> {
    let c = textureSample(image_tex, image_sampler, v.uv);
    let a = c.a * v.alpha;
    return out_color(vec4<f32>(c.rgb * a, a));
}

@fragment
fn fs_main(v: VOut) -> @location(0) vec4<f32> {
    if (view.c.y > 0.0) {
        // 6 on, 4 off (as the CPU selection highlight's dashes).
        if (fract(v.along / (view.c.y * view.c.x)) > 0.6) {
            discard;
        }
    }
    return v.color;
}
"#;

/// Shows a kept frame: one triangle over the window, each pixel the texture's own (the scissor
/// keeps it to the canvas). The texture has the window's format, so the values pass unchanged.
const BLIT_SHADER: &str = r#"
@group(0) @binding(0) var frame: texture_2d<f32>;

@vertex
fn vs_blit(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_blit(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(frame, vec2<i32>(pos.xy), 0);
}
"#;

/// Shows a kept frame drawn smaller than the window (the view was moving): one triangle over the
/// window, the texture stretched over it and filtered. Premultiplied values filter correctly.
const SCALED_SHADER: &str = r#"
@group(0) @binding(0) var frame: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_scaled(@builtin(vertex_index) i: u32) -> VOut {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var o: VOut;
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

@fragment
fn fs_scaled(v: VOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(frame, smp, v.uv, 0.0);
}
"#;

fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Unorm8x4];
    wgpu::VertexBufferLayout { array_stride: VERTEX_SIZE as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRS }
}

fn image_vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32];
    wgpu::VertexBufferLayout { array_stride: IMAGE_VERTEX_SIZE as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRS }
}

fn instance_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4, 3 => Float32, 4 => Float32];
    wgpu::VertexBufferLayout { array_stride: INSTANCE_SIZE as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &ATTRS }
}

impl Resources {
    fn new(device: &wgpu::Device, target: GpuTarget) -> Self {
        let module =
            device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("cad_canvas"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("cad_canvas"), entries: &[entry(0)] });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cad_canvas"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let layer = || new_layer(device, &bgl);
        // A render pass with a stencil buffer needs every pipeline to name its format; only the
        // clipped-viewport pipelines read or write it.
        let face = |compare, pass_op| wgpu::StencilFaceState {
            compare,
            fail_op: wgpu::StencilOperation::Keep,
            depth_fail_op: wgpu::StencilOperation::Keep,
            pass_op,
        };
        let stencil_state = |s: Stencil| {
            target.stencil.map(|format| {
                let stencil = match s {
                    Stencil::Off => wgpu::StencilState::default(),
                    Stencil::Mask => {
                        let f = face(wgpu::CompareFunction::Always, wgpu::StencilOperation::Invert);
                        wgpu::StencilState { front: f, back: f, read_mask: 1, write_mask: 1 }
                    }
                    Stencil::Inside => {
                        let f = face(wgpu::CompareFunction::Equal, wgpu::StencilOperation::Keep);
                        wgpu::StencilState { front: f, back: f, read_mask: 1, write_mask: 0 }
                    }
                };
                wgpu::DepthStencilState { format, depth_write_enabled: None, depth_compare: None, stencil, bias: wgpu::DepthBiasState::default() }
            })
        };
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cad_image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let textured = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cad_image"),
            bind_group_layouts: &[Some(&bgl), Some(&image_layout)],
            immediate_size: 0,
        });
        let pipeline = |label: &str, vs: &str, topology: wgpu::PrimitiveTopology, buffer: wgpu::VertexBufferLayout<'static>, s: Stencil| {
            let image = vs == "vs_image";
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(if image { &textured } else { &layout }),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vs),
                    buffers: &[Some(buffer)],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState { topology, ..Default::default() },
                depth_stencil: stencil_state(s),
                multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(if image { "fs_image" } else { "fs_main" }),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target.format,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: if s == Stencil::Mask { wgpu::ColorWrites::empty() } else { wgpu::ColorWrites::ALL },
                    })],
                    compilation_options: Default::default(),
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let blit_module = device
            .create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("cad_frame"), source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()) });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cad_frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let blit_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cad_frame"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });
        let blit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cad_frame"),
            layout: Some(&blit_pl),
            vertex: wgpu::VertexState { module: &blit_module, entry_point: Some("vs_blit"), buffers: &[], compilation_options: Default::default() },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: stencil_state(Stencil::Off),
            multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &blit_module,
                entry_point: Some("fs_blit"),
                // The kept frame is premultiplied, as the layer's own fragments are: laid over
                // what is under the canvas it gives what drawing the layer there gives.
                targets: &[Some(wgpu::ColorTargetState {
                    format: target.format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let scaled_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cad_frame_scaled"),
            source: wgpu::ShaderSource::Wgsl(SCALED_SHADER.into()),
        });
        let scaled_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cad_frame_scaled"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let scaled_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cad_frame_scaled"),
            bind_group_layouts: &[Some(&scaled_layout)],
            immediate_size: 0,
        });
        let scaled_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cad_frame_scaled"),
            layout: Some(&scaled_pl),
            vertex: wgpu::VertexState {
                module: &scaled_module,
                entry_point: Some("vs_scaled"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: stencil_state(Stencil::Off),
            multisample: wgpu::MultisampleState { count: 1, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &scaled_module,
                entry_point: Some("fs_scaled"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target.format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let timer = device.features().contains(wgpu::Features::TIMESTAMP_QUERY).then(|| PassTimer::new(device));
        PASS_TIMER.store(timer.is_some(), Ordering::Relaxed);
        use wgpu::PrimitiveTopology::{LineList, TriangleList};
        Resources {
            line_pipeline: pipeline("cad_lines", "vs_line", LineList, vertex_layout(), Stencil::Off),
            tri_pipeline: pipeline("cad_fills", "vs_line", TriangleList, vertex_layout(), Stencil::Off),
            quad_pipeline: pipeline("cad_quads", "vs_quad", TriangleList, instance_layout(), Stencil::Off),
            image_pipeline: pipeline("cad_images", "vs_image", TriangleList, image_vertex_layout(), Stencil::Off),
            clipped: target.stencil.map(|_| ClipPipelines {
                mask: pipeline("cad_clip_mask", "vs_line", TriangleList, vertex_layout(), Stencil::Mask),
                line: pipeline("cad_clip_lines", "vs_line", LineList, vertex_layout(), Stencil::Inside),
                tri: pipeline("cad_clip_fills", "vs_line", TriangleList, vertex_layout(), Stencil::Inside),
                quad: pipeline("cad_clip_quads", "vs_quad", TriangleList, instance_layout(), Stencil::Inside),
                image: pipeline("cad_clip_images", "vs_image", TriangleList, image_vertex_layout(), Stencil::Inside),
            }),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("cad_image"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            image_layout,
            textures: std::collections::HashMap::new(),
            blit_pipeline,
            blit_layout,
            scaled_pipeline,
            scaled_layout,
            timer,
            target,
            frame_cache_min: FRAME_CACHE_MIN,
            linear_out: target.format.is_srgb(),
            layers: (0..=LAYER_MASK).map(|_| layer()).collect(),
            layer_layout: bgl,
        }
    }
}

impl Resources {
    /// Create the textures the layers' images need and drop those no layer draws any more.
    fn sync_textures(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let used: std::collections::HashMap<usize, Arc<cadaddy_render::images::Raster>> =
            self.layers.iter().flat_map(|l| &l.images).map(|im| (Arc::as_ptr(&im.raster) as usize, Arc::clone(&im.raster))).collect();
        self.textures.retain(|k, _| used.contains_key(k));
        let max = device.limits().max_texture_dimension_2d;
        for (key, raster) in used {
            if self.textures.contains_key(&key) {
                continue;
            }
            let fitted = raster.fit(max);
            let size = wgpu::Extent3d { width: fitted.width, height: fitted.height, depth_or_array_layers: 1 };
            let texture = Tracked::new(wgpu::util::DeviceExt::create_texture_with_data(
                device,
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("cad_image"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    // sRGB values read as they are (as vertex colours); `out_color` linearises
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &fitted.rgba,
            ));
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cad_image"),
                layout: &self.image_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            self.textures.insert(key, (raster, group, texture));
        }
    }
}

/// The slowest GPU preparation (uploading a changed drawing) since [`take_slowest_prepare`], in
/// hundredths of a millisecond: it runs while the frame is painted, outside the app's own frame
/// timing, so `ui.inspect` reports it apart.
static SLOWEST_PREPARE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Records how long a `prepare` took when it ends.
struct PrepareTimer(web_time::Instant);

impl Drop for PrepareTimer {
    fn drop(&mut self) {
        let t = (self.0.elapsed().as_secs_f64() * 100_000.0) as u64;
        SLOWEST_PREPARE.fetch_max(t, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The slowest GPU preparation since the last call (ms), and start again.
pub fn take_slowest_prepare() -> f64 {
    SLOWEST_PREPARE.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / 100.0
}

/// The slowest upload of a drawing's vertex buffers made while preparing a frame (the list was
/// converted on the UI thread) since [`take_slowest_upload`], in hundredths of a millisecond:
/// the 2D draw statistics' part of the preparation (`canvas::drawstats`).
static SLOWEST_UPLOAD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The slowest upload in a frame's preparation since the last call (ms), and start again.
pub fn take_slowest_upload() -> f64 {
    SLOWEST_UPLOAD.swap(0, std::sync::atomic::Ordering::Relaxed) as f64 / 100.0
}

/// Vertices (or instances) per uploaded piece: small, so that after an edit only the pieces whose
/// bytes changed are made again ([`upload`]).
const PIECE: usize = 65_536;

/// The pieces the last upload made, by kind and content: an unchanged piece is used again
/// instead of re-created. Every edit used to upload the whole drawing into new buffers; freeing
/// tens of megabytes of them each time stalled the window about 0.35 s every few edits on a
/// 167k-object drawing (UI review 0.7.7c). Cleared when the GPU is set up ([`install`]).
#[allow(clippy::type_complexity)]
static REUSE: Mutex<Option<HashMap<(&'static str, usize, u64), Batch>>> = Mutex::new(None);

/// A piece's content key: its length and two checksums (its halves), so a different piece
/// matching by chance is out of the question in practice.
fn piece_key(label: &'static str, c: &[u8]) -> (&'static str, usize, u64) {
    let (a, b) = c.split_at(c.len() / 2);
    (label, c.len(), (u64::from(crc32fast::hash(a)) << 32) | u64::from(crc32fast::hash(b)))
}

fn upload(device: &wgpu::Device, label: &'static str, bytes: &[u8], stride: usize, usage: wgpu::BufferUsages) -> Vec<Batch> {
    let mut reuse = REUSE.lock().ok();
    let mut previous = reuse.as_mut().and_then(|r| r.as_mut()).map(|m| {
        let keys: Vec<_> = m.keys().filter(|k| k.0 == label).copied().collect();
        keys.into_iter().filter_map(|k| m.remove(&k).map(|b| (k, b))).collect::<HashMap<_, _>>()
    });
    let mut made = Vec::new();
    let batches = bytes
        .chunks(PIECE.min(CHUNK) * stride)
        .filter(|c| c.len() >= stride)
        .map(|c| {
            let key = piece_key(label, c);
            let batch = previous.as_mut().and_then(|p| p.remove(&key)).unwrap_or_else(|| Batch {
                buffer: Arc::new(Tracked::new(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: c,
                    usage,
                }))),
                count: u32::try_from(c.len() / stride).unwrap_or(0),
            });
            made.push((key, batch.clone()));
            batch
        })
        .collect();
    // Keep this upload's pieces for the next; the ones it didn't use are let go.
    if let Some(r) = reuse.as_mut() {
        r.get_or_insert_with(HashMap::new).extend(made);
    }
    batches
}

/// GPU trouble wgpu reported: uncaptured errors (validation, out of memory) and the device being
/// lost (a driver reset or a hung GPU). wgpu's default handler panics on the first uncaptured
/// error, closing the app with any unsaved work (#602); the handlers [`install`] registers record
/// them here instead, and the app autosaves and tells the user (see `CadApp::gpu_fault_tick`).
#[derive(Debug, Default)]
pub struct GpuFaults {
    state: Mutex<FaultState>,
}

#[derive(Debug, Default)]
struct FaultState {
    errors: u64,
    errors_seen: u64,
    lost: bool,
    lost_seen: bool,
    last: String,
}

/// What [`GpuFaults::take`] reports.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuFault {
    /// Uncaptured errors so far (an invalid resource can fail again every frame).
    pub errors: u64,
    /// The device was lost: nothing more can be drawn until the app restarts.
    pub lost: bool,
    /// The latest error or device-lost message.
    pub message: String,
}

impl GpuFaults {
    /// Errors logged in full before only every 1000th is (a repeating error would flood the log).
    const LOGGED: u64 = 10;

    pub fn error(&self, message: String) {
        let Ok(mut s) = self.state.lock() else { return };
        s.errors += 1;
        if s.errors <= Self::LOGGED || s.errors % 1000 == 0 {
            log::error!("wgpu error #{}: {message}", s.errors);
        }
        s.last = message;
    }

    pub fn device_lost(&self, reason: wgpu::DeviceLostReason, message: String) {
        // `Destroyed` is the device being dropped on purpose (the app closing), not a failure.
        if reason == wgpu::DeviceLostReason::Destroyed {
            return;
        }
        log::error!("wgpu device lost ({reason:?}): {message}");
        let Ok(mut s) = self.state.lock() else { return };
        s.lost = true;
        s.last = message;
    }

    /// Faults reported since the last call, if any.
    pub fn take(&self) -> Option<GpuFault> {
        let Ok(mut s) = self.state.lock() else { return None };
        if s.errors == s.errors_seen && s.lost == s.lost_seen {
            return None;
        }
        s.errors_seen = s.errors;
        s.lost_seen = s.lost;
        Some(GpuFault { errors: s.errors, lost: s.lost, message: s.last.clone() })
    }
}

/// Create the canvas pipelines in the renderer's callback resources, and report wgpu errors and
/// device loss to `faults` instead of panicking. Call once with the app's render state (eframe
/// `CreationContext::wgpu_render_state`) and the format of its render pass's stencil buffer,
/// if it has one (eframe `NativeOptions::stencil_buffer`).
pub fn install(rs: &egui_wgpu::RenderState, faults: &Arc<GpuFaults>, stencil: Option<wgpu::TextureFormat>) -> GpuTarget {
    // Pieces kept for reuse belong to the device they were made on.
    if let Ok(mut r) = REUSE.lock() {
        *r = None;
    }
    let f = Arc::clone(faults);
    rs.device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| f.error(e.to_string())));
    let f = Arc::clone(faults);
    rs.device.set_device_lost_callback(move |reason, message| f.device_lost(reason, message));
    let target = GpuTarget { format: rs.target_format, stencil: stencil.filter(|s| s.has_stencil_aspect()) };
    let res = Resources::new(&rs.device, target);
    rs.renderer.write().callback_resources.insert(res);
    log::info!("gpu canvas: target {:?}, stencil {:?}", rs.target_format, target.stencil);
    target
}

impl egui_wgpu::CallbackTrait for CanvasCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let t0 = web_time::Instant::now();
        let _timed = PrepareTimer(t0);
        let Some(shared) = resources.get_mut::<Resources>() else { return Vec::new() };
        // A drawing window's layers are made the first time it draws.
        if self.layer < MAX_CANVASES * LAYERS_PER_CANVAS {
            while shared.layers.len() <= self.layer {
                let l = new_layer(device, &shared.layer_layout);
                shared.layers.push(l);
            }
        }
        let linear_out = shared.linear_out;
        let Some(res) = shared.layers.get_mut(self.layer) else { return Vec::new() };
        if res.key != Some(self.key) {
            let pending = self.slot.lock().ok().and_then(|mut s| s.take());
            if let Some(mut m) = pending {
                if m.uploaded.is_none() {
                    let t = web_time::Instant::now();
                    m.upload_now(device);
                    SLOWEST_UPLOAD.fetch_max((t.elapsed().as_secs_f64() * 100_000.0) as u64, std::sync::atomic::Ordering::Relaxed);
                }
                if let Some(u) = m.uploaded {
                    (res.lines, res.tris, res.quads, res.images) = (u.lines, u.tris, u.quads, u.images);
                }
                res.breaks = m.breaks;
                res.key = Some(self.key);
                shared.sync_textures(device, queue);
            }
        }
        let Some(res) = shared.layers.get_mut(self.layer) else { return Vec::new() };
        let ppp = screen.pixels_per_point;
        let [w, h] = screen.size_in_pixels;
        let a = [self.offset[0], self.offset[1], self.scale * ppp, if linear_out { 1.0 } else { 0.0 }];
        let b = [self.center[0] * ppp, self.center[1] * ppp, w.max(1) as f32, h.max(1) as f32];
        let c = [ppp, self.dash, 0.0, 0.0];
        let [ma, mb, mc, md, tx, ty] = self.xform;
        let d = [ma, mb, mc, md];
        let e = [tx, ty, 0.0, 0.0];
        let bytes: Vec<u8> = a.iter().chain(&b).chain(&c).chain(&d).chain(&e).flat_map(|v| v.to_le_bytes()).collect();
        queue.write_buffer(&res.uniform, 0, &bytes);
        shared.keep_frame(device, encoder, self, [w.max(1), h.max(1)], bytes);
        Vec::new()
    }

    fn paint(&self, info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &egui_wgpu::CallbackResources) {
        let Some(shared) = resources.get::<Resources>() else { return };
        let Some(res) = shared.layers.get(self.layer) else { return };
        if res.key != Some(self.key) {
            return;
        }
        let [sw, sh] = info.screen_size_px;
        if sw == 0 || sh == 0 {
            return;
        }
        // Whole-screen device pixels; egui already set the scissor to the canvas clip rect.
        pass.set_viewport(0.0, 0.0, sw as f32, sh as f32, 0.0, 1.0);
        // The frame kept for this mesh and view (see `keep_frame`): its picture, not the layer.
        if let Some(c) = res.cache.as_ref().filter(|c| res.cached && c.window == [sw, sh]) {
            pass.set_pipeline(if c.size == c.window { &shared.blit_pipeline } else { &shared.scaled_pipeline });
            pass.set_bind_group(0, &c.group, &[]);
            pass.draw(0..3, 0..1);
            return;
        }
        shared.draw(res, self.stencil, pass);
    }
}

impl Resources {
    /// Draw a layer's batches and images in draw order into `pass` (the window's, or a kept
    /// frame's).
    fn draw(&self, res: &Layer, stencil: Stencil, pass: &mut wgpu::RenderPass<'_>) {
        let shared = self;
        pass.set_bind_group(0, &res.bind_group, &[]);
        let (tri, line, quad, image) = match (stencil, &shared.clipped) {
            (Stencil::Off, _) => (&shared.tri_pipeline, &shared.line_pipeline, &shared.quad_pipeline, &shared.image_pipeline),
            // The outline is all triangles.
            (Stencil::Mask, Some(c)) => (&c.mask, &c.mask, &c.mask, &c.mask),
            (Stencil::Inside, Some(c)) => (&c.tri, &c.line, &c.quad, &c.image),
            // No stencil buffer: the canvas never asks for these.
            (_, None) => return,
        };
        if stencil != Stencil::Off {
            pass.set_stencil_reference(1);
        }
        let mask = stencil == Stencil::Mask;
        // Draw-order groups split at each wipeout; without wipeouts there is one group.
        let total = |bs: &[Batch]| bs.iter().map(|b| b.count as usize).sum::<usize>();
        let all = [total(&res.tris), if mask { 0 } else { total(&res.lines) }, if mask { 0 } else { total(&res.quads) }];
        let mut from = [0usize; 3];
        for (group, to) in res.breaks.iter().copied().chain(std::iter::once(all)).enumerate() {
            for (i, (pipeline, batches)) in [(tri, &res.tris), (line, &res.lines), (quad, &res.quads)].into_iter().enumerate() {
                let parts = batch_ranges(batches.iter().map(|b| b.count), from[i]..to[i]);
                if parts.is_empty() {
                    continue;
                }
                pass.set_pipeline(pipeline);
                for (j, r) in parts {
                    let Some(b) = batches.get(j) else { continue };
                    pass.set_vertex_buffer(0, b.buffer.slice(..));
                    // Quads are instanced: six vertices per instance.
                    if i == 2 { pass.draw(0..6, r) } else { pass.draw(r, 0..1) }
                }
            }
            // the images between this group and the next (never into the stencil)
            for im in res.images.iter().filter(|im| im.group == group && !mask) {
                let (Some(b), Some((_, tex, _))) = (&im.vertices, shared.textures.get(&(Arc::as_ptr(&im.raster) as usize))) else { continue };
                pass.set_pipeline(image);
                pass.set_bind_group(1, tex, &[]);
                pass.set_vertex_buffer(0, b.buffer.slice(..));
                pass.draw(0..b.count, 0..1);
            }
            from = to;
        }
    }

    /// Keep the frame of a large layer drawn without the stencil: draw it into the layer's
    /// texture when the mesh, the view or the window's size changed, and leave the texture as it
    /// is otherwise. `paint` then shows the texture. Kept frames nothing shows any more are freed.
    fn keep_frame(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, cb: &CanvasCallback, size: [u32; 2], uniform: Vec<u8>) {
        let now = web_time::Instant::now();
        self.free_idle_frames(now);
        let Some(res) = self.layers.get_mut(cb.layer) else { return };
        res.cached = false;
        let held: u64 = res.lines.iter().chain(&res.tris).chain(&res.quads).map(|b| u64::from(b.count)).sum();
        let max = device.limits().max_texture_dimension_2d;
        if !frame_cache_on() || cb.stencil != Stencil::Off || res.key != Some(cb.key) || held < self.frame_cache_min || size[0] > max || size[1] > max
        {
            res.cache = None;
            return;
        }
        // While the view moves a slow drawing is drawn smaller and shown scaled up.
        let scale = if cb.reducible() { crate::dynres::with(|c| c.scale(cb.moving)).unwrap_or(1.0) } else { 1.0 };
        let tex = crate::dynres::scaled_size(size, scale);
        let scale = if tex == size { 1.0 } else { scale };
        if cb.reducible() {
            SHOWN_SCALE.store(scale.to_bits(), Ordering::Relaxed);
        }
        let mut cache = match res.cache.take() {
            Some(c) if c.size == tex && c.window == size => c,
            _ => self.new_frame_cache(device, tex, size),
        };
        cache.used = now;
        let shows = (cb.key, uniform, tex);
        if cache.shows.as_ref() == Some(&shows) {
            FRAMES_KEPT.fetch_add(1, Ordering::Relaxed);
        } else {
            FRAMES_DRAWN.fetch_add(1, Ordering::Relaxed);
            let stencil = cache.stencil.as_ref().zip(self.target.stencil).map(|(view, format)| wgpu::RenderPassDepthStencilAttachment {
                view,
                depth_ops: format.has_depth_aspect().then_some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: format.has_stencil_aspect().then_some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
            });
            let timed = self.timer.as_ref().and_then(|t| Some((t, t.free_slot()?)));
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cad_frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cache.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: stencil,
                timestamp_writes: timed.map(|(t, i)| wgpu::RenderPassTimestampWrites {
                    query_set: &t.set,
                    beginning_of_pass_write_index: Some(2 * i as u32),
                    end_of_pass_write_index: Some(2 * i as u32 + 1),
                }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(res) = self.layers.get(cb.layer) {
                self.draw(res, Stencil::Off, &mut pass);
            }
            drop(pass);
            if let Some((t, i)) = timed {
                // Timed only when it is the drawing's own picture: the scale is the controller's.
                t.written(encoder, i, if cb.reducible() { Some(scale) } else { None });
            }
            cache.shows = Some(shows);
        }
        if let Some(res) = self.layers.get_mut(cb.layer) {
            res.cache = Some(cache);
            res.cached = true;
        }
    }

    /// Free the kept frames nothing has shown lately.
    fn free_idle_frames(&mut self, now: web_time::Instant) {
        for l in &mut self.layers {
            if l.cache.as_ref().is_some_and(|c| now.duration_since(c.used) > FRAME_CACHE_IDLE) {
                l.cache = None;
                l.cached = false;
            }
        }
    }

    /// A kept frame `size` big covering a `window`-size window (smaller while the view moves).
    fn new_frame_cache(&self, device: &wgpu::Device, size: [u32; 2], window: [u32; 2]) -> FrameCache {
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let texture = |label: &str, format: wgpu::TextureFormat, usage: wgpu::TextureUsages| {
            Tracked::new(device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            }))
            .into_view(&wgpu::TextureViewDescriptor::default())
        };
        let view = texture("cad_frame", self.target.format, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
        // The window's size: shown pixel for pixel; smaller: shown stretched and filtered.
        let group = if size == window {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cad_frame"),
                layout: &self.blit_layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }],
            })
        } else {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cad_frame_scaled"),
                layout: &self.scaled_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            })
        };
        FrameCache {
            view,
            stencil: self.target.stencil.map(|f| texture("cad_frame_stencil", f, wgpu::TextureUsages::RENDER_ATTACHMENT)),
            group,
            size,
            window,
            shows: None,
            used: web_time::Instant::now(),
        }
    }
}

/// Times the passes that draw kept frames with GPU timestamps, for [`crate::dynres`]: each pass
/// writes its start and end into a slot's two queries, resolved and copied into the slot's
/// readback buffer in the same encoder; the next app frame (the commands have gone to the GPU)
/// maps it, and a later one reads it. A slot is reused once read, so at most [`TIMER_SLOTS`]
/// passes are in flight; a pass with no free slot isn't timed.
struct PassTimer {
    set: wgpu::QuerySet,
    resolve: Tracked,
    slots: Vec<TimerSlot>,
}

struct TimerSlot {
    read: Tracked,
    state: Arc<AtomicU8>,
    /// The scale the timed picture was drawn at (f32 bits); NaN: not the drawing's own.
    scale: AtomicU32,
}

const TIMER_SLOTS: usize = 4;
/// Where each slot's queries are resolved in [`PassTimer::resolve`] (the resolve alignment).
const TIMER_STRIDE: u64 = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
const SLOT_FREE: u8 = 0;
const SLOT_WRITTEN: u8 = 1;
const SLOT_MAPPING: u8 = 2;
const SLOT_MAPPED: u8 = 3;
const SLOT_FAILED: u8 = 4;

impl PassTimer {
    fn new(device: &wgpu::Device) -> PassTimer {
        let set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("cad_frame_time"),
            ty: wgpu::QueryType::Timestamp,
            count: 2 * TIMER_SLOTS as u32,
        });
        let resolve = Tracked::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cad_frame_time"),
            size: TIMER_STRIDE * TIMER_SLOTS as u64,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        }));
        let slots = (0..TIMER_SLOTS)
            .map(|_| TimerSlot {
                read: Tracked::new(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("cad_frame_time_read"),
                    size: 16,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })),
                state: Arc::new(AtomicU8::new(SLOT_FREE)),
                scale: AtomicU32::new(f32::NAN.to_bits()),
            })
            .collect();
        PassTimer { set, resolve, slots }
    }

    fn free_slot(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.state.load(Ordering::Acquire) == SLOT_FREE)
    }

    /// Slot `i`'s pass was recorded: resolve its queries and copy them out.
    fn written(&self, encoder: &mut wgpu::CommandEncoder, i: usize, scale: Option<f32>) {
        let Some(slot) = self.slots.get(i) else { return };
        let at = TIMER_STRIDE * i as u64;
        encoder.resolve_query_set(&self.set, 2 * i as u32..2 * i as u32 + 2, &self.resolve, at);
        encoder.copy_buffer_to_buffer(&self.resolve, at, &slot.read, 0, 16);
        slot.scale.store(scale.unwrap_or(f32::NAN).to_bits(), Ordering::Relaxed);
        slot.state.store(SLOT_WRITTEN, Ordering::Release);
    }

    /// Map the slots written last frame; read the mapped ones into the controller.
    fn read(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let _ = device.poll(wgpu::PollType::Poll);
        let period = f64::from(queue.get_timestamp_period());
        for slot in &self.slots {
            match slot.state.load(Ordering::Acquire) {
                SLOT_WRITTEN => {
                    slot.state.store(SLOT_MAPPING, Ordering::Release);
                    let state = Arc::clone(&slot.state);
                    slot.read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                        state.store(if r.is_ok() { SLOT_MAPPED } else { SLOT_FAILED }, Ordering::Release);
                    });
                }
                SLOT_MAPPED => {
                    let ticks = slot.read.slice(..).get_mapped_range().ok().and_then(|r| {
                        let t = |b: Option<&[u8]>| b.and_then(|b| <[u8; 8]>::try_from(b).ok()).map(u64::from_le_bytes);
                        Some((t(r.get(0..8))?, t(r.get(8..16))?))
                    });
                    slot.read.unmap();
                    slot.state.store(SLOT_FREE, Ordering::Release);
                    let scale = f32::from_bits(slot.scale.load(Ordering::Relaxed));
                    if let (Some((start, end)), false) = (ticks, scale.is_nan()) {
                        // A pass ends after it starts; anything else is a reset counter.
                        if end > start {
                            let ms = (end - start) as f64 * period / 1e6;
                            crate::dynres::with(|c| c.sample(ms, scale));
                        }
                    }
                }
                SLOT_FAILED => slot.state.store(SLOT_FREE, Ordering::Release),
                _ => {}
            }
        }
    }
}

/// For batches holding `counts` vertices (or instances) each: the batch indices and the ranges
/// within them that cover `range` of the batches laid end to end.
fn batch_ranges(counts: impl IntoIterator<Item = u32>, range: std::ops::Range<usize>) -> Vec<(usize, std::ops::Range<u32>)> {
    let mut out = Vec::new();
    let mut off = 0usize;
    for (j, n) in counts.into_iter().enumerate() {
        let n = n as usize;
        let (lo, hi) = (range.start.max(off), range.end.min(off + n));
        if lo < hi {
            out.push((j, (lo - off) as u32..(hi - off) as u32));
        }
        off += n;
    }
    out
}

#[cfg(test)]
mod tests {
    /// A counted buffer and a counted texture raise the total by exactly their sizes and give
    /// it back on drop (the textures by width × height × layers × bytes a pixel × samples).
    #[test]
    fn counted_buffers_and_textures_add_and_return_their_bytes() {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        let device = &rs.device;
        assert_eq!(COUNT.load(Ordering::Relaxed), 0);
        let buffer = Tracked::counting(
            &COUNT,
            device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 4096, usage: wgpu::BufferUsages::VERTEX, mapped_at_creation: false }),
        );
        assert_eq!(COUNT.load(Ordering::Relaxed), 4096);
        let texture = Tracked::counting(
            &COUNT,
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: 64, height: 32, depth_or_array_layers: 3 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }),
        );
        assert_eq!(COUNT.load(Ordering::Relaxed), 4096 + 64 * 32 * 3 * 4);
        // A view owns its texture: the count stays while the view lives.
        let view = texture.into_view(&wgpu::TextureViewDescriptor::default());
        assert_eq!(COUNT.load(Ordering::Relaxed), 4096 + 64 * 32 * 3 * 4);
        drop(buffer);
        assert_eq!(COUNT.load(Ordering::Relaxed), 64 * 32 * 3 * 4);
        drop(view);
        assert_eq!(COUNT.load(Ordering::Relaxed), 0);
    }

    use super::*;
    use cadaddy_doc::Handle;
    use cadaddy_render::DPrim;

    #[test]
    fn surface_config_prefers_low_latency_where_asked_and_honours_override() {
        use wgpu::PresentMode::{AutoNoVsync, AutoVsync};
        assert_eq!(surface_config(None, true).present_mode, AutoNoVsync);
        assert_eq!(surface_config(None, false).present_mode, AutoVsync);
        for on in ["1", "on", " TRUE ", "yes"] {
            assert_eq!(surface_config(Some(on), true).present_mode, AutoVsync, "{on}");
        }
        for off in ["0", "Off", "false", "no"] {
            assert_eq!(surface_config(Some(off), false).present_mode, AutoNoVsync, "{off}");
        }
        assert_eq!(surface_config(Some("maybe"), true).present_mode, AutoNoVsync);
        assert_eq!(surface_config(Some(""), false).present_mode, AutoVsync);
        assert_eq!(surface_config(None, true).desired_maximum_frame_latency, Some(1));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn frame_cap_waits_out_the_minimum_interval_only() {
        use std::time::{Duration, Instant};
        let mut cap = FrameCap::new(250); // 4 ms
        let t0 = Instant::now();
        assert_eq!(cap.wait_at(t0), Duration::ZERO, "first frame never waits");
        assert_eq!(cap.wait_at(t0 + Duration::from_millis(1)), Duration::from_millis(3));
        // That frame started at 4 ms; the next one 10 ms later is long past the interval.
        assert_eq!(cap.wait_at(t0 + Duration::from_millis(14)), Duration::ZERO);
        assert_eq!(cap.wait_at(t0 + Duration::from_millis(18)), Duration::ZERO);
        assert_eq!(FrameCap::new(0).min, Duration::from_secs(1));
    }

    fn list() -> DisplayList {
        let mut l = DisplayList {
            verts: vec![Vec2::new(1e6, 1e6), Vec2::new(1e6 + 1.0, 1e6), Vec2::new(1e6 + 1.0, 1e6 + 1.0), Vec2::new(5.0, 5.0)],
            tris: vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)],
            ..Default::default()
        };
        let p = |kind, start, len, lw| DPrim { handle: Handle(1), color: Rgb(255, 255, 255), aci7: true, lw, transparency: 0, kind, start, len };
        l.prims =
            vec![p(Kind::Polyline, 0, 3, 0.0), p(Kind::Tris, 0, 3, 0.0), p(Kind::Point, 3, 1, 0.0), p(Kind::Infinite { ray: false }, 0, 2, 0.0)];
        for v in l.verts.clone() {
            l.bounds.add(v);
        }
        l
    }

    /// Hatch lines, then a filled box in the entity colour (green) of the given kind.
    fn hatch_then_fill(kind: Kind) -> DisplayList {
        let mut l = DisplayList {
            verts: vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0), Vec2::new(1.0, 2.0)],
            tris: vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)],
            ..Default::default()
        };
        let p = |kind, color, start, len| DPrim { handle: Handle(1), color, aci7: false, lw: 0.0, transparency: 0, kind, start, len };
        l.prims = vec![p(Kind::Polyline, Rgb(255, 0, 0), 0, 2), p(Kind::Polyline, Rgb(255, 0, 0), 2, 2), p(kind, Rgb(0, 255, 0), 0, 3)];
        l
    }

    #[test]
    fn coloured_masks_start_a_draw_order_group_and_keep_their_colour() {
        // MTEXT background with a colour, DIMTFILL 2 and a TEXTMASK solid are all `InkMask`: the
        // hatch's four line vertices are drawn before the group break, the fill after it.
        let m = build_mesh(&hatch_then_fill(Kind::InkMask), Vec2::ZERO, Rgb(0, 0, 0), false);
        assert_eq!(m.breaks, vec![[0, 4, 0]], "one break, after the hatch's line vertices");
        assert_eq!(m.line_vertices(), 4);
        assert_eq!(m.tri_vertices(), 3);
        assert_eq!(&m.tris[8..12], &[0, 255, 0, 255], "filled in the entity colour, not the background (black)");
        // A plain fill starts no group (lines of an earlier object would show over it).
        let m = build_mesh(&hatch_then_fill(Kind::Tris), Vec2::ZERO, Rgb(0, 0, 0), false);
        assert!(m.breaks.is_empty());
        // A background mask breaks too, but in the background colour.
        let m = build_mesh(&hatch_then_fill(Kind::Mask), Vec2::ZERO, Rgb(9, 8, 7), false);
        assert_eq!(m.breaks, vec![[0, 4, 0]]);
        assert_eq!(&m.tris[8..12], &[9, 8, 7, 255]);
    }

    #[test]
    fn highlight_dashes_run_on_along_each_polyline() {
        let l = list();
        // The 3-point polyline (two unit segments) and the point marker.
        let m = build_highlight_mesh(&l, &[0, 2], Vec2::new(1e6, 1e6), [1, 2, 3, 255], 1.5, true);
        assert_eq!(m.quad_instances(), 3);
        let start = |i: usize| {
            let o = i * INSTANCE_SIZE + 24;
            f32::from_le_bytes([m.quads[o], m.quads[o + 1], m.quads[o + 2], m.quads[o + 3]])
        };
        assert_eq!([start(0), start(1), start(2)], [0.0, 1.0, 0.0]);
        assert!(m.lines.is_empty() && m.tris.is_empty());
    }

    #[test]
    fn highlight_fills_fills_and_outlines_wipeouts() {
        let mut l = list();
        // The filled triangle (0,0) (1,0) (0,1) is filled in the highlight colour...
        let m = build_highlight_mesh(&l, &[1], Vec2::ZERO, [1, 2, 3, 255], 1.5, false);
        assert_eq!((m.tri_vertices(), m.quad_instances()), (3, 0));
        assert_eq!(&m.tris[8..12], &[1, 1, 2, 128], "in the highlight colour, see-through (premultiplied)");
        // ...or, dashed (SELECTIONEFFECT 0), outlined by its three edges, the dash running on.
        let m = build_highlight_mesh(&l, &[1], Vec2::ZERO, [1, 2, 3, 255], 1.5, true);
        assert_eq!((m.tri_vertices(), m.quad_instances()), (0, 3));
        let start = |i: usize| {
            let o = i * INSTANCE_SIZE + 24;
            f32::from_le_bytes([m.quads[o], m.quads[o + 1], m.quads[o + 2], m.quads[o + 3]])
        };
        let mut runs = [start(0), start(1), start(2)];
        runs.sort_by(f32::total_cmp);
        assert!(runs[0] == 0.0 && runs[1] > 0.9 && runs[2] > runs[1] + 0.9, "{runs:?}");
        // A wipeout would hide the drawing if filled: outlined either way.
        if let Some(p) = l.prims.get_mut(1) {
            p.kind = Kind::Mask;
        }
        let m = build_highlight_mesh(&l, &[1], Vec2::ZERO, [1, 2, 3, 255], 1.5, false);
        assert_eq!((m.tri_vertices(), m.quad_instances()), (0, 3));
    }

    #[test]
    fn rebuilding_dashed_fill_highlights_keeps_the_same_gpu_dash_positions() {
        let l = list();
        let expected = build_highlight_mesh(&l, &[1], Vec2::ZERO, [1, 2, 3, 255], 1.5, true);
        assert_eq!(expected.quad_instances(), 3);
        for _ in 0..64 {
            let rebuilt = build_highlight_mesh(&l, &[1], Vec2::ZERO, [1, 2, 3, 255], 1.5, true);
            assert_eq!(rebuilt.quads, expected.quads, "the same filled triangle must keep its segment positions, directions and dash offsets");
        }
    }

    /// A list of `n` objects (own handles), each a polyline of `pts` points around `at(i)`
    /// spanning `size` drawing units.
    fn objects(n: usize, pts: usize, size: f64, at: impl Fn(usize) -> Vec2) -> DisplayList {
        let mut l = DisplayList::default();
        for i in 0..n {
            let c = at(i);
            let start = l.verts.len() as u32;
            for k in 0..pts {
                let t = k as f64 / pts.max(2) as f64 * std::f64::consts::TAU;
                l.verts.push(c + Vec2::new(t.cos(), t.sin()) * (size / 2.0));
            }
            l.prims.push(DPrim {
                handle: Handle(i as u64 + 1),
                color: Rgb(200, 10, 10),
                aci7: false,
                lw: 0.0,
                transparency: 0,
                kind: Kind::Polyline,
                start,
                len: pts as u32,
            });
        }
        l.bounds = Bounds2::from_points(l.verts.iter().copied());
        l
    }

    /// The pixel cells (of `pixel` from `origin`) the dots of `m` light.
    fn dot_cells(m: &MeshData, pixel: f64) -> std::collections::BTreeSet<(i64, i64)> {
        m.quads
            .chunks(INSTANCE_SIZE)
            .map(|q| {
                let f = |o: usize| f64::from(f32::from_le_bytes([q[o], q[o + 1], q[o + 2], q[o + 3]]));
                ((f(0) / pixel).floor() as i64, (f(4) / pixel).floor() as i64)
            })
            .collect()
    }

    #[test]
    fn sub_pixel_objects_draw_as_one_dot_a_pixel_cell() {
        // 2,000 symbol pieces of a tenth of a pixel, 8 segments each, in a 20 x 20 pixel area.
        let pixel = 0.5;
        let at = |i: usize| Vec2::new((i * 37 % 400) as f64 * 0.025 + 0.01, (i * 91 % 400) as f64 * 0.025 + 0.01);
        let l = objects(2_000, 9, pixel * 0.1, at);
        let full = build_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false);
        assert_eq!(full.line_vertices(), 2_000 * 16, "the full mesh draws every segment");
        let m = build_screen_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false, Some(pixel));
        assert_eq!(m.line_vertices(), 0, "no segment of a sub-pixel object is drawn");
        // Every pixel cell holding an object lights, once.
        let want: std::collections::BTreeSet<_> = (0..2_000).map(at).map(|c| ((c.x / pixel).floor() as i64, (c.y / pixel).floor() as i64)).collect();
        assert_eq!(m.quad_instances(), want.len(), "one dot a cell");
        assert_eq!(dot_cells(&m, pixel), want, "the dots light the cells the objects are in");
        assert_eq!(m.simplified, Simplified { dot_objects: 2_000, dot_prims: 0, dots: want.len(), boxes: 0 });
        assert!(m.bytes() * 10 < full.bytes(), "{} vs {} bytes", m.bytes(), full.bytes());
        // The dot is a pixel (a point), in the object's colour; the same every time.
        let q = &m.quads[..INSTANCE_SIZE];
        assert_eq!(&q[16..20], &[200, 10, 10, 255]);
        assert_eq!(f32::from_le_bytes([q[20], q[21], q[22], q[23]]), DOT_POINTS);
        assert_eq!(build_screen_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false, Some(pixel)).quads, m.quads);
        // At full detail (no pixel) or zoomed in far enough, everything is drawn as it is.
        assert_eq!(build_screen_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false, None).lines, full.lines);
        assert_eq!(build_screen_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false, Some(pixel / 64.0)).lines, full.lines);
    }

    #[test]
    fn sub_pixel_lines_cheaper_than_a_dot_stay_lines() {
        // One segment (2 vertices) under a pixel: drawn as it is, alone or in a crowd.
        for n in [1, 6] {
            let l = objects(n, 2, 0.1, |i| Vec2::new(3.1 + i as f64 * 0.1, 3.5));
            let m = build_screen_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false, Some(1.0));
            assert_eq!((m.line_vertices(), m.quad_instances()), (2 * n, 0));
            assert_eq!(m.lines, build_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false).lines);
        }
        // Two segments (4 vertices) alone in a cell cost less than a dot: still lines. Two of
        // them in one cell are one dot.
        let l = objects(1, 3, 0.1, |_| Vec2::new(3.5, 3.5));
        let m = build_screen_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false, Some(1.0));
        assert_eq!((m.line_vertices(), m.quad_instances()), (4, 0));
        let l = objects(2, 3, 0.1, |i| Vec2::new(3.2 + i as f64 * 0.4, 3.5));
        let m = build_screen_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false, Some(1.0));
        assert_eq!((m.line_vertices(), m.quad_instances()), (0, 1));
        assert_eq!(m.simplified, Simplified { dot_objects: 2, dot_prims: 0, dots: 1, boxes: 0 });
    }

    #[test]
    fn a_small_block_reference_draws_as_its_box_and_a_larger_one_as_it_is() {
        // A symbol: one handle (a block reference and its contents), a circle and a cross.
        let symbol = |size: f64| {
            let mut l = objects(1, 33, size, |_| Vec2::new(10.0, 10.0));
            for (a, b) in [(Vec2::new(-0.5, 0.0), Vec2::new(0.5, 0.0)), (Vec2::new(0.0, -0.5), Vec2::new(0.0, 0.5))] {
                let start = l.verts.len() as u32;
                l.verts.extend([Vec2::new(10.0, 10.0) + a * size, Vec2::new(10.0, 10.0) + b * size]);
                l.prims.push(DPrim {
                    handle: Handle(1),
                    color: Rgb(1, 2, 3),
                    aci7: false,
                    lw: 0.0,
                    transparency: 0,
                    kind: Kind::Polyline,
                    start,
                    len: 2,
                });
            }
            l
        };
        let l = symbol(3.0);
        let m = build_screen_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false, Some(1.0));
        assert_eq!((m.line_vertices(), m.quad_instances()), (8, 0), "four sides of its box");
        assert_eq!(m.simplified.boxes, 1);
        // In the colour of its largest part (the circle), around its extents.
        assert_eq!(&m.lines[8..12], &[200, 10, 10, 255]);
        let x = |i: usize| f32::from_le_bytes([m.lines[i * 12], m.lines[i * 12 + 1], m.lines[i * 12 + 2], m.lines[i * 12 + 3]]);
        let xs: Vec<f32> = (0..8).map(x).collect();
        assert!(xs.iter().all(|&v| (v - 8.5).abs() < 1e-4 || (v - 11.5).abs() < 1e-4), "{xs:?}");
        // Bigger than the box limit: its own lines.
        let l = symbol(BOX_BELOW_PX * 2.0);
        let m = build_screen_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false, Some(1.0));
        assert_eq!(m.lines, build_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false).lines);
        assert_eq!(m.simplified, Simplified::default());
    }

    #[test]
    fn sub_pixel_pieces_of_a_large_object_draw_as_dots() {
        // A long text line two pixels tall: its strokes are under a pixel, the line is not.
        let mut l = objects(400, 5, 0.6, |i| Vec2::new(i as f64 * 0.25, 1.2 + (i % 2) as f64));
        for p in &mut l.prims {
            p.handle = Handle(7);
        }
        let m = build_screen_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false, Some(1.0));
        assert_eq!(m.simplified.dot_prims, 400);
        assert_eq!(m.simplified.dot_objects + m.simplified.boxes, 0);
        assert_eq!(m.line_vertices(), 0);
        // 100 pixels long, two rows of cells.
        assert_eq!(m.quad_instances(), 200);
    }

    #[test]
    fn dots_stay_in_their_draw_order_group_and_masks_markers_stay() {
        // Tiny objects, a wipeout over them, tiny objects on top, then a point marker.
        let mut l = objects(20, 9, 0.1, |i| Vec2::new(i as f64 * 2.0, 0.0));
        let tail = objects(20, 9, 0.1, |i| Vec2::new(i as f64 * 2.0, 5.0));
        l.tris = vec![Vec2::new(-1.0, -1.0), Vec2::new(40.0, -1.0), Vec2::new(-1.0, 10.0)];
        l.prims.push(DPrim { handle: Handle(100), color: Rgb(0, 0, 0), aci7: false, lw: 0.0, transparency: 0, kind: Kind::Mask, start: 0, len: 3 });
        let base = l.verts.len() as u32;
        l.verts.extend(tail.verts.iter().copied());
        l.prims.extend(tail.prims.iter().map(|p| DPrim { handle: Handle(p.handle.0 + 200), start: p.start + base, ..*p }));
        l.verts.push(Vec2::new(0.05, 0.05));
        let at = l.verts.len() as u32 - 1;
        l.prims.push(DPrim { handle: Handle(999), color: Rgb(9, 9, 9), aci7: false, lw: 0.0, transparency: 0, kind: Kind::Point, start: at, len: 1 });
        let m = build_screen_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false, Some(1.0));
        assert_eq!(m.breaks, vec![[0, 0, 20]], "the 20 dots under the wipeout come before it");
        assert_eq!(m.tri_vertices(), 3, "the mask is drawn");
        assert_eq!(m.quad_instances(), 41, "20 + 20 dots and the point marker");
    }

    #[test]
    fn a_screen_mesh_never_costs_more_than_the_full_one() {
        // Objects of every size and detail, some sharing handles (block contents), at three bands.
        let mut seed = 7u64;
        let mut next = move || {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as f64 / f64::from(1u32 << 31)
        };
        let mut l = DisplayList::default();
        for i in 0..3_000u64 {
            let (c, size, pts) = (Vec2::new(next() * 200.0, next() * 200.0), 0.02 + next() * next() * 12.0, 2 + (next() * 18.0) as usize);
            let start = l.verts.len() as u32;
            for k in 0..pts {
                let t = k as f64 / pts as f64 * std::f64::consts::TAU;
                l.verts.push(c + Vec2::new(t.cos(), t.sin()) * size);
            }
            let lw = if i % 5 == 0 { 0.7 } else { 0.0 };
            l.prims.push(DPrim {
                handle: Handle(i / 3),
                color: Rgb(5, 6, 7),
                aci7: false,
                lw,
                transparency: 0,
                kind: Kind::Polyline,
                start,
                len: pts as u32,
            });
        }
        let cost = |m: &MeshData| m.line_vertices() + m.tri_vertices() + 6 * m.quad_instances();
        for lw in [false, true] {
            let full = build_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), lw);
            for pixel in [0.05, 0.5, 2.0, 20.0] {
                let m = build_screen_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), lw, Some(pixel));
                assert!(cost(&m) <= cost(&full), "pixel {pixel}: {} > {}", cost(&m), cost(&full));
                assert!(m.bytes() <= full.bytes(), "pixel {pixel}: {} > {} bytes", m.bytes(), full.bytes());
            }
        }
    }

    #[test]
    fn mesh_counts_and_precision() {
        let l = list();
        let origin = Vec2::new(1e6, 1e6);
        let m = build_mesh(&l, origin, Rgb(255, 255, 255), false);
        assert_eq!(m.line_vertices(), 4, "two segments");
        assert_eq!(m.tri_vertices(), 3);
        assert_eq!(m.quad_instances(), 1, "the point marker");
        // First vertex is stored relative to the origin, so it's exactly zero.
        assert_eq!(&m.lines[0..8], &[0u8; 8]);
        // Colour 7 on a white background inverts to black (display_rgb).
        assert_eq!(&m.lines[8..12], &[0, 0, 0, 255]);
        let x = f32::from_le_bytes([m.lines[12], m.lines[13], m.lines[14], m.lines[15]]);
        assert_eq!(x, 1.0);
    }

    #[test]
    fn transparent_objects_get_premultiplied_alpha() {
        let mut l = list();
        for p in &mut l.prims {
            p.transparency = 50;
        }
        // Colour 7 on a black background is white: half of it, premultiplied.
        let m = build_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false);
        assert_eq!(&m.lines[8..12], &[128, 128, 128, 128]);
        assert_eq!(&m.tris[8..12], &[128, 128, 128, 128]);
        // A wipeout's mask stays opaque.
        if let Some(p) = l.prims.get_mut(1) {
            p.kind = Kind::Mask;
        }
        let m = build_mesh(&l, Vec2::ZERO, Rgb(10, 20, 30), false);
        assert_eq!(&m.tris[8..12], &[10, 20, 30, 255]);
        assert_eq!(premultiplied(Rgb(255, 0, 0), 255), [255, 0, 0, 255], "opaque colours are unchanged");
    }

    #[test]
    fn lineweights_become_quads() {
        let mut l = list();
        if let Some(p) = l.prims.first_mut() {
            p.lw = 0.7;
        }
        let m = build_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), true);
        assert_eq!(m.line_vertices(), 0);
        assert_eq!(m.quad_instances(), 3);
        assert_eq!(wide_width(0.25, true), None, "thin lineweights stay hairlines");
        assert_eq!(wide_width(0.7, false), None);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn shader_is_valid_for_webgpu_and_webgl2() {
        let module = naga::front::wgsl::parse_str(SHADER).unwrap();
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).unwrap();
        for (stage, entry) in
            [(naga::ShaderStage::Vertex, "vs_line"), (naga::ShaderStage::Vertex, "vs_quad"), (naga::ShaderStage::Fragment, "fs_main")]
        {
            let options = naga::back::glsl::Options { version: naga::back::glsl::Version::new_gles(300), ..Default::default() };
            let pipeline = naga::back::glsl::PipelineOptions { shader_stage: stage, entry_point: entry.into(), multiview: None };
            let mut out = String::new();
            let mut w =
                naga::back::glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default()).unwrap();
            w.write().unwrap();
            assert!(out.contains("#version 300 es"), "{entry}");
        }
    }

    /// A large layer's kept frame is the picture drawing the layer gives, over whatever is under
    /// the canvas; a frame with the same mesh and view is shown from it without drawing again;
    /// a zoom or a new mesh draws again.
    #[test]
    fn a_kept_frame_shows_what_drawing_the_layer_shows() {
        use egui_wgpu::CallbackTrait;
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        let (device, queue) = (&rs.device, &rs.queue);
        let (w, h) = (64u32, 64u32);
        let format = wgpu::TextureFormat::Rgba8Unorm;
        // A red fill, a half-transparent green fill over part of it, and a white polyline.
        let mesh = || {
            let mut l = DisplayList {
                verts: vec![Vec2::new(2.0, 2.0), Vec2::new(60.0, 50.0), Vec2::new(10.0, 60.0)],
                tris: vec![
                    Vec2::new(5.0, 5.0),
                    Vec2::new(45.0, 5.0),
                    Vec2::new(5.0, 45.0),
                    Vec2::new(20.0, 10.0),
                    Vec2::new(60.0, 10.0),
                    Vec2::new(60.0, 40.0),
                ],
                ..Default::default()
            };
            let p = |kind, color, start, len, transparency| DPrim { handle: Handle(1), color, aci7: false, lw: 0.0, transparency, kind, start, len };
            l.prims =
                vec![p(Kind::Tris, Rgb(255, 0, 0), 0, 3, 0), p(Kind::Tris, Rgb(0, 255, 0), 3, 3, 50), p(Kind::Polyline, Rgb(255, 255, 255), 0, 3, 0)];
            for v in l.verts.iter().chain(&l.tris).copied().collect::<Vec<_>>() {
                l.bounds.add(v);
            }
            build_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false)
        };
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
        // A window: its resources, and one frame of it at a mesh key and zoom, read back.
        let window = |min: u64| {
            let mut r = egui_wgpu::CallbackResources::default();
            let mut res = Resources::new(device, GpuTarget { format, stencil: None });
            res.frame_cache_min = min;
            r.insert(res);
            r
        };
        // Whether the frames below are of a moving view (dynamic resolution).
        let moving = std::cell::Cell::new(false);
        let frame = |r: &mut egui_wgpu::CallbackResources, key: u64, scale: f32| -> Vec<u8> {
            let slot: MeshSlot = Arc::new(Mutex::new(Some(mesh())));
            let cb = CanvasCallback {
                key,
                slot,
                offset: [-32.0, -32.0],
                scale,
                center: [32.0, 32.0],
                layer: LAYER_DRAWING,
                xform: IDENTITY_XFORM,
                dash: 0.0,
                stencil: Stencil::Off,
                moving: moving.get(),
            };
            let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [w, h], pixels_per_point: 1.0 };
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            cb.prepare(device, queue, &screen, &mut enc, r);
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32));
            {
                let mut pass = enc
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &target_view,
                            depth_slice: None,
                            resolve_target: None,
                            // What is under the canvas: not black, not the mesh's colours.
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.2, g: 0.3, b: 0.4, a: 1.0 }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    })
                    .forget_lifetime();
                cb.paint(egui::PaintCallbackInfo { viewport: rect, clip_rect: rect, pixels_per_point: 1.0, screen_size_px: [w, h] }, &mut pass, r);
            }
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: u64::from(w * 4 * h),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            enc.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            queue.submit([enc.finish()]);
            buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(10)) }).unwrap();
            buffer.slice(..).get_mapped_range().unwrap().to_vec()
        };
        // Largest channel difference, and pixels that differ at all.
        let differ = |a: &[u8], b: &[u8]| {
            let worst = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
            (worst, a.chunks(4).zip(b.chunks(4)).filter(|(x, y)| x != y).count())
        };
        let at = |img: &[u8], x: u32, y: u32| {
            let i = (y * w + x) as usize * 4;
            [img[i], img[i + 1], img[i + 2]]
        };
        // Drawn directly, as before: nothing kept.
        let mut direct = window(u64::MAX);
        let (d0, k0) = frame_cache_counts();
        let plain = frame(&mut direct, 7, 1.0);
        assert_eq!(frame_cache_counts(), (d0, k0), "a layer under the size to keep draws directly");
        assert_eq!(at(&plain, 8, 40), [255, 0, 0], "the red fill");
        let under = at(&plain, 1, 1);
        assert!(under != [0, 0, 0] && under != [255, 0, 0], "what is under the canvas shows where nothing is drawn");
        // Kept: the first frame draws the layer into its picture.
        let mut kept = window(0);
        let first = frame(&mut kept, 7, 1.0);
        assert_eq!(frame_cache_counts(), (d0 + 1, k0));
        let (worst, n) = differ(&first, &plain);
        // Opaque pixels are the same values; a half-transparent one is blended in two steps
        // instead of one, which can round the other way by one.
        assert!(worst <= 1, "a kept frame differs from the drawn one by {worst} in {n} pixels");
        assert_eq!(at(&first, 8, 40), [255, 0, 0]);
        assert_eq!(at(&first, 1, 1), under, "the kept frame is laid over what is under it");
        // The same frame again (the pointer moved): shown from the picture, not drawn.
        for i in 1..=3 {
            let again = frame(&mut kept, 7, 1.0);
            assert_eq!(frame_cache_counts(), (d0 + 1, k0 + i));
            assert_eq!(differ(&again, &first), (0, 0));
        }
        // A zoom draws again, and shows what drawing at that zoom shows.
        let zoomed = frame(&mut kept, 7, 2.0);
        assert_eq!(frame_cache_counts(), (d0 + 2, k0 + 3));
        assert!(differ(&zoomed, &first).1 > 100, "the zoomed picture is another picture");
        assert!(differ(&zoomed, &frame(&mut direct, 7, 2.0)).0 <= 1);
        // A new mesh (an edit) draws again.
        let _ = frame(&mut kept, 8, 2.0);
        assert_eq!(frame_cache_counts(), (d0 + 3, k0 + 3));
        // A layer no longer drawn (its drawing closed, the view gone to 3D) gives its picture up.
        let res = kept.get_mut::<Resources>().unwrap();
        assert!(res.layers[LAYER_DRAWING].cache.is_some());
        res.free_idle_frames(web_time::Instant::now() + FRAME_CACHE_IDLE + std::time::Duration::from_secs(1));
        assert!(res.layers[LAYER_DRAWING].cache.is_none());

        // Dynamic resolution. A moving view at half scale: the drawing is drawn into a picture a
        // quarter the pixels and stretched over the window.
        let kept_size = |r: &egui_wgpu::CallbackResources| {
            r.get::<Resources>().and_then(|res| res.layers[LAYER_DRAWING].cache.as_ref().map(|c| (c.size, c.window)))
        };
        crate::dynres::with(|c| c.configure(true, 30, Some(0.5)));
        moving.set(true);
        let small = frame(&mut kept, 9, 1.0);
        assert_eq!(kept_size(&kept), Some(([32, 32], [64, 64])), "drawn at half resolution each way");
        assert_eq!(shown_scale(), 0.5);
        assert_eq!(at(&small, 12, 44), [255, 0, 0], "inside the red fill it is still red");
        assert!(differ(&small, &first).1 > 0, "the edges are the smaller picture's");
        // The view stops: drawn once at full resolution, exactly today's picture.
        moving.set(false);
        let rest = frame(&mut kept, 9, 1.0);
        assert_eq!(kept_size(&kept), Some(([64, 64], [64, 64])));
        assert_eq!(shown_scale(), 1.0);
        assert_eq!(differ(&rest, &first), (0, 0), "at rest the picture is the full-resolution one");
        // Off (DYNRESMODE 0): a moving view is drawn at full resolution.
        crate::dynres::with(|c| c.configure(false, 30, Some(0.5)));
        moving.set(true);
        let off = frame(&mut kept, 10, 1.0);
        assert_eq!(kept_size(&kept), Some(([64, 64], [64, 64])));
        assert_eq!(differ(&off, &first), (0, 0));
        // A screenshot waiting: full resolution too.
        crate::dynres::with(|c| {
            c.configure(true, 30, Some(0.5));
            c.full = true;
        });
        let _ = frame(&mut kept, 11, 1.0);
        assert_eq!(kept_size(&kept), Some(([64, 64], [64, 64])));
        crate::dynres::with(|c| {
            c.full = false;
            c.configure(true, 30, None);
        });
    }

    #[test]
    fn frame_shader_is_valid_for_webgpu_and_webgl2() {
        for (source, vs, fs) in [(BLIT_SHADER, "vs_blit", "fs_blit"), (SCALED_SHADER, "vs_scaled", "fs_scaled")] {
            frame_shader_valid(source, vs, fs);
        }
    }

    fn frame_shader_valid(source: &str, vs: &str, fs: &str) {
        let module = naga::front::wgsl::parse_str(source).unwrap();
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).unwrap();
        for (stage, entry) in [(naga::ShaderStage::Vertex, vs), (naga::ShaderStage::Fragment, fs)] {
            let options = naga::back::glsl::Options { version: naga::back::glsl::Version::new_gles(300), ..Default::default() };
            let pipeline = naga::back::glsl::PipelineOptions { shader_stage: stage, entry_point: entry.into(), multiview: None };
            let mut out = String::new();
            let mut w =
                naga::back::glsl::Writer::new(&mut out, &module, &info, &options, &pipeline, naga::proc::BoundsCheckPolicies::default()).unwrap();
            w.write().unwrap();
            assert!(out.contains("#version 300 es"), "{entry}");
        }
    }

    #[test]
    fn origin_follows_deep_zoom() {
        let b = Bounds2::new(Vec2::new(0.0, 0.0), Vec2::new(1e6, 1e6));
        assert_eq!(choose_origin(&b, Vec2::new(10.0, 10.0), 1e5), b.center());
        let o = choose_origin(&b, Vec2::new(123_456.7, 654_321.2), 1.0);
        assert!((o.x - 123_456.7).abs() <= 32.0 && (o.y - 654_321.2).abs() <= 32.0);
        assert_eq!(choose_origin(&Bounds2::EMPTY, Vec2::new(1.0, 1.0), 1.0), Vec2::ZERO);
        assert_eq!(choose_origin(&b, Vec2::new(f64::NAN, 0.0), 1.0), b.center());
    }

    #[test]
    fn images_draw_between_draw_order_groups() {
        let mut l = list();
        let raster = Arc::new(cadaddy_render::images::Raster::new(1, 1, vec![9, 9, 9, 255]).unwrap());
        // the fill becomes an image over the unit square (top-left corner at 0,1)
        l.images.push(cadaddy_render::ImageRef {
            path: "x".into(),
            raster,
            origin: Vec2::new(0.0, 1.0),
            ex: Vec2::new(1.0, 0.0),
            ey: Vec2::new(0.0, -1.0),
            alpha: 255,
            dark_stencil: false,
        });
        if let Some(p) = l.prims.get_mut(1) {
            p.kind = Kind::Image(0);
        }
        let m = build_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false);
        // after the polyline's two segments, before the point marker
        assert_eq!(m.breaks, vec![[0, 4, 0]]);
        assert_eq!(m.images.len(), 1);
        assert_eq!(m.images[0].group, 0);
        assert_eq!(m.images[0].verts.len(), 3 * IMAGE_VERTEX_SIZE);
        // the vertex at (1, 0) is the image's bottom-right corner: u 1, v 1
        let f = |i: usize| f32::from_le_bytes(m.images[0].verts[i * 4..i * 4 + 4].try_into().unwrap());
        assert_eq!([f(5), f(6), f(7), f(8), f(9)], [1.0, 0.0, 1.0, 1.0, 1.0]);
        assert_eq!(m.tri_vertices(), 0, "no plain fill");
    }

    #[test]
    fn wipeout_masks_start_a_draw_order_group() {
        let mut l = list();
        if let Some(p) = l.prims.get_mut(1) {
            p.kind = Kind::Mask;
        }
        let bg = Rgb(10, 20, 30);
        let m = build_mesh(&l, Vec2::ZERO, bg, false);
        // The polyline's two segments come before the mask; the point marker after it.
        assert_eq!(m.breaks, vec![[0, 4, 0]]);
        assert_eq!(m.tri_vertices(), 3);
        assert_eq!(&m.tris[8..12], &[10, 20, 30, 255], "the mask takes the background colour");
        assert_eq!(m.quad_instances(), 1);
    }

    #[test]
    fn batch_ranges_cover_a_range_across_batches() {
        assert_eq!(batch_ranges([4, 4, 4], 2..9), vec![(0, 2..4), (1, 0..4), (2, 0..1)]);
        assert_eq!(batch_ranges([4, 4], 4..8), vec![(1, 0..4)]);
        assert!(batch_ranges([4, 4], 3..3).is_empty());
        assert!(batch_ranges([], 0..5).is_empty());
    }
}
