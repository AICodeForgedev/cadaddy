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
            "{spec?: object (merged into the drawing's, not stored), replace?: bool, slabThickness?: feet (0.5), auto?: bool (default true: build again when the plan changes), stack?: bool (default the spec's `stack`: move each level's 3D onto the lowest level's plan, one stacked building from plans drawn side by side), clear?: true} → {walls, doors, windows, floors, ceilings, columns, curtainWalls, rooms, ground, stairs, ramps, railings, guards, roofs, parapets, crickets, equipment, facadeGlass, rcpCeilings, grids, soffits, lights, diffusers, mepRuns, elbows, drops, fittings, terminals, mepEquipment, gridColumns, levels, stacked?: levels moved, removed, notes, warnings}",
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
    let mut outline_cache = None;
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
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
        // Without room tag layers only the spec's rooms are looked for; on them, names and
        // numbers (not a grid bubble's letter, a note's word).
        if room.is_none() && (!tagged || !room_tag_text(&text)) {
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
        // A room no spec room names is 16 sq ft or more (not a grid bubble, a tag's box).
        if room.is_none() && cadaddy_geom::shoelace(&outline).abs() < 16.0 * upf * upf {
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
    r["rooms"] = json!(rooms.len());
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
fn with_stacked(xdata: &[RawTag], off: Vec2) -> Vec<RawTag> {
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
