//! Section planes (SECTIONPLANE): a plane through the model that can cut it live in 3D views and
//! from which 2D sections are made. The plane's `normal` is the direction looked in: what lies
//! on its +normal side is kept and seen, the rest is cut away.

use cadaddy_geom::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

use crate::solid::bsp::Poly;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SectionPlane {
    /// The middle of the plane's outline.
    pub origin: Vec3,
    /// Unit; the side kept and looked at.
    pub normal: Vec3,
    /// The outline's size along the plane's horizontal and vertical directions.
    pub width: f64,
    pub height: f64,
    /// Cuts the model in 3D views.
    #[serde(default)]
    pub live: bool,
    #[serde(default)]
    pub name: String,
}

impl SectionPlane {
    /// The plane's horizontal and vertical directions (horizontal square to world Z where it
    /// can be), right-handed with the normal: h × v = −normal (looking along the normal, h is
    /// to the right and v up).
    pub fn axes(&self) -> (Vec3, Vec3) {
        let n = self.normal.normalized();
        let up = if n.z.abs() > 0.999 { Vec3::new(0.0, 1.0, 0.0) } else { Vec3::new(0.0, 0.0, 1.0) };
        let h = n.cross(up).normalized();
        let v = h.cross(n).normalized();
        (h, v)
    }

    pub fn is_valid(&self) -> bool {
        self.origin.is_finite()
            && self.normal.is_finite()
            && self.normal.len() > 0.5
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width >= 0.0
            && self.height >= 0.0
    }

    /// The outline's corners.
    pub fn corners(&self) -> [Vec3; 4] {
        let (h, v) = self.axes();
        let (hw, hh) = (h * (self.width / 2.0), v * (self.height / 2.0));
        let o = self.origin;
        [o - hw - hh, o + hw - hh, o + hw + hh, o - hw + hh]
    }

    /// The point in the plane's own 2D coordinates (h, v) about its origin.
    pub fn to_plane(&self, p: Vec3) -> Vec2 {
        let (h, v) = self.axes();
        Vec2::new((p - self.origin).dot(h), (p - self.origin).dot(v))
    }

    /// Signed distance along the normal (positive: the kept side).
    pub fn side(&self, p: Vec3) -> f64 {
        (p - self.origin).dot(self.normal.normalized())
    }

    pub fn transform(&mut self, m: &cadaddy_geom::Mat4) {
        let o = m.apply(self.origin);
        let n = m.apply(self.origin + self.normal) - o;
        let s = (m.apply(self.origin + self.axes().0) - o).len();
        self.origin = o;
        if n.len() > 0.0 {
            self.normal = n.normalized();
        }
        if s.is_finite() && s > 0.0 {
            self.width *= s;
            self.height *= s;
        }
    }
}

/// Where a plane cuts closed polygons: segments, chained into loops (in drawing coordinates).
pub fn section_loops(polys: &[Poly], origin: Vec3, normal: Vec3) -> Vec<Vec<Vec3>> {
    let n = normal.normalized();
    let scale = polys.iter().flat_map(|p| p.pts.iter()).fold(1.0f64, |m, p| m.max(p.x.abs()).max(p.y.abs()).max(p.z.abs()));
    let eps = 1e-9 * scale;
    let mut segs: Vec<[Vec3; 2]> = Vec::new();
    for p in polys {
        // A face lying in the plane is the section's face itself, not a cut through it.
        if p.plane.n.cross(n).len() < 1e-9 && ((p.pts[0] - origin).dot(n)).abs() <= eps {
            continue;
        }
        // A point on the plane counts as on the kept side, so a cut through a vertex or along an
        // edge is made once, consistently, by the faces that really cross.
        let d: Vec<f64> = p.pts.iter().map(|q| (*q - origin).dot(n)).map(|x| if x.abs() <= eps { 0.0 } else { x }).collect();
        let m = p.pts.len();
        let mut hits = Vec::new();
        for i in 0..m {
            let j = (i + 1) % m;
            let (a, b) = (d[i], d[j]);
            if (a >= 0.0) != (b >= 0.0) {
                hits.push(p.pts[i] + (p.pts[j] - p.pts[i]) * (a / (a - b)));
            }
        }
        if hits.len() >= 2 {
            // A convex polygon meets a plane in one segment: its two furthest points.
            let (mut best, mut pair) = (-1.0, [hits[0], hits[1]]);
            for x in 0..hits.len() {
                for y in x + 1..hits.len() {
                    let l = (hits[x] - hits[y]).len();
                    if l > best {
                        best = l;
                        pair = [hits[x], hits[y]];
                    }
                }
            }
            if best > eps {
                segs.push(pair);
            }
        }
    }
    chain(segs, eps * 1e3).into_iter().map(|l| simplify(l, eps * 1e3)).collect()
}

/// A polyline without points lying on the straight line between their neighbours.
fn simplify(mut l: Vec<Vec3>, eps: f64) -> Vec<Vec3> {
    let closed = l.len() > 3 && l.first().zip(l.last()).is_some_and(|(a, b)| (*a - *b).len() <= eps);
    if closed {
        l.pop();
    }
    let mut changed = true;
    while changed && l.len() > 2 {
        changed = false;
        let n = l.len();
        let ends = if closed { 0..n } else { 1..n - 1 };
        for i in ends {
            let (a, b, c) = (l[(i + n - 1) % n], l[i], l[(i + 1) % n]);
            let d = c - a;
            let off = if d.len() > 0.0 { (b - a).cross(d).len() / d.len() } else { (b - a).len() };
            if off <= eps && (b - a).dot(d) >= 0.0 && (c - b).dot(d) >= 0.0 {
                l.remove(i);
                changed = true;
                break;
            }
        }
    }
    if closed && let Some(f) = l.first().copied() {
        l.push(f);
    }
    l
}

/// Segments joined end to end into polylines (closed ones repeat their first point).
pub fn chain(segs: Vec<[Vec3; 2]>, eps: f64) -> Vec<Vec<Vec3>> {
    use std::collections::HashMap;
    let q = |p: Vec3| [(p.x / eps).round() as i64, (p.y / eps).round() as i64, (p.z / eps).round() as i64];
    let mut at: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (i, s) in segs.iter().enumerate() {
        at.entry(q(s[0])).or_default().push(i);
        at.entry(q(s[1])).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut out = Vec::new();
    for start in 0..segs.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut line = vec![segs[start][0], segs[start][1]];
        // Grow at the end, then at the front.
        for front in [false, true] {
            loop {
                let end = if front { line[0] } else { line[line.len() - 1] };
                let next = at.get(&q(end)).and_then(|c| c.iter().copied().find(|k| !used[*k]));
                let Some(k) = next else { break };
                used[k] = true;
                let s = segs[k];
                let other = if q(s[0]) == q(end) { s[1] } else { s[0] };
                if front {
                    line.insert(0, other);
                } else {
                    line.push(other);
                }
            }
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BoolOp, Solid3d, SolidShape, combine, placement};

    #[test]
    fn a_box_with_a_hole_cuts_into_two_rectangles() {
        let mut hole = Solid3d::new(SolidShape::Frustum { base_radius: 2.0, top_radius: 2.0, height: 12.0 });
        hole.transform(&placement(Vec3::new(5.0, 5.0, -1.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)));
        let s = combine(Solid3d::new(SolidShape::Box { length: 10.0, width: 10.0, height: 10.0 }), hole, BoolOp::Subtract);
        let loops = section_loops(&s.polys(0.01), Vec3::new(5.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(loops.len(), 2, "{loops:?}");
        for l in &loops {
            assert!((*l.first().unwrap() - *l.last().unwrap()).len() < 1e-9, "closed");
            let ys: Vec<f64> = l.iter().map(|p| p.y).collect();
            let (lo, hi) = (ys.iter().copied().fold(f64::MAX, f64::min), ys.iter().copied().fold(f64::MIN, f64::max));
            assert!(
                ((lo, hi) == (0.0, 3.0))
                    || ((hi - 10.0).abs() < 1e-9 && (lo - 7.0).abs() < 1e-9)
                    || ((lo - 0.0).abs() < 1e-9 && (hi - 3.0).abs() < 1e-9),
                "{lo} {hi}"
            );
            assert!(l.iter().all(|p| (p.x - 5.0).abs() < 1e-9));
            assert_eq!(l.len(), 5, "a rectangle: {l:?}");
        }
    }

    #[test]
    fn axes_look_along_the_normal() {
        let sp = SectionPlane { origin: Vec3::ZERO, normal: Vec3::new(0.0, 1.0, 0.0), width: 4.0, height: 2.0, live: true, name: String::new() };
        let (h, v) = sp.axes();
        // Looking along +Y: +X is to the right, Z up.
        assert!((h - Vec3::new(1.0, 0.0, 0.0)).len() < 1e-12 && (v - Vec3::new(0.0, 0.0, 1.0)).len() < 1e-12, "{h:?} {v:?}");
        assert!((h.cross(v) + sp.normal).len() < 1e-12);
    }
}
