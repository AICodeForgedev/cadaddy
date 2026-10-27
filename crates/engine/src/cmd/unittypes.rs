//! Unit types: typical rooms that repeat across floors (Q305). A hotel has a few guest-room
//! types (king, double queen, ADA king, suite) placed dozens of times; each instance records the
//! type, the hand (normal, or mirrored for the opposite corridor side) and any overrides (ADA
//! unit, end unit, a column in the room), each with its source and author.
//!
//! Commands:
//! - `unittypes.read`     — read the types from the enlarged unit plans and the instances from the
//!   floor plans' unit tags (and the overrides table) into the building spec
//! - `unittypes.list`     — list every unit type defined in the building spec
//! - `unittypes.schedule` — every placed instance: room, type, hand, floor, overrides
//! - `unittypes.takeoff`  — quantities by type × count, each instance's overrides applied
//! - `unittypes.edit`     — change a type; every instance follows except the fields it overrides
//! - `unittypes.override` — set or clear an instance's overrides (source and author kept)
//! - `unittypes.place`    — where each instance's items go, mirrored rooms flipped
//! - `unittypes.detect`   — find the units on the open plan (unit tags in rooms) or by room size

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

use super::{CommandSpec, bad, bool_or, str_param};
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("unittypes.read", "Read Unit Types from Prints", run_read)
            .params("{dir?: folder of the set's DXF sheets, files?: [DXF sheet paths]} → {types, instances, overrides, by_type: [{type, name, normal, mirrored}], notes, sheets_read}")
            .files(),
        CommandSpec::new("unittypes.list", "List Unit Types", run_list)
            .params("{} → {types: [{id, name, width_ft, depth_ft, devices, terminals, fixtures, casework, equipment, items, wet_wall_side, hvac_sleeve_side, source, status}]}")
            .noundo(),
        CommandSpec::new("unittypes.schedule", "Unit Schedule", run_schedule)
            .params("{level?: code, type?: id} → {instances: [{id, type, type_name, room, number, level, hand, at, overrides, source, status}], summary: [{type, type_name, count, normal, mirrored}]}")
            .noundo(),
        CommandSpec::new("unittypes.takeoff", "Unit Type Takeoff", run_takeoff)
            .params("{level?: code, instances?: bool (each instance's quantities too)} → {types: [{type, type_name, count, normal, mirrored, devices, terminals, fixtures, casework, equipment, totals, rooms}], grand_total, instances?}")
            .noundo(),
        CommandSpec::new("unittypes.edit", "Edit Unit Type", run_edit)
            .params("{type: id, set: {field: value} (name, width_ft, depth_ft, ceiling_height_ft, wet_wall_side, hvac_sleeve_side, ceiling_kind, wall_assembly, ada, or a count category.kind (category devices, terminals, fixtures, casework or equipment) such as devices.receptacle), author?: name, preview?: bool} → {type, changed: [{field, from, to}], instances, kept: [{number, level, field, value}]}"),
        CommandSpec::new("unittypes.override", "Override Unit Instance", run_override)
            .params("{number: room number, room?: the same (an alias of number), level?: code, set?: {key: value} (ada, end_unit, column, or a count category.kind (category devices, terminals, fixtures, casework or equipment) such as casework.wardrobe), remove?: [key], author?: name, note?: text} → {instance}"),
        CommandSpec::new("unittypes.place", "Place Unit Instances", run_place)
            .params("{level?: code, number?: room number} → {instances: [{number, level, type, hand, wet_wall_side, hvac_sleeve_side, items: [{category, kind, tag, at_ft}], unplaced: [{category, kind, count}] (kinds an override changes)}]}")
            .noundo(),
        CommandSpec::new("unittypes.detect", "Detect Unit Types", run_detect)
            .params("{tolerance?: ft (default 1.0)} → {detected: [{room, number, name, width_ft, depth_ft, suggested_type?, hand?, confidence, by}], types: [{id, name, count, normal, mirrored}]}")
            .noundo(),
    ]
}

// --------------- helpers ---------------

/// What a unit type counts per unit, by category (each `{kind: count}`).
const CATEGORIES: [&str; 5] = ["devices", "terminals", "fixtures", "casework", "equipment"];
/// Unit types looked at, at most.
const MAX_TYPES: usize = 10_000;
/// Unit instances looked at, at most.
const MAX_INSTANCES: usize = 50_000;
/// Items of a type, overrides of an instance, edits of a type, at most.
const MAX_ITEMS: usize = 1_000;
/// DXF sheets read at most, and the largest read.
const MAX_SHEETS: usize = 2_000;
const MAX_SHEET_BYTES: u64 = 256 * 1024 * 1024;
/// Who a change is by when the call doesn't say.
const DEFAULT_AUTHOR: &str = "user";

/// The drawing's building spec as JSON (`{}` without one).
fn spec_value(s: &Session) -> Result<Value> {
    Ok(s.doc()?.building_spec.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()).filter(Value::is_object).unwrap_or_else(|| json!({})))
}

/// Write `keys` (`unit_types`, `unit_instances`) into the drawing's building spec, the rest kept
/// (a drawing without one gets the spec its rules give, as `bspec.set` would).
fn write_spec(s: &mut Session, cmd: &str, keys: Map<String, Value>) -> Result<()> {
    let stored = s.doc()?.building_spec.clone();
    let mut base = match stored.as_deref() {
        Some(t) => serde_json::from_str::<Value>(t)
            .ok()
            .filter(Value::is_object)
            .ok_or_else(|| bad(cmd, "the stored building spec isn't readable; fix it with bspec.set first"))?,
        None => serde_json::to_value(super::bspec::current(s, cmd)?.0).map_err(|e| bad(cmd, e.to_string()))?,
    };
    if let Some(b) = base.as_object_mut() {
        b.extend(keys);
    }
    s.doc_mut()?.building_spec = Some(base.to_string());
    Ok(())
}

fn array<'a>(v: &'a Value, key: &str, cap: usize) -> impl Iterator<Item = &'a Value> {
    v.get(key).and_then(Value::as_array).into_iter().flatten().take(cap)
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// A JSON value as text (a number as written).
fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// An instance's type: `type`, else `unit_type` (both are written: the field view's and the
/// generator's).
fn inst_type(i: &Value) -> &str {
    Some(text(i, "type")).filter(|t| !t.is_empty()).unwrap_or_else(|| text(i, "unit_type"))
}

/// An instance's room number (else its room).
fn inst_number(i: &Value) -> String {
    let n = i.get("number").map(as_text).unwrap_or_default();
    if n.is_empty() { i.get("room").map(as_text).unwrap_or_default() } else { n }
}

/// A type's id (else its tag).
fn type_id(t: &Value) -> &str {
    Some(text(t, "id")).filter(|x| !x.is_empty()).unwrap_or_else(|| text(t, "tag"))
}

/// An instance's overrides as (key, value): a list of `{key, value, …}` or a `{key: value}` map.
fn overrides_of(i: &Value) -> Vec<(String, String)> {
    match i.get("overrides") {
        Some(Value::Array(a)) => a
            .iter()
            .take(MAX_ITEMS)
            .filter_map(|o| Some((o.get("key")?.as_str()?.to_string(), o.get("value").map(as_text).unwrap_or_default())))
            .collect(),
        Some(Value::Object(m)) => m.iter().take(MAX_ITEMS).map(|(k, v)| (k.clone(), as_text(v))).collect(),
        _ => Vec::new(),
    }
}

/// A count from hostile JSON: a whole number, a finite non-negative number (cut to a whole one,
/// saturating), or a numeral; anything else is 0.
fn count(v: &Value) -> u64 {
    if let Some(n) = v.as_u64() {
        return n;
    }
    let float = |f: f64| if f.is_finite() && f > 0.0 { f.floor() as u64 } else { 0 };
    if let Some(f) = v.as_f64() {
        // A finite, non-negative float converts saturating (`as` caps at u64::MAX).
        return float(f);
    }
    v.as_str().map(str::trim).and_then(|s| s.parse::<u64>().ok().or_else(|| s.parse::<f64>().ok().map(float))).unwrap_or(0)
}

/// Per-unit counts: category → kind → count.
type Counts = BTreeMap<String, BTreeMap<String, u64>>;

fn type_counts(t: &Value) -> Counts {
    let mut out = Counts::new();
    for c in CATEGORIES {
        let m = out.entry(c.to_string()).or_default();
        for (k, v) in t.get(c).and_then(Value::as_object).into_iter().flatten().take(MAX_ITEMS) {
            m.insert(k.clone(), count(v));
        }
    }
    out
}

/// A type's counts with an instance's overrides (`devices.receptacle` = `3`) applied.
fn instance_counts(t: Option<&Value>, overrides: &[(String, String)]) -> Counts {
    let mut c = t.map(type_counts).unwrap_or_default();
    for (k, v) in overrides {
        if let Some((cat, kind)) = k.split_once('.')
            && CATEGORIES.contains(&cat)
        {
            c.entry(cat.to_string()).or_default().insert(kind.to_string(), count(&json!(v)));
        }
    }
    c
}

fn add_into(total: &mut Counts, c: &Counts) {
    for (cat, m) in c {
        let t = total.entry(cat.clone()).or_default();
        for (k, v) in m {
            let e = t.entry(k.clone()).or_default();
            *e = e.saturating_add(*v);
        }
    }
}

fn sum(m: Option<&BTreeMap<String, u64>>) -> u64 {
    m.into_iter().flatten().fold(0u64, |a, (_, v)| a.saturating_add(*v))
}

/// The types and the instances of a spec.
fn units(spec: &Value) -> (Vec<&Value>, Vec<&Value>) {
    (array(spec, "unit_types", MAX_TYPES).collect(), array(spec, "unit_instances", MAX_INSTANCES).collect())
}

fn find_type<'a>(types: &[&'a Value], id: &str) -> Option<&'a Value> {
    types.iter().copied().find(|t| type_id(t) == id)
}

fn type_name(types: &[&Value], id: &str) -> String {
    find_type(types, id).map(|t| text(t, "name")).filter(|n| !n.is_empty()).unwrap_or(id).to_string()
}

fn level_ok(i: &Value, level: Option<&str>) -> bool {
    level.is_none_or(|f| text(i, "level").eq_ignore_ascii_case(f))
}

// --------------- read ---------------

fn run_read(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "unittypes.read";
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    if let Some(dir) = str_param(p, "dir") {
        let entries = std::fs::read_dir(dir).map_err(|e| bad(CMD, format!("can't read the folder {dir}: {e}")))?;
        for e in entries.flatten().take(MAX_SHEETS * 4) {
            let path = e.path();
            if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("dxf")) {
                paths.push(path);
            }
        }
    }
    for f in array(p, "files", MAX_SHEETS) {
        let Some(f) = f.as_str() else { return Err(bad(CMD, "`files` must be a list of paths")) };
        paths.push(f.into());
    }
    if paths.is_empty() {
        return Err(bad(CMD, "give `dir` (the folder of the print set's DXF sheets) or `files`"));
    }
    paths.sort();
    paths.truncate(MAX_SHEETS);
    let mut dxf: Vec<(String, Vec<u8>)> = Vec::new();
    for path in &paths {
        let too_big = std::fs::metadata(path).map(|m| m.len() > MAX_SHEET_BYTES).unwrap_or(true);
        if too_big {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else { continue };
        // The sheet number is the file's name (`A-401.dxf`).
        let stem = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        dxf.push((format!("{stem}.dxf"), bytes));
    }
    if dxf.is_empty() {
        return Err(bad(CMD, "no readable DXF sheets were given"));
    }
    let map = cadaddy_mepfix::sheet_reader::recognise_fixture_set(&dxf);
    let r = cadaddy_mepfix::unit_reader::read_units(&dxf, &map);
    let stored = spec_value(s)?;
    let Value::Object(keys) = cadaddy_mepfix::unit_reader::spec_patch(&r, &stored) else { return Err(bad(CMD, "nothing read")) };
    write_spec(s, CMD, keys)?;
    let by_type: Vec<Value> = r
        .types
        .iter()
        .map(|t| {
            let n = |hand: &str| r.units.iter().filter(|u| u.type_tag == t.tag && u.hand == hand).count();
            json!({"type": t.tag, "name": t.name, "normal": n("normal"), "mirrored": n("mirrored"), "source": t.sheet})
        })
        .collect();
    let mut out = json!({
        "types": r.types.len(), "instances": r.units.len(), "overrides": r.units.iter().map(|u| u.overrides.len()).sum::<usize>(),
        "by_type": by_type, "notes": r.notes.iter().take(200).collect::<Vec<_>>(), "sheets_read": dxf.len(),
    });
    if r.types.is_empty() {
        out["next_step"] =
            json!("no enlarged unit plans were found: the set needs an A-sheet titled like ENLARGED GUEST UNIT PLANS with a UNIT PLAN LEGEND");
    }
    Ok(out)
}

// --------------- list ---------------

fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let spec = spec_value(s)?;
    let types: Vec<Value> = array(&spec, "unit_types", MAX_TYPES).cloned().collect();
    let mut out = json!({ "types": types });
    if types.is_empty() {
        out["next_step"] = json!("no unit types in the building spec: run unittypes.read {dir} on the print set's DXF sheets");
    }
    Ok(out)
}

// --------------- schedule ---------------

fn run_schedule(s: &mut Session, p: &Value) -> Result<Value> {
    let level_filter = str_param(p, "level");
    let type_filter = str_param(p, "type");
    let spec = spec_value(s)?;
    let (types, instances) = units(&spec);
    let mut rows = Vec::new();
    let mut summary_map: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    for inst in instances.iter().filter(|i| level_ok(i, level_filter)) {
        let ut = inst_type(inst);
        if type_filter.is_some_and(|f| ut != f) {
            continue;
        }
        let overrides: Value = match inst.get("overrides") {
            Some(Value::Object(m)) => Value::Array(m.iter().take(MAX_ITEMS).map(|(k, v)| json!({"key": k, "value": as_text(v)})).collect()),
            Some(Value::Array(a)) => Value::Array(a.iter().take(MAX_ITEMS).cloned().collect()),
            _ => json!([]),
        };
        let hand = Some(text(inst, "hand")).filter(|h| !h.is_empty()).unwrap_or("normal");
        rows.push(json!({
            "id": text(inst, "id"), "type": ut, "type_name": type_name(&types, ut), "room": inst.get("room").map(as_text).unwrap_or_default(),
            "number": inst_number(inst), "level": text(inst, "level"), "hand": hand,
            "at": inst.get("at_ft").or_else(|| inst.get("at")).cloned().unwrap_or(Value::Null), "overrides": overrides,
            "source": inst.get("source").cloned().unwrap_or(Value::Null), "status": text(inst, "status"),
        }));
        let e = summary_map.entry(ut.to_string()).or_default();
        e.0 = e.0.saturating_add(1);
        if hand == "mirrored" {
            e.2 = e.2.saturating_add(1);
        } else {
            e.1 = e.1.saturating_add(1);
        }
    }
    let summary: Vec<Value> = summary_map
        .iter()
        .map(|(k, (count, normal, mirrored))| json!({"type": k, "type_name": type_name(&types, k), "count": count, "normal": normal, "mirrored": mirrored}))
        .collect();
    let mut out = json!({ "instances": rows, "summary": summary });
    if instances.is_empty() {
        out["next_step"] = json!("no unit instances in the building spec: run unittypes.read {dir} on the print set's DXF sheets");
    }
    Ok(out)
}

// --------------- takeoff ---------------

/// One type's line of the takeoff.
#[derive(Default)]
struct TypeLine {
    count: u64,
    normal: u64,
    mirrored: u64,
    counts: Counts,
    rooms: Vec<String>,
}

fn run_takeoff(s: &mut Session, p: &Value) -> Result<Value> {
    let level_filter = str_param(p, "level");
    let spec = spec_value(s)?;
    let (types, instances) = units(&spec);
    let mut by: BTreeMap<String, TypeLine> = BTreeMap::new();
    let mut each: Vec<Value> = Vec::new();
    let want_each = bool_or(p, "instances", false);
    for inst in instances.iter().filter(|i| level_ok(i, level_filter)) {
        let id = inst_type(inst);
        let ov = overrides_of(inst);
        let c = instance_counts(find_type(&types, id), &ov);
        let line = by.entry(id.to_string()).or_default();
        line.count = line.count.saturating_add(1);
        if text(inst, "hand") == "mirrored" {
            line.mirrored = line.mirrored.saturating_add(1);
        } else {
            line.normal = line.normal.saturating_add(1);
        }
        add_into(&mut line.counts, &c);
        if line.rooms.len() < 500 {
            line.rooms.push(inst_number(inst));
        }
        if want_each {
            let mut v = json!({"number": inst_number(inst), "level": text(inst, "level"), "type": id, "hand": text(inst, "hand"),
                "overridden": ov.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()});
            for cat in CATEGORIES {
                v[cat] = json!(c.get(cat).cloned().unwrap_or_default());
            }
            each.push(v);
        }
    }
    if by.is_empty() && types.is_empty() {
        // No unit types in the spec: the unit-type tags the plan carries (rooms named for them,
        // else the tags themselves), with the device marks inside those rooms.
        return plan_takeoff(s, level_filter);
    }
    let mut grand = Counts::new();
    let mut type_rows = Vec::new();
    for (id, line) in &by {
        add_into(&mut grand, &line.counts);
        let mut v = json!({"type": id, "type_name": type_name(&types, id), "count": line.count, "normal": line.normal, "mirrored": line.mirrored, "rooms": line.rooms});
        let mut totals = json!({});
        for cat in CATEGORIES {
            v[cat] = json!(line.counts.get(cat).cloned().unwrap_or_default());
            totals[cat] = json!(sum(line.counts.get(cat)));
        }
        v["totals"] = totals;
        if find_type(&types, id).is_none() {
            v["note"] = json!("no unit type with this id in the spec: its quantities are its overrides only");
        }
        type_rows.push(v);
    }
    let mut grand_total = json!({});
    for cat in CATEGORIES {
        grand_total[cat] = json!(grand.get(cat).cloned().unwrap_or_default());
    }
    let mut out = json!({ "types": type_rows, "grand_total": grand_total, "source": "spec" });
    if want_each {
        out["instances"] = json!(each);
    }
    if by.is_empty() {
        out["next_step"] = json!("unit types but no instances: run unittypes.read {dir} on the floor plans with the unit tags");
    }
    Ok(out)
}

// --------------- edit ---------------

/// The value an edited field (or a count override) may take.
fn edit_value(cmd: &str, field: &str, v: &Value) -> Result<Value> {
    if let Some((cat, kind)) = field.split_once('.') {
        if !CATEGORIES.contains(&cat) || kind.is_empty() || kind.len() > 64 {
            return Err(bad(cmd, format!("`{field}`: a count is `<{}>.<kind>`", CATEGORIES.join("|"))));
        }
        let n = v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())).filter(|n| *n <= 1_000_000);
        return n.map(|n| json!(n)).ok_or_else(|| bad(cmd, format!("`{field}` must be a whole number from 0 to 1000000")));
    }
    match field {
        "name" => {
            v.as_str().filter(|s| !s.trim().is_empty() && s.len() <= 200).map(|s| json!(s.trim())).ok_or_else(|| bad(cmd, "`name` must be text"))
        }
        "width_ft" | "depth_ft" | "ceiling_height_ft" => v
            .as_f64()
            .filter(|x| x.is_finite() && *x > 0.0 && *x <= 1_000.0)
            .map(|x| json!(x))
            .ok_or_else(|| bad(cmd, format!("`{field}` must be feet above 0"))),
        "wet_wall_side" | "hvac_sleeve_side" => {
            v.as_str().filter(|s| matches!(*s, "left" | "right")).map(|s| json!(s)).ok_or_else(|| bad(cmd, format!("`{field}` is left or right")))
        }
        "ceiling_kind" | "wall_assembly" => {
            v.as_str().filter(|s| s.len() <= 64).map(|s| json!(s)).ok_or_else(|| bad(cmd, format!("`{field}` must be text")))
        }
        "ada" => v.as_bool().map(|b| json!(b)).ok_or_else(|| bad(cmd, "`ada` is true or false")),
        _ => Err(bad(cmd, format!("`{field}` can't be edited (id, tag, items and source are the reader's)"))),
    }
}

fn run_edit(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "unittypes.edit";
    let Some(id) = str_param(p, "type").filter(|t| !t.is_empty()) else { return Err(bad(CMD, "`type` (a unit type id) is required")) };
    let Some(set) = p.get("set").and_then(Value::as_object).filter(|m| !m.is_empty()) else {
        return Err(bad(CMD, "`set` ({field: value}) is required"));
    };
    if set.len() > 100 {
        return Err(bad(CMD, "at most 100 fields at once"));
    }
    let author = str_param(p, "author").filter(|a| !a.trim().is_empty()).unwrap_or(DEFAULT_AUTHOR);
    let mut checked: Vec<(String, Value)> = Vec::new();
    for (field, v) in set {
        checked.push((field.clone(), edit_value(CMD, field, v)?));
    }
    let mut spec = spec_value(s)?;
    let Some(types) = spec.get_mut("unit_types").and_then(Value::as_array_mut) else {
        return Err(bad(CMD, "no unit types in the building spec: run unittypes.read first"));
    };
    let Some(ty) = types.iter_mut().take(MAX_TYPES).find(|t| t.is_object() && type_id(t) == id) else {
        return Err(bad(CMD, format!("no unit type {id}")));
    };
    let mut changed = Vec::new();
    for (field, value) in &checked {
        let from = match field.split_once('.') {
            Some((c, k)) => ty.get(c).and_then(|m| m.get(k)).cloned().unwrap_or(json!(0)),
            None => ty.get(field.as_str()).cloned().unwrap_or(Value::Null),
        };
        cadaddy_mepfix::unit_reader::set_field(ty, field, value.clone());
        changed.push(json!({"field": field, "from": from, "to": value}));
        // The edit is kept (by field, the latest wins) so a re-read makes it again.
        if !ty.get("edits").is_some_and(Value::is_array) {
            ty["edits"] = json!([]);
        }
        if let Some(edits) = ty.get_mut("edits").and_then(Value::as_array_mut) {
            edits.retain(|e| e.get("field").and_then(Value::as_str) != Some(field.as_str()));
            if edits.len() < MAX_ITEMS {
                edits.push(json!({"field": field, "value": value, "author": author}));
            }
        }
    }
    let ty = ty.clone();
    let (_, instances) = units(&spec);
    let of_type: Vec<&Value> = instances.into_iter().filter(|i| inst_type(i) == id).collect();
    let mut kept = Vec::new();
    for i in &of_type {
        for (k, v) in overrides_of(i) {
            if checked.iter().any(|(f, _)| *f == k) {
                kept.push(json!({"number": inst_number(i), "level": text(i, "level"), "field": k, "value": v}));
            }
        }
    }
    let n = of_type.len();
    let preview = bool_or(p, "preview", false);
    let types_now = spec.get("unit_types").cloned().unwrap_or(json!([]));
    if !preview {
        let mut keys = Map::new();
        keys.insert("unit_types".into(), types_now);
        write_spec(s, CMD, keys)?;
    }
    Ok(json!({"type": ty, "changed": changed, "instances": n, "kept": kept, "preview": preview}))
}

// --------------- override ---------------

fn run_override(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "unittypes.override";
    let number = p.get("number").or_else(|| p.get("room")).map(as_text).filter(|n| !n.trim().is_empty());
    let Some(number) = number else { return Err(bad(CMD, "`number` (the room number) is required")) };
    let level = str_param(p, "level");
    let set = p.get("set").and_then(Value::as_object);
    let remove: Vec<String> = array(p, "remove", MAX_ITEMS).filter_map(|k| k.as_str().map(str::to_string)).collect();
    if set.is_none_or(Map::is_empty) && remove.is_empty() {
        return Err(bad(CMD, "give `set` ({key: value}) or `remove` ([key])"));
    }
    let author = str_param(p, "author").filter(|a| !a.trim().is_empty()).unwrap_or(DEFAULT_AUTHOR).to_string();
    let note: String = str_param(p, "note").unwrap_or_default().chars().take(500).collect();
    let mut spec = spec_value(s)?;
    let Some(instances) = spec.get_mut("unit_instances").and_then(Value::as_array_mut) else {
        return Err(bad(CMD, "no unit instances in the building spec: run unittypes.read first"));
    };
    let hits: Vec<usize> =
        instances.iter().take(MAX_INSTANCES).enumerate().filter(|(_, i)| inst_number(i) == number && level_ok(i, level)).map(|(n, _)| n).collect();
    let idx = match hits.as_slice() {
        [one] => *one,
        [] => return Err(bad(CMD, format!("no unit instance in room {number}"))),
        _ => return Err(bad(CMD, format!("room {number} is on several levels: give `level`"))),
    };
    let Some(inst) = instances.get_mut(idx) else { return Err(bad(CMD, "no such instance")) };
    let mut list: Vec<Value> = match inst.get("overrides") {
        Some(Value::Array(a)) => a.iter().take(MAX_ITEMS).cloned().collect(),
        Some(Value::Object(m)) => m.iter().take(MAX_ITEMS).map(|(k, v)| json!({"key": k, "value": as_text(v)})).collect(),
        _ => Vec::new(),
    };
    list.retain(|o| !remove.iter().any(|k| Some(k.as_str()) == o.get("key").and_then(Value::as_str)));
    for (k, v) in set.into_iter().flatten() {
        if k.is_empty() || k.len() > 64 {
            return Err(bad(CMD, "an override key is 1 to 64 characters"));
        }
        if k.contains('.') {
            edit_value(CMD, k, v)?;
        }
        list.retain(|o| o.get("key").and_then(Value::as_str) != Some(k.as_str()));
        if list.len() >= MAX_ITEMS {
            return Err(bad(CMD, "too many overrides on one instance"));
        }
        list.push(json!({"key": k, "value": as_text(v), "author": author, "note": note, "source": {"command": CMD}}));
    }
    let Some(obj) = inst.as_object_mut() else { return Err(bad(CMD, format!("the unit instance in room {number} isn't an object"))) };
    obj.insert("overrides".into(), Value::Array(list));
    let out = inst.clone();
    let mut keys = Map::new();
    keys.insert("unit_instances".into(), spec.get("unit_instances").cloned().unwrap_or(json!([])));
    write_spec(s, CMD, keys)?;
    Ok(json!({"instance": out}))
}

// --------------- place ---------------

fn flip_side(side: &str, mirrored: bool) -> String {
    match (side, mirrored) {
        ("left", true) => "right".into(),
        ("right", true) => "left".into(),
        (s, _) => s.into(),
    }
}

fn run_place(s: &mut Session, p: &Value) -> Result<Value> {
    let level = str_param(p, "level");
    let number = p.get("number").map(as_text);
    let spec = spec_value(s)?;
    let (types, instances) = units(&spec);
    let num = |v: Option<&Value>| v.and_then(Value::as_f64).filter(|x| x.is_finite());
    let mut out = Vec::new();
    for inst in instances.iter().filter(|i| level_ok(i, level)).filter(|i| number.as_ref().is_none_or(|n| inst_number(i) == *n)) {
        let id = inst_type(inst);
        let Some(ty) = find_type(&types, id) else { continue };
        let (Some(w), Some(d)) = (num(ty.get("width_ft")), num(ty.get("depth_ft"))) else { continue };
        let at = inst.get("at_ft").or_else(|| inst.get("at")).and_then(Value::as_array);
        let (Some(x0), Some(y0)) = (num(at.and_then(|a| a.first())), num(at.and_then(|a| a.get(1)))) else { continue };
        let mirrored = text(inst, "hand") == "mirrored";
        let typed = type_counts(ty);
        let counts = instance_counts(Some(ty), &overrides_of(inst));
        let of = |c: &Counts, cat: &str, kind: &str| c.get(cat).and_then(|m| m.get(kind)).copied().unwrap_or(0);
        let mut items = Vec::new();
        let mut placed: BTreeMap<(String, String), u64> = BTreeMap::new();
        for it in array(ty, "items", MAX_ITEMS) {
            let (cat, kind) = (text(it, "category"), text(it, "kind"));
            let (Some(u), Some(v)) = (num(it.get("u")), num(it.get("v"))) else { continue };
            // A kind whose count the instance overrides (a wardrobe left out for a column,
            // receptacles added to an end unit) isn't where the type's plan has it.
            let want = of(&counts, cat, kind);
            if want != of(&typed, cat, kind) {
                continue;
            }
            let n = placed.entry((cat.to_string(), kind.to_string())).or_default();
            if *n >= want {
                continue;
            }
            *n += 1;
            let u = if mirrored { 1.0 - u } else { u };
            let pt = json!([x0 + u * w, y0 + v * d]);
            items.push(json!({"category": cat, "kind": kind, "tag": text(it, "tag"), "at_ft": pt}));
            if kind == "ptac" {
                // The PTAC's wall sleeve and its receptacle go where it goes.
                items.push(json!({"category": cat, "kind": "ptac_sleeve", "tag": text(it, "tag"), "at_ft": pt}));
                items.push(json!({"category": "devices", "kind": "ptac_receptacle", "tag": text(it, "tag"), "at_ft": pt}));
            }
        }
        // What the overrides change: counted, not placed.
        let mut unplaced = Vec::new();
        for (cat, m) in &counts {
            for (kind, want) in m {
                if *want > 0 && *want != of(&typed, cat, kind) {
                    unplaced.push(json!({"category": cat, "kind": kind, "count": want}));
                }
            }
        }
        out.push(json!({
            "number": inst_number(inst), "level": text(inst, "level"), "type": id, "hand": if mirrored { "mirrored" } else { "normal" },
            "wet_wall_side": flip_side(text(ty, "wet_wall_side"), mirrored), "hvac_sleeve_side": flip_side(text(ty, "hvac_sleeve_side"), mirrored),
            "at_ft": [x0, y0], "items": items, "unplaced": unplaced,
        }));
    }
    Ok(json!({"instances": out}))
}

// --------------- takeoff from the plan's tags ---------------

/// Words that start a unit type's tag (`DOUBLE Q`, `SINGLE Q`, `ADA KING`, `UNIT B`, `TYPE A1`).
const UNIT_WORDS: &[&str] = &["SINGLE", "DOUBLE", "KING", "QUEEN", "ADA", "SUITE", "STUDIO", "UNIT", "TYPE", "1BR", "2BR", "3BR", "EFFICIENCY"];

/// A unit type a tag names (normalised: upper case, single spaces), if it names one.
pub(crate) fn unit_type_of(text: &str) -> Option<String> {
    let t = text.trim().to_ascii_uppercase().split_whitespace().collect::<Vec<_>>().join(" ");
    let first = t.split(' ').next()?;
    let words = t.split(' ').count();
    // Equipment named like a unit (`UNIT HEATER`, `UNIT VENTILATOR`) and notes are no unit type.
    let equipment =
        t.split(' ').any(|w| matches!(w, "HEATER" | "HEATERS" | "VENTILATOR" | "VENTILATORS" | "SUBSTATION" | "PRICE" | "PRICES" | "WEIGHT"));
    (UNIT_WORDS.contains(&first) && words <= 4 && t.len() <= 30 && !equipment && !t.chars().any(|c| matches!(c, '\'' | '"' | ':'))).then_some(t)
}

/// Unit tags looked at, at most.
const MAX_UNIT_TAGS: usize = 50_000;

fn plan_takeoff(s: &mut Session, level: Option<&str>) -> Result<Value> {
    let rooms = super::rooms3d::rooms(s)?;
    let d = s.doc()?;
    let plan: Vec<&std::sync::Arc<cadaddy_doc::Entity>> = d
        .model
        .iter()
        .filter(|e| matches!(e.kind, cadaddy_doc::EntityKind::Text(_) | cadaddy_doc::EntityKind::MText(_)))
        .take(MAX_UNIT_TAGS * 4)
        .collect();
    let notes = super::perceive::texts(&plan);
    let in_level = |l: &Option<String>| level.is_none_or(|f| l.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(f)));
    // Each unit: a room whose name (or a tag inside it) is a unit type.
    let boxes: Vec<cadaddy_geom::Bounds2> = rooms.iter().map(|r| cadaddy_geom::Bounds2::from_points(r.outline.iter().copied())).collect();
    let inside = |i: usize, p: cadaddy_geom::Vec2| {
        boxes.get(i).is_some_and(|b| b.contains(p)) && rooms.get(i).is_some_and(|r| cadaddy_geom::point_in_polygon(&r.outline, p))
    };
    let mut units: Vec<(String, Option<usize>)> = Vec::new();
    for (i, r) in rooms.iter().enumerate().filter(|(_, r)| in_level(&r.level)) {
        let ty = unit_type_of(&r.name).or_else(|| notes.iter().filter(|t| inside(i, t.2)).find_map(|t| unit_type_of(&t.1)));
        if let Some(ty) = ty {
            units.push((ty, Some(i)));
        }
    }
    // Without rooms holding them: the tags on the plan, one unit each.
    let by_room = !units.is_empty();
    if !by_room && level.is_none() {
        units = notes.iter().filter_map(|t| unit_type_of(&t.1)).take(MAX_UNIT_TAGS).map(|t| (t, None)).collect();
    }
    let mut by: BTreeMap<String, (u32, BTreeMap<String, u32>)> = BTreeMap::new();
    for (ty, room) in &units {
        let e = by.entry(ty.clone()).or_default();
        e.0 += 1;
        if let Some(i) = *room {
            for t in notes.iter().filter(|t| cadaddy_mepfix::sheet_facts::is_type_tag(&t.1) && inside(i, t.2)) {
                *e.1.entry(t.1.trim().to_string()).or_default() += 1;
            }
        }
    }
    let mut grand: BTreeMap<String, u32> = BTreeMap::new();
    let types: Vec<Value> = by
        .iter()
        .map(|(ty, (count, devices))| {
            for (k, v) in devices {
                *grand.entry(k.clone()).or_default() += v;
            }
            json!({
                "type": ty, "type_name": ty, "count": count, "devices": devices, "fixtures": {}, "casework": {},
                "totals": {"devices": devices.values().sum::<u32>(), "fixtures": 0, "casework": 0},
            })
        })
        .collect();
    let mut out = json!({
        "types": types,
        "grand_total": {"devices": grand, "fixtures": {}, "casework": {}},
        "source": if by_room { "plan rooms" } else { "plan tags" },
    });
    if by.is_empty() {
        out["next_step"] =
            json!("no unit types in the spec or unit-type tags on the plan: define them (bspec.set unit_types) or run unittypes.detect");
    }
    Ok(out)
}

// --------------- detect ---------------

/// A unit tag found on the open plan, in its room.
struct PlanUnit {
    tag: String,
    mirrored: bool,
    number: String,
    name: String,
    room: cadaddy_geom::Bounds2,
}

/// The unit tags (`UNIT A`, `UNIT A (MIRROR)`) on the open plan, each in its room (the smallest
/// closed rectangle round it) with the room number and name in that room.
fn plan_units(d: &cadaddy_doc::Drawing) -> Vec<PlanUnit> {
    use cadaddy_doc::EntityKind;
    use cadaddy_geom::{Bounds2, Vec2};
    let mut texts: Vec<(String, Vec2, f64)> = Vec::new();
    let mut boxes: Vec<Bounds2> = Vec::new();
    for e in d.model.iter().take(MAX_UNIT_TAGS * 20) {
        match &e.kind {
            EntityKind::Text(t) => {
                let p = t.align_pt.unwrap_or(t.insert);
                texts.push((t.value.trim().to_string(), Vec2::new(p.x, p.y), t.height));
            }
            EntityKind::MText(t) => texts.push((t.contents.trim().to_string(), Vec2::new(t.insert.x, t.insert.y), t.height)),
            EntityKind::LwPolyline(p) if p.closed && p.vertices.len() == 4 => {
                let b = Bounds2::from_points(p.vertices.iter().map(|v| v.p));
                if b.width().is_finite() && b.height().is_finite() && b.width() > 0.0 && b.height() > 0.0 {
                    boxes.push(b);
                }
            }
            _ => {}
        }
    }
    texts.retain(|t| t.1.is_finite() && t.2.is_finite());
    let mut out = Vec::new();
    for (s, at, _) in &texts {
        if out.len() >= MAX_UNIT_TAGS {
            break;
        }
        let Some((tag, mirrored)) = cadaddy_mepfix::unit_reader::unit_tag(s) else { continue };
        let Some(room) =
            boxes.iter().filter(|b| b.contains(*at)).min_by(|a, b| (a.width() * a.height()).total_cmp(&(b.width() * b.height()))).copied()
        else {
            continue;
        };
        let in_room: Vec<&(String, Vec2, f64)> = texts.iter().filter(|t| room.contains(t.1)).take(200).collect();
        let Some(number) = in_room.iter().find(|t| (2..=4).contains(&t.0.len()) && t.0.chars().all(|c| c.is_ascii_digit())) else { continue };
        let h = number.2.abs().max(1e-9);
        let name = in_room
            .iter()
            .filter(|t| t.1.y > number.1.y && t.1.y < number.1.y + 2.5 * h && t.0.chars().any(|c| c.is_ascii_alphabetic()))
            .map(|t| t.0.clone())
            .next()
            .unwrap_or_default();
        out.push(PlanUnit { tag, mirrored, number: number.0.clone(), name, room });
    }
    out
}

/// Find the units: on the open plan, the unit tag in each room names its type and hand (checked
/// against the type's size); without tags, the building spec's rooms matched to the types by
/// width and depth within `tolerance` feet.
fn run_detect(s: &mut Session, p: &Value) -> Result<Value> {
    let tolerance = super::f64_or(p, "tolerance", 1.0).clamp(0.0, 100.0);
    let spec = spec_value(s)?;
    let (types, _) = units(&spec);
    let upf = spec.get("units_per_foot").and_then(Value::as_f64).filter(|u| u.is_finite() && *u > 0.0).unwrap_or(12.0);
    let mut detected = Vec::new();
    let mut type_counts: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    let mut tally = |id: &str, mirrored: bool| {
        let e = type_counts.entry(id.to_string()).or_default();
        e.0 = e.0.saturating_add(1);
        if mirrored {
            e.2 = e.2.saturating_add(1);
        } else {
            e.1 = e.1.saturating_add(1);
        }
    };
    let found = plan_units(s.doc()?);
    for u in &found {
        let (w, dep) = (u.room.width() / upf, u.room.height() / upf);
        let ty = find_type(&types, &u.tag);
        // The room drawn is its inside face: the type's size less a wall, within the tolerance.
        let fits = ty.is_some_and(|t| {
            let size = |k: &str| t.get(k).and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(0.0);
            (size("width_ft") - w).abs() <= tolerance && (size("depth_ft") - dep).abs() <= tolerance
        });
        let hand = if u.mirrored { "mirrored" } else { "normal" };
        tally(&u.tag, u.mirrored);
        detected.push(json!({
            "room": u.number, "number": u.number, "name": u.name, "width_ft": w, "depth_ft": dep,
            "suggested_type": u.tag, "hand": hand, "confidence": if fits { 1.0 } else if ty.is_some() { 0.6 } else { 0.4 }, "by": "tag",
        }));
    }
    if found.is_empty() {
        // No unit tags on the plan: the spec's rooms by size.
        for rm in array(&spec, "rooms", MAX_INSTANCES) {
            let corner =
                |k: &str, i: usize| rm.get(k).and_then(Value::as_array).and_then(|a| a.get(i)).and_then(Value::as_f64).filter(|x| x.is_finite());
            let (Some(x0), Some(y0), Some(x1), Some(y1)) = (corner("min_ft", 0), corner("min_ft", 1), corner("max_ft", 0), corner("max_ft", 1))
            else {
                continue;
            };
            let (w, dep) = ((x1 - x0).abs(), (y1 - y0).abs());
            let mut best: Option<(&Value, f64)> = None;
            for ut in &types {
                let size = |k: &str| ut.get(k).and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(0.0);
                let (dw, dd) = ((w - size("width_ft")).abs(), (dep - size("depth_ft")).abs());
                if dw <= tolerance && dd <= tolerance && best.is_none_or(|(_, d)| dw + dd < d) {
                    best = Some((ut, dw + dd));
                }
            }
            let (suggested, confidence) = match best {
                Some((ut, dist)) => {
                    tally(type_id(ut), false);
                    let conf = if dist < 0.1 || tolerance <= 0.0 { 1.0 } else { (1.0 - dist / tolerance).max(0.0) };
                    (Some(type_id(ut).to_string()), conf)
                }
                None => (None, 0.0),
            };
            detected.push(json!({
                "room": text(rm, "id"), "number": text(rm, "number"), "name": text(rm, "name"), "width_ft": w, "depth_ft": dep,
                "suggested_type": suggested, "confidence": confidence, "by": "size",
            }));
        }
    }
    let type_summary: Vec<Value> = type_counts
        .iter()
        .map(|(id, (count, normal, mirrored))| json!({"id": id, "name": type_name(&types, id), "count": count, "normal": normal, "mirrored": mirrored}))
        .collect();
    let mut out = json!({ "detected": detected, "types": type_summary });
    if detected.is_empty() {
        out["next_step"] =
            json!("no unit tags on the open plan and no sized rooms in the spec: open a floor plan with unit tags, or run unittypes.read");
    }
    Ok(out)
}
