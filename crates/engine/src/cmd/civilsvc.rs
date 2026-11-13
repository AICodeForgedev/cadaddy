//! The services a civil utility plan brings to the building, built as runs (Q6597).
//!
//! A civil utility plan draws the water service, the fire line, the building sewer and the gas
//! service from the street mains to the building, each labelled with its size and service as
//! printed (`40 L.F. 3" DOM. W.L.`, `10" S.S. (PRIVATE)`). The building's own sheets carry them only
//! to the wall, so without this the model has no pipe outside the footprint.
//!
//! - [`labels`]: the service labels of a sheet (a size and a service; existing and public lines,
//!   fittings and counts left out).
//! - [`is_utility_plan`]: a civil sheet carrying service labels.
//! - [`services`]: each label's line (the one it lies along or its leader points at), followed
//!   through the vaults drawn between its pieces to the building outline the plan draws.
//! - [`runs`]: each service laid onto the slab level by the sheet's fit ([`super::sitework::register`]),
//!   carried to the wall, joined to the building's own entry of its system, and split at the
//!   point of connection: the building's trade inside it (`P-UG-<system>`), the site/civil
//!   contractor outside it (`C-UG-<system>`). Each run keeps its size, material and length as
//!   printed, its depth (printed inverts against the printed finished floor, else the kind's
//!   cover by rule) and a gravity line's fall, and its provenance and basis.
//!
//! Every word and number the reader uses is in `civilsvc.json` (rules data, each with its source).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::{BuildingSpec, Made, MepKind, MepRun, MepSystem};
use serde::Deserialize;

use super::sitework::{self, Fit, SiteRead};

const RULES: &str = include_str!("civilsvc.json");

/// Texts of one sheet looked at, at most (hostile input).
const MAX_TEXTS: usize = 200_000;
/// Pieces of one service followed, at most.
const MAX_PIECES: usize = 2_000;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Kind {
    pub kind: String,
    pub system: String,
    pub trade: String,
    pub words: Vec<String>,
    pub not_words: Vec<String>,
    pub entry_systems: Vec<String>,
    pub cover_in: f64,
    pub cover_source: String,
    pub gravity: bool,
}

/// The rules data (`civilsvc.json`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub kinds: Vec<Kind>,
    pub sheet_title_words: Vec<String>,
    pub sheet_not_title_words: Vec<String>,
    pub sheet_source: String,
    pub building_label_words: Vec<String>,
    pub fit_step_ft: Vec<f64>,
    pub fit_tolerance_ft: f64,
    pub fit_sample_ft: f64,
    pub fit_min_share: f64,
    pub fit_margin: f64,
    pub fit_parallel_cos: f64,
    pub fit_turn_deg: Vec<f64>,
    pub station_words: Vec<String>,
    pub fit_source: String,
    pub approach_share: f64,
    pub domestic_words: Vec<String>,
    pub existing_words: Vec<String>,
    pub public_words: Vec<String>,
    pub fitting_words: Vec<String>,
    pub pointer_words: Vec<String>,
    pub appurtenance_words: Vec<String>,
    pub material_words: Vec<String>,
    pub min_labels: usize,
    pub label_reach_h: f64,
    pub leader_reach_h: f64,
    pub leader_end_h: f64,
    pub leader_max_h: f64,
    pub dash_gap_h: f64,
    pub turn_deg: f64,
    pub straight_deg: f64,
    pub entry_reach_ft: f64,
    pub chain_gap_ft: f64,
    pub chain_steps: usize,
    pub min_line_ft: f64,
    pub length_weight: f64,
    pub length_slack_share: f64,
    pub length_slack_ft: f64,
    pub line_source: String,
    pub poc_words: Vec<String>,
    pub poc_reach_h: f64,
    pub poc_ft: f64,
    pub poc_source: String,
    pub join_reach_ft: f64,
    pub join_source: String,
    pub undesignated_source: String,
    pub slope: Vec<[f64; 2]>,
    pub slope_source: String,
    pub grade_below_floor_in: f64,
    pub grade_source: String,
    pub ff_words: Vec<String>,
    pub invert_words: Vec<String>,
    pub invert_reach_h: f64,
    pub building_prefix: String,
    pub civil_prefix: String,
    pub scope_source: String,
    pub max_sheets: usize,
    pub max_labels: usize,
}

pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// Whether a run's system is one this module makes (`P-UG-CW`, `C-UG-SAN`).
pub(crate) fn is_service_system(system: &str) -> bool {
    let r = rules();
    let up = system.to_ascii_uppercase();
    r.kinds.iter().any(|k| {
        [&r.building_prefix, &r.civil_prefix]
            .iter()
            .any(|p| !p.is_empty() && up == format!("{}-{}", p.to_ascii_uppercase(), k.system.to_ascii_uppercase()))
    })
}

/// Whether a sheet is a civil sheet whose title names utility work (`UTILITY LAYOUT`): loaded
/// with the plans so its services are read, though its title names no plan.
pub(crate) fn utility_sheet(discipline: &str, title: &str) -> bool {
    let r = rules();
    let n = norm(title);
    cadaddy_mepfix::sheet_facts::is_site_sheet(discipline, title) && has_word(&n, &r.sheet_title_words) && !has_word(&n, &r.sheet_not_title_words)
}

// ---------------- laying the plan on the building by its linework ----------------

/// The plan's lines in cells (sheet units), for "is a line near here".
struct Cells {
    size: f64,
    map: HashMap<(i64, i64), Vec<usize>>,
    segs: Vec<([f64; 2], [f64; 2])>,
}

impl Cells {
    fn new(segs: Vec<([f64; 2], [f64; 2])>, size: f64) -> Cells {
        let mut map: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        let key = |p: [f64; 2]| ((p[0] / size).floor() as i64, (p[1] / size).floor() as i64);
        for (i, (a, b)) in segs.iter().enumerate() {
            let l = sitework::dist(*a, *b);
            let n = ((l / size).ceil() as usize).clamp(1, 10_000);
            let mut last = None;
            for k in 0..=n {
                let t = k as f64 / n as f64;
                let c = key([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
                if last != Some(c) {
                    map.entry(c).or_default().push(i);
                    last = Some(c);
                }
            }
        }
        Cells { size, map, segs }
    }
    /// A line `p` lies on the middle of (within `tol`, away from its ends by more): a tee or a
    /// crossing with a through line.
    fn through(&self, p: [f64; 2], tol: f64, skip: &BTreeSet<usize>) -> bool {
        let (kx, ky) = ((p[0] / self.size).floor() as i64, (p[1] / self.size).floor() as i64);
        (-1..=1).any(|dx| {
            (-1..=1).any(|dy| {
                self.map.get(&(kx + dx, ky + dy)).is_some_and(|list| {
                    list.iter().any(|i| {
                        !skip.contains(i)
                            && self.segs.get(*i).is_some_and(|(a, b)| {
                                sitework::seg_dist(p, *a, *b) <= tol && sitework::dist(p, *a) > 2.0 * tol && sitework::dist(p, *b) > 2.0 * tol
                            })
                    })
                })
            })
        })
    }
    /// A line within `tol` of `p` running along `dir` (within the cosine `cos_min`); any line
    /// when `dir` is none.
    fn near(&self, p: [f64; 2], tol: f64, dir: Option<[f64; 2]>, cos_min: f64) -> bool {
        let (kx, ky) = ((p[0] / self.size).floor() as i64, (p[1] / self.size).floor() as i64);
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(list) = self.map.get(&(kx + dx, ky + dy))
                    && list.iter().any(|i| {
                        self.segs.get(*i).is_some_and(|(a, b)| {
                            sitework::seg_dist(p, *a, *b) <= tol
                                && dir.is_none_or(|d| {
                                    let l = sitework::dist(*a, *b);
                                    l > 1e-9 && ((b[0] - a[0]) * d[0] + (b[1] - a[1]) * d[1]).abs() / l >= cos_min
                                })
                        })
                    })
                {
                    return true;
                }
            }
        }
        false
    }
}

/// Points along the building's outline on the model, each with its edge's direction, every
/// `step`: the boundary of its shells' boxes taken together (wall boxes or the floor's strips
/// alike), else the footprint's sides.
fn wall_samples(sp: &BuildingSpec, slab: &str, fp: [f64; 4], step: f64, eps: f64) -> Vec<([f64; 2], [f64; 2])> {
    let boxes: Vec<[f64; 4]> = sp
        .shells
        .iter()
        .find(|(l, _)| l.eq_ignore_ascii_case(slab))
        .map(|(_, b)| {
            b.iter()
                .filter(|b| b.iter().all(|v| v.is_finite()))
                .take(5_000)
                .map(|b| [b[0].min(b[2]), b[1].min(b[3]), b[0].max(b[2]), b[1].max(b[3])])
                .collect()
        })
        .unwrap_or_default();
    let boxes = if boxes.is_empty() { vec![fp] } else { boxes };
    let inside_any = |p: [f64; 2]| boxes.iter().any(|b| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3]);
    let mut out = Vec::new();
    for b in &boxes {
        // Each side, its direction and its outward normal.
        let sides = [
            ([b[0], b[1]], [b[2], b[1]], [0.0, -1.0]),
            ([b[2], b[1]], [b[2], b[3]], [1.0, 0.0]),
            ([b[2], b[3]], [b[0], b[3]], [0.0, 1.0]),
            ([b[0], b[3]], [b[0], b[1]], [-1.0, 0.0]),
        ];
        for (a, z, nrm) in sides {
            let l = sitework::dist(a, z);
            if l <= 1e-9 {
                continue;
            }
            let dir = [(z[0] - a[0]) / l, (z[1] - a[1]) / l];
            let n = ((l / step).floor() as usize).clamp(1, 2_000);
            for k in 0..=n {
                let t = k as f64 / n as f64;
                let p = [a[0] + t * (z[0] - a[0]), a[1] + t * (z[1] - a[1])];
                if !inside_any([p[0] + nrm[0] * eps, p[1] + nrm[1] * eps]) {
                    out.push((p, dir));
                }
                if out.len() >= 20_000 {
                    return out;
                }
            }
        }
    }
    out
}

/// The main direction of lines (radians, in [-π/4, π/4)), length-weighted four-fold.
fn main_angle<'a>(segs: impl Iterator<Item = &'a ([f64; 2], [f64; 2])>) -> f64 {
    let (mut c, mut s) = (0.0, 0.0);
    for (a, b) in segs {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = dx.hypot(dy);
        if l > 0.0 {
            let t = dy.atan2(dx) * 4.0;
            c += l * t.cos();
            s += l * t.sin();
        }
    }
    s.atan2(c) / 4.0
}

/// A civil plan at its printed scale laid onto the slab level by its linework: the building's
/// outside walls matched to the plan's lines around its `BUILDING` label (Q6597). None: the
/// plan isn't at its printed scale, labels no building, or no place puts the walls on its lines.
pub(crate) fn register_by_linework(read: &SiteRead, texts: &[TextItem], sp: &BuildingSpec, slab: &str, sheet_real: bool) -> Option<Fit> {
    let r = rules();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    if !sheet_real {
        return None;
    }
    let fp = super::ugplan::slab_box(sp, slab)?;
    let (w, h) = (fp[2] - fp[0], fp[3] - fp[1]);
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    let fc = [(fp[0] + fp[2]) / 2.0, (fp[1] + fp[3]) / 2.0];
    let tol = r.fit_tolerance_ft.max(0.25) * upf;
    let perimeter = 2.0 * ((fp[2] - fp[0]) + (fp[3] - fp[1]));
    let samples = wall_samples(sp, slab, fp, (r.fit_sample_ft.max(0.5) * upf).max(perimeter / 400.0), 0.1 * upf);
    if samples.len() < 8 {
        return None;
    }
    let every = samples.len().div_ceil(400).max(1);
    let samples: Vec<([f64; 2], [f64; 2])> = samples.into_iter().step_by(every).collect();
    let model_angle = {
        let edges: Vec<([f64; 2], [f64; 2])> = samples.iter().map(|(p, d)| (*p, [p[0] + d[0], p[1] + d[1]])).collect();
        main_angle(edges.iter())
    };
    let segs: Vec<([f64; 2], [f64; 2])> = read.segs.iter().chain(&read.edges).take(400_000).map(|g| (g.a, g.b)).collect();
    if segs.is_empty() {
        return None;
    }
    let cells = Cells::new(segs, (2.0 * tol).max(1e-6));
    // The building labels: where to look.
    let reach = w.max(h);
    let mut anchors: Vec<([f64; 2], String)> = Vec::new();
    for t in texts.iter().take(MAX_TEXTS) {
        let n = norm(&t.value);
        if !has_word(&n, &r.building_label_words) || has_word(&n, &r.poc_words) {
            continue;
        }
        let at = sitework::centre(&sitework::text_box(t));
        if anchors.iter().any(|(p, _)| sitework::dist(*p, at) < reach / 4.0) {
            continue;
        }
        anchors.push((at, t.value.trim().to_string()));
        if anchors.len() >= 8 {
            break;
        }
    }
    let mut best: Option<(f64, f64, [f64; 2], String)> = None; // share, phi, sheet centre, label
    let mut places: Vec<(f64, f64, [f64; 2])> = Vec::new();
    for (anchor, label) in &anchors {
        let near: Vec<([f64; 2], [f64; 2])> = cells
            .segs
            .iter()
            .filter(|(a, b)| sitework::dist(*a, *anchor) <= reach || sitework::dist(*b, *anchor) <= reach)
            .take(100_000)
            .copied()
            .collect();
        let sheet_angle = main_angle(near.iter());
        for k in 0..4 {
            let base = model_angle - sheet_angle + f64::from(k) * std::f64::consts::FRAC_PI_2;
            // The walls on lines that run along them, the building's centre at `at` on the sheet,
            // turned `phi`.
            let share = |at: [f64; 2], phi: f64| {
                let (c, s) = (phi.cos(), phi.sin());
                let hits = samples
                    .iter()
                    .filter(|(m, d)| {
                        let (dx, dy) = (m[0] - fc[0], m[1] - fc[1]);
                        let q = [at[0] + c * dx + s * dy, at[1] - s * dx + c * dy];
                        cells.near(q, tol, Some([c * d[0] + s * d[1], -s * d[0] + c * d[1]]), r.fit_parallel_cos)
                    })
                    .count();
                hits as f64 / samples.len() as f64
            };
            let mut here = (share(*anchor, base), *anchor, base);
            let mut span = reach / 2.0;
            for (round, step) in r.fit_step_ft.iter().map(|f| f.max(0.05) * upf).enumerate() {
                let n = ((span / step).ceil() as i64).clamp(1, 200);
                let (centre, phi0) = (here.1, here.2);
                // The plan's lines' direction is only near the walls': after the first round
                // the turn is refined too.
                let turns: Vec<f64> =
                    if round == 0 { vec![phi0] } else { r.fit_turn_deg.iter().map(|d| phi0 + d.to_radians() / round as f64).collect() };
                for phi in turns {
                    for i in -n..=n {
                        for j in -n..=n {
                            let at = [centre[0] + i as f64 * step, centre[1] + j as f64 * step];
                            let v = share(at, phi);
                            if v > here.0 + 1e-12 {
                                here = (v, at, phi);
                            }
                        }
                    }
                }
                span = 2.0 * step;
            }
            places.push((here.0, here.2, here.1));
            if best.as_ref().is_none_or(|b| here.0 > b.0 + 1e-9) {
                best = Some((here.0, here.2, here.1, label.clone()));
            }
        }
    }
    let (share, phi, at, label) = best?;
    // Another place nearly as good: the plan's lines don't say where the building is.
    // (A turn of the same place as good is a symmetric footprint: the least turn is taken.)
    let other = places.iter().filter(|(_, _, a)| sitework::dist(*a, at) > reach / 4.0).map(|x| x.0).fold(f64::NEG_INFINITY, f64::max);
    if share < r.fit_min_share || share - other < r.fit_margin {
        return None;
    }
    // Sheet → model: p ↦ R(φ)(p − at) + fc.
    let (a, b) = (phi.cos(), phi.sin());
    let reference = sp
        .sheets
        .iter()
        .find(|l| {
            l.level.as_deref().is_some_and(|v| v.eq_ignore_ascii_case(slab)) && l.discipline.as_deref().is_some_and(|d| d.eq_ignore_ascii_case("A"))
        })
        .map_or_else(|| slab.to_string(), |l| l.number.clone());
    let reg = cadaddy_walk3d::levels::SheetRegistration {
        reference,
        method: "building linework".into(),
        a,
        b,
        tx: fc[0] - (a * at[0] - b * at[1]),
        ty: fc[1] - (b * at[0] + a * at[1]),
        matched: samples.len(),
        residual: None,
    };
    let outline: Vec<[f64; 2]> = vec![[fp[0], fp[1]], [fp[2], fp[1]], [fp[2], fp[3]], [fp[0], fp[3]]];
    let sheet_outline: Vec<[f64; 2]> = outline.iter().filter_map(|p| reg.to_sheet(*p)).collect();
    if sheet_outline.len() != 4 {
        return None;
    }
    let why = format!(
        "laid on {slab} by its linework: {:.0}% of the building's outside walls on the plan's lines at its printed scale, by \"{label}\", turned {:.1} deg ({})",
        share * 100.0,
        phi.to_degrees(),
        r.fit_source
    );
    Some(Fit { reg, method: "building linework", why, outline, sheet_outline })
}

// ---------------- the labels ----------------

/// A label's words: upper case, a dot dropped unless it is a decimal point, every other mark
/// but the inch and foot marks a space, padded with a space each side (`W.L.` → ` WL `).
fn norm(v: &str) -> String {
    let chars: Vec<char> = v.chars().take(400).collect();
    let mut out = String::from(" ");
    for (i, c) in chars.iter().enumerate() {
        let c = c.to_ascii_uppercase();
        if c == '.' {
            let digit = |j: Option<usize>| j.and_then(|j| chars.get(j)).is_some_and(|x| x.is_ascii_digit());
            if digit(i.checked_sub(1)) && digit(Some(i + 1)) {
                out.push('.');
            }
            continue;
        }
        if c.is_ascii_alphanumeric() || c == '"' || c == '\'' || c == '/' {
            out.push(c);
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

fn has_word(n: &str, words: &[String]) -> bool {
    words.iter().any(|w| !w.trim().is_empty() && n.contains(&format!(" {} ", w.trim())))
}

/// A label with its quoted names left out (`S.S. LINE "L-1"`): a quote opening on a letter
/// is no inch mark.
fn unquoted(v: &str) -> String {
    let chars: Vec<char> = v.chars().take(400).collect();
    let mut out = String::new();
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        let opens = c == '"'
            && chars.get(i + 1).is_some_and(|x| x.is_ascii_alphabetic())
            && i.checked_sub(1).and_then(|j| chars.get(j)).is_none_or(|p| !p.is_ascii_digit());
        if opens && let Some(close) = chars.iter().skip(i + 1).position(|x| *x == '"') {
            i += close + 2;
            out.push(' ');
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// A number as printed: `3`, `1.5`, `3/4`, `1-1/2` is read by its parts elsewhere.
fn number(t: &str) -> Option<f64> {
    if let Some((a, b)) = t.split_once('/') {
        let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
        return (b > 0.0).then_some(a / b);
    }
    t.parse::<f64>().ok()
}

/// The pipe size a label prints, inches: the first number marked as inches (`8"`, `8''`,
/// `8 IN`, `8 INCH`).
pub(crate) fn size_in(n: &str) -> Option<f64> {
    let toks: Vec<&str> = n.split_whitespace().collect();
    for (i, t) in toks.iter().enumerate() {
        let next = toks.get(i + 1).copied().unwrap_or("");
        let v = if let Some(x) = t.strip_suffix("''").or_else(|| t.strip_suffix('"')) {
            number(x)
        } else if matches!(next, "IN" | "INCH" | "INCHES") {
            number(t)
        } else {
            None
        };
        if let Some(v) = v.filter(|v| v.is_finite() && *v > 0.0 && *v <= 120.0) {
            return Some(v);
        }
    }
    None
}

/// The length a label prints, feet (`40 L.F.`, `40 LF`, `40 LIN FT`).
fn length_ft(n: &str) -> Option<f64> {
    let toks: Vec<&str> = n.split_whitespace().collect();
    for (i, t) in toks.iter().enumerate() {
        let next = toks.get(i + 1).copied().unwrap_or("");
        let after = toks.get(i + 2).copied().unwrap_or("");
        if (next == "LF" || (next == "LIN" && matches!(after, "FT" | "FEET")) || next == "LINEAR")
            && let Some(v) = number(t).filter(|v| v.is_finite() && *v > 0.0 && *v < 100_000.0)
        {
            return Some(v);
        }
    }
    None
}

/// A service label read: what it names and where it is (sheet units).
#[derive(Clone, Debug)]
pub(crate) struct Label {
    /// Index into the rules' kinds.
    pub kind: usize,
    /// It names its service outright (domestic, fire, sewer, gas); a water line named neither
    /// domestic nor fire is not.
    pub designated: bool,
    pub size_in: f64,
    pub material: String,
    pub length_ft: Option<f64>,
    pub text: String,
    pub bbox: [f64; 4],
    /// Where its lettering starts and ends (mid-height, each line's), where a leader leaves it.
    pub tips: Vec<[f64; 2]>,
}

/// The kind a label's words name, and whether outright.
fn kind_of(n: &str) -> Option<(usize, bool)> {
    let r = rules();
    for (i, k) in r.kinds.iter().enumerate() {
        if has_word(n, &k.words) && !has_word(n, &k.not_words) {
            let designated = k.kind != "water" || has_word(n, &r.domestic_words);
            return Some((i, designated));
        }
    }
    None
}

/// The service labels among `texts`: a size and a service printed together, not an existing or
/// public line's, not a fitting's (`1 - 8" GATE VALVE`). A label's second line (`(PRIVATE)`)
/// under it is read with it.
pub(crate) fn labels(texts: &[&TextItem]) -> Vec<Label> {
    let r = rules();
    let mut out = Vec::new();
    for t in texts.iter().take(MAX_TEXTS) {
        if out.len() >= r.max_labels.max(1) {
            break;
        }
        let raw = t.value.trim();
        if raw.is_empty() || raw.chars().count() > 120 {
            continue;
        }
        // The line under it, as tall and close: `(PRIVATE)`, `(PUBLIC)`.
        let h = t.height.max(0.0);
        let under = texts
            .iter()
            .filter(|o| {
                let dy = t.at.y - o.at.y;
                dy > 0.3 * h && dy < 2.0 * h && (o.at.x - t.at.x).abs() < 20.0 * h && (o.height - h).abs() <= 0.3 * h.max(1e-9)
            })
            .min_by(|a, b| (t.at.y - a.at.y).total_cmp(&(t.at.y - b.at.y)))
            .filter(|o| o.value.trim().starts_with('(') && o.value.trim().chars().count() <= 20);
        let tips_of = |o: &TextItem| {
            let n = o.value.trim().chars().count().max(1) as f64;
            let (len, hh) = (n * 0.8 * o.height.max(0.0), o.height.max(0.0) / 2.0);
            let (c, sn) = (o.rotation.cos(), o.rotation.sin());
            let p = [o.at.x - sn * hh, o.at.y + c * hh];
            [p, [p[0] + c * len, p[1] + sn * len]]
        };
        let mut tips: Vec<[f64; 2]> = tips_of(t).to_vec();
        if let Some(o) = under {
            tips.extend(tips_of(o));
        }
        // A callout stack naming an appurtenance (`1 - FIRE HYDRANT ASSEMBLY` over `12 L.F. 6" W.L.`)
        // labels that appurtenance's lead, not a service.
        let stacked = texts.iter().any(|o| {
            let dy = (o.at.y - t.at.y).abs();
            dy > 0.3 * h && dy < 3.0 * h && (o.at.x - t.at.x).abs() < 10.0 * h && has_word(&norm(&o.value), &r.appurtenance_words)
        });
        if stacked {
            continue;
        }
        let under_box = under.map(|o| sitework::text_box(o));
        let under = under.map(|o| o.value.trim().to_string()).unwrap_or_default();
        let n = norm(&unquoted(&format!("{raw} {under}")));
        // A profile's station (`STA 1+12.22`) names a point, not a line.
        if n.split_whitespace().any(|w| {
            r.station_words
                .iter()
                .any(|x| w.strip_prefix(x.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_digit())))
        }) {
            continue;
        }
        let Some((kind, designated)) = kind_of(&n) else { continue };
        // `TO 8" S.S. LINE`: a pointer to another line.
        if n.split_whitespace().next().is_some_and(|w| r.pointer_words.iter().any(|x| x == w)) {
            continue;
        }
        if has_word(&n, &r.existing_words) || has_word(&n, &r.public_words) || has_word(&n, &r.fitting_words) {
            continue;
        }
        let Some(size) = size_in(&n) else { continue };
        let material = r.material_words.iter().find(|w| has_word(&n, std::slice::from_ref(*w))).cloned().unwrap_or_default();
        out.push(Label {
            kind,
            designated,
            size_in: size,
            material,
            length_ft: length_ft(&n),
            text: if under.is_empty() { raw.to_string() } else { format!("{raw} {under}") },
            bbox: under_box.map_or(sitework::text_box(t), |b| sitework::union(sitework::text_box(t), b)),
            tips,
        });
    }
    out
}

/// Whether a civil sheet is a utility plan: it labels services to a building.
pub(crate) fn is_utility_plan(texts: &[TextItem]) -> usize {
    let refs: Vec<&TextItem> = texts.iter().take(MAX_TEXTS).collect();
    let n = labels(&refs).len();
    if n >= rules().min_labels.max(1) { n } else { 0 }
}

// ---------------- the lines ----------------

/// A service read off the plan: its label, its line from the site end to the building end
/// (sheet units), which pieces it is.
#[derive(Clone, Debug)]
pub(crate) struct Service {
    pub label: Label,
    /// The labels of the other pieces it was followed through.
    pub more: Vec<String>,
    pub pts: Vec<[f64; 2]>,
    pub comps: BTreeSet<usize>,
    /// Resolved kind (a water line named neither domestic nor fire is resolved later).
    pub kind: usize,
    pub inferred_kind: bool,
}

/// The distance from `p` to a closed outline's edge (0 inside it).
fn to_outline(poly: &[[f64; 2]], p: [f64; 2]) -> f64 {
    if poly.len() < 3 {
        return f64::INFINITY;
    }
    if sitework::inside(poly, p) {
        return 0.0;
    }
    sitework::nearest_on(poly, p).map_or(f64::INFINITY, |q| sitework::dist(p, q))
}

/// The gap between a segment and a box (0 when it crosses it).
fn seg_box(a: [f64; 2], b: [f64; 2], bb: &[f64; 4]) -> f64 {
    let (ga, gb) = (sitework::gap(bb, a), sitework::gap(bb, b));
    if ga <= 0.0 || gb <= 0.0 {
        return 0.0;
    }
    let corners = [[bb[0], bb[1]], [bb[2], bb[1]], [bb[2], bb[3]], [bb[0], bb[3]]];
    let mid = [(bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0];
    let through = sitework::seg_dist(mid, a, b) <= 0.5 * (bb[2] - bb[0]).abs().min((bb[3] - bb[1]).abs());
    if through {
        return 0.0;
    }
    corners.iter().map(|c| sitework::seg_dist(*c, a, b)).fold(ga.min(gb), f64::min)
}

/// The plan's line pieces by their ends, for following a line.
struct Ends {
    cell: f64,
    map: HashMap<(i64, i64), Vec<usize>>,
}

impl Ends {
    fn new(segs: &[sitework::Seg], cell: f64) -> Ends {
        let mut map: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        let cell = cell.max(1e-6);
        for (i, s) in segs.iter().enumerate() {
            for (e, p) in [(0usize, s.a), (1usize, s.b)] {
                map.entry(((p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64)).or_default().push(2 * i + e);
            }
        }
        Ends { cell, map }
    }
    /// The piece ends (`2i` start, `2i + 1` end) within `r` of `p` (r no more than the cell).
    fn around(&self, p: [f64; 2]) -> impl Iterator<Item = usize> + '_ {
        let (kx, ky) = ((p[0] / self.cell).floor() as i64, (p[1] / self.cell).floor() as i64);
        (-1..=1).flat_map(move |dx| (-1..=1).flat_map(move |dy| self.map.get(&(kx + dx, ky + dy)).into_iter().flatten().copied()))
    }
}

fn unit(a: [f64; 2], b: [f64; 2]) -> Option<[f64; 2]> {
    let l = sitework::dist(a, b);
    (l > 1e-12).then(|| [(b[0] - a[0]) / l, (b[1] - a[1]) / l])
}

/// One drawn line followed from the piece `start` both ways: at a corner only one other piece
/// meets, on round it; where several meet, on along the straightest within `turn_cos`; across a
/// gap up to `gap` only where it runs on straight (`straight_cos`: a dashed line). Its points and
/// pieces.
/// How a line is followed: its pieces by their ends and in cells, and the reaches.
struct Follow<'a> {
    segs: &'a [sitework::Seg],
    ends: Ends,
    cells: Cells,
    touch: f64,
    gap: f64,
    turn_cos: f64,
    straight_cos: f64,
}

fn trace(f: &Follow, start: usize, skip: &BTreeSet<usize>) -> (Vec<[f64; 2]>, BTreeSet<usize>) {
    let (segs, ends, touch, gap, turn_cos, straight_cos) = (f.segs, &f.ends, f.touch, f.gap, f.turn_cos, f.straight_cos);
    let mut used: BTreeSet<usize> = [start].into_iter().collect();
    let Some(s0) = segs.get(start) else { return (Vec::new(), used) };
    let mut sides: Vec<Vec<[f64; 2]>> = Vec::new();
    for (from, to) in [(s0.a, s0.b), (s0.b, s0.a)] {
        let mut pts: Vec<[f64; 2]> = Vec::new();
        let (mut end, mut dir) = (to, unit(from, to).unwrap_or([1.0, 0.0]));
        for _ in 0..MAX_PIECES {
            // Ending on the middle of another line (a tee on a main): it stops there.
            if f.cells.through(end, touch, &used) {
                break;
            }
            let mut touching: Vec<(usize, [f64; 2], [f64; 2])> = Vec::new();
            let mut across: Vec<(usize, [f64; 2], [f64; 2], f64)> = Vec::new();
            for node in ends.around(end) {
                let (j, e) = (node / 2, node % 2);
                if used.contains(&j) || skip.contains(&j) {
                    continue;
                }
                let Some(sj) = segs.get(j) else { continue };
                let (q, o) = if e == 0 { (sj.a, sj.b) } else { (sj.b, sj.a) };
                let Some(d2) = unit(q, o) else { continue };
                let d = sitework::dist(q, end);
                if d <= touch {
                    touching.push((j, o, d2));
                } else if d <= gap {
                    let dot = dir[0] * d2[0] + dir[1] * d2[1];
                    let toward = unit(end, q).is_some_and(|g| dir[0] * g[0] + dir[1] * g[1] >= straight_cos);
                    if dot >= straight_cos && toward {
                        across.push((j, o, d2, d));
                    }
                }
            }
            let next = if touching.len() == 1 {
                touching.first().map(|(j, o, d2)| (*j, *o, *d2))
            } else if !touching.is_empty() {
                touching
                    .iter()
                    .map(|(j, o, d2)| (*j, *o, *d2, dir[0] * d2[0] + dir[1] * d2[1]))
                    .filter(|x| x.3 >= turn_cos)
                    .max_by(|a, b| a.3.total_cmp(&b.3))
                    .map(|(j, o, d2, _)| (j, o, d2))
            } else {
                across.iter().min_by(|a, b| a.3.total_cmp(&b.3)).map(|(j, o, d2, _)| (*j, *o, *d2))
            };
            let Some((j, o, d2)) = next else { break };
            used.insert(j);
            pts.push(o);
            end = o;
            dir = d2;
        }
        sides.push(pts);
    }
    let mut out: Vec<[f64; 2]> = sides.get(1).map(|b| b.iter().rev().copied().collect()).unwrap_or_default();
    out.push(s0.a);
    out.push(s0.b);
    out.extend(sides.first().map(Vec::as_slice).unwrap_or_default());
    out.dedup_by(|a, b| sitework::dist(*a, *b) < 1e-9);
    (out, used)
}

/// The services into the building `outline` (sheet units) one sheet labels. `upf`: sheet units
/// per foot.
pub(crate) fn services(read: &SiteRead, texts: &[TextItem], outline: &[[f64; 2]], upf: f64) -> (Vec<Service>, Vec<String>) {
    let r = rules();
    let h = read.h;
    let mut notes = Vec::new();
    if !(h.is_finite() && h > 0.0 && upf.is_finite() && upf > 0.0) || outline.len() < 3 {
        return (Vec::new(), notes);
    }
    let refs: Vec<&TextItem> = texts.iter().take(MAX_TEXTS).collect();
    let found = labels(&refs);
    if found.is_empty() {
        return (Vec::new(), notes);
    }
    // The plan's line pieces, the building's own layers left out.
    let building_layers: BTreeSet<usize> = read
        .layers
        .iter()
        .enumerate()
        .filter(|(_, l)| {
            let up = l.to_ascii_uppercase();
            up.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| sitework::rules().building_layer_words.iter().any(|x| x == w))
        })
        .map(|(i, _)| i)
        .collect();
    let segs: Vec<sitework::Seg> =
        read.segs.iter().filter(|s| !building_layers.contains(&s.layer) && sitework::dist(s.a, s.b) > 1e-9).copied().collect();
    let touch = sitework::rules().line_touch_h * h;
    let gap = r.dash_gap_h * h;
    let follow = Follow {
        segs: &segs,
        ends: Ends::new(&segs, gap.max(touch)),
        cells: Cells::new(segs.iter().map(|g| (g.a, g.b)).collect(), (4.0 * touch).max(1e-6)),
        touch,
        gap,
        turn_cos: r.turn_deg.to_radians().cos(),
        straight_cos: r.straight_deg.to_radians().cos(),
    };
    let reach = r.label_reach_h * h;
    let lead_reach = r.leader_reach_h * h;
    // The leaders: from a label, piece by piece, to their far end. No leader is a service; the
    // line at a leader's far end is what its label names.
    let mut leader_pieces: BTreeSet<usize> = BTreeSet::new();
    let mut pointed: Vec<Vec<[f64; 2]>> = vec![Vec::new(); found.len()];
    for (li, lab) in found.iter().enumerate() {
        // At a line's start or end: within its first sixth or past its middle along it (the
        // lettering's length is estimated long).
        let at_tip = |p: [f64; 2]| {
            lab.tips.chunks(2).any(|w| match w {
                [a, z] => {
                    let l = sitework::dist(*a, *z);
                    l > 1e-9 && {
                        let t = ((p[0] - a[0]) * (z[0] - a[0]) + (p[1] - a[1]) * (z[1] - a[1])) / (l * l);
                        t <= 0.15 || t >= 0.55
                    }
                }
                _ => false,
            })
        };
        for (i, s) in segs.iter().enumerate() {
            let (ga, gb) = (sitework::gap(&lab.bbox, s.a), sitework::gap(&lab.bbox, s.b));
            // A leader leaves its label's lettering at its start or its end.
            // Its far end off the lettering (the box is estimated long: half a height clear).
            let far = if ga <= lead_reach && gb > ga + 0.5 * h && at_tip(s.a) {
                s.b
            } else if gb <= lead_reach && ga > gb + 0.5 * h && at_tip(s.b) {
                s.a
            } else {
                continue;
            };
            // On along its pieces that meet end to end (its bend, its arrow).
            let mut mine: BTreeSet<usize> = [i].into_iter().collect();
            let mut far = far;
            let mut len = sitework::dist(s.a, s.b);
            for _ in 0..6 {
                let next = follow.ends.around(far).find_map(|node| {
                    let (j, e) = (node / 2, node % 2);
                    let sj = segs.get(j)?;
                    let (q, o) = if e == 0 { (sj.a, sj.b) } else { (sj.b, sj.a) };
                    (!mine.contains(&j) && sitework::dist(q, far) <= touch).then_some((j, o))
                });
                let Some((j, o)) = next else { break };
                // A line it ends on (two or more pieces there) is its target, not more leader.
                let meeting = follow
                    .ends
                    .around(far)
                    .filter(|n| segs.get(n / 2).is_some_and(|sj| sitework::dist(if n % 2 == 0 { sj.a } else { sj.b }, far) <= touch))
                    .count();
                if meeting > 2 {
                    break;
                }
                mine.insert(j);
                len += sitework::dist(far, o);
                far = o;
            }
            if len <= r.leader_max_h * h {
                leader_pieces.extend(mine);
                if let Some(v) = pointed.get_mut(li) {
                    v.push(far);
                }
            }
        }
    }
    // Each label's own piece: what its leader points at, else the nearest by it.
    let own_piece = |li: usize, lab: &Label| -> Vec<(usize, f64)> {
        let mut out: Vec<(usize, f64)> = Vec::new();
        let ptd = pointed.get(li).map(Vec::as_slice).unwrap_or_default();
        if ptd.is_empty() {
            for (i, s) in segs.iter().enumerate() {
                let g = seg_box(s.a, s.b, &lab.bbox);
                if g <= reach && !leader_pieces.contains(&i) {
                    out.push((i, g));
                }
            }
        } else {
            for end in ptd {
                for (i, s) in segs.iter().enumerate() {
                    let d = sitework::seg_dist(*end, s.a, s.b);
                    if d <= r.leader_end_h * h && !leader_pieces.contains(&i) {
                        out.push((i, d));
                    }
                }
            }
        }
        out.sort_by(|a, b| a.1.total_cmp(&b.1));
        out.dedup_by_key(|x| x.0);
        out
    };
    let owned: Vec<Vec<(usize, f64)>> = found.iter().enumerate().map(|(li, lab)| own_piece(li, lab)).collect();
    // A service is not followed through another label's piece of another size or service.
    let mut owner: HashMap<usize, BTreeSet<(usize, u64)>> = HashMap::new();
    // Which service labels name each piece: a service through several labelled pieces prints
    // its length piece by piece.
    let mut named_by: HashMap<usize, Vec<usize>> = HashMap::new();
    for (li, (lab, own)) in found.iter().zip(&owned).enumerate() {
        if let Some((i, _)) = own.first() {
            owner.entry(*i).or_default().insert((lab.kind, lab.size_in.to_bits()));
            named_by.entry(*i).or_default().push(li);
        }
    }
    // A line another label sizes otherwise (`12" PVC` by a storm drain) is another line.
    for t in refs.iter().take(MAX_TEXTS) {
        let n = norm(&unquoted(&t.value));
        if kind_of(&n).is_some() || has_word(&n, &r.fitting_words) {
            continue;
        }
        let Some(size) = size_in(&n) else { continue };
        let bb = sitework::text_box(t);
        // Its line, both sides of a pipe drawn double: the nearest and those as near within a
        // height of it.
        let near: Vec<(usize, f64)> = segs.iter().enumerate().map(|(i, s)| (i, seg_box(s.a, s.b, &bb))).filter(|(_, g)| *g <= reach).collect();
        let least = near.iter().map(|x| x.1).fold(f64::INFINITY, f64::min);
        for (i, g) in near {
            if g <= least + h {
                owner.entry(i).or_default().insert((usize::MAX, size.to_bits()));
            }
        }
    }
    let entry = r.entry_reach_ft * upf;
    let chain = r.chain_gap_ft * upf;
    let min_len = r.min_line_ft * upf;
    let mut out: Vec<Service> = Vec::new();
    for (li, (lab, own)) in found.iter().zip(&owned).enumerate() {
        let key = (lab.kind, lab.size_in.to_bits());
        // The labels of one service (its size and kind) a set of pieces carries.
        let labels_on = |pieces: &BTreeSet<usize>| -> BTreeSet<usize> {
            pieces
                .iter()
                .filter_map(|i| named_by.get(i))
                .flatten()
                .copied()
                .filter(|j| found.get(*j).is_some_and(|o| (o.kind, o.size_in.to_bits()) == key))
                .chain([li])
                .collect()
        };
        let foreign: BTreeSet<usize> =
            owner.iter().filter(|(_, o)| !o.contains(&key)).map(|(i, _)| *i).chain(leader_pieces.iter().copied()).collect();
        let mut best: Option<(f64, Service)> = None;
        let mut tried: BTreeSet<usize> = BTreeSet::new();
        for &(start, d) in own.iter().take(40) {
            if tried.contains(&start) || foreign.contains(&start) {
                continue;
            }
            let (pts, used) = trace(&follow, start, &foreign);
            tried.extend(used.iter().copied());
            if pts.len() < 2 || sitework::path_len(&pts) < min_len {
                continue;
            }
            let mut pts = pts;
            let mut pieces = used;
            // Site end first.
            if to_outline(outline, pts.first().copied().unwrap_or_default()) < to_outline(outline, pts.last().copied().unwrap_or_default()) {
                pts.reverse();
            }
            // Through a vault, meter or backflow preventer drawn between its pieces: the next
            // line that starts by this one's end and comes nearer the building.
            for _ in 0..r.chain_steps {
                let Some(end) = pts.last().copied() else { break };
                let here = to_outline(outline, end);
                if here <= entry {
                    break;
                }
                let mut next: Option<(f64, Vec<[f64; 2]>, BTreeSet<usize>)> = None;
                for (i, s) in segs.iter().enumerate() {
                    if pieces.contains(&i) || foreign.contains(&i) {
                        continue;
                    }
                    let g = sitework::dist(end, s.a).min(sitework::dist(end, s.b));
                    if g > chain || next.as_ref().is_some_and(|(b, _, _)| g >= *b) {
                        continue;
                    }
                    let (mut q, u) = trace(&follow, i, &foreign);
                    if q.len() < 2 || u.iter().any(|x| pieces.contains(x)) {
                        continue;
                    }
                    if sitework::dist(end, q.first().copied().unwrap_or_default()) > sitework::dist(end, q.last().copied().unwrap_or_default()) {
                        q.reverse();
                    }
                    let (Some(q0), Some(qz)) = (q.first().copied(), q.last().copied()) else { continue };
                    if sitework::dist(end, q0) <= chain && to_outline(outline, qz) < here && sitework::path_len(&q) >= min_len {
                        next = Some((g, q, u));
                    }
                }
                let Some((_, q, u)) = next else { break };
                // A printed length is the service's: an unlabelled piece after it adds no more
                // than a vault (a piece its own label names prints its own).
                let labelled = u.iter().any(|i| named_by.get(i).is_some_and(|v| v.iter().any(|j| *j != li)));
                if let Some(lf) = lab.length_ft
                    && !labelled
                    && (sitework::path_len(&pts) + sitework::path_len(&q)) / upf > lf + r.chain_gap_ft
                {
                    break;
                }
                pieces.extend(u);
                pts.extend(q);
                if pts.len() > MAX_PIECES {
                    break;
                }
            }
            // It ends where it enters the building (what it is traced on beyond is the building's).
            if let Some(k) = pts.iter().position(|p| to_outline(outline, *p) <= 0.0) {
                pts.truncate(k + 1);
            }
            // The length its labels print is the line's (a line far off it is another).
            let printed: Option<f64> = labels_on(&pieces).iter().map(|j| found.get(*j).and_then(|o| o.length_ft)).sum();
            if let Some(lf) = printed {
                let len = sitework::path_len(&pts) / upf;
                if (len - lf).abs() > (r.length_slack_share * lf).max(r.length_slack_ft) {
                    continue;
                }
            }
            let (Some(site_end), Some(bldg_end)) = (pts.first().copied(), pts.last().copied()) else { continue };
            if to_outline(outline, bldg_end) > entry {
                continue;
            }
            // It runs to the building (a curb or a walk along the wall is no service).
            if to_outline(outline, site_end) - to_outline(outline, bldg_end) < r.approach_share * sitework::path_len(&pts) {
                continue;
            }
            let len_ft = sitework::path_len(&pts) / upf;
            let off = lab.length_ft.map_or(0.0, |l| (len_ft - l).abs() * upf * r.length_weight);
            let score = d + off;
            if best.as_ref().is_none_or(|(b, _)| score < *b) {
                best = Some((score, Service { label: lab.clone(), more: Vec::new(), pts, comps: pieces, kind: lab.kind, inferred_kind: false }));
            }
        }
        match best {
            Some((_, s)) => out.push(s),
            None => notes.push(format!("{}: no line of it reaches the building", lab.text)),
        }
    }
    // A service followed through another's line is one service: the one with more pieces keeps
    // both labels.
    out.sort_by(|a, b| b.comps.len().cmp(&a.comps.len()).then(a.label.text.cmp(&b.label.text)));
    let mut kept: Vec<Service> = Vec::new();
    for s in out {
        if let Some(k) = kept.iter_mut().find(|k| k.label.kind == s.label.kind && k.label.size_in == s.label.size_in && s.comps.is_subset(&k.comps)) {
            if !k.more.contains(&s.label.text) && k.label.text != s.label.text {
                k.more.push(s.label.text.clone());
            }
            if !k.label.designated && s.label.designated {
                k.label.designated = true;
            }
            continue;
        }
        kept.push(s);
    }
    (kept, notes)
}

// ---------------- what is built ----------------

/// A number after one of `words` in a text (`FF= 100.50`, `INV OUT = 95.25`).
fn number_after(v: &str, words: &[String]) -> Option<f64> {
    let n = norm(v);
    let toks: Vec<&str> = n.split_whitespace().collect();
    for w in words {
        let wt: Vec<&str> = w.split_whitespace().collect();
        if wt.is_empty() {
            continue;
        }
        for i in 0..toks.len() {
            if toks.get(i..i + wt.len()).is_some_and(|s| s == wt.as_slice()) {
                // The first number after it within four words.
                for t in toks.iter().skip(i + wt.len()).take(4) {
                    if let Some(x) = t.parse::<f64>().ok().filter(|x| x.is_finite() && x.abs() < 100_000.0) {
                        return Some(x);
                    }
                }
            }
        }
    }
    None
}

/// The point of connection a note by the building end prints (`5' FROM BUILDING`), feet.
fn poc_note(texts: &[TextItem], at: [f64; 2], reach: f64) -> Option<(f64, String)> {
    let r = rules();
    texts
        .iter()
        .take(MAX_TEXTS)
        .filter(|t| sitework::dist([t.at.x, t.at.y], at) <= reach)
        .filter_map(|t| {
            let n = norm(&t.value);
            if !has_word(&n, &r.poc_words) {
                return None;
            }
            let toks: Vec<&str> = n.split_whitespace().collect();
            let ft = toks.iter().enumerate().find_map(|(i, x)| {
                let v = x
                    .strip_suffix('\'')
                    .and_then(number)
                    .or_else(|| matches!(toks.get(i + 1).copied(), Some("FT" | "FEET" | "FOOT")).then(|| number(x)).flatten())?;
                (v.is_finite() && v > 0.0 && v <= 50.0).then_some(v)
            })?;
            Some((sitework::dist([t.at.x, t.at.y], at), ft, t.value.trim().to_string()))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, ft, v)| (ft, v))
}

/// The finished floor the plan prints for the building (`FF= 100.50`): the one nearest it.
fn finished_floor(texts: &[TextItem], outline: &[[f64; 2]]) -> Option<(f64, String)> {
    let r = rules();
    texts
        .iter()
        .take(MAX_TEXTS)
        .filter_map(|t| number_after(&t.value, &r.ff_words).map(|v| (to_outline(outline, [t.at.x, t.at.y]), v, t.value.trim().to_string())))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, v, s)| (v, s))
}

/// The invert printed by `at` (`INV 95.25`), its elevation as printed.
fn invert_near(texts: &[TextItem], at: [f64; 2], reach: f64) -> Option<(f64, String)> {
    let r = rules();
    texts
        .iter()
        .take(MAX_TEXTS)
        .filter(|t| sitework::dist([t.at.x, t.at.y], at) <= reach)
        .filter_map(|t| number_after(&t.value, &r.invert_words).map(|v| (sitework::dist([t.at.x, t.at.y], at), v, t.value.trim().to_string())))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, v, s)| (v, s))
}

/// A gravity line's least fall for its size, in/ft, by the rules' table.
fn fall_in_per_ft(size: f64) -> f64 {
    rules().slope.iter().find(|row| size <= row[0]).or(rules().slope.last()).map_or(0.125, |row| row[1])
}

/// The point on a path `d` back from its end, and the index of the segment it is on.
fn back_from_end(pts: &[[f64; 2]], d: f64) -> Option<(usize, [f64; 2])> {
    let mut left = d;
    for i in (1..pts.len()).rev() {
        let (Some(a), Some(b)) = (pts.get(i - 1), pts.get(i)) else { continue };
        let l = sitework::dist(*a, *b);
        if left <= l {
            let t = if l > 1e-12 { (l - left) / l } else { 0.0 };
            return Some((i - 1, [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]));
        }
        left -= l;
    }
    None
}

/// Where a path first crosses into a closed outline: the index of the segment and the point.
fn crossing(pts: &[[f64; 2]], poly: &[[f64; 2]]) -> Option<(usize, [f64; 2])> {
    for i in 1..pts.len() {
        let (Some(a), Some(b)) = (pts.get(i - 1).copied(), pts.get(i).copied()) else { continue };
        if sitework::inside(poly, a) || !sitework::inside(poly, b) {
            continue;
        }
        // Halve toward the edge.
        let (mut lo, mut hi) = (a, b);
        for _ in 0..40 {
            let m = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
            if sitework::inside(poly, m) {
                hi = m;
            } else {
                lo = m;
            }
        }
        return Some((i - 1, hi));
    }
    None
}

/// One piece of a service as built: its points, whether drawn, and its scope.
struct Piece {
    pts: Vec<[f64; 2]>,
    made: Made,
    civil: bool,
    basis: String,
}

/// What one sheet's services build.
#[derive(Clone, Debug, Default)]
pub(crate) struct Built {
    pub runs: Vec<MepRun>,
    pub lines: Vec<String>,
    pub findings: Vec<String>,
    /// Feet built, by `kind` and scope (`water building`, `water civil`).
    pub feet: BTreeMap<String, f64>,
}

fn entry_ends<'a>(sp: &'a BuildingSpec, slab: &str, kind: &Kind) -> Vec<(&'a MepRun, [f64; 2])> {
    sp.mep
        .runs
        .iter()
        .take(200_000)
        .filter(|r| {
            r.level.eq_ignore_ascii_case(slab)
                && kind.entry_systems.iter().any(|s| s.eq_ignore_ascii_case(&r.system))
                && !is_service_system(&r.system)
        })
        .flat_map(|r| r.points.first().into_iter().chain(r.points.last()).map(move |p| (r, *p)))
        .collect()
}

/// The runs one sheet's services build on the slab level `slab`. `texts`: the sheet's (for its
/// notes, inverts and finished floor); `fit` lays the sheet onto the model.
pub(crate) fn runs(sp: &BuildingSpec, read: &SiteRead, texts: &[TextItem], fit: &Fit, slab: &str) -> Built {
    let r = rules();
    let mut out = Built::default();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let scale = fit.reg.scale();
    if fit.sheet_outline.len() < 3 || fit.outline.len() < 3 || !(scale.is_finite() && scale > 0.0) {
        if !fit.sheet_outline.is_empty() || fit.method != "none" {
            out.findings.push(format!("{}: the civil plan's services not built: {}", read.sheet, fit.why));
        }
        return out;
    }
    let sheet_upf = upf / scale;
    let (mut found, notes) = services(read, texts, &fit.sheet_outline, sheet_upf);
    out.findings.extend(notes.into_iter().map(|n| format!("{}: {n}", read.sheet)));
    let sheet = read.sheet.as_str();
    let source = format!("{}{sheet}", sitework::SOURCE_PREFIX);
    let kind_ix = |k: &str| r.kinds.iter().position(|x| x.kind == k);
    // A water line named neither domestic nor fire: the fire line where the building takes a
    // separate domestic line, or only the fire protection plans' entry is by it.
    let domestic = found.iter().any(|s| r.kinds.get(s.kind).is_some_and(|k| k.kind == "water") && s.label.designated);
    for s in found.iter_mut() {
        if r.kinds.get(s.kind).is_none_or(|k| k.kind != "water") || s.label.designated {
            continue;
        }
        let end = s.pts.last().map(|p| fit.to_model(*p)).unwrap_or_default();
        let near = |k: Option<usize>| {
            k.and_then(|k| r.kinds.get(k))
                .is_some_and(|k| entry_ends(sp, slab, k).iter().any(|(_, p)| sitework::dist(*p, end) <= r.join_reach_ft * upf))
        };
        let (fire, water) = (kind_ix("fire"), kind_ix("water"));
        if (domestic || (near(fire) && !near(water)))
            && let Some(f) = fire
        {
            s.kind = f;
            s.inferred_kind = true;
        }
    }
    let ff = finished_floor(texts, &fit.sheet_outline);
    let grade_ft = r.grade_below_floor_in / 12.0;
    let model_outline = &fit.outline;
    let mut n = 0usize;
    for s in &found {
        let Some(kind) = r.kinds.get(s.kind) else { continue };
        n += 1;
        let size = s.label.size_in;
        let mut labels = vec![s.label.text.clone()];
        labels.extend(s.more.iter().cloned());
        let printed = labels.join("; ");
        let kind_note = if s.inferred_kind { format!(" ({} as the {}: {})", printed, kind.kind, r.undesignated_source) } else { String::new() };
        // On the model, from the site end.
        let mut pts: Vec<[f64; 2]> = s.pts.iter().map(|p| fit.to_model(*p)).collect();
        pts.dedup_by(|a, b| sitework::dist(*a, *b) < 1e-9);
        let Some(last) = pts.last().copied() else { continue };
        // To the wall: where it crosses into the building, else the wall nearest its end.
        let mut pieces: Vec<Piece> = Vec::new();
        let (outside, inside_pts, wall): (Vec<[f64; 2]>, Vec<[f64; 2]>, [f64; 2]) = match crossing(&pts, model_outline) {
            Some((i, x)) => {
                let mut o: Vec<[f64; 2]> = pts.iter().take(i + 1).copied().collect();
                o.push(x);
                let mut inn = vec![x];
                inn.extend(pts.iter().skip(i + 1).copied());
                (o, inn, x)
            }
            None => {
                let w = sitework::nearest_on(model_outline, last).unwrap_or(last);
                (pts.clone(), Vec::new(), w)
            }
        };
        // The point of connection: the plan's note by its end, else the rule's distance.
        let poc_reach = r.poc_reach_h * read.h;
        let (poc_ft, poc_basis) = match s.pts.last().and_then(|e| poc_note(texts, *e, poc_reach)) {
            Some((ft, note)) => (ft, format!("the civil plan's note \"{note}\"")),
            None => (r.poc_ft, r.poc_source.clone()),
        };
        let drawn_to = format!("drawn on {sheet} (civil utility plan, {}), \"{printed}\"", fit.method);
        // Outside: the drawn line, then the piece to the wall the civil plan leaves to the
        // building's trade.
        let mut out_path = outside.clone();
        let drawn_n = out_path.len();
        if inside_pts.is_empty() && sitework::dist(last, wall) > 0.25 * upf {
            out_path.push(wall);
        }
        let poc_d = poc_ft * upf;
        let split = back_from_end(&out_path, poc_d);
        let civil_scope = format!("site/civil scope: outside the point of connection {poc_ft:.0} ft from the wall ({poc_basis}; {})", r.scope_source);
        let trade_scope =
            format!("{} scope: from the point of connection {poc_ft:.0} ft outside the wall ({poc_basis}; {})", kind.trade, r.scope_source);
        // Each stretch of the outside path: drawn or carried to the wall, civil or the trade's.
        let seg_count = out_path.len().saturating_sub(1);
        let mut cur: Option<Piece> = None;
        for i in 0..seg_count {
            let (Some(a), Some(b)) = (out_path.get(i).copied(), out_path.get(i + 1).copied()) else { continue };
            let drawn = i + 1 < drawn_n;
            let parts: Vec<([f64; 2], [f64; 2], bool)> = match split {
                Some((k, p)) if k == i => vec![(a, p, true), (p, b, false)],
                Some((k, _)) => vec![(a, b, i < k)],
                None => vec![(a, b, false)],
            };
            for (pa, pb, civil) in parts {
                if sitework::dist(pa, pb) < 1e-6 {
                    continue;
                }
                let made = if drawn { Made::Drawn } else { Made::Derived };
                match cur.as_mut() {
                    Some(c) if c.civil == civil && c.made == made => c.pts.push(pb),
                    _ => {
                        if let Some(c) = cur.take() {
                            pieces.push(c);
                        }
                        let basis = if drawn {
                            format!("{drawn_to}{kind_note}; {}", if civil { &civil_scope } else { &trade_scope })
                        } else {
                            format!(
                                "derived: the civil plan's line stops {:.1} ft short of the wall; carried to the wall where the building's {} enters ({}){kind_note}; {trade_scope}",
                                sitework::dist(last, wall) / upf,
                                kind.trade,
                                r.line_source
                            )
                        };
                        cur = Some(Piece { pts: vec![pa, pb], made, civil, basis });
                    }
                }
            }
        }
        if let Some(c) = cur.take() {
            pieces.push(c);
        }
        // Inside: what the civil plan draws past the wall, then the join to the building's own
        // entry of its system.
        if inside_pts.len() >= 2 {
            pieces.push(Piece { pts: inside_pts.clone(), made: Made::Drawn, civil: false, basis: format!("{drawn_to}{kind_note}; {trade_scope}") });
        }
        let from = inside_pts.last().copied().unwrap_or(wall);
        let entry = entry_ends(sp, slab, kind)
            .into_iter()
            .map(|(run, p)| (sitework::dist(p, from), run, p))
            .filter(|(d, _, _)| *d <= r.join_reach_ft * upf)
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let mut entry_bottom: Option<f64> = None;
        match entry {
            Some((d, run, p)) if d > 1e-6 => {
                entry_bottom = run.bottoms_ft.first().copied();
                pieces.push(Piece {
                    pts: vec![from, p],
                    made: Made::Inferred,
                    civil: false,
                    basis: format!(
                        "inferred: joined to the building's {} entry drawn on {} ({}){kind_note}; {trade_scope}",
                        run.system, run.source_sheet, r.join_source
                    ),
                });
            }
            Some((_, run, _)) => entry_bottom = run.bottoms_ft.first().copied(),
            None => out.findings.push(format!(
                "{sheet}: \"{printed}\" reaches the wall with no {} entry on the building's plans within {:.0} ft: it ends at the wall",
                kind.kind, r.join_reach_ft
            )),
        }
        // Depth. A gravity line falls from the building at its size's least slope (from the
        // printed inverts where the plan prints them by its ends); a pressure line keeps its cover.
        let inv_reach = r.invert_reach_h * read.h;
        let printed_inv = |p: Option<&[f64; 2]>| p.and_then(|p| invert_near(texts, *p, inv_reach));
        let (site_inv, bldg_inv) = (printed_inv(s.pts.first()), printed_inv(s.pts.last()));
        let rel = |v: f64| ff.as_ref().map(|(f, _)| v - f);
        let cover_bottom = -(grade_ft + (kind.cover_in + size) / 12.0);
        let (bottom_at_wall, height_source): (f64, String) = match (bldg_inv.as_ref().and_then(|(v, _)| rel(*v)), entry_bottom) {
            (Some(b), _) if b < 0.0 && b > -50.0 => (
                b,
                format!(
                    "drawn: invert \"{}\" less the finished floor \"{}\" ({sheet})",
                    bldg_inv.as_ref().map_or("", |x| x.1.as_str()),
                    ff.as_ref().map_or("", |x| x.1.as_str())
                ),
            ),
            (_, Some(b)) if kind.gravity && b < 0.0 => (b, "derived: the building's entry invert its plumbing plan gives".to_string()),
            _ => (
                cover_bottom,
                format!(
                    "rule: {:.0} in cover below grade {:.0} in under the floor ({}; {})",
                    kind.cover_in, r.grade_below_floor_in, kind.cover_source, r.grade_source
                ),
            ),
        };
        let mut fall = if kind.gravity { fall_in_per_ft(size) } else { 0.0 };
        let total_out = sitework::path_len(&out_path);
        if kind.gravity
            && let Some(site) = site_inv.as_ref().and_then(|(v, _)| rel(*v))
            && total_out > 0.0
            && site < bottom_at_wall
        {
            fall = (bottom_at_wall - site) * 12.0 / (total_out / upf);
        }
        // Distance from the wall along the path, signed: positive outside.
        let wall_d = |p: [f64; 2]| -> f64 {
            // Outside path: distance back from the wall.
            let mut acc = 0.0;
            let mut best = f64::INFINITY;
            let mut at = 0.0;
            for i in (1..out_path.len()).rev() {
                let (Some(a), Some(b)) = (out_path.get(i - 1), out_path.get(i)) else { continue };
                let d = sitework::seg_dist(p, *a, *b);
                if d < best - 1e-9 {
                    best = d;
                    let l = sitework::dist(*a, *b);
                    at = acc + sitework::dist(p, *b).min(l);
                }
                acc += sitework::dist(*a, *b);
            }
            if best <= 1e-3 * upf { at } else { -sitework::dist(p, wall) }
        };
        let elev = |p: [f64; 2]| bottom_at_wall - fall / 12.0 * (wall_d(p) / upf);
        for (k, pc) in pieces.into_iter().enumerate() {
            if pc.pts.len() < 2 {
                continue;
            }
            let prefix = if pc.civil { &r.civil_prefix } else { &r.building_prefix };
            let system = format!("{prefix}-{}", kind.system);
            let mut pts = pc.pts;
            // A gravity line's points in its flow (from the building); a pressure line's from the
            // site.
            if kind.gravity {
                pts.reverse();
            }
            let bottoms: Vec<f64> = pts.windows(2).map(|w| w.first().map_or(bottom_at_wall, |p| elev(*p))).collect();
            let mut run = super::printcond::run(&system, slab, &source, pts, size, bottom_at_wall);
            run.bottoms_ft = bottoms;
            run.slope_in_per_ft = fall;
            run.made = pc.made;
            run.basis = if s.label.material.is_empty() { pc.basis } else { format!("{} ({}); {}", s.label.material, sheet, pc.basis) };
            run.size_source = format!("drawn: \"{}\" ({sheet})", s.label.text);
            run.height_source =
                if fall > 0.0 { format!("{height_source}; falls {fall:.4} in/ft ({})", r.slope_source) } else { height_source.clone() };
            run.id = format!("CS{n}-{}", k + 1);
            let ft = sitework::path_len(&run.points) / upf;
            *out.feet.entry(format!("{} {}", kind.kind, if pc.civil { "civil" } else { "building" })).or_default() += ft;
            out.runs.push(run);
        }
        let built_ft: f64 = out.runs.iter().filter(|x| x.id.starts_with(&format!("CS{n}-"))).map(|x| sitework::path_len(&x.points) / upf).sum();
        out.lines.push(format!(
            "{sheet}: {} {size} in{} \"{printed}\": {built_ft:.0} ft built ({}), point of connection {poc_ft:.0} ft out{}",
            kind.kind,
            if s.label.material.is_empty() { String::new() } else { format!(" {}", s.label.material) },
            fit.method,
            s.label.length_ft.map_or(String::new(), |l| format!(", {l:.0} ft printed"))
        ));
    }
    out
}

/// The pipe systems the runs need in the spec, each once.
pub(crate) fn systems(sp: &mut BuildingSpec, runs: &[MepRun]) {
    for name in runs.iter().map(|r| r.system.clone()).collect::<BTreeSet<_>>() {
        if !sp.mep.systems.iter().any(|y| y.name.eq_ignore_ascii_case(&name)) {
            sp.mep.systems.push(MepSystem {
                name: name.clone(),
                layer: name,
                kind: MepKind::Pipe,
                size_in: [4.0, 0.0],
                bottom_ft: -4.0,
                show: true,
                duct_type: None,
                color_index: 0,
            });
        }
    }
}

/// Two services the same: one kind, their ends each within `tol` (a line two sheets draw).
pub(crate) fn same_service(a: &MepRun, b: &MepRun, tol: f64) -> bool {
    let near = |p: Option<&[f64; 2]>, q: Option<&[f64; 2]>| matches!((p, q), (Some(p), Some(q)) if sitework::dist(*p, *q) <= tol);
    a.system == b.system
        && ((near(a.points.first(), b.points.first()) && near(a.points.last(), b.points.last()))
            || (near(a.points.first(), b.points.last()) && near(a.points.last(), b.points.first())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_labels_say_what_they_print() {
        let t = |v: &str| TextItem { value: v.into(), at: cadaddy_geom::Vec2::new(0.0, 0.0), height: 10.0, rotation: 0.0 };
        let all = [
            t("40 L.F. 3\" DOM. W.L."),
            t("52 L.F. 8\" W.L. (PRIVATE)"),
            t("10\" S.S. (PRIVATE)"),
            t("EX. 12\" W.L."),
            t("210 L.F 8\" W.L. (PUBLIC)"),
            t("1 - 8\" GATE VALVE & VALVE BOX"),
            t("18\" R.C.P."),
            t("8\" FIRE LINE DIP"),
            t("2\" GAS SERVICE"),
            t("12\" STORM SEWER"),
        ];
        let refs: Vec<&TextItem> = all.iter().collect();
        let got = labels(&refs);
        let kinds: Vec<(String, f64, bool, Option<f64>)> = got
            .iter()
            .map(|l| (rules().kinds.get(l.kind).map(|k| k.kind.clone()).unwrap_or_default(), l.size_in, l.designated, l.length_ft))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("water".into(), 3.0, true, Some(40.0)),
                ("water".into(), 8.0, false, Some(52.0)),
                ("sewer".into(), 10.0, true, None),
                ("fire".into(), 8.0, true, None),
                ("gas".into(), 2.0, true, None),
            ]
        );
        assert_eq!(got.get(3).map(|l| l.material.as_str()), Some("DIP"));
        assert_eq!(number_after("FF= 100.50", &rules().ff_words), Some(100.5));
        assert_eq!(number_after("8\" INV OUT = 95.25", &rules().invert_words), Some(95.25));
        assert_eq!(size_in(&norm("1.5\" GAS")), Some(1.5));
        assert!(is_service_system("P-UG-CW") && is_service_system("C-UG-SAN") && !is_service_system("E-UG"));
    }

    use cadaddy_doc::{Common, Drawing, EntityKind, HAlign, Handle, Line, LwPolyline, Space, Text, VAlign};
    use cadaddy_geom::{PolyVertex, Vec2, Vec3};

    fn add(d: &mut Drawing, layer: &str, kind: EntityKind) -> Handle {
        d.add(&Space::Model, Common { layer: layer.into(), ..Common::default() }, kind).unwrap_or(Handle(0))
    }
    fn text(d: &mut Drawing, s: &str, x: f64, y: f64, h: f64) {
        let t = Text {
            insert: Vec3::new(x, y, 0.0),
            align_pt: None,
            height: h,
            value: s.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: HAlign::Left,
            valign: VAlign::Baseline,
        };
        add(d, "C-ANNO", EntityKind::Text(t));
    }
    fn pline(d: &mut Drawing, layer: &str, pts: &[[f64; 2]], closed: bool) {
        let vertices = pts.iter().map(|p| PolyVertex::new(Vec2::new(p[0], p[1]))).collect();
        add(d, layer, EntityKind::LwPolyline(LwPolyline { vertices, closed, const_width: 0.0, elevation: 0.0, plinegen: false }));
    }
    fn line(d: &mut Drawing, layer: &str, a: [f64; 2], b: [f64; 2]) {
        add(d, layer, EntityKind::Line(Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) }));
    }

    /// A civil utility plan at its printed scale (drawing units inches): the building's outline
    /// (80 x 50 ft), the street's water main north of it with a 3 in domestic service through a
    /// meter vault (stopping 5 ft out, by its note) and an 8 in private water line named neither
    /// domestic nor fire, a 2 in gas service labelled by a leader, and south of it a 6 in sewer
    /// from the building to the sewer main with its invert and the finished floor printed. Curbs
    /// and a hydrant lead that reach no building are no services.
    fn utility_plan() -> (Drawing, [f64; 4]) {
        let mut d = Drawing::default();
        let h = 12.0;
        pline(&mut d, "C-BLDG-OUTLINE", &[[20_000.0, 5_000.0], [20_960.0, 5_000.0], [20_960.0, 5_600.0], [20_000.0, 5_600.0]], true);
        text(&mut d, "FF= 600.00", 20_400.0, 5_300.0, h);
        // The water main and its services.
        line(&mut d, "C-WATR", [19_500.0, 6_400.0], [21_500.0, 6_400.0]);
        text(&mut d, "EX. 12\" W.L.", 19_600.0, 6_420.0, h);
        line(&mut d, "C-WATR", [20_400.0, 6_400.0], [20_400.0, 6_100.0]);
        pline(&mut d, "C-WATR-SYMB", &[[20_380.0, 6_060.0], [20_420.0, 6_060.0], [20_420.0, 6_100.0], [20_380.0, 6_100.0]], true);
        line(&mut d, "C-WATR", [20_400.0, 6_060.0], [20_400.0, 5_660.0]);
        text(&mut d, "25 L.F. 3\" DOM. W.L.", 20_415.0, 6_250.0, h);
        text(&mut d, "33 L.F. 3\" DOM. W.L.", 20_415.0, 5_850.0, h);
        text(&mut d, "5' FROM BUILDING", 20_415.0, 5_680.0, h);
        line(&mut d, "C-UTIL-PIPE", [20_700.0, 6_400.0], [20_700.0, 5_600.0]);
        text(&mut d, "8\" W.L.", 20_715.0, 6_000.0, h);
        text(&mut d, "(PRIVATE)", 20_715.0, 5_985.0, h);
        // A hydrant lead off the main: it reaches no building.
        line(&mut d, "C-WATR", [19_700.0, 6_400.0], [19_700.0, 6_200.0]);
        text(&mut d, "16 L.F. 6\" W.L.", 19_715.0, 6_300.0, h);
        // The gas service: its label off to the side, a leader to its line.
        line(&mut d, "C-GAS", [20_880.0, 6_700.0], [20_880.0, 5_650.0]);
        text(&mut d, "2\" GAS SERVICE", 21_200.0, 6_200.0, h);
        pline(&mut d, "C-ANNO", &[[21_195.0, 6_205.0], [21_000.0, 6_150.0], [20_882.0, 6_150.0]], false);
        // A curb crossing the services.
        line(&mut d, "C-CURB", [19_500.0, 6_150.0], [21_500.0, 6_150.0]);
        // The sewer.
        line(&mut d, "C-SSWR", [19_500.0, 4_400.0], [21_500.0, 4_400.0]);
        text(&mut d, "EX. 8\" S.S.", 19_600.0, 4_420.0, h);
        pline(&mut d, "C-SSWR", &[[20_200.0, 4_400.0], [20_200.0, 4_700.0], [20_250.0, 4_940.0]], false);
        text(&mut d, "6\" S.S. SDR 35", 20_215.0, 4_600.0, h);
        text(&mut d, "INV 594.00", 20_215.0, 4_420.0, h);
        (d, [19_000.0, 4_000.0, 22_000.0, 7_000.0])
    }

    /// The building: 80 x 50 ft square to the model, its plumbing and fire protection plans'
    /// entries just inside the north and south walls.
    fn building() -> BuildingSpec {
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..BuildingSpec::default() };
        let level: cadaddy_walk3d::Level = serde_json::from_value(
            serde_json::json!({"name": "Level 1", "elevation_ft": 0.0, "height_ft": 12.0, "region": [0.0, 0.0, 2_000.0, 1_500.0]}),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        sp.levels.push(level);
        sp.shells.insert(
            "Level 1".into(),
            vec![[100.0, 100.0, 1_060.0, 106.0], [100.0, 694.0, 1_060.0, 700.0], [100.0, 100.0, 106.0, 700.0], [1_054.0, 100.0, 1_060.0, 700.0]],
        );
        let entry = |system: &str, pts: Vec<[f64; 2]>, bottom: f64| super::super::printcond::run(system, "Level 1", "P-101", pts, 3.0, bottom);
        sp.mep.runs.push(entry("CW", vec![[500.0, 400.0], [500.0, 690.0]], -1.0));
        sp.mep.runs.push(entry("FP", vec![[800.0, 400.0], [800.0, 690.0]], -1.0));
        sp.mep.runs.push(entry("SAN", vec![[350.0, 400.0], [350.0, 110.0]], -2.0));
        sp
    }

    /// The underground plan step's rerun keeps the site services' building pieces (they are the
    /// site work step's), and replaces its own.
    #[test]
    fn the_underground_step_leaves_the_site_services_alone() {
        let mine = super::super::printcond::run(
            "P-UG-CW",
            "Level 1",
            &format!("{}C-401", sitework::SOURCE_PREFIX),
            vec![[0.0, 0.0], [12.0, 0.0]],
            3.0,
            -3.0,
        );
        let theirs = super::super::printcond::run("P-UG-SAN", "Level 1", "P-101", vec![[0.0, 0.0], [12.0, 0.0]], 4.0, -2.0);
        assert!(!super::super::ugplan::made_here(&mine));
        assert!(super::super::ugplan::made_here(&theirs));
    }

    fn outside(p: &[f64; 2]) -> bool {
        p[0] < 100.0 - 1e-6 || p[0] > 1_060.0 + 1e-6 || p[1] < 100.0 - 1e-6 || p[1] > 700.0 + 1e-6
    }

    /// Q6597: the civil utility plan's water, fire, sewer and gas services into the building are
    /// built as runs: on the model outside the footprint, carried to the wall, joined to the
    /// building's own entries, split at the point of connection (the plan's note, else the rule)
    /// into the building trade's and the site/civil scope, with their printed sizes, material and
    /// a sewer's fall between its printed invert and the building's. Before: no pipe outside the
    /// building at all.
    #[test]
    fn the_civil_utility_plans_services_are_built_to_the_building() {
        use super::super::guided::{GuidedJob, JobGoal, SheetRegion, SheetScale, site_work_step};
        let mut s = crate::Session::new();
        let (plan, area) = utility_plan();
        if let Ok(d) = s.doc_mut() {
            for e in plan.model.iter() {
                let _ = d.add(&Space::Model, e.common.clone(), e.kind.clone());
            }
            d.building_spec = Some(building().to_json()).into();
        }
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        job.sheet_regions = vec![SheetRegion {
            number: "C-401".into(),
            level: String::new(),
            discipline: "C".into(),
            kind: "plan".into(),
            rect: area,
            area: Some(area),
            title_block: None,
            not_built: "site/civil".into(),
            levels: Vec::new(),
            view: None,
        }];
        let scale: SheetScale = serde_json::from_value(serde_json::json!({"sheet": "C-401", "scale": 240.0, "source": "scale note 1\"=20'"}))
            .unwrap_or_else(|e| panic!("{e}"));
        job.sheet_scales.push(scale);
        let mut counts = Vec::new();
        for _ in 0..2 {
            let mut result = serde_json::json!({});
            site_work_step(&mut s, &mut job, &mut result).unwrap_or_else(|e| panic!("{e}"));
            let w = &result["site_work"]["civil_services"];
            let (sp, _) = super::super::bspec::current(&s, "test").unwrap_or_else(|e| panic!("{e}"));
            let mine: Vec<&MepRun> = sp.mep.runs.iter().filter(|r| is_service_system(&r.system)).collect();
            let of = |sys: &str| mine.iter().filter(|r| r.system == sys).copied().collect::<Vec<_>>();
            let ft = |rs: &[&MepRun]| rs.iter().map(|r| sitework::path_len(&r.points) / 12.0).sum::<f64>();
            // Pipe outside the footprint, for each service.
            for sys in ["C-UG-CW", "C-UG-FP", "C-UG-SAN", "C-UG-G"] {
                assert!(of(sys).iter().any(|r| r.points.iter().any(outside)), "{sys}: {w:#}");
            }
            // The domestic service: main to the vault and on to its note's point of connection
            // (5 ft out) by the civil plan, the building's 5 ft to the wall, joined to its entry.
            let cw_civil = ft(&of("C-UG-CW"));
            assert!((cw_civil - 61.67).abs() < 0.5, "{cw_civil} {w:#}");
            let cw_bldg = of("P-UG-CW");
            assert!((ft(&cw_bldg) - (5.0 + 10.0 / 12.0)).abs() < 0.2, "{} {w:#}", ft(&cw_bldg));
            assert!(cw_bldg.iter().any(|r| r.made == Made::Inferred && r.points.last().is_some_and(|p| sitework::dist(*p, [500.0, 690.0]) < 1e-6)));
            assert!(cw_bldg.iter().any(|r| r.made == Made::Derived && r.basis.contains("5' FROM BUILDING")), "{cw_bldg:#?}");
            assert!(of("C-UG-CW").iter().all(|r| r.size_in[0] == 3.0 && r.made == Made::Drawn && r.basis.contains("site/civil scope")));
            // The water line named neither: the fire line (the building takes a domestic line),
            // its point of connection by rule.
            let fp = of("C-UG-FP");
            assert!(fp.iter().all(|r| r.size_in[0] == 8.0 && r.basis.contains("as the fire")), "{fp:#?}");
            assert!((ft(&fp) - 61.67).abs() < 0.5 && (ft(&of("P-UG-FP")) - (5.0 + 10.0 / 12.0)).abs() < 0.2, "{w:#}");
            // The sewer: its material as printed, falling from the building's entry invert to
            // the printed invert at the main (FF 600.00, INV 594.00: 6 ft below the floor).
            let san: Vec<&MepRun> = mine.iter().filter(|r| r.system.ends_with("-SAN")).copied().collect();
            assert!(san.iter().all(|r| r.basis.contains("SDR 35") && r.slope_in_per_ft > 0.0), "{san:#?}");
            let lowest = san.iter().flat_map(|r| r.bottoms_ft.iter().copied()).fold(f64::INFINITY, f64::min);
            let highest = san.iter().flat_map(|r| r.bottoms_ft.iter().copied()).fold(f64::NEG_INFINITY, f64::max);
            assert!(lowest < -4.0 && highest >= -2.0 - 1e-6, "{lowest} {highest} {san:#?}");
            assert!(of("P-UG-SAN").iter().any(|r| r.points.first().is_some_and(|p| sitework::dist(*p, [350.0, 110.0]) < 1e-6)), "{san:#?}");
            // The gas service: no gas entry on the building's plans: built to the wall and said.
            assert!(
                w["findings"]
                    .as_array()
                    .is_some_and(|f| f.iter().any(|x| x.as_str().is_some_and(|t| t.contains("GAS") && t.contains("ends at the wall")))),
                "{w:#}"
            );
            // Nothing from the existing mains, the curb or the hydrant lead.
            assert_eq!(mine.iter().map(|r| r.system.split('-').next_back().unwrap_or("")).collect::<BTreeSet<_>>().len(), 4, "{w:#}");
            counts.push((mine.len(), (ft(&mine) * 10.0).round()));
            // Not in the electrical trench.
            assert!(sitework::trench_ft(&sp).is_empty());
        }
        // A rerun builds the same again, never twice.
        assert_eq!(counts.first(), counts.get(1), "{counts:?}");
    }

    /// The same kind of plan as a print set's job reads it: every line on one layer (the PDF's
    /// layers flattened), the building drawn wall by wall with a door's gap and its rooms' lines,
    /// no closed outline; labels by leaders; parking stripes, a curb and a storm drain drawn
    /// double (`12" PVC`) to the building; a hydrant lead under its callout.
    fn flattened_plan() -> (Drawing, [f64; 4]) {
        let mut d = Drawing::default();
        let h = 12.0;
        let g = "PDF-GEOMETRY";
        // The building, wall by wall (a door's gap in the north wall), and two room lines.
        for (a, b) in [
            ([20_000.0, 5_600.0], [20_400.0, 5_600.0]),
            ([20_436.0, 5_600.0], [20_960.0, 5_600.0]),
            ([20_960.0, 5_600.0], [20_960.0, 5_000.0]),
            ([20_960.0, 5_000.0], [20_000.0, 5_000.0]),
            ([20_000.0, 5_000.0], [20_000.0, 5_600.0]),
            ([20_480.0, 5_000.0], [20_480.0, 5_600.0]),
            ([20_000.0, 5_300.0], [20_480.0, 5_300.0]),
        ] {
            line(&mut d, g, a, b);
        }
        text(&mut d, "BUILDING A", 20_600.0, 5_300.0, h);
        text(&mut d, "FF=600.00", 20_600.0, 5_270.0, h);
        // Parking stripes and a curb east of it.
        for k in 0..8 {
            let x = 21_200.0 + 108.0 * f64::from(k);
            line(&mut d, g, [x, 5_000.0], [x, 5_216.0]);
        }
        line(&mut d, g, [21_100.0, 4_900.0], [22_100.0, 4_900.0]);
        // The water main, the fire line (its label off to the east, by a leader in two pieces),
        // the domestic service with its note.
        line(&mut d, g, [19_500.0, 6_400.0], [21_500.0, 6_400.0]);
        line(&mut d, g, [20_700.0, 6_400.0], [20_700.0, 5_600.0]);
        text(&mut d, "8\" FIRE LINE DIP", 21_000.0, 6_050.0, h);
        pline(&mut d, g, &[[20_995.0, 6_055.0], [20_760.0, 6_000.0]], false);
        line(&mut d, g, [20_760.0, 6_000.0], [20_702.0, 6_000.0]);
        line(&mut d, g, [20_400.0, 6_400.0], [20_400.0, 5_660.0]);
        text(&mut d, "62 L.F. 3\" DOM. W.L.", 20_415.0, 6_250.0, h);
        text(&mut d, "5' FROM BLDG", 20_415.0, 5_680.0, h);
        // A storm drain drawn double to the building, its size by it.
        line(&mut d, g, [20_330.0, 6_300.0], [20_330.0, 5_600.0]);
        line(&mut d, g, [20_340.0, 6_300.0], [20_340.0, 5_600.0]);
        text(&mut d, "12\" PVC", 20_250.0, 6_150.0, h);
        // A hydrant lead off the main toward the building's corner, under its callout.
        line(&mut d, g, [19_900.0, 6_400.0], [19_900.0, 5_650.0]);
        text(&mut d, "1 - FIRE HYDRANT ASSEMBLY", 19_700.0, 6_110.0, h);
        text(&mut d, "62 L.F. 6\" W.L.", 19_700.0, 6_090.0, h);
        // The sewer from the building to its manhole, labelled by a leader.
        line(&mut d, g, [20_250.0, 4_300.0], [20_250.0, 4_940.0]);
        text(&mut d, "6\" SAN SEWER", 19_900.0, 4_600.0, h);
        line(&mut d, g, [20_010.0, 4_606.0], [20_248.0, 4_650.0]);
        text(&mut d, "INV 594.00", 20_265.0, 4_310.0, h);
        (d, [19_000.0, 4_000.0, 22_500.0, 7_000.0])
    }

    /// Q6597, the flattened print: the plan is laid on the building by its linework (no closed
    /// outline, no layer names), and its fire line, domestic service and sewer are built; the
    /// storm drain, the hydrant lead, the main, the curb and the stripes are not.
    #[test]
    fn a_flattened_utility_plan_is_laid_by_its_linework_and_its_services_built() {
        use super::super::guided::{GuidedJob, JobGoal, SheetRegion, SheetScale, site_work_step};
        let mut s = crate::Session::new();
        let (plan, area) = flattened_plan();
        if let Ok(d) = s.doc_mut() {
            for e in plan.model.iter() {
                let _ = d.add(&Space::Model, e.common.clone(), e.kind.clone());
            }
            d.building_spec = Some(building().to_json()).into();
        }
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        job.sheet_regions = vec![SheetRegion {
            number: "C-5".into(),
            level: String::new(),
            discipline: "C".into(),
            kind: "plan".into(),
            rect: area,
            area: Some(area),
            title_block: None,
            not_built: "site/civil".into(),
            levels: Vec::new(),
            view: None,
        }];
        let scale: SheetScale = serde_json::from_value(serde_json::json!({"sheet": "C-5", "scale": 360.0, "source": "scale note 1\"=30'"}))
            .unwrap_or_else(|e| panic!("{e}"));
        job.sheet_scales.push(scale);
        let mut result = serde_json::json!({});
        site_work_step(&mut s, &mut job, &mut result).unwrap_or_else(|e| panic!("{e}"));
        let w = &result["site_work"]["civil_services"];
        assert!(w["sheets"].as_array().is_some_and(|a| a.iter().any(|x| x.as_str().is_some_and(|t| t.contains("building linework")))), "{w:#}");
        let (sp, _) = super::super::bspec::current(&s, "test").unwrap_or_else(|e| panic!("{e}"));
        let mine: Vec<&MepRun> = sp.mep.runs.iter().filter(|r| is_service_system(&r.system)).collect();
        let systems: BTreeSet<&str> = mine.iter().map(|r| r.system.as_str()).collect();
        assert_eq!(systems, ["C-UG-CW", "C-UG-FP", "C-UG-SAN", "P-UG-CW", "P-UG-FP", "P-UG-SAN"].into_iter().collect(), "{w:#}");
        // Laid where the building is: the fire line onto its entry, the sewer onto its own.
        assert!(
            mine.iter().any(|r| r.system == "P-UG-FP" && r.points.last().is_some_and(|p| sitework::dist(*p, [800.0, 690.0]) < 1e-6)),
            "{mine:#?}"
        );
        assert!(
            mine.iter().any(|r| r.system == "P-UG-SAN" && r.points.first().is_some_and(|p| sitework::dist(*p, [350.0, 110.0]) < 1e-6)),
            "{mine:#?}"
        );
        // Sizes as printed; nothing 6 in (the hydrant lead) or 12 in (the storm drain).
        assert!(mine.iter().all(|r| [3.0, 6.0, 8.0].contains(&r.size_in[0])), "{mine:#?}");
        assert!(mine.iter().filter(|r| r.system.ends_with("-SAN")).all(|r| r.size_in[0] == 6.0));
        assert!(mine.iter().filter(|r| r.system.ends_with("-CW")).all(|r| r.size_in[0] == 3.0));
        assert!(mine.iter().filter(|r| r.system.ends_with("-FP")).all(|r| r.size_in[0] == 8.0 && r.basis.contains("DIP")));
        // Outside the footprint, each.
        for sys in ["C-UG-CW", "C-UG-FP", "C-UG-SAN"] {
            assert!(mine.iter().any(|r| r.system == sys && r.points.iter().any(outside)), "{sys}");
        }
    }
}
