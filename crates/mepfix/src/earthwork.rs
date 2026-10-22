//! Surfaces and earthwork for the site takeoff: existing and proposed TINs from grading contours
//! and spot grades, cut and fill between them with each proposed object's share, trench
//! quantities and the utility structure lines. Shared by the engine's `site.takeoff` command and
//! the sitework fixture's ground truth (moved here from the engine so the engine can depend on
//! this crate without a cycle).

use std::collections::BTreeMap;

use serde::Serialize;

/// Most points in one surface (contour vertices after densifying, plus spot grades).
const MAX_TIN_POINTS: usize = 2_000;
/// Grid cells along the graded area's longer side for the volume.
const GRID_CELLS: usize = 240;
/// Trench rule: width = the pipe plus this much each side, feet.
pub const TRENCH_SIDE_FT: f64 = 1.0;

/// Which ground a contour or spot grade describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    Existing,
    Proposed,
}

/// A contour line: its elevation and vertices (feet).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Contour {
    pub id: String,
    pub sheet: String,
    pub surface: Surface,
    pub elevation_ft: f64,
    pub points_ft: Vec<[f64; 2]>,
}

/// A spot grade.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Spot {
    pub id: String,
    pub sheet: String,
    pub surface: Surface,
    pub elevation_ft: f64,
    pub at_ft: [f64; 2],
}

/// One object's share of a quantity.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Part {
    pub id: String,
    pub quantity: f64,
}

/// A triangulated surface.
struct Tin {
    pts: Vec<[f64; 3]>,
    owner: Vec<usize>,
    tris: Vec<[usize; 3]>,
    min: [f64; 2],
    max: [f64; 2],
    cell: [f64; 2],
    n: usize,
    buckets: Vec<Vec<usize>>,
}

fn circum(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> Option<([f64; 2], f64)> {
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-12 {
        return None;
    }
    let a2 = a[0] * a[0] + a[1] * a[1];
    let b2 = b[0] * b[0] + b[1] * b[1];
    let c2 = c[0] * c[0] + c[1] * c[1];
    let ux = (a2 * (b[1] - c[1]) + b2 * (c[1] - a[1]) + c2 * (a[1] - b[1])) / d;
    let uy = (a2 * (c[0] - b[0]) + b2 * (a[0] - c[0]) + c2 * (b[0] - a[0])) / d;
    let r2 = (a[0] - ux).powi(2) + (a[1] - uy).powi(2);
    Some(([ux, uy], r2))
}

/// Delaunay triangulation (Bowyer–Watson) of `pts`; triangles index `pts`.
fn delaunay(pts: &[[f64; 2]]) -> Vec<[usize; 3]> {
    let n = pts.len();
    if n < 3 {
        return Vec::new();
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in pts {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-9);
    let mid = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let mut all: Vec<[f64; 2]> = pts.to_vec();
    all.push([mid[0] - 50.0 * span, mid[1] - 40.0 * span]);
    all.push([mid[0] + 50.0 * span, mid[1] - 40.0 * span]);
    all.push([mid[0], mid[1] + 50.0 * span]);
    let get = |i: usize, all: &[[f64; 2]]| all.get(i).copied().unwrap_or([0.0, 0.0]);
    let mut tris: Vec<([usize; 3], [f64; 2], f64)> = Vec::new();
    if let Some((c, r2)) = circum(get(n, &all), get(n + 1, &all), get(n + 2, &all)) {
        tris.push(([n, n + 1, n + 2], c, r2));
    }
    for i in 0..n {
        let p = get(i, &all);
        let mut edges: Vec<[usize; 2]> = Vec::new();
        tris.retain(|(t, c, r2)| {
            let bad = (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) < *r2 * (1.0 - 1e-12);
            if bad {
                for e in [[t[0], t[1]], [t[1], t[2]], [t[2], t[0]]] {
                    edges.push(e);
                }
            }
            !bad
        });
        // The hole's boundary: the edges only one removed triangle had.
        let mut boundary: Vec<[usize; 2]> = Vec::new();
        for e in &edges {
            let shared = edges.iter().filter(|o| (o[0] == e[0] && o[1] == e[1]) || (o[0] == e[1] && o[1] == e[0])).count();
            if shared == 1 {
                boundary.push(*e);
            }
        }
        for e in boundary {
            if let Some((c, r2)) = circum(get(e[0], &all), get(e[1], &all), p) {
                tris.push(([e[0], e[1], i], c, r2));
            }
        }
    }
    tris.into_iter().map(|(t, _, _)| t).filter(|t| t.iter().all(|v| *v < n)).collect()
}

impl Tin {
    /// `pts`: x, y, z and the index of the object each point came from.
    fn new(pts: Vec<([f64; 3], usize)>) -> Option<Tin> {
        let xy: Vec<[f64; 2]> = pts.iter().map(|(p, _)| [p[0], p[1]]).collect();
        let tris = delaunay(&xy);
        if tris.is_empty() {
            return None;
        }
        let (mut min, mut max) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for p in &xy {
            min = [min[0].min(p[0]), min[1].min(p[1])];
            max = [max[0].max(p[0]), max[1].max(p[1])];
        }
        let n = ((tris.len() as f64).sqrt() as usize).clamp(1, 200);
        let cell = [((max[0] - min[0]) / n as f64).max(1e-9), ((max[1] - min[1]) / n as f64).max(1e-9)];
        let mut buckets = vec![Vec::new(); n * n];
        for (ti, t) in tris.iter().enumerate() {
            let vs: Vec<[f64; 2]> = t.iter().filter_map(|i| xy.get(*i).copied()).collect();
            let (lo, hi) = vs.iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), p| {
                ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])
            });
            let ix = |v: f64, o: f64, c: f64| (((v - o) / c).floor().max(0.0) as usize).min(n - 1);
            for bx in ix(lo[0], min[0], cell[0])..=ix(hi[0], min[0], cell[0]) {
                for by in ix(lo[1], min[1], cell[1])..=ix(hi[1], min[1], cell[1]) {
                    if let Some(b) = buckets.get_mut(by * n + bx) {
                        b.push(ti);
                    }
                }
            }
        }
        Some(Tin { owner: pts.iter().map(|(_, o)| *o).collect(), pts: pts.into_iter().map(|(p, _)| p).collect(), tris, min, max, cell, n, buckets })
    }

    /// The surface's elevation at `p` and the object of the nearest corner, if `p` is on it.
    fn at(&self, p: [f64; 2]) -> Option<(f64, usize)> {
        let tol = 1e-9 * (self.max[0] - self.min[0] + self.max[1] - self.min[1]);
        if p[0] < self.min[0] - tol || p[1] < self.min[1] - tol || p[0] > self.max[0] + tol || p[1] > self.max[1] + tol {
            return None;
        }
        let ix = |v: f64, o: f64, c: f64| (((v - o) / c).floor().max(0.0) as usize).min(self.n - 1);
        let b = self.buckets.get(ix(p[1], self.min[1], self.cell[1]) * self.n + ix(p[0], self.min[0], self.cell[0]))?;
        for ti in b {
            let t = self.tris.get(*ti)?;
            let (a, bb, c) = (self.pts.get(t[0])?, self.pts.get(t[1])?, self.pts.get(t[2])?);
            let det = (bb[1] - c[1]) * (a[0] - c[0]) + (c[0] - bb[0]) * (a[1] - c[1]);
            if det.abs() < 1e-18 {
                continue;
            }
            let l1 = ((bb[1] - c[1]) * (p[0] - c[0]) + (c[0] - bb[0]) * (p[1] - c[1])) / det;
            let l2 = ((c[1] - a[1]) * (p[0] - c[0]) + (a[0] - c[0]) * (p[1] - c[1])) / det;
            let l3 = 1.0 - l1 - l2;
            let e = -1e-9;
            if l1 >= e && l2 >= e && l3 >= e {
                let z = l1 * a[2] + l2 * bb[2] + l3 * c[2];
                let corner = if l1 >= l2 && l1 >= l3 {
                    t[0]
                } else if l2 >= l3 {
                    t[1]
                } else {
                    t[2]
                };
                return Some((z, self.owner.get(corner).copied().unwrap_or(0)));
            }
        }
        None
    }
}

/// ∫ max(f, 0) over a triangle where `f` is linear with values `f` at its corners.
pub fn positive_part(p: [[f64; 2]; 3], f: [f64; 3]) -> f64 {
    let mut poly: Vec<([f64; 2], f64)> = Vec::with_capacity(4);
    for i in 0..3 {
        let (a, fa) = (p[i], f[i]);
        let (b, fb) = (p[(i + 1) % 3], f[(i + 1) % 3]);
        if fa >= 0.0 {
            poly.push((a, fa));
        }
        if (fa >= 0.0) != (fb >= 0.0) {
            let t = fa / (fa - fb);
            poly.push(([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t], 0.0));
        }
    }
    let Some(&(o, fo)) = poly.first() else { return 0.0 };
    let mut s = 0.0;
    for w in poly.windows(2).skip(1) {
        if let [(a, fa), (b, fb)] = w {
            let area = ((a[0] - o[0]) * (b[1] - o[1]) - (b[0] - o[0]) * (a[1] - o[1])).abs() / 2.0;
            s += area * (fo + fa + fb) / 3.0;
        }
    }
    s
}

/// Cut and fill between two surfaces.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Earthwork {
    pub cut_cy: f64,
    pub fill_cy: f64,
    /// Where both surfaces are known, square feet.
    pub graded_area_sf: f64,
    /// Each proposed contour's or spot's share (the nearest TIN corner's object).
    pub cut_parts: Vec<Part>,
    pub fill_parts: Vec<Part>,
}

fn surface_points(contours: &[&Contour], spots: &[&Spot]) -> Vec<([f64; 3], usize)> {
    let total: f64 = contours.iter().map(|c| path_len(&c.points_ft)).sum();
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in contours.iter().flat_map(|c| c.points_ft.iter()).chain(spots.iter().map(|s| &s.at_ft)) {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2)).sqrt();
    let budget = MAX_TIN_POINTS.saturating_sub(spots.len()).max(3) as f64;
    let step = (diag / 80.0).max(total / budget * 1.5).max(1e-6);
    let mut out: Vec<([f64; 3], usize)> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut push = |p: [f64; 3], o: usize, out: &mut Vec<([f64; 3], usize)>| {
        let key = ((p[0] / step * 1e3).round() as i64, (p[1] / step * 1e3).round() as i64);
        if out.len() < MAX_TIN_POINTS && seen.insert(key) {
            out.push((p, o));
        }
    };
    for (ci, c) in contours.iter().enumerate() {
        for w in c.points_ft.windows(2) {
            let [a, b] = w else { continue };
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            let n = ((len / step).ceil() as usize).clamp(1, MAX_TIN_POINTS);
            for i in 0..=n {
                let t = i as f64 / n as f64;
                push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, c.elevation_ft], ci, &mut out);
            }
        }
        if c.points_ft.len() == 1
            && let Some(a) = c.points_ft.first()
        {
            push([a[0], a[1], c.elevation_ft], ci, &mut out);
        }
    }
    for (si, s) in spots.iter().enumerate() {
        push([s.at_ft[0], s.at_ft[1], s.elevation_ft], contours.len() + si, &mut out);
    }
    out
}

/// Cut and fill between the existing and the proposed surface, each made from its contours and
/// spot grades, over the area where both are known. `Err` names what is missing.
pub fn earthwork(contours: &[Contour], spots: &[Spot]) -> std::result::Result<Earthwork, String> {
    let pick = |s: Surface| -> (Vec<&Contour>, Vec<&Spot>) {
        (
            contours.iter().filter(|c| c.surface == s && c.elevation_ft.is_finite()).collect(),
            spots.iter().filter(|p| p.surface == s && p.elevation_ft.is_finite()).collect(),
        )
    };
    let (ec, es) = pick(Surface::Existing);
    let (pc, ps) = pick(Surface::Proposed);
    let ex = Tin::new(surface_points(&ec, &es));
    let pr = Tin::new(surface_points(&pc, &ps));
    let (ex, pr) = match (ex, pr) {
        (Some(e), Some(p)) => (e, p),
        (None, None) => return Err("no existing and no proposed surface (contours or spot grades) on the sheets".into()),
        (None, _) => return Err("no existing surface (existing contours or spot grades) on the sheets".into()),
        (_, None) => return Err("no proposed surface (proposed contours or spot grades) on the sheets".into()),
    };
    let ids: Vec<String> = pc.iter().map(|c| c.id.clone()).chain(ps.iter().map(|s| s.id.clone())).collect();
    let lo = [ex.min[0].max(pr.min[0]), ex.min[1].max(pr.min[1])];
    let hi = [ex.max[0].min(pr.max[0]), ex.max[1].min(pr.max[1])];
    if hi[0] <= lo[0] || hi[1] <= lo[1] {
        return Err("the existing and proposed surfaces don't overlap".into());
    }
    let h = (hi[0] - lo[0]).max(hi[1] - lo[1]) / GRID_CELLS as f64;
    let nx = ((hi[0] - lo[0]) / h).ceil().max(1.0) as usize;
    let ny = ((hi[1] - lo[1]) / h).ceil().max(1.0) as usize;
    let (hx, hy) = ((hi[0] - lo[0]) / nx as f64, (hi[1] - lo[1]) / ny as f64);
    let node = |i: usize, j: usize| [lo[0] + i as f64 * hx, lo[1] + j as f64 * hy];
    // Existing minus proposed at each node: positive is cut.
    let mut dz: Vec<Option<(f64, usize)>> = Vec::with_capacity((nx + 1) * (ny + 1));
    for j in 0..=ny {
        for i in 0..=nx {
            let p = node(i, j);
            dz.push(match (ex.at(p), pr.at(p)) {
                (Some((e, _)), Some((q, o))) => Some((e - q, o)),
                _ => None,
            });
        }
    }
    let at = |i: usize, j: usize| dz.get(j * (nx + 1) + i).copied().flatten();
    let (mut cut, mut fill, mut area) = (0.0, 0.0, 0.0);
    let mut cut_by: BTreeMap<usize, f64> = BTreeMap::new();
    let mut fill_by: BTreeMap<usize, f64> = BTreeMap::new();
    for j in 0..ny {
        for i in 0..nx {
            let (Some(a), Some(b), Some(c), Some(d)) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)) else { continue };
            let (pa, pb, pc, pd) = (node(i, j), node(i + 1, j), node(i + 1, j + 1), node(i, j + 1));
            let mut cc = 0.0;
            let mut ff = 0.0;
            for (p, f) in [([pa, pb, pc], [a.0, b.0, c.0]), ([pa, pc, pd], [a.0, c.0, d.0])] {
                cc += positive_part(p, f);
                ff += positive_part(p, [-f[0], -f[1], -f[2]]);
            }
            area += hx * hy;
            cut += cc;
            fill += ff;
            // The cell's share goes to the proposed object nearest its centre.
            let owner = pr.at([(pa[0] + pc[0]) / 2.0, (pa[1] + pc[1]) / 2.0]).map_or(a.1, |(_, o)| o);
            *cut_by.entry(owner).or_default() += cc;
            *fill_by.entry(owner).or_default() += ff;
        }
    }
    if area <= 0.0 {
        return Err("the existing and proposed surfaces don't overlap".into());
    }
    let parts = |m: BTreeMap<usize, f64>| -> Vec<Part> {
        m.into_iter()
            .filter(|(_, v)| *v > 0.0)
            .map(|(o, v)| Part { id: ids.get(o).cloned().unwrap_or_default(), quantity: round2(v / 27.0) })
            .collect()
    };
    let cut_parts = parts(cut_by);
    let fill_parts = parts(fill_by);
    // A sliver of rounding noise is no cut or fill (and never -0).
    let cy = |v: f64| if v > 0.0 { v } else { 0.0 };
    Ok(Earthwork {
        cut_cy: cy(if cut_parts.is_empty() { round2(cut / 27.0) } else { sum(&cut_parts) }),
        fill_cy: cy(if fill_parts.is_empty() { round2(fill / 27.0) } else { sum(&fill_parts) }),
        graded_area_sf: round2(area),
        cut_parts,
        fill_parts,
    })
}

pub fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

pub fn sum(parts: &[Part]) -> f64 {
    // `+ 0.0`: an empty sum is -0.0.
    round2(parts.iter().map(|p| p.quantity).sum::<f64>() + 0.0)
}

/// Trench excavation and backfill for a run, cubic yards: the trench is the pipe plus
/// [`TRENCH_SIDE_FT`] each side wide and `depth_ft` deep; backfill is the trench less the pipe.
pub fn trench_cy(length_ft: f64, size_in: f64, depth_ft: f64) -> (f64, f64) {
    let od_ft = size_in.max(0.0) / 12.0;
    let width = od_ft + 2.0 * TRENCH_SIDE_FT;
    let exc = length_ft * width * depth_ft / 27.0;
    let pipe = length_ft * std::f64::consts::PI * (od_ft / 2.0).powi(2) / 27.0;
    (exc, (exc - pipe).max(0.0))
}

fn path_len(pts: &[[f64; 2]]) -> f64 {
    pts.windows(2).map(|w| if let [a, b] = w { ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt() } else { 0.0 }).sum()
}

/// A structure's takeoff line: CSI and description.
pub fn structure_line(kind: &str, system: &str) -> Option<(&'static str, String)> {
    Some(match kind {
        "manhole" => match system {
            "storm" => ("33 41 13", "STORM MANHOLE".into()),
            "sanitary" => ("33 31 13", "SANITARY MANHOLE".into()),
            _ => ("33 05 13", "MANHOLE, SYSTEM NOT SHOWN".into()),
        },
        "catch_basin" => ("33 41 13", "CATCH BASIN".into()),
        "cleanout" => ("33 31 13", "CLEANOUT".into()),
        "valve_box" => ("33 11 13", "VALVE BOX".into()),
        "junction_box" => ("33 71 00", "ELECTRIC JUNCTION BOX".into()),
        "meter" => ("33 11 13", "WATER METER".into()),
        "backflow_preventer" => ("33 11 13", "BACKFLOW PREVENTER".into()),
        _ => return None,
    })
}
