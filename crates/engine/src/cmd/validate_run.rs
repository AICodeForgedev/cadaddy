//! The Validate run (Q6116): CADaddy drives, a connected agent judges, findings come back as
//! review items with the evidence side by side.
//!
//! CADaddy talks to the network only as PRIVACY.md says, so the judge is the agent connected over
//! MCP or the control channel. CADaddy holds the rules (`validate.rules`), makes the evidence
//! packets (`validate.packet`), hands them out one area at a time and keeps the results:
//!
//! - `validate {scope: selection|area|room|level|building, area?, room?, level?, trades?, rules?,
//!   out_dir?, force?}` starts a run, or resumes the run of the same scope, trades and rules:
//!   lists its areas, runs `bim.validate` on the scope first (the deterministic pre-screen) and
//!   returns `{run, areas, next: "validate.next"}`. An area whose model and rules are unchanged
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

/// What the status bar says while a run started in the app has areas left and no agent watches.
pub const WAITING: &str = "Validate: waiting for an agent";

/// The drawing's agent-state keys: the runs, and a planted copy's record.
const KEY: &str = "validate";
const PLANT_KEY: &str = "validate_plant";
const RUNS_KEPT: usize = 10;
const MAX_RUN_AREAS: usize = 10_000;
const MAX_FINDINGS: usize = 300;
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
            "{scope?: selection|area|room|level|building (building), area?: id from validate.areas | {area: [x0,y0,x1,y1] (feet), level} (scope area), room?: number or name (scope room), level?: name (scope level), trades?: [architecture|electrical|mechanical|plumbing|fire|general], rules?: [rule id], out_dir?: folder for the packets, each run in a folder of its own named by its id (default: a folder named after the drawing plus \" validate\", beside it), force?: bool (judge unchanged areas again)} → {run, areas: [{id, status}], areas_total, pending, unchanged, prescreen, out_dir, next: \"validate.next\", loop, waiting?}: starts (or resumes) a Validate run. The judge is the connected agent: it loops validate.next → judges the packet by its rules → validate.report, until done; then validate.status",
        ),
        CommandSpec::new(CMD_NEXT, "Next Validation Packet", run_next).files().params(
            "{run?: id (the latest), area?: id (this one again)} → {run, area, packet: {dir, manifest, prompt, objects, rules}, rules, images, prompt?, remaining, report: {call: \"validate.report\", params}} | {run, done: true, next: \"validate.status\"}: writes the next area's evidence packet and hands it to the judge; read prompt.md and the pictures, judge every rule for every element, then validate.report",
        ),
        CommandSpec::new(CMD_REPORT, "Report Validation Findings", run_report).params(
            "{run?: id (the latest), area: id, findings: [{rule: id from the packet's rules, handle?: model handle (none for missing; required for extra), verdict: pass|fail|missing|extra|unsure, issue, expected, found, evidence: [{image: packet file (prints/… for fail, missing, extra), sheet, what}], confidence: 0-1, severity?: high|medium|low}]} → {run, area, stored, by_verdict, review_items, remaining, next}: keeps the judge's findings for the area (replacing an earlier report); a report with a bad finding is refused whole, with every problem said; fail, missing, extra and unsure become review items (kind validation)",
        ),
        CommandSpec::new(CMD_STATUS, "Validation Run Status", run_status)
            .params(
                "{run?: id (the latest)} → {run, scope, areas: {total, done, issued, pending, unchanged}, findings: {total, by_verdict, by_trade, by_rule}, accuracy: {overall, by_trade, by_rule} (pass / (pass+fail+missing+extra)), review: {accepted, rejected, open, deferred, false_alarm_rate}, prescreen, planted?: {total, caught, missed, recall, errors}, runs, next}: how far the run is and how the model scores",
            )
            .noundo(),
        CommandSpec::new(CMD_PLANT, "Plant Validation Errors", run_plant)
            .params(
                "{count?: 1-50 (6), kinds?: [move|delete|duct_size|panel_mount|fan_room|drop_gfci], seed?: n (1)} → {copy, planted, by_kind, not_planted, seed}: opens a copy of the drawing with known errors planted (a device moved 3 ft, one deleted, a duct size changed, a panel's mounting flipped, a fan moved to the next room, a GFCI type dropped); they are recorded in the copy only; run validate on the copy and validate.status says which were caught",
            )
            .noundo(),
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
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Finding {
    id: String,
    rule: String,
    trade: String,
    handle: Option<String>,
    verdict: String,
    issue: String,
    expected: String,
    found: String,
    evidence: Vec<Evidence>,
    confidence: f64,
    severity: String,
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

fn hash8(s: &str) -> String {
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

/// The areas of a scope, and the scope as kept in the run.
fn scope_areas(
    s: &Session,
    cmd: &str,
    p: &Value,
    d: &Drawing,
    sp: &cadaddy_walk3d::BuildingSpec,
    model: &Model,
    frame: &Frame,
) -> Result<(Value, Vec<Area>)> {
    let scope = str_param(p, "scope").map(|x| x.trim().to_ascii_lowercase()).unwrap_or_else(|| "building".into());
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
            let base = |id: &str| id.split('#').next().unwrap_or(id).to_string();
            let a: Vec<Area> = all
                .into_iter()
                .filter(|a| {
                    a.number.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(r))
                        || a.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(r))
                        || a.id.eq_ignore_ascii_case(r)
                        || base(&a.id).eq_ignore_ascii_case(r)
                })
                .collect();
            if a.is_empty() {
                return Err(bad(cmd, format!("no model room is numbered or named {} (validate.areas lists them)", cut(r, 40))));
            }
            (json!({"scope": "room", "room": r}), a)
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

/// The deterministic pre-screen: `bim.validate` round the scope's areas, level by level.
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
    })
}

fn remaining(run: &Run) -> usize {
    run.areas.iter().filter(|a| a.status != "done").count()
}

/// Whether the active drawing has a run with areas left that the app started.
fn waiting_run(d: &Drawing) -> Option<String> {
    let st = load(d);
    st.runs.iter().filter(|r| r.by == "user" && remaining(r) > 0).max_by_key(|r| r.started).map(|r| r.id.clone())
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
    let pre = prescreen(s, &frame, &scope_name, &areas);
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

// ---------------------------------------------------------------- validate.next

fn read_json(path: &std::path::Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn run_next(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = CMD_NEXT;
    let mut st = load(s.doc()?);
    let run = run_param(CMD, &st, p)?.clone();
    let want = str_param(p, "area").map(str::trim).filter(|a| !a.is_empty());
    let pick = match want {
        Some(a) => Some(
            run.areas
                .iter()
                .position(|x| x.id.eq_ignore_ascii_case(a))
                .ok_or_else(|| bad(CMD, format!("area {} is not in run {} (validate.status lists its areas)", cut(a, 60), run.id)))?,
        ),
        None => run.areas.iter().position(|x| x.status == "issued").or_else(|| run.areas.iter().position(|x| x.status == "pending")),
    };
    let Some(i) = pick else {
        return Ok(
            json!({"run": run.id, "done": true, "remaining": 0, "areas": area_counts(&run), "next": CMD_STATUS, "message": "every area is judged: validate.status has the scores"}),
        );
    };
    let Some(area) = run.areas.get(i).cloned() else { return Err(bad(CMD, "the run's area list changed: call validate.next again")) };
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
    let list = match p.get("findings") {
        Some(Value::Array(a)) => a,
        _ => {
            return Err(bad(
                CMD,
                "`findings` is a list: [{rule, handle?, verdict, issue, expected, found, evidence: [{image, sheet, what}], confidence, severity?}]",
            ));
        }
    };
    if list.is_empty() && !area.rules.is_empty() {
        return Err(bad(
            CMD,
            format!(
                "`findings` is empty: check every rule of the packet ({}) for every element it applies to; a pass is a finding too",
                area.rules.join(", ")
            ),
        ));
    }
    if list.len() > MAX_FINDINGS {
        return Err(bad(CMD, format!("{} findings: at most {MAX_FINDINGS} per area (one per rule and element)", list.len())));
    }
    let d = s.doc()?;
    let (all, _) = vr::effective(&d.project_rules).map_err(|e| bad(CMD, e))?;
    let dir = std::path::Path::new(&run.out_dir).join(&area.dir);
    // The trade of each element of the packet (a general rule's finding takes its element's).
    let trades: BTreeMap<String, String> = read_json(&dir.join("objects.json"))
        .map(|o| {
            ["elements", "runs"]
                .iter()
                .flat_map(|k| o[*k].as_array().cloned().unwrap_or_default())
                .filter_map(|e| Some((e["handle"].as_str()?.to_ascii_uppercase(), e["trade"].as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let mut problems: Vec<String> = Vec::new();
    let mut findings: Vec<Finding> = Vec::new();
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
        let verdict = o.get("verdict").and_then(Value::as_str).map(|x| x.trim().to_ascii_lowercase()).unwrap_or_default();
        if !VERDICTS.contains(&verdict.as_str()) {
            problems.push(format!("finding {n}: `verdict` is {} (got {})", VERDICTS.join(", "), cut(&text_of(o.get("verdict")), 20)));
        }
        let handle = match o.get("handle") {
            None | Some(Value::Null) => None,
            Some(Value::String(h)) if h.trim().is_empty() => None,
            Some(Value::String(h)) => match Handle::parse_hex(h.trim()).filter(|x| d.model.get(*x).is_some()) {
                Some(x) => Some(x.hex()),
                None => {
                    problems.push(format!("finding {n}: handle `{}` is not an object of the drawing (objects.json lists the area's)", cut(h, 20)));
                    None
                }
            },
            Some(_) => {
                problems.push(format!("finding {n}: `handle` is a handle string"));
                None
            }
        };
        if verdict == "missing" && handle.is_some() {
            problems.push(format!(
                "finding {n}: a `missing` finding has no handle (the model has nothing there; say what the print shows in `expected`)"
            ));
        }
        if verdict == "extra" && handle.is_none() && problems.len() == before {
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
        match o.get("evidence") {
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
                if is_problem(&verdict) && !evidence.is_empty() && !evidence.iter().any(|e| e.image.starts_with("prints/")) {
                    problems.push(format!("finding {n}: a {verdict} finding cites at least one print picture (prints/…): what the documents show"));
                }
            }
            _ => problems.push(format!("finding {n}: `evidence` lists the pictures it rests on: [{{image, sheet, what}}]")),
        }
        if problems.len() > before {
            continue;
        }
        let trade = all
            .iter()
            .find(|e| e.rule.id == rule)
            .map(|e| e.rule.trade.clone())
            .filter(|t| t != "general")
            .or_else(|| handle.as_ref().and_then(|h| trades.get(&h.to_ascii_uppercase()).cloned()))
            .unwrap_or_else(|| "general".into());
        let id = format!("F{}", hash8(&format!("{rule}|{handle:?}|{verdict}|{issue}")));
        if findings.iter().any(|x| x.id == id) {
            continue;
        }
        findings.push(Finding {
            id,
            rule,
            trade,
            handle,
            verdict,
            issue,
            expected: cut(&text_of(o.get("expected")), TEXT_MAX),
            found: cut(&text_of(o.get("found")), TEXT_MAX),
            evidence,
            confidence: r3(confidence.unwrap_or(0.0)),
            severity,
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
    let mut by_verdict: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &findings {
        if let Some(v) = VERDICTS.iter().find(|v| **v == f.verdict) {
            *by_verdict.entry(v).or_default() += 1;
        }
    }
    let items = findings.iter().filter(|f| f.verdict != "pass").count();
    let stored = findings.len();
    let mut left = 0;
    if let Some(r) = run_index(&st, &run.id).and_then(|k| st.runs.get_mut(k)) {
        if let Some(a) = r.areas.get_mut(i) {
            a.findings = findings;
            a.status = "done".into();
            a.unchanged = false;
        }
        left = remaining(r);
    }
    save(s, &st)?;
    s.event("validate.reported", json!({"run": run.id, "area": area.id, "findings": stored, "review_items": items, "remaining": left}));
    Ok(json!({
        "run": run.id,
        "area": area.id,
        "stored": stored,
        "by_verdict": by_verdict,
        "review_items": items,
        "remaining": left,
        "next": if left > 0 { CMD_NEXT } else { CMD_STATUS },
    }))
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
                    title: cut(&format!("{} {} ({}): {}", f.rule, f.verdict, a.label, f.issue), 120),
                    detail: json!({
                        "run": run.id, "area": a.id, "rule": f.rule, "trade": f.trade, "verdict": f.verdict, "handle": f.handle,
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
    fn add(&mut self, verdict: &str) {
        match verdict {
            "pass" => self.pass += 1,
            "fail" => self.fail += 1,
            "missing" => self.missing += 1,
            "extra" => self.extra += 1,
            _ => self.unsure += 1,
        }
    }
    fn score(&self) -> Value {
        let judged = self.pass + self.fail + self.missing + self.extra;
        if judged == 0 { Value::Null } else { json!(r3(self.pass as f64 / judged as f64)) }
    }
    fn to_json(&self) -> Value {
        json!({"pass": self.pass, "fail": self.fail, "missing": self.missing, "extra": self.extra, "unsure": self.unsure, "accuracy": self.score()})
    }
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
                        f.handle.as_deref().is_some_and(|h| h.eq_ignore_ascii_case(&e.handle))
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
    let mut overall = Tally::default();
    let mut by_trade: BTreeMap<String, Tally> = BTreeMap::new();
    let mut by_rule: BTreeMap<String, Tally> = BTreeMap::new();
    let (mut accepted, mut rejected, mut open, mut deferred) = (0usize, 0usize, 0usize, 0usize);
    for a in &run.areas {
        for f in &a.findings {
            overall.add(&f.verdict);
            by_trade.entry(f.trade.clone()).or_default().add(&f.verdict);
            by_rule.entry(f.rule.clone()).or_default().add(&f.verdict);
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
        "by": run.by,
        "out_dir": run.out_dir,
        "areas": area_counts(run),
        "findings": {
            "total": overall.pass + overall.fail + overall.missing + overall.extra + overall.unsure,
            "by_verdict": overall.to_json(),
            "by_trade": by_trade.iter().map(|(k, t)| (k.clone(), t.to_json())).collect::<serde_json::Map<_, _>>(),
            "by_rule": by_rule.iter().map(|(k, t)| (k.clone(), t.to_json())).collect::<serde_json::Map<_, _>>(),
        },
        "accuracy": {
            "overall": overall.score(),
            "by_trade": by_trade.iter().map(|(k, t)| (k.clone(), t.score())).collect::<serde_json::Map<_, _>>(),
            "by_rule": by_rule.iter().map(|(k, t)| (k.clone(), t.score())).collect::<serde_json::Map<_, _>>(),
        },
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
    if let Some(plant) = plant_of(d) {
        out["planted"] = planted_report(&plant, run, &all, reveal);
    }
    if run.by == "user" && left > 0 && !s.events.on {
        out["waiting"] = json!(WAITING);
    }
    if run.left_out > 0 {
        out["left_out"] = json!(run.left_out);
    }
    if out.to_string().len() > super::agentready::ANSWER_MAX {
        out["findings"]["by_rule"] = json!("over the answer's size: see accuracy.by_rule");
        if let Some(pl) = out.get_mut("planted").and_then(Value::as_object_mut) {
            pl.insert("errors".into(), json!("too many to list here"));
        }
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
