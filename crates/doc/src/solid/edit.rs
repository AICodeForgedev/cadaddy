//! Editing solids: SLICE's half-spaces, FILLETEDGE and CHAMFEREDGE cutters, SHELL, and SOLIDEDIT's
//! face move, offset and delete. Edits change the definition (the composite tree and its leaves'
//! sizes), so the solid stays exact.

use cadaddy_geom::{Mat4, PolyVertex, Vec2, Vec3};

use super::bsp::Poly;
use super::csg::{BoolOp, FACE_END, FACE_START, tag_face, tag_leaf};
use super::{Solid3d, SolidShape, frame_for, mat_vec, offset_loop, placement};

/// Combine two solids.
pub fn combine(a: Solid3d, b: Solid3d, op: BoolOp) -> Solid3d {
    Solid3d::new(SolidShape::Boolean { op, a: Box::new(a), b: Box::new(b) })
}

/// A box reaching `size` from `p` on the side of the plane (through `p`, normal `n`) that `n`
/// points away from: intersect with it to keep that side.
pub fn half_space(p: Vec3, n: Vec3, size: f64) -> Solid3d {
    let n = n.normalized();
    let (x, y) = frame_for(n);
    let o = p - x * size - y * size - n * (2.0 * size);
    Solid3d { shape: SolidShape::Box { length: 2.0 * size, width: 2.0 * size, height: 2.0 * size }, xf: placement(o, x, y, n) }
}

/// A straight edge between two flat faces of a solid.
#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub a: Vec3,
    pub b: Vec3,
    /// The faces' outward normals.
    pub n1: Vec3,
    pub n2: Vec3,
    /// In each face, square to the edge, pointing away from it into the face.
    pub w1: Vec3,
    pub w2: Vec3,
}

fn inverse3(m: &Mat4) -> Option<[[f64; 3]; 3]> {
    let a = &m.m;
    let det = super::det3(m);
    if !(det.is_finite() && det.abs() > 1e-300) {
        return None;
    }
    let c = |r1: usize, c1: usize, r2: usize, c2: usize| a[r1][c1] * a[r2][c2] - a[r1][c2] * a[r2][c1];
    let inv = [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ];
    Some(inv.map(|r| r.map(|v| v / det)))
}

fn apply3(m: &[[f64; 3]; 3], v: Vec3) -> Vec3 {
    Vec3::new(
        m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
        m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
        m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
    )
}

/// Move a placement by `v` (in the coordinates it is placed in).
fn shift(xf: &mut Mat4, v: Vec3) {
    xf.m[0][3] += v.x;
    xf.m[1][3] += v.y;
    xf.m[2][3] += v.z;
}

fn in_convex(p: &Poly, q: Vec3) -> bool {
    let n = p.pts.len();
    (0..n).all(|i| (p.pts[(i + 1) % n] - p.pts[i]).cross(q - p.pts[i]).dot(p.plane.n) >= -1e-12 * (1.0 + q.len()))
}

impl Solid3d {
    /// The face at `p`: seen from above (`p`'s X and Y, the highest face there) when
    /// `from_above`, else the face nearest `p`. Its tag and outward normal.
    pub fn face_at(&self, p: Vec3, from_above: bool, tol: f64) -> Option<(u64, Vec3)> {
        let polys = self.polys(tol);
        let mut best: Option<(f64, u64, Vec3)> = None;
        for poly in polys.iter() {
            let n = poly.plane.n;
            let score = if from_above {
                if n.z.abs() < 1e-9 {
                    continue;
                }
                let z = (poly.plane.w - n.x * p.x - n.y * p.y) / n.z;
                let q = Vec3::new(p.x, p.y, z);
                if !in_convex(poly, q) {
                    continue;
                }
                -z
            } else {
                let d = n.dot(p) - poly.plane.w;
                let q = p - n * d;
                if in_convex(poly, q) { d.abs() } else { poly.pts.iter().map(|v| (*v - p).len()).fold(f64::MAX, f64::min) }
            };
            if best.is_none_or(|(s, _, _)| score < s) {
                best = Some((score, poly.tag, n));
            }
        }
        best.map(|(_, t, n)| (t, n))
    }

    /// The path to the leaf a tag names, and whether that leaf's outside is the solid's inside
    /// (it was subtracted).
    fn leaf_of(&self, tag: u64) -> Option<(Vec<bool>, bool)> {
        let (path, _) = self.leaves().into_iter().nth(tag_leaf(tag) as usize)?;
        let mut flipped = false;
        for i in 0..path.len() {
            if let Some(Solid3d { shape: SolidShape::Boolean { op: BoolOp::Subtract, .. }, .. }) = self.node(&path[..i])
                && path[i]
            {
                flipped = !flipped;
            }
        }
        Some((path, flipped))
    }

    /// The part on the path that was added or taken away (the second operand of a union or
    /// subtraction) nearest the top: a whole fillet or hole, not a piece of the cutter that made
    /// it. Its path.
    fn added_part(&self, path: &[bool]) -> Option<Vec<bool>> {
        (0..path.len()).find_map(|i| match self.node(&path[..i]) {
            Some(Solid3d { shape: SolidShape::Boolean { op: BoolOp::Subtract | BoolOp::Union, .. }, .. }) if path[i] => Some(path[..=i].to_vec()),
            _ => None,
        })
    }

    /// The solid without the face: the hole, fillet, chamfer or boss it belongs to is removed.
    pub fn delete_face(&self, tag: u64) -> Option<Solid3d> {
        let (path, _) = self.leaf_of(tag)?;
        let part = self.added_part(&path)?;
        let parent = &part[..part.len() - 1];
        let mut out = self.clone();
        let node = out.node_mut(parent)?;
        let SolidShape::Boolean { a, .. } = &node.shape else { return None };
        let mut keep = (**a).clone();
        keep.xf = node.xf.mul(&keep.xf);
        *node = keep;
        Some(out)
    }

    /// The solid with the face moved by `v`: the hole or boss it belongs to moves with it; a
    /// flat face of the main body is offset by the part of `v` along its normal.
    pub fn move_face(&self, tag: u64, v: Vec3, normal: Vec3) -> Option<Solid3d> {
        let (path, _) = self.leaf_of(tag)?;
        match self.added_part(&path) {
            Some(part) => {
                // Into the coordinates the part is placed in.
                let mut m = Mat4::IDENTITY;
                for i in 0..part.len() {
                    m = m.mul(&self.node(&part[..i])?.xf);
                }
                let local = apply3(&inverse3(&m)?, v);
                let mut out = self.clone();
                shift(&mut out.node_mut(&part)?.xf, local);
                out.is_valid().then_some(out)
            }
            None => self.offset_face(tag, v.dot(normal.normalized())),
        }
    }

    /// The solid with the face moved out by `d` (in by a negative `d`) along its normal: a box
    /// side, an end of a cylinder, cone, pyramid or extrusion, the side of a cylinder or a
    /// sphere's surface, wherever it is in the tree.
    pub fn offset_face(&self, tag: u64, d: f64) -> Option<Solid3d> {
        if !d.is_finite() {
            return None;
        }
        let (path, flipped) = self.leaf_of(tag)?;
        let d = if flipped { -d } else { d };
        let face = tag_face(tag);
        let mut out = self.clone();
        let leaf = out.node_mut(&path)?;
        let (x, y, z) = (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
        // The local direction the leaf's origin moves (for faces on its low sides).
        let mut origin_shift = Vec3::ZERO;
        match (&mut leaf.shape, face) {
            (SolidShape::Box { height, .. }, FACE_END) => *height += d,
            (SolidShape::Box { height, .. }, FACE_START) => {
                *height += d;
                origin_shift = z * -d;
            }
            (SolidShape::Box { width, .. }, 2) => {
                *width += d;
                origin_shift = y * -d;
            }
            (SolidShape::Box { length, .. }, 3) => *length += d,
            (SolidShape::Box { width, .. }, 4) => *width += d,
            (SolidShape::Box { length, .. }, 5) => {
                *length += d;
                origin_shift = x * -d;
            }
            (SolidShape::Frustum { height, .. } | SolidShape::Pyramid { height, .. } | SolidShape::Extrusion { height, .. }, FACE_END) => {
                *height += d
            }
            (SolidShape::Frustum { height, .. } | SolidShape::Pyramid { height, .. } | SolidShape::Extrusion { height, .. }, FACE_START) => {
                *height += d;
                origin_shift = z * -d;
            }
            (SolidShape::Frustum { base_radius, top_radius, height }, f) if f >= 2 => {
                // Along the slant: each radius by d / cos(slope).
                let k = ((*base_radius - *top_radius).powi(2) + *height * *height).sqrt() / *height;
                *base_radius += d * k;
                if *top_radius > 0.0 {
                    *top_radius += d * k;
                }
            }
            (SolidShape::Sphere { radius }, _) => *radius += d,
            _ => return None,
        }
        let s = mat_vec(&leaf.xf, origin_shift);
        shift(&mut leaf.xf, s);
        out.is_valid().then_some(out)
    }

    /// The straight edge between two flat faces nearest `p` (seen from above when `from_above`).
    pub fn edge_near(&self, p: Vec3, from_above: bool, tol: f64) -> Option<Edge> {
        let polys = self.polys(tol);
        let segs = super::feature_edges(&polys);
        let dist = |a: Vec3, b: Vec3| {
            let (a, b, q) = if from_above { (Vec3::new(a.x, a.y, 0.0), Vec3::new(b.x, b.y, 0.0), Vec3::new(p.x, p.y, 0.0)) } else { (a, b, p) };
            let d = b - a;
            let l2 = d.dot(d);
            let t = if l2 > 0.0 { ((q - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
            (a + d * t - q).len()
        };
        // Candidates nearest first; the first that is a straight edge between flat faces wins.
        let mut order: Vec<&[Vec3; 2]> = segs.iter().collect();
        // Seen from above, of edges at the same place the highest is the one in view.
        let key = |s: &[Vec3; 2]| (dist(s[0], s[1]), if from_above { -(s[0].z.max(s[1].z)) } else { 0.0 });
        order.sort_by(|s, t| {
            let (a, b) = (key(s), key(t));
            if (a.0 - b.0).abs() <= 1e-9 * (1.0 + a.0.abs()) { a.1.total_cmp(&b.1) } else { a.0.total_cmp(&b.0) }
        });
        let scale = polys.iter().flat_map(|q| q.pts.iter()).fold(1.0f64, |m, v| m.max(v.x.abs()).max(v.y.abs()).max(v.z.abs()));
        let eps = 1e-7 * scale;
        let (lo, hi) = polys
            .iter()
            .flat_map(|q| q.pts.iter())
            .fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(lo, hi), v| {
                (Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z)), Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z)))
            });
        let reach = 0.1 * (hi - lo).len();
        for [a, b] in order.into_iter().take(8) {
            // Picks well away from the solid find nothing.
            if dist(*a, *b) > reach {
                break;
            }
            let u = (*b - *a).normalized();
            if u.len() < 0.5 {
                continue;
            }
            let on_line = |v: Vec3| ((v - *a) - u * (v - *a).dot(u)).len() <= eps;
            // The whole edge: every feature segment along this line that touches it.
            let (mut t0, mut t1) = (0.0f64, (*b - *a).dot(u));
            let mut grew = true;
            while grew {
                grew = false;
                for [c, e] in &segs {
                    if on_line(*c) && on_line(*e) {
                        let (s0, s1) = ((*c - *a).dot(u), (*e - *a).dot(u));
                        let (s0, s1) = (s0.min(s1), s0.max(s1));
                        if s0 <= t1 + eps && s1 >= t0 - eps && (s0 < t0 - eps || s1 > t1 + eps) {
                            t0 = t0.min(s0);
                            t1 = t1.max(s1);
                            grew = true;
                        }
                    }
                }
            }
            // The two faces along it, both flat.
            let mut faces: Vec<(u64, Vec3, Vec3)> = Vec::new();
            for poly in polys.iter() {
                let n = poly.pts.len();
                let touches = (0..n).any(|i| {
                    let (c, e) = (poly.pts[i], poly.pts[(i + 1) % n]);
                    on_line(c) && on_line(e) && (c - e).len() > eps
                });
                if touches && !faces.iter().any(|f| f.0 == poly.tag) {
                    let centre = poly.pts.iter().fold(Vec3::ZERO, |s, v| s + *v) * (1.0 / n as f64);
                    faces.push((poly.tag, poly.plane.n, centre));
                }
            }
            let [(t1g, n1, c1), (t2g, n2, c2)] = faces[..] else { continue };
            let flat = |t: u64, nrm: Vec3| polys.iter().filter(|q| q.tag == t).all(|q| q.plane.n.dot(nrm) > 1.0 - 1e-9);
            if !flat(t1g, n1) || !flat(t2g, n2) || n1.dot(n2) > 1.0 - 1e-9 {
                continue;
            }
            let e = *a + u * t0;
            let into = |nrm: Vec3, c: Vec3| {
                let w = nrm.cross(u).normalized();
                if w.dot(c - e) < 0.0 { w * -1.0 } else { w }
            };
            return Some(Edge { a: e, b: *a + u * t1, n1, n2, w1: into(n1, c1), w2: into(n2, c2) });
        }
        None
    }

    /// The solid with an edge chamfered: `d` back along both faces.
    pub fn chamfer_edge(&self, e: &Edge, d: f64) -> Result<Solid3d, &'static str> {
        let corner = convex_corner(e)?;
        let s = d.max(0.0);
        if !(s.is_finite() && s > 0.0) {
            return Err("The distance must be more than zero.");
        }
        let out = |w: Vec3, n: Vec3| e.a + w * s + n * s;
        let profile = [e.a + e.w1 * s, e.a + e.w2 * s, out(e.w2, e.n2), corner(s), out(e.w1, e.n1)];
        Ok(combine(self.clone(), prism_along(e, &profile, s), BoolOp::Subtract))
    }

    /// The solid with an edge rounded to radius `r`.
    pub fn fillet_edge(&self, e: &Edge, r: f64) -> Result<Solid3d, &'static str> {
        let corner = convex_corner(e)?;
        if !(r.is_finite() && r > 0.0) {
            return Err("The radius must be more than zero.");
        }
        // The angle between the faces, inside the solid.
        let phi = e.w1.dot(e.w2).clamp(-1.0, 1.0).acos();
        let s = r / (phi / 2.0).tan();
        let c = e.a + (e.w1 + e.w2).normalized() * (r / (phi / 2.0).sin());
        let (t1, t2) = (e.a + e.w1 * s, e.a + e.w2 * s);
        let profile = [t1, c, t2, t2 + e.n2 * s, corner(s), t1 + e.n1 * s];
        let block = prism_along(e, &profile, s);
        let u = (e.b - e.a).normalized();
        let len = (e.b - e.a).len() + 4.0 * s;
        let (x, y) = frame_for(u);
        let round = Solid3d { shape: SolidShape::Frustum { base_radius: r, top_radius: r, height: len }, xf: placement(c - u * (2.0 * s), x, y, u) };
        Ok(combine(self.clone(), combine(block, round, BoolOp::Subtract), BoolOp::Subtract))
    }

    /// The solid hollowed to walls `t` thick (inward), open where `remove` faces were. Boxes,
    /// cylinders, spheres and straight extrusions.
    pub fn shell(&self, t: f64, remove: &[u64]) -> Result<Solid3d, &'static str> {
        if !(t.is_finite() && t > 0.0) {
            return Err("The thickness must be more than zero.");
        }
        let opened = |f: u32| remove.iter().any(|r| tag_leaf(*r) == 0 && tag_face(*r) == f);
        let z = Vec3::new(0.0, 0.0, 1.0);
        // The inner solid in the outer one's own coordinates; an opened face's side reaches out
        // past it.
        let inner = match &self.shape {
            SolidShape::Box { length, width, height } => {
                let lo = |f: u32| if opened(f) { -t } else { t };
                let (x0, y0, z0) = (lo(5), lo(2), lo(FACE_START));
                let (x1, y1, z1) = (length - lo(3), width - lo(4), height - lo(FACE_END));
                Solid3d {
                    shape: SolidShape::Box { length: x1 - x0, width: y1 - y0, height: z1 - z0 },
                    xf: placement(Vec3::new(x0, y0, z0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), z),
                }
            }
            SolidShape::Frustum { base_radius, top_radius, height } if (base_radius - top_radius).abs() < 1e-12 => {
                let z0 = if opened(FACE_START) { -t } else { t };
                let z1 = if opened(FACE_END) { height + t } else { height - t };
                let r = base_radius - t;
                Solid3d {
                    shape: SolidShape::Frustum { base_radius: r, top_radius: r, height: z1 - z0 },
                    xf: placement(Vec3::new(0.0, 0.0, z0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), z),
                }
            }
            SolidShape::Sphere { radius } => Solid3d::new(SolidShape::Sphere { radius: radius - t }),
            SolidShape::Extrusion { loops, height, taper } if taper.abs() < 1e-15 => {
                let tol = self.mesh_tol();
                let shrunk: Vec<Vec<PolyVertex>> = super::orient(loops.iter().map(|l| super::loop_points(l, tol)).collect())
                    .iter()
                    .map(|(p, _)| offset_loop(p, t).into_iter().map(PolyVertex::new).collect())
                    .collect();
                let z0 = if opened(FACE_START) { -t } else { t };
                let z1 = if opened(FACE_END) { height + t } else { height - t };
                Solid3d {
                    shape: SolidShape::Extrusion { loops: shrunk, height: z1 - z0, taper: 0.0 },
                    xf: placement(Vec3::new(0.0, 0.0, z0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), z),
                }
            }
            _ => return Err("SHELL works on boxes, cylinders, spheres and straight extrusions."),
        };
        if !inner.is_valid() {
            return Err("The walls are too thick for this solid.");
        }
        let mut inner = inner;
        inner.xf = self.xf.mul(&inner.xf);
        let mut outer = self.clone();
        outer.xf = Mat4::IDENTITY;
        let mut shell = combine(outer, Solid3d { shape: inner.shape, xf: inner_local(&self.xf, &inner.xf) }, BoolOp::Subtract);
        shell.xf = self.xf;
        Ok(shell)
    }
}

/// `inner_world` expressed in the coordinates `outer` places.
fn inner_local(outer: &Mat4, inner_world: &Mat4) -> Mat4 {
    let Some(inv) = inverse3(outer) else { return *inner_world };
    let t = Vec3::new(outer.m[0][3], outer.m[1][3], outer.m[2][3]);
    let o = Vec3::new(inner_world.m[0][3], inner_world.m[1][3], inner_world.m[2][3]);
    let col = |c: usize| apply3(&inv, Vec3::new(inner_world.m[0][c], inner_world.m[1][c], inner_world.m[2][c]));
    placement(apply3(&inv, o - t), col(0), col(1), col(2))
}

/// For a convex edge, the point outside the corner `s` away (along both normals); an error for
/// a concave one.
fn convex_corner(e: &Edge) -> Result<impl Fn(f64) -> Vec3 + '_, &'static str> {
    // Convex: each face's inside lies behind the other face.
    if e.w2.dot(e.n1) >= -1e-9 || e.w1.dot(e.n2) >= -1e-9 {
        return Err("Only outside (convex) edges can be filleted or chamfered.");
    }
    Ok(move |s: f64| e.a + (e.n1 + e.n2) * s)
}

/// A prism along the edge (and `margin` past each end) whose section is `profile` (points in
/// the plane square to the edge through its start).
fn prism_along(e: &Edge, profile: &[Vec3], margin: f64) -> Solid3d {
    let u = (e.b - e.a).normalized();
    let (x, y) = (e.w1, u.cross(e.w1));
    let loops = vec![profile.iter().map(|p| PolyVertex::new(Vec2::new((*p - e.a).dot(x), (*p - e.a).dot(y)))).collect()];
    let len = (e.b - e.a).len() + 4.0 * margin;
    Solid3d { shape: SolidShape::Extrusion { loops, height: len, taper: 0.0 }, xf: placement(e.a - u * (2.0 * margin), x, y, u) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn bx(l: f64, w: f64, h: f64) -> Solid3d {
        Solid3d::new(SolidShape::Box { length: l, width: w, height: h })
    }

    fn near(a: f64, b: f64, rel: f64) -> bool {
        (a - b).abs() <= rel * b.abs().max(1e-9)
    }

    #[test]
    fn classic_booleans_have_the_right_volumes() {
        // A 10 cube with a radius-2 hole straight through it.
        let mut cyl = Solid3d::new(SolidShape::Frustum { base_radius: 2.0, top_radius: 2.0, height: 14.0 });
        cyl.transform(&placement(Vec3::new(5.0, 5.0, -2.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)));
        let holed = combine(bx(10.0, 10.0, 10.0), cyl.clone(), BoolOp::Subtract);
        let tol = holed.mesh_tol();
        let p = holed.props(tol);
        assert!(near(p.volume, 1000.0 - 40.0 * PI, 1e-3), "{}", p.volume);
        assert!((p.centroid - Vec3::new(5.0, 5.0, 5.0)).len() < 1e-6);
        // Union, intersection; nested solids; touching solids.
        assert!(near(combine(bx(10.0, 10.0, 10.0), cyl.clone(), BoolOp::Union).props(tol).volume, 1000.0 + 16.0 * PI, 1e-3));
        let i = combine(bx(10.0, 10.0, 10.0), cyl, BoolOp::Intersect).props(tol).volume;
        // The cylinder's chords lose a little of its section (0.3% at this tolerance).
        assert!(near(i, 40.0 * PI, 4e-3), "{i}");
        let mut ball = Solid3d::new(SolidShape::Sphere { radius: 2.0 });
        ball.transform(&placement(Vec3::new(5.0, 5.0, 5.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)));
        assert!(near(combine(bx(10.0, 10.0, 10.0), ball, BoolOp::Subtract).props(tol).volume, 1000.0 - 32.0 / 3.0 * PI, 2e-3));
        let mut next = bx(10.0, 10.0, 10.0);
        next.transform(&placement(Vec3::new(10.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)));
        let both = combine(bx(10.0, 10.0, 10.0), next, BoolOp::Union);
        assert!(near(both.props(0.01).volume, 2000.0, 1e-9));
        // The shared face is gone: the joined box shows a box's 12 edges.
        assert_eq!(merged_edges(&both), 12, "{:?}", both.feature_edges(0.01));
    }

    /// Feature edges merged along their lines.
    fn merged_edges(s: &Solid3d) -> usize {
        let segs = s.feature_edges(0.01);
        let mut lines: Vec<(Vec3, Vec3)> = Vec::new();
        for [a, b] in segs {
            let u = (b - a).normalized();
            if !lines.iter().any(|(p, v)| v.cross(u).len() < 1e-9 && ((a - *p) - *v * (a - *p).dot(*v)).len() < 1e-9) {
                lines.push((a, u));
            }
        }
        lines.len()
    }

    #[test]
    fn slicing_keeps_a_side() {
        let half = combine(bx(4.0, 4.0, 4.0), half_space(Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), 100.0), BoolOp::Intersect);
        assert!(near(half.props(0.01).volume, 16.0, 1e-9), "{}", half.props(0.01).volume);
        // A diagonal cut through opposite edges: half the cube.
        let diag = combine(bx(4.0, 4.0, 4.0), half_space(Vec3::ZERO, Vec3::new(1.0, -1.0, 0.0), 100.0), BoolOp::Intersect);
        assert!(near(diag.props(0.01).volume, 32.0, 1e-9));
    }

    #[test]
    fn fillets_chamfers_and_shells() {
        let b = bx(10.0, 10.0, 10.0);
        let tol = 0.01;
        // The top front edge (y = 0, z = 10), picked from the side.
        let e = b.edge_near(Vec3::new(5.0, -1.0, 11.0), false, tol).unwrap();
        assert!((e.b - e.a).len() > 9.99, "{e:?}");
        let ch = b.chamfer_edge(&e, 2.0).unwrap();
        assert!(near(ch.props(tol).volume, 1000.0 - 2.0 * 2.0 / 2.0 * 10.0, 1e-9), "{}", ch.props(tol).volume);
        let fi = b.fillet_edge(&e, 2.0).unwrap();
        assert!(near(fi.props(0.001).volume, 1000.0 - (4.0 - PI) * 10.0, 1e-3), "{}", fi.props(0.001).volume);
        // A concave edge is refused: the inside corner of an L.
        let l = combine(
            bx(10.0, 10.0, 10.0),
            {
                let mut c = bx(5.0, 10.0, 5.0);
                c.transform(&placement(Vec3::new(5.0, 0.0, 5.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)));
                c
            },
            BoolOp::Subtract,
        );
        let inner = l.edge_near(Vec3::new(5.0, 5.0, 5.0), false, tol).unwrap();
        assert!(l.fillet_edge(&inner, 1.0).is_err());
        // A box hollowed to 1, open at the top: 1000 − 8 · 8 · 9.
        let top = b.face_at(Vec3::new(5.0, 5.0, 0.0), true, tol).unwrap().0;
        let sh = b.shell(1.0, &[top]).unwrap();
        assert!(near(sh.props(tol).volume, 1000.0 - 8.0 * 8.0 * 9.0, 1e-9), "{}", sh.props(tol).volume);
        assert!(b.shell(6.0, &[]).is_err());
    }

    #[test]
    fn faces_move_offset_and_delete() {
        let tol = 0.01;
        let mut hole = Solid3d::new(SolidShape::Frustum { base_radius: 1.0, top_radius: 1.0, height: 12.0 });
        hole.transform(&placement(Vec3::new(3.0, 3.0, -1.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)));
        let s = combine(bx(10.0, 10.0, 10.0), hole, BoolOp::Subtract);
        let v0 = s.props(tol).volume;
        // The hole's wall, picked beside it.
        let (wall, n) = s.face_at(Vec3::new(4.0, 3.0, 5.0), false, tol).unwrap();
        // Offset it by −0.5 (a positive offset adds material, closing a hole): a radius-1.5 hole.
        let wider = s.offset_face(wall, -0.5).unwrap();
        assert!(s.offset_face(wall, 0.5).unwrap().props(tol).volume > s.props(tol).volume);
        assert!(near(wider.props(tol).volume, 1000.0 - 1.5 * 1.5 * PI * 10.0, 2e-3), "{}", wider.props(tol).volume);
        // Move it: the same volume, the hole elsewhere.
        let moved = s.move_face(wall, Vec3::new(4.0, 0.0, 0.0), n).unwrap();
        assert!(near(moved.props(tol).volume, v0, 1e-9));
        assert!(moved.face_at(Vec3::new(8.0, 3.0, 5.0), false, tol).is_some_and(|(t, _)| tag_leaf(t) == 1));
        // Delete it: the plain box.
        assert!(near(s.delete_face(wall).unwrap().props(tol).volume, 1000.0, 1e-9));
        // The box's top: up 2 → 1200 less the longer hole.
        let (top, n) = s.face_at(Vec3::new(8.0, 8.0, 0.0), true, tol).unwrap();
        let taller = s.move_face(top, Vec3::new(0.0, 0.0, 2.0), n).unwrap();
        assert!(near(taller.props(tol).volume, 1200.0 - PI * 11.0, 2e-3), "{}", taller.props(tol).volume);
        // The body itself can't be deleted.
        assert!(s.delete_face(top).is_none());
    }
}
