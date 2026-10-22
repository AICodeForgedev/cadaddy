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

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("ugsheets.check", "Check Underground", run_check)
            .params("{today?: YYYY-MM-DD} → {ok, errors, warnings, runs: [{id, kind, system, level, from, to, size_in, conduits, material, depth_in, length_ft}], stubups: [{tag, level, at, feeds, run, size_in, conduits, material, height_in, x: {from, ft}, y: {from, ft}, coords_ft}], services: [{kind, tag, status, to, at}], routes: [{feeder, from, to, level, under_slab: {length_ft, cost?}, overhead: {length_ft, cost?}, basis, suggest}], holds: [{pour, item, kind, size, status}], findings: [{check, severity, about, level, message, fix}]}")
            .noundo(),
        CommandSpec::new("ugsheets.review", "Review Underground", run_review)
            .params("{} → {errors, rfis: [id], findings}: an RFI (service_entrance) for each required service not shown or unclear"),
        CommandSpec::new("ugsheets.build", "Underground Sheets", run_build)
            .params("{level?: name (default: every level with buried work), number?: first sheet number (default \"E-001\"), title_block?: block name (default: a block named like a title block), today?, pdf?: path, dwg?: path} → {sheets: [{layout, number, title, level, runs, stubups, dimensions}], files: [path]}"),
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
}

impl Finding {
    fn json(&self) -> Value {
        json!({ "check": self.check, "severity": self.severity, "about": self.about, "level": self.level, "at": self.at, "message": self.message, "fix": self.fix })
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
struct Grids {
    x: Vec<(String, f64)>,
    y: Vec<(String, f64)>,
}

struct Ug {
    runs: Vec<Buried>,
    stubs: Vec<Stub>,
    services: Vec<Value>,
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

/// Grid lines from the spec's columns (`A-1` at a point): a label whose columns line up on one x
/// is a vertical line, on one y a horizontal one.
fn grids(sp: &BuildingSpec) -> Grids {
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

fn study(sp: &BuildingSpec, types: &cadaddy_doc::bimspec::BimTypes, today: Option<i64>) -> Ug {
    let u = upf(sp);
    let mut runs: Vec<Buried> = Vec::new();
    let mut findings: Vec<Finding> = Vec::new();
    // Conduit runs below the floor.
    for (i, r) in sp.mep.runs.iter().enumerate().take(MAX_ITEMS) {
        let Some(sys) = sp.mep.systems.iter().find(|s| s.name.eq_ignore_ascii_case(&r.system)) else { continue };
        if sys.kind != MepKind::Conduit {
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
        services.push(json!({ "kind": svc.kind, "tag": svc.tag, "status": status, "to": svc.to, "at": svc.at, "level": svc.level }));
        let (Some(at), true) = (svc.at, svc.shown) else { continue };
        let points: Vec<[f64; 2]> = svc.from.into_iter().chain([at]).filter(|p| finite(*p)).collect();
        let id = if svc.tag.trim().is_empty() { format!("SVC-{}", svc.kind.to_ascii_uppercase()) } else { svc.tag.clone() };
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
        let under = Option_ { length_ft: under_len, cost: route_cost(sp, f, "PVC conduit", under_len, &mut missing) };
        let over = Option_ { length_ft: over_len, cost: route_cost(sp, f, "EMT conduit", over_len, &mut missing) };
        let (suggest, basis) = match (under.cost, over.cost) {
            (Some(uc), Some(oc)) => (if uc < oc * CHEAPER { "under_slab" } else { "overhead" }, "estimator prices and crew rate".to_string()),
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
    findings.truncate(MAX_ITEMS);
    Ug { runs, stubs, services, routes, holds, findings, grids: g }
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
        })).collect::<Vec<_>>(),
        "stubups": ug.stubs.iter().map(|s| json!({
            "tag": s.tag, "level": s.level, "at": s.at, "feeds": s.feeds, "run": s.run, "size_in": s.size_in, "conduits": s.conduits,
            "mix": s.mix.iter().map(|(n, size)| json!({ "count": n, "size_in": size })).collect::<Vec<_>>(), "material": s.material, "height_in": r2(s.height_in), "x": dim(&s.x), "y": dim(&s.y), "coords_ft": [r2(s.at[0] - ox), r2(s.at[1] - oy)],
        })).collect::<Vec<_>>(),
        "services": ug.services,
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

fn run_check(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec_of("ugsheets.check", s)?;
    let t = today_of("ugsheets.check", p)?;
    Ok(ug_json(&study(&sp, &s.doc()?.bim_types, t)))
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
    let ug = study(&sp, &s.doc()?.bim_types, None);
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
    let ug = study(&sp, &s.doc()?.bim_types, t);
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
    let first = str_param(p, "number").map(str::trim).filter(|n| !n.is_empty()).unwrap_or("E-001").to_string();
    let tb = str_param(p, "title_block").map(str::to_string).or_else(|| title_block(s));
    let mut sheets = Vec::new();
    for (i, level) in levels.iter().enumerate().take(50) {
        let number = sheet_number(&first, i);
        let title = format!("UNDERGROUND / UNDER-SLAB ELECTRICAL - {}", level_name(&sp, level));
        let name = format!("{number} Underground {}", level_name(&sp, level));
        let made = s.execute("layout.new", &json!({ "name": name, "viewport": false }))?;
        let layout = made["name"].as_str().unwrap_or(&name).to_string();
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
        // The buried runs, each tagged.
        pen.layer("E-UG-COND", 1, Some("HIDDEN"))?;
        for r in &runs {
            let pp: Vec<[f64; 2]> = r.points.iter().map(|q| f.to(*q)).collect();
            pen.pline(&pp, false)?;
            if let Some(mid) = pp.get(pp.len() / 2).or(pp.first()) {
                let what = format!("{} {}-{} {} @ {}\" BFF", r.id, r.conduits, trade_size(r.size_in), r.material, r2(r.depth_in));
                pen.text([mid[0] + 0.05, mid[1] + 0.08], &what, 0.08)?;
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
                ]
            })
            .collect();
        let mut rows = vec![["RUN", "FROM", "TO", "QTY-SIZE", "MATERIAL", "DEPTH", "LENGTH"].map(String::from).to_vec()];
        rows.extend(run_rows);
        y = pen.table(SIDE_X, y, "UNDERGROUND RUN SCHEDULE", &[1.2, 1.0, 1.0, 0.9, 1.4, 0.7, 0.9], &rows)?;
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
        let mut notes: Vec<String> = Vec::new();
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
        for (n, note) in notes.iter().enumerate().take(30) {
            for (j, line) in wrap(&format!("{}. {note}", n + 1), NOTE_CHARS).iter().enumerate() {
                pen.text([SIDE_X + if j == 0 { 0.0 } else { 0.2 }, ny], line, 0.08)?;
                ny -= 0.16;
            }
            ny -= 0.06;
        }
        let dims = pen.dims;
        sheets.push(json!({ "layout": layout, "number": number, "title": title, "level": level, "runs": runs.len(), "stubups": stubs.len(), "dimensions": dims, "scale": f.scale }));
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
}
