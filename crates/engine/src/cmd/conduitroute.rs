//! Conduit routing (Q6152): each branch circuit's raceway or cable routed through the building
//! the model holds, checked against the code, and taken off from its pieces.
//!
//! `model.build` routes after the devices are built and before their pieces are cut
//! ([`super::mepatoms`]), on each level's own plan:
//!
//! - **Wiring method first** ([`Methods`]): the specification's wiring-methods table (26 05 19,
//!   read by the job into the drawing as [`METHODS_FILE`]) by area and use (a guest room's, a
//!   concealed common area's, an exposed room's), else the project's wiring-method rule, else the
//!   company default (EMT); the minimum raceway size and the circuits a homerun may share from the
//!   specification when it says. Conductor sizes by homerun length come from the legend's table
//!   (`0-60' = #12 AWG`) when the drawing prints one, else from the voltage drop.
//! - **Route**: a circuit's devices are joined device to device (a rectilinear spanning tree, never
//!   diagonal); each joint runs from its box up into the ceiling space (or along the wall between
//!   two boxes of one wall), across square, and down into the next box. The homerun leaves the
//!   device nearest the panel, takes the corridor (its centre lane) when the panel is far, and lands
//!   in the panel's allowed top entry window, each raceway at its own slot. Drawn conduit is never
//!   touched; only the circuits' routes are made here.
//! - **Collisions**: every horizontal piece is kept clear of the ducts, pipes, terminals, equipment,
//!   structure and other trades' conduit by the clearance (rules data), trying the level's tiers
//!   above the ceiling and other square shapes; what can't be avoided is a `clash` review item with
//!   both objects, the spot, the clearance needed and a fix. Rated walls crossed are listed as
//!   penetrations with their firestop.
//! - **Checks**: conduit fill (NEC Ch. 9, 40%), the 310.15(C)(1) adjustment for more than three
//!   current-carrying conductors, voltage drop (upsizing per the legend's table or 3%), and box fill
//!   (314.16) at every device box.
//!
//! The report is stored as [`REPORT_FILE`] among the drawing's embedded files: `conduit.route`
//! shows it, `conduit.takeoff` takes the routed model off (conduit, fittings, boxes, rings, straps,
//! wire, cable and firestop by level, area and circuit) and prices it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use cadaddy_doc::props::{self, Prop};
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::tags::ElementTag;
use cadaddy_walk3d::{BuildingSpec, MepKind, MepRun, MepSystem};
use serde::Deserialize;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// The system routed branch circuits go into the spec as.
pub(crate) const BRANCH_SYSTEM: &str = "E-BRANCH";
/// What a routed run says of its height.
pub(crate) const ROUTED: &str = "routed";
/// The routing's report among the drawing's embedded files.
pub(crate) const REPORT_FILE: &str = "route/report.json";
/// The specification's wiring methods as the job read them (the router's input).
pub(crate) const METHODS_FILE: &str = "route/methods.json";
const RULES: &str = include_str!("conduitroute.json");
/// What marks a homerun's own tag (the circuits it carries): `circuit:<level>:home:<panel>:<n>`.
const HOME_PART: &str = ":home:";
/// What an in-wall joint's run says it is (it follows its wall, not the plan's axes).
const IN_WALL: &str = "in the wall";

/// Most circuits, devices, obstacles, runs and items one routing handles (hostile models).
const MAX_CIRCUITS: usize = 20_000;
const MAX_DEVICES: usize = 100_000;
const MAX_OBSTACLES: usize = 400_000;
const MAX_RUNS: usize = 100_000;
const MAX_ITEMS: usize = 20_000;
const MAX_TEXTS: usize = 400_000;
/// A device this close (feet) to a point of its circuit's earlier route belongs to it.
const NEAR_RUN_FT: f64 = 3.0;
/// A typical room's own panel this close (feet) at most.
const ROOM_PANEL_FT: f64 = 60.0;
/// Where across a corridor its branch homerun lane runs (a fraction of its width).
const LANE_AT: f64 = 0.25;
/// Bucket size of the obstacle grid, feet.
const CELL_FT: f64 = 8.0;

// ─── Rules ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RouteRules {
    pub route: RouteGeom,
    pub clearance_in: BTreeMap<String, Value>,
    pub grouping: Grouping,
    pub fill: Fill,
    pub derate: Derate,
    pub box_fill: BoxFill,
    pub vdrop: VDrop,
    pub methods: MethodRules,
    pub firestop: Firestop,
    pub entry: Entry,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RouteGeom {
    pub above_ceiling_in: f64,
    pub tier_step_in: f64,
    pub structure_depth_in: f64,
    pub default_ceiling_ft: f64,
    pub corridor_min_ft: f64,
    pub in_wall_max_ft: f64,
    pub in_wall_above_box_in: f64,
    pub bend_penalty_ft: f64,
    pub max_detour_ft: f64,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Grouping {
    pub max_circuits: usize,
    pub reach_ft: f64,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Fill {
    pub one_pct: f64,
    pub two_pct: f64,
    pub over_two_pct: f64,
    pub source: String,
    pub area_sq_in: BTreeMap<String, Vec<[f64; 2]>>,
    pub wire_sq_in: BTreeMap<String, f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Derate {
    pub source: String,
    pub factors: Vec<[f64; 3]>,
    pub amp_75c: BTreeMap<String, f64>,
    pub amp_90c: BTreeMap<String, f64>,
    pub small_conductor_max_a: BTreeMap<String, f64>,
    pub small_conductor_source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct BoxFill {
    pub source: String,
    pub cu_in: BTreeMap<String, f64>,
    pub yoke: f64,
    pub grounds: f64,
    pub bigger: Vec<(String, String, f64)>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct VDrop {
    pub max_pct: f64,
    pub k_copper: f64,
    pub load_pct: f64,
    pub breaker_a: f64,
    pub voltage: f64,
    pub cmil: BTreeMap<String, f64>,
    pub sizes: Vec<String>,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct MethodRules {
    pub raceway: String,
    pub mc: String,
    pub ac: String,
    pub nm: String,
    pub se: String,
    pub uf: String,
    pub prefer: Vec<String>,
    pub branch_words: Vec<String>,
    pub feeder_words: Vec<String>,
    pub unit_words: Vec<String>,
    pub concealed_words: Vec<String>,
    pub exposed_words: Vec<String>,
    pub below_words: Vec<String>,
    pub exposed_rooms: Vec<String>,
    pub minimum_size_in: f64,
    pub minimum_source: String,
    pub source: String,
}

impl MethodRules {
    /// The method name (`EMT`, `MC`) for a spec table's method key (`raceway`, `mc`).
    fn name_of(&self, key: &str) -> Option<&str> {
        let v = match key {
            "raceway" => &self.raceway,
            "mc" => &self.mc,
            "ac" => &self.ac,
            "nm" => &self.nm,
            "se" => &self.se,
            "uf" => &self.uf,
            _ => return None,
        };
        Some(v.as_str()).filter(|v| !v.is_empty())
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Firestop {
    pub item: String,
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Entry {
    pub spacing_in: f64,
    pub locknut_od_in: BTreeMap<String, f64>,
    pub source: String,
}

impl RouteRules {
    fn clearance_in(&self, category: &str) -> f64 {
        let key = match category {
            "duct" | "pipe" | "fitting" | "terminal" | "equipment" | "conduit" => category,
            "beam" | "column" | "joist" | "truss" | "slab" | "structure" => "structure",
            _ => "other",
        };
        self.clearance_in.get(key).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.5)
    }
    fn clearance_source(&self) -> String {
        self.clearance_in.get("source").and_then(Value::as_str).unwrap_or_default().to_string()
    }
}

pub(crate) fn rules() -> &'static RouteRules {
    static R: OnceLock<RouteRules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

// ─── The documents' methods (what the job read) ─────────────────────────────

/// One application of the specification's wiring-methods table.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct MethodApp {
    pub section: String,
    pub item: String,
    pub application: String,
    pub methods: Vec<String>,
}

/// What the documents say of the branch circuits' wiring, as the job read it.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Methods {
    pub applications: Vec<MethodApp>,
    /// The specification's minimum raceway size, inches, and where it says so.
    pub min_size_in: Option<f64>,
    pub min_size_source: String,
    /// The circuits a homerun may share, and where it says so.
    pub max_circuits: Option<usize>,
    pub max_circuits_source: String,
}

impl Methods {
    pub(crate) fn stored(d: &Drawing) -> Self {
        d.embedded.get(METHODS_FILE).and_then(|b| serde_json::from_slice(b).ok()).unwrap_or_default()
    }
    pub(crate) fn store(&self, d: &mut Drawing) {
        if let Ok(b) = serde_json::to_vec(self) {
            d.embedded.insert(METHODS_FILE.into(), std::sync::Arc::new(b));
        }
    }
}

/// A trade size in a line of text (`3/4"`, `3/4 INCH`, `1"`): inches.
fn trade_size_in(t: &str) -> Option<f64> {
    let up = t.to_ascii_uppercase();
    for (word, v) in [("1/2", 0.5), ("3/4", 0.75), ("1-1/4", 1.25), ("1-1/2", 1.5)] {
        if up.contains(&format!("{word}\"")) || up.contains(&format!("{word} INCH")) || up.contains(&format!("{word}-INCH")) {
            return Some(v);
        }
    }
    None
}

/// A count in words or digits (`THREE`, `3`).
fn count_word(w: &str) -> Option<usize> {
    let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric());
    let n = match w {
        "ONE" => 1,
        "TWO" => 2,
        "THREE" => 3,
        "FOUR" => 4,
        "FIVE" => 5,
        "SIX" => 6,
        _ => w.parse::<usize>().ok()?,
    };
    (1..=12).contains(&n).then_some(n)
}

/// What a specification's text says of the minimum raceway size and the circuits per homerun.
pub(crate) fn spec_limits(text: &str, section: &str, m: &mut Methods) {
    for line in text.lines().take(200_000) {
        let up = line.to_ascii_uppercase();
        if m.min_size_in.is_none() && up.contains("MINIMUM") && (up.contains("RACEWAY") || up.contains("CONDUIT")) && up.contains("SIZE") {
            if let Some(v) = trade_size_in(&up) {
                m.min_size_in = Some(v);
                m.min_size_source = format!("spec {section}: {}", line.trim().chars().take(160).collect::<String>());
            }
        }
        if m.max_circuits.is_none() && (up.contains("HOMERUN") || up.contains("HOME RUN")) && up.contains("CIRCUIT") {
            let words: Vec<&str> = up.split_whitespace().collect();
            let n = words.windows(3).find_map(|w| match w {
                [a, b, c] if (*a == "THAN" || *a == "MAXIMUM" || *a == "OF") && c.starts_with("CIRCUIT") => count_word(b),
                [a, b, _] if (*a == "MAXIMUM" || *a == "THAN") && b.starts_with("OF") => None,
                _ => None,
            });
            if let Some(n) = n {
                m.max_circuits = Some(n);
                m.max_circuits_source = format!("spec {section}: {}", line.trim().chars().take(160).collect::<String>());
            }
        }
    }
}

/// The legend's conductor size by homerun length (`0-60' = #12 AWG`, `61' TO 100' = #10`): the
/// bands (longest feet, size) and the text they were read from.
pub(crate) fn length_table(texts: &[String]) -> (Vec<(f64, String)>, String) {
    let mut bands: Vec<(f64, String)> = Vec::new();
    let mut said: Vec<String> = Vec::new();
    for t in texts.iter().take(MAX_TEXTS) {
        let up = t.to_ascii_uppercase().replace(['\u{2019}', '`'], "'");
        if !up.contains('#') || !up.contains('\'') {
            continue;
        }
        // Each `<a> - <b>' = #<n>` (or `<a>' TO <b>'`) in the text.
        let mut found = false;
        for part in up.split(['\n', ';', ',']).take(64) {
            let Some((range, size)) = part.split_once('#') else { continue };
            let size: String = size.chars().take_while(|c| c.is_ascii_digit() || *c == '/').collect();
            if size.is_empty() {
                continue;
            }
            let nums: Vec<f64> = range
                .split(|c: char| !c.is_ascii_digit() && c != '.')
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0 && *v < 5000.0)
                .collect();
            let joined = range.contains('-') || range.contains(" TO ");
            if let (true, [_, hi, ..]) = (joined, nums.as_slice()) {
                if *hi > 0.0 && !bands.iter().any(|(h, _)| (h - hi).abs() < 0.5) && bands.len() < 16 {
                    bands.push((*hi, size));
                    found = true;
                }
            }
        }
        if found && said.len() < 4 {
            said.push(t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(160).collect());
        }
    }
    bands.sort_by(|a, b| a.0.total_cmp(&b.0));
    (if bands.len() >= 2 { bands } else { Vec::new() }, said.join(" | "))
}

/// The texts of a drawing (model space; each block's once), for the legend's tables.
fn drawing_texts(d: &Drawing) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |e: &Entity| match &e.kind {
        EntityKind::Text(t) => out.push(t.value.clone()),
        EntityKind::MText(t) => out.push(t.contents.clone()),
        _ => {}
    };
    for e in d.model.iter().take(MAX_TEXTS) {
        add(e);
    }
    out
}

// ─── Geometry ────────────────────────────────────────────────────────────────

/// A thing a conduit keeps clear of: its box (sheet units), what it is.
#[derive(Clone, Debug)]
pub(crate) struct Obs {
    pub handle: Handle,
    pub lo: Vec3,
    pub hi: Vec3,
    pub category: String,
    pub system: String,
    pub trade: &'static str,
}

/// The obstacles of one level, bucketed in plan.
#[derive(Default)]
pub(crate) struct Field {
    pub obs: Vec<Obs>,
    cell: f64,
    grid: HashMap<(i64, i64), Vec<usize>>,
}

impl Field {
    pub(crate) fn new(obs: Vec<Obs>, upf: f64) -> Self {
        let cell = (CELL_FT * upf).max(1e-6);
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, o) in obs.iter().enumerate() {
            if !(o.lo.is_finite() && o.hi.is_finite()) {
                continue;
            }
            let (x0, y0, x1, y1) = (Self::c(o.lo.x, cell), Self::c(o.lo.y, cell), Self::c(o.hi.x, cell), Self::c(o.hi.y, cell));
            // A huge box (a mis-built element) is skipped rather than filling the grid.
            let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
            if !(0..=512).contains(&w) || !(0..=512).contains(&h) || w.saturating_mul(h) > 4096 {
                continue;
            }
            for x in x0..=x1 {
                for y in y0..=y1 {
                    grid.entry((x, y)).or_default().push(i);
                }
            }
        }
        Field { obs, cell, grid }
    }
    fn c(v: f64, cell: f64) -> i64 {
        (v / cell).floor().clamp(-1e12, 1e12) as i64
    }
    /// The first obstacle a square piece `a`–`b` at height `z` (radius `r`) comes within its
    /// clearance of, skipping `skip`.
    fn hit(&self, a: Vec2, b: Vec2, z: f64, r: f64, upf: f64, skip: &[Handle], rl: &RouteRules) -> Option<usize> {
        let (lo, hi) = (Vec2::new(a.x.min(b.x), a.y.min(b.y)), Vec2::new(a.x.max(b.x), a.y.max(b.y)));
        let pad = 6.0 * upf;
        let (x0, y0, x1, y1) =
            (Self::c(lo.x - pad, self.cell), Self::c(lo.y - pad, self.cell), Self::c(hi.x + pad, self.cell), Self::c(hi.y + pad, self.cell));
        let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        if !(a.is_finite() && b.is_finite() && z.is_finite())
            || !(0..=4096).contains(&w)
            || !(0..=4096).contains(&h)
            || w.saturating_mul(h) > 1_000_000
        {
            return None;
        }
        let mut seen: BTreeSet<usize> = BTreeSet::new();
        let mut best: Option<(f64, usize)> = None;
        for x in x0..=x1 {
            for y in y0..=y1 {
                for &i in self.grid.get(&(x, y)).into_iter().flatten() {
                    if !seen.insert(i) {
                        continue;
                    }
                    let Some(o) = self.obs.get(i) else { continue };
                    if skip.contains(&o.handle) {
                        continue;
                    }
                    let c = rl.clearance_in(&o.category) / 12.0 * upf + r;
                    let meets =
                        lo.x <= o.hi.x + c && hi.x >= o.lo.x - c && lo.y <= o.hi.y + c && hi.y >= o.lo.y - c && z >= o.lo.z - c && z <= o.hi.z + c;
                    if meets {
                        // The nearest along the piece first.
                        let along = Vec2::new((o.lo.x + o.hi.x) / 2.0, (o.lo.y + o.hi.y) / 2.0).dist(a);
                        if best.is_none_or(|(d, _)| along < d) {
                            best = Some((along, i));
                        }
                    }
                }
            }
        }
        best.map(|(_, i)| i)
    }
}

fn manhattan(a: Vec2, b: Vec2) -> f64 {
    (a.x - b.x).abs() + (a.y - b.y).abs()
}

fn poly_len(p: &[Vec2]) -> f64 {
    p.windows(2).map(|w| if let [a, b] = w { a.dist(*b) } else { 0.0 }).sum()
}

/// A square path without repeated points or straight-through corners.
fn tidy(p: Vec<Vec2>, eps: f64) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::new();
    for q in p {
        if out.last().is_none_or(|l: &Vec2| l.dist(q) > eps) {
            out.push(q);
        }
    }
    let mut i = 1;
    while i + 1 < out.len() {
        let (Some(a), Some(b), Some(c)) = (out.get(i - 1).copied(), out.get(i).copied(), out.get(i + 1).copied()) else { break };
        let straight = ((a.x - b.x).abs() <= eps && (b.x - c.x).abs() <= eps) || ((a.y - b.y).abs() <= eps && (b.y - c.y).abs() <= eps);
        if straight {
            out.remove(i);
        } else {
            i += 1;
        }
    }
    out
}

/// The square shapes from `a` to `b`: the two Ls, Zs through the middle and around a detour.
fn shapes(a: Vec2, b: Vec2, detour: f64, eps: f64) -> Vec<Vec<Vec2>> {
    let mut out = vec![vec![a, Vec2::new(b.x, a.y), b], vec![a, Vec2::new(a.x, b.y), b]];
    for t in [0.5, 0.25, 0.75] {
        let x = a.x + t * (b.x - a.x);
        let y = a.y + t * (b.y - a.y);
        out.push(vec![a, Vec2::new(x, a.y), Vec2::new(x, b.y), b]);
        out.push(vec![a, Vec2::new(a.x, y), Vec2::new(b.x, y), b]);
    }
    for k in [0.5, 1.0] {
        let d = detour * k;
        let (ylo, yhi) = (a.y.min(b.y) - d, a.y.max(b.y) + d);
        let (xlo, xhi) = (a.x.min(b.x) - d, a.x.max(b.x) + d);
        out.push(vec![a, Vec2::new(a.x, ylo), Vec2::new(b.x, ylo), b]);
        out.push(vec![a, Vec2::new(a.x, yhi), Vec2::new(b.x, yhi), b]);
        out.push(vec![a, Vec2::new(xlo, a.y), Vec2::new(xlo, b.y), b]);
        out.push(vec![a, Vec2::new(xhi, a.y), Vec2::new(xhi, b.y), b]);
    }
    out.into_iter().map(|p| tidy(p, eps)).collect()
}

/// A piece the router could not keep clear.
#[derive(Clone, Debug)]
pub(crate) struct Blocked {
    pub obstacle: usize,
    pub at: Vec3,
    pub z: f64,
}

/// A square path at its pieces' heights.
#[derive(Clone, Debug, Default)]
pub(crate) struct Leg {
    pub pts: Vec<Vec2>,
    pub z: Vec<f64>,
    pub blocked: Vec<Blocked>,
}

/// Each piece of `path` at the lowest of `tiers` it is clear at; a piece clear at none stays at
/// the first, blocked.
fn seat(path: &[Vec2], tiers: &[f64], r: f64, field: &Field, upf: f64, skip: &[Handle], rl: &RouteRules) -> Leg {
    let mut leg = Leg { pts: path.to_vec(), ..Leg::default() };
    let first = tiers.first().copied().unwrap_or(0.0);
    for w in path.windows(2) {
        let [a, b] = w else { continue };
        match tiers.iter().copied().find(|z| field.hit(*a, *b, *z, r, upf, skip, rl).is_none()) {
            Some(z) => leg.z.push(z),
            None => {
                leg.z.push(first);
                if let Some(i) = field.hit(*a, *b, first, r, upf, skip, rl) {
                    let at = field.obs.get(i).map_or((*a + *b) * 0.5, |o| {
                        // The point on the piece nearest the obstacle's centre.
                        let c = Vec2::new((o.lo.x + o.hi.x) / 2.0, (o.lo.y + o.hi.y) / 2.0);
                        let (dx, dy) = (b.x - a.x, b.y - a.y);
                        let l2 = dx * dx + dy * dy;
                        let t = if l2 > 0.0 { (((c.x - a.x) * dx + (c.y - a.y) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                        Vec2::new(a.x + t * dx, a.y + t * dy)
                    });
                    leg.blocked.push(Blocked { obstacle: i, at: Vec3::new(at.x, at.y, first), z: first });
                }
            }
        }
    }
    leg
}

/// The best square route from `a` to `b`: fewest blocked pieces, then shortest (bends cost).
pub(crate) fn route_leg(a: Vec2, b: Vec2, tiers: &[f64], r: f64, field: &Field, upf: f64, skip: &[Handle], rl: &RouteRules) -> Leg {
    let eps = upf / 96.0;
    let detour = rl.route.max_detour_ft.max(0.0) * upf;
    let bend = rl.route.bend_penalty_ft.max(0.0) * upf;
    let score = |l: &Leg| {
        let tiers_changed = l.z.windows(2).filter(|w| matches!(w, [p, q] if (p - q).abs() > 1e-9)).count() as f64;
        (l.blocked.len(), poly_len(&l.pts) + bend * (l.pts.len().saturating_sub(2) as f64 + tiers_changed))
    };
    let mut best: Option<((usize, f64), Leg)> = None;
    for p in shapes(a, b, detour, eps) {
        if p.len() < 2 {
            let one = Leg { pts: vec![a, b], z: vec![tiers.first().copied().unwrap_or(0.0)], blocked: Vec::new() };
            return one;
        }
        let leg = seat(&p, tiers, r, field, upf, skip, rl);
        let s = score(&leg);
        if best.as_ref().is_none_or(|(bs, _)| s.0 < bs.0 || (s.0 == bs.0 && s.1 < bs.1 - 1e-9)) {
            let clear = s.0 == 0;
            best = Some((s, leg));
            // The two Ls first: a clear L is never beaten by a longer shape.
            if clear && best.as_ref().is_some_and(|(s, _)| s.1 <= manhattan(a, b) + bend + 1e-9) {
                break;
            }
        }
    }
    best.map(|(_, l)| l).unwrap_or_default()
}

/// A rectilinear spanning tree over `pts` (Prim, square distance) as an electrician chains the
/// boxes: the root (where the homerun lands) takes one joint, every other box at most two (a
/// box in, a box out), so no box collects more raceways than its fill allows. Its joints, from
/// `root`.
pub(crate) fn spanning_tree(pts: &[Vec2], root: usize) -> Vec<(usize, usize)> {
    let n = pts.len();
    if n < 2 || root >= n || n > 20_000 {
        return Vec::new();
    }
    let cap = |i: usize| if i == root { 1 } else { 2 };
    let mut inside = vec![false; n];
    let mut degree = vec![0usize; n];
    if let Some(r) = inside.get_mut(root) {
        *r = true;
    }
    let mut members = vec![root];
    let mut out = Vec::new();
    for _ in 1..n {
        // The nearest box outside to a box inside that can still take a joint.
        let mut best: Option<(f64, usize, usize)> = None;
        for &f in &members {
            if degree.get(f).copied().unwrap_or(usize::MAX) >= cap(f) {
                continue;
            }
            let Some(pf) = pts.get(f) else { continue };
            for (i, p) in pts.iter().enumerate() {
                if inside.get(i).copied().unwrap_or(true) {
                    continue;
                }
                let d = manhattan(*p, *pf);
                if best.is_none_or(|(bd, _, _)| d < bd) {
                    best = Some((d, f, i));
                }
            }
        }
        let Some((_, from, next)) = best else { break };
        for x in [from, next] {
            if let Some(dg) = degree.get_mut(x) {
                *dg += 1;
            }
        }
        if let Some(x) = inside.get_mut(next) {
            *x = true;
        }
        members.push(next);
        out.push((from, next));
    }
    out
}

// ─── Corridors ───────────────────────────────────────────────────────────────

/// A corridor's centre lane: a horizontal or vertical line through its rectangle.
#[derive(Clone, Copy, Debug)]
struct Lane {
    a: Vec2,
    b: Vec2,
    lo: Vec2,
    hi: Vec2,
}

impl Lane {
    fn of(r: &[[f64; 2]; 2]) -> Option<Lane> {
        let [p, q] = r;
        let lo = Vec2::new(p[0].min(q[0]), p[1].min(q[1]));
        let hi = Vec2::new(p[0].max(q[0]), p[1].max(q[1]));
        if !(lo.x.is_finite() && lo.y.is_finite() && hi.x.is_finite() && hi.y.is_finite()) || hi.x - lo.x <= 0.0 || hi.y - lo.y <= 0.0 {
            return None;
        }
        // A quarter of the way across: branch homeruns keep beside the centre line, where the
        // feeders and mains run.
        let (a, b) = if hi.x - lo.x >= hi.y - lo.y {
            let y = lo.y + (hi.y - lo.y) * LANE_AT;
            (Vec2::new(lo.x, y), Vec2::new(hi.x, y))
        } else {
            let x = lo.x + (hi.x - lo.x) * LANE_AT;
            (Vec2::new(x, lo.y), Vec2::new(x, hi.y))
        };
        Some(Lane { a, b, lo, hi })
    }
    fn horizontal(&self) -> bool {
        (self.a.y - self.b.y).abs() < 1e-9
    }
    /// The point on the lane nearest `p`.
    fn foot(&self, p: Vec2) -> Vec2 {
        if self.horizontal() { Vec2::new(p.x.clamp(self.a.x, self.b.x), self.a.y) } else { Vec2::new(self.a.x, p.y.clamp(self.a.y, self.b.y)) }
    }
    fn holds(&self, p: Vec2, pad: f64) -> bool {
        p.x >= self.lo.x - pad && p.x <= self.hi.x + pad && p.y >= self.lo.y - pad && p.y <= self.hi.y + pad
    }
}

/// The corridors of a level as a graph of lanes: the square path along them from `from` to `to`
/// (each joined at the nearest lane point), or none.
pub(crate) fn corridor_path(rects: &[[[f64; 2]; 2]], from: Vec2, to: Vec2, upf: f64) -> Option<Vec<Vec2>> {
    let lanes: Vec<Lane> = rects.iter().filter_map(Lane::of).take(2_000).collect();
    if lanes.is_empty() {
        return None;
    }
    let near = |p: Vec2| lanes.iter().enumerate().min_by(|a, b| a.1.foot(p).dist(p).total_cmp(&b.1.foot(p).dist(p))).map(|(i, l)| (i, l.foot(p)));
    let (la, pa) = near(from)?;
    let (lb, pb) = near(to)?;
    // Nodes: each lane's ends, its crossings with the others, and the two feet.
    let mut on: Vec<Vec<Vec2>> = lanes.iter().map(|l| vec![l.a, l.b]).collect();
    let mut links: Vec<(Vec2, Vec2)> = Vec::new();
    let pad = upf * 0.5;
    for (i, l) in lanes.iter().enumerate() {
        for (j, m) in lanes.iter().enumerate().skip(i + 1) {
            if l.horizontal() != m.horizontal() {
                let (h, v) = if l.horizontal() { (l, m) } else { (m, l) };
                let x = Vec2::new(v.a.x, h.a.y);
                if h.holds(x, pad) || v.holds(x, pad) {
                    // Crossing or reaching into each other's rectangle: a joint at the corner.
                    let (xh, xv) = (h.foot(x), v.foot(x));
                    let (ih, iv) = if l.horizontal() { (i, j) } else { (j, i) };
                    if let Some(v) = on.get_mut(ih) {
                        v.push(xh);
                    }
                    if let Some(v) = on.get_mut(iv) {
                        v.push(xv);
                    }
                    if (h.holds(x, pad) && v.holds(x, pad)) || xh.dist(xv) < 30.0 * upf {
                        links.push((xh, x));
                        links.push((x, xv));
                    }
                }
            } else {
                // Side by side: joined where their rectangles touch, by a square jog.
                let touch = l.lo.x <= m.hi.x + pad && m.lo.x <= l.hi.x + pad && l.lo.y <= m.hi.y + pad && m.lo.y <= l.hi.y + pad;
                if touch {
                    let mid = Vec2::new((l.lo.x.max(m.lo.x) + l.hi.x.min(m.hi.x)) / 2.0, (l.lo.y.max(m.lo.y) + l.hi.y.min(m.hi.y)) / 2.0);
                    let (fl, fm) = (l.foot(mid), m.foot(mid));
                    if let Some(v) = on.get_mut(i) {
                        v.push(fl);
                    }
                    if let Some(v) = on.get_mut(j) {
                        v.push(fm);
                    }
                    links.push((fl, fm));
                }
            }
        }
    }
    if let Some(v) = on.get_mut(la) {
        v.push(pa);
    }
    if let Some(v) = on.get_mut(lb) {
        v.push(pb);
    }
    // The graph: points (merged within a hair), edges along each lane and the links.
    let eps = upf / 24.0;
    let mut pts: Vec<Vec2> = Vec::new();
    let id = |p: Vec2, pts: &mut Vec<Vec2>| -> usize {
        if let Some(i) = pts.iter().position(|q| q.dist(p) <= eps) {
            return i;
        }
        pts.push(p);
        pts.len() - 1
    };
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for (k, list) in on.iter_mut().enumerate() {
        let horiz = lanes.get(k).is_some_and(Lane::horizontal);
        list.sort_by(|a, b| if horiz { a.x.total_cmp(&b.x) } else { a.y.total_cmp(&b.y) });
        let ids: Vec<usize> = list.iter().map(|p| id(*p, &mut pts)).collect();
        for w in ids.windows(2) {
            if let [a, b] = w
                && a != b
            {
                edges.push((*a, *b));
            }
        }
    }
    for (p, q) in &links {
        let (a, b) = (id(*p, &mut pts), id(*q, &mut pts));
        if a != b {
            edges.push((a, b));
        }
    }
    if pts.len() > 20_000 {
        return None;
    }
    let (s, t) = (id(pa, &mut pts), id(pb, &mut pts));
    let n = pts.len();
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for (a, b) in edges {
        let (Some(p), Some(q)) = (pts.get(a), pts.get(b)) else { continue };
        let w = manhattan(*p, *q);
        if let Some(v) = adj.get_mut(a) {
            v.push((b, w));
        }
        if let Some(v) = adj.get_mut(b) {
            v.push((a, w));
        }
    }
    // Dijkstra (small graphs: a plain scan).
    let mut dist = vec![f64::INFINITY; n];
    let mut prev = vec![usize::MAX; n];
    let mut done = vec![false; n];
    if let Some(x) = dist.get_mut(s) {
        *x = 0.0;
    }
    for _ in 0..n {
        let Some((u, du)) = dist
            .iter()
            .copied()
            .enumerate()
            .filter(|(i, d)| !done.get(*i).copied().unwrap_or(true) && d.is_finite())
            .min_by(|a, b| a.1.total_cmp(&b.1))
        else {
            break;
        };
        if let Some(x) = done.get_mut(u) {
            *x = true;
        }
        if u == t {
            break;
        }
        for &(v, w) in adj.get(u).into_iter().flatten() {
            if du + w < dist.get(v).copied().unwrap_or(0.0) {
                if let Some(x) = dist.get_mut(v) {
                    *x = du + w;
                }
                if let Some(x) = prev.get_mut(v) {
                    *x = u;
                }
            }
        }
    }
    if !dist.get(t).is_some_and(|d| d.is_finite()) {
        return None;
    }
    let mut path = vec![t];
    let mut u = t;
    while u != s && path.len() <= n {
        u = prev.get(u).copied().unwrap_or(usize::MAX);
        if u == usize::MAX {
            return None;
        }
        path.push(u);
    }
    path.reverse();
    // Square: a link that is a diagonal (it can't be by construction) gets its corner.
    let mut out: Vec<Vec2> = vec![from];
    let mut last = from;
    for p in path.into_iter().filter_map(|i| pts.get(i).copied()) {
        if (p.x - last.x).abs() > eps && (p.y - last.y).abs() > eps {
            out.push(Vec2::new(p.x, last.y));
        }
        out.push(p);
        last = p;
    }
    if (to.x - last.x).abs() > eps && (to.y - last.y).abs() > eps {
        out.push(Vec2::new(to.x, last.y));
    }
    out.push(to);
    Some(tidy(out, eps))
}

// ─── Code checks ─────────────────────────────────────────────────────────────

/// One conductor group of a makeup: count, size (`12`, `1/0`), ground.
type Part = (u32, String, bool);

pub(crate) fn makeup_text(parts: &[Part]) -> String {
    parts.iter().map(|(n, s, g)| format!("{n}#{s}{}", if *g { "G" } else { "" })).collect::<Vec<_>>().join(" + ")
}

/// The next larger conductor size (`12` → `10`), or the same at the table's end.
fn larger(size: &str, rl: &RouteRules) -> String {
    let s = &rl.vdrop.sizes;
    s.iter().position(|x| x == size).and_then(|i| s.get(i + 1)).cloned().unwrap_or_else(|| size.to_string())
}

/// The smallest raceway of `material` whose fill holds `parts` (NEC Ch. 9), at least `min_in`:
/// its trade size, the conductors' area and the allowed area. None: no size in the table holds it.
pub(crate) fn fill_size(parts: &[Part], material: &str, min_in: f64, rl: &RouteRules) -> Option<(f64, f64, f64)> {
    let count: u32 = parts.iter().fold(0u32, |s, p| s.saturating_add(p.0));
    let area: f64 = parts.iter().map(|(n, s, _)| f64::from(*n) * rl.fill.wire_sq_in.get(s).copied().unwrap_or(0.0133)).sum();
    let pct = match count {
        0 | 1 => rl.fill.one_pct,
        2 => rl.fill.two_pct,
        _ => rl.fill.over_two_pct,
    } / 100.0;
    let table = rl.fill.area_sq_in.get(material).or_else(|| rl.fill.area_sq_in.get("EMT"))?;
    table.iter().filter(|[t, _]| *t + 1e-9 >= min_in).find(|[_, a]| a * pct + 1e-12 >= area).map(|[t, a]| (*t, area, a * pct))
}

/// The NEC 310.15(C)(1) factor for `ccc` current-carrying conductors in one raceway.
pub(crate) fn adjustment(ccc: u32, rl: &RouteRules) -> f64 {
    let n = f64::from(ccc);
    rl.derate.factors.iter().find(|[lo, hi, _]| n >= *lo && n <= *hi).map_or(1.0, |f| f[2])
}

/// A conductor's allowed current in a raceway with `ccc` current-carrying conductors: the 90 °C
/// column adjusted, never above the 75 °C column (terminations) or 240.4(D).
pub(crate) fn allowed_a(size: &str, ccc: u32, rl: &RouteRules) -> f64 {
    let f = adjustment(ccc, rl);
    let a90 = rl.derate.amp_90c.get(size).copied().unwrap_or(0.0) * f;
    let a75 = rl.derate.amp_75c.get(size).copied().unwrap_or(0.0);
    let small = rl.derate.small_conductor_max_a.get(size).copied().unwrap_or(f64::INFINITY);
    a90.min(a75).min(small)
}

/// The voltage drop in percent over `ft` one way at `amps` on copper `size`.
pub(crate) fn vdrop_pct(size: &str, ft: f64, amps: f64, volts: f64, rl: &RouteRules) -> Option<f64> {
    let cm = rl.vdrop.cmil.get(size).copied().filter(|c| *c > 0.0)?;
    (volts > 0.0).then(|| 2.0 * rl.vdrop.k_copper * amps * ft / cm / volts * 100.0)
}

/// The conductor size a homerun of `ft` needs: the legend's band, else the voltage drop.
pub(crate) fn size_for_length(ft: f64, amps: f64, volts: f64, table: &[(f64, String)], rl: &RouteRules) -> Option<(String, String)> {
    if let Some((hi, s)) = table.iter().find(|(hi, _)| ft <= *hi + 1e-9) {
        return Some((s.clone(), format!("legend: up to {hi:.0} ft #{s}")));
    }
    if let Some((hi, s)) = table.last() {
        return Some((s.clone(), format!("legend: over its last band ({hi:.0} ft), its largest size #{s}; the engineer confirms")));
    }
    rl.vdrop
        .sizes
        .iter()
        .filter(|s| rl.vdrop.cmil.contains_key(*s))
        .find(|s| vdrop_pct(s, ft, amps, volts, rl).is_some_and(|v| v <= rl.vdrop.max_pct))
        .map(|s| {
            let v = vdrop_pct(s, ft, amps, volts, rl).unwrap_or(0.0);
            (s.clone(), format!("computed: {ft:.0} ft at {amps:.0} A, {volts:.0} V: {v:.1}% on #{s} ({})", rl.vdrop.source))
        })
}

/// Box fill (NEC 314.16(B)): the volume `conductors` (each entering the box) and a device yoke
/// need, with the grounds counted once.
pub(crate) fn box_volume(conductors: &[(u32, String)], grounds: bool, yoke: bool, rl: &RouteRules) -> f64 {
    let vol = |s: &str| rl.box_fill.cu_in.get(s).copied().unwrap_or(2.25);
    let largest = conductors.iter().map(|(_, s)| vol(s)).fold(0.0, f64::max);
    let mut v: f64 = conductors.iter().map(|(n, s)| f64::from(*n) * vol(s)).sum();
    if grounds {
        v += rl.box_fill.grounds * largest;
    }
    if yoke {
        v += rl.box_fill.yoke * largest;
    }
    v
}

// ─── The model's side ────────────────────────────────────────────────────────

/// A device as the router sees it.
#[derive(Clone, Debug)]
struct Dev {
    handle: Handle,
    level: String,
    circuit: String,
    kind: String,
    room: String,
    at: Vec2,
    /// The top of its box (where a conduit enters), sheet units.
    top: f64,
    /// Its face's outward direction (a wall device).
    face: Option<Vec2>,
    yoke: bool,
}

/// A gear element a homerun lands on.
#[derive(Clone, Debug)]
struct GearEl {
    handle: Handle,
    level: String,
    tag: String,
    lo: Vec3,
    hi: Vec3,
}

/// A rated wall a route may cross.
#[derive(Clone, Debug)]
struct Rated {
    handle: Handle,
    level: String,
    hull: Vec<Vec2>,
    top: f64,
    wall_type: String,
    rating: String,
}

fn mesh_box(e: &Entity) -> Option<(Vec3, Vec3)> {
    let EntityKind::Mesh(m) = &e.kind else { return None };
    let mut lo = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut hi = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for v in m.vertices.iter().take(200_000).filter(|v| v.is_finite()) {
        lo = Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
        hi = Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
    }
    (lo.x <= hi.x && lo.y <= hi.y && lo.z <= hi.z).then_some((lo, hi))
}

fn face_of(ps: &props::Props) -> Option<Vec2> {
    ["face_normal", "host_face"].iter().find_map(|k| {
        let t = ps.get(k).map(Prop::text)?;
        let v: Vec<f64> = t.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        match v[..] {
            [x, y] if x.is_finite() && y.is_finite() && x.hypot(y) > 1e-9 => Some(Vec2::new(x, y) * (1.0 / x.hypot(y))),
            _ => None,
        }
    })
}

fn same_level(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

/// Whether a point is inside a polygon (even-odd).
fn inside(p: Vec2, poly: &[Vec2]) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + n - 1) % n)) else { continue };
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            c = !c;
        }
    }
    c
}

/// Whether segment `a`–`b` crosses polygon `poly` (goes through it, ends outside it).
fn crosses(a: Vec2, b: Vec2, poly: &[Vec2]) -> bool {
    if inside(a, poly) || inside(b, poly) {
        return false;
    }
    let n = poly.len();
    let cross = |o: Vec2, p: Vec2, q: Vec2| (p.x - o.x) * (q.y - o.y) - (p.y - o.y) * (q.x - o.x);
    (0..n).any(|i| {
        let (Some(c), Some(d)) = (poly.get(i), poly.get((i + 1) % n)) else { return false };
        let (d1, d2, d3, d4) = (cross(*c, *d, a), cross(*c, *d, b), cross(a, b, *c), cross(a, b, *d));
        (d1 > 0.0) != (d2 > 0.0) && (d3 > 0.0) != (d4 > 0.0)
    })
}

/// What one routing made and found.
#[derive(Clone, Debug, Default)]
pub(crate) struct Routed {
    pub runs: Vec<MepRun>,
    pub tags: Vec<ElementTag>,
    pub report: Value,
}

/// One circuit being routed.
struct Circ {
    tag: usize,
    label: String,
    panel: String,
    level: String,
    devs: Vec<usize>,
}

fn field_of<'a>(t: &'a ElementTag, k: &str) -> Option<&'a str> {
    t.fields.get(k).map(|f| f.value.trim()).filter(|v| !v.is_empty())
}

/// The circuits' routes for `sp` over the model in `d` (its devices built, their boxes not yet):
/// the runs (replacing the estimated ones), the circuits' tags as routed, and the report.
pub(crate) fn route(d: &Drawing, sp: &BuildingSpec) -> Routed {
    let rl = rules();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let methods = Methods::stored(d);
    let est = super::printcond::CIRCUIT_SYSTEM;
    let mut out = Routed::default();
    let circuit_tags: Vec<(usize, &ElementTag)> =
        sp.tags.iter().enumerate().filter(|(_, t)| t.kind == "circuit" && !t.id.contains(HOME_PART)).take(MAX_CIRCUITS).collect();
    if circuit_tags.is_empty() {
        out.report = json!({"circuits": 0, "note": "no circuits in the spec: nothing to route"});
        return out;
    }

    // -- The devices, the gear, the obstacles, the rated walls. --
    let mut devs: Vec<Dev> = Vec::new();
    let mut gear: Vec<GearEl> = Vec::new();
    let mut rated: Vec<Rated> = Vec::new();
    let rating_of = |name: &str| {
        sp.wall_types
            .iter()
            .find(|w| w.name.eq_ignore_ascii_case(name))
            .and_then(|w| w.assembly.as_ref())
            .and_then(|a| a.fire_rating.clone())
            .filter(|r| !r.trim().is_empty() && !r.trim().eq_ignore_ascii_case("none") && !r.trim().starts_with('0'))
    };
    for e in d.model.iter() {
        match super::wallgen::made_as(e).as_deref() {
            Some("device") if devs.len() < MAX_DEVICES => {
                let ps = props::read(&e.common.xdata);
                if ps.get("misplaced").is_some() {
                    continue;
                }
                let Some((lo, hi)) = mesh_box(e) else { continue };
                let off = super::bspec::stacked_by(e);
                let (lo, hi) = (lo - Vec3::new(off.x, off.y, 0.0), hi - Vec3::new(off.x, off.y, 0.0));
                let kind = ps.get("kind").map(Prop::text).unwrap_or_default();
                let yoke = matches!(kind.replace('_', " ").as_str(), "receptacle" | "switch" | "dimmer" | "occupancy sensor");
                devs.push(Dev {
                    handle: e.handle,
                    level: super::bspec::made_level(e).unwrap_or_default(),
                    circuit: ps.get("circuit").map(Prop::text).unwrap_or_default().trim().to_string(),
                    kind,
                    room: ps.get("room").map(Prop::text).unwrap_or_default(),
                    at: Vec2::new((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0),
                    top: hi.z,
                    face: face_of(&ps),
                    yoke,
                });
            }
            Some("wall") if rated.len() < MAX_DEVICES => {
                let Some(t) = super::bimlook::made_tag(e, "type") else { continue };
                let Some(r) = rating_of(t) else { continue };
                let Some((_, hi)) = mesh_box(e) else { continue };
                let EntityKind::Mesh(m) = &e.kind else { continue };
                let off = super::bspec::stacked_by(e);
                let hull = super::mep::hull(&m.vertices.iter().take(10_000).map(|v| v.xy() - off).collect::<Vec<_>>());
                if hull.len() >= 3 {
                    rated.push(Rated {
                        handle: e.handle,
                        level: super::bspec::made_level(e).unwrap_or_default(),
                        hull,
                        top: hi.z,
                        wall_type: t.to_string(),
                        rating: r,
                    });
                }
            }
            _ => {}
        }
        if super::bimlook::made_tag(e, "part").is_some_and(|p| p.to_ascii_lowercase().starts_with("equipment"))
            && gear.len() < MAX_DEVICES
            && let Some((lo, hi)) = mesh_box(e)
        {
            let off = super::bspec::stacked_by(e);
            let (lo, hi) = (lo - Vec3::new(off.x, off.y, 0.0), hi - Vec3::new(off.x, off.y, 0.0));
            gear.push(GearEl {
                handle: e.handle,
                level: super::bspec::made_level(e).unwrap_or_default(),
                tag: super::bimlook::made_tag(e, "tag").unwrap_or_default().to_string(),
                lo,
                hi,
            });
        }
    }
    let mut obstacles: BTreeMap<String, Vec<Obs>> = BTreeMap::new();
    let mut n_obs = 0usize;
    for el in super::bimwork::elements(d) {
        if n_obs >= MAX_OBSTACLES {
            break;
        }
        let keep = match el.trade {
            "architecture" => false,
            "electrical" => matches!(el.category.as_str(), "equipment") || (el.category == "conduit" && el.run.is_empty()),
            _ => !matches!(el.category.as_str(), "room" | "device" | "space" | "zone"),
        };
        if !keep || !(el.lo.is_finite() && el.hi.is_finite()) {
            continue;
        }
        n_obs += 1;
        let off = d.model.get(el.handle).map_or(Vec2::new(0.0, 0.0), |e| super::bspec::stacked_by(e));
        let off3 = Vec3::new(off.x, off.y, 0.0);
        obstacles.entry(el.level.to_ascii_lowercase()).or_default().push(Obs {
            handle: el.handle,
            lo: el.lo - off3,
            hi: el.hi - off3,
            category: el.category.clone(),
            system: el.system.clone(),
            trade: el.trade,
        });
    }
    // Each panel's working space (NEC 110.26(A): 3 ft deep before it, 6'-6" high; which side is its
    // front the model doesn't say, so all round): no branch piece through it. A panel's own
    // homerun skips it (by the panel's handle).
    for g in &gear {
        let floor = sp.level(&g.level).map_or(0.0, |l| l.elevation_ft * upf);
        let reach = 3.0 * upf;
        obstacles.entry(g.level.to_ascii_lowercase()).or_default().push(Obs {
            handle: g.handle,
            lo: Vec3::new(g.lo.x - reach, g.lo.y - reach, floor),
            hi: Vec3::new(g.hi.x + reach, g.hi.y + reach, (floor + 6.5 * upf).max(g.hi.z)),
            category: "working space".into(),
            system: g.tag.clone(),
            trade: "electrical",
        });
    }
    let fields: BTreeMap<String, Field> = obstacles.into_iter().map(|(k, v)| (k, Field::new(v, upf))).collect();
    let empty = Field::default();

    // -- Each circuit's devices: those of its label near its earlier route (a typical room's own
    // circuits first), then the rest of its label on its level. --
    let mut pts_of: HashMap<&str, Vec<Vec2>> = HashMap::new();
    for r in sp.mep.runs.iter().filter(|r| r.system == est || r.system == BRANCH_SYSTEM).take(MAX_RUNS) {
        if let Some(c) = super::printcond::circuit_of_run(r) {
            pts_of.entry(c).or_default().extend(r.points.iter().take(1_000).map(|p| Vec2::new(p[0], p[1])));
        }
    }
    let mut circs: Vec<Circ> = circuit_tags
        .iter()
        .map(|(i, t)| Circ {
            tag: *i,
            label: field_of(t, "circuit").unwrap_or_default().to_string(),
            panel: field_of(t, "panel").unwrap_or_default().to_string(),
            level: t.level.clone(),
            devs: Vec::new(),
        })
        .collect();
    let mut by_label: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (ci, c) in circs.iter().enumerate() {
        by_label.entry((c.level.to_ascii_lowercase(), c.label.to_ascii_uppercase())).or_default().push(ci);
    }
    let near = NEAR_RUN_FT * upf;
    let mut no_circuit = 0usize;
    let mut no_tag = 0usize;
    for (di, dv) in devs.iter().enumerate() {
        if dv.circuit.is_empty() {
            no_circuit += 1;
            continue;
        }
        let Some(cands) = by_label.get(&(dv.level.to_ascii_lowercase(), dv.circuit.to_ascii_uppercase())) else {
            no_tag += 1;
            continue;
        };
        let room_first = |ci: &usize| circuit_tags.get(circs.get(*ci).map_or(usize::MAX, |c| c.tag)).is_none_or(|(_, t)| !t.id.contains(":room-"));
        let dist_to = |ci: usize| {
            circs
                .get(ci)
                .and_then(|c| sp.tags.get(c.tag))
                .and_then(|t| pts_of.get(t.id.as_str()))
                .map_or(f64::INFINITY, |ps| ps.iter().map(|p| p.dist(dv.at)).fold(f64::INFINITY, f64::min))
        };
        let best = cands
            .iter()
            .copied()
            .map(|ci| (room_first(&ci), dist_to(ci), ci))
            .min_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).unwrap_or(std::cmp::Ordering::Equal));
        // A typical room's circuit takes only the devices by its route; a building circuit takes
        // its label's devices wherever they are on its level.
        let chosen =
            best.and_then(|(building, dd, ci)| (dd <= near || building).then_some(ci)).or_else(|| cands.iter().copied().find(|ci| room_first(ci)));
        match chosen.and_then(|ci| circs.get_mut(ci)) {
            Some(c) => c.devs.push(di),
            None => no_tag += 1,
        }
    }

    // -- What each level gives: floor, ceiling tiers, corridors. --
    let tiers_of = |level: &str| -> (f64, Vec<f64>) {
        let Some(l) = sp.level(level) else { return (0.0, vec![rl.route.default_ceiling_ft * upf]) };
        let floor = l.elevation_ft * upf;
        let ceiling = if l.ceiling_ft > 0.0 { l.ceiling_ft } else { rl.route.default_ceiling_ft };
        let z0 = floor + (ceiling + rl.route.above_ceiling_in / 12.0) * upf;
        let zmax = floor + (l.height_ft.max(ceiling + 1.0) - rl.route.structure_depth_in / 12.0) * upf;
        let step = (rl.route.tier_step_in.max(1.0) / 12.0) * upf;
        let mut tiers = vec![z0];
        while let Some(&z) = tiers.last() {
            if z + step > zmax || tiers.len() >= 12 {
                break;
            }
            tiers.push(z + step);
        }
        (floor, tiers)
    };
    let corridors = |level: &str| -> Vec<[[f64; 2]; 2]> {
        sp.circulation
            .iter()
            .filter(|c| same_level(&c.level, level) && matches!(c.kind, cadaddy_walk3d::CirculationKind::Corridor))
            .flat_map(|c| c.rects.iter().copied())
            .take(2_000)
            .collect()
    };

    // -- The methods: the documents' table by area, the project rule, the default. --
    let rule_method = d
        .project_rules
        .get(cadaddy_doc::projrules::RuleDomain::WiringMethod, "general")
        .map(|r| r.active.value.trim().to_string())
        .filter(|v| !v.is_empty());
    let has = |text: &str, words: &[String]| words.iter().any(|w| text.contains(w.as_str()));
    let app_for = |area: &str| -> Option<&MethodApp> {
        let mr = &rl.methods;
        let branch: Vec<&MethodApp> = methods
            .applications
            .iter()
            .filter(|a| {
                let up = a.application.to_ascii_uppercase();
                !has(&up, &mr.feeder_words) || has(&up, &mr.branch_words)
            })
            .collect();
        let pick = |pred: &dyn Fn(&str) -> bool| branch.iter().copied().find(|a| pred(&a.application.to_ascii_uppercase()));
        match area {
            "unit" => pick(&|u| has(u, &mr.unit_words))
                .or_else(|| pick(&|u| has(u, &mr.concealed_words) && !has(u, &mr.exposed_words) && !has(u, &mr.below_words))),
            "exposed" => pick(&|u| has(u, &mr.exposed_words) && !has(u, &mr.unit_words)),
            _ => pick(&|u| has(u, &mr.concealed_words) && !has(u, &mr.exposed_words) && !has(u, &mr.unit_words) && !has(u, &mr.below_words)),
        }
    };
    let method_for = |area: &str| -> (String, String, bool, Vec<String>) {
        if let Some(v) = &rule_method {
            let ar = super::mepatoms::rules();
            if let Some((k, _)) = ar.raceway_named(v) {
                return (k.to_string(), format!("project rule wiring_method/general: {v}"), false, Vec::new());
            }
            if let Some((k, _)) = ar.cable_named(v) {
                return (k.to_string(), format!("project rule wiring_method/general: {v}"), true, Vec::new());
            }
        }
        if let Some(a) = app_for(area) {
            let mr = &rl.methods;
            let key = mr.prefer.iter().find(|k| a.methods.contains(k)).or_else(|| a.methods.first());
            if let Some(name) = key.and_then(|k| mr.name_of(k)) {
                let cable = super::mepatoms::rules().raceway_named(name).is_none();
                let allowed: Vec<String> = a.methods.iter().filter_map(|k| mr.name_of(k)).map(str::to_string).collect();
                return (name.to_string(), format!("spec {} {}: {}: {}", a.section, a.item, a.application, allowed.join(", ")), cable, allowed);
            }
        }
        (
            rl.methods.raceway.clone(),
            format!("company default {} (no wiring-methods table read for this area)", rl.methods.raceway),
            false,
            Vec::new(),
        )
    };
    let min_in = methods.min_size_in.filter(|v| v.is_finite() && *v > 0.0).unwrap_or(rl.methods.minimum_size_in.max(0.5));
    let min_src = if methods.min_size_in.is_some() { methods.min_size_source.clone() } else { rl.methods.minimum_source.clone() };
    let max_group = methods.max_circuits.unwrap_or(rl.grouping.max_circuits).clamp(1, 12);
    let group_src = if methods.max_circuits.is_some() { methods.max_circuits_source.clone() } else { rl.grouping.source.clone() };
    let texts = drawing_texts(d);
    let (len_table, len_src) = length_table(&texts);
    let trip_of = |panel: &str, label: &str| -> Option<f64> {
        let num = super::printcond::parse_circuit(label)?.1;
        let first = num.split(',').next()?.trim().to_string();
        let s = sp.mep.panel_schedules.iter().find(|p| p.panel.trim().eq_ignore_ascii_case(panel.trim()))?;
        let c = s.circuits.iter().find(|c| c.number.trim() == first)?;
        let digits: String = c.breaker.chars().skip_while(|ch| !ch.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
        digits.parse::<f64>().ok().filter(|a| (5.0..=4000.0).contains(a))
    };

    // -- Route each circuit. --
    struct Home {
        circ: usize,
        root: usize,
        gear: Option<usize>,
        level: String,
        len_tree_ft: f64,
        cable: bool,
        method: String,
    }
    let mut homes: Vec<Home> = Vec::new();
    let mut clashes: Vec<Value> = Vec::new();
    let mut penetrations: Vec<Value> = Vec::new();
    let mut unrouted: Vec<Value> = Vec::new();
    let mut methods_used: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut runs: Vec<MepRun> = Vec::new();
    let mut tags: Vec<ElementTag> = Vec::new();
    let mut box_in: HashMap<usize, Vec<(u32, String)>> = HashMap::new();
    let mut in_wall_legs = 0usize;
    let mut ceiling_legs = 0usize;
    let r_of = |size_in: f64| size_in / 24.0 * upf;
    let add_clash = |clashes: &mut Vec<Value>, field: &Field, b: &Blocked, what: &str, circuit: &str, level: &str, size_in: f64| {
        if clashes.len() >= MAX_ITEMS {
            return;
        }
        let Some(o) = field.obs.get(b.obstacle) else { return };
        let (hx, at) = (o.handle.hex(), [r2(b.at.x), r2(b.at.y)]);
        if clashes
            .iter()
            .any(|c| c["b"]["handle"] == json!(hx) && c["a"]["circuit"] == json!(circuit) && c["at"][0] == json!(at[0]) && c["at"][1] == json!(at[1]))
        {
            return;
        }
        let need = rl.clearance_in(&o.category);
        let floor = sp.level(level).map_or(0.0, |l| l.elevation_ft * upf);
        let over = (o.hi.z + (need + size_in / 2.0) / 12.0 * upf - floor) / upf;
        let under = (o.lo.z - (need + size_in / 2.0) / 12.0 * upf - floor) / upf;
        clashes.push(json!({
            "kind": "clash",
            "a": {"what": what, "circuit": circuit, "trade": "electrical", "system": BRANCH_SYSTEM},
            "b": {"handle": o.handle.hex(), "category": o.category, "system": o.system, "trade": o.trade},
            "pair": format!("electrical × {}", o.trade),
            "level": level,
            "at": [r2(b.at.x), r2(b.at.y), r2((b.z - floor) / upf)],
            "clearance_in": need,
            "fix": format!(
                "no square route at the level's tiers clears the {} {}: offset under it (bottom of conduit {:.2} ft AFF) or over it ({:.2} ft AFF), or coordinate a sleeve with the {} trade",
                o.category, o.system, under, over, o.trade
            ),
        }));
    };
    for (ci, c) in circs.iter().enumerate() {
        if runs.len() >= MAX_RUNS {
            break;
        }
        let Some(tag) = sp.tags.get(c.tag) else { continue };
        let members: Vec<&Dev> = c.devs.iter().filter_map(|i| devs.get(*i)).collect();
        if members.is_empty() {
            unrouted.push(json!({"circuit": c.label, "level": c.level, "reason": "no device of this circuit is in the model"}));
            continue;
        }
        let field = fields.get(&c.level.to_ascii_lowercase()).unwrap_or(&empty);
        let (floor, tiers) = tiers_of(&c.level);
        // The area: a typical room's own (a unit), an exposed room's, else concealed.
        let room_up = members.iter().map(|m| m.room.to_ascii_uppercase()).collect::<Vec<_>>().join(" ");
        let area = if tag.id.contains(":room-") || has(&room_up, &rl.methods.unit_words) {
            "unit"
        } else if has(&room_up, &rl.methods.exposed_rooms) {
            "exposed"
        } else {
            "concealed"
        };
        let (method, msrc, cable, allowed) = method_for(area);
        let e = methods_used.entry(format!("{area}: {method}")).or_insert((0, msrc.clone()));
        e.0 += 1;
        // The panel: this level's gear of the circuit's panel nearest the devices.
        let centre = members.iter().fold(Vec2::new(0.0, 0.0), |s, m| s + m.at) * (1.0 / members.len() as f64);
        let room_circuit = tag.id.contains(":room-");
        let panel_up = c.panel.to_ascii_uppercase();
        let gi = gear
            .iter()
            .enumerate()
            .filter(|(_, g)| {
                let t = g.tag.to_ascii_uppercase();
                same_level(&g.level, &c.level) && !panel_up.is_empty() && (t == panel_up || t.starts_with(&format!("{panel_up}-")))
            })
            .map(|(i, g)| (i, Vec2::new((g.lo.x + g.hi.x) / 2.0, (g.lo.y + g.hi.y) / 2.0).dist(centre)))
            .filter(|(_, dd)| !room_circuit || *dd <= ROOM_PANEL_FT * upf)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);
        let panel_at = gi.and_then(|i| gear.get(i)).map(|g| Vec2::new((g.lo.x + g.hi.x) / 2.0, (g.lo.y + g.hi.y) / 2.0));
        let pts: Vec<Vec2> = members.iter().map(|m| m.at).collect();
        let root = panel_at
            .and_then(|p| pts.iter().enumerate().min_by(|a, b| manhattan(*a.1, p).total_cmp(&manhattan(*b.1, p))).map(|(i, _)| i))
            .unwrap_or(0);
        let tree = spanning_tree(&pts, root);
        let size = min_in;
        let cid = tag.id.clone();
        let mut leg_n = 0usize;
        let mut tree_len = 0.0;
        let mut edge_ft: Vec<(usize, usize, f64)> = Vec::new();
        // The circuit's own makeup per leg (two conductors and a ground, or what the tag says).
        let makeup = field_of(tag, "conductors").unwrap_or(super::printcond::DEFAULT_CONDUCTORS).to_string();
        let parts = super::mepatoms::conductor_parts(&makeup);
        let hots: Vec<(u32, String)> = parts.iter().filter(|p| !p.2).map(|p| (p.0, p.1.clone())).collect();
        for (u, v) in &tree {
            let (Some(a), Some(b)) = (members.get(*u), members.get(*v)) else { continue };
            leg_n += 1;
            let before = tree_len;
            let same_wall = match (a.face, b.face) {
                (Some(fa), Some(fb)) => {
                    fa.dot(fb) > 0.9 && (b.at - a.at).dot(fa).abs() < 0.75 * upf && a.at.dist(b.at) <= rl.route.in_wall_max_ft * upf
                }
                _ => false,
            };
            let mut r = super::printcond::run(BRANCH_SYSTEM, &c.level, &tag_sheet(tag), Vec::new(), size, 0.0);
            if same_wall {
                // Along the wall between the boxes: through the bored studs above them.
                in_wall_legs += 1;
                let zw = a.top.max(b.top) + rl.route.in_wall_above_box_in / 12.0 * upf;
                r.points = vec![[a.at.x, a.at.y], [a.at.x, a.at.y], [b.at.x, b.at.y], [b.at.x, b.at.y]];
                // Along its wall, whichever way the wall runs.
                r.source_sheet = IN_WALL.into();
                r.bottoms_ft = vec![(a.top - floor) / upf, (zw - floor) / upf, (b.top - floor) / upf];
                tree_len += a.at.dist(b.at) / upf + 2.0 * (zw - a.top.min(b.top)).abs() / upf;
            } else {
                ceiling_legs += 1;
                let leg = route_leg(a.at, b.at, &tiers, r_of(size), field, upf, &[], rl);
                for bl in &leg.blocked {
                    add_clash(&mut clashes, field, bl, "branch conduit", &c.label, &c.level, size);
                }
                let mut pts = vec![[a.at.x, a.at.y]];
                pts.extend(leg.pts.iter().map(|p| [p.x, p.y]));
                pts.push([b.at.x, b.at.y]);
                let mut bot = vec![(a.top - floor) / upf];
                bot.extend(leg.z.iter().map(|z| (z - floor) / upf));
                bot.push((b.top - floor) / upf);
                r.points = pts;
                r.bottoms_ft = bot;
                let up = leg.z.first().copied().unwrap_or(a.top);
                let down = leg.z.last().copied().unwrap_or(b.top);
                tree_len += poly_len(&leg.pts) / upf + ((up - a.top).abs() + (down - b.top).abs()) / upf;
                penetrate(&mut penetrations, &rated, &c.level, &leg, &c.label, size, upf, floor, rl);
            }
            edge_ft.push((*u, *v, tree_len - before));
            r.id = format!("{cid}#{leg_n}");
            r.size_source = format!("minimum: {min_src}");
            r.height_source = ROUTED.into();
            runs.push(r);
            box_in.entry(c.devs.get(*u).copied().unwrap_or(usize::MAX)).or_default().extend(hots.iter().cloned());
            box_in.entry(c.devs.get(*v).copied().unwrap_or(usize::MAX)).or_default().extend(hots.iter().cloned());
        }
        let mut t = tag.clone();
        t.set("route", ROUTED, "computed");
        t.set(
            "conduit",
            method.clone(),
            if msrc.starts_with("spec") {
                "spec"
            } else if msrc.starts_with("project") {
                "rule"
            } else {
                "default"
            },
        );
        t.set("wiring_method", format!("{method}: {msrc}"), "computed");
        if !allowed.is_empty() {
            t.set("methods_allowed", allowed.join(", "), "spec");
        }
        t.set("area", area, "computed");
        t.set("devices", members.len().to_string(), "computed");
        let gi_other = gi
            .is_none()
            .then(|| gear.iter().find(|g| !panel_up.is_empty() && g.tag.eq_ignore_ascii_case(&c.panel)).map(|g| g.level.clone()))
            .flatten();
        match (gi, gi_other) {
            (Some(_), _) => {}
            (None, Some(other)) => unrouted.push(json!({"circuit": c.label, "level": c.level, "reason": format!("its panel {} is on {other}: the riser to it is not routed", c.panel)})),
            (None, None) => unrouted.push(json!({"circuit": c.label, "level": c.level, "reason": format!("panel {} is not in the model: no homerun", c.panel)})),
        }
        // The farthest device along the joints from the root: the voltage drop's length.
        let mut far = vec![f64::NAN; members.len()];
        if let Some(x) = far.get_mut(root) {
            *x = 0.0;
        }
        for _ in 0..members.len() {
            let mut changed = false;
            for (u, v, ft) in &edge_ft {
                let (du, dv) = (far.get(*u).copied().unwrap_or(f64::NAN), far.get(*v).copied().unwrap_or(f64::NAN));
                if du.is_finite() && !dv.is_finite() {
                    if let Some(x) = far.get_mut(*v) {
                        *x = du + ft;
                        changed = true;
                    }
                } else if dv.is_finite() && !du.is_finite() {
                    if let Some(x) = far.get_mut(*u) {
                        *x = dv + ft;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        let farthest = far.iter().copied().filter(|v| v.is_finite()).fold(0.0, f64::max);
        homes.push(Home { circ: ci, root, gear: gi, level: c.level.clone(), len_tree_ft: farthest, cable, method });
        tags.push(t);
    }

    // -- Homeruns: grouped by panel and nearness, each from its lead's box to the panel's top. --
    let mut slots_used: HashMap<usize, f64> = HashMap::new();
    let mut conflicts: Vec<Value> = Vec::new();
    let mut entry_unknown: BTreeSet<String> = BTreeSet::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    {
        let mut order: Vec<usize> = (0..homes.len()).filter(|i| homes.get(*i).is_some_and(|h| h.gear.is_some())).collect();
        let root_at = |h: &Home| circs.get(h.circ).and_then(|c| c.devs.get(h.root)).and_then(|d| devs.get(*d)).map(|d| d.at);
        // By panel and method, the root nearest its panel first: it leads its group.
        let to_gear = |h: &Home| {
            let g = h.gear.and_then(|i| gear.get(i)).map(|g| Vec2::new((g.lo.x + g.hi.x) / 2.0, (g.lo.y + g.hi.y) / 2.0));
            match (root_at(h), g) {
                (Some(p), Some(q)) => manhattan(p, q),
                _ => f64::INFINITY,
            }
        };
        order.sort_by(|a, b| {
            let (Some(ha), Some(hb)) = (homes.get(*a), homes.get(*b)) else { return std::cmp::Ordering::Equal };
            (ha.gear, ha.method.as_str()).cmp(&(hb.gear, hb.method.as_str())).then(to_gear(ha).total_cmp(&to_gear(hb)))
        });
        let reach = rl.grouping.reach_ft.max(0.0) * upf;
        for i in order {
            let Some(h) = homes.get(i) else { continue };
            let here = root_at(h);
            let joined = groups.iter_mut().find(|g| {
                let Some(lead) = g.first().and_then(|j| homes.get(*j)) else { return false };
                !h.cable
                    && !lead.cable
                    && g.len() < max_group
                    && lead.gear == h.gear
                    && lead.method == h.method
                    && match (root_at(lead), here) {
                        (Some(p), Some(q)) => manhattan(p, q) <= reach,
                        _ => false,
                    }
            });
            match joined {
                Some(g) => g.push(i),
                None => groups.push(vec![i]),
            }
        }
    }
    let mut grouped_circuits = 0usize;
    let mut upsized: Vec<Value> = Vec::new();
    let mut fill_rows: Vec<Value> = Vec::new();
    let mut derated: Vec<Value> = Vec::new();
    let mut corridor_homes = 0usize;
    for (gn, g) in groups.iter().enumerate() {
        if runs.len() >= MAX_RUNS {
            break;
        }
        let Some(lead_h) = g.first().and_then(|i| homes.get(*i)) else { continue };
        let Some(lead_c) = circs.get(lead_h.circ) else { continue };
        let Some(gear_el) = lead_h.gear.and_then(|i| gear.get(i)) else { continue };
        let Some(lead_dev) = lead_c.devs.get(lead_h.root).and_then(|d| devs.get(*d)) else { continue };
        let field = fields.get(&lead_h.level.to_ascii_lowercase()).unwrap_or(&empty);
        let (floor, tiers) = tiers_of(&lead_h.level);
        let gctr = Vec2::new((gear_el.lo.x + gear_el.hi.x) / 2.0, (gear_el.lo.y + gear_el.hi.y) / 2.0);
        // Members tap into the lead's box.
        let mut labels: Vec<String> = Vec::new();
        let mut group_parts: Vec<Part> = Vec::new();
        let mut longest = 0.0_f64;
        let mut trips: Vec<f64> = Vec::new();
        let mut path_len_ft = 0.0;
        // The path to the panel first (its length sizes the conductors).
        let lane = corridors(&lead_h.level);
        let far = manhattan(lead_dev.at, gctr) >= rl.route.corridor_min_ft * upf;
        let plan: Vec<Vec2> = match (far && !lane.is_empty()).then(|| corridor_path(&lane, lead_dev.at, gctr, upf)).flatten() {
            Some(p) => {
                corridor_homes += 1;
                p
            }
            None => vec![lead_dev.at, gctr],
        };
        // Each square piece of the plan path seated clear of the obstacles (the gear itself is
        // the target, not an obstacle).
        let mut home_pts: Vec<Vec2> = vec![lead_dev.at];
        let mut home_z: Vec<f64> = Vec::new();
        let mut blocked: Vec<Blocked> = Vec::new();
        let skip = [gear_el.handle];
        // The raceway's size comes after the fill: route at the minimum's radius (fill rarely
        // makes a branch homerun more than 1").
        for w in plan.windows(2) {
            let [a, b] = w else { continue };
            let leg = route_leg(*a, *b, &tiers, r_of(min_in.max(1.0)), field, upf, &skip, rl);
            home_pts.extend(leg.pts.iter().skip(1).copied());
            home_z.extend(leg.z.iter().copied());
            blocked.extend(leg.blocked);
        }
        path_len_ft += poly_len(&home_pts) / upf;
        for i in g {
            let Some(h) = homes.get(*i) else { continue };
            let Some(c) = circs.get(h.circ) else { continue };
            let Some(tag) = tags.get(*i) else { continue };
            labels.push(c.label.clone());
            let mk = field_of(tag, "conductors").unwrap_or(super::printcond::DEFAULT_CONDUCTORS).to_string();
            let trip = trip_of(&c.panel, &c.label);
            trips.push(trip.unwrap_or(rl.vdrop.breaker_a));
            // The member's own tap from its root's box to the lead's box.
            let mut tap_ft = 0.0;
            if *i != g.first().copied().unwrap_or(usize::MAX)
                && let Some(md) = c.devs.get(h.root).and_then(|d| devs.get(*d))
            {
                let leg = route_leg(md.at, lead_dev.at, &tiers, r_of(min_in), field, upf, &[], rl);
                for bl in &leg.blocked {
                    add_clash(&mut clashes, field, bl, "homerun tap", &c.label, &c.level, min_in);
                }
                let mut r = super::printcond::run(BRANCH_SYSTEM, &c.level, &tag_sheet(tag), Vec::new(), min_in, 0.0);
                let mut pts = vec![[md.at.x, md.at.y]];
                pts.extend(leg.pts.iter().map(|p| [p.x, p.y]));
                pts.push([lead_dev.at.x, lead_dev.at.y]);
                let mut bot = vec![(md.top - floor) / upf];
                bot.extend(leg.z.iter().map(|z| (z - floor) / upf));
                bot.push((lead_dev.top - floor) / upf);
                r.points = pts;
                r.bottoms_ft = bot;
                r.id = format!("{}#tap", tag.id);
                r.size_source = format!("minimum: {min_src}");
                r.height_source = ROUTED.into();
                tap_ft = poly_len(&leg.pts) / upf + 2.0 * ((tiers.first().copied().unwrap_or(0.0) - lead_dev.top).abs() / upf);
                runs.push(r);
                penetrate(&mut penetrations, &rated, &c.level, &leg, &c.label, min_in, upf, floor, rl);
                let hots: Vec<(u32, String)> = super::mepatoms::conductor_parts(&mk).into_iter().filter(|p| !p.2).map(|p| (p.0, p.1)).collect();
                box_in.entry(c.devs.get(h.root).copied().unwrap_or(usize::MAX)).or_default().extend(hots.iter().cloned());
                box_in.entry(lead_c.devs.get(lead_h.root).copied().unwrap_or(usize::MAX)).or_default().extend(hots);
            }
            // Its conductors by the homerun's length (the legend's bands, else the drop).
            let ft = path_len_ft + tap_ft + h.len_tree_ft;
            longest = longest.max(ft);
            let mut parts = super::mepatoms::conductor_parts(&mk);
            let volts = rl.vdrop.voltage;
            let amps = trip.unwrap_or(rl.vdrop.breaker_a) * rl.vdrop.load_pct / 100.0;
            if let Some((need, why)) = size_for_length(ft, amps, volts, &len_table, rl) {
                let order = |s: &str| rl.vdrop.sizes.iter().position(|x| x == s).unwrap_or(0);
                let mut changed = false;
                for p in &mut parts {
                    if order(&p.1) < order(&need) {
                        p.1 = need.clone();
                        changed = true;
                    }
                }
                if changed {
                    let now = makeup_text(&parts);
                    if upsized.len() < MAX_ITEMS {
                        upsized.push(json!({"circuit": c.label, "level": c.level, "feet": r1(ft), "from": mk, "to": now, "why": why}));
                    }
                    if let Some(t) = tags.get_mut(*i) {
                        t.set("conductors", now, "computed");
                        t.set("conductor_size", why.clone(), if why.starts_with("legend") { "legend" } else { "rule" });
                    }
                }
            }
            if let Some(t) = tags.get_mut(*i) {
                t.set("homerun_ft", format!("{:.1}", ft), "computed");
                t.set(
                    "homerun",
                    format!(
                        "to panel {} ({})",
                        c.panel,
                        if g.len() > 1 { format!("sharing with {} other circuit(s)", g.len() - 1) } else { "its own raceway".to_string() }
                    ),
                    "computed",
                );
                t.set("homerun_group", format!("{}:{gn}", gear_el.tag), "computed");
            }
            group_parts.extend(parts.into_iter().filter(|p| !p.2));
        }
        if g.len() > 1 {
            grouped_circuits += g.len();
        }
        // One ground, the largest size of the group (NEC 250.122: the circuit's largest).
        let order = |s: &str| rl.vdrop.sizes.iter().position(|x| x == s).unwrap_or(0);
        let ground = group_parts.iter().map(|p| p.1.clone()).max_by_key(|s| order(s)).unwrap_or_else(|| "12".into());
        // The 310.15(C)(1) adjustment: each circuit's conductors carry current.
        let ccc: u32 = group_parts.iter().fold(0u32, |s, p| s.saturating_add(p.0));
        let trip = trips.iter().copied().fold(0.0, f64::max).max(1.0);
        let mut steps = 0;
        while group_parts.iter().any(|p| allowed_a(&p.1, ccc, rl) + 1e-9 < trip) && steps < 3 {
            for p in &mut group_parts {
                if allowed_a(&p.1, ccc, rl) + 1e-9 < trip {
                    p.1 = larger(&p.1, rl);
                }
            }
            steps += 1;
        }
        if ccc > 3 && derated.len() < MAX_ITEMS {
            let f = adjustment(ccc, rl);
            derated.push(json!({
                "panel": gear_el.tag, "circuits": labels, "ccc": ccc, "factor": f,
                "allowed_a": group_parts.iter().map(|p| r1(allowed_a(&p.1, ccc, rl))).fold(f64::INFINITY, f64::min),
                "breaker_a": trip, "upsized": steps > 0, "source": rl.derate.source,
            }));
        }
        let mut all = group_parts.clone();
        all.push((1, ground, true));
        // Combine like sizes: `6#12 + 1#12G`.
        let mut combined: Vec<Part> = Vec::new();
        for p in all {
            match combined.iter_mut().find(|q| q.1 == p.1 && q.2 == p.2) {
                Some(q) => q.0 += p.0,
                None => combined.push(p),
            }
        }
        let gmakeup = makeup_text(&combined);
        let (size_in, ssrc) = if lead_h.cable {
            (min_in, format!("{} cable: no raceway fill", lead_h.method))
        } else {
            match fill_size(&combined, &lead_h.method, min_in, rl) {
                Some((t, area, allowed)) => (
                    t,
                    format!(
                        "computed: {} sq in of conductors in {} sq in allowed ({}); not under the minimum: {min_src}",
                        r3(area),
                        r3(allowed),
                        rl.fill.source
                    ),
                ),
                None => (4.0, format!("over the table's largest raceway: split this homerun ({})", rl.fill.source)),
            }
        };
        if fill_rows.len() < MAX_ITEMS {
            fill_rows.push(json!({"panel": gear_el.tag, "level": lead_h.level, "circuits": labels, "conductors": gmakeup, "method": lead_h.method, "size_in": size_in, "source": ssrc}));
        }
        // The panel's top entry window: each raceway at its own slot along it.
        let eq = sp.mep.equipment.iter().find(|e| e.tag.eq_ignore_ascii_case(&gear_el.tag) && same_level(&e.level, &gear_el.level));
        let window = eq.and_then(|e| e.enclosure.as_ref()).map(|en| en.top_entry_in).filter(|w| w[0] > 0.0 && w[1] > 0.0);
        let area_text = eq.and_then(|e| e.properties.get("TOP CONDUIT AREA")).cloned();
        let bottom_fed = eq.and_then(|e| e.properties.get("INCOMING LOCATION")).is_some_and(|v| v.trim().to_ascii_lowercase().starts_with("bottom"));
        let od = rl.entry.locknut_od_in.get(&format!("{size_in:?}")).copied().unwrap_or(size_in + 1.0);
        let pitch = (od + rl.entry.spacing_in) / 12.0 * upf;
        let land = match window {
            Some([w, dpt]) => {
                // Slots row by row across the window (W along the box's long side, D across it).
                let (w_u, d_u) = (w / 12.0 * upf, dpt / 12.0 * upf);
                let k = slots_used.entry(lead_h.gear.unwrap_or(usize::MAX)).or_insert(0.0);
                let per_row = (w_u / pitch).floor().max(1.0);
                let rows = (d_u / pitch).floor().max(1.0);
                let (col, row) = (*k % per_row, (*k / per_row).floor());
                if row >= rows && conflicts.len() < MAX_ITEMS {
                    conflicts.push(json!({
                        "kind": "conflict", "gear": gear_el.tag, "level": gear_el.level, "side": "top",
                        "window_in": [w, dpt], "window": area_text.clone().unwrap_or_default(),
                        "raceways": *k + 1.0, "holds": per_row * rows, "pitch_in": r2(pitch / upf * 12.0),
                        "fix": "the top entry window can't hold another raceway: add an extension gutter or wireway, a pull box above the panel, a larger can, or split the entry",
                    }));
                }
                *k += 1.0;
                let along = (-w_u / 2.0 + pitch * (col + 0.5)).min(w_u / 2.0);
                let across = (-d_u / 2.0 + pitch * (row.min(rows - 1.0) + 0.5)).min(d_u / 2.0);
                let wide = (gear_el.hi.x - gear_el.lo.x) >= (gear_el.hi.y - gear_el.lo.y);
                if wide { Vec2::new(gctr.x + along, gctr.y + across) } else { Vec2::new(gctr.x + across, gctr.y + along) }
            }
            None => {
                entry_unknown.insert(format!("{} on {}", gear_el.tag, gear_el.level));
                gctr
            }
        };
        if bottom_fed && window.is_none() && conflicts.len() < MAX_ITEMS {
            conflicts.push(json!({"kind": "conflict", "gear": gear_el.tag, "level": gear_el.level, "side": "top", "fix": "the panel is bottom fed and its submittal gives no top conduit area: confirm where the branch raceways may enter"}));
        }
        // Last piece: square over to the slot, then down onto the panel's top.
        if let Some(last) = home_pts.last().copied()
            && last.dist(land) > upf / 96.0
        {
            home_pts.pop();
            let z = home_z.pop().unwrap_or(tiers.first().copied().unwrap_or(gear_el.hi.z));
            let prev = home_pts.last().copied().unwrap_or(last);
            let mut tail = vec![prev, Vec2::new(land.x, prev.y), land];
            tail = tidy(tail, upf / 96.0);
            for _ in 1..tail.len() {
                home_z.push(z);
            }
            home_pts.extend(tail.into_iter().skip(1));
        }
        for bl in &blocked {
            add_clash(&mut clashes, field, bl, "homerun", &labels.join(","), &lead_h.level, size_in);
        }
        let gid = format!(
            "{}{}{HOME_PART}{}:{gn}",
            super::printcond::CIRCUIT_TAG,
            lead_h.level.replace(['/', '#', '@'], "-"),
            gear_el.tag.replace(['/', '#', '@'], "-")
        );
        let mut r = super::printcond::run(BRANCH_SYSTEM, &lead_h.level, "", Vec::new(), size_in, 0.0);
        let mut pts = vec![[lead_dev.at.x, lead_dev.at.y]];
        pts.extend(home_pts.iter().map(|p| [p.x, p.y]));
        pts.push([land.x, land.y]);
        let mut bot = vec![(lead_dev.top - floor) / upf];
        bot.extend(home_z.iter().map(|z| (z - floor) / upf));
        bot.push((gear_el.hi.z - floor) / upf);
        r.points = pts;
        r.bottoms_ft = bot;
        r.id = format!("{gid}#home");
        r.size_source = ssrc.clone();
        r.height_source = ROUTED.into();
        let seat_leg = Leg { pts: home_pts.clone(), z: home_z.clone(), blocked: Vec::new() };
        penetrate(&mut penetrations, &rated, &lead_h.level, &seat_leg, &labels.join(","), size_in, upf, floor, rl);
        runs.push(r);
        box_in
            .entry(lead_c.devs.get(lead_h.root).copied().unwrap_or(usize::MAX))
            .or_default()
            .extend(combined.iter().filter(|p| !p.2).map(|p| (p.0, p.1.clone())));
        // The homerun's own tag: its circuits (one list: panel and numbers), conductors, raceway.
        let numbers: Vec<String> = labels.iter().filter_map(|l| super::printcond::parse_circuit(l).map(|(_, n)| n)).collect();
        let panel_name = circs.get(lead_h.circ).map(|c| c.panel.clone()).unwrap_or_default();
        let mut t = ElementTag { id: gid, kind: "circuit".into(), level: lead_h.level.clone(), ..ElementTag::default() };
        t.at_ft = [lead_dev.at.x / upf, lead_dev.at.y / upf];
        t.set("circuit", format!("{panel_name}-{}", numbers.join(",")), "computed");
        t.set("circuits", labels.join(", "), "computed");
        t.set("panel", panel_name, "plan");
        t.set("conductors", gmakeup, "computed");
        t.set("conduit", lead_h.method.clone(), "computed");
        t.set("route", ROUTED, "computed");
        t.set(
            "homerun",
            format!(
                "to panel {} ({})",
                gear_el.tag,
                if window.is_some() { "in its top entry window" } else { "entry window unknown: centred on its top, derived (centered)" }
            ),
            "computed",
        );
        t.set("max_circuits", format!("{max_group}: {group_src}"), if methods.max_circuits.is_some() { "spec" } else { "rule" });
        t.set("homerun_ft", format!("{longest:.1}"), "computed");
        tags.push(t);
    }

    // -- The gear's dedicated space (NEC 110.26(E)(1)): its footprint from its top 6 ft up (or to
    // the structure) is the electrical installation's; another trade's duct or pipe there is a
    // clash item. --
    let mut gear_space = 0usize;
    for g in gear.iter().take(MAX_DEVICES) {
        let field = fields.get(&g.level.to_ascii_lowercase()).unwrap_or(&empty);
        let top = g.hi.z + 6.0 * upf;
        let mut seen: BTreeSet<Handle> = BTreeSet::new();
        for o in &field.obs {
            let foreign = matches!(o.trade, "mechanical" | "plumbing" | "fire");
            let over = o.lo.x < g.hi.x && o.hi.x > g.lo.x && o.lo.y < g.hi.y && o.hi.y > g.lo.y && o.hi.z > g.hi.z && o.lo.z < top;
            if foreign && over && seen.insert(o.handle) && clashes.len() < MAX_ITEMS {
                gear_space += 1;
                let floor = sp.level(&g.level).map_or(0.0, |l| l.elevation_ft * upf);
                clashes.push(json!({
                    "kind": "clash",
                    "a": {"what": "gear dedicated space", "circuit": "", "trade": "electrical", "gear": g.tag, "handle": g.handle.hex()},
                    "b": {"handle": o.handle.hex(), "category": o.category, "system": o.system, "trade": o.trade},
                    "pair": format!("electrical × {}", o.trade),
                    "level": g.level,
                    "at": [r2((g.lo.x + g.hi.x) / 2.0), r2((g.lo.y + g.hi.y) / 2.0), r2((o.lo.z.max(g.hi.z) - floor) / upf)],
                    "clearance_in": 0.0,
                    "fix": format!("the {} {} is in panel {}'s dedicated electrical space (its footprint to 6 ft above it, NEC 110.26(E)(1)): move it out of that column, or add the protection 110.26(E)(1)(b) asks for", o.category, o.system, g.tag),
                }));
            }
        }
    }

    // -- Box fill (314.16): what enters each device's box, checked against the box the pieces
    // give it ([`check_box_fill`], once they are cut). --
    let box_conductors: Vec<Value> = box_in
        .iter()
        .filter_map(|(di, conds)| {
            let dv = devs.get(*di)?;
            Some(json!({"device": dv.handle.hex(), "kind": dv.kind, "level": dv.level, "circuit": dv.circuit, "yoke": dv.yoke, "conductors": conds.iter().map(|(n, s)| json!([n, s])).collect::<Vec<_>>()}))
        })
        .take(MAX_DEVICES)
        .collect();

    // -- The runs and tags out; the report. --
    let mut by_pair: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for c in &clashes {
        let pair = c["pair"].as_str().unwrap_or("electrical × ?").to_string();
        let level = c["level"].as_str().unwrap_or_default().to_string();
        *by_pair.entry(pair).or_default().entry(level).or_default() += 1;
    }
    let routed_ft: f64 = runs.iter().map(|r| run_ft(r, upf)).sum();
    let mut by_method: BTreeMap<String, f64> = BTreeMap::new();
    let tag_method: HashMap<&str, &str> = tags.iter().filter_map(|t| Some((t.id.as_str(), field_of(t, "conduit")?))).collect();
    for r in &runs {
        let m = super::printcond::circuit_of_run(r).and_then(|c| tag_method.get(c)).copied().unwrap_or("?");
        *by_method.entry(format!("{m} {}", super::mepatoms::trade_size(r.size_in[0]))).or_default() += run_ft(r, upf);
    }
    let estimated_ft: f64 = sp.mep.runs.iter().filter(|r| r.system == est).map(|r| run_ft(r, upf)).sum();
    // Pieces off both plan axes (none, by construction: checked, not assumed).
    let hair = upf / 12.0;
    let diagonal = runs
        .iter()
        .filter(|r| r.source_sheet != IN_WALL)
        .flat_map(|r| r.points.windows(2))
        .filter(|w| matches!(w, [a, b] if (a[0] - b[0]).abs() > hair && (a[1] - b[1]).abs() > hair))
        .count();
    out.report = json!({
        "circuits": circs.len(),
        "routed_circuits": homes.len(),
        "devices": devs.len(),
        "devices_without_circuit": no_circuit,
        "devices_without_tag": no_tag,
        "runs": runs.len(),
        "in_wall_legs": in_wall_legs,
        "ceiling_legs": ceiling_legs,
        "homeruns": groups.len(),
        "homeruns_by_corridor": corridor_homes,
        "grouped_circuits": grouped_circuits,
        "routed_ft": r1(routed_ft),
        "diagonal_pieces": diagonal,
        "estimated_ft_before": r1(estimated_ft),
        "ft_by_method_size": by_method.iter().map(|(k, v)| (k.clone(), json!(r1(*v)))).collect::<serde_json::Map<_, _>>(),
        "methods": methods_used.iter().map(|(k, (n, s))| json!({"area_method": k, "circuits": n, "source": s})).collect::<Vec<_>>(),
        "wire_size_table": if len_table.is_empty() { json!(null) } else { json!({"bands": len_table.iter().map(|(h, s)| json!([h, s])).collect::<Vec<_>>(), "read": len_src}) },
        "minimum_size_in": min_in,
        "minimum_source": min_src,
        "max_circuits_per_homerun": max_group,
        "max_circuits_source": group_src,
        "clashes": clashes,
        "clashes_by_pair_level": by_pair,
        "gear_space_clashes": gear_space,
        "conflicts": conflicts,
        "entry_unknown": entry_unknown,
        "penetrations": penetrations,
        "box_fill": {"checked": 0, "over": [], "pending": box_conductors},
        "conductors_upsized": upsized,
        "fill": fill_rows,
        "derated": derated,
        "unrouted": unrouted,
        "clearance_source": rl.clearance_source(),
        "route_source": rl.route.source,
    });
    out.runs = runs;
    out.tags = tags;
    out
}

/// The sheet a circuit was read on (its tag's), for the run's source.
fn tag_sheet(t: &ElementTag) -> String {
    field_of(t, "sheet").unwrap_or_default().to_string()
}

/// A run's length, feet (risers by their height).
fn run_ft(r: &MepRun, upf: f64) -> f64 {
    if let Some(top) = r.top_ft {
        return (top - r.bottoms_ft.first().copied().unwrap_or(top)).abs();
    }
    let plan: f64 = r.points.windows(2).map(|w| if let [a, b] = w { (b[0] - a[0]).hypot(b[1] - a[1]) } else { 0.0 }).sum::<f64>() / upf;
    let vert: f64 = r.bottoms_ft.windows(2).map(|w| if let [a, b] = w { (b - a).abs() } else { 0.0 }).sum();
    plan + vert
}

/// The rated walls a seated leg crosses, each a penetration with its firestop.
#[allow(clippy::too_many_arguments)]
fn penetrate(out: &mut Vec<Value>, rated: &[Rated], level: &str, leg: &Leg, circuit: &str, size_in: f64, upf: f64, floor: f64, rl: &RouteRules) {
    for (i, w) in leg.pts.windows(2).enumerate() {
        let [a, b] = w else { continue };
        let z = leg.z.get(i).copied().unwrap_or(0.0);
        for wall in rated.iter().filter(|x| same_level(&x.level, level) && x.top >= z) {
            if out.len() >= MAX_ITEMS {
                return;
            }
            if crosses(*a, *b, &wall.hull) {
                let at = wall.hull.iter().fold(Vec2::new(0.0, 0.0), |s, p| s + *p) * (1.0 / wall.hull.len().max(1) as f64);
                out.push(json!({
                    "wall": wall.handle.hex(), "wall_type": wall.wall_type, "rating": wall.rating, "level": level,
                    "circuit": circuit, "raceway_in": size_in, "at": [r2(at.x), r2(at.y), r2((z - floor) / upf)],
                    "firestop": format!("{} for {} raceway in a {} wall", rl.firestop.item, super::mepatoms::trade_size(size_in), wall.rating),
                    "source": rl.firestop.source,
                }));
            }
        }
    }
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}
fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// Box fill (NEC 314.16) at each device box the routing put conductors in, against the box its
/// pieces gave it (its `cubic_in`): the report's `box_fill` filled in.
pub(crate) fn check_box_fill(d: &Drawing, report: &mut Value) {
    let rl = rules();
    let Some(pending) = report["box_fill"]["pending"].as_array().cloned() else { return };
    let mut boxes: HashMap<String, (String, f64)> = HashMap::new();
    for e in d.model.iter().filter(|e| super::mepatoms::is_atom(e)) {
        let ps = props::read(&e.common.xdata);
        if ps.get(super::mepatoms::ATOM_KEY).map(Prop::text).as_deref() != Some("box") {
            continue;
        }
        let (Some(of), Some(cu)) = (ps.get("box_of").map(Prop::text), ps.get("cubic_in").and_then(|p| p.text().parse::<f64>().ok())) else {
            continue;
        };
        boxes.insert(of.to_ascii_uppercase(), (ps.get("size").map(Prop::text).unwrap_or_default(), cu));
    }
    let (mut checked, mut over) = (0usize, Vec::new());
    for p in &pending {
        let Some((size, cu)) = p["device"].as_str().and_then(|h| boxes.get(&h.to_ascii_uppercase())) else { continue };
        checked += 1;
        let conds: Vec<(u32, String)> = p["conductors"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| Some((u32::try_from(c.get(0)?.as_u64()?).ok()?, c.get(1)?.as_str()?.to_string())))
            .collect();
        let need = box_volume(&conds, true, p["yoke"].as_bool().unwrap_or(false), rl);
        if need > cu + 1e-9 && over.len() < MAX_ITEMS {
            let bigger =
                rl.box_fill.bigger.iter().find(|(from, _, v)| size.contains(from.as_str()) && *v + 1e-9 >= need).map(|(_, to, _)| to.clone());
            over.push(json!({
                "kind": "box_fill", "device": p["device"], "device_kind": p["kind"], "level": p["level"], "circuit": p["circuit"],
                "box": size, "cubic_in": cu, "needed_cu_in": r2(need),
                "conductors": conds.iter().map(|(n, s)| format!("{n}#{s}")).collect::<Vec<_>>(),
                "fix": bigger.unwrap_or_else(|| "a deeper box or an extension ring, or fewer raceways into it (a junction box above)".into()),
                "source": rl.box_fill.source,
            }));
        }
    }
    report["box_fill"] = json!({"checked": checked, "over": over});
}

/// The spec with its circuits routed: the estimated runs of the circuits routed go, the routed
/// runs and the routed tags come in (the drawn conduit stays as it is).
pub(crate) fn apply(sp: &mut BuildingSpec, routed: &Routed) {
    if routed.runs.is_empty() {
        return;
    }
    let routed_tags: BTreeSet<&str> = routed.tags.iter().map(|t| t.id.as_str()).collect();
    sp.mep.runs.retain(|r| {
        let c = super::printcond::circuit_of_run(r);
        !(r.system == BRANCH_SYSTEM || (r.system == super::printcond::CIRCUIT_SYSTEM && c.is_some_and(|c| routed_tags.contains(c))))
    });
    sp.mep.runs.extend(routed.runs.iter().cloned());
    sp.tags.retain(|t| !routed_tags.contains(t.id.as_str()) && !t.id.contains(HOME_PART));
    sp.tags.extend(routed.tags.iter().cloned());
    if !sp.mep.systems.iter().any(|s| s.name == BRANCH_SYSTEM) {
        sp.mep.systems.push(MepSystem {
            name: BRANCH_SYSTEM.into(),
            layer: BRANCH_SYSTEM.into(),
            kind: MepKind::Conduit,
            size_in: [0.75, 0.0],
            bottom_ft: 9.0,
            show: true,
            duct_type: None,
            color_index: 0,
        });
    }
}

/// The switch that keeps `model.build` from routing (`model.build {route: false}`).
const OFF_FILE: &str = "route/off";

pub(crate) fn enabled(d: &Drawing) -> bool {
    !d.embedded.contains_key(OFF_FILE)
}

pub(crate) fn set_enabled(d: &mut Drawing, on: bool) {
    if on {
        d.embedded.remove(OFF_FILE);
    } else {
        d.embedded.insert(OFF_FILE.into(), std::sync::Arc::new(b"off".to_vec()));
    }
}

/// Store the routing's report among the drawing's embedded files.
pub(crate) fn store_report(d: &mut Drawing, report: &Value) {
    if let Ok(b) = serde_json::to_vec(report) {
        d.embedded.insert(REPORT_FILE.into(), std::sync::Arc::new(b));
    }
}

pub(crate) fn stored_report(d: &Drawing) -> Option<Value> {
    d.embedded.get(REPORT_FILE).and_then(|b| serde_json::from_slice(b).ok())
}

/// Route the circuits again over the model as it stands now (after the job placed its gear and
/// feeders, the levels stacked): the routed runs into the stored spec (each level's own plan),
/// the pieces cut again where the model's are (the stacking move kept on them), the report stored.
pub(crate) fn reroute(s: &mut Session) -> Result<Value> {
    const C: &str = "conduit.reroute";
    let (mut sp, stored) = super::bspec::current(s, C)?;
    if !stored {
        return Err(bad(C, "the drawing has no building spec: model.build first"));
    }
    if !enabled(s.doc()?) {
        return Ok(json!({"routed": false, "note": "routing switched off (model.build {route: true} turns it on)"}));
    }
    let t0 = web_time::Instant::now();
    let routed = route(s.doc()?, &sp);
    apply(&mut sp, &routed);
    let d = s.doc_mut()?;
    // Each level's stacking move (a level's built objects all moved by the same).
    let mut offsets: BTreeMap<String, Vec2> = BTreeMap::new();
    for e in d.model.iter().filter(|e| super::wallgen::made_as(e).is_some()) {
        if let Some(l) = super::bspec::made_level(e) {
            offsets.entry(l.to_ascii_lowercase()).or_insert_with(|| super::bspec::stacked_by(e));
        }
    }
    let off_of = |level: &str| offsets.get(&level.to_ascii_lowercase()).copied().unwrap_or(Vec2::new(0.0, 0.0));
    // The pieces there are now, and their systems: only those are cut again (runs the job built
    // as whole runs stay whole).
    let mut systems: BTreeSet<String> = BTreeSet::from([BRANCH_SYSTEM.to_string()]);
    let old: Vec<Handle> = d
        .model
        .iter()
        .filter(|e| super::mepatoms::is_atom(e))
        .inspect(|e| {
            if let Some(sy) = super::bimlook::made_tag(e, "system") {
                systems.insert(sy.to_string());
            }
        })
        .map(|e| e.handle)
        .collect();
    let mut shifted = sp.clone();
    shifted.mep.runs.retain(|r| systems.contains(&r.system));
    // A feeder's entry stubs are cut again only where they were pieces already.
    shifted.mep.feeders.retain(|f| systems.contains(&f.tag));
    for r in &mut shifted.mep.runs {
        let o = off_of(&r.level);
        r.points.iter_mut().for_each(|p| *p = [p[0] + o.x, p[1] + o.y]);
    }
    for e in &mut shifted.mep.equipment {
        let o = off_of(&e.level);
        e.min = [e.min[0] + o.x, e.min[1] + o.y];
        e.max = [e.max[0] + o.x, e.max[1] + o.y];
    }
    for h in &old {
        d.remove_entity(*h);
    }
    let before: BTreeSet<Handle> = d.model.handles().into_iter().collect();
    let atoms = super::mepatoms::build::build(d, &shifted);
    let made: Vec<Handle> = d.model.handles().into_iter().filter(|h| !before.contains(h)).collect();
    for h in made {
        let level = d.model.get(h).and_then(|e| super::bspec::made_level(e)).unwrap_or_default();
        let o = off_of(&level);
        if o.len() > 1e-9 {
            let _ = d.modify_entity(h, |e| e.common.xdata = super::bspec::with_stacked(&e.common.xdata, o).into());
        }
    }
    let mut report = routed.report.clone();
    check_box_fill(d, &mut report);
    store_report(d, &report);
    d.building_spec = Some(sp.to_json());
    s.touch();
    Ok(json!({
        "routed": true, "removed": old.len(), "atoms": atoms["atoms"], "seconds": (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        "circuits": routed.report["routed_circuits"], "homeruns": routed.report["homeruns"], "routed_ft": routed.report["routed_ft"],
        "clashes": routed.report["clashes"].as_array().map_or(0, Vec::len), "unrouted": routed.report["unrouted"].as_array().map_or(0, Vec::len),
    }))
}

fn run_reroute(s: &mut Session, _p: &Value) -> Result<Value> {
    reroute(s)
}

// ─── Commands ────────────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("conduit.route", "Conduit Routing Report", run_report)
            .menu(&["Draw", "BIM", "Conduit Routing Report"])
            .params("{section?: \"clashes\"|\"conflicts\"|\"penetrations\"|\"box_fill\"|\"fill\"|\"derated\"|\"conductors_upsized\"|\"unrouted\"|\"methods\", max?: n (items listed, default 200)} → the last model.build's routing: {circuits, routed_circuits, runs, homeruns, routed_ft, estimated_ft_before, ft_by_method_size, methods, wire_size_table, clashes, clashes_by_pair_level, conflicts, entry_unknown, penetrations, box_fill, conductors_upsized, fill, derated, unrouted}")
            .noundo(),
        CommandSpec::new("conduit.reroute", "Reroute Conduit", run_reroute)
            .menu(&["Draw", "BIM", "Reroute Conduit"])
            .params("{} → {routed, removed, atoms, seconds, circuits, homeruns, routed_ft, clashes, unrouted}: the circuits routed again over the model as it stands (gear and feeders placed since the build), the pieces cut again, the report and the spec's routed runs stored"),
        CommandSpec::new("conduit.takeoff", "Conduit Takeoff", run_takeoff)
            .menu(&["Draw", "BIM", "Conduit Takeoff"])
            .params("{group_by?: [\"level\",\"area\",\"circuit\"] (any of them; default level), file?: path (.csv; the table, each line with its elements' handles), cost_db?: path (.csv: item, size?, unit, material_unit, labor_unit — labor_unit in DOLLARS per unit), labor_rate?: $/hour (turns labor dollars into hours; default the estimate's rate), price?: bool (default true)} → {lines: [{level?, area?, circuit?, kind, item, size, type, qty, unit, material, labor_dollars, labor_hours, labor_basis, via, handles, elements}], totals: {material, labor_dollars, labor_hours, cost}, firestop, unpriced, file}")
            .noundo(),
    ]
}

fn run_report(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let Some(mut r) = stored_report(d) else {
        return Ok(json!({"routed": false, "note": "no routing stored: model.build routes the circuits when the spec has them"}));
    };
    let max = p.get("max").and_then(Value::as_u64).map_or(200, |v| v.min(20_000) as usize);
    let lists = ["clashes", "conflicts", "penetrations", "conductors_upsized", "fill", "derated", "unrouted"];
    if let Some(sec) = p.get("section").and_then(Value::as_str) {
        if sec == "box_fill" {
            return Ok(r["box_fill"].clone());
        }
        if !lists.contains(&sec) && sec != "methods" {
            return Err(bad("conduit.route", format!("`section` must be one of {lists:?}, box_fill or methods")));
        }
        let v = r.get(sec).cloned().unwrap_or(Value::Null);
        return Ok(match v {
            Value::Array(a) => json!({"total": a.len(), sec: a.into_iter().take(max).collect::<Vec<_>>()}),
            v => v,
        });
    }
    for k in lists {
        if let Some(a) = r.get_mut(k).and_then(Value::as_array_mut) {
            let n = a.len();
            a.truncate(max);
            r[format!("{k}_total")] = json!(n);
        }
    }
    if let Some(a) = r["box_fill"]["over"].as_array_mut() {
        a.truncate(max);
    }
    r["routed"] = json!(true);
    Ok(r)
}

/// One cost DB row: material and labor per unit, labor in dollars.
struct CostRow {
    item: String,
    size: String,
    unit: String,
    material: f64,
    labor_dollars: f64,
}

fn read_cost_db(path: &str) -> Result<Vec<CostRow>> {
    let text = std::fs::read_to_string(path).map_err(|e| bad("conduit.takeoff", format!("`cost_db` {path}: {e}")))?;
    let mut lines = text.lines().take(200_000);
    let head: Vec<String> = lines.next().unwrap_or_default().split(',').map(|h| h.trim().trim_matches('"').to_ascii_lowercase()).collect();
    let col = |names: &[&str]| head.iter().position(|h| names.contains(&h.as_str()));
    let (Some(ci), Some(cm), Some(cl)) = (col(&["item", "description", "name"]), col(&["material_unit", "material"]), col(&["labor_unit", "labor"]))
    else {
        return Err(bad("conduit.takeoff", "`cost_db` needs columns item, material_unit and labor_unit (labor in dollars per unit)"));
    };
    let (cs, cu) = (col(&["size"]), col(&["unit", "uom"]));
    let num =
        |s: &str| s.trim().trim_matches('"').trim_start_matches('$').replace(',', "").parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0);
    let mut out = Vec::new();
    for l in lines {
        let f: Vec<&str> = l.split(',').collect();
        let get = |i: Option<usize>| i.and_then(|i| f.get(i)).map(|v| v.trim().trim_matches('"').to_string()).unwrap_or_default();
        let item = get(Some(ci));
        if item.is_empty() {
            continue;
        }
        out.push(CostRow {
            item,
            size: get(cs),
            unit: get(cu),
            material: num(&get(Some(cm))).unwrap_or(0.0),
            labor_dollars: num(&get(Some(cl))).unwrap_or(0.0),
        });
    }
    Ok(out)
}

fn norm(s: &str) -> String {
    s.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '/' || *c == '-').collect()
}

fn csv_cell(v: &str) -> String {
    let t = v.replace('"', "\"\"");
    let risky = t.starts_with(['=', '+', '-', '@']);
    if t.contains([',', '"', '\n']) || risky { format!("\"{}{t}\"", if risky { "'" } else { "" }) } else { t }
}

fn run_takeoff(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "conduit.takeoff";
    let groups: Vec<String> = match p.get("group_by") {
        None | Some(Value::Null) => vec!["level".into()],
        Some(Value::String(g)) => vec![g.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(_) => return Err(bad(C, "`group_by` is a list of level, area, circuit")),
    };
    if groups.iter().any(|g| !matches!(g.as_str(), "level" | "area" | "circuit")) {
        return Err(bad(C, "`group_by` takes level, area and circuit"));
    }
    let price = bool_or(p, "price", true);
    let rate = p.get("labor_rate").and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0);
    let cost_db = match str_param(p, "cost_db") {
        Some(path) => Some(read_cost_db(&path)?),
        None => None,
    };
    // Every piece of the model, grouped.
    type Key = (String, String, String, String, String, String, String);
    let mut rows: BTreeMap<Key, (f64, Vec<Handle>)> = BTreeMap::new();
    {
        let d = s.doc()?;
        for e in d.model.iter().filter(|e| super::mepatoms::is_atom(e)) {
            let ps = props::read(&e.common.xdata);
            let g = |k: &str| ps.get(k).map(Prop::text).unwrap_or_default();
            let kind = g(super::mepatoms::ATOM_KEY);
            // Field bends are labor, not material: listed (a count where the route turns) all the same.
            if kind.is_empty() {
                continue;
            }
            let length = g("length_ft").parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0);
            let by_ft = matches!(kind.as_str(), "conduit" | "cable" | "wire");
            let qty = if by_ft { length.unwrap_or(0.0) } else { 1.0 };
            let level = if groups.iter().any(|x| x == "level") { super::bspec::made_level(e).unwrap_or_default() } else { String::new() };
            let area = if groups.iter().any(|x| x == "area") { g("room") } else { String::new() };
            let circuit = if groups.iter().any(|x| x == "circuit") { g("circuit") } else { String::new() };
            let ty = if kind == "wire" { format!("{} {}", g("type"), g("color")).trim().to_string() } else { g("type") };
            let key = (level, area, circuit, kind, g("item"), g("size"), ty);
            let r = rows.entry(key).or_insert((0.0, Vec::new()));
            r.0 += qty;
            if r.1.len() < 100_000 {
                r.1.push(e.handle);
            }
        }
    }
    let report = stored_report(s.doc()?);
    let firestop: Vec<Value> = report.as_ref().and_then(|r| r["penetrations"].as_array().cloned()).unwrap_or_default();
    // Firestop per penetration: by level, rating and raceway size.
    let mut fs: BTreeMap<(String, String, String), (f64, Vec<String>)> = BTreeMap::new();
    for f in &firestop {
        let k = (
            if groups.iter().any(|x| x == "level") { f["level"].as_str().unwrap_or_default().to_string() } else { String::new() },
            f["rating"].as_str().unwrap_or_default().to_string(),
            super::mepatoms::trade_size(f["raceway_in"].as_f64().unwrap_or(0.75)),
        );
        let e = fs.entry(k).or_insert((0.0, Vec::new()));
        e.0 += 1.0;
        if let Some(w) = f["wall"].as_str() {
            e.1.push(w.to_string());
        }
    }
    let item_fs = rules().firestop.item.clone();
    // Lines, priced.
    let mut lines: Vec<Value> = Vec::new();
    let mut unpriced: Vec<String> = Vec::new();
    let (mut tm, mut tl, mut th) = (0.0, 0.0, 0.0);
    let mut est_lines: Vec<Value> = Vec::new();
    let mut est_at: Vec<usize> = Vec::new();
    let add = |lines: &mut Vec<Value>, k: [&str; 7], qty: f64, unit: &str, handles: Vec<String>| {
        let n = handles.len();
        lines.push(json!({
            "level": k[0], "area": k[1], "circuit": k[2], "kind": k[3], "item": k[4], "size": k[5], "type": k[6],
            "qty": r2(qty), "unit": unit, "elements": n, "handles": handles.into_iter().take(400).collect::<Vec<_>>(),
        }));
    };
    for ((level, area, circuit, kind, item, size, ty), (qty, hs)) in &rows {
        let unit = if matches!(kind.as_str(), "conduit" | "cable" | "wire") { "ft" } else { "ea" };
        add(&mut lines, [level, area, circuit, kind, item, size, ty], *qty, unit, hs.iter().map(|h| h.hex()).collect());
    }
    for ((level, rating, size), (n, walls)) in &fs {
        add(&mut lines, [level, "", "", "firestop", &item_fs, size, rating], *n, "ea", walls.clone());
    }
    if price {
        match &cost_db {
            Some(db) => {
                let rate =
                    rate.or_else(|| s.execute("estimate.settings", &json!({})).ok().and_then(|v| v["labor_rate"].as_f64()).filter(|v| *v > 0.0));
                for l in &mut lines {
                    let (item, size) = (l["item"].as_str().unwrap_or_default().to_string(), l["size"].as_str().unwrap_or_default().to_string());
                    let qty = l["qty"].as_f64().unwrap_or(0.0);
                    let hit = db.iter().find(|r| norm(&r.item) == norm(&item) && (r.size.is_empty() || norm(&r.size) == norm(&size)));
                    match hit {
                        Some(r) => {
                            let (m, ld) = (r.material * qty, r.labor_dollars * qty);
                            let lh = rate.map(|rt| ld / rt);
                            l["material"] = json!(r2(m));
                            l["labor_dollars"] = json!(r2(ld));
                            l["labor_hours"] = lh.map_or(Value::Null, |h| json!(r3(h)));
                            l["labor_basis"] = json!(match rate {
                                Some(rt) => format!(
                                    "cost DB labor_unit ${}/{} (dollars) ÷ ${rt}/h",
                                    r.labor_dollars,
                                    if r.unit.is_empty() { "unit" } else { &r.unit }
                                ),
                                None => format!(
                                    "cost DB labor_unit ${}/{} (dollars); no labor rate given: hours not computed",
                                    r.labor_dollars,
                                    if r.unit.is_empty() { "unit" } else { &r.unit }
                                ),
                            });
                            l["via"] = json!("cost DB");
                            tm += m;
                            tl += ld;
                            th += lh.unwrap_or(0.0);
                        }
                        None => unpriced.push(format!("{item} {size}").trim().to_string()),
                    }
                }
            }
            None => {
                for (i, l) in lines.iter().enumerate() {
                    if l["qty"].as_f64().is_some_and(|q| q > 0.0) && !l["item"].as_str().unwrap_or_default().is_empty() {
                        est_at.push(i);
                        est_lines.push(
                            json!({"item": l["item"], "size": l["size"], "unit": if l["unit"] == "ft" { "ft" } else { "each" }, "qty": l["qty"]}),
                        );
                    }
                }
            }
        }
    }
    if !est_lines.is_empty() {
        // The estimate's prices (the drawing's own, the price book, the starter book): hours
        // per unit there, dollars by the estimate's rate. One call, its lines in order.
        let mut got: Vec<Value> = Vec::new();
        for chunk in est_lines.chunks(200) {
            let priced = s.execute("estimate.change", &json!({"lines": chunk})).ok();
            let mut part: Vec<Value> = priced.as_ref().and_then(|v| v["lines"].as_array().cloned()).unwrap_or_default();
            part.resize(chunk.len(), Value::Null);
            got.extend(part);
        }
        for (k, i) in est_at.iter().enumerate() {
            let Some(l) = lines.get_mut(*i) else { continue };
            let x = got.get(k).cloned().unwrap_or(Value::Null);
            if x["priced"].as_bool() == Some(true) {
                let (m, lh, ld) =
                    (x["material"].as_f64().unwrap_or(0.0), x["labor_hours"].as_f64().unwrap_or(0.0), x["labor_cost"].as_f64().unwrap_or(0.0));
                l["material"] = json!(r2(m));
                l["labor_hours"] = json!(r3(lh));
                l["labor_dollars"] = json!(r2(ld));
                l["labor_basis"] = json!("estimate: labor hours per unit × the estimate's labor rate");
                l["via"] = x["via"].clone();
                tm += m;
                tl += ld;
                th += lh;
            } else {
                unpriced.push(format!("{} {}", l["item"].as_str().unwrap_or_default(), l["size"].as_str().unwrap_or_default()).trim().to_string());
            }
        }
    }
    unpriced.sort();
    unpriced.dedup();
    let mut file_out = Value::Null;
    if let Some(path) = str_param(p, "file") {
        let mut csv =
            String::from("level,area,circuit,kind,item,size,type,qty,unit,material,labor_dollars,labor_hours,labor_basis,via,elements,handles\n");
        for l in &lines {
            let g = |k: &str| match &l[k] {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                v => v.to_string(),
            };
            let hs: Vec<String> = l["handles"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            let row: Vec<String> = [
                "level",
                "area",
                "circuit",
                "kind",
                "item",
                "size",
                "type",
                "qty",
                "unit",
                "material",
                "labor_dollars",
                "labor_hours",
                "labor_basis",
                "via",
                "elements",
            ]
            .iter()
            .map(|k| csv_cell(&g(k)))
            .chain(std::iter::once(csv_cell(&hs.join(" "))))
            .collect();
            csv.push_str(&row.join(","));
            csv.push('\n');
        }
        std::fs::write(&path, csv).map_err(|e| bad(C, format!("could not write {path}: {e}")))?;
        file_out = json!(path);
    }
    let shown: Vec<Value> = lines
        .iter()
        .map(|l| {
            let mut l = l.clone();
            if let Some(a) = l["handles"].as_array_mut() {
                a.truncate(20);
            }
            l
        })
        .take(5_000)
        .collect();
    Ok(json!({
        "lines": shown, "line_count": lines.len(),
        "totals": {"material": r2(tm), "labor_dollars": r2(tl), "labor_hours": r2(th), "cost": r2(tm + tl)},
        "firestop": firestop.len(), "unpriced": unpriced, "file": file_out,
        "group_by": groups,
    }))
}

#[cfg(test)]
#[path = "conduitroute_tests.rs"]
mod tests;
