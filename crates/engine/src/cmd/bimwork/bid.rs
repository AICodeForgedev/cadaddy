//! `bim.bid` (Q6196): the bid package a contractor sends a general contractor, from the model.
//!
//! - **Lines:** the takeoff by level and branch circuit (`bim.takeoff`'s lines, each with its
//!   elements), of the trades the bid covers (rules data: `bid_rules.json`), and the underground
//!   plan's runs (`ugsheets.check`: the ones the documents call in the base bid, the ones CADaddy
//!   PROPOSED priced apart).
//! - **Systems:** each line's system (power, lighting, fire alarm, low voltage, site and
//!   underground, gear) by the rules' words for its item and category; raceway, wire and boxes
//!   take the system of the devices on their circuit. Every line says why.
//! - **Material:** the price-book workbook (`price_book`, the Web Prices layout `bim.estimate`
//!   reads), else the price book on this machine, by catalog number then the Item Map; each
//!   line keeps its quote (key, vendor, date, source type). Never priced at zero: an unpriced
//!   line is listed with why.
//! - **Labor:** hours per unit from labor-unit rules data with their source (`labor_units`: a
//!   company's table per item, size or category; else the estimating baseline `estimate.import`
//!   kept; else the starter book's placeholder hours, marked as such), never dollars divided by a
//!   rate. A line no table covers has no labor and is listed.
//! - **Totals:** by system and by floor (material, labor hours, labor $, allowances, direct, and
//!   each one's share of the bid to the cent); allowances, tax on material, overhead, profit,
//!   contingency and bond as their own lines.
//! - **Alternates:** each value-engineering option `ve.study` proposes, with its add or deduct.
//! - **Scope:** inclusions, exclusions (work by others from the scope split, other trades, items
//!   the price book excludes), clarifications (PROPOSED work priced apart, open RFIs, open review
//!   items, unpriced lines, lines with no labor unit) and assumptions, each with its source.
//! - **Exports:** `file` (.xlsx in the Web Prices layout: the priced takeoff with live INDEX/MATCH
//!   formulas into its Price List and Item Map, a Bid Summary whose rates are editable cells,
//!   Alternates, Allowances, Scope and Quotes) and `letter` (.pdf: a one-page proposal, CADaddy
//!   branding only; a person's or company's name only as the user entered it).

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::priced::{self, Outcome};
use crate::cmd::pricebook::{self, Book, Chosen};
use crate::cmd::table::xlsx::Cell;
use crate::cmd::{bad, bool_or, str_param};
use crate::{Result, Session};

const CMD: &str = "bim.bid";
const RULES: &str = include_str!("bid_rules.json");
/// Lines in one bid at most; allowances; labor-unit rows; listed items per list.
const MAX_LINES: usize = 50_000;
const MAX_ALLOWANCES: usize = 200;
const MAX_LABOR_ROWS: usize = 50_000;
const MAX_LIST: usize = 200;
/// The longest name or note kept from the parameters.
const MAX_TEXT: usize = 200;
/// The state key the bid's inputs are kept under (bimwork.json).
const STATE_KEY: &str = "bid";
const TAKEOFF: &str = "Priced Takeoff";
const SUMMARY: &str = "Bid Summary";

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct System {
    id: String,
    name: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Rule {
    system: String,
    categories: Vec<String>,
    words: Vec<String>,
    words_none: Vec<String>,
    line_systems: Vec<String>,
    sizes: Vec<String>,
    no_circuit: bool,
    why: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Letter {
    title: String,
    scope: String,
    footer: String,
    full_lists: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Rules {
    trades: Vec<String>,
    systems: Vec<System>,
    default_system: String,
    underground_system: String,
    allowance_system: String,
    inherit: Vec<String>,
    rules: Vec<Rule>,
    letter: Letter,
}

fn rules() -> Rules {
    serde_json::from_str(RULES).unwrap_or_default()
}

pub fn specs() -> Vec<crate::cmd::CommandSpec> {
    vec![
        crate::cmd::CommandSpec::new(CMD, "Bid Package", run)
            .menu(&["Tools", "Building", "Bid Package"])
            .params("{price_book?: .xlsx (the Web Prices workbook: Price List + Item Map; default this machine's price book), labor_units?: .csv|.xlsx (item|kind, category?, size?, unit, hours, per? (1), source: hours per unit with their source; else the estimating baseline, else the starter book's placeholder hours), labor_rate?: $/h (else the estimate's crew rate), markups?: {tax_pct? (on material), overhead_pct?, profit_pct?, contingency_pct?, bond_pct?} (else the estimate's settings), allowances?: [{name, amount, system?, note?}], trades?: [trade] (default the rules': electrical), alternates?: bool (default true: ve.study's options with their add/deduct), ve?: {ve.study params}, underground?: bool (default true), project?, client?, bidder?: text the user entered (the letter names no one else), today?: YYYY-MM-DD, stale_days?, keep?: bool (keep the inputs for the next run; default true), show_lines?: bool, file?: .xlsx, letter?: .pdf, force?: bool} → {base_bid: {material, tax, labor_hours, labor_rate, labor, allowances, direct, overhead, profit, contingency, bond, total}, markups, by_system: [{system, name, lines, line_ids, material, labor_hours, labor, allowances, direct, share}], by_floor: [{level, lines, material, labor_hours, labor, direct, share}], quotes: [{key, description, vendor, source_type, date, unit, unit_price, lines, extended, stale}], allowances, alternates: [{id, name, tier, rfi, decision, material, labor_hours, labor, add_deduct, basis}], proposed: {lines, material, labor_hours, total}, inclusions, exclusions, clarifications, assumptions, counts, unpriced, no_labor, warnings, lines?, text, path?, letter?}: the bid package, every amount traced to its takeoff lines")
            .noundo(),
    ]
}

// ---------------- words ----------------

fn norm(t: &str) -> String {
    t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Does `hay` hold the phrase `w` starting at a word boundary (and ending at one when `whole`)?
fn has_word(hay: &str, w: &str, whole: bool) -> bool {
    let w = w.to_lowercase();
    if w.trim().is_empty() {
        return false;
    }
    let h = hay.to_lowercase();
    let mut from = 0;
    while let Some(i) = h.get(from..).and_then(|x| x.find(&w)) {
        let at = from + i;
        let before = h.get(..at).and_then(|x| x.chars().last());
        let after = h.get(at + w.len()..).and_then(|x| x.chars().next());
        if before.is_none_or(|c| !c.is_alphanumeric()) && (!whole || after.is_none_or(|c| !c.is_alphanumeric())) {
            return true;
        }
        from = at + w.len().max(1);
        if from >= h.len() {
            break;
        }
    }
    false
}

/// A size as compared: `#2/0` = `2/0`, spaces dropped.
fn size_key(t: &str) -> String {
    t.trim().trim_start_matches('#').chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase()
}

/// The circuit's own ids: `A-21,19,1` is A-21, A-19 and A-1.
fn circuit_parts(c: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut prefix = String::new();
    for (i, part) in c.split(',').map(str::trim).filter(|x| !x.is_empty()).take(64).enumerate() {
        if i == 0 {
            prefix = part.rsplit_once('-').map(|(p, _)| format!("{p}-")).unwrap_or_default();
            out.push(part.to_string());
        } else if part.contains('-') || prefix.is_empty() {
            out.push(part.to_string());
        } else {
            out.push(format!("{prefix}{part}"));
        }
    }
    out
}

/// The line's system by the rules' words, and why.
fn classify(r: &Rules, l: &Value) -> Option<(String, String)> {
    let s = |k: &str| l[k].as_str().unwrap_or_default();
    let (item, category, size, line_system) = (s("item"), s("category"), s("size"), s("system").replace(|c: char| !c.is_alphanumeric(), " "));
    let on_circuit = !s("circuit").is_empty();
    for rule in &r.rules {
        if !rule.categories.is_empty() && !rule.categories.iter().any(|c| norm(c) == norm(category)) {
            continue;
        }
        if rule.no_circuit && on_circuit {
            continue;
        }
        if !rule.sizes.is_empty() && !rule.sizes.iter().any(|z| size_key(z) == size_key(size)) {
            continue;
        }
        if !rule.words.is_empty() && !rule.words.iter().any(|w| has_word(item, w, false)) {
            continue;
        }
        if rule.words_none.iter().any(|w| has_word(item, w, false)) {
            continue;
        }
        if !rule.line_systems.is_empty() && !rule.line_systems.iter().any(|w| has_word(&line_system, w, true)) {
            continue;
        }
        return Some((rule.system.clone(), rule.why.clone()));
    }
    None
}

// ---------------- inputs ----------------

#[derive(Clone, Debug, Default)]
struct Allowance {
    name: String,
    amount: f64,
    system: String,
    note: String,
}

#[derive(Clone, Debug, Default)]
struct Markups {
    tax: f64,
    overhead: f64,
    profit: f64,
    contingency: f64,
    bond: f64,
}

impl Markups {
    fn factor(&self) -> f64 {
        (1.0 + self.overhead / 100.0) * (1.0 + self.profit / 100.0) * (1.0 + self.contingency / 100.0) * (1.0 + self.bond / 100.0)
    }
    fn json(&self) -> Value {
        json!({"tax_pct": self.tax, "overhead_pct": self.overhead, "profit_pct": self.profit, "contingency_pct": self.contingency, "bond_pct": self.bond})
    }
}

fn pct(cmd: &str, v: Option<&Value>, k: &str, default: f64) -> Result<f64> {
    match v.and_then(|m| m.get(k)) {
        None | Some(Value::Null) => Ok(default),
        Some(x) => {
            x.as_f64().filter(|t| t.is_finite() && (0.0..=100.0).contains(t)).ok_or_else(|| bad(cmd, format!("`markups.{k}` is a percent, 0 to 100")))
        }
    }
}

fn text_of(v: &Value) -> String {
    v.as_str().map(|t| t.trim().chars().take(MAX_TEXT).collect()).unwrap_or_default()
}

fn allowances(r: &Rules, v: &Value) -> Result<Vec<Allowance>> {
    let Some(a) = v.as_array() else {
        return if v.is_null() { Ok(Vec::new()) } else { Err(bad(CMD, "`allowances` is a list of {name, amount, system?, note?}")) };
    };
    if a.len() > MAX_ALLOWANCES {
        return Err(bad(CMD, format!("at most {MAX_ALLOWANCES} allowances")));
    }
    let mut out = Vec::new();
    for x in a {
        let name = text_of(&x["name"]);
        let amount = x["amount"].as_f64().filter(|n| n.is_finite() && n.abs() <= 1e10);
        let (Some(amount), false) = (amount, name.is_empty()) else {
            return Err(bad(CMD, "each allowance has a `name` and an `amount` in dollars"));
        };
        let system = text_of(&x["system"]);
        let system = if system.is_empty() { r.allowance_system.clone() } else { system };
        if !r.systems.iter().any(|s| s.id == system) {
            let ids: Vec<&str> = r.systems.iter().map(|s| s.id.as_str()).collect();
            return Err(bad(CMD, format!("allowance {name}: system {system} is not one of {}", ids.join(", "))));
        }
        out.push(Allowance { name, amount, system, note: text_of(&x["note"]) });
    }
    Ok(out)
}

/// One labor-unit row: hours per `per` units of an item (and size) or of a whole category.
#[derive(Clone, Debug)]
struct LaborRow {
    item: String,
    category: String,
    size: String,
    unit: String,
    hours: f64,
    source: String,
}

/// The labor-unit table: `.csv` or `.xlsx` (first sheet), a header row naming the columns. Rows
/// with no source are not used (they are counted).
fn labor_table(path: &str) -> Result<(Vec<LaborRow>, usize)> {
    let bytes = crate::readfile::read(path).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
    let rows: Vec<BTreeMap<String, String>> = if bytes.starts_with(b"PK") {
        let wb = crate::cmd::table::xlsx::open(&bytes).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        let cells = wb.values(None, None).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        let Some(head) = cells.first() else { return Ok((Vec::new(), 0)) };
        let head: Vec<String> = head.iter().map(|h| norm(h)).collect();
        cells.iter().skip(1).map(|r| head.iter().cloned().zip(r.iter().cloned()).collect()).collect()
    } else {
        crate::cmd::estimate::csv_rows(&String::from_utf8_lossy(&bytes))
            .into_iter()
            .map(|r| r.into_iter().map(|(k, v)| (norm(&k), v)).collect())
            .collect()
    };
    let get = |r: &BTreeMap<String, String>, ks: &[&str]| ks.iter().find_map(|k| r.get(*k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()));
    let mut out = Vec::new();
    let mut unsourced = 0usize;
    for r in rows.iter().take(MAX_LABOR_ROWS) {
        let hours = get(r, &["hours", "labor_hours", "hours per unit", "labor hours", "manhours", "man-hours"]).and_then(|v| v.parse::<f64>().ok());
        let per = get(r, &["per", "per qty", "per units"]).and_then(|v| v.parse::<f64>().ok()).unwrap_or(1.0);
        let (Some(hours), true) = (hours.filter(|h| h.is_finite() && *h >= 0.0 && *h <= 1e6), per.is_finite() && per > 0.0) else { continue };
        let Some(source) = get(r, &["source", "citation", "basis"]) else {
            unsourced += 1;
            continue;
        };
        out.push(LaborRow {
            item: norm(&get(r, &["item", "kind", "atom", "description"]).unwrap_or_default()),
            category: norm(&get(r, &["category"]).unwrap_or_default()),
            size: norm(&get(r, &["size"]).unwrap_or_default()),
            unit: get(r, &["unit"]).unwrap_or_else(|| "each".into()),
            hours: hours / per,
            source,
        });
    }
    Ok((out, unsourced))
}

/// The hours per unit of a line, and where they come from.
fn labor_unit(table: &[LaborRow], baseline: &Value, starter: &[Value], l: &Value) -> Option<(f64, String)> {
    let s = |k: &str| l[k].as_str().unwrap_or_default();
    let (item, size, unit, category) = (norm(s("item")), norm(s("size")), s("unit"), norm(s("category")));
    let ok = |r: &&LaborRow| pricebook::same_unit(&r.unit, unit);
    let row = table
        .iter()
        .filter(ok)
        .find(|r| !r.item.is_empty() && r.item == item && r.size == size)
        .or_else(|| table.iter().filter(ok).find(|r| !r.item.is_empty() && r.item == item && r.size.is_empty()))
        .or_else(|| {
            table.iter().filter(ok).find(|r| (r.item.is_empty() || r.item == "*") && r.category == category && (r.size.is_empty() || r.size == size))
        });
    if let Some(r) = row {
        return Some((r.hours, format!("labor units: {}", r.source)));
    }
    if let Some(h) =
        baseline["items"].as_object().and_then(|m| m.iter().find(|(k, _)| norm(k) == item)).and_then(|(_, v)| v["labor"]["normal"].as_f64())
    {
        return Some((h, "estimating baseline (estimate.import)".into()));
    }
    let same = |p: &&Value, sz: &str| norm(p["item"].as_str().unwrap_or_default()) == item && norm(p["size"].as_str().unwrap_or_default()) == sz;
    starter
        .iter()
        .filter(|p| pricebook::same_unit(p["unit"].as_str().unwrap_or_default(), unit))
        .find(|p| same(p, &size))
        .or_else(|| starter.iter().filter(|p| pricebook::same_unit(p["unit"].as_str().unwrap_or_default(), unit)).find(|p| same(p, "")))
        .and_then(|p| p["labor_hours"].as_f64())
        .map(|h| (h, format!("{} (placeholder hours: replace with your own)", crate::cmd::estimate::STARTER_SOURCE)))
}

// ---------------- the lines ----------------

/// Where a line goes in the bid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bid {
    Base,
    Proposed,
    Excluded,
}

impl Bid {
    fn name(self) -> &'static str {
        match self {
            Bid::Base => "BASE",
            Bid::Proposed => "PROPOSED",
            Bid::Excluded => "EXCLUDED",
        }
    }
}

#[derive(Clone, Debug)]
struct Line {
    id: usize,
    src: Value,
    system: String,
    system_why: String,
    bid: Bid,
    qty: f64,
    waste_pct: f64,
    bought: f64,
    parts: Vec<Chosen>,
    matched_by: &'static str,
    unit_price: Option<f64>,
    material: f64,
    reason: String,
    labor_unit: Option<f64>,
    labor_source: String,
    labor_hours: f64,
}

impl Line {
    fn s(&self, k: &str) -> &str {
        self.src[k].as_str().unwrap_or_default()
    }
    fn name(&self) -> String {
        let size = self.s("size");
        if size.is_empty() { self.s("item").to_string() } else { format!("{} {size}", self.s("item")) }
    }
    fn stale(&self, today: &str, stale_days: i64) -> bool {
        self.parts.iter().any(|c| pricebook::days_between(&c.quote.date, today).is_none_or(|a| a > stale_days))
    }
    fn json(&self, rate: f64) -> Value {
        json!({
            "line": self.id, "bid": self.bid.name(), "system": self.system, "system_why": self.system_why,
            "level": self.src["level"], "circuit": self.src["circuit"], "trade": self.src["trade"], "category": self.src["category"],
            "item": self.src["item"], "size": self.src["size"], "unit": self.src["unit"], "qty": r3(self.qty), "waste_pct": r3(self.waste_pct),
            "qty_bought": r3(self.bought), "status": if self.unit_price.is_some() { "priced" } else if self.bid == Bid::Excluded { "excluded" } else { "unpriced" },
            "matched_by": self.matched_by, "price_keys": self.parts.iter().map(|c| c.quote.key.clone()).collect::<Vec<_>>(),
            "unit_price": self.unit_price.map(|v| (v * 1e4).round() / 1e4), "material": cents(self.material),
            "labor_unit_hours": self.labor_unit.map(|h| (h * 1e5).round() / 1e5), "labor_source": self.labor_source,
            "labor_hours": r3(self.labor_hours), "labor": cents(self.labor_hours * rate),
            "vendor": self.parts.iter().map(|c| c.quote.vendor.as_str()).filter(|v| !v.is_empty()).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>().join(" + "),
            "price_date": self.parts.iter().map(|c| c.quote.date.clone()).min().unwrap_or_default(),
            "reason": self.reason, "elements": self.src["element_count"], "source": self.src["source"],
        })
    }
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn cents(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// The underground plan's runs as lines: the called ones in the base bid, the PROPOSED apart.
fn underground_lines(s: &mut Session, r: &Rules) -> Vec<Value> {
    let Ok(ug) = s.execute("ugsheets.check", &json!({})) else { return Vec::new() };
    let mut out = Vec::new();
    for l in ug["takeoff"]["E"]["lines"].as_array().into_iter().flatten().take(MAX_LIST) {
        let (material, size) = (l["material"].as_str().unwrap_or_default(), l["size"].as_str().unwrap_or_default());
        let proposed = l["status"] == "proposed";
        let source =
            if proposed { "underground plan: PROPOSED by CADaddy, not called on the documents" } else { "underground plan: called on the documents" };
        let base = json!({"level": "underground", "trade": "electrical", "system": r.underground_system, "size": size, "status": l["status"], "proposed": proposed, "source": source, "element_count": 0});
        if let Some(ft) = l["ft"].as_f64().filter(|f| *f > 0.0) {
            let mut x = base.clone();
            x["category"] = json!("conduit");
            x["item"] = json!(format!("{material} conduit"));
            x["unit"] = json!("ft");
            x["qty"] = json!(ft);
            out.push(x);
        }
        if let Some(n) = l["elbows_90"].as_f64().filter(|n| *n > 0.0) {
            let mut x = base;
            x["category"] = json!("conduit fittings");
            x["item"] = json!(format!("{material} 90° elbow"));
            x["unit"] = json!("each");
            x["qty"] = json!(n);
            out.push(x);
        }
    }
    out
}

/// Split `target` cents over `raw` dollar amounts, the rounding cents to the largest remainders.
fn apportion(raw: &[f64], target: i64) -> Vec<i64> {
    let mut c: Vec<i64> = raw.iter().map(|v| (v * 100.0).floor() as i64).collect();
    let mut order: Vec<usize> = (0..raw.len()).collect();
    let frac = |i: usize| raw.get(i).map_or(0.0, |v| v * 100.0 - (v * 100.0).floor());
    order.sort_by(|a, b| frac(*b).total_cmp(&frac(*a)));
    let mut left = target - c.iter().sum::<i64>();
    let n = order.len();
    let mut k = 0usize;
    while left != 0 && n > 0 && k < 1_000_000 {
        let at = if left > 0 { order.get(k % n) } else { order.get(n - 1 - k % n) };
        if let Some(x) = at.and_then(|i| c.get_mut(*i)) {
            *x += left.signum();
            left -= left.signum();
        }
        k += 1;
    }
    c
}

// ---------------- the command ----------------

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    crate::cmd::exchange::guard_params(s, CMD, p, &["file", "letter"])?;
    let r = rules();
    let kept = super::state(s.doc()?)[STATE_KEY].clone();
    let given = |k: &str| p.get(k).filter(|v| !v.is_null()).or_else(|| kept.get(k).filter(|v| !v.is_null())).cloned().unwrap_or(Value::Null);
    // The estimate's settings are the defaults of the rates.
    let est = crate::cmd::estimate::state(s, CMD).map(|(_, st)| st).unwrap_or_default();
    let est_setting = |k: &str| crate::cmd::estimate::setting(&est, k, 0.0);
    let mk = given("markups");
    if !mk.is_null() && !mk.is_object() {
        return Err(bad(CMD, "`markups` is {tax_pct?, overhead_pct?, profit_pct?, contingency_pct?, bond_pct?}"));
    }
    let m = Markups {
        tax: pct(CMD, Some(&mk), "tax_pct", est_setting("tax_pct"))?,
        overhead: pct(CMD, Some(&mk), "overhead_pct", est_setting("overhead_pct"))?,
        profit: pct(CMD, Some(&mk), "profit_pct", est_setting("profit_pct"))?,
        contingency: pct(CMD, Some(&mk), "contingency_pct", est_setting("contingency_pct"))?,
        bond: pct(CMD, Some(&mk), "bond_pct", est_setting("bond_pct"))?,
    };
    let (rate, rate_from) = match given("labor_rate") {
        Value::Null if est_setting("crew_rate") > 0.0 => (est_setting("crew_rate"), "the estimate's crew rate (estimate.settings)".to_string()),
        Value::Null => (0.0, String::new()),
        v => (
            v.as_f64()
                .filter(|x| x.is_finite() && (0.0..=10_000.0).contains(x))
                .ok_or_else(|| bad(CMD, "`labor_rate` is dollars per hour, 0 to 10000"))?,
            "entered".to_string(),
        ),
    };
    let allow = allowances(&r, &given("allowances"))?;
    let trades: Vec<String> = match p.get("trades").and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(Value::as_str).map(norm).take(20).collect(),
        None => r.trades.iter().map(|t| norm(t)).collect(),
    };
    if trades.is_empty() {
        return Err(bad(CMD, "`trades` names at least one trade"));
    }
    let names: BTreeMap<&str, String> = ["project", "client", "bidder"].into_iter().map(|k| (k, text_of(&given(k)))).collect();
    let stale_days = match p.get("stale_days") {
        None | Some(Value::Null) => 90,
        Some(v) => v.as_i64().filter(|n| (1..=36_500).contains(n)).ok_or_else(|| bad(CMD, "`stale_days` is a whole number of days, 1 or more"))?,
    };
    let today = match str_param(p, "today").map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => {
            crate::cmd::estimate::parse_date(t).map(|_| t.chars().take(10).collect::<String>()).ok_or_else(|| bad(CMD, "`today` is YYYY-MM-DD"))?
        }
        None => crate::cmd::coordination::iso_date().chars().take(10).collect(),
    };
    // The price book.
    let (book, book_name) = match str_param(p, "price_book").map(str::trim).filter(|x| !x.is_empty()) {
        Some(path) => {
            let bytes = crate::readfile::read(path).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
            if !bytes.starts_with(b"PK") {
                return Err(bad(CMD, format!("{path}: a price book is an .xlsx workbook (sheets \"Price List\" and \"Item Map\")")));
            }
            let b = pricebook::workbook_book(&bytes, &today, stale_days).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
            let name = std::path::Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().to_string());
            (b, format!("the price book {name}"))
        }
        None => (pricebook::load(None), "the price book on this machine (shipped and local quotes)".to_string()),
    };
    let by_catalog = priced::catalog_index(&book);
    let mut warnings: Vec<Value> = Vec::new();
    let (table, unsourced) = match str_param(p, "labor_units").map(str::trim).filter(|x| !x.is_empty()) {
        Some(path) => labor_table(path)?,
        None => (Vec::new(), 0),
    };
    if unsourced > 0 {
        warnings.push(json!({"kind": "labor_units_unsourced", "message": format!("{unsourced} labor-unit rows have no source: not used"), "fix": "give each row its source (the labor table, page, edition)"}));
    }
    let baseline = est.get("baseline").cloned().unwrap_or(Value::Null);
    let starter = crate::cmd::estimate::starter_prices();

    // The takeoff, by level and circuit.
    let all = super::takeoff::lines(s, "circuit", &Value::Null)?;
    let mut other_trades: BTreeMap<String, usize> = BTreeMap::new();
    let mut src: Vec<Value> = Vec::new();
    for l in all.iter().take(MAX_LINES) {
        let t = norm(l["trade"].as_str().unwrap_or_default());
        if trades.contains(&t) {
            src.push(l.clone());
        } else if l["qty"].as_f64().is_some_and(|q| q > 0.0) {
            *other_trades.entry(t).or_default() += 1;
        }
    }
    if bool_or(p, "underground", true) {
        src.extend(underground_lines(s, &r));
    }
    // Systems: the rules, then the circuit's devices for raceway, wire and boxes.
    let inherit = |c: &str| r.inherit.iter().any(|x| norm(x) == norm(c));
    // Each circuit's devices are the spec's tags on it (a luminaire's BOM line names no circuit).
    let mut circuit_sys: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    let classified: Vec<Option<(String, String)>> = src.iter().map(|l| classify(&r, l)).collect();
    if let Ok((sp, _)) = crate::cmd::bspec::current(s, CMD) {
        for t in sp.tags.iter().take(MAX_LINES) {
            let Some(c) = t.fields.get("circuit").map(|v| v.value.clone()).filter(|c| !c.trim().is_empty()) else { continue };
            if matches!(t.kind.as_str(), "circuit" | "homerun" | "conduit") {
                continue;
            }
            let what = t.fields.get("device_type").map_or_else(|| t.kind.replace('_', " "), |v| format!("{} {}", t.kind.replace('_', " "), v.value));
            if let Some((sys, _)) = classify(&r, &json!({"item": what, "category": "devices"})) {
                for part in circuit_parts(&c) {
                    *circuit_sys.entry(part).or_default().entry(sys.clone()).or_default() += 1.0;
                }
            }
        }
    }
    let order = |sys: &str| r.systems.iter().position(|x| x.id == sys).unwrap_or(usize::MAX);
    let mut lines: Vec<Line> = Vec::new();
    for (l, c) in src.into_iter().zip(classified) {
        let category = l["category"].as_str().unwrap_or_default().to_string();
        let circuit = l["circuit"].as_str().unwrap_or_default().to_string();
        let (system, why) = match c {
            _ if l["proposed"].is_boolean() => (r.underground_system.clone(), "an underground run (the underground plan)".to_string()),
            Some((sys, why)) if !inherit(&category) => (sys, why),
            c => {
                let mut tally: BTreeMap<String, f64> = BTreeMap::new();
                for part in circuit_parts(&circuit) {
                    for (sys, q) in circuit_sys.get(&part).into_iter().flatten() {
                        *tally.entry(sys.clone()).or_default() += q;
                    }
                }
                let best = tally.iter().max_by(|a, b| a.1.total_cmp(b.1).then(order(b.0).cmp(&order(a.0)))).map(|(k, _)| k.clone());
                match (best, c) {
                    (Some(sys), _) => {
                        let name = r.systems.iter().find(|x| x.id == sys).map_or(sys.as_str(), |x| x.name.as_str()).to_lowercase();
                        (sys.clone(), format!("on circuit {circuit}, whose devices are {name}"))
                    }
                    (None, Some((sys, why))) => (sys, why),
                    (None, None) => (r.default_system.clone(), "no rule names it and no device on its circuit says otherwise".to_string()),
                }
            }
        };
        let (qty, waste_pct, bought) = priced::bought(&l);
        if qty <= 0.0 {
            continue;
        }
        let proposed = l["proposed"].as_bool() == Some(true);
        let mut line = Line {
            id: lines.len() + 1,
            src: l,
            system,
            system_why: why,
            bid: if proposed { Bid::Proposed } else { Bid::Base },
            qty,
            waste_pct,
            bought,
            parts: Vec::new(),
            matched_by: "",
            unit_price: None,
            material: 0.0,
            reason: String::new(),
            labor_unit: None,
            labor_source: String::new(),
            labor_hours: 0.0,
        };
        match priced::price(&book, &by_catalog, &line.src) {
            Outcome::Priced { by, parts } => {
                let u: f64 = parts.iter().map(|c| c.quote.unit_price).sum();
                line.unit_price = Some(u);
                line.material = u * bought;
                line.matched_by = if by == "catalog" { "catalog" } else { "item_map" };
                line.parts = parts;
            }
            Outcome::Excluded(why) => {
                line.bid = Bid::Excluded;
                line.reason = why;
            }
            Outcome::Unpriced(why) => line.reason = why,
        }
        if line.bid != Bid::Excluded
            && let Some((h, from)) = labor_unit(&table, &baseline, &starter, &line.src)
        {
            line.labor_unit = Some(h);
            line.labor_source = from;
            // Labor on the net quantity (waste isn't installed).
            line.labor_hours = h * qty;
        }
        lines.push(line);
    }

    if lines.is_empty() && allow.is_empty() {
        return Err(crate::EngineError::Other(format!(
            "{CMD}: nothing to bid: the model has no takeoff lines of {} (build the model first: model.build or the job)",
            trades.join(", ")
        )));
    }
    // ---- totals ----
    let base: Vec<&Line> = lines.iter().filter(|l| l.bid == Bid::Base).collect();
    let material: f64 = base.iter().map(|l| l.material).sum();
    let labor_hours: f64 = base.iter().map(|l| l.labor_hours).sum();
    let labor = labor_hours * rate;
    let allowances_total: f64 = allow.iter().map(|a| a.amount).sum();
    let tax = material * m.tax / 100.0;
    let direct = material + tax + labor + allowances_total;
    let overhead = direct * m.overhead / 100.0;
    let profit = (direct + overhead) * m.profit / 100.0;
    let contingency = (direct + overhead + profit) * m.contingency / 100.0;
    let bond = (direct + overhead + profit + contingency) * m.bond / 100.0;
    let total = direct + overhead + profit + contingency + bond;
    let f = m.factor();
    let total_cents = (total * 100.0).round() as i64;
    // By system: each system's direct cost and its share of the bid (to the cent, summing to it).
    let mut sys_rows: Vec<(String, String, Vec<usize>, f64, f64, f64)> =
        r.systems.iter().map(|x| (x.id.clone(), x.name.clone(), Vec::new(), 0.0, 0.0, 0.0)).collect();
    for l in &base {
        if let Some(row) = sys_rows.iter_mut().find(|x| x.0 == l.system) {
            row.2.push(l.id);
            row.3 += l.material;
            row.4 += l.labor_hours;
        }
    }
    for a in &allow {
        if let Some(row) = sys_rows.iter_mut().find(|x| x.0 == a.system) {
            row.5 += a.amount;
        }
    }
    let sys_direct: Vec<f64> = sys_rows.iter().map(|x| x.3 * (1.0 + m.tax / 100.0) + x.4 * rate + x.5).collect();
    let sys_share = apportion(&sys_direct.iter().map(|d| d * f).collect::<Vec<_>>(), total_cents);
    let by_system: Vec<Value> = sys_rows
        .iter()
        .zip(&sys_direct)
        .zip(&sys_share)
        .filter(|(((_, _, ids, _, _, a), _), _)| !ids.is_empty() || *a != 0.0)
        .map(|(((id, name, ids, mat, h, a), d), sh)| {
            json!({"system": id, "name": name, "lines": ids.len(), "line_ids": ids.iter().take(5_000).collect::<Vec<_>>(), "material": cents(*mat),
                   "labor_hours": r3(*h), "labor": cents(h * rate), "allowances": cents(*a), "direct": cents(*d), "share": *sh as f64 / 100.0})
        })
        .collect();
    // By floor: the lines' levels (allowances are the job's).
    let mut floors: BTreeMap<String, (usize, f64, f64)> = BTreeMap::new();
    for l in &base {
        let lv = l.s("level");
        let e = floors.entry(if lv.is_empty() { "job-wide".to_string() } else { lv.to_string() }).or_default();
        e.0 += 1;
        e.1 += l.material;
        e.2 += l.labor_hours;
    }
    if allowances_total != 0.0 {
        floors.entry("allowances".to_string()).or_default();
    }
    let floor_direct: Vec<f64> =
        floors.iter().map(|(k, (_, mat, h))| if k == "allowances" { allowances_total } else { mat * (1.0 + m.tax / 100.0) + h * rate }).collect();
    let floor_share = apportion(&floor_direct.iter().map(|d| d * f).collect::<Vec<_>>(), total_cents);
    let by_floor: Vec<Value> = floors
        .iter()
        .zip(&floor_direct)
        .zip(&floor_share)
        .map(|(((lv, (n, mat, h)), d), sh)| {
            json!({"level": lv, "lines": n, "material": cents(*mat), "labor_hours": r3(*h), "labor": cents(h * rate), "direct": cents(*d), "share": *sh as f64 / 100.0})
        })
        .collect();
    // Quotes used.
    let mut quotes: BTreeMap<String, (Chosen, usize, f64)> = BTreeMap::new();
    for l in lines.iter().filter(|l| l.bid != Bid::Excluded) {
        for c in &l.parts {
            let e = quotes.entry(c.quote.key.clone()).or_insert_with(|| (c.clone(), 0, 0.0));
            e.1 += 1;
            e.2 += c.quote.unit_price * l.bought;
        }
    }
    let quotes_json: Vec<Value> = quotes
        .values()
        .map(|(c, n, x)| {
            let q = &c.quote;
            json!({"key": q.key, "description": q.description, "vendor": q.vendor, "source_type": q.source_type, "date": q.date, "unit": q.unit,
                   "unit_price": q.unit_price, "lines": n, "extended": cents(*x), "stale": pricebook::days_between(&q.date, &today).is_none_or(|a| a > stale_days)})
        })
        .collect();
    // PROPOSED work, priced apart.
    let prop: Vec<&Line> = lines.iter().filter(|l| l.bid == Bid::Proposed).collect();
    let prop_mat: f64 = prop.iter().map(|l| l.material).sum();
    let prop_h: f64 = prop.iter().map(|l| l.labor_hours).sum();
    let prop_total = (prop_mat * (1.0 + m.tax / 100.0) + prop_h * rate) * f;

    // ---- alternates ----
    let mut alternates: Vec<Value> = Vec::new();
    let mut alt_note = String::new();
    if bool_or(p, "alternates", true) {
        let mut vp = p.get("ve").filter(|v| v.is_object()).cloned().unwrap_or_else(|| json!({}));
        vp["model"] = json!(false);
        match s.execute("ve.study", &vp) {
            Ok(v) => {
                for sc in v["scenarios"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|x| matches!(x["tier"].as_str(), Some("allowed_now" | "needs_rfi")))
                    .take(MAX_LIST)
                {
                    let sv = &sc["saves"];
                    let tier = sc["tier"].as_str().unwrap_or_default();
                    let mut a = json!({"id": sc["id"], "name": sc["name"], "tier": tier, "rfi": sc["rfi"]["id"].as_str().or(sc["rfi"].as_str()).unwrap_or_default(),
                        "decision": sc["decision"], "basis": "ve.study: both ways priced from the routed takeoff"});
                    if sv.is_object() {
                        let (dm, dl, dh) = (
                            sv["material"].as_f64().unwrap_or(0.0),
                            sv["labor_dollars"].as_f64().unwrap_or(0.0),
                            sv["man_hours"].as_f64().unwrap_or(0.0),
                        );
                        // Savings are the design less the alternative: the bid changes by minus that, marked up.
                        a["material"] = json!(cents(-dm));
                        a["labor_hours"] = json!(r3(-dh));
                        a["labor"] = json!(cents(-dl));
                        a["add_deduct"] = json!(cents(-(dm * (1.0 + m.tax / 100.0) + dl) * f));
                        let unpriced = sc["unpriced"].as_array().map_or(0, Vec::len);
                        if unpriced > 0 {
                            a["unpriced_items"] = json!(unpriced);
                        }
                    } else {
                        a["add_deduct"] = Value::Null;
                        let unpriced = sc["unpriced"].as_array().map_or(0, Vec::len);
                        a["basis"] = json!(if unpriced > 0 {
                            format!("not priced both ways: {unpriced} items have no price (give ve: {{cost_db}} or price them)")
                        } else {
                            "not priced: no routed circuit or feeder of the model is in its areas".to_string()
                        });
                    }
                    alternates.push(a);
                }
            }
            Err(e) => alt_note = format!("value engineering not studied: {e}"),
        }
    }

    // ---- scope ----
    let sys_name = |id: &str| r.systems.iter().find(|x| x.id == id).map_or_else(|| id.to_string(), |x| x.name.clone());
    let money = |v: f64| {
        let c = (v.abs() * 100.0).round() as i64;
        let (d, cc) = (c / 100, c % 100);
        let mut ds = d.to_string();
        let mut i = ds.len() as i64 - 3;
        while i > 0 {
            ds.insert(i as usize, ',');
            i -= 3;
        }
        format!("{}${ds}.{cc:02}", if v < 0.0 { "-" } else { "" })
    };
    let entry = |text: String, source: &str| json!({"text": text, "source": source});
    let mut inclusions: Vec<Value> = Vec::new();
    for x in &by_system {
        if x["lines"].as_u64().unwrap_or(0) == 0 {
            continue;
        }
        let ids: Vec<usize> = x["line_ids"].as_array().into_iter().flatten().filter_map(|v| v.as_u64().map(|n| n as usize)).collect();
        let mut tops: Vec<&Line> = lines.iter().filter(|l| ids.binary_search(&l.id).is_ok()).collect();
        tops.sort_by(|a, b| b.material.total_cmp(&a.material).then(b.qty.total_cmp(&a.qty)));
        let mut seen = BTreeSet::new();
        let named: Vec<String> = tops.iter().map(|l| l.s("item").to_string()).filter(|n| seen.insert(n.clone())).take(6).collect();
        inclusions.push(entry(
            format!(
                "{}: {} takeoff lines ({}{}).",
                x["name"].as_str().unwrap_or_default(),
                x["lines"],
                named.join(", "),
                if seen.len() > 6 { ", …" } else { "" }
            ),
            "the takeoff (sheet Priced Takeoff, column System)",
        ));
    }
    for a in &allow {
        inclusions.push(entry(
            format!("Allowance: {} {}{}.", a.name, money(a.amount), if a.note.is_empty() { String::new() } else { format!(" ({})", a.note) }),
            "entered",
        ));
    }
    let bom = s.execute("bim.bom", &json!({})).unwrap_or(Value::Null);
    let mut by_others: BTreeMap<(String, String), (usize, String, Vec<String>, String)> = BTreeMap::new();
    for x in bom["by_others"].as_array().into_iter().flatten().take(MAX_LINES) {
        let kind = x["kind"].as_str().unwrap_or_default().replace('_', " ");
        let owner = x["owner"].as_str().or(x["scope"].as_str()).unwrap_or_default().to_string();
        let e =
            by_others.entry((kind, owner)).or_insert_with(|| (0, x["source"].as_str().unwrap_or_default().to_string(), Vec::new(), String::new()));
        e.0 += 1;
        if e.2.is_empty() {
            e.2 = x["ours"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
            e.3 = x["ours_source"].as_str().unwrap_or_default().to_string();
        }
    }
    let mut exclusions: Vec<Value> = Vec::new();
    for ((kind, owner), (n, source, ours, ours_source)) in &by_others {
        exclusions.push(entry(format!("{kind} ({n}): by the {owner}."), source));
        if !ours.is_empty() {
            inclusions.push(entry(format!("Rough-in only for {kind} ({n}): {}.", ours.join(", ")), ours_source));
        }
    }
    if !other_trades.is_empty() {
        let list: Vec<String> = other_trades.iter().map(|(t, n)| format!("{t} ({n} lines)")).collect();
        exclusions.push(entry(format!("Work of other trades in the model: {}.", list.join(", ")), "the takeoff's trades outside this bid"));
    }
    let mut ex_reasons: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for l in lines.iter().filter(|l| l.bid == Bid::Excluded) {
        ex_reasons.entry(l.reason.clone()).or_default().push(l.name());
    }
    for (why, items) in &ex_reasons {
        let mut v = items.clone();
        v.dedup();
        exclusions.push(entry(
            format!(
                "{}{}: {why}.",
                v.iter().take(8).cloned().collect::<Vec<_>>().join(", "),
                if v.len() > 8 { format!(" and {} more", v.len() - 8) } else { String::new() }
            ),
            "the price book's Item Map",
        ));
    }
    let mut clarifications: Vec<Value> = Vec::new();
    if !prop.is_empty() {
        let items: Vec<String> = prop.iter().map(|l| format!("{} {} {}", l.name(), r3(l.qty), l.s("unit"))).collect();
        let no_price = prop.iter().filter(|l| l.unit_price.is_none()).count();
        let priced_text = if no_price == 0 {
            money(prop_total)
        } else {
            format!(
                "{} for its priced lines ({no_price} of {} lines have no price: sheet Priced Takeoff, Bid PROPOSED)",
                money(prop_total),
                prop.len()
            )
        };
        clarifications.push(entry(
            format!(
                "PROPOSED work not called on the documents is priced apart, not in the base bid: {priced_text}; {}; confirm by RFI.",
                items.join("; ")
            ),
            "the underground plan's proposed runs (ugsheets.check)",
        ));
    }
    if let Ok(v) = s.execute("rfi.list", &json!({})) {
        let open: Vec<&Value> =
            v["items"].as_array().into_iter().flatten().filter(|x| matches!(x["status"].as_str(), Some("open" | "sent") | None)).collect();
        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        for x in &open {
            *kinds.entry(x["kind"].as_str().unwrap_or("other").to_string()).or_default() += 1;
        }
        if !open.is_empty() {
            let k: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} {n}")).collect();
            clarifications.push(entry(
                format!("{} open RFIs ({}): the bid assumes what each records as assumed until it is answered.", open.len(), k.join(", ")),
                "rfi.list",
            ));
        }
        for x in open.iter().filter(|x| x["severity"] == "high").take(20) {
            clarifications.push(entry(
                format!(
                    "RFI {}: {}{}",
                    x["id"].as_str().unwrap_or_default(),
                    x["title"].as_str().unwrap_or_default(),
                    x["assumed"].as_str().filter(|a| !a.is_empty()).map(|a| format!(" (assumed: {a})")).unwrap_or_default()
                ),
                &format!(
                    "rfi.list, sheets {}",
                    x["sheets"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
                ),
            ));
        }
    }
    if let Ok(v) = s.execute("review.list", &json!({})) {
        let mut k: Vec<String> = Vec::new();
        for (kind, n) in v["byKind"].as_object().into_iter().flatten() {
            if kind != "rfi" {
                k.push(format!("{kind} {n}"));
            }
        }
        if !k.is_empty() {
            clarifications.push(entry(format!("Open review items ({}): quantities they touch may change.", k.join(", ")), "review.list"));
        }
        for x in v["items"].as_array().into_iter().flatten().filter(|x| matches!(x["kind"].as_str(), Some("question" | "conflict"))).take(10) {
            clarifications
                .push(entry(format!("Review {}: {}", x["id"].as_str().unwrap_or_default(), x["title"].as_str().unwrap_or_default()), "review.list"));
        }
    }
    let unpriced: Vec<&Line> = lines.iter().filter(|l| l.bid != Bid::Excluded && l.unit_price.is_none()).collect();
    if !unpriced.is_empty() {
        clarifications.push(entry(
            format!(
                "{} takeoff lines have no material price and their material is not in the bid (sheet Priced Takeoff, Status UNPRICED).",
                unpriced.len()
            ),
            &book_name,
        ));
    }
    let no_labor: Vec<&Line> = lines.iter().filter(|l| l.bid != Bid::Excluded && l.labor_unit.is_none()).collect();
    if !no_labor.is_empty() {
        clarifications.push(entry(
            format!(
                "{} takeoff lines have no labor unit and their labor is not in the bid: give them a row in the labor-unit table.",
                no_labor.len()
            ),
            "labor-unit rules data",
        ));
    }
    if !alt_note.is_empty() {
        clarifications.push(entry(format!("No alternates: {alt_note}."), "ve.study"));
    }
    // Assumptions.
    let mut assumptions: Vec<Value> = Vec::new();
    let by_cat = lines.iter().filter(|l| l.matched_by == "catalog").count();
    let by_map = lines.iter().filter(|l| l.matched_by == "item_map").count();
    let vendors: BTreeSet<&str> = quotes.values().map(|q| q.0.quote.vendor.as_str()).filter(|v| !v.is_empty()).collect();
    assumptions.push(entry(
        format!("Material priced from {book_name} as of {today}: {by_cat} lines by catalog number, {by_map} by the Item Map, {} quotes from {} vendors (sheet Quotes).", quotes.len(), vendors.len()),
        &book_name,
    ));
    let stale_n = lines.iter().filter(|l| l.bid != Bid::Excluded && !l.parts.is_empty() && l.stale(&today, stale_days)).count();
    if stale_n > 0 {
        assumptions.push(entry(
            format!("{stale_n} lines are priced from quotes older than {stale_days} days: confirm them before the bid date."),
            &book_name,
        ));
        warnings.push(json!({"kind": "stale_prices", "message": format!("{stale_n} lines priced from quotes older than {stale_days} days"), "fix": "update those Price List rows' prices and dates"}));
    }
    let mut lab_sources: BTreeMap<&str, usize> = BTreeMap::new();
    for l in lines.iter().filter(|l| l.labor_unit.is_some()) {
        *lab_sources.entry(l.labor_source.as_str()).or_default() += 1;
    }
    for (src_name, n) in &lab_sources {
        assumptions.push(entry(format!("Labor hours on {n} lines from {src_name}."), src_name));
    }
    if lab_sources.keys().any(|k| k.contains("placeholder")) {
        warnings.push(json!({"kind": "placeholder_labor", "message": "some labor comes from the starter book's placeholder hours", "fix": "give labor_units: your company's labor-unit table with its source"}));
    }
    if rate > 0.0 {
        assumptions.push(entry(format!("Labor at {}/h ({rate_from}).", money(rate)), &rate_from));
    } else {
        warnings.push(json!({"kind": "no_labor_rate", "message": "no labor rate: labor hours are counted but not priced", "fix": "give labor_rate (or estimate.settings crew_rate)"}));
    }
    let rule_lines = base.iter().filter(|l| l.s("status") == "rule").count();
    if rule_lines > 0 {
        assumptions.push(entry(
            format!(
                "{rule_lines} quantities come from takeoff rules (boxes per device, fittings per run, wire makeup), not counted on the drawings."
            ),
            "bim.bom rules",
        ));
    }
    assumptions.push(entry(
        "Material quantities include the takeoff's waste allowance by category (column Qty bought); labor is on the installed quantity.".into(),
        "bim.bom waste",
    ));
    if m.tax > 0.0 {
        assumptions.push(entry(format!("Sales tax {}% on material.", m.tax), "markups"));
    }
    let defaulted = base.iter().filter(|l| l.system_why.starts_with("no rule")).count();
    if defaulted > 0 {
        assumptions.push(entry(
            format!(
                "{defaulted} lines put in {} by default: no rule names them and no device on their circuit says otherwise.",
                sys_name(&r.default_system)
            ),
            "bid_rules.json",
        ));
    }

    let base_bid = json!({"material": cents(material), "tax": cents(tax), "labor_hours": r3(labor_hours), "labor_rate": rate, "labor": cents(labor),
        "allowances": cents(allowances_total), "direct": cents(direct), "overhead": cents(overhead), "profit": cents(profit), "contingency": cents(contingency),
        "bond": cents(bond), "total": total_cents as f64 / 100.0});
    let list = |v: &[&Line]| {
        v.iter().take(MAX_LIST).map(|l| json!({"line": l.id, "item": l.s("item"), "size": l.s("size"), "level": l.s("level"), "qty": r3(l.qty), "unit": l.s("unit"), "reason": l.reason})).collect::<Vec<_>>()
    };
    let mut text = format!("# Bid summary\n\nBase bid: {}\n\n## By system\n", money(total_cents as f64 / 100.0));
    for x in &by_system {
        text += &format!("- {}: {} ({} lines)\n", x["name"].as_str().unwrap_or_default(), money(x["share"].as_f64().unwrap_or(0.0)), x["lines"]);
    }
    text += "\n## By floor\n";
    for x in &by_floor {
        text += &format!("- {}: {}\n", x["level"].as_str().unwrap_or_default(), money(x["share"].as_f64().unwrap_or(0.0)));
    }
    if !alternates.is_empty() {
        text += "\n## Alternates (not in the base bid)\n";
        for a in &alternates {
            let v = a["add_deduct"].as_f64();
            text += &format!(
                "- {} ({}): {}\n",
                a["name"].as_str().unwrap_or_default(),
                a["tier"].as_str().unwrap_or_default().replace('_', " "),
                v.map_or("not priced".to_string(), |v| if v <= 0.0 { format!("deduct {}", money(-v)) } else { format!("add {}", money(v)) })
            );
        }
    }
    for (h, v) in [("Inclusions", &inclusions), ("Exclusions", &exclusions), ("Clarifications", &clarifications), ("Assumptions", &assumptions)] {
        if !v.is_empty() {
            text += &format!("\n## {h}\n");
            for x in v.iter() {
                text += &format!("- {}\n", x["text"].as_str().unwrap_or_default());
            }
        }
    }
    let mut out = json!({
        "base_bid": base_bid, "markups": m.json(), "by_system": by_system, "by_floor": by_floor, "quotes": quotes_json,
        "allowances": allow.iter().map(|a| json!({"name": a.name, "amount": a.amount, "system": a.system, "note": a.note})).collect::<Vec<_>>(),
        "alternates": alternates,
        "proposed": {"lines": prop.iter().map(|l| l.id).collect::<Vec<_>>(), "material": cents(prop_mat), "labor_hours": r3(prop_h), "total": cents(prop_total)},
        "inclusions": inclusions, "exclusions": exclusions, "clarifications": clarifications, "assumptions": assumptions,
        "counts": {"lines": lines.len(), "base": base.len(), "proposed": prop.len(), "excluded": lines.iter().filter(|l| l.bid == Bid::Excluded).count(),
                   "priced": lines.iter().filter(|l| l.unit_price.is_some()).count(), "unpriced": unpriced.len(), "no_labor": no_labor.len(), "stale": stale_n},
        "unpriced": list(&unpriced), "no_labor": list(&no_labor), "warnings": warnings, "today": today, "text": text,
    });
    if bool_or(p, "show_lines", false) {
        out["lines"] = json!(lines.iter().map(|l| l.json(rate)).collect::<Vec<_>>());
    }
    // Keep the inputs for the next run.
    if bool_or(p, "keep", true) {
        let mut st = Map::new();
        for k in ["markups", "labor_rate", "allowances", "project", "client", "bidder"] {
            let v = given(k);
            if !v.is_null() {
                st.insert(k.into(), v);
            }
        }
        if !st.is_empty() {
            super::set_state(s, STATE_KEY, Value::Object(st))?;
        }
    }
    let file = str_param(p, "file").map(str::trim).filter(|x| !x.is_empty()).map(String::from);
    let letter_path = str_param(p, "letter").map(str::trim).filter(|x| !x.is_empty()).map(String::from);
    for path in file.iter().chain(letter_path.iter()) {
        crate::cmd::exchange::guard_output(s, CMD, p, path)?;
    }
    let (file_bytes, letter_bytes) = {
        let ctx = Ctx { r: &r, lines: &lines, m: &m, rate, allow: &allow, out: &out, book: &book, names: &names, today: &today, stale_days };
        let fb = match &file {
            Some(_) => Some(crate::cmd::table::xlsx::write_cells(&workbook(&ctx)).map_err(|e| bad(CMD, e))?),
            None => None,
        };
        let lb = match &letter_path {
            Some(_) => Some(letter(&ctx).map_err(|e| bad(CMD, e))?),
            None => None,
        };
        (fb, lb)
    };
    for (path, bytes, key) in [(&file, file_bytes, "path"), (&letter_path, letter_bytes, "letter")] {
        if let (Some(path), Some(bytes)) = (path, bytes) {
            #[cfg(not(target_arch = "wasm32"))]
            crate::writefile::write(path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
            #[cfg(target_arch = "wasm32")]
            let _ = bytes;
            out[key] = json!(path);
        }
    }
    Ok(out)
}

/// What the exports read.
struct Ctx<'a> {
    r: &'a Rules,
    lines: &'a [Line],
    m: &'a Markups,
    rate: f64,
    allow: &'a [Allowance],
    out: &'a Value,
    book: &'a Book,
    names: &'a BTreeMap<&'a str, String>,
    today: &'a str,
    stale_days: i64,
}

// ---------------- the workbook ----------------

/// Columns of the priced takeoff (0-based positions the formulas use).
const COLS: [&str; 30] = [
    "Line",
    "Bid",
    "System",
    "Level",
    "Circuit",
    "Trade",
    "Category",
    "Item",
    "Size",
    "Type mark",
    "Catalog",
    "Unit",
    "Qty",
    "Waste %",
    "Qty bought",
    "Status",
    "Price key 1",
    "Price key 2",
    "Price key 3",
    "Unit $",
    "Material $",
    "Labor h/unit",
    "Labor source",
    "Labor h",
    "Labor $",
    "Price date",
    "Vendor",
    "Stale",
    "Elements",
    "Why",
];
const C_BID: usize = 1;
const C_SYS: usize = 2;
const C_LEVEL: usize = 3;
const C_QTY: usize = 12;
const C_BOUGHT: usize = 14;
const C_KEY: usize = 16;
const C_UNIT: usize = 19;
const C_MAT: usize = 20;
const C_LUNIT: usize = 21;
const C_LH: usize = 23;
/// The Bid Summary's input cells: labor rate, tax, overhead, profit, contingency, bond.
const IN_RATE: &str = "$B$3";
const IN_TAX: &str = "$B$4";
const IN_OH: &str = "$B$5";
const IN_PROFIT: &str = "$B$6";
const IN_CONT: &str = "$B$7";
const IN_BOND: &str = "$B$8";

fn workbook(c: &Ctx) -> Vec<(String, Vec<Vec<Cell>>)> {
    let col = priced::col;
    let ps = priced::PRICE_SHEET;
    let t = |v: &str| Cell::text(v.to_string());
    let rate_ref = format!("'{SUMMARY}'!{IN_RATE}");
    // ---- the priced takeoff ----
    let mut tk: Vec<Vec<Cell>> = vec![COLS.iter().map(|h| t(h)).collect()];
    for (i, l) in c.lines.iter().enumerate() {
        let r = i + 2;
        let keys: Vec<String> = l.parts.iter().map(|p| p.quote.key.clone()).collect();
        let look = |k: usize| format!("INDEX('{ps}'!$E:$E,MATCH(${}{r},'{ps}'!$A:$A,0))", col(C_KEY + k));
        let unit_f = format!(
            "IF(${}{r}=\"\",\"\",{}{})",
            col(C_KEY),
            look(0),
            (1..3).map(|k| format!("+IF(${}{r}=\"\",0,{})", col(C_KEY + k), look(k))).collect::<String>()
        );
        let (u, b) = (col(C_UNIT), col(C_BOUGHT));
        let mat_f = format!("IF(${u}{r}=\"\",\"\",${b}{r}*${u}{r})");
        let lh_f = format!("IF(${}{r}=\"\",0,${}{r}*${}{r})", col(C_LUNIT), col(C_QTY), col(C_LUNIT));
        let ld_f = format!("${}{r}*{rate_ref}", col(C_LH));
        let mut row = vec![
            Cell::Number(l.id as f64),
            t(l.bid.name()),
            t(&l.system),
            t(l.s("level")),
            t(l.s("circuit")),
            t(l.s("trade")),
            t(l.s("category")),
            t(l.s("item")),
            t(l.s("size")),
            t(l.s("type_mark")),
            t(l.s("catalog")),
            t(l.s("unit")),
            Cell::Number(r3(l.qty)),
            Cell::Number(r3(l.waste_pct)),
            Cell::Number(r3(l.bought)),
            t(if l.unit_price.is_some() {
                "PRICED"
            } else if l.bid == Bid::Excluded {
                "EXCLUDED"
            } else {
                "UNPRICED"
            }),
        ];
        row.extend((0..3).map(|k| keys.get(k).map_or(Cell::Empty, |x| Cell::text(x.clone()))));
        row.push(match l.unit_price {
            Some(v) => Cell::Formula(unit_f, v),
            None => Cell::FormulaText(unit_f, String::new()),
        });
        row.push(match l.unit_price {
            Some(_) => Cell::Formula(mat_f, l.material),
            None => Cell::FormulaText(mat_f, String::new()),
        });
        row.push(l.labor_unit.map_or(Cell::Empty, Cell::Number));
        row.push(t(&l.labor_source));
        row.push(Cell::Formula(lh_f, l.labor_hours));
        row.push(Cell::Formula(ld_f, l.labor_hours * c.rate));
        row.push(t(&l.parts.iter().map(|p| p.quote.date.clone()).min().unwrap_or_default()));
        row.push(t(&l
            .parts
            .iter()
            .map(|p| p.quote.vendor.as_str())
            .filter(|v| !v.is_empty())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(" + ")));
        row.push(t(if !l.parts.is_empty() && l.stale(c.today, c.stale_days) { "STALE" } else { "" }));
        row.push(Cell::Number(l.src["element_count"].as_f64().unwrap_or(0.0)));
        row.push(t(&if l.reason.is_empty() { l.system_why.clone() } else { format!("{}; {}", l.reason, l.system_why) }));
        tk.push(row);
    }
    // ---- the summary: inputs, systems, totals, floors ----
    let tkr = |cidx: usize| format!("'{TAKEOFF}'!${0}:${0}", col(cidx));
    let sumifs = |what: usize, by: usize, crit: &str, bid: &str| format!("SUMIFS({},{},{crit},{},\"{bid}\")", tkr(what), tkr(by), tkr(C_BID));
    let bb = &c.out["base_bid"];
    let n = |k: &str| bb[k].as_f64().unwrap_or(0.0);
    let mut sm: Vec<Vec<Cell>> = vec![vec![t("Bid summary"), t(c.names.get("project").map_or("", String::as_str)), t(&format!("as of {}", c.today))]];
    sm.push(vec![t("Inputs (edit the blue column B)"), Cell::Empty]);
    sm.push(vec![t("Labor rate $/h"), Cell::Number(c.rate)]);
    sm.push(vec![t("Tax % (on material)"), Cell::Number(c.m.tax)]);
    sm.push(vec![t("Overhead %"), Cell::Number(c.m.overhead)]);
    sm.push(vec![t("Profit %"), Cell::Number(c.m.profit)]);
    sm.push(vec![t("Contingency %"), Cell::Number(c.m.contingency)]);
    sm.push(vec![t("Bond %"), Cell::Number(c.m.bond)]);
    sm.push(Vec::new());
    sm.push(["System", "Name", "Lines", "Material $", "Labor h", "Labor $", "Allowances $", "Direct $"].iter().map(|h| t(h)).collect());
    let first = sm.len() + 1;
    for x in c.out["by_system"].as_array().into_iter().flatten() {
        let r = sm.len() + 1;
        let id = x["system"].as_str().unwrap_or_default();
        sm.push(vec![
            t(id),
            t(x["name"].as_str().unwrap_or_default()),
            Cell::Number(x["lines"].as_f64().unwrap_or(0.0)),
            Cell::Formula(sumifs(C_MAT, C_SYS, &format!("$A{r}"), "BASE"), x["material"].as_f64().unwrap_or(0.0)),
            Cell::Formula(sumifs(C_LH, C_SYS, &format!("$A{r}"), "BASE"), x["labor_hours"].as_f64().unwrap_or(0.0)),
            Cell::Formula(format!("E{r}*{IN_RATE}"), x["labor"].as_f64().unwrap_or(0.0)),
            Cell::Formula(format!("SUMIFS(Allowances!$C:$C,Allowances!$B:$B,$A{r})"), x["allowances"].as_f64().unwrap_or(0.0)),
            Cell::Formula(format!("D{r}*(1+{IN_TAX}/100)+F{r}+G{r}"), x["direct"].as_f64().unwrap_or(0.0)),
        ]);
    }
    let last = sm.len();
    sm.push(Vec::new());
    let tot = |label: &str, f: String, v: f64| vec![t(label), Cell::Empty, Cell::Empty, Cell::Formula(f, v)];
    let rr = sm.len() + 1;
    sm.push(tot("Material", format!("SUM(D{first}:D{last})"), n("material")));
    sm.push(tot("Tax", format!("D{rr}*{IN_TAX}/100"), n("tax")));
    sm.push(tot("Labor h", format!("SUM(E{first}:E{last})"), n("labor_hours")));
    sm.push(tot("Labor $", format!("D{}*{IN_RATE}", rr + 2), n("labor")));
    sm.push(tot("Allowances", format!("SUM(G{first}:G{last})"), n("allowances")));
    sm.push(tot("Direct cost", format!("D{rr}+D{}+D{}+D{}", rr + 1, rr + 3, rr + 4), n("direct")));
    sm.push(tot("Overhead", format!("D{}*{IN_OH}/100", rr + 5), n("overhead")));
    sm.push(tot("Profit", format!("(D{}+D{})*{IN_PROFIT}/100", rr + 5, rr + 6), n("profit")));
    sm.push(tot("Contingency", format!("(D{}+D{}+D{})*{IN_CONT}/100", rr + 5, rr + 6, rr + 7), n("contingency")));
    sm.push(tot("Bond", format!("(D{}+D{}+D{}+D{})*{IN_BOND}/100", rr + 5, rr + 6, rr + 7, rr + 8), n("bond")));
    sm.push(tot("BASE BID", format!("SUM(D{}:D{})", rr + 5, rr + 9), n("total")));
    let pr = &c.out["proposed"];
    sm.push(vec![
        t("PROPOSED work, priced apart (not in the base bid)"),
        Cell::Empty,
        Cell::Empty,
        Cell::Formula(
            format!(
                "({}*(1+{IN_TAX}/100)+{}*{IN_RATE})*(1+{IN_OH}/100)*(1+{IN_PROFIT}/100)*(1+{IN_CONT}/100)*(1+{IN_BOND}/100)",
                sumifs(C_MAT, C_BID, "\"PROPOSED\"", "PROPOSED"),
                sumifs(C_LH, C_BID, "\"PROPOSED\"", "PROPOSED")
            ),
            pr["total"].as_f64().unwrap_or(0.0),
        ),
    ]);
    sm.push(Vec::new());
    sm.push(["Floor", "Lines", "", "Material $", "Labor h", "Labor $", "", "Direct $"].iter().map(|h| t(h)).collect());
    for x in c.out["by_floor"].as_array().into_iter().flatten() {
        let r = sm.len() + 1;
        let lv = x["level"].as_str().unwrap_or_default();
        if lv == "allowances" {
            sm.push(vec![
                t(lv),
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                Cell::Empty,
                Cell::Formula("SUM(Allowances!$C:$C)".into(), x["direct"].as_f64().unwrap_or(0.0)),
            ]);
            continue;
        }
        let crit = if lv == "job-wide" { "\"\"".to_string() } else { format!("$A{r}") };
        sm.push(vec![
            t(lv),
            Cell::Number(x["lines"].as_f64().unwrap_or(0.0)),
            Cell::Empty,
            Cell::Formula(sumifs(C_MAT, C_LEVEL, &crit, "BASE"), x["material"].as_f64().unwrap_or(0.0)),
            Cell::Formula(sumifs(C_LH, C_LEVEL, &crit, "BASE"), x["labor_hours"].as_f64().unwrap_or(0.0)),
            Cell::Formula(format!("E{r}*{IN_RATE}"), x["labor"].as_f64().unwrap_or(0.0)),
            Cell::Empty,
            Cell::Formula(format!("D{r}*(1+{IN_TAX}/100)+F{r}"), x["direct"].as_f64().unwrap_or(0.0)),
        ]);
    }
    // ---- alternates, allowances, scope, quotes ----
    let factor = format!("(1+{0}!{IN_OH}/100)*(1+{0}!{IN_PROFIT}/100)*(1+{0}!{IN_CONT}/100)*(1+{0}!{IN_BOND}/100)", format_args!("'{SUMMARY}'"));
    let mut alt: Vec<Vec<Cell>> = vec![
        ["ID", "Name", "Tier", "RFI", "Decision", "Material $", "Labor h", "Labor $", "Add (+) / deduct (-) $", "Basis"]
            .iter()
            .map(|h| t(h))
            .collect(),
    ];
    for a in c.out["alternates"].as_array().into_iter().flatten() {
        let r = alt.len() + 1;
        let s = |k: &str| a[k].as_str().unwrap_or_default().to_string();
        let num = |k: &str| a[k].as_f64().map_or(Cell::Empty, Cell::Number);
        alt.push(vec![
            t(&s("id")),
            t(&s("name")),
            t(&s("tier")),
            t(&s("rfi")),
            t(&s("decision")),
            num("material"),
            num("labor_hours"),
            num("labor"),
            match a["add_deduct"].as_f64() {
                Some(v) => Cell::Formula(format!("(F{r}*(1+'{SUMMARY}'!{IN_TAX}/100)+H{r})*{factor}"), v),
                None => t("not priced"),
            },
            t(&s("basis")),
        ]);
    }
    let mut al: Vec<Vec<Cell>> = vec![["Name", "System", "Amount $", "Note"].iter().map(|h| t(h)).collect()];
    for a in c.allow {
        al.push(vec![t(&a.name), t(&a.system), Cell::Number(a.amount), t(&a.note)]);
    }
    let mut sc: Vec<Vec<Cell>> = vec![["Section", "Item", "Source"].iter().map(|h| t(h)).collect()];
    for (h, k) in [("Inclusion", "inclusions"), ("Exclusion", "exclusions"), ("Clarification", "clarifications"), ("Assumption", "assumptions")] {
        for x in c.out[k].as_array().into_iter().flatten() {
            sc.push(vec![t(h), t(x["text"].as_str().unwrap_or_default()), t(x["source"].as_str().unwrap_or_default())]);
        }
    }
    let mut qs: Vec<Vec<Cell>> = vec![
        ["Key", "Description", "Vendor", "Source type", "Price date", "Unit", "Unit $", "Lines", "Extended $", "Stale"]
            .iter()
            .map(|h| t(h))
            .collect(),
    ];
    for q in c.out["quotes"].as_array().into_iter().flatten() {
        let s = |k: &str| q[k].as_str().unwrap_or_default().to_string();
        qs.push(vec![
            t(&s("key")),
            t(&s("description")),
            t(&s("vendor")),
            t(&s("source_type")),
            t(&s("date")),
            t(&s("unit")),
            Cell::Number(q["unit_price"].as_f64().unwrap_or(0.0)),
            Cell::Number(q["lines"].as_f64().unwrap_or(0.0)),
            Cell::Number(q["extended"].as_f64().unwrap_or(0.0)),
            t(if q["stale"] == true { "STALE" } else { "" }),
        ]);
    }
    // The price book's own two sheets (the formulas' lookups), as bim.estimate writes them.
    let book_sheets = priced::workbook(c.book, &[], &BTreeMap::new(), c.m.tax);
    let mut sheets = vec![
        (SUMMARY.to_string(), sm),
        (TAKEOFF.to_string(), tk),
        ("Alternates".to_string(), alt),
        ("Allowances".to_string(), al),
        ("Scope".to_string(), sc),
        ("Quotes".to_string(), qs),
    ];
    sheets.extend(book_sheets.into_iter().filter(|(n, _)| n == ps || n == "Item Map"));
    let _ = c.r;
    sheets
}

// ---------------- the letter ----------------

/// A one-page proposal letter (letter size, portrait), CADaddy's branding only.
fn letter(c: &Ctx) -> std::result::Result<Vec<u8>, String> {
    use crate::cmd::rfi::{line, text, wrap};
    use cadaddy_geom::Vec2;
    // Characters a row holds at each text height (the drafting font, 7" of text width).
    const WIDE: usize = 74;
    const NARROW: usize = 88;
    let mut d = cadaddy_doc::Drawing::new_imperial();
    let mut y = 10.3;
    let x = 0.75;
    let lt = &c.r.letter;
    let name = |k: &str| c.names.get(k).filter(|v| !v.is_empty()).cloned();
    text(&mut d, Vec2::new(x, y), 0.3, &lt.title);
    text(&mut d, Vec2::new(5.6, y + 0.05), 0.11, "CADaddy  |  AICodeForge");
    y -= 0.35;
    text(&mut d, Vec2::new(x, y), 0.11, &format!("Date: {}", c.today));
    y -= 0.2;
    text(&mut d, Vec2::new(x, y), 0.11, &format!("To: {}", name("client").unwrap_or_else(|| "____________________________".into())));
    y -= 0.2;
    text(&mut d, Vec2::new(x, y), 0.11, &format!("Project: {}", name("project").unwrap_or_else(|| "____________________________".into())));
    y -= 0.2;
    if let Some(b) = name("bidder") {
        text(&mut d, Vec2::new(x, y), 0.11, &format!("From: {b}"));
        y -= 0.2;
    }
    line(&mut d, Vec2::new(x, y), Vec2::new(7.75, y));
    y -= 0.3;
    let total = c.out["base_bid"]["total"].as_f64().unwrap_or(0.0);
    let money = |v: f64| {
        let cc = (v.abs() * 100.0).round() as i64;
        let mut ds = (cc / 100).to_string();
        let mut i = ds.len() as i64 - 3;
        while i > 0 {
            ds.insert(i as usize, ',');
            i -= 3;
        }
        format!("{}${ds}.{:02}", if v < 0.0 { "-" } else { "" }, cc % 100)
    };
    for l in wrap(&format!("{} {}.", lt.scope, money(total)), WIDE) {
        text(&mut d, Vec2::new(x, y), 0.12, &l);
        y -= 0.2;
    }
    // Sections, each line at most one row, until the page is full (the workbook has them all).
    let bottom = 2.1;
    let step = 0.17;
    let section = |d: &mut cadaddy_doc::Drawing, y: &mut f64, title: &str, rows: Vec<String>, most: usize| {
        if rows.is_empty() || *y - step * 2.0 < bottom {
            return;
        }
        *y -= 0.1;
        text(d, Vec2::new(x, *y), 0.12, title);
        *y -= 0.2;
        for r in rows.into_iter().take(most) {
            if *y < bottom {
                break;
            }
            let mut w = wrap(&r, NARROW - 4).into_iter();
            let first = w.next().unwrap_or_default();
            text(d, Vec2::new(x + 0.15, *y), 0.1, &if w.next().is_some() { format!("{first} ...") } else { first });
            *y -= step;
        }
    };
    let sys: Vec<String> = c.out["by_system"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| format!("{}: {}", s["name"].as_str().unwrap_or_default(), money(s["share"].as_f64().unwrap_or(0.0))))
        .collect();
    section(&mut d, &mut y, "Base bid by system", sys, 8);
    let alts: Vec<String> = c.out["alternates"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| {
            format!(
                "{} ({}): {}",
                a["name"].as_str().unwrap_or_default(),
                a["tier"].as_str().unwrap_or_default().replace('_', " "),
                match a["add_deduct"].as_f64() {
                    Some(v) if v <= 0.0 => format!("deduct {}", money(-v)),
                    Some(v) => format!("add {}", money(v)),
                    None => "price pending (see the bid workbook)".to_string(),
                }
            )
        })
        .collect();
    section(&mut d, &mut y, "Alternates (not in the base bid)", alts, 5);
    let pick = |k: &str| c.out[k].as_array().into_iter().flatten().filter_map(|x| x["text"].as_str().map(String::from)).collect::<Vec<_>>();
    section(&mut d, &mut y, "Includes", pick("inclusions"), 6);
    section(&mut d, &mut y, "Excludes", pick("exclusions"), 7);
    section(&mut d, &mut y, "Clarifications", pick("clarifications"), 6);
    let mut fy = 1.75;
    for l in wrap(&lt.full_lists, NARROW) {
        text(&mut d, Vec2::new(x, fy), 0.1, &l);
        fy -= 0.17;
    }
    fy -= 0.2;
    text(&mut d, Vec2::new(x, fy), 0.11, &format!("Submitted by: {}", name("bidder").unwrap_or_else(|| "________________".into())));
    text(&mut d, Vec2::new(4.2, fy), 0.11, "Accepted by: ______________ Date: ______");
    fy -= 0.4;
    line(&mut d, Vec2::new(x, fy + 0.15), Vec2::new(7.75, fy + 0.15));
    text(&mut d, Vec2::new(x, fy), 0.08, &lt.footer);
    let plot = crate::cmd::file::io().and_then(|h| h.plot).ok_or("PDF export isn't available in this build")?;
    let page = json!({"window": [[0.0, 0.0], [8.5, 11.0]], "paper": "Letter", "landscape": false});
    plot(&d, &cadaddy_doc::Space::Model, &json!({"pages": [page], "margins": [0.0, 0.0, 0.0, 0.0], "lineweights": false, "stamp": false}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_match_at_word_starts_only() {
        assert!(has_word("Duplex receptacle", "receptacle", false));
        assert!(!has_word("Device plates", "ats", false));
        assert!(!has_word("diameter", "meter", false));
        assert!(has_word("e bda", "bda", true));
        assert!(!has_word("e bdax", "bda", true));
        assert!(!has_word("", "x", false) && !has_word("abc", "  ", false));
    }

    #[test]
    fn a_homerun_names_each_of_its_circuits() {
        assert_eq!(circuit_parts("A-21,19,1"), ["A-21", "A-19", "A-1"]);
        assert_eq!(circuit_parts("LP1-3"), ["LP1-3"]);
        assert_eq!(circuit_parts("7, 9"), ["7", "9"]);
        assert!(circuit_parts("").is_empty());
    }

    #[test]
    fn the_rules_classify_by_the_items_words() {
        let r = rules();
        assert!(!r.systems.is_empty() && r.rules.iter().all(|x| r.systems.iter().any(|s| s.id == x.system)), "every rule's system is listed");
        let sys = |item: &str, category: &str, size: &str, circuit: &str| {
            classify(&r, &json!({"item": item, "category": category, "size": size, "circuit": circuit})).map(|x| x.0)
        };
        assert_eq!(sys("luminaire type X", "devices", "", "").as_deref(), Some("lighting"));
        assert_eq!(sys("switch, dimmer, 0-10V", "devices", "", "").as_deref(), Some("lighting"));
        assert_eq!(sys("duplex receptacle", "devices", "", "").as_deref(), Some("power"));
        assert_eq!(sys("smoke detector", "devices", "", "").as_deref(), Some("fire_alarm"));
        assert_eq!(sys("switchboard MSB", "equipment", "", "").as_deref(), Some("gear"));
        assert_eq!(sys("equipment connection RTU", "devices", "", "").as_deref(), Some("power"));
        assert_eq!(sys("THHN copper", "wire", "#4", "").as_deref(), Some("gear"), "a feeder size on no circuit");
        assert_eq!(sys("THHN copper", "wire", "#4", "X-1"), None, "on a circuit it takes the circuit's");
        assert_eq!(sys("THHN copper", "wire", "#12", ""), None);
        // No job's words in the rules.
        let text = RULES.to_ascii_uppercase();
        for w in ["HOTEL", "QUEEN", "KING", "SUITE", "GUEST"] {
            assert!(!text.contains(w), "{w}");
        }
    }

    #[test]
    fn shares_sum_to_the_total_to_the_cent() {
        let c = apportion(&[10.005, 20.004, 0.333], 3034);
        assert_eq!(c.iter().sum::<i64>(), 3034);
        assert!(apportion(&[], 0).is_empty());
    }
}
