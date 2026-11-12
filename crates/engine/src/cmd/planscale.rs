//! A plan whose scale isn't read (Q6360).
//!
//! A plan's scale comes from its scale note (as text, or lettering drawn as shapes) or, when none
//! reads, from its dimension strings ([`cadaddy_io::pdf_read`]). A plan none of those scale used
//! to come in at paper size, and its level was built a few square feet big with nothing asked.
//! Now, in this order:
//! 1. a plan of the same floor that prints its scale (a trade's plan drawn over the floor plan's
//!    background) whose building outline is the plan's at one uniform ratio, its linework then
//!    lying on the plan's, gives the plan its scale ([`scale_from_same_floor`]); the source says
//!    which sheet and how well they agree. It is tried for a plan scaled by its dimension strings
//!    too, which it outranks: a scale the two disagree on is the printed one's;
//! 2. else the plan stays at paper size and the job **always asks** for its scale (or that it is
//!    no plan); it is never built at the sheet's own units on a default;
//! 3. before the build, a level whose floor plan covers less than the rules data's minimum area
//!    is a high finding and an always-asked question, and after it a level whose walls total less
//!    than the minimum length is too ([`small_levels`], [`short_wall_levels`]).
//!
//! The numbers (the minimum footprint and wall length, how closely the outlines must agree, the
//! print scales a derived scale snaps to) are rule data: `planscale.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use cadaddy_doc::{Drawing, Entity, EntityKind};
use cadaddy_geom::{Bounds2, Mat3, Vec2};
use serde::Deserialize;
use serde_json::{Value, json};

use super::guided::{GuidedJob, Question, QuestionEvidence, QuestionKind, SheetRegion, SheetScale, SheetSpan, StageName};

const RULES: &str = include_str!("planscale.json");

/// What a plan scaled from another plan of its floor says its scale came from (its
/// [`SheetScale::how`] is `same floor`).
pub const SAME_FLOOR_SOURCE: &str = "same-floor plan";
/// The question for a plan whose scale nothing gives (always asked).
pub const NO_SCALE_QUESTION: &str = "No scale for plan sheet";
/// The question for a level too small to be a building (always asked).
pub const SMALL_LEVEL_QUESTION: &str = "Too small to be a building: level";
/// The answer that leaves a sheet out of the building.
pub const LEAVE_OUT: &str = "Not a plan: leave it out of the building";
/// The answer that keeps a small level as read.
pub const KEEP: &str = "Keep it: the building really is that small";

/// Most points sampled from one sheet's linework.
const MAX_SHEET_POINTS: usize = 200_000;
/// Fewest points on each side of a fit.
const MIN_POINTS: usize = 200;
/// Model objects looked at, at most (hostile drawings).
const MAX_OBJECTS: usize = 5_000_000;
/// Plans weighed against each other, at most.
const MAX_PAIRS: usize = 64;
/// Sheets one question lists as the set's printed scales, at most.
const MAX_SCALE_CHOICES: usize = 6;

/// A print scale a derived scale may snap to.
#[derive(Clone, Debug, Deserialize)]
pub struct PrintScale {
    pub note: String,
    /// Sheet inches per foot.
    pub in_per_ft: f64,
}

/// The rule data (`planscale.json`).
#[derive(Clone, Debug, Deserialize)]
pub struct Rules {
    /// A level whose floor plan covers less (square feet) is too small to be a building.
    pub min_level_footprint_sq_ft: f64,
    /// A level whose built walls total less (feet) is too.
    pub min_level_walls_ft: f64,
    /// How closely (relative) two plans' outlines must keep one ratio across and up the sheet.
    pub outline_ratio_agree: f64,
    /// How close (relative) a derived scale must be to a print scale to be taken as it.
    pub scale_snap: f64,
    /// Two scales this close (relative) are the same.
    pub same_scale: f64,
    /// A plan of the floor gives a plan its scale when their lines lie on each other at least
    /// this well (F1) at that print scale...
    pub min_fit_f1: f64,
    /// ...and at least this much better than at any other print scale...
    pub min_fit_lead: f64,
    /// ...and its building and the plan's span the same within this share across and up (the
    /// smaller over the larger): the whole floor, not a small plan of a part of the site.
    pub min_outline_share: f64,
    pub print_scales: Vec<PrintScale>,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            min_level_footprint_sq_ft: 100.0,
            min_level_walls_ft: 20.0,
            outline_ratio_agree: 0.03,
            scale_snap: 0.015,
            same_scale: 0.02,
            min_fit_f1: 0.85,
            min_fit_lead: 0.05,
            min_outline_share: 0.6,
            print_scales: Vec::new(),
        }
    }
}

pub fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// A plan given its scale by another plan of its floor.
#[derive(Clone, Debug, PartialEq)]
pub struct Rescaled {
    pub sheet: String,
    /// The plan of the same floor whose printed scale it took.
    pub onto: String,
    /// Drawing units a sheet inch, before and after.
    pub from_scale: f64,
    pub scale: f64,
    /// The print scale it is (`1/8" = 1'-0"`), when it is one.
    pub note: Option<String>,
    /// F1 of the two plans' lines on each other, the uniform scale the fit found (1 at the print
    /// scale taken) and its margin over the next best placement.
    pub score: f64,
    pub fit: [f64; 2],
    /// How it was read before (`none`: paper size, `dimensions`).
    pub was: &'static str,
}

/// A level's key (`Level 2`, `L2`, `2` are one).
fn level_key(l: &str) -> String {
    let d: String = l.chars().filter(char::is_ascii_digit).collect();
    if d.is_empty() { l.trim().to_ascii_uppercase() } else { d.trim_start_matches('0').to_string() }
}

fn base(name: &str) -> &str {
    name.split('/').next().unwrap_or(name).trim()
}

fn finite_pos(v: f64) -> bool {
    v.is_finite() && v > 0.0
}

/// Whether two scales are the same within `tol` (relative).
fn same(a: f64, b: f64, tol: f64) -> bool {
    finite_pos(a) && finite_pos(b) && (a / b - 1.0).abs() <= tol
}

/// The print scale `k` (drawing units a sheet inch, `k_in` drawing units an inch) is within the
/// rules' snap, and its note.
pub fn snap(k: f64, k_in: f64) -> (f64, Option<String>) {
    let r = rules();
    if !(finite_pos(k) && finite_pos(k_in)) {
        return (k, None);
    }
    let p = 12.0 * k_in / k;
    r.print_scales
        .iter()
        .filter(|s| finite_pos(s.in_per_ft))
        .map(|s| (s, (s.in_per_ft / p - 1.0).abs()))
        .filter(|(_, d)| *d <= r.scale_snap)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or((k, None), |(s, _)| (12.0 * k_in / s.in_per_ft, Some(s.note.clone())))
}

/// A plan that can be built: a floor plan of a storey, not left out.
fn built_plan(r: &SheetRegion) -> bool {
    r.kind == "plan" && r.not_built.is_empty() && !r.all_levels().is_empty()
}

/// Each plan of the set left at paper size, or scaled from its dimension strings, given the
/// scale of a plan of the same floor that prints its own when their building outlines agree at
/// one ratio and their linework lies on each other there. The sheet's objects (`spans`) are
/// scaled in the drawing and moved clear of the others, its rectangle, frame and scale updated
/// (its source naming the plan it took the scale from). What was rescaled.
pub fn scale_from_same_floor(
    d: &mut Drawing,
    regions: &mut [SheetRegion],
    scales: &mut [SheetScale],
    spans: &[SheetSpan],
    log: &mut Vec<String>,
) -> Vec<Rescaled> {
    let rr = rules();
    let k_in = super::perceive::units_per_inch(d);
    let region_of = |scales: &[SheetScale], regions: &[SheetRegion], i: usize| -> Option<usize> {
        let sc = scales.get(i)?;
        let b = base(&sc.sheet);
        if b.is_empty() {
            return None;
        }
        regions.iter().position(|r| r.view.is_none() && r.number.eq_ignore_ascii_case(b)).filter(|j| regions.get(*j).is_some_and(built_plan))
    };
    let targets: Vec<(usize, usize)> = (0..scales.len())
        .filter(|i| scales.get(*i).is_some_and(|sc| sc.kind == "plan" && (sc.plan_at_paper_size() || sc.how() == "dimensions")))
        .filter_map(|i| Some((i, region_of(scales, regions, i)?)))
        .collect();
    if targets.is_empty() {
        return Vec::new();
    }
    let printed: Vec<(usize, usize)> = (0..scales.len())
        .filter(|i| {
            scales
                .get(*i)
                .is_some_and(|sc| sc.kind == "plan" && matches!(sc.how(), "given" | "note" | "lettering") && sc.scale > 1.5 && sc.scale.is_finite())
        })
        .filter_map(|i| Some((i, region_of(scales, regions, i)?)))
        .collect();
    let shares_level = |a: &SheetRegion, b: &SheetRegion| {
        let la: BTreeSet<String> = a.all_levels().iter().map(|l| level_key(l)).collect();
        b.all_levels().iter().any(|l| la.contains(&level_key(l)))
    };
    // Each target with the plans of its floor that print their scale.
    let pairs: Vec<(usize, usize, usize, usize)> = targets
        .iter()
        .flat_map(|&(ti, tr)| printed.iter().filter(move |&&(_, pr)| pr != tr).map(move |&(pi, pr)| (ti, tr, pi, pr)))
        .filter(|&(_, tr, _, pr)| matches!((regions.get(tr), regions.get(pr)), (Some(a), Some(b)) if shares_level(a, b)))
        .take(MAX_PAIRS)
        .collect();
    for &(ti, tr) in &targets {
        if !pairs.iter().any(|p| p.1 == tr)
            && let (Some(t), Some(r)) = (scales.get(ti), regions.get(tr))
        {
            log.push(format!("{}: no other plan of its floor ({}) prints its scale", t.sheet, r.all_levels().join(", ")));
        }
    }
    if pairs.is_empty() {
        return Vec::new();
    }
    // The linework of the sheets weighed (lines and polylines of each sheet's objects).
    let wanted: BTreeSet<usize> = pairs.iter().flat_map(|p| [p.1, p.3]).collect();
    let mut spans_of: Vec<(u64, u64, usize)> = Vec::new();
    for &r in &wanted {
        let Some(reg) = regions.get(r) else { continue };
        let name = scales.iter().find(|sc| base(&sc.sheet).eq_ignore_ascii_case(&reg.number)).map(|sc| sc.sheet.clone()).unwrap_or_default();
        for sp in spans.iter().filter(|sp| sp.sheet == name) {
            spans_of.push((sp.lo, sp.hi, r));
        }
    }
    spans_of.sort_by_key(|s| s.0);
    let mut ents: BTreeMap<usize, Vec<&Arc<Entity>>> = BTreeMap::new();
    for e in d.model.iter().take(MAX_OBJECTS) {
        if !matches!(e.kind, EntityKind::Line(_) | EntityKind::LwPolyline(_)) {
            continue;
        }
        let h = e.handle.0;
        let at = spans_of.partition_point(|s| s.0 <= h);
        if let Some(&(lo, hi, r)) = at.checked_sub(1).and_then(|i| spans_of.get(i))
            && h >= lo
            && h < hi
        {
            ents.entry(r).or_default().push(e);
        }
    }
    let scale_of = |r: usize| -> f64 {
        regions
            .get(r)
            .and_then(|reg| scales.iter().find(|sc| base(&sc.sheet).eq_ignore_ascii_case(&reg.number)))
            .map_or(1.0, |sc| if finite_pos(sc.scale) { sc.scale } else { 1.0 })
    };
    // Each sheet's plan: the biggest block of its lines inside its frame (sampled every eighth of
    // a sheet inch, blocks of half-inch cells).
    let mut plan_pts: BTreeMap<usize, Vec<Vec2>> = BTreeMap::new();
    let mut all_pts: BTreeMap<usize, Vec<Vec2>> = BTreeMap::new();
    for (&r, es) in &ents {
        let Some(reg) = regions.get(r) else { continue };
        let k = scale_of(r) * k_in;
        let a = reg.area.unwrap_or(reg.rect);
        let inset = 0.02 * (a[2] - a[0]).abs().min((a[3] - a[1]).abs());
        let inside = |p: &Vec2| {
            p.x >= a[0].min(a[2]) + inset && p.x <= a[0].max(a[2]) - inset && p.y >= a[1].min(a[3]) + inset && p.y <= a[1].max(a[3]) - inset
        };
        let all: Vec<Vec2> = super::grids::line_samples(es, 0.125 * k).into_iter().filter(inside).collect();
        let stride = (all.len() / MAX_SHEET_POINTS).max(1);
        let pts: Vec<Vec2> = all.into_iter().step_by(stride).collect();
        plan_pts.insert(r, super::grids::largest_block(&pts, 0.5 * k));
        all_pts.insert(r, pts);
    }
    drop(ents);
    // The best plan of its floor for each target: the plan that prints its scale (its biggest
    // block of lines, the building) laid on the target's lines scaled to each print scale of
    // the rules data (the reference's own first), at one uniform scale within the rules'
    // agreement; the print scale whose fit holds best.
    let cands: Vec<f64> = rr.print_scales.iter().filter(|p| finite_pos(p.in_per_ft)).map(|p| 12.0 * k_in / p.in_per_ft).collect();
    // Per target: every (plan, print scale) weighed, its F1, uniform scale and margin.
    #[allow(clippy::type_complexity)]
    let mut weighed: BTreeMap<usize, Vec<(usize, usize, f64, f64, [f64; 2], bool)>> = BTreeMap::new();
    for &(ti, tr, pi, pr) in &pairs {
        let (Some(src), Some(tgt)) = (plan_pts.get(&pr), all_pts.get(&tr)) else { continue };
        if src.len() < MIN_POINTS || tgt.len() < MIN_POINTS {
            if let (Some(t), Some(p)) = (scales.get(ti), scales.get(pi)) {
                log.push(format!("{}: too few lines to weigh against {} ({} and {} points)", t.sheet, p.sheet, tgt.len(), src.len()));
            }
            continue;
        }
        let old = scale_of(tr);
        let mut tried: Vec<f64> = vec![scale_of(pr)];
        tried.extend(cands.iter().copied().filter(|c| !same(*c, scale_of(pr), rr.same_scale)));
        for c in tried {
            let f = c / old;
            if !finite_pos(f) {
                continue;
            }
            let o = tgt.first().copied().unwrap_or(Vec2::new(0.0, 0.0));
            let scaled: Vec<Vec2> = tgt.iter().map(|p| o + (*p - o) * f).collect();
            let Some((s, _, f1, margin)) =
                super::grids::linework_fit_part(src, &scaled, k_in, 1.0 - rr.outline_ratio_agree, 1.0 + rr.outline_ratio_agree)
            else {
                continue;
            };
            // The reference is the floor's outline: as wide and as tall as the plan's building
            // (its biggest block), within the rules' share.
            let (bs, bt) = (Bounds2::from_points(src.iter().copied()), plan_pts.get(&tr).map(|p| Bounds2::from_points(p.iter().copied())));
            let near = |a: f64, b: f64| finite_pos(a) && finite_pos(b) && a.min(b) >= rr.min_outline_share * a.max(b);
            let whole = bt.is_some_and(|bt| near(bs.width() * s, bt.width() * f) && near(bs.height() * s, bt.height() * f));
            if finite_pos(s) && f1.is_finite() {
                weighed.entry(tr).or_default().push((ti, pi, c, f1, [s, margin], whole));
            }
        }
    }
    // The print scale whose fit holds (the rules' F1, the fit's margin over another placement)
    // and leads every other print scale's by the rules' lead: a scale off the fit's own few
    // percent is its sampling's, so the print scale itself.
    let mut best: BTreeMap<usize, (usize, usize, f64, f64, [f64; 2])> = BTreeMap::new();
    for (tr, list) in weighed {
        // The best fit whose outline is the plan's, against every other scale weighed (an
        // outline of another size included: a plan shrunk to a small plan's size lies on its lines
        // as well, and leads nothing).
        let Some(top) = list.iter().filter(|x| x.5).max_by(|a, b| a.3.total_cmp(&b.3)).copied() else { continue };
        let runner = list.iter().filter(|x| !same(x.2, top.2, rr.same_scale)).map(|x| x.3).fold(0.0, f64::max);
        if top.3 >= rr.min_fit_f1 && top.4[1] >= super::sheetreg::FIT_MARGIN && top.3 - runner >= rr.min_fit_lead {
            let old = scale_of(tr);
            best.insert(tr, (top.0, top.1, top.2 / old, top.3, top.4));
        } else if let (Some(t), Some(p)) = (scales.get(top.0), scales.get(top.1)) {
            log.push(format!(
                "{}: no plan of its floor gives its scale (best {} at {}: {:.0}% of their lines, {:.0} points over another placement, {:.0} over another scale; needs {:.0}%, {:.0} and {:.0})",
                t.sheet,
                p.sheet,
                snap(top.2, k_in).1.unwrap_or_else(|| format!("{:.2}x", top.2)),
                top.3 * 100.0,
                top.4[1] * 100.0,
                (top.3 - runner) * 100.0,
                rr.min_fit_f1 * 100.0,
                super::sheetreg::FIT_MARGIN * 100.0,
                rr.min_fit_lead * 100.0
            ));
        }
    }
    let mut out = Vec::new();
    for (tr, (ti, pi, s, f1, fit)) in best {
        let (Some(sc), Some(onto)) = (scales.get(ti).cloned(), scales.get(pi).map(|p| p.sheet.clone())) else { continue };
        let old = if finite_pos(sc.scale) { sc.scale } else { 1.0 };
        let (k, note) = snap(old * s, k_in);
        let f = k / old;
        if !finite_pos(f) || same(f, 1.0, rr.same_scale) {
            continue;
        }
        let was = sc.how();
        // The sheet's objects scaled about its lower-left corner, then moved clear of every sheet.
        let Some(reg) = regions.get(tr).cloned() else { continue };
        let (w, h) = ((reg.rect[2] - reg.rect[0]) * f, (reg.rect[3] - reg.rect[1]) * f);
        let right = regions.iter().map(|r| r.rect[2].max(r.rect[0])).fold(f64::NEG_INFINITY, f64::max);
        if !(right.is_finite() && w.is_finite() && h.is_finite()) {
            continue;
        }
        let at = Vec2::new(right + 0.05 * w.abs(), reg.rect[1]);
        let m = Mat3::translate(Vec2::new(-reg.rect[0], -reg.rect[1])).then(Mat3::scale(f, f)).then(Mat3::translate(at));
        let mine: Vec<(u64, u64)> = spans.iter().filter(|sp| sp.sheet == sc.sheet).map(|sp| (sp.lo, sp.hi)).collect();
        let handles: Vec<cadaddy_doc::Handle> =
            d.model.iter().take(MAX_OBJECTS).map(|e| e.handle).filter(|h| mine.iter().any(|(lo, hi)| h.0 >= *lo && h.0 < *hi)).collect();
        for h in handles {
            let _ = d.modify_entity(h, |e| e.kind.transform(&m));
        }
        let moved = |r: [f64; 4]| {
            let (a, b) = (m.apply(Vec2::new(r[0], r[1])), m.apply(Vec2::new(r[2], r[3])));
            [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)]
        };
        if let Some(r) = regions.get_mut(tr) {
            r.rect = moved(r.rect);
            r.area = r.area.map(moved);
            r.title_block = r.title_block.map(moved);
        }
        if let Some(e) = scales.get_mut(ti) {
            e.scale = k;
            e.plan_scale = None;
            e.source = format!(
                "{SAME_FLOOR_SOURCE} {onto}: {} {onto}'s building lies on its lines at one uniform scale ({:.3}), {:.0}% of their lines on each other, {:.0} points over the next placement; was {}",
                note.as_deref().map_or_else(|| format!("{k:.2}x;"), |n| format!("{n} ({k:.2}x);")),
                fit[0],
                f1 * 100.0,
                fit[1] * 100.0,
                if was == "none" { "paper size".to_string() } else { format!("{old:.2}x from its {was}") }
            )
            .chars()
            .take(240)
            .collect();
        }
        out.push(Rescaled { sheet: sc.sheet.clone(), onto, from_scale: old, scale: k, note, score: f1, fit, was });
    }
    out
}

/// The set's printed plan scales, most used first, as answers to offer.
pub fn printed_scale_choices(scales: &[SheetScale]) -> Vec<String> {
    let k_in = 1.0;
    let mut n: BTreeMap<String, usize> = BTreeMap::new();
    for sc in scales.iter().filter(|sc| sc.kind == "plan" && matches!(sc.how(), "note" | "lettering" | "given")) {
        if let (_, Some(note)) = snap(sc.scale, k_in) {
            *n.entry(note).or_default() += 1;
        }
    }
    let mut v: Vec<(String, usize)> = n.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.into_iter().take(MAX_SCALE_CHOICES).map(|(k, _)| k).collect()
}

/// Ask `question` about `sheet` in `stage`, always (never taken by default, in auto mode too).
#[allow(clippy::too_many_arguments)]
pub fn ask_always(job: &mut GuidedJob, stage: StageName, question: String, why: String, sheet: &str, choices: Vec<String>, values: Value) {
    let id = job.next_qid(stage);
    job.always_ask.insert(id.clone());
    let q = Question {
        id,
        stage,
        question,
        why,
        choices,
        evidence: Some(QuestionEvidence {
            crop_png_path: None,
            sheet: Some(sheet.to_string()),
            at: None,
            values: Some(values),
            handles: Vec::new(),
            tiled_crops: None,
            leftovers: Vec::new(),
        }),
        default: None,
        answer_schema: Some(json!({"type": "string"})),
        answer: None,
        answered_by: None,
        answered_at: None,
        kind: QuestionKind::Completeness,
    };
    job.add_question_to_worklist(&q);
    if let Some(st) = job.stage_mut(stage) {
        st.completeness.push(q);
    }
}

/// The question for plan `sheet` that nothing scaled (always asked).
pub fn no_scale_question(sheet: &str, title: &str) -> String {
    if title.trim().is_empty() {
        format!("{NO_SCALE_QUESTION} {sheet}: what scale is it drawn at?")
    } else {
        format!("{NO_SCALE_QUESTION} {sheet} \"{}\": what scale is it drawn at?", title.trim().chars().take(80).collect::<String>())
    }
}

/// Each storey's floor-plan area (square feet) from the architectural plans it is built from,
/// with the sheet giving the largest: the levels under the rules' minimum.
pub fn small_levels(regions: &[SheetRegion], k_in: f64) -> Vec<(String, String, f64)> {
    let k_in = if finite_pos(k_in) { k_in } else { 1.0 };
    let mut by: BTreeMap<String, (String, String, f64)> = BTreeMap::new();
    for r in regions.iter().filter(|r| built_plan(r) && r.discipline.eq_ignore_ascii_case("A")) {
        let p = r.plan_rect();
        let sq_ft = (p[2] - p[0]).abs() * (p[3] - p[1]).abs() / (12.0 * k_in).powi(2);
        if !sq_ft.is_finite() {
            continue;
        }
        for l in r.all_levels() {
            let e = by.entry(level_key(&l)).or_insert_with(|| (l.clone(), r.number.clone(), sq_ft));
            if sq_ft > e.2 {
                *e = (l.clone(), r.number.clone(), sq_ft);
            }
        }
    }
    by.into_values().filter(|(_, _, a)| *a < rules().min_level_footprint_sq_ft).collect()
}

/// A built wall: its level, its length along the floor and its plan box `[x0, y0, x1, y1]`
/// (drawing units).
pub type BuiltWall = (String, f64, [f64; 4]);

/// The levels whose built walls total less than the rules' minimum (feet) and all stand inside
/// less than the minimum footprint: a plan at the wrong scale, not a building (a real level of a
/// few walls, a penthouse on the roof, spans more than that). Each with its walls' total, feet.
/// `upf` drawing units a foot.
pub fn short_wall_levels(walls: &[BuiltWall], upf: f64) -> Vec<(String, f64)> {
    let upf = if finite_pos(upf) { upf } else { 12.0 };
    let r = rules();
    let mut by: BTreeMap<String, (f64, [f64; 4])> = BTreeMap::new();
    for (level, len, b) in walls {
        if !(len.is_finite() && *len > 0.0 && b.iter().all(|v| v.is_finite())) || level.is_empty() {
            continue;
        }
        let e = by.entry(level.clone()).or_insert((0.0, [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY]));
        e.0 += len / upf;
        e.1 = [e.1[0].min(b[0]), e.1[1].min(b[1]), e.1[2].max(b[2]), e.1[3].max(b[3])];
    }
    by.into_iter()
        .filter(|(_, (ft, b))| *ft < r.min_level_walls_ft && (b[2] - b[0]).abs() * (b[3] - b[1]).abs() / (upf * upf) < r.min_level_footprint_sq_ft)
        .map(|(l, (ft, _))| (l, ft))
        .collect()
}

/// What an answer to one of these questions says: the sheet's scale (drawing units a sheet inch,
/// as given), that it is no plan, or that the level is kept as read.
#[derive(Clone, Debug, PartialEq)]
pub enum ScaleAnswer {
    Scale(f64, String),
    LeaveOut,
    Keep,
    Bad(String),
}

/// Whether `q` is one of this module's questions.
pub fn is_scale_question(q: &Question) -> bool {
    q.question.starts_with(NO_SCALE_QUESTION) || q.question.starts_with(SMALL_LEVEL_QUESTION)
}

/// Read an answer to [`is_scale_question`].
pub fn read_answer(answer: &Value) -> ScaleAnswer {
    let text = answer.as_str().map(str::trim).unwrap_or("");
    if text == LEAVE_OUT {
        return ScaleAnswer::LeaveOut;
    }
    if text == KEEP {
        return ScaleAnswer::Keep;
    }
    match super::sheetcheck::parse_answer_scale(answer) {
        Ok((k, t)) => ScaleAnswer::Scale(k, t),
        Err(e) => ScaleAnswer::Bad(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_load_and_snap_to_print_scales() {
        let r = rules();
        assert!(r.min_level_footprint_sq_ft > 0.0 && r.min_level_walls_ft > 0.0 && !r.print_scales.is_empty());
        assert_eq!(snap(95.4, 1.0), (96.0, Some("1/8\" = 1'-0\"".to_string())));
        assert_eq!(snap(48.3, 1.0).1.as_deref(), Some("1/4\" = 1'-0\""));
        // off every print scale: kept as measured
        assert_eq!(snap(70.0, 1.0), (70.0, None));
        assert_eq!(snap(f64::NAN, 1.0).1, None);
    }

    #[test]
    fn answers_read() {
        assert_eq!(read_answer(&json!(LEAVE_OUT)), ScaleAnswer::LeaveOut);
        assert_eq!(read_answer(&json!(KEEP)), ScaleAnswer::Keep);
        assert!(matches!(read_answer(&json!("1/8\" = 1'-0\"")), ScaleAnswer::Scale(k, _) if (k - 96.0).abs() < 1e-9));
        assert!(matches!(read_answer(&json!("48")), ScaleAnswer::Scale(k, _) if (k - 48.0).abs() < 1e-9));
        assert!(matches!(read_answer(&json!("big")), ScaleAnswer::Bad(_)));
        assert!(matches!(read_answer(&json!("0")), ScaleAnswer::Bad(_)));
    }

    fn region(number: &str, level: &str, rect: [f64; 4]) -> SheetRegion {
        SheetRegion {
            number: number.into(),
            level: level.into(),
            discipline: "A".into(),
            kind: "plan".into(),
            rect,
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        }
    }

    #[test]
    fn a_level_the_size_of_a_sheet_is_too_small() {
        // A 36" x 24" sheet at paper size: 6 sq ft; at 1/8" = 1'-0": 288' x 192'.
        let paper = region("A-1", "1", [0.0, 0.0, 36.0, 24.0]);
        let scaled = region("A-2", "2", [100.0, 0.0, 100.0 + 36.0 * 96.0, 24.0 * 96.0]);
        let small = small_levels(&[paper.clone(), scaled.clone()], 1.0);
        assert_eq!(small.len(), 1, "{small:?}");
        assert_eq!((small[0].0.as_str(), small[0].1.as_str()), ("1", "A-1"));
        assert!((small[0].2 - 6.0).abs() < 1e-9);
        // a level with a real plan as well is not small
        let mut both = paper;
        both.level = "2".into();
        assert!(small_levels(&[both, scaled], 1.0).is_empty());
    }

    #[test]
    fn walls_a_few_feet_long_are_too_short() {
        let w = |l: &str, len_ft: f64, b: [f64; 4]| (l.to_string(), 12.0 * len_ft, b);
        let walls = vec![
            // a plan at paper size: two walls inside a 33" x 24" box
            w("1", 1.0, [0.0, 0.0, 12.0, 6.0]),
            w("1", 1.15, [10.0, 10.0, 33.0, 24.0]),
            // a real floor
            w("2", 300.0, [0.0, 0.0, 1200.0, 2400.0]),
            // a roof with one short stretch of wall that still spans a real roof
            w("R", 15.0, [0.0, 0.0, 180.0, 6.0]),
            w("R", 2.0, [1180.0, 2380.0, 1200.0, 2400.0]),
        ];
        let short = short_wall_levels(&walls, 12.0);
        assert_eq!(short.len(), 1, "{short:?}");
        assert_eq!(short[0].0, "1");
        assert!((short[0].1 - 2.15).abs() < 1e-9);
        assert!(short_wall_levels(&[w("", 1.0, [0.0; 4]), w("3", f64::NAN, [0.0; 4])], 12.0).is_empty());
    }
}
