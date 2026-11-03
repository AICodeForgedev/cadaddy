//! The 3D view's model for big scenes: what [`crate::shade::scene`] draws, organised so a frame
//! touches only what it shows.
//!
//! - Block references whose blocks have surfaces are **instances**: each block's surfaces are
//!   built once (a *prototype*) and every reference is a placement of it, so a building's
//!   twenty thousand light fittings cost one fitting's triangles plus a matrix each.
//! - Each prototype has **levels of detail**: its full surfaces, a simplified copy (vertices
//!   merged on a grid), and its bounding box, chosen by how many pixels it covers.
//! - Everything else is split into **chunks** (a grid over the plan) with bounds.
//! - [`Model3::visible`] culls chunks and instances outside the view (the view box of the
//!   parallel projection), picks levels of detail, and orders them front to back so the depth test
//!   rejects hidden surfaces early.
//!
//! The model is built off the UI thread ([`build`] reports its progress).
//!
//! Implements: the simplified level by vertex clustering after Rossignac and Borrel,
//! "Multi-Resolution 3D Approximations for Rendering Complex Scenes" (Modeling in Computer
//! Graphics, 1993); view-volume culling of bounding boxes and front-to-back ordering as in
//! Akenine-Möller, Haines and Hoffman, *Real-Time Rendering*, 4th ed. (2018), chapter 19.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use cadaddy_color::Rgb;
use cadaddy_doc::{Drawing, Entity, EntityKind, EntityStore, Handle, Space};
use cadaddy_geom::{Mat4, Vec3};

use crate::shade::{Line3, Scene, ShadeView, Tri, scene, scene_clipped};

mod fid;
pub use fid::{
    AutoTiers, Elem, ElemClass, Fid, Force, Grid, GridCell, Held, MAX_MARKERS, NO_GROUP, STREAM_BUDGET, Show, Stream, StreamCell, StreamStep, Tier,
    TierCount,
};

/// A box in space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub lo: Vec3,
    pub hi: Vec3,
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb { lo: Vec3 { x: f64::MAX, y: f64::MAX, z: f64::MAX }, hi: Vec3 { x: f64::MIN, y: f64::MIN, z: f64::MIN } };
    pub fn grow(&mut self, p: Vec3) {
        self.lo = Vec3::new(self.lo.x.min(p.x), self.lo.y.min(p.y), self.lo.z.min(p.z));
        self.hi = Vec3::new(self.hi.x.max(p.x), self.hi.y.max(p.y), self.hi.z.max(p.z));
    }
    pub fn is_empty(&self) -> bool {
        self.lo.x > self.hi.x
    }
    pub fn center(&self) -> Vec3 {
        (self.lo + self.hi) * 0.5
    }
    pub fn diag(&self) -> f64 {
        if self.is_empty() { 0.0 } else { (self.hi - self.lo).len() }
    }
    /// The box of this box placed by `m`.
    pub fn placed(&self, m: &Mat4) -> Aabb {
        let mut b = Aabb::EMPTY;
        if self.is_empty() {
            return b;
        }
        for i in 0..8 {
            let p = Vec3::new(
                if i & 1 == 0 { self.lo.x } else { self.hi.x },
                if i & 2 == 0 { self.lo.y } else { self.hi.y },
                if i & 4 == 0 { self.lo.z } else { self.hi.z },
            );
            b.grow(m.apply(p));
        }
        b
    }
}

/// Whether a box can show in the view: its image on the view plane meets the view's rectangle
/// (a parallel projection sees everything in front of and behind its plane).
pub fn in_view(b: &Aabb, v: &ShadeView) -> bool {
    if b.is_empty() {
        return false;
    }
    let c = b.center();
    let h = (b.hi - b.lo) * 0.5;
    let reach = |axis: Vec3| h.x * axis.x.abs() + h.y * axis.y.abs() + h.z * axis.z.abs();
    let hh = v.height / 2.0;
    let hw = hh * f64::from(v.width_px) / f64::from(v.height_px.max(1));
    (c.dot(v.right) - v.center.x).abs() <= hw + reach(v.right) && (c.dot(v.up) - v.center.y).abs() <= hh + reach(v.up)
}

/// A run of the base scene's triangles and lines in one cell of the plan, and its bounds.
#[derive(Clone, Debug)]
pub struct Chunk {
    pub tris: std::ops::Range<usize>,
    pub lines: std::ops::Range<usize>,
    pub bounds: Aabb,
    /// The fidelity tiers it draws at ([`Show::ALWAYS`] in a model built without fidelity).
    pub show: Show,
}

/// A block's surfaces at three levels of detail (full, simplified, box), in block coordinates.
#[derive(Clone, Debug)]
pub struct Proto {
    pub name: String,
    pub lods: [Scene; 3],
    pub bounds: Aabb,
}

/// A reference to a prototype.
#[derive(Clone, Copy, Debug)]
pub struct Instance {
    pub proto: u32,
    /// Block coordinates to the drawing's.
    pub m: Mat4,
    pub bounds: Aabb,
    /// The fidelity tiers it draws at.
    pub show: Show,
}

/// The 3D view's model.
#[derive(Clone, Debug, Default)]
pub struct Model3 {
    /// Everything but the instances, its triangles and lines ordered by chunk.
    pub base: Scene,
    pub chunks: Vec<Chunk>,
    pub protos: Vec<Proto>,
    pub instances: Vec<Instance>,
    pub bounds: Aabb,
    /// The elements by fidelity tier, when built with a classifier ([`build_fid`]).
    pub fid: Option<std::sync::Arc<Fid>>,
}

impl Default for Aabb {
    fn default() -> Self {
        Aabb::EMPTY
    }
}

/// What one frame draws: base chunks and, per prototype and level, instances; front to back.
#[derive(Clone, Debug, Default)]
pub struct Visible {
    pub chunks: Vec<usize>,
    /// (prototype, level, instance indices).
    pub instances: Vec<(u32, u8, Vec<u32>)>,
    pub tris: usize,
    /// The fidelity tier drawn.
    pub tier: Tier,
    /// Elements shown as markers (indices in [`Fid::elems`]): the ones a search forces into the
    /// view that its tier doesn't draw.
    pub markers: Vec<u32>,
}

/// Pixels a prototype covers below which it draws simplified, and below which as a box.
pub const LOD_SIMPLE_PX: f64 = 48.0;
pub const LOD_BOX_PX: f64 = 6.0;

impl Model3 {
    /// What the view `v` shows, levels of detail chosen and nearest first (every tier).
    pub fn visible(&self, v: &ShadeView) -> Visible {
        self.visible_at(v, Tier::Detail, None)
    }

    /// What the view `v` shows at fidelity `tier`: the chunks and instances of that tier in view,
    /// levels of detail chosen, nearest first; elements `force` names that the tier hides, as
    /// markers.
    pub fn visible_at(&self, v: &ShadeView, tier: Tier, force: Option<&Force>) -> Visible {
        let px = v.height / f64::from(v.height_px.max(1));
        let depth = |b: &Aabb| -b.center().dot(v.dir);
        let mut chunks: Vec<(f64, usize)> =
            self.chunks.iter().enumerate().filter(|(_, c)| c.show.at(tier) && in_view(&c.bounds, v)).map(|(i, c)| (depth(&c.bounds), i)).collect();
        chunks.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut tris = chunks.iter().filter_map(|(_, i)| self.chunks.get(*i)).map(|c| c.tris.len()).sum::<usize>();
        let mut groups: std::collections::BTreeMap<(u32, u8), Vec<(f64, u32)>> = std::collections::BTreeMap::new();
        for (i, inst) in self.instances.iter().enumerate() {
            if !inst.show.at(tier) || !in_view(&inst.bounds, v) {
                continue;
            }
            let size = inst.bounds.diag() / px;
            let lod = if size < LOD_BOX_PX {
                2
            } else if size < LOD_SIMPLE_PX {
                1
            } else {
                0
            };
            groups.entry((inst.proto, lod)).or_default().push((depth(&inst.bounds), i as u32));
        }
        let mut instances = Vec::new();
        for ((p, lod), mut list) in groups {
            list.sort_by(|a, b| a.0.total_cmp(&b.0));
            if let Some(proto) = self.protos.get(p as usize) {
                tris += proto.lods[lod as usize].tris.len() * list.len();
            }
            instances.push((p, lod, list.into_iter().map(|x| x.1).collect()));
        }
        // Prototypes nearest first too (by their nearest instance).
        instances.sort_by(|a, b| {
            let near = |g: &(u32, u8, Vec<u32>)| g.2.first().and_then(|i| self.instances.get(*i as usize)).map_or(0.0, |x| depth(&x.bounds));
            near(a).total_cmp(&near(b))
        });
        let markers = match (&self.fid, force) {
            (Some(f), Some(force)) => f.forced(v, tier, force),
            _ => Vec::new(),
        };
        Visible { chunks: chunks.into_iter().map(|c| c.1).collect(), instances, tris, tier, markers }
    }

    /// What a GPU holds for drawing up to `tier`: the triangles and lines of the base chunks that
    /// draw at or below it, and of every prototype's levels of detail (streamed cells come on top).
    pub fn loaded_at(&self, tier: Tier) -> (usize, usize) {
        let (mut t, mut l) = (0, 0);
        for c in self.chunks.iter().filter(|c| c.show.from <= tier) {
            t += c.tris.len();
            l += c.lines.len();
        }
        for p in &self.protos {
            t += p.lods.iter().map(|s| s.tris.len()).sum::<usize>();
            l += p.lods.iter().map(|s| s.lines.len()).sum::<usize>();
        }
        (t, l)
    }

    /// Triangles in the whole model, instances counted as placed.
    pub fn total_tris(&self) -> usize {
        self.base.tris.len() + self.instances.iter().filter_map(|i| self.protos.get(i.proto as usize)).map(|p| p.lods[0].tris.len()).sum::<usize>()
    }

    /// Bytes the model holds (triangles, lines, matrices; the fidelity index).
    pub fn bytes(&self) -> usize {
        scene_bytes(&self.base)
            + self.protos.iter().map(|p| p.lods.iter().map(scene_bytes).sum::<usize>()).sum::<usize>()
            + self.instances.len() * std::mem::size_of::<Instance>()
            + self.fid.as_ref().map_or(0, |f| f.bytes())
    }

    /// The model as one flat scene (every instance placed): for the CPU rasterizer.
    pub fn flatten(&self) -> Scene {
        let mut out = self.base.clone();
        for i in &self.instances {
            let Some(p) = self.protos.get(i.proto as usize) else { continue };
            for t in &p.lods[0].tris {
                if out.tris.len() >= crate::shade::MAX_SCENE {
                    return out;
                }
                out.tris.push(Tri { p: t.p.map(|q| i.m.apply(q)), ..*t });
            }
            for l in &p.lods[0].lines {
                out.lines.push(Line3 { a: i.m.apply(l.a), b: i.m.apply(l.b), ..*l });
            }
        }
        out
    }

    /// Only the visible parts, at their LOD levels, as a flat scene for the CPU rasterizer.
    pub fn flatten_visible(&self, vis: &Visible) -> Scene {
        let mut out = Scene {
            lighting: self.base.lighting.clone(),
            surfaces: self.base.surfaces.clone(),
            textures: self.base.textures.clone(),
            ..Scene::default()
        };
        for &ci in &vis.chunks {
            if let Some(c) = self.chunks.get(ci) {
                out.tris.extend_from_slice(&self.base.tris[c.tris.clone()]);
                out.lines.extend_from_slice(&self.base.lines[c.lines.clone()]);
            }
        }
        for (proto_idx, lod, indices) in &vis.instances {
            let Some(proto) = self.protos.get(*proto_idx as usize) else { continue };
            let lod_scene = &proto.lods[*lod as usize];
            for &inst_idx in indices {
                let Some(inst) = self.instances.get(inst_idx as usize) else { continue };
                for t in &lod_scene.tris {
                    if out.tris.len() >= crate::shade::MAX_SCENE {
                        return out;
                    }
                    out.tris.push(Tri { p: t.p.map(|q| inst.m.apply(q)), ..*t });
                }
                for l in &lod_scene.lines {
                    out.lines.push(Line3 { a: inst.m.apply(l.a), b: inst.m.apply(l.b), ..*l });
                }
            }
        }
        out
    }
}

/// A reference's placement: the block's base to the insertion point, scaled, turned about Z.
fn insert_matrix(base: Vec3, at: Vec3, scale: Vec3, rot: f64) -> Mat4 {
    let (s, c) = rot.sin_cos();
    let (sx, sy, sz) = (scale.x, scale.y, scale.z);
    let t = Vec3::new(-base.x * sx, -base.y * sy, -base.z * sz);
    Mat4 {
        m: [
            [c * sx, -s * sy, 0.0, c * t.x - s * t.y + at.x],
            [s * sx, c * sy, 0.0, s * t.x + c * t.y + at.y],
            [0.0, 0.0, sz, t.z + at.z],
            [0.0, 0.0, 0.0, 1.0],
        ],
    }
}

/// `d` with its model space holding only `entities`.
fn with_model(d: &Drawing, entities: impl Iterator<Item = cadaddy_doc::Entity>) -> Drawing {
    let mut t = d.clone();
    let mut store = EntityStore::new();
    for e in entities {
        store.push(e);
    }
    t.model = store;
    t
}

/// A simplified copy: corners merged into a grid of `cells` across the bounds, triangles that
/// collapse dropped.
pub fn simplify(s: &Scene, bounds: &Aabb, cells: f64) -> Scene {
    let size = bounds.diag().max(1e-12) / cells.max(1.0);
    let q = |p: Vec3| ((p.x / size).round() as i64, (p.y / size).round() as i64, (p.z / size).round() as i64);
    // Each cell's corners averaged.
    let mut sums: std::collections::HashMap<(i64, i64, i64), (Vec3, f64)> = std::collections::HashMap::new();
    for t in &s.tris {
        for p in t.p {
            let e = sums.entry(q(p)).or_insert((Vec3::ZERO, 0.0));
            e.0 = e.0 + p;
            e.1 += 1.0;
        }
    }
    let at = |p: Vec3| sums.get(&q(p)).map_or(p, |(sum, n)| *sum * (1.0 / n));
    let mut out = Scene { lighting: None, surfaces: s.surfaces.clone(), textures: s.textures.clone(), ..Scene::default() };
    let mut seen: std::collections::HashSet<[(i64, i64, i64); 3]> = std::collections::HashSet::new();
    for t in &s.tris {
        let k = t.p.map(q);
        if k[0] == k[1] || k[1] == k[2] || k[0] == k[2] {
            continue;
        }
        let mut sorted = k;
        sorted.sort_unstable();
        if !seen.insert(sorted) {
            continue;
        }
        out.tris.push(Tri { p: t.p.map(at), ..*t });
    }
    out
}

/// The box of a scene's triangles as 12 triangles, in the colour most of its surface has.
pub fn box_of(s: &Scene, b: &Aabb) -> Scene {
    let mut out = Scene { lighting: None, surfaces: s.surfaces.clone(), ..Scene::default() };
    if b.is_empty() {
        return out;
    }
    let first = s.tris.first().copied();
    let v = |x: bool, y: bool, z: bool| Vec3::new(if x { b.hi.x } else { b.lo.x }, if y { b.hi.y } else { b.lo.y }, if z { b.hi.z } else { b.lo.z });
    let quads = [
        [(false, false, false), (false, true, false), (true, true, false), (true, false, false)],
        [(false, false, true), (true, false, true), (true, true, true), (false, true, true)],
        [(false, false, false), (true, false, false), (true, false, true), (false, false, true)],
        [(false, true, false), (false, true, true), (true, true, true), (true, true, false)],
        [(false, false, false), (false, false, true), (false, true, true), (false, true, false)],
        [(true, false, false), (true, true, false), (true, true, true), (true, false, true)],
    ];
    let proto = first.unwrap_or_else(|| Tri::plain([Vec3::ZERO; 3], Rgb(200, 200, 200)));
    for q in quads {
        let c = q.map(|(x, y, z)| v(x, y, z));
        out.tris.push(Tri { p: [c[0], c[1], c[2]], ..proto });
        out.tris.push(Tri { p: [c[0], c[2], c[3]], ..proto });
    }
    out
}

fn bounds_of(s: &Scene) -> Aabb {
    let mut b = Aabb::EMPTY;
    for p in s.tris.iter().flat_map(|t| t.p).chain(s.lines.iter().flat_map(|l| [l.a, l.b])) {
        b.grow(p);
    }
    b
}

/// Bytes a scene's triangles and lines take.
pub(crate) fn scene_bytes(s: &Scene) -> usize {
    s.tris.len() * std::mem::size_of::<Tri>() + s.lines.len() * std::mem::size_of::<Line3>()
}

/// The base scene's triangles and lines regrouped into a grid of chunks over the plan: by the tiers they draw at
/// (`tri_show`, `line_show`, parallel to them; missing: always), then by a grid over the plan.
fn chunked_by(mut s: Scene, tri_show: &[Show], line_show: &[Show]) -> (Scene, Vec<Chunk>) {
    let b = bounds_of(&s);
    if b.is_empty() {
        return (s, Vec::new());
    }
    let n = ((s.tris.len() + s.lines.len()) as f64 / 4000.0).sqrt().ceil().clamp(1.0, 32.0) as usize;
    let (w, h) = ((b.hi.x - b.lo.x).max(1e-12), (b.hi.y - b.lo.y).max(1e-12));
    let cell = |p: Vec3| {
        let i = (((p.x - b.lo.x) / w * n as f64) as usize).min(n - 1);
        let j = (((p.y - b.lo.y) / h * n as f64) as usize).min(n - 1);
        j * n + i
    };
    let shows: Vec<Show> = {
        let mut v: Vec<Show> = tri_show.iter().chain(line_show).copied().collect();
        v.push(Show::ALWAYS);
        v.sort();
        v.dedup();
        v
    };
    let band = |sh: Show| shows.binary_search(&sh).unwrap_or(0);
    let key = |sh: Option<&Show>, c: usize| band(sh.copied().unwrap_or(Show::ALWAYS)) * n * n + c;
    // The triangles' owners (LIGHTBAKE's lookup) move with them, the lines' too.
    let owners = std::mem::take(&mut s.owners);
    let keep_owners = owners.len() == s.tris.len();
    let line_owners = std::mem::take(&mut s.line_owners);
    let keep_line_owners = line_owners.len() == s.lines.len();
    let mut tris: Vec<(usize, Tri, cadaddy_doc::Handle)> = s
        .tris
        .drain(..)
        .enumerate()
        .map(|(k, t)| (key(tri_show.get(k), cell((t.p[0] + t.p[1] + t.p[2]) * (1.0 / 3.0))), t, owners.get(k).copied().unwrap_or_default()))
        .collect();
    tris.sort_by_key(|x| x.0);
    let mut lines: Vec<(usize, Line3, cadaddy_doc::Handle)> = s
        .lines
        .drain(..)
        .enumerate()
        .map(|(k, l)| (key(line_show.get(k), cell((l.a + l.b) * 0.5)), l, line_owners.get(k).copied().unwrap_or_default()))
        .collect();
    lines.sort_by_key(|x| x.0);
    let mut chunks: Vec<Chunk> = (0..shows.len() * n * n)
        .map(|k| Chunk { tris: 0..0, lines: 0..0, bounds: Aabb::EMPTY, show: shows.get(k / (n * n)).copied().unwrap_or(Show::ALWAYS) })
        .collect();
    let (mut ti, mut li) = (0usize, 0usize);
    for (c, chunk) in chunks.iter_mut().enumerate() {
        let start = ti;
        while let Some(t) = tris.get(ti).filter(|t| t.0 == c) {
            for p in t.1.p {
                chunk.bounds.grow(p);
            }
            ti += 1;
        }
        chunk.tris = start..ti;
        let start = li;
        while let Some(l) = lines.get(li).filter(|l| l.0 == c) {
            chunk.bounds.grow(l.1.a);
            chunk.bounds.grow(l.1.b);
            li += 1;
        }
        chunk.lines = start..li;
    }
    if keep_owners {
        s.owners = tris.iter().map(|x| x.2).collect();
    }
    if keep_line_owners {
        s.line_owners = lines.iter().map(|x| x.2).collect();
    }
    s.tris = tris.into_iter().map(|x| x.1).collect();
    s.lines = lines.into_iter().map(|x| x.1).collect();
    chunks.retain(|c| !c.tris.is_empty() || !c.lines.is_empty());
    (s, chunks)
}

/// Where a placed box is against clipping planes (each keeps its positive side).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Inside,
    Outside,
    Cut,
}

/// Whether `b` is wholly kept by `planes` (none: inside), wholly behind one of them, or cut.
fn box_side(b: &Aabb, planes: &[cadaddy_doc::SectionPlane]) -> Side {
    if planes.is_empty() || b.is_empty() {
        return Side::Inside;
    }
    let corners = [
        Vec3::new(b.lo.x, b.lo.y, b.lo.z),
        Vec3::new(b.hi.x, b.lo.y, b.lo.z),
        Vec3::new(b.lo.x, b.hi.y, b.lo.z),
        Vec3::new(b.hi.x, b.hi.y, b.lo.z),
        Vec3::new(b.lo.x, b.lo.y, b.hi.z),
        Vec3::new(b.hi.x, b.lo.y, b.hi.z),
        Vec3::new(b.lo.x, b.hi.y, b.hi.z),
        Vec3::new(b.hi.x, b.hi.y, b.hi.z),
    ];
    let mut inside = true;
    for pl in planes {
        let kept = corners.iter().filter(|c| pl.side(**c) >= 0.0).count();
        if kept == 0 {
            return Side::Outside;
        }
        if kept < corners.len() {
            inside = false;
        }
    }
    if inside { Side::Inside } else { Side::Cut }
}

/// Most block references made instances (beyond them, references draw as part of the base).
pub const MAX_INSTANCES: usize = 2_000_000;

/// Build the model of `space` (curves to `tol`, colour 7 for `bg`), `progress` counting the steps
/// done out of the total stored in `total`.
pub fn build(d: &Drawing, space: &Space, tol: f64, bg: Rgb, progress: &AtomicUsize, total: &AtomicUsize) -> Model3 {
    build_clipped(d, space, tol, bg, progress, total, &[])
}

/// Like [`build`] but with extra clipping planes (e.g. from a coordination section box).
pub fn build_clipped(
    d: &Drawing,
    space: &Space,
    tol: f64,
    bg: Rgb,
    progress: &AtomicUsize,
    total: &AtomicUsize,
    extra_planes: &[cadaddy_doc::SectionPlane],
) -> Model3 {
    build_impl(d, space, tol, bg, progress, total, extra_planes, None)
}

/// What a model element is for fidelity ([`build_fid`]); `None`: not an element (drawn always).
pub type Classifier<'a> = dyn Fn(&Drawing, &Entity) -> Option<ElemClass> + Sync + 'a;

/// How [`build_fid`] holds a model's elements.
#[derive(Clone, Copy, Debug)]
pub struct FidOptions {
    /// Elements of this tier and finer are streamed ([`Stream`]) rather than held, unless
    /// repeated (then they are instances).
    pub stream_from: Tier,
    /// Identical meshes repeated this many times or more are drawn as instances of one prototype.
    pub min_repeat: usize,
}

impl Default for FidOptions {
    fn default() -> Self {
        FidOptions { stream_from: Tier::Room, min_repeat: 4 }
    }
}

/// Like [`build_clipped`], with each element of the model given a fidelity tier by `classify`
/// ([`Model3::fid`]): its chunks and instances draw at their tiers ([`Model3::visible_at`]);
/// identical meshes repeated [`FidOptions::min_repeat`] times become instances of one prototype;
/// the meshes of the finer tiers stay out of the model until a view streams their cells in
/// ([`Stream`]); a run's pieces finer than the view draw as one line along the run, from the
/// tier the classifier says.
#[allow(clippy::too_many_arguments)]
pub fn build_fid(
    d: Arc<Drawing>,
    space: &Space,
    tol: f64,
    bg: Rgb,
    progress: &AtomicUsize,
    total: &AtomicUsize,
    extra_planes: &[cadaddy_doc::SectionPlane],
    classify: &Classifier<'_>,
    opts: FidOptions,
) -> Model3 {
    build_impl(&d, space, tol, bg, progress, total, extra_planes, Some((classify, opts)))
}

/// A mesh's shape up to where it stands: its layer, colour and material, faces and corners
/// relative to its first (to 1e-4 units). Equal keys draw alike once moved into place.
fn mesh_key(e: &Entity, m: &cadaddy_doc::Mesh) -> Option<(u64, Vec3)> {
    use std::hash::{Hash, Hasher};
    let o = *m.vertices.first()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    e.common.layer.to_ascii_lowercase().hash(&mut h);
    e.common.color.hash(&mut h);
    format!("{:?}|{:?}", e.common.material, e.common.transparency).hash(&mut h);
    m.vertices.len().hash(&mut h);
    for v in &m.vertices {
        let q = |x: f64| (x / 1e-4).round();
        let dv = *v - o;
        if !dv.is_finite() {
            return None;
        }
        [q(dv.x) as i64, q(dv.y) as i64, q(dv.z) as i64].hash(&mut h);
    }
    for f in &m.faces {
        f.v.hash(&mut h);
    }
    Some((h.finish(), o))
}

/// A mesh's corners' box.
fn mesh_bounds(m: &cadaddy_doc::Mesh) -> Aabb {
    let mut b = Aabb::EMPTY;
    for v in m.vertices.iter().filter(|v| v.is_finite()) {
        b.grow(*v);
    }
    b
}

/// A mesh's faces as triangles.
fn mesh_tris(m: &cadaddy_doc::Mesh) -> u32 {
    m.faces.iter().map(|f| f.v.len().saturating_sub(2) as u32).fold(0u32, u32::saturating_add)
}

/// A translation by `o`.
fn translation(o: Vec3) -> Mat4 {
    Mat4 { m: [[1.0, 0.0, 0.0, o.x], [0.0, 1.0, 0.0, o.y], [0.0, 0.0, 1.0, o.z], [0.0, 0.0, 0.0, 1.0]] }
}

/// The line a run's piece shows as: along its box's longest side, through its middle.
fn run_line(b: &Aabb) -> Option<(Vec3, Vec3)> {
    if b.is_empty() {
        return None;
    }
    let (c, h) = (b.center(), (b.hi - b.lo) * 0.5);
    let axis = if h.x >= h.y && h.x >= h.z {
        Vec3::new(h.x, 0.0, 0.0)
    } else if h.y >= h.z {
        Vec3::new(0.0, h.y, 0.0)
    } else {
        Vec3::new(0.0, 0.0, h.z)
    };
    (axis.len() > 1e-9).then_some((c - axis, c + axis))
}

/// An element's colour as set on it (the BIM view colours elements by trade).
fn own_rgb(e: &Entity) -> Rgb {
    match e.common.color {
        cadaddy_color::Color::True(c) => c,
        _ => Rgb(160, 160, 160),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_impl(
    d: &Drawing,
    space: &Space,
    tol: f64,
    bg: Rgb,
    progress: &AtomicUsize,
    total: &AtomicUsize,
    extra_planes: &[cadaddy_doc::SectionPlane],
    fid: Option<(&Classifier<'_>, FidOptions)>,
) -> Model3 {
    let Some(store) = d.space(space) else { return Model3::default() };
    // The blocks referenced from the space, each built once.
    let mut names: Vec<String> = Vec::new();
    for e in store.iter() {
        if let EntityKind::Insert(i) = &e.kind
            && d.is_visible(e)
            && !names.iter().any(|n| n.eq_ignore_ascii_case(&i.block))
        {
            names.push(i.block.clone());
        }
    }
    total.store(names.len() + 2 + usize::from(fid.is_some()), Ordering::Relaxed);
    let mut protos: Vec<Proto> = Vec::new();
    let mut proto_of: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for n in &names {
        progress.fetch_add(1, Ordering::Relaxed);
        let Some(b) = d.block(n) else { continue };
        if b.xref_path.is_some() {
            continue;
        }
        // Prototypes are whole; a section box drops the instances outside it and draws the ones
        // it cuts through, placed and clipped, in the base scene (below).
        let full = scene(&with_model(d, b.entities.iter().map(|x| (**x).clone())), &Space::Model, tol, bg);
        // Only blocks with surfaces: flat symbols keep drawing as the plan does.
        if full.tris.is_empty() {
            continue;
        }
        let bounds = bounds_of(&full);
        let simple = simplify(&full, &bounds, 5.0);
        let boxed = box_of(&full, &bounds);
        proto_of.insert(n.to_ascii_lowercase(), protos.len() as u32);
        protos.push(Proto { name: n.clone(), lods: [full, simple, boxed], bounds });
    }
    // The elements by fidelity, as they are met.
    let mut elems: Vec<Elem> = Vec::new();
    let (mut kinds, mut levels, mut groups) = (fid::Names::default(), fid::Names::default(), fid::Names::default());
    // Each element's run line: from the tier it shows from.
    let mut line_from: Vec<Option<Tier>> = Vec::new();
    // Meshes by shape (instancing candidates): (element, entity, first corner).
    let mut shapes: std::collections::HashMap<u64, Vec<(u32, Arc<Entity>, Vec3)>> = std::collections::HashMap::new();
    let mut instances = Vec::new();
    let mut rest = Vec::new();
    // The tiers of the objects in `rest` that draw at some only.
    let mut rest_show: std::collections::HashMap<Handle, Show> = std::collections::HashMap::new();
    // Placements the section box cuts through: (reference, prototype, placement).
    let mut cut_through: Vec<(cadaddy_doc::Handle, u32, Mat4)> = Vec::new();
    for e in store.iter() {
        let class = match fid {
            Some((classify, _)) if d.is_visible(e) => classify(d, e),
            _ => None,
        };
        let show = class.as_ref().map_or(Show::ALWAYS, |c| Show::from(c.tier));
        let mut elem = class.as_ref().map(|c| Elem {
            handle: e.handle,
            tier: c.tier,
            kind: kinds.id(&c.kind),
            level: levels.id(&c.level),
            group: c.group.as_deref().filter(|g| !g.is_empty()).map_or(NO_GROUP, |g| groups.id(g)),
            bounds: Aabb::EMPTY,
            held: Held::Base,
            tris: 0,
        });
        if let EntityKind::Insert(i) = &e.kind
            && let Some(&p) = proto_of.get(&i.block.to_ascii_lowercase())
            && let Some(b) = d.block(&i.block)
            && instances.len() < MAX_INSTANCES
        {
            if !d.is_visible(e) {
                continue;
            }
            let (cols, rows) = (i.cols.clamp(1, 1000), i.rows.clamp(1, 1000));
            for r in 0..rows {
                for c in 0..cols {
                    let (s, co) = i.rotation.sin_cos();
                    let (dx, dy) = (f64::from(c) * i.col_spacing, f64::from(r) * i.row_spacing);
                    let at = i.insert + Vec3::new(co * dx - s * dy, s * dx + co * dy, 0.0);
                    let m = insert_matrix(b.base, at, i.scale, i.rotation);
                    let pr = protos.get(p as usize);
                    let bounds = pr.map_or(Aabb::EMPTY, |pr| pr.bounds.placed(&m));
                    if let Some(el) = elem.as_mut() {
                        el.bounds.grow(bounds.lo);
                        el.bounds.grow(bounds.hi);
                        el.tris = el.tris.saturating_add(pr.map_or(0, |pr| pr.lods[0].tris.len() as u32));
                        el.held = Held::Instance;
                    }
                    match box_side(&bounds, extra_planes) {
                        Side::Inside => instances.push(Instance { proto: p, m, bounds, show }),
                        Side::Outside => {}
                        Side::Cut => cut_through.push((e.handle, p, m)),
                    }
                }
            }
            if let (Some(el), Some(c)) = (elem, class) {
                elems.push(el);
                line_from.push(c.line_from);
            }
            continue;
        }
        if let (Some(mut el), Some(c)) = (elem, class.as_ref()) {
            let idx = elems.len() as u32;
            if let EntityKind::Mesh(m) = &e.kind
                && let Some((key, o)) = mesh_key(e, m)
            {
                el.bounds = mesh_bounds(m);
                el.tris = mesh_tris(m);
                elems.push(el);
                line_from.push(c.line_from);
                shapes.entry(key).or_default().push((idx, Arc::clone(e), o));
                continue;
            }
            elems.push(el);
            line_from.push(c.line_from);
            if c.tier != Tier::Building {
                rest_show.insert(e.handle, Show::from(c.tier));
            }
        }
        rest.push((**e).clone());
    }
    // Meshes: repeated shapes are instances of one prototype; the finer tiers' others stream;
    // the rest (and what the section box cuts) are triangles in the base.
    let mut streamed: Vec<Arc<Entity>> = Vec::new();
    if let Some((_, opts)) = fid {
        progress.fetch_add(1, Ordering::Relaxed);
        let mut shapes: Vec<(u64, Vec<(u32, Arc<Entity>, Vec3)>)> = shapes.into_iter().collect();
        shapes.sort_by_key(|(_, list)| list.first().map_or(0, |x| x.0));
        for (_, list) in shapes {
            let repeated = list.len() >= opts.min_repeat.max(2) && instances.len() + list.len() <= MAX_INSTANCES;
            let mut proto: Option<u32> = None;
            if repeated && let Some((_, first, o)) = list.first() {
                // The prototype: the first one moved so its first corner is at the origin.
                let mut moved = (**first).clone();
                if let EntityKind::Mesh(m) = &mut moved.kind {
                    for v in &mut m.vertices {
                        *v = *v - *o;
                    }
                }
                let full = scene(&with_model(d, std::iter::once(moved)), &Space::Model, tol, bg);
                if !full.tris.is_empty() {
                    let bounds = bounds_of(&full);
                    let simple = simplify(&full, &bounds, 5.0);
                    let boxed = box_of(&full, &bounds);
                    proto = Some(protos.len() as u32);
                    protos.push(Proto { name: format!("*mesh{}", first.handle.0), lods: [full, simple, boxed], bounds });
                }
            }
            for (idx, e, o) in list {
                let Some(el) = elems.get_mut(idx as usize) else { continue };
                let side = box_side(&el.bounds, extra_planes);
                if side == Side::Outside {
                    el.held = Held::Outside;
                    continue;
                }
                if let (Some(p), Side::Inside) = (proto, side) {
                    let m = translation(o);
                    let bounds = protos.get(p as usize).map_or(el.bounds, |pr| pr.bounds.placed(&m));
                    instances.push(Instance { proto: p, m, bounds, show: Show::from(el.tier) });
                    el.held = Held::Instance;
                } else if el.tier >= opts.stream_from && side == Side::Inside {
                    el.held = Held::Streamed;
                    streamed.push(e);
                } else {
                    if el.tier != Tier::Building {
                        rest_show.insert(e.handle, Show::from(el.tier));
                    }
                    rest.push((*e).clone());
                }
            }
        }
    }
    progress.fetch_add(1, Ordering::Relaxed);
    // The base scene is clipped by the section box planes (if any).
    let mut base = scene_clipped(&with_model(d, rest.into_iter()), &Space::Model, tol, bg, extra_planes);
    base.lighting = Some(crate::light::Lighting::of(d, space));
    // What the box cuts through: the prototype's triangles and edges placed, clipped, in the base.
    for (h, p, m) in &cut_through {
        let Some(full) = protos.get(*p as usize).and_then(|pr| pr.lods.first()) else { continue };
        base.owner_now = *h;
        for t in &full.tris {
            let pts = crate::live::clip_polygon(t.p.iter().map(|q| m.apply(*q)).collect(), extra_planes);
            for k in 1..pts.len().saturating_sub(1) {
                if let (Some(a), Some(b), Some(c)) = (pts.first(), pts.get(k), pts.get(k + 1)) {
                    crate::shade::push_tri(&mut base, Tri { p: [*a, *b, *c], mat: crate::shade::NO_SURFACE, ..*t });
                }
            }
        }
        for l in &full.lines {
            if let Some((a, b)) = crate::live::clip_segment(m.apply(l.a), m.apply(l.b), extra_planes) {
                crate::shade::push_line(&mut base, Line3 { a, b, ..*l });
            }
        }
    }
    // Every triangle and line at its object's tiers; then each run's pieces, from the tier the
    // classifier says up to the one below their own, as lines along the run.
    let tri_show: Vec<Show> =
        if rest_show.is_empty() { Vec::new() } else { base.owners.iter().map(|h| rest_show.get(h).copied().unwrap_or(Show::ALWAYS)).collect() };
    let mut line_show: Vec<Show> =
        if rest_show.is_empty() { Vec::new() } else { base.line_owners.iter().map(|h| rest_show.get(h).copied().unwrap_or(Show::ALWAYS)).collect() };
    if fid.is_some() {
        // Elements not meshes: their bounds from their triangles.
        let mut tb: std::collections::HashMap<Handle, (Aabb, u32)> = std::collections::HashMap::new();
        for (t, h) in base.tris.iter().zip(&base.owners) {
            let e = tb.entry(*h).or_insert((Aabb::EMPTY, 0));
            for p in t.p {
                e.0.grow(p);
            }
            e.1 += 1;
        }
        for el in elems.iter_mut().filter(|el| el.bounds.is_empty()) {
            if let Some((b, n)) = tb.get(&el.handle) {
                el.bounds = *b;
                el.tris = *n;
            }
        }
        if line_show.len() != base.lines.len() {
            line_show.resize(base.lines.len(), Show::ALWAYS);
        }
        let mut runs = 0usize;
        for (el, from) in elems.iter().zip(&line_from) {
            let (Some(from), Some(below)) = (*from, el.tier.coarser()) else { continue };
            if el.group == NO_GROUP || from > below || el.held == Held::Outside || runs >= crate::shade::MAX_SCENE {
                continue;
            }
            let Some((a, b)) = run_line(&el.bounds) else { continue };
            let color = store.get(el.handle).map_or(Rgb(160, 160, 160), |e| own_rgb(e));
            base.owner_now = el.handle;
            crate::shade::push_line(&mut base, Line3 { a, b, color, edge: false, faint: false });
            if line_show.len() < base.lines.len() {
                line_show.push(Show { from, to: below });
                runs += 1;
            }
        }
    }
    let (mut base, chunks) = chunked_by(base, &tri_show, &line_show);
    let mut bounds = bounds_of(&base);
    for i in &instances {
        if !i.bounds.is_empty() {
            bounds.grow(i.bounds.lo);
            bounds.grow(i.bounds.hi);
        }
    }
    for el in elems.iter().filter(|el| el.held == Held::Streamed) {
        bounds.grow(el.bounds.lo);
        bounds.grow(el.bounds.hi);
    }
    progress.fetch_add(1, Ordering::Relaxed);
    // Fog distances start at the front of the whole model (instances included).
    if let Some(lit) = base.lighting.as_mut()
        && !bounds.is_empty()
    {
        lit.bounds = Some((bounds.lo, bounds.hi));
    }
    let fid = fid.map(|_| {
        let mut f = Fid::new(elems, kinds.list, levels.list, groups.list, &bounds);
        if !streamed.is_empty() {
            let mut src = d.clone();
            let mut st = EntityStore::new();
            for e in streamed {
                st.push_shared(e);
            }
            src.model = st;
            f.source = Some(Arc::new(src));
        }
        f.tol = tol;
        f.bg = bg;
        f.planes = extra_planes.to_vec();
        Arc::new(f)
    });
    Model3 { base, chunks, protos, instances, bounds, fid }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Block, Common, Entity, Handle, Insert, Mesh, MeshFace, MeshKind};
    use cadaddy_geom::Vec2;

    fn boxmesh(lo: Vec3, hi: Vec3) -> Mesh {
        let v = |x: bool, y: bool, z: bool| Vec3::new(if x { hi.x } else { lo.x }, if y { hi.y } else { lo.y }, if z { hi.z } else { lo.z });
        let vertices = vec![
            v(false, false, false),
            v(true, false, false),
            v(true, true, false),
            v(false, true, false),
            v(false, false, true),
            v(true, false, true),
            v(true, true, true),
            v(false, true, true),
        ];
        let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
        Mesh {
            vertices,
            faces: vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])],
            kind: MeshKind::Subd,
        }
    }

    /// A floor slab and a 100 × 100 grid of unit boxes (a block) on it.
    fn drawing() -> Drawing {
        let mut d = Drawing::new_metric();
        d.add(&Space::Model, Common::default(), EntityKind::Mesh(boxmesh(Vec3::new(0.0, 0.0, -1.0), Vec3::new(1000.0, 1000.0, 0.0)))).unwrap();
        let mut b = Block::new("Fixture");
        b.entities.push(Entity::new(Handle(0x9000), EntityKind::Mesh(boxmesh(Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0)))));
        d.blocks.insert("Fixture".into(), std::sync::Arc::new(b));
        for i in 0..100 {
            for j in 0..100 {
                let ins = Insert {
                    block: "Fixture".into(),
                    insert: Vec3::new(f64::from(i) * 10.0, f64::from(j) * 10.0, 0.0),
                    scale: Vec3::new(1.0, 1.0, 1.0),
                    rotation: 0.0,
                    attribs: vec![],
                    cols: 1,
                    rows: 1,
                    col_spacing: 0.0,
                    row_spacing: 0.0,
                    clip: None,
                };
                d.add(&Space::Model, Common::default(), EntityKind::Insert(ins)).unwrap();
            }
        }
        d
    }

    fn view(center: Vec2, height: f64, w: u32, h: u32) -> ShadeView {
        ShadeView::new(None, center, height, w, h)
    }

    #[test]
    fn references_become_instances_and_the_view_culls_them() {
        let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let m = build(&drawing(), &Space::Model, 0.01, Rgb(0, 0, 0), &p, &t);
        assert_eq!(m.protos.len(), 1);
        assert_eq!(m.instances.len(), 10_000);
        assert_eq!(p.load(Ordering::Relaxed), t.load(Ordering::Relaxed));
        // One box's 12 triangles once, not ten thousand times.
        assert_eq!(m.protos[0].lods[0].tris.len(), 12);
        assert_eq!(m.total_tris(), 12 + 12 * 10_000);
        // Looking at a 50-unit corner of the plan from above: 25 fittings, and the slab.
        let v = m.visible(&view(Vec2::new(25.0, 25.0), 50.0, 500, 500));
        let shown: usize = v.instances.iter().map(|g| g.2.len()).sum();
        assert!((25..=36).contains(&shown), "{shown}");
        assert_eq!(v.chunks.len(), 1);
        // Seen whole, each fitting covers 1.7 units of 1000 in 500 pixels: a box.
        let all = m.visible(&view(Vec2::new(500.0, 500.0), 1000.0, 500, 500));
        assert!(all.instances.iter().all(|g| g.1 == 2), "{:?}", all.instances.iter().map(|g| g.1).collect::<Vec<_>>());
        assert_eq!(all.instances.iter().map(|g| g.2.len()).sum::<usize>(), 10_000);
        // Front to back: the nearest first.
        let side = ShadeView::new(cadaddy_geom::view3::Projection::new(Vec3::new(1.0, 0.0, 0.0)), Vec2::new(500.0, 0.0), 1200.0, 500, 500);
        let s = m.visible(&side);
        let first = &s.instances[0].2;
        let x = |i: u32| m.instances[i as usize].bounds.center().x;
        assert!(x(first[0]) >= x(*first.last().unwrap()));
        // The flat scene for the CPU path holds every placed triangle.
        assert_eq!(m.flatten().tris.len(), m.total_tris());
    }

    /// The six planes of the box `lo`..`hi` (each keeping the inside), as a coordination section
    /// box makes them.
    fn box_planes(lo: Vec3, hi: Vec3) -> Vec<cadaddy_doc::SectionPlane> {
        let c = (lo + hi) * 0.5;
        let plane =
            |origin: Vec3, normal: Vec3| cadaddy_doc::SectionPlane { origin, normal, width: 1e6, height: 1e6, live: true, name: String::new() };
        vec![
            plane(Vec3::new(hi.x, c.y, c.z), Vec3::new(-1.0, 0.0, 0.0)),
            plane(Vec3::new(lo.x, c.y, c.z), Vec3::new(1.0, 0.0, 0.0)),
            plane(Vec3::new(c.x, hi.y, c.z), Vec3::new(0.0, -1.0, 0.0)),
            plane(Vec3::new(c.x, lo.y, c.z), Vec3::new(0.0, 1.0, 0.0)),
            plane(Vec3::new(c.x, c.y, hi.z), Vec3::new(0.0, 0.0, -1.0)),
            plane(Vec3::new(c.x, c.y, lo.z), Vec3::new(0.0, 0.0, 1.0)),
        ]
    }

    #[test]
    fn section_box_clips_block_instances() {
        let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(0));
        // Round the fixtures at x 0 and 10, y 0, 10 and 20; through the ones at x 20 (20..21).
        let (lo, hi) = (Vec3::new(-0.5, -0.5, -0.5), Vec3::new(20.5, 25.5, 2.0));
        let m = build_clipped(&drawing(), &Space::Model, 0.01, Rgb(0, 0, 0), &p, &t, &box_planes(lo, hi));
        // Wholly inside: still instances, all of them in the box.
        assert_eq!(m.instances.len(), 6, "{:?}", m.instances.iter().map(|i| i.bounds.lo).collect::<Vec<_>>());
        for i in &m.instances {
            assert!(i.bounds.lo.x >= lo.x && i.bounds.hi.x <= hi.x && i.bounds.lo.y >= lo.y && i.bounds.hi.y <= hi.y, "{:?}", i.bounds);
        }
        // Wholly outside (x 30.., y 30..): gone. Cut by the box: drawn clipped in the base.
        let flat = m.flatten();
        let pts: Vec<Vec3> = flat.tris.iter().flat_map(|t| t.p).collect();
        assert!(!pts.is_empty());
        let eps = 1e-6;
        assert!(pts.iter().all(|q| q.x <= hi.x + eps && q.x >= lo.x - eps && q.y <= hi.y + eps && q.y >= lo.y - eps), "geometry outside the box");
        // The three cut fixtures stand above the slab between x 20 and the box's face.
        let cut_tops = m.base.tris.iter().flat_map(|t| t.p).filter(|q| q.x > 20.0 + eps && q.z > 0.5).count();
        assert!(cut_tops > 0, "the fixtures the box cuts are drawn, clipped");
        assert!(m.base.tris.iter().flat_map(|t| t.p).all(|q| q.x <= hi.x + eps));
        // Without a box every fixture is an instance.
        let all = build(&drawing(), &Space::Model, 0.01, Rgb(0, 0, 0), &p, &t);
        assert_eq!(all.instances.len(), 10_000);
    }

    #[test]
    fn box_side_classifies() {
        let planes = box_planes(Vec3::ZERO, Vec3::new(10.0, 10.0, 10.0));
        let b = |lo: Vec3, hi: Vec3| Aabb { lo, hi };
        assert_eq!(box_side(&b(Vec3::new(1.0, 1.0, 1.0), Vec3::new(2.0, 2.0, 2.0)), &planes), Side::Inside);
        assert_eq!(box_side(&b(Vec3::new(11.0, 1.0, 1.0), Vec3::new(12.0, 2.0, 2.0)), &planes), Side::Outside);
        assert_eq!(box_side(&b(Vec3::new(9.0, 1.0, 1.0), Vec3::new(12.0, 2.0, 2.0)), &planes), Side::Cut);
        assert_eq!(box_side(&b(Vec3::new(11.0, 1.0, 1.0), Vec3::new(12.0, 2.0, 2.0)), &[]), Side::Inside);
    }

    #[test]
    fn culling_and_simplifying() {
        let b = Aabb { lo: Vec3::new(10.0, 10.0, 0.0), hi: Vec3::new(11.0, 11.0, 1.0) };
        assert!(in_view(&b, &view(Vec2::new(0.0, 0.0), 22.0, 100, 100)));
        assert!(!in_view(&b, &view(Vec2::new(0.0, 0.0), 18.0, 100, 100)));
        // Wider than tall: the sides reach further.
        let side = Aabb { lo: Vec3::new(10.0, 0.0, 0.0), hi: Vec3::new(11.0, 1.0, 1.0) };
        assert!(!in_view(&side, &view(Vec2::new(0.0, 0.0), 18.0, 100, 100)));
        assert!(in_view(&side, &view(Vec2::new(0.0, 0.0), 18.0, 200, 100)));
        assert!(!in_view(&Aabb::EMPTY, &view(Vec2::ZERO, 1e9, 10, 10)));
        // A finely cut sphere-ish scene simplifies to fewer triangles.
        let mut s = Scene::default();
        for i in 0..64 {
            for j in 0..64 {
                let p = |a: f64, b: f64| Vec3::new(a / 64.0, b / 64.0, ((a + b) / 30.0).sin());
                let (a, b) = (f64::from(i), f64::from(j));
                s.tris.push(Tri::plain([p(a, b), p(a + 1.0, b), p(a + 1.0, b + 1.0)], Rgb(9, 9, 9)));
            }
        }
        let bb = bounds_of(&s);
        assert!(simplify(&s, &bb, 12.0).tris.len() < s.tris.len() / 4);
        assert_eq!(box_of(&s, &bb).tris.len(), 12);
    }

    #[test]
    fn flatten_visible_uses_lod_and_culling() {
        let d = drawing();
        let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let m = build(&d, &Space::Model, 0.01, Rgb(0, 0, 0), &p, &t);
        // The full model: 10,000 instances + 1 slab.
        let full = m.flatten();
        assert_eq!(full.tris.len(), m.total_tris());
        // A wide view where every fixture is a box (LOD 2, 12 tris) instead of full (12 tris
        // for our unit cube, but for a real fixture with many faces the savings are large).
        let wide = view(Vec2::new(500.0, 500.0), 1000.0, 500, 500);
        let vis = m.visible(&wide);
        let scene = m.flatten_visible(&vis);
        // All 10,000 instances are visible but at box LOD.
        assert_eq!(vis.instances.iter().map(|g| g.2.len()).sum::<usize>(), 10_000);
        assert!(vis.instances.iter().all(|g| g.1 == 2));
        // The visible scene has the same count here (box LOD for a cube = same 12 tris) but
        // the base chunks are only those in view, which is all of them for a wide view.
        assert!(!scene.tris.is_empty());
        // A corner view: fewer instances visible.
        let corner = view(Vec2::new(25.0, 25.0), 50.0, 500, 500);
        let vis_corner = m.visible(&corner);
        let scene_corner = m.flatten_visible(&vis_corner);
        let shown: usize = vis_corner.instances.iter().map(|g| g.2.len()).sum();
        assert!(shown < 10_000, "corner view should cull most instances");
        assert!(scene_corner.tris.len() < full.tris.len(), "corner scene should have fewer tris than full");
    }

    #[test]
    fn fit_bounds_produces_a_view() {
        let bounds = Aabb { lo: Vec3::new(0.0, 0.0, 0.0), hi: Vec3::new(100.0, 100.0, 10.0) };
        let v = crate::shade::fit_bounds(&bounds, None, 800, 600);
        assert!(v.is_some());
        let v = v.unwrap();
        assert!(v.height > 0.0);
        assert_eq!(v.width_px, 800);
        assert_eq!(v.height_px, 600);
        // Empty bounds: no view.
        assert!(crate::shade::fit_bounds(&Aabb::EMPTY, None, 800, 600).is_none());
    }

    /// A made-up storey for fidelity: a slab (building tier), four walls (floor), 120 identical
    /// receptacle boxes (room), a run of 12 conduit sticks of different lengths with a coupling
    /// between each (room, couplings detail), and one big unit (building).
    fn storey() -> Drawing {
        let mut d = Drawing::new_metric();
        let mut put = |layer: &str, lo: Vec3, hi: Vec3, rgb: Rgb| {
            let c = Common { layer: layer.into(), color: cadaddy_color::Color::True(rgb), ..Common::default() };
            d.add(&Space::Model, c, EntityKind::Mesh(boxmesh(lo, hi))).unwrap();
        };
        put("SLAB", Vec3::new(0.0, 0.0, -1.0), Vec3::new(400.0, 200.0, 0.0), Rgb(200, 200, 200));
        put("WALL", Vec3::new(0.0, 0.0, 0.0), Vec3::new(400.0, 1.0, 10.0), Rgb(210, 210, 200));
        put("WALL", Vec3::new(0.0, 199.0, 0.0), Vec3::new(400.0, 200.0, 10.0), Rgb(210, 210, 200));
        put("WALL", Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 200.0, 10.0), Rgb(210, 210, 200));
        put("WALL", Vec3::new(399.0, 0.0, 0.0), Vec3::new(400.0, 200.0, 10.0), Rgb(210, 210, 200));
        put("UNIT", Vec3::new(300.0, 150.0, 0.0), Vec3::new(320.0, 170.0, 8.0), Rgb(180, 180, 190));
        for i in 0..12 {
            for j in 0..10 {
                let (x, y) = (10.0 + f64::from(i) * 30.0, 10.0 + f64::from(j) * 18.0);
                put("DEV", Vec3::new(x, y, 1.5), Vec3::new(x + 0.2, y + 0.33, 1.83), Rgb(250, 212, 40));
            }
        }
        let mut x = 5.0;
        for k in 0..12 {
            let len = 9.0 + f64::from(k) * 0.05;
            put("COND", Vec3::new(x, 100.0, 9.0), Vec3::new(x + len, 100.06, 9.06), Rgb(246, 166, 24));
            x += len;
            put("CPLG", Vec3::new(x - 0.05, 99.99, 8.99), Vec3::new(x + 0.05, 100.07, 9.07), Rgb(246, 166, 24));
        }
        d
    }

    fn storey_class(_: &Drawing, e: &Entity) -> Option<ElemClass> {
        let layer = e.common.layer.to_ascii_uppercase();
        let (tier, kind, group, line_from) = match layer.as_str() {
            "SLAB" => (Tier::Building, "floor", None, None),
            "UNIT" => (Tier::Building, "equipment", None, None),
            "WALL" => (Tier::Floor, "wall", None, None),
            "DEV" => (Tier::Room, "receptacle", None, None),
            "COND" => (Tier::Room, "conduit", Some("run 1"), Some(Tier::Floor)),
            "CPLG" => (Tier::Detail, "coupling", Some("run 1"), None),
            _ => return None,
        };
        Some(ElemClass { tier, kind: kind.into(), level: "Level 1".into(), group: group.map(str::to_string), line_from })
    }

    fn fid_model(d: Drawing) -> Model3 {
        let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let m = build_fid(Arc::new(d), &Space::Model, 0.01, Rgb(255, 255, 255), &p, &t, &[], &storey_class, FidOptions::default());
        assert_eq!(p.load(Ordering::Relaxed), t.load(Ordering::Relaxed));
        m
    }

    /// Drawn at a tier: the elements whose triangles (base chunks, instances or streamed cells)
    /// the view draws, by layer.
    fn drawn(m: &Model3, d: &Drawing, v: &ShadeView, tier: Tier) -> std::collections::BTreeMap<String, usize> {
        let vis = m.visible_at(v, tier, None);
        let mut st = Stream::default();
        st.update(m, v, tier, f64::INFINITY);
        let mut handles: std::collections::BTreeSet<Handle> = std::collections::BTreeSet::new();
        for ci in &vis.chunks {
            let c = &m.chunks[*ci];
            handles.extend(m.base.owners[c.tris.clone()].iter().copied());
        }
        let f = m.fid.as_ref().unwrap();
        for (_, _, list) in &vis.instances {
            for i in list {
                let inst = &m.instances[*i as usize];
                // The element placed there: the one whose box it is.
                if let Some(e) = f.elems.iter().find(|e| e.held == Held::Instance && (e.bounds.lo - inst.bounds.lo).len() < 1e-6) {
                    handles.insert(e.handle);
                }
            }
        }
        for k in st.visible(v) {
            handles.extend(st.cells[&k].scene.owners.iter().copied());
        }
        let mut out = std::collections::BTreeMap::new();
        for h in handles {
            if let Some(e) = d.model.get(h) {
                *out.entry(e.common.layer.to_string()).or_insert(0) += 1;
            }
        }
        out
    }

    #[test]
    fn fidelity_tiers_by_kind_and_zoom() {
        let d = storey();
        let m = fid_model(d.clone());
        let f = m.fid.as_ref().unwrap();
        // Every element indexed once, at its tier; counts don't depend on the view.
        let per = f.per_tier();
        assert_eq!(per.iter().map(|c| c.elements).sum::<usize>(), d.model.len());
        assert_eq!([per[0].elements, per[1].elements, per[2].elements, per[3].elements], [2, 4, 120 + 12, 12]);
        let far = view(Vec2::new(200.0, 100.0), 260.0, 800, 520);
        let near = view(Vec2::new(40.0, 28.0), 12.0, 800, 520);
        // Far at the building tier: the slab and the unit only, no device, no wall.
        let b = drawn(&m, &d, &far, Tier::Building);
        assert_eq!(b.get("SLAB"), Some(&1));
        assert_eq!(b.get("UNIT"), Some(&1));
        assert!(!b.contains_key("DEV") && !b.contains_key("WALL") && !b.contains_key("COND"), "{b:?}");
        // At the floor tier the walls join; the devices still don't; the run is a line.
        let fl = m.visible_at(&far, Tier::Floor, None);
        assert!(drawn(&m, &d, &far, Tier::Floor).get("WALL") == Some(&4));
        assert!(!drawn(&m, &d, &far, Tier::Floor).contains_key("DEV"));
        let run_lines: usize =
            fl.chunks.iter().map(|c| &m.chunks[*c]).filter(|c| c.show == Show { from: Tier::Floor, to: Tier::Floor }).map(|c| c.lines.len()).sum();
        assert_eq!(run_lines, 12, "each stick a line along the run at the floor tier");
        // Near at the room tier: this room's devices, not the far ones; the run lines give way.
        let r = drawn(&m, &d, &near, Tier::Room);
        let devs = r.get("DEV").copied().unwrap_or(0);
        assert!((1..=6).contains(&devs), "{r:?}");
        let rv = m.visible_at(&near, Tier::Room, None);
        assert!(rv.chunks.iter().all(|c| m.chunks[*c].show.at(Tier::Room)));
        assert!(!rv.chunks.iter().any(|c| m.chunks[*c].show == Show { from: Tier::Floor, to: Tier::Floor }));
        // Detail: the couplings appear; at room they don't.
        let run_view = view(Vec2::new(30.0, 100.0), 30.0, 800, 520);
        assert!(!drawn(&m, &d, &run_view, Tier::Room).contains_key("CPLG"));
        assert!(drawn(&m, &d, &run_view, Tier::Detail).contains_key("CPLG"));
        assert!(drawn(&m, &d, &run_view, Tier::Room).contains_key("COND"));
        // The far view draws far fewer triangles at the building tier than with everything.
        assert!(m.visible_at(&far, Tier::Building, None).tris * 4 < m.visible(&far).tris + drawn_stream_tris(&m, &far));
    }

    fn drawn_stream_tris(m: &Model3, v: &ShadeView) -> usize {
        let mut st = Stream::default();
        st.update(m, v, Tier::Detail, f64::INFINITY);
        st.tris()
    }

    #[test]
    fn identical_atoms_are_instances_and_unique_ones_stream() {
        let d = storey();
        let m = fid_model(d);
        let f = m.fid.as_ref().unwrap();
        // 120 identical boxes: one prototype, 120 placements; the 12 identical couplings too.
        let devs: Vec<&Elem> = f.elems.iter().filter(|e| f.kind_of(e) == "receptacle").collect();
        assert!(devs.iter().all(|e| e.held == Held::Instance));
        let cplg: Vec<&Elem> = f.elems.iter().filter(|e| f.kind_of(e) == "coupling").collect();
        assert!(cplg.iter().all(|e| e.held == Held::Instance), "{:?}", cplg.iter().map(|e| e.held).collect::<Vec<_>>());
        assert_eq!(m.protos.len(), 2);
        assert_eq!(m.instances.len(), 132);
        assert_eq!(m.protos.iter().map(|p| p.lods[0].tris.len()).sum::<usize>(), 24);
        // Twelve sticks all of a different length: not repeated, room tier ⟹ held back.
        let sticks: Vec<&Elem> = f.elems.iter().filter(|e| f.kind_of(e) == "conduit").collect();
        assert_eq!(sticks.len(), 12);
        assert!(sticks.iter().all(|e| e.held == Held::Streamed));
        assert!(m.base.owners.iter().all(|h| !sticks.iter().any(|s| s.handle == *h)), "a streamed stick isn't in the base");
        // The slab, walls and unit are in the base.
        assert_eq!(f.elems.iter().filter(|e| e.held == Held::Base).count(), 6);
        // Instances draw from their tier only.
        let far = view(Vec2::new(200.0, 100.0), 260.0, 800, 520);
        assert!(m.visible_at(&far, Tier::Floor, None).instances.is_empty());
        assert_eq!(m.visible_at(&far, Tier::Room, None).instances.iter().map(|g| g.2.len()).sum::<usize>(), 120);
        assert_eq!(m.visible_at(&far, Tier::Detail, None).instances.iter().map(|g| g.2.len()).sum::<usize>(), 132);
    }

    #[test]
    fn stream_brings_cells_in_and_lets_them_go() {
        let d = storey();
        let m = fid_model(d);
        let mut st = Stream::default();
        let run_view = view(Vec2::new(30.0, 100.0), 30.0, 800, 520);
        // Floor tier: nothing streamed.
        let s0 = st.update(&m, &run_view, Tier::Floor, f64::INFINITY);
        assert_eq!((s0.added, st.cells.len()), (0, 0));
        // Room tier, looking at the start of the run: its sticks come in.
        let s1 = st.update(&m, &run_view, Tier::Room, f64::INFINITY);
        assert!(s1.added > 0 && st.tris() > 0 && st.bytes > 0);
        let g1 = st.generation;
        // The same view again: nothing new.
        let s2 = st.update(&m, &run_view, Tier::Room, f64::INFINITY);
        assert_eq!((s2.added, s2.dropped, st.generation), (0, 0, g1));
        // Zoomed out to the floor tier: the cells go, the memory with them.
        let s3 = st.update(&m, &run_view, Tier::Floor, f64::INFINITY);
        assert!(s3.dropped > 0);
        assert_eq!((st.cells.len(), st.bytes), (0, 0));
        // A budget of one byte keeps only what the view sees.
        st.budget = 1;
        st.update(&m, &view(Vec2::new(30.0, 100.0), 30.0, 800, 520), Tier::Room, f64::INFINITY);
        let here = st.cells.len();
        st.update(&m, &view(Vec2::new(330.0, 100.0), 30.0, 800, 520), Tier::Room, f64::INFINITY);
        assert!(st.cells.len() <= here.max(1) + 1, "{}", st.cells.len());
        // A model built without fidelity streams nothing and draws everything at every tier.
        let (p, t) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let plain = build(&storey(), &Space::Model, 0.01, Rgb(0, 0, 0), &p, &t);
        let far = view(Vec2::new(200.0, 100.0), 260.0, 800, 520);
        assert!(plain.fid.is_none());
        assert_eq!(plain.visible_at(&far, Tier::Building, None).tris, plain.visible(&far).tris);
        assert_eq!(Stream::default().update(&plain, &far, Tier::Detail, 1.0).added, 0);
    }

    #[test]
    fn auto_tier_holds_until_past_the_margin() {
        let a = AutoTiers { building_ft: 240.0, floor_ft: 90.0, room_ft: 12.0, margin: 0.15 };
        assert_eq!(a.raw(500.0), Tier::Building);
        assert_eq!(a.raw(100.0), Tier::Floor);
        assert_eq!(a.raw(30.0), Tier::Room);
        assert_eq!(a.raw(5.0), Tier::Detail);
        // Zooming in from the building tier: it holds until 15 % under 240 ft.
        assert_eq!(a.tier(230.0, Some(Tier::Building)), Tier::Building);
        assert_eq!(a.tier(200.0, Some(Tier::Building)), Tier::Floor);
        // Back out: the floor tier holds until 15 % over 240 ft.
        assert_eq!(a.tier(250.0, Some(Tier::Floor)), Tier::Floor);
        assert_eq!(a.tier(280.0, Some(Tier::Floor)), Tier::Building);
        // Wavering about the room threshold doesn't flicker.
        let mut t = Some(Tier::Floor);
        let mut changes = 0;
        for k in 0..40 {
            let h = 90.0 + if k % 2 == 0 { 5.0 } else { -5.0 };
            let nt = a.tier(h, t);
            if Some(nt) != t {
                changes += 1;
            }
            t = Some(nt);
        }
        assert_eq!(changes, 0);
        assert_eq!(a.tier(f64::NAN, Some(Tier::Room)), Tier::Room);
        assert_eq!(Tier::parse("Room"), Some(Tier::Room));
        assert_eq!(Tier::parse("x"), None);
    }

    #[test]
    fn coarse_pick_takes_the_run_and_search_forces_markers() {
        let d = storey();
        let m = fid_model(d.clone());
        let f = m.fid.as_ref().unwrap();
        let stick = f.elems.iter().find(|e| f.kind_of(e) == "conduit").unwrap().handle;
        // At the floor tier a stick is drawn as part of its run: a click picks all of it.
        let run = f.pick(stick, Tier::Floor);
        assert_eq!(run.len(), 24, "12 sticks and 12 couplings");
        // Zoomed in to the room tier: the stick itself.
        assert_eq!(f.pick(stick, Tier::Room), vec![stick]);
        let wall = f.elems.iter().find(|e| f.kind_of(e) == "wall").unwrap().handle;
        assert_eq!(f.pick(wall, Tier::Building), vec![wall]);
        // "Show all receptacles on Level 1" at the building tier: markers for the 120 boxes.
        let far = view(Vec2::new(200.0, 100.0), 260.0, 800, 520);
        let force = Force { kind: "RECEPTACLE".into(), level: Some("level 1".into()) };
        let vis = m.visible_at(&far, Tier::Building, Some(&force));
        assert_eq!(vis.markers.len(), 120);
        assert!(m.visible_at(&far, Tier::Room, Some(&force)).markers.is_empty(), "drawn already: no markers");
        let other = Force { kind: "receptacle".into(), level: Some("Level 9".into()) };
        assert!(m.visible_at(&far, Tier::Building, Some(&other)).markers.is_empty());
    }
}
