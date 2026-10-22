//! The human outputs: the total broken down by level, trade and system (each sums to the total
//! to the cent), the estimate as CSV or XLSX, and a bid summary with its assumptions,
//! clarifications and exclusions drafted from the scope decisions and the open findings.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

use super::{Job, Priced, setting, totals};
use crate::Result;
use crate::cmd::{bad, str_param};

/// Export columns, in their default order.
const COLUMNS: [&str; 17] = [
    "key",
    "trade",
    "category",
    "item",
    "size",
    "unit",
    "qty",
    "net",
    "level",
    "material",
    "labor_hours",
    "labor_cost",
    "cost",
    "total",
    "scope",
    "status",
    "via",
];

/// Split `target` cents over `raw` dollar amounts (summing to it but for rounding): each gets
/// its amount to the cent, the rounding cents going to the largest remainders, so the parts sum
/// to `target` exactly.
fn apportion(raw: &[f64], target: i64) -> Vec<i64> {
    let mut cents: Vec<i64> = raw.iter().map(|v| (v * 100.0).floor() as i64).collect();
    let mut order: Vec<usize> = (0..raw.len()).collect();
    let frac = |i: usize| raw.get(i).map_or(0.0, |v| v * 100.0 - (v * 100.0).floor());
    order.sort_by(|a, b| frac(*b).total_cmp(&frac(*a)));
    let mut left = target - cents.iter().sum::<i64>();
    let len = order.len();
    // A cent at a time: to the largest remainders when short, from the smallest when over
    // (float noise is a few cents at most; the loop is capped).
    let mut k = 0usize;
    while left != 0 && len > 0 && k < 1_000_000 {
        let at = if left > 0 { order.get(k % len) } else { order.get(len - 1 - k % len) };
        if let Some(c) = at.and_then(|i| cents.get_mut(*i)) {
            *c += left.signum();
            left -= left.signum();
        }
        k += 1;
    }
    cents
}

/// Each in-contract line's share of the total: its cost plus tax on its material, times the
/// compounded overhead, profit, contingency and bond (the lines' shares sum to the total).
fn shares(st: &Map<String, Value>, priced: &[Priced]) -> Vec<(usize, f64)> {
    let pct = |k: &str| setting(st, k, 0.0) / 100.0;
    let f = (1.0 + pct("overhead_pct")) * (1.0 + pct("profit_pct")) * (1.0 + pct("contingency_pct")) * (1.0 + pct("bond_pct"));
    let tax = pct("tax_pct");
    priced.iter().enumerate().filter(|(_, p)| p.scope.as_deref() == Some("in_contract")).map(|(i, p)| (i, (p.cost + p.material * tax) * f)).collect()
}

fn grand_cents(st: &Map<String, Value>, priced: &[Priced]) -> i64 {
    (totals(st, priced)["totals"]["total"].as_f64().unwrap_or(0.0) * 100.0).round() as i64
}

/// Each line's levels: its net quantity per level from the by-level BOM, else the job.
fn level_split(job: &Job, p: &Priced) -> Vec<(String, f64)> {
    let split: Vec<(String, f64)> = job
        .levels
        .get(&p.key)
        .map(|v| v.iter().filter(|(_, n)| *n > 0.0).map(|(l, n)| (if l.is_empty() { "unassigned".to_string() } else { l.clone() }, *n)).collect())
        .unwrap_or_default();
    if split.is_empty() { vec![("job".to_string(), 1.0)] } else { split }
}

/// The total by level, trade and system (BOM category), each summing to the total to the cent;
/// and each in-contract line's share of the total, to the cent.
pub(super) fn breakdown(st: &Map<String, Value>, job: &Job) -> Value {
    let sh = shares(st, &job.priced);
    let target = grand_cents(st, &job.priced);
    let group = |by: &dyn Fn(&Priced) -> Vec<(String, f64)>| -> Value {
        let mut raw: BTreeMap<String, f64> = BTreeMap::new();
        for (i, m) in &sh {
            let Some(p) = job.priced.get(*i).filter(|_| *m != 0.0) else { continue };
            let parts = by(p);
            let w: f64 = parts.iter().map(|(_, x)| x).sum();
            for (g, x) in parts {
                *raw.entry(g).or_insert(0.0) += if w > 0.0 { m * x / w } else { 0.0 };
            }
        }
        let keys: Vec<String> = raw.keys().cloned().collect();
        let vals: Vec<f64> = raw.values().copied().collect();
        let cents = apportion(&vals, target);
        Value::Object(keys.into_iter().zip(cents).map(|(k, c)| (k, json!(c as f64 / 100.0))).collect())
    };
    let s = |k: &'static str| move |p: &Priced| vec![(p.line[k].as_str().unwrap_or("other").to_string(), 1.0)];
    let line_cents = apportion(&sh.iter().map(|(_, m)| *m).collect::<Vec<_>>(), target);
    let lines: Map<String, Value> =
        sh.iter().zip(line_cents).filter_map(|((i, _), c)| job.priced.get(*i).map(|p| (p.key.clone(), json!(c as f64 / 100.0)))).collect();
    json!({
        "by_level": group(&|p| level_split(job, p)),
        "by_trade": group(&s("trade")),
        "by_system": group(&s("category")),
        "lines": lines,
    })
}

fn line_row(job: &Job, p: &Priced, total: Option<f64>, cols: &[String]) -> Vec<String> {
    let v = super::line_json(p);
    cols.iter()
        .map(|c| match c.as_str() {
            "level" => level_split(job, p).iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>().join("; "),
            "total" => format!("{:.2}", total.unwrap_or(0.0)),
            "material" | "labor_cost" | "cost" | "labor_hours" => format!("{:.2}", v[c.as_str()].as_f64().unwrap_or(0.0)),
            k => match &v[k] {
                Value::String(x) => x.clone(),
                Value::Null => String::new(),
                x => x.to_string(),
            },
        })
        .collect()
}

/// `estimate.export {path, format?, columns?}`: every line (its share of the total in `total`)
/// and a last TOTAL row.
pub(super) fn export(st: &Map<String, Value>, job: &Job, p: &Value) -> Result<Value> {
    const CMD: &str = "estimate.export";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(CMD, "`path` (.xlsx or .csv) is required"))?;
    let format = match str_param(p, "format") {
        Some(f) => f.to_ascii_lowercase(),
        None if path.to_ascii_lowercase().ends_with(".xlsx") => "xlsx".into(),
        None => "csv".into(),
    };
    if format != "xlsx" && format != "csv" {
        return Err(bad(CMD, "`format` is xlsx or csv"));
    }
    let cols: Vec<String> = match p.get("columns").and_then(Value::as_array) {
        Some(a) => {
            let c: Vec<String> = a.iter().filter_map(Value::as_str).map(str::to_string).take(COLUMNS.len()).collect();
            if let Some(x) = c.iter().find(|x| !COLUMNS.contains(&x.as_str())) {
                return Err(bad(CMD, format!("unknown column {x}: the columns are {}", COLUMNS.join(", "))));
            }
            if c.is_empty() {
                return Err(bad(CMD, "`columns` is empty"));
            }
            c
        }
        None => COLUMNS.iter().map(|c| (*c).to_string()).collect(),
    };
    let b = breakdown(st, job);
    let total = totals(st, &job.priced)["totals"]["total"].as_f64().unwrap_or(0.0);
    let mut rows = vec![cols.clone()];
    for pr in &job.priced {
        rows.push(line_row(job, pr, b["lines"][pr.key.as_str()].as_f64(), &cols));
    }
    rows.push(
        cols.iter()
            .map(|c| match c.as_str() {
                "key" => "TOTAL".to_string(),
                "total" => format!("{total:.2}"),
                _ => String::new(),
            })
            .collect(),
    );
    let bytes = if format == "xlsx" {
        crate::cmd::table::xlsx::write(&rows, "Estimate").map_err(|e| bad(CMD, e))?
    } else {
        rows.iter()
            .map(|r| {
                r.iter()
                    .map(|c| if c.contains([',', '"', '\n']) { format!("\"{}\"", c.replace('"', "\"\"")) } else { c.clone() })
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes()
    };
    #[cfg(not(target_arch = "wasm32"))]
    crate::writefile::write(path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
    #[cfg(target_arch = "wasm32")]
    let _ = bytes;
    Ok(json!({ "path": path, "format": format, "lines": job.priced.len(), "total": total, "columns": cols }))
}

/// `estimate.bid_summary`: the bid total, its alternates, and drafted assumptions,
/// clarifications and exclusions, for a person to edit and send.
pub(super) fn bid_summary(st: &Map<String, Value>, job: &Job, findings: &[Value]) -> Value {
    let t = totals(st, &job.priced);
    let b = breakdown(st, job);
    let inc: Vec<&Priced> = job.priced.iter().filter(|p| p.scope.as_deref() == Some("in_contract")).collect();
    let name = |p: &Priced| {
        let size = p.line["size"].as_str().unwrap_or_default();
        format!("{}{}", p.line["item"].as_str().unwrap_or_default(), if size.is_empty() { String::new() } else { format!(" {size}") })
    };
    let reason = |p: &Priced| st.get("scope").and_then(|m| m.get(&p.key)).and_then(|x| x["reason"].as_str()).unwrap_or_default().to_string();
    // Exclusions and owner-furnished items, grouped by scope and reason.
    let mut groups: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for p in &job.priced {
        if let Some(sc) = p.scope.as_deref().filter(|s| matches!(*s, "nic" | "by_others" | "ofci")) {
            groups.entry((sc.to_string(), reason(p))).or_default().push(name(p));
        }
    }
    let list = |v: &[String]| {
        let mut s = v.iter().take(12).cloned().collect::<Vec<_>>().join(", ");
        if v.len() > 12 {
            s += &format!(" and {} more", v.len() - 12);
        }
        s
    };
    let mut exclusions = Vec::new();
    let mut clarifications = Vec::new();
    for ((sc, why), items) in &groups {
        let why = if why.is_empty() { String::new() } else { format!(" ({why})") };
        match sc.as_str() {
            "nic" => exclusions.push(format!("Not in contract: {}{why}.", list(items))),
            "by_others" => exclusions.push(format!("By others: {}{why}.", list(items))),
            _ => clarifications.push(format!("Owner-furnished, contractor-installed: {}{why}; we install, the owner supplies.", list(items))),
        }
    }
    for f in findings.iter().take(50) {
        clarifications.push(format!(
            "Open item ({}): {}{}. {}",
            f["kind"].as_str().unwrap_or_default(),
            f["line"].as_str().map(|l| format!("{l}: ")).unwrap_or_default(),
            f["message"].as_str().unwrap_or_default(),
            f["fix"].as_str().unwrap_or_default()
        ));
    }
    // Assumptions: what the number rests on that the drawings and specs don't fix.
    let mut assumptions = Vec::new();
    let rule_lines = inc.iter().filter(|p| p.line["status"].as_str() == Some("rule") && p.line["trade"].as_str() != Some("job")).count();
    if rule_lines > 0 {
        assumptions.push(format!(
            "{rule_lines} quantities come from company takeoff rules (boxes per device, fittings per run, wire makeup), not counted on the drawings."
        ));
    }
    let base = st.get("baseline").cloned().unwrap_or(json!({}));
    for (fname, f) in base["factors"].as_object().into_iter().flatten() {
        let n = inc.iter().filter(|p| p.labor_steps.iter().any(|s| s.contains(&format!(" {fname} (")))).count();
        if n > 0 && f["basis"].as_str() == Some("assumption") {
            assumptions.push(format!(
                "Labor on {n} line(s) assumes the {} condition \"{fname}\" (× {}; {}).",
                f["kind"].as_str().unwrap_or_default(),
                f["multiplier"],
                f["source"].as_str().unwrap_or_default()
            ));
        }
    }
    let tier = st.get("settings").and_then(|x| x["tier"].as_str()).unwrap_or("normal");
    if tier != "normal" {
        assumptions.push(format!("Book labor at the {tier} tier."));
    }
    let driven = inc.iter().filter(|p| p.labor_steps.iter().any(|s| s.starts_with("driver "))).count();
    if driven > 0 {
        assumptions.push(format!("Labor for {driven} line(s) from the production model, book rates blended with field observations."));
    }
    if inc.iter().any(|p| p.material_steps.iter().any(|s| s.contains("escalated"))) {
        let today = st.get("settings").and_then(|x| x["today"].as_str()).unwrap_or_default();
        assumptions.push(format!("Dated prices escalated by their index to {today}."));
    }
    let days = setting(st, "duration_days", 0.0);
    if days > 0.0 {
        assumptions.push(format!("Rentals and tool charges for a {days}-day duration."));
    }
    let alternates = t["alternates"].clone();
    let total = t["totals"]["total"].clone();
    let money = |v: &Value| format!("${:.2}", v.as_f64().unwrap_or(0.0));
    let mut text = format!("# Bid summary\n\nBase bid: {}\n\n## By trade\n", money(&total));
    for (k, v) in b["by_trade"].as_object().into_iter().flatten() {
        text += &format!("- {k}: {}\n", money(v));
    }
    if let Some(a) = alternates.as_object().filter(|a| !a.is_empty()) {
        text += "\n## Alternates (not in the base bid)\n";
        for (k, v) in a {
            text += &format!("- {k}: add {}\n", money(v));
        }
    }
    for (h, v) in [("Assumptions", &assumptions), ("Clarifications", &clarifications), ("Exclusions", &exclusions)] {
        if !v.is_empty() {
            text += &format!("\n## {h}\n");
            for x in v {
                text += &format!("- {x}\n");
            }
        }
    }
    json!({
        "total": total,
        "base_bid": { "by_trade": b["by_trade"], "totals": t["totals"] },
        "alternates": alternates,
        "assumptions": assumptions,
        "clarifications": clarifications,
        "exclusions": exclusions,
        "open_findings": findings.len(),
        "text": text,
    })
}
