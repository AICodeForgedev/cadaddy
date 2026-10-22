//! Procurement from the model (Q333): material releases from the bill of materials (`bim.bom`),
//! the submittal log, lead times per item class, "order by" dates from the 4D schedule, order
//! status, and items that must arrive with something else.
//!
//! - `procure.release {name, level?, trades?, categories?, need_by?, allow_unlinked?}`: a release
//!   of what the BOM still needs (by floor, area or trade): the exact need of each line not yet
//!   released, plus spares by rule. Every release is taken from what is left, so the releases sum
//!   to the BOM. A release that takes an item without what must arrive with it (lugs and bolt
//!   kits with their gear, covers with panels, breakers with load centers) is refused, or, with
//!   `allow_unlinked`, made and flagged.
//! - `procure.list {today?}`: the releases, each line's order-by date and its flags (late, not
//!   approved, unlinked), and what of the BOM no release has taken yet.
//! - `procure.status {release, item, status, received?}`: ordered, shipped, received, short.
//! - `procure.submittal {item, spec_section?, status?, submitted?, returned?, reviewer?, comment?}`
//!   and `procure.submittals`: the submittal log.
//! - `procure.leadtimes {set?}`: lead times per item class (company data; defaults below).
//! - `procure.export {release, path}`: a release as XLSX, CSV or PDF.
//!
//! Order by = the date it is needed (the release's `need_by`, else the 4D schedule's first task on
//! its floor) − the longest lead time − the submittal review allowance while it isn't approved
//! (`custom/procure.review_days`, default 14). Spares: `custom/spares.<category>` percent.

use std::collections::BTreeMap;

use cadaddy_doc::projrules::RuleDomain;
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::fourd;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const MAX_RELEASES: usize = 1_000;
const MAX_LOG: usize = 10_000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Procurement {
    pub releases: Vec<Release>,
    pub submittals: Vec<Submittal>,
    pub lead_times: Vec<LeadTime>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Release {
    pub id: String,
    pub name: String,
    pub level: String,
    /// The 4D task(s) it was released for (`phase`), when it was.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub phase: String,
    pub need_by: String,
    pub created: String,
    pub lines: Vec<ReleaseLine>,
    /// Linked items released apart (allowed by hand): what ships without what.
    pub unlinked: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReleaseLine {
    /// The BOM line it takes from: trade|category|item|size.
    pub key: String,
    pub trade: String,
    pub category: String,
    pub item: String,
    pub size: String,
    pub unit: String,
    pub level: String,
    /// The exact need, the spares on top, and the gear tag it belongs to (lug kits).
    pub need: f64,
    pub spares: f64,
    pub status: String,
    pub received: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Submittal {
    pub item: String,
    pub spec_section: String,
    /// `pending`, `submitted`, `approved`, `approved_as_noted`, `revise_resubmit`, `rejected`.
    pub status: String,
    pub submitted: String,
    pub returned: String,
    pub reviewer: String,
    pub comments: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LeadTime {
    pub class: String,
    pub weeks_min: f64,
    pub weeks_max: f64,
}

/// Default lead times, weeks (placeholders for the company's own data).
const LEAD_DEFAULTS: &[(&str, f64, f64)] = &[
    ("gear", 30.0, 60.0),
    ("transformers", 20.0, 40.0),
    ("panelboards", 8.0, 16.0),
    ("disconnects", 4.0, 8.0),
    ("lug kits", 4.0, 8.0),
    ("equipment", 12.0, 24.0),
    ("lighting", 6.0, 12.0),
    ("devices", 1.0, 2.0),
    ("wire", 1.0, 3.0),
    ("conduit", 1.0, 2.0),
    ("ductwork", 2.0, 4.0),
    ("piping", 1.0, 3.0),
    ("drywall", 1.0, 2.0),
    ("other", 2.0, 4.0),
];

/// An item's class for lead times.
pub fn class_of(category: &str, item: &str) -> &'static str {
    let t = format!("{} {}", category.to_ascii_lowercase(), item.to_ascii_lowercase());
    let has = |w: &str| t.contains(w);
    if has("lug kit") {
        "lug kits"
    } else if has("switchboard") || has("switchgear") {
        "gear"
    } else if has("transformer") {
        "transformers"
    } else if has("panel") && !has("cover") {
        "panelboards"
    } else if has("disconnect") {
        "disconnects"
    } else if has("luminaire") || has("light") || has("fixture") {
        "lighting"
    } else if has("device") || has("receptacle") || has("switch") || has("plate") || has("box") || has("ring") {
        "devices"
    } else if has("wire") || has("conductor") || has("cable") {
        "wire"
    } else if has("conduit") || has("coupling") || has("connector") || has("strap") || has("elbow") || has("bushing") {
        "conduit"
    } else if has("duct") {
        "ductwork"
    } else if has("pipe") || has("fitting") || has("hanger") {
        "piping"
    } else if has("drywall") || has("stud") || has("track") || has("board") {
        "drywall"
    } else if has("equipment") {
        "equipment"
    } else {
        "other"
    }
}

fn lead(p: &Procurement, class: &str) -> (f64, f64) {
    p.lead_times
        .iter()
        .find(|l| l.class.eq_ignore_ascii_case(class))
        .map(|l| (l.weeks_min, l.weeks_max))
        .or_else(|| LEAD_DEFAULTS.iter().find(|(c, _, _)| *c == class).map(|(_, a, b)| (*a, *b)))
        .unwrap_or((2.0, 4.0))
}

fn load(sp: &BuildingSpec) -> Procurement {
    sp.procurement.as_ref().and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

fn save(s: &mut Session, mut sp: BuildingSpec, p: &Procurement) -> Result<()> {
    sp.procurement = serde_json::to_value(p).ok();
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(())
}

fn spec(cmd: &str, s: &Session) -> Result<BuildingSpec> {
    let text = s.doc()?.building_spec.clone().ok_or_else(|| bad(cmd, "the drawing has no building spec (bspec.set)"))?;
    BuildingSpec::from_json(&text).map_err(|e| bad(cmd, e))
}

/// The BOM by level: (key, line JSON).
fn bom(cmd: &str, s: &mut Session) -> Result<Vec<(String, Value)>> {
    let b = s.execute("bim.bom", &json!({"group_by": "level"})).map_err(|e| bad(cmd, format!("the bill of materials: {e}")))?;
    Ok(b["lines"].as_array().cloned().unwrap_or_default().into_iter().map(|l| (key(&l), l)).collect())
}

fn st(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_string()
}

fn key(l: &Value) -> String {
    format!("{}|{}|{}|{}|{}", st(l, "trade"), st(l, "category"), st(l, "item"), st(l, "size"), st(l, "level"))
}

/// What has to arrive together: the group a BOM line belongs to (lug kits with their gear,
/// covers and breakers with their panel), by the gear's tag.
fn group_of(l: &Value) -> Option<String> {
    let item = st(l, "item");
    // A lug kit names its gear: `Lug kit TX-1 secondary: …`.
    if let Some(rest) = item.strip_prefix("Lug kit ") {
        return rest.split_whitespace().next().map(|t| t.trim_end_matches(':').to_string());
    }
    for acc in ["Panel cover/trim ", "Breaker "] {
        if let Some(rest) = item.strip_prefix(acc) {
            return rest.split_whitespace().next().map(str::to_string);
        }
    }
    // Gear: `transformer TX-1`, `panel P1` (the BOM's equipment lines).
    if st(l, "category") == "equipment" {
        let lower = item.to_ascii_lowercase();
        if ["switchboard", "switchgear", "panel", "transformer", "disconnect", "load center"].iter().any(|k| lower.starts_with(k)) {
            return item.split_whitespace().last().map(str::to_string);
        }
    }
    None
}

fn spares_pct(d: &cadaddy_doc::Drawing, category: &str) -> f64 {
    d.project_rules
        .resolve(RuleDomain::Custom, &format!("spares.{category}"))
        .and_then(|v| v.value.trim().trim_end_matches('%').parse::<f64>().ok())
        .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
        .unwrap_or(match category {
            "devices" => 2.0,
            "luminaires" | "lighting" => 1.0,
            _ => 0.0,
        })
}

fn today(p: &Value) -> i32 {
    str_param(p, "today").and_then(fourd::day).unwrap_or_else(|| {
        let secs = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        i32::try_from(secs / 86_400).unwrap_or(0)
    })
}

/// The day a release's material is needed: its `need_by`, else the first 4D task on its floor
/// (any floor for a job-wide release).
fn need_day(d: &cadaddy_doc::Drawing, r: &Release) -> Option<i32> {
    if let Some(day) = fourd::day(&r.need_by) {
        return Some(day);
    }
    let sched = fourd::Schedule4d::from_json(d.schedule4d.as_deref()).ok()?;
    sched
        .tasks
        .iter()
        .filter(|t| r.level.is_empty() || t.filter.level.as_deref().is_none_or(|l| fourd::wildcard(l, &r.level)))
        .filter_map(|t| fourd::day(&t.start))
        .min()
}

fn run_release(s: &mut Session, p: &Value) -> Result<Value> {
    let name = str_param(p, "name").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("procure.release", "name required"))?.to_string();
    let level = str_param(p, "level").map(str::trim).unwrap_or_default().to_string();
    let list = |k: &str| -> Vec<String> {
        p.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_ascii_lowercase).collect()).unwrap_or_default()
    };
    let (trades, cats) = (list("trades"), list("categories"));
    // A phase: the 4D tasks named (wildcards), their floors and systems, their start the need date.
    let phase = str_param(p, "phase").map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    let mut tasks: Vec<fourd::Task> = Vec::new();
    if let Some(ph) = &phase {
        let sched = fourd::Schedule4d::from_json(s.doc()?.schedule4d.as_deref()).map_err(|e| bad("procure.release", e))?;
        if sched.tasks.is_empty() {
            return Err(bad("procure.release", "there is no 4D schedule to take a phase from (fourd.import)"));
        }
        tasks = sched.tasks.iter().filter(|t| fourd::wildcard(ph, &t.name)).cloned().collect();
        if tasks.is_empty() {
            let names: Vec<&str> = sched.tasks.iter().map(|t| t.name.as_str()).take(50).collect();
            return Err(bad("procure.release", format!("no 4D task `{ph}`: the tasks are {}", names.join(", "))));
        }
    }
    let lines = bom("procure.release", s)?;
    let sp = spec("procure.release", s)?;
    let mut proc = load(&sp);
    if proc.releases.len() >= MAX_RELEASES {
        return Err(bad("procure.release", "too many releases"));
    }
    if proc.releases.iter().any(|r| r.name.eq_ignore_ascii_case(&name)) {
        return Err(bad("procure.release", format!("there is already a release named {name}")));
    }
    // What earlier releases took, per BOM line.
    let mut taken: BTreeMap<String, f64> = BTreeMap::new();
    for r in &proc.releases {
        for l in &r.lines {
            *taken.entry(l.key.clone()).or_default() += l.need;
        }
    }
    let d = s.doc()?;
    // A line a phase's task covers: on its floor (its level filter), and of its system or trade
    // where it names one (matched against the line's trade, category or item).
    let in_task = |t: &fourd::Task, l: &Value| {
        t.filter.level.as_deref().is_none_or(|lv| fourd::wildcard(lv, &st(l, "level")))
            && t.filter.system.as_deref().is_none_or(|sys| ["trade", "category", "item"].iter().any(|k| fourd::wildcard(sys, &st(l, k))))
    };
    let wanted = |l: &Value| {
        (level.is_empty() || st(l, "level").eq_ignore_ascii_case(&level))
            && (trades.is_empty() || trades.contains(&st(l, "trade").to_ascii_lowercase()))
            && (cats.is_empty() || cats.contains(&st(l, "category").to_ascii_lowercase()))
            && (phase.is_none() || tasks.iter().any(|t| in_task(t, l)))
    };
    let mut out_lines = Vec::new();
    for (k, l) in &lines {
        if !wanted(l) {
            continue;
        }
        let left = l["net"].as_f64().unwrap_or(0.0) - taken.get(k).copied().unwrap_or(0.0);
        if left <= 1e-9 {
            continue;
        }
        let cat = st(l, "category");
        let unit = st(l, "unit");
        let pct = spares_pct(d, &cat);
        let spares =
            if pct > 0.0 { if unit == "each" { (left * pct / 100.0).ceil() } else { (left * pct / 100.0 * 10.0).round() / 10.0 } } else { 0.0 };
        out_lines.push(ReleaseLine {
            key: k.clone(),
            trade: st(l, "trade"),
            category: cat,
            item: st(l, "item"),
            size: st(l, "size"),
            unit,
            level: st(l, "level"),
            need: left,
            spares,
            status: "planned".into(),
            received: 0.0,
        });
    }
    if out_lines.is_empty() {
        return Err(bad("procure.release", "nothing left to release for that floor, trade, category or phase"));
    }
    // What must arrive together: every line of a group in this release, or none.
    let mut unlinked = Vec::new();
    let mut groups: BTreeMap<String, (Vec<String>, Vec<String>)> = BTreeMap::new();
    for (k, l) in &lines {
        let Some(g) = group_of(l) else { continue };
        let left = l["net"].as_f64().unwrap_or(0.0) - taken.get(k).copied().unwrap_or(0.0);
        if left <= 1e-9 {
            continue;
        }
        let e = groups.entry(g).or_default();
        if out_lines.iter().any(|x| &x.key == k) {
            e.0.push(st(l, "item"));
        } else {
            e.1.push(st(l, "item"));
        }
    }
    for (g, (inside, outside)) in &groups {
        if !inside.is_empty() && !outside.is_empty() {
            unlinked.push(format!("{} is released without {} (they arrive together: {g})", inside.join(", "), outside.join(", ")));
        }
    }
    if !unlinked.is_empty() && !bool_or(p, "allow_unlinked", false) {
        return Err(bad(
            "procure.release",
            format!("linked items would be released apart: {} (release them together, or allow_unlinked)", unlinked.join("; ")),
        ));
    }
    let id = format!("R{:03}", proc.releases.len() + 1);
    let created = fourd::iso(today(p));
    let r = Release {
        id: id.clone(),
        name,
        level,
        phase: phase.unwrap_or_default(),
        // Given, else the phase's (its first task's) start.
        need_by: str_param(p, "need_by")
            .map(str::to_string)
            .or_else(|| tasks.iter().filter_map(|t| fourd::day(&t.start)).min().map(fourd::iso))
            .unwrap_or_default(),
        created,
        lines: out_lines,
        unlinked,
    };
    let summary = json!({"id": id, "lines": r.lines.len(), "unlinked": r.unlinked});
    proc.releases.push(r);
    save(s, sp, &proc)?;
    Ok(summary)
}

fn submittal_for<'a>(proc: &'a Procurement, item: &str, class: &str) -> Option<&'a Submittal> {
    proc.submittals.iter().find(|x| {
        let k = x.item.trim().to_ascii_lowercase();
        !k.is_empty() && (item.to_ascii_lowercase().contains(&k) || class.eq_ignore_ascii_case(&k))
    })
}

fn line_json(d: &cadaddy_doc::Drawing, proc: &Procurement, r: &Release, l: &ReleaseLine, today: i32) -> Value {
    let class = class_of(&l.category, &l.item);
    let (lmin, lmax) = lead(proc, class);
    let sub = submittal_for(proc, &l.item, class);
    let approved = sub.is_some_and(|x| x.status.starts_with("approved"));
    let review_days = d
        .project_rules
        .resolve(RuleDomain::Custom, "procure.review_days")
        .and_then(|v| v.value.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && (0.0..=365.0).contains(v))
        .unwrap_or(14.0);
    let need = need_day(d, r);
    let order_by = need.map(|n| n - (lmax * 7.0).round() as i32 - if approved { 0 } else { review_days.round() as i32 });
    let mut flags = Vec::new();
    if let Some(o) = order_by
        && o < today
        && l.status == "planned"
    {
        flags.push(format!(
            "late: order by {} (lead time {lmin}–{lmax} weeks{}), today is {}",
            fourd::iso(o),
            if approved { "" } else { " plus the submittal review" },
            fourd::iso(today)
        ));
    }
    if !approved && matches!(class, "gear" | "transformers" | "panelboards" | "lighting" | "equipment" | "lug kits") {
        flags.push(format!("submittal not approved{}", sub.map(|x| format!(" ({})", x.status)).unwrap_or_else(|| " (not in the log)".into())));
    }
    if l.status == "short" {
        flags.push(format!("short: {} of {} received", l.received, l.need + l.spares));
    }
    json!({
        "key": l.key, "trade": l.trade, "category": l.category, "item": l.item, "size": l.size, "unit": l.unit, "level": l.level,
        "need": l.need, "spares": l.spares, "qty": l.need + l.spares, "status": l.status, "received": l.received,
        "class": class, "lead_weeks": [lmin, lmax], "need_date": need.map(fourd::iso), "order_by": order_by.map(fourd::iso),
        "submittal": sub.map(|x| x.status.clone()), "flags": flags,
    })
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let lines = bom("procure.list", s)?;
    let sp = spec("procure.list", s)?;
    let proc = load(&sp);
    let d = s.doc()?;
    let t = today(p);
    let releases: Vec<Value> = proc
        .releases
        .iter()
        .map(|r| {
            let ls: Vec<Value> = r.lines.iter().map(|l| line_json(d, &proc, r, l, t)).collect();
            let late = ls.iter().filter(|l| l["flags"].as_array().is_some_and(|f| f.iter().any(|x| x.as_str().is_some_and(|x| x.starts_with("late"))))).count();
            json!({"id": r.id, "name": r.name, "level": r.level, "phase": r.phase, "need_by": r.need_by, "created": r.created, "lines": ls, "late": late, "unlinked": r.unlinked})
        })
        .collect();
    // What no release has taken yet; do the releases sum to the BOM?
    let mut taken: BTreeMap<String, f64> = BTreeMap::new();
    for r in &proc.releases {
        for l in &r.lines {
            *taken.entry(l.key.clone()).or_default() += l.need;
        }
    }
    let unreleased: Vec<Value> = lines
        .iter()
        .filter_map(|(k, l)| {
            let left = l["net"].as_f64().unwrap_or(0.0) - taken.get(k).copied().unwrap_or(0.0);
            (left > 1e-9).then(
                || json!({"key": k, "item": st(l, "item"), "level": st(l, "level"), "unit": st(l, "unit"), "left": (left * 1000.0).round() / 1000.0}),
            )
        })
        .collect();
    let over: Vec<String> = taken
        .iter()
        .filter(|(k, v)| lines.iter().find(|(x, _)| x == *k).is_none_or(|(_, l)| **v > l["net"].as_f64().unwrap_or(0.0) + 1e-6))
        .map(|(k, _)| k.clone())
        .collect();
    Ok(json!({"releases": releases, "unreleased": unreleased, "sums_to_bom": unreleased.is_empty() && over.is_empty(), "over_released": over}))
}

fn run_status(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec("procure.status", s)?;
    let mut proc = load(&sp);
    let rel =
        str_param(p, "release").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("procure.status", "release required (its id or name)"))?;
    let item = str_param(p, "item")
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| bad("procure.status", "item required"))?;
    let status = str_param(p, "status").map(|t| t.trim().to_ascii_lowercase()).unwrap_or_default();
    if !["planned", "ordered", "shipped", "received", "short"].contains(&status.as_str()) {
        return Err(bad("procure.status", "status is planned, ordered, shipped, received or short"));
    }
    let r = proc
        .releases
        .iter_mut()
        .find(|r| r.id.eq_ignore_ascii_case(rel) || r.name.eq_ignore_ascii_case(rel))
        .ok_or_else(|| bad("procure.status", format!("no release {}", rel.chars().take(40).collect::<String>())))?;
    let mut n = 0;
    for l in r.lines.iter_mut().filter(|l| l.item.to_ascii_lowercase().contains(&item) || l.key.to_ascii_lowercase() == item) {
        l.status = status.clone();
        l.received = match (status.as_str(), p.get("received").and_then(Value::as_f64)) {
            ("received", None) => l.need + l.spares,
            (_, Some(v)) if v.is_finite() && v >= 0.0 => v.min(1e9),
            _ => l.received,
        };
        n += 1;
    }
    if n == 0 {
        return Err(bad("procure.status", "no line of that release matches the item"));
    }
    save(s, sp, &proc)?;
    Ok(json!({"changed": n}))
}

fn run_submittal(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec("procure.submittal", s)?;
    let mut proc = load(&sp);
    let item = str_param(p, "item")
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| bad("procure.submittal", "item required (an item class or the item's text)"))?
        .to_string();
    let t = |k: &str| str_param(p, k).map(|v| v.trim().chars().take(500).collect::<String>());
    if let Some(st) = t("status")
        && !["pending", "submitted", "approved", "approved_as_noted", "revise_resubmit", "rejected"].contains(&st.as_str())
    {
        return Err(bad("procure.submittal", "status is pending, submitted, approved, approved_as_noted, revise_resubmit or rejected"));
    }
    let i = match proc.submittals.iter().position(|x| x.item.eq_ignore_ascii_case(&item)) {
        Some(i) => i,
        None if proc.submittals.len() < MAX_LOG => {
            proc.submittals.push(Submittal { item: item.clone(), status: "pending".into(), ..Submittal::default() });
            proc.submittals.len() - 1
        }
        None => return Err(bad("procure.submittal", "the submittal log is full")),
    };
    let Some(x) = proc.submittals.get_mut(i) else { return Err(bad("procure.submittal", "no such submittal")) };
    for (k, f) in [
        ("spec_section", &mut x.spec_section),
        ("status", &mut x.status),
        ("submitted", &mut x.submitted),
        ("returned", &mut x.returned),
        ("reviewer", &mut x.reviewer),
    ] {
        if let Some(v) = t(k) {
            *f = v;
        }
    }
    if let Some(c) = t("comment").filter(|c| !c.is_empty())
        && x.comments.len() < 500
    {
        x.comments.push(c);
    }
    let out = serde_json::to_value(&*x).unwrap_or(Value::Null);
    save(s, sp, &proc)?;
    Ok(out)
}

fn run_submittals(s: &mut Session, _p: &Value) -> Result<Value> {
    let proc = load(&spec("procure.submittals", s)?);
    Ok(json!({"submittals": proc.submittals}))
}

fn run_leadtimes(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec("procure.leadtimes", s)?;
    let mut proc = load(&sp);
    let mut changed = false;
    if let Some(a) = p.get("set").and_then(Value::as_array) {
        for x in a.iter().take(500) {
            let (Some(class), Some(lo), Some(hi)) = (x["class"].as_str(), x["weeks_min"].as_f64(), x["weeks_max"].as_f64()) else {
                return Err(bad("procure.leadtimes", "each lead time needs class, weeks_min and weeks_max"));
            };
            if !(lo.is_finite() && hi.is_finite() && 0.0 <= lo && lo <= hi && hi <= 520.0) {
                return Err(bad("procure.leadtimes", "weeks from 0 to 520, min ≤ max"));
            }
            proc.lead_times.retain(|l| !l.class.eq_ignore_ascii_case(class));
            proc.lead_times.push(LeadTime { class: class.trim().to_ascii_lowercase(), weeks_min: lo, weeks_max: hi });
            changed = true;
        }
    }
    let table: Vec<Value> = LEAD_DEFAULTS
        .iter()
        .map(|(c, _, _)| *c)
        .chain(proc.lead_times.iter().map(|l| l.class.as_str()).filter(|c| !LEAD_DEFAULTS.iter().any(|(d, _, _)| d == c)))
        .map(|c| {
            let (a, b) = lead(&proc, c);
            json!({"class": c, "weeks_min": a, "weeks_max": b, "company": proc.lead_times.iter().any(|l| l.class == c)})
        })
        .collect();
    if changed {
        save(s, sp, &proc)?;
    }
    Ok(json!({"lead_times": table}))
}

/// A release found by id or name in `procure.list`, and its rows as exported (a header first).
fn release_rows(s: &mut Session, p: &Value) -> Result<(Value, Vec<Vec<String>>)> {
    let rel = str_param(p, "release").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("procure.export", "release required"))?.to_string();
    let listed = run_list(s, p)?;
    let r = listed["releases"]
        .as_array()
        .and_then(|a| {
            a.iter().find(|r| {
                r["id"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(&rel)) || r["name"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(&rel))
            })
        })
        .cloned()
        .ok_or_else(|| bad("procure.export", format!("no release {}", rel.chars().take(40).collect::<String>())))?;
    let mut rows = vec![
        ["Item", "Size", "Level", "Unit", "Need", "Spares", "Qty", "Class", "Lead (wk)", "Need date", "Order by", "Submittal", "Status", "Flags"]
            .map(String::from)
            .to_vec(),
    ];
    for l in r["lines"].as_array().into_iter().flatten() {
        let n = |k: &str| {
            let v = l[k].as_f64().unwrap_or(0.0);
            if (v - v.round()).abs() < 1e-9 { format!("{}", v.round() as i64) } else { format!("{:.1}", v) }
        };
        rows.push(vec![
            st(l, "item"),
            st(l, "size"),
            st(l, "level"),
            st(l, "unit"),
            n("need"),
            n("spares"),
            n("qty"),
            st(l, "class"),
            format!("{}–{}", l["lead_weeks"][0], l["lead_weeks"][1]),
            st(l, "need_date"),
            st(l, "order_by"),
            st(l, "submittal"),
            st(l, "status"),
            l["flags"].as_array().map(|f| f.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")).unwrap_or_default(),
        ]);
    }
    Ok((r, rows))
}

/// The PDF's sheet: Letter landscape (inches), the text height and the columns' widths (every
/// column of the export, Class and Flags included).
const PDF_PAGE: [f64; 2] = [11.0, 8.5];
const PDF_TEXT: f64 = 0.08;
const PDF_ROW: f64 = 0.16;
const PDF_COLS: [f64; 14] = [2.0, 0.55, 0.45, 0.4, 0.45, 0.45, 0.45, 0.75, 0.55, 0.75, 0.75, 0.8, 0.65, 1.7];

/// A release as PDF pages: a drawing of its sheets (one under another, a page window each), the
/// plot parameters, and the cells too long for their column (cut with "…", every one listed:
/// nothing is left out silently). Every line of the release is on a page.
pub fn release_pdf(s: &mut Session, p: &Value) -> Result<(cadaddy_doc::Drawing, Value, Vec<String>)> {
    let (r, rows) = release_rows(s, p)?;
    let mut d = cadaddy_doc::Drawing::new_imperial();
    let text = |d: &mut cadaddy_doc::Drawing, x: f64, y: f64, h: f64, v: String| {
        let t = cadaddy_doc::Text {
            insert: cadaddy_geom::Vec3::new(x, y, 0.0),
            align_pt: None,
            height: h,
            value: v,
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: Default::default(),
            valign: Default::default(),
        };
        let _ = d.add(&cadaddy_doc::Space::Model, cadaddy_doc::Common::default(), cadaddy_doc::EntityKind::Text(t));
    };
    let (header, body) = rows.split_first().map_or((&[][..], &[][..]), |(h, b)| (h.as_slice(), b));
    // Rows a page holds under the title and the header.
    let per_page = (((PDF_PAGE[1] - 1.4) / PDF_ROW).floor() as usize).max(1);
    let pages = body.len().div_ceil(per_page).max(1);
    let mut cut = Vec::new();
    let mut windows = Vec::new();
    for page in 0..pages {
        // Page `page` is the sheet `page` sheets below the first.
        let y0 = -(page as f64) * (PDF_PAGE[1] + 1.0);
        windows.push(json!({"window": [[0.0, y0], [PDF_PAGE[0], y0 + PDF_PAGE[1]]], "paper": "Letter", "landscape": true}));
        text(&mut d, 0.25, y0 + 8.0, 0.18, format!("RELEASE {}: {}   page {} of {pages}", st(&r, "id"), st(&r, "name"), page + 1));
        // The header, then this page's lines (each with its number in the release).
        let page_rows: Vec<(Option<usize>, &[String])> = std::iter::once((None, header))
            .chain(body.iter().enumerate().skip(page * per_page).take(per_page).map(|(i, r)| (Some(i), r.as_slice())))
            .collect();
        for (row_i, (line, row)) in page_rows.into_iter().enumerate() {
            let y = y0 + 7.6 - row_i as f64 * PDF_ROW;
            let mut x = 0.25;
            for (c, w) in PDF_COLS.iter().enumerate() {
                let cell = row.get(c).map_or("", String::as_str);
                let fits = ((w / (PDF_TEXT * 0.85)).floor() as usize).max(2);
                let shown = if cell.chars().count() > fits {
                    if let Some(i) = line {
                        cut.push(format!("line {}, {}: {cell}", i + 1, header.get(c).map_or("", String::as_str)));
                    }
                    format!("{}…", cell.chars().take(fits - 1).collect::<String>())
                } else {
                    cell.to_string()
                };
                text(&mut d, x, y, PDF_TEXT, shown);
                x += w;
            }
        }
    }
    let plot = json!({"pages": windows, "margins": [0.0, 0.0, 0.0, 0.0], "lineweights": false, "stamp": false});
    Ok((d, plot, cut))
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("procure.export", "path required"))?.to_string();
    let format = std::path::Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("xlsx").to_ascii_lowercase();
    let (r, rows) = release_rows(s, p)?;
    let mut answer = json!({"path": path, "lines": rows.len().saturating_sub(1)});
    let bytes = match format.as_str() {
        "xlsx" => super::table::xlsx::write(&rows, &format!("Release {}", st(&r, "id"))).map_err(|e| bad("procure.export", e))?,
        "csv" => {
            let field = |t: &str| if t.contains([',', '"', '\n']) { format!("\"{}\"", t.replace('"', "\"\"")) } else { t.to_string() };
            rows.iter().map(|r| r.iter().map(|f| field(f)).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join("\n").into_bytes()
        }
        "pdf" => {
            let plot = super::file::io()
                .and_then(|h| h.plot)
                .ok_or_else(|| bad("procure.export", "PDF export isn't available in this build (export XLSX or CSV)"))?;
            let (d, params, cut) = release_pdf(s, p)?;
            answer["pages"] = json!(params["pages"].as_array().map_or(0, Vec::len));
            // What didn't fit its column is said (the XLSX and CSV have it whole).
            answer["cut"] = json!(cut);
            plot(&d, &cadaddy_doc::Space::Model, &params).map_err(|e| bad("procure.export", e))?
        }
        _ => return Err(bad("procure.export", "the file is .xlsx, .csv or .pdf")),
    };
    #[cfg(not(target_arch = "wasm32"))]
    crate::writefile::write(&path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
    #[cfg(target_arch = "wasm32")]
    let _ = &bytes;
    Ok(answer)
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("procure.release", "New Material Release", run_release)
            .params("{name, level?: floor, phase?: 4D task name (wildcards: its floor and system, its start the need date), trades?: [trade], categories?: [category], need_by?: date, allow_unlinked?: bool, today?} → {id, lines, unlinked}: what the BOM still needs there, exact need + spares by rule"),
        CommandSpec::new("procure.list", "Procurement", run_list)
            .params("{today?: date} → {releases: [{id, name, level, lines: [{item, size, need, spares, qty, class, lead_weeks, need_date, order_by, submittal, status, flags}], late, unlinked}], unreleased, sums_to_bom}")
            .noundo(),
        CommandSpec::new("procure.status", "Order Status", run_status).params("{release, item, status: planned|ordered|shipped|received|short, received?: qty} → {changed}"),
        CommandSpec::new("procure.submittal", "Submittal Log Entry", run_submittal)
            .params("{item: class or item text, spec_section?, status?: pending|submitted|approved|approved_as_noted|revise_resubmit|rejected, submitted?, returned?, reviewer?, comment?} → the entry"),
        CommandSpec::new("procure.submittals", "Submittal Log", run_submittals).params("{} → {submittals}").noundo(),
        CommandSpec::new("procure.leadtimes", "Lead Times", run_leadtimes).params("{set?: [{class, weeks_min, weeks_max}]} → {lead_times: [{class, weeks_min, weeks_max, company}]}"),
        CommandSpec::new("procure.export", "Export Release", run_export).params("{release, path: .xlsx|.csv|.pdf, today?} → {path, lines}").noundo(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes_and_groups() {
        assert_eq!(class_of("equipment", "switchboard MDP"), "gear");
        assert_eq!(class_of("lug kits", "Lug kit TX-1 secondary: bolt"), "lug kits");
        assert_eq!(class_of("devices", "Duplex receptacle"), "devices");
        assert_eq!(class_of("equipment", "panel P1"), "panelboards");
        assert_eq!(group_of(&json!({"item": "Lug kit TX-1 secondary: nut"})).as_deref(), Some("TX-1"));
        assert_eq!(group_of(&json!({"item": "transformer TX-1", "category": "equipment"})).as_deref(), Some("TX-1"));
        assert_eq!(group_of(&json!({"item": "ahu AHU-1", "category": "equipment"})), None);
    }
}
