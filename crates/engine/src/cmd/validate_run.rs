//! The Validate run (Q6116): CADaddy drives, a connected agent judges, findings come back as
//! review items with the evidence side by side.
//!
//! CADaddy talks to the network only as PRIVACY.md says, so the judge is the agent connected over
//! MCP or the control channel. CADaddy holds the rules (`validate.rules`), makes the evidence
//! packets (`validate.packet`), hands them out one area at a time and keeps the results:
//!
//! - `validate {scope: selection|area|room|level|building, area?, room?, level?, trades?, rules?,
//!   out_dir?, force?}` starts a run, or resumes the run of the same scope, trades and rules:
//!   lists its areas, runs `bim.validate` and the measured checks of `validate.prescreen` on the
//!   scope first (the deterministic pre-screen) and returns `{run, areas, next: "validate.next"}`. An area whose model and rules are unchanged
//!   since it was judged keeps its findings (`force` judges it again).
//! - `validate.next {run}`: the next area's packet (manifest, folder, prompt) for the judge.
//! - `validate.report {run, area, findings}`: checks the findings' shape against the packet (rule
//!   ids it gave, handles of the drawing, pictures of the packet) and keeps them; everything
//!   but a pass becomes a review item of kind `validation`, ranked by severity × confidence.
//! - `validate.status {run?}`: areas done, findings by trade, rule and verdict, the accuracy per
//!   trade and per rule (pass / (pass + fail + missing + extra)), the judge's false-alarm rate
//!   from the items a person rejected, and on a planted copy the planted errors caught and missed.
//! - `validate.plant {count, kinds?, seed}`: a copy of the drawing (opened beside it) with known
//!   errors planted, recorded in the copy only; the same seed plants the same errors.
//! - `validate.cancel {run?}`: stops handing out a run's areas (the Validate panel's Cancel); what
//!   was judged stays, and `validate` with the same scope resumes it.
//!
//! Runs live in the drawing ([`cadaddy_doc::Drawing::agent`], key `validate`) beside the review
//! queue's state, so they survive save and reopen and are undone like any edit.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cadaddy_doc::{Drawing, EntityKind, Handle, RawTag};
use cadaddy_walk3d::validate::{self as v, Model};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::bspec::current;
use super::validate::{Frame, PROV_APP, model_of};
use super::validate_packet::{self as vp, Area};
use super::validate_rules::{self as vr, Effective};
use super::*;
use crate::{Result, Session};

pub const CMD_START: &str = "validate";
pub const CMD_NEXT: &str = "validate.next";
pub const CMD_REPORT: &str = "validate.report";
pub const CMD_STATUS: &str = "validate.status";
pub const CMD_PLANT: &str = "validate.plant";
pub const CMD_CANCEL: &str = "validate.cancel";

/// What the status bar says while a run started in the app has areas left and no agent watches.
pub const WAITING: &str = "Validate: waiting for an agent";

/// The drawing's agent-state keys: the runs, and a planted copy's record.
const KEY: &str = "validate";
const PLANT_KEY: &str = "validate_plant";
const RUNS_KEPT: usize = 10;
const MAX_RUN_AREAS: usize = 10_000;
/// Findings one `validate.report` call carries, at most: a larger area comes in parts.
const MAX_PART: usize = 1_000;
/// Handles one finding may name (the same evidence and verdict for each).
const MAX_HANDLES: usize = 500;
/// Per rule, the findings of what the model lacks (`missing`, no handle) an area may hold beyond
/// its elements × rules.
const MISSING_SLACK: usize = 100;
/// Parts of one area's report, at most.
const MAX_PARTS: u64 = 10_000;
const WARNINGS_SHOWN: usize = 10;
/// Objects of a packet's `objects.json` read for checking a report, at most (per list).
const MAX_PACKET_OBJECTS: usize = 5_000;
const MAX_EVIDENCE: usize = 8;
const TEXT_MAX: usize = 600;
const WHAT_MAX: usize = 300;
const PROBLEMS_SHOWN: usize = 10;
const MAX_PLANT: usize = 50;
const DEFAULT_PLANT: usize = 6;
/// How far a planted move takes a device, feet.
const PLANT_MOVE_FT: f64 = 3.0;
pub const VERDICTS: [&str; 5] = ["pass", "fail", "missing", "extra", "unsure"];
pub const SCOPES: [&str; 5] = ["selection", "area", "room", "level", "building"];
pub const PLANT_KINDS: [&str; 6] = ["move", "delete", "duct_size", "panel_mount", "fan_room", "drop_gfci"];
const LOOP: &str =
    "validate → validate.next → judge the packet by its rules (prompt.md) → validate.report → validate.next … until done → validate.status";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD_START, "Validate Model Against Prints (Agent)", run_start).params(
            "{scope?: selection|area|room|level|building (default from what is given: area, else room, else level, else building), area?: id from validate.areas | {area: [x0,y0,x1,y1] (feet), level} (scope area), room?: number or name (scope room; with level: on that level), level?: name (scope level), trades?: [architecture|electrical|mechanical|plumbing|fire|general], rules?: [rule id], out_dir?: folder for the packets, each run in a folder of its own named by its id (default: a folder named after the drawing plus \" validate\", beside it), force?: bool (judge unchanged areas again)} → {run, areas: [{id, status}], areas_total, pending, unchanged, prescreen, out_dir, next: \"validate.next\", loop, waiting?}: starts (or resumes) a Validate run. The judge is the connected agent: it loops validate.next → judges the packet by its rules → validate.report, until done; then validate.status",
        ),
        CommandSpec::new(CMD_NEXT, "Next Validation Packet", run_next).files().params(
            "{run?: id (the latest), area?: id (this one again)} → {run, area, packet: {dir, manifest, prompt, objects, rules}, rules, images, prompt?, remaining, report: {call: \"validate.report\", params}} | {run, done: true, next: \"validate.status\"}: writes the next area's evidence packet and hands it to the judge; read prompt.md and the pictures, judge every rule for every element, then validate.report (a large area in parts: {part: 1}, {part: 2}, … {final: true})",
        ),
        CommandSpec::new(CMD_REPORT, "Report Validation Findings", run_report).params(
            "{run?: id (the latest), area: id, part?: n | true (one part of the area's report: added to the parts before it), final?: bool (true: the last part, the area is done; false: more parts follow), findings: [{rule: id from the packet's rules, handle?: model handle (none for missing; required for extra), handles?: [model handle] (several elements the same evidence and verdict cover; counted once per handle), verdict: pass|fail|missing|extra|unsure, issue, expected, found, evidence: [{image: packet file (prints/… for fail, missing, extra), sheet, what}], confidence: 0-1, severity?: high|medium|low}] (at most 1000 per call)} → {run, area, stored (per handle), by_verdict, review_items, area_findings, part, final, pairs: {asked, judged, coverage}, merged?, conflicts?, warnings?, remaining, next}: keeps the judge's findings for the area; without part or final the report is the area's whole report (replacing an earlier one); a large area comes in parts, the area is done on the final one, and an area holds up to its elements × rules findings; a duplicate (the same rule, handle and verdict; for missing, the same rule and issue) is merged into the first with a warning; a report with a bad finding is refused whole, with every problem said; fail, missing, extra and unsure become review items (kind validation)",
        ),
        CommandSpec::new(CMD_STATUS, "Validation Run Status", run_status)
            .params(
                "{run?: id (the latest)} → {run, scope, trades, rules, cancelled?, areas: {total, done, issued, pending, unchanged, in_parts}, findings: {total (per handle), by_verdict, by_trade, by_rule}, accuracy: {overall, judged_pairs, unsure_pairs, pairs_asked, coverage, by_trade, by_rule, coverage_by_rule, products?: {by_kind: {kind: accuracy}, by_field: {field: accuracy}} (G-PRODUCT and G-PRODUCT-CODE as the run's prescreen measured them)} (pass / (pass+fail+missing+extra) over the element × rule pairs judged, a pair judged twice by its worse verdict; coverage: the pairs the packets asked for that a pass, fail or extra judged), products?: {by_kind, by_field} (the counts), review: {accepted, rejected, open, deferred, false_alarm_rate}, prescreen, planted?: {total, caught, missed, recall, errors}, runs, per_run: [{run, scope, areas, findings, accuracy, judged_pairs, pairs_asked, coverage}], totals: {runs, areas_judged, findings, accuracy} (every run of the drawing, an area judged in several runs once, from the latest), next}: how far the run is and how the model scores, and the totals over every run",
            )
            .noundo(),
        CommandSpec::new(CMD_PLANT, "Plant Validation Errors", run_plant)
            .params(
                "{count?: 1-50 (6), kinds?: [move|delete|duct_size|panel_mount|fan_room|drop_gfci], seed?: n (1)} → {copy, planted, by_kind, not_planted, seed}: opens a copy of the drawing with known errors planted (a device moved 3 ft, one deleted, a duct size changed, a panel's mounting flipped, a fan moved to the next room, a GFCI type dropped); they are recorded in the copy only; run validate on the copy and validate.status says which were caught",
            )
            .noundo(),
        CommandSpec::new(CMD_CANCEL, "Cancel Validation Run", run_cancel).params(
            "{run?: id (the latest)} → {run, cancelled: true, areas, changed}: stops handing out the run's areas (validate.next refuses it and the status bar stops waiting for an agent); the findings already reported stay, and validate with the same scope, trades and rules resumes it",
        ),
    ]
}

// ---------------------------------------------------------------- state

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Store {
    seq: u64,
    runs: Vec<Run>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Run {
    id: String,
    scope: Value,
    trades: Vec<String>,
    /// The rules asked for (none: every rule that applies).
    rules: Vec<String>,
    out_dir: String,
    /// `agent` or `user`: who started it.
    by: String,
    /// When it was (re)started, in the store's sequence.
    started: u64,
    prescreen: Value,
    areas: Vec<RunArea>,
    /// Areas the scope had past the cap.
    left_out: usize,
    /// Cancelled (`validate.cancel`): no areas are handed out until `validate` resumes it.
    cancelled: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct RunArea {
    id: String,
    kind: String,
    level: String,
    label: String,
    bbox: [f64; 4],
    /// The model and rules round the area when it was handed out (a hash).
    fp: String,
    /// `pending`, `issued` or `done`.
    status: String,
    /// Kept from an earlier pass of the run: unchanged since it was judged.
    unchanged: bool,
    /// The packet's folder, under the run's `out_dir`.
    dir: String,
    /// The packet's pictures.
    images: Vec<String>,
    /// Its print crops: (file, sheet, the model drawn over it).
    prints: Vec<(String, String, Option<String>)>,
    /// The rules it gave the judge.
    rules: Vec<String>,
    findings: Vec<Finding>,
    /// The parts of its report since it was handed out (`validate.report {part}`); the area is
    /// done on the `final` one.
    parts: usize,
    /// Per rule, the element × rule pairs its packet asks the judge for (each of its own elements
    /// and runs by each of the packet's rules that applies to it) and how many of them a pass,
    /// fail or extra judged (an `unsure` pair is not judged): counted when a report comes in.
    pairs: BTreeMap<String, [usize; 2]>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Finding {
    id: String,
    rule: String,
    trade: String,
    handle: Option<String>,
    /// More elements the same evidence and verdict cover, beyond `handle`: a finding counts once
    /// per handle.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    also: Vec<String>,
    verdict: String,
    issue: String,
    expected: String,
    found: String,
    evidence: Vec<Evidence>,
    confidence: f64,
    severity: String,
}

impl Finding {
    /// Every handle it names (none for `missing`).
    fn handles(&self) -> impl Iterator<Item = &str> {
        self.handle.iter().chain(self.also.iter()).map(String::as_str)
    }

    /// How many findings it counts as: one per handle, one when it names none.
    fn weight(&self) -> usize {
        self.handles().count().max(1)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Evidence {
    image: String,
    sheet: String,
    what: String,
}

/// A planted copy's record: what was changed, never shown in a packet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Plant {
    seed: u64,
    asked: usize,
    errors: Vec<Planted>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Planted {
    n: usize,
    kind: String,
    handle: String,
    /// The element's kind and mark.
    element: String,
    mark: Option<String>,
    level: String,
    /// Where it was, and where it went (moves), feet.
    at_ft: [f64; 2],
    to_ft: Option<[f64; 2]>,
    from: String,
    to: String,
}

fn root(d: &Drawing) -> serde_json::Map<String, Value> {
    match d.agent.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()) {
        Some(Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    }
}

fn load(d: &Drawing) -> Store {
    root(d).get(KEY).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

fn plant_of(d: &Drawing) -> Option<Plant> {
    root(d).get(PLANT_KEY).and_then(|v| serde_json::from_value(v.clone()).ok())
}

fn save(s: &mut Session, st: &Store) -> Result<()> {
    let doc = s.doc_mut()?;
    let mut r = root(doc);
    r.insert(KEY.into(), serde_json::to_value(st).unwrap_or(Value::Null));
    doc.agent = serde_json::to_string(&Value::Object(r)).ok();
    Ok(())
}

// ---------------------------------------------------------------- small helpers

/// FNV-1a over a text: stable ids and fingerprints.
fn hash64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

pub(crate) fn hash8(s: &str) -> String {
    format!("{:08x}", hash64(s) & 0xffff_ffff)
}

fn cut(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n { s.to_string() } else { format!("{}…", s.chars().take(n.saturating_sub(1)).collect::<String>()) }
}

fn r2(x: f64) -> f64 {
    let r = (x * 100.0).round() / 100.0;
    if r == 0.0 { 0.0 } else { r }
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

fn grow(r: [f64; 4], by: f64) -> [f64; 4] {
    [r[0] - by, r[1] - by, r[2] + by, r[3] + by]
}

fn inside(r: [f64; 4], p: [f64; 2]) -> bool {
    p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3]
}

fn segment_touches(r: [f64; 4], a: [f64; 2], b: [f64; 2]) -> bool {
    (0..=16).any(|i| {
        let t = f64::from(i) / 16.0;
        inside(r, [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t])
    })
}

/// A file name for an area id.
fn folder_of(id: &str) -> String {
    let s: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).take(80).collect();
    let h = hash8(id);
    format!("{s}_{}", h.get(..4).unwrap_or(&h))
}

fn label_of(a: &Area) -> String {
    match (&a.number, &a.name) {
        (Some(n), Some(m)) => format!("{n} {m}"),
        (Some(n), None) => n.clone(),
        (None, Some(m)) => m.clone(),
        _ => a.id.clone(),
    }
}

fn severity_weight(s: &str) -> f64 {
    match s {
        "high" => 3.0,
        "medium" => 2.0,
        _ => 1.0,
    }
}

fn is_problem(verdict: &str) -> bool {
    matches!(verdict, "fail" | "missing" | "extra")
}

/// The fingerprint of what the judge would see of an area: the model round it and the rules.
fn area_fp(model: &Model, area: &Area, rules_fp: &str) -> String {
    let reach = grow(area.bbox, 1.0);
    let lvl = |l: &str| l.eq_ignore_ascii_case(&area.level);
    let mut parts: Vec<String> = Vec::new();
    for e in model.elements.iter().filter(|e| lvl(&e.level) && inside(reach, e.at)) {
        parts.push(format!(
            "e|{}|{}|{:?}|{:?}|{:.2},{:.2}|{:?}|{:?}|{:?}|{:?}|{:?}",
            e.id,
            e.kind,
            e.mark,
            e.tag,
            e.at[0],
            e.at[1],
            e.z_in.map(r2),
            e.mounting,
            e.status,
            e.source,
            e.fields
        ));
    }
    for r in model.runs.iter().filter(|r| lvl(&r.level) && r.path.windows(2).any(|w| matches!(w, [a, b] if segment_touches(reach, *a, *b)))) {
        let path: Vec<String> = r.path.iter().map(|p| format!("{:.2},{:.2}", p[0], p[1])).collect();
        parts.push(format!("r|{}|{}|{:?}|{}|{:?}", r.id, r.kind, r.mark, path.join(";"), r.fields));
    }
    for w in model.walls.iter().filter(|w| lvl(&w.level) && segment_touches(reach, w.a, w.b)) {
        parts.push(format!("w|{}|{:.2},{:.2}|{:.2},{:.2}|{:.2}|{:?}", w.id, w.a[0], w.a[1], w.b[0], w.b[1], w.thickness_in, w.wall_type));
    }
    for o in model.openings.iter().filter(|o| lvl(&o.level) && inside(reach, [(o.min[0] + o.max[0]) / 2.0, (o.min[1] + o.max[1]) / 2.0])) {
        parts.push(format!("o|{}|{}|{:.2},{:.2}|{:.2},{:.2}", o.id, o.kind, o.min[0], o.min[1], o.max[0], o.max[1]));
    }
    parts.sort();
    format!("{:016x}", hash64(&format!("{rules_fp}\n{}", parts.join("\n"))))
}

fn trades_param(cmd: &str, p: &Value) -> Result<Vec<String>> {
    match p.get("trades") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => {
            let mut out: Vec<String> = a
                .iter()
                .take(16)
                .map(|t| {
                    t.as_str()
                        .map(|x| x.trim().to_ascii_lowercase())
                        .filter(|x| vr::TRADES.contains(&x.as_str()))
                        .ok_or_else(|| bad(cmd, format!("unknown trade {t} ({})", vr::TRADES.join(", "))))
                })
                .collect::<Result<Vec<_>>>()?;
            out.sort();
            out.dedup();
            Ok(out)
        }
        Some(_) => Err(bad(cmd, "`trades` is a list of trades")),
    }
}

fn rules_param(cmd: &str, p: &Value, all: &[Effective]) -> Result<Vec<String>> {
    match p.get("rules") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => {
            let mut out: Vec<String> = a
                .iter()
                .take(100)
                .map(|r| {
                    r.as_str()
                        .map(|x| x.trim().to_ascii_uppercase())
                        .filter(|x| all.iter().any(|e| e.rule.id == *x))
                        .ok_or_else(|| bad(cmd, format!("unknown rule {r} (validate.rules lists them)")))
                })
                .collect::<Result<Vec<_>>>()?;
            out.sort();
            out.dedup();
            Ok(out)
        }
        Some(_) => Err(bad(cmd, "`rules` is a list of rule ids (validate.rules)")),
    }
}

/// The rules a run gives its packets: the asked ones, else those of the asked trades (and the
/// general ones), else none named (every rule that applies).
fn packet_rules(trades: &[String], asked: &[String], all: &[Effective]) -> Vec<String> {
    if !asked.is_empty() {
        return asked.to_vec();
    }
    if trades.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<String> =
        all.iter().filter(|e| e.enabled && (e.rule.trade == "general" || trades.contains(&e.rule.trade))).map(|e| e.rule.id.clone()).collect();
    out.sort();
    out
}

fn rules_fp(all: &[Effective], rules: &[String]) -> String {
    let text: Vec<String> = all
        .iter()
        .filter(|e| rules.is_empty() || rules.contains(&e.rule.id))
        .map(|e| format!("{}|{}|{}", e.rule.id, e.enabled, e.to_json(true)))
        .collect();
    format!("{:016x}", hash64(&text.join("\n")))
}

fn run_param<'a>(cmd: &str, st: &'a Store, p: &Value) -> Result<&'a Run> {
    match str_param(p, "run").map(str::trim).filter(|r| !r.is_empty()) {
        Some(id) => st.runs.iter().find(|r| r.id.eq_ignore_ascii_case(id)).ok_or_else(|| {
            let known: Vec<&str> = st.runs.iter().map(|r| r.id.as_str()).collect();
            bad(
                cmd,
                format!(
                    "there is no validate run {} (runs: {})",
                    cut(id, 40),
                    if known.is_empty() { "none yet — validate {scope} starts one".into() } else { known.join(", ") }
                ),
            )
        }),
        None => st.runs.iter().max_by_key(|r| r.started).ok_or_else(|| bad(cmd, "no validate run yet: validate {scope} starts one")),
    }
}

fn run_index(st: &Store, id: &str) -> Option<usize> {
    st.runs.iter().position(|r| r.id == id)
}

/// The scope a call without `scope` asks for, from what it gives: `area`, else `room`, else
/// `level`; the whole building when it gives none of them.
pub(crate) fn inferred_scope(p: &Value) -> &'static str {
    ["area", "room", "level"].into_iter().find(|k| p.get(*k).is_some_and(|v| !v.is_null())).unwrap_or("building")
}

/// The areas of a scope (`scope`, else inferred from what is given: [`inferred_scope`]), and
/// the scope as kept in the run.
pub(crate) fn scope_areas(
    s: &Session,
    cmd: &str,
    p: &Value,
    d: &Drawing,
    sp: &cadaddy_walk3d::BuildingSpec,
    model: &Model,
    frame: &Frame,
) -> Result<(Value, Vec<Area>)> {
    let scope = str_param(p, "scope").map(|x| x.trim().to_ascii_lowercase()).unwrap_or_else(|| inferred_scope(p).into());
    if !SCOPES.contains(&scope.as_str()) {
        return Err(bad(cmd, format!("unknown scope {} ({})", cut(&scope, 30), SCOPES.join(", "))));
    }
    let all = vp::model_areas(d, frame);
    let none_yet = || {
        bad(
            cmd,
            "no model rooms to check yet (model.build makes them from the plan's room tags): give scope area with a window {area: [x0, y0, x1, y1], level}",
        )
    };
    let (key, areas): (Value, Vec<Area>) = match scope.as_str() {
        "building" => {
            if all.is_empty() {
                return Err(none_yet());
            }
            (json!({"scope": "building"}), all)
        }
        "level" => {
            let l = str_param(p, "level").map(str::trim).filter(|l| !l.is_empty()).ok_or_else(|| bad(cmd, "scope level needs `level`"))?;
            let name = sp.level(l).map(|x| x.name.clone()).ok_or_else(|| bad(cmd, format!("there is no level named {}", cut(l, 40))))?;
            let a: Vec<Area> = all.into_iter().filter(|a| a.level.eq_ignore_ascii_case(&name)).collect();
            if a.is_empty() {
                return Err(bad(cmd, format!("level {name} has no model rooms to check yet (model.build makes them)")));
            }
            (json!({"scope": "level", "level": name}), a)
        }
        "room" => {
            let r = str_param(p, "room")
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .ok_or_else(|| bad(cmd, "scope room needs `room` (its number or name)"))?;
            // On one level when the call names it (a number may repeat on another floor).
            let level = match str_param(p, "level").map(str::trim).filter(|l| !l.is_empty()) {
                Some(l) => Some(sp.level(l).map(|x| x.name.clone()).ok_or_else(|| bad(cmd, format!("there is no level named {}", cut(l, 40))))?),
                None => None,
            };
            let a: Vec<Area> =
                all.into_iter().filter(|a| vp::names_room(a, r) && level.as_ref().is_none_or(|l| a.level.eq_ignore_ascii_case(l))).collect();
            if a.is_empty() {
                return Err(bad(cmd, format!("no model room is numbered or named {} (validate.areas lists them)", cut(r, 40))));
            }
            match level {
                Some(l) => (json!({"scope": "room", "room": r, "level": l}), a),
                None => (json!({"scope": "room", "room": r}), a),
            }
        }
        "area" => {
            if p.get("area").is_none_or(Value::is_null) {
                return Err(bad(cmd, "scope area needs `area`: an id from validate.areas, or {area: [x0, y0, x1, y1] (feet), level}"));
            }
            let a = vp::area_param(cmd, sp, p, d, frame)?;
            (json!({"scope": "area", "area": a.id}), vec![a])
        }
        _ => {
            // The selection: rooms selected, and the rooms holding selected elements or runs.
            let sel: BTreeSet<String> = s.selection().iter().map(|h| h.hex().to_ascii_uppercase()).collect();
            if sel.is_empty() {
                return Err(bad(cmd, "scope selection: nothing is selected (select rooms or model elements first)"));
            }
            let up = |h: &str| h.to_ascii_uppercase();
            let points: Vec<(String, [f64; 2])> =
                model.elements.iter().filter(|e| sel.contains(&up(&e.id))).map(|e| (e.level.clone(), e.at)).collect();
            let paths: Vec<(&str, &[[f64; 2]])> =
                model.runs.iter().filter(|r| sel.contains(&up(&r.id))).map(|r| (r.level.as_str(), r.path.as_slice())).collect();
            let a: Vec<Area> = all
                .into_iter()
                .filter(|a| {
                    a.room.as_deref().is_some_and(|h| sel.contains(&up(h)))
                        || points.iter().any(|(l, q)| l.eq_ignore_ascii_case(&a.level) && inside(a.bbox, *q))
                        || paths.iter().any(|(l, path)| {
                            l.eq_ignore_ascii_case(&a.level) && path.windows(2).any(|w| matches!(w, [x, y] if segment_touches(a.bbox, *x, *y)))
                        })
                })
                .collect();
            if a.is_empty() {
                return Err(bad(
                    cmd,
                    "the selection holds no model room and no element inside one (select rooms or elements, or use scope area with a window)",
                ));
            }
            let handles: Vec<&String> = sel.iter().collect();
            (json!({"scope": "selection", "selection": hash8(&format!("{handles:?}")), "selected": sel.len()}), a)
        }
    };
    Ok((key, areas))
}

/// The folder packets go in by default: beside the drawing, `<name> validate` (each run's packets
/// in a folder of its own: `<out_dir>/<run>/<area>`).
fn default_out_dir(s: &Session) -> Option<String> {
    let path = s.state().ok()?.path.clone()?;
    let p = std::path::Path::new(&path);
    let stem = p.file_stem()?.to_string_lossy().into_owned();
    let dir = p.parent()?.join(format!("{stem} validate"));
    Some(dir.to_string_lossy().into_owned())
}

/// The deterministic pre-screen: `bim.validate` round the scope's areas, level by level (the
/// measured checks of `validate.prescreen` are added by the caller).
fn prescreen(s: &mut Session, frame: &Frame, scope: &str, areas: &[Area]) -> Value {
    let mut by_level: BTreeMap<String, [f64; 4]> = BTreeMap::new();
    for a in areas {
        let b = a.bbox;
        by_level.entry(a.level.clone()).and_modify(|u| *u = [u[0].min(b[0]), u[1].min(b[1]), u[2].max(b[2]), u[3].max(b[3])]).or_insert(b);
    }
    let mut total = 0u64;
    let mut by_check: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_severity: BTreeMap<String, u64> = BTreeMap::new();
    let mut errors: Vec<String> = Vec::new();
    for (level, b) in by_level.iter().take(200) {
        let mut q = json!({"level": level, "limit": 1});
        if !matches!(scope, "building" | "level") {
            let lo = frame.to_sheet(level, [b[0] - 1.0, b[1] - 1.0]);
            let hi = frame.to_sheet(level, [b[2] + 1.0, b[3] + 1.0]);
            q["area"] = json!([lo.x, lo.y, hi.x, hi.y]);
        }
        match s.execute("bim.validate", &q) {
            Ok(r) => {
                total += r["total"].as_u64().unwrap_or(0);
                for (key, into) in [("byCheck", &mut by_check), ("bySeverity", &mut by_severity)] {
                    for (k, n) in r[key].as_object().into_iter().flatten() {
                        *into.entry(k.clone()).or_default() += n.as_u64().unwrap_or(0);
                    }
                }
            }
            Err(e) => errors.push(format!("{level}: {}", cut(&e.to_string(), 200))),
        }
    }
    let mut out = json!({"call": "bim.validate", "failures": total, "byCheck": by_check, "bySeverity": by_severity});
    if !errors.is_empty() {
        out["not_run"] = json!(errors);
    }
    out
}

fn area_counts(run: &Run) -> Value {
    let n = |st: &str| run.areas.iter().filter(|a| a.status == st).count();
    json!({
        "total": run.areas.len(),
        "done": n("done"),
        "issued": n("issued"),
        "pending": n("pending"),
        "unchanged": run.areas.iter().filter(|a| a.unchanged && a.status == "done").count(),
        // Handed out, with parts of its report in and the final one not yet.
        "in_parts": run.areas.iter().filter(|a| a.status == "issued" && a.parts > 0).count(),
    })
}

fn remaining(run: &Run) -> usize {
    run.areas.iter().filter(|a| a.status != "done").count()
}

/// Whether the active drawing has a run with areas left that the app started.
fn waiting_run(d: &Drawing) -> Option<String> {
    let st = load(d);
    st.runs.iter().filter(|r| r.by == "user" && !r.cancelled && remaining(r) > 0).max_by_key(|r| r.started).map(|r| r.id.clone())
}

/// What the status bar says about Validate: [`WAITING`] while a run started in the app has areas
/// left and no agent watches the session's events.
pub fn status_text(s: &Session) -> Option<&'static str> {
    if s.events.on {
        return None;
    }
    let d = s.doc().ok()?;
    // Cheap first: most drawings hold no run.
    if !d.agent.as_deref().is_some_and(|t| t.contains("\"validate\"")) {
        return None;
    }
    waiting_run(d).map(|_| WAITING)
}

// ---------------------------------------------------------------- validate

fn run_start(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = CMD_START;
    let force = p.get("force").and_then(Value::as_bool).unwrap_or(false);
    let trades = trades_param(CMD, p)?;
    let doc: Arc<Drawing> = Arc::clone(&s.state()?.doc);
    let d: &Drawing = &doc;
    let (all, _) = vr::effective(&d.project_rules).map_err(|e| bad(CMD, e))?;
    let asked = rules_param(CMD, p, &all)?;
    let (sp, _) = current(s, CMD)?;
    let (model, frame) = model_of(d, &sp);
    let (scope, mut areas) = scope_areas(s, CMD, p, d, &sp, &model, &frame)?;
    let left_out = areas.len().saturating_sub(MAX_RUN_AREAS);
    areas.truncate(MAX_RUN_AREAS);
    let id = format!("VR-{}", hash8(&format!("{scope}|{trades:?}|{asked:?}")));
    let mut st = load(d);
    let old: Option<Run> = run_index(&st, &id).and_then(|i| st.runs.get(i).cloned());
    let out_dir = match str_param(p, "out_dir").map(str::trim).filter(|x| !x.is_empty()) {
        Some(o) => o.to_string(),
        None => match old.as_ref().map(|r| r.out_dir.clone()).filter(|o| !o.is_empty()).or_else(|| default_out_dir(s)) {
            Some(o) => o,
            None => return Err(bad(CMD, "`out_dir` is required: the drawing isn't saved yet, so there is no folder beside it for the packets")),
        },
    };
    let rfp = rules_fp(&all, &packet_rules(&trades, &asked, &all));
    let mut kept = 0usize;
    let run_areas: Vec<RunArea> = areas
        .iter()
        .map(|a| {
            let fp = area_fp(&model, a, &rfp);
            let prev = old.as_ref().and_then(|r| r.areas.iter().find(|x| x.id == a.id));
            match prev {
                Some(x) if !force && x.status == "done" && x.fp == fp => {
                    kept += 1;
                    RunArea { unchanged: true, ..x.clone() }
                }
                _ => RunArea {
                    id: a.id.clone(),
                    kind: a.kind.to_string(),
                    level: a.level.clone(),
                    label: label_of(a),
                    bbox: [r2(a.bbox[0]), r2(a.bbox[1]), r2(a.bbox[2]), r2(a.bbox[3])],
                    fp,
                    status: "pending".into(),
                    // The run's own packet folder: written again without asking.
                    dir: prev.map(|x| x.dir.clone()).unwrap_or_default(),
                    ..RunArea::default()
                },
            }
        })
        .collect();
    let scope_name = scope["scope"].as_str().unwrap_or("building").to_string();
    let mut pre = prescreen(s, &frame, &scope_name, &areas);
    // The measured checks too (G-HOST, G-HEIGHT, G-PRESENT), with the run's rules.
    let prints: Option<v::Prints> = super::validate::prints_of(&sp).cloned().or_else(|| super::validate::prints_from_job(s, &frame));
    let only: Vec<String> = if asked.is_empty() {
        Vec::new()
    } else {
        asked.iter().filter(|r| super::validate_prescreen::RULES.contains(&r.as_str())).cloned().collect()
    };
    if asked.is_empty() || !only.is_empty() {
        let gear_docs = if super::validate_gear::wanted(&all, &only) { super::validate_gear::read_docs(s, &sp) } else { None };
        let measured = super::validate_prescreen::screen(&super::validate_prescreen::Scope {
            d,
            sp: &sp,
            model: &model,
            frame: &frame,
            prints: prints.as_ref(),
            rules: &all,
            areas: &areas,
            whole_levels: matches!(scope_name.as_str(), "building" | "level"),
            only: &only,
            gear_docs: gear_docs.as_ref(),
        });
        pre["measured"] = measured.summary();
    }
    let by = if s.agent.from_agent { "agent" } else { "user" };
    st.seq += 1;
    let run = Run {
        id: id.clone(),
        scope: scope.clone(),
        trades: trades.clone(),
        rules: asked.clone(),
        out_dir: out_dir.clone(),
        by: by.into(),
        started: st.seq,
        prescreen: pre.clone(),
        areas: run_areas,
        left_out,
        cancelled: false,
    };
    let pending = remaining(&run);
    let listed: Vec<Value> = run.areas.iter().map(|a| json!({"id": a.id, "status": a.status})).collect();
    match run_index(&st, &id) {
        Some(i) => {
            if let Some(r) = st.runs.get_mut(i) {
                *r = run;
            }
        }
        None => st.runs.push(run),
    }
    st.runs.sort_by_key(|r| r.started);
    let n = st.runs.len();
    if n > RUNS_KEPT {
        st.runs.drain(..n - RUNS_KEPT);
    }
    save(s, &st)?;
    s.event("validate.started", json!({"run": id, "areas": listed.len(), "pending": pending, "by": by, "next": CMD_NEXT}));
    let mut out = json!({
        "run": id,
        "scope": scope,
        "areas_total": listed.len(),
        "pending": pending,
        "unchanged": kept,
        "prescreen": pre,
        "out_dir": out_dir,
        "next": CMD_NEXT,
        "loop": LOOP,
    });
    if left_out > 0 {
        out["left_out"] = json!(format!("{left_out} more areas past the cap of {MAX_RUN_AREAS}: run the levels one by one"));
    }
    if by == "user" && !s.events.on && pending > 0 {
        out["waiting"] = json!(WAITING);
    }
    if pending == 0 {
        out["next"] = json!(CMD_STATUS);
        out["note"] = json!("every area is unchanged since it was judged: validate.status has the scores; force: true judges them again");
    }
    // The area list, as much as fits.
    let mut shown: Vec<Value> = Vec::new();
    let mut used = out.to_string().len() + 50;
    for a in listed {
        let n = a.to_string().len() + 1;
        if used + n > super::agentready::ANSWER_MAX {
            out["areas_more"] = json!("validate.status lists the counts; validate.next hands them out in order");
            break;
        }
        used += n;
        shown.push(a);
    }
    out["areas"] = json!(shown);
    Ok(out)
}

// ---------------------------------------------------------------- validate.cancel

fn run_cancel(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = CMD_CANCEL;
    let mut st = load(s.doc()?);
    let id = run_param(CMD, &st, p)?.id.clone();
    let i = run_index(&st, &id).ok_or_else(|| bad(CMD, format!("there is no validate run {id}")))?;
    let Some(run) = st.runs.get_mut(i) else { return Err(bad(CMD, format!("there is no validate run {id}"))) };
    let changed = !run.cancelled;
    run.cancelled = true;
    let areas = area_counts(run);
    if changed {
        save(s, &st)?;
        s.event("validate.cancelled", json!({"run": id}));
    }
    Ok(json!({"run": id, "cancelled": true, "areas": areas, "changed": changed, "next": "validate (the same scope resumes it)"}))
}

// ---------------------------------------------------------------- validate.next

fn read_json(path: &std::path::Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn run_next(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = CMD_NEXT;
    let mut st = load(s.doc()?);
    let run = run_param(CMD, &st, p)?.clone();
    if run.cancelled {
        return Err(bad(CMD, format!("run {} was cancelled: validate with the same scope resumes it", run.id)));
    }
    let want = str_param(p, "area").map(str::trim).filter(|a| !a.is_empty());
    let pick = match want {
        Some(a) => Some(
            run.areas
                .iter()
                .position(|x| x.id.eq_ignore_ascii_case(a))
                .ok_or_else(|| bad(CMD, format!("area {} is not in run {} (validate.status lists its areas)", cut(a, 60), run.id)))?,
        ),
        // The first pending area; an issued, unreported one only when none is pending.
        None => run.areas.iter().position(|x| x.status == "pending").or_else(|| run.areas.iter().position(|x| x.status == "issued")),
    };
    let Some(i) = pick else {
        return Ok(
            json!({"run": run.id, "done": true, "remaining": 0, "areas": area_counts(&run), "next": CMD_STATUS, "message": "every area is judged: validate.status has the scores"}),
        );
    };
    let Some(area) = run.areas.get(i).cloned() else { return Err(bad(CMD, "the run's area list changed: call validate.next again")) };
    // Handed out before and not reported: said, so the judge knows it is no new area.
    let again = want.is_none() && area.status == "issued";
    // Parts of its report already in are dropped: it is judged again from the start.
    let dropped: usize = area.findings.iter().map(Finding::weight).sum();
    let folder = if area.dir.is_empty() { format!("{}/{}", run.id, folder_of(&area.id)) } else { area.dir.clone() };
    let dir = std::path::Path::new(&run.out_dir).join(&folder);
    let mut q = json!({"area": area.id, "out_dir": dir.to_string_lossy(), "force": !area.dir.is_empty()});
    if !run.trades.is_empty() {
        q["trades"] = json!(run.trades);
    }
    let doc: Arc<Drawing> = Arc::clone(&s.state()?.doc);
    let (all, _) = vr::effective(&doc.project_rules).map_err(|e| bad(CMD, e))?;
    let only = packet_rules(&run.trades, &run.rules, &all);
    if !only.is_empty() {
        q["rules"] = json!(only);
    }
    s.execute(vp::CMD_PACKET, &q).map_err(|e| bad(CMD, format!("the packet of {}: {e}", area.id)))?;
    let manifest = read_json(&dir.join("manifest.json")).ok_or_else(|| bad(CMD, format!("the packet of {} has no manifest.json", area.id)))?;
    let files =
        |k: &str| -> Vec<String> { manifest[k].as_array().into_iter().flatten().filter_map(|x| x["file"].as_str().map(str::to_string)).collect() };
    let mut images = files("prints");
    images.extend(files("model"));
    images.extend(files("submittals"));
    let prints: Vec<(String, String, Option<String>)> = manifest["prints"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let f = c["file"].as_str()?.to_string();
            let over = manifest["model"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|m| m["of"].as_str() == Some(f.as_str()))
                .and_then(|m| m["file"].as_str())
                .map(str::to_string);
            Some((f, c["sheet"].as_str().unwrap_or_default().to_string(), over))
        })
        .collect();
    let rules: Vec<String> = read_json(&dir.join("rules.json"))
        .map(|r| r["rules"].as_array().into_iter().flatten().filter_map(|x| x["id"].as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    // The fingerprint now (the model may have changed since the run started).
    let (sp, _) = current(s, CMD)?;
    let (model, frame) = model_of(&doc, &sp);
    let fp = vp::model_areas(&doc, &frame)
        .into_iter()
        .find(|a| a.id == area.id)
        .or_else(|| vp::area_param(CMD, &sp, &json!({"area": area.id}), &doc, &frame).ok())
        .map(|a| area_fp(&model, &a, &rules_fp(&all, &only)))
        .unwrap_or_else(|| area.fp.clone());
    let left = remaining(&run).saturating_sub(1);
    if let Some(r) = run_index(&st, &run.id).and_then(|k| st.runs.get_mut(k))
        && let Some(a) = r.areas.get_mut(i)
    {
        a.status = "issued".into();
        a.unchanged = false;
        a.dir = folder;
        a.images = images.clone();
        a.prints = prints;
        a.rules = rules.clone();
        a.fp = fp;
        a.findings.clear();
        a.parts = 0;
        a.pairs.clear();
    }
    save(s, &st)?;
    let full = |f: &str| dir.join(f).to_string_lossy().into_owned();
    let mut out = json!({
        "run": run.id,
        "area": {"id": area.id, "kind": area.kind, "level": area.level, "label": area.label, "box_ft": area.bbox},
        "packet": {"dir": dir.to_string_lossy(), "manifest": full("manifest.json"), "prompt": full("prompt.md"), "objects": full("objects.json"), "rules": full("rules.json")},
        "rules": rules,
        "images": images,
        "remaining": left,
        "report": {"call": CMD_REPORT, "params": {"run": run.id, "area": area.id, "findings": [{"rule": "<rule id>", "handle": "<model handle>", "verdict": "pass|fail|missing|extra|unsure", "issue": "…", "expected": "…", "found": "…", "evidence": [{"image": "prints/…", "sheet": "…", "what": "…"}], "confidence": 0.9}]}},
    });
    if rules.is_empty() {
        out["note"] = json!("no rule applies to what this area holds: report it with findings: [] and go on");
    }
    if again {
        out["again"] = json!(true);
        out["note"] = json!(format!(
            "every pending area is handed out; this one ({}) was handed out before and not reported yet: judge it and validate.report it",
            area.id
        ));
    }
    if area.parts > 0 && area.status == "issued" {
        out["parts_dropped"] = json!({"parts": area.parts, "findings": dropped, "note": "the area is handed out again: report it from part 1"});
    }
    // The prompt itself when it fits; else its file.
    if let Ok(text) = std::fs::read_to_string(dir.join("prompt.md"))
        && out.to_string().len() + text.len() + 40 <= super::agentready::ANSWER_MAX
    {
        out["prompt"] = json!(text);
    }
    if out.to_string().len() > super::agentready::ANSWER_MAX {
        out["images"] = json!(images.len());
    }
    Ok(out)
}

// ---------------------------------------------------------------- validate.report

fn text_of(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(t)) => t.clone(),
        Some(other) => other.to_string(),
    }
}

/// The packet picture a finding cites, as the packet names it (a full path to it is accepted).
fn packet_image(images: &[String], dir: &std::path::Path, given: &str) -> Option<String> {
    let g = given.trim().replace('\\', "/");
    if let Some(f) = images.iter().find(|f| **f == g) {
        return Some(f.clone());
    }
    let full = dir.to_string_lossy().replace('\\', "/");
    images.iter().find(|f| g == format!("{}/{}", full.trim_end_matches('/'), f) || g.ends_with(&format!("/{f}"))).cloned()
}

/// A packet object as `validate.report` checks a finding against it.
struct PacketObject {
    kind: String,
    category: Option<String>,
    mark: Option<String>,
    /// Listed among the packet's elements or runs (not a wall or an opening).
    element: bool,
}

enum Held<'a> {
    /// One of the area's own objects.
    Own(&'a PacketObject),
    /// An element beside the area (in the margin, outside its outline), with its own room.
    Nearby(Option<&'a str>),
}

/// The objects of a packet's `objects.json`, by handle (upper case).
struct PacketObjects {
    own: BTreeMap<String, PacketObject>,
    nearby: BTreeMap<String, Option<String>>,
}

impl PacketObjects {
    fn of(o: &Value) -> PacketObjects {
        let s = |v: &Value, k: &str| v[k].as_str().map(str::to_string).filter(|x| !x.trim().is_empty());
        let mut own = BTreeMap::new();
        for k in ["elements", "runs", "walls", "openings"] {
            for e in o[k].as_array().into_iter().flatten().take(MAX_PACKET_OBJECTS) {
                let Some(h) = e["handle"].as_str() else { continue };
                let kind = s(e, "kind").unwrap_or_else(|| if k == "walls" { "wall".into() } else { String::new() });
                let element = matches!(k, "elements" | "runs");
                own.insert(h.trim().to_ascii_uppercase(), PacketObject { kind, category: s(e, "category"), mark: s(e, "mark"), element });
            }
        }
        let nearby = o["nearby"]
            .as_array()
            .into_iter()
            .flatten()
            .take(MAX_PACKET_OBJECTS)
            .filter_map(|e| Some((e["handle"].as_str()?.trim().to_ascii_uppercase(), s(e, "room"))))
            .filter(|(h, _)| !own.contains_key(h))
            .collect();
        PacketObjects { own, nearby }
    }

    fn what(&self, handle: &str) -> Option<Held<'_>> {
        let h = handle.trim().to_ascii_uppercase();
        self.own.get(&h).map(Held::Own).or_else(|| self.nearby.get(&h).map(|r| Held::Nearby(r.as_deref())))
    }

    /// Does the packet ask the judge for `rule` on this object? An element or run gets every
    /// rule that applies to it (a rule for every element too); a wall or an opening only a rule
    /// that names its kind.
    fn asks(o: &PacketObject, rule: &vr::Rule) -> bool {
        if rule.names_kinds() { rule.applies_to_element(&o.kind, o.category.as_deref(), o.mark.as_deref()) } else { o.element }
    }

    /// Per rule of the packet, the element × rule pairs it asks for.
    fn expected(&self, rules: &[String], all: &[Effective]) -> BTreeMap<String, usize> {
        rules
            .iter()
            .filter_map(|id| all.iter().find(|e| e.rule.id == *id))
            .map(|e| (e.rule.id.clone(), self.own.values().filter(|o| Self::asks(o, &e.rule)).count()))
            .collect()
    }
}

/// What a rule applies to, in words: its kinds, categories and mark patterns.
fn applies_words(r: &vr::Rule) -> String {
    let a = &r.applies_to;
    let mut parts: Vec<String> = a.kinds.iter().map(|k| k.replace('_', " ")).collect();
    parts.extend(a.categories.iter().cloned());
    parts.extend(a.marks.iter().map(|m| format!("marks {m}")));
    cut(&parts.join(", "), 200)
}

/// How bad a verdict is, for the one verdict of an element × rule pair judged more than once.
fn verdict_rank(v: &str) -> u8 {
    match v {
        "fail" => 4,
        "missing" => 3,
        "extra" => 2,
        "unsure" => 1,
        _ => 0,
    }
}

/// The element × rule pairs of an area's findings, each with its trade and its worst verdict:
/// one per handle of a finding, and one per finding that names no element (`missing`).
fn area_pairs(findings: &[Finding]) -> BTreeMap<(String, String), (String, String)> {
    let mut out: BTreeMap<(String, String), (String, String)> = BTreeMap::new();
    for f in findings {
        let keys: Vec<String> =
            if f.handle.is_none() && f.also.is_empty() { vec![format!("#{}", f.id)] } else { f.handles().map(str::to_ascii_uppercase).collect() };
        for k in keys {
            let e = out.entry((f.rule.clone(), k)).or_insert_with(|| (f.trade.clone(), f.verdict.clone()));
            if verdict_rank(&f.verdict) > verdict_rank(&e.1) {
                e.1 = f.verdict.clone();
            }
        }
    }
    out
}

/// Per rule, the pairs the packet asks for and how many of them the findings judged (pass, fail
/// or extra: not unsure).
fn area_coverage(
    findings: &[Finding],
    expected: &BTreeMap<String, usize>,
    packet: Option<&PacketObjects>,
    all: &[Effective],
) -> BTreeMap<String, [usize; 2]> {
    let mut judged: BTreeMap<String, usize> = BTreeMap::new();
    for ((rule, key), (_, verdict)) in area_pairs(findings) {
        if key.starts_with('#') || verdict == "unsure" {
            continue;
        }
        let asked = match (packet.and_then(|p| p.own.get(&key)), all.iter().find(|e| e.rule.id == rule)) {
            (Some(o), Some(r)) => PacketObjects::asks(o, &r.rule),
            _ => false,
        };
        if asked {
            *judged.entry(rule).or_default() += 1;
        }
    }
    expected.iter().map(|(r, n)| (r.clone(), [*n, judged.get(r).copied().unwrap_or(0).min(*n)])).collect()
}

/// A finding read from a report, with its number there.
struct Read {
    n: usize,
    finding: Finding,
    /// It named elements (some may be merged away as duplicates).
    named: bool,
}

/// `part`: the part's number, or `true`.
fn part_param(cmd: &str, p: &Value) -> Result<Option<u64>> {
    match p.get("part") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(None),
        Some(Value::Bool(true)) => Ok(Some(0)),
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=MAX_PARTS).contains(n))
            .map(Some)
            .ok_or_else(|| bad(cmd, format!("`part` is the part's number (1 to {MAX_PARTS}) or true"))),
    }
}

fn run_report(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = CMD_REPORT;
    let mut st = load(s.doc()?);
    let run = run_param(CMD, &st, p)?.clone();
    let area_id = str_param(p, "area")
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| bad(CMD, "`area` is required: the area id validate.next gave"))?;
    let i = run
        .areas
        .iter()
        .position(|x| x.id.eq_ignore_ascii_case(area_id))
        .ok_or_else(|| bad(CMD, format!("area {} is not in run {} (validate.status lists its areas)", cut(area_id, 60), run.id)))?;
    let Some(area) = run.areas.get(i).cloned() else { return Err(bad(CMD, "the run's area list changed: call validate.next again")) };
    if area.status == "pending" || area.dir.is_empty() {
        return Err(bad(CMD, format!("area {} has no packet yet: call validate.next {{run: \"{}\"}} first", area.id, run.id)));
    }
    // In parts (`part`, or `final: false`): each part adds to the area's findings and the area
    // is done on the `final` one. With neither, the report is the area's whole report.
    let part = part_param(CMD, p)?;
    let final_given = match p.get("final") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(bad(CMD, "`final` is true (the last part: the area is done) or false (more parts follow)")),
    };
    let in_parts = part.is_some() || final_given == Some(false);
    let last = final_given.unwrap_or(!in_parts);
    if in_parts && area.status == "done" {
        return Err(bad(
            CMD,
            format!(
                "area {}'s report is final already: a report without `part` replaces it, or validate.next {{area: \"{}\"}} hands it out again",
                area.id, area.id
            ),
        ));
    }
    let list = match p.get("findings") {
        Some(Value::Array(a)) => a,
        _ => {
            return Err(bad(
                CMD,
                "`findings` is a list: [{rule, handle? | handles?, verdict, issue, expected, found, evidence: [{image, sheet, what}], confidence, severity?}]",
            ));
        }
    };
    let earlier: &[Finding] = if in_parts { &area.findings } else { &[] };
    if list.is_empty() && !area.rules.is_empty() {
        if !last {
            return Err(bad(CMD, "a part with no findings: send its findings, or {final: true} to close the area"));
        }
        if earlier.is_empty() {
            return Err(bad(
                CMD,
                format!(
                    "`findings` is empty: check every rule of the packet ({}) for every element it applies to; a pass is a finding too",
                    area.rules.join(", ")
                ),
            ));
        }
    }
    if list.len() > MAX_PART {
        return Err(bad(
            CMD,
            format!(
                "{} findings in one report: at most {MAX_PART} at a time; send the area in parts ({{part: 1}}, {{part: 2}}, … the last with {{final: true}}), or name several handles in one finding when the same evidence and verdict cover them",
                list.len()
            ),
        ));
    }
    let d = s.doc()?;
    let (all, _) = vr::effective(&d.project_rules).map_err(|e| bad(CMD, e))?;
    let dir = std::path::Path::new(&run.out_dir).join(&area.dir);
    // What each object of the packet is (a general rule's finding takes its element's trade),
    // and the elements beside the area that are not its own.
    let objects = read_json(&dir.join("objects.json"));
    let trades: BTreeMap<String, String> = objects
        .as_ref()
        .map(|o| {
            ["elements", "runs"]
                .iter()
                .flat_map(|k| o[*k].as_array().cloned().unwrap_or_default())
                .filter_map(|e| Some((e["handle"].as_str()?.to_ascii_uppercase(), e["trade"].as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let packet: Option<PacketObjects> = objects.as_ref().map(PacketObjects::of);
    let mut problems: Vec<String> = Vec::new();
    // Answers kept as unsure because they cite no print page (G-COVERAGE).
    let mut uncited: Vec<String> = Vec::new();
    let mut read: Vec<Read> = Vec::new();
    for (k, f) in list.iter().enumerate() {
        let n = k + 1;
        let before = problems.len();
        let Some(o) = f.as_object() else {
            problems.push(format!("finding {n} is not an object"));
            continue;
        };
        let rule = o.get("rule").and_then(Value::as_str).map(|r| r.trim().to_ascii_uppercase()).unwrap_or_default();
        if rule.is_empty() {
            problems.push(format!("finding {n} has no `rule`"));
        } else if !area.rules.contains(&rule) {
            problems.push(format!(
                "finding {n}: rule `{}` is not one of this packet's rules ({})",
                cut(&rule, 30),
                if area.rules.is_empty() { "none".into() } else { area.rules.join(", ") }
            ));
        }
        let mut verdict = o.get("verdict").and_then(Value::as_str).map(|x| x.trim().to_ascii_lowercase()).unwrap_or_default();
        if !VERDICTS.contains(&verdict.as_str()) {
            problems.push(format!("finding {n}: `verdict` is {} (got {})", VERDICTS.join(", "), cut(&text_of(o.get("verdict")), 20)));
        }
        // `handle`, and `handles` when the same evidence and verdict cover several elements.
        let mut given: Vec<&Value> = Vec::new();
        match o.get("handle") {
            None | Some(Value::Null) => {}
            Some(Value::String(h)) if h.trim().is_empty() => {}
            Some(h) => given.push(h),
        }
        match o.get("handles") {
            None | Some(Value::Null) => {}
            Some(Value::Array(a)) if a.len() <= MAX_HANDLES => given.extend(a.iter()),
            Some(Value::Array(a)) => problems.push(format!("finding {n}: {} handles: at most {MAX_HANDLES} in one finding", a.len())),
            Some(_) => problems.push(format!("finding {n}: `handles` is a list of handle strings")),
        }
        let mut handles: Vec<String> = Vec::new();
        for h in given {
            let Some(h) = h.as_str() else {
                problems.push(format!("finding {n}: `handle` is a handle string"));
                continue;
            };
            let Some(x) = Handle::parse_hex(h.trim()).filter(|x| d.model.get(*x).is_some()) else {
                problems.push(format!("finding {n}: handle `{}` is not an object of the drawing (objects.json lists the area's)", cut(h, 20)));
                continue;
            };
            let h = x.hex();
            if handles.contains(&h) {
                continue;
            }
            if let Some(po) = &packet
                && problems.len() == before
            {
                match po.what(&h) {
                    Some(Held::Own(obj)) => {
                        if let Some(r) = all.iter().find(|e| e.rule.id == rule)
                            && r.rule.names_kinds()
                            && !r.rule.applies_to_element(&obj.kind, obj.category.as_deref(), obj.mark.as_deref())
                        {
                            problems.push(format!(
                                "finding {n}: rule {rule} does not apply to {h}, a {}{} (it applies to {})",
                                obj.kind.replace('_', " "),
                                obj.category.as_deref().map(|c| format!(", {c}")).unwrap_or_default(),
                                applies_words(&r.rule)
                            ));
                        }
                    }
                    Some(Held::Nearby(room)) => problems.push(format!(
                        "finding {n}: {h} is beside this area, not in it ({}; objects.json `nearby`): judge it in its own area",
                        room.map(|r| format!("in room {r}")).unwrap_or_else(|| "in no room".into())
                    )),
                    None => problems.push(format!("finding {n}: {h} is not one of this area's objects (objects.json lists them)")),
                }
            }
            handles.push(h);
        }
        if verdict == "missing" && !handles.is_empty() {
            problems.push(format!(
                "finding {n}: a `missing` finding has no handle (the model has nothing there; say what the print shows in `expected`)"
            ));
        }
        if verdict == "extra" && handles.is_empty() && problems.len() == before {
            problems.push(format!("finding {n}: an `extra` finding names the model element (`handle`)"));
        }
        let issue = cut(&text_of(o.get("issue")), TEXT_MAX);
        if issue.is_empty() && verdict != "pass" {
            problems.push(format!("finding {n}: `issue` says what is wrong in one sentence"));
        }
        let confidence = o.get("confidence").and_then(Value::as_f64).filter(|c| c.is_finite() && (0.0..=1.0).contains(c));
        if confidence.is_none() {
            problems.push(format!("finding {n}: `confidence` is a number from 0 to 1"));
        }
        let severity = match o.get("severity").and_then(Value::as_str).map(|x| x.trim().to_ascii_lowercase()) {
            None => all.iter().find(|e| e.rule.id == rule).map(|e| e.rule.severity.clone()).unwrap_or_else(|| "medium".into()),
            Some(x) if vr::SEVERITIES.contains(&x.as_str()) => x,
            Some(x) => {
                problems.push(format!("finding {n}: `severity` is high, medium or low (got {})", cut(&x, 20)));
                String::new()
            }
        };
        let mut evidence: Vec<Evidence> = Vec::new();
        // G-COVERAGE: an answer that cites no print page is kept, as `unsure` (Q6183).
        let cites_or_unsure = rule == super::validate_coverage::RULE;
        match o.get("evidence") {
            None | Some(Value::Null) if cites_or_unsure => {}
            Some(Value::Array(ev)) if ev.is_empty() && cites_or_unsure => {}
            Some(Value::Array(ev)) if !ev.is_empty() => {
                for (j, e) in ev.iter().take(MAX_EVIDENCE).enumerate() {
                    let given = e.get("image").and_then(Value::as_str).unwrap_or_default();
                    let what = cut(&text_of(e.get("what")), WHAT_MAX);
                    match packet_image(&area.images, &dir, given) {
                        Some(image) if !what.is_empty() => evidence.push(Evidence { image, sheet: cut(&text_of(e.get("sheet")), 40), what }),
                        Some(_) => problems.push(format!("finding {n}, evidence {}: `what` says what that picture shows", j + 1)),
                        None => problems.push(format!(
                            "finding {n}, evidence {}: `{}` is not a picture of this packet (manifest.json lists them: prints/…, model/…)",
                            j + 1,
                            cut(given, 60)
                        )),
                    }
                }
                if is_problem(&verdict) && !cites_or_unsure && !evidence.is_empty() && !evidence.iter().any(|e| e.image.starts_with("prints/")) {
                    problems.push(format!("finding {n}: a {verdict} finding cites at least one print picture (prints/…): what the documents show"));
                }
            }
            _ => problems.push(format!("finding {n}: `evidence` lists the pictures it rests on: [{{image, sheet, what}}]")),
        }
        if problems.len() > before {
            continue;
        }
        if cites_or_unsure && verdict != "unsure" && !evidence.iter().any(|e| e.image.starts_with("prints/") && !e.sheet.trim().is_empty()) {
            uncited.push(format!("finding {n} ({rule}, {verdict}) cites no print page (a prints/ picture with its sheet): kept as unsure"));
            verdict = "unsure".into();
        }
        let trade = all
            .iter()
            .find(|e| e.rule.id == rule)
            .map(|e| e.rule.trade.clone())
            .filter(|t| t != "general")
            .or_else(|| handles.first().and_then(|h| trades.get(&h.to_ascii_uppercase()).cloned()))
            .unwrap_or_else(|| "general".into());
        let named = !handles.is_empty();
        let mut hs = handles.into_iter();
        read.push(Read {
            n,
            named,
            finding: Finding {
                id: String::new(),
                rule,
                trade,
                handle: hs.next(),
                also: hs.collect(),
                verdict,
                issue,
                expected: cut(&text_of(o.get("expected")), TEXT_MAX),
                found: cut(&text_of(o.get("found")), TEXT_MAX),
                evidence,
                confidence: r3(confidence.unwrap_or(0.0)),
                severity,
            },
        });
    }
    if !problems.is_empty() {
        let more = problems.len().saturating_sub(PROBLEMS_SHOWN);
        let mut msg = format!("report refused, nothing kept: {}", problems.iter().take(PROBLEMS_SHOWN).cloned().collect::<Vec<_>>().join("; "));
        if more > 0 {
            msg.push_str(&format!("; and {more} more"));
        }
        return Err(bad(CMD, msg));
    }
    let merge = merge_findings(earlier, read);
    let (kept, added) = (merge.kept, merge.added);
    // No cap below the area's elements × rules: room for one finding per element and rule
    // (counted per handle), and for what is missing.
    let own = packet.as_ref().map_or(0, |po| po.own.len());
    let cap = own.saturating_add(MISSING_SLACK).saturating_mul(area.rules.len().max(1)).max(MAX_PART);
    let total: usize = kept.iter().map(Finding::weight).sum();
    if total > cap {
        return Err(bad(
            CMD,
            format!(
                "report refused, nothing kept: the area would hold {total} findings (counted per handle), over its {cap} ({own} objects and room for {MISSING_SLACK} missing, by {} rules)",
                area.rules.len()
            ),
        ));
    }
    let expected = packet.as_ref().map(|po| po.expected(&area.rules, &all)).unwrap_or_default();
    let pairs = area_coverage(&kept, &expected, packet.as_ref(), &all);
    let mut by_verdict: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &added {
        if let Some(v) = VERDICTS.iter().find(|v| **v == f.verdict) {
            *by_verdict.entry(v).or_default() += f.weight();
        }
    }
    let items = added.iter().filter(|f| f.verdict != "pass").count();
    let stored: usize = added.iter().map(Finding::weight).sum();
    let parts = if in_parts { area.parts.saturating_add(1) } else { 1 };
    let mut left = 0;
    if let Some(r) = run_index(&st, &run.id).and_then(|k| st.runs.get_mut(k)) {
        if let Some(a) = r.areas.get_mut(i) {
            a.findings = kept;
            a.status = if last { "done".into() } else { "issued".into() };
            a.unchanged = false;
            a.parts = parts;
            a.pairs = pairs.clone();
        }
        left = remaining(r);
    }
    save(s, &st)?;
    let (asked, judged) = pairs.values().fold((0usize, 0usize), |(a, j), x| (a + x[0], j + x[1]));
    s.event(
        "validate.reported",
        json!({"run": run.id, "area": area.id, "findings": stored, "review_items": items, "remaining": left, "part": parts, "final": last}),
    );
    let mut out = json!({
        "run": run.id,
        "area": area.id,
        "stored": stored,
        "by_verdict": by_verdict,
        "review_items": items,
        "area_findings": total,
        "part": parts,
        "final": last,
        "pairs": {"asked": asked, "judged": judged, "coverage": ratio(judged, asked)},
        "remaining": left,
        "next": if !last { CMD_REPORT } else if left > 0 { CMD_NEXT } else { CMD_STATUS },
    });
    if !last {
        out["note"] = json!(format!(
            "part {parts} kept; the area is done on the last part: validate.report {{run: \"{}\", area: \"{}\", part: {}, final: true, findings}}",
            run.id,
            area.id,
            parts + 1
        ));
    }
    if merge.merged > 0 || merge.conflicts > 0 {
        let mut warnings = merge.warnings;
        out["merged"] = json!(merge.merged);
        out["conflicts"] = json!(merge.conflicts);
        let more = warnings.len().saturating_sub(WARNINGS_SHOWN);
        warnings.truncate(WARNINGS_SHOWN);
        if more > 0 {
            warnings.push(format!("and {more} more"));
        }
        out["warnings"] = json!(warnings);
    }
    if !uncited.is_empty() {
        let mut w: Vec<Value> = out["warnings"].as_array().cloned().unwrap_or_default();
        let more = uncited.len().saturating_sub(WARNINGS_SHOWN);
        w.extend(uncited.into_iter().take(WARNINGS_SHOWN).map(Value::from));
        if more > 0 {
            w.push(json!(format!("and {more} more kept as unsure")));
        }
        out["warnings"] = json!(w);
    }
    Ok(out)
}

/// `n / of`, to three places; none when `of` is 0.
fn ratio(n: usize, of: usize) -> Value {
    if of == 0 { Value::Null } else { json!(r3(n as f64 / of as f64)) }
}

/// What merging a report into an area's findings gives.
struct Merged {
    /// The area's findings after it.
    kept: Vec<Finding>,
    /// The report's own, as kept.
    added: Vec<Finding>,
    /// Handles (or findings of what is missing) merged into an earlier finding.
    merged: usize,
    /// Element × rule pairs given two verdicts.
    conflicts: usize,
    warnings: Vec<String>,
}

/// Merge a report's findings into the area's earlier ones (an earlier part's). A duplicate (the
/// same rule, element and verdict; for what is missing, the same rule, verdict and issue) is
/// merged into the first and said, never dropped unsaid; ids are stable and distinct.
fn merge_findings(earlier: &[Finding], read: Vec<Read>) -> Merged {
    let key_of = |f: &Finding, h: Option<&str>| match h {
        Some(h) => (f.rule.clone(), h.to_ascii_uppercase(), f.verdict.clone()),
        None => (f.rule.clone(), String::new(), format!("{}|{}", f.verdict, f.issue.to_lowercase())),
    };
    let mut kept: Vec<Finding> = earlier.to_vec();
    // Where each key was first reported: 0 for an earlier part, else the finding's number.
    let mut seen: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    let mut verdicts: BTreeMap<(String, String), String> = BTreeMap::new();
    for f in &kept {
        if f.handle.is_none() && f.also.is_empty() {
            seen.insert(key_of(f, None), 0);
        }
        for h in f.handles() {
            seen.insert(key_of(f, Some(h)), 0);
            verdicts.entry((f.rule.clone(), h.to_ascii_uppercase())).or_insert_with(|| f.verdict.clone());
        }
    }
    let mut ids: BTreeSet<String> = kept.iter().map(|f| f.id.clone()).collect();
    let mut out = Merged { kept: Vec::new(), added: Vec::new(), merged: 0, conflicts: 0, warnings: Vec::new() };
    let said = |at: usize| if at == 0 { "in an earlier part".to_string() } else { format!("finding {at}") };
    for Read { n, mut finding, named } in read {
        if named {
            let mut left: Vec<String> = Vec::new();
            for h in finding.handles().map(str::to_string).collect::<Vec<_>>() {
                let key = key_of(&finding, Some(&h));
                if let Some(at) = seen.get(&key) {
                    out.merged += 1;
                    out.warnings.push(format!(
                        "finding {n}: {h} under {} ({}) is {} already: merged into it",
                        finding.rule,
                        finding.verdict,
                        said(*at)
                    ));
                    continue;
                }
                seen.insert(key, n);
                let pair = (finding.rule.clone(), h.to_ascii_uppercase());
                match verdicts.get(&pair) {
                    Some(v) if *v != finding.verdict => {
                        out.conflicts += 1;
                        out.warnings.push(format!(
                            "finding {n}: {h} under {} is {} here and {v} in another finding: both kept, the pair counts as the worse",
                            finding.rule, finding.verdict
                        ));
                    }
                    Some(_) => {}
                    None => {
                        verdicts.insert(pair, finding.verdict.clone());
                    }
                }
                left.push(h);
            }
            if left.is_empty() {
                continue;
            }
            let mut hs = left.into_iter();
            finding.handle = hs.next();
            finding.also = hs.collect();
        } else {
            let key = key_of(&finding, None);
            if let Some(at) = seen.get(&key) {
                out.merged += 1;
                out.warnings.push(format!(
                    "finding {n}: the same {} {} with the same issue is {} already: merged into it",
                    finding.rule,
                    finding.verdict,
                    said(*at)
                ));
                continue;
            }
            seen.insert(key, n);
        }
        // Its id: the same finding keeps its id (a person's answer to its review item stays
        // with it); two findings never share one.
        let base = if finding.also.is_empty() {
            format!("F{}", hash8(&format!("{}|{:?}|{}|{}", finding.rule, finding.handle, finding.verdict, finding.issue)))
        } else {
            let hs: Vec<&str> = finding.handles().collect();
            format!("F{}", hash8(&format!("{}|{hs:?}|{}|{}", finding.rule, finding.verdict, finding.issue)))
        };
        let mut id = base.clone();
        let mut k = 2usize;
        while ids.contains(&id) {
            id = format!("{base}-{k}");
            k += 1;
        }
        ids.insert(id.clone());
        finding.id = id;
        out.added.push(finding);
    }
    kept.extend(out.added.iter().cloned());
    out.kept = kept;
    out
}

// ---------------------------------------------------------------- review items

/// A judged finding as a review item (kind `validation`): the review queue lists them.
pub(crate) struct JudgeItem {
    pub id: String,
    pub level: String,
    pub title: String,
    pub detail: Value,
    pub documents: Vec<String>,
    pub rank: f64,
    pub key: Value,
}

fn judge_item_id(run: &str, area: &str, finding: &str) -> String {
    format!("RV-J{}", hash8(&format!("judge|{run}|{area}|{finding}")))
}

/// Every judged finding that isn't a pass, ranked by severity × confidence.
pub(crate) fn judge_items(d: &Drawing) -> Vec<JudgeItem> {
    let st = load(d);
    let mut out = Vec::new();
    for run in &st.runs {
        for a in run.areas.iter().filter(|a| a.status == "done") {
            let dir = std::path::Path::new(&run.out_dir).join(&a.dir);
            let full = |f: &str| dir.join(f).to_string_lossy().into_owned();
            for f in a.findings.iter().filter(|f| f.verdict != "pass") {
                let evidence: Vec<Value> = f
                    .evidence
                    .iter()
                    .map(|e| {
                        // Side by side: the print crop and the model drawn over the same crop.
                        let model = a.prints.iter().find(|(file, _, _)| *file == e.image).and_then(|(_, _, m)| m.as_deref()).map(full);
                        json!({"image": full(&e.image), "sheet": e.sheet, "what": e.what, "model": model})
                    })
                    .collect();
                let mut documents: Vec<String> = f.evidence.iter().map(|e| e.sheet.clone()).filter(|x| !x.is_empty()).collect();
                documents.sort();
                documents.dedup();
                out.push(JudgeItem {
                    id: judge_item_id(&run.id, &a.id, &f.id),
                    level: a.level.clone(),
                    title: cut(
                        &match f.also.len() {
                            0 => format!("{} {} ({}): {}", f.rule, f.verdict, a.label, f.issue),
                            n => format!("{} {} ({}, {} elements): {}", f.rule, f.verdict, a.label, n + 1, f.issue),
                        },
                        120,
                    ),
                    detail: json!({
                        "run": run.id, "area": a.id, "rule": f.rule, "trade": f.trade, "verdict": f.verdict, "handle": f.handle,
                        "handles": f.handles().collect::<Vec<_>>(),
                        "issue": f.issue, "expected": f.expected, "found": f.found, "confidence": f.confidence, "severity": f.severity,
                        "evidence": evidence, "packet": dir.to_string_lossy(),
                        "zoom": {"level": a.level, "handle": f.handle, "box_ft": a.bbox},
                        "judge": "agent",
                    }),
                    documents,
                    rank: 3.0 - severity_weight(&f.severity) * f.confidence,
                    key: json!({"judge": run.id, "area": a.id, "finding": f.id}),
                });
            }
        }
    }
    out
}

/// A person's answer to a judged finding: `accept` (a real error) or `reject` (a false alarm).
pub(crate) fn judge_answer(answer: &Value) -> std::result::Result<&'static str, String> {
    let t = match answer {
        Value::String(t) => t.clone(),
        Value::Object(o) => o.get("verdict").or_else(|| o.get("answer")).and_then(Value::as_str).unwrap_or_default().to_string(),
        _ => String::new(),
    };
    match t.trim().to_ascii_lowercase().as_str() {
        "accept" | "accepted" | "real" | "real error" => Ok("accept"),
        "reject" | "rejected" | "false alarm" => Ok("reject"),
        _ => Err("a judged finding's answer is \"accept\" (it is a real error) or \"reject\" (a false alarm)".into()),
    }
}

// ---------------------------------------------------------------- validate.status

#[derive(Default)]
struct Tally {
    pass: usize,
    fail: usize,
    missing: usize,
    extra: usize,
    unsure: usize,
}

impl Tally {
    fn add(&mut self, verdict: &str, n: usize) {
        let into = match verdict {
            "pass" => &mut self.pass,
            "fail" => &mut self.fail,
            "missing" => &mut self.missing,
            "extra" => &mut self.extra,
            _ => &mut self.unsure,
        };
        *into = into.saturating_add(n);
    }
    fn total(&self) -> usize {
        self.judged() + self.unsure
    }
    /// Judged: everything but `unsure`.
    fn judged(&self) -> usize {
        self.pass + self.fail + self.missing + self.extra
    }
    fn score(&self) -> Value {
        ratio(self.pass, self.judged())
    }
    /// The counts, and the accuracy of the same pairs.
    fn to_json(&self, accuracy: Value) -> Value {
        json!({"pass": self.pass, "fail": self.fail, "missing": self.missing, "extra": self.extra, "unsure": self.unsure, "accuracy": accuracy})
    }
}

/// The scores of a set of judged areas: the findings counted per handle, and the accuracy over
/// the element × rule pairs they judged (a pair judged twice counts once, by its worse verdict;
/// an `unsure` pair is not judged), with the coverage of the pairs the packets asked for.
#[derive(Default)]
struct Scores {
    findings: Tally,
    findings_by_trade: BTreeMap<String, Tally>,
    findings_by_rule: BTreeMap<String, Tally>,
    pairs: Tally,
    pairs_by_trade: BTreeMap<String, Tally>,
    pairs_by_rule: BTreeMap<String, Tally>,
    /// Per rule: the pairs the packets asked for, and how many were judged.
    asked: BTreeMap<String, [usize; 2]>,
    areas: usize,
}

impl Scores {
    fn add(&mut self, a: &RunArea) {
        self.areas += 1;
        for f in &a.findings {
            let w = f.weight();
            self.findings.add(&f.verdict, w);
            self.findings_by_trade.entry(f.trade.clone()).or_default().add(&f.verdict, w);
            self.findings_by_rule.entry(f.rule.clone()).or_default().add(&f.verdict, w);
        }
        for ((rule, _), (trade, verdict)) in area_pairs(&a.findings) {
            self.pairs.add(&verdict, 1);
            self.pairs_by_trade.entry(trade).or_default().add(&verdict, 1);
            self.pairs_by_rule.entry(rule).or_default().add(&verdict, 1);
        }
        for (r, x) in &a.pairs {
            let e = self.asked.entry(r.clone()).or_default();
            e[0] = e[0].saturating_add(x[0]);
            e[1] = e[1].saturating_add(x[1]);
        }
    }

    fn of<'a>(areas: impl IntoIterator<Item = &'a RunArea>) -> Scores {
        let mut s = Scores::default();
        for a in areas {
            s.add(a);
        }
        s
    }

    fn asked_judged(&self) -> (usize, usize) {
        self.asked.values().fold((0, 0), |(a, j), x| (a + x[0], j + x[1]))
    }

    fn findings_json(&self) -> Value {
        let by = |f: &BTreeMap<String, Tally>, p: &BTreeMap<String, Tally>| -> serde_json::Map<String, Value> {
            f.iter().map(|(k, t)| (k.clone(), t.to_json(p.get(k).map(Tally::score).unwrap_or(Value::Null)))).collect()
        };
        json!({
            "total": self.findings.total(),
            "by_verdict": self.findings.to_json(self.pairs.score()),
            "by_trade": by(&self.findings_by_trade, &self.pairs_by_trade),
            "by_rule": by(&self.findings_by_rule, &self.pairs_by_rule),
        })
    }

    fn accuracy_json(&self) -> Value {
        let (asked, judged) = self.asked_judged();
        json!({
            "overall": self.pairs.score(),
            "judged_pairs": self.pairs.judged(),
            "unsure_pairs": self.pairs.unsure,
            "pairs_asked": asked,
            "coverage": ratio(judged, asked),
            "by_trade": self.pairs_by_trade.iter().map(|(k, t)| (k.clone(), t.score())).collect::<serde_json::Map<_, _>>(),
            "by_rule": self.pairs_by_rule.iter().map(|(k, t)| (k.clone(), t.score())).collect::<serde_json::Map<_, _>>(),
            "coverage_by_rule": self.asked.iter().map(|(k, x)| (k.clone(), ratio(x[1], x[0]))).collect::<serde_json::Map<_, _>>(),
        })
    }
}

/// Every area of the drawing's runs judged (done, or with parts in) once: an area judged in
/// more than one run counts from the latest of them.
fn latest_areas(st: &Store) -> Vec<&RunArea> {
    let mut runs: Vec<&Run> = st.runs.iter().collect();
    runs.sort_by_key(|r| r.started);
    let mut by_id: BTreeMap<&str, &RunArea> = BTreeMap::new();
    for r in runs {
        for a in r.areas.iter().filter(|a| a.status == "done" || !a.findings.is_empty()) {
            by_id.insert(a.id.as_str(), a);
        }
    }
    by_id.into_values().collect()
}

fn planted_report(plant: &Plant, run: &Run, all: &[Effective], reveal: bool) -> Value {
    let covers = |a: &RunArea, level: &str, p: [f64; 2]| a.level.eq_ignore_ascii_case(level) && inside(grow(a.bbox, 1.0), p);
    let mut caught = 0usize;
    let mut missed = 0usize;
    let mut waiting = 0usize;
    let mut outside = 0usize;
    let mut by_kind: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut errors: Vec<Value> = Vec::new();
    for e in &plant.errors {
        let here: Vec<&RunArea> =
            run.areas.iter().filter(|a| covers(a, &e.level, e.at_ft) || e.to_ft.is_some_and(|t| covers(a, &e.level, t))).collect();
        let by: Vec<Value> = run
            .areas
            .iter()
            .flat_map(|a| a.findings.iter().map(move |f| (a, f)))
            .filter(|(a, f)| {
                is_problem(&f.verdict)
                    && if e.kind == "delete" {
                        f.verdict == "missing"
                            && covers(a, &e.level, e.at_ft)
                            && all
                                .iter()
                                .find(|r| r.rule.id == f.rule)
                                .is_some_and(|r| r.rule.trade == "general" || r.rule.applies_to_kind(&e.element))
                    } else {
                        f.handles().any(|h| h.eq_ignore_ascii_case(&e.handle))
                    }
            })
            .map(|(a, f)| json!({"area": a.id, "rule": f.rule, "verdict": f.verdict}))
            .collect();
        let status = if here.is_empty() {
            outside += 1;
            "outside the run"
        } else if !by.is_empty() {
            caught += 1;
            by_kind.entry(e.kind.clone()).or_default().0 += 1;
            "caught"
        } else if here.iter().all(|a| a.status == "done") {
            missed += 1;
            by_kind.entry(e.kind.clone()).or_default().1 += 1;
            "missed"
        } else {
            waiting += 1;
            "not judged yet"
        };
        if reveal {
            errors.push(json!({"n": e.n, "kind": e.kind, "element": e.element, "mark": e.mark, "handle": e.handle, "level": e.level, "at_ft": e.at_ft, "to_ft": e.to_ft, "from": e.from, "to": e.to, "status": status, "by": by}));
        }
    }
    let judged = caught + missed;
    let mut out = json!({
        "total": plant.errors.len(),
        "caught": caught,
        "missed": missed,
        "not_judged_yet": waiting,
        "outside_run": outside,
        "recall": if judged == 0 { Value::Null } else { json!(r3(caught as f64 / judged as f64)) },
        "by_kind": by_kind.iter().map(|(k, (c, m))| (k.clone(), json!({"caught": c, "missed": m}))).collect::<serde_json::Map<_, _>>(),
        "seed": plant.seed,
    });
    if reveal {
        out["errors"] = json!(errors);
    } else {
        out["errors"] = json!("shown to a person, or once every area is judged");
    }
    out
}

fn run_status(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = CMD_STATUS;
    let d = s.doc()?;
    let st = load(d);
    if st.runs.is_empty() {
        return Ok(json!({"runs": [], "note": "no validate run yet: validate {scope} starts one", "next": CMD_START}));
    }
    let run = run_param(CMD, &st, p)?;
    let (all, _) = vr::effective(&d.project_rules).map_err(|e| bad(CMD, e))?;
    let answers = super::agentready::decisions(d);
    let scores = Scores::of(&run.areas);
    let (mut accepted, mut rejected, mut open, mut deferred) = (0usize, 0usize, 0usize, 0usize);
    for a in &run.areas {
        for f in &a.findings {
            if f.verdict == "pass" {
                continue;
            }
            match answers.get(&judge_item_id(&run.id, &a.id, &f.id)) {
                Some((status, _)) if status == "deferred" => deferred += 1,
                Some((_, ans)) => match judge_answer(ans) {
                    Ok("accept") => accepted += 1,
                    Ok(_) => rejected += 1,
                    Err(_) => open += 1,
                },
                None => open += 1,
            }
        }
    }
    let answered = accepted + rejected;
    let left = remaining(run);
    let reveal = !s.agent.from_agent || left == 0;
    let mut out = json!({
        "run": run.id,
        "scope": run.scope,
        "trades": run.trades,
        "rules": run.rules,
        "by": run.by,
        "out_dir": run.out_dir,
        "areas": area_counts(run),
        "findings": scores.findings_json(),
        "accuracy": scores.accuracy_json(),
        "review": {
            "accepted": accepted,
            "rejected": rejected,
            "open": open,
            "deferred": deferred,
            "false_alarm_rate": if answered == 0 { Value::Null } else { json!(r3(rejected as f64 / answered as f64)) },
        },
        "prescreen": run.prescreen,
        "runs": st.runs.iter().map(|r| json!({"run": r.id, "scope": r.scope["scope"], "areas": r.areas.len(), "done": r.areas.iter().filter(|a| a.status == "done").count()})).collect::<Vec<_>>(),
        "next": if left > 0 { CMD_NEXT } else { "review.list {kind: \"validation\"}" },
    });
    // The products' accuracy per kind and per field, as the run's prescreen measured them.
    if let Some(pk) = run.prescreen.get("measured").and_then(|m| m.get("products")).filter(|p| p.is_object()) {
        let scores = |key: &str| -> serde_json::Map<String, Value> {
            pk.get(key)
                .and_then(Value::as_object)
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.get("accuracy").cloned().unwrap_or(Value::Null))).collect())
                .unwrap_or_default()
        };
        out["accuracy"]["products"] = json!({"by_kind": scores("by_kind"), "by_field": scores("by_field")});
        out["products"] = json!({"by_kind": pk.get("by_kind"), "by_field": pk.get("by_field")});
    }
    // Every run of the drawing: each one's scores, and the totals over all of them.
    let mut runs: Vec<&Run> = st.runs.iter().collect();
    runs.sort_by_key(|r| std::cmp::Reverse(r.started));
    out["per_run"] = json!(
        runs.iter()
            .map(|r| {
                let sc = Scores::of(&r.areas);
                let (asked, judged) = sc.asked_judged();
                json!({
                    "run": r.id, "scope": r.scope, "by": r.by, "areas": area_counts(r), "findings": sc.findings.total(),
                    "accuracy": sc.pairs.score(), "judged_pairs": sc.pairs.judged(), "pairs_asked": asked, "coverage": ratio(judged, asked),
                })
            })
            .collect::<Vec<_>>()
    );
    let every = Scores::of(latest_areas(&st));
    out["totals"] = json!({
        "runs": st.runs.len(),
        "areas_judged": every.areas,
        "findings": every.findings_json(),
        "accuracy": every.accuracy_json(),
        "note": "every run of the drawing; an area judged in more than one run counts once, from the latest",
    });
    if let Some(plant) = plant_of(d) {
        out["planted"] = planted_report(&plant, run, &all, reveal);
    }
    if run.cancelled {
        out["cancelled"] = json!(true);
        if left > 0 {
            out["next"] = json!("validate (the same scope resumes it)");
        }
    } else if run.by == "user" && left > 0 && !s.events.on {
        out["waiting"] = json!(WAITING);
    }
    if run.left_out > 0 {
        out["left_out"] = json!(run.left_out);
    }
    if out.to_string().len() > super::agentready::ANSWER_MAX {
        out["findings"]["by_rule"] = json!("over the answer's size: see accuracy.by_rule");
        if out.get("products").is_some() {
            out["products"] = json!("over the answer's size: see accuracy.products");
        }
        out["totals"]["findings"]["by_rule"] = json!("over the answer's size: see totals.accuracy.by_rule");
        if let Some(pl) = out.get_mut("planted").and_then(Value::as_object_mut) {
            pl.insert("errors".into(), json!("too many to list here"));
        }
    }
    if out.to_string().len() > super::agentready::ANSWER_MAX
        && let Some(list) = out.get_mut("per_run").and_then(Value::as_array_mut)
    {
        let n = list.len();
        list.truncate(3);
        list.push(json!(format!("and {} older runs: validate.status {{run}} gives each", n.saturating_sub(3))));
    }
    Ok(out)
}

// ---------------------------------------------------------------- validate.plant

/// SplitMix64: the same seed plants the same errors.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

/// Put `key=value` in an entity's provenance group (the group made when it has none).
fn set_prov(x: &mut Vec<RawTag>, key: &str, value: &str) {
    let tag = RawTag { code: 1000, value: format!("{key}={value}") };
    match x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(PROV_APP)) {
        Some(start) => {
            let end = x.iter().skip(start + 1).position(|t| t.code == 1001).map_or(x.len(), |k| start + 1 + k);
            let prefix = format!("{key}=");
            let mut k = start + 1;
            let mut end = end;
            while k < end {
                if x.get(k).is_some_and(|t| t.code == 1000 && t.value.to_ascii_lowercase().starts_with(&prefix)) {
                    x.remove(k);
                    end -= 1;
                } else {
                    k += 1;
                }
            }
            x.insert(start + 1, tag);
        }
        None => {
            x.push(RawTag { code: 1001, value: PROV_APP.into() });
            x.push(tag);
        }
    }
}

/// A larger size of the same shape: `10x8` → `14x10`, `8` → `10`.
fn bigger_size(s: &str) -> String {
    let nums: Vec<f64> =
        s.split(|c: char| !(c.is_ascii_digit() || c == '.')).filter_map(|x| x.parse::<f64>().ok()).filter(|x| x.is_finite()).collect();
    match nums.as_slice() {
        [w, h, ..] => format!("{}x{}", w + 4.0, h + 2.0),
        [d] => format!("{}", d + 2.0),
        [] => "12x10".into(),
    }
}

/// A receptacle type without its ground-fault words: `GFCI` → `DUPLEX`, `DUPLEX GFI WP` → `DUPLEX WP`.
fn without_gfci(s: &str) -> String {
    let kept: Vec<&str> = s
        .split(|c: char| c.is_whitespace() || c == '-' || c == '_' || c == '/')
        .filter(|w| !w.is_empty() && !["GFCI", "GFI", "GF"].contains(&w.to_ascii_uppercase().as_str()))
        .collect();
    if kept.is_empty() { "DUPLEX".into() } else { kept.join(" ") }
}

fn has_gfci(s: &str) -> bool {
    let u = s.to_ascii_uppercase();
    u.contains("GFCI") || u.contains("GFI")
}

fn run_plant(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = CMD_PLANT;
    let count = match p.get("count") {
        None | Some(Value::Null) => DEFAULT_PLANT,
        Some(c) => c
            .as_u64()
            .filter(|c| (1..=MAX_PLANT as u64).contains(c))
            .map(|c| c as usize)
            .ok_or_else(|| bad(CMD, format!("`count` is a number from 1 to {MAX_PLANT}")))?,
    };
    let seed = match p.get("seed") {
        None | Some(Value::Null) => 1,
        Some(x) => x.as_u64().ok_or_else(|| bad(CMD, "`seed` is a whole number (the same seed plants the same errors)"))?,
    };
    let kinds: Vec<String> = match p.get("kinds") {
        None | Some(Value::Null) => PLANT_KINDS.iter().map(|k| k.to_string()).collect(),
        Some(Value::Array(a)) if !a.is_empty() => {
            let mut k: Vec<String> = a
                .iter()
                .take(16)
                .map(|x| {
                    x.as_str()
                        .map(|t| t.trim().to_ascii_lowercase())
                        .filter(|t| PLANT_KINDS.contains(&t.as_str()))
                        .ok_or_else(|| bad(CMD, format!("unknown kind {x} ({})", PLANT_KINDS.join(", "))))
                })
                .collect::<Result<Vec<_>>>()?;
            k.sort();
            k.dedup();
            k
        }
        Some(_) => return Err(bad(CMD, format!("`kinds` is a list of {}", PLANT_KINDS.join(", ")))),
    };
    let doc: Arc<Drawing> = Arc::clone(&s.state()?.doc);
    let d: &Drawing = &doc;
    let title = s.state()?.title.clone();
    let (sp, _) = current(s, CMD)?;
    let (model, frame) = model_of(d, &sp);
    let upf = frame.upf();
    let type_attr = sp.to_rules().devices.type_attr;
    let rooms: Vec<Area> = vp::model_areas(d, &frame).into_iter().filter(|a| a.stretch.is_none_or(|x| x[0] == 1)).collect();
    let is_insert = |id: &str| Handle::parse_hex(id).and_then(|h| d.model.get(h)).is_some_and(|e| matches!(e.kind, EntityKind::Insert(_)));
    let devices: Vec<&v::Element> = {
        let mut e: Vec<&v::Element> = model.elements.iter().filter(|e| is_insert(&e.id)).collect();
        e.sort_by(|a, b| a.id.cmp(&b.id));
        e
    };
    let room_of = |level: &str, p: [f64; 2]| rooms.iter().find(|r| r.level.eq_ignore_ascii_case(level) && inside(r.bbox, p));
    let candidates = |kind: &str| -> Vec<String> {
        match kind {
            "move" | "delete" => devices.iter().map(|e| e.id.clone()).collect(),
            "duct_size" => {
                let mut r: Vec<String> = model.runs.iter().filter(|r| r.kind.contains("duct")).map(|r| r.id.clone()).collect();
                r.sort();
                r
            }
            "panel_mount" => devices.iter().filter(|e| e.kind.contains("panel")).map(|e| e.id.clone()).collect(),
            "fan_room" => devices
                .iter()
                .filter(|e| e.kind.contains("fan") && rooms.iter().any(|r| r.level.eq_ignore_ascii_case(&e.level) && !inside(r.bbox, e.at)))
                .map(|e| e.id.clone())
                .collect(),
            _ => devices.iter().filter(|e| has_gfci(&e.kind) || e.mark.as_deref().is_some_and(has_gfci)).map(|e| e.id.clone()).collect(),
        }
    };
    let mut rng = Rng(seed);
    let mut order = kinds.clone();
    for i in (1..order.len()).rev() {
        let j = rng.below(i + 1);
        order.swap(i, j);
    }
    let mut copy: Drawing = (*doc).clone();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut errors: Vec<Planted> = Vec::new();
    let mut exhausted: BTreeSet<String> = BTreeSet::new();
    let mut turn = 0usize;
    while errors.len() < count && exhausted.len() < order.len() {
        let Some(kind) = order.get(turn % order.len()).cloned() else { break };
        turn += 1;
        if exhausted.contains(&kind) {
            continue;
        }
        let free: Vec<String> = candidates(&kind).into_iter().filter(|h| !used.contains(h)).collect();
        if free.is_empty() {
            exhausted.insert(kind);
            continue;
        }
        let Some(hex) = free.get(rng.below(free.len())).cloned() else { continue };
        let Some(h) = Handle::parse_hex(&hex) else { continue };
        let el = model.elements.iter().find(|e| e.id == hex);
        let run = model.runs.iter().find(|r| r.id == hex);
        let (element, mark, level, at) = match (el, run) {
            (Some(e), _) => (e.kind.clone(), e.mark.clone(), e.level.clone(), e.at),
            (None, Some(r)) => (r.kind.clone(), r.mark.clone(), r.level.clone(), r.path.first().copied().unwrap_or([0.0, 0.0])),
            _ => continue,
        };
        let mut rec = Planted {
            n: errors.len() + 1,
            kind: kind.clone(),
            handle: hex.clone(),
            element,
            mark,
            level: level.clone(),
            at_ft: [r2(at[0]), r2(at[1])],
            ..Planted::default()
        };
        let shift = |copy: &mut Drawing, dx: f64, dy: f64| {
            copy.modify_entity(h, |e| {
                if let EntityKind::Insert(i) = &mut e.kind {
                    i.insert.x += dx * upf;
                    i.insert.y += dy * upf;
                }
            })
            .is_ok()
        };
        let done = match kind.as_str() {
            "move" => {
                let (dx, dy) = match rng.below(4) {
                    0 => (PLANT_MOVE_FT, 0.0),
                    1 => (-PLANT_MOVE_FT, 0.0),
                    2 => (0.0, PLANT_MOVE_FT),
                    _ => (0.0, -PLANT_MOVE_FT),
                };
                rec.to_ft = Some([r2(at[0] + dx), r2(at[1] + dy)]);
                rec.from = format!("at {:.2}, {:.2} ft", at[0], at[1]);
                rec.to = format!("moved {PLANT_MOVE_FT} ft to {:.2}, {:.2} ft", at[0] + dx, at[1] + dy);
                shift(&mut copy, dx, dy)
            }
            "delete" => {
                rec.from = "in the model".into();
                rec.to = "deleted".into();
                copy.remove_entity(h).is_some()
            }
            "duct_size" => {
                let now = run.and_then(|r| ["size", "ductSize"].iter().find_map(|k| r.fields.get(*k).cloned())).unwrap_or_default();
                let to = bigger_size(&now);
                rec.from = if now.is_empty() { "no size".into() } else { now };
                rec.to = to.clone();
                copy.modify_entity(h, |e| {
                    let mut x: Vec<RawTag> = e.common.xdata.to_vec();
                    set_prov(&mut x, "field.size", &to);
                    e.common.xdata = x.into();
                })
                .is_ok()
            }
            "panel_mount" => {
                let stored = d.model.get(h).map(|e| cadaddy_doc::props::read(&e.common.xdata)).unwrap_or_default();
                let now = stored
                    .get("mounting")
                    .map(|p| p.text())
                    .or_else(|| el.and_then(|e| e.fields.get("mounting").cloned()))
                    .unwrap_or_else(|| "recessed".into());
                let to = if now.to_ascii_lowercase().contains("surface") { "recessed" } else { "surface" };
                rec.from = now;
                rec.to = to.into();
                copy.modify_entity(h, |e| {
                    let mut x: Vec<RawTag> = e.common.xdata.to_vec();
                    let mut props = cadaddy_doc::props::read(&x);
                    if let Some(old) = props.get("mounting").cloned() {
                        props.set("mounting", cadaddy_doc::props::Prop { value: json!(to), ..old });
                        x = cadaddy_doc::props::write(&x, &props);
                    } else {
                        set_prov(&mut x, "field.mounting", to);
                    }
                    e.common.xdata = x.into();
                })
                .is_ok()
            }
            "fan_room" => {
                let mine = room_of(&level, at).map(|r| r.id.clone());
                let target = rooms
                    .iter()
                    .filter(|r| r.level.eq_ignore_ascii_case(&level) && Some(&r.id) != mine.as_ref() && !inside(r.bbox, at))
                    .map(|r| {
                        let c = [(r.bbox[0] + r.bbox[2]) / 2.0, (r.bbox[1] + r.bbox[3]) / 2.0];
                        ((c[0] - at[0]).hypot(c[1] - at[1]), c, r)
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0).then(a.2.id.cmp(&b.2.id)));
                match target {
                    Some((_, c, r)) => {
                        rec.to_ft = Some([r2(c[0]), r2(c[1])]);
                        rec.from = format!("in {}", mine.unwrap_or_else(|| "no room".into()));
                        rec.to = format!("in {}", r.id);
                        shift(&mut copy, c[0] - at[0], c[1] - at[1])
                    }
                    None => false,
                }
            }
            _ => {
                let mut to = String::new();
                let ok = copy
                    .modify_entity(h, |e| {
                        if let EntityKind::Insert(i) = &mut e.kind {
                            for a in i.attribs.iter_mut().filter(|a| a.tag.eq_ignore_ascii_case(&type_attr) && has_gfci(&a.text.value)) {
                                rec.from = a.text.value.clone();
                                a.text.value = without_gfci(&a.text.value);
                                to = a.text.value.clone();
                            }
                        }
                        if let Some(m) = cadaddy_doc::bimspec::entity_type_mark(&e.common.xdata).map(str::to_string).filter(|m| has_gfci(m)) {
                            let new = without_gfci(&m);
                            let mut x = cadaddy_doc::bimspec::strip_bim_xdata(&e.common.xdata);
                            x.extend(cadaddy_doc::bimspec::type_mark_xdata(&new));
                            e.common.xdata = x.into();
                            if rec.from.is_empty() {
                                rec.from = m;
                            }
                            to = new;
                        }
                    })
                    .is_ok();
                rec.to = to;
                ok && !rec.to.is_empty()
            }
        };
        used.insert(hex);
        if done {
            errors.push(rec);
        }
    }
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for e in &errors {
        *by_kind.entry(e.kind.clone()).or_default() += 1;
    }
    let not_planted: Vec<&String> = kinds.iter().filter(|k| !by_kind.contains_key(*k)).collect();
    if errors.is_empty() {
        return Err(bad(CMD, "nothing to plant: the model has no devices, ducts, panels, fans or GFCI receptacles yet (model.build places them)"));
    }
    // The record lives in the copy only; the copy starts with no runs of its own.
    let planted = errors.len();
    let mut r = root(&copy);
    r.remove(KEY);
    r.insert(PLANT_KEY.into(), serde_json::to_value(Plant { seed, asked: count, errors }).unwrap_or(Value::Null));
    copy.agent = serde_json::to_string(&Value::Object(r)).ok();
    let copy_title = format!("{title} (planted {seed})");
    s.open_drawing(copy, &copy_title, None);
    Ok(json!({
        "copy": copy_title,
        "planted": planted,
        "asked": count,
        "by_kind": by_kind,
        "not_planted": not_planted,
        "seed": seed,
        "note": "the copy is open and current; what was planted is kept in it, not shown here: run validate on it, then validate.status says which were caught",
        "next": CMD_START,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_grow_and_gfci_words_drop() {
        assert_eq!(bigger_size("10x8"), "14x10");
        assert_eq!(bigger_size("8\""), "10");
        assert_eq!(bigger_size(""), "12x10");
        assert_eq!(without_gfci("GFCI"), "DUPLEX");
        assert_eq!(without_gfci("DUPLEX GFI WP"), "DUPLEX WP");
        assert!(has_gfci("duplex-gfci") && !has_gfci("DUPLEX"));
    }

    #[test]
    fn the_rng_is_the_same_for_a_seed() {
        let a: Vec<u64> = (0..5).scan(Rng(7), |r, _| Some(r.next())).collect();
        let b: Vec<u64> = (0..5).scan(Rng(7), |r, _| Some(r.next())).collect();
        let c: Vec<u64> = (0..5).scan(Rng(8), |r, _| Some(r.next())).collect();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn provenance_values_are_replaced_not_repeated() {
        let mut x = vec![RawTag { code: 1001, value: PROV_APP.into() }, RawTag { code: 1000, value: "status=fixed".into() }];
        set_prov(&mut x, "field.size", "12x10");
        set_prov(&mut x, "field.size", "14x10");
        let sizes: Vec<&str> = x.iter().filter(|t| t.value.starts_with("field.size=")).map(|t| t.value.as_str()).collect();
        assert_eq!(sizes, ["field.size=14x10"]);
        let mut y = Vec::new();
        set_prov(&mut y, "field.mounting", "surface");
        assert_eq!(y.len(), 2);
    }

    #[test]
    fn answers_are_accept_or_reject() {
        assert_eq!(judge_answer(&json!("Accept")), Ok("accept"));
        assert_eq!(judge_answer(&json!({"verdict": "reject"})), Ok("reject"));
        assert!(judge_answer(&json!("maybe")).is_err());
    }

    #[test]
    fn folders_are_safe_and_distinct() {
        let a = folder_of("L1/103#1");
        let b = folder_of("L1/103_1");
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || "-._".contains(c)), "{a}");
        assert_ne!(a, b);
    }
}
