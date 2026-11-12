//! Typical exhaust risers (Q6496): a set often draws a guest bathroom's exhaust only on its
//! typical room view (the fan and its small duct to the wall chase) and on a riser detail
//! (`TYPICAL EXHAUST RISER, 12 FANS`: the fans of the rooms stacked over each other, their ducts
//! up the chase to the roof terminal it tags, `GRH-7`). The floor plans only key one instance; the
//! roof plan places a terminal over every chase. Nothing on a floor plan draws the risers, so a
//! build that reads plans gets fans with a stub of duct and roof hoods connected to nothing.
//!
//! This step builds what the field installs from those documents:
//! - [`riser_details`] reads the details (title words in rules data
//!   `mechanical.exhaust_riser_titles`): how many fans each says it carries and the roof
//!   terminals it tags (of the terminals the schedules list and the roof plan places).
//! - [`stack_fan_ducts`] puts the fans' duct ends of every storey on one plan (lined up by the
//!   chases they share, so a storey's sheet registered a few feet off still stacks), takes ends
//!   close together as one chase, lines the roof's terminals up with the chases and gives each
//!   chase the nearest free terminal of a detail (within the trade practice rule
//!   `M-EXHAUST-RISER`'s reach): each fan's duct goes on up the chase to just under the roof
//!   deck, across to the terminal and up into it. Where a note requires each exhaust duct to be
//!   continuous to the outside
//!   (rules data `mechanical.continuous_exhaust_duct_notes`, [`continuous_required`]), every fan
//!   keeps its own duct, side by side in the chase (derived: the detail and the roof plan say
//!   where); else the chase carries one riser of the fans' combined duct area (inferred, the
//!   rule's basis said).
//!
//! A riser rises in the chase each storey's own plan draws (Q6550): the storeys are lined up by
//! their chases here, before the build sets the levels' base points from other things, and a
//! storey's plan may stand feet off where those carry a riser (into the bath beside the chase).
//! So each riser keeps its place on every storey it passes (`MepRun::storey_points`), and the
//! build's cut at the floors puts each storey's piece there. A riser that would stand in a
//! plumbing fixture the plans draw or a piece of equipment (not one a duct runs into) on any
//! storey is moved to the nearest place clear of it, the drawn place kept beside it as a
//! correction (rule `M-EXHAUST-RISER`); one that can't be is reported.
//!
//! Every piece carries what it is and why (`made`, `basis`) and the detail in its height source,
//! so a rebuild replaces them and nothing passes for drawn; the fans' own ducts are left as they
//! are (a piece of its own carries each on across the chase). A terminal no chase reaches, a chase
//! under no terminal, and a riser carrying another number of fans than its detail says are
//! reported, with how far each storey and the roof were moved to line up.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::Drawing;
use cadaddy_walk3d::{BuildingSpec, Made, MepKind, MepRun};
use serde_json::{Value, json};

use super::schedequip::{Txt, find_tags};

/// Details looked at, at most; fans and terminals taken, at most (hostile drawings).
const MAX_DETAILS: usize = 64;
const MAX_ITEMS: usize = 20_000;
/// What every piece this step lays says first in its height source: a rebuild replaces them.
pub(crate) const RISER_SOURCE: &str = "typical riser detail";
/// How far (feet) from its fan's box a duct may start and still be the fan's duct.
const FAN_DUCT_START_FT: f64 = 1.5;
/// Where no view region was found round a detail's title: how far round it (in its title's text
/// heights) its tags are looked for, across and above.
const DETAIL_ACROSS_H: f64 = 40.0;
const DETAIL_ABOVE_H: f64 = 80.0;
/// A turn shorter than this (feet) is no piece of its own.
const MIN_PIECE_FT: f64 = 0.05;
/// How far along its line (text heights) a note's words are read.
const NOTE_LINE_H: f64 = 80.0;

/// One typical exhaust riser detail.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RiserDetail {
    /// The sheet it is on and its number there (`M-9`, `3`).
    pub sheet: String,
    pub number: Option<String>,
    pub title: String,
    /// How many fans its title says it carries (`12 FANS`).
    pub fans: Option<usize>,
    /// The roof terminals it tags (of the terminals the job has: `GRH-7`).
    pub terminals: Vec<String>,
}

impl RiserDetail {
    fn name(&self) -> String {
        match &self.number {
            Some(n) => format!("{} detail {n} {}", self.sheet, self.title),
            None => format!("{} {}", self.sheet, self.title),
        }
    }
}

/// Whether `text` has `word` (a phrase) as whole words.
fn has_words(text: &str, word: &str) -> bool {
    let spaced = |s: &str| {
        let w: Vec<String> = s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect();
        format!(" {} ", w.join(" "))
    };
    let w = spaced(word);
    !w.trim().is_empty() && spaced(text).contains(&w)
}

/// Whether `text` has `word` (a phrase) starting at a word's start: a print's text often loses the
/// space after a word (`CONTINUOUSFROM`).
fn has_word_start(text: &str, word: &str) -> bool {
    let (t, w) = (text.to_ascii_uppercase(), word.trim().to_ascii_uppercase());
    !w.is_empty() && t.match_indices(&w).any(|(i, _)| t.get(..i).and_then(|b| b.chars().next_back()).is_none_or(|c| !c.is_ascii_alphanumeric()))
}

/// The typical exhaust riser details on `pages` (sheet number, page): each one's title, how many
/// fans it says and which of `terminal_tags` it tags (the tags inside its view; with no view
/// found, round its title).
pub(crate) fn riser_details(pages: &[(String, Drawing)], terminal_tags: &[String]) -> Vec<RiserDetail> {
    let mut out: Vec<RiserDetail> = Vec::new();
    for (sheet, d) in pages {
        let texts = cadaddy_mepfix::sheet_facts::texts_with_tags(d);
        let txts: Vec<Txt> = texts
            .iter()
            .filter(|t| t.at.is_finite() && t.height.is_finite() && t.height > 0.0)
            .take(MAX_ITEMS)
            .map(|t| Txt { value: t.value.trim().to_ascii_uppercase(), at: [t.at.x, t.at.y], h: t.height })
            .filter(|t| !t.value.is_empty())
            .collect();
        let views = cadaddy_mepfix::sheet_facts::view_titles(d, &texts);
        let titles: Vec<[f64; 2]> = views.iter().map(|v| v.at).collect();
        for v in views {
            if out.len() >= MAX_DETAILS {
                return out;
            }
            let up = v.title.to_ascii_uppercase();
            if cadaddy_mepfix::wordrules::first_rule("mechanical.exhaust_riser_titles", |w| has_words(&up, w)).is_none() {
                continue;
            }
            let fans = up.split(|c: char| !c.is_ascii_alphanumeric()).find_map(|w| w.parse::<usize>().ok()).filter(|n| (1..=500).contains(n));
            let h = txts
                .iter()
                .filter(|t| (t.at[0] - v.at[0]).hypot(t.at[1] - v.at[1]) <= 1e-6 * (1.0 + v.at[0].abs()))
                .map(|t| t.h)
                .next()
                .or_else(|| txts.iter().map(|t| t.h).min_by(f64::total_cmp))
                .unwrap_or(1.0);
            let region = v.region.unwrap_or([v.at[0] - DETAIL_ACROSS_H * h, v.at[1], v.at[0] + DETAIL_ACROSS_H * h, v.at[1] + DETAIL_ABOVE_H * h]);
            let inside = |p: [f64; 2]| {
                p[0] >= region[0].min(region[2])
                    && p[0] <= region[0].max(region[2])
                    && p[1] >= region[1].min(region[3])
                    && p[1] <= region[1].max(region[3])
            };
            // With no view found, a tag is this detail's only when its title is the nearest title
            // under it to its left (a sheet of details side by side, titled under each).
            let mine = |p: [f64; 2]| {
                v.region.is_some()
                    || titles
                        .iter()
                        .filter(|q| q[1] < p[1] && q[0] <= p[0])
                        .max_by(|a, b| a[0].total_cmp(&b[0]).then(b[1].total_cmp(&a[1])))
                        .is_some_and(|q| (q[0] - v.at[0]).abs() <= 1e-9 && (q[1] - v.at[1]).abs() <= 1e-9)
            };
            let near: Vec<Txt> =
                txts.iter().filter(|t| inside(t.at) && mine(t.at)).map(|t| Txt { value: t.value.clone(), at: t.at, h: t.h }).collect();
            let terminals: Vec<String> = terminal_tags.iter().filter(|t| !find_tags(&near, &t.to_ascii_uppercase()).is_empty()).cloned().collect();
            out.push(RiserDetail { sheet: sheet.clone(), number: v.number.clone(), title: v.title.trim().to_string(), fans, terminals });
        }
    }
    out
}

/// Whether a note on any of `drawings` requires each exhaust duct to be continuous to the outside
/// (rules data `mechanical.continuous_exhaust_duct_notes`): the sentence that says so, if one does.
/// A note's lines are often separate texts: each line with a rule word is read with the rest of
/// its line and the line under it (lines of one note start one under the other, lettered alike).
pub(crate) fn continuous_required<'a>(drawings: impl IntoIterator<Item = &'a Drawing>) -> Option<String> {
    let rules = cadaddy_mepfix::wordrules::rules("mechanical.continuous_exhaust_duct_notes");
    let keys: Vec<&str> = rules.iter().flat_map(|r| r.any.iter().chain(r.all.iter()).copied()).collect();
    let says = |t: &str| cadaddy_mepfix::wordrules::first_rule("mechanical.continuous_exhaust_duct_notes", |w| has_word_start(t, w)).is_some();
    for d in drawings {
        let texts: Vec<(String, [f64; 2], f64)> = cadaddy_mepfix::sheet_facts::texts(d)
            .iter()
            .take(MAX_ITEMS)
            .filter(|t| t.at.is_finite() && t.height.is_finite() && t.height > 0.0)
            .map(|t| (t.value.to_ascii_uppercase(), [t.at.x, t.at.y], t.height))
            .collect();
        for (v, at, h) in texts.iter().filter(|(v, _, _)| keys.iter().any(|k| has_word_start(v, k))).take(MAX_DETAILS) {
            let line = |p: [f64; 2]| (p[1] - at[1]).abs() <= 0.5 * h && p[0] >= at[0] && p[0] <= at[0] + NOTE_LINE_H * h;
            let under = |p: [f64; 2]| (p[0] - at[0]).abs() <= 2.0 * h && p[1] < at[1] - 0.5 * h && p[1] >= at[1] - 2.5 * h;
            // A note's lines are lettered alike (another block's text beside it is not).
            let alike = |g: f64| (g - h).abs() <= 0.15 * h;
            let mut near: Vec<&(String, [f64; 2], f64)> = texts.iter().filter(|(_, p, g)| alike(*g) && (line(*p) || under(*p))).collect();
            near.sort_by(|a, b| b.1[1].total_cmp(&a.1[1]).then(a.1[0].total_cmp(&b.1[0])));
            let joined = near.iter().map(|(t, _, _)| t.as_str()).collect::<Vec<_>>().join(" ");
            let joined = if joined.contains(v.as_str()) { joined } else { format!("{v} {joined}") };
            if says(&joined) {
                // The sentence that says it (the note's others are not this step's).
                let sentence = joined.split(". ").find(|s| says(s)).unwrap_or(&joined);
                return Some(sentence.split_whitespace().collect::<Vec<_>>().join(" "));
            }
        }
    }
    None
}

/// A fan this step stacks: its level, the end of its duct (on its level's plan),
/// the way the duct's last piece runs (unit, plan), its size and bottom.
struct Fan {
    level: String,
    end: [f64; 2],
    dir: Option<[f64; 2]>,
    size_in: f64,
    bottom_ft: f64,
}

/// A roof terminal: its tag, level, centre on its level's plan and where its duct goes in
/// (feet above its level's floor: the top of its curb).
struct Terminal {
    tag: String,
    level: String,
    centre: [f64; 2],
    inlet_ft: f64,
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

/// Points looked at when fitting one layout onto another, at most.
const MAX_FIT: usize = 300;

/// The shift (no longer than `most`) that puts the most of `a` within `tol` of a point of `b`:
/// how many it puts there and the shift (the shorter of equals; none moved: zero). Two plans of
/// one building registered a few feet apart line up again by what they share (the same chases
/// floor after floor; the terminals over them).
fn best_shift(a: &[[f64; 2]], b: &[[f64; 2]], tol: f64, most: f64) -> (usize, [f64; 2]) {
    let (a, b) = (a.get(..a.len().min(MAX_FIT)).unwrap_or(a), b.get(..b.len().min(MAX_FIT)).unwrap_or(b));
    if !(tol.is_finite() && tol > 0.0) {
        return (0, [0.0, 0.0]);
    }
    // `b` by cells `tol` wide: a point's neighbours are in its cell and the eight round it.
    let cell = |p: [f64; 2]| ((p[0] / tol).floor() as i64, (p[1] / tol).floor() as i64);
    let mut grid: std::collections::HashMap<(i64, i64), Vec<[f64; 2]>> = std::collections::HashMap::new();
    for q in b.iter().filter(|q| q[0].is_finite() && q[1].is_finite()) {
        grid.entry(cell(*q)).or_default().push(*q);
    }
    // The nearest point of `b` within `tol` of `p`.
    let hit = |p: [f64; 2]| {
        let (i, j) = cell(p);
        (i - 1..=i + 1)
            .flat_map(|x| (j - 1..=j + 1).map(move |y| (x, y)))
            .filter_map(|k| grid.get(&k))
            .flatten()
            .filter(|q| dist(p, **q) <= tol)
            .min_by(|u, v| dist(p, **u).total_cmp(&dist(p, **v)))
            .copied()
    };
    let count = |t: [f64; 2]| a.iter().filter(|p| hit(add(**p, t)).is_some()).count();
    let mut best: (usize, [f64; 2]) = (count([0.0, 0.0]), [0.0, 0.0]);
    for p in a {
        for q in b {
            let t = sub(*q, *p);
            let len = t[0].hypot(t[1]);
            if !len.is_finite() || len > most {
                continue;
            }
            let n = count(t);
            let best_len = best.1[0].hypot(best.1[1]);
            if n > best.0 || n == best.0 && len < best_len {
                best = (n, t);
            }
        }
    }
    // Every shift within `tol` of the true one matches as many: the one the matched pairs agree
    // on (their mean), not the shortest.
    let t = best.1;
    let pairs: Vec<[f64; 2]> = a.iter().filter_map(|p| hit(add(*p, t)).map(|q| sub(q, add(*p, t)))).collect();
    if !pairs.is_empty() {
        let n = pairs.len() as f64;
        let mean = [pairs.iter().map(|d| d[0]).sum::<f64>() / n, pairs.iter().map(|d| d[1]).sum::<f64>() / n];
        let refined = add(t, mean);
        if count(refined) >= best.0 {
            best.1 = refined;
        }
    }
    best
}

/// How close (feet) two floors' chases must come to be the same chase once lined up; how far
/// apart the ends of one chase's ducts may be; how far apart two floors' plans may be registered
/// and still be lined up by their chases; how far off the roof plan may be.
const SAME_CHASE_FT: f64 = 1.5;
const CHASE_FT: f64 = 3.0;
const FLOOR_SHIFT_FT: f64 = 15.0;
const ROOF_SHIFT_FT: f64 = 40.0;
/// A terminal within this (feet) of a chase once the roof is lined up counts towards the fit.
const ROOF_FIT_FT: f64 = 5.0;

/// What a riser may not stand in, on its level's plan (sheet units): a plumbing fixture's drawn
/// outline or a piece of equipment's box, from `z` up to `top` (feet over its floor).
struct Obstacle {
    lo: [f64; 2],
    hi: [f64; 2],
    z: f64,
    top: f64,
    what: String,
}

/// The obstacles of each level (by its name, lower case): the plumbing fixtures the plans draw
/// (their outlines; standing on the floor as high as their kind's usual body, else
/// `fixture_top_ft`) and the equipment, but for the kinds a duct runs into or through
/// (`pass_kinds`, words of the kind: the fans, the hoods, the dampers).
fn obstacles(sp: &BuildingSpec, upf: f64, fixture_top_ft: f64, pass_kinds: &[String]) -> BTreeMap<String, Vec<Obstacle>> {
    let mut out: BTreeMap<String, Vec<Obstacle>> = BTreeMap::new();
    let field = |t: &cadaddy_walk3d::tags::ElementTag, k: &str| t.fields.get(k).map(|f| f.value.trim().to_string()).filter(|v| !v.is_empty());
    for t in sp.tags.iter().filter(|t| t.id.starts_with("device:") && t.kind == "plumbing_fixture").take(MAX_ITEMS) {
        let Some([w, d]) = field(t, "drawn_size_in").and_then(|v| super::fixturebody::drawn_size(&v)) else { continue };
        let c = [t.at_ft[0] * upf, t.at_ft[1] * upf];
        if !c.iter().all(|v| v.is_finite()) {
            continue;
        }
        let name = ["fixture", "description", "device_type"].iter().find_map(|k| field(t, k)).unwrap_or_else(|| "plumbing fixture".into());
        let top = super::fixturebody::family_of(super::fixturebody::Class::Plumbing, &name)
            .map(|f| f.size_in[2] / 12.0)
            .filter(|h| h.is_finite() && *h > 0.0)
            .unwrap_or(fixture_top_ft);
        let (hw, hd) = (w / 24.0 * upf, d / 24.0 * upf);
        out.entry(t.level.to_ascii_lowercase()).or_default().push(Obstacle {
            lo: [c[0] - hw, c[1] - hd],
            hi: [c[0] + hw, c[1] + hd],
            z: 0.0,
            top,
            what: format!("{} {}", name.to_ascii_lowercase(), t.id),
        });
    }
    let passes = |kind: &str| {
        let k = format!(" {} ", kind.to_ascii_lowercase().replace(['_', '-'], " "));
        pass_kinds.iter().any(|w| !w.trim().is_empty() && k.contains(&format!(" {} ", w.trim().to_ascii_lowercase())))
    };
    for e in sp.mep.equipment.iter().take(MAX_ITEMS).filter(|e| !passes(&e.kind)) {
        let (lo, hi) = ([e.min[0].min(e.max[0]), e.min[1].min(e.max[1])], [e.min[0].max(e.max[0]), e.min[1].max(e.max[1])]);
        if !(lo.iter().chain(hi.iter()).all(|v| v.is_finite()) && e.bottom_ft.is_finite() && e.height_ft.is_finite()) {
            continue;
        }
        out.entry(e.level.to_ascii_lowercase()).or_default().push(Obstacle {
            lo,
            hi,
            z: e.bottom_ft,
            top: e.bottom_ft + e.height_ft.max(0.0),
            what: format!("{} {}", e.kind.replace('_', " "), e.tag),
        });
    }
    out
}

/// Where a riser at `p` from `z0` up to `z1` (feet over its floor) clears every obstacle there
/// once each is grown by `grow` (sheet units: the duct's half width and a gap): `None` when it
/// stands clear; else the nearest place clear of them all, found by stepping out of each across
/// its nearest side (`None` when none is near), and what it stood in.
fn clear_of(obs: &[Obstacle], p: [f64; 2], z0: f64, z1: f64, grow: f64) -> Option<(Option<[f64; 2]>, String)> {
    let here: Vec<&Obstacle> = obs.iter().filter(|o| o.z < z1 && o.top > z0).collect();
    let inside =
        |q: [f64; 2]| here.iter().copied().find(|o| q[0] > o.lo[0] - grow && q[0] < o.hi[0] + grow && q[1] > o.lo[1] - grow && q[1] < o.hi[1] + grow);
    let what = inside(p)?.what.clone();
    let mut frontier = vec![p];
    for _ in 0..CLEAR_STEPS {
        let mut next: Vec<[f64; 2]> = Vec::new();
        for q in &frontier {
            if let Some(o) = inside(*q) {
                next.extend([[o.lo[0] - grow, q[1]], [o.hi[0] + grow, q[1]], [q[0], o.lo[1] - grow], [q[0], o.hi[1] + grow]]);
            }
        }
        next.sort_by(|a, b| dist(*a, p).total_cmp(&dist(*b, p)));
        if let Some(q) = next.iter().find(|q| inside(**q).is_none()) {
            return Some((Some(*q), what));
        }
        next.truncate(CLEAR_FRONTIER);
        frontier = next;
    }
    Some((None, what))
}

/// How many times a riser steps out of what it stands in, at most, and how many places it tries
/// from at each step.
const CLEAR_STEPS: usize = 4;
const CLEAR_FRONTIER: usize = 16;

/// What [`stand`] places a riser by.
struct StandBy<'a> {
    sp: &'a BuildingSpec,
    /// Level → the shift that puts its plan's points on the stacking storey's plan.
    onto: &'a BTreeMap<String, [f64; 2]>,
    /// The storeys (name, floor elevation), lowest first.
    storeys: &'a [(String, f64)],
    obstacles: &'a BTreeMap<String, Vec<Obstacle>>,
    upf: f64,
    gap_in: f64,
}

/// Where one riser stands: on its own level, on each other storey it passes (where that
/// storey's own plan puts it), what it was moved clear of (each a correction) and what it could
/// not be moved clear of.
struct Stand {
    at: [f64; 2],
    storey_points: BTreeMap<String, [f64; 2]>,
    corrections: Vec<cadaddy_walk3d::RunCorrection>,
    stuck: Vec<String>,
}

/// A riser given on `level` at `at` (its own plan), standing for `slot` on the stacking storey's
/// plan, from `bottom` to `top` feet over its floor, `size_in` across. On each storey it passes
/// whose plan the chases lined up, it stands in that storey's own drawing of the chase (`slot`
/// on that plan): the levels' base points, set later in the build from other things (a storey's
/// shell), may disagree with what its plan draws by feet, and a riser carried by them stands in
/// that storey's rooms (Q6550). Where it would stand in a plumbing fixture or a piece of
/// equipment on any storey, it is moved to the nearest place clear of it, the drawn place kept
/// beside the corrected one.
fn stand(by: &StandBy, level: &str, at: [f64; 2], slot: [f64; 2], bottom: f64, top: f64, size_in: f64) -> Stand {
    const EPS: f64 = 1e-6;
    let upf = by.upf;
    let grow = (size_in.max(0.0) / 2.0 + by.gap_in) / 12.0 * upf;
    let floor = by.storeys.iter().find(|(n, _)| n.eq_ignore_ascii_case(level)).map(|s| s.1).or_else(|| by.sp.level(level).map(|l| l.elevation_ft));
    let next_floor = |e: f64| by.storeys.iter().map(|s| s.1).filter(|x| *x > e + EPS).fold(f64::INFINITY, f64::min);
    let pt = |p: [f64; 2]| format!("{:.2},{:.2} ft", p[0] / upf, p[1] / upf);
    let mut s = Stand { at, storey_points: BTreeMap::new(), corrections: Vec::new(), stuck: Vec::new() };
    let place = |lv: &str, p: [f64; 2], z0: f64, z1: f64, s: &mut Stand| -> [f64; 2] {
        match by.obstacles.get(&lv.to_ascii_lowercase()).and_then(|o| clear_of(o, p, z0, z1, grow)) {
            None => p,
            Some((Some(q), what)) => {
                s.corrections.push(cadaddy_walk3d::RunCorrection {
                    key: format!("plan point on {lv}"),
                    design: pt(p),
                    corrected: pt(q),
                    reason: format!("it stood in the {what} on {lv}: moved {:.0} in to the nearest place clear of it", dist(p, q) / upf * 12.0),
                    rule: "M-EXHAUST-RISER".into(),
                });
                q
            }
            Some((None, what)) => {
                s.stuck.push(format!("{what} on {lv}"));
                p
            }
        }
    };
    let Some(fz) = floor.filter(|f| f.is_finite()) else { return s };
    s.at = place(level, at, bottom, top.min(next_floor(fz) - fz), &mut s);
    for (name, e) in by.storeys.iter().filter(|(n, e)| !n.eq_ignore_ascii_case(level) && *e > fz + EPS && *e < fz + top - EPS) {
        let z1 = next_floor(*e).min(fz + top) - e;
        match by.onto.iter().find(|(l, _)| l.eq_ignore_ascii_case(name)).map(|(_, v)| *v) {
            Some(shift) => {
                let q = place(name, sub(slot, shift), 0.0, z1, &mut s);
                s.storey_points.insert(name.clone(), q);
            }
            // No fan of this storey lined it up: where the base points carry it, moved only if it
            // stands in something there (as far as the plans' origins tell now).
            None => {
                let p = super::risercut::carry(by.sp, s.at, level, name);
                let q = place(name, p, 0.0, z1, &mut s);
                if dist(p, q) > EPS {
                    s.storey_points.insert(name.clone(), q);
                }
            }
        }
    }
    s
}

/// Points within `reach` of a group's mean, grouped (in order): each group's mean.
fn centres(points: &[[f64; 2]], reach: f64) -> Vec<[f64; 2]> {
    let mut out: Vec<([f64; 2], f64)> = Vec::new();
    for p in points {
        match out.iter_mut().find(|(c, _)| dist(*c, *p) <= reach) {
            Some((c, n)) => {
                *c = [(c[0] * *n + p[0]) / (*n + 1.0), (c[1] * *n + p[1]) / (*n + 1.0)];
                *n += 1.0;
            }
            None => out.push((*p, 1.0)),
        }
    }
    out.into_iter().map(|(c, _)| c).collect()
}

/// Each exhaust fan whose duct ends in a chase under a roof terminal one of `details` tags (see
/// the module notes): its duct carried on up the chase into the terminal, one duct per fan where
/// `continuous` names the note that requires it, else one shared riser per chase. Pieces an
/// earlier run of this step laid are replaced. What was built and what was left, for the log.
///
/// The fans' duct ends are put on one plan (the storey with most of them): each storey by its
/// plan's place, then lined up with that one by the ends they share (a storey's plan registered a
/// few feet off moves onto the others). Ends within [`CHASE_FT`] of each other are one chase. The
/// roof's terminals are lined up with the chases the same way, then each chase takes the nearest
/// terminal still free within the rule's reach (one terminal per riser, as the details draw).
/// Each riser stands in each storey's own drawing of its chase and clear of fixtures and
/// equipment ([`stand`]).
pub(crate) fn stack_fan_ducts(sp: &mut BuildingSpec, details: &[RiserDetail], continuous: Option<&str>) -> Value {
    sp.mep.runs.retain(|r| !r.height_source.starts_with(RISER_SOURCE));
    if details.is_empty() {
        return json!({"details": 0});
    }
    let rule = super::provenance::rule("M-EXHAUST-RISER");
    let data = super::provenance::rule_data("M-EXHAUST-RISER");
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let reach = if rule.reach_ft > 0.0 { rule.reach_ft } else { 15.0 } * upf;
    let clear_ft = if rule.clear_ft > 0.0 { rule.clear_ft } else { 1.0 };
    let default_in = if rule.default_size_in > 0.0 { rule.default_size_in } else { 4.0 };
    let gap_in = data.get("duct_gap_in").and_then(Value::as_f64).filter(|g| g.is_finite() && *g >= 0.0).unwrap_or(1.0);
    let tagged: BTreeSet<String> = details.iter().flat_map(|d| d.terminals.iter().map(|t| t.trim().to_ascii_uppercase())).collect();
    let elevation = |level: &str| sp.level(level).map(|l| l.elevation_ft).filter(|e| e.is_finite());
    let terminals: Vec<Terminal> = sp
        .mep
        .equipment
        .iter()
        .filter(|e| if tagged.is_empty() { e.kind == "relief_hood" } else { tagged.contains(&e.tag.trim().to_ascii_uppercase()) })
        .take(MAX_ITEMS)
        .map(|e| Terminal {
            tag: e.tag.trim().to_ascii_uppercase(),
            level: e.level.clone(),
            centre: [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0],
            inlet_ft: e.bottom_ft.max(0.0),
        })
        .collect();
    let ducts: BTreeSet<&str> = sp.mep.systems.iter().filter(|y| y.kind == MepKind::Duct).map(|y| y.name.as_str()).collect();
    let lowest_terminal = terminals.iter().filter_map(|t| elevation(&t.level)).fold(f64::INFINITY, f64::min);
    // Each fan's duct: a duct run on its level starting at the fan, the one starting nearest it.
    let mut fans: Vec<Fan> = Vec::new();
    for e in sp.mep.equipment.iter().filter(|e| e.kind == "exhaust_fan").take(MAX_ITEMS) {
        if elevation(&e.level).is_none_or(|z| z >= lowest_terminal) {
            continue;
        }
        let c = [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0];
        let near = FAN_DUCT_START_FT * upf;
        let at_fan = |p: [f64; 2]| p[0] >= e.min[0] - near && p[0] <= e.max[0] + near && p[1] >= e.min[1] - near && p[1] <= e.max[1] + near;
        let run = sp
            .mep
            .runs
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                r.level == e.level
                    && r.top_ft.is_none()
                    && r.points.len() >= 2
                    && (ducts.contains(r.system.as_str()) || r.system.eq_ignore_ascii_case("EA"))
                    && r.points.first().is_some_and(|p| at_fan(*p))
            })
            .min_by(|a, b| {
                let d = |r: &MepRun| r.points.first().map_or(f64::INFINITY, |p| dist(*p, c));
                d(a.1).total_cmp(&d(b.1))
            });
        let Some((_, r)) = run else { continue };
        let (Some(end), Some(prev)) = (r.points.last().copied(), r.points.iter().rev().nth(1).copied()) else { continue };
        let len = dist(end, prev);
        fans.push(Fan {
            level: e.level.clone(),
            end,
            dir: (len > 1e-9).then(|| [(end[0] - prev[0]) / len, (end[1] - prev[1]) / len]),
            size_in: if r.size_in[0].is_finite() && r.size_in[0] > 0.0 { r.size_in[0] } else { default_in },
            bottom_ft: r.bottoms_ft.last().copied().filter(|b| b.is_finite()).unwrap_or(0.0),
        });
    }
    // One plan to stack on: the storey with the most fan ducts. Each storey's shift onto it: its
    // plan's place (the levels' origins), then lined up by the duct ends they share.
    // Each storey's chases on its own plan (the fans of a chase end close together; lined up by
    // the chases, never by single ends, which a chase's neighbour can stand in for).
    let mut per_level: BTreeMap<&str, Vec<[f64; 2]>> = BTreeMap::new();
    for f in &fans {
        per_level.entry(f.level.as_str()).or_default().push(f.end);
    }
    let per_level: BTreeMap<&str, Vec<[f64; 2]>> = per_level.into_iter().map(|(l, ends)| (l, centres(&ends, CHASE_FT * upf))).collect();
    let Some(base) = per_level.iter().max_by_key(|(_, v)| v.len()).map(|(l, _)| l.to_string()) else {
        return json!({"details": details.len(), "fans_with_ducts": 0, "terminals": terminals.len()});
    };
    let base_ends: Vec<[f64; 2]> = per_level.get(base.as_str()).cloned().unwrap_or_default();
    // Level → the shift that puts its plan's points on the base storey's plan.
    let mut onto: BTreeMap<String, [f64; 2]> = BTreeMap::new();
    let mut floor_fit: BTreeMap<String, Value> = BTreeMap::new();
    for (level, ends) in &per_level {
        let Some((o_l, o_b)) = super::schedequip::origins(sp, level, &base) else { continue };
        let placed = sub(o_b, o_l);
        let moved: Vec<[f64; 2]> = ends.iter().map(|p| add(*p, placed)).collect();
        let (n, fit) =
            if *level == base { (ends.len(), [0.0, 0.0]) } else { best_shift(&moved, &base_ends, SAME_CHASE_FT * upf, FLOOR_SHIFT_FT * upf) };
        // Lined up by too few to trust: by its plan's place alone.
        let fit = if n >= 2 && 3 * n >= ends.len() { fit } else { [0.0, 0.0] };
        onto.insert((*level).to_string(), add(placed, fit));
        floor_fit.insert(
            (*level).to_string(),
            json!({"chases_matched": n, "of": ends.len(), "moved_ft": [(fit[0] / upf * 10.0).round() / 10.0, (fit[1] / upf * 10.0).round() / 10.0]}),
        );
    }
    // The chases: duct ends within CHASE_FT of each other's mean, on the base storey's plan.
    let mut chases: Vec<([f64; 2], Vec<usize>)> = Vec::new();
    // (Fans in order, so the same building gives the same chases.)
    for (fi, f) in fans.iter().enumerate() {
        let Some(shift) = onto.get(&f.level) else { continue };
        let p = add(f.end, *shift);
        match chases.iter_mut().find(|(c, _)| dist(*c, p) <= CHASE_FT * upf) {
            Some((c, m)) => {
                let n = m.len() as f64;
                *c = [(c[0] * n + p[0]) / (n + 1.0), (c[1] * n + p[1]) / (n + 1.0)];
                m.push(fi);
            }
            None => chases.push((p, vec![fi])),
        }
    }
    // The terminals on the base storey's plan: by the roof plan's place, then lined up with the
    // chases (a terminal stands over each).
    let mut roof_onto: BTreeMap<String, [f64; 2]> = BTreeMap::new();
    let mut roof_fit: Vec<Value> = Vec::new();
    let roofs: BTreeSet<&str> = terminals.iter().map(|t| t.level.as_str()).collect();
    let chase_points: Vec<[f64; 2]> = chases.iter().map(|(c, _)| *c).collect();
    for level in roofs {
        let Some((o_b, o_r)) = super::schedequip::origins(sp, &base, level) else { continue };
        let placed = sub(o_b, o_r);
        let here: Vec<[f64; 2]> = terminals.iter().filter(|t| t.level == level).map(|t| add(t.centre, placed)).collect();
        let (n, fit) = best_shift(&chase_points, &here, ROOF_FIT_FT * upf, ROOF_SHIFT_FT * upf);
        // `best_shift` moves the chases onto the terminals: the terminals move the other way.
        let fit = if n >= 2 { [-fit[0], -fit[1]] } else { [0.0, 0.0] };
        roof_onto.insert(level.to_string(), add(placed, fit));
        roof_fit.push(json!({"level": level, "matched": n, "chases": chase_points.len(), "moved_ft": [(fit[0] / upf * 10.0).round() / 10.0, (fit[1] / upf * 10.0).round() / 10.0]}));
    }
    let terminal_at = |t: &Terminal| roof_onto.get(&t.level).map(|s| add(t.centre, *s));
    // Each chase to the nearest terminal still free (nearest pairs first), within reach.
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (ci, (c, _)) in chases.iter().enumerate() {
        for (ti, t) in terminals.iter().enumerate() {
            if let Some(at) = terminal_at(t) {
                let d = dist(*c, at);
                if d <= reach {
                    pairs.push((d, ci, ti));
                }
            }
        }
    }
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut chase_of, mut taken): (BTreeMap<usize, usize>, BTreeSet<usize>) = (BTreeMap::new(), BTreeSet::new());
    for (_, ci, ti) in pairs {
        if !chase_of.contains_key(&ci) && !taken.contains(&ti) {
            chase_of.insert(ci, ti);
            taken.insert(ti);
        }
    }
    // Where each riser stands on the storeys it passes, clear of fixtures and equipment.
    let fixture_top_ft = data.get("fixture_top_ft").and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(7.0);
    let pass_kinds: Vec<String> = data
        .get("pass_kinds")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let obstacles = obstacles(sp, upf, fixture_top_ft, &pass_kinds);
    let mut storeys: Vec<(String, f64)> = sp
        .levels
        .iter()
        .filter(|l| l.elevation_ft.is_finite() && l.sheet.is_none())
        .take(MAX_ITEMS)
        .map(|l| (l.name.clone(), l.elevation_ft))
        .collect();
    storeys.sort_by(|a, b| a.1.total_cmp(&b.1));
    let by = StandBy { sp, onto: &onto, storeys: &storeys, obstacles: &obstacles, upf, gap_in };
    let (mut moved, mut stuck, mut storey_points) = (Vec::<String>::new(), Vec::<String>::new(), 0usize);
    let mut new_runs: Vec<MepRun> = Vec::new();
    let mut per_riser: BTreeMap<usize, usize> = BTreeMap::new();
    let mut off_count: Vec<String> = Vec::new();
    let mut stacked = 0usize;
    for (ci, ti) in &chase_of {
        let (Some((chase, members)), Some(t)) = (chases.get(*ci), terminals.get(*ti)) else { continue };
        let (Some(tz), Some(roof_shift)) = (elevation(&t.level), roof_onto.get(&t.level).copied()) else { continue };
        // Its detail: one tagging this terminal, the one whose fan count agrees first.
        let tagging: Vec<&RiserDetail> =
            details.iter().filter(|d| tagged.is_empty() || d.terminals.iter().any(|x| x.trim().eq_ignore_ascii_case(&t.tag))).collect();
        let Some(detail) = tagging.iter().find(|d| d.fans == Some(members.len())).or_else(|| tagging.first()).copied() else { continue };
        *per_riser.entry(members.len()).or_default() += 1;
        if let Some(n) = detail.fans.filter(|n| *n != members.len()) {
            off_count.push(format!("{} on {}: {} fans, the detail says {n}", t.tag, t.level, members.len()));
        }
        // The line across the chase the ducts stand side by side along (square to the way the
        // ducts come in).
        let mut ins = [0.0_f64, 0.0_f64];
        let mut first: Option<[f64; 2]> = None;
        for d in members.iter().filter_map(|fi| fans.get(*fi)).filter_map(|f| f.dir) {
            let s = first.map_or(1.0, |a| if a[0] * d[0] + a[1] * d[1] < 0.0 { -1.0 } else { 1.0 });
            first.get_or_insert(d);
            ins = [ins[0] + s * d[0], ins[1] + s * d[1]];
        }
        let l = ins[0].hypot(ins[1]);
        let along = if l > 1e-9 { [-ins[1] / l, ins[0] / l] } else { [1.0, 0.0] };
        // A point on the base storey's plan onto a level's own plan.
        let to_level = |p: [f64; 2], level: &str| onto.get(level).map(|s| sub(p, *s));
        let to_roof = |p: [f64; 2]| sub(p, roof_shift);
        let mut order: Vec<usize> = members.clone();
        order.sort_by(|a, b| {
            let key =
                |i: &usize| fans.get(*i).map_or((f64::INFINITY, 0.0, 0.0), |f| (elevation(&f.level).unwrap_or(f64::INFINITY), f.end[0], f.end[1]));
            let (ka, kb) = (key(a), key(b));
            ka.0.total_cmp(&kb.0).then(ka.1.total_cmp(&kb.1)).then(ka.2.total_cmp(&kb.2))
        });
        let what = detail.name();
        let source = format!("{RISER_SOURCE} {what}: up the chase to {clear_ft:.1} ft under the roof deck, then to the roof terminal {}", t.tag);
        let piece = |level: &str, points: Vec<[f64; 2]>, bottom: f64, top: Option<f64>, size: f64, made: Made, basis: &str| {
            let mut r = serde_json::from_value::<MepRun>(json!({
                "system": "EA", "level": level, "bottoms_ft": vec![bottom; points.len().saturating_sub(1).max(1)], "points": points,
                "size_in": [size, 0.0], "top_ft": top, "height_source": source, "size_source": "the fan's duct", "source_sheet": detail.sheet,
            }))
            .ok()?;
            super::provenance::set_run(&mut r, made, basis.to_string());
            Some(r)
        };
        // The roof: from over the chase (`at`, on the base storey's plan) across to the terminal
        // and up into it, `off` across its middle.
        let roof = |at: [f64; 2], off: [f64; 2], size: f64, made: Made, basis: &str, runs: &mut Vec<MepRun>| {
            let from = to_roof(at);
            let goes_in = add(t.centre, off);
            if dist(from, goes_in) >= MIN_PIECE_FT * upf {
                runs.extend(piece(&t.level, vec![from, goes_in], -clear_ft, None, size, made, basis));
            }
            runs.extend(piece(&t.level, vec![goes_in], -clear_ft, Some(t.inlet_ft), size, made, basis));
        };
        // A fan's duct on to where its riser stands (`at`, its own plan).
        let to_riser = |f: &Fan, at: [f64; 2], made: Made, basis: &str, runs: &mut Vec<MepRun>| {
            if dist(f.end, at) >= MIN_PIECE_FT * upf {
                runs.extend(piece(&f.level, vec![f.end, at], f.bottom_ft, None, f.size_in, made, basis));
            }
        };
        let n = members.len() as f64;
        if let Some(note) = continuous {
            // One duct per fan, side by side across the chase, lowest storey's first.
            let pitch = order.iter().filter_map(|i| fans.get(*i)).map(|f| f.size_in).fold(0.0_f64, f64::max) + gap_in;
            let basis = format!(
                "derived: {what} stacks the typical rooms' fan ducts in the chase up to the roof terminal {} the roof plan places over it; the notes require each duct continuous to the outside ({note})",
                t.tag
            );
            for (k, fi) in order.iter().enumerate() {
                let Some(f) = fans.get(*fi) else { continue };
                let Some(fz) = elevation(&f.level) else { continue };
                let off = (k as f64 - (n - 1.0) / 2.0) * pitch / 12.0 * upf;
                let off = [along[0] * off, along[1] * off];
                let slot = add(*chase, off);
                let Some(slot_f) = to_level(slot, &f.level) else { continue };
                let top = tz - clear_ft - fz;
                if top <= f.bottom_ft {
                    continue;
                }
                let st = stand(&by, &f.level, slot_f, slot, f.bottom_ft, top, f.size_in);
                to_riser(f, st.at, Made::Derived, &basis, &mut new_runs);
                if let Some(mut r) = piece(&f.level, vec![st.at], f.bottom_ft, Some(top), f.size_in, Made::Derived, &basis) {
                    storey_points += st.storey_points.len();
                    moved.extend(st.corrections.iter().map(|c| c.reason.clone()));
                    stuck.extend(st.stuck);
                    r.storey_points = st.storey_points;
                    r.corrections = st.corrections;
                    new_runs.push(r);
                }
                roof(slot, off, f.size_in, Made::Derived, &basis, &mut new_runs);
                stacked += 1;
            }
        } else {
            // One riser of the fans' combined duct area, from the lowest of them.
            let Some(low) = order.first().and_then(|i| fans.get(*i)) else { continue };
            let Some(lz) = elevation(&low.level) else { continue };
            let area: f64 = order.iter().filter_map(|i| fans.get(*i)).map(|f| f.size_in * f.size_in).sum();
            let size = area.sqrt().ceil();
            let basis = format!(
                "{}; {what} stacks these fans in the chase up to the roof terminal {} the roof plan places over it, one riser of their combined area ({size:.0} in)",
                rule.basis, t.tag
            );
            let Some(at) = to_level(*chase, &low.level) else { continue };
            let top = tz - clear_ft - lz;
            let st = (top > low.bottom_ft).then(|| stand(&by, &low.level, at, *chase, low.bottom_ft, top, size));
            for fi in &order {
                let Some(f) = fans.get(*fi) else { continue };
                // To where the riser stands on the fan's storey.
                let on = st.as_ref().and_then(|s| {
                    if f.level.eq_ignore_ascii_case(&low.level) {
                        Some(s.at)
                    } else {
                        s.storey_points.iter().find(|(l, _)| l.eq_ignore_ascii_case(&f.level)).map(|(_, p)| *p)
                    }
                });
                if let Some(at) = on.or_else(|| to_level(*chase, &f.level)) {
                    to_riser(f, at, Made::Inferred, &basis, &mut new_runs);
                    stacked += 1;
                }
            }
            if let Some(st) = st
                && let Some(mut r) = piece(&low.level, vec![st.at], low.bottom_ft, Some(top), size, Made::Inferred, &basis)
            {
                storey_points += st.storey_points.len();
                moved.extend(st.corrections.iter().map(|c| c.reason.clone()));
                stuck.extend(st.stuck);
                r.storey_points = st.storey_points;
                r.corrections = st.corrections;
                new_runs.push(r);
                roof(*chase, [0.0, 0.0], size, Made::Inferred, &basis, &mut new_runs);
            }
        }
    }
    let risers = new_runs.iter().filter(|r| r.top_ft.is_some() && !roofs_has(&terminals, &r.level)).count();
    let pieces = new_runs.len();
    sp.mep.runs.extend(new_runs);
    let idle: Vec<String> =
        terminals.iter().enumerate().filter(|(i, _)| !taken.contains(i)).map(|(_, t)| format!("{} on {}", t.tag, t.level)).take(50).collect();
    let lone: Vec<usize> = chases.iter().enumerate().filter(|(i, _)| !chase_of.contains_key(i)).map(|(_, (_, m))| m.len()).collect();
    json!({
        "details": details.iter().map(|d| json!({"detail": d.name(), "fans": d.fans, "terminals": d.terminals})).collect::<Vec<_>>(),
        "continuous": continuous.is_some(),
        "terminals": terminals.len(),
        "fans_with_ducts": fans.len(),
        "floors_lined_up_on": base,
        "floors": floor_fit,
        "roof": roof_fit,
        "chases": chases.len(),
        "fans_stacked": stacked,
        "chases_under_no_terminal": lone.len(),
        "fans_under_no_terminal": lone.iter().sum::<usize>(),
        "risers": risers,
        "pieces": pieces,
        "chases_by_fan_count": per_riser,
        "terminals_no_chase_reaches": idle,
        "fan_count_not_the_details": off_count.into_iter().take(50).collect::<Vec<_>>(),
        "storey_points": storey_points,
        "moved_clear": moved.len(),
        "moved_clear_of": moved.into_iter().take(20).collect::<Vec<_>>(),
        "not_cleared": stuck.into_iter().take(20).collect::<Vec<_>>(),
    })
}

/// Whether `level` is a level some terminal stands on (a roof).
fn roofs_has(terminals: &[Terminal], level: &str) -> bool {
    terminals.iter().any(|t| t.level == level)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Common, EntityKind, Space};
    use cadaddy_walk3d::{Level, MepEquipment, MepSystem};

    fn text(d: &mut Drawing, v: &str, x: f64, y: f64, h: f64) {
        let t = cadaddy_doc::Text {
            insert: cadaddy_geom::Vec3::new(x, y, 0.0),
            align_pt: None,
            height: h,
            value: v.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: cadaddy_doc::HAlign::Left,
            valign: cadaddy_doc::VAlign::Baseline,
        };
        let _ = d.add(&Space::Model, Common::default(), EntityKind::Text(t));
    }

    fn line(d: &mut Drawing, a: [f64; 2], b: [f64; 2]) {
        let l = cadaddy_doc::Line { a: cadaddy_geom::Vec3::new(a[0], a[1], 0.0), b: cadaddy_geom::Vec3::new(b[0], b[1], 0.0) };
        let _ = d.add(&Space::Model, Common::default(), EntityKind::Line(l));
    }

    fn rect(d: &mut Drawing, lo: [f64; 2], hi: [f64; 2]) {
        let vertices = [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]]
            .iter()
            .map(|p| cadaddy_geom::PolyVertex::with_bulge(cadaddy_geom::Vec2::new(p[0], p[1]), 0.0))
            .collect();
        let pl = cadaddy_doc::LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false };
        let _ = d.add(&Space::Model, Common::default(), EntityKind::LwPolyline(pl));
    }

    /// A details sheet (made up, in inches at `k` units each): an isometric of stacked ducts to a
    /// hood tagged `tag` (letters over number), titled `title` over its scale; another detail
    /// beside it that is no riser.
    fn details_page(k: f64, title: &str, letters: &str, number: &str) -> Drawing {
        let mut d = Drawing::default();
        for i in 0..4 {
            let x = (100.0 + 6.0 * f64::from(i)) * k;
            line(&mut d, [x, 140.0 * k], [x, 300.0 * k]);
        }
        rect(&mut d, [95.0 * k, 300.0 * k], [135.0 * k, 320.0 * k]);
        text(&mut d, letters, 140.0 * k, 322.0 * k, 3.0 * k);
        text(&mut d, number, 141.0 * k, 316.0 * k, 3.0 * k);
        text(&mut d, "3", 92.0 * k, 106.0 * k, 5.0 * k);
        text(&mut d, title, 100.0 * k, 106.0 * k, 5.0 * k);
        text(&mut d, "SCALE: NOT TO SCALE", 100.0 * k, 100.0 * k, 3.0 * k);
        // A detail beside it: a damper, no riser.
        rect(&mut d, [400.0 * k, 160.0 * k], [460.0 * k, 220.0 * k]);
        text(&mut d, "4", 392.0 * k, 106.0 * k, 5.0 * k);
        text(&mut d, "FIRE DAMPER INSTALLATION", 400.0 * k, 106.0 * k, 5.0 * k);
        text(&mut d, "SCALE: NOT TO SCALE", 400.0 * k, 100.0 * k, 3.0 * k);
        d
    }

    #[test]
    fn a_riser_detail_says_how_many_fans_it_carries_and_the_roof_terminal_it_tags() {
        for k in [1.0, 72.0] {
            let page = details_page(k, "TYPICAL EXHAUST RISER, 12 FANS", "GRH", "7");
            let tags = vec!["GRH-7".to_string(), "GRH-9".to_string()];
            let found = riser_details(&[("M-9".to_string(), page)], &tags);
            assert_eq!(found.len(), 1, "only the riser detail, at {k}: {found:?}");
            let r = found.first().unwrap_or_else(|| panic!("detail"));
            assert_eq!(r.fans, Some(12));
            assert_eq!(r.terminals, vec!["GRH-7".to_string()]);
            assert_eq!(r.number.as_deref(), Some("3"));
        }
        // A supply riser or an exhaust plan is no exhaust riser.
        for title in ["TYPICAL SUPPLY RISER", "EXHAUST PLAN"] {
            let page = details_page(1.0, title, "GRH", "7");
            assert!(riser_details(&[("M-9".to_string(), page)], &["GRH-7".to_string()]).is_empty(), "{title}");
        }
    }

    #[test]
    fn a_note_requiring_continuous_exhaust_ducts_is_found() {
        let mut d = Drawing::default();
        text(&mut d, "ALL TOILET EXHAUST DUCTWORK TO BE CONTINUOUS TO OUTDOORS", 0.0, 0.0, 3.0);
        assert!(continuous_required([&d]).is_some_and(|n| n.contains("CONTINUOUS")));
        // Its lines separate texts, a space lost after a word.
        let mut lines = Drawing::default();
        text(&mut lines, "BATH FANS ARE IN THE CEILINGS. EACH EXHAUST", 0.0, 10.0, 3.0);
        text(&mut lines, "DUCT TO RUN CONTINUOUSTO THE FAN'S ROOF HOOD", 0.0, 5.0, 3.0);
        assert!(continuous_required([&lines]).is_some());
        // Only the note's own lines: not the title block beside it.
        let mut boxed = Drawing::default();
        text(&mut boxed, "SEAL ALL JOINTS. EACH EXHAUST DUCT TO RUN CONTINUOUSTO", 0.0, 10.0, 3.0);
        text(&mut boxed, "1 SAMPLE STREET", 120.0, 9.0, 2.0);
        text(&mut boxed, "ITS ROOF HOOD. SUPPORT EVERY 8 FT", 0.0, 5.0, 3.0);
        assert_eq!(continuous_required([&boxed]).as_deref(), Some("EACH EXHAUST DUCT TO RUN CONTINUOUSTO ITS ROOF HOOD"));
        let mut other = Drawing::default();
        text(&mut other, "PROVIDE CONTINUOUS INSULATION ON SUPPLY DUCT", 0.0, 0.0, 3.0);
        assert!(continuous_required([&other]).is_none());
    }

    fn level(name: &str, el: f64, base: [f64; 2]) -> Level {
        Level { name: name.into(), elevation_ft: el, height_ft: 10.0, ceiling_ft: 8.0, region: None, base: Some(base), attr: None, sheet: None }
    }

    /// Four storeys (plans side by side, 10,000 units apart) and a roof, inches: on each storey two
    /// back-to-back bathroom fans, each with its 4" duct to the chase between them (x 125 and 135,
    /// y 240); `hoods`: roof terminals (tag, centre on the roof's plan).
    fn building(hoods: &[(&str, [f64; 2])]) -> BuildingSpec {
        building_with(&[0.0], [[0.0, 0.0]; 4], hoods)
    }

    /// [`building`] with a chase pair of fans every `chases` (x offsets, inches), each storey's
    /// drawing of them `off` its plan's place (a sheet registered a few feet off).
    fn building_with(chases: &[f64], off: [[f64; 2]; 4], hoods: &[(&str, [f64; 2])]) -> BuildingSpec {
        let names = ["Level 1", "Level 2", "Level 3", "Level 4"];
        let mut levels: Vec<Level> = names.iter().enumerate().map(|(i, n)| level(n, 10.0 * i as f64, [10_000.0 * i as f64, 0.0])).collect();
        levels.push(level("Roof", 40.0, [40_000.0, 0.0]));
        let mut sp = BuildingSpec { units_per_foot: 12.0, levels, ..Default::default() };
        if let Ok(ea) =
            serde_json::from_value::<MepSystem>(json!({"name": "EA", "layer": "M-EXH", "kind": "duct", "size_in": [4.0, 0.0], "bottom_ft": 8.25}))
        {
            sp.mep.systems.push(ea);
        }
        for (i, n) in names.iter().enumerate() {
            let [dx, dy] = off.get(i).copied().unwrap_or_default();
            for (fx, end) in chases.iter().flat_map(|c| [(100.0 + c, 125.0 + c), (160.0 + c, 135.0 + c)]) {
                let ox = 10_000.0 * i as f64 + dx;
                sp.mep.equipment.push(MepEquipment {
                    tag: "EF".into(),
                    kind: "exhaust_fan".into(),
                    level: (*n).into(),
                    min: [ox + fx - 5.0, 295.0 + dy],
                    max: [ox + fx + 5.0, 305.0 + dy],
                    bottom_ft: 8.0,
                    height_ft: 0.83,
                    ..Default::default()
                });
                let run = serde_json::from_value::<MepRun>(json!({
                    "system": "EA", "level": n, "points": [[ox + fx, 300.0 + dy], [ox + fx, 240.0 + dy], [ox + end, 240.0 + dy]],
                    "size_in": [4.0, 0.0], "bottoms_ft": [8.25, 8.25], "made": "derived", "basis": "derived: the typical room view laid into its room",
                }));
                sp.mep.runs.extend(run.ok());
            }
        }
        for (tag, c) in hoods {
            sp.mep.equipment.push(MepEquipment {
                tag: (*tag).into(),
                kind: "relief_hood".into(),
                level: "Roof".into(),
                min: [c[0] - 15.0, c[1] - 18.0],
                max: [c[0] + 15.0, c[1] + 18.0],
                bottom_ft: 14.0 / 12.0,
                height_ft: 1.5,
                ..Default::default()
            });
        }
        sp
    }

    fn detail(fans: usize, tag: &str) -> RiserDetail {
        RiserDetail {
            sheet: "M-9".into(),
            number: Some("3".into()),
            title: format!("TYPICAL EXHAUST RISER, {fans} FANS"),
            fans: Some(fans),
            terminals: vec![tag.into()],
        }
    }

    fn near(a: [f64; 2], b: [f64; 2]) -> bool {
        dist(a, b) < 1e-6
    }

    #[test]
    fn each_fans_duct_goes_on_up_the_chase_into_the_hood_over_it_when_the_notes_require_it_continuous() {
        // The hood over the chase (20" off it in plan), and one over another chase no fan here reaches.
        let mut sp = building(&[("GRH-7", [40_130.0, 260.0]), ("GRH-7", [40_800.0, 900.0])]);
        let before = sp.mep.runs.len();
        let report = stack_fan_ducts(&mut sp, &[detail(8, "GRH-7")], Some("EACH EXHAUST DUCT CONTINUOUS TO ITS HOOD"));
        assert_eq!(report["fans_stacked"], 8, "{report}");
        assert_eq!(report["risers"], 8, "{report}");
        assert_eq!(report["terminals_no_chase_reaches"].as_array().map(Vec::len), Some(1), "{report}");
        assert!(report["fan_count_not_the_details"].as_array().is_some_and(Vec::is_empty), "{report}");
        let risers: Vec<&MepRun> = sp.mep.runs.iter().filter(|r| r.top_ft.is_some() && r.level != "Roof").collect();
        assert_eq!(risers.len(), 8);
        // Each fan's duct goes on (as it was, then a piece across the chase) to where its own riser
        // stands, every riser in its own place, and each goes up to a foot under the roof deck
        // (roof at 40 ft).
        for r in sp.mep.runs.iter().take(before) {
            let end = r.points.last().copied().unwrap_or_default();
            assert_eq!(r.points.len(), 3, "the fan's duct as it was");
            let on =
                sp.mep.runs.iter().skip(before).find(|c| c.level == r.level && c.top_ft.is_none() && c.points.first().is_some_and(|p| near(*p, end)));
            let at = on.and_then(|c| c.points.last().copied()).unwrap_or(end);
            let mine: Vec<&&MepRun> = risers.iter().filter(|x| x.level == r.level && x.points.first().is_some_and(|p| near(*p, at))).collect();
            assert_eq!(mine.len(), 1, "one riser at the end of the duct on {}: {end:?}", r.level);
            let x = mine.first().unwrap_or_else(|| panic!("riser"));
            let el = sp.level(&x.level).map_or(f64::NAN, |l| l.elevation_ft);
            assert!((x.top_ft.unwrap_or_default() + el - 39.0).abs() < 1e-9, "{:?} on {}", x.top_ft, x.level);
            assert!((x.bottoms_ft.first().copied().unwrap_or_default() - 8.25).abs() < 1e-9);
            assert_eq!(x.made, Made::Derived);
            assert!(x.basis.contains("TYPICAL EXHAUST RISER, 8 FANS") && x.basis.contains("CONTINUOUS"), "{}", x.basis);
        }
        let mut spots: Vec<[f64; 2]> = risers.iter().filter_map(|r| r.points.first()).map(|p| [p[0] % 10_000.0, p[1]]).collect();
        spots.sort_by(|a, b| a[0].total_cmp(&b[0]));
        spots.dedup_by(|a, b| near(*a, *b));
        assert_eq!(spots.len(), 8, "side by side across the chase: {spots:?}");
        // On the roof each duct goes across and up into the hood (its curb's top), within its outline.
        let into: Vec<&MepRun> = sp.mep.runs.iter().filter(|r| r.level == "Roof" && r.top_ft.is_some()).collect();
        assert_eq!(into.len(), 8);
        for r in &into {
            let p = r.points.first().copied().unwrap_or_default();
            assert!(p[0] >= 40_115.0 && p[0] <= 40_145.0 && p[1] >= 242.0 && p[1] <= 278.0, "in the hood: {p:?}");
            assert!((r.top_ft.unwrap_or_default() - 14.0 / 12.0).abs() < 1e-9);
            assert!((r.bottoms_ft.first().copied().unwrap_or_default() + 1.0).abs() < 1e-9, "a foot under the deck");
        }
        // Built again (a rebuild): the same, not twice.
        let n = sp.mep.runs.len();
        let again = stack_fan_ducts(&mut sp, &[detail(8, "GRH-7")], Some("EACH EXHAUST DUCT CONTINUOUS TO ITS HOOD"));
        assert_eq!(again["risers"], 8);
        assert_eq!(sp.mep.runs.len(), n);
    }

    #[test]
    fn without_the_note_the_chase_carries_one_riser_of_the_fans_combined_area_and_says_it_was_inferred() {
        let mut sp = building(&[("GRH-7", [40_130.0, 240.0])]);
        let before = sp.mep.runs.len();
        let report = stack_fan_ducts(&mut sp, &[detail(8, "GRH-7")], None);
        assert_eq!(report["risers"], 1, "{report}");
        let riser = sp.mep.runs.iter().find(|r| r.top_ft.is_some() && r.level != "Roof").unwrap_or_else(|| panic!("riser"));
        assert_eq!(riser.level, "Level 1", "from the lowest storey");
        assert_eq!(riser.size_in[0], 12.0, "eight 4\" ducts: about 11.3\" round, 12\"");
        assert_eq!(riser.made, Made::Inferred);
        assert!(riser.basis.starts_with("trade practice"), "{}", riser.basis);
        // Every fan's duct goes on to the chase, where the riser passes its storey.
        for r in sp.mep.runs.iter().take(before) {
            let end = r.points.last().copied().unwrap_or_default();
            let on =
                sp.mep.runs.iter().skip(before).find(|c| c.level == r.level && c.top_ft.is_none() && c.points.first().is_some_and(|p| near(*p, end)));
            let at = on.and_then(|c| c.points.last().copied()).unwrap_or(end);
            assert!(near([at[0] % 10_000.0, at[1]], [130.0, 240.0]), "{at:?}");
        }
    }

    #[test]
    fn a_storey_or_roof_plan_registered_some_feet_off_still_stacks_its_fans_into_the_chases_under_their_hoods() {
        // Three chases 25 ft apart; Level 2's sheet 6 ft off, Level 3's 4 ft the other way; the
        // roof plan's hoods 20 ft east and 12 ft north of where they stand over the chases (and
        // one more hood over no chase).
        let chases = [0.0, 300.0, 600.0];
        let off = [[0.0, 0.0], [72.0, 10.0], [-48.0, 0.0], [0.0, 0.0]];
        let hoods: Vec<(&str, [f64; 2])> =
            [0.0, 300.0, 600.0, 900.0].iter().map(|c| ("GRH-7", [40_000.0 + 130.0 + c + 240.0, 240.0 + 144.0])).collect();
        let mut sp = building_with(&chases, off, &hoods);
        let report = stack_fan_ducts(&mut sp, &[detail(8, "GRH-7")], Some("CONTINUOUS"));
        assert_eq!(report["chases"], 3, "each chase once, its storeys lined up: {report}");
        assert_eq!(report["fans_stacked"], 24, "{report}");
        assert_eq!(report["chases_by_fan_count"]["8"], 3, "{report}");
        assert_eq!(report["terminals_no_chase_reaches"].as_array().map(Vec::len), Some(1), "{report}");
        // Each chase's ducts go into the hood over it, not the next one along.
        for (k, c) in chases.iter().enumerate() {
            let hood = [40_000.0 + 130.0 + c + 240.0, 384.0];
            let into = sp
                .mep
                .runs
                .iter()
                .filter(|r| r.level == "Roof" && r.top_ft.is_some() && r.points.first().is_some_and(|p| dist(*p, hood) < 30.0))
                .count();
            assert_eq!(into, 8, "chase {k}");
        }
        // Each storey's risers stand in its own drawing of the chase (side by side across it: the
        // ducts come in along x, so along y).
        let l2: Vec<[f64; 2]> =
            sp.mep.runs.iter().filter(|r| r.level == "Level 2" && r.top_ft.is_some()).filter_map(|r| r.points.first().copied()).collect();
        assert_eq!(l2.len(), 6);
        for p in l2 {
            let x = p[0] - 10_000.0 - 72.0;
            assert!(chases.iter().any(|c| (x - 130.0 - c).abs() < 1e-6) && (p[1] - 250.0).abs() <= 20.0, "{p:?}");
        }
    }

    #[test]
    fn fans_far_from_any_hood_and_a_riser_off_its_details_count_are_said() {
        // A hood of another type over the chase: not the detail's terminal.
        let mut sp = building(&[("GRH-9", [40_130.0, 240.0])]);
        let report = stack_fan_ducts(&mut sp, &[detail(8, "GRH-7")], Some("CONTINUOUS"));
        assert_eq!(report["risers"], 0, "{report}");
        // Its own hood over the chase, but the detail says 6 fans.
        let mut sp = building(&[("GRH-7", [40_130.0, 240.0])]);
        let report = stack_fan_ducts(&mut sp, &[detail(6, "GRH-7")], Some("CONTINUOUS"));
        assert_eq!(report["risers"], 8);
        assert_eq!(report["fan_count_not_the_details"].as_array().map(Vec::len), Some(1), "{report}");
        // No detail: nothing is stacked.
        let mut sp = building(&[("GRH-7", [40_130.0, 240.0])]);
        assert_eq!(stack_fan_ducts(&mut sp, &[], Some("CONTINUOUS"))["details"], 0);
        assert!(sp.mep.runs.iter().all(|r| r.top_ft.is_none()));
    }

    /// The documents end to end: the relief hood schedule (one row), the roof plan printing its
    /// tag over each chase, the riser detail tagging it, and a storey's typical fans: the hoods are
    /// built at every tag and the fans' ducts go up into them. Before Q6496 a relief hood was no
    /// kind the schedules knew, so no hood was built and every fan kept its stub of duct.
    /// A plumbing fixture the plan draws (`TUB SHOWER`, 60 by 32 in) centred at `c` on `level`.
    fn tub(level: &str, c: [f64; 2]) -> cadaddy_walk3d::tags::ElementTag {
        let f = |v: &str| cadaddy_walk3d::tags::Field { value: v.into(), source: "plan".into() };
        cadaddy_walk3d::tags::ElementTag {
            id: format!("device:{level}:{},{}", c[0], c[1]),
            kind: "plumbing_fixture".into(),
            level: level.into(),
            at_ft: [c[0] / 12.0, c[1] / 12.0],
            fields: [("fixture".to_string(), f("TUB SHOWER")), ("drawn_size_in".to_string(), f("60.0, 32.0"))].into(),
            ..Default::default()
        }
    }

    /// Whether `p` stands inside `t`'s drawn outline grown by `grow` (inches).
    fn in_tub(t: &cadaddy_walk3d::tags::ElementTag, p: [f64; 2], grow: f64) -> bool {
        let c = [t.at_ft[0] * 12.0, t.at_ft[1] * 12.0];
        (p[0] - c[0]).abs() < 30.0 + grow && (p[1] - c[1]).abs() < 16.0 + grow
    }

    /// Q6550: Level 3's drawing stands 55" off its plan's place, so the levels' base points carry
    /// a riser through it 55" off its chase: into the tub beside the chase. The chases line the
    /// storeys up, and every storey's piece of every riser stands in that storey's own drawing of
    /// the chase, still one riser cut at its floors.
    #[test]
    fn every_storeys_piece_of_a_riser_stands_in_that_storeys_own_chase_not_where_the_base_points_carry_it() {
        // Three chases 25 ft apart, each under its hood.
        let chases = [0.0, 300.0, 600.0];
        let off = [[0.0, 0.0], [0.0, 0.0], [55.0, 0.0], [0.0, 0.0]];
        let hoods: Vec<(&str, [f64; 2])> = chases.iter().map(|c| ("GRH-7", [40_130.0 + c, 260.0])).collect();
        let mut sp = building_with(&chases, off, &hoods);
        // On Level 3 a bath beside its first chase, where the base points put that chase.
        let bath = tub("Level 3", [20_130.0, 240.0]);
        sp.tags.push(bath.clone());
        let report = stack_fan_ducts(&mut sp, &[detail(8, "GRH-7")], Some("CONTINUOUS"));
        assert_eq!(report["chases"], 3, "{report}");
        assert_eq!(report["risers"], 24, "{report}");
        assert_eq!(report["storey_points"], 3 * 2 * (3 + 2 + 1), "{report}");
        assert_eq!(report["moved_clear"], 0, "{report}");
        let risers: Vec<MepRun> = sp.mep.runs.iter().filter(|r| r.top_ft.is_some() && r.level != "Roof").cloned().collect();
        let mut pieces = 0;
        for r in &risers {
            let cut = super::super::risercut::storey_runs(&sp, r);
            assert_eq!(cut.len(), 5 - 1 - sp.levels.iter().position(|l| l.name == r.level).unwrap_or(0), "{}", r.level);
            for s in &cut {
                let i = sp.levels.iter().position(|l| l.name == s.run.level).unwrap_or(9);
                let [dx, dy] = off.get(i).copied().unwrap_or_default();
                let p = s.run.points.first().copied().unwrap_or_default();
                let x = p[0] - 10_000.0 * i as f64 - dx - 130.0;
                assert!(
                    chases.iter().any(|c| (x - c).abs() < 1e-6) && (p[1] - dy - 240.0).abs() <= 20.0,
                    "{} piece of a {} riser: {p:?}",
                    s.run.level,
                    r.level
                );
                assert!(!in_tub(&bath, p, 0.0) || s.run.level != "Level 3", "in the bath: {p:?}");
                if s.run.level != r.level {
                    assert!(s.cut.as_ref().is_some_and(|c| c.floors == vec![s.run.level.clone()]), "through its floor: {:?}", s.cut);
                }
                pieces += 1;
            }
        }
        assert_eq!(pieces, 3 * 2 * (4 + 3 + 2 + 1));
    }

    /// A riser that would stand in a plumbing fixture or a unit on a storey it passes is moved to
    /// the nearest place clear of it there, the drawn place kept beside the corrected one; one
    /// over a fixture on its own storey's floor (it starts at the ceiling), and a damper it runs
    /// through, are no clash.
    #[test]
    fn a_riser_that_would_stand_in_a_tub_or_a_unit_is_moved_clear_and_says_so() {
        let mut sp = building(&[("GRH-7", [40_130.0, 260.0])]);
        // A tub over the chase on Level 1 (under its fans' ducts) and on Level 2; a unit on the
        // floor over it on Level 3; a damper in the chase on Level 2.
        sp.tags.push(tub("Level 1", [130.0, 240.0]));
        let bath = tub("Level 2", [10_130.0, 240.0]);
        sp.tags.push(bath.clone());
        let unit = |kind: &str, level: &str, lo: [f64; 2], hi: [f64; 2], bottom: f64, height: f64| MepEquipment {
            tag: "U-1".into(),
            kind: kind.into(),
            level: level.into(),
            min: lo,
            max: hi,
            bottom_ft: bottom,
            height_ft: height,
            ..Default::default()
        };
        sp.mep.equipment.push(unit("fan_coil_unit", "Level 3", [20_120.0, 200.0], [20_140.0, 250.0], 0.0, 6.0));
        sp.mep.equipment.push(unit("fire_damper", "Level 2", [10_125.0, 230.0], [10_135.0, 250.0], 8.0, 1.0));
        let report = stack_fan_ducts(&mut sp, &[detail(8, "GRH-7")], Some("CONTINUOUS"));
        assert_eq!(report["risers"], 8, "{report}");
        // Level 1's two risers through Level 2's tub, and those of Levels 1 and 2 through Level 3's
        // unit.
        assert_eq!(report["moved_clear"], 2 + 4, "{report}");
        assert!(report["not_cleared"].as_array().is_some_and(Vec::is_empty), "{report}");
        let risers: Vec<MepRun> = sp.mep.runs.iter().filter(|r| r.top_ft.is_some() && r.level != "Roof").cloned().collect();
        for r in &risers {
            // The pieces through a storey from below (a storey's own riser starts at its ceiling).
            for s in super::super::risercut::storey_runs(&sp, r).iter().filter(|s| s.run.level != r.level) {
                let p = s.run.points.first().copied().unwrap_or_default();
                if s.run.level == "Level 2" {
                    assert!(!in_tub(&bath, p, 3.0 - 1e-6), "Level 2 piece of a {} riser in the tub: {p:?}", r.level);
                }
                if s.run.level == "Level 3" {
                    let clear = p[0] <= 20_117.0 + 1e-6 || p[0] >= 20_143.0 - 1e-6 || p[1] <= 197.0 + 1e-6 || p[1] >= 253.0 - 1e-6;
                    assert!(clear, "Level 3 piece of a {} riser in the unit: {p:?}", r.level);
                }
            }
            let keys: Vec<&str> = r.corrections.iter().map(|c| c.key.as_str()).collect();
            match r.level.as_str() {
                "Level 1" => assert_eq!(keys, vec!["plan point on Level 2", "plan point on Level 3"], "{:?}", r.corrections),
                "Level 2" => assert_eq!(keys, vec!["plan point on Level 3"], "{:?}", r.corrections),
                _ => assert!(keys.is_empty(), "{} {:?}", r.level, r.corrections),
            }
            for c in &r.corrections {
                assert_eq!(c.rule, "M-EXHAUST-RISER");
                assert!(c.design != c.corrected && c.design.ends_with(" ft"), "{c:?}");
                let on2 = c.key.ends_with("Level 2");
                assert!(c.reason.contains(if on2 { "tub shower" } else { "fan coil unit U-1" }), "{}", c.reason);
            }
        }
    }

    #[test]
    fn a_typical_riser_detail_stacks_the_typical_rooms_fan_ducts_up_to_the_hoods_the_roof_plan_places() {
        let mut row = BTreeMap::new();
        row.insert("MARK".to_string(), "GRH-7".to_string());
        row.insert("CFM".to_string(), "600".to_string());
        let table = cadaddy_mepfix::equip_reader::Table {
            sheet: "M-8".into(),
            title: "RELIEF HOOD SCHEDULE".into(),
            columns: vec!["MARK".into(), "CFM".into()],
            rows: vec![row],
        };
        let sched = cadaddy_mepfix::equip_reader::scheduled(&[table]);
        assert_eq!(sched.first().map(|x| x.kind.as_str()), Some("relief_hood"), "{sched:?}");
        // The roof plan (its own sheet, inches): GRH over 7 by each of two hoods, 30" x 36".
        let mut roof = Drawing::default();
        for (x, y) in [(130.0, 260.0), (800.0, 900.0)] {
            rect(&mut roof, [x - 15.0, y - 18.0], [x + 15.0, y + 18.0]);
            text(&mut roof, "GRH", x + 20.0, y + 22.0, 6.0);
            text(&mut roof, "7", x + 22.0, y + 12.0, 6.0);
        }
        let sheet = super::super::printmep::SheetRef {
            number: "M-7".into(),
            level: "R".into(),
            discipline: "M".into(),
            kind: "plan".into(),
            rect: [0.0, 0.0, 2000.0, 2000.0],
            ..Default::default()
        };
        let plan = super::super::schedequip::PlanIn { sheet: &sheet, drawing: &roof, sheet_upf: 12.0, upf: 12.0 };
        let mut sp = building(&[]);
        let mut read = super::super::schedequip::read_scheduled(&sched, std::slice::from_ref(&plan), &sp.mep.equipment, &sp.levels);
        // The roof's plan is its own place in the drawing.
        read.choose(|e| {
            for p in [&mut e.min, &mut e.max] {
                p[0] += 40_000.0;
            }
            true
        });
        let hoods: Vec<&MepEquipment> = read.equipment.iter().filter(|e| e.kind == "relief_hood").collect();
        assert_eq!(hoods.len(), 2, "a hood at every tag: {:?}", read.equipment.iter().map(|e| (&e.tag, &e.kind)).collect::<Vec<_>>());
        sp.mep.equipment.extend(read.equipment);
        let page = details_page(1.0, "TYPICAL EXHAUST RISER, 8 FANS", "GRH", "7");
        let details = riser_details(&[("M-9".to_string(), page)], &["GRH-7".to_string()]);
        let report = stack_fan_ducts(&mut sp, &details, Some("EACH EXHAUST DUCT CONTINUOUS TO ITS HOOD"));
        assert_eq!(report["fans_stacked"], 8, "{report}");
        assert_eq!(sp.mep.runs.iter().filter(|r| r.level == "Roof" && r.top_ft.is_some()).count(), 8);
    }
}
