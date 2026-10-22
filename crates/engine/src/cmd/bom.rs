//! The job's bill of materials (Q326): every modelled item and the small parts the rules imply,
//! across the trades, with waste shown apart from the net, firm vs estimated quantities, the
//! required job items (tools, consumables, setup, labeling), a check that what the documents
//! require has a line, and what changed since the last BOM.
//!
//! `bim.bom` (typed `BOM`):
//! - **Electrical** from the element tags (Q299): luminaires by schedule type, devices by type,
//!   a box, plaster ring and plate per wall device, wire from each homerun's conductors over its
//!   route (plus makeup at each end), and branch conduit by material and size with its couplings,
//!   connectors, straps and elbows, each by a stated rule.
//! - **Mechanical and plumbing** from `mep.qty` (Q269): duct by size and gauge (ft, sq ft, lb),
//!   pipe by system and size, fittings, terminals, equipment, hangers.
//! - **Drywall** from `drywall.takeoff` (Q294) where the walls have assemblies.
//! - **Waste** per category (defaults below; project rules `custom` `waste.<category>` or the
//!   `waste` param override), shown as its own column.
//! - **Required job items** from the modelled work, each with the rule that requires it; a line
//!   is zeroed only with a reason (`waive`), recorded in the project rules
//!   (`custom` `bom.waive.<item>`). The BOM is `complete` when every required line is there or
//!   waived and the spec check is clean.
//! - **Spec check**: each catalog type (Q297/Q298) whose finish/colour/options the documents fix
//!   must have a BOM line carrying them; separately sold accessories (plates for devices,
//!   covers/trims and lug kits for panels) must have their lines.
//! - `compare_to`: an earlier BOM's `lines` → added, removed and changed quantities.
//! - `group_by`: `job` (default), `level` or `trade`; `file` writes CSV or XLSX, `columns` renames
//!   the columns for another estimating tool.

use cadaddy_doc::projrules::{RuleDomain, RuleValue};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::bspec::current;
use super::*;
use crate::{Result, Session};

/// Lines in one BOM, at most (hostile specs).
const MAX_LINES: usize = 50_000;
/// A conduit stick, feet (a coupling joins each two).
const STICK_FT: f64 = 10.0;
/// Conduit supports: within 3' of each end and every 10' between (NEC 358.30 for EMT).
const STRAP_SPACING_FT: f64 = 10.0;
/// Wire left at each end for makeup, feet.
const MAKEUP_FT: f64 = 1.0;

/// Default waste, percent, by category.
fn default_waste(category: &str) -> f64 {
    match category {
        "conduit" | "wire" | "duct" | "pipe" => 10.0,
        "conduit fittings" | "fittings" | "boxes" | "hangers" => 5.0,
        "drywall" => 12.0,
        "devices" | "fixtures" => 2.0,
        _ => 0.0,
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bim.bom", "Bill of Materials", run)
            .alias(&["bom"])
            .params(
                "{group_by?: job|level|trade, waste?: {category: percent}, waive?: [{item, reason}], compare_to?: {lines} (an earlier BOM), file?: path (.csv/.xlsx), columns?: {trade|category|item|size|unit|net|waste|total|status|rule|level: header}} → {lines: [{trade, category, item, size, unit, net, waste_pct, waste, total, status: firm|rule|model, rule?, level?, elements}], required: [{item, qty, supply: purchase|rental|owned, rule, waived?}], spec_check: [{issue, item, detail}], complete, changes?: {added, removed, changed}, csv, path?}",
            ),
    ]
}

/// One BOM line before grouping.
#[derive(Clone, Debug, Default)]
struct Line {
    trade: &'static str,
    category: String,
    item: String,
    size: String,
    unit: &'static str,
    net: f64,
    /// `firm` (fixed by the documents), `rule` (a rule produced it), `model` (from the model's
    /// geometry, as built).
    status: &'static str,
    rule: Option<String>,
    level: String,
    elements: Vec<String>,
}

impl Line {
    fn key(&self, by_level: bool) -> String {
        format!("{}|{}|{}|{}|{}", self.trade, self.category, self.item, self.size, if by_level { self.level.as_str() } else { "" })
    }
}

/// Add `l` into `lines`, merging with an equal line (same trade, category, item, size, level).
fn add(lines: &mut Vec<Line>, l: Line) {
    if l.net <= 0.0 || lines.len() >= MAX_LINES {
        return;
    }
    match lines.iter_mut().find(|x| x.key(true) == l.key(true) && x.status == l.status && x.rule == l.rule) {
        Some(x) => {
            x.net += l.net;
            x.elements.extend(l.elements);
        }
        None => lines.push(l),
    }
}

/// A run's plan length, feet.
pub(super) fn run_length_ft(points: &[[f64; 2]], upf: f64) -> f64 {
    points.windows(2).map(|w| if let [a, b] = w { (b[0] - a[0]).hypot(b[1] - a[1]) } else { 0.0 }).sum::<f64>() / upf
}

/// Conductors `2#12 + 1#12G` → (count, size, ground?) parts.
fn conductor_parts(makeup: &str) -> Vec<(u32, String, bool)> {
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

/// AWG #4 and larger (`4`, `2`, `1/0`…): conductors that need insulating bushings (NEC 300.4(G)).
fn large_conductor(size: &str) -> bool {
    size.contains('/') || size.parse::<u32>().is_ok_and(|n| n <= 4)
}

fn electrical(s: &Session, lines: &mut Vec<Line>) -> Result<()> {
    let (sp, _) = current(s, "bim.bom")?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    // A luminaire type's schedule description, from whichever tag carries it.
    let mut described: BTreeMap<String, String> = BTreeMap::new();
    for t in sp.tags.iter().filter(|t| t.kind == "luminaire") {
        if let (Some(ty), Some(desc)) = (t.fields.get("type"), t.fields.get("luminaire")) {
            described.entry(ty.value.clone()).or_insert_with(|| desc.value.clone());
        }
    }
    for t in sp.tags.iter().take(MAX_LINES) {
        let f = |k: &str| t.fields.get(k).map(|v| (v.value.clone(), v.source.clone()));
        let firm = |src: &str| if matches!(src, "plan" | "schedule" | "note" | "user") { "firm" } else { "rule" };
        match t.kind.as_str() {
            "luminaire" => {
                let (ty, src) = f("type").unwrap_or_else(|| ("UNTAGGED".into(), "default".into()));
                let desc = described.get(&ty).cloned().unwrap_or_default();
                add(
                    lines,
                    Line {
                        trade: "electrical",
                        category: "fixtures".into(),
                        item: format!("Luminaire type {ty}"),
                        size: desc,
                        unit: "each",
                        net: 1.0,
                        status: firm(&src),
                        rule: None,
                        level: t.level.clone(),
                        elements: vec![t.id.clone()],
                    },
                );
            }
            "homerun" | "conduit" => {}
            kind => {
                let (ty, src) = f("device_type").unwrap_or_else(|| (kind.replace('_', " "), "plan".into()));
                add(
                    lines,
                    Line {
                        trade: "electrical",
                        category: "devices".into(),
                        item: ty,
                        size: String::new(),
                        unit: "each",
                        net: 1.0,
                        status: firm(&src),
                        rule: None,
                        level: t.level.clone(),
                        elements: vec![t.id.clone()],
                    },
                );
                if matches!(kind, "receptacle" | "switch" | "data_outlet") {
                    let rule = "one box, plaster ring and plate per wall device (company default)".to_string();
                    for (item, size) in [("Box, 4\" square", "2-1/8\" deep"), ("Plaster ring", "1 gang"), ("Device plate", "1 gang")] {
                        add(
                            lines,
                            Line {
                                trade: "electrical",
                                category: "boxes".into(),
                                item: item.into(),
                                size: size.into(),
                                unit: "each",
                                net: 1.0,
                                status: "rule",
                                rule: Some(rule.clone()),
                                level: t.level.clone(),
                                elements: vec![t.id.clone()],
                            },
                        );
                    }
                }
            }
        }
    }
    // Branch conduit and its implied fittings; homerun wire over its route.
    let kind_of = |sys: &str| sp.mep.systems.iter().find(|x| x.name == sys).map(|x| x.kind);
    for (i, r) in sp.mep.runs.iter().enumerate().take(MAX_LINES) {
        let len = run_length_ft(&r.points, upf);
        if !(len.is_finite() && len > 0.0) {
            continue;
        }
        let id = format!("run:{i}");
        match kind_of(&r.system) {
            Some(cadaddy_walk3d::MepKind::Conduit) => {
                let size = format!("{}\"", trade_size(r.size_in[0]));
                let el = vec![id.clone()];
                add(
                    lines,
                    Line {
                        trade: "electrical",
                        category: "conduit".into(),
                        item: "EMT conduit".into(),
                        size: size.clone(),
                        unit: "ft",
                        net: len,
                        status: "model",
                        rule: None,
                        level: r.level.clone(),
                        elements: el.clone(),
                    },
                );
                let sticks = (len / STICK_FT).ceil();
                let straps = (len / STRAP_SPACING_FT).ceil() + 1.0;
                let elbows = r.points.len().saturating_sub(2) as f64;
                let parts: [(&str, f64, String); 4] = [
                    ("EMT coupling", (sticks - 1.0).max(0.0), format!("one per joint between {STICK_FT}' sticks")),
                    ("EMT connector", 2.0, "one at each conduit end".into()),
                    ("Conduit strap", straps, format!("within 3' of each end and every {STRAP_SPACING_FT}' (NEC 358.30)")),
                    ("EMT 90° elbow", elbows, "one at each turn of the routed run".into()),
                ];
                for (item, n, rule) in parts {
                    add(
                        lines,
                        Line {
                            trade: "electrical",
                            category: "conduit fittings".into(),
                            item: item.into(),
                            size: size.clone(),
                            unit: "each",
                            net: n,
                            status: "rule",
                            rule: Some(rule),
                            level: r.level.clone(),
                            elements: el.clone(),
                        },
                    );
                }
            }
            Some(cadaddy_walk3d::MepKind::Cable) => {
                // The homerun this route is for: its tag starts where the route does.
                let origin = r.points.first().map(|p| [p[0] / upf, p[1] / upf]).unwrap_or([f64::NAN, f64::NAN]);
                let tag = sp.tags.iter().find(|t| {
                    t.kind == "homerun" && t.level.eq_ignore_ascii_case(&r.level) && (t.at_ft[0] - origin[0]).hypot(t.at_ft[1] - origin[1]) < 1.0
                });
                let makeup = tag
                    .and_then(|t| t.fields.get("conductors"))
                    .map_or_else(|| ("2#12 + 1#12G".to_string(), "rule".to_string()), |f| (f.value.clone(), f.source.clone()));
                let each = len + 2.0 * MAKEUP_FT;
                for (n, size, ground) in conductor_parts(&makeup.0) {
                    let item = if ground { "Copper ground, green".to_string() } else { "THHN copper".to_string() };
                    let el = tag.map_or_else(|| vec![id.clone()], |t| vec![t.id.clone()]);
                    add(
                        lines,
                        Line {
                            trade: "electrical",
                            category: "wire".into(),
                            item,
                            size: format!("#{size}"),
                            unit: "ft",
                            net: f64::from(n) * each,
                            status: "rule",
                            rule: Some(format!("{} over the assumed homerun route plus {MAKEUP_FT}' makeup at each end", makeup.0)),
                            level: r.level.clone(),
                            elements: el,
                        },
                    );
                    if large_conductor(&size) && !ground {
                        add(
                            lines,
                            Line {
                                trade: "electrical",
                                category: "conduit fittings".into(),
                                item: "Insulating bushing (bang-on)".into(),
                                size: String::new(),
                                unit: "each",
                                net: 2.0,
                                status: "rule",
                                rule: Some("#4 AWG and up: an insulating bushing at each raceway end (NEC 300.4(G))".into()),
                                level: r.level.clone(),
                                elements: vec![id.clone()],
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// A trade size as printed (`3/4`, `1-1/4`).
fn trade_size(v: f64) -> String {
    let e = (v * 8.0).round() as i64;
    let (w, r) = (e / 8, e % 8);
    let frac = match r {
        0 => String::new(),
        x if x % 4 == 0 => format!("{}/2", x / 4),
        x if x % 2 == 0 => format!("{}/4", x / 2),
        x => format!("{x}/8"),
    };
    match (w, frac.is_empty()) {
        (0, false) => frac,
        (w, true) => w.to_string(),
        (w, false) => format!("{w}-{frac}"),
    }
}

/// Mechanical and plumbing from `mep.qty` (conduit is counted above, from the runs).
fn mechanical(s: &mut Session, lines: &mut Vec<Line>) {
    // Raceway and cable systems are taken off above, with their own fittings and supports.
    let raceway: Vec<String> = current(s, "bim.bom")
        .map(|(sp, _)| {
            sp.mep
                .systems
                .iter()
                .filter(|x| matches!(x.kind, cadaddy_walk3d::MepKind::Conduit | cadaddy_walk3d::MepKind::Cable))
                .map(|x| x.name.clone())
                .collect()
        })
        .unwrap_or_default();
    let Ok(q) = s.execute("mep.qty", &json!({ "group_by": "level_system" })) else { return };
    let ours = |v: &Value| v["system"].as_str().is_some_and(|x| raceway.iter().any(|r| r == x));
    let arr = |k: &str| q[k].as_array().cloned().unwrap_or_default();
    let st = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
    let num = |v: &Value, k: &str| v[k].as_f64().unwrap_or(0.0);
    for d in arr("ducts") {
        let size = format!("{} {}", st(&d, "size"), st(&d, "shape"));
        let lv = st(&d, "level");
        add(
            lines,
            Line {
                trade: "mechanical",
                category: "duct".into(),
                item: format!("Duct {}, {} ga", st(&d, "system"), d["gauge"]),
                size: size.clone(),
                unit: "ft",
                net: num(&d, "length_ft"),
                status: "model",
                level: lv.clone(),
                ..Line::default()
            },
        );
        add(
            lines,
            Line {
                trade: "mechanical",
                category: "duct".into(),
                item: format!("Sheet metal {} ga", d["gauge"]),
                size,
                unit: "lb",
                net: num(&d, "weight_lb"),
                status: "model",
                level: lv,
                ..Line::default()
            },
        );
    }
    for p in arr("pipes").into_iter().filter(|p| !matches!(p["kind"].as_str(), Some("conduit" | "cable"))) {
        let trade = if p["kind"].as_str() == Some("tray") { "electrical" } else { "plumbing" };
        add(
            lines,
            Line {
                trade,
                category: "pipe".into(),
                item: format!("{} {}", st(&p, "system"), st(&p, "kind")),
                size: st(&p, "size"),
                unit: "ft",
                net: num(&p, "length_ft"),
                status: "model",
                level: st(&p, "level"),
                ..Line::default()
            },
        );
    }
    for f in arr("fittings").into_iter().filter(|f| !ours(f)) {
        add(
            lines,
            Line {
                trade: "mechanical",
                category: "fittings".into(),
                item: format!("{} {}", st(&f, "system"), st(&f, "type")),
                size: st(&f, "size"),
                unit: "each",
                net: num(&f, "count"),
                status: "model",
                level: st(&f, "level"),
                ..Line::default()
            },
        );
    }
    for t in arr("terminals") {
        add(
            lines,
            Line {
                trade: "mechanical",
                category: "terminals".into(),
                item: st(&t, "kind"),
                size: st(&t, "size"),
                unit: "each",
                net: num(&t, "count"),
                status: "model",
                level: st(&t, "level"),
                ..Line::default()
            },
        );
    }
    // Lugs ordered separately from their gear (`gear.check`'s lug kits): a line per part of the
    // kit, on the gear's level, so they are bought with it.
    let equipment = arr("equipment");
    for k in arr("lug_kits") {
        let gear = st(&k, "gear");
        let level = equipment.iter().find(|e| e["tag"].as_str() == Some(gear.as_str())).map(|e| st(e, "level")).unwrap_or_default();
        let at = format!("{gear} {}", st(&k, "point"));
        let lug = st(&k, "lug");
        let bolt = st(&k, "bolt");
        for (part, qty) in [
            (lug.as_str(), "lugs"),
            (bolt.as_str(), "bolts"),
            ("nut", "nuts"),
            ("flat washer", "flat_washers"),
            ("Belleville washer", "belleville_washers"),
        ] {
            add(
                lines,
                Line {
                    trade: "electrical",
                    category: "lug kits".into(),
                    item: format!("Lug kit {at}: {part}"),
                    unit: "each",
                    net: num(&k, qty),
                    status: "rule",
                    rule: Some("lugs not supplied with the gear (gear.check)".into()),
                    level: level.clone(),
                    elements: vec![format!("gear:{gear}")],
                    ..Line::default()
                },
            );
        }
    }
    for e in equipment {
        let electrical = matches!(
            st(&e, "kind").to_ascii_lowercase().as_str(),
            "panel" | "panelboard" | "switchboard" | "switchgear" | "transformer" | "disconnect"
        );
        add(
            lines,
            Line {
                trade: if electrical { "electrical" } else { "mechanical" },
                category: "equipment".into(),
                item: format!("{} {}", st(&e, "kind"), st(&e, "tag")),
                size: st(&e, "size"),
                unit: "each",
                net: 1.0,
                status: "firm",
                level: st(&e, "level"),
                elements: vec![st(&e, "tag")].into_iter().filter(|t| !t.is_empty()).collect(),
                ..Line::default()
            },
        );
    }
    for h in arr("hangers").into_iter().filter(|h| !ours(h)) {
        let rule = format!("{}' spacing", h["spacing_ft"]);
        add(
            lines,
            Line {
                trade: "mechanical",
                category: "hangers".into(),
                item: format!("Hanger, {}", st(&h, "kind")),
                size: st(&h, "size"),
                unit: "each",
                net: num(&h, "count"),
                status: "rule",
                rule: Some(rule),
                level: st(&h, "level"),
                ..Line::default()
            },
        );
    }
}

/// Drywall from `drywall.takeoff`, where walls have assemblies.
fn drywall(s: &mut Session, lines: &mut Vec<Line>) {
    let Ok(q) = s.execute("drywall.takeoff", &json!({ "waste": 0 })) else { return };
    for r in q["rows"].as_array().into_iter().flatten() {
        for b in r["boards"].as_array().into_iter().flatten() {
            let item = format!("Board {}", b["material"].as_str().unwrap_or_default());
            let size = format!("{}\"", b["thicknessIn"]);
            add(
                lines,
                Line {
                    trade: "drywall",
                    category: "drywall".into(),
                    item,
                    size,
                    unit: "sq ft",
                    net: b["areaSqFt"].as_f64().unwrap_or(0.0),
                    status: "model",
                    ..Line::default()
                },
            );
        }
        let studs = r["studs"]["lengthFt"].as_f64().unwrap_or(0.0);
        add(
            lines,
            Line {
                trade: "drywall",
                category: "drywall".into(),
                item: format!("Studs {}", r["studs"]["material"].as_str().unwrap_or_default()),
                unit: "ft",
                net: studs,
                status: "model",
                ..Line::default()
            },
        );
        add(
            lines,
            Line {
                trade: "drywall",
                category: "drywall".into(),
                item: "Track".into(),
                unit: "ft",
                net: r["track"]["lengthFt"].as_f64().unwrap_or(0.0),
                status: "model",
                ..Line::default()
            },
        );
    }
}

/// The job's required items from the modelled work.
fn required(lines: &[Line], levels: usize) -> Vec<(String, f64, &'static str, String)> {
    let mut out: Vec<(String, f64, &'static str, String)> = Vec::new();
    let has = |cat: &str| lines.iter().any(|l| l.category == cat);
    let conduit_sizes: Vec<String> = {
        let mut v: Vec<String> = lines.iter().filter(|l| l.category == "conduit").map(|l| l.size.clone()).collect();
        v.sort();
        v.dedup();
        v
    };
    for sz in &conduit_sizes {
        out.push((format!("Carbide hole saw for {sz} conduit"), 1.0, "purchase", format!("{sz} conduit is modelled: a hole saw sized to it")));
        out.push((format!("Conduit bender {sz}"), 1.0, "owned", format!("{sz} EMT bends are modelled")));
    }
    if !conduit_sizes.is_empty() {
        out.push(("Knockout set".into(), 1.0, "owned", "conduit enters boxes and panels".into()));
    }
    let wire_ft: f64 = lines.iter().filter(|l| l.category == "wire").map(|l| l.net).sum();
    if wire_ft > 0.0 {
        out.push(("Fish tape".into(), 1.0, "owned", "wire is pulled in raceways".into()));
        if wire_ft > 2_000.0 {
            out.push(("Wire tugger".into(), 1.0, "rental", format!("{wire_ft:.0} ft of wire to pull")));
        }
        out.push(("Wire pulling lube".into(), (wire_ft / 5_000.0).ceil().max(1.0), "purchase", "one quart per 5,000 ft pulled".into()));
    }
    if lines.iter().any(|l| l.category == "wire" && large_conductor(l.size.trim_start_matches('#'))) {
        out.push(("Crimper with dies".into(), 1.0, "rental", "#4 AWG and larger conductors need lugs".into()));
        out.push(("Torque wrench".into(), 1.0, "owned", "lugs are torqued to the listing".into()));
    }
    if has("devices") || has("fixtures") {
        out.push(("Label printer and tape".into(), 1.0, "purchase", "devices and panels are labelled".into()));
    }
    if !lines.is_empty() {
        out.push(("Gang box".into(), levels.max(1) as f64, "owned", "one per floor".into()));
        out.push(("Step ladder, 8'".into(), levels.max(1) as f64, "owned", "one per floor for ceiling work".into()));
        out.push(("Temporary power".into(), 1.0, "rental", "the job needs power before the service".into()));
    }
    out
}

/// The electrical and mechanical lines per floor, before grouping and waste, each with every
/// element it counts (`bim.bom` lists at most 500): what progress marks resolve against (Q332).
pub(super) fn model_lines(s: &mut Session) -> Result<Vec<Value>> {
    let mut lines: Vec<Line> = Vec::new();
    electrical(s, &mut lines)?;
    mechanical(s, &mut lines);
    Ok(lines
        .into_iter()
        .map(|l| {
            json!({"trade": l.trade, "category": l.category, "item": l.item, "size": l.size, "unit": l.unit, "net": l.net, "level": l.level, "elements": l.elements})
        })
        .collect())
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let group = str_param(p, "group_by").unwrap_or("job").to_ascii_lowercase();
    if !matches!(group.as_str(), "job" | "level" | "trade") {
        return Err(bad("bim.bom", "`group_by` is job, level or trade"));
    }
    let mut lines: Vec<Line> = Vec::new();
    electrical(s, &mut lines)?;
    mechanical(s, &mut lines);
    drywall(s, &mut lines);
    // Waivers: recorded in the project rules, with the reason.
    if let Some(ws) = p.get("waive").and_then(Value::as_array) {
        for w in ws.iter().take(1_000) {
            let (Some(item), Some(reason)) = (w["item"].as_str(), w["reason"].as_str().map(str::trim).filter(|r| !r.is_empty())) else {
                return Err(bad("bim.bom", "each waiver needs an `item` and a `reason` (owned / not needed / by others)"));
            };
            s.doc_mut()?.project_rules.set(RuleDomain::Custom, &format!("bom.waive.{item}"), RuleValue::new(reason), reason);
        }
    }
    let d = s.doc()?;
    let rules = &d.project_rules;
    let waste_of = |cat: &str| -> f64 {
        p["waste"][cat]
            .as_f64()
            .or_else(|| {
                rules.resolve(RuleDomain::Custom, &format!("waste.{cat}")).and_then(|v| v.value.trim().trim_end_matches('%').parse::<f64>().ok())
            })
            .filter(|v| v.is_finite() && (0.0..=200.0).contains(v))
            .unwrap_or_else(|| default_waste(cat))
    };
    let (sp, _) = current(s, "bim.bom")?;
    let by_level = group == "level";
    // Group.
    let mut grouped: Vec<Line> = Vec::new();
    for l in lines {
        let l = Line { level: if by_level { l.level } else { String::new() }, ..l };
        match grouped.iter_mut().find(|x| x.key(by_level) == l.key(by_level) && x.status == l.status && x.rule == l.rule) {
            Some(x) => {
                x.net += l.net;
                x.elements.extend(l.elements);
            }
            None => grouped.push(l),
        }
    }
    grouped.sort_by(|a, b| {
        a.trade
            .cmp(b.trade)
            .then_with(|| a.category.cmp(&b.category))
            .then_with(|| a.item.cmp(&b.item))
            .then_with(|| a.size.cmp(&b.size))
            .then_with(|| a.level.cmp(&b.level))
    });
    let round = |v: f64| (v * 100.0).round() / 100.0;
    let out_lines: Vec<Value> = grouped
        .iter()
        .map(|l| {
            let pct = waste_of(&l.category);
            // Whole units of counted items (to the nearest one), lengths and areas as they are.
            let waste = if l.unit == "each" { (l.net * pct / 100.0).round() } else { l.net * pct / 100.0 };
            let mut v = json!({
                "trade": l.trade, "category": l.category, "item": l.item, "size": l.size, "unit": l.unit,
                "net": round(l.net), "waste_pct": pct, "waste": round(waste), "total": round(l.net + waste),
                "status": l.status, "elements": l.elements.iter().take(500).collect::<Vec<_>>(),
            });
            if let Some(r) = &l.rule {
                v["rule"] = json!(r);
            }
            if by_level {
                v["level"] = json!(l.level);
            }
            v
        })
        .collect();
    // Required job items and their waivers.
    let mut complete = true;
    let req: Vec<Value> = required(&grouped, sp.levels.len())
        .into_iter()
        .map(|(item, qty, supply, rule)| {
            let waived = rules.resolve(RuleDomain::Custom, &format!("bom.waive.{item}")).map(|v| v.value.clone());
            let mut v = json!({ "item": item, "qty": if waived.is_some() { 0.0 } else { qty }, "supply": supply, "rule": rule });
            if let Some(w) = waived {
                v["waived"] = json!(w);
            }
            complete &= qty > 0.0;
            v
        })
        .collect();
    // Spec check: catalog requirements and separately sold accessories.
    let mut check: Vec<Value> = Vec::new();
    for t in d.bim_types.types.iter().take(MAX_LINES) {
        let wanted: Vec<String> =
            ["finish", "color", "colour", "options"].iter().filter_map(|k| t.fields.get(*k).map(|f| format!("{k}: {}", f.value))).collect();
        if wanted.is_empty() {
            continue;
        }
        let carried = grouped.iter().any(|l| l.item.contains(&format!("type {}", t.mark)) || l.item.eq_ignore_ascii_case(&t.mark));
        if !carried {
            check.push(json!({ "issue": "requirement_without_line", "item": t.mark, "detail": format!("{} ({})", wanted.join(", "), t.fields.values().next().map(|f| f.source.clone()).unwrap_or_default()) }));
        }
    }
    let devices: f64 = grouped.iter().filter(|l| l.category == "devices" && !l.item.contains("floor")).map(|l| l.net).sum();
    let plates: f64 = grouped.iter().filter(|l| l.item == "Device plate").map(|l| l.net).sum();
    if devices > 0.0 && plates <= 0.0 {
        check.push(json!({ "issue": "accessory_missing", "item": "Device plate", "detail": "devices are on the BOM without their plates (sold separately)" }));
    }
    for e in grouped.iter().filter(|l| l.category == "equipment" && l.item.to_ascii_lowercase().contains("panel")) {
        for acc in ["Panel cover/trim", "Lug kit"] {
            if !grouped.iter().any(|l| l.item.starts_with(acc)) {
                check.push(json!({ "issue": "accessory_missing", "item": acc, "detail": format!("{} has no {acc} line (sold separately)", e.item) }));
            }
        }
    }
    let req_ok = req.iter().all(|r| r["qty"].as_f64().unwrap_or(0.0) > 0.0 || r["waived"].is_string());
    let mut out = json!({ "lines": out_lines, "required": req, "spec_check": check, "complete": complete && req_ok && check.is_empty() });
    // What changed since an earlier BOM.
    if let Some(prev) = p.get("compare_to").and_then(|v| v["lines"].as_array().or_else(|| v.as_array())) {
        out["changes"] = changes(prev, out["lines"].as_array().map(Vec::as_slice).unwrap_or_default());
    }
    // CSV / XLSX.
    let cols = ["trade", "category", "item", "size", "unit", "net", "waste", "total", "status", "rule", "level"];
    let header: Vec<String> = cols.iter().map(|c| p["columns"][*c].as_str().map_or_else(|| (*c).to_string(), str::to_string)).collect();
    let mut rows = vec![header];
    for l in out["lines"].as_array().into_iter().flatten() {
        rows.push(
            cols.iter()
                .map(|c| match &l[*c] {
                    Value::String(x) => x.clone(),
                    Value::Null => String::new(),
                    v => v.to_string(),
                })
                .collect(),
        );
    }
    out["csv"] = json!(
        rows.iter()
            .map(|r| r
                .iter()
                .map(|c| if c.contains([',', '"', '\n']) { format!("\"{}\"", c.replace('"', "\"\"")) } else { c.clone() })
                .collect::<Vec<_>>()
                .join(","))
            .collect::<Vec<_>>()
            .join("\n")
    );
    if let Some(path) = str_param(p, "file").filter(|x| !x.trim().is_empty()) {
        let xlsx = path.to_ascii_lowercase().ends_with(".xlsx");
        let bytes = if xlsx {
            super::table::xlsx::write(&rows, "Bill of Materials").map_err(|e| bad("bim.bom", e))?
        } else {
            out["csv"].as_str().unwrap_or_default().as_bytes().to_vec()
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::writefile::write(path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
            out["path"] = json!(path);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = bytes;
    }
    Ok(out)
}

/// Added, removed and changed lines between two BOMs (by trade, category, item, size, level).
fn changes(prev: &[Value], now: &[Value]) -> Value {
    let key = |l: &Value| {
        format!(
            "{}|{}|{}|{}|{}",
            l["trade"].as_str().unwrap_or_default(),
            l["category"].as_str().unwrap_or_default(),
            l["item"].as_str().unwrap_or_default(),
            l["size"].as_str().unwrap_or_default(),
            l["level"].as_str().unwrap_or_default()
        )
    };
    let sum = |ls: &[Value]| {
        let mut m: BTreeMap<String, f64> = BTreeMap::new();
        for l in ls.iter().take(MAX_LINES) {
            *m.entry(key(l)).or_insert(0.0) += l["net"].as_f64().unwrap_or(0.0);
        }
        m
    };
    let (a, b) = (sum(prev), sum(now));
    let added: Vec<Value> = b.iter().filter(|(k, _)| !a.contains_key(*k)).map(|(k, v)| json!({ "line": k, "net": v })).collect();
    let removed: Vec<Value> = a.iter().filter(|(k, _)| !b.contains_key(*k)).map(|(k, v)| json!({ "line": k, "net": v })).collect();
    let changed: Vec<Value> = b
        .iter()
        .filter_map(|(k, v)| {
            a.get(k).filter(|w| (*w - v).abs() > 1e-6).map(|w| json!({ "line": k, "was": w, "now": v, "delta": ((v - w) * 100.0).round() / 100.0 }))
        })
        .collect();
    json!({ "added": added, "removed": removed, "changed": changed })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small electrical job: two luminaires, three receptacles (one GFCI), a 25' conduit run
    /// with a turn, and a homerun of `2#12 + 1#12G` over 30'.
    fn job(s: &mut Session) {
        let tags = json!([
            {"id": "L1", "kind": "luminaire", "level": "1", "at_ft": [1, 1], "fields": {"type": {"value": "A", "source": "plan"}, "luminaire": {"value": "2X4 TROFFER", "source": "schedule"}}},
            {"id": "L2", "kind": "luminaire", "level": "1", "at_ft": [2, 1], "fields": {"type": {"value": "A", "source": "plan"}}},
            {"id": "R1", "kind": "receptacle", "level": "1", "at_ft": [3, 1], "fields": {"device_type": {"value": "standard duplex receptacle", "source": "default"}}},
            {"id": "R2", "kind": "receptacle", "level": "1", "at_ft": [4, 1], "fields": {"device_type": {"value": "standard duplex receptacle", "source": "default"}}},
            {"id": "R3", "kind": "receptacle", "level": "1", "at_ft": [5, 1], "fields": {"device_type": {"value": "GFCI receptacle", "source": "plan"}}},
            {"id": "H1", "kind": "homerun", "level": "1", "at_ft": [0, 0], "fields": {"conductors": {"value": "2#12 + 1#12G", "source": "note"}}}
        ]);
        let spec = json!({
            "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}],
            "mep": {
                "systems": [
                    {"name": "E-BRANCH-COND", "layer": "E-BRANCH-COND", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9},
                    {"name": "E-HOMERUN", "layer": "E-HOMERUN", "kind": "cable", "size_in": [0.5, 0], "bottom_ft": 9}
                ],
                "runs": [
                    {"system": "E-BRANCH-COND", "level": "1", "points": [[0, 0], [240, 0], [240, 60]], "size_in": [0.75, 0], "bottoms_ft": [9, 9]},
                    {"system": "E-HOMERUN", "level": "1", "points": [[0, 0], [360, 0]], "size_in": [0.5, 0], "bottoms_ft": [9]}
                ]
            },
            "tags": tags
        });
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
    }

    fn line<'a>(r: &'a Value, item: &str) -> &'a Value {
        r["lines"].as_array().unwrap().iter().find(|l| l["item"] == json!(item)).unwrap_or(&Value::Null)
    }

    #[test]
    fn a_small_job_takes_off_with_its_implied_parts_and_waste() {
        let mut s = Session::new();
        job(&mut s);
        let r = s.execute("bim.bom", &json!({})).unwrap();
        assert_eq!(line(&r, "Luminaire type A")["net"], json!(2.0), "{r}");
        assert_eq!(line(&r, "standard duplex receptacle")["net"], json!(2.0));
        assert_eq!(line(&r, "GFCI receptacle")["net"], json!(1.0));
        assert_eq!(line(&r, "Device plate")["net"], json!(3.0));
        assert!(line(&r, "Device plate")["rule"].is_string());
        // 25' of 3/4" EMT: 3 sticks → 2 couplings, 2 connectors, 4 straps, 1 elbow; 10% waste.
        let c = line(&r, "EMT conduit");
        assert_eq!(
            (c["size"].clone(), c["net"].clone(), c["waste"].clone(), c["total"].clone()),
            (json!("3/4\""), json!(25.0), json!(2.5), json!(27.5))
        );
        assert_eq!(line(&r, "EMT coupling")["net"], json!(2.0));
        assert_eq!(line(&r, "EMT connector")["net"], json!(2.0));
        assert_eq!(line(&r, "Conduit strap")["net"], json!(4.0));
        assert_eq!(line(&r, "EMT 90° elbow")["net"], json!(1.0));
        // 2#12 + 1#12G over 30' plus 1' makeup at each end: 64' of #12 THHN, 32' of ground.
        let w = r["lines"].as_array().unwrap().iter().find(|l| l["item"] == json!("THHN copper")).unwrap();
        assert_eq!((w["size"].clone(), w["net"].clone()), (json!("#12"), json!(64.0)));
        assert_eq!(line(&r, "Copper ground, green")["net"], json!(32.0));
        // Firm vs rule.
        assert_eq!(line(&r, "GFCI receptacle")["status"], json!("firm"));
        assert_eq!(line(&r, "EMT coupling")["status"], json!("rule"));
        // Required items follow the work.
        let req: Vec<&str> = r["required"].as_array().unwrap().iter().filter_map(|x| x["item"].as_str()).collect();
        assert!(
            req.contains(&"Carbide hole saw for 3/4\" conduit") && req.contains(&"Knockout set") && req.contains(&"Label printer and tape"),
            "{req:?}"
        );
        assert!(r["csv"].as_str().unwrap().lines().count() > 10);
    }

    #[test]
    fn waivers_are_recorded_and_changes_are_reported() {
        let mut s = Session::new();
        job(&mut s);
        let before = s.execute("bim.bom", &json!({ "group_by": "level" })).unwrap();
        // Waive one required item with a reason; a waiver with no reason is refused.
        assert!(s.execute("bim.bom", &json!({ "waive": [{ "item": "Knockout set" }] })).is_err());
        let r = s.execute("bim.bom", &json!({ "waive": [{ "item": "Knockout set", "reason": "owned" }] })).unwrap();
        let ko = r["required"].as_array().unwrap().iter().find(|x| x["item"] == json!("Knockout set")).unwrap();
        assert_eq!((ko["qty"].clone(), ko["waived"].clone()), (json!(0.0), json!("owned")));
        // A revision: a receptacle removed → a changed line.
        let mut spec = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
        spec["tags"].as_array_mut().unwrap().retain(|t| t["id"] != json!("R2"));
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();
        let after = s.execute("bim.bom", &json!({ "group_by": "level", "compare_to": { "lines": before["lines"] } })).unwrap();
        let ch = &after["changes"]["changed"];
        assert!(ch.as_array().unwrap().iter().any(|c| c["line"].as_str().unwrap().contains("standard duplex") && c["delta"] == json!(-1.0)), "{ch}");
        // Waste override and grouping by trade.
        let r = s.execute("bim.bom", &json!({ "waste": { "conduit": 0 }, "group_by": "trade" })).unwrap();
        assert_eq!(line(&r, "EMT conduit")["waste"], json!(0.0));
        assert!(s.execute("bim.bom", &json!({ "group_by": "nonsense" })).is_err());
    }

    #[test]
    fn spec_requirements_without_a_line_are_flagged() {
        let mut s = Session::new();
        job(&mut s);
        let mut t = cadaddy_doc::bimspec::BimType::new("WP1");
        t.category = "receptacle".into();
        t.set_field("finish", cadaddy_doc::bimspec::SpecField { value: "WHITE".into(), source: "26 27 26 2.3".into(), ..Default::default() });
        s.doc_mut().unwrap().bim_types.types.push(t);
        let r = s.execute("bim.bom", &json!({})).unwrap();
        let issues: Vec<&str> = r["spec_check"].as_array().unwrap().iter().filter_map(|c| c["issue"].as_str()).collect();
        assert!(issues.contains(&"requirement_without_line"), "{r}");
        assert_eq!(r["complete"], json!(false));
        assert!(large_conductor("4") && large_conductor("1/0") && !large_conductor("12"));
        assert_eq!(conductor_parts("3#8 THHN + 1#10G in 3/4\" EMT"), vec![(3, "8".into(), false), (1, "10".into(), true)]);
    }
}
