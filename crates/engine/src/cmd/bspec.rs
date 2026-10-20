//! The building spec commands (`cadaddy-walk3d`'s [`BuildingSpec`], kept in the drawing):
//! `bspec.get` (the stored spec, or one drafted from Walk mode's rules), `bspec.set` (merge into
//! it), `bspec.validate` (errors and warnings, with the handles of the objects concerned) and
//! `model.build`, which builds the 3D building from the spec and the plan. A build is
//! deterministic: the same spec and drawing give the same objects in the same order. It uses
//! WALLGEN's generators (walls from face pairs or centrelines, openings, floors, ceilings) and adds
//! what the spec says beyond Walk mode's rules: columns, curtain walls, a ceiling per room at its
//! own height, the ground, and the materials of every part (`material` in the objects' xdata).
//!
//! Room and column caps are triangulated by ear clipping, after D. Eberly, "Triangulation by Ear
//! Clipping", Geometric Tools (2002).

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, MeshKind, RawTag, Space};
use cadaddy_geom::{Polyline, Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, Issue, Level, Part, Role, level_at};
use serde_json::{Value, json};
use std::sync::Arc;

use super::wallgen::{self, WallGen};
use super::*;
use crate::{Result, Session};

/// Objects a validation lists per issue, at most.
const MAX_HANDLES: usize = 50;
/// Rooms traced per build, at most.
const MAX_ROOMS: usize = 2_000;
/// Objects looked at per build, at most (hostile drawings).
const MAX_OBJECTS: usize = 200_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("bspec.get", "Building Spec", run_get)
            .params("{} → {spec, stored: bool (false: drafted from the walk rules — <drawing>.walk.json, the last WALLGEN, else the defaults), parts: [{section, key, text (plain language), approved, guess?: why it was guessed}], built: bool}")
            .noundo(),
        CommandSpec::new("bspec.set", "Set Building Spec", run_set).params(
            "{spec: object (merged into the drawing's spec: objects key by key, null removes; levels, wall_types, rooms, families merge by name and layers by layer, {\"remove\": true} removes one), replace?: bool} → {spec, errors, warnings}",
        ),
        CommandSpec::new("bspec.validate", "Validate Building Spec", run_validate)
            .params("{spec?: object (merged into the drawing's, not stored), replace?: bool (the given spec alone)} → {ok, errors: [{path, message, handles}], warnings: [...]}")
            .noundo(),
        CommandSpec::new("model.build", "Build Model from Spec", run_build).params(
            "{spec?: object (merged into the drawing's, not stored), replace?: bool, slabThickness?: feet (0.5), auto?: bool (default true: build again when the plan changes), clear?: true} → {walls, doors, windows, floors, ceilings, columns, curtainWalls, rooms, ground, stairs, ramps, railings, roofs, parapets, crickets, equipment, facadeGlass, rcpCeilings, grids, soffits, lights, diffusers, mepRuns, elbows, drops, gridColumns, levels, removed, notes, warnings}",
        ),
    ]
}

/// The drawing's stored spec, if it has one.
fn stored(d: &Drawing) -> std::result::Result<Option<BuildingSpec>, String> {
    d.building_spec.as_deref().filter(|t| !t.trim().is_empty()).map(BuildingSpec::from_json).transpose()
}

/// The drawing's spec, else one drafted from the walk rules; whether it was stored.
pub(crate) fn current(s: &Session, cmd: &str) -> Result<(BuildingSpec, bool)> {
    match stored(s.doc()?).map_err(|e| bad(cmd, e))? {
        Some(sp) => Ok((sp, true)),
        None => Ok((BuildingSpec::from_rules(&wallgen::rules_for(s, &json!({}))?), false)),
    }
}

/// The current spec with `patch` merged in (or `patch` alone with `replace`).
fn merged(s: &Session, cmd: &str, patch: Option<&Value>, replace: bool) -> Result<BuildingSpec> {
    let (base, _) = current(s, cmd)?;
    let Some(patch) = patch.filter(|v| !v.is_null()) else { return Ok(base) };
    if !patch.is_object() {
        return Err(bad(cmd, "`spec` must be an object"));
    }
    let mut v = if replace { json!({}) } else { serde_json::to_value(&base).map_err(|e| bad(cmd, e.to_string()))? };
    cadaddy_walk3d::merge(&mut v, patch);
    BuildingSpec::from_json(&v.to_string()).map_err(|e| bad(cmd, e))
}

fn issue_json(i: &Issue) -> Value {
    json!({ "path": i.path, "message": i.message, "handles": i.handles.iter().take(MAX_HANDLES).map(|h| h.hex()).collect::<Vec<_>>() })
}

fn report(issues: &[Issue]) -> Value {
    let errors: Vec<Value> = issues.iter().filter(|i| i.error).map(issue_json).collect();
    let warnings: Vec<Value> = issues.iter().filter(|i| !i.error).map(issue_json).collect();
    json!({ "ok": errors.is_empty(), "errors": errors, "warnings": warnings })
}

/// The plan's objects (not what a build made), at most [`MAX_OBJECTS`].
pub(crate) fn plan_objects(d: &Drawing) -> Vec<&Arc<Entity>> {
    d.model.iter().filter(|e| wallgen::made_as(e).is_none()).take(MAX_OBJECTS).collect()
}

/// The centre of an object on the sheet.
pub(crate) fn centre(d: &Drawing, e: &Entity) -> Option<Vec2> {
    let b = cadaddy_doc::entity_bounds(d, e, 0);
    (!b.is_empty()).then(|| b.center())
}

/// The spec checked alone and against the drawing.
pub(crate) fn validate(d: &Drawing, sp: &BuildingSpec) -> Vec<Issue> {
    let mut out = sp.check();
    let ents = plan_objects(d);
    let wild = |pat: &str, name: &str| super::qselect::wildcard(pat, name);
    for (i, r) in sp.layers.iter().enumerate() {
        let at = format!("layers[{i}]");
        if !d.layers.iter().any(|l| wild(&r.layer, &l.name)) {
            out.push(Issue::warning(&at, format!("no layer matches {}", r.layer)));
        } else if !ents.iter().any(|e| sp.role(&e.common.layer).is_some_and(|x| std::ptr::eq(x, r))) {
            out.push(Issue::warning(&at, format!("nothing is drawn on {}", r.layer)));
        }
        if (r.role == Role::Stair && sp.stairs.is_empty()) || r.role == Role::CeilingGrid {
            let hs: Vec<Handle> = ents.iter().filter(|e| sp.role(&e.common.layer).is_some_and(|x| std::ptr::eq(x, r))).map(|e| e.handle).collect();
            if !hs.is_empty() {
                let message = if r.role == Role::Stair {
                    format!("{} objects on {} but no stairs in the spec (stairs.detect finds them)", hs.len(), r.layer)
                } else {
                    format!("ceiling grids aren't built yet ({} objects on {})", hs.len(), r.layer)
                };
                out.push(Issue { error: false, path: at, message, handles: hs });
            }
        }
    }
    for (i, f) in sp.families.iter().enumerate() {
        if !d.blocks.keys().any(|b| wild(&f.block, b)) {
            out.push(Issue::warning(format!("families[{i}].block"), format!("no block matches {}", f.block)));
        }
    }
    // Blocks on door and window layers that no family covers (and whose name says nothing).
    let mut loose: std::collections::BTreeMap<String, Vec<Handle>> = std::collections::BTreeMap::new();
    for e in &ents {
        let EntityKind::Insert(ins) = &e.kind else { continue };
        let on = sp.role(&e.common.layer).is_some_and(|r| matches!(r.role, Role::Door | Role::Window));
        if on && sp.family(&ins.block).is_none() && !sp.families.is_empty() {
            loose.entry(ins.block.clone()).or_default().push(e.handle);
        }
    }
    for (block, hs) in loose {
        out.push(Issue {
            error: false,
            path: "families".into(),
            message: format!("no family covers block {block} ({} references)", hs.len()),
            handles: hs,
        });
    }
    // Rooms the plan has no tag for.
    if !sp.rooms.is_empty() {
        let tags = room_texts(sp, &ents);
        for (i, r) in sp.rooms.iter().enumerate() {
            if !tags.iter().any(|(_, t, _)| names_room(r, t)) {
                out.push(Issue::warning(format!("rooms[{i}]"), format!("no room tag reads {}", room_label(r))));
            }
        }
    }
    // Building objects outside every level's region.
    let here: Vec<&Level> = sp.levels.iter().filter(|l| l.sheet.is_none()).collect();
    if !here.is_empty() && here.iter().all(|l| l.region.is_some()) {
        let hs: Vec<Handle> = ents
            .iter()
            .filter(|e| sp.role(&e.common.layer).is_some_and(|r| r.role != Role::Ignore))
            .filter(|e| centre(d, e).is_some_and(|c| level_at(&sp.levels, c.x, c.y).is_none()))
            .map(|e| e.handle)
            .collect();
        if !hs.is_empty() {
            out.push(Issue {
                error: false,
                path: "levels".into(),
                message: format!("{} building objects lie outside every level's region", hs.len()),
                handles: hs,
            });
        }
    }
    for (i, l) in sp.levels.iter().enumerate().filter(|(_, l)| l.sheet.is_some()) {
        out.push(Issue::warning(
            format!("levels[{i}].sheet"),
            format!("{} is on another sheet: model.build builds this drawing's levels only", l.name),
        ));
    }
    out
}

pub(crate) fn room_label(r: &cadaddy_walk3d::SpecRoom) -> String {
    match &r.number {
        Some(n) if r.name.trim().is_empty() => n.clone(),
        Some(n) => format!("{} or {n}", r.name),
        None => r.name.clone(),
    }
}

/// Whether a tag text names the room (its name or number, case and spacing aside).
pub(crate) fn names_room(r: &cadaddy_walk3d::SpecRoom, text: &str) -> bool {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let t = norm(text);
    (!r.name.trim().is_empty() && norm(&r.name) == t) || r.number.as_deref().is_some_and(|n| !n.trim().is_empty() && norm(n) == t)
}

/// The texts that tag rooms: those on room tag layers, else every text.
pub(crate) fn room_texts(sp: &BuildingSpec, ents: &[&Arc<Entity>]) -> Vec<(Handle, String, Vec2)> {
    let tagged = sp.layers.iter().any(|r| r.role == Role::RoomTag);
    let on: Vec<&Arc<Entity>> =
        ents.iter().copied().filter(|e| !tagged || sp.role(&e.common.layer).is_some_and(|r| r.role == Role::RoomTag)).collect();
    super::perceive::texts(&on).into_iter().filter(|(_, t, at, _)| !t.is_empty() && at.is_finite()).map(|(h, t, at, _)| (h, t, at)).collect()
}

/// Whether a layer's objects bound rooms: walls, curtain walls, columns, doors and windows.
pub(crate) fn bounds_rooms(sp: &BuildingSpec, layer: &str) -> bool {
    sp.role(layer).is_some_and(|r| matches!(r.role, Role::Wall | Role::CurtainWall | Role::Column | Role::Door | Role::Window))
}

/// A plan to trace rooms on: the model space with only what bounds rooms. Never what a build
/// made (wall, floor and room volumes: their triangles would cut rooms up). With a spec that
/// names wall layers, only the objects on room-bounding layers ([`bounds_rooms`]), so
/// annotation, stair, MEP and dimension lines never split a room, and each centreline wall
/// drawn as its two faces (a room ends at the partition's face, not its middle). None: nothing
/// to leave out, or not in model space (trace on the drawing itself).
pub(crate) fn room_plan(s: &Session) -> Result<Option<Session>> {
    if s.space().is_paper() {
        return Ok(None);
    }
    let d = s.doc()?;
    let sp = d.building_spec.as_deref().and_then(|t| BuildingSpec::from_json(t).ok());
    let sp = sp.filter(|sp| sp.layers.iter().any(|r| matches!(r.role, Role::Wall | Role::CurtainWall)));
    let generated = |e: &Entity| wallgen::made_as(e).is_some() || super::rooms3d::room_of(&e.common).is_some();
    let keep = |e: &Entity| !generated(e) && sp.as_ref().is_none_or(|sp| bounds_rooms(sp, &e.common.layer));
    if d.model.iter().all(|e| keep(e)) && sp.as_ref().is_none_or(|sp| !sp.wall_types.iter().any(|t| t.from == cadaddy_walk3d::WallFrom::Centerline)) {
        return Ok(None);
    }
    let mut wd = d.clone();
    let mut faces: Vec<(Common, Vec<Vec2>)> = Vec::new();
    let mut drop: Vec<Handle> = Vec::new();
    for e in d.model.iter().take(MAX_OBJECTS) {
        if !keep(e) {
            drop.push(e.handle);
            continue;
        }
        // A centreline wall: a closed outline its thickness wide around each straight run.
        let half = sp.as_ref().and_then(|sp| {
            let t = sp.role(&e.common.layer).and_then(|r| sp.wall_type_of(r)).filter(|t| t.from == cadaddy_walk3d::WallFrom::Centerline)?;
            let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
            Some(t.thickness_in * upf / 24.0).filter(|h| h.is_finite() && *h > 0.0)
        });
        let Some(half) = half else { continue };
        let runs: Vec<(Vec2, Vec2)> = match &e.kind {
            EntityKind::Line(l) => vec![(l.a.xy(), l.b.xy())],
            EntityKind::LwPolyline(p) if p.vertices.iter().all(|v| v.bulge == 0.0) => {
                let n = p.vertices.len();
                let ends = if p.closed { n } else { n.saturating_sub(1) };
                (0..ends).filter_map(|i| Some((p.vertices.get(i)?.p, p.vertices.get((i + 1) % n)?.p))).collect()
            }
            _ => continue,
        };
        drop.push(e.handle);
        for (a, b) in runs {
            let len = a.dist(b);
            if !(len.is_finite() && len > 0.0) {
                continue;
            }
            let n = Vec2::new(-(b.y - a.y), b.x - a.x) * (half / len);
            faces.push((e.common.clone(), vec![a + n, b + n, b - n, a - n]));
        }
    }
    for h in drop {
        wd.model.remove(h);
    }
    for (common, q) in faces.into_iter().take(MAX_OBJECTS) {
        let kind = super::helpers::lwpoly(q.into_iter().map(cadaddy_geom::PolyVertex::new).collect(), true);
        wd.add(&Space::Model, common, kind)?;
    }
    let mut t = Session::empty();
    t.open_drawing(wd, "Room boundaries", None);
    Ok(Some(t))
}

/// A described part as `bspec.get` and `bspec.draft` give it.
pub(crate) fn part_json(d: &cadaddy_walk3d::Described) -> Value {
    let mut v = json!({ "section": d.section, "key": d.key, "text": d.text, "approved": d.approved });
    if let Some(why) = &d.guess {
        v["guess"] = json!(why);
    }
    v
}

fn run_get(s: &mut Session, _p: &Value) -> Result<Value> {
    let (sp, was) = current(s, "bspec.get")?;
    let parts: Vec<Value> = sp.describe().iter().map(part_json).collect();
    let built = s.doc()?.model.iter().any(|e| wallgen::made_as(e).is_some());
    Ok(json!({ "spec": serde_json::to_value(&sp).unwrap_or(Value::Null), "stored": was, "parts": parts, "built": built }))
}

fn run_set(s: &mut Session, p: &Value) -> Result<Value> {
    let Some(patch) = p.get("spec").filter(|v| !v.is_null()) else { return Err(bad("bspec.set", "`spec` is required")) };
    let sp = merged(s, "bspec.set", Some(patch), bool_or(p, "replace", false))?;
    let issues = validate(s.doc()?, &sp);
    s.doc_mut()?.building_spec = Some(sp.to_json());
    let mut r = report(&issues);
    r["spec"] = serde_json::to_value(&sp).unwrap_or(Value::Null);
    if let Some(o) = r.as_object_mut() {
        o.remove("ok");
    }
    Ok(r)
}

fn run_validate(s: &mut Session, p: &Value) -> Result<Value> {
    let sp = merged(s, "bspec.validate", p.get("spec"), bool_or(p, "replace", false))?;
    Ok(report(&validate(s.doc()?, &sp)))
}

fn run_build(s: &mut Session, p: &Value) -> Result<Value> {
    if bool_or(p, "clear", false) {
        let n = wallgen::remove_made(s.doc_mut()?);
        s.state_mut()?.wallgen = None;
        return Ok(json!({ "removed": n }));
    }
    let sp = merged(s, "model.build", p.get("spec"), bool_or(p, "replace", false))?;
    let issues = validate(s.doc()?, &sp);
    let errors: Vec<String> = issues.iter().filter(|i| i.error).map(|i| format!("{}: {}", i.path, i.message)).collect();
    if !errors.is_empty() {
        return Err(bad("model.build", format!("the building spec has errors — {}", errors.iter().take(5).cloned().collect::<Vec<_>>().join("; "))));
    }
    let slab_ft = match p.get("slabThickness") {
        None | Some(Value::Null) => 0.5,
        Some(v) => v
            .as_f64()
            .filter(|t| t.is_finite() && (0.0..=10.0).contains(t))
            .ok_or_else(|| bad("model.build", "`slabThickness` must be from 0 to 10 feet"))?,
    };
    let g = WallGen { rules: sp.to_rules(), auto: bool_or(p, "auto", true), slab_ft, layers: None, spec: Some(sp) };
    let mut r = build_spec(s, &g)?;
    r["warnings"] = json!(issues.iter().filter(|i| !i.error).map(issue_json).collect::<Vec<_>>());
    super::revise3d::record(s, &g);
    s.state_mut()?.wallgen = Some(g);
    s.echo(format!(
        "3D building from the spec: {} walls, {} doors, {} windows, {} columns, {} curtain walls, {} floors, {} ceilings ({} rooms) on {} level(s).",
        r["walls"], r["doors"], r["windows"], r["columns"], r["curtainWalls"], r["floors"], r["ceilings"], r["rooms"], r["levels"]
    ));
    Ok(r)
}

/// A room traced on the plan: its spec entry (if any), level, outline and tag.
struct Traced {
    room: Option<usize>,
    level: String,
    outline: Vec<Vec2>,
    tag: Handle,
}

/// The rooms of the plan: each room tag's closed outline (the boundary search HATCH uses),
/// traced before anything is built so the generated walls don't get in the way.
fn trace_rooms(s: &Session, sp: &BuildingSpec, notes: &mut Vec<String>) -> Result<Vec<Traced>> {
    if sp.rooms.is_empty() && !sp.layers.iter().any(|r| r.role == Role::RoomTag) {
        return Ok(Vec::new());
    }
    if s.space().is_paper() {
        notes.push("Rooms are traced in model space; switch to the Model tab to build room ceilings.".into());
        return Ok(Vec::new());
    }
    let d = s.doc()?;
    let ents = plan_objects(d);
    let tagged = sp.layers.iter().any(|r| r.role == Role::RoomTag);
    let plan = room_plan(s)?;
    let plan = plan.as_ref().unwrap_or(s);
    let mut out: Vec<Traced> = Vec::new();
    for (h, text, at) in room_texts(sp, &ents) {
        if out.len() >= MAX_ROOMS {
            break;
        }
        let room = sp.rooms.iter().position(|r| names_room(r, &text));
        // Without room tag layers only the spec's rooms are looked for.
        if room.is_none() && !tagged {
            continue;
        }
        let Some(level) =
            room.and_then(|i| sp.rooms.get(i)).and_then(|r| r.level.as_ref()).and_then(|n| sp.level(n)).or_else(|| level_at(&sp.levels, at.x, at.y))
        else {
            continue;
        };
        let Ok(loops) = super::hatch::loops_at(plan, at) else { continue };
        let Some(outer) = loops.iter().find(|l| l.outer).or(loops.first()) else { continue };
        let outline = Polyline { vertices: outer.vertices.clone(), closed: true }.tessellate(1e-3);
        if outline.len() < 3 || cadaddy_geom::shoelace(&outline).abs() < 1e-9 {
            continue;
        }
        // A name and a number in one room: the first traced keeps it (the spec's room wins).
        // The same outline traced from another point may start at another corner: same area and
        // extents.
        let area = cadaddy_geom::shoelace(&outline).abs();
        let bounds = cadaddy_geom::Bounds2::from_points(outline.iter().copied());
        let same = |o: &Traced| {
            let b = cadaddy_geom::Bounds2::from_points(o.outline.iter().copied());
            (cadaddy_geom::shoelace(&o.outline).abs() - area).abs() <= 1e-6 * area.max(1.0)
                && b.min.dist(bounds.min) < 1e-6
                && b.max.dist(bounds.max) < 1e-6
        };
        match out.iter_mut().find(|o| same(o)) {
            Some(o) => {
                if o.room.is_none() {
                    o.room = room;
                }
            }
            None => out.push(Traced { room, level: level.name.clone(), outline, tag: h }),
        }
    }
    for (i, r) in sp.rooms.iter().enumerate() {
        if !out.iter().any(|t| t.room == Some(i)) {
            notes.push(format!("Room {}: no closed outline found around its tag; it gets the level's ceiling.", room_label(r)));
        }
    }
    Ok(out)
}

/// Build from a spec: WALLGEN's walls, openings, floors and ceilings, then the spec's own parts
/// and materials.
pub(crate) fn build_spec(s: &mut Session, g: &WallGen) -> Result<Value> {
    let Some(sp) = g.spec.clone() else { return wallgen::build(s, g) };
    let removed = wallgen::remove_made(s.doc_mut()?);
    let mut notes: Vec<String> = Vec::new();
    let rooms = trace_rooms(s, &sp, &mut notes)?;
    let mut r = wallgen::build(s, g)?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let d = s.doc_mut()?;
    let mut columns = 0;
    let mut curtain = 0;
    // Columns and curtain walls, level by level.
    let plan: Vec<Arc<Entity>> = plan_objects(d).into_iter().cloned().collect();
    let mut add: Vec<(&'static str, &'static str, String, Handle, Mesh)> = Vec::new();
    for e in &plan {
        let Some(role) = sp.role(&e.common.layer).filter(|r| matches!(r.role, Role::Column | Role::CurtainWall)) else { continue };
        let Some(c) = centre(d, e) else { continue };
        let Some(level) = level_at(&sp.levels, c.x, c.y) else { continue };
        let z0 = level.elevation_ft * upf;
        let height = sp.wall_type_of(role).and_then(|t| t.height_ft).unwrap_or(level.height_ft - 0.1).max(0.1);
        let z1 = z0 + height * upf;
        if role.role == Role::Column {
            let Some(outline) = outline_of(e) else { continue };
            if let Some(m) = prism(&outline, z0, z1) {
                add.push(("column", "3D-Columns", level.name.clone(), e.handle, m));
            }
        } else {
            let t = sp.wall_type_of(role).map_or(2.0, |t| t.thickness_in.min(2.0)) * upf / 12.0;
            let mut m = wallgen::empty_mesh();
            for sg in super::perceive::geometry::segments(&[e]) {
                wallgen::add_box(&mut m, sg.a, sg.b, t, z0, z1);
            }
            if !m.faces.is_empty() {
                add.push(("curtainwall", "3D-Windows", level.name.clone(), e.handle, m));
            }
        }
    }
    // A ceiling per room: on a level with traced rooms they replace the level's ceiling.
    let room_levels: Vec<&str> = rooms.iter().map(|t| t.level.as_str()).collect();
    let level_ceilings: Vec<Handle> = d
        .model
        .iter()
        .filter(|e| wallgen::made_as(e).as_deref() == Some("ceiling"))
        .filter(|e| made_level(e).is_some_and(|l| room_levels.iter().any(|r| r.eq_ignore_ascii_case(&l))))
        .map(|e| e.handle)
        .collect();
    for h in &level_ceilings {
        d.model.remove(*h);
    }
    let mut room_of: Vec<(Handle, usize)> = Vec::new();
    let mut ceilings = 0;
    for t in &rooms {
        let Some(level) = sp.level(&t.level) else { continue };
        let room = t.room.and_then(|i| sp.rooms.get(i));
        let z = (level.elevation_ft + room.and_then(|r| r.ceiling_ft).unwrap_or(level.ceiling_ft)) * upf;
        let Some(m) = flat(&t.outline, z) else { continue };
        let name = room.map_or(String::new(), room_label);
        let mut xdata = wallgen::tag(Part::Ceiling, &level.name, t.tag);
        xdata.push(RawTag { code: 1000, value: format!("room:{}", name.chars().take(240).collect::<String>()) });
        let common = Common { layer: wallgen::layer_of(Part::Ceiling).into(), xdata, ..Common::default() };
        if let Ok(h) = d.add(&Space::Model, common, EntityKind::Mesh(m)) {
            ceilings += 1;
            if let Some(i) = t.room {
                room_of.push((h, i));
            }
        }
    }
    // The ground: the building's extents and the margin, a slab under the site elevation.
    let mut ground = 0;
    if let Some(site) = &sp.site {
        let mut b = cadaddy_geom::Bounds2::EMPTY;
        for e in d.model.iter().filter(|e| wallgen::made_as(e).is_some()) {
            b = b.union(&cadaddy_doc::entity_bounds(d, e, 0));
        }
        for e in &plan {
            if sp.role(&e.common.layer).is_some_and(|r| r.role != Role::Ignore) {
                b = b.union(&cadaddy_doc::entity_bounds(d, e, 0));
            }
        }
        if !b.is_empty() {
            let m = site.margin_ft * upf;
            let (lo, hi) = (b.min - Vec2::new(m, m), b.max + Vec2::new(m, m));
            let z = site.elevation_ft * upf;
            let mut mesh = wallgen::empty_mesh();
            let y = (lo.y + hi.y) / 2.0;
            wallgen::add_box(&mut mesh, Vec2::new(lo.x, y), Vec2::new(hi.x, y), hi.y - lo.y, z - upf, z);
            let level = sp.levels.first().map_or(String::new(), |l| l.name.clone());
            add.push(("ground", "3D-Site", level, Handle(0), mesh));
        }
    }
    for (part, layer, level, source, m) in add {
        if d.layer(layer).is_none() {
            let color = match part {
                "column" => 8,
                "ground" => 94,
                _ => 4,
            };
            d.layers.push(cadaddy_doc::Layer { name: layer.into(), color: cadaddy_doc::color::Color::Index(color), ..cadaddy_doc::Layer::default() });
        }
        let t = |code: i32, v: String| RawTag { code, value: v };
        let xdata = vec![t(1001, wallgen::APP.into()), t(1000, part.into()), t(1000, level.chars().take(255).collect()), t(1000, source.hex())];
        if d.add(&Space::Model, Common { layer: layer.into(), xdata, ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
            match part {
                "column" => columns += 1,
                "curtainwall" => curtain += 1,
                _ => ground += 1,
            }
        }
    }
    // Stairs, ramps and their handrails.
    let (stairs, ramps, railings) = super::stairs::build(d, &sp);
    // The outside: roofs, parapets, crickets, rooftop equipment, facade glass and materials.
    let outside = super::exterior::build(d, &sp);
    // The reflected ceiling plans' ceilings (replacing the level's and rooms'), grids, soffits,
    // lights and diffusers.
    let rcp = super::ceilings::build(d, &sp);
    // Duct, pipe and conduit runs of the shown systems.
    let mep = super::mep::build(d, &sp);
    // Columns at the grid intersections.
    let grid_columns = super::grids::build(d, &sp);
    // Materials: the room's for its ceiling, else the building's for the part.
    let made: Vec<(Handle, String)> = d.model.iter().filter_map(|e| wallgen::made_as(e).map(|p| (e.handle, p))).collect();
    for (h, part) in made {
        let room = room_of.iter().find(|(x, _)| *x == h).and_then(|(_, i)| sp.rooms.get(*i));
        let Some(mat) = room.and_then(|r| r.materials.get(&part)).or_else(|| sp.materials.get(&part)) else { continue };
        let v = format!("material:{}", mat.chars().take(240).collect::<String>());
        let _ = d.modify_entity(h, |e| e.common.xdata.push(RawTag { code: 1000, value: v }));
    }
    for role in sp.layers.iter().filter(|r| (r.role == Role::Stair && sp.stairs.is_empty()) || r.role == Role::CeilingGrid) {
        if plan.iter().any(|e| sp.role(&e.common.layer).is_some_and(|x| std::ptr::eq(x, role))) {
            notes.push(if role.role == Role::Stair {
                format!("{}: stairs are drawn but the spec has none (stairs.detect finds them).", role.layer)
            } else {
                format!("{}: ceiling grids aren't built yet.", role.layer)
            });
        }
    }
    let ceilings_now = r["ceilings"].as_u64().unwrap_or(0).saturating_sub(level_ceilings.len() as u64) + ceilings;
    if let Some(n) = r["notes"].as_array_mut() {
        n.extend(notes.into_iter().map(Value::from));
    }
    // RCP ceilings replace those of their levels.
    let replaced = rcp["replacedCeilings"].as_u64().unwrap_or(0);
    r["ceilings"] = json!(ceilings_now.saturating_sub(replaced) + rcp["rcpCeilings"].as_u64().unwrap_or(0));
    r["rooms"] = json!(rooms.len());
    r["columns"] = json!(columns);
    r["curtainWalls"] = json!(curtain);
    r["ground"] = json!(ground);
    r["stairs"] = json!(stairs);
    r["ramps"] = json!(ramps);
    r["railings"] = json!(railings);
    if let (Some(o), Some(x)) = (r.as_object_mut(), outside.as_object()) {
        o.extend(x.clone());
    }
    if let (Some(o), Some(x)) = (r.as_object_mut(), rcp.as_object()) {
        o.extend(x.clone());
    }
    if let (Some(o), Some(x)) = (r.as_object_mut(), mep.as_object()) {
        o.extend(x.clone());
    }
    r["gridColumns"] = json!(grid_columns);
    r["removed"] = json!(removed);
    Ok(r)
}

/// The level a built object belongs to (its WALLGEN tag).
pub(crate) fn made_level(e: &Entity) -> Option<String> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(wallgen::APP))?;
    x.get(at + 2).filter(|t| t.code == 1000).map(|t| t.value.clone())
}

/// A column's outline: a circle (24 sides) or a closed polyline.
fn outline_of(e: &Entity) -> Option<Vec<Vec2>> {
    let pts = match &e.kind {
        EntityKind::Circle(c) if c.radius.is_finite() && c.radius > 0.0 => {
            (0..24).map(|i| c.center.xy() + Vec2::from_angle(f64::from(i) * std::f64::consts::TAU / 24.0) * c.radius).collect()
        }
        EntityKind::LwPolyline(p) if p.closed || p.vertices.first().zip(p.vertices.last()).is_some_and(|(a, b)| a.p.dist(b.p) < 1e-9) => {
            Polyline { vertices: p.vertices.clone(), closed: true }.tessellate(1e-3)
        }
        _ => return None,
    };
    let mut pts: Vec<Vec2> = pts;
    if pts.len() > 1 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.dist(*b) < 1e-9) {
        pts.pop();
    }
    (pts.len() >= 3 && pts.iter().all(|p| p.is_finite())).then_some(pts)
}

/// Triangles of a simple polygon by ear clipping (counter-clockwise result order); indexes into
/// `poly`.
pub(crate) fn ear_clip(poly: &[Vec2]) -> Vec<[usize; 3]> {
    let n = poly.len();
    if !(3..=100_000).contains(&n) {
        return Vec::new();
    }
    let mut idx: Vec<usize> = (0..n).collect();
    if cadaddy_geom::shoelace(poly) < 0.0 {
        idx.reverse();
    }
    let p = |i: usize| poly.get(i).copied().unwrap_or(Vec2::new(0.0, 0.0));
    let mut out = Vec::new();
    let mut guard = 0usize;
    while idx.len() > 3 && guard < 4 * n * n.min(2000) {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for k in 0..m {
            let (Some(&ia), Some(&ib), Some(&ic)) = (idx.get((k + m - 1) % m), idx.get(k), idx.get((k + 1) % m)) else { continue };
            let (a, b, c) = (p(ia), p(ib), p(ic));
            if (b - a).cross(c - b) <= 1e-12 {
                continue;
            }
            let inside = idx.iter().any(|&j| {
                if j == ia || j == ib || j == ic {
                    return false;
                }
                let q = p(j);
                // A point where the outline touches itself (the same place twice) is the ear's
                // own corner, not inside it.
                if q.dist(a) < 1e-9 || q.dist(b) < 1e-9 || q.dist(c) < 1e-9 {
                    return false;
                }
                (b - a).cross(q - a) >= 0.0 && (c - b).cross(q - b) >= 0.0 && (a - c).cross(q - c) >= 0.0
            });
            if !inside {
                out.push([ia, ib, ic]);
                idx.remove(k);
                clipped = true;
                break;
            }
        }
        if !clipped {
            // Degenerate (collinear or self-touching): drop a vertex rather than loop.
            idx.remove(0);
        }
    }
    if let [a, b, c] = idx[..]
        && (p(b) - p(a)).cross(p(c) - p(b)).abs() > 1e-12
    {
        out.push([a, b, c]);
    }
    out
}

/// A flat polygon at height `z`, facing down (a ceiling seen from the room).
fn flat(poly: &[Vec2], z: f64) -> Option<Mesh> {
    let tris = ear_clip(poly);
    if tris.is_empty() {
        return None;
    }
    let vertices = poly.iter().map(|q| Vec3::new(q.x, q.y, z)).collect();
    let faces = tris.iter().map(|t| MeshFace { v: [t[0], t[2], t[1]].iter().map(|&i| u32::try_from(i).unwrap_or(0)).collect(), hidden: 0 }).collect();
    Some(Mesh { vertices, faces, kind: MeshKind::Polyface })
}

/// A prism over a polygon from `z0` up to `z1`: sides and both caps, faces outward.
pub(crate) fn prism(poly: &[Vec2], z0: f64, z1: f64) -> Option<Mesh> {
    let ccw = cadaddy_geom::shoelace(poly) > 0.0;
    let ring: Vec<Vec2> = if ccw { poly.to_vec() } else { poly.iter().rev().copied().collect() };
    let tris = ear_clip(&ring);
    if tris.is_empty() || z1 <= z0 {
        return None;
    }
    let n = u32::try_from(ring.len()).ok()?;
    let mut vertices: Vec<Vec3> = ring.iter().map(|q| Vec3::new(q.x, q.y, z0)).collect();
    vertices.extend(ring.iter().map(|q| Vec3::new(q.x, q.y, z1)));
    let id = |i: usize| u32::try_from(i).unwrap_or(0);
    let mut faces: Vec<MeshFace> = Vec::new();
    for t in &tris {
        faces.push(MeshFace { v: vec![id(t[0]), id(t[2]), id(t[1])], hidden: 0 });
        faces.push(MeshFace { v: vec![id(t[0]) + n, id(t[1]) + n, id(t[2]) + n], hidden: 0 });
    }
    for i in 0..n {
        let j = (i + 1) % n;
        faces.push(MeshFace { v: vec![i, j, j + n, i + n], hidden: 0 });
    }
    Some(Mesh { vertices, faces, kind: MeshKind::Polyface })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ear_clipping_an_l() {
        // An outline touching itself at (2, 1): a notch in from the top edge and back.
        let notch = [(0.0, 0.0), (4.0, 0.0), (4.0, 2.0), (2.0, 2.0), (2.0, 1.0), (3.0, 1.5), (2.0, 2.0), (0.0, 2.0)].map(|(x, y)| Vec2::new(x, y));
        let area: f64 = ear_clip(&notch).iter().map(|t| cadaddy_geom::shoelace(&[notch[t[0]], notch[t[1]], notch[t[2]]])).sum();
        assert!((area - cadaddy_geom::shoelace(&notch)).abs() < 1e-9, "{area}");
        let l = [(0.0, 0.0), (4.0, 0.0), (4.0, 1.0), (1.0, 1.0), (1.0, 3.0), (0.0, 3.0)].map(|(x, y)| Vec2::new(x, y));
        let tris = ear_clip(&l);
        assert_eq!(tris.len(), 4);
        let area: f64 = tris.iter().map(|t| cadaddy_geom::shoelace(&[l[t[0]], l[t[1]], l[t[2]]])).sum();
        assert!((area - 6.0).abs() < 1e-9, "{area}");
        let mut cw = l;
        cw.reverse();
        let area: f64 = ear_clip(&cw).iter().map(|t| cadaddy_geom::shoelace(&[cw[t[0]], cw[t[1]], cw[t[2]]])).sum();
        assert!((area - 6.0).abs() < 1e-9, "{area}");
        assert!(ear_clip(&l[..2]).is_empty());
    }
}
