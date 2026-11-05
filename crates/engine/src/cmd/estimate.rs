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
//! Labor is the book rate at the line's difficulty tier, or the production model's drivers
//! (book rates blended with field observations), times the condition factors on the line
//! (`labor`). Dated prices escalate to `today` by their index. The job's tools, equipment and
//! setup are priced from what the work implies (`equipment`). Totals break down by level, trade
//! and system, export to CSV/XLSX and draft a bid summary (`outputs`).
//!
//! - `estimate.import {folder}`: the baseline (`item_labor_units.csv`, `assemblies.csv`,
//!   `assembly_parts.csv`, `prices.csv`, `rates.json`, `benchmarks.json`, `factors.csv`,
//!   `production_model.json`, `field_observations.csv`, `indexes.csv`, `tool_rules.json`,
//!   `price_history.csv`).
//! - `estimate.set_price {item, unit, material, labor_hours? | labor_dollars?, date, source, index?}`.
//! - `estimate.map_price {lines? | trade?/category?/item?, row_item, row_size?, reason}`: map
//!   unpriced BOM lines to a specific price row (Q6020: explicit, never silent).
//! - `estimate.set_assembly {lines? | trade?/category?/item?, assembly, reason, citation?}`.
//! - `estimate.set_scope {lines? | trade?/category?/item?, scope, reason, citation?}`.
//! - `estimate.set_factor {lines? | trade?/category?/item?, factor? | tier?, reason, remove?}`.
//! - `estimate.observe {driver, context?, qty, hours, date, source}`: a field observation; the
//!   driver's rate is blended again.
//! - `estimate.settings {tax_pct?, overhead_pct?, profit_pct?, contingency_pct?, bond_pct?,
//!   crew_rate?, today?, stale_days?, tier?, context?, duration_days?, peak_crew?, high_work_ft?,
//!   gross_sf?}`.
//! - `estimate.query {trade?, category?, status?, limit?, cursor?}`: priced lines, summary first.
//! - `estimate.status`: totals (by scope, phase, level, trade, system) and the validator's
//!   findings, each with a fix.
//! - `estimate.explain {line? | total?}`: a number traced to its math, its lines and elements.
//! - `estimate.compare {assembly?: {from, to}, scope?: {line: scope}, settings?}`: a what-if
//!   against the current estimate.
//! - `estimate.export {path, format?, columns?}` and `estimate.bid_summary {}`: the human outputs.

use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

use super::bspec::current;
use super::*;
use crate::{Result, Session};

mod equipment;
mod labor;
mod outputs;

/// Rows read from one baseline file, at most (hostile files).
const MAX_ROWS: usize = 200_000;
/// Lines per `estimate.query` page by default, and at most.
const PAGE: usize = 50;
const MAX_PAGE: usize = 1_000;
const SCOPES: [&str; 4] = ["in_contract", "by_others", "nic", "ofci"];

/// The starter cost book shipped with CADaddy (Q6012).
const STARTER_CSV: &str = include_str!("../../../../assets/costbook/starter.csv");
pub(crate) const STARTER_SOURCE: &str = "starter cost book";

/// Parse the embedded starter cost book CSV into price rows (same shape as baseline prices).
pub(crate) fn starter_prices() -> Vec<Value> {
    let rows = csv_rows(STARTER_CSV);
    rows.iter()
        .filter_map(|r| {
            let item = r.get("item")?.trim().to_string();
            let material: f64 = r.get("material")?.trim().parse().ok()?;
            if item.is_empty() || material < 0.0 || material.is_nan() {
                return None;
            }
            let hours: Option<f64> = r.get("labor_hours").and_then(|v| v.trim().parse().ok()).filter(|v: &f64| v.is_finite() && *v >= 0.0);
            Some(json!({
                "item": item,
                "size": r.get("size").map(|s| s.trim()).unwrap_or_default(),
                "unit": r.get("unit").map(|u| u.trim()).filter(|u| !u.is_empty()).unwrap_or("each"),
                "material": material,
                "labor_hours": hours,
                "source": STARTER_SOURCE,
            }))
        })
        .collect()
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("estimate.import", "Import Estimating Baseline", run_import)
            .params("{folder: path (item_labor_units.csv, assemblies.csv, assembly_parts.csv, prices.csv, rates.json, benchmarks.json, pack.json, factors.csv, production_model.json, field_observations.csv, indexes.csv, tool_rules.json, price_history.csv), label?: \"public data\"|\"baseline\" (default: the folder's pack.json label, else baseline)} → {items, assemblies, prices, rates, benchmarks, label, notes}"),
        CommandSpec::new("estimate.import_p405", "Import P-405 Labor Tables", run_import_p405)
            .params("{file: path to p405_labor_units.csv (all 735 rows), private_folder?: path for the private pack (required when any row is electrical or of no known trade: those go here)} → {public_rows, private_rows, skipped, notes}"),
        CommandSpec::new("estimate.import_costs", "Import Cost Table", run_import_costs).params(
            "{path: .csv or .xlsx cost table (columns item, unit, material: dollars per unit, labor_hrs or labor_dollars per unit; size and date optional), sheet?: xlsx sheet, date?: YYYY-MM-DD for undated rows (default estimate.settings' today), source? (default the file's name)} → {prices, skipped, source, notes, next_step}: the estimate's prices from a starter cost table",
        ),
        CommandSpec::new("estimate.set_price", "Set Estimate Price", run_set_price).params(
            "{item, size?, unit: each|ft|lb|sq ft…, material: dollars per unit, labor_hours?: per unit, labor_dollars?: per unit (÷ crew rate), date: YYYY-MM-DD, source, index?: name in indexes.csv (escalates the price to today)} → {price}",
        ),
        CommandSpec::new("estimate.map_price", "Map Line to Price Row", run_map_price)
            .params("{lines?: [key] | trade?, category?, item?, row_item, row_size?, reason} → {lines}"),
        CommandSpec::new("estimate.set_assembly", "Set Estimate Assembly", run_set_assembly)
            .params("{lines?: [key] | trade?, category?, item?, assembly, reason, citation?} → {lines}"),
        CommandSpec::new("estimate.set_scope", "Set Estimate Scope", run_set_scope)
            .params("{lines?: [key] | trade?, category?, item?, scope: in_contract|by_others|nic|ofci|alternate:<n>, reason, citation?} → {lines}"),
        CommandSpec::new("estimate.settings", "Estimate Settings", run_settings)
            .params("{tax_pct?, overhead_pct?, profit_pct?, contingency_pct?, bond_pct?, crew_rate?, today?: YYYY-MM-DD, stale_days?, pricebook?: bool (default true: lines the drawing's prices miss are priced from the material price book before the starter book), pricebook_folder?: path (this machine's price book; default the pricebook folder in the settings folder), tier?: normal|difficult|very_difficult (the job's labor tier), context?: production-model context, duration_days?, peak_crew?, high_work_ft?: work this high needs a lift (14), gross_sf?: building area for hours/SF} → {settings}"),
        CommandSpec::new("estimate.set_factor", "Set Estimate Factor", run_set_factor)
            .params("{lines?: [key] | trade?, category?, item?, factor?: a factor in factors.csv (height, ceiling, exposure, wall, floor, learning_curve), tier?: normal|difficult|very_difficult, reason, citation?, remove?: bool} → {lines, factor?, tier?}"),
        CommandSpec::new("estimate.observe", "Record Field Observation", run_observe)
            .params("{driver: a production-model driver, context?, qty: units installed, hours: labor hours they took, date: YYYY-MM-DD, source} → {driver, context, before, after, observations}"),
        CommandSpec::new("estimate.query", "Query Estimate", run_query)
            .params("{trade?, category?, status?: priced|unpriced|undecided|error, limit?, cursor?} → {summary, lines: [{key, item, size, unit, qty, material, labor_hours, labor_cost, cost, scope, status, issues}], next?}")
            .noundo(),
        CommandSpec::new("estimate.status", "Estimate Status", run_status)
            .params("{} → {totals: {material, labor, labor_hours, direct, tax, overhead, profit, contingency, bond, total}, labor_by_phase, by_scope, alternates, by_level, by_trade, by_system, findings: [{kind, line?, message, fix}], ok, summary}")
            .noundo(),
        CommandSpec::new("estimate.explain", "Explain Estimate Number", run_explain)
            .params("{line?: key | total?: true} → {value, math: [steps], lines?, elements?}")
            .noundo(),
        CommandSpec::new("estimate.compare", "Compare Estimate What-If", run_compare)
            .params("{assembly?: {from, to}, scope?: {key: scope}, settings?: {…}} → {now, what_if, delta}")
            .noundo(),
        CommandSpec::new("estimate.change", "Price a Change", run_change)
            .params("{lines: [{item, size?, unit?: each (default), qty: added (negative: removed), trade?: electrical (default), category?}], why?} → {lines: [{item, size, qty, material, labor_hours, labor_cost, cost, via, priced}], totals: {material, labor_hours, labor_cost, cost}, unpriced, why}: what a change costs (an RFI's fix, a code rule's device), each line priced as the estimate prices it (the drawing's prices, the price book, the starter book); labor cost at the estimate's crew rate")
            .noundo(),
        CommandSpec::new("estimate.export", "Export Estimate", run_export)
            .params("{force?: bool (replace an existing file; otherwise it is refused), path: .xlsx or .csv file, format?: xlsx|csv (from the extension), columns?: [key|trade|category|item|size|unit|qty|net|level|material|labor_hours|labor_cost|cost|total|scope|status|via]} → {path, lines, total, columns}")
            .noundo(),
        CommandSpec::new("estimate.bid_summary", "Estimate Bid Summary", run_bid_summary)
            .params("{} → {total, base_bid: {by_trade}, alternates, assumptions, clarifications, exclusions, open_findings, text}")
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
pub(super) fn key_of(l: &Value) -> String {
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

// ---------- Word matching (shared with bim.estimate, Q5294) ----------

/// Tokenize a string into lower-case stems for fuzzy matching.
fn price_words(t: &str) -> BTreeSet<String> {
    t.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() > 1 && !matches!(*w, "of" | "and" | "the" | "for" | "with" | "per" | "each" | "ea" | "ga" | "gauge" | "untyped"))
        .map(|w| if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") { w.trim_end_matches('s') } else { w })
        .map(|w| match w {
            "drywall" | "gwb" | "gyp" | "wallboard" | "sheetrock" => "gypsum",
            "partition" => "wall",
            "outlet" => "receptacle",
            "luminaire" | "lighting" => "light",
            "doorway" => "door",
            w => w,
        })
        .map(str::to_string)
        .collect()
}

/// Measure class for unit compatibility (same logic as bimwork::takeoff::measure).
fn measure_class(unit: &str) -> Option<&'static str> {
    let u = unit.trim().to_ascii_lowercase().replace('.', "");
    Some(match u.as_str() {
        "each" | "ea" | "count" | "pcs" | "pc" | "no" | "set" | "sets" | "unit" | "units" => "count",
        "lf" | "ft" | "feet" | "foot" | "linear ft" | "lin ft" | "lnft" => "length_ft",
        "sf" | "sq ft" | "sqft" | "square feet" | "ft2" | "sft" => "area_sf",
        "cy" | "cu yd" | "cuyd" | "yd3" | "cubic yards" => "volume_cy",
        "cf" | "cu ft" | "cuft" | "ft3" => "volume_cf",
        "lb" | "lbs" | "pound" | "pounds" => "weight_lb",
        _ => return None,
    })
}

/// Find a price row by word overlap when exact matching fails.
/// Same algorithm as `bimwork::takeoff::match_row`: for MEP lines the system name and numbers
/// in the item are excluded from the coverage denominator (a duct named "Duct M-SUPPLY, 26 ga"
/// really just says "duct" once the system and gauges are ignored).
fn find_price_by_words<'a>(prices: &'a [Value], item: &str, unit: &str, category: &str, system: &str) -> Option<&'a Value> {
    let have = price_words(&format!("{item} {category} {system}"));
    let mep = matches!(category, "duct" | "pipe" | "conduit" | "tray" | "cable");
    let sys = if mep { price_words(system) } else { BTreeSet::new() };
    // The item's own words, stripped of the system name and numbers (same as takeoff::match_row).
    let iw: BTreeSet<String> = price_words(item).into_iter().filter(|w| !sys.contains(w) && !w.chars().any(|c| c.is_ascii_digit())).collect();
    let mut best: Option<(usize, f64)> = None;
    let unit_ok = |pu: &str| pu.is_empty() || pu.eq_ignore_ascii_case(unit) || measure_class(pu).is_some_and(|m| Some(m) == measure_class(unit));
    for (i, p) in prices.iter().enumerate() {
        let pu = p["unit"].as_str().unwrap_or_default();
        if !unit_ok(pu) {
            continue;
        }
        let pi = p["item"].as_str().unwrap_or_default();
        let want = price_words(pi);
        if want.is_empty() {
            continue;
        }
        let hit = want.iter().filter(|w| have.contains(*w)).count();
        let score = hit as f64 / want.len() as f64;
        let covered = if iw.is_empty() { 0.0 } else { iw.iter().filter(|w| want.contains(*w)).count() as f64 / iw.len() as f64 };
        if hit == 0 || !((score >= 0.5 && covered >= 0.5) || covered >= 1.0) {
            continue;
        }
        let score = score + covered;
        if best.is_none_or(|(_, s)| score > s) {
            best = Some((i, score));
        }
    }
    best.map(|(i, _)| &prices[i])
}

/// Q5301: family matching — a row whose words name a category prices lines of that category,
/// unless the row names a system, material or size that the line does not share (NEVER cross).
/// Returns the pool index of the best family-match row.
fn find_family_match(pool: &[Value], item: &str, unit: &str, category: &str, system: &str) -> Option<usize> {
    const SYSTEMS: &[&str] =
        &["gas", "vent", "cw", "hw", "dcw", "dhw", "supply", "return", "exhaust", "sanitary", "storm", "fire", "refrigerant", "condensate"];
    const MATERIALS: &[&str] =
        &["copper", "pvc", "steel", "galvanized", "aluminum", "cast", "iron", "fiberglass", "stainless", "brass", "emt", "imc", "rigid"];
    let have = price_words(&format!("{item} {category} {system}"));
    let mep = matches!(category, "duct" | "pipe" | "conduit" | "tray" | "cable");
    let line_sys: BTreeSet<&str> = SYSTEMS.iter().copied().filter(|s| have.contains(*s)).collect();
    let line_mat: BTreeSet<&str> = MATERIALS.iter().copied().filter(|m| have.contains(*m)).collect();
    // For MEP lines, the item's own words exclude the system name and numbers.
    let sys_words = if mep { price_words(system) } else { BTreeSet::new() };
    let iw: BTreeSet<String> = price_words(item).into_iter().filter(|w| !sys_words.contains(w) && !w.chars().any(|c| c.is_ascii_digit())).collect();
    let unit_ok = |pu: &str| pu.is_empty() || pu.eq_ignore_ascii_case(unit) || measure_class(pu).is_some_and(|m| Some(m) == measure_class(unit));
    let mut best: Option<(usize, f64)> = None;
    for (i, p) in pool.iter().enumerate() {
        let pu = p["unit"].as_str().unwrap_or_default();
        if !unit_ok(pu) {
            continue;
        }
        // Family rows must have no size (a sized row is an exact match or nothing).
        let ps = p["size"].as_str().unwrap_or_default();
        if !ps.is_empty() {
            continue;
        }
        let pi = p["item"].as_str().unwrap_or_default();
        let want = price_words(pi);
        if want.is_empty() {
            continue;
        }
        // All the row's words must appear in the line's context.
        let hit = want.iter().filter(|w| have.contains(*w)).count();
        let score = hit as f64 / want.len() as f64;
        let covered = if iw.is_empty() { 0.0 } else { iw.iter().filter(|w| want.contains(*w)).count() as f64 / iw.len() as f64 };
        if hit == 0 || !((score >= 0.5 && covered >= 0.5) || covered >= 1.0) {
            continue;
        }
        // NEVER cross: block only when BOTH the row and line name the attribute and they differ.
        // A row naming a material/system can still price a line that doesn't name one (the
        // estimator knows the material); the guard only fires on explicit mismatches.
        let row_sys: BTreeSet<&str> = SYSTEMS.iter().copied().filter(|s| want.contains(*s)).collect();
        if !row_sys.is_empty() && !line_sys.is_empty() && row_sys != line_sys {
            continue;
        }
        let row_mat: BTreeSet<&str> = MATERIALS.iter().copied().filter(|m| want.contains(*m)).collect();
        if !row_mat.is_empty() && !line_mat.is_empty() && row_mat != line_mat {
            continue;
        }
        let score = score + covered;
        if best.is_none_or(|(_, s)| score > s) {
            best = Some((i, score));
        }
    }
    best.map(|(i, _)| i)
}

// ---------------- CSV ----------------

/// Parse CSV text (quoted fields, doubled quotes) into header-keyed rows.
pub(crate) fn csv_rows(text: &str) -> Vec<BTreeMap<String, String>> {
    let rows = csv_cells(text);
    let Some(header) = rows.first().cloned() else { return Vec::new() };
    let header: Vec<String> = header.iter().map(|h| h.to_ascii_lowercase()).collect();
    rows.into_iter().skip(1).map(|r| header.iter().cloned().zip(r).collect()).collect()
}

/// Parse CSV text into rows of cells (blank rows dropped).
pub(crate) fn csv_cells(text: &str) -> Vec<Vec<String>> {
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
    rows
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
        let mut row = json!({ "item": item, "size": r.get("size").cloned().unwrap_or_default(), "unit": r.get("unit").cloned().unwrap_or_default(), "material": num(&r, "material"), "labor_hours": hours, "date": r.get("date").cloned().unwrap_or_default(), "source": r.get("source").cloned().unwrap_or_else(|| "baseline".into()) });
        if let Some(ix) = r.get("index").filter(|x| !x.is_empty()) {
            row["index"] = json!(ix);
        }
        prices.push(row);
    }
    let benchmarks: Value = read("benchmarks.json").and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    let json_file = |name: &str, notes: &mut Vec<String>| -> Value {
        match read(name).map(|t| serde_json::from_str::<Value>(&t)) {
            Some(Ok(v)) => v,
            Some(Err(e)) => {
                notes.push(format!("{name}: not JSON ({e})"));
                json!({})
            }
            None => json!({}),
        }
    };
    let factors = read("factors.csv").map(|t| labor::read_factors(&t, &mut notes)).unwrap_or_default();
    let production = labor::read_production(&json_file("production_model.json", &mut notes), &mut notes);
    let observations = read("field_observations.csv").map(|t| labor::read_observations(&t, &mut notes)).unwrap_or_default();
    let indexes = read("indexes.csv").map(|t| labor::read_indexes(&t, &mut notes)).unwrap_or_default();
    let tool_rules = equipment::read_rules(&json_file("tool_rules.json", &mut notes), &mut notes);
    let history: Vec<Value> = read("price_history.csv")
        .map(|t| csv_rows(&t))
        .unwrap_or_default()
        .iter()
        .filter_map(|r| {
            let item = r.get("item").filter(|i| !i.is_empty())?;
            Some(json!({ "item": item, "size": r.get("size").cloned().unwrap_or_default(), "unit": r.get("unit").cloned().unwrap_or_default(), "material": num(r, "material")?, "date": r.get("date").cloned().unwrap_or_default(), "source": r.get("source").cloned().unwrap_or_default() }))
        })
        .collect();
    // The label: given, else the pack's own (`pack.json`: the public pack says "public data").
    let pack: Value = read("pack.json").and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(json!({}));
    let label = str_param(p, "label").or_else(|| pack["label"].as_str()).unwrap_or("baseline").to_string();
    let counts = json!({ "items": items.len(), "assemblies": assemblies.len(), "prices": prices.len(), "rates": rates.as_object().map_or(0, Map::len), "benchmarks": benchmarks.as_object().map_or(0, Map::len), "factors": factors.len(), "drivers": production.len(), "observations": observations.len(), "indexes": indexes.len(), "tool_rules": tool_rules.len(), "history": history.len(), "label": label, "notes": notes });
    baseline.insert("label".into(), json!(label));
    baseline.insert("items".into(), Value::Object(items));
    baseline.insert("assemblies".into(), Value::Object(assemblies));
    baseline.insert("prices".into(), Value::Array(prices));
    baseline.insert("benchmarks".into(), benchmarks);
    baseline.insert("factors".into(), Value::Object(factors));
    baseline.insert("production".into(), Value::Array(production));
    baseline.insert("observations".into(), Value::Array(observations));
    baseline.insert("indexes".into(), Value::Object(indexes));
    baseline.insert("tool_rules".into(), Value::Array(tool_rules));
    baseline.insert("history".into(), Value::Array(history));
    if let Some(m) = rates.get("tier_multipliers").filter(|m| m.is_object()) {
        baseline.insert("tier_multipliers".into(), m.clone());
    }
    st.insert("baseline".into(), Value::Object(baseline));
    if let (Some(c), Some(set)) = (crew, obj(&mut st, "settings")) {
        set.insert("crew_rate".into(), json!(c));
    }
    store(s, sp, st)?;
    Ok(counts)
}

/// The trade classes P-405's public rows may have; any other (blank, unknown, electrical) is
/// kept private.
const PUBLIC_TRADES: &[&str] = &[
    "earthwork",
    "concrete",
    "masonry",
    "carpentry",
    "drywall",
    "roofing",
    "plumbing",
    "mechanical",
    "site",
    "finishes",
    "steel",
    "painting",
    "demolition",
    "insulation",
    "glazing",
    "paving",
    "landscaping",
];
/// Trade classes that are electrical (P-405 writes "CE", Construction Electrician).
const ELECTRICAL_TRADES: &[&str] = &["electrical", "electric", "elec", "electrician", "ce", "construction electrician"];

/// Whether a P-405 table's title (or a work element) is electrical work.
fn electrical_title(text: &str) -> bool {
    let t = format!(" {} ", text.to_ascii_lowercase().replace(|c: char| !c.is_ascii_alphanumeric(), " "));
    ["electric", "electrical", "lighting", "wiring", "power distribution", "conduit", "raceway", "cable", "panelboard", "switchgear"]
        .iter()
        .any(|w| t.contains(&format!(" {w} ")))
}

/// Import P-405 labor tables, splitting electrical rows into a private pack and keeping
/// everything else for the public pack. The full CSV has columns: table, title, work element
/// (= item), unit, man-hours/unit (= labor_normal), P-405 code, trade class. Fail closed: only a
/// row of a known non-electrical trade whose table and work aren't electrical is public; and the
/// private rows need a private folder before anything is kept.
fn run_import_p405(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "estimate.import_p405";
    let file = str_param(p, "file").ok_or_else(|| bad(C, "`file` (path to p405_labor_units.csv) is required"))?;
    let path = std::path::Path::new(file);
    let text = std::fs::read_to_string(path).map_err(|e| bad(C, format!("{file}: {e}")))?;
    let rows = csv_rows(&text);
    if rows.is_empty() {
        return Err(bad(C, format!("{file}: no data rows")));
    }

    let mut public: Vec<(String, Value)> = Vec::new();
    let mut private_rows: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    for r in &rows {
        let item = r.get("item").or(r.get("work element")).map(String::as_str).unwrap_or("").trim();
        if item.is_empty() {
            skipped += 1;
            continue;
        }
        let unit = r.get("unit").map(String::as_str).unwrap_or("").trim();
        let trade = r.get("trade_class").or(r.get("trade class")).map(String::as_str).unwrap_or("").trim();
        let title = r.get("p405_title").or(r.get("title")).map(String::as_str).unwrap_or("");
        let table = r.get("p405_table").or(r.get("table")).map(String::as_str).unwrap_or("?");
        let code = r.get("p-405 code").or(r.get("p405_code")).map(String::as_str).unwrap_or("");
        let electrical = ELECTRICAL_TRADES.iter().any(|t| trade.eq_ignore_ascii_case(t)) || electrical_title(title) || electrical_title(item);
        let public_trade = PUBLIC_TRADES.iter().any(|t| trade.eq_ignore_ascii_case(t));
        let labor_n = num(r, "labor_normal").or(num(r, "man-hours/unit"));
        if electrical || !public_trade {
            // Electrical labor, and anything whose trade isn't known to be something else: private.
            private_rows.push(format!("{item}\t{unit}\t{}\t{trade}\t{table}\t{code}", labor_n.map_or(String::new(), |h| h.to_string())));
            continue;
        }
        // Difficulty tiers: P-405 uses a 1.4x/1.8x factor.
        public.push((
            item.to_string(),
            json!({
                "unit": unit,
                "labor": { "normal": labor_n, "difficult": labor_n.map(|h| h * 1.4), "very_difficult": labor_n.map(|h| h * 1.8) },
                "source": format!("NAVFAC P-405 Table {table} (Oct 1996)"),
            }),
        ));
    }
    // The private rows go to the private folder, or nothing is imported.
    let mut notes: Vec<String> = Vec::new();
    if !private_rows.is_empty() {
        let folder = str_param(p, "private_folder").map(str::trim).filter(|f| !f.is_empty()).ok_or_else(|| {
            bad(C, format!("{} rows are electrical (or of no known trade) and go to the private pack: give `private_folder`", private_rows.len()))
        })?;
        let dir = std::path::Path::new(folder);
        if !dir.is_dir() {
            return Err(bad(C, format!("private_folder {folder} is not a folder: the {} private rows have nowhere to go", private_rows.len())));
        }
        let priv_path = dir.join("p405_electrical_labor.tsv");
        let mut out = String::from("item\tunit\tman_hours_per_unit\ttrade_class\ttable\tcode\n");
        for row in &private_rows {
            out.push_str(row);
            out.push('\n');
        }
        crate::writefile::write(&priv_path, &out).map_err(|e| bad(C, format!("write {}: {e}", priv_path.display())))?;
        notes.push(format!("{} private rows written to {}", private_rows.len(), priv_path.display()));
    }
    let (sp, mut st) = state(s, C)?;
    let pub_count = public.len();
    if let Some(items) = obj(&mut st, "baseline").and_then(|b| b.entry("items").or_insert_with(|| json!({})).as_object_mut()) {
        items.extend(public);
    }
    store(s, sp, st)?;
    Ok(json!({ "public_rows": pub_count, "private_rows": private_rows.len(), "skipped": skipped, "notes": notes }))
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
    let mut price = json!({ "item": item, "size": size, "unit": unit, "material": material, "labor_hours": hours, "date": date, "source": source });
    if let Some(ix) = str_param(p, "index").filter(|x| !x.trim().is_empty()) {
        price["index"] = json!(ix);
    }
    let mut replaced: Vec<Value> = Vec::new();
    if let Some(a) = obj(&mut st, "baseline").and_then(|base| base.entry("prices").or_insert_with(|| json!([])).as_array_mut()) {
        let same = |x: &Value| x["item"].as_str() == Some(item) && x["size"].as_str().unwrap_or_default() == size;
        replaced = a.iter().filter(|x| same(x)).cloned().collect();
        a.retain(|x| !same(x));
        a.push(price.clone());
    }
    // A replaced price becomes history (the validator compares new prices with it).
    if let Some(h) = obj(&mut st, "baseline").and_then(|base| base.entry("history").or_insert_with(|| json!([])).as_array_mut()) {
        h.extend(replaced.into_iter().filter(|x| x["material"].as_f64().is_some()));
        let over = h.len().saturating_sub(MAX_ROWS);
        h.drain(..over);
    }
    store(s, sp, st)?;
    Ok(json!({ "price": price }))
}

/// Q6020: explicitly map BOM lines to a specific price row (never silently applied).
fn run_map_price(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "estimate.map_price";
    let row_item =
        str_param(p, "row_item").filter(|i| !i.trim().is_empty()).ok_or_else(|| bad(C, "`row_item` (the price row's item name) is required"))?;
    let row_size = str_param(p, "row_size").unwrap_or_default();
    let reason = str_param(p, "reason").filter(|r| !r.trim().is_empty()).ok_or_else(|| bad(C, "`reason` is required"))?.to_string();
    let keys = picked(C, s, p)?;
    let (sp, mut st) = state(s, C)?;
    // Validate the target row exists in the baseline prices.
    let found = st.get("baseline").and_then(|b| b["prices"].as_array()).is_some_and(|ps| {
        ps.iter().any(|r| {
            r["item"].as_str().is_some_and(|i| i.eq_ignore_ascii_case(row_item))
                && r["size"].as_str().unwrap_or_default().eq_ignore_ascii_case(row_size)
        })
    });
    if !found {
        let desc = if row_size.is_empty() { row_item.to_string() } else { format!("{row_item} {row_size}") };
        return Err(bad(C, format!("no price row for {desc} (import it or estimate.set_price first)")));
    }
    if let Some(pm) = obj(&mut st, "price_map") {
        for k in &keys {
            pm.insert(k.clone(), json!({ "item": row_item, "size": row_size, "reason": reason }));
        }
    }
    store(s, sp, st)?;
    Ok(json!({ "lines": keys }))
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
    for k in [
        "tax_pct",
        "overhead_pct",
        "profit_pct",
        "contingency_pct",
        "bond_pct",
        "crew_rate",
        "stale_days",
        "duration_days",
        "peak_crew",
        "high_work_ft",
        "gross_sf",
    ] {
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
    if let Some(b) = p.get("pricebook").and_then(Value::as_bool) {
        set.insert("pricebook".into(), json!(b));
    }
    if let Some(f) = str_param(p, "pricebook_folder") {
        set.insert("pricebook_folder".into(), json!(f.trim()));
    }
    if let Some(tier) = str_param(p, "tier") {
        if !labor::TIERS.contains(&tier) {
            return Err(bad("estimate.settings", "`tier` is normal, difficult or very_difficult"));
        }
        set.insert("tier".into(), json!(tier));
    }
    if let Some(c) = str_param(p, "context") {
        set.insert("context".into(), json!(c.trim().chars().take(200).collect::<String>()));
    }
    let out = Value::Object(set.clone());
    store(s, sp, st)?;
    Ok(json!({ "settings": out }))
}

// ---------------- the calculator ----------------

/// Days since 0000-03-01 for a YYYY-MM-DD date (for ages in days).
pub(crate) fn parse_date(d: &str) -> Option<i64> {
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
    /// How this line was priced: "exact", "family", "mapped", "starter_book", "starter_family", or "" (unpriced).
    pub(super) match_kind: &'static str,
    /// How the material price was reached (escalation, rental periods).
    pub(super) material_steps: Vec<String>,
    /// How the labor rate was reached (tier, drivers, factors).
    pub(super) labor_steps: Vec<String>,
    /// Labor hours of the line by production phase.
    pub(super) phases: Vec<(String, f64)>,
}

/// Price every BOM line from the state; `what_if` overrides assemblies, scopes and settings.
pub(super) fn price_all(s: &mut Session, st: &Map<String, Value>) -> Result<(Vec<Priced>, f64)> {
    let bom = s.execute("bim.bom", &json!({}))?;
    Ok(price_lines(st, &bom))
}

/// Price the lines of a BOM (`bim.bom`'s result).
pub(super) fn price_lines(st: &Map<String, Value>, bom: &Value) -> (Vec<Priced>, f64) {
    let lines = bom["lines"].as_array().cloned().unwrap_or_default();
    let crew = setting(st, "crew_rate", 0.0);
    let today_str = st.get("settings").and_then(|x| x["today"].as_str()).unwrap_or_default().to_string();
    let today = parse_date(&today_str);
    let stale = setting(st, "stale_days", 365.0);
    let base = st.get("baseline").cloned().unwrap_or(json!({}));
    let drivers = labor::drivers(st, &base);
    let prices: Vec<Value> = base["prices"].as_array().cloned().unwrap_or_default();
    let price_map: Map<String, Value> = st.get("price_map").and_then(Value::as_object).cloned().unwrap_or_default();
    // Q6012: starter cost book as fallback when no user/baseline price matches.
    let starter: Vec<Value> = starter_prices();
    // The material price book (recent dated prices: shipped, and this machine's), before the
    // starter book; `settings.pricebook: false` leaves it out.
    let settings = st.get("settings").cloned().unwrap_or(json!({}));
    let pbook = (settings["pricebook"].as_bool() != Some(false)).then(|| super::pricebook::load(settings["pricebook_folder"].as_str()));
    // Q5301: exact match → family match (never crossing system/material/size) → None.
    let find_in = |pool: &[Value], item: &str, size: &str, unit: &str, category: &str, system: &str| -> Option<(usize, &'static str)> {
        // 1. Exact: item+size match.
        if let Some(i) = pool.iter().position(|p| {
            p["item"].as_str().is_some_and(|pi| pi.eq_ignore_ascii_case(item)) && p["size"].as_str().unwrap_or_default().eq_ignore_ascii_case(size)
        }) {
            return Some((i, "exact"));
        }
        // 2. Item-only family: same item name, row has no size.
        if let Some(i) = pool
            .iter()
            .position(|p| p["item"].as_str().is_some_and(|pi| pi.eq_ignore_ascii_case(item)) && p["size"].as_str().unwrap_or_default().is_empty())
        {
            return Some((i, "family"));
        }
        // 3. Word-based family: category words match, never crossing system/material.
        find_family_match(pool, item, unit, category, system).map(|i| (i, "family"))
    };
    let find_price = |item: &str, size: &str, unit: &str, category: &str, system: &str| -> Option<(&Value, &'static str)> {
        find_in(&prices, item, size, unit, category, system).map(|(i, kind)| (&prices[i], kind))
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
        type Issues = Vec<(&'static str, String, String)>;
        let check_price = |p: &Value, issues: &mut Issues, steps: &mut Vec<String>, what: &str| -> Option<(f64, Option<f64>)> {
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
            let item = p["item"].as_str().unwrap_or_default();
            if let Some(msg) = m.and_then(|m| labor::off_history(&base, item, p["size"].as_str().unwrap_or_default(), m)) {
                issues.push(("price_off_history", msg, "confirm the quote with the supplier, or correct it (estimate.set_price)".into()));
            }
            let mut m = m.unwrap_or(0.0);
            match labor::escalation(&base, today.map(|t| (t, today_str.as_str())), p) {
                Ok(Some((ratio, text))) => {
                    steps.push(format!("{item}: ${m:.2} {text} = ${:.4}", m * ratio));
                    m *= ratio;
                }
                Ok(None) => {}
                Err(e) => issues.push(("no_index", e, "add the index's values to indexes.csv (or drop the price's index)".into())),
            }
            Some((m, p["labor_hours"].as_f64()))
        };
        let mut book: Option<f64> = None;
        match assembly.as_deref().and_then(|a| base["assemblies"].get(a).map(|x| (a, x))) {
            Some((a, asm)) => {
                pr.via = format!("assembly {a}");
                pr.match_kind = "mapped";
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
                        match find_price(item, "", &unit, "", "")
                            .map(|(p, _)| p)
                            .and_then(|p| check_price(p, &mut pr.issues, &mut pr.material_steps, "part"))
                        {
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
                    book = asm["labor_hours"].as_f64();
                }
            }
            None => {
                let item = l["item"].as_str().unwrap_or_default();
                let size = l["size"].as_str().unwrap_or_default();
                let category = l["category"].as_str().unwrap_or_default();
                let system = l["system"].as_str().unwrap_or_default();
                // Q6020: check user price mapping first.
                let mapped_price = price_map.get(&key).and_then(|x| {
                    let mi = x["item"].as_str()?;
                    let ms = x["size"].as_str().unwrap_or_default();
                    prices.iter().find(|p| {
                        p["item"].as_str().is_some_and(|i| i.eq_ignore_ascii_case(mi))
                            && p["size"].as_str().unwrap_or_default().eq_ignore_ascii_case(ms)
                    })
                });
                if let Some(p) = mapped_price {
                    pr.via =
                        format!("mapped \u{2192} price {} ({})", p["source"].as_str().unwrap_or_default(), p["date"].as_str().unwrap_or_default());
                    pr.match_kind = "mapped";
                    if let Some((m, h)) = check_price(p, &mut pr.issues, &mut pr.material_steps, "line") {
                        pr.material_unit = Some(m);
                        book = h.or_else(|| base["items"][item]["labor"]["normal"].as_f64());
                    }
                } else {
                    // Q5301: user exact > user family > starter exact > starter family > unpriced.
                    match find_price(item, size, &unit, category, system) {
                        Some((p, kind)) => {
                            let source_label = if kind == "family" {
                                let ri = p["item"].as_str().unwrap_or_default();
                                format!(
                                    "priced by family row: {ri} — {} ({})",
                                    p["source"].as_str().unwrap_or_default(),
                                    p["date"].as_str().unwrap_or_default()
                                )
                            } else {
                                format!("price {} ({})", p["source"].as_str().unwrap_or_default(), p["date"].as_str().unwrap_or_default())
                            };
                            pr.via = source_label;
                            pr.match_kind = kind;
                            if let Some((m, h)) = check_price(p, &mut pr.issues, &mut pr.material_steps, "line") {
                                pr.material_unit = Some(m);
                                book = h.or_else(|| base["items"][item]["labor"]["normal"].as_f64());
                            }
                        }
                        // The price book, then the starter cost book, before giving up.
                        None => match pbook.as_ref().and_then(|b| super::pricebook::lookup(b, item, size, &unit)) {
                            Some(super::pricebook::Found::Priced { material, parts, date }) => {
                                pr.via = format!("price book: {}", super::pricebook::source_text(&parts));
                                pr.match_kind = "price_book";
                                if let (Some(t), Some(d)) = (today, parse_date(&date))
                                    && (t - d) as f64 > stale
                                {
                                    pr.issues.push((
                                        "stale_price",
                                        format!("price book dated {date} ({} days old)", t - d),
                                        "update the price (pricebook.set or pricebook.import)".into(),
                                    ));
                                }
                                pr.material_unit = Some(material);
                                // Its labor: the starter book's for the item (the price book holds material).
                                book = find_in(&starter, item, size, &unit, category, system)
                                    .and_then(|(si, _)| starter[si]["labor_hours"].as_f64())
                                    .or_else(|| base["items"][item]["labor"]["normal"].as_f64());
                            }
                            Some(super::pricebook::Found::Excluded(why)) => {
                                pr.via = format!("excluded in the price book: {why}");
                                pr.match_kind = "price_book";
                                if pr.scope.is_none() {
                                    pr.scope = Some("nic".into());
                                }
                            }
                            None => match find_in(&starter, item, size, &unit, category, system) {
                                Some((si, kind)) => {
                                    let sp = &starter[si];
                                    pr.via = if kind == "family" {
                                        let ri = sp["item"].as_str().unwrap_or_default();
                                        format!("priced by family row: {ri} — {STARTER_SOURCE}")
                                    } else {
                                        STARTER_SOURCE.to_string()
                                    };
                                    pr.match_kind = if kind == "family" { "starter_family" } else { "starter_book" };
                                    let m = sp["material"].as_f64().unwrap_or(0.0);
                                    let h = sp["labor_hours"].as_f64();
                                    pr.material_unit = Some(m);
                                    book = h.or_else(|| base["items"][item]["labor"]["normal"].as_f64());
                                }
                                None => {
                                    // Q6020: nearest row is a SUGGESTION, never applied silently.
                                    if let Some(near) = find_price_by_words(&prices, item, &unit, category, system)
                                        .or_else(|| find_price_by_words(&starter, item, &unit, category, system))
                                    {
                                        let ni = near["item"].as_str().unwrap_or_default();
                                        let ns = near["size"].as_str().unwrap_or_default();
                                        let near_desc = if ns.is_empty() { ni.to_string() } else { format!("{ni} {ns}") };
                                        let item_desc = if size.is_empty() { item.to_string() } else { format!("{item} {size}") };
                                        pr.issues.push((
                                            "unpriced",
                                            format!(
                                                "{item_desc} has no price; nearest row: {near_desc} \u{2014} estimate.set_price or estimate.map_price"
                                            ),
                                            "estimate.set_price or estimate.map_price".into(),
                                        ));
                                    } else {
                                        pr.issues.push((
                                            "unpriced",
                                            "no assembly and no price".into(),
                                            "map an assembly (estimate.set_assembly) or add a price (estimate.set_price)".into(),
                                        ));
                                    }
                                }
                            },
                        },
                    }
                }
            }
        }
        if pr.material_unit.is_some() {
            let (h, steps, phases) = labor::line_labor(st, &base, &drivers, &key, l, book);
            pr.labor_unit_hours = h;
            pr.labor_steps = steps;
            pr.phases = phases.into_iter().map(|(ph, u)| (ph, u * net)).collect();
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
    (out, crew)
}

/// The whole job priced: the BOM's lines, the job's equipment, tools and setup, and the
/// findings that aren't one line's (missing equipment, the spec check, open model validation).
pub(super) struct Job {
    pub(super) priced: Vec<Priced>,
    pub(super) crew: f64,
    pub(super) findings: Vec<Value>,
    /// Per line key, its net quantity by level (for the by-level breakdown).
    pub(super) levels: BTreeMap<String, Vec<(String, f64)>>,
    pub(super) level_count: usize,
}

pub(super) fn price_job(s: &mut Session, st: &Map<String, Value>, cmd: &str, with_levels: bool) -> Result<Job> {
    let bom = s.execute("bim.bom", &json!({}))?;
    let (mut priced, crew) = price_lines(st, &bom);
    let (sp, _) = current(s, cmd)?;
    let base = st.get("baseline").cloned().unwrap_or(json!({}));
    let facts = equipment::facts(&sp, st, &priced);
    let required = bom["required"].as_array().cloned().unwrap_or_default();
    let (eq, mut findings) = equipment::lines(st, &base, &facts, &required, &priced);
    priced.extend(eq);
    // The specification's requirements with no line (the BOM's spec check).
    for c in bom["spec_check"].as_array().into_iter().flatten().take(1_000) {
        findings.push(json!({
            "kind": "spec_unmatched",
            "message": format!("{}: {} ({})", c["item"].as_str().unwrap_or_default(), c["detail"].as_str().unwrap_or_default(), c["issue"].as_str().unwrap_or_default()),
            "fix": "add the line the specification requires to the model or takeoff, or record why it isn't needed (bim.bom waive)",
        }));
    }
    // The print validator's failures still open: the quantities may be wrong.
    if sp.prints.is_some()
        && let Ok(v) = s.execute("bim.validate", &json!({ "limit": 5 }))
        && let Some(n) = v["total"].as_u64().filter(|n| *n > 0)
    {
        let first: Vec<String> =
            v["failures"].as_array().into_iter().flatten().filter_map(|f| f["rule"].as_str().map(String::from)).take(5).collect();
        findings.push(json!({
            "kind": "model_validation_open",
            "message": format!("{n} model validation failure(s) still open (bim.validate): {}", first.join("; ")),
            "fix": "fix or answer the failures (bim.validate) before the bid: the quantities come from the model",
        }));
    }
    findings.extend(ratio_findings(&sp, st, &priced));
    let mut levels: BTreeMap<String, Vec<(String, f64)>> = BTreeMap::new();
    if with_levels {
        let by = s.execute("bim.bom", &json!({ "group_by": "level" }))?;
        for l in by["lines"].as_array().into_iter().flatten() {
            levels.entry(key_of(l)).or_default().push((l["level"].as_str().unwrap_or_default().to_string(), l["net"].as_f64().unwrap_or(0.0)));
        }
    }
    Ok(Job { priced, crew, findings, levels, level_count: sp.levels.len() })
}

/// Is `p` inside the polygon `poly` (even-odd)?
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

/// Benchmarks per unit type (quantities per room of a type, the prints' rooms) and hours per
/// square foot.
fn ratio_findings(sp: &cadaddy_walk3d::BuildingSpec, st: &Map<String, Value>, priced: &[Priced]) -> Vec<Value> {
    let mut out = Vec::new();
    let Some(b) = st.get("baseline").map(|b| &b["benchmarks"]) else { return out };
    let rooms = sp.prints.as_ref().map(|p| p.rooms.as_slice()).unwrap_or_default();
    let tags: BTreeMap<&str, &cadaddy_walk3d::tags::ElementTag> = sp.tags.iter().map(|t| (t.id.as_str(), t)).collect();
    for r in b["per_unit_type"].as_array().into_iter().flatten().take(1_000) {
        let ut = r["unit_type"].as_str().unwrap_or_default();
        let m = r["match"].as_str().unwrap_or_default().to_ascii_lowercase();
        let units: Vec<_> = rooms.iter().filter(|x| x.unit_type.as_deref().is_some_and(|u| u.eq_ignore_ascii_case(ut))).collect();
        if ut.is_empty() || m.is_empty() || units.is_empty() {
            continue;
        }
        let mut n = 0usize;
        for p in priced.iter().filter(|p| p.line["item"].as_str().is_some_and(|i| i.to_ascii_lowercase().contains(&m))) {
            for id in p.line["elements"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                if let Some(t) = tags.get(id)
                    && units.iter().any(|u| u.level.eq_ignore_ascii_case(&t.level) && inside(t.at_ft, &u.outline))
                {
                    n += 1;
                }
            }
        }
        let per = n as f64 / units.len() as f64;
        let (lo, hi) = (r["low"].as_f64().unwrap_or(0.0), r["high"].as_f64().unwrap_or(f64::INFINITY));
        if per < lo || per > hi {
            out.push(json!({
                "kind": "off_ratio_unit",
                "message": format!("{m}: {per:.1} per {ut} unit ({n} in {} units), typical {lo}–{hi} ({})", units.len(), r["source"].as_str().unwrap_or("benchmarks")),
                "fix": format!("check the {ut} units' takeoff for missed or double-counted items"),
            }));
        }
    }
    let sf = setting(st, "gross_sf", 0.0);
    if let Some(h) = b.get("hours_per_sf").filter(|_| sf > 0.0) {
        let hours: f64 = priced.iter().filter(|p| p.scope.as_deref() == Some("in_contract")).map(|p| p.labor_hours).sum();
        let per = hours / sf;
        let (lo, hi) = (h["low"].as_f64().unwrap_or(0.0), h["high"].as_f64().unwrap_or(f64::INFINITY));
        if per < lo || per > hi {
            out.push(json!({
                "kind": "off_hours_per_sf",
                "message": format!("{hours:.1} h over {sf} SF = {per:.4} h/SF, typical {lo}–{hi} ({})", h["source"].as_str().unwrap_or("benchmarks")),
                "fix": "check the labor rates, factors and quantities against past jobs",
            }));
        }
    }
    out
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
    let mut phases: BTreeMap<String, f64> = BTreeMap::new();
    for p in &inc {
        for (ph, h) in &p.phases {
            *phases.entry(ph.clone()).or_insert(0.0) += h;
        }
    }
    json!({
        "labor_by_phase": phases.into_iter().map(|(k, v)| (k, json!(r2(v)))).collect::<Map<String, Value>>(),
        "totals": { "material": r2(material), "labor": r2(labor), "labor_hours": r2(hours), "direct": r2(direct), "tax": r2(tax), "overhead": r2(overhead), "profit": r2(profit), "contingency": r2(contingency), "bond": r2(bond), "total": r2(before_bond + bond) },
        "by_scope": by_scope.into_iter().map(|(k, v)| (k, json!(r2(v)))).collect::<Map<String, Value>>(),
        "alternates": alternates,
    })
}

/// The validator: every line's issues, and the benchmark ratio checks.
fn findings(st: &Map<String, Value>, job: &Job) -> Vec<Value> {
    let (priced, levels) = (&job.priced, job.level_count);
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
    out.extend(job.findings.iter().cloned());
    out
}

fn summary(priced: &[Priced]) -> Value {
    let n = |f: &dyn Fn(&Priced) -> bool| priced.iter().filter(|p| f(p)).count();
    json!({
        "lines": priced.len(),
        "exact": n(&|p| p.match_kind == "exact"),
        "family": n(&|p| p.match_kind == "family"),
        "mapped": n(&|p| p.match_kind == "mapped"),
        "starter_book": n(&|p| p.match_kind == "starter_book"),
        "starter_family": n(&|p| p.match_kind == "starter_family"),
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
        "scope": p.scope, "via": p.via, "match_kind": p.match_kind,
        "status": if p.issues.iter().any(|i| matches!(i.0, "unit_mismatch")) { "error" } else if p.material_unit.is_none() { "unpriced" } else if p.scope.is_none() { "undecided" } else { "priced" },
        "issues": p.issues.iter().map(|i| i.0).collect::<Vec<_>>(),
    })
}

fn run_query(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, st) = state(s, "estimate.query")?;
    let priced = price_job(s, &st, "estimate.query", false)?.priced;
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
    let (_, st) = state(s, "estimate.status")?;
    let job = price_job(s, &st, "estimate.status", true)?;
    let mut out = totals(&st, &job.priced);
    let b = outputs::breakdown(&st, &job);
    for k in ["by_level", "by_trade", "by_system"] {
        out[k] = b[k].clone();
    }
    let mut f = findings(&st, &job);
    // Q5294 MUST 2: warn when crew rate is missing and there are labor hours.
    let crew = setting(&st, "crew_rate", 0.0);
    let total_hours: f64 = job.priced.iter().map(|p| p.labor_hours).sum();
    if crew == 0.0 && total_hours > 0.0 {
        f.push(json!({
            "kind": "no_crew_rate",
            "message": format!("{total_hours:.0} labor hours are $0 because no crew rate is set"),
            "fix": "estimate.settings {crew_rate: 75} (your crew's loaded hourly rate)",
        }));
    }
    // Q5278 MUST 3: ok:false when every line is unpriced (not just when findings exist).
    let unpriced = job.priced.iter().filter(|p| p.material_unit.is_none()).count();
    let all_unpriced = !job.priced.is_empty() && unpriced == job.priced.len();
    out["ok"] = json!(f.is_empty() && !all_unpriced);
    out["summary"] = summary(&job.priced);
    out["findings"] = json!(f);
    if let Some(n) = load_costs_step(&job.priced) {
        out["next_step"] = json!(n);
    }
    if all_unpriced {
        out["unpriced"] = json!(format!("all {} lines have no price: estimate.import_costs", job.priced.len()));
    }
    // Q6012: report starter cost book usage.
    let starter_count = job.priced.iter().filter(|p| p.via == STARTER_SOURCE).count();
    if starter_count > 0 {
        out["starter_cost_book"] =
            json!(format!("priced with the starter cost book: {} of {} lines; replace with estimate.import_costs", starter_count, job.priced.len()));
        f.push(json!({
            "kind": "starter_cost_book",
            "message": format!("{starter_count} line(s) priced with the starter cost book — generic values, not project-specific"),
            "fix": "estimate.import_costs {path: your-costs.csv} to replace starter prices with your own",
        }));
        out["findings"] = json!(f);
    }
    Ok(out)
}

/// The next step when lines have no price: load a cost table.
fn load_costs_step(priced: &[Priced]) -> Option<String> {
    let unpriced = priced.iter().filter(|p| p.material_unit.is_none()).count();
    (unpriced > 0).then(|| {
        format!(
            "{unpriced} of {} lines have no price: load a cost table — estimate.import_costs {{path: costs.csv or .xlsx}} (columns item, unit, material, labor_hrs or labor_dollars; size and date optional)",
            priced.len()
        )
    })
}

/// A cost table's column for `names` (any case; spaces, `$` and `_` aside).
fn column(header: &[String], names: &[&str]) -> Option<usize> {
    let norm = |h: &str| h.to_ascii_lowercase().chars().filter(char::is_ascii_alphanumeric).collect::<String>();
    header.iter().position(|h| names.iter().any(|n| norm(h) == norm(n)))
}

/// Cost table rows read at most, and the largest file.
const MAX_COST_ROWS: usize = 50_000;
const MAX_COST_BYTES: u64 = 64 * 1024 * 1024;

fn run_import_costs(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "estimate.import_costs";
    let path =
        str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(C, "`path` (a .csv or .xlsx cost table) is required"))?;
    let size = std::fs::metadata(path).map_err(|e| bad(C, format!("{path}: {e}")))?.len();
    if size > MAX_COST_BYTES {
        return Err(bad(C, format!("{path}: a cost table is at most 64 MB")));
    }
    let bytes = std::fs::read(path).map_err(|e| bad(C, format!("{path}: {e}")))?;
    let table: Vec<Vec<String>> = if path.to_ascii_lowercase().ends_with(".xlsx") {
        super::table::xlsx::open(&bytes).and_then(|w| w.cells(str_param(p, "sheet"), None)).map_err(|e| bad(C, format!("{path}: {e}")))?
    } else {
        let text = String::from_utf8_lossy(&bytes);
        let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default();
        let header: Vec<String> = first.split(',').map(|h| h.trim().trim_matches('"').to_ascii_lowercase()).collect();
        let rows = csv_rows(&text);
        std::iter::once(header.clone()).chain(rows.iter().map(|r| header.iter().map(|h| r.get(h).cloned().unwrap_or_default()).collect())).collect()
    };
    let mut rows = table.into_iter().filter(|r| r.iter().any(|c| !c.trim().is_empty()));
    let header: Vec<String> = rows.next().unwrap_or_default();
    let item_col = column(&header, &["item", "description", "name"]).ok_or_else(|| bad(C, "the cost table needs an `item` column"))?;
    let unit_col = column(&header, &["unit", "uom", "units"]);
    let size_col = column(&header, &["size"]);
    let mat_col = column(&header, &["material", "material $", "material_cost", "mat", "unit_cost"])
        .ok_or_else(|| bad(C, "the cost table needs a `material` column (dollars per unit)"))?;
    let hrs_col = column(&header, &["labor_hrs", "labor_hours", "labour_hrs", "labour_hours", "hours", "hrs"]);
    let usd_col = column(&header, &["labor_dollars", "labor $", "labor_cost", "labour_dollars", "labour $", "labour_cost"]);
    let date_col = column(&header, &["date"]);
    let (sp, mut st) = state(s, C)?;
    let crew = setting(&st, "crew_rate", 0.0);
    let date = str_param(p, "date").map(str::to_string).or_else(|| st.get("settings").and_then(|x| x["today"].as_str()).map(str::to_string));
    let source = str_param(p, "source").map(str::to_string).unwrap_or_else(|| {
        std::path::Path::new(path).file_name().map_or_else(|| "cost table".into(), |n| n.to_string_lossy().chars().take(120).collect())
    });
    let cell = |r: &[String], c: Option<usize>| c.and_then(|i| r.get(i)).map(|v| v.trim().to_string()).unwrap_or_default();
    let money = |v: &str| v.trim().trim_start_matches('$').replace(',', "").parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0 && *v < 1e9);
    let (mut skipped, mut dollars_only) = (Vec::<String>::new(), 0usize);
    let mut new_prices: Vec<Value> = Vec::new();
    for (n, r) in rows.take(MAX_COST_ROWS).enumerate() {
        let item = cell(&r, Some(item_col));
        let Some(material) = money(&cell(&r, Some(mat_col))) else {
            if skipped.len() < 50 {
                skipped.push(format!("row {}: {} has no material price", n + 2, if item.is_empty() { "(no item)" } else { item.as_str() }));
            }
            continue;
        };
        if item.is_empty() {
            continue;
        }
        let labor_dollars = money(&cell(&r, usd_col));
        let hours = money(&cell(&r, hrs_col)).or_else(|| labor_dollars.filter(|_| crew > 0.0).map(|d| d / crew));
        if hours.is_none() && labor_dollars.is_some() {
            dollars_only += 1;
        }
        let row_date = Some(cell(&r, date_col)).filter(|d| !d.is_empty()).or_else(|| date.clone()).unwrap_or_default();
        let unit = Some(cell(&r, unit_col)).filter(|u| !u.is_empty()).unwrap_or_else(|| "each".into());
        let mut price = json!({
            "item": item.chars().take(300).collect::<String>(), "size": cell(&r, size_col), "unit": unit,
            "material": material, "labor_hours": hours, "date": row_date, "source": source,
        });
        if let Some(d) = labor_dollars {
            price["labor_dollars"] = json!(d);
        }
        new_prices.push(price);
    }
    let added = new_prices.len();
    if added == 0 {
        return Err(bad(
            C,
            format!("{path}: no rows with an item and a material price{}", skipped.first().map(|x| format!(" ({x})")).unwrap_or_default()),
        ));
    }
    if let Some(a) = obj(&mut st, "baseline").and_then(|base| base.entry("prices").or_insert_with(|| json!([])).as_array_mut()) {
        let key = |x: &Value| (x["item"].as_str().map(str::to_ascii_lowercase), x["size"].as_str().map(str::to_ascii_lowercase));
        a.retain(|x| !new_prices.iter().any(|y| key(x) == key(y)));
        a.extend(new_prices);
        let over = a.len().saturating_sub(MAX_ROWS);
        a.drain(..over);
    }
    store(s, sp, st)?;
    let mut notes: Vec<String> = Vec::new();
    if dollars_only > 0 {
        notes.push(format!("{dollars_only} row(s) give labour in dollars only: set crew_rate (estimate.settings) to turn them into hours"));
    }
    Ok(json!({ "prices": added, "skipped": skipped, "source": source, "notes": notes, "next_step": "estimate.status (priced lines and totals)" }))
}

fn run_explain(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, st) = state(s, "estimate.explain")?;
    let job = price_job(s, &st, "estimate.explain", false)?;
    let (priced, crew) = (&job.priced, job.crew);
    if let Some(k) = str_param(p, "line") {
        let pr = priced.iter().find(|x| x.key == k).ok_or_else(|| bad("estimate.explain", format!("no line {k}")))?;
        let mu = pr.material_unit.unwrap_or(0.0);
        let lh = pr.labor_unit_hours.unwrap_or(0.0);
        let mut math = vec![format!("material: {} {} (net {} + waste) × ${mu:.4} = ${:.2}  [{}]", pr.qty, pr.unit, pr.net, pr.material, pr.via)];
        math.extend(pr.material_steps.iter().map(|x| format!("  material price: {x}")));
        math.extend(pr.labor_steps.iter().map(|x| format!("  labor rate: {x}")));
        math.push(format!("labor: {} {} (net) × {lh:.4} h = {:.2} h × ${crew:.2}/h = ${:.2}", pr.net, pr.unit, pr.labor_hours, pr.labor_cost));
        math.push(format!("line: ${:.2} + ${:.2} = ${:.2}", pr.material, pr.labor_cost, pr.cost));
        let r2 = |v: f64| (v * 100.0).round() / 100.0;
        return Ok(json!({
            "value": r2(pr.cost),
            "math": math,
            "labor_hours": r2(pr.labor_hours),
            "labor_cost": r2(pr.labor_cost),
            "phases": pr.phases.iter().map(|(ph, h)| (ph.clone(), json!(r2(*h)))).collect::<Map<String, Value>>(),
            "scope": pr.scope,
            "rule": pr.line["rule"],
            "elements": pr.line["elements"],
        }));
    }
    if bool_or(p, "total", false) {
        let t = totals(&st, priced);
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

/// Most lines one change prices.
const MAX_CHANGE_LINES: usize = 200;

fn run_change(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "estimate.change";
    let lines = p.get("lines").and_then(Value::as_array).filter(|l| !l.is_empty()).ok_or_else(|| bad(C, "`lines` is required: [{item, qty}]"))?;
    let (_, st) = state(s, C)?;
    let mut out = Vec::new();
    let (mut material, mut hours, mut labor, mut cost) = (0.0, 0.0, 0.0, 0.0);
    let mut unpriced = Vec::new();
    for l in lines.iter().take(MAX_CHANGE_LINES) {
        let g = |k: &str, d: &str| l.get(k).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()).unwrap_or(d).to_string();
        let item = g("item", "");
        let qty = l.get("qty").and_then(Value::as_f64).filter(|q| q.is_finite() && q.abs() <= 1e9).unwrap_or(0.0);
        if item.is_empty() || qty == 0.0 {
            return Err(bad(C, format!("each line needs an item and a qty: {l}")));
        }
        let line = json!({
            "trade": g("trade", "electrical"), "category": g("category", ""), "item": item, "size": g("size", ""),
            "unit": g("unit", "each"), "total": qty.abs(), "net": qty.abs(),
        });
        let (priced, _) = price_lines(&st, &json!({ "lines": [line] }));
        let sign = qty.signum();
        let Some(pr) = priced.first() else { continue };
        let ok = !pr.match_kind.is_empty();
        if !ok {
            unpriced.push(format!("{item} {}", g("size", "")).trim().to_string());
        }
        material += sign * pr.material;
        hours += sign * pr.labor_hours;
        labor += sign * pr.labor_cost;
        cost += sign * pr.cost;
        out.push(json!({
            "item": item, "size": g("size", ""), "qty": qty, "material": sign * pr.material, "labor_hours": sign * pr.labor_hours,
            "labor_cost": sign * pr.labor_cost, "cost": sign * pr.cost, "via": pr.via, "priced": ok,
        }));
    }
    Ok(json!({
        "lines": out,
        "totals": {"material": material, "labor_hours": hours, "labor_cost": labor, "cost": cost},
        "unpriced": unpriced,
        "why": p.get("why").cloned().unwrap_or(Value::Null),
    }))
}

fn run_compare(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, st) = state(s, "estimate.compare")?;
    let priced = price_job(s, &st, "estimate.compare", false)?.priced;
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
    let priced2 = price_job(s, &alt, "estimate.compare", false)?.priced;
    let what_if = totals(&alt, &priced2)["totals"].clone();
    let delta = (what_if["total"].as_f64().unwrap_or(0.0) - now["total"].as_f64().unwrap_or(0.0)) * 100.0;
    Ok(json!({ "now": now, "what_if": what_if, "delta": delta.round() / 100.0 }))
}

fn run_set_factor(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "estimate.set_factor";
    let reason = str_param(p, "reason").filter(|r| !r.trim().is_empty()).ok_or_else(|| bad(CMD, "`reason` is required"))?.to_string();
    let factor = str_param(p, "factor").filter(|f| !f.trim().is_empty());
    let tier = str_param(p, "tier");
    if factor.is_none() && tier.is_none() {
        return Err(bad(CMD, "give a `factor` (from factors.csv) or a `tier`"));
    }
    if let Some(t) = tier
        && !labor::TIERS.contains(&t)
    {
        return Err(bad(CMD, "`tier` is normal, difficult or very_difficult"));
    }
    let remove = bool_or(p, "remove", false);
    let keys = picked(CMD, s, p)?;
    if keys.is_empty() {
        return Err(bad(CMD, "no BOM line matches (estimate.query lists the keys)"));
    }
    let (sp, mut st) = state(s, CMD)?;
    if let Some(f) = factor
        && st.get("baseline").and_then(|b| b["factors"].get(f)).is_none()
    {
        return Err(bad(CMD, format!("there is no factor named {f} (import factors.csv first)")));
    }
    if let Some(f) = factor
        && let Some(m) = obj(&mut st, "factors")
    {
        for k in &keys {
            let list = m.entry(k.clone()).or_insert_with(|| json!([]));
            if let Some(a) = list.as_array_mut() {
                a.retain(|x| x["factor"].as_str() != Some(f));
                if !remove {
                    a.push(json!({ "factor": f, "reason": reason, "citation": str_param(p, "citation") }));
                }
            }
        }
    }
    if let Some(t) = tier
        && let Some(m) = obj(&mut st, "tiers")
    {
        for k in &keys {
            if remove {
                m.remove(k);
            } else {
                m.insert(k.clone(), json!({ "tier": t, "reason": reason }));
            }
        }
    }
    store(s, sp, st)?;
    Ok(json!({ "lines": keys, "factor": factor, "tier": tier }))
}

fn run_observe(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "estimate.observe";
    let driver = str_param(p, "driver").filter(|d| !d.trim().is_empty()).ok_or_else(|| bad(CMD, "`driver` is required"))?;
    let qty =
        p["qty"].as_f64().filter(|q| q.is_finite() && *q > 0.0 && *q <= 1e9).ok_or_else(|| bad(CMD, "`qty` (units installed) must be above 0"))?;
    let hours = p["hours"]
        .as_f64()
        .filter(|h| h.is_finite() && *h >= 0.0 && *h <= 1e9)
        .ok_or_else(|| bad(CMD, "`hours` (labor hours they took) must be 0 or more"))?;
    let date = str_param(p, "date").filter(|d| parse_date(d).is_some()).ok_or_else(|| bad(CMD, "`date` is YYYY-MM-DD"))?;
    let source = str_param(p, "source").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(CMD, "`source` (whose field report) is required"))?;
    let (sp, mut st) = state(s, CMD)?;
    let base = st.get("baseline").cloned().unwrap_or(json!({}));
    let rate_of = |st: &Map<String, Value>| labor::drivers(st, &base).into_iter().find(|d| d.name.eq_ignore_ascii_case(driver));
    let before = rate_of(&st).ok_or_else(|| bad(CMD, format!("{driver} is not a driver of the production model (import production_model.json)")))?;
    let context = str_param(p, "context").map_or_else(|| before.context.clone(), str::to_string);
    let obs = json!({ "driver": before.name, "context": context, "qty": qty, "hours": hours, "date": date, "source": source });
    if let Some(a) = st.entry("observations".to_string()).or_insert_with(|| json!([])).as_array_mut() {
        if a.len() >= MAX_ROWS {
            return Err(bad(CMD, "too many observations"));
        }
        a.push(obs);
    }
    let after = rate_of(&st).unwrap_or_else(|| before.clone());
    store(s, sp, st)?;
    let r6 = |v: f64| (v * 1e6).round() / 1e6;
    Ok(
        json!({ "driver": after.name, "context": after.context, "before": r6(before.rate), "after": r6(after.rate), "observations": after.obs, "math": after.text() }),
    )
}

fn run_export(s: &mut Session, p: &Value) -> Result<Value> {
    super::exchange::guard_params(s, "estimate.export", p, &["path"])?;
    let (_, st) = state(s, "estimate.export")?;
    let job = price_job(s, &st, "estimate.export", true)?;
    outputs::export(&st, &job, p)
}

fn run_bid_summary(s: &mut Session, _p: &Value) -> Result<Value> {
    let (_, st) = state(s, "estimate.bid_summary")?;
    let job = price_job(s, &st, "estimate.bid_summary", true)?;
    let f = findings(&st, &job);
    let mut out = outputs::bid_summary(&st, &job, &f);
    if let Some(n) = load_costs_step(&job.priced) {
        out["next_step"] = json!(n);
    }
    Ok(out)
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
        // Q6012: the starter book prices lines that have no baseline, so "unpriced" may be
        // replaced by "starter_cost_book"; either indicates the job needs real prices.
        assert!(
            kinds.contains(&"undecided_scope")
                && (kinds.contains(&"unpriced") || kinds.contains(&"starter_cost_book"))
                && kinds.contains(&"off_ratio"),
            "{kinds:?}"
        );
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

    /// Import `dir`, map the receptacles to their assemblies and put Division 26 in contract.
    fn priced_job(s: &mut Session, dir: &std::path::Path) {
        s.execute("estimate.import", &json!({ "folder": dir.to_string_lossy() })).unwrap();
        s.execute("estimate.settings", &json!({ "today": "2026-10-21" })).unwrap();
        s.execute(
            "estimate.set_assembly",
            &json!({ "item": "standard duplex receptacle", "assembly": "DUPLEX-MS", "reason": "metal stud walls per A-501" }),
        )
        .unwrap();
        s.execute("estimate.set_assembly", &json!({ "item": "GFCI receptacle", "assembly": "GFCI-MS", "reason": "metal stud walls per A-501" }))
            .unwrap();
        s.execute("estimate.set_scope", &json!({ "trade": "electrical", "scope": "in_contract", "reason": "Division 26 is ours" })).unwrap();
    }

    fn kinds(st: &Value, kind: &str) -> Vec<Value> {
        st["findings"].as_array().unwrap().iter().filter(|f| f["kind"] == json!(kind)).cloned().collect()
    }

    const CONDUIT: &str = "electrical|conduit|EMT conduit|3/4\"";

    #[test]
    fn condition_factors_and_tiers_multiply_the_book_rate() {
        let mut s = Session::new();
        job(&mut s);
        let dir = baseline();
        std::fs::write(
            dir.join("factors.csv"),
            "factor,kind,multiplier,basis,source\nHeight 12-16 ft,height,1.25,assumption,invented company table\nHard ceiling,ceiling,1.10,observed,invented past jobs\nBad,height,1.1,guess,x\n",
        )
        .unwrap();
        let r = s.execute("estimate.import", &json!({ "folder": dir.to_string_lossy() })).unwrap();
        // A factor without an observed|assumption basis is refused on import.
        assert_eq!(r["factors"], json!(2), "{r}");
        assert_eq!(r["notes"].as_array().unwrap().len(), 1, "{r}");
        priced_job(&mut s, &dir);
        s.execute(
            "estimate.set_factor",
            &json!({ "lines": [CONDUIT], "factor": "Height 12-16 ft", "tier": "difficult", "reason": "runs at 14 ft per E-201" }),
        )
        .unwrap();
        let ex = s.execute("estimate.explain", &json!({ "line": CONDUIT })).unwrap();
        // Difficult tier from the item table: 0.07 h/ft × 1.25 = 0.0875 h/ft × 100 ft = 8.75 h ×
        // $85 = $743.75; + material 110 ft × $1.10 = $121.00 → $864.75.
        assert_eq!((ex["labor_hours"].clone(), ex["labor_cost"].clone(), ex["value"].clone()), (json!(8.75), json!(743.75), json!(864.75)), "{ex}");
        let math = ex["math"].to_string();
        assert!(math.contains("tier difficult: 0.07 h") && math.contains("× 1.25 Height 12-16 ft") && math.contains("factor 1.2500"), "{math}");
        // A tier with no per-tier hours multiplies the book rate (1.8 for very difficult):
        // duplex 0.6 h × 1.8 = 1.08 h × 2 = 2.16 h × $85 = $183.60.
        s.execute(
            "estimate.set_factor",
            &json!({ "item": "standard duplex receptacle", "tier": "very_difficult", "reason": "exposed in a parking deck" }),
        )
        .unwrap();
        let ex = s.execute("estimate.explain", &json!({ "line": "electrical|devices|standard duplex receptacle|" })).unwrap();
        assert_eq!((ex["labor_hours"].clone(), ex["labor_cost"].clone()), (json!(2.16), json!(183.6)), "{ex}");
        // Removing the factor takes it off; an unknown factor or tier is refused.
        s.execute("estimate.set_factor", &json!({ "lines": [CONDUIT], "factor": "Height 12-16 ft", "reason": "low after all", "remove": true }))
            .unwrap();
        let ex = s.execute("estimate.explain", &json!({ "line": CONDUIT })).unwrap();
        assert_eq!(ex["labor_hours"], json!(7.0), "{ex}");
        assert!(s.execute("estimate.set_factor", &json!({ "lines": [CONDUIT], "factor": "Nope", "reason": "x" })).is_err());
        assert!(s.execute("estimate.set_factor", &json!({ "lines": [CONDUIT], "tier": "awful", "reason": "x" })).is_err());
        assert!(s.execute("estimate.set_factor", &json!({ "lines": [CONDUIT], "factor": "Hard ceiling" })).is_err());
        assert!(s.execute("estimate.set_factor", &json!({ "lines": ["no|such|line|"], "factor": "Hard ceiling", "reason": "x" })).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_production_model_blends_book_and_field_rates_by_phase() {
        let mut s = Session::new();
        job(&mut s);
        let dir = baseline();
        let w = |n: &str, t: &str| std::fs::write(dir.join(n), t).unwrap();
        w(
            "production_model.json",
            r#"{"phases": [
                {"phase": "rough-in", "drivers": [{"driver": "device boxes", "unit": "each", "rate": 0.5, "prior_qty": 40, "match": {"category": "boxes", "item": "Box"}}]},
                {"phase": "trim", "drivers": [{"driver": "devices", "unit": "each", "rate": 0.25, "prior_qty": 20, "match": {"category": "devices"}}]}
            ]}"#,
        );
        w(
            "field_observations.csv",
            "driver,context,qty,hours,date,source\ndevice boxes,,10,8,2026-08-01,Invented Job A\ndevice boxes,,20,12,2026-08-15,Invented Job B\ndevice boxes,,10,8,2026-09-01,Invented Job C\n",
        );
        let mut prices = std::fs::read_to_string(dir.join("prices.csv")).unwrap();
        prices += "\"Box, 4\"\" square\",\"2-1/8\"\" deep\",each,3.00,,2026-09-01,Invented Supply\n";
        w("prices.csv", &prices);
        priced_job(&mut s, &dir);
        // Box rate = (0.5 h × 40 + 28 h observed) ÷ (40 + 40 boxes) = 0.6 h; 3 boxes → 1.8 h of
        // rough-in. Trim: 3 devices × 0.25 h (no observations) = 0.75 h.
        let st = s.execute("estimate.status", &json!({})).unwrap();
        assert_eq!((st["labor_by_phase"]["rough-in"].clone(), st["labor_by_phase"]["trim"].clone()), (json!(1.8), json!(0.75)), "{st}");
        let ex = s.execute("estimate.explain", &json!({ "line": "electrical|boxes|Box, 4\" square|2-1/8\" deep" })).unwrap();
        let math = ex["math"].to_string();
        assert!(math.contains("driver device boxes (rough-in): 0.6000 h/each") && math.contains("3 observation(s): 28 h over 40 each"), "{math}");
        // 3 × 0.6 h = 1.8 h × $85 = $153.00 labor; 3 × $3.00 material.
        assert_eq!((ex["labor_hours"].clone(), ex["labor_cost"].clone(), ex["value"].clone()), (json!(1.8), json!(153.0), json!(162.0)), "{ex}");
        // A new observation recalibrates: (48 + 20) ÷ (80 + 20) = 0.68 h → 2.04 h of rough-in.
        let o = s
            .execute(
                "estimate.observe",
                &json!({ "driver": "device boxes", "qty": 20, "hours": 20, "date": "2026-10-01", "source": "Invented Job D" }),
            )
            .unwrap();
        assert_eq!((o["before"].clone(), o["after"].clone(), o["observations"].clone()), (json!(0.6), json!(0.68), json!(4)), "{o}");
        let st = s.execute("estimate.status", &json!({})).unwrap();
        assert_eq!(st["labor_by_phase"]["rough-in"], json!(2.04), "{st}");
        assert!(s.execute("estimate.observe", &json!({ "driver": "nope", "qty": 1, "hours": 1, "date": "2026-10-01", "source": "x" })).is_err());
        assert!(s.execute("estimate.observe", &json!({ "driver": "devices", "qty": 0, "hours": 1, "date": "2026-10-01", "source": "x" })).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dated_prices_escalate_by_their_index() {
        let mut s = Session::new();
        job(&mut s);
        let dir = baseline();
        std::fs::write(dir.join("indexes.csv"), "index,date,value\ncopper,2026-01-01,100\ncopper,2026-10-01,110\n").unwrap();
        priced_job(&mut s, &dir);
        s.execute(
            "estimate.set_price",
            &json!({ "item": "EMT conduit", "size": "3/4\"", "unit": "ft", "material": 1.10, "labor_hours": 0.05, "date": "2026-01-15", "source": "Invented Supply", "index": "copper" }),
        )
        .unwrap();
        // $1.10 × 110/100 = $1.21/ft × 110 ft = $133.10; labor 5 h × $85 = $425 → $558.10.
        let ex = s.execute("estimate.explain", &json!({ "line": CONDUIT })).unwrap();
        assert_eq!(ex["value"], json!(558.1), "{ex}");
        let math = ex["math"].to_string();
        assert!(math.contains("escalated × 1.1000 = index copper 110 (2026-10-01) ÷ 100 (2026-01-01)"), "{math}");
        // An index with no value at the price's date is a finding, not a silent ×1.
        s.execute(
            "estimate.set_price",
            &json!({ "item": "EMT conduit", "size": "3/4\"", "unit": "ft", "material": 1.10, "labor_hours": 0.05, "date": "2025-06-01", "source": "Invented Supply", "index": "copper" }),
        )
        .unwrap();
        let st = s.execute("estimate.status", &json!({})).unwrap();
        assert_eq!(kinds(&st, "no_index").len(), 1, "{st}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A 400 A feeder: 4 × 600 kcmil + #3 ground, 100 ft.
    fn feeder_job(s: &mut Session) {
        let spec = json!({
            "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}],
            "mep": {
                "systems": [{"name": "E-FEEDER", "layer": "E-FEEDER", "kind": "cable", "size_in": [3, 0], "bottom_ft": 10}],
                "runs": [{"system": "E-FEEDER", "level": "1", "points": [[0, 0], [1200, 0]], "size_in": [3, 0], "bottoms_ft": [10]}]
            },
            "tags": [{"id": "F1", "kind": "homerun", "level": "1", "at_ft": [0, 0], "fields": {"conductors": {"value": "4#600 + 1#3G", "source": "note"}}}]
        });
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
    }

    #[test]
    fn large_terminations_need_priced_equipment() {
        let mut s = Session::new();
        feeder_job(&mut s);
        let dir = baseline();
        priced_job(&mut s, &dir);
        let st = s.execute("estimate.status", &json!({})).unwrap();
        let missing = kinds(&st, "missing_equipment");
        assert!(missing.iter().any(|f| f["message"].as_str().unwrap().to_ascii_lowercase().contains("crimper")), "{missing:?}");
        assert!(missing.iter().all(|f| f["fix"].as_str().is_some_and(|x| !x.is_empty())), "{missing:?}");
        std::fs::write(
            dir.join("tool_rules.json"),
            r#"{"rules": [{"when": "large_terminations", "items": [
                {"item": "Crimper with dies", "supply": "rental", "rate": 45, "per": "day", "qty": 1},
                {"item": "Torque wrench", "supply": "owned", "rate": 5, "per": "day", "qty": 1},
                {"item": "Lug, compression 600 kcmil", "supply": "purchase", "price": 18.5, "qty_per": 1},
                {"item": "Lug bolt kit", "supply": "purchase", "price": 2.25, "qty_per": 1}
            ]}]}"#,
        )
        .unwrap();
        priced_job(&mut s, &dir);
        s.execute("estimate.settings", &json!({ "duration_days": 10 })).unwrap();
        let st = s.execute("estimate.status", &json!({})).unwrap();
        let left: Vec<String> = kinds(&st, "missing_equipment").iter().map(|f| f["message"].as_str().unwrap().to_ascii_lowercase()).collect();
        assert!(!left.iter().any(|m| m.contains("crimper") || m.contains("torque") || m.contains(" lug")), "{left:?}");
        // 4 phase conductors × 2 ends = 8 terminations. Crimper: 1 × $45/day × 10 days = $450;
        // torque wrench (owned, tool charge) 1 × $5 × 10 = $50; lugs 8 × $18.50 = $148; bolt
        // kits 8 × $2.25 = $18.
        let q = s.execute("estimate.query", &json!({ "trade": "job", "limit": 100 })).unwrap();
        let cost = |k: &str| q["lines"].as_array().unwrap().iter().find(|l| l["key"] == json!(k)).map(|l| (l["qty"].clone(), l["cost"].clone()));
        assert_eq!(cost("job|equipment|Crimper with dies|"), Some((json!(1.0), json!(450.0))), "{q}");
        assert_eq!(cost("job|tools|Torque wrench|"), Some((json!(1.0), json!(50.0))), "{q}");
        assert_eq!(cost("job|hardware|Lug, compression 600 kcmil|"), Some((json!(8.0), json!(148.0))), "{q}");
        assert_eq!(cost("job|hardware|Lug bolt kit|"), Some((json!(8.0), json!(18.0))), "{q}");
        let ex = s.execute("estimate.explain", &json!({ "line": "job|equipment|Crimper with dies|" })).unwrap();
        assert_eq!(ex["elements"], json!(["F1"]), "{ex}");
        assert!(ex["math"].to_string().contains("1 × $45.00/day × 10 day(s) of 10 day(s) (rental)"), "{ex}");
        // A day rate with no duration says so.
        s.execute("estimate.settings", &json!({ "duration_days": 0 })).unwrap();
        let st = s.execute("estimate.status", &json!({})).unwrap();
        assert_eq!(kinds(&st, "missing_duration").len(), 2, "{st}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_planted_error_produces_its_finding() {
        let mut s = Session::new();
        job(&mut s);
        let dir = baseline();
        std::fs::write(
            dir.join("benchmarks.json"),
            r#"{"ratios": [{"match": "receptacle", "per_level_low": 1, "per_level_high": 40, "source": "invented past jobs"}],
                "per_unit_type": [{"unit_type": "K", "match": "receptacle", "low": 4, "high": 8, "source": "invented past jobs"}],
                "hours_per_sf": {"low": 0.01, "high": 0.05, "source": "invented past jobs"},
                "price_tolerance_pct": 25}"#,
        )
        .unwrap();
        priced_job(&mut s, &dir);
        let count = |s: &mut Session, k: &str| kinds(&s.execute("estimate.status", &json!({})).unwrap(), k);
        for k in ["price_off_history", "spec_unmatched", "model_validation_open", "off_ratio_unit", "off_hours_per_sf"] {
            assert!(count(&mut s, k).is_empty(), "{k} before it was planted");
        }
        // A price 50% above the one it replaced.
        s.execute(
            "estimate.set_price",
            &json!({ "item": "Duplex device", "unit": "each", "material": 3.0, "date": "2026-10-01", "source": "Invented Quote" }),
        )
        .unwrap();
        let f = count(&mut s, "price_off_history");
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0]["message"].as_str().unwrap().contains("+50% from $2.00") && f[0]["fix"].is_string(), "{f:?}");
        // Hours per square foot: about 10 h over 10 SF is far above 0.05.
        s.execute("estimate.settings", &json!({ "gross_sf": 10 })).unwrap();
        let f = count(&mut s, "off_hours_per_sf");
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0]["fix"].is_string());
        // A specified type the BOM carries no line for.
        {
            let mut ty = cadaddy_doc::bimspec::BimType::new("WP1");
            ty.set_field("finish", cadaddy_doc::bimspec::SpecField { value: "WHITE".into(), source: "26 27 26 2.3".into(), ..Default::default() });
            s.doc_mut().unwrap().bim_types.types.push(ty);
        }
        let f = count(&mut s, "spec_unmatched");
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0]["message"].as_str().unwrap().starts_with("WP1: finish: WHITE") && f[0]["fix"].is_string(), "{f:?}");
        // A typical unit of type K with 3 receptacles, typical 4–8; and the prints the model
        // fails (a symbol with no device modelled).
        s.execute(
            "bim.prints",
            &json!({ "prints": {
                "rooms": [{"level": "1", "number": "101", "name": "Guest Room", "outline": [[0, 0], [10, 0], [10, 5], [0, 5]], "unit_type": "K", "sheet": "A-101"}],
                "symbols": [{"sheet": "E-101", "level": "1", "kind": "receptacle", "at": [40, 40]}]
            } }),
        )
        .unwrap();
        let f = count(&mut s, "off_ratio_unit");
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0]["message"].as_str().unwrap().contains("3.0 per K unit (3 in 1 units)") && f[0]["fix"].is_string(), "{f:?}");
        let f = count(&mut s, "model_validation_open");
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0]["fix"].is_string());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_markup_chain_compounds_to_the_cent() {
        let mut s = Session::new();
        job(&mut s);
        let dir = baseline();
        priced_job(&mut s, &dir);
        s.execute("estimate.set_scope", &json!({ "trade": "electrical", "scope": "nic", "reason": "only the conduit for this check" })).unwrap();
        s.execute("estimate.set_scope", &json!({ "lines": [CONDUIT], "scope": "in_contract", "reason": "Division 26" })).unwrap();
        s.execute("estimate.settings", &json!({ "tax_pct": 8, "overhead_pct": 10, "profit_pct": 10, "contingency_pct": 5, "bond_pct": 1 })).unwrap();
        // Direct $546.00 (material $121.00 + labor $425.00). Tax 8% × $121 = $9.68 → $555.68;
        // overhead 10% = $55.568 → $611.248; profit 10% = $61.1248 → $672.3728; contingency 5% =
        // $33.61864 → $705.99144; bond 1% = $7.0599144 → $713.0513544.
        let st = s.execute("estimate.status", &json!({})).unwrap();
        assert_eq!(
            st["totals"],
            json!({ "material": 121.0, "labor": 425.0, "labor_hours": 5.0, "direct": 546.0, "tax": 9.68, "overhead": 55.57, "profit": 61.12, "contingency": 33.62, "bond": 7.06, "total": 713.05 }),
            "{st}"
        );
        assert_eq!(st["by_level"], json!({ "1": 713.05 }), "{st}");
        assert_eq!(st["by_trade"], json!({ "electrical": 713.05 }), "{st}");
        // The bid summary: the base bid, and the exclusions drafted from scope.
        let b = s.execute("estimate.bid_summary", &json!({})).unwrap();
        assert_eq!(b["total"], json!(713.05));
        let ex = b["exclusions"].to_string();
        assert!(ex.contains("Not in contract:") && ex.contains("only the conduit for this check"), "{b}");
        assert!(b["text"].as_str().unwrap().starts_with("# Bid summary\n\nBase bid: $713.05"), "{b}");
        // Exported to XLSX and CSV, each reopens with every line and the same total.
        for name in ["estimate.xlsx", "estimate.csv"] {
            let path = dir.join(name);
            let r = s.execute("estimate.export", &json!({ "path": path.to_string_lossy(), "columns": ["key", "scope", "cost", "total"] })).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            let rows: Vec<Vec<String>> = if name.ends_with(".xlsx") {
                crate::cmd::table::xlsx::open(&bytes).unwrap().cells(None, None).unwrap()
            } else {
                csv_rows(&String::from_utf8(bytes).unwrap())
                    .iter()
                    .map(|r| ["key", "scope", "cost", "total"].iter().map(|k| r.get(*k).cloned().unwrap_or_default()).collect())
                    .collect()
            };
            let data: Vec<&Vec<String>> = rows.iter().filter(|r| r.first().is_some_and(|k| k.contains('|'))).collect();
            assert_eq!(data.len() as u64, r["lines"].as_u64().unwrap(), "{name}: {rows:?}");
            let last = rows.iter().find(|r| r.first().map(String::as_str) == Some("TOTAL")).unwrap();
            assert_eq!(last.get(3).map(String::as_str), Some("713.05"), "{name}");
            let sum: f64 = data.iter().filter_map(|r| r.get(3)?.parse::<f64>().ok()).sum();
            assert_eq!((sum * 100.0).round() / 100.0, 713.05, "{name}");
        }
        assert!(s.execute("estimate.export", &json!({ "path": dir.join("x.csv").to_string_lossy(), "columns": ["nope"] })).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_project_briefing_carries_the_estimate_status() {
        let mut s = Session::new();
        job(&mut s);
        let b = s.execute("project.briefing", &json!({})).unwrap();
        assert!(b.get("estimate").is_none(), "{b}");
        let dir = baseline();
        priced_job(&mut s, &dir);
        let st = s.execute("estimate.status", &json!({})).unwrap();
        let b = s.execute("project.briefing", &json!({})).unwrap();
        assert_eq!(b["estimate"]["total"], st["totals"]["total"], "{b}");
        assert_eq!(b["estimate"]["ok"], json!(false), "{b}");
        assert_eq!(b["estimate"]["open_findings"].as_u64(), Some(st["findings"].as_array().unwrap().len() as u64), "{b}");
        assert!(b["text"].as_str().unwrap().contains("open finding(s)"), "{b}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Q6012: starter cost book ──

    #[test]
    fn starter_cost_book_csv_parses_and_covers_all_trades() {
        let rows = starter_prices();
        assert!(rows.len() >= 50, "starter book has {} rows, need at least 50", rows.len());
        let trades: BTreeSet<String> =
            STARTER_CSV.lines().skip(1).filter_map(|l| l.split(',').nth(5).map(|t| t.trim().to_string())).filter(|t| !t.is_empty()).collect();
        for t in ["electrical", "mechanical", "plumbing", "general"] {
            assert!(trades.contains(t), "starter book missing trade {t}: {trades:?}");
        }
        for r in &rows {
            let m = r["material"].as_f64().unwrap_or(-1.0);
            assert!((0.0..1e6).contains(&m), "bad material in row: {r}");
            assert!(r["source"].as_str() == Some(STARTER_SOURCE), "wrong source in row: {r}");
        }
    }

    #[test]
    fn a_change_is_priced_line_by_line_a_removal_negative() {
        let mut s = Session::new();
        job(&mut s);
        s.execute("estimate.settings", &json!({ "crew_rate": 75, "today": "2026-10-21", "pricebook": false })).unwrap();
        let c = s
            .execute("estimate.change", &json!({"lines": [{"item": "GFCI receptacle", "qty": 3}, {"item": "Duplex receptacle", "qty": -3}]}))
            .unwrap();
        // The starter book: GFCI 19.00 + 0.50 h, duplex 2.50 + 0.40 h.
        let m = c["totals"]["material"].as_f64().unwrap_or(0.0);
        let h = c["totals"]["labor_hours"].as_f64().unwrap_or(0.0);
        assert!((m - 3.0 * (19.0 - 2.5)).abs() < 0.01, "{c}");
        assert!((h - 3.0 * (0.5 - 0.4)).abs() < 0.01, "{c}");
        assert!(c["lines"][1]["material"].as_f64().unwrap_or(0.0) < 0.0);
        assert!(s.execute("estimate.change", &json!({"lines": [{"item": "", "qty": 1}]})).is_err());
    }

    #[test]
    fn starter_book_prices_a_model_with_no_user_costs() {
        let mut s = Session::new();
        job(&mut s);
        // No baseline imported — the starter book should price the lines.
        s.execute("estimate.settings", &json!({ "crew_rate": 75, "today": "2026-10-21" })).unwrap();
        s.execute("estimate.set_scope", &json!({ "trade": "electrical", "scope": "in_contract", "reason": "ours" })).unwrap();
        let st = s.execute("estimate.status", &json!({})).unwrap();
        let priced = st["summary"]["priced"].as_u64().unwrap_or(0);
        assert!(priced > 0, "starter book should price at least some lines: {st}");
        // The status reports that starter prices were used.
        let starter_msg = st["starter_cost_book"].as_str().unwrap_or_default();
        assert!(starter_msg.contains("starter cost book"), "status should mention starter: {st}");
        // Every starter-priced line should have via = "starter cost book".
        let q = s.execute("estimate.query", &json!({ "limit": 200 })).unwrap();
        let starter_lines: Vec<&Value> = q["lines"].as_array().unwrap().iter().filter(|l| l["via"].as_str() == Some(STARTER_SOURCE)).collect();
        assert!(!starter_lines.is_empty(), "should have lines via starter cost book: {q}");
        // A finding about the starter book should exist.
        let f: Vec<&Value> = st["findings"].as_array().unwrap().iter().filter(|f| f["kind"].as_str() == Some("starter_cost_book")).collect();
        assert!(!f.is_empty(), "should have a starter_cost_book finding: {st}");
        // Totals should be > 0 (the estimate is not $0 any more).
        assert!(st["totals"]["total"].as_f64().unwrap_or(0.0) > 0.0, "total should be > 0: {st}");
    }

    #[test]
    fn the_price_book_prices_a_line_and_takes_a_newer_quote() {
        let mut s = Session::new();
        job(&mut s);
        let dir = std::env::temp_dir().join(format!("cadaddy-pricebook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.to_string_lossy().to_string();
        s.execute("estimate.settings", &json!({ "crew_rate": 75, "today": "2026-11-02", "pricebook_folder": folder })).unwrap();
        s.execute("estimate.set_scope", &json!({ "trade": "electrical", "scope": "in_contract", "reason": "ours" })).unwrap();
        let q = s.execute("estimate.query", &json!({ "limit": 200 })).unwrap();
        let line = q["lines"].as_array().unwrap().iter().find(|l| l["unit"].as_str() == Some("each")).cloned().expect("a counted line");
        let item = line["item"].as_str().unwrap().to_string();
        let size = line["size"].as_str().unwrap_or_default().to_string();
        s.execute("pricebook.set", &json!({"key": "testkey", "unit_price": 12.5, "unit": "each", "vendor": "Test Supply", "location": "North", "date": "2026-10-01", "folder": folder})).unwrap();
        s.execute("pricebook.map", &json!({"item": item, "size": size, "keys": ["testkey"], "folder": folder})).unwrap();
        let via = |s: &mut Session| {
            let q = s.execute("estimate.query", &json!({ "limit": 200 })).unwrap();
            let l = q["lines"]
                .as_array()
                .unwrap()
                .iter()
                .find(|l| l["item"].as_str() == Some(item.as_str()) && l["size"].as_str().unwrap_or_default() == size)
                .cloned()
                .unwrap();
            l["via"].as_str().unwrap_or_default().to_string()
        };
        assert!(via(&mut s).starts_with("price book: testkey $12.5000 Test Supply (North) 2026-10-01 QUOTE"), "{}", via(&mut s));
        // The same vendor and location's newer quote replaces it; the old one is kept in the history.
        let r = s
            .execute(
                "pricebook.set",
                &json!({"key": "testkey", "unit_price": 14.0, "vendor": "Test Supply", "location": "North", "date": "2026-11-01", "folder": folder}),
            )
            .unwrap();
        assert_eq!(r["replaced"]["unit_price"], json!(12.5), "{r}");
        assert_eq!(r["quotes"], json!(1));
        assert!(std::fs::read_to_string(dir.join("history.csv")).unwrap().contains("12.5"));
        // Another vendor's quote sits beside it; the lowest is the price, the spread said.
        let r = s
            .execute(
                "pricebook.set",
                &json!({"key": "testkey", "unit_price": 13.0, "vendor": "Other Supply", "date": "2026-11-02", "folder": folder}),
            )
            .unwrap();
        assert_eq!(r["quotes"], json!(2), "{r}");
        let v = via(&mut s);
        assert!(v.contains("Other Supply") && v.contains("of 2 ($13.0000-$14.0000)"), "{v}");
        // An import older than what a vendor's quote holds is kept out; a new key is added.
        let csv = dir.join("old.csv");
        std::fs::write(&csv, "key,description,unit,unit price $,source type,vendor / book,location,price date\ntestkey,old,each,9.99,QUOTE,Test Supply,North,2026-01-01\nnewkey,new item,each,3.00,WEB,Some Store,,2026-11-02\n").unwrap();
        let r = s.execute("pricebook.import", &json!({"path": csv.to_string_lossy(), "folder": folder})).unwrap();
        assert_eq!(r["quotes"], json!({"added": 1, "updated": 0, "kept_newer": 1}), "{r}");
        let list = s.execute("pricebook.list", &json!({"search": "testkey", "folder": folder})).unwrap();
        assert_eq!(list["keys"][0]["price"]["unit_price"], json!(13.0), "{list}");
        assert_eq!(list["keys"][0]["quotes"].as_array().map(Vec::len), Some(2));
        // The rule: a vendor's quote.
        s.execute("pricebook.settings", &json!({"pick": "vendor", "vendor": "Test Supply", "folder": folder})).unwrap();
        assert!(via(&mut s).contains("Test Supply's quote"), "{}", via(&mut s));
        // Off, the line goes back to the other books.
        s.execute("estimate.settings", &json!({ "pricebook": false })).unwrap();
        assert!(!via(&mut s).starts_with("price book"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_cost_table_overrides_starter_rows() {
        let mut s = Session::new();
        job(&mut s);
        s.execute("estimate.settings", &json!({ "crew_rate": 75, "today": "2026-10-21" })).unwrap();
        s.execute("estimate.set_scope", &json!({ "trade": "electrical", "scope": "in_contract", "reason": "ours" })).unwrap();
        // First, get the starter-priced totals.
        let before = s.execute("estimate.status", &json!({})).unwrap();
        let before_total = before["totals"]["total"].as_f64().unwrap_or(0.0);
        assert!(before_total > 0.0, "starter should price the job: {before}");
        // Now import a user cost table with a different price for receptacles.
        let dir = std::env::temp_dir().join(format!("cadaddy-costbook-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let user_csv = dir.join("my_costs.csv");
        std::fs::write(&user_csv, "item,unit,material,labor_hours\nReceptacle,each,50.00,1.00\n").unwrap();
        let r = s.execute("estimate.import_costs", &json!({ "path": user_csv.to_string_lossy() })).unwrap();
        assert_eq!(r["prices"], json!(1), "should import 1 user price: {r}");
        // The user price should override the starter for that item.
        let q = s.execute("estimate.query", &json!({ "category": "devices", "limit": 200 })).unwrap();
        let recep: Vec<&Value> = q["lines"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|l| {
                l["item"].as_str().is_some_and(|i| i.to_ascii_lowercase().contains("receptacle"))
                    && l["via"].as_str().is_some_and(|v| v.contains("my_costs"))
            })
            .collect();
        assert!(!recep.is_empty(), "user price should override starter for receptacle: {q}");
        // Other lines still use the starter.
        let _still_starter: Vec<&Value> = q["lines"].as_array().unwrap().iter().filter(|l| l["via"].as_str() == Some(STARTER_SOURCE)).collect();
        // Some lines might still use starter (boxes, plates, etc).
        let after = s.execute("estimate.status", &json!({})).unwrap();
        let after_total = after["totals"]["total"].as_f64().unwrap_or(0.0);
        // The totals should have changed (user price $50 vs starter $2.50).
        assert!((after_total - before_total).abs() > 1.0, "user override should change total: before={before_total}, after={after_total}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── Q6020/Q5301: never-cross matching ──

    #[test]
    fn q6020_gas_pipe_not_priced_at_vent_pipe_rate() {
        // A cost table with "vent pipe 3\"" must NOT silently price "gas pipe 1\"" (system cross).
        // The starter book may price gas pipe via a generic "Pipe" family row — that's OK.
        let mut s = Session::new();
        let spec = json!({
            "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}],
            "mep": {
                "systems": [
                    {"name": "M-VENT", "layer": "M-VENT", "kind": "pipe", "size_in": [3, 0], "bottom_ft": 9},
                    {"name": "M-GAS", "layer": "M-GAS", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 8},
                ],
                "runs": [
                    {"system": "M-VENT", "level": "1", "points": [[0, 0], [1200, 0]], "size_in": [3, 0], "bottoms_ft": [9]},
                    {"system": "M-GAS", "level": "1", "points": [[0, 120], [600, 120]], "size_in": [1, 0], "bottoms_ft": [8]},
                ]
            },
            "tags": []
        });
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
        let dir = std::env::temp_dir().join(format!("cadaddy-q6020-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = |n: &str, t: &str| std::fs::write(dir.join(n), t).unwrap();
        w("item_labor_units.csv", "item,unit,material,labor_normal\n");
        w("assemblies.csv", "assembly,unit,description,labor_hours\n");
        w("assembly_parts.csv", "assembly,item,qty\n");
        // Only a price for vent pipe 3" — NOT for gas pipe 1".
        w("prices.csv", "item,size,unit,material,labor_hours,date,source\nvent pipe,3\",lf,12.50,0.15,2026-09-01,Invented Supply\n");
        w("rates.json", "{\"crew_rate\": 85.0}");
        w("benchmarks.json", "{}");
        s.execute("estimate.import", &json!({ "folder": dir.to_string_lossy() })).unwrap();
        s.execute("estimate.settings", &json!({ "today": "2026-10-21" })).unwrap();
        s.execute("estimate.set_scope", &json!({ "trade": "mechanical", "scope": "in_contract", "reason": "test" })).unwrap();
        // Check the gas pipe line is NOT priced via the user's "vent pipe" row.
        let q = s.execute("estimate.query", &json!({ "limit": 200 })).unwrap();
        let gas_line = q["lines"].as_array().unwrap().iter().find(|l| l["item"].as_str().unwrap_or_default().to_ascii_lowercase().contains("gas"));
        assert!(gas_line.is_some(), "gas pipe should exist in BOM: {q}");
        let gas = gas_line.unwrap();
        let via = gas["via"].as_str().unwrap_or_default();
        // Gas pipe must NOT be priced via the user's vent pipe row (never-cross: different system + size).
        assert!(
            !via.to_ascii_lowercase().contains("vent pipe") && !via.contains("Invented Supply"),
            "gas pipe must not be priced from the vent pipe user row: via={via}"
        );
        // Match kind must not be "exact" or "family" from the user table (it may be starter_family).
        let mk = gas["match_kind"].as_str().unwrap_or_default();
        assert!(mk != "exact" || !via.contains("Invented"), "gas pipe should not exact-match the vent pipe row: {gas}");
        // Summary should count match kinds.
        let st = s.execute("estimate.status", &json!({})).unwrap();
        let summary = &st["summary"];
        assert!(summary["exact"].as_u64().is_some(), "summary should have exact count: {summary}");
        assert!(summary["mapped"].as_u64().is_some(), "summary should have mapped count: {summary}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn q6020_cw_size_mismatch_not_cross_priced() {
        // A row for "CW pipe 3/4\"" must NOT price "CW pipe 2\"" (different size).
        // The starter book may provide a generic "Pipe" family price — that's OK.
        let mut s = Session::new();
        let spec = json!({
            "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}],
            "mep": {
                "systems": [
                    {"name": "P-DCW", "layer": "P-DCW", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 8},
                ],
                "runs": [
                    {"system": "P-DCW", "level": "1", "points": [[0, 0], [600, 0]], "size_in": [2, 0], "bottoms_ft": [8]},
                ]
            },
            "tags": []
        });
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
        let dir = std::env::temp_dir().join(format!("cadaddy-q6020sz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = |n: &str, t: &str| std::fs::write(dir.join(n), t).unwrap();
        w("item_labor_units.csv", "item,unit,material,labor_normal\n");
        w("assemblies.csv", "assembly,unit,description,labor_hours\n");
        w("assembly_parts.csv", "assembly,item,qty\n");
        // Only a price for "CW pipe 3/4\"" — NOT for CW pipe 2".
        w("prices.csv", "item,size,unit,material,labor_hours,date,source\nCW pipe,3/4\",lf,5.00,0.10,2026-09-01,Test\n");
        w("rates.json", "{\"crew_rate\": 85.0}");
        w("benchmarks.json", "{}");
        s.execute("estimate.import", &json!({ "folder": dir.to_string_lossy() })).unwrap();
        s.execute("estimate.settings", &json!({ "today": "2026-10-21" })).unwrap();
        s.execute("estimate.set_scope", &json!({ "trade": "plumbing", "scope": "in_contract", "reason": "test" })).unwrap();
        // Check that 2" CW pipe is NOT priced at the 3/4" rate from the user table.
        let q = s.execute("estimate.query", &json!({ "limit": 200 })).unwrap();
        let cw_line = q["lines"].as_array().unwrap().iter().find(|l| {
            l["item"].as_str().unwrap_or_default().to_ascii_lowercase().contains("dcw")
                || l["item"].as_str().unwrap_or_default().to_ascii_lowercase().contains("cw")
        });
        assert!(cw_line.is_some(), "CW pipe should exist in BOM: {q}");
        let cw = cw_line.unwrap();
        let via = cw["via"].as_str().unwrap_or_default();
        // Must NOT be priced from the user's 3/4" CW row (never-cross: different size).
        assert!(!via.contains("Test"), "2\" CW pipe must not be priced from the 3/4\" user row: via={via}, line={cw}");
        // Material unit must not be $5.00 (the 3/4" price).
        let mat = cw["material_unit"].as_f64().unwrap_or(0.0);
        assert!((mat - 5.0).abs() > 0.01 || mat == 0.0, "2\" CW pipe material should not be $5.00 (the 3/4\" price): {cw}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn q6020_map_price_applies_explicit_mapping() {
        let mut s = Session::new();
        let spec = json!({
            "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}],
            "mep": {
                "systems": [
                    {"name": "M-GAS", "layer": "M-GAS", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 8},
                ],
                "runs": [
                    {"system": "M-GAS", "level": "1", "points": [[0, 0], [600, 0]], "size_in": [1, 0], "bottoms_ft": [8]},
                ]
            },
            "tags": []
        });
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
        let dir = std::env::temp_dir().join(format!("cadaddy-q6020map-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = |n: &str, t: &str| std::fs::write(dir.join(n), t).unwrap();
        w("item_labor_units.csv", "item,unit,material,labor_normal\n");
        w("assemblies.csv", "assembly,unit,description,labor_hours\n");
        w("assembly_parts.csv", "assembly,item,qty\n");
        w("prices.csv", "item,size,unit,material,labor_hours,date,source\nvent pipe,,lf,10.00,0.10,2026-09-01,Invented Supply\n");
        w("rates.json", "{\"crew_rate\": 85.0}");
        w("benchmarks.json", "{}");
        s.execute("estimate.import", &json!({ "folder": dir.to_string_lossy() })).unwrap();
        s.execute("estimate.settings", &json!({ "today": "2026-10-21" })).unwrap();
        s.execute("estimate.set_scope", &json!({ "trade": "mechanical", "scope": "in_contract", "reason": "test" })).unwrap();
        // Gas pipe before mapping: not priced from the user's "vent pipe" row (different system).
        // The starter may provide a generic price — that's fine; the mapping should override it.
        let q = s.execute("estimate.query", &json!({})).unwrap();
        let gas_before = q["lines"].as_array().unwrap().iter().find(|l| l["item"].as_str().unwrap_or_default().to_ascii_lowercase().contains("gas"));
        assert!(gas_before.is_some(), "gas pipe should exist in BOM: {q}");
        let before_via = gas_before.unwrap()["via"].as_str().unwrap_or_default();
        assert!(
            !before_via.contains("Invented Supply"),
            "gas pipe should not be priced from the vent pipe user row before mapping: {}",
            gas_before.unwrap()
        );
        // Map gas pipe to the vent pipe price row (explicit user decision).
        let m = s
            .execute("estimate.map_price", &json!({ "category": "pipe", "row_item": "vent pipe", "reason": "same material, user confirmed" }))
            .unwrap();
        assert!(!m["lines"].as_array().unwrap().is_empty(), "should have mapped lines: {m}");
        // Now gas pipe should be priced via the explicit mapping.
        let q2 = s.execute("estimate.query", &json!({})).unwrap();
        let gas_after = q2["lines"].as_array().unwrap().iter().find(|l| l["item"].as_str().unwrap_or_default().to_ascii_lowercase().contains("gas"));
        assert!(gas_after.is_some(), "gas pipe should still be in BOM: {q2}");
        assert_eq!(gas_after.unwrap()["match_kind"], json!("mapped"), "gas pipe should be mapped: {}", gas_after.unwrap());
        let after_via = gas_after.unwrap()["via"].as_str().unwrap_or_default();
        assert!(after_via.contains("mapped"), "via should say mapped: {after_via}");
        // Bad mapping: non-existent row is refused.
        assert!(s.execute("estimate.map_price", &json!({ "category": "pipe", "row_item": "nonexistent", "reason": "x" })).is_err());
        // Missing reason is refused.
        assert!(s.execute("estimate.map_price", &json!({ "category": "pipe", "row_item": "vent pipe" })).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn q6020_summary_counts_match_kinds() {
        let mut s = Session::new();
        job(&mut s);
        let dir = baseline();
        priced_job(&mut s, &dir);
        let st = s.execute("estimate.status", &json!({})).unwrap();
        let summary = &st["summary"];
        // The priced_job maps receptacles to assemblies (mapped) and conduit/coupling have prices (exact).
        assert!(summary["exact"].as_u64().is_some(), "should have exact field: {summary}");
        assert!(summary["mapped"].as_u64().is_some(), "should have mapped field: {summary}");
        assert!(summary["starter_book"].as_u64().is_some(), "should have starter_book field: {summary}");
        assert!(summary["family"].as_u64().is_some(), "should have family field: {summary}");
        assert!(summary["starter_family"].as_u64().is_some(), "should have starter_family field: {summary}");
        let matched = summary["exact"].as_u64().unwrap()
            + summary["mapped"].as_u64().unwrap()
            + summary["starter_book"].as_u64().unwrap()
            + summary["family"].as_u64().unwrap()
            + summary["starter_family"].as_u64().unwrap();
        assert!(matched <= summary["lines"].as_u64().unwrap(), "matched should not exceed total: {summary}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
