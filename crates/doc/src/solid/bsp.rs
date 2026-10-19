//! Boolean operations on closed polygon meshes with binary space partitioning trees (Naylor,
//! Amanatides and Thibault, "Merging BSP trees yields polyhedral set operations", 1990): each
//! solid's polygons build a tree of their planes; clipping one solid's polygons by the other's
//! tree keeps the parts outside (or inside) it. Polygons are convex and carry a tag (which face of
//! which operand they came from) through every split.
//!
//! Trees live in an arena and every walk uses an explicit stack, so deep trees can't overflow
//! the call stack; the polygon count is capped.

use cadaddy_geom::Vec3;

/// Most polygons an operation works with; beyond that it gives up (`None`).
pub const MAX_POLYS: usize = 2_000_000;

#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub n: Vec3,
    pub w: f64,
}

impl Plane {
    fn from_points(a: Vec3, b: Vec3, c: Vec3) -> Option<Plane> {
        let n = (b - a).cross(c - a);
        let l = n.len();
        if !(l.is_finite() && l > 1e-300) {
            return None;
        }
        let n = n * (1.0 / l);
        Some(Plane { n, w: n.dot(a) })
    }
    fn flip(&mut self) {
        self.n = self.n * -1.0;
        self.w = -self.w;
    }
}

/// A convex planar polygon and the face it belongs to.
#[derive(Clone, Debug)]
pub struct Poly {
    pub pts: Vec<Vec3>,
    pub plane: Plane,
    pub tag: u64,
}

impl Poly {
    /// A polygon from its points (counter-clockwise seen from outside); `None` when they don't
    /// span a plane.
    pub fn new(pts: Vec<Vec3>, tag: u64) -> Option<Poly> {
        let n = pts.len();
        if n < 3 {
            return None;
        }
        // Newell's normal: robust for any convex polygon.
        let mut nrm = Vec3::ZERO;
        for i in 0..n {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            nrm = nrm + Vec3::new((a.y - b.y) * (a.z + b.z), (a.z - b.z) * (a.x + b.x), (a.x - b.x) * (a.y + b.y));
        }
        let l = nrm.len();
        if !(l.is_finite() && l > 1e-300) {
            return Plane::from_points(pts[0], pts[1], pts[2]).map(|plane| Poly { pts, plane, tag });
        }
        let nrm = nrm * (1.0 / l);
        let c = pts.iter().fold(Vec3::ZERO, |s, p| s + *p) * (1.0 / n as f64);
        Some(Poly { pts, plane: Plane { n: nrm, w: nrm.dot(c) }, tag })
    }
    fn flip(&mut self) {
        self.pts.reverse();
        self.plane.flip();
    }
}

const COPLANAR: u8 = 0;
const FRONT: u8 = 1;
const BACK: u8 = 2;
const SPANNING: u8 = 3;

/// Sort `p` by `plane`: coplanar parts go to `cf` or `cb` by their facing, the rest to `f`/`b`
/// (split where they span it).
fn split(plane: &Plane, p: Poly, eps: f64, cf: &mut Vec<Poly>, cb: &mut Vec<Poly>, f: &mut Vec<Poly>, b: &mut Vec<Poly>) {
    let mut kind = 0u8;
    let types: Vec<u8> = p
        .pts
        .iter()
        .map(|v| {
            let t = plane.n.dot(*v) - plane.w;
            let k = if t < -eps {
                BACK
            } else if t > eps {
                FRONT
            } else {
                COPLANAR
            };
            kind |= k;
            k
        })
        .collect();
    match kind {
        COPLANAR => {
            if plane.n.dot(p.plane.n) > 0.0 {
                cf.push(p)
            } else {
                cb.push(p)
            }
        }
        FRONT => f.push(p),
        BACK => b.push(p),
        _ => {
            let n = p.pts.len();
            let (mut fp, mut bp) = (Vec::with_capacity(n + 1), Vec::with_capacity(n + 1));
            for i in 0..n {
                let j = (i + 1) % n;
                let (ti, tj) = (types[i], types[j]);
                let (vi, vj) = (p.pts[i], p.pts[j]);
                if ti != BACK {
                    fp.push(vi);
                }
                if ti != FRONT {
                    bp.push(vi);
                }
                if (ti | tj) == SPANNING {
                    let d = plane.n.dot(vj - vi);
                    if d.abs() > 1e-300 {
                        let t = ((plane.w - plane.n.dot(vi)) / d).clamp(0.0, 1.0);
                        let v = vi + (vj - vi) * t;
                        fp.push(v);
                        bp.push(v);
                    }
                }
            }
            if fp.len() >= 3 {
                f.push(Poly { pts: fp, plane: p.plane, tag: p.tag });
            }
            if bp.len() >= 3 {
                b.push(Poly { pts: bp, plane: p.plane, tag: p.tag });
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Node {
    plane: Option<Plane>,
    front: Option<usize>,
    back: Option<usize>,
    polys: Vec<Poly>,
}

/// A BSP tree of one solid's polygons.
#[derive(Clone, Debug)]
pub struct Bsp {
    nodes: Vec<Node>,
    eps: f64,
    /// Too many polygons: the result can't be trusted.
    pub overflow: bool,
}

impl Bsp {
    pub fn new(polys: Vec<Poly>, eps: f64) -> Bsp {
        let mut t = Bsp { nodes: vec![Node::default()], eps, overflow: false };
        t.build(0, polys);
        t
    }

    fn count(&self) -> usize {
        self.nodes.iter().map(|n| n.polys.len()).sum()
    }

    /// Add polygons under node `at`.
    fn build(&mut self, at: usize, polys: Vec<Poly>) {
        let mut stack = vec![(at, polys)];
        while let Some((i, polys)) = stack.pop() {
            if polys.is_empty() {
                continue;
            }
            if self.nodes.len() + polys.len() > MAX_POLYS {
                self.overflow = true;
                return;
            }
            let Some(node) = self.nodes.get_mut(i) else { continue };
            let plane = *node.plane.get_or_insert(polys[0].plane);
            let (mut cf, mut cb, mut f, mut b) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for p in polys {
                split(&plane, p, self.eps, &mut cf, &mut cb, &mut f, &mut b);
            }
            node.polys.extend(cf);
            node.polys.extend(cb);
            let (front, back) = (node.front, node.back);
            for (child, list, is_front) in [(front, f, true), (back, b, false)] {
                if list.is_empty() {
                    continue;
                }
                let c = match child {
                    Some(c) => c,
                    None => {
                        self.nodes.push(Node::default());
                        let c = self.nodes.len() - 1;
                        if let Some(n) = self.nodes.get_mut(i) {
                            if is_front {
                                n.front = Some(c);
                            } else {
                                n.back = Some(c);
                            }
                        }
                        c
                    }
                };
                stack.push((c, list));
            }
        }
    }

    /// Solid space becomes empty space and the other way round.
    fn invert(&mut self) {
        for n in &mut self.nodes {
            for p in &mut n.polys {
                p.flip();
            }
            if let Some(pl) = &mut n.plane {
                pl.flip();
            }
            std::mem::swap(&mut n.front, &mut n.back);
        }
    }

    /// The parts of `polys` outside this tree's solid.
    fn clip_polygons(&self, polys: Vec<Poly>) -> Vec<Poly> {
        let mut out = Vec::new();
        let mut stack = vec![(0usize, polys)];
        while let Some((i, polys)) = stack.pop() {
            let Some(node) = self.nodes.get(i) else {
                out.extend(polys);
                continue;
            };
            let Some(plane) = node.plane else {
                out.extend(polys);
                continue;
            };
            let (mut cf, mut cb, mut f, mut b) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for p in polys {
                split(&plane, p, self.eps, &mut cf, &mut cb, &mut f, &mut b);
            }
            f.extend(cf);
            b.extend(cb);
            match node.front {
                Some(c) => stack.push((c, f)),
                None => out.extend(f),
            }
            // Behind a leaf is inside the solid: dropped.
            if let Some(c) = node.back {
                stack.push((c, b));
            }
            if out.len() > MAX_POLYS {
                break;
            }
        }
        out
    }

    /// Remove the parts of this tree's polygons inside `other`.
    fn clip_to(&mut self, other: &Bsp) {
        for i in 0..self.nodes.len() {
            let polys = std::mem::take(&mut self.nodes[i].polys);
            self.nodes[i].polys = other.clip_polygons(polys);
        }
        if self.count() > MAX_POLYS {
            self.overflow = true;
        }
    }

    pub fn polygons(&self) -> Vec<Poly> {
        self.nodes.iter().flat_map(|n| n.polys.iter().cloned()).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Union,
    Subtract,
    Intersect,
}

/// `a` op `b` for closed, outward-facing polygon sets; `None` when it grew too large.
pub fn boolean(a: Vec<Poly>, b: Vec<Poly>, op: Op, eps: f64) -> Option<Vec<Poly>> {
    let mut a = Bsp::new(a, eps);
    let mut b = Bsp::new(b, eps);
    match op {
        Op::Union => {
            a.clip_to(&b);
            b.clip_to(&a);
            b.invert();
            b.clip_to(&a);
            b.invert();
            let bp = b.polygons();
            a.build(0, bp);
        }
        Op::Subtract => {
            a.invert();
            a.clip_to(&b);
            b.clip_to(&a);
            b.invert();
            b.clip_to(&a);
            b.invert();
            let bp = b.polygons();
            a.build(0, bp);
            a.invert();
        }
        Op::Intersect => {
            a.invert();
            b.clip_to(&a);
            b.invert();
            a.clip_to(&b);
            b.clip_to(&a);
            let bp = b.polygons();
            a.build(0, bp);
            a.invert();
        }
    }
    (!a.overflow && !b.overflow).then(|| a.polygons())
}

/// Volume, area and centroid of a closed polygon set.
pub fn mass(polys: &[Poly]) -> (f64, f64, Vec3) {
    let (mut v, mut a, mut c) = (0.0, 0.0, Vec3::ZERO);
    for p in polys {
        let Some(&o) = p.pts.first() else { continue };
        for k in 1..p.pts.len().saturating_sub(1) {
            let (q, r) = (p.pts[k], p.pts[k + 1]);
            let tv = o.dot(q.cross(r)) / 6.0;
            v += tv;
            c = c + (o + q + r) * (tv / 4.0);
            a += (q - o).cross(r - o).len() / 2.0;
        }
    }
    if v.abs() < 1e-300 { (0.0, a, Vec3::ZERO) } else { (v, a, c * (1.0 / v)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An axis-aligned box as six quads.
    fn cube(lo: Vec3, hi: Vec3, tag: u64) -> Vec<Poly> {
        let v = |x: bool, y: bool, z: bool| Vec3::new(if x { hi.x } else { lo.x }, if y { hi.y } else { lo.y }, if z { hi.z } else { lo.z });
        let f = |a: [(bool, bool, bool); 4]| Poly::new(a.iter().map(|&(x, y, z)| v(x, y, z)).collect(), tag);
        [
            f([(false, false, false), (false, true, false), (true, true, false), (true, false, false)]),
            f([(false, false, true), (true, false, true), (true, true, true), (false, true, true)]),
            f([(false, false, false), (true, false, false), (true, false, true), (false, false, true)]),
            f([(false, true, false), (false, true, true), (true, true, true), (true, true, false)]),
            f([(false, false, false), (false, false, true), (false, true, true), (false, true, false)]),
            f([(true, false, false), (true, true, false), (true, true, true), (true, false, true)]),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    fn vol(p: &[Poly]) -> f64 {
        mass(p).0
    }

    #[test]
    fn overlapping_touching_coplanar_and_nested_boxes() {
        let a = || cube(Vec3::ZERO, Vec3::new(2.0, 2.0, 2.0), 1);
        let b = || cube(Vec3::new(1.0, 1.0, 1.0), Vec3::new(3.0, 3.0, 3.0), 2);
        let e = 1e-9;
        assert!((vol(&boolean(a(), b(), Op::Union, e).unwrap()) - 15.0).abs() < 1e-9);
        assert!((vol(&boolean(a(), b(), Op::Subtract, e).unwrap()) - 7.0).abs() < 1e-9);
        assert!((vol(&boolean(a(), b(), Op::Intersect, e).unwrap()) - 1.0).abs() < 1e-9);
        // Sharing a face.
        let c = || cube(Vec3::new(2.0, 0.0, 0.0), Vec3::new(4.0, 2.0, 2.0), 3);
        assert!((vol(&boolean(a(), c(), Op::Union, e).unwrap()) - 16.0).abs() < 1e-9);
        assert!((vol(&boolean(a(), c(), Op::Subtract, e).unwrap()) - 8.0).abs() < 1e-9);
        assert!(vol(&boolean(a(), c(), Op::Intersect, e).unwrap()).abs() < 1e-9);
        // Coplanar faces: a box flush with the top of another.
        let d = || cube(Vec3::new(0.5, 0.5, 1.0), Vec3::new(1.5, 1.5, 2.0), 4);
        assert!((vol(&boolean(a(), d(), Op::Subtract, e).unwrap()) - 7.0).abs() < 1e-9);
        assert!((vol(&boolean(a(), d(), Op::Union, e).unwrap()) - 8.0).abs() < 1e-9);
        // Nested.
        let n = || cube(Vec3::new(0.5, 0.5, 0.5), Vec3::new(1.5, 1.5, 1.5), 5);
        assert!((vol(&boolean(a(), n(), Op::Subtract, e).unwrap()) - 7.0).abs() < 1e-9);
        assert!((vol(&boolean(a(), n(), Op::Intersect, e).unwrap()) - 1.0).abs() < 1e-9);
        // Tags survive.
        assert!(boolean(a(), b(), Op::Subtract, e).unwrap().iter().any(|p| p.tag == 2));
    }
}
