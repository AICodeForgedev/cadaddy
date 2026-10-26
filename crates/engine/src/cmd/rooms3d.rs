//! Rooms as 3D spaces: every room the plan's room tags name (`plan.rooms`: the closed outline
//! around each name label, its number) becomes a space with its area, ceiling height, volume and
//! finishes, listed (`rooms.list`, `rooms.get`), gone to (`rooms.goto`) and built as pick-able
//! volumes (`rooms.build`: a mesh per room on the 3D-Rooms layer, see-through, tagged with the
//! room's name and number).
//!
//! A room's ceiling: the building spec's for that room, else the finish schedule's, else a
//! ceiling note inside the room (`CLG 9'-0"`), else its level's in the spec, else 9'-0".
//! Finishes: schedule row (floor/base/wall/ceiling) + spec materials + finish codes written
//! inside the room (`CPT-1`, `VCT-2`…). Each ceiling and finish carries its source.

use cadaddy_doc::{Common, EntityKind, Handle, Mesh, MeshFace, MeshKind, RawTag, Space};
use cadaddy_geom::{Bounds2, Vec2, Vec3};
use cadaddy_mepfix::arch_reader::{RoomFinish, find_finish_schedule};
use cadaddy_walk3d::BuildingSpec;
use serde_json::{Value, json};

use super::perceive::geometry::rooms_found;
use super::perceive::{feet_inches, finish_codes, objects, pt, round, texts, units_per_inch};
use super::*;
use crate::{Result, Session};

/// The xdata application tagging a room's volume (its name and number).
pub const ROOM_APP: &str = "CADADDY_ROOM";
/// The layer room volumes go on.
pub const ROOM_LAYER: &str = "3D-Rooms";
/// A ceiling height when nothing says one: 9'-0" (in inches).
const DEFAULT_CEILING_IN: f64 = 108.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("rooms", "Rooms", run_list)
            .menu(&["Tools", "Building", "Rooms"])
            .params("{outline?: bool (default true)} → as rooms.list (typed or from the menu: the Rooms palette)")
            .noundo(),
        CommandSpec::new("rooms.list", "List Rooms", run_list)
            .params("{outline?: bool (default true)} → {rooms: [{name, number, area, areaSqFt, areaM2, perimeter, ceiling, ceilingFt, ceilingSource, elevation, volume, volumeCuFt, volumeM3, finishes, finishDetail, materials, level, label, at, outline}], count}")
            .noundo(),
        CommandSpec::new("rooms.get", "Room", run_get).params("{name? | number? | label?} → the room (as rooms.list's)").noundo(),
        CommandSpec::new("rooms.goto", "Go to Room", run_goto).params("{name? | number? | label?} → {name, window}: the view shows the room").noundo(),
        CommandSpec::new("rooms.build", "Build Room Volumes", run_build)
            .menu(&["Tools", "Building", "Build Room Volumes"])
            .params("{clear?: true (remove them)} → {built, removed}: a see-through mesh per room on 3D-Rooms, tagged with its name and number"),
    ]
}

/// A finish with its slot and source.
#[derive(Clone, Debug)]
pub struct FinishDetail {
    pub slot: String,
    pub code: String,
    pub source: String,
}

/// A room as a space.
#[derive(Clone, Debug)]
pub struct Room3 {
    pub name: String,
    pub number: Option<String>,
    pub at: Vec2,
    pub label: Handle,
    pub outline: Vec<Vec2>,
    pub area: f64,
    pub perimeter: f64,
    /// Floor elevation and ceiling height above it, drawing units.
    pub elevation: f64,
    pub ceiling: f64,
    pub ceiling_source: String,
    pub finishes: Vec<String>,
    pub finish_detail: Vec<FinishDetail>,
    pub materials: Value,
    pub level: Option<String>,
}

impl Room3 {
    pub fn volume(&self) -> f64 {
        self.area * self.ceiling
    }
}

/// Match a room (by number, else name) to a finish schedule row.
fn match_schedule<'a>(schedule: &'a [RoomFinish], number: Option<&str>, name: &str) -> Option<&'a RoomFinish> {
    if let Some(num) = number
        && let Some(row) = schedule.iter().find(|r| r.number.eq_ignore_ascii_case(num.trim()))
    {
        return Some(row);
    }
    schedule.iter().find(|r| r.name.eq_ignore_ascii_case(name.trim()))
}

/// The plan's rooms as spaces. A built model's rooms are its room ceilings (one per traced room,
/// what `model.build` reported): the one list every room command reads. Before a build, the
/// rooms traced around the plan's name labels.
pub fn rooms(s: &mut Session) -> Result<Vec<Room3>> {
    if let Some(built) = built_rooms(s)? {
        return Ok(built);
    }
    let found = rooms_found(s)?;
    let (d, ents) = objects(s)?;
    let k = units_per_inch(d);
    let ft = 12.0 * k;
    let spec = d.building_spec.as_deref().and_then(|t| BuildingSpec::from_json(t).ok());
    let notes = texts(&ents);
    let schedule = find_finish_schedule(d);
    let mut out = Vec::with_capacity(found.len());
    for r in found {
        let inside: Vec<&String> =
            notes.iter().filter(|(_, _, at, _)| cadaddy_geom::point_in_polygon(&r.outline, *at)).map(|(_, t, _, _)| t).collect();
        let spec_room = spec.as_ref().and_then(|sp| {
            sp.rooms.iter().find(|x| {
                x.name.eq_ignore_ascii_case(&r.name)
                    || (x.number.is_some() && x.number.as_deref().map(str::trim) == r.number.as_deref().map(str::trim))
            })
        });
        let level = spec.as_ref().and_then(|sp| {
            let by_name = spec_room.and_then(|x| x.level.as_deref()).and_then(|n| sp.level(n));
            let in_region = || {
                sp.levels.iter().find(|l| {
                    l.region
                        .is_some_and(|[x0, y0, x1, y1]| r.at.x >= x0.min(x1) && r.at.x <= x0.max(x1) && r.at.y >= y0.min(y1) && r.at.y <= y0.max(y1))
                })
            };
            by_name.or_else(in_region).or_else(|| sp.levels.first())
        });
        let sched_row = match_schedule(&schedule, r.number.as_deref(), &r.name);
        let note = inside.iter().find_map(|t| {
            let up = t.to_ascii_uppercase();
            (up.contains("CLG") || up.contains("CEILING")).then(|| feet_inches(&up)).flatten()
        });
        // Ceiling priority: spec room > schedule > CLG note > spec level > default.
        let (ceiling, ceiling_source) = spec_room
            .and_then(|x| x.ceiling_ft)
            .map(|f| (f * ft, "spec"))
            .or_else(|| sched_row.and_then(|r| r.ceiling_height_ft).map(|f| (f * ft, "schedule")))
            .or_else(|| note.map(|i| (i * k, "note")))
            .or_else(|| level.map(|l| (l.ceiling_ft * ft, "level")))
            .filter(|(c, _)| c.is_finite() && *c > 0.0)
            .unwrap_or((DEFAULT_CEILING_IN * k, "default"));
        // Finishes: schedule slots first, then in-room finish codes.
        let mut finishes: Vec<String> = Vec::new();
        let mut finish_detail: Vec<FinishDetail> = Vec::new();
        if let Some(row) = sched_row {
            for (slot, code) in [("floor", &row.floor), ("base", &row.base), ("wall", &row.wall), ("ceiling", &row.ceiling)] {
                if !code.is_empty() {
                    if !finishes.contains(code) {
                        finishes.push(code.clone());
                    }
                    finish_detail.push(FinishDetail { slot: slot.into(), code: code.clone(), source: "schedule".into() });
                }
            }
        }
        for t in &inside {
            for c in finish_codes(t) {
                if !finishes.contains(&c) {
                    finishes.push(c.clone());
                }
                if !finish_detail.iter().any(|d| d.code == c) {
                    finish_detail.push(FinishDetail { slot: String::new(), code: c, source: "note".into() });
                }
            }
        }
        let materials = spec_room.map(|x| serde_json::to_value(&x.materials).unwrap_or(Value::Null)).unwrap_or(Value::Null);
        out.push(Room3 {
            name: r.name,
            number: r.number,
            at: r.at,
            label: r.label,
            outline: r.outline,
            area: r.area,
            perimeter: r.perimeter,
            elevation: level.map_or(0.0, |l| l.elevation_ft * ft),
            ceiling,
            ceiling_source: ceiling_source.into(),
            finishes,
            finish_detail,
            materials,
            level: level.map(|l| l.name.clone()),
        });
    }
    Ok(out)
}

/// A built room ceiling: its outline, height, level, source tag and the spec room it was built
/// for (`room:<name>` in its tag).
struct BuiltCeiling {
    outline: Vec<Vec2>,
    z: f64,
    level: String,
    source: Handle,
    spec_name: String,
}

/// The room ceilings a build made, if any.
fn built_ceilings(d: &cadaddy_doc::Drawing) -> Vec<BuiltCeiling> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if super::wallgen::made_as(e).as_deref() != Some("ceiling") {
            continue;
        }
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let x = &e.common.xdata;
        let Some(at) = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP)) else { continue };
        let Some(spec_name) = x.iter().skip(at + 1).take_while(|t| t.code != 1001).find_map(|t| t.value.strip_prefix("room:")) else { continue };
        let mut outline: Vec<Vec2> = m.vertices.iter().map(|v| v.xy()).collect();
        if outline.len() > 1 && outline.first().zip(outline.last()).is_some_and(|(a, b)| a.dist(*b) < 1e-9) {
            outline.pop();
        }
        let Some(z) = m.vertices.first().map(|v| v.z).filter(|z| z.is_finite()) else { continue };
        if outline.len() < 3 || !outline.iter().all(|p| p.is_finite()) || out.len() >= MAX_BUILT_ROOMS {
            continue;
        }
        let source = x.get(at + 3).and_then(|t| u64::from_str_radix(t.value.trim(), 16).ok()).map_or(Handle(0), Handle);
        out.push(BuiltCeiling {
            outline,
            z,
            level: super::bspec::made_level(e).unwrap_or_default(),
            source,
            spec_name: spec_name.trim().to_string(),
        });
    }
    out
}

/// Built room ceilings read at most (hostile drawings).
const MAX_BUILT_ROOMS: usize = 20_000;

/// The rooms of a built model, from its room ceilings; `None` when nothing was built.
fn built_rooms(s: &mut Session) -> Result<Option<Vec<Room3>>> {
    let d = s.doc()?;
    let ceilings = built_ceilings(d);
    if ceilings.is_empty() {
        return Ok(None);
    }
    let k = units_per_inch(d);
    let spec = d.building_spec.as_deref().and_then(|t| BuildingSpec::from_json(t).ok());
    let upf = spec.as_ref().map(|sp| sp.units_per_foot).filter(|u| u.is_finite() && *u > 0.0).unwrap_or(12.0 * k);
    let schedule = find_finish_schedule(d);
    let plan: Vec<&std::sync::Arc<cadaddy_doc::Entity>> =
        d.model.iter().filter(|e| matches!(e.kind, EntityKind::Text(_) | EntityKind::MText(_)) && super::wallgen::made_as(e).is_none()).collect();
    let notes = texts(&plan);
    let mut out = Vec::with_capacity(ceilings.len());
    for c in &ceilings {
        let b = Bounds2::from_points(c.outline.iter().copied());
        let inside: Vec<&(Handle, String, Vec2, f64)> =
            notes.iter().filter(|(_, _, at, _)| b.contains(*at) && cadaddy_geom::point_in_polygon(&c.outline, *at)).collect();
        let spec_room = spec
            .as_ref()
            .and_then(|sp| sp.rooms.iter().find(|x| !c.spec_name.is_empty() && super::bspec::room_label(x).eq_ignore_ascii_case(&c.spec_name)));
        let area = cadaddy_geom::shoelace(&c.outline).abs();
        let centre = c.outline.iter().fold(Vec2::ZERO, |a, p| a + *p) * (1.0 / c.outline.len() as f64);
        let label = inside
            .iter()
            .map(|t| (super::perceive::geometry::room_name_score(&t.1), *t))
            .filter(|(sc, _)| *sc > 0)
            .max_by(|a, b| a.0.cmp(&b.0).then_with(|| u8::from(a.1.0 == c.source).cmp(&u8::from(b.1.0 == c.source))))
            .map(|(_, t)| t);
        let source_text = notes.iter().find(|t| t.0 == c.source);
        let number = inside
            .iter()
            .filter(|t| super::perceive::geometry::room_number(&t.1))
            .min_by(|a, b| a.2.dist(label.map_or(centre, |l| l.2)).total_cmp(&b.2.dist(label.map_or(centre, |l| l.2))))
            .map(|t| t.1.clone())
            .or_else(|| spec_room.and_then(|x| x.number.clone()));
        let name = match (spec_room, label) {
            (Some(x), _) if !x.name.trim().is_empty() => x.name.clone(),
            (_, Some(l)) => l.1.clone(),
            _ => number.as_ref().map_or_else(|| "Room".to_string(), |n| format!("Room {n}")),
        };
        let level = spec.as_ref().and_then(|sp| sp.level(&c.level));
        let elevation = level.map_or(0.0, |l| l.elevation_ft * upf);
        let sched_row = match_schedule(&schedule, number.as_deref(), &name);
        let note_in = inside.iter().find_map(|t| {
            let up = t.1.to_ascii_uppercase();
            (up.contains("CLG") || up.contains("CEILING")).then(|| feet_inches(&up)).flatten()
        });
        let ceiling_from_z = Some(c.z - elevation).filter(|h| h.is_finite() && *h > 0.0);
        let (ceiling, ceiling_source) = ceiling_from_z
            .map(|h| (h, "spec"))
            .or_else(|| sched_row.and_then(|r| r.ceiling_height_ft).map(|f| (f * upf, "schedule")))
            .or_else(|| note_in.map(|i| (i * k, "note")))
            .or_else(|| level.map(|l| (l.ceiling_ft * upf, "level")))
            .filter(|(c, _)| c.is_finite() && *c > 0.0)
            .unwrap_or((DEFAULT_CEILING_IN * k, "default"));
        let mut finishes: Vec<String> = Vec::new();
        let mut finish_detail: Vec<FinishDetail> = Vec::new();
        if let Some(row) = sched_row {
            for (slot, code) in [("floor", &row.floor), ("base", &row.base), ("wall", &row.wall), ("ceiling", &row.ceiling)] {
                if !code.is_empty() {
                    if !finishes.contains(code) {
                        finishes.push(code.clone());
                    }
                    finish_detail.push(FinishDetail { slot: slot.into(), code: code.clone(), source: "schedule".into() });
                }
            }
        }
        for t in &inside {
            for f in finish_codes(&t.1) {
                if !finishes.contains(&f) {
                    finishes.push(f.clone());
                }
                if !finish_detail.iter().any(|d| d.code == f) {
                    finish_detail.push(FinishDetail { slot: String::new(), code: f, source: "note".into() });
                }
            }
        }
        let perimeter: f64 = c.outline.iter().zip(c.outline.iter().cycle().skip(1)).map(|(a, b)| a.dist(*b)).sum();
        out.push(Room3 {
            name,
            number,
            at: label.map(|l| l.2).or_else(|| source_text.map(|t| t.2)).unwrap_or(centre),
            label: label.map(|l| l.0).unwrap_or(c.source),
            outline: c.outline.clone(),
            area,
            perimeter,
            elevation,
            ceiling,
            ceiling_source: ceiling_source.into(),
            finishes,
            finish_detail,
            materials: spec_room.map(|x| serde_json::to_value(&x.materials).unwrap_or(Value::Null)).unwrap_or(Value::Null),
            level: Some(c.level.clone()).filter(|l| !l.is_empty()),
        });
    }
    Ok(Some(out))
}

fn room_json(d: &cadaddy_doc::Drawing, r: &Room3, outline: bool) -> Value {
    let k = units_per_inch(d);
    let (ft, m) = (12.0 * k, k / 0.0254);
    let detail: Vec<Value> = r
        .finish_detail
        .iter()
        .map(|d| {
            if d.slot.is_empty() { json!({"code": d.code, "source": d.source}) } else { json!({"slot": d.slot, "code": d.code, "source": d.source}) }
        })
        .collect();
    let mut v = json!({
        "name": r.name, "number": r.number, "level": r.level, "label": r.label.hex(), "at": pt(r.at),
        "area": round(r.area), "areaSqFt": round(r.area / (ft * ft)), "areaM2": round(r.area / (m * m)),
        "perimeter": round(r.perimeter),
        "elevation": round(r.elevation), "ceiling": round(r.ceiling), "ceilingFt": round(r.ceiling / ft),
        "ceilingSource": r.ceiling_source,
        "volume": round(r.volume()), "volumeCuFt": round(r.volume() / (ft * ft * ft)), "volumeM3": round(r.volume() / (m * m * m)),
        "finishes": r.finishes, "finishDetail": detail, "materials": r.materials,
    });
    if outline && let Some(o) = v.as_object_mut() {
        let step = (r.outline.len() / 64).max(1);
        o.insert("outline".into(), json!(r.outline.iter().step_by(step).map(|q| pt(*q)).collect::<Vec<_>>()));
    }
    v
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let rs = rooms(s)?;
    let outline = bool_or(p, "outline", true);
    let d = s.doc()?;
    let list: Vec<Value> = rs.iter().map(|r| room_json(d, r, outline)).collect();
    Ok(json!({ "rooms": list, "count": rs.len() }))
}

/// The room `p` names (`name`, any case; `number`; or `label` (handle hex)).
fn named(s: &mut Session, cmd: &str, p: &Value) -> Result<Room3> {
    let (name, number, label) = (str_param(p, "name").map(str::trim), str_param(p, "number").map(str::trim), str_param(p, "label").map(str::trim));
    if name.is_none() && number.is_none() && label.is_none() {
        return Err(bad(cmd, "pass the room's `name`, `number` or `label`"));
    }
    let all = rooms(s)?;
    // Label (handle hex) is exact: one room.
    if let Some(lbl) = label
        && let Ok(h) = u64::from_str_radix(lbl.trim_start_matches("0x").trim_start_matches("0X"), 16)
    {
        return all.into_iter().find(|r| r.label.0 == h).ok_or_else(|| bad(cmd, format!("no room with label {lbl}")));
    }
    // Number is unambiguous.
    if let Some(num) = number {
        return all
            .into_iter()
            .find(|r| r.number.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(num)))
            .ok_or_else(|| bad(cmd, format!("no room numbered {num}")));
    }
    // Name: check for duplicates.
    // Safety: name must be Some here — we returned Err above when all three are None,
    // and we returned above for label and number cases.
    let Some(n) = name else {
        return Err(bad(cmd, "pass the room's `name`, `number` or `label`"));
    };
    let mut matches: Vec<Room3> = all.into_iter().filter(|r| r.name.eq_ignore_ascii_case(n)).collect();
    match matches.len() {
        0 => Err(bad(cmd, format!("no room {n}"))),
        1 => Ok(matches.swap_remove(0)),
        _ => {
            let list: Vec<String> = matches.iter().map(|r| format!("{} (label {})", r.number.as_deref().unwrap_or("—"), r.label.hex())).collect();
            Err(bad(cmd, format!("{} rooms match \"{}\": {}; pass `number` or `label` to select one", matches.len(), n, list.join(", "))))
        }
    }
}

fn run_get(s: &mut Session, p: &Value) -> Result<Value> {
    let r = named(s, "rooms.get", p)?;
    let d = s.doc()?;
    Ok(room_json(d, &r, true))
}

fn run_goto(s: &mut Session, p: &Value) -> Result<Value> {
    let r = named(s, "rooms.goto", p)?;
    let b = Bounds2::from_points(r.outline.iter().copied());
    let pad = b.width().max(b.height()) * 0.1;
    let (lo, hi) = (b.min - Vec2::new(pad, pad), b.max + Vec2::new(pad, pad));
    let zoom = find_command("zoom").ok_or_else(|| EngineError::UnknownCommand("zoom".into()))?;
    (zoom.run)(s, &json!({ "mode": "window", "p1": [lo.x, lo.y], "p2": [hi.x, hi.y] }))?;
    Ok(json!({ "name": r.name, "number": r.number, "window": [[lo.x, lo.y], [hi.x, hi.y]] }))
}

/// Whether an object is a room volume (its xdata), and which room.
pub fn room_of(c: &Common) -> Option<String> {
    let i = c.xdata.iter().position(|t| t.code == 1001 && t.value == ROOM_APP)?;
    c.xdata.get(i + 1).filter(|t| t.code == 1000).map(|t| t.value.clone())
}

/// A room's volume: its outline as floor and ceiling faces joined by its walls.
fn volume_mesh(r: &Room3) -> Option<Mesh> {
    let mut pts = r.outline.clone();
    if pts.len() > 1 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.dist(*b) < 1e-9) {
        pts.pop();
    }
    let n = pts.len();
    if !(3..=10_000).contains(&n) {
        return None;
    }
    if cadaddy_geom::shoelace(&pts) < 0.0 {
        pts.reverse();
    }
    let (z0, z1) = (r.elevation, r.elevation + r.ceiling);
    let mut vertices: Vec<Vec3> = pts.iter().map(|p| Vec3::new(p.x, p.y, z0)).collect();
    vertices.extend(pts.iter().map(|p| Vec3::new(p.x, p.y, z1)));
    let n32 = u32::try_from(n).ok()?;
    let mut faces = vec![MeshFace { v: (0..n32).rev().collect(), hidden: 0 }, MeshFace { v: (n32..2 * n32).collect(), hidden: 0 }];
    for i in 0..n32 {
        let j = (i + 1) % n32;
        faces.push(MeshFace { v: vec![i, j, j + n32, i + n32], hidden: 0 });
    }
    Some(Mesh { vertices, faces, kind: MeshKind::Subd })
}

fn run_build(s: &mut Session, p: &Value) -> Result<Value> {
    let old: Vec<Handle> = s.doc()?.model.iter().filter(|e| room_of(&e.common).is_some()).map(|e| e.handle).collect();
    for h in &old {
        s.doc_mut()?.remove_entity(*h);
    }
    if p.get("clear").and_then(Value::as_bool) == Some(true) {
        return Ok(json!({ "built": 0, "removed": old.len() }));
    }
    let rs = rooms(s)?;
    let d = s.doc_mut()?;
    if d.layer(ROOM_LAYER).is_none() {
        d.layers.push(cadaddy_doc::Layer { name: ROOM_LAYER.into(), color: cadaddy_doc::color::Color::Index(151), ..cadaddy_doc::Layer::default() });
    }
    let mut built = 0;
    for r in &rs {
        let Some(m) = volume_mesh(r) else { continue };
        let tag = vec![
            RawTag { code: 1001, value: ROOM_APP.into() },
            RawTag { code: 1000, value: r.name.chars().take(250).collect() },
            RawTag { code: 1000, value: r.number.clone().unwrap_or_default().chars().take(250).collect() },
        ];
        let common =
            Common { layer: ROOM_LAYER.into(), transparency: cadaddy_doc::Transparency::Percent(80), xdata: tag.into(), ..Common::default() };
        if d.add(&Space::Model, common, EntityKind::Mesh(m)).is_ok() {
            built += 1;
        }
    }
    s.echo(format!("{built} room volume(s) on {ROOM_LAYER}."));
    Ok(json!({ "built": built, "removed": old.len() }))
}
