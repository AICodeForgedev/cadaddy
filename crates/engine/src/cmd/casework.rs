//! `casework.read`: casework (cabinets, counters, vanities, backsplashes, appliances, wardrobes)
//! and plumbing fixtures read from the open architectural floor plan, with the casework /
//! plumbing fixture schedule that names each tag ([`cadaddy_mepfix::casework_reader`]). Every
//! item carries its SOURCE (the sheet, and the schedule when its tag has a row) and STATUS
//! (`schedule`: kind and height from the schedule; `note`: from the plan's tag and AFF note;
//! `pattern`: from the symbol only; `tag`: a fixture named by its tag alone). With `place`, each
//! item that has a 3D library family becomes that model element at the item's footprint
//! (casework stretched to its run), tagged with its id, source and status (xdata
//! `CADADDY_CASEWORK`) so placing again adds only what is new.

use cadaddy_doc::library::models3d::{self, Model};
use cadaddy_doc::{Drawing, EntityKind, RawTag};
use cadaddy_geom::{Bounds2, Vec2};
use cadaddy_mepfix::casework_reader::{self, Reading};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::*;
use crate::{Result, Session};

pub const CASEWORK_APP: &str = "CADADDY_CASEWORK";
/// Items listed per answer by default, and at most.
const PAGE: usize = 50;
const PAGE_MAX: usize = 200;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("casework.read", "Read Casework and Fixtures", run)
            .params("{sheet?: sheet number for the source (default: the title block's, else the drawing's name), kind?: only this kind, place?: bool (default false; true places each item that has a 3D library family as that model), offset?: first item listed (0), limit?: items listed (50, max 200)} → {sheet, schedule: {title, rows} | null, count, by_kind, by_status, items: [{id, category: casework|fixture, tag, kind, family, at, size?, height_in, spec?, source: {sheet, schedule}, status, handle?}], offset, next?, unscheduled, placed?}: reads the open floor plan (one sheet)"),
    ]
}

/// The 3D library family for a kind, if it has one.
fn family(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "base_cabinet" | "vanity" => "M3_CABINET_BASE",
        "upper_cabinet" => "M3_CABINET_WALL",
        "water_closet" => "M3_WATER_CLOSET",
        "lavatory" => "M3_LAVATORY",
        "sink" => "M3_SINK_KITCHEN",
        "urinal" => "M3_URINAL",
        "shower" => "M3_SHOWER",
        "bathtub" => "M3_BATHTUB",
        _ => return None,
    })
}

/// A read item, ready to list or place.
struct Item {
    id: String,
    category: &'static str,
    tag: String,
    kind: String,
    /// Centre and (casework) footprint size, drawing units.
    at: Vec2,
    size: Option<(f64, f64)>,
    height_in: f64,
    spec: Option<String>,
    scheduled: bool,
    status: String,
}

fn items(r: &Reading, sheet: &str) -> Vec<Item> {
    let mut out = Vec::new();
    for (i, c) in r.casework.iter().enumerate() {
        let n = if c.tag.is_empty() { format!("#{}", i + 1) } else { format!("{}#{}", c.tag, i + 1) };
        out.push(Item {
            id: format!("{sheet}:{n}"),
            category: "casework",
            tag: c.tag.clone(),
            kind: c.kind.clone(),
            at: Vec2::new((c.min_in[0] + c.max_in[0]) / 2.0, (c.min_in[1] + c.max_in[1]) / 2.0),
            size: Some((c.max_in[0] - c.min_in[0], c.max_in[1] - c.min_in[1])),
            height_in: c.height_in,
            spec: c.spec_section.clone(),
            scheduled: c.status == "schedule",
            status: c.status.clone(),
        });
    }
    let base = r.casework.len();
    for (i, f) in r.fixtures.iter().enumerate() {
        out.push(Item {
            id: format!("{sheet}:{}#{}", f.tag, base + i + 1),
            category: "fixture",
            tag: f.tag.clone(),
            kind: f.kind.clone(),
            at: Vec2::new(f.at_in[0], f.at_in[1]),
            size: None,
            height_in: f.height_in,
            spec: f.spec_section.clone(),
            scheduled: f.status == "schedule",
            status: f.status.clone(),
        });
    }
    out
}

/// The sheet's number: from the title block, else the drawing's name.
fn sheet_name(s: &Session, d: &Drawing, p: &Value) -> String {
    if let Some(n) = str_param(p, "sheet").map(str::trim).filter(|n| !n.is_empty()) {
        return n.chars().take(40).collect();
    }
    let texts = cadaddy_mepfix::sheet_facts::texts(d);
    cadaddy_mepfix::sheet_facts::identify_sheet(&texts)
        .map(|i| i.number)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| s.state().map(|st| st.title.chars().take(40).collect()).unwrap_or_default())
}

/// A model's plan bounds, inches.
fn plan_bounds(m: &Model) -> Bounds2 {
    let d = Drawing::new_imperial();
    let mut b = Bounds2::EMPTY;
    for e in m.plan() {
        let eb = cadaddy_doc::entity_bounds(&d, &e, 0);
        if !eb.is_empty() {
            b = b.union(&eb);
        }
    }
    b
}

/// Drawing units per inch (INSUNITS; unitless follows MEASUREMENT).
fn units_per_inch(d: &Drawing) -> f64 {
    match d.header.i64("INSUNITS", 0) {
        2 => 1.0 / 12.0,
        4 => 25.4,
        5 => 2.54,
        6 => 0.0254,
        1 => 1.0,
        _ if d.header.i64("MEASUREMENT", 0) == 1 => 25.4,
        _ => 1.0,
    }
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "casework.read";
    let place = bool_or(p, "place", false);
    let kind = str_param(p, "kind").map(str::to_ascii_lowercase).filter(|k| !k.is_empty());
    let offset = p.get("offset").and_then(Value::as_u64).map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let limit = match p.get("limit") {
        None | Some(Value::Null) => PAGE,
        Some(v) => v
            .as_u64()
            .map(|n| usize::try_from(n).unwrap_or(PAGE_MAX).clamp(1, PAGE_MAX))
            .ok_or_else(|| bad(cmd, "`limit` is a count of items, 1 to 200"))?,
    };
    let (reading, sheet, k, placed_ids) = {
        let d = s.doc()?;
        let placed_ids: Vec<String> = d
            .model
            .iter()
            .filter_map(|e| {
                let i = e.common.xdata.iter().position(|t| t.code == 1001 && t.value == CASEWORK_APP)?;
                e.common.xdata.get(i + 1).map(|t| t.value.clone())
            })
            .collect();
        (casework_reader::read(d), sheet_name(s, d, p), units_per_inch(d), placed_ids)
    };
    let all: Vec<Item> = items(&reading, &sheet).into_iter().filter(|i| kind.as_deref().is_none_or(|k| i.kind == k)).collect();
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_status: BTreeMap<&str, usize> = BTreeMap::new();
    for i in &all {
        *by_kind.entry(i.kind.as_str()).or_default() += 1;
        *by_status.entry(i.status.as_str()).or_default() += 1;
    }
    // Place the families first: their handles go into the listed items.
    let mut handles: BTreeMap<String, String> = BTreeMap::new();
    if place {
        let place_cmd = find_command("models3d.place").ok_or_else(|| EngineError::UnknownCommand("models3d.place".into()))?;
        for it in all.iter().filter(|i| !placed_ids.contains(&i.id)) {
            let Some(m) = family(&it.kind).and_then(models3d::model) else { continue };
            let b = plan_bounds(m);
            let (mw, md) = (b.width(), b.height());
            if mw <= 1e-9 || md <= 1e-9 {
                continue;
            }
            // Casework: the model's width (its x, back at y 0) along the footprint's long side,
            // stretched to the run and its depth. Fixtures: the model as it is.
            let (turn, sx, sy) = match it.size {
                Some((w, d)) => {
                    let (long, short) = if d > w { (d, w) } else { (w, d) };
                    (d > w, long / mw, short / md)
                }
                None => (false, k, k),
            };
            if !(sx.is_finite() && sy.is_finite() && sx > 0.0 && sy > 0.0) {
                continue;
            }
            let rot = if turn { std::f64::consts::FRAC_PI_2 } else { 0.0 };
            let c = b.center();
            let at = it.at - Vec2::new(c.x * sx, c.y * sy).rotate(rot);
            let r = (place_cmd.run)(s, &json!({ "name": m.name, "at": [at.x, at.y], "rotation": rot.to_degrees() }))?;
            let Some(h) = r.get("handle").and_then(Value::as_str).and_then(cadaddy_doc::Handle::parse_hex) else { continue };
            let layer = if it.category == "fixture" { casework_reader::FIXTURE_LAYER } else { "CASEWORK" };
            let tags = [it.id.as_str(), it.tag.as_str(), it.kind.as_str(), it.status.as_str(), sheet.as_str()];
            s.doc_mut()?.modify_entity(h, |e| {
                e.common.layer = layer.into();
                let mut x = vec![RawTag { code: 1001, value: CASEWORK_APP.into() }];
                x.extend(tags.iter().map(|v| RawTag { code: 1000, value: (*v).to_string() }));
                e.common.xdata = x.into();
                if let EntityKind::Insert(i) = &mut e.kind {
                    i.scale.x = sx;
                    i.scale.y = sy;
                }
            })?;
            handles.insert(it.id.clone(), h.hex());
        }
        s.echo(format!("{} casework and fixture models placed.", handles.len()));
    }
    let round = |v: f64| (v * 100.0).round() / 100.0;
    let listed: Vec<Value> = all
        .iter()
        .skip(offset)
        .take(limit)
        .map(|i| {
            let mut v = json!({
                "id": i.id, "category": i.category, "tag": i.tag, "kind": i.kind, "family": family(&i.kind),
                "at": [round(i.at.x), round(i.at.y)], "height_in": round(i.height_in),
                "source": {"sheet": sheet, "schedule": if i.scheduled { Some(casework_reader::SCHEDULE_TITLE) } else { None }},
                "status": i.status,
            });
            if let (Some((w, d)), Some(o)) = (i.size, v.as_object_mut()) {
                o.insert("size".into(), json!([round(w), round(d)]));
            }
            if let (Some(sp), Some(o)) = (&i.spec, v.as_object_mut()) {
                o.insert("spec".into(), json!(sp));
            }
            if let (Some(h), Some(o)) = (handles.get(&i.id), v.as_object_mut()) {
                o.insert("handle".into(), json!(h));
            }
            v
        })
        .collect();
    let mut out = json!({
        "sheet": sheet,
        "schedule": if reading.schedule.is_empty() { Value::Null } else { json!({"title": casework_reader::SCHEDULE_TITLE, "rows": reading.schedule.len()}) },
        "count": all.len(), "by_kind": by_kind, "by_status": by_status, "items": listed, "offset": offset,
        "unscheduled": reading.unscheduled.iter().take(50).collect::<Vec<_>>(),
    });
    if offset.saturating_add(limit) < all.len()
        && let Some(o) = out.as_object_mut()
    {
        o.insert("next".into(), json!(offset + limit));
    }
    if place && let Some(o) = out.as_object_mut() {
        o.insert("placed".into(), json!(handles.len()));
    }
    if all.is_empty()
        && let Some(o) = out.as_object_mut()
    {
        o.insert("message".into(), json!("no casework or plumbing fixtures on this drawing (casework rectangles on a CASEWORK layer, fixture symbols on a *FIXT* layer, with their tags)"));
    }
    Ok(out)
}
