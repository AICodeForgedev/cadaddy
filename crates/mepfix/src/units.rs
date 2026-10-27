//! Typical guest units on the prints (Q305): the enlarged unit plans sheet (one plan per unit
//! type, drawn from a room of that type in its normal hand, every device, terminal, fixture,
//! casework item and the PTAC tagged where it sits), its tag legend and the unit overrides
//! table; and the unit tag (`UNIT A`, `UNIT A (MIRROR)`) in every guest room on the floor plans.
//! [`crate::unit_reader`] reads them back.

use crate::draw::{Pen, readable, table};
use crate::truth::{Truth, UnitInstance, feet_inches};
use cadaddy_doc::{HAlign, VAlign};
use std::collections::BTreeMap;

/// The enlarged unit plans sheet of the hotel set.
pub const UNIT_SHEET: &str = "A-401";
pub const UNIT_SHEET_TITLE: &str = "ENLARGED GUEST UNIT PLANS";
/// Title of the tag legend on the unit plans sheet.
pub const LEGEND_TITLE: &str = "UNIT PLAN LEGEND";
/// Title of the instance overrides table on the unit plans sheet.
pub const OVERRIDES_TITLE: &str = "UNIT OVERRIDES";
/// Paper inches per foot of the unit plans (1/4" = 1'-0").
const SCALE: f64 = 0.25;
const SCALE_TEXT: &str = "SCALE: 1/4\" = 1'-0\"";

/// The letter a unit type is tagged with on the plans (`A`, `B`, …, in type order).
pub fn type_tag(t: &Truth, type_id: &str) -> Option<String> {
    let i = t.unit_types.iter().position(|u| u.id == type_id)?;
    char::from_u32(u32::from(b'A') + u32::try_from(i % 26).ok()?).map(String::from)
}

/// The unit tag printed in a guest room on the floor plans.
pub fn instance_tag(t: &Truth, u: &UnitInstance) -> Option<String> {
    let tag = type_tag(t, &u.unit_type)?;
    Some(if u.hand == "mirrored" { format!("UNIT {tag} (MIRROR)") } else { format!("UNIT {tag}") })
}

/// One item drawn in a unit: category (`devices`, `terminals`, `fixtures`, `casework`,
/// `equipment`), kind, the tag printed and where it is, building feet.
pub struct UnitItem {
    pub category: &'static str,
    pub kind: String,
    pub tag: String,
    pub at_ft: [f64; 2],
}

/// Everything placed in a room that a unit type counts, with the tag each is printed with.
pub fn room_items(t: &Truth, room: &str) -> Vec<UnitItem> {
    let mut out = Vec::new();
    let mut add =
        |category: &'static str, kind: &str, tag: &str, at: [f64; 2]| out.push(UnitItem { category, kind: kind.into(), tag: tag.into(), at_ft: at });
    for d in t.elec_devices.iter().filter(|d| d.room == room) {
        add("devices", &d.kind, &d.type_tag, [d.at_ft[0], d.at_ft[1]]);
    }
    for x in t.terminals.iter().filter(|x| x.room == room) {
        add("terminals", &x.kind, &x.type_tag, [x.at_ft[0], x.at_ft[1]]);
    }
    for f in t.fixtures.iter().filter(|f| f.room == room) {
        add("fixtures", &f.kind, &f.tag, [f.at_ft[0], f.at_ft[1]]);
    }
    for c in t.casework.iter().filter(|c| c.room == room) {
        add("casework", &c.kind, &c.tag, [(c.min_ft[0] + c.max_ft[0]) / 2.0, (c.min_ft[1] + c.max_ft[1]) / 2.0]);
    }
    for e in t.equip_connections.iter().filter(|e| e.room == room && e.kind == "ptac") {
        add("equipment", "ptac", "PTAC", [e.at_ft[0], e.at_ft[1]]);
    }
    out
}

/// The room each unit type is drawn from: its first instance in the normal hand without
/// overrides (a type with none placed isn't drawn).
fn sample_rooms(t: &Truth) -> Vec<(String, &crate::truth::UnitType, &crate::truth::Room)> {
    t.unit_types
        .iter()
        .filter_map(|ut| {
            let u = t.unit_instances.iter().find(|u| u.unit_type == ut.id && u.hand == "normal" && u.overrides.is_empty())?;
            let rm = t.building.rooms.iter().find(|r| r.id == u.room)?;
            Some((type_tag(t, &ut.id)?, ut, rm))
        })
        .collect()
}

/// `kind` as printed in the legend: `WATER CLOSET`.
fn item_words(kind: &str) -> String {
    kind.replace('_', " ").to_ascii_uppercase()
}

/// What an override row says, by the instance's flag.
fn override_label(u: &UnitInstance) -> (&'static str, &'static str) {
    let on = |k: &str| u.overrides.get(k).is_some_and(|v| v == "true");
    if on("ada") {
        ("ADA", "ROLL-IN SHOWER IN LIEU OF TUB")
    } else if on("end_unit") {
        ("END UNIT", "RECEPTACLE ADDED AT END WALL")
    } else if on("column") {
        ("COLUMN", "COLUMN FURRING IN LIEU OF WARDROBE")
    } else {
        ("OTHER", "")
    }
}

/// The enlarged unit plans sheet: the plans side by side across the top of `area` (paper
/// inches), each titled `UNIT A - KING ROOM` with its size and scale under it; the tag legend and
/// the overrides table below them.
pub fn draw_unit_plans(pen: &mut Pen, t: &Truth, area: [[f64; 2]; 2]) {
    let [[x0, _], [x1, y1]] = area;
    let mut x = x0 + 0.5;
    let mut top = y1 - 0.8;
    let mut row_depth: f64 = 0.0;
    // The legend: tag → (category, kind).
    let mut legend: BTreeMap<String, (&'static str, String)> = BTreeMap::new();
    for (tag, ut, rm) in sample_rooms(t) {
        let (w, d) = (rm.max_ft[0] - rm.min_ft[0], rm.max_ft[1] - rm.min_ft[1]);
        if x + w * SCALE > x1 - 0.5 && x > x0 + 0.5 {
            x = x0 + 0.5;
            top -= row_depth * SCALE + 1.4;
            row_depth = 0.0;
        }
        let lo = [x, top - d * SCALE];
        let paper = |p: [f64; 2]| [lo[0] + (p[0] - rm.min_ft[0]) * SCALE, lo[1] + (p[1] - rm.min_ft[1]) * SCALE];
        pen.prect("ARCH", lo, [x + w * SCALE, top]);
        for it in room_items(t, &rm.id) {
            let p = pen.p(paper(it.at_ft));
            pen.text("TEXT", p, 0.07, &it.tag, 0.0, HAlign::Center, VAlign::Middle);
            legend.entry(it.tag).or_insert((it.category, it.kind));
        }
        pen.ptext("TITLE", [x, lo[1] - 0.3], 0.13, &format!("UNIT {tag} - {}", ut.name), HAlign::Left);
        pen.ptext("TEXT", [x, lo[1] - 0.52], 0.09, &format!("{} x {}", feet_inches(w), feet_inches(d)), HAlign::Left);
        pen.ptext("TEXT", [x, lo[1] - 0.72], 0.08, SCALE_TEXT, HAlign::Left);
        x += w * SCALE + 1.0;
        row_depth = row_depth.max(d);
    }
    // What the overrides bring in that no typical unit has (the ADA unit's shower).
    for f in t.fixtures.iter().filter(|f| t.unit_instances.iter().any(|u| u.room == f.room && !u.overrides.is_empty())) {
        legend.entry(f.tag.clone()).or_insert(("fixtures", f.kind.clone()));
    }
    let category_word = |c: &str| match c {
        "devices" => "DEVICE",
        "terminals" => "TERMINAL",
        "fixtures" => "FIXTURE",
        "casework" => "CASEWORK",
        _ => "EQUIPMENT",
    };
    let mut rows = vec![vec!["TAG".to_string(), "ITEM".into(), "CATEGORY".into()]];
    for (tag, (category, kind)) in &legend {
        let what = if kind == "ptac" { "PTAC UNIT IN WALL SLEEVE WITH RECEPTACLE".to_string() } else { item_words(kind) };
        rows.push(vec![tag.clone(), what, category_word(category).into()]);
    }
    let below = top - row_depth * SCALE - 1.6;
    table(pen, [x0 + 0.5, below], LEGEND_TITLE, &rows);
    // Each instance override: one row per item it changes.
    let mut ov = vec![vec!["ROOM NO.".to_string(), "OVERRIDE".into(), "ITEM".into(), "QTY".into(), "REMARKS".into()]];
    let mut units: Vec<&UnitInstance> = t.unit_instances.iter().filter(|u| !u.overrides.is_empty()).collect();
    units.sort_by(|a, b| a.number.cmp(&b.number));
    for u in units {
        let (label, remark) = override_label(u);
        for (key, value) in &u.overrides {
            let Some((category, kind)) = key.split_once('.') else { continue };
            let Some(tag) = legend.iter().find(|(_, (c, k))| *c == category && k == kind).map(|(tag, _)| tag.clone()) else { continue };
            ov.push(vec![u.number.clone(), label.into(), tag, value.clone(), remark.into()]);
        }
    }
    if ov.len() > 1 {
        table(pen, [x0 + 9.0, below], OVERRIDES_TITLE, &ov);
    }
}

/// The unit tag in every guest room of `level` on a floor plan, under the room's name.
pub fn draw_unit_tags(pen: &mut Pen, t: &Truth, level: &str) {
    for u in t.unit_instances.iter().filter(|u| u.level == level) {
        let (Some(rm), Some(tag)) = (t.building.rooms.iter().find(|r| r.id == u.room), instance_tag(t, u)) else { continue };
        let c = [(rm.min_ft[0] + rm.max_ft[0]) / 2.0, rm.min_ft[1] + 0.3 * (rm.max_ft[1] - rm.min_ft[1])];
        let p = pen.b(c);
        let rot = readable(pen.rot());
        pen.text("ARCH-TEXT", p, 0.08, &tag, rot, HAlign::Center, VAlign::Middle);
    }
}
