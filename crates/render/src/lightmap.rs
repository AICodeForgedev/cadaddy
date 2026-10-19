//! Baked indirect light (LIGHTBAKE): every triangle of the scene's objects gets a square cell of
//! texels in one atlas; each texel holds the light that reaches its point of the surface from all
//! around it — sky through openings and light bounced off other surfaces — path traced with the
//! RENDER tracer and filtered within its cell. The Realistic style then lights surfaces with it
//! in place of its uniform sky ambient, so rooms are dark in their corners and bright by their
//! windows, at full frame rate.
//!
//! Each side of a triangle has its own cell (a wall's room side and its outside see different
//! light). A cell is `n × n` texels with a one-texel border (copies of its edge texels, so
//! filtering at the cell's edge doesn't bleed in its neighbour). Texel `(i, j)` stands for the
//! barycentric point `((i + ½)/n, (j + ½)/n)` (pulled onto the triangle where that is past its
//! long edge). Each object keeps its cells in triangle order, and a hash of what it is (its
//! geometry, layer, colour and material) so a rebake redoes only what changed.
//!
//! Stored as an RGBM PNG (colour × alpha × [`RANGE`]) and an index beside the drawing (written
//! and read by the engine's LIGHTBAKE).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use cadaddy_doc::{Drawing, Entity, Handle};
use cadaddy_geom::{Vec2, Vec3};

use crate::light::Lighting;
use crate::shade::{Scene, ShadeView};
use crate::trace::{Render, Rng, TraceOptions};

/// RGBM range: a texel's light up to this.
pub const RANGE: f32 = 8.0;
/// Most texels an atlas holds (4096 × 4096).
pub const MAX_TEXELS: usize = 4096 * 4096;
/// Texels across a triangle's cell, at most and at least.
pub const MAX_CELL: u32 = 32;
pub const MIN_CELL: u32 = 2;
/// The atlas's width.
const ATLAS_W: u32 = 2048;

/// Where a triangle's texels are: the top-left texel of its cell (inside the border) and its
/// size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub x: u32,
    pub y: u32,
    pub n: u32,
}

/// One object's cells, triangle by triangle, and what it was when baked.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectCells {
    pub hash: u64,
    pub cells: Vec<Cell>,
}

/// A baked atlas.
#[derive(Clone, Debug, PartialEq)]
pub struct Lightmap {
    pub width: u32,
    pub height: u32,
    /// Linear light per texel (as radiance: irradiance / π), row by row.
    pub texels: Vec<[f32; 3]>,
    pub objects: BTreeMap<Handle, ObjectCells>,
    /// What lit it (the sun, the sky, the lights): a change rebakes everything.
    pub lighting: u64,
}

/// How to bake.
#[derive(Clone, Copy, Debug)]
pub struct BakeOptions {
    /// Paths per texel.
    pub samples: u32,
    pub bounces: u32,
    /// Texel size in drawing units.
    pub texel: f64,
    pub seed: u64,
}

/// What a bake did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BakeReport {
    pub objects: usize,
    pub baked: usize,
    pub kept: usize,
    pub texels: usize,
    pub seconds: f64,
}

/// FNV-1a.
fn fnv(bytes: &[u8], mut h: u64) -> u64 {
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

const FNV0: u64 = 0xcbf2_9ce4_8422_2325;

/// What an object is, for the bake: its geometry, layer, colour and material.
pub fn object_hash(d: &Drawing, e: &Entity) -> u64 {
    let mut h = fnv(format!("{:?}", e.kind).as_bytes(), FNV0);
    h = fnv(e.common.layer.as_bytes(), h);
    h = fnv(format!("{:?}", e.common.color).as_bytes(), h);
    let mat = d.material_of(e, None).map(|m| format!("{:?}", m)).unwrap_or_default();
    fnv(mat.as_bytes(), h)
}

/// What lights a drawing's space, for the bake.
pub fn lighting_hash(lit: &Lighting) -> u64 {
    let mut h = fnv(format!("{:?}", lit.sun).as_bytes(), FNV0);
    h = fnv(format!("{:?}", lit.sky).as_bytes(), h);
    fnv(format!("{:?}", lit.lights).as_bytes(), h)
}

/// The cells each object's triangles have in the scene (ordinal within the object → scene
/// triangle index).
fn object_tris(scene: &Scene) -> BTreeMap<Handle, Vec<usize>> {
    let mut m: BTreeMap<Handle, Vec<usize>> = BTreeMap::new();
    for (i, h) in scene.owners.iter().enumerate().take(scene.tris.len()) {
        m.entry(*h).or_default().push(i);
    }
    m
}

/// A triangle's cell size for a texel size.
fn cell_size(p: &[Vec3; 3], texel: f64) -> u32 {
    let longest = [(p[0] - p[1]).len(), (p[1] - p[2]).len(), (p[2] - p[0]).len()].into_iter().fold(0.0, f64::max);
    let n = (longest / texel.max(1e-12)).ceil();
    if n.is_finite() { (n as u32).clamp(MIN_CELL, MAX_CELL) } else { MIN_CELL }
}

/// The barycentric point texel `(i, j)` of an `n` cell stands for.
fn texel_point(i: u32, j: u32, n: u32) -> (f64, f64) {
    let nf = f64::from(n);
    let (mut u, mut v) = ((f64::from(i) + 0.5) / nf, (f64::from(j) + 0.5) / nf);
    let s = u + v;
    if s > 1.0 {
        // Past the long edge: onto it.
        u /= s;
        v /= s;
    }
    (u, v)
}

/// Shelf-pack cells of the given sizes (border included); `None` when they don't fit.
fn pack(sizes: &[u32]) -> Option<(Vec<Cell>, u32, u32)> {
    let mut cells = Vec::with_capacity(sizes.len());
    let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
    for &n in sizes {
        let s = n + 2;
        if x + s > ATLAS_W {
            x = 0;
            y = y.checked_add(shelf)?;
            shelf = 0;
        }
        cells.push(Cell { x: x + 1, y: y + 1, n });
        x += s;
        shelf = shelf.max(s);
    }
    let h = y.checked_add(shelf)?.max(1);
    ((ATLAS_W as usize) * (h as usize) <= MAX_TEXELS).then_some((cells, ATLAS_W, h))
}

/// Bake `scene`'s objects (all, or with `previous`, only those changed or near a change), lit by
/// `lit`. `hashes`: each object's [`object_hash`] now.
pub fn bake(scene: &Scene, lit: &Lighting, hashes: &HashMap<Handle, u64>, previous: Option<&Lightmap>, o: &BakeOptions) -> (Lightmap, BakeReport) {
    #[cfg(not(target_arch = "wasm32"))]
    let started = std::time::Instant::now();
    let tris = object_tris(scene);
    let light_hash = lighting_hash(lit);
    let prev = previous.filter(|p| p.lighting == light_hash);
    // What must be baked again: new or changed objects, and those whose bounds come within a
    // room's reach (3 m) of a changed one (their light changes with it).
    let bounds = |idx: &[usize]| {
        let mut lo = Vec3::new(f64::MAX, f64::MAX, f64::MAX);
        let mut hi = Vec3::new(f64::MIN, f64::MIN, f64::MIN);
        for p in idx.iter().filter_map(|i| scene.tris.get(*i)).flat_map(|t| t.p) {
            lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        (lo, hi)
    };
    let fits = |h: &Handle, idx: &Vec<usize>| {
        prev.and_then(|p| p.objects.get(h)).is_some_and(|c| Some(&c.hash) == hashes.get(h) && c.cells.len() == idx.len() * 2)
    };
    let changed: Vec<(Vec3, Vec3)> = tris.iter().filter(|(h, idx)| !fits(h, idx)).map(|(_, idx)| bounds(idx)).collect();
    // Removed objects change their room too.
    let removed: Vec<Handle> = prev.map(|p| p.objects.keys().filter(|h| !tris.contains_key(h)).copied().collect()).unwrap_or_default();
    let reach = 3.0 / lit.unit_m.max(1e-9);
    let near = |b: (Vec3, Vec3), c: &(Vec3, Vec3)| {
        b.0.x <= c.1.x + reach
            && b.1.x >= c.0.x - reach
            && b.0.y <= c.1.y + reach
            && b.1.y >= c.0.y - reach
            && b.0.z <= c.1.z + reach
            && b.1.z >= c.0.z - reach
    };
    let redo: HashSet<Handle> = tris
        .iter()
        .filter(|(h, idx)| {
            !fits(h, idx) || {
                let b = bounds(idx);
                changed.iter().any(|c| near(b, c))
            }
        })
        .map(|(h, _)| *h)
        .collect();
    let redo = if removed.is_empty() { redo } else { tris.keys().copied().collect() };

    // Cell sizes: kept objects keep theirs; the texel grows until everything fits.
    let mut texel = o.texel.max(1e-9);
    let (order, cells, w, h) = loop {
        let mut order: Vec<(Handle, usize)> = Vec::new();
        let mut sizes = Vec::new();
        for (hd, idx) in &tris {
            let old = prev.and_then(|p| p.objects.get(hd)).filter(|_| !redo.contains(hd));
            // Two cells a triangle: its front (as its corners turn) and its back.
            for slot in 0..idx.len() * 2 {
                let n = match old.and_then(|c| c.cells.get(slot)) {
                    Some(c) => c.n,
                    None => idx.get(slot / 2).and_then(|ti| scene.tris.get(*ti)).map_or(MIN_CELL, |t| cell_size(&t.p, texel)),
                };
                order.push((*hd, slot));
                sizes.push(n);
            }
        }
        match pack(&sizes) {
            Some((cells, w, h)) => break (order, cells, w, h),
            None if texel < 1e12 => texel *= 1.5,
            None => break (Vec::new(), Vec::new(), 1, 1),
        }
    };
    let mut lm =
        Lightmap { width: w, height: h, texels: vec![[0.0; 3]; (w as usize) * (h as usize)], objects: BTreeMap::new(), lighting: light_hash };
    for ((hd, _), c) in order.iter().zip(&cells) {
        lm.objects.entry(*hd).or_insert_with(|| ObjectCells { hash: hashes.get(hd).copied().unwrap_or(0), cells: Vec::new() }).cells.push(*c);
    }
    // The tracer, over the whole scene.
    let view = ShadeView::new(None, Vec2::ZERO, 1.0, 1, 1);
    let tracer = Render::new(scene.clone(), view, lit.clone(), TraceOptions { bounces: o.bounces.max(1), denoise: false, crop: None, seed: o.seed });
    let eps = tracer.epsilon().max(1e-9) * 10.0;
    let samples = o.samples.max(1);
    // Every texel to bake: (cell, triangle, i, j).
    let mut jobs: Vec<(Cell, usize, bool, u32, u32)> = Vec::new();
    let mut report = BakeReport { objects: tris.len(), ..BakeReport::default() };
    for ((hd, k), c) in order.iter().zip(&cells) {
        let Some(ti) = tris.get(hd).and_then(|v| v.get(*k / 2)).copied() else { continue };
        if redo.contains(hd) {
            for j in 0..c.n {
                for i in 0..c.n {
                    jobs.push((*c, ti, *k % 2 == 1, i, j));
                }
            }
        } else if let Some(old) = prev.and_then(|p| p.objects.get(hd)).and_then(|oc| oc.cells.get(*k))
            && let Some(p) = prev
        {
            // Kept: copy its texels across.
            for j in 0..c.n {
                for i in 0..c.n {
                    let src = p.texels.get(((old.y + j) * p.width + old.x + i) as usize).copied().unwrap_or([0.0; 3]);
                    if let Some(t) = lm.texels.get_mut(((c.y + j) * lm.width + c.x + i) as usize) {
                        *t = src;
                    }
                }
            }
        }
    }
    report.baked = redo.len();
    report.kept = tris.len().saturating_sub(redo.len());
    report.texels = jobs.len();
    let values = crate::trace::par_map(0..jobs.len(), |q| {
        let Some(&(c, ti, back, i, j)) = jobs.get(q) else { return [0.0; 3] };
        let Some(t) = scene.tris.get(ti) else { return [0.0; 3] };
        let (u, v) = texel_point(i, j, c.n);
        let p = t.p[0] + (t.p[1] - t.p[0]) * u + (t.p[2] - t.p[0]) * v;
        let ng = (t.p[1] - t.p[0]).cross(t.p[2] - t.p[0]).normalized();
        if !ng.is_finite() {
            return [0.0; 3];
        }
        let n = if back { ng * -1.0 } else { ng };
        let mut rng = Rng::new(o.seed ^ ((ti as u64) << 21) ^ (u64::from(back) << 20) ^ u64::from(j * c.n + i), 0x5eed);
        let mut sum = [0.0f32; 3];
        for _ in 0..samples {
            let d = crate::trace::cosine_dir(n, rng.f(), rng.f());
            let l = tracer.radiance(p + n * eps, d, &mut rng);
            for k in 0..3 {
                sum[k] += l[k];
            }
        }
        sum.map(|x| x / samples as f32)
    });
    for (&(c, _, _, i, j), v) in jobs.iter().zip(values) {
        if let Some(t) = lm.texels.get_mut(((c.y + j) * lm.width + c.x + i) as usize) {
            *t = v.map(|x| if x.is_finite() { x.max(0.0) } else { 0.0 });
        }
    }
    // Filter the baked cells (3 × 3 within the cell, twice), then copy their edges into the borders.
    let redo_cells: Vec<Cell> = order.iter().zip(&cells).filter(|((hd, _), _)| redo.contains(hd)).map(|(_, c)| *c).collect();
    for c in redo_cells.iter().chain(redo_cells.iter()) {
        let get = |lm: &Lightmap, i: u32, j: u32| lm.texels.get(((c.y + j) * lm.width + c.x + i) as usize).copied().unwrap_or([0.0; 3]);
        let mut out = Vec::with_capacity((c.n * c.n) as usize);
        for j in 0..c.n {
            for i in 0..c.n {
                let mut s = [0.0f32; 3];
                let mut w = 0.0f32;
                for dj in -1i64..=1 {
                    for di in -1i64..=1 {
                        let (ii, jj) = (i64::from(i) + di, i64::from(j) + dj);
                        if ii < 0 || jj < 0 || ii >= i64::from(c.n) || jj >= i64::from(c.n) {
                            continue;
                        }
                        let k = if di == 0 && dj == 0 { 2.0 } else { 1.0 };
                        let t = get(&lm, ii as u32, jj as u32);
                        for q in 0..3 {
                            s[q] += t[q] * k;
                        }
                        w += k;
                    }
                }
                out.push(s.map(|x| x / w.max(1.0)));
            }
        }
        for j in 0..c.n {
            for i in 0..c.n {
                if let (Some(t), Some(v)) = (lm.texels.get_mut(((c.y + j) * lm.width + c.x + i) as usize), out.get((j * c.n + i) as usize)) {
                    *t = *v;
                }
            }
        }
    }
    for c in &cells {
        border(&mut lm, *c);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        report.seconds = started.elapsed().as_secs_f64();
    }
    (lm, report)
}

/// Copy a cell's edge texels into its one-texel border.
fn border(lm: &mut Lightmap, c: Cell) {
    let w = lm.width;
    let at = |x: u32, y: u32| (y * w + x) as usize;
    let n = c.n;
    for k in 0..n {
        let pairs = [
            ((c.x + k, c.y), (c.x + k, c.y - 1)),
            ((c.x + k, c.y + n - 1), (c.x + k, c.y + n)),
            ((c.x, c.y + k), (c.x - 1, c.y + k)),
            ((c.x + n - 1, c.y + k), (c.x + n, c.y + k)),
        ];
        for ((sx, sy), (dx, dy)) in pairs {
            if let Some(v) = lm.texels.get(at(sx, sy)).copied()
                && let Some(t) = lm.texels.get_mut(at(dx, dy))
            {
                *t = v;
            }
        }
    }
    for ((sx, sy), (dx, dy)) in [
        ((c.x, c.y), (c.x - 1, c.y - 1)),
        ((c.x + n - 1, c.y), (c.x + n, c.y - 1)),
        ((c.x, c.y + n - 1), (c.x - 1, c.y + n)),
        ((c.x + n - 1, c.y + n - 1), (c.x + n, c.y + n)),
    ] {
        if let Some(v) = lm.texels.get(at(sx, sy)).copied()
            && let Some(t) = lm.texels.get_mut(at(dx, dy))
        {
            *t = v;
        }
    }
}

/// A scene's baked light: each triangle's front and back cells (where its object matches the
/// lightmap) and the atlas.
#[derive(Clone, Debug)]
pub struct Baked {
    pub map: Arc<Lightmap>,
    /// Per scene triangle: front and back.
    pub cells: Vec<Option<[Cell; 2]>>,
}

impl Baked {
    /// The cells of `scene`'s triangles in `map`: objects whose hash still matches.
    pub fn of(scene: &Scene, map: Arc<Lightmap>, hashes: &HashMap<Handle, u64>) -> Baked {
        let mut cells = vec![None; scene.tris.len()];
        for (h, idx) in object_tris(scene) {
            let Some(oc) = map.objects.get(&h).filter(|oc| Some(&oc.hash) == hashes.get(&h) && oc.cells.len() == idx.len() * 2) else { continue };
            for (k, ti) in idx.iter().enumerate() {
                if let (Some(slot), Some(f), Some(b)) = (cells.get_mut(*ti), oc.cells.get(2 * k), oc.cells.get(2 * k + 1)) {
                    *slot = Some([*f, *b]);
                }
            }
        }
        Baked { map, cells }
    }

    /// The light baked at barycentric `(u, v)` of triangle `ti`, on its back (`back`) or front
    /// (bilinear within its cell).
    pub fn at(&self, ti: usize, u: f64, v: f64, back: bool) -> Option<[f32; 3]> {
        let pair = (*self.cells.get(ti)?)?;
        let c = pair[usize::from(back)];
        let (x, y) = (u * f64::from(c.n) - 0.5, v * f64::from(c.n) - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = ((x - x0) as f32, (y - y0) as f32);
        let m = &self.map;
        let get = |i: f64, j: f64| {
            let (i, j) = (i.clamp(-1.0, f64::from(c.n)) as i64, j.clamp(-1.0, f64::from(c.n)) as i64);
            let (px, py) = (i64::from(c.x) + i, i64::from(c.y) + j);
            if px < 0 || py < 0 {
                return [0.0; 3];
            }
            m.texels.get((py as usize) * (m.width as usize) + px as usize).copied().unwrap_or([0.0; 3])
        };
        let (a, b, cc, d) = (get(x0, y0), get(x0 + 1.0, y0), get(x0, y0 + 1.0), get(x0 + 1.0, y0 + 1.0));
        Some([0, 1, 2].map(|k| (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (cc[k] * (1.0 - fx) + d[k] * fx) * fy))
    }

    /// Texture coordinates (0–1 across the atlas) of a triangle's corners in its front and back
    /// cells.
    pub fn corner_uv(&self, ti: usize) -> Option<[[[f32; 2]; 3]; 2]> {
        let pair = (*self.cells.get(ti)?)?;
        let (w, h) = (self.map.width as f32, self.map.height as f32);
        let p = |x: u32, y: u32| [x as f32 / w, y as f32 / h];
        Some(pair.map(|c| [p(c.x, c.y), p(c.x + c.n, c.y), p(c.x, c.y + c.n)]))
    }
}

// ---------- files ----------

/// The atlas as RGBM bytes (RGBA8, colour × alpha × [`RANGE`]).
pub fn rgbm(lm: &Lightmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(lm.texels.len() * 4);
    for t in &lm.texels {
        let m = (t.iter().copied().fold(0.0f32, f32::max) / RANGE).clamp(1.0 / 255.0, 1.0);
        let a = (m * 255.0).ceil() / 255.0;
        for c in t {
            out.push(((c / (a * RANGE)).clamp(0.0, 1.0) * 255.0).round() as u8);
        }
        out.push((a * 255.0).round() as u8);
    }
    out
}

/// Texels back from RGBM bytes.
pub fn from_rgbm(bytes: &[u8]) -> Vec<[f32; 3]> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let a = f32::from(p[3]) / 255.0 * RANGE;
            [f32::from(p[0]) / 255.0 * a, f32::from(p[1]) / 255.0 * a, f32::from(p[2]) / 255.0 * a]
        })
        .collect()
}

/// An atlas PNG's size and texels; `None` when it isn't one (or is too large).
pub fn from_png(bytes: &[u8]) -> Option<(u32, u32, Vec<[f32; 3]>)> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let (w, h) = (img.width(), img.height());
    ((w as usize) * (h as usize) <= MAX_TEXELS).then(|| (w, h, from_rgbm(img.as_raw())))
}

/// The atlas as a PNG.
pub fn to_png(lm: &Lightmap) -> Option<Vec<u8>> {
    let img = image::RgbaImage::from_raw(lm.width, lm.height, rgbm(lm))?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shade::{VisualStyle, fit, rasterize, scene};
    use cadaddy_doc::{Common, EntityKind, Mesh, MeshFace, MeshKind, Space};
    use cadaddy_geom::view3::Projection;

    /// A 4 m × 4 m room, 3 m high, open to the sky, one face per mesh (floor and four walls);
    /// noon sun.
    fn room() -> Drawing {
        let mut d = Drawing::new_metric();
        d.sun.on = true;
        d.sun.minutes = 13 * 60;
        let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
        let quads = [
            [v(0.0, 0.0, 0.0), v(4.0, 0.0, 0.0), v(4.0, 4.0, 0.0), v(0.0, 4.0, 0.0)],
            [v(0.0, 0.0, 0.0), v(4.0, 0.0, 0.0), v(4.0, 0.0, 3.0), v(0.0, 0.0, 3.0)],
            [v(4.0, 0.0, 0.0), v(4.0, 4.0, 0.0), v(4.0, 4.0, 3.0), v(4.0, 0.0, 3.0)],
            [v(4.0, 4.0, 0.0), v(0.0, 4.0, 0.0), v(0.0, 4.0, 3.0), v(4.0, 4.0, 3.0)],
            [v(0.0, 4.0, 0.0), v(0.0, 0.0, 0.0), v(0.0, 0.0, 3.0), v(0.0, 4.0, 3.0)],
        ];
        for q in quads {
            let m = Mesh { vertices: q.to_vec(), faces: vec![MeshFace { v: vec![0, 1, 2, 3], hidden: 0 }], kind: MeshKind::Polyface };
            d.add(&Space::Model, Common::default(), EntityKind::Mesh(m)).unwrap();
        }
        d
    }

    fn hashes(d: &Drawing) -> HashMap<Handle, u64> {
        d.model.iter().map(|e| (e.handle, object_hash(d, e))).collect()
    }

    fn opts() -> BakeOptions {
        BakeOptions { samples: 12, bounces: 2, texel: 0.5, seed: 7 }
    }

    #[test]
    fn a_room_bakes_deterministically_and_rebakes_only_what_changed() {
        let d = room();
        let s = scene(&d, &Space::Model, 0.01, cadaddy_doc::color::Rgb(0, 0, 0));
        let lit = s.lighting.clone().unwrap();
        let (a, ra) = bake(&s, &lit, &hashes(&d), None, &opts());
        let (b, _) = bake(&s, &lit, &hashes(&d), None, &opts());
        assert_eq!(a, b, "the same light every time");
        assert_eq!((ra.objects, ra.baked, ra.kept), (5, 5, 0));
        assert!(ra.texels > 0 && a.texels.iter().all(|t| t.iter().all(|c| c.is_finite() && *c >= 0.0)));
        assert!(a.texels.iter().any(|t| t[0] > 0.01), "light arrives");
        // Nothing changed: everything kept, the same texels.
        let (c, rc) = bake(&s, &lit, &hashes(&d), Some(&a), &opts());
        assert_eq!((rc.baked, rc.kept), (0, 5));
        assert_eq!(c.texels.iter().map(|t| t[0]).sum::<f32>(), a.texels.iter().map(|t| t[0]).sum::<f32>());
        // A new object 10 m away (millimetre drawing) bakes alone; the room keeps its light.
        let mut far = d.clone();
        let q = [Vec3::new(10_000.0, 0.0, 0.0), Vec3::new(10_001.0, 0.0, 0.0), Vec3::new(10_001.0, 1.0, 0.0)];
        far.add(
            &Space::Model,
            Common::default(),
            EntityKind::Mesh(Mesh { vertices: q.to_vec(), faces: vec![MeshFace { v: vec![0, 1, 2], hidden: 0 }], kind: MeshKind::Polyface }),
        )
        .unwrap();
        let fs = scene(&far, &Space::Model, 0.01, cadaddy_doc::color::Rgb(0, 0, 0));
        let (_, rf) = bake(&fs, &lit, &hashes(&far), Some(&a), &opts());
        assert_eq!((rf.baked, rf.kept), (1, 5));
        // Other light: everything again.
        let mut night = lit.clone();
        night.sun = None;
        let (_, rn) = bake(&s, &night, &hashes(&d), Some(&a), &opts());
        assert_eq!(rn.baked, 5);
    }

    #[test]
    fn baked_light_changes_the_realistic_view() {
        let d = room();
        let mut s = scene(&d, &Space::Model, 0.01, cadaddy_doc::color::Rgb(0, 0, 0));
        let lit = s.lighting.clone().unwrap();
        let (map, _) = bake(&s, &lit, &hashes(&d), None, &opts());
        let view = fit(&s, Projection::new(Vec3::new(-1.0, -1.5, 1.2)), 120, 90).unwrap();
        let lum = |r: &crate::images::Raster| r.rgba.chunks(4).map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2])).sum::<u64>() as f64;
        let plain = rasterize(&s, &view, VisualStyle::Realistic, cadaddy_doc::color::Rgb(0, 0, 0));
        let baked = Baked::of(&s, Arc::new(map), &hashes(&d));
        assert!(baked.cells.iter().all(Option::is_some));
        s.baked = Some(baked);
        let lit_view = rasterize(&s, &view, VisualStyle::Realistic, cadaddy_doc::color::Rgb(0, 0, 0));
        // Per pixel: how much the baked light changes the picture.
        let diff: f64 = plain.rgba.iter().zip(&lit_view.rgba).map(|(x, y)| f64::from(x.abs_diff(*y))).sum();
        let (a, b) = (lum(&plain), lum(&lit_view));
        // (Most of the picture is sky; the room is what changes.)
        assert!(diff / a > 0.005, "baked {b} vs ambient {a}: differ by {diff}");
        // A changed object loses its cells (it shows the plain ambient until rebaked).
        let mut h = hashes(&d);
        if let Some(v) = h.values_mut().next() {
            *v ^= 1;
        }
        let stale = Baked::of(&s, s.baked.as_ref().unwrap().map.clone(), &h);
        assert_eq!(stale.cells.iter().filter(|c| c.is_none()).count(), 2, "one quad = two triangles");
    }

    #[test]
    fn rgbm_keeps_the_light() {
        let lm = Lightmap { width: 2, height: 1, texels: vec![[0.25, 1.5, 7.0], [0.0, 0.01, 0.002]], objects: BTreeMap::new(), lighting: 0 };
        let back = from_rgbm(&rgbm(&lm));
        for (a, b) in lm.texels.iter().zip(&back) {
            for k in 0..3 {
                assert!((a[k] - b[k]).abs() <= 0.03 * a[k].max(0.05), "{a:?} {b:?}");
            }
        }
        assert!(to_png(&lm).is_some());
        // Hostile: nothing to pack, and a huge cell list.
        assert!(pack(&[]).is_some());
        assert!(pack(&vec![MAX_CELL; 2_000_000]).is_none());
    }
}
