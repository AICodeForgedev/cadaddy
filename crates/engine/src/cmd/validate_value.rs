//! G-VE-TIER, G-VE-AREA, G-VE-COST, G-VE-RFI and G-VE-DESIGN (Q6155): Validate checks the
//! value-engineering recommendations ([`super::valueeng`], Q6154) against the documents, the code
//! rules, the model's own pieces and the price book, never by asking the study again.
//!
//! What is checked is the study's answer as the job kept it (the BIM stage's
//! `value_engineering`, or a `ve.study` answer given as a file) and the scenarios the drawing
//! keeps (their pieces, the design's pieces each replaces, their RFI and decision).
//!
//! - **G-VE-TIER**: an ALLOWED NOW option is permitted by BOTH the specification's paragraph for
//!   every area its pieces are in (the wiring-methods table as the job read it lists the method)
//!   and a code rule it cites (rules data: the construction types it allows, concealed only, a
//!   thermal barrier, the areas it never allows), with every condition met everywhere it goes.
//!   An option the specification contradicts in any of its areas must be NEEDS RFI; a NEEDS RFI
//!   option still needs a code rule it cites that permits it.
//! - **G-VE-AREA**: no NM or other cable option where its method is never used (rules data:
//!   NM never in common, exposed, plenum or below-slab areas): each of the scenario's cable
//!   pieces placed by the room it lies in, the room of the design piece it runs beside, its
//!   circuit's area, its ceiling and its height against the lowest floor.
//! - **G-VE-COST**: the savings recomputed here: the study's quantities against a recount of the
//!   model's pieces (the design's pieces a scenario replaces, the scenario's own), every line
//!   priced again from the price book (material and labor per unit, labor in dollars), the
//!   saving's material, labor dollars and cost within `cost_pct` (1%), and its man-hours = labor
//!   $ ÷ the stated manpower rate.
//! - **G-VE-RFI**: every NEEDS RFI option has its RFI on the RFI list (a review item of kind
//!   `rfi`, method choice) whose text gives each code citation of the option and states the
//!   design's intent.
//! - **G-VE-DESIGN**: the as-designed model is unchanged by any scenario not approved: the
//!   design's pieces it replaces are all there, still design pieces and shown (unless the
//!   scenario is being shown), its own pieces carry no takeoff kind and stay on their hidden
//!   layer, its circuits keep their designed method; a scenario that became the design was
//!   approved by its RFI (NEEDS RFI) or chosen (ALLOWED NOW).
//!
//! Anything the documents or the model don't give (no study kept, no price book, a ceiling the
//! documents don't type, a construction type not read) is `unsure`, never a pass.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cadaddy_doc::props::{self, Prop};
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, RfiKind};
use serde::Deserialize;
use serde_json::{Value, json};

use super::validate::Frame;
use super::validate_packet::{Area, inside, point_in, pt, r2};
use super::validate_rules::Effective;

pub const TIER: &str = "G-VE-TIER";
pub const AREA: &str = "G-VE-AREA";
pub const COST: &str = "G-VE-COST";
pub const RFI: &str = "G-VE-RFI";
pub const DESIGN: &str = "G-VE-DESIGN";
/// Every rule of this module.
pub const ALL: [&str; 5] = [TIER, AREA, COST, RFI, DESIGN];
/// The verdicts these rules give.
pub const VERDICTS: [&str; 3] = ["pass", "fail", "unsure"];

/// The rules data.
const DATA: &str = include_str!("validate_value.json");
/// A study file read, at most (bytes).
const MAX_STUDY: usize = 64 << 20;
/// Scenarios looked at, at most; lines one side prices, at most.
const MAX_SCENARIOS: usize = 200;
const MAX_LINES: usize = 20_000;
/// Pieces of one scenario looked at, at most.
const MAX_PIECES: usize = 400_000;
/// Rooms named in a finding, at most.
const MAX_NAMED: usize = 12;
/// A plan-grid bucket for the design pieces' rooms, feet; how far a scenario piece takes the
/// room of the design piece it runs beside, feet.
const CELL_FT: f64 = 8.0;
const BESIDE_FT: f64 = 3.0;
/// A scenario piece is placed at points this far apart along its edges, feet; at most this many.
const SAMPLE_FT: f64 = 2.0;
const MAX_SAMPLES: usize = 400;
/// Points of a piece an area needs to hold it (each about [`SAMPLE_FT`]), unless all are there.
const MIN_POINTS: usize = 2;
/// The flags a point may carry (not areas).
const FLAG_NAMES: [&str; 6] = ["corridor", "plenum", "below_slab", "suspended", "open_ceiling", "ceiling_unknown"];
/// What marks a homerun's own circuit tag (`circuit:<level>:home:<group>`), the router's.
const HOME_PART: &str = ":home:";
/// A cost or quantity off by more than this share fails (percent).
const DEFAULT_COST_PCT: f64 = 1.0;
/// Differences under these are rounding (dollars, man-hours, feet or each).
const ABS_DOLLARS: f64 = 1.0;
const ABS_HOURS: f64 = 0.05;
const ABS_QTY: f64 = 0.05;

// ---------------------------------------------------------------- rules data

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Data {
    cables: BTreeMap<String, String>,
    never: BTreeMap<String, BTreeMap<String, String>>,
    words: Words,
    code: Vec<CodeRule>,
    same_either_way: Vec<String>,
    rfi_no_intent: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Words {
    corridor: Vec<String>,
    plenum: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct CodeRule {
    cite: String,
    methods: Vec<String>,
    construction: Vec<String>,
    concealed: bool,
    thermal_barrier: bool,
    not_in: Vec<String>,
    says: String,
}

fn data() -> Data {
    serde_json::from_str(DATA).unwrap_or_default()
}

// ---------------------------------------------------------------- inputs

/// One row of the price book: material and labor (dollars) per unit.
#[derive(Clone, Debug, Default)]
pub(crate) struct BookRow {
    item: String,
    size: String,
    material: f64,
    labor_dollars: f64,
}

/// The price book the savings are recomputed from.
#[derive(Clone, Debug, Default)]
pub(crate) struct Book {
    rows: Vec<BookRow>,
    manpower_rate: Option<f64>,
    path: String,
}

/// An item or size for matching: lower case, letters, digits, `/` and `-`.
fn norm(s: &str) -> String {
    s.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '/' || *c == '-').collect()
}

impl Book {
    /// The row for an item and size (a row with no size takes any size).
    fn find(&self, item: &str, size: &str) -> Option<&BookRow> {
        let (i, s) = (norm(item), norm(size));
        self.rows
            .iter()
            .find(|r| norm(&r.item) == i && !r.size.is_empty() && norm(&r.size) == s)
            .or_else(|| self.rows.iter().find(|r| norm(&r.item) == i && r.size.is_empty()))
    }
}

/// What the rules check: the study's answer and where it came from, the price book.
#[derive(Clone, Debug, Default)]
pub(crate) struct Inputs {
    pub study: Option<Value>,
    pub study_from: String,
    pub book: Option<Book>,
    pub book_from: String,
}

/// Is any rule of this module on?
pub(crate) fn wanted(rules: &[Effective], only: &[String]) -> bool {
    rules
        .iter()
        .any(|r| r.enabled && ALL.contains(&r.rule.id.as_str()) && (only.is_empty() || only.iter().any(|o| o.eq_ignore_ascii_case(&r.rule.id))))
}

/// A study answer from a value: a `ve.study` answer, a job result holding `value_engineering`,
/// or the stage result itself.
fn study_of(v: &Value) -> Option<Value> {
    if v.get("scenarios").is_some_and(Value::is_array) {
        return Some(v.clone());
    }
    let inner = v.get("value_engineering")?;
    inner.get("scenarios").is_some_and(Value::is_array).then(|| inner.clone())
}

fn read_json(path: &str) -> std::result::Result<Value, String> {
    let bytes = crate::readfile::read(std::path::Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
    if bytes.len() > MAX_STUDY {
        return Err(format!("{path} is larger than {} MB", MAX_STUDY >> 20));
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("{path} isn't JSON: {e}"))
}

/// The study (the caller's `value_engineering` file or object, else the job's BIM stage
/// result) and the price book (the caller's `price_book`, else the study's private book, else the
/// job's cost DB).
pub(crate) fn read(s: &crate::Session, p: &Value, rules: &[Effective], only: &[String]) -> std::result::Result<Option<Inputs>, String> {
    if !wanted(rules, only) {
        return Ok(None);
    }
    let mut inp = Inputs::default();
    let job = super::guided::load_job(s).ok().flatten();
    match p.get("value_engineering") {
        Some(Value::String(path)) if !path.trim().is_empty() => {
            let v = read_json(path.trim())?;
            inp.study = Some(study_of(&v).ok_or_else(|| format!("{path} holds no value engineering study (scenarios)"))?);
            inp.study_from = format!("the file {}", path.trim());
        }
        Some(v @ Value::Object(_)) => {
            inp.study = Some(study_of(v).ok_or_else(|| "`value_engineering` holds no study (scenarios)".to_string())?);
            inp.study_from = "given".into();
        }
        None | Some(Value::Null) => {
            if let Some(v) =
                job.as_ref().and_then(|j| j.stage(super::guided::StageName::BimBuild)).and_then(|st| st.result.as_ref()).and_then(study_of)
            {
                inp.study = Some(v);
                inp.study_from = "the job's BIM stage".into();
            }
        }
        Some(_) => return Err("`value_engineering` is a path to a ve.study answer (JSON) or the answer itself".into()),
    }
    let given = p.get("price_book").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(|t| (t.to_string(), "given"));
    let kept = inp.study.as_ref().and_then(|v| v["price_book"]["file"].as_str()).map(|t| (t.to_string(), "the study's price book"));
    let job_db = job.as_ref().and_then(|j| j.cost_db.clone()).filter(|t| !t.trim().is_empty()).map(|t| (t, "the job's cost DB"));
    for (path, from) in [given.clone(), kept, job_db].into_iter().flatten() {
        match super::valueeng::read_cost_db(&path, "validate.prescreen") {
            Ok(db) => {
                inp.book = Some(Book {
                    rows: db
                        .rows
                        .iter()
                        .map(|r| BookRow { item: r.item.clone(), size: r.size.clone(), material: r.material, labor_dollars: r.labor_dollars })
                        .collect(),
                    manpower_rate: db.manpower_rate,
                    path: path.clone(),
                });
                inp.book_from = format!("{from}: {path}");
                break;
            }
            // A book the caller names must read; one kept elsewhere may be on another machine.
            Err(e) if given.as_ref().is_some_and(|g| g.0 == path) => return Err(e.to_string()),
            Err(_) => {}
        }
    }
    Ok(Some(inp))
}

// ---------------------------------------------------------------- the check

pub(crate) struct Ctx<'a> {
    pub d: &'a Drawing,
    pub sp: &'a BuildingSpec,
    pub frame: &'a Frame,
    pub areas: &'a [Area],
    pub whole_levels: bool,
    pub inputs: Option<&'a Inputs>,
}

/// The rules switched on.
#[derive(Default, Clone, Copy)]
pub(crate) struct On<'a> {
    pub tier: Option<&'a Effective>,
    pub area: Option<&'a Effective>,
    pub cost: Option<&'a Effective>,
    pub rfi: Option<&'a Effective>,
    pub design: Option<&'a Effective>,
}

impl<'a> On<'a> {
    pub(crate) fn of(on: impl Fn(&str) -> Option<&'a Effective>) -> On<'a> {
        On { tier: on(TIER), area: on(AREA), cost: on(COST), rfi: on(RFI), design: on(DESIGN) }
    }
    fn any(&self) -> bool {
        [self.tier, self.area, self.cost, self.rfi, self.design].iter().any(Option::is_some)
    }
}

/// What the rules found.
#[derive(Default)]
pub(crate) struct Checked {
    /// (rank, rule, verdict, finding).
    pub findings: Vec<(u8, &'static str, &'static str, Value)>,
    /// Per rule: why → scenarios not measured.
    pub unmeasured: BTreeMap<String, BTreeMap<String, u64>>,
}

impl Checked {
    fn unmeasured(&mut self, rule: &str, why: impl Into<String>, n: u64) {
        if n > 0 {
            *self.unmeasured.entry(rule.to_string()).or_default().entry(why.into()).or_default() += n;
        }
    }
}

fn rank(verdict: &str, severity: &str) -> u8 {
    match verdict {
        "pass" => 9,
        "unsure" => 5,
        _ => match severity {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        },
    }
}

fn tol(r: &Effective, name: &str, default: f64) -> f64 {
    r.rule.tolerance.get(name).copied().filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(default)
}

fn has(text: &str, words: &[String]) -> bool {
    let up = text.to_ascii_uppercase();
    words.iter().any(|w| !w.is_empty() && up.contains(w.as_str()))
}

fn cite_key(c: &str) -> String {
    c.to_ascii_uppercase().chars().filter(|c| !c.is_whitespace()).collect()
}

/// A construction type's class (`V-A` → `V`).
fn class_of(t: &str) -> String {
    let up = t.trim().to_ascii_uppercase();
    up.trim_start_matches("TYPE").trim().split(['-', ' ']).next().unwrap_or_default().to_string()
}

/// Where one of a scenario's pieces is.
#[derive(Clone, Debug, Default)]
struct Place {
    /// `unit`, `common`, `exposed`, or empty when nothing says.
    area: &'static str,
    /// `corridor`, `plenum`, `below_slab`, `suspended`, `open_ceiling`, `ceiling_unknown`.
    flags: BTreeSet<&'static str>,
    room: String,
    /// What gave the area: `room`, `beside`, `circuit`.
    from: &'static str,
    /// Every area its points are in (empty: a point no area is known for).
    areas: BTreeSet<&'static str>,
    /// The rooms of each of its areas.
    rooms: BTreeMap<&'static str, BTreeSet<String>>,
    /// Its points in each area and with each flag (each about [`SAMPLE_FT`] of it).
    points: BTreeMap<&'static str, usize>,
    /// The areas and flags only a point or so of it touches (not held: its edge).
    edges: BTreeMap<&'static str, usize>,
}

/// A scenario piece: its handle, kind, level, plan centre and lowest point.
struct Piece {
    handle: Handle,
    kind: String,
    level: String,
    of: String,
    at: Vec3,
    z_min: f64,
    /// Points along its body (its edges, every [`SAMPLE_FT`]): a long cable crosses rooms.
    samples: Vec<Vec3>,
}

/// A mesh piece's plan centre and lowest point.
fn mesh_place(e: &Entity) -> Option<(Vec3, f64)> {
    let EntityKind::Mesh(m) = &e.kind else { return None };
    let mut lo = Vec3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut hi = Vec3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for v in m.vertices.iter().take(100_000).filter(|v| v.x.is_finite() && v.y.is_finite() && v.z.is_finite()) {
        lo = Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z));
        hi = Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z));
    }
    (lo.x.is_finite() && hi.x.is_finite()).then(|| (Vec3::new((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5, (lo.z + hi.z) * 0.5), lo.z))
}

/// Points along a mesh's edges every `step` (drawing units), one per `step` cell.
fn mesh_samples(e: &Entity, step: f64) -> Vec<Vec3> {
    let EntityKind::Mesh(m) = &e.kind else { return Vec::new() };
    let step = if step.is_finite() && step > 0.0 { step } else { 24.0 };
    let mut seen: BTreeSet<(i64, i64)> = BTreeSet::new();
    let mut out = Vec::new();
    let key = |p: Vec3| ((p.x / step).floor().clamp(-1e12, 1e12) as i64, (p.y / step).floor().clamp(-1e12, 1e12) as i64);
    'faces: for f in m.faces.iter().take(4_000) {
        let n = f.v.len();
        for i in 0..n {
            let (Some(a), Some(b)) =
                (f.v.get(i).and_then(|k| m.vertices.get(*k as usize)), f.v.get((i + 1) % n.max(1)).and_then(|k| m.vertices.get(*k as usize)))
            else {
                continue;
            };
            let (a, b) = (*a, *b);
            if !(a.x.is_finite() && a.y.is_finite() && a.z.is_finite() && b.x.is_finite() && b.y.is_finite() && b.z.is_finite()) {
                continue;
            }
            let steps = ((b - a).len() / step).ceil().clamp(1.0, 200.0) as usize;
            for k in 0..=steps {
                let p = a + (b - a) * (k as f64 / steps as f64);
                if seen.insert(key(p)) {
                    out.push(p);
                    if out.len() >= MAX_SAMPLES {
                        break 'faces;
                    }
                }
            }
        }
    }
    out
}

fn text(ps: &props::Props, k: &str) -> String {
    ps.get(k).map(Prop::text).unwrap_or_default().trim().to_string()
}

fn handle_of(hex: &str) -> Option<Handle> {
    u64::from_str_radix(hex.trim(), 16).ok().map(Handle)
}

/// The places a scenario's pieces can be read from: the model's rooms, the design pieces' rooms,
/// the circuits' areas, the ceilings, the lowest floor.
struct Places<'a> {
    rooms: HashMap<String, Vec<(Vec<Vec2>, [f64; 4], String)>>,
    beside: HashMap<(String, i64, i64), Vec<(Vec2, String)>>,
    circuit_area: HashMap<&'a str, String>,
    lowest_z: Option<f64>,
    sp: &'a BuildingSpec,
    upf: f64,
    data: &'a Data,
}

impl<'a> Places<'a> {
    fn new(d: &Drawing, sp: &'a BuildingSpec, data: &'a Data, design: &[Handle]) -> Self {
        let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
        let mut rooms: HashMap<String, Vec<(Vec<Vec2>, [f64; 4], String)>> = HashMap::new();
        for r in super::bspec::room_elements(d).into_iter().filter(|r| r.outline.len() >= 3).take(MAX_PIECES) {
            let mut bb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            for p in &r.outline {
                bb = [bb[0].min(p.x), bb[1].min(p.y), bb[2].max(p.x), bb[3].max(p.y)];
            }
            let name = match &r.number {
                Some(n) if !n.trim().is_empty() => format!("{} {}", r.name.trim(), n.trim()),
                _ => r.name.trim().to_string(),
            };
            rooms.entry(r.level.to_ascii_lowercase()).or_default().push((r.outline.clone(), bb, name));
        }
        let cell = CELL_FT * upf;
        let mut beside: HashMap<(String, i64, i64), Vec<(Vec2, String)>> = HashMap::new();
        for h in design.iter().take(MAX_PIECES) {
            let Some(e) = d.model.get(*h) else { continue };
            let ps = props::read(&e.common.xdata);
            let room = text(&ps, "room");
            if room.is_empty() {
                continue;
            }
            let at = super::mepatoms::build::stick_ends(&ps).map(|(a, b)| (a + b) * 0.5).or_else(|| mesh_place(e).map(|m| m.0));
            let Some(at) = at.filter(|a| a.x.is_finite() && a.y.is_finite()) else { continue };
            let level = super::bspec::made_level(e).unwrap_or_default().to_ascii_lowercase();
            let key = (level, (at.x / cell).floor().clamp(-1e12, 1e12) as i64, (at.y / cell).floor().clamp(-1e12, 1e12) as i64);
            beside.entry(key).or_default().push((Vec2::new(at.x, at.y), room));
        }
        // Each circuit's area as the router gave it (a typical room's circuit is a unit's); a
        // homerun's, its circuits' when they agree (its own tag carries none).
        let tag_area = |t: &cadaddy_walk3d::tags::ElementTag| -> String {
            let a = t.fields.get("area").map(|f| f.value.trim().to_ascii_lowercase()).unwrap_or_default();
            if t.id.contains(":room-") { "unit".to_string() } else { a }
        };
        let mut circuit_area: HashMap<&str, String> = HashMap::new();
        let mut groups: HashMap<(String, String), BTreeSet<String>> = HashMap::new();
        for t in sp.tags.iter().filter(|t| t.kind == "circuit").take(MAX_PIECES) {
            let a = tag_area(t);
            if a.is_empty() {
                continue;
            }
            if let Some(g) = t.fields.get("homerun_group").map(|f| f.value.trim().replace(['/', '#', '@'], "-")).filter(|g| !g.is_empty()) {
                groups.entry((t.level.to_ascii_lowercase(), g)).or_default().insert(a.clone());
            }
            circuit_area.insert(t.id.as_str(), a);
        }
        let homeruns: Vec<(&str, String)> = sp
            .tags
            .iter()
            .filter(|t| t.kind == "circuit" && !circuit_area.contains_key(t.id.as_str()))
            .take(MAX_PIECES)
            .filter_map(|t| {
                let (_, g) = t.id.split_once(HOME_PART)?;
                let set = groups.get(&(t.level.to_ascii_lowercase(), g.to_string()))?;
                (set.len() == 1).then(|| set.iter().next().map(|a| (t.id.as_str(), a.clone()))).flatten()
            })
            .collect();
        circuit_area.extend(homeruns);
        let lowest_z = sp.levels.iter().filter(|l| l.elevation_ft.is_finite()).map(|l| l.elevation_ft * upf).min_by(f64::total_cmp);
        Places { rooms, beside, circuit_area, lowest_z, sp, upf, data }
    }

    /// The room holding a plan point on a level.
    fn room_at(&self, level: &str, p: Vec2) -> Option<&str> {
        self.rooms
            .get(&level.to_ascii_lowercase())?
            .iter()
            .find(|(o, bb, _)| p.x >= bb[0] && p.x <= bb[2] && p.y >= bb[1] && p.y <= bb[3] && cadaddy_geom::point_in_polygon(o, p))
            .map(|r| r.2.as_str())
    }

    /// The room of the nearest design piece within reach.
    fn beside_room(&self, level: &str, p: Vec2) -> Option<&str> {
        let cell = CELL_FT * self.upf;
        let (cx, cy) = ((p.x / cell).floor().clamp(-1e12, 1e12) as i64, (p.y / cell).floor().clamp(-1e12, 1e12) as i64);
        let lv = level.to_ascii_lowercase();
        let reach = BESIDE_FT * self.upf;
        let mut best: Option<(f64, &str)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for (q, room) in self.beside.get(&(lv.clone(), cx + dx, cy + dy)).into_iter().flatten() {
                    let dd = q.dist(p);
                    if dd <= reach && best.is_none_or(|b| dd < b.0) {
                        best = Some((dd, room.as_str()));
                    }
                }
            }
        }
        best.map(|b| b.1)
    }

    /// The ceiling over a plan point: `suspended` (a grid), `open` (the structure exposed),
    /// `barrier` (gypsum); none when the documents don't type one.
    fn ceiling(&self, level: &str, p: Vec2) -> Option<&'static str> {
        let c = self.sp.rcp.ceilings.iter().take(MAX_PIECES).find(|c| {
            c.level.eq_ignore_ascii_case(level) && c.outline.len() >= 3 && {
                let o: Vec<Vec2> = c.outline.iter().map(|q| Vec2::new(q[0], q[1])).collect();
                cadaddy_geom::point_in_polygon(&o, p)
            }
        })?;
        let ty = c.ceiling_type.as_deref().and_then(|t| self.sp.ceiling_types.iter().find(|x| x.name.eq_ignore_ascii_case(t)));
        Some(match ty.map(|t| t.kind) {
            _ if c.grid.is_some() => "suspended",
            Some(cadaddy_walk3d::CeilingKind::Grid) => "suspended",
            Some(cadaddy_walk3d::CeilingKind::Open) => "open",
            _ => "barrier",
        })
    }

    /// Where a piece is: each of its points placed, the areas and flags of all of them (one
    /// with no area at some point is unplaced there).
    fn place(&self, p: &Piece) -> Place {
        let mut all = Place::default();
        let points: Vec<Vec3> = if p.samples.is_empty() { vec![p.at] } else { p.samples.clone() };
        let total = points.len();
        let mut flags: BTreeSet<&'static str> = BTreeSet::new();
        for q in points {
            let one = self.place_at(p, q);
            *all.points.entry(one.area).or_default() += 1;
            for f in &one.flags {
                *all.points.entry(f).or_default() += 1;
            }
            flags.extend(one.flags);
            if !one.room.is_empty() {
                let e = all.rooms.entry(one.area).or_default();
                if e.len() < MAX_NAMED {
                    e.insert(one.room.clone());
                }
                if all.room.is_empty() {
                    all.room = one.room;
                }
            }
            if all.from.is_empty() {
                all.from = one.from;
            }
            all.area = one.area;
        }
        // An area or a flag holds the piece where at least [`MIN_POINTS`] of its points are (about
        // 4 ft), or all of them: one point is a cable grazing a room's edge along its wall.
        let holds = |n: usize| n >= MIN_POINTS.min(total) || n == total;
        let mut edges: BTreeMap<&'static str, usize> = BTreeMap::new();
        for (k, n) in &all.points {
            if !holds(*n) {
                edges.insert(k, *n);
            }
        }
        all.areas = all.points.iter().filter(|(k, n)| holds(**n) && !flags.contains(*k) && !FLAG_NAMES.contains(*k)).map(|(k, _)| *k).collect();
        if all.areas.is_empty() {
            all.areas = all.points.iter().filter(|(k, _)| !FLAG_NAMES.contains(*k)).max_by_key(|(_, n)| **n).map(|(k, _)| *k).into_iter().collect();
        }
        all.flags = flags.into_iter().filter(|f| all.points.get(f).is_some_and(|n| holds(*n))).collect();
        all.edges = edges;
        if self.lowest_z.is_some_and(|z| p.z_min < z - 0.1 * self.upf) {
            all.flags.insert("below_slab");
        }
        all.area = all
            .areas
            .iter()
            .copied()
            .find(|a| !a.is_empty() && *a != "unit")
            .or_else(|| all.areas.iter().copied().find(|a| !a.is_empty()))
            .unwrap_or("");
        all
    }

    fn place_at(&self, p: &Piece, at: Vec3) -> Place {
        let mr = &super::conduitroute::rules().methods;
        let plan = Vec2::new(at.x, at.y);
        let mut pl = Place::default();
        let (room, from) = match self.room_at(&p.level, plan) {
            Some(r) => (Some(r), "room"),
            None => (self.beside_room(&p.level, plan), "beside"),
        };
        let circuit = self.circuit_area.get(p.of.as_str()).map(String::as_str);
        if let Some(r) = room {
            pl.room = r.to_string();
            if has(r, &self.data.words.corridor) {
                pl.flags.insert("corridor");
            }
            if has(r, &self.data.words.plenum) {
                pl.flags.insert("plenum");
            }
        }
        // The area: an exposed room or a corridor as the room says; else the circuit's (a typical
        // room's circuit is a unit's); else the room's words.
        pl.area = match (room, circuit) {
            (Some(r), _) if has(r, &mr.exposed_rooms) => "exposed",
            (Some(_), _) if pl.flags.contains("corridor") => "common",
            (_, Some("unit")) => "unit",
            (_, Some("exposed")) => "exposed",
            (_, Some(_)) => "common",
            (Some(r), None) if has(r, &mr.unit_words) => "unit",
            (Some(_), None) => "common",
            (None, None) => "",
        };
        pl.from = match (room, circuit) {
            (Some(_), _) if pl.area == "exposed" || pl.flags.contains("corridor") => from,
            (_, Some(_)) => "circuit",
            (Some(_), None) => from,
            _ => "",
        };
        match self.ceiling(&p.level, plan) {
            Some("suspended") => {
                pl.flags.insert("suspended");
            }
            Some("open") => {
                pl.flags.insert("open_ceiling");
            }
            Some(_) => {}
            None => {
                pl.flags.insert("ceiling_unknown");
            }
        }
        pl
    }
}

/// One scenario as the study answered it and as the drawing keeps it.
struct Sc<'a> {
    id: String,
    name: String,
    tier: String,
    kind: String,
    cable: String,
    study: Option<&'a Value>,
    kept: Option<&'a super::valueeng::KeptScenario>,
}

impl Sc<'_> {
    fn proposed(&self) -> bool {
        matches!(self.tier.as_str(), "allowed_now" | "needs_rfi")
    }
}

/// The method key a cable stands for (`NM` → `nm`).
fn method_of(data: &Data, cable: &str) -> Option<String> {
    let up = cable.trim().to_ascii_uppercase();
    data.cables.get(&up).cloned().or_else(|| data.cables.iter().find(|(k, _)| up.starts_with(k.as_str())).map(|(_, v)| v.clone()))
}

/// A side's lines, summed by kind and unit (firestops apart: they come from the router's
/// penetrations, not the model's pieces).
fn line_sums(side: &Value, skip: &[String]) -> BTreeMap<(String, String), f64> {
    let mut out = BTreeMap::new();
    for l in side["lines"].as_array().into_iter().flatten().take(MAX_LINES) {
        let kind = l["kind"].as_str().unwrap_or_default().to_string();
        if kind == "firestop" || skip.contains(&kind) {
            continue;
        }
        let qty = l["qty"].as_f64().filter(|q| q.is_finite()).unwrap_or(0.0);
        *out.entry((kind, l["unit"].as_str().unwrap_or("ea").to_string())).or_default() += qty;
    }
    out
}

/// The model's pieces summed by kind and unit (`key`: the prop naming their kind).
fn piece_sums(d: &Drawing, handles: &[String], key: &str, skip: &[String]) -> (BTreeMap<(String, String), f64>, usize) {
    let mut out = BTreeMap::new();
    let mut found = 0usize;
    for h in handles.iter().take(MAX_PIECES).filter_map(|h| handle_of(h)) {
        let Some(e) = d.model.get(h) else { continue };
        found += 1;
        let ps = props::read(&e.common.xdata);
        let kind = text(&ps, key);
        if skip.contains(&kind) {
            continue;
        }
        let by_ft = matches!(kind.as_str(), "conduit" | "cable" | "wire");
        let qty = if by_ft { text(&ps, "length_ft").parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0) } else { 1.0 };
        *out.entry((kind, if by_ft { "ft".to_string() } else { "ea".to_string() })).or_default() += qty;
    }
    (out, found)
}

fn close(a: f64, b: f64, pct: f64, abs: f64) -> bool {
    (a - b).abs() <= (pct / 100.0 * a.abs().max(b.abs())).max(abs)
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

/// Run the value-engineering rules over the scope.
pub(crate) fn check(cx: &Ctx, on: On) -> Checked {
    let mut out = Checked::default();
    if !on.any() {
        return out;
    }
    let data = data();
    let kept = super::valueeng::kept_scenarios(cx.d);
    let study = cx.inputs.and_then(|i| i.study.as_ref());
    let study_from = cx.inputs.map(|i| i.study_from.as_str()).unwrap_or_default();
    // Every scenario of the study and of the drawing, by id.
    let mut scs: Vec<Sc> = Vec::new();
    for v in study.and_then(|s| s["scenarios"].as_array()).into_iter().flatten().take(MAX_SCENARIOS) {
        let id = v["id"].as_str().unwrap_or_default().to_string();
        if id.is_empty() {
            continue;
        }
        let k = kept.iter().find(|k| k.id == id);
        scs.push(Sc {
            name: v["name"].as_str().unwrap_or_default().to_string(),
            tier: v["tier"].as_str().unwrap_or_default().to_string(),
            kind: v["kind"].as_str().map(str::to_string).or_else(|| k.map(|k| k.kind.clone())).unwrap_or_default(),
            cable: k.map(|k| k.cable.clone()).unwrap_or_default(),
            id,
            study: Some(v),
            kept: k,
        });
    }
    for k in kept.iter().take(MAX_SCENARIOS) {
        if !scs.iter().any(|s| s.id == k.id) {
            scs.push(Sc {
                id: k.id.clone(),
                name: k.name.clone(),
                tier: k.tier.clone(),
                kind: k.kind.clone(),
                cable: k.cable.clone(),
                study: None,
                kept: Some(k),
            });
        }
    }
    if scs.is_empty() {
        for r in ALL {
            out.unmeasured(r, "no value engineering study in the model or the job", 1);
        }
        return out;
    }
    // A scenario's cable when the drawing doesn't keep it: its alternative method's words.
    for sc in scs.iter_mut().filter(|s| s.cable.is_empty()) {
        let m = sc.study.and_then(|v| v["alternative_method"].as_str()).unwrap_or_default().to_ascii_uppercase();
        if let Some((c, _)) = data.cables.iter().filter(|(c, _)| m.contains(&format!("({c})"))).max_by_key(|(c, _)| c.len()) {
            sc.cable = c.clone();
        }
    }
    let design_all: Vec<Handle> = kept.iter().flat_map(|k| k.replaces.iter().filter_map(|h| handle_of(h))).take(MAX_PIECES).collect();
    let places = Places::new(cx.d, cx.sp, &data, &design_all);
    let frame = cx.frame;
    let in_scope = |level: &str, p: Vec3| -> Option<(Option<&Area>, [f64; 2])> {
        let at = frame.to_ft_on(level, Vec2::new(p.x, p.y));
        let mine: Vec<&Area> = cx.areas.iter().filter(|a| a.level.eq_ignore_ascii_case(level)).collect();
        if mine.is_empty() {
            return None;
        }
        let a = mine.iter().copied().find(|a| inside(a.bbox, at) && (a.outline.len() < 3 || point_in(&a.outline, at)));
        if a.is_some() || cx.whole_levels { Some((a, at)) } else { None }
    };
    let construction = study.and_then(|s| s["facts"]["construction_type"]["value"].as_str()).map(str::to_string);
    let stated_rate = study.and_then(|s| s["pricing"]["manpower"]["manpower_rate"].as_f64()).filter(|r| r.is_finite() && *r > 0.0);
    let book = cx.inputs.and_then(|i| i.book.as_ref());
    let book_from = cx.inputs.map(|i| i.book_from.as_str()).unwrap_or_default();
    // The specification's wiring-methods table: the job's own reading, else the study's.
    let mut apps: Vec<(String, String, String, Vec<String>)> =
        super::conduitroute::Methods::stored(cx.d).applications.into_iter().map(|a| (a.section, a.item, a.application, a.methods)).collect();
    let apps_from = if apps.is_empty() {
        for a in study.and_then(|s| s["applications"].as_array()).into_iter().flatten().take(200) {
            let s = |k: &str| a[k].as_str().unwrap_or_default().to_string();
            let methods = a["methods"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            apps.push((s("section"), s("item"), s("application"), methods));
        }
        "the study's reading of the specification"
    } else {
        "the job's reading of the specification"
    };
    let mr = &super::conduitroute::rules().methods;

    for sc in &scs {
        // Its pieces and where each is.
        let pieces: Vec<Piece> = sc
            .kept
            .map(|k| {
                k.pieces
                    .iter()
                    .take(MAX_PIECES)
                    .filter_map(|h| handle_of(h))
                    .filter_map(|h| cx.d.model.get(h))
                    .filter_map(|e| {
                        let ps = props::read(&e.common.xdata);
                        let (at, z_min) = mesh_place(e)?;
                        // An adopted scenario's pieces are design pieces: their kind and level as
                        // the build tags them.
                        let kind = Some(text(&ps, super::valueeng::VE_ATOM))
                            .filter(|k| !k.is_empty())
                            .unwrap_or_else(|| text(&ps, super::mepatoms::ATOM_KEY));
                        let level = Some(text(&ps, "level")).filter(|l| !l.is_empty()).or_else(|| super::bspec::made_level(e)).unwrap_or_default();
                        let samples = if kind == "cable" { mesh_samples(e, SAMPLE_FT * places.upf) } else { Vec::new() };
                        Some(Piece { handle: e.handle, kind, level, of: text(&ps, "of"), at, z_min, samples })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let cables: Vec<(&Piece, Place)> = pieces.iter().filter(|p| p.kind == "cable").map(|p| (p, places.place(p))).collect();
        // Its place in the scope: its first piece there; one with no pieces, the scope's.
        let place = pieces.iter().find_map(|p| in_scope(&p.level, p.at).map(|s| (p, s)));
        let (lead, level, at_place) = match (place, pieces.first()) {
            (Some((p, s)), _) => (Some(p.handle), p.level.clone(), s),
            (None, first) if cx.whole_levels => (first.map(|p| p.handle), first.map(|p| p.level.clone()).unwrap_or_default(), (None, [0.0, 0.0])),
            _ => continue,
        };
        let mut push = |rule: &'static str, r: &Effective, verdict: &'static str, issue: String, expected: String, found: String, measured: Value| {
            let id = format!("PS-{}", super::validate_run::hash8(&format!("{rule}|{}|{verdict}", sc.id)));
            out.findings.push((
                rank(verdict, &r.rule.severity),
                rule,
                verdict,
                json!({
                    "id": id, "rule": rule, "handle": lead.map(|h| h.hex()), "kind": "scenario", "scenario": sc.id, "tier": sc.tier,
                    "verdict": verdict, "issue": issue, "expected": expected, "found": found, "measured": measured, "evidence": [],
                    "confidence": if verdict == "unsure" { 0.5 } else { 1.0 },
                    "severity": r.rule.severity, "area": at_place.0.map(|a| a.id.clone()), "level": level,
                    "at": pt([r2(at_place.1[0]), r2(at_place.1[1])]), "source": "prescreen",
                }),
            ));
        };
        let method = method_of(&data, &sc.cable);
        let label = if sc.name.is_empty() { sc.id.clone() } else { format!("{} ({})", sc.name, sc.id) };
        // The areas its cable runs in, and what is said of each.
        let mut by_area: BTreeMap<&str, usize> = BTreeMap::new();
        let mut by_flag: BTreeMap<&str, usize> = BTreeMap::new();
        let mut rooms_of: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for (_, pl) in &cables {
            for a in &pl.areas {
                *by_area.entry(if a.is_empty() { "unknown" } else { a }).or_default() += 1;
            }
            for f in &pl.flags {
                *by_flag.entry(f).or_default() += 1;
            }
            for (a, rs) in &pl.rooms {
                let e = rooms_of.entry(if a.is_empty() { "unknown" } else { a }).or_default();
                for r in rs {
                    if e.len() < MAX_NAMED {
                        e.insert(r.as_str());
                    }
                }
            }
        }

        // ---- G-VE-AREA
        if let Some(r) = on.area.filter(|_| sc.proposed() || sc.kept.is_some_and(|k| k.adopted)) {
            let never = method.as_ref().and_then(|m| data.never.get(m));
            let measured_base = json!({"check": "area", "method": method, "cable": sc.cable, "cable_pieces": cables.len(), "by_area": by_area, "flags": by_flag, "rooms": rooms_of});
            match (never, cables.is_empty()) {
                (_, true) => push(
                    AREA,
                    r,
                    "unsure",
                    format!("{label}: none of its cable is in the model (no scenario pieces), so where it runs can't be measured"),
                    "its cable only where its method is used".into(),
                    "no pieces".into(),
                    measured_base,
                ),
                (None, false) => push(
                    AREA,
                    r,
                    "unsure",
                    format!("{label}: no area rule for {} cable in the rules data", if sc.cable.is_empty() { "its" } else { &sc.cable }),
                    "an area rule for the method".into(),
                    sc.cable.clone(),
                    measured_base,
                ),
                (Some(never), false) => {
                    let mut bad: BTreeMap<&str, (usize, Option<Handle>, BTreeSet<&str>)> = BTreeMap::new();
                    // About how much of the cable is there, feet (its points there).
                    let mut feet: BTreeMap<&str, f64> = BTreeMap::new();
                    let mut unknown = 0usize;
                    // Pieces that only graze a never area (a point at its edge): said, not failed.
                    let mut grazing: BTreeMap<&str, usize> = BTreeMap::new();
                    for (p, pl) in &cables {
                        for k in pl.edges.keys().filter(|k| never.contains_key(**k)) {
                            *grazing.entry(k).or_default() += 1;
                        }
                        let mut hit: Vec<&str> = pl.areas.iter().copied().filter(|a| !a.is_empty() && never.contains_key(*a)).collect();
                        hit.extend(pl.flags.iter().copied().filter(|f| never.contains_key(*f)));
                        if pl.areas.contains("") && never.keys().any(|k| matches!(k.as_str(), "common" | "exposed" | "unit")) {
                            unknown += 1;
                        }
                        for h in hit {
                            *feet.entry(h).or_default() += pl.points.get(h).copied().unwrap_or(1) as f64 * SAMPLE_FT;
                            let e = bad.entry(h).or_insert((0, Some(p.handle), BTreeSet::new()));
                            e.0 += 1;
                            // The rooms of that area (a flag: all of the piece's rooms).
                            let rooms: Vec<&String> = match pl.rooms.get(h) {
                                Some(rs) => rs.iter().collect(),
                                None if !pl.areas.contains(h) => pl.rooms.values().flatten().collect(),
                                None => Vec::new(),
                            };
                            for r in rooms {
                                if e.2.len() < MAX_NAMED {
                                    e.2.insert(r.as_str());
                                }
                            }
                        }
                    }
                    let mut measured = measured_base;
                    measured["never"] = json!(never);
                    measured["grazing"] = json!(grazing);
                    if !bad.is_empty() {
                        measured["in_never_areas"] = json!(
                            bad.iter()
                                .map(|(a, (n, h, rooms))| (
                                    a.to_string(),
                                    json!({"pieces": n, "about_ft": feet.get(a).copied().unwrap_or(0.0), "first": h.map(|h| h.hex()), "rooms": rooms, "why": never.get(*a)})
                                ))
                                .collect::<BTreeMap<_, _>>()
                        );
                        let said = bad.iter().map(|(a, (n, _, rooms))| {
                            let rooms: Vec<&str> = rooms.iter().copied().take(4).collect();
                            format!(
                                "{n} piece(s) {} (about {:.0} ft){}",
                                a.replace('_', " "),
                                feet.get(a).copied().unwrap_or(0.0),
                                if rooms.is_empty() { String::new() } else { format!(" ({})", rooms.join(", ")) }
                            )
                        });
                        push(
                            AREA,
                            r,
                            "fail",
                            format!("{label} puts {} cable where it is never used: {}", sc.cable, said.collect::<Vec<_>>().join("; ")),
                            format!("no {} cable in {}", sc.cable, never.keys().map(|k| k.replace('_', " ")).collect::<Vec<_>>().join(", ")),
                            bad.keys().map(|k| k.replace('_', " ")).collect::<Vec<_>>().join(", "),
                            measured,
                        );
                    } else if unknown > 0 {
                        push(
                            AREA,
                            r,
                            "unsure",
                            format!(
                                "{label}: {unknown} of its {} cable pieces are in no room the model gives and on no circuit whose area is known",
                                cables.len()
                            ),
                            format!("no {} cable in {}", sc.cable, never.keys().map(|k| k.replace('_', " ")).collect::<Vec<_>>().join(", ")),
                            format!("{unknown} unplaced"),
                            measured,
                        );
                    } else {
                        push(
                            AREA,
                            r,
                            "pass",
                            format!("{label}: its {} cable pieces are all where {} is used", cables.len(), sc.cable),
                            format!("no {} cable in {}", sc.cable, never.keys().map(|k| k.replace('_', " ")).collect::<Vec<_>>().join(", ")),
                            by_area.keys().copied().collect::<Vec<_>>().join(", "),
                            measured,
                        );
                    }
                }
            }
        }

        // ---- G-VE-TIER
        if let Some(r) = on.tier.filter(|_| sc.proposed()) {
            let mut fails: Vec<String> = Vec::new();
            let mut unsure: Vec<String> = Vec::new();
            let mut notes: Vec<String> = Vec::new();
            // The specification: each area's paragraph.
            let kind_feeder = sc.kind == "feeder";
            let areas: Vec<&str> = by_area.keys().copied().filter(|a| *a != "unknown").collect();
            if cables.is_empty() {
                unsure.push("none of its cable is in the model: the areas it covers can't be read".into());
            } else if by_area.contains_key("unknown") {
                unsure.push(format!("{} of its cable pieces are in no area the model gives", by_area.get("unknown").copied().unwrap_or(0)));
            }
            let mine: Vec<&(String, String, String, Vec<String>)> = apps
                .iter()
                .filter(|(_, _, a, _)| {
                    let up = a.to_ascii_uppercase();
                    if kind_feeder {
                        has(&up, &mr.feeder_words) && !has(&up, &mr.branch_words)
                    } else {
                        !has(&up, &mr.feeder_words) || has(&up, &mr.branch_words)
                    }
                })
                .collect();
            let app_for = |area: &str| -> Option<&(String, String, String, Vec<String>)> {
                let pick = |pred: &dyn Fn(&str) -> bool| mine.iter().copied().find(|a| pred(&a.2.to_ascii_uppercase()));
                let concealed = |u: &str| has(u, &mr.concealed_words) && !has(u, &mr.exposed_words) && !has(u, &mr.below_words);
                match area {
                    "unit" => pick(&|u| has(u, &mr.unit_words) && !has(u, &mr.exposed_words)).or_else(|| pick(&|u| concealed(u))),
                    "exposed" => pick(&|u| has(u, &mr.exposed_words) && !has(u, &mr.unit_words)),
                    "below_slab" => pick(&|u| has(u, &mr.below_words)),
                    _ => pick(&|u| concealed(u) && !has(u, &mr.unit_words)),
                }
            };
            let mut spec_rows: Vec<Value> = Vec::new();
            let mut contradicted: Vec<String> = Vec::new();
            let mut spec_missing: Vec<String> = Vec::new();
            let mut wanted_areas: Vec<&str> = areas.clone();
            if by_flag.contains_key("below_slab") {
                wanted_areas.push("below_slab");
            }
            match &method {
                None => {
                    unsure.push(format!("its cable ({}) is no method the rules data names", if sc.cable.is_empty() { "not kept" } else { &sc.cable }))
                }
                Some(m) => {
                    if apps.is_empty() {
                        unsure.push("no wiring-methods table read from the specification".into());
                    }
                    for a in &wanted_areas {
                        match app_for(a) {
                            Some((section, item, application, methods)) => {
                                let lists = methods.iter().any(|x| x == m);
                                spec_rows.push(json!({"area": a, "section": section, "item": item, "application": application, "methods": methods, "lists": lists}));
                                if !lists {
                                    contradicted.push(format!(
                                        "{section} §{item} ({application}) lists {} only for its {} work",
                                        methods.join(", "),
                                        a.replace('_', " ")
                                    ));
                                }
                            }
                            None if !apps.is_empty() => spec_missing.push(a.replace('_', " ")),
                            None => {}
                        }
                    }
                }
            }
            if !spec_missing.is_empty() {
                unsure.push(format!("the specification's table has no paragraph for its {} work", spec_missing.join(", ")));
            }
            // The code: a rule it cites that permits its method, every condition met.
            let cites: Option<Vec<String>> =
                sc.study.map(|v| v["code"].as_array().into_iter().flatten().filter_map(|c| c["cite"].as_str().map(str::to_string)).collect());
            let rule = method.as_ref().and_then(|m| {
                let cs = cites.as_ref()?;
                data.code.iter().find(|c| c.methods.contains(m) && cs.iter().any(|x| cite_key(x) == cite_key(&c.cite)))
            });
            let mut code_row = Value::Null;
            match (&cites, rule) {
                (None, _) => unsure.push(format!(
                    "no study answer ({}): the code it cites isn't known",
                    if study_from.is_empty() { "none kept" } else { study_from }
                )),
                (Some(cs), None) => fails.push(format!(
                    "it cites {} — no code rule there (rules data) permits {}",
                    if cs.is_empty() { "nothing".to_string() } else { cs.join(", ") },
                    method.as_deref().unwrap_or("its method")
                )),
                (Some(_), Some(c)) => {
                    code_row = json!({"cite": c.cite, "says": c.says});
                    if !c.construction.is_empty() {
                        match &construction {
                            None => unsure.push(format!(
                                "the building's construction type isn't read: {} allows types {} only",
                                c.cite,
                                c.construction.join(", ")
                            )),
                            Some(t) if !c.construction.iter().any(|k| k.eq_ignore_ascii_case(&class_of(t))) => {
                                fails.push(format!("{} allows it in Types {} only; the building is Type {t}", c.cite, c.construction.join(", ")))
                            }
                            Some(_) => {}
                        }
                    }
                    if c.concealed && by_area.contains_key("exposed") {
                        fails.push(format!(
                            "{} permits it concealed only; {} of its pieces are exposed",
                            c.cite,
                            by_area.get("exposed").copied().unwrap_or(0)
                        ));
                    }
                    for f in &c.not_in {
                        if let Some(n) = by_flag.get(f.as_str()) {
                            fails.push(format!("{} never allows it {}: {n} of its pieces are there", c.cite, f.replace('_', " ")));
                        }
                    }
                    if c.thermal_barrier
                        && let Some(n) = by_flag.get("open_ceiling")
                    {
                        fails.push(format!("{} needs a thermal barrier: {n} of its pieces run under no ceiling (the structure exposed)", c.cite));
                    }
                    if c.thermal_barrier
                        && let Some(n) = by_flag.get("ceiling_unknown")
                    {
                        unsure.push(format!(
                            "{n} of its pieces run under ceilings the documents don't type: the thermal barrier {} needs (gypsum, not a suspended cavity) can't be confirmed",
                            c.cite
                        ));
                    }
                }
            }
            let tier_ok_spec = contradicted.is_empty();
            if sc.tier == "allowed_now" && !tier_ok_spec {
                fails.push(format!("the specification contradicts it — {}: it must be NEEDS RFI, never allowed now", contradicted.join("; ")));
            }
            if sc.tier == "needs_rfi" && tier_ok_spec && !spec_rows.is_empty() && spec_missing.is_empty() && unsure.is_empty() {
                notes.push("the specification lists it for every area it covers: it could be allowed now".into());
            }
            let verdict = if !fails.is_empty() {
                "fail"
            } else if !unsure.is_empty() {
                "unsure"
            } else {
                "pass"
            };
            let measured = json!({
                "check": "tier", "tier": sc.tier, "method": method, "areas": by_area, "flags": by_flag, "spec": spec_rows, "spec_from": apps_from,
                "code": code_row, "cites": cites, "construction_type": construction, "fails": fails, "unsure": unsure, "notes": notes,
            });
            let expected = if sc.tier == "allowed_now" {
                "permitted by the specification's paragraph for every area it covers and by a code rule it cites, every condition met".to_string()
            } else {
                "permitted by a code rule it cites, every condition met (the specification needs the RFI)".to_string()
            };
            let issue = match verdict {
                "fail" => format!("{label} is {} but {}", sc.tier.replace('_', " "), fails.join("; ")),
                "unsure" => format!("{label} ({}): {}", sc.tier.replace('_', " "), unsure.join("; ")),
                _ => format!(
                    "{label} is {}: {}{}",
                    sc.tier.replace('_', " "),
                    rule.map(|c| format!("{} permits it", c.cite)).unwrap_or_default(),
                    if notes.is_empty() { String::new() } else { format!(" ({})", notes.join("; ")) }
                ),
            };
            push(TIER, r, verdict, issue, expected, sc.tier.clone(), measured);
        }

        // ---- G-VE-COST
        if let Some(r) = on.cost.filter(|_| sc.proposed()) {
            let pct = tol(r, "cost_pct", DEFAULT_COST_PCT);
            let mut fails: Vec<String> = Vec::new();
            let mut unsure: Vec<String> = Vec::new();
            let mut measured = json!({"check": "cost", "cost_pct": pct, "stated_rate": stated_rate, "book": book_from});
            match sc.study {
                None => {
                    unsure.push(format!("no study answer ({}) to check its savings", if study_from.is_empty() { "none kept" } else { study_from }))
                }
                Some(v) => {
                    let claim = &v["saves"];
                    // Quantities: the study's lines against the model's pieces.
                    let mut qty_rows: Vec<Value> = Vec::new();
                    if let Some(k) = sc.kept {
                        let mut skip_design = data.same_either_way.clone();
                        if sc.kind == "feeder" {
                            // A feeder's conductors come from its schedule, not its pieces.
                            skip_design.push("wire".into());
                        }
                        for (side, handles, key, skip) in [
                            ("as_designed", &k.replaces, super::mepatoms::ATOM_KEY, &skip_design),
                            ("alternative", &k.pieces, super::valueeng::VE_ATOM, &data.same_either_way),
                        ] {
                            let said = line_sums(&v[side], skip);
                            if said.is_empty() {
                                continue;
                            }
                            let (model, found) = piece_sums(cx.d, handles, key, skip);
                            if found == 0 {
                                unsure.push(format!(
                                    "{} side: the model has none of its pieces (cut from the routes by the study)",
                                    side.replace('_', " ")
                                ));
                                continue;
                            }
                            for ((kind, unit), q) in &said {
                                let m = model.get(&(kind.clone(), unit.clone())).copied().unwrap_or(0.0);
                                let ok = close(*q, m, pct, ABS_QTY);
                                qty_rows.push(json!({"side": side, "kind": kind, "unit": unit, "study": r1(*q), "model": r1(m), "ok": ok}));
                                if !ok {
                                    fails.push(format!(
                                        "{} {kind}: the study counts {:.1} {unit}, the model's pieces {:.1}",
                                        side.replace('_', " "),
                                        q,
                                        m
                                    ));
                                }
                            }
                            for ((kind, unit), m) in model.iter().filter(|(k, _)| !said.contains_key(*k)) {
                                if *m > ABS_QTY {
                                    qty_rows.push(json!({"side": side, "kind": kind, "unit": unit, "study": 0.0, "model": r1(*m), "ok": false}));
                                    fails.push(format!("{} {kind}: {:.1} {unit} in the model, none in the study", side.replace('_', " "), m));
                                }
                            }
                        }
                    } else {
                        unsure.push("the drawing keeps no scenario for it: its quantities can't be recounted".into());
                    }
                    measured["quantities"] = json!(qty_rows);
                    // Prices: every line again from the price book.
                    match book {
                        None => unsure.push("no price book (cost DB) on this machine to price it again".into()),
                        Some(b) => {
                            let mut sum = BTreeMap::new();
                            let mut missing: Vec<String> = Vec::new();
                            for side in ["as_designed", "alternative"] {
                                let (mut m, mut l) = (0.0, 0.0);
                                for line in v[side]["lines"].as_array().into_iter().flatten().take(MAX_LINES) {
                                    let (item, size) = (line["item"].as_str().unwrap_or_default(), line["size"].as_str().unwrap_or_default());
                                    let q = line["qty"].as_f64().filter(|q| q.is_finite()).unwrap_or(0.0);
                                    match b.find(item, size) {
                                        Some(row) => {
                                            m += q * row.material;
                                            l += q * row.labor_dollars;
                                        }
                                        None => {
                                            if missing.len() < MAX_NAMED {
                                                missing.push(format!("{item} {size}").trim().to_string());
                                            }
                                        }
                                    }
                                }
                                sum.insert(side, (m, l));
                            }
                            let (dm, dl) = sum.get("as_designed").copied().unwrap_or_default();
                            let (am, al) = sum.get("alternative").copied().unwrap_or_default();
                            let rate = stated_rate.or(b.manpower_rate);
                            let re = json!({
                                "material": r2(dm - am), "labor_dollars": r2(dl - al), "cost": r2(dm + dl - am - al),
                                "man_hours": rate.map(|rt| r2((dl - al) / rt)), "rate": rate,
                                "rate_from": if stated_rate.is_some() { "the study's stated manpower rate" } else if b.manpower_rate.is_some() { "the price book's manpower rate" } else { "none" },
                            });
                            measured["recomputed"] = re.clone();
                            if !missing.is_empty() {
                                unsure.push(format!("the price book ({}) has no price for {}", b.path, missing.join(", ")));
                            } else if !claim.is_object() {
                                unsure.push(format!(
                                    "the study claims no saving (unpriced: {}); priced here it saves ${:.0}",
                                    v["unpriced"]
                                        .as_array()
                                        .map(|u| u.iter().filter_map(Value::as_str).take(6).collect::<Vec<_>>().join(", "))
                                        .unwrap_or_default(),
                                    dm + dl - am - al
                                ));
                            } else {
                                for (k, abs) in [("material", ABS_DOLLARS), ("labor_dollars", ABS_DOLLARS), ("cost", ABS_DOLLARS)] {
                                    let (c, x) = (claim[k].as_f64().unwrap_or(f64::NAN), re[k].as_f64().unwrap_or(f64::NAN));
                                    if !(c.is_finite() && close(c, x, pct, abs)) {
                                        fails.push(format!("its saving's {}: the study says ${c:.2}, recomputed ${x:.2}", k.replace('_', " ")));
                                    }
                                }
                                match (rate, claim["man_hours"].as_f64()) {
                                    (None, _) => unsure.push("no manpower rate stated: man-hours can't be checked".into()),
                                    (Some(rt), c) => {
                                        let x = (dl - al) / rt;
                                        let c = c.unwrap_or(f64::NAN);
                                        if !(c.is_finite() && close(c, x, pct, ABS_HOURS)) {
                                            fails.push(format!(
                                                "its saving's man-hours: the study says {c:.1}, labor ${:.2} ÷ ${rt}/h = {x:.1}",
                                                dl - al
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                    measured["claimed"] = claim.clone();
                }
            }
            measured["fails"] = json!(fails);
            measured["unsure"] = json!(unsure);
            let verdict = if !fails.is_empty() {
                "fail"
            } else if !unsure.is_empty() {
                "unsure"
            } else {
                "pass"
            };
            let issue = match verdict {
                "fail" => format!("{label}: its savings don't hold: {}", fails.join("; ")),
                "unsure" => format!("{label}: {}", unsure.join("; ")),
                _ => format!(
                    "{label}: its saving recomputed from the model's pieces and the price book (${:.0}, {:.1} man-hours)",
                    measured["recomputed"]["cost"].as_f64().unwrap_or(0.0),
                    measured["recomputed"]["man_hours"].as_f64().unwrap_or(0.0)
                ),
            };
            push(
                COST,
                r,
                verdict,
                issue,
                format!("savings within {pct}% of a recount and the price book; man-hours = labor $ ÷ the stated rate"),
                sc.study.map(|v| v["saves"].to_string()).unwrap_or_default(),
                measured,
            );
        }

        // ---- G-VE-RFI
        if let Some(r) = on.rfi.filter(|_| sc.tier == "needs_rfi") {
            let id = sc
                .kept
                .map(|k| k.rfi.clone())
                .filter(|x| !x.is_empty())
                .or_else(|| sc.study.and_then(|v| v["rfi"]["id"].as_str()).map(str::to_string))
                .unwrap_or_default();
            let mut fails: Vec<String> = Vec::new();
            let mut notes: Vec<String> = Vec::new();
            let rfi = (!id.is_empty()).then(|| cx.sp.rfis.iter().find(|x| x.id == id)).flatten();
            let mut measured = json!({"check": "rfi", "rfi": id});
            match rfi {
                None if id.is_empty() => fails.push("no RFI drafted for it".into()),
                None => fails.push(format!("its RFI {id} isn't on the RFI list")),
                Some(x) => {
                    measured["status"] = json!(x.status.name());
                    if x.kind != Some(RfiKind::MethodChoice) {
                        fails.push(format!("{id} isn't a method-choice RFI (a review item of kind rfi for the method)"));
                    }
                    let up = x.issue.to_ascii_uppercase();
                    let cites: Vec<String> = sc
                        .study
                        .map(|v| v["code"].as_array().into_iter().flatten().filter_map(|c| c["cite"].as_str().map(str::to_string)).collect())
                        .unwrap_or_default();
                    if cites.is_empty() {
                        if !up.contains("NEC ") {
                            fails.push(format!("{id} cites no code section"));
                        }
                        notes.push("no study answer: its citations are checked for a code section, not against the option's".into());
                    } else {
                        let gone: Vec<&String> = cites.iter().filter(|c| !cite_key(&up).contains(&cite_key(c))).collect();
                        if !gone.is_empty() {
                            fails.push(format!(
                                "{id} leaves out the option's citation(s) {}",
                                gone.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(", ")
                            ));
                        }
                    }
                    measured["cites"] = json!(cites);
                    let intent = x.issue.lines().find_map(|l| l.trim().strip_prefix("Intent:")).map(str::trim).unwrap_or_default();
                    measured["intent"] = json!(intent.chars().take(400).collect::<String>());
                    if intent.is_empty() || has(intent, &data.rfi_no_intent) {
                        fails.push(format!(
                            "{id} states no design intent (why the design asks for the method, and how the alternative still meets it)"
                        ));
                    }
                }
            }
            measured["fails"] = json!(fails);
            measured["notes"] = json!(notes);
            let verdict = if fails.is_empty() { "pass" } else { "fail" };
            push(
                RFI,
                r,
                verdict,
                if fails.is_empty() {
                    format!("{label}: RFI {id} gives its citations and the design's intent")
                } else {
                    format!("{label}: {}", fails.join("; "))
                },
                "a method-choice RFI with every citation of the option and the design's intent".into(),
                id,
                measured,
            );
        }

        // ---- G-VE-DESIGN
        if let (Some(r), Some(k)) = (on.design, sc.kept) {
            let mut fails: Vec<String> = Vec::new();
            let mut notes: Vec<String> = Vec::new();
            let rfi_status = cx.sp.rfis.iter().find(|x| !k.rfi.is_empty() && x.id == k.rfi).map(|x| x.status);
            let approved = match k.decision.as_str() {
                "approved" => k.rfi.is_empty() || rfi_status == Some(cadaddy_walk3d::RfiStatus::Approved),
                "chosen" => k.tier == "allowed_now",
                _ => false,
            };
            if k.adopted {
                if !approved {
                    fails.push(format!(
                        "it became the design ({}) without {}",
                        if k.decision.is_empty() { "no decision" } else { &k.decision },
                        if k.tier == "needs_rfi" { format!("its RFI {} approved", k.rfi) } else { "being chosen".to_string() }
                    ));
                }
            } else {
                // The design's pieces it replaces: all there, still design pieces, shown.
                let (mut gone, mut not_design, mut hidden) = (0usize, 0usize, 0usize);
                for h in k.replaces.iter().take(MAX_PIECES).filter_map(|h| handle_of(h)) {
                    match cx.d.model.get(h) {
                        None => gone += 1,
                        Some(e) => {
                            let ps = props::read(&e.common.xdata);
                            if text(&ps, super::mepatoms::ATOM_KEY).is_empty() || !text(&ps, super::valueeng::VE_ATOM).is_empty() {
                                not_design += 1;
                            }
                            if !e.common.visible {
                                hidden += 1;
                            }
                        }
                    }
                }
                if gone > 0 {
                    fails.push(format!("{gone} of the {} design pieces it replaces are gone from the model", k.replaces.len()));
                }
                if not_design > 0 {
                    fails.push(format!("{not_design} of the design pieces it replaces are no longer design pieces"));
                }
                if hidden > 0 && !k.shown {
                    fails.push(format!("{hidden} of the design pieces it replaces are hidden while it isn't shown"));
                } else if hidden > 0 {
                    notes.push(format!("shown now (a view): {hidden} design pieces hidden until it is hidden again"));
                }
                // Its own pieces: never takeoff pieces, on their own layer, off unless shown.
                let (mut counted, mut elsewhere) = (0usize, 0usize);
                for h in k.pieces.iter().take(MAX_PIECES).filter_map(|h| handle_of(h)) {
                    let Some(e) = cx.d.model.get(h) else { continue };
                    let ps = props::read(&e.common.xdata);
                    if !text(&ps, super::mepatoms::ATOM_KEY).is_empty() {
                        counted += 1;
                    }
                    if !k.layer.is_empty() && !e.common.layer.eq_ignore_ascii_case(&k.layer) {
                        elsewhere += 1;
                    }
                }
                if counted > 0 {
                    fails.push(format!("{counted} of its pieces are takeoff pieces (counted as the design)"));
                }
                if elsewhere > 0 {
                    fails.push(format!("{elsewhere} of its pieces are off its layer {}", k.layer));
                }
                if let Some(l) = cx.d.layer(&k.layer)
                    && l.on
                    && !l.frozen
                    && !k.shown
                {
                    fails.push(format!("its layer {} is on while it isn't shown", k.layer));
                }
                // Its circuits keep the designed method.
                let changed = cx
                    .sp
                    .tags
                    .iter()
                    .filter(|t| t.kind == "circuit" && k.circuits.contains(&t.id))
                    .filter(|t| {
                        t.fields.get("value_engineering").is_some_and(|f| f.value.contains(&k.id) || (!k.rfi.is_empty() && f.value.contains(&k.rfi)))
                    })
                    .count();
                if changed > 0 {
                    fails.push(format!("{changed} of its circuits carry its method as their design"));
                }
                if k.decision == "approved" && rfi_status != Some(cadaddy_walk3d::RfiStatus::Approved) && !k.rfi.is_empty() {
                    notes.push(format!("decided approved but its RFI {} isn't recorded approved", k.rfi));
                }
            }
            let verdict = if fails.is_empty() { "pass" } else { "fail" };
            push(
                DESIGN,
                r,
                verdict,
                if fails.is_empty() {
                    if k.adopted {
                        format!(
                            "{label} is the design: {}",
                            if k.rfi.is_empty() { "chosen (allowed now)".to_string() } else { format!("RFI {} approved", k.rfi) }
                        )
                    } else {
                        format!(
                            "{label} leaves the as-designed model as it is ({} design pieces, {} of its own hidden)",
                            k.replaces.len(),
                            k.pieces.len()
                        )
                    }
                } else {
                    format!("{label} changed the as-designed model: {}", fails.join("; "))
                },
                "the as-designed model unchanged until the scenario is approved (RFI) or chosen (allowed now)".into(),
                if k.adopted { format!("adopted ({})", k.decision) } else { "not adopted".into() },
                json!({"check": "design", "adopted": k.adopted, "decision": k.decision, "rfi": k.rfi, "rfi_status": rfi_status.map(|s| s.name()), "shown": k.shown, "replaces": k.replaces.len(), "pieces": k.pieces.len(), "fails": fails, "notes": notes}),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_data_reads_and_says_where_each_cable_is_never_used() {
        let d = data();
        assert_eq!(method_of(&d, "NM"), Some("nm".into()));
        assert_eq!(method_of(&d, "SER"), Some("se".into()));
        assert_eq!(method_of(&d, "MC"), Some("mc".into()));
        for a in ["common", "exposed", "plenum", "below_slab"] {
            assert!(d.never.get("nm").is_some_and(|n| n.contains_key(a)), "NM never {a}");
        }
        assert!(!d.never.get("se").is_some_and(|n| n.contains_key("common")), "SER feeders pass through common areas to the units");
        assert!(d.code.iter().any(|c| c.cite == "NEC 334.10(3)" && c.thermal_barrier && c.construction.contains(&"V".to_string())));
        assert_eq!(class_of("V-A"), "V");
        assert_eq!(class_of("Type III-B"), "III");
        assert_eq!(cite_key("NEC 334.10 (3)"), cite_key("nec 334.10(3)"));
        // No job words in the data.
        let up = DATA.to_ascii_uppercase();
        for w in ["QUEEN", "KING", "HOTEL", "DP1", "MDP"] {
            assert!(!up.contains(w), "{w}");
        }
    }

    #[test]
    fn a_price_book_matches_an_item_by_its_size_else_a_row_with_no_size() {
        let b = Book {
            rows: vec![
                BookRow { item: "NM-B cable".into(), size: "12/2 w/G".into(), material: 0.5, labor_dollars: 0.75 },
                BookRow { item: "Staple".into(), size: String::new(), material: 0.02, labor_dollars: 0.1 },
            ],
            manpower_rate: Some(50.0),
            path: "book.csv".into(),
        };
        assert!(b.find("nm-b CABLE", "12/2 W/G").is_some_and(|r| (r.labor_dollars - 0.75).abs() < 1e-9));
        assert!(b.find("NM-B cable", "10/2 w/G").is_none());
        assert!(b.find("Staple", "anything").is_some());
        assert!(close(100.0, 100.9, 1.0, 0.0) && !close(100.0, 101.5, 1.0, 0.0));
        assert!(close(0.0, 0.04, 1.0, ABS_QTY));
    }

    #[test]
    fn a_study_is_read_from_its_answer_or_a_job_result() {
        let a = json!({"scenarios": [{"id": "x"}]});
        assert!(study_of(&a).is_some());
        let j = json!({"value_engineering": {"scenarios": [], "pricing": {}}});
        assert!(study_of(&j).is_some());
        assert!(study_of(&json!({"value_engineering": {"facts": {}}})).is_none());
        assert!(study_of(&json!([1, 2])).is_none());
    }
}
