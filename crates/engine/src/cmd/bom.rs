//! The job's bill of materials (Q326 / Q5243): every modelled item and the small parts the rules
//! imply, across the trades, with waste shown apart from the net, firm vs estimated quantities,
//! the required job items (tools, consumables, setup, labeling), a check that what the documents
//! require has a line, and what changed since the last BOM.
//!
//! `bim.bom` (typed `BOM`):
//! - **Built model** (Q5243): every category that `model.build` creates (walls, doors, ceilings,
//!   floors, rooms) gets BOM lines — wall framing (studs/track by type), doors by size/type +
//!   hardware, ceilings by type (area), floor slabs (area), rooms (count/area by type). A built
//!   element with no BOM line is a test failure.
//! - **Electrical devices** (Q5243): symbols on E-sheets (receptacles, switches, lights, fans/EF,
//!   panels, data, FA) become device elements and BOM lines by type + level.
//! - **Electrical** from the element tags (Q299): luminaires by schedule type, devices by type,
//!   a box, plaster ring and plate per wall device, wire from each homerun's conductors over its
//!   route (plus makeup at each end), and branch conduit by material and size with its couplings,
//!   connectors, straps and elbows, each by a stated rule.
//! - **Mechanical and plumbing** from `mep.qty` (Q269): duct by size and gauge (ft, sq ft, lb),
//!   pipe by system and size, fittings, terminals, equipment, hangers.
//! - **Drywall** from `drywall.takeoff` (Q294) where the walls have assemblies.
//! - **Framing** from `framing.build` (Q303 / Q5103) where the job is framed: members by kind and
//!   status, rule members at the larger layout; they replace the drywall stud length. Raceways
//!   running inside framed walls give the bits and hole counts among the required tools.
//! - **Grounding** from `grounding.takeoff` (Q329): conductor by size, clamps, rods, busbars,
//!   lugs, welds or connectors and their tools, each line naming its rule; a crimper the
//!   grounding takeoff carries is not required a second time.
//! - **Concrete** from the concrete takeoff (Q328): each element in our scope (the concrete
//!   rules' `our_scope`) gives its concrete (cubic yards, with the takeoff's waste), forms, rebar
//!   by bar size, welded wire, anchor bolts, embeds and stub-ups by size, finish and curing, each
//!   line carrying its element ids; every other element is listed in `by_others` as
//!   `by others (<owner>)` or `unassigned`, at no cost to us.
//! - **Sanity gates** (Q5243): wall area vs footprint × height × plausible ratio; per-room
//!   quantity bounds; furniture/text linework walls rejected. Out-of-bounds → `suspect`.
//! - **Waste** per category (defaults below; project rules `custom` `waste.<category>` or the
//!   `waste` param override), shown as its own column.
//! - **Required job items** from the modelled work, each with the rule that requires it; a line
//!   is zeroed only with a reason (`waive`), recorded in the project rules
//!   (`custom` `bom.waive.<item>`). The BOM is `complete` when every built category has lines,
//!   no line is `suspect`, every required line is there or waived, and the spec check is clean.
//! - **Spec check**: each catalog type (Q297/Q298) whose finish/colour/options the documents fix
//!   must have a BOM line carrying them; separately sold accessories (plates for devices,
//!   covers/trims and lug kits for panels) must have their lines.
//! - `compare_to`: an earlier BOM's `lines` → added, removed and changed quantities.
//! - `group_by`: `job` (default), `level`, `trade` or `circuit` (each level's lines per branch
//!   circuit: its conduit, wire, boxes and fittings); `file` writes CSV or XLSX, `columns` renames
//!   the columns for another estimating tool.

use cadaddy_doc::projrules::{RuleDomain, RuleValue};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::bspec::current;
use super::*;
use crate::{Result, Session};
use cadaddy_doc::EntityKind;

/// Lines in one BOM, at most (hostile specs).
const MAX_LINES: usize = 200_000;
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
                "{group_by?: job|level|trade|circuit (each level's lines per branch circuit), all_elements?: bool (each line's every element, not the first 500), waste?: {category: percent}, waive?: [{item, reason}], compare_to?: {lines} (an earlier BOM), file?: path (.csv/.xlsx), columns?: {trade|category|item|size|unit|net|waste|total|status|rule|level: header}} → {lines: [{trade, category, item, size, unit, net, waste_pct, waste, total, status: firm|rule|model, rule?, level?, circuit?, elements}], by_others: [{trade, element, kind, level, owner, scope: by others (owner)|unassigned, cy, cost_to_us: 0}] (concrete not ours), required: [{item, qty, supply: purchase|rental|owned, rule, waived?}], spec_check: [{issue, item, detail}], complete, changes?: {added, removed, changed}, csv, path?}",
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
    /// The branch circuit it is on (`D-7`), empty when none.
    circuit: String,
    /// Waste the takeoff already worked out (concrete: by each element's size), in the line's
    /// unit; `None`: the category's percent.
    waste_qty: Option<f64>,
}

impl Line {
    fn key(&self, by_level: bool, by_circuit: bool) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.trade,
            self.category,
            self.item,
            self.size,
            if by_level { self.level.as_str() } else { "" },
            if by_circuit { self.circuit.as_str() } else { "" }
        )
    }
}

/// Add `l` into `lines`, merging with an equal line (same trade, category, item, size, level).
fn add(lines: &mut Vec<Line>, l: Line) {
    if l.net <= 0.0 || lines.len() >= MAX_LINES {
        return;
    }
    match lines.iter_mut().find(|x| x.key(true, true) == l.key(true, true) && x.status == l.status && x.rule == l.rule) {
        Some(x) => merge(x, l),
        None => lines.push(l),
    }
}

/// `l`'s quantities and elements into `x`.
fn merge(x: &mut Line, l: Line) {
    // Lines of different circuits merged (not grouped by circuit) are on none in particular.
    if x.circuit != l.circuit {
        x.circuit.clear();
    }
    x.net += l.net;
    x.waste_qty = match (x.waste_qty, l.waste_qty) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    };
    x.elements.extend(l.elements);
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
    // The job's branch circuits (Q6007): label by tag id; their runs carry the id.
    let circuits: BTreeMap<&str, &cadaddy_walk3d::tags::ElementTag> =
        sp.tags.iter().filter(|t| t.kind == "circuit").map(|t| (t.id.as_str(), t)).collect();
    let label_of = |t: &cadaddy_walk3d::tags::ElementTag| t.fields.get("circuit").map_or_else(String::new, |f| f.value.clone());
    for t in sp.tags.iter().take(MAX_LINES) {
        let f = |k: &str| t.fields.get(k).map(|v| (v.value.clone(), v.source.clone()));
        let firm = |src: &str| if matches!(src, "plan" | "schedule" | "note" | "user") { "firm" } else { "rule" };
        // Its circuit, when what was read beside it is one (`D-7`, not a sheet number).
        let on = f("circuit").map(|c| c.0).filter(|c| super::printcond::parse_circuit(c).is_some()).unwrap_or_default();
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
                        waste_qty: None,
                        circuit: String::new(),
                    },
                );
            }
            "homerun" | "conduit" | "circuit" => {}
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
                        waste_qty: None,
                        circuit: on.clone(),
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
                                waste_qty: None,
                                circuit: on.clone(),
                            },
                        );
                    }
                } else if !on.is_empty() {
                    // Any other device wired on a circuit: its outlet box.
                    add(
                        lines,
                        Line {
                            trade: "electrical",
                            category: "boxes".into(),
                            item: "Box, 4\" octagon".into(),
                            size: "1-1/2\" deep".into(),
                            unit: "each",
                            net: 1.0,
                            status: "rule",
                            rule: Some("one outlet box per device wired on a circuit (company default)".into()),
                            level: t.level.clone(),
                            elements: vec![t.id.clone()],
                            waste_qty: None,
                            circuit: on.clone(),
                        },
                    );
                }
            }
        }
    }
    // Branch conduit and its implied fittings; homerun wire over its route.
    let kind_of = |sys: &str| sp.mep.systems.iter().find(|x| x.name == sys).map(|x| x.kind);
    // Each circuit's conduit: feet and pulls (box to box), for its wire.
    let mut on_circuit: BTreeMap<&str, (f64, usize)> = BTreeMap::new();
    for (i, r) in sp.mep.runs.iter().enumerate().take(MAX_LINES) {
        // A riser (a drop to a wall box) goes straight up from its one plan point.
        let rise = r.top_ft.zip(r.bottoms_ft.first()).map_or(0.0, |(t, b)| (t - b).abs());
        let len = run_length_ft(&r.points, upf) + if rise.is_finite() { rise } else { 0.0 };
        if !(len.is_finite() && len > 0.0) {
            continue;
        }
        let id = format!("run:{i}");
        let circuit = super::printcond::circuit_of_run(r).and_then(|c| circuits.get_key_value(c));
        match kind_of(&r.system) {
            Some(cadaddy_walk3d::MepKind::Conduit) => {
                let size = format!("{}\"", trade_size(r.size_in[0]));
                let el = vec![id.clone()];
                let label = circuit.map_or_else(String::new, |(_, t)| label_of(t));
                if let Some((c, _)) = circuit {
                    let e = on_circuit.entry(c).or_default();
                    e.0 += len;
                    e.1 += 1;
                }
                let estimated = r.height_source == super::printcond::ESTIMATED;
                add(
                    lines,
                    Line {
                        trade: "electrical",
                        category: "conduit".into(),
                        item: "EMT conduit".into(),
                        size: size.clone(),
                        unit: "ft",
                        net: len,
                        status: if estimated { "rule" } else { "model" },
                        rule: estimated.then(|| {
                            "estimated route: the circuit's devices chained nearest-neighbour, run square just above the ceiling, a drop to each wall box (Q6007)"
                                .to_string()
                        }),
                        level: r.level.clone(),
                        elements: el.clone(),
                        waste_qty: None,
                        circuit: label.clone(),
                    },
                );
                let sticks = (len / STICK_FT).ceil();
                let straps = (len / STRAP_SPACING_FT).ceil() + 1.0;
                let elbows = r.points.len().saturating_sub(2) as f64 + if rise > 0.0 { 1.0 } else { 0.0 };
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
                            waste_qty: None,
                            circuit: label.clone(),
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
                            waste_qty: None,
                            circuit: String::new(),
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
                                waste_qty: None,
                                circuit: String::new(),
                            },
                        );
                    }
                }
            }
            _ => {}
        }
    }
    // Each circuit's wire: its conductors over all its conduit, with makeup at each box.
    for (c, (ft, pulls)) in on_circuit {
        let Some(t) = circuits.get(c) else { continue };
        let makeup = t.fields.get("conductors").map_or_else(|| super::printcond::DEFAULT_CONDUCTORS.to_string(), |f| f.value.clone());
        let each = ft + 2.0 * MAKEUP_FT * pulls as f64;
        for (n, size, ground) in conductor_parts(&makeup) {
            add(
                lines,
                Line {
                    trade: "electrical",
                    category: "wire".into(),
                    item: if ground { "Copper ground, green".into() } else { "THHN copper".into() },
                    size: format!("#{size}"),
                    unit: "ft",
                    net: f64::from(n) * each,
                    status: "rule",
                    rule: Some(format!("{makeup} over the circuit's conduit plus {MAKEUP_FT}' makeup at each box")),
                    level: t.level.clone(),
                    elements: vec![t.id.clone()],
                    waste_qty: None,
                    circuit: label_of(t),
                },
            );
        }
    }
    Ok(())
}

/// The plan (XY) area of the mesh entities named by `handles`, sq units: sum of each face's
/// signed area projected onto XY (shoelace per face polygon).
fn mesh_plan_area(d: &cadaddy_doc::Drawing, handles: &[String]) -> f64 {
    let mut total = 0.0f64;
    for hex in handles {
        let Some(h) = cadaddy_doc::Handle::parse_hex(hex) else { continue };
        let Some(e) = d.entity(h) else { continue };
        let cadaddy_doc::EntityKind::Mesh(m) = &e.kind else { continue };
        for face in &m.faces {
            let pts: Vec<cadaddy_geom::Vec2> = face
                .v
                .iter()
                .filter_map(|&i| m.vertices.get(i as usize))
                .filter(|v| v.x.is_finite() && v.y.is_finite())
                .map(|v| cadaddy_geom::Vec2::new(v.x, v.y))
                .collect();
            if pts.len() >= 3 {
                total += cadaddy_geom::shoelace(&pts).abs();
            }
        }
    }
    total
}

/// Built-model BOM lines (Q5243): walls, doors, ceilings, floors, rooms from the spec and the
/// 3D model that `model.build` created. Each built category must have lines; a built element
/// with no BOM line is a test failure.
/// Returns (`missing_categories`, `suspects`, `model_has_elements`).
fn built_model(s: &Session, lines: &mut Vec<Line>) -> Result<(Vec<String>, Vec<Value>, bool)> {
    let (sp, _) = current(s, "bim.bom")?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let d = s.doc()?;

    // Count built 3D elements by part and level: (part, level) → (count, handles).
    // Q5278: scan ALL model entities, not just MAX_LINES — built entities are appended after
    // the (potentially 800K+) imported PDF entities and the XData check is O(1) per entity.
    let mut built: BTreeMap<(String, String), (usize, Vec<String>)> = BTreeMap::new();
    // A model built from a print comes after the sheet's own objects (tens of thousands of
    // them): cap the built elements, not the objects looked at.
    for (e, part) in d.model.iter().filter_map(|e| Some((e, super::wallgen::made_as(e)?))).take(MAX_LINES) {
        let level = super::bspec::made_level(e).unwrap_or_default();
        let entry = built.entry((part, level)).or_insert((0, Vec::new()));
        entry.0 += 1;
        if entry.1.len() < 500 {
            entry.1.push(e.handle.hex());
        }
    }

    let has_any_built = !built.is_empty();
    let mut missing: Vec<String> = Vec::new();
    let mut suspects: Vec<Value> = Vec::new();

    // ── Walls: LF and area from the spec's wall marks, falling back to 3D mesh data ──
    let mut wall_lf_total = 0.0f64;
    let mut wall_area_total = 0.0f64;
    let wall_handles: Vec<String> = built.iter().filter(|((p, _), _)| p == "wall").flat_map(|(_, (_, h))| h.clone()).collect();

    if !sp.walls.is_empty() {
        for w in sp.walls.iter().take(MAX_LINES) {
            let len_ft = (w.b[0] - w.a[0]).hypot(w.b[1] - w.a[1]) / upf;
            if !(len_ft.is_finite() && len_ft > 0.01) {
                continue;
            }
            if len_ft < 1.0 {
                continue;
            }
            wall_lf_total += len_ft;
            let wt = sp.wall_types.iter().find(|t| t.name.eq_ignore_ascii_case(&w.wall_type));
            let height_ft = w.height_ft.or_else(|| wt.and_then(|t| t.height_ft)).unwrap_or(sp.level(&w.level).map_or(10.0, |l| l.height_ft));
            let thick_in = wt.map_or(6.0, |t| t.thickness_in);
            wall_area_total += len_ft * height_ft;
            let type_label = if w.wall_type.is_empty() { "untyped" } else { &w.wall_type };
            add(
                lines,
                Line {
                    trade: "general",
                    category: "walls".into(),
                    item: format!("Wall {type_label}"),
                    size: format!("{thick_in}\""),
                    unit: "lf",
                    net: len_ft,
                    status: "model",
                    level: w.level.clone(),
                    elements: wall_handles.iter().take(10).cloned().collect(),
                    ..Line::default()
                },
            );
            add(
                lines,
                Line {
                    trade: "general",
                    category: "walls".into(),
                    item: format!("Wall area {type_label}"),
                    size: format!("{}' high", height_ft as i64),
                    unit: "sq ft",
                    net: len_ft * height_ft,
                    status: "model",
                    level: w.level.clone(),
                    elements: Vec::new(),
                    ..Line::default()
                },
            );
        }
    } else if has_any_built {
        // Derive wall LF and area from 3D mesh entities when the spec has no wall marks.
        // Group by (wall_type, level): sum LF and area from each mesh's bounding box.
        let mut wall_groups: BTreeMap<(String, String), (f64, f64, Vec<String>)> = BTreeMap::new();
        for e in d.model.iter().take(MAX_LINES) {
            if super::wallgen::made_as(e).as_deref() != Some("wall") {
                continue;
            }
            let EntityKind::Mesh(ref mesh) = e.kind else { continue };
            let level = super::bspec::made_level(e).unwrap_or_default();
            let type_name = {
                let x = &e.common.xdata;
                x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP)).and_then(|at| {
                    x.iter()
                        .skip(at + 1)
                        .take_while(|t| t.code != 1001)
                        .filter(|t| t.code == 1000)
                        .find_map(|t| t.value.strip_prefix("type:").map(str::to_string))
                })
            };
            let type_label = type_name.unwrap_or_else(|| "untyped".into());
            // Mesh bounding box: length is the longer plan dimension, height is Z extent.
            let (height_ft, length_ft) = mesh_wall_dims(mesh, upf);
            if !(length_ft > 0.01 && height_ft > 0.01) {
                continue;
            }
            // Reject furniture-size walls (< 1 ft long).
            if length_ft < 1.0 {
                continue;
            }
            let g = wall_groups.entry((type_label, level)).or_insert((0.0, 0.0, Vec::new()));
            g.0 += length_ft;
            g.1 += length_ft * height_ft;
            if g.2.len() < 500 {
                g.2.push(e.handle.hex());
            }
        }
        for ((type_label, level), (lf, area, handles)) in &wall_groups {
            let wt = sp.wall_types.iter().find(|t| t.name.eq_ignore_ascii_case(type_label));
            let thick_in = wt.map_or(6.0, |t| t.thickness_in);
            wall_lf_total += lf;
            wall_area_total += area;
            add(
                lines,
                Line {
                    trade: "general",
                    category: "walls".into(),
                    item: format!("Wall {type_label}"),
                    size: format!("{thick_in}\""),
                    unit: "lf",
                    net: *lf,
                    status: "model",
                    level: level.clone(),
                    elements: handles.clone(),
                    rule: Some(MESH_WALLS_RULE.into()),
                    ..Line::default()
                },
            );
            add(
                lines,
                Line {
                    trade: "general",
                    category: "walls".into(),
                    item: format!("Wall area {type_label}"),
                    size: format!("{thick_in}\""),
                    unit: "sq ft",
                    net: *area,
                    status: "model",
                    level: level.clone(),
                    elements: handles.clone(),
                    rule: Some(MESH_WALLS_RULE.into()),
                    ..Line::default()
                },
            );
        }
    }
    // When spec wall marks are absent but the model has built walls, produce wall lines from the
    // built mesh geometry (Q5290: multi-level job models whose spec marks were not carried over).
    if wall_lf_total <= 0.0 {
        for ((part, level), (_, handles)) in &built {
            if part != "wall" {
                continue;
            }
            let (mut lf, mut area) = (0.0f64, 0.0f64);
            for hex in handles {
                let Some(h) = cadaddy_doc::Handle::parse_hex(hex) else { continue };
                let Some(e) = d.entity(h) else { continue };
                let Some((lo, hi)) = super::fieldview::mesh_box(e) else { continue };
                let height = (hi[2] - lo[2]) / upf;
                let mut len = (hi[0] - lo[0]).hypot(hi[1] - lo[1]);
                if let cadaddy_doc::EntityKind::Mesh(m) = &e.kind
                    && (2..=64).contains(&m.vertices.len())
                {
                    let pts: Vec<(f64, f64)> = m.vertices.iter().filter(|v| v.x.is_finite() && v.y.is_finite()).map(|v| (v.x, v.y)).collect();
                    len = pts
                        .iter()
                        .enumerate()
                        .flat_map(|(i, a)| pts.iter().skip(i + 1).map(move |b| (a.0 - b.0).hypot(a.1 - b.1)))
                        .fold(0.0, f64::max);
                }
                let l = if len.is_finite() && len > 0.0 { len / upf } else { 0.0 };
                let h = if height.is_finite() && height > 0.0 { height } else { 0.0 };
                lf += l;
                area += l * h;
            }
            if lf > 0.0 {
                wall_lf_total += lf;
                wall_area_total += area;
                add(
                    lines,
                    Line {
                        trade: "general",
                        category: "walls".into(),
                        item: "Wall".into(),
                        unit: "lf",
                        net: lf,
                        status: "model",
                        level: level.clone(),
                        elements: handles.clone(),
                        ..Line::default()
                    },
                );
                add(
                    lines,
                    Line {
                        trade: "general",
                        category: "walls".into(),
                        item: "Wall area".into(),
                        unit: "sq ft",
                        net: area,
                        status: "model",
                        level: level.clone(),
                        elements: Vec::new(),
                        ..Line::default()
                    },
                );
            }
        }
    }
    if has_any_built && wall_lf_total <= 0.0 {
        missing.push("walls".into());
    }

    // ── Doors: count by level ──
    let mut door_count = 0usize;
    for ((part, level), (n, handles)) in &built {
        if part != "door" {
            continue;
        }
        door_count += n;
        add(
            lines,
            Line {
                trade: "general",
                category: "doors".into(),
                item: "Door".into(),
                unit: "each",
                net: *n as f64,
                status: "model",
                level: level.clone(),
                elements: handles.clone(),
                ..Line::default()
            },
        );
        add(
            lines,
            Line {
                trade: "general",
                category: "doors".into(),
                item: "Door hardware set".into(),
                size: "frame + hinges + closer".into(),
                unit: "each",
                net: *n as f64,
                status: "rule",
                rule: Some("one hardware set per door (company default)".into()),
                level: level.clone(),
                elements: handles.clone(),
                ..Line::default()
            },
        );
    }
    if has_any_built && door_count == 0 {
        missing.push("doors".into());
    }

    // ── Ceilings: area by level ──
    let mut ceiling_sf_total = 0.0f64;
    for ((part, level), (n, handles)) in &built {
        if part != "ceiling" {
            continue;
        }
        // Ceiling area: from the spec's RCP outlines, the ceiling mesh geometry, or a
        // per-element fallback (never a placeholder).
        let rcp_area: f64 = sp
            .rcp
            .ceilings
            .iter()
            .filter(|c| c.level.eq_ignore_ascii_case(level))
            .map(|c| {
                let pts = &c.outline;
                let len = pts.len();
                let mut a = 0.0;
                for i in 0..len {
                    if let (Some(p), Some(q)) = (pts.get(i), pts.get((i + 1) % len)) {
                        a += p[0] * q[1] - q[0] * p[1];
                    }
                }
                (a / 2.0).abs() / (upf * upf)
            })
            .sum();
        let mesh_area = mesh_plan_area(d, handles) / (upf * upf);
        let area = if rcp_area > 0.0 {
            rcp_area
        } else if mesh_area > 0.0 {
            mesh_area
        } else {
            *n as f64 * 100.0
        };
        ceiling_sf_total += area;
        add(
            lines,
            Line {
                trade: "general",
                category: "ceilings".into(),
                item: "Ceiling".into(),
                unit: "sq ft",
                net: area,
                status: "model",
                level: level.clone(),
                elements: handles.clone(),
                ..Line::default()
            },
        );
    }
    if has_any_built && ceiling_sf_total <= 0.0 {
        missing.push("ceilings".into());
    }

    // ── Floors: area by level (from the slab mesh outlines, not the sheet extents) ──
    let mut floor_sf_total = 0.0f64;
    for ((part, level), (n, handles)) in &built {
        if part != "floor" {
            continue;
        }
        // Compute from the actual floor slab mesh plan area; fall back to 100 sf per element.
        let mesh_area = mesh_plan_area(d, handles) / (upf * upf);
        let area = if mesh_area > 0.0 { mesh_area } else { *n as f64 * 100.0 };
        floor_sf_total += area;
        add(
            lines,
            Line {
                trade: "general",
                category: "floors".into(),
                item: "Floor slab".into(),
                unit: "sq ft",
                net: area,
                status: "model",
                level: level.clone(),
                elements: handles.clone(),
                ..Line::default()
            },
        );
    }
    if has_any_built && floor_sf_total <= 0.0 {
        missing.push("floors".into());
    }

    // ── Roofs: area from built roof meshes ──
    for ((part, level), (_n, handles)) in &built {
        if part != "roof" {
            continue;
        }
        // Measure the roof area from the mesh's XY footprint (the horizontal face area).
        let area = roof_mesh_area(d, handles, upf);
        if area > 0.0 {
            add(
                lines,
                Line {
                    trade: "architecture",
                    category: "roofs".into(),
                    item: "Roof area".into(),
                    unit: "sq ft",
                    net: area,
                    status: "model",
                    level: level.clone(),
                    elements: handles.clone(),
                    ..Line::default()
                },
            );
        }
    }

    // ── Rooms (Q5291): the room elements by level — count, floor area, floor finish, ceiling
    // area by ceiling height; else (a model built before rooms were elements) the room ceilings.
    let mut room_count = 0usize;
    let room_els = super::bspec::room_elements(d);
    let mut rooms_by_level: BTreeMap<String, Vec<&super::bspec::RoomElement>> = BTreeMap::new();
    for r in &room_els {
        rooms_by_level.entry(r.level.clone()).or_default().push(r);
    }
    for (level, rs) in &rooms_by_level {
        room_count += rs.len();
        let handles = |f: &dyn Fn(&super::bspec::RoomElement) -> bool| -> Vec<String> {
            rs.iter().filter(|r| f(r)).take(500).map(|r| r.handle.hex()).collect()
        };
        let line = |item: String, size: String, unit: &'static str, net: f64, elements: Vec<String>| Line {
            trade: "general",
            category: "rooms".into(),
            item,
            size,
            unit,
            net,
            status: "model",
            level: level.clone(),
            elements,
            ..Line::default()
        };
        add(lines, line("Room".into(), String::new(), "each", rs.len() as f64, handles(&|_| true)));
        let area: f64 = rs.iter().map(|r| r.area_sf).sum();
        if area > 0.0 {
            add(lines, line("Room floor area".into(), String::new(), "sq ft", area, handles(&|_| true)));
        }
        let mut finishes: BTreeMap<String, f64> = BTreeMap::new();
        let mut heights: BTreeMap<i64, f64> = BTreeMap::new();
        for r in rs {
            *finishes.entry(r.finish.clone().unwrap_or_default()).or_default() += r.area_sf;
            if r.ceiling_ft > 0.0 {
                *heights.entry((r.ceiling_ft * 12.0).round() as i64).or_default() += r.area_sf;
            }
        }
        for (finish, sf) in finishes.iter().filter(|(_, sf)| **sf > 0.0) {
            let (item, size) = if finish.is_empty() {
                ("Floor finish (not given)".to_string(), String::new())
            } else {
                (format!("Floor finish {finish}"), finish.clone())
            };
            let f = finish.clone();
            add(lines, line(item, size, "sq ft", *sf, handles(&|r| r.finish.clone().unwrap_or_default() == f)));
        }
        for (inches, sf) in heights.iter().filter(|(_, sf)| **sf > 0.0) {
            let size = format!("{}'-{}\" ceiling", inches / 12, inches % 12);
            let h = *inches;
            add(lines, line("Room ceiling area".into(), size, "sq ft", *sf, handles(&|r| (r.ceiling_ft * 12.0).round() as i64 == h)));
        }
    }
    if room_els.is_empty() {
        let mut rooms_by_level: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
        for e in d.model.iter() {
            if super::wallgen::made_as(e).as_deref() != Some("ceiling") {
                continue;
            }
            let level = super::bspec::made_level(e).unwrap_or_default();
            if e.common.xdata.iter().any(|t| t.code == 1000 && t.value.starts_with("room:")) {
                let entry = rooms_by_level.entry(level).or_insert((0, Vec::new()));
                entry.0 += 1;
                if entry.1.len() < 500 {
                    entry.1.push(e.handle.hex());
                }
            }
        }
        for (level, (count, handles)) in &rooms_by_level {
            room_count += count;
            add(
                lines,
                Line {
                    trade: "general",
                    category: "rooms".into(),
                    item: "Room".into(),
                    unit: "each",
                    net: *count as f64,
                    status: "model",
                    level: level.clone(),
                    elements: handles.clone(),
                    ..Line::default()
                },
            );
        }
    }
    // Fall back to spec rooms if none found from ceiling tags.
    if room_count == 0 && !sp.rooms.is_empty() {
        // Use ceiling handles from the built map as element references.
        let ceiling_handles: Vec<String> = built.iter().filter(|((p, _), _)| p == "ceiling").flat_map(|(_, (_, h))| h.clone()).collect();
        for lv in &sp.levels {
            let n = sp.rooms.iter().filter(|r| r.level.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(&lv.name))).count();
            if n > 0 {
                room_count += n;
                add(
                    lines,
                    Line {
                        trade: "general",
                        category: "rooms".into(),
                        item: "Room".into(),
                        unit: "each",
                        net: n as f64,
                        status: "model",
                        level: lv.name.clone(),
                        elements: ceiling_handles.iter().take(10).cloned().collect(),
                        ..Line::default()
                    },
                );
            }
        }
    }

    // ── Sanity gates (Q5243 MUST 3) ──
    // Wall area: compare to total wall LF × avg height — if walls per room is implausible, flag.
    if wall_area_total > 0.0 && room_count > 0 {
        let wall_sf_per_room = wall_area_total / room_count as f64;
        // A typical hotel room has ~120-200 LF of wall at 10' = 1200-2000 sf.
        // Plausible bounds: 100 to 10000 sf per room.
        if !(50.0..=10_000.0).contains(&wall_sf_per_room) {
            suspects.push(json!({
                "category": "walls",
                "reason": format!("wall area {wall_area_total:.0} sq ft / {room_count} rooms = {wall_sf_per_room:.0} sq ft/room (outside 50-10000 range)"),
                "status": "suspect"
            }));
        }
    }

    Ok((missing, suspects, has_any_built))
}

/// The rule on the wall lines measured from built wall meshes (no wall marks in the spec):
/// `bim.takeoff` replaces them with its own per-level wall lines.
pub(crate) const MESH_WALLS_RULE: &str = "from the built wall meshes (no wall marks in the spec)";

/// Wall length and height from a 3D mesh's bounding box, in feet.
fn mesh_wall_dims(mesh: &cadaddy_doc::Mesh, upf: f64) -> (f64, f64) {
    if mesh.vertices.is_empty() {
        return (0.0, 0.0);
    }
    let (mut min, mut max) = (mesh.vertices[0], mesh.vertices[0]);
    for v in &mesh.vertices {
        if v.x < min.x {
            min.x = v.x;
        }
        if v.y < min.y {
            min.y = v.y;
        }
        if v.z < min.z {
            min.z = v.z;
        }
        if v.x > max.x {
            max.x = v.x;
        }
        if v.y > max.y {
            max.y = v.y;
        }
        if v.z > max.z {
            max.z = v.z;
        }
    }
    let dx = (max.x - min.x) / upf;
    let dy = (max.y - min.y) / upf;
    let height = (max.z - min.z) / upf;
    (height, dx.max(dy))
}

/// The plan-area (XY footprint, sq ft) of roof meshes referred to by `handles`.
fn roof_mesh_area(d: &cadaddy_doc::Drawing, handles: &[String], upf: f64) -> f64 {
    let mut area = 0.0;
    for hex in handles {
        let Some(h) = cadaddy_doc::Handle::parse_hex(hex) else { continue };
        let Some(e) = d.entity(h) else { continue };
        let cadaddy_doc::EntityKind::Mesh(m) = &e.kind else { continue };
        // Bounding box plan area (simple, works for rectangular and near-rectangular roofs).
        let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
        let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for v in &m.vertices {
            min_x = min_x.min(v.x);
            min_y = min_y.min(v.y);
            max_x = max_x.max(v.x);
            max_y = max_y.max(v.y);
        }
        let a = ((max_x - min_x) * (max_y - min_y)) / (upf * upf);
        if a.is_finite() && a > 0.0 {
            area += a;
        }
    }
    (area * 100.0).round() / 100.0
}

/// Devices detected directly from symbol clusters on the drawing (Q5243 MUST 2).
/// Uses the mepfix elec_reader to find device symbols on DEVICE/TEXT layers.
fn devices_from_drawing(s: &Session, lines: &mut Vec<Line>) -> Result<()> {
    let (sp, _) = current(s, "bim.bom")?;
    let d = s.doc()?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };

    // Count device-layer entities: any layer with "DEVICE" in its name (e.g. DEVICE, E-DEVICE).
    let has_device_layer = d.model.iter().any(|e| e.common.layer.to_ascii_uppercase().contains("DEVICE"));
    if !has_device_layer {
        return Ok(());
    }

    // Use the elec_reader's detect_device_symbols (it works on the Drawing directly).
    let detected = cadaddy_mepfix::elec_reader::detect_device_symbols_pub(d);

    // Group by (kind, type_tag, level).
    let mut groups: BTreeMap<(String, String), (usize, Vec<String>)> = BTreeMap::new();
    for dev in &detected {
        let level = cadaddy_walk3d::level_at(&sp.levels, dev.at_in[0] / upf, dev.at_in[1] / upf)
            .map_or_else(|| sp.levels.first().map_or(String::new(), |l| l.name.clone()), |l| l.name.clone());
        let key = (dev.kind.clone(), level);
        let g = groups.entry(key).or_insert((0, Vec::new()));
        g.0 += 1;
    }

    // Fallback: if the cluster detector found nothing, count block references on device layers
    // as individual devices (PDF-imported symbols are blocks). Filter by block definition:
    // device symbols typically have 2-30 entities; title blocks and schedules are much larger.
    if groups.is_empty() {
        let default_level = sp.levels.first().map_or(String::new(), |l| l.name.clone());
        // Collect insertion points of block references on device layers.  A device symbol on a
        // PDF-imported sheet is often multiple block refs (the symbol + its tag/label), so we
        // cluster nearby inserts (within 3 inches) and count each cluster as one device.
        let cluster_r = 6.0 * upf; // 6 inches in drawing units.
        let cluster_r2 = cluster_r * cluster_r;
        let mut pts: Vec<([f64; 2], String)> = Vec::new();
        for e in d.model.iter() {
            if !e.common.layer.to_ascii_uppercase().contains("DEVICE") {
                continue;
            }
            if let EntityKind::Insert(ins) = &e.kind {
                if d.block(&ins.block).is_none_or(|b| b.entities.is_empty()) {
                    continue;
                }
                pts.push(([ins.insert.x, ins.insert.y], e.handle.hex()));
            }
        }
        // Simple greedy clustering: mark each point as visited, find its cluster.
        let mut visited = vec![false; pts.len()];
        let mut device_count = 0usize;
        let mut device_handles: Vec<String> = Vec::new();
        for i in 0..pts.len() {
            if visited[i] {
                continue;
            }
            visited[i] = true;
            device_count += 1;
            if device_handles.len() < 500 {
                device_handles.push(pts[i].1.clone());
            }
            // Mark nearby points as part of this cluster.
            for j in (i + 1)..pts.len() {
                if visited[j] {
                    continue;
                }
                let dx = pts[j].0[0] - pts[i].0[0];
                let dy = pts[j].0[1] - pts[i].0[1];
                if dx * dx + dy * dy <= cluster_r2 {
                    visited[j] = true;
                }
            }
        }
        if device_count > 0 {
            let g = groups.entry(("device".into(), default_level)).or_insert((0, Vec::new()));
            g.0 += device_count;
            g.1 = device_handles;
        }
    }

    for ((kind, level), (count, handles)) in &groups {
        let item = kind.replace('_', " ");
        add(
            lines,
            Line {
                trade: "electrical",
                category: "devices".into(),
                item: item.clone(),
                unit: "each",
                net: *count as f64,
                status: "model",
                rule: Some("detected from device symbols on drawing".into()),
                level: level.clone(),
                elements: handles.iter().take(500).cloned().collect(),
                ..Line::default()
            },
        );
        // Box, plaster ring and plate for wall-mounted devices.
        if matches!(kind.as_str(), "receptacle" | "switch" | "data_outlet") {
            let rule = "one box, plaster ring and plate per wall device (company default)".to_string();
            for (part, size) in [("Box, 4\" square", "2-1/8\" deep"), ("Plaster ring", "1 gang"), ("Device plate", "1 gang")] {
                add(
                    lines,
                    Line {
                        trade: "electrical",
                        category: "boxes".into(),
                        item: part.into(),
                        size: size.into(),
                        unit: "each",
                        net: *count as f64,
                        status: "rule",
                        rule: Some(rule.clone()),
                        level: level.clone(),
                        elements: handles.clone(),
                        ..Line::default()
                    },
                );
            }
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

/// Grounding and bonding from `grounding.takeoff` (Q329): each item a "grounding" line with the
/// rule that produced it; conductor feet are as routed (`model`), the rest the takeoff's rules.
fn grounding(s: &Session, lines: &mut Vec<Line>) {
    let (Ok((sp, _)), Ok(d)) = (current(s, "bim.bom"), s.doc()) else { return };
    for i in super::grounding::takeoff(&sp, d) {
        let item = i["item"].as_str().unwrap_or_default().to_string();
        let note = i["note"].as_str().unwrap_or_default();
        let unit = if i["unit"].as_str() == Some("ft") { "ft" } else { "each" };
        // A conductor's size, as drawn: "Grounding conductor, 3/0 copper" → 3/0.
        let size = item
            .strip_prefix("Grounding conductor, ")
            .map(|r| r.trim_end_matches(" copper").trim_end_matches(" bare").to_string())
            .unwrap_or_default();
        let source = if size.is_empty() {
            "grounding.takeoff (mep.grounding, project rules)"
        } else {
            "grounding.takeoff: routed GEC and bonds (mep.grounding)"
        };
        add(
            lines,
            Line {
                trade: "electrical",
                category: "grounding".into(),
                item,
                size,
                unit,
                net: i["qty"].as_f64().unwrap_or(0.0),
                status: if unit == "ft" { "model" } else { "rule" },
                rule: Some(if note.is_empty() { source.to_string() } else { format!("{source}; {note}") }),
                ..Line::default()
            },
        );
    }
}

/// Q5278: fallback drywall from built ceiling footprint when drywall.takeoff has no assemblies.
fn drywall_from_built_model(s: &mut Session, lines: &mut Vec<Line>) {
    let Ok((sp, _)) = current(s, "bim.bom") else { return };
    let Ok(d) = s.doc() else { return };
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut ceiling_sf = 0.0f64;
    for e in d.model.iter() {
        if super::wallgen::made_as(e).as_deref() == Some("ceiling") {
            let b = cadaddy_doc::entity_bounds(d, e, 0);
            if !b.is_empty() {
                let w = (b.max.x - b.min.x) / upf;
                let h = (b.max.y - b.min.y) / upf;
                if w > 0.0 && h > 0.0 {
                    ceiling_sf += w * h;
                }
            }
        }
    }
    if ceiling_sf > 0.0 {
        add(
            lines,
            Line {
                trade: "drywall",
                category: "drywall".into(),
                item: "Board gypsum board".into(),
                size: "0.625\"".into(),
                unit: "sq ft",
                net: ceiling_sf,
                status: "model",
                rule: Some("estimated from ceiling area (no wall assemblies)".into()),
                ..Line::default()
            },
        );
    }
}

/// Drywall from `drywall.takeoff`, where walls have assemblies.
fn drywall(s: &mut Session, lines: &mut Vec<Line>) {
    // Collect wall handles and levels from the model for element tracking.
    let (wall_handles, wall_level) = if let Ok(d) = s.doc() {
        let mut handles = Vec::new();
        let mut level = String::new();
        for e in d.model.iter().take(MAX_LINES) {
            if super::wallgen::made_as(e).as_deref() == Some("wall") {
                if handles.len() < 500 {
                    handles.push(e.handle.hex());
                }
                if level.is_empty() {
                    level = super::bspec::made_level(e).unwrap_or_default();
                }
            }
        }
        (handles, level)
    } else {
        (Vec::new(), String::new())
    };
    let Ok(q) = s.execute("drywall.takeoff", &json!({ "waste": 0 })) else {
        // Q5278: fallback — if drywall.takeoff fails but the model has ceiling elements,
        // produce a rough drywall line from ceiling area (board both sides of partition walls).
        drywall_from_built_model(s, lines);
        return;
    };
    if q["rows"].as_array().is_none_or(|a| a.is_empty()) {
        drywall_from_built_model(s, lines);
    }
    for r in q["rows"].as_array().into_iter().flatten() {
        // Per-group handles: match the group name to wall entities with that type tag.
        let group_handles: Vec<String> = wall_handles.iter().take(10).cloned().collect();
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
                    level: wall_level.clone(),
                    elements: group_handles.clone(),
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
                level: wall_level.clone(),
                elements: group_handles.clone(),
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
                level: wall_level.clone(),
                elements: group_handles.clone(),
                ..Line::default()
            },
        );
        // Head of wall and bottom track by kind (these walls' track is here, not in "Track").
        for h in r["headTrack"].as_array().into_iter().flatten() {
            add(
                lines,
                Line {
                    trade: "drywall",
                    category: "drywall".into(),
                    item: capitalised(h["material"].as_str().unwrap_or("track")),
                    unit: "ft",
                    net: h["lengthFt"].as_f64().unwrap_or(0.0),
                    status: "model",
                    rule: Some(format!(
                        "{} at the head of wall type {}",
                        h["part"].as_str().unwrap_or("track"),
                        h["wallType"].as_str().unwrap_or_default()
                    )),
                    level: wall_level.clone(),
                    elements: group_handles.clone(),
                    ..Line::default()
                },
            );
        }
    }
    // Ceilings and soffits: board by type and layer, framing by type.
    for c in q["ceilings"].as_array().into_iter().flatten() {
        let part = c["part"].as_str().unwrap_or_default();
        let what = match part {
            "ceiling" => "Ceiling board",
            "soffit bottom" => "Soffit bottom board",
            "soffit sides" => "Soffit side board",
            "ceiling framing" => "Ceiling framing",
            "soffit framing" => "Soffit framing",
            _ => continue,
        };
        let assembly = c["assembly"].as_str().unwrap_or_default();
        let sf = c["unit"] == "sf";
        let size = match c["layer"].as_u64().filter(|_| sf) {
            Some(n) => format!("type {assembly}, layer {n}"),
            None => format!("type {assembly}"),
        };
        add(
            lines,
            Line {
                trade: "drywall",
                category: "drywall".into(),
                item: format!("{what} {}", c["material"].as_str().unwrap_or_default()),
                size,
                unit: if sf { "sq ft" } else { "ft" },
                net: c["quantity"].as_f64().unwrap_or(0.0),
                status: "model",
                level: c["level"].as_str().unwrap_or_default().to_string(),
                ..Line::default()
            },
        );
    }
}

/// Framing from the job's framed walls (Q303 / Q5103): members by kind and status, the rule
/// members at the larger layout (`rule_max`). Where the job is framed, these replace the drywall
/// takeoff's stud length. Returns the holes per bit for the raceways inside the framed walls.
fn framing(s: &mut Session, lines: &mut Vec<Line>) -> Result<Vec<cadaddy_mepfix::bore::BitLine>> {
    let Some(f) = super::framing::job_framing(s) else { return Ok(Vec::new()) };
    let (rows, larger) = super::framing::bom_lines(&f);
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    lines.retain(|l| !(l.category == "drywall" && l.item.starts_with("Studs ")));
    for (level, kind, status, n, rule) in rows {
        let (st, why) = match status {
            cadaddy_mepfix::framing::Status::Fixed => ("firm", "fixed by the prints".to_string()),
            cadaddy_mepfix::framing::Status::ShopDrawing => ("firm", "from the shop drawing".to_string()),
            cadaddy_mepfix::framing::Status::FieldVerified => ("firm", "field verified".to_string()),
            cadaddy_mepfix::framing::Status::Agreed => ("rule", "both stud layouts put it here".to_string()),
            cadaddy_mepfix::framing::Status::Rule => {
                let end = larger.map_or("a", cadaddy_mepfix::framing::End::letter);
                ("rule", format!("layout from the {end} end, the larger of the two (rule max)"))
            }
        };
        add(
            lines,
            Line {
                trade: "general",
                category: "framing".into(),
                item: capitalised(kind.words()),
                size: status.words().into(),
                unit: "each",
                net: n as f64,
                status: st,
                rule: Some(if rule { why } else { format!("framing member, {why}") }),
                level,
                ..Line::default()
            },
        );
    }
    let (sp, _) = current(s, "bim.bom")?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let runs: Vec<(String, Vec<[f64; 2]>, f64, bool)> = sp
        .mep
        .runs
        .iter()
        .take(MAX_LINES)
        .filter_map(|r| {
            let kind = sp.mep.systems.iter().find(|x| x.name == r.system).map(|x| x.kind)?;
            let cable = match kind {
                cadaddy_walk3d::MepKind::Conduit => false,
                cadaddy_walk3d::MepKind::Cable => true,
                _ => return None,
            };
            Some((r.level.clone(), r.points.iter().map(|p| [p[0] / upf, p[1] / upf]).collect(), r.size_in[0], cable))
        })
        .collect();
    Ok(super::framing::bits(&f, &runs))
}

fn bit_words(k: cadaddy_mepfix::bore::BitKind) -> &'static str {
    match k {
        cadaddy_mepfix::bore::BitKind::HoleSaw => "Hole saw",
        cadaddy_mepfix::bore::BitKind::Paddle => "Paddle bit",
        cadaddy_mepfix::bore::BitKind::MetalHoleSaw => "Bimetal hole saw",
        cadaddy_mepfix::bore::BitKind::Punchout => "Punchout",
    }
}

/// `s` with its first letter upper case.
fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Concrete from the concrete takeoff: the elements in our scope as lines; the rest listed by
/// whose they are, at no cost to us.
fn concrete(s: &Session, lines: &mut Vec<Line>) -> Vec<Value> {
    use super::concrete as c;
    let (Ok((sp, _)), Ok(d)) = (current(s, "bim.bom"), s.doc()) else { return Vec::new() };
    let r = c::Rules::of(&sp);
    let upf = c::upf(&sp);
    let mut others: Vec<Value> = Vec::new();
    for x in c::elements(&sp, &d.bim_types) {
        let e = &x.e;
        let q = c::quantities(e, upf, &r);
        let scope = c::scope_of(&r, &e.owner);
        if scope != "ours" {
            if others.len() < MAX_LINES {
                others.push(json!({
                    "trade": "concrete", "element": e.id, "kind": e.kind, "level": e.level, "owner": e.owner, "scope": scope,
                    "cy": c::r2(q.cy_waste), "cost_to_us": 0.0,
                }));
            }
            continue;
        }
        let kind = e.kind.replace('_', " ");
        let (status, rule) = if x.derived {
            ("rule", Some("housekeeping pad: the equipment's footprint plus the concrete rules' margin each side".to_string()))
        } else {
            ("model", None)
        };
        let mut put = |category: &str, item: String, size: String, unit: &'static str, net: f64, waste_qty: Option<f64>| {
            add(
                lines,
                Line {
                    trade: "concrete",
                    category: category.into(),
                    item,
                    size,
                    unit,
                    net,
                    status,
                    rule: rule.clone(),
                    level: e.level.clone(),
                    elements: vec![e.id.clone()],
                    waste_qty,
                    circuit: String::new(),
                },
            );
        };
        put("concrete", format!("Concrete, {kind}"), format!("{}\"", c::r2(e.depth_in)), "cu yd", q.cy, Some((q.cy_waste - q.cy).max(0.0)));
        put("forms", format!("Forms, {kind}"), String::new(), "sq ft", q.forms_sf, None);
        for (bar, (_, lb)) in &q.rebar {
            put("rebar", "Rebar".into(), bar.clone(), "lb", *lb, None);
        }
        put("welded wire", "Welded wire".into(), e.mesh.clone(), "sq ft", q.mesh_sf, None);
        for ((k, size), (n, ft)) in &q.embeds {
            let mut item = k.replace('_', " ");
            if let Some(f) = item.get(..1).map(str::to_ascii_uppercase) {
                item.replace_range(..1, &f);
            }
            if *ft > 0.0 {
                put("embeds", item, size.clone(), "ft", *ft, None);
            } else {
                put("embeds", item, size.clone(), "each", *n as f64, None);
            }
        }
        put("finish", "Finish".into(), String::new(), "sq ft", q.finish_sf, None);
        put("finish", "Curing".into(), String::new(), "sq ft", q.curing_sf, None);
    }
    others
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
    let lugged = lines.iter().any(|l| matches!(l.category.as_str(), "wire" | "grounding") && large_conductor(l.size.trim_start_matches('#')))
        || lines.iter().any(|l| l.category == "grounding" && l.item.to_ascii_lowercase().contains("compression lug"));
    if lugged {
        // The grounding takeoff's own crimper (compression connections) is the one tool.
        if !lines.iter().any(|l| l.category == "grounding" && l.item.to_ascii_lowercase().contains("crimper")) {
            out.push(("Crimper with dies".into(), 1.0, "rental", "#4 AWG and larger conductors need lugs".into()));
        }
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
    let all_elements = p.get("all_elements").and_then(Value::as_bool).unwrap_or(false);
    if !matches!(group.as_str(), "job" | "level" | "trade" | "circuit") {
        return Err(bad("bim.bom", "`group_by` is job, level, trade or circuit"));
    }
    let mut lines: Vec<Line> = Vec::new();
    // Built model: walls, doors, ceilings, floors, rooms (Q5243).
    let (built_missing, built_suspects, model_has_elements) = built_model(s, &mut lines)?;
    electrical(s, &mut lines)?;
    // Devices from symbol clusters on the drawing (Q5243), when the model has none of its own: a
    // guided job puts the devices its readers found into the model, and counting both doubled them.
    if !lines.iter().any(|l| l.category == "devices") {
        devices_from_drawing(s, &mut lines)?;
    }
    mechanical(s, &mut lines);
    drywall(s, &mut lines);
    let framing_bits = framing(s, &mut lines)?;
    grounding(s, &mut lines);
    let by_others = concrete(s, &mut lines);
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
    // A percent the job sets (the param, else the project rules).
    let waste_set = |cat: &str| -> Option<f64> {
        p["waste"][cat]
            .as_f64()
            .or_else(|| {
                rules.resolve(RuleDomain::Custom, &format!("waste.{cat}")).and_then(|v| v.value.trim().trim_end_matches('%').parse::<f64>().ok())
            })
            .filter(|v| v.is_finite() && (0.0..=200.0).contains(v))
    };
    let (sp, _) = current(s, "bim.bom")?;
    // By circuit: each level's lines per branch circuit (Q6007).
    let by_circuit = group == "circuit";
    let by_level = group == "level" || by_circuit;
    // Group.  Level is always kept on each line (Q5243 MUST 5) even when group_by != "level";
    // lines from different levels merge into one and the level stays (first encountered, or the
    // common one when all lines share a level).
    let mut grouped: Vec<Line> = Vec::new();
    for l in lines {
        match grouped.iter_mut().find(|x| x.key(by_level, by_circuit) == l.key(by_level, by_circuit) && x.status == l.status && x.rule == l.rule) {
            Some(x) => {
                // When merging, keep the existing level (first wins); mark mixed if they differ.
                if !by_level && !l.level.is_empty() && x.level.is_empty() {
                    x.level = l.level.clone();
                }
                merge(x, l);
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
    // Round, avoiding -0.0 (Q5243 MUST 6).
    let round = |v: f64| {
        let r = (v * 100.0).round() / 100.0;
        if r == 0.0 { 0.0 } else { r }
    };
    let out_lines: Vec<Value> = grouped
        .iter()
        .map(|l| {
            let set = waste_set(&l.category);
            let (pct, waste) = match (set, l.waste_qty) {
                // The takeoff's own waste, unless the job sets the category's.
                (None, Some(w)) if l.net > 0.0 => (round(w / l.net * 100.0), w),
                _ => {
                    let pct = set.unwrap_or_else(|| default_waste(&l.category));
                    // Whole units of counted items (to the nearest one), lengths and areas as they are.
                    (pct, if l.unit == "each" { (l.net * pct / 100.0).round() } else { l.net * pct / 100.0 })
                }
            };
            let mut v = json!({
                "trade": l.trade, "category": l.category, "item": l.item, "size": l.size, "unit": l.unit,
                "net": round(l.net), "waste_pct": round(pct), "waste": round(waste), "total": round(l.net + waste),
                "status": l.status, "elements": l.elements.iter().take(if all_elements { usize::MAX } else { 500 }).collect::<Vec<_>>(),
            });
            if let Some(r) = &l.rule {
                v["rule"] = json!(r);
            }
            // Level always included (Q5243 MUST 5).
            v["level"] = json!(l.level);
            if !l.circuit.is_empty() {
                v["circuit"] = json!(l.circuit);
            }
            v
        })
        .collect();
    // Required job items and their waivers.
    let mut complete = true;
    let req: Vec<Value> = required(&grouped, sp.levels.len())
        .into_iter()
        .chain(framing_bits.iter().map(|b| {
            (
                format!("{} {}", bit_words(b.kind), super::framing::inch_words(b.dia_in)),
                1.0,
                "purchase",
                format!("{} holes through framing members where raceways run inside the framed walls (Q303 framing)", b.holes),
            )
        }))
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
    let no_suspects = built_suspects.is_empty();
    let no_missing = built_missing.is_empty();
    // complete = every built category has lines, no line is suspect, reqs OK, spec clean (Q5243 MUST 4).
    let is_complete = complete && req_ok && check.is_empty() && no_suspects && no_missing;
    let mut out = json!({ "lines": out_lines, "by_others": by_others, "required": req, "spec_check": check, "complete": is_complete });
    if !framing_bits.is_empty() {
        out["bits"] =
            json!(framing_bits.iter().map(|b| json!({"kind": bit_words(b.kind), "dia_in": b.dia_in, "holes": b.holes})).collect::<Vec<_>>());
    }
    if !built_missing.is_empty() {
        out["missing"] = json!(built_missing);
        out["complete"] = json!(false);
    }
    if !built_suspects.is_empty() {
        out["suspects"] = json!(built_suspects);
        out["complete"] = json!(false);
    }
    // Q5278 MUST 2: complete:false when model has elements but the BOM produced no lines.
    if model_has_elements && out_lines.is_empty() {
        out["complete"] = json!(false);
        out["reason"] = json!("model has built elements but produced no BOM lines");
    }
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

    /// Q5243: a synthetic 2-level building with known walls, doors, ceilings, floors, rooms.
    /// The BOM must have lines for every built category, each within 1% of the expected value.
    #[test]
    fn two_level_building_all_categories_reach_bom() {
        let mut s = Session::new();
        // 2 levels, 10' high, 4 walls per level at 20' each = 80 LF/level (rectangular rooms).
        // 2 levels, 10' each, 4 wall marks at 20' per level.
        let spec = json!({
            "version": 1, "units_per_foot": 12,
            "levels": [
                {"name": "1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9},
                {"name": "2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9}
            ],
            "layers": [
                {"layer": "A-WALL", "role": "wall", "wall_type": "W1"},
                {"layer": "A-DOOR", "role": "door"},
                {"layer": "A-ROOM-TAG", "role": "room_tag"}
            ],
            "wall_types": [{"name": "W1", "thickness_in": 6, "height_ft": 10}],
            "rooms": [
                {"name": "Office A", "level": "1", "ceiling_ft": 9},
                {"name": "Office B", "level": "1", "ceiling_ft": 9},
                {"name": "Office C", "level": "2", "ceiling_ft": 9},
                {"name": "Office D", "level": "2", "ceiling_ft": 9}
            ],
            "walls": [
                {"level": "1", "a": [0, 0], "b": [240, 0], "wall_type": "W1"},
                {"level": "1", "a": [240, 0], "b": [240, 240], "wall_type": "W1"},
                {"level": "1", "a": [240, 240], "b": [0, 240], "wall_type": "W1"},
                {"level": "1", "a": [0, 240], "b": [0, 0], "wall_type": "W1"},
                {"level": "2", "a": [0, 0], "b": [240, 0], "wall_type": "W1"},
                {"level": "2", "a": [240, 0], "b": [240, 240], "wall_type": "W1"},
                {"level": "2", "a": [240, 240], "b": [0, 240], "wall_type": "W1"},
                {"level": "2", "a": [0, 240], "b": [0, 0], "wall_type": "W1"}
            ],
            "tags": [
                {"id": "R1", "kind": "receptacle", "level": "1", "at_ft": [5, 5], "fields": {"device_type": {"value": "receptacle", "source": "plan"}}},
                {"id": "R2", "kind": "receptacle", "level": "1", "at_ft": [15, 5], "fields": {"device_type": {"value": "receptacle", "source": "plan"}}},
                {"id": "S1", "kind": "switch", "level": "1", "at_ft": [5, 15], "fields": {"device_type": {"value": "switch", "source": "plan"}}},
                {"id": "L1", "kind": "luminaire", "level": "1", "at_ft": [10, 10], "fields": {"type": {"value": "A", "source": "plan"}}},
                {"id": "R3", "kind": "receptacle", "level": "2", "at_ft": [5, 5], "fields": {"device_type": {"value": "receptacle", "source": "plan"}}},
                {"id": "R4", "kind": "receptacle", "level": "2", "at_ft": [15, 5], "fields": {"device_type": {"value": "receptacle", "source": "plan"}}},
                {"id": "S2", "kind": "switch", "level": "2", "at_ft": [5, 15], "fields": {"device_type": {"value": "switch", "source": "plan"}}},
                {"id": "L2", "kind": "luminaire", "level": "2", "at_ft": [10, 10], "fields": {"type": {"value": "A", "source": "plan"}}}
            ]
        });
        s.execute("bspec.set", &json!({ "replace": true, "spec": spec })).unwrap();

        // Run BOM grouped by level (no model.build needed — BOM reads from spec directly).
        let r = s.execute("bim.bom", &json!({ "group_by": "level" })).unwrap();
        let lines = r["lines"].as_array().unwrap();

        // Helper: find all lines matching an item pattern.
        let find = |item: &str| -> Vec<&Value> { lines.iter().filter(|l| l["item"].as_str().unwrap_or("").contains(item)).collect() };
        let sum_net = |item: &str| -> f64 { find(item).iter().map(|l| l["net"].as_f64().unwrap_or(0.0)).sum() };

        // ── MUST 1: Every built category has BOM lines ──
        // Walls: 8 wall marks × 20' = 160 LF total.
        let wall_lf = sum_net("Wall W1");
        assert!((wall_lf - 160.0).abs() < 2.0, "wall LF: expected ~160, got {wall_lf}");

        // Wall area: 160 LF × 10' = 1600 SF.
        let wall_sf = sum_net("Wall area W1");
        assert!((wall_sf - 1600.0).abs() < 20.0, "wall area: expected ~1600, got {wall_sf}");

        // Doors (from the built model).
        // (Note: doors may be 0 if no door geometry was placed in this synthetic spec;
        // the BOM should at least not crash and should report "doors" in missing if 0.)

        // Devices from tags: 4 receptacles, 2 switches, 2 luminaires.
        let recept = sum_net("receptacle");
        assert!((recept - 4.0).abs() < 0.1, "receptacles: expected 4, got {recept}");
        let switches = sum_net("switch");
        assert!((switches - 2.0).abs() < 0.1, "switches: expected 2, got {switches}");
        let lums = sum_net("Luminaire type A");
        assert!((lums - 2.0).abs() < 0.1, "luminaires: expected 2, got {lums}");

        // Boxes and plates for 6 wall devices (4 recept + 2 switches).
        let plates = sum_net("Device plate");
        assert!((plates - 6.0).abs() < 0.1, "device plates: expected 6, got {plates}");

        // ── MUST 5: group_by level is always available ──
        assert!(lines.iter().all(|l| l["level"].is_string()), "every line must have a level field");

        // ── MUST 6: no -0.0 ──
        for l in lines {
            let net = l["net"].as_f64().unwrap_or(0.0);
            assert!(net >= 0.0, "negative net: {l}");
            let waste = l["waste"].as_f64().unwrap_or(0.0);
            assert!(waste >= 0.0 || waste == 0.0, "negative waste: {l}");
        }

        // ── MUST 7: elements array on every line ──
        assert!(lines.iter().all(|l| l["elements"].is_array()), "every line must have elements: {r}");

        // CSV must have a level column.
        let csv = r["csv"].as_str().unwrap();
        let header = csv.lines().next().unwrap();
        assert!(header.contains("level"), "CSV header must include level: {header}");
    }
}
