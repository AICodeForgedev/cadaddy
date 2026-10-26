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
            "{spec?: object (merged into the drawing's, not stored), replace?: bool, slabThickness?: feet (0.5), auto?: bool (default true: build again when the plan changes), stack?: bool (default the spec's `stack`: move each level's 3D onto the lowest level's plan, one stacked building from plans drawn side by side), clear?: true} → {walls, doors, windows, floors, ceilings, columns, curtainWalls, rooms (room elements built), roomsByLevel: {level: n}, ground, stairs, ramps, railings, guards, roofs, parapets, crickets, equipment, facadeGlass, rcpCeilings, grids, soffits, lights, diffusers, mepRuns, elbows, drops, fittings, terminals, mepEquipment, gridColumns, levels, stacked?: levels moved, removed, notes, warnings}",
        ),
        CommandSpec::new("bim.from_pdf", "PDF to BIM", run_bim_from_pdf).params(
            "{path: string (PDF file of any number of pages, or a folder: one print set), out?: string (.cdy to save; default a temp folder — nothing is written next to the PDFs), force?: bool (build even if import check fails)} → {cdy_paths: [string], pages: [{sheet, level, discipline, type}] (the pages the model was built from: the set's plan sheets, or every page without an index), pages_note, spec_guesses: [{key, why}] (what the spec assumed), check_results: [{file, pass (false on any hard fail), score, hard_fails, checks}], built_counts: {walls, doors, windows, ...}, warnings: [string], next_steps: [string (exact calls to make next)]}",
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
    // Building objects outside every level's region and every sheet of the set (another
    // discipline's plan of a level is that sheet's: its readers take what they need from it).
    let here: Vec<&Level> = sp.levels.iter().filter(|l| l.sheet.is_none()).collect();
    if !here.is_empty() && here.iter().all(|l| l.region.is_some()) {
        let on_sheet = |c: Vec2| {
            sp.sheets
                .iter()
                .any(|s| s.rect.is_some_and(|[x0, y0, x1, y1]| c.x >= x0.min(x1) && c.x <= x0.max(x1) && c.y >= y0.min(y1) && c.y <= y0.max(y1)))
        };
        let hs: Vec<Handle> = ents
            .iter()
            .filter(|e| sp.role(&e.common.layer).is_some_and(|r| r.role != Role::Ignore))
            .filter(|e| centre(d, e).is_some_and(|c| level_at(&sp.levels, c.x, c.y).is_none() && !on_sheet(c)))
            .map(|e| e.handle)
            .collect();
        if !hs.is_empty() {
            let why = if sp.sheets.iter().any(|s| s.rect.is_some()) {
                "on no level's plan and no sheet of the set"
            } else {
                "outside every level's region"
            };
            out.push(Issue { error: false, path: "levels".into(), message: format!("{} building objects lie {why}", hs.len()), handles: hs });
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

/// A text that can tag a room: a name (two letters or more, at most 40 characters) or a room
/// number (`101`, `B12`).
fn room_tag_text(t: &str) -> bool {
    let t = t.trim();
    (t.chars().filter(|c| c.is_alphabetic()).count() >= 2 && t.chars().count() <= 40) || super::perceive::geometry::room_number(t)
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
    let sp = s.doc()?.building_spec.as_deref().and_then(|t| BuildingSpec::from_json(t).ok());
    room_plan_for(s, sp)
}

/// [`room_plan`] with the spec given instead of the drawing's own (`bspec.draft`'s layer roles
/// before they are stored).
pub(crate) fn room_plan_for(s: &Session, sp: Option<BuildingSpec>) -> Result<Option<Session>> {
    if s.space().is_paper() {
        return Ok(None);
    }
    let d = s.doc()?;
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

/// A plan to trace rooms on from the objects that bound them, picked by handle: a plan with no
/// layer roles (a PDF-converted sheet), its walls, doors and windows as `plan.classify` reads
/// them. Everything else (MEP, stairs, tags, furniture, dimensions) is left out, so it never
/// splits a room. None: nothing picked (trace on the drawing itself).
pub(crate) fn room_plan_of(s: &Session, keep: &std::collections::HashSet<Handle>) -> Result<Option<Session>> {
    if keep.is_empty() {
        return Ok(None);
    }
    let mut wd = (*s.doc()?).clone();
    let drop: Vec<Handle> = wd.model.iter().map(|e| e.handle).filter(|h| !keep.contains(h)).collect();
    for h in drop {
        wd.model.remove(h);
    }
    let mut t = Session::empty();
    t.open_drawing(wd, "Room boundaries", None);
    Ok(Some(t))
}

/// Two traced outlines are the same room: same area and extents (a second text in it, or traced
/// from another corner).
pub(crate) fn same_outline(a: &[Vec2], b: &[Vec2]) -> bool {
    let (aa, ab) = (cadaddy_geom::shoelace(a).abs(), cadaddy_geom::shoelace(b).abs());
    let (ba, bb) = (cadaddy_geom::Bounds2::from_points(a.iter().copied()), cadaddy_geom::Bounds2::from_points(b.iter().copied()));
    let tol = 1e-6 * aa.max(ab).max(1.0);
    (aa - ab).abs() <= 1e-6 * aa.max(ab) && ba.min.dist(bb.min) < tol.sqrt() && ba.max.dist(bb.max) < tol.sqrt()
}

/// An equipment or device mark, not a room's name: one word with a digit, short (`EF-1`, `VAV2`,
/// `RTU-3A`, `P1`).
pub(crate) fn equipment_mark(t: &str) -> bool {
    let t = t.trim();
    !t.contains(char::is_whitespace) && t.chars().any(|c| c.is_ascii_digit()) && t.chars().count() <= 8
}

/// The tag naming each traced area: of the names inside it (area index, handle, text, where,
/// height), the tallest, then the one nearest the area's middle. Labels of what stands in the
/// room (an exhaust fan's name, a stair note) are smaller than the room's tag; equipment marks
/// never name a room. (area index, tag, text), in the order the areas were found.
pub(crate) fn room_tags(areas: &[Vec<Vec2>], names: &[(usize, Handle, String, Vec2, f64)]) -> Vec<(usize, Handle, String)> {
    let mut out = Vec::new();
    for (i, area) in areas.iter().enumerate() {
        let mid = cadaddy_geom::Bounds2::from_points(area.iter().copied()).center();
        let best = names.iter().filter(|n| n.0 == i && !equipment_mark(&n.2)).max_by(|a, b| {
            let h = |n: &(usize, Handle, String, Vec2, f64)| if n.4.is_finite() { n.4 } else { 0.0 };
            h(a).total_cmp(&h(b)).then_with(|| b.3.dist(mid).total_cmp(&a.3.dist(mid)))
        });
        if let Some((_, h, t, _, _)) = best {
            out.push((i, *h, t.clone()));
        }
    }
    out
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
    let mut sp = merged(s, "model.build", p.get("spec"), bool_or(p, "replace", false))?;
    if let Some(stack) = p.get("stack").and_then(Value::as_bool) {
        sp.stack = stack;
    }
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
    // BIM drawings open in 3D shaded view (SW iso + realistic), not 2D wireframe.
    let _ = s.execute("view.swiso", &json!({}));
    let _ = s.execute("vscurrent", &json!({ "style": "realistic" }));
    Ok(r)
}

// ─── Multi-plan detection ───────────────────────────────────────────────────

struct FloorLevel {
    name: String,
    region: [f64; 4],
}

/// Scan text entities for floor-level titles (e.g. "3RD FLOOR", "LEVEL 3") and return detected
/// levels with spatial regions. The texts are parsed for ordinal floor patterns ("3RD FLOOR")
/// and "LEVEL N" patterns, then split into horizontal bands.
fn detect_floor_levels(s: &Session) -> std::result::Result<Vec<FloorLevel>, ()> {
    let d = s.doc().map_err(|_| ())?;
    let ext = d.extents(&Space::Model);
    if ext.is_empty() {
        return Err(());
    }
    let (x_min, y_min, x_max, y_max) = (ext.min.x, ext.min.y, ext.max.x, ext.max.y);

    let mut hits: Vec<(u32, f64)> = Vec::new(); // (floor_number, y_position)
    for e in d.model.iter() {
        let (text, h, y) = match &e.kind {
            EntityKind::Text(t) => (t.value.as_str(), t.height, t.insert.y),
            EntityKind::MText(t) => (t.contents.as_str(), t.height, t.insert.y),
            _ => continue,
        };
        if h < 8.0 {
            continue;
        } // skip small text — titles are larger
        if let Some(num) = parse_floor_number(text)
            && !hits.iter().any(|(n, _)| *n == num)
        {
            hits.push((num, y));
        }
    }
    if hits.len() < 2 {
        return Err(());
    }

    // Sort by Y to assign horizontal bands (lower Y = lower on the sheet).
    hits.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut levels: Vec<FloorLevel> = Vec::new();
    for (i, (num, _y)) in hits.iter().enumerate() {
        let band_lo = if i == 0 { y_min } else { (hits[i - 1].1 + hits[i].1) / 2.0 };
        let band_hi = if i + 1 == hits.len() { y_max } else { (hits[i].1 + hits[i + 1].1) / 2.0 };
        levels.push(FloorLevel { name: format!("Level {num}"), region: [x_min, band_lo, x_max, band_hi] });
    }
    // Sort by floor number for spec ordering.
    levels.sort_by_key(|l| l.name.split_whitespace().last().and_then(|s| s.parse::<u32>().ok()).unwrap_or(0));
    Ok(levels)
}

/// Parse a floor number from text like "ELECTRICAL - 3RD FLOOR", "4TH FLOOR PLAN", "LEVEL 3".
fn parse_floor_number(text: &str) -> Option<u32> {
    let up = text.to_ascii_uppercase();
    let words: Vec<&str> = up.split_whitespace().collect();
    // Pattern: "<N>ST/ND/RD/TH FLOOR"
    for (i, w) in words.iter().enumerate() {
        if i + 1 < words.len() && words[i + 1].starts_with("FLOOR") {
            let num_part = w.trim_end_matches(|c: char| c.is_ascii_alphabetic());
            let suffix = &w[num_part.len()..];
            if matches!(suffix, "ST" | "ND" | "RD" | "TH")
                && let Ok(n) = num_part.parse::<u32>()
            {
                return Some(n);
            }
        }
    }
    // Pattern: "LEVEL <N>"
    for (i, w) in words.iter().enumerate() {
        if *w == "LEVEL"
            && i + 1 < words.len()
            && let Ok(n) = words[i + 1].parse::<u32>()
        {
            return Some(n);
        }
    }
    None
}

/// Where `bim.from_pdf` saves a model when no `out` is given: a folder of its own in the
/// system's temp folder, never next to the PDFs (often the job's print set). `None` on the web.
fn bim_temp_out(stem: &str) -> Option<std::path::PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let clean: String = stem.chars().take(80).map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ' | '.') { c } else { '_' }).collect();
        let dir = std::env::temp_dir().join(format!("cadaddy-bim-{}-{n}", std::process::id()));
        Some(dir.join(format!("{}.cdy", clean.trim())))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = stem;
        None
    }
}

/// The import check as the build step reports it: `pass` only when the drawing is usable AND no
/// hard check failed (the check's own `pass` means only "not empty or unreadable").
fn build_check(file: &str, check: &super::import_check::ImportCheckResult) -> Value {
    json!({
        "file": file,
        "pass": check.pass && check.hard_fails.is_empty(),
        "score": check.score,
        "hard_fails": check.hard_fails,
        "checks": check.checks,
    })
}

/// Whether a hard-failed import check stops the build: every hard fail does, except SCALE when a
/// scale note was read (correctable: the plan says its scale).
fn check_blocks_build(check: &super::import_check::ImportCheckResult) -> bool {
    check.checks.iter().any(|c| {
        if c.pass || !c.hard_fail {
            return false;
        }
        if c.id == "SCALE" {
            return c.evidence.as_ref().and_then(|e| e.get("scale_note")).is_none_or(Value::is_null);
        }
        true
    })
}

/// `bim.from_pdf`: one-call PDF → BIM through the guided job's path: the print set (one PDF of
/// any number of pages, or a folder) is classified, its plan pages imported (every page when it
/// has no plans), the import checked, the spec drafted with one stacked level per architectural
/// floor plan, the model built and saved as .cdy at `out` (default: a temp folder). Nothing is
/// written next to the PDFs. A one-sheet plan with several floor titles is stacked as levels.
fn run_bim_from_pdf(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").ok_or_else(|| bad("bim.from_pdf", "`path` is required (a PDF file or folder)"))?;
    let force = bool_or(p, "force", false);
    let path_obj = std::path::Path::new(path);
    if !path_obj.exists() {
        return Err(bad("bim.from_pdf", format!("{path}: file or folder not found")));
    }
    let is_pdf = |p: &std::path::Path| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    let has_pdfs = if path_obj.is_dir() {
        std::fs::read_dir(path_obj).is_ok_and(|entries| entries.flatten().any(|e| e.path().is_file() && is_pdf(&e.path())))
    } else {
        is_pdf(path_obj)
    };
    if !has_pdfs {
        return Err(bad("bim.from_pdf", format!("{path}: no PDF files found. Next: provide a path to a PDF file or folder.")));
    }
    let stem = path_obj.file_stem().map(|n| n.to_string_lossy().to_string()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Building".into());
    let out = match str_param(p, "out").filter(|o| !o.trim().is_empty()) {
        Some(o) => std::path::PathBuf::from(o),
        None => bim_temp_out(&stem).ok_or_else(|| bad("bim.from_pdf", "`out` is required here (the .cdy file to save)"))?,
    };

    let mut cdy_paths: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut built_counts = json!({});
    let mut guesses: Vec<Value> = Vec::new();

    // Step 1: classify the set and import its plan pages, as the guided job does.
    s.echo(format!("bim.from_pdf: reading the sheet index of {stem}..."));
    s.new_drawing_with_viewports(false);
    let plan = super::guided::load_plan_pages(s, path_obj)?;
    let names: Vec<String> = plan
        .pages
        .iter()
        .map(|pg| {
            let sheet = pg.get("sheet").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or("(no number)");
            match pg.get("level").and_then(Value::as_str).filter(|l| !l.is_empty()) {
                Some(l) => format!("{sheet} (level {l})"),
                None => sheet.to_string(),
            }
        })
        .collect();
    let pages_note = if plan.indexed == 0 {
        format!("no sheet index read; imported every page ({})", names.len())
    } else {
        format!("imported {} of {} sheets: {}", names.len(), plan.indexed, names.join(", "))
    };
    s.echo(format!("bim.from_pdf: {pages_note}"));

    // Step 2: the import check of everything imported. A hard fail is never a pass.
    let check = super::import_check::check_drawing(s.doc()?);
    let check_results = vec![build_check(path, &check)];
    let hard = check.hard_fails.join(", ");
    let blocked = !check.hard_fails.is_empty() && check_blocks_build(&check) && !force;
    if names.is_empty() {
        warnings.push(format!("{stem}: no page could be imported"));
    } else if blocked {
        warnings.push(format!(
            "{stem}: import check failed ({hard}; score {}/100); skipping build. Use force:true to build anyway, or fix the import first with: import_check {{}}",
            check.score
        ));
    } else if !check.hard_fails.is_empty() {
        let why = if force { "force=true, building anyway" } else { "SCALE mismatch with a detected scale note — proceeding" };
        warnings.push(format!("{stem}: import check failed ({hard}; score {}/100); {why}", check.score));
    }

    if !names.is_empty() && !blocked {
        // Step 3: the spec — one stacked level per architectural floor plan.
        s.echo(format!("bim.from_pdf: drafting spec for {stem}..."));
        // One plan level at most, but one sheet showing several floor titles: those are the levels.
        let floors = if plan.levels.len() <= 1 { detect_floor_levels(s).ok().filter(|f| f.len() >= 2) } else { None };
        if let Some(levels_found) = floors {
            s.execute("bspec.draft", &json!({"store": true}))?;
            let levels_json: Vec<Value> = levels_found
                .iter()
                .enumerate()
                .map(|(i, fl)| {
                    let elevation = i as f64 * 10.0; // 10' floor-to-floor default
                    json!({"name": fl.name, "elevation_ft": elevation, "height_ft": 10.0, "ceiling_ft": 8.0, "region": fl.region})
                })
                .collect();
            let _ = s.execute("bspec.set", &json!({"spec": {"levels": levels_json}}));
            s.echo(format!(
                "bim.from_pdf: detected {} levels: {}",
                levels_found.len(),
                levels_found.iter().map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ")
            ));
        } else {
            let draft = if plan.levels.is_empty() { json!({"store": true}) } else { json!({"store": true, "levels": plan.levels, "stack": true}) };
            let spec = s.execute("bspec.draft", &draft)?;
            guesses = spec.get("guesses").and_then(Value::as_array).map(|g| g.iter().take(200).cloned().collect()).unwrap_or_default();
        }

        // Where each imported sheet sits: what lies on the other sheets is theirs.
        if let Ok((mut sp, true)) = current(s, "bim.from_pdf") {
            super::guided::add_sheet_links(&mut sp, &plan.regions);
            s.doc_mut()?.building_spec = Some(sp.to_json());
        }

        // Step 4: build the 3D model.
        s.echo(format!("bim.from_pdf: building 3D model for {stem}..."));
        match s.execute("model.build", &json!({})) {
            Ok(r) => built_counts = r,
            Err(e) => warnings.push(format!("{stem}: model.build failed: {e}")),
        }

        // Step 5: save at `out` (its images go next to it).
        if built_counts.as_object().is_some_and(|o| !o.is_empty()) {
            let cdy_str = out.to_string_lossy().to_string();
            let made = out.parent().filter(|d| !d.as_os_str().is_empty()).map_or(Ok(()), std::fs::create_dir_all);
            match made.map_err(|e| e.to_string()).and_then(|()| s.execute("saveas", &json!({ "path": cdy_str })).map_err(|e| e.to_string())) {
                Ok(_) => {
                    s.echo(format!("bim.from_pdf: saved {cdy_str}"));
                    cdy_paths.push(cdy_str);
                }
                Err(e) => warnings.push(format!("{stem}: could not save .cdy: {e}")),
            }
        }
    }

    let mut next_steps: Vec<String> = Vec::new();
    if cdy_paths.is_empty() {
        next_steps.push("import_check {} — review import quality and fix issues before building".into());
        next_steps.push("bim.from_pdf {path: \"...\", force: true} — force build even with import check failures".into());
    } else {
        next_steps.push("render {} — render the 3D view to see the building".into());
        next_steps.push("bspec.set {spec: {...}} — refine the building spec (room heights, materials, etc.)".into());
        next_steps.push("model.build {} — rebuild after spec changes".into());
    }

    // Build a per-level breakdown from the notes (e.g. "Level 3: 1303 walls ...").
    let mut built: serde_json::Map<String, Value> = serde_json::Map::new();
    let levels_count = built_counts.get("levels").and_then(Value::as_u64).unwrap_or(0);
    if let Some(notes) = built_counts.get("notes").and_then(Value::as_array) {
        for note in notes {
            if let Some(text) = note.as_str() {
                // Parse "Level N: NNNN walls ..."
                if let Some(rest) = text.strip_prefix("Level ") {
                    let parts: Vec<&str> = rest.splitn(2, ':').collect();
                    if parts.len() == 2 {
                        let level_name = format!("Level {}", parts[0].trim());
                        let desc = parts[1];
                        let mut walls = 0u64;
                        // Extract wall count from "NNNN walls"
                        for w in desc.split_whitespace().collect::<Vec<&str>>().windows(2) {
                            if w[1].starts_with("wall")
                                && let Ok(n) = w[0].parse::<u64>()
                            {
                                walls = n;
                            }
                        }
                        built.insert(level_name, json!({"walls": walls}));
                    }
                }
            }
        }
    }

    Ok(json!({
        "cdy_paths": cdy_paths,
        "pages": plan.pages,
        "pages_note": pages_note,
        "spec_guesses": guesses,
        "check_results": check_results,
        "built_counts": built_counts,
        "levels": if levels_count > 1 { json!(built.keys().collect::<Vec<_>>()) } else { json!(levels_count) },
        "built": built,
        "warnings": warnings,
        "next_steps": next_steps,
    }))
}

/// A room traced on the plan: its spec entry (if any), level, outline and tag, and the name and
/// number its tag texts read.
struct Traced {
    room: Option<usize>,
    level: String,
    outline: Vec<Vec2>,
    tag: Handle,
    /// Where the tag text is (a room is traced again from here on the built walls).
    at: Vec2,
    name: Option<String>,
    number: Option<String>,
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
    let mut outline_cache = None;
    for (h, text, at) in room_texts(sp, &ents) {
        if out.len() >= MAX_ROOMS {
            break;
        }
        // A spec room on a level is looked for on that level's plan only (every floor has its
        // OFFICE).
        let here = level_at(&sp.levels, at.x, at.y);
        let room = sp
            .rooms
            .iter()
            .position(|r| names_room(r, &text) && r.level.as_ref().is_none_or(|l| here.is_some_and(|h| h.name.eq_ignore_ascii_case(l))));
        // Without room tag layers only the spec's rooms are looked for. On them, a room's name
        // or number only (not a grid bubble's letter, a note's word): a note, a sentence, a stamp
        // or a title block's line is never a room.
        let is_number = super::perceive::geometry::room_number(&text);
        let score = super::perceive::geometry::room_name_score(&text);
        if room.is_none() && (!tagged || !(room_tag_text(&text) && (is_number || (score > 0 && super::autospec::room_like_name(&text))))) {
            continue;
        }
        let Some(level) =
            room.and_then(|i| sp.rooms.get(i)).and_then(|r| r.level.as_ref()).and_then(|n| sp.level(n)).or_else(|| level_at(&sp.levels, at.x, at.y))
        else {
            continue;
        };
        let Ok(loops) = super::hatch::loops_at_lazy(plan, at, &mut outline_cache) else { continue };
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
        let (name, number) = if is_number { (None, Some(text.trim().to_string())) } else { (Some(text.trim().to_string()), None) };
        match out.iter_mut().find(|o| same(o)) {
            Some(o) => {
                if o.room.is_none() {
                    o.room = room;
                }
                // The best room label names it (a room or unit-type word first).
                let better = |n: &String| {
                    o.name.as_ref().is_none_or(|x| super::perceive::geometry::room_name_score(x) < super::perceive::geometry::room_name_score(n))
                };
                if name.as_ref().is_some_and(better) {
                    o.name = name;
                }
                o.number = o.number.take().or(number);
            }
            None => out.push(Traced { room, level: level.name.clone(), outline, tag: h, at, name, number }),
        }
    }
    for (i, r) in sp.rooms.iter().enumerate() {
        if !out.iter().any(|t| t.room == Some(i)) {
            notes.push(format!("Room {}: no closed outline found around its tag; it gets the level's ceiling.", room_label(r)));
        }
    }
    Ok(out)
}

/// The layer room elements go on (with `rooms.build`'s volumes).
const ROOM_LAYER: &str = super::rooms3d::ROOM_LAYER;
/// The share of a room's outline that must run along its level's built walls, doors and
/// windows (the rest: openings, gaps the tracing closed).
const ROOM_WALLED: f64 = 0.5;
/// Most plan footprints and grid cells looked at per level (hostile drawings).
const MAX_BOUNDS: usize = 200_000;
const MAX_CELLS: usize = 4_000_000;

/// The plan footprints of what bounds rooms in the built model (walls, curtain walls, columns,
/// doors, windows), by level (upper case): each mesh's bottom faces.
fn room_bounds(d: &Drawing) -> std::collections::BTreeMap<String, Vec<Vec<Vec2>>> {
    let mut out: std::collections::BTreeMap<String, Vec<Vec<Vec2>>> = std::collections::BTreeMap::new();
    let mut n = 0usize;
    for e in d.model.iter() {
        let Some(part) = wallgen::made_as(e) else { continue };
        if !matches!(part.as_str(), "wall" | "curtainwall" | "column" | "door" | "window" | "glass") {
            continue;
        }
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let z0 = m.vertices.iter().map(|v| v.z).filter(|z| z.is_finite()).fold(f64::INFINITY, f64::min);
        if !z0.is_finite() {
            continue;
        }
        let (z1, tol) = (m.vertices.iter().map(|v| v.z).filter(|z| z.is_finite()).fold(z0, f64::max), 1e-6);
        let tol = ((z1 - z0) * tol).max(1e-9);
        let level = made_level(e).unwrap_or_default().trim().to_ascii_uppercase();
        let polys = out.entry(level).or_default();
        for f in m.faces.iter().take(10_000) {
            let vs: Vec<Vec2> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize)).filter(|v| (v.z - z0).abs() <= tol).map(|v| v.xy()).collect();
            if vs.len() >= 3 && vs.len() == f.v.len() && vs.iter().all(|p| p.is_finite()) {
                polys.push(vs);
                n += 1;
            }
        }
        if n >= MAX_BOUNDS {
            break;
        }
    }
    out
}

/// How much of an outline (0..1, by length) runs along the footprints (within `tol`): sampled
/// along its edges, the footprints looked up in a grid of cells.
fn walled_share(outline: &[Vec2], grid: &std::collections::HashMap<(i64, i64), Vec<usize>>, polys: &[Vec<Vec2>], cell: f64, tol: f64) -> f64 {
    let perimeter: f64 = outline.iter().zip(outline.iter().cycle().skip(1)).map(|(a, b)| a.dist(*b)).sum();
    if !(perimeter.is_finite() && perimeter > 0.0 && cell > 0.0) {
        return 0.0;
    }
    let step = (perimeter / 400.0).max(tol);
    let near = |p: Vec2| {
        let key = ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
        grid.get(&key).is_some_and(|ids| {
            ids.iter()
                .filter_map(|i| polys.get(*i))
                .any(|q| cadaddy_geom::point_in_polygon(q, p) || q.iter().zip(q.iter().cycle().skip(1)).any(|(a, b)| seg_dist(p, *a, *b) <= tol))
        })
    };
    let (mut on, mut all) = (0.0f64, 0.0f64);
    for (a, b) in outline.iter().zip(outline.iter().cycle().skip(1)) {
        let len = a.dist(*b);
        if !(len.is_finite() && len > 0.0) {
            continue;
        }
        let k = ((len / step).ceil() as usize).clamp(1, 400);
        for i in 0..k {
            let p = *a + (*b - *a) * ((i as f64 + 0.5) / k as f64);
            all += len / k as f64;
            if near(p) {
                on += len / k as f64;
            }
        }
    }
    if all > 0.0 { on / all } else { 0.0 }
}

/// Smallest and largest room no spec room names (square feet): less is a grid bubble, a tag's
/// box or a piece of furniture (traced again on the built walls), more is the outside of the
/// building.
const MIN_ROOM_SF: f64 = 16.0;
const MAX_ROOM_SF: f64 = 50_000.0;
/// Most tags traced again on the built walls per build.
const MAX_RETRACE: usize = 5_000;

/// A drawing of the built footprints' edges (each once), every level's where it was built: the
/// boundaries a room is traced on when its drawn outline isn't one.
fn walls_session(bounds: &std::collections::BTreeMap<String, Vec<Vec<Vec2>>>) -> Option<Session> {
    let mut d = Drawing::default();
    let mut seen: std::collections::HashSet<[i64; 4]> = std::collections::HashSet::new();
    let key = |p: Vec2| ((p.x * 1e4).round() as i64, (p.y * 1e4).round() as i64);
    for q in bounds.values().flatten() {
        for (a, b) in q.iter().zip(q.iter().cycle().skip(1)) {
            let (ka, kb) = (key(*a), key(*b));
            if ka == kb || !seen.insert(if ka < kb { [ka.0, ka.1, kb.0, kb.1] } else { [kb.0, kb.1, ka.0, ka.1] }) {
                continue;
            }
            let line = cadaddy_doc::Line { a: Vec3::new(a.x, a.y, 0.0), b: Vec3::new(b.x, b.y, 0.0) };
            if d.add(&Space::Model, Common::default(), EntityKind::Line(line)).is_err() || seen.len() >= MAX_BOUNDS * 4 {
                break;
            }
        }
    }
    if d.model.is_empty() {
        return None;
    }
    let mut t = Session::empty();
    t.open_drawing(d, "Built walls", None);
    Some(t)
}

/// Add a room, or fold it into the same room already kept on its level (a name and a number
/// in one room): the best name, the first number, the spec's room.
fn keep_room(kept: &mut Vec<Traced>, t: Traced) {
    let lv = t.level.trim().to_ascii_uppercase();
    let Some(o) = kept.iter_mut().find(|o| o.level.trim().eq_ignore_ascii_case(&lv) && same_outline(&o.outline, &t.outline)) else {
        kept.push(t);
        return;
    };
    if o.room.is_none() {
        o.room = t.room;
    }
    let score = |n: &str| super::perceive::geometry::room_name_score(n);
    if t.name.as_deref().is_some_and(|n| o.name.as_deref().is_none_or(|x| score(x) < score(n))) {
        o.name = t.name;
    }
    if o.number.is_none() {
        o.number = t.number;
    }
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    let t = if l2 > 1e-18 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + d * t)
}

/// Keep the rooms of the building (Q5291 MUST 3, 4): on a level with built walls, a room's
/// outline runs along them (at least [`ROOM_WALLED`] of it); a box of the title block, a note's
/// frame, an enlarged view (its walls are not built) or a schedule's cell closes no room. A
/// numbered room traced twice on its level (a floor plan and an enlarged plan of it) is one
/// room: the smaller outline, the plan's own scale.
fn walled_rooms(d: &Drawing, rooms: &mut Vec<Traced>, upf: f64, notes: &mut Vec<String>) {
    if rooms.is_empty() {
        return;
    }
    let bounds = room_bounds(d);
    let tol = 3.0 * upf / 12.0;
    let cell = 4.0 * upf;
    let mut grids: std::collections::HashMap<String, std::collections::HashMap<(i64, i64), Vec<usize>>> = std::collections::HashMap::new();
    for (level, polys) in &bounds {
        let mut g: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
        let mut cells = 0usize;
        for (i, q) in polys.iter().enumerate() {
            let b = cadaddy_geom::Bounds2::from_points(q.iter().copied());
            let (x0, x1) = (((b.min.x - tol) / cell).floor() as i64, ((b.max.x + tol) / cell).floor() as i64);
            let (y0, y1) = (((b.min.y - tol) / cell).floor() as i64, ((b.max.y + tol) / cell).floor() as i64);
            let span = x1.saturating_sub(x0).saturating_add(1).saturating_mul(y1.saturating_sub(y0).saturating_add(1));
            if !(0..=10_000).contains(&span) || cells >= MAX_CELLS {
                continue;
            }
            for x in x0..=x1 {
                for y in y0..=y1 {
                    g.entry((x, y)).or_default().push(i);
                    cells += 1;
                }
            }
        }
        grids.insert(level.clone(), g);
    }
    // A tag whose outline is no room (a box around the tag, a piece of furniture, a gap the
    // lines leave open) is traced again on the level's built walls, doors and windows.
    let (min_area, max_area) = (MIN_ROOM_SF * upf * upf, MAX_ROOM_SF * upf * upf);
    let mut walls_plan: Option<Option<Session>> = None;
    let mut cache = None;
    let mut unwalled: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut retraced = 0usize;
    let mut retraced_or_tried = 0usize;
    let extents: std::collections::HashMap<&String, cadaddy_geom::Bounds2> =
        bounds.iter().map(|(l, polys)| (l, cadaddy_geom::Bounds2::from_points(polys.iter().flatten().copied()))).collect();
    let mut kept: Vec<Traced> = Vec::new();
    for mut t in std::mem::take(rooms) {
        let key = t.level.trim().to_ascii_uppercase();
        let a = cadaddy_geom::shoelace(&t.outline).abs();
        let big = a >= min_area || t.room.is_some();
        let (Some(polys), Some(grid)) = (bounds.get(&key).filter(|p| !p.is_empty()), grids.get(&key)) else {
            if big {
                keep_room(&mut kept, t);
            }
            continue;
        };
        if big && walled_share(&t.outline, grid, polys, cell, tol) >= ROOM_WALLED {
            keep_room(&mut kept, t);
            continue;
        }
        // Only a tag among the level's built walls (a schedule's or a note's text isn't).
        let inside = extents.get(&key).is_some_and(|b| b.contains(t.at)) && retraced_or_tried < MAX_RETRACE;
        let plan = walls_plan.get_or_insert_with(|| walls_session(&bounds));
        let outline = plan.as_ref().filter(|_| inside).and_then(|plan| {
            retraced_or_tried += 1;
            let loops = super::hatch::loops_at_lazy(plan, t.at, &mut cache).ok()?;
            let outer = loops.iter().find(|l| l.outer).or(loops.first())?;
            let o = Polyline { vertices: outer.vertices.clone(), closed: true }.tessellate(1e-3);
            let a = cadaddy_geom::shoelace(&o).abs();
            (o.len() >= 3 && (min_area..=max_area).contains(&a) && walled_share(&o, grid, polys, cell, tol) >= ROOM_WALLED).then_some(o)
        });
        match outline {
            Some(o) => {
                t.outline = o;
                retraced += 1;
                keep_room(&mut kept, t);
            }
            None => *unwalled.entry(t.level.clone()).or_default() += 1,
        }
    }
    *rooms = kept;
    if retraced > 0 {
        notes.push(format!("{retraced} room tag(s) traced again on the built walls (their drawn outline was a tag box or furniture, not the room)."));
    }
    for (level, n) in &unwalled {
        notes.push(format!("{level}: {n} room tag(s) not inside built walls (an enlarged view, a note's or the title block's box) — no room built."));
    }
    // One room per number and level: the smaller outline.
    let area = |t: &Traced| cadaddy_geom::shoelace(&t.outline).abs();
    let mut best: std::collections::HashMap<(String, String), usize> = std::collections::HashMap::new();
    for (i, t) in rooms.iter().enumerate() {
        let Some(n) = t.number.as_deref().map(|n| n.trim().to_ascii_uppercase()).filter(|n| !n.is_empty()) else { continue };
        let key = (t.level.trim().to_ascii_uppercase(), n);
        match best.get(&key).and_then(|j| rooms.get(*j)) {
            Some(o) if area(o) <= area(t) => {}
            _ => {
                best.insert(key, i);
            }
        }
    }
    let before = rooms.len();
    let mut i = 0usize;
    rooms.retain(|t| {
        let keep = match t.number.as_deref().map(|n| n.trim().to_ascii_uppercase()).filter(|n| !n.is_empty()) {
            Some(n) => best.get(&(t.level.trim().to_ascii_uppercase(), n)) == Some(&i),
            None => true,
        };
        i += 1;
        keep
    });
    if rooms.len() < before {
        notes.push(format!(
            "{} room(s) traced twice on their level (an enlarged plan of a room on the floor plan): one room each.",
            before - rooms.len()
        ));
    }
}

/// A floor face over a polygon at `z`, facing up.
fn floor_face(poly: &[Vec2], z: f64) -> Option<Mesh> {
    let mut ring: Vec<Vec2> = poly.to_vec();
    if ring.len() > 1 && ring.first().zip(ring.last()).is_some_and(|(a, b)| a.dist(*b) < 1e-9) {
        ring.pop();
    }
    if cadaddy_geom::shoelace(&ring) < 0.0 {
        ring.reverse();
    }
    let tris = ear_clip(&ring);
    if tris.is_empty() || !z.is_finite() {
        return None;
    }
    let vertices = ring.iter().map(|q| Vec3::new(q.x, q.y, z)).collect();
    let id = |i: usize| u32::try_from(i).unwrap_or(0);
    let faces = tris.iter().map(|t| MeshFace { v: vec![id(t[0]), id(t[1]), id(t[2])], hidden: 0 }).collect();
    Some(Mesh { vertices, faces, kind: MeshKind::Polyface })
}

/// A room element's tag value (`name:`, `number:`, `area_sf:`, `ceiling_ft:`, `finish:`).
pub(crate) fn room_tag<'a>(e: &'a Entity, key: &str) -> Option<&'a str> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(wallgen::APP))?;
    x.iter().skip(at + 4).take_while(|t| t.code != 1001).find_map(|t| t.value.strip_prefix(key)).map(str::trim)
}

/// The rooms built as model elements (part `room`), with their tags.
#[derive(Clone, Debug)]
pub(crate) struct RoomElement {
    pub handle: Handle,
    pub level: String,
    pub name: String,
    pub number: Option<String>,
    pub area_sf: f64,
    pub ceiling_ft: f64,
    pub finish: Option<String>,
    pub source: Handle,
    pub outline: Vec<Vec2>,
    pub z: f64,
}

/// The model's room elements (at most `MAX_ROOMS` × 10).
pub(crate) fn room_elements(d: &Drawing) -> Vec<RoomElement> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if wallgen::made_as(e).as_deref() != Some("room") {
            continue;
        }
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let num = |k: &str| room_tag(e, k).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite() && *v >= 0.0);
        let x = &e.common.xdata;
        let source = x
            .iter()
            .position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(wallgen::APP))
            .and_then(|at| x.get(at + 3))
            .and_then(|t| u64::from_str_radix(t.value.trim(), 16).ok())
            .map_or(Handle(0), Handle);
        out.push(RoomElement {
            handle: e.handle,
            level: made_level(e).unwrap_or_default(),
            name: room_tag(e, "name:").filter(|n| !n.is_empty()).unwrap_or("Room").to_string(),
            number: room_tag(e, "number:").filter(|n| !n.is_empty()).map(str::to_string),
            area_sf: num("area_sf:").unwrap_or(0.0),
            ceiling_ft: num("ceiling_ft:").unwrap_or(0.0),
            finish: room_tag(e, "finish:").filter(|n| !n.is_empty()).map(str::to_string),
            source,
            outline: m.vertices.iter().map(|v| v.xy()).collect(),
            z: m.vertices.first().map_or(0.0, |v| v.z),
        });
        if out.len() >= MAX_ROOMS * 10 {
            break;
        }
    }
    out
}

/// Add each traced room as a model element of its level: its floor outline (a face at the
/// level's floor, not drawn: a room is a space), tagged with its name, number, area (sq ft),
/// ceiling height (ft) and floor finish (the spec room's, else a finish code written inside
/// it). How many per level.
/// A room element's key across a rebuild: its level and its number (unnumbered, its name).
fn room_key(level: &str, name: &str, number: Option<&str>) -> (String, String) {
    let id = number.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(name.trim());
    (level.trim().to_ascii_uppercase(), id.to_ascii_uppercase())
}

/// The user edits on the model's room elements, by [`room_key`] (at most `MAX_ROOMS` × 10).
fn room_user_edits(d: &Drawing) -> std::collections::BTreeMap<(String, String), cadaddy_doc::props::Props> {
    let mut out = std::collections::BTreeMap::new();
    for e in d.model.iter().filter(|e| wallgen::made_as(e).as_deref() == Some("room")).take(MAX_ROOMS * 10) {
        let edits = cadaddy_doc::props::read(&e.common.xdata).user_edits();
        if edits.is_empty() {
            continue;
        }
        let level = made_level(e).unwrap_or_default();
        let name = room_tag(e, "name:").unwrap_or("");
        out.insert(room_key(&level, name, room_tag(e, "number:")), edits);
    }
    out
}

fn add_room_elements(
    d: &mut Drawing,
    sp: &BuildingSpec,
    rooms: &[&Traced],
    upf: f64,
    edits: &std::collections::BTreeMap<(String, String), cadaddy_doc::props::Props>,
) -> std::collections::BTreeMap<String, usize> {
    let mut by_level: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    if rooms.is_empty() {
        return by_level;
    }
    if d.layer(ROOM_LAYER).is_none() {
        d.layers.push(cadaddy_doc::Layer { name: ROOM_LAYER.into(), color: cadaddy_doc::color::Color::Index(151), ..cadaddy_doc::Layer::default() });
    }
    // The plan's texts, for the finish codes written inside a room.
    let notes: Vec<(Vec2, String)> = {
        let plan = plan_objects(d);
        let texts: Vec<&Arc<Entity>> =
            plan.into_iter().filter(|e| matches!(e.kind, EntityKind::Text(_) | EntityKind::MText(_))).take(MAX_OBJECTS).collect();
        super::perceive::texts(&texts).into_iter().map(|(_, t, at, _)| (at, t)).filter(|(at, t)| at.is_finite() && !t.is_empty()).collect()
    };
    let clip = |v: &str| v.chars().take(240).collect::<String>();
    for t in rooms {
        let Some(level) = sp.level(&t.level) else { continue };
        let room = t.room.and_then(|i| sp.rooms.get(i));
        let Some(m) = floor_face(&t.outline, level.elevation_ft * upf) else { continue };
        let name = room.map(|r| r.name.trim().to_string()).filter(|n| !n.is_empty()).or_else(|| t.name.clone()).unwrap_or_default();
        let number = room.and_then(|r| r.number.clone()).map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).or_else(|| t.number.clone());
        let name = if name.is_empty() { number.as_ref().map_or_else(|| "Room".to_string(), |n| format!("Room {n}")) } else { name };
        let area_sf = cadaddy_geom::shoelace(&t.outline).abs() / (upf * upf);
        let ceiling_ft = room.and_then(|r| r.ceiling_ft).unwrap_or(level.ceiling_ft);
        let finish = room.and_then(|r| r.materials.get("floor").map(str::to_string)).or_else(|| {
            let b = cadaddy_geom::Bounds2::from_points(t.outline.iter().copied());
            notes
                .iter()
                .filter(|(at, _)| b.contains(*at) && cadaddy_geom::point_in_polygon(&t.outline, *at))
                .flat_map(|(_, tx)| super::perceive::finish_codes(tx))
                .next()
        });
        let tg = |code: i32, v: String| RawTag { code, value: v };
        let mut xdata = vec![
            tg(1001, wallgen::APP.into()),
            tg(1000, "room".into()),
            tg(1000, clip(&level.name)),
            tg(1000, t.tag.hex()),
            tg(1000, format!("type:{}", clip(&name))),
            tg(1000, format!("name:{}", clip(&name))),
        ];
        if let Some(n) = &number {
            xdata.push(tg(1000, format!("number:{}", clip(n))));
        }
        xdata.push(tg(1000, format!("area_sf:{:.2}", area_sf)));
        xdata.push(tg(1000, format!("ceiling_ft:{:.2}", ceiling_ft)));
        if let Some(f) = &finish {
            xdata.push(tg(1000, format!("finish:{}", clip(f))));
        }
        if let Some(kept) = edits.get(&room_key(&level.name, &name, number.as_deref())) {
            xdata = cadaddy_doc::props::write(&xdata, kept);
        }
        let common = Common { layer: ROOM_LAYER.into(), visible: false, xdata: xdata.into(), ..Common::default() };
        if d.add(&Space::Model, common, EntityKind::Mesh(m)).is_ok() {
            *by_level.entry(level.name.clone()).or_default() += 1;
        }
    }
    by_level
}

/// Build from a spec: WALLGEN's walls, openings, floors and ceilings, then the spec's own parts
/// and materials.
pub(crate) fn build_spec(s: &mut Session, g: &WallGen) -> Result<Value> {
    let Some(sp) = g.spec.clone() else { return wallgen::build(s, g) };
    // What a person set on a room element (bim.props.set) goes on the rebuilt room.
    let room_edits = room_user_edits(s.doc()?);
    let removed = wallgen::remove_made(s.doc_mut()?);
    let mut notes: Vec<String> = Vec::new();
    let mut rooms = trace_rooms(s, &sp, &mut notes)?;
    let mut r = wallgen::build(s, g)?;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let d = s.doc_mut()?;
    walled_rooms(d, &mut rooms, upf, &mut notes);
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
    // Each level's built walls: a room is inside them (an outline that got out through a gap
    // runs round the sheet's border, a title block's box is no room).
    let mut walls: Vec<(String, cadaddy_geom::Bounds2)> = Vec::new();
    for e in d.model.iter().filter(|e| wallgen::made_as(e).as_deref() == Some("wall")) {
        let (Some(level), b) = (made_level(e), cadaddy_doc::entity_bounds(d, e, 0)) else { continue };
        match walls.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(&level)) {
            Some((_, x)) => *x = x.union(&b),
            None => walls.push((level, b)),
        }
    }
    let rooms: Vec<&Traced> = rooms
        .iter()
        .filter(|t| {
            // A roof plan has no rooms (its title block, a curb's box).
            if wallgen::roof_level(&sp.levels, &t.level) {
                return false;
            }
            let b = cadaddy_geom::Bounds2::from_points(t.outline.iter().copied());
            let inside = walls.iter().find(|(n, _)| n.eq_ignore_ascii_case(&t.level)).is_none_or(|(_, w)| {
                let w = w.expand(upf);
                b.min.x >= w.min.x && b.min.y >= w.min.y && b.max.x <= w.max.x && b.max.y <= w.max.y
            });
            if !inside && let Some(r) = t.room.and_then(|i| sp.rooms.get(i)) {
                notes.push(format!(
                    "Room {}: its outline runs outside {}'s walls (a gap in them?), so it gets no ceiling of its own.",
                    room_label(r),
                    t.level
                ));
            }
            inside
        })
        .collect();
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
        let common = Common { layer: wallgen::layer_of(Part::Ceiling).into(), xdata: xdata.into(), ..Common::default() };
        if let Ok(h) = d.add(&Space::Model, common, EntityKind::Mesh(m)) {
            ceilings += 1;
            if let Some(i) = t.room {
                room_of.push((h, i));
            }
        }
    }
    // Each room as a model element of its level (a space: its floor outline, name, number,
    // area, ceiling and floor finish), what bim.tree, the takeoff and the room commands list.
    let room_elements = add_room_elements(d, &sp, &rooms, upf, &room_edits);
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
        if d.add(&Space::Model, Common { layer: layer.into(), xdata: xdata.into(), ..Common::default() }, EntityKind::Mesh(m)).is_ok() {
            match part {
                "column" => columns += 1,
                "curtainwall" => curtain += 1,
                _ => ground += 1,
            }
        }
    }
    // Stairs, ramps, their handrails and the guards on their landings.
    let built_stairs = super::stairs::build(d, &sp);
    // The outside: roofs, parapets, crickets, rooftop equipment, facade glass and materials.
    let outside = super::exterior::build(d, &sp);
    // The reflected ceiling plans' ceilings (replacing the level's and rooms'), grids, soffits,
    // lights and diffusers.
    let rcp = super::ceilings::build(d, &sp);
    // Duct, pipe and conduit runs of the shown systems.
    let mep = super::mep::build(d, &sp);
    // The devices the readers found, each an element with what was read about it.
    let devices = super::propfill::build_devices(d, &sp);
    // Concrete: slabs, footings, pads, bases and encasement, with rebar and what is cast in.
    let concrete = super::concrete::build(d, &sp);
    // Takeoff tables linked to the model count it again.
    let qty_tables = super::mepqty::refresh(d, &sp);
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
    r["rooms"] = json!(room_elements.values().sum::<usize>());
    r["roomsByLevel"] = json!(room_elements);
    r["columns"] = json!(columns);
    r["curtainWalls"] = json!(curtain);
    r["ground"] = json!(ground);
    r["stairs"] = json!(built_stairs.stairs);
    r["ramps"] = json!(built_stairs.ramps);
    r["railings"] = json!(built_stairs.railings);
    r["guards"] = json!(built_stairs.guards);
    if let (Some(o), Some(x)) = (r.as_object_mut(), outside.as_object()) {
        o.extend(x.iter().filter(|(k, _)| *k != "roofNotes").map(|(k, v)| (k.clone(), v.clone())));
    }
    if let (Some(n), Some(x)) = (r["notes"].as_array_mut(), outside["roofNotes"].as_array()) {
        n.extend(x.iter().cloned());
    }
    if let (Some(o), Some(x)) = (r.as_object_mut(), rcp.as_object()) {
        o.extend(x.clone());
    }
    if let (Some(o), Some(x)) = (r.as_object_mut(), mep.as_object()) {
        o.extend(x.clone());
    }
    if let (Some(o), Some(x)) = (r.as_object_mut(), concrete.as_object()) {
        o.extend(x.clone());
    }
    r["gridColumns"] = json!(grid_columns);
    r["deviceElements"] = json!(devices);
    r["qtyTables"] = json!(qty_tables);
    r["removed"] = json!(removed);
    // One building: the levels' 3D moved off their plans onto the lowest level's.
    if sp.stack {
        r["stacked"] = json!(stack_levels(d, &sp));
    }
    Ok(r)
}

/// Where a level's plan sits on the sheet: its base, else the lower-left corner of the walls
/// built from it, else its region's.
fn level_anchor(d: &Drawing, level: &Level) -> Option<Vec2> {
    if let Some([x, y]) = level.base {
        return Some(Vec2::new(x, y));
    }
    let mut b = cadaddy_geom::Bounds2::EMPTY;
    for e in d.model.iter().filter(|e| wallgen::made_as(e).as_deref() == Some("wall")) {
        if made_level(e).is_some_and(|l| l.eq_ignore_ascii_case(&level.name)) {
            b = b.union(&cadaddy_doc::entity_bounds(d, e, 0));
        }
    }
    if !b.is_empty() && b.min.x.is_finite() && b.min.y.is_finite() {
        return Some(b.min);
    }
    level.region.map(|[x0, y0, x1, y1]| Vec2::new(x0.min(x1), y0.min(y1)))
}

/// Move each level's built objects (by their level tag) from over its own plan onto the lowest
/// level's plan, so plans drawn side by side make one stacked building; the plans stay put.
/// How many levels were moved.
fn stack_levels(d: &mut Drawing, sp: &BuildingSpec) -> usize {
    let mut here: Vec<&Level> = sp.levels.iter().filter(|l| l.sheet.is_none()).collect();
    here.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    let Some(lowest) = here.first() else { return 0 };
    let Some(origin) = level_anchor(d, lowest) else { return 0 };
    // Each level's move: its plan's corner onto the lowest's. A roof plan (no walls of its own)
    // is centred over the walls of the storey under it, wherever that went: a roof edge runs a
    // slab's overhang past the walls, so the corners don't match.
    let mut offsets: Vec<(&str, Vec2)> = vec![(lowest.name.as_str(), Vec2::ZERO)];
    for level in here.iter().skip(1) {
        let roof = (level.base.is_none() && wallgen::roof_level(&sp.levels, &level.name))
            .then(|| super::exterior::built_bounds(d, &level.name, &["roof", "parapet"]))
            .filter(|b| !b.is_empty());
        let under = roof.and_then(|r| {
            offsets.iter().rev().find_map(|(n, off)| {
                let w = super::exterior::built_bounds(d, n, &["wall"]);
                (!w.is_empty()).then(|| w.center() + *off - r.center())
            })
        });
        let off = match under {
            Some(off) => off,
            None => {
                let Some(at) = level_anchor(d, level) else { continue };
                origin - at
            }
        };
        offsets.push((level.name.as_str(), off));
    }
    let mut moved = 0;
    for (name, off) in offsets.iter().skip(1) {
        let (level, off) = (*name, *off);
        if !(off.x.is_finite() && off.y.is_finite()) || off.len() < 1e-9 {
            continue;
        }
        let m = cadaddy_geom::Mat3::translate(off);
        let mine: Vec<Handle> = d
            .model
            .iter()
            .filter(|e| wallgen::made_as(e).is_some() && made_level(e).is_some_and(|l| l.eq_ignore_ascii_case(level)))
            .map(|e| e.handle)
            .take(MAX_OBJECTS)
            .collect();
        for h in &mine {
            let _ = d.modify_entity(*h, |e| {
                e.kind.transform(&m);
                e.common.xdata = with_stacked(&e.common.xdata, off).into();
            });
        }
        if !mine.is_empty() {
            moved += 1;
        }
    }
    moved
}

/// WALLGEN's tag key for how far `model.build` moved a level's object to stack it
/// (`stacked:<dx>,<dy>`, drawing units): views and exports that stack levels by the plans' base
/// points take it off, so a stacked object never moves twice.
pub(crate) const STACKED: &str = "stacked";

/// `xdata` with the stacking move `off` recorded (added to one recorded before) at the end of
/// WALLGEN's tags.
pub(crate) fn with_stacked(xdata: &[RawTag], off: Vec2) -> Vec<RawTag> {
    let mut x = xdata.to_vec();
    let Some(at) = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(wallgen::APP)) else { return x };
    let end = x.iter().skip(at + 1).position(|t| t.code == 1001).map_or(x.len(), |i| at + 1 + i);
    let key = format!("{STACKED}:");
    let old = x.get(at + 1..end).and_then(|g| g.iter().position(|t| t.code == 1000 && t.value.starts_with(&key))).map(|i| at + 1 + i);
    let before = old.and_then(|i| x.get(i)).and_then(|t| parse_stacked(t.value.get(key.len()..).unwrap_or(""))).unwrap_or(Vec2::ZERO);
    let total = before + off;
    let tag = RawTag { code: 1000, value: format!("{key}{},{}", total.x, total.y) };
    match old.and_then(|i| x.get_mut(i)) {
        Some(t) => *t = tag,
        None => x.insert(end, tag),
    }
    x
}

fn parse_stacked(v: &str) -> Option<Vec2> {
    let (a, b) = v.split_once(',')?;
    let p = Vec2::new(a.trim().parse().ok()?, b.trim().parse().ok()?);
    p.is_finite().then_some(p)
}

/// How far `model.build` moved a built object to stack its level (zero for one it didn't).
pub(crate) fn stacked_by(e: &Entity) -> Vec2 {
    super::bimlook::made_tag(e, STACKED).and_then(parse_stacked).unwrap_or(Vec2::ZERO)
}

/// The level a built object belongs to (its WALLGEN tag).
pub fn made_level(e: &Entity) -> Option<String> {
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
