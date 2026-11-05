//! A print crop's registration checked against the model's walls (Q6141).
//!
//! A packet's plan crops are carried onto their sheets by the sheet's registration on its level's
//! plan (or the view placement the job kept). A registration that is off puts the crop, and the
//! model drawn over it, beside what the sheet shows there. So each placed crop is checked: the
//! model's wall faces round the area, carried onto the sheet as the crop is, are fitted to the
//! sheet's own straight lines there (a similarity: shift, turn and scale), and
//!
//! - `residual_ft`: how far the placement is off, the root-mean-square distance (feet) the fit
//!   moves the model's walls by; `rotation_deg`, `scale` and `shift_ft` the fit's turn, scale and
//!   shift at the crop's centre; `fit_ft` how close the walls lie to the sheet's lines after it,
//!   `coverage` the share of the walls' length that does (`coverage_before`: as placed);
//! - `ok`: the residual is within the threshold (1 ft, or the project rule
//!   `validate.registration.residual_ft`, domain `tolerance`);
//! - `refitted`: over it, and the fit is good (at least half the walls' length on the sheet's
//!   lines, both ways): the crop is placed again from the wall lines;
//! - `misregistered`: the sheet's lines don't fit the walls well enough to tell (the numbers of
//!   the best fit are given, for the judge to measure from the walls, not the overlay);
//! - `not_checked`: no model walls or no straight lines in the crop, or the walls run one way
//!   only (a fit can't tell the shift along them).
//!
//! The fit: the turn from the two sets' line directions (length-weighted, modulo 90°); the scale
//! and the shift by voting, each wall face against each parallel sheet line; the best few
//! candidates (and the placement as it is) refined by point-to-line least squares on samples
//! along the walls; the one that puts the most wall length on the sheet's lines kept (the
//! smaller move among equals).

use cadaddy_doc::projrules::{ProjectRules, RuleDomain};
use serde_json::{Value, json};

/// The project rule of the threshold (domain `tolerance`, feet).
pub const RESIDUAL_KEY: &str = "validate.registration.residual_ft";
const KEY_PREFIX: &str = "validate.registration.";
pub const DEFAULT_RESIDUAL_FT: f64 = 1.0;
const RESIDUAL_RANGE: (f64, f64) = (0.05, 50.0);

/// How far off a placement may be and still be found, feet (the crop's search reach).
pub(crate) const REACH_FT: f64 = 10.0;
/// The shortest line looked at, feet.
const MIN_LINE_FT: f64 = 1.0;
/// Samples along the walls: spacing (feet) and how many at most.
const SAMPLE_FT: f64 = 0.5;
const MAX_SAMPLES: usize = 6_000;
pub(crate) const MAX_MODEL_LINES: usize = 4_000;
pub(crate) const MAX_PRINT_LINES: usize = 30_000;
/// The largest turn and the scales a fit looks for.
const MAX_TURN_DEG: f64 = 8.0;
const SCALE_RANGE: (f64, f64) = (0.85, 1.20);
const SCALE_STEP: f64 = 0.0025;
/// Direction histogram bins (modulo 90°), degrees.
const ANGLE_BIN_DEG: f64 = 0.1;
/// A line runs one of the two main ways within this, degrees.
const FAMILY_DEG: f64 = 2.0;
/// Shift histogram bins, feet.
const SHIFT_BIN_FT: f64 = 0.1;
/// A wall sample lies on a sheet line within this (feet), parallel within this (degrees).
const ON_LINE_FT: f64 = 0.3;
const PARALLEL_DEG: f64 = 3.0;
/// Past a line's ends a sample still lies on it, feet.
const END_SLACK_FT: f64 = 0.25;
/// The refinement's matching radii, feet, one per round.
const ROUNDS_FT: [f64; 7] = [1.0, 0.7, 0.5, 0.4, 0.3, 0.3, 0.3];
/// A good fit: this share of the walls' length on the sheet's lines, and this much (feet) each way.
const MIN_COVERAGE: f64 = 0.5;
const MIN_FAMILY_FT: f64 = 3.0;
/// Candidates within this coverage of the best count as equal (the smaller move wins).
const SAME_COVERAGE: f64 = 0.02;
/// Nearest-line grid: cell size (feet) and how many cells and entries at most.
const CELL_FT: f64 = 2.0;
const MAX_CELLS: usize = 400_000;
const MAX_CELL_ENTRIES: usize = 3_000_000;

/// A straight piece: two points.
pub(crate) type Seg = ([f64; 2], [f64; 2]);

/// A similarity of the plane: `p ↦ (a + ib)·p + (tx + i·ty)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Sim {
    pub a: f64,
    pub b: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Sim {
    pub const IDENTITY: Sim = Sim { a: 1.0, b: 0.0, tx: 0.0, ty: 0.0 };

    pub fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        [self.a * p[0] - self.b * p[1] + self.tx, self.b * p[0] + self.a * p[1] + self.ty]
    }
    fn turn(&self, d: [f64; 2]) -> [f64; 2] {
        [self.a * d[0] - self.b * d[1], self.b * d[0] + self.a * d[1]]
    }
    pub fn scale(&self) -> f64 {
        self.a.hypot(self.b)
    }
    pub fn rotation_deg(&self) -> f64 {
        self.b.atan2(self.a).to_degrees()
    }
    /// The inverse (none when degenerate).
    pub fn inverse(&self) -> Option<Sim> {
        let det = self.a * self.a + self.b * self.b;
        if !det.is_finite() || det < 1e-18 {
            return None;
        }
        let (a, b) = (self.a / det, -self.b / det);
        let t = Sim { a, b, tx: 0.0, ty: 0.0 }.apply([self.tx, self.ty]);
        Some(Sim { a, b, tx: -t[0], ty: -t[1] })
    }
    /// `self` after `first`: `p ↦ self(first(p))`.
    pub fn after(&self, first: &Sim) -> Sim {
        let t = self.apply([first.tx, first.ty]);
        Sim { a: self.a * first.a - self.b * first.b, b: self.a * first.b + self.b * first.a, tx: t[0], ty: t[1] }
    }
    fn finite(&self) -> bool {
        [self.a, self.b, self.tx, self.ty].iter().all(|v| v.is_finite())
    }
}

/// A line in the fit's frame (feet round the crop's centre).
#[derive(Clone, Copy, Debug)]
struct Line {
    a: [f64; 2],
    /// Unit direction.
    u: [f64; 2],
    len: f64,
    /// Direction modulo 180°, radians in [0, π).
    ang: f64,
}

impl Line {
    fn of(a: [f64; 2], b: [f64; 2]) -> Option<Line> {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dy);
        if !(len.is_finite() && len >= MIN_LINE_FT && a.iter().chain(b.iter()).all(|v| v.is_finite())) {
            return None;
        }
        let ang = dy.atan2(dx).rem_euclid(std::f64::consts::PI);
        Some(Line { a, u: [dx / len, dy / len], len, ang })
    }
    fn mid(&self) -> [f64; 2] {
        [self.a[0] + self.u[0] * self.len / 2.0, self.a[1] + self.u[1] * self.len / 2.0]
    }
}

/// A sample along a model wall: point, direction, the length it stands for.
#[derive(Clone, Copy, Debug)]
struct Sample {
    p: [f64; 2],
    u: [f64; 2],
    w: f64,
}

/// The sheet's lines in cells, for the nearest parallel line to a point.
struct Grid {
    lines: Vec<Line>,
    origin: [f64; 2],
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(lines: Vec<Line>, reach: f64) -> Grid {
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for l in &lines {
            let b = [l.a[0] + l.u[0] * l.len, l.a[1] + l.u[1] * l.len];
            for k in 0..2 {
                lo[k] = lo[k].min(l.a[k].min(b[k]) - reach);
                hi[k] = hi[k].max(l.a[k].max(b[k]) + reach);
            }
        }
        if !(lo[0].is_finite() && lo[1].is_finite() && hi[0].is_finite() && hi[1].is_finite()) {
            return Grid { lines, origin: [0.0, 0.0], cell: 1.0, nx: 0, ny: 0, cells: Vec::new() };
        }
        let (w, h) = ((hi[0] - lo[0]).max(1e-6), (hi[1] - lo[1]).max(1e-6));
        let mut cell = CELL_FT;
        while (w / cell).ceil() * (h / cell).ceil() > MAX_CELLS as f64 {
            cell *= 2.0;
        }
        let nx = ((w / cell).ceil() as usize).max(1);
        let ny = ((h / cell).ceil() as usize).max(1);
        let mut cells: Vec<Vec<u32>> = vec![Vec::new(); nx * ny];
        let mut entries = 0usize;
        'lines: for (i, l) in lines.iter().enumerate() {
            let b = [l.a[0] + l.u[0] * l.len, l.a[1] + l.u[1] * l.len];
            let c0 = (((l.a[0].min(b[0]) - reach - lo[0]) / cell).floor().max(0.0) as usize).min(nx - 1);
            let c1 = (((l.a[0].max(b[0]) + reach - lo[0]) / cell).floor().max(0.0) as usize).min(nx - 1);
            let r0 = (((l.a[1].min(b[1]) - reach - lo[1]) / cell).floor().max(0.0) as usize).min(ny - 1);
            let r1 = (((l.a[1].max(b[1]) + reach - lo[1]) / cell).floor().max(0.0) as usize).min(ny - 1);
            for r in r0..=r1 {
                for c in c0..=c1 {
                    // Only the cells the line passes near (a long slanted line's box is mostly empty).
                    let centre = [lo[0] + (c as f64 + 0.5) * cell, lo[1] + (r as f64 + 0.5) * cell];
                    if seg_dist(centre, l) > reach + cell * std::f64::consts::FRAC_1_SQRT_2 {
                        continue;
                    }
                    if entries >= MAX_CELL_ENTRIES {
                        break 'lines;
                    }
                    if let Some(v) = cells.get_mut(r * nx + c) {
                        v.push(i as u32);
                        entries += 1;
                    }
                }
            }
        }
        Grid { lines, origin: lo, cell, nx, ny, cells }
    }

    /// The nearest line parallel to `u` within `radius` of `p`: (distance, unit normal, a point
    /// on the line).
    fn nearest(&self, p: [f64; 2], u: [f64; 2], radius: f64) -> Option<(f64, [f64; 2], [f64; 2])> {
        if self.nx == 0 {
            return None;
        }
        let c = ((p[0] - self.origin[0]) / self.cell).floor();
        let r = ((p[1] - self.origin[1]) / self.cell).floor();
        if !(c >= 0.0 && r >= 0.0 && (c as usize) < self.nx && (r as usize) < self.ny) {
            return None;
        }
        let sin_tol = PARALLEL_DEG.to_radians().sin();
        let mut best: Option<(f64, [f64; 2], [f64; 2])> = None;
        for &i in self.cells.get(r as usize * self.nx + c as usize)? {
            let Some(l) = self.lines.get(i as usize) else { continue };
            if (l.u[0] * u[1] - l.u[1] * u[0]).abs() > sin_tol {
                continue;
            }
            let (qx, qy) = (p[0] - l.a[0], p[1] - l.a[1]);
            let along = qx * l.u[0] + qy * l.u[1];
            if along < -END_SLACK_FT || along > l.len + END_SLACK_FT {
                continue;
            }
            let n = [-l.u[1], l.u[0]];
            let dist = (qx * n[0] + qy * n[1]).abs();
            if dist <= radius && best.is_none_or(|(bd, ..)| dist < bd) {
                best = Some((dist, n, l.a));
            }
        }
        best
    }
}

fn seg_dist(p: [f64; 2], l: &Line) -> f64 {
    let (qx, qy) = (p[0] - l.a[0], p[1] - l.a[1]);
    let t = (qx * l.u[0] + qy * l.u[1]).clamp(0.0, l.len);
    (qx - l.u[0] * t).hypot(qy - l.u[1] * t)
}

/// A fit of the sheet's lines to the model's walls.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fit {
    /// The correction on the sheet: where a point the placement puts at `q` is on the sheet.
    pub correction: Sim,
    /// How far the placement is off: the RMS move of the wall samples, feet.
    pub residual_ft: f64,
    pub rotation_deg: f64,
    pub scale: f64,
    /// The move at the crop's centre, feet.
    pub shift_ft: [f64; 2],
    /// RMS distance of the walls on the sheet's lines after the fit, feet.
    pub fit_ft: f64,
    pub coverage: f64,
    pub coverage_before: f64,
    /// Good enough to place the crop by.
    pub good: bool,
    /// Why it isn't good.
    pub weak: Option<String>,
    /// Wall length looked at (feet) and sheet lines looked at.
    pub walls_ft: f64,
    pub lines: usize,
}

/// One candidate's quality: coverage, RMS on the lines, wall length matched each way.
#[derive(Clone, Copy, Debug)]
struct Quality {
    coverage: f64,
    fit_ft: f64,
    family: [f64; 2],
}

fn evaluate(samples: &[Sample], grid: &Grid, sim: &Sim, axis: f64) -> Quality {
    let (mut on, mut all, mut sq) = (0.0, 0.0, 0.0);
    let mut family = [0.0, 0.0];
    let (c, s) = (axis.cos(), axis.sin());
    for sm in samples {
        all += sm.w;
        let p = sim.apply(sm.p);
        let u = unit(sim.turn(sm.u));
        if let Some((d, ..)) = grid.nearest(p, u, ON_LINE_FT) {
            on += sm.w;
            sq += sm.w * d * d;
            // Which way the wall runs: along the main axis or across it.
            let along = (sm.u[0] * c + sm.u[1] * s).abs();
            if along >= FAMILY_DEG.to_radians().cos() {
                family[0] += sm.w;
            } else if along <= FAMILY_DEG.to_radians().sin() {
                family[1] += sm.w;
            }
        }
    }
    Quality { coverage: if all > 0.0 { on / all } else { 0.0 }, fit_ft: if on > 0.0 { (sq / on).sqrt() } else { f64::INFINITY }, family }
}

fn unit(d: [f64; 2]) -> [f64; 2] {
    let l = d[0].hypot(d[1]);
    if l > 1e-12 { [d[0] / l, d[1] / l] } else { [1.0, 0.0] }
}

/// The RMS move of the samples under `sim`, feet.
fn displacement(samples: &[Sample], sim: &Sim) -> f64 {
    let (mut sq, mut all) = (0.0, 0.0);
    for sm in samples {
        let q = sim.apply(sm.p);
        sq += sm.w * ((q[0] - sm.p[0]).powi(2) + (q[1] - sm.p[1]).powi(2));
        all += sm.w;
    }
    if all > 0.0 { (sq / all).sqrt() } else { 0.0 }
}

/// Point-to-line least squares from `start`, the matches taken again each round.
fn refine(samples: &[Sample], grid: &Grid, start: Sim) -> Sim {
    let mut sim = start;
    for radius in ROUNDS_FT {
        let mut ata = [[0.0f64; 4]; 4];
        let mut atb = [0.0f64; 4];
        let mut n_matched = 0usize;
        for sm in samples {
            let p = sim.apply(sm.p);
            let u = unit(sim.turn(sm.u));
            let Some((_, n, on)) = grid.nearest(p, u, radius) else { continue };
            let x = sm.p;
            let j = [n[0] * x[0] + n[1] * x[1], -n[0] * x[1] + n[1] * x[0], n[0], n[1]];
            let target = n[0] * on[0] + n[1] * on[1];
            for r in 0..4 {
                for c in 0..4 {
                    ata[r][c] += sm.w * j[r] * j[c];
                }
                atb[r] += sm.w * j[r] * target;
            }
            n_matched += 1;
        }
        if n_matched < 4 {
            break;
        }
        match solve4(ata, atb) {
            Some([a, b, tx, ty]) => {
                let next = Sim { a, b, tx, ty };
                if !next.finite() {
                    break;
                }
                sim = next;
            }
            None => break,
        }
    }
    sim
}

/// `A·x = b` for a 4 × 4 system (partial pivoting); none when singular.
#[allow(clippy::needless_range_loop)]
fn solve4(mut a: [[f64; 4]; 4], mut b: [f64; 4]) -> Option<[f64; 4]> {
    let scale = a.iter().flat_map(|r| r.iter()).fold(0.0f64, |m, v| m.max(v.abs()));
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    for col in 0..4 {
        let piv = (col..4).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-9 * scale {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..4 {
            let f = a[row][col] / a[col][col];
            for k in col..4 {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0f64; 4];
    for row in (0..4).rev() {
        let mut s = b[row];
        for k in row + 1..4 {
            s -= a[row][k] * x[k];
        }
        x[row] = s / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Length-weighted direction histogram modulo 90°, smoothed.
fn directions(lines: &[Line]) -> Vec<f64> {
    let n = (90.0 / ANGLE_BIN_DEG).round() as usize;
    let mut h = vec![0.0f64; n];
    for l in lines {
        let k = ((l.ang.to_degrees().rem_euclid(90.0) / ANGLE_BIN_DEG).round() as usize) % n;
        if let Some(v) = h.get_mut(k) {
            *v += l.len;
        }
    }
    // Triangular smoothing, ±3 bins (circular).
    (0..n)
        .map(|i| {
            (-3i64..=3)
                .map(|d| {
                    let k = (i as i64 + d).rem_euclid(n as i64) as usize;
                    h.get(k).copied().unwrap_or(0.0) * (4 - d.abs()) as f64
                })
                .sum()
        })
        .collect()
}

/// Top peaks of a histogram at least `apart` bins from each other: (bin, value).
fn peaks(h: &[f64], apart: usize, count: usize) -> Vec<(usize, f64)> {
    let mut idx: Vec<usize> = (0..h.len()).filter(|&i| h.get(i).copied().unwrap_or(0.0) > 0.0).collect();
    idx.sort_by(|&i, &j| h.get(j).copied().unwrap_or(0.0).total_cmp(&h.get(i).copied().unwrap_or(0.0)).then(i.cmp(&j)));
    let mut out: Vec<(usize, f64)> = Vec::new();
    for i in idx {
        if out.len() >= count {
            break;
        }
        if out.iter().all(|(k, _)| k.abs_diff(i) >= apart) {
            out.push((i, h.get(i).copied().unwrap_or(0.0)));
        }
    }
    out
}

fn rotate(p: [f64; 2], ang: f64) -> [f64; 2] {
    let (c, s) = (ang.cos(), ang.sin());
    [c * p[0] - s * p[1], s * p[0] + c * p[1]]
}

/// A line's place across its way and its length, for one way's lines (after turning the main
/// axis onto x): `x` for lines running up, `y` for lines running across.
fn way(lines: &[Line], turn: f64, vertical: bool) -> Vec<(f64, f64)> {
    let tol = FAMILY_DEG.to_radians();
    lines
        .iter()
        .filter_map(|l| {
            let ang = (l.ang + turn).rem_euclid(std::f64::consts::PI);
            let off = if vertical { (ang - std::f64::consts::FRAC_PI_2).abs() } else { ang.min(std::f64::consts::PI - ang) };
            if off > tol {
                return None;
            }
            let m = rotate(l.mid(), turn);
            Some((if vertical { m[0] } else { m[1] }, l.len))
        })
        .collect()
}

/// The shift votes of one way at scale `s`: smoothed histogram over [−REACH, REACH].
fn shift_votes(model: &[(f64, f64)], print: &[(f64, f64)], s: f64) -> Vec<f64> {
    let n = (2.0 * REACH_FT / SHIFT_BIN_FT).round() as usize + 1;
    let mut h = vec![0.0f64; n];
    for (x, lm) in model {
        for (xp, lp) in print {
            let t = xp - s * x;
            if t.abs() > REACH_FT {
                continue;
            }
            let k = ((t + REACH_FT) / SHIFT_BIN_FT).round() as usize;
            if let Some(v) = h.get_mut(k) {
                *v += (s * lm).min(*lp);
            }
        }
    }
    (0..n)
        .map(|i| {
            (-2i64..=2)
                .map(|d| {
                    let k = i as i64 + d;
                    if k < 0 { 0.0 } else { h.get(k as usize).copied().unwrap_or(0.0) * (3 - d.abs()) as f64 }
                })
                .sum()
        })
        .collect()
}

/// Fit the sheet's lines (`print`, sheet units) to the model's wall faces as placed (`model`,
/// sheet units), `unit` sheet units a foot, round `centre`. Err: why it can't be checked.
pub(crate) fn fit_walls(model: &[Seg], print: &[Seg], unit: f64, centre: [f64; 2]) -> Result<Fit, String> {
    if !(unit.is_finite() && unit > 0.0 && centre.iter().all(|v| v.is_finite())) {
        return Err("the crop's scale is unknown".into());
    }
    let local = |p: [f64; 2]| [(p[0] - centre[0]) / unit, (p[1] - centre[1]) / unit];
    let walls: Vec<Line> = model.iter().take(MAX_MODEL_LINES).filter_map(|(a, b)| Line::of(local(*a), local(*b))).collect();
    let walls_ft: f64 = walls.iter().map(|l| l.len).sum();
    if walls_ft < 2.0 * MIN_FAMILY_FT {
        return Err("no model walls in the crop to fit (model.build makes them)".into());
    }
    let mut lines: Vec<Line> = print.iter().filter_map(|(a, b)| Line::of(local(*a), local(*b))).collect();
    if lines.is_empty() {
        return Err("the crop holds no straight lines of 1 ft or more".into());
    }
    if lines.len() > MAX_PRINT_LINES {
        lines.sort_by(|x, y| y.len.total_cmp(&x.len).then(x.a[0].total_cmp(&y.a[0])).then(x.a[1].total_cmp(&y.a[1])));
        lines.truncate(MAX_PRINT_LINES);
    }
    let n_lines = lines.len();

    // The walls' main axis and the turn between the two sets' directions.
    let hm = directions(&walls);
    let hp = directions(&lines);
    let nb = hm.len();
    let axis_bin = peaks(&hm, 1, 1).first().map_or(0, |p| p.0);
    let axis = (axis_bin as f64 * ANGLE_BIN_DEG).to_radians();
    let max_k = (MAX_TURN_DEG / ANGLE_BIN_DEG).round() as i64;
    let mut best_turn = (0i64, f64::NEG_INFINITY);
    for k in -max_k..=max_k {
        let s: f64 =
            (0..nb).map(|i| hm.get(i).copied().unwrap_or(0.0) * hp.get((i as i64 + k).rem_euclid(nb as i64) as usize).copied().unwrap_or(0.0)).sum();
        if s > best_turn.1 + 1e-9 || ((s - best_turn.1).abs() <= 1e-9 && k.abs() < best_turn.0.abs()) {
            best_turn = (k, s);
        }
    }
    let turn = (best_turn.0 as f64 * ANGLE_BIN_DEG).to_radians();

    // The walls and the sheet's lines each way, each set turned so its main axis is on x (the
    // sheet's axis is the walls' turned by the turn found).
    let mv = way(&walls, -axis, true);
    let mh = way(&walls, -axis, false);
    let pv = way(&lines, -(axis + turn), true);
    let ph = way(&lines, -(axis + turn), false);
    if mv.iter().map(|x| x.1).sum::<f64>() < MIN_FAMILY_FT || mh.iter().map(|x| x.1).sum::<f64>() < MIN_FAMILY_FT {
        return Err("the model's walls here run one way only: a fit can't tell the shift along them".into());
    }

    // Scale and shift by voting.
    let steps = ((SCALE_RANGE.1 - SCALE_RANGE.0) / SCALE_STEP).round() as usize;
    let mut by_scale: Vec<(f64, Vec<(usize, f64)>, Vec<(usize, f64)>, f64)> = Vec::new();
    for i in 0..=steps {
        let s = SCALE_RANGE.0 + i as f64 * SCALE_STEP;
        let px = peaks(&shift_votes(&mv, &pv, s), 5, 2);
        let py = peaks(&shift_votes(&mh, &ph, s), 5, 2);
        let score = px.first().map_or(0.0, |p| p.1) + py.first().map_or(0.0, |p| p.1);
        by_scale.push((s, px, py, score));
    }
    let scores: Vec<f64> = by_scale.iter().map(|x| x.3).collect();
    let mut starts: Vec<Sim> = vec![Sim::IDENTITY];
    for (i, _) in peaks(&scores, 4, 3) {
        let Some((s, px, py, _)) = by_scale.get(i) else { continue };
        let tx: Vec<f64> = if px.is_empty() { vec![0.0] } else { px.iter().map(|p| p.0 as f64 * SHIFT_BIN_FT - REACH_FT).collect() };
        let ty: Vec<f64> = if py.is_empty() { vec![0.0] } else { py.iter().map(|p| p.0 as f64 * SHIFT_BIN_FT - REACH_FT).collect() };
        for x in &tx {
            for y in &ty {
                let t = rotate([*x, *y], axis + turn);
                starts.push(Sim { a: s * turn.cos(), b: s * turn.sin(), tx: t[0], ty: t[1] });
            }
        }
    }

    // Refine each, keep the one that puts the most wall on the sheet's lines.
    let samples = sample(&walls);
    let grid = Grid::new(lines, ROUNDS_FT[0]);
    let before = evaluate(&samples, &grid, &Sim::IDENTITY, axis);
    let mut found: Vec<(Sim, Quality, f64)> = Vec::new();
    for st in starts {
        let sim = refine(&samples, &grid, st);
        let (sc, rot) = (sim.scale(), sim.rotation_deg());
        if !(sim.finite() && sc >= SCALE_RANGE.0 - 0.05 && sc <= SCALE_RANGE.1 + 0.05 && rot.abs() <= MAX_TURN_DEG + 1.0) {
            continue;
        }
        let q = evaluate(&samples, &grid, &sim, axis);
        found.push((sim, q, displacement(&samples, &sim)));
    }
    let top = found.iter().map(|f| f.1.coverage).fold(0.0f64, f64::max);
    let (sim, q, moved) = found.iter().filter(|f| f.1.coverage >= top - SAME_COVERAGE).min_by(|x, y| x.2.total_cmp(&y.2)).copied().unwrap_or((
        Sim::IDENTITY,
        before,
        0.0,
    ));
    let weak = if q.coverage < MIN_COVERAGE {
        Some(format!("only {:.0}% of the walls' length lies on the sheet's lines after the best fit", q.coverage * 100.0))
    } else if q.family[0] < MIN_FAMILY_FT || q.family[1] < MIN_FAMILY_FT {
        Some("the sheet's lines match the walls one way only".to_string())
    } else {
        None
    };
    // Back to the sheet: q ↦ c + unit·T((q − c)/unit).
    let to_local = Sim { a: 1.0 / unit, b: 0.0, tx: -centre[0] / unit, ty: -centre[1] / unit };
    let from_local = Sim { a: unit, b: 0.0, tx: centre[0], ty: centre[1] };
    let correction = from_local.after(&sim.after(&to_local));
    Ok(Fit {
        correction,
        residual_ft: moved,
        rotation_deg: sim.rotation_deg(),
        scale: sim.scale(),
        shift_ft: [sim.tx, sim.ty],
        fit_ft: if q.fit_ft.is_finite() { q.fit_ft } else { 0.0 },
        coverage: q.coverage,
        coverage_before: before.coverage,
        good: weak.is_none(),
        weak,
        walls_ft,
        lines: n_lines,
    })
}

/// Samples along the walls, about every [`SAMPLE_FT`] (wider when there are many).
fn sample(walls: &[Line]) -> Vec<Sample> {
    let total: f64 = walls.iter().map(|l| l.len).sum();
    let step = SAMPLE_FT.max(total / MAX_SAMPLES as f64);
    let mut out = Vec::new();
    for l in walls {
        let n = ((l.len / step).floor() as usize).clamp(2, MAX_SAMPLES);
        let w = l.len / n as f64;
        for i in 0..n {
            if out.len() >= MAX_SAMPLES {
                return out;
            }
            let t = (i as f64 + 0.5) * w;
            out.push(Sample { p: [l.a[0] + l.u[0] * t, l.a[1] + l.u[1] * t], u: l.u, w });
        }
    }
    out
}

/// What the check says about one crop.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Registered {
    pub status: &'static str,
    pub json: Value,
    /// For a refitted crop: the correction on the sheet.
    pub correction: Option<Sim>,
}

fn r2(x: f64) -> f64 {
    let y = (x * 100.0).round() / 100.0;
    if y == 0.0 || !y.is_finite() { 0.0 } else { y }
}
fn r3(x: f64) -> f64 {
    let y = (x * 1000.0).round() / 1000.0;
    if y == 0.0 || !y.is_finite() { 0.0 } else { y }
}

/// The verdict on a fit against the threshold (feet).
pub(crate) fn judge(fit: Result<Fit, String>, threshold_ft: f64) -> Registered {
    let f = match fit {
        Err(why) => return Registered { status: "not_checked", json: json!({"status": "not_checked", "why": why}), correction: None },
        Ok(f) => f,
    };
    let numbers = json!({
        "residual_ft": r2(f.residual_ft),
        "rotation_deg": r2(f.rotation_deg),
        "scale": r3(f.scale),
        "shift_ft": [r2(f.shift_ft[0]), r2(f.shift_ft[1])],
        "fit_ft": r2(f.fit_ft),
        "coverage": r2(f.coverage),
        "coverage_before": r2(f.coverage_before),
        "walls_ft": r2(f.walls_ft.min(1e6)),
        "lines": f.lines,
        "threshold_ft": r2(threshold_ft),
    });
    let mut o = numbers;
    let how = format!("{:.2} ft off the model's walls (turned {:.2}°, ×{:.3})", f.residual_ft, f.rotation_deg, f.scale);
    let (status, correction) = if !f.good {
        o["why"] = json!(f.weak.clone().unwrap_or_default());
        o["read"] = json!(format!(
            "misregistered: the sheet's lines don't fit the model's walls well enough to place it ({}); the best fit puts it {how}: measure from the walls, not from the overlay",
            f.weak.as_deref().unwrap_or("a weak fit")
        ));
        ("misregistered", None)
    } else if f.residual_ft <= threshold_ft {
        ("ok", None)
    } else {
        o["read"] = json!(format!("placed {how}: placed again from the sheet's wall lines (they lie {:.2} ft from the walls after it)", f.fit_ft));
        ("refitted", Some(f.correction))
    };
    o["status"] = json!(status);
    Registered { status, json: o, correction }
}

/// Where a crop's registration ranks, worst first: misregistered (its error unknown), then by
/// residual, largest first; unchecked last.
pub(crate) fn rank(status: &str, residual_ft: Option<f64>) -> (u8, f64) {
    match status {
        "misregistered" => (0, -residual_ft.unwrap_or(0.0)),
        "not_checked" => (2, 0.0),
        _ => (1, -residual_ft.unwrap_or(0.0)),
    }
}

/// The packet's registration summary from its prints (each placed crop's `registration`): the
/// threshold and where it came from, the count of each verdict, the crops worst first.
pub(crate) fn summary(prints: &[Value], threshold_ft: f64, rule: Option<&str>) -> Value {
    let mut rows: Vec<Value> = prints
        .iter()
        .filter_map(|c| {
            let r = c.get("registration")?;
            let mut o = json!({"file": c.get("file"), "sheet": c.get("sheet"), "status": r.get("status"), "residual_ft": r.get("residual_ft")});
            if let Some(v) = c.get("view") {
                o["view"] = v.clone();
            }
            Some(o)
        })
        .collect();
    let key = |v: &Value| rank(v["status"].as_str().unwrap_or_default(), v["residual_ft"].as_f64());
    rows.sort_by(|a, b| {
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1)).then(a["file"].as_str().cmp(&b["file"].as_str()))
    });
    let count = |st: &str| rows.iter().filter(|r| r["status"] == st).count();
    json!({
        "about": "each placed print crop's registration against the model's walls: the model's wall faces round the area, carried onto the sheet as the crop is, fitted to the sheet's own lines there (shift, turn, scale); residual_ft is how far the placement is off (the RMS move of the walls), fit_ft how close they lie to the sheet's lines after the fit, coverage the share of their length that does; over the threshold a good fit places the crop again (refitted), a weak one marks it (misregistered: measure from the walls, not from the overlay)",
        "threshold_ft": r2(threshold_ft),
        "threshold_from": rule.map_or_else(|| format!("default ({DEFAULT_RESIDUAL_FT} ft; project rule {RESIDUAL_KEY}, domain tolerance, changes it)"), |k| format!("project rule {k}")),
        "counts": {"ok": count("ok"), "refitted": count("refitted"), "misregistered": count("misregistered"), "not_checked": count("not_checked")},
        "by_residual": rows,
    })
}

/// The judge's note on the crops that were placed again or are misregistered (empty: none).
pub(crate) fn prompt_section(prints: &[Value]) -> String {
    let mut out = String::new();
    for c in prints {
        let Some(r) = c.get("registration") else { continue };
        let status = r["status"].as_str().unwrap_or_default();
        if !matches!(status, "refitted" | "misregistered") {
            continue;
        }
        out.push_str(&format!(
            "- `{}` (sheet {}): {}\n",
            c["file"].as_str().unwrap_or_default(),
            c["sheet"].as_str().unwrap_or_default(),
            r["read"].as_str().unwrap_or(status)
        ));
    }
    if out.is_empty() {
        return out;
    }
    format!(
        "\n## Print crops placed off the model's walls\n\nEach placed crop's registration was checked against the model's walls (manifest `registration`).\n\n{out}"
    )
}

// ---------------------------------------------------------------- the project rule

/// Is `key` a registration rule (`validate.registration.…`)?
pub fn is_register_key(key: &str) -> bool {
    key.trim().get(..KEY_PREFIX.len()).is_some_and(|p| p.eq_ignore_ascii_case(KEY_PREFIX))
}

/// Read a project rule `validate.registration.residual_ft = <feet>`; `project.rules.set`
/// refuses one that doesn't read.
pub fn check_register_rule(domain: RuleDomain, key: &str, value: &str) -> std::result::Result<f64, String> {
    if !key.trim().eq_ignore_ascii_case(RESIDUAL_KEY) {
        return Err(format!("`{key}`: the registration rule a project may set is `{RESIDUAL_KEY}` (feet)"));
    }
    if domain != RuleDomain::Tolerance {
        return Err(format!("`{key}` is a tolerance: set it in domain `tolerance`"));
    }
    let v = value.trim();
    let num = v.strip_suffix("ft").or_else(|| v.strip_suffix('\'')).unwrap_or(v).trim();
    let n: f64 = num.parse().map_err(|_| format!("`{key}` is feet, a number, not `{}`", v.chars().take(40).collect::<String>()))?;
    if !(n.is_finite() && (RESIDUAL_RANGE.0..=RESIDUAL_RANGE.1).contains(&n)) {
        return Err(format!("`{key}` is from {} to {} ft", RESIDUAL_RANGE.0, RESIDUAL_RANGE.1));
    }
    Ok(n)
}

/// The threshold in feet and the project rule it came from (none: the default).
pub(crate) fn threshold(pr: &ProjectRules) -> (f64, Option<String>) {
    pr.list_domain(RuleDomain::Tolerance)
        .into_iter()
        .filter(|e| e.key.trim().eq_ignore_ascii_case(RESIDUAL_KEY))
        .find_map(|e| check_register_rule(RuleDomain::Tolerance, &e.key, &e.active.value).ok().map(|n| (n, Some(e.key.clone()))))
        .unwrap_or((DEFAULT_RESIDUAL_FT, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up floor in feet: a 70' × 44' shell, rooms of unequal widths both sides of a
    /// corridor, a few short walls; each wall as its two faces 0.5' apart.
    fn floor() -> Vec<Seg> {
        let mut walls: Vec<Seg> = Vec::new();
        let mut wall = |a: [f64; 2], b: [f64; 2]| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l = dx.hypot(dy);
            let (nx, ny) = (-dy / l * 0.25, dx / l * 0.25);
            walls.push(([a[0] + nx, a[1] + ny], [b[0] + nx, b[1] + ny]));
            walls.push(([a[0] - nx, a[1] - ny], [b[0] - nx, b[1] - ny]));
        };
        wall([0.0, 0.0], [70.0, 0.0]);
        wall([70.0, 0.0], [70.0, 44.0]);
        wall([70.0, 44.0], [0.0, 44.0]);
        wall([0.0, 44.0], [0.0, 0.0]);
        wall([0.0, 18.0], [70.0, 18.0]);
        wall([0.0, 24.0], [70.0, 24.0]);
        for x in [13.0, 27.5, 41.0, 58.0] {
            wall([x, 0.0], [x, 18.0]);
        }
        for x in [9.0, 24.0, 36.5, 52.0] {
            wall([x, 24.0], [x, 44.0]);
        }
        wall([3.0, 30.0], [9.0, 30.0]);
        wall([41.0, 8.0], [47.0, 8.0]);
        walls
    }

    /// The model's faces on a sheet at 12 units a foot from (100, 200), and the print drawn by
    /// `truth` (feet → feet) from the same origin, with noise lines: text strokes, a door swing's
    /// chord and a dimension line.
    fn sheets(truth: impl Fn([f64; 2]) -> [f64; 2]) -> (Vec<Seg>, Vec<Seg>) {
        let on = |p: [f64; 2]| [100.0 + p[0] * 12.0, 200.0 + p[1] * 12.0];
        let model: Vec<Seg> = floor().iter().map(|(a, b)| (on(*a), on(*b))).collect();
        let mut print: Vec<Seg> = floor().iter().map(|(a, b)| (on(truth(*a)), on(truth(*b)))).collect();
        for k in 0..40 {
            let x = 2.0 + f64::from(k) * 1.6;
            print.push((on(truth([x, 10.0])), on(truth([x + 0.6, 10.9]))));
        }
        print.push((on(truth([14.0, 3.0])), on(truth([17.0, 6.0]))));
        print.push((on(truth([0.0, -4.0])), on(truth([70.0, -4.0]))));
        (model, print)
    }

    fn check(truth: impl Fn([f64; 2]) -> [f64; 2]) -> Fit {
        let (model, print) = sheets(truth);
        fit_walls(&model, &print, 12.0, [100.0 + 35.0 * 12.0, 200.0 + 22.0 * 12.0]).unwrap()
    }

    #[test]
    fn a_3_ft_shift_is_found_and_corrected() {
        let f = check(|p| [p[0] + 3.0, p[1]]);
        assert!(f.good, "{f:?}");
        assert!((f.residual_ft - 3.0).abs() < 0.1, "{f:?}");
        assert!((f.shift_ft[0] - 3.0).abs() < 0.1 && f.shift_ft[1].abs() < 0.1, "{f:?}");
        assert!(f.rotation_deg.abs() < 0.05 && (f.scale - 1.0).abs() < 0.002, "{f:?}");
        // Shifted along x, the walls running across still lie on their lines: about half.
        assert!(f.coverage > 0.9 && f.coverage_before < 0.6 && f.fit_ft < 0.05, "{f:?}");
        let r = judge(Ok(f), DEFAULT_RESIDUAL_FT);
        assert_eq!(r.status, "refitted");
        // The correction carries a model point to where the sheet really has it.
        let c = r.correction.unwrap();
        let q = c.apply([100.0 + 13.0 * 12.0, 200.0 + 5.0 * 12.0]);
        assert!((q[0] - (100.0 + 16.0 * 12.0)).abs() < 1.0 && (q[1] - (200.0 + 5.0 * 12.0)).abs() < 1.0, "{q:?}");
    }

    #[test]
    fn a_3_degree_skew_is_found() {
        // Turned 3° about a point well off the floor (the sheet's registration point).
        let t = 3.0f64.to_radians();
        let f = check(|p| {
            let (x, y) = (p[0] + 40.0, p[1] + 10.0);
            [x * t.cos() - y * t.sin() - 40.0, x * t.sin() + y * t.cos() - 10.0]
        });
        assert!(f.good, "{f:?}");
        assert!((f.rotation_deg - 3.0).abs() < 0.05, "{f:?}");
        assert!((f.scale - 1.0).abs() < 0.002, "{f:?}");
        assert!(f.residual_ft > 1.0, "{f:?}");
        assert_eq!(judge(Ok(f), DEFAULT_RESIDUAL_FT).status, "refitted");
    }

    #[test]
    fn a_1_1_scale_is_found() {
        let f = check(|p| [(p[0] - 10.0) * 1.1 + 10.0, (p[1] - 5.0) * 1.1 + 5.0]);
        assert!(f.good, "{f:?}");
        assert!((f.scale - 1.1).abs() < 0.003, "{f:?}");
        assert!(f.rotation_deg.abs() < 0.05, "{f:?}");
        assert!(f.residual_ft > 1.0, "{f:?}");
    }

    #[test]
    fn shift_turn_and_scale_together() {
        let t = (-2.0f64).to_radians();
        let f = check(|p| {
            let (x, y) = (p[0] * 0.92, p[1] * 0.92);
            [x * t.cos() - y * t.sin() - 4.0, x * t.sin() + y * t.cos() + 2.5]
        });
        assert!(f.good, "{f:?}");
        assert!((f.rotation_deg + 2.0).abs() < 0.05 && (f.scale - 0.92).abs() < 0.003, "{f:?}");
    }

    #[test]
    fn a_true_registration_is_ok_and_kept() {
        let f = check(|p| p);
        assert!(f.good && f.residual_ft < 0.02 && f.coverage > 0.95, "{f:?}");
        let r = judge(Ok(f), DEFAULT_RESIDUAL_FT);
        assert_eq!(r.status, "ok");
        assert!(r.correction.is_none());
        // Half a foot off is within the threshold: kept as placed.
        let f = check(|p| [p[0] + 0.5, p[1] - 0.3]);
        assert!((f.residual_ft - 0.58).abs() < 0.05, "{f:?}");
        assert_eq!(judge(Ok(f.clone()), DEFAULT_RESIDUAL_FT).status, "ok");
        // A tighter project threshold refits it.
        assert_eq!(judge(Ok(f), 0.25).status, "refitted");
    }

    #[test]
    fn lines_that_fit_no_walls_are_misregistered_with_the_numbers() {
        let (model, _) = sheets(|p| p);
        // A sheet of other lines: a coarse grid not like the floor.
        let mut print: Vec<Seg> = Vec::new();
        for k in 0..6 {
            let v = 100.0 + f64::from(k) * 61.0;
            print.push(([v, 150.0], [v, 800.0]));
        }
        let r = judge(fit_walls(&model, &print, 12.0, [520.0, 464.0]), DEFAULT_RESIDUAL_FT);
        assert_eq!(r.status, "misregistered", "{:?}", r.json);
        assert!(r.json["why"].as_str().is_some_and(|w| !w.is_empty()), "{}", r.json);
        assert!(r.json["coverage"].as_f64().is_some() && r.json["residual_ft"].as_f64().is_some(), "{}", r.json);
        assert!(r.correction.is_none());
    }

    #[test]
    fn nothing_to_fit_is_not_checked() {
        let (model, print) = sheets(|p| p);
        assert_eq!(judge(fit_walls(&[], &print, 12.0, [0.0, 0.0]), 1.0).status, "not_checked");
        assert_eq!(judge(fit_walls(&model, &[], 12.0, [0.0, 0.0]), 1.0).status, "not_checked");
        // Walls one way only.
        let one: Vec<Seg> = model.iter().filter(|(a, b)| (a[1] - b[1]).abs() < 1e-9).copied().collect();
        let r = judge(fit_walls(&one, &print, 12.0, [0.0, 0.0]), 1.0);
        assert_eq!(r.status, "not_checked");
        assert!(r.json["why"].as_str().unwrap().contains("one way"), "{}", r.json);
        // Hostile numbers never panic.
        assert!(fit_walls(&model, &print, f64::NAN, [0.0, 0.0]).is_err());
        assert!(fit_walls(&model, &print, 0.0, [0.0, 0.0]).is_err());
        let wild: Vec<Seg> = vec![([f64::INFINITY, 0.0], [1.0, 1.0]), ([0.0, f64::NAN], [5.0, 5.0]), ([1e300, 1e300], [-1e300, -1e300])];
        let _ = fit_walls(&model, &wild, 12.0, [0.0, 0.0]);
        let _ = fit_walls(&wild, &wild, 12.0, [0.0, 0.0]);
    }

    #[test]
    fn similarities_compose_and_invert() {
        let s = Sim { a: 0.9, b: 0.2, tx: 3.0, ty: -1.0 };
        let i = s.inverse().unwrap();
        let p = [7.0, -2.5];
        let q = i.apply(s.apply(p));
        assert!((q[0] - p[0]).abs() < 1e-9 && (q[1] - p[1]).abs() < 1e-9);
        let t = Sim { a: 1.1, b: -0.1, tx: 0.5, ty: 2.0 };
        let both = t.after(&s).apply(p);
        let step = t.apply(s.apply(p));
        assert!((both[0] - step[0]).abs() < 1e-9 && (both[1] - step[1]).abs() < 1e-9);
        assert!(Sim { a: 0.0, b: 0.0, tx: 1.0, ty: 1.0 }.inverse().is_none());
    }

    #[test]
    fn the_threshold_is_a_project_rule_in_feet() {
        assert!(is_register_key("validate.registration.residual_ft") && is_register_key("VALIDATE.Registration.x"));
        assert!(!is_register_key("validate.G-HOST.tolerance.plan_in"));
        assert_eq!(check_register_rule(RuleDomain::Tolerance, RESIDUAL_KEY, "2"), Ok(2.0));
        assert_eq!(check_register_rule(RuleDomain::Tolerance, RESIDUAL_KEY, " 1.5 ft"), Ok(1.5));
        assert_eq!(check_register_rule(RuleDomain::Tolerance, RESIDUAL_KEY, "0.5'"), Ok(0.5));
        assert!(check_register_rule(RuleDomain::Custom, RESIDUAL_KEY, "2").unwrap_err().contains("tolerance"));
        assert!(check_register_rule(RuleDomain::Tolerance, RESIDUAL_KEY, "-1").is_err());
        assert!(check_register_rule(RuleDomain::Tolerance, RESIDUAL_KEY, "NaN").is_err());
        assert!(check_register_rule(RuleDomain::Tolerance, RESIDUAL_KEY, "wide").is_err());
        assert!(check_register_rule(RuleDomain::Tolerance, "validate.registration.rotation", "2").is_err());
        let mut pr = ProjectRules::new();
        assert_eq!(threshold(&pr), (DEFAULT_RESIDUAL_FT, None));
        pr.set(RuleDomain::Tolerance, RESIDUAL_KEY, cadaddy_doc::projrules::RuleValue::new("3"), "loose prints");
        assert_eq!(threshold(&pr), (3.0, Some(RESIDUAL_KEY.to_string())));
        // One that doesn't read (written past the check) leaves the default.
        let mut pr = ProjectRules::new();
        pr.set(RuleDomain::Tolerance, RESIDUAL_KEY, cadaddy_doc::projrules::RuleValue::new("wide"), "imported");
        assert_eq!(threshold(&pr).0, DEFAULT_RESIDUAL_FT);
    }
}
