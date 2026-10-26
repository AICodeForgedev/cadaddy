//! Low-voltage and fire alarm commands: `lv.devices`, `lv.sleeves`, `lv.bom`, `lv.connections`.
//!
//! Paged, compact answers for agents: each command returns a JSON object with counts and a page
//! of items.  Data comes from the drawing's embedded `lv-data.json`, populated by the printset
//! pipeline's LV stage or by `lv.read`.

use super::*;
use crate::{Result, Session};
use serde_json::{Value, json};
use std::sync::Arc;

const LV_FILE: &str = "lv-data.json";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("lv.devices", "LV/FA Devices", run_devices)
            .params("{system?: fire_alarm|data|security|tv|telephone|nurse_call|sound|all, level?: string, page?: number, page_size?: number} → {total, page, items: [{kind, tag, level, room, system, mounting, candela?, slc?, nac?}]}"),
        CommandSpec::new("lv.sleeves", "LV Sleeves", run_sleeves)
            .params("{level?: string, system?: string, page?: number, page_size?: number} → {total, page, items: [{id, systems, level, penetrates, material, size_in, fire_rating, cables}]}"),
        CommandSpec::new("lv.bom", "LV Bill of Materials", run_bom)
            .params("{has_lv_docs?: bool, page?: number, page_size?: number} → {total, page, items: [{system, category, description, quantity, unit, assumed, scope}]}"),
        CommandSpec::new("lv.connections", "Equipment Connections", run_connections)
            .params("{level?: string, kind?: string, page?: number, page_size?: number} → {total, page, items: [{kind, tag, level, circuit, panel, connection_type}]}"),
    ]
}

fn page_params(p: &Value) -> (usize, usize) {
    let page = p.get("page").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
    let size = p.get("page_size").and_then(Value::as_u64).unwrap_or(50).clamp(1, 500) as usize;
    (page, size)
}

fn paginate(items: Vec<Value>, page: usize, size: usize) -> Value {
    let total = items.len();
    let start = (page - 1) * size;
    let page_items: Vec<Value> = items.into_iter().skip(start).take(size).collect();
    json!({ "total": total, "page": page, "page_size": size, "items": page_items })
}

fn lv_data(s: &Session) -> Value {
    let d = match s.doc() {
        Ok(d) => d,
        Err(_) => return json!({}),
    };
    d.embedded.get(LV_FILE).and_then(|b| serde_json::from_slice::<Value>(b).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}))
}

pub fn set_lv_data(s: &mut Session, data: Value) -> Result<()> {
    let bytes = serde_json::to_vec(&data).map_err(|e| crate::EngineError::Other(e.to_string()))?;
    s.doc_mut()?.embedded.insert(LV_FILE.into(), Arc::new(bytes));
    Ok(())
}

fn run_devices(s: &mut Session, p: &Value) -> Result<Value> {
    let system = p.get("system").and_then(Value::as_str).unwrap_or("all");
    let level = p.get("level").and_then(Value::as_str);
    let (page, size) = page_params(p);
    let data = lv_data(s);
    let mut items = Vec::new();
    if system == "all" || system == "fire_alarm" {
        for d in data.get("fa_devices").and_then(Value::as_array).unwrap_or(&Vec::new()) {
            if let Some(lf) = level
                && d.get("level").and_then(Value::as_str) != Some(lf)
            {
                continue;
            }
            items.push(d.clone());
        }
    }
    if system == "all" || system != "fire_alarm" {
        for d in data.get("tc_devices").and_then(Value::as_array).unwrap_or(&Vec::new()) {
            if system != "all" && d.get("lv_system").and_then(Value::as_str) != Some(system) {
                continue;
            }
            if let Some(lf) = level
                && d.get("level").and_then(Value::as_str) != Some(lf)
            {
                continue;
            }
            items.push(d.clone());
        }
    }
    Ok(paginate(items, page, size))
}

fn run_sleeves(s: &mut Session, p: &Value) -> Result<Value> {
    let level = p.get("level").and_then(Value::as_str);
    let system = p.get("system").and_then(Value::as_str);
    let (page, size) = page_params(p);
    let data = lv_data(s);
    let empty = Vec::new();
    let mut items = Vec::new();
    for sl in data.get("sleeves").and_then(Value::as_array).unwrap_or(&empty) {
        if let Some(lf) = level
            && sl.get("level").and_then(Value::as_str) != Some(lf)
        {
            continue;
        }
        if let Some(sf) = system
            && !sl.get("systems").and_then(Value::as_array).unwrap_or(&empty).iter().any(|v| v.as_str() == Some(sf))
        {
            continue;
        }
        items.push(sl.clone());
    }
    Ok(paginate(items, page, size))
}

fn run_bom(s: &mut Session, p: &Value) -> Result<Value> {
    let (page, size) = page_params(p);
    let data = lv_data(s);
    let items: Vec<Value> = data.get("bom").and_then(Value::as_array).cloned().unwrap_or_default();
    Ok(paginate(items, page, size))
}

fn run_connections(s: &mut Session, p: &Value) -> Result<Value> {
    let level = p.get("level").and_then(Value::as_str);
    let kind = p.get("kind").and_then(Value::as_str);
    let (page, size) = page_params(p);
    let data = lv_data(s);
    let mut items = Vec::new();
    for ec in data.get("connections").and_then(Value::as_array).unwrap_or(&Vec::new()) {
        if let Some(lf) = level
            && ec.get("level").and_then(Value::as_str) != Some(lf)
        {
            continue;
        }
        if let Some(kf) = kind
            && ec.get("kind").and_then(Value::as_str) != Some(kf)
        {
            continue;
        }
        items.push(ec.clone());
    }
    Ok(paginate(items, page, size))
}
