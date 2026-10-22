//! The whole-job estimate (Q340): every trade's quantities rolled into one estimate by CSI
//! division, priced per trade, with general conditions, bid packages, bid leveling and bid
//! history.
//!
//! - `wholejob.quantities {trade, division?, lines} | {from_bom: true}`: a trade's takeoff (the
//!   envelope, finishes, sitework readers give theirs; MEP comes from `bim.bom`).
//! - `wholejob.scope {trade, inclusions?, exclusions?, alternates?}`: a trade's scope of work.
//! - `wholejob.prices {trade, quality, source, date?, rows}`: a trade's unit prices, marked by data
//!   quality: `private` (the company's history), `public` (the public pack) or `assumed`.
//!   Electrical prices are private only (the public pack carries no electrical labor).
//! - `wholejob.settings {markups?, gc?, electrical_from_estimate?}`: trade markups (data,
//!   editable) and the general conditions' rates.
//! - `wholejob.estimate`: by division and trade: priced lines, what isn't priced, subtotal,
//!   markup, contract value and its data quality; general conditions from the duration; the total.
//! - `wholejob.package {trade, path?}`: a bid package ready to send to subs.
//! - `wholejob.bid {trade, bidder, total, lines, exclusions?}` and `wholejob.level {trade}`: sub
//!   bids against the model's quantities and scope: missing scope, quantity gaps, outlier prices,
//!   exclusions, and the recommended low COMPLETE bid.
//! - `wholejob.history {pack, private?, trade, …}`, `wholejob.benchmarks {packs}`,
//!   `wholejob.outcome {pack, …}`: public bid tabulations into $/SF benchmarks by trade, building
//!   type, region and date; electrical (any source) and every outcome only into a private pack.
//!
//! No prices ship with CADaddy: tests use invented ones.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_walk3d::BuildingSpec;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const MAX_LINES: usize = 100_000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WholeJob {
    pub trades: BTreeMap<String, Trade>,
    pub prices: BTreeMap<String, PricePack>,
    pub markups: BTreeMap<String, f64>,
    pub gc: Gc,
    pub bids: Vec<Bid>,
    pub electrical_from_estimate: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Trade {
    pub division: String,
    pub lines: Vec<QLine>,
    pub inclusions: Vec<String>,
    pub exclusions: Vec<String>,
    pub alternates: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QLine {
    pub item: String,
    pub unit: String,
    pub qty: f64,
    pub spec_section: String,
    pub source: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PricePack {
    /// `private`, `public` or `assumed`.
    pub quality: String,
    pub source: String,
    pub date: String,
    pub rows: Vec<Price>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Price {
    pub item: String,
    pub unit: String,
    /// Installed cost per unit (material and labor).
    pub unit_cost: f64,
}

/// General conditions: time-based items per week and percentage items.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Gc {
    pub weeks: f64,
    pub supervision_per_week: f64,
    pub temp_facilities_per_week: f64,
    pub equipment_per_week: f64,
    pub cleanup_per_week: f64,
    pub insurance_pct: f64,
    pub bond_pct: f64,
}

impl Default for Gc {
    fn default() -> Self {
        // Invented placeholders: the company's own data replaces them.
        Gc {
            weeks: 0.0,
            supervision_per_week: 3_000.0,
            temp_facilities_per_week: 600.0,
            equipment_per_week: 900.0,
            cleanup_per_week: 400.0,
            insurance_pct: 1.0,
            bond_pct: 1.0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bid {
    pub trade: String,
    pub bidder: String,
    pub total: f64,
    pub lines: Vec<BidLine>,
    pub exclusions: Vec<String>,
    pub alternates: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BidLine {
    pub item: String,
    pub unit: String,
    pub qty: f64,
    pub amount: f64,
}

/// CSI divisions by trade, and a trade's typical markup (percent; data, editable).
const TRADES: &[(&str, &str, &str, f64)] = &[
    ("general", "01", "General Requirements", 0.0),
    ("concrete", "03", "Concrete", 10.0),
    ("masonry", "04", "Masonry", 12.0),
    ("metals", "05", "Metals", 10.0),
    ("carpentry", "06", "Wood, Plastics and Composites", 10.0),
    ("roofing", "07", "Thermal and Moisture Protection", 12.0),
    ("envelope", "07", "Thermal and Moisture Protection", 12.0),
    ("openings", "08", "Openings", 10.0),
    ("drywall", "09", "Finishes", 10.0),
    ("painting", "09", "Finishes", 12.0),
    ("flooring", "09", "Finishes", 10.0),
    ("ceilings", "09", "Finishes", 10.0),
    ("tile", "09", "Finishes", 12.0),
    ("finishes", "09", "Finishes", 10.0),
    ("specialties", "10", "Specialties", 10.0),
    ("fire protection", "21", "Fire Suppression", 10.0),
    ("plumbing", "22", "Plumbing", 12.0),
    ("mechanical", "23", "HVAC", 12.0),
    ("hvac", "23", "HVAC", 12.0),
    ("electrical", "26", "Electrical", 12.0),
    ("communications", "27", "Communications", 12.0),
    ("fire alarm", "28", "Electronic Safety and Security", 12.0),
    ("earthwork", "31", "Earthwork", 10.0),
    ("site", "32", "Exterior Improvements", 10.0),
    ("landscaping", "32", "Exterior Improvements", 12.0),
    ("utilities", "33", "Utilities", 10.0),
];

pub(crate) fn trade_info(trade: &str) -> (&'static str, &'static str, f64) {
    let t = trade.trim().to_ascii_lowercase();
    TRADES.iter().find(|x| x.0 == t).map(|x| (x.1, x.2, x.3)).unwrap_or(("00", "Other", 10.0))
}

/// Electrical work: division 26, 27 or 28, or an electrical trade name.
pub fn is_electrical(trade: &str, division: &str) -> bool {
    matches!(division.trim(), "26" | "27" | "28") || trade.to_ascii_lowercase().contains("electric")
}

fn spec(cmd: &str, s: &Session) -> Result<BuildingSpec> {
    let text = s.doc()?.building_spec.clone().ok_or_else(|| bad(cmd, "the drawing has no building spec (bspec.set)"))?;
    BuildingSpec::from_json(&text).map_err(|e| bad(cmd, e))
}

fn load(sp: &BuildingSpec) -> WholeJob {
    serde_json::from_value(sp.wholejob.clone()).unwrap_or_default()
}

fn save(s: &mut Session, mut sp: BuildingSpec, w: &WholeJob) -> Result<()> {
    sp.wholejob = serde_json::to_value(w).unwrap_or(Value::Null);
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(())
}

fn trade_param(cmd: &str, p: &Value) -> Result<String> {
    str_param(p, "trade").map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty() && t.len() < 60).ok_or_else(|| bad(cmd, "trade required"))
}

pub(crate) fn strings(p: &Value, k: &str) -> Option<Vec<String>> {
    p.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|x| x.trim().chars().take(300).collect()).take(1_000).collect())
}

pub(crate) fn norm(s: &str) -> String {
    s.trim().to_ascii_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn run_quantities(s: &mut Session, p: &Value) -> Result<Value> {
    if bool_or(p, "from_bom", false) {
        let b = s.execute("bim.bom", &json!({})).map_err(|e| bad("wholejob.quantities", format!("the bill of materials: {e}")))?;
        let sp = spec("wholejob.quantities", s)?;
        let mut w = load(&sp);
        let mut by: BTreeMap<String, Vec<QLine>> = BTreeMap::new();
        for l in b["lines"].as_array().into_iter().flatten().take(MAX_LINES) {
            let trade = l["trade"].as_str().unwrap_or("other").to_ascii_lowercase();
            let item = [l["item"].as_str().unwrap_or(""), l["size"].as_str().unwrap_or("")]
                .iter()
                .filter(|x| !x.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
            by.entry(trade).or_default().push(QLine {
                item,
                unit: l["unit"].as_str().unwrap_or("each").into(),
                qty: l["total"].as_f64().or_else(|| l["net"].as_f64()).unwrap_or(0.0),
                spec_section: String::new(),
                source: "bim.bom".into(),
            });
        }
        let n: Vec<Value> = by.iter().map(|(t, ls)| json!({"trade": t, "lines": ls.len()})).collect();
        for (t, ls) in by {
            let e = w.trades.entry(t.clone()).or_default();
            e.division = trade_info(&t).0.into();
            e.lines = ls;
        }
        save(s, sp, &w)?;
        return Ok(json!({"trades": n}));
    }
    let trade = trade_param("wholejob.quantities", p)?;
    let lines: Vec<QLine> = p
        .get("lines")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("wholejob.quantities", "lines required: [{item, unit, qty}]"))?
        .iter()
        .take(MAX_LINES)
        .map(|l| -> Result<QLine> {
            let item =
                l["item"].as_str().map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad("wholejob.quantities", "each line needs an item"))?;
            let qty = l["qty"]
                .as_f64()
                .filter(|v| v.is_finite() && (0.0..1e12).contains(v))
                .ok_or_else(|| bad("wholejob.quantities", format!("{item}: qty must be 0 or more")))?;
            Ok(QLine {
                item: item.chars().take(300).collect(),
                unit: l["unit"].as_str().unwrap_or("ea").trim().to_ascii_lowercase(),
                qty,
                spec_section: l["spec_section"].as_str().unwrap_or("").into(),
                source: l["source"].as_str().unwrap_or("").into(),
            })
        })
        .collect::<Result<_>>()?;
    let sp = spec("wholejob.quantities", s)?;
    let mut w = load(&sp);
    let e = w.trades.entry(trade.clone()).or_default();
    e.division = str_param(p, "division").map(|d| d.trim().chars().take(2).collect()).unwrap_or_else(|| trade_info(&trade).0.into());
    if bool_or(p, "replace", true) {
        e.lines = lines;
    } else {
        e.lines.extend(lines);
    }
    let n = e.lines.len();
    save(s, sp, &w)?;
    Ok(json!({"trade": trade, "lines": n}))
}

fn run_scope(s: &mut Session, p: &Value) -> Result<Value> {
    let trade = trade_param("wholejob.scope", p)?;
    let sp = spec("wholejob.scope", s)?;
    let mut w = load(&sp);
    let e = w.trades.entry(trade.clone()).or_default();
    if e.division.is_empty() {
        e.division = trade_info(&trade).0.into();
    }
    if let Some(v) = strings(p, "inclusions") {
        e.inclusions = v;
    }
    if let Some(v) = strings(p, "exclusions") {
        e.exclusions = v;
    }
    if let Some(v) = strings(p, "alternates") {
        e.alternates = v;
    }
    let out = json!({"trade": trade, "inclusions": e.inclusions, "exclusions": e.exclusions, "alternates": e.alternates});
    save(s, sp, &w)?;
    Ok(out)
}

fn run_prices(s: &mut Session, p: &Value) -> Result<Value> {
    let trade = trade_param("wholejob.prices", p)?;
    let quality = str_param(p, "quality")
        .map(|q| q.trim().to_ascii_lowercase())
        .filter(|q| ["private", "public", "assumed"].contains(&q.as_str()))
        .ok_or_else(|| bad("wholejob.prices", "quality is private, public or assumed"))?;
    let source = str_param(p, "source")
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .ok_or_else(|| bad("wholejob.prices", "source required (where the prices come from)"))?;
    let sp = spec("wholejob.prices", s)?;
    let mut w = load(&sp);
    let division = w.trades.get(&trade).map(|t| t.division.clone()).unwrap_or_else(|| trade_info(&trade).0.into());
    if quality == "public" && is_electrical(&trade, &division) {
        return Err(bad(
            "wholejob.prices",
            "electrical prices come from the company's private pack (or the electrical estimate): the public pack carries no electrical labor",
        ));
    }
    let rows: Vec<Price> = p
        .get("rows")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("wholejob.prices", "rows required: [{item, unit, unit_cost}]"))?
        .iter()
        .take(MAX_LINES)
        .map(|r| -> Result<Price> {
            let item =
                r["item"].as_str().map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad("wholejob.prices", "each price needs an item"))?;
            let c = r["unit_cost"]
                .as_f64()
                .filter(|v| v.is_finite() && *v > 0.0 && *v < 1e9)
                .ok_or_else(|| bad("wholejob.prices", format!("{item}: unit_cost must be more than 0")))?;
            Ok(Price { item: item.into(), unit: r["unit"].as_str().unwrap_or("ea").trim().to_ascii_lowercase(), unit_cost: c })
        })
        .collect::<Result<_>>()?;
    let n = rows.len();
    w.prices
        .insert(trade.clone(), PricePack { quality: quality.clone(), source: source.into(), date: str_param(p, "date").unwrap_or("").into(), rows });
    save(s, sp, &w)?;
    Ok(json!({"trade": trade, "quality": quality, "rows": n}))
}

fn run_settings(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec("wholejob.settings", s)?;
    let mut w = load(&sp);
    if let Some(m) = p.get("markups").and_then(Value::as_object) {
        for (k, v) in m.iter().take(200) {
            let v = v
                .as_f64()
                .filter(|v| v.is_finite() && (0.0..=100.0).contains(v))
                .ok_or_else(|| bad("wholejob.settings", "a markup is a percent from 0 to 100"))?;
            w.markups.insert(k.trim().to_ascii_lowercase(), v);
        }
    }
    if let Some(g) = p.get("gc").and_then(Value::as_object) {
        let mut gc = serde_json::to_value(&w.gc).unwrap_or(Value::Null);
        for (k, v) in g {
            let v = v
                .as_f64()
                .filter(|v| v.is_finite() && *v >= 0.0 && *v < 1e9)
                .ok_or_else(|| bad("wholejob.settings", format!("gc.{k} must be 0 or more")))?;
            if gc.get(k).is_none() {
                return Err(bad("wholejob.settings", format!("no general conditions setting {}", k.chars().take(40).collect::<String>())));
            }
            gc[k] = json!(v);
        }
        w.gc = serde_json::from_value(gc).unwrap_or_default();
    }
    if let Some(b) = p.get("electrical_from_estimate").and_then(Value::as_bool) {
        w.electrical_from_estimate = b;
    }
    let out = json!({"markups": w.markups, "gc": w.gc, "electrical_from_estimate": w.electrical_from_estimate});
    save(s, sp, &w)?;
    Ok(out)
}

/// A price for a line: the pack's row with the same item (ignoring case and spacing) and unit.
fn price_for<'a>(pack: Option<&'a PricePack>, l: &QLine) -> Option<&'a Price> {
    pack?.rows.iter().find(|r| norm(&r.item) == norm(&l.item) && r.unit.eq_ignore_ascii_case(&l.unit))
}

pub(crate) fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// The estimate: per trade, its contract value and data quality; general conditions; total.
fn estimate(s: &mut Session, w: &WholeJob) -> Result<Value> {
    let electrical_total = if w.electrical_from_estimate {
        s.execute("estimate.status", &json!({})).ok().and_then(|v| v["totals"]["total"].as_f64()).filter(|v| v.is_finite() && *v > 0.0)
    } else {
        None
    };
    let mut trades = Vec::new();
    let mut by_div: BTreeMap<String, f64> = BTreeMap::new();
    let mut subtotal_all = 0.0;
    let mut unpriced_all = 0usize;
    for (name, t) in &w.trades {
        let pack = w.prices.get(name);
        let (div, div_name, default_markup) = trade_info(name);
        let division = if t.division.is_empty() { div.to_string() } else { t.division.clone() };
        let mut priced = Vec::new();
        let mut unpriced = Vec::new();
        let mut direct = 0.0;
        for l in &t.lines {
            match price_for(pack, l) {
                Some(pr) => {
                    let cost = l.qty * pr.unit_cost;
                    direct += cost;
                    priced.push(json!({"item": l.item, "unit": l.unit, "qty": l.qty, "unit_cost": pr.unit_cost, "cost": r2(cost)}));
                }
                None => unpriced.push(json!({"item": l.item, "unit": l.unit, "qty": l.qty})),
            }
        }
        let mut quality = pack.map_or("unpriced".to_string(), |p| p.quality.clone());
        let mut source = pack.map(|p| p.source.clone()).unwrap_or_default();
        if is_electrical(name, &division)
            && let Some(total) = electrical_total
        {
            direct = total;
            quality = "private".into();
            source = "the electrical estimate (estimate.*)".into();
            unpriced.clear();
        }
        let markup_pct = w.markups.get(name).copied().unwrap_or(default_markup);
        let contract = direct * (1.0 + markup_pct / 100.0);
        unpriced_all += unpriced.len();
        subtotal_all += contract;
        *by_div.entry(division.clone()).or_default() += contract;
        trades.push(json!({
            "trade": name, "division": division, "division_name": if div == division { div_name.to_string() } else { String::new() },
            "direct": r2(direct), "markup_pct": markup_pct, "contract_value": r2(contract), "quality": quality, "source": source,
            "lines": priced.len() + unpriced.len(), "priced": priced, "unpriced": unpriced,
        }));
    }
    let g = &w.gc;
    let weekly = g.weeks * (g.supervision_per_week + g.temp_facilities_per_week + g.equipment_per_week + g.cleanup_per_week);
    let insurance = (subtotal_all + weekly) * g.insurance_pct / 100.0;
    let bond = (subtotal_all + weekly + insurance) * g.bond_pct / 100.0;
    let gc_total = weekly + insurance + bond;
    let total = subtotal_all + gc_total;
    Ok(json!({
        "trades": trades,
        "by_division": by_div.into_iter().map(|(k, v)| (k, json!(r2(v)))).collect::<serde_json::Map<String, Value>>(),
        "general_conditions": {
            "weeks": g.weeks, "supervision": r2(g.weeks * g.supervision_per_week), "temp_facilities": r2(g.weeks * g.temp_facilities_per_week),
            "equipment": r2(g.weeks * g.equipment_per_week), "cleanup": r2(g.weeks * g.cleanup_per_week), "insurance": r2(insurance), "bond": r2(bond), "total": r2(gc_total),
        },
        "subtotal": r2(subtotal_all), "total": r2(total), "unpriced_lines": unpriced_all,
    }))
}

fn run_estimate(s: &mut Session, _p: &Value) -> Result<Value> {
    let sp = spec("wholejob.estimate", s)?;
    let w = load(&sp);
    if w.trades.is_empty() {
        return Err(bad("wholejob.estimate", "no trade quantities yet (wholejob.quantities)"));
    }
    estimate(s, &w)
}

fn run_package(s: &mut Session, p: &Value) -> Result<Value> {
    let trade = trade_param("wholejob.package", p)?;
    let sp = spec("wholejob.package", s)?;
    let w = load(&sp);
    let t = w.trades.get(&trade).ok_or_else(|| bad("wholejob.package", format!("no quantities for {trade}")))?;
    let sections: BTreeSet<&str> = t.lines.iter().map(|l| l.spec_section.as_str()).filter(|x| !x.is_empty()).collect();
    let out = json!({
        "trade": trade, "division": t.division, "spec_sections": sections, "scope": {"inclusions": t.inclusions, "exclusions": t.exclusions, "alternates": t.alternates},
        "quantities": t.lines.iter().map(|l| json!({"item": l.item, "unit": l.unit, "qty": l.qty, "spec_section": l.spec_section})).collect::<Vec<_>>(),
    });
    if let Some(path) = str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()) {
        let mut rows = vec![
            vec![format!("BID PACKAGE: {}", trade.to_uppercase()), format!("Division {}", t.division)],
            vec!["Item".into(), "Unit".into(), "Quantity".into(), "Spec section".into()],
        ];
        rows.extend(t.lines.iter().map(|l| vec![l.item.clone(), l.unit.clone(), format!("{}", r2(l.qty)), l.spec_section.clone()]));
        for (k, v) in [("Inclusion", &t.inclusions), ("Exclusion", &t.exclusions), ("Alternate", &t.alternates)] {
            rows.extend(v.iter().map(|x| vec![k.to_string(), x.clone()]));
        }
        let bytes = if path.to_ascii_lowercase().ends_with(".csv") {
            rows.iter()
                .map(|r| {
                    r.iter()
                        .map(|c| if c.contains([',', '"']) { format!("\"{}\"", c.replace('"', "\"\"")) } else { c.clone() })
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .collect::<Vec<_>>()
                .join("\n")
                .into_bytes()
        } else {
            super::table::xlsx::write(&rows, "Bid package").map_err(|e| bad("wholejob.package", e))?
        };
        #[cfg(not(target_arch = "wasm32"))]
        crate::writefile::write(path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
        #[cfg(target_arch = "wasm32")]
        let _ = bytes;
    }
    Ok(out)
}

fn run_bid(s: &mut Session, p: &Value) -> Result<Value> {
    let trade = trade_param("wholejob.bid", p)?;
    let bidder = str_param(p, "bidder").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad("wholejob.bid", "bidder required"))?.to_string();
    let total = p
        .get("total")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0 && *v < 1e12)
        .ok_or_else(|| bad("wholejob.bid", "total required (the bid's amount)"))?;
    let lines: Vec<BidLine> = p
        .get("lines")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(MAX_LINES)
                .filter_map(|l| {
                    Some(BidLine {
                        item: l["item"].as_str()?.trim().to_string(),
                        unit: l["unit"].as_str().unwrap_or("").trim().to_ascii_lowercase(),
                        qty: l["qty"].as_f64().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0),
                        amount: l["amount"].as_f64().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let sp = spec("wholejob.bid", s)?;
    let mut w = load(&sp);
    w.bids.retain(|b| !(b.trade == trade && b.bidder.eq_ignore_ascii_case(&bidder)));
    w.bids.push(Bid {
        trade: trade.clone(),
        bidder: bidder.clone(),
        total,
        lines,
        exclusions: strings(p, "exclusions").unwrap_or_default(),
        alternates: strings(p, "alternates").unwrap_or_default(),
    });
    save(s, sp, &w)?;
    Ok(json!({"trade": trade, "bidder": bidder}))
}

/// Whether bid text `b` names the model line `m` (either holds the other, ignoring case).
fn names(b: &str, m: &str) -> bool {
    let (b, m) = (norm(b), norm(m));
    !b.is_empty() && (b == m || b.contains(&m) || m.contains(&b))
}

pub(crate) fn median(mut v: Vec<f64>) -> Option<f64> {
    v.retain(|x| x.is_finite());
    v.sort_by(f64::total_cmp);
    let n = v.len();
    match n {
        0 => None,
        _ if n % 2 == 1 => v.get(n / 2).copied(),
        _ => Some((v.get(n / 2 - 1).copied().unwrap_or(0.0) + v.get(n / 2).copied().unwrap_or(0.0)) / 2.0),
    }
}

fn run_level(s: &mut Session, p: &Value) -> Result<Value> {
    let trade = trade_param("wholejob.level", p)?;
    let sp = spec("wholejob.level", s)?;
    let w = load(&sp);
    let t = w.trades.get(&trade).ok_or_else(|| bad("wholejob.level", format!("no quantities for {trade}")))?;
    let bids: Vec<&Bid> = w.bids.iter().filter(|b| b.trade == trade).collect();
    if bids.is_empty() {
        return Err(bad("wholejob.level", format!("no bids for {trade} (wholejob.bid)")));
    }
    let pack = w.prices.get(&trade);
    // Each model line's unit price per bidder (for outliers).
    let unit_prices = |l: &QLine| -> Vec<f64> {
        bids.iter()
            .filter_map(|b| b.lines.iter().find(|x| names(&x.item, &l.item) && x.qty > 0.0 && x.amount > 0.0).map(|x| x.amount / x.qty))
            .collect()
    };
    let gap_tol = 0.10;
    let price_tol = 0.35;
    let mut out = Vec::new();
    for b in &bids {
        let (mut missing, mut gaps, mut outliers) = (Vec::new(), Vec::new(), Vec::new());
        let mut missing_value = 0.0;
        for l in &t.lines {
            let excluded = b.exclusions.iter().any(|x| names(x, &l.item));
            match b.lines.iter().find(|x| names(&x.item, &l.item)) {
                Some(_) if excluded => missing.push(json!({"item": l.item, "why": "excluded"})),
                Some(x) => {
                    if l.qty > 0.0 && (x.qty - l.qty).abs() / l.qty > gap_tol {
                        gaps.push(json!({"item": l.item, "model_qty": l.qty, "bid_qty": x.qty, "unit": l.unit}));
                    }
                    if x.qty > 0.0
                        && let Some(m) = median(unit_prices(l))
                        && m > 0.0
                        && ((x.amount / x.qty) - m).abs() / m > price_tol
                    {
                        outliers.push(json!({"item": l.item, "unit_price": r2(x.amount / x.qty), "median": r2(m)}));
                    }
                    continue;
                }
                None => missing.push(json!({"item": l.item, "why": if excluded { "excluded" } else { "not in the bid" }})),
            }
            // What the missing scope costs: our price, else the other bidders' median.
            let unit = price_for(pack, l).map(|p| p.unit_cost).or_else(|| median(unit_prices(l))).unwrap_or(0.0);
            missing_value += unit * l.qty;
        }
        let complete = missing.is_empty();
        out.push(json!({
            "bidder": b.bidder, "total": b.total, "complete": complete, "missing_scope": missing, "quantity_gaps": gaps, "outlier_prices": outliers,
            "exclusions": b.exclusions, "alternates": b.alternates, "leveled_total": r2(b.total + missing_value),
        }));
    }
    let recommended = out
        .iter()
        .filter(|x| x["complete"] == true)
        .min_by(|a, b| a["total"].as_f64().unwrap_or(f64::MAX).total_cmp(&b["total"].as_f64().unwrap_or(f64::MAX)))
        .map(|x| x["bidder"].clone());
    let note = if recommended.is_none() { "no complete bid: compare the leveled totals, and ask the bidders to fill the gaps" } else { "" };
    Ok(json!({"trade": trade, "bids": out, "recommended": recommended, "note": note}))
}

// ---------------- bid history ----------------

fn pack_path(cmd: &str, p: &Value) -> Result<String> {
    str_param(p, "pack")
        .map(str::trim)
        .filter(|x| x.to_ascii_lowercase().ends_with(".jsonl"))
        .map(str::to_string)
        .ok_or_else(|| bad(cmd, "pack required: a .jsonl history file"))
}

pub(crate) fn append(path: &str, rows: &[Value]) -> Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut text = std::fs::read_to_string(path).unwrap_or_default();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for r in rows {
            text += &r.to_string();
            text.push('\n');
        }
        crate::writefile::write(path, text.as_bytes()).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (path, rows);
        Err(crate::EngineError::Other("history packs can't be written on the web".into()))
    }
}

fn run_history(_s: &mut Session, p: &Value) -> Result<Value> {
    let pack = pack_path("wholejob.history", p)?;
    let private = bool_or(p, "private", false);
    let trade = trade_param("wholejob.history", p)?;
    let division = str_param(p, "division").map(str::to_string).unwrap_or_else(|| trade_info(&trade).0.into());
    if is_electrical(&trade, &division) && !private {
        return Err(bad("wholejob.history", "electrical bid history goes only to a private pack (private: true)"));
    }
    let num = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0 && *v < 1e12);
    let amount = num("amount").ok_or_else(|| bad("wholejob.history", "amount required"))?;
    let sf = num("sf").ok_or_else(|| bad("wholejob.history", "sf required (the building's area)"))?;
    let row = json!({
        "kind": "bid", "trade": trade, "division": division, "building_type": str_param(p, "building_type").unwrap_or(""), "region": str_param(p, "region").unwrap_or(""),
        "date": str_param(p, "date").unwrap_or(""), "sf": sf, "amount": amount, "per_sf": r2(amount / sf), "source": str_param(p, "source").unwrap_or(""), "private": private,
    });
    append(&pack, &[row])?;
    Ok(json!({"pack": pack, "per_sf": r2(amount / sf)}))
}

fn run_outcome(s: &mut Session, p: &Value) -> Result<Value> {
    let pack = pack_path("wholejob.outcome", p)?;
    if !bool_or(p, "private", false) {
        return Err(bad("wholejob.outcome", "outcomes go only to the company's private pack (private: true)"));
    }
    let trade = trade_param("wholejob.outcome", p)?;
    let num = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0 && *v < 1e12);
    let job = s.state().map(|x| x.title.clone()).unwrap_or_default();
    let row = json!({
        "kind": "outcome", "trade": trade, "job": job, "award": num("award"), "final_cost": num("final_cost"), "change_orders": num("change_orders"),
        "sf": num("sf"), "building_type": str_param(p, "building_type").unwrap_or(""), "date": str_param(p, "date").unwrap_or(""), "private": true,
    });
    append(&pack, &[row])?;
    Ok(json!({"pack": pack}))
}

fn run_benchmarks(_s: &mut Session, p: &Value) -> Result<Value> {
    let packs: Vec<String> =
        strings(p, "packs").filter(|v| !v.is_empty()).ok_or_else(|| bad("wholejob.benchmarks", "packs required: [.jsonl history files]"))?;
    let (bt, region) = (str_param(p, "building_type").map(norm), str_param(p, "region").map(norm));
    let mut by: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for path in &packs {
        let text = crate::readfile::read(path.trim())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .map_err(|e| bad("wholejob.benchmarks", format!("{path}: {e}")))?;
        for line in text.lines().take(1_000_000) {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            if v["kind"] != "bid"
                || bt.as_ref().is_some_and(|b| norm(v["building_type"].as_str().unwrap_or("")) != *b)
                || region.as_ref().is_some_and(|r| norm(v["region"].as_str().unwrap_or("")) != *r)
            {
                continue;
            }
            if let (Some(t), Some(ps)) = (v["trade"].as_str(), v["per_sf"].as_f64()) {
                by.entry(t.to_string()).or_default().push(ps);
            }
        }
    }
    let out: Vec<Value> = by
        .into_iter()
        .map(|(t, v)| {
            let (lo, hi) = v.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| (a.min(*x), b.max(*x)));
            json!({"trade": t, "count": v.len(), "per_sf_median": median(v.clone()).map(r2), "per_sf_min": r2(lo), "per_sf_max": r2(hi)})
        })
        .collect();
    Ok(json!({"benchmarks": out}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("wholejob.quantities", "Trade Quantities", run_quantities)
            .params("{trade, division?: CSI 2-digit, lines: [{item, unit, qty, spec_section?, source?}], replace?: bool (default true)} | {from_bom: true} → {trade, lines}"),
        CommandSpec::new("wholejob.scope", "Trade Scope", run_scope).params("{trade, inclusions?: [text], exclusions?: [text], alternates?: [text]} → the scope"),
        CommandSpec::new("wholejob.prices", "Trade Prices", run_prices)
            .params("{trade, quality: private|public|assumed (electrical: private only), source, date?, rows: [{item, unit, unit_cost}]} → {trade, quality, rows}"),
        CommandSpec::new("wholejob.settings", "Whole-Job Settings", run_settings)
            .params("{markups?: {trade: percent}, gc?: {weeks, supervision_per_week, temp_facilities_per_week, equipment_per_week, cleanup_per_week, insurance_pct, bond_pct}, electrical_from_estimate?: bool} → {markups, gc, electrical_from_estimate}"),
        CommandSpec::new("wholejob.estimate", "Whole-Job Estimate", run_estimate)
            .params("{} → {trades: [{trade, division, direct, markup_pct, contract_value, quality, source, priced, unpriced}], by_division, general_conditions, subtotal, total, unpriced_lines}")
            .noundo(),
        CommandSpec::new("wholejob.package", "Bid Package", run_package)
            .files().params("{trade, path?: .xlsx|.csv} → {trade, division, spec_sections, scope, quantities}").noundo(),
        CommandSpec::new("wholejob.bid", "Sub Bid", run_bid).params("{trade, bidder, total, lines?: [{item, unit, qty, amount}], exclusions?: [text], alternates?: [text]} → {trade, bidder}"),
        CommandSpec::new("wholejob.level", "Level Bids", run_level)
            .params("{trade} → {bids: [{bidder, total, complete, missing_scope, quantity_gaps, outlier_prices, exclusions, leveled_total}], recommended: the low complete bid}")
            .noundo(),
        CommandSpec::new("wholejob.history", "Add Bid History", run_history)
            .files()
            .enabled(always)
            .params("{pack: .jsonl, private?: bool (electrical: required), trade, division?, building_type, region, date, sf, amount, source} → {pack, per_sf}")
            .noundo(),
        CommandSpec::new("wholejob.benchmarks", "Bid Benchmarks", run_benchmarks)
            .files()
            .enabled(always)
            .params("{packs: [.jsonl], building_type?, region?} → {benchmarks: [{trade, count, per_sf_median, per_sf_min, per_sf_max}]}")
            .noundo(),
        CommandSpec::new("wholejob.outcome", "Record Outcome", run_outcome)
            .files()
            .params("{pack: private .jsonl, private: true, trade, award?, final_cost?, change_orders?, sf?, building_type?, date?} → {pack}")
            .noundo(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_medians_and_divisions() {
        assert!(names("Gypsum board ceilings", "gypsum board ceilings"));
        assert!(names("ACT ceilings incl. grid", "ACT ceilings"));
        assert!(!names("", "x"));
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![1.0, 2.0]), Some(1.5));
        assert_eq!(median(vec![]), None);
        assert_eq!(trade_info("Painting").0, "09");
        assert!(is_electrical("electrical", "26") && is_electrical("x", "28") && !is_electrical("roofing", "07"));
    }
}
