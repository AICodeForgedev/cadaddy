//! `finishes.takeoff` (Q5121): the finish takeoff from the job's prints. It reads the print set's
//! DXF sheets (`dir`, `paths`, else the open print set's), runs the architecture, ceiling and wall
//! readers on them ([`cadaddy_mepfix::finishes::read_input`]) and takes off flooring, base, wall
//! finishes, ceilings, doors/frames/hardware, specialties and glazing by room, trade and unit type
//! (unit types from the building spec's `unit_instances`, as `unittypes.read` writes them). The
//! answer is compact and paged: per-trade totals first, then the lines, each with its source and
//! status. No rooms or finish schedule read → a guiding message, never an error.

use cadaddy_mepfix::finishes::{self as fin, FinishTakeoff, RoomRule, TakeoffInput, TakeoffItem};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::*;
use crate::{Result, Session};

const CMD: &str = "finishes.takeoff";
/// Lines per page.
const PAGE: usize = 50;
/// DXF sheets read at most, and the largest read.
const MAX_SHEETS: usize = 2_000;
#[cfg(not(target_arch = "wasm32"))]
const MAX_SHEET_BYTES: usize = 256 * 1024 * 1024;
/// Room-type rules a call may give at most.
const MAX_RULES: usize = 500;

/// What to say when nothing to take off was read.
pub const NOTHING_READ: &str = "no rooms or room finish schedule have been read from the prints: run arch.read first to see what the architectural sheets give, then finishes.takeoff {dir} with the print set's DXF folder (A floor plans with room tags, and the ROOM FINISHES schedule and FINISH LEGEND), or open the set with printset.open";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("finishes.takeoff", "Finish Takeoff from Prints", run)
            .params("{dir?: folder of the print set's DXF sheets, paths?: [DXF sheet] (none: the open print set's DXF sheets), level?, room?: number, unit_type?, trade?: flooring|painting|ceilings|doors_frames_hardware|specialties|glazing, page?: 1-based (50 lines a page), rules?: [{names: [room name words], rule: text, items: [{description, quantity, category}]}] (the company's room-type standards; default the built-in table)} → {sheets_read, totals: [{trade, lines, quantity: {SF|LF|EA: n}, with_waste: {…}}], count, page, pages, lines: [{trade, description, room, room_name, level, qty, unit, with_waste, source, status, rule?, coats?, sheen?, unit_type?}], by_unit_type?: {type: [{trade, description, unit, quantity, with_waste, rooms}]}, missing: [why], message?}: the takeoff runs on what the readers read from the sheets, never on an answer key")
            .noundo(),
    ]
}

fn is_dxf(p: &str) -> bool {
    p.to_ascii_lowercase().ends_with(".dxf")
}

/// The sheets to read: `paths`, the DXF files in `dir`, else the open print set's DXF sheets.
fn sheet_paths(s: &Session, p: &Value) -> Result<Vec<String>> {
    if let Some(a) = p.get("paths").filter(|v| !v.is_null()) {
        let a = a.as_array().ok_or_else(|| bad(CMD, "`paths` is a list of DXF sheet files"))?;
        if a.len() > MAX_SHEETS {
            return Err(bad(CMD, format!("at most {MAX_SHEETS} sheets at once")));
        }
        return a.iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| bad(CMD, "each of `paths` is a file path"))).collect();
    }
    if let Some(dir) = str_param(p, "dir").map(str::trim).filter(|d| !d.is_empty()) {
        return dxf_in(dir);
    }
    if let Some(proj) = s.printset_project.as_deref() {
        let base = std::path::Path::new(&proj.source_dir);
        let mut out: Vec<String> =
            proj.sheets.iter().filter(|e| is_dxf(&e.file)).map(|e| base.join(&e.file).display().to_string()).take(MAX_SHEETS).collect();
        out.sort();
        out.dedup();
        return Ok(out);
    }
    Ok(Vec::new())
}

#[cfg(not(target_arch = "wasm32"))]
fn dxf_in(dir: &str) -> Result<Vec<String>> {
    let rd = std::fs::read_dir(dir).map_err(|e| bad(CMD, format!("{dir}: {e}")))?;
    let mut out: Vec<String> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_dxf(&p.display().to_string()))
        .map(|p| p.display().to_string())
        .take(MAX_SHEETS + 1)
        .collect();
    if out.len() > MAX_SHEETS {
        return Err(bad(CMD, format!("{dir} has more than {MAX_SHEETS} DXF sheets")));
    }
    out.sort();
    Ok(out)
}

#[cfg(target_arch = "wasm32")]
fn dxf_in(_dir: &str) -> Result<Vec<String>> {
    Err(bad(CMD, "folders are not available on the web"))
}

fn read_bytes(path: &str) -> Result<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let b = crate::readfile::read(path.trim()).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        if b.len() > MAX_SHEET_BYTES {
            return Err(bad(CMD, format!("{path} is larger than {} MB", MAX_SHEET_BYTES / (1024 * 1024))));
        }
        Ok(b)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = path;
        Err(bad(CMD, "files are not available on the web"))
    }
}

/// Each room's unit type from the building spec's unit instances: the instance with its number
/// on its level (a room numbered `301B` belongs to unit `301`).
fn unit_types(s: &Session, input: &mut TakeoffInput) {
    let Some(spec) = s.doc().ok().and_then(|d| d.building_spec.as_deref()).and_then(|t| serde_json::from_str::<Value>(t).ok()) else { return };
    let Some(units) = spec.get("unit_instances").and_then(Value::as_array) else { return };
    let text = |v: &Value, k: &str| match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(o) => o.to_string(),
    };
    let units: Vec<(String, String, String)> = units
        .iter()
        .take(100_000)
        .filter_map(|u| {
            let ty = Some(text(u, "type")).filter(|t| !t.is_empty()).unwrap_or_else(|| text(u, "unit_type"));
            let n = Some(text(u, "number")).filter(|n| !n.is_empty()).unwrap_or_else(|| text(u, "room"));
            (!ty.is_empty() && !n.is_empty()).then(|| (text(u, "level"), n, ty))
        })
        .collect();
    for r in input.rooms.iter_mut() {
        let of = |n: &str| r.number == n || r.number.strip_prefix(n).is_some_and(|rest| rest.chars().next().is_some_and(|c| !c.is_ascii_digit()));
        r.unit_type = units.iter().find(|(l, n, _)| (l.is_empty() || *l == r.level) && of(n)).map(|(_, _, t)| t.clone());
    }
}

fn rules(p: &Value) -> Result<Vec<RoomRule>> {
    match p.get("rules") {
        None | Some(Value::Null) => Ok(fin::default_room_rules()),
        Some(Value::Array(a)) if a.len() <= MAX_RULES => serde_json::from_value(Value::Array(a.clone()))
            .map_err(|e| bad(CMD, format!("`rules`: each is {{names: [words], rule: text, items: [{{description, quantity, category}}]}} ({e})"))),
        Some(_) => Err(bad(CMD, format!("`rules` is a list of at most {MAX_RULES} room-type rules"))),
    }
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn line_json(i: &TakeoffItem) -> Value {
    let mut v = json!({
        "trade": i.trade, "description": i.description, "room": i.room, "room_name": i.room_name, "level": i.level,
        "qty": i.quantity, "unit": i.unit, "with_waste": i.with_waste, "source": i.source, "status": i.status,
    });
    if let Some(o) = v.as_object_mut() {
        for (k, val) in [
            ("rule", i.rule.as_ref().map(|r| json!(r))),
            ("coats", i.coats.map(|c| json!(c))),
            ("sheen", i.sheen.as_ref().map(|x| json!(x))),
            ("unit_type", i.unit_type.as_ref().map(|x| json!(x))),
        ] {
            if let Some(val) = val {
                o.insert(k.into(), val);
            }
        }
    }
    v
}

/// Per-trade totals of some lines: line count and quantity (and with waste) by unit.
fn totals(lines: &[&TakeoffItem]) -> Value {
    let order = ["flooring", "painting", "ceilings", "doors_frames_hardware", "specialties", "glazing"];
    let mut by: BTreeMap<usize, (String, usize, BTreeMap<String, f64>, BTreeMap<String, f64>)> = BTreeMap::new();
    for i in lines {
        let k = order.iter().position(|t| *t == i.trade).unwrap_or(order.len());
        let e = by.entry(k).or_insert_with(|| (i.trade.clone(), 0, BTreeMap::new(), BTreeMap::new()));
        e.1 += 1;
        *e.2.entry(i.unit.clone()).or_default() += i.quantity;
        *e.3.entry(i.unit.clone()).or_default() += i.with_waste;
    }
    let round = |m: BTreeMap<String, f64>| m.into_iter().map(|(u, q)| (u, json!(r2(q)))).collect::<serde_json::Map<String, Value>>();
    Value::Array(by.into_values().map(|(t, n, q, w)| json!({"trade": t, "lines": n, "quantity": round(q), "with_waste": round(w)})).collect())
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let rules = rules(p)?;
    let page = match p.get("page") {
        None | Some(Value::Null) => 1,
        Some(v) => v
            .as_u64()
            .filter(|n| *n >= 1)
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
            .ok_or_else(|| bad(CMD, "`page` is a page number from 1"))?,
    };
    let trade = str_param(p, "trade").map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty());
    if let Some(t) = &trade
        && !["flooring", "painting", "ceilings", "doors_frames_hardware", "specialties", "glazing"].contains(&t.as_str())
    {
        return Err(bad(CMD, "`trade` is one of flooring, painting, ceilings, doors_frames_hardware, specialties, glazing"));
    }
    let level = str_param(p, "level").map(str::trim).filter(|l| !l.is_empty()).map(str::to_string);
    let room = str_param(p, "room").map(str::trim).filter(|r| !r.is_empty()).map(str::to_string);
    let unit_type = str_param(p, "unit_type").map(str::trim).filter(|u| !u.is_empty()).map(str::to_string);

    let paths = sheet_paths(s, p)?;
    let mut dxf: Vec<(String, Vec<u8>)> = Vec::new();
    for path in paths.iter().filter(|p| is_dxf(p)).take(MAX_SHEETS) {
        let name = std::path::Path::new(path.trim()).file_name().and_then(|f| f.to_str()).unwrap_or("sheet.dxf").to_string();
        dxf.push((name, read_bytes(path)?));
    }
    let mut input = fin::read_input(&dxf);
    let with_finishes = input.rooms.iter().filter(|r| r.floor.is_some() || r.wall.is_some() || r.ceiling.is_some()).count();
    if input.rooms.is_empty() || with_finishes == 0 {
        let mut out = json!({
            "sheets_read": dxf.len(), "totals": [], "count": 0, "page": 1, "pages": 0, "lines": [],
            "rooms_read": input.rooms.len(), "missing": input.notes.iter().take(50).collect::<Vec<_>>(), "message": NOTHING_READ,
        });
        if dxf.is_empty() {
            out["message"] = json!(format!("no sheets given and no print set open: {NOTHING_READ}"));
        }
        return Ok(out);
    }
    unit_types(s, &mut input);
    let tk: FinishTakeoff = fin::compute(&input, &rules);

    let keep = |i: &&TakeoffItem| {
        trade.as_deref().is_none_or(|t| i.trade == t)
            && level.as_deref().is_none_or(|l| i.level.eq_ignore_ascii_case(l))
            && room.as_deref().is_none_or(|r| i.room.eq_ignore_ascii_case(r))
            && unit_type.as_deref().is_none_or(|u| i.unit_type.as_deref().is_some_and(|t| t.eq_ignore_ascii_case(u)))
    };
    let kept: Vec<&TakeoffItem> = tk.items.iter().filter(keep).collect();
    let pages = kept.len().div_ceil(PAGE);
    let listed: Vec<Value> = kept.iter().skip(page.saturating_sub(1).saturating_mul(PAGE)).take(PAGE).map(|i| line_json(i)).collect();
    let mut out = json!({
        "sheets_read": dxf.len(),
        "rooms_read": input.rooms.len(),
        "totals": totals(&kept),
        "count": kept.len(),
        "page": page,
        "pages": pages,
        "lines": listed,
        "missing": tk.missing.iter().take(50).collect::<Vec<_>>(),
    });
    if !tk.by_unit_type.is_empty()
        && let Some(o) = out.as_object_mut()
    {
        let by: BTreeMap<&String, &Vec<fin::UnitTypeLine>> =
            tk.by_unit_type.iter().filter(|(k, _)| unit_type.as_deref().is_none_or(|u| k.eq_ignore_ascii_case(u))).collect();
        o.insert("by_unit_type".into(), json!(by));
    }
    if page > pages.max(1)
        && let Some(o) = out.as_object_mut()
    {
        o.insert("message".into(), json!(format!("page {page} is past the last page ({pages})")));
    } else if page < pages
        && let Some(o) = out.as_object_mut()
    {
        o.insert("next_page".into(), json!(page + 1));
    }
    if kept.is_empty()
        && let Some(o) = out.as_object_mut()
    {
        o.insert("message".into(), json!("no takeoff lines match the filters (level, room, unit_type, trade)"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_params_are_refused_not_panics() {
        let mut s = Session::new();
        for p in [
            json!({"page": 0}),
            json!({"page": -3}),
            json!({"page": "x"}),
            json!({"trade": "roofing"}),
            json!({"paths": "a.dxf"}),
            json!({"paths": [1, 2]}),
            json!({"rules": 7}),
            json!({"rules": [{"names": 1}]}),
        ] {
            assert!(run(&mut s, &p).is_err(), "{p}");
        }
        // Nothing to read: the guiding message, not an error.
        let r = run(&mut s, &json!({})).expect("no sheets");
        assert!(r["message"].as_str().is_some_and(|m| m.contains("arch.read")), "{r}");
        let r = run(&mut s, &json!({"paths": ["/nonexistent/none.txt"]})).expect("not a DXF");
        assert_eq!(r["count"], 0);
    }
}
