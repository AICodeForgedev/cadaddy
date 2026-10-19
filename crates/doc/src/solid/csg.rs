//! Composite solids: UNION, SUBTRACT and INTERSECT of solids, kept as a tree whose leaves are
//! exact solids. The surfaces of a composite come from a polygon boolean of its operands'
//! surfaces ([`super::bsp`]); every polygon remembers which face of which leaf it came from,
//! so the display shows the edges between faces, and face edits know what they touch.

use cadaddy_geom::{Mat4, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, OnceLock};

use super::bsp::{self, Poly};
use super::{Solid3d, SolidShape, frame_for};

/// How two solids combine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BoolOp {
    Union,
    Subtract,
    Intersect,
}

/// Deepest composite tree and most leaves one solid has.
pub const MAX_CSG_DEPTH: usize = 32;
pub const MAX_CSG_LEAVES: usize = 256;

/// A polygon's tag: its leaf (in depth-first order) and the leaf's face.
pub fn tag(leaf: u32, face: u32) -> u64 {
    (u64::from(leaf) << 32) | u64::from(face)
}
pub fn tag_leaf(t: u64) -> u32 {
    (t >> 32) as u32
}
pub fn tag_face(t: u64) -> u32 {
    (t & 0xffff_ffff) as u32
}

/// Face numbers of a leaf: its two ends, then its sides (loop × 65536 + the run of points
/// between two corners of that loop).
pub const FACE_START: u32 = 0;
pub const FACE_END: u32 = 1;
fn side_face(l: usize, group: usize) -> u32 {
    2 + (l.min(1000) as u32) * 65536 + group.min(65535) as u32
}

/// Which way to go at each composite on the way down to a leaf: `false` = first operand.
pub type LeafPath = Vec<bool>;

impl Solid3d {
    /// The leaves under this solid, in depth-first order: their paths and their placements in
    /// the drawing.
    pub fn leaves(&self) -> Vec<(LeafPath, Solid3d)> {
        let mut out = Vec::new();
        let mut stack = vec![(Vec::new(), self.clone(), Mat4::IDENTITY)];
        while let Some((path, s, outer)) = stack.pop() {
            let xf = outer.mul(&s.xf);
            match &s.shape {
                SolidShape::Boolean { a, b, .. } if path.len() < MAX_CSG_DEPTH && out.len() < MAX_CSG_LEAVES => {
                    let mut pb = path.clone();
                    pb.push(true);
                    let mut pa = path;
                    pa.push(false);
                    // `a` pops first.
                    stack.push((pb, (**b).clone(), xf));
                    stack.push((pa, (**a).clone(), xf));
                }
                SolidShape::Boolean { .. } => {}
                _ => out.push((path, Solid3d { shape: s.shape.clone(), xf })),
            }
        }
        out
    }

    /// The node at `path` (`self` for an empty path).
    pub fn node(&self, path: &[bool]) -> Option<&Solid3d> {
        let mut s = self;
        for &right in path {
            let SolidShape::Boolean { a, b, .. } = &s.shape else { return None };
            s = if right { b } else { a };
        }
        Some(s)
    }

    pub fn node_mut(&mut self, path: &[bool]) -> Option<&mut Solid3d> {
        let mut s = self;
        for &right in path {
            let SolidShape::Boolean { a, b, .. } = &mut s.shape else { return None };
            s = if right { b } else { a };
        }
        Some(s)
    }

    /// The surfaces as tagged convex polygons facing outward, in drawing coordinates (curves to
    /// `tol`). Empty when the solid isn't valid or a boolean grew too large.
    pub fn polys(&self, tol: f64) -> Arc<Vec<Poly>> {
        let key = cache_key(self, tol);
        if let Some(hit) = cache_get(key) {
            return hit;
        }
        let mut leaf = 0u32;
        let polys = Arc::new(if self.is_valid() { self.tree_polys(tol, &Mat4::IDENTITY, &mut leaf).unwrap_or_default() } else { Vec::new() });
        cache_put(key, polys.clone());
        polys
    }

    fn tree_polys(&self, tol: f64, outer: &Mat4, leaf: &mut u32) -> Option<Vec<Poly>> {
        let xf = outer.mul(&self.xf);
        match &self.shape {
            SolidShape::Boolean { op, a, b } => {
                let pa = a.tree_polys(tol, &xf, leaf)?;
                let pb = b.tree_polys(tol, &xf, leaf)?;
                let scale = pa.iter().chain(&pb).flat_map(|p| p.pts.iter()).fold(1.0f64, |m, p| m.max(p.x.abs()).max(p.y.abs()).max(p.z.abs()));
                let op = match op {
                    BoolOp::Union => bsp::Op::Union,
                    BoolOp::Subtract => bsp::Op::Subtract,
                    BoolOp::Intersect => bsp::Op::Intersect,
                };
                bsp::boolean(pa, pb, op, 1e-9 * scale)
            }
            _ => {
                let me = *leaf;
                *leaf = leaf.saturating_add(1);
                Some(Solid3d { shape: self.shape.clone(), xf }.leaf_polys(tol, me))
            }
        }
    }

    /// A leaf's surfaces as tagged polygons: quads between stations (two triangles where a quad
    /// isn't flat) and the triangulated ends, turned outward.
    fn leaf_polys(&self, tol: f64, leaf: u32) -> Vec<Poly> {
        let r = self.rings(tol);
        let n = r.stations.len();
        let mut out = Vec::new();
        let size = r.stations.iter().flatten().flatten().fold(0.0f64, |m, p| m.max(p.x.abs()).max(p.y.abs()).max(p.z.abs())).max(1e-300);
        let flat_eps = 1e-12 * size.max(1.0);
        // Per loop, which run of points between corners each point starts.
        let groups: Vec<Vec<usize>> = r
            .long_wires
            .iter()
            .map(|m| {
                let mut g = 0usize;
                m.iter()
                    .enumerate()
                    .map(|(i, c)| {
                        if *c && i > 0 {
                            g += 1;
                        }
                        g
                    })
                    .collect()
            })
            .collect();
        let pairs = if r.wrap { n } else { n.saturating_sub(1) };
        for s in 0..pairs {
            let (Some(a), Some(b)) = (r.stations.get(s), r.stations.get((s + 1) % n)) else { continue };
            for (li, (la, lb)) in a.iter().zip(b).enumerate() {
                let m = la.len().min(lb.len());
                for i in 0..m {
                    let j = (i + 1) % m;
                    let g = groups.get(li).and_then(|g| g.get(i)).copied().unwrap_or(0);
                    // The last run of a closed loop continues the first unless point 0 is a corner.
                    let wraps = r.long_wires.get(li).and_then(|w| w.first()).is_some_and(|c| !c);
                    let g = if wraps && groups.get(li).and_then(|gg| gg.last()) == Some(&g) { 0 } else { g };
                    let t = tag(leaf, side_face(li, g));
                    let mut quad = vec![la[i], la[j], lb[j], lb[i]];
                    quad.dedup_by(|x, y| (*x - *y).len() <= flat_eps);
                    if quad.len() > 1 && quad.first().zip(quad.last()).is_some_and(|(x, y)| (*x - *y).len() <= flat_eps) {
                        quad.pop();
                    }
                    if quad.len() == 4 {
                        let flat = Poly::new(vec![quad[0], quad[1], quad[2]], t)
                            .is_some_and(|p| (p.plane.n.dot(quad[3]) - p.plane.w).abs() <= flat_eps * 1e3);
                        if !flat {
                            out.extend(Poly::new(vec![quad[0], quad[1], quad[2]], t));
                            out.extend(Poly::new(vec![quad[0], quad[2], quad[3]], t));
                            continue;
                        }
                    }
                    out.extend(Poly::new(quad, t));
                }
            }
        }
        if r.caps && n >= 2 {
            for (si, face, rev) in [(0, FACE_START, true), (n - 1, FACE_END, false)] {
                let Some(st) = r.stations.get(si) else { continue };
                let loops: Vec<Vec<Vec3>> = st.iter().map(|l| if rev { l.iter().rev().copied().collect() } else { l.clone() }).collect();
                cap_polys(&loops, tag(leaf, face), &mut out);
            }
        }
        // Outward: the stations of some shapes run the other way round.
        if bsp::mass(&out).0 < 0.0 {
            for p in &mut out {
                p.pts.reverse();
                p.plane.n = p.plane.n * -1.0;
                p.plane.w = -p.plane.w;
            }
        }
        out
    }

    /// The edges between faces (where the tagged polygons of one face meet another's), as
    /// segments in drawing coordinates.
    pub fn feature_edges(&self, tol: f64) -> Vec<[Vec3; 2]> {
        feature_edges(&self.polys(tol))
    }
}

/// One planar end: its loops (outer counter-clockwise seen from outside) as triangles.
fn cap_polys(loops: &[Vec<Vec3>], t: u64, out: &mut Vec<Poly>) {
    let live: Vec<&Vec<Vec3>> = loops.iter().filter(|l| l.len() >= 3).collect();
    let mut nrm = Vec3::ZERO;
    for l in &live {
        let m = l.len();
        for i in 0..m {
            nrm = nrm + l[i].cross(l[(i + 1) % m]);
        }
    }
    let nrm = nrm.normalized();
    if nrm.len() < 0.5 {
        return;
    }
    let (ax, ay) = frame_for(nrm);
    let Some(o) = live.first().and_then(|l| l.first()).copied() else { return };
    let flat: Vec<Vec<Vec2>> = live.iter().map(|l| l.iter().map(|p| Vec2::new((*p - o).dot(ax), (*p - o).dot(ay))).collect()).collect();
    let pts: Vec<Vec3> = live.iter().flat_map(|l| l.iter().copied()).collect();
    for [a, b, c] in super::triangulate(&flat) {
        if let (Some(pa), Some(pb), Some(pc)) = (pts.get(a), pts.get(b), pts.get(c)) {
            out.extend(Poly::new(vec![*pa, *pb, *pc], t));
        }
    }
}

/// Where faces meet: for each face, the parts of its polygons' edges that no other polygon of the
/// same face covers the other way (splits leave T-junctions, so edges are compared along their
/// lines, not end to end); each such boundary once.
pub fn feature_edges(polys: &[Poly]) -> Vec<[Vec3; 2]> {
    use std::collections::HashMap;
    let scale = polys.iter().flat_map(|p| p.pts.iter()).fold(1.0f64, |m, p| m.max(p.x.abs()).max(p.y.abs()).max(p.z.abs()));
    let q = |v: f64, quantum: f64| (v / quantum).round() as i64;
    let oq = scale * 1e-7;
    // A flat face is known by its plane (so flat faces of different operands lying in one plane
    // are one face), a curved one by its tag.
    let mut planes: HashMap<u64, Option<(Vec3, f64)>> = HashMap::new();
    for p in polys {
        let e = planes.entry(p.tag).or_insert(Some((p.plane.n, p.plane.w)));
        if let Some((n, w)) = *e
            && (n.dot(p.plane.n) < 1.0 - 1e-9 || (w - p.plane.w).abs() > oq)
        {
            *e = None;
        }
    }
    let face_key = |p: &Poly| -> u64 {
        match planes.get(&p.tag).copied().flatten() {
            Some((n, w)) => {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                [q(n.x, 1e-6), q(n.y, 1e-6), q(n.z, 1e-6), q(w, oq)].hash(&mut h);
                h.finish() | 1
            }
            None => p.tag << 1,
        }
    };
    // (face, line) → directed intervals along the line.
    type LineKey = (u64, [i64; 6]);
    let mut lines: HashMap<LineKey, (Vec3, Vec3, Vec<(f64, f64, i32)>)> = HashMap::new();
    for p in polys {
        let n = p.pts.len();
        for i in 0..n {
            let (a, b) = (p.pts[i], p.pts[(i + 1) % n]);
            let d = b - a;
            let len = d.len();
            if !(len.is_finite() && len > oq) {
                continue;
            }
            let mut u = d * (1.0 / len);
            // One direction per line.
            let flip = u.x < -1e-9 || (u.x.abs() <= 1e-9 && (u.y < -1e-9 || (u.y.abs() <= 1e-9 && u.z < 0.0)));
            if flip {
                u = u * -1.0;
            }
            let o = a - u * u.dot(a);
            let key = (face_key(p), [q(u.x, 1e-6), q(u.y, 1e-6), q(u.z, 1e-6), q(o.x, oq), q(o.y, oq), q(o.z, oq)]);
            let (ta, tb) = (u.dot(a), u.dot(b));
            let e = lines.entry(key).or_insert((u, o, Vec::new()));
            e.2.push((ta.min(tb), ta.max(tb), if flip { -1 } else { 1 }));
        }
    }
    let mut out: Vec<[Vec3; 2]> = Vec::new();
    let mut seen: std::collections::HashSet<[i64; 6]> = std::collections::HashSet::new();
    for (_, (u, o, iv)) in lines {
        let mut ev: Vec<(f64, i32)> = iv.iter().flat_map(|&(s, e, w)| [(s, w), (e, -w)]).collect();
        ev.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut w = 0;
        let mut start = 0.0;
        for (t, dw) in ev {
            let before = w;
            w += dw;
            if before == 0 && w != 0 {
                start = t;
            } else if before != 0 && w == 0 && t - start > oq {
                let (a, b) = (o + u * start, o + u * t);
                let k = [q(a.x, oq * 10.0), q(a.y, oq * 10.0), q(a.z, oq * 10.0), q(b.x, oq * 10.0), q(b.y, oq * 10.0), q(b.z, oq * 10.0)];
                if seen.insert(k) {
                    out.push([a, b]);
                }
            }
        }
    }
    out
}

/// The edges a drawing of the polygons shows from `view_dir` (towards the viewer), each with
/// whether it is drawn: edges between flat faces, a curved face's edges where it meets another
/// at more than 20°, and curved faces' outlines (where they turn away from the viewer). Smooth
/// joins (a fillet's tangent edges, a cylinder's isolines) are given as not drawn.
pub fn drawing_edges(polys: &[Poly], view_dir: Vec3) -> Vec<([Vec3; 2], bool)> {
    use std::collections::HashMap;
    let scale = polys.iter().flat_map(|p| p.pts.iter()).fold(1.0f64, |m, p| m.max(p.x.abs()).max(p.y.abs()).max(p.z.abs()));
    let eps = scale * 1e-7;
    let mut planar: HashMap<u64, Option<Vec3>> = HashMap::new();
    for p in polys {
        let e = planar.entry(p.tag).or_insert(Some(p.plane.n));
        if let Some(n) = *e
            && n.dot(p.plane.n) < 1.0 - 1e-9
        {
            *e = None;
        }
    }
    let flat = |t: u64| planar.get(&t).copied().flatten().is_some();
    let d = view_dir.normalized();
    let cos20 = 20f64.to_radians().cos();
    let mut out = Vec::new();
    // Polygons by grid cell of their bounds, to find those along an edge.
    let cell = (scale * 0.02).max(eps);
    let key = |p: Vec3| [(p.x / cell).floor() as i64, (p.y / cell).floor() as i64, (p.z / cell).floor() as i64];
    let mut near: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (i, p) in polys.iter().enumerate() {
        let (mut lo, mut hi) = ([i64::MAX; 3], [i64::MIN; 3]);
        for v in &p.pts {
            let k = key(*v);
            for a in 0..3 {
                lo[a] = lo[a].min(k[a]);
                hi[a] = hi[a].max(k[a]);
            }
        }
        let cells = (hi[0] - lo[0] + 1).saturating_mul(hi[1] - lo[1] + 1).saturating_mul(hi[2] - lo[2] + 1);
        if cells > 4096 {
            near.entry([i64::MIN; 3]).or_default().push(i);
            continue;
        }
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    near.entry([x, y, z]).or_default().push(i);
                }
            }
        }
    }
    let on_edge = |p: &Poly, m: Vec3| {
        let n = p.pts.len();
        (0..n).any(|i| {
            let (a, b) = (p.pts[i], p.pts[(i + 1) % n]);
            let ab = b - a;
            let l2 = ab.dot(ab);
            let t = if l2 > 0.0 { ((m - a).dot(ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
            (a + ab * t - m).len() <= eps * 10.0
        })
    };
    for seg in feature_edges(polys) {
        let m = (seg[0] + seg[1]) * 0.5;
        let mut sides: Vec<(u64, Vec3)> = Vec::new();
        let big = near.get(&[i64::MIN; 3]).map(Vec::as_slice).unwrap_or(&[]);
        for &i in near.get(&key(m)).map(Vec::as_slice).unwrap_or(&[]).iter().chain(big) {
            let p = &polys[i];
            if on_edge(p, m) && !sides.iter().any(|(t, n)| *t == p.tag && n.dot(p.plane.n) > 1.0 - 1e-9) {
                sides.push((p.tag, p.plane.n));
            }
        }
        let sharp = match sides.as_slice() {
            [(ta, na), (tb, nb), ..] => (flat(*ta) && flat(*tb)) || na.dot(*nb) < cos20 || na.dot(d) * nb.dot(d) < 0.0,
            _ => true,
        };
        out.push((seg, sharp));
    }
    // Outlines inside curved faces: edges between two of the face's polygons, one facing the
    // viewer and one facing away.
    let q = |p: Vec3| [(p.x / eps).round() as i64, (p.y / eps).round() as i64, (p.z / eps).round() as i64];
    let mut edges: HashMap<([i64; 3], [i64; 3], u64), Vec<(Vec3, [Vec3; 2])>> = HashMap::new();
    for p in polys.iter().filter(|p| !flat(p.tag)) {
        let n = p.pts.len();
        for i in 0..n {
            let (a, b) = (p.pts[i], p.pts[(i + 1) % n]);
            let (ka, kb) = (q(a), q(b));
            let k = if ka <= kb { (ka, kb, p.tag) } else { (kb, ka, p.tag) };
            edges.entry(k).or_default().push((p.plane.n, [a, b]));
        }
    }
    for (_, faces) in edges {
        if let [(na, seg), (nb, _)] = faces.as_slice()
            && na.dot(d) * nb.dot(d) < 0.0
        {
            out.push((*seg, true));
        }
    }
    out
}

type CacheEntry = (u64, Arc<Vec<Poly>>);

fn cache() -> &'static Mutex<Vec<CacheEntry>> {
    static CACHE: OnceLock<Mutex<Vec<CacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Most evaluated solids kept.
const CACHE_SIZE: usize = 64;

fn cache_key(s: &Solid3d, tol: f64) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(s).unwrap_or_default().hash(&mut h);
    tol.to_bits().hash(&mut h);
    h.finish()
}

fn cache_get(key: u64) -> Option<Arc<Vec<Poly>>> {
    let c = cache().lock().ok()?;
    c.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone())
}

fn cache_put(key: u64, v: Arc<Vec<Poly>>) {
    if let Ok(mut c) = cache().lock() {
        if c.len() >= CACHE_SIZE {
            c.remove(0);
        }
        c.push((key, v));
    }
}
