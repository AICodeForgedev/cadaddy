//! The accuracy trend (Q6193): one build measured without an agent, one row per build, and the
//! rows compared so a step back is found the day it lands, not when someone compares runs by eye.
//!
//! - `validate.measure {commit?, label?, rooms?, skip_rules?, job_s?, peak_mb?, path?, force?}`
//!   measures the open, built model and returns ONE row keyed by `commit`:
//!   - Gate A, worked out as the Gate A script works it from the building prescreen and the
//!     model's areas: wall devices on a wall face (`host_pass_pct`, `host_fail_far` over 12 in),
//!     devices at a documented height (`height_pass_pct`, `height_unmeasured`) and devices
//!     outside every room of their level (`outside_building`);
//!   - G-COVERAGE when the prescreen has it: kinds defined, failing, passing, with zero model
//!     elements, and the failing kinds' names;
//!   - every prescreen rule's verdict counts and accuracy over the building and over the fixed
//!     rooms the caller names (`rooms`: the Validate round's rooms, from its config, never code);
//!   - the takeoff's totals by trade and system, the model's elements by kind;
//!   - the job's wall time and peak memory as the caller measured them (`job_s`, `peak_mb`),
//!     and the job's own stage times.
//!
//!   Every number is in `metrics`, flat (`gate_a.host_pass_pct`, `rooms.G-HOST.accuracy_pct`,
//!   `takeoff.electrical/power.ft`), so rows compare key by key; the lists are in `detail`.
//! - `validate.trend {path, row? | row_file?, last?, merge_log?, commits?, table?, svg?, force?}`
//!   appends a row to the trend file (JSON lines, append-only), then compares the newest row
//!   with the one before it by the rules data (`validate_trend.json`: each metric's better
//!   direction and tolerance). A metric worse beyond its tolerance is a REGRESSION finding naming
//!   the metric, both values and both commits (a stable count that rose is a CHANGE to explain,
//!   listed, no step back), the commits between (`commits`, as the caller's
//!   git gives them) and, with the merger's batch log, the batches and branches merged between
//!   the two builds as the suspects. It returns a table of the last N rows (and writes it, and a
//!   small SVG plot, when asked). A small index beside the trend file (`<path>.idx`) keeps the
//!   row counts and where the last rows are, so a call reads only what was appended since and the
//!   rows it shows: its cost doesn't grow with the project's history (Q6479).
//!
//! Nothing here changes the drawing: the measure runs the read-only checks (`validate.prescreen`,
//! `validate.areas`, `bim.takeoff` with `keep: false`, `job.status`) and reads the elements' kinds.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::*;
use crate::{Result, Session};

pub const CMD_MEASURE: &str = "validate.measure";
pub const CMD_TREND: &str = "validate.trend";
/// What a row says it is.
pub const SCHEMA: &str = "cadaddy.accuracy/1";
const RULES_JSON: &str = include_str!("validate_trend.json");

/// Fixed rooms measured, at most.
const MAX_ROOMS: usize = 50;
/// Characters of a commit, label or name kept.
const MAX_TEXT: usize = 80;
/// Metrics of one row, at most (a row past this is junk).
const MAX_METRICS: usize = 20_000;
/// Takeoff lines read, at most.
const MAX_LINES: usize = 50_000;
/// Drawing objects looked at for the element counts, at most (every one).
const MAX_ENTITIES: usize = super::validate::MAX_ENTITIES;
/// Failing coverage kinds named in a row.
const MAX_KINDS_NAMED: usize = 100;
/// Areas read from `validate.areas` per page, and pages at most.
const AREA_PAGE: usize = 200;
const MAX_AREA_PAGES: usize = 1_000;
/// Gate A: a G-HOST failure further than this off a wall face is a floater, inches.
const FAR_IN: f64 = 12.0;
/// Gate A: a device this far outside every area box of its level is outside the building, feet.
const OUTSIDE_FT: f64 = 2.0;
/// The trend file and the merge log, bytes at most.
const MAX_FILE: u64 = 64 << 20;
/// Rows shown in the table by default and at most.
const DEFAULT_LAST: usize = 10;
const MAX_LAST: usize = 200;
/// Commits between two rows the caller may name, and batches named as suspects, at most.
const MAX_COMMITS: usize = 500;
const MAX_SUSPECTS: usize = 50;
/// Regression findings returned, at most (the count says how many there were).
const MAX_FINDINGS: usize = 500;
/// The shortest commit prefix two commits are matched by.
const MIN_SHA: usize = 7;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD_MEASURE, "Measure Build Accuracy", run_measure)
            .params(
                "{commit?: text (the build's commit; the row's key), label?: text (the job or machine, e.g. \"Project A\"), rooms?: [number | {room, level?}] (the fixed Validate rooms, at most 50: from the round's config), skip_rules?: [rule id] (prescreen rules the rooms aren't measured by), job_s?: seconds (the job's wall time as the caller timed it), peak_mb?: MB (the job's peak memory), path?: .json file (the row), force?: bool (replace an existing file)} → {schema, commit, at, label, metrics: {name: number} (gate_a.*, coverage.*, building.<rule>.*, rooms.<rule>.*, rooms.found, takeoff.<trade>/<system>.<unit>, elements.<kind>, elements.total, build.job_s, build.peak_mb, build.measure_s), detail: {rooms: {found, missing, by_room}, coverage_failing, stages_s, errors}, path?}: one build measured with no agent — Gate A (as the Gate A script works it), G-COVERAGE, every prescreen rule over the building and the fixed rooms, the takeoff by trade and system, elements by kind, job time and peak memory. Changes nothing in the drawing.",
            )
            .noundo()
            .files(),
        CommandSpec::new(CMD_TREND, "Accuracy Trend", run_trend)
            .params(
                "{path: .jsonl trend file (one row per line; made when missing, only ever appended to), row?: a validate.measure row (appended first), row_file?: .json file holding one, last?: n (rows in the table, 10, at most 200), merge_log?: the merger's batch log (lines `TIME BATCH merged BRANCH(notes) BRANCH -> next COMMIT`), commits?: [text] (the commits between the previous row and the newest, as `git log --oneline` gives them), table?: .md file, svg?: .svg file (a plot of the percentages), force?: bool (replace an existing table or plot)} → {rows, appended, skipped_lines, index: {used, read_bytes} (the small index kept beside the trend file as <path>.idx, so a call reads only what was appended and the rows it shows), compared?: {from, to}, regressions, changes, findings: [{verdict: regression|change, metric, previous, current, change, allowed, better, why, issue, from, to, commits, suspects: [{batch, next, branches}]}], table, table_path?, svg_path?}: the newest row against the one before it by the rules data (each metric's better direction and tolerance); a metric worse beyond its tolerance is a REGRESSION naming both values, both commits and the batches merged between them (a stable count that rose: a CHANGE, listed, no step back).",
            )
            .noundo()
            .files(),
    ]
}

// ---------------------------------------------------------------- the rules data

#[derive(Debug, Clone, Deserialize)]
pub struct MetricRule {
    pub key: String,
    /// `higher`, `lower` or `stable` (any move beyond the tolerance).
    pub better: String,
    #[serde(default)]
    pub abs: f64,
    #[serde(default)]
    pub share: f64,
    #[serde(default)]
    pub why: String,
    /// A number the caller gives (the job's time and memory): a row without it isn't a step back.
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Column {
    pub key: String,
    pub title: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rules {
    pub source: String,
    pub metrics: Vec<MetricRule>,
    #[serde(default)]
    pub table: Vec<Column>,
    #[serde(default)]
    pub plot: Vec<String>,
}

/// The built-in tolerances (`validate_trend.json`).
pub fn rules() -> Result<&'static Rules> {
    static R: OnceLock<std::result::Result<Rules, String>> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES_JSON).map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| bad(CMD_TREND, format!("validate_trend.json: {e}")))
}

/// `*` in `pattern` stands for any run of characters.
fn glob(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut i, mut j) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while j < t.len() {
        match p.get(i) {
            Some('*') => {
                star = Some((i, j));
                i += 1;
            }
            Some(c) if *c == t[j] => {
                i += 1;
                j += 1;
            }
            _ => match star {
                Some((si, sj)) => {
                    i = si + 1;
                    j = sj + 1;
                    star = Some((si, sj + 1));
                }
                None => return false,
            },
        }
    }
    p.get(i..).is_some_and(|rest| rest.iter().all(|c| *c == '*'))
}

impl Rules {
    /// The first rule whose key matches the metric.
    pub fn rule_for(&self, metric: &str) -> Option<&MetricRule> {
        self.metrics.iter().find(|r| glob(&r.key, metric))
    }
}

// ---------------------------------------------------------------- small helpers

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// A metric name's part: no `.` (the separator) and no spaces.
fn part(s: &str) -> String {
    let t: String = s.trim().chars().take(MAX_TEXT).map(|c| if c == '.' || c.is_whitespace() { '_' } else { c }).collect();
    if t.is_empty() { "none".into() } else { t }
}

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn put(m: &mut BTreeMap<String, f64>, k: impl Into<String>, v: f64) {
    if v.is_finite() && m.len() < MAX_METRICS {
        // `+ 0.0`: no negative zero (an empty sum) in a row.
        m.insert(k.into(), r3(v) + 0.0);
    }
}

fn add(m: &mut BTreeMap<String, f64>, k: impl Into<String>, v: f64) {
    if !v.is_finite() {
        return;
    }
    let k = k.into();
    if m.len() < MAX_METRICS || m.contains_key(&k) {
        *m.entry(k).or_insert(0.0) += v;
    }
}

fn num(v: &Value) -> f64 {
    v.as_f64().filter(|x| x.is_finite()).unwrap_or(0.0)
}

/// A rule's verdict counts as metrics under `prefix.<rule>`, with its accuracy (pass over pass,
/// fail, missing and extra) and its problems (fail, missing, extra, conflict); the sums over
/// every rule under `prefix.all`.
fn rule_metrics(m: &mut BTreeMap<String, f64>, prefix: &str, checked: &BTreeMap<String, BTreeMap<String, f64>>) {
    let mut all: BTreeMap<String, f64> = BTreeMap::new();
    for (rule, verdicts) in checked {
        for (v, n) in verdicts {
            put(m, format!("{prefix}.{}.{}", part(rule), part(v)), *n);
            *all.entry(v.clone()).or_insert(0.0) += n;
        }
        accuracy(m, &format!("{prefix}.{}", part(rule)), verdicts);
    }
    if !checked.is_empty() {
        accuracy(m, &format!("{prefix}.all"), &all);
    }
}

fn accuracy(m: &mut BTreeMap<String, f64>, at: &str, v: &BTreeMap<String, f64>) {
    let g = |k: &str| v.get(k).copied().unwrap_or(0.0);
    let judged = g("pass") + g("fail") + g("missing") + g("extra");
    if judged > 0.0 {
        put(m, format!("{at}.accuracy_pct"), r1(100.0 * g("pass") / judged));
    }
    put(m, format!("{at}.problems"), g("fail") + g("missing") + g("extra") + g("conflict"));
}

/// A prescreen answer's `checked`: rule → verdict → count.
fn checked_of(r: &Value) -> BTreeMap<String, BTreeMap<String, f64>> {
    r["checked"]
        .as_object()
        .map(|o| {
            o.iter()
                .take(500)
                .map(|(rule, vs)| {
                    (rule.clone(), vs.as_object().map(|v| v.iter().take(50).map(|(k, n)| (k.clone(), num(n))).collect()).unwrap_or_default())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn merge_checked(into: &mut BTreeMap<String, BTreeMap<String, f64>>, from: &BTreeMap<String, BTreeMap<String, f64>>) {
    for (rule, vs) in from {
        let e = into.entry(rule.clone()).or_default();
        for (v, n) in vs {
            *e.entry(v.clone()).or_insert(0.0) += n;
        }
    }
}

fn text_param(p: &Value, key: &str) -> Option<String> {
    match p.get(key) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(cut(s.trim(), MAX_TEXT)),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn number_param(cmd: &str, p: &Value, key: &str) -> Result<Option<f64>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite() && *x >= 0.0 && *x < 1e12)
            .map(Some)
            .ok_or_else(|| bad(cmd, format!("`{key}` is a number of 0 or more"))),
    }
}

/// A fixed room to measure: its number or name, and its level when given.
#[derive(Debug, Clone, PartialEq)]
struct RoomPick {
    room: String,
    level: Option<String>,
}

fn rooms_param(p: &Value) -> Result<Vec<RoomPick>> {
    let list = match p.get("rooms") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(a)) => a,
        Some(_) => return Err(bad(CMD_MEASURE, "`rooms` is a list of room numbers or {room, level}")),
    };
    if list.len() > MAX_ROOMS {
        return Err(bad(CMD_MEASURE, format!("`rooms` lists at most {MAX_ROOMS} rooms")));
    }
    let mut out: Vec<RoomPick> = Vec::new();
    for r in list {
        let pick = match r {
            Value::String(_) | Value::Number(_) => text_param(&json!({"r": r}), "r").map(|room| RoomPick { room, level: None }),
            Value::Object(_) => text_param(r, "room").map(|room| RoomPick { room, level: text_param(r, "level") }),
            _ => None,
        }
        .ok_or_else(|| bad(CMD_MEASURE, format!("a room is a number or name, or {{room, level}}: {}", cut(&r.to_string(), 60))))?;
        if !out.contains(&pick) {
            out.push(pick);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- validate.measure

/// Gate A from the building prescreen's answer, its findings and the model's areas, worked out
/// as the Gate A script works it (so its numbers and the trend's are the same).
fn gate_a(m: &mut BTreeMap<String, f64>, r: &Value, findings: &[Value], boxes: &BTreeMap<String, Vec<[f64; 4]>>) {
    let c = &r["checked"];
    let (hp, hf) = (num(&c["G-HOST"]["pass"]), num(&c["G-HOST"]["fail"]));
    let (tp, tf) = (num(&c["G-HEIGHT"]["pass"]), num(&c["G-HEIGHT"]["fail"]));
    put(m, "gate_a.elements", num(&r["elements"]));
    if hp + hf > 0.0 {
        put(m, "gate_a.host_pass_pct", r1(100.0 * hp / (hp + hf)));
    }
    put(m, "gate_a.host_pass", hp);
    put(m, "gate_a.host_fail", hf);
    let far = findings
        .iter()
        .filter(|f| f["verdict"] == "fail" && f["rule"] == "G-HOST" && f["measured"]["off_face_in"].as_f64().is_some_and(|d| d > FAR_IN))
        .count();
    put(m, "gate_a.host_fail_far", far as f64);
    put(m, "gate_a.height_measured", tp + tf);
    if tp + tf > 0.0 {
        put(m, "gate_a.height_pass_pct", r1(100.0 * tp / (tp + tf)));
    }
    let unmeasured: f64 = r["unmeasured"]["G-HEIGHT"].as_object().map_or(0.0, |o| o.values().filter_map(Value::as_u64).map(|n| n as f64).sum());
    put(m, "gate_a.height_unmeasured", unmeasured);
    // Outside: a finding with a point on a level of areas, outside every area box (+2 ft); one
    // per handle (findings with no handle count once together, as the script counts them).
    let mut outside: BTreeSet<String> = BTreeSet::new();
    for f in findings {
        let (Some(at), Some(level)) = (f["at"].as_array(), f["level"].as_str()) else { continue };
        let (Some(x), Some(y)) = (at.first().and_then(Value::as_f64), at.get(1).and_then(Value::as_f64)) else { continue };
        let Some(bs) = boxes.get(level) else { continue };
        let inside = bs.iter().any(|b| b[0] - OUTSIDE_FT <= x && x <= b[2] + OUTSIDE_FT && b[1] - OUTSIDE_FT <= y && y <= b[3] + OUTSIDE_FT);
        if !inside {
            outside.insert(f["handle"].to_string());
        }
    }
    put(m, "gate_a.outside_building", outside.len() as f64);
}

/// The model's areas by level: their boxes (feet), read as `validate.areas` lists them.
fn area_boxes(s: &mut Session) -> BTreeMap<String, Vec<[f64; 4]>> {
    let mut boxes: BTreeMap<String, Vec<[f64; 4]>> = BTreeMap::new();
    let mut cursor = 0u64;
    for _ in 0..MAX_AREA_PAGES {
        let Ok(r) = s.execute(super::validate_packet::CMD_AREAS, &json!({"cursor": cursor, "limit": AREA_PAGE})) else { break };
        let areas = r["areas"].as_array().map_or(&[][..], Vec::as_slice);
        for a in areas {
            let b = a["box"].as_array().map(|b| b.iter().filter_map(Value::as_f64).collect::<Vec<_>>()).unwrap_or_default();
            if let (Some(level), [x0, y0, x1, y1]) = (a["level"].as_str(), b.as_slice()) {
                boxes.entry(level.to_string()).or_default().push([*x0, *y0, *x1, *y1]);
            }
        }
        match r["next"].as_u64() {
            Some(n) if n > cursor && !areas.is_empty() => cursor = n,
            _ => break,
        }
    }
    boxes
}

/// G-COVERAGE's counts from the prescreen answer (absent when the prescreen has no such rule).
fn coverage(m: &mut BTreeMap<String, f64>, r: &Value) -> Vec<String> {
    let c = &r["coverage"];
    if !c.is_object() {
        return Vec::new();
    }
    put(m, "coverage.kinds_defined", num(&c["kinds_defined"]));
    for v in ["fail", "pass", "unsure", "not_marked"] {
        put(m, format!("coverage.{v}"), num(&c["by_verdict"][v]));
    }
    put(m, "coverage.zero_in_model", num(&c["zero_in_model"]));
    put(m, "coverage.unexplained_groups", num(&c["unexplained_groups"]));
    c["kinds"]
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter(|k| k["verdict"] == "fail")
        .take(MAX_KINDS_NAMED)
        .map(|k| {
            let name = k["name"].as_str().or_else(|| k["kind"].as_str()).unwrap_or("?");
            format!("{} ({} marks, {} in the model)", cut(name, MAX_TEXT), num(&k["plan_marks"]), num(&k["model"]))
        })
        .collect()
}

fn takeoff(m: &mut BTreeMap<String, f64>, r: &Value) {
    let mut sums: BTreeMap<String, f64> = BTreeMap::new();
    for l in r["lines"].as_array().map_or(&[][..], Vec::as_slice).iter().take(MAX_LINES) {
        let trade = part(l["trade"].as_str().unwrap_or("other"));
        let system = part(l["system"].as_str().unwrap_or("none"));
        let unit = part(l["unit"].as_str().unwrap_or("ea"));
        let qty = num(&l["qty"]);
        add(&mut sums, format!("takeoff.{trade}/{system}.{unit}"), qty);
        add(&mut sums, format!("takeoff.{trade}/{system}.elements"), num(&l["element_count"]));
        add(&mut sums, format!("takeoff.{trade}.{unit}"), qty);
    }
    for (k, v) in sums {
        put(m, k, v);
    }
}

/// The model's elements by kind, as a reader says them (a device by its own kind: `receptacle`,
/// `smoke_detector`), and in all.
fn element_kinds(m: &mut BTreeMap<String, f64>, d: &cadaddy_doc::Drawing) {
    let mut by: BTreeMap<String, f64> = BTreeMap::new();
    for e in d.model.iter().take(MAX_ENTITIES) {
        if let Some(k) = super::elemprops::element_kind(e) {
            let k = super::elemprops::reported_kind(e, k);
            add(&mut by, format!("elements.{}", part(&k)), 1.0);
        }
    }
    put(m, "elements.total", by.values().sum());
    for (k, v) in by {
        put(m, k, v);
    }
}

fn run_measure(s: &mut Session, p: &Value) -> Result<Value> {
    let started = web_time::Instant::now();
    let commit = text_param(p, "commit");
    let label = text_param(p, "label");
    let rooms = rooms_param(p)?;
    let skip: BTreeSet<String> = match p.get("skip_rules") {
        None | Some(Value::Null) => BTreeSet::new(),
        Some(Value::Array(a)) => a.iter().take(100).filter_map(Value::as_str).map(|r| r.trim().to_ascii_uppercase()).collect(),
        Some(_) => return Err(bad(CMD_MEASURE, "`skip_rules` is a list of prescreen rule ids")),
    };
    let job_s = number_param(CMD_MEASURE, p, "job_s")?;
    let peak_mb = number_param(CMD_MEASURE, p, "peak_mb")?;
    let path = str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()).map(str::to_string);
    if let Some(f) = &path {
        super::exchange::guard_output(s, CMD_MEASURE, p, f)?;
    }
    s.doc()?;
    let mut m: BTreeMap<String, f64> = BTreeMap::new();
    let mut errors: Vec<String> = Vec::new();
    let mut note = |what: &str, e: &crate::EngineError| errors.push(cut(&format!("{what}: {e}"), 300));

    // The building: every prescreen rule, every finding (Gate A reads them), the coverage.
    let mut failing = Vec::new();
    match super::validate_prescreen::prescreen(s, &json!({"scope": "building", "all": true}), true) {
        Ok((r, findings)) => {
            let boxes = area_boxes(s);
            gate_a(&mut m, &r, &findings, &boxes);
            failing = coverage(&mut m, &r);
            rule_metrics(&mut m, "building", &checked_of(&r));
        }
        Err(e) => note("building prescreen", &e),
    }

    // The fixed rooms: each room's prescreen, summed.
    let room_rules: Vec<&str> = super::validate_prescreen::RULES.iter().copied().filter(|r| !skip.contains(*r)).collect();
    let mut sum: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    let mut by_room = Map::new();
    let mut missing: Vec<String> = Vec::new();
    for pick in &rooms {
        let mut q = json!({"scope": "room", "room": pick.room, "rules": room_rules});
        if let Some(l) = &pick.level {
            q["level"] = json!(l);
        }
        let name = pick.level.as_ref().map_or_else(|| pick.room.clone(), |l| format!("{l}/{}", pick.room));
        match super::validate_prescreen::prescreen(s, &q, false) {
            Ok((r, _)) => {
                let c = checked_of(&r);
                merge_checked(&mut sum, &c);
                let mut one: BTreeMap<String, f64> = BTreeMap::new();
                rule_metrics(&mut one, "room", &c);
                by_room.insert(name, json!({"areas": r["areas"], "elements": r["elements"], "accuracy_pct": one.get("room.all.accuracy_pct"), "problems": one.get("room.all.problems"), "checked": r["checked"]}));
            }
            Err(e) => {
                missing.push(cut(&format!("{name}: {e}"), 200));
            }
        }
    }
    if !rooms.is_empty() {
        put(&mut m, "rooms.listed", rooms.len() as f64);
        put(&mut m, "rooms.found", by_room.len() as f64);
        rule_metrics(&mut m, "rooms", &sum);
    }

    // The takeoff by trade and system, the elements by kind.
    match s.execute("bim.takeoff", &json!({"group_by": "system", "keep": false})) {
        Ok(r) => takeoff(&mut m, &r),
        Err(e) => note("bim.takeoff", &e),
    }
    match s.doc() {
        Ok(d) => element_kinds(&mut m, d),
        Err(e) => note("elements", &e),
    }

    // The job: the caller's timing, else nothing (a checkpoint's earlier stages ran elsewhere);
    // its own stage times beside.
    if let Some(v) = job_s {
        put(&mut m, "build.job_s", r1(v));
    }
    if let Some(v) = peak_mb {
        put(&mut m, "build.peak_mb", v.round());
    }
    let mut stages = Map::new();
    if let Ok(r) = s.execute("job.status", &json!({})) {
        for st in r["stages"].as_array().map_or(&[][..], Vec::as_slice).iter().take(50) {
            if let (Some(name), Some(ms)) = (st["stage"].as_str(), st["elapsed_ms"].as_f64()) {
                stages.insert(cut(name, MAX_TEXT), json!(r1(ms / 1000.0)));
            }
        }
    }
    put(&mut m, "build.measure_s", r1(started.elapsed().as_secs_f64()));

    let mut row = json!({
        "schema": SCHEMA,
        "commit": commit,
        "at": super::project::timestamp(),
        "label": label,
        "metrics": m,
        "detail": {
            "rooms": {"listed": rooms.len(), "found": by_room.len(), "missing": missing, "by_room": by_room},
            "coverage_failing": failing,
            "stages_s": stages,
            "errors": errors,
        },
    });
    if let Some(f) = &path {
        let text = serde_json::to_string_pretty(&row).map_err(|e| bad(CMD_MEASURE, e.to_string()))?;
        super::exchange::write_file(CMD_MEASURE, f, text.as_bytes())?;
        row["path"] = json!(f);
    }
    Ok(row)
}

// ---------------------------------------------------------------- validate.trend

/// One row of the trend: its key and its metrics.
#[derive(Debug, Clone)]
pub struct Row {
    pub commit: String,
    pub at: String,
    pub label: String,
    pub metrics: BTreeMap<String, f64>,
}

impl Row {
    /// A row as `validate.measure` gives it; None when it holds no metrics.
    pub fn of(v: &Value) -> Option<Row> {
        let metrics: BTreeMap<String, f64> = v["metrics"]
            .as_object()?
            .iter()
            .take(MAX_METRICS)
            .filter_map(|(k, n)| Some((cut(k, 200), n.as_f64().filter(|x| x.is_finite())?)))
            .collect();
        let s = |k: &str| v[k].as_str().map(|x| cut(x, MAX_TEXT)).unwrap_or_default();
        Some(Row { commit: s("commit"), at: s("at"), label: s("label"), metrics })
    }

    fn short(&self) -> String {
        if self.commit.is_empty() { "?".into() } else { cut(&self.commit, 8) }
    }
}

/// One merger batch: its name, the commit it left `next` at and the branches it merged.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    pub batch: String,
    pub next: String,
    pub branches: Vec<String>,
}

/// The merger's batch log: each `<time> <batch> merged <branch>(…) <branch> … -> next <commit>;`
/// line (other lines and anything in parentheses are skipped).
pub fn parse_merge_log(text: &str) -> Vec<Batch> {
    let mut out = Vec::new();
    for line in text.lines().take(1_000_000) {
        let Some((head, rest)) = line.split_once(" merged ") else { continue };
        let Some((merged, after)) = rest.split_once(" -> next ") else { continue };
        let batch = head.split_whitespace().nth(1).unwrap_or_default();
        let next: String = after.chars().take_while(char::is_ascii_hexdigit).take(40).collect();
        if batch.is_empty() || next.len() < MIN_SHA {
            continue;
        }
        // The branch names: the words outside parentheses.
        let mut depth = 0usize;
        let mut plain = String::new();
        for c in merged.chars().take(100_000) {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ if depth == 0 => plain.push(c),
                _ => {}
            }
        }
        let branches: Vec<String> = plain
            .split_whitespace()
            .filter(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/')))
            .take(100)
            .map(|w| cut(w, MAX_TEXT))
            .collect();
        out.push(Batch { batch: cut(batch, MAX_TEXT), next, branches });
    }
    out
}

/// Two commits are one when one is a prefix of the other (at least 7 characters).
fn same_commit(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_ascii_lowercase(), b.trim().to_ascii_lowercase());
    a.len() >= MIN_SHA && b.len() >= MIN_SHA && (a.starts_with(&b) || b.starts_with(&a))
}

/// The batches merged after the build `from` up to the build `to`; with `commits` (the commits
/// between them) also any batch whose commit is among them.
pub fn suspects(batches: &[Batch], from: &str, to: &str, commits: &[String]) -> Vec<Batch> {
    let at = |c: &str| batches.iter().rposition(|b| same_commit(&b.next, c));
    let mut out: Vec<Batch> = match (at(from), at(to)) {
        (Some(i), Some(j)) if j > i => batches.get(i + 1..=j).map(<[Batch]>::to_vec).unwrap_or_default(),
        (None, Some(j)) => batches.get(j).cloned().into_iter().collect(),
        _ => Vec::new(),
    };
    for c in commits {
        let sha = c.split_whitespace().next().unwrap_or_default();
        for b in batches.iter().filter(|b| same_commit(&b.next, sha)) {
            if !out.contains(b) {
                out.push(b.clone());
            }
        }
    }
    out.truncate(MAX_SUSPECTS);
    out
}

/// A regression: a metric worse than the row before beyond its tolerance.
#[derive(Debug, Clone)]
pub struct Regression {
    pub metric: String,
    pub previous: Option<f64>,
    pub current: Option<f64>,
    pub allowed: f64,
    pub better: String,
    pub why: String,
    /// How far past its tolerance, in tolerances (the ranking).
    pub over: f64,
    /// A step back (`regression`), or a stable count that rose: a `change` to explain (new
    /// elements a merge builds), listed but no step back.
    pub verdict: &'static str,
}

/// The newest row against the one before, metric by metric, by the rules: worse beyond the
/// tolerance (`abs`, or `share` of the earlier value, whichever is larger) is a regression; a
/// metric the earlier row had and the newest lacks is one too ("not measured now"), unless the
/// rule says it is `optional` (a number the caller gives). A `stable` count that fell (elements
/// or quantities lost) is a regression; one that rose is a `change` to explain. Regressions
/// first, each group ranked by how far past the tolerance.
pub fn compare(rules: &Rules, previous: &Row, current: &Row) -> Vec<Regression> {
    let mut out = Vec::new();
    let keys: BTreeSet<&String> = previous.metrics.keys().chain(current.metrics.keys()).collect();
    for k in keys {
        let Some(rule) = rules.rule_for(k) else { continue };
        let (a, b) = (previous.metrics.get(k).copied(), current.metrics.get(k).copied());
        let Some(a) = a else { continue };
        let allowed = rule.abs.max(rule.share * a.abs());
        let Some(b) = b else {
            if rule.optional {
                continue;
            }
            out.push(Regression {
                metric: k.clone(),
                previous: Some(a),
                current: None,
                allowed,
                better: rule.better.clone(),
                why: "not measured now".into(),
                over: 1.0,
                verdict: "regression",
            });
            continue;
        };
        let worse = match rule.better.as_str() {
            "higher" => a - b,
            "lower" => b - a,
            _ => (b - a).abs(),
        };
        if worse > allowed + 1e-9 {
            let why = match rule.better.as_str() {
                "higher" => "fell",
                "lower" => "rose",
                _ if b > a => "rose",
                _ => "fell",
            };
            let over = if allowed > 0.0 { worse / allowed } else { worse.max(1.0) * 1_000.0 };
            let verdict = if rule.better != "higher" && rule.better != "lower" && b > a { "change" } else { "regression" };
            out.push(Regression {
                metric: k.clone(),
                previous: Some(a),
                current: Some(b),
                allowed,
                better: rule.better.clone(),
                why: why.into(),
                over,
                verdict,
            });
        }
    }
    out.sort_by(|x, y| {
        (x.verdict != "regression").cmp(&(y.verdict != "regression")).then(y.over.total_cmp(&x.over)).then_with(|| x.metric.cmp(&y.metric))
    });
    out
}

fn fmt(v: Option<f64>) -> String {
    match v {
        None => "-".into(),
        Some(x) if x == x.trunc() && x.abs() < 1e15 => format!("{:.0}", x + 0.0),
        Some(x) => format!("{}", r3(x)),
    }
}

/// The last rows as a Markdown table of the rules data's columns; a value that regressed against
/// the row before is marked `(!)`, a stable count that rose past its tolerance `(~)`.
pub fn table(rules: &Rules, rows: &[Row]) -> String {
    let mut t = String::from("| commit | at | label |");
    for c in &rules.table {
        t.push_str(&format!(" {} |", c.title));
    }
    t.push_str("\n|---|---|---|");
    for _ in &rules.table {
        t.push_str("---:|");
    }
    t.push('\n');
    for (i, r) in rows.iter().enumerate() {
        let marked: BTreeMap<String, &str> = i
            .checked_sub(1)
            .and_then(|j| rows.get(j))
            .map(|p| compare(rules, p, r).into_iter().map(|g| (g.metric, if g.verdict == "regression" { " (!)" } else { " (~)" })).collect())
            .unwrap_or_default();
        t.push_str(&format!("| {} | {} | {} |", r.short(), cut(&r.at, 16), r.label));
        for c in &rules.table {
            let mark = marked.get(&c.key).copied().unwrap_or("");
            t.push_str(&format!(" {}{mark} |", fmt(r.metrics.get(&c.key).copied())));
        }
        t.push('\n');
    }
    t
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A small line plot of the rules data's `plot` metrics (percentages) over the rows.
pub fn svg(rules: &Rules, rows: &[Row]) -> String {
    const W: f64 = 760.0;
    const H: f64 = 340.0;
    const L: f64 = 50.0;
    const R: f64 = 200.0;
    const T: f64 = 20.0;
    const B: f64 = 50.0;
    const COLOURS: [&str; 6] = ["#1f77b4", "#d62728", "#2ca02c", "#9467bd", "#ff7f0e", "#17becf"];
    let values: Vec<f64> = rules.plot.iter().flat_map(|k| rows.iter().filter_map(|r| r.metrics.get(k).copied())).collect();
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (lo, hi) = if lo.is_finite() && hi.is_finite() { ((lo - 2.0).max(0.0).floor(), (hi + 2.0).min(100.0).ceil()) } else { (0.0, 100.0) };
    let span = if hi > lo { hi - lo } else { 1.0 };
    let n = rows.len().max(1);
    let x = |i: usize| L + if n > 1 { (W - L - R) * i as f64 / (n - 1) as f64 } else { (W - L - R) / 2.0 };
    let y = |v: f64| T + (H - T - B) * (1.0 - (v - lo) / span);
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{W}\" height=\"{H}\" viewBox=\"0 0 {W} {H}\" font-family=\"sans-serif\" font-size=\"11\">\n<rect width=\"{W}\" height=\"{H}\" fill=\"white\"/>\n"
    );
    out.push_str(&format!("<line x1=\"{L}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"#888\"/>\n", H - B, W - R, H - B));
    out.push_str(&format!("<line x1=\"{L}\" y1=\"{T}\" x2=\"{L}\" y2=\"{}\" stroke=\"#888\"/>\n", H - B));
    for k in 0..=4 {
        let v = lo + span * f64::from(k) / 4.0;
        out.push_str(&format!("<text x=\"{}\" y=\"{:.1}\" text-anchor=\"end\">{}</text>\n", L - 4.0, y(v) + 4.0, r1(v)));
    }
    for (i, r) in rows.iter().enumerate() {
        out.push_str(&format!("<text x=\"{:.1}\" y=\"{}\" text-anchor=\"middle\">{}</text>\n", x(i), H - B + 16.0, xml(&cut(&r.short(), 7))));
    }
    for (si, key) in rules.plot.iter().enumerate() {
        let colour = COLOURS.get(si % COLOURS.len()).copied().unwrap_or("#000");
        let pts: Vec<String> = rows.iter().enumerate().filter_map(|(i, r)| r.metrics.get(key).map(|v| format!("{:.1},{:.1}", x(i), y(*v)))).collect();
        if !pts.is_empty() {
            out.push_str(&format!("<polyline fill=\"none\" stroke=\"{colour}\" stroke-width=\"2\" points=\"{}\"/>\n", pts.join(" ")));
            for p in &pts {
                if let Some((px, py)) = p.split_once(',') {
                    out.push_str(&format!("<circle cx=\"{px}\" cy=\"{py}\" r=\"3\" fill=\"{colour}\"/>\n"));
                }
            }
        }
        let ly = T + 16.0 * si as f64 + 8.0;
        out.push_str(&format!(
            "<line x1=\"{}\" y1=\"{ly}\" x2=\"{}\" y2=\"{ly}\" stroke=\"{colour}\" stroke-width=\"2\"/>\n",
            W - R + 12.0,
            W - R + 30.0
        ));
        out.push_str(&format!("<text x=\"{}\" y=\"{}\">{}</text>\n", W - R + 34.0, ly + 4.0, xml(key)));
    }
    out.push_str("</svg>\n");
    out
}

/// A file's text, refused past `MAX_FILE`.
fn read_capped(cmd: &str, path: &str) -> Result<String> {
    let len = std::fs::metadata(path).map_err(|e| bad(cmd, format!("{path}: {e}")))?.len();
    if len > MAX_FILE {
        return Err(bad(cmd, format!("{path} is {len} bytes, over the {MAX_FILE} this reads")));
    }
    std::fs::read_to_string(path).map_err(|e| bad(cmd, format!("{path}: {e}")))
}

fn append_row(path: &str, row: &Value) -> Result<()> {
    use std::io::Write;
    let line = serde_json::to_string(row).map_err(|e| bad(CMD_TREND, e.to_string()))?;
    if let Some(dir) = std::path::Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()) {
        crate::writefile::check(dir).map_err(|e| bad(CMD_TREND, format!("{path}: {e}")))?;
        std::fs::create_dir_all(dir).map_err(|e| bad(CMD_TREND, format!("{path}: {e}")))?;
    }
    let mut f = crate::writefile::open_append(path).map_err(|e| bad(CMD_TREND, format!("{path}: {e}")))?;
    f.write_all(format!("{line}\n").as_bytes()).map_err(|e| bad(CMD_TREND, format!("{path}: {e}")))
}

// ---------------------------------------------------------------- the trend file's index (Q6479)
//
// A trend file grows one row per measured build for the life of a job. Reading and parsing all of
// it on every call made each call slower with the history, so the file has a small index beside
// it (`<path>.idx`): how many bytes of whole lines it describes, the row and skipped-line counts
// of those bytes, where the last rows are, and a check of the bytes at its start and at its end.
// A call reads only what was appended since (normally its own row) and the few rows it shows; the
// answer is the same as a full read. The index is rebuilt by one full read when it is missing,
// unreadable or doesn't match the file: the file is shorter, the same length but changed since,
// the checked bytes differ (edited or replaced outside CADaddy), or a row it points at no longer
// reads as one.

/// What an index says it is.
const INDEX_SCHEMA: &str = "cadaddy.trend-index/1";
/// Rows whose place the index keeps: the most the table shows (the two compared are among them).
const INDEX_KEEP: usize = MAX_LAST;
/// Bytes checked at the start and at the end of what the index describes.
const CHECK_BYTES: u64 = 4096;

/// What the index knows about the first `len` bytes of a trend file (whole lines only).
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
struct TrendIndex {
    schema: String,
    /// Bytes described: through the last newline read.
    len: u64,
    /// The file's modification time when the index was written (ns since 1970), when known.
    #[serde(default)]
    modified_ns: Option<u64>,
    rows: usize,
    skipped: usize,
    /// The last rows' byte ranges `[start, end)` (the line, before trimming), oldest first.
    tail: Vec<(u64, u64)>,
    /// Checks (FNV-1a) of the first and the last `CHECK_BYTES` of the `len` bytes.
    head_check: u64,
    end_check: u64,
}

impl TrendIndex {
    fn empty() -> TrendIndex {
        TrendIndex { schema: INDEX_SCHEMA.into(), len: 0, modified_ns: None, rows: 0, skipped: 0, tail: Vec::new(), head_check: 0, end_check: 0 }
    }

    /// One line of the file at `[start, end)`: a row, a skipped line, or nothing (blank).
    fn feed(&mut self, start: u64, end: u64, line: &str) {
        let t = line.trim();
        if t.is_empty() {
            return;
        }
        if serde_json::from_str::<Value>(t).ok().as_ref().and_then(Row::of).is_some() {
            self.rows += 1;
            self.tail.push((start, end));
            if self.tail.len() > INDEX_KEEP {
                let extra = self.tail.len() - INDEX_KEEP;
                self.tail.drain(..extra);
            }
        } else {
            self.skipped += 1;
        }
    }

    /// Sound on its face: its schema, its ranges inside it and in order, its counts agreeing.
    fn sound(&self) -> bool {
        self.schema == INDEX_SCHEMA
            && self.tail.len() <= INDEX_KEEP
            && self.tail.len() <= self.rows
            && (self.rows > 0 || self.tail.is_empty())
            && self.tail.windows(2).all(|w| w[0].1 <= w[1].0)
            && self.tail.iter().all(|(a, b)| a < b && *b <= self.len)
    }
}

fn index_path(path: &str) -> String {
    format!("{path}.idx")
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

fn modified_ns(meta: &std::fs::Metadata) -> Option<u64> {
    let t = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
    u64::try_from(t.as_nanos()).ok()
}

/// `n` bytes of `f` from `at`.
fn read_at(f: &mut std::fs::File, at: u64, n: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    f.seek(SeekFrom::Start(at))?;
    let mut buf = Vec::new();
    f.take(n).read_to_end(&mut buf)?;
    Ok(buf)
}

/// The checks of the first and last `CHECK_BYTES` of the first `len` bytes of `f`.
fn checks(f: &mut std::fs::File, len: u64) -> std::io::Result<(u64, u64)> {
    let n = len.min(CHECK_BYTES);
    Ok((fnv(&read_at(f, 0, n)?), fnv(&read_at(f, len - n, n)?)))
}

/// Writes the index through a new temporary file renamed over it, so a reader never sees half of
/// one. Not synced to disk: the index is a cache, and one lost or torn only costs the next call a
/// full read (a sync costs every call more than the read it saves on a short file). Best effort,
/// never during a dry run, never into a system folder.
fn write_index(path: &str, bytes: &[u8]) {
    // The browser has no files (and no process id).
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (path, bytes);
    }
    #[cfg(not(target_arch = "wasm32"))]
    write_index_native(path, bytes);
}

#[cfg(not(target_arch = "wasm32"))]
fn write_index_native(path: &str, bytes: &[u8]) {
    use std::io::Write;
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = format!("{path}.{}-{n}.tmp", std::process::id());
    if crate::writefile::check(std::path::Path::new(path)).is_err() || crate::writefile::check(std::path::Path::new(&tmp)).is_err() {
        return;
    }
    let wrote = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp).and_then(|mut f| f.write_all(bytes));
    if wrote.is_err() || crate::writefile::rename(&tmp, path).is_err() {
        let _ = crate::writefile::remove(&tmp);
    }
}

/// The trend file as a call needs it: rows and skipped lines in all, and the last rows.
#[derive(Debug, Default)]
struct TrendRead {
    rows: usize,
    skipped: usize,
    /// The last rows (as many as asked, fewer when the file has fewer), oldest first.
    last: Vec<Row>,
    /// The index was used (false: built by a full read).
    index_used: bool,
    /// Bytes of the trend file read by this call.
    read_bytes: u64,
}

fn utf8_err(cmd: &str, path: &str) -> crate::EngineError {
    bad(cmd, format!("{path}: stream did not contain valid UTF-8"))
}

/// Reads the trend file at `path` for its counts and its last `need` rows (at most
/// `INDEX_KEEP`), through its index: only the bytes appended since the index was written and the
/// rows returned are read. Refuses a file over `MAX_FILE` and one that isn't UTF-8, as a full read
/// does. The index is brought up to date (never written during a dry run).
fn read_trend(path: &str, need: usize) -> Result<TrendRead> {
    let need = need.min(INDEX_KEEP);
    let Ok(meta) = std::fs::metadata(path) else { return Ok(TrendRead::default()) };
    let len = meta.len();
    if len > MAX_FILE {
        return Err(bad(CMD_TREND, format!("{path} is {len} bytes, over the {MAX_FILE} this reads")));
    }
    let mut f = std::fs::File::open(path).map_err(|e| bad(CMD_TREND, format!("{path}: {e}")))?;
    let io = |e: std::io::Error| bad(CMD_TREND, format!("{path}: {e}"));
    let mut read_bytes = 0u64;
    // The index, when it still describes the start of this file.
    let loaded: Option<TrendIndex> = std::fs::read(index_path(path))
        .ok()
        .filter(|b| b.len() < (1 << 20))
        .and_then(|b| serde_json::from_slice::<TrendIndex>(&b).ok())
        .filter(TrendIndex::sound);
    let mut idx = match loaded.clone() {
        Some(i) if i.len == 0 => Some(i),
        Some(i) if i.len < len || (i.len == len && i.modified_ns.is_some() && i.modified_ns == modified_ns(&meta)) => {
            let (h, e) = checks(&mut f, i.len).map_err(io)?;
            read_bytes += 2 * i.len.min(CHECK_BYTES);
            (h == i.head_check && e == i.end_check).then_some(i)
        }
        _ => None,
    };
    let mut index_used = idx.is_some();
    for _attempt in 0..2 {
        let mut ix = idx.take().unwrap_or_else(TrendIndex::empty);
        let base = ix.len;
        // What was appended since: whole lines go into the index, a last line with no newline yet
        // counts for this answer only.
        let added = read_at(&mut f, base, MAX_FILE + 1 - base.min(MAX_FILE)).map_err(io)?;
        read_bytes += added.len() as u64;
        if base + added.len() as u64 > MAX_FILE {
            return Err(bad(CMD_TREND, format!("{path} is over the {MAX_FILE} bytes this reads")));
        }
        let text = std::str::from_utf8(&added).map_err(|_| utf8_err(CMD_TREND, path))?;
        let whole = text.rfind('\n').map_or(0, |i| i + 1);
        let mut at = base;
        for line in text.get(..whole).unwrap_or_default().split_inclusive('\n') {
            let end = at + line.len() as u64;
            ix.feed(at, end, line);
            at = end;
        }
        ix.len = base + whole as u64;
        let mut view = ix.clone();
        if let Some(rest) = text.get(whole..).filter(|r| !r.trim().is_empty()) {
            view.feed(ix.len, base + added.len() as u64, rest);
        }
        // The last rows: from the bytes just read, else from where the index says they are.
        let mut last = Vec::new();
        let mut moved = false;
        for &(a, b) in view.tail.iter().skip(view.tail.len().saturating_sub(need)) {
            let bytes = if a >= base {
                let (i, j) = (usize::try_from(a - base).unwrap_or(usize::MAX), usize::try_from(b - base).unwrap_or(usize::MAX));
                added.get(i..j).map(<[u8]>::to_vec)
            } else {
                read_bytes += b - a;
                read_at(&mut f, a, b - a).ok()
            };
            match bytes
                .as_deref()
                .and_then(|x| std::str::from_utf8(x).ok())
                .and_then(|t| serde_json::from_str::<Value>(t.trim()).ok())
                .and_then(|v| Row::of(&v))
            {
                Some(r) => last.push(r),
                None => {
                    moved = true;
                    break;
                }
            }
        }
        if moved {
            // A row the index points at isn't one any more: the file changed under it. Read it all.
            idx = None;
            index_used = false;
            continue;
        }
        // Keep the index for the next call (best effort: a folder that can't be written only
        // costs the next call a full read).
        if ix.len > 0 {
            if let Ok((h, e)) = checks(&mut f, ix.len) {
                ix.head_check = h;
                ix.end_check = e;
            }
            let now = std::fs::metadata(path).ok();
            ix.modified_ns = now.as_ref().filter(|m| m.len() == ix.len).and_then(modified_ns);
            if loaded.as_ref() != Some(&ix)
                && let Ok(text) = serde_json::to_vec(&ix)
            {
                write_index(&index_path(path), &text);
            }
        }
        return Ok(TrendRead { rows: view.rows, skipped: view.skipped, last, index_used, read_bytes });
    }
    Err(bad(CMD_TREND, format!("{path} changed while it was read; try again")))
}

/// Every row of the trend file `text` and its skipped lines, read in full (the answer the index
/// must give).
#[cfg(test)]
fn read_all(text: &str) -> (Vec<Row>, usize) {
    let mut skipped = 0usize;
    let mut rows: Vec<Row> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        match serde_json::from_str::<Value>(line).ok().as_ref().and_then(Row::of) {
            Some(r) => rows.push(r),
            None => skipped += 1,
        }
    }
    (rows, skipped)
}

fn run_trend(s: &mut Session, p: &Value) -> Result<Value> {
    let rules = rules()?;
    let path = str_param(p, "path")
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .ok_or_else(|| bad(CMD_TREND, "`path` names the trend file (.jsonl)"))?
        .to_string();
    let last = match p.get("last") {
        None | Some(Value::Null) => DEFAULT_LAST,
        Some(v) => {
            v.as_u64().filter(|n| *n >= 1).map(|n| (n as usize).min(MAX_LAST)).ok_or_else(|| bad(CMD_TREND, "`last` is a whole number from 1"))?
        }
    };
    let commits: Vec<String> = match p.get("commits") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).take(MAX_COMMITS).map(|c| cut(c.trim(), 200)).filter(|c| !c.is_empty()).collect(),
        Some(_) => return Err(bad(CMD_TREND, "`commits` is a list of commits (`git log --oneline` lines)")),
    };
    // The row to append: given, or read from its file; it must be a measure's row.
    let row: Option<Value> = match (p.get("row").filter(|v| !v.is_null()), str_param(p, "row_file").map(str::trim).filter(|x| !x.is_empty())) {
        (Some(_), Some(_)) => return Err(bad(CMD_TREND, "give `row` or `row_file`, not both")),
        (Some(r), None) => Some(r.clone()),
        (None, Some(f)) => Some(serde_json::from_str(&read_capped(CMD_TREND, f)?).map_err(|e| bad(CMD_TREND, format!("{f}: {e}")))?),
        (None, None) => None,
    };
    if let Some(r) = &row
        && (Row::of(r).is_none() || r["schema"] != SCHEMA)
    {
        return Err(bad(CMD_TREND, format!("the row isn't a {CMD_MEASURE} row (schema {SCHEMA} with metrics)")));
    }
    // Guard every output before anything is written.
    for key in ["path", "table", "svg"] {
        if let Some(f) = str_param(p, key).map(str::trim).filter(|x| !x.is_empty()) {
            if key == "path" {
                // The trend file is appended to: an existing one is the point, not refused.
                let mut q = p.clone();
                q["force"] = json!(true);
                super::exchange::guard_output(s, CMD_TREND, &q, f)?;
            } else {
                super::exchange::guard_output(s, CMD_TREND, p, f)?;
            }
        }
    }
    let mut appended = false;
    if let Some(r) = &row {
        let mut r = r.clone();
        if let Some(o) = r.as_object_mut() {
            o.remove("path");
        }
        append_row(&path, &r)?;
        appended = true;
    }
    // The counts and the last rows, through the file's index: a call's cost doesn't grow with the
    // project's history (Q6479).
    let read = read_trend(&path, last.max(2))?;
    let rows = &read.last;
    let batches = match str_param(p, "merge_log").map(str::trim).filter(|x| !x.is_empty()) {
        Some(f) => parse_merge_log(&read_capped(CMD_TREND, f)?),
        None => Vec::new(),
    };
    let mut out = json!({"path": path, "rows": read.rows, "appended": appended, "skipped_lines": read.skipped, "rules_source": rules.source, "regressions": 0, "findings": [],
        "index": {"used": read.index_used, "read_bytes": read.read_bytes}});
    let n = rows.len();
    if let (Some(prev), Some(cur)) = (n.checked_sub(2).and_then(|i| rows.get(i)), n.checked_sub(1).and_then(|i| rows.get(i))) {
        let found = compare(rules, prev, cur);
        let sus = suspects(&batches, &prev.commit, &cur.commit, &commits);
        let sus_json: Vec<Value> = sus.iter().map(|b| json!({"batch": b.batch, "next": b.next, "branches": b.branches})).collect();
        let named: Vec<String> = sus.iter().map(|b| format!("{} ({})", b.batch, b.branches.join(", "))).collect();
        let findings: Vec<Value> = found
            .iter()
            .take(MAX_FINDINGS)
            .map(|g| {
                let change = match (g.previous, g.current) {
                    (Some(a), Some(b)) => json!(r3(b - a)),
                    _ => Value::Null,
                };
                let why_rule = rules.rule_for(&g.metric).map(|r| r.why.clone()).unwrap_or_default();
                let mut issue = format!(
                    "{} {}: {} {} -> {} (allowed {}, {} is better) between {} and {}",
                    if g.verdict == "regression" { "REGRESSION" } else { "CHANGE" },
                    g.metric,
                    g.why,
                    fmt(g.previous),
                    fmt(g.current),
                    fmt(Some(r3(g.allowed))),
                    g.better,
                    prev.short(),
                    cur.short()
                );
                if !commits.is_empty() {
                    issue.push_str(&format!(" ({} commits between)", commits.len()));
                }
                if !named.is_empty() {
                    issue.push_str(&format!("; suspects: {}", named.join("; ")));
                }
                json!({
                    "verdict": g.verdict,
                    "metric": g.metric,
                    "previous": g.previous,
                    "current": g.current,
                    "change": change,
                    "allowed": r3(g.allowed),
                    "better": g.better,
                    "why": g.why,
                    "measures": why_rule,
                    "issue": issue,
                    "from": prev.commit,
                    "to": cur.commit,
                    "commits": commits,
                    "suspects": sus_json,
                })
            })
            .collect();
        out["compared"] = json!({"from": prev.commit, "to": cur.commit, "from_at": prev.at, "to_at": cur.at});
        out["regressions"] = json!(found.iter().filter(|g| g.verdict == "regression").count());
        out["changes"] = json!(found.iter().filter(|g| g.verdict == "change").count());
        out["findings"] = json!(findings);
        out["suspects"] = json!(sus_json);
    } else {
        out["note"] = json!("fewer than two rows: nothing to compare yet");
    }
    let shown: Vec<Row> = rows.iter().skip(n.saturating_sub(last)).cloned().collect();
    let t = table(rules, &shown);
    if let Some(f) = str_param(p, "table").map(str::trim).filter(|x| !x.is_empty()) {
        super::exchange::write_file(CMD_TREND, f, t.as_bytes())?;
        out["table_path"] = json!(f);
    }
    if let Some(f) = str_param(p, "svg").map(str::trim).filter(|x| !x.is_empty()) {
        super::exchange::write_file(CMD_TREND, f, svg(rules, &shown).as_bytes())?;
        out["svg_path"] = json!(f);
    }
    out["table"] = json!(t);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(commit: &str, m: &[(&str, f64)]) -> Row {
        Row { commit: commit.into(), at: String::new(), label: String::new(), metrics: m.iter().map(|(k, v)| ((*k).to_string(), *v)).collect() }
    }

    #[test]
    fn the_rules_data_loads_and_every_rule_has_a_direction() {
        let r = rules().unwrap();
        assert!(r.metrics.len() >= 10 && !r.source.is_empty());
        for m in &r.metrics {
            assert!(matches!(m.better.as_str(), "higher" | "lower" | "stable"), "{m:?}");
            assert!(m.abs >= 0.0 && m.share >= 0.0 && !m.why.is_empty(), "{m:?}");
        }
        assert_eq!(r.rule_for("rooms.G-HOST.accuracy_pct").map(|m| m.key.as_str()), Some("rooms.*.accuracy_pct"));
        assert_eq!(r.rule_for("rooms.found").map(|m| m.key.as_str()), Some("rooms.found"));
        assert_eq!(r.rule_for("elements.total").map(|m| m.key.as_str()), Some("elements.total"));
        assert_eq!(r.rule_for("elements.receptacle").map(|m| m.key.as_str()), Some("elements.*"));
        assert!(r.rule_for("rooms.G-HOST.pass").is_none());
    }

    #[test]
    fn gate_a_and_coverage_read_the_prescreen_as_the_gate_a_script_does() {
        let r = json!({
            "elements": 10,
            "checked": {"G-HOST": {"pass": 8, "fail": 2}, "G-HEIGHT": {"pass": 3, "fail": 1, "default": 5}},
            "unmeasured": {"G-HEIGHT": {"receptacle": 4, "switch": 2, "why": "text, not a count"}},
            "coverage": {"kinds_defined": 12, "by_verdict": {"fail": 3, "pass": 5, "not_marked": 4}, "zero_in_model": 2, "unexplained_groups": 7,
                "kinds": [{"name": "MADE-UP DAMPER", "verdict": "fail", "plan_marks": 3, "model": 0}, {"name": "X", "verdict": "pass"}]}
        });
        let findings = [
            json!({"rule": "G-HOST", "verdict": "fail", "handle": "A1", "measured": {"off_face_in": 30.0}, "level": "L1", "at": [5.0, 5.0]}),
            json!({"rule": "G-HOST", "verdict": "fail", "handle": "A2", "measured": {"off_face_in": 6.0}, "level": "L1", "at": [5.0, 5.0]}),
            json!({"rule": "G-HOST", "verdict": "pass", "handle": "A3", "level": "L1", "at": [100.0, 100.0]}),
            json!({"rule": "G-HEIGHT", "verdict": "pass", "handle": "A3", "level": "L1", "at": [100.0, 100.0]}),
            json!({"rule": "G-HOST", "verdict": "pass", "handle": "A4", "level": "L1", "at": [11.5, 5.0]}),
            json!({"rule": "G-LAYOUT", "verdict": "pass", "level": "L1", "at": [50.0, 50.0]}),
            json!({"rule": "G-LAYOUT", "verdict": "pass", "level": "L1", "at": [60.0, 60.0]}),
            json!({"rule": "G-HOST", "verdict": "pass", "handle": "A5", "level": "L9", "at": [100.0, 100.0]}),
            json!({"rule": "G-HOST", "verdict": "pass", "handle": "A6", "level": "L1", "at": "junk"}),
        ];
        let boxes: BTreeMap<String, Vec<[f64; 4]>> = [("L1".to_string(), vec![[0.0, 0.0, 10.0, 10.0]])].into_iter().collect();
        let mut m = BTreeMap::new();
        gate_a(&mut m, &r, &findings, &boxes);
        let g = |k: &str| m.get(k).copied();
        assert_eq!((g("gate_a.host_pass_pct"), g("gate_a.host_fail_far"), g("gate_a.height_pass_pct")), (Some(80.0), Some(1.0), Some(75.0)));
        assert_eq!((g("gate_a.height_measured"), g("gate_a.height_unmeasured"), g("gate_a.elements")), (Some(4.0), Some(6.0), Some(10.0)));
        // Outside every box (+2 ft): A3 once, the two with no handle once together; A4 is within
        // 2 ft, A5's level has no areas, A6 has no point.
        assert_eq!(g("gate_a.outside_building"), Some(2.0), "{m:?}");
        let failing = coverage(&mut m, &r);
        assert_eq!((m.get("coverage.zero_in_model"), m.get("coverage.fail"), m.get("coverage.unsure")), (Some(&2.0), Some(&3.0), Some(&0.0)));
        assert_eq!(failing, ["MADE-UP DAMPER (3 marks, 0 in the model)"]);
        // A prescreen without G-COVERAGE: no coverage metrics.
        let mut m = BTreeMap::new();
        assert!(coverage(&mut m, &json!({})).is_empty() && m.is_empty());
    }

    #[test]
    fn glob_stars_match_any_run() {
        assert!(glob("a.*.b", "a.x.y.b"));
        assert!(glob("takeoff.*", "takeoff.electrical/power.ft"));
        assert!(!glob("a.*.b", "a.x.c"));
        assert!(glob("*", ""));
        assert!(!glob("abc", "ab"));
    }

    #[test]
    fn a_kind_of_element_the_build_stops_making_is_a_regression() {
        // A build that made 145 windows, then one that makes none (the kind's key is gone from
        // its row) or a fifth of them: both fail the gate, naming the kind (Q6369).
        let r = rules().unwrap();
        let a = row("aaaaaaa1", &[("elements.window", 145.0), ("elements.door", 274.0), ("elements.total", 3000.0)]);
        let none = row("bbbbbbb2", &[("elements.door", 274.0), ("elements.total", 2855.0)]);
        let g = compare(r, &a, &none);
        let w = g.iter().find(|x| x.metric == "elements.window").map(|x| (x.verdict, x.why.as_str(), x.current));
        assert_eq!(w, Some(("regression", "not measured now", None)), "{g:?}");
        let few = row("ccccccc3", &[("elements.window", 26.0), ("elements.door", 274.0), ("elements.total", 2881.0)]);
        let g = compare(r, &a, &few);
        let w = g.iter().find(|x| x.metric == "elements.window").map(|x| (x.verdict, x.why.as_str()));
        assert_eq!(w, Some(("regression", "fell")), "{g:?}");
    }

    #[test]
    fn worse_beyond_the_tolerance_is_a_regression_and_within_it_is_not() {
        let r = rules().unwrap();
        let a =
            row("aaaaaaa1", &[("gate_a.host_pass_pct", 94.2), ("gate_a.host_fail_far", 100.0), ("elements.total", 1000.0), ("build.job_s", 1800.0)]);
        // 94.2 -> 92.3 (allowed 0.5) and far 100 -> 104 (allowed 5): only the first; elements
        // 1000 -> 1040 within 5%, the job 1800 -> 2000 within 25%.
        let b =
            row("bbbbbbb2", &[("gate_a.host_pass_pct", 92.3), ("gate_a.host_fail_far", 104.0), ("elements.total", 1040.0), ("build.job_s", 2000.0)]);
        let g = compare(r, &a, &b);
        assert_eq!(g.iter().map(|x| x.metric.as_str()).collect::<Vec<_>>(), ["gate_a.host_pass_pct"], "{g:?}");
        assert_eq!((g[0].previous, g[0].current, g[0].why.as_str()), (Some(94.2), Some(92.3), "fell"));
        // Better is never a regression; a stable count that jumps either way is.
        let c = row("ccccccc3", &[("gate_a.host_pass_pct", 99.0), ("gate_a.host_fail_far", 0.0), ("elements.total", 500.0)]);
        let g = compare(r, &a, &c);
        let names: Vec<&str> = g.iter().map(|x| x.metric.as_str()).collect();
        assert!(names.contains(&"elements.total") && !names.contains(&"build.job_s"), "{names:?}");
        assert_eq!(g.iter().find(|x| x.metric == "elements.total").map(|x| (x.verdict, x.why.as_str())), Some(("regression", "fell")));
        // A stable count that rose (new elements a merge builds): a change, after the regressions.
        let e = row("eeeeeee5", &[("gate_a.host_pass_pct", 90.0), ("gate_a.host_fail_far", 100.0), ("elements.total", 2000.0)]);
        let g = compare(r, &a, &e);
        let got: Vec<(&str, &str)> = g.iter().map(|x| (x.metric.as_str(), x.verdict)).collect();
        assert_eq!(got, [("gate_a.host_pass_pct", "regression"), ("elements.total", "change")], "{g:?}");
        assert!(!names.contains(&"gate_a.host_pass_pct") && !names.contains(&"gate_a.host_fail_far"), "{names:?}");
        assert_eq!(g.iter().find(|x| x.metric == "gate_a.host_fail_far").map(|x| x.why.as_str()), None, "{g:?}");
        // A metric the newest row lacks: "not measured now", unless the caller gives it (the job's
        // time).
        let d = row("ddddddd4", &[("gate_a.host_pass_pct", 94.2), ("elements.total", 1000.0)]);
        let g = compare(r, &a, &d);
        assert_eq!(
            g.iter().map(|x| (x.metric.as_str(), x.why.as_str())).collect::<Vec<_>>(),
            [("gate_a.host_fail_far", "not measured now")],
            "{g:?}"
        );
    }

    #[test]
    fn the_merge_log_names_the_batches_between_two_builds() {
        let log = "2026-01-01T00:00:00Z batch-a start\n\
                   2026-01-01T01:00:00Z batch-a merged alpha(1111111 = a thing (with parens) here) beta -> next 1234567a; FLUSH run; notes\n\
                   pass 1\n\
                   2026-01-01T03:00:00Z batch-b merged gamma -> next 89abcdef; fine\n\
                   2026-01-01T05:00:00Z batch-c merged delta(x) epsilon(y) -> next fedcba98; ok\n\
                   junk -> next zz\n";
        let b = parse_merge_log(log);
        assert_eq!(b.len(), 3, "{b:?}");
        assert_eq!(b[0], Batch { batch: "batch-a".into(), next: "1234567a".into(), branches: vec!["alpha".into(), "beta".into()] });
        assert_eq!(b[2].branches, ["delta", "epsilon"]);
        let s = suspects(&b, "1234567abcdef", "fedcba9876", &[]);
        assert_eq!(s.iter().map(|x| x.batch.as_str()).collect::<Vec<_>>(), ["batch-b", "batch-c"]);
        // Only the newer build in the log: its own batch.
        assert_eq!(suspects(&b, "0000000", "89abcdef", &[]).len(), 1);
        // A commit the caller names adds its batch.
        assert_eq!(suspects(&b, "0000000", "1111111", &["89abcde gamma merge".into()]).len(), 1);
        assert!(parse_merge_log("").is_empty() && parse_merge_log("x merged -> next ").is_empty());
    }

    fn trow(i: usize) -> String {
        json!({"schema": SCHEMA, "commit": format!("c{i:07}"), "label": "made-up", "metrics": {"gate_a.host_pass_pct": i as f64 % 97.0, "elements.total": 1000.0 + i as f64}})
            .to_string()
    }

    fn tdir(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("cadaddy-trendidx-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("trend.jsonl").to_string_lossy().into_owned()
    }

    fn append(path: &str, text: &str) {
        use std::io::Write;
        std::fs::OpenOptions::new().append(true).create(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
    }

    /// The index's answer is the full read's: counts, and the last `need` rows in order.
    fn same_as_full(path: &str, need: usize) -> TrendRead {
        let got = read_trend(path, need).unwrap();
        let need = need.min(INDEX_KEEP);
        let (rows, skipped) = read_all(&std::fs::read_to_string(path).unwrap_or_default());
        assert_eq!((got.rows, got.skipped), (rows.len(), skipped), "{path}");
        let want: Vec<(String, BTreeMap<String, f64>)> =
            rows.iter().skip(rows.len().saturating_sub(need)).map(|r| (r.commit.clone(), r.metrics.clone())).collect();
        let have: Vec<(String, BTreeMap<String, f64>)> = got.last.iter().map(|r| (r.commit.clone(), r.metrics.clone())).collect();
        assert_eq!(have, want, "{path}");
        got
    }

    #[test]
    fn the_index_answers_as_a_full_read_does_through_appends_junk_and_a_line_with_no_newline() {
        let path = tdir("same");
        // No file yet: nothing.
        assert_eq!(same_as_full(&path, 10).rows, 0);
        let mut text = String::new();
        for i in 0..300 {
            text.push_str(&trow(i));
            text.push_str(if i % 50 == 0 { "\r\n" } else { "\n" });
            if i % 37 == 0 {
                text.push_str("not a row\n\n   \n{\"metrics\": 5}\n");
            }
        }
        std::fs::write(&path, &text).unwrap();
        let first = same_as_full(&path, 10);
        assert!(!first.index_used && std::fs::metadata(index_path(&path)).is_ok());
        for need in [1, 2, 10, 200, 500] {
            assert!(same_as_full(&path, need).index_used);
        }
        // Appends, one at a time and several at once, junk among them.
        for i in 300..320 {
            append(&path, &format!("{}\n", trow(i)));
            assert!(same_as_full(&path, 10).index_used);
        }
        append(&path, &format!("junk\n{}\n{}\n", trow(320), trow(321)));
        same_as_full(&path, 200);
        // A last line with no newline counts, and isn't put in the index until it is whole; the
        // next append joins it (one junk line, as a full read sees it).
        append(&path, &trow(322));
        assert_eq!(same_as_full(&path, 5).last.last().map(|r| r.commit.clone()), Some("c0000322".into()));
        append(&path, &format!("{}\n", trow(323)));
        same_as_full(&path, 5);
        append(&path, &format!("{}\n", trow(324)));
        assert!(same_as_full(&path, 5).index_used);
        let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
    }

    #[test]
    fn a_file_changed_outside_cadaddy_is_read_in_full_again() {
        let path = tdir("edited");
        let text: String = (0..400).map(|i| format!("{}\n", trow(i))).collect();
        std::fs::write(&path, &text).unwrap();
        same_as_full(&path, 10);
        // Shorter (rows taken off the end): rebuilt.
        let cut_at = text.match_indices('\n').nth(349).unwrap().0 + 1;
        std::fs::write(&path, &text[..cut_at]).unwrap();
        let r = same_as_full(&path, 10);
        assert!(!r.index_used && r.rows == 350);
        // Replaced by another file, longer: the checked bytes differ, rebuilt.
        let other: String = (1000..1500).map(|i| format!("{}\n", trow(i))).collect();
        std::fs::write(&path, &other).unwrap();
        assert!(!same_as_full(&path, 10).index_used);
        // Edited in place, same length, a new time: rebuilt.
        let edited = other.replacen("c0001001", "c0009001", 1);
        std::fs::write(&path, &edited).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(later).unwrap();
        assert!(!same_as_full(&path, 500).index_used);
        // A row the index points at, past the checked bytes, made junk with the time put back:
        // the row no longer reads, so it is read in full.
        same_as_full(&path, 10);
        let meta_time = std::fs::metadata(&path).unwrap().modified().unwrap();
        let broken = edited.replacen("{\"commit\":\"c0001495\"", "[\"commit\",\"c0001495\"", 1);
        assert_eq!(broken.len(), edited.len());
        assert_ne!(broken, edited, "the test's edit must change the file");
        std::fs::write(&path, &broken).unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(meta_time).unwrap();
        let r = same_as_full(&path, 10);
        assert!(!r.index_used, "a moved row is read again in full");
        // A garbled index is rebuilt; an index for a file that is gone is ignored.
        std::fs::write(index_path(&path), "{not json").unwrap();
        assert!(!same_as_full(&path, 10).index_used);
        assert!(same_as_full(&path, 10).index_used);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(same_as_full(&path, 10).rows, 0);
        let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
    }

    #[test]
    fn a_call_reads_what_was_appended_and_the_rows_it_shows_not_the_history() {
        // Bytes read per call: the same bound at 500 rows and at 20,000 (Q6479).
        let mut most = Vec::new();
        for n in [500usize, 20_000] {
            let path = tdir(&format!("bytes{n}"));
            let text: String = (0..n).map(|i| format!("{}\n", trow(i))).collect();
            std::fs::write(&path, &text).unwrap();
            assert_eq!(read_trend(&path, 10).unwrap().read_bytes, text.len() as u64, "the first call reads it all once");
            let mut top = 0u64;
            for k in 0..5 {
                append(&path, &format!("{}\n", trow(n + k)));
                top = top.max(same_as_full(&path, 10).read_bytes);
            }
            let row_len = trow(n).len() as u64 + 1;
            assert!(top <= 2 * CHECK_BYTES + 11 * row_len, "{n} rows: {top} bytes read");
            most.push(top);
            let _ = std::fs::remove_dir_all(std::path::Path::new(&path).parent().unwrap());
        }
        assert!(most[1] <= most[0] + 64, "{most:?}");
    }

    #[test]
    fn the_table_marks_what_regressed_and_the_plot_is_svg() {
        let r = rules().unwrap();
        let rows = [row("aaaaaaa1", &[("gate_a.host_pass_pct", 94.2)]), row("bbbbbbb2", &[("gate_a.host_pass_pct", 92.3)])];
        let t = table(r, &rows);
        assert!(t.contains("| aaaaaaa1 |") && t.contains("92.3 (!)") && !t.contains("94.2 (!)"), "{t}");
        let s = svg(r, &rows);
        assert!(s.starts_with("<svg") && s.contains("polyline") && s.trim_end().ends_with("</svg>"), "{s}");
        // No rows, junk values: still a picture, never a panic.
        assert!(svg(r, &[]).contains("</svg>"));
        assert!(table(r, &[]).lines().count() == 2);
    }
}
