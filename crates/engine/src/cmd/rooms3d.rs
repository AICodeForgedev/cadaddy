//! Rooms as 3D spaces: every room the plan's room tags name (`plan.rooms`: the closed outline
//! around each name label, its number) becomes a space with its area, ceiling height, volume and
//! finishes, listed (`rooms.list`, `rooms.get`), gone to (`rooms.goto`) and built as pick-able
//! volumes (`rooms.build`: a mesh per room on the 3D-Rooms layer, see-through, tagged with the
//! room's name and number).
//!
//! A room's ceiling: the building spec's for that room, else a ceiling note inside the room
//! (`CLG 9'-0"`), else its level's in the spec, else 9'-0". Finishes: the spec room's materials and
//! the finish codes written inside the room (`CPT-1`, `VCT-2`…).

use cadaddy_doc::{Common, EntityKind, Handle, Mesh, MeshFace, MeshKind, RawTag, Space};
use cadaddy_geom::{Bounds2, Vec2, Vec3};
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
            .params("{outline?: bool (default true)} → {rooms: [{name, number, area, areaSqFt, areaM2, perimeter, ceiling, ceilingFt, elevation, volume, volumeCuFt, volumeM3, finishes, materials, level, label, at, outline}], count}")
            .noundo(),
        CommandSpec::new("rooms.get", "Room", run_get).params("{name? | number?} → the room (as rooms.list's)").noundo(),
        CommandSpec::new("rooms.goto", "Go to Room", run_goto).params("{name? | number?} → {name, window}: the view shows the room").noundo(),
        CommandSpec::new("rooms.build", "Build Room Volumes", run_build)
            .menu(&["Tools", "Building", "Build Room Volumes"])
            .params("{clear?: true (remove them)} → {built, removed}: a see-through mesh per room on 3D-Rooms, tagged with its name and number"),
    ]
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
    pub finishes: Vec<String>,
    pub materials: Value,
    pub level: Option<String>,
}

impl Room3 {
    pub fn volume(&self) -> f64 {
        self.area * self.ceiling
    }
}

/// The plan's rooms as spaces.
pub fn rooms(s: &mut Session) -> Result<Vec<Room3>> {
    let found = rooms_found(s)?;
    let (d, ents) = objects(s)?;
    let k = units_per_inch(d);
    let ft = 12.0 * k;
    let spec = d.building_spec.as_deref().and_then(|t| BuildingSpec::from_json(t).ok());
    let notes = texts(&ents);
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
            // The level whose sheet region holds the room's tag.
            let in_region = || {
                sp.levels.iter().find(|l| {
                    l.region
                        .is_some_and(|[x0, y0, x1, y1]| r.at.x >= x0.min(x1) && r.at.x <= x0.max(x1) && r.at.y >= y0.min(y1) && r.at.y <= y0.max(y1))
                })
            };
            by_name.or_else(in_region).or_else(|| sp.levels.first())
        });
        let note = inside.iter().find_map(|t| {
            let up = t.to_ascii_uppercase();
            (up.contains("CLG") || up.contains("CEILING")).then(|| feet_inches(&up)).flatten()
        });
        let ceiling = spec_room
            .and_then(|x| x.ceiling_ft)
            .map(|f| f * ft)
            .or_else(|| note.map(|i| i * k))
            .or_else(|| level.map(|l| l.ceiling_ft * ft))
            .filter(|c| c.is_finite() && *c > 0.0)
            .unwrap_or(DEFAULT_CEILING_IN * k);
        let mut finishes: Vec<String> = Vec::new();
        for t in &inside {
            for c in finish_codes(t) {
                if !finishes.contains(&c) {
                    finishes.push(c);
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
            finishes,
            materials,
            level: level.map(|l| l.name.clone()),
        });
    }
    Ok(out)
}

fn room_json(d: &cadaddy_doc::Drawing, r: &Room3, outline: bool) -> Value {
    let k = units_per_inch(d);
    let (ft, m) = (12.0 * k, k / 0.0254);
    let mut v = json!({
        "name": r.name, "number": r.number, "level": r.level, "label": r.label.hex(), "at": pt(r.at),
        "area": round(r.area), "areaSqFt": round(r.area / (ft * ft)), "areaM2": round(r.area / (m * m)),
        "perimeter": round(r.perimeter),
        "elevation": round(r.elevation), "ceiling": round(r.ceiling), "ceilingFt": round(r.ceiling / ft),
        "volume": round(r.volume()), "volumeCuFt": round(r.volume() / (ft * ft * ft)), "volumeM3": round(r.volume() / (m * m * m)),
        "finishes": r.finishes, "materials": r.materials,
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

/// The room `p` names (`name`, any case, or `number`).
fn named(s: &mut Session, cmd: &str, p: &Value) -> Result<Room3> {
    let (name, number) = (str_param(p, "name").map(str::trim), str_param(p, "number").map(str::trim));
    if name.is_none() && number.is_none() {
        return Err(bad(cmd, "pass the room's `name` or `number`"));
    }
    rooms(s)?
        .into_iter()
        .find(|r| {
            name.is_some_and(|n| r.name.eq_ignore_ascii_case(n))
                || number.is_some_and(|n| r.number.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(n)))
        })
        .ok_or_else(|| bad(cmd, format!("no room {}", name.or(number).unwrap_or_default())))
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
    // Counterclockwise from above, so the faces point out.
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
    // What an earlier build made goes first.
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
        let common = Common { layer: ROOM_LAYER.into(), transparency: cadaddy_doc::Transparency::Percent(80), xdata: tag, ..Common::default() };
        if d.add(&Space::Model, common, EntityKind::Mesh(m)).is_ok() {
            built += 1;
        }
    }
    s.echo(format!("{built} room volume(s) on {ROOM_LAYER}."));
    Ok(json!({ "built": built, "removed": old.len() }))
}
