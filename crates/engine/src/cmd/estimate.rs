//! Agent-first estimating (Q327): the agent brings judgment and prices; the software does every
//! count, multiplication and roll-up, explains every number and checks the result. No number in
//! an estimate is typed or computed by the agent.
//!
//! Quantities are the bill of materials (`bim.bom`, Q326). Each line is priced through its
//! **assembly** (parts × part prices + labor per unit) or its own **price**, from a baseline
//! imported in CADaddy's open format (`docs/estimating-baseline.md`) or prices the agent adds
//! from supplier quotes. Labor is hours or dollars (dollars ÷ crew rate = hours). Each line has
//! a **scope** (`in_contract`, `by_others`, `nic`, `ofci`, `alternate:<n>`); nothing undecided
//! reaches a total.
//!
//! - `estimate.import {folder}`: the baseline (`item_labor_units.csv`, `assemblies.csv`,
//!   `assembly_parts.csv`, `prices.csv`, `rates.json`, `benchmarks.json`).
//! - `estimate.set_price {item, unit, material, labor_hours? | labor_dollars?, date, source}`.
//! - `estimate.set_assembly {lines? | trade?/category?/item?, assembly, reason, citation?}`.
//! - `estimate.set_scope {lines? | trade?/category?/item?, scope, reason, citation?}`.
//! - `estimate.settings {tax_pct?, overhead_pct?, profit_pct?, contingency_pct?, bond_pct?,
//!   crew_rate?, today?, stale_days?}`.
//! - `estimate.query {trade?, category?, status?, limit?, cursor?}`: priced lines, summary first.
//! - `estimate.status`: totals by scope and the validator's findings, each with a fix.
//! - `estimate.explain {line? | total?}`: a number traced to its math, its lines and elements.
//! - `estimate.compare {assembly?: {from, to}, scope?: {line: scope}, settings?}`: a what-if
//!   against the current estimate.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

use super::bspec::current;
use super::*;
use crate::{Result, Session};

/// Rows read from one baseline file, at most (hostile files).
const MAX_ROWS: usize = 200_000;
/// Lines per `estimate.query` page by default, and at most.
const PAGE: usize = 50;
const MAX_PAGE: usize = 1_000;
const SCOPES: [&str; 4] = ["in_contract", "by_others", "nic", "ofci"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("estimate.import", "Import Estimating Baseline", run_import)
            .params("{folder: path (item_labor_units.csv, assemblies.csv, assembly_parts.csv, prices.csv, rates.json, benchmarks.json)} → {items, assemblies, prices, rates, benchmarks, notes}"),
        CommandSpec::new("estimate.set_price", "Set Estimate Price", run_set_price).params(
            "{item, size?, unit: each|ft|lb|sq ft…, material: dollars per unit, labor_hours?: per unit, labor_dollars?: per unit (÷ crew rate), date: YYYY-MM-DD, source} → {price}",
        ),
        CommandSpec::new("estimate.set_assembly", "Set Estimate Assembly", run_set_assembly)
            .params("{lines?: [key] | trade?, category?, item?, assembly, reason, citation?} → {lines}"),
        CommandSpec::new("estimate.set_scope", "Set Estimate Scope", run_set_scope)
            .params("{lines?: [key] | trade?, category?, item?, scope: in_contract|by_others|nic|ofci|alternate:<n>, reason, citation?} → {lines}"),
        CommandSpec::new("estimate.settings", "Estimate Settings", run_settings)
            .params("{tax_pct?, overhead_pct?, profit_pct?, contingency_pct?, bond_pct?, crew_rate?, today?: YYYY-MM-DD, stale_days?} → {settings}"),
        CommandSpec::new("estimate.query", "Query Estimate", run_query)
            .params("{trade?, category?, status?: priced|unpriced|undecided|error, limit?, cursor?} → {summary, lines: [{key, item, size, unit, qty, material, labor_hours, labor_cost, cost, scope, status, issues}], next?}")
            .noundo(),
        CommandSpec::new("estimate.status", "Estimate Status", run_status)
            .params("{} → {totals: {direct, tax, overhead, profit, contingency, bond, total, labor_hours}, by_scope, alternates, findings: [{kind, line?, message, fix}], ok}")
            .noundo(),
        CommandSpec::new("estimate.explain", "Explain Estimate Number", run_explain)
            .params("{line?: key | total?: true} → {value, math: [steps], lines?, elements?}")
            .noundo(),
        CommandSpec::new("estimate.compare", "Compare Estimate What-If", run_compare)
            .params("{assembly?: {from, to}, scope?: {key: scope}, settings?: {…}} → {now, what_if, delta}")
            .noundo(),
    ]
}

// ---------------- state ----------------

/// The estimate's state from the spec (an object with `baseline`, `map`, `scope`, `settings`).
pub(super) fn state(s: &Session, cmd: &str) -> Result<(cadaddy_walk3d::BuildingSpec, Map<String, Value>)> {
    let (sp, _) = current(s, cmd)?;
    let st = sp.estimate.as_object().cloned().unwrap_or_default();
    Ok((sp, st))
}

pub(super) fn store(s: &mut Session, mut sp: cadaddy_walk3d::BuildingSpec, st: Map<String, Value>) -> Result<()> {
    sp.estimate = Value::Object(st);
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(())
}

/// The state's object `k`, made one if it isn't.
pub(super) fn obj<'a>(st: &'a mut Map<String, Value>, k: &str) -> Option<&'a mut Map<String, Value>> {
    let v = st.entry(k.to_string()).or_insert_with(|| json!({}));
    if !v.is_object() {
        *v = json!({});
    }
    v.as_object_mut()
}

/// A BOM line's key: trade|category|item|size.
fn key_of(l: &Value) -> String {
    format!(
        "{}|{}|{}|{}",
        l["trade"].as_str().unwrap_or_default(),
        l["category"].as_str().unwrap_or_default(),
        l["item"].as_str().unwrap_or_default(),
        l["size"].as_str().unwrap_or_default()
    )
}

pub(super) fn setting(st: &Map<String, Value>, k: &str, default: f64) -> f64 {
    st.get("settings").and_then(|s| s.get(k)).and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(default)
}

// ---------------- CSV ----------------

/// Parse CSV text (quoted fields, doubled quotes) into header-keyed rows.
fn csv_rows(text: &str) -> Vec<BTreeMap<String, String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for line in text.lines().take(MAX_ROWS) {
        let mut cells = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match (c, quoted) {
                ('"', true) if chars.peek() == Some(&'"') => {
                    cur.push('"');
                    chars.next();
                }
                ('"', true) => quoted = false,
                // A quote opens a quoted field only at its start; inside one it's an inch mark.
                ('"', false) if cur.is_empty() => quoted = true,
                (',', false) => cells.push(std::mem::take(&mut cur)),
                _ => cur.push(c),
            }
        }
        cells.push(cur);
        if cells.iter().any(|c| !c.trim().is_empty()) {
            rows.push(cells.into_iter().map(|c| c.trim().to_string()).collect());
        }
    }
    let Some(header) = rows.first().cloned() else { return Vec::new() };
    let header: Vec<String> = header.iter().map(|h| h.to_ascii_lowercase()).collect();
    rows.into_iter().skip(1).map(|r| header.iter().cloned().zip(r).collect()).collect()
}

fn num(r: &BTreeMap<String, String>, k: &str) -> Option<f64> {
    r.get(k).and_then(|v| v.trim().trim_start_matches('$').replace(',', "").parse::<f64>().ok()).filter(|v| v.is_finite())
}

// ---------------- import and set ----------------

fn run_import(s: &mut Session, p: &Value) -> Result<Value> {
    let folder = str_param(p, "folder").ok_or_else(|| bad("estimate.import", "`folder` is required"))?;
    let dir = std::path::Path::new(folder);
    if !dir.is_dir() {
        return Err(bad("estimate.import", format!("{folder}: not a folder")));
    }
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
    let (sp, mut st) = state(s, "estimate.import")?;
    let mut notes: Vec<String> = Vec::new();
    let mut baseline = Map::new();
    let mut items = Map::new();
    for r in read("item_labor_units.csv").map(|t| csv_rows(&t)).unwrap_or_default() {
        let Some(item) = r.get("item").filter(|i| !i.is_empty()) else { continue };
        items.insert(
            item.clone(),
            json!({ "unit": r.get("unit").cloned().unwrap_or_default(), "material": num(&r, "material"), "labor": { "normal": num(&r, "labor_normal"), "difficult": num(&r, "labor_difficult"), "very_difficult": num(&r, "labor_very_difficult") } }),
        );
    }
    let mut assemblies = Map::new();
    for r in read("assemblies.csv").map(|t| csv_rows(&t)).unwrap_or_default() {
        let Some(a) = r.get("assembly").filter(|i| !i.is_empty()) else { continue };
        assemblies.insert(a.clone(), json!({ "unit": r.get("unit").cloned().unwrap_or_default(), "description": r.get("description").cloned().unwrap_or_default(), "labor_hours": num(&r, "labor_hours"), "parts": [] }));
    }
    for r in read("assembly_parts.csv").map(|t| csv_rows(&t)).unwrap_or_default() {
        let (Some(a), Some(item)) = (r.get("assembly"), r.get("item")) else { continue };
        match assemblies.get_mut(a).and_then(|x| x["parts"].as_array_mut()) {
            Some(parts) => parts.push(json!({ "item": item, "qty": num(&r, "qty").unwrap_or(1.0) })),
            None => notes.push(format!("assembly_parts: {a} is not in assemblies.csv")),
        }
    }
    let rates: Value = read("rates.json").and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    let crew = rates["crew_rate"].as_f64().filter(|v| *v > 0.0);
    let mut prices: Vec<Value> = Vec::new();
    for r in read("prices.csv").map(|t| csv_rows(&t)).unwrap_or_default() {
        let Some(item) = r.get("item").filter(|i| !i.is_empty()) else { continue };
        // Labor in dollars (as some cost databases keep it): hours = dollars ÷ crew rate.
        let hours = num(&r, "labor_hours").or_else(|| num(&r, "labor_dollars").zip(crew).map(|(d, c)| d / c));
        if num(&r, "labor_dollars").is_some() && crew.is_none() {
            notes.push(format!("prices: {item} has labor in dollars but rates.json has no crew_rate"));
        }
        prices.push(json!({ "item": item, "size": r.get("size").cloned().unwrap_or_default(), "unit": r.get("unit").cloned().unwrap_or_default(), "material": num(&r, "material"), "labor_hours": hours, "date": r.get("date").cloned().unwrap_or_default(), "source": r.get("source").cloned().unwrap_or_else(|| "baseline".into()) }));
    }
    let benchmarks: Value = read("benchmarks.json").and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    let counts = json!({ "items": items.len(), "assemblies": assemblies.len(), "prices": prices.len(), "rates": rates.as_object().map_or(0, Map::len), "benchmarks": benchmarks.as_object().map_or(0, Map::len), "notes": notes });
    baseline.insert("items".into(), Value::Object(items));
    baseline.insert("assemblies".into(), Value::Object(assemblies));
    baseline.insert("prices".into(), Value::Array(prices));
    baseline.insert("benchmarks".into(), benchmarks);
    st.insert("baseline".into(), Value::Object(baseline));
    if let (Some(c), Some(set)) = (crew, obj(&mut st, "settings")) {
        set.insert("crew_rate".into(), json!(c));
    }
    store(s, sp, st)?;
    Ok(counts)
}

fn run_set_price(s: &mut Session, p: &Value) -> Result<Value> {
    let item = str_param(p, "item").filter(|i| !i.trim().is_empty()).ok_or_else(|| bad("estimate.set_price", "`item` is required"))?;
    let unit = str_param(p, "unit").filter(|u| !u.trim().is_empty()).ok_or_else(|| bad("estimate.set_price", "`unit` is required"))?;
    let material =
        p["material"].as_f64().filter(|v| v.is_finite()).ok_or_else(|| bad("estimate.set_price", "`material` (dollars per unit) is required"))?;
    let date = str_param(p, "date").ok_or_else(|| bad("estimate.set_price", "`date` (YYYY-MM-DD) is required"))?;
    let source = str_param(p, "source")
        .filter(|x| !x.trim().is_empty())
        .ok_or_else(|| bad("estimate.set_price", "`source` (where the price came from) is required"))?;
    let (sp, mut st) = state(s, "estimate.set_price")?;
    let crew = setting(&st, "crew_rate", 0.0);
    let hours = p["labor_hours"].as_f64().or_else(|| p["labor_dollars"].as_f64().filter(|_| crew > 0.0).map(|d| d / crew));
    let size = str_param(p, "size").unwrap_or_default();
    let price = json!({ "item": item, "size": size, "unit": unit, "material": material, "labor_hours": hours, "date": date, "source": source });
    if let Some(a) = obj(&mut st, "baseline").and_then(|base| base.entry("prices").or_insert_with(|| json!([])).as_array_mut()) {
        a.retain(|x| !(x["item"].as_str() == Some(item) && x["size"].as_str().unwrap_or_default() == size));
        a.push(price.clone());
    }
    store(s, sp, st)?;
    Ok(json!({ "price": price }))
}

/// The BOM line keys a set call picks: `lines`, else every line matching trade/category/item.
fn picked(cmd: &str, s: &mut Session, p: &Value) -> Result<Vec<String>> {
    let bom = s.execute("bim.bom", &json!({}))?;
    let lines = bom["lines"].as_array().cloned().unwrap_or_default();
    if let Some(ks) = p.get("lines").and_then(Value::as_array) {
        let want: Vec<&str> = ks.iter().filter_map(Value::as_str).collect();
        return Ok(lines.iter().map(key_of).filter(|k| want.contains(&k.as_str())).collect());
    }
    if ["trade", "category", "item"].iter().all(|k| str_param(p, k).is_none()) {
        return Err(bad(cmd, "name the lines: `lines`, or a `trade`, `category` or `item` filter"));
    }
    let m = |l: &Value, k: &str| str_param(p, k).is_none_or(|w| l[k].as_str().is_some_and(|v| v.eq_ignore_ascii_case(w)));
    Ok(lines.iter().filter(|l| m(l, "trade") && m(l, "category") && m(l, "item")).map(key_of).collect())
}

fn run_set_assembly(s: &mut Session, p: &Value) -> Result<Value> {
    let assembly = str_param(p, "assembly").ok_or_else(|| bad("estimate.set_assembly", "`assembly` is required"))?.to_string();
    let reason =
        str_param(p, "reason").filter(|r| !r.trim().is_empty()).ok_or_else(|| bad("estimate.set_assembly", "`reason` is required"))?.to_string();
    let keys = picked("estimate.set_assembly", s, p)?;
    let (sp, mut st) = state(s, "estimate.set_assembly")?;
    if st.get("baseline").and_then(|b| b["assemblies"].get(&assembly)).is_none() {
        return Err(bad("estimate.set_assembly", format!("there is no assembly named {assembly} (import a baseline first)")));
    }
    if let Some(map) = obj(&mut st, "map") {
        for k in &keys {
            map.insert(k.clone(), json!({ "assembly": assembly, "reason": reason, "citation": str_param(p, "citation") }));
        }
    }
    store(s, sp, st)?;
    Ok(json!({ "lines": keys }))
}

fn run_set_scope(s: &mut Session, p: &Value) -> Result<Value> {
    let scope = str_param(p, "scope").map(str::trim).unwrap_or_default().to_ascii_lowercase();
    if !(SCOPES.contains(&scope.as_str()) || scope.strip_prefix("alternate:").is_some_and(|n| !n.is_empty() && n.len() <= 16)) {
        return Err(bad("estimate.set_scope", "`scope` is in_contract, by_others, nic, ofci or alternate:<n>"));
    }
    let reason =
        str_param(p, "reason").filter(|r| !r.trim().is_empty()).ok_or_else(|| bad("estimate.set_scope", "`reason` is required"))?.to_string();
    let keys = picked("estimate.set_scope", s, p)?;
    let (sp, mut st) = state(s, "estimate.set_scope")?;
    if let Some(m) = obj(&mut st, "scope") {
        for k in &keys {
            m.insert(k.clone(), json!({ "scope": scope, "reason": reason, "citation": str_param(p, "citation") }));
        }
    }
    store(s, sp, st)?;
    Ok(json!({ "lines": keys }))
}

fn run_settings(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, mut st) = state(s, "estimate.settings")?;
    let Some(set) = obj(&mut st, "settings") else { return Err(bad("estimate.settings", "no settings")) };
    for k in ["tax_pct", "overhead_pct", "profit_pct", "contingency_pct", "bond_pct", "crew_rate", "stale_days"] {
        if let Some(v) = p.get(k).filter(|v| !v.is_null()) {
            let v = v
                .as_f64()
                .filter(|x| x.is_finite() && *x >= 0.0 && *x <= 1e6)
                .ok_or_else(|| bad("estimate.settings", format!("`{k}` must be a number, 0 or more")))?;
            set.insert(k.into(), json!(v));
        }
    }
    if let Some(d) = str_param(p, "today") {
        if parse_date(d).is_none() {
            return Err(bad("estimate.settings", "`today` is YYYY-MM-DD"));
        }
        set.insert("today".into(), json!(d));
    }
    let out = Value::Object(set.clone());
    store(s, sp, st)?;
    Ok(json!({ "settings": out }))
}

// ---------------- the calculator ----------------

/// Days since 0000-03-01 for a YYYY-MM-DD date (for ages in days).
fn parse_date(d: &str) -> Option<i64> {
    let mut it = d.trim().split('-');
    let (y, m, dd) = (it.next()?.parse::<i64>().ok()?, it.next()?.parse::<i64>().ok()?, it.next()?.parse::<i64>().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&dd) || !(1900..=3000).contains(&y) {
        return None;
    }
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    Some(365 * y + y / 4 - y / 100 + y / 400 + (153 * m + 2) / 5 + dd)
}

/// One priced line.
#[derive(Clone, Debug, Default)]
pub(super) struct Priced {
    pub(super) key: String,
    pub(super) line: Value,
    pub(super) qty: f64,
    pub(super) net: f64,
    pub(super) unit: String,
    pub(super) material_unit: Option<f64>,
    pub(super) labor_unit_hours: Option<f64>,
    pub(super) material: f64,
    pub(super) labor_hours: f64,
    pub(super) labor_cost: f64,
    pub(super) cost: f64,
    pub(super) scope: Option<String>,
    pub(super) via: String,
    pub(super) issues: Vec<(&'static str, String, String)>,
}

/// Price every BOM line from the state; `what_if` overrides assemblies, scopes and settings.
pub(super) fn price_all(s: &mut Session, st: &Map<String, Value>) -> Result<(Vec<Priced>, f64)> {
    let bom = s.execute("bim.bom", &json!({}))?;
    let lines = bom["lines"].as_array().cloned().unwrap_or_default();
    let crew = setting(st, "crew_rate", 0.0);
    let today = st.get("settings").and_then(|x| x["today"].as_str()).and_then(parse_date);
    let stale = setting(st, "stale_days", 365.0);
    let base = st.get("baseline").cloned().unwrap_or(json!({}));
    let prices: Vec<Value> = base["prices"].as_array().cloned().unwrap_or_default();
    let find_price = |item: &str, size: &str| -> Option<&Value> {
        prices
            .iter()
            .find(|p| {
                p["item"].as_str().is_some_and(|i| i.eq_ignore_ascii_case(item)) && p["size"].as_str().unwrap_or_default().eq_ignore_ascii_case(size)
            })
            .or_else(|| {
                prices
                    .iter()
                    .find(|p| p["item"].as_str().is_some_and(|i| i.eq_ignore_ascii_case(item)) && p["size"].as_str().unwrap_or_default().is_empty())
            })
    };
    let mut out = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for l in &lines {
        let key = key_of(l);
        *seen.entry(key.clone()).or_insert(0) += 1;
        let unit = l["unit"].as_str().unwrap_or_default().to_string();
        let (qty, net) = (l["total"].as_f64().unwrap_or(0.0), l["net"].as_f64().unwrap_or(0.0));
        let mut pr = Priced { key: key.clone(), line: l.clone(), qty, net, unit: unit.clone(), ..Priced::default() };
        pr.scope = st.get("scope").and_then(|m| m.get(&key)).and_then(|x| x["scope"].as_str()).map(String::from);
        let assembly = st.get("map").and_then(|m| m.get(&key)).and_then(|x| x["assembly"].as_str()).map(String::from);
        let check_price = |p: &Value, issues: &mut Vec<(&'static str, String, String)>, what: &str| -> Option<(f64, Option<f64>)> {
            let pu = p["unit"].as_str().unwrap_or_default();
            // A part's unit is its own; a line's price must be in the line's unit.
            if what == "line" && !pu.eq_ignore_ascii_case(&unit) {
                issues.push((
                    "unit_mismatch",
                    format!("priced per {pu}, counted per {unit}"),
                    "price it per the counted unit (or convert the price)".into(),
                ));
                return None;
            }
            let m = p["material"].as_f64();
            if m.is_some_and(|m| m <= 0.0) {
                issues.push(("zero_price", format!("{what} price is {m:?}"), "check the price list".into()));
            }
            if let (Some(t), Some(d)) = (today, p["date"].as_str().and_then(parse_date))
                && (t - d) as f64 > stale
            {
                issues.push((
                    "stale_price",
                    format!("price dated {} ({} days old)", p["date"].as_str().unwrap_or_default(), t - d),
                    "get a current quote".into(),
                ));
            }
            Some((m.unwrap_or(0.0), p["labor_hours"].as_f64()))
        };
        match assembly.as_deref().and_then(|a| base["assemblies"].get(a).map(|x| (a, x))) {
            Some((a, asm)) => {
                pr.via = format!("assembly {a}");
                let asm_unit = asm["unit"].as_str().unwrap_or_default();
                if !asm_unit.eq_ignore_ascii_case(&unit) {
                    pr.issues.push((
                        "unit_mismatch",
                        format!("assembly {a} is per {asm_unit}, the line per {unit}"),
                        "map an assembly in the line's unit".into(),
                    ));
                } else {
                    let mut mat = 0.0;
                    let mut missing = Vec::new();
                    for part in asm["parts"].as_array().into_iter().flatten() {
                        let item = part["item"].as_str().unwrap_or_default();
                        let q = part["qty"].as_f64().unwrap_or(1.0);
                        match find_price(item, "").and_then(|p| check_price(p, &mut pr.issues, "part")) {
                            Some((m, _)) => mat += q * m,
                            None => missing.push(item.to_string()),
                        }
                    }
                    if !missing.is_empty() {
                        pr.issues.push((
                            "unpriced_part",
                            format!("assembly {a}: no price for {}", missing.join(", ")),
                            "add the part prices (estimate.set_price)".into(),
                        ));
                    }
                    pr.material_unit = Some(mat);
                    pr.labor_unit_hours = asm["labor_hours"].as_f64();
                }
            }
            None => {
                let item = l["item"].as_str().unwrap_or_default();
                let size = l["size"].as_str().unwrap_or_default();
                match find_price(item, size) {
                    Some(p) => {
                        pr.via = format!("price {} ({})", p["source"].as_str().unwrap_or_default(), p["date"].as_str().unwrap_or_default());
                        if let Some((m, h)) = check_price(p, &mut pr.issues, "line") {
                            pr.material_unit = Some(m);
                            pr.labor_unit_hours = h.or_else(|| base["items"][item]["labor"]["normal"].as_f64());
                        }
                    }
                    None => pr.issues.push((
                        "unpriced",
                        "no assembly and no price".into(),
                        "map an assembly (estimate.set_assembly) or add a price (estimate.set_price)".into(),
                    )),
                }
            }
        }
        if pr.material_unit.is_some() && pr.labor_unit_hours.is_none() {
            pr.issues.push(("missing_labor", "no labor per unit".into(), "add labor hours or dollars to the price or assembly".into()));
        }
        if pr.scope.is_none() {
            pr.issues.push(("undecided_scope", "scope not set".into(), "set it from the bid documents (estimate.set_scope)".into()));
        }
        // Material on the quantity with waste; labor on the net (waste isn't installed).
        pr.material = pr.material_unit.unwrap_or(0.0) * qty;
        pr.labor_hours = pr.labor_unit_hours.unwrap_or(0.0) * net;
        pr.labor_cost = pr.labor_hours * crew;
        pr.cost = pr.material + pr.labor_cost;
        out.push(pr);
    }
    // Lines counted twice (the same key from two sources).
    for pr in &mut out {
        if seen.get(&pr.key).copied().unwrap_or(0) > 1 && pr.line["status"].as_str() == Some("model") {
            pr.issues.push(("duplicate", "counted on more than one line".into(), "check the takeoff for double counting".into()));
        }
    }
    Ok((out, crew))
}

/// Totals over the in-contract lines, with markups; and each alternate's own direct cost.
fn totals(st: &Map<String, Value>, priced: &[Priced]) -> Value {
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    let pct = |k: &str| setting(st, k, 0.0) / 100.0;
    let inc: Vec<&Priced> = priced.iter().filter(|p| p.scope.as_deref() == Some("in_contract")).collect();
    let material: f64 = inc.iter().map(|p| p.material).sum();
    let labor: f64 = inc.iter().map(|p| p.labor_cost).sum();
    let hours: f64 = inc.iter().map(|p| p.labor_hours).sum();
    let direct = material + labor;
    let tax = material * pct("tax_pct");
    let sub = direct + tax;
    let overhead = sub * pct("overhead_pct");
    let profit = (sub + overhead) * pct("profit_pct");
    let contingency = (sub + overhead + profit) * pct("contingency_pct");
    let before_bond = sub + overhead + profit + contingency;
    let bond = before_bond * pct("bond_pct");
    let mut by_scope: BTreeMap<String, f64> = BTreeMap::new();
    for p in priced {
        *by_scope.entry(p.scope.clone().unwrap_or_else(|| "undecided".into())).or_insert(0.0) += p.cost;
    }
    let alternates: BTreeMap<String, f64> = by_scope.iter().filter(|(k, _)| k.starts_with("alternate:")).map(|(k, v)| (k.clone(), r2(*v))).collect();
    json!({
        "totals": { "material": r2(material), "labor": r2(labor), "labor_hours": r2(hours), "direct": r2(direct), "tax": r2(tax), "overhead": r2(overhead), "profit": r2(profit), "contingency": r2(contingency), "bond": r2(bond), "total": r2(before_bond + bond) },
        "by_scope": by_scope.into_iter().map(|(k, v)| (k, json!(r2(v)))).collect::<Map<String, Value>>(),
        "alternates": alternates,
    })
}

/// The validator: every line's issues, and the benchmark ratio checks.
fn findings(st: &Map<String, Value>, priced: &[Priced], levels: usize) -> Vec<Value> {
    let mut out: Vec<Value> = priced
        .iter()
        .filter(|p| p.scope.as_deref() != Some("nic") && p.scope.as_deref() != Some("by_others"))
        .flat_map(|p| p.issues.iter().map(move |(k, m, fix)| json!({ "kind": k, "line": p.key, "message": m, "fix": fix })))
        .collect();
    // Benchmarks: quantities per level within the typical range.
    if let Some(ratios) = st.get("baseline").and_then(|b| b["benchmarks"]["ratios"].as_array()) {
        for r in ratios.iter().take(1_000) {
            let m = r["match"].as_str().unwrap_or_default().to_ascii_lowercase();
            if m.is_empty() {
                continue;
            }
            let qty: f64 =
                priced.iter().filter(|p| p.line["item"].as_str().is_some_and(|i| i.to_ascii_lowercase().contains(&m))).map(|p| p.net).sum();
            let per = qty / levels.max(1) as f64;
            let (lo, hi) = (r["per_level_low"].as_f64().unwrap_or(0.0), r["per_level_high"].as_f64().unwrap_or(f64::INFINITY));
            if per < lo || per > hi {
                out.push(json!({ "kind": "off_ratio", "message": format!("{m}: {per:.1} per level, typical {lo}–{hi} ({})", r["source"].as_str().unwrap_or("benchmarks")), "fix": "check the takeoff for missed or double-counted items" }));
            }
        }
    }
    out
}

fn summary(priced: &[Priced]) -> Value {
    let n = |f: &dyn Fn(&Priced) -> bool| priced.iter().filter(|p| f(p)).count();
    json!({
        "lines": priced.len(),
        "priced": n(&|p| p.material_unit.is_some()),
        "unpriced": n(&|p| p.material_unit.is_none()),
        "undecided": n(&|p| p.scope.is_none()),
        "with_issues": n(&|p| !p.issues.is_empty()),
    })
}

fn line_json(p: &Priced) -> Value {
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    json!({
        "key": p.key, "item": p.line["item"], "size": p.line["size"], "unit": p.unit, "qty": p.qty, "net": p.net,
        "material_unit": p.material_unit, "labor_unit_hours": p.labor_unit_hours,
        "material": r2(p.material), "labor_hours": r2(p.labor_hours), "labor_cost": r2(p.labor_cost), "cost": r2(p.cost),
        "scope": p.scope, "via": p.via,
        "status": if p.issues.iter().any(|i| matches!(i.0, "unit_mismatch")) { "error" } else if p.material_unit.is_none() { "unpriced" } else if p.scope.is_none() { "undecided" } else { "priced" },
        "issues": p.issues.iter().map(|i| i.0).collect::<Vec<_>>(),
    })
}

fn run_query(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, st) = state(s, "estimate.query")?;
    let (priced, _) = price_all(s, &st)?;
    let m = |l: &Value, k: &str| {
        str_param(p, k).is_none_or(|w| l["key"].as_str().is_some_and(|key| key.split('|').any(|part| part.eq_ignore_ascii_case(w))))
    };
    let rows: Vec<Value> = priced
        .iter()
        .map(line_json)
        .filter(|l| m(l, "trade") && m(l, "category") && str_param(p, "status").is_none_or(|s| l["status"].as_str() == Some(s)))
        .collect();
    let limit = p.get("limit").and_then(Value::as_u64).map_or(PAGE, |n| usize::try_from(n).unwrap_or(MAX_PAGE).clamp(1, MAX_PAGE));
    let from = p.get("cursor").and_then(Value::as_u64).map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let mut out = json!({ "summary": summary(&priced), "total": rows.len(), "lines": rows.iter().skip(from).take(limit).collect::<Vec<_>>() });
    if from.saturating_add(limit) < rows.len() {
        out["next"] = json!(from + limit);
    }
    Ok(out)
}

fn run_status(s: &mut Session, _p: &Value) -> Result<Value> {
    let (sp, st) = state(s, "estimate.status")?;
    let (priced, _) = price_all(s, &st)?;
    let mut out = totals(&st, &priced);
    let f = findings(&st, &priced, sp.levels.len());
    out["ok"] = json!(f.is_empty());
    out["summary"] = summary(&priced);
    out["findings"] = json!(f);
    Ok(out)
}

fn run_explain(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, st) = state(s, "estimate.explain")?;
    let (priced, crew) = price_all(s, &st)?;
    if let Some(k) = str_param(p, "line") {
        let pr = priced.iter().find(|x| x.key == k).ok_or_else(|| bad("estimate.explain", format!("no line {k}")))?;
        let mu = pr.material_unit.unwrap_or(0.0);
        let lh = pr.labor_unit_hours.unwrap_or(0.0);
        return Ok(json!({
            "value": (pr.cost * 100.0).round() / 100.0,
            "math": [
                format!("material: {} {} (net {} + waste) × ${mu:.2} = ${:.2}  [{}]", pr.qty, pr.unit, pr.net, pr.material, pr.via),
                format!("labor: {} {} (net) × {lh:.3} h = {:.2} h × ${crew:.2}/h = ${:.2}", pr.net, pr.unit, pr.labor_hours, pr.labor_cost),
                format!("line: ${:.2} + ${:.2} = ${:.2}", pr.material, pr.labor_cost, pr.cost),
            ],
            "scope": pr.scope,
            "rule": pr.line["rule"],
            "elements": pr.line["elements"],
        }));
    }
    if bool_or(p, "total", false) {
        let t = totals(&st, &priced);
        let inc: Vec<Value> = priced
            .iter()
            .filter(|x| x.scope.as_deref() == Some("in_contract"))
            .map(|x| json!({ "line": x.key, "cost": (x.cost * 100.0).round() / 100.0 }))
            .collect();
        let tt = &t["totals"];
        return Ok(json!({
            "value": tt["total"],
            "math": [
                format!("direct = material ${} + labor ${} ({} h) = ${}", tt["material"], tt["labor"], tt["labor_hours"], tt["direct"]),
                format!("tax {}% on material = ${}", setting(&st, "tax_pct", 0.0), tt["tax"]),
                format!("overhead {}% = ${}; profit {}% = ${}; contingency {}% = ${}; bond {}% = ${}", setting(&st, "overhead_pct", 0.0), tt["overhead"], setting(&st, "profit_pct", 0.0), tt["profit"], setting(&st, "contingency_pct", 0.0), tt["contingency"], setting(&st, "bond_pct", 0.0), tt["bond"]),
                format!("total = ${}", tt["total"]),
            ],
            "lines": inc,
        }));
    }
    Err(bad("estimate.explain", "give a `line` key or `total: true`"))
}

fn run_compare(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, st) = state(s, "estimate.compare")?;
    let (priced, _) = price_all(s, &st)?;
    let now = totals(&st, &priced)["totals"].clone();
    let mut alt = st.clone();
    if let Some(a) = p.get("assembly") {
        let (Some(from), Some(to)) = (a["from"].as_str(), a["to"].as_str()) else {
            return Err(bad("estimate.compare", "`assembly` is {from, to}"));
        };
        for v in obj(&mut alt, "map").into_iter().flat_map(|m| m.values_mut()) {
            if v["assembly"].as_str() == Some(from) {
                v["assembly"] = json!(to);
            }
        }
    }
    if let Some(sc) = p.get("scope").and_then(Value::as_object)
        && let Some(m) = obj(&mut alt, "scope")
    {
        for (k, v) in sc {
            m.insert(k.clone(), json!({ "scope": v, "reason": "what-if" }));
        }
    }
    if let Some(set) = p.get("settings").and_then(Value::as_object)
        && let Some(m) = obj(&mut alt, "settings")
    {
        for (k, v) in set {
            m.insert(k.clone(), v.clone());
        }
    }
    let (priced2, _) = price_all(s, &alt)?;
    let what_if = totals(&alt, &priced2)["totals"].clone();
    let delta = (what_if["total"].as_f64().unwrap_or(0.0) - now["total"].as_f64().unwrap_or(0.0)) * 100.0;
    Ok(json!({ "now": now, "what_if": what_if, "delta": delta.round() / 100.0 }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An invented baseline in the open layout.
    fn baseline() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cadaddy-estimate-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("t").replace("::", "-")
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let w = |n: &str, t: &str| std::fs::write(dir.join(n), t).unwrap();
        w("item_labor_units.csv", "item,unit,material,labor_normal,labor_difficult,labor_very_difficult\nEMT conduit,ft,1.00,0.05,0.07,0.09\n");
        w(
            "assemblies.csv",
            "assembly,unit,description,labor_hours\nDUPLEX-MS,each,\"Duplex receptacle, metal stud, box and ring\",0.6\nGFCI-MS,each,GFCI receptacle on metal stud,0.7\n",
        );
        w(
            "assembly_parts.csv",
            "assembly,item,qty\nDUPLEX-MS,Duplex device,1\nDUPLEX-MS,Box 4in sq,1\nGFCI-MS,GFCI device,1\nGFCI-MS,Box 4in sq,1\n",
        );
        w(
            "prices.csv",
            "item,size,unit,material,labor_dollars,date,source\nDuplex device,,each,2.00,,2026-09-01,Invented Supply\nGFCI device,,each,15.00,,2026-09-01,Invented Supply\nBox 4in sq,,each,3.00,,2026-09-01,Invented Supply\nEMT conduit,3/4\",ft,1.10,4.25,2026-09-01,Invented Supply\nEMT coupling,3/4\",C,50.00,0,2020-01-01,Old Book\n",
        );
        w("rates.json", "{\"crew_rate\": 85.0}");
        w(
            "benchmarks.json",
            "{\"ratios\": [{\"match\": \"receptacle\", \"per_level_low\": 10, \"per_level_high\": 40, \"source\": \"invented past jobs\"}]}",
        );
        dir
    }

    fn job(s: &mut Session) {
        let tags = json!([
            {"id": "R1", "kind": "receptacle", "level": "1", "at_ft": [3, 1], "fields": {"device_type": {"value": "standard duplex receptacle", "source": "default"}}},
            {"id": "R2", "kind": "receptacle", "level": "1", "at_ft": [4, 1], "fields": {"device_type": {"value": "standard duplex receptacle", "source": "default"}}},
            {"id": "R3", "kind": "receptacle", "level": "1", "at_ft": [5, 1], "fields": {"device_type": {"value": "GFCI receptacle", "source": "plan"}}}
        ]);
        let spec = json!({
            "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}],
            "mep": {
                "systems": [{"name": "E-BRANCH-COND", "layer": "E-BRANCH-COND", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9}],
                "runs": [{"system": "E-BRANCH-COND", "level": "1", "points": [[0, 0], [1200, 0]], "size_in": [0.75, 0], "bottoms_ft": [9]}]
            },
            "tags": tags
        });
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
    }

    #[test]
    fn a_job_is_priced_explained_and_checked_by_the_software() {
        let mut s = Session::new();
        job(&mut s);
        let dir = baseline();
        let r = s.execute("estimate.import", &json!({ "folder": dir.to_string_lossy() })).unwrap();
        assert_eq!((r["assemblies"].clone(), r["prices"].clone()), (json!(2), json!(5)), "{r}");
        s.execute("estimate.settings", &json!({ "tax_pct": 8, "overhead_pct": 10, "profit_pct": 10, "today": "2026-10-21" })).unwrap();
        // Before scope and mapping: nothing reaches a total, and the validator says why.
        let st = s.execute("estimate.status", &json!({})).unwrap();
        assert_eq!(st["totals"]["total"], json!(0.0));
        let kinds: Vec<&str> = st["findings"].as_array().unwrap().iter().filter_map(|f| f["kind"].as_str()).collect();
        assert!(kinds.contains(&"undecided_scope") && kinds.contains(&"unpriced") && kinds.contains(&"off_ratio"), "{kinds:?}");
        // The agent maps assemblies and sets scope (with reasons); the software prices.
        s.execute(
            "estimate.set_assembly",
            &json!({ "item": "standard duplex receptacle", "assembly": "DUPLEX-MS", "reason": "metal stud walls per A-501" }),
        )
        .unwrap();
        s.execute("estimate.set_assembly", &json!({ "item": "GFCI receptacle", "assembly": "GFCI-MS", "reason": "metal stud walls per A-501" }))
            .unwrap();
        s.execute(
            "estimate.set_scope",
            &json!({ "trade": "electrical", "scope": "in_contract", "reason": "Division 26 is ours", "citation": "bid form" }),
        )
        .unwrap();
        let conduit_key = "electrical|conduit|EMT conduit|3/4\"";
        let ex = s.execute("estimate.explain", &json!({ "line": conduit_key })).unwrap();
        // 100' + 10% waste = 110' × $1.10 = $121.00; labor $4.25/ft ÷ $85/h = 0.05 h × 100' = 5 h = $425.
        assert_eq!(ex["value"], json!(546.0), "{ex}");
        assert!(ex["math"][0].as_str().unwrap().contains("110"), "{ex}");
        let dup = s.execute("estimate.query", &json!({ "category": "devices" })).unwrap();
        let d = dup["lines"].as_array().unwrap().iter().find(|l| l["item"] == json!("standard duplex receptacle")).unwrap();
        // (2 + 2% waste → 2) × ($2 + $3) = $10; labor 2 × 0.6 h × $85 = $102.
        assert_eq!((d["material"].clone(), d["labor_cost"].clone(), d["cost"].clone()), (json!(10.0), json!(102.0), json!(112.0)), "{d}");
        // The old coupling price: per hundred, counted per each → refused, and stale.
        let st = s.execute("estimate.status", &json!({})).unwrap();
        let f: Vec<(String, String)> = st["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| (x["kind"].as_str().unwrap().to_string(), x["line"].as_str().unwrap_or_default().to_string()))
            .collect();
        assert!(f.iter().any(|(k, l)| k == "unit_mismatch" && l.contains("EMT coupling")), "{f:?}");
        assert!(st["totals"]["total"].as_f64().unwrap() > 0.0);
        let tot = s.execute("estimate.explain", &json!({ "total": true })).unwrap();
        assert_eq!(tot["value"], st["totals"]["total"]);
        // What-if: every duplex as a GFCI.
        let cmp = s.execute("estimate.compare", &json!({ "assembly": { "from": "DUPLEX-MS", "to": "GFCI-MS" } })).unwrap();
        assert!(cmp["delta"].as_f64().unwrap() > 0.0, "{cmp}");
        // Alternates are priced apart from the total.
        s.execute("estimate.set_scope", &json!({ "item": "GFCI receptacle", "scope": "alternate:1", "reason": "Alt 1 on the bid form" })).unwrap();
        let st2 = s.execute("estimate.status", &json!({})).unwrap();
        assert!(st2["alternates"]["alternate:1"].as_f64().unwrap() > 0.0);
        assert!(st2["totals"]["total"].as_f64().unwrap() < st["totals"]["total"].as_f64().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_input_is_refused() {
        let mut s = Session::new();
        job(&mut s);
        assert!(s.execute("estimate.import", &json!({ "folder": "/no/such/folder" })).is_err());
        assert!(s.execute("estimate.set_scope", &json!({ "trade": "electrical", "scope": "maybe", "reason": "x" })).is_err());
        assert!(s.execute("estimate.set_scope", &json!({ "trade": "electrical", "scope": "nic" })).is_err());
        assert!(s.execute("estimate.set_scope", &json!({ "scope": "nic", "reason": "x" })).is_err());
        assert!(s.execute("estimate.set_assembly", &json!({ "trade": "electrical", "assembly": "NOPE", "reason": "x" })).is_err());
        assert!(s.execute("estimate.set_price", &json!({ "item": "X", "unit": "each", "material": 1, "date": "2026-01-01" })).is_err());
        assert!(s.execute("estimate.settings", &json!({ "tax_pct": -1 })).is_err());
        assert!(s.execute("estimate.settings", &json!({ "today": "yesterday" })).is_err());
        assert!(s.execute("estimate.explain", &json!({})).is_err());
        assert_eq!(csv_rows("a,b\n\"x, y\",\"q\"\"r\"\n").first().and_then(|r| r.get("a")).map(String::as_str), Some("x, y"));
        assert_eq!(csv_rows("item,size\nEMT,3/4\"\n").first().and_then(|r| r.get("size")).map(String::as_str), Some("3/4\""));
        assert!(parse_date("2026-13-01").is_none() && parse_date("2026-10-21").is_some());
    }
}
