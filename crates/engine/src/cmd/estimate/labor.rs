//! The labor model: difficulty tiers and condition factors on book rates, the driver-based
//! production model blended with field observations, and escalation of dated prices by index.
//! Pure functions over the estimate's state; `price_all` applies them line by line.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

use super::{MAX_ROWS, csv_rows, num, parse_date};

/// Condition factor kinds (the field's reasons labor runs slower or faster).
pub(super) const FACTOR_KINDS: [&str; 7] = ["height", "ceiling", "exposure", "wall", "floor", "learning_curve", "other"];
pub(super) const TIERS: [&str; 3] = ["normal", "difficult", "very_difficult"];
/// A book rate's weight, in installed units, when the model doesn't give its `prior_qty`.
const PRIOR_QTY: f64 = 10.0;
/// Default tier multipliers on a book rate with no per-tier hours (as P-405 tiers them).
const TIER_MULT: [(&str, f64); 2] = [("difficult", 1.4), ("very_difficult", 1.8)];

fn s(r: &BTreeMap<String, String>, k: &str) -> String {
    r.get(k).cloned().unwrap_or_default()
}

/// `factors.csv`: `factor, kind, multiplier, basis, source, trade?, category?, item?` (a row with
/// trade/category/item applies to the lines it matches; one without is applied by
/// `estimate.set_factor`).
pub(super) fn read_factors(text: &str, notes: &mut Vec<String>) -> Map<String, Value> {
    let mut out = Map::new();
    for r in csv_rows(text) {
        let name = s(&r, "factor");
        if name.is_empty() {
            continue;
        }
        let kind = s(&r, "kind").to_ascii_lowercase();
        let basis = s(&r, "basis").to_ascii_lowercase();
        let Some(m) = num(&r, "multiplier").filter(|m| *m > 0.0 && *m <= 10.0) else {
            notes.push(format!("factors: {name} needs a multiplier above 0 and at most 10"));
            continue;
        };
        if !FACTOR_KINDS.contains(&kind.as_str()) {
            notes.push(format!("factors: {name} kind is {}", FACTOR_KINDS.join(", ")));
            continue;
        }
        if basis != "observed" && basis != "assumption" {
            notes.push(format!("factors: {name} basis is observed or assumption"));
            continue;
        }
        let applies: Map<String, Value> =
            ["trade", "category", "item"].iter().filter_map(|k| r.get(*k).filter(|v| !v.is_empty()).map(|v| ((*k).to_string(), json!(v)))).collect();
        out.insert(name, json!({ "kind": kind, "multiplier": m, "basis": basis, "source": s(&r, "source"), "applies": applies }));
    }
    out
}

/// `production_model.json`: `{phases: [{phase, drivers: [{driver, unit, rate, prior_qty?, context?,
/// match: {trade?, category?, item?}}]}]}`, flattened to one entry per driver and context.
pub(super) fn read_production(v: &Value, notes: &mut Vec<String>) -> Vec<Value> {
    let mut out = Vec::new();
    for ph in v["phases"].as_array().into_iter().flatten().take(1_000) {
        let phase = ph["phase"].as_str().unwrap_or_default();
        for d in ph["drivers"].as_array().into_iter().flatten().take(1_000) {
            let name = d["driver"].as_str().unwrap_or_default();
            let rate = d["rate"].as_f64().filter(|r| r.is_finite() && *r >= 0.0);
            let applies = d["match"].as_object().filter(|m| !m.is_empty());
            let (Some(rate), Some(applies)) = (rate.filter(|_| !name.is_empty() && !phase.is_empty()), applies) else {
                notes.push(format!("production_model: driver {name:?} in phase {phase:?} needs a name, a rate (hours per unit) and a match"));
                continue;
            };
            let prior = d["prior_qty"].as_f64().filter(|q| q.is_finite() && *q >= 0.0).unwrap_or(PRIOR_QTY);
            out.push(json!({ "driver": name, "phase": phase, "unit": d["unit"].as_str().unwrap_or("each"), "rate": rate, "prior_qty": prior, "context": d["context"].as_str().unwrap_or_default(), "match": applies }));
        }
    }
    out
}

/// `field_observations.csv`: `driver, context?, qty, hours, date, source`.
pub(super) fn read_observations(text: &str, notes: &mut Vec<String>) -> Vec<Value> {
    let mut out = Vec::new();
    for r in csv_rows(text) {
        let driver = s(&r, "driver");
        match (num(&r, "qty").filter(|q| *q > 0.0), num(&r, "hours").filter(|h| *h >= 0.0)) {
            (Some(qty), Some(hours)) if !driver.is_empty() => out.push(
                json!({ "driver": driver, "context": s(&r, "context"), "qty": qty, "hours": hours, "date": s(&r, "date"), "source": s(&r, "source") }),
            ),
            _ => notes.push(format!("field_observations: a row for {driver:?} needs a driver, qty above 0 and hours")),
        }
    }
    out
}

/// `indexes.csv`: `index, date, value` → {index: [{date, value}]} oldest first.
pub(super) fn read_indexes(text: &str, notes: &mut Vec<String>) -> Map<String, Value> {
    let mut by: BTreeMap<String, Vec<(i64, String, f64)>> = BTreeMap::new();
    for r in csv_rows(text) {
        let name = s(&r, "index");
        let date = s(&r, "date");
        match (parse_date(&date), num(&r, "value").filter(|v| *v > 0.0)) {
            (Some(d), Some(v)) if !name.is_empty() => by.entry(name).or_default().push((d, date, v)),
            _ => notes.push(format!("indexes: a row for {name:?} needs a date (YYYY-MM-DD) and a value above 0")),
        }
    }
    by.into_iter()
        .map(|(k, mut v)| {
            v.sort_by_key(|x| x.0);
            (k, json!(v.into_iter().map(|(_, d, x)| json!({ "date": d, "value": x })).collect::<Vec<_>>()))
        })
        .collect()
}

/// An index's value on a day: its latest value dated on or before it.
fn index_at(base: &Value, name: &str, day: i64) -> Option<(f64, String)> {
    base["indexes"][name]
        .as_array()?
        .iter()
        .filter_map(|r| Some((parse_date(r["date"].as_str()?)?, r["value"].as_f64()?, r["date"].as_str()?.to_string())))
        .filter(|(d, _, _)| *d <= day)
        .max_by_key(|(d, _, _)| *d)
        .map(|(_, v, d)| (v, d))
}

/// Escalation of a dated price to `today` by its index: `Ok(Some((ratio, text)))`, `Ok(None)` for
/// a price with no index (or no `today`), `Err(message)` when the index can't give a ratio.
pub(super) fn escalation(base: &Value, today: Option<(i64, &str)>, price: &Value) -> Result<Option<(f64, String)>, String> {
    let Some(name) = price["index"].as_str().filter(|n| !n.is_empty()) else { return Ok(None) };
    let Some((t, tstr)) = today else { return Ok(None) };
    let pdate = price["date"].as_str().unwrap_or_default();
    let d = parse_date(pdate).ok_or_else(|| format!("price indexed to {name} has no date"))?;
    let (from, fdate) = index_at(base, name, d).ok_or_else(|| format!("index {name} has no value on or before {pdate}"))?;
    let (to, tdate) = index_at(base, name, t).ok_or_else(|| format!("index {name} has no value on or before {tstr}"))?;
    let ratio = to / from;
    Ok(Some((ratio, format!("escalated × {ratio:.4} = index {name} {to} ({tdate}) ÷ {from} ({fdate}), price dated {pdate} to {tstr}"))))
}

/// Does a `{trade?, category?, item?}` match a BOM line (trade and category exactly, item as a
/// part of the line's item; all case-insensitive)? An empty match matches nothing.
pub(super) fn matches(applies: &Value, line: &Value) -> bool {
    let Some(m) = applies.as_object().filter(|m| !m.is_empty()) else { return false };
    m.iter().all(|(k, want)| {
        let want = want.as_str().unwrap_or_default().to_ascii_lowercase();
        let have = line[k.as_str()].as_str().unwrap_or_default().to_ascii_lowercase();
        match k.as_str() {
            "item" => have.contains(&want),
            "trade" | "category" => have == want,
            _ => false,
        }
    })
}

/// One production driver with its rate blended from book and observations.
#[derive(Clone, Debug)]
pub(super) struct Driver {
    pub(super) name: String,
    pub(super) phase: String,
    pub(super) unit: String,
    pub(super) context: String,
    pub(super) applies: Value,
    pub(super) book: f64,
    pub(super) prior_qty: f64,
    pub(super) obs: usize,
    pub(super) obs_qty: f64,
    pub(super) obs_hours: f64,
    pub(super) rate: f64,
}

impl Driver {
    pub(super) fn text(&self) -> String {
        format!(
            "driver {} ({}{}): {:.4} h/{} = (book {} h × {} {} + {} observation(s): {} h over {} {}) ÷ {} {}",
            self.name,
            self.phase,
            if self.context.is_empty() { String::new() } else { format!(", context {}", self.context) },
            self.rate,
            self.unit,
            self.book,
            self.prior_qty,
            self.unit,
            self.obs,
            self.obs_hours,
            self.obs_qty,
            self.unit,
            self.prior_qty + self.obs_qty,
            self.unit
        )
    }
}

/// Every observation: the baseline's and those added with `estimate.observe`.
pub(super) fn observations<'a>(st: &'a Map<String, Value>, base: &'a Value) -> impl Iterator<Item = &'a Value> {
    base["observations"].as_array().into_iter().flatten().chain(st.get("observations").and_then(Value::as_array).into_iter().flatten())
}

/// The production model's drivers for the job's context (`settings.context`): per driver, the
/// entry for that context, else its context-free one. Rate = (book × prior_qty + observed hours)
/// ÷ (prior_qty + observed quantity): the book is a prior, observations weigh by quantity.
pub(super) fn drivers(st: &Map<String, Value>, base: &Value) -> Vec<Driver> {
    let ctx = st.get("settings").and_then(|x| x["context"].as_str()).unwrap_or_default().to_string();
    let entries: Vec<&Value> = base["production"].as_array().into_iter().flatten().collect();
    let mut names: Vec<(String, String)> = Vec::new();
    for e in &entries {
        let k = (e["driver"].as_str().unwrap_or_default().to_string(), e["phase"].as_str().unwrap_or_default().to_string());
        if !names.contains(&k) {
            names.push(k);
        }
    }
    let mut out = Vec::new();
    for (name, phase) in names {
        let of = |c: &str| {
            entries.iter().find(|e| {
                e["driver"].as_str() == Some(name.as_str())
                    && e["phase"].as_str() == Some(phase.as_str())
                    && e["context"].as_str().unwrap_or_default().eq_ignore_ascii_case(c)
            })
        };
        let Some(e) = of(&ctx).or_else(|| of("")) else { continue };
        let context = e["context"].as_str().unwrap_or_default().to_string();
        let (mut n, mut q, mut h) = (0usize, 0.0, 0.0);
        for o in observations(st, base).filter(|o| {
            o["driver"].as_str().is_some_and(|d| d.eq_ignore_ascii_case(&name))
                && o["context"].as_str().unwrap_or_default().eq_ignore_ascii_case(&context)
        }) {
            if let (Some(oq), Some(oh)) =
                (o["qty"].as_f64().filter(|x| x.is_finite() && *x > 0.0), o["hours"].as_f64().filter(|x| x.is_finite() && *x >= 0.0))
            {
                n += 1;
                q += oq;
                h += oh;
            }
        }
        let book = e["rate"].as_f64().unwrap_or(0.0);
        let prior = e["prior_qty"].as_f64().unwrap_or(PRIOR_QTY);
        let w = prior + q;
        let rate = if w > 0.0 { (book * prior + h) / w } else { book };
        out.push(Driver {
            name: name.clone(),
            phase,
            unit: e["unit"].as_str().unwrap_or("each").to_string(),
            context,
            applies: e["match"].clone(),
            book,
            prior_qty: prior,
            obs: n,
            obs_qty: q,
            obs_hours: h,
            rate,
        });
    }
    out
}

/// A line's labor per unit: `(hours per unit, explanation steps, per-phase hours per unit)`.
/// Production drivers matching the line replace its book rate; otherwise the book rate at the
/// line's tier. Condition factors multiply either.
pub(super) fn line_labor(
    st: &Map<String, Value>,
    base: &Value,
    drivers: &[Driver],
    key: &str,
    line: &Value,
    book: Option<f64>,
) -> (Option<f64>, Vec<String>, Vec<(String, f64)>) {
    let mut steps = Vec::new();
    let mut phases: Vec<(String, f64)> = Vec::new();
    let mine: Vec<&Driver> = drivers.iter().filter(|d| matches(&d.applies, line)).collect();
    let unit = if mine.is_empty() {
        let tier = st
            .get("tiers")
            .and_then(|t| t.get(key))
            .and_then(|t| t["tier"].as_str())
            .or_else(|| st.get("settings").and_then(|x| x["tier"].as_str()))
            .unwrap_or("normal");
        let Some(b) = book else { return (None, steps, phases) };
        let h = if tier == "normal" {
            b
        } else {
            let item = line["item"].as_str().unwrap_or_default();
            match base["items"][item]["labor"][tier].as_f64().filter(|x| x.is_finite() && *x >= 0.0) {
                Some(t) => {
                    steps.push(format!("tier {tier}: {t} h/unit (item labor table, {tier})"));
                    t
                }
                None => {
                    let m = base["tier_multipliers"][tier]
                        .as_f64()
                        .filter(|x| x.is_finite() && *x > 0.0)
                        .or_else(|| TIER_MULT.iter().find(|(t, _)| *t == tier).map(|(_, m)| *m))
                        .unwrap_or(1.0);
                    steps.push(format!("tier {tier}: book {b} h × {m} = {:.4} h/unit", b * m));
                    b * m
                }
            }
        };
        phases.push(("unphased".into(), h));
        h
    } else {
        let mut sum = 0.0;
        for d in &mine {
            steps.push(d.text());
            sum += d.rate;
            match phases.iter_mut().find(|(p, _)| *p == d.phase) {
                Some(p) => p.1 += d.rate,
                None => phases.push((d.phase.clone(), d.rate)),
            }
        }
        sum
    };
    let fs = factors_for(st, base, key, line);
    let mut product = 1.0;
    for (name, m, why) in &fs {
        product *= m;
        steps.push(format!("× {m} {name} ({why})"));
    }
    if !fs.is_empty() {
        steps.push(format!("rate {unit:.4} h × factor {product:.4} = {:.4} h/unit", unit * product));
    }
    for p in &mut phases {
        p.1 *= product;
    }
    (Some(unit * product), steps, phases)
}

/// The condition factors on a line: those whose rule matches it, and those set on it by
/// `estimate.set_factor`. (name, multiplier, why).
fn factors_for(st: &Map<String, Value>, base: &Value, key: &str, line: &Value) -> Vec<(String, f64, String)> {
    let mut out: Vec<(String, f64, String)> = Vec::new();
    let Some(all) = base["factors"].as_object() else { return out };
    let describe = |f: &Value, how: &str| {
        format!(
            "{}, {}: {}; {how}",
            f["kind"].as_str().unwrap_or_default(),
            f["basis"].as_str().unwrap_or_default(),
            f["source"].as_str().unwrap_or_default()
        )
    };
    for (name, f) in all {
        if matches(&f["applies"], line) {
            out.push((name.clone(), f["multiplier"].as_f64().unwrap_or(1.0), describe(f, "by rule")));
        }
    }
    for set in st.get("factors").and_then(|m| m.get(key)).and_then(Value::as_array).into_iter().flatten() {
        let name = set["factor"].as_str().unwrap_or_default();
        if out.iter().any(|(n, _, _)| n == name) {
            continue;
        }
        if let Some(f) = all.get(name) {
            out.push((
                name.to_string(),
                f["multiplier"].as_f64().unwrap_or(1.0),
                describe(f, &format!("set: {}", set["reason"].as_str().unwrap_or_default())),
            ));
        }
    }
    out
}

/// A past price's spread from a price: the history rows and benchmark unit prices for the same
/// item (and size), and the stated tolerance. `Some(message)` when the price is off.
pub(super) fn off_history(base: &Value, item: &str, size: &str, material: f64) -> Option<String> {
    let same = |p: &&Value| {
        p["item"].as_str().is_some_and(|i| i.eq_ignore_ascii_case(item)) && p["size"].as_str().unwrap_or_default().eq_ignore_ascii_case(size)
    };
    let past: Vec<f64> = base["history"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(base["benchmarks"]["unit_prices"].as_array().into_iter().flatten())
        .filter(same)
        .filter_map(|p| p["material"].as_f64().filter(|m| m.is_finite() && *m > 0.0))
        .take(MAX_ROWS)
        .collect();
    if past.is_empty() {
        return None;
    }
    let avg = past.iter().sum::<f64>() / past.len() as f64;
    let tol = base["benchmarks"]["price_tolerance_pct"].as_f64().filter(|t| t.is_finite() && *t >= 0.0).unwrap_or(25.0);
    let off = (material - avg) / avg * 100.0;
    (off.abs() > tol)
        .then(|| format!("{item} ${material:.2} is {off:+.0}% from ${avg:.2}, the average of {} prior price(s) (tolerance {tol}%)", past.len()))
}
