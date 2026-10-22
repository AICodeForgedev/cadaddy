//! Rates from public projects (Q341): each project's quantities per trade and driver (from the
//! BIM build of its bid documents) beside its published bid results, fitted for unit rates.
//!
//! - `calibrate.project {pack, id, …, quantities | from_wholejob, totals?, total?, hours?}`: one
//!   project into a local `.jsonl` pack. Ids are anonymous; plans, specs and project names never
//!   go in a pack.
//! - `calibrate.fit {packs, target?, priors?, markups?, regions?, index?, base_date?, …}`: observed
//!   total (per trade where known, else per project) ≈ Σ quantity × rate × region × escalation ×
//!   (1 + markup), solved as a Bayesian (ridge) regression on relative error with priors, each rate
//!   with its uncertainty and the number of projects behind it; held-out projects measure the
//!   prediction error; too few projects give "insufficient data", never a number. With `hours`
//!   (certified payrolls) as the target, the rates are field hours per unit.
//! - `calibrate.payroll {rows | path, map?}`: certified-payroll hours by trade classification.
//! - `calibrate.topdown {values, census | ratios, …}`: the top-down estimate from census construction
//!   figures (value of work, subcontracted out, worker hours and wages, from which it computes the
//!   self-performed share, field hours per dollar and wage), beside the bottom-up hours, gaps flagged.
//!
//! Electrical results (divisions 26–28) go only to a private pack.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::wholejob::{append, is_electrical, median, norm, r2, strings, trade_info};
use super::*;
use crate::{Result, Session};

const MAX_PROJECTS: usize = 100_000;
const MAX_RATES: usize = 400;
const MAX_ROWS: usize = 1_000_000;

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite() && *x >= 0.0 && *x < 1e13)
}

fn pack_param(cmd: &str, p: &Value, key: &str) -> Result<String> {
    str_param(p, key)
        .map(str::trim)
        .filter(|x| x.to_ascii_lowercase().ends_with(".jsonl"))
        .map(str::to_string)
        .ok_or_else(|| bad(cmd, format!("{key} required: a .jsonl file")))
}

/// `{trade: {driver: qty}}` from a JSON object, trades and drivers normalised.
fn quantities(cmd: &str, v: &Value) -> Result<BTreeMap<String, BTreeMap<String, f64>>> {
    let mut out: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    for (t, ds) in v.as_object().ok_or_else(|| bad(cmd, "quantities: {trade: {driver: quantity}}"))?.iter().take(200) {
        for (d, q) in ds.as_object().ok_or_else(|| bad(cmd, format!("quantities.{t}: {{driver: quantity}}")))?.iter().take(MAX_RATES) {
            let q = num(q).ok_or_else(|| bad(cmd, format!("{t}/{d}: a quantity is 0 or more")))?;
            if q > 0.0 {
                *out.entry(norm(t)).or_default().entry(norm(d)).or_default() += q;
            }
        }
    }
    Ok(out)
}

fn amounts(cmd: &str, v: Option<&Value>) -> Result<BTreeMap<String, f64>> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(BTreeMap::new()) };
    v.as_object()
        .ok_or_else(|| bad(cmd, "totals and hours are {trade: amount}"))?
        .iter()
        .take(200)
        .map(|(t, a)| num(a).filter(|a| *a > 0.0).map(|a| (norm(t), a)).ok_or_else(|| bad(cmd, format!("{t}: an amount is more than 0"))))
        .collect()
}

fn run_project(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "calibrate.project";
    let pack = pack_param(cmd, p, "pack")?;
    let id = str_param(p, "id")
        .map(str::trim)
        .filter(|x| !x.is_empty() && x.len() <= 40)
        .ok_or_else(|| bad(cmd, "id required: an anonymous project id (never the project's name)"))?;
    let q = if bool_or(p, "from_wholejob", false) {
        let text = s.doc()?.building_spec.clone().ok_or_else(|| bad(cmd, "the drawing has no building spec"))?;
        let sp = cadaddy_walk3d::BuildingSpec::from_json(&text).map_err(|e| bad(cmd, e))?;
        let w: super::wholejob::WholeJob = serde_json::from_value(sp.wholejob).unwrap_or_default();
        let mut out: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
        for (t, tr) in &w.trades {
            for l in tr.lines.iter().filter(|l| l.qty > 0.0) {
                *out.entry(norm(t)).or_default().entry(norm(&format!("{} [{}]", l.item, l.unit))).or_default() += l.qty;
            }
        }
        out
    } else {
        quantities(cmd, p.get("quantities").ok_or_else(|| bad(cmd, "quantities required (or from_wholejob: true)"))?)?
    };
    if q.is_empty() {
        return Err(bad(cmd, "no quantities"));
    }
    let totals = amounts(cmd, p.get("totals"))?;
    let hours = amounts(cmd, p.get("hours"))?;
    let total = p.get("total").and_then(num).filter(|t| *t > 0.0);
    if totals.is_empty() && hours.is_empty() && total.is_none() {
        return Err(bad(cmd, "a result is required: totals per trade, a project total, or hours"));
    }
    let row = json!({
        "kind": "project", "id": id, "building_type": str_param(p, "building_type").unwrap_or(""), "region": str_param(p, "region").unwrap_or(""),
        "bid_date": str_param(p, "bid_date").unwrap_or(""), "delivery": str_param(p, "delivery").unwrap_or(""),
        "quantities": q, "totals": totals, "total": total, "hours": hours,
    });
    let replaced = put_project(&pack, id, &row)?;
    Ok(json!({"pack": pack, "id": id, "trades": q.len(), "replaced": replaced}))
}

/// Writes a project row into its pack, replacing any earlier row with the same id, so a retried
/// `calibrate.project` never counts one project twice. True when a row was replaced.
fn put_project(path: &str, id: &str, row: &Value) -> Result<bool> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut out = String::with_capacity(text.len() + 256);
        let mut replaced = false;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let same = serde_json::from_str::<Value>(line).is_ok_and(|v| v["kind"] == "project" && v["id"].as_str().map(str::trim) == Some(id));
            if same {
                replaced = true;
            } else {
                out += line;
                out.push('\n');
            }
        }
        out += &row.to_string();
        out.push('\n');
        crate::writefile::write(path, out.as_bytes()).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
        Ok(replaced)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = id;
        append(path, std::slice::from_ref(row)).map(|()| false)
    }
}

#[derive(Clone, Debug)]
struct Project {
    id: String,
    building_type: String,
    region: String,
    month: Option<f64>,
    q: BTreeMap<String, BTreeMap<String, f64>>,
    totals: BTreeMap<String, f64>,
    total: Option<f64>,
    hours: BTreeMap<String, f64>,
}

/// Months since year 0 from `YYYY`, `YYYY-MM` or `YYYY-MM-DD` (a bare year is mid-year).
fn month(s: &str) -> Option<f64> {
    let mut it = s.trim().split('-');
    let y: f64 = it.next()?.parse::<u16>().ok()?.into();
    match it.next() {
        Some(m) => m.parse::<u8>().ok().filter(|m| (1..=12).contains(m)).map(|m| y * 12.0 + f64::from(m) - 1.0),
        None => Some(y * 12.0 + 6.0),
    }
}

/// A cost index at a month: linear between the given points, held flat past the ends.
fn index_at(idx: &[(f64, f64)], m: f64) -> Option<f64> {
    let (first, last) = (idx.first()?, idx.last()?);
    if m <= first.0 {
        return Some(first.1);
    }
    if m >= last.0 {
        return Some(last.1);
    }
    idx.windows(2).find(|w| w.first().is_some_and(|a| a.0 <= m) && w.get(1).is_some_and(|b| m <= b.0)).and_then(|w| {
        let (a, b) = (w.first()?, w.get(1)?);
        let f = if b.0 > a.0 { (m - a.0) / (b.0 - a.0) } else { 0.0 };
        Some(a.1 + f * (b.1 - a.1))
    })
}

/// The projects in the packs, one per id: a later row (in pack order, then line order) replaces
/// an earlier one with the same id. Also the ids that appeared more than once, with their counts.
fn load_projects(cmd: &str, packs: &[String]) -> Result<(Vec<Project>, BTreeMap<String, usize>)> {
    let mut by_id: BTreeMap<String, Project> = BTreeMap::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for path in packs {
        let text =
            crate::readfile::read(path.trim()).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
        for line in text.lines().take(MAX_ROWS) {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            if v["kind"] != "project" {
                continue;
            }
            let id = v["id"].as_str().unwrap_or("").trim().to_string();
            if by_id.len() >= MAX_PROJECTS && !by_id.contains_key(&id) {
                continue;
            }
            let Ok(q) = quantities(cmd, &v["quantities"]) else { continue };
            *seen.entry(id.clone()).or_default() += 1;
            by_id.insert(
                id.clone(),
                Project {
                    id,
                    building_type: norm(v["building_type"].as_str().unwrap_or("")),
                    region: norm(v["region"].as_str().unwrap_or("")),
                    month: v["bid_date"].as_str().and_then(month),
                    q,
                    totals: amounts(cmd, Some(&v["totals"])).unwrap_or_default(),
                    total: num(&v["total"]).filter(|t| *t > 0.0),
                    hours: amounts(cmd, Some(&v["hours"])).unwrap_or_default(),
                },
            );
        }
    }
    seen.retain(|_, n| *n > 1);
    // Sorted by id (the map's order), which the holdout split relies on.
    Ok((by_id.into_values().collect(), seen))
}

/// One observation: a total over some trades' drivers.
struct Obs {
    project: usize,
    cols: Vec<(usize, f64)>,
    y: f64,
}

/// Cholesky solve of the symmetric positive-definite `a` (k×k, row-major) for each right side.
fn cholesky(a: &[f64], k: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0; k * k];
    for i in 0..k {
        for j in 0..=i {
            let mut s = *a.get(i * k + j)?;
            for t in 0..j {
                s -= l.get(i * k + t)? * l.get(j * k + t)?;
            }
            if i == j {
                if !(s > 0.0 && s.is_finite()) {
                    return None;
                }
                *l.get_mut(i * k + i)? = s.sqrt();
            } else {
                *l.get_mut(i * k + j)? = s / l.get(j * k + j)?;
            }
        }
    }
    Some(l)
}

fn solve(l: &[f64], k: usize, b: &[f64]) -> Option<Vec<f64>> {
    let mut y = vec![0.0; k];
    for i in 0..k {
        let mut s = *b.get(i)?;
        for t in 0..i {
            s -= l.get(i * k + t)? * y.get(t)?;
        }
        *y.get_mut(i)? = s / l.get(i * k + i)?;
    }
    let mut x = vec![0.0; k];
    for i in (0..k).rev() {
        let mut s = *y.get(i)?;
        for t in i + 1..k {
            s -= l.get(t * k + i)? * x.get(t)?;
        }
        *x.get_mut(i)? = s / l.get(i * k + i)?;
    }
    Some(x)
}

struct Fit {
    mean: Vec<f64>,
    sd: Vec<f64>,
    sigma: f64,
}

/// Bayesian ridge on relative error: each row is divided by its total, so a 10 % miss weighs
/// the same on a small trade as on a large one. Priors N(mu, s²) per rate; the noise is
/// re-estimated from the residuals.
fn fit(obs: &[&Obs], k: usize, mu: &[f64], s: &[f64]) -> Option<Fit> {
    let mut sigma2 = 0.1f64.powi(2);
    let mut out = None;
    for _ in 0..4 {
        let mut a = vec![0.0; k * k];
        let mut b = vec![0.0; k];
        for o in obs {
            for &(i, xi) in &o.cols {
                let xi = xi / o.y;
                *b.get_mut(i)? += xi / sigma2;
                for &(j, xj) in &o.cols {
                    *a.get_mut(i * k + j)? += xi * (xj / o.y) / sigma2;
                }
            }
        }
        for j in 0..k {
            let sj = s.get(j)?.max(1e-12);
            *a.get_mut(j * k + j)? += 1.0 / (sj * sj);
            *b.get_mut(j)? += mu.get(j)? / (sj * sj);
        }
        let l = cholesky(&a, k)?;
        let mean = solve(&l, k, &b)?;
        let mut sd = vec![0.0; k];
        for j in 0..k {
            let mut e = vec![0.0; k];
            *e.get_mut(j)? = 1.0;
            *sd.get_mut(j)? = solve(&l, k, &e)?.get(j)?.max(0.0).sqrt();
        }
        let ss: f64 =
            obs.iter().map(|o| (1.0 - o.cols.iter().map(|&(j, x)| x * mean.get(j).copied().unwrap_or(0.0)).sum::<f64>() / o.y).powi(2)).sum();
        let dof = obs.len().saturating_sub(k).max(1) as f64;
        let next = if obs.len() > k { (ss / dof).max(1e-8) } else { sigma2 };
        out = Some(Fit { mean, sd, sigma: next.sqrt() });
        if (next - sigma2).abs() < 1e-12 {
            break;
        }
        sigma2 = next;
    }
    out
}

fn run_fit(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "calibrate.fit";
    let packs = strings(p, "packs").filter(|v| !v.is_empty()).ok_or_else(|| bad(cmd, "packs required: [.jsonl project packs]"))?;
    let hours = match str_param(p, "target").unwrap_or("cost") {
        "cost" => false,
        "hours" => true,
        t => return Err(bad(cmd, format!("target is cost or hours, not {}", t.chars().take(20).collect::<String>()))),
    };
    let min_projects = p.get("min_projects").and_then(Value::as_u64).unwrap_or(3).clamp(1, 1000) as usize;
    let holdout_every = p.get("holdout_every").and_then(Value::as_u64).unwrap_or(5).min(1000) as usize;
    let (bt, region) = (str_param(p, "building_type").map(norm), str_param(p, "region").map(norm));
    let (mut projects, dups) = load_projects(cmd, &packs)?;
    projects.retain(|x| bt.as_ref().is_none_or(|b| x.building_type == *b) && region.as_ref().is_none_or(|r| x.region == *r));
    let obj = |k: &str| p.get(k).and_then(Value::as_object).cloned().unwrap_or_default();
    let regions: BTreeMap<String, f64> = obj("regions").iter().filter_map(|(k, v)| num(v).filter(|v| *v > 0.0).map(|v| (norm(k), v))).collect();
    let markups: BTreeMap<String, f64> = obj("markups").iter().filter_map(|(k, v)| num(v).filter(|v| *v <= 100.0).map(|v| (norm(k), v))).collect();
    let mut idx: Vec<(f64, f64)> = obj("index").iter().filter_map(|(k, v)| Some((month(k)?, num(v).filter(|v| *v > 0.0)?))).collect();
    idx.sort_by(|a, b| a.0.total_cmp(&b.0));
    let base = match str_param(p, "base_date") {
        Some(d) => Some(month(d).ok_or_else(|| bad(cmd, "base_date is YYYY, YYYY-MM or YYYY-MM-DD"))?),
        None => idx.last().map(|x| x.0),
    };
    let base_index = base.and_then(|b| index_at(&idx, b));
    // A project's factor: its region, and its bid date's index over the base date's.
    let mut warnings: BTreeSet<String> =
        dups.iter().map(|(id, n)| format!("{id}: {n} rows with this id, only the last is used (one project counts once)")).collect();
    let factor = |x: &Project, warnings: &mut BTreeSet<String>| -> f64 {
        if hours {
            return 1.0;
        }
        let r = if x.region.is_empty() { 1.0 } else { regions.get(&x.region).copied().unwrap_or(1.0) };
        let e = match (x.month.and_then(|m| index_at(&idx, m)), base_index) {
            (Some(a), Some(b)) if b > 0.0 => a / b,
            _ => {
                if !idx.is_empty() {
                    warnings.insert(format!("{}: no bid date, not escalated", x.id));
                }
                1.0
            }
        };
        r * e
    };
    let markup = |t: &str| if hours { 0.0 } else { markups.get(t).copied().unwrap_or_else(|| trade_info(t).2) };
    // The rates: every (trade, driver) any project has.
    let mut cols: BTreeMap<(String, String), usize> = BTreeMap::new();
    for x in &projects {
        for (t, ds) in &x.q {
            for d in ds.keys() {
                let n = cols.len();
                if n >= MAX_RATES && !cols.contains_key(&(t.clone(), d.clone())) {
                    return Err(bad(cmd, format!("more than {MAX_RATES} rates: fit one building type or region at a time")));
                }
                cols.entry((t.clone(), d.clone())).or_insert(n);
            }
        }
    }
    let k = cols.len();
    // The observations: per trade where the result names it, else the project's total.
    let mut obs = Vec::new();
    for (pi, x) in projects.iter().enumerate() {
        let f = factor(x, &mut warnings);
        let row = |trades: &[&String]| -> Vec<(usize, f64)> {
            trades
                .iter()
                .flat_map(|t| {
                    x.q.get(*t)
                        .into_iter()
                        .flatten()
                        .filter_map(|(d, q)| Some((*cols.get(&((*t).clone(), d.clone()))?, q * f * (1.0 + markup(t) / 100.0))))
                })
                .collect()
        };
        let results = if hours { &x.hours } else { &x.totals };
        for (t, y) in results {
            let c = row(&[t]);
            if !c.is_empty() {
                obs.push(Obs { project: pi, cols: c, y: *y });
            }
        }
        if !hours
            && x.totals.is_empty()
            && let Some(y) = x.total
        {
            let ts: Vec<&String> = x.q.keys().collect();
            obs.push(Obs { project: pi, cols: row(&ts), y });
        }
    }
    // Priors: given, else a weak prior around 0 scaled to the data.
    let mut mu = vec![0.0; k];
    let mut s = vec![1.0; k];
    let priors = obj("priors");
    for ((t, d), &j) in &cols {
        let given = priors.get(&format!("{t}/{d}")).or_else(|| priors.iter().find(|(key, _)| norm(key) == format!("{t}/{d}")).map(|x| x.1));
        if let Some((m, sd)) = given.and_then(|g| Some((num(&g["rate"])?, num(&g["sd"]).filter(|v| *v > 0.0)?))) {
            mu[j] = m;
            s[j] = sd;
        } else {
            let scale = median(obs.iter().filter_map(|o| o.cols.iter().find(|c| c.0 == j).map(|c| o.y / c.1)).collect()).unwrap_or(1.0);
            s[j] = (scale * 100.0).max(1e-6);
        }
    }
    // How many projects stand behind each rate.
    let mut behind: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); k];
    for o in &obs {
        for c in &o.cols {
            if let Some(b) = behind.get_mut(c.0) {
                b.insert(o.project);
            }
        }
    }
    let all: Vec<&Obs> = obs.iter().collect();
    let result = if k == 0 || all.is_empty() { None } else { fit(&all, k, &mu, &s) };
    // Held-out projects: every n-th, fitted without them and predicted.
    let mut holdout = json!({"projects": 0, "observations": 0, "mape_pct": null});
    if holdout_every >= 2 && k > 0 {
        let (test, train): (Vec<&Obs>, Vec<&Obs>) = all.iter().partition(|o| o.project % holdout_every == holdout_every - 1);
        if !test.is_empty()
            && !train.is_empty()
            && let Some(h) = fit(&train, k, &mu, &s)
        {
            let errs: Vec<f64> = test
                .iter()
                .map(|o| (o.cols.iter().map(|&(j, x)| x * h.mean.get(j).copied().unwrap_or(0.0)).sum::<f64>() - o.y).abs() / o.y * 100.0)
                .collect();
            let projects: BTreeSet<usize> = test.iter().map(|o| o.project).collect();
            holdout = json!({"projects": projects.len(), "observations": errs.len(), "mape_pct": r2(errs.iter().sum::<f64>() / errs.len() as f64)});
        }
    }
    let mut rates = Vec::new();
    for ((t, d), &j) in &cols {
        let n = behind.get(j).map_or(0, BTreeSet::len);
        let private = is_electrical(t, trade_info(t).0);
        let mut r = json!({"trade": t, "driver": d, "projects": n, "private": private});
        match &result {
            Some(f) if n >= min_projects => {
                let (m, sd) = (f.mean.get(j).copied().unwrap_or(0.0), f.sd.get(j).copied().unwrap_or(0.0));
                r["rate"] = json!(sig(m));
                r["sd"] = json!(sig(sd));
                r["quality"] = json!(if m > 0.0 && sd / m <= 0.25 { "fitted" } else { "weak" });
            }
            _ => {
                r["rate"] = Value::Null;
                r["quality"] = json!("insufficient data");
            }
        }
        rates.push(r);
    }
    // The pack: electrical only into a private one.
    let mut written = json!(null);
    if let Some(out) = str_param(p, "out").map(str::trim).filter(|x| !x.is_empty()) {
        if !out.to_ascii_lowercase().ends_with(".jsonl") {
            return Err(bad(cmd, "out is a .jsonl pack"));
        }
        let private = bool_or(p, "private", false);
        let rows: Vec<Value> = rates
            .iter()
            .filter(|r| r["rate"].is_number() && (private || r["private"] != true))
            .map(|r| {
                let mut r = r.clone();
                r["kind"] = json!("rate");
                r["unit"] = json!(if hours { "hours" } else { "dollars" });
                r["base_date"] = json!(str_param(p, "base_date").unwrap_or(""));
                r["building_type"] = json!(bt.clone().unwrap_or_default());
                r["region"] = json!(region.clone().unwrap_or_default());
                r
            })
            .collect();
        append(out, &rows)?;
        let withheld: Vec<String> = rates
            .iter()
            .filter(|r| !private && r["private"] == true && r["rate"].is_number())
            .map(|r| format!("{}/{}", r["trade"].as_str().unwrap_or(""), r["driver"].as_str().unwrap_or("")))
            .collect();
        written = json!({"path": out, "rates": rows.len(), "withheld_electrical": withheld});
    }
    Ok(json!({
        "target": if hours { "hours" } else { "cost" }, "projects": projects.len(), "observations": obs.len(), "rates": rates,
        "noise_pct": result.as_ref().map(|f| r2(f.sigma * 100.0)), "holdout": holdout, "written": written, "warnings": warnings,
    }))
}

/// Four significant figures.
fn sig(v: f64) -> f64 {
    if v == 0.0 || !v.is_finite() {
        return 0.0;
    }
    let mag = 10f64.powi(3 - v.abs().log10().floor() as i32);
    (v * mag).round() / mag
}

/// A certified-payroll classification's trade.
fn classify(c: &str) -> Option<&'static str> {
    let c = c.to_ascii_lowercase();
    // Concrete crafts come before "mason": a cement mason finishes concrete, not masonry.
    const MAP: &[(&str, &str)] = &[
        ("cement mason", "concrete"),
        ("concrete finisher", "concrete"),
        ("electric", "electrical"),
        ("lineman", "electrical"),
        ("sprinkler", "fire protection"),
        ("plumb", "plumbing"),
        ("pipefitter", "mechanical"),
        ("steamfitter", "mechanical"),
        ("sheet metal", "mechanical"),
        ("hvac", "mechanical"),
        ("roofer", "roofing"),
        ("painter", "painting"),
        ("drywall", "drywall"),
        ("taper", "drywall"),
        ("tile", "tile"),
        ("floor", "flooring"),
        ("glazier", "openings"),
        ("iron", "metals"),
        ("cement", "concrete"),
        ("concrete", "concrete"),
        ("brick", "masonry"),
        ("mason", "masonry"),
        ("carpenter", "carpentry"),
        ("operator", "earthwork"),
        ("laborer", "general"),
    ];
    MAP.iter().find(|(k, _)| c.contains(k)).map(|x| x.1)
}

fn run_payroll(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "calibrate.payroll";
    let map: BTreeMap<String, String> = p
        .get("map")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, v)| Some((norm(k), norm(v.as_str()?)))).collect())
        .unwrap_or_default();
    let mut hours_cols: Vec<String> = Vec::new();
    let rows: Vec<(String, f64)> = if let Some(rows) = p.get("rows").and_then(Value::as_array) {
        rows.iter().take(MAX_ROWS).filter_map(|r| Some((r["classification"].as_str()?.to_string(), num(&r["hours"])?))).collect()
    } else {
        let path = str_param(p, "path").ok_or_else(|| bad(cmd, "rows or path (a payroll CSV with classification and hours columns) required"))?;
        let text =
            crate::readfile::read(path.trim()).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
        let table = super::estimate::csv_rows(&text);
        let col = |names: &[&str]| table.first().and_then(|r| r.keys().find(|h| names.iter().any(|n| h.to_ascii_lowercase().contains(n))).cloned());
        let c = col(&["classification", "craft", "trade"]).ok_or_else(|| bad(cmd, "no classification column"))?;
        let headers: Vec<String> = table.first().map(|r| r.keys().cloned().collect()).unwrap_or_default();
        hours_cols = hours_columns(&headers).map_err(|e| bad(cmd, e))?;
        table
            .iter()
            .take(MAX_ROWS)
            .filter_map(|r| {
                // Every hours column of the row (an empty cell is 0); a row with none is skipped.
                let mut sum = 0.0;
                let mut any = false;
                for h in &hours_cols {
                    let cell = r.get(h).map_or("", |x| x.trim());
                    if cell.is_empty() {
                        continue;
                    }
                    sum += cell.parse::<f64>().ok().filter(|x| x.is_finite() && *x >= 0.0 && *x < 1e9)?;
                    any = true;
                }
                if !any {
                    return None;
                }
                Some((r.get(&c)?.clone(), sum))
            })
            .collect()
    };
    let mut by: BTreeMap<String, f64> = BTreeMap::new();
    let mut unmapped: BTreeMap<String, f64> = BTreeMap::new();
    for (c, h) in rows {
        match map.get(&norm(&c)).cloned().or_else(|| classify(&c).map(str::to_string)) {
            Some(t) => *by.entry(t).or_default() += h,
            None => *unmapped.entry(c.trim().chars().take(80).collect()).or_default() += h,
        }
    }
    Ok(json!({
        "hours": by.into_iter().map(|(k, v)| (k, json!(r2(v)))).collect::<Map<String, Value>>(),
        "unmapped": unmapped.into_iter().map(|(k, v)| (k, json!(r2(v)))).collect::<Map<String, Value>>(),
        "hours_columns": hours_cols,
    }))
}

/// The payroll CSV's hours columns to add up for a row's hours, never a guess:
/// - a single hours column, or the one "total" hours column when there is one;
/// - else every straight-time and overtime column (ST + OT + double time), or every per-day
///   column (Mon … Sun, Day 1 …);
/// - else an error naming the columns.
fn hours_columns(headers: &[String]) -> std::result::Result<Vec<String>, String> {
    const NOT_HOURS: &[&str] = &["rate", "pay", "wage", "wages", "gross", "net", "amount", "deduction", "deductions", "fringe", "fringes"];
    const ST_OT: &[&str] = &["st", "straight", "reg", "regular", "ot", "overtime", "dt", "double", "doubletime"];
    const DAYS: &[&str] = &[
        "mon",
        "tue",
        "tues",
        "wed",
        "thu",
        "thur",
        "thurs",
        "fri",
        "sat",
        "sun",
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    let words = |h: &str| -> Vec<String> {
        h.to_ascii_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
    };
    let is_day = |w: &[String]| {
        w.iter().any(|x| DAYS.contains(&x.as_str()))
            || w.windows(2).any(|p| p.first().is_some_and(|a| a == "day") && p.get(1).is_some_and(|b| b.parse::<u8>().is_ok()))
    };
    let mut hours = Vec::new();
    for h in headers {
        let w = words(h);
        if w.iter().any(|x| NOT_HOURS.contains(&x.as_str())) {
            continue;
        }
        if w.iter().any(|x| x.starts_with("hour") || x == "hrs" || x == "hr") || is_day(&w) {
            hours.push((h.clone(), w));
        }
    }
    let names = |cols: &[(String, Vec<String>)]| cols.iter().map(|c| format!("\"{}\"", c.0)).collect::<Vec<_>>().join(", ");
    match hours.as_slice() {
        [] => return Err("no hours column (a column named like \"Hours\", \"Hrs\", \"Total Hours\" or \"ST Hours\"/\"OT Hours\")".into()),
        [one] => return Ok(vec![one.0.clone()]),
        _ => {}
    }
    let totals: Vec<&(String, Vec<String>)> = hours.iter().filter(|c| c.1.iter().any(|x| x == "total" || x == "tot")).collect();
    match totals.as_slice() {
        [one] => return Ok(vec![one.0.clone()]),
        [] => {}
        _ => return Err(format!("several total hours columns ({}): keep one, or send rows", names(&hours))),
    }
    let st_ot = hours.iter().filter(|c| c.1.iter().any(|x| ST_OT.contains(&x.as_str())) && !is_day(&c.1)).count();
    let days = hours.iter().filter(|c| is_day(&c.1) && !c.1.iter().any(|x| ST_OT.contains(&x.as_str()))).count();
    if st_ot == hours.len() || days == hours.len() {
        return Ok(hours.into_iter().map(|c| c.0).collect());
    }
    Err(format!(
        "can't tell which hours columns to add up ({}): add a \"Total Hours\" column, name them as straight-time/overtime (ST Hours, OT Hours) or per-day (Mon … Sun), or send rows",
        names(&hours)
    ))
}

fn run_topdown(_s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "calibrate.topdown";
    let values = p.get("values").and_then(Value::as_object).ok_or_else(|| bad(cmd, "values required: {trade: contract value}"))?;
    let empty = Map::new();
    let (ratios, census) = (p.get("ratios").and_then(Value::as_object), p.get("census").and_then(Value::as_object));
    if ratios.is_none() && census.is_none() {
        return Err(bad(
            cmd,
            "ratios or census required: {trade: {value_of_work, subcontracted_out, construction_worker_hours, construction_worker_wages}} (the census figures) or {trade: {self_performed_share, hours_per_dollar, wage_per_hour}}",
        ));
    }
    let (ratios, census) = (ratios.unwrap_or(&empty), census.unwrap_or(&empty));
    let pos = |v: &Value, k: &str| v.get(k).and_then(num);
    // Census year → bid date: wages by the wage index, materials by an assumed factor, weighted by
    // the wage share of the self-performed work.
    let wage_esc = p.get("wage_escalation").and_then(num).filter(|v| *v > 0.0).unwrap_or(1.0);
    let mat_esc = p.get("materials_escalation").and_then(num).filter(|v| *v > 0.0).unwrap_or(1.0);
    let gap_pct = p.get("gap_pct").and_then(num).unwrap_or(25.0);
    let bottom: BTreeMap<String, f64> = p
        .get("bottom_up_hours")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, v)| Some((norm(k), num(v)?))).collect())
        .unwrap_or_default();
    let mut out = Vec::new();
    let mut missing = Vec::new();
    for (t, v) in values.iter().take(200) {
        let t = norm(t);
        let value = num(v).ok_or_else(|| bad(cmd, format!("{t}: a value is 0 or more")))?;
        let Some(r) = [census, ratios].into_iter().find_map(|m| m.iter().find(|(k, _)| norm(k) == t).map(|x| x.1)) else {
            missing.push(t);
            continue;
        };
        let raw = CENSUS.iter().any(|k| r.get(*k).is_some());
        let (share, hpd, wage) = if raw {
            census_ratios(r).map_err(|e| bad(cmd, format!("{t}: {e}")))?
        } else {
            let (Some(share), Some(hpd), Some(wage)) =
                (pos(r, "self_performed_share").filter(|x| *x <= 1.0), pos(r, "hours_per_dollar"), pos(r, "wage_per_hour"))
            else {
                return Err(bad(
                    cmd,
                    format!(
                        "{t}: ratios need self_performed_share (0–1), hours_per_dollar and wage_per_hour, or the census figures {}",
                        CENSUS.join(", ")
                    ),
                ));
            };
            (share, hpd, wage)
        };
        let wage_share = if hpd * wage > 0.0 { (hpd * wage).min(1.0) } else { 0.0 };
        let esc = wage_share * wage_esc + (1.0 - wage_share) * mat_esc;
        let self_performed = value * share;
        let hours = self_performed / esc * hpd;
        let mut row = json!({
            "trade": t, "value": value, "self_performed": r2(self_performed), "escalation": sig(esc), "field_hours": r2(hours),
            "wages": r2(hours * wage * wage_esc), "private": is_electrical(&t, trade_info(&t).0),
            "ratios": {"self_performed_share": sig(share), "hours_per_dollar": sig(hpd), "wage_per_hour": sig(wage), "from": if raw { "census" } else { "given" }},
        });
        if let Some(b) = bottom.get(&t).filter(|b| **b > 0.0) {
            let gap = (hours - b) / b * 100.0;
            row["bottom_up_hours"] = json!(b);
            row["gap_pct"] = json!(r2(gap));
            row["flag"] = json!(gap.abs() > gap_pct);
        }
        out.push(row);
    }
    Ok(json!({"trades": out, "no_ratios": missing}))
}

/// A trade's raw census construction figures (in the census year's dollars).
const CENSUS: &[&str] = &["value_of_work", "subcontracted_out", "construction_worker_hours", "construction_worker_wages"];

/// The top-down ratios from a trade's census figures: self-performed share = (value of work −
/// subcontracted out) / value of work; field hours per self-performed dollar = construction
/// worker hours / self-performed value; wage per hour = construction worker wages / hours.
fn census_ratios(r: &Value) -> std::result::Result<(f64, f64, f64), String> {
    let get = |k: &str| r.get(k).and_then(|v| v.as_f64()).filter(|x| x.is_finite() && *x < 1e15);
    let need = "census figures need value_of_work > 0, subcontracted_out (0 or more, less than value_of_work), construction_worker_hours > 0 and construction_worker_wages > 0";
    let (Some(value), Some(sub), Some(hours), Some(wages)) =
        (get("value_of_work"), get("subcontracted_out"), get("construction_worker_hours"), get("construction_worker_wages"))
    else {
        return Err(need.into());
    };
    if value <= 0.0 || sub < 0.0 || sub >= value || hours <= 0.0 || wages <= 0.0 {
        return Err(need.into());
    }
    let self_performed = value - sub;
    Ok((self_performed / value, hours / self_performed, wages / hours))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("calibrate.project", "Add Calibration Project", run_project)
            .files()
            .params("{pack: .jsonl, id: anonymous (re-adding an id replaces it), building_type?, region?, bid_date?: YYYY-MM, delivery?, quantities: {trade: {driver: qty}} | from_wholejob: true, totals?: {trade: $}, total?: $, hours?: {trade: h}} → {pack, id, trades, replaced}")
            .noundo(),
        CommandSpec::new("calibrate.fit", "Fit Rates to Bids", run_fit)
            .files()
            .enabled(always)
            .params("{packs: [.jsonl], target?: cost|hours, building_type?, region?, priors?: {\"trade/driver\": {rate, sd}}, markups?: {trade: %}, regions?: {region: factor}, index?: {YYYY-MM: value}, base_date?, min_projects?: 3, holdout_every?: 5, out?: .jsonl, private?: bool} → {rates: [{trade, driver, rate, sd, projects, quality}], noise_pct, holdout: {projects, mape_pct}, written, warnings}")
            .noundo(),
        CommandSpec::new("calibrate.payroll", "Certified Payroll Hours", run_payroll)
            .files()
            .enabled(always)
            .params("{rows: [{classification, hours}] | path: CSV (a total hours column, else ST + OT or per-day columns), map?: {classification: trade}} → {hours: {trade: h}, unmapped, hours_columns}")
            .noundo(),
        CommandSpec::new("calibrate.topdown", "Top-Down Labor Estimate", run_topdown)
            .files()
            .enabled(always)
            .params("{values: {trade: $}, census: {trade: {value_of_work, subcontracted_out, construction_worker_hours, construction_worker_wages}} | ratios: {trade: {self_performed_share, hours_per_dollar, wage_per_hour}}, wage_escalation?, materials_escalation?, bottom_up_hours?: {trade: h}, gap_pct?: 25} → {trades: [{trade, self_performed, field_hours, wages, ratios, gap_pct, flag}], no_ratios}")
            .noundo(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_index_and_solver() {
        assert_eq!(month("2025"), Some(2025.0 * 12.0 + 6.0));
        assert_eq!(month("2025-03-14"), Some(2025.0 * 12.0 + 2.0));
        assert_eq!(month("2025-13"), None);
        assert_eq!(month("x"), None);
        let idx = [(month("2024-01").unwrap(), 100.0), (month("2025-01").unwrap(), 112.0)];
        assert_eq!(index_at(&idx, month("2024-07").unwrap()), Some(106.0));
        assert_eq!(index_at(&idx, 0.0), Some(100.0));
        assert_eq!(index_at(&[], 1.0), None);
        // [[4,2],[2,3]] x = [2,1] → x = [0.5, 0].
        let l = cholesky(&[4.0, 2.0, 2.0, 3.0], 2).unwrap();
        let x = solve(&l, 2, &[2.0, 1.0]).unwrap();
        assert!((x[0] - 0.5).abs() < 1e-12 && x[1].abs() < 1e-12);
        assert!(cholesky(&[0.0], 1).is_none());
        assert_eq!(sig(1234.567), 1235.0);
        assert_eq!(classify("Electrician - Journeyman"), Some("electrical"));
        assert_eq!(classify("Pipefitter"), Some("mechanical"));
        assert_eq!(classify("Clerk"), None);
        assert_eq!(classify("Cement Mason / Concrete Finisher"), Some("concrete"));
        assert_eq!(classify("Brick Mason"), Some("masonry"));
    }

    #[test]
    fn payroll_hours_columns() {
        let h = |v: &[&str]| hours_columns(&v.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        assert_eq!(h(&["Craft", "Hrs"]), Ok(vec!["Hrs".to_string()]));
        assert_eq!(h(&["OT Hours", "ST Hours", "Total Hours"]), Ok(vec!["Total Hours".to_string()]));
        assert_eq!(h(&["Hourly Rate", "OT Hours", "ST Hours"]), Ok(vec!["OT Hours".to_string(), "ST Hours".to_string()]));
        assert_eq!(h(&["Day 1", "Day 2"]), Ok(vec!["Day 1".to_string(), "Day 2".to_string()]));
        assert!(h(&["Classification", "Gross Pay"]).is_err());
        assert!(h(&["Total Hours", "Total OT Hours"]).is_err());
        assert!(h(&["Mon", "OT Hours"]).is_err(), "per-day beside overtime would count twice");
        assert!(h(&["Shop Hours", "Field Hours"]).is_err());
    }
}
