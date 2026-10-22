//! A NURBS surface object (PLANESURF, SURFBLEND, SURFPATCH, SURFOFFSET, SURFEXTEND, CONVTOSURFACE
//! make them; SURFTRIM cuts holes in them): the surface, the part of its parameter square that
//! is kept, and whether its control vertices are shown (CVSHOW).
//!
//! Trims are loops in the surface's own parameters `(u, v)`: an outer boundary (what is kept,
//! empty: the whole surface) and holes (what is cut away). Trimming only adds loops, so
//! SURFUNTRIM gets the surface back exactly.

use serde::{Deserialize, Serialize};

use crate::geom::{Mat4, NurbsSurface, Vec2, Vec3, point_in_polygon};
use crate::{Mesh, MeshFace, MeshKind};

/// Points in one trim loop, and loops, at most (hostile files).
pub const MAX_TRIM_POINTS: usize = 4_000;
pub const MAX_TRIM_LOOPS: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Surface3d {
    pub nurbs: NurbsSurface,
    /// The kept region's boundary in `(u, v)`; empty: the whole surface.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outer: Vec<[f64; 2]>,
    /// Loops cut away, in `(u, v)`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<Vec<[f64; 2]>>,
    /// Its control vertices drawn and gripped (CVSHOW / CVHIDE).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub cvs_shown: bool,
}

fn poly(l: &[[f64; 2]]) -> Vec<Vec2> {
    l.iter().map(|p| Vec2::new(p[0], p[1])).collect()
}

impl Surface3d {
    pub fn new(nurbs: NurbsSurface) -> Surface3d {
        Surface3d { nurbs, ..Surface3d::default() }
    }

    pub fn is_valid(&self) -> bool {
        let loop_ok = |l: &Vec<[f64; 2]>| l.len() <= MAX_TRIM_POINTS && l.iter().all(|p| p[0].is_finite() && p[1].is_finite());
        self.nurbs.is_valid() && loop_ok(&self.outer) && self.holes.len() <= MAX_TRIM_LOOPS && self.holes.iter().all(loop_ok)
    }

    pub fn is_trimmed(&self) -> bool {
        !self.outer.is_empty() || !self.holes.is_empty()
    }

    /// Whether parameter point `(u, v)` is on the kept part.
    pub fn kept(&self, u: f64, v: f64) -> bool {
        let p = Vec2::new(u, v);
        (self.outer.len() < 3 || point_in_polygon(&poly(&self.outer), p))
            && !self.holes.iter().filter(|h| h.len() >= 3).any(|h| point_in_polygon(&poly(h), p))
    }

    /// Grid steps for a tolerance: finer for bigger, more curved surfaces (8 to 96 each way).
    fn steps(&self, tol: f64) -> usize {
        let size = self.size();
        let n = if tol.is_finite() && tol > 0.0 && size > 0.0 { (size / tol).sqrt() } else { 24.0 };
        let curved = (self.nurbs.degree_u.max(self.nurbs.degree_v) > 1 || self.nurbs.count_u * self.nurbs.count_v > 4) as usize;
        // A flat untrimmed patch is exact with a few cells; trims need cells fine enough to follow.
        if curved == 0 && !self.is_trimmed() { n.clamp(1.0, 8.0) as usize } else { n.clamp(48.0, 96.0) as usize }
    }

    /// The diagonal of the control vertices' box.
    pub fn size(&self) -> f64 {
        let (mut lo, mut hi) =
            (Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY), Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY));
        for p in &self.nurbs.control {
            lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        let d = (hi - lo).len();
        if d.is_finite() { d } else { 0.0 }
    }

    /// The kept part as triangles (a mesh of the grid cells whose middle is kept, split in two).
    pub fn mesh(&self, tol: f64) -> Mesh {
        let n = self.steps(tol);
        let Some(g) = self.nurbs.grid(n, n) else { return Mesh::default() };
        let (u0, u1) = self.nurbs.domain_u();
        let (v0, v1) = self.nurbs.domain_v();
        let cols = n + 1;
        let vertices: Vec<Vec3> = g.iter().flatten().copied().collect();
        let mut faces = Vec::new();
        for a in 0..n {
            for b in 0..n {
                let (u, v) = (u0 + (u1 - u0) * (a as f64 + 0.5) / n as f64, v0 + (v1 - v0) * (b as f64 + 0.5) / n as f64);
                if !self.kept(u, v) {
                    continue;
                }
                let i = |r: usize, c: usize| (r * cols + c) as u32;
                faces.push(MeshFace { v: vec![i(a, b), i(a + 1, b), i(a + 1, b + 1)], hidden: 0b100 });
                faces.push(MeshFace { v: vec![i(a, b), i(a + 1, b + 1), i(a, b + 1)], hidden: 0b001 });
            }
        }
        Mesh { vertices, faces, kind: MeshKind::Subd }
    }

    /// Its area (from the mesh at `tol`).
    pub fn area(&self, tol: f64) -> f64 {
        let m = self.mesh(tol);
        m.faces
            .iter()
            .filter_map(|f| {
                let p = |k: usize| f.v.get(k).and_then(|i| m.vertices.get(*i as usize)).copied();
                let (a, b, c) = (p(0)?, p(1)?, p(2)?);
                Some((b - a).cross(c - a).len() / 2.0)
            })
            .sum()
    }

    /// What is drawn: isolines over the kept part (broken where it is cut away), the trim loops
    /// on the surface, and the control vertex net when shown.
    pub fn wires(&self, tol: f64) -> Vec<Vec<Vec3>> {
        let steps = self.steps(tol).max(8) * 2;
        let (u0, u1) = self.nurbs.domain_u();
        let (v0, v1) = self.nurbs.domain_v();
        let iso = 4usize.max(self.nurbs.count_u.min(12)).min(12);
        let mut out = Vec::new();
        for along_u in [true, false] {
            for c in 0..=iso {
                let f = c as f64 / iso as f64;
                let mut cur: Vec<Vec3> = Vec::new();
                for s in 0..=steps {
                    let g = s as f64 / steps as f64;
                    let (u, v) = if along_u { (u0 + (u1 - u0) * f, v0 + (v1 - v0) * g) } else { (u0 + (u1 - u0) * g, v0 + (v1 - v0) * f) };
                    match (self.kept(u, v), self.nurbs.point(u, v)) {
                        (true, Some(p)) => cur.push(p),
                        _ => {
                            if cur.len() >= 2 {
                                out.push(std::mem::take(&mut cur));
                            }
                            cur.clear();
                        }
                    }
                }
                if cur.len() >= 2 {
                    out.push(cur);
                }
            }
        }
        for l in std::iter::once(&self.outer).chain(self.holes.iter()).filter(|l| l.len() >= 3) {
            let mut pts: Vec<Vec3> = l.iter().filter_map(|p| self.nurbs.point(p[0], p[1])).collect();
            if let Some(first) = pts.first().copied() {
                pts.push(first);
            }
            out.push(pts);
        }
        if self.cvs_shown {
            let s = &self.nurbs;
            for i in 0..s.count_u {
                out.push((0..s.count_v).filter_map(|j| s.cv(i, j)).collect());
            }
            for j in 0..s.count_v {
                out.push((0..s.count_u).filter_map(|i| s.cv(i, j)).collect());
            }
        }
        out.retain(|l| l.len() >= 2);
        out
    }

    /// Mapped by a 3D transform (the trims are in parameters: they don't change).
    pub fn transformed(&self, m: &Mat4) -> Surface3d {
        Surface3d { nurbs: self.nurbs.transformed(m), ..self.clone() }
    }

    /// The surface with its trims removed (SURFUNTRIM).
    pub fn untrimmed(&self) -> Surface3d {
        Surface3d { outer: Vec::new(), holes: Vec::new(), ..self.clone() }
    }

    /// A curve's points mapped to parameters on the surface (each point's closest), as a loop.
    pub fn to_params(&self, pts: &[Vec3]) -> Vec<[f64; 2]> {
        pts.iter().take(MAX_TRIM_POINTS).filter_map(|p| self.nurbs.closest(*p)).map(|(u, v)| [u, v]).collect()
    }

    /// The control vertices (for grips and snaps when shown).
    pub fn cv_points(&self) -> Vec<Vec3> {
        if self.cvs_shown { self.nurbs.control.clone() } else { Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Surface3d {
        Surface3d::new(NurbsSurface::bilinear(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), Vec3::new(0.0, 10.0, 0.0), Vec3::new(10.0, 10.0, 0.0)).unwrap())
    }

    #[test]
    fn a_hole_takes_its_area_and_untrimming_restores_it() {
        let s = square();
        assert!((s.area(0.01) - 100.0).abs() < 1e-9);
        let mut t = s.clone();
        t.holes.push(vec![[0.25, 0.25], [0.75, 0.25], [0.75, 0.75], [0.25, 0.75]]);
        let a = t.area(0.001);
        assert!((a - 75.0).abs() < 2.0, "{a}");
        assert!(!t.kept(0.5, 0.5) && t.kept(0.1, 0.1));
        assert_eq!(t.untrimmed(), s);
        assert!(t.wires(0.01).len() > s.wires(0.01).len() - 2, "the isolines break round the hole and the loop is drawn");
    }

    #[test]
    fn cv_net_shows_only_when_asked() {
        let mut s = square();
        let w = s.wires(0.1).len();
        s.cvs_shown = true;
        assert_eq!(s.wires(0.1).len(), w + 4);
        assert_eq!(s.cv_points().len(), 4);
    }
}
