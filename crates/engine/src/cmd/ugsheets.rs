//! Underground and under-slab field drawings (Q330): from the building spec, everything buried
//! under the slab or in the site — conduit runs below the floor (`bottoms_ft` under 0), the
//! utility and service entrances (`mep.services`: power, telephone, CATV, fire alarm monitoring,
//! gas meter bonding) — laid out for the field before the slab is poured.
//!
//! - `ugsheets.check`: the buried runs (size, count, material, depth, length, what each end
//!   feeds), the stub-ups dimensioned from the nearest grid lines (else the level's edges) with
//!   coordinates for layout tools and their heights, the services the building needs and whether
//!   the drawings show them, the pour hold points (Q328) of the slab, and a route review: each
//!   feeder between gear on the slab level that no run routes yet, under the slab against
//!   overhead, by length, conduit and wire and labor at the estimator's prices and crew rate
//!   (Q327). Routing only: sizes are the feeder schedule's.
//! - `ugsheets.review`: an RFI for each required service the drawings don't show or show too
//!   unclearly to lay out (`service_entrance`).
//! - `ugsheets.build`: one paper space sheet per level with buried work: the plan (grids, gear,
//!   runs, stub-ups with their dimensions), the run and stub-up schedules, the hold points and
//!   the notes, in the project's title block when the drawing has one; optionally exported to
//!   PDF and DWG.

use std::collections::BTreeMap;

use cadaddy_walk3d::{BuildingSpec, MepEquipment, MepKind, Service};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Runs, stub-ups and findings looked at, at most (hostile specs).
const MAX_ITEMS: usize = 5_000;
/// Points of one run used, at most.
const MAX_POINTS: usize = 1_000;
/// A run's end within this of a piece of equipment feeds it (feet).
const FEEDS_WITHIN_FT: f64 = 3.0;
/// Grid lines whose columns stray more than this from one line aren't a line (feet).
const GRID_STRAY_FT: f64 = 1.0;
/// Depth of a feeder under the slab and the riser at each end, for the route review (feet).
const UNDER_SLAB_DEPTH_FT: f64 = 2.0;
const STUB_RISE_FT: f64 = 1.0;
/// Under the slab is suggested when it costs less than this share of overhead.
const CHEAPER: f64 = 0.95;
/// The sheet (inches on paper): size, the plan's box, the schedules' column.
const SHEET: [f64; 2] = [36.0, 24.0];
const PLAN: [f64; 4] = [1.0, 2.5, 24.5, 22.5];
const SIDE_X: f64 = 25.5;
/// Characters in a line of the notes (0.08" text across the side column).
const NOTE_CHARS: usize = 110;
/// A buried electrical run this close to another trade's line (plan, feet) is a crossing to
/// check, unless `clearance_in` says otherwise.
const CLEARANCE_FT: f64 = 1.0;
/// Clear vertical distance under which a crossing is an error (inches).
const MIN_SEPARATION_IN: f64 = 12.0;
/// The most bends between pull points (degrees).
const MAX_BENDS_DEG: f64 = 360.0;
/// Segment pairs the crossing check looks at, at most (hostile specs).
const MAX_PAIR_TESTS: usize = 2_000_000;
/// The trench sections' box at the foot of the side column (paper inches): x, y, width, height
/// of each of the two.
const SECTION_BOX: [f64; 4] = [SIDE_X, 2.3, 4.9, 4.2];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("ugsheets.check", "Check Underground", run_check)
            .params("{today?: YYYY-MM-DD, clearance_in?: plan distance to another trade's buried line checked as a crossing (default 12)} → {ok, errors, warnings, runs: [{id, kind, system, level, from, to, size_in, conduits, material, depth_in, length_ft, bends_deg, encased}], stubups: [{tag, level, at, feeds, run, size_in, conduits, material, height_in, x: {from, ft}, y: {from, ft}, coords_ft}], services: [{kind, tag, status, to, at, encased}], other_trade: [{id, label, level, depth_in}], crossings: [{id, level, at, electrical, other, how, distance_ft, vertical_separation_in}], routes: [{feeder, from, to, level, under_slab: {length_ft, cost?}, overhead: {length_ft, cost?}, basis, suggest}], holds: [{pour, item, kind, size, status}], findings: [{check, severity, about, level, message, fix}]}")
            .noundo(),
        CommandSpec::new("ugsheets.review", "Review Underground", run_review)
            .params("{} → {errors, rfis: [id], findings}: an RFI (service_entrance) for each required service not shown or unclear"),
        CommandSpec::new("ugsheets.build", "Underground Sheets", run_build)
            .params("{level?: name (default: every level with buried work), number?: first sheet number (default \"E-001\"), called?: [{sheet, level?, text}] (buried work the documents' notes call for that no plan locates: listed in the notes), title_block?: block name (default: a block named like a title block), today?, clearance_in?, pdf?: path, dwg?: path} → {sheets: [{layout, number, title, level, runs, stubups, dimensions, crossings, sections, replaced}], files: [path]}: building again replaces the sheet's layout"),
    ]
}

// ---------------- the model ----------------

/// A run in the ground or under the slab.
#[derive(Clone, Debug)]
struct Buried {
    id: String,
    /// `run` (a conduit run below the floor) or `service` (a utility entrance).
    kind: &'static str,
    system: String,
    level: String,
    points: Vec<[f64; 2]>,
    size_in: f64,
    conduits: u32,
    material: String,
    depth_in: f64,
    length_ft: f64,
    from: String,
    to: String,
    /// The bends along the route plus a 90 at each end that comes up (degrees).
    bends_deg: f64,
    /// In a concrete encasement.
    encased: bool,
    /// Not called by the documents: CADaddy proposes it (drawn dashed, with an RFI).
    proposed: bool,
    /// Where it comes from: the note or schedule that calls it, or what the proposal is for.
    basis: String,
}

/// Buried work a note of the documents calls for (its sheet, the level when known, its words).
struct Called {
    sheet: String,
    level: String,
    text: String,
}

/// Notes of called work a sheet lists, at most.
const MAX_CALLED: usize = 60;
/// The first note of every generated sheet.
const GENERATED_NOTE: &str = "THIS SHEET IS GENERATED BY CADADDY FROM THE CONTRACT DOCUMENTS (NO UNDERGROUND PLAN IS ISSUED). IT IS NOT AN ENGINEER'S DRAWING: THE ENGINEER OF RECORD REVIEWS IT BEFORE USE. SOLID RUNS ARE CALLED BY THE DOCUMENTS.";

/// A buried line of another trade: a pipe-kind run below the floor or a pipe service (water,
/// sewer, storm, gas), for coordination and the crossing check.
#[derive(Clone, Debug)]
struct Other {
    id: String,
    label: String,
    level: String,
    points: Vec<[f64; 2]>,
    size_in: f64,
    /// Below the floor or grade to its top (inches), when the drawings say.
    depth_in: Option<f64>,
}

/// Where a buried electrical run crosses (or comes within the clearance of) another trade's line.
#[derive(Clone, Debug)]
struct Crossing {
    id: String,
    level: String,
    at: [f64; 2],
    electrical: String,
    other: String,
    other_label: String,
    /// `crosses` or `within clearance`.
    how: &'static str,
    /// Plan distance between the two (0 where they cross), feet.
    distance_ft: f64,
    /// Clear vertical distance between them (inches), when both depths are known.
    separation_in: Option<f64>,
}

/// Where a buried run comes up.
#[derive(Clone, Debug)]
struct Stub {
    tag: String,
    level: String,
    at: [f64; 2],
    feeds: String,
    run: String,
    size_in: f64,
    conduits: u32,
    material: String,
    height_in: f64,
    /// The conduits coming up, by size: (count, inches), largest first.
    mix: Vec<(u32, f64)>,
    /// The grid line or edge each dimension is taken from, and the distance (feet, signed).
    x: Option<(String, f64, f64)>,
    y: Option<(String, f64, f64)>,
}

#[derive(Clone, Debug)]
struct Finding {
    check: &'static str,
    severity: &'static str,
    about: String,
    level: String,
    at: [f64; 2],
    message: String,
    fix: String,
    /// More fields for the finding's JSON (an object), else null.
    data: Value,
}

impl Finding {
    fn json(&self) -> Value {
        let mut v = json!({ "check": self.check, "severity": self.severity, "about": self.about, "level": self.level, "at": self.at, "message": self.message, "fix": self.fix });
        if let (Some(o), Value::Object(more)) = (v.as_object_mut(), &self.data) {
            o.extend(more.iter().map(|(k, x)| (k.clone(), x.clone())));
        }
        v
    }
}

/// One option of a feeder's route.
#[derive(Clone, Debug)]
struct Option_ {
    length_ft: f64,
    cost: Option<f64>,
}

#[derive(Clone, Debug)]
struct Route {
    feeder: String,
    from: String,
    to: String,
    level: String,
    under: Option_,
    over: Option_,
    basis: String,
    suggest: &'static str,
}

/// Grid lines: vertical ones (an x each) and horizontal ones (a y each), by label.
#[derive(Clone, Debug, Default)]
pub(super) struct Grids {
    pub(super) x: Vec<(String, f64)>,
    pub(super) y: Vec<(String, f64)>,
}

struct Ug {
    runs: Vec<Buried>,
    stubs: Vec<Stub>,
    services: Vec<Value>,
    /// Pipe services (water, sewer, storm, gas) and pipe runs below the floor, shown for
    /// coordination only.
    other_trade: Vec<Other>,
    crossings: Vec<Crossing>,
    routes: Vec<Route>,
    holds: Vec<super::concrete::Hold>,
    findings: Vec<Finding>,
    grids: Grids,
}

fn upf(sp: &BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

fn finite(p: [f64; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

fn centre(e: &MepEquipment) -> [f64; 2] {
    [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0]
}

/// The distance from a point to an equipment box (0 inside), sheet units.
fn to_box(p: [f64; 2], e: &MepEquipment) -> f64 {
    let dx = (e.min[0] - p[0]).max(0.0).max(p[0] - e.max[0]);
    let dy = (e.min[1] - p[1]).max(0.0).max(p[1] - e.max[1]);
    dx.hypot(dy)
}

/// The equipment a point on a level is at (within [`FEEDS_WITHIN_FT`]).
fn equipment_at<'a>(sp: &'a BuildingSpec, level: &str, p: [f64; 2]) -> Option<&'a MepEquipment> {
    let reach = FEEDS_WITHIN_FT * upf(sp);
    sp.mep
        .equipment
        .iter()
        .take(MAX_ITEMS)
        .filter(|e| level.is_empty() || e.level.is_empty() || e.level.eq_ignore_ascii_case(level))
        .map(|e| (e, to_box(p, e)))
        .filter(|(_, d)| d.is_finite() && *d <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(e, _)| e)
}

fn length(points: &[[f64; 2]]) -> f64 {
    points.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).filter(|l| l.is_finite()).sum()
}

/// The turns along a polyline (degrees): the angle between each segment and the next, segments
/// of no length skipped.
fn turns(points: &[[f64; 2]]) -> f64 {
    let dirs: Vec<[f64; 2]> = points
        .windows(2)
        .filter_map(|w| {
            let (dx, dy) = (w[1][0] - w[0][0], w[1][1] - w[0][1]);
            let l = dx.hypot(dy);
            (l.is_finite() && l > 1e-9).then(|| [dx / l, dy / l])
        })
        .collect();
    dirs.windows(2).map(|w| (w[0][0] * w[1][0] + w[0][1] * w[1][1]).clamp(-1.0, 1.0).acos().to_degrees()).filter(|a| a.is_finite()).sum()
}

fn cross2(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// Where segments `a0-a1` and `b0-b1` meet (touching ends included), if they do.
fn seg_hit(a0: [f64; 2], a1: [f64; 2], b0: [f64; 2], b1: [f64; 2]) -> Option<[f64; 2]> {
    let r = [a1[0] - a0[0], a1[1] - a0[1]];
    let s = [b1[0] - b0[0], b1[1] - b0[1]];
    let den = cross2(r, s);
    if den.abs() < 1e-12 {
        return None;
    }
    let q = [b0[0] - a0[0], b0[1] - a0[1]];
    let t = cross2(q, s) / den;
    let u = cross2(q, r) / den;
    let ok = |v: f64| v.is_finite() && (-1e-9..=1.0 + 1e-9).contains(&v);
    (ok(t) && ok(u)).then(|| [a0[0] + t * r[0], a0[1] + t * r[1]])
}

/// The point of segment `a-b` nearest `p`.
fn nearest_on(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 1e-18 { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    [a[0] + t * d[0], a[1] + t * d[1]]
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// The least distance between two segments that don't cross, and the point on the first
/// nearest the second.
fn seg_gap(a0: [f64; 2], a1: [f64; 2], b0: [f64; 2], b1: [f64; 2]) -> (f64, [f64; 2]) {
    let cands = [(b0, nearest_on(b0, a0, a1)), (b1, nearest_on(b1, a0, a1)), (nearest_on(a0, b0, b1), a0), (nearest_on(a1, b0, b1), a1)];
    cands.into_iter().map(|(other, on_a)| (dist(other, on_a), on_a)).min_by(|x, y| x.0.total_cmp(&y.0)).unwrap_or((f64::INFINITY, a0))
}

/// Clear vertical distance between two buried things given their tops (inches below the floor
/// or grade) and sizes; 0 when they overlap.
fn separation(top_a: f64, size_a: f64, top_b: f64, size_b: f64) -> f64 {
    let (bot_a, bot_b) = (top_a + size_a.max(0.0), top_b + size_b.max(0.0));
    if bot_a <= top_b {
        top_b - bot_a
    } else if bot_b <= top_a {
        top_a - bot_b
    } else {
        0.0
    }
}

/// Every place a buried electrical run meets another trade's line on its level: each crossing
/// point, else (when they don't cross) the nearest approach within `clearance` (sheet units).
fn crossings(runs: &[Buried], others: &[Other], clearance: f64, u: f64, findings: &mut Vec<Finding>) -> Vec<Crossing> {
    let mut out: Vec<Crossing> = Vec::new();
    let mut tests = 0usize;
    let same_level = |a: &str, b: &str| a.is_empty() || b.is_empty() || a.eq_ignore_ascii_case(b);
    'all: for r in runs {
        for o in others.iter().filter(|o| same_level(&r.level, &o.level)) {
            let mut hits: Vec<[f64; 2]> = Vec::new();
            let mut near: Option<(f64, [f64; 2])> = None;
            for a in r.points.windows(2) {
                for b in o.points.windows(2) {
                    tests += 1;
                    if tests > MAX_PAIR_TESTS {
                        findings.push(Finding {
                            check: "crossing_incomplete",
                            severity: "warning",
                            about: "crossings".into(),
                            level: r.level.clone(),
                            at: r.points.first().copied().unwrap_or([0.0, 0.0]),
                            message: "too many segments to check every crossing; the list stops here".into(),
                            fix: "check the crossings level by level, or simplify the routes".into(),
                            data: Value::Null,
                        });
                        break 'all;
                    }
                    if let Some(p) = seg_hit(a[0], a[1], b[0], b[1]) {
                        // One crossing where a route passes through a shared vertex.
                        if !hits.iter().any(|h| dist(*h, p) <= 0.5 * u) {
                            hits.push(p);
                        }
                    } else {
                        let (d, p) = seg_gap(a[0], a[1], b[0], b[1]);
                        if d.is_finite() && near.is_none_or(|(nd, _)| d < nd) {
                            near = Some((d, p));
                        }
                    }
                }
            }
            let sep = match (r.depth_in.is_finite() && r.depth_in > 0.0, o.depth_in) {
                (true, Some(od)) => Some(separation(r.depth_in, r.size_in, od, o.size_in)),
                _ => None,
            };
            let found: Vec<([f64; 2], &'static str, f64)> = if hits.is_empty() {
                near.filter(|(d, _)| *d <= clearance).map(|(d, p)| (p, "within clearance", d / u)).into_iter().collect()
            } else {
                hits.into_iter().map(|p| (p, "crosses", 0.0)).collect()
            };
            for (at, how, distance_ft) in found {
                if out.len() >= MAX_ITEMS {
                    break 'all;
                }
                out.push(Crossing {
                    id: format!("X-{}", out.len() + 1),
                    level: if r.level.is_empty() { o.level.clone() } else { r.level.clone() },
                    at,
                    electrical: r.id.clone(),
                    other: o.id.clone(),
                    other_label: o.label.clone(),
                    how,
                    distance_ft,
                    separation_in: sep,
                });
            }
        }
    }
    for x in &out {
        let low = x.separation_in.is_some_and(|s| s < MIN_SEPARATION_IN);
        let sep_text =
            x.separation_in.map_or("vertical separation unknown (a depth isn't given)".to_string(), |s| format!("{}\" clear vertically", r2(s)));
        findings.push(Finding {
            check: "crossing",
            severity: if low { "error" } else { "warning" },
            about: x.id.clone(),
            level: x.level.clone(),
            at: x.at,
            message: format!("{} {} {} ({}): {sep_text}", x.electrical, x.how, x.other, x.other_label),
            fix: if low {
                format!("keep at least {MIN_SEPARATION_IN}\" between them: lower one, or sleeve or encase the crossing")
            } else {
                "pothole and confirm both depths before trenching; mark the crossing in the field".into()
            },
            data: json!({ "items": [x.electrical, x.other], "how": x.how, "distance_ft": r2(x.distance_ft), "vertical_separation_in": x.separation_in.map(r2) }),
        });
    }
    out
}

/// The conduit material a buried system is: from its name or layer, else PVC Schedule 40.
fn material_of(name: &str) -> String {
    let up = name.to_ascii_uppercase();
    if up.contains("RGS") || up.contains("RMC") {
        "RGS".into()
    } else if up.contains("80") {
        "PVC Sch 80".into()
    } else {
        "PVC Sch 40".into()
    }
}

/// The required kinds' names, and the words a service's kind may be given in.
fn service_words(kind: &str) -> &'static [&'static str] {
    match kind.to_ascii_lowercase().as_str() {
        "power" | "electric" | "electrical" => &["power", "electric", "electrical", "service"],
        "telephone" | "telecom" | "tel" | "phone" => &["telephone", "telecom", "tel", "phone", "communications"],
        "catv" | "cable" | "cable_tv" => &["catv", "cable", "cable_tv", "tv"],
        "fire_alarm" | "fa" | "fire" => &["fire_alarm", "fa", "fire", "fire alarm"],
        "gas_bond" | "gas" => &["gas_bond", "gas", "gas bond"],
        _ => &[],
    }
}

fn is_kind(s: &Service, kind: &str) -> bool {
    let k = s.kind.trim().to_ascii_lowercase();
    k == kind.to_ascii_lowercase() || service_words(kind).contains(&k.as_str())
}

/// Pipe services (water, sewer, storm, gas) are other-trade utilities: they are NOT electrical
/// conduit or duct-bank services and must not be drawn as conduit runs or given PVC material.
fn is_pipe_service(kind: &str) -> bool {
    matches!(kind.to_ascii_lowercase().as_str(), "water" | "sewer" | "storm" | "gas")
}

/// Grid lines from the spec's columns (`A-1` at a point): a label whose columns line up on one x
/// is a vertical line, on one y a horizontal one.
pub(super) fn grids(sp: &BuildingSpec) -> Grids {
    let stray = GRID_STRAY_FT * upf(sp);
    let mut by: BTreeMap<String, Vec<[f64; 2]>> = BTreeMap::new();
    for c in sp.columns.iter().take(MAX_ITEMS).filter(|c| finite(c.at)) {
        let mut parts = c.grid.split(['-', '/']).map(str::trim).filter(|p| !p.is_empty());
        for p in [parts.next(), parts.next()].into_iter().flatten() {
            by.entry(p.to_string()).or_default().push(c.at);
        }
    }
    let mut g = Grids::default();
    for (label, pts) in by {
        if pts.len() < 2 {
            continue;
        }
        let (lo_x, hi_x) = pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p[0]), b.max(p[0])));
        let (lo_y, hi_y) = pts.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| (a.min(p[1]), b.max(p[1])));
        let n = pts.len() as f64;
        if hi_x - lo_x <= stray && hi_y - lo_y > stray {
            g.x.push((label, pts.iter().map(|p| p[0]).sum::<f64>() / n));
        } else if hi_y - lo_y <= stray && hi_x - lo_x > stray {
            g.y.push((label, pts.iter().map(|p| p[1]).sum::<f64>() / n));
        }
    }
    g.x.sort_by(|a, b| a.1.total_cmp(&b.1));
    g.y.sort_by(|a, b| a.1.total_cmp(&b.1));
    g
}

/// The nearest line to `v` and the distance from it (feet, signed); else the level's edge.
fn dim_from(lines: &[(String, f64)], v: f64, edge: Option<(&str, f64)>, upf: f64) -> Option<(String, f64, f64)> {
    let near = lines.iter().min_by(|a, b| (a.1 - v).abs().total_cmp(&(b.1 - v).abs()));
    match near {
        Some((label, at)) => Some((format!("grid {label}"), *at, (v - at) / upf)),
        None => edge.map(|(name, at)| (name.to_string(), at, (v - at) / upf)),
    }
}

/// The level's edges (its region, else the extent of its equipment and runs): west and south.
fn edges(sp: &BuildingSpec, level: &str, pts: &[[f64; 2]]) -> Option<[f64; 2]> {
    if let Some(r) = sp.level(level).and_then(|l| l.region).filter(|r| r.iter().all(|v| v.is_finite())) {
        return Some([r[0].min(r[2]), r[1].min(r[3])]);
    }
    let all: Vec<[f64; 2]> = sp
        .mep
        .equipment
        .iter()
        .filter(|e| e.level.eq_ignore_ascii_case(level))
        .flat_map(|e| [e.min, e.max])
        .chain(pts.iter().copied())
        .filter(|p| finite(*p))
        .collect();
    (!all.is_empty()).then(|| [all.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min), all.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min)])
}

/// The slab level: the lowest one (the first, when none has an elevation).
fn slab_level(sp: &BuildingSpec) -> Option<&str> {
    sp.levels.iter().filter(|l| l.elevation_ft.is_finite()).min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)).map(|l| l.name.as_str())
}

// ---------------- prices ----------------

/// A unit price from the estimator's baseline: material and labor hours per unit (item and
/// size, else the item with no size).
fn price(sp: &BuildingSpec, item: &str, size: &str) -> Option<(f64, f64)> {
    let prices = sp.estimate.get("baseline").and_then(|b| b.get("prices")).and_then(Value::as_array)?;
    let matches = |p: &&Value, sized: bool| {
        p["item"].as_str().is_some_and(|i| i.eq_ignore_ascii_case(item))
            && if sized {
                p["size"].as_str().is_some_and(|s| s.eq_ignore_ascii_case(size))
            } else {
                p["size"].as_str().unwrap_or_default().is_empty()
            }
    };
    let p = prices.iter().find(|p| matches(p, true)).or_else(|| prices.iter().find(|p| matches(p, false)))?;
    let material = p["material"].as_f64().filter(|v| v.is_finite() && *v >= 0.0)?;
    let labor = p["labor_hours"].as_f64().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0);
    Some((material, labor))
}

fn crew_rate(sp: &BuildingSpec) -> f64 {
    sp.estimate.get("settings").and_then(|s| s.get("crew_rate")).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0)
}

/// A trade size as the estimator writes it (`2"`, `1-1/4"`).
fn trade_size(inches: f64) -> String {
    let whole = inches.floor();
    let frac = inches - whole;
    let f = [(0.25, "1/4"), (0.5, "1/2"), (0.75, "3/4")].into_iter().find(|(v, _)| (frac - v).abs() < 0.01).map(|(_, t)| t);
    match (whole as i64, f) {
        (0, Some(t)) => format!("{t}\""),
        (w, Some(t)) => format!("{w}-{t}\""),
        (w, None) => format!("{w}\""),
    }
}

/// A conductor size as the estimator writes it (`#4/0`, `500 kcmil`).
fn wire_size(size: &str) -> String {
    let t = size.trim();
    if t.is_empty() || t.starts_with('#') || t.to_ascii_lowercase().contains("kcmil") || t.to_ascii_lowercase().contains("mcm") {
        t.to_string()
    } else {
        format!("#{t}")
    }
}

/// The cost of one route for a feeder (`length` feet): conduits of `item`, its conductors.
fn route_cost(sp: &BuildingSpec, f: &cadaddy_walk3d::Feeder, conduit_item: &str, length: f64, missing: &mut Vec<String>) -> Option<f64> {
    let crew = crew_rate(sp);
    let size = trade_size(f.conduit_in);
    let wire_item = if f.material.trim().eq_ignore_ascii_case("al") { "XHHW aluminum" } else { "THHN copper" };
    let sets = f64::from(f.sets.max(1));
    let phase = wire_size(&f.size);
    let neutral = wire_size(if f.neutral_size.is_empty() { &f.size } else { &f.neutral_size });
    let ground = wire_size(&f.ground_size);
    let mut cost = 0.0;
    let mut ok = true;
    let mut add = |item: &str, size: &str, qty: f64| match price(sp, item, size) {
        Some((m, h)) => cost += qty * (m + h * crew),
        None => {
            if qty > 0.0 {
                ok = false;
                let what = format!("{item} {size}").trim().to_string();
                if !missing.contains(&what) {
                    missing.push(what);
                }
            }
        }
    };
    add(conduit_item, &size, length * f64::from(f.conduits.max(1)));
    add(wire_item, &phase, length * sets * f64::from(f.phases));
    add(wire_item, &neutral, length * sets * f64::from(f.neutrals));
    add(wire_item, &ground, length * sets * f64::from(f.grounds));
    ok.then_some(cost)
}

// ---------------- the check ----------------

fn study(sp: &BuildingSpec, types: &cadaddy_doc::bimspec::BimTypes, today: Option<i64>, clearance_ft: f64) -> Ug {
    let u = upf(sp);
    let mut runs: Vec<Buried> = Vec::new();
    let mut findings: Vec<Finding> = Vec::new();
    let mut other_trade: Vec<Other> = Vec::new();
    // Conduit runs below the floor (and pipe runs below it, another trade's, for crossings).
    for (i, r) in sp.mep.runs.iter().enumerate().take(MAX_ITEMS) {
        let Some(sys) = sp.mep.systems.iter().find(|s| s.name.eq_ignore_ascii_case(&r.system)) else { continue };
        if !matches!(sys.kind, MepKind::Conduit | MepKind::Pipe) {
            continue;
        }
        let bottom = r.bottoms_ft.iter().copied().filter(|b| b.is_finite()).fold(f64::INFINITY, f64::min);
        let bottom = if bottom.is_finite() { bottom } else { sys.bottom_ft };
        if !(bottom.is_finite() && bottom < 0.0) {
            continue;
        }
        let points: Vec<[f64; 2]> = r.points.iter().copied().filter(|p| finite(*p)).take(MAX_POINTS).collect();
        if points.len() < 2 {
            continue;
        }
        let size = [r.size_in[0], sys.size_in[0]].into_iter().find(|v| v.is_finite() && *v > 0.0).unwrap_or(0.0);
        if sys.kind == MepKind::Pipe {
            let id = if r.id.trim().is_empty() { format!("P-{}", i + 1) } else { r.id.clone() };
            let depth = (-bottom * 12.0 - size).max(0.0);
            let label = format!("{}\" {} @ {}\" BFF", r2(size), sys.name.to_ascii_uppercase(), r2(depth));
            other_trade.push(Other { id, label, level: r.level.clone(), points, size_in: size, depth_in: Some(depth) });
            continue;
        }
        let id = if r.id.trim().is_empty() { format!("UG-{}", i + 1) } else { r.id.clone() };
        let end = |p: Option<&[f64; 2]>| p.and_then(|p| equipment_at(sp, &r.level, *p)).map(|e| e.tag.clone()).unwrap_or_default();
        runs.push(Buried {
            kind: "run",
            system: r.system.clone(),
            level: r.level.clone(),
            length_ft: length(&points) / u,
            from: end(points.first()),
            to: end(points.last()),
            points,
            size_in: size,
            conduits: 1,
            material: material_of(&format!("{} {}", sys.name, sys.layer)),
            // Below the finished floor to the conduit's top.
            depth_in: (-bottom * 12.0 - size).max(0.0),
            id,
            bends_deg: 0.0,
            encased: r.encased,
            proposed: r.height_source.eq_ignore_ascii_case("proposed"),
            basis: [r.source_sheet.as_str(), r.height_source.as_str()].into_iter().filter(|v| !v.trim().is_empty()).collect::<Vec<_>>().join("; "),
        });
    }
    // Service entrances.
    let mut services: Vec<Value> = Vec::new();
    let required: Vec<String> =
        if sp.mep.required_services.is_empty() { vec!["power".into(), "telephone".into()] } else { sp.mep.required_services.clone() };
    for svc in sp.mep.services.iter().take(MAX_ITEMS) {
        let status = match (svc.shown, svc.at) {
            (true, Some(_)) => "shown",
            (false, Some(_)) => "unclear",
            _ => "no location",
        };
        services.push(
            json!({ "kind": svc.kind, "tag": svc.tag, "status": status, "to": svc.to, "at": svc.at, "level": svc.level, "encased": svc.encased }),
        );
        let (Some(at), true) = (svc.at, svc.shown) else { continue };
        // The route as drawn, else straight from where it starts.
        let points: Vec<[f64; 2]> = if svc.route.len() >= 2 {
            svc.route.iter().take(MAX_POINTS).copied().filter(|p| finite(*p)).collect()
        } else {
            svc.from.into_iter().chain([at]).filter(|p| finite(*p)).collect()
        };
        let id = if svc.tag.trim().is_empty() { format!("SVC-{}", svc.kind.to_ascii_uppercase()) } else { svc.tag.clone() };
        // Pipe services (water, sewer, storm, gas) are other-trade utilities for coordination:
        // they don't become electrical conduit runs or stub-ups.
        if is_pipe_service(&svc.kind) {
            let mut label = svc.kind.to_ascii_uppercase();
            if let Some(size) = svc.size_in {
                label = format!("{size}\" {label}");
            }
            if let Some(dep) = svc.depth_ft {
                label = format!("{label} @ {dep}' DEEP");
            } else if svc.cover_in.is_finite() && svc.cover_in > 0.0 {
                label = format!("{label} @ {}' DEEP", svc.cover_in / 12.0);
            }
            if let Some(inv) = svc.invert_ft {
                label = format!("{label} INV {inv}");
            }
            let depth_in =
                svc.depth_ft.map(|d| d * 12.0).or_else(|| (svc.cover_in > 0.0).then_some(svc.cover_in)).filter(|d| d.is_finite() && *d >= 0.0);
            let size_in = svc.size_in.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(0.0);
            other_trade.push(Other { id, label, level: svc.level.clone(), points, size_in, depth_in });
            continue;
        }
        runs.push(Buried {
            kind: "service",
            system: svc.kind.clone(),
            level: svc.level.clone(),
            length_ft: length(&points) / u,
            from: if svc.from.is_some() { "site".into() } else { String::new() },
            to: if svc.to.is_empty() { equipment_at(sp, &svc.level, at).map(|e| e.tag.clone()).unwrap_or_default() } else { svc.to.clone() },
            points,
            size_in: svc.conduit_in,
            conduits: svc.conduits.max(1),
            material: if svc.material.trim().is_empty() { "PVC Sch 40".into() } else { svc.material.clone() },
            depth_in: svc.cover_in,
            id,
            bends_deg: 0.0,
            encased: svc.encased,
            proposed: false,
            basis: format!("{} service", svc.kind),
        });
        if !(svc.conduit_in.is_finite() && svc.conduit_in > 0.0) {
            findings.push(Finding {
                check: "service_size",
                severity: "warning",
                about: id_of(svc),
                level: svc.level.clone(),
                at,
                message: format!("the {} service's conduit size isn't given", svc.kind),
                fix: "give the conduit count and size (the utility's requirements)".into(),
                data: Value::Null,
            });
        }
    }
    for kind in &required {
        let found: Vec<&Service> = sp.mep.services.iter().filter(|s| is_kind(s, kind)).collect();
        let at = found.iter().find_map(|s| s.at).unwrap_or([0.0, 0.0]);
        let level = found.first().map(|s| s.level.clone()).or_else(|| slab_level(sp).map(str::to_string)).unwrap_or_default();
        if found.is_empty() {
            findings.push(Finding {
                check: "service_missing",
                severity: "error",
                about: format!("{kind} service"),
                level,
                at,
                message: format!("the building needs a {kind} service entrance and the drawings show none"),
                fix: "show the entrance: conduits, size, route from the property line or the utility's structure, and where it comes up".into(),
                data: Value::Null,
            });
        } else if !found.iter().any(|s| s.shown && s.at.is_some()) {
            findings.push(Finding {
                check: "service_unclear",
                severity: "error",
                about: found.first().map(|s| id_of(s)).unwrap_or_else(|| format!("{kind} service")),
                level,
                at,
                message: format!("the {kind} service entrance is named but not laid out: the field can't place it from the drawings before the slab"),
                fix: "draw its route and where it comes up, with the conduit count and size".into(),
                data: Value::Null,
            });
        }
    }
    // Bends: the route's turns and a 90 at each end that comes up (a service comes up inside).
    for r in runs.iter_mut() {
        let ends = if r.kind == "service" { 1.0 } else { [&r.from, &r.to].into_iter().filter(|t| !t.is_empty()).count() as f64 };
        r.bends_deg = (turns(&r.points) + 90.0 * ends).round();
        if r.bends_deg > MAX_BENDS_DEG {
            findings.push(Finding {
                check: "bends",
                severity: "warning",
                about: r.id.clone(),
                level: r.level.clone(),
                at: r.points.first().copied().unwrap_or([0.0, 0.0]),
                message: format!("{} has {}° of bends between pull points; more than {MAX_BENDS_DEG}° can't be pulled", r.id, r.bends_deg),
                fix: "add a pull box or handhole, or straighten the route".into(),
                data: json!({ "bends_deg": r.bends_deg }),
            });
        }
    }
    // Stub-ups: a buried run's ends at equipment, and where a service comes up.
    let g = grids(sp);
    let mut stubs: Vec<Stub> = Vec::new();
    for r in runs.iter().take(MAX_ITEMS) {
        let ends: Vec<([f64; 2], String)> = if r.kind == "service" {
            r.points.last().map(|p| (*p, r.to.clone())).into_iter().collect()
        } else {
            [(r.points.first(), &r.from), (r.points.last(), &r.to)]
                .into_iter()
                .filter_map(|(p, t)| p.map(|p| (*p, t.clone())))
                .filter(|(_, t)| !t.is_empty())
                .collect()
        };
        for (at, feeds) in ends {
            let edge = edges(sp, &r.level, &r.points);
            let x = dim_from(&g.x, at[0], edge.map(|e| ("west edge", e[0])), u);
            let y = dim_from(&g.y, at[1], edge.map(|e| ("south edge", e[1])), u);
            let height_in = sp
                .mep
                .equipment
                .iter()
                .find(|e| e.tag.eq_ignore_ascii_case(&feeds))
                .map_or(6.0, |e| if e.bottom_ft.is_finite() && e.bottom_ft > 0.0 { e.bottom_ft * 12.0 + 2.0 } else { 6.0 });
            stubs.push(Stub {
                tag: format!("SU-{}", stubs.len() + 1),
                level: r.level.clone(),
                at,
                feeds,
                run: r.id.clone(),
                size_in: r.size_in,
                conduits: r.conduits,
                // Elbows and stub-ups through the slab in Schedule 80 (or the run's RGS).
                material: if r.material == "RGS" { "RGS".into() } else { "PVC Sch 80".into() },
                height_in,
                mix: vec![(r.conduits, r.size_in)],
                x,
                y,
            });
        }
    }
    // Runs coming up at the same place into the same equipment are one stub-up group (their
    // conduits side by side), listed once.
    let near = 0.5 * u;
    let mut merged: Vec<Stub> = Vec::new();
    for st in stubs {
        match merged.iter_mut().find(|m| {
            m.level.eq_ignore_ascii_case(&st.level)
                && m.feeds.eq_ignore_ascii_case(&st.feeds)
                && (m.at[0] - st.at[0]).hypot(m.at[1] - st.at[1]) <= near
        }) {
            Some(m) => {
                m.conduits = m.conduits.saturating_add(st.conduits);
                m.run = format!("{}, {}", m.run, st.run);
                m.size_in = m.size_in.max(st.size_in);
                for (n, size) in st.mix {
                    match m.mix.iter_mut().find(|(_, s)| (*s - size).abs() < 1e-6) {
                        Some(e) => e.0 = e.0.saturating_add(n),
                        None => m.mix.push((n, size)),
                    }
                }
                m.mix.sort_by(|a, b| b.1.total_cmp(&a.1));
                if st.material == "RGS" {
                    m.material = "RGS".into();
                }
            }
            None => merged.push(st),
        }
    }
    let mut stubs = merged;
    for (i, st) in stubs.iter_mut().enumerate() {
        st.tag = format!("SU-{}", i + 1);
    }
    for st in &stubs {
        if st.x.is_none() || st.y.is_none() {
            findings.push(Finding {
                check: "stub_undimensioned",
                severity: "warning",
                about: st.tag.clone(),
                level: st.level.clone(),
                at: st.at,
                message: format!("{} has no grid line or wall to be dimensioned from", st.tag),
                fix: "add the structural grid (columns with grid labels) or the level's region".into(),
                data: Value::Null,
            });
        }
    }
    // Route review: feeders between gear on the slab level that no run routes yet.
    let mut routes = Vec::new();
    let slab = slab_level(sp).unwrap_or_default().to_string();
    let routed = |tag: &str| sp.mep.runs.iter().any(|r| r.id.eq_ignore_ascii_case(tag) || r.objects.iter().any(|o| o.eq_ignore_ascii_case(tag)));
    for f in sp.mep.feeders.iter().take(MAX_ITEMS) {
        let gear = |t: &str| sp.mep.equipment.iter().find(|e| !t.is_empty() && e.tag.eq_ignore_ascii_case(t));
        let (Some(a), Some(b)) = (gear(&f.from), gear(&f.to)) else { continue };
        let on_slab = |e: &MepEquipment| e.level.is_empty() || e.level.eq_ignore_ascii_case(&slab);
        if !(on_slab(a) && on_slab(b)) || routed(&f.tag) {
            continue;
        }
        let (ca, cb) = (centre(a), centre(b));
        let run = ((ca[0] - cb[0]).abs() + (ca[1] - cb[1]).abs()) / u;
        if !run.is_finite() {
            continue;
        }
        let ceiling = sp.level(&slab).map(|l| l.ceiling_ft).filter(|c| c.is_finite() && *c > 0.0).unwrap_or(9.0);
        let under_len = run + 2.0 * (UNDER_SLAB_DEPTH_FT + STUB_RISE_FT);
        let over_len = run + 2.0 * ceiling;
        let mut missing = Vec::new();
        // Trenching and backfill along the run, when the estimator prices it (per foot).
        let trench = price(sp, "Trenching", "").map(|(m, h)| run * (m + h * crew_rate(sp)));
        let under_cost = route_cost(sp, f, "PVC conduit", under_len, &mut missing).map(|c| c + trench.unwrap_or(0.0));
        let under = Option_ { length_ft: under_len, cost: under_cost };
        let over = Option_ { length_ft: over_len, cost: route_cost(sp, f, "EMT conduit", over_len, &mut missing) };
        let priced = if trench.is_some() { "estimator prices and crew rate, with trenching" } else { "estimator prices and crew rate" };
        let (suggest, basis) = match (under.cost, over.cost) {
            (Some(uc), Some(oc)) => (if uc < oc * CHEAPER { "under_slab" } else { "overhead" }, priced.to_string()),
            _ => (
                if under_len < over_len * CHEAPER { "under_slab" } else { "overhead" },
                format!("length only: no estimator price for {}", missing.join(", ")),
            ),
        };
        routes.push(Route { feeder: f.tag.clone(), from: f.from.clone(), to: f.to.clone(), level: slab.clone(), under, over, basis, suggest });
    }
    // The slab's hold points.
    let els = super::concrete::elements(sp, types);
    let holds = super::concrete::holds(sp, &els, today, &super::concrete::Rules::of(sp));
    let crossings = crossings(&runs, &other_trade, clearance_ft * u, u, &mut findings);
    findings.truncate(MAX_ITEMS);
    Ug { runs, stubs, services, other_trade, crossings, routes, holds, findings, grids: g }
}

fn id_of(s: &Service) -> String {
    if s.tag.trim().is_empty() { format!("{} service", s.kind) } else { s.tag.clone() }
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Feet and inches (`12'-6"`), signs kept.
fn ft_in(ft: f64) -> String {
    if !ft.is_finite() {
        return "?".into();
    }
    let sign = if ft < 0.0 { "-" } else { "" };
    let total = (ft.abs() * 12.0).round() as i64;
    format!("{sign}{}'-{}\"", total / 12, total % 12)
}

fn ug_json(ug: &Ug) -> Value {
    let errors = ug.findings.iter().filter(|f| f.severity == "error").count();
    let dim = |d: &Option<(String, f64, f64)>| d.as_ref().map(|(from, _, ft)| json!({ "from": from, "ft": r2(*ft) }));
    // Coordinates for layout tools: from the first grid lines (else the sheet's origin), feet.
    let ox = ug.grids.x.first().map_or(0.0, |g| g.1);
    let oy = ug.grids.y.first().map_or(0.0, |g| g.1);
    json!({
        "ok": errors == 0,
        "errors": errors,
        "warnings": ug.findings.len() - errors,
        "runs": ug.runs.iter().map(|r| json!({
            "id": r.id, "kind": r.kind, "system": r.system, "level": r.level, "from": r.from, "to": r.to, "size_in": r.size_in,
            "conduits": r.conduits, "material": r.material, "depth_in": r2(r.depth_in), "length_ft": r2(r.length_ft),
            "bends_deg": r.bends_deg, "encased": r.encased, "proposed": r.proposed, "basis": r.basis,
        })).collect::<Vec<_>>(),
        "stubups": ug.stubs.iter().map(|s| json!({
            "tag": s.tag, "level": s.level, "at": s.at, "feeds": s.feeds, "run": s.run, "size_in": s.size_in, "conduits": s.conduits,
            "mix": s.mix.iter().map(|(n, size)| json!({ "count": n, "size_in": size })).collect::<Vec<_>>(), "material": s.material, "height_in": r2(s.height_in), "x": dim(&s.x), "y": dim(&s.y), "coords_ft": [r2(s.at[0] - ox), r2(s.at[1] - oy)],
        })).collect::<Vec<_>>(),
        "services": ug.services,
        "other_trade": ug.other_trade.iter().map(|o| json!({ "id": o.id, "label": o.label, "level": o.level, "depth_in": o.depth_in.map(r2) })).collect::<Vec<_>>(),
        "crossings": ug.crossings.iter().map(|x| json!({
            "id": x.id, "level": x.level, "at": x.at, "electrical": x.electrical, "other": x.other, "how": x.how,
            "distance_ft": r2(x.distance_ft), "vertical_separation_in": x.separation_in.map(r2),
        })).collect::<Vec<_>>(),
        "routes": ug.routes.iter().map(|r| json!({
            "feeder": r.feeder, "from": r.from, "to": r.to, "level": r.level,
            "under_slab": { "length_ft": r2(r.under.length_ft), "cost": r.under.cost.map(r2) },
            "overhead": { "length_ft": r2(r.over.length_ft), "cost": r.over.cost.map(r2) },
            "basis": r.basis, "suggest": r.suggest,
        })).collect::<Vec<_>>(),
        "holds": ug.holds.iter().map(|h| json!({ "pour": h.pour, "item": h.key, "element": h.element, "level": h.level, "kind": h.kind, "size": h.size, "count": h.count, "status": h.status })).collect::<Vec<_>>(),
        "findings": ug.findings.iter().map(Finding::json).collect::<Vec<_>>(),
    })
}

fn spec_of(cmd: &str, s: &Session) -> Result<BuildingSpec> {
    let text = s.doc()?.building_spec.clone().ok_or_else(|| bad(cmd, "the drawing has no building spec (bspec.set)"))?;
    BuildingSpec::from_json(&text).map_err(|e| bad(cmd, e))
}

fn today_of(cmd: &str, p: &Value) -> Result<Option<i64>> {
    match str_param(p, "today").map(str::trim).filter(|t| !t.is_empty()) {
        None => Ok(None),
        Some(t) => super::concrete::day(t).map(Some).ok_or_else(|| bad(cmd, "today: a date as YYYY-MM-DD")),
    }
}

/// The plan clearance a crossing is checked to (`clearance_in`, default a foot), feet.
fn clearance_of(cmd: &str, p: &Value) -> Result<f64> {
    match p.get("clearance_in") {
        None | Some(Value::Null) => Ok(CLEARANCE_FT),
        Some(v) => v
            .as_f64()
            .filter(|c| c.is_finite() && (0.0..=1200.0).contains(c))
            .map(|c| c / 12.0)
            .ok_or_else(|| bad(cmd, "clearance_in: inches, 0 to 1200")),
    }
}

fn run_check(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec_of("ugsheets.check", s)?;
    let t = today_of("ugsheets.check", p)?;
    let c = clearance_of("ugsheets.check", p)?;
    Ok(ug_json(&study(&sp, &s.doc()?.bim_types, t, c)))
}

fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", h as u32)
}

fn run_review(s: &mut Session, _p: &Value) -> Result<Value> {
    let mut sp = spec_of("ugsheets.review", s)?;
    let ug = study(&sp, &s.doc()?.bim_types, None, CLEARANCE_FT);
    let mut added = Vec::new();
    for f in ug.findings.iter().filter(|f| f.severity == "error" && f.check.starts_with("service_")) {
        let id = format!("RFI-U{}", hash(&format!("{}|{}", f.about, f.check)));
        if sp.rfis.iter().any(|r| r.id == id) || sp.rfis.len() >= cadaddy_walk3d::rfi::MAX_RFIS {
            continue;
        }
        let kind = cadaddy_walk3d::RfiKind::ServiceEntrance;
        sp.rfis.push(cadaddy_walk3d::Rfi {
            id: id.clone(),
            kind: Some(kind),
            severity: kind.severity(),
            level: f.level.clone(),
            at: f.at,
            about: format!("{} ({})", f.about, f.check),
            issue: f.message.clone(),
            assumed: "Held: the slab isn't poured over this service's route until it is answered.".into(),
            proposed: (!f.fix.is_empty()).then(|| f.fix.clone()),
            ..cadaddy_walk3d::Rfi::default()
        });
        added.push(id);
    }
    if !added.is_empty() {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(
        json!({ "errors": ug.findings.iter().filter(|f| f.severity == "error").count(), "rfis": added, "findings": ug.findings.iter().map(Finding::json).collect::<Vec<_>>() }),
    )
}

// ---------------- the sheets ----------------

/// Paper scales tried, largest first: paper inches per real foot (1/4" = 1'-0" … 1/32").
const SCALES: [(f64, &str); 6] = [
    (0.25, "1/4\" = 1'-0\""),
    (0.125, "1/8\" = 1'-0\""),
    (3.0 / 32.0, "3/32\" = 1'-0\""),
    (1.0 / 16.0, "1/16\" = 1'-0\""),
    (1.0 / 32.0, "1/32\" = 1'-0\""),
    (1.0 / 64.0, "1/64\" = 1'-0\""),
];

/// Where the plan goes on paper: model point → paper point.
struct Frame {
    min: [f64; 2],
    k: f64,
    origin: [f64; 2],
    scale: &'static str,
}

impl Frame {
    fn to(&self, p: [f64; 2]) -> [f64; 2] {
        [self.origin[0] + (p[0] - self.min[0]) * self.k, self.origin[1] + (p[1] - self.min[1]) * self.k]
    }
}

fn frame(pts: &[[f64; 2]], upf: f64) -> Frame {
    let pts: Vec<[f64; 2]> = pts.iter().copied().filter(|p| finite(*p)).collect();
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in &pts {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    if pts.is_empty() {
        lo = [0.0, 0.0];
        hi = [upf, upf];
    }
    let (w, h) = (((hi[0] - lo[0]) / upf).max(1.0), ((hi[1] - lo[1]) / upf).max(1.0));
    let (pw, ph) = (PLAN[2] - PLAN[0], PLAN[3] - PLAN[1]);
    let (per_ft, scale) = SCALES.into_iter().find(|(k, _)| w * k <= pw && h * k <= ph).unwrap_or((pw.min(ph) / w.max(h), "not to scale"));
    let k = per_ft / upf;
    // Centred in the plan's box.
    let origin = [PLAN[0] + (pw - w * per_ft) / 2.0, PLAN[1] + (ph - h * per_ft) / 2.0];
    Frame { min: lo, k, origin, scale }
}

/// Draw through the session's commands into the current layout.
struct Pen<'a> {
    s: &'a mut Session,
    dims: usize,
}

impl Pen<'_> {
    /// Make `name` current, made first when it isn't there (with the linetype when the drawing
    /// has it loaded, else continuous).
    fn layer(&mut self, name: &str, color: i64, linetype: Option<&str>) -> Result<()> {
        if self.s.doc()?.layer(name).is_some() {
            return self.s.execute("layer.current", &json!({ "name": name })).map(|_| ());
        }
        let p = json!({ "name": name, "current": true, "color": color });
        if let Some(lt) = linetype {
            // Loaded from the library first (a layer can't take a linetype the drawing lacks).
            if self.s.doc()?.linetypes.iter().all(|l| !l.name.eq_ignore_ascii_case(lt)) {
                let _ = self.s.execute("linetype", &json!({ "load": lt }));
            }
            let mut with = p.clone();
            with["linetype"] = json!(lt);
            if self.s.execute("layer.new", &with).is_ok() {
                return Ok(());
            }
        }
        self.s.execute("layer.new", &p).map(|_| ())
    }
    fn line(&mut self, a: [f64; 2], b: [f64; 2]) -> Result<()> {
        self.s.execute("line", &json!({ "points": [a, b] })).map(|_| ())
    }
    fn pline(&mut self, pts: &[[f64; 2]], closed: bool) -> Result<()> {
        if pts.len() < 2 {
            return Ok(());
        }
        self.s.execute("pline", &json!({ "vertices": pts, "closed": closed })).map(|_| ())
    }
    fn rect(&mut self, a: [f64; 2], b: [f64; 2]) -> Result<()> {
        self.pline(&[a, [b[0], a[1]], b, [a[0], b[1]]], true)
    }
    fn text(&mut self, at: [f64; 2], t: &str, h: f64) -> Result<()> {
        if t.trim().is_empty() {
            return Ok(());
        }
        self.s.execute("text", &json!({ "at": at, "text": t, "height": h })).map(|_| ())
    }
    fn circle(&mut self, c: [f64; 2], r: f64) -> Result<()> {
        self.s.execute("circle", &json!({ "center": c, "radius": r })).map(|_| ())
    }
    /// A dimension between two paper points, its text the real distance.
    fn dim(&mut self, a: [f64; 2], b: [f64; 2], at: [f64; 2], text: &str) -> Result<()> {
        if (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-6 {
            return Ok(());
        }
        self.s.execute("dimlinear", &json!({ "p1": a, "p2": b, "at": at, "text": text }))?;
        self.dims += 1;
        Ok(())
    }
    /// A typical trench section at `x0` in the section box, for the runs `of`: the datum (grade
    /// or the slab's bottom), the trench, the conduits side by side at the least cover, the
    /// encasement when there is one, the warning tape, the cover dimensioned, and its notes.
    fn section(&mut self, x0: f64, of: &[&Buried], title: &str, datum: &str) -> Result<()> {
        let [_, y0, w, h] = SECTION_BOX;
        let cover = of.iter().map(|r| r.depth_in).filter(|d| d.is_finite() && *d > 0.0).fold(f64::INFINITY, f64::min);
        let n = of.iter().map(|r| r.conduits).max().unwrap_or(1).clamp(1, 12);
        let size = of.iter().map(|r| r.size_in).filter(|v| v.is_finite() && *v > 0.0).fold(0.0, f64::max);
        let material = of.first().map(|r| r.material.clone()).unwrap_or_default();
        let encased = of.iter().any(|r| r.encased);
        // Drawn with a stand-in where the drawings don't say; the notes say so.
        let c = if cover.is_finite() { cover } else { 24.0 };
        let d = if size > 0.0 { size } else { 4.0 };
        const GAP: f64 = 3.0;
        const SIDE: f64 = 6.0;
        const BED: f64 = 6.0;
        let nf = f64::from(n);
        let trench_w = nf * d + (nf - 1.0) * GAP + 2.0 * SIDE;
        let depth = c + d + BED;
        // 1" = 1'-0" (1/12" a real inch), smaller when it wouldn't fit.
        let k = (1.0 / 12.0_f64).min(w * 0.45 / trench_w).min((h - 1.0) / depth);
        let top = y0 + h - 0.5;
        let (tx0, tx1, bot) = (x0 + 0.3, x0 + 0.3 + trench_w * k, top - depth * k);
        self.line([x0, top], [x0 + w * 0.55, top])?;
        self.text([x0, top + 0.06], datum, 0.08)?;
        self.line([tx0, top], [tx0, bot])?;
        self.line([tx1, top], [tx1, bot])?;
        self.line([tx0, bot], [tx1, bot])?;
        let cy = top - (c + d / 2.0) * k;
        for i in 0..n {
            let cx = tx0 + (SIDE + d / 2.0 + f64::from(i) * (d + GAP)) * k;
            self.circle([cx, cy], d / 2.0 * k)?;
        }
        if encased {
            let e = 3.0_f64.min(c);
            self.rect([tx0 + (SIDE - e) * k, top - (c - e) * k], [tx1 - (SIDE - e) * k, top - (c + d + e) * k])?;
        }
        // The warning tape, a foot over the conduits (half the cover when that's less).
        let tape = if c > 18.0 { c - 12.0 } else { c / 2.0 };
        self.line([tx0, top - tape * k], [tx1, top - tape * k])?;
        let cover_text = if cover.is_finite() { ft_in(cover / 12.0) } else { "VERIFY".into() };
        self.dim([tx1 + 0.15, top], [tx1 + 0.15, top - c * k], [tx1 + 0.4, top - c * k / 2.0], &cover_text)?;
        let notes = [
            format!("{n}-{} {material}", if size > 0.0 { trade_size(size) } else { "SIZE ?".into() }),
            if cover.is_finite() { format!("COVER {cover_text} BELOW {datum} TO TOP OF CONDUIT") } else { "COVER NOT GIVEN - VERIFY".into() },
            if encased { "CONCRETE ENCASED, 3\" MIN AROUND CONDUITS".into() } else { "DIRECT BURIED, NOT ENCASED".into() },
            "DETECTABLE WARNING TAPE 12\" ABOVE CONDUITS".into(),
            "SAND BEDDING AND SELECT BACKFILL, COMPACTED".into(),
        ];
        let mut ly = top - 0.1;
        for note in &notes {
            for line in wrap(note, 26) {
                self.text([x0 + w * 0.63, ly], &line, 0.08)?;
                ly -= 0.14;
            }
            ly -= 0.06;
        }
        self.text([x0, y0 + 0.2], title, 0.12)?;
        let scale = if (k - 1.0 / 12.0).abs() < 1e-9 { "SCALE: 1\" = 1'-0\"" } else { "NOT TO SCALE" };
        self.text([x0, y0 + 0.02], scale, 0.08)
    }
    /// Rows of text in columns from `top`, going down; the y below the last row.
    fn table(&mut self, x: f64, top: f64, title: &str, widths: &[f64], rows: &[Vec<String>]) -> Result<f64> {
        const ROW: f64 = 0.22;
        self.text([x, top], title, 0.16)?;
        let mut y = top - 0.3;
        let w: f64 = widths.iter().sum();
        for row in rows.iter().take(60) {
            let mut cx = x;
            for (cell, cw) in row.iter().zip(widths) {
                self.text([cx + 0.04, y - 0.15], cell, 0.09)?;
                cx += cw;
            }
            self.line([x, y - ROW], [x + w, y - ROW])?;
            y -= ROW;
        }
        if rows.len() > 60 {
            self.text([x, y - 0.18], &format!("… and {} more (ugsheets.check lists them all)", rows.len() - 60), 0.09)?;
            y -= 0.25;
        }
        Ok(y - 0.35)
    }
}

/// A block named like a title block (`TITLE`, `TB`, `BORDER` in its name).
fn title_block(s: &Session) -> Option<String> {
    let d = s.doc().ok()?;
    d.blocks
        .keys()
        .find(|n| {
            let up = n.to_ascii_uppercase();
            !n.starts_with('*') && (up.contains("TITLE") || up.starts_with("TB") || up.contains("BORDER"))
        })
        .cloned()
}

fn sheet_number(first: &str, i: usize) -> String {
    if i == 0 {
        return first.to_string();
    }
    // Count on the trailing digits (`E-001` → `E-002`), else add `.n`.
    let digits: String = first.chars().rev().take_while(char::is_ascii_digit).collect::<Vec<_>>().into_iter().rev().collect();
    match digits.parse::<u64>() {
        Ok(n) => {
            let head = first.get(..first.len() - digits.len()).unwrap_or_default();
            format!("{head}{:0width$}", n + i as u64, width = digits.len())
        }
        Err(_) => format!("{first}.{}", i + 1),
    }
}

fn run_build(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "ugsheets.build";
    let sp = spec_of(cmd, s)?;
    let t = today_of(cmd, p)?;
    let c = clearance_of(cmd, p)?;
    let ug = study(&sp, &s.doc()?.bim_types, t, c);
    let u = upf(&sp);
    let only = str_param(p, "level").map(str::trim).filter(|l| !l.is_empty());
    let mut levels: Vec<String> = Vec::new();
    for l in ug.runs.iter().map(|r| r.level.clone()).chain(ug.stubs.iter().map(|s| s.level.clone())) {
        if only.is_none_or(|o| o.eq_ignore_ascii_case(&l)) && !levels.iter().any(|x| x.eq_ignore_ascii_case(&l)) {
            levels.push(l);
        }
    }
    if levels.is_empty() {
        if let Some(o) = only {
            levels.push(o.to_string());
        } else if let Some(l) = slab_level(&sp) {
            levels.push(l.to_string());
        }
    }
    let called: Vec<Called> = p
        .get("called")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(MAX_CALLED)
                .filter_map(|c| {
                    let text = c.get("text")?.as_str()?.trim();
                    (!text.is_empty()).then(|| Called {
                        sheet: c.get("sheet").and_then(Value::as_str).unwrap_or_default().to_string(),
                        level: c.get("level").and_then(Value::as_str).unwrap_or_default().to_string(),
                        text: text.chars().take(400).collect(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let first = str_param(p, "number").map(str::trim).filter(|n| !n.is_empty()).unwrap_or("E-001").to_string();
    let tb = str_param(p, "title_block").map(str::to_string).or_else(|| title_block(s));
    let mut sheets = Vec::new();
    for (i, level) in levels.iter().enumerate().take(50) {
        let number = sheet_number(&first, i);
        let title = format!("UNDERGROUND / UNDER-SLAB ELECTRICAL - {}", level_name(&sp, level));
        // Characters a layout name can't have (a level named `1/2`) become dashes.
        let name: String = format!("{number} Underground {}", level_name(&sp, level))
            .chars()
            .map(|c| if super::layout::BAD_CHARS.contains(&c) || c.is_control() { '-' } else { c })
            .collect();
        // Built before: the sheet is drawn again on its own layout, emptied first.
        let existing = s.doc()?.layout(&name).map(|l| l.name.clone());
        let layout = match &existing {
            Some(old) => {
                if let Some(l) = s.doc_mut()?.layouts.iter_mut().find(|l| &l.name == old) {
                    l.entities = cadaddy_doc::EntityStore::default();
                }
                s.state_mut()?.selection.clear();
                old.clone()
            }
            None => {
                let made = s.execute("layout.new", &json!({ "name": name, "viewport": false }))?;
                made["name"].as_str().unwrap_or(&name).to_string()
            }
        };
        s.execute("layout.set", &json!({ "name": layout }))?;
        // A 36" × 24" sheet, plotted 1:1 (the plan is drawn to scale on it).
        s.execute(
            "pagesetup",
            &json!({ "layout": layout, "width": SHEET[0] * 25.4, "height": SHEET[1] * 25.4, "margins": [0, 0, 0, 0], "scale": 1, "center": false }),
        )?;
        let runs: Vec<&Buried> = ug.runs.iter().filter(|r| r.level.eq_ignore_ascii_case(level)).collect();
        let stubs: Vec<&Stub> = ug.stubs.iter().filter(|st| st.level.eq_ignore_ascii_case(level)).collect();
        let gear: Vec<&MepEquipment> = sp
            .mep
            .equipment
            .iter()
            .filter(|e| {
                e.level.eq_ignore_ascii_case(level)
                    && (runs.iter().any(|r| r.from == e.tag || r.to == e.tag)
                        || stubs.iter().any(|st| st.feeds == e.tag)
                        || ug.routes.iter().any(|r| r.from == e.tag || r.to == e.tag))
            })
            .collect();
        let mut pts: Vec<[f64; 2]> = runs.iter().flat_map(|r| r.points.iter().copied()).collect();
        pts.extend(gear.iter().flat_map(|e| [e.min, e.max]));
        // The grid lines' places, so every line is inside the plan.
        if let Some((y0, x0)) = span(&pts, 1).zip(span(&pts, 0)) {
            pts.extend(ug.grids.x.iter().map(|(_, x)| [*x, y0.0]));
            pts.extend(ug.grids.y.iter().map(|(_, y)| [x0.0, *y]));
        }
        let f = frame(&pts, u);
        let mut pen = Pen { s, dims: 0 };
        // The sheet: the title block, else a border and a title strip.
        pen.layer("E-UG-ANNO", 7, None)?;
        match &tb {
            Some(b) => {
                pen.s.execute("insert", &json!({ "name": b, "at": [0, 0] }))?;
            }
            None => {
                pen.rect([0.5, 0.5], [SHEET[0] - 0.5, SHEET[1] - 0.5])?;
                pen.line([0.5, 2.0], [SHEET[0] - 0.5, 2.0])?;
            }
        }
        pen.text([1.0, 1.2], &title, 0.3)?;
        pen.text([1.0, 1.65], "GENERATED BY CADADDY FROM THE CONTRACT DOCUMENTS - REQUIRES ENGINEER OF RECORD REVIEW", 0.14)?;
        pen.text([1.0, 0.75], &format!("SCALE: {}", f.scale), 0.14)?;
        pen.text([SHEET[0] - 4.0, 1.0], &number, 0.45)?;
        // Grids across the plan.
        pen.layer("E-UG-GRID", 8, Some("CENTER"))?;
        let (lo, hi) = (f.to(f.min), [PLAN[2], PLAN[3]]);
        for (label, x) in &ug.grids.x {
            let px = f.to([*x, f.min[1]])[0];
            pen.line([px, lo[1] - 0.3], [px, hi[1]])?;
            pen.text([px - 0.06, lo[1] - 0.6], label, 0.14)?;
        }
        for (label, y) in &ug.grids.y {
            let py = f.to([f.min[0], *y])[1];
            pen.line([lo[0] - 0.3, py], [hi[0], py])?;
            pen.text([lo[0] - 0.7, py - 0.06], label, 0.14)?;
        }
        // Gear fed.
        pen.layer("E-UG-EQPM", 5, None)?;
        for e in &gear {
            pen.rect(f.to(e.min), f.to(e.max))?;
            pen.text([f.to(e.max)[0] + 0.05, f.to(e.max)[1] + 0.05], &e.tag, 0.1)?;
        }
        // The buried runs, each tagged: what the documents call for solid, what CADaddy proposes
        // dashed on a layer of its own.
        for r in &runs {
            if r.proposed {
                pen.layer("E-UG-PROP", 30, Some("DASHED"))?;
            } else {
                pen.layer("E-UG-COND", 1, None)?;
            }
            let pp: Vec<[f64; 2]> = r.points.iter().map(|q| f.to(*q)).collect();
            pen.pline(&pp, false)?;
            if let Some(mid) = pp.get(pp.len() / 2).or(pp.first()) {
                let mut what = format!("{} {}-{} {} @ {}\" BFF", r.id, r.conduits, trade_size(r.size_in), r.material, r2(r.depth_in));
                if r.proposed {
                    what.push_str(" (PROPOSED)");
                }
                pen.text([mid[0] + 0.05, mid[1] + 0.08], &what, 0.08)?;
            }
        }
        // Other-trade utilities (pipe services, pipes below the floor) for coordination only.
        let others: Vec<&Other> = ug.other_trade.iter().filter(|o| o.level.is_empty() || o.level.eq_ignore_ascii_case(level)).collect();
        if !others.is_empty() {
            pen.layer("E-UG-OTHR", 8, Some("DASHDOT"))?;
            for o in &others {
                let pp: Vec<[f64; 2]> = o.points.iter().map(|q| f.to(*q)).collect();
                pen.pline(&pp, false)?;
                if let Some(mid) = pp.get(pp.len() / 2).or(pp.first()) {
                    pen.text([mid[0] + 0.05, mid[1] + 0.08], &o.label, 0.08)?;
                }
            }
        }
        // Stub-ups, dimensioned from the grid lines (or the level's edges).
        for st in &stubs {
            pen.layer("E-UG-STUB", 2, None)?;
            let c = f.to(st.at);
            pen.circle(c, 0.08)?;
            pen.text([c[0] + 0.12, c[1] + 0.12], &st.tag, 0.1)?;
            pen.layer("E-UG-DIMS", 3, None)?;
            if let Some((_, from, ft)) = &st.x {
                let a = f.to([*from, st.at[1]]);
                pen.dim(a, c, [c[0], c[1] - 0.35], &ft_in(ft.abs()))?;
            }
            if let Some((_, from, ft)) = &st.y {
                let a = f.to([st.at[0], *from]);
                pen.dim(a, c, [c[0] + 0.35, c[1]], &ft_in(ft.abs()))?;
            }
        }
        // Crossings with other trades, marked where they are.
        let xings: Vec<&Crossing> = ug.crossings.iter().filter(|x| x.level.is_empty() || x.level.eq_ignore_ascii_case(level)).collect();
        if !xings.is_empty() {
            pen.layer("E-UG-XING", 6, None)?;
            for x in &xings {
                let c = f.to(x.at);
                pen.circle(c, 0.15)?;
                pen.line([c[0] - 0.11, c[1] - 0.11], [c[0] + 0.11, c[1] + 0.11])?;
                pen.line([c[0] - 0.11, c[1] + 0.11], [c[0] + 0.11, c[1] - 0.11])?;
                pen.text([c[0] + 0.2, c[1] - 0.25], &x.id, 0.1)?;
            }
        }
        let stub_dims = pen.dims;
        // The schedules and notes down the side.
        pen.layer("E-UG-ANNO", 7, None)?;
        let mut y = SHEET[1] - 1.0;
        let run_rows: Vec<Vec<String>> = runs
            .iter()
            .map(|r| {
                vec![
                    r.id.clone(),
                    r.from.clone(),
                    r.to.clone(),
                    format!("{}-{}", r.conduits, trade_size(r.size_in)),
                    r.material.clone(),
                    format!("{}\"", r2(r.depth_in)),
                    format!("{} ft", r2(r.length_ft).round()),
                    format!("{}°", r.bends_deg),
                    if r.encased { "ENCASED".into() } else { "-".into() },
                    if r.proposed { "PROPOSED - RFI".into() } else { "CALLED".into() },
                ]
            })
            .collect();
        let mut rows = vec![["RUN", "FROM", "TO", "QTY-SIZE", "MATERIAL", "DEPTH", "LENGTH", "BENDS", "ENCASED", "BASIS"].map(String::from).to_vec()];
        rows.extend(run_rows);
        y = pen.table(SIDE_X, y, "UNDERGROUND RUN SCHEDULE", &[0.9, 0.8, 0.8, 0.8, 1.1, 0.6, 0.7, 0.6, 0.8, 1.1], &rows)?;
        let mut rows = vec![["STUB", "FEEDS", "FROM GRID X", "FROM GRID Y", "COORD (FT)", "HEIGHT", "TYPE"].map(String::from).to_vec()];
        let ox = ug.grids.x.first().map_or(0.0, |g| g.1);
        let oy = ug.grids.y.first().map_or(0.0, |g| g.1);
        for st in &stubs {
            rows.push(vec![
                st.tag.clone(),
                st.feeds.clone(),
                along(&st.x, "E", "W"),
                along(&st.y, "N", "S"),
                format!("{:.1}, {:.1}", (st.at[0] - ox) / u, (st.at[1] - oy) / u),
                format!("{}\" AFF", r2(st.height_in)),
                format!("{} {}", mix_text(&st.mix), st.material),
            ]);
        }
        y = pen.table(SIDE_X, y, "STUB-UP SCHEDULE", &[0.6, 0.9, 1.3, 1.3, 1.1, 0.8, 1.4], &rows)?;
        let holds: Vec<&super::concrete::Hold> = ug.holds.iter().filter(|h| h.level.is_empty() || h.level.eq_ignore_ascii_case(level)).collect();
        let mut rows = vec![["POUR", "ITEM", "KIND", "SIZE", "QTY / LENGTH", "STATUS"].map(String::from).to_vec()];
        rows.extend(holds.iter().map(|h| {
            let qty = if h.length_ft > 0.0 { format!("{} ft", r2(h.length_ft).round()) } else { h.count.to_string() };
            vec![h.pour.clone(), h.element.clone(), h.kind.clone(), h.size.clone(), qty, h.status.to_uppercase()]
        }));
        y = pen.table(SIDE_X, y, "POUR HOLD POINTS - CONFIRM BEFORE CONCRETE", &[0.9, 1.6, 1.1, 0.9, 1.1, 1.2], &rows)?;
        if !xings.is_empty() {
            let mut rows = vec![["MARK", "ELECTRICAL", "OTHER TRADE", "WHERE", "VERTICAL CLEAR"].map(String::from).to_vec()];
            rows.extend(xings.iter().map(|x| {
                vec![
                    x.id.clone(),
                    x.electrical.clone(),
                    x.other_label.clone(),
                    if x.how == "crosses" { "CROSSES".into() } else { format!("{} APART", ft_in(x.distance_ft)) },
                    x.separation_in.map_or("VERIFY".into(), |v| format!("{}\"", r2(v))),
                ]
            }));
            y = pen.table(SIDE_X, y, "CROSSINGS - POTHOLE AND VERIFY BEFORE TRENCHING", &[0.6, 1.2, 3.6, 1.2, 1.2], &rows)?;
        }
        let mut notes: Vec<String> = vec![GENERATED_NOTE.into()];
        if runs.iter().any(|r| r.proposed) {
            notes.push(
                "DASHED RUNS ARE NOT CALLED BY THE DOCUMENTS: CADADDY PROPOSES THEM UNDER THE SLAB (SEE THE RUN SCHEDULE'S BASIS). CONFIRM BY RFI BEFORE THE POUR."
                    .into(),
            );
        }
        // Buried work the documents call for in notes whose place no plan shows.
        for c in called.iter().filter(|c| c.level.is_empty() || c.level.eq_ignore_ascii_case(level)).take(12) {
            notes.push(format!("CALLED, NOT LOCATED ON ANY PLAN ({}): {}", c.sheet, c.text.to_ascii_uppercase()));
        }
        for r in ug.routes.iter().filter(|r| r.level.eq_ignore_ascii_case(level) && r.suggest == "under_slab") {
            let saves = match (r.under.cost, r.over.cost) {
                (Some(a), Some(b)) => format!(", saves about ${:.0}", b - a),
                _ => String::new(),
            };
            notes.push(format!(
                "ROUTE {} ({} TO {}) UNDER SLAB: {} FT VS {} FT OVERHEAD{saves}. ROUTING ONLY - SIZES PER FEEDER SCHEDULE.",
                r.feeder,
                r.from,
                r.to,
                r2(r.under.length_ft).round(),
                r2(r.over.length_ft).round()
            ));
        }
        for fd in ug.findings.iter().filter(|fd| fd.severity == "error") {
            notes.push(format!("{}: {} - RFI BEFORE THE SLAB.", fd.about.to_uppercase(), fd.message.to_uppercase()));
        }
        notes.push("VERIFY ALL STUB-UP LOCATIONS AGAINST EQUIPMENT SUBMITTALS BEFORE THE POUR.".into());
        pen.text([SIDE_X, y], "NOTES", 0.16)?;
        let mut ny = y - 0.3;
        // The notes stop above the trench sections.
        let floor = SECTION_BOX[1] + SECTION_BOX[3] + 0.3;
        'notes: for (n, note) in notes.iter().enumerate().take(30) {
            for (j, line) in wrap(&format!("{}. {note}", n + 1), NOTE_CHARS).iter().enumerate() {
                if ny < floor {
                    pen.text([SIDE_X, ny], "... more in ugsheets.check", 0.08)?;
                    break 'notes;
                }
                pen.text([SIDE_X + if j == 0 { 0.0 } else { 0.2 }, ny], line, 0.08)?;
                ny -= 0.16;
            }
            ny -= 0.06;
        }
        // Typical trench sections: the services' and the under-slab runs'.
        pen.layer("E-UG-SECT", 7, None)?;
        let mut sections = 0usize;
        for (kind, title, datum) in
            [("service", "TYPICAL TRENCH SECTION - SERVICE", "GRADE"), ("run", "TYPICAL TRENCH SECTION - UNDER SLAB", "FINISHED FLOOR")]
        {
            let of: Vec<&Buried> = runs.iter().copied().filter(|r| r.kind == kind).collect();
            if of.is_empty() {
                continue;
            }
            let x0 = SECTION_BOX[0] + sections as f64 * (SECTION_BOX[2] + 0.2);
            pen.section(x0, &of, title, datum)?;
            sections += 1;
        }
        sheets.push(json!({
            "layout": layout, "number": number, "title": title, "level": level, "runs": runs.len(), "stubups": stubs.len(), "dimensions": stub_dims,
            "section_dimensions": pen.dims - stub_dims, "crossings": xings.len(), "sections": sections, "replaced": existing.is_some(), "scale": f.scale,
        }));
    }
    let mut files = Vec::new();
    if let Some(path) = str_param(p, "pdf").map(str::trim).filter(|x| !x.is_empty()) {
        let many = sheets.len() > 1;
        for (i, sh) in sheets.iter().enumerate() {
            let out = if many { numbered(path, i + 1) } else { path.to_string() };
            s.execute("exportpdf", &json!({ "path": out, "layout": sh["layout"] }))?;
            files.push(out);
        }
    }
    if let Some(path) = str_param(p, "dwg").map(str::trim).filter(|x| !x.is_empty()) {
        s.execute("export", &json!({ "path": path, "format": "dwg" }))?;
        files.push(path.to_string());
    }
    Ok(json!({ "sheets": sheets, "files": files }))
}

/// Conduits by size: `4-4" + 2-1-1/2"`.
fn mix_text(mix: &[(u32, f64)]) -> String {
    mix.iter().map(|(n, s)| format!("{n}-{}", trade_size(*s))).collect::<Vec<_>>().join(" + ")
}

/// Words into lines of at most `width` characters (a longer word on its own line).
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for w in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > width {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A stub-up's distance from its reference as the field reads it: `4'-0" W of 7`, `0'-0" at B`.
fn along(d: &Option<(String, f64, f64)>, plus: &str, minus: &str) -> String {
    let Some((from, _, ft)) = d else { return "-".into() };
    let name = from.trim_start_matches("grid ");
    if ft.abs() < 1.0 / 24.0 { format!("on {name}") } else { format!("{} {} of {name}", ft_in(ft.abs()), if *ft > 0.0 { plus } else { minus }) }
}

/// The span of points along axis `i` (0: x, 1: y): least and most.
fn span(pts: &[[f64; 2]], i: usize) -> Option<(f64, f64)> {
    let vs: Vec<f64> = pts.iter().filter_map(|p| p.get(i).copied()).filter(|v| v.is_finite()).collect();
    (!vs.is_empty()).then(|| (vs.iter().copied().fold(f64::INFINITY, f64::min), vs.iter().copied().fold(f64::NEG_INFINITY, f64::max)))
}

fn level_name(sp: &BuildingSpec, level: &str) -> String {
    sp.level(level).map(|l| l.name.clone()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| format!("Level {level}"))
}

/// `sheets.pdf` → `sheets-2.pdf`.
fn numbered(path: &str, n: usize) -> String {
    match path.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.contains(['/', '\\']) => format!("{stem}-{n}.{ext}"),
        _ => format!("{path}-{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_numbers_read_like_the_prints() {
        assert_eq!(
            (trade_size(2.0), trade_size(1.25), trade_size(0.75), trade_size(4.0)),
            ("2\"".into(), "1-1/4\"".into(), "3/4\"".into(), "4\"".into())
        );
        assert_eq!((wire_size("4/0"), wire_size("#2"), wire_size("500 kcmil")), ("#4/0".into(), "#2".into(), "500 kcmil".into()));
        assert_eq!((ft_in(12.5), ft_in(-3.25), ft_in(f64::NAN)), ("12'-6\"".into(), "-3'-3\"".into(), "?".into()));
        assert_eq!((sheet_number("E-001", 0), sheet_number("E-001", 2), sheet_number("UG", 1)), ("E-001".into(), "E-003".into(), "UG.2".into()));
        assert_eq!((numbered("out/ug.pdf", 2), numbered("ug", 3)), ("out/ug-2.pdf".into(), "ug-3".into()));
        assert_eq!(mix_text(&[(4, 4.0), (2, 1.5)]), "4-4\" + 2-1-1/2\"");
        assert_eq!(wrap("one two three four", 9), vec!["one two", "three", "four"]);
        assert_eq!(wrap("averyveryverylongword x", 5), vec!["averyveryverylongword", "x"]);
        assert_eq!((material_of("E-UG-80"), material_of("E-UG RGS"), material_of("E-UG")), ("PVC Sch 80".into(), "RGS".into(), "PVC Sch 40".into()));
        let s = Service { kind: "telecom".into(), ..Service::default() };
        assert!(is_kind(&s, "telephone") && !is_kind(&s, "power"));
    }

    #[test]
    fn turns_crossings_and_separation() {
        // An L, a U, a straight line with a repeated point, a reversal; nothing from NaN.
        assert!((turns(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]]) - 90.0).abs() < 1e-9);
        assert!((turns(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]) - 180.0).abs() < 1e-9);
        assert!(turns(&[[0.0, 0.0], [5.0, 0.0], [5.0, 0.0], [10.0, 0.0]]).abs() < 1e-9);
        assert!((turns(&[[0.0, 0.0], [10.0, 0.0], [0.0, 0.0]]) - 180.0).abs() < 1e-9);
        assert!(turns(&[[0.0, 0.0], [f64::NAN, 1.0], [2.0, 2.0]]).is_finite());
        // Crossing, touching at an end, parallel, apart.
        assert_eq!(seg_hit([0.0, 0.0], [10.0, 0.0], [5.0, -5.0], [5.0, 5.0]), Some([5.0, 0.0]));
        assert_eq!(seg_hit([0.0, 0.0], [10.0, 0.0], [10.0, 0.0], [10.0, 5.0]), Some([10.0, 0.0]));
        assert_eq!(seg_hit([0.0, 0.0], [10.0, 0.0], [0.0, 1.0], [10.0, 1.0]), None);
        assert_eq!(seg_hit([0.0, 0.0], [10.0, 0.0], [12.0, -5.0], [12.0, 5.0]), None);
        assert_eq!(seg_gap([0.0, 0.0], [10.0, 0.0], [12.0, -5.0], [12.0, 5.0]), (2.0, [10.0, 0.0]));
        // Above, below, overlapping.
        assert_eq!((separation(22.5, 1.5, 48.0, 6.0), separation(48.0, 6.0, 22.5, 1.5), separation(22.5, 4.0, 24.0, 2.0)), (24.0, 24.0, 0.0));
    }
}
