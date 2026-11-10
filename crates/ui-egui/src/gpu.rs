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
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// The streams cut into area tiles ([`build_mesh_tiled`]): each stream holds its tiles one
    /// after the other, and a frame draws only the tiles in view. `None`: drawn whole, in the
    /// order of `breaks`.
    pub tiles: Option<Arc<Tiles>>,
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
    tiles: Option<Arc<Tiles>>,
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
        // A tiled mesh's buffers start at its tiles' starts, so an edit makes again only the
        // pieces of the tiles it changed; its pieces are kept apart from the other layers'.
        let uploaded = match &self.tiles {
            Some(t) => Uploaded {
                tris: upload_parts(device, "cad_tile_fills", t.parts(&self.tris, 0, VERTEX_SIZE), VERTEX_SIZE, usage),
                lines: upload_parts(device, "cad_tile_lines", t.parts(&self.lines, 1, VERTEX_SIZE), VERTEX_SIZE, usage),
                quads: upload_parts(device, "cad_tile_quads", t.parts(&self.quads, 2, INSTANCE_SIZE), INSTANCE_SIZE, usage),
                images,
                tiles: Some(Arc::clone(t)),
            },
            None => Uploaded {
                lines: upload(device, "cad_lines", &self.lines, VERTEX_SIZE, usage),
                tris: upload(device, "cad_fills", &self.tris, VERTEX_SIZE, usage),
                quads: upload(device, "cad_quads", &self.quads, INSTANCE_SIZE, usage),
                images,
                tiles: None,
            },
        };
        self.uploaded = Some(uploaded);
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

/// The vertex data of one primitive (not an image, not an infinite line) appended to the
/// stream it belongs in: fills to `tris`, hairlines to `lines`, wide lines and points to `quads`.
#[allow(clippy::too_many_arguments)]
fn push_prim(list: &DisplayList, prim: &cadaddy_render::DPrim, origin: Vec2, bg: Rgb, lwdisplay: bool, lines: &mut Vec<u8>, tris: &mut Vec<u8>, quads: &mut Vec<u8>) {
    let col = premultiplied(prim.display_rgb(bg), prim.alpha());
    let pts = list.points(prim);
    match prim.kind {
        Kind::Polyline => match wide_width(prim.lw, lwdisplay) {
            Some(w) => {
                for s in pts.windows(2) {
                    if let [a, b] = s {
                        push_instance(quads, rel(*a, origin), rel(*b, origin), col, w, 0.0);
                    }
                }
            }
            None => {
                for s in pts.windows(2) {
                    if let [a, b] = s {
                        push_vertex(lines, rel(*a, origin), col);
                        push_vertex(lines, rel(*b, origin), col);
                    }
                }
            }
        },
        Kind::Tris | Kind::InkMask | Kind::Mask => {
            // A wipeout's mask takes the background colour.
            let col = if prim.kind == Kind::Mask { [bg.0, bg.1, bg.2, 255] } else { col };
            for t in pts.as_chunks::<3>().0 {
                for q in t {
                    push_vertex(tris, rel(*q, origin), col);
                }
            }
        }
        Kind::Point => {
            if let Some(q) = pts.first() {
                let p = rel(*q, origin);
                push_instance(quads, p, p, col, POINT_SIZE, 0.0);
            }
        }
        Kind::Infinite { .. } | Kind::Image(_) => {}
    }
}

/// An image primitive's textured triangles, painted after draw-order group `group`.
fn mesh_image(list: &DisplayList, prim: &cadaddy_render::DPrim, origin: Vec2, bg: Rgb, group: usize) -> Option<MeshImage> {
    let img = list.image(prim)?;
    let pts = list.points(prim);
    let opacity = f32::from(img.alpha) / 255.0 * f32::from(prim.alpha()) / 255.0;
    let mut verts = Vec::with_capacity(pts.len() * IMAGE_VERTEX_SIZE);
    for q in pts.as_chunks::<3>().0.iter().flatten() {
        let (u, v) = img.uv(*q);
        let [x, y] = rel(*q, origin);
        for f in [x, y, u as f32, v as f32, opacity] {
            verts.extend_from_slice(&f.to_le_bytes());
        }
    }
    Some(MeshImage { raster: img.raster_on(bg), verts, group })
}

/// Convert a display list into vertex data relative to `origin`, colours resolved against `bg`.
pub fn build_mesh(list: &DisplayList, origin: Vec2, bg: Rgb, lwdisplay: bool) -> MeshData {
    let mut m = MeshData {
        lines: Vec::with_capacity(list.segment_count().saturating_mul(2 * VERTEX_SIZE)),
        tris: Vec::with_capacity(list.tris.len().saturating_mul(VERTEX_SIZE)),
        ..MeshData::default()
    };
    for prim in &list.prims {
        match prim.kind {
            // Drawn on the CPU each frame (clipped to the view).
            Kind::Infinite { .. } => {}
            Kind::Image(_) => {
                if list.image(prim).is_none() {
                    continue;
                }
                // drawn between the groups before and after it
                m.breaks.push([m.tri_vertices(), m.line_vertices(), m.quad_instances()]);
                m.images.extend(mesh_image(list, prim, origin, bg, m.breaks.len() - 1));
            }
            _ => {
                if matches!(prim.kind, Kind::InkMask | Kind::Mask) {
                    m.breaks.push([m.tri_vertices(), m.line_vertices(), m.quad_instances()]);
                }
                push_prim(list, prim, origin, bg, lwdisplay, &mut m.lines, &mut m.tris, &mut m.quads);
            }
        }
    }
    m
}

// ---------- area tiles (Q6472) ----------
//
// A large drawing's vertex data is cut by place: a grid of square cells over the list, each cell
// a tile, and one tile more for primitives larger than a cell. Each stream (fills, lines, quads)
// holds the tiles one after the other, and a frame draws only the tiles whose primitives reach
// into the view: zoomed in on one room of a print set, the GPU runs the vertices around the room,
// not every vertex of the set.
//
// The picture must not change, and it depends on draw order wherever two primitives light the
// same pixel. Within one stream of one draw-order group (see `MeshData::breaks`) each primitive
// gets a layer: at least its tile's last one, and above every bucket (a tile's run of primitives
// in one layer) of another tile it comes within `Tiling::margin` of. The buckets are drawn by
// layer, then tile, each in list order: every pair of primitives that may share a pixel is drawn
// in the order of the list, and the rest don't affect each other. A primitive of a cell reaches
// at most half a cell past it, so only the neighbouring cells (and the large tile) are looked at.

/// How [`build_mesh_tiled`] cuts a list into area tiles: square cells of side `cell` from
/// `min`, `cols` × `rows` of them (world units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tiling {
    pub min: Vec2,
    pub cell: f64,
    pub cols: usize,
    pub rows: usize,
    /// Primitives closer than this (world units) may light the same pixel where the list is
    /// drawn: their draw order is kept, whatever tiles they are in.
    pub margin: f64,
}

/// Vertices a tile holds on average: fewer, larger tiles below that (each is a draw call).
const TILE_VERTICES: usize = 32_768;
/// Most cells.
const MAX_TILES: usize = 4096;
/// A tile is at least this many pixels across at the coarsest zoom its list is drawn at, so a
/// view meets a few tiles, and two tiles' primitives seldom come within the margin of each other.
const TILE_MIN_PX: f64 = 1024.0;
/// Pixels within which two primitives may light the same pixel: half the widest line (12
/// points, at up to 4 pixels a point) and the rounding of a pixel on each side.
const ORDER_MARGIN_PX: f64 = 28.0;
/// A primitive whose neighbourhood covers more cells than this is placed above everything of the
/// other tiles in its group (looking at every cell would cost more than it saves).
const MAX_NEIGHBOUR_CELLS: usize = 64;

impl Tiling {
    /// The tiling for `list` drawn at up to `px` drawing units per pixel; `None` (one tile,
    /// drawn as before) for a list too small to be worth cutting, or with nothing to cut.
    pub fn choose(list: &DisplayList, px: f64) -> Option<Tiling> {
        let vertices = list.segment_count().saturating_mul(2).saturating_add(list.tris.len());
        Tiling::for_bounds(&list.bounds, vertices, px)
    }

    fn for_bounds(b: &Bounds2, vertices: usize, px: f64) -> Option<Tiling> {
        let want = (vertices / TILE_VERTICES).min(MAX_TILES);
        if want < 2 || b.is_empty() || !b.min.is_finite() || !b.max.is_finite() || !(px.is_finite() && px > 0.0) {
            return None;
        }
        let (w, h) = (b.width(), b.height());
        let floor = TILE_MIN_PX * px;
        if !(w.is_finite() && h.is_finite() && floor.is_finite()) {
            return None;
        }
        let mut cell = (w.max(floor) * h.max(floor) / want as f64).sqrt().max(floor);
        let count = |cell: f64| ((w / cell) as usize).saturating_add(1);
        let (mut cols, mut rows) = (count(cell), (h / cell) as usize + 1);
        // A long, thin drawing: never more cells than the most.
        for _ in 0..64 {
            if cols.saturating_mul(rows) <= MAX_TILES {
                break;
            }
            cell *= 1.5;
            (cols, rows) = (count(cell), ((h / cell) as usize).saturating_add(1));
        }
        let cells = cols.saturating_mul(rows);
        (cell.is_finite() && cell > 0.0 && (2..=MAX_TILES).contains(&cells)).then_some(Tiling { min: b.min, cell, cols, rows, margin: ORDER_MARGIN_PX * px })
    }

    fn cells(&self) -> usize {
        self.cols * self.rows
    }

    /// The column and row of the cell holding `p` (the nearest one, outside the grid).
    fn cell_xy(&self, p: Vec2) -> (usize, usize) {
        let at = |v: f64, n: usize| {
            let i = (v / self.cell).floor();
            if i.is_nan() || i < 0.0 { 0 } else { (i as usize).min(n.saturating_sub(1)) }
        };
        (at(p.x - self.min.x, self.cols), at(p.y - self.min.y, self.rows))
    }
}

/// The area tiles of a mesh ([`build_mesh_tiled`]): where each is, and the order its parts are
/// drawn in. The last tile holds the primitives larger than a cell.
#[derive(Debug, Default)]
pub struct Tiles {
    /// The bounds of each tile's primitives (world units).
    pub bounds: Vec<Bounds2>,
    /// Primitives in each tile.
    pub prims: Vec<usize>,
    /// Where each tile starts in each stream (fills, lines, quads: vertices or instances), and
    /// one entry more where the last ends.
    pub starts: Vec<[usize; 3]>,
    /// What is drawn, in order: runs of a stream, each of one tile.
    pub steps: Vec<Step>,
    /// The steps of each draw-order group, per stream (fills, lines, quads).
    pub groups: Vec<[std::ops::Range<usize>; 3]>,
}

/// A run of a stream (vertices or instances `from..to`) of one tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub tile: u32,
    pub from: usize,
    pub to: usize,
}

/// What a frame draws of a tiled mesh: tiles, primitives and GPU vertices (six a quad).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TilesDrawn {
    pub tiles: usize,
    pub prims: usize,
    pub vertices: usize,
}

impl Tiles {
    /// Which tiles reach into `view` (world units, grown by the half width of the widest line).
    pub fn visible(&self, view: &Bounds2) -> Vec<bool> {
        self.bounds.iter().map(|b| b.intersects(view)).collect()
    }

    /// What drawing the tiles `visible` says draws.
    pub fn drawn(&self, visible: &[bool]) -> TilesDrawn {
        let mut d = TilesDrawn::default();
        for (t, w) in self.starts.windows(2).enumerate() {
            if let ([a, b], true, Some(&p)) = (w, visible.get(t).copied().unwrap_or(true), self.prims.get(t)) {
                d.tiles += 1;
                d.prims += p;
                d.vertices += (b[0] - a[0]) + (b[1] - a[1]) + 6 * (b[2] - a[2]);
            }
        }
        d
    }

    /// Stream `class`'s bytes (`stride` a vertex), cut where each tile starts.
    fn parts<'a>(&'a self, bytes: &'a [u8], class: usize, stride: usize) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.starts.windows(2).filter_map(move |w| match w {
            [a, b] => bytes.get(a.get(class)? * stride..b.get(class)? * stride),
            _ => None,
        })
    }
}

/// One tile's run of one stream in one layer while the mesh is built.
struct Bucket {
    group: u32,
    class: u8,
    layer: u32,
    tile: u32,
    /// Vertices within the tile's stream.
    from: usize,
    to: usize,
    bounds: Bounds2,
}

/// A tile's buckets in one stream of the current draw-order group, layers rising.
#[derive(Default)]
struct Lane {
    group: Option<u32>,
    top: u32,
    open: Vec<usize>,
}

impl Lane {
    fn top(&self, group: u32) -> u32 {
        if self.group == Some(group) { self.top } else { 0 }
    }

    /// The lowest layer from `layer` up that draws after every bucket of this lane (in `group`)
    /// coming near `probe`.
    fn above(&self, buckets: &[Bucket], group: u32, probe: &Bounds2, layer: u32) -> u32 {
        if self.group != Some(group) {
            return layer;
        }
        for b in self.open.iter().rev().filter_map(|&i| buckets.get(i)) {
            if b.layer < layer {
                break;
            }
            if b.bounds.intersects(probe) {
                return b.layer + 1;
            }
        }
        layer
    }
}

/// The highest layer of the current group's lanes in one stream with its tile, and the highest
/// of the other tiles: together, the highest layer of any tile but a given one.
#[derive(Clone, Copy, Default)]
struct Tops {
    first: Option<(u32, usize)>,
    second: Option<u32>,
}

impl Tops {
    fn other_than(&self, tile: usize) -> Option<u32> {
        match self.first {
            Some((f, t)) if t != tile => Some(f),
            _ => self.second,
        }
    }
    fn raise(&mut self, tile: usize, layer: u32) {
        match self.first {
            None => self.first = Some((layer, tile)),
            Some((f, t)) if t == tile => self.first = Some((f.max(layer), t)),
            Some((f, _)) if layer > f => (self.first, self.second) = (Some((layer, tile)), Some(f)),
            Some(_) => self.second = Some(self.second.map_or(layer, |s| s.max(layer))),
        }
    }
}

/// [`build_mesh`] cut into the area tiles of `tiling` (`None`: one tile, as [`build_mesh`]).
/// Drawing every tile draws exactly what [`build_mesh`]'s data draws, and leaving out tiles
/// that don't reach into the view changes nothing in it.
pub fn build_mesh_tiled(list: &DisplayList, origin: Vec2, bg: Rgb, lwdisplay: bool, tiling: Option<&Tiling>) -> MeshData {
    let Some(t) = tiling else { return build_mesh(list, origin, bg, lwdisplay) };
    let large = t.cells();
    let n_tiles = large + 1;
    let mut m = MeshData::default();
    // Each primitive's tile (`u32::MAX`: none).
    let mut placed: Vec<u32> = vec![u32::MAX; list.prims.len()];
    let mut counts = vec![[0usize; 3]; n_tiles];
    let mut prims_in = vec![0usize; n_tiles];
    let mut bounds = vec![Bounds2::EMPTY; n_tiles];
    let mut buckets: Vec<Bucket> = Vec::new();
    let mut lanes: Vec<[Lane; 3]> = (0..n_tiles).map(|_| Default::default()).collect();
    let mut tops = [Tops::default(); 3];
    let mut group = 0u32;
    let mut totals = [0usize; 3];
    for (i, prim) in list.prims.iter().enumerate() {
        let pts = list.points(prim);
        let (class, n) = match prim.kind {
            Kind::Infinite { .. } => continue,
            Kind::Image(_) => {
                if list.image(prim).is_none() {
                    continue;
                }
                m.breaks.push(totals);
                (group, tops) = (group + 1, [Tops::default(); 3]);
                m.images.extend(mesh_image(list, prim, origin, bg, m.breaks.len() - 1));
                continue;
            }
            Kind::Tris | Kind::InkMask | Kind::Mask => {
                if prim.kind != Kind::Tris {
                    m.breaks.push(totals);
                    (group, tops) = (group + 1, [Tops::default(); 3]);
                }
                (0usize, pts.as_chunks::<3>().0.len() * 3)
            }
            Kind::Polyline => match wide_width(prim.lw, lwdisplay) {
                Some(_) => (2, pts.len().saturating_sub(1)),
                None => (1, 2 * pts.len().saturating_sub(1)),
            },
            Kind::Point => (2, usize::from(!pts.is_empty())),
        };
        if n == 0 {
            continue;
        }
        let bb = Bounds2::from_points(pts.iter().copied());
        // A primitive's cell holds its centre, and it reaches at most half a cell past the cell
        // (the neighbours looked at below rely on it); any other goes in the large tile.
        let (x, y) = t.cell_xy(bb.center());
        let home = Bounds2::new(t.min + Vec2::new(x as f64, y as f64) * t.cell, t.min + Vec2::new(x as f64 + 1.0, y as f64 + 1.0) * t.cell);
        let tile = if !bb.is_empty() && home.expand(t.cell * 0.5).contains_box(&bb) { y * t.cols + x } else { large };
        // Its layer: above whatever of another tile may share a pixel with it.
        let probe = bb.expand(t.margin);
        let Some(top) = tops.get(class).copied() else { continue };
        let mut layer = lanes.get(tile).and_then(|l| l.get(class)).map_or(0, |l| l.top(group));
        if !probe.is_empty() {
            let reach = probe.expand(t.cell * 0.5);
            let ((x0, y0), (x1, y1)) = (t.cell_xy(reach.min), t.cell_xy(reach.max));
            if (x1 - x0 + 1) * (y1 - y0 + 1) > MAX_NEIGHBOUR_CELLS {
                if let Some(other) = top.other_than(tile) {
                    layer = layer.max(other + 1);
                }
            } else {
                for c in (y0..=y1).flat_map(|y| (x0..=x1).map(move |x| y * t.cols + x)).chain(std::iter::once(large)) {
                    if c == tile || !bounds.get(c).is_some_and(|b| b.intersects(&probe)) {
                        continue;
                    }
                    if let Some(lane) = lanes.get(c).and_then(|l| l.get(class)) {
                        layer = lane.above(&buckets, group, &probe, layer);
                    }
                }
            }
        }
        let Some(lane) = lanes.get_mut(tile).and_then(|l| l.get_mut(class)) else { continue };
        if lane.group != Some(group) {
            (lane.group, lane.top) = (Some(group), 0);
            lane.open.clear();
        }
        let Some(count) = counts.get_mut(tile).and_then(|c| c.get_mut(class)) else { continue };
        let from = *count;
        match lane.open.last().and_then(|&b| buckets.get_mut(b)).filter(|b| b.layer == layer) {
            Some(b) => {
                b.to = from + n;
                b.bounds = b.bounds.union(&bb);
            }
            None => {
                lane.open.push(buckets.len());
                buckets.push(Bucket { group, class: class as u8, layer, tile: tile as u32, from, to: from + n, bounds: bb });
            }
        }
        lane.top = layer;
        if let Some(top) = tops.get_mut(class) {
            top.raise(tile, layer);
        }
        *count += n;
        if let (Some(p), Some(b), Some(slot), Some(total)) = (prims_in.get_mut(tile), bounds.get_mut(tile), placed.get_mut(i), totals.get_mut(class)) {
            *p += 1;
            *b = b.union(&bb);
            *slot = tile as u32;
            *total += n;
        }
    }
    // Where each tile starts in each stream.
    let mut starts = Vec::with_capacity(n_tiles + 1);
    let mut at = [0usize; 3];
    for c in &counts {
        starts.push(at);
        for (a, n) in at.iter_mut().zip(c) {
            *a += n;
        }
    }
    starts.push(at);
    // Each tile's primitives in list order (a counting sort), written tile after tile.
    let mut first = vec![0usize; n_tiles + 1];
    for &tile in &placed {
        if let Some(f) = (tile as usize).checked_add(1).and_then(|k| first.get_mut(k)) {
            *f += 1;
        }
    }
    for k in 1..first.len() {
        let before = first.get(k - 1).copied().unwrap_or(0);
        if let Some(f) = first.get_mut(k) {
            *f += before;
        }
    }
    let mut order = vec![0u32; first.last().copied().unwrap_or(0)];
    let mut next = first.clone();
    for (i, &tile) in placed.iter().enumerate() {
        if let Some(slot) = next.get_mut(tile as usize).and_then(|n| {
            let k = *n;
            *n += 1;
            order.get_mut(k)
        }) {
            *slot = i as u32;
        }
    }
    m.tris = Vec::with_capacity(at[0] * VERTEX_SIZE);
    m.lines = Vec::with_capacity(at[1] * VERTEX_SIZE);
    m.quads = Vec::with_capacity(at[2] * INSTANCE_SIZE);
    for &i in &order {
        if let Some(prim) = list.prims.get(i as usize) {
            push_prim(list, prim, origin, bg, lwdisplay, &mut m.lines, &mut m.tris, &mut m.quads);
        }
    }
    // The buckets in draw order: by group, stream, layer, then tile.
    buckets.sort_by_key(|b| (b.group, b.class, b.layer, b.tile));
    let mut groups = vec![[0..0, 0..0, 0..0]; m.breaks.len() + 1];
    let mut steps = Vec::with_capacity(buckets.len());
    for (k, b) in buckets.iter().enumerate() {
        let base = starts.get(b.tile as usize).and_then(|s| s.get(usize::from(b.class))).copied().unwrap_or(0);
        steps.push(Step { tile: b.tile, from: base + b.from, to: base + b.to });
        if let Some(r) = groups.get_mut(b.group as usize).and_then(|g| g.get_mut(usize::from(b.class))) {
            if r.start == r.end {
                *r = k..k + 1;
            } else {
                r.end = k + 1;
            }
        }
    }
    m.tiles = Some(Arc::new(Tiles { bounds, prims: prims_in, starts, steps, groups }));
    m
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
    /// The part of the drawing on screen (world units, grown by the half width of the widest
    /// line): a tiled mesh draws only the tiles reaching into it. `None`: all of it.
    pub cull: Option<Bounds2>,
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
    /// The mesh's area tiles, and where each batch of the fills, lines and quads starts in its
    /// stream (one entry more for the end), when the mesh is tiled.
    tiles: Option<Arc<Tiles>>,
    offsets: [Vec<usize>; 3],
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
    size: [u32; 2],
    /// What the texture shows: the mesh and the view uniform's bytes.
    shows: Option<(u64, Vec<u8>)>,
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
        tiles: None,
        offsets: Default::default(),
        cache: None,
        cached: false,
    }
}

/// Where each of `batches` starts when they are laid end to end, and where the last ends.
fn batch_offsets(batches: &[Batch]) -> Vec<usize> {
    let mut at = 0usize;
    let mut out = Vec::with_capacity(batches.len() + 1);
    out.push(0);
    for b in batches {
        at += b.count as usize;
        out.push(at);
    }
    out
}

/// [`batch_ranges`] from the batches' starts (`offsets`, see [`batch_offsets`]), found by
/// bisection: a tiled layer draws many short runs.
fn batch_ranges_at(offsets: &[usize], range: std::ops::Range<usize>, out: &mut Vec<(usize, std::ops::Range<u32>)>) {
    out.clear();
    let mut j = offsets.partition_point(|&o| o <= range.start).saturating_sub(1);
    while let (Some(&lo), Some(&hi)) = (offsets.get(j), offsets.get(j + 1)) {
        if lo >= range.end {
            break;
        }
        let (a, b) = (range.start.max(lo), range.end.min(hi));
        if a < b {
            out.push((j, (a - lo) as u32..(b - lo) as u32));
        }
        j += 1;
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

/// Pieces uploads made and pieces they used again (see [`upload_pieces`]).
static PIECES_MADE: AtomicU64 = AtomicU64::new(0);
static PIECES_REUSED: AtomicU64 = AtomicU64::new(0);

/// `(made, reused)`: the GPU buffer pieces vertex uploads have made, and the ones they found
/// unchanged from the upload before and used again, since the app started (measurements take
/// the difference across an edit).
pub fn upload_pieces() -> (u64, u64) {
    (PIECES_MADE.load(Ordering::Relaxed), PIECES_REUSED.load(Ordering::Relaxed))
}

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
    upload_parts(device, label, std::iter::once(bytes), stride, usage)
}

/// [`upload`] of a stream in parts (a tiled mesh's tiles): each part is cut into pieces of its
/// own, so a part's pieces are the same whatever the parts before it hold.
fn upload_parts<'a>(
    device: &wgpu::Device,
    label: &'static str,
    parts: impl IntoIterator<Item = &'a [u8]>,
    stride: usize,
    usage: wgpu::BufferUsages,
) -> Vec<Batch> {
    let mut reuse = REUSE.lock().ok();
    let mut previous = reuse.as_mut().and_then(|r| r.as_mut()).map(|m| {
        let keys: Vec<_> = m.keys().filter(|k| k.0 == label).copied().collect();
        keys.into_iter().filter_map(|k| m.remove(&k).map(|b| (k, b))).collect::<HashMap<_, _>>()
    });
    let mut made = Vec::new();
    let batches = parts
        .into_iter()
        .flat_map(|part| part.chunks(PIECE.min(CHUNK) * stride))
        .filter(|c| c.len() >= stride)
        .map(|c| {
            let key = piece_key(label, c);
            let old = previous.as_mut().and_then(|p| p.remove(&key));
            (if old.is_some() { &PIECES_REUSED } else { &PIECES_MADE }).fetch_add(1, Ordering::Relaxed);
            let batch = old.unwrap_or_else(|| Batch {
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
                    (res.lines, res.tris, res.quads, res.images, res.tiles) = (u.lines, u.tris, u.quads, u.images, u.tiles);
                    res.offsets = [batch_offsets(&res.tris), batch_offsets(&res.lines), batch_offsets(&res.quads)];
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
        // What a kept frame shows: the view, and the tiles drawn in it.
        let mut shows = bytes;
        for v in self.cull.iter().flat_map(|c| [c.min.x, c.min.y, c.max.x, c.max.y]) {
            shows.extend_from_slice(&v.to_le_bytes());
        }
        shared.keep_frame(device, encoder, self, [w.max(1), h.max(1)], shows);
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
        if let Some(c) = res.cache.as_ref().filter(|c| res.cached && c.size == [sw, sh]) {
            pass.set_pipeline(&shared.blit_pipeline);
            pass.set_bind_group(0, &c.group, &[]);
            pass.draw(0..3, 0..1);
            return;
        }
        shared.draw(res, self.stencil, self.cull.as_ref(), pass);
    }
}

impl Resources {
    /// Draw a layer's batches and images in draw order into `pass` (the window's, or a kept
    /// frame's); of a tiled layer, only the tiles reaching into `cull`.
    fn draw(&self, res: &Layer, stencil: Stencil, cull: Option<&Bounds2>, pass: &mut wgpu::RenderPass<'_>) {
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
        if let Some(tiles) = &res.tiles {
            self.draw_tiles(res, tiles, [tri, line, quad, image], mask, cull, pass);
            return;
        }
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

    /// Draw a tiled layer: each draw-order group's fills, lines and quads, the runs of the tiles
    /// reaching into `cull` in order (runs that follow each other in a stream drawn as one), then
    /// the images after the group.
    fn draw_tiles(&self, res: &Layer, tiles: &Tiles, pipelines: [&wgpu::RenderPipeline; 4], mask: bool, cull: Option<&Bounds2>, pass: &mut wgpu::RenderPass<'_>) {
        let visible = cull.map(|c| tiles.visible(c));
        let shown = |t: u32| visible.as_ref().is_none_or(|v| v.get(t as usize).copied().unwrap_or(true));
        let streams = [(&res.tris, &res.offsets[0]), (&res.lines, &res.offsets[1]), (&res.quads, &res.offsets[2])];
        let mut parts = Vec::new();
        for (group, ranges) in tiles.groups.iter().enumerate() {
            for (class, ((batches, offsets), range)) in streams.iter().zip(ranges).enumerate() {
                if mask && class != 0 {
                    continue;
                }
                let Some(&pipeline) = pipelines.get(class) else { continue };
                let mut set = false;
                let mut draw = |run: std::ops::Range<usize>, pass: &mut wgpu::RenderPass<'_>| {
                    batch_ranges_at(offsets, run, &mut parts);
                    for (j, r) in parts.drain(..) {
                        let Some(b) = batches.get(j) else { continue };
                        if !set {
                            pass.set_pipeline(pipeline);
                            set = true;
                        }
                        pass.set_vertex_buffer(0, b.buffer.slice(..));
                        // Quads are instanced: six vertices per instance.
                        if class == 2 { pass.draw(0..6, r) } else { pass.draw(r, 0..1) }
                    }
                };
                let mut run: Option<std::ops::Range<usize>> = None;
                for s in tiles.steps.get(range.clone()).unwrap_or(&[]).iter().filter(|s| shown(s.tile)) {
                    match &mut run {
                        Some(r) if r.end == s.from => r.end = s.to,
                        _ => {
                            if let Some(r) = run.replace(s.from..s.to) {
                                draw(r, pass);
                            }
                        }
                    }
                }
                if let Some(r) = run {
                    draw(r, pass);
                }
            }
            // the images between this group and the next (never into the stencil)
            for im in res.images.iter().filter(|im| im.group == group && !mask) {
                let (Some(b), Some((_, tex, _))) = (&im.vertices, self.textures.get(&(Arc::as_ptr(&im.raster) as usize))) else { continue };
                pass.set_pipeline(pipelines[3]);
                pass.set_bind_group(1, tex, &[]);
                pass.set_vertex_buffer(0, b.buffer.slice(..));
                pass.draw(0..b.count, 0..1);
            }
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
        let mut cache = match res.cache.take() {
            Some(c) if c.size == size => c,
            _ => self.new_frame_cache(device, size),
        };
        cache.used = now;
        let shows = (cb.key, uniform);
        if cache.shows.as_ref() == Some(&shows) {
            FRAMES_KEPT.fetch_add(1, Ordering::Relaxed);
        } else {
            FRAMES_DRAWN.fetch_add(1, Ordering::Relaxed);
            let stencil = cache.stencil.as_ref().zip(self.target.stencil).map(|(view, format)| wgpu::RenderPassDepthStencilAttachment {
                view,
                depth_ops: format.has_depth_aspect().then_some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: format.has_stencil_aspect().then_some(wgpu::Operations { load: wgpu::LoadOp::Clear(0), store: wgpu::StoreOp::Discard }),
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cad_frame"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &cache.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: stencil,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(res) = self.layers.get(cb.layer) {
                self.draw(res, Stencil::Off, cb.cull.as_ref(), &mut pass);
            }
            drop(pass);
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

    fn new_frame_cache(&self, device: &wgpu::Device, size: [u32; 2]) -> FrameCache {
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
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cad_frame"),
            layout: &self.blit_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }],
        });
        FrameCache {
            view,
            stencil: self.target.stencil.map(|f| texture("cad_frame_stencil", f, wgpu::TextureUsages::RENDER_ATTACHMENT)),
            group,
            size,
            shows: None,
            used: web_time::Instant::now(),
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
                cull: None,
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
    }

    #[test]
    fn frame_shader_is_valid_for_webgpu_and_webgl2() {
        let module = naga::front::wgsl::parse_str(BLIT_SHADER).unwrap();
        let info = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).unwrap();
        for (stage, entry) in [(naga::ShaderStage::Vertex, "vs_blit"), (naga::ShaderStage::Fragment, "fs_blit")] {
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

    #[test]
    fn bisected_batch_ranges_are_the_scanned_ones() {
        let counts = [4u32, 0, 7, 1, 9, 3];
        let offsets = {
            let mut at = 0;
            let mut v = vec![0];
            for c in counts {
                at += c as usize;
                v.push(at);
            }
            v
        };
        let mut out = Vec::new();
        for a in 0..26 {
            for b in a..26 {
                batch_ranges_at(&offsets, a..b, &mut out);
                assert_eq!(out, batch_ranges(counts, a..b), "{a}..{b}");
            }
        }
    }

    // ---------- area tiles (Q6472) ----------

    /// A small deterministic random source (xorshift).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn f(&mut self, span: f64) -> f64 {
            (self.next() % 10_000) as f64 / 10_000.0 * span
        }
    }

    /// A busy 64 × 64 drawing in which order shows everywhere: short hairlines, wide lines and
    /// points in six colours crossing each other (and cell edges), opaque and see-through fills
    /// over them, long lines across the whole area, wipeouts and coloured masks starting
    /// draw-order groups, and an image between two groups.
    fn busy_list(seed: u64) -> DisplayList {
        let mut r = Rng(seed);
        let mut l = DisplayList::default();
        let colors = [Rgb(255, 0, 0), Rgb(0, 255, 0), Rgb(0, 0, 255), Rgb(255, 255, 0), Rgb(255, 0, 255), Rgb(0, 255, 255)];
        let add = |l: &mut DisplayList, kind: Kind, color: Rgb, lw: f32, transparency: u8, pts: Vec<Vec2>| {
            let fill = kind.is_fill();
            let start = if fill { l.tris.len() } else { l.verts.len() } as u32;
            for p in &pts {
                l.bounds.add(*p);
            }
            let len = pts.len() as u32;
            if fill { l.tris.extend(pts) } else { l.verts.extend(pts) }
            l.prims.push(DPrim { handle: Handle(1), color, aci7: false, lw, transparency, kind, start, len });
        };
        l.images.push(cadaddy_render::ImageRef {
            path: "x".into(),
            raster: Arc::new(cadaddy_render::images::Raster::new(2, 1, vec![200, 100, 50, 255, 20, 40, 60, 255]).unwrap()),
            origin: Vec2::new(30.0, 40.0),
            ex: Vec2::new(12.0, 0.0),
            ey: Vec2::new(0.0, -12.0),
            alpha: 255,
            dark_stencil: false,
        });
        for i in 0..600 {
            let color = colors[(r.next() % 6) as usize];
            let p = Vec2::new(r.f(64.0), r.f(64.0));
            let q = p + Vec2::new(r.f(14.0) - 7.0, r.f(14.0) - 7.0);
            match (i, r.next() % 20) {
                (300, _) => {
                    let s = [Vec2::new(30.0, 28.0), Vec2::new(42.0, 28.0), Vec2::new(42.0, 40.0), Vec2::new(30.0, 40.0)];
                    add(&mut l, Kind::Image(0), Rgb(255, 255, 255), 0.0, 0, vec![s[0], s[1], s[2], s[0], s[2], s[3]]);
                }
                (_, 0..=8) => add(&mut l, Kind::Polyline, color, 0.0, 0, vec![p, q, q + Vec2::new(r.f(6.0), -r.f(6.0))]),
                (_, 9 | 10) => add(&mut l, Kind::Polyline, color, 0.5, 0, vec![p, q]),
                (_, 11 | 12) => {
                    let t = if r.next() % 2 == 0 { 0 } else { 50 };
                    add(&mut l, Kind::Tris, color, 0.0, t, vec![p, q, Vec2::new(p.x, q.y)]);
                }
                (_, 13 | 14) => add(&mut l, Kind::Point, color, 0.0, 0, vec![p]),
                (_, 15 | 16) => add(&mut l, Kind::Polyline, color, 0.0, 0, vec![Vec2::new(r.f(64.0), 0.0), Vec2::new(r.f(64.0), 64.0)]),
                (_, 17) if i % 3 == 0 => add(&mut l, Kind::Mask, color, 0.0, 0, vec![p, q, Vec2::new(q.x, p.y)]),
                (_, 17) => add(&mut l, Kind::InkMask, color, 0.0, 0, vec![p, q, Vec2::new(q.x, p.y)]),
                _ => add(&mut l, Kind::Polyline, color, 0.0, 30, vec![p, q]),
            }
        }
        l
    }

    /// Cells of 8 units over the busy list: a grid of 8 × 8, order kept within 3 pixels at one
    /// pixel a unit (more than half the widest line drawn, 1.9 points, and a pixel).
    fn small_tiles() -> Tiling {
        Tiling { min: Vec2::ZERO, cell: 8.0, cols: 8, rows: 8, margin: 3.0 }
    }

    /// One 64 × 64 frame of `mesh` (origin at 0,0) drawn by a window of its own, with its frame
    /// kept from the first frame on (`kept`) or drawn directly, looking at `center` at `scale`
    /// pixels a unit, read back.
    fn read_frame(rs: &egui_wgpu::RenderState, mesh: MeshData, kept: bool, center: Vec2, scale: f32, cull: Option<Bounds2>) -> Vec<u8> {
        use egui_wgpu::CallbackTrait;
        let (device, queue) = (&rs.device, &rs.queue);
        let (w, h) = (64u32, 64u32);
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut r = egui_wgpu::CallbackResources::default();
        let mut res = Resources::new(device, GpuTarget { format, stencil: None });
        res.frame_cache_min = if kept { 0 } else { u64::MAX };
        r.insert(res);
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
        let cb = CanvasCallback {
            key: 1,
            slot: Arc::new(Mutex::new(Some(mesh))),
            offset: [-center.x as f32, -center.y as f32],
            scale,
            center: [32.0, 32.0],
            layer: LAYER_DRAWING,
            xform: IDENTITY_XFORM,
            dash: 0.0,
            stencil: Stencil::Off,
            cull,
        };
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [w, h], pixels_per_point: 1.0 };
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        cb.prepare(device, queue, &screen, &mut enc, &mut r);
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32));
        {
            let mut pass = enc
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.1, g: 0.1, b: 0.1, a: 1.0 }), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            cb.paint(egui::PaintCallbackInfo { viewport: rect, clip_rect: rect, pixels_per_point: 1.0, screen_size_px: [w, h] }, &mut pass, &r);
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(w * 4 * h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: Some(std::time::Duration::from_secs(10)) }).unwrap();
        buffer.slice(..).get_mapped_range().unwrap().to_vec()
    }

    /// Pixels that differ between two read backs.
    fn differing(a: &[u8], b: &[u8]) -> usize {
        a.chunks(4).zip(b.chunks(4)).filter(|(x, y)| x != y).count()
    }

    /// Q6472: the vertex data cut into area tiles draws pixel for pixel what the whole list
    /// draws: hairlines, wide lines, points and fills of different colours crossing each other
    /// and the tiles' edges, see-through ones, wipeouts and coloured masks across tile edges, an
    /// image between draw-order groups. Seen whole, and zoomed in with the tiles out of view left
    /// out, drawn directly and into a kept frame.
    #[test]
    fn tiles_draw_exactly_what_the_whole_list_draws() {
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        for seed in [3u64, 77, 4242] {
            let l = busy_list(seed);
            let whole = || build_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), true);
            let tiled = || build_mesh_tiled(&l, Vec2::ZERO, Rgb(0, 0, 0), true, Some(&small_tiles()));
            let tiles = tiled().tiles.unwrap();
            assert_eq!(tiles.bounds.len(), 65, "8 × 8 cells and the large tile");
            assert!(tiles.groups.len() > 5, "wipeouts and masks start groups: {}", tiles.groups.len());
            let held = tiles.drawn(&vec![true; 65]);
            assert_eq!(held.vertices, whole().line_vertices() + whole().tri_vertices() + 6 * whole().quad_instances());
            // Whole: every tile drawn. (A kept frame blends a see-through pixel in two steps,
            // which may round it apart from drawing it directly: each is compared with its own.)
            let center = Vec2::new(32.0, 32.0);
            let empty = read_frame(&rs, MeshData::default(), false, center, 1.0, None);
            for kept in [false, true] {
                let reference = read_frame(&rs, whole(), kept, center, 1.0, None);
                let drawn = differing(&reference, &empty);
                assert!(drawn > 2000, "the frame is busy: {drawn} pixels drawn");
                let all = read_frame(&rs, tiled(), kept, center, 1.0, Some(Bounds2::new(Vec2::new(-8.0, -8.0), Vec2::new(72.0, 72.0))));
                assert_eq!(differing(&all, &reference), 0, "seed {seed}, kept {kept}: tiled differs from whole");
            }
            // Zoomed in three times on a corner: only the tiles reaching into it are drawn.
            let (center, scale) = (Vec2::new(20.0, 44.0), 3.0f32);
            let half = 32.0 / f64::from(scale);
            let cull = Bounds2::new(center - Vec2::new(half, half), center + Vec2::new(half, half)).expand(8.0 / f64::from(scale));
            let shown = tiles.visible(&cull);
            let part = tiles.drawn(&shown);
            assert!(part.tiles < 40 && part.vertices < held.vertices, "{part:?} of {held:?}");
            for kept in [false, true] {
                let reference = read_frame(&rs, whole(), kept, center, scale, None);
                let near = read_frame(&rs, tiled(), kept, center, scale, Some(cull));
                assert_eq!(differing(&near, &reference), 0, "seed {seed}, kept {kept}: zoomed in, tiled differs from whole");
            }
        }
    }

    /// Wipeouts and draw order across a tile edge: a red line drawn first, then a wipeout over
    /// it in the next cell, then a green line over both, then a blue line of the first cell
    /// crossing the green one in the second. Each pixel shows what came last, as the whole list
    /// draws it (drawn tile by tile, the green line would cover the blue one).
    #[test]
    fn draw_order_and_wipeouts_hold_across_a_tile_edge() {
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        let mut l = DisplayList::default();
        let mut add = |kind: Kind, color: Rgb, pts: Vec<Vec2>| {
            let fill = kind.is_fill();
            let start = if fill { l.tris.len() } else { l.verts.len() } as u32;
            for p in &pts {
                l.bounds.add(*p);
            }
            let len = pts.len() as u32;
            if fill { l.tris.extend(pts) } else { l.verts.extend(pts) }
            l.prims.push(DPrim { handle: Handle(1), color, aci7: false, lw: 0.0, transparency: 0, kind, start, len });
        };
        // The cell edge is x = 8; each object's centre is in the cell on one side of it.
        add(Kind::Polyline, Rgb(255, 0, 0), vec![Vec2::new(3.0, 4.5), Vec2::new(10.5, 4.5)]);
        add(Kind::Mask, Rgb(0, 0, 0), vec![Vec2::new(7.0, 2.0), Vec2::new(14.0, 2.0), Vec2::new(14.0, 7.0), Vec2::new(7.0, 2.0), Vec2::new(14.0, 7.0), Vec2::new(7.0, 7.0)]);
        add(Kind::Polyline, Rgb(0, 255, 0), vec![Vec2::new(9.5, 1.0), Vec2::new(9.5, 7.0)]);
        add(Kind::Polyline, Rgb(0, 0, 255), vec![Vec2::new(5.5, 6.0), Vec2::new(9.9, 6.0)]);
        add(Kind::Tris, Rgb(255, 255, 255), vec![Vec2::new(6.0, 5.0), Vec2::new(10.0, 5.0), Vec2::new(10.0, 6.0)]);
        let tiling = Tiling { min: Vec2::ZERO, cell: 8.0, cols: 2, rows: 1, margin: 0.5 };
        let tiled = build_mesh_tiled(&l, Vec2::ZERO, Rgb(9, 9, 9), false, Some(&tiling));
        assert_eq!(tiled.tiles.as_ref().map(|t| t.groups.len()), Some(2), "the wipeout starts a group");
        let center = Vec2::new(8.0, 4.0);
        let reference = read_frame(&rs, build_mesh(&l, Vec2::ZERO, Rgb(9, 9, 9), false), false, center, 4.0, None);
        let got = read_frame(&rs, tiled, false, center, 4.0, Some(Bounds2::new(Vec2::new(-1.0, -1.0), Vec2::new(17.0, 9.0))));
        assert_eq!(differing(&got, &reference), 0);
    }

    /// A small drawing is drawn whole, as before; a large one gets tiles at least 1024 pixels
    /// across where its zoom band is drawn, never more cells than the most, and a margin to keep
    /// the order of primitives within the widest line of each other.
    #[test]
    fn tiles_follow_the_drawing_and_the_zoom() {
        let b = Bounds2::new(Vec2::ZERO, Vec2::new(10_000.0, 5_000.0));
        assert_eq!(Tiling::for_bounds(&b, TILE_VERTICES, 1.0), None, "a small list stays one tile");
        let t = Tiling::for_bounds(&b, 10_000_000, 0.5).unwrap();
        assert!(t.cell >= TILE_MIN_PX * 0.5 && t.cells() <= MAX_TILES && t.cells() >= 2, "{t:?}");
        assert_eq!(t.margin, ORDER_MARGIN_PX * 0.5);
        // Zoomed far out, a tile would be smaller than the screen: one tile.
        assert_eq!(Tiling::for_bounds(&b, 10_000_000, 20.0), None);
        // A long, thin drawing.
        let thin = Tiling::for_bounds(&Bounds2::new(Vec2::ZERO, Vec2::new(1e7, 1.0)), 100_000_000, 1e-3).unwrap();
        assert!(thin.cells() <= MAX_TILES, "{thin:?}");
        for bad in [f64::NAN, 0.0, -1.0, f64::INFINITY] {
            assert_eq!(Tiling::for_bounds(&b, 10_000_000, bad), None);
        }
        assert_eq!(Tiling::for_bounds(&Bounds2::EMPTY, 10_000_000, 1.0), None);
    }

    /// A drawing's own draw order (DRAWORDER: an object brought to the front, one sent to the
    /// back) across tile edges, through the renderer: the tiled frame is the whole one.
    #[test]
    fn draworder_across_tile_edges_draws_as_the_whole_list() {
        use cadaddy_doc::{Common, Drawing, EntityKind, Line, Space};
        use cadaddy_geom::Vec3;
        let rs = egui_kittest::wgpu::create_render_state(egui_kittest::wgpu::default_wgpu_setup(), Default::default());
        let mut d = Drawing::new_imperial();
        let mut handles = Vec::new();
        for i in 0..24 {
            let (a, b) = if i % 2 == 0 {
                (Vec3::new(2.0, 1.0 + f64::from(i) * 2.5, 0.0), Vec3::new(13.0, 1.5 + f64::from(i) * 2.5, 0.0))
            } else {
                (Vec3::new(1.0 + f64::from(i) * 2.5, 2.0, 0.0), Vec3::new(1.5 + f64::from(i) * 2.5, 13.0, 0.0))
            };
            let common = Common { color: cadaddy_color::Color::Index(1 + (i % 6) as u8), ..Common::default() };
            let Ok(h) = d.add(&Space::Model, common, EntityKind::Line(Line { a, b })) else { panic!("add") };
            handles.push(h);
        }
        for (k, h) in handles.iter().enumerate() {
            if k % 5 == 0 {
                d.model.bring_to_front(*h);
            } else if k % 7 == 0 {
                d.model.send_to_back(*h);
            }
        }
        let l = cadaddy_render::build(&d, &Space::Model, &cadaddy_render::Options { tolerance: 0.1, min_dash: 0.4, ..Default::default() });
        let tiling = Tiling { min: Vec2::ZERO, cell: 8.0, cols: 8, rows: 8, margin: 1.5 };
        let center = Vec2::new(32.0, 32.0);
        let whole = read_frame(&rs, build_mesh(&l, Vec2::ZERO, Rgb(0, 0, 0), false), false, center, 1.0, None);
        let tiled = read_frame(&rs, build_mesh_tiled(&l, Vec2::ZERO, Rgb(0, 0, 0), false, Some(&tiling)), false, center, 1.0, None);
        assert_eq!(differing(&tiled, &whole), 0);
    }

    /// An edit changes the bytes of the tiles of the objects it changed, and no other tile's:
    /// the upload cuts each tile into pieces of its own, so the others' pieces are used again.
    #[test]
    fn an_edit_changes_only_its_tiles_bytes() {
        let l = busy_list(9);
        let before = build_mesh_tiled(&l, Vec2::ZERO, Rgb(0, 0, 0), true, Some(&small_tiles()));
        // A short hairline moved a little, inside its cell.
        let mut moved = l.clone();
        let Some((i, p)) = moved.prims.iter().enumerate().find(|(_, p)| {
            let b = Bounds2::from_points(l.points(p).iter().copied());
            let inside = (b.center().x % 8.0) > 1.0 && (b.center().x % 8.0) < 7.0;
            p.kind == Kind::Polyline && p.lw == 0.0 && b.width() < 2.0 && b.height() < 2.0 && b.min.x > 10.0 && b.max.x < 54.0 && inside
        }) else {
            panic!("no short line")
        };
        let (start, len) = (p.start as usize, p.len as usize);
        for v in moved.verts.iter_mut().skip(start).take(len) {
            v.x += 0.25;
        }
        let after = build_mesh_tiled(&moved, Vec2::ZERO, Rgb(0, 0, 0), true, Some(&small_tiles()));
        let (tb, ta) = (before.tiles.unwrap(), after.tiles.unwrap());
        let changed: Vec<usize> = (0..tb.bounds.len())
            .filter(|&t| {
                let (a, b): (Vec<&[u8]>, Vec<&[u8]>) = (tb.parts(&before.lines, 1, VERTEX_SIZE).collect(), ta.parts(&after.lines, 1, VERTEX_SIZE).collect());
                a.get(t) != b.get(t)
            })
            .collect();
        assert_eq!(changed.len(), 1, "prim {i}: tiles changed {changed:?}");
        assert_eq!(tb.parts(&before.tris, 0, VERTEX_SIZE).collect::<Vec<_>>(), ta.parts(&after.tris, 0, VERTEX_SIZE).collect::<Vec<_>>());
    }

    /// The highest layer of the tiles other than one.
    #[test]
    fn tops_know_the_highest_layer_of_the_other_tiles() {
        let mut t = Tops::default();
        assert_eq!(t.other_than(0), None);
        t.raise(3, 0);
        assert_eq!((t.other_than(3), t.other_than(1)), (None, Some(0)));
        t.raise(3, 2);
        t.raise(5, 1);
        assert_eq!((t.other_than(3), t.other_than(5), t.other_than(7)), (Some(1), Some(2), Some(2)));
        t.raise(5, 4);
        assert_eq!((t.other_than(3), t.other_than(5)), (Some(4), Some(2)));
    }
}
