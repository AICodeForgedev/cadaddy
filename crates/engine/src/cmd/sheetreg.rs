//! A trade sheet laid onto its level's plan by the linework both draw (Q6284).
//!
//! A trade plan (electrical, mechanical, plumbing) is drawn over the floor plan's background: its
//! walls are the floor plan's walls. When its tags are too few to fit it by, or a sheet of the
//! same size may be another part of the site (a set of several buildings prints each building's
//! units on sheets of one size, in one title block), the linework says where it goes:
//! - a sheet's *template* (its border and title block) is what most sheets of its size draw in the
//!   same place on the page; it says nothing of where a plan is and is left out of every
//!   comparison ([`SheetLines`]);
//! - a placement by the sheet frame holds when the sheet's own linework lies on the plan's
//!   ([`SheetLines::frame_share`]): a sheet of another building in the same frame doesn't;
//! - a sheet nothing else places is fitted onto its level's plan by its linework, at the ratio
//!   of the scales the two sheets' plans print at (one to one when both came in at their
//!   printed scales; a quarter for a trade's 1/4" roof plan onto a roof plan read at a tenth of
//!   its scale), with the fit's score and its margin over the next best placement recorded
//!   ([`SheetLines::fit`]);
//! - a sheet whose views name another part of the site than the plan's (`BUILDING 3 - UNIT 8`
//!   against `BUILDING 2 - UNIT 6`: [`site_parts`]) is never laid on it, however alike its
//!   linework (the units of a complex repeat a few typical layouts, so another building's unit
//!   plans in the same frame lie three quarters on the plan's lines).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cadaddy_doc::{Drawing, Entity};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::tagid::Sim;

/// Sheets of one size at least this many before what they share is taken for their template.
const TEMPLATE_MIN_SHEETS: usize = 4;
/// The share of a size's sheets that must draw a spot of the page for it to be template.
const TEMPLATE_SHARE: f64 = 0.7;
/// Most line points kept for one sheet.
const MAX_SHEET_POINTS: usize = 200_000;
/// Fewest points (after the template is left out) on each side for a judgement or a fit.
pub(crate) const MIN_POINTS: usize = 200;
/// A frame placement holds when this share of each side's linework lies on the other's: the
/// sheet's lines on the plan's, and the plan's lines (under the sheet) on the sheet's. One way
/// alone is no test: a dense electrical plan covers half of any plan by chance (a plan of another
/// building in the same title block lay half on it), and a sparse plan lies on a dense one.
pub(crate) const FRAME_SHARE: f64 = 0.4;
/// A sheet this much wider than the plan it's laid on can't be weighed whole
/// ([`super::grids::linework_fit_part`] won't try).
const MAX_SHEET_OVER_PLAN: f64 = 1.2;

/// A sheet drawn up to this share off the scale it prints (an engineer's background plotted a few
/// percent off the architect's, the same note on both) is still placed, at the scale it is drawn
/// at, when its fit is decisive: [`OFF_SCALE_F1`] of their lines on each other and
/// [`OFF_SCALE_MARGIN`] over the next best, its best scale inside the scales tried.
const MAX_OFF_SCALE: f64 = 0.07;
const OFF_SCALE_F1: f64 = 0.85;
const OFF_SCALE_MARGIN: f64 = 0.06;
/// A fit at the ratio whose best scale is this far off it is weighed against the off-scale fit.
const OFF_SCALE_SUSPECT: f64 = 0.015;
/// Scales whose fit is this close to the best one's are among the best.
const PLATEAU: f64 = 0.02;

/// Most blocks of a too-wide sheet's lines tried as its plan.
const MAX_BLOCKS_TRIED: usize = 3;

/// How much of two sheets' linework must lie on each other, both ways, for one to be placed
/// through the other: the same consultant's background in the same spot agrees nearly line for
/// line, where a sheet laid on the architect's plan differs by the architect's own detail.
pub(crate) const SIBLING_SHARE: f64 = 0.8;
/// A linework fit holds with this F1 of the two drawings' lines on each other (a whole sheet's
/// walls are the plan's walls: another building's plan of rooms on the same grid of partitions
/// lies on it two thirds of the way)...
pub(crate) const FIT_F1: f64 = 0.75;
/// ...and this much over the best placement 10' or more away (a run of identical units).
pub(crate) const FIT_MARGIN: f64 = 0.03;

/// Each sheet's linework (points every 2'), its template left out.
pub(crate) struct SheetLines {
    rects: Vec<[f64; 4]>,
    pts: Vec<Vec<Vec2>>,
    /// Whether the sheet's template was known (else its points are all its lines).
    stripped: Vec<bool>,
    k_in: f64,
}

/// What a sheet's linework said of a placement, for the job's report.
#[derive(Clone, Debug)]
pub(crate) struct LineworkFit {
    pub sim: Sim,
    /// F1 of the two drawings' lines on each other once placed.
    pub score: f64,
    /// Over the next best placement 10' or more away.
    pub margin: f64,
    /// Placed at the scale its linework is drawn at, this share of the ratio its printed scale
    /// gives (its drawing a few percent off its scale note); `None` at the ratio.
    pub off_scale: Option<f64>,
}

impl SheetLines {
    /// The linework of the sheets `rects` (drawing units, `k_in` units per inch).
    pub(crate) fn new(d: &Drawing, rects: &[[f64; 4]], k_in: f64) -> Self {
        let k_in = if k_in.is_finite() && k_in > 0.0 { k_in } else { 1.0 };
        let step = 24.0 * k_in;
        // Each object onto the sheet holding its middle (sheets lie side by side: by their left
        // edges, then the few that start before it).
        let mut order: Vec<usize> = (0..rects.len()).collect();
        order.sort_by(|a, b| rects[*a][0].total_cmp(&rects[*b][0]));
        let lefts: Vec<f64> = order.iter().map(|i| rects[*i][0]).collect();
        let mut on: Vec<Vec<&Arc<Entity>>> = vec![Vec::new(); rects.len()];
        for e in super::bspec::plan_objects(d) {
            if !matches!(e.kind, cadaddy_doc::EntityKind::Line(_) | cadaddy_doc::EntityKind::LwPolyline(_)) {
                continue;
            }
            let Some(c) = super::bspec::centre(d, e) else { continue };
            let upto = lefts.partition_point(|x| *x <= c.x);
            let hit = order[..upto]
                .iter()
                .rev()
                .take(64)
                .copied()
                .filter(|i| inside(&rects[*i], c))
                .min_by(|a, b| area(&rects[*a]).total_cmp(&area(&rects[*b])));
            if let Some(i) = hit
                && let Some(v) = on.get_mut(i)
            {
                v.push(e);
            }
        }
        let mut pts: Vec<Vec<Vec2>> = on
            .iter()
            .map(|es| {
                let all = super::grids::line_samples(es, step);
                let stride = (all.len() / MAX_SHEET_POINTS).max(1);
                all.into_iter().step_by(stride).collect()
            })
            .collect();
        drop(on);
        // The sheets of each size, and the spots of the page most of them draw.
        let cell = step;
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (i, r) in rects.iter().enumerate() {
            let (w, h) = (r[2] - r[0], r[3] - r[1]);
            let same = |j: &usize| {
                let (w1, h1) = (rects[*j][2] - rects[*j][0], rects[*j][3] - rects[*j][1]);
                (w - w1).abs() <= 0.01 * w1.abs() && (h - h1).abs() <= 0.01 * h1.abs()
            };
            match groups.iter_mut().find(|g| g.first().is_some_and(same)) {
                Some(g) => g.push(i),
                None => groups.push(vec![i]),
            }
        }
        let mut stripped = vec![false; rects.len()];
        for g in groups.iter().filter(|g| g.len() >= TEMPLATE_MIN_SHEETS) {
            let rel = |i: usize, p: &Vec2| (((p.x - rects[i][0]) / cell).floor() as i64, ((p.y - rects[i][1]) / cell).floor() as i64);
            let mut count: HashMap<(i64, i64), usize> = HashMap::new();
            for &i in g {
                let cells: HashSet<(i64, i64)> = pts[i].iter().map(|p| rel(i, p)).collect();
                for c in cells {
                    *count.entry(c).or_default() += 1;
                }
            }
            let need = ((g.len() as f64 * TEMPLATE_SHARE).ceil() as usize).max(3);
            let template: HashSet<(i64, i64)> = count.into_iter().filter(|(_, n)| *n >= need).map(|(c, _)| c).collect();
            // A spot next to the template is its (lines a little off from sheet to sheet).
            let near = |c: (i64, i64)| (-1..=1).any(|dx| (-1..=1).any(|dy| template.contains(&(c.0 + dx, c.1 + dy))));
            for &i in g {
                pts[i].retain(|p| !near(rel(i, p)));
                stripped[i] = true;
            }
        }
        SheetLines { rects: rects.to_vec(), pts, stripped, k_in }
    }

    /// Sheet `i`'s linework inside `region` (all of it with none).
    pub(crate) fn points(&self, i: usize, region: Option<&[f64; 4]>) -> Vec<Vec2> {
        let Some(all) = self.pts.get(i) else { return Vec::new() };
        match region {
            Some(r) => all.iter().copied().filter(|p| inside(r, *p)).collect(),
            None => all.clone(),
        }
    }

    /// Whether sheet `i`'s template was left out of its linework.
    pub(crate) fn stripped(&self, i: usize) -> bool {
        self.stripped.get(i).copied().unwrap_or(false)
    }

    /// The sheet holding `p`.
    pub(crate) fn sheet_at(&self, p: [f64; 2]) -> Option<usize> {
        let c = Vec2::new(p[0], p[1]);
        (0..self.rects.len()).filter(|i| inside(&self.rects[*i], c)).min_by(|a, b| area(&self.rects[*a]).total_cmp(&area(&self.rects[*b])))
    }

    /// How well sheet `i` placed by `m` lies on `tgt` (its level's plan linework): the share of
    /// its lines on the plan's, and the share of the plan's lines (under it) on its.
    /// `None` when either side has too few lines to say, or the sheet's template is unknown (its
    /// border and title block would agree with any sheet in the same frame).
    pub(crate) fn frame_share(&self, i: usize, m: &Sim, tgt: &[Vec2]) -> Option<(f64, f64)> {
        let src = self.pts.get(i)?;
        if !self.stripped(i) || src.len() < MIN_POINTS || tgt.len() < MIN_POINTS {
            return None;
        }
        let placed: Vec<Vec2> = src.iter().map(|p| m.apply([p.x, p.y])).map(|q| Vec2::new(q[0], q[1])).collect();
        let on_plan = super::grids::overlay_share(&placed, tgt, self.k_in);
        let b = cadaddy_geom::Bounds2::from_points(placed.iter().copied());
        let under: Vec<Vec2> = tgt.iter().copied().filter(|p| b.contains(*p)).collect();
        let on_sheet = if under.len() < MIN_POINTS { 0.0 } else { super::grids::overlay_share(&under, &placed, self.k_in) };
        Some((on_plan, on_sheet))
    }

    /// Sheet `i` placed through sheet `j` that `j_fit` already places: by their frames, if the
    /// sheets are one size and `i`'s lines lie on `j`'s both ways. The placement, and the two shares.
    pub(crate) fn through_sibling(&self, i: usize, j: usize, j_fit: &Sim) -> Option<(Sim, f64, f64)> {
        let (a, b) = (self.rects.get(i)?, self.rects.get(j)?);
        let alike = |p: f64, q: f64| (p - q).abs() <= 0.01 * q.abs();
        if !alike(a[2] - a[0], b[2] - b[0]) || !alike(a[3] - a[1], b[3] - b[1]) {
            return None;
        }
        let frame = Sim::scale_offset(1.0, [b[0] - a[0], b[1] - a[1]]);
        let (on, under) = self.frame_share(i, &frame, self.pts.get(j)?)?;
        // Frame onto `j`, then where `j` goes: `j`'s scale about the offset its fit gives the
        // frame's (a fit at the ratio of the sheets' printed scales too; never a turned one).
        let start = j_fit.apply([frame.tx, frame.ty]);
        let k = j_fit.scale();
        (on.min(under) >= SIBLING_SHARE && j_fit.b.abs() <= 1e-9 * k.max(1.0) && k.is_finite() && k > 0.0)
            .then(|| (Sim::scale_offset(k, start), on, under))
    }

    /// Why [`Self::fit`] found none, for the log: the best placement it weighed at `ratio`.
    pub(crate) fn best_tried(&self, i: usize, tgt: &[Vec2], ratio: f64) -> String {
        let (Some(src), true) = (self.pts.get(i), self.stripped(i)) else { return "its template could not be told from its drawing".into() };
        if src.len() < MIN_POINTS || tgt.len() < MIN_POINTS {
            return format!("{} sheet points, {} plan points", src.len(), tgt.len());
        }
        let r = fit_ratio(ratio);
        let at_ratio = super::grids::linework_fit_part(src, tgt, self.k_in, 0.97 * r, 1.03 * r).map_or_else(
            || "nothing to weigh".to_string(),
            |(s, _, score, margin)| format!("best scale {s:.3}, {:.0}% on the plan, {:.0} points over the next", score * 100.0, margin * 100.0),
        );
        match self.off_scale(src, tgt, r) {
            Err(why) => format!("{at_ratio}; {why}"),
            Ok(_) => at_ratio,
        }
    }

    /// Sheet `i` fitted onto `tgt` (its level's plan linework) by its linework at `ratio` (the
    /// plan's drawing units per foot over the sheet's, from the scales their plans print: 1 when
    /// both came in at them) within 2.5% (a fit off that is its sampling's error, so at `ratio`
    /// about its middle; one at the end of the scales tried is no fit), the whole floor or a part
    /// of it (an enlarged plan). Never scaled to fit without that ratio, but for a sheet drawn a
    /// few percent off its printed scale whose fit is decisive ([`MAX_OFF_SCALE`]): placed at the
    /// scale it is drawn at. `None` without a fit that holds ([`FIT_F1`], [`FIT_MARGIN`]).
    pub(crate) fn fit(&self, i: usize, tgt: &[Vec2], ratio: f64) -> Option<LineworkFit> {
        let src = self.pts.get(i)?;
        if !self.stripped(i) || src.len() < MIN_POINTS || tgt.len() < MIN_POINTS {
            return None;
        }
        let r = fit_ratio(ratio);
        // A fit whose best scale is well off the ratio may be a sheet drawn off its scale: the
        // off-scale fit, when it is the better one.
        let at_ratio = match self.fit_points_at(src, tgt, r) {
            Some((f, s)) if (s / r - 1.0).abs() <= OFF_SCALE_SUSPECT => Some(f),
            Some((f, _)) => Some(self.fit_off_scale(src, tgt, r).filter(|o| o.score >= f.score).unwrap_or(f)),
            None => self.fit_off_scale(src, tgt, r),
        };
        at_ratio.or_else(|| {
            // Schedules and notes beside the plan can leave the sheet wider than the plan it is
            // laid on (at the ratio it is laid on at), so it can't be weighed whole: its biggest
            // block of lines is its plan.
            let span = |p: &[Vec2]| {
                let b = cadaddy_geom::Bounds2::from_points(p.iter().copied());
                b.width().max(b.height())
            };
            if r * span(src) <= MAX_SHEET_OVER_PLAN * span(tgt) {
                return None;
            }
            // A schedule's table can outweigh the plan: the biggest few are each tried, and only
            // a plan passes the fit.
            let mut rest = src.clone();
            for _ in 0..MAX_BLOCKS_TRIED {
                let block = super::grids::largest_block(&rest, 48.0 * self.k_in);
                if block.len() < MIN_POINTS || block.len() >= src.len() {
                    return None;
                }
                if let Some(f) = self.fit_points(&block, tgt, r) {
                    return Some(f);
                }
                let taken: std::collections::HashSet<(u64, u64)> = block.iter().map(|p| (p.x.to_bits(), p.y.to_bits())).collect();
                rest.retain(|p| !taken.contains(&(p.x.to_bits(), p.y.to_bits())));
            }
            None
        })
    }

    /// [`Self::fit`] of the points `src` at the ratio `r`.
    fn fit_points(&self, src: &[Vec2], tgt: &[Vec2], r: f64) -> Option<LineworkFit> {
        self.fit_points_at(src, tgt, r).map(|(f, _)| f)
    }

    /// [`Self::fit_points`] and the scale its best placement was weighed at.
    fn fit_points_at(&self, src: &[Vec2], tgt: &[Vec2], r: f64) -> Option<(LineworkFit, f64)> {
        // Each scale proposes its best few placements, so the margin is over a real runner-up.
        let fit = super::grids::linework_fit_part(src, tgt, self.k_in, 0.97 * r, 1.03 * r);
        let holds = |f: &(f64, Vec2, f64, f64)| f.2 >= FIT_F1 && f.3 >= FIT_MARGIN;
        // A small building's lines fit nearly as well over a range of scales (a few percent is
        // under the fit's foot at its walls), and a tie goes to the first scale tried: its best
        // scale off the ratio, the fit at the ratio itself when that is among the best.
        let fit = match fit {
            Some(f) if holds(&f) && (f.0 / r - 1.0).abs() > 0.025 => super::grids::linework_fit_part(src, tgt, self.k_in, r, r)
                .filter(|g| holds(g) && g.2 >= f.2 - PLATEAU && (g.0 / r - 1.0).abs() <= 0.025)
                .or(Some(f)),
            other => other,
        };
        let (s, t, score, margin) = fit.filter(|f| holds(f) && (f.0 / r - 1.0).abs() <= 0.025)?;
        let mid = src.iter().fold(Vec2::new(0.0, 0.0), |a, q| a + *q) * (1.0 / src.len().max(1) as f64);
        let sim = Sim::scale_offset(r, [t.x + mid.x * (s - r), t.y + mid.y * (s - r)]);
        sim.is_finite().then_some((LineworkFit { sim, score, margin, off_scale: None }, s))
    }

    /// [`Self::fit`] of the points `src` drawn a few percent off the ratio `r` (up to
    /// [`MAX_OFF_SCALE`]): each scale a percent apart weighed on its own, the scale it is drawn at
    /// the middle of the run of scales that fit best (a fit holds over a range of scales, wider the
    /// smaller the plan), placed there when that fit is decisive and the run is inside the scales
    /// tried and away from the ratio.
    fn fit_off_scale(&self, src: &[Vec2], tgt: &[Vec2], r: f64) -> Option<LineworkFit> {
        self.off_scale(src, tgt, r).ok()
    }

    /// [`Self::fit_off_scale`], or why there is none (each scale weighed and what was missing).
    fn off_scale(&self, src: &[Vec2], tgt: &[Vec2], r: f64) -> Result<LineworkFit, String> {
        let steps = (2.0 * MAX_OFF_SCALE / 0.01).round() as i32;
        let profile: Vec<(f64, f64, f64)> = (0..=steps)
            .filter_map(|k| {
                let sk = r * (1.0 - MAX_OFF_SCALE + 0.01 * f64::from(k));
                super::grids::linework_fit_part(src, tgt, self.k_in, sk, sk).map(|(_, _, score, margin)| (sk, score, margin))
            })
            .collect();
        let weighed = || profile.iter().map(|(k, f, m)| format!("{:.2}: {:.0}%/{:.0}", k / r, f * 100.0, m * 100.0)).collect::<Vec<_>>().join(", ");
        let best = profile.iter().map(|p| p.1).fold(0.0, f64::max);
        if best < OFF_SCALE_F1 {
            return Err(format!("drawn off its scale: none fits well enough ({})", weighed()));
        }
        let top: Vec<usize> = (0..profile.len()).filter(|i| profile.get(*i).is_some_and(|p| p.1 >= best - PLATEAU)).collect();
        let (Some(&first), Some(&last)) = (top.first(), top.last()) else { return Err("drawn off its scale: nothing weighed".into()) };
        // One run of them, not at the ends of the scales tried.
        if last - first + 1 != top.len() || first == 0 || last + 1 >= profile.len() {
            return Err(format!("drawn off its scale: the best scales are not one run inside those tried ({})", weighed()));
        }
        let (Some(lo), Some(hi)) = (profile.get(first), profile.get(last)) else { return Err("drawn off its scale: nothing weighed".into()) };
        let drawn = (lo.0 * hi.0).sqrt();
        let off = drawn / r;
        if (off - 1.0).abs() <= OFF_SCALE_SUSPECT {
            return Err(format!("drawn off its scale: it fits best at its printed scale ({})", weighed()));
        }
        let (s, t, score, margin) =
            super::grids::linework_fit_part(src, tgt, self.k_in, drawn, drawn).ok_or_else(|| format!("drawn off its scale: no fit at {:.3}", off))?;
        if score < OFF_SCALE_F1 || margin < OFF_SCALE_MARGIN {
            return Err(format!("drawn off its scale: at {:.3} {:.0}% on the plan, {:.0} points over the next ({})", off, score * 100.0, margin * 100.0, weighed()));
        }
        let sim = Sim::scale_offset(s, [t.x, t.y]);
        if !sim.is_finite() {
            return Err("drawn off its scale: no finite placement".into());
        }
        Ok(LineworkFit { sim, score, margin, off_scale: Some(s / r) })
    }
}

/// The ratios of printed scales a fit is tried at (a sheet a fiftieth of its plan's size, or fifty
/// times it, is a misread note): outside them, and for one that isn't a number, one to one.
const MIN_FIT_RATIO: f64 = 0.02;
const MAX_FIT_RATIO: f64 = 50.0;

fn fit_ratio(ratio: f64) -> f64 {
    if ratio.is_finite() && (MIN_FIT_RATIO..=MAX_FIT_RATIO).contains(&ratio) { ratio } else { 1.0 }
}

/// The ratio a sheet is fitted onto its level's plan at: the plan's drawing units per foot over
/// the sheet's (each the scale its sheet came in at times the scale its plans print), 1 when
/// either is unknown.
pub(crate) fn scale_ratio(plan_upf: Option<f64>, sheet_upf: Option<f64>) -> f64 {
    match (plan_upf, sheet_upf) {
        (Some(p), Some(q)) if p.is_finite() && q.is_finite() && p > 0.0 && q > 0.0 => fit_ratio(p / q),
        _ => 1.0,
    }
}

/// Most texts looked at for the parts of the site a sheet names.
const MAX_PART_TEXTS: usize = 200_000;

/// The parts of the site the lettering inside `rect` names: each text that starts with one of
/// the rules data's site-part words (`sheets.site_part_words`: BUILDING, BLDG, UNIT…) and an
/// identifier (a number, or one letter) gives its (kind, identifier), and so does every such pair
/// after it in that text (`BUILDING 2 - UNIT 7 -`). A note that only mentions a word
/// (`UNIT HEATERS`, `AREA OF WORK`) names nothing.
pub(crate) fn site_parts(texts: &[cadaddy_mepfix::sheet_facts::TextItem], rect: &[f64; 4]) -> std::collections::BTreeSet<(String, String)> {
    let words = cadaddy_mepfix::wordrules::pairs("sheets.site_part_words");
    let kind_of = |w: &str| words.iter().find(|(k, _)| w == *k).map(|(_, v)| (*v).to_string());
    let ident = |w: &str| {
        let ok = w.chars().all(|c| c.is_ascii_alphanumeric())
            && (w.chars().any(|c| c.is_ascii_digit()) || (w.len() == 1 && w.chars().all(|c| c.is_ascii_alphabetic())));
        (ok && !w.is_empty() && w.len() <= 8).then(|| w.to_string())
    };
    let mut out = std::collections::BTreeSet::new();
    for t in texts.iter().take(MAX_PART_TEXTS).filter(|t| inside(rect, t.at)) {
        let up = t.value.to_ascii_uppercase();
        let tokens: Vec<&str> = up
            .split(|c: char| c.is_whitespace() || c == '-' || c == ',' || c == ':')
            .map(|w| w.trim_matches('.'))
            .filter(|w| !w.is_empty())
            .collect();
        if tokens.first().and_then(|w| kind_of(w)).is_none() {
            continue;
        }
        for pair in tokens.windows(2) {
            if let (Some(k), Some(id)) = (pair.first().and_then(|w| kind_of(w)), pair.get(1).and_then(|w| ident(w))) {
                out.insert((k, id));
            }
        }
    }
    out
}

/// Whether two sheets' parts of the site ([`site_parts`]) are other parts: of a kind both name,
/// they name none in common. Either naming none of a kind says nothing of it.
pub(crate) fn other_parts(a: &std::collections::BTreeSet<(String, String)>, b: &std::collections::BTreeSet<(String, String)>) -> Option<String> {
    let kinds: std::collections::BTreeSet<&str> = a.iter().map(|(k, _)| k.as_str()).filter(|k| b.iter().any(|(j, _)| j == k)).collect();
    let ids =
        |s: &std::collections::BTreeSet<(String, String)>, k: &str| s.iter().filter(|(j, _)| j == k).map(|(_, v)| v.clone()).collect::<Vec<_>>();
    kinds
        .into_iter()
        .find(|k| ids(a, k).iter().all(|v| !ids(b, k).contains(v)))
        .map(|k| format!("{k} {} against {k} {}", ids(a, k).join("/"), ids(b, k).join("/")))
}

fn inside(r: &[f64; 4], p: Vec2) -> bool {
    p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3]
}

fn area(r: &[f64; 4]) -> f64 {
    (r[2] - r[0]).abs() * (r[3] - r[1]).abs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_mepfix::sheet_facts::TextItem;
    use std::collections::BTreeSet;

    fn t(v: &str, x: f64, y: f64) -> TextItem {
        TextItem { value: v.into(), at: Vec2::new(x, y), height: 6.0, rotation: 0.0 }
    }

    fn set(pairs: &[(&str, &str)]) -> BTreeSet<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// The same drawing on two sheets side by side, and a third of the same size drawing another.
    fn three_sheets() -> SheetLines {
        // Upright lines 150 apart: a drawing shifted half way lies clear of the overlay's reach.
        let drawing = |x0: f64, phase: f64| -> Vec<Vec2> {
            (0..600).map(f64::from).map(|n| Vec2::new(x0 + phase + 150.0 * (n % 6.0), 5.0 * (n / 6.0).floor())).collect()
        };
        SheetLines {
            rects: vec![[0.0, 0.0, 1000.0, 600.0], [2000.0, 0.0, 3000.0, 600.0], [4000.0, 0.0, 5000.0, 600.0]],
            pts: vec![drawing(0.0, 20.0), drawing(2000.0, 20.0), drawing(4000.0, 95.0)],
            stripped: vec![true, true, true],
            k_in: 1.0,
        }
    }

    #[test]
    fn a_sheet_is_placed_through_one_that_draws_the_same_background() {
        let lines = three_sheets();
        let placed = Sim::scale_offset(1.0, [500.0, -7.0]);
        let (m, on, under) = lines.through_sibling(0, 1, &placed).expect("the same drawing, frame on frame");
        assert!(on > 0.9 && under > 0.9, "{on} {under}");
        // Onto sheet 1 by the frame (+2000), then where sheet 1 goes.
        assert_eq!(m.apply([10.0, 10.0]), [2510.0, 3.0]);
        assert!(lines.through_sibling(0, 2, &placed).is_none(), "another drawing is no sibling");
    }

    /// A made-up floor's walls (inches, 150' by 100'), sampled every 6": an outline and
    /// partitions at uneven spacings, so it lies on itself in one place only.
    fn floor(scale: f64, at: [f64; 2]) -> Vec<Vec2> {
        let scale = 2.5 * scale;
        let walls: [[f64; 4]; 9] = [
            [0.0, 0.0, 720.0, 0.0],
            [720.0, 0.0, 720.0, 480.0],
            [720.0, 480.0, 0.0, 480.0],
            [0.0, 480.0, 0.0, 0.0],
            [170.0, 0.0, 170.0, 300.0],
            [0.0, 300.0, 410.0, 300.0],
            [410.0, 120.0, 410.0, 480.0],
            [530.0, 0.0, 530.0, 210.0],
            [530.0, 210.0, 720.0, 210.0],
        ];
        let mut out = Vec::new();
        for [x0, y0, x1, y1] in walls {
            let n = ((x1 - x0).hypot(y1 - y0) / 6.0).ceil().max(1.0) as usize;
            for k in 0..=n {
                let t = k as f64 / n as f64;
                out.push(Vec2::new(at[0] + scale * (x0 + t * (x1 - x0)), at[1] + scale * (y0 + t * (y1 - y0))));
            }
        }
        out
    }

    /// A small building's sheet at its printed scale whose lines fit about as well a few percent
    /// either way (each wall within the fit's foot) is placed at that scale, not refused for a
    /// best scale that is only the first of the ties.
    #[test]
    fn a_small_buildings_sheet_whose_fit_is_flat_over_scales_is_placed_at_its_printed_scale() {
        let plan = floor(0.15, [100.0, 50.0]);
        let lines = SheetLines {
            rects: vec![[0.0, 0.0, 400.0, 300.0], [5000.0, 0.0, 5400.0, 300.0]],
            pts: vec![plan.clone(), floor(0.15, [5100.0, 60.0])],
            stripped: vec![true, true],
            k_in: 1.0,
        };
        let f = lines.fit(1, &plan, 1.0).unwrap_or_else(|| panic!("no fit: {}", lines.best_tried(1, &plan, 1.0)));
        assert!((f.sim.scale() - 1.0).abs() < 1e-9 && f.off_scale.is_none(), "{} {:?}", f.sim.scale(), f.off_scale);
        let q = f.sim.apply([5100.0, 60.0]);
        assert!((q[0] - 100.0).abs() <= 18.0 && (q[1] - 50.0).abs() <= 18.0, "{q:?}");
    }

    /// A trade's sheet whose drawing is a few percent smaller (or larger) than the architect's at
    /// the same printed scale (its background plotted off) is placed at the scale it is drawn at,
    /// and says so; one drawn far off it, or one whose fit is weak, is not.
    #[test]
    fn a_sheet_drawn_a_few_percent_off_its_printed_scale_is_placed_at_its_drawn_scale() {
        let plan = floor(1.0, [100.0, 50.0]);
        for (drawn, placed) in [(0.96, true), (1.045, true), (0.85, false)] {
            let lines = SheetLines {
                rects: vec![[0.0, 0.0, 2000.0, 1400.0], [5000.0, 0.0, 9000.0, 2800.0]],
                pts: vec![plan.clone(), floor(1.0 / drawn, [5200.0, 100.0])],
                stripped: vec![true, true],
                k_in: 1.0,
            };
            let f = lines.fit(1, &plan, 1.0);
            assert_eq!(f.is_some(), placed, "drawn at {drawn}: {}", lines.best_tried(1, &plan, 1.0));
            let Some(f) = f else { continue };
            // (to the fit's steps of a percent)
            assert!((f.sim.scale() / drawn - 1.0).abs() <= 0.02, "placed at the scale it is drawn at: {} for {drawn}", f.sim.scale());
            assert!(f.off_scale.is_some_and(|k| (k / drawn - 1.0).abs() <= 0.02), "{:?}", f.off_scale);
            // The trade sheet's corner of the floor lands on the plan's.
            let q = f.sim.apply([5200.0, 100.0]);
            assert!((q[0] - 100.0).abs() <= 18.0 && (q[1] - 50.0).abs() <= 18.0, "{q:?}");
        }
        // At its printed scale: placed at the ratio, nothing off.
        let lines = SheetLines {
            rects: vec![[0.0, 0.0, 2000.0, 1400.0], [5000.0, 0.0, 9000.0, 2800.0]],
            pts: vec![plan.clone(), floor(1.0, [5200.0, 100.0])],
            stripped: vec![true, true],
            k_in: 1.0,
        };
        assert!(lines.fit(1, &plan, 1.0).is_some_and(|f| f.off_scale.is_none()));
    }

    /// Q6396: a trade's plan whose sheet came in at twice the scale of the architect's plan (its
    /// plans print 1/4" where the architect's print 1/8", and the architect's was read at that)
    /// fits onto it at the ratio of their printed scales, about its middle; one to one it has
    /// nothing to weigh (a sheet twice the plan's size), as every such sheet had before.
    #[test]
    fn a_sheet_fits_onto_a_plan_at_the_ratio_of_their_scales() {
        let plan = floor(1.0, [100.0, 50.0]);
        let lines = SheetLines {
            rects: vec![[0.0, 0.0, 2000.0, 1400.0], [5000.0, 0.0, 9000.0, 2800.0]],
            pts: vec![plan.clone(), floor(2.0, [5200.0, 100.0])],
            stripped: vec![true, true],
            k_in: 1.0,
        };
        assert!(lines.fit(1, &plan, 1.0).is_none(), "one to one: no fit");
        let f = lines.fit(1, &plan, 0.5).expect("a fit at the ratio");
        assert!((f.sim.scale() - 0.5).abs() < 1e-9, "never scaled to fit: {}", f.sim.scale());
        // The trade sheet's corner of the floor lands on the plan's.
        let q = f.sim.apply([5200.0, 100.0]);
        assert!((q[0] - 100.0).abs() <= 18.0 && (q[1] - 50.0).abs() <= 18.0, "{q:?}");
        assert!(f.score >= FIT_F1, "{}", f.score);
        // The log's best try is weighed at that ratio too.
        let tried = lines.best_tried(1, &plan, 0.5);
        let k: f64 = tried.trim_start_matches("best scale ").split(',').next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        assert!((k / 0.5 - 1.0).abs() <= 0.025, "{tried}");
        // A sheet placed through a sibling fitted at the ratio keeps its scale.
        let (m, _, _) = SheetLines {
            rects: vec![[5000.0, 0.0, 9000.0, 2800.0], [10000.0, 0.0, 14000.0, 2800.0]],
            pts: vec![floor(2.0, [5200.0, 100.0]), floor(2.0, [10200.0, 100.0])],
            stripped: vec![true, true],
            k_in: 1.0,
        }
        .through_sibling(1, 0, &f.sim)
        .expect("the same background, frame on frame");
        assert!((m.scale() - 0.5).abs() < 1e-9);
        let q = m.apply([10200.0, 100.0]);
        assert!((q[0] - 100.0).abs() <= 18.0 && (q[1] - 50.0).abs() <= 18.0, "{q:?}");
    }

    #[test]
    fn a_misread_ratio_is_one_to_one() {
        assert_eq!(scale_ratio(Some(12.0), Some(12.0)), 1.0);
        assert_eq!(scale_ratio(Some(1.0), Some(1000.0)), 1.0, "a thousandth is a misread note");
        assert_eq!(scale_ratio(Some(f64::NAN), Some(12.0)), 1.0);
        assert_eq!(scale_ratio(Some(-3.0), Some(12.0)), 1.0);
        assert_eq!(scale_ratio(Some(24.0), Some(12.0)), 2.0);
    }

    #[test]
    fn a_sheet_with_schedules_beside_its_plan_is_fitted_by_its_plan() {
        // A floor of unlike rooms, and the same floor 100 along on a sheet with a schedule's table
        // far beside it: the sheet is much wider than the plan.
        let walls = |dx: f64| -> Vec<Vec2> {
            let mut p = Vec::new();
            let mut line = |a: (f64, f64), b: (f64, f64)| {
                let n = ((b.0 - a.0).abs() + (b.1 - a.1).abs()) as i32 / 6;
                p.extend((0..=n).map(|i| f64::from(i) / f64::from(n.max(1))).map(|f| Vec2::new(dx + a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f)));
            };
            line((0.0, 0.0), (900.0, 0.0));
            line((900.0, 0.0), (900.0, 500.0));
            line((900.0, 500.0), (0.0, 500.0));
            line((0.0, 500.0), (0.0, 0.0));
            for (x, h) in [(130.0, 410.0), (290.0, 170.0), (370.0, 500.0), (610.0, 260.0), (770.0, 390.0)] {
                line((x, 0.0), (x, h));
            }
            for (y, a, b) in [(90.0, 0.0, 370.0), (340.0, 130.0, 610.0), (220.0, 370.0, 900.0), (430.0, 610.0, 900.0)] {
                line((a, y), (b, y));
            }
            p
        };
        let plan = walls(0.0);
        let mut sheet = walls(100.0);
        let table: Vec<Vec2> = (0..40).flat_map(|r| (0..40).map(move |c| Vec2::new(2000.0 + f64::from(c) * 20.0, f64::from(r) * 12.0))).collect();
        let whole_is_too_wide = SheetLines {
            rects: vec![[0.0, 0.0, 3000.0, 600.0]],
            pts: vec![{
                sheet.extend(table);
                sheet
            }],
            stripped: vec![true],
            k_in: 0.25,
        };
        let f = whole_is_too_wide.fit(0, &plan, 1.0).expect("its plan fits");
        assert!((f.sim.tx + 100.0).abs() <= 15.0 && f.sim.ty.abs() <= 15.0, "{:?}", (f.sim.tx, f.sim.ty));
    }

    #[test]
    fn views_name_the_part_of_the_site_they_draw() {
        let texts = vec![
            t("BUILDING 2 - UNIT 7 -", 10.0, 10.0),
            t("2ND FLOOR PLAN - MECHANICAL", 10.0, 0.0),
            t("UNIT HEATERS SHALL BE CEILING HUNG", 20.0, 20.0),
            t("AREA OF WORK", 30.0, 30.0),
            t("REFER TO BUILDING 9 FOR RISERS", 40.0, 40.0),
            t("BLDG. 3 - UNIT 8", 1010.0, 10.0),
            t("WING B", 2010.0, 10.0),
        ];
        let a = site_parts(&texts, &[0.0, 0.0, 500.0, 500.0]);
        // A title names its building and unit; a note mentioning a word, or naming a part in its
        // middle, names nothing.
        assert_eq!(a, set(&[("BUILDING", "2"), ("UNIT", "7")]));
        let b = site_parts(&texts, &[1000.0, 0.0, 1500.0, 500.0]);
        assert_eq!(b, set(&[("BUILDING", "3"), ("UNIT", "8")]));
        assert_eq!(site_parts(&texts, &[2000.0, 0.0, 2500.0, 500.0]), set(&[("WING", "B")]));
        // Another building: other parts. The same building's unit: not. Naming none: says nothing.
        assert!(other_parts(&a, &b).is_some_and(|w| w.contains("BUILDING 2") && w.contains("BUILDING 3")));
        let plan = set(&[("BUILDING", "2"), ("UNIT", "6"), ("UNIT", "7")]);
        assert_eq!(other_parts(&set(&[("BUILDING", "2"), ("UNIT", "7")]), &plan), None);
        assert!(other_parts(&set(&[("BUILDING", "2"), ("UNIT", "5")]), &plan).is_some());
        assert_eq!(other_parts(&a, &BTreeSet::new()), None);
        assert_eq!(other_parts(&set(&[("WING", "B")]), &plan), None);
    }
}
