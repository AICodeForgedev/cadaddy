//! Air terminals in the ceiling plane, and the ducts that serve them above it (Q6341; trade
//! practice `M-TERMINAL-CEILING` in `tradepractice.json`).
//!
//! A ceiling diffuser, register or grille is set in the finished ceiling of the room it is in:
//! its face is flush with that ceiling, never over it in the plenum or hanging under it. The
//! ceiling is the one built over the terminal (its room's, at the height the reflected ceiling
//! plans, unit RCPs, legend or sections give, else derived; an RCP area's), so the terminal
//! follows its own room's ceiling and not its level's. A duct the documents give no height (a
//! default, an estimated route, a branch inferred to a terminal) runs in the plenum, at least the
//! rule's clearance over the highest built ceiling it passes under; it never hangs below a
//! finished ceiling.
//!
//! Nor does it ever run up in the structure over it (Q6554): its top stays under the underside of
//! the floor or roof framing over its level (the next storey's floor framing and deck as its
//! notes give them, else the roof's framing; where nothing frames it, the next floor's slab), so a
//! duct with no height of its own above that is brought down under it. Where the plenum is too
//! shallow for it (the ceiling's clearance and its depth past the structure's underside) the
//! structure wins, as the field hangs it: under the structure, below the ceiling, marked for
//! review (a soffit, a flatter duct or a lower ceiling settles it).
//!
//! Where no ceiling is built on a level the build laid ceilings out on (a space no room was
//! traced in), the ceiling is its level's, as model.clashes measures it. What stays: an area the reflected ceiling plans leave open to the
//! structure is exposed, so what is in it keeps its height. A terminal kind the rule names as not
//! set in a ceiling (a sidewall register, a floor grille) keeps its height, and so does a height
//! an edit set (its `bottom_ft` correction). A duct height a document gives stays too
//! (model.clashes reports one below the ceiling), as do risers, the roof's runs and a run with no
//! height source (a spec written by hand gives its heights).
//!
//! model.build runs [`seat`] once the ceilings are built and before the MEP is laid out, so every
//! build seats them in the ceilings as built. Nothing here panics; every scan is capped.

use std::collections::BTreeMap;

use cadaddy_doc::Drawing;
use cadaddy_geom::Vec2;
use cadaddy_walk3d::{BuildingSpec, MepKind};
use serde_json::{Value, json};

/// The trade practice rule this module follows.
pub(crate) const RULE: &str = "M-TERMINAL-CEILING";
/// Most terminals and runs looked at (hostile specs), and most listed in a report.
const MAX_ITEMS: usize = 200_000;
const MAX_LISTED: usize = 40;
/// How far apart (feet) a run is looked at along its length for the ceilings it passes under.
const STEP_FT: f64 = 2.0;
/// Points looked at along one segment, at most.
const MAX_STEPS: usize = 500;
/// A height this close (feet) is where it should be.
const SLACK_FT: f64 = 0.25 / 12.0;
/// What a duct too deep for its plenum says of its height: hung under the structure.
const TIGHT: &str = "under the structure: too deep for the plenum over the ceiling";

/// The rule's numbers and words.
pub(crate) struct Rule {
    /// How far over the ceiling a duct's bottom is at least (feet).
    pub clear_ft: f64,
    /// Words that name a terminal set in a ceiling (upper case).
    pub terminal_words: Vec<String>,
    /// Words that name one that is not (upper case).
    pub not_ceiling_words: Vec<String>,
}

impl Rule {
    pub(crate) fn load() -> Rule {
        let data = super::provenance::rule_data(RULE);
        let words = |key: &str| -> Vec<String> {
            data.get(key)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(|w| w.trim().to_ascii_uppercase()).filter(|w| !w.is_empty()).take(100).collect())
                .unwrap_or_default()
        };
        let clear = super::provenance::rule(RULE).clear_ft;
        Rule {
            clear_ft: if clear.is_finite() && clear >= 0.0 { clear.min(5.0) } else { 0.0 },
            terminal_words: words("terminal_words"),
            not_ceiling_words: words("not_ceiling_words"),
        }
    }

    /// Whether a terminal of `kind` (`return_grille`, `supply_diffuser`) is set in a ceiling.
    pub(crate) fn in_ceiling(&self, kind: &str) -> bool {
        let words: Vec<String> =
            kind.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect();
        // A rule word (or words, in order) among the kind's words.
        let has = |w: &String| {
            let parts: Vec<&str> = w.split_whitespace().collect();
            !parts.is_empty() && words.windows(parts.len()).any(|win| win.iter().zip(&parts).all(|(a, b)| a == b))
        };
        !kind.to_ascii_lowercase().contains("sprinkler") && self.terminal_words.iter().any(has) && !self.not_ceiling_words.iter().any(has)
    }
}

/// What [`seat`] moved.
#[derive(Default, Debug)]
pub(crate) struct Seated {
    /// Ceiling terminals looked at (with a ceiling over them: built, else their level's).
    pub terminals_checked: usize,
    /// Terminals set into the ceiling over them.
    pub terminals: usize,
    /// Duct runs with no height of their own raised over the ceilings they pass under.
    pub runs: usize,
    /// Duct runs with no height of their own brought down out of the structure over them.
    pub lowered: usize,
    /// Of the ducts moved, those the plenum is too shallow for: hung under the structure, below
    /// the ceiling's clearance, marked for review.
    pub tight: usize,
    /// Per level: terminals and runs moved.
    pub by_level: BTreeMap<String, [usize; 2]>,
    /// Some of what moved: kind, level, plan point, from and to (feet over the floor).
    pub listed: Vec<Value>,
}

impl Seated {
    pub(crate) fn moved(&self) -> usize {
        self.terminals + self.runs + self.lowered
    }

    pub(crate) fn json(&self) -> Value {
        json!({
            "rule": RULE,
            "terminals_checked": self.terminals_checked,
            "terminals_set_in_ceiling": self.terminals,
            "ducts_raised_over_ceiling": self.runs,
            "ducts_lowered_under_structure": self.lowered,
            "ducts_too_deep_for_plenum": self.tight,
            "by_level": self.by_level.iter().map(|(l, [t, r])| (l.clone(), json!({"terminals": t, "ducts": r}))).collect::<serde_json::Map<_, _>>(),
            "listed": self.listed,
        })
    }

    fn list(&mut self, what: &str, level: &str, at: [f64; 2], from: f64, to: f64) {
        if self.listed.len() < MAX_LISTED {
            let r2 = |v: f64| (v * 100.0).round() / 100.0;
            self.listed.push(json!({"what": what, "level": level, "at": [r2(at[0]), r2(at[1])], "from_ft": r2(from), "to_ft": r2(to)}));
        }
    }
}

/// Whether a run's height is one the build chose, none the documents give: a default, an
/// estimated route, a branch inferred by a rule (`inferred (…)` as an older build wrote it). A
/// run with no height source at all keeps its heights (a spec written by hand gives them).
fn no_document_height(source: &str) -> bool {
    let first = source.split(['(', ';']).next().unwrap_or_default().trim().replace(' ', "-").to_ascii_lowercase();
    matches!(first.as_str(), "default" | "estimated-route" | super::waterfill::INFERRED)
}

/// Each ceiling terminal of `sp` set in the ceiling built over it in `d` (where none is, its
/// level's), and each duct with no height of its own raised over the ceilings it passes under
/// (the rule's clearance); nothing in an area left open to the structure.
pub(crate) fn seat(d: &Drawing, sp: &mut BuildingSpec) -> Seated {
    let ceilings = super::propfill::built_ceilings_on_plans(d);
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let floors: BTreeMap<String, (String, f64)> = sp
        .levels
        .iter()
        .filter(|l| !super::wallgen::roof_level(&sp.levels, &l.name))
        .map(|l| (l.name.to_ascii_lowercase(), (l.name.clone(), l.elevation_ft * upf)))
        .collect();
    // The underside of the structure over each level, feet over its floor.
    let heights = super::ceilheight::Rule::load();
    let structure: BTreeMap<String, f64> =
        floors.values().filter_map(|(name, _)| structure_under(sp, name, &heights).map(|h| (name.to_ascii_lowercase(), h))).collect();
    // The reflected ceiling plans' areas open to the structure (an exposed ceiling type).
    let mut open: BTreeMap<String, Vec<Vec<Vec2>>> = BTreeMap::new();
    for c in sp.rcp.ceilings.iter().take(MAX_ITEMS) {
        let exposed = c
            .ceiling_type
            .as_ref()
            .and_then(|t| sp.ceiling_types.iter().find(|ct| ct.name.eq_ignore_ascii_case(t)))
            .is_some_and(|ct| ct.kind == cadaddy_walk3d::CeilingKind::Open);
        if exposed && c.outline.len() >= 3 {
            open.entry(c.level.to_ascii_lowercase()).or_default().push(c.outline.iter().take(10_000).map(|q| Vec2::new(q[0], q[1])).collect());
        }
    }
    // A level the build laid ceilings out on: a point none covers is in a space no room was
    // traced in. (A level with none built has nothing to measure against.)
    let built_on: std::collections::BTreeSet<String> = ceilings.iter().filter_map(|c| c.level()).map(str::to_ascii_lowercase).collect();
    let level_ceiling: BTreeMap<String, f64> = sp
        .levels
        .iter()
        .filter(|l| built_on.contains(&l.name.to_ascii_lowercase()))
        .filter(|l| l.ceiling_ft.is_finite() && l.ceiling_ft > 6.0 && (!l.height_ft.is_finite() || l.ceiling_ft < l.height_ft))
        .map(|l| (l.name.to_ascii_lowercase(), l.ceiling_ft))
        .collect();
    let at = |level: &str, p: [f64; 2]| -> Option<f64> {
        let key = level.to_ascii_lowercase();
        let (name, floor) = floors.get(&key)?;
        let q = Vec2::new(p[0], p[1]);
        // An area the reflected ceiling plans leave open to the structure is exposed (the panel
        // built there is no finished ceiling).
        if open.get(&key).is_some_and(|areas| areas.iter().any(|a| cadaddy_geom::point_in_polygon(a, q))) {
            return None;
        }
        if let Some(inches) = super::propfill::ceiling_over(&ceilings, name, q, *floor, upf) {
            return Some(inches / 12.0);
        }
        // Where no ceiling is built (no room traced there), the level's, as model.clashes measures it.
        level_ceiling.get(&key).copied()
    };
    seat_with(sp, &Rule::load(), &at, &|level: &str| structure.get(&level.to_ascii_lowercase()).copied())
}

/// The underside of the structure over `level`, feet over its floor: its framing's as the
/// documents give it ([`super::ceilheight::structure_over`]), else under the next floor's slab.
pub(crate) fn structure_under(sp: &BuildingSpec, level: &str, heights: &super::ceilheight::Rule) -> Option<f64> {
    let lv = sp.level(level)?;
    let h = match super::ceilheight::structure_over(sp, level, heights) {
        Some((s, _)) => s - lv.elevation_ft,
        None => lv.height_ft - super::mep::SLAB_FT,
    };
    (h.is_finite() && h > 0.0 && lv.height_ft.is_finite() && h <= lv.height_ft).then_some(h)
}

/// [`seat`] with the ceiling over a plan point of a level (feet over its floor; None: no ceiling
/// built there) given by `ceiling`, and the underside of the structure over a level (feet over
/// its floor; None: nothing known) by `structure`.
pub(crate) fn seat_with(
    sp: &mut BuildingSpec,
    rule: &Rule,
    ceiling: &dyn Fn(&str, [f64; 2]) -> Option<f64>,
    structure: &dyn Fn(&str) -> Option<f64>,
) -> Seated {
    let mut out = Seated::default();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    for t in sp.mep.terminals.iter_mut().take(MAX_ITEMS) {
        if !rule.in_ceiling(&t.terminal_kind) || !t.bottom_ft.is_finite() || t.corrections.iter().any(|c| c.key == "bottom_ft") {
            continue;
        }
        let Some(c) = ceiling(&t.level, t.at).filter(|c| c.is_finite() && *c > 0.0) else { continue };
        out.terminals_checked += 1;
        if (t.bottom_ft - c).abs() > SLACK_FT {
            out.list(&t.terminal_kind, &t.level, [t.at[0] / upf, t.at[1] / upf], t.bottom_ft, c);
            t.bottom_ft = c;
            out.terminals += 1;
            out.by_level.entry(t.level.clone()).or_default()[0] += 1;
        }
    }
    let ducts: Vec<String> = sp.mep.systems.iter().filter(|s| s.kind == MepKind::Duct).map(|s| s.name.clone()).collect();
    let step = STEP_FT * upf;
    for r in sp.mep.runs.iter_mut().take(MAX_ITEMS) {
        if !ducts.contains(&r.system) || r.top_ft.is_some() || r.points.len() < 2 || !no_document_height(&r.height_source) {
            continue;
        }
        // The highest ceiling it passes under.
        let mut high: Option<f64> = None;
        for w in r.points.windows(2) {
            let len = (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1]);
            let n = if len.is_finite() { ((len / step).ceil() as usize).clamp(1, MAX_STEPS) } else { 1 };
            for i in 0..=n {
                let f = i as f64 / n as f64;
                let p = [w[0][0] + (w[1][0] - w[0][0]) * f, w[0][1] + (w[1][1] - w[0][1]) * f];
                if let Some(c) = ceiling(&r.level, p).filter(|c| c.is_finite() && *c > 0.0) {
                    high = Some(high.map_or(c, |h: f64| h.max(c)));
                }
            }
        }
        // Its bottom over the ceiling by the rule's clearance, its top under the structure.
        let depth = if r.size_in[1] > 0.0 { r.size_in[1] } else { r.size_in[0] };
        let depth = if depth.is_finite() { depth.max(0.0) / 12.0 } else { 0.0 };
        let low = high.map(|c| c + rule.clear_ft);
        let cap = structure(&r.level).filter(|s| s.is_finite() && *s > 0.0).map(|s| (s - depth).max(0.0));
        if low.is_none() && cap.is_none() {
            continue;
        }
        let tight = matches!((low, cap), (Some(l), Some(c)) if l > c + SLACK_FT);
        let (mut from, mut to, mut up, mut down) = (f64::NAN, f64::NAN, false, false);
        for b in r.bottoms_ft.iter_mut().filter(|b| b.is_finite()) {
            let mut seated = *b;
            if let Some(l) = low.filter(|l| seated < l - SLACK_FT) {
                seated = l;
            }
            // The structure wins: a duct never runs up in the floor or roof framing.
            if let Some(c) = cap.filter(|c| seated > c + SLACK_FT) {
                seated = c;
            }
            if (seated - *b).abs() > SLACK_FT {
                up |= seated > *b;
                down |= seated < *b;
                if from.is_nan() {
                    (from, to) = (*b, seated);
                }
                *b = seated;
            }
        }
        if !(up || down) {
            continue;
        }
        let first = r.points.first().copied().unwrap_or_default();
        let what = if tight { format!("{} duct (too deep for the plenum: under the structure)", r.system) } else { format!("{} duct", r.system) };
        out.list(&what, &r.level, [first[0] / upf, first[1] / upf], from, to);
        if up {
            out.runs += 1;
        } else {
            out.lowered += 1;
        }
        if tight {
            out.tight += 1;
            r.reviewed = false;
            if !r.height_source.contains(TIGHT) {
                r.height_source = format!("{} ({TIGHT})", r.height_source.trim());
            }
        }
        out.by_level.entry(r.level.clone()).or_default()[1] += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule() -> Rule {
        let r = Rule::load();
        assert!(r.clear_ft > 0.0 && !r.terminal_words.is_empty() && !r.not_ceiling_words.is_empty(), "the rule is in the trade practice data");
        r
    }

    #[test]
    fn the_terminal_kinds_set_in_a_ceiling_are_the_rules_words() {
        let r = rule();
        for k in ["return_grille", "exhaust_grille", "supply_diffuser", "register", "Linear Slot Diffuser"] {
            assert!(r.in_ceiling(k), "{k}");
        }
        for k in ["sidewall_register", "side wall grille", "floor_register", "door_grille", "vav_box", "sprinkler_pendent", "mixing_box", "grilled"] {
            assert!(!r.in_ceiling(k), "{k}");
        }
        assert!(no_document_height("default") && no_document_height("estimated route") && no_document_height("inferred"));
        assert!(!no_document_height(""), "a hand-written spec's height");
        assert!(no_document_height("inferred (capped under the floor above)"));
        assert!(!no_document_height("tag") && !no_document_height("derived") && !no_document_height("override") && !no_document_height("proposed"));
    }

    /// Q6341: two rooms with ceilings of their own (7'-10" and 8'-6") under a level whose
    /// ceiling is a guess (8'-6"); grilles read at the level's ceiling, a branch duct inferred off
    /// a main at the main's height under the higher ceiling.
    #[test]
    fn ceiling_terminals_sit_in_their_rooms_ceiling_and_ducts_with_no_height_run_over_it() {
        let mut sp: BuildingSpec = serde_json::from_value(json!({
            "units_per_foot": 12.0,
            "levels": [{"name": "Level 1", "elevation_ft": 0.0, "height_ft": 11.0, "ceiling_ft": 8.5}],
            "mep": {
                "systems": [
                    {"name": "RA", "layer": "M-RETURN", "kind": "duct", "size_in": [12, 10], "bottom_ft": 9.0},
                    {"name": "EA", "layer": "M-EXHAUST", "kind": "duct", "size_in": [12, 0], "bottom_ft": 9.0},
                    {"name": "CW", "layer": "P-CW", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 7.0}
                ],
                "terminals": [
                    {"system": "RA", "level": "Level 1", "at": [60, 60], "size_in": [12, 12], "bottom_ft": 8.5, "terminal_kind": "return_grille"},
                    {"system": "EA", "level": "Level 1", "at": [300, 60], "size_in": [10, 10], "bottom_ft": 8.0, "terminal_kind": "exhaust_grille"},
                    {"system": "RA", "level": "Level 1", "at": [70, 60], "size_in": [12, 6], "bottom_ft": 7.0, "terminal_kind": "sidewall_register"},
                    {"system": "RA", "level": "Level 1", "at": [80, 60], "size_in": [12, 12], "bottom_ft": 9.5, "terminal_kind": "vav_box"},
                    {"system": "RA", "level": "Level 1", "at": [900, 60], "size_in": [12, 12], "bottom_ft": 9.0, "terminal_kind": "return_grille"},
                    {"system": "RA", "level": "Level 1", "at": [90, 60], "size_in": [12, 12], "bottom_ft": 8.25, "terminal_kind": "return_grille",
                     "corrections": [{"key": "bottom_ft", "design": "8.50 ft", "corrected": "8.25 ft", "reason": "an edit", "rule": "edit"}]}
                ],
                "runs": [
                    {"system": "EA", "level": "Level 1", "points": [[300, 60], [300, 200]], "size_in": [12, 0], "bottoms_ft": [8.08], "height_source": "inferred", "made": "inferred"},
                    {"system": "RA", "level": "Level 1", "points": [[20, 100], [400, 100]], "size_in": [12, 10], "bottoms_ft": [7.5], "height_source": "default"},
                    {"system": "RA", "level": "Level 1", "points": [[20, 110], [400, 110]], "size_in": [12, 10], "bottoms_ft": [7.5], "height_source": "tag"},
                    {"system": "EA", "level": "Level 1", "points": [[300, 60], [300, 200]], "size_in": [12, 0], "bottoms_ft": [9.5], "height_source": "inferred"},
                    {"system": "EA", "level": "Level 1", "points": [[900, 60], [900, 200]], "size_in": [12, 0], "bottoms_ft": [6.0], "height_source": "default"},
                    {"system": "CW", "level": "Level 1", "points": [[20, 120], [400, 120]], "size_in": [1, 0], "bottoms_ft": [7.0], "height_source": "default"}
                ]
            }
        }))
        .unwrap();
        // Room A (x 0..240) at 7'-10", room B (x 240..480) at 8'-6"; past x 480 no ceiling.
        let ceiling = |level: &str, p: [f64; 2]| -> Option<f64> {
            (level == "Level 1" && (0.0..=240.0).contains(&p[1])).then_some(())?;
            if (0.0..240.0).contains(&p[0]) {
                Some(94.0 / 12.0)
            } else if (240.0..=480.0).contains(&p[0]) {
                Some(8.5)
            } else {
                None
            }
        };
        let got = seat_with(&mut sp, &rule(), &ceiling, &|_: &str| None);
        let t: Vec<f64> = sp.mep.terminals.iter().map(|t| (t.bottom_ft * 1000.0).round() / 1000.0).collect();
        // Down into room A's ceiling from the level's, up into room B's; the sidewall register, the
        // VAV box over the ceiling, a grille in the exposed space and a height an edit set stay.
        assert_eq!(t, vec![7.833, 8.5, 7.0, 9.5, 9.0, 8.25]);
        let b: Vec<f64> = sp.mep.runs.iter().map(|r| (r.bottoms_ft[0] * 1000.0).round() / 1000.0).collect();
        // The inferred branch over room B's 8'-6" (and the clearance); the default main over the
        // highest it passes under (room B's); a tagged height, one already over the ceiling, one
        // in the exposed space and a pipe stay.
        assert_eq!(b, vec![8.75, 8.75, 7.5, 9.5, 6.0, 7.0]);
        assert_eq!((got.terminals, got.runs, got.terminals_checked), (2, 2, 2));
        assert_eq!(got.json()["by_level"]["Level 1"], json!({"terminals": 2, "ducts": 2}));
        let again = seat_with(&mut sp, &rule(), &ceiling, &|_: &str| None);
        assert_eq!(again.moved(), 0, "once");
    }

    /// Q6554: an 11'-0" storey under a floor framed with 22" joists and the usual deck (the
    /// structure's underside 9'-0 1/2" over the floor), an 8'-0" ceiling. Ducts no document gives a
    /// height (an inferred branch, a default main, one at the next floor's slab) stay under the
    /// structure, over the ceiling; one too deep for that plenum hangs under the structure, marked
    /// for review; a tagged height and a riser keep theirs.
    #[test]
    fn ducts_with_no_height_of_their_own_stay_under_the_floor_framing_over_them() {
        let sp: BuildingSpec = serde_json::from_value(json!({
            "units_per_foot": 12.0,
            "levels": [
                {"name": "Level 1", "elevation_ft": 0.0, "height_ft": 11.0, "ceiling_ft": 8.0},
                {"name": "Level 2", "elevation_ft": 11.0, "height_ft": 10.0, "ceiling_ft": 8.0}
            ],
            "floor_framing": [{"level": "Level 2", "member": "joist", "spacing_in": 16, "depth_in": 22}],
            "mep": {
                "systems": [{"name": "RA", "layer": "M-RETURN", "kind": "duct", "size_in": [18, 18], "bottom_ft": 9.0}],
                "runs": [
                    {"system": "RA", "level": "Level 1", "points": [[0, 0], [240, 0]], "size_in": [6, 0], "bottoms_ft": [9.5], "height_source": "inferred", "made": "inferred"},
                    {"system": "RA", "level": "Level 1", "points": [[0, 50], [240, 50]], "size_in": [10, 8], "bottoms_ft": [8.0], "height_source": "default"},
                    {"system": "RA", "level": "Level 1", "points": [[0, 100], [240, 100]], "size_in": [18, 18], "bottoms_ft": [9.0], "height_source": "inferred", "made": "inferred"},
                    {"system": "RA", "level": "Level 1", "points": [[0, 150], [240, 150]], "size_in": [10, 8], "bottoms_ft": [9.5], "height_source": "tag"},
                    {"system": "RA", "level": "Level 1", "points": [[0, 200]], "size_in": [10, 8], "bottoms_ft": [0.0], "top_ft": 21.0, "height_source": "default"}
                ]
            }
        }))
        .unwrap();
        let heights = super::super::ceilheight::Rule::load();
        let under = structure_under(&sp, "Level 1", &heights).unwrap_or_else(|| panic!("the framing over Level 1 is known"));
        // 11'-0" less the 22" joists and the rule's deck.
        assert!((under - (11.0 - (22.0 + heights.deck_in) / 12.0)).abs() < 1e-9, "{under}");
        assert_eq!(structure_under(&sp, "Level 2", &heights), Some(9.5), "nothing framed over it: under the next floor's slab");
        let ceiling = |level: &str, _: [f64; 2]| (level == "Level 1").then_some(8.0);
        let structure = |level: &str| structure_under(&sp, level, &heights);
        let mut seated = sp.clone();
        let got = seat_with(&mut seated, &rule(), &ceiling, &structure);
        let tops: Vec<f64> =
            seated.mep.runs.iter().map(|r| r.bottoms_ft[0] * 12.0 + if r.size_in[1] > 0.0 { r.size_in[1] } else { r.size_in[0] }).collect();
        let bottoms: Vec<f64> = seated.mep.runs.iter().map(|r| (r.bottoms_ft[0] * 1200.0).round() / 100.0).collect();
        let under_in = (under * 1200.0).round() / 100.0;
        // The 6" branch was up in the joists (top 120"): brought down under them. The default
        // main over the 8'-0" ceiling by the clearance (99"). The 18x18 main can't fit between
        // the ceiling (99") and the structure: hung under the structure. The tagged run and
        // the riser stay.
        assert_eq!(bottoms, vec![under_in - 6.0, 99.0, under_in - 18.0, 114.0, 0.0], "{tops:?}");
        for (i, t) in tops.iter().enumerate().take(3) {
            assert!(*t <= under_in + 1e-6, "run {i} top {t} in, the structure's underside {under_in} in");
        }
        assert_eq!((got.runs, got.lowered, got.tight), (1, 2, 1), "{}", got.json());
        assert_eq!(got.json()["ducts_too_deep_for_plenum"], json!(1));
        let deep = &seated.mep.runs[2];
        assert!(!deep.reviewed && deep.height_source.starts_with("inferred (under the structure"), "{}", deep.height_source);
        assert!(no_document_height(&deep.height_source), "still a height no document gives");
        let again = seat_with(&mut seated, &rule(), &ceiling, &structure);
        assert_eq!(again.moved(), 0, "once: {}", again.json());
        assert_eq!(seated.mep.runs[2].height_source.matches("under the structure").count(), 1);
    }

    /// The same through the job's merge (`mep::seat_in_plenum`): a default-height duct is seated
    /// under the framing, not under a 6" slab up in the joists.
    #[test]
    fn the_plenum_seat_keeps_a_duct_under_the_floor_framing() {
        let mut sp: BuildingSpec = serde_json::from_value(json!({
            "units_per_foot": 12.0,
            "levels": [
                {"name": "Level 1", "elevation_ft": 0.0, "height_ft": 11.0, "ceiling_ft": 8.0},
                {"name": "Level 2", "elevation_ft": 11.0, "height_ft": 10.0, "ceiling_ft": 8.0}
            ],
            "floor_framing": [{"level": "Level 2", "member": "joist", "spacing_in": 16, "depth_in": 22}],
            "mep": {
                "systems": [
                    {"name": "RA", "layer": "M-RETURN", "kind": "duct", "size_in": [12, 12], "bottom_ft": 9.0},
                    {"name": "CW", "layer": "P-CW", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 9.0}
                ],
                "runs": [
                    {"system": "RA", "level": "Level 1", "points": [[0, 0], [240, 0]], "size_in": [8, 8], "bottoms_ft": [9.0], "height_source": "default"},
                    {"system": "RA", "level": "Level 1", "points": [[0, 50], [240, 50]], "size_in": [18, 18], "bottoms_ft": [9.0], "height_source": "default"},
                    {"system": "CW", "level": "Level 1", "points": [[0, 100], [240, 100]], "size_in": [1, 0], "bottoms_ft": [9.0], "height_source": "default"}
                ]
            }
        }))
        .unwrap();
        // 11'-0" less the 22" joists and the rule's deck.
        let under = 11.0 - (22.0 + super::super::ceilheight::Rule::load().deck_in) / 12.0;
        super::super::mep::seat_in_plenum(&mut sp);
        let b: Vec<f64> = sp.mep.runs.iter().map(|r| r.bottoms_ft[0]).collect();
        // The 8" duct, up in the joists, comes down under them; the 18" one can't fit over the
        // ceiling: under the joists all the same; the pipe keeps its height in the plenum.
        assert!((b[0] - (under - 8.0 / 12.0)).abs() < 1e-9, "{b:?} under {under}");
        assert!((b[1] - (under - 1.5)).abs() < 1e-9, "{b:?} under {under}");
        assert!((b[2] - 9.0).abs() < 1e-9, "{b:?}");
    }

    /// Q6554 through model.build: a storey under a floor framed with 22" joists (the structure's
    /// underside 108.5" over the floor) and an 8'-0" ceiling; an inferred 6" branch and an
    /// inferred 18x18 main laid at a main's height up in the joists. Built, no duct's top is
    /// over the structure's underside.
    #[test]
    fn model_build_keeps_ducts_with_no_height_under_the_floor_framing() {
        use cadaddy_doc::EntityKind;
        let mut s = crate::Session::empty();
        s.open_drawing(Drawing::new_imperial(), "Sample Duct Plenum", None);
        let spec = json!({
            "version": 1, "units_per_foot": 12,
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 11, "ceiling_ft": 8, "base": [0, 0]},
                {"name": "L2", "elevation_ft": 11, "height_ft": 10, "ceiling_ft": 8, "base": [0, 0]}
            ],
            "floor_framing": [{"level": "L2", "member": "joist", "spacing_in": 16, "depth_in": 22}],
            "rcp": {"ceilings": [{"level": "L1", "outline": [[0, 0], [480, 0], [480, 240], [0, 240]], "height_ft": 8.0}]},
            "mep": {
                "systems": [{"name": "RA", "layer": "M-RETURN", "kind": "duct", "size_in": [18, 18], "bottom_ft": 9.0}],
                "runs": [
                    {"system": "RA", "level": "L1", "points": [[60, 60], [60, 200]], "size_in": [6, 0], "bottoms_ft": [9.5], "height_source": "inferred", "made": "inferred", "basis": "M-BRANCH-DUCT"},
                    {"system": "RA", "level": "L1", "points": [[100, 120], [400, 120]], "size_in": [18, 18], "bottoms_ft": [9.0], "height_source": "inferred", "made": "inferred", "basis": "M-BRANCH-DUCT"}
                ]
            }
        });
        s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
        s.execute("model.build", &json!({})).unwrap();
        let tops: Vec<f64> = s
            .doc()
            .unwrap()
            .model
            .iter()
            .filter(|e| e.common.xdata.iter().any(|t| t.value.starts_with("part:segment")))
            .filter_map(|e| match &e.kind {
                EntityKind::Mesh(m) => m.vertices.iter().map(|p| p.z).reduce(f64::max),
                _ => None,
            })
            .collect();
        assert_eq!(tops.len(), 2, "{tops:?}");
        let under = (11.0 * 12.0) - 22.0 - super::super::ceilheight::Rule::load().deck_in;
        let over = tops.iter().filter(|t| **t > under + 0.1).count();
        assert_eq!(over, 0, "duct tops {tops:?} in, the structure's underside {under} in");
    }

    /// model.build seats them in the ceilings it builds (the reflected ceiling plans' areas here),
    /// the drawing's spec with them; built again unchanged, the model is kept.
    #[test]
    fn model_build_sets_grilles_in_the_ceilings_it_builds_and_raises_inferred_branches_over_them() {
        use cadaddy_doc::EntityKind;
        let mut s = crate::Session::empty();
        s.open_drawing(Drawing::new_imperial(), "Sample Ceiling Grilles", None);
        let spec = json!({
            "version": 1, "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 11, "ceiling_ft": 8.5, "base": [0, 0]}],
            "rcp": {"ceilings": [
                {"level": "L1", "outline": [[0, 0], [240, 0], [240, 240], [0, 240]], "height_ft": 7.8333},
                {"level": "L1", "outline": [[240, 0], [480, 0], [480, 240], [240, 240]], "height_ft": 8.5}
            ]},
            "mep": {
                "systems": [{"name": "EA", "layer": "M-EXHAUST", "kind": "duct", "size_in": [12, 0], "bottom_ft": 9.0}],
                "terminals": [
                    {"system": "EA", "level": "L1", "at": [60, 60], "size_in": [10, 10], "bottom_ft": 8.5, "terminal_kind": "exhaust_grille"},
                    {"system": "EA", "level": "L1", "at": [300, 60], "size_in": [10, 10], "bottom_ft": 8.5, "terminal_kind": "exhaust_grille"}
                ],
                "runs": [
                    {"system": "EA", "level": "L1", "points": [[300, 60], [300, 200]], "size_in": [12, 0], "bottoms_ft": [8.08], "height_source": "inferred", "made": "inferred", "basis": "M-BRANCH-DUCT"},
                    {"system": "EA", "level": "L1", "points": [[60, 60], [60, 200]], "size_in": [8, 0], "bottoms_ft": [8.5], "height_source": "inferred", "made": "inferred", "basis": "M-BRANCH-DUCT"}
                ]
            }
        });
        s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
        let r = s.execute("model.build", &json!({})).unwrap();
        // The built grilles' faces (their meshes' undersides) and the ducts' bottoms, inches.
        let low = |s: &crate::Session, part: &str| -> Vec<f64> {
            let mut v: Vec<f64> = s
                .doc()
                .unwrap()
                .model
                .iter()
                .filter(|e| e.common.xdata.iter().any(|t| t.value.starts_with(part)))
                .filter_map(|e| match &e.kind {
                    EntityKind::Mesh(m) => m.vertices.iter().map(|p| p.z).reduce(f64::min),
                    _ => None,
                })
                .map(|z| (z * 10.0).round() / 10.0)
                .collect();
            v.sort_by(f64::total_cmp);
            v
        };
        assert_eq!(low(&s, "part:terminal:"), vec![94.0, 102.0], "in the 7'-10\" and 8'-6\" ceilings");
        // The branch under the 8'-6" ceiling 3" over it; the one over the 7'-10" ceiling as it was.
        assert_eq!(low(&s, "part:segment"), vec![102.0, 105.0]);
        let (sp, _) = super::super::bspec::current(&s, "test").unwrap();
        let t: Vec<f64> = sp.mep.terminals.iter().map(|t| (t.bottom_ft * 1000.0).round() / 1000.0).collect();
        assert_eq!(t, vec![7.833, 8.5], "the drawing's spec has the heights built");
        assert_eq!(r["inCeiling"]["terminals_set_in_ceiling"], json!(1), "{}", r["inCeiling"]);
        assert_eq!(r["inCeiling"]["ducts_raised_over_ceiling"], json!(1), "{}", r["inCeiling"]);
        let again = s.execute("model.build", &json!({})).unwrap();
        assert_eq!(again["kept"], json!(true), "built again unchanged, kept: {}", again["rebuild"]);
    }

    /// The same with the ceilings as model.build makes them: meshes on the sheet (one of a
    /// stacked level, moved back), the smallest over a point its ceiling.
    #[test]
    fn the_ceilings_are_the_ones_built_over_the_plan_points() {
        use cadaddy_doc::{Common, EntityKind, Handle, Space};
        let mut d = Drawing::new_imperial();
        let ceiling = |d: &mut Drawing, level: &str, x0: f64, x1: f64, z: f64, moved: Option<Vec2>| {
            let mut m = super::super::wallgen::empty_mesh();
            let off = moved.unwrap_or(Vec2::ZERO);
            super::super::wallgen::add_box(&mut m, Vec2::new(x0, 120.0) + off, Vec2::new(x1, 120.0) + off, 240.0, z, z + 0.5);
            let x = super::super::wallgen::tag(cadaddy_walk3d::Part::Ceiling, level, Handle(0));
            let x = match moved {
                Some(o) => super::super::bspec::with_stacked(&x, o),
                None => x,
            };
            d.add(&Space::Model, Common { xdata: x.into(), ..Common::default() }, EntityKind::Mesh(m)).unwrap();
        };
        // Level 1: a whole-floor ceiling at 8'-6" with a room at 7'-10" in it; level 2 (from 11')
        // stacked 1000" east, its room at 8'.
        ceiling(&mut d, "Level 1", 0.0, 480.0, 102.0, None);
        ceiling(&mut d, "Level 1", 0.0, 240.0, 94.0, None);
        ceiling(&mut d, "Level 2", 0.0, 480.0, 132.0 + 96.0, Some(Vec2::new(1000.0, 0.0)));
        let mut sp: BuildingSpec = serde_json::from_value(json!({
            "units_per_foot": 12.0,
            "levels": [
                {"name": "Level 1", "elevation_ft": 0.0, "height_ft": 11.0, "ceiling_ft": 8.5},
                {"name": "Level 2", "elevation_ft": 11.0, "height_ft": 10.0, "ceiling_ft": 8.5},
                {"name": "Level 3", "elevation_ft": 21.0, "height_ft": 10.0, "ceiling_ft": 8.5}
            ],
            "mep": {
                "systems": [{"name": "RA", "layer": "M-RETURN", "kind": "duct", "size_in": [12, 10], "bottom_ft": 9.0}],
                "terminals": [
                    {"system": "RA", "level": "Level 1", "at": [60, 60], "size_in": [12, 12], "bottom_ft": 8.5, "terminal_kind": "return_grille"},
                    {"system": "RA", "level": "Level 1", "at": [300, 60], "size_in": [12, 12], "bottom_ft": 8.5, "terminal_kind": "return_grille"},
                    {"system": "RA", "level": "Level 2", "at": [60, 60], "size_in": [12, 12], "bottom_ft": 8.5, "terminal_kind": "return_grille"},
                    {"system": "RA", "level": "Level 1", "at": [600, 60], "size_in": [12, 12], "bottom_ft": 8.0, "terminal_kind": "return_grille"},
                    {"system": "RA", "level": "Level 1", "at": [800, 60], "size_in": [12, 12], "bottom_ft": 9.5, "terminal_kind": "return_grille"},
                    {"system": "RA", "level": "Level 3", "at": [60, 60], "size_in": [12, 12], "bottom_ft": 9.5, "terminal_kind": "return_grille"}
                ],
                "runs": [
                    {"system": "RA", "level": "Level 1", "points": [[600, 100], [700, 100]], "size_in": [12, 10], "bottoms_ft": [7.0], "height_source": "default"},
                    {"system": "RA", "level": "Level 1", "points": [[800, 100], [900, 100]], "size_in": [12, 10], "bottoms_ft": [7.0], "height_source": "default"},
                    {"system": "RA", "level": "Level 3", "points": [[0, 100], [200, 100]], "size_in": [12, 10], "bottoms_ft": [6.0], "height_source": "default"}
                ]
            },
            // x 750..1000 on level 1 open to the structure.
            "ceiling_types": [{"name": "EXPOSED", "kind": "open"}],
            "rcp": {"ceilings": [{"level": "Level 1", "outline": [[750, 0], [1000, 0], [1000, 240], [750, 240]], "height_ft": 10.0, "ceiling_type": "EXPOSED"}]}
        }))
        .unwrap();
        let got = seat(&d, &mut sp);
        let t: Vec<f64> = sp.mep.terminals.iter().map(|t| (t.bottom_ft * 1000.0).round() / 1000.0).collect();
        // In the room's ceiling, the floor's, level 2's room (stacked); where no ceiling is built,
        // the level's; in the exposed area as it is; on level 3, where the build laid out no
        // ceiling at all, as they are.
        assert_eq!(t, vec![7.833, 8.5, 8.0, 8.5, 9.5, 9.5]);
        let b: Vec<f64> = sp.mep.runs.iter().map(|r| r.bottoms_ft[0]).collect();
        assert_eq!(b, vec![8.75, 7.0, 6.0]);
        assert_eq!((got.terminals, got.terminals_checked, got.runs), (3, 4, 1));
    }
}
