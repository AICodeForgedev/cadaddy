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
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// The system routed branch circuits go into the spec as.
pub(crate) const BRANCH_SYSTEM: &str = "E-BRANCH";
/// What a routed run says of its height.
pub(crate) const ROUTED: &str = "routed";
/// The routing's report among the drawing's embedded files.
pub(crate) const REPORT_FILE: &str = "route/report.json";
/// Each routed leg's path as a cable runs it where that is not the raceway's (through the studs
/// of the room's walls): what a cable alternative is measured on.
pub(crate) const CABLE_FILE: &str = "route/cable.json";
/// The specification's wiring methods as the job read them (the router's input).
pub(crate) const METHODS_FILE: &str = "route/methods.json";
const RULES: &str = include_str!("conduitroute.json");
/// What marks a homerun's own tag (the circuits it carries): `circuit:<level>:home:<panel>:<n>`.
const HOME_PART: &str = ":home:";
/// What an in-wall joint's run says it is (it follows its wall, not the plan's axes).
const IN_WALL: &str = "in the wall";
/// The obstacle category of a gear's working space (NEC 110.26(A)).
const WORKING_SPACE: &str = "working space";

/// Most circuits, devices, obstacles, runs and items one routing handles (hostile models).
const MAX_CIRCUITS: usize = 20_000;
const MAX_DEVICES: usize = 100_000;
const MAX_OBSTACLES: usize = 400_000;
const MAX_RUNS: usize = 100_000;
const MAX_ITEMS: usize = 20_000;
/// Most bored holes listed.
const MAX_BORED: usize = 400_000;
const MAX_TEXTS: usize = 400_000;
/// A device this close (feet) to a point of its circuit's earlier route belongs to it.
const NEAR_RUN_FT: f64 = 3.0;
/// A typical room's own panel this close (feet) at most.
const ROOM_PANEL_FT: f64 = 60.0;
/// Where across a corridor its branch homerun lane runs (a fraction of its width).
const LANE_AT: f64 = 0.25;
/// Bucket size of the obstacle grid, feet.
const CELL_FT: f64 = 8.0;
/// Most devices one switch or sensor names as what it controls.
const MAX_CONTROLS: usize = 256;

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
    pub framing: FramingRules,
    pub entry: Entry,
    pub gear: GearRules,
    pub cable: CableRoute,
    pub switch_legs: SwitchLegs,
}

/// How a cable (NM, MC, AC) runs between the boxes of a room's walls (Q6200 branchruns).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct CableRoute {
    pub above_box_in: f64,
    pub in_wall_max_ft: f64,
    pub source: String,
}

/// A switch or sensor on the circuit of what it controls.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct SwitchLegs {
    pub source: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct GearRules {
    pub kinds: Vec<String>,
    pub working_depth_ft: f64,
    pub working_height_ft: f64,
    pub dedicated_above_ft: f64,
    pub window_margin_in: f64,
    pub source: String,
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
    pub drop_reach_ft: f64,
    pub drop_step_in: f64,
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
    pub floor_unrated: String,
    pub source: String,
}

/// The framing's rules: where a raceway may bore an I-joist's web, and a truss's chords.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct FramingRules {
    pub flange_in: f64,
    pub edge_in: f64,
    pub hole_over_in: f64,
    /// `[hole diameter in, least distance from the joist's end ft]`, smallest hole first; a hole
    /// larger than the last row is not bored.
    pub min_end_ft: Vec<[f64; 2]>,
    pub apart_x: f64,
    pub hole_penalty_ft: f64,
    pub truss_chord_in: f64,
    pub bored_item: String,
    pub truss_item: String,
    pub source: String,
}

impl FramingRules {
    /// The least distance from a joist's end a hole of `dia_in` is bored at (ft); none: too big.
    pub(crate) fn min_end(&self, dia_in: f64) -> Option<f64> {
        self.min_end_ft.iter().find(|r| dia_in <= r[0] + 1e-9).map(|r| r[1].max(0.0))
    }
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
        if m.min_size_in.is_none()
            && up.contains("MINIMUM")
            && (up.contains("RACEWAY") || up.contains("CONDUIT"))
            && up.contains("SIZE")
            && let Some(v) = trade_size_in(&up)
        {
            m.min_size_in = Some(v);
            m.min_size_source = format!("spec {section}: {}", line.trim().chars().take(160).collect::<String>());
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
            if let (true, [_, hi, ..]) = (joined, nums.as_slice())
                && *hi > 0.0
                && !bands.iter().any(|(h, _)| (h - hi).abs() < 0.5)
                && bands.len() < 16
            {
                bands.push((*hi, size));
                found = true;
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

/// The category the framing's members (joists, trusses) are among a level's obstacles as.
pub(crate) const FRAMING: &str = "framing";

/// A framing member over a level: its centreline, width and whether it is a truss (never bored).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub a: Vec2,
    pub b: Vec2,
    pub t: f64,
    pub truss: bool,
}

/// A raceway's way through a framing member: a hole bored in a joist's web, or a pass between a
/// truss's chords.
#[derive(Clone, Debug)]
pub(crate) struct Hole {
    /// The member (its index among the field's obstacles).
    pub obstacle: usize,
    pub at: Vec2,
    pub z: f64,
    /// The hole's diameter (0: through a truss's open web).
    pub dia_in: f64,
    /// Along the member from its start (sheet units), and to its nearer end (ft).
    pub along: f64,
    pub end_ft: f64,
}

/// Why the framing stops a piece.
pub(crate) const IN_JOIST: &str = "runs inside the member";
pub(crate) const FLANGE: &str = "the hole would cut a flange (outside the web's hole zone)";
pub(crate) const NEAR_END: &str = "the hole is too near the member's bearing";
pub(crate) const TOO_BIG: &str = "the hole is bigger than the web takes";
pub(crate) const CHORD: &str = "it would bore a truss chord";

/// The distance from `p` to the segment `a`–`b`.
fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(Vec2::new(a.x + t * dx, a.y + t * dy))
}

/// The obstacles of one level, bucketed in plan.
#[derive(Default)]
pub(crate) struct Field {
    pub obs: Vec<Obs>,
    cell: f64,
    grid: HashMap<(i64, i64), Vec<usize>>,
    /// The framing members among `obs` (by index).
    frames: HashMap<usize, Frame>,
}

impl Field {
    /// The obstacles and the framing over the level (each member an obstacle of category
    /// [`FRAMING`]: a horizontal piece bores it or keeps to its bays, a vertical one stands clear).
    pub(crate) fn with_framing(mut obs: Vec<Obs>, framing: Vec<(Obs, Frame)>, upf: f64) -> Self {
        let mut frames = HashMap::new();
        for (o, f) in framing.into_iter().take(MAX_OBSTACLES) {
            if !(f.a.is_finite() && f.b.is_finite() && f.t.is_finite() && f.t > 0.0) {
                continue;
            }
            frames.insert(obs.len(), f);
            obs.push(o);
        }
        let mut field = Field::new(obs, upf);
        field.frames = frames;
        field
    }
    /// Whether the level has framing over it.
    pub(crate) fn framed(&self) -> bool {
        !self.frames.is_empty()
    }
    /// The lowest top of the framing members over the level (the floor above's joists), where
    /// the documents give its framing.
    pub(crate) fn framing_top(&self) -> Option<f64> {
        self.frames.keys().filter_map(|i| self.obs.get(*i)).map(|o| o.hi.z).filter(|z| z.is_finite()).min_by(f64::total_cmp)
    }
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
        Field { obs, cell, grid, frames: HashMap::new() }
    }
    fn c(v: f64, cell: f64) -> i64 {
        (v / cell).floor().clamp(-1e12, 1e12) as i64
    }
    /// The first obstacle a square piece `a`–`b` at height `z` (radius `r`) comes within its
    /// clearance of, skipping `skip`.
    fn hit(&self, a: Vec2, b: Vec2, z: f64, r: f64, upf: f64, skip: &[Handle], rl: &RouteRules) -> Option<usize> {
        self.hit_span(a, b, z, z, r, upf, skip, &[], rl)
    }
    /// The first obstacle a vertical piece at `p` from `z0` to `z1` (radius `r`) comes within
    /// its clearance of, skipping `skip`: a drop, a riser, a change of tier.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn hit_column(&self, p: Vec2, z0: f64, z1: f64, r: f64, upf: f64, skip: &[Handle], rl: &RouteRules) -> Option<usize> {
        if (z1 - z0).abs() < 1e-9 {
            return None;
        }
        self.hit_span(p, p, z0.min(z1), z0.max(z1), r, upf, skip, &[], rl)
    }
    /// A piece inside a wall (a drop, its jog) or over gear (into a panel's top), `a`–`b` from
    /// `z0` to `z1`: as [`Self::hit_column`], but the gear's working space (the air before it) is
    /// not the wall's cavity, nor the electrical space over the gear (NEC 110.26(E)).
    #[allow(clippy::too_many_arguments)]
    fn hit_unworked(&self, a: Vec2, b: Vec2, z0: f64, z1: f64, r: f64, upf: f64, skip: &[Handle], rl: &RouteRules) -> Option<usize> {
        if (z1 - z0).abs() < 1e-9 && a.dist(b) < 1e-9 {
            return None;
        }
        self.hit_span(a, b, z0.min(z1), z0.max(z1), r, upf, skip, &[WORKING_SPACE], rl)
    }
    #[allow(clippy::too_many_arguments)]
    fn hit_span(&self, a: Vec2, b: Vec2, zlo: f64, zhi: f64, r: f64, upf: f64, skip: &[Handle], skip_cat: &[&str], rl: &RouteRules) -> Option<usize> {
        let z = zlo;
        let (lo, hi) = (Vec2::new(a.x.min(b.x), a.y.min(b.y)), Vec2::new(a.x.max(b.x), a.y.max(b.y)));
        let pad = 6.0 * upf;
        let (x0, y0, x1, y1) =
            (Self::c(lo.x - pad, self.cell), Self::c(lo.y - pad, self.cell), Self::c(hi.x + pad, self.cell), Self::c(hi.y + pad, self.cell));
        let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        let sane = a.is_finite()
            && b.is_finite()
            && z.is_finite()
            && zhi.is_finite()
            && (0..=4096).contains(&w)
            && (0..=4096).contains(&h)
            && w.saturating_mul(h) <= 1_000_000;
        if !sane {
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
                    if skip.contains(&o.handle) || skip_cat.contains(&o.category.as_str()) {
                        continue;
                    }
                    // A framing member: a horizontal piece bores it or keeps to its bays (see
                    // `framing`); a vertical one must stand clear of it.
                    if let Some(f) = self.frames.get(&i) {
                        let vertical = a.dist(b) <= 1e-9;
                        if vertical && zhi + r > o.lo.z && z - r < o.hi.z && seg_dist(a, f.a, f.b) < f.t / 2.0 + r {
                            let along = Vec2::new((o.lo.x + o.hi.x) / 2.0, (o.lo.y + o.hi.y) / 2.0).dist(a);
                            if best.is_none_or(|(d, _)| along < d) {
                                best = Some((along, i));
                            }
                        }
                        continue;
                    }
                    let c = rl.clearance_in(&o.category) / 12.0 * upf + r;
                    let meets =
                        lo.x <= o.hi.x + c && hi.x >= o.lo.x - c && lo.y <= o.hi.y + c && hi.y >= o.lo.y - c && zhi >= o.lo.z - c && z <= o.hi.z + c;
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

impl Field {
    /// What the framing over the level does to a square piece `a`–`b` at height `z` (its centre,
    /// radius `r`): the holes it bores through joists' webs and the trusses it passes between
    /// the chords of, nearest first; or the first member that stops it and why.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn framing(
        &self,
        a: Vec2,
        b: Vec2,
        z: f64,
        r: f64,
        upf: f64,
        rl: &RouteRules,
    ) -> std::result::Result<Vec<Hole>, (usize, &'static str)> {
        let s = b - a;
        let slen = s.len();
        if self.frames.is_empty() || slen.is_nan() || slen <= 1e-9 || !(a.is_finite() && b.is_finite() && z.is_finite() && r.is_finite()) {
            return Ok(Vec::new());
        }
        let fr = &rl.framing;
        let (lo, hi) = (Vec2::new(a.x.min(b.x), a.y.min(b.y)), Vec2::new(a.x.max(b.x), a.y.max(b.y)));
        let pad = upf;
        let (x0, y0, x1, y1) =
            (Self::c(lo.x - pad, self.cell), Self::c(lo.y - pad, self.cell), Self::c(hi.x + pad, self.cell), Self::c(hi.y + pad, self.cell));
        let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
        if !(0..=4096).contains(&w) || !(0..=4096).contains(&h) || w.saturating_mul(h) > 1_000_000 {
            return Ok(Vec::new());
        }
        let sn = s * (1.0 / slen);
        let size_in = r * 24.0 / upf;
        let mut seen: BTreeSet<usize> = BTreeSet::new();
        let mut holes: Vec<(f64, Hole)> = Vec::new();
        let mut stop: Option<(f64, usize, &'static str)> = None;
        let mut halt = |d: f64, i: usize, why: &'static str| {
            if stop.is_none_or(|(sd, _, _)| d < sd) {
                stop = Some((d, i, why));
            }
        };
        for x in x0..=x1 {
            for y in y0..=y1 {
                for &i in self.grid.get(&(x, y)).into_iter().flatten() {
                    let Some(f) = self.frames.get(&i) else { continue };
                    if !seen.insert(i) {
                        continue;
                    }
                    let Some(o) = self.obs.get(i) else { continue };
                    if z + r <= o.lo.z || z - r >= o.hi.z {
                        continue;
                    }
                    let m = f.b - f.a;
                    let mlen = m.len();
                    if mlen.is_nan() || mlen <= 1e-9 {
                        continue;
                    }
                    let u = m * (1.0 / mlen);
                    let cross = u.x * sn.y - u.y * sn.x;
                    if cross.abs() < 0.5 {
                        // Along the member: inside it unless it keeps to the bay beside it.
                        let lateral = ((a.x - f.a.x) * u.y - (a.y - f.a.y) * u.x).abs();
                        let (pa, pb) = ((a - f.a).dot(u), (b - f.a).dot(u));
                        if lateral < f.t / 2.0 + r && pa.max(pb) > 0.0 && pa.min(pb) < mlen {
                            halt((a - f.a).dot(u).clamp(0.0, mlen).min(slen), i, IN_JOIST);
                        }
                        continue;
                    }
                    // Across it: where the piece meets the member's centreline.
                    let d = f.a - a;
                    let t = (d.x * u.y - d.y * u.x) / (sn.x * u.y - sn.y * u.x);
                    let p = a + sn * t;
                    let along = (p - f.a).dot(u);
                    if !(t >= 0.0 && t <= slen && along >= 0.0 && along <= mlen) {
                        continue;
                    }
                    let end_ft = along.min(mlen - along) / upf;
                    if f.truss {
                        let chord = fr.truss_chord_in.max(0.0) / 12.0 * upf;
                        if z - r < o.lo.z + chord || z + r > o.hi.z - chord {
                            halt(t, i, CHORD);
                        } else {
                            holes.push((t, Hole { obstacle: i, at: p, z, dia_in: 0.0, along, end_ft }));
                        }
                        continue;
                    }
                    let dia_in = size_in + fr.hole_over_in.max(0.0);
                    let half = dia_in / 24.0 * upf;
                    let keep = (fr.flange_in.max(0.0) + fr.edge_in.max(0.0)) / 12.0 * upf;
                    match fr.min_end(dia_in) {
                        None => halt(t, i, TOO_BIG),
                        Some(_) if z - half < o.lo.z + keep || z + half > o.hi.z - keep => halt(t, i, FLANGE),
                        Some(need) if end_ft < need => halt(t, i, NEAR_END),
                        Some(_) => holes.push((t, Hole { obstacle: i, at: p, z, dia_in, along, end_ft })),
                    }
                }
            }
        }
        if let Some((_, i, why)) = stop {
            return Err((i, why));
        }
        holes.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(holes.into_iter().map(|(_, h)| h).collect())
    }

    /// The bay centres beside `p` across the framing's members near it (their direction's normal
    /// coordinate, as `(normal, coordinate)` with `normal` the unit vector across the members):
    /// where a piece along the members runs clear of them.
    pub(crate) fn bays_near(&self, p: Vec2, upf: f64) -> Option<(Vec2, Vec<f64>)> {
        if self.frames.is_empty() {
            return None;
        }
        let reach = 4.0 * upf;
        let (x0, y0, x1, y1) =
            (Self::c(p.x - reach, self.cell), Self::c(p.y - reach, self.cell), Self::c(p.x + reach, self.cell), Self::c(p.y + reach, self.cell));
        let mut near: Vec<&Frame> = Vec::new();
        let mut seen: BTreeSet<usize> = BTreeSet::new();
        for x in x0..=x1 {
            for y in y0..=y1 {
                for &i in self.grid.get(&(x, y)).into_iter().flatten() {
                    if let Some(f) = self.frames.get(&i)
                        && seen.insert(i)
                        && seg_dist(p, f.a, f.b) <= reach
                    {
                        near.push(f);
                    }
                }
            }
        }
        let first = near.first()?;
        let m = first.b - first.a;
        let ml = m.len();
        if ml.is_nan() || ml <= 1e-9 {
            return None;
        }
        let n = Vec2::new(-m.y / ml, m.x / ml);
        let u0 = m * (1.0 / ml);
        let parallel = |f: &&&Frame| {
            let d = f.b - f.a;
            let l = d.len();
            l > 1e-9 && (d.x * u0.y - d.y * u0.x).abs() / l < 0.1
        };
        let mut cs: Vec<f64> = near.iter().filter(parallel).map(|f| f.a.dot(n)).collect();
        cs.sort_by(f64::total_cmp);
        cs.dedup_by(|a, b| (*a - *b).abs() < upf / 24.0);
        let pc = p.dot(n);
        let mut bays: Vec<f64> = cs.windows(2).filter_map(|w| if let [a, b] = w { Some((a + b) / 2.0) } else { None }).collect();
        bays.sort_by(|a, b| (a - pc).abs().total_cmp(&(b - pc).abs()));
        bays.truncate(2);
        Some((n, bays))
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
    /// Why the framing stops it (empty: an obstacle in the way).
    pub why: &'static str,
}

/// A square path at its pieces' heights.
#[derive(Clone, Debug, Default)]
pub(crate) struct Leg {
    pub pts: Vec<Vec2>,
    pub z: Vec<f64>,
    pub blocked: Vec<Blocked>,
    /// The holes it bores through the framing (a cost: labor at each).
    pub holes: usize,
}

/// Each piece of `path` at the lowest of `tiers` it is clear at; a piece clear at none stays at
/// the first, blocked.
fn seat(path: &[Vec2], tiers: &[f64], r: f64, field: &Field, upf: f64, skip: &[Handle], rl: &RouteRules) -> Leg {
    let mut leg = Leg { pts: path.to_vec(), ..Leg::default() };
    let first = tiers.first().copied().unwrap_or(0.0);
    for w in path.windows(2) {
        let [a, b] = w else { continue };
        // Clear of the obstacles, and through the framing only where its web may be bored.
        let clear = |z: &f64| field.hit(*a, *b, *z, r, upf, skip, rl).is_none() && field.framing(*a, *b, *z, r, upf, rl).is_ok();
        match tiers.iter().copied().find(clear) {
            Some(z) => {
                leg.holes += field.framing(*a, *b, z, r, upf, rl).map_or(0, |h| h.len());
                leg.z.push(z);
            }
            None => {
                leg.z.push(first);
                if field.hit(*a, *b, first, r, upf, skip, rl).is_none()
                    && let Err((i, why)) = field.framing(*a, *b, first, r, upf, rl)
                {
                    let at = field.obs.get(i).map_or((*a + *b) * 0.5, |o| {
                        let c = Vec2::new((o.lo.x + o.hi.x) / 2.0, (o.lo.y + o.hi.y) / 2.0);
                        let (dx, dy) = (b.x - a.x, b.y - a.y);
                        let l2 = dx * dx + dy * dy;
                        let t = if l2 > 0.0 { (((c.x - a.x) * dx + (c.y - a.y) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                        Vec2::new(a.x + t * dx, a.y + t * dy)
                    });
                    leg.blocked.push(Blocked { obstacle: i, at: Vec3::new(at.x, at.y, first), z: first, why });
                } else if let Some(i) = field.hit(*a, *b, first, r, upf, skip, rl) {
                    let at = field.obs.get(i).map_or((*a + *b) * 0.5, |o| {
                        // The point on the piece nearest the obstacle's centre.
                        let c = Vec2::new((o.lo.x + o.hi.x) / 2.0, (o.lo.y + o.hi.y) / 2.0);
                        let (dx, dy) = (b.x - a.x, b.y - a.y);
                        let l2 = dx * dx + dy * dy;
                        let t = if l2 > 0.0 { (((c.x - a.x) * dx + (c.y - a.y) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                        Vec2::new(a.x + t * dx, a.y + t * dy)
                    });
                    leg.blocked.push(Blocked { obstacle: i, at: Vec3::new(at.x, at.y, first), z: first, why: "" });
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
    let bore = rl.framing.hole_penalty_ft.max(0.0) * upf;
    let score = |l: &Leg| {
        let tiers_changed = l.z.windows(2).filter(|w| matches!(w, [p, q] if (p - q).abs() > 1e-9)).count() as f64;
        (l.blocked.len(), poly_len(&l.pts) + bend * (l.pts.len().saturating_sub(2) as f64 + tiers_changed) + bore * l.holes as f64)
    };
    let mut best: Option<((usize, f64), Leg)> = None;
    let mut cands = shapes(a, b, detour, eps);
    // Under framing: the run along the members kept to the bays beside its ends.
    for p in [a, b] {
        if let Some((n, bays)) = field.bays_near(p, upf) {
            for c in bays {
                let (ca, cb) = (c - a.dot(n), c - b.dot(n));
                cands.push(tidy(vec![a, a + n * ca, b + n * cb, b], eps));
            }
        }
    }
    for p in cands {
        if p.len() < 2 {
            let one = Leg { pts: vec![a, b], z: vec![tiers.first().copied().unwrap_or(0.0)], blocked: Vec::new(), holes: 0 };
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
    /// What it controls (a switch's or sensor's luminaires), by handle.
    controls: Vec<Handle>,
    /// A switch (its box takes the switch loop) and its travelers: 0 single-pole, 1 3-way, 2 4-way.
    switch: bool,
    ways: u8,
}

/// The devices a switch or sensor controls: its `controls` (their handles).
pub(crate) fn controls_of(ps: &props::Props) -> Vec<Handle> {
    ps.get("controls")
        .map(Prop::text)
        .unwrap_or_default()
        .split([',', ' ', ';'])
        .filter_map(|h| u64::from_str_radix(h.trim(), 16).ok())
        .filter(|h| *h != 0)
        .take(MAX_CONTROLS)
        .map(Handle)
        .collect()
}

/// The circuit of a device the prints give none, from what it controls (a switch's leg): the
/// circuit most of them are on, the first named breaking a tie.
pub(crate) fn controlled_circuit(controls: &[Handle], circuit_of: &HashMap<Handle, String>) -> Option<String> {
    let mut votes: Vec<(String, usize)> = Vec::new();
    for c in controls.iter().filter_map(|h| circuit_of.get(h)) {
        match votes.iter_mut().find(|v| v.0.eq_ignore_ascii_case(c)) {
            Some(v) => v.1 += 1,
            None => votes.push((c.trim().to_string(), 1)),
        }
    }
    let most = votes.iter().map(|v| v.1).max()?;
    votes.into_iter().find(|v| v.1 == most).map(|v| v.0)
}

/// A cable's leg between two boxes in the walls of one room (Q6200): up from the lower box to
/// the rules' height over the higher one, through the studs along its wall (round the corner
/// onto the other's wall when they are on two walls), and down into the other box; plan points
/// and each segment's height above `floor` (feet), as the routed runs carry them. None when the
/// boxes are not both in a wall of the same room, face away from each other (opposite walls:
/// the cable goes over the ceiling), or are farther apart along the walls than the rules allow.
pub(crate) fn stud_path(
    a: (Vec2, f64, Option<Vec2>, &str),
    b: (Vec2, f64, Option<Vec2>, &str),
    floor: f64,
    upf: f64,
    rl: &RouteRules,
) -> Option<(Vec<[f64; 2]>, Vec<f64>)> {
    let ((pa, ta, fa, ra), (pb, tb, fb, rb)) = (a, b);
    let (fa, fb) = (fa?, fb?);
    if ra.trim().is_empty() || !ra.trim().eq_ignore_ascii_case(rb.trim()) || fa.dot(fb) < -0.5 {
        return None;
    }
    let tol = upf / 96.0;
    // Along its wall, then onto the other's: the corner where they meet (none on one wall).
    let along = Vec2::new(-fa.y, fa.x);
    let corner = if fa.dot(fb) > 0.9 {
        let off = (pb - pa).dot(fa);
        // Parallel faces a little apart (a jog in the wall): across at the jog.
        (off.abs() > tol).then(|| pa + along * (pb - pa).dot(along))
    } else {
        // Two walls: along `a`'s to the line of `b`'s.
        Some(pa + along * (pb - pa).dot(along))
    };
    let mut plan = vec![pa];
    plan.extend(corner.filter(|c| c.dist(pa) > tol && c.dist(pb) > tol));
    plan.push(pb);
    let flat = poly_len(&plan);
    if !(flat.is_finite() && flat <= rl.cable.in_wall_max_ft.max(0.0) * upf) {
        return None;
    }
    let z = ta.max(tb) + rl.cable.above_box_in.max(0.0) / 12.0 * upf;
    let mut pts = vec![[pa.x, pa.y]];
    let mut bot = vec![(ta - floor) / upf];
    pts.extend(plan.iter().map(|p| [p.x, p.y]));
    bot.extend(std::iter::repeat_n((z - floor) / upf, plan.len().saturating_sub(1)));
    pts.push([pb.x, pb.y]);
    bot.push((tb - floor) / upf);
    Some((pts, bot))
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

/// A rated wall a route may cross: its outline on its level's own plan, its top (drawing z),
/// its type and that type's rating.
#[derive(Clone, Debug)]
pub(crate) struct Rated {
    pub handle: Handle,
    pub level: String,
    pub hull: Vec<Vec2>,
    pub top: f64,
    pub wall_type: String,
    pub rating: String,
}

/// The built walls whose type has a fire rating in the documents (the wall types' assemblies),
/// at most `cap`.
pub(crate) fn rated_walls(d: &Drawing, sp: &BuildingSpec, cap: usize) -> Vec<Rated> {
    let rating_of = |name: &str| {
        sp.wall_types
            .iter()
            .find(|w| w.name.eq_ignore_ascii_case(name))
            .and_then(|w| w.assembly.as_ref())
            .and_then(|a| a.fire_rating.clone())
            .filter(|r| !r.trim().is_empty() && !r.trim().eq_ignore_ascii_case("none") && !r.trim().starts_with('0'))
    };
    let mut rated = Vec::new();
    for e in d.model.iter() {
        if rated.len() >= cap {
            break;
        }
        if super::wallgen::made_as(e).as_deref() != Some("wall") {
            continue;
        }
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
    rated
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
pub(crate) fn crosses(a: Vec2, b: Vec2, poly: &[Vec2]) -> bool {
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
    /// The circuit tags (ids) that are feeders to gear, not branch circuits: the feeder's own
    /// routing carries them, so their estimated branch routes go too.
    pub feeders: BTreeSet<String>,
    /// Each leg's path as a cable runs it where that is not the raceway's ([`CABLE_FILE`]).
    pub cable: Vec<MepRun>,
}

/// The feeder a circuit's breaker serves, when it is one: its panel's schedule row for the
/// circuit names the gear a feeder from that panel lands on (`PANELBOARD U (6 UNITS)` with a
/// feeder from the panel to `U`), or the row's wire is that feeder (`feeder 7: …`). Such a
/// circuit is the feeder (its schedule's conductors and raceway, routed with the feeders), not a
/// branch homerun to devices.
pub(crate) fn feeder_of<'a>(sp: &'a BuildingSpec, panel: &str, label: &str) -> Option<&'a cadaddy_walk3d::Feeder> {
    let panel = panel.trim();
    if panel.is_empty() {
        return None;
    }
    let num = super::printcond::parse_circuit(label)?.1;
    let first = num.split(',').next()?.trim().to_string();
    let sched = sp.mep.panel_schedules.iter().find(|p| p.panel.trim().eq_ignore_ascii_case(panel))?;
    let row = sched.circuits.iter().find(|c| c.number.trim() == first)?;
    let words: Vec<String> = row
        .description
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '/'))
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_uppercase)
        .take(64)
        .collect();
    let wire = row.wire.trim().to_ascii_uppercase();
    sp.mep.feeders.iter().filter(|f| f.from.trim().eq_ignore_ascii_case(panel) && !f.to.trim().is_empty()).find(|f| {
        let to = f.to.trim().to_ascii_uppercase();
        let tag = f.tag.trim().to_ascii_uppercase();
        words.contains(&to)
            || (!tag.is_empty() && wire.strip_prefix("FEEDER ").is_some_and(|w| w.split([':', ' ', ',']).next() == Some(tag.as_str())))
    })
}

/// One circuit's routed lengths, feet: its legs in its area, its share of the homeruns (to the
/// panel), and its conductors with their makeup.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Lens {
    pub in_area: f64,
    pub home: f64,
    pub wire: f64,
    /// Its own homerun from its first box to the panel (a member's tap and the shared homerun
    /// all the way), its own conductors' feet along all of it with their makeup, and its own
    /// cable's feet (Q6206: in a cable method every circuit is its own cable, never shared).
    pub own_home: f64,
    pub own_wire: f64,
    pub cable: f64,
}

/// A circuit's cable size as a cable alternative names it (`12/2 w/G`), from its conductors.
pub(crate) fn cable_size(makeup: &str) -> String {
    let (ty, _, _) = super::mepatoms::cable_type("", makeup);
    ty.trim().to_string()
}

/// The routed lengths by room and level (Q6200): each typical room by its unit type (the
/// spec's unit instances: their average and each room), each common area, each level, all of
/// them. `rows`: each routed circuit (or part) with its level and room. Raceway (or cable) feet
/// in the room and to the panel, and the conductors' feet with their makeup.
pub(crate) fn lengths_report(sp: &BuildingSpec, rows: &[(String, String, Lens, BTreeMap<String, f64>)]) -> Value {
    #[derive(Default, Clone)]
    struct Sum {
        n: usize,
        l: Lens,
        cable: BTreeMap<String, f64>,
    }
    let add = |s: &mut Sum, l: &Lens, c: &BTreeMap<String, f64>| {
        s.n += 1;
        s.l.in_area += l.in_area;
        s.l.home += l.home;
        s.l.wire += l.wire;
        s.l.own_home += l.own_home;
        s.l.own_wire += l.own_wire;
        s.l.cable += l.cable;
        for (k, v) in c {
            *s.cable.entry(k.clone()).or_default() += v;
        }
    };
    let row = |s: &Sum, per: f64| {
        let per = if per > 0.0 { per } else { 1.0 };
        json!({
            "circuits": r1(s.n as f64 / per), "home_ft": r1(s.l.home / per), "in_area_ft": r1(s.l.in_area / per),
            "total_ft": r1((s.l.home + s.l.in_area) / per), "conductor_ft": r1(s.l.wire / per),
            "own_home_ft": r1(s.l.own_home / per), "own_conductor_ft": r1(s.l.own_wire / per), "cable_ft": r1(s.l.cable / per),
            "cable_ft_by_size": s.cable.iter().map(|(k, v)| (k.clone(), json!(r1(v / per)))).collect::<serde_json::Map<_, _>>(),
        })
    };
    // Each unit instance's room (by number, on its level where it says) and its type.
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
    let instances: Vec<(String, String, String)> = sp
        .unit_instances
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_ITEMS)
        .map(|u| {
            let ty = text(u, "unit_type");
            (text(u, "number").to_ascii_uppercase(), text(u, "level").to_ascii_lowercase(), if ty.is_empty() { text(u, "type") } else { ty })
        })
        .filter(|(n, _, t)| !n.is_empty() && !t.is_empty())
        .collect();
    let names: HashMap<String, String> = sp
        .unit_types
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_ITEMS)
        .map(|t| (text(t, "id"), text(t, "name")))
        .filter(|(i, _)| !i.is_empty())
        .collect();
    let unit_of = |level: &str, room: &str| -> Option<String> {
        let (n, l) = (room.trim().to_ascii_uppercase(), level.trim().to_ascii_lowercase());
        let all: Vec<&(String, String, String)> = instances.iter().filter(|(un, _, _)| *un == n).collect();
        let only = if all.len() == 1 { all.first() } else { None };
        all.iter().find(|(_, ul, _)| *ul == l).or_else(|| all.iter().find(|(_, ul, _)| ul.is_empty())).or(only).map(|x| x.2.clone())
    };
    let mut rooms: BTreeMap<(String, String), Sum> = BTreeMap::new();
    let mut levels: BTreeMap<String, Sum> = BTreeMap::new();
    let mut total = Sum::default();
    for (level, room, l, c) in rows.iter().take(MAX_CIRCUITS) {
        add(rooms.entry((level.clone(), room.clone())).or_default(), l, c);
        add(levels.entry(level.clone()).or_default(), l, c);
        add(&mut total, l, c);
    }
    let mut types: BTreeMap<String, (usize, Sum)> = BTreeMap::new();
    let mut typical: Vec<Value> = Vec::new();
    let mut common: Vec<Value> = Vec::new();
    for ((level, room), sm) in &rooms {
        let mut v = row(sm, 1.0);
        v["level"] = json!(level);
        v["room"] = json!(if room.is_empty() { "(no room)" } else { room.as_str() });
        match (!room.is_empty()).then(|| unit_of(level, room)).flatten() {
            Some(ty) => {
                let t = types.entry(ty.clone()).or_default();
                t.0 += 1;
                let n = sm.n;
                add(&mut t.1, &sm.l, &sm.cable);
                t.1.n += n.saturating_sub(1);
                v["unit_type"] = json!(ty);
                if typical.len() < MAX_ITEMS {
                    typical.push(v);
                }
            }
            None if common.len() < MAX_ITEMS => common.push(v),
            None => {}
        }
    }
    let by_type: Vec<Value> = types
        .iter()
        .map(|(ty, (n, sm))| {
            json!({
                "unit_type": ty, "name": names.get(ty).cloned().unwrap_or_default(), "rooms": n,
                "per_room": row(sm, *n as f64), "all": row(sm, 1.0),
            })
        })
        .collect();
    let by_level: Vec<Value> = levels
        .iter()
        .map(|(l, sm)| {
            let mut v = row(sm, 1.0);
            v["level"] = json!(l);
            v
        })
        .collect();
    json!({
        "unit_types": by_type, "typical_rooms": typical, "common_areas": common, "levels": by_level, "total": row(&total, 1.0),
        "note": "home_ft: the homeruns to the panel (a shared homerun counted once, on its lead circuit; a member's tap on its own); in_area_ft: the legs device to device with their drops and rises; conductor_ft: each conductor's length with its makeup at each box and panel; own_home_ft: each circuit's own way from its first box to the panel (its tap and the shared homerun all the way); own_conductor_ft: its own conductors along its legs and its own way; cable_ft (by size): each circuit as its own cable (a cable method: never shared), its legs through the studs where it runs there, with its makeup at each box and at the panel",
    })
}

/// One circuit being routed.
struct Circ {
    tag: usize,
    label: String,
    panel: String,
    level: String,
    devs: Vec<usize>,
    /// Its panel, when the circuit was split among a typical panel's copies (one per unit).
    gear: Option<usize>,
    /// That panel's tag (its part's id suffix), empty when not split.
    part: String,
}

fn field_of<'a>(t: &'a ElementTag, k: &str) -> Option<&'a str> {
    t.fields.get(k).map(|f| f.value.trim()).filter(|v| !v.is_empty())
}

/// The circuits' routes for `sp` over the model in `d` (its devices built, their boxes not yet):
/// the runs (replacing the estimated ones), the circuits' tags as routed, and the report.
/// Where a box's raceway rises into the ceiling space, the height of its jog in the wall, and
/// what blocks it: over the box when that column is clear up to the lowest tier `z0`, else along
/// its wall (a square wall only: the face on a plan axis, either axis when the face isn't known)
/// in steps up to the rules' reach, jogging in the wall just above the box; none clear ⟹ over
/// the box, with the obstacle. `skip`: the circuit's own panel (its boxes sit beside it).
#[allow(clippy::too_many_arguments)]
pub(crate) fn rise_point(
    at: Vec2,
    top: f64,
    face: Option<Vec2>,
    z0: f64,
    r: f64,
    field: &Field,
    upf: f64,
    skip: &[Handle],
    rl: &RouteRules,
) -> (Vec2, f64, Option<usize>) {
    let zj = (top + rl.route.in_wall_above_box_in.max(0.0) / 12.0 * upf).min(z0);
    let Some(first) = field.hit_unworked(at, at, top, z0, r, upf, skip, rl) else { return (at, zj, None) };
    let dirs: Vec<Vec2> = match face {
        Some(f) if f.x.abs() > 0.99 => vec![Vec2::new(0.0, 1.0), Vec2::new(0.0, -1.0)],
        Some(f) if f.y.abs() > 0.99 => vec![Vec2::new(1.0, 0.0), Vec2::new(-1.0, 0.0)],
        Some(_) => Vec::new(),
        None => vec![Vec2::new(1.0, 0.0), Vec2::new(-1.0, 0.0), Vec2::new(0.0, 1.0), Vec2::new(0.0, -1.0)],
    };
    let step = rl.route.drop_step_in.max(1.0) / 12.0 * upf;
    let n = ((rl.route.drop_reach_ft.max(0.0) * upf) / step).floor().clamp(0.0, 48.0) as usize;
    for k in 1..=n {
        for dir in &dirs {
            let q = at + *dir * (step * k as f64);
            if field.hit_unworked(q, q, zj, z0, r, upf, skip, rl).is_none() && field.hit_unworked(at, q, zj, zj, r, upf, skip, rl).is_none() {
                return (q, zj, None);
            }
        }
    }
    (at, zj, Some(first))
}

/// A run's start at box `a` (its top `top`): the box, the jog in the wall to its rise point when
/// that is off the box, then the seated path (`leg`, from the rise point); plan points and each
/// segment's height above `floor` (feet).
fn from_box(a: Vec2, top: f64, rise: (Vec2, f64), leg: &Leg, floor: f64, upf: f64) -> (Vec<[f64; 2]>, Vec<f64>) {
    let mut pts = vec![[a.x, a.y]];
    let mut bot = vec![(top - floor) / upf];
    if rise.0.dist(a) > upf / 96.0 {
        pts.push([a.x, a.y]);
        bot.push((rise.1 - floor) / upf);
    }
    pts.extend(leg.pts.iter().map(|p| [p.x, p.y]));
    bot.extend(leg.z.iter().map(|z| (z - floor) / upf));
    (pts, bot)
}

/// A run's end at box `b`: the jog in the wall from its rise point when that is off the box, then
/// the box.
fn to_box(pts: &mut Vec<[f64; 2]>, bot: &mut Vec<f64>, b: Vec2, top: f64, rise: (Vec2, f64), floor: f64, upf: f64) {
    if rise.0.dist(b) > upf / 96.0 {
        pts.push([b.x, b.y]);
        bot.push((rise.1 - floor) / upf);
    }
    pts.push([b.x, b.y]);
    bot.push((top - floor) / upf);
}

/// The vertical pieces of a seated leg that meet an obstacle: from the lowest tier `z0` up to its
/// first piece at its start and end, and each change of tier at a corner.
#[allow(clippy::too_many_arguments)]
fn leg_columns(leg: &Leg, z0: f64, r: f64, field: &Field, upf: f64, skip: &[Handle], rl: &RouteRules, out: &mut Vec<Blocked>) {
    let mut check = |p: Option<&Vec2>, za: f64, zb: f64| {
        if let Some(p) = p
            && let Some(i) = field.hit_column(*p, za, zb, r, upf, skip, rl)
        {
            out.push(Blocked { obstacle: i, at: Vec3::new(p.x, p.y, za.max(zb)), z: za.max(zb), why: "" });
        }
    };
    check(leg.pts.first(), z0, leg.z.first().copied().unwrap_or(z0));
    for i in 1..leg.z.len() {
        let (za, zb) = (leg.z.get(i - 1).copied().unwrap_or(z0), leg.z.get(i).copied().unwrap_or(z0));
        check(leg.pts.get(i), za, zb);
    }
    check(leg.pts.last(), z0, leg.z.last().copied().unwrap_or(z0));
}

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
    let rated = rated_walls(d, sp, MAX_DEVICES);
    for e in d.model.iter() {
        if super::wallgen::made_as(e).as_deref() == Some("device") && devs.len() < MAX_DEVICES {
            let ps = props::read(&e.common.xdata);
            if ps.get("misplaced").is_some() {
                continue;
            }
            let Some((lo, hi)) = mesh_box(e) else { continue };
            let off = super::bspec::stacked_by(e);
            let (lo, hi) = (lo - Vec3::new(off.x, off.y, 0.0), hi - Vec3::new(off.x, off.y, 0.0));
            let kind = ps.get("kind").map(Prop::text).unwrap_or_default();
            let yoke = matches!(kind.replace('_', " ").as_str(), "receptacle" | "switch" | "dimmer" | "occupancy sensor");
            let switch = gap::is_switch(&kind) || ps.get("controls").is_some();
            let mates = ps.get("three_way_with").map(Prop::text).unwrap_or_default().split(',').filter(|h| !h.trim().is_empty()).count();
            let ways = gap::switch_ways(&kind, mates);
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
                switch,
                ways,
                controls: controls_of(&ps),
            });
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
            // The trades' pieces (studs, sheets, pours, batts) are their hosts' layout, not more
            // obstacles; the framing over a level comes from its layout below.
            _ if el.category.starts_with("piece:") => false,
            "architecture" => false,
            "electrical" => matches!(el.category.as_str(), "equipment") || (el.category == "conduit" && el.run.is_empty()),
            _ => !matches!(el.category.as_str(), "room" | "device" | "space" | "zone"),
        };
        if !(keep && el.lo.is_finite() && el.hi.is_finite()) {
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
    let electrical = |g: &GearEl| {
        sp.mep.equipment.iter().find(|e| e.tag.eq_ignore_ascii_case(&g.tag)).is_some_and(|e| {
            let k = format!("{} {}", e.kind, e.title).to_ascii_lowercase();
            rl.gear.kinds.iter().any(|w| k.contains(w.as_str()))
        })
    };
    for g in gear.iter().filter(|g| electrical(g)) {
        let floor = sp.level(&g.level).map_or(0.0, |l| l.elevation_ft * upf);
        let reach = rl.gear.working_depth_ft.max(0.0) * upf;
        obstacles.entry(g.level.to_ascii_lowercase()).or_default().push(Obs {
            handle: g.handle,
            lo: Vec3::new(g.lo.x - reach, g.lo.y - reach, floor),
            hi: Vec3::new(g.hi.x + reach, g.hi.y + reach, (floor + rl.gear.working_height_ft * upf).max(g.hi.z)),
            category: WORKING_SPACE.into(),
            system: g.tag.clone(),
            trade: "electrical",
        });
    }
    // Each level's stacking move (a level's built objects all moved by the same): a riser to a
    // panel on another level stands where the stacked building has that panel.
    let mut level_off: HashMap<String, Vec2> = HashMap::new();
    for e in d.model.iter() {
        if super::wallgen::made_as(e).is_some()
            && let Some(l) = super::bspec::made_level(e)
        {
            level_off.entry(l.to_ascii_lowercase()).or_insert_with(|| super::bspec::stacked_by(e));
        }
    }
    // Each level's stacking move between plans: recorded on its objects once the build stacked
    // them; before that (model.build routes before it stacks) the move the stack will make.
    let stacked_now = level_off.values().any(|v| v.len() > 1e-9);
    let planned: HashMap<String, Vec2> = if !stacked_now && sp.stack {
        super::bspec::stack_offsets(d, sp).into_iter().map(|(n, v)| (n.to_ascii_lowercase(), v)).collect()
    } else {
        HashMap::new()
    };
    let move_of = |l: &str| {
        let k = l.to_ascii_lowercase();
        planned.get(&k).or_else(|| level_off.get(&k)).copied().unwrap_or(Vec2::new(0.0, 0.0))
    };
    // The framing over each level (the floor joists above it, the roof's trusses), as the trades'
    // layout makes them from the joist and truss documents: each member over the level's space,
    // on its plan.
    let members = super::tradeatoms::framing_members(d, sp);
    let mut framing: BTreeMap<String, Vec<(Obs, Frame)>> = BTreeMap::new();
    if !members.is_empty() {
        let mut storeys: Vec<&cadaddy_walk3d::Level> = sp.levels.iter().filter(|l| l.sheet.is_none()).collect();
        storeys.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
        for (li, l) in storeys.iter().enumerate() {
            let floor = l.elevation_ft * upf;
            let top = storeys.get(li + 1).map_or((l.elevation_ft + l.height_ft.max(1.0)) * upf, |n| n.elevation_ft * upf);
            let list = framing.entry(l.name.to_ascii_lowercase()).or_default();
            for m in members.iter().filter(|m| m.z1 > floor && m.z0 < top + upf / 12.0) {
                // From its floor's own plan onto this level's.
                let own = d.model.get(m.host).map_or(Vec2::new(0.0, 0.0), |e| super::bspec::stacked_by(e));
                let o = own - move_of(&m.level) + move_of(&l.name);
                let (a, b) = (m.a - o, m.b - o);
                let n = {
                    let d = b - a;
                    let len = d.len().max(1e-9);
                    Vec2::new(-d.y / len, d.x / len) * (m.t / 2.0)
                };
                let (pa, pb, pc, pd) = (a + n, a - n, b + n, b - n);
                let lo = Vec3::new(pa.x.min(pb.x).min(pc.x).min(pd.x), pa.y.min(pb.y).min(pc.y).min(pd.y), m.z0);
                let hi = Vec3::new(pa.x.max(pb.x).max(pc.x).max(pd.x), pa.y.max(pb.y).max(pc.y).max(pd.y), m.z1);
                list.push((
                    Obs { handle: m.host, lo, hi, category: FRAMING.into(), system: m.item.clone(), trade: "framing" },
                    Frame { a, b, t: m.t, truss: m.kind == "truss" },
                ));
            }
        }
    }
    let mut fields: BTreeMap<String, Field> = BTreeMap::new();
    for (k, v) in obstacles {
        let fr = framing.remove(&k).unwrap_or_default();
        fields.insert(k, Field::with_framing(v, fr, upf));
    }
    for (k, fr) in framing.into_iter().filter(|(_, f)| !f.is_empty()) {
        fields.insert(k, Field::with_framing(Vec::new(), fr, upf));
    }
    let member_source = members.first().map(|m| m.source.clone()).unwrap_or_default();
    // The built ceilings by level (each room's from its elevations or RCP area, the RCPs'): their
    // outline on the level's plan and height. A box's raceway rises into the ceiling space over
    // its own room's ceiling, not over a level-wide default.
    let mut ceilings: HashMap<String, Vec<(Vec<Vec2>, f64, f64)>> = HashMap::new();
    for e in d.model.iter().filter(|e| super::wallgen::made_as(e).as_deref() == Some("ceiling")).take(MAX_DEVICES) {
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let Some(level) = super::bspec::made_level(e) else { continue };
        let off = super::bspec::stacked_by(e);
        let z = m.vertices.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max);
        let hull = super::mep::hull(&m.vertices.iter().take(10_000).map(|v| v.xy() - off).collect::<Vec<_>>());
        let area = cadaddy_geom::shoelace(&hull).abs();
        if hull.len() >= 3 && z.is_finite() && area > 0.0 {
            ceilings.entry(level.to_ascii_lowercase()).or_default().push((hull, z, area));
        }
    }
    // The ceiling over a plan point of a level: the smallest built ceiling round it (a room's
    // inside a larger one's), else none.
    let ceiling_at = |level: &str, p: Vec2| -> Option<f64> {
        ceilings.get(&level.to_ascii_lowercase())?.iter().filter(|(h, _, _)| inside(p, h)).min_by(|a, b| a.2.total_cmp(&b.2)).map(|(_, z, _)| *z)
    };
    // A box's room's ceiling: at the box, else a little into the room before its face (a box in
    // a wall stands between the rooms' outlines).
    let box_ceiling = |level: &str, at: Vec2, face: Option<Vec2>| -> Option<f64> {
        ceiling_at(level, at).or_else(|| {
            let f = face?;
            ceiling_at(level, at + f * (0.75 * upf)).or_else(|| ceiling_at(level, at - f * (0.75 * upf)))
        })
    };
    let empty = Field::default();
    // A point of `from`'s own plan on `to`'s.
    let carry = |p: Vec2, from: &str, to: &str| p + move_of(from) - move_of(to);
    let gear_ctr = |g: &GearEl| Vec2::new((g.lo.x + g.hi.x) / 2.0, (g.lo.y + g.hi.y) / 2.0);
    let elev = |l: &str| sp.level(l).map_or(0.0, |x| x.elevation_ft);

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
            gear: None,
            part: String::new(),
        })
        .collect();
    // A circuit whose breaker feeds gear is that feeder (routed with the feeders, by the feeder
    // schedule's conductors and raceway), not a branch circuit to devices.
    let mut fed_as_feeders: Vec<Value> = Vec::new();
    circs.retain(|c| {
        let Some(f) = feeder_of(sp, &c.panel, &c.label) else { return true };
        if let Some(t) = sp.tags.get(c.tag) {
            out.feeders.insert(t.id.clone());
        }
        if fed_as_feeders.len() < MAX_ITEMS {
            fed_as_feeders.push(json!({
                "circuit": c.label,
                "level": c.level,
                "feeder": f.tag,
                "from": f.from,
                "to": f.to,
                "source": f.source,
            }));
        }
        false
    });
    let feeder_circuits = out.feeders.len();
    let mut by_label: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (ci, c) in circs.iter().enumerate() {
        by_label.entry((c.level.to_ascii_lowercase(), c.label.to_ascii_uppercase())).or_default().push(ci);
    }
    let near = NEAR_RUN_FT * upf;
    let mut no_circuit = 0usize;
    let mut no_tag = 0usize;
    let mut controllers: Vec<usize> = Vec::new();
    // Devices whose label has no circuit tag on their level (resolved below, Q6206), and those
    // with no circuit at all.
    let mut tagless: Vec<usize> = Vec::new();
    let mut uncircuited: Vec<usize> = Vec::new();
    for (di, dv) in devs.iter().enumerate() {
        if dv.circuit.is_empty() {
            if dv.controls.is_empty() {
                no_circuit += 1;
                uncircuited.push(di);
            } else {
                controllers.push(di);
            }
            continue;
        }
        let Some(cands) = by_label.get(&(dv.level.to_ascii_lowercase(), dv.circuit.to_ascii_uppercase())) else {
            no_tag += 1;
            tagless.push(di);
            continue;
        };
        // A building circuit (not a typical room's own): its tag's id says (the spec's tag the
        // circuit was read as).
        let room_first = |ci: &usize| sp.tags.get(circs.get(*ci).map_or(usize::MAX, |c| c.tag)).is_none_or(|t| !t.id.contains(":room-"));
        let dist_to = |ci: usize| {
            circs
                .get(ci)
                .and_then(|c| sp.tags.get(c.tag))
                .and_then(|t| pts_of.get(t.id.as_str()))
                .map_or(f64::INFINITY, |ps| ps.iter().map(|p| p.dist(dv.at)).fold(f64::INFINITY, f64::min))
        };
        // A typical room's own circuit of the label in the device's own room (its tag names the
        // room): that one, whatever its estimated route passes (Q6206).
        let own_room = |ci: &usize| {
            !dv.room.trim().is_empty()
                && circs
                    .get(*ci)
                    .and_then(|c| sp.tags.get(c.tag))
                    .and_then(|t| field_of(t, "room"))
                    .is_some_and(|r| r.eq_ignore_ascii_case(dv.room.trim()))
        };
        let mine = cands.iter().copied().filter(|ci| own_room(ci)).min_by(|a, b| dist_to(*a).total_cmp(&dist_to(*b)));
        let best = cands
            .iter()
            .copied()
            .map(|ci| (room_first(&ci), dist_to(ci), ci))
            .min_by(|a, b| (a.0, a.1).partial_cmp(&(b.0, b.1)).unwrap_or(std::cmp::Ordering::Equal));
        // A typical room's circuit takes only the devices by its route or in its room; a building
        // circuit takes its label's devices wherever they are on its level.
        let chosen = mine
            .or_else(|| best.and_then(|(building, dd, ci)| (dd <= near || building).then_some(ci)))
            .or_else(|| cands.iter().copied().find(|ci| room_first(ci)));
        match chosen.and_then(|ci| circs.get_mut(ci)) {
            Some(c) => c.devs.push(di),
            None => {
                no_tag += 1;
                tagless.push(di);
            }
        }
    }
    // -- A label with no circuit tag on its level (Q6206): read through the panel schedules (the
    // panel that lists that number), a tag of the circuit on another level, and the riser; each
    // resolved label its own circuit on the level (a typical room's own in each room, where the
    // label is a typical room's), routed like the rest; the rest listed with why. --
    let base_n = sp.tags.len();
    let mut extra_tags: Vec<ElementTag> = Vec::new();
    let mut resolved_rows: Vec<Value> = Vec::new();
    let mut unresolved_rows: Vec<Value> = Vec::new();
    let no_tag_before = no_tag;
    let mut resolved_devices = 0usize;
    {
        let mut any_level: HashMap<String, Vec<&ElementTag>> = HashMap::new();
        for (_, t) in &circuit_tags {
            if let Some(l) = field_of(t, "circuit") {
                any_level.entry(l.to_ascii_uppercase()).or_default().push(t);
            }
        }
        let mut groups: BTreeMap<(String, String, Option<String>), Vec<usize>> = BTreeMap::new();
        for di in &tagless {
            let Some(dv) = devs.get(*di) else { continue };
            let up = dv.circuit.trim().to_ascii_uppercase();
            // A typical room's label (its circuits are the rooms' own, or its panel's schedule is
            // a typical one): a room's own circuit, one per room.
            let panel = super::printcond::parse_circuit(&up).map(|(p, _)| p).unwrap_or_default();
            let typical = any_level.get(&up).is_some_and(|ts| ts.iter().any(|t| t.id.contains(":room-")))
                || sp.mep.panel_schedules.iter().any(|s| s.panel.trim().eq_ignore_ascii_case(&panel) && s.typical_of.is_some());
            let room = if typical { Some(dv.room.trim().to_string()) } else { None };
            groups.entry((dv.level.clone(), up, room)).or_default().push(*di);
        }
        for ((level, up, room), ds) in groups.into_iter().take(MAX_CIRCUITS) {
            let others: Vec<&ElementTag> =
                any_level.get(&up).map(|ts| ts.iter().copied().filter(|t| !same_level(&t.level, &level)).collect()).unwrap_or_default();
            let label = ds.first().and_then(|i| devs.get(*i)).map(|d| d.circuit.trim().to_string()).unwrap_or_else(|| up.clone());
            let kinds: BTreeMap<&str, usize> = ds.iter().filter_map(|i| devs.get(*i)).fold(BTreeMap::new(), |mut m, d| {
                *m.entry(d.kind.as_str()).or_default() += 1;
                m
            });
            match gap::resolve_label(sp, &label, &others, rl) {
                Ok(r) => {
                    let panel = super::printcond::parse_circuit(&label).map(|(p, _)| p).unwrap_or_default();
                    if feeder_of(sp, &panel, &label).is_some() {
                        unresolved_rows.push(json!({"level": level, "circuit": label, "devices": ds.len(), "kinds": kinds, "reason": "the schedule's circuit is a feeder to gear: not a branch circuit"}));
                        continue;
                    }
                    let at = ds.first().and_then(|i| devs.get(*i)).map_or(Vec2::new(0.0, 0.0), |d| d.at);
                    let t = gap::resolved_tag(&label, &level, room.as_deref(), [at.x / upf, at.y / upf], &r, ds.len());
                    if resolved_rows.len() < MAX_ITEMS {
                        resolved_rows.push(json!({"level": level, "circuit": label, "room": room, "devices": ds.len(), "kinds": kinds, "by": r.by, "source": r.source, "tag": t.id}));
                    }
                    resolved_devices += ds.len();
                    circs.push(Circ { tag: base_n + extra_tags.len(), label, panel, level, devs: ds, gear: None, part: String::new() });
                    extra_tags.push(t);
                }
                Err(why) => {
                    if unresolved_rows.len() < MAX_ITEMS {
                        unresolved_rows
                            .push(json!({"level": level, "circuit": label, "room": room, "devices": ds.len(), "kinds": kinds, "reason": why}));
                    }
                }
            }
        }
    }
    no_tag = no_tag.saturating_sub(resolved_devices);
    let tag_at = |i: usize| -> Option<&ElementTag> { if i < base_n { sp.tags.get(i) } else { extra_tags.get(i - base_n) } };
    // -- A switch or sensor the prints give no circuit to is on the circuit of what it controls
    // (its switch leg): with the circuit most of its luminaires are on, the nearest of them
    // breaking a tie, so its box gets its drop and the leg to them is routed and measured. --
    let mut switch_legs = 0usize;
    let mut switches_unjoined = 0usize;
    if !controllers.is_empty() {
        let dev_at: HashMap<Handle, usize> = devs.iter().enumerate().map(|(i, d)| (d.handle, i)).collect();
        let mut circ_of: HashMap<usize, usize> = HashMap::new();
        for (ci, c) in circs.iter().enumerate() {
            for d in &c.devs {
                circ_of.insert(*d, ci);
            }
        }
        for di in controllers {
            let Some(dv) = devs.get(di) else { continue };
            let mut votes: BTreeMap<usize, (usize, f64)> = BTreeMap::new();
            for t in dv.controls.iter().filter_map(|h| dev_at.get(h)) {
                let Some((ci, other)) = circ_of.get(t).copied().zip(devs.get(*t)) else { continue };
                if !same_level(&other.level, &dv.level) {
                    continue;
                }
                let v = votes.entry(ci).or_insert((0, f64::INFINITY));
                v.0 += 1;
                v.1 = v.1.min(other.at.dist(dv.at));
            }
            let best = votes.into_iter().max_by(|a, b| a.1.0.cmp(&b.1.0).then(b.1.1.total_cmp(&a.1.1))).map(|(ci, _)| ci);
            match best.and_then(|ci| circs.get_mut(ci)) {
                Some(c) => {
                    c.devs.push(di);
                    switch_legs += 1;
                }
                None => {
                    no_circuit += 1;
                    switches_unjoined += 1;
                }
            }
        }
    }

    // -- A circuit of a panel there is one of per unit (`P` → P-1A, P-1B …): its devices split
    // by the nearest copy on their level, each part to its own panel. --
    let mut split: Vec<Circ> = Vec::new();
    for c in circs.drain(..) {
        let up = c.panel.to_ascii_uppercase();
        let copies: Vec<usize> = gear
            .iter()
            .enumerate()
            .filter(|(_, g)| same_level(&g.level, &c.level) && !up.is_empty() && g.tag.to_ascii_uppercase().starts_with(&format!("{up}-")))
            .map(|(i, _)| i)
            .collect();
        if copies.len() < 2 || c.devs.len() < 2 {
            split.push(c);
            continue;
        }
        let mut parts: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for di in &c.devs {
            let Some(dv) = devs.get(*di) else { continue };
            let near = copies.iter().copied().min_by(|a, b| {
                let d = |i: usize| gear.get(i).map_or(f64::INFINITY, |g| Vec2::new((g.lo.x + g.hi.x) / 2.0, (g.lo.y + g.hi.y) / 2.0).dist(dv.at));
                d(*a).total_cmp(&d(*b))
            });
            if let Some(g) = near {
                parts.entry(g).or_default().push(*di);
            }
        }
        if parts.len() < 2 {
            split.push(c);
            continue;
        }
        for (g, ds) in parts {
            let tag = gear.get(g).map(|x| x.tag.replace(['/', '#', '@'], "-")).unwrap_or_default();
            split.push(Circ {
                tag: c.tag,
                label: c.label.clone(),
                panel: c.panel.clone(),
                level: c.level.clone(),
                devs: ds,
                gear: Some(g),
                part: tag,
            });
        }
    }
    let circs = split;

    // -- What each level gives: floor, ceiling tiers, corridors. --
    // `over`: the highest built ceiling under the run (its rooms'), else the level's ceiling.
    let tiers_of = |level: &str, over: Option<f64>| -> (f64, Vec<f64>) {
        let Some(l) = sp.level(level) else { return (0.0, vec![rl.route.default_ceiling_ft * upf]) };
        let floor = l.elevation_ft * upf;
        let ceiling = if l.ceiling_ft > 0.0 { l.ceiling_ft } else { rl.route.default_ceiling_ft };
        let ceiling = over.map(|z| (z - floor) / upf).filter(|c| *c > 0.0 && *c < l.height_ft.max(ceiling + 1.0)).unwrap_or(ceiling);
        let z0 = floor + (ceiling + rl.route.above_ceiling_in / 12.0) * upf;
        let mut zmax = floor + (l.height_ft.max(ceiling + 1.0) - rl.route.structure_depth_in / 12.0) * upf;
        // The documents' framing over the level is its structure: the tiers go up through the
        // joists' web (to its hole zone's top), not to a default depth under the floor.
        if let Some(top) = fields.get(&level.to_ascii_lowercase()).and_then(Field::framing_top) {
            let keep = (rl.framing.flange_in.max(0.0) + rl.framing.edge_in.max(0.0) + 0.5) / 12.0 * upf;
            if top - keep > floor && top - keep < floor + l.height_ft.max(ceiling + 1.0) * upf + upf {
                zmax = zmax.max(top - keep);
            }
        }
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
    let approved_ve = super::valueeng::approved_circuits(d);
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

    // The circuit's load from its panel's schedule (VA), for the voltage drop.
    let load_of = |panel: &str, label: &str| -> Option<f64> {
        let num = super::printcond::parse_circuit(label)?.1;
        let first = num.split(',').next()?.trim().to_string();
        let s = sp.mep.panel_schedules.iter().find(|p| p.panel.trim().eq_ignore_ascii_case(panel.trim()))?;
        s.circuits.iter().find(|c| c.number.trim() == first)?.load_va.filter(|v| v.is_finite() && *v > 0.0 && *v < 1.0e6)
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
    let mut bored: Vec<Bored> = Vec::new();
    let mut unrouted: Vec<Value> = Vec::new();
    let mut methods_used: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut runs: Vec<MepRun> = Vec::new();
    let mut tags: Vec<ElementTag> = Vec::new();
    // What enters each device's box: count and size, and the circuit (part) whose final size it
    // takes (a circuit upsized for its voltage drop later in the routing), none: as given.
    let mut box_in: HashMap<usize, Vec<(u32, String, Option<String>)>> = HashMap::new();
    let mut in_wall_legs = 0usize;
    let mut ceiling_legs = 0usize;
    // Each routed leg's path as a cable runs it where that differs (through the studs).
    let mut cable_paths: Vec<MepRun> = Vec::new();
    let mut stud_legs = 0usize;
    // Each circuit's (part's) lengths: its legs in its area, its share of the homeruns (the
    // lead's homerun, a member's tap), and its conductors' feet with their makeup.
    let mut lens: HashMap<usize, Lens> = HashMap::new();
    let ar = super::mepatoms::rules();
    let box_makeup = ar.makeup_ft.max(0.0);
    let panel_makeup = ar.panel_makeup_ft.max(box_makeup);
    let wires_in = |parts: &[Part]| -> f64 { f64::from(parts.iter().fold(0u32, |n, p| n.saturating_add(p.0))) };
    let mut drops_moved = 0usize;
    let mut drop_clashes = 0usize;
    // The switch loops (Q6206): this circuit's (id → ways), every circuit's tags, the legs.
    let mut switch_tags: BTreeMap<String, u8> = BTreeMap::new();
    let mut loop_tags: Vec<ElementTag> = Vec::new();
    let mut switch_loop_legs = 0usize;
    // Each circuit's own cable feet by size.
    let mut cable_sizes: HashMap<usize, BTreeMap<String, f64>> = HashMap::new();
    let mut cable_legs: HashMap<usize, BTreeMap<u32, f64>> = HashMap::new();
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
            "clearance_in": if o.category == FRAMING { 0.0 } else { need },
            "fix": if o.category == FRAMING {
                if matches!(what, "drop" | "vertical conduit" | "riser") {
                    format!(
                        "the {what} stands in the {} over it: move the box or the {what} into the bay beside it (the member is never notched or bored through a flange)",
                        o.system
                    )
                } else {
                    format!(
                        "{} ({}) at every tier of the ceiling space: keep the run in the bay along the members, cross the {} in its web's hole zone ({} in flanges, {} in of web kept, holes bigger than {} in not bored, {} ft from the bearing for the largest), or have the joist maker check the hole",
                        if b.why.is_empty() { "the framing stops it" } else { b.why },
                        o.system,
                        o.system,
                        rl.framing.flange_in,
                        rl.framing.edge_in,
                        rl.framing.min_end_ft.last().map_or(0.0, |r| r[0]),
                        rl.framing.min_end_ft.last().map_or(0.0, |r| r[1]),
                    )
                }
            } else if matches!(what, "drop" | "vertical conduit" | "riser") {
                format!(
                    "the {what} can't stand clear of the {} {} (over its box or within {} ft along its wall): move the box or the {} sideways, or coordinate a sleeve with the {} trade",
                    o.category, o.system, rl.route.drop_reach_ft, o.category, o.trade
                )
            } else {
                format!(
                    "no square route at the level's tiers clears the {} {}: offset under it (bottom of conduit {:.2} ft AFF) or over it ({:.2} ft AFF), or coordinate a sleeve with the {} trade",
                    o.category, o.system, under, over, o.trade
                )
            },
        }));
    };
    for (ci, c) in circs.iter().enumerate() {
        if runs.len() >= MAX_RUNS {
            break;
        }
        let Some(tag) = tag_at(c.tag) else { continue };
        let members: Vec<&Dev> = c.devs.iter().filter_map(|i| devs.get(*i)).collect();
        if members.is_empty() {
            unrouted.push(json!({"circuit": c.label, "level": c.level, "reason": "no device of this circuit is in the model"}));
            continue;
        }
        let field = fields.get(&c.level.to_ascii_lowercase()).unwrap_or(&empty);
        // Over the highest ceiling of its boxes' rooms.
        let over = members.iter().filter_map(|m| box_ceiling(&c.level, m.at, m.face)).fold(None, |a: Option<f64>, z| Some(a.map_or(z, |a| a.max(z))));
        let (floor, tiers) = tiers_of(&c.level, over);
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
        // An approved (or chosen) value engineering alternative keeps its cable on every route.
        let (method, msrc, cable) = match approved_ve.get(tag.id.as_str()) {
            Some((m, why)) => (m.clone(), format!("{why}: {m}"), super::mepatoms::rules().cable_named(m).is_some()),
            None => (method, msrc, cable),
        };
        let e = methods_used.entry(format!("{area}: {method}")).or_insert((0, msrc.clone()));
        e.0 += 1;
        // The panel: this level's gear of the circuit's panel nearest the devices.
        let centre = members.iter().fold(Vec2::new(0.0, 0.0), |s, m| s + m.at) * (1.0 / members.len() as f64);
        let room_circuit = tag.id.contains(":room-");
        let panel_up = c.panel.to_ascii_uppercase();
        let gi = c.gear.or_else(|| {
            gear.iter()
                .enumerate()
                .filter(|(_, g)| {
                    let t = g.tag.to_ascii_uppercase();
                    same_level(&g.level, &c.level) && !panel_up.is_empty() && (t == panel_up || t.starts_with(&format!("{panel_up}-")))
                })
                .map(|(i, g)| (i, Vec2::new((g.lo.x + g.hi.x) / 2.0, (g.lo.y + g.hi.y) / 2.0).dist(centre)))
                .filter(|(_, dd)| !room_circuit || *dd <= ROOM_PANEL_FT * upf)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        });
        // None of its panel on its level: that panel (its own tag, not a unit's copy) on another
        // storey, the nearest, reached by a riser through the floors between.
        let gi = gi.or_else(|| {
            if panel_up.is_empty() {
                return None;
            }
            let here = elev(&c.level);
            gear.iter()
                .enumerate()
                .filter(|(_, g)| !same_level(&g.level, &c.level) && g.tag.eq_ignore_ascii_case(&c.panel))
                .map(|(i, g)| (i, (elev(&g.level) - here).abs(), carry(gear_ctr(g), &g.level, &c.level).dist(centre)))
                .min_by(|a, b| (a.1, a.2).partial_cmp(&(b.1, b.2)).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _, _)| i)
        });
        let panel_at = gi.and_then(|i| gear.get(i)).map(|g| carry(gear_ctr(g), &g.level, &c.level));
        let pts: Vec<Vec2> = members.iter().map(|m| m.at).collect();
        let nearest_panel = |cand: &[usize]| {
            panel_at.and_then(|p| {
                cand.iter().copied().min_by(|a, b| {
                    let d = |i: usize| pts.get(i).map_or(f64::INFINITY, |q| manhattan(*q, p));
                    d(*a).total_cmp(&d(*b))
                })
            })
        };
        // A switch's box is the end of its own loop (Q6206): the circuit's other devices joined
        // device to device, each switch on a leg of its own from the nearest of what it controls
        // (else the nearest of the rest). All switches, or none: device to device.
        let core: Vec<usize> = (0..members.len()).filter(|i| members.get(*i).is_some_and(|m| !m.switch)).collect();
        let (root, tree) = if core.is_empty() || core.len() == members.len() {
            let all: Vec<usize> = (0..members.len()).collect();
            let root = nearest_panel(&all).unwrap_or(0);
            (root, spanning_tree(&pts, root))
        } else {
            let cpts: Vec<Vec2> = core.iter().filter_map(|i| pts.get(*i).copied()).collect();
            let local: Vec<usize> = (0..core.len()).collect();
            let croot = panel_at
                .and_then(|p| {
                    local.iter().copied().min_by(|a, b| {
                        let d = |i: usize| cpts.get(i).map_or(f64::INFINITY, |q| manhattan(*q, p));
                        d(*a).total_cmp(&d(*b))
                    })
                })
                .unwrap_or(0);
            let mut t: Vec<(usize, usize)> =
                spanning_tree(&cpts, croot).into_iter().filter_map(|(a, b)| Some((*core.get(a)?, *core.get(b)?))).collect();
            for (i, m) in members.iter().enumerate().filter(|(_, m)| m.switch) {
                let ctl: Vec<usize> = core.iter().copied().filter(|j| members.get(*j).is_some_and(|x| m.controls.contains(&x.handle))).collect();
                let pool = if ctl.is_empty() { &core } else { &ctl };
                let near = pool.iter().copied().min_by(|a, b| {
                    let d = |j: usize| pts.get(j).map_or(f64::INFINITY, |q| manhattan(*q, m.at));
                    d(*a).total_cmp(&d(*b))
                });
                if let Some(j) = near {
                    t.push((j, i));
                }
            }
            (core.get(croot).copied().unwrap_or(0), t)
        };
        let size = min_in;
        // A part of a split circuit is its own (the circuit's id and its panel's).
        let cid = if c.part.is_empty() { tag.id.clone() } else { format!("{}@{}", tag.id, c.part) };
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
            // A cable between two boxes of a room's walls runs through the studs.
            let stud = stud_path((a.at, a.top, a.face, &a.room), (b.at, b.top, b.face, &b.room), floor, upf, rl);
            if let Some((pts, bot)) = stud.as_ref().filter(|_| cable) {
                in_wall_legs += 1;
                stud_legs += 1;
                r.points = pts.clone();
                r.bottoms_ft = bot.clone();
                r.source_sheet = IN_WALL.into();
                tree_len += run_ft(&r, upf);
            } else if same_wall {
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
                // Each end rises over its box, or along its wall where that is blocked.
                let z0 = tiers.first().copied().unwrap_or(a.top.max(b.top));
                let own: Vec<Handle> = gi.and_then(|i| gear.get(i)).map(|g| g.handle).into_iter().collect();
                let (qa, ja, ba) = rise_point(a.at, a.top, a.face, z0, r_of(size), field, upf, &own, rl);
                let (qb, jb, bb) = rise_point(b.at, b.top, b.face, z0, r_of(size), field, upf, &own, rl);
                drops_moved += usize::from(qa.dist(a.at) > upf / 96.0) + usize::from(qb.dist(b.at) > upf / 96.0);
                let leg = route_leg(qa, qb, &tiers, r_of(size), field, upf, &[], rl);
                for bl in &leg.blocked {
                    add_clash(&mut clashes, field, bl, "branch conduit", &c.label, &c.level, size);
                }
                for (blk, q) in [(ba, qa), (bb, qb)] {
                    if let Some(i) = blk {
                        drop_clashes += 1;
                        add_clash(
                            &mut clashes,
                            field,
                            &Blocked { obstacle: i, at: Vec3::new(q.x, q.y, z0), z: z0, why: "" },
                            "drop",
                            &c.label,
                            &c.level,
                            size,
                        );
                    }
                }
                let mut cols = Vec::new();
                leg_columns(&leg, z0, r_of(size), field, upf, &[], rl, &mut cols);
                for bl in &cols {
                    drop_clashes += 1;
                    add_clash(&mut clashes, field, bl, "vertical conduit", &c.label, &c.level, size);
                }
                let (mut pts, mut bot) = from_box(a.at, a.top, (qa, ja), &leg, floor, upf);
                to_box(&mut pts, &mut bot, b.at, b.top, (qb, jb), floor, upf);
                r.points = pts;
                r.bottoms_ft = bot;
                tree_len += run_ft(&r, upf);
                penetrate(&mut penetrations, &rated, &c.level, &leg, &c.label, size, upf, floor, rl);
                for bl in bore(&mut bored, field, &leg, &c.label, &c.level, size, upf, rl) {
                    add_clash(&mut clashes, field, &bl, "branch conduit", &c.label, &c.level, size);
                }
            }
            edge_ft.push((*u, *v, tree_len - before));
            // A leg into a switch's box carries the switch loop (Q6206): the line, the switched
            // leg(s), the travelers and the grounded conductor (rules data), its own conductors.
            let ways = [a, b].iter().filter(|m| m.switch).map(|m| m.ways).max();
            let (leg_parts, leg_id) = match ways {
                Some(w) => {
                    let hot = hots.first().map_or_else(|| "12".to_string(), |h| h.1.clone());
                    let mk = gap::switch_leg_makeup(&hot, w);
                    let lp = super::mepatoms::conductor_parts(&mk);
                    let id = format!("{cid}~sw{}", lp.iter().filter(|p| !p.2).map(|p| p.0).sum::<u32>());
                    switch_tags.entry(id.clone()).or_insert(w);
                    switch_loop_legs += 1;
                    (lp, id)
                }
                None => (parts.clone(), cid.clone()),
            };
            r.id = format!("{leg_id}#{leg_n}");
            r.size_source = format!("minimum: {min_src}");
            r.height_source = ROUTED.into();
            // The circuit's own cable on this leg: through the studs where it runs there.
            let mut cable_leg_ft = run_ft(&r, upf);
            if let Some((pts, bot)) = stud.clone() {
                let mut alt = r.clone();
                alt.points = pts;
                alt.bottoms_ft = bot;
                alt.source_sheet = IN_WALL.into();
                cable_leg_ft = run_ft(&alt, upf);
                if !cable && cable_paths.len() < MAX_RUNS {
                    cable_paths.push(alt);
                }
            }
            let l = lens.entry(ci).or_default();
            let ft = run_ft(&r, upf);
            l.in_area += ft;
            l.wire += (ft + 2.0 * box_makeup) * wires_in(&leg_parts);
            l.own_wire += (ft + 2.0 * box_makeup) * wires_in(&leg_parts);
            l.cable += cable_leg_ft + 2.0 * box_makeup;
            // By its insulated count (0: the circuit's own conductors), sized once the circuit is.
            let loop_n = if ways.is_some() { leg_parts.iter().filter(|p| !p.2).map(|p| p.0).sum::<u32>() } else { 0 };
            *cable_legs.entry(ci).or_default().entry(loop_n).or_default() += cable_leg_ft + 2.0 * box_makeup;
            runs.push(r);
            let leg_hots: Vec<(u32, String)> = leg_parts.iter().filter(|p| !p.2).map(|p| (p.0, p.1.clone())).collect();
            for end in [*u, *v] {
                box_in
                    .entry(c.devs.get(end).copied().unwrap_or(usize::MAX))
                    .or_default()
                    .extend(leg_hots.iter().map(|(n, sz)| (*n, sz.clone(), Some(cid.clone()))));
            }
        }
        let mut t = tag.clone();
        if !c.part.is_empty() {
            t.id = cid.clone();
            t.set("split_of", tag.id.clone(), "computed");
            t.set("panel_copy", c.part.clone(), "computed");
        }
        t.set("route", ROUTED, "computed");
        t.set(
            "conduit",
            method.clone(),
            if msrc.starts_with("spec") {
                "spec"
            } else if msrc.starts_with("project") || approved_ve.contains_key(tag.id.as_str()) {
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
            (None, Some(other)) => {
                unrouted.push(json!({"circuit": c.label, "level": c.level, "reason": format!("its panel {} is on {other}: no riser to it", c.panel)}))
            }
            (None, None) => {
                unrouted.push(json!({"circuit": c.label, "level": c.level, "reason": format!("panel {} is not in the model: no homerun", c.panel)}))
            }
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
                } else if dv.is_finite()
                    && !du.is_finite()
                    && let Some(x) = far.get_mut(*u)
                {
                    *x = dv + ft;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let farthest = far.iter().copied().filter(|v| v.is_finite()).fold(0.0, f64::max);
        homes.push(Home { circ: ci, root, gear: gi, level: c.level.clone(), len_tree_ft: farthest, cable, method });
        // Each switch loop's own tag: the circuit's, its conductors the loop's (a part of the
        // circuit as read, like a split circuit's).
        for (id, w) in std::mem::take(&mut switch_tags) {
            let mut st = t.clone();
            st.id = id;
            let hot = hots.first().map_or_else(|| "12".to_string(), |h| h.1.clone());
            st.set("conductors", gap::switch_leg_makeup(&hot, w), "rule");
            st.set("split_of", tag.id.clone(), "computed");
            st.set("switch_loop_of", cid.clone(), "computed");
            st.set("switch_loop", gap::rules().switch_loop.source.clone(), "rule");
            loop_tags.push(st);
        }
        tags.push(t);
    }

    // -- Homeruns: grouped by panel and nearness, each from its lead's box to the panel's top. --
    let mut slots_used: HashMap<usize, BTreeSet<usize>> = HashMap::new();
    let mut conflicts: Vec<Value> = Vec::new();
    let mut entry_unknown: BTreeSet<String> = BTreeSet::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    {
        let mut order: Vec<usize> = (0..homes.len()).filter(|i| homes.get(*i).is_some_and(|h| h.gear.is_some())).collect();
        let root_at = |h: &Home| circs.get(h.circ).and_then(|c| c.devs.get(h.root)).and_then(|d| devs.get(*d)).map(|d| d.at);
        // By panel and method, the root nearest its panel first: it leads its group.
        let to_gear = |h: &Home| {
            let g = h.gear.and_then(|i| gear.get(i)).map(|g| carry(gear_ctr(g), &g.level, &h.level));
            match (root_at(h), g) {
                (Some(p), Some(q)) => manhattan(p, q),
                _ => f64::INFINITY,
            }
        };
        order.sort_by(|a, b| {
            let (Some(ha), Some(hb)) = (homes.get(*a), homes.get(*b)) else { return std::cmp::Ordering::Equal };
            (ha.gear, ha.level.to_ascii_lowercase(), ha.method.as_str())
                .cmp(&(hb.gear, hb.level.to_ascii_lowercase(), hb.method.as_str()))
                .then(to_gear(ha).total_cmp(&to_gear(hb)))
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
                    && lead.level.eq_ignore_ascii_case(&h.level)
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
    let mut home_segs: Vec<gap::HomeSeg> = Vec::new();
    let mut upsized: Vec<Value> = Vec::new();
    let mut fill_rows: Vec<Value> = Vec::new();
    let mut derated: Vec<Value> = Vec::new();
    let mut corridor_homes = 0usize;
    let mut risers: Vec<Value> = Vec::new();
    for (gn, g) in groups.iter().enumerate() {
        if runs.len() >= MAX_RUNS {
            break;
        }
        let Some(lead_h) = g.first().and_then(|i| homes.get(*i)) else { continue };
        let Some(lead_c) = circs.get(lead_h.circ) else { continue };
        let Some(gear_el) = lead_h.gear.and_then(|i| gear.get(i)) else { continue };
        let Some(lead_dev) = lead_c.devs.get(lead_h.root).and_then(|d| devs.get(*d)) else { continue };
        let field = fields.get(&lead_h.level.to_ascii_lowercase()).unwrap_or(&empty);
        let (floor, _) = tiers_of(&lead_h.level, None);
        // A panel on another storey: the homerun runs to the point over (or under) it on its own
        // level's plan, then rises through the floors between into the panel's top (from above)
        // or bottom (from below).
        let gfield = fields.get(&gear_el.level.to_ascii_lowercase()).unwrap_or(&empty);
        let riser = !same_level(&gear_el.level, &lead_h.level);
        let from_below = riser && elev(&gear_el.level) > elev(&lead_h.level);
        let gear_z = if from_below { gear_el.lo.z } else { gear_el.hi.z };
        let gfloor = elev(&gear_el.level) * upf;
        let gtop = gfloor + sp.level(&gear_el.level).map_or(12.0, |l| l.height_ft) * upf;
        let gctr = carry(gear_ctr(gear_el), &gear_el.level, &lead_h.level);
        // Members tap into the lead's box.
        let mut labels: Vec<String> = Vec::new();
        // Each member's own way to the panel: its tap and its conductors (Q6206).
        let mut member_ways: Vec<(usize, f64, Vec<Part>)> = Vec::new();
        let mut group_parts: Vec<Part> = Vec::new();
        let mut longest = 0.0_f64;
        let mut trips: Vec<f64> = Vec::new();
        let mut path_len_ft = 0.0;
        // The path to the panel first (its length sizes the conductors).
        let lane = corridors(&lead_h.level);
        let far = manhattan(lead_dev.at, gctr) >= rl.route.corridor_min_ft * upf;
        let mut plan: Vec<Vec2> = match (far && !lane.is_empty()).then(|| corridor_path(&lane, lead_dev.at, gctr, upf)).flatten() {
            Some(p) => {
                corridor_homes += 1;
                p
            }
            None => vec![lead_dev.at, gctr],
        };
        // Over the highest ceiling along its way: its box's room's, the corridor's, the panel's room's.
        let over = std::iter::once(box_ceiling(&lead_h.level, lead_dev.at, lead_dev.face))
            .chain(plan.iter().skip(1).map(|p| ceiling_at(&lead_h.level, *p)))
            .flatten()
            .fold(None, |a: Option<f64>, z| Some(a.map_or(z, |a| a.max(z))));
        let (_, tiers) = tiers_of(&lead_h.level, over);

        // Each square piece of the plan path seated clear of the obstacles (the gear itself is
        // the target, not an obstacle).
        let z0 = tiers.first().copied().unwrap_or(lead_dev.top);
        let (qa, ja, ba) = rise_point(lead_dev.at, lead_dev.top, lead_dev.face, z0, r_of(min_in.max(1.0)), field, upf, &[gear_el.handle], rl);
        drops_moved += usize::from(qa.dist(lead_dev.at) > upf / 96.0);
        let mut home_pts: Vec<Vec2> = vec![qa];
        let mut home_z: Vec<f64> = Vec::new();
        let mut blocked: Vec<Blocked> = Vec::new();
        let mut vertical: Vec<Blocked> = Vec::new();
        if let Some(i) = ba {
            vertical.push(Blocked { obstacle: i, at: Vec3::new(qa.x, qa.y, z0), z: z0, why: "" });
        }
        if let Some(p0) = plan.first_mut() {
            *p0 = qa;
        }
        // The panel's own entry raceways (its feeder's, on its top): not in the homerun's way, and
        // their slots in the window taken.
        let pad = upf / 6.0;
        let entries: Vec<Handle> = gfield
            .obs
            .iter()
            .filter(|o| {
                matches!(o.category.as_str(), "conduit" | "fitting" | "cable")
                    && o.lo.x >= gear_el.lo.x - pad
                    && o.hi.x <= gear_el.hi.x + pad
                    && o.lo.y >= gear_el.lo.y - pad
                    && o.hi.y <= gear_el.hi.y + pad
                    && if from_below { o.hi.z <= gear_el.lo.z + pad } else { o.lo.z >= gear_el.hi.z - pad }
            })
            .map(|o| o.handle)
            .take(64)
            .collect();
        let mut skip = vec![gear_el.handle];
        skip.extend(entries.iter().copied());
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
            let mut tap_run_ft = 0.0;
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
                let l = lens.entry(h.circ).or_default();
                let ft = run_ft(&r, upf);
                tap_run_ft = ft;
                l.home += ft;
                l.wire += (ft + 2.0 * box_makeup) * wires_in(&super::mepatoms::conductor_parts(&mk));
                runs.push(r);
                penetrate(&mut penetrations, &rated, &c.level, &leg, &c.label, min_in, upf, floor, rl);
                for bl in bore(&mut bored, field, &leg, &c.label, &c.level, min_in, upf, rl) {
                    add_clash(&mut clashes, field, &bl, "homerun tap", &c.label, &c.level, min_in);
                }
                let hots: Vec<(u32, String, Option<String>)> =
                    super::mepatoms::conductor_parts(&mk).into_iter().filter(|p| !p.2).map(|p| (p.0, p.1, Some(tag.id.clone()))).collect();
                box_in.entry(c.devs.get(h.root).copied().unwrap_or(usize::MAX)).or_default().extend(hots.iter().cloned());
                box_in.entry(lead_c.devs.get(lead_h.root).copied().unwrap_or(usize::MAX)).or_default().extend(hots);
            }
            // Its conductors by the homerun's length (the legend's bands, else the drop).
            let ft = path_len_ft + tap_ft + h.len_tree_ft;
            longest = longest.max(ft);
            let mut parts = super::mepatoms::conductor_parts(&mk);
            let volts = rl.vdrop.voltage;
            // The schedule's load, else most of the breaker (a default).
            let (amps, load_src) = match load_of(&c.panel, &c.label) {
                Some(va) => (va / volts.max(1.0), format!("panel {} schedule: {va:.0} VA", c.panel)),
                None => (trip.unwrap_or(rl.vdrop.breaker_a) * rl.vdrop.load_pct / 100.0, format!("default: {}% of the breaker", rl.vdrop.load_pct)),
            };
            if let Some((need, why)) = size_for_length(ft, amps, volts, &len_table, rl).map(|(n, w)| (n, format!("{w}; load {load_src}"))) {
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
            member_ways.push((h.circ, tap_run_ft, parts.clone()));
            group_parts.extend(parts.into_iter().filter(|p| !p.2));
        }
        if g.len() > 1 {
            grouped_circuits += g.len();
        }
        let order = |s: &str| rl.vdrop.sizes.iter().position(|x| x == s).unwrap_or(0);
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
        // One ground, the largest size of the group after the adjustment upsized it (NEC 250.122:
        // the circuit's largest; (B) up in proportion when the ungrounded conductors go up).
        let ground = group_parts.iter().map(|p| p.1.clone()).max_by_key(|s| order(s)).unwrap_or_else(|| "12".into());
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
        let side = if from_below { "bottom" } else { "top" };
        let given = eq
            .and_then(|e| e.enclosure.as_ref())
            .map(|en| if from_below { en.bottom_entry_in } else { en.top_entry_in })
            .filter(|w| w[0] > 0.0 && w[1] > 0.0);
        // None given: the box's top less its margins, derived (and asked).
        let derived = given
            .is_none()
            .then(|| {
                let (dx, dy) = ((gear_el.hi.x - gear_el.lo.x) / upf * 12.0, (gear_el.hi.y - gear_el.lo.y) / upf * 12.0);
                let m = 2.0 * rl.gear.window_margin_in;
                [dx.max(dy) - m, dx.min(dy) - m]
            })
            .filter(|w| w[0] > 0.0 && w[1] > 0.0);
        let window = given.or(derived);
        let area_text = eq.and_then(|e| e.properties.get(if from_below { "BOTTOM CONDUIT AREA" } else { "TOP CONDUIT AREA" })).cloned();
        let bottom_fed = !from_below
            && eq.and_then(|e| e.properties.get("INCOMING LOCATION")).is_some_and(|v| v.trim().to_ascii_lowercase().starts_with("bottom"));
        let od = rl.entry.locknut_od_in.get(&format!("{size_in:?}")).copied().unwrap_or(size_in + 1.0);
        let pitch = (od + rl.entry.spacing_in) / 12.0 * upf;
        let land = match window {
            Some([w, dpt]) => {
                // Slots row by row across the window (W along the box's long side, D across it).
                let (w_u, d_u) = (w / 12.0 * upf, dpt / 12.0 * upf);
                let taken = slots_used.entry(lead_h.gear.unwrap_or(usize::MAX)).or_insert_with(|| (0..entries.len()).collect());
                if given.is_none() {
                    entry_unknown.insert(format!("{} on {}", gear_el.tag, gear_el.level));
                }
                let per_row = (w_u / pitch).floor().clamp(1.0, 64.0);
                let rows = (d_u / pitch).floor().clamp(1.0, 64.0);
                let cap = (per_row * rows) as usize;
                let wide = (gear_el.hi.x - gear_el.lo.x) >= (gear_el.hi.y - gear_el.lo.y);
                let pos = |i: usize| {
                    let (col, row) = ((i as f64) % per_row, ((i as f64) / per_row).floor().min(rows - 1.0));
                    let along = (-w_u / 2.0 + pitch * (col + 0.5)).min(w_u / 2.0);
                    let across = (-d_u / 2.0 + pitch * (row + 0.5)).min(d_u / 2.0);
                    if wide { Vec2::new(gctr.x + along, gctr.y + across) } else { Vec2::new(gctr.x + across, gctr.y + along) }
                };
                // The first free slot whose raceway reaches the panel clear of what is around it
                // on the panel's level (a pipe over the panel takes the slots under it), else the
                // first free one.
                let last = home_z.last().copied().unwrap_or(z0);
                let (lo_z, hi_z) =
                    if riser { if from_below { (gfloor, gear_z) } else { (gear_z, gtop) } } else { (gear_z.min(last), gear_z.max(last)) };
                let clear = |p: Vec2| {
                    let q = carry(p, &lead_h.level, &gear_el.level);
                    gfield.hit_unworked(q, q, lo_z, hi_z, r_of(size_in), upf, &skip, rl).is_none()
                };
                let free: Vec<usize> = (0..cap).filter(|i| !taken.contains(i)).collect();
                let pick = free.iter().copied().find(|i| clear(pos(*i))).or_else(|| free.first().copied());
                let i = match pick {
                    Some(i) => i,
                    None => {
                        if conflicts.len() < MAX_ITEMS {
                            conflicts.push(json!({
                                "kind": "conflict", "gear": gear_el.tag, "level": gear_el.level, "side": side,
                                "window_in": [w, dpt], "window": area_text.clone().unwrap_or_default(),
                                "raceways": taken.len() + 1, "holds": cap, "pitch_in": r2(pitch / upf * 12.0),
                                "fix": format!("the {side} entry window can't hold another raceway: add an extension gutter or wireway, a pull box {} the panel, a larger can, or split the entry", if from_below { "below" } else { "above" }),
                            }));
                        }
                        cap.saturating_add(taken.len())
                    }
                };
                taken.insert(i);
                pos(i.min(cap.saturating_sub(1)))
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
        // The vertical pieces: the changes of tier along it, then the last piece down (or up)
        // onto the panel, through the floors between when it is on another storey.
        let last_z = home_z.last().copied().unwrap_or(z0);
        let seat_leg = Leg { pts: home_pts.clone(), z: home_z.clone(), ..Leg::default() };
        let mut cols = Vec::new();
        leg_columns(&seat_leg, z0, r_of(size_in), field, upf, &skip, rl, &mut cols);
        // (its end column is the panel's drop, checked next, against the panel's own skip list)
        cols.retain(|b| Vec2::new(b.at.x, b.at.y).dist(land) > upf / 96.0);
        vertical.extend(cols);
        let what_end = if riser { "riser" } else { "vertical conduit" };
        for bl in &vertical {
            drop_clashes += 1;
            add_clash(
                &mut clashes,
                field,
                bl,
                if bl.at.z <= z0 + 1e-9 && Vec2::new(bl.at.x, bl.at.y).dist(qa) <= upf / 96.0 { "drop" } else { "vertical conduit" },
                &labels.join(","),
                &lead_h.level,
                size_in,
            );
        }
        // Its own level's part of the last piece, then (a riser) the panel level's.
        let (zlo_here, zhi_here) = if riser {
            if from_below { (last_z, floor + sp.level(&lead_h.level).map_or(12.0, |l| l.height_ft) * upf) } else { (floor, last_z) }
        } else {
            (gear_z.min(last_z), gear_z.max(last_z))
        };
        if let Some(i) = field.hit_unworked(land, land, zlo_here, zhi_here, r_of(size_in), upf, &skip, rl) {
            drop_clashes += 1;
            add_clash(
                &mut clashes,
                field,
                &Blocked { obstacle: i, at: Vec3::new(land.x, land.y, zhi_here), z: zhi_here, why: "" },
                what_end,
                &labels.join(","),
                &lead_h.level,
                size_in,
            );
        }
        let mut floors: Vec<Value> = Vec::new();
        if riser {
            let gland = carry(land, &lead_h.level, &gear_el.level);
            let (glo, ghi) = if from_below { (gfloor, gear_z) } else { (gear_z, gtop) };
            if let Some(i) = gfield.hit_unworked(gland, gland, glo, ghi, r_of(size_in), upf, &skip, rl) {
                drop_clashes += 1;
                add_clash(
                    &mut clashes,
                    gfield,
                    &Blocked { obstacle: i, at: Vec3::new(gland.x, gland.y, ghi), z: ghi, why: "" },
                    "riser",
                    &labels.join(","),
                    &gear_el.level,
                    size_in,
                );
            }
            // Each floor it passes: a penetration with its firestop, rated by the floor/ceiling
            // assembly under that floor where the documents give one.
            let (zl, zh) = ((last_z / upf).min(gear_z / upf), (last_z / upf).max(gear_z / upf));
            let mut levels: Vec<_> = sp.levels.iter().collect();
            levels.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
            for (li, l) in levels.iter().enumerate() {
                if !(l.elevation_ft > zl && l.elevation_ft <= zh) || penetrations.len() >= MAX_ITEMS {
                    continue;
                }
                let below = li.checked_sub(1).and_then(|j| levels.get(j));
                let rating = below.and_then(|b| {
                    let p = carry(land, &lead_h.level, &b.name);
                    sp.rcp
                        .ceilings
                        .iter()
                        .filter(|ca| same_level(&ca.level, &b.name) && ca.outline.len() >= 3)
                        .find(|ca| inside(p, &ca.outline.iter().map(|q| Vec2::new(q[0], q[1])).collect::<Vec<_>>()))
                        .and_then(|ca| ca.ceiling_type.as_ref())
                        .and_then(|t| sp.ceiling_types.iter().find(|ct| ct.name.eq_ignore_ascii_case(t)))
                        .and_then(|ct| ct.fire_rating.clone())
                        .filter(|r| !r.trim().is_empty())
                });
                let fl = json!({
                    "floor": l.name, "rating": rating.clone().unwrap_or_else(|| "not given".into()), "level": lead_h.level,
                    "circuit": labels.join(","), "raceway_in": size_in,
                    "at": [r2(land.x), r2(land.y), r2((l.elevation_ft * upf - floor) / upf)],
                    "firestop": match &rating {
                        Some(r) => format!("{} for {} raceway through the {} floor ({r})", rl.firestop.item, super::mepatoms::trade_size(size_in), l.name),
                        None => format!("{} for {} raceway through the {} floor: rating {}", rl.firestop.item, super::mepatoms::trade_size(size_in), l.name, rl.firestop.floor_unrated),
                    },
                    "source": rl.firestop.source,
                });
                floors.push(json!(l.name));
                penetrations.push(fl);
            }
            if risers.len() < MAX_ITEMS {
                risers.push(json!({
                    "panel": gear_el.tag, "panel_level": gear_el.level, "level": lead_h.level, "circuits": labels, "side": side,
                    "floors": floors, "at": [r2(land.x), r2(land.y)],
                }));
            }
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
        bot.push((gear_z - floor) / upf);
        // A jog in the wall from the box to its rise point.
        if qa.dist(lead_dev.at) > upf / 96.0 {
            pts.insert(1, [lead_dev.at.x, lead_dev.at.y]);
            bot.insert(1, (ja - floor) / upf);
        }
        r.points = pts;
        r.bottoms_ft = bot;
        r.id = format!("{gid}#home");
        r.size_source = ssrc.clone();
        r.height_source = ROUTED.into();
        // The panel end's makeup: down its can to the breaker (Q6206).
        let pm = gap::panel_end_makeup_ft(Some((gear_el.hi.z - gear_el.lo.z) / upf));
        let l = lens.entry(lead_h.circ).or_default();
        let ft = run_ft(&r, upf);
        l.home += ft;
        l.wire += (ft + box_makeup + pm) * wires_in(&combined);
        for (mc, tap, mparts) in &member_ways {
            let l = lens.entry(*mc).or_default();
            let way = tap + ft;
            l.own_home += way;
            l.own_wire += (way + box_makeup + pm) * wires_in(mparts);
            l.cable += way + box_makeup + pm;
            *cable_sizes.entry(*mc).or_default().entry(cable_size(&makeup_text(mparts))).or_default() += way + box_makeup + pm;
        }
        if home_segs.len() < MAX_RUNS {
            home_segs.push(gap::HomeSeg {
                run: runs.len(),
                key: format!("{}|{}", lead_h.level.to_ascii_lowercase(), gear_el.handle.hex()),
                pts: r.points.clone(),
                bot: r.bottoms_ft.clone(),
                parts: combined.clone(),
                ccc: combined.iter().filter(|p| !p.2).fold(0u32, |n, p| n.saturating_add(p.0)),
                trip_a: trips.iter().copied().fold(0.0, f64::max),
                labels: labels.clone(),
            });
        }
        penetrate(&mut penetrations, &rated, &lead_h.level, &seat_leg, &labels.join(","), size_in, upf, floor, rl);
        for bl in bore(&mut bored, field, &seat_leg, &labels.join(","), &lead_h.level, size_in, upf, rl) {
            add_clash(&mut clashes, field, &bl, "homerun", &labels.join(","), &lead_h.level, size_in);
        }
        runs.push(r);
        box_in
            .entry(lead_c.devs.get(lead_h.root).copied().unwrap_or(usize::MAX))
            .or_default()
            .extend(combined.iter().filter(|p| !p.2).map(|p| (p.0, p.1.clone(), None)));
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
                if window.is_some() {
                    format!("in its {side} entry window")
                } else {
                    format!("entry window unknown: centred on its {side}, derived (centered)")
                }
            ),
            "computed",
        );
        t.set("max_circuits", format!("{max_group}: {group_src}"), if methods.max_circuits.is_some() { "spec" } else { "rule" });
        t.set("homerun_ft", format!("{longest:.1}"), "computed");
        if riser {
            t.set(
                "riser",
                format!(
                    "to panel {} on {} through the {} floor(s), into its {side}",
                    gear_el.tag,
                    gear_el.level,
                    floors.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
                ),
                "computed",
            );
        }
        tags.push(t);
    }

    // The switch loops' tags after the circuits' conductors are sized: the loop's count at the
    // circuit's hot size.
    for st in &mut loop_tags {
        let of = field_of(st, "switch_loop_of").unwrap_or_default().to_string();
        let hot = tags
            .iter()
            .find(|t| t.id == of)
            .and_then(|t| field_of(t, "conductors"))
            .and_then(|m| super::mepatoms::conductor_parts(m).into_iter().find(|p| !p.2))
            .map(|p| p.1);
        let ways = st.id.rsplit("~sw").next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(3);
        if let Some(h) = hot {
            st.set("conductors", format!("{ways}#{h} + 1#{h}G"), "rule");
        }
    }
    tags.extend(loop_tags);
    // Each leg's and tap's raceway by the fill of what it carries (Q6206, NEC Chapter 9): its
    // circuit's conductors as sized (a circuit upsized for its voltage drop is that size in its
    // legs too), a switch loop's; never under the minimum. A homerun is sized with its group's.
    let mut legs_fill_sized = 0usize;
    {
        let of: HashMap<&str, (&str, &str)> =
            tags.iter().filter_map(|t| Some((t.id.as_str(), (field_of(t, "conductors")?, field_of(t, "conduit").unwrap_or_default())))).collect();
        for r in runs.iter_mut().filter(|r| !r.id.contains(HOME_PART)).take(MAX_RUNS) {
            let Some((mk, method)) = super::printcond::circuit_of_run(r).and_then(|c| of.get(c)).copied() else { continue };
            if super::mepatoms::rules().raceway_named(method).is_none() {
                continue;
            }
            let parts = super::mepatoms::conductor_parts(mk);
            if let Some((t, area, allowed)) = fill_size(&parts, method, min_in, rl)
                && t > r.size_in[0] + 1e-9
            {
                r.size_in[0] = t;
                r.size_source = format!("computed: {mk}: {} sq in of conductors in {} sq in allowed ({})", r3(area), r3(allowed), rl.fill.source);
                legs_fill_sized += 1;
            }
        }
    }
    // -- Homeruns of one panel on one path (Q6206): each stretch one raceway carrying all their
    // circuits (up to the rules' conductors per raceway), sized by fill and derated. --
    let shared = gap::shared_runs(&home_segs, upf);
    let mut shared_rows: Vec<Value> = Vec::new();
    let mut shared_runs_out: Vec<MepRun> = Vec::new();
    let mut shared_tags: Vec<ElementTag> = Vec::new();
    let mut cuts_of: BTreeMap<usize, Vec<([f64; 2], [f64; 2], f64)>> = BTreeMap::new();
    let mut shared_before: BTreeMap<String, f64> = BTreeMap::new();
    let mut shared_after: BTreeMap<String, f64> = BTreeMap::new();
    let order_of = |s: &str| rl.vdrop.sizes.iter().position(|x| x == s).unwrap_or(0);
    for st in &shared {
        let ft = Vec2::new(st.a[0], st.a[1]).dist(Vec2::new(st.b[0], st.b[1])) / upf;
        let mut parts: Vec<Part> = Vec::new();
        let mut labels: Vec<String> = Vec::new();
        let mut trip = 0.0_f64;
        for m in st.members.iter().filter_map(|i| home_segs.get(*i)) {
            parts.extend(m.parts.iter().filter(|p| !p.2).cloned());
            labels.extend(m.labels.iter().cloned());
            trip = trip.max(m.trip_a);
            let size = runs.get(m.run).map_or(min_in, |r| r.size_in[0]);
            *shared_before.entry(super::mepatoms::trade_size(size)).or_default() += ft;
        }
        let ccc: u32 = parts.iter().fold(0u32, |n, p| n.saturating_add(p.0));
        let ground = parts.iter().map(|p| p.1.clone()).max_by_key(|s| order_of(s)).unwrap_or_else(|| "12".into());
        // The 310.15(C)(1) adjustment over all of them: conductors up a size where it needs.
        let mut up = parts.clone();
        let mut steps = 0;
        while up.iter().any(|p| allowed_a(&p.1, ccc, rl) + 1e-9 < trip.max(1.0)) && steps < 3 {
            for p in &mut up {
                if allowed_a(&p.1, ccc, rl) + 1e-9 < trip.max(1.0) {
                    p.1 = larger(&p.1, rl);
                }
            }
            steps += 1;
        }
        let mut all = up.clone();
        all.push((1, ground, true));
        let mut combined: Vec<Part> = Vec::new();
        for p in all {
            match combined.iter_mut().find(|q| q.1 == p.1 && q.2 == p.2) {
                Some(q) => q.0 += p.0,
                None => combined.push(p),
            }
        }
        let size = fill_size(&combined, rl.methods.raceway.as_str(), min_in, rl).map_or(4.0, |f| f.0);
        *shared_after.entry(super::mepatoms::trade_size(size)).or_default() += ft;
        // The stretch as one raceway (its own run and tag, carrying every circuit on it); the
        // homeruns' own runs leave it (cut out of them below).
        let Some(m0) = st.members.first().and_then(|i| home_segs.get(*i)) else { continue };
        let Some(lead) = runs.get(m0.run).cloned() else { continue };
        let level = lead.level.clone();
        let gid = format!(
            "{}{}{HOME_PART}{}:shared-{}",
            super::printcond::CIRCUIT_TAG,
            level.replace(['/', '#', '@'], "-"),
            st.key.rsplit('|').next().unwrap_or_default(),
            shared_runs_out.len()
        );
        let mut r = super::printcond::run(BRANCH_SYSTEM, &level, "", vec![st.a, st.b], size, 0.0);
        r.bottoms_ft = vec![st.z_ft];
        r.id = format!("{gid}#shared");
        r.size_source = format!("computed: {} carrying {} homeruns ({})", makeup_text(&combined), st.members.len(), gap::rules().shared.source);
        r.height_source = ROUTED.into();
        let lead_tag = tags.iter().find(|t| super::printcond::circuit_of_run(&lead) == Some(t.id.as_str()));
        let mut t = ElementTag { id: gid, kind: "circuit".into(), level: level.clone(), ..ElementTag::default() };
        t.at_ft = [st.a[0] / upf, st.a[1] / upf];
        let numbers: Vec<String> = labels.iter().filter_map(|l| super::printcond::parse_circuit(l).map(|(_, n)| n)).collect();
        let panel_name = lead_tag.and_then(|x| field_of(x, "panel")).unwrap_or_default().to_string();
        t.set("circuit", format!("{panel_name}-{}", numbers.join(",")), "computed");
        t.set("circuits", labels.join(", "), "computed");
        t.set("panel", panel_name, "plan");
        t.set("conductors", makeup_text(&combined), "computed");
        t.set("conduit", lead_tag.and_then(|x| field_of(x, "conduit")).unwrap_or(rl.methods.raceway.as_str()).to_string(), "computed");
        t.set("route", ROUTED, "computed");
        t.set("homerun", format!("shared by {} homeruns to the panel", st.members.len()), "computed");
        t.set("shared_raceway", gap::rules().shared.source.clone(), "rule");
        if gap::rules().shared.merge {
            for m in st.members.iter().filter_map(|i| home_segs.get(*i)) {
                cuts_of.entry(m.run).or_default().push((st.a, st.b, st.z_ft));
            }
            shared_runs_out.push(r);
            shared_tags.push(t);
        }
        if shared_rows.len() < MAX_ITEMS {
            shared_rows.push(json!({
                "panel_level": st.key.split('|').next().unwrap_or_default(), "homeruns": st.members.len(), "circuits": labels,
                "ft": r1(ft), "at": [[r2(st.a[0]), r2(st.a[1])], [r2(st.b[0]), r2(st.b[1])]], "height_ft": r2(st.z_ft),
                "conductors": makeup_text(&combined), "ccc": ccc, "factor": adjustment(ccc, rl), "upsized": steps > 0,
                "size_in": size, "size": super::mepatoms::trade_size(size),
            }));
        }
    }
    // Each homerun's run with its shared stretches cut out: its pieces (a stretch at its end
    // leaves one; one in its middle two).
    let mut cut_runs = 0usize;
    let (mut cut_expected_ft, mut cut_removed_ft) = (0.0_f64, 0.0_f64);
    if !cuts_of.is_empty() {
        let tol = gap::rules().shared.same_path_in.max(0.0) / 12.0 * upf + 1e-6;
        let mut kept: Vec<MepRun> = Vec::with_capacity(runs.len() + shared_runs_out.len());
        for (i, r) in runs.drain(..).enumerate() {
            let Some(cuts) = cuts_of.get(&i) else {
                kept.push(r);
                continue;
            };
            cut_runs += 1;
            let whole = run_ft(&r, upf);
            let mut left = 0.0;
            cut_expected_ft += cuts.iter().map(|(a, b, _)| Vec2::new(a[0], a[1]).dist(Vec2::new(b[0], b[1])) / upf).sum::<f64>();
            for (k, (pts, bot)) in gap::cut_run(&r.points, &r.bottoms_ft, cuts, tol, tol / upf).into_iter().enumerate() {
                let mut piece = r.clone();
                piece.points = pts;
                piece.bottoms_ft = bot;
                if k > 0 {
                    piece.id = format!("{}~{k}", r.id);
                }
                left += run_ft(&piece, upf);
                kept.push(piece);
            }
            cut_removed_ft += whole - left;
        }
        runs = kept;
    }
    runs.extend(shared_runs_out);
    tags.extend(shared_tags);
    let shared_report = json!({
        "merged": gap::rules().shared.merge,
        "merge_source": gap::rules().shared.merge_source,
        "stretches": shared.len(),
        "homeruns_cut": cut_runs,
        "cut_ft": {"stretches": r1(cut_expected_ft), "removed": r1(cut_removed_ft)},
        "parallel_ft_by_size": shared_before.iter().map(|(k, v)| (k.clone(), json!(r1(*v)))).collect::<serde_json::Map<_, _>>(),
        "shared_ft_by_size": shared_after.iter().map(|(k, v)| (k.clone(), json!(r1(*v)))).collect::<serde_json::Map<_, _>>(),
        "items": shared_rows,
        "note": "parallel_ft_by_size: the homeruns' own raceways along the stretches; shared_ft_by_size: each stretch as its one raceway (fill and derating of all its circuits). Merged: routed_ft counts a stretch once; the lengths' home_ft counts each homerun's own way to the panel",
        "source": gap::rules().shared.source,
    });

    // -- A power device the prints give no circuit (Q6206): proposed on the nearest panel of its
    // level with a spare, not drawn, priced apart, a review item and an RFI line. --
    let mut proposed: Vec<Value> = Vec::new();
    let mut proposed_by: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    {
        let mut next_spare: HashMap<String, (Vec<String>, usize)> = HashMap::new();
        for di in &uncircuited {
            let Some(dv) = devs.get(*di) else { continue };
            if !gap::is_power_device(&dv.kind) || proposed.len() >= MAX_ITEMS {
                continue;
            }
            let near_gear = gear
                .iter()
                .filter(|g| same_level(&g.level, &dv.level) && electrical(g))
                .min_by(|a, b| manhattan(gear_ctr(a), dv.at).total_cmp(&manhattan(gear_ctr(b), dv.at)));
            let Some(g) = near_gear else { continue };
            // A unit's copy (`P-1A`) has its typical panel's schedule (`P`).
            let sched_of = |t: &str| {
                let (sp_, sc) = gap::spare_circuits(sp, t);
                if sp_.is_empty() && sc.is_empty() {
                    t.split_once('-').map(|(p, _)| gap::spare_circuits(sp, p)).unwrap_or((sp_, sc))
                } else {
                    (sp_, sc)
                }
            };
            let e = next_spare.entry(g.tag.clone()).or_insert_with(|| {
                let (a, b) = sched_of(&g.tag);
                (a.into_iter().chain(b).collect(), 0)
            });
            let spare = e.0.get(e.1).cloned();
            e.1 += 1;
            let (floor, tiers) = tiers_of(&dv.level, None);
            let up = (tiers.first().copied().unwrap_or(dv.top) - dv.top).abs() / upf;
            let down = (tiers.first().copied().unwrap_or(g.hi.z) - g.hi.z).abs() / upf;
            let _ = floor;
            let est = manhattan(gear_ctr(g), dv.at) / upf + up + down;
            *proposed_by.entry(dv.level.clone()).or_default().entry(dv.kind.clone()).or_default() += 1;
            proposed.push(gap::proposed_row(&dv.handle.hex(), &dv.kind, &dv.level, dv.room.trim(), &g.tag, spare.as_deref(), est, dv.at));
        }
    }

    // -- The gear's dedicated space (NEC 110.26(E)(1)): its footprint from its top 6 ft up (or to
    // the structure) is the electrical installation's; another trade's duct or pipe there is a
    // clash item. --
    let mut gear_space = 0usize;
    for g in gear.iter().filter(|g| electrical(g)).take(MAX_DEVICES) {
        let field = fields.get(&g.level.to_ascii_lowercase()).unwrap_or(&empty);
        let top = g.hi.z + rl.gear.dedicated_above_ft * upf;
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
    let final_hot: HashMap<&str, String> = tags
        .iter()
        .filter_map(|t| Some((t.id.as_str(), super::mepatoms::conductor_parts(field_of(t, "conductors")?).into_iter().find(|p| !p.2)?.1)))
        .collect();
    let order_sz = |s: &str| rl.vdrop.sizes.iter().position(|x| x == s).unwrap_or(0);
    let box_conductors: Vec<Value> = box_in
        .iter()
        .filter_map(|(di, conds)| {
            let dv = devs.get(*di)?;
            let sized: Vec<Value> = conds
                .iter()
                .map(|(n, sz, of)| {
                    let up = of.as_deref().and_then(|c| final_hot.get(c)).filter(|f| order_sz(f) > order_sz(sz)).cloned();
                    json!([n, up.unwrap_or_else(|| sz.clone())])
                })
                .collect();
            Some(json!({"device": dv.handle.hex(), "kind": dv.kind, "level": dv.level, "circuit": dv.circuit, "yoke": dv.yoke, "conductors": sized}))
        })
        .take(MAX_DEVICES)
        .collect();

    // -- The holes through the framing: each member's holes apart by the rule (edge to edge, in
    // the web's plane); the closer ones are review items. --
    let mut per_member: BTreeMap<(String, usize), Vec<usize>> = BTreeMap::new();
    for (i, h) in bored.iter().enumerate().filter(|(_, h)| h.dia_in > 0.0) {
        per_member.entry((h.level.to_ascii_lowercase(), h.obstacle)).or_default().push(i);
    }
    let mut too_close = 0usize;
    for ids in per_member.values_mut() {
        ids.sort_by(|a, b| {
            let k = |i: &usize| bored.get(*i).map_or((0.0, 0.0), |h| (h.along, h.z));
            k(a).partial_cmp(&k(b)).unwrap_or(std::cmp::Ordering::Equal)
        });
        for w in ids.windows(2) {
            let (Some(p), Some(q)) = (w.first().and_then(|i| bored.get(*i)), w.get(1).and_then(|i| bored.get(*i))) else { continue };
            let apart_in = (q.along - p.along).hypot(q.z - p.z) / upf * 12.0 - (p.dia_in + q.dia_in) / 2.0;
            let need_in = rl.framing.apart_x.max(0.0) * p.dia_in.max(q.dia_in);
            if apart_in + 1e-6 >= need_in {
                continue;
            }
            too_close += 1;
            if clashes.len() < MAX_ITEMS {
                let floor = sp.level(&p.level).map_or(0.0, |l| l.elevation_ft * upf);
                clashes.push(json!({
                    "kind": "clash",
                    "a": {"what": "bored hole", "circuit": p.circuit, "trade": "electrical", "system": BRANCH_SYSTEM},
                    "b": {"handle": p.host.hex(), "category": FRAMING, "system": p.member, "trade": "framing", "circuit": q.circuit},
                    "pair": "electrical × framing",
                    "level": p.level,
                    "at": [r2(q.at.x), r2(q.at.y), r2((q.z - floor) / upf)],
                    "clearance_in": r2(need_in),
                    "fix": format!(
                        "two holes in one {} are {:.1} in apart edge to edge, the hole rule asks {:.1} in ({}× the larger): bore them farther apart, run both raceways through one hole sized for them, or cross in the next bay",
                        p.member, apart_in.max(0.0), need_in, rl.framing.apart_x
                    ),
                }));
            }
        }
    }
    let mut holes_by_level: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for h in &bored {
        let k = if h.dia_in > 0.0 { format!("{} hole", super::mepatoms::trade_size(h.dia_in)) } else { "through truss web".to_string() };
        *holes_by_level.entry(h.level.clone()).or_default().entry(k).or_default() += 1;
    }
    let framed_levels: Vec<String> = fields.iter().filter(|(_, f)| f.framed()).map(|(k, _)| k.clone()).collect();
    let framing_report = json!({
        "levels_framed": framed_levels,
        "members": members.len(),
        "members_source": member_source,
        "bored_holes": bored.iter().filter(|h| h.dia_in > 0.0).count(),
        "truss_passes": bored.iter().filter(|h| h.dia_in <= 0.0).count(),
        "by_level": holes_by_level,
        "holes_too_close": too_close,
        "blocked_pieces": clashes.iter().filter(|c| c["b"]["category"] == json!(FRAMING) && c["a"]["what"] != json!("bored hole")).count(),
        "rules": {"flange_in": rl.framing.flange_in, "edge_in": rl.framing.edge_in, "hole_over_in": rl.framing.hole_over_in, "min_end_ft": rl.framing.min_end_ft, "apart_x": rl.framing.apart_x},
        "source": rl.framing.source,
        "note": if members.is_empty() { "the documents read give no floor framing or roof trusses: the router keeps to the ceiling-space tiers only" } else { "raceways keep to the joist bays along the members and bore the web only in its hole zone; each hole is listed (bored_holes) and taken off" },
    });
    let bored_list: Vec<Value> = bored
        .iter()
        .take(MAX_ITEMS)
        .map(|h| {
            let floor = sp.level(&h.level).map_or(0.0, |l| l.elevation_ft * upf);
            json!({
                "level": h.level, "circuit": h.circuit, "member": h.member, "floor": h.host.hex(),
                "hole_in": r3(h.dia_in), "raceway_in": h.raceway_in, "end_ft": r2(h.end_ft),
                "at": [r2(h.at.x), r2(h.at.y), r2((h.z - floor) / upf)],
                "item": if h.dia_in > 0.0 { rl.framing.bored_item.clone() } else { rl.framing.truss_item.clone() },
            })
        })
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
    // The lengths by room (each circuit's: the room most of its devices are in) and level.
    // Each circuit's legs' cable by its size now its conductors are sized (a circuit upsized for
    // its voltage drop is that size in its legs too; a switch loop the loop's count of it).
    for (ci, legs) in &cable_legs {
        let Some(c) = circs.get(*ci) else { continue };
        let id = {
            let base = tag_at(c.tag).map(|t| t.id.clone()).unwrap_or_default();
            if c.part.is_empty() { base } else { format!("{base}@{}", c.part) }
        };
        let mk = tags.iter().find(|t| t.id == id).and_then(|t| field_of(t, "conductors")).unwrap_or(super::printcond::DEFAULT_CONDUCTORS).to_string();
        let hot = super::mepatoms::conductor_parts(&mk).into_iter().find(|p| !p.2).map_or_else(|| "12".to_string(), |p| p.1);
        for (n, ft) in legs {
            let size = if *n == 0 { cable_size(&mk) } else { cable_size(&format!("{n}#{hot} + 1#{hot}G")) };
            *cable_sizes.entry(*ci).or_default().entry(size).or_default() += ft;
        }
    }
    let mut rows: Vec<(String, String, Lens, BTreeMap<String, f64>)> = Vec::new();
    let mut circuit_rows: Vec<Value> = Vec::new();
    let c_id = |c: &Circ| {
        let base = tag_at(c.tag).map(|t| t.id.clone()).unwrap_or_default();
        if c.part.is_empty() { base } else { format!("{base}@{}", c.part) }
    };
    let mut keys: Vec<usize> = lens.keys().copied().collect();
    keys.sort_unstable();
    for ci in keys {
        let (Some(c), Some(l)) = (circs.get(ci), lens.get(&ci)) else { continue };
        let mut n: BTreeMap<&str, usize> = BTreeMap::new();
        for r in c.devs.iter().filter_map(|i| devs.get(*i)).map(|d| d.room.trim()).filter(|r| !r.is_empty()) {
            *n.entry(r).or_default() += 1;
        }
        let room = n.iter().max_by_key(|(_, k)| **k).map(|(r, _)| (*r).to_string()).unwrap_or_default();
        if circuit_rows.len() < MAX_CIRCUITS {
            let t = tags.iter().find(|t| t.id == c_id(c)).or_else(|| tag_at(c.tag));
            let mk = t.and_then(|t| field_of(t, "conductors")).unwrap_or_default().to_string();
            circuit_rows.push(json!({
                "circuit": c.label, "id": c_id(c), "level": c.level, "room": room,
                "method": t.and_then(|t| field_of(t, "conduit")).unwrap_or_default(),
                "raceway_ft": r1(l.in_area + l.own_home), "conductor_ft": r1(l.own_wire), "conductors": mk,
                "cable_ft": r1(l.cable), "cable_ft_by_size": cable_sizes.get(&ci).map(|m| m.iter().map(|(k, v)| (k.clone(), json!(r1(*v)))).collect::<serde_json::Map<_, _>>()).unwrap_or_default(),
            }));
        }
        rows.push((c.level.clone(), room, *l, cable_sizes.get(&ci).cloned().unwrap_or_default()));
    }
    let lengths = lengths_report(sp, &rows);
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
        "risers": risers,
        "drops_moved_along_wall": drops_moved,
        "vertical_clashes": drop_clashes,
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
        "framing": framing_report,
        "bored_holes": bored_list,
        "box_fill": {"checked": 0, "over": [], "pending": box_conductors},
        "conductors_upsized": upsized,
        "fill": fill_rows,
        "derated": derated,
        "unrouted": unrouted,
        "feeder_circuits": feeder_circuits,
        "fed_as_feeders": fed_as_feeders,
        "clearance_source": rl.clearance_source(),
        "route_source": rl.route.source,
    });
    // What Q6206 adds: the labels resolved, the switch loops, the shared raceway, the proposed
    // circuits, each circuit's own lengths.
    // The devices on the routed circuits, by kind (what became routed).
    let mut routed_kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for h in &homes {
        for d in circs.get(h.circ).map(|c| c.devs.as_slice()).unwrap_or_default().iter().filter_map(|i| devs.get(*i)) {
            *routed_kinds.entry(d.kind.as_str()).or_default() += 1;
        }
    }
    let gap_more = json!({
        "routed_devices_by_kind": routed_kinds,
        "devices_without_tag_before": no_tag_before,
        "tags_resolved": {"devices": resolved_devices, "circuits": resolved_rows.len(), "items": resolved_rows, "source": gap::rules().tag_reading.source},
        "tags_unresolved": unresolved_rows,
        "switches_unjoined": switches_unjoined,
        "switch_loop_legs": switch_loop_legs,
        "legs_fill_sized": legs_fill_sized,
        "switch_loop_source": gap::rules().switch_loop.source,
        "shared_raceway": shared_report,
        "proposed_circuits": {"total": proposed.len(), "by_level_kind": proposed_by, "items": proposed, "source": gap::rules().proposed.source},
        "circuit_lengths": circuit_rows,
    });
    if let (Some(o), Value::Object(m)) = (out.report.as_object_mut(), gap_more) {
        o.extend(m);
    }
    // What Q6200 adds: the switch legs, the cable's paths, the makeup, the lengths by room.
    let more = json!({
        "switch_legs": switch_legs,
        "switch_legs_source": rl.switch_legs.source,
        "cable_stud_legs": stud_legs,
        "cable_paths": cable_paths.len(),
        "cable_source": rl.cable.source,
        "makeup": {"box_ft": box_makeup, "box_min_in": ar.makeup_min_in, "box_allowance_in": ar.makeup_allowance_in, "panel_ft": panel_makeup, "panel_height_share": ar.panel_makeup_height_share, "source": format!("{}; {}", ar.makeup_source, ar.panel_makeup_source)},
        "lengths": lengths,
    });
    if let (Some(o), Value::Object(m)) = (out.report.as_object_mut(), more) {
        o.extend(m);
    }
    out.cable = cable_paths;
    out.runs = runs;
    out.tags = tags;
    out
}

/// An MEP run's piece (a stick, fitting, box, cable, wire): not a building trade's (studs,
/// sheets, joists are atoms too, and a reroute keeps them).
pub(crate) fn mep_piece(e: &Entity) -> bool {
    super::mepatoms::is_atom(e) && !super::tradeatoms::is_trade_piece(e)
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
/// A hole a routed raceway bores through a framing member (or its pass through a truss's web).
#[derive(Clone, Debug)]
struct Bored {
    level: String,
    circuit: String,
    obstacle: usize,
    member: String,
    host: Handle,
    at: Vec2,
    z: f64,
    along: f64,
    dia_in: f64,
    raceway_in: f64,
    end_ft: f64,
}

/// The holes a seated leg bores through the framing over its level, into `out`; the pieces the
/// framing stops at the leg's own heights (a homerun seated at a smaller raceway's size), as
/// blocked pieces.
#[allow(clippy::too_many_arguments)]
fn bore(out: &mut Vec<Bored>, field: &Field, leg: &Leg, circuit: &str, level: &str, size_in: f64, upf: f64, rl: &RouteRules) -> Vec<Blocked> {
    let mut stopped = Vec::new();
    if !field.framed() {
        return stopped;
    }
    let r = size_in / 24.0 * upf;
    for (i, w) in leg.pts.windows(2).enumerate() {
        let [a, b] = w else { continue };
        let z = leg.z.get(i).copied().unwrap_or(0.0);
        match field.framing(*a, *b, z, r, upf, rl) {
            Ok(holes) => {
                for h in holes {
                    if out.len() >= MAX_BORED {
                        return stopped;
                    }
                    let Some(o) = field.obs.get(h.obstacle) else { continue };
                    out.push(Bored {
                        level: level.to_string(),
                        circuit: circuit.to_string(),
                        obstacle: h.obstacle,
                        member: o.system.clone(),
                        host: o.handle,
                        at: h.at,
                        z: h.z,
                        along: h.along,
                        dia_in: h.dia_in,
                        raceway_in: size_in,
                        end_ft: h.end_ft,
                    });
                }
            }
            Err((obstacle, why)) => {
                let at = (*a + *b) * 0.5;
                stopped.push(Blocked { obstacle, at: Vec3::new(at.x, at.y, z), z, why });
            }
        }
    }
    stopped
}

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
    for e in d.model.iter().filter(|e| mep_piece(e)) {
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

/// What a routing took out of the spec it went into: the circuits' tags as read (the routed ones
/// and their parts replace them) and their estimated runs. The next routing starts from these.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Replaced {
    tags: Vec<ElementTag>,
    runs: Vec<MepRun>,
}

/// Where the stored spec's [`Replaced`] is kept (among the drawing's embedded files).
const BASE_FILE: &str = "route/base.json";

/// The spec with its circuits routed: the estimated runs of the circuits routed go, the routed
/// runs and the routed tags come in (the drawn conduit stays as it is). What went: the tags and
/// estimated runs as read (a routing's own earlier output is not among them).
pub(crate) fn apply(sp: &mut BuildingSpec, routed: &Routed) -> Replaced {
    let mut replaced = Replaced::default();
    if routed.runs.is_empty() {
        return replaced;
    }
    let mut routed_tags: BTreeSet<&str> = routed.tags.iter().map(|t| t.id.as_str()).collect();
    // A split circuit's own tag gives way to its parts'.
    routed_tags.extend(routed.tags.iter().filter_map(|t| t.fields.get("split_of").map(|f| f.value.as_str())));
    // A feeder's circuit tag keeps no estimated branch route: the feeder's routing carries it.
    let gone = |r: &MepRun| {
        let c = super::printcond::circuit_of_run(r);
        r.system == BRANCH_SYSTEM
            || (r.system == super::printcond::CIRCUIT_SYSTEM && c.is_some_and(|c| routed_tags.contains(c) || routed.feeders.contains(c)))
    };
    replaced.runs = sp.mep.runs.iter().filter(|r| r.system != BRANCH_SYSTEM && gone(r)).take(MAX_RUNS).cloned().collect();
    sp.mep.runs.retain(|r| !gone(r));
    sp.mep.runs.extend(routed.runs.iter().cloned());
    let routing_made = |t: &ElementTag| t.id.contains(HOME_PART) || t.fields.contains_key("split_of") || t.fields.contains_key(gap::RESOLVED);
    replaced.tags = sp.tags.iter().filter(|t| routed_tags.contains(t.id.as_str()) && !routing_made(t)).take(MAX_CIRCUITS).cloned().collect();
    // Each circuit's routed tag (or its parts) where its tag was read, so the spec keeps its order
    // (the order a later routing reads the circuits in); the homerun tags after them.
    let mut by_read: HashMap<&str, Vec<&ElementTag>> = HashMap::new();
    for t in routed.tags.iter().filter(|t| !t.id.contains(HOME_PART)) {
        let of = t.fields.get("split_of").map_or(t.id.as_str(), |f| f.value.as_str());
        by_read.entry(of).or_default().push(t);
    }
    let mut placed: BTreeSet<String> = BTreeSet::new();
    let mut tags = Vec::with_capacity(sp.tags.len() + routed.tags.len());
    for t in sp.tags.drain(..) {
        if t.id.contains(HOME_PART) {
            continue;
        }
        if !routed_tags.contains(t.id.as_str()) {
            tags.push(t);
            continue;
        }
        if let Some(rs) = by_read.get(t.id.as_str())
            && placed.insert(t.id.clone())
        {
            tags.extend(rs.iter().map(|r| (*r).clone()));
        }
    }
    tags.extend(
        routed
            .tags
            .iter()
            .filter(|t| t.id.contains(HOME_PART) || !placed.contains(t.fields.get("split_of").map_or(t.id.as_str(), |f| f.value.as_str())))
            .cloned(),
    );
    sp.tags = tags;
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
    replaced
}

/// The spec as read, before the routing stored in it (a reroute stores its routed spec): each
/// routed circuit's tag as read again in its place (its parts and homerun tags gone), the routed
/// runs gone and the estimated ones back. A spec no routing went into is as it is.
pub(crate) fn unrouted(d: &Drawing, sp: &BuildingSpec) -> BuildingSpec {
    let mut out = sp.clone();
    if !sp.mep.runs.iter().any(|r| r.system == BRANCH_SYSTEM) {
        return out;
    }
    let Some(base) = d.embedded.get(BASE_FILE).and_then(|b| serde_json::from_slice::<Replaced>(b).ok()) else { return out };
    let by_id: HashMap<&str, &ElementTag> = base.tags.iter().take(MAX_CIRCUITS).map(|t| (t.id.as_str(), t)).collect();
    let mut placed: BTreeSet<&str> = BTreeSet::new();
    let mut tags = Vec::with_capacity(out.tags.len());
    for t in out.tags.drain(..) {
        if t.id.contains(HOME_PART) {
            continue;
        }
        // A part stands for the circuit it was split from; a routed circuit for itself as read.
        let of = t.fields.get("split_of").map_or(t.id.as_str(), |f| f.value.as_str());
        match by_id.get(of) {
            Some(orig) => {
                if placed.insert(orig.id.as_str()) {
                    tags.push((*orig).clone());
                }
            }
            // A label's circuit the routing resolved is made again from the plans' labels.
            None if t.fields.contains_key("split_of") || t.fields.contains_key(gap::RESOLVED) => {}
            None => tags.push(t),
        }
    }
    tags.extend(base.tags.iter().take(MAX_CIRCUITS).filter(|t| !placed.contains(t.id.as_str())).cloned());
    out.tags = tags;
    out.mep.runs.retain(|r| r.system != BRANCH_SYSTEM);
    // An estimated run already back in the spec (read again since) is not doubled.
    let key = |r: &MepRun| serde_json::to_string(r).unwrap_or_default();
    let mut have: BTreeSet<String> = out.mep.runs.iter().filter(|r| r.system == super::printcond::CIRCUIT_SYSTEM).map(key).collect();
    for r in base.runs.into_iter().take(MAX_RUNS) {
        if have.insert(key(&r)) {
            out.mep.runs.push(r);
        }
    }
    out
}

/// Keep what the routing stored in the spec replaced, for the next routing.
fn store_base(d: &mut Drawing, replaced: &Replaced) {
    if let Ok(b) = serde_json::to_vec(replaced) {
        d.embedded.insert(BASE_FILE.into(), std::sync::Arc::new(b));
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

/// Store each leg's cable path (where a cable runs other than the raceway) among the drawing's
/// embedded files; none: the file goes.
pub(crate) fn store_cable(d: &mut Drawing, routed: &Routed) {
    if routed.cable.is_empty() {
        d.embedded.remove(CABLE_FILE);
        return;
    }
    if let Ok(b) = serde_json::to_vec(&routed.cable) {
        d.embedded.insert(CABLE_FILE.into(), std::sync::Arc::new(b));
    }
}

/// The legs' cable paths the last routing stored, by run id.
pub(crate) fn cable_paths(d: &Drawing) -> HashMap<String, MepRun> {
    d.embedded
        .get(CABLE_FILE)
        .and_then(|b| serde_json::from_slice::<Vec<MepRun>>(b).ok())
        .unwrap_or_default()
        .into_iter()
        .take(MAX_RUNS)
        .filter(|r| !r.id.is_empty())
        .map(|r| (r.id.clone(), r))
        .collect()
}

/// Route the circuits again over the model as it stands now (after the job placed its gear and
/// feeders, the levels stacked): the routed runs into the stored spec (each level's own plan),
/// the pieces cut again where the model's are (the stacking move kept on them), the report stored.
pub(crate) fn reroute(s: &mut Session) -> Result<Value> {
    const C: &str = "conduit.reroute";
    let (sp, stored) = super::bspec::current(s, C)?;
    if !stored {
        // A state, not a parameter error (the menu starts it with none).
        return Ok(json!({"routed": false, "note": "the drawing has no building spec: model.build first"}));
    }
    if !enabled(s.doc()?) {
        return Ok(json!({"routed": false, "note": "routing switched off (model.build {route: true} turns it on)"}));
    }
    let t0 = web_time::Instant::now();
    // From the circuits as read, not from the last routing's parts and sizes.
    let mut sp = unrouted(s.doc()?, &sp);
    let routed = route(s.doc()?, &sp);
    let replaced = apply(&mut sp, &routed);
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
        .filter(|e| mep_piece(e))
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
    // The routing's report first: the takeoff sizes each box by the conductors entering it.
    store_report(d, &routed.report);
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
    store_cable(d, &routed);
    store_base(d, &replaced);
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

/// After `moved` devices were set in their walls again (a merge removed the walls they were in):
/// their boxes, rings and the raceways' drops are pieces built where the devices stood, so the
/// circuits are routed again and the pieces cut again where the devices are now. Nothing to do
/// when none moved or the model has no pieces yet (the build makes them where the devices are).
pub(crate) fn follow_reseated(s: &mut Session, moved: usize) -> Option<Result<Value>> {
    if moved == 0 {
        return None;
    }
    let has_atoms = s.doc().ok()?.model.iter().any(|e| mep_piece(e));
    has_atoms.then(|| reroute(s))
}

// ─── Commands ────────────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("conduit.route", "Conduit Routing Report", run_report)
            .menu(&["Draw", "BIM", "Conduit Routing Report"])
            .params("{section?: \"clashes\"|\"conflicts\"|\"penetrations\"|\"bored_holes\"|\"box_fill\"|\"fill\"|\"derated\"|\"conductors_upsized\"|\"unrouted\"|\"methods\", max?: n (items listed, default 200)} → the last model.build's routing: {circuits, routed_circuits, runs, homeruns, routed_ft, estimated_ft_before, ft_by_method_size, methods, wire_size_table, clashes, clashes_by_pair_level, conflicts, entry_unknown, penetrations, framing (the joists and trusses over each level: holes bored, passes, holes too close, the hole rules), bored_holes, box_fill, conductors_upsized, fill, derated, unrouted}")
            .noundo(),
        CommandSpec::new("conduit.reroute", "Reroute Conduit", run_reroute)
            .menu(&["Draw", "BIM", "Reroute Conduit"])
            .params("{} → {routed, removed, atoms, seconds, circuits, homeruns, routed_ft, clashes, unrouted}: the circuits routed again over the model as it stands (gear and feeders placed since the build), the pieces cut again, the report and the spec's routed runs stored"),
        CommandSpec::new("conduit.takeoff", "Conduit Takeoff", run_takeoff)
            .menu(&["Draw", "BIM", "Conduit Takeoff"])
            .params("{group_by?: [\"level\",\"area\",\"circuit\"] (any of them; default level), file?: path (.csv; the table, each line with its elements' handles), force?: bool (replace an existing file), cost_db?: path (.csv: item, size?, unit, material_unit, labor_unit — labor_unit in DOLLARS per unit), labor_rate?: $/hour (turns labor dollars into hours; default the cost DB's manpower rate, else the estimate's rate), price?: bool (default true), scenario?: a value engineering scenario id (ve.scenario: its pieces in place of the design's it replaces)} → {lines: [{level?, area?, circuit?, kind, item, size, type, qty, unit, material, labor_dollars, labor_hours, labor_basis, via, handles, elements}], totals: {material, labor_dollars, labor_hours, cost}, firestop, unpriced, file}")
            .noundo(),
    ]
}

fn run_report(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let Some(mut r) = stored_report(d) else {
        return Ok(json!({"routed": false, "note": "no routing stored: model.build routes the circuits when the spec has them"}));
    };
    let max = p.get("max").and_then(Value::as_u64).map_or(200, |v| v.min(20_000) as usize);
    let lists = ["clashes", "conflicts", "penetrations", "bored_holes", "conductors_upsized", "fill", "derated", "unrouted"];
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
    // The table's file: never a drawing's, never replacing one without `force`.
    if let Some(path) = str_param(p, "file").filter(|x| !x.trim().is_empty()) {
        super::exchange::guard_output(s, C, p, path)?;
    }
    let cost_db = match str_param(p, "cost_db") {
        Some(path) => Some(super::valueeng::read_cost_db(path, C)?),
        None => None,
    };
    // A value engineering scenario: its pieces in place of the design's it replaces.
    let (replaced, scenario_pieces) = match str_param(p, "scenario").filter(|x| !x.trim().is_empty()) {
        Some(id) => super::valueeng::takeoff_view(s.doc()?, id)
            .ok_or_else(|| bad(C, format!("there is no value engineering scenario {id} (ve.scenario lists them)")))?,
        None => Default::default(),
    };
    // Every piece of the model, grouped.
    type Key = (String, String, String, String, String, String, String);
    let mut rows: BTreeMap<Key, (f64, Vec<Handle>)> = BTreeMap::new();
    {
        let d = s.doc()?;
        for e in d.model.iter().filter(|e| (mep_piece(e) && !replaced.contains(&e.handle)) || scenario_pieces.contains(&e.handle)) {
            let ps = props::read(&e.common.xdata);
            let g = |k: &str| ps.get(k).map(Prop::text).unwrap_or_default();
            let ve = scenario_pieces.contains(&e.handle);
            let kind = if ve { g(super::valueeng::VE_ATOM) } else { g(super::mepatoms::ATOM_KEY) };
            // Field bends are labor, not material: listed (a count where the route turns) all the same.
            if kind.is_empty() {
                continue;
            }
            let length = g("length_ft").parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0);
            let by_ft = matches!(kind.as_str(), "conduit" | "cable" | "wire");
            let qty = if by_ft { length.unwrap_or(0.0) } else { 1.0 };
            let level = match (groups.iter().any(|x| x == "level"), ve) {
                (false, _) => String::new(),
                (true, true) => g("level"),
                (true, false) => super::bspec::made_level(e).unwrap_or_default(),
            };
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
    // The holes bored through the framing (and passes through trusses): by level, hole size and
    // member, each line to the floors they are in.
    let holes: Vec<Value> = report.as_ref().and_then(|r| r["bored_holes"].as_array().cloned()).unwrap_or_default();
    let mut bh: BTreeMap<(String, String, String, String), (f64, BTreeSet<String>)> = BTreeMap::new();
    for h in &holes {
        let k = (
            if groups.iter().any(|x| x == "level") { h["level"].as_str().unwrap_or_default().to_string() } else { String::new() },
            h["item"].as_str().unwrap_or_default().to_string(),
            super::mepatoms::trade_size(h["hole_in"].as_f64().unwrap_or(0.0)),
            h["member"].as_str().unwrap_or_default().to_string(),
        );
        let e = bh.entry(k).or_insert((0.0, BTreeSet::new()));
        e.0 += 1.0;
        if let Some(f) = h["floor"].as_str() {
            e.1.insert(f.to_string());
        }
    }
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
    for ((level, item, size, member), (n, floors)) in &bh {
        add(&mut lines, [level, "", "", "bored hole", item, size, member], *n, "ea", floors.iter().cloned().collect());
    }
    if price {
        match &cost_db {
            Some(db) => {
                let rate = rate
                    .or(db.manpower_rate)
                    .or_else(|| s.execute("estimate.settings", &json!({})).ok().and_then(|v| v["labor_rate"].as_f64()).filter(|v| *v > 0.0));
                for l in &mut lines {
                    let (item, size) = (l["item"].as_str().unwrap_or_default().to_string(), l["size"].as_str().unwrap_or_default().to_string());
                    let qty = l["qty"].as_f64().unwrap_or(0.0);
                    let hit = db.find(&item, &size);
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
        crate::writefile::write(path, csv).map_err(|e| bad(C, format!("could not write {path}: {e}")))?;
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
        "firestop": firestop.len(), "bored_holes": holes.len(), "unpriced": unpriced, "file": file_out,
        "group_by": groups,
    }))
}

#[path = "conduitroute_gap.rs"]
mod gap;
pub(crate) use gap::panel_end_makeup_ft;

#[cfg(test)]
#[path = "conduitroute_tests.rs"]
mod tests;
