//! Tags are identities (`bim.identity`, `rooms.graph`): one element per real thing, however many
//! sheets of the set draw it.
//!
//! A door number, a room number, a wall-type tag at a wall's centreline, a device's type and
//! circuit at its spot each name one thing. The model is put together from every sheet of a
//! print set, side by side in one drawing, so the same door can be read off the architectural
//! plan and off each discipline's background; `bim.identity`:
//! 1. puts every plan sheet the building spec knows onto its level's reference plan (the
//!    architectural plan with the most door and room numbers) by the numbers both print, else by
//!    the sheet frame when the sheets are drawn alike ([`cadaddy_mepfix::tagid::register_sheets`]);
//! 2. gives each door without a number the door number lettered beside it on its sheet;
//! 3. merges elements of one kind on one level with the same key whose places agree in project
//!    coordinates (doors within 6", devices within 12", plus the registration's error): the best
//!    source is kept (schedule/spec > architectural plan > discipline background > default),
//!    takes the others' values it lacks (a person's edits always win) and lists every sheet it
//!    was seen on (`seen_on`); the others are removed. The same key far apart is a finding;
//! 4. checks the doors against the door schedule and the rooms against the finish schedule.
//!
//! `rooms.graph` puts each door between the rooms on its two sides (`from_room`, `to_room`) and
//! lists the rooms' connections.

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::tagid::{self, IdFinding, IdItem, SheetTags, SheetTransform};
use cadaddy_walk3d::BuildingSpec;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const IDENTITY: &str = "bim.identity";
const GRAPH: &str = "rooms.graph";

/// Most texts looked at for door and room numbers.
const MAX_TEXTS: usize = 2_000_000;
/// Most findings and merges listed in an answer.
const MAX_LISTED: usize = 200;
/// How far a door's number may be lettered from it, feet.
const DOOR_TAG_REACH_FT: f64 = 3.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(IDENTITY, "Element Identities (BIM)", run_identity)
            .params("{apply?: bool (default true: merge and write; false: report only), room_numbers?: [string] (the job's rooms, for the finish-schedule check; default the model's rooms)} → {sheets: [{sheet, level, reference, registered, method: reference|tags|frame, transform: {a, b, tx, ty}, matched, residual?}], elements: {kind: n}, numbered_doors, merged: n, by_kind: {kind: {before, after}}, merges: [{kind, level, key, kept, removed: [handle], sheets}], findings: [{kind: same_tag_apart|schedule_not_placed|not_in_schedule|doors_vs_schedule|room_not_on_plan, ...}]}"),
        CommandSpec::new(GRAPH, "Room Connections", run_graph)
            .params("{set?: bool (default true: write from_room/to_room on the doors)} → {rooms: [{number?, name, level?}], doors: [{handle, number?, level?, from_room?, to_room?}], edges: [{door, from, to}], connected: n}"),
    ]
}

// ─── What the model holds ───────────────────────────────────────────────────

fn units_per_foot(sp: Option<&BuildingSpec>) -> f64 {
    sp.map(|s| s.units_per_foot).filter(|u| u.is_finite() && *u > 0.0).unwrap_or(12.0)
}

fn spec_of(d: &Drawing) -> Option<BuildingSpec> {
    d.building_spec.as_deref().and_then(|t| BuildingSpec::from_json(t).ok())
}

/// A plan sheet of the set: number, discipline, level and rectangle.
pub(crate) struct SheetBox {
    pub number: String,
    pub discipline: String,
    pub level: String,
    pub rect: [f64; 4],
}

impl SheetBox {
    pub fn new(number: &str, discipline: &str, level: &str, r: [f64; 4]) -> Self {
        SheetBox {
            number: number.to_string(),
            discipline: if discipline.trim().is_empty() { cadaddy_mepfix::sheet_facts::discipline_of(number) } else { discipline.to_string() },
            level: level.to_string(),
            rect: [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])],
        }
    }
}

pub(crate) fn sheets_of(sp: Option<&BuildingSpec>) -> Vec<SheetBox> {
    let Some(sp) = sp else { return Vec::new() };
    sp.sheets
        .iter()
        .filter_map(|s| {
            let r = s.rect.filter(|r| r.iter().all(|v| v.is_finite()))?;
            Some(SheetBox::new(&s.number, s.discipline.as_deref().unwrap_or(""), s.level.as_deref().unwrap_or(""), r))
        })
        .collect()
}

fn inside(r: &[f64; 4], p: [f64; 2]) -> bool {
    p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3]
}

/// The sheet `p` lies on (the smallest rectangle holding it).
fn sheet_at(sheets: &[SheetBox], p: [f64; 2]) -> Option<usize> {
    sheets
        .iter()
        .enumerate()
        .filter(|(_, s)| inside(&s.rect, p))
        .min_by(|a, b| {
            ((a.1.rect[2] - a.1.rect[0]) * (a.1.rect[3] - a.1.rect[1])).total_cmp(&((b.1.rect[2] - b.1.rect[0]) * (b.1.rect[3] - b.1.rect[1])))
        })
        .map(|(i, _)| i)
}

/// Every door or room number lettered in model space and where (the lettering's middle).
pub(crate) fn number_texts(d: &Drawing) -> Vec<(String, [f64; 2])> {
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_TEXTS) {
        if super::wallgen::made_as(e).is_some() {
            continue;
        }
        let (value, at, h) = match &e.kind {
            EntityKind::Text(t) => (t.value.as_str(), t.insert.xy(), t.height),
            EntityKind::MText(t) => (t.contents.as_str(), t.insert.xy(), t.height),
            _ => continue,
        };
        let Some(k) = tagid::tag_key(value) else { continue };
        if !at.is_finite() || !h.is_finite() {
            continue;
        }
        let w = 0.6 * h * value.trim().chars().count() as f64;
        out.push((k, [at.x + w / 2.0, at.y + h / 2.0]));
    }
    out
}

/// A model element the merge looks at.
struct Element {
    /// A model object, or a building-spec element tag (devices) by its id.
    handle: Option<Handle>,
    tag_id: Option<String>,
    kind: String,
    level: String,
    /// Its own identity value (door/room number, wall type, device type + circuit).
    number: Option<String>,
    sort: String,
    /// Drawing coordinates.
    at: [f64; 2],
    sheet: Option<usize>,
}

fn centre(d: &Drawing, e: &Entity) -> Option<[f64; 2]> {
    let b = cadaddy_doc::entity_bounds(d, e, 0);
    if b.is_empty() {
        return None;
    }
    let c = [(b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0];
    c.iter().all(|v| v.is_finite()).then_some(c)
}

fn prop_text(store: &[(String, Prop, bool)], key: &str) -> Option<String> {
    store.iter().find(|(k, p, _)| k.eq_ignore_ascii_case(key) && p.has_value()).map(|(_, p, _)| p.text().trim().to_string()).filter(|t| !t.is_empty())
}

/// The model's doors, windows, walls and rooms (objects) and its devices (spec tags).
fn elements(d: &Drawing, sp: Option<&BuildingSpec>, sheets: &[SheetBox]) -> Vec<Element> {
    let upf = units_per_foot(sp);
    let mut out = Vec::new();
    for e in d.model.iter() {
        let Some(kind) = super::elemprops::element_kind(e) else { continue };
        if !matches!(kind.as_str(), "door" | "window" | "wall" | "room") {
            continue;
        }
        let Some(at) = centre(d, e) else { continue };
        let store = super::elemprops::effective(e, &kind, upf);
        let level = prop_text(&store, "level").unwrap_or_default();
        let number = match kind.as_str() {
            // A wall's identity: its type tag and its run (one type at one centreline is one
            // wall only when it is as long). Untyped walls are never merged.
            "wall" => prop_text(&store, "wall_type").or_else(|| prop_text(&store, "type")).map(|t| {
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                format!("{t} {:.0}'", (b.max.x - b.min.x).max(b.max.y - b.min.y) / upf)
            }),
            _ => prop_text(&store, "number").and_then(|n| tagid::tag_key(&n).or(Some(n.to_ascii_uppercase()))),
        };
        out.push(Element { handle: Some(e.handle), tag_id: None, kind, level, number, sort: String::new(), at, sheet: sheet_at(sheets, at) });
    }
    if let Some(sp) = sp {
        for t in sp.tags.iter().filter(|t| t.id.starts_with("device:")) {
            let at = [t.at_ft[0] * upf, t.at_ft[1] * upf];
            if !at.iter().all(|v| v.is_finite()) {
                continue;
            }
            let field = |k: &str| t.fields.get(k).map(|f| f.value.trim().to_string()).filter(|v| !v.is_empty());
            let ty = field("type").unwrap_or_else(|| t.kind.clone());
            // A device's identity: its type and circuit (a circuit names one run of devices, so
            // the type and the spot tell them apart), in its room when it has one: two rooms'
            // devices are two however near (back to back across the wall two guest rooms share,
            // their panels' circuits named alike).
            let room = field("room");
            let in_room = |v: String| match &room {
                Some(r) => format!("{v} @{r}"),
                None => v,
            };
            let number = field("circuit").map(|c| in_room(format!("{ty} {c}")));
            let ty = in_room(ty);
            let sheet = field("sheet").and_then(|n| sheets.iter().position(|s| s.number.eq_ignore_ascii_case(&n))).or_else(|| sheet_at(sheets, at));
            out.push(Element {
                handle: None,
                tag_id: Some(t.id.clone()),
                kind: "device".into(),
                level: t.level.clone(),
                number,
                sort: ty,
                at,
                sheet,
            });
        }
    }
    out
}

/// A source's priority for an element of `kind` read off a sheet of `discipline`: the trade
/// that draws it first (architectural for doors, rooms and walls; the MEP trades for devices).
fn rank(kind: &str, discipline: &str, has_schedule_value: bool) -> u8 {
    if has_schedule_value {
        return 0;
    }
    let a = discipline.eq_ignore_ascii_case("A") || discipline.eq_ignore_ascii_case("ID");
    match kind {
        "device" | "equipment" | "fixture" | "light_fixture" if !a && !discipline.is_empty() => 1,
        "door" | "window" | "wall" | "room" if a => 1,
        _ if discipline.is_empty() => 3,
        _ => 2,
    }
}

// ─── bim.identity ───────────────────────────────────────────────────────────

/// What `bim.identity` did, for the guided job.
pub(crate) struct IdentityRun {
    pub answer: Value,
    /// Elements removed by kind.
    pub removed: BTreeMap<String, usize>,
}

fn run_identity(s: &mut Session, p: &Value) -> Result<Value> {
    let apply = p.get("apply").and_then(Value::as_bool).unwrap_or(true);
    let rooms: Option<Vec<String>> = match p.get("room_numbers") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => Some(a.iter().filter_map(Value::as_str).map(str::to_string).take(100_000).collect()),
        Some(_) => return Err(bad(IDENTITY, "`room_numbers` must be a list of room numbers")),
    };
    let r = identity(s, apply, rooms.as_deref())?;
    if apply && r.removed.values().sum::<usize>() > 0 {
        s.touch();
    }
    Ok(r.answer)
}

/// The sheets' door and room numbers (each sheet's, and every one in the drawing) and every
/// plan sheet put onto its level's reference plan.
pub(crate) struct Registration {
    pub texts: Vec<(String, [f64; 2])>,
    pub on_sheet: Vec<Vec<(String, [f64; 2])>>,
    pub transforms: Vec<SheetTransform>,
}

/// Register `sheets` (see the module docs): `upf` drawing units per foot.
pub(crate) fn register(d: &Drawing, sheets: &[SheetBox], upf: f64) -> Registration {
    let texts = number_texts(d);
    // Each sheet's numbers, for the registration and the doors' numbers.
    let mut on_sheet: Vec<Vec<(String, [f64; 2])>> = sheets.iter().map(|_| Vec::new()).collect();
    for (k, at) in &texts {
        if let Some(i) = sheet_at(sheets, *at)
            && let Some(v) = on_sheet.get_mut(i)
        {
            v.push((k.clone(), *at));
        }
    }
    let tags: Vec<SheetTags> = sheets
        .iter()
        .zip(&on_sheet)
        .filter(|(s, _)| !s.level.trim().is_empty())
        .map(|(s, t)| SheetTags { number: s.number.clone(), level: s.level.clone(), discipline: s.discipline.clone(), tags: t.clone(), rect: s.rect })
        .collect();
    // A tag sits within a couple of feet of its copy on another discipline's background.
    let transforms = tagid::register_sheets(&tags, 2.0 * upf);
    Registration { texts, on_sheet, transforms }
}

/// The devices of `devices` that are one thing (same level, type and circuit, or same type at
/// one spot with no circuit, within 12" plus the registration's error once their sheets are put
/// onto their level's plan): the indices to leave out (the copies; the best sheet's is kept).
pub(crate) fn duplicate_devices(d: &Drawing, sheets: &[SheetBox], upf: f64, devices: &[super::readers::ReadDevice]) -> (BTreeSet<usize>, usize) {
    if sheets.len() < 2 || devices.len() < 2 {
        return (BTreeSet::new(), 0);
    }
    let reg = register(d, sheets, upf);
    let items: Vec<IdItem> = devices
        .iter()
        .map(|dv| {
            let sheet = sheets.iter().position(|s| s.number.eq_ignore_ascii_case(&dv.sheet)).or_else(|| sheet_at(sheets, dv.at));
            let tr = sheet.and_then(|i| sheets.get(i)).and_then(|s| reg.transforms.iter().find(|t| t.sheet == s.number));
            let discipline = sheet.and_then(|i| sheets.get(i)).map_or("", |s| s.discipline.as_str());
            IdItem {
                kind: "device".into(),
                level: dv.level.clone(),
                key: if dv.circuit.trim().is_empty() { String::new() } else { format!("{} {}", dv.tag.trim(), dv.circuit.trim()) },
                sort: dv.tag.trim().to_string(),
                at: tr.and_then(|t| t.place(dv.at)),
                sheet: sheet.and_then(|i| sheets.get(i)).map_or_else(String::new, |s| s.number.clone()),
                rank: rank("device", discipline, false),
            }
        })
        .collect();
    let slack = reg.transforms.iter().filter_map(|t| t.residual).fold(0.0, f64::max).min(upf);
    let (groups, _) = tagid::merge_identities(&items, &|_| upf + slack);
    let drop: BTreeSet<usize> = groups.iter().flat_map(|g| g.merged.iter().copied()).collect();
    let registered = reg.transforms.iter().filter(|t| t.registered && t.method != "reference").count();
    (drop, registered)
}

/// Run the identity pass on the open drawing (see the module docs); `room_numbers`: the rooms
/// the finish schedule is checked against (default the model's rooms).
pub(crate) fn identity(s: &mut Session, apply: bool, room_numbers: Option<&[String]>) -> Result<IdentityRun> {
    let d = s.doc()?;
    let sp = spec_of(d);
    let upf = units_per_foot(sp.as_ref());
    let sheets = sheets_of(sp.as_ref());
    let Registration { texts, on_sheet, transforms } = register(d, &sheets, upf);
    let transform_of = |i: Option<usize>| -> Option<&SheetTransform> {
        let n = &sheets.get(i?)?.number;
        transforms.iter().find(|t| &t.sheet == n)
    };

    let mut els = elements(d, sp.as_ref(), &sheets);
    // Doors with no number: the door number lettered beside it on its own sheet.
    let reach = DOOR_TAG_REACH_FT * upf;
    let mut numbered = Vec::new();
    for (i, el) in els.iter_mut().enumerate() {
        if el.kind != "door" || el.number.is_some() {
            continue;
        }
        let pool: &[(String, [f64; 2])] = match el.sheet.and_then(|k| on_sheet.get(k)) {
            Some(v) => v,
            None if sheets.is_empty() => &texts,
            None => &[],
        };
        let near = pool
            .iter()
            .map(|(k, at)| (k, *at, (at[0] - el.at[0]).hypot(at[1] - el.at[1])))
            .filter(|x| x.2 <= reach)
            .min_by(|a, b| a.2.total_cmp(&b.2).then_with(|| a.0.cmp(b.0)));
        if let Some((k, at, _)) = near {
            el.number = Some(k.clone());
            numbered.push((i, k.clone(), at));
        }
    }
    let sheet_name = |i: Option<usize>| i.and_then(|k| sheets.get(k)).map_or_else(String::new, |s| s.number.clone());
    let items: Vec<IdItem> = els
        .iter()
        .map(|el| {
            let tr = transform_of(el.sheet);
            let at = match tr {
                Some(t) => t.place(el.at),
                // Not on a registered sheet: compared in its own drawing coordinates only with
                // elements on no sheet.
                None if el.sheet.is_none() => Some(el.at),
                None => None,
            };
            let discipline = el.sheet.and_then(|k| sheets.get(k)).map_or("", |s| s.discipline.as_str());
            IdItem {
                kind: el.kind.clone(),
                level: el.level.clone(),
                key: el.number.clone().unwrap_or_default(),
                sort: el.sort.clone(),
                at,
                sheet: sheet_name(el.sheet),
                rank: rank(&el.kind, discipline, false),
            }
        })
        .collect();
    // Tolerances (MUST: doors 6", devices 12"), plus the worst registration error of the set.
    let slack = transforms.iter().filter_map(|t| t.residual).fold(0.0, f64::max).min(upf);
    let tol = |kind: &str| {
        let inches = match kind {
            "door" | "window" => 6.0,
            "wall" => 6.0,
            "room" => 24.0,
            _ => 12.0,
        };
        inches / 12.0 * upf + slack
    };
    let (groups, apart) = tagid::merge_identities(&items, &tol);

    // Findings: the same number in two places on one level, and the schedules.
    let mut findings: Vec<Value> = apart.iter().take(MAX_LISTED).map(finding_apart).collect();
    let door_rows = cadaddy_mepfix::arch_reader::find_door_schedule(d);
    let removed_idx: BTreeSet<usize> = groups.iter().flat_map(|g| g.merged.iter().copied()).collect();
    let doors_after: Vec<&Element> = els.iter().enumerate().filter(|(i, e)| e.kind == "door" && !removed_idx.contains(i)).map(|(_, e)| e).collect();
    findings.extend(schedule_findings(&door_rows, &doors_after));
    let finish = cadaddy_mepfix::arch_reader::find_finish_schedule(d);
    if !finish.is_empty() {
        let plan_rooms: BTreeSet<String> = match room_numbers {
            Some(list) => list.iter().filter_map(|n| tagid::tag_key(n).or_else(|| Some(n.trim().to_ascii_uppercase()))).collect(),
            None => els.iter().filter(|e| e.kind == "room").filter_map(|e| e.number.clone()).collect(),
        };
        let missing: Vec<String> = finish
            .iter()
            .filter_map(|r| tagid::tag_key(&r.number).or_else(|| Some(r.number.trim().to_ascii_uppercase())).filter(|n| !n.is_empty()))
            .filter(|n| !plan_rooms.contains(n))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if !missing.is_empty() {
            findings.push(json!({
                "kind": "room_not_on_plan",
                "message": format!("{} room(s) in the finish schedule have no room on the plan", missing.len()),
                "count": missing.len(),
                "rooms": missing.iter().take(MAX_LISTED).collect::<Vec<_>>(),
            }));
        }
    }

    // Counts before and after, by kind.
    let mut by_kind: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (i, el) in els.iter().enumerate() {
        let c = by_kind.entry(el.kind.clone()).or_default();
        c.0 += 1;
        if !removed_idx.contains(&i) {
            c.1 += 1;
        }
    }
    let label =
        |i: usize| -> String { els.get(i).map_or_else(String::new, |e| e.handle.map(|h| h.hex()).or_else(|| e.tag_id.clone()).unwrap_or_default()) };
    let merges: Vec<Value> = groups
        .iter()
        .take(MAX_LISTED)
        .map(|g| {
            json!({
                "kind": g.kind, "level": g.level, "key": g.key, "kept": label(g.keep),
                "removed": g.merged.iter().map(|i| label(*i)).collect::<Vec<_>>(), "sheets": g.sheets,
            })
        })
        .collect();
    let mut removed: BTreeMap<String, usize> = BTreeMap::new();
    if apply {
        write_registrations(s, &transforms)?;
        write_identities(s, &els, &groups, &numbered, &sheets)?;
        for g in &groups {
            *removed.entry(g.kind.clone()).or_default() += g.merged.len();
        }
    }
    let answer = json!({
        "applied": apply,
        "sheets": transforms,
        "elements": by_kind.iter().map(|(k, v)| (k.clone(), json!(v.0))).collect::<serde_json::Map<_, _>>(),
        "numbered_doors": numbered.len(),
        "merged": groups.iter().map(|g| g.merged.len()).sum::<usize>(),
        "by_kind": by_kind.iter().map(|(k, v)| (k.clone(), json!({"before": v.0, "after": v.1}))).collect::<serde_json::Map<_, _>>(),
        "merges": merges,
        "findings": findings,
    });
    Ok(IdentityRun { answer, removed })
}

fn finding_apart(f: &IdFinding) -> Value {
    json!({
        "kind": "same_tag_apart",
        "message": format!("{} {} is drawn in {} places on level {}", f.kind, f.key, f.places.len(), if f.level.is_empty() { "?" } else { &f.level }),
        "element": f.kind, "key": f.key, "level": f.level,
        "places": f.places.iter().map(|(at, sh)| json!({"at": [round2(at[0]), round2(at[1])], "sheet": sh})).collect::<Vec<_>>(),
    })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// The level a door mark belongs to by its leading digit (`204` → `2`), when the level's name
/// carries one.
fn mark_level(mark: &str) -> Option<char> {
    mark.chars().find(char::is_ascii_digit)
}

fn level_digit(level: &str) -> Option<char> {
    let up = level.to_ascii_uppercase();
    if let Some(c) = up.chars().find(char::is_ascii_digit) {
        return Some(c);
    }
    [("FIRST", '1'), ("SECOND", '2'), ("THIRD", '3'), ("FOURTH", '4'), ("FIFTH", '5'), ("SIXTH", '6')]
        .iter()
        .find(|(w, _)| up.contains(w))
        .map(|(_, c)| *c)
}

/// The door schedule against the doors placed: marks never placed, doors the schedule doesn't
/// list, and the count per level.
fn schedule_findings(rows: &[cadaddy_mepfix::arch_reader::DoorRow], doors: &[&Element]) -> Vec<Value> {
    let mut out = Vec::new();
    if rows.is_empty() {
        return out;
    }
    let marks: BTreeSet<String> =
        rows.iter().filter_map(|r| tagid::tag_key(&r.mark).or_else(|| Some(r.mark.trim().to_ascii_uppercase()))).filter(|m| !m.is_empty()).collect();
    let placed: BTreeSet<String> = doors.iter().filter_map(|d| d.number.clone()).collect();
    let never: Vec<&String> = marks.iter().filter(|m| !placed.contains(*m)).collect();
    if !never.is_empty() {
        out.push(json!({
            "kind": "schedule_not_placed",
            "message": format!("{} door(s) in the door schedule are on no plan", never.len()),
            "count": never.len(), "doors": never.iter().take(MAX_LISTED).collect::<Vec<_>>(),
        }));
    }
    let unlisted: Vec<&String> = placed.iter().filter(|m| !marks.contains(*m)).collect();
    if !unlisted.is_empty() {
        out.push(json!({
            "kind": "not_in_schedule",
            "message": format!("{} door number(s) on the plans are not in the door schedule", unlisted.len()),
            "count": unlisted.len(), "doors": unlisted.iter().take(MAX_LISTED).collect::<Vec<_>>(),
        }));
    }
    // Per level: the doors placed against the schedule's marks of that level.
    let mut levels: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for d in doors {
        levels.entry(d.level.clone()).or_default().0 += 1;
    }
    for (level, c) in levels.iter_mut() {
        if let Some(digit) = level_digit(level) {
            c.1 = marks.iter().filter(|m| mark_level(m) == Some(digit)).count();
        }
    }
    for (level, (placed, scheduled)) in levels {
        if scheduled == 0 {
            continue;
        }
        let off = placed.abs_diff(scheduled);
        if off > 2 && off * 10 > scheduled {
            out.push(json!({
                "kind": "doors_vs_schedule",
                "message": format!("level {level}: {placed} doors placed, the door schedule lists {scheduled}"),
                "level": level, "placed": placed, "scheduled": scheduled,
            }));
        }
    }
    out
}

/// Keep each registered sheet's transform on its sheet in the building spec.
fn write_registrations(s: &mut Session, transforms: &[SheetTransform]) -> Result<()> {
    if transforms.is_empty() {
        return Ok(());
    }
    let d = s.doc_mut()?;
    let Some(mut sp) = spec_of(d) else { return Ok(()) };
    for sh in sp.sheets.iter_mut() {
        sh.registration = transforms.iter().find(|t| t.registered && t.sheet == sh.number).map(|t| cadaddy_walk3d::levels::SheetRegistration {
            reference: t.reference.clone(),
            method: t.method.clone(),
            a: t.transform.a,
            b: t.transform.b,
            tx: t.transform.tx,
            ty: t.transform.ty,
            matched: t.matched,
            residual: t.residual,
        });
    }
    d.building_spec = Some(sp.to_json());
    Ok(())
}

/// Write the merges: the kept element takes the others' values it lacks and lists its sheets;
/// the others go. Doors that were given a number keep it (source print).
fn write_identities(
    s: &mut Session,
    els: &[Element],
    groups: &[tagid::IdGroup],
    numbered: &[(usize, String, [f64; 2])],
    sheets: &[SheetBox],
) -> Result<()> {
    let sheet_of = |i: usize| els.get(i).and_then(|e| e.sheet).and_then(|k| sheets.get(k)).map_or_else(String::new, |s| s.number.clone());
    // Per object: the values to write.
    let mut writes: BTreeMap<Handle, Vec<(String, Prop, bool)>> = BTreeMap::new();
    for (i, k, at) in numbered {
        if let Some(h) = els.get(*i).and_then(|e| e.handle) {
            writes.entry(h).or_default().push(("number".into(), Prop::new(k.clone(), Source::print(&sheet_of(*i), Some(*at))), false));
        }
    }
    let d = s.doc()?;
    let mut drop_objects: Vec<Handle> = Vec::new();
    let mut drop_tags: BTreeSet<String> = BTreeSet::new();
    let mut tag_sheets: BTreeMap<String, String> = BTreeMap::new();
    for g in groups {
        let Some(keep) = els.get(g.keep) else { continue };
        let seen = g.sheets.iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(", ");
        match (keep.handle, &keep.tag_id) {
            (Some(h), _) => {
                let w = writes.entry(h).or_default();
                if !seen.is_empty() {
                    w.push(("seen_on".into(), Prop::new(seen.clone(), Source::computed()), true));
                }
                if !g.key.is_empty() {
                    w.push(("identity".into(), Prop::new(format!("{} {}", g.kind, g.key), Source::computed()), true));
                }
                // The others' stored values: the best source first (the merge's order).
                for m in &g.merged {
                    let Some(other) = els.get(*m).and_then(|e| e.handle).and_then(|oh| d.model.get(oh)) else { continue };
                    for (key, prop) in props::read(&other.common.xdata).iter() {
                        w.push((key.to_string(), prop.clone(), false));
                    }
                }
            }
            (None, Some(id)) if !seen.is_empty() => {
                tag_sheets.insert(id.clone(), seen);
            }
            _ => {}
        }
        for m in &g.merged {
            match els.get(*m).map(|e| (e.handle, e.tag_id.clone())) {
                Some((Some(h), _)) => drop_objects.push(h),
                Some((None, Some(id))) => {
                    drop_tags.insert(id);
                }
                _ => {}
            }
        }
    }
    let d = s.doc_mut()?;
    for (h, list) in writes {
        let _ = d.modify_entity(h, |e| {
            let mut store: Props = props::read(&e.common.xdata);
            for (key, prop, replace) in &list {
                match store.get(key) {
                    // A person's edit always wins; a computed identity value is refreshed.
                    Some(old) if old.source.kind == SourceKind::User => {}
                    Some(_) if !*replace => {
                        // A merged copy's own edit beats the kept one's read value.
                        if prop.source.kind == SourceKind::User {
                            store.set(key, prop.clone());
                        }
                    }
                    _ => {
                        store.set(key, prop.clone());
                    }
                }
            }
            e.common.xdata = props::write(&e.common.xdata, &store).into();
        });
    }
    for h in drop_objects {
        d.model.remove(h);
    }
    if (!drop_tags.is_empty() || !tag_sheets.is_empty())
        && let Some(mut sp) = spec_of(d)
    {
        sp.tags.retain(|t| !drop_tags.contains(&t.id));
        for t in sp.tags.iter_mut() {
            if let Some(seen) = tag_sheets.get(&t.id) {
                t.set("seen_on", seen.clone(), "plan");
            }
        }
        d.building_spec = Some(sp.to_json());
    }
    Ok(())
}

// ─── rooms.graph ────────────────────────────────────────────────────────────

/// A door's two sides: for each face of its wall (its plan box's short axis), points stepping
/// away from it, nearest first (6", 18", then just past the swing: a room traced round a drawn
/// swing leaves the swing's quarter circle out).
fn door_sides(d: &Drawing, e: &Entity, upf: f64) -> Option<[Vec<Vec2>; 2]> {
    let b = cadaddy_doc::entity_bounds(d, e, 0);
    if b.is_empty() {
        return None;
    }
    let (w, h) = (b.max.x - b.min.x, b.max.y - b.min.y);
    let c = Vec2::new((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0);
    let (half, width, axis) = if w <= h { (w / 2.0, h, Vec2::new(1.0, 0.0)) } else { (h / 2.0, w, Vec2::new(0.0, 1.0)) };
    let steps = [0.5 * upf, 1.5 * upf, 0.95 * width, 1.25 * width];
    let side = |sign: f64| steps.iter().map(|t| c + axis * (sign * (half + t))).filter(|p| p.is_finite()).collect::<Vec<_>>();
    Some([side(-1.0), side(1.0)])
}

fn run_graph(s: &mut Session, p: &Value) -> Result<Value> {
    let set = p.get("set").and_then(Value::as_bool).unwrap_or(true);
    let v = graph(s, set)?;
    if set && v.get("connected").and_then(Value::as_u64).unwrap_or(0) > 0 {
        s.touch();
    }
    Ok(v)
}

/// Each door between the rooms on its two sides; with `set`, written on the doors as
/// `from_room` / `to_room` (computed).
pub(crate) fn graph(s: &mut Session, set: bool) -> Result<Value> {
    let rooms = super::rooms3d::rooms(s)?;
    let d = s.doc()?;
    let upf = units_per_foot(spec_of(d).as_ref());
    let name_of = |r: &super::rooms3d::Room3| r.number.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| r.name.clone());
    let mut doors = Vec::new();
    let mut edges = Vec::new();
    let mut writes: Vec<(Handle, Option<String>, Option<String>)> = Vec::new();
    for e in d.model.iter() {
        if super::elemprops::element_kind(e).as_deref() != Some("door") {
            continue;
        }
        let store = super::elemprops::effective(e, "door", upf);
        let level = prop_text(&store, "level");
        let Some(sides) = door_sides(d, e, upf) else { continue };
        let room_at = |probes: &[Vec2]| {
            probes.iter().find_map(|p| {
                rooms
                    .iter()
                    .filter(|r| level.is_none() || r.level.is_none() || r.level == level)
                    .filter(|r| cadaddy_geom::point_in_polygon(&r.outline, *p))
                    .min_by(|a, b| a.area.total_cmp(&b.area))
                    .map(name_of)
            })
        };
        let (from, to) = (room_at(&sides[0]), room_at(&sides[1]));
        if from.is_some() && to.is_some() && from != to {
            edges.push(json!({"door": e.handle.hex(), "from": from, "to": to}));
        }
        if from.is_some() || to.is_some() {
            writes.push((e.handle, from.clone(), to.clone()));
        }
        if doors.len() < 10_000 {
            doors.push(json!({"handle": e.handle.hex(), "number": prop_text(&store, "number"), "level": level, "from_room": from, "to_room": to}));
        }
    }
    let connected = writes.len();
    if set {
        let d = s.doc_mut()?;
        for (h, from, to) in writes {
            let _ = d.modify_entity(h, |e| {
                let mut store: Props = props::read(&e.common.xdata);
                for (key, v) in [("from_room", from.clone()), ("to_room", to.clone())] {
                    if store.get(key).is_some_and(|p| p.source.kind == SourceKind::User) {
                        continue;
                    }
                    match v {
                        Some(v) => {
                            store.set(key, Prop::new(v, Source::computed()));
                        }
                        None => {
                            store.remove(key);
                        }
                    }
                }
                e.common.xdata = props::write(&e.common.xdata, &store).into();
            });
        }
    }
    let rooms_json: Vec<Value> = rooms.iter().take(10_000).map(|r| json!({"number": r.number, "name": r.name, "level": r.level})).collect();
    Ok(json!({"rooms": rooms_json, "doors": doors, "edges": edges, "connected": connected}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_mepfix::arch_reader::DoorRow;

    fn placed(number: Option<&str>, level: &str) -> Element {
        Element {
            handle: None,
            tag_id: None,
            kind: "door".into(),
            level: level.into(),
            number: number.map(str::to_string),
            sort: String::new(),
            at: [0.0, 0.0],
            sheet: None,
        }
    }

    #[test]
    fn the_door_schedule_is_checked_against_the_doors_placed() {
        let rows: Vec<DoorRow> =
            ["101", "102", "103", "104", "105", "106", "201", "202"].iter().map(|m| DoorRow { mark: (*m).into(), ..DoorRow::default() }).collect();
        // Level 1 places 101 and 102 (and 199, which the schedule lacks); level 2 both of its own.
        let els = [
            placed(Some("101"), "Level 1"),
            placed(Some("102"), "Level 1"),
            placed(Some("199"), "Level 1"),
            placed(None, "Level 1"),
            placed(Some("201"), "Level 2"),
            placed(Some("202"), "Level 2"),
        ];
        let doors: Vec<&Element> = els.iter().collect();
        let f = schedule_findings(&rows, &doors);
        let kind = |k: &str| f.iter().find(|x| x["kind"] == k).cloned();
        let never = kind("schedule_not_placed").expect("never placed");
        assert_eq!(never["doors"], json!(["103", "104", "105", "106"]));
        assert_eq!(kind("not_in_schedule").expect("unlisted")["doors"], json!(["199"]));
        // Level 1: 4 doors placed against the schedule's six 1xx marks, 2 off (a finding takes
        // more than 2 and more than 10%); level 2 matches.
        assert!(kind("doors_vs_schedule").is_none(), "{f:?}");
        let few = [placed(Some("101"), "Level 1")];
        let f = schedule_findings(&rows, &few.iter().collect::<Vec<_>>());
        let lv = f.iter().find(|x| x["kind"] == "doors_vs_schedule").expect("level 1 short");
        assert_eq!((lv["placed"].as_u64(), lv["scheduled"].as_u64()), (Some(1), Some(6)));
        assert!(schedule_findings(&[], &doors).is_empty(), "no schedule, no findings");
    }

    /// Two guest rooms' thermostats back to back on the wall they share (inches apart, no circuit)
    /// and their receptacles on circuits named alike are two each; one room's device read off two
    /// sheets at one spot is one.
    #[test]
    fn two_rooms_devices_at_one_spot_are_two() {
        let mut sp = BuildingSpec { units_per_foot: 12.0, ..Default::default() };
        let tag = |id: &str, ty: &str, circuit: Option<&str>, room: &str, x: f64| {
            let mut t = cadaddy_walk3d::tags::ElementTag {
                id: id.into(),
                kind: "device".into(),
                level: "Level 1".into(),
                at_ft: [x, 10.0],
                ..Default::default()
            };
            t.set("type", ty, "plan");
            if let Some(c) = circuit {
                t.set("circuit", c, "plan");
            }
            if !room.is_empty() {
                t.set("room", room, "rule");
            }
            t
        };
        sp.tags = vec![
            tag("device:a", "T", None, "101", 41.4),
            tag("device:b", "T", None, "103", 41.6),
            tag("device:c", "REC", Some("G-1"), "101", 30.0),
            tag("device:d", "REC", Some("G-1"), "103", 30.2),
            tag("device:e", "S", None, "105", 50.0),
            tag("device:f", "S", None, "105", 50.1),
        ];
        let els = elements(&Drawing::default(), Some(&sp), &[]);
        let items: Vec<IdItem> = els
            .iter()
            .map(|el| IdItem {
                kind: el.kind.clone(),
                level: el.level.clone(),
                key: el.number.clone().unwrap_or_default(),
                sort: el.sort.clone(),
                at: Some(el.at),
                sheet: String::new(),
                rank: 1,
            })
            .collect();
        let (groups, _) = tagid::merge_identities(&items, &|_| 12.0);
        let gone: usize = groups.iter().map(|g| g.merged.len()).sum();
        assert_eq!(gone, 1, "only the one room's switch read twice: {groups:?}");
    }

    #[test]
    fn a_level_name_gives_its_digit() {
        assert_eq!(level_digit("Level 2"), Some('2'));
        assert_eq!(level_digit("THIRD FLOOR"), Some('3'));
        assert_eq!(level_digit("ROOF"), None);
        assert_eq!(mark_level("D-204"), Some('2'));
    }
}
