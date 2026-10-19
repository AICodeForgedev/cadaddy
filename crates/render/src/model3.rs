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

use std::sync::atomic::{AtomicUsize, Ordering};

use cadaddy_color::Rgb;
use cadaddy_doc::{Drawing, EntityKind, EntityStore, Space};
use cadaddy_geom::{Mat4, Vec3};

use crate::shade::{Line3, Scene, ShadeView, Tri, scene};

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
}

/// Pixels a prototype covers below which it draws simplified, and below which as a box.
pub const LOD_SIMPLE_PX: f64 = 48.0;
pub const LOD_BOX_PX: f64 = 6.0;

impl Model3 {
    /// What the view `v` shows, levels of detail chosen and nearest first.
    pub fn visible(&self, v: &ShadeView) -> Visible {
        let px = v.height / f64::from(v.height_px.max(1));
        let depth = |b: &Aabb| -b.center().dot(v.dir);
        let mut chunks: Vec<(f64, usize)> =
            self.chunks.iter().enumerate().filter(|(_, c)| in_view(&c.bounds, v)).map(|(i, c)| (depth(&c.bounds), i)).collect();
        chunks.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut tris = chunks.iter().filter_map(|(_, i)| self.chunks.get(*i)).map(|c| c.tris.len()).sum::<usize>();
        let mut groups: std::collections::BTreeMap<(u32, u8), Vec<(f64, u32)>> = std::collections::BTreeMap::new();
        for (i, inst) in self.instances.iter().enumerate() {
            if !in_view(&inst.bounds, v) {
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
        Visible { chunks: chunks.into_iter().map(|c| c.1).collect(), instances, tris }
    }

    /// Triangles in the whole model, instances counted as placed.
    pub fn total_tris(&self) -> usize {
        self.base.tris.len() + self.instances.iter().filter_map(|i| self.protos.get(i.proto as usize)).map(|p| p.lods[0].tris.len()).sum::<usize>()
    }

    /// Bytes the model holds (triangles, lines, matrices).
    pub fn bytes(&self) -> usize {
        let scene = |s: &Scene| s.tris.len() * std::mem::size_of::<Tri>() + s.lines.len() * std::mem::size_of::<Line3>();
        scene(&self.base)
            + self.protos.iter().map(|p| p.lods.iter().map(scene).sum::<usize>()).sum::<usize>()
            + self.instances.len() * std::mem::size_of::<Instance>()
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

/// The base scene's triangles and lines regrouped into a grid of chunks over the plan.
fn chunked(mut s: Scene) -> (Scene, Vec<Chunk>) {
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
    let mut tris: Vec<(usize, Tri)> = s.tris.drain(..).map(|t| (cell((t.p[0] + t.p[1] + t.p[2]) * (1.0 / 3.0)), t)).collect();
    tris.sort_by_key(|x| x.0);
    let mut lines: Vec<(usize, Line3)> = s.lines.drain(..).map(|l| (cell((l.a + l.b) * 0.5), l)).collect();
    lines.sort_by_key(|x| x.0);
    let mut chunks: Vec<Chunk> = (0..n * n).map(|_| Chunk { tris: 0..0, lines: 0..0, bounds: Aabb::EMPTY }).collect();
    let (mut ti, mut li) = (0usize, 0usize);
    for (c, chunk) in chunks.iter_mut().enumerate() {
        let start = ti;
        while ti < tris.len() && tris[ti].0 == c {
            for p in tris[ti].1.p {
                chunk.bounds.grow(p);
            }
            ti += 1;
        }
        chunk.tris = start..ti;
        let start = li;
        while li < lines.len() && lines[li].0 == c {
            chunk.bounds.grow(lines[li].1.a);
            chunk.bounds.grow(lines[li].1.b);
            li += 1;
        }
        chunk.lines = start..li;
    }
    s.tris = tris.into_iter().map(|x| x.1).collect();
    s.lines = lines.into_iter().map(|x| x.1).collect();
    chunks.retain(|c| !c.tris.is_empty() || !c.lines.is_empty());
    (s, chunks)
}

/// Most block references made instances (beyond them, references draw as part of the base).
pub const MAX_INSTANCES: usize = 2_000_000;

/// Build the model of `space` (curves to `tol`, colour 7 for `bg`), `progress` counting the steps
/// done out of the total stored in `total`.
pub fn build(d: &Drawing, space: &Space, tol: f64, bg: Rgb, progress: &AtomicUsize, total: &AtomicUsize) -> Model3 {
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
    total.store(names.len() + 2, Ordering::Relaxed);
    let mut protos: Vec<Proto> = Vec::new();
    let mut proto_of: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for n in &names {
        progress.fetch_add(1, Ordering::Relaxed);
        let Some(b) = d.block(n) else { continue };
        if b.xref_path.is_some() {
            continue;
        }
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
    let mut instances = Vec::new();
    let mut rest = Vec::new();
    for e in store.iter() {
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
                    let bounds = protos.get(p as usize).map_or(Aabb::EMPTY, |pr| pr.bounds.placed(&m));
                    instances.push(Instance { proto: p, m, bounds });
                }
            }
            continue;
        }
        rest.push((**e).clone());
    }
    progress.fetch_add(1, Ordering::Relaxed);
    let mut base = scene(&with_model(d, rest.into_iter()), &Space::Model, tol, bg);
    base.lighting = Some(crate::light::Lighting::of(d, space));
    let (base, chunks) = chunked(base);
    let mut bounds = bounds_of(&base);
    for i in &instances {
        if !i.bounds.is_empty() {
            bounds.grow(i.bounds.lo);
            bounds.grow(i.bounds.hi);
        }
    }
    progress.fetch_add(1, Ordering::Relaxed);
    Model3 { base, chunks, protos, instances, bounds }
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
}
