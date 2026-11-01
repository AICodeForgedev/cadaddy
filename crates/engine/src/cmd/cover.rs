//! Pre-cover hold points (Q334 / Q5119) as commands and the MCP `cover` tool, on
//! [`cadaddy_mepfix::covercheck`]:
//!
//! - `cover.init {model}`: a cover record from the print set's `truth.json` (each plan wall's two
//!   faces and each room with a ceiling get the rough-in checklist; the AHJ phases per level).
//! - `cover.status`: per level and per area, paged.
//! - `cover.check` / `cover.uncheck`: a checklist item done or reopened (reopening takes
//!   "ready to cover" off).
//! - `cover.sign {phase, level}`: an AHJ inspection signed for one level.
//! - `cover.ready {area}`: the hold point. Refused while an item is open or a phase that applies
//!   is unsigned, with what is still open.
//! - `cover.wall {wall | at, level}`: a wall's faces, their checklists and photos; a tap position
//!   picks the face on that side.
//! - `cover.photo {at, level, uri}`: an in-wall photo filed on the wall face where it was taken.
//!
//! The record is kept in the drawing (embedded `covercheck.json`) with the plan it was made
//! from (`covercheck-plan.json`: walls, rooms, levels), so it travels with the job's file. Every
//! load re-checks it ([`CoverRecord::validate`]): an area that claims ready with open items or
//! unsigned phases is no longer ready, and the answer says which.

use std::sync::Arc;

use cadaddy_mepfix::covercheck::{
    self as cc, AreaKind, ClearedArea, CoverArea, CoverRecord, InWallPhoto, PHOTO_REACH_FT, add_photo_at, clamp_text, record_from_truth,
    resolve_wall_tap,
};
use cadaddy_mepfix::truth::{PlanWall, Room, Truth};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{CommandSpec, bad, bool_or, str_param};
use crate::{Result, Session};

const INIT: &str = "cover.init";
const STATUS: &str = "cover.status";
const CHECK: &str = "cover.check";
const UNCHECK: &str = "cover.uncheck";
const SIGN: &str = "cover.sign";
const READY: &str = "cover.ready";
const WALL: &str = "cover.wall";
const PHOTO: &str = "cover.photo";

/// The embedded file the drawing keeps its cover record in.
pub const RECORD_FILE: &str = "covercheck.json";
/// The embedded file with the plan the record was made from.
pub const PLAN_FILE: &str = "covercheck-plan.json";
/// Areas per status page: by default and at most.
const PAGE: usize = 50;
const PAGE_MAX: usize = 500;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(INIT, "Start Cover Check", run_init)
            .params("{model: the print set's truth.json or its folder, project?, replace?: bool (default false: refused when the drawing has a record)} → {areas, wall_faces, ceilings, phases, levels}: every plan wall's two faces and every room with a ceiling get the rough-in checklist; the AHJ phases are one per phase and level")
            .files(),
        CommandSpec::new(STATUS, "Cover Check Status", run_status)
            .params("{level?, kind?: wall|ceiling, ready?: bool, offset?, limit? (default 50, at most 500)} → {project, levels: [{level, walls_total, walls_ready, ceilings_total, ceilings_ready, photos, phases_signed, phases_total}], total, offset, areas: [{id, kind, level, face?, rooms, done, items, photos, ready}], next_offset?, phases: [{id, name, level, signed}], cleared?: [{area, open_items, unsigned_phases}], hint?}")
            .noundo(),
        CommandSpec::new(CHECK, "Check Cover Item", run_check)
            .params("{area: id (W1-003/A, CLG-1-101), item: id or category (boxes, cable, protection_plates, firestop, sleeves, grounding, inspection_signed), by?, date?, notes?} → {area, item, done, items_done, items, ready}"),
        CommandSpec::new(UNCHECK, "Reopen Cover Item", run_uncheck)
            .params("{area, item: id or category} → {area, item, was_ready, ready: false}: work undone or failed; the area is no longer ready to cover"),
        CommandSpec::new(SIGN, "Sign Inspection", run_sign)
            .params("{phase: id or name (rough_elec or Rough-In Electrical), level, inspector?, date?, permit?, notes?} → {signed: [phase], level}: the AHJ sign-off for that phase on that level only"),
        CommandSpec::new(READY, "Ready to Cover", run_ready)
            .params("{area, by?, date?} → {area, ready, open_items?: [\"id: category\"], unsigned_phases?: [\"name (level)\"], reason?}: marked ready only when every checklist item is done and every phase that applies is signed; otherwise nothing changes and the answer lists what is open"),
        CommandSpec::new(WALL, "Cover Check Wall", run_wall)
            .params("{wall?: id | at?: [x, y] (building feet), level?: name (with at), max_distance_ft? (default 3)} → {wall, level, tapped?: area, photos?: [the tapped face's photos], faces: [{area, face, rooms, fire_rating?, ready, checklist: [{id, category, done, by?}], photos: [{id, uri, caption, author, date, elevation_ft, along_ft}]}]}")
            .noundo(),
        CommandSpec::new(PHOTO, "Add In-Wall Photo", run_photo)
            .params("{action?: add, at: [x, y] (building feet, where it was taken), level, uri: the photo file, id?, face?: A|B (refused if the position says the other side), elevation_ft?, author?, date?, caption?, max_distance_ft? (default 3)} → {id, area, face, along_ft}: filed on the face of the nearest wall on that side; no wall near: refused with the reason"),
    ]
}

// ---- What the drawing keeps --------------------------------------------------------------------

/// The plan a record was made from.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CoverPlan {
    pub walls: Vec<PlanWall>,
    pub rooms: Vec<Room>,
    /// Level names and floor elevations, feet.
    pub levels: Vec<(String, f64)>,
}

impl CoverPlan {
    pub fn elevation(&self, level: &str) -> Option<f64> {
        self.levels.iter().find(|(n, _)| n == level).map(|(_, e)| *e)
    }
}

/// The drawing's cover record, validated, and the areas validation took "ready" off. `None`
/// when the drawing has none.
pub fn stored(s: &Session, cmd: &str) -> Result<Option<(CoverRecord, Vec<ClearedArea>)>> {
    let d = s.doc()?;
    let Some(b) = d.embedded.get(RECORD_FILE) else { return Ok(None) };
    let mut rec: CoverRecord = serde_json::from_slice(b).map_err(|e| bad(cmd, format!("the drawing's cover record is unreadable: {e}")))?;
    let cleared = rec.validate();
    Ok(Some((rec, cleared)))
}

fn need(s: &Session, cmd: &str) -> Result<(CoverRecord, Vec<ClearedArea>)> {
    stored(s, cmd)?.ok_or_else(|| bad(cmd, "the drawing has no cover record yet: run cover.init {model: the print set's truth.json or its folder}"))
}

pub fn stored_plan(s: &Session) -> CoverPlan {
    s.doc().ok().and_then(|d| d.embedded.get(PLAN_FILE)).and_then(|b| serde_json::from_slice(b).ok()).unwrap_or_default()
}

pub fn store(s: &mut Session, cmd: &str, rec: &CoverRecord) -> Result<()> {
    let text = serde_json::to_vec(rec).map_err(|e| bad(cmd, e.to_string()))?;
    s.doc_mut()?.embedded.insert(RECORD_FILE.into(), Arc::new(text));
    Ok(())
}

// ---- Shared helpers ----------------------------------------------------------------------------

/// Parameter `key` as text, trimmed, cut on a character boundary.
fn text(p: &Value, key: &str) -> String {
    clamp_text(str_param(p, key).map(str::trim).unwrap_or_default())
}

fn req(cmd: &str, p: &Value, key: &str) -> Result<String> {
    let t = text(p, key);
    if t.is_empty() { Err(bad(cmd, format!("give `{key}`"))) } else { Ok(t) }
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64().filter(|x| x.is_finite())
}

fn pt2(v: &Value) -> Option<[f64; 2]> {
    Some([num(v.get(0)?)?, num(v.get(1)?)?])
}

fn reach(p: &Value) -> f64 {
    p.get("max_distance_ft").and_then(num).filter(|d| *d > 0.0).map_or(PHOTO_REACH_FT, |d| d.min(100.0))
}

fn cleared_value(cleared: &[ClearedArea]) -> Value {
    serde_json::to_value(cleared).unwrap_or(Value::Null)
}

fn with_cleared(mut v: Value, cleared: &[ClearedArea]) -> Value {
    if !cleared.is_empty() {
        v["cleared"] = cleared_value(cleared);
    }
    v
}

fn kind_name(k: AreaKind) -> &'static str {
    match k {
        AreaKind::WallFace => "wall",
        AreaKind::Ceiling => "ceiling",
    }
}

fn compact(a: &CoverArea) -> Value {
    let (done, items) = a.checklist_progress();
    let mut v = json!({ "id": a.id, "kind": kind_name(a.kind), "level": a.level, "rooms": a.rooms, "done": done, "items": items, "photos": a.photos.len(), "ready": a.ready_to_cover });
    if !a.face.is_empty() {
        v["face"] = json!(a.face);
    }
    v
}

fn photo_value(ph: &InWallPhoto) -> Value {
    json!({ "id": ph.id, "uri": ph.uri, "caption": ph.caption, "author": ph.author, "date": ph.date, "elevation_ft": ph.elevation_ft, "along_ft": ph.along_ft })
}

fn face_value(a: &CoverArea) -> Value {
    let checklist: Vec<Value> = a
        .checklist
        .iter()
        .map(|c| {
            let mut v = json!({ "id": c.id, "category": c.category, "done": c.done });
            if let Some(by) = &c.done_by {
                v["by"] = json!(by);
            }
            v
        })
        .collect();
    let mut v = json!({ "area": a.id, "face": a.face, "rooms": a.rooms, "ready": a.ready_to_cover, "checklist": checklist, "photos": a.photos.iter().map(photo_value).collect::<Vec<_>>() });
    if let Some(r) = &a.fire_rating {
        v["fire_rating"] = json!(r);
    }
    v
}

fn area_req<'a>(cmd: &str, rec: &'a mut CoverRecord, id: &str) -> Result<&'a mut CoverArea> {
    let found = rec.areas.iter().position(|a| a.id.eq_ignore_ascii_case(id));
    match found.and_then(|i| rec.areas.get_mut(i)) {
        Some(a) => Ok(a),
        None => Err(bad(cmd, format!("no area `{id}` (wall faces are `<wall>/A` or `/B`, ceilings `CLG-<level>-<room>`; cover.status lists them)"))),
    }
}

/// A checklist item of `a` by id or category.
fn item_id(cmd: &str, a: &CoverArea, item: &str) -> Result<String> {
    a.checklist
        .iter()
        .find(|c| c.id.eq_ignore_ascii_case(item))
        .or_else(|| a.checklist.iter().find(|c| c.category.eq_ignore_ascii_case(item)))
        .map(|c| c.id.clone())
        .ok_or_else(|| {
            let have: Vec<&str> = a.checklist.iter().map(|c| c.category.as_str()).collect();
            bad(cmd, format!("{} has no item `{item}` (it has {})", a.id, have.join(", ")))
        })
}

// ---- cover.init ----------------------------------------------------------------------------------

fn run_init(s: &mut Session, p: &Value) -> Result<Value> {
    let model = req(INIT, p, "model")?;
    if s.doc()?.embedded.contains_key(RECORD_FILE) && !bool_or(p, "replace", false) {
        return Err(bad(
            INIT,
            "the drawing already has a cover record; give replace: true to start it again (its checklists, photos and signatures are lost)",
        ));
    }
    let file = if std::path::Path::new(&model).is_dir() { format!("{}/truth.json", model.trim_end_matches(['/', '\\'])) } else { model.clone() };
    let bytes =
        crate::readfile::read(&file).map_err(|e| bad(INIT, format!("{file}: {e} (give `model`: the print set's truth.json or its folder)")))?;
    let t: Truth = serde_json::from_slice(&bytes).map_err(|e| bad(INIT, format!("{file} is not a print set's truth.json: {e}")))?;
    let project = Some(text(p, "project")).filter(|x| !x.is_empty()).unwrap_or_else(|| "Project".into());
    let rec = record_from_truth(&t, &project);
    let plan = CoverPlan {
        walls: t.plan_walls.clone(),
        rooms: t.building.rooms.clone(),
        levels: t.building.levels.iter().map(|l| (cc::level_key(l), l.elevation_ft)).collect(),
    };
    let plan_text = serde_json::to_vec(&plan).map_err(|e| bad(INIT, e.to_string()))?;
    store(s, INIT, &rec)?;
    s.doc_mut()?.embedded.insert(PLAN_FILE.into(), Arc::new(plan_text));
    let faces = rec.areas.iter().filter(|a| a.kind == AreaKind::WallFace).count();
    Ok(json!({
        "areas": rec.areas.len(),
        "wall_faces": faces,
        "ceilings": rec.areas.len() - faces,
        "phases": rec.phases.len(),
        "levels": plan.levels.iter().map(|(n, _)| n).collect::<Vec<_>>(),
    }))
}

// ---- cover.status --------------------------------------------------------------------------------

fn run_status(s: &mut Session, p: &Value) -> Result<Value> {
    let Some((rec, cleared)) = stored(s, STATUS)? else {
        return Ok(
            json!({ "project": "", "levels": [], "total": 0, "offset": 0, "areas": [], "phases": [], "hint": "the drawing has no cover record yet: run cover.init {model: the print set's truth.json or its folder}" }),
        );
    };
    let level = text(p, "level");
    let kind = text(p, "kind").to_ascii_lowercase();
    let ready = p.get("ready").and_then(Value::as_bool);
    let offset = p.get("offset").and_then(Value::as_u64).map_or(0, |v| usize::try_from(v).unwrap_or(usize::MAX));
    let limit = p.get("limit").and_then(Value::as_u64).map_or(PAGE, |v| usize::try_from(v).unwrap_or(PAGE_MAX)).clamp(1, PAGE_MAX);
    let picked: Vec<&CoverArea> = rec
        .areas
        .iter()
        .filter(|a| level.is_empty() || a.level.eq_ignore_ascii_case(&level))
        .filter(|a| kind.is_empty() || kind_name(a.kind) == kind || (kind == "wall_face" && a.kind == AreaKind::WallFace))
        .filter(|a| ready.is_none_or(|r| a.ready_to_cover == r))
        .collect();
    let total = picked.len();
    let areas: Vec<Value> = picked.iter().skip(offset).take(limit).map(|a| compact(a)).collect();
    let levels: Vec<Value> = cc::summary_by_level(&rec)
        .into_iter()
        .filter(|l| level.is_empty() || l.level.eq_ignore_ascii_case(&level))
        .map(|l| {
            json!({ "level": l.level, "walls_total": l.walls_total, "walls_ready": l.walls_ready, "ceilings_total": l.ceilings_total, "ceilings_ready": l.ceilings_ready, "photos": l.photos, "phases_signed": l.phases_signed, "phases_total": l.phases_total })
        })
        .collect();
    let phases: Vec<Value> = rec
        .phases
        .iter()
        .filter(|ph| level.is_empty() || ph.levels.is_empty() || ph.levels.iter().any(|l| l.eq_ignore_ascii_case(&level)))
        .map(|ph| json!({ "id": ph.id, "name": ph.name, "level": ph.levels.join(", "), "signed": ph.signed }))
        .collect();
    let mut v = json!({ "project": rec.project, "levels": levels, "total": total, "offset": offset, "areas": areas, "phases": phases });
    if offset.saturating_add(limit) < total {
        v["next_offset"] = json!(offset + limit);
    }
    Ok(with_cleared(v, &cleared))
}

// ---- cover.check / cover.uncheck -----------------------------------------------------------------

fn run_check(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut rec, cleared) = need(s, CHECK)?;
    let id = req(CHECK, p, "area")?;
    let item = req(CHECK, p, "item")?;
    let (by, date, notes) = (text(p, "by"), text(p, "date"), text(p, "notes"));
    let a = area_req(CHECK, &mut rec, &id)?;
    let item_id = item_id(CHECK, a, &item)?;
    a.check_item(&item_id, &by, &date);
    if !notes.is_empty()
        && let Some(c) = a.checklist.iter_mut().find(|c| c.id == item_id)
    {
        c.notes = notes;
    }
    let (done, items) = a.checklist_progress();
    let v = json!({ "area": a.id, "item": item_id, "done": true, "items_done": done, "items": items, "ready": a.ready_to_cover });
    store(s, CHECK, &rec)?;
    Ok(with_cleared(v, &cleared))
}

fn run_uncheck(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut rec, cleared) = need(s, UNCHECK)?;
    let id = req(UNCHECK, p, "area")?;
    let item = req(UNCHECK, p, "item")?;
    let a = area_req(UNCHECK, &mut rec, &id)?;
    let item_id = item_id(UNCHECK, a, &item)?;
    let was_ready = a.ready_to_cover;
    a.uncheck_item(&item_id);
    let v = json!({ "area": a.id, "item": item_id, "was_ready": was_ready, "ready": false });
    store(s, UNCHECK, &rec)?;
    Ok(with_cleared(v, &cleared))
}

// ---- cover.sign ----------------------------------------------------------------------------------

fn run_sign(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut rec, cleared) = need(s, SIGN)?;
    let phase = req(SIGN, p, "phase")?;
    let level = req(SIGN, p, "level")?;
    let (inspector, date, permit, notes) = (text(p, "inspector"), text(p, "date"), text(p, "permit"), text(p, "notes"));
    if rec.sign_phase(&phase, &level, &inspector, &date) == 0 {
        let mut names: Vec<String> = Vec::new();
        for ph in &rec.phases {
            if !names.contains(&ph.name) {
                names.push(ph.name.clone());
            }
        }
        let mut levels: Vec<String> = rec.phases.iter().flat_map(|ph| ph.levels.iter().cloned()).collect();
        levels.sort();
        levels.dedup();
        return Err(bad(SIGN, format!("no phase `{phase}` on level `{level}` (phases: {}; levels: {})", names.join(", "), levels.join(", "))));
    }
    let want = phase.to_ascii_lowercase();
    let mut signed = Vec::new();
    for ph in &mut rec.phases {
        let base = ph.id.split('@').next().unwrap_or_default().to_ascii_lowercase();
        let named = ph.id.to_ascii_lowercase() == want || base == want || ph.name.to_ascii_lowercase() == want;
        if named && (ph.levels.is_empty() || ph.levels.contains(&level)) {
            if !permit.is_empty() {
                ph.permit_number = permit.clone();
            }
            if !notes.is_empty() {
                ph.notes = notes.clone();
            }
            signed.push(ph.label());
        }
    }
    store(s, SIGN, &rec)?;
    Ok(with_cleared(json!({ "signed": signed, "level": level }), &cleared))
}

// ---- cover.ready ---------------------------------------------------------------------------------

fn run_ready(s: &mut Session, p: &Value) -> Result<Value> {
    let (mut rec, cleared) = need(s, READY)?;
    let id = req(READY, p, "area")?;
    let (by, date) = (text(p, "by"), text(p, "date"));
    let phases = rec.phases.clone();
    let a = area_req(READY, &mut rec, &id)?;
    if a.mark_ready(&by, &date, &phases) {
        let v = json!({ "area": a.id, "ready": true });
        store(s, READY, &rec)?;
        return Ok(with_cleared(v, &cleared));
    }
    let (open_items, unsigned_phases) = a.open_reasons(&phases);
    let mut why = Vec::new();
    if !open_items.is_empty() {
        let cats: Vec<&str> = open_items.iter().filter_map(|i| i.split(": ").nth(1)).collect();
        why.push(format!("{} of {} items open: {}", open_items.len(), a.checklist.len(), cats.join(", ")));
    }
    if !unsigned_phases.is_empty() {
        why.push(format!("unsigned: {}", unsigned_phases.join(", ")));
    }
    if why.is_empty() {
        why.push("the area has no checklist".into());
    }
    let v = json!({ "area": a.id, "ready": false, "open_items": open_items, "unsigned_phases": unsigned_phases, "reason": format!("not ready to cover: {}", why.join("; ")) });
    if !cleared.is_empty() {
        store(s, READY, &rec)?;
    }
    Ok(with_cleared(v, &cleared))
}

// ---- cover.wall ----------------------------------------------------------------------------------

fn run_wall(s: &mut Session, p: &Value) -> Result<Value> {
    let (rec, cleared) = need(s, WALL)?;
    let plan = stored_plan(s);
    let (wall, tapped) = if let Some(at) = p.get("at").and_then(pt2) {
        let level = req(WALL, p, "level")?;
        let d = reach(p);
        let (area, _) = resolve_wall_tap(&rec, &plan.walls, at, &level, d)
            .ok_or_else(|| bad(WALL, format!("no wall within {d} ft of [{}, {}] on level {level}", at[0], at[1])))?;
        let wall = rec.area(&area).map(|a| a.element_id.clone()).unwrap_or_default();
        (wall, Some(area))
    } else if p.get("at").is_some() {
        return Err(bad(WALL, "`at` must be [x, y] in building feet"));
    } else {
        let id = req(WALL, p, "wall")?;
        let wall = rec
            .areas
            .iter()
            .find(|a| a.kind == AreaKind::WallFace && a.element_id.eq_ignore_ascii_case(&id))
            .map(|a| a.element_id.clone())
            .ok_or_else(|| bad(WALL, format!("no wall `{id}` in the cover record (or give at: [x, y] and level)")))?;
        (wall, None)
    };
    let faces = rec.wall_areas(&wall);
    let level = faces.first().map(|a| a.level.clone()).unwrap_or_default();
    let mut v = json!({ "wall": wall, "level": level, "faces": faces.iter().map(|a| face_value(a)).collect::<Vec<_>>() });
    if let Some(t) = &tapped {
        v["tapped"] = json!(t);
        let photos: Vec<Value> = rec.area(t).map(|a| a.photos.iter().map(photo_value).collect()).unwrap_or_default();
        v["photos"] = json!(photos);
    }
    Ok(with_cleared(v, &cleared))
}

// ---- cover.photo ---------------------------------------------------------------------------------

/// The next free photo id (`PH-0007`).
fn next_photo_id(rec: &CoverRecord) -> String {
    let n = rec
        .areas
        .iter()
        .flat_map(|a| a.photos.iter())
        .filter_map(|ph| ph.id.strip_prefix("PH-").and_then(|n| n.parse::<usize>().ok()))
        .max()
        .unwrap_or(0);
    format!("PH-{:04}", n.saturating_add(1))
}

fn run_photo(s: &mut Session, p: &Value) -> Result<Value> {
    let action = text(p, "action");
    if !action.is_empty() && !action.eq_ignore_ascii_case("add") {
        return Err(bad(PHOTO, format!("cover.photo has no action `{action}` (add)")));
    }
    let (mut rec, cleared) = need(s, PHOTO)?;
    let plan = stored_plan(s);
    let at = p.get("at").and_then(pt2).ok_or_else(|| bad(PHOTO, "give `at`: [x, y] in building feet, where the photo was taken"))?;
    let level = req(PHOTO, p, "level")?;
    let uri = req(PHOTO, p, "uri")?;
    let id = Some(text(p, "id")).filter(|x| !x.is_empty()).unwrap_or_else(|| next_photo_id(&rec));
    let elevation_ft = match p.get("elevation_ft") {
        None | Some(Value::Null) => 4.0,
        Some(v) => num(v).ok_or_else(|| bad(PHOTO, "`elevation_ft` must be a finite number"))?,
    };
    let photo = InWallPhoto {
        id: id.clone(),
        face: text(p, "face"),
        elevation_ft,
        along_ft: 0.0,
        uri,
        author: text(p, "author"),
        date: text(p, "date"),
        caption: text(p, "caption"),
        at_ft: at,
        level,
    };
    let area = add_photo_at(&mut rec, &plan.walls, photo, reach(p)).map_err(|e| bad(PHOTO, e))?;
    let (face, along) =
        rec.area(&area).and_then(|a| a.photos.iter().find(|ph| ph.id == id)).map(|ph| (ph.face.clone(), ph.along_ft)).unwrap_or_default();
    store(s, PHOTO, &rec)?;
    Ok(with_cleared(json!({ "id": id, "area": area, "face": face, "along_ft": (along * 1000.0).round() / 1000.0 }), &cleared))
}

// ---- The field viewer ----------------------------------------------------------------------------

/// A field photo note filed on the cover record: `Ok(area)`, `Err(reason)`. Already filed (the
/// same id on any area): `Ok` with that area, nothing added.
pub fn file_field_photo(rec: &mut CoverRecord, plan: &CoverPlan, n: &cadaddy_walk3d::field::FieldNote) -> std::result::Result<String, String> {
    if let Some(a) = rec.areas.iter().find(|a| a.photos.iter().any(|ph| ph.id == n.id)) {
        return Ok(a.id.clone());
    }
    let floor = plan.elevation(&n.level).unwrap_or(0.0);
    let uri = if n.photo.trim().is_empty() { n.id.clone() } else { n.photo.clone() };
    let photo = InWallPhoto {
        id: n.id.clone(),
        face: String::new(),
        elevation_ft: n.at[2] - floor,
        along_ft: 0.0,
        uri,
        author: n.author.clone(),
        date: n.when.clone(),
        caption: n.text.clone(),
        at_ft: [n.at[0], n.at[1]],
        level: n.level.clone(),
    };
    add_photo_at(rec, &plan.walls, photo, PHOTO_REACH_FT)
}

/// The record's wall faces for the field package: where each wall runs, its checklist, photos.
pub fn field_cover(rec: &CoverRecord, plan: &CoverPlan) -> Vec<cadaddy_walk3d::field::FieldCover> {
    use cadaddy_walk3d::field::{FieldCheck, FieldCover, FieldPhoto, MAX_COVER, MAX_COVER_ITEMS, MAX_COVER_PHOTOS};
    rec.areas
        .iter()
        .filter(|a| a.kind == AreaKind::WallFace)
        .filter_map(|a| {
            let w = plan.walls.iter().find(|w| w.id == a.element_id && w.level == a.level)?;
            Some(FieldCover {
                area: a.id.clone(),
                wall: a.element_id.clone(),
                face: a.face.clone(),
                level: a.level.clone(),
                a: w.a_ft,
                b: w.b_ft,
                rooms: a.rooms.clone(),
                fire_rating: a.fire_rating.clone().unwrap_or_default(),
                ready: a.ready_to_cover,
                checklist: a
                    .checklist
                    .iter()
                    .take(MAX_COVER_ITEMS)
                    .map(|c| FieldCheck { id: c.id.clone(), category: c.category.clone(), description: c.description.clone(), done: c.done })
                    .collect(),
                photos: a
                    .photos
                    .iter()
                    .take(MAX_COVER_PHOTOS)
                    .map(|ph| FieldPhoto {
                        id: ph.id.clone(),
                        uri: ph.uri.clone(),
                        caption: ph.caption.clone(),
                        author: ph.author.clone(),
                        date: ph.date.clone(),
                        elevation_ft: ph.elevation_ft,
                        along_ft: ph.along_ft,
                    })
                    .collect(),
            })
        })
        .take(MAX_COVER)
        .collect()
}
