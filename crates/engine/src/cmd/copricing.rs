//! Change order pricing (Q336): when an RFI answer, an ASI, a bulletin or a revision changes the
//! model, the priced difference — ready to submit and logged against its number. As with the
//! estimate (Q327), the software does every count and every multiplication; the agent names the
//! change and decides scope.
//!
//! - `co.snapshot {label?}`: keep the priced estimate as it stands (quantities, unit prices,
//!   scope, where each line's elements are) before the model changes.
//! - `co.price {number, reference?, description?, revision?, scope?, markups?, markup_on?,
//!   clouds?, file?, accept?, replace?}`: price what changed since the snapshot — every line
//!   added, removed or changed in quantity, with material (on the quantity with waste), labor
//!   hours and dollars (on the net), and job equipment the BOM requires (`bim.bom` `required`,
//!   when it has a price). Each line is priced at today's unit prices; what didn't change in
//!   quantity is never repriced. Adds and credits are totalled apart and netted; markups
//!   (tax on material, then overhead, profit, bond — no contingency on a change) go on the net
//!   (`markup_on: net`, the default) or on the adds only, the credits returned at cost with
//!   their tax (`markup_on: adds`). The changed elements are clouded on the `CO-CLOUD` layer
//!   with the change's number, the affected sheets (levels, the RFI's sheets) listed, and a
//!   cover summary written. With `revision` the printed revision's own changes and clouds
//!   (`printrev.compare`) are the backup too. The change is logged, and (with `accept`, the
//!   default) the snapshot moves on, so the next change prices only itself.
//! - `co.log {number?}`: the change orders priced so far, or one in full.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::EntityKind;
use cadaddy_geom::{Bounds2, Vec2};
use serde_json::{Map, Value, json};

use super::estimate::{obj, price_all, setting, state, store};
use super::*;
use crate::{Result, Session};

/// The layer the change clouds go on.
pub const CLOUD_LAYER: &str = "CO-CLOUD";
/// The change clouds' xdata application name.
const APP: &str = "CADADDY_COPRICING";
/// Change orders kept in the log, at most.
const MAX_LOG: usize = 10_000;
/// The longest number, reference and description kept.
const MAX_NUMBER: usize = 64;
const MAX_TEXT: usize = 4_000;
/// Elements located per line, at most.
const MAX_ELEMENTS: usize = 500;
const SCOPES: [&str; 4] = ["in_contract", "by_others", "nic", "ofci"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("co.snapshot", "Change Order Snapshot", run_snapshot)
            .params("{label?: text (default Contract, or the last change order's number)} → {label, lines, in_contract, unpriced}: the priced estimate as it stands, kept to price the next change against"),
        CommandSpec::new("co.price", "Price Change Order", run_price).params(
            "{number: CO number, reference?: RFI/ASI/bulletin number, description?, revision?: printrev label (its changes and clouds are backup), scope?: in_contract|by_others|nic|ofci|alternate:<n> (for lines the change adds that have none), markups?: {tax_pct?, overhead_pct?, profit_pct?, bond_pct?} (default the estimate's), markup_on?: net|adds (default net), clouds?: bool (default true), file?: path (.csv|.md), accept?: bool (default true: the snapshot moves on), replace?: bool (price the latest change order again)} → {number, reference, lines: [{key, change: added|removed|changed, item, size, unit, qty_was, qty_now, qty_delta, material, labor_hours, labor, equipment, cost, math}], adds, credits, net, totals: {material, labor_hours, labor, equipment, direct, tax, overhead, profit, bond, total}, sheets, clouds, backup, findings, ok, cover, csv, path?}",
        ),
        CommandSpec::new("co.log", "Change Order Log", run_log)
            .params("{number?} → {change_orders: [{number, reference, description, date, total, adds, credits, lines, sheets}], total} | {change_order}")
            .noundo(),
    ]
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn f(v: &Value, k: &str) -> f64 {
    v[k].as_f64().filter(|x| x.is_finite()).unwrap_or(0.0)
}

fn valid_scope(scope: &str) -> bool {
    SCOPES.contains(&scope) || scope.strip_prefix("alternate:").is_some_and(|n| !n.is_empty() && n.len() <= 16)
}

/// The change-order state inside the estimate's (`baseline`, `before_last`, `log`).
fn co_state(st: &mut Map<String, Value>) -> Option<&mut Map<String, Value>> {
    obj(st, "co")
}

// ---------------- the snapshot ----------------

/// Where elements are: (element, level, box in plan drawing units).
type Where = Vec<(String, String, [f64; 4])>;

fn locate(sp: &cadaddy_walk3d::BuildingSpec, ids: &[Value]) -> Where {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut out = Vec::new();
    for id in ids.iter().filter_map(Value::as_str).take(MAX_ELEMENTS) {
        if let Some(i) = id.strip_prefix("run:").and_then(|n| n.parse::<usize>().ok()) {
            let Some(r) = sp.mep.runs.get(i) else { continue };
            let b = Bounds2::from_points(r.points.iter().map(|p| Vec2::new(p[0], p[1])));
            if !b.is_empty() {
                out.push((id.to_string(), r.level.clone(), [b.min.x, b.min.y, b.max.x, b.max.y]));
            }
        } else if let Some(t) = sp.tags.iter().find(|t| t.id == id) {
            let (x, y) = (t.at_ft[0] * upf, t.at_ft[1] * upf);
            if x.is_finite() && y.is_finite() {
                out.push((id.to_string(), t.level.clone(), [x, y, x, y]));
            }
        }
    }
    out
}

fn where_json(w: &Where) -> Value {
    json!(w.iter().map(|(id, level, b)| json!({ "id": id, "level": level, "box": b })).collect::<Vec<_>>())
}

fn where_of(v: &Value) -> Where {
    v["where"]
        .as_array()
        .into_iter()
        .flatten()
        .take(MAX_ELEMENTS)
        .filter_map(|w| {
            let b = w["box"].as_array()?;
            let n = |i: usize| b.get(i).and_then(Value::as_f64).filter(|x| x.is_finite());
            Some((w["id"].as_str()?.to_string(), w["level"].as_str().unwrap_or_default().to_string(), [n(0)?, n(1)?, n(2)?, n(3)?]))
        })
        .collect()
}

/// The priced estimate as it stands: every BOM line, and the job equipment the BOM requires.
fn take(s: &mut Session, st: &Map<String, Value>) -> Result<Vec<Value>> {
    let (sp, _) = current_spec(s)?;
    let (priced, _) = price_all(s, st)?;
    let mut out: Vec<Value> = priced
        .iter()
        .map(|p| {
            let ids = p.line["elements"].as_array().cloned().unwrap_or_default();
            json!({
                "key": p.key, "kind": "line", "trade": p.line["trade"], "category": p.line["category"], "item": p.line["item"],
                "size": p.line["size"], "unit": p.unit, "qty": p.qty, "net": p.net,
                "material_unit": p.material_unit, "labor_unit_hours": p.labor_unit_hours, "scope": p.scope, "via": p.via,
                "where": where_json(&locate(&sp, &ids)),
            })
        })
        .collect();
    // Job equipment (rentals, purchases, owned tools) at its price, when it has one.
    let bom = s.execute("bim.bom", &json!({}))?;
    let prices: Vec<Value> = st.get("baseline").and_then(|b| b["prices"].as_array()).cloned().unwrap_or_default();
    for r in bom["required"].as_array().into_iter().flatten() {
        let item = r["item"].as_str().unwrap_or_default();
        let qty = f(r, "qty");
        if item.is_empty() || qty <= 0.0 {
            continue;
        }
        let price = prices.iter().find(|p| p["item"].as_str().is_some_and(|i| i.eq_ignore_ascii_case(item)));
        let supply = r["supply"].as_str().unwrap_or_default();
        out.push(json!({
            "key": format!("equipment|{supply}|{item}|"), "kind": "equipment", "trade": "", "category": "equipment", "item": item, "size": "",
            "unit": "each", "qty": qty, "net": qty, "material_unit": price.and_then(|p| p["material"].as_f64()), "labor_unit_hours": null,
            "supply": supply, "scope": "in_contract", "via": price.map(|p| format!("price {}", p["source"].as_str().unwrap_or_default())), "where": [],
        }));
    }
    Ok(out)
}

fn current_spec(s: &Session) -> Result<(cadaddy_walk3d::BuildingSpec, bool)> {
    super::bspec::current(s, "co.price")
}

fn run_snapshot(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, mut st) = state(s, "co.snapshot")?;
    let lines = take(s, &st)?;
    let label = str_param(p, "label").map(str::trim).filter(|l| !l.is_empty()).unwrap_or("Contract").chars().take(MAX_NUMBER).collect::<String>();
    let n = |pred: &dyn Fn(&Value) -> bool| lines.iter().filter(|l| pred(l)).count();
    let out = json!({
        "label": label, "lines": lines.len(),
        "in_contract": n(&|l| l["scope"].as_str() == Some("in_contract")),
        "unpriced": n(&|l| l["material_unit"].is_null() && l["kind"] == json!("line")),
    });
    if let Some(co) = co_state(&mut st) {
        co.insert("baseline".into(), json!({ "label": label, "lines": lines }));
        co.remove("before_last");
    }
    store(s, sp, st)?;
    Ok(out)
}

// ---------------- the delta ----------------

/// One line of a change.
struct Delta {
    key: String,
    change: &'static str,
    line: Value,
    qty_was: f64,
    qty_now: f64,
    material: f64,
    labor_hours: f64,
    labor: f64,
    equipment: f64,
    math: Vec<String>,
    places: Where,
}

impl Delta {
    fn cost(&self) -> f64 {
        self.material + self.labor + self.equipment
    }
    fn to_json(&self) -> Value {
        json!({
            "key": self.key, "change": self.change, "trade": self.line["trade"], "category": self.line["category"], "item": self.line["item"],
            "size": self.line["size"], "unit": self.line["unit"], "qty_was": r2(self.qty_was), "qty_now": r2(self.qty_now),
            "qty_delta": r2(self.qty_now - self.qty_was), "material": r2(self.material), "labor_hours": r2(self.labor_hours),
            "labor": r2(self.labor), "equipment": r2(self.equipment), "cost": r2(self.cost()), "math": self.math,
            "elements": self.places.iter().map(|w| w.0.clone()).collect::<BTreeSet<_>>(),
        })
    }
}

fn in_contract(l: Option<&Value>) -> bool {
    l.is_some_and(|l| l["scope"].as_str() == Some("in_contract"))
}

/// Every line whose in-contract quantity changed, priced at today's unit prices, and what kept
/// a line from being priced.
fn deltas(was: &[Value], now: &[Value], crew: f64) -> (Vec<Delta>, Vec<Value>) {
    let index = |v: &[Value]| -> BTreeMap<String, Value> { v.iter().filter_map(|l| Some((l["key"].as_str()?.to_string(), l.clone()))).collect() };
    let (w, n) = (index(was), index(now));
    let keys: BTreeSet<&String> = w.keys().chain(n.keys()).collect();
    let (mut out, mut findings) = (Vec::new(), Vec::new());
    for k in keys {
        let (lw, ln) = (w.get(k), n.get(k));
        let Some(line) = ln.or(lw) else { continue };
        let (qw, nw) = if in_contract(lw) { lw.map_or((0.0, 0.0), |l| (f(l, "qty"), f(l, "net"))) } else { (0.0, 0.0) };
        let (qn, nn) = if in_contract(ln) { ln.map_or((0.0, 0.0), |l| (f(l, "qty"), f(l, "net"))) } else { (0.0, 0.0) };
        // A line the change brought in with no scope yet: never counted, always reported.
        if let Some(l) = ln.filter(|l| l["scope"].is_null())
            && (f(l, "qty") - lw.filter(|x| x["scope"].is_null()).map_or(0.0, |x| f(x, "qty"))).abs() > 1e-9
        {
            findings.push(json!({ "kind": "undecided_scope", "line": k, "message": "the change adds a line with no scope", "fix": "set its scope (estimate.set_scope, or co.price `scope`) and price again with `replace`" }));
        }
        if (qn - qw).abs() < 1e-9 && (nn - nw).abs() < 1e-9 {
            continue;
        }
        let unit_price = |key: &str| ln.and_then(|l| l[key].as_f64()).or_else(|| lw.and_then(|l| l[key].as_f64())).filter(|v| v.is_finite());
        let (mu, lu) = (unit_price("material_unit"), unit_price("labor_unit_hours"));
        let unit = line["unit"].as_str().unwrap_or_default();
        let equipment = line["kind"].as_str() == Some("equipment");
        if mu.is_none() {
            findings.push(json!({ "kind": if equipment { "unpriced_equipment" } else { "unpriced" }, "line": k, "message": "a changed line has no price", "fix": "add a price (estimate.set_price) or map an assembly (estimate.set_assembly), then price again with `replace`" }));
        }
        if mu.is_some() && lu.is_none() && !equipment {
            findings.push(json!({ "kind": "missing_labor", "line": k, "message": "a changed line has no labor per unit", "fix": "add labor hours or dollars to its price or assembly" }));
        }
        let (dq, dn) = (qn - qw, nn - nw);
        let money = dq * mu.unwrap_or(0.0);
        let hours = if equipment { 0.0 } else { dn * lu.unwrap_or(0.0) };
        let mut math =
            vec![format!("{} {unit}: {} → {} = {:+} {unit}", if equipment { "equipment" } else { "quantity with waste" }, r2(qw), r2(qn), r2(dq))];
        math.push(format!("{}: {:+} × ${:.2} = ${:.2}", if equipment { "equipment" } else { "material" }, r2(dq), mu.unwrap_or(0.0), r2(money)));
        if !equipment {
            math.push(format!(
                "labor: {:+} {unit} (net) × {:.3} h = {:.2} h × ${crew:.2}/h = ${:.2}",
                r2(dn),
                lu.unwrap_or(0.0),
                r2(hours),
                r2(hours * crew)
            ));
        }
        // What to cloud: the elements that came or went; if the same ones changed, all of them.
        let (pw, pn) = (lw.map(where_of).unwrap_or_default(), ln.map(where_of).unwrap_or_default());
        let ids_w: BTreeSet<&String> = pw.iter().map(|x| &x.0).collect();
        let ids_n: BTreeSet<&String> = pn.iter().map(|x| &x.0).collect();
        let mut places: Where = pn.iter().filter(|x| !ids_w.contains(&x.0)).chain(pw.iter().filter(|x| !ids_n.contains(&x.0))).cloned().collect();
        if places.is_empty() {
            places = if pn.is_empty() { pw } else { pn };
        }
        out.push(Delta {
            key: k.clone(),
            change: if qw <= 0.0 && nw <= 0.0 {
                "added"
            } else if qn <= 0.0 && nn <= 0.0 {
                "removed"
            } else {
                "changed"
            },
            line: line.clone(),
            qty_was: qw,
            qty_now: qn,
            material: if equipment { 0.0 } else { money },
            labor_hours: hours,
            labor: hours * crew,
            equipment: if equipment { money } else { 0.0 },
            math,
            places,
        });
    }
    (out, findings)
}

/// Markup percentages (each 0–100): the call's, else the estimate's.
struct Markups {
    tax: f64,
    overhead: f64,
    profit: f64,
    bond: f64,
}

/// Direct cost to a total: tax on material, then overhead, profit and bond, each on what's
/// above it. `marked` false: tax only (credits returned at cost).
fn marked_up(material: f64, direct: f64, m: &Markups, marked: bool) -> Value {
    let tax = material * m.tax / 100.0;
    let sub = direct + tax;
    let k = if marked { 1.0 } else { 0.0 };
    let overhead = sub * m.overhead / 100.0 * k;
    let profit = (sub + overhead) * m.profit / 100.0 * k;
    let bond = (sub + overhead + profit) * m.bond / 100.0 * k;
    json!({ "direct": r2(direct), "tax": r2(tax), "overhead": r2(overhead), "profit": r2(profit), "bond": r2(bond), "total": r2(sub + overhead + profit + bond) })
}

fn sums(ds: &[&Delta]) -> (f64, f64, f64, f64) {
    ds.iter().fold((0.0, 0.0, 0.0, 0.0), |a, d| (a.0 + d.material, a.1 + d.labor_hours, a.2 + d.labor, a.3 + d.equipment))
}

fn group_json(ds: &[&Delta], m: &Markups, marked: bool) -> Value {
    let (mat, hours, labor, equip) = sums(ds);
    let mut v = marked_up(mat, mat + labor + equip, m, marked);
    v["material"] = json!(r2(mat));
    v["labor_hours"] = json!(r2(hours));
    v["labor"] = json!(r2(labor));
    v["equipment"] = json!(r2(equip));
    v["lines"] = json!(ds.len());
    v
}

fn run_price(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "co.price";
    let number =
        str_param(p, "number").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "`number` (the change order's) is required"))?;
    if number.chars().count() > MAX_NUMBER {
        return Err(bad(C, format!("`number` is {MAX_NUMBER} characters at most")));
    }
    let number = number.to_string();
    let text = |k: &str| str_param(p, k).map(str::trim).filter(|x| !x.is_empty()).map(|x| x.chars().take(MAX_TEXT).collect::<String>());
    let (reference, description, revision) = (text("reference"), text("description"), text("revision"));
    let markup_on = str_param(p, "markup_on").map_or_else(|| "net".to_string(), |m| m.trim().to_ascii_lowercase());
    if !matches!(markup_on.as_str(), "net" | "adds") {
        return Err(bad(C, "`markup_on` is net or adds"));
    }
    let new_scope = str_param(p, "scope").map(|x| x.trim().to_ascii_lowercase());
    if new_scope.as_deref().is_some_and(|x| !valid_scope(x)) {
        return Err(bad(C, "`scope` is in_contract, by_others, nic, ofci or alternate:<n>"));
    }
    let replace = bool_or(p, "replace", false);
    let (sp, mut st) = state(s, C)?;
    let co = st.get("co").cloned().unwrap_or(json!({}));
    let log: Vec<Value> = co["log"].as_array().cloned().unwrap_or_default();
    let exists = log.iter().any(|e| e["number"].as_str() == Some(number.as_str()));
    // Priced again: only the latest one, against the snapshot it was priced from (the one before
    // it when it moved the snapshot on).
    let reopened = replace && exists && co["before_last"]["number"].as_str() == Some(number.as_str());
    if replace && exists && log.last().and_then(|e| e["number"].as_str()) != Some(number.as_str()) {
        return Err(bad(C, format!("only the latest change order can be priced again; {number} is not it")));
    }
    if exists && !replace {
        return Err(bad(C, format!("{number} is already priced (give `replace: true` to price it again)")));
    }
    let from = if reopened { &co["before_last"]["baseline"] } else { &co["baseline"] };
    let was: Vec<Value> = from["lines"]
        .as_array()
        .cloned()
        .ok_or_else(|| bad(C, "no snapshot to price against: take one with co.snapshot before the model changes"))?;
    let from_label = from["label"].clone();
    // Lines the change brings in with no scope: the call's scope, recorded with its reason.
    if let Some(scope) = &new_scope {
        let known: BTreeSet<&str> = was.iter().filter_map(|l| l["key"].as_str()).collect();
        let (priced, _) = price_all(s, &st)?;
        let fresh: Vec<String> = priced.iter().filter(|x| x.scope.is_none() && !known.contains(x.key.as_str())).map(|x| x.key.clone()).collect();
        if let Some(m) = obj(&mut st, "scope") {
            for k in fresh {
                m.insert(k, json!({ "scope": scope, "reason": format!("added by {number}"), "citation": reference }));
            }
        }
    }
    let now = take(s, &st)?;
    let crew = setting(&st, "crew_rate", 0.0);
    let (ds, mut findings) = deltas(&was, &now, crew);
    if crew <= 0.0 && ds.iter().any(|d| d.labor_hours.abs() > 0.0) {
        findings.push(
            json!({ "kind": "no_crew_rate", "message": "labor hours changed but there is no crew rate", "fix": "estimate.settings {crew_rate}" }),
        );
    }
    let pm = |k: &str| p["markups"][k].as_f64().filter(|v| v.is_finite() && (0.0..=1e6).contains(v)).unwrap_or_else(|| setting(&st, k, 0.0));
    let m = Markups { tax: pm("tax_pct"), overhead: pm("overhead_pct"), profit: pm("profit_pct"), bond: pm("bond_pct") };
    let adds: Vec<&Delta> = ds.iter().filter(|d| d.cost() > 0.0).collect();
    let credits: Vec<&Delta> = ds.iter().filter(|d| d.cost() <= 0.0).collect();
    let all: Vec<&Delta> = ds.iter().collect();
    let (adds_j, credits_j) = (group_json(&adds, &m, true), group_json(&credits, &m, markup_on == "net"));
    let mut totals = group_json(&all, &m, true);
    if markup_on == "adds" {
        // Adds marked up, credits at cost with their tax: the parts summed.
        for k in ["tax", "overhead", "profit", "bond", "total"] {
            totals[k] = json!(r2(f(&adds_j, k) + f(&credits_j, k)));
        }
    }
    // The sheets: the levels the changed elements are on, and the RFI's sheets.
    let rfi = reference.as_deref().and_then(|r| sp.rfis.iter().find(|x| x.id.eq_ignore_ascii_case(r)));
    let mut sheets: BTreeSet<String> = ds.iter().flat_map(|d| d.places.iter().map(|w| w.1.clone())).filter(|l| !l.is_empty()).collect();
    sheets.extend(rfi.map(|r| r.sheets.clone()).unwrap_or_default());
    // The printed revision as backup: its run changes and its clouds.
    let rev_changes: Vec<Value> = revision
        .as_deref()
        .map(|r| {
            sp.mep
                .revisions
                .iter()
                .filter(|x| x.revision.eq_ignore_ascii_case(r))
                .take(2_000)
                .map(|x| json!({ "item": x.item, "system": x.system, "level": x.level, "change": x.change, "from": x.from }))
                .collect()
        })
        .unwrap_or_default();
    let d = s.doc()?;
    let rev_clouds = revision.as_deref().map_or(0, |r| {
        d.model
            .iter()
            .filter(|e| {
                matches!(e.kind, EntityKind::LwPolyline(_))
                    && super::printrev::label_of(e, super::printrev::APP).is_some_and(|l| l.eq_ignore_ascii_case(r))
            })
            .count()
    });
    if revision.is_some() && rev_changes.is_empty() && rev_clouds == 0 {
        findings.push(json!({ "kind": "revision_not_found", "message": format!("no printed revision {} (printrev.compare)", revision.as_deref().unwrap_or_default()), "fix": "give the label the revision was compared with" }));
    }
    let lines: Vec<Value> = ds.iter().map(Delta::to_json).collect();
    let date = st.get("settings").and_then(|x| x["today"].as_str()).unwrap_or_default().to_string();
    let cover = cover_text(
        &number,
        reference.as_deref(),
        description.as_deref(),
        &date,
        &markup_on,
        &[&adds_j, &credits_j, &totals],
        &sheets,
        &ds,
        rfi,
        &findings,
    );
    let csv = csv_text(&number, reference.as_deref(), &lines, &totals);
    let mut out = json!({
        "number": number, "reference": reference, "description": description, "date": date, "from": from_label, "markup_on": markup_on,
        "lines": lines, "adds": adds_j, "credits": credits_j, "net": r2(f(&totals, "direct")), "totals": totals,
        "sheets": sheets, "backup": { "revision": revision, "revision_changes": rev_changes, "revision_clouds": rev_clouds },
        "ok": findings.is_empty(), "findings": findings, "cover": cover, "csv": csv,
    });
    if let Some(r) = rfi {
        out["rfi"] = json!({ "id": r.id, "about": r.about, "issue": r.issue, "answer": r.answer, "status": r.status.name(), "sheets": r.sheets });
    }
    if let Some(path) = str_param(p, "file").map(str::trim).filter(|x| !x.is_empty()) {
        let lower = path.to_ascii_lowercase();
        let bytes = if lower.ends_with(".csv") {
            csv.into_bytes()
        } else if lower.ends_with(".md") || lower.ends_with(".txt") {
            format!("{}\n\n{}\n", out["cover"].as_str().unwrap_or_default(), md_table(&out["lines"])).into_bytes()
        } else {
            return Err(bad(C, "`file` is a .csv or .md path"));
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::writefile::write(path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
            out["path"] = json!(path);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = bytes;
    }
    // Clouds round the changed elements, per level; again: the earlier clouds go.
    let mut clouds = Vec::new();
    {
        let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
        let d = s.doc_mut()?;
        if exists {
            let old: Vec<cadaddy_doc::Handle> =
                d.model.iter().filter(|e| super::printrev::label_of(e, APP) == Some(number.as_str())).map(|e| e.handle).collect();
            for h in old {
                d.model.remove(h);
            }
        }
        if bool_or(p, "clouds", true) && !ds.is_empty() {
            super::printrev::ensure_layer(d, CLOUD_LAYER);
            let mut by_level: BTreeMap<&str, Vec<(Bounds2, usize)>> = BTreeMap::new();
            for (_, level, b) in ds.iter().flat_map(|x| x.places.iter()) {
                by_level.entry(level).or_default().push((Bounds2::new(Vec2::new(b[0], b[1]), Vec2::new(b[2], b[3])), 1));
            }
            let common = |label: &str| super::printrev::tagged_as(CLOUD_LAYER, APP, label);
            for (_, boxes) in by_level {
                for (b, _) in super::printrev::regions(boxes, 10.0 * upf) {
                    if let Some(h) = super::printrev::cloud_with(d, &b, 2.0 * upf, &number, &common) {
                        clouds.push(h.hex());
                    }
                }
            }
        }
    }
    out["clouds"] = json!(clouds);
    // The log, and the snapshot moved on.
    let entry = json!({
        "number": number, "reference": out["reference"], "description": out["description"], "date": date, "from": from_label,
        "markup_on": markup_on, "total": out["totals"]["total"], "adds": out["adds"]["total"], "credits": out["credits"]["total"],
        "totals": out["totals"], "lines": out["lines"], "sheets": out["sheets"], "clouds": clouds.len(), "findings": out["findings"],
        "revision": revision, "accepted": bool_or(p, "accept", true),
    });
    if let Some(co) = co_state(&mut st) {
        let mut log = log;
        log.retain(|e| e["number"].as_str() != Some(number.as_str()));
        if log.len() >= MAX_LOG {
            return Err(bad(C, format!("the change order log is full ({MAX_LOG})")));
        }
        log.push(entry);
        co.insert("log".into(), Value::Array(log));
        match (bool_or(p, "accept", true), reopened) {
            (true, false) => {
                let base = co.get("baseline").cloned().unwrap_or(Value::Null);
                co.insert("before_last".into(), json!({ "number": number, "baseline": base }));
                co.insert("baseline".into(), json!({ "label": number, "lines": now }));
            }
            (true, true) => {
                co.insert("baseline".into(), json!({ "label": number, "lines": now }));
            }
            // No longer accepted: the snapshot goes back to the one before it.
            (false, true) => {
                let base = co.remove("before_last").map(|b| b["baseline"].clone()).unwrap_or(Value::Null);
                co.insert("baseline".into(), base);
            }
            (false, false) => {}
        }
    }
    store(s, sp, st)?;
    Ok(out)
}

fn money(v: f64) -> String {
    let (sign, v) = if v < 0.0 { ("-", -v) } else { ("", v) };
    let cents = (v * 100.0).round();
    let whole = (cents / 100.0).trunc();
    let mut digits = format!("{whole:.0}");
    let mut grouped = String::new();
    while digits.len() > 3 {
        let tail = digits.split_off(digits.len() - 3);
        grouped = format!(",{tail}{grouped}");
    }
    format!("{sign}${digits}{grouped}.{:02}", (cents - whole * 100.0).max(0.0) as u32)
}

#[allow(clippy::too_many_arguments)]
fn cover_text(
    number: &str,
    reference: Option<&str>,
    description: Option<&str>,
    date: &str,
    markup_on: &str,
    groups: &[&Value; 3],
    sheets: &BTreeSet<String>,
    ds: &[Delta],
    rfi: Option<&cadaddy_walk3d::Rfi>,
    findings: &[Value],
) -> String {
    let [adds, credits, t] = *groups;
    let mut c = vec![format!("CHANGE ORDER {number}{}", reference.map(|r| format!(" — re {r}")).unwrap_or_default())];
    if !date.is_empty() {
        c.push(format!("Date: {date}"));
    }
    if let Some(d) = description {
        c.push(format!("Description: {d}"));
    }
    if let Some(r) = rfi {
        c.push(format!("{}: {} — answer: {}", r.id, r.issue, r.answer.as_deref().unwrap_or("(none recorded)")));
    }
    let n = |k: &str| ds.iter().filter(|d| d.change == k).count();
    c.push(format!("Lines: {} added, {} removed, {} changed", n("added"), n("removed"), n("changed")));
    c.push(format!(
        "Adds: {} ({} lines)    Credits: {} ({} lines)",
        money(f(adds, "total")),
        adds["lines"],
        money(f(credits, "total")),
        credits["lines"]
    ));
    c.push(format!(
        "Material {}   Labor {} ({} h)   Equipment {}   Direct {}",
        money(f(t, "material")),
        money(f(t, "labor")),
        f(t, "labor_hours"),
        money(f(t, "equipment")),
        money(f(t, "direct"))
    ));
    c.push(format!(
        "Tax {}   Overhead {}   Profit {}   Bond {}   (markups on the {})",
        money(f(t, "tax")),
        money(f(t, "overhead")),
        money(f(t, "profit")),
        money(f(t, "bond")),
        if markup_on == "adds" { "adds; credits at cost" } else { "net" }
    ));
    c.push(format!("TOTAL THIS CHANGE: {}", money(f(t, "total"))));
    if !sheets.is_empty() {
        c.push(format!("Affected sheets: {}", sheets.iter().cloned().collect::<Vec<_>>().join(", ")));
    }
    if !findings.is_empty() {
        c.push(format!("NOT READY: {} finding(s) — see findings", findings.len()));
    }
    c.join("\n")
}

fn csv_cell(c: &str) -> String {
    if c.contains([',', '"', '\n']) { format!("\"{}\"", c.replace('"', "\"\"")) } else { c.to_string() }
}

fn cell(v: &Value) -> String {
    match v {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        v => v.to_string(),
    }
}

const COLS: [&str; 14] = [
    "change",
    "trade",
    "category",
    "item",
    "size",
    "unit",
    "qty_was",
    "qty_now",
    "qty_delta",
    "material",
    "labor_hours",
    "labor",
    "equipment",
    "cost",
];

fn csv_text(number: &str, reference: Option<&str>, lines: &[Value], totals: &Value) -> String {
    let mut rows = vec![
        std::iter::once("co".to_string()).chain(std::iter::once("reference".into())).chain(COLS.iter().map(|c| (*c).to_string())).collect::<Vec<_>>(),
    ];
    for l in lines {
        rows.push(
            std::iter::once(number.to_string())
                .chain(std::iter::once(reference.unwrap_or_default().to_string()))
                .chain(COLS.iter().map(|c| cell(&l[*c])))
                .collect(),
        );
    }
    for k in ["direct", "tax", "overhead", "profit", "bond", "total"] {
        let mut r = vec![number.to_string(), reference.unwrap_or_default().to_string(), k.to_string()];
        r.extend(std::iter::repeat_n(String::new(), COLS.len() - 2));
        r.push(cell(&totals[k]));
        rows.push(r);
    }
    rows.iter().map(|r| r.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join("\n")
}

fn md_table(lines: &Value) -> String {
    let mut t = vec![format!("| {} |", COLS.join(" | ")), format!("|{}", "---|".repeat(COLS.len()))];
    for l in lines.as_array().into_iter().flatten() {
        t.push(format!("| {} |", COLS.iter().map(|c| cell(&l[*c]).replace('|', "/")).collect::<Vec<_>>().join(" | ")));
    }
    t.join("\n")
}

fn run_log(s: &mut Session, p: &Value) -> Result<Value> {
    let (_, st) = state(s, "co.log")?;
    let log: Vec<Value> = st.get("co").and_then(|c| c["log"].as_array()).cloned().unwrap_or_default();
    if let Some(n) = str_param(p, "number") {
        let e = log.iter().find(|e| e["number"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(n.trim())));
        return e.map(|e| json!({ "change_order": e })).ok_or_else(|| bad("co.log", format!("no change order {n}")));
    }
    let total: f64 = log.iter().map(|e| f(e, "total")).sum();
    let rows: Vec<Value> = log
        .iter()
        .map(|e| {
            json!({ "number": e["number"], "reference": e["reference"], "description": e["description"], "date": e["date"], "total": e["total"], "adds": e["adds"], "credits": e["credits"], "lines": e["lines"].as_array().map_or(0, Vec::len), "sheets": e["sheets"] })
        })
        .collect();
    Ok(json!({ "change_orders": rows, "total": r2(total) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An invented baseline pricing every line of the small job below.
    fn baseline(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cadaddy-copricing-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = |n: &str, t: &str| std::fs::write(dir.join(n), t).unwrap();
        w("assemblies.csv", "assembly,unit,description,labor_hours\nDUPLEX-MS,each,Duplex on metal stud,0.6\nGFCI-MS,each,GFCI on metal stud,0.7\n");
        w(
            "assembly_parts.csv",
            "assembly,item,qty\nDUPLEX-MS,Duplex device,1\nDUPLEX-MS,Box 4in sq,1\nGFCI-MS,GFCI device,1\nGFCI-MS,Box 4in sq,1\n",
        );
        w(
            "prices.csv",
            "item,size,unit,material,labor_hours,date,source\n\
             Duplex device,,each,2.00,,2026-09-01,Invented Supply\n\
             GFCI device,,each,15.00,,2026-09-01,Invented Supply\n\
             Box 4in sq,,each,3.00,,2026-09-01,Invented Supply\n\
             \"Box, 4\"\" square\",,each,2.00,0.2,2026-09-01,Invented Supply\n\
             Plaster ring,,each,1.00,0.05,2026-09-01,Invented Supply\n\
             Device plate,,each,0.50,0.05,2026-09-01,Invented Supply\n\
             EMT conduit,,ft,1.10,0.05,2026-09-01,Invented Supply\n\
             EMT coupling,,each,0.40,0.02,2026-09-01,Invented Supply\n\
             EMT connector,,each,0.60,0.05,2026-09-01,Invented Supply\n\
             Conduit strap,,each,0.20,0.03,2026-09-01,Invented Supply\n\
             EMT 90° elbow,,each,1.50,0.1,2026-09-01,Invented Supply\n\
             Carbide hole saw for 1\" conduit,,each,40.00,,2026-09-01,Invented Supply\n",
        );
        w("rates.json", "{\"crew_rate\": 85.0}");
        dir
    }

    fn device(id: &str, x: f64, ty: &str) -> Value {
        json!({"id": id, "kind": "receptacle", "level": "1", "at_ft": [x, 1], "fields": {"device_type": {"value": ty, "source": "plan"}}})
    }

    fn set_tags(s: &mut Session, tags: Value) {
        s.execute("bspec.set", &json!({ "spec": { "tags": tags } })).unwrap();
    }

    /// Two duplexes and a GFCI on level 1 and a 100' conduit run, all in contract and priced.
    fn job(tag: &str) -> (Session, std::path::PathBuf) {
        let mut s = Session::new();
        let spec = json!({
            "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}],
            "mep": {
                "systems": [{"name": "E-BRANCH-COND", "layer": "E-BRANCH-COND", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9}],
                "runs": [{"system": "E-BRANCH-COND", "level": "1", "points": [[0, 0], [1200, 0]], "size_in": [0.75, 0], "bottoms_ft": [9]}]
            },
            "tags": [device("R1", 3.0, "standard duplex receptacle"), device("R2", 4.0, "standard duplex receptacle"), device("R3", 5.0, "GFCI receptacle")]
        });
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
        let dir = baseline(tag);
        s.execute("estimate.import", &json!({ "folder": dir.to_string_lossy() })).unwrap();
        s.execute("estimate.settings", &json!({ "tax_pct": 8, "overhead_pct": 10, "profit_pct": 10, "today": "2026-10-21" })).unwrap();
        s.execute("estimate.set_assembly", &json!({ "item": "standard duplex receptacle", "assembly": "DUPLEX-MS", "reason": "metal stud walls" }))
            .unwrap();
        s.execute("estimate.set_assembly", &json!({ "item": "GFCI receptacle", "assembly": "GFCI-MS", "reason": "metal stud walls" })).unwrap();
        s.execute("estimate.set_scope", &json!({ "trade": "electrical", "scope": "in_contract", "reason": "Division 26 is ours" })).unwrap();
        (s, dir)
    }

    fn line<'a>(r: &'a Value, item: &str) -> &'a Value {
        r["lines"].as_array().unwrap().iter().find(|l| l["item"] == json!(item)).unwrap_or_else(|| panic!("no {item} in {r}"))
    }

    #[test]
    fn a_swap_prices_adds_and_credits_and_nets_them() {
        let (mut s, dir) = job("swap");
        let snap = s.execute("co.snapshot", &json!({})).unwrap();
        assert_eq!(snap["label"], json!("Contract"));
        // ASI-02: the duplex at R2 becomes a GFCI.
        set_tags(
            &mut s,
            json!([device("R1", 3.0, "standard duplex receptacle"), device("R2", 4.0, "GFCI receptacle"), device("R3", 5.0, "GFCI receptacle")]),
        );
        let co = s.execute("co.price", &json!({ "number": "CO-001", "reference": "ASI-02", "description": "GFCI at the counter" })).unwrap();
        assert_eq!(co["lines"].as_array().unwrap().len(), 2, "boxes don't change: {co}");
        // Duplex 2 → 1: −1 × $5 material, −0.6 h × $85 = −$51 → −$56 (a credit).
        let dup = line(&co, "standard duplex receptacle");
        assert_eq!((dup["change"].clone(), dup["qty_delta"].clone(), dup["cost"].clone()), (json!("changed"), json!(-1.0), json!(-56.0)), "{dup}");
        // GFCI 1 → 2: +$18 material, +0.7 h × $85 = $59.50 → $77.50 (an add).
        assert_eq!(line(&co, "GFCI receptacle")["cost"], json!(77.5));
        assert_eq!((co["adds"]["direct"].clone(), co["credits"]["direct"].clone(), co["net"].clone()), (json!(77.5), json!(-56.0), json!(21.5)));
        // On the net: tax 8% × $13 = $1.04 → $22.54; overhead 10% → $24.794; profit 10% → $27.27.
        assert_eq!(co["totals"]["total"], json!(27.27), "{co}");
        assert!(co["ok"].as_bool().unwrap(), "{co}");
        assert!(
            co["cover"].as_str().unwrap().contains("CHANGE ORDER CO-001 — re ASI-02") && co["cover"].as_str().unwrap().contains("$27.27"),
            "{}",
            co["cover"]
        );
        assert_eq!(co["sheets"], json!(["1"]));
        // One cloud round R2 (the only element that changed), labelled with the number.
        assert_eq!(co["clouds"].as_array().unwrap().len(), 1);
        let d = s.doc().unwrap();
        let clouds: Vec<_> = d.model.iter().filter(|e| super::super::printrev::label_of(e, APP) == Some("CO-001")).collect();
        assert_eq!(clouds.len(), 2, "a cloud and its label");
        assert!(clouds.iter().all(|e| e.common.layer == CLOUD_LAYER));
        // The same change, marked up on the adds only: the credit at cost with its tax.
        // Adds $77.50 + $1.44 tax = $78.94 → $86.834 → $95.5174; credits −$56 − $0.40 = −$56.40.
        let again = s.execute("co.price", &json!({ "number": "CO-001", "reference": "ASI-02", "markup_on": "adds", "replace": true })).unwrap();
        assert_eq!(
            (again["adds"]["total"].clone(), again["credits"]["total"].clone(), again["totals"]["total"].clone()),
            (json!(95.52), json!(-56.4), json!(39.12)),
            "{again}"
        );
        let d = s.doc().unwrap();
        assert_eq!(d.model.iter().filter(|e| super::super::printrev::label_of(e, APP) == Some("CO-001")).count(), 2, "the earlier clouds went");
        // CO-002 prices only its own change: R3 goes — credits only.
        set_tags(&mut s, json!([device("R1", 3.0, "standard duplex receptacle"), device("R2", 4.0, "GFCI receptacle")]));
        let c2 = s.execute("co.price", &json!({ "number": "CO-002", "reference": "RFI-007", "markup_on": "adds" })).unwrap();
        // GFCI $18 + box $2 + ring $1 + plate $0.50 = $21.50; labor 0.7 + 0.2 + 0.05 + 0.05 = 1 h = $85.
        assert_eq!((c2["adds"]["lines"].clone(), c2["credits"]["direct"].clone()), (json!(0), json!(-106.5)), "{c2}");
        assert_eq!(c2["totals"]["total"], json!(-108.22), "credits at cost with their tax: {c2}");
        assert!(c2["lines"].as_array().unwrap().iter().all(|l| l["change"] == json!("changed")));
        // The log.
        let log = s.execute("co.log", &json!({})).unwrap();
        assert_eq!(log["change_orders"].as_array().unwrap().len(), 2);
        assert_eq!(log["total"], json!(r2(39.12 - 108.22)));
        let one = s.execute("co.log", &json!({ "number": "co-002" })).unwrap();
        assert_eq!(one["change_order"]["reference"], json!("RFI-007"));
        // An older one can't be priced again; a number can't be used twice.
        assert!(s.execute("co.price", &json!({ "number": "CO-001", "replace": true })).is_err());
        assert!(s.execute("co.price", &json!({ "number": "CO-002" })).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_longer_run_and_a_new_size_match_the_estimate_and_write_the_package() {
        let (mut s, dir) = job("run");
        s.execute("co.snapshot", &json!({ "label": "Bid" })).unwrap();
        let before = s.execute("estimate.status", &json!({})).unwrap()["totals"]["total"].as_f64().unwrap();
        // Bulletin 3: the run goes on 50' further.
        let runs = json!([{"system": "E-BRANCH-COND", "level": "1", "points": [[0, 0], [1200, 0], [1200, 600]], "size_in": [0.75, 0], "bottoms_ft": [9, 9]}]);
        s.execute("bspec.set", &json!({ "spec": { "mep": { "runs": runs } } })).unwrap();
        let out = std::env::temp_dir().join(format!("cadaddy-copricing-{}-co.csv", std::process::id()));
        let co = s
            .execute("co.price", &json!({ "number": "CO-003", "reference": "Bulletin 3", "scope": "in_contract", "file": out.to_string_lossy() }))
            .unwrap();
        let after = s.execute("estimate.status", &json!({})).unwrap()["totals"]["total"].as_f64().unwrap();
        // The change order is the estimate's own difference (no contingency or bond set).
        assert!((co["totals"]["total"].as_f64().unwrap() - (after - before)).abs() < 0.05, "{co} vs {}", after - before);
        assert_eq!(co["from"], json!("Bid"));
        let emt = line(&co, "EMT conduit");
        // 100' → 150' net; with 10% waste 110' → 165': +55' × $1.10 = $60.50; +50' × 0.05 h × $85 = $212.50.
        assert_eq!((emt["qty_delta"].clone(), emt["material"].clone(), emt["labor"].clone()), (json!(55.0), json!(60.5), json!(212.5)), "{emt}");
        // The bend brings an elbow, new to the job (in contract by the call's `scope`): $1.50 + 0.1 h × $85.
        assert_eq!((line(&co, "EMT 90° elbow")["change"].clone(), line(&co, "EMT 90° elbow")["cost"].clone()), (json!("added"), json!(10.0)));
        assert!(co["ok"].as_bool().unwrap(), "{co}");
        assert!(!co["clouds"].as_array().unwrap().is_empty());
        let csv = std::fs::read_to_string(&out).unwrap();
        assert!(csv.starts_with("co,reference,change,") && csv.contains("CO-003,Bulletin 3,changed,electrical,conduit,EMT conduit"), "{csv}");
        assert!(csv.lines().last().unwrap().starts_with("CO-003,Bulletin 3,total,"), "{csv}");
        let _ = std::fs::remove_file(&out);
        // CO-004: a 1" run — a new size brings its hole saw (equipment) and new lines with no scope.
        let runs = json!([
            {"system": "E-BRANCH-COND", "level": "1", "points": [[0, 0], [1200, 0], [1200, 600]], "size_in": [0.75, 0], "bottoms_ft": [9, 9]},
            {"system": "E-BRANCH-COND", "level": "1", "points": [[0, 2400], [600, 2400]], "size_in": [1.0, 0], "bottoms_ft": [9]}
        ]);
        s.execute("bspec.set", &json!({ "spec": { "mep": { "runs": runs } } })).unwrap();
        let undecided = s.execute("co.price", &json!({ "number": "CO-004", "accept": false })).unwrap();
        assert!(!undecided["ok"].as_bool().unwrap());
        assert!(undecided["findings"].as_array().unwrap().iter().any(|f| f["kind"] == json!("undecided_scope")), "{undecided}");
        assert!(undecided["lines"].as_array().unwrap().iter().all(|l| l["size"] != json!("1\"")), "undecided lines never count");
        let co4 = s.execute("co.price", &json!({ "number": "CO-004", "replace": true, "scope": "in_contract" })).unwrap();
        assert!(
            co4["lines"]
                .as_array()
                .unwrap()
                .iter()
                .any(|l| l["item"] == json!("EMT conduit") && l["size"] == json!("1\"") && l["change"] == json!("added")),
            "{co4}"
        );
        assert_eq!(line(&co4, "Carbide hole saw for 1\" conduit")["equipment"], json!(40.0), "{co4}");
        let q = s.execute("estimate.query", &json!({ "status": "undecided" })).unwrap();
        assert_eq!(q["total"], json!(0), "the new lines' scope is recorded: {q}");
        // Withdrawn (priced again, not accepted): the snapshot goes back, so the next change order
        // carries the same lines.
        let withdrawn = s.execute("co.price", &json!({ "number": "CO-004", "replace": true, "accept": false })).unwrap();
        let next = s.execute("co.price", &json!({ "number": "CO-005" })).unwrap();
        assert_eq!(next["totals"], withdrawn["totals"]);
        assert_eq!(next["from"], json!("CO-003"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_calls_are_refused() {
        let (mut s, dir) = job("bad");
        assert!(s.execute("co.price", &json!({ "number": "CO-1" })).is_err(), "no snapshot yet");
        s.execute("co.snapshot", &json!({})).unwrap();
        assert!(s.execute("co.price", &json!({})).is_err());
        assert!(s.execute("co.price", &json!({ "number": "  " })).is_err());
        assert!(s.execute("co.price", &json!({ "number": "X".repeat(65) })).is_err());
        assert!(s.execute("co.price", &json!({ "number": "CO-1", "markup_on": "gross" })).is_err());
        assert!(s.execute("co.price", &json!({ "number": "CO-1", "scope": "maybe" })).is_err());
        assert!(s.execute("co.price", &json!({ "number": "CO-1", "file": "/tmp/co.exe" })).is_err());
        assert!(s.execute("co.log", &json!({ "number": "CO-9" })).is_err());
        // Nothing changed: an empty change order, no clouds.
        let co = s.execute("co.price", &json!({ "number": "CO-1", "revision": "Rev 9", "markups": { "profit_pct": f64::NAN } })).unwrap();
        assert_eq!((co["lines"].as_array().unwrap().len(), co["totals"]["total"].clone()), (0, json!(0.0)));
        assert!(co["findings"].as_array().unwrap().iter().any(|f| f["kind"] == json!("revision_not_found")));
        assert!(co["clouds"].as_array().unwrap().is_empty());
        assert_eq!(money(-1234567.891), "-$1,234,567.89");
        assert_eq!(money(0.5), "$0.50");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
