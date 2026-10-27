//! Sheet registration and background subtraction (Q5258 stagedbuild).
//!
//! Every electrical, mechanical, plumbing and fire protection plan is drawn over the
//! architectural plan of its level. Read on its own, that background's wall pairs, grids and
//! borders look like ducts and pipes; read against the architectural plan they are not: a line
//! that is on the A sheet is the building, a line that is not is the trade's.
//!
//! [`register`] finds where a trade sheet sits on its level's architectural plan: a similarity
//! transform (scale, rotation, offset) fitted to the labels both sheets print (grid bubbles, room
//! and door tags) and checked against their linework, with the sheet's own placement (same sheet
//! size: same title block and plan placement) as the other candidate. The better one wins when
//! enough of the linework lines up ([`MIN_MATCHED`]); otherwise the sheet is not registered and
//! the caller asks. [`Registration::background`] lists the trade sheet's objects that lie on
//! architectural lines (and its labels the A sheet prints at the same place): the trade readers
//! never see them.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::{Polyline, Segment, Vec2};
use serde_json::{Value, json};

use super::printmep::SheetRef;

/// Segments looked at per sheet, at most (hostile drawings).
const MAX_SEGS: usize = 300_000;
/// Labels looked at per sheet, at most.
const MAX_TEXTS: usize = 20_000;
/// Label correspondences tried, at most.
const MAX_PAIRS: usize = 4_000;
/// Two-correspondence fits tried, at most.
const MAX_TRIALS: usize = 60_000;
/// A label printed more often than this on one sheet says nothing about where it is.
const MAX_SAME_LABEL: usize = 6;
/// Lines lined up, at least, for a sheet to count as registered.
pub(crate) const MIN_MATCHED: f64 = 0.6;
/// A trade object is background when this much of its length lies on architectural lines.
const BACKGROUND_SHARE: f64 = 0.8;
/// Line tolerance for scoring a fit, as a share of the architectural sheet's diagonal (about
/// 3" on a 36" sheet at 1/8" = 1'-0").
const TOL_SHARE: f64 = 0.0008;
/// Line tolerance for "this line is the A plan's" (background, runs on walls): a background is
/// printed where the A plan's line is, while a pipe in a wall cavity is a few inches off its
/// faces (about 1").
const TIGHT_SHARE: f64 = 0.00025;
/// Lines shorter than this share of the diagonal are not used to score a fit.
const LONG_SHARE: f64 = 0.004;
/// An object smaller than this (feet, in the building) is a symbol, never background.
const SYMBOL_FT: f64 = 3.0;
/// An object spanning this share of the sheet's extents is its frame (border, title block box).
const FRAME_SHARE: f64 = 0.85;
/// Two lines are parallel within this sine (about 3 degrees).
const PARALLEL_SIN: f64 = 0.05;

/// `p ↦ m·p + t` with `m` a complex number (scale and rotation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Similarity {
    pub m: [f64; 2],
    pub t: [f64; 2],
}

impl Similarity {
    pub fn identity() -> Similarity {
        Similarity { m: [1.0, 0.0], t: [0.0, 0.0] }
    }
    /// Scale by `k` about `from`, then move `from` onto `to`.
    pub fn place(from: [f64; 2], to: [f64; 2], k: f64) -> Similarity {
        Similarity { m: [k, 0.0], t: [to[0] - k * from[0], to[1] - k * from[1]] }
    }
    pub fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        [self.m[0] * p[0] - self.m[1] * p[1] + self.t[0], self.m[1] * p[0] + self.m[0] * p[1] + self.t[1]]
    }
    pub fn scale(&self) -> f64 {
        self.m[0].hypot(self.m[1])
    }
    pub fn rotation_deg(&self) -> f64 {
        self.m[1].atan2(self.m[0]).to_degrees()
    }
    /// The box `min`..`max` moved: the box round its four moved corners.
    pub fn apply_box(&self, min: [f64; 2], max: [f64; 2]) -> ([f64; 2], [f64; 2]) {
        let c = [self.apply(min), self.apply([max[0], min[1]]), self.apply(max), self.apply([min[0], max[1]])];
        let lo = [c.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min), c.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min)];
        let hi = [c.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max), c.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max)];
        (lo, hi)
    }
    fn sane(&self) -> bool {
        let k = self.scale();
        self.m.iter().chain(self.t.iter()).all(|v| v.is_finite()) && (0.02..=50.0).contains(&k)
    }
    /// The transform taking `t1` to `a1` and `t2` to `a2`.
    fn from_two(t1: [f64; 2], t2: [f64; 2], a1: [f64; 2], a2: [f64; 2]) -> Option<Similarity> {
        let dt = [t2[0] - t1[0], t2[1] - t1[1]];
        let da = [a2[0] - a1[0], a2[1] - a1[1]];
        let n = dt[0] * dt[0] + dt[1] * dt[1];
        if !(n.is_finite() && n > 1e-12) {
            return None;
        }
        let m = [(da[0] * dt[0] + da[1] * dt[1]) / n, (da[1] * dt[0] - da[0] * dt[1]) / n];
        let s = Similarity { m, t: [a1[0] - (m[0] * t1[0] - m[1] * t1[1]), a1[1] - (m[1] * t1[0] + m[0] * t1[1])] };
        s.sane().then_some(s)
    }
    /// The least-squares transform taking each `.0` to its `.1`.
    fn fit(pairs: &[([f64; 2], [f64; 2])]) -> Option<Similarity> {
        let n = pairs.len() as f64;
        if pairs.len() < 2 {
            return None;
        }
        let ct = [pairs.iter().map(|p| p.0[0]).sum::<f64>() / n, pairs.iter().map(|p| p.0[1]).sum::<f64>() / n];
        let ca = [pairs.iter().map(|p| p.1[0]).sum::<f64>() / n, pairs.iter().map(|p| p.1[1]).sum::<f64>() / n];
        let (mut re, mut im, mut den) = (0.0, 0.0, 0.0);
        for (t, a) in pairs {
            let (tx, ty, ax, ay) = (t[0] - ct[0], t[1] - ct[1], a[0] - ca[0], a[1] - ca[1]);
            re += ax * tx + ay * ty;
            im += ay * tx - ax * ty;
            den += tx * tx + ty * ty;
        }
        if !(den.is_finite() && den > 1e-12) {
            return None;
        }
        let m = [re / den, im / den];
        let s = Similarity { m, t: [ca[0] - (m[0] * ct[0] - m[1] * ct[1]), ca[1] - (m[1] * ct[0] + m[0] * ct[1])] };
        s.sane().then_some(s)
    }
}

/// A straight piece of one object.
#[derive(Clone, Copy, Debug)]
struct Line {
    a: [f64; 2],
    b: [f64; 2],
    /// Index of its object in [`SheetGeom::objects`].
    object: usize,
}

impl Line {
    fn len(&self) -> f64 {
        (self.b[0] - self.a[0]).hypot(self.b[1] - self.a[1])
    }
}

/// The lines and labels of one sheet.
#[derive(Default)]
pub(crate) struct SheetGeom {
    lines: Vec<Line>,
    /// The objects the lines belong to.
    objects: Vec<Handle>,
    /// Each object's extents, `[x0, y0, x1, y1]`, and how many straight pieces it has.
    boxes: Vec<[f64; 4]>,
    pieces: Vec<usize>,
    /// Normalised label, where, its object.
    texts: Vec<(String, [f64; 2], Handle)>,
    diag: f64,
}

/// A label as compared between sheets: upper case, inner spaces gone; none for long text.
fn label(s: &str) -> Option<String> {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_uppercase).collect();
    (!t.is_empty() && t.chars().count() <= 16 && t.chars().any(|c| c.is_ascii_alphanumeric())).then_some(t)
}

/// The objects of `sh` in `d` (by the handles its import made, else by where they are).
pub(crate) fn sheet_objects<'a>(d: &'a Drawing, sh: &SheetRef) -> Vec<&'a Arc<Entity>> {
    let [x0, y0, x1, y1] = sh.rect;
    d.model
        .iter()
        .filter(|e| match sh.span {
            Some((lo, hi)) => e.handle.0 >= lo && e.handle.0 < hi,
            None => {
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                let (cx, cy) = ((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0);
                cx.is_finite() && cy.is_finite() && cx >= x0 && cx <= x1 && cy >= y0 && cy <= y1
            }
        })
        .collect()
}

pub(crate) fn sheet_geom(d: &Drawing, sh: &SheetRef) -> SheetGeom {
    let mut g = SheetGeom { diag: (sh.rect[2] - sh.rect[0]).hypot(sh.rect[3] - sh.rect[1]), ..SheetGeom::default() };
    for e in sheet_objects(d, sh) {
        if g.lines.len() >= MAX_SEGS {
            break;
        }
        let object = g.objects.len();
        let before = g.lines.len();
        let mut push = |a: Vec2, b: Vec2| {
            if a.is_finite() && b.is_finite() && a.dist(b) > 1e-9 {
                g.lines.push(Line { a: [a.x, a.y], b: [b.x, b.y], object });
            }
        };
        match &e.kind {
            EntityKind::Line(l) => push(l.a.xy(), l.b.xy()),
            EntityKind::LwPolyline(p) => {
                for sg in (Polyline { vertices: p.vertices.clone(), closed: p.closed }).segments().into_iter().take(10_000) {
                    if let Segment::Line(l) = sg {
                        push(l.a, l.b);
                    }
                }
            }
            EntityKind::Text(_) | EntityKind::MText(_) if g.texts.len() < MAX_TEXTS => {
                for (h, text, at, _) in super::perceive::texts(&[e]) {
                    if let Some(l) = label(&text)
                        && at.is_finite()
                    {
                        g.texts.push((l, [at.x, at.y], h));
                    }
                }
            }
            _ => {}
        }
        if g.lines.len() > before {
            g.objects.push(e.handle);
            let mine = g.lines.get(before..).unwrap_or_default();
            let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            for l in mine {
                for p in [l.a, l.b] {
                    b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
                }
            }
            g.boxes.push(b);
            g.pieces.push(mine.len());
        }
    }
    g
}

/// The lines of a sheet in a grid of cells, for "is there a line here".
struct LineIndex {
    cell: f64,
    cells: HashMap<(i64, i64), Vec<u32>>,
    lines: Vec<([f64; 2], [f64; 2])>,
}

impl LineIndex {
    fn new(lines: impl Iterator<Item = ([f64; 2], [f64; 2])>, cell: f64) -> LineIndex {
        let mut ix = LineIndex { cell: cell.max(1e-9), cells: HashMap::new(), lines: Vec::new() };
        for (a, b) in lines.take(MAX_SEGS) {
            let i = ix.lines.len() as u32;
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            let steps = ((len / ix.cell).ceil() as usize).clamp(1, 4_096);
            let mut last = None;
            for k in 0..=steps {
                let f = k as f64 / steps as f64;
                let c = ix.key([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]);
                if last != Some(c) {
                    ix.cells.entry(c).or_default().push(i);
                    last = Some(c);
                }
            }
            ix.lines.push((a, b));
        }
        ix
    }
    fn key(&self, p: [f64; 2]) -> (i64, i64) {
        let f = |v: f64| {
            let c = (v / self.cell).floor();
            if c.is_finite() { c.clamp(-1e15, 1e15) as i64 } else { 0 }
        };
        (f(p[0]), f(p[1]))
    }
    /// A line within `tol` of `p` running along `dir` (a unit vector).
    fn near(&self, p: [f64; 2], dir: [f64; 2], tol: f64) -> bool {
        self.nearest(p, dir, tol).is_some()
    }
    /// The nearest point within `tol` of `p` on a line running along `dir` (a unit vector).
    fn nearest(&self, p: [f64; 2], dir: [f64; 2], tol: f64) -> Option<[f64; 2]> {
        let mut best: Option<([f64; 2], f64)> = None;
        let (cx, cy) = self.key(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(list) = self.cells.get(&(cx + dx, cy + dy)) else { continue };
                for &i in list {
                    let Some(&(a, b)) = self.lines.get(i as usize) else { continue };
                    let (ux, uy) = (b[0] - a[0], b[1] - a[1]);
                    let l2 = ux * ux + uy * uy;
                    if l2 <= 0.0 {
                        continue;
                    }
                    let l = l2.sqrt();
                    if ((ux * dir[1] - uy * dir[0]) / l).abs() > PARALLEL_SIN {
                        continue;
                    }
                    let t = (((p[0] - a[0]) * ux + (p[1] - a[1]) * uy) / l2).clamp(0.0, 1.0);
                    let q = [a[0] + ux * t, a[1] + uy * t];
                    let d2 = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2);
                    if d2 <= tol * tol && best.is_none_or(|(_, b)| d2 < b) {
                        best = Some((q, d2));
                    }
                }
            }
        }
        best.map(|(q, _)| q)
    }
    /// How much of the line `a`..`b` lies on lines here: samples along it.
    fn on_share(&self, a: [f64; 2], b: [f64; 2], tol: f64) -> f64 {
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if !(len.is_finite() && len > 0.0) {
            return 0.0;
        }
        let dir = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        let n = ((len / self.cell).ceil() as usize).clamp(2, 16);
        let hit = (0..n)
            .filter(|k| {
                let f = (*k as f64 + 0.5) / n as f64;
                self.near([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f], dir, tol)
            })
            .count();
        hit as f64 / n as f64
    }
}

/// The architectural plan a sheet is lined up on: its lines indexed once for every trade sheet
/// of the level.
pub(crate) struct ArchPlan {
    pub sheet: String,
    geom: SheetGeom,
    index: LineIndex,
    tol: f64,
    tight: f64,
}

impl ArchPlan {
    pub fn new(d: &Drawing, sh: &SheetRef) -> ArchPlan {
        let geom = sheet_geom(d, sh);
        let tol = (geom.diag * TOL_SHARE).max(1e-6);
        let tight = (geom.diag * TIGHT_SHARE).max(1e-6);
        let index = LineIndex::new(geom.lines.iter().map(|l| (l.a, l.b)), tol * 8.0);
        ArchPlan { sheet: sh.number.clone(), geom, index, tol, tight }
    }
    /// Whether the line `a`..`b` (drawing units, on this plan) lies on its lines: the share.
    pub fn on_lines(&self, a: [f64; 2], b: [f64; 2]) -> f64 {
        self.index.on_share(a, b, self.tight)
    }
}

/// Where a trade sheet sits on its level's architectural plan, and what of it is background.
#[derive(Clone, Debug)]
pub(crate) struct Registration {
    pub sheet: String,
    pub discipline: String,
    pub level: String,
    /// The architectural plan it was lined up on.
    pub arch_sheet: Option<String>,
    pub registered: bool,
    /// `tags` (shared labels), `grids` (grid bubble labels), `sheet` (same sheet size and
    /// placement) or `none`.
    pub method: &'static str,
    /// Share of the lines lined up, 0–1.
    pub matched_pct: f64,
    pub transform: Similarity,
    /// The sheet's objects on architectural lines (and labels the A plan prints there).
    pub background: Vec<Handle>,
    pub background_lines: usize,
    pub residual_lines: usize,
    pub note: Option<String>,
}

impl Registration {
    pub fn to_json(&self) -> Value {
        json!({
            "sheet": self.sheet,
            "discipline": self.discipline,
            "level": super::printmep::level_name(&self.level),
            "arch_sheet": self.arch_sheet,
            "registered": self.registered,
            "method": self.method,
            "matched_pct": (self.matched_pct * 1000.0).round() / 1000.0,
            "scale": (self.transform.scale() * 10_000.0).round() / 10_000.0,
            "rotation_deg": (self.transform.rotation_deg() * 100.0).round() / 100.0 + 0.0,
            "background_lines": self.background_lines,
            "residual_lines": self.residual_lines,
            "note": self.note,
        })
    }
}

/// A trade plan: electrical, mechanical, plumbing or fire protection.
pub(crate) fn is_trade_plan(sh: &SheetRef) -> bool {
    matches!(sh.kind.as_str(), "plan" | "enlarged_plan")
        && matches!(sh.discipline.to_ascii_uppercase().as_str(), "E" | "M" | "P" | "FP" | "F" | "FA" | "MP" | "PL" | "EL")
}

/// The architectural plan of `sh`'s level: a floor plan of the same level (an enlarged plan when
/// that is all there is).
pub(crate) fn arch_of<'a>(sh: &SheetRef, all: &'a [SheetRef]) -> Option<&'a SheetRef> {
    if sh.level.trim().is_empty() {
        return None;
    }
    let of_level =
        |kind: &str| all.iter().find(|a| a.discipline.eq_ignore_ascii_case("A") && a.kind == kind && a.level.eq_ignore_ascii_case(&sh.level));
    of_level("plan").or_else(|| if sh.kind == "enlarged_plan" { of_level("enlarged_plan") } else { None })
}

/// The share of `lines` (moved by `tf`) lying on `ix`'s lines, by length.
fn share_on(lines: &[Line], tf: &Similarity, ix: &LineIndex, tol: f64, min_len: f64) -> f64 {
    let (mut on, mut all) = (0.0, 0.0);
    for l in lines.iter().filter(|l| l.len() * tf.scale() >= min_len) {
        let (a, b) = (tf.apply(l.a), tf.apply(l.b));
        let len = l.len() * tf.scale();
        all += len;
        on += len * ix.on_share(a, b, tol);
    }
    if all > 0.0 { on / all } else { 0.0 }
}

/// The share of the two sheets' long lines that line up under `tf`: the better of "the A plan's
/// lines found on the trade sheet" and "the trade sheet's lines found on the A plan" (a busy
/// trade sheet hides some of its background, a busy A plan has more than any background).
fn matched(trade: &SheetGeom, arch: &ArchPlan, tf: &Similarity) -> f64 {
    let min_len = arch.geom.diag * LONG_SHARE;
    let trade_on_arch = share_on(&trade.lines, tf, &arch.index, arch.tol, min_len);
    let moved = LineIndex::new(trade.lines.iter().map(|l| (tf.apply(l.a), tf.apply(l.b))), arch.tol * 8.0);
    let arch_on_trade = share_on(&arch.geom.lines, &Similarity::identity(), &moved, arch.tol, min_len);
    trade_on_arch.max(arch_on_trade)
}

/// The transform the labels both sheets print agree on (each label printed a few times at
/// most): the fit most label pairs agree with, refined over them. With how many agree and
/// whether they are mostly grid bubbles.
fn by_labels(trade: &SheetGeom, arch: &ArchPlan) -> Option<(Similarity, usize, bool)> {
    let mut on_arch: BTreeMap<&str, Vec<[f64; 2]>> = BTreeMap::new();
    for (l, at, _) in &arch.geom.texts {
        on_arch.entry(l.as_str()).or_default().push(*at);
    }
    let mut on_trade: BTreeMap<&str, Vec<[f64; 2]>> = BTreeMap::new();
    for (l, at, _) in &trade.texts {
        on_trade.entry(l.as_str()).or_default().push(*at);
    }
    let mut pairs: Vec<(&str, [f64; 2], [f64; 2])> = Vec::new();
    for (l, ts) in &on_trade {
        let Some(as_) = on_arch.get(l) else { continue };
        if ts.len() > MAX_SAME_LABEL || as_.len() > MAX_SAME_LABEL {
            continue;
        }
        for t in ts {
            for a in as_ {
                if pairs.len() < MAX_PAIRS {
                    pairs.push((l, *t, *a));
                }
            }
        }
    }
    if pairs.len() < 2 {
        return None;
    }
    // Labels agree within a few line tolerances (a tag's text may sit a little apart).
    let tol = arch.tol * 6.0;
    let agree = |tf: &Similarity| -> Vec<usize> {
        let mut used: Vec<&str> = Vec::new();
        let mut out = Vec::new();
        for (i, (l, t, a)) in pairs.iter().enumerate() {
            let p = tf.apply(*t);
            if (p[0] - a[0]).hypot(p[1] - a[1]) <= tol {
                out.push(i);
                if !used.contains(l) {
                    used.push(l);
                }
            }
        }
        // Two distinct labels at least: one label's two ends say nothing on their own.
        if used.len() < 2 { Vec::new() } else { out }
    };
    let mut best: Option<(Similarity, Vec<usize>)> = None;
    let mut trials = 0usize;
    'outer: for i in 0..pairs.len() {
        for j in (i + 1)..pairs.len() {
            let (Some(p), Some(q)) = (pairs.get(i), pairs.get(j)) else { continue };
            if p.0 == q.0 {
                continue;
            }
            trials += 1;
            if trials > MAX_TRIALS {
                break 'outer;
            }
            let Some(tf) = Similarity::from_two(p.1, q.1, p.2, q.2) else { continue };
            let inl = agree(&tf);
            if inl.len() > best.as_ref().map_or(1, |b| b.1.len()) {
                best = Some((tf, inl));
            }
        }
    }
    let (tf, inl) = best?;
    let fit: Vec<([f64; 2], [f64; 2])> = inl.iter().filter_map(|i| pairs.get(*i)).map(|p| (p.1, p.2)).collect();
    let refined = Similarity::fit(&fit).unwrap_or(tf);
    let grid_like = inl.iter().filter_map(|i| pairs.get(*i)).filter(|p| p.0.chars().count() <= 2).count() * 2 > inl.len();
    Some((refined, inl.len(), grid_like))
}

/// A label fit made exact on the linework: each long trade line's points moved onto the nearest
/// A plan line along it, and the transform fitted again (a few rounds).
fn refine(trade: &SheetGeom, arch: &ArchPlan, mut tf: Similarity) -> Similarity {
    let min_len = arch.geom.diag * LONG_SHARE;
    for _ in 0..4 {
        let mut pairs: Vec<([f64; 2], [f64; 2])> = Vec::new();
        for l in trade.lines.iter().filter(|l| l.len() * tf.scale() >= min_len).take(20_000) {
            let (a, b) = (tf.apply(l.a), tf.apply(l.b));
            let len = (b[0] - a[0]).hypot(b[1] - a[1]);
            if !(len.is_finite() && len > 0.0) {
                continue;
            }
            let dir = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
            for f in [0.2, 0.5, 0.8] {
                let p = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
                if let Some(q) = arch.index.nearest(p, dir, arch.tol) {
                    pairs.push(([l.a[0] + (l.b[0] - l.a[0]) * f, l.a[1] + (l.b[1] - l.a[1]) * f], q));
                }
            }
        }
        if pairs.len() < 6 {
            break;
        }
        match Similarity::fit(&pairs) {
            Some(n) if (n.scale() / tf.scale() - 1.0).abs() < 0.05 => tf = n,
            _ => break,
        }
    }
    tf
}

/// Line `sh` (a trade plan of `d`) up on its level's architectural plan `arch` (none: the level
/// has none). `placed`: where its sheet placement puts it (same size as the A plan), if anywhere.
pub(crate) fn register(d: &Drawing, sh: &SheetRef, arch: Option<&ArchPlan>, placed: Option<Similarity>, upf: f64) -> Registration {
    let mut r = Registration {
        sheet: sh.number.clone(),
        discipline: sh.discipline.clone(),
        level: sh.level.clone(),
        arch_sheet: arch.map(|a| a.sheet.clone()),
        registered: false,
        method: "none",
        matched_pct: 0.0,
        transform: placed.unwrap_or_else(Similarity::identity),
        background: Vec::new(),
        background_lines: 0,
        residual_lines: 0,
        note: None,
    };
    let trade = sheet_geom(d, sh);
    r.residual_lines = trade.lines.len();
    let Some(arch) = arch else {
        r.note = Some(if sh.level.trim().is_empty() {
            format!("{}: its level is not known, so it has no architectural plan to line up on", sh.number)
        } else {
            format!("{}: no architectural floor plan of level {} in the set", sh.number, sh.level)
        });
        return r;
    };
    let mut candidates: Vec<(&'static str, Similarity)> = Vec::new();
    if let Some(p) = placed {
        candidates.push(("sheet", p));
    }
    if let Some((tf, _, grid_like)) = by_labels(&trade, arch) {
        candidates.push((if grid_like { "grids" } else { "tags" }, refine(&trade, arch, tf)));
    }
    for (method, tf) in candidates {
        let m = matched(&trade, arch, &tf);
        if m > r.matched_pct + 1e-9 {
            r.matched_pct = m;
            r.method = method;
            r.transform = tf;
        }
    }
    r.registered = r.matched_pct >= MIN_MATCHED;
    if !r.registered {
        r.note = Some(format!(
            "{}: only {:.0}% of its lines line up with {} (wrong level? another revision? no background?)",
            sh.number,
            r.matched_pct * 100.0,
            arch.sheet
        ));
        if r.method == "none" {
            r.method = if placed.is_some() { "sheet" } else { "none" };
        }
        return r;
    }
    // The background: objects lying on architectural lines, labels the A plan prints there.
    let tf = r.transform;
    let mut length: Vec<(f64, f64)> = vec![(0.0, 0.0); trade.objects.len()];
    for l in &trade.lines {
        let (a, b) = (tf.apply(l.a), tf.apply(l.b));
        let len = l.len();
        if let Some(x) = length.get_mut(l.object) {
            x.0 += len * arch.index.on_share(a, b, arch.tight);
            x.1 += len;
        }
    }
    // The sheet's frame (border, title block box) stays: readers size the sheet by its extents.
    let ext = trade.boxes.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, x| {
        [b[0].min(x[0]), b[1].min(x[1]), b[2].max(x[2]), b[3].max(x[3])]
    });
    let (ew, eh) = (ext[2] - ext[0], ext[3] - ext[1]);
    let frame = |i: usize| trade.boxes.get(i).is_some_and(|b| (b[2] - b[0]) >= FRAME_SHARE * ew || (b[3] - b[1]) >= FRAME_SHARE * eh);
    // A small symbol (a diffuser's square, a device) is the trade's even where a ceiling plan
    // shows it too: only the building's linework (and its dashes) is background.
    let min_size = SYMBOL_FT * upf.max(1e-9) / tf.scale().max(1e-9);
    let symbol =
        |i: usize| trade.pieces.get(i).is_some_and(|n| *n >= 2) && trade.boxes.get(i).is_some_and(|b| (b[2] - b[0]).max(b[3] - b[1]) < min_size);
    let gone: std::collections::BTreeSet<usize> = length
        .iter()
        .enumerate()
        .filter(|(i, (on, all))| *all > 0.0 && on / all >= BACKGROUND_SHARE && !frame(*i) && !symbol(*i))
        .map(|(i, _)| i)
        .collect();
    let mut background: Vec<Handle> = gone.iter().filter_map(|i| trade.objects.get(*i).copied()).collect();
    r.background_lines = trade.lines.iter().filter(|l| gone.contains(&l.object)).count();
    r.residual_lines = trade.lines.len() - r.background_lines;
    let near = arch.tol * 6.0;
    for (l, at, h) in &trade.texts {
        let p = tf.apply(*at);
        if arch.geom.texts.iter().any(|(al, aat, _)| al == l && (aat[0] - p[0]).hypot(aat[1] - p[1]) <= near) {
            background.push(*h);
        }
    }
    background.sort_unstable();
    background.dedup();
    r.background = background;
    r
}

/// Every trade plan of `sheets` (electrical, mechanical, plumbing, fire protection) lined up on
/// its level's architectural plan; each A plan is indexed once.
pub(crate) fn register_all(d: &Drawing, sheets: &[SheetRef], upf: f64, only: &dyn Fn(&SheetRef) -> bool) -> Vec<Registration> {
    let mut plans: HashMap<String, ArchPlan> = HashMap::new();
    let mut out = Vec::new();
    for sh in sheets.iter().filter(|s| is_trade_plan(s) && only(s)).take(500) {
        out.push(register_with(d, sh, sheets, upf, &mut plans));
    }
    out
}

/// Line one sheet up, the A plans indexed in `plans` (kept for the next sheet).
pub(crate) fn register_with(d: &Drawing, sh: &SheetRef, sheets: &[SheetRef], upf: f64, plans: &mut HashMap<String, ArchPlan>) -> Registration {
    let arch = arch_of(sh, sheets);
    let placed = super::printmep::sheet_placement(d, sh, sheets, upf);
    match arch {
        Some(a) => {
            let plan = plans.entry(a.number.clone()).or_insert_with(|| ArchPlan::new(d, a));
            register(d, sh, Some(plan), placed, upf)
        }
        None => register(d, sh, None, placed, upf),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, Space};
    use cadaddy_geom::Vec3;

    fn line(d: &mut Drawing, layer: &str, a: [f64; 2], b: [f64; 2]) -> Handle {
        let common = Common { layer: layer.into(), ..Common::default() };
        let kind = EntityKind::Line(cadaddy_doc::Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) });
        d.add(&Space::Model, common, kind).expect("add line")
    }

    fn text(d: &mut Drawing, s: &str, at: [f64; 2]) -> Handle {
        let t = cadaddy_doc::Text {
            insert: Vec3::new(at[0], at[1], 0.0),
            align_pt: None,
            height: 6.0,
            value: s.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: cadaddy_doc::HAlign::Left,
            valign: cadaddy_doc::VAlign::Baseline,
        };
        d.add(&Space::Model, Common { layer: "A-TEXT".into(), ..Common::default() }, EntityKind::Text(t)).expect("add text")
    }

    /// A made-up plan: walls of a row of rooms (inches), grid lines and labelled bubbles.
    fn plan(d: &mut Drawing, tf: &Similarity, layer: &str) -> Vec<Handle> {
        let mut hs = Vec::new();
        let w = |d: &mut Drawing, hs: &mut Vec<Handle>, a: [f64; 2], b: [f64; 2]| hs.push(line(d, layer, tf.apply(a), tf.apply(b)));
        for x in [0.0, 240.0, 480.0, 720.0, 960.0] {
            w(d, &mut hs, [x, 0.0], [x, 600.0]);
            w(d, &mut hs, [x + 6.0, 0.0], [x + 6.0, 600.0]);
        }
        for y in [0.0, 600.0] {
            w(d, &mut hs, [0.0, y], [966.0, y]);
            w(d, &mut hs, [0.0, y + 6.0], [966.0, y + 6.0]);
        }
        for (i, x) in [0.0, 480.0, 960.0].iter().enumerate() {
            w(d, &mut hs, [*x + 3.0, -120.0], [*x + 3.0, 720.0]);
            let at = tf.apply([*x + 3.0, -150.0]);
            hs.push(text(d, &format!("{}", i + 1), at));
        }
        for (i, y) in [0.0, 600.0].iter().enumerate() {
            w(d, &mut hs, [-120.0, *y + 3.0], [1080.0, *y + 3.0]);
            let at = tf.apply([-150.0, *y + 3.0]);
            hs.push(text(d, ["A", "B"][i], at));
        }
        hs
    }

    fn sheet(number: &str, discipline: &str, rect: [f64; 4], span: (u64, u64)) -> SheetRef {
        SheetRef {
            number: number.into(),
            level: "1".into(),
            discipline: discipline.into(),
            kind: "plan".into(),
            rect,
            span: Some(span),
            ..SheetRef::default()
        }
    }

    #[test]
    fn a_rotated_scaled_mechanical_plan_lines_up_and_its_background_goes() {
        let mut d = Drawing::default();
        let lo = d.handseed;
        plan(&mut d, &Similarity::identity(), "A-WALL");
        let mid = d.handseed;
        // The M sheet: the same plan at half scale, turned 90 degrees, far to the right; a duct on top.
        let at = Similarity { m: [0.0, 0.5], t: [5_000.0, 100.0] };
        let bg = plan(&mut d, &at, "M-BKGD");
        let duct = line(&mut d, "M-DUCT", at.apply([120.0, 300.0]), at.apply([840.0, 300.0]));
        // Its own border: the sheet's frame, never background.
        let border: Vec<Handle> = [
            ([4_100.0, -350.0], [5_150.0, -350.0]),
            ([5_150.0, -350.0], [5_150.0, 750.0]),
            ([5_150.0, 750.0], [4_100.0, 750.0]),
            ([4_100.0, 750.0], [4_100.0, -350.0]),
        ]
        .iter()
        .map(|(a, b)| line(&mut d, "BORDER", *a, *b))
        .collect();
        let hi = d.handseed;
        let a = sheet("A-101", "A", [-200.0, -200.0, 1_100.0, 800.0], (lo, mid));
        let m = sheet("M-101", "M", [4_000.0, -400.0, 5_200.0, 800.0], (mid, hi));
        let all = vec![a.clone(), m.clone()];
        let r = register_with(&d, &m, &all, 12.0, &mut HashMap::new());
        assert!(r.registered, "{:?}", r.to_json());
        assert!(r.matched_pct > 0.95, "{}", r.to_json());
        assert!((r.transform.scale() - 2.0).abs() < 1e-6, "{}", r.to_json());
        assert!((r.transform.rotation_deg() + 90.0).abs() < 1e-6, "{}", r.to_json());
        // Every background line and label goes, the duct stays.
        for h in &bg {
            assert!(r.background.contains(h), "{h:?} kept: {}", r.to_json());
        }
        assert!(!r.background.contains(&duct));
        assert!(border.iter().all(|h| !r.background.contains(h)), "the border stays");
        assert_eq!(r.residual_lines, 5, "the duct and the border: {}", r.to_json());
    }

    #[test]
    fn a_sheet_without_the_background_is_not_registered() {
        let mut d = Drawing::default();
        let lo = d.handseed;
        plan(&mut d, &Similarity::identity(), "A-WALL");
        let mid = d.handseed;
        // Only grid labels in common: the labels fit, the lines don't.
        let at = Similarity { m: [1.0, 0.0], t: [5_000.0, 0.0] };
        for (i, x) in [0.0, 480.0, 960.0].iter().enumerate() {
            let p = at.apply([*x + 3.0, -150.0]);
            text(&mut d, &format!("{}", i + 1), p);
        }
        for k in 0..6 {
            let y = 50.0 + 90.0 * f64::from(k);
            line(&mut d, "M-DUCT", at.apply([30.0, y]), at.apply([930.0, y]));
        }
        let hi = d.handseed;
        let a = sheet("A-101", "A", [-200.0, -200.0, 1_100.0, 800.0], (lo, mid));
        let m = sheet("M-101", "M", [4_700.0, -400.0, 6_200.0, 800.0], (mid, hi));
        let r = register_with(&d, &m, &[a, m.clone()], 12.0, &mut HashMap::new());
        assert!(!r.registered, "{}", r.to_json());
        assert!(r.matched_pct < MIN_MATCHED);
        assert!(r.background.is_empty());
        assert!(r.note.as_deref().is_some_and(|n| n.contains("line up")), "{:?}", r.note);
    }

    #[test]
    fn similarity_fits_and_refuses_nonsense() {
        let tf = Similarity { m: [0.0, 2.0], t: [10.0, -5.0] };
        let pts = [[0.0, 0.0], [10.0, 0.0], [3.0, 7.0]];
        let pairs: Vec<_> = pts.iter().map(|p| (*p, tf.apply(*p))).collect();
        let f = Similarity::fit(&pairs).expect("fit");
        for p in pts {
            let (a, b) = (f.apply(p), tf.apply(p));
            assert!((a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
        }
        assert!(Similarity::from_two([0.0, 0.0], [0.0, 0.0], [1.0, 1.0], [2.0, 2.0]).is_none());
        assert!(Similarity::from_two([0.0, 0.0], [1.0, 0.0], [0.0, 0.0], [f64::NAN, 0.0]).is_none());
        assert!(Similarity::fit(&[([0.0, 0.0], [1.0, 1.0])]).is_none());
        let (lo, hi) = Similarity { m: [0.0, 1.0], t: [0.0, 0.0] }.apply_box([0.0, 0.0], [2.0, 1.0]);
        assert!((lo[0] + 1.0).abs() < 1e-9 && hi[1] > 1.99);
    }
}
