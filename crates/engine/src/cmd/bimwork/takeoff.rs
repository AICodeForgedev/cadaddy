//! `bim.takeoff`, `bim.takeoff.show`, `bim.estimate`: quantities from the model and their cost.
//!
//! The takeoff regroups the `bim.bom` lines (by level, trade, system or type), gives each its
//! count, length, area or volume, and keeps each line's elements: on a per-level line, the
//! elements of that level (a BOM line that lists a category's elements across levels is narrowed
//! to the line's level, and a line without elements gets its level's built elements of that
//! category; a line that names its elements is those only, never its category's on the level).
//! Another trade's devices (the BOM's `by_others`) are on lines of their own, `by others`, out of
//! our totals and never priced, so every built device is on a line (`devices`). Every line's level is one of the model's levels, as the model names it (a line
//! naming `2` is on `Level 2`; a line naming none is on its elements' level), else the explicit
//! `(unassigned level)` group, counted. Each takeoff is compared with the last one kept in the
//! drawing.
//!
//! The estimate maps each takeoff line to a row of a cost table (item, unit, material $ per unit,
//! labour hours per unit, optional waste %): an exact item name first, else the row whose words
//! the line's item, size and category carry most of, in a compatible unit. Lines no row prices are
//! listed, never priced at zero silently. A named `revision` is kept; `compare_to` another
//! revision gives the change order between them.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::Handle;
use serde_json::{Value, json};

use super::{diff, elements, frame, handles_of, r3, set_state, state};
use crate::cmd::{CommandSpec, bad, bool_or, str_param};
use crate::{Result, Session};

const CMD: &str = "bim.takeoff";
const CMD_SHOW: &str = "bim.takeoff.show";
const CMD_EST: &str = "bim.estimate";
/// Rows read from a cost table, takeoff lines priced, revisions kept, elements a line keeps.
const MAX_ROWS: usize = 20_000;
const MAX_LINES: usize = 50_000;
const MAX_REVISIONS: usize = 50;
const MAX_LINE_ELEMENTS: usize = 5_000;
/// The level of a takeoff line whose work is on no one level of the model (Q6271): a job-wide
/// quantity, or a level the model has no such name for. Counted apart (`unassigned_level`).
pub(crate) const UNASSIGNED_LEVEL: &str = "(unassigned level)";
/// The labour rate when neither the call nor the table gives one, $ per hour.
const DEFAULT_LABOR_RATE: f64 = 75.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Model Takeoff", run)
            .alias(&["takeoffmodel"])
            .params("{force?: bool (replace an existing file; otherwise it is refused), group_by?: level|trade|system|type|circuit (default level; circuit: each level's lines per branch circuit) | [property key] (by the elements' properties, e.g. [\"level\", \"circuit\"]), filter?: {trade?, level?, category?, circuit?, text?} | text | {kind?, where?, key: value…} (by the elements' properties: e.g. {kind: \"device\", type: \"duplex\"}), compare_to?: {lines} (else the last takeoff), keep?: bool (keep this takeoff as the last; default true), file?: .csv|.xlsx} → {lines: [{key, level (one of the model's levels, else \"(unassigned level)\"), circuit?, trade, system, category, item, size, unit, qty, count?, length_ft?, area_sf?, volume_cy?, weight_lb?, elements (exactly the elements counted on it), element_count, elements_unresolved? (named elements the model has none of), status (\"by others\": another trade's devices, never priced), scope?, owner?, made: drawn|derived|inferred}], totals: {count, length_ft, area_sf, volume_cy, by_made: {drawn, derived, inferred} (each the same totals), by_others: {lines, count}} (our lines: another trade's are apart), devices?: {built, on_lines, by_others, not_on_a_line, not_on_a_line_first} (every built device on a line, unfiltered), levels, unassigned_level?: {level, lines, elements} (the lines on no one level of the model), changes?: {added, removed, changed}, revision, csv, path?}")
            .noundo(),
        CommandSpec::new(CMD_SHOW, "Show Takeoff Line", run_show)
            .params("{line: key or index (of the last takeoff) | elements: [handle]} → {key?, selected, handles, min, max, viewpoint} — selects the elements (marked in 3D) and points the 3D camera at them")
            .noundo(),
        CommandSpec::new(CMD_EST, "Model Estimate", run_estimate)
            .params("{force?: bool (replace an existing file; otherwise it is refused), price_book?: .xlsx path (sheets \"Price List\": key, description, catalog, unit, unit price $, source type, vendor, URL, price date; \"Item Map\": CADaddy item, size, action price|exclude, price key 1, price key 2, note): material priced by each line's catalog number in the Price List, else its Item Map row (keys added), else listed unpriced, stale_days?: n (90: an older price is flagged STALE), tax_pct?: percent (0), today?: YYYY-MM-DD, by_level?, filter?, file?: .xlsx (live INDEX/MATCH formulas into the book's Price List) | .csv → {lines: [{item, size, unit, qty, qty_total, type_mark?, manufacturer?, catalog?, dimming?, product_source?, matched_by: catalog|item_map, price_keys, unit_price, extended, price_date, age_days, source_type, vendor, price, flag?: STALE}], unpriced: [{…, reason}], excluded: [{…, reason}], counts, by_category, totals: {by_group: [{trade, category, lines, extended}], subtotal, tax: {rate_pct, amount}, total}, warnings, csv, path?} | cost_table: .csv|.xlsx path | csv: text | rows: [{item, unit, material, labor_hrs, waste?}], labor_rate?: $/h (75, or the table's labor_rate column), waste?: percent | {category: percent} (default the BOM's), by_level?: bool, filter?: as bim.takeoff, revision?: name (keep it), compare_to?: revision name | {lines}, file?: .csv|.xlsx} → {lines: [{item, size, unit, qty, waste_pct, qty_total, cost_item, material_unit, labor_hrs_unit, material, labor_hrs, labor, total, elements}], unmatched: [{item, size, unit, qty, reason}], unused_rows, material_total, labor_hours, labor_total, total, labor_rate, change_order?: {lines: [{item, size, unit, before_qty, after_qty, delta_qty, delta_cost}], total}, csv, path?}")
            .noundo(),
    ]
}

/// A BOM line's product fields (Q6164), kept on its takeoff line.
pub(super) const PRODUCT_FIELDS: [&str; 5] = ["type_mark", "manufacturer", "catalog", "dimming", "product_source"];

/// The quantity columns a unit fills.
fn measure(unit: &str) -> Option<&'static str> {
    let u = unit.trim().to_ascii_lowercase().replace('.', "");
    Some(match u.as_str() {
        "each" | "ea" | "count" | "pcs" | "pc" | "no" | "set" | "sets" | "unit" | "units" => "count",
        "lf" | "ft" | "feet" | "foot" | "linear ft" | "lin ft" | "lnft" => "length_ft",
        "sf" | "sq ft" | "sqft" | "square feet" | "ft2" | "sft" => "area_sf",
        "cy" | "cu yd" | "cuyd" | "yd3" | "cubic yards" => "volume_cy",
        "cf" | "cu ft" | "cuft" | "ft3" => "volume_cf",
        "lb" | "lbs" | "pound" | "pounds" => "weight_lb",
        _ => return None,
    })
}

/// A takeoff line's filter.
fn passes(f: &Value, l: &Value) -> bool {
    let has = |k: &str| l[k].as_str().unwrap_or_default().to_ascii_lowercase();
    match f {
        Value::Null => true,
        Value::String(t) => {
            let t = t.to_ascii_lowercase();
            ["trade", "category", "item", "size", "level", "system"].iter().any(|k| has(k).contains(&t))
        }
        Value::Object(o) => o.iter().all(|(k, v)| {
            let want = v.as_str().unwrap_or_default().to_ascii_lowercase();
            if want.is_empty() {
                return true;
            }
            match k.as_str() {
                "text" => ["trade", "category", "item", "size", "system"].iter().any(|k| has(k).contains(&want)),
                "trade" | "level" | "category" | "system" | "item" | "unit" | "circuit" => has(k) == want || has(k).contains(&want),
                _ => true,
            }
        }),
        _ => true,
    }
}

/// The BOM category a built part belongs to.
fn part_of(category: &str) -> Option<&'static str> {
    Some(match category {
        "walls" => "wall",
        "doors" => "door",
        "ceilings" => "ceiling",
        "floors" | "floor slabs" | "slabs" => "floor",
        "rooms" => "room",
        "windows" => "window",
        "devices" => "device",
        _ => return None,
    })
}

/// The takeoff lines, by `group_by`: each with its quantities and its elements. Our scope only:
/// another trade's devices are not on them (`lines_of` with `others` adds them, apart).
pub(super) fn lines(s: &mut Session, group_by: &str, filter: &Value) -> Result<Vec<Value>> {
    lines_of(s, group_by, filter, false)
}

/// The status of a takeoff line for another trade's devices (the BOM's `by_others`): counted, so
/// every built device is on a line, but not ours (out of our totals, never priced).
pub(super) const BY_OTHERS: &str = "by others";

/// The BOM's `by_others` devices as BOM lines: one per element, its owner's scope on it.
fn by_others_lines(bom: &Value) -> Vec<Value> {
    bom["by_others"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .take(MAX_LINES)
        .filter_map(|o| {
            let element = o["element"].as_str().filter(|e| !e.is_empty())?;
            let kind = o["kind"].as_str().filter(|k| !k.is_empty())?;
            Some(json!({"level": o["level"], "trade": o["trade"], "category": "devices", "item": kind, "size": "", "unit": "each", "net": 1.0,
                "status": BY_OTHERS, "scope": o["scope"], "owner": o["owner"], "elements": [element], "made": "drawn"}))
        })
        .collect()
}

/// The takeoff lines; with `others`, also another trade's devices on their own lines (status
/// `by others`, their owner's `scope`), each the built devices it names only.
fn lines_of(s: &mut Session, group_by: &str, filter: &Value, others: bool) -> Result<Vec<Value>> {
    // Per level and branch circuit: lines on no circuit are the level's as they are.
    let bom = s.execute("bim.bom", &json!({"group_by": "circuit", "all_elements": true}))?;
    let d = s.doc()?;
    let (sp, _) = crate::cmd::bspec::current(s, CMD)?;
    // Each built element's level (as the model names it), category and system.
    let els = elements(d);
    let on_elements: BTreeSet<String> = els.iter().map(|e| e.level.trim().to_string()).filter(|l| !l.is_empty()).collect();
    let mut names: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut model_level =
        |name: &str| -> Option<String> { names.entry(name.to_string()).or_insert_with(|| level_name(&sp, &on_elements, name)).clone() };
    let info: BTreeMap<String, (String, String, String)> =
        els.iter().map(|e| (e.handle.hex(), (model_level(&e.level).unwrap_or_default(), e.category.clone(), e.system.clone()))).collect();
    let mut built: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for e in &els {
        let level = info.get(&e.handle.hex()).map(|i| i.0.clone()).unwrap_or_default();
        let v = built.entry((e.category.clone(), level)).or_default();
        if v.len() < MAX_LINE_ELEMENTS {
            v.push(e.handle.hex());
        }
    }
    // The BOM names a spec run as `run:<index>`: the meshes built from it (their `mepid` starts
    // with the run's id).
    let mut by_run: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // The BOM counts rooms from the ceilings built with a room's tag: a level's rooms are those.
    let mut room_ceilings: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // The device elements by the spec device they were built from: a device line counts those.
    // The built elements by the name their build gave them (`EXIT_SIGN-1`): a line naming one
    // counts it.
    let mut by_device: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Built elements only, before the cap (Q6373): a print set's linework comes first, and a
    // large job has well over the cap of entities, its built devices after them.
    for e in d.model.iter().filter(|e| crate::cmd::bimlook::made_tags(e).is_some()).take(super::MAX_ELEMENTS) {
        let tags = crate::cmd::fieldview::wallgen_tags(e);
        if tags.first() == Some(&"device")
            && let Some(id) = crate::cmd::fieldview::tag_value(&tags, "devid")
        {
            by_device.entry(id.to_string()).or_default().push(e.handle.hex());
        } else if let Some(name) = crate::cmd::bimlook::made_name(e).filter(|n| !n.is_empty()) {
            by_name.entry(name.to_string()).or_default().push(e.handle.hex());
        }
        if tags.first() == Some(&"mep")
            && let Some(id) = crate::cmd::fieldview::tag_value(&tags, "mepid")
        {
            let base = id.rsplit_once('/').map_or(id, |(b, _)| b);
            by_run.entry(base.to_string()).or_default().push(e.handle.hex());
        }
        if let (Some(&"ceiling"), Some(level)) = (tags.first(), tags.get(1))
            && e.common.xdata.iter().any(|t| t.code == 1000 && t.value.starts_with("room:"))
        {
            let v = room_ceilings.entry(model_level(level).unwrap_or_else(|| (*level).to_string())).or_default();
            if v.len() < MAX_LINE_ELEMENTS {
                v.push(e.handle.hex());
            }
        }
    }
    // The BOM's walls measured from the meshes give way to this takeoff's own (per level, every
    // wall's real length).
    let bom_lines: Vec<Value> = bom["lines"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|l| !(l["category"] == "walls" && l["rule"] == crate::cmd::bom::MESH_WALLS_RULE))
        .cloned()
        .collect();
    let bom_lines = bom_lines.as_slice();
    let wall_lines = built_wall_lines(d, &els, bom_lines, super::units_per_foot(s));
    let other_lines = if others { by_others_lines(&bom) } else { Vec::new() };
    let resolve = |h: &str| -> Vec<String> {
        if let Some(el) = by_device.get(h) {
            return el.clone();
        }
        match h.strip_prefix("run:").and_then(|i| i.parse::<usize>().ok()) {
            Some(i) => sp.mep.runs.get(i).and_then(|r| by_run.get(&crate::cmd::mep::run_base(r))).cloned().unwrap_or_default(),
            None => match Handle::parse_hex(h).filter(|x| d.entity(*x).is_some()) {
                Some(x) => vec![x.hex()],
                None => by_name.get(h).cloned().unwrap_or_default(),
            },
        }
    };
    let on_level = |h: &str, lv: &str| info.get(h).is_some_and(|(x, _, _)| x.eq_ignore_ascii_case(lv));
    let mut grouped: BTreeMap<String, Value> = BTreeMap::new();
    for l in bom_lines.iter().chain(wall_lines.iter()).chain(other_lines.iter()).take(MAX_LINES) {
        let by_others = l["status"] == BY_OTHERS;
        // The line's level as the model names it (Q6271): `2` or `L2` is `Level 2`; a level no
        // level of the model answers to stays as written while the elements are looked up.
        let named = l["level"].as_str().unwrap_or_default().trim().to_string();
        let resolved = model_level(&named);
        let mut level = resolved.clone().unwrap_or_else(|| named.clone());
        let category = l["category"].as_str().unwrap_or_default().to_string();
        // The elements the BOM counted on the line: a line that names its own elements is those
        // (Q6373), never every element of its category on its level when they are not found
        // (those not found are counted on the line).
        let named_els: Vec<&str> = l["elements"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let mut unresolved = 0usize;
        let mut elems: Vec<String> = Vec::new();
        for h in &named_els {
            let r = resolve(h);
            if r.is_empty() {
                unresolved += 1;
            }
            elems.extend(r);
        }
        let mut seen = BTreeSet::new();
        elems.retain(|h| seen.insert(h.clone()));
        let names_its_own = !named_els.is_empty();
        // Another trade's line is its built devices only: one the model has none of is not a
        // device of this model.
        if by_others {
            elems.retain(|h| info.get(h).is_some_and(|(_, c, _)| c == "device"));
            if elems.is_empty() {
                continue;
            }
        }
        // A line that names no level: its elements' level, when they are all on one.
        if level.is_empty()
            && let Some(lv) = one_level(&info, &elems)
        {
            level = lv;
        }
        // On a level's line, that level's elements.
        if !level.is_empty() {
            elems.retain(|h| info.get(h).is_none_or(|(lv, _, _)| lv.is_empty() || lv.eq_ignore_ascii_case(&level)));
        }
        if elems.is_empty()
            && !names_its_own
            && let Some(part) = part_of(&category)
        {
            elems = built.get(&(part.to_string(), level.clone())).cloned().unwrap_or_default();
        }
        if elems.is_empty() && !names_its_own && category == "rooms" {
            elems = room_ceilings.get(&level).cloned().unwrap_or_default();
        }
        // Board and framing are on the walls: the level's walls (every level's, on a line the BOM
        // gives for the whole job).
        if elems.is_empty() && !names_its_own && matches!(category.as_str(), "drywall" | "framing") {
            elems =
                built.iter().filter(|((p, lv), _)| p == "wall" && (level.is_empty() || *lv == level)).flat_map(|(_, h)| h.iter().cloned()).collect();
            elems.truncate(MAX_LINE_ELEMENTS);
        }
        // MEP lines (duct, pipe, conduit by system and size): the level's runs of that kind, of
        // the system the item names when it names one.
        if elems.is_empty() && !names_its_own && matches!(category.as_str(), "duct" | "pipe" | "conduit" | "tray" | "cable") {
            let item = l["item"].as_str().unwrap_or_default().to_ascii_uppercase();
            let of_kind: Vec<&super::El> =
                els.iter().filter(|e| e.category == category && (level.is_empty() || on_level(&e.handle.hex(), &level))).collect();
            let named: Vec<&&super::El> = of_kind.iter().filter(|e| !e.system.is_empty() && item.contains(&e.system.to_ascii_uppercase())).collect();
            elems = if named.is_empty() { of_kind.iter().map(|e| e.handle.hex()).collect() } else { named.iter().map(|e| e.handle.hex()).collect() };
            elems.truncate(MAX_LINE_ELEMENTS);
        }
        // Every line on one of the model's levels: still none, its elements' one level, else
        // the unassigned group (counted), never a raw name or a blank.
        if resolved.is_none() {
            level = one_level(&info, &elems).unwrap_or_else(|| UNASSIGNED_LEVEL.to_string());
        }
        let system = elems
            .iter()
            .filter_map(|h| info.get(h))
            .map(|(_, _, sy)| sy.clone())
            .find(|sy| !sy.is_empty() && !sy.starts_with("3D-"))
            .unwrap_or_else(|| category.clone());
        let unit = l["unit"].as_str().unwrap_or_default().to_string();
        let (trade, item, size) = (
            l["trade"].as_str().unwrap_or_default().to_string(),
            l["item"].as_str().unwrap_or_default().to_string(),
            l["size"].as_str().unwrap_or_default().to_string(),
        );
        let circuit = if group_by == "circuit" { l["circuit"].as_str().unwrap_or_default() } else { "" };
        // What the line's items are by the documents (Q6216): drawn, derived or inferred; lines
        // of two kinds stay two.
        let made = l["made"].as_str().filter(|m| !m.is_empty()).unwrap_or("drawn").to_string();
        let mut line =
            json!({"level": level, "trade": trade, "system": system, "category": category, "item": item, "size": size, "unit": unit, "made": made});
        if !circuit.is_empty() {
            line["circuit"] = json!(circuit);
        }
        if by_others {
            line["scope"] = l["scope"].clone();
            line["owner"] = l["owner"].clone();
        }
        // What the line's items are as a product (Q6164): lines of two products stay two.
        let mut product = String::new();
        for k in PRODUCT_FIELDS {
            if let Some(v) = l[k].as_str().filter(|v| !v.is_empty()) {
                line[k] = json!(v);
                if k != "product_source" {
                    product.push('|');
                    product.push_str(v);
                }
            }
        }
        if !passes(filter, &line) {
            continue;
        }
        let mut filled = if made == "drawn" { String::new() } else { format!("|{made}") };
        if by_others {
            filled.push('|');
            filled.push_str(l["scope"].as_str().unwrap_or(BY_OTHERS));
        }
        let key = match group_by {
            "trade" => format!("{trade}|{category}|{item}|{size}|{unit}{product}{filled}"),
            "system" => format!("{system}|{item}|{size}|{unit}{product}{filled}"),
            "type" => format!("{item}|{size}|{unit}{product}{filled}"),
            "circuit" => format!("{level}|{circuit}|{trade}|{category}|{item}|{size}|{unit}{product}{filled}"),
            _ => format!("{level}|{trade}|{category}|{item}|{size}|{unit}{product}{filled}"),
        };
        let qty = l["net"].as_f64().filter(|v| v.is_finite()).unwrap_or(0.0);
        let entry = grouped.entry(key.clone()).or_insert_with(|| {
            let mut x = line.clone();
            x["key"] = json!(key);
            x["qty"] = json!(0.0);
            x["elements"] = json!([]);
            x["status"] = l["status"].clone();
            x["waste_pct"] = l["waste_pct"].clone();
            x["levels"] = json!([]);
            x["elements_unresolved"] = json!(0);
            if !matches!(group_by, "level" | "circuit") {
                x["level"] = json!("");
            }
            if group_by == "type" || group_by == "system" {
                x["trade"] = line["trade"].clone();
            }
            x
        });
        entry["qty"] = json!(entry["qty"].as_f64().unwrap_or(0.0) + qty);
        // A trade piece line's count and measures beside its quantity (Q6527).
        if let Some(m) = l["measures"].as_object() {
            for (from, to) in
                [("pieces", "count"), ("length_ft", "length_ft"), ("area_sf", "area_sf"), ("volume_cy", "volume_cy"), ("weight_lb", "weight_lb")]
            {
                if let Some(v) = m.get(from).and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0) {
                    let had = entry["measures"][to].as_f64().unwrap_or(0.0);
                    entry["measures"][to] = json!(had + v);
                }
            }
        }
        entry["elements_unresolved"] = json!(entry["elements_unresolved"].as_u64().unwrap_or(0) + unresolved as u64);
        if let Some(a) = entry["elements"].as_array_mut() {
            let have: BTreeSet<String> = a.iter().filter_map(Value::as_str).map(str::to_string).collect();
            for h in elems {
                if a.len() >= MAX_LINE_ELEMENTS {
                    break;
                }
                if !have.contains(&h) {
                    a.push(json!(h));
                }
            }
        }
        if !level.is_empty()
            && let Some(a) = entry["levels"].as_array_mut()
            && !a.iter().any(|x| x.as_str() == Some(level.as_str()))
        {
            a.push(json!(level));
        }
    }
    Ok(grouped
        .into_values()
        .map(|mut l| {
            let q = l["qty"].as_f64().unwrap_or(0.0);
            l["qty"] = json!(r3(q));
            if let Some(m) = measure(l["unit"].as_str().unwrap_or_default()) {
                l[m] = json!(r3(q));
            }
            // The pieces' count and other measures (Q6527), beside the quantity's own.
            if let Some(Value::Object(ms)) = l.as_object_mut().and_then(|o| o.remove("measures")) {
                for (k, v) in ms {
                    if l[k.as_str()].is_null() {
                        l[k.as_str()] = json!(r3(v.as_f64().unwrap_or(0.0)));
                    }
                }
            }
            l["element_count"] = json!(l["elements"].as_array().map_or(0, Vec::len));
            // The named elements the model has none of, counted; none: no field.
            if l["elements_unresolved"].as_u64() == Some(0)
                && let Some(o) = l.as_object_mut()
            {
                o.remove("elements_unresolved");
            }
            l
        })
        .collect())
}

/// Built devices a takeoff names as on no line, at most.
const MAX_UNLISTED: usize = 50;

/// Whether every built device is on a takeoff line (Q6373): the model's devices, those on a line
/// (ours or another trade's), those on another trade's, and those on none (the first of them).
/// Only for an unfiltered takeoff; none for a model with no devices.
fn device_check(d: &cadaddy_doc::Drawing, lines: &[Value], filter: &Value) -> Option<Value> {
    if !filter.is_null() {
        return None;
    }
    let built: Vec<String> = elements(d).into_iter().filter(|e| e.category == "device").map(|e| e.handle.hex()).collect();
    if built.is_empty() {
        return None;
    }
    let on = |by_others: bool| -> BTreeSet<&str> {
        lines
            .iter()
            .filter(|l| (l["status"] == BY_OTHERS) == by_others)
            .flat_map(|l| l["elements"].as_array().map(Vec::as_slice).unwrap_or_default())
            .filter_map(Value::as_str)
            .collect()
    };
    let (ours, theirs) = (on(false), on(true));
    let unlisted: Vec<&String> = built.iter().filter(|h| !ours.contains(h.as_str()) && !theirs.contains(h.as_str())).collect();
    let on_lines = built.len() - unlisted.len();
    let by_others = built.iter().filter(|h| theirs.contains(h.as_str())).count();
    Some(json!({"built": built.len(), "on_lines": on_lines, "by_others": by_others, "not_on_a_line": unlisted.len(),
        "not_on_a_line_first": unlisted.into_iter().take(MAX_UNLISTED).collect::<Vec<_>>()}))
}

/// A built wall's length along the floor and its height, feet: the farthest two of its points in
/// plan (its box's diagonal for a wall of many points), and its height span.
fn wall_size(e: &cadaddy_doc::Entity, lo: [f64; 3], hi: [f64; 3], upf: f64) -> (f64, f64) {
    let height = (hi[2] - lo[2]) / upf;
    let mut len = (hi[0] - lo[0]).hypot(hi[1] - lo[1]);
    if let cadaddy_doc::EntityKind::Mesh(m) = &e.kind
        && (2..=64).contains(&m.vertices.len())
    {
        let pts: Vec<(f64, f64)> = m.vertices.iter().filter(|v| v.x.is_finite() && v.y.is_finite()).map(|v| (v.x, v.y)).collect();
        len = pts.iter().enumerate().flat_map(|(i, a)| pts.iter().skip(i + 1).map(move |b| (a.0 - b.0).hypot(a.1 - b.1))).fold(0.0, f64::max);
    }
    let ok = |v: f64| if v.is_finite() && v > 0.0 { v } else { 0.0 };
    (ok(len / upf), ok(height))
}

/// The model's name of a level a line or an element names (Q6271): the spec's level it names in
/// any form (`2`, `L2`, `SECOND FLOOR` find `Level 2`), else a level built elements stand on as
/// written; none for a blank or a level the model has no such name for.
fn level_name(sp: &cadaddy_walk3d::BuildingSpec, on_elements: &BTreeSet<String>, name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if let Some(l) = sp.level(name) {
        return Some(l.name.clone());
    }
    on_elements.iter().find(|l| l.eq_ignore_ascii_case(name)).cloned()
}

/// The one level `elems` are all on (elements of no level aside), if there is one.
fn one_level(info: &BTreeMap<String, (String, String, String)>, elems: &[String]) -> Option<String> {
    let on: BTreeSet<&str> = elems.iter().filter_map(|h| info.get(h)).map(|(lv, _, _)| lv.as_str()).filter(|lv| !lv.is_empty()).collect();
    match on.len() {
        1 => on.into_iter().next().map(str::to_string),
        _ => None,
    }
}

/// Wall lines for the levels whose built walls the BOM has no wall line for (walls built from the
/// plan's wall linework, with no wall marks in the spec): length and face area per level, from
/// the built wall meshes, as BOM lines.
fn built_wall_lines(d: &cadaddy_doc::Drawing, els: &[super::El], bom: &[Value], upf: f64) -> Vec<Value> {
    let has_line: BTreeSet<String> =
        bom.iter().filter(|l| l["category"] == "walls").filter_map(|l| l["level"].as_str()).map(str::to_ascii_lowercase).collect();
    let mut by_level: BTreeMap<String, (f64, f64, Vec<String>)> = BTreeMap::new();
    for el in els.iter().filter(|e| e.category == "wall" && !has_line.contains(&e.level.to_ascii_lowercase())) {
        let Some(e) = d.entity(el.handle) else { continue };
        let (len, height) = wall_size(e, [el.lo.x, el.lo.y, el.lo.z], [el.hi.x, el.hi.y, el.hi.z], upf);
        let v = by_level.entry(el.level.clone()).or_default();
        v.0 += len;
        v.1 += len * height;
        if v.2.len() < MAX_LINE_ELEMENTS {
            v.2.push(el.handle.hex());
        }
    }
    let line = |level: &str, item: &str, unit: &str, net: f64, els: &[String]| {
        json!({"level": level, "trade": "general", "category": "walls", "item": item, "size": "", "unit": unit, "net": r3(net), "status": "model",
            "rule": "from the built wall meshes (no wall marks in the spec)", "elements": els})
    };
    by_level
        .iter()
        .filter(|(_, (len, _, _))| *len > 0.0)
        .flat_map(|(lv, (len, area, hs))| [line(lv, "Wall", "lf", *len, hs), line(lv, "Wall area", "sq ft", *area, hs)])
        .collect()
}

fn qty_map(lines: &[Value]) -> BTreeMap<String, f64> {
    lines.iter().filter_map(|l| Some((l["key"].as_str()?.to_string(), l["qty"].as_f64()?))).collect()
}

fn csv_cell(v: &str) -> String {
    if v.contains([',', '"', '\n']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.to_string() }
}

fn write_table(cmd: &str, path: &str, rows: &[Vec<String>], sheet: &str) -> Result<Value> {
    let csv: String = rows.iter().map(|r| r.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(",") + "\n").collect();
    let bytes = if path.to_ascii_lowercase().ends_with(".xlsx") {
        crate::cmd::table::xlsx::write(rows, sheet).map_err(|e| bad(cmd, e))?
    } else {
        csv.clone().into_bytes()
    };
    #[cfg(not(target_arch = "wasm32"))]
    crate::writefile::write(path, &bytes).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
    #[cfg(target_arch = "wasm32")]
    let _ = bytes;
    Ok(json!(path))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    super::super::exchange::guard_params(s, "bim.takeoff", p, &["file"])?;
    // By the elements' properties (Q6003): any keys, a filter by kind or property.
    if crate::cmd::propuse::by_properties(
        p,
        &["level", "trade", "system", "type", "circuit"],
        &["trade", "level", "category", "circuit", "text", "system", "item", "unit"],
    ) {
        return crate::cmd::propuse::grouped(s, CMD, p);
    }
    let group_by = str_param(p, "group_by").unwrap_or("level").to_ascii_lowercase();
    if !matches!(group_by.as_str(), "level" | "trade" | "system" | "type" | "circuit") {
        return Err(bad(CMD, "`group_by` is level, trade, system, type or circuit"));
    }
    let filter = p.get("filter").cloned().unwrap_or(Value::Null);
    // Our lines, and another trade's devices on lines of their own (Q6373).
    let lines = lines_of(s, &group_by, &filter, true)?;
    let ours: Vec<&Value> = lines.iter().filter(|l| l["status"] != BY_OTHERS).collect();
    let theirs: Vec<&Value> = lines.iter().filter(|l| l["status"] == BY_OTHERS).collect();
    let mut totals = json!({"lines": ours.len()});
    for m in ["count", "length_ft", "area_sf", "volume_cy"] {
        totals[m] = json!(r3(ours.iter().filter_map(|l| l[m].as_f64()).sum::<f64>()));
    }
    totals["by_others"] = json!({"lines": theirs.len(), "count": r3(theirs.iter().filter_map(|l| l["count"].as_f64()).sum::<f64>())});
    // Every total split by what the work is by the documents (Q6216): inferred work is in the
    // job's totals, and shown apart from the drawn.
    let mut by_made = serde_json::Map::new();
    for made in ["drawn", "derived", "inferred"] {
        let of: Vec<&Value> = ours.iter().copied().filter(|l| l["made"].as_str().unwrap_or("drawn") == made).collect();
        let mut t = json!({"lines": of.len()});
        for m in ["count", "length_ft", "area_sf", "volume_cy"] {
            t[m] = json!(r3(of.iter().filter_map(|l| l[m].as_f64()).sum::<f64>()));
        }
        by_made.insert(made.to_string(), t);
    }
    totals["by_made"] = Value::Object(by_made);
    let levels: BTreeSet<String> =
        lines.iter().flat_map(|l| l["levels"].as_array().cloned().unwrap_or_default()).filter_map(|v| v.as_str().map(str::to_string)).collect();
    let now = qty_map(&lines);
    let st = state(s.doc()?);
    let last_key = format!("takeoff.{group_by}");
    let before: Option<BTreeMap<String, f64>> = match p.get("compare_to").and_then(|c| c["lines"].as_array()) {
        Some(a) => Some(qty_map(a)),
        None => st[&last_key]["lines"].as_object().map(|o| o.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?))).collect()),
    };
    let revision = st[&last_key]["revision"].as_u64().unwrap_or(0) + u64::from(before.as_ref().is_none_or(|b| *b != now));
    // By circuit, the circuit is a column after the level.
    let by_circuit = group_by == "circuit";
    let mut head = ["level", "trade", "system", "category", "item", "size", "unit", "qty", "elements"].map(String::from).to_vec();
    if by_circuit {
        head.insert(1, "circuit".into());
    }
    let rows: Vec<Vec<String>> = std::iter::once(head)
        .chain(lines.iter().map(|l| {
            let f = |k: &str| l[k].as_str().unwrap_or_default().to_string();
            // Another trade's line says so in its trade column.
            let trade = if l["status"] == BY_OTHERS { format!("{} ({BY_OTHERS})", f("trade")) } else { f("trade") };
            let mut row = vec![
                f("level"),
                trade,
                f("system"),
                f("category"),
                f("item"),
                f("size"),
                f("unit"),
                r3(l["qty"].as_f64().unwrap_or(0.0)).to_string(),
                l["element_count"].to_string(),
            ];
            if by_circuit {
                row.insert(1, f("circuit"));
            }
            row
        }))
        .collect();
    let devices = device_check(s.doc()?, &lines, &filter);
    let mut out = json!({"lines": lines, "totals": totals, "levels": levels, "group_by": group_by, "revision": revision});
    if let Some(dv) = devices {
        out["devices"] = dv;
    }
    // Each scheduled luminaire type's count in the model beside the schedule's quantity
    // (fix-lum-schedule-type-column-wrong): both shown, a difference a review item.
    if let Ok((sp, _)) = crate::cmd::bspec::current(s, CMD) {
        let read = crate::cmd::guided::load_job(s).ok().flatten().map(|j| crate::cmd::guided::read_luminaires(&j)).unwrap_or_default();
        let q = crate::cmd::lighting::schedule_quantities(s.doc()?, &sp.mep.fixture_schedule, &read);
        if !q.is_empty() {
            out["luminaire_schedule_quantities"] = Value::Array(q);
        }
    }
    // The work on no one level of the model, counted (Q6271).
    let unassigned: Vec<&Value> = lines.iter().filter(|l| l["level"] == UNASSIGNED_LEVEL).collect();
    if !unassigned.is_empty() {
        let elements: u64 = unassigned.iter().filter_map(|l| l["element_count"].as_u64()).sum();
        out["unassigned_level"] = json!({"level": UNASSIGNED_LEVEL, "lines": unassigned.len(), "elements": elements});
    }
    if let Some(b) = &before {
        out["changes"] = diff(b, &now);
    }
    out["csv"] = json!(rows.iter().map(|r| r.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(",") + "\n").collect::<String>());
    if let Some(path) = str_param(p, "file").map(str::trim).filter(|x| !x.is_empty()) {
        out["path"] = write_table(CMD, path, &rows, "Takeoff")?;
    }
    if bool_or(p, "keep", true) {
        let m: serde_json::Map<String, Value> = now.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        set_state(s, &last_key, json!({"lines": m, "revision": revision}))?;
        // The lines' elements, for bim.takeoff.show.
        let shown: serde_json::Map<String, Value> = out["lines"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|l| Some((l["key"].as_str()?.to_string(), l["elements"].clone())))
            .collect();
        set_state(
            s,
            "takeoff.last",
            json!({"group_by": group_by, "elements": shown, "order": out["lines"].as_array().map(|a| a.iter().map(|l| l["key"].clone()).collect::<Vec<_>>())}),
        )?;
    }
    Ok(out)
}

fn run_show(s: &mut Session, p: &Value) -> Result<Value> {
    if let Some(e) = p.get("elements") {
        return frame(s, &handles_of(e));
    }
    let st = state(s.doc()?);
    let last = &st["takeoff.last"];
    let key = match p.get("line") {
        Some(Value::String(k)) => k.clone(),
        Some(Value::Number(n)) => n
            .as_u64()
            .and_then(|i| last["order"].as_array()?.get(usize::try_from(i).ok()?)?.as_str().map(str::to_string))
            .ok_or_else(|| bad(CMD_SHOW, "no takeoff line at that index"))?,
        _ => return Err(bad(CMD_SHOW, "`line` (a key or an index of the last bim.takeoff) or `elements` is required")),
    };
    let els = last["elements"].get(&key).ok_or_else(|| bad(CMD_SHOW, format!("no line {key} in the last takeoff: run bim.takeoff first")))?;
    let hs: Vec<Handle> = handles_of(els);
    if hs.is_empty() {
        return Err(bad(CMD_SHOW, format!("line {key} has no model elements to show")));
    }
    let mut out = frame(s, &hs)?;
    out["key"] = json!(key);
    Ok(out)
}

// ---------------- estimate ----------------

/// One row of the cost table.
#[derive(Clone, Debug)]
struct Cost {
    item: String,
    unit: String,
    material: f64,
    labor_hrs: f64,
    waste: Option<f64>,
    labor_rate: Option<f64>,
}

fn num(v: Option<&String>) -> Option<f64> {
    v.and_then(|v| v.trim().trim_start_matches('$').replace(',', "").trim_end_matches('%').trim().parse::<f64>().ok()).filter(|v| v.is_finite())
}

fn cost_rows(cmd: &str, p: &Value) -> Result<Vec<Cost>> {
    let rows: Vec<BTreeMap<String, String>> = if let Some(a) = p.get("rows").and_then(Value::as_array) {
        a.iter()
            .take(MAX_ROWS)
            .filter_map(Value::as_object)
            .map(|o| o.iter().map(|(k, v)| (k.to_ascii_lowercase(), v.as_str().map_or_else(|| v.to_string(), str::to_string))).collect())
            .collect()
    } else if let Some(t) = str_param(p, "csv") {
        crate::cmd::estimate::csv_rows(t)
    } else if let Some(path) = str_param(p, "cost_table").map(str::trim).filter(|x| !x.is_empty()) {
        let bytes = crate::readfile::read(path).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
        if bytes.starts_with(b"PK") {
            let cells = crate::cmd::table::xlsx::open(&bytes).and_then(|w| w.cells(None, None)).map_err(|e| bad(cmd, format!("{path}: {e}")))?;
            let mut it = cells.into_iter();
            let header: Vec<String> = it.next().unwrap_or_default().iter().map(|h| h.trim().to_ascii_lowercase()).collect();
            it.take(MAX_ROWS).map(|r| header.iter().cloned().zip(r.into_iter().map(|c| c.trim().to_string())).collect()).collect()
        } else {
            crate::cmd::estimate::csv_rows(&String::from_utf8_lossy(&bytes))
        }
    } else {
        // Q6034: no user cost table — the starter cost book is the fallback.
        return Ok(vec![]);
    };
    let pick = |r: &BTreeMap<String, String>, keys: &[&str]| keys.iter().find_map(|k| r.get(*k)).cloned();
    let out: Vec<Cost> = rows
        .iter()
        .take(MAX_ROWS)
        .filter_map(|r| {
            let item = pick(r, &["item", "description", "name", "cost item"])?.trim().to_string();
            (!item.is_empty()).then(|| Cost {
                item,
                unit: pick(r, &["unit", "uom", "units"]).unwrap_or_default(),
                material: num(pick(r, &["material", "material_cost", "material $", "mat", "price", "unit_cost"]).as_ref()).unwrap_or(0.0),
                labor_hrs: num(pick(r, &["labor_hrs", "labor_hours", "labour_hrs", "labor", "hours", "labor units"]).as_ref()).unwrap_or(0.0),
                waste: num(pick(r, &["waste", "waste_pct", "waste %"]).as_ref()).filter(|w| (0.0..=200.0).contains(w)),
                labor_rate: num(pick(r, &["labor_rate", "rate"]).as_ref()).filter(|v| *v >= 0.0),
            })
        })
        .collect();
    if out.is_empty() {
        return Err(bad(cmd, "the cost table has no rows with an `item` column"));
    }
    Ok(out)
}

fn words(t: &str) -> BTreeSet<String> {
    t.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| w.len() > 1 && !matches!(*w, "of" | "and" | "the" | "for" | "with" | "per" | "each" | "ea" | "ga" | "gauge" | "untyped"))
        .map(|w| if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") { w.trim_end_matches('s') } else { w })
        // One word for what cost tables call by several names.
        .map(|w| match w {
            "drywall" | "gwb" | "gyp" | "wallboard" | "sheetrock" => "gypsum",
            "partition" => "wall",
            "outlet" => "receptacle",
            "luminaire" | "lighting" => "light",
            "doorway" => "door",
            w => w,
        })
        .map(str::to_string)
        .collect()
}

/// Q5301: system and material keywords for the never-cross rule.
const SYSTEM_WORDS: &[&str] =
    &["gas", "vent", "cw", "hw", "dcw", "dhw", "supply", "return", "exhaust", "sanitary", "storm", "fire", "refrigerant", "condensate"];
const MATERIAL_WORDS: &[&str] =
    &["copper", "pvc", "steel", "galvanized", "aluminum", "cast", "iron", "fiberglass", "stainless", "brass", "emt", "imc", "rigid"];
/// Q6371: the building elements a line or row is of; a window's area is no wall's.
const ELEMENT_WORDS: &[&str] = &["wall", "door", "window", "ceiling", "floor", "roof", "stair"];

/// The cost row for a takeoff line: its index and how it matched.
/// Q5301: exact match first, then family match (word-based, never crossing system/material/size).
fn match_row(rows: &[Cost], l: &Value) -> Option<(usize, &'static str)> {
    let item = l["item"].as_str().unwrap_or_default();
    let unit = l["unit"].as_str().unwrap_or_default();
    let unit_ok =
        |r: &Cost| r.unit.trim().is_empty() || measure(&r.unit).is_some_and(|m| Some(m) == measure(unit)) || r.unit.trim().eq_ignore_ascii_case(unit);
    if let Some(i) = rows.iter().position(|r| r.item.trim().eq_ignore_ascii_case(item.trim()) && unit_ok(r)) {
        return Some((i, "item"));
    }
    let have = words(&format!(
        "{item} {} {} {}",
        l["size"].as_str().unwrap_or_default(),
        l["category"].as_str().unwrap_or_default(),
        l["system"].as_str().unwrap_or_default()
    ));
    let mep = matches!(l["category"].as_str().unwrap_or_default(), "duct" | "pipe" | "conduit" | "tray" | "cable");
    let sys_words = if mep { words(l["system"].as_str().unwrap_or_default()) } else { BTreeSet::new() };
    let iw: BTreeSet<String> = words(item).into_iter().filter(|w| !sys_words.contains(w) && !w.chars().any(|c| c.is_ascii_digit())).collect();
    // Line's system and material keywords for the never-cross check.
    let line_sys: BTreeSet<&str> = SYSTEM_WORDS.iter().copied().filter(|s| have.contains(*s)).collect();
    let line_mat: BTreeSet<&str> = MATERIAL_WORDS.iter().copied().filter(|m| have.contains(*m)).collect();
    let line_el: BTreeSet<&str> = ELEMENT_WORDS.iter().copied().filter(|m| have.contains(*m)).collect();
    let mut best: Option<(usize, f64, usize)> = None;
    for (i, r) in rows.iter().enumerate() {
        if !unit_ok(r) {
            continue;
        }
        let want = words(&r.item);
        if want.is_empty() {
            continue;
        }
        let hit = want.iter().filter(|w| have.contains(*w)).count();
        let score = hit as f64 / want.len() as f64;
        let covered = if iw.is_empty() { 0.0 } else { iw.iter().filter(|w| want.contains(*w)).count() as f64 / iw.len() as f64 };
        if hit == 0 || !((score >= 0.5 && covered >= 0.5) || covered >= 1.0) {
            continue;
        }
        // Q5301 NEVER cross: block only when BOTH the row and line name the attribute and they differ.
        let row_sys: BTreeSet<&str> = SYSTEM_WORDS.iter().copied().filter(|s| want.contains(*s)).collect();
        if !row_sys.is_empty() && !line_sys.is_empty() && row_sys != line_sys {
            continue;
        }
        let row_mat: BTreeSet<&str> = MATERIAL_WORDS.iter().copied().filter(|m| want.contains(*m)).collect();
        if !row_mat.is_empty() && !line_mat.is_empty() && row_mat != line_mat {
            continue;
        }
        // Nor across building elements (Q6371): a row of one element never prices another's line.
        let row_el: BTreeSet<&str> = ELEMENT_WORDS.iter().copied().filter(|m| want.contains(*m)).collect();
        if !row_el.is_empty() && !line_el.is_empty() && row_el.is_disjoint(&line_el) {
            continue;
        }
        let score = score + covered;
        if best.is_none_or(|(_, s, n)| score > s || (score == s && want.len() > n)) {
            best = Some((i, score, want.len()));
        }
    }
    best.map(|(i, _, _)| (i, "family"))
}

fn run_estimate(s: &mut Session, p: &Value) -> Result<Value> {
    super::super::exchange::guard_params(s, "bim.estimate", p, &["file"])?;
    // Q6164: material from a price-book workbook, by catalog number, then the Item Map.
    if p.get("price_book").is_some_and(|v| !v.is_null()) {
        let by_level = bool_or(p, "by_level", false);
        let filter = p.get("filter").cloned().unwrap_or(Value::Null);
        let lines = lines(s, if by_level { "level" } else { "type" }, &filter)?;
        return super::priced::run(p, &lines);
    }
    let rows = cost_rows(CMD_EST, p)?;
    // Q6012: starter cost book as fallback, same as estimate.status (estimate.rs price_lines).
    let starter: Vec<Cost> = crate::cmd::estimate::starter_prices()
        .into_iter()
        .filter_map(|v| {
            Some(Cost {
                item: v["item"].as_str()?.to_string(),
                unit: v["unit"].as_str().unwrap_or("each").to_string(),
                material: v["material"].as_f64()?,
                labor_hrs: v["labor_hours"].as_f64().unwrap_or(0.0),
                waste: None,
                labor_rate: None,
            })
        })
        .collect();
    let by_level = bool_or(p, "by_level", false);
    let filter = p.get("filter").cloned().unwrap_or(Value::Null);
    let lines = lines(s, if by_level { "level" } else { "type" }, &filter)?;
    let explicit_rate =
        p.get("labor_rate").and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0).is_some() || rows.iter().any(|r| r.labor_rate.is_some());
    let rate = p
        .get("labor_rate")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .or_else(|| rows.iter().find_map(|r| r.labor_rate))
        .unwrap_or(DEFAULT_LABOR_RATE);
    let waste_for = |l: &Value, r: &Cost| -> f64 {
        let cat = l["category"].as_str().unwrap_or_default();
        r.waste
            .or_else(|| p.get("waste").and_then(Value::as_f64))
            .or_else(|| p["waste"][cat].as_f64())
            .or_else(|| l["waste_pct"].as_f64())
            .filter(|w| w.is_finite() && (0.0..=200.0).contains(w))
            .unwrap_or(0.0)
    };
    let mut out_lines = Vec::new();
    let mut unmatched = Vec::new();
    let mut used = BTreeSet::new();
    let mut used_starter = BTreeSet::new();
    let (mut mat_total, mut hrs_total) = (0.0f64, 0.0f64);
    for l in &lines {
        let qty = l["qty"].as_f64().unwrap_or(0.0);
        if qty <= 0.0 {
            continue;
        }
        // Q5301: user exact/family > starter exact/family > unmatched with suggestion.
        let (r, how, via) = if let Some((i, how)) = match_row(&rows, l) {
            used.insert(i);
            (&rows[i], how, None)
        } else if let Some((i, how)) = match_row(&starter, l) {
            used_starter.insert(i);
            (&starter[i], how, Some(crate::cmd::estimate::STARTER_SOURCE))
        } else {
            // Q6020: suggest the nearest row but never apply it.
            let item_name = l["item"].as_str().unwrap_or_default();
            let nearest: Option<&str> =
                rows.iter().chain(starter.iter()).filter(|r| !r.item.trim().is_empty()).map(|r| r.item.as_str()).min_by_key(|ri| {
                    let rw = words(ri);
                    let lw = words(item_name);
                    let overlap = rw.iter().filter(|w| lw.contains(*w)).count();
                    usize::MAX - overlap
                });
            let reason = if let Some(near) = nearest.filter(|_| !rows.is_empty() || !starter.is_empty()) {
                format!("{} has no exact match; nearest row: {} \u{2014} add a row for this item or map it", item_name, near)
            } else {
                "no cost-table row names this item in this unit".to_string()
            };
            unmatched.push(json!({"item": l["item"], "size": l["size"], "unit": l["unit"], "qty": qty, "level": l["level"], "reason": reason}));
            continue;
        };
        let w = waste_for(l, r);
        let qt = qty * (1.0 + w / 100.0);
        // Material on the quantity with waste; labor on the net (waste isn't installed).
        // This matches the estimate.* engine (estimate.rs price_lines).
        let (m, h) = (qt * r.material, qty * r.labor_hrs);
        mat_total += m;
        hrs_total += h;
        let mut line = json!({
            "key": l["key"], "level": l["level"], "trade": l["trade"], "category": l["category"], "item": l["item"], "size": l["size"], "unit": l["unit"],
            "qty": r3(qty), "waste_pct": r3(w), "qty_total": r3(qt), "cost_item": r.item, "matched_by": how,
            "material_unit": r.material, "labor_hrs_unit": r.labor_hrs,
            "material": r3(m), "labor_hrs": r3(h), "labor": r3(h * rate), "total": r3(m + h * rate),
            "elements": l["element_count"],
        });
        if let Some(v) = via {
            line["via"] = json!(v);
        }
        if how == "family" {
            line["via_family_row"] = json!(r.item);
        }
        out_lines.push(line);
    }
    let unused: Vec<Value> = rows.iter().enumerate().filter(|(i, _)| !used.contains(i)).map(|(_, r)| json!(r.item)).collect();
    let total = mat_total + hrs_total * rate;
    // Warnings for missing inputs (MUST 2: never silently $0).
    let mut warnings: Vec<Value> = Vec::new();
    if !explicit_rate {
        warnings.push(json!({
            "kind": "default_labor_rate",
            "message": format!("using default labor rate ${DEFAULT_LABOR_RATE}/h; neither the call nor the cost table set one"),
            "fix": "pass labor_rate ($/h for your crew) or add a labor_rate column to the cost table",
        }));
    }
    if rate == 0.0 && hrs_total > 0.0 {
        warnings.push(json!({
            "kind": "zero_labor_rate",
            "message": format!("{:.0} labor hours are $0 because labor_rate is 0", hrs_total),
            "fix": "set labor_rate to your crew's hourly rate (e.g. 75 or 90)",
        }));
    }
    if !unmatched.is_empty() {
        warnings.push(json!({
            "kind": "unpriced_lines",
            "message": format!("{} takeoff lines have no cost-table match; their cost is not in the total", unmatched.len()),
            "fix": "add rows to the cost table for each unmatched item, or verify they are not in contract",
        }));
    }
    if !used_starter.is_empty() {
        warnings.push(json!({
            "kind": "starter_cost_book",
            "message": format!("{} line(s) priced with the starter cost book — generic values, not project-specific", used_starter.len()),
            "fix": "add rows to the cost table for each starter-priced item, or import your own costs",
        }));
    }
    let mut out = json!({
        "lines": out_lines, "unmatched": unmatched, "unused_rows": unused,
        "material_total": r3(mat_total), "labor_hours": r3(hrs_total), "labor_total": r3(hrs_total * rate), "total": r3(total), "labor_rate": rate,
        "warnings": warnings,
    });
    // Store the cost table in the estimate state so estimate.status uses the same prices.
    if let Ok(est_sp) = crate::cmd::bspec::current(s, CMD_EST) {
        let (mut sp, _) = est_sp;
        let mut est: serde_json::Map<String, Value> = sp.estimate.as_object().cloned().unwrap_or_default();
        let base = est.entry(String::from("baseline")).or_insert_with(|| json!({}));
        if let Some(b) = base.as_object_mut() {
            let prices: Vec<Value> = rows
                .iter()
                .map(|r| {
                    let mut pv = json!({
                        "item": r.item, "unit": if r.unit.is_empty() { "each" } else { &r.unit },
                        "material": r.material, "source": "bim.estimate cost table", "date": "",
                    });
                    if r.labor_hrs > 0.0 {
                        pv["labor_hours"] = json!(r.labor_hrs);
                    }
                    pv
                })
                .collect();
            // Starter-book prices are NOT stored in the baseline: estimate.status
            // falls back to starter_prices() independently. The two routes match
            // differently (grouped takeoff lines vs individual BOM lines), so
            // storing them would cause price-shadowing.
            b.insert("prices".into(), json!(prices));
        }
        let settings = est.entry(String::from("settings")).or_insert_with(|| json!({}));
        if let Some(s_obj) = settings.as_object_mut() {
            s_obj.insert("crew_rate".into(), json!(rate));
        }
        sp.estimate = Value::Object(est);
        s.doc_mut().map(|d| d.building_spec = Some(sp.to_json()).into()).ok();
    }
    // Change order against an earlier revision.
    let st = state(s.doc()?);
    let earlier: Option<Vec<Value>> = match p.get("compare_to") {
        Some(Value::String(name)) => Some(
            st["estimates"][name.as_str()]["lines"]
                .as_array()
                .cloned()
                .ok_or_else(|| bad(CMD_EST, format!("no estimate revision {name} kept (pass `revision` to keep one)")))?,
        ),
        Some(v) => v["lines"].as_array().cloned(),
        None => None,
    };
    if let Some(before) = earlier {
        let k = |l: &Value| {
            format!(
                "{}|{}|{}|{}",
                l["level"].as_str().unwrap_or_default(),
                l["item"].as_str().unwrap_or_default(),
                l["size"].as_str().unwrap_or_default(),
                l["unit"].as_str().unwrap_or_default()
            )
        };
        let mut all: BTreeMap<String, (Option<&Value>, Option<&Value>)> = BTreeMap::new();
        for l in &before {
            all.entry(k(l)).or_default().0 = Some(l);
        }
        for l in out["lines"].as_array().map(Vec::as_slice).unwrap_or_default() {
            all.entry(k(l)).or_default().1 = Some(l);
        }
        let mut co = Vec::new();
        let mut co_total = 0.0;
        for (_, (b, a)) in all {
            let q = |x: Option<&Value>, f: &str| x.and_then(|x| x[f].as_f64()).unwrap_or(0.0);
            let (bq, aq) = (q(b, "qty"), q(a, "qty"));
            let dc = q(a, "total") - q(b, "total");
            if (aq - bq).abs() < 1e-9 && dc.abs() < 0.005 {
                continue;
            }
            let src = a.or(b);
            co_total += dc;
            co.push(json!({
                "item": src.map(|x| x["item"].clone()), "size": src.map(|x| x["size"].clone()), "unit": src.map(|x| x["unit"].clone()), "level": src.map(|x| x["level"].clone()),
                "before_qty": r3(bq), "after_qty": r3(aq), "delta_qty": r3(aq - bq), "delta_cost": r3(dc),
                "change": if b.is_none() { "added" } else if a.is_none() { "removed" } else { "changed" },
            }));
        }
        out["change_order"] = json!({"lines": co, "total": r3(co_total)});
    }
    let header = ["level", "item", "size", "unit", "qty", "waste_pct", "qty_total", "cost_item", "material", "labor_hrs", "labor", "total"];
    let mut table: Vec<Vec<String>> = vec![header.iter().map(|h| (*h).to_string()).collect()];
    for l in out["lines"].as_array().map(Vec::as_slice).unwrap_or_default() {
        table.push(header.iter().map(|h| l[*h].as_str().map_or_else(|| l[*h].to_string(), str::to_string)).collect());
    }
    table.push(vec![
        String::new(),
        "TOTAL".into(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        r3(mat_total).to_string(),
        r3(hrs_total).to_string(),
        r3(hrs_total * rate).to_string(),
        r3(total).to_string(),
    ]);
    out["csv"] = json!(table.iter().map(|r| r.iter().map(|c| csv_cell(c)).collect::<Vec<_>>().join(",") + "\n").collect::<String>());
    if let Some(path) = str_param(p, "file").map(str::trim).filter(|x| !x.is_empty()) {
        out["path"] = write_table(CMD_EST, path, &table, "Estimate")?;
    }
    if let Some(name) = str_param(p, "revision").map(str::trim).filter(|x| !x.is_empty()) {
        let mut revs = st["estimates"].as_object().cloned().unwrap_or_default();
        if !revs.contains_key(name) && revs.len() >= MAX_REVISIONS {
            return Err(bad(CMD_EST, format!("at most {MAX_REVISIONS} estimate revisions are kept")));
        }
        revs.insert(name.chars().take(100).collect(), json!({"lines": out["lines"], "total": out["total"]}));
        set_state(s, "estimates", Value::Object(revs))?;
        out["revision"] = json!(name);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(item: &str, unit: &str) -> Cost {
        Cost { item: item.into(), unit: unit.into(), material: 1.0, labor_hrs: 0.1, waste: None, labor_rate: None }
    }

    #[test]
    fn cost_rows_match_by_item_then_words_in_a_compatible_unit() {
        let rows = vec![row("Door", "ea"), row("Wall framing", "LF"), row("Duplex receptacle", "each"), row("Ceiling tile", "SF")];
        let l = |item: &str, unit: &str, cat: &str| json!({"item": item, "unit": unit, "category": cat, "size": ""});
        assert_eq!(match_row(&rows, &l("Door", "each", "doors")), Some((0, "item")));
        assert_eq!(match_row(&rows, &l("Wall untyped", "lf", "walls")).map(|m| m.0), Some(1));
        assert_eq!(match_row(&rows, &l("receptacle", "each", "devices")).map(|m| m.0), Some(2));
        assert_eq!(match_row(&rows, &l("Ceiling ACT", "sq ft", "ceilings")).map(|m| m.0), Some(3));
        // The wrong unit, or no shared words: unpriced.
        assert_eq!(match_row(&rows, &l("Wall untyped", "sq ft", "walls")), None);
        assert_eq!(match_row(&rows, &l("Fan coil", "each", "equipment")), None);
        assert_eq!(match_row(&rows, &l("Door hardware set", "each", "doors")), None, "hardware is not the door");
        // Never across building elements (Q6371): a wall's area row prices no window's area.
        let rows = vec![row("Wall area untyped", "sq ft"), row("Window", "each")];
        assert_eq!(match_row(&rows, &l("Window area", "sq ft", "windows")), None, "a window's area is no wall's");
        assert_eq!(match_row(&rows, &l("Window", "each", "windows")), Some((1, "item")));
        assert_eq!(match_row(&rows, &l("Wall area", "sq ft", "walls")).map(|m| m.0), Some(0));
        // What cost tables call by other names.
        let rows =
            vec![row("Interior partition, metal stud", "lf"), row("Drywall 5/8", "sf"), row("Duplex outlet", "ea"), row("LED luminaire", "ea")];
        assert_eq!(match_row(&rows, &l("Wall", "lf", "walls")).map(|m| m.0), Some(0));
        assert_eq!(match_row(&rows, &l("Board gypsum board", "sq ft", "drywall")).map(|m| m.0), Some(1));
        assert_eq!(match_row(&rows, &l("receptacle", "each", "devices")).map(|m| m.0), Some(2));
        assert_eq!(match_row(&rows, &l("Light fixture", "each", "devices")).map(|m| m.0), Some(3));
        // A line that isn't MEP carries its category as its system (`Wall`, system `walls`).
        let mut w = l("Wall", "lf", "walls");
        w["system"] = json!("walls");
        assert_eq!(match_row(&rows, &w).map(|m| m.0), Some(0));
    }

    #[test]
    fn units_fill_their_columns() {
        assert_eq!(measure("LF"), Some("length_ft"));
        assert_eq!(measure("sq ft"), Some("area_sf"));
        assert_eq!(measure("each"), Some("count"));
        assert_eq!(measure("cu yd"), Some("volume_cy"));
        assert_eq!(measure("furlong"), None);
    }
}
