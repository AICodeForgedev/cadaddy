//! Hidden-line removal for 2D drawings of the 3D model (FLATSHOT, section backgrounds, drawing
//! views): each edge is cut into short pieces and every piece is tested against the surfaces in
//! front of it (projected triangles found through a grid), then runs of the same visibility are
//! joined back into segments.

use cadaddy_doc::{Drawing, EntityKind, Mesh, Solid3d, Space};
use cadaddy_geom::{Vec2, Vec3};

/// A viewing direction: `dir` points from the model towards the viewer; `right` and `up` are the
/// drawing's X and Y.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub dir: Vec3,
    pub right: Vec3,
    pub up: Vec3,
}

impl View {
    /// The view looking along `-dir`, with world Z up where it can be (Y looking straight down).
    pub fn new(dir: Vec3) -> Option<View> {
        let d = dir.normalized();
        if d.len() < 0.5 {
            return None;
        }
        let up0 = if d.z.abs() > 0.999 { Vec3::new(0.0, 1.0, 0.0) } else { Vec3::new(0.0, 0.0, 1.0) };
        let right = up0.cross(d).normalized();
        let up = d.cross(right).normalized();
        Some(View { dir: d, right, up })
    }
    pub fn project(&self, p: Vec3) -> Vec2 {
        Vec2::new(p.dot(self.right), p.dot(self.up))
    }
    fn depth(&self, p: Vec3) -> f64 {
        p.dot(self.dir)
    }
}

/// What a drawing of the model is made from: surfaces that hide, and the edges drawn.
#[derive(Clone, Debug, Default)]
pub struct Scene3 {
    pub tris: Vec<[Vec3; 3]>,
    pub edges: Vec<[Vec3; 2]>,
}

/// Most triangles, edges and edge pieces a drawing takes (beyond them the rest is left out).
const MAX_TRIS: usize = 4_000_000;
const MAX_EDGES: usize = 2_000_000;
const MAX_PIECES: usize = 20_000_000;

impl Scene3 {
    /// A solid's surfaces and the edges a drawing shows for it from `view`: where flat faces
    /// meet, where a curved face meets another at an angle, and a curved face's outline.
    pub fn add_solid(&mut self, s: &Solid3d, tol: f64, view: &View) {
        let polys = s.polys(tol);
        for p in polys.iter() {
            for k in 1..p.pts.len().saturating_sub(1) {
                if self.tris.len() < MAX_TRIS {
                    self.tris.push([p.pts[0], p.pts[k], p.pts[k + 1]]);
                }
            }
        }
        for (seg, sharp) in cadaddy_doc::drawing_edges(&polys, view.dir) {
            if sharp && self.edges.len() < MAX_EDGES {
                self.edges.push(seg);
            }
        }
    }

    /// A mesh: its faces hide, its visible edges are drawn.
    pub fn add_mesh(&mut self, m: &Mesh) {
        for t in m.triangles() {
            if self.tris.len() < MAX_TRIS {
                self.tris.push(t);
            }
        }
        for (a, b) in m.edges() {
            if let (Some(a), Some(b)) = (m.vertices.get(a as usize), m.vertices.get(b as usize))
                && self.edges.len() < MAX_EDGES
            {
                self.edges.push([*a, *b]);
            }
        }
    }

    /// Everything 3D in a space: solids, meshes, 3D faces, and lines and polylines in space.
    pub fn of_space(d: &Drawing, space: &Space, view: &View, mut keep: impl FnMut(&cadaddy_doc::Entity) -> bool) -> Scene3 {
        let mut sc = Scene3::default();
        let Some(store) = d.space(space) else { return sc };
        for e in store.iter() {
            if !d.is_visible(e) || !keep(e) {
                continue;
            }
            match &e.kind {
                EntityKind::Solid3d(so) => sc.add_solid(so, so.mesh_tol(), view),
                EntityKind::Mesh(m) => sc.add_mesh(m),
                EntityKind::SectionPlane(_) => {}
                k => {
                    if let EntityKind::Face3d(f) = k {
                        let [a, b, c, dd] = f.corners;
                        sc.tris.push([a, b, c]);
                        if c != dd {
                            sc.tris.push([a, c, dd]);
                        }
                    }
                    if let Some((pts, closed)) = k.points3d() {
                        let n = pts.len();
                        for i in 0..if closed { n } else { n.saturating_sub(1) } {
                            if let (Some(a), Some(b)) = (pts.get(i), pts.get((i + 1) % n)) {
                                sc.edges.push([*a, *b]);
                            }
                        }
                    }
                }
            }
        }
        sc
    }
}

/// The edges as seen from `view`: (visible, hidden) segments in the view's 2D coordinates.
pub fn hidden_lines(sc: &Scene3, view: &View) -> (Vec<[Vec2; 2]>, Vec<[Vec2; 2]>) {
    let (mut lo, mut hi) = (Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN));
    let mut dmin = f64::MAX;
    let mut dmax = f64::MIN;
    for p in sc.tris.iter().flatten().chain(sc.edges.iter().flatten()) {
        let q = view.project(*p);
        lo = Vec2::new(lo.x.min(q.x), lo.y.min(q.y));
        hi = Vec2::new(hi.x.max(q.x), hi.y.max(q.y));
        let z = view.depth(*p);
        dmin = dmin.min(z);
        dmax = dmax.max(z);
    }
    if !(lo.x <= hi.x && lo.y <= hi.y) {
        return (Vec::new(), Vec::new());
    }
    let size = (hi.x - lo.x).max(hi.y - lo.y).max(1e-12);
    let eps = 1e-6 * size.max(dmax - dmin);
    // The projected triangles, in a grid.
    let tris: Vec<([Vec2; 3], [f64; 3])> = sc
        .tris
        .iter()
        .filter_map(|t| {
            let p = t.map(|v| view.project(v));
            let area = (p[1] - p[0]).cross(p[2] - p[0]);
            (area.abs() > 1e-18 * size * size).then(|| (p, t.map(|v| view.depth(v))))
        })
        .collect();
    let g = ((tris.len() as f64).sqrt() as usize * 2).clamp(8, 512);
    let cell = size / g as f64;
    let cell_of = |v: f64, o: f64| (((v - o) / cell).floor().max(0.0) as usize).min(g - 1);
    let mut grid: Vec<Vec<u32>> = vec![Vec::new(); g * g];
    let mut placed = 0usize;
    for (i, (p, _)) in tris.iter().enumerate() {
        let (x0, x1) = (p.iter().map(|q| q.x).fold(f64::MAX, f64::min), p.iter().map(|q| q.x).fold(f64::MIN, f64::max));
        let (y0, y1) = (p.iter().map(|q| q.y).fold(f64::MAX, f64::min), p.iter().map(|q| q.y).fold(f64::MIN, f64::max));
        for cy in cell_of(y0, lo.y)..=cell_of(y1, lo.y) {
            for cx in cell_of(x0, lo.x)..=cell_of(x1, lo.x) {
                if placed < MAX_PIECES {
                    grid[cy * g + cx].push(i as u32);
                    placed += 1;
                }
            }
        }
    }
    // Whether a point (2D, depth) is behind a triangle that covers it (strictly inside).
    let hidden_at = |m: Vec2, z: f64| -> bool {
        let Some(list) = grid.get(cell_of(m.y, lo.y) * g + cell_of(m.x, lo.x)) else { return false };
        list.iter().any(|&i| {
            let Some((p, d)) = tris.get(i as usize) else { return false };
            let area = (p[1] - p[0]).cross(p[2] - p[0]);
            let w0 = (p[2] - p[1]).cross(m - p[1]) / area;
            let w1 = (p[0] - p[2]).cross(m - p[2]) / area;
            let w2 = 1.0 - w0 - w1;
            let inset = 1e-9;
            if w0 <= inset || w1 <= inset || w2 <= inset {
                return false;
            }
            let dz = w0 * d[0] + w1 * d[1] + w2 * d[2];
            dz > z + eps
        })
    };
    let step = size / 1500.0;
    let mut vis: Vec<[Vec2; 2]> = Vec::new();
    let mut hid: Vec<[Vec2; 2]> = Vec::new();
    let mut budget = MAX_PIECES;
    for [a, b] in &sc.edges {
        let (pa, pb) = (view.project(*a), view.project(*b));
        let len = pa.dist(pb);
        if !len.is_finite() || len <= 1e-12 * size {
            continue;
        }
        let n = ((len / step).ceil() as usize).clamp(1, 4000).min(budget.max(1));
        budget = budget.saturating_sub(n);
        let mut run_start = 0usize;
        let mut run_hidden: Option<bool> = None;
        for k in 0..=n {
            let state = if k < n {
                let t = (k as f64 + 0.5) / n as f64;
                let p3 = *a + (*b - *a) * t;
                Some(hidden_at(view.project(p3), view.depth(p3)))
            } else {
                None
            };
            if state != run_hidden {
                if let Some(h) = run_hidden {
                    let seg = [pa + (pb - pa) * (run_start as f64 / n as f64), pa + (pb - pa) * (k as f64 / n as f64)];
                    if h { hid.push(seg) } else { vis.push(seg) }
                }
                run_start = k;
                run_hidden = state;
            }
        }
    }
    (dedupe(vis, eps), dedupe(hid, eps))
}

/// Segments without exact repeats (each solid edge comes from both its faces).
fn dedupe(segs: Vec<[Vec2; 2]>, eps: f64) -> Vec<[Vec2; 2]> {
    let q = |p: Vec2| ((p.x / (eps * 10.0)).round() as i64, (p.y / (eps * 10.0)).round() as i64);
    let mut seen = std::collections::HashSet::new();
    segs.into_iter()
        .filter(|[a, b]| {
            let (ka, kb) = (q(*a), q(*b));
            seen.insert(if ka <= kb { (ka, kb) } else { (kb, ka) })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::SolidShape;

    #[test]
    fn a_box_seen_from_the_front_shows_its_outline_and_hides_its_back_edges() {
        let b = Solid3d::new(SolidShape::Box { length: 4.0, width: 2.0, height: 3.0 });
        // Looking along +Y (from the front).
        let v = View::new(Vec3::new(0.0, -1.0, 0.0)).unwrap();
        let mut sc = Scene3::default();
        sc.add_solid(&b, 0.01, &v);
        let (vis, hid) = hidden_lines(&sc, &v);
        let total = |s: &[[Vec2; 2]]| s.iter().map(|[a, b]| a.dist(*b)).sum::<f64>();
        // The front face's outline (14); the side edges seen end on vanish.
        assert!((total(&vis) - 14.0).abs() < 1e-6, "{vis:?}");
        // The back face's edges coincide with the front's in this view: hidden behind them.
        assert!(total(&hid) < 14.0 + 1e-6);
        // From above at an angle: the far vertical edge is hidden.
        let iso = View::new(Vec3::new(1.0, -1.0, 1.0)).unwrap();
        let (vis, hid) = hidden_lines(&sc, &iso);
        assert_eq!(vis.len(), 9, "{vis:?}");
        assert_eq!(hid.len(), 3, "{hid:?}");
    }

    #[test]
    fn a_cylinder_shows_its_outline_not_its_isolines() {
        let c = Solid3d::new(SolidShape::Frustum { base_radius: 1.0, top_radius: 1.0, height: 3.0 });
        let v = View::new(Vec3::new(0.0, -1.0, 0.0)).unwrap();
        let mut sc = Scene3::default();
        sc.add_solid(&c, 0.001, &v);
        let (vis, _) = hidden_lines(&sc, &v);
        let total: f64 = vis.iter().map(|[a, b]| a.dist(*b)).sum();
        // Two sides of 3 and the ends seen edge on (2 each), within the tessellation.
        assert!((total - 10.0).abs() < 0.05, "{total}: {vis:?}");
        // Nothing in the middle (no isoline at x = 0).
        assert!(vis.iter().all(|[a, b]| (a.x.abs() > 0.9 && b.x.abs() > 0.9) || (a.y - b.y).abs() < 1e-9), "{vis:?}");
    }
}
