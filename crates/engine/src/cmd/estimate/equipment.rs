//! Job equipment, tools and setup: the BOM's required items (Q326) and the company's
//! `tool_rules.json`, priced from what the modelled work implies (large terminations, large
//! pulls, high work, floors, crew, duration). Owned tools carry a tool charge, rentals a rate ×
//! the duration, purchases a price × quantity. Work that implies equipment no line prices is a
//! `missing_equipment` finding.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

use super::{Priced, setting};

/// Rules and items per rule read, at most (hostile files).
const MAX_RULES: usize = 1_000;
const MAX_ITEMS: usize = 100;
/// Elements a job line lists, at most.
const MAX_ELEMENTS: usize = 50;
/// Facts a rule may test.
pub(super) const FACTS: [&str; 9] =
    ["large_terminations", "large_pull_ft", "wire_ft", "high_work_ft", "floors", "peak_crew", "duration_days", "labor_hours", "always"];

/// What the work implies, by the field lessons: (fact, keyword a priced line must carry, why).
const IMPLIED: [(&str, &str, &str); 7] = [
    ("large_terminations", "crimper", "large conductors are terminated: their lugs are crimped"),
    ("large_terminations", "torque wrench", "large lugs are torqued to the listing"),
    ("large_terminations", "lug", "large terminations need lugs, bolts, nuts and washers"),
    ("large_pull_ft", "tugger", "large conductors are pulled by machine"),
    ("high_work_ft", "lift", "work above ladder height"),
    ("floors", "ladder", "ceiling work on every floor"),
    ("floors", "gang box", "tools are locked up on every floor"),
];

/// `tool_rules.json`: `{rules: [{when: fact, min?, items: [{item, supply: purchase|rental|owned,
/// category?, price? | rate?, per?: each|job|day|week|month, qty? | qty_per?}]}]}`.
pub(super) fn read_rules(v: &Value, notes: &mut Vec<String>) -> Vec<Value> {
    let mut out = Vec::new();
    for r in v["rules"].as_array().into_iter().flatten().take(MAX_RULES) {
        let when = r["when"].as_str().unwrap_or_default();
        if !FACTS.contains(&when) {
            notes.push(format!("tool_rules: `when` {when:?} is one of {}", FACTS.join(", ")));
            continue;
        }
        let mut items = Vec::new();
        for it in r["items"].as_array().into_iter().flatten().take(MAX_ITEMS) {
            let item = it["item"].as_str().unwrap_or_default();
            let supply = it["supply"].as_str().unwrap_or("purchase");
            let rate = it["rate"].as_f64().or_else(|| it["price"].as_f64()).filter(|x| x.is_finite() && *x >= 0.0);
            let per = it["per"].as_str().unwrap_or(if supply == "purchase" { "each" } else { "day" });
            let ok_qty = it["qty"].as_f64().or_else(|| it["qty_per"].as_f64()).is_none_or(|q| q.is_finite() && (0.0..=1e6).contains(&q));
            if item.is_empty()
                || !["purchase", "rental", "owned"].contains(&supply)
                || rate.is_none()
                || !["each", "job", "day", "week", "month"].contains(&per)
                || !ok_qty
            {
                notes.push(format!("tool_rules: item {item:?} needs a name, supply purchase|rental|owned, a price or rate, per each|job|day|week|month and a quantity from 0 to 1,000,000"));
                continue;
            }
            let mut v = it.clone();
            v["supply"] = json!(supply);
            v["rate"] = json!(rate);
            v["per"] = json!(per);
            items.push(v);
        }
        out.push(json!({ "when": when, "min": r["min"], "items": items }));
    }
    out
}

/// The job's facts with the elements behind each.
pub(super) struct Facts {
    pub(super) values: BTreeMap<&'static str, f64>,
    pub(super) elements: BTreeMap<&'static str, Vec<String>>,
}

/// Conductors `4#600 + 1#3G` → (count, size, ground?).
fn conductors(makeup: &str) -> Vec<(u32, String, bool)> {
    makeup
        .split(" in ")
        .next()
        .unwrap_or_default()
        .split('+')
        .filter_map(|p| {
            let (n, rest) = p.trim().split_once('#')?;
            let n = n.trim().parse::<u32>().ok().unwrap_or(1).min(12);
            let size: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '/').collect();
            let ground = rest.get(size.len()..).is_some_and(|r| r.starts_with('G'));
            (!size.is_empty()).then_some((n, size, ground))
        })
        .collect()
}

/// #4 AWG and larger: `4`…`1`, `1/0`…`4/0`, and kcmil sizes (250 and up).
pub(super) fn large(size: &str) -> bool {
    let size = size.trim().trim_start_matches('#');
    size.contains('/') || size.parse::<u32>().is_ok_and(|n| n <= 4 || n >= 250)
}

pub(super) fn facts(sp: &cadaddy_walk3d::BuildingSpec, st: &Map<String, Value>, priced: &[Priced]) -> Facts {
    let mut values: BTreeMap<&'static str, f64> = BTreeMap::new();
    let mut elements: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let mut push = |k: &'static str, v: f64, els: Vec<String>| {
        *values.entry(k).or_insert(0.0) += v;
        let e = elements.entry(k).or_default();
        for x in els {
            if e.len() < MAX_ELEMENTS && !e.contains(&x) {
                e.push(x);
            }
        }
    };
    for t in sp.tags.iter().filter(|t| t.kind == "homerun") {
        if let Some(f) = t.fields.get("conductors") {
            let n: u32 = conductors(&f.value).iter().filter(|(_, sz, g)| !g && large(sz)).map(|(n, _, _)| *n).sum();
            if n > 0 {
                push("large_terminations", f64::from(n) * 2.0, vec![t.id.clone()]);
            }
        }
    }
    let els = |p: &Priced| p.line["elements"].as_array().into_iter().flatten().filter_map(|e| e.as_str().map(String::from)).collect::<Vec<_>>();
    for p in priced.iter().filter(|p| p.line["category"].as_str() == Some("wire")) {
        push("wire_ft", p.net, els(p));
        let ground = p.line["item"].as_str().is_some_and(|i| i.to_ascii_lowercase().contains("ground"));
        if !ground && large(p.line["size"].as_str().unwrap_or_default()) {
            push("large_pull_ft", p.net, els(p));
        }
    }
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let high = setting(st, "high_work_ft", 14.0);
    for (i, r) in sp.mep.runs.iter().enumerate() {
        if r.bottoms_ft.iter().any(|b| b.is_finite() && *b >= high) {
            let len = r.points.windows(2).map(|w| if let [a, b] = w { (b[0] - a[0]).hypot(b[1] - a[1]) } else { 0.0 }).sum::<f64>() / upf;
            if len.is_finite() && len > 0.0 {
                push("high_work_ft", len, vec![format!("run:{i}")]);
            }
        }
    }
    let all: Vec<String> = priced.iter().filter_map(|p| p.line["elements"].get(0).and_then(Value::as_str).map(String::from)).collect();
    if !priced.is_empty() {
        push("floors", sp.levels.len().max(1) as f64, all.clone());
        push("always", 1.0, all.clone());
    }
    let hours: f64 = priced.iter().filter(|p| p.scope.as_deref() == Some("in_contract")).map(|p| p.labor_hours).sum();
    push("labor_hours", hours, all.clone());
    let days = setting(st, "duration_days", 0.0);
    if days > 0.0 {
        push("duration_days", days, all.clone());
    }
    let crew = match setting(st, "peak_crew", 0.0) {
        c if c > 0.0 => c,
        _ if days > 0.0 => (hours / (days * 8.0)).ceil().max(1.0),
        _ => 1.0,
    };
    push("peak_crew", crew, all);
    Facts { values, elements }
}

fn periods(per: &str, days: f64) -> Option<f64> {
    match per {
        "day" => Some(days),
        "week" => Some((days / 7.0).ceil()),
        "month" => Some((days / 30.0).ceil()),
        _ => None,
    }
}

fn job_line(key: &str, category: &str, item: &str, qty: f64, rule: &str, elements: &[String]) -> Value {
    json!({ "trade": "job", "category": category, "item": item, "size": "", "unit": "each", "net": qty, "total": qty, "status": "rule", "rule": rule, "elements": elements, "key": key })
}

/// The priced job lines (tool rules, then the BOM's required items) and the findings for work
/// with no equipment line.
pub(super) fn lines(st: &Map<String, Value>, base: &Value, facts: &Facts, required: &[Value], bom: &[Priced]) -> (Vec<Priced>, Vec<Value>) {
    let days = setting(st, "duration_days", 0.0);
    let scope_of = |key: &str| Some(st.get("scope").and_then(|m| m.get(key)).and_then(|x| x["scope"].as_str()).unwrap_or("in_contract").to_string());
    let mut out: Vec<Priced> = Vec::new();
    for r in base["tool_rules"].as_array().into_iter().flatten() {
        let when = r["when"].as_str().unwrap_or_default();
        let v = facts.values.get(when).copied().unwrap_or(0.0);
        let fires = match r["min"].as_f64() {
            Some(m) => v >= m,
            None => v > 0.0,
        };
        if !fires {
            continue;
        }
        let els = facts.elements.get(when).cloned().unwrap_or_default();
        for it in r["items"].as_array().into_iter().flatten() {
            let item = it["item"].as_str().unwrap_or_default();
            let supply = it["supply"].as_str().unwrap_or("purchase");
            let per = it["per"].as_str().unwrap_or("each");
            let rate = it["rate"].as_f64().unwrap_or(0.0);
            let qty = match (it["qty"].as_f64(), it["qty_per"].as_f64()) {
                (Some(q), _) => q,
                (None, Some(q)) => (q * v).ceil(),
                _ => 1.0,
            };
            let category = it["category"].as_str().unwrap_or(match supply {
                "rental" => "equipment",
                "owned" => "tools",
                _ => "hardware",
            });
            let key = format!("job|{category}|{item}|");
            let mut issues = Vec::new();
            let (unit_cost, math) = match periods(per, days) {
                Some(n) => {
                    if days <= 0.0 {
                        issues.push((
                            "missing_duration",
                            format!("{item} is a {supply} per {per}: the job has no duration"),
                            "set the duration (estimate.settings {duration_days})".to_string(),
                        ));
                    }
                    let what = if supply == "owned" { "tool charge" } else { "rental" };
                    (rate * n, format!("{qty} × ${rate:.2}/{per} × {n} {per}(s) of {days} day(s) ({what})"))
                }
                None => (rate, format!("{qty} × ${rate:.2}/{per} ({supply})")),
            };
            let rule = format!("{when} = {v} → {item}");
            let cost = qty * unit_cost;
            if let Some(x) = out.iter_mut().find(|x| x.key == key) {
                x.qty += qty;
                x.net += qty;
                x.material += cost;
                x.cost += cost;
                x.material_steps.push(math);
                x.line["net"] = json!(x.net);
                x.line["total"] = json!(x.qty);
                continue;
            }
            out.push(Priced {
                key: key.clone(),
                line: job_line(&key, category, item, qty, &rule, &els),
                qty,
                net: qty,
                unit: "each".into(),
                material_unit: Some(unit_cost),
                labor_unit_hours: Some(0.0),
                material: cost,
                cost,
                scope: scope_of(&key),
                via: format!("tool rule ({supply})"),
                issues,
                material_steps: vec![math],
                ..Priced::default()
            });
        }
    }
    let mut findings = Vec::new();
    let prices: Vec<&Value> = base["prices"].as_array().into_iter().flatten().collect();
    let all_els = facts.elements.get("always").cloned().unwrap_or_default();
    for r in required {
        let item = r["item"].as_str().unwrap_or_default();
        let qty = r["qty"].as_f64().unwrap_or(0.0);
        if item.is_empty()
            || qty <= 0.0
            || r["waived"].is_string()
            || out.iter().any(|x| x.line["item"].as_str().is_some_and(|i| i.eq_ignore_ascii_case(item)))
        {
            continue;
        }
        let key = format!("job|required|{item}|");
        let rule = format!("BOM required item: {}", r["rule"].as_str().unwrap_or_default());
        let mut pr = Priced {
            key: key.clone(),
            line: job_line(&key, "required", item, qty, &rule, &all_els),
            qty,
            net: qty,
            unit: "each".into(),
            labor_unit_hours: Some(0.0),
            scope: scope_of(&key),
            ..Priced::default()
        };
        match prices.iter().find(|p| p["item"].as_str().is_some_and(|i| i.eq_ignore_ascii_case(item))) {
            Some(p) => {
                let m = p["material"].as_f64().unwrap_or(0.0);
                pr.via = format!("price {} ({})", p["source"].as_str().unwrap_or_default(), p["date"].as_str().unwrap_or_default());
                pr.material_unit = Some(m);
                pr.material = qty * m;
                pr.cost = pr.material;
                pr.material_steps.push(format!("{qty} × ${m:.2} ({})", r["supply"].as_str().unwrap_or("purchase")));
            }
            None => pr.issues.push((
                "missing_equipment",
                format!("the work needs {item} ({}) and no line prices it", r["rule"].as_str().unwrap_or_default()),
                "add it to tool_rules.json (owned: tool charge; else rental × duration) or price it (estimate.set_price)".into(),
            )),
        }
        out.push(pr);
    }
    // The field lessons: work that implies equipment no priced line carries.
    let priced_has = |kw: &str| {
        out.iter().chain(bom).any(|p| p.material_unit.is_some() && p.line["item"].as_str().is_some_and(|i| i.to_ascii_lowercase().contains(kw)))
    };
    let flagged =
        |kw: &str| out.iter().any(|p| p.material_unit.is_none() && p.line["item"].as_str().is_some_and(|i| i.to_ascii_lowercase().contains(kw)));
    for (fact, kw, why) in IMPLIED {
        let v = facts.values.get(fact).copied().unwrap_or(0.0);
        if v > 0.0 && !priced_has(kw) && !flagged(kw) {
            findings.push(json!({
                "kind": "missing_equipment",
                "message": format!("{fact} = {v}: {why}, and no priced line is a {kw}"),
                "fix": format!("add a {kw} to tool_rules.json (owned: tool charge; else rental × duration) or price it (estimate.set_price)"),
                "elements": facts.elements.get(fact).cloned().unwrap_or_default().into_iter().take(10).collect::<Vec<_>>(),
            }));
        }
    }
    (out, findings)
}
