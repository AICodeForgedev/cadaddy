//! The construction sequence (Q6014): the built model turned into a schedule of activities, per
//! level and zone, in the order a building goes up — slab and structure, exterior walls,
//! interior framing, overhead rough-in by trade (duct, then sprinkler, then plumbing, then
//! electrical), in-wall rough-in, the inspection before cover-up, insulation, drywall, the
//! ceiling grid, trim-out, doors and casework, finishes. Each activity names the elements it
//! installs and their quantities; labor hours come from the cost book (Q6012) and give the
//! duration at a crew size (editable per trade and per activity).
//!
//! `schedule.build` writes the sequence as the drawing's 4D schedule (`fourd.*`, the 4D palette
//! and Walk show it on a date); `schedule.view` gives the model as built on a date or at an
//! activity's finish; `schedule.export` writes CSV, XLSX or MS Project XML;
//! `schedule.conflicts` reads whatever 4D schedule the drawing has (built here or imported) for
//! trades in the same ceiling space at the same time, cover-up before rough-in or without an
//! inspection (the cover-check phases), elements installed before their host, and overhead work
//! out of order; `schedule.fit` checks each room's ceiling space (structure to ceiling) against
//! the duct, pipe and conduit stacked in it; `schedule.takeoff` is the takeoff by activity.
//!
//! The settings (start, crews, working days, zone size, rates, edits) are kept in the building
//! spec (`sequence`); the sequence itself is built from the model each time, so it follows the
//! model as it changes.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::{Drawing, EntityKind, Handle};
use cadaddy_walk3d::fourd::{self, Schedule4d, Task, day, iso};
use serde_json::{Map, Value, json};

use super::*;
use crate::{Result, Session};

/// Most model elements read.
const MAX_ELEMENTS: usize = 500_000;
/// Most vertices of one object measured.
const MAX_VERTICES: usize = 4_096;
/// Most tags of one object read.
const MAX_TAGS: usize = 128;
/// Most zones one level is split into.
const MAX_ZONES: usize = 50;
/// Most activities listed in one reply (the rest are counted).
const MAX_LISTED: usize = 2_000;
/// Most handles one finding keeps.
const FINDING_ELEMENTS: usize = 20;
/// Most findings of one kind returned.
const MAX_FINDINGS: usize = 1_000;
/// Most element pairs compared by a sweep.
const MAX_PAIRS: usize = 5_000_000;
/// Most points a ceiling-space check samples per area.
const MAX_SAMPLES: usize = 20_000;
/// Longest a schedule may run, working days (hostile settings).
const MAX_DAYS: i64 = 20_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("schedule.build", "Build Construction Sequence", run_build).params(
            "{start?: date (2026-03-02; default the stored start, else today), hours_per_day?: 8, workdays?: 5 (working days a week: 5 Mon-Fri, 6, 7), crew?: number (people per crew, every trade) | {trade or activity kind: people} (4), zone_sf?: square feet per zone (10000; a level larger than this is split into zones along its long side), rates?: {activity kind: labor hours per unit} (default the cost book), apply4d?: bool (default true: the sequence becomes the drawing's 4D schedule)} → {activities, span: [start, finish], working_days, hours, by_trade: [{trade, activities, hours}], by_level, not_sequenced: {part: count}, notes, list: [activity]}",
        ),
        CommandSpec::new("schedule.list", "Construction Sequence", run_list)
            .params("{level?, trade?, zone?, handles?: bool (list each activity's element handles; default false)} → {activities, span, list: [{id, name, level, zone, kind, trade, quantity, unit, quantities, hours, crew, days, start, finish, predecessors, elements, rate, phases?, handles?}]}")
            .noundo(),
        CommandSpec::new("schedule.edit", "Edit Sequence Activity", run_edit).params(
            "{activity: id, crew?: people, days?: working days (fixes the duration), hours?: labor hours (instead of quantity × rate), clear?: bool (drop the activity's edits)} → {activity, span, changed: [ids whose dates moved]}",
        ),
        CommandSpec::new("schedule.view", "Show Sequence at Date", run_view).params(
            "{date?: e.g. 2026-03-09 | activity?: id (the day after it finishes: the model with that work in place), png?: path (render the model as it stands), width?: 1280, height?: 720, view?: \"swiso\", ghost?: bool (show not-yet work pale), handles?: bool (default false)} → {date, counts: {built, inProgress, notYet, unscheduled}, in_progress: [activity ids], finished, upcoming: [next activities], png?, built?, inProgressHandles?}",
        ),
        CommandSpec::new("schedule.export", "Export Construction Sequence", run_export)
            .params("{path: .csv, .xlsx or .xml (MS Project XML) file, format?: csv|xlsx|msproject (from the extension)} → {path, activities, format}")
            .noundo(),
        CommandSpec::new("schedule.conflicts", "Sequence Trade Conflicts", run_conflicts)
            .params("{level?, tight_in?: ceiling space margin below which overhead order matters most (6)} → {schedule: tasks checked, findings: [{kind: trade_overlap|covered_before_roughin|cover_without_inspection|before_host|overhead_order, severity, level, zone, tasks, trades?, dates?, elements, message, phases?}], by_kind, ok}")
            .noundo(),
        CommandSpec::new("schedule.fit", "Ceiling Space Fit", run_fit)
            .params("{level?, clearance_in?: space kept around each run, inches (1), tight_in?: margin called tight (6)} → {areas: [{area, number?, level, available_in, required_in, margin_in, fits, tight, governing: {at: [x,y], elements: [{handle, system, kind, size, depth_in}]}, modeled_overlap}], findings: [{area, level, message, elements}], ok}")
            .noundo(),
        CommandSpec::new("schedule.takeoff", "Takeoff by Activity", run_takeoff)
            .params("{level?, trade?} → {activities: [{id, name, level, zone, trade, hours, material, items: [{item, size, unit, qty, labor_hours, material}]}], totals: {hours, material}, source}")
            .noundo(),
    ]
}

// ── the model's elements ─────────────────────────────────────────────────────

/// A model element as the sequence sees it.
#[derive(Clone, Debug, Default)]
struct El {
    handle: Handle,
    part: String,
    level: String,
    tags: BTreeMap<String, String>,
    /// Plan box `[x0, y0, x1, y1]` and height range, drawing units.
    bx: [f64; 4],
    z: [f64; 2],
    /// Length along the plan's long axis and thickness across it, drawing units.
    len: f64,
    thick: f64,
    /// Area of the top faces in plan, square drawing units.
    area: f64,
    /// The top faces as plan polygons (slabs, rooms).
    tops: Vec<Vec<[f64; 2]>>,
}

impl El {
    fn tag(&self, k: &str) -> &str {
        self.tags.get(k).map(String::as_str).unwrap_or_default()
    }
    fn kind(&self) -> &str {
        self.tag("kind")
    }
    fn system(&self) -> &str {
        self.tag("system")
    }
    fn center(&self) -> [f64; 2] {
        [(self.bx[0] + self.bx[2]) / 2.0, (self.bx[1] + self.bx[3]) / 2.0]
    }
}

fn read_elements(d: &Drawing) -> Vec<El> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if out.len() >= MAX_ELEMENTS {
            break;
        }
        let x = &e.common.xdata;
        let Some(at) = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP)) else { continue };
        let tags: Vec<&str> =
            x.iter().skip(at + 1).take_while(|t| t.code != 1001).filter(|t| t.code == 1000).map(|t| t.value.as_str()).take(MAX_TAGS).collect();
        let (Some(part), Some(level)) = (tags.first(), tags.get(1)) else { continue };
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let mut el = El { handle: e.handle, part: part.trim().to_ascii_lowercase(), level: level.trim().to_string(), ..El::default() };
        for t in tags.iter().skip(3) {
            if let Some((k, v)) = t.split_once(':') {
                el.tags.entry(k.trim().to_ascii_lowercase()).or_insert_with(|| v.trim().to_string());
            }
        }
        if el.part == "device" {
            // A device's kind is in its properties (`mep.read_devices`, `model.build`).
            let pr = cadaddy_doc::props::read(x);
            for k in ["type", "type_mark"] {
                if let Some(v) = pr.get(k).map(|p| p.text()).filter(|v| !v.trim().is_empty()) {
                    el.tags.insert(k.into(), v.trim().chars().take(200).collect());
                }
            }
        }
        if !measure(m, &mut el) {
            continue;
        }
        out.push(el);
    }
    out
}

/// The element's plan box, height range, principal length and thickness, and top faces.
fn measure(m: &cadaddy_doc::Mesh, el: &mut El) -> bool {
    let pts: Vec<&cadaddy_geom::Vec3> =
        m.vertices.iter().take(MAX_VERTICES).filter(|p| p.x.is_finite() && p.y.is_finite() && p.z.is_finite()).collect();
    if pts.is_empty() {
        return false;
    }
    let mut bx = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    let mut z = [f64::INFINITY, f64::NEG_INFINITY];
    let n = pts.len() as f64;
    let (mut mx, mut my) = (0.0, 0.0);
    for p in &pts {
        bx = [bx[0].min(p.x), bx[1].min(p.y), bx[2].max(p.x), bx[3].max(p.y)];
        z = [z[0].min(p.z), z[1].max(p.z)];
        mx += p.x;
        my += p.y;
    }
    let (mx, my) = (mx / n, my / n);
    let (mut cxx, mut cyy, mut cxy) = (0.0, 0.0, 0.0);
    for p in &pts {
        let (dx, dy) = (p.x - mx, p.y - my);
        cxx += dx * dx;
        cyy += dy * dy;
        cxy += dx * dy;
    }
    let angle = 0.5 * (2.0 * cxy).atan2(cxx - cyy);
    let (c, s) = (angle.cos(), angle.sin());
    let (mut u, mut v) = ([f64::INFINITY, f64::NEG_INFINITY], [f64::INFINITY, f64::NEG_INFINITY]);
    for p in &pts {
        let (dx, dy) = (p.x - mx, p.y - my);
        let (a, b) = (dx * c + dy * s, -dx * s + dy * c);
        u = [u[0].min(a), u[1].max(a)];
        v = [v[0].min(b), v[1].max(b)];
    }
    let (a, b) = (u[1] - u[0], v[1] - v[0]);
    el.bx = bx;
    el.z = z;
    el.len = a.max(b);
    el.thick = a.min(b);
    // The top faces: every vertex at the highest point (a slab's or a room's plan outline).
    let eps = ((z[1] - z[0]).abs() * 1e-3).max(1e-6);
    let mut area = 0.0;
    for f in m.faces.iter().take(MAX_VERTICES) {
        let poly: Vec<[f64; 2]> =
            f.v.iter().filter_map(|&i| m.vertices.get(i as usize)).filter(|p| (p.z - z[1]).abs() <= eps).map(|p| [p.x, p.y]).collect();
        if poly.len() < 3 || poly.len() != f.v.len() {
            continue;
        }
        let a = shoelace(&poly).abs();
        if a > 0.0 && a.is_finite() {
            area += a;
            if el.tops.len() < 256 {
                el.tops.push(poly);
            }
        }
    }
    el.area = if area > 0.0 { area } else { (bx[2] - bx[0]) * (bx[3] - bx[1]) };
    [el.len, el.thick, el.area, z[0], z[1]].iter().all(|v| v.is_finite())
}

fn shoelace(p: &[[f64; 2]]) -> f64 {
    let n = p.len();
    (0..n)
        .map(|i| {
            let (a, b) = (p[i], p[(i + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
        / 2.0
}

fn inside(poly: &[[f64; 2]], q: [f64; 2]) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > q[1]) != (b[1] > q[1]) && q[0] < (b[0] - a[0]) * (q[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
    }
    c
}

fn grow(b: [f64; 4], d: f64) -> [f64; 4] {
    [b[0] - d, b[1] - d, b[2] + d, b[3] + d]
}

fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

fn contains(b: [f64; 4], p: [f64; 2]) -> bool {
    b[0] <= p[0] && p[0] <= b[2] && b[1] <= p[1] && p[1] <= b[3]
}

// ── levels ───────────────────────────────────────────────────────────────────

/// A level's heights, drawing units.
#[derive(Clone, Debug)]
struct Lvl {
    name: String,
    elev: f64,
    ceiling: f64,
    top: f64,
}

struct Ctx {
    upf: f64,
    levels: Vec<Lvl>,
}

impl Ctx {
    fn new(sp: &cadaddy_walk3d::BuildingSpec) -> Ctx {
        let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
        let mut levels: Vec<Lvl> = sp
            .levels
            .iter()
            .filter(|l| l.elevation_ft.is_finite())
            .map(|l| {
                let h = if l.height_ft.is_finite() && l.height_ft > 0.0 { l.height_ft } else { 10.0 };
                let c = if l.ceiling_ft.is_finite() && l.ceiling_ft > 0.0 { l.ceiling_ft.min(h) } else { h.min(8.0) };
                Lvl { name: l.name.clone(), elev: l.elevation_ft * upf, ceiling: (l.elevation_ft + c) * upf, top: (l.elevation_ft + h) * upf }
            })
            .collect();
        levels.sort_by(|a, b| a.elev.total_cmp(&b.elev));
        Ctx { upf, levels }
    }

    fn level(&self, name: &str) -> Option<&Lvl> {
        self.levels.iter().find(|l| l.name.eq_ignore_ascii_case(name.trim()))
    }

    fn rank(&self, name: &str) -> usize {
        self.levels.iter().position(|l| l.name.eq_ignore_ascii_case(name.trim())).unwrap_or(self.levels.len())
    }

    fn inches(&self, units: f64) -> f64 {
        units / self.upf * 12.0
    }

    /// Whether the element sits in the ceiling space (above its level's ceiling).
    fn overhead(&self, e: &El) -> bool {
        self.level(&e.level).is_some_and(|l| e.z[0] >= l.ceiling - self.upf / 12.0)
    }

    /// Whether a device hangs at the ceiling (its top within 6" of it, or above).
    fn at_ceiling(&self, e: &El) -> bool {
        self.level(&e.level).is_some_and(|l| e.z[1] >= l.ceiling - self.upf / 2.0)
    }
}

// ── activity kinds ───────────────────────────────────────────────────────────

/// What an activity does, in the order the work goes in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Kind {
    Slab,
    Structure,
    ExteriorWalls,
    Framing,
    Duct,
    Sprinkler,
    Plumbing,
    Electrical,
    InWallElectrical,
    InWallPlumbing,
    Inspection,
    Insulation,
    Drywall,
    Ceilings,
    TrimOut,
    Casework,
    Finishes,
}

impl Kind {
    const ALL: [Kind; 17] = [
        Kind::Slab,
        Kind::Structure,
        Kind::ExteriorWalls,
        Kind::Framing,
        Kind::Duct,
        Kind::Sprinkler,
        Kind::Plumbing,
        Kind::Electrical,
        Kind::InWallElectrical,
        Kind::InWallPlumbing,
        Kind::Inspection,
        Kind::Insulation,
        Kind::Drywall,
        Kind::Ceilings,
        Kind::TrimOut,
        Kind::Casework,
        Kind::Finishes,
    ];

    fn code(self) -> &'static str {
        match self {
            Kind::Slab => "slab",
            Kind::Structure => "structure",
            Kind::ExteriorWalls => "exterior_walls",
            Kind::Framing => "framing",
            Kind::Duct => "duct",
            Kind::Sprinkler => "sprinkler",
            Kind::Plumbing => "plumbing",
            Kind::Electrical => "electrical",
            Kind::InWallElectrical => "inwall_electrical",
            Kind::InWallPlumbing => "inwall_plumbing",
            Kind::Inspection => "inspection",
            Kind::Insulation => "insulation",
            Kind::Drywall => "drywall",
            Kind::Ceilings => "ceilings",
            Kind::TrimOut => "trim_out",
            Kind::Casework => "casework",
            Kind::Finishes => "finishes",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Slab => "Floor slab",
            Kind::Structure => "Structure",
            Kind::ExteriorWalls => "Exterior walls and windows",
            Kind::Framing => "Interior framing",
            Kind::Duct => "Overhead rough-in: duct",
            Kind::Sprinkler => "Overhead rough-in: sprinkler",
            Kind::Plumbing => "Overhead rough-in: plumbing",
            Kind::Electrical => "Overhead rough-in: electrical",
            Kind::InWallElectrical => "In-wall rough-in: electrical",
            Kind::InWallPlumbing => "In-wall rough-in: plumbing",
            Kind::Inspection => "Rough-in inspection before cover-up",
            Kind::Insulation => "Insulation",
            Kind::Drywall => "Drywall",
            Kind::Ceilings => "Ceiling grid",
            Kind::TrimOut => "Trim-out",
            Kind::Casework => "Doors and casework",
            Kind::Finishes => "Finishes",
        }
    }

    fn trade(self) -> &'static str {
        match self {
            Kind::Slab => "Concrete",
            Kind::Structure => "Structural",
            Kind::ExteriorWalls | Kind::Framing => "Framing",
            Kind::Duct => "Mechanical",
            Kind::Sprinkler => "Fire Protection",
            Kind::Plumbing | Kind::InWallPlumbing => "Plumbing",
            Kind::Electrical | Kind::InWallElectrical | Kind::TrimOut => "Electrical",
            Kind::Inspection => "Inspection",
            Kind::Insulation => "Insulation",
            Kind::Drywall => "Drywall",
            Kind::Ceilings => "Ceilings",
            Kind::Casework => "Carpentry",
            Kind::Finishes => "Finishes",
        }
    }

    fn overhead(self) -> bool {
        matches!(self, Kind::Duct | Kind::Sprinkler | Kind::Plumbing | Kind::Electrical)
    }

    fn rough_in(self) -> bool {
        self.overhead() || matches!(self, Kind::InWallElectrical | Kind::InWallPlumbing)
    }

    fn from_code(c: &str) -> Option<Kind> {
        Kind::ALL.iter().copied().find(|k| k.code().eq_ignore_ascii_case(c.trim()))
    }
}

/// What one element puts into an activity: a cost-book item and its quantity.
#[derive(Clone, Debug)]
struct Contribution {
    kind: Kind,
    item: &'static str,
    size: String,
    unit: &'static str,
    qty: f64,
    /// Labor hours per unit when the cost book has no such item.
    fallback: f64,
}

/// The trade an element belongs to in the ceiling space (for overlap and order checks).
fn el_trade(e: &El) -> Option<&'static str> {
    match e.part.as_str() {
        "mep" => Some(match e.kind() {
            "duct" => "Mechanical",
            "pipe" if is_fire(e.system()) => "Fire Protection",
            "pipe" => "Plumbing",
            "conduit" | "tray" | "cable" | "light_fixture" | "outlet" | "receptacle" | "switch" | "junction_box" | "panel" | "transformer" => {
                "Electrical"
            }
            _ if is_fire(e.system()) || e.tag("part").starts_with("terminal:sprinkler") => "Fire Protection",
            _ => "Mechanical",
        }),
        "device" => Some("Electrical"),
        _ => None,
    }
}

/// A device or terminal set in the finished surface (a receptacle, a light, a diffuser): not
/// rough-in hidden in the ceiling space, though it may hang above the ceiling line.
fn surface_device(e: &El) -> bool {
    e.part == "device"
        || (e.part == "mep"
            && (matches!(
                e.kind(),
                "outlet" | "receptacle" | "data_outlet" | "floor_box" | "switch" | "disconnect" | "junction_box" | "light_fixture"
            ) || e.tag("part").starts_with("terminal:")))
}

/// The order overhead work goes in: duct, sprinkler, plumbing, electrical.
fn overhead_rank(trade: &str) -> Option<usize> {
    ["Mechanical", "Fire Protection", "Plumbing", "Electrical"].iter().position(|t| *t == trade)
}

fn is_fire(system: &str) -> bool {
    let up = system.to_ascii_uppercase();
    ["FP", "FIRE", "SPRINK", "SPRK", "STANDPIPE"].iter().any(|k| up.contains(k))
}

fn ft(c: &Ctx, units: f64) -> f64 {
    units / c.upf
}

fn sf(c: &Ctx, units2: f64) -> f64 {
    units2 / (c.upf * c.upf)
}

/// The size tag as the cost book writes it (`1` → `1"`, `3/4` → `3/4"`).
fn inch_size(tag: &str) -> String {
    let t = tag.trim().trim_end_matches('"');
    if t.is_empty() { String::new() } else { format!("{t}\"") }
}

/// What an element contributes to which activities. Walls also say whether they are outside.
fn contributions(c: &Ctx, e: &El, exterior: bool) -> Vec<Contribution> {
    let one = |kind, item, unit, qty: f64, fallback| Contribution { kind, item, size: String::new(), unit, qty, fallback };
    let wall_sf = sf(c, e.len * (e.z[1] - e.z[0]).max(0.0));
    match e.part.as_str() {
        "floor" | "slab" => vec![one(Kind::Slab, "Floor slab", "sq ft", sf(c, e.area), 0.04)],
        "column" | "beam" | "gridcolumn" | "joist" => vec![one(Kind::Structure, "Structural member", "each", 1.0, 4.0)],
        "stair" | "ramp" => vec![one(Kind::Structure, "Stair", "each", 1.0, 16.0)],
        "roof" => vec![one(Kind::Structure, "Roof area", "sq ft", sf(c, e.area), 0.05)],
        "wall" | "parapet" | "curtainwall" if exterior || e.part != "wall" => vec![
            one(Kind::ExteriorWalls, "Wall area", "sq ft", wall_sf, 0.08),
            one(Kind::Insulation, "Insulation", "sq ft", wall_sf, 0.015),
            one(Kind::Drywall, "Board gypsum board", "sq ft", wall_sf, 0.02),
        ],
        "wall" => vec![one(Kind::Framing, "Wall", "lf", ft(c, e.len), 0.25), one(Kind::Drywall, "Board gypsum board", "sq ft", 2.0 * wall_sf, 0.02)],
        "window" => vec![one(Kind::ExteriorWalls, "Window", "each", 1.0, 1.0)],
        "door" => vec![one(Kind::Casework, "Door", "each", 1.0, 1.5), one(Kind::Casework, "Door hardware set", "each", 1.0, 0.5)],
        "casework" | "millwork" | "furniture" => vec![one(Kind::Casework, "Casework", "lf", ft(c, e.len).max(1.0), 1.0)],
        "ceiling" | "soffit" => vec![one(Kind::Ceilings, "Ceiling", "sq ft", sf(c, e.area), 0.06)],
        "room" => vec![one(Kind::Finishes, "Finishes", "sq ft", sf(c, e.area), 0.03)],
        "railing" => vec![one(Kind::Finishes, "Railing", "lf", ft(c, e.len), 0.3)],
        "device" => device(c, e),
        "mep" => mep(c, e),
        _ => Vec::new(),
    }
}

fn device(c: &Ctx, e: &El) -> Vec<Contribution> {
    let ty = format!("{} {} {}", e.tag("type"), e.tag("type_mark"), e.tag("name")).to_ascii_uppercase();
    let has = |keys: &[&str]| keys.iter().any(|k| ty.contains(k));
    let (item, fallback) = if has(&["SENSOR"]) {
        ("Occupancy sensor", 0.45)
    } else if has(&["LIGHT", "LUMIN", "FIXTURE", "LT-"]) {
        ("Light fixture", 0.75)
    } else if has(&["FIRE ALARM", "SMOKE", "HORN", "STROBE"]) {
        ("Fire alarm device", 0.6)
    } else if has(&["DATA", "LOW VOLTAGE", "TEL"]) {
        ("Data outlet", 0.45)
    } else if has(&["SWITCH"]) {
        ("Switch", 0.3)
    } else {
        ("Receptacle", 0.4)
    };
    let mk = |kind, item, fallback| Contribution { kind, item, size: String::new(), unit: "each", qty: 1.0, fallback };
    if c.at_ceiling(e) {
        vec![mk(Kind::TrimOut, item, fallback)]
    } else {
        // The box is roughed in in the wall; the device goes on at trim-out.
        vec![mk(Kind::InWallElectrical, "Box, 4\" square", 0.15), mk(Kind::TrimOut, item, fallback)]
    }
}

fn mep(c: &Ctx, e: &El) -> Vec<Contribution> {
    let piece = e.tag("part");
    let kind = e.kind();
    let overhead = c.overhead(e);
    let len = ft(c, e.len);
    let mk = |k, item, size: String, unit, qty, fallback| vec![Contribution { kind: k, item, size, unit, qty, fallback }];
    if piece.starts_with("terminal:sprinkler") {
        return mk(Kind::TrimOut, "Sprinkler head", String::new(), "each", 1.0, 0.3);
    }
    if piece.starts_with("terminal:") {
        return mk(Kind::TrimOut, "Diffuser", String::new(), "each", 1.0, 0.5);
    }
    let fitting = piece.starts_with("fitting:");
    match kind {
        "duct" if fitting => mk(Kind::Duct, "Duct fitting", String::new(), "each", 1.0, 0.5),
        "duct" => mk(Kind::Duct, "Duct", String::new(), "ft", len, 0.25),
        "pipe" => {
            let k = if is_fire(e.system()) {
                Kind::Sprinkler
            } else if overhead {
                Kind::Plumbing
            } else {
                Kind::InWallPlumbing
            };
            if fitting { mk(k, "Pipe fitting", String::new(), "each", 1.0, 0.25) } else { mk(k, "Pipe", String::new(), "ft", len, 0.2) }
        }
        "conduit" | "tray" | "cable" => {
            let k = if overhead { Kind::Electrical } else { Kind::InWallElectrical };
            if fitting {
                mk(k, "EMT 90 elbow", inch_size(e.tag("size")), "each", 1.0, 0.12)
            } else if kind == "tray" {
                mk(k, "Cable tray", String::new(), "ft", len, 0.3)
            } else {
                mk(k, "EMT conduit", inch_size(e.tag("size")), "ft", len, 0.05)
            }
        }
        "light_fixture" => mk(Kind::TrimOut, "Light fixture", String::new(), "each", 1.0, 0.75),
        "outlet" | "receptacle" | "data_outlet" | "floor_box" | "switch" | "disconnect" | "junction_box" => {
            let item = if kind == "switch" { "Switch" } else { "Receptacle" };
            vec![
                Contribution { kind: Kind::InWallElectrical, item: "Box, 4\" square", size: String::new(), unit: "each", qty: 1.0, fallback: 0.15 },
                Contribution { kind: Kind::TrimOut, item, size: String::new(), unit: "each", qty: 1.0, fallback: 0.4 },
            ]
        }
        "panel" | "transformer" => mk(Kind::Electrical, "Panel", String::new(), "each", 1.0, 6.0),
        _ => {
            // Equipment: set with its trade's rough-in.
            let k = if is_fire(e.system()) {
                Kind::Sprinkler
            } else if matches!(piece.strip_prefix("equipment:"), Some("water_heater" | "pump")) {
                Kind::Plumbing
            } else {
                Kind::Duct
            };
            let item = match piece.strip_prefix("equipment:").unwrap_or_default() {
                "ahu" | "rtu" => "AHU",
                "exhaust_fan" | "fan" => "Exhaust fan unit",
                "water_heater" => "Water heater",
                _ => "Terminal",
            };
            mk(k, item, String::new(), "each", 1.0, 2.0)
        }
    }
}

// ── the cost book ────────────────────────────────────────────────────────────

/// Labor hours and material per unit by item and size.
struct Book {
    rows: HashMap<(String, String), (f64, f64)>,
}

impl Book {
    fn starter() -> Book {
        let mut rows = HashMap::new();
        for r in super::estimate::starter_prices() {
            let item = r["item"].as_str().unwrap_or_default().to_ascii_lowercase();
            let size = r["size"].as_str().unwrap_or_default().to_ascii_lowercase();
            let hours = r["labor_hours"].as_f64().filter(|v| v.is_finite() && *v >= 0.0);
            let material = r["material"].as_f64().filter(|v| v.is_finite()).unwrap_or(0.0);
            if let Some(h) = hours {
                rows.entry((item, size)).or_insert((h, material));
            }
        }
        Book { rows }
    }

    /// Hours and material per unit, and where they came from.
    fn rate(&self, item: &str, size: &str) -> Option<(f64, f64, String)> {
        let (i, s) = (item.to_ascii_lowercase(), size.to_ascii_lowercase());
        if let Some((h, m)) = self.rows.get(&(i.clone(), s.clone())) {
            return Some((*h, *m, format!("cost book: {item}{}", if size.is_empty() { String::new() } else { format!(" {size}") })));
        }
        if let Some((h, m)) = self.rows.get(&(i.clone(), String::new())) {
            return Some((*h, *m, format!("cost book: {item}")));
        }
        // A size the book doesn't list: the item's first size.
        let mut sized: Vec<(&(String, String), &(f64, f64))> = self.rows.iter().filter(|((it, _), _)| *it == i).collect();
        sized.sort_by(|a, b| a.0.1.cmp(&b.0.1));
        sized.first().map(|((_, sz), (h, m))| (*h, *m, format!("cost book: {item} {sz} (nearest listed size)")))
    }
}

// ── settings ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Settings {
    start: Option<i32>,
    hours_per_day: f64,
    workdays: u8,
    default_crew: f64,
    crew: BTreeMap<String, f64>,
    zone_sf: f64,
    rates: BTreeMap<String, f64>,
    overrides: BTreeMap<String, Override>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Override {
    crew: Option<f64>,
    days: Option<u32>,
    hours: Option<f64>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            start: None,
            hours_per_day: 8.0,
            workdays: 5,
            default_crew: 4.0,
            crew: BTreeMap::new(),
            zone_sf: 10_000.0,
            rates: BTreeMap::new(),
            overrides: BTreeMap::new(),
        }
    }
}

fn pos(v: &Value) -> Option<f64> {
    v.as_f64().filter(|v| v.is_finite() && *v > 0.0)
}

impl Settings {
    fn from_value(v: &Value) -> Settings {
        let mut s = Settings::default();
        let _ = s.apply(CMD_BUILD, v);
        s
    }

    /// Take the settings `p` gives; a bad value is an error.
    fn apply(&mut self, cmd: &str, p: &Value) -> Result<()> {
        if let Some(v) = p.get("start").filter(|v| !v.is_null()) {
            let t = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            self.start = Some(day(&t).ok_or_else(|| bad(cmd, format!("`start` `{t}` is not a date (2026-03-02)")))?);
        }
        if let Some(v) = p.get("hours_per_day").filter(|v| !v.is_null()) {
            self.hours_per_day = pos(v).filter(|h| *h <= 24.0).ok_or_else(|| bad(cmd, "`hours_per_day` is more than 0 and at most 24"))?;
        }
        if let Some(v) = p.get("workdays").filter(|v| !v.is_null()) {
            self.workdays =
                v.as_u64().filter(|d| (5..=7).contains(d)).and_then(|d| u8::try_from(d).ok()).ok_or_else(|| bad(cmd, "`workdays` is 5, 6 or 7"))?;
        }
        match p.get("crew") {
            None | Some(Value::Null) => {}
            Some(v @ Value::Number(_)) => {
                self.default_crew = pos(v).filter(|c| *c <= 1000.0).ok_or_else(|| bad(cmd, "`crew` is 1 to 1000 people"))?;
                self.crew.clear();
            }
            Some(Value::Object(m)) => {
                for (k, v) in m.iter().take(64) {
                    let n = pos(v).filter(|c| *c <= 1000.0).ok_or_else(|| bad(cmd, format!("`crew.{k}` is 1 to 1000 people")))?;
                    self.crew.insert(k.trim().to_ascii_lowercase(), n);
                }
            }
            Some(_) => return Err(bad(cmd, "`crew` is a number or {trade: people}")),
        }
        if let Some(v) = p.get("zone_sf").filter(|v| !v.is_null()) {
            self.zone_sf = pos(v).filter(|z| *z >= 100.0).ok_or_else(|| bad(cmd, "`zone_sf` is at least 100 square feet"))?;
        }
        match p.get("rates") {
            None | Some(Value::Null) => {}
            Some(Value::Object(m)) => {
                for (k, v) in m.iter().take(64) {
                    if Kind::from_code(k).is_none() {
                        return Err(bad(cmd, format!("`rates.{k}`: not an activity kind ({})", kind_codes())));
                    }
                    let r = v
                        .as_f64()
                        .filter(|r| r.is_finite() && *r >= 0.0 && *r <= 1000.0)
                        .ok_or_else(|| bad(cmd, format!("`rates.{k}` is hours per unit, 0 to 1000")))?;
                    self.rates.insert(k.trim().to_ascii_lowercase(), r);
                }
            }
            Some(_) => return Err(bad(cmd, "`rates` is {activity kind: hours per unit}")),
        }
        if let Some(Value::Object(m)) = p.get("overrides") {
            for (k, v) in m.iter().take(100_000) {
                let o = Override {
                    crew: v.get("crew").and_then(pos).filter(|c| *c <= 1000.0),
                    days: v.get("days").and_then(Value::as_u64).filter(|d| *d >= 1 && *d <= 5000).and_then(|d| u32::try_from(d).ok()),
                    hours: v.get("hours").and_then(Value::as_f64).filter(|h| h.is_finite() && *h >= 0.0 && *h <= 1e7),
                };
                if o != Override::default() {
                    self.overrides.insert(k.clone(), o);
                }
            }
        }
        Ok(())
    }

    fn to_value(&self) -> Value {
        let overrides: Map<String, Value> = self
            .overrides
            .iter()
            .map(|(k, o)| {
                let mut m = Map::new();
                if let Some(c) = o.crew {
                    m.insert("crew".into(), json!(c));
                }
                if let Some(d) = o.days {
                    m.insert("days".into(), json!(d));
                }
                if let Some(h) = o.hours {
                    m.insert("hours".into(), json!(h));
                }
                (k.clone(), Value::Object(m))
            })
            .collect();
        let mut v = json!({
            "hours_per_day": self.hours_per_day,
            "workdays": self.workdays,
            "zone_sf": self.zone_sf,
            "crew": if self.crew.is_empty() { json!(self.default_crew) } else { json!(self.crew) },
            "default_crew": self.default_crew,
            "rates": self.rates,
            "overrides": overrides,
        });
        if let Some(s) = self.start {
            v["start"] = json!(iso(s));
        }
        v
    }

    fn crew_for(&self, kind: Kind) -> f64 {
        self.crew.get(kind.code()).or_else(|| self.crew.get(&kind.trade().to_ascii_lowercase())).copied().unwrap_or(self.default_crew)
    }
}

fn kind_codes() -> String {
    Kind::ALL.iter().map(|k| k.code()).collect::<Vec<_>>().join(", ")
}

const CMD_BUILD: &str = "schedule.build";

fn today() -> i32 {
    let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    i32::try_from(secs / 86_400).unwrap_or(0)
}

// ── working days ─────────────────────────────────────────────────────────────

/// Day of the week, Monday 0 (1970-01-01 was a Thursday).
fn weekday(d: i32) -> i32 {
    (d + 3).rem_euclid(7)
}

fn working(d: i32, workdays: u8) -> bool {
    weekday(d) < i32::from(workdays)
}

/// The first working day on or after `d`.
fn on_or_after(d: i32, workdays: u8) -> i32 {
    let mut d = d;
    for _ in 0..7 {
        if working(d, workdays) {
            break;
        }
        d = d.saturating_add(1);
    }
    d
}

/// The working day `n` working days after `d` (a working day): `n` = 0 is `d`.
fn add_working(d: i32, n: i64, workdays: u8) -> i32 {
    let w = i64::from(workdays.clamp(5, 7));
    let n = n.clamp(0, MAX_DAYS);
    let mut d = d;
    let (weeks, rest) = (n / w, n % w);
    d = d.saturating_add(i32::try_from(weeks * 7).unwrap_or(i32::MAX / 2));
    for _ in 0..rest {
        d = on_or_after(d.saturating_add(1), workdays);
    }
    d
}

/// Working days from `a` to `b`, both counted.
fn working_between(a: i32, b: i32, workdays: u8) -> i64 {
    if b < a {
        return 0;
    }
    let span = i64::from(b) - i64::from(a) + 1;
    let (weeks, rest) = (span / 7, span % 7);
    let mut n = weeks * i64::from(workdays);
    for i in 0..rest {
        if working(a.saturating_add(i32::try_from(weeks * 7 + i).unwrap_or(0)), workdays) {
            n += 1;
        }
    }
    n
}

// ── building the sequence ────────────────────────────────────────────────────

/// One activity of the sequence.
#[derive(Clone, Debug)]
struct Activity {
    id: String,
    kind: Kind,
    level: String,
    zone: String,
    handles: Vec<Handle>,
    /// Per (item, size, unit): quantity, hours, material dollars.
    items: BTreeMap<(String, String, String), (f64, f64, f64)>,
    rate_sources: BTreeSet<String>,
    hours: f64,
    crew: f64,
    days: i64,
    preds: Vec<usize>,
    start: i32,
    finish: i32,
    phases: Vec<&'static str>,
}

impl Activity {
    fn name(&self) -> String {
        format!("{} — {} {}", self.kind.label(), self.level, self.zone)
    }

    /// The quantity in the unit carrying most of the hours.
    fn main_quantity(&self) -> (f64, String) {
        let mut by_unit: BTreeMap<&str, (f64, f64)> = BTreeMap::new();
        for ((_, _, unit), (q, h, _)) in &self.items {
            let e = by_unit.entry(unit.as_str()).or_default();
            e.0 += q;
            e.1 += h;
        }
        by_unit.iter().max_by(|a, b| a.1.1.total_cmp(&b.1.1)).map(|(u, (q, _))| (round2(*q), (*u).to_string())).unwrap_or((0.0, String::new()))
    }
}

fn round2(v: f64) -> f64 {
    // `+ 0.0` turns a negative zero into zero.
    (v * 100.0).round() / 100.0 + 0.0
}

/// The zones of a level: strips across its long side, each about `zone_sf`.
#[derive(Clone, Debug)]
struct Zones {
    bx: [f64; 4],
    n: usize,
    along_x: bool,
}

impl Zones {
    fn of(c: &Ctx, els: &[&El], zone_sf: f64) -> Zones {
        let mut bx = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        let main: Vec<&&El> = els.iter().filter(|e| matches!(e.part.as_str(), "wall" | "floor" | "slab")).collect();
        let use_all = main.is_empty();
        for e in els.iter().filter(|e| use_all || matches!(e.part.as_str(), "wall" | "floor" | "slab")) {
            bx = [bx[0].min(e.bx[0]), bx[1].min(e.bx[1]), bx[2].max(e.bx[2]), bx[3].max(e.bx[3])];
        }
        if !bx.iter().all(|v| v.is_finite()) {
            return Zones { bx: [0.0; 4], n: 1, along_x: true };
        }
        let area = sf(c, (bx[2] - bx[0]) * (bx[3] - bx[1]));
        let n = if area.is_finite() && zone_sf > 0.0 { (area / zone_sf).ceil() } else { 1.0 };
        let n = if n.is_finite() && n >= 1.0 { (n as usize).min(MAX_ZONES) } else { 1 };
        Zones { bx, n, along_x: bx[2] - bx[0] >= bx[3] - bx[1] }
    }

    fn index(&self, p: [f64; 2]) -> usize {
        if self.n <= 1 {
            return 0;
        }
        let (lo, hi, v) = if self.along_x { (self.bx[0], self.bx[2], p[0]) } else { (self.bx[1], self.bx[3], p[1]) };
        let span = hi - lo;
        if span.is_nan() || span <= 0.0 {
            return 0;
        }
        let f = ((v - lo) / span * self.n as f64).floor();
        if f.is_finite() && f > 0.0 { (f as usize).min(self.n - 1) } else { 0 }
    }

    fn name(i: usize) -> String {
        format!("Z{}", i + 1)
    }
}

fn zone_number(z: &str) -> usize {
    z.trim_start_matches('Z').parse().unwrap_or(0)
}

/// Which walls are outside walls: a point just off one face is outside the level's slab, or
/// (without a slab) the wall runs along the edge of the level; a wall type naming it decides
/// first.
fn exterior_walls(c: &Ctx, els: &[El], wall_types: &HashMap<String, bool>) -> BTreeSet<Handle> {
    let mut by_level: BTreeMap<String, Vec<&El>> = BTreeMap::new();
    for e in els {
        by_level.entry(e.level.to_ascii_lowercase()).or_default().push(e);
    }
    let mut out = BTreeSet::new();
    for (_, on) in by_level {
        let slabs: Vec<&Vec<[f64; 2]>> = on.iter().filter(|e| matches!(e.part.as_str(), "floor" | "slab")).flat_map(|e| e.tops.iter()).collect();
        let walls: Vec<&&El> = on.iter().filter(|e| e.part == "wall").collect();
        let mut ext = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for w in &walls {
            ext = [ext[0].min(w.bx[0]), ext[1].min(w.bx[1]), ext[2].max(w.bx[2]), ext[3].max(w.bx[3])];
        }
        for w in walls {
            let ty = w.tag("type").to_ascii_lowercase();
            if let Some(&x) = wall_types.get(&ty) {
                if x {
                    out.insert(w.handle);
                }
                continue;
            }
            if ty.contains("ext") {
                out.insert(w.handle);
                continue;
            }
            if ty.contains("int") || ty.contains("partition") || ty.contains("demis") {
                continue;
            }
            let off = w.thick / 2.0 + c.upf / 2.0;
            let cen = w.center();
            // The wall's direction from its box: the long side.
            let along_x = w.bx[2] - w.bx[0] >= w.bx[3] - w.bx[1];
            let probes = if along_x { [[cen[0], cen[1] - off], [cen[0], cen[1] + off]] } else { [[cen[0] - off, cen[1]], [cen[0] + off, cen[1]]] };
            let outside = if slabs.is_empty() {
                let tol = w.thick + c.upf / 6.0;
                w.bx[0] - ext[0] <= tol || ext[2] - w.bx[2] <= tol || w.bx[1] - ext[1] <= tol || ext[3] - w.bx[3] <= tol
            } else {
                probes.iter().any(|p| !slabs.iter().any(|s| inside(s, *p)))
            };
            if outside {
                out.insert(w.handle);
            }
        }
    }
    out
}

/// Wall types the spec marks outside or inside by their assembly or name.
fn wall_type_sides(sp: &cadaddy_walk3d::BuildingSpec) -> HashMap<String, bool> {
    let mut out = HashMap::new();
    if let Ok(Value::Array(types)) = serde_json::to_value(&sp.wall_types) {
        for t in types.iter().take(10_000) {
            let name = t["name"].as_str().unwrap_or_default().to_ascii_lowercase();
            let text = t.to_string().to_ascii_lowercase();
            if name.contains("ext") || text.contains("exterior sheathing") {
                out.insert(name, true);
            } else if name.contains("partition") || name.contains("int") {
                out.insert(name, false);
            }
        }
    }
    out
}

/// The sequence built from the model.
struct Sequence {
    acts: Vec<Activity>,
    levels: Vec<String>,
    not_sequenced: BTreeMap<String, usize>,
    notes: Vec<String>,
    start: i32,
    settings: Settings,
}

fn build(s: &Session, settings: Settings) -> Result<Sequence> {
    let (sp, _) = super::bspec::current(s, CMD_BUILD)?;
    let c = Ctx::new(&sp);
    let els = read_elements(s.doc()?);
    let mut notes = Vec::new();
    if els.is_empty() {
        notes.push("the drawing has no model elements: build the model first (model.build or bim.from_pdf)".to_string());
    }
    let ext = exterior_walls(&c, &els, &wall_type_sides(&sp));
    let book = Book::starter();
    let start = on_or_after(settings.start.unwrap_or_else(today), settings.workdays);

    // Levels in order of elevation, then any the spec doesn't list.
    let mut level_names: Vec<String> = c.levels.iter().map(|l| l.name.clone()).collect();
    for e in &els {
        if !level_names.iter().any(|l| l.eq_ignore_ascii_case(&e.level)) {
            level_names.push(e.level.clone());
        }
    }
    level_names.sort_by_key(|l| c.rank(l));
    let mut acts: Vec<Activity> = Vec::new();
    let mut index: HashMap<(usize, usize, Kind), usize> = HashMap::new();
    let mut not_sequenced: BTreeMap<String, usize> = BTreeMap::new();
    let mut zones_per_level = Vec::new();
    for (li, lname) in level_names.iter().enumerate() {
        let on: Vec<&El> = els.iter().filter(|e| e.level.eq_ignore_ascii_case(lname)).collect();
        let zones = Zones::of(&c, &on, settings.zone_sf);
        zones_per_level.push(zones.n);
        for e in on {
            let zi = zones.index(e.center());
            let parts = contributions(&c, e, ext.contains(&e.handle));
            if parts.is_empty() {
                *not_sequenced.entry(e.part.clone()).or_default() += 1;
                continue;
            }
            for p in parts {
                let ai = *index.entry((li, zi, p.kind)).or_insert_with(|| {
                    acts.push(Activity {
                        id: format!("{lname}/{}/{}", Zones::name(zi), p.kind.code()),
                        kind: p.kind,
                        level: lname.clone(),
                        zone: Zones::name(zi),
                        handles: Vec::new(),
                        items: BTreeMap::new(),
                        rate_sources: BTreeSet::new(),
                        hours: 0.0,
                        crew: 0.0,
                        days: 0,
                        preds: Vec::new(),
                        start,
                        finish: start,
                        phases: Vec::new(),
                    });
                    acts.len() - 1
                });
                let Some(a) = acts.get_mut(ai) else { continue };
                if a.handles.last() != Some(&e.handle) {
                    a.handles.push(e.handle);
                }
                let (hours_per, material_per, source) = match settings.rates.get(p.kind.code()) {
                    Some(r) => (*r, 0.0, format!("rate set for {}", p.kind.code())),
                    None => book.rate(p.item, &p.size).unwrap_or((p.fallback, 0.0, format!("default rate for {} (not in the cost book)", p.item))),
                };
                let qty = if p.qty.is_finite() && p.qty > 0.0 { p.qty } else { 0.0 };
                let slot = a.items.entry((p.item.to_string(), p.size.clone(), p.unit.to_string())).or_default();
                slot.0 += qty;
                slot.1 += qty * hours_per;
                slot.2 += qty * material_per;
                a.rate_sources.insert(source);
            }
        }
    }
    // An inspection before cover-up in every zone with rough-in.
    let mut with_rough: BTreeMap<(usize, usize), BTreeSet<&'static str>> = BTreeMap::new();
    for (&(li, zi, k), &ai) in &index {
        if k.rough_in() {
            let trade = acts.get(ai).map_or("", |a| a.kind.trade());
            with_rough.entry((li, zi)).or_default().insert(trade);
        }
    }
    for ((li, zi), trades) in with_rough {
        let lname = level_names.get(li).cloned().unwrap_or_default();
        let mut phases: Vec<&'static str> = Vec::new();
        for (id, _) in cadaddy_mepfix::covercheck::STANDARD_PHASES {
            let needed = match *id {
                "rough_elec" => trades.contains("Electrical"),
                "rough_mech" => trades.contains("Mechanical"),
                "rough_plumb" => trades.contains("Plumbing"),
                "rough_fa" | "firestop" => trades.contains("Fire Protection"),
                _ => false,
            };
            if needed {
                phases.push(id);
            }
        }
        acts.push(Activity {
            id: format!("{lname}/{}/{}", Zones::name(zi), Kind::Inspection.code()),
            kind: Kind::Inspection,
            level: lname,
            zone: Zones::name(zi),
            handles: Vec::new(),
            items: BTreeMap::new(),
            rate_sources: BTreeSet::from(["one working day per zone".to_string()]),
            hours: 0.0,
            crew: 0.0,
            days: 1,
            preds: Vec::new(),
            start,
            finish: start,
            phases,
        });
        index.insert((li, zi, Kind::Inspection), acts.len() - 1);
    }

    // Hours, crew and duration.
    for a in &mut acts {
        let o = settings.overrides.get(&a.id).cloned().unwrap_or_default();
        a.hours = o.hours.unwrap_or_else(|| a.items.values().map(|v| v.1).sum::<f64>().max(0.0));
        a.crew = o.crew.unwrap_or_else(|| settings.crew_for(a.kind));
        if a.kind == Kind::Inspection {
            a.crew = 0.0;
            a.days = i64::from(o.days.unwrap_or(1));
            continue;
        }
        let per_day = a.crew * settings.hours_per_day;
        let raw = if per_day > 0.0 { (a.hours / per_day).ceil() } else { 1.0 };
        let raw = if raw.is_finite() { raw.clamp(1.0, MAX_DAYS as f64) as i64 } else { 1 };
        a.days = o.days.map_or(raw, i64::from);
    }

    // Logic: each zone's activities in physical order; a level's slab after the structure
    // below; overhead rough-in after the deck above it; one crew per kind of work, zone after
    // zone, level after level.
    let key_of = |li: usize, zi: usize, k: Kind| index.get(&(li, zi, k)).copied();
    let mut preds: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); acts.len()];
    for li in 0..level_names.len() {
        let nz = zones_per_level.get(li).copied().unwrap_or(1);
        for zi in 0..nz {
            let mut prev: Option<usize> = None;
            for k in Kind::ALL {
                let Some(ai) = key_of(li, zi, k) else { continue };
                if let Some(p) = prev
                    && let Some(set) = preds.get_mut(ai)
                {
                    set.insert(p);
                }
                prev = Some(ai);
                if li > 0 && matches!(k, Kind::Slab) {
                    let below = li - 1;
                    for bz in 0..zones_per_level.get(below).copied().unwrap_or(1) {
                        let p = key_of(below, bz, Kind::Structure)
                            .or_else(|| key_of(below, bz, Kind::ExteriorWalls))
                            .or_else(|| key_of(below, bz, Kind::Slab));
                        if let (Some(p), Some(set)) = (p, preds.get_mut(ai)) {
                            set.insert(p);
                        }
                    }
                }
                if k.overhead() && li + 1 < level_names.len() {
                    let above = li + 1;
                    for az in 0..zones_per_level.get(above).copied().unwrap_or(1) {
                        if let (Some(p), Some(set)) = (key_of(above, az, Kind::Slab), preds.get_mut(ai)) {
                            set.insert(p);
                        }
                    }
                }
            }
        }
    }
    for k in Kind::ALL {
        if k == Kind::Inspection {
            continue;
        }
        let mut prev: Option<usize> = None;
        for li in 0..level_names.len() {
            for zi in 0..zones_per_level.get(li).copied().unwrap_or(1) {
                if let Some(ai) = key_of(li, zi, k) {
                    if let (Some(p), Some(set)) = (prev, preds.get_mut(ai)) {
                        set.insert(p);
                    }
                    prev = Some(ai);
                }
            }
        }
    }
    for (a, p) in acts.iter_mut().zip(&preds) {
        a.preds = p.iter().copied().collect();
    }
    schedule(&mut acts, start, settings.workdays, &mut notes);
    Ok(Sequence { acts, levels: level_names, not_sequenced, notes, start, settings })
}

/// Early dates: every activity starts the working day after its last predecessor finishes.
fn schedule(acts: &mut [Activity], start: i32, workdays: u8, notes: &mut Vec<String>) {
    let n = acts.len();
    let mut indeg: Vec<usize> = acts.iter().map(|a| a.preds.len()).collect();
    let mut next: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, a) in acts.iter().enumerate() {
        for &p in &a.preds {
            if let Some(v) = next.get_mut(p) {
                v.push(i);
            }
        }
    }
    let mut ready: std::collections::VecDeque<usize> = (0..n).filter(|&i| indeg.get(i) == Some(&0)).collect();
    let mut done = vec![false; n];
    let mut order = Vec::with_capacity(n);
    while let Some(i) = ready.pop_front() {
        order.push(i);
        if let Some(d) = done.get_mut(i) {
            *d = true;
        }
        for &j in next.get(i).map(Vec::as_slice).unwrap_or_default() {
            if let Some(d) = indeg.get_mut(j) {
                *d = d.saturating_sub(1);
                if *d == 0 {
                    ready.push_back(j);
                }
            }
        }
    }
    if order.len() < n {
        notes.push(format!("{} activities wait on each other in a loop: they start at the beginning", n - order.len()));
        order.extend((0..n).filter(|i| !done.get(*i).copied().unwrap_or(true)));
    }
    let mut finish: Vec<Option<i32>> = vec![None; n];
    for i in order {
        let Some(a) = acts.get(i) else { continue };
        let after = a.preds.iter().filter_map(|&p| finish.get(p).copied().flatten()).max();
        let s = on_or_after(after.map_or(start, |f| f.saturating_add(1)), workdays);
        let f = add_working(s, a.days.saturating_sub(1), workdays);
        if let Some(a) = acts.get_mut(i) {
            a.start = s;
            a.finish = f;
        }
        if let Some(slot) = finish.get_mut(i) {
            *slot = Some(f);
        }
    }
}

impl Sequence {
    /// The activities by start date, then level, zone and order of work.
    fn ordered(&self) -> Vec<&Activity> {
        let rank: HashMap<&str, usize> = self.levels.iter().enumerate().map(|(i, l)| (l.as_str(), i)).collect();
        let mut v: Vec<&Activity> = self.acts.iter().collect();
        v.sort_by(|a, b| {
            (a.start, rank.get(a.level.as_str()), zone_number(&a.zone), a.kind).cmp(&(
                b.start,
                rank.get(b.level.as_str()),
                zone_number(&b.zone),
                b.kind,
            ))
        });
        v
    }

    fn span(&self) -> Option<(i32, i32)> {
        let s = self.acts.iter().map(|a| a.start).min()?;
        let f = self.acts.iter().map(|a| a.finish).max()?;
        Some((s, f))
    }

    fn activity_json(&self, a: &Activity, handles: bool) -> Value {
        let (qty, unit) = a.main_quantity();
        let mut quantities: BTreeMap<&str, f64> = BTreeMap::new();
        for ((_, _, u), (q, _, _)) in &a.items {
            *quantities.entry(u.as_str()).or_default() += q;
        }
        let mut v = json!({
            "id": a.id,
            "name": a.name(),
            "level": a.level,
            "zone": a.zone,
            "kind": a.kind.code(),
            "trade": a.kind.trade(),
            "quantity": qty,
            "unit": unit,
            "quantities": quantities.iter().map(|(u, q)| (u.to_string(), json!(round2(*q)))).collect::<Map<String, Value>>(),
            "hours": round2(a.hours),
            "crew": a.crew,
            "days": a.days,
            "start": iso(a.start),
            "finish": iso(a.finish),
            "predecessors": a.preds.iter().filter_map(|&p| self.acts.get(p)).map(|p| p.id.clone()).collect::<Vec<_>>(),
            "elements": a.handles.len(),
            "rate": a.rate_sources.iter().take(20).collect::<Vec<_>>(),
        });
        if !a.phases.is_empty() {
            v["phases"] = json!(a.phases);
        }
        if handles {
            v["handles"] = a.handles.iter().map(|h| json!(h.hex())).collect();
        }
        v
    }

    fn to_schedule(&self) -> Schedule4d {
        Schedule4d {
            version: fourd::SCHEDULE_VERSION,
            tasks: self
                .acts
                .iter()
                .take(fourd::MAX_TASKS)
                .map(|a| Task {
                    id: a.id.chars().take(200).collect(),
                    name: a.name().chars().take(200).collect(),
                    start: iso(a.start),
                    finish: iso(a.finish),
                    filter: fourd::Filter::default(),
                    add: a.handles.iter().take(fourd::MAX_PICKS).map(|h| h.hex()).collect(),
                    remove: Vec::new(),
                })
                .collect(),
        }
    }

    fn summary(&self) -> Value {
        let mut by_trade: BTreeMap<&str, (usize, f64)> = BTreeMap::new();
        let mut by_level: BTreeMap<&str, (usize, f64, i32, i32)> = BTreeMap::new();
        for a in &self.acts {
            let t = by_trade.entry(a.kind.trade()).or_default();
            t.0 += 1;
            t.1 += a.hours;
            let l = by_level.entry(a.level.as_str()).or_insert((0, 0.0, i32::MAX, i32::MIN));
            l.0 += 1;
            l.1 += a.hours;
            l.2 = l.2.min(a.start);
            l.3 = l.3.max(a.finish);
        }
        let span = self.span();
        json!({
            "activities": self.acts.len(),
            "span": span.map(|(a, b)| json!([iso(a), iso(b)])),
            "working_days": span.map_or(0, |(a, b)| working_between(a, b, self.settings.workdays)),
            "hours": round2(self.acts.iter().map(|a| a.hours).sum()),
            "by_trade": by_trade.iter().map(|(t, (n, h))| json!({"trade": t, "activities": n, "hours": round2(*h)})).collect::<Vec<_>>(),
            "by_level": by_level.iter().map(|(l, (n, h, s, f))| json!({"level": l, "activities": n, "hours": round2(*h), "start": iso(*s), "finish": iso(*f)})).collect::<Vec<_>>(),
            "not_sequenced": self.not_sequenced,
            "notes": self.notes,
            "start": iso(self.start),
        })
    }
}

fn stored_settings(s: &Session, cmd: &str) -> Result<(cadaddy_walk3d::BuildingSpec, Settings)> {
    let (sp, _) = super::bspec::current(s, cmd)?;
    let st = Settings::from_value(&sp.sequence);
    Ok((sp, st))
}

fn store_settings(s: &mut Session, mut sp: cadaddy_walk3d::BuildingSpec, st: &Settings) -> Result<()> {
    sp.sequence = st.to_value();
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(())
}

/// The sequence with the stored settings (the start fixed once built).
fn current(s: &Session, cmd: &str) -> Result<Sequence> {
    let (_, st) = stored_settings(s, cmd)?;
    build(s, st)
}

fn run_build(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, mut st) = stored_settings(s, CMD_BUILD)?;
    st.apply(CMD_BUILD, p)?;
    let seq = build(s, st)?;
    let mut keep = seq.settings.clone();
    keep.start = Some(seq.start);
    store_settings(s, sp, &keep)?;
    if bool_or(p, "apply4d", true) {
        s.doc_mut()?.schedule4d = seq.to_schedule().to_json();
    }
    let mut out = seq.summary();
    out["list"] = seq.ordered().into_iter().take(MAX_LISTED).map(|a| seq.activity_json(a, false)).collect();
    out["applied4d"] = json!(bool_or(p, "apply4d", true));
    Ok(out)
}

fn filtered<'a>(seq: &'a Sequence, p: &Value) -> Vec<&'a Activity> {
    let level = str_param(p, "level");
    let trade = str_param(p, "trade");
    let zone = str_param(p, "zone");
    seq.ordered()
        .into_iter()
        .filter(|a| level.is_none_or(|l| a.level.eq_ignore_ascii_case(l)))
        .filter(|a| trade.is_none_or(|t| a.kind.trade().eq_ignore_ascii_case(t) || a.kind.code().eq_ignore_ascii_case(t)))
        .filter(|a| zone.is_none_or(|z| a.zone.eq_ignore_ascii_case(z)))
        .collect()
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let seq = current(s, "schedule.list")?;
    let handles = bool_or(p, "handles", false);
    let acts = filtered(&seq, p);
    Ok(json!({
        "activities": acts.len(),
        "span": seq.span().map(|(a, b)| json!([iso(a), iso(b)])),
        "list": acts.iter().take(MAX_LISTED).map(|a| seq.activity_json(a, handles)).collect::<Vec<_>>(),
    }))
}

fn run_edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "schedule.edit";
    let id = str_param(p, "activity").ok_or_else(|| bad(CMD, "`activity` is required"))?.trim().to_string();
    let (sp, mut st) = stored_settings(s, CMD)?;
    let before = build(s, st.clone())?;
    let Some(a) = before.acts.iter().find(|a| a.id.eq_ignore_ascii_case(&id)) else {
        return Err(bad(CMD, format!("no activity {id} (schedule.list)")));
    };
    let id = a.id.clone();
    let mut o = if bool_or(p, "clear", false) { Override::default() } else { st.overrides.get(&id).cloned().unwrap_or_default() };
    if let Some(v) = p.get("crew").filter(|v| !v.is_null()) {
        o.crew = Some(pos(v).filter(|c| *c <= 1000.0).ok_or_else(|| bad(CMD, "`crew` is 1 to 1000 people"))?);
    }
    if let Some(v) = p.get("days").filter(|v| !v.is_null()) {
        o.days = Some(
            v.as_u64()
                .filter(|d| (1..=5000).contains(d))
                .and_then(|d| u32::try_from(d).ok())
                .ok_or_else(|| bad(CMD, "`days` is 1 to 5000 working days"))?,
        );
    }
    if let Some(v) = p.get("hours").filter(|v| !v.is_null()) {
        o.hours = Some(v.as_f64().filter(|h| h.is_finite() && *h >= 0.0 && *h <= 1e7).ok_or_else(|| bad(CMD, "`hours` is 0 to 10,000,000"))?);
    }
    if o == Override::default() {
        st.overrides.remove(&id);
    } else {
        st.overrides.insert(id.clone(), o);
    }
    let after = build(s, st.clone())?;
    let old: HashMap<&str, (i32, i32)> = before.acts.iter().map(|a| (a.id.as_str(), (a.start, a.finish))).collect();
    let changed: Vec<&str> =
        after.acts.iter().filter(|a| old.get(a.id.as_str()) != Some(&(a.start, a.finish))).map(|a| a.id.as_str()).take(MAX_LISTED).collect();
    let mut keep = st;
    keep.start = Some(after.start);
    store_settings(s, sp, &keep)?;
    // A 4D schedule this sequence wrote follows the edit.
    let ours = fourd_from_sequence(s.doc()?, &before);
    if ours {
        s.doc_mut()?.schedule4d = after.to_schedule().to_json();
    }
    let act = after.acts.iter().find(|a| a.id == id).map(|a| after.activity_json(a, false)).unwrap_or(Value::Null);
    Ok(json!({"activity": act, "span": after.span().map(|(a, b)| json!([iso(a), iso(b)])), "changed": changed, "applied4d": ours}))
}

/// Whether the drawing's 4D schedule is the one `seq` writes (not an imported one).
fn fourd_from_sequence(d: &Drawing, seq: &Sequence) -> bool {
    let Ok(sc) = super::fourd::load(d) else { return false };
    !sc.tasks.is_empty() && sc.tasks.len() == seq.acts.len().min(fourd::MAX_TASKS) && sc.tasks.iter().zip(&seq.acts).all(|(t, a)| t.id == a.id)
}

fn run_view(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "schedule.view";
    let sc = super::fourd::load(s.doc()?).map_err(|e| bad(CMD, e))?;
    let Some((first, _)) = sc.span() else { return Err(bad(CMD, "the drawing has no 4D schedule: run schedule.build (or fourd.import)")) };
    let date = match (p.get("date").filter(|v| !v.is_null()), str_param(p, "activity")) {
        (Some(v), _) => {
            let t = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            day(&t).ok_or_else(|| bad(CMD, format!("`date` `{t}` is not a date (2026-03-09)")))?
        }
        (None, Some(id)) => {
            let ti = sc.find(id).ok_or_else(|| bad(CMD, format!("no activity {id}")))?;
            // The day after its finish: the model with that work in place.
            sc.tasks
                .get(ti)
                .and_then(Task::finish_day)
                .map(|f| f.saturating_add(1))
                .ok_or_else(|| bad(CMD, format!("activity {id} has no finish date")))?
        }
        (None, None) => first,
    };
    let doc = s.doc()?;
    let els = super::fourd::elements(doc);
    let links = sc.links(&els);
    let counts = sc.counts(&links, date);
    let in_progress: Vec<&str> =
        sc.tasks.iter().filter(|t| t.phase(date) == fourd::Phase::InProgress).map(|t| t.id.as_str()).take(MAX_LISTED).collect();
    let finished = sc.tasks.iter().filter(|t| t.phase(date) == fourd::Phase::Built).count();
    let mut upcoming: Vec<&Task> = sc.tasks.iter().filter(|t| t.phase(date) == fourd::Phase::NotYet).collect();
    upcoming.sort_by_key(|t| t.start_day());
    let mut out = json!({
        "date": iso(date),
        "counts": counts,
        "in_progress": in_progress,
        "finished": finished,
        "upcoming": upcoming.iter().take(10).map(|t| json!({"id": t.id, "start": t.start})).collect::<Vec<_>>(),
    });
    if bool_or(p, "handles", false) {
        let ph = sc.phases(&links, date);
        for (key, want) in [("built", fourd::Phase::Built), ("inProgressHandles", fourd::Phase::InProgress)] {
            out[key] = els.iter().zip(&ph).filter(|(_, p)| **p == Some(want)).map(|(e, _)| json!(e.handle.hex())).take(10_000).collect();
        }
    }
    if let Some(png) = str_param(p, "png") {
        let side = |k: &str, dflt: u32| -> Result<u32> {
            let v = p.get(k).and_then(Value::as_u64).unwrap_or(u64::from(dflt));
            u32::try_from(v).ok().filter(|v| (16..=8192).contains(v)).ok_or_else(|| bad(CMD, format!("`{k}` is 16 to 8192 pixels")))
        };
        let (w, h) = (side("width", 1280)?, side("height", 720)?);
        let view = str_param(p, "view").unwrap_or("swiso");
        let dir = cadaddy_geom::view3::preset(view).ok_or_else(|| bad(CMD, format!("unknown view {view} (swiso, seiso, front…)")))?;
        let (st, _) = super::fourd::staged(doc, date, bool_or(p, "ghost", false)).map_err(|e| bad(CMD, e))?;
        let bytes = cadaddy_render::shade::render_fitted(
            &st,
            cadaddy_geom::view3::Projection::new(dir),
            cadaddy_render::shade::VisualStyle::Shaded,
            w,
            h,
            cadaddy_color::Rgb(255, 255, 255),
        )
        .and_then(|r| r.encode_png())
        .ok_or_else(|| bad(CMD, "the model could not be drawn"))?;
        write_file(CMD, png, &bytes)?;
        out["png"] = json!(png);
    }
    Ok(out)
}

fn write_file(cmd: &str, path: &str, bytes: &[u8]) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        crate::writefile::write(path, bytes).map_err(|e| bad(cmd, format!("{path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = bytes;
        Err(bad(cmd, format!("{path}: writing files is not available in the browser")))
    }
}

// ── export ───────────────────────────────────────────────────────────────────

const COLUMNS: [&str; 15] =
    ["id", "activity", "level", "zone", "trade", "kind", "quantity", "unit", "hours", "crew", "days", "start", "finish", "predecessors", "elements"];

fn rows(seq: &Sequence) -> Vec<Vec<String>> {
    let mut out = vec![COLUMNS.iter().map(|c| c.to_string()).collect::<Vec<_>>()];
    for a in &seq.acts {
        let (q, u) = a.main_quantity();
        let preds: Vec<&str> = a.preds.iter().filter_map(|&p| seq.acts.get(p)).map(|p| p.id.as_str()).collect();
        out.push(vec![
            a.id.clone(),
            a.name(),
            a.level.clone(),
            a.zone.clone(),
            a.kind.trade().to_string(),
            a.kind.code().to_string(),
            format!("{q}"),
            u,
            format!("{}", round2(a.hours)),
            format!("{}", a.crew),
            a.days.to_string(),
            iso(a.start),
            iso(a.finish),
            preds.join("; "),
            a.handles.len().to_string(),
        ]);
    }
    out
}

fn csv_cell(v: &str) -> String {
    if v.contains([',', '"', '\n', '\r']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.to_string() }
}

fn xml_text(v: &str) -> String {
    v.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// MS Project XML: tasks with their dates, working duration and finish-to-start predecessors.
fn msproject(seq: &Sequence) -> String {
    let mut x = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Project xmlns=\"http://schemas.microsoft.com/project\">\n");
    x.push_str("  <Name>Construction Sequence</Name>\n");
    if let Some((s, f)) = seq.span() {
        x.push_str(&format!("  <StartDate>{}T08:00:00</StartDate>\n  <FinishDate>{}T17:00:00</FinishDate>\n", iso(s), iso(f)));
    }
    x.push_str("  <Tasks>\n");
    for (i, a) in seq.acts.iter().enumerate() {
        let uid = i + 1;
        let hours = (a.days.max(1) as f64 * seq.settings.hours_per_day).round() as i64;
        x.push_str(&format!(
            "    <Task>\n      <UID>{uid}</UID>\n      <ID>{uid}</ID>\n      <Name>{}</Name>\n      <WBS>{}</WBS>\n      <Start>{}T08:00:00</Start>\n      <Finish>{}T17:00:00</Finish>\n      <Duration>PT{hours}H0M0S</Duration>\n      <Work>PT{}H0M0S</Work>\n      <Notes>{}</Notes>\n",
            xml_text(&a.name()),
            xml_text(&a.id),
            iso(a.start),
            iso(a.finish),
            a.hours.round() as i64,
            xml_text(&format!("{}; {} elements", a.kind.trade(), a.handles.len())),
        ));
        for &p in &a.preds {
            x.push_str(&format!(
                "      <PredecessorLink>\n        <PredecessorUID>{}</PredecessorUID>\n        <Type>1</Type>\n      </PredecessorLink>\n",
                p + 1
            ));
        }
        x.push_str("    </Task>\n");
    }
    x.push_str("  </Tasks>\n</Project>\n");
    x
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "schedule.export";
    let path = str_param(p, "path").ok_or_else(|| bad(CMD, "`path` is required"))?;
    let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let format = match str_param(p, "format").map(str::to_ascii_lowercase).as_deref().unwrap_or(ext.as_str()) {
        "csv" => "csv",
        "xlsx" => "xlsx",
        "xml" | "msproject" | "mspdi" => "msproject",
        other => return Err(bad(CMD, format!("`{other}`: the sequence exports to .csv, .xlsx or .xml (MS Project)"))),
    };
    let seq = current(s, CMD)?;
    if seq.acts.is_empty() {
        return Err(bad(CMD, "the sequence has no activities: build the model first"));
    }
    let bytes = match format {
        "csv" => rows(&seq).iter().map(|r| r.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join("\n").into_bytes(),
        "xlsx" => super::table::xlsx::write(&rows(&seq), "Sequence").map_err(|e| bad(CMD, e))?,
        _ => msproject(&seq).into_bytes(),
    };
    write_file(CMD, path, &bytes)?;
    Ok(json!({"path": path, "activities": seq.acts.len(), "format": format}))
}

// ── the takeoff by activity ──────────────────────────────────────────────────

fn run_takeoff(s: &mut Session, p: &Value) -> Result<Value> {
    let seq = current(s, "schedule.takeoff")?;
    let acts = filtered(&seq, p);
    let (mut hours, mut material) = (0.0, 0.0);
    let list: Vec<Value> = acts
        .iter()
        .take(MAX_LISTED)
        .map(|a| {
            let m: f64 = a.items.values().map(|v| v.2).sum();
            hours += a.hours;
            material += m;
            json!({
                "id": a.id, "name": a.name(), "level": a.level, "zone": a.zone, "trade": a.kind.trade(), "kind": a.kind.code(),
                "hours": round2(a.hours), "material": round2(m), "start": iso(a.start), "finish": iso(a.finish),
                "items": a.items.iter().map(|((item, size, unit), (q, h, m))| json!({"item": item, "size": size, "unit": unit, "qty": round2(*q), "labor_hours": round2(*h), "material": round2(*m)})).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({"activities": list, "totals": {"hours": round2(hours), "material": round2(material)}, "source": super::estimate::STARTER_SOURCE}))
}

// ── the ceiling space ────────────────────────────────────────────────────────

/// One area's ceiling space.
#[derive(Clone, Debug)]
struct Fit {
    area: String,
    number: String,
    level: String,
    bx: [f64; 4],
    available_in: f64,
    required_in: f64,
    at: [f64; 2],
    governing: Vec<Handle>,
    modeled_overlap: bool,
}

impl Fit {
    fn margin(&self) -> f64 {
        self.available_in - self.required_in
    }
}

fn fits(c: &Ctx, els: &[El], level: Option<&str>, clearance_in: f64) -> Vec<Fit> {
    let mut out = Vec::new();
    let mut by_level: BTreeMap<String, Vec<&El>> = BTreeMap::new();
    for e in els {
        by_level.entry(e.level.clone()).or_default().push(e);
    }
    let structural = |e: &El| matches!(e.part.as_str(), "floor" | "slab" | "roof" | "beam" | "joist");
    for (lname, on) in &by_level {
        if level.is_some_and(|l| !l.eq_ignore_ascii_case(lname)) {
            continue;
        }
        let Some(lv) = c.level(lname) else { continue };
        let runs: Vec<&&El> = on
            .iter()
            .filter(|e| e.part == "mep" && matches!(e.kind(), "duct" | "pipe" | "conduit" | "tray" | "cable" | "equipment") && c.overhead(e))
            .collect();
        let mut areas: Vec<(String, String, [f64; 4], Vec<Vec<[f64; 2]>>)> = on
            .iter()
            .filter(|e| e.part == "room")
            .map(|e| {
                (
                    if e.tag("name").is_empty() { "Room".to_string() } else { e.tag("name").to_string() },
                    e.tag("number").to_string(),
                    e.bx,
                    e.tops.clone(),
                )
            })
            .collect();
        if areas.is_empty() && !runs.is_empty() {
            let mut bx = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            for e in on {
                bx = [bx[0].min(e.bx[0]), bx[1].min(e.bx[1]), bx[2].max(e.bx[2]), bx[3].max(e.bx[3])];
            }
            areas.push((format!("{lname} (no rooms)"), String::new(), bx, Vec::new()));
        }
        // The deck above: the next level's slab (or this level's top).
        let rank = c.rank(lname);
        let above = c.levels.get(rank + 1);
        let deck_default = match above {
            Some(a) => {
                let slab = els
                    .iter()
                    .filter(|e| e.level.eq_ignore_ascii_case(&a.name) && matches!(e.part.as_str(), "floor" | "slab"))
                    .map(|e| e.z[1] - e.z[0])
                    .fold(0.0, f64::max);
                a.elev - if slab > 0.0 { slab } else { c.upf / 2.0 }
            }
            None => lv.top,
        };
        for (name, number, bx, tops) in areas {
            let in_area = |p: [f64; 2]| contains(bx, p) && (tops.is_empty() || tops.iter().any(|t| inside(t, p)));
            let center = [(bx[0] + bx[2]) / 2.0, (bx[1] + bx[3]) / 2.0];
            let ceiling_top = on
                .iter()
                .filter(|e| matches!(e.part.as_str(), "ceiling" | "soffit") && contains(grow(e.bx, 1.0), center))
                .map(|e| e.z[1])
                .fold(f64::NEG_INFINITY, f64::max);
            let ceiling_top = if ceiling_top.is_finite() { ceiling_top } else { lv.ceiling };
            let deck = els
                .iter()
                .filter(|e| structural(e) && e.z[0] > ceiling_top && overlaps(e.bx, bx) && !e.level.eq_ignore_ascii_case(lname))
                .map(|e| e.z[0])
                .fold(f64::INFINITY, f64::min);
            let deck = if deck.is_finite() { deck.min(deck_default.max(ceiling_top)) } else { deck_default };
            let here: Vec<&&&El> = runs.iter().filter(|e| overlaps(e.bx, bx)).collect();
            if here.is_empty() {
                continue;
            }
            // Sample points: each run's middle, and where two runs cross.
            let mut samples: Vec<[f64; 2]> = Vec::new();
            for (i, a) in here.iter().enumerate() {
                let clip = [a.bx[0].max(bx[0]), a.bx[1].max(bx[1]), a.bx[2].min(bx[2]), a.bx[3].min(bx[3])];
                samples.push([(clip[0] + clip[2]) / 2.0, (clip[1] + clip[3]) / 2.0]);
                for b in here.iter().skip(i + 1) {
                    if samples.len() >= MAX_SAMPLES {
                        break;
                    }
                    if overlaps(a.bx, b.bx) {
                        let r = [a.bx[0].max(b.bx[0]), a.bx[1].max(b.bx[1]), a.bx[2].min(b.bx[2]), a.bx[3].min(b.bx[3])];
                        samples.push([(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0]);
                    }
                }
            }
            let mut best: Option<(f64, [f64; 2], Vec<&El>)> = None;
            for q in samples.into_iter().filter(|q| in_area(*q)) {
                let stack: Vec<&El> = here.iter().filter(|e| contains(grow(e.bx, c.upf / 24.0), q)).map(|e| ***e).collect();
                let req: f64 = stack.iter().map(|e| c.inches(e.z[1] - e.z[0]) + clearance_in).sum();
                if best.as_ref().is_none_or(|b| req > b.0) {
                    best = Some((req, q, stack));
                }
            }
            let Some((required, at, stack)) = best else { continue };
            let mut overlap = false;
            for (i, a) in stack.iter().enumerate() {
                for b in stack.iter().skip(i + 1) {
                    if a.z[0] < b.z[1] && b.z[0] < a.z[1] {
                        overlap = true;
                    }
                }
            }
            out.push(Fit {
                area: name,
                number,
                level: lname.clone(),
                bx,
                available_in: round2(c.inches(deck - ceiling_top).max(0.0)),
                required_in: round2(required),
                at,
                governing: stack.iter().map(|e| e.handle).collect(),
                modeled_overlap: overlap,
            });
        }
    }
    out
}

fn run_fit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "schedule.fit";
    let clearance = f64_or(p, "clearance_in", 1.0);
    let tight = f64_or(p, "tight_in", 6.0);
    if !(0.0..=24.0).contains(&clearance) || !(0.0..=120.0).contains(&tight) {
        return Err(bad(CMD, "`clearance_in` is 0 to 24 and `tight_in` 0 to 120 inches"));
    }
    let (sp, _) = super::bspec::current(s, CMD)?;
    let c = Ctx::new(&sp);
    let els = read_elements(s.doc()?);
    let by_handle: HashMap<Handle, &El> = els.iter().map(|e| (e.handle, e)).collect();
    let all = fits(&c, &els, str_param(p, "level"), clearance);
    let mut findings = Vec::new();
    let areas: Vec<Value> = all
        .iter()
        .map(|f| {
            let elements: Vec<Value> = f
                .governing
                .iter()
                .filter_map(|h| by_handle.get(h))
                .map(|e| json!({"handle": e.handle.hex(), "system": e.system(), "kind": e.kind(), "size": e.tag("size"), "depth_in": round2(c.inches(e.z[1] - e.z[0]))}))
                .collect();
            let fits = f.margin() >= 0.0;
            if !fits {
                findings.push(json!({
                    "area": f.area, "number": f.number, "level": f.level,
                    "message": format!("{} on {}: {:.1}\" of runs stack where {:.1}\" is free between the structure and the ceiling ({:.1}\" short)", f.area, f.level, f.required_in, f.available_in, -f.margin()),
                    "elements": f.governing.iter().map(|h| h.hex()).collect::<Vec<_>>(),
                }));
            }
            json!({
                "area": f.area, "number": f.number, "level": f.level,
                "available_in": f.available_in, "required_in": f.required_in, "margin_in": round2(f.margin()),
                "fits": fits, "tight": f.margin() < tight,
                "governing": {"at": [round2(f.at[0]), round2(f.at[1])], "elements": elements},
                "modeled_overlap": f.modeled_overlap,
            })
        })
        .collect();
    let ok = findings.is_empty();
    Ok(json!({"areas": areas, "findings": findings, "ok": ok}))
}

// ── conflicts in time and space ──────────────────────────────────────────────

/// A task of the drawing's 4D schedule as the checks see it.
struct T<'a> {
    task: &'a Task,
    start: i32,
    finish: i32,
    els: Vec<usize>,
}

/// Where a task is (a level and zone the sequence wrote into its id, else a level its name
/// names).
fn task_place<'a>(t: &Task, levels: &'a [String]) -> (Option<&'a String>, Option<String>) {
    let mut parts = t.id.split('/');
    if let (Some(l), Some(z), Some(_)) = (parts.next(), parts.next(), parts.next())
        && let Some(level) = levels.iter().find(|x| x.eq_ignore_ascii_case(l))
    {
        return (Some(level), Some(z.to_string()));
    }
    let name = t.name.to_ascii_lowercase();
    (levels.iter().filter(|l| name.contains(&l.to_ascii_lowercase())).max_by_key(|l| l.len()), None)
}

fn words(t: &Task, keys: &[&str]) -> bool {
    let n = format!("{} {}", t.name, t.id).to_ascii_lowercase();
    keys.iter().any(|k| n.contains(k))
}

struct Finding {
    kind: &'static str,
    severity: &'static str,
    level: String,
    zone: String,
    tasks: Vec<String>,
    trades: Vec<String>,
    dates: Option<(i32, i32)>,
    elements: Vec<Handle>,
    count: usize,
    message: String,
    phases: Vec<&'static str>,
}

impl Finding {
    fn json(&self) -> Value {
        let mut v = json!({
            "kind": self.kind, "severity": self.severity, "level": self.level, "zone": self.zone,
            "tasks": self.tasks, "elements": self.elements.iter().take(FINDING_ELEMENTS).map(|h| h.hex()).collect::<Vec<_>>(),
            "count": self.count, "message": self.message,
        });
        if !self.trades.is_empty() {
            v["trades"] = json!(self.trades);
        }
        if let Some((a, b)) = self.dates {
            v["dates"] = json!([iso(a), iso(b)]);
        }
        if !self.phases.is_empty() {
            v["phases"] = json!(self.phases);
        }
        v
    }
}

type Groups = BTreeMap<(&'static str, String, usize, usize), Finding>;

/// Count an element pair into the finding of its kind, level and pair of tasks.
fn add_to(
    groups: &mut Groups,
    kind: &'static str,
    severity: &'static str,
    tasks: (usize, usize),
    e: &El,
    other: Option<&El>,
    build: impl FnOnce() -> Finding,
) {
    let g = groups.entry((kind, e.level.to_ascii_lowercase(), tasks.0, tasks.1)).or_insert_with(build);
    g.count += 1;
    if severity == "high" {
        g.severity = "high";
    }
    for h in std::iter::once(e.handle).chain(other.map(|o| o.handle)) {
        if g.elements.len() < FINDING_ELEMENTS && !g.elements.contains(&h) {
            g.elements.push(h);
        }
    }
}

fn run_conflicts(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "schedule.conflicts";
    let tight = f64_or(p, "tight_in", 6.0);
    if !(0.0..=120.0).contains(&tight) {
        return Err(bad(CMD, "`tight_in` is 0 to 120 inches"));
    }
    let only = str_param(p, "level");
    let doc = s.doc()?;
    let sc = super::fourd::load(doc).map_err(|e| bad(CMD, e))?;
    if sc.tasks.is_empty() {
        return Err(bad(CMD, "the drawing has no 4D schedule: run schedule.build (or fourd.import)"));
    }
    let (sp, _) = super::bspec::current(s, CMD)?;
    let c = Ctx::new(&sp);
    let (_, st) = stored_settings(s, CMD)?;
    let doc = s.doc()?;
    let els = read_elements(doc);
    let fels = super::fourd::elements(doc);
    let links = sc.links(&fels);
    let at: HashMap<Handle, usize> = els.iter().enumerate().map(|(i, e)| (e.handle, i)).collect();
    // Per element, the tasks it follows; per task, its elements (indices into `els`).
    let mut tasks: Vec<T> = Vec::new();
    let mut of_el: Vec<Vec<usize>> = vec![Vec::new(); els.len()];
    for (ti, t) in sc.tasks.iter().enumerate() {
        let (Some(start), Some(finish)) = (t.start_day(), t.finish_day()) else { continue };
        let mine: Vec<usize> = links
            .tasks
            .get(ti)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|&fi| fels.get(fi))
            .filter_map(|fe| at.get(&fe.handle).copied())
            .collect();
        let k = tasks.len();
        for &i in &mine {
            if let Some(v) = of_el.get_mut(i) {
                v.push(k);
            }
        }
        tasks.push(T { task: t, start, finish, els: mine });
    }
    let first_start = |i: usize| of_el.get(i).and_then(|ts| ts.iter().filter_map(|&k| tasks.get(k)).map(|t| t.start).min());
    let first_task = |i: usize| {
        of_el.get(i).and_then(|ts| ts.iter().filter_map(|&k| tasks.get(k).map(|t| (k, t))).min_by_key(|(_, t)| (t.start, t.finish)).map(|(k, _)| k))
    };
    let level_ok = |e: &El| only.is_none_or(|l| e.level.eq_ignore_ascii_case(l));
    // Zones by level, as the sequence splits them.
    let mut zones: HashMap<String, Zones> = HashMap::new();
    {
        let mut by_level: BTreeMap<String, Vec<&El>> = BTreeMap::new();
        for e in &els {
            by_level.entry(e.level.to_ascii_lowercase()).or_default().push(e);
        }
        for (l, on) in by_level {
            zones.insert(l, Zones::of(&c, &on, st.zone_sf));
        }
    }
    let zone_of = |e: &El| zones.get(&e.level.to_ascii_lowercase()).map_or_else(|| Zones::name(0), |z| Zones::name(z.index(e.center())));
    let fit_areas = fits(&c, &els, only, 1.0);
    let tight_at = |e: &El| fit_areas.iter().any(|f| f.level.eq_ignore_ascii_case(&e.level) && overlaps(f.bx, e.bx) && f.margin() < tight);

    let mut groups: Groups = BTreeMap::new();
    // Ceiling-space elements, swept by x for pairs that share space.
    let infl = c.upf / 2.0;
    let mut ceil: Vec<usize> = (0..els.len())
        .filter(|&i| {
            els.get(i).is_some_and(|e| {
                level_ok(e) && el_trade(e).is_some() && c.overhead(e) && !surface_device(e) && !of_el.get(i).is_none_or(Vec::is_empty)
            })
        })
        .collect();
    ceil.sort_by(|&a, &b| els.get(a).map_or(0.0, |e| e.bx[0]).total_cmp(&els.get(b).map_or(0.0, |e| e.bx[0])));
    let mut pairs = 0usize;
    'sweep: for (n, &i) in ceil.iter().enumerate() {
        let Some(a) = els.get(i) else { continue };
        for &j in ceil.iter().skip(n + 1) {
            let Some(b) = els.get(j) else { continue };
            if b.bx[0] > a.bx[2] + 2.0 * infl {
                break;
            }
            pairs += 1;
            if pairs > MAX_PAIRS {
                break 'sweep;
            }
            if !a.level.eq_ignore_ascii_case(&b.level) || !overlaps(grow(a.bx, infl), grow(b.bx, infl)) {
                continue;
            }
            let (Some(ta), Some(tb)) = (el_trade(a), el_trade(b)) else { continue };
            if ta == tb {
                continue;
            }
            // Same space, same time, different trades.
            for &ka in of_el.get(i).map(Vec::as_slice).unwrap_or_default() {
                for &kb in of_el.get(j).map(Vec::as_slice).unwrap_or_default() {
                    let (Some(x), Some(y)) = (tasks.get(ka), tasks.get(kb)) else { continue };
                    if ka == kb || x.finish < y.start || y.finish < x.start {
                        continue;
                    }
                    let (lo, hi) = (x.start.max(y.start), x.finish.min(y.finish));
                    let (k1, k2) = (ka.min(kb), ka.max(kb));
                    let (t1, t2) = if ka < kb { (ta, tb) } else { (tb, ta) };
                    add_to(&mut groups, "trade_overlap", "medium", (k1, k2), a, Some(b), || Finding {
                        kind: "trade_overlap",
                        severity: "medium",
                        level: a.level.clone(),
                        zone: zone_of(a),
                        tasks: vec![
                            tasks.get(k1).map_or(String::new(), |t| t.task.id.clone()),
                            tasks.get(k2).map_or(String::new(), |t| t.task.id.clone()),
                        ],
                        trades: vec![t1.to_string(), t2.to_string()],
                        dates: Some((lo, hi)),
                        elements: Vec::new(),
                        count: 0,
                        message: format!("{t1} and {t2} work in the same ceiling space on {} from {} to {}", a.level, iso(lo), iso(hi)),
                        phases: Vec::new(),
                    });
                }
            }
            // Overhead order: duct, sprinkler, plumbing, electrical.
            let (Some(ra), Some(rb)) = (overhead_rank(ta), overhead_rank(tb)) else { continue };
            let (lo_el, hi_el, lo_i, hi_i) = if ra < rb { (a, b, i, j) } else { (b, a, j, i) };
            let (Some(s_lo), Some(s_hi)) = (first_start(lo_i), first_start(hi_i)) else { continue };
            if s_hi < s_lo {
                let (Some(k_lo), Some(k_hi)) = (first_task(lo_i), first_task(hi_i)) else { continue };
                let tight_here = tight_at(lo_el) || tight_at(hi_el);
                let (t_lo, t_hi) = (el_trade(lo_el).unwrap_or_default(), el_trade(hi_el).unwrap_or_default());
                let sev = if tight_here { "high" } else { "medium" };
                add_to(&mut groups, "overhead_order", sev, (k_hi, k_lo), hi_el, Some(lo_el), || Finding {
                    kind: "overhead_order",
                    severity: sev,
                    level: hi_el.level.clone(),
                    zone: zone_of(hi_el),
                    tasks: vec![
                        tasks.get(k_hi).map_or(String::new(), |t| t.task.id.clone()),
                        tasks.get(k_lo).map_or(String::new(), |t| t.task.id.clone()),
                    ],
                    trades: vec![t_hi.to_string(), t_lo.to_string()],
                    dates: Some((s_hi, s_lo)),
                    elements: Vec::new(),
                    count: 0,
                    message: format!(
                        "{t_hi} is planned before {t_lo} in the same ceiling space on {}{} (overhead order: duct, sprinkler, plumbing, electrical)",
                        hi_el.level,
                        if tight_here { ", where the ceiling space is tight" } else { "" }
                    ),
                    phases: Vec::new(),
                });
            }
        }
    }

    // Cover-up: drywall and insulation close the walls, the ceiling grid the ceiling space.
    let level_names: Vec<String> = c.levels.iter().map(|l| l.name.clone()).collect();
    let inspections: Vec<(usize, Option<&String>, Option<String>)> = tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| words(t.task, &["inspect"]))
        .map(|(k, t)| {
            let (l, z) = task_place(t.task, &level_names);
            (k, l, z)
        })
        .collect();
    for (kc, cover) in tasks.iter().enumerate() {
        let walls = words(cover.task, &["drywall", "gyp", "board", "insulat", "sheetrock", "close-in", "cover"]);
        let ceiling = words(cover.task, &["ceiling", "grid", "tile"]) && !walls;
        if !walls && !ceiling {
            continue;
        }
        let covered: Vec<&El> = cover.els.iter().filter_map(|&i| els.get(i)).filter(|e| level_ok(e)).collect();
        if covered.is_empty() {
            continue;
        }
        let (_, cover_zone) = task_place(cover.task, &level_names);
        // Rough-in tasks with work in that space.
        let mut needs: BTreeSet<&'static str> = BTreeSet::new();
        let mut rough: BTreeMap<usize, Vec<Handle>> = BTreeMap::new();
        // An element's first task roughs it in (a device's later trim-out comes after the cover).
        for (i, e) in els.iter().enumerate() {
            let Some(trade) = el_trade(e) else { continue };
            let Some(kr) = first_task(i).filter(|k| *k != kc) else { continue };
            {
                let in_space = if ceiling { c.overhead(e) && !surface_device(e) } else { !c.overhead(e) && !c.at_ceiling(e) };
                if !in_space || !covered.iter().any(|w| w.level.eq_ignore_ascii_case(&e.level) && overlaps(grow(w.bx, infl), e.bx)) {
                    continue;
                }
                let v = rough.entry(kr).or_default();
                if v.len() < FINDING_ELEMENTS {
                    v.push(e.handle);
                }
                needs.insert(match trade {
                    "Electrical" => "rough_elec",
                    "Plumbing" => "rough_plumb",
                    "Fire Protection" => "firestop",
                    _ => "rough_mech",
                });
            }
        }
        let lvl = covered.first().map(|e| e.level.clone()).unwrap_or_default();
        let zone = cover_zone.clone().unwrap_or_else(|| covered.first().map(|e| zone_of(e)).unwrap_or_default());
        let space = if ceiling { "ceiling" } else { "walls" };
        let mut last_rough: Option<i32> = None;
        for (&kr, hs) in &rough {
            let Some(r) = tasks.get(kr) else { continue };
            last_rough = Some(last_rough.map_or(r.finish, |l: i32| l.max(r.finish)));
            if cover.start <= r.finish {
                let g = groups.entry(("covered_before_roughin", lvl.to_ascii_lowercase(), kc, kr)).or_insert_with(|| Finding {
                    kind: "covered_before_roughin",
                    severity: "high",
                    level: lvl.clone(),
                    zone: zone.clone(),
                    tasks: vec![cover.task.id.clone(), r.task.id.clone()],
                    trades: Vec::new(),
                    dates: Some((cover.start, r.finish)),
                    elements: hs.clone(),
                    count: hs.len(),
                    message: format!(
                        "{} closes the {space} on {} before {} finishes ({} < {})",
                        cover.task.name,
                        lvl,
                        r.task.name,
                        iso(cover.start),
                        iso(r.finish)
                    ),
                    phases: Vec::new(),
                });
                g.count = g.count.max(hs.len());
            }
        }
        let Some(last) = last_rough else { continue };
        if cover.start <= last {
            continue;
        }
        let inspected = inspections.iter().any(|(ki, l, z)| {
            let Some(i) = tasks.get(*ki) else { return false };
            let here = match l {
                Some(l) if !l.eq_ignore_ascii_case(&lvl) => false,
                Some(_) => z.as_ref().is_none_or(|z| cover_zone.as_ref().is_none_or(|cz| cz.eq_ignore_ascii_case(z))),
                None => true,
            };
            here && i.start > last && i.finish < cover.start
        });
        if !inspected {
            let phases: Vec<&'static str> = needs.iter().copied().collect();
            groups.entry(("cover_without_inspection", lvl.to_ascii_lowercase(), kc, 0)).or_insert_with(|| Finding {
                kind: "cover_without_inspection",
                severity: "high",
                level: lvl.clone(),
                zone: zone.clone(),
                tasks: vec![cover.task.id.clone()],
                trades: Vec::new(),
                dates: Some((last, cover.start)),
                elements: rough.values().flatten().take(FINDING_ELEMENTS).copied().collect(),
                count: rough.values().map(Vec::len).sum(),
                message: format!(
                    "{} closes the {space} on {} with no inspection between the rough-in and the cover-up (cover-check phases: {})",
                    cover.task.name,
                    lvl,
                    phases.join(", ")
                ),
                phases,
            });
        }
    }

    // Elements installed before their host.
    let hosts = hosts(&c, &els);
    for (i, h) in hosts {
        let (Some(e), Some(host)) = (els.get(i), els.get(h)) else { continue };
        if !level_ok(e) {
            continue;
        }
        let (Some(start), Some(kh)) = (first_start(i), first_task(h)) else { continue };
        let Some(ht) = tasks.get(kh) else { continue };
        if start <= ht.finish {
            let Some(ke) = first_task(i) else { continue };
            add_to(&mut groups, "before_host", "high", (ke, kh), e, Some(host), || Finding {
                kind: "before_host",
                severity: "high",
                level: e.level.clone(),
                zone: zone_of(e),
                tasks: vec![tasks.get(ke).map_or(String::new(), |t| t.task.id.clone()), ht.task.id.clone()],
                trades: Vec::new(),
                dates: Some((start, ht.finish)),
                elements: Vec::new(),
                count: 0,
                message: format!(
                    "a {} is installed ({}) before its host {} is in place ({})",
                    label_of(e),
                    iso(start),
                    label_of(host),
                    iso(ht.finish)
                ),
                phases: Vec::new(),
            });
        }
    }

    let mut list: Vec<&Finding> = groups.values().collect();
    list.sort_by_key(|f| (f.severity != "high", f.kind, f.level.clone(), f.dates.map(|d| d.0)));
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &list {
        *by_kind.entry(f.kind).or_default() += 1;
    }
    Ok(json!({
        "schedule": tasks.len(),
        "findings": list.iter().take(MAX_FINDINGS).map(|f| f.json()).collect::<Vec<_>>(),
        "by_kind": by_kind,
        "ok": list.is_empty(),
    }))
}

fn label_of(e: &El) -> String {
    match e.part.as_str() {
        "mep" => format!("{} {}", e.system(), e.kind()).trim().to_string(),
        "floor" => "floor slab".into(),
        p => p.to_string(),
    }
}

/// Each element's host, as indices into `els`: doors and windows their wall, wall devices the
/// wall they sit on, ceiling devices and terminals the ceiling, walls and columns the slab they
/// stand on, overhead runs the deck above them.
fn hosts(c: &Ctx, els: &[El]) -> Vec<(usize, usize)> {
    let mut by_level: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, e) in els.iter().enumerate() {
        by_level.entry(e.level.to_ascii_lowercase()).or_default().push(i);
    }
    let of = |level: &str, parts: &[&str]| -> Vec<usize> {
        by_level
            .get(&level.to_ascii_lowercase())
            .map(|v| v.iter().copied().filter(|&i| els.get(i).is_some_and(|e| parts.contains(&e.part.as_str()))).collect())
            .unwrap_or_default()
    };
    // The candidate nearest `p` among those whose box, grown by `reach`, meets `within`.
    let nearest_to = |cands: &[usize], p: [f64; 2], within: [f64; 4], reach: f64| -> Option<usize> {
        cands
            .iter()
            .copied()
            .filter_map(|i| els.get(i).map(|w| (i, w)))
            .filter(|(_, w)| overlaps(grow(w.bx, reach), within))
            .min_by(|a, b| {
                let d = |w: &El| {
                    let dx = (w.bx[0] - p[0]).max(p[0] - w.bx[2]).max(0.0);
                    let dy = (w.bx[1] - p[1]).max(p[1] - w.bx[3]).max(0.0);
                    dx * dx + dy * dy + (w.bx[2] - w.bx[0]) * (w.bx[3] - w.bx[1]) * 1e-9
                };
                d(a.1).total_cmp(&d(b.1))
            })
            .map(|(i, _)| i)
    };
    let nearest = |cands: &[usize], p: [f64; 2], reach: f64| nearest_to(cands, p, [p[0], p[1], p[0], p[1]], reach);
    let mut out = Vec::new();
    for (i, e) in els.iter().enumerate() {
        let p = e.center();
        let host = match e.part.as_str() {
            // The opening's own box meets the wall it is cut in (a door between two wall pieces).
            "door" | "window" => nearest_to(&of(&e.level, &["wall", "curtainwall"]), p, e.bx, c.upf / 6.0),
            "wall" | "column" | "stair" | "curtainwall" => nearest(&of(&e.level, &["floor", "slab"]), p, c.upf),
            "device" | "mep"
                if e.part == "device"
                    || matches!(e.kind(), "outlet" | "receptacle" | "switch" | "light_fixture" | "data_outlet" | "junction_box")
                    || e.tag("part").starts_with("terminal:") =>
            {
                if c.at_ceiling(e) {
                    nearest(&of(&e.level, &["ceiling", "soffit"]), p, 1.0)
                } else {
                    nearest(&of(&e.level, &["wall"]), p, c.upf)
                }
            }
            "mep" if c.overhead(e) => {
                let above = c.levels.get(c.rank(&e.level) + 1);
                above.and_then(|a| {
                    let slabs = of(&a.name, &["floor", "slab"]);
                    // Stacked plans: the slab over the run; plans side by side: the level's slab.
                    nearest(&slabs, p, 1.0).or_else(|| if slabs.len() == 1 { slabs.first().copied() } else { None })
                })
            }
            _ => None,
        };
        if let Some(h) = host
            && h != i
        {
            out.push((i, h));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_days_skip_weekends() {
        let mon = day("2026-03-02").unwrap();
        assert_eq!(weekday(mon), 0);
        assert_eq!(on_or_after(day("2026-02-28").unwrap(), 5), mon, "a Saturday start moves to Monday");
        assert_eq!(iso(add_working(mon, 4, 5)), "2026-03-06");
        assert_eq!(iso(add_working(mon, 5, 5)), "2026-03-09", "five working days on from Monday is the next Monday");
        assert_eq!(iso(add_working(mon, 7, 7)), "2026-03-09");
        assert_eq!(iso(add_working(mon, 6, 6)), "2026-03-09", "a six-day week works Saturday");
        assert_eq!(working_between(mon, day("2026-03-13").unwrap(), 5), 10);
        assert_eq!(working_between(mon, mon, 5), 1);
        assert_eq!(working_between(mon, mon - 1, 5), 0);
        // Hostile numbers stay bounded.
        let _ = add_working(i32::MAX - 10, i64::MAX, 5);
        let _ = add_working(i32::MIN, -5, 5);
    }

    #[test]
    fn settings_round_trip_and_refuse_bad_values() {
        let mut s = Settings::default();
        s.apply("t", &json!({"start": "2026-03-02", "crew": {"Drywall": 6, "duct": 3}, "rates": {"drywall": 0.03}, "workdays": 6, "zone_sf": 2500}))
            .unwrap();
        let back = Settings::from_value(&s.to_value());
        assert_eq!(back.start, s.start);
        assert_eq!(back.crew_for(Kind::Drywall), 6.0);
        assert_eq!(back.crew_for(Kind::Duct), 3.0);
        assert_eq!(back.crew_for(Kind::Slab), 4.0);
        assert_eq!((back.workdays, back.zone_sf, back.rates.get("drywall").copied()), (6, 2500.0, Some(0.03)));
        for bad_p in [
            json!({"start": "soon"}),
            json!({"crew": -1}),
            json!({"crew": "many"}),
            json!({"workdays": 4}),
            json!({"rates": {"painting": 1}}),
            json!({"hours_per_day": 30}),
            json!({"zone_sf": 1}),
        ] {
            assert!(Settings::default().apply("t", &bad_p).is_err(), "{bad_p}");
        }
    }

    #[test]
    fn the_cost_book_gives_hours_by_item_and_size() {
        let b = Book::starter();
        let (h, _, src) = b.rate("EMT conduit", "3/4\"").unwrap();
        assert_eq!(h, 0.05);
        assert!(src.contains("EMT conduit"), "{src}");
        assert_eq!(b.rate("Duct", "").unwrap().0, 0.25);
        assert!(b.rate("EMT conduit", "9\"").unwrap().2.contains("nearest"), "an unlisted size falls back");
        assert!(b.rate("Sprinkler head", "").is_none(), "not in the starter book: the default rate applies");
    }

    #[test]
    fn polygons_and_boxes() {
        let sq = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert_eq!(shoelace(&sq), 100.0);
        assert!(inside(&sq, [5.0, 5.0]) && !inside(&sq, [15.0, 5.0]));
        assert!(overlaps([0.0, 0.0, 1.0, 1.0], [1.0, 1.0, 2.0, 2.0]) && !overlaps([0.0, 0.0, 1.0, 1.0], [1.5, 0.0, 2.0, 1.0]));
        let z = Zones { bx: [0.0, 0.0, 100.0, 10.0], n: 4, along_x: true };
        assert_eq!((z.index([1.0, 5.0]), z.index([99.0, 5.0]), z.index([250.0, 5.0]), z.index([-5.0, 5.0])), (0, 3, 3, 0));
        assert_eq!(z.index([f64::NAN, 0.0]), 0);
    }
}
