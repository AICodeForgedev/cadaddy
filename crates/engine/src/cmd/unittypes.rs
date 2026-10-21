//! Unit types: typical rooms that repeat across floors (Q305). A hotel has four guest-room
//! types (king, double queen, ADA king, suite) placed dozens of times; each instance records
//! the type, the hand (normal or mirrored for the opposite corridor side) and any overrides
//! (ADA unit, end unit, a column in the room).
//!
//! Commands:
//! - `unittypes.list`     — list every unit type defined in the building spec
//! - `unittypes.schedule` — a table of every placed instance: room, type, hand, floor, overrides
//! - `unittypes.takeoff`  — quantities grouped by type × count (devices, fixtures, casework)
//! - `unittypes.detect`   — infer unit types from the rooms in the drawing

use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::{CommandSpec, bool_or, str_param};
use crate::{Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("unittypes.list", "List Unit Types", run_list)
            .params("{} → {types: [{id, name, width_ft, depth_ft, ceiling_height_ft, ceiling_kind, wall_assembly, ada, devices, fixtures, casework}]}")
            .noundo(),
        CommandSpec::new("unittypes.schedule", "Unit Schedule", run_schedule)
            .params("{level?: code, type?: id, table?: bool (insert a table)} → {instances: [{id, type, type_name, room, number, level, hand, at, overrides}], summary: [{type, type_name, count, normal, mirrored}]}")
            .noundo(),
        CommandSpec::new("unittypes.takeoff", "Unit Type Takeoff", run_takeoff)
            .params("{level?: code} → {types: [{type, type_name, count, devices, fixtures, casework, totals}], grand_total}")
            .noundo(),
        CommandSpec::new("unittypes.detect", "Detect Unit Types", run_detect)
            .params("{tolerance?: ft (default 1.0)} → {detected: [{room, name, width_ft, depth_ft, suggested_type?, confidence}], types: [{id, name, count}]}")
            .noundo(),
    ]
}

// --------------- helpers ---------------

/// Read the building spec's unit types from the drawing's building_spec JSON.
fn read_unit_types(s: &Session) -> Result<Vec<serde_json::Value>> {
    let st = s.state()?;
    let d = st.drawing();
    let spec = d.building_spec.as_deref().unwrap_or("{}");
    let obj: Value = serde_json::from_str(spec).unwrap_or(json!({}));
    Ok(obj.get("unit_types").and_then(Value::as_array).cloned().unwrap_or_default())
}

fn read_unit_instances(s: &Session) -> Result<Vec<serde_json::Value>> {
    let st = s.state()?;
    let d = st.drawing();
    let spec = d.building_spec.as_deref().unwrap_or("{}");
    let obj: Value = serde_json::from_str(spec).unwrap_or(json!({}));
    Ok(obj.get("unit_instances").and_then(Value::as_array).cloned().unwrap_or_default())
}

// --------------- list ---------------

fn run_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let types = read_unit_types(s)?;
    Ok(json!({ "types": types }))
}

// --------------- schedule ---------------

fn run_schedule(s: &mut Session, p: &Value) -> Result<Value> {
    let level_filter = str_param(p, "level");
    let type_filter = str_param(p, "type");

    let types = read_unit_types(s)?;
    let instances = read_unit_instances(s)?;

    let type_name = |id: &str| -> String {
        types.iter()
            .find(|t| t.get("id").and_then(Value::as_str) == Some(id))
            .and_then(|t| t.get("name").and_then(Value::as_str))
            .unwrap_or(id)
            .to_string()
    };

    let mut rows = Vec::new();
    for inst in &instances {
        let lv = inst.get("level").and_then(Value::as_str).unwrap_or("");
        let ut = inst.get("unit_type").and_then(Value::as_str).unwrap_or("");
        if level_filter.is_some_and(|f| lv != f) { continue; }
        if type_filter.is_some_and(|f| ut != f) { continue; }
        let mut row = inst.clone();
        if let Some(obj) = row.as_object_mut() {
            obj.insert("type_name".into(), json!(type_name(ut)));
            // Rename unit_type to type for the output
            if let Some(v) = obj.remove("unit_type") {
                obj.insert("type".into(), v);
            }
        }
        rows.push(row);
    }

    // Summary: count per type, normal vs mirrored
    let mut summary_map: BTreeMap<String, (String, u32, u32, u32)> = BTreeMap::new();
    for row in &rows {
        let ut = row.get("type").and_then(Value::as_str).unwrap_or("");
        let hand = row.get("hand").and_then(Value::as_str).unwrap_or("normal");
        let e = summary_map.entry(ut.to_string()).or_insert_with(|| (type_name(ut), 0, 0, 0));
        e.1 += 1;
        if hand == "normal" { e.2 += 1; } else { e.3 += 1; }
    }
    let summary: Vec<Value> = summary_map.iter().map(|(k, (name, count, normal, mirrored))| {
        json!({"type": k, "type_name": name, "count": count, "normal": normal, "mirrored": mirrored})
    }).collect();

    // Insert table if requested
    if bool_or(p, "table", false) {
        // Table insertion would go here — for now just return the data
    }

    Ok(json!({ "instances": rows, "summary": summary }))
}

// --------------- takeoff ---------------

fn run_takeoff(s: &mut Session, p: &Value) -> Result<Value> {
    let level_filter = str_param(p, "level");

    let types = read_unit_types(s)?;
    let instances = read_unit_instances(s)?;

    // Count instances per type (optionally filtered by level)
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for inst in &instances {
        let lv = inst.get("level").and_then(Value::as_str).unwrap_or("");
        let ut = inst.get("unit_type").and_then(Value::as_str).unwrap_or("");
        if level_filter.is_some_and(|f| lv != f) { continue; }
        *counts.entry(ut.to_string()).or_default() += 1;
    }

    let mut type_rows = Vec::new();
    let mut grand_devices: BTreeMap<String, u32> = BTreeMap::new();
    let mut grand_fixtures: BTreeMap<String, u32> = BTreeMap::new();
    let mut grand_casework: BTreeMap<String, u32> = BTreeMap::new();

    for ut_val in &types {
        let id = ut_val.get("id").and_then(Value::as_str).unwrap_or("");
        let name = ut_val.get("name").and_then(Value::as_str).unwrap_or("");
        let count = counts.get(id).copied().unwrap_or(0);
        if count == 0 { continue; }

        // Multiply per-unit counts by instance count
        let devices = ut_val.get("devices").and_then(Value::as_object).cloned().unwrap_or_default();
        let fixtures = ut_val.get("fixtures").and_then(Value::as_object).cloned().unwrap_or_default();
        let casework = ut_val.get("casework").and_then(Value::as_object).cloned().unwrap_or_default();

        let mut dev_totals = BTreeMap::new();
        for (k, v) in &devices {
            let per_unit = v.as_u64().unwrap_or(0) as u32;
            let total = per_unit * count;
            dev_totals.insert(k.clone(), total);
            *grand_devices.entry(k.clone()).or_default() += total;
        }
        let mut fix_totals = BTreeMap::new();
        for (k, v) in &fixtures {
            let per_unit = v.as_u64().unwrap_or(0) as u32;
            let total = per_unit * count;
            fix_totals.insert(k.clone(), total);
            *grand_fixtures.entry(k.clone()).or_default() += total;
        }
        let mut cw_totals = BTreeMap::new();
        for (k, v) in &casework {
            let per_unit = v.as_u64().unwrap_or(0) as u32;
            let total = per_unit * count;
            cw_totals.insert(k.clone(), total);
            *grand_casework.entry(k.clone()).or_default() += total;
        }

        type_rows.push(json!({
            "type": id,
            "type_name": name,
            "count": count,
            "devices": dev_totals,
            "fixtures": fix_totals,
            "casework": cw_totals,
            "totals": {
                "devices": dev_totals.values().sum::<u32>(),
                "fixtures": fix_totals.values().sum::<u32>(),
                "casework": cw_totals.values().sum::<u32>(),
            }
        }));
    }

    Ok(json!({
        "types": type_rows,
        "grand_total": {
            "devices": grand_devices,
            "fixtures": grand_fixtures,
            "casework": grand_casework,
        }
    }))
}

// --------------- detect ---------------

/// Detect unit types from room geometry in the drawing. Groups rooms by size and suggests
/// type matches.
fn run_detect(s: &mut Session, p: &Value) -> Result<Value> {
    let tolerance = super::f64_or(p, "tolerance", 1.0);
    let st = s.state()?;
    let d = st.drawing();

    // Read the building spec to find rooms
    let spec = d.building_spec.as_deref().unwrap_or("{}");
    let obj: Value = serde_json::from_str(spec).unwrap_or(json!({}));

    let types = read_unit_types(s)?;
    let rooms = obj.get("rooms").and_then(Value::as_array).cloned().unwrap_or_default();

    let mut detected = Vec::new();
    let mut type_counts: BTreeMap<String, u32> = BTreeMap::new();

    for rm in &rooms {
        let name = rm.get("name").and_then(Value::as_str).unwrap_or("");
        let min_ft = rm.get("min_ft").and_then(Value::as_array);
        let max_ft = rm.get("max_ft").and_then(Value::as_array);
        let (w, dep) = match (min_ft, max_ft) {
            (Some(lo), Some(hi)) => {
                let x0 = lo.first().and_then(Value::as_f64).unwrap_or(0.0);
                let y0 = lo.get(1).and_then(Value::as_f64).unwrap_or(0.0);
                let x1 = hi.first().and_then(Value::as_f64).unwrap_or(0.0);
                let y1 = hi.get(1).and_then(Value::as_f64).unwrap_or(0.0);
                ((x1 - x0).abs(), (y1 - y0).abs())
            }
            _ => continue,
        };

        // Find best matching type by size
        let mut best: Option<(&Value, f64)> = None;
        for ut in &types {
            let tw = ut.get("width_ft").and_then(Value::as_f64).unwrap_or(0.0);
            let td = ut.get("depth_ft").and_then(Value::as_f64).unwrap_or(0.0);
            let dw = (w - tw).abs();
            let dd = (dep - td).abs();
            if dw <= tolerance && dd <= tolerance {
                let dist = dw + dd;
                if best.is_none() || dist < best.map(|(_, d)| d).unwrap_or(f64::MAX) {
                    best = Some((ut, dist));
                }
            }
        }

        let (suggested, confidence) = match best {
            Some((ut, dist)) => {
                let id = ut.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                let conf = if dist < 0.1 { 1.0 } else { (1.0 - dist / tolerance).max(0.0) };
                *type_counts.entry(id.clone()).or_default() += 1;
                (Some(id), conf)
            }
            None => (None, 0.0),
        };

        detected.push(json!({
            "room": rm.get("id").and_then(Value::as_str).unwrap_or(""),
            "name": name,
            "width_ft": w,
            "depth_ft": dep,
            "suggested_type": suggested,
            "confidence": confidence,
        }));
    }

    let type_summary: Vec<Value> = type_counts.iter().map(|(id, count)| {
        let name = types.iter()
            .find(|t| t.get("id").and_then(Value::as_str) == Some(id))
            .and_then(|t| t.get("name").and_then(Value::as_str))
            .unwrap_or(id);
        json!({"id": id, "name": name, "count": count})
    }).collect();

    Ok(json!({ "detected": detected, "types": type_summary }))
}

// Tests for the truth-level unit type system are in crates/mepfix/src/hotel.rs (they need
// cadaddy_mepfix which is not a dependency of the engine crate).
