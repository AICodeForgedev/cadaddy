//! Unit reader (Q305): typical units from the architectural sheets.
//!
//! - **Unit types** from the enlarged unit plans sheet: each plan's outline (a box on the `ARCH`
//!   layer) with its title under it (`UNIT A - KING ROOM`, then `13'-0" x 24'-0"`); every tag
//!   inside the outline, looked up in the sheet's `UNIT PLAN LEGEND` (tag, item, category), is
//!   one item of the type, counted and kept with its place in the unit (0–1 across and up). The
//!   wet wall is the side the plumbing fixtures are on, the PTAC sleeve the side of the PTAC.
//! - **Instances** from the floor plans: the unit tag in a room (`UNIT A`, `UNIT A (MIRROR)`), the
//!   room's box and the room number and name in it.
//! - **Overrides** from the `UNIT OVERRIDES` table (room number, override, item tag, quantity,
//!   remarks) on the unit plans sheet.
//!
//! Each type, instance and override keeps the sheet it was read from; [`spec_patch`] turns the
//! result into the building spec's `unit_types` and `unit_instances`, keeping what a person set
//! by hand (instance overrides, type edits) across re-runs.

use crate::arch_reader::{Shapes, Txt, cell, contains, length_in, shapes, tables, upper};
use crate::sheet_reader::{self, SheetMap};
use crate::truth::Truth;
use crate::units::{LEGEND_TITLE, OVERRIDES_TITLE};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Who the reader's findings are written by (an override's author).
pub const READER: &str = "unittypes.read";
/// Most unit types read.
const MAX_TYPES: usize = 200;
/// Most instances read from a set.
const MAX_UNITS: usize = 20_000;
/// Most items read in one unit plan.
const MAX_ITEMS: usize = 500;
/// Most instances kept from a stored spec when merging.
const MAX_STORED: usize = 50_000;

/// One tagged item in a unit plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundItem {
    /// `devices`, `terminals`, `fixtures`, `casework` or `equipment`.
    pub category: String,
    pub kind: String,
    pub tag: String,
    /// Where it is in the unit in its normal hand: 0–1 from the left, 0–1 from the bottom.
    pub uv: [f64; 2],
}

/// A unit type read from its enlarged plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundUnitType {
    /// The tag the plans use (`A`).
    pub tag: String,
    pub name: String,
    pub width_ft: f64,
    pub depth_ft: f64,
    /// Category → kind → count per unit.
    pub counts: BTreeMap<String, BTreeMap<String, u32>>,
    pub items: Vec<FoundItem>,
    /// `left` or `right` in the normal hand (empty: no plumbing fixture / no PTAC drawn).
    pub wet_wall_side: String,
    pub hvac_sleeve_side: String,
    pub sheet: String,
}

/// One change an override makes to an instance: `fixtures.bathtub` = `0`, or a flag (`ada` =
/// `true`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundOverride {
    pub key: String,
    pub value: String,
    /// `ADA`, `END UNIT`, `COLUMN`, as the table says.
    pub kind: String,
    pub remark: String,
    pub sheet: String,
}

/// A unit instance read from a floor plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundUnit {
    pub type_tag: String,
    pub level: String,
    pub number: String,
    pub name: String,
    /// `normal` or `mirrored`.
    pub hand: String,
    /// The room's south-west corner (wall centre lines), building feet.
    pub at_ft: [f64; 2],
    pub sheet: String,
    pub overrides: Vec<FoundOverride>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitResult {
    pub types: Vec<FoundUnitType>,
    pub units: Vec<FoundUnit>,
    /// What couldn't be resolved (a unit tag with no plan, an override for an unknown room).
    pub notes: Vec<String>,
}

/// The legend's category word as the spec's category.
fn category(word: &str) -> Option<&'static str> {
    let w = upper(word);
    Some(if w.starts_with("DEVICE") {
        "devices"
    } else if w.starts_with("TERMINAL") {
        "terminals"
    } else if w.starts_with("FIXTURE") {
        "fixtures"
    } else if w.starts_with("CASEWORK") {
        "casework"
    } else if w.starts_with("EQUIPMENT") {
        "equipment"
    } else {
        return None;
    })
}

/// The legend's item words as a kind: `WATER CLOSET` → `water_closet`; a PTAC (with its sleeve
/// and receptacle) → `ptac`.
fn kind_of(item: &str) -> String {
    let w = upper(item);
    if w.split_whitespace().any(|x| x == "PTAC" || x == "VTAC") {
        return "ptac".into();
    }
    w.split_whitespace().collect::<Vec<_>>().join("_").to_ascii_lowercase()
}

/// `UNIT A - KING ROOM` → (`A`, `KING ROOM`).
fn plan_title(s: &str) -> Option<(String, String)> {
    let u = upper(s);
    let rest = u.strip_prefix("UNIT ")?;
    let (tag, name) = rest.split_once(" - ")?;
    let tag = tag.trim();
    (!tag.is_empty() && tag.len() <= 8 && !tag.contains(' ')).then(|| (tag.to_string(), name.trim().to_string()))
}

/// `UNIT A` / `UNIT A (MIRROR)` → (`A`, mirrored).
pub fn unit_tag(s: &str) -> Option<(String, bool)> {
    let u = upper(s);
    let rest = u.strip_prefix("UNIT ")?.trim();
    let (tag, mirrored) = match rest.strip_suffix("(MIRROR)") {
        Some(t) => (t.trim(), true),
        None => (rest, false),
    };
    (!tag.is_empty() && tag.len() <= 8 && !tag.contains(' ') && !tag.contains('-')).then(|| (tag.to_string(), mirrored))
}

/// `13'-0" x 24'-0"` → feet.
fn size_ft(s: &str) -> Option<(f64, f64)> {
    let u = upper(s);
    let (a, b) = u.split_once(" X ")?;
    let (w, d) = (length_in(a)? / 12.0, length_in(b)? / 12.0);
    (w > 0.0 && d > 0.0 && w < 1_000.0 && d < 1_000.0).then_some((w, d))
}

fn side(u: f64) -> &'static str {
    if u < 0.5 { "left" } else { "right" }
}

/// The unit plans sheet: the types (with the legend's items) and the overrides by room number.
fn read_unit_sheet(s: &Shapes, sheet: &str, notes: &mut Vec<String>) -> (Vec<FoundUnitType>, BTreeMap<String, Vec<FoundOverride>>) {
    let mut legend: BTreeMap<String, (&'static str, String)> = BTreeMap::new();
    for t in tables(s, &[LEGEND_TITLE]) {
        let (tc, ic, cc) = (t.col(&|h| h == "TAG"), t.col(&|h| h == "ITEM" || h == "DESCRIPTION"), t.col(&|h| h.starts_with("CATEG")));
        for row in &t.rows {
            let (tag, item) = (upper(&cell(row, tc)), cell(row, ic));
            match category(&cell(row, cc)) {
                Some(c) if !tag.is_empty() && !item.is_empty() => {
                    legend.insert(tag, (c, kind_of(&item)));
                }
                _ => {}
            }
        }
    }
    if legend.is_empty() {
        notes.push(format!("{sheet}: no {LEGEND_TITLE}, so no unit plan items could be named"));
    }
    let outlines: Vec<([f64; 2], [f64; 2])> = s.boxes.iter().filter(|(l, _, _)| l == "ARCH").map(|(_, lo, hi)| (*lo, *hi)).collect();
    let mut types: Vec<FoundUnitType> = Vec::new();
    for title in s.texts.iter() {
        if types.len() >= MAX_TYPES {
            break;
        }
        let Some((tag, name)) = plan_title(&title.s) else { continue };
        // The plan above its title: the nearest outline whose left edge is at the title's start.
        let Some((lo, hi)) = outlines
            .iter()
            .filter(|(lo, hi)| lo[1] > title.at[1] && (lo[0] - title.at[0]).abs() < 0.25 * (hi[0] - lo[0]))
            .min_by(|a, b| (a.0[1] - title.at[1]).total_cmp(&(b.0[1] - title.at[1])))
            .copied()
        else {
            notes.push(format!("{sheet}: {} has no plan above it", title.s));
            continue;
        };
        let (bw, bh) = (hi[0] - lo[0], hi[1] - lo[1]);
        if bw <= 0.0 || bh <= 0.0 {
            continue;
        }
        // Its size: the text right under the title.
        let size = s
            .texts
            .iter()
            .filter(|x| x.at[1] < title.at[1] && x.at[1] > title.at[1] - 4.0 * title.h && (x.at[0] - title.at[0]).abs() < title.h)
            .find_map(|x| size_ft(&x.s));
        let Some((width_ft, depth_ft)) = size else {
            notes.push(format!("{sheet}: {} has no size under its title", title.s));
            continue;
        };
        let mut ty = FoundUnitType { tag: tag.clone(), name, width_ft, depth_ft, sheet: sheet.into(), ..FoundUnitType::default() };
        let inside: Vec<&Txt> = s.texts.iter().filter(|x| contains(lo, hi, x.at, 0.0)).take(MAX_ITEMS).collect();
        for x in inside {
            let Some((c, k)) = legend.get(&upper(&x.s)) else { continue };
            let uv = [(x.at[0] - lo[0]) / bw, (x.at[1] - lo[1]) / bh];
            let n = ty.counts.entry((*c).to_string()).or_default().entry(k.clone()).or_default();
            *n = n.saturating_add(1);
            ty.items.push(FoundItem { category: (*c).into(), kind: k.clone(), tag: upper(&x.s), uv });
        }
        let mean_u = |f: &dyn Fn(&FoundItem) -> bool| {
            let us: Vec<f64> = ty.items.iter().filter(|i| f(i)).map(|i| i.uv[0]).collect();
            (!us.is_empty()).then(|| us.iter().sum::<f64>() / us.len() as f64)
        };
        ty.wet_wall_side = mean_u(&|i| i.category == "fixtures").map(side).unwrap_or_default().into();
        ty.hvac_sleeve_side = mean_u(&|i| i.kind == "ptac").map(side).unwrap_or_default().into();
        if types.iter().any(|t| t.tag == tag) {
            notes.push(format!("{sheet}: unit {tag} is drawn twice; the first plan is kept"));
            continue;
        }
        types.push(ty);
    }
    // The overrides, by room number.
    let mut overrides: BTreeMap<String, Vec<FoundOverride>> = BTreeMap::new();
    for t in tables(s, &[OVERRIDES_TITLE]) {
        let rc = t.col(&|h| h.starts_with("ROOM"));
        let oc = t.col(&|h| h == "OVERRIDE" || h == "TYPE");
        let ic = t.col(&|h| h == "ITEM" || h == "TAG");
        let qc = t.col(&|h| h == "QTY" || h.starts_with("QUANT"));
        let mc = t.col(&|h| h.starts_with("REMARK") || h.starts_with("NOTE"));
        for row in t.rows.iter().take(MAX_UNITS) {
            let (number, kind, item, qty, remark) = (cell(row, rc), upper(&cell(row, oc)), upper(&cell(row, ic)), cell(row, qc), cell(row, mc));
            if number.is_empty() {
                continue;
            }
            let list = overrides.entry(number.clone()).or_default();
            let flag = match kind.as_str() {
                "ADA" => Some("ada"),
                "END UNIT" | "END" => Some("end_unit"),
                k if k.starts_with("COLUMN") => Some("column"),
                _ => None,
            };
            if let Some(f) = flag
                && !list.iter().any(|o| o.key == f)
            {
                list.push(FoundOverride { key: f.into(), value: "true".into(), kind: kind.clone(), remark: remark.clone(), sheet: sheet.into() });
            }
            let (Some((c, k)), Ok(q)) = (legend.get(&item), qty.trim().parse::<u32>()) else {
                if !item.is_empty() {
                    notes.push(format!("{sheet}: override of room {number}: item {item} or quantity {qty} not understood"));
                }
                continue;
            };
            list.push(FoundOverride {
                key: format!("{c}.{k}"),
                value: q.to_string(),
                kind: kind.clone(),
                remark: remark.clone(),
                sheet: sheet.into(),
            });
        }
    }
    (types, overrides)
}

/// The unit tags on a floor plan (building feet): each with its room's box, number and name.
fn read_plan_units(s: &Shapes, level: &str, sheet: &str) -> Vec<(FoundUnit, [f64; 2])> {
    let rooms: Vec<([f64; 2], [f64; 2])> =
        s.boxes.iter().filter(|(l, lo, hi)| l == "ARCH" && hi[0] - lo[0] > 4.0 && hi[1] - lo[1] > 4.0).map(|(_, lo, hi)| (*lo, *hi)).collect();
    let mut out = Vec::new();
    for t in s.texts.iter().filter(|t| !t.grid) {
        if out.len() >= MAX_UNITS {
            break;
        }
        let Some((tag, mirrored)) = unit_tag(&t.s) else { continue };
        let Some((lo, hi)) = rooms
            .iter()
            .filter(|(lo, hi)| contains(*lo, *hi, t.at, 0.0))
            .min_by(|a, b| ((a.1[0] - a.0[0]) * (a.1[1] - a.0[1])).total_cmp(&((b.1[0] - b.0[0]) * (b.1[1] - b.0[1]))))
            .copied()
        else {
            continue;
        };
        let in_room = |x: &&Txt| contains(lo, hi, x.at, 0.0);
        let number = s.texts.iter().filter(in_room).find(|x| (2..=4).contains(&x.s.len()) && x.s.chars().all(|c| c.is_ascii_digit()));
        let Some(number) = number else { continue };
        let name = s
            .texts
            .iter()
            .filter(in_room)
            .filter(|x| x.at[1] > number.at[1] && x.at[1] < number.at[1] + 2.5 * number.h.max(0.1))
            .find(|x| x.s.chars().any(|c| c.is_ascii_alphabetic()))
            .map(|x| x.s.clone())
            .unwrap_or_default();
        let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        out.push((
            FoundUnit {
                type_tag: tag,
                level: level.into(),
                number: number.s.clone(),
                name,
                hand: if mirrored { "mirrored" } else { "normal" }.into(),
                at_ft: lo,
                sheet: sheet.into(),
                overrides: Vec::new(),
            },
            centre,
        ));
    }
    out
}

/// Read the typical units of a set.
pub fn read_units(dxf: &[(String, Vec<u8>)], map: &SheetMap) -> UnitResult {
    let mut res = UnitResult::default();
    let mut overrides: BTreeMap<String, Vec<FoundOverride>> = BTreeMap::new();
    let mut placed: Vec<(FoundUnit, [f64; 2])> = Vec::new();
    for (name, bytes) in dxf {
        let stem = name.trim_end_matches(".dxf");
        let Some(sh) = map.sheets.iter().find(|s| s.number == stem) else { continue };
        if sh.discipline != "A" {
            continue;
        }
        let title = upper(&sh.title);
        let unit_sheet = title.contains("UNIT") && title.contains("PLAN") && !title.contains("FLOOR PLAN");
        let floor = title.contains("FLOOR PLAN");
        if !unit_sheet && !floor {
            continue;
        }
        let Ok(d) = cadaddy_io::read_dxf(bytes) else {
            res.notes.push(format!("{}: unreadable", sh.number));
            continue;
        };
        if unit_sheet {
            let s = shapes(&d, &|p| p, 1.0);
            let (types, ov) = read_unit_sheet(&s, &sh.number, &mut res.notes);
            for t in types {
                if res.types.len() < MAX_TYPES && !res.types.iter().any(|x| x.tag == t.tag) {
                    res.types.push(t);
                }
            }
            for (k, v) in ov {
                overrides.entry(k).or_default().extend(v);
            }
            continue;
        }
        let (Some(level), Some(al)) = (&sh.level, &sh.alignment) else { continue };
        let s = shapes(&d, &|p| sheet_reader::building_from_model(al, p), 1.0 / 12.0);
        for u in read_plan_units(&s, level, &sh.number) {
            if placed.len() >= MAX_UNITS {
                break;
            }
            // A room drawn on more than one plan of its level counts once.
            if !placed.iter().any(|(p, _)| p.level == u.0.level && p.number == u.0.number) {
                placed.push(u);
            }
        }
    }
    for (mut u, centre) in placed {
        // The corner from the room's middle and its type's size (the box drawn is the room's
        // inside face).
        match res.types.iter().find(|t| t.tag == u.type_tag) {
            Some(t) => u.at_ft = [centre[0] - t.width_ft / 2.0, centre[1] - t.depth_ft / 2.0],
            None => res.notes.push(format!("room {} on level {}: unit {} has no enlarged plan", u.number, u.level, u.type_tag)),
        }
        if let Some(o) = overrides.remove(&u.number) {
            u.overrides = o;
        }
        res.units.push(u);
    }
    for number in overrides.keys() {
        res.notes.push(format!("unit overrides name room {number}, which has no unit tag on the floor plans"));
    }
    res.units.sort_by(|a, b| (&a.level, &a.number).cmp(&(&b.level, &b.number)));
    res
}

/// The hotel set's architectural floor plans and unit plans sheet for a seed, as DXF (only those:
/// quick to draw), with its truth.
pub fn fixture_unit_sheets(seed: u64, size: crate::hotel::HotelSize) -> (Truth, Vec<(String, Vec<u8>)>) {
    let (t, st) = crate::hotel_planned(seed, size);
    let kinds = crate::layout::hotel_sheet_list();
    let mut files = Vec::new();
    for sh in t.sheets.iter().filter(|s| s.discipline == "A" && (s.title.contains("FLOOR PLAN") || s.number == crate::units::UNIT_SHEET)) {
        let Some((_, _, _, kind)) = kinds.iter().find(|(n, _, _, _)| *n == sh.number) else { continue };
        let (d, _, _) = crate::draw::draw(&t, &st, sh, kind);
        files.push((sh.dxf.clone(), cadaddy_io::write_dxf(&d).into_bytes()));
    }
    (t, files)
}

/// Draw the hotel's unit sheets for a seed, recognise them and read the units.
pub fn read_fixture_units(seed: u64, size: crate::hotel::HotelSize) -> (Truth, UnitResult) {
    let (t, dxf) = fixture_unit_sheets(seed, size);
    let map = sheet_reader::recognise_fixture_set(&dxf);
    let r = read_units(&dxf, &map);
    (t, r)
}

// ---- Scoring -----------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct UnitScore {
    pub units_total: usize,
    /// Instances read with the right type, room number, level and hand.
    pub units_exact: usize,
    /// Instances read where the truth has none.
    pub extras: usize,
    /// Types with at least one instance.
    pub types_total: usize,
    /// Types read with the right name, size, counts and sides.
    pub types_exact: usize,
    pub overrides_total: usize,
    /// Instances whose overrides read are exactly the truth's.
    pub overrides_exact: usize,
    pub misses: Vec<String>,
}

/// A read instance's overrides as the truth keeps them (key → value).
pub fn override_map(u: &FoundUnit) -> BTreeMap<String, String> {
    u.overrides.iter().map(|o| (o.key.clone(), o.value.clone())).collect()
}

/// Score what was read against the truth.
pub fn score_units(t: &Truth, r: &UnitResult) -> UnitScore {
    let mut sc = UnitScore::default();
    let type_name = |tag: &str| r.types.iter().find(|x| x.tag == tag).map(|x| x.name.as_str());
    for ti in &t.unit_instances {
        sc.units_total += 1;
        let truth_name = t.unit_types.iter().find(|u| u.id == ti.unit_type).map_or("", |u| u.name.as_str());
        let Some(f) = r.units.iter().find(|u| u.level == ti.level && u.number == ti.number) else {
            sc.misses.push(format!("room {} level {}: not read", ti.number, ti.level));
            continue;
        };
        if type_name(&f.type_tag) == Some(truth_name) && f.hand == ti.hand {
            sc.units_exact += 1;
        } else {
            sc.misses.push(format!("room {}: read {:?} {} vs {truth_name} {}", ti.number, type_name(&f.type_tag), f.hand, ti.hand));
        }
        if !ti.overrides.is_empty() {
            sc.overrides_total += 1;
            if override_map(f) == ti.overrides {
                sc.overrides_exact += 1;
            } else {
                sc.misses.push(format!("room {} overrides: read {:?} vs {:?}", ti.number, override_map(f), ti.overrides));
            }
        } else if !f.overrides.is_empty() {
            sc.misses.push(format!("room {}: overrides read where there are none", ti.number));
        }
    }
    sc.extras = r.units.iter().filter(|u| !t.unit_instances.iter().any(|ti| ti.level == u.level && ti.number == u.number)).count();
    for ut in t.unit_types.iter().filter(|ut| t.unit_instances.iter().any(|i| i.unit_type == ut.id)) {
        sc.types_total += 1;
        let Some(f) = r.types.iter().find(|x| x.name == ut.name) else {
            sc.misses.push(format!("type {} not read", ut.name));
            continue;
        };
        let counts = |c: &str| f.counts.get(c).cloned().unwrap_or_default();
        let ok = (f.width_ft - ut.width_ft).abs() < 0.01
            && (f.depth_ft - ut.depth_ft).abs() < 0.01
            && counts("devices") == ut.devices
            && counts("terminals") == ut.terminals
            && counts("fixtures") == ut.fixtures
            && counts("casework") == ut.casework
            && f.wet_wall_side == ut.wet_wall_side
            && f.hvac_sleeve_side == ut.hvac_sleeve_side;
        if ok {
            sc.types_exact += 1;
        } else {
            sc.misses.push(format!("type {}: read {f:?}", ut.name));
        }
    }
    sc
}

// ---- Building spec -----------------------------------------------------------------------------

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// A unit type as the building spec keeps it.
fn type_json(t: &FoundUnitType) -> Value {
    let mut v = json!({
        "id": t.tag, "tag": t.tag, "name": t.name, "width_ft": round3(t.width_ft), "depth_ft": round3(t.depth_ft),
        "wet_wall_side": t.wet_wall_side, "hvac_sleeve_side": t.hvac_sleeve_side,
        "items": t.items.iter().map(|i| json!({"category": i.category, "kind": i.kind, "tag": i.tag, "u": round3(i.uv[0]), "v": round3(i.uv[1])})).collect::<Vec<_>>(),
        "source": {"sheet": t.sheet}, "status": "read",
    });
    for c in ["devices", "terminals", "fixtures", "casework", "equipment"] {
        v[c] = json!(t.counts.get(c).cloned().unwrap_or_default());
    }
    v
}

/// Set `field` (`name`, `width_ft`, or `devices.receptacle`) of a type object to `value`.
pub fn set_field(ty: &mut Value, field: &str, value: Value) -> bool {
    let Some(obj) = ty.as_object_mut() else { return false };
    match field.split_once('.') {
        Some((c, k)) if !c.is_empty() && !k.is_empty() => {
            let m = obj.entry(c.to_string()).or_insert_with(|| json!({}));
            if !m.is_object() {
                *m = json!({});
            }
            if let Some(m) = m.as_object_mut() {
                m.insert(k.into(), value);
            }
            true
        }
        Some(_) => false,
        None => {
            obj.insert(field.into(), value);
            true
        }
    }
}

fn same_unit(a: &Value, level: &str, number: &str) -> bool {
    a.get("level").and_then(Value::as_str) == Some(level) && a.get("number").and_then(Value::as_str) == Some(number)
}

/// The building spec's `unit_types` and `unit_instances` for what was read, merged with the
/// stored spec `stored`: overrides a person set (any author but the reader) and type edits are
/// kept, a person's override winning over a read one of the same key; instances and types the
/// reader didn't write are kept as they are.
pub fn spec_patch(r: &UnitResult, stored: &Value) -> Value {
    let old_types: Vec<&Value> = stored.get("unit_types").and_then(Value::as_array).into_iter().flatten().take(MAX_TYPES * 10).collect();
    let old_units: Vec<&Value> = stored.get("unit_instances").and_then(Value::as_array).into_iter().flatten().take(MAX_STORED).collect();
    let mut types: Vec<Value> = Vec::new();
    for t in &r.types {
        let mut v = type_json(t);
        // Edits made by hand to the type are made again.
        if let Some(edits) =
            old_types.iter().find(|o| o.get("id").and_then(Value::as_str) == Some(&t.tag)).and_then(|o| o.get("edits")).and_then(Value::as_array)
        {
            for e in edits.iter().take(1_000) {
                if let (Some(f), Some(val)) = (e.get("field").and_then(Value::as_str), e.get("value")) {
                    set_field(&mut v, f, val.clone());
                }
            }
            v["edits"] = json!(edits);
        }
        types.push(v);
    }
    let read_by_reader = |v: &Value| v.get("status").and_then(Value::as_str) == Some("read");
    let kept: Vec<Value> =
        old_types.iter().filter(|o| !read_by_reader(o) && !types.iter().any(|t| t.get("id") == o.get("id"))).map(|o| (*o).clone()).collect();
    types.extend(kept);
    let mut units: Vec<Value> = Vec::new();
    for u in &r.units {
        let old = old_units.iter().find(|o| same_unit(o, &u.level, &u.number));
        let manual: Vec<Value> = old
            .and_then(|o| o.get("overrides"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|o| o.get("author").and_then(Value::as_str).is_some_and(|a| a != READER))
            .take(1_000)
            .cloned()
            .collect();
        let mut overrides: Vec<Value> = u
            .overrides
            .iter()
            .filter(|o| !manual.iter().any(|m| m.get("key").and_then(Value::as_str) == Some(&o.key)))
            .map(|o| {
                json!({"key": o.key, "value": o.value, "kind": o.kind, "note": o.remark, "source": {"sheet": o.sheet, "table": OVERRIDES_TITLE}, "author": READER})
            })
            .collect();
        overrides.extend(manual);
        units.push(json!({
            "id": format!("u-{}-{}", u.level, u.number), "type": u.type_tag, "room": u.number, "number": u.number, "name": u.name,
            "level": u.level, "hand": u.hand, "at_ft": [round3(u.at_ft[0]), round3(u.at_ft[1])],
            "source": {"sheet": u.sheet}, "status": "read", "overrides": overrides,
        }));
    }
    units.extend(
        old_units
            .iter()
            .filter(|o| !read_by_reader(o))
            .filter(|o| {
                let s = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
                !r.units.iter().any(|u| u.level == s("level") && u.number == s("number"))
            })
            .map(|o| (*o).clone()),
    );
    json!({"unit_types": types, "unit_instances": units})
}
