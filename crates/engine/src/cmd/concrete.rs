//! Concrete in the BIM: slabs, footings, grade beams, piers and pole bases, equipment
//! housekeeping pads, ductbank encasement and handhole bases (`concrete` in the building spec),
//! with their quantities, the pad coordination for floor- and grade-mounted equipment and the
//! hold points of each pour.
//!
//! - **Pads (coordination):** every floor- or grade-mounted piece of equipment (switchgear and
//!   switchboards, transformers, generators, transfer switches, floor-standing panels, on-grade
//!   rooftop units and condensers, water heaters, pumps…) needs a pad decision: required or not,
//!   why (a spec section, a detail, the utility or the manufacturer) and whose scope it is.
//!   Equipment with no decision, or a pad nobody owns, is a failure and an RFI at review. A pad
//!   is sized from the approved submittal's footprint (the BIM type's `dimensions`, else the
//!   plan box) plus the rules' margin each side, so it grows when the submittal does; a pad
//!   fixed by a detail is checked against the footprint. The working space in front of gear is
//!   checked for other pads and equipment.
//! - **Quantities:** volume (cubic yards, with waste), forms (edge and side area, linear feet,
//!   round forms, chamfer strip), rebar by bar size (with laps, by weight), welded wire,
//!   dowels, anchor bolts and embeds, conduit stub-ups, finish and curing area. On the MEP
//!   takeoff (`mep.qty`) and the job BOM (`bim.bom`) when the element is in our scope (the
//!   rules' `our_scope`, electrical by default); listed as by others otherwise, so nobody
//!   assumes it.
//! - **Hold points:** for every pour, each item that must be in or under it before concrete —
//!   the concrete-encased electrode, under-slab conduit and stub-ups, sleeves, anchor bolts,
//!   embeds — as a checklist to sign off; warned ahead of the pour date, and a pour placed
//!   without one says what replaces it and roughly what that costs (a missed encased electrode:
//!   a ground ring or rods). The encased electrode is an `ufer` embed or the grounding system's
//!   concrete-encased electrode (Q329), on the checklist of the foundation it names or sits in;
//!   documented as installed it stands signed off, and poured undocumented it is missed.
//! - **Rebar and embeds:** conduits, sleeves and the electrode against the element's cover and
//!   its rebar mats, anchor bolts against the depth, items outside the element.
//!
//! The tables (bar weights, covers, laps, margins, waste, unit costs of the alternatives) are
//! data with defaults; a project gives its own in `concrete.rules`. The default costs are rough
//! placeholders to rank what a miss costs, not an estimate.

use std::collections::BTreeMap;

use cadaddy_doc::bimspec::BimTypes;
use cadaddy_walk3d::{BuildingSpec, ConcreteElement, Electrode, Embed, MAX_CONCRETE, MepEquipment, MepKind, PadDecision, Pour, Rebar, Signoff};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// Findings and hold points a check returns, at most.
const MAX_FINDINGS: usize = 10_000;
/// Bars, ties or pieces counted per rebar row, at most (hostile sizes).
const MAX_BARS: f64 = 1.0e6;

// ---------------- rules ----------------

/// Reinforcing bars: size, diameter (inches), weight (pounds per foot).
const BARS: [(&str, f64, f64); 11] = [
    ("#3", 0.375, 0.376),
    ("#4", 0.5, 0.668),
    ("#5", 0.625, 1.043),
    ("#6", 0.75, 1.502),
    ("#7", 0.875, 2.044),
    ("#8", 1.0, 2.670),
    ("#9", 1.128, 3.400),
    ("#10", 1.27, 4.303),
    ("#11", 1.41, 5.313),
    ("#14", 1.693, 7.650),
    ("#18", 2.257, 13.600),
];

/// The tables and defaults the concrete is sized, counted and checked with.
#[derive(Clone, Debug, PartialEq)]
pub struct Rules {
    /// Pad beyond the equipment on each side, inches, and the least a fixed pad may give.
    pub pad_margin_in: f64,
    pub pad_margin_min_in: f64,
    pub pad_height_in: f64,
    pub pad_chamfer_in: f64,
    /// A pad's reinforcing when its decision gives none.
    pub pad_rebar: Vec<Rebar>,
    /// Anchor bolts per pad (0: none unless the decision gives them), their diameter, inches.
    pub pad_anchor_bolts: u32,
    pub pad_anchor_in: f64,
    /// Waste on concrete, percent; a pour under one cubic yard takes `small_waste_pct`.
    pub waste_pct: f64,
    pub small_waste_pct: f64,
    /// Lap on welded wire, percent of the area.
    pub mesh_lap_pct: f64,
    /// Clear cover by element kind, inches.
    pub cover_in: BTreeMap<String, f64>,
    /// Lap splice in bar diameters, and the stock bar length, feet.
    pub lap_db: f64,
    pub stock_ft: f64,
    /// Hooks on ties, in bar diameters each end.
    pub hook_db: f64,
    /// The concrete-encased electrode: required when the building has footings, its least
    /// length (feet) and concrete round it (inches).
    pub ufer_required: bool,
    pub ufer_min_ft: f64,
    pub ufer_cover_in: f64,
    /// Working space kept clear in front of gear, inches.
    pub clearance_in: f64,
    /// Days ahead of a pour its open hold points are warned.
    pub warn_days: i64,
    /// Owners whose concrete is on our takeoff.
    pub our_scope: Vec<String>,
    /// Unit costs of what replaces a missed item: ground ring per foot, ground rod, post-installed
    /// anchor, core drill, saw-cut and trench per foot, a surface plate, an exothermic bond.
    pub costs: BTreeMap<String, f64>,
}

impl Default for Rules {
    fn default() -> Self {
        let cover = [
            ("slab", 1.5),
            ("footing", 3.0),
            ("grade_beam", 3.0),
            ("pier", 3.0),
            ("pole_base", 3.0),
            ("encasement", 3.0),
            ("pad", 1.5),
            ("handhole_base", 1.5),
        ];
        let costs = [
            ("ground_ring_per_ft", 35.0),
            ("ground_rod", 450.0),
            ("adhesive_anchor", 90.0),
            ("core_drill", 300.0),
            ("sawcut_trench_per_ft", 65.0),
            ("surface_plate", 400.0),
            ("exothermic_bond", 250.0),
        ];
        Rules {
            pad_margin_in: 4.0,
            pad_margin_min_in: 3.0,
            pad_height_in: 4.0,
            pad_chamfer_in: 0.75,
            pad_rebar: vec![Rebar { bar: "#4".into(), direction: "each_way".into(), spacing_in: 12.0, ..Rebar::default() }],
            pad_anchor_bolts: 4,
            pad_anchor_in: 0.5,
            waste_pct: 5.0,
            small_waste_pct: 10.0,
            mesh_lap_pct: 10.0,
            cover_in: cover.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            lap_db: 40.0,
            stock_ft: 20.0,
            hook_db: 6.0,
            ufer_required: true,
            ufer_min_ft: 20.0,
            ufer_cover_in: 2.0,
            clearance_in: 36.0,
            warn_days: 7,
            our_scope: vec!["electrical".into()],
            costs: costs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }
}

/// A finite, non-negative number from a project's rules, capped.
fn num(v: Option<&Value>, max: f64) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0 && *x <= max)
}

impl Rules {
    /// The defaults with the project's `concrete.rules` over them (unknown keys and bad values
    /// are ignored: the default stands).
    pub fn of(sp: &BuildingSpec) -> Rules {
        let mut r = Rules::default();
        let Some(p) = sp.concrete.rules.as_ref() else { return r };
        let set = |field: &mut f64, key: &str, max: f64| {
            if let Some(x) = num(p.get(key), max) {
                *field = x;
            }
        };
        set(&mut r.pad_margin_in, "pad_margin_in", 120.0);
        set(&mut r.pad_margin_min_in, "pad_margin_min_in", 120.0);
        set(&mut r.pad_height_in, "pad_height_in", 120.0);
        set(&mut r.pad_chamfer_in, "pad_chamfer_in", 12.0);
        set(&mut r.pad_anchor_in, "pad_anchor_in", 12.0);
        set(&mut r.waste_pct, "waste_pct", 100.0);
        set(&mut r.small_waste_pct, "small_waste_pct", 100.0);
        set(&mut r.mesh_lap_pct, "mesh_lap_pct", 100.0);
        set(&mut r.lap_db, "lap_db", 200.0);
        set(&mut r.hook_db, "hook_db", 50.0);
        set(&mut r.ufer_min_ft, "ufer_min_ft", 1000.0);
        set(&mut r.ufer_cover_in, "ufer_cover_in", 24.0);
        set(&mut r.clearance_in, "clearance_in", 240.0);
        if let Some(x) = num(p.get("stock_ft"), 200.0).filter(|x| *x >= 1.0) {
            r.stock_ft = x;
        }
        if let Some(x) = num(p.get("pad_anchor_bolts"), 1000.0) {
            r.pad_anchor_bolts = x as u32;
        }
        if let Some(x) = num(p.get("warn_days"), 3650.0) {
            r.warn_days = x as i64;
        }
        if let Some(b) = p.get("ufer_required").and_then(Value::as_bool) {
            r.ufer_required = b;
        }
        if let Some(v) = p.get("pad_rebar")
            && let Ok(bars) = serde_json::from_value::<Vec<Rebar>>(v.clone())
        {
            r.pad_rebar = bars.into_iter().take(32).collect();
        }
        if let Some(o) = p.get("cover_in").and_then(Value::as_object) {
            for (k, v) in o.iter().take(64) {
                if let Some(x) = num(Some(v), 24.0) {
                    r.cover_in.insert(k.to_ascii_lowercase(), x);
                }
            }
        }
        if let Some(o) = p.get("costs").and_then(Value::as_object) {
            for (k, v) in o.iter().take(64) {
                if let Some(x) = num(Some(v), 1.0e9) {
                    r.costs.insert(k.clone(), x);
                }
            }
        }
        if let Some(a) = p.get("our_scope").and_then(Value::as_array) {
            r.our_scope = a.iter().take(32).filter_map(Value::as_str).map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect();
        }
        r
    }

    fn cover(&self, kind: &str) -> f64 {
        self.cover_in.get(kind).copied().unwrap_or(1.5)
    }

    fn cost(&self, key: &str) -> f64 {
        self.costs.get(key).copied().unwrap_or(0.0)
    }

    fn ours(&self, owner: &str) -> bool {
        let o = owner.trim();
        !o.is_empty() && self.our_scope.iter().any(|s| s.eq_ignore_ascii_case(o))
    }

    fn json(&self) -> Value {
        json!({
            "pad_margin_in": self.pad_margin_in, "pad_margin_min_in": self.pad_margin_min_in, "pad_height_in": self.pad_height_in,
            "pad_chamfer_in": self.pad_chamfer_in, "pad_rebar": self.pad_rebar, "pad_anchor_bolts": self.pad_anchor_bolts,
            "pad_anchor_in": self.pad_anchor_in, "waste_pct": self.waste_pct, "small_waste_pct": self.small_waste_pct,
            "mesh_lap_pct": self.mesh_lap_pct, "cover_in": self.cover_in, "lap_db": self.lap_db, "stock_ft": self.stock_ft,
            "hook_db": self.hook_db, "ufer_required": self.ufer_required, "ufer_min_ft": self.ufer_min_ft,
            "ufer_cover_in": self.ufer_cover_in, "clearance_in": self.clearance_in, "warn_days": self.warn_days,
            "our_scope": self.our_scope, "costs": self.costs,
        })
    }
}

/// A bar by its printed size (`#5`, `5`, `No. 5`): diameter and weight per foot.
pub fn bar(size: &str) -> Option<(f64, f64)> {
    let s = size.trim().trim_start_matches("No.").trim_start_matches("no.").trim();
    let n = s.trim_start_matches('#').trim();
    let n = n.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("");
    BARS.iter().find(|(k, _, _)| k.trim_start_matches('#') == n).map(|(_, d, w)| (*d, *w))
}

pub(crate) fn r2(v: f64) -> f64 {
    if v.is_finite() { (v * 100.0).round() / 100.0 } else { 0.0 }
}

// ---------------- sizes as printed ----------------

/// A number as printed: `36`, `36.5`, `36-1/2`, `36 1/2`, `1/2`.
fn number(text: &str) -> Option<f64> {
    let mut total = 0.0;
    let mut any = false;
    for tok in text.split(|c: char| c.is_whitespace() || c == '-').filter(|t| !t.is_empty()) {
        let v = if let Some((a, b)) = tok.split_once('/') {
            let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
            if b == 0.0 {
                return None;
            }
            a / b
        } else {
            tok.parse::<f64>().ok()?
        };
        total += v;
        any = true;
    }
    (any && total.is_finite()).then_some(total)
}

/// One printed length in inches: `7'-6"`, `90"`, `90 in`, `7.5 ft`, `2286 mm`, `90` (in the
/// string's own unit, `unit` inches each).
fn length_in(part: &str, unit: f64) -> Option<f64> {
    let p: String = part.chars().filter(|c| c.is_ascii_digit() || matches!(c, '.' | '/' | '\'' | '"' | '-' | ' ')).collect();
    let p = p.trim().trim_matches('-').trim();
    if p.is_empty() {
        return None;
    }
    let v = if let Some((ft, inch)) = p.split_once('\'') {
        let ft = number(ft)?;
        let inch = inch.trim().trim_start_matches('-').trim_end_matches('"').trim();
        ft * 12.0 + if inch.is_empty() { 0.0 } else { number(inch)? }
    } else if p.contains('"') {
        number(p.trim_end_matches('"').split('"').next().unwrap_or(""))?
    } else {
        number(p)? * unit
    };
    (v.is_finite() && v > 0.0 && v < 1.0e5).then_some(v)
}

/// The plan footprint (width, depth, inches) in a printed size: `90"W x 36"D x 90"H`,
/// `90 x 36 x 90 in`, `7'-6" x 3'-0"`, `2286 x 914 x 2286 mm`.
pub fn footprint(dimensions: &str) -> Option<(f64, f64)> {
    let lower = dimensions.to_ascii_lowercase();
    let unit = if lower.contains("mm") {
        1.0 / 25.4
    } else if lower.contains("cm") {
        1.0 / 2.54
    } else if lower.contains("ft") || lower.contains("feet") {
        12.0
    } else {
        1.0
    };
    let parts: Vec<&str> = lower.split(['x', '×', '*']).map(str::trim).filter(|p| p.chars().any(|c| c.is_ascii_digit())).collect();
    let w = length_in(parts.first()?, unit)?;
    let d = length_in(parts.get(1)?, unit)?;
    Some((w, d))
}

/// A conduit's trade size from its printed size: `2"`, `2 in`, `1-1/4"`.
fn trade(size: &str) -> Option<f64> {
    length_in(size, 1.0).filter(|v| *v <= 12.0)
}

// ---------------- dates ----------------

/// Days since 1970-01-01 of a `YYYY-MM-DD` date.
pub fn day(text: &str) -> Option<i64> {
    let mut it = text.trim().get(..10)?.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1..=9999).contains(&y) {
        return None;
    }
    // Days from civil (proleptic Gregorian).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

// ---------------- equipment and pads ----------------

/// Equipment always set on the floor or grade (unless its mounting says otherwise).
const FLOOR_KINDS: [&str; 10] =
    ["switchboard", "switchgear", "transformer", "generator", "ups", "mdp", "chiller", "boiler", "unit_substation", "battery"];
/// Equipment set on the floor or grade only when its mounting or elevation says so.
const MAYBE_KINDS: [&str; 13] = [
    "panel",
    "panelboard",
    "ats",
    "transfer_switch",
    "rtu",
    "condensing_unit",
    "ahu",
    "water_heater",
    "pump",
    "tank",
    "air_compressor",
    "disconnect",
    "fire_pump",
];

fn mounting(e: &MepEquipment) -> String {
    e.properties
        .iter()
        .find(|(k, _)| matches!(k.to_ascii_lowercase().as_str(), "mounting" | "mtg" | "mount"))
        .map(|(_, v)| v.to_ascii_lowercase())
        .unwrap_or_default()
}

/// Why `e` needs a pad decision (floor- or grade-mounted), or none.
pub fn needs_pad(e: &MepEquipment) -> Option<&'static str> {
    let kind = e.kind.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    let m = mounting(e);
    let off_floor = ["wall", "surface", "recess", "flush", "pole", "ceiling", "roof", "curb", "suspend", "hung", "rack", "stand"];
    if off_floor.iter().any(|w| m.contains(w)) || e.level.to_ascii_lowercase().contains("roof") {
        return None;
    }
    let on_floor = ["floor", "pad", "grade", "free", "slab"].iter().any(|w| m.contains(w));
    if FLOOR_KINDS.contains(&kind.as_str()) {
        return Some(if on_floor { "floor-mounted (mounting)" } else { "floor-standing equipment" });
    }
    if MAYBE_KINDS.contains(&kind.as_str()) {
        if on_floor {
            return Some("floor-mounted (mounting)");
        }
        if e.bottom_ft.is_finite() && e.bottom_ft.abs() <= 0.25 && e.height_ft.is_finite() && e.height_ft > 0.0 {
            return Some("sits on the floor (bottom at the floor)");
        }
    }
    None
}

/// The footprint a pad is sized from (width, depth, inches, along the plan's x and y) and where
/// it came from: the approved submittal's dimensions, else the plan box.
pub fn equipment_footprint(sp: &BuildingSpec, types: &BimTypes, e: &MepEquipment) -> ([f64; 2], String) {
    let upf = upf(sp);
    let plan = [((e.max[0] - e.min[0]).abs() / upf * 12.0), ((e.max[1] - e.min[1]).abs() / upf * 12.0)];
    let plan = if plan.iter().all(|v| v.is_finite()) { plan } else { [0.0, 0.0] };
    let marks = [
        Some(e.tag.as_str()),
        e.properties.iter().find(|(k, _)| matches!(k.to_ascii_lowercase().as_str(), "type" | "mark")).map(|(_, v)| v.as_str()),
    ];
    for mark in marks.into_iter().flatten() {
        let Some(t) = types.get(mark) else { continue };
        let Some(dims) = t.get_field(cadaddy_doc::bimspec::keys::DIMENSIONS) else { continue };
        let status = t.get_field(cadaddy_doc::bimspec::keys::REVIEW_STATUS).map(|f| f.value.to_ascii_lowercase()).unwrap_or_default();
        if !(status.is_empty() || status.starts_with("approved")) {
            continue;
        }
        let Some((w, d)) = footprint(&dims.value) else { continue };
        // The longer side along the plan box's longer side.
        let (a, b) = (w.max(d), w.min(d));
        let fp = if plan[0] >= plan[1] { [a, b] } else { [b, a] };
        let src = if status.is_empty() { "BIM type dimensions" } else { "approved submittal" };
        return (fp, format!("{src} {} ({})", t.mark, dims.value.chars().take(60).collect::<String>()));
    }
    (plan, "plan".into())
}

pub(crate) fn upf(sp: &BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

/// A concrete element of the model, with where its pad numbers came from.
#[derive(Clone, Debug)]
pub struct Element {
    pub e: ConcreteElement,
    /// A pad made from a pad decision (not drawn as an element).
    pub derived: bool,
    /// A pad: the equipment's footprint (inches) and its source.
    pub footprint: Option<([f64; 2], String)>,
}

fn decision<'a>(sp: &'a BuildingSpec, tag: &str) -> Option<&'a PadDecision> {
    sp.concrete.pads.iter().find(|p| p.equipment.trim().eq_ignore_ascii_case(tag.trim()))
}

/// Feeders' raceways coming up through the bottom of `e`: (count, trade size inches).
fn stub_ups(sp: &BuildingSpec, e: &MepEquipment) -> Vec<(u32, f64, String)> {
    let mut out = Vec::new();
    for f in sp.mep.feeders.iter().take(MAX_CONCRETE) {
        let to = f.to.trim().eq_ignore_ascii_case(e.tag.trim()) && f.entry.trim().eq_ignore_ascii_case("bottom");
        let from_side = if f.from_entry.trim().is_empty() { &f.entry } else { &f.from_entry };
        let from = f.from.trim().eq_ignore_ascii_case(e.tag.trim()) && from_side.trim().eq_ignore_ascii_case("bottom");
        if (to || from) && f.conduits > 0 && f.conduit_in.is_finite() && f.conduit_in > 0.0 {
            out.push((f.conduits.min(400), f.conduit_in, f.tag.clone()));
        }
    }
    out
}

/// Every concrete element: those in the spec and the pads the pad decisions call for (sized
/// from the equipment now), in that order.
pub fn elements(sp: &BuildingSpec, types: &BimTypes) -> Vec<Element> {
    let r = Rules::of(sp);
    let upf = upf(sp);
    let mut out: Vec<Element> =
        sp.concrete.elements.iter().take(MAX_CONCRETE).map(|e| Element { e: e.clone(), derived: false, footprint: None }).collect();
    for x in out.iter_mut().filter(|x| x.e.kind == "pad" && !x.e.equipment.is_empty()) {
        if let Some(eq) = sp.mep.equipment.iter().find(|q| q.tag.eq_ignore_ascii_case(x.e.equipment.trim())) {
            x.footprint = Some(equipment_footprint(sp, types, eq));
        }
    }
    for d in sp.concrete.pads.iter().take(MAX_CONCRETE).filter(|d| d.required == Some(true)) {
        if out.iter().any(|x| x.e.kind == "pad" && x.e.equipment.eq_ignore_ascii_case(d.equipment.trim())) {
            continue;
        }
        let Some(eq) = sp.mep.equipment.iter().find(|q| q.tag.eq_ignore_ascii_case(d.equipment.trim())) else { continue };
        let (fp, src) = equipment_footprint(sp, types, eq);
        let m = d.margin_in.filter(|v| v.is_finite() && *v >= 0.0 && *v <= 120.0).unwrap_or(r.pad_margin_in);
        let size = match d.size_in.filter(|s| s.iter().all(|v| v.is_finite() && *v > 0.0 && *v < 1.0e4)) {
            // A detail's size, the longer side along the equipment's longer side.
            Some(s) => {
                let (a, b) = (s[0].max(s[1]), s[0].min(s[1]));
                if fp[0] >= fp[1] { [a, b] } else { [b, a] }
            }
            None => [fp[0] + 2.0 * m, fp[1] + 2.0 * m],
        };
        let c = [(eq.min[0] + eq.max[0]) / 2.0, (eq.min[1] + eq.max[1]) / 2.0];
        let half = [size[0] / 24.0 * upf, size[1] / 24.0 * upf];
        let height = d.height_in.filter(|v| v.is_finite() && *v > 0.0 && *v <= 120.0).unwrap_or(r.pad_height_in);
        let mut embeds: Vec<Embed> = d.embeds.iter().take(MAX_CONCRETE).cloned().collect();
        if r.pad_anchor_bolts > 0 && !embeds.iter().any(|x| x.kind == "anchor_bolt") {
            embeds.push(Embed {
                kind: "anchor_bolt".into(),
                size: format!("{}\" dia", r.pad_anchor_in),
                count: r.pad_anchor_bolts,
                diameter_in: r.pad_anchor_in,
                source: "rules: anchor bolts per pad".into(),
                ..Embed::default()
            });
        }
        if !embeds.iter().any(|x| x.kind == "stub_up") {
            for (n, t, tag) in stub_ups(sp, eq) {
                embeds.push(Embed {
                    kind: "stub_up".into(),
                    size: format!("{t}\""),
                    count: n,
                    source: format!("feeder {tag} (bottom entry)"),
                    ..Embed::default()
                });
            }
        }
        out.push(Element {
            e: ConcreteElement {
                id: format!("PAD-{}", eq.tag),
                kind: "pad".into(),
                level: eq.level.clone(),
                min: [c[0] - half[0], c[1] - half[1]],
                max: [c[0] + half[0], c[1] + half[1]],
                depth_in: height,
                top_ft: height / 12.0,
                chamfer_in: d.chamfer_in.filter(|v| v.is_finite() && *v >= 0.0 && *v <= 12.0).unwrap_or(r.pad_chamfer_in),
                rebar: if d.rebar.is_empty() { r.pad_rebar.clone() } else { d.rebar.clone() },
                embeds,
                owner: d.owner.trim().to_ascii_lowercase(),
                source: d.basis.clone(),
                pour: d.pour.clone(),
                equipment: eq.tag.clone(),
                ..ConcreteElement::default()
            },
            derived: true,
            footprint: Some((fp, src)),
        });
    }
    out
}

/// The pad an equipment item stands on, if any: its top above the floor, feet.
pub fn pad_top(els: &[Element], tag: &str) -> Option<f64> {
    els.iter().find(|x| x.e.kind == "pad" && x.e.equipment.eq_ignore_ascii_case(tag)).map(|x| x.e.top_ft).filter(|t| t.is_finite() && *t > 0.0)
}

// ---------------- geometry ----------------

/// Plan extents of an element in feet: (width along x, depth along y), its plan area (sq ft),
/// perimeter (ft) and, for a path, its length (ft).
struct Shape {
    area: f64,
    perimeter: f64,
    /// Along x and y (a box), or the path's length and the width (a path).
    a: f64,
    b: f64,
    path: bool,
    round: bool,
}

fn shape(e: &ConcreteElement, upf: f64) -> Shape {
    if e.path.len() >= 2 {
        let len: f64 = e
            .path
            .windows(2)
            .map(|w| match w {
                [p, q] => ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt() / upf,
                _ => 0.0,
            })
            .sum();
        let w = (e.width_in / 12.0).max(0.0);
        let len = if len.is_finite() { len } else { 0.0 };
        return Shape { area: len * w, perimeter: 2.0 * (len + w), a: len, b: w, path: true, round: false };
    }
    let a = (e.max[0] - e.min[0]).abs() / upf;
    let b = (e.max[1] - e.min[1]).abs() / upf;
    let (a, b) = if a.is_finite() && b.is_finite() { (a, b) } else { (0.0, 0.0) };
    if e.round {
        let d = a.min(b);
        return Shape { area: std::f64::consts::PI * d * d / 4.0, perimeter: std::f64::consts::PI * d, a: d, b: d, path: false, round: true };
    }
    Shape { area: a * b, perimeter: 2.0 * (a + b), a, b, path: false, round: false }
}

// ---------------- quantities ----------------

/// What one element takes.
#[derive(Clone, Debug, Default)]
pub struct Qty {
    pub cy: f64,
    pub cy_waste: f64,
    pub forms_sf: f64,
    pub edge_lf: f64,
    pub round_form_lf: f64,
    pub chamfer_lf: f64,
    /// By bar size: (linear feet, pounds).
    pub rebar: BTreeMap<String, (f64, f64)>,
    pub mesh_sf: f64,
    pub finish_sf: f64,
    pub curing_sf: f64,
    /// By (kind, size): (count, feet).
    pub embeds: BTreeMap<(String, String), (u64, f64)>,
}

/// Feet of bar with its laps: a bar longer than stock gets a lap per extra piece.
fn with_laps(len_ft: f64, lap_ft: f64, stock: f64) -> f64 {
    if len_ft <= stock || lap_ft >= stock {
        return len_ft;
    }
    let laps = ((len_ft - stock) / (stock - lap_ft)).ceil().clamp(0.0, MAX_BARS);
    len_ft + laps * lap_ft
}

fn rebar_lf(e: &ConcreteElement, s: &Shape, b: &Rebar, r: &Rules) -> f64 {
    let Some((db, _)) = bar(&b.bar) else { return 0.0 };
    let cover = if b.cover_in > 0.0 && b.cover_in.is_finite() { b.cover_in } else { r.cover(&e.kind) } / 12.0;
    let lap = if b.lap_in > 0.0 && b.lap_in.is_finite() { b.lap_in / 12.0 } else { r.lap_db * db / 12.0 };
    let sp = b.spacing_in / 12.0;
    let layers = f64::from(b.layers.clamp(1, 8));
    let depth = (e.depth_in / 12.0).max(0.0);
    let count = |span: f64| -> f64 { if sp > 0.0 && sp.is_finite() && span > 0.0 { ((span / sp).floor() + 1.0).min(MAX_BARS) } else { 0.0 } };
    let lf = match b.direction.as_str() {
        "x" | "y" | "each_way" => {
            let (lx, ly) = (s.a - 2.0 * cover, s.b - 2.0 * cover);
            if lx <= 0.0 || ly <= 0.0 {
                return 0.0;
            }
            // Bars along x are spaced across y, and the other way.
            let along_x = count(ly) * with_laps(lx, lap, r.stock_ft);
            let along_y = count(lx) * with_laps(ly, lap, r.stock_ft);
            match b.direction.as_str() {
                "x" => along_x,
                "y" => along_y,
                _ => along_x + along_y,
            }
        }
        "long" => {
            let run = if s.path {
                s.a
            } else if matches!(e.kind.as_str(), "pier" | "pole_base") {
                depth
            } else {
                s.a.max(s.b)
            } - 2.0 * cover;
            f64::from(b.count.min(10_000)) * with_laps(run.max(0.0), lap, r.stock_ft)
        }
        "ties" => {
            let (run, perim) = if s.round {
                (depth - 2.0 * cover, std::f64::consts::PI * (s.a - 2.0 * cover))
            } else if s.path {
                (s.a - 2.0 * cover, 2.0 * ((s.b - 2.0 * cover) + (depth - 2.0 * cover)))
            } else {
                (s.a.max(s.b) - 2.0 * cover, 2.0 * ((s.a.min(s.b) - 2.0 * cover) + (depth - 2.0 * cover)))
            };
            if run <= 0.0 || perim <= 0.0 {
                return 0.0;
            }
            count(run) * (perim + 2.0 * r.hook_db * db / 12.0)
        }
        "dowels" => f64::from(b.count.min(10_000)) * (b.length_in / 12.0).max(0.0),
        _ => 0.0,
    };
    if lf.is_finite() { lf * layers } else { 0.0 }
}

pub fn quantities(e: &ConcreteElement, upf: f64, r: &Rules) -> Qty {
    let s = shape(e, upf);
    let mut q = Qty::default();
    let depth = (e.depth_in / 12.0).max(0.0);
    let mut cf = s.area * depth;
    if let Some([w, d]) = e.thickened_edge_in
        && e.kind == "slab"
        && w > 0.0
        && d > e.depth_in
    {
        cf += s.perimeter * (w / 12.0) * ((d - e.depth_in) / 12.0);
    }
    q.cy = cf / 27.0;
    let waste = if q.cy < 1.0 { r.small_waste_pct } else { r.waste_pct };
    q.cy_waste = q.cy * (1.0 + waste / 100.0);
    match e.kind.as_str() {
        "slab" => {
            let edge = e.thickened_edge_in.map_or(e.depth_in, |[_, d]| d.max(e.depth_in)) / 12.0;
            q.edge_lf = s.perimeter;
            q.forms_sf = s.perimeter * edge;
            for dep in e.depressions.iter().take(MAX_CONCRETE) {
                let w = (dep.max[0] - dep.min[0]).abs() / upf;
                let h = (dep.max[1] - dep.min[1]).abs() / upf;
                if w.is_finite() && h.is_finite() && dep.depth_in.is_finite() && dep.depth_in > 0.0 {
                    q.edge_lf += 2.0 * (w + h);
                    q.forms_sf += 2.0 * (w + h) * dep.depth_in / 12.0;
                }
            }
            q.finish_sf = s.area;
            q.curing_sf = s.area;
        }
        "pad" | "handhole_base" => {
            q.edge_lf = s.perimeter;
            q.forms_sf = s.perimeter * depth;
            if e.chamfer_in > 0.0 && e.kind == "pad" {
                q.chamfer_lf = s.perimeter;
            }
            q.finish_sf = s.area;
            q.curing_sf = s.area;
        }
        _ if s.round => {
            if !e.earth_formed {
                q.round_form_lf = depth;
                q.forms_sf = s.perimeter * depth;
            }
        }
        _ => {
            if !e.earth_formed {
                q.edge_lf = s.perimeter;
                q.forms_sf = s.perimeter * depth;
            }
        }
    }
    for b in e.rebar.iter().take(64) {
        let lf = rebar_lf(e, &s, b, r);
        if let Some((_, w)) = bar(&b.bar)
            && lf > 0.0
        {
            let k = format!("#{}", b.bar.trim().trim_start_matches('#'));
            let v = q.rebar.entry(k).or_default();
            v.0 += lf;
            v.1 += lf * w;
        }
    }
    if !e.mesh.trim().is_empty() {
        q.mesh_sf = s.area * (1.0 + r.mesh_lap_pct / 100.0);
    }
    for x in e.embeds.iter().take(MAX_CONCRETE) {
        let v = q.embeds.entry((x.kind.clone(), x.size.clone())).or_default();
        v.0 = v.0.saturating_add(u64::from(x.count.max(1)));
        if x.length_ft.is_finite() && x.length_ft > 0.0 {
            v.1 += x.length_ft * f64::from(x.count.max(1));
        }
    }
    q
}

pub(crate) fn scope_of(r: &Rules, owner: &str) -> String {
    if owner.trim().is_empty() {
        "unassigned".into()
    } else if r.ours(owner) {
        "ours".into()
    } else {
        format!("by others ({})", owner.trim())
    }
}

/// The concrete takeoff as rows `[scope, level, element, kind, item, size, quantity, unit]`,
/// header first.
pub fn rows(sp: &BuildingSpec, els: &[Element], level: Option<&str>) -> Vec<Vec<String>> {
    let r = Rules::of(sp);
    let upf = upf(sp);
    let mut out = vec![["Scope", "Level", "Element", "Kind", "Item", "Size", "Quantity", "Unit"].map(String::from).to_vec()];
    for x in els.iter().filter(|x| level.is_none_or(|l| x.e.level.eq_ignore_ascii_case(l))) {
        let q = quantities(&x.e, upf, &r);
        let scope = scope_of(&r, &x.e.owner);
        let mut line = |item: &str, size: String, v: f64, unit: &str| {
            if v > 0.0 {
                out.push(vec![
                    scope.clone(),
                    x.e.level.clone(),
                    x.e.id.clone(),
                    x.e.kind.clone(),
                    item.into(),
                    size,
                    format!("{}", r2(v)),
                    unit.into(),
                ]);
            }
        };
        line("Concrete (with waste)", format!("{}\"", r2(x.e.depth_in)), q.cy_waste, "cy");
        line("Forms", String::new(), q.forms_sf, "sf");
        line("Edge forms", String::new(), q.edge_lf, "lf");
        line("Round forms", String::new(), q.round_form_lf, "lf");
        line("Chamfer strip", format!("{}\"", r2(x.e.chamfer_in)), q.chamfer_lf, "lf");
        for (b, (lf, lb)) in &q.rebar {
            line("Rebar", b.clone(), *lf, "lf");
            line("Rebar weight", b.clone(), *lb, "lb");
        }
        line("Welded wire", x.e.mesh.clone(), q.mesh_sf, "sf");
        line("Finish", String::new(), q.finish_sf, "sf");
        line("Curing", String::new(), q.curing_sf, "sf");
        for ((k, size), (n, ft)) in &q.embeds {
            let item = k.replace('_', " ");
            if *ft > 0.0 {
                line(&item, size.clone(), *ft, "lf");
            } else {
                line(&item, size.clone(), *n as f64, "ea");
            }
        }
    }
    out
}

/// The concrete rows for the MEP takeoff (`mep.qty`), in its ten columns.
pub fn mep_rows(sp: &BuildingSpec, types: &BimTypes, level: Option<&str>) -> Vec<Vec<String>> {
    let els = elements(sp, types);
    rows(sp, &els, level)
        .into_iter()
        .skip(1)
        .map(|row| {
            let g = |i: usize| row.get(i).cloned().unwrap_or_default();
            let cat = if g(0) == "ours" { "Concrete".to_string() } else { format!("Concrete, {}", g(0)) };
            vec![
                cat,
                g(1),
                g(2),
                format!("{} {}", g(3).replace('_', " "), g(4).to_lowercase()),
                g(5),
                String::new(),
                g(6),
                g(7),
                String::new(),
                String::new(),
            ]
        })
        .collect()
}

// ---------------- findings ----------------

/// One problem found.
#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    /// `pad decision`, `pad owner`, `pad basis`, `pad size`, `clearance`, `ufer`, `cover`,
    /// `clash`, `embed`, `hold point`, `scope`.
    pub check: &'static str,
    /// `error`, `warning` or `info`.
    pub severity: &'static str,
    /// The equipment tag or the element id.
    pub about: String,
    pub level: String,
    pub at: [f64; 2],
    pub message: String,
    pub fix: String,
}

impl Finding {
    fn json(&self) -> Value {
        json!({"check": self.check, "severity": self.severity, "about": self.about, "level": self.level, "message": self.message, "fix": self.fix})
    }

    fn rfi_kind(&self) -> Option<cadaddy_walk3d::RfiKind> {
        use cadaddy_walk3d::RfiKind;
        match self.check {
            "pad decision" | "pad owner" | "pad size" | "clearance" => Some(RfiKind::PadUndecided),
            "hold point" | "ufer" => Some(RfiKind::PourHold),
            "cover" | "clash" | "embed" => Some(RfiKind::EmbedConflict),
            _ => None,
        }
    }
}

fn centre(e: &ConcreteElement) -> [f64; 2] {
    if let (Some(a), Some(b)) = (e.path.first(), e.path.last())
        && e.path.len() >= 2
    {
        return [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    }
    [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0]
}

fn rect(min: [f64; 2], max: [f64; 2]) -> [f64; 4] {
    [min[0].min(max[0]), min[1].min(max[1]), min[0].max(max[0]), min[1].max(max[1])]
}

/// Overlap of two rectangles with some area (touching doesn't count).
fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[2] - 1e-9 && b[0] < a[2] - 1e-9 && a[1] < b[3] - 1e-9 && b[1] < a[3] - 1e-9
}

fn inside(e: &ConcreteElement, p: [f64; 2], upf: f64) -> bool {
    if e.path.len() >= 2 {
        let half = e.width_in / 24.0 * upf;
        return e.path.windows(2).any(|w| match w {
            [a, b] => {
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l2 = dx * dx + dy * dy;
                let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                let (cx, cy) = (a[0] + t * dx, a[1] + t * dy);
                ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt() <= half + 1e-9
            }
            _ => false,
        });
    }
    let r = rect(e.min, e.max);
    p[0] >= r[0] - 1e-9 && p[0] <= r[2] + 1e-9 && p[1] >= r[1] - 1e-9 && p[1] <= r[3] + 1e-9
}

/// An embed's outside diameter, inches.
fn embed_od(x: &Embed, gear: &super::gear::Rules) -> f64 {
    if x.diameter_in.is_finite() && x.diameter_in > 0.0 {
        return x.diameter_in;
    }
    match x.kind.as_str() {
        "conduit" | "stub_up" | "sleeve" => trade(&x.size).map_or(0.0, |t| gear.od(t)),
        "ufer" => bar(&x.size).map_or(0.5, |(d, _)| d),
        _ => trade(&x.size).unwrap_or(0.0),
    }
}

/// The rebar mats of an element: the band (top and bottom, inches from the element's top) of
/// each layer.
fn mats(e: &ConcreteElement, r: &Rules) -> Vec<(String, f64, f64)> {
    let mut out = Vec::new();
    for b in e.rebar.iter().take(64).filter(|b| matches!(b.direction.as_str(), "x" | "y" | "each_way")) {
        let Some((db, _)) = bar(&b.bar) else { continue };
        let cover = if b.cover_in > 0.0 && b.cover_in.is_finite() { b.cover_in } else { r.cover(&e.kind) };
        // Each way: two bars crossing.
        let thick = if b.direction == "each_way" { 2.0 * db } else { db };
        let bottom = (e.depth_in - cover - thick, e.depth_in - cover);
        if b.layers >= 2 {
            out.push((format!("top mat ({})", b.bar), cover, cover + thick));
            out.push((format!("bottom mat ({})", b.bar), bottom.0, bottom.1));
        } else if matches!(e.kind.as_str(), "slab" | "pad" | "handhole_base") {
            let mid = e.depth_in / 2.0;
            out.push((format!("mat ({}) at mid-depth", b.bar), mid - thick / 2.0, mid + thick / 2.0));
        } else {
            out.push((format!("bottom mat ({})", b.bar), bottom.0, bottom.1));
        }
    }
    out
}

/// What is cast into each element against its size, cover and rebar.
fn embed_findings(sp: &BuildingSpec, els: &[Element], r: &Rules, out: &mut Vec<Finding>) {
    let upf = upf(sp);
    let gear = super::gear::Rules::of(sp);
    for x in els {
        let e = &x.e;
        let cover = r.cover(&e.kind);
        let mats = mats(e, r);
        for m in e.embeds.iter().take(MAX_CONCRETE) {
            let at = m.at.unwrap_or_else(|| centre(e));
            let mut push = |check: &'static str, message: String, fix: &str| {
                if out.len() < MAX_FINDINGS {
                    out.push(Finding { check, severity: "error", about: e.id.clone(), level: e.level.clone(), at, message, fix: fix.into() });
                }
            };
            if m.at.is_some() && !inside(e, at, upf) {
                push(
                    "embed",
                    format!("{}: the {} ({}) is drawn outside the element", e.id, m.kind.replace('_', " "), m.size),
                    "Move it into the element or into the element it belongs to",
                );
                continue;
            }
            let od = embed_od(m, &gear);
            match m.kind.as_str() {
                "conduit" | "sleeve" | "ufer" if e.depth_in > 0.0 && od > 0.0 => {
                    let c = if m.depth_in > 0.0 { m.depth_in } else { e.depth_in / 2.0 };
                    let (top, bottom) = (c - od / 2.0, e.depth_in - c - od / 2.0);
                    let need = if m.kind == "ufer" { r.ufer_cover_in } else { cover };
                    if top.min(bottom) + 1e-9 < need {
                        push(
                            "cover",
                            format!(
                                "{}: the {} ({}, {}\" OD) at {}\" deep leaves {}\" of cover; {}\" needed",
                                e.id,
                                m.kind.replace('_', " "),
                                m.size,
                                r2(od),
                                r2(c),
                                r2(top.min(bottom)),
                                r2(need)
                            ),
                            "Deepen the element, lower or raise the item, or use a smaller one",
                        );
                    } else if m.kind != "ufer"
                        && let Some((name, a, b)) = mats.iter().find(|(_, a, b)| c - od / 2.0 < *b && c + od / 2.0 > *a)
                    {
                        push(
                            "clash",
                            format!(
                                "{}: the {} ({}) at {}\" deep runs through the {name} ({}\"–{}\" deep)",
                                e.id,
                                m.kind.replace('_', " "),
                                m.size,
                                r2(c),
                                r2(*a),
                                r2(*b)
                            ),
                            "Run it above or below the mat (chairs, a lower conduit), or detail the bars round it",
                        );
                    }
                }
                "anchor_bolt" if m.embed_in > 0.0 && e.depth_in > 0.0 => {
                    let room = e.depth_in - cover;
                    if m.embed_in > room + 1e-9 {
                        push(
                            "embed",
                            format!(
                                "{}: anchor bolts embedded {}\" in {}\" of concrete leave less than {}\" below them",
                                e.id,
                                r2(m.embed_in),
                                r2(e.depth_in),
                                r2(cover)
                            ),
                            "A deeper pad, shorter bolts per the manufacturer, or anchors into the slab below",
                        );
                    }
                }
                _ => {}
            }
            if m.kind == "ufer" {
                let len = m.length_ft * f64::from(m.count.max(1));
                if len + 1e-9 < r.ufer_min_ft {
                    push(
                        "ufer",
                        format!("{}: the concrete-encased electrode is {} ft; at least {} ft is needed", e.id, r2(len), r.ufer_min_ft),
                        "Extend it along the footing",
                    );
                }
                if !matches!(e.kind.as_str(), "footing" | "grade_beam" | "pier" | "pole_base") {
                    push(
                        "ufer",
                        format!(
                            "{}: a concrete-encased electrode must be in a foundation in contact with the earth, not a {}",
                            e.id,
                            e.kind.replace('_', " ")
                        ),
                        "Put it in a footing or grade beam",
                    );
                }
            }
        }
    }
}

/// Pad decisions, owners, sizes and the working space in front of gear.
fn pad_findings(sp: &BuildingSpec, types: &BimTypes, els: &[Element], only: Option<&str>, r: &Rules, out: &mut Vec<Finding>) {
    let upf = upf(sp);
    for eq in sp.mep.equipment.iter().take(MAX_CONCRETE).filter(|e| only.is_none_or(|t| e.tag.eq_ignore_ascii_case(t))) {
        let Some(why) = needs_pad(eq) else { continue };
        let at = [(eq.min[0] + eq.max[0]) / 2.0, (eq.min[1] + eq.max[1]) / 2.0];
        let mut push = |check: &'static str, severity: &'static str, message: String, fix: &str| {
            if out.len() < MAX_FINDINGS {
                out.push(Finding { check, severity, about: eq.tag.clone(), level: eq.level.clone(), at, message, fix: fix.into() });
            }
        };
        let d = decision(sp, &eq.tag);
        let drawn = els.iter().find(|x| !x.derived && x.e.kind == "pad" && x.e.equipment.eq_ignore_ascii_case(&eq.tag));
        match d.and_then(|d| d.required) {
            None if drawn.is_none() => {
                push(
                    "pad decision",
                    "error",
                    format!("{} ({}, {why}) has no pad decision: is a housekeeping pad required, and whose is it?", eq.tag, eq.kind),
                    "Decide from the spec section, the details and the utility's or manufacturer's requirements; record the owner",
                );
                continue;
            }
            Some(false) => continue,
            _ => {}
        }
        let owner = drawn.map(|x| x.e.owner.clone()).filter(|o| !o.trim().is_empty()).or_else(|| d.map(|d| d.owner.clone())).unwrap_or_default();
        if owner.trim().is_empty() {
            push(
                "pad owner",
                "error",
                format!("{}: the pad has no owner (electrical, concrete, GC or utility)", eq.tag),
                "Assign the pad's scope from the specs and the project's division of work",
            );
        }
        if d.is_some_and(|d| d.basis.trim().is_empty()) && drawn.is_none_or(|x| x.e.source.trim().is_empty()) {
            push(
                "pad basis",
                "warning",
                format!("{}: the pad decision gives no basis (spec section, detail, requirement)", eq.tag),
                "Cite the spec section or detail",
            );
        }
        // The pad against the footprint.
        let Some(pad) = els.iter().find(|x| x.e.kind == "pad" && x.e.equipment.eq_ignore_ascii_case(&eq.tag)) else { continue };
        let (fp, src) = equipment_footprint(sp, types, eq);
        let pr = rect(pad.e.min, pad.e.max);
        let pw = (pr[2] - pr[0]) / upf * 12.0;
        let pd = (pr[3] - pr[1]) / upf * 12.0;
        let need = [fp[0] + 2.0 * r.pad_margin_min_in, fp[1] + 2.0 * r.pad_margin_min_in];
        if fp[0] > 0.0 && fp[1] > 0.0 && (pw + 1e-6 < need[0] || pd + 1e-6 < need[1]) {
            push(
                "pad size",
                "error",
                format!(
                    "{}: the pad is {}\" x {}\"; the equipment ({}\" x {}\", {src}) needs {}\" x {}\" with {}\" each side",
                    eq.tag,
                    r2(pw),
                    r2(pd),
                    r2(fp[0]),
                    r2(fp[1]),
                    r2(need[0]),
                    r2(need[1]),
                    r.pad_margin_min_in
                ),
                "Enlarge the pad (or have the detail revised) to the approved submittal's footprint",
            );
        }
        if src == "plan" && super::gear::is_gear(eq) {
            push("pad size", "info", format!("{}: the pad is sized from the plan; check it again when the submittal is approved", eq.tag), "");
        }
        // The working space in front of gear.
        if super::gear::is_gear(eq) || matches!(eq.kind.to_ascii_lowercase().as_str(), "generator" | "ats" | "transfer_switch") {
            let er = rect(eq.min, eq.max);
            let depth = r.clearance_in / 12.0 * upf;
            let front = d.map(|d| d.front.trim().to_ascii_lowercase()).unwrap_or_default();
            let wide_x = er[2] - er[0] >= er[3] - er[1];
            let sides: Vec<&str> = match front.as_str() {
                "n" | "s" | "e" | "w" => vec![front.as_str()],
                _ if wide_x => vec!["s", "n"],
                _ => vec!["w", "e"],
            };
            let zone = |side: &str| match side {
                "n" => [er[0], er[3], er[2], er[3] + depth],
                "s" => [er[0], er[1] - depth, er[2], er[1]],
                "e" => [er[2], er[1], er[2] + depth, er[3]],
                _ => [er[0] - depth, er[1], er[0], er[3]],
            };
            let blockers = |z: [f64; 4]| -> Vec<String> {
                let mut b: Vec<String> = sp
                    .mep
                    .equipment
                    .iter()
                    .take(MAX_CONCRETE)
                    .filter(|o| o.tag != eq.tag && o.level.eq_ignore_ascii_case(&eq.level) && overlaps(z, rect(o.min, o.max)))
                    .map(|o| o.tag.clone())
                    .collect();
                b.extend(
                    els.iter()
                        .filter(|x| {
                            x.e.level.eq_ignore_ascii_case(&eq.level)
                                && !x.e.equipment.eq_ignore_ascii_case(&eq.tag)
                                && matches!(x.e.kind.as_str(), "pad" | "pier" | "pole_base" | "handhole_base")
                                && x.e.top_ft > 0.0
                                && overlaps(z, rect(x.e.min, x.e.max))
                        })
                        .map(|x| x.e.id.clone()),
                );
                b
            };
            let blocked: Vec<(String, Vec<String>)> = sides.iter().map(|s| (s.to_string(), blockers(zone(s)))).collect();
            if blocked.iter().all(|(_, b)| !b.is_empty()) {
                let what = blocked.iter().map(|(s, b)| format!("{s}: {}", b.join(", "))).collect::<Vec<_>>().join("; ");
                push(
                    "clearance",
                    "error",
                    format!("{}: the {}\" working space in front of it is not clear ({what})", eq.tag, r.clearance_in),
                    "Move the pad or the equipment so the working space in front of the gear stays clear",
                );
            }
        }
    }
    for d in sp.concrete.pads.iter().take(MAX_CONCRETE) {
        if !sp.mep.equipment.iter().any(|e| e.tag.eq_ignore_ascii_case(d.equipment.trim())) && out.len() < MAX_FINDINGS {
            out.push(Finding {
                check: "pad decision",
                severity: "warning",
                about: d.equipment.clone(),
                level: String::new(),
                at: [0.0, 0.0],
                message: format!("a pad decision for {}, which isn't in the equipment", d.equipment.chars().take(40).collect::<String>()),
                fix: "Remove it or correct the tag".into(),
            });
        }
    }
}

// ---------------- hold points ----------------

/// One item to be in or under a pour before concrete.
#[derive(Clone, Debug)]
pub struct Hold {
    pub pour: String,
    pub date: String,
    /// Its key in the checklist (`element|kind|size`), what a sign-off names.
    pub key: String,
    pub element: String,
    pub level: String,
    pub at: [f64; 2],
    pub kind: String,
    pub size: String,
    pub count: u32,
    pub length_ft: f64,
    pub owner: String,
    pub source: String,
    pub signed: Option<Signoff>,
    /// `signed`, `missed`, `due`, `open`, `unscheduled`, `not drawn`.
    pub status: &'static str,
    pub days: Option<i64>,
    pub alternative: String,
    pub cost: f64,
    pub cost_basis: String,
}

impl Hold {
    fn json(&self) -> Value {
        json!({
            "pour": self.pour, "date": self.date, "item": self.key, "element": self.element, "level": self.level, "kind": self.kind,
            "size": self.size, "count": self.count, "length_ft": r2(self.length_ft), "owner": self.owner, "source": self.source,
            "signed_by": self.signed.as_ref().map(|s| s.by.clone()), "signed_on": self.signed.as_ref().map(|s| s.on.clone()),
            "status": self.status, "days_to_pour": self.days, "alternative": self.alternative,
            "cost": if self.alternative.is_empty() { Value::Null } else { json!(r2(self.cost)) }, "cost_basis": self.cost_basis,
        })
    }
}

/// The building's perimeter round its foundations (feet), for a ground ring.
fn foundation_perimeter(sp: &BuildingSpec, els: &[Element]) -> f64 {
    let upf = upf(sp);
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for x in els.iter().filter(|x| matches!(x.e.kind.as_str(), "footing" | "grade_beam" | "slab")) {
        let pts: Vec<[f64; 2]> = if x.e.path.len() >= 2 { x.e.path.clone() } else { vec![x.e.min, x.e.max] };
        for p in pts.iter().filter(|p| p.iter().all(|v| v.is_finite())) {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
    }
    if b[0] > b[2] {
        return 0.0;
    }
    2.0 * ((b[2] - b[0]) + (b[3] - b[1])) / upf
}

/// What replaces an item missed in a pour, and roughly what it costs.
fn alternative(kind: &str, count: u32, length_ft: f64, perimeter_ft: f64, r: &Rules) -> (String, f64, String) {
    let n = f64::from(count.max(1));
    match kind {
        "ufer" => {
            let ring = perimeter_ft.max(r.ufer_min_ft);
            let rods = 2.0;
            let ring_cost = ring * r.cost("ground_ring_per_ft");
            let rod_cost = rods * r.cost("ground_rod");
            (
                format!(
                    "A ground ring round the building (bare copper, buried, about {} ft) or, where accepted, ground rods (2 rods, tested) bonded to the service",
                    r2(ring)
                ),
                ring_cost.max(rod_cost),
                format!("{} ft x {} per ft (ring); 2 rods x {}", r2(ring), r.cost("ground_ring_per_ft"), r.cost("ground_rod")),
            )
        }
        "conduit" => {
            let ft = if length_ft > 0.0 { length_ft } else { 10.0 * n };
            (
                "Saw-cut and trench the slab and patch, or run the conduit overhead or exposed".into(),
                ft * r.cost("sawcut_trench_per_ft"),
                format!("{} ft x {} per ft", r2(ft), r.cost("sawcut_trench_per_ft")),
            )
        }
        "stub_up" | "sleeve" => (
            "Core-drill through the concrete (scan for rebar first) and patch; or trench in from the side".into(),
            n * r.cost("core_drill"),
            format!("{n} x {}", r.cost("core_drill")),
        ),
        "anchor_bolt" => (
            "Post-installed adhesive anchors, as the engineer and the manufacturer accept".into(),
            n * r.cost("adhesive_anchor"),
            format!("{n} x {}", r.cost("adhesive_anchor")),
        ),
        "embed_plate" => (
            "A surface plate on post-installed anchors, as the engineer accepts".into(),
            n * r.cost("surface_plate"),
            format!("{n} x {}", r.cost("surface_plate")),
        ),
        "bond" => (
            "An exothermic bond to rebar exposed at a blockout, or a separate electrode".into(),
            n * r.cost("exothermic_bond"),
            format!("{n} x {}", r.cost("exothermic_bond")),
        ),
        "ground_rod" => {
            ("Drive the rod beside the foundation and bond it".into(), n * r.cost("ground_rod"), format!("{n} x {}", r.cost("ground_rod")))
        }
        _ => (String::new(), 0.0, String::new()),
    }
}

/// Every pour's checklist: what is cast into or run under each of its elements, with the
/// grounding's concrete-encased electrodes in their foundations; and, when the building has
/// footings but no concrete-encased electrode, the electrode as a missing item of the first
/// footing pour.
pub fn holds(sp: &BuildingSpec, els: &[Element], today: Option<i64>, r: &Rules) -> Vec<Hold> {
    let perimeter = foundation_perimeter(sp, els);
    let pour_of = |id: &str| -> Option<&Pour> { sp.concrete.pours.iter().find(|p| !id.is_empty() && p.id.eq_ignore_ascii_case(id.trim())) };
    let mut out: Vec<Hold> = Vec::new();
    let add = |out: &mut Vec<Hold>,
               e: &ConcreteElement,
               kind: &str,
               size: &str,
               count: u32,
               length_ft: f64,
               owner: &str,
               source: &str,
               at: [f64; 2],
               drawn: bool,
               electrode: Option<&Electrode>| {
        if out.len() >= MAX_FINDINGS {
            return;
        }
        let pour = pour_of(&e.pour);
        let n = out.iter().filter(|h| h.element == e.id && h.kind == kind && h.size == size).count();
        let key = if n == 0 { format!("{}|{kind}|{size}", e.id) } else { format!("{}|{kind}|{size}|{}", e.id, n + 1) };
        // A grounding electrode documented as installed stands as signed off; its footing marked
        // poured is a placed pour.
        let signed = pour.and_then(|p| p.signoffs.iter().find(|s| s.item.eq_ignore_ascii_case(&key)).cloned()).or_else(|| {
            electrode.filter(|g| g.documented).map(|g| Signoff { item: key.clone(), by: format!("documented ({})", g.tag), on: String::new() })
        });
        let days = pour.and_then(|p| day(&p.date)).zip(today).map(|(d, t)| d - t);
        let placed =
            pour.is_some_and(|p| p.placed) || days.is_some_and(|d| d < 0) || electrode.is_some_and(|g| g.pour.trim().eq_ignore_ascii_case("poured"));
        let status = if signed.is_some() && drawn {
            "signed"
        } else if placed {
            "missed"
        } else if !drawn {
            "not drawn"
        } else if pour.is_none() {
            "unscheduled"
        } else if days.is_some_and(|d| d <= r.warn_days) {
            "due"
        } else {
            "open"
        };
        let (alternative, cost, cost_basis) =
            if status == "missed" || !drawn { alternative(kind, count, length_ft, perimeter, r) } else { (String::new(), 0.0, String::new()) };
        out.push(Hold {
            pour: pour.map(|p| p.id.clone()).unwrap_or_default(),
            date: pour.map(|p| p.date.clone()).unwrap_or_default(),
            key,
            element: e.id.clone(),
            level: e.level.clone(),
            at,
            kind: kind.into(),
            size: size.into(),
            count,
            length_ft,
            owner: if owner.trim().is_empty() { e.owner.clone() } else { owner.trim().to_ascii_lowercase() },
            source: source.into(),
            signed,
            status,
            days,
            alternative,
            cost,
            cost_basis,
        });
    };
    let cast_in = ["ufer", "conduit", "stub_up", "sleeve", "anchor_bolt", "embed_plate", "ground_rod", "bond"];
    for x in els {
        let e = &x.e;
        for m in e.embeds.iter().take(MAX_CONCRETE).filter(|m| cast_in.contains(&m.kind.as_str())) {
            add(&mut out, e, &m.kind, &m.size, m.count.max(1), m.length_ft, &m.owner, &m.source, m.at.unwrap_or_else(|| centre(e)), true, None);
        }
        // Conduit run below the floor of a slab's level: under-slab, in before the slab.
        if e.kind == "slab" {
            for run in sp.mep.runs.iter().take(MAX_CONCRETE).filter(|m| m.level.eq_ignore_ascii_case(&e.level)) {
                let conduit = sp.mep.systems.iter().any(|s| s.name == run.system && s.kind == MepKind::Conduit);
                if !conduit || !run.bottoms_ft.iter().any(|b| b.is_finite() && *b < 0.0) {
                    continue;
                }
                let Some(p) = run.points.first().copied() else { continue };
                if !inside(e, p, upf(sp)) && !run.points.iter().any(|q| inside(e, *q, upf(sp))) {
                    continue;
                }
                let len: f64 = run
                    .points
                    .windows(2)
                    .map(|w| match w {
                        [a, b] => ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt() / upf(sp),
                        _ => 0.0,
                    })
                    .sum();
                let size = if run.size_in[0] > 0.0 { format!("{}\"", r2(run.size_in[0])) } else { String::new() };
                add(&mut out, e, "conduit", &size, 1, len, "electrical", &format!("under-slab run {}", run.system), p, true, None);
            }
        }
    }
    // The grounding's concrete-encased electrodes (Q329): each on the checklist of the
    // foundation it names or sits in.
    let encased = encased_electrodes(sp);
    for g in &encased {
        if let Some(x) = electrode_host(sp, els, g) {
            let source = format!("grounding electrode {}", g.tag);
            add(&mut out, &x.e, "ufer", &g.conductor, 1, g.length_ft.max(0.0), "electrical", &source, g.at, true, Some(*g));
        }
    }
    // No encased electrode anywhere: the first footing pour's missing item.
    let foundations: Vec<&Element> = els.iter().filter(|x| matches!(x.e.kind.as_str(), "footing" | "grade_beam")).collect();
    if r.ufer_required && !foundations.is_empty() && encased.is_empty() && !els.iter().any(|x| x.e.embeds.iter().any(|m| m.kind == "ufer")) {
        let first = foundations.iter().min_by_key(|x| pour_of(&x.e.pour).and_then(|p| day(&p.date)).unwrap_or(i64::MAX)).map(|x| &x.e);
        if let Some(e) = first {
            add(&mut out, e, "ufer", "", 1, r.ufer_min_ft, "electrical", "required: the building has footings", centre(e), false, None);
        }
    }
    out
}

/// The grounding system's concrete-encased electrodes (Q329).
fn encased_electrodes(sp: &BuildingSpec) -> Vec<&Electrode> {
    sp.mep
        .grounding
        .iter()
        .flat_map(|g| g.electrodes.iter().take(MAX_CONCRETE))
        .filter(|g| g.kind.trim().eq_ignore_ascii_case("concrete_encased"))
        .collect()
}

/// The foundation a concrete-encased electrode is cast in: the element it names, else the
/// footing, grade beam, pier or pole base on its level under its connection point.
fn electrode_host<'a>(sp: &BuildingSpec, els: &'a [Element], g: &Electrode) -> Option<&'a Element> {
    let foundation = |x: &&Element| matches!(x.e.kind.as_str(), "footing" | "grade_beam" | "pier" | "pole_base");
    let named = g.element.trim();
    if !named.is_empty() {
        return els.iter().filter(foundation).find(|x| x.e.id.eq_ignore_ascii_case(named));
    }
    let same_level = |x: &&Element| g.level.trim().is_empty() || x.e.level.eq_ignore_ascii_case(g.level.trim());
    els.iter().filter(foundation).filter(same_level).find(|x| g.at.iter().all(|v| v.is_finite()) && inside(&x.e, g.at, upf(sp)))
}

fn hold_findings(hs: &[Hold], out: &mut Vec<Finding>) {
    for h in hs {
        let what = if h.size.is_empty() { h.kind.replace('_', " ") } else { format!("{} ({})", h.kind.replace('_', " "), h.size) };
        let pour = if h.pour.is_empty() { "no pour".to_string() } else { format!("pour {} ({})", h.pour, h.date) };
        let (check, severity, message) = match h.status {
            "missed" => (
                "hold point",
                "error",
                format!(
                    "{}: the {what} was not confirmed before {pour}; it can't be cast in now. Instead: {} (about {})",
                    h.element,
                    h.alternative,
                    r2(h.cost)
                ),
            ),
            "not drawn" if h.kind == "ufer" => (
                "ufer",
                "error",
                format!(
                    "No concrete-encased electrode in any footing: it must be in {} before {pour}, or a ground ring is needed later (about {})",
                    h.element,
                    r2(h.cost)
                ),
            ),
            "due" => (
                "hold point",
                "warning",
                format!("{}: the {what} must be in and signed off before {pour} (in {} days)", h.element, h.days.unwrap_or(0)),
            ),
            _ => continue,
        };
        if out.len() < MAX_FINDINGS {
            out.push(Finding {
                check,
                severity,
                about: h.element.clone(),
                level: h.level.clone(),
                at: h.at,
                message,
                fix: if h.status == "due" { "Install it and sign off the hold point".into() } else { h.alternative.clone() },
            });
        }
    }
}

/// Everything the concrete is checked for, and the hold points.
pub fn review(sp: &BuildingSpec, types: &BimTypes, only: Option<&str>, today: Option<i64>) -> (Vec<Finding>, Vec<Hold>) {
    let r = Rules::of(sp);
    let els = elements(sp, types);
    let mut out = Vec::new();
    pad_findings(sp, types, &els, only, &r, &mut out);
    let hs = holds(sp, &els, today, &r);
    if only.is_none() {
        embed_findings(sp, &els, &r, &mut out);
        hold_findings(&hs, &mut out);
        for x in els.iter().filter(|x| x.e.owner.trim().is_empty() && x.e.kind != "pad") {
            if out.len() < MAX_FINDINGS {
                out.push(Finding {
                    check: "scope",
                    severity: "warning",
                    about: x.e.id.clone(),
                    level: x.e.level.clone(),
                    at: centre(&x.e),
                    message: format!("{}: no owner; the takeoff lists it as unassigned", x.e.id),
                    fix: "Assign its scope".into(),
                });
            }
        }
    }
    (out, hs)
}

// ---------------- 3D ----------------

/// Bars drawn per element, at most (a big slab's mats are shown in part).
const MAX_DRAWN_BARS: usize = 400;

/// The concrete in 3D (`model.build`): each element as a solid on `3D-CONC-<level>`, its rebar
/// mats and long bars on `3D-CONC-REBAR-<level>`, what is cast in (the electrode, sleeves,
/// anchor bolts, conduit stub-ups) on `3D-CONC-EMBED-<level>`, so clashes and cover show in 3D
/// and in section before the pour. Feeders' stub-ups into gear are drawn by the MEP build.
pub(crate) fn build(d: &mut cadaddy_doc::Drawing, sp: &BuildingSpec) -> Value {
    use cadaddy_doc::{Common, EntityKind, Handle, RawTag, Space};
    use cadaddy_geom::Vec2;
    let els = elements(sp, &d.bim_types);
    let upf = upf(sp);
    let r = Rules::of(sp);
    let gear = super::gear::Rules::of(sp);
    let t = |code: i32, v: String| RawTag { code, value: v };
    let cap = |v: String| -> String { v.chars().take(250).collect() };
    let (mut solids, mut bars, mut embeds) = (0usize, 0usize, 0usize);
    for x in els.iter().take(MAX_CONCRETE) {
        let e = &x.e;
        let Some(level) = sp.level(&e.level) else { continue };
        let depth = e.depth_in / 12.0 * upf;
        let z1 = (level.elevation_ft + e.top_ft) * upf;
        let z0 = z1 - depth;
        if !(z0.is_finite() && z1.is_finite() && depth > 0.0) {
            continue;
        }
        let lv = super::mep::sanitise(&level.name, 16);
        let xd = |part: &str| {
            let mut v = vec![
                t(1001, super::wallgen::APP.into()),
                t(1000, "concrete".into()),
                t(1000, level.name.clone()),
                t(1000, Handle(0).hex()),
                t(1000, cap(format!("kind:{}", e.kind))),
                t(1000, cap(format!("part:{part}"))),
                t(1000, cap(format!("tag:{}", e.id))),
            ];
            if e.path.len() >= 2 {
                v.push(t(1000, "prop:shape=strip".into()));
            }
            for (k, val) in [("owner", &e.owner), ("pour", &e.pour), ("equipment", &e.equipment), ("source", &e.source)] {
                if !val.is_empty() {
                    v.push(t(1000, cap(format!("prop:{k}={val}"))));
                }
            }
            v
        };
        let lo = Vec2::new(e.min[0].min(e.max[0]), e.min[1].min(e.max[1]));
        let hi = Vec2::new(e.min[0].max(e.max[0]), e.min[1].max(e.max[1]));
        let pts: Vec<Vec2> = e.path.iter().take(MAX_CONCRETE).filter(|p| p.iter().all(|v| v.is_finite())).map(|p| Vec2::new(p[0], p[1])).collect();
        let width = e.width_in / 12.0 * upf;
        // The element.
        let mut m = super::wallgen::empty_mesh();
        if pts.len() >= 2 {
            for w in pts.windows(2) {
                if let [a, b] = w {
                    super::wallgen::add_box(&mut m, *a, *b, width, z0, z1);
                }
            }
        } else if e.round {
            super::mep::add_upright_cylinder(&mut m, (lo + hi) * 0.5, (hi.x - lo.x).min(hi.y - lo.y) / 2.0, z0, z1);
        } else {
            let cy = (lo.y + hi.y) / 2.0;
            super::wallgen::add_box(&mut m, Vec2::new(lo.x, cy), Vec2::new(hi.x, cy), hi.y - lo.y, z0, z1);
        }
        if !m.faces.is_empty() {
            let layer = format!("3D-CONC-{lv}");
            super::mep::ensure_layer(d, &layer, 8);
            if d.add(&Space::Model, Common { layer: layer.into(), xdata: xd("element").into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
                solids += 1;
            }
        }
        // Rebar: mats across a box, long bars along a path or up a pier.
        let mut rm = super::wallgen::empty_mesh();
        let mut n = 0usize;
        let bar_box = |m: &mut cadaddy_doc::Mesh, n: &mut usize, a: Vec2, b: Vec2, db: f64, top_in: f64| {
            if *n < MAX_DRAWN_BARS {
                let zt = z1 - top_in / 12.0 * upf;
                super::wallgen::add_box(m, a, b, db / 12.0 * upf, zt - db / 12.0 * upf, zt);
                *n += 1;
            }
        };
        for b in e.rebar.iter().take(64) {
            let Some((db, _)) = bar(&b.bar) else { continue };
            let cover = (if b.cover_in > 0.0 && b.cover_in.is_finite() { b.cover_in } else { r.cover(&e.kind) }) / 12.0 * upf;
            let step = b.spacing_in / 12.0 * upf;
            match b.direction.as_str() {
                "x" | "y" | "each_way" if pts.len() < 2 && !e.round => {
                    let bands: Vec<(String, f64, f64)> = mats(e, &r).into_iter().filter(|(name, _, _)| name.contains(&b.bar)).collect();
                    let (x0, x1, y0, y1) = (lo.x + cover, hi.x - cover, lo.y + cover, hi.y - cover);
                    if !(step > 0.0 && step.is_finite() && x1 > x0 && y1 > y0) {
                        continue;
                    }
                    for (_, top_in, _) in bands {
                        if b.direction != "y" {
                            let mut y = y0;
                            while y <= y1 + 1e-9 && n < MAX_DRAWN_BARS {
                                bar_box(&mut rm, &mut n, Vec2::new(x0, y), Vec2::new(x1, y), db, top_in);
                                y += step;
                            }
                        }
                        if b.direction != "x" {
                            let mut x = x0;
                            while x <= x1 + 1e-9 && n < MAX_DRAWN_BARS {
                                bar_box(
                                    &mut rm,
                                    &mut n,
                                    Vec2::new(x, y0),
                                    Vec2::new(x, y1),
                                    db,
                                    top_in + if b.direction == "each_way" { db } else { 0.0 },
                                );
                                x += step;
                            }
                        }
                    }
                }
                "long" if pts.len() >= 2 => {
                    let k = b.count.clamp(1, 64);
                    for i in 0..k {
                        // Spread across the width at the bottom.
                        let off = if k == 1 { 0.0 } else { -width / 2.0 + cover + (width - 2.0 * cover) * f64::from(i) / f64::from(k - 1) };
                        for w in pts.windows(2) {
                            if let [a, c] = w {
                                let dir = *c - *a;
                                let len = a.dist(*c);
                                if len <= 0.0 {
                                    continue;
                                }
                                let nrm = dir.perp() * (off / len);
                                bar_box(&mut rm, &mut n, *a + nrm, *c + nrm, db, e.depth_in - cover / upf * 12.0 - db);
                            }
                        }
                    }
                }
                "long" if e.round || matches!(e.kind.as_str(), "pier" | "pole_base") => {
                    let c = (lo + hi) * 0.5;
                    let rad = ((hi.x - lo.x).min(hi.y - lo.y) / 2.0 - cover).max(0.0);
                    let k = b.count.clamp(1, 64);
                    for i in 0..k {
                        if n >= MAX_DRAWN_BARS {
                            break;
                        }
                        let a = f64::from(i) * std::f64::consts::TAU / f64::from(k);
                        let p = c + Vec2::new(rad * a.cos(), rad * a.sin());
                        super::mep::add_upright_cylinder(&mut rm, p, db / 24.0 * upf, z0 + cover, z1 - cover);
                        n += 1;
                    }
                }
                _ => {}
            }
        }
        if !rm.faces.is_empty() {
            let layer = format!("3D-CONC-REBAR-{lv}");
            super::mep::ensure_layer(d, &layer, 1);
            if d.add(&Space::Model, Common { layer: layer.into(), xdata: xd("rebar").into(), ..Common::default() }, EntityKind::Mesh(rm)).is_ok() {
                bars += n;
            }
        }
        // What is cast in.
        let ctr = Vec2::new(centre(e)[0], centre(e)[1]);
        for m in e.embeds.iter().take(MAX_CONCRETE) {
            // Feeders' stub-ups into gear are the MEP build's.
            if m.kind == "stub_up" && m.source.starts_with("feeder ") {
                continue;
            }
            let at = m.at.map_or(ctr, |p| Vec2::new(p[0], p[1]));
            let od = embed_od(m, &gear).max(0.25) / 12.0 * upf;
            let c = if m.depth_in > 0.0 { m.depth_in } else { e.depth_in / 2.0 } / 12.0 * upf;
            let mut em = super::wallgen::empty_mesh();
            match m.kind.as_str() {
                "ufer" => {
                    // Along the footing, at its depth: the path, or the box's longer side.
                    let line: Vec<Vec2> = if pts.len() >= 2 {
                        pts.clone()
                    } else if hi.x - lo.x >= hi.y - lo.y {
                        vec![Vec2::new(lo.x + od, at.y), Vec2::new(hi.x - od, at.y)]
                    } else {
                        vec![Vec2::new(at.x, lo.y + od), Vec2::new(at.x, hi.y - od)]
                    };
                    for w in line.windows(2) {
                        if let [a, b] = w {
                            super::wallgen::add_box(&mut em, *a, *b, od, z1 - c - od / 2.0, z1 - c + od / 2.0);
                        }
                    }
                }
                "anchor_bolt" => {
                    // At the equipment's corners, inset, else round the element's centre.
                    let k = m.count.clamp(1, 32);
                    let eq = sp.mep.equipment.iter().find(|q| !e.equipment.is_empty() && q.tag.eq_ignore_ascii_case(&e.equipment));
                    let (a, b) = eq.map_or((lo, hi), |q| {
                        (Vec2::new(q.min[0].min(q.max[0]), q.min[1].min(q.max[1])), Vec2::new(q.min[0].max(q.max[0]), q.min[1].max(q.max[1])))
                    });
                    let inset = 3.0 / 12.0 * upf;
                    let corners = [
                        Vec2::new(a.x + inset, a.y + inset),
                        Vec2::new(b.x - inset, a.y + inset),
                        Vec2::new(b.x - inset, b.y - inset),
                        Vec2::new(a.x + inset, b.y - inset),
                    ];
                    let emb = if m.embed_in > 0.0 { m.embed_in } else { e.depth_in * 0.75 } / 12.0 * upf;
                    let proj = if m.projection_in > 0.0 { m.projection_in } else { 2.0 } / 12.0 * upf;
                    for i in 0..k {
                        let p = if m.at.is_some() && k == 1 { at } else { corners.get(i as usize % 4).copied().unwrap_or(at) };
                        super::mep::add_upright_cylinder(&mut em, p, od / 2.0, z1 - emb, z1 + proj);
                    }
                }
                "stub_up" | "sleeve" | "ground_rod" => {
                    let (b0, b1) = if m.kind == "ground_rod" {
                        (z0 - 8.0 * upf, z0)
                    } else {
                        (z0 - if m.kind == "stub_up" { upf } else { 0.0 }, z1 + 0.5 * upf)
                    };
                    super::mep::add_upright_cylinder(&mut em, at, od / 2.0, b0, b1);
                }
                "conduit" => {
                    // A run across the element at its depth: along the path or the longer side.
                    let len = if m.length_ft > 0.0 { m.length_ft * upf } else { (hi.x - lo.x).max(hi.y - lo.y) };
                    let (a, b) = if hi.x - lo.x >= hi.y - lo.y {
                        (at - Vec2::new(len / 2.0, 0.0), at + Vec2::new(len / 2.0, 0.0))
                    } else {
                        (at - Vec2::new(0.0, len / 2.0), at + Vec2::new(0.0, len / 2.0))
                    };
                    super::wallgen::add_box(&mut em, a, b, od, z1 - c - od / 2.0, z1 - c + od / 2.0);
                }
                _ => {
                    super::wallgen::add_box(&mut em, at - Vec2::new(od, 0.0), at + Vec2::new(od, 0.0), od, z1 - c - od / 2.0, z1);
                }
            }
            if em.faces.is_empty() {
                continue;
            }
            let layer = format!("3D-CONC-EMBED-{lv}");
            super::mep::ensure_layer(d, &layer, 3);
            let mut xdata = xd(&format!("embed:{}", m.kind));
            if !m.size.is_empty() {
                xdata.push(t(1000, cap(format!("prop:size={}", m.size))));
            }
            if d.add(&Space::Model, Common { layer: layer.into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(em)).is_ok() {
                embeds += 1;
            }
        }
    }
    json!({ "concrete": solids, "rebarBars": bars, "concreteEmbeds": embeds })
}

// ---------------- commands ----------------

fn spec(cmd: &str, s: &Session) -> Result<BuildingSpec> {
    let text = s.doc()?.building_spec.clone().ok_or_else(|| bad(cmd, "the drawing has no building spec (bspec.set)"))?;
    BuildingSpec::from_json(&text).map_err(|e| bad(cmd, e))
}

fn today(cmd: &str, p: &Value) -> Result<Option<i64>> {
    match str_param(p, "today").map(str::trim).filter(|t| !t.is_empty()) {
        None => Ok(None),
        Some(t) => day(t).map(Some).ok_or_else(|| bad(cmd, "`today` is a date, YYYY-MM-DD")),
    }
}

fn element_json(x: &Element, upf: f64, r: &Rules) -> Value {
    let q = quantities(&x.e, upf, r);
    let s = shape(&x.e, upf);
    json!({
        "id": x.e.id, "kind": x.e.kind, "level": x.e.level, "owner": x.e.owner, "scope": scope_of(r, &x.e.owner), "pour": x.e.pour,
        "equipment": x.e.equipment, "derived": x.derived, "depth_in": x.e.depth_in, "top_ft": x.e.top_ft,
        "size_ft": [r2(s.a), r2(s.b)], "area_sqft": r2(s.area),
        "footprint_in": x.footprint.as_ref().map(|(f, _)| [r2(f[0]), r2(f[1])]), "footprint_source": x.footprint.as_ref().map(|(_, s)| s.clone()),
        "cy": r2(q.cy), "cy_with_waste": r2(q.cy_waste), "forms_sf": r2(q.forms_sf), "edge_lf": r2(q.edge_lf), "round_form_lf": r2(q.round_form_lf),
        "chamfer_lf": r2(q.chamfer_lf),
        "rebar": q.rebar.iter().map(|(b, (lf, lb))| json!({"bar": b, "lf": r2(*lf), "lb": r2(*lb)})).collect::<Vec<_>>(),
        "mesh_sf": r2(q.mesh_sf), "finish_sf": r2(q.finish_sf), "curing_sf": r2(q.curing_sf),
        "embeds": q.embeds.iter().map(|((k, size), (n, ft))| json!({"kind": k, "size": size, "count": n, "length_ft": r2(*ft)})).collect::<Vec<_>>(),
        "source": x.e.source,
    })
}

fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let sp = spec("concrete.list", s)?;
    let types = &s.doc()?.bim_types;
    let r = Rules::of(&sp);
    let upf = upf(&sp);
    let els = elements(&sp, types);
    let equipment: Vec<Value> = sp
        .mep
        .equipment
        .iter()
        .take(MAX_CONCRETE)
        .filter_map(|e| {
            let why = needs_pad(e)?;
            let d = decision(&sp, &e.tag);
            let pad = els.iter().find(|x| x.e.kind == "pad" && x.e.equipment.eq_ignore_ascii_case(&e.tag));
            let (fp, src) = equipment_footprint(&sp, types, e);
            Some(json!({
                "tag": e.tag, "kind": e.kind, "level": e.level, "why": why,
                "required": d.and_then(|d| d.required), "decided": d.is_some_and(|d| d.required.is_some()) || pad.is_some_and(|p| !p.derived),
                "basis": d.map(|d| d.basis.clone()), "owner": pad.map(|p| p.e.owner.clone()).or_else(|| d.map(|d| d.owner.clone())),
                "footprint_in": [r2(fp[0]), r2(fp[1])], "footprint_source": src,
                "pad": pad.map(|p| p.e.id.clone()),
                "pad_in": pad.map(|p| { let s = shape(&p.e, upf); [r2(s.a * 12.0), r2(s.b * 12.0)] }),
                "height_in": pad.map(|p| p.e.depth_in),
            }))
        })
        .collect();
    Ok(json!({
        "elements": els.iter().map(|x| element_json(x, upf, &r)).collect::<Vec<_>>(),
        "equipment": equipment,
        "pours": sp.concrete.pours,
        "rules": r.json(),
    }))
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "concrete.set";
    let mut sp = spec(CMD, s)?;
    let c = &mut sp.concrete;
    let mut changed = Vec::new();
    let full = |n: usize| if n >= MAX_CONCRETE { Err(bad(CMD, format!("at most {MAX_CONCRETE} items"))) } else { Ok(()) };
    for v in p.get("elements").and_then(Value::as_array).into_iter().flatten().chain(p.get("element").filter(|v| v.is_object())).take(MAX_CONCRETE) {
        let e: ConcreteElement = serde_json::from_value(v.clone()).map_err(|e| bad(CMD, format!("element: {e}")))?;
        if e.id.trim().is_empty() {
            return Err(bad(CMD, "an element needs an id"));
        }
        if e.embeds.len() > MAX_CONCRETE || e.rebar.len() > 64 || e.path.len() > MAX_CONCRETE {
            return Err(bad(CMD, "too many embeds, bars or path points"));
        }
        match c.elements.iter_mut().find(|x| x.id.eq_ignore_ascii_case(e.id.trim())) {
            Some(x) => *x = e.clone(),
            None => {
                full(c.elements.len())?;
                c.elements.push(e.clone());
            }
        }
        changed.push(format!("element {}", e.id));
    }
    for v in p.get("pads").and_then(Value::as_array).into_iter().flatten().chain(p.get("pad").filter(|v| v.is_object())).take(MAX_CONCRETE) {
        let d: PadDecision = serde_json::from_value(v.clone()).map_err(|e| bad(CMD, format!("pad: {e}")))?;
        if d.equipment.trim().is_empty() {
            return Err(bad(CMD, "a pad decision needs the equipment's tag"));
        }
        match c.pads.iter_mut().find(|x| x.equipment.eq_ignore_ascii_case(d.equipment.trim())) {
            Some(x) => *x = d.clone(),
            None => {
                full(c.pads.len())?;
                c.pads.push(d.clone());
            }
        }
        changed.push(format!("pad {}", d.equipment));
    }
    for v in p.get("pours").and_then(Value::as_array).into_iter().flatten().chain(p.get("pour").filter(|v| v.is_object())).take(MAX_CONCRETE) {
        let mut q: Pour = serde_json::from_value(v.clone()).map_err(|e| bad(CMD, format!("pour: {e}")))?;
        if q.id.trim().is_empty() {
            return Err(bad(CMD, "a pour needs an id"));
        }
        if !q.date.trim().is_empty() && day(&q.date).is_none() {
            return Err(bad(CMD, format!("pour {}: the date is YYYY-MM-DD", q.id.chars().take(40).collect::<String>())));
        }
        q.signoffs.truncate(MAX_CONCRETE);
        match c.pours.iter_mut().find(|x| x.id.eq_ignore_ascii_case(q.id.trim())) {
            Some(x) => {
                // The sign-offs stay unless the pour gives its own.
                if q.signoffs.is_empty() {
                    q.signoffs = std::mem::take(&mut x.signoffs);
                }
                *x = q.clone();
            }
            None => {
                full(c.pours.len())?;
                c.pours.push(q.clone());
            }
        }
        changed.push(format!("pour {}", q.id));
    }
    let ids = |key: &str| -> Vec<String> {
        p.get(key).and_then(Value::as_array).into_iter().flatten().take(MAX_CONCRETE).filter_map(Value::as_str).map(str::to_string).collect()
    };
    for id in ids("remove_elements") {
        let n = c.elements.len();
        c.elements.retain(|x| !x.id.eq_ignore_ascii_case(&id));
        if c.elements.len() != n {
            changed.push(format!("removed element {id}"));
        }
    }
    for id in ids("remove_pads") {
        let n = c.pads.len();
        c.pads.retain(|x| !x.equipment.eq_ignore_ascii_case(&id));
        if c.pads.len() != n {
            changed.push(format!("removed pad {id}"));
        }
    }
    for id in ids("remove_pours") {
        let n = c.pours.len();
        c.pours.retain(|x| !x.id.eq_ignore_ascii_case(&id));
        if c.pours.len() != n {
            changed.push(format!("removed pour {id}"));
        }
    }
    if let Some(v) = p.get("rules") {
        c.rules = if v.is_null() || v.as_object().is_some_and(|o| o.is_empty()) {
            None
        } else if v.is_object() {
            Some(v.clone())
        } else {
            return Err(bad(CMD, "`rules` is an object"));
        };
        changed.push("rules".into());
    }
    if !changed.is_empty() {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(json!({ "changed": changed, "elements": sp.concrete.elements.len(), "pads": sp.concrete.pads.len(), "pours": sp.concrete.pours.len() }))
}

fn run_check(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec("concrete.check", s)?;
    let only = str_param(p, "equipment").map(str::trim).filter(|t| !t.is_empty());
    let t = today("concrete.check", p)?;
    let (findings, hs) = review(&sp, &s.doc()?.bim_types, only, t);
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    Ok(json!({
        "ok": errors == 0, "errors": errors, "warnings": findings.iter().filter(|f| f.severity == "warning").count(),
        "findings": findings.iter().map(Finding::json).collect::<Vec<_>>(),
        "holds": hs.iter().map(Hold::json).collect::<Vec<_>>(),
    }))
}

fn hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", h as u32)
}

fn run_review(s: &mut Session, p: &Value) -> Result<Value> {
    let mut sp = spec("concrete.review", s)?;
    let t = today("concrete.review", p)?;
    let (findings, _) = review(&sp, &s.doc()?.bim_types, None, t);
    let mut added = Vec::new();
    for f in findings.iter().filter(|f| f.severity == "error") {
        let Some(kind) = f.rfi_kind() else { continue };
        // Stable over the wording: the item, the check and the first words.
        let id = format!("RFI-C{}", hash(&format!("{}|{}", f.about, f.check)));
        if sp.rfis.iter().any(|r| r.id == id) || sp.rfis.len() >= cadaddy_walk3d::rfi::MAX_RFIS {
            continue;
        }
        let assumed = match kind {
            cadaddy_walk3d::RfiKind::PadUndecided => "Held: no pad is formed or ordered for it until this is answered.",
            cadaddy_walk3d::RfiKind::PourHold => "Held: the pour doesn't go ahead without the item, or the alternative is accepted.",
            _ => "Held: the element isn't poured as drawn until this is answered.",
        };
        sp.rfis.push(cadaddy_walk3d::Rfi {
            id: id.clone(),
            kind: Some(kind),
            severity: kind.severity(),
            level: f.level.clone(),
            at: f.at,
            about: format!("{} ({})", f.about, f.check),
            issue: f.message.clone(),
            assumed: assumed.into(),
            proposed: (!f.fix.is_empty()).then(|| f.fix.clone()),
            ..cadaddy_walk3d::Rfi::default()
        });
        added.push(id);
    }
    if !added.is_empty() {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    Ok(json!({ "errors": errors, "rfis": added, "findings": findings.iter().map(Finding::json).collect::<Vec<_>>() }))
}

fn csv(rows: &[Vec<String>]) -> String {
    let field = |t: &str| {
        // Formulas defused, quotes doubled.
        let t = if t.starts_with(['=', '+', '-', '@']) && t.parse::<f64>().is_err() { format!("'{t}") } else { t.to_string() };
        if t.contains([',', '"', '\n']) { format!("\"{}\"", t.replace('"', "\"\"")) } else { t }
    };
    rows.iter().map(|r| r.iter().map(|c| field(c)).collect::<Vec<_>>().join(",")).collect::<Vec<_>>().join("\n")
}

fn run_qty(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "concrete.qty";
    let sp = spec(CMD, s)?;
    let r = Rules::of(&sp);
    let upf = upf(&sp);
    let level = str_param(p, "level").map(str::trim).filter(|l| !l.is_empty());
    let scope = str_param(p, "scope").unwrap_or("all").trim().to_ascii_lowercase();
    if !matches!(scope.as_str(), "all" | "ours" | "others") {
        return Err(bad(CMD, "`scope` is all, ours or others"));
    }
    let els: Vec<Element> = elements(&sp, &s.doc()?.bim_types)
        .into_iter()
        .filter(|x| match scope.as_str() {
            "ours" => r.ours(&x.e.owner),
            "others" => !r.ours(&x.e.owner),
            _ => true,
        })
        .collect();
    let rows = rows(&sp, &els, level);
    let mut tot = Qty::default();
    let mut ours_cy = 0.0;
    for x in els.iter().filter(|x| level.is_none_or(|l| x.e.level.eq_ignore_ascii_case(l))) {
        let q = quantities(&x.e, upf, &r);
        tot.cy += q.cy;
        tot.cy_waste += q.cy_waste;
        tot.forms_sf += q.forms_sf;
        tot.edge_lf += q.edge_lf;
        tot.chamfer_lf += q.chamfer_lf;
        tot.mesh_sf += q.mesh_sf;
        if r.ours(&x.e.owner) {
            ours_cy += q.cy_waste;
        }
        for (b, (lf, lb)) in q.rebar {
            let v = tot.rebar.entry(b).or_default();
            v.0 += lf;
            v.1 += lb;
        }
    }
    let mut out = json!({
        "elements": els.iter().filter(|x| level.is_none_or(|l| x.e.level.eq_ignore_ascii_case(l))).map(|x| element_json(x, upf, &r)).collect::<Vec<_>>(),
        "totals": {
            "cy": r2(tot.cy), "cy_with_waste": r2(tot.cy_waste), "cy_ours_with_waste": r2(ours_cy), "forms_sf": r2(tot.forms_sf),
            "edge_lf": r2(tot.edge_lf), "chamfer_lf": r2(tot.chamfer_lf), "mesh_sf": r2(tot.mesh_sf),
            "rebar": tot.rebar.iter().map(|(b, (lf, lb))| json!({"bar": b, "lf": r2(*lf), "lb": r2(*lb)})).collect::<Vec<_>>(),
            "rebar_lb": r2(tot.rebar.values().map(|v| v.1).sum()),
        },
        "rows": rows.len().saturating_sub(1),
        "csv": csv(&rows),
    });
    if let Some(path) = str_param(p, "file").filter(|x| !x.trim().is_empty()) {
        let format = match str_param(p, "format") {
            Some(f) => f.to_ascii_lowercase(),
            None => std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("csv").to_ascii_lowercase(),
        };
        let bytes = match format.as_str() {
            "csv" => csv(&rows).into_bytes(),
            "xlsx" => super::table::xlsx::write(&rows, "Concrete").map_err(|e| bad(CMD, e))?,
            _ => return Err(bad(CMD, "`format` is csv or xlsx")),
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::writefile::write(path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
            out["path"] = json!(path);
        }
        #[cfg(target_arch = "wasm32")]
        {
            out["data"] = json!(super::file::base64_encode(&bytes));
        }
    }
    s.echo(format!(
        "Concrete: {} cy with waste ({} cy ours), {} sf of forms, {} lb of rebar.",
        out["totals"]["cy_with_waste"],
        r2(ours_cy),
        out["totals"]["forms_sf"],
        out["totals"]["rebar_lb"]
    ));
    Ok(out)
}

fn run_holds(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = spec("concrete.holds", s)?;
    let t = today("concrete.holds", p)?;
    let only = str_param(p, "pour").map(str::trim).filter(|x| !x.is_empty());
    let els = elements(&sp, &s.doc()?.bim_types);
    let hs = holds(&sp, &els, t, &Rules::of(&sp));
    let mut pours: Vec<Value> = Vec::new();
    for q in sp.concrete.pours.iter().filter(|q| only.is_none_or(|o| q.id.eq_ignore_ascii_case(o))) {
        let items: Vec<&Hold> = hs.iter().filter(|h| h.pour.eq_ignore_ascii_case(&q.id)).collect();
        let open = items.iter().filter(|h| h.status != "signed").count();
        pours.push(json!({
            "pour": q.id, "name": q.name, "date": q.date, "placed": q.placed,
            "days_to_pour": day(&q.date).zip(t).map(|(d, t)| d - t), "items": items.iter().map(|h| h.json()).collect::<Vec<_>>(),
            "open": open, "ready": open == 0,
        }));
    }
    if let Some(o) = only
        && pours.is_empty()
    {
        return Err(bad("concrete.holds", format!("no pour {}", o.chars().take(40).collect::<String>())));
    }
    let unscheduled: Vec<Value> = if only.is_none() { hs.iter().filter(|h| h.pour.is_empty()).map(Hold::json).collect() } else { Vec::new() };
    Ok(json!({ "pours": pours, "unscheduled": unscheduled }))
}

fn run_signoff(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "concrete.signoff";
    let mut sp = spec(CMD, s)?;
    let pour = str_param(p, "pour").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(CMD, "pour required"))?.to_string();
    let item = str_param(p, "item")
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .ok_or_else(|| bad(CMD, "item required (its key in concrete.holds)"))?
        .to_string();
    let undo = bool_or(p, "undo", false);
    let els = elements(&sp, &s.doc()?.bim_types);
    let hs = holds(&sp, &els, None, &Rules::of(&sp));
    let h = hs.iter().find(|h| h.pour.eq_ignore_ascii_case(&pour) && h.key.eq_ignore_ascii_case(&item)).ok_or_else(|| {
        bad(CMD, format!("pour {} has no hold point {}", pour.chars().take(40).collect::<String>(), item.chars().take(80).collect::<String>()))
    })?;
    if h.status == "not drawn" && !undo {
        return Err(bad(CMD, "the item isn't in the model: add it to the element first (concrete.set)"));
    }
    let key = h.key.clone();
    let q = sp.concrete.pours.iter_mut().find(|q| q.id.eq_ignore_ascii_case(&pour)).ok_or_else(|| bad(CMD, "no such pour"))?;
    let had = q.signoffs.iter().any(|x| x.item.eq_ignore_ascii_case(&key));
    if undo {
        q.signoffs.retain(|x| !x.item.eq_ignore_ascii_case(&key));
    } else {
        let by = str_param(p, "by").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(CMD, "by required (who checked it)"))?;
        let on = str_param(p, "on").map(str::trim).unwrap_or("");
        if !on.is_empty() && day(on).is_none() {
            return Err(bad(CMD, "`on` is a date, YYYY-MM-DD"));
        }
        q.signoffs.retain(|x| !x.item.eq_ignore_ascii_case(&key));
        if q.signoffs.len() >= MAX_CONCRETE {
            return Err(bad(CMD, "too many sign-offs on the pour"));
        }
        q.signoffs.push(Signoff { item: key.clone(), by: by.chars().take(80).collect(), on: on.to_string() });
    }
    let changed = undo == had || !undo;
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(json!({ "pour": pour, "item": key, "signed": !undo, "changed": changed }))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("concrete.list", "Concrete Elements", run_list)
            .params("{} → {elements: [{id, kind, level, owner, scope, pour, equipment, derived, depth_in, top_ft, size_ft, area_sqft, footprint_in, footprint_source, cy, cy_with_waste, forms_sf, edge_lf, round_form_lf, chamfer_lf, rebar: [{bar, lf, lb}], mesh_sf, finish_sf, curing_sf, embeds: [{kind, size, count, length_ft}], source}], equipment: [{tag, kind, level, why, required, decided, basis, owner, footprint_in, footprint_source, pad, pad_in, height_in}] (floor- and grade-mounted equipment and its pad), pours, rules}")
            .noundo(),
        CommandSpec::new("concrete.set", "Set Concrete", run_set).params(
            "{element?|elements?: [{id, kind: slab|footing|grade_beam|pier|pole_base|pad|encasement|handhole_base, level, min, max | path + width_in, depth_in, top_ft, round?, chamfer_in?, thickened_edge_in?: [w, d], depressions?: [{min, max, depth_in}], earth_formed?, rebar?: [{bar, direction: x|y|each_way|long|ties|dowels, spacing_in, count, length_in, cover_in, lap_in, layers}], mesh?, embeds?: [{kind: ufer|conduit|stub_up|sleeve|anchor_bolt|embed_plate|ground_rod|bond, size, count, at?, length_ft, diameter_in, depth_in, embed_in, projection_in, owner, source}], owner, source, pour, equipment}] (by id), pad?|pads?: [{equipment, required: true|false|null, basis, owner, margin_in?, height_in?, chamfer_in?, size_in?: [w, d], front?: n|s|e|w, pour?, rebar?, embeds?}] (by equipment), pour?|pours?: [{id, name, date: YYYY-MM-DD, placed}], remove_elements?, remove_pads?, remove_pours?: [id], rules?: {pad_margin_in, pad_margin_min_in, pad_height_in, pad_chamfer_in, pad_rebar, pad_anchor_bolts, pad_anchor_in, waste_pct, small_waste_pct, mesh_lap_pct, cover_in: {kind: in}, lap_db, stock_ft, hook_db, ufer_required, ufer_min_ft, ufer_cover_in, clearance_in, warn_days, our_scope: [owner], costs: {name: rate}} | null} → {changed, elements, pads, pours}",
        ),
        CommandSpec::new("concrete.check", "Check Concrete", run_check)
            .params("{equipment?: tag (its pad only), today?: YYYY-MM-DD (hold points against the pour dates)} → {ok, errors, warnings, findings: [{check: pad decision|pad owner|pad basis|pad size|clearance|ufer|cover|clash|embed|hold point|scope, severity, about, level, message, fix}], holds}")
            .noundo(),
        CommandSpec::new("concrete.review", "Concrete Coordination Review", run_review)
            .params("{today?: YYYY-MM-DD} → as concrete.check, and every failure an RFI (pad_undecided, pour_hold, embed_conflict): {errors, rfis: [id], findings}"),
        CommandSpec::new("concrete.qty", "Concrete Quantities", run_qty)
            .params("{level?, scope?: all|ours|others, file?: path (.csv/.xlsx), format?: csv|xlsx} → {elements, totals: {cy, cy_with_waste, cy_ours_with_waste, forms_sf, edge_lf, chamfer_lf, mesh_sf, rebar: [{bar, lf, lb}], rebar_lb}, rows, csv, path?} (each row's scope: ours, by others (owner) or unassigned)")
            .noundo(),
        CommandSpec::new("concrete.holds", "Pour Hold Points", run_holds)
            .params("{pour?: id, today?: YYYY-MM-DD} → {pours: [{pour, name, date, placed, days_to_pour, items: [{pour, date, item, element, level, kind, size, count, length_ft, owner, source, signed_by, signed_on, status: signed|missed|due|open|not drawn, days_to_pour, alternative, cost, cost_basis}], open, ready}], unscheduled}")
            .noundo(),
        CommandSpec::new("concrete.signoff", "Sign Off Hold Point", run_signoff)
            .params("{pour, item: key from concrete.holds, by: who checked it, on?: YYYY-MM-DD, undo?: bool} → {pour, item, signed, changed}"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printed_sizes_read() {
        assert_eq!(footprint("90\"W x 36\"D x 90\"H"), Some((90.0, 36.0)));
        assert_eq!(footprint("90 x 36 x 90 in"), Some((90.0, 36.0)));
        assert_eq!(footprint("7'-6\" x 3'-0\""), Some((90.0, 36.0)));
        assert_eq!(footprint("7.5 x 3 ft"), Some((90.0, 36.0)));
        let mm = footprint("2286 x 914.4 mm").unwrap();
        assert!((mm.0 - 90.0).abs() < 1e-6 && (mm.1 - 36.0).abs() < 1e-6);
        assert_eq!(footprint("36-1/2\" x 24\""), Some((36.5, 24.0)));
        assert_eq!(footprint("junk"), None);
        assert_eq!(footprint("90 x"), None);
        assert_eq!(footprint("1/0 x 3"), None);
        assert_eq!(bar("#5"), Some((0.625, 1.043)));
        assert_eq!(bar("No. 4"), Some((0.5, 0.668)));
        assert_eq!(bar("#2"), None);
        assert_eq!(trade("2\""), Some(2.0));
        assert_eq!(trade("1-1/4\""), Some(1.25));
    }

    #[test]
    fn dates_count_days() {
        assert_eq!(day("1970-01-01"), Some(0));
        assert_eq!(day("2000-03-01"), Some(11_017));
        assert_eq!(day("2026-03-01").zip(day("2026-02-27")).map(|(a, b)| a - b), Some(2));
        assert_eq!(day("2026-13-01"), None);
        assert_eq!(day("soon"), None);
        assert_eq!(day("2026-1"), None);
    }

    #[test]
    fn laps_only_past_stock_length() {
        assert_eq!(with_laps(18.0, 2.0, 20.0), 18.0);
        // 50 ft from 20 ft bars lapping 2 ft: 2 laps.
        assert_eq!(with_laps(50.0, 2.0, 20.0), 54.0);
        assert_eq!(with_laps(50.0, 25.0, 20.0), 50.0, "a lap longer than stock is ignored");
    }
}
