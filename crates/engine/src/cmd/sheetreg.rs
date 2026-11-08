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
//! - a sheet nothing else places is fitted onto its level's plan by its linework, one to one
//!   (sheets come in at their printed scales), with the fit's score and its margin over the next
//!   best placement recorded ([`SheetLines::fit`]);
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

    /// Sheet `i` fitted onto `tgt` (its level's plan linework) by its linework, one to one within
    /// 2.5% (a fit off that is its sampling's error, so one to one about its middle; one at the
    /// end of the scales tried is no fit), the whole floor or a part of it (an enlarged plan). `None` without a fit that holds ([`FIT_F1`],
    /// [`FIT_MARGIN`]).
    pub(crate) fn fit(&self, i: usize, tgt: &[Vec2]) -> Option<LineworkFit> {
        let src = self.pts.get(i)?;
        if !self.stripped(i) || src.len() < MIN_POINTS || tgt.len() < MIN_POINTS {
            return None;
        }
        // Each scale proposes its best few placements, so the margin is over a real runner-up.
        let fit = super::grids::linework_fit_part(src, tgt, self.k_in, 0.97, 1.03);
        let (s, t, score, margin) = fit.filter(|f| f.2 >= FIT_F1 && f.3 >= FIT_MARGIN && (f.0 - 1.0).abs() <= 0.025)?;
        let mid = src.iter().fold(Vec2::new(0.0, 0.0), |a, q| a + *q) * (1.0 / src.len().max(1) as f64);
        let sim = Sim::scale_offset(1.0, [t.x + mid.x * (s - 1.0), t.y + mid.y * (s - 1.0)]);
        sim.is_finite().then_some(LineworkFit { sim, score, margin })
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
