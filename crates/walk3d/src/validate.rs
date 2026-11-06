//! The print validator: a deterministic check of a model against the documents it was built
//! from, run like a compiler or a test suite after every change.
//!
//! Two plain inputs, both in plan feet on each level's own plan:
//! - [`Prints`]: what the documents say — the symbols on each sheet, schedule rows, dimension
//!   strings, rooms, the rules file (mounting heights, spec rules, the source order), approved
//!   submittals and the lugs at each termination point;
//! - [`Model`]: what the model has — elements with their type mark, type fields, mounting height,
//!   host wall and provenance; walls, openings, raceway/duct/pipe runs and levels.
//!
//! [`validate`] runs the checks ([`Check`]) and gives a [`Report`]: a pass/fail list, each
//! failure with its element, rule, expected against found, the source (sheet and location,
//! schedule row or spec section), a severity and a suggested fix, plus a score. A [`Scope`]
//! limits the work to what changed (handles, a level, an area): only the elements, symbols, rooms
//! and runs within reach of it are looked at, so an edit is re-checked in time proportional to
//! the area it touched, not to the building.
//!
//! Nothing here reads files or panics; every list is capped ([`MAX_ITEMS`]) and hostile numbers
//! (NaN, infinities) are skipped.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

/// Items of each kind looked at, at most (hostile inputs).
pub const MAX_ITEMS: usize = 200_000;
/// Failures listed at most.
pub const MAX_FAILURES: usize = 20_000;
/// Points in one run's path, at most.
const MAX_PATH: usize = 10_000;
/// The prints file format version this code writes and reads.
pub const PRINTS_VERSION: u32 = 1;

// ---------------------------------------------------------------- what the documents say

/// What the documents say.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prints {
    pub version: u32,
    pub sheets: Vec<PrintSheet>,
    /// Every symbol on the sheets (devices, fixtures, panels, equipment), at its plan point.
    pub symbols: Vec<Symbol>,
    /// Schedule rows (panel, fixture, equipment, device schedules).
    pub schedule: Vec<ScheduleRow>,
    /// Dimension strings on the architectural sheets.
    pub dimensions: Vec<DimString>,
    /// Rooms with their outlines (for counts per room and unit type, ceilings and exterior).
    pub rooms: Vec<PrintRoom>,
    /// Approved submittals: what a type must be.
    pub submittals: Vec<Submittal>,
    /// Termination points (panelboards, switchboards, disconnects) and their lugs.
    pub terminations: Vec<Termination>,
    pub rules: RulesFile,
    pub settings: Settings,
}

/// One sheet of the set and what it shows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrintSheet {
    pub number: String,
    pub title: String,
    pub level: Option<String>,
    /// Element kinds this sheet shows (`receptacle`, `luminaire`…): model elements of these
    /// kinds on its level must appear on it. Empty: the kinds of its symbols.
    pub kinds: Vec<String>,
}

/// A symbol on a sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Symbol {
    pub sheet: String,
    pub level: String,
    pub kind: String,
    /// Its type mark (the letter in the hexagon, the fixture tag…).
    pub mark: Option<String>,
    /// Its tag (`LP-1A`, `P-12`), when it names one element.
    pub tag: Option<String>,
    pub at: [f64; 2],
    /// A mounting height noted beside it, inches above the floor.
    pub mounting_in: Option<f64>,
}

/// A schedule row.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScheduleRow {
    pub sheet: String,
    /// `luminaire`, `panel`, `equipment`, `device`…
    pub schedule: String,
    pub mark: String,
    pub kind: Option<String>,
    /// The quantity it gives, if any: the placed count must equal it.
    pub qty: Option<u32>,
    pub mounting_in: Option<f64>,
    /// Its fields (description, catalogNumber, volts…).
    pub fields: BTreeMap<String, String>,
}

/// A dimension string: from `a` to `b` reads `value_ft`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DimString {
    pub sheet: String,
    pub level: String,
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub value_ft: f64,
    /// As written (`12'-6"`).
    pub text: String,
    /// What it measures to at walls: `center` (centrelines), `face`, or anything (empty: a
    /// face, a centreline, a jamb or a device, whichever is nearest each end).
    pub to: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrintRoom {
    pub level: String,
    pub number: String,
    pub name: String,
    pub outline: Vec<[f64; 2]>,
    /// Typical unit type (`K`, `QQ`): rooms of one type are counted together too.
    pub unit_type: Option<String>,
    /// Outdoors (patios, the roof): devices there must suit wet locations.
    pub exterior: bool,
    /// Its ceiling height, feet above the floor (the RCP).
    pub ceiling_ft: Option<f64>,
    pub sheet: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Submittal {
    pub mark: String,
    /// Its spec section.
    pub section: String,
    /// What was approved: the type's fields must say the same.
    pub fields: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Termination {
    /// The equipment tag (`LP-1A`).
    pub tag: String,
    pub sheet: String,
    pub lugs: Lugs,
}

/// What a set of lugs takes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lugs {
    /// Conductors per phase it takes, at most.
    pub per_phase: u32,
    /// Smallest and largest wire (`#6`, `1/0`, `250 kcmil`).
    pub min: String,
    pub max: String,
    /// `CU`, `AL` or `AL/CU`.
    pub material: String,
}

/// The project rules file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RulesFile {
    pub heights: Vec<HeightRule>,
    pub spec: Vec<SpecRule>,
    /// Which source wins when two disagree, strongest first (default [`DEFAULT_ORDER`]).
    pub source_order: Vec<String>,
    pub tolerances: Tolerances,
}

/// A mounting height by kind or mark.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeightRule {
    pub kind: Option<String>,
    pub mark: Option<String>,
    pub mounting_in: f64,
    /// Where it comes from (`26 05 00 3.2`, a general note).
    pub source: String,
}

/// A spec rule: elements it applies to must (or must not) have a field value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpecRule {
    /// The spec section (`26 05 33`).
    pub section: String,
    pub kind: Option<String>,
    pub category: Option<String>,
    /// Only elements outdoors (in an exterior room).
    pub outdoors: bool,
    pub field: String,
    /// Allowed values (any word match, case-insensitive); empty: anything not forbidden.
    pub allowed: Vec<String>,
    pub forbidden: Vec<String>,
    /// The rule in words.
    pub text: String,
}

/// How close is close enough, inches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tolerances {
    /// A symbol and its element (plan distance).
    pub place_in: f64,
    /// How far an element may stray and still be the symbol's (beyond: missing and extra).
    pub search_in: f64,
    /// A dimension string against the model.
    pub dim_in: f64,
    /// A mounting height.
    pub height_in: f64,
    /// Run ends meeting.
    pub connect_in: f64,
    /// Failures per 1 000 checked items still counted a pass (the false-alarm threshold).
    pub pass_per_mille: f64,
    /// Along-wall tolerance for misplaced: an element on the correct wall may be this far
    /// along the wall from its symbol before it is misplaced (inches). Default 120 (10 feet)
    /// because E-sheet symbols establish the room and wall, not the exact stud bay.
    pub along_wall_in: f64,
}

impl Default for Tolerances {
    fn default() -> Self {
        Tolerances { place_in: 6.0, search_in: 48.0, dim_in: 0.5, height_in: 1.0, connect_in: 1.0, pass_per_mille: 0.0, along_wall_in: 120.0 }
    }
}

/// What an agent's edit that adds failures gets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateMode {
    /// Not checked.
    Off,
    /// The new failures come back with the result.
    #[default]
    Warn,
    /// The edit is undone and refused.
    Refuse,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Agent edits through MCP and the control channel.
    pub gate: GateMode,
}

/// The default source order, strongest first.
pub const DEFAULT_ORDER: [&str; 6] = ["field_verified", "override", "submittal", "schedule", "note", "rule"];

impl Prints {
    pub fn from_json(text: &str) -> Result<Prints, String> {
        let p: Prints = serde_json::from_str(text).map_err(|e| format!("prints: {e}"))?;
        if p.version > PRINTS_VERSION {
            return Err(format!("prints: version {} is newer than this CADaddy reads ({PRINTS_VERSION})", p.version));
        }
        Ok(p.capped())
    }

    /// Lists cut to [`MAX_ITEMS`], non-finite points dropped.
    pub fn capped(mut self) -> Prints {
        self.version = PRINTS_VERSION;
        self.symbols.retain(|s| fin2(s.at));
        self.symbols.truncate(MAX_ITEMS);
        self.schedule.truncate(MAX_ITEMS);
        self.dimensions.retain(|d| fin2(d.a) && fin2(d.b) && d.value_ft.is_finite());
        self.dimensions.truncate(MAX_ITEMS);
        for r in &mut self.rooms {
            r.outline.retain(|p| fin2(*p));
            r.outline.truncate(MAX_PATH);
        }
        self.rooms.truncate(MAX_ITEMS);
        self.sheets.truncate(MAX_ITEMS);
        self.submittals.truncate(MAX_ITEMS);
        self.terminations.truncate(MAX_ITEMS);
        self.rules.heights.retain(|h| h.mounting_in.is_finite());
        self.rules.heights.truncate(MAX_ITEMS);
        self.rules.spec.truncate(MAX_ITEMS);
        self.rules.source_order.truncate(64);
        let t = &mut self.rules.tolerances;
        let d = Tolerances::default();
        for (v, dv) in [
            (&mut t.place_in, d.place_in),
            (&mut t.search_in, d.search_in),
            (&mut t.dim_in, d.dim_in),
            (&mut t.height_in, d.height_in),
            (&mut t.connect_in, d.connect_in),
            (&mut t.pass_per_mille, d.pass_per_mille),
            (&mut t.along_wall_in, d.along_wall_in),
        ] {
            if !v.is_finite() || *v < 0.0 || *v > 1.0e6 {
                *v = dv;
            }
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
            && self.schedule.is_empty()
            && self.dimensions.is_empty()
            && self.rooms.is_empty()
            && self.submittals.is_empty()
            && self.terminations.is_empty()
            && self.rules.heights.is_empty()
            && self.rules.spec.is_empty()
    }

    fn order(&self) -> Vec<String> {
        if self.rules.source_order.is_empty() {
            DEFAULT_ORDER.iter().map(|s| s.to_string()).collect()
        } else {
            self.rules.source_order.iter().map(|s| s.to_ascii_lowercase()).collect()
        }
    }
}

// ---------------------------------------------------------------- what the model has

/// What the model has.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Model {
    pub levels: Vec<ModelLevel>,
    pub elements: Vec<Element>,
    pub walls: Vec<ModelWall>,
    pub openings: Vec<ModelOpening>,
    pub runs: Vec<Run>,
    /// Type marks the catalog has (the drawing's BIM types), with their fields.
    pub types: BTreeMap<String, BTreeMap<String, String>>,
    /// Pieces of walls that don't stand on the floor (the wall over a door or window opening):
    /// not in `walls` (an opening is a gap in the plan), but a face an element may be on.
    pub wall_spans: Vec<WallSpan>,
    /// Walls the model has whose footprint gives no centreline, with why.
    pub unmeasured_walls: Vec<UnmeasuredWall>,
}

/// A piece of a wall that spans only part of its level's height (over an opening).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallSpan {
    pub wall: ModelWall,
    /// Its bottom and top, inches above its level's floor.
    pub bottom_in: f64,
    pub top_in: f64,
}

/// A wall of the model that can't be measured, and why.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UnmeasuredWall {
    pub id: String,
    pub level: String,
    /// Where it is, feet on its level's plan.
    pub at: [f64; 2],
    pub why: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelLevel {
    pub name: String,
    /// Floor to floor, feet.
    pub height_ft: f64,
}

/// Where an element's place and type come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Placed where the print shows it.
    Fixed,
    /// Placed by a rule.
    Rule,
    /// Placed somewhere within a zone the print allows.
    Zone,
    /// Measured in the field.
    FieldVerified,
    /// A person's override.
    Override,
    /// An agent's edit.
    Agent,
}

impl Status {
    pub fn parse(s: &str) -> Option<Status> {
        Some(match s.trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "fixed" => Status::Fixed,
            "rule" => Status::Rule,
            "zone" => Status::Zone,
            "field_verified" | "fieldverified" | "verified" => Status::FieldVerified,
            "override" => Status::Override,
            "agent" => Status::Agent,
            _ => return None,
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Status::Fixed => "fixed",
            Status::Rule => "rule",
            Status::Zone => "zone",
            Status::FieldVerified => "field_verified",
            Status::Override => "override",
            Status::Agent => "agent",
        }
    }
}

/// How an element is mounted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mounting {
    /// Not said: inferred from the kind ([`mounting_of`]).
    #[default]
    Unknown,
    Wall,
    Ceiling,
    Floor,
    /// None of those (on furniture, a column, a pad): no mounting checks.
    Other,
}

/// A model element.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Element {
    /// Its handle.
    pub id: String,
    pub kind: String,
    pub mark: Option<String>,
    pub tag: Option<String>,
    pub level: String,
    /// Plan point, feet.
    pub at: [f64; 2],
    /// Mounting height (centre of a wall device, the face of a ceiling fixture), inches above
    /// the floor.
    pub z_in: Option<f64>,
    pub mounting: Mounting,
    /// Its box depth, inches (wall devices).
    pub box_depth_in: Option<f64>,
    /// Its host wall's id.
    pub wall: Option<String>,
    pub status: Option<Status>,
    /// Where it was traced from (`E-201 @ 12'-3", 40'-0"`, `rule: 18" AFF`).
    pub source: Option<String>,
    /// Its type's fields (resolved by mark).
    pub fields: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelWall {
    pub id: String,
    pub level: String,
    /// Centreline, feet.
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub thickness_in: f64,
    /// Stud depth, inches (a box deeper than this doesn't fit).
    pub stud_in: Option<f64>,
    pub wall_type: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelOpening {
    pub id: String,
    pub level: String,
    /// `door` or `window`.
    pub kind: String,
    pub min: [f64; 2],
    pub max: [f64; 2],
    /// A door's swing: hinge point and leaf width, feet.
    pub swing: Option<([f64; 2], f64)>,
}

/// A conduit, duct or pipe run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Run {
    pub id: String,
    /// `conduit`, `duct`, `pipe`.
    pub kind: String,
    pub level: String,
    pub path: Vec<[f64; 2]>,
    pub mark: Option<String>,
    /// A homerun: the panel tag it must end at.
    pub homerun: Option<String>,
    /// Conductors it carries to a termination.
    pub conductors: Option<Conductors>,
    pub status: Option<Status>,
    pub source: Option<String>,
    pub fields: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Conductors {
    pub per_phase: u32,
    pub size: String,
    pub material: String,
}

impl Model {
    /// Lists cut to [`MAX_ITEMS`], non-finite points dropped.
    pub fn capped(mut self) -> Model {
        self.elements.retain(|e| fin2(e.at) && e.z_in.is_none_or(f64::is_finite));
        self.elements.truncate(MAX_ITEMS);
        self.walls.retain(|w| fin2(w.a) && fin2(w.b) && w.thickness_in.is_finite());
        self.walls.truncate(MAX_ITEMS);
        self.wall_spans
            .retain(|s| fin2(s.wall.a) && fin2(s.wall.b) && s.wall.thickness_in.is_finite() && s.bottom_in.is_finite() && s.top_in.is_finite());
        self.wall_spans.truncate(MAX_ITEMS);
        self.unmeasured_walls.truncate(MAX_ITEMS);
        self.openings.retain(|o| fin2(o.min) && fin2(o.max));
        self.openings.truncate(MAX_ITEMS);
        for r in &mut self.runs {
            r.path.retain(|p| fin2(*p));
            r.path.truncate(MAX_PATH);
        }
        self.runs.truncate(MAX_ITEMS);
        self
    }
}

// ---------------------------------------------------------------- the report

/// The checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// Back to the sheet: symbols against elements, place by place.
    Overlay,
    /// Counts per sheet, room and unit type; schedule quantities.
    Counts,
    /// Dimension strings re-measured in the model.
    Dimensions,
    /// Tags resolve, types and heights agree with the documents.
    Tags,
    /// Runs connect end to end; homeruns end at their panel.
    Connectivity,
    /// In the wall, out of the door, at the ceiling, not floating, not doubled.
    Physical,
    /// Every element traced to a source.
    Provenance,
    /// Spec rules.
    Spec,
    /// Conductors fit the lugs they land on.
    Termination,
}

pub const ALL_CHECKS: [Check; 9] = [
    Check::Overlay,
    Check::Counts,
    Check::Dimensions,
    Check::Tags,
    Check::Connectivity,
    Check::Physical,
    Check::Provenance,
    Check::Spec,
    Check::Termination,
];

impl Check {
    pub fn parse(s: &str) -> Option<Check> {
        ALL_CHECKS.iter().copied().find(|c| c.name().eq_ignore_ascii_case(s.trim()))
    }
    pub fn name(self) -> &'static str {
        match self {
            Check::Overlay => "overlay",
            Check::Counts => "counts",
            Check::Dimensions => "dimensions",
            Check::Tags => "tags",
            Check::Connectivity => "connectivity",
            Check::Physical => "physical",
            Check::Provenance => "provenance",
            Check::Spec => "spec",
            Check::Termination => "termination",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    High,
    Medium,
    Low,
}

impl Severity {
    pub fn parse(s: &str) -> Option<Severity> {
        match s.trim().to_ascii_lowercase().as_str() {
            "high" => Some(Severity::High),
            "medium" => Some(Severity::Medium),
            "low" => Some(Severity::Low),
            _ => None,
        }
    }
    fn weight(self) -> f64 {
        match self {
            Severity::High => 3.0,
            Severity::Medium => 2.0,
            Severity::Low => 1.0,
        }
    }
}

/// Where a failure's truth comes from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    /// Plan point on the sheet's level, feet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

/// One failure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    /// Stable across runs while the failure is the same (`V-1a2b3c4d`).
    pub id: String,
    pub check: Check,
    /// The rule broken (`missing_in_model`, `wrong_type`…).
    pub rule: String,
    /// The element's id, if one is involved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    pub level: String,
    /// Where it is, plan feet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    pub expected: String,
    pub found: String,
    pub source: Source,
    pub severity: Severity,
    pub fix: String,
}

/// The validator's answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// No failures beyond the false-alarm threshold.
    pub pass: bool,
    /// 0–100: the share of checked items that pass, weighted by severity.
    pub score: f64,
    pub failures: Vec<Failure>,
    /// Items looked at per check.
    pub checked: BTreeMap<String, usize>,
    /// Whether the run was limited to a scope.
    pub scoped: bool,
}

impl Report {
    pub fn by_check(&self) -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for f in &self.failures {
            *m.entry(f.check.name().to_string()).or_insert(0) += 1;
        }
        m
    }
    pub fn by_severity(&self) -> [usize; 3] {
        let mut n = [0; 3];
        for f in &self.failures {
            if let Some(c) = n.get_mut(f.severity as usize) {
                *c += 1;
            }
        }
        n
    }
}

/// What to look at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scope {
    /// Only these checks (empty: all).
    pub checks: Vec<Check>,
    /// Only these levels (empty: all).
    pub levels: Vec<String>,
    /// Only round these elements (ids)…
    pub elements: Vec<String>,
    /// …and these plan areas (feet, `[min, max]`, with their level).
    pub areas: Vec<(String, [[f64; 2]; 2])>,
}

impl Scope {
    pub fn all() -> Scope {
        Scope::default()
    }
    fn spatial(&self) -> bool {
        !self.elements.is_empty() || !self.areas.is_empty()
    }
    fn wants(&self, c: Check) -> bool {
        self.checks.is_empty() || self.checks.contains(&c)
    }
}

// ---------------------------------------------------------------- helpers

fn fin2(p: [f64; 2]) -> bool {
    p[0].is_finite() && p[1].is_finite()
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

/// Distance from `p` to segment `a`–`b`, and the parameter along it (0–1).
fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let d = sub(b, a);
    let l2 = dot(d, d);
    let t = if l2 > 1e-12 { (dot(sub(p, a), d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let q = [a[0] + d[0] * t, a[1] + d[1] * t];
    (dist(p, q), t)
}

fn inside(p: [f64; 2], poly: &[[f64; 2]]) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + n - 1) % n)) else { continue };
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
    }
    c
}

fn area(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    let mut a = 0.0;
    for i in 0..n {
        if let (Some(p), Some(q)) = (poly.get(i), poly.get((i + 1) % n)) {
            a += p[0] * q[1] - q[0] * p[1];
        }
    }
    (a / 2.0).abs()
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

fn same_opt(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => same(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// Feet and inches, as on a print (`12'-6"`, `-0'-3 1/2"`).
pub fn ft_in(ft: f64) -> String {
    if !ft.is_finite() {
        return "?".into();
    }
    let neg = ft < 0.0;
    let total = (ft.abs() * 12.0 * 8.0).round() / 8.0;
    let f = (total / 12.0).floor();
    let i = total - f * 12.0;
    let whole = i.floor();
    let eighths = ((i - whole) * 8.0).round() as u32;
    let frac = match eighths {
        0 => String::new(),
        4 => " 1/2".into(),
        2 | 6 => format!(" {}/4", eighths / 2),
        e => format!(" {e}/8"),
    };
    format!("{}{f}'-{whole}{frac}\"", if neg { "-" } else { "" })
}

fn inches(v: f64) -> String {
    if !v.is_finite() {
        return "?".into();
    }
    let r = (v * 8.0).round() / 8.0;
    if (r - r.round()).abs() < 1e-9 { format!("{}\"", r.round()) } else { format!("{r}\"") }
}

fn pt(p: [f64; 2]) -> String {
    format!("({}, {})", ft_in(p[0]), ft_in(p[1]))
}

/// FNV-1a, for stable failure ids.
fn fnv(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.bytes().chain(std::iter::once(0x1f)) {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("V-{:08x}", (h ^ (h >> 32)) as u32)
}

/// What a kind is mounted on, when the element doesn't say.
pub fn mounting_of(kind: &str) -> Mounting {
    let k = kind.to_ascii_lowercase();
    let any = |w: &[&str]| w.iter().any(|x| k.contains(x));
    if any(&["receptacle", "outlet", "switch", "data", "telephone", "thermostat", "pull", "horn", "strobe_wall", "card_reader", "tv", "jack", "wall"])
    {
        Mounting::Wall
    } else if any(&[
        "luminaire",
        "light",
        "fixture_ceiling",
        "diffuser",
        "grille",
        "smoke",
        "heat_detector",
        "sprinkler",
        "speaker",
        "exit",
        "ceiling",
        "wap",
        "access_point",
    ]) {
        Mounting::Ceiling
    } else if k == "panel" || any(&["floor", "panelboard", "transformer", "switchboard", "mdf", "idf", "rack"]) {
        Mounting::Floor
    } else {
        Mounting::Other
    }
}

/// A wire size in circular mils (`#12`, `12 AWG`, `1/0`, `250 kcmil`, `250MCM`), for ordering.
pub fn wire_cmil(s: &str) -> Option<f64> {
    let t = s.trim().to_ascii_lowercase().replace(['#', ' '], "");
    let t = t.trim_end_matches("awg");
    if let Some(k) = t.strip_suffix("kcmil").or_else(|| t.strip_suffix("mcm")) {
        return k.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0).map(|v| v * 1000.0);
    }
    let awg: i32 = match t {
        "1/0" => 0,
        "2/0" => -1,
        "3/0" => -2,
        "4/0" => -3,
        _ => t.parse::<i32>().ok().filter(|n| (1..=40).contains(n))?,
    };
    // d(mils) = 5 × 92^((36 − n)/39); cmil = d².
    let d = 5.0 * 92f64.powf(f64::from(36 - awg) / 39.0);
    Some(d * d)
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric() && c != '/').filter(|w| !w.is_empty()).map(str::to_ascii_lowercase).collect()
}

/// Whether `value` says `want` (a whole word or the whole value, case-insensitive).
fn says(value: &str, want: &str) -> bool {
    let w = want.trim().to_ascii_lowercase();
    !w.is_empty() && (value.trim().eq_ignore_ascii_case(&w) || words(value).contains(&w))
}

/// Level filter and spatial reach of a scope.
struct Reach<'a> {
    scope: &'a Scope,
    /// Per level: the boxes in reach.
    boxes: HashMap<String, Vec<[[f64; 2]; 2]>>,
    ids: BTreeSet<&'a str>,
}

impl<'a> Reach<'a> {
    fn new(scope: &'a Scope, model: &Model, grow_ft: f64) -> Reach<'a> {
        let mut boxes: HashMap<String, Vec<[[f64; 2]; 2]>> = HashMap::new();
        let ids: BTreeSet<&str> = scope.elements.iter().map(String::as_str).collect();
        let mut add = |level: &str, a: [f64; 2], b: [f64; 2]| {
            let bx = [[a[0].min(b[0]) - grow_ft, a[1].min(b[1]) - grow_ft], [a[0].max(b[0]) + grow_ft, a[1].max(b[1]) + grow_ft]];
            boxes.entry(level.to_ascii_lowercase()).or_default().push(bx);
        };
        for e in model.elements.iter().filter(|e| ids.contains(e.id.as_str())) {
            add(&e.level, e.at, e.at);
        }
        for w in model.walls.iter().filter(|w| ids.contains(w.id.as_str())) {
            add(&w.level, w.a, w.b);
        }
        for o in model.openings.iter().filter(|o| ids.contains(o.id.as_str())) {
            add(&o.level, o.min, o.max);
        }
        for r in model.runs.iter().filter(|r| ids.contains(r.id.as_str())) {
            for p in &r.path {
                add(&r.level, *p, *p);
            }
        }
        for (l, [a, b]) in &scope.areas {
            add(l, *a, *b);
        }
        Reach { scope, boxes, ids }
    }
    fn level(&self, l: &str) -> bool {
        self.scope.levels.is_empty() || self.scope.levels.iter().any(|x| same(x, l))
    }
    /// Whether a point on a level is in reach.
    fn at(&self, l: &str, p: [f64; 2]) -> bool {
        if !self.level(l) {
            return false;
        }
        if !self.scope.spatial() {
            return true;
        }
        self.boxes.get(&l.to_ascii_lowercase()).is_some_and(|v| v.iter().any(|[a, b]| p[0] >= a[0] && p[0] <= b[0] && p[1] >= a[1] && p[1] <= b[1]))
    }
    fn id(&self, id: &str) -> bool {
        self.ids.contains(id)
    }
}

/// A uniform grid over plan points for the nearest-neighbour searches (per level).
struct Grid {
    cell: f64,
    map: HashMap<(String, i64, i64), Vec<usize>>,
}

impl Grid {
    fn new(cell: f64) -> Grid {
        Grid { cell: cell.max(0.5), map: HashMap::new() }
    }
    fn key(&self, l: &str, p: [f64; 2]) -> (String, i64, i64) {
        let c = |v: f64| (v / self.cell).floor().clamp(-1.0e12, 1.0e12) as i64;
        (l.to_ascii_lowercase(), c(p[0]), c(p[1]))
    }
    fn put(&mut self, l: &str, p: [f64; 2], i: usize) {
        let k = self.key(l, p);
        self.map.entry(k).or_default().push(i);
    }
    /// Items in the cells within `r` of `p`.
    fn near(&self, l: &str, p: [f64; 2], r: f64) -> Vec<usize> {
        let (lv, cx, cy) = self.key(l, p);
        let n = ((r / self.cell).ceil() as i64).clamp(1, 50);
        let mut out = Vec::new();
        for dx in -n..=n {
            for dy in -n..=n {
                if let Some(v) = self.map.get(&(lv.clone(), cx.saturating_add(dx), cy.saturating_add(dy))) {
                    out.extend_from_slice(v);
                }
            }
        }
        out
    }
}

struct Ctx<'a> {
    prints: &'a Prints,
    model: &'a Model,
    reach: Reach<'a>,
    tol: Tolerances,
    out: Vec<Failure>,
    checked: BTreeMap<String, usize>,
    /// Weighted failures (for the score).
    weight: f64,
}

#[allow(clippy::too_many_arguments)]
fn failure(
    check: Check,
    rule: &str,
    element: Option<&str>,
    level: &str,
    at: Option<[f64; 2]>,
    expected: String,
    found: String,
    source: Source,
    severity: Severity,
    fix: String,
) -> Failure {
    let key_at = at.map(|p| format!("{:.1},{:.1}", p[0], p[1])).unwrap_or_default();
    let id =
        fnv(&[check.name(), rule, element.unwrap_or(""), level, source.sheet.as_deref().unwrap_or(""), source.row.as_deref().unwrap_or(""), &key_at]);
    Failure { id, check, rule: rule.into(), element: element.map(str::to_string), level: level.into(), at, expected, found, source, severity, fix }
}

impl Ctx<'_> {
    fn push(&mut self, f: Failure) {
        if self.out.len() < MAX_FAILURES {
            self.weight += f.severity.weight();
            self.out.push(f);
        }
    }
    fn count(&mut self, c: Check, n: usize) {
        *self.checked.entry(c.name().into()).or_insert(0) += n;
    }
    fn el_in(&self, e: &Element) -> bool {
        self.reach.id(&e.id) || self.reach.at(&e.level, e.at)
    }
    fn sym_in(&self, s: &Symbol) -> bool {
        self.reach.at(&s.level, s.at)
    }
    /// The room a point is in: the smallest one, where rooms overlap (a closet in a suite).
    fn room_of(&self, level: &str, p: [f64; 2]) -> Option<&PrintRoom> {
        self.prints
            .rooms
            .iter()
            .filter(|r| same(&r.level, level) && r.outline.len() >= 3 && inside(p, &r.outline))
            .min_by(|a, b| area(&a.outline).total_cmp(&area(&b.outline)))
    }
    fn level_height(&self, level: &str) -> Option<f64> {
        self.model.levels.iter().find(|l| same(&l.name, level)).map(|l| l.height_ft).filter(|h| h.is_finite() && *h > 0.0)
    }
    fn known_mark(&self, mark: &str) -> bool {
        self.model.types.keys().any(|k| same(k, mark)) || self.prints.schedule.iter().any(|r| same(&r.mark, mark))
    }
}

// ---------------------------------------------------------------- the checks

/// Run the checks in `scope`.
pub fn validate(prints: &Prints, model: &Model, scope: &Scope) -> Report {
    let tol = prints.rules.tolerances.clone();
    // A scope reaches as far as a symbol may stray from its element, and a little more.
    let grow = (tol.search_in / 12.0).max(1.0) + 2.0;
    let mut cx = Ctx { prints, model, reach: Reach::new(scope, model, grow), tol, out: Vec::new(), checked: BTreeMap::new(), weight: 0.0 };
    if scope.wants(Check::Overlay) {
        overlay(&mut cx);
    }
    if scope.wants(Check::Counts) {
        counts(&mut cx);
    }
    if scope.wants(Check::Dimensions) {
        dimensions(&mut cx);
    }
    if scope.wants(Check::Tags) {
        tags(&mut cx);
    }
    if scope.wants(Check::Connectivity) {
        connectivity(&mut cx);
    }
    if scope.wants(Check::Physical) {
        physical(&mut cx);
    }
    if scope.wants(Check::Provenance) {
        provenance(&mut cx);
    }
    if scope.wants(Check::Spec) {
        spec(&mut cx);
    }
    if scope.wants(Check::Termination) {
        termination(&mut cx);
    }
    let total: usize = cx.checked.values().sum();
    let mut failures = cx.out;
    failures.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.check.cmp(&b.check)).then_with(|| a.id.cmp(&b.id)));
    failures.dedup_by(|a, b| a.id == b.id);
    let allowed = cx.tol.pass_per_mille * total as f64 / 1000.0;
    let score = if total == 0 { 100.0 } else { (100.0 * (1.0 - cx.weight / (3.0 * total as f64))).clamp(0.0, 100.0) };
    Report {
        pass: failures.len() as f64 <= allowed,
        score: (score * 10.0).round() / 10.0,
        failures,
        checked: cx.checked,
        scoped: scope.spatial() || !scope.levels.is_empty(),
    }
}

/// Model elements of the kinds the prints show, per level.
fn shown_kinds(p: &Prints) -> HashMap<String, BTreeSet<String>> {
    let mut m: HashMap<String, BTreeSet<String>> = HashMap::new();
    for s in &p.symbols {
        m.entry(s.level.to_ascii_lowercase()).or_default().insert(s.kind.to_ascii_lowercase());
    }
    for sh in &p.sheets {
        if let Some(l) = &sh.level {
            for k in &sh.kinds {
                m.entry(l.to_ascii_lowercase()).or_default().insert(k.to_ascii_lowercase());
            }
        }
    }
    m
}

/// The nearest wall (by index) for a point, within a tolerance.
fn nearest_wall<'a>(walls: &'a [ModelWall], wall_grid: &Grid, level: &str, p: [f64; 2], tol: f64) -> Option<(usize, &'a ModelWall)> {
    let mut ids = wall_grid.near(level, p, tol);
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .filter_map(|i| walls.get(i).filter(|w| same(&w.level, level)).map(|w| (i, w)))
        .min_by(|(_, a), (_, b)| seg_dist(p, a.a, a.b).0.total_cmp(&seg_dist(p, b.a, b.b).0))
        .filter(|(_, w)| seg_dist(p, w.a, w.b).0 <= tol)
}

/// A human-readable description of a location (room name/number and wall).
fn location_desc(room: Option<&PrintRoom>, wall: Option<&ModelWall>) -> String {
    let room_part = room.map_or_else(
        || "an unknown area".to_string(),
        |r| {
            let label = if !r.name.is_empty() { &r.name } else { &r.number };
            format!("room {label}")
        },
    );
    match wall {
        Some(w) => format!("{room_part}, wall {}", w.id),
        None => room_part,
    }
}

/// Each symbol's element (one to one, nearest first, within the search distance), by index.
fn pairing(cx: &Ctx) -> (Vec<Option<usize>>, Vec<bool>) {
    let p = cx.prints;
    let m = cx.model;
    let search = cx.tol.search_in / 12.0;
    let mut grid = Grid::new(search.max(1.0));
    for (i, e) in m.elements.iter().enumerate() {
        grid.put(&e.level, e.at, i);
    }
    let mut cand: Vec<(f64, usize, usize)> = Vec::new();
    for (si, s) in p.symbols.iter().enumerate() {
        if !cx.sym_in(s) {
            continue;
        }
        for ei in grid.near(&s.level, s.at, search) {
            let Some(e) = m.elements.get(ei) else { continue };
            if !same(&e.level, &s.level) || !same(&e.kind, &s.kind) {
                continue;
            }
            let d = dist(e.at, s.at);
            if d <= search {
                // Same tag first, then the same mark, then distance.
                let tag_bonus = if s.tag.is_some() && same_opt(s.tag.as_deref(), e.tag.as_deref()) { -1.0e6 } else { 0.0 };
                let mark_bonus = if same_opt(s.mark.as_deref(), e.mark.as_deref()) { -search * 0.5 } else { 0.0 };
                cand.push((d + tag_bonus + mark_bonus, si, ei));
            }
        }
    }
    cand.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let mut sym = vec![None; p.symbols.len()];
    let mut used = vec![false; m.elements.len()];
    for (_, si, ei) in cand {
        let (Some(slot), Some(u)) = (sym.get_mut(si), used.get_mut(ei)) else { continue };
        if slot.is_none() && !*u {
            *slot = Some(ei);
            *u = true;
        }
    }
    (sym, used)
}

fn sym_source(s: &Symbol) -> Source {
    Source { sheet: Some(s.sheet.clone()), at: Some(s.at), ..Source::default() }
}

fn label(kind: &str, mark: Option<&str>) -> String {
    match mark {
        Some(m) if !m.is_empty() => format!("{} type {m}", kind.replace('_', " ")),
        _ => kind.replace('_', " "),
    }
}

fn overlay(cx: &mut Ctx) {
    let (pairs, used) = pairing(cx);
    let p = cx.prints;
    let m = cx.model;
    let along_wall = cx.tol.along_wall_in / 12.0;
    let wall_tol: f64 = 4.0; // feet: nearness threshold for "on a wall"
    // Wall grid for location lookups.
    let mut wall_grid = Grid::new(wall_tol.max(1.0));
    for (i, w) in m.walls.iter().enumerate() {
        let l = dist(w.a, w.b);
        let steps = ((l / 4.0).ceil() as usize).clamp(1, 2_000);
        for k in 0..=steps {
            let t = k as f64 / steps as f64;
            wall_grid.put(&w.level, [w.a[0] + (w.b[0] - w.a[0]) * t, w.a[1] + (w.b[1] - w.a[1]) * t], i);
        }
    }
    let mut n = 0;
    let mut out = Vec::new();
    for (s, pair) in p.symbols.iter().zip(&pairs) {
        if !cx.sym_in(s) {
            continue;
        }
        n += 1;
        match pair.and_then(|i| m.elements.get(i)) {
            None => out.push(failure(
                Check::Overlay,
                "missing_in_model",
                None,
                &s.level,
                Some(s.at),
                format!("{} at {}", label(&s.kind, s.mark.as_deref()), pt(s.at)),
                "nothing in the model".into(),
                sym_source(s),
                Severity::High,
                format!("Add a {} at {} as {} shows.", label(&s.kind, s.mark.as_deref()), pt(s.at), s.sheet),
            )),
            Some(e) => {
                // Field-verified places and overrides are where they are on purpose.
                let kept = matches!(e.status, Some(Status::FieldVerified | Status::Override));
                // Zone elements are checked only against their zone (if stored), not the
                // symbol point. With no zone geometry stored yet, they are exempt.
                let zone = matches!(e.status, Some(Status::Zone));
                if kept || zone {
                    continue;
                }
                // Location doctrine: E-sheet symbols establish WHAT and which room/wall, not
                // the exact point. `misplaced` fires only when the element is in a different
                // room or on a different wall than its symbol, or beyond the along-wall
                // tolerance. The fix names the room/wall, never the symbol coordinates.
                let sym_room = cx.room_of(&s.level, s.at);
                let sym_wall = nearest_wall(&m.walls, &wall_grid, &s.level, s.at, wall_tol);
                let el_room = cx.room_of(&e.level, e.at);
                let el_wall = e
                    .wall
                    .as_deref()
                    .and_then(|id| m.walls.iter().enumerate().find(|(_, w)| w.id == id))
                    .or_else(|| nearest_wall(&m.walls, &wall_grid, &e.level, e.at, wall_tol));
                let same_room = match (sym_room, el_room) {
                    (Some(sr), Some(er)) => sr.number.eq_ignore_ascii_case(&er.number),
                    // No rooms at all, or one side has no room: skip room check.
                    _ => true,
                };
                // Location doctrine: within `place_in` always passes (symbols are
                // drawn approximately). Beyond that, check room and wall.
                let d = dist(e.at, s.at);
                let place = cx.tol.place_in / 12.0;
                let misplaced = if d <= place {
                    // Within the symbol tolerance: never misplaced.
                    false
                } else if !same_room {
                    // Different room and beyond place_in: misplaced.
                    true
                } else if let (Some((si, _)), Some((ei, _))) = (sym_wall, el_wall) {
                    if si != ei {
                        // Different wall in the same room: misplaced.
                        true
                    } else {
                        // Same wall: check along-wall distance.
                        let w = &m.walls[si];
                        let (_, ts) = seg_dist(s.at, w.a, w.b);
                        let (_, te) = seg_dist(e.at, w.a, w.b);
                        let wlen = dist(w.a, w.b);
                        (ts - te).abs() * wlen > along_wall
                    }
                } else {
                    // Same room, no wall data: only the radial distance matters,
                    // and we already know d > place_in. Misplaced.
                    true
                };
                if misplaced {
                    let expected_loc = location_desc(sym_room, sym_wall.map(|(_, w)| w));
                    let found_loc = location_desc(el_room, el_wall.map(|(_, w)| w));
                    out.push(failure(
                        Check::Overlay,
                        "misplaced",
                        Some(&e.id),
                        &s.level,
                        Some(e.at),
                        format!("in {expected_loc}"),
                        format!("in {found_loc}"),
                        sym_source(s),
                        Severity::Medium,
                        format!("Move it to {expected_loc} as {} shows.", s.sheet,),
                    ));
                }
            }
        }
    }
    let shown = shown_kinds(p);
    for (e, u) in m.elements.iter().zip(&used) {
        if *u || !cx.el_in(e) {
            continue;
        }
        let on_prints = shown.get(&e.level.to_ascii_lowercase()).is_some_and(|k| k.contains(&e.kind.to_ascii_lowercase()));
        if !on_prints {
            continue;
        }
        n += 1;
        if matches!(e.status, Some(Status::FieldVerified | Status::Override)) {
            continue;
        }
        out.push(failure(
            Check::Overlay,
            "not_on_print",
            Some(&e.id),
            &e.level,
            Some(e.at),
            "a symbol on the sheet for it".into(),
            format!("{} at {} with no symbol within {}", label(&e.kind, e.mark.as_deref()), pt(e.at), ft_in(cx.tol.search_in / 12.0)),
            Source { sheet: sheet_for(p, &e.level, &e.kind), at: Some(e.at), ..Source::default() },
            Severity::Medium,
            "Delete it, or record why it is there (an override or a field-verified place).".into(),
        ));
    }
    cx.count(Check::Overlay, n);
    for f in out {
        cx.push(f);
    }
}

/// The sheet showing a kind on a level.
fn sheet_for(p: &Prints, level: &str, kind: &str) -> Option<String> {
    p.symbols.iter().find(|s| same(&s.level, level) && same(&s.kind, kind)).map(|s| s.sheet.clone()).or_else(|| {
        p.sheets
            .iter()
            .find(|s| s.level.as_deref().is_some_and(|l| same(l, level)) && s.kinds.iter().any(|k| same(k, kind)))
            .map(|s| s.number.clone())
    })
}

fn counts(cx: &mut Ctx) {
    let p = cx.prints;
    let m = cx.model;
    let shown = shown_kinds(p);
    // Per room (and its unit type), per kind and mark. Rooms touched by the scope only.
    type Key = (String, String, String);
    let mut by_room: BTreeMap<Key, (usize, usize, String)> = BTreeMap::new();
    let mut by_unit: BTreeMap<(String, String, String), (usize, usize)> = BTreeMap::new();
    let room_key = |r: &PrintRoom| format!("{}|{}", r.level.to_ascii_lowercase(), r.number);
    let mut rooms_in: BTreeSet<String> = BTreeSet::new();
    for s in p.symbols.iter() {
        if cx.sym_in(s)
            && let Some(r) = cx.room_of(&s.level, s.at)
        {
            rooms_in.insert(room_key(r));
        }
    }
    for e in m.elements.iter() {
        if cx.el_in(e)
            && let Some(r) = cx.room_of(&e.level, e.at)
        {
            rooms_in.insert(room_key(r));
        }
    }
    let all_rooms = !cx.reach.scope.spatial() && cx.reach.scope.levels.is_empty();
    for s in &p.symbols {
        let Some(r) = cx.room_of(&s.level, s.at) else { continue };
        let k = room_key(r);
        if !all_rooms && !rooms_in.contains(&k) {
            continue;
        }
        let e = by_room.entry((k, s.kind.to_ascii_lowercase(), s.mark.clone().unwrap_or_default().to_ascii_uppercase())).or_insert((
            0,
            0,
            s.sheet.clone(),
        ));
        e.0 += 1;
        if let Some(u) = &r.unit_type {
            by_unit.entry((u.clone(), s.kind.to_ascii_lowercase(), s.mark.clone().unwrap_or_default().to_ascii_uppercase())).or_insert((0, 0)).0 += 1;
        }
    }
    for e in &m.elements {
        if !shown.get(&e.level.to_ascii_lowercase()).is_some_and(|k| k.contains(&e.kind.to_ascii_lowercase())) {
            continue;
        }
        let Some(r) = cx.room_of(&e.level, e.at) else { continue };
        let k = room_key(r);
        if !all_rooms && !rooms_in.contains(&k) {
            continue;
        }
        let sheet = sheet_for(p, &e.level, &e.kind).unwrap_or_default();
        let c = by_room.entry((k, e.kind.to_ascii_lowercase(), e.mark.clone().unwrap_or_default().to_ascii_uppercase())).or_insert((0, 0, sheet));
        c.1 += 1;
        if let Some(u) = &r.unit_type {
            by_unit.entry((u.clone(), e.kind.to_ascii_lowercase(), e.mark.clone().unwrap_or_default().to_ascii_uppercase())).or_insert((0, 0)).1 += 1;
        }
    }
    let mut out = Vec::new();
    let mut n = 0;
    for ((room, kind, mark), (want, have, sheet)) in &by_room {
        n += 1;
        if want != have {
            let (level, number) = room.split_once('|').unwrap_or(("", room));
            let r = p.rooms.iter().find(|r| same(&r.level, level) && r.number == number);
            out.push(failure(
                Check::Counts,
                "room_count",
                None,
                r.map_or(level, |r| r.level.as_str()),
                r.and_then(|r| centroid(&r.outline)),
                format!("{want} × {} in room {number}", label(kind, Some(mark).filter(|m| !m.is_empty()).map(String::as_str))),
                format!("{have} in the model"),
                Source { sheet: Some(sheet.clone()), row: Some(format!("room {number}")), ..Source::default() },
                Severity::Medium,
                if have < want {
                    format!("Add {} as {sheet} shows.", want - have)
                } else {
                    format!("Remove {} the print doesn't show.", have - want)
                },
            ));
        }
    }
    if all_rooms {
        for ((unit, kind, mark), (want, have)) in &by_unit {
            n += 1;
            if want != have {
                out.push(failure(
                    Check::Counts,
                    "unit_type_count",
                    None,
                    "",
                    None,
                    format!("{want} × {} in unit type {unit}", label(kind, Some(mark).filter(|m| !m.is_empty()).map(String::as_str))),
                    format!("{have} in the model"),
                    Source { row: Some(format!("unit type {unit}")), ..Source::default() },
                    Severity::Low,
                    "Check the units of this type against the typical unit plan.".into(),
                ));
            }
        }
    }
    // Schedule quantities against placed counts (the whole building: a quantity is a total).
    if all_rooms || cx.reach.scope.spatial() {
        for row in p.schedule.iter().filter(|r| r.qty.is_some()) {
            let want = row.qty.unwrap_or(0) as usize;
            let touched = all_rooms || m.elements.iter().any(|e| cx.el_in(e) && same_opt(e.mark.as_deref(), Some(&row.mark)));
            if !touched {
                continue;
            }
            n += 1;
            let have = m
                .elements
                .iter()
                .filter(|e| same_opt(e.mark.as_deref(), Some(&row.mark)) && row.kind.as_deref().is_none_or(|k| same(k, &e.kind)))
                .count();
            if have != want {
                out.push(failure(
                    Check::Counts,
                    "schedule_quantity",
                    None,
                    "",
                    None,
                    format!("{want} × type {} ({} schedule)", row.mark, row.schedule),
                    format!("{have} placed"),
                    Source { sheet: Some(row.sheet.clone()), row: Some(format!("{} {}", row.schedule, row.mark)), ..Source::default() },
                    Severity::Medium,
                    format!("Make the placed count of type {} match the schedule on {}, or raise an RFI.", row.mark, row.sheet),
                ));
            }
        }
    }
    cx.count(Check::Counts, n);
    for f in out {
        cx.push(f);
    }
}

fn centroid(poly: &[[f64; 2]]) -> Option<[f64; 2]> {
    if poly.is_empty() {
        return None;
    }
    let n = poly.len() as f64;
    let (x, y) = poly.iter().fold((0.0, 0.0), |(x, y), p| (x + p[0], y + p[1]));
    Some([x / n, y / n])
}

/// Where the model has a feature across `u` near the line through `p`: wall faces, opening
/// jambs and element centres, as a coordinate along `u`; the nearest to `p`'s within `reach`.
fn feature(m: &Model, level: &str, p: [f64; 2], u: [f64; 2], reach: f64, to: &str) -> Option<f64> {
    let centre_only = to.eq_ignore_ascii_case("center") || to.eq_ignore_ascii_case("centre") || to.eq_ignore_ascii_case("centerline");
    let face_only = to.eq_ignore_ascii_case("face");
    let t0 = dot(p, u);
    let v = [-u[1], u[0]];
    let s0 = dot(p, v);
    let mut best: Option<(f64, f64)> = None;
    let mut take = |t: f64| {
        let d = (t - t0).abs();
        if d <= reach && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, t));
        }
    };
    for w in m.walls.iter().filter(|w| same(&w.level, level)) {
        let dir = sub(w.b, w.a);
        let len = dir[0].hypot(dir[1]);
        if len < 1e-9 {
            continue;
        }
        // Walls across the dimension line only.
        if (dot(dir, u) / len).abs() > 0.2 {
            continue;
        }
        // …that the line through p crosses (with half a foot of slack at the ends).
        let (sa, sb) = (dot(w.a, v), dot(w.b, v));
        if s0 < sa.min(sb) - 0.5 || s0 > sa.max(sb) + 0.5 {
            continue;
        }
        let c = (dot(w.a, u) + dot(w.b, u)) / 2.0;
        let h = w.thickness_in / 24.0;
        if !centre_only {
            take(c - h);
            take(c + h);
        }
        if !face_only {
            take(c);
        }
    }
    for o in m.openings.iter().filter(|o| same(&o.level, level)) {
        let corners = [o.min, [o.max[0], o.min[1]], o.max, [o.min[0], o.max[1]]];
        let ss: Vec<f64> = corners.iter().map(|c| dot(*c, v)).collect();
        let (lo, hi) = ss.iter().fold((f64::MAX, f64::MIN), |(a, b), s| (a.min(*s), b.max(*s)));
        if s0 < lo - 0.5 || s0 > hi + 0.5 {
            continue;
        }
        for c in corners {
            take(dot(c, u));
        }
    }
    for e in m.elements.iter().filter(|e| same(&e.level, level)) {
        if (dot(e.at, v) - s0).abs() <= 0.5 {
            take(dot(e.at, u));
        }
    }
    best.map(|(_, t)| t)
}

fn dimensions(cx: &mut Ctx) {
    let p = cx.prints;
    let m = cx.model;
    let tol = cx.tol.dim_in / 12.0;
    let mut out = Vec::new();
    let mut n = 0;
    for d in &p.dimensions {
        let mid = [(d.a[0] + d.b[0]) / 2.0, (d.a[1] + d.b[1]) / 2.0];
        if !(cx.reach.at(&d.level, d.a) || cx.reach.at(&d.level, d.b) || cx.reach.at(&d.level, mid)) {
            continue;
        }
        let len = dist(d.a, d.b);
        if len < 1e-6 {
            continue;
        }
        n += 1;
        let u = [(d.b[0] - d.a[0]) / len, (d.b[1] - d.a[1]) / len];
        // Look a little beyond the string's own error for each end.
        let reach = ((d.value_ft - len).abs() + 0.5).min(4.0);
        let src = Source { sheet: Some(d.sheet.clone()), at: Some(mid), row: Some(format!("dimension {}", d.text)), ..Source::default() };
        match (feature(m, &d.level, d.a, u, reach, &d.to), feature(m, &d.level, d.b, u, reach, &d.to)) {
            (Some(ta), Some(tb)) => {
                let got = (tb - ta).abs();
                if (got - d.value_ft).abs() > tol {
                    out.push(failure(
                        Check::Dimensions,
                        "dimension_mismatch",
                        None,
                        &d.level,
                        Some(mid),
                        format!("{} ({})", ft_in(d.value_ft), d.text),
                        format!("{} in the model, {} off", ft_in(got), ft_in((got - d.value_ft).abs())),
                        src,
                        Severity::High,
                        format!(
                            "Move the wall or opening at one end by {} so the model reads {}.",
                            ft_in((got - d.value_ft).abs()),
                            ft_in(d.value_ft)
                        ),
                    ));
                }
            }
            _ => out.push(failure(
                Check::Dimensions,
                "dimension_end_missing",
                None,
                &d.level,
                Some(mid),
                format!("a wall face or opening at each end of {} ({})", ft_in(d.value_ft), d.text),
                "nothing in the model at one end".into(),
                src,
                Severity::High,
                "Build the wall or opening the dimension ends at.".into(),
            )),
        }
    }
    cx.count(Check::Dimensions, n);
    for f in out {
        cx.push(f);
    }
}

/// The mounting height an element should have and where that comes from, by the source order.
fn wanted_height(cx: &Ctx, e: &Element, sym: Option<&Symbol>) -> Option<(f64, String, Source)> {
    let p = cx.prints;
    let mut cands: Vec<(String, f64, String, Source)> = Vec::new();
    if let Some(s) = sym
        && let Some(h) = s.mounting_in.filter(|h| h.is_finite())
    {
        cands.push(("note".into(), h, format!("noted on {}", s.sheet), sym_source(s)));
    }
    if let Some(mark) = &e.mark {
        for sub in p.submittals.iter().filter(|s| same(&s.mark, mark)) {
            if let Some(h) = sub.fields.get("mountingHeight").and_then(|v| parse_inches(v)) {
                cands.push((
                    "submittal".into(),
                    h,
                    format!("approved submittal {}", sub.section),
                    Source { section: Some(sub.section.clone()), ..Source::default() },
                ));
            }
        }
        for row in p.schedule.iter().filter(|r| same(&r.mark, mark)) {
            if let Some(h) = row.mounting_in.filter(|h| h.is_finite()) {
                cands.push((
                    "schedule".into(),
                    h,
                    format!("{} schedule", row.schedule),
                    Source { sheet: Some(row.sheet.clone()), row: Some(format!("{} {}", row.schedule, row.mark)), ..Source::default() },
                ));
            }
        }
    }
    for r in &p.rules.heights {
        let hit = r.mark.as_deref().is_some_and(|m| e.mark.as_deref().is_some_and(|x| same(x, m)))
            || (r.mark.is_none() && r.kind.as_deref().is_some_and(|k| same(k, &e.kind)));
        if hit {
            cands.push((
                "rule".into(),
                r.mounting_in,
                format!("rules file {}", r.source),
                Source { section: Some(r.source.clone()), ..Source::default() },
            ));
        }
    }
    let order = p.order();
    let rank = |s: &str| order.iter().position(|o| o == s).unwrap_or(order.len());
    // Mark-specific rules beat kind rules of the same rank: they come first in `cands` order
    // only by source, so sort stably by rank.
    cands.sort_by_key(|c| rank(&c.0));
    cands.into_iter().next().map(|(_, h, why, src)| (h, why, src))
}

/// `18"`, `1'-6"`, `48 in`, `18` → inches.
pub fn parse_inches(s: &str) -> Option<f64> {
    let t = s.trim().to_ascii_lowercase();
    let t = t.trim_end_matches("aff").trim().trim_end_matches("in").trim().trim_end_matches('"').trim();
    if let Some((f, i)) = t.split_once('\'') {
        let f: f64 = f.trim().parse().ok()?;
        let i = i.trim().trim_start_matches('-').trim().trim_end_matches('"').trim();
        let i: f64 = if i.is_empty() { 0.0 } else { i.parse().ok()? };
        return Some(f * 12.0 + i).filter(|v| v.is_finite());
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn tags(cx: &mut Ctx) {
    let (pairs, _) = pairing(cx);
    let p = cx.prints;
    let m = cx.model;
    let mut out = Vec::new();
    let mut n = 0;
    let sym_of: HashMap<usize, &Symbol> = pairs.iter().enumerate().filter_map(|(si, e)| Some(((*e)?, p.symbols.get(si)?))).collect();
    for (ei, e) in m.elements.iter().enumerate() {
        if !cx.el_in(e) {
            continue;
        }
        n += 1;
        let sym = sym_of.get(&ei).copied();
        // The tag resolves in the catalog or a schedule.
        if let Some(mark) = e.mark.as_deref().filter(|m| !m.is_empty())
            && !cx.known_mark(mark)
        {
            out.push(failure(
                Check::Tags,
                "unknown_type",
                Some(&e.id),
                &e.level,
                Some(e.at),
                format!("type {mark} in a schedule or the type catalog"),
                "no such type".into(),
                Source { sheet: sym.map(|s| s.sheet.clone()), ..Source::default() },
                Severity::High,
                format!("Add type {mark} to the catalog from its schedule, or give the element a scheduled type."),
            ));
        }
        // The type the print shows.
        if let Some(s) = sym
            && let Some(want) = s.mark.as_deref().filter(|m| !m.is_empty())
            && !e.mark.as_deref().is_some_and(|x| same(x, want))
            && e.status != Some(Status::FieldVerified)
            && e.status != Some(Status::Override)
        {
            out.push(failure(
                Check::Tags,
                "wrong_type",
                Some(&e.id),
                &e.level,
                Some(e.at),
                format!("type {want}"),
                format!("type {}", e.mark.as_deref().unwrap_or("(none)")),
                sym_source(s),
                Severity::High,
                format!("Set its type to {want} as {} shows.", s.sheet),
            ));
        }
        if let Some(s) = sym
            && let Some(want) = s.tag.as_deref()
            && !e.tag.as_deref().is_some_and(|t| same(t, want))
        {
            out.push(failure(
                Check::Tags,
                "wrong_tag",
                Some(&e.id),
                &e.level,
                Some(e.at),
                format!("tag {want}"),
                format!("tag {}", e.tag.as_deref().unwrap_or("(none)")),
                sym_source(s),
                Severity::Medium,
                format!("Tag it {want}."),
            ));
        }
        // Approved submittals.
        if let Some(mark) = &e.mark {
            for sub in p.submittals.iter().filter(|s| same(&s.mark, mark)) {
                for (k, v) in &sub.fields {
                    if k == "mountingHeight" {
                        continue;
                    }
                    let got = e.fields.get(k);
                    if !got.is_some_and(|g| same(g, v)) {
                        out.push(failure(
                            Check::Tags,
                            "not_as_submitted",
                            Some(&e.id),
                            &e.level,
                            Some(e.at),
                            format!("{k} {v} (approved submittal)"),
                            format!("{k} {}", got.map_or("(none)", String::as_str)),
                            Source { section: Some(sub.section.clone()), row: Some(format!("submittal {mark}")), ..Source::default() },
                            Severity::Medium,
                            format!("Set type {mark}'s {k} to {v} as approved."),
                        ));
                    }
                }
            }
        }
        // Mounting height by the source order (field-verified heights stand).
        if e.status != Some(Status::FieldVerified)
            && let Some(z) = e.z_in
            && let Some((want, why, src)) = wanted_height(cx, e, sym)
            && (z - want).abs() > cx.tol.height_in
        {
            out.push(failure(
                Check::Tags,
                "wrong_height",
                Some(&e.id),
                &e.level,
                Some(e.at),
                format!("{} AFF ({why})", inches(want)),
                format!("{} AFF", inches(z)),
                src,
                Severity::Medium,
                format!("Set its mounting height to {} AFF.", inches(want)),
            ));
        }
    }
    // Every symbol's mark resolves too.
    for s in p.symbols.iter().filter(|s| cx.sym_in(s)) {
        if let Some(mark) = s.mark.as_deref().filter(|m| !m.is_empty()) {
            n += 1;
            if !cx.known_mark(mark) {
                out.push(failure(
                    Check::Tags,
                    "tag_not_scheduled",
                    None,
                    &s.level,
                    Some(s.at),
                    format!("type {mark} in a schedule"),
                    "the print's tag has no schedule row or catalog type".into(),
                    sym_source(s),
                    Severity::Low,
                    format!("Raise an RFI: {} tags a {} {mark} that no schedule lists.", s.sheet, s.kind.replace('_', " ")),
                ));
            }
        }
    }
    cx.count(Check::Tags, n);
    for f in out {
        cx.push(f);
    }
}

/// What run ends may land on: elements whose kind takes a raceway, duct or pipe.
fn lands_on(kind: &str) -> bool {
    !matches!(mounting_of(kind), Mounting::Other | Mounting::Unknown) || kind.contains("box") || kind.contains("equipment") || kind.contains("panel")
}

fn connectivity(cx: &mut Ctx) {
    let m = cx.model;
    let reach = cx.tol.connect_in / 12.0;
    let near = (reach * 12.0).max(1.0);
    let mut out = Vec::new();
    let mut n = 0;
    let runs: Vec<&Run> = m.runs.iter().filter(|r| r.path.len() >= 2).collect();
    let mut seg_grid = Grid::new(4.0);
    for (ri, r) in runs.iter().enumerate() {
        for p in r.path.iter() {
            seg_grid.put(&r.level, *p, ri);
        }
        // Long segments: their midpoints too, so tees along them are found.
        for w in r.path.windows(2) {
            if let [a, b] = w {
                let l = dist(*a, *b);
                let steps = ((l / 4.0).ceil() as usize).min(1_000);
                for k in 1..steps {
                    let t = k as f64 / steps as f64;
                    seg_grid.put(&r.level, [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t], ri);
                }
            }
        }
    }
    let mut el_grid = Grid::new(4.0);
    for (i, e) in m.elements.iter().enumerate() {
        if lands_on(&e.kind) {
            el_grid.put(&e.level, e.at, i);
        }
    }
    // The element each end lands on, and gaps.
    let mut ends: Vec<[Option<usize>; 2]> = vec![[None, None]; runs.len()];
    let mut touches_run = vec![[false, false]; runs.len()];
    // The runs each run's far end touches (a shared homerun: the next run home).
    let mut touch: Vec<Vec<usize>> = vec![Vec::new(); runs.len()];
    let mut gaps: Vec<(usize, usize, [f64; 2], f64)> = Vec::new();
    // Element landing: the end within half a box (6") plus the tolerance of the element's point.
    let land = reach + 0.5;
    for (ri, r) in runs.iter().enumerate() {
        for (k, p) in [r.path.first(), r.path.last()].into_iter().enumerate() {
            let Some(p) = p.copied() else { continue };
            let mut best: Option<(f64, usize)> = None;
            for ei in el_grid.near(&r.level, p, land.max(1.0)) {
                if let Some(e) = m.elements.get(ei).filter(|e| same(&e.level, &r.level)) {
                    let d = dist(e.at, p);
                    if d <= land && best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, ei));
                    }
                }
            }
            if let Some(slot) = ends.get_mut(ri).and_then(|x| x.get_mut(k)) {
                *slot = best.map(|(_, i)| i);
            }
            for rj in seg_grid.near(&r.level, p, near) {
                if rj == ri {
                    continue;
                }
                let Some(o) = runs.get(rj) else { continue };
                if !same(&o.level, &r.level) {
                    continue;
                }
                let d = o.path.windows(2).filter_map(|w| if let [a, b] = w { Some(seg_dist(p, *a, *b).0) } else { None }).fold(f64::MAX, f64::min);
                if d <= reach {
                    if let Some(t) = touches_run.get_mut(ri).and_then(|x| x.get_mut(k)) {
                        *t = true;
                    }
                    if k == 1
                        && let Some(v) = touch.get_mut(ri)
                    {
                        v.push(rj);
                    }
                } else if d <= near {
                    gaps.push((ri, k, p, d));
                }
            }
        }
    }
    for (ri, r) in runs.iter().enumerate() {
        let in_scope = cx.reach.id(&r.id) || r.path.iter().any(|p| cx.reach.at(&r.level, *p));
        if !in_scope {
            continue;
        }
        n += 1;
        let src = Source { sheet: r.source.clone(), ..Source::default() };
        let what = r.mark.as_deref().map_or_else(|| r.kind.clone(), |mk| format!("{} {mk}", r.kind));
        for k in 0..2 {
            let landed = ends.get(ri).and_then(|x| x.get(k)).copied().flatten().is_some();
            let teed = touches_run.get(ri).and_then(|x| x.get(k)).copied().unwrap_or(false);
            let p = if k == 0 { r.path.first() } else { r.path.last() };
            let Some(p) = p.copied() else { continue };
            if landed || teed {
                continue;
            }
            if let Some((_, _, _, d)) = gaps.iter().find(|g| g.0 == ri && g.1 == k) {
                out.push(failure(
                    Check::Connectivity,
                    "gap",
                    Some(&r.id),
                    &r.level,
                    Some(p),
                    format!("the {what} meeting the next run"),
                    format!("a {} gap at {}", inches(d * 12.0), pt(p)),
                    src.clone(),
                    Severity::Medium,
                    format!("Close the {} gap.", inches(d * 12.0)),
                ));
            } else {
                out.push(failure(
                    Check::Connectivity,
                    "dangling",
                    Some(&r.id),
                    &r.level,
                    Some(p),
                    format!("the {what} ending at a box, equipment or another run"),
                    format!("an open end at {}", pt(p)),
                    src.clone(),
                    Severity::High,
                    "Extend it to the box or equipment it serves, or remove the stub.".into(),
                ));
            }
        }
        if let Some(panel) = r.homerun.as_deref()
            && !homerun_reaches(&runs, &ends, &touch, ri, panel, m)
        {
            out.push(failure(
                Check::Connectivity,
                "homerun_not_at_panel",
                Some(&r.id),
                &r.level,
                r.path.last().copied(),
                format!("the homerun ending at panel {panel}"),
                "it doesn't reach the panel".into(),
                src.clone(),
                Severity::High,
                format!("Run the homerun to panel {panel}."),
            ));
        }
    }
    cx.count(Check::Connectivity, n);
    for f in out {
        cx.push(f);
    }
}

/// Whether a homerun gets to its panel: its far end lands on the panel, or on a run homing to
/// the same panel that does (followed run by run).
fn homerun_reaches(runs: &[&Run], ends: &[[Option<usize>; 2]], touch: &[Vec<usize>], start: usize, panel: &str, m: &Model) -> bool {
    let mut seen = BTreeSet::new();
    let mut todo = vec![start];
    while let Some(ri) = todo.pop() {
        if !seen.insert(ri) || seen.len() > 10_000 {
            continue;
        }
        let at_panel = ends
            .get(ri)
            .and_then(|e| e.get(1))
            .copied()
            .flatten()
            .and_then(|ei| m.elements.get(ei))
            .is_some_and(|e| e.tag.as_deref().is_some_and(|t| same(t, panel)));
        if at_panel {
            return true;
        }
        for rj in touch.get(ri).into_iter().flatten() {
            if runs.get(*rj).is_some_and(|r| r.homerun.as_deref().is_some_and(|h| same(h, panel))) {
                todo.push(*rj);
            }
        }
    }
    false
}

fn physical(cx: &mut Ctx) {
    let m = cx.model;
    let mut out = Vec::new();
    let mut n = 0;
    let mut wall_grid = Grid::new(8.0);
    for (i, w) in m.walls.iter().enumerate() {
        let l = dist(w.a, w.b);
        let steps = ((l / 4.0).ceil() as usize).clamp(1, 2_000);
        for k in 0..=steps {
            let t = k as f64 / steps as f64;
            wall_grid.put(&w.level, [w.a[0] + (w.b[0] - w.a[0]) * t, w.a[1] + (w.b[1] - w.a[1]) * t], i);
        }
    }
    let mut el_grid = Grid::new(2.0);
    for (i, e) in m.elements.iter().enumerate() {
        el_grid.put(&e.level, e.at, i);
    }
    let tol = cx.tol.place_in / 12.0;
    for (ei, e) in m.elements.iter().enumerate() {
        if !cx.el_in(e) {
            continue;
        }
        n += 1;
        let mounting = if e.mounting == Mounting::Unknown { mounting_of(&e.kind) } else { e.mounting };
        // On a wall: inside its thickness, on a face.
        if mounting == Mounting::Wall {
            let host = e.wall.as_deref().and_then(|id| m.walls.iter().find(|w| w.id == id));
            let near_w = host.or_else(|| {
                let mut ids = wall_grid.near(&e.level, e.at, 8.0);
                ids.sort_unstable();
                ids.dedup();
                ids.into_iter()
                    .filter_map(|i| m.walls.get(i))
                    .filter(|w| same(&w.level, &e.level))
                    .min_by(|a, b| seg_dist(e.at, a.a, a.b).0.total_cmp(&seg_dist(e.at, b.a, b.b).0))
            });
            match near_w {
                None => out.push(failure(
                    Check::Physical,
                    "not_in_wall",
                    Some(&e.id),
                    &e.level,
                    Some(e.at),
                    "a wall it is mounted on".into(),
                    "no wall there".into(),
                    Source::default(),
                    Severity::High,
                    "Move it onto the wall face it belongs on.".into(),
                )),
                Some(w) => {
                    let (d, t) = seg_dist(e.at, w.a, w.b);
                    let half = w.thickness_in / 24.0;
                    let off_face = (d - half).abs();
                    let along_ok = t > 0.0 && t < 1.0 || d <= half + tol;
                    if d > half + tol || !along_ok {
                        out.push(failure(
                            Check::Physical,
                            "off_wall",
                            Some(&e.id),
                            &e.level,
                            Some(e.at),
                            format!("on a face of wall {} ({} thick)", w.id, inches(w.thickness_in)),
                            format!("{} off its face", ft_in(off_face)),
                            Source::default(),
                            Severity::High,
                            format!("Move it {} onto the wall face.", ft_in(off_face)),
                        ));
                    } else if off_face > tol && d < half - tol {
                        out.push(failure(
                            Check::Physical,
                            "buried_in_wall",
                            Some(&e.id),
                            &e.level,
                            Some(e.at),
                            "on a face of the wall".into(),
                            format!("{} inside the wall", inches(off_face * 12.0)),
                            Source::default(),
                            Severity::Medium,
                            "Move it out to the wall face.".into(),
                        ));
                    }
                    if let (Some(depth), Some(stud)) = (e.box_depth_in, w.stud_in)
                        && depth > stud + 1e-6
                    {
                        out.push(failure(
                            Check::Physical,
                            "box_too_deep",
                            Some(&e.id),
                            &e.level,
                            Some(e.at),
                            format!("a box at most {} deep ({} studs)", inches(stud), inches(stud)),
                            format!("a {} deep box", inches(depth)),
                            Source::default(),
                            Severity::High,
                            format!("Use a box at most {} deep, or a deeper wall type.", inches(stud)),
                        ));
                    }
                }
            }
            // Not in a door or opening.
            for o in m.openings.iter().filter(|o| same(&o.level, &e.level)) {
                let lo = [o.min[0].min(o.max[0]) - 0.01, o.min[1].min(o.max[1]) - 0.01];
                let hi = [o.min[0].max(o.max[0]) + 0.01, o.min[1].max(o.max[1]) + 0.01];
                let in_open = e.at[0] > lo[0] && e.at[0] < hi[0] && e.at[1] > lo[1] && e.at[1] < hi[1];
                let in_swing = o.swing.is_some_and(|(h, r)| r.is_finite() && dist(h, e.at) < r - tol && e.z_in.is_none_or(|z| z < 84.0));
                if in_open || in_swing {
                    out.push(failure(
                        Check::Physical,
                        if in_open { "in_opening" } else { "in_door_swing" },
                        Some(&e.id),
                        &e.level,
                        Some(e.at),
                        "clear of doors and openings".into(),
                        format!("in {} {}", o.kind, o.id),
                        Source::default(),
                        Severity::High,
                        "Move it along the wall clear of the door.".into(),
                    ));
                    break;
                }
            }
        }
        // At the ceiling.
        if mounting == Mounting::Ceiling
            && let Some(z) = e.z_in
            && let Some(ceil) = cx.room_of(&e.level, e.at).and_then(|r| r.ceiling_ft)
            && (z - ceil * 12.0).abs() > cx.tol.height_in
        {
            out.push(failure(
                Check::Physical,
                if z < ceil * 12.0 { "below_ceiling" } else { "above_ceiling" },
                Some(&e.id),
                &e.level,
                Some(e.at),
                format!("in the ceiling at {} AFF", ft_in(ceil)),
                format!("at {} AFF", ft_in(z / 12.0)),
                Source { sheet: cx.room_of(&e.level, e.at).map(|r| r.sheet.clone()).filter(|s| !s.is_empty()), ..Source::default() },
                Severity::Medium,
                format!("Set it in the ceiling at {} AFF.", ft_in(ceil)),
            ));
        }
        // Floating or buried.
        if let Some(z) = e.z_in {
            let top = cx.level_height(&e.level);
            if z < -cx.tol.height_in {
                out.push(failure(
                    Check::Physical,
                    "below_floor",
                    Some(&e.id),
                    &e.level,
                    Some(e.at),
                    "at or above the floor".into(),
                    format!("{} below the floor", inches(-z)),
                    Source::default(),
                    Severity::High,
                    "Raise it to its mounting height.".into(),
                ));
            } else if let Some(h) = top
                && z > h * 12.0 + cx.tol.height_in
            {
                out.push(failure(
                    Check::Physical,
                    "floating",
                    Some(&e.id),
                    &e.level,
                    Some(e.at),
                    format!("below the next floor ({} up)", ft_in(h)),
                    format!("at {} AFF", ft_in(z / 12.0)),
                    Source::default(),
                    Severity::High,
                    "Lower it to its mounting height.".into(),
                ));
            } else if mounting == Mounting::Floor && z > cx.tol.height_in.max(6.0) {
                out.push(failure(
                    Check::Physical,
                    "floating",
                    Some(&e.id),
                    &e.level,
                    Some(e.at),
                    "on the floor (or a housekeeping pad)".into(),
                    format!("at {} AFF", ft_in(z / 12.0)),
                    Source::default(),
                    Severity::Medium,
                    "Set it on the floor.".into(),
                ));
            }
        }
        // Doubled.
        let dup = el_grid.near(&e.level, e.at, 0.1).into_iter().filter(|j| *j < ei).filter_map(|j| m.elements.get(j)).find(|o| {
            same(&o.level, &e.level)
                && same(&o.kind, &e.kind)
                && same_opt(o.mark.as_deref(), e.mark.as_deref())
                && dist(o.at, e.at) < 1.0 / 12.0
                && match (o.z_in, e.z_in) {
                    (Some(a), Some(b)) => (a - b).abs() < 1.0,
                    _ => true,
                }
        });
        if let Some(o) = dup {
            out.push(failure(
                Check::Physical,
                "duplicate",
                Some(&e.id),
                &e.level,
                Some(e.at),
                "one element here".into(),
                format!("two: this and {}", o.id),
                Source::default(),
                Severity::High,
                format!("Delete one of the two (keep {}).", o.id),
            ));
        }
    }
    cx.count(Check::Physical, n);
    for f in out {
        cx.push(f);
    }
}

fn provenance(cx: &mut Ctx) {
    let m = cx.model;
    let mut out = Vec::new();
    let mut n = 0;
    let lists = m.elements.iter().map(|e| (&e.id, &e.kind, &e.level, Some(e.at), e.status, &e.source, cx.el_in(e))).chain(m.runs.iter().map(|r| {
        (
            &r.id,
            &r.kind,
            &r.level,
            r.path.first().copied(),
            r.status,
            &r.source,
            cx.reach.id(&r.id) || r.path.iter().any(|p| cx.reach.at(&r.level, *p)),
        )
    }));
    for (id, kind, level, at, status, source, in_scope) in lists {
        if !in_scope {
            continue;
        }
        n += 1;
        let traced = source.as_deref().is_some_and(|s| !s.trim().is_empty());
        let (rule, found) = match (status, traced) {
            (None, false) => ("untraced", "no status and no source".to_string()),
            (None, true) => ("no_status", "a source but no status".to_string()),
            (Some(s), false) if s != Status::Agent && s != Status::Rule => ("no_source", format!("{} with no source", s.name())),
            _ => continue,
        };
        out.push(failure(
            Check::Provenance,
            rule,
            Some(id),
            level,
            at,
            "a status (fixed, rule, zone, field-verified, override or agent) and its source".into(),
            found,
            Source::default(),
            if rule == "untraced" { Severity::Medium } else { Severity::Low },
            format!("Record where this {} comes from (bim.source).", kind.replace('_', " ")),
        ));
    }
    cx.count(Check::Provenance, n);
    for f in out {
        cx.push(f);
    }
}

fn spec(cx: &mut Ctx) {
    let p = cx.prints;
    let m = cx.model;
    let mut out = Vec::new();
    let mut n = 0;
    let scheduled = |mark: &str| p.schedule.iter().any(|r| same(&r.mark, mark));
    let has_schedule = !p.schedule.is_empty();
    struct Item<'a> {
        id: &'a str,
        kind: &'a str,
        level: &'a str,
        at: Option<[f64; 2]>,
        mark: Option<&'a str>,
        fields: &'a BTreeMap<String, String>,
        outdoors: bool,
    }
    let mut items: Vec<Item> = Vec::new();
    for e in m.elements.iter().filter(|e| cx.el_in(e)) {
        let outdoors = cx.room_of(&e.level, e.at).is_some_and(|r| r.exterior) || e.fields.get("location").is_some_and(|l| says(l, "outdoors"));
        items.push(Item { id: &e.id, kind: &e.kind, level: &e.level, at: Some(e.at), mark: e.mark.as_deref(), fields: &e.fields, outdoors });
    }
    for r in m.runs.iter().filter(|r| cx.reach.id(&r.id) || r.path.iter().any(|q| cx.reach.at(&r.level, *q))) {
        let outdoors = r.path.iter().any(|q| cx.room_of(&r.level, *q).is_some_and(|x| x.exterior));
        items.push(Item {
            id: &r.id,
            kind: &r.kind,
            level: &r.level,
            at: r.path.first().copied(),
            mark: r.mark.as_deref(),
            fields: &r.fields,
            outdoors,
        });
    }
    for it in &items {
        n += 1;
        let category = it.fields.get("category").map(String::as_str).unwrap_or("");
        for rule in &p.rules.spec {
            let applies = rule.kind.as_deref().is_none_or(|k| same(k, it.kind))
                && rule.category.as_deref().is_none_or(|c| same(c, category) || same(c, it.kind))
                && (!rule.outdoors || it.outdoors)
                && (rule.kind.is_some() || rule.category.is_some() || rule.outdoors);
            if !applies {
                continue;
            }
            let v = it.fields.get(&rule.field).map(String::as_str).unwrap_or("");
            let bad_forbidden = rule.forbidden.iter().any(|f| says(v, f));
            let bad_allowed = !rule.allowed.is_empty() && !rule.allowed.iter().any(|a| says(v, a));
            if bad_forbidden || bad_allowed {
                let want = if rule.allowed.is_empty() {
                    format!("{} not {}", rule.field, rule.forbidden.join("/"))
                } else {
                    format!("{} {}", rule.field, rule.allowed.join(" or "))
                };
                out.push(failure(
                    Check::Spec,
                    "spec_violation",
                    Some(it.id),
                    it.level,
                    it.at,
                    if rule.text.is_empty() { want.clone() } else { format!("{want} ({})", rule.text) },
                    format!("{} {}", rule.field, if v.is_empty() { "(not given)" } else { v }),
                    Source { section: Some(rule.section.clone()), ..Source::default() },
                    Severity::High,
                    match it.mark {
                        Some(mk) => format!("Change type {mk} to one with {want} per spec {}.", rule.section),
                        None => format!("Give it a type with {want} per spec {}.", rule.section),
                    },
                ));
            }
        }
        if has_schedule
            && let Some(mk) = it.mark.filter(|m| !m.is_empty())
            && !scheduled(mk)
            && m.types.keys().any(|k| same(k, mk))
        {
            out.push(failure(
                Check::Spec,
                "type_not_scheduled",
                Some(it.id),
                it.level,
                it.at,
                format!("type {mk} with a schedule row"),
                "a catalog type no schedule lists".into(),
                Source::default(),
                Severity::Low,
                format!("Use a scheduled type, or raise an RFI to add {mk} to the schedule."),
            ));
        }
    }
    cx.count(Check::Spec, n);
    for f in out {
        cx.push(f);
    }
}

fn termination(cx: &mut Ctx) {
    let p = cx.prints;
    let m = cx.model;
    let land = cx.tol.connect_in / 12.0 + 0.5;
    let mut out = Vec::new();
    let mut n = 0;
    for t in &p.terminations {
        let Some(eq) = m.elements.iter().find(|e| e.tag.as_deref().is_some_and(|x| same(x, &t.tag))) else { continue };
        // Runs with conductors landing at it: an end at it, or a homerun to it.
        for r in m.runs.iter() {
            let Some(c) = &r.conductors else { continue };
            let at_end = same(&r.level, &eq.level) && [r.path.first(), r.path.last()].into_iter().flatten().any(|q| dist(*q, eq.at) <= land);
            let to_it = r.homerun.as_deref().is_some_and(|h| same(h, &t.tag));
            if !(at_end || to_it) {
                continue;
            }
            if !(cx.el_in(eq) || cx.reach.id(&r.id) || r.path.iter().any(|q| cx.reach.at(&r.level, *q))) {
                continue;
            }
            n += 1;
            let mut why: Vec<String> = Vec::new();
            if t.lugs.per_phase > 0 && c.per_phase > t.lugs.per_phase {
                why.push(format!("{} conductors per phase on lugs for {}", c.per_phase, t.lugs.per_phase));
            }
            let (size, lo, hi) = (wire_cmil(&c.size), wire_cmil(&t.lugs.min), wire_cmil(&t.lugs.max));
            match size {
                Some(s) => {
                    if lo.is_some_and(|l| s < l * 0.999) || hi.is_some_and(|h| s > h * 1.001) {
                        why.push(format!("{} outside the lugs' {}–{}", c.size, t.lugs.min, t.lugs.max));
                    }
                }
                None if !c.size.is_empty() => why.push(format!("wire size {} not understood", c.size)),
                None => {}
            }
            let lug = t.lugs.material.to_ascii_uppercase();
            let mat = c.material.to_ascii_uppercase();
            if !lug.is_empty() && !mat.is_empty() && !lug.contains("AL/CU") && !lug.contains("CU/AL") && !lug.contains(mat.trim()) {
                why.push(format!("{} conductors on {} lugs", mat.trim(), lug.trim()));
            }
            if !why.is_empty() {
                out.push(failure(
                    Check::Termination,
                    "lug_mismatch",
                    Some(&r.id),
                    &eq.level,
                    Some(eq.at),
                    format!(
                        "{}: ≤{} per phase, {}–{}, {}",
                        t.tag,
                        t.lugs.per_phase,
                        t.lugs.min,
                        t.lugs.max,
                        if t.lugs.material.is_empty() { "any metal" } else { &t.lugs.material }
                    ),
                    format!("{} × {} {} per phase", c.per_phase, c.size, c.material),
                    Source { sheet: Some(t.sheet.clone()), row: Some(format!("{} lugs", t.tag)), ..Source::default() },
                    Severity::High,
                    format!("Resize the feeder or order {} with lugs for it: {}.", t.tag, why.join("; ")),
                ));
            }
        }
    }
    cx.count(Check::Termination, n);
    for f in out {
        cx.push(f);
    }
}

/// Failures in `after` not in `before` (by id), and how many of `before` are gone.
pub fn new_failures<'a>(before: &[Failure], after: &'a [Failure]) -> (Vec<&'a Failure>, usize) {
    let b: BTreeSet<&str> = before.iter().map(|f| f.id.as_str()).collect();
    let a: BTreeSet<&str> = after.iter().map(|f| f.id.as_str()).collect();
    (after.iter().filter(|f| !b.contains(f.id.as_str())).collect(), b.iter().filter(|id| !a.contains(*id)).count())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn el(id: &str, kind: &str, mark: &str, at: [f64; 2], z: f64) -> Element {
        Element {
            id: id.into(),
            kind: kind.into(),
            mark: Some(mark.into()),
            level: "1".into(),
            at,
            z_in: Some(z),
            status: Some(Status::Fixed),
            source: Some("E-101".into()),
            ..Element::default()
        }
    }

    fn sym(kind: &str, mark: &str, at: [f64; 2]) -> Symbol {
        Symbol { sheet: "E-101".into(), level: "1".into(), kind: kind.into(), mark: Some(mark.into()), at, ..Symbol::default() }
    }

    fn base() -> (Prints, Model) {
        let mut p = Prints::default();
        p.symbols.push(sym("receptacle", "A", [0.0, 0.25]));
        p.schedule.push(ScheduleRow { sheet: "E-601".into(), schedule: "device".into(), mark: "A".into(), ..ScheduleRow::default() });
        p.rules.heights.push(HeightRule { kind: Some("receptacle".into()), mounting_in: 18.0, source: "rules 1".into(), ..HeightRule::default() });
        let mut m = Model::default();
        m.levels.push(ModelLevel { name: "1".into(), height_ft: 12.0 });
        m.walls.push(ModelWall {
            id: "W1".into(),
            level: "1".into(),
            a: [-10.0, 0.0],
            b: [10.0, 0.0],
            thickness_in: 6.0,
            stud_in: Some(3.5),
            wall_type: None,
        });
        m.elements.push(el("E1", "receptacle", "A", [0.0, 0.25], 18.0));
        (p, m)
    }

    #[test]
    fn a_true_model_passes() {
        let (p, m) = base();
        let r = validate(&p, &m, &Scope::all());
        assert!(r.pass, "{:#?}", r.failures);
        assert_eq!(r.score, 100.0);
    }

    #[test]
    fn heights_follow_the_source_order() {
        let (mut p, mut m) = base();
        if let Some(s) = p.symbols.first_mut() {
            s.mounting_in = Some(24.0);
        }
        // A note beside the symbol beats the rules file.
        let r = validate(&p, &m, &Scope::all());
        assert!(r.failures.iter().any(|f| f.rule == "wrong_height" && f.expected.starts_with("24")), "{:#?}", r.failures);
        if let Some(e) = m.elements.first_mut() {
            e.z_in = Some(24.0);
        }
        assert!(validate(&p, &m, &Scope::all()).pass);
        // …unless the rules file comes first.
        p.rules.source_order = vec!["rule".into(), "note".into()];
        assert!(validate(&p, &m, &Scope::all()).failures.iter().any(|f| f.rule == "wrong_height"));
    }

    #[test]
    fn wire_sizes_order() {
        let s = |x: &str| wire_cmil(x).unwrap_or(f64::NAN);
        assert!(s("#12") < s("#10"));
        assert!(s("#1") < s("1/0") && s("1/0") < s("4/0") && s("4/0") < s("250 kcmil"));
        assert!((s("4/0") - 211_600.0).abs() < 1_000.0);
        assert!(wire_cmil("big").is_none() && wire_cmil("#99").is_none());
    }

    #[test]
    fn ft_in_prints() {
        assert_eq!(ft_in(12.5), "12'-6\"");
        assert_eq!(ft_in(0.25), "0'-3\"");
        assert_eq!(ft_in(1.0 + 1.5 / 12.0), "1'-1 1/2\"");
        assert_eq!(parse_inches("1'-6\""), Some(18.0));
        assert_eq!(parse_inches("48\" AFF"), Some(48.0));
    }

    #[test]
    fn hostile_inputs_never_panic() {
        let mut p = Prints::default();
        p.symbols.push(Symbol { at: [f64::NAN, 1.0], ..Symbol::default() });
        p.dimensions.push(DimString { a: [0.0, 0.0], b: [0.0, 0.0], value_ft: 1.0, ..DimString::default() });
        p.rules.tolerances.search_in = f64::INFINITY;
        p.rooms.push(PrintRoom { outline: vec![[0.0, 0.0]], ..PrintRoom::default() });
        let p = p.capped();
        let mut m = Model::default();
        m.elements.push(Element { at: [1e300, -1e300], z_in: Some(f64::MAX), ..Element::default() });
        m.walls.push(ModelWall { a: [0.0, 0.0], b: [0.0, 0.0], thickness_in: -5.0, ..ModelWall::default() });
        m.runs.push(Run { path: vec![[0.0, 0.0]], ..Run::default() });
        m.runs.push(Run {
            path: vec![[0.0, 0.0], [1e300, 0.0]],
            conductors: Some(Conductors { size: "?".into(), ..Conductors::default() }),
            ..Run::default()
        });
        p.terminations.iter().for_each(|_| {});
        let m = m.capped();
        let _ = validate(&p, &m, &Scope::all());
        let _ = validate(&p, &m, &Scope { areas: vec![("".into(), [[f64::NAN, 0.0], [1.0, f64::INFINITY]])], ..Scope::default() });
        assert!(Prints::from_json("{\"version\": 99}").is_err());
        assert!(Prints::from_json("\"x\"").is_err());
    }
}
