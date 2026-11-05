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
//!   the metric, both values and both commits, the commits between (`commits`, as the caller's
//!   git gives them) and, with the merger's batch log, the batches and branches merged between
//!   the two builds as the suspects. It returns a table of the last N rows (and writes it, and a
//!   small SVG plot, when asked).
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
/// Rows of a trend file read, at most.
const MAX_ROWS: usize = 100_000;
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
                "{path: .jsonl trend file (one row per line; made when missing, only ever appended to), row?: a validate.measure row (appended first), row_file?: .json file holding one, last?: n (rows in the table, 10, at most 200), merge_log?: the merger's batch log (lines `<time> <batch> merged <branch>(…) … -> next <commit>`), commits?: [text] (the commits between the previous row and the newest, as `git log --oneline` gives them), table?: .md file, svg?: .svg file (a plot of the percentages), force?: bool (replace an existing table or plot)} → {rows, appended, skipped_lines, compared?: {from, to}, regressions, findings: [{metric, previous, current, change, allowed, better, why, issue, from, to, commits, suspects: [{batch, next, branches}]}], table, table_path?, svg_path?}: the newest row against the one before it by the rules data (each metric's better direction and tolerance); a metric worse beyond its tolerance is a REGRESSION naming both values, both commits and the batches merged between them.",
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
    R.get_or_init(|| serde_json::from_str(RULES_JSON).map_err(|e| e.to_string())).as_ref().map_err(|e| bad(CMD_TREND, format!("validate_trend.json: {e}")))
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
        m.insert(k.into(), r3(v));
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
                .map(|(rule, vs)| (rule.clone(), vs.as_object().map(|v| v.iter().take(50).map(|(k, n)| (k.clone(), num(n))).collect()).unwrap_or_default()))
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
            Value::String(_) | Value::Number(_) => {
                text_param(&json!({"r": r}), "r").map(|room| RoomPick { room, level: None })
            }
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
}

/// The newest row against the one before, metric by metric, by the rules: worse beyond the
/// tolerance (`abs`, or `share` of the earlier value, whichever is larger) is a regression; a
/// metric the earlier row had and the newest lacks is one too ("not measured now"). Ranked by
/// how far past the tolerance.
pub fn compare(rules: &Rules, previous: &Row, current: &Row) -> Vec<Regression> {
    let mut out = Vec::new();
    let keys: BTreeSet<&String> = previous.metrics.keys().chain(current.metrics.keys()).collect();
    for k in keys {
        let Some(rule) = rules.rule_for(k) else { continue };
        let (a, b) = (previous.metrics.get(k).copied(), current.metrics.get(k).copied());
        let Some(a) = a else { continue };
        let allowed = rule.abs.max(rule.share * a.abs());
        let Some(b) = b else {
            out.push(Regression { metric: k.clone(), previous: Some(a), current: None, allowed, better: rule.better.clone(), why: "not measured now".into(), over: 1.0 });
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
            out.push(Regression { metric: k.clone(), previous: Some(a), current: Some(b), allowed, better: rule.better.clone(), why: why.into(), over });
        }
    }
    out.sort_by(|x, y| y.over.total_cmp(&x.over).then_with(|| x.metric.cmp(&y.metric)));
    out
}

fn fmt(v: Option<f64>) -> String {
    match v {
        None => "-".into(),
        Some(x) if x == x.trunc() && x.abs() < 1e15 => format!("{x:.0}"),
        Some(x) => format!("{}", r3(x)),
    }
}

/// The last rows as a Markdown table of the rules data's columns; a value that regressed against
/// the row before is marked `(!)`.
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
        let regressed: BTreeSet<String> =
            i.checked_sub(1).and_then(|j| rows.get(j)).map(|p| compare(rules, p, r).into_iter().map(|g| g.metric).collect()).unwrap_or_default();
        t.push_str(&format!("| {} | {} | {} |", r.short(), cut(&r.at, 16), r.label));
        for c in &rules.table {
            let mark = if regressed.contains(&c.key) { " (!)" } else { "" };
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
        out.push_str(&format!("<line x1=\"{}\" y1=\"{ly}\" x2=\"{}\" y2=\"{ly}\" stroke=\"{colour}\" stroke-width=\"2\"/>\n", W - R + 12.0, W - R + 30.0));
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

fn run_trend(s: &mut Session, p: &Value) -> Result<Value> {
    let rules = rules()?;
    let path = str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(CMD_TREND, "`path` names the trend file (.jsonl)"))?.to_string();
    let last = match p.get("last") {
        None | Some(Value::Null) => DEFAULT_LAST,
        Some(v) => v.as_u64().filter(|n| *n >= 1).map(|n| (n as usize).min(MAX_LAST)).ok_or_else(|| bad(CMD_TREND, "`last` is a whole number from 1"))?,
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
    let text = match std::fs::metadata(&path) {
        Ok(_) => read_capped(CMD_TREND, &path)?,
        Err(_) => String::new(),
    };
    let mut skipped = 0usize;
    let mut rows: Vec<Row> = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()).take(MAX_ROWS) {
        match serde_json::from_str::<Value>(line).ok().as_ref().and_then(Row::of) {
            Some(r) => rows.push(r),
            None => skipped += 1,
        }
    }
    let batches = match str_param(p, "merge_log").map(str::trim).filter(|x| !x.is_empty()) {
        Some(f) => parse_merge_log(&read_capped(CMD_TREND, f)?),
        None => Vec::new(),
    };
    let mut out = json!({"path": path, "rows": rows.len(), "appended": appended, "skipped_lines": skipped, "rules_source": rules.source, "regressions": 0, "findings": []});
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
                    "REGRESSION {}: {} {} -> {} (allowed {}, {} is better) between {} and {}",
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
                    "verdict": "regression",
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
        out["regressions"] = json!(found.len());
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
    fn glob_stars_match_any_run() {
        assert!(glob("a.*.b", "a.x.y.b"));
        assert!(glob("takeoff.*", "takeoff.electrical/power.ft"));
        assert!(!glob("a.*.b", "a.x.c"));
        assert!(glob("*", ""));
        assert!(!glob("abc", "ab"));
    }

    #[test]
    fn worse_beyond_the_tolerance_is_a_regression_and_within_it_is_not() {
        let r = rules().unwrap();
        let a = row("aaaaaaa1", &[("gate_a.host_pass_pct", 94.2), ("gate_a.host_fail_far", 100.0), ("elements.total", 1000.0), ("build.job_s", 1800.0)]);
        // 94.2 -> 92.3 (allowed 0.5) and far 100 -> 104 (allowed 5): only the first; elements
        // 1000 -> 1040 within 5%, the job 1800 -> 2000 within 25%.
        let b = row("bbbbbbb2", &[("gate_a.host_pass_pct", 92.3), ("gate_a.host_fail_far", 104.0), ("elements.total", 1040.0), ("build.job_s", 2000.0)]);
        let g = compare(r, &a, &b);
        assert_eq!(g.iter().map(|x| x.metric.as_str()).collect::<Vec<_>>(), ["gate_a.host_pass_pct"], "{g:?}");
        assert_eq!((g[0].previous, g[0].current, g[0].why.as_str()), (Some(94.2), Some(92.3), "fell"));
        // Better is never a regression; a stable count that jumps either way is.
        let c = row("ccccccc3", &[("gate_a.host_pass_pct", 99.0), ("gate_a.host_fail_far", 0.0), ("elements.total", 500.0)]);
        let g = compare(r, &a, &c);
        let names: Vec<&str> = g.iter().map(|x| x.metric.as_str()).collect();
        assert!(names.contains(&"elements.total") && names.contains(&"build.job_s"), "{names:?}");
        assert!(!names.contains(&"gate_a.host_pass_pct") && !names.contains(&"gate_a.host_fail_far"), "{names:?}");
        assert_eq!(g.iter().find(|x| x.metric == "build.job_s").map(|x| x.why.as_str()), Some("not measured now"));
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
