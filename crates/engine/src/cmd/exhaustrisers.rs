//! Typical exhaust risers (Q6496): a set often draws a guest bathroom's exhaust only on its
//! typical room view (the fan and its small duct to the wall chase) and on a riser detail
//! (`TYPICAL RISER OF 8 EXHAUST FANS`: the fans of the rooms stacked over each other, their ducts
//! up the chase to the roof terminal it tags, `RH-1`). The floor plans only key one instance; the
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
    /// The sheet it is on and its number there (`M-6`, `3`).
    pub sheet: String,
    pub number: Option<String>,
    pub title: String,
    /// How many fans its title says it carries (`OF 8 EXHAUST FANS`).
    pub fans: Option<usize>,
    /// The roof terminals it tags (of the terminals the job has: `RH-1`).
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
                to_riser(f, slot_f, Made::Derived, &basis, &mut new_runs);
                new_runs.extend(piece(&f.level, vec![slot_f], f.bottom_ft, Some(top), f.size_in, Made::Derived, &basis));
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
            for fi in &order {
                let Some(f) = fans.get(*fi) else { continue };
                if let Some(at) = to_level(*chase, &f.level) {
                    to_riser(f, at, Made::Inferred, &basis, &mut new_runs);
                    stacked += 1;
                }
            }
            let Some(at) = to_level(*chase, &low.level) else { continue };
            let top = tz - clear_ft - lz;
            if top > low.bottom_ft {
                new_runs.extend(piece(&low.level, vec![at], low.bottom_ft, Some(top), size, Made::Inferred, &basis));
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
            let page = details_page(k, "TYPICAL RISER OF 8 EXHAUST FANS", "RH", "1");
            let tags = vec!["RH-1".to_string(), "RH-2".to_string()];
            let found = riser_details(&[("M-9".to_string(), page)], &tags);
            assert_eq!(found.len(), 1, "only the riser detail, at {k}: {found:?}");
            let r = found.first().unwrap_or_else(|| panic!("detail"));
            assert_eq!(r.fans, Some(8));
            assert_eq!(r.terminals, vec!["RH-1".to_string()]);
            assert_eq!(r.number.as_deref(), Some("3"));
        }
        // A supply riser or an exhaust plan is no exhaust riser.
        for title in ["TYPICAL SUPPLY RISER", "EXHAUST PLAN"] {
            let page = details_page(1.0, title, "RH", "1");
            assert!(riser_details(&[("M-9".to_string(), page)], &["RH-1".to_string()]).is_empty(), "{title}");
        }
    }

    #[test]
    fn a_note_requiring_continuous_exhaust_ducts_is_found() {
        let mut d = Drawing::default();
        text(&mut d, "GALVANIZED STEEL. EXHAUST DUCT SHALL BE CONTINUOUS FROM", 0.0, 0.0, 3.0);
        assert!(continuous_required([&d]).is_some_and(|n| n.contains("CONTINUOUS")));
        // Its lines separate texts, a space lost after a word.
        let mut lines = Drawing::default();
        text(&mut lines, "RESTROOM EXHAUST DUCTS SHALL BE IN WALLS. THE EXHAUST", 0.0, 10.0, 3.0);
        text(&mut lines, "DUCT SHALL BE CONTINUOUSFROM THE FAN TO THE OUTSIDE", 0.0, 5.0, 3.0);
        assert!(continuous_required([&lines]).is_some());
        // Only the note's own lines: not the title block beside it.
        let mut boxed = Drawing::default();
        text(&mut boxed, "GALVANIZED STEEL. EXHAUST DUCT SHALL BE CONTINUOUSFROM", 0.0, 10.0, 3.0);
        text(&mut boxed, "1 SAMPLE STREET", 120.0, 9.0, 2.0);
        text(&mut boxed, "THE FAN TO THE OUTSIDE. EXHAUST DUCT", 0.0, 5.0, 3.0);
        assert_eq!(continuous_required([&boxed]).as_deref(), Some("EXHAUST DUCT SHALL BE CONTINUOUSFROM THE FAN TO THE OUTSIDE"));
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
            title: format!("TYPICAL RISER OF {fans} EXHAUST FANS"),
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
        let mut sp = building(&[("RH-1", [40_130.0, 260.0]), ("RH-1", [40_800.0, 900.0])]);
        let before = sp.mep.runs.len();
        let report = stack_fan_ducts(&mut sp, &[detail(8, "RH-1")], Some("EXHAUST DUCT SHALL BE CONTINUOUS"));
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
            assert!(x.basis.contains("TYPICAL RISER OF 8 EXHAUST FANS") && x.basis.contains("CONTINUOUS"), "{}", x.basis);
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
        let again = stack_fan_ducts(&mut sp, &[detail(8, "RH-1")], Some("EXHAUST DUCT SHALL BE CONTINUOUS"));
        assert_eq!(again["risers"], 8);
        assert_eq!(sp.mep.runs.len(), n);
    }

    #[test]
    fn without_the_note_the_chase_carries_one_riser_of_the_fans_combined_area_and_says_it_was_inferred() {
        let mut sp = building(&[("RH-1", [40_130.0, 240.0])]);
        let before = sp.mep.runs.len();
        let report = stack_fan_ducts(&mut sp, &[detail(8, "RH-1")], None);
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
            [0.0, 300.0, 600.0, 900.0].iter().map(|c| ("RH-1", [40_000.0 + 130.0 + c + 240.0, 240.0 + 144.0])).collect();
        let mut sp = building_with(&chases, off, &hoods);
        let report = stack_fan_ducts(&mut sp, &[detail(8, "RH-1")], Some("CONTINUOUS"));
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
        let mut sp = building(&[("RH-2", [40_130.0, 240.0])]);
        let report = stack_fan_ducts(&mut sp, &[detail(8, "RH-1")], Some("CONTINUOUS"));
        assert_eq!(report["risers"], 0, "{report}");
        // Its own hood over the chase, but the detail says 6 fans.
        let mut sp = building(&[("RH-1", [40_130.0, 240.0])]);
        let report = stack_fan_ducts(&mut sp, &[detail(6, "RH-1")], Some("CONTINUOUS"));
        assert_eq!(report["risers"], 8);
        assert_eq!(report["fan_count_not_the_details"].as_array().map(Vec::len), Some(1), "{report}");
        // No detail: nothing is stacked.
        let mut sp = building(&[("RH-1", [40_130.0, 240.0])]);
        assert_eq!(stack_fan_ducts(&mut sp, &[], Some("CONTINUOUS"))["details"], 0);
        assert!(sp.mep.runs.iter().all(|r| r.top_ft.is_none()));
    }

    /// The documents end to end: the relief hood schedule (one row), the roof plan printing its
    /// tag over each chase, the riser detail tagging it, and a storey's typical fans: the hoods are
    /// built at every tag and the fans' ducts go up into them. Before Q6496 a relief hood was no
    /// kind the schedules knew, so no hood was built and every fan kept its stub of duct.
    #[test]
    fn a_typical_riser_detail_stacks_the_typical_rooms_fan_ducts_up_to_the_hoods_the_roof_plan_places() {
        let mut row = BTreeMap::new();
        row.insert("MARK".to_string(), "RH-1".to_string());
        row.insert("CFM".to_string(), "440".to_string());
        let table = cadaddy_mepfix::equip_reader::Table {
            sheet: "M-1".into(),
            title: "RELIEF HOOD SCHEDULE".into(),
            columns: vec!["MARK".into(), "CFM".into()],
            rows: vec![row],
        };
        let sched = cadaddy_mepfix::equip_reader::scheduled(&[table]);
        assert_eq!(sched.first().map(|x| x.kind.as_str()), Some("relief_hood"), "{sched:?}");
        // The roof plan (its own sheet, inches): RH over 1 by each of two hoods, 30" x 36".
        let mut roof = Drawing::default();
        for (x, y) in [(130.0, 260.0), (800.0, 900.0)] {
            rect(&mut roof, [x - 15.0, y - 18.0], [x + 15.0, y + 18.0]);
            text(&mut roof, "RH", x + 20.0, y + 22.0, 6.0);
            text(&mut roof, "1", x + 22.0, y + 12.0, 6.0);
        }
        let sheet = super::super::printmep::SheetRef {
            number: "M-4".into(),
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
        let page = details_page(1.0, "TYPICAL RISER OF 8 EXHAUST FANS", "RH", "1");
        let details = riser_details(&[("M-6".to_string(), page)], &["RH-1".to_string()]);
        let report = stack_fan_ducts(&mut sp, &details, Some("EXHAUST DUCT SHALL BE CONTINUOUS"));
        assert_eq!(report["fans_stacked"], 8, "{report}");
        assert_eq!(sp.mep.runs.iter().filter(|r| r.level == "Roof" && r.top_ft.is_some()).count(), 8);
    }
}
