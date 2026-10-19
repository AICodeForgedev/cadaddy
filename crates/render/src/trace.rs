//! RENDER: a progressive path tracer over a shaded view's scene. Rays are cast through every
//! pixel of the view; each hit is lit directly by the sun (its disc, so shadows are soft) and the
//! lights (shadow rays), then the path bounces on (a Lambert and GGX mixture, the same surfaces
//! the Realistic style draws) until it leaves the scene and picks up the sky. Passes add one
//! sample per pixel; a filter we wrote (an edge-aware à-trous wavelet guided by each pixel's
//! surface normal, depth and colour) cleans the noise of the samples so far.
//!
//! Triangles sit in a bounding volume hierarchy (binned surface-area heuristic), built and walked
//! with explicit stacks.

use cadaddy_geom::Vec3;

use crate::images::Raster;
use crate::light::{Lighting, Surface, brdf_cos, incoming, sky_only, tonemap};
use crate::shade::{Scene, ShadeView};

/// Render quality presets (RENDERPRESETS): samples per pixel, bounces, and whether to filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Draft,
    Medium,
    High,
}

impl Preset {
    pub fn parse(s: &str) -> Option<Preset> {
        match s.trim().to_ascii_lowercase().as_str() {
            "draft" | "low" => Some(Preset::Draft),
            "medium" => Some(Preset::Medium),
            "high" | "presentation" => Some(Preset::High),
            _ => None,
        }
    }
    pub fn samples(self) -> u32 {
        match self {
            Preset::Draft => 8,
            Preset::Medium => 48,
            Preset::High => 256,
        }
    }
    pub fn bounces(self) -> u32 {
        match self {
            Preset::Draft => 2,
            Preset::Medium => 4,
            Preset::High => 8,
        }
    }
}

/// Most samples per pixel and bounces a render takes.
pub const MAX_SAMPLES: u32 = 65_536;
pub const MAX_BOUNCES: u32 = 32;

/// `f` over `range`, in parallel where threads are available (not in the browser).
fn par_map<T: Send>(range: std::ops::Range<usize>, f: impl Fn(usize) -> T + Sync + Send) -> Vec<T> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rayon::prelude::*;
        range.into_par_iter().map(f).collect()
    }
    #[cfg(target_arch = "wasm32")]
    {
        range.map(f).collect()
    }
}

// ---------------- random numbers ----------------

/// PCG32 (O'Neill, "PCG: A Family of Simple Fast Space-Efficient Statistically Good Algorithms
/// for Random Number Generation", 2014): small, fast, and the same sequence on every platform.
#[derive(Clone, Copy, Debug)]
pub struct Rng {
    state: u64,
    inc: u64,
}

impl Rng {
    pub fn new(seed: u64, stream: u64) -> Rng {
        let mut r = Rng { state: 0, inc: (stream << 1) | 1 };
        r.next_u32();
        r.state = r.state.wrapping_add(seed);
        r.next_u32();
        r
    }
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }
    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f64 {
        f64::from(self.next_u32()) / 4_294_967_296.0
    }
}

// ---------------- bounding volume hierarchy ----------------

#[derive(Clone, Copy, Debug)]
struct Aabb {
    lo: Vec3,
    hi: Vec3,
}

impl Aabb {
    const EMPTY: Aabb = Aabb { lo: Vec3 { x: f64::MAX, y: f64::MAX, z: f64::MAX }, hi: Vec3 { x: f64::MIN, y: f64::MIN, z: f64::MIN } };
    fn grow(&mut self, p: Vec3) {
        self.lo = Vec3::new(self.lo.x.min(p.x), self.lo.y.min(p.y), self.lo.z.min(p.z));
        self.hi = Vec3::new(self.hi.x.max(p.x), self.hi.y.max(p.y), self.hi.z.max(p.z));
    }
    fn join(&mut self, b: &Aabb) {
        self.grow(b.lo);
        self.grow(b.hi);
    }
    fn area(&self) -> f64 {
        let d = self.hi - self.lo;
        if d.x < 0.0 { 0.0 } else { 2.0 * (d.x * d.y + d.y * d.z + d.z * d.x) }
    }
    fn axis(v: Vec3, a: usize) -> f64 {
        match a {
            0 => v.x,
            1 => v.y,
            _ => v.z,
        }
    }
    /// Entry distance along the ray (with the inverse direction), if it is hit before `tmax`.
    fn hit(&self, o: Vec3, inv: Vec3, tmax: f64) -> Option<f64> {
        let (mut t0, mut t1) = (0.0f64, tmax);
        for a in 0..3 {
            let (o, inv, lo, hi) = (Aabb::axis(o, a), Aabb::axis(inv, a), Aabb::axis(self.lo, a), Aabb::axis(self.hi, a));
            let (mut ta, mut tb) = ((lo - o) * inv, (hi - o) * inv);
            if ta > tb {
                std::mem::swap(&mut ta, &mut tb);
            }
            // NaN (0 × ∞ on a slab face) never narrows the interval.
            if ta > t0 {
                t0 = ta;
            }
            if tb < t1 {
                t1 = tb;
            }
            if t0 > t1 {
                return None;
            }
        }
        Some(t0)
    }
}

#[derive(Clone, Copy, Debug)]
struct Node {
    b: Aabb,
    /// Leaf: first triangle in `prims`, and count (> 0). Inner: `start` is the left child (the
    /// right one follows it), and `count` 0.
    start: u32,
    count: u32,
}

/// A triangle prepared for intersection.
#[derive(Clone, Copy, Debug)]
struct Prim {
    v0: Vec3,
    e1: Vec3,
    e2: Vec3,
    /// Index into the scene's triangles.
    tri: u32,
}

pub struct Bvh {
    nodes: Vec<Node>,
    prims: Vec<Prim>,
}

/// A ray's hit: distance, triangle, barycentric (u, v).
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub t: f64,
    pub tri: u32,
    pub u: f64,
    pub v: f64,
}

const LEAF: usize = 4;
const BINS: usize = 12;

impl Bvh {
    pub fn new(tris: &[[Vec3; 3]]) -> Bvh {
        let mut prims: Vec<Prim> = tris
            .iter()
            .enumerate()
            .filter(|(_, t)| t.iter().all(|p| p.is_finite()))
            .map(|(i, t)| Prim { v0: t[0], e1: t[1] - t[0], e2: t[2] - t[0], tri: u32::try_from(i).unwrap_or(u32::MAX) })
            .collect();
        let centre = |p: &Prim| p.v0 + (p.e1 + p.e2) * (1.0 / 3.0);
        let bounds = |ps: &[Prim]| {
            let mut b = Aabb::EMPTY;
            for p in ps {
                b.grow(p.v0);
                b.grow(p.v0 + p.e1);
                b.grow(p.v0 + p.e2);
            }
            b
        };
        let mut nodes: Vec<Node> = Vec::with_capacity(prims.len() * 2 / LEAF + 1);
        if prims.is_empty() {
            return Bvh { nodes, prims };
        }
        nodes.push(Node { b: bounds(&prims), start: 0, count: 0 });
        // (node, first, end)
        let mut stack = vec![(0usize, 0usize, prims.len())];
        while let Some((ni, first, end)) = stack.pop() {
            let n = end - first;
            let slice = &mut prims[first..end];
            let b = bounds(slice);
            let mut split: Option<usize> = None;
            if n > LEAF {
                let mut cb = Aabb::EMPTY;
                for p in slice.iter() {
                    cb.grow(centre(p));
                }
                // The best plane among the bins of each axis.
                let mut best = (f64::MAX, 0usize, 0.0f64);
                for axis in 0..3 {
                    let (lo, hi) = (Aabb::axis(cb.lo, axis), Aabb::axis(cb.hi, axis));
                    if hi - lo <= 1e-300 {
                        continue;
                    }
                    let mut bins = [(Aabb::EMPTY, 0usize); BINS];
                    let k = BINS as f64 / (hi - lo);
                    for p in slice.iter() {
                        let i = (((Aabb::axis(centre(p), axis) - lo) * k) as usize).min(BINS - 1);
                        let (bb, c) = &mut bins[i];
                        bb.grow(p.v0);
                        bb.grow(p.v0 + p.e1);
                        bb.grow(p.v0 + p.e2);
                        *c += 1;
                    }
                    for cut in 1..BINS {
                        let (mut l, mut r) = (Aabb::EMPTY, Aabb::EMPTY);
                        let (mut nl, mut nr) = (0, 0);
                        for (i, (bb, c)) in bins.iter().enumerate() {
                            if *c == 0 {
                                continue;
                            }
                            if i < cut {
                                l.join(bb);
                                nl += c;
                            } else {
                                r.join(bb);
                                nr += c;
                            }
                        }
                        if nl == 0 || nr == 0 {
                            continue;
                        }
                        let cost = l.area() * nl as f64 + r.area() * nr as f64;
                        if cost < best.0 {
                            best = (cost, axis, lo + cut as f64 / k);
                        }
                    }
                }
                if best.0 < b.area() * n as f64 {
                    let (axis, at) = (best.1, best.2);
                    let mut i = 0;
                    for j in 0..n {
                        if Aabb::axis(centre(&slice[j]), axis) < at {
                            slice.swap(i, j);
                            i += 1;
                        }
                    }
                    if i > 0 && i < n {
                        split = Some(first + i);
                    }
                }
            }
            match split {
                None => {
                    if let Some(node) = nodes.get_mut(ni) {
                        *node = Node { b, start: first as u32, count: n as u32 };
                    }
                }
                Some(mid) => {
                    // Both children side by side: the left at `start`, the right after it.
                    let left = nodes.len();
                    nodes.push(Node { b: Aabb::EMPTY, start: 0, count: 0 });
                    nodes.push(Node { b: Aabb::EMPTY, start: 0, count: 0 });
                    if let Some(node) = nodes.get_mut(ni) {
                        *node = Node { b, start: left as u32, count: 0 };
                    }
                    stack.push((left + 1, mid, end));
                    stack.push((left, first, mid));
                }
            }
        }
        Bvh { nodes, prims }
    }

    fn walk(&self, o: Vec3, d: Vec3, tmax: f64, any: bool) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = Vec3::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
        let mut best: Option<Hit> = None;
        let mut limit = tmax;
        let mut stack: Vec<usize> = Vec::with_capacity(64);
        stack.push(0);
        while let Some(i) = stack.pop() {
            let Some(node) = self.nodes.get(i) else { continue };
            if node.b.hit(o, inv, limit).is_none() {
                continue;
            }
            if node.count > 0 {
                let (s, n) = (node.start as usize, node.count as usize);
                for p in self.prims.get(s..s + n).unwrap_or(&[]) {
                    if let Some((t, u, v)) = intersect(p, o, d)
                        && t < limit
                    {
                        limit = t;
                        best = Some(Hit { t, tri: p.tri, u, v });
                        if any {
                            return best;
                        }
                    }
                }
            } else {
                stack.push(node.start as usize + 1);
                stack.push(node.start as usize);
            }
        }
        best
    }

    /// The nearest hit along the ray before `tmax`.
    pub fn intersect(&self, o: Vec3, d: Vec3, tmax: f64) -> Option<Hit> {
        self.walk(o, d, tmax, false)
    }

    /// Whether anything is hit before `tmax`.
    pub fn occluded(&self, o: Vec3, d: Vec3, tmax: f64) -> bool {
        self.walk(o, d, tmax, true).is_some()
    }
}

/// Möller–Trumbore ray/triangle intersection, both sides.
fn intersect(p: &Prim, o: Vec3, d: Vec3) -> Option<(f64, f64, f64)> {
    let pv = d.cross(p.e2);
    let det = p.e1.dot(pv);
    if det.abs() < 1e-300 {
        return None;
    }
    let inv = 1.0 / det;
    let tv = o - p.v0;
    let u = tv.dot(pv) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qv = tv.cross(p.e1);
    let v = d.dot(qv) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = p.e2.dot(qv) * inv;
    (t > 0.0).then_some((t, u, v))
}

// ---------------- the renderer ----------------

/// What a render is asked for.
#[derive(Clone, Copy, Debug)]
pub struct TraceOptions {
    pub bounces: u32,
    /// Filter the samples' noise.
    pub denoise: bool,
    /// Only these pixels (x0, y0, x1, y1; RENDERCROP); the rest stays the background.
    pub crop: Option<(u32, u32, u32, u32)>,
    pub seed: u64,
}

impl TraceOptions {
    pub fn preset(p: Preset) -> TraceOptions {
        TraceOptions { bounces: p.bounces(), denoise: true, crop: None, seed: 1 }
    }
}

/// A progressive render: add passes, read the image at any time.
pub struct Render {
    scene: Scene,
    view: ShadeView,
    lit: Lighting,
    bvh: Bvh,
    opts: TraceOptions,
    /// Sum of samples and of their squares (linear), per pixel.
    sum: Vec<[f32; 3]>,
    sum2: Vec<f32>,
    /// The first hit's normal, depth and albedo (the filter's guides).
    normal: Vec<Vec3>,
    depth: Vec<f32>,
    albedo: Vec<[f32; 3]>,
    pub samples: u32,
    /// Seconds spent tracing.
    pub seconds: f64,
    /// Rays leave from slightly off a surface (relative to the scene's size).
    eps: f64,
    far: f64,
}

/// One path's light, and on the first bounce the surface it hit (normal, depth, albedo).
struct PathOut {
    rgb: [f32; 3],
    first: Option<(Vec3, f32, [f32; 3])>,
}

fn lum(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn lin(c: u8) -> f32 {
    (f32::from(c) / 255.0).powf(2.2)
}

/// Two unit directions square to `n` (unit) and to each other.
fn basis(n: Vec3) -> (Vec3, Vec3) {
    let a = if n.x.abs() < 0.9 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
    let t = a.cross(n).normalized();
    (t, n.cross(t))
}

/// A direction around `n` with density cos θ / π.
fn cosine_dir(n: Vec3, r1: f64, r2: f64) -> Vec3 {
    let (t, b) = basis(n);
    let phi = std::f64::consts::TAU * r1;
    let r = r2.sqrt();
    (t * (r * phi.cos()) + b * (r * phi.sin()) + n * (1.0 - r2).max(0.0).sqrt()).normalized()
}

/// GGX normal distribution D(h) for roughness α² `a2`, from cos θh.
fn ggx_d(nh: f64, a2: f64) -> f64 {
    let k = nh * nh * (a2 - 1.0) + 1.0;
    a2 / (std::f64::consts::PI * k * k).max(1e-12)
}

impl Render {
    /// A render of `scene` (its lighting decides the light; without one, a default daylight)
    /// through `view`.
    pub fn new(scene: Scene, view: ShadeView, lit: Lighting, opts: TraceOptions) -> Render {
        let tris: Vec<[Vec3; 3]> = scene.tris.iter().map(|t| t.p).collect();
        let bvh = Bvh::new(&tris);
        let (mut lo, mut hi) = (Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN));
        for p in tris.iter().flatten() {
            lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        let size = if lo.x <= hi.x { (hi - lo).len().max(1e-9) } else { 1.0 };
        let reach = [lo, hi].iter().map(|p| p.len()).fold(size, f64::max);
        let n = (view.width_px as usize) * (view.height_px as usize);
        Render {
            scene,
            view,
            lit,
            bvh,
            opts,
            sum: vec![[0.0; 3]; n],
            sum2: vec![0.0; n],
            normal: vec![Vec3::ZERO; n],
            depth: vec![f32::INFINITY; n],
            albedo: vec![[1.0; 3]; n],
            samples: 0,
            seconds: 0.0,
            eps: size * 1e-6,
            far: reach * 4.0 + size,
        }
    }

    /// The surface at a hit: material (texture applied), opacity, and geometric normal.
    fn surface(&self, h: &Hit) -> (Surface, f32, Vec3) {
        let Some(t) = self.scene.tris.get(h.tri as usize) else { return (Surface { base: [0.5; 3], roughness: 0.6, metallic: 0.0 }, 1.0, Vec3::Z) };
        let n = (t.p[1] - t.p[0]).cross(t.p[2] - t.p[0]).normalized();
        let w = [1.0 - h.u - h.v, h.u, h.v];
        match self.scene.surfaces.get(t.mat as usize) {
            Some(m) => {
                let mut base = m.base;
                if let Some(r) = m.texture.and_then(|i| self.scene.textures.get(i as usize)) {
                    let uv = [0, 1].map(|k| (f64::from(t.uv[0][k]) * w[0] + f64::from(t.uv[1][k]) * w[1] + f64::from(t.uv[2][k]) * w[2]) as f32);
                    let tex = crate::shade::sample(r, uv);
                    base = [base[0] * tex[0], base[1] * tex[1], base[2] * tex[2]];
                }
                (Surface { base, roughness: m.roughness, metallic: m.metallic }, m.opacity, n)
            }
            None => (Surface { base: [lin(t.color.0), lin(t.color.1), lin(t.color.2)], roughness: 0.6, metallic: 0.0 }, 1.0, n),
        }
    }

    /// The light along one path from the pixel's ray.
    fn path(&self, mut o: Vec3, mut d: Vec3, rng: &mut Rng, row_t: f32) -> PathOut {
        let mut l = [0.0f32; 3];
        let mut thr = [1.0f32; 3];
        let mut first = None;
        let mut bounce = 0u32;
        let mut steps = 0u32;
        while steps < self.opts.bounces + 16 {
            steps += 1;
            let Some(hit) = self.bvh.intersect(o, d, self.far * 2.0) else {
                // Leaving: the sky (behind the view, its gradient; elsewhere, the dome).
                let sky = if bounce == 0 { crate::light::background(&self.lit, row_t) } else { sky_only(&self.lit, d) };
                for i in 0..3 {
                    l[i] += thr[i] * sky[i];
                }
                break;
            };
            let p = o + d * hit.t;
            let (s, opacity, ng) = self.surface(&hit);
            // See-through: on through the surface, as often as it is clear.
            if opacity < 1.0 && rng.f() as f32 > opacity {
                o = p + d * self.eps;
                continue;
            }
            let n = if ng.dot(d) > 0.0 { ng * -1.0 } else { ng };
            let v = d * -1.0;
            if first.is_none() {
                first = Some((n, hit.t as f32, s.base));
            }
            let off = p + n * self.eps;
            let mut add = |rgb: [f32; 3], f: [f32; 3]| {
                for i in 0..3 {
                    l[i] += thr[i] * rgb[i] * f[i];
                }
            };
            // The sun, from a random point of its disc (half a degree across).
            if let Some((sd, rgb)) = self.lit.sun {
                let (t, b) = basis(sd);
                let (r, a) = ((rng.f()).sqrt() * 0.0045, rng.f() * std::f64::consts::TAU);
                let dir = (sd + t * (r * a.cos()) + b * (r * a.sin())).normalized();
                if n.dot(dir) > 0.0 && !self.bvh.occluded(off, dir, self.far * 2.0) {
                    add(rgb, brdf_cos(n, v, dir, &s));
                }
            }
            for light in &self.lit.lights {
                if let Some((dir, rgb, dist)) = incoming(light, p, self.lit.unit_m)
                    && n.dot(dir) > 0.0
                    && !self.bvh.occluded(off, dir, if dist.is_finite() { dist - self.eps * 2.0 } else { self.far * 2.0 })
                {
                    add(rgb, brdf_cos(n, v, dir, &s));
                }
            }
            bounce += 1;
            if bounce > self.opts.bounces {
                break;
            }
            // Bounce: glossy (GGX) or diffuse, chosen by how shiny the surface is.
            let rough = f64::from(s.roughness.clamp(0.04, 1.0));
            let a2 = (rough * rough).powi(2);
            let p_spec = (0.5 * (1.0 - rough) + 0.5 * f64::from(s.metallic)).clamp(0.1, 0.9);
            let dir = if rng.f() < p_spec {
                // A half vector from D(h) cos θh, mirrored.
                let (u1, u2) = (rng.f(), rng.f());
                let cos_t = ((1.0 - u1) / (1.0 + (a2 - 1.0) * u1)).max(0.0).sqrt();
                let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
                let phi = std::f64::consts::TAU * u2;
                let (t, b) = basis(n);
                let h = (t * (sin_t * phi.cos()) + b * (sin_t * phi.sin()) + n * cos_t).normalized();
                (h * (2.0 * v.dot(h)) - v).normalized()
            } else {
                cosine_dir(n, rng.f(), rng.f())
            };
            let nl = n.dot(dir);
            if nl <= 0.0 {
                break;
            }
            let h = (dir + v).normalized();
            let (nh, vh) = (n.dot(h).max(0.0), v.dot(h).max(1e-6));
            let pdf = (1.0 - p_spec) * nl / std::f64::consts::PI + p_spec * ggx_d(nh, a2) * nh / (4.0 * vh);
            if !(pdf.is_finite() && pdf > 1e-9) {
                break;
            }
            let f = brdf_cos(n, v, dir, &s);
            for i in 0..3 {
                thr[i] *= (f64::from(f[i]) / pdf) as f32;
            }
            // Russian roulette after a few bounces.
            if bounce >= 3 {
                let q = thr.iter().copied().fold(0.0f32, f32::max).clamp(0.05, 0.95);
                if rng.f() as f32 > q {
                    break;
                }
                thr = thr.map(|x| x / q);
            }
            if !thr.iter().all(|x| x.is_finite()) {
                break;
            }
            o = off;
            d = dir;
        }
        PathOut { rgb: l.map(|x| if x.is_finite() { x.max(0.0) } else { 0.0 }), first }
    }

    /// Add one sample to every pixel (in the crop).
    pub fn pass(&mut self) {
        let start = std::time::Instant::now();
        let v = self.view;
        let (w, h) = (v.width_px as usize, v.height_px as usize);
        let (x0, y0, x1, y1) = self.opts.crop.map_or((0, 0, w, h), |(a, b, c, d)| (a as usize, b as usize, (c as usize).min(w), (d as usize).min(h)));
        let px = v.height / f64::from(v.height_px);
        let sample = self.samples;
        let seed = self.opts.seed;
        let this = &*self;
        let rows: Vec<(usize, Vec<PathOut>)> = par_map(y0..y1, |y| {
            let mut rng = Rng::new(seed ^ (u64::from(sample) << 32), (y as u64) << 1);
            let row_t = 1.0 - (y as f32 + 0.5) / h as f32;
            let outs = (x0..x1)
                .map(|x| {
                    let (jx, jy) = (rng.f(), rng.f());
                    let sx = (x as f64 + jx - w as f64 / 2.0) * px + v.center.x;
                    let sy = (h as f64 / 2.0 - y as f64 - jy) * px + v.center.y;
                    let o = v.right * sx + v.up * sy + v.dir * this.far;
                    this.path(o, v.dir * -1.0, &mut rng, row_t)
                })
                .collect();
            (y, outs)
        });
        for (y, outs) in rows {
            for (k, out) in outs.into_iter().enumerate() {
                let i = y * w + x0 + k;
                if let (Some(s), Some(s2)) = (self.sum.get_mut(i), self.sum2.get_mut(i)) {
                    for (a, b) in s.iter_mut().zip(out.rgb) {
                        *a += b;
                    }
                    *s2 += lum(out.rgb).powi(2);
                }
                if sample == 0
                    && let Some((n, z, a)) = out.first
                {
                    self.normal[i] = n;
                    self.depth[i] = z;
                    self.albedo[i] = a;
                }
            }
        }
        self.samples += 1;
        self.seconds += start.elapsed().as_secs_f64();
    }

    /// Seconds per sample per pixel pass so far.
    pub fn seconds_per_sample(&self) -> f64 {
        if self.samples == 0 { 0.0 } else { self.seconds / f64::from(self.samples) }
    }

    /// The mean of the samples so far, linear, per pixel.
    pub fn mean(&self) -> Vec<[f32; 3]> {
        let k = 1.0 / self.samples.max(1) as f32;
        self.sum.iter().map(|c| c.map(|x| x * k)).collect()
    }

    /// The image so far: filtered (if asked) and tone mapped.
    pub fn image(&self) -> Raster {
        let (w, h) = (self.view.width_px, self.view.height_px);
        let mut c = self.mean();
        if self.opts.denoise && self.samples > 0 {
            let k = 1.0 / self.samples.max(1) as f32;
            // Each pixel's variance of the mean (luminance).
            let var: Vec<f32> = self.sum.iter().zip(&self.sum2).map(|(s, s2)| (s2 * k - (lum(*s) * k).powi(2)).max(0.0) * k).collect();
            c = denoise(&c, &var, &self.normal, &self.depth, &self.albedo, w as usize, h as usize);
        }
        let crop = self.opts.crop;
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for (i, px) in c.iter().enumerate() {
            let (x, y) = ((i % w as usize) as u32, (i / w as usize) as u32);
            let inside = crop.is_none_or(|(a, b, cc, d)| x >= a && x < cc && y >= b && y < d);
            let rgb = if inside { *px } else { crate::light::background(&self.lit, 1.0 - (y as f32 + 0.5) / h as f32) };
            let t = tonemap(rgb, self.lit.exposure);
            rgba.extend([t[0], t[1], t[2], 255]);
        }
        Raster::new(w, h, rgba).unwrap_or(Raster { width: 0, height: 0, rgba: Vec::new() })
    }
}

/// An edge-aware à-trous wavelet filter (Dammertz et al., "Edge-Avoiding À-Trous Wavelet
/// Transform for fast Global Illumination Filtering", 2010), our own implementation: five passes
/// of a 5 × 5 B-spline kernel spread 1, 2, 4, 8, 16 pixels apart, each neighbour weighted by how
/// alike its normal, depth and (relative to the noise) its light are. The light is filtered
/// without the surface colour (divided by the albedo) so textures stay sharp.
pub fn denoise(c: &[[f32; 3]], var: &[f32], n: &[Vec3], z: &[f32], albedo: &[[f32; 3]], w: usize, h: usize) -> Vec<[f32; 3]> {
    let k = [1.0 / 16.0, 1.0 / 4.0, 3.0 / 8.0, 1.0 / 4.0, 1.0 / 16.0];
    let demod = |i: usize, x: [f32; 3]| -> [f32; 3] {
        let a = albedo.get(i).copied().unwrap_or([1.0; 3]);
        [0, 1, 2].map(|j| x[j] / a[j].max(0.02))
    };
    let mut cur: Vec<[f32; 3]> = c.iter().enumerate().map(|(i, x)| demod(i, *x)).collect();
    let mut v: Vec<f32> = var.iter().enumerate().map(|(i, x)| x / lum(albedo.get(i).copied().unwrap_or([1.0; 3])).max(0.02).powi(2)).collect();
    for level in 0..5 {
        let step = 1usize << level;
        let src = cur.clone();
        let vsrc = v.clone();
        let out: Vec<([f32; 3], f32)> = par_map(0..w * h, |i| {
            let (x, y) = (i % w, i / w);
            let (ci, ni, zi) = (src[i], n[i], z[i]);
            if !zi.is_finite() {
                return (ci, vsrc[i]);
            }
            let sigma = (vsrc[i].max(0.0).sqrt() * 4.0).max(1e-4);
            let (mut acc, mut wsum, mut vacc) = ([0.0f32; 3], 0.0f32, 0.0f32);
            for (dy, ky) in k.iter().enumerate() {
                for (dx, kx) in k.iter().enumerate() {
                    let xx = x as isize + (dx as isize - 2) * step as isize;
                    let yy = y as isize + (dy as isize - 2) * step as isize;
                    if xx < 0 || yy < 0 || xx >= w as isize || yy >= h as isize {
                        continue;
                    }
                    let j = yy as usize * w + xx as usize;
                    if !z[j].is_finite() {
                        continue;
                    }
                    let wn = (ni.dot(n[j]).max(0.0) as f32).powi(32);
                    let wz = (-(zi - z[j]).abs() / (zi.abs().max(1e-3) * 0.02 * step as f32)).exp();
                    let wc = (-(lum(ci) - lum(src[j])).abs() / sigma).exp();
                    let wt = kx * ky * wn * wz * wc;
                    for c in 0..3 {
                        acc[c] += src[j][c] * wt;
                    }
                    vacc += vsrc[j] * wt * wt;
                    wsum += wt;
                }
            }
            if wsum <= 1e-12 { (ci, vsrc[i]) } else { (acc.map(|a| a / wsum), vacc / (wsum * wsum)) }
        });
        for (i, (c2, v2)) in out.into_iter().enumerate() {
            cur[i] = c2;
            v[i] = v2;
        }
    }
    cur.iter()
        .enumerate()
        .map(|(i, x)| {
            let a = albedo.get(i).copied().unwrap_or([1.0; 3]);
            [0, 1, 2].map(|j| x[j] * a[j].max(0.02))
        })
        .collect()
}

/// A raster as PNG or JPEG bytes.
pub fn encode(r: &Raster, jpeg: bool) -> Option<Vec<u8>> {
    let img = image::RgbaImage::from_raw(r.width, r.height, r.rgba.clone())?;
    let mut out = std::io::Cursor::new(Vec::new());
    if jpeg {
        let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
        rgb.write_to(&mut out, image::ImageFormat::Jpeg).ok()?;
    } else {
        img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    }
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light::{PointLight, Sky};
    use crate::shade::Tri;
    use cadaddy_color::Rgb;
    use cadaddy_doc::lighting::{Attenuation, LightKind};
    use cadaddy_geom::Vec2;

    fn quad(out: &mut Vec<Tri>, a: Vec3, b: Vec3, c: Vec3, d: Vec3, color: Rgb) {
        out.push(Tri::plain([a, b, c], color));
        out.push(Tri::plain([a, c, d], color));
    }

    /// A Cornell-style box: a unit room open towards −Y, red wall on the left, green on the
    /// right, a white block inside, a lamp under the ceiling.
    fn cornell(px: u32) -> Render {
        let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
        let (white, red, green) = (Rgb(200, 200, 200), Rgb(200, 30, 30), Rgb(30, 200, 30));
        let mut t = Vec::new();
        quad(&mut t, v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(1.0, 1.0, 0.0), v(0.0, 1.0, 0.0), white);
        quad(&mut t, v(0.0, 0.0, 1.0), v(0.0, 1.0, 1.0), v(1.0, 1.0, 1.0), v(1.0, 0.0, 1.0), white);
        quad(&mut t, v(0.0, 1.0, 0.0), v(1.0, 1.0, 0.0), v(1.0, 1.0, 1.0), v(0.0, 1.0, 1.0), white);
        quad(&mut t, v(0.0, 0.0, 0.0), v(0.0, 1.0, 0.0), v(0.0, 1.0, 1.0), v(0.0, 0.0, 1.0), red);
        quad(&mut t, v(1.0, 0.0, 0.0), v(1.0, 0.0, 1.0), v(1.0, 1.0, 1.0), v(1.0, 1.0, 0.0), green);
        // The block's top and front.
        quad(&mut t, v(0.3, 0.4, 0.3), v(0.6, 0.4, 0.3), v(0.6, 0.7, 0.3), v(0.3, 0.7, 0.3), white);
        quad(&mut t, v(0.3, 0.4, 0.0), v(0.6, 0.4, 0.0), v(0.6, 0.4, 0.3), v(0.3, 0.4, 0.3), white);
        let scene = Scene { tris: t, ..Scene::default() };
        let lit = Lighting {
            sun: None,
            sky: Sky { zenith: [0.0; 3], horizon: [0.0; 3], ground: [0.0; 3] },
            lights: vec![PointLight {
                pos: v(0.5, 0.5, 0.9),
                dir: v(0.0, 0.0, -1.0),
                rgb: [0.6; 3],
                kind: LightKind::Point,
                attenuation: Attenuation::InverseSquare,
                cos_inner: 0.0,
                cos_outer: -1.0,
            }],
            exposure: 1.0,
            unit_m: 1.0,
        };
        // Looking into the room from the open side.
        let view = ShadeView {
            dir: v(0.0, -1.0, 0.0),
            right: v(1.0, 0.0, 0.0),
            up: v(0.0, 0.0, 1.0),
            center: Vec2::new(0.5, 0.5),
            height: 1.0,
            width_px: px,
            height_px: px,
        };
        Render::new(scene, view, lit, TraceOptions { bounces: 4, denoise: false, crop: None, seed: 7 })
    }

    fn rmse(a: &[[f32; 3]], b: &[[f32; 3]]) -> f64 {
        let s: f64 = a.iter().zip(b).map(|(x, y)| (0..3).map(|i| f64::from(x[i] - y[i]).powi(2)).sum::<f64>()).sum();
        (s / (a.len() * 3) as f64).sqrt()
    }

    #[test]
    fn the_bvh_finds_what_testing_every_triangle_finds() {
        let mut rng = Rng::new(3, 5);
        let mut r = |k: f64| (rng.f() - 0.5) * k;
        let tris: Vec<[Vec3; 3]> = (0..500)
            .map(|_| {
                let c = Vec3::new(r(10.0), r(10.0), r(10.0));
                [c, c + Vec3::new(r(1.0), r(1.0), r(1.0)), c + Vec3::new(r(1.0), r(1.0), r(1.0))]
            })
            .collect();
        let bvh = Bvh::new(&tris);
        let prims: Vec<Prim> = tris.iter().enumerate().map(|(i, t)| Prim { v0: t[0], e1: t[1] - t[0], e2: t[2] - t[0], tri: i as u32 }).collect();
        let mut rng = Rng::new(9, 1);
        for _ in 0..2000 {
            let o = Vec3::new(rng.f() * 30.0 - 15.0, rng.f() * 30.0 - 15.0, 20.0);
            let d = (Vec3::new(rng.f() * 10.0 - 5.0, rng.f() * 10.0 - 5.0, 0.0) - o).normalized();
            let brute = prims.iter().filter_map(|p| intersect(p, o, d)).map(|h| h.0).fold(f64::MAX, f64::min);
            let fast = bvh.intersect(o, d, f64::MAX).map_or(f64::MAX, |h| h.t);
            assert!((brute - fast).abs() < 1e-9, "{brute} vs {fast}");
            assert_eq!(bvh.occluded(o, d, f64::MAX), brute < f64::MAX);
        }
    }

    #[test]
    fn a_cornell_box_converges() {
        let mut reference = cornell(12);
        for _ in 0..1024 {
            reference.pass();
        }
        let truth = reference.mean();
        let err = |spp: u32| {
            let mut r = cornell(12);
            r.opts.seed = 99;
            for _ in 0..spp {
                r.pass();
            }
            (rmse(&r.mean(), &truth), r.seconds_per_sample())
        };
        let (e4, _) = err(4);
        let (e64, t64) = err(64);
        // Monte Carlo: 16 × the samples, about a quarter of the error.
        assert!(e64 < e4 * 0.5, "4 spp {e4}, 64 spp {e64}");
        assert!(t64 > 0.0);
        // Light bounces off the red wall onto the left of the room: redder there than on the right.
        let m = &truth;
        let px = |x: usize, y: usize| m[y * 12 + x];
        let (left, right) = (px(1, 6), px(10, 6));
        assert!(left[0] > left[1] && right[1] > right[0], "{left:?} {right:?}");
        // The filter takes most of the noise off a few samples.
        let mut r = cornell(12);
        r.opts.seed = 5;
        for _ in 0..8 {
            r.pass();
        }
        let k = 1.0 / r.samples as f32;
        let var: Vec<f32> = r.sum.iter().zip(&r.sum2).map(|(s, s2)| (s2 * k - (lum(*s) * k).powi(2)).max(0.0) * k).collect();
        let filtered = denoise(&r.mean(), &var, &r.normal, &r.depth, &r.albedo, 12, 12);
        assert!(rmse(&filtered, &truth) < rmse(&r.mean(), &truth), "{} vs {}", rmse(&filtered, &truth), rmse(&r.mean(), &truth));
        // A crop renders only its pixels.
        let mut c = cornell(12);
        c.opts.crop = Some((2, 2, 6, 6));
        c.pass();
        assert!(c.sum[0] == [0.0; 3] && c.sum[3 * 12 + 3] != [0.0; 3]);
        assert!(encode(&c.image(), false).is_some_and(|b| b.starts_with(&[0x89, b'P', b'N', b'G'])));
        assert!(encode(&c.image(), true).is_some_and(|b| b.starts_with(&[0xFF, 0xD8])));
    }
}
