//! Field progress → actual production rates (Q332). The foreman, the crew or an agent marks
//! installed work against the model — by floor, by a share of a floor, by count or by element,
//! per phase (rough, makeup, trim, panel, test) — and timecard hours come in from CSV. Out come
//! percent complete and earned value per phase and floor, installed against planned quantities,
//! phases running over their estimate flagged early, and the **actual production rate per driver**
//! (hours per box, per foot of conduit, per termination, per device…), appended to the company's
//! private field observations. Production rates never go into the drawing: electrical labor stays
//! in the private pack.
//!
//! - `progress.plan {drivers?: [{level, driver, qty}], from_bom?: bool, rates?: {driver: hours}}`:
//!   what is to be installed, per floor and driver (from the BOM, or given), and the book rates.
//! - `progress.mark {phase, level?, fraction?, count?, driver?, elements?, date?, by?, note?}`.
//! - `progress.hours {path | rows}`: timecards (date, name or crew, hours, phase or cost code,
//!   level).
//! - `progress.status {date?, crew_rate?}`: per phase and floor: planned and installed quantities,
//!   budget, earned and actual hours (and dollars at the crew rate), percent complete, CPI, and the
//!   flags.
//! - `progress.rates {date?}`: the actual rate per driver against its book rate.
//! - `progress.observe {path, context?}`: appends the actual rates to a private
//!   `field_observations.csv` (outside the project).
//!
//! Drivers and their phases: boxes and conduit feet (rough), terminations (makeup), devices and
//! luminaires (trim), panels (panel), devices tested (test). Book rates are hours per unit:
//! invented defaults (`basis: assumption`) until the company's production model gives its own.

use std::collections::BTreeMap;

use cadaddy_doc::projrules::RuleDomain;
use cadaddy_walk3d::BuildingSpec;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const MAX_ITEMS: usize = 100_000;

/// The drivers: name, phase, unit, default book rate (hours per unit; invented, an assumption).
pub const DRIVERS: &[(&str, &str, &str, f64)] = &[
    ("boxes", "rough", "each", 0.25),
    ("conduit_ft", "rough", "ft", 0.04),
    ("terminations", "makeup", "each", 0.08),
    ("devices", "trim", "each", 0.2),
    ("luminaires", "trim", "each", 0.5),
    ("panels", "panel", "each", 8.0),
    ("devices_tested", "test", "each", 0.05),
];
pub const PHASES: [&str; 5] = ["rough", "makeup", "trim", "panel", "test"];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub plan: Vec<Planned>,
    pub rates: BTreeMap<String, f64>,
    pub marks: Vec<Mark>,
    pub hours: Vec<TimeEntry>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Planned {
    pub level: String,
    pub driver: String,
    pub qty: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Mark {
    pub phase: String,
    /// The floor (empty: every floor).
    pub level: String,
    /// The share of the phase done on that floor (0–1), or a count of one driver's units.
    pub fraction: Option<f64>,
    pub count: Option<f64>,
    pub driver: String,
    pub elements: Vec<String>,
    pub date: String,
    pub by: String,
    pub note: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TimeEntry {
    pub date: String,
    pub who: String,
    pub hours: f64,
    pub phase: String,
    pub level: String,
}

/// Days since 1970-01-01 of a date: `2026-10-21` (a time after it ignored), `2026/10/21`, or
/// US `10/21/2026`.
pub fn parse_day(text: &str) -> Option<i32> {
    let t = text.trim();
    let t = t.split(['T', ' ']).next().unwrap_or(t);
    let parts: Vec<i32> = t.split(['-', '/', '.']).map(|x| x.trim().parse::<i32>()).collect::<std::result::Result<_, _>>().ok()?;
    let (y, m, d) = match parts[..] {
        [y, m, d] if y > 999 => (y, m, d),
        [m, d, y] if y > 999 => (y, m, d),
        _ => return None,
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1000..=9999).contains(&y) {
        return None;
    }
    let (y2, m2) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let doy = (153 * m2 + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// A day number as an ISO date.
pub fn iso(days: i32) -> String {
    let z = i64::from(days) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn driver(name: &str) -> Option<&'static (&'static str, &'static str, &'static str, f64)> {
    DRIVERS.iter().find(|d| d.0.eq_ignore_ascii_case(name.trim()))
}

fn spec(cmd: &str, s: &Session) -> Result<BuildingSpec> {
    let text = s.doc()?.building_spec.clone().ok_or_else(|| bad(cmd, "the drawing has no building spec (bspec.set)"))?;
    BuildingSpec::from_json(&text).map_err(|e| bad(cmd, e))
}

fn load(sp: &BuildingSpec) -> State {
    serde_json::from_value(sp.progress.clone()).unwrap_or_default()
}

fn save(s: &mut Session, mut sp: BuildingSpec, st: &State) -> Result<()> {
    sp.progress = serde_json::to_value(st).unwrap_or(Value::Null);
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(())
}

/// A phase from a cost code or task text (`EL-ROUGH`, `Trim out L3`, `panel makeup` is panel).
pub fn phase_of(text: &str) -> Option<&'static str> {
    let t = text.to_ascii_lowercase();
    if t.contains("panel") {
        Some("panel")
    } else if t.contains("rough") {
        Some("rough")
    } else if t.contains("makeup") || t.contains("make-up") || t.contains("make up") {
        Some("makeup")
    } else if t.contains("trim") {
        Some("trim")
    } else if t.contains("test") {
        Some("test")
    } else {
        PHASES.iter().find(|p| t == **p).copied()
    }
}

fn book_rate(st: &State, d: &str) -> f64 {
    st.rates.get(d).copied().filter(|v| v.is_finite() && *v > 0.0).or_else(|| driver(d).map(|x| x.3)).unwrap_or(0.0)
}

/// Planned quantities from the bill of materials, per floor.
fn plan_from_bom(s: &mut Session) -> Result<Vec<Planned>> {
    let b = s.execute("bim.bom", &json!({"group_by": "level"})).map_err(|e| bad("progress.plan", format!("the bill of materials: {e}")))?;
    let per_box = s
        .doc()?
        .project_rules
        .resolve(RuleDomain::Custom, "progress.conductors_per_box")
        .and_then(|v| v.value.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0 && *v < 100.0)
        .unwrap_or(3.0);
    let mut q: BTreeMap<(String, &'static str), f64> = BTreeMap::new();
    for l in b["lines"].as_array().into_iter().flatten().take(MAX_ITEMS) {
        let (cat, item, unit) = (
            l["category"].as_str().unwrap_or("").to_ascii_lowercase(),
            l["item"].as_str().unwrap_or("").to_ascii_lowercase(),
            l["unit"].as_str().unwrap_or(""),
        );
        let level = l["level"].as_str().unwrap_or("").to_string();
        let net = l["net"].as_f64().filter(|v| v.is_finite() && *v > 0.0).unwrap_or(0.0);
        let mut add = |d: &'static str, v: f64| *q.entry((level.clone(), d)).or_default() += v;
        if item.contains("box") && unit == "each" && !item.contains("gang box") {
            add("boxes", net);
            add("terminations", net * per_box);
        } else if cat.contains("conduit") && unit == "ft" {
            add("conduit_ft", net);
        } else if cat.contains("luminaire") || cat.contains("lighting") {
            add("luminaires", net);
            add("devices_tested", net);
        } else if cat.contains("device") && !item.contains("plate") && !item.contains("ring") {
            add("devices", net);
            add("devices_tested", net);
        } else if cat == "equipment" && (item.starts_with("panel") || item.starts_with("switchboard")) {
            add("panels", net);
        }
    }
    Ok(q.into_iter().map(|((level, driver), qty)| Planned { level, driver: driver.into(), qty }).collect())
}

fn run_plan(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec("progress.plan", s)?;
    let mut st = load(&sp);
    if bool_or(p, "from_bom", false) {
        st.plan = plan_from_bom(s)?;
    }
    if let Some(a) = p.get("drivers").and_then(Value::as_array) {
        for x in a.iter().take(MAX_ITEMS) {
            let d = x["driver"]
                .as_str()
                .and_then(driver)
                .ok_or_else(|| bad("progress.plan", format!("driver is one of {}", DRIVERS.iter().map(|d| d.0).collect::<Vec<_>>().join(", "))))?;
            let qty =
                x["qty"].as_f64().filter(|v| v.is_finite() && (0.0..1e9).contains(v)).ok_or_else(|| bad("progress.plan", "qty must be 0 or more"))?;
            let level = x["level"].as_str().unwrap_or("").trim().to_string();
            st.plan.retain(|q| !(q.level == level && q.driver == d.0));
            st.plan.push(Planned { level, driver: d.0.into(), qty });
        }
    }
    if let Some(r) = p.get("rates").and_then(Value::as_object) {
        for (k, v) in r.iter().take(100) {
            let d = driver(k).ok_or_else(|| bad("progress.plan", format!("no driver {}", k.chars().take(30).collect::<String>())))?;
            let v = v
                .as_f64()
                .filter(|v| v.is_finite() && *v > 0.0 && *v < 1e4)
                .ok_or_else(|| bad("progress.plan", "a rate is hours per unit, more than 0"))?;
            st.rates.insert(d.0.into(), v);
        }
    }
    let out = json!({"plan": st.plan.iter().map(|q| json!({"level": q.level, "driver": q.driver, "qty": q.qty})).collect::<Vec<_>>()});
    save(s, sp, &st)?;
    Ok(out)
}

fn run_mark(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec("progress.mark", s)?;
    let mut st = load(&sp);
    let phase = str_param(p, "phase").and_then(phase_of).ok_or_else(|| bad("progress.mark", "phase is rough, makeup, trim, panel or test"))?;
    let fraction = p.get("fraction").and_then(Value::as_f64);
    let count = p.get("count").and_then(Value::as_f64);
    let elements: Vec<String> = p
        .get("elements")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).take(MAX_ITEMS).collect())
        .unwrap_or_default();
    let drv = str_param(p, "driver").map(str::trim).unwrap_or_default();
    if let Some(f) = fraction
        && !(f.is_finite() && (0.0..=1.0).contains(&f))
    {
        return Err(bad("progress.mark", "fraction is from 0 to 1"));
    }
    if let Some(c) = count
        && !(c.is_finite() && (0.0..1e9).contains(&c))
    {
        return Err(bad("progress.mark", "count is 0 or more"));
    }
    if (count.is_some() || !elements.is_empty()) && driver(drv).is_none_or(|d| d.1 != phase) {
        return Err(bad("progress.mark", format!("a count or elements need the driver they are of, one of the {phase} phase's")));
    }
    let date = match str_param(p, "date") {
        Some(d) => iso(parse_day(d).ok_or_else(|| bad("progress.mark", "date is a date (YYYY-MM-DD)"))?),
        None => iso(today_of(p)),
    };
    let fraction = if fraction.is_none() && count.is_none() && elements.is_empty() { Some(1.0) } else { fraction };
    let m = Mark {
        phase: phase.into(),
        level: str_param(p, "level").unwrap_or("").trim().to_string(),
        fraction,
        count: count.or_else(|| (!elements.is_empty()).then_some(elements.len() as f64)),
        driver: driver(drv).map(|d| d.0.to_string()).unwrap_or_default(),
        elements,
        date,
        by: str_param(p, "by").unwrap_or("").chars().take(100).collect(),
        note: str_param(p, "note").unwrap_or("").chars().take(500).collect(),
    };
    if st.marks.len() >= MAX_ITEMS {
        return Err(bad("progress.mark", "too many marks"));
    }
    st.marks.push(m);
    let n = st.marks.len();
    save(s, sp, &st)?;
    Ok(json!({"marks": n}))
}

fn today_of(p: &Value) -> i32 {
    str_param(p, "date").or_else(|| str_param(p, "today")).and_then(parse_day).unwrap_or_else(|| {
        let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        i32::try_from(secs / 86_400).unwrap_or(0)
    })
}

fn run_hours(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec("progress.hours", s)?;
    let mut st = load(&sp);
    let rows: Vec<BTreeMap<String, String>> = if let Some(a) = p.get("rows").and_then(Value::as_array) {
        a.iter()
            .filter_map(Value::as_object)
            .take(MAX_ITEMS)
            .map(|o| o.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.as_str().map_or_else(|| v.to_string(), str::to_string))).collect())
            .collect()
    } else if let Some(path) = str_param(p, "path") {
        let bytes = crate::readfile::read(path.trim()).map_err(|e| bad("progress.hours", format!("{path}: {e}")))?;
        super::estimate::csv_rows(&String::from_utf8_lossy(&bytes))
    } else {
        return Err(bad("progress.hours", "path (a timecard CSV) or rows required"));
    };
    let get = |r: &BTreeMap<String, String>, keys: &[&str]| {
        keys.iter().find_map(|k| r.iter().find(|(c, v)| c.contains(k) && !v.trim().is_empty()).map(|(_, v)| v.trim().to_string())).unwrap_or_default()
    };
    let (mut added, mut skipped) = (0usize, Vec::new());
    for (i, r) in rows.iter().enumerate() {
        let hours = get(r, &["hours", "hrs"]).parse::<f64>().ok().filter(|h| h.is_finite() && *h > 0.0 && *h < 1e5);
        let phase = phase_of(&get(r, &["phase", "cost code", "costcode", "task", "activity"]));
        let date = parse_day(&get(r, &["date"]));
        match (hours, phase, date) {
            (Some(h), Some(ph), Some(dt)) if st.hours.len() < MAX_ITEMS => {
                st.hours.push(TimeEntry {
                    date: iso(dt),
                    who: get(r, &["name", "crew", "employee", "worker"]),
                    hours: h,
                    phase: ph.into(),
                    level: get(r, &["level", "floor"]),
                });
                added += 1;
            }
            _ => skipped.push(format!("row {}: needs a date, hours and a phase (rough, makeup, trim, panel, test)", i + 2)),
        }
    }
    if added > 0 {
        save(s, sp, &st)?;
    }
    Ok(json!({"added": added, "skipped": skipped, "total_hours": st.hours.iter().map(|h| h.hours).sum::<f64>()}))
}

/// What was installed of each driver on each floor by `day`.
fn installed(st: &State, day: i32) -> BTreeMap<(String, String), f64> {
    let mut out = BTreeMap::new();
    for q in &st.plan {
        let Some(d) = driver(&q.driver) else { continue };
        let marks = st.marks.iter().filter(|m| m.phase == d.1 && parse_day(&m.date).is_none_or(|x| x <= day));
        let frac =
            marks.clone().filter(|m| m.level.is_empty() || m.level.eq_ignore_ascii_case(&q.level)).filter_map(|m| m.fraction).fold(0.0f64, f64::max);
        let counted: f64 = marks.filter(|m| m.level.eq_ignore_ascii_case(&q.level) && m.driver == d.0).filter_map(|m| m.count).sum();
        let v = (frac * q.qty).max(counted).min(q.qty);
        out.insert((q.level.clone(), d.0.to_string()), v);
    }
    out
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

struct PhaseRow {
    phase: &'static str,
    level: String,
    budget: f64,
    earned: f64,
    actual: f64,
    drivers: Vec<Value>,
}

fn rows(st: &State, day: i32) -> Vec<PhaseRow> {
    let inst = installed(st, day);
    let mut out = Vec::new();
    let mut levels: Vec<String> = st.plan.iter().map(|q| q.level.clone()).collect();
    levels.sort();
    levels.dedup();
    for ph in PHASES {
        for lv in &levels {
            let mut r = PhaseRow { phase: ph, level: lv.clone(), budget: 0.0, earned: 0.0, actual: 0.0, drivers: Vec::new() };
            for q in st.plan.iter().filter(|q| &q.level == lv && driver(&q.driver).is_some_and(|d| d.1 == ph)) {
                let rate = book_rate(st, &q.driver);
                let done = inst.get(&(lv.clone(), q.driver.clone())).copied().unwrap_or(0.0);
                r.budget += q.qty * rate;
                r.earned += done * rate;
                r.drivers.push(json!({"driver": q.driver, "unit": driver(&q.driver).map_or("", |d| d.2), "planned": r2(q.qty), "installed": r2(done), "book_rate": rate}));
            }
            r.actual = st
                .hours
                .iter()
                .filter(|h| h.phase == ph && h.level.eq_ignore_ascii_case(lv) && parse_day(&h.date).is_none_or(|x| x <= day))
                .map(|h| h.hours)
                .sum();
            if r.budget > 0.0 || r.actual > 0.0 {
                out.push(r);
            }
        }
    }
    out
}

/// Hours on a phase with no floor on the timecard.
fn unplaced_hours(st: &State, phase: &str, day: i32) -> f64 {
    st.hours.iter().filter(|h| h.phase == phase && h.level.trim().is_empty() && parse_day(&h.date).is_none_or(|x| x <= day)).map(|h| h.hours).sum()
}

fn crew_rate(sp: &BuildingSpec, p: &Value) -> Option<f64> {
    p.get("crew_rate").and_then(Value::as_f64).or_else(|| sp.estimate["settings"]["crew_rate"].as_f64()).filter(|v| v.is_finite() && *v > 0.0)
}

fn tolerance(d: &cadaddy_doc::Drawing) -> (f64, f64) {
    let get = |k: &str, def: f64| {
        d.project_rules
            .resolve(RuleDomain::Custom, k)
            .and_then(|v| v.value.trim().trim_end_matches('%').parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(def)
    };
    (get("progress.over_pct", 10.0) / 100.0, get("progress.early_pct", 5.0) / 100.0)
}

fn run_status(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec("progress.status", s)?;
    let st = load(&sp);
    if st.plan.is_empty() {
        return Err(bad("progress.status", "no plan yet (progress.plan)"));
    }
    let day = today_of(p);
    let rate = crew_rate(&sp, p);
    let (over, early) = tolerance(s.doc()?);
    let money = |h: f64| rate.map(|r| r2(h * r));
    let rs = rows(&st, day);
    let mut phases = Vec::new();
    let mut flags = Vec::new();
    for ph in PHASES {
        let these: Vec<&PhaseRow> = rs.iter().filter(|r| r.phase == ph).collect();
        let (budget, earned, placed): (f64, f64, f64) = these.iter().fold((0.0, 0.0, 0.0), |a, r| (a.0 + r.budget, a.1 + r.earned, a.2 + r.actual));
        let actual = placed + unplaced_hours(&st, ph, day);
        if budget <= 0.0 && actual <= 0.0 {
            continue;
        }
        let pct = if budget > 0.0 { earned / budget } else { 0.0 };
        let cpi = (actual > 0.0).then(|| earned / actual);
        let projected = (pct > 0.0).then(|| actual / pct);
        if let Some(eac) = projected
            && pct >= early
            && pct < 1.0
            && eac > budget * (1.0 + over)
        {
            flags.push(format!(
                "{ph} is running over: {:.0} h projected against {:.0} h budgeted at {:.0}% complete ({:.0} h spent for {:.0} h earned)",
                eac,
                budget,
                pct * 100.0,
                actual,
                earned
            ));
        }
        phases.push(json!({
            "phase": ph, "budget_hours": r2(budget), "earned_hours": r2(earned), "actual_hours": r2(actual), "percent_complete": r2(pct * 100.0),
            "cpi": cpi.map(r2), "projected_hours": projected.map(r2), "budget_cost": money(budget), "earned_value": money(earned), "actual_cost": money(actual),
            "floors": these.iter().map(|r| json!({"level": r.level, "budget_hours": r2(r.budget), "earned_hours": r2(r.earned), "actual_hours": r2(r.actual), "percent_complete": if r.budget > 0.0 { r2(r.earned / r.budget * 100.0) } else { 0.0 }, "drivers": r.drivers})).collect::<Vec<_>>(),
        }));
    }
    let (b, e, a) = rs.iter().fold((0.0, 0.0, 0.0), |acc, r| (acc.0 + r.budget, acc.1 + r.earned, acc.2 + r.actual));
    let a = a + PHASES.iter().map(|ph| unplaced_hours(&st, ph, day)).sum::<f64>();
    Ok(json!({
        "date": iso(day), "crew_rate": rate, "phases": phases, "flags": flags,
        "total": {"budget_hours": r2(b), "earned_hours": r2(e), "actual_hours": r2(a), "percent_complete": if b > 0.0 { r2(e / b * 100.0) } else { 0.0 }, "earned_value": money(e), "actual_cost": money(a)},
    }))
}

/// The actual rate per driver: a phase's hours shared across its drivers by their earned hours,
/// over the units installed.
fn actual_rates(st: &State, day: i32) -> Vec<Value> {
    let inst = installed(st, day);
    let mut out = Vec::new();
    for ph in PHASES {
        let hours: f64 = st.hours.iter().filter(|h| h.phase == ph && parse_day(&h.date).is_none_or(|x| x <= day)).map(|h| h.hours).sum();
        let mut done: BTreeMap<String, f64> = BTreeMap::new();
        for ((_, d), v) in &inst {
            if driver(d).is_some_and(|x| x.1 == ph) {
                *done.entry(d.clone()).or_default() += v;
            }
        }
        let earned_total: f64 = done.iter().map(|(d, v)| v * book_rate(st, d)).sum();
        if hours <= 0.0 || earned_total <= 0.0 {
            continue;
        }
        for (d, units) in done.iter().filter(|(_, v)| **v > 0.0) {
            let share = units * book_rate(st, d) / earned_total;
            let h = hours * share;
            out.push(json!({
                "phase": ph, "driver": d, "unit": driver(d).map_or("", |x| x.2), "installed": r2(*units), "hours": r2(h),
                "actual_rate": (h / units * 10_000.0).round() / 10_000.0, "book_rate": book_rate(st, d), "ratio": r2(h / units / book_rate(st, d)),
            }));
        }
    }
    out
}

fn run_rates(s: &mut Session, p: &Value) -> Result<Value> {
    let st = load(&spec("progress.rates", s)?);
    Ok(
        json!({"rates": actual_rates(&st, today_of(p)), "note": "hours per unit; kept out of the drawing — progress.observe adds them to the private field observations"}),
    )
}

fn run_observe(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path")
        .map(str::trim)
        .filter(|t| t.to_ascii_lowercase().ends_with(".csv"))
        .ok_or_else(|| bad("progress.observe", "path required: the private field_observations.csv"))?
        .to_string();
    let st = load(&spec("progress.observe", s)?);
    let day = today_of(p);
    let rates = actual_rates(&st, day);
    if rates.is_empty() {
        return Err(bad("progress.observe", "no actual rates yet (installed work and hours on the same phase)"));
    }
    let ctx = |k: &str| p["context"][k].as_str().unwrap_or("").replace([',', '\n', '"'], " ");
    let job = s.state().map(|x| x.title.clone()).unwrap_or_default().replace([',', '\n', '"'], " ");
    let header = "date,job,phase,driver,unit,installed,hours,rate_hours_per_unit,book_rate,building_type,wiring_method,working_height,basis\n";
    let mut text = String::new();
    #[cfg(not(target_arch = "wasm32"))]
    let existing = std::fs::read_to_string(&path).ok();
    #[cfg(target_arch = "wasm32")]
    let existing: Option<String> = None;
    match &existing {
        Some(e) if !e.is_empty() => text.push_str(e),
        _ => text.push_str(header),
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
    for r in &rates {
        text += &format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},observed\n",
            iso(day),
            job,
            r["phase"].as_str().unwrap_or(""),
            r["driver"].as_str().unwrap_or(""),
            r["unit"].as_str().unwrap_or(""),
            r["installed"],
            r["hours"],
            r["actual_rate"],
            r["book_rate"],
            ctx("building_type"),
            ctx("wiring_method"),
            ctx("working_height")
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    crate::writefile::write(&path, text.as_bytes()).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
    Ok(json!({"path": path, "appended": rates.len()}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("progress.plan", "Progress Plan", run_plan)
            .params("{drivers?: [{level, driver: boxes|conduit_ft|terminations|devices|luminaires|panels|devices_tested, qty}], from_bom?: bool, rates?: {driver: hours per unit}} → {plan}"),
        CommandSpec::new("progress.mark", "Mark Installed Work", run_mark)
            .params("{phase: rough|makeup|trim|panel|test, level?: floor (empty: every floor), fraction?: 0–1 (default 1: complete), count?, driver?, elements?: [id], date?, by?, note?} → {marks}"),
        CommandSpec::new("progress.hours", "Import Timecards", run_hours).params("{path: CSV (date, name/crew, hours, phase or cost code, level) | rows} → {added, skipped, total_hours}"),
        CommandSpec::new("progress.status", "Progress and Earned Value", run_status)
            .params("{date?, crew_rate?} → {phases: [{phase, budget_hours, earned_hours, actual_hours, percent_complete, cpi, projected_hours, earned_value, actual_cost, floors}], total, flags}")
            .noundo(),
        CommandSpec::new("progress.rates", "Actual Production Rates", run_rates).params("{date?} → {rates: [{phase, driver, unit, installed, hours, actual_rate, book_rate, ratio}]}").noundo(),
        CommandSpec::new("progress.observe", "Add Field Observations", run_observe)
            .params("{path: the private field_observations.csv, context?: {building_type, wiring_method, working_height}, date?} → {path, appended}")
            .noundo(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(parse_day("1970-01-01"), Some(0));
        assert_eq!(iso(parse_day("2026-10-21").unwrap()), "2026-10-21");
        assert_eq!(parse_day("10/21/2026"), parse_day("2026-10-21"));
        assert_eq!(parse_day("2026-10-21T08:00:00"), parse_day("2026-10-21"));
        assert_eq!(parse_day("2026-13-01"), None);
        assert_eq!(iso(parse_day("2024-02-29").unwrap() + 1), "2024-03-01");
    }

    #[test]
    fn phases_from_cost_codes() {
        assert_eq!(phase_of("EL-ROUGH"), Some("rough"));
        assert_eq!(phase_of("Panel makeup"), Some("panel"));
        assert_eq!(phase_of("box make-up L3"), Some("makeup"));
        assert_eq!(phase_of("Trim out"), Some("trim"));
        assert_eq!(phase_of("lunch"), None);
    }
}
