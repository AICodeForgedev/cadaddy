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
const MAX_OBJECTS: usize = 4_000_000;

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
            "{spec?: object (merged into the drawing's, not stored), replace?: bool, slabThickness?: feet (0.5), auto?: bool (default true: build again when the plan changes), stack?: bool (default the spec's `stack`: move each level's 3D onto the lowest level's plan, one stacked building from plans drawn side by side), clear?: true, budget_s?: seconds (1200: stop with an error naming the step when the build runs longer), echo?: bool (default true: the counts on the command line; a caller that changes the model after the build says its own), route?: bool (default true, kept with the drawing: route the circuits through the model, Q6152; conduit.route reports it)} → {walls, doors, windows, floors, ceilings, columns, curtainWalls, rooms (room elements built), roomsByLevel: {level: n}, ground, stairs, ramps, railings, guards, roofs, parapets, crickets, equipment, facadeGlass, rcpCeilings, grids, soffits, lights, diffusers, mepRuns, elbows, drops, fittings, terminals, mepEquipment, gridColumns, conduitRoute: {circuits, runs, homeruns, routed_ft, clashes, conflicts, penetrations}, levels, stacked?: levels moved, removed, notes, warnings}",
        ),
        CommandSpec::new("bim.from_pdf", "PDF to BIM", run_bim_from_pdf).params(
            "{path: string (PDF file of any number of pages, or a folder: one print set), out?: string (.cdy to save; default a temp folder — nothing is written next to the PDFs), force?: bool (save the model even when the run failed)} → {cdy_paths: [string], pages: [{sheet, level, discipline, type}] (the plan sheets the job built from), pages_note, spec_guesses, check_results: [{file, pass (false on any hard fail), score, hard_fails, checks}], built_counts: {wall, door, window, ...} (elements built), built: {level: {part: n}}, levels, job: {job_id, status, done, findings} (the guided job that ran: job.status / job.report say more), ok, failed: [why the run failed: the job didn't finish, a hard import-check fail, a storey that came out empty], warnings: [string], next_steps: [string (exact calls to make next)]}",
        ),
    ]
}

/// The drawing's stored spec, if it has one.
fn stored(d: &Drawing) -> std::result::Result<Option<BuildingSpec>, String> {
    d.building_spec.as_deref().filter(|t| !t.trim().is_empty()).map(BuildingSpec::from_json).transpose()
}

/// What to do about a stored spec that can't be read (a newer version, or the wrong shape).
const UNREADABLE_HINT: &str = "the drawing's stored building spec can't be read; bspec.set {spec, replace: true} replaces it";

/// The drawing's spec, else one drafted from the walk rules; whether it was stored.
pub(crate) fn current(s: &Session, cmd: &str) -> Result<(BuildingSpec, bool)> {
    match stored(s.doc()?).map_err(|e| bad(cmd, format!("{e} — {UNREADABLE_HINT}")))? {
        Some(sp) => Ok((sp, true)),
        None => Ok((BuildingSpec::from_rules(&wallgen::rules_for(s, &json!({}))?), false)),
    }
}

/// The current spec with `patch` merged in (or `patch` alone with `replace`).
/// With `replace` the stored spec is never read, so an unreadable one can still be replaced.
fn merged(s: &Session, cmd: &str, patch: Option<&Value>, replace: bool) -> Result<BuildingSpec> {
    let Some(patch) = patch.filter(|v| !v.is_null()) else { return Ok(current(s, cmd)?.0) };
    if !patch.is_object() {
        return Err(bad(cmd, "`spec` must be an object"));
    }
    let mut v = if replace { json!({}) } else { serde_json::to_value(current(s, cmd)?.0).map_err(|e| bad(cmd, e.to_string()))? };
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
    room_texts_sized(sp, ents).into_iter().map(|(h, t, at, _)| (h, t, at)).collect()
}

/// [`room_texts`] with each text's height.
fn room_texts_sized(sp: &BuildingSpec, ents: &[&Arc<Entity>]) -> Vec<(Handle, String, Vec2, f64)> {
    let tagged = sp.layers.iter().any(|r| r.role == Role::RoomTag);
    let on: Vec<&Arc<Entity>> =
        ents.iter().copied().filter(|e| !tagged || sp.role(&e.common.layer).is_some_and(|r| r.role == Role::RoomTag)).collect();
    let mut read = super::perceive::texts(&on);
    super::perceive::geometry::lift_qualifiers(&mut read);
    let mut out: Vec<(Handle, String, Vec2, f64)> = read.into_iter().filter(|(_, t, at, _)| !t.is_empty() && at.is_finite()).collect();
    // Room tag blocks: their name and number attributes, as the rooms stage reads them.
    for e in &on {
        let EntityKind::Insert(i) = &e.kind else { continue };
        for a in &i.attribs {
            let v = a.text.value.trim();
            let at = a.text.justify_point();
            if !v.is_empty() && v.chars().count() <= 40 && at.is_finite() && super::perceive::geometry::tag_attribute(&a.tag).is_some() {
                out.push((e.handle, v.to_string(), at, a.text.height));
            }
        }
    }
    out
}

/// How far a plan room tag's text height may be from the plan's room labels' (their median):
/// the same tag style, not a note, a title or an OCR guess drawn at the size it read.
const TAG_HEIGHT_BAND: (f64, f64) = (0.75, 1.33);

/// The plan's room tag style, from the labels that name the spec's rooms: their middle text
/// height and the shapes of the spec's room numbers (`101` → `999`, `B12` → `A99`).
#[derive(Clone, Debug, PartialEq)]
struct TagStyle {
    height: f64,
    number_shapes: std::collections::HashSet<String>,
}

/// A room number's shape: each digit `9`, each letter `A`.
fn number_shape(t: &str) -> String {
    t.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_digit() {
                '9'
            } else if c.is_alphabetic() {
                'A'
            } else {
                c
            }
        })
        .collect()
}

/// [`TagStyle`] of the labels naming the spec's rooms; None without three of them.
fn tag_height(sp: &BuildingSpec, texts: &[(Handle, String, Vec2, f64)]) -> Option<TagStyle> {
    let mut hs: Vec<f64> = texts
        .iter()
        .filter(|(_, t, _, h)| h.is_finite() && *h > 0.0 && room_tag_text(t) && sp.rooms.iter().any(|r| names_room(r, t)))
        .map(|(_, _, _, h)| *h)
        .collect();
    if hs.len() < 3 {
        return None;
    }
    hs.sort_by(f64::total_cmp);
    let number_shapes = sp.rooms.iter().filter_map(|r| r.number.as_deref()).filter(|n| !n.trim().is_empty()).map(number_shape).collect();
    hs.get(hs.len() / 2).map(|&height| TagStyle { height, number_shapes })
}

/// Whether a text the spec's rooms don't name is still a room's tag on a plan without room tag
/// layers: a room number shaped as the spec's (a door's `101A` is not), or a name with a room
/// word, in the plan's room tag size.
fn plan_room_tag(text: &str, height: f64, style: Option<&TagStyle>) -> bool {
    let Some(st) = style else { return false };
    let sized = height.is_finite() && height >= TAG_HEIGHT_BAND.0 * st.height && height <= TAG_HEIGHT_BAND.1 * st.height;
    let geo = |t: &str| super::perceive::geometry::room_name_score(t);
    let number = super::perceive::geometry::room_number(text) && st.number_shapes.contains(&number_shape(text));
    sized && room_tag_text(text) && (number || (geo(text) >= 3 && super::autospec::room_like_name(text)))
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
    let generated = |e: &Entity| wallgen::made_as(e).is_some() || super::rooms3d::room_of(&e.common).is_some();
    // Wall layers the drawing doesn't use (a default spec's `*WALL*` on a PDF import, everything
    // on layer 0) decide nothing: kept, they would leave no object to bound a room.
    let sp = sp.filter(|sp| sp.layers.iter().any(|r| matches!(r.role, Role::Wall | Role::CurtainWall))).filter(|sp| {
        d.model.iter().any(|e| !generated(e) && sp.role(&e.common.layer).is_some_and(|r| matches!(r.role, Role::Wall | Role::CurtainWall)))
    });
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
    // Its switches are true or false and its budget whole seconds: a wrong one is refused, not
    // read as the default.
    for key in ["replace", "auto", "stack", "clear", "echo", "route"] {
        bool_param("model.build", p, key)?;
    }
    // Routing the circuits (Q6152) is on unless switched off; the switch stays with the drawing.
    if let Some(on) = p.get("route").and_then(Value::as_bool) {
        super::conduitroute::set_enabled(s.doc_mut()?, on);
    }
    let budget = count_param("model.build", p, "budget_s", 1, MAX_BUILD_BUDGET_S)?;
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
    let budget_s = budget.unwrap_or(BUILD_BUDGET_S);
    let g = WallGen { rules: sp.to_rules(), auto: bool_or(p, "auto", true), slab_ft, layers: None, spec: Some(sp) };
    let mut r = build_spec_with_budget(s, &g, budget_s)?;
    r["warnings"] = json!(issues.iter().filter(|i| !i.error).map(issue_json).collect::<Vec<_>>());
    // The counts are of what the model holds when the build is done (steps after the walls
    // took some away again: the summary said 2,977 walls of 2,794 built).
    recount_built(s.doc()?, &mut r);
    let recording = web_time::Instant::now();
    super::revise3d::record(s, &g);
    super::guided::progress_line(&format!("  model.build: plan recorded in {:.1} s", recording.elapsed().as_secs_f64()));
    s.state_mut()?.wallgen = Some(g);
    if super::bool_param("model.build", p, "echo")?.unwrap_or(true) {
        s.echo(format!(
            "3D building from the spec: {} walls, {} doors, {} windows, {} columns, {} curtain walls, {} floors, {} ceilings ({} rooms) on {} level(s).",
            r["walls"], r["doors"], r["windows"], r["columns"], r["curtainWalls"], r["floors"], r["ceilings"], r["rooms"], r["levels"]
        ));
    }
    // BIM drawings open in 3D shaded view (SW iso + realistic), not 2D wireframe.
    let viewing = web_time::Instant::now();
    let _ = s.execute("view.swiso", &json!({}));
    let _ = s.execute("vscurrent", &json!({ "style": "realistic" }));
    super::guided::progress_line(&format!("  model.build: 3D view set in {:.1} s", viewing.elapsed().as_secs_f64()));
    Ok(r)
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

/// `bim.from_pdf`: one-call PDF → BIM: the guided job (`job.start {goal: bim, auto: true}`) on
/// the print set (one PDF of any number of pages, or a folder), the import checked, and the model
/// saved as .cdy at `out` (default: a temp folder). Nothing is written next to the PDFs. A job
/// that didn't finish, a hard import-check fail or a storey that came out empty is a failure
/// (`ok: false`, named in `failed`; the CLI exits non-zero), not saved unless `force`.
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

    // The guided job's pipeline, unattended: the same sheet index, plan regions, rooms,
    // circulation, devices, MEP and build as job.start (B7-004: the old level-region reader
    // built a broken model of a real set and called it a success).
    s.echo(format!("bim.from_pdf: running the job on {stem}..."));
    s.new_drawing_with_viewports(false);
    let status = s.execute("job.start", &json!({"pdfs": path, "goal": "bim", "auto": true, "checkpoint": false, "resume": false}))?;
    let mut warnings: Vec<String> = Vec::new();
    // What makes the run a failure (the CLI exits non-zero on any).
    let mut failed: Vec<String> = Vec::new();
    if status.get("done").and_then(Value::as_bool) != Some(true) {
        let why = status.get("error").and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or("it stopped before the build");
        failed.push(format!("{stem}: the job didn't finish: {why}"));
    }
    // The import check of everything imported: a hard fail is never a pass.
    let check = super::import_check::check_drawing(s.doc()?);
    let check_results = vec![build_check(path, &check)];
    if !check.hard_fails.is_empty() {
        failed.push(format!("{stem}: import check failed ({}; score {}/100)", check.hard_fails.join(", "), check.score));
    }
    // What was built, per level: a storey with no walls came out empty.
    let mut built: std::collections::BTreeMap<String, std::collections::BTreeMap<String, u64>> = std::collections::BTreeMap::new();
    let mut totals: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for e in s.doc()?.model.iter() {
        let (Some(part), Some(level)) = (wallgen::made_as(e), made_level(e)) else { continue };
        *built.entry(level).or_default().entry(part.clone()).or_default() += 1;
        *totals.entry(part).or_default() += 1;
    }
    if let Ok((sp, true)) = current(s, "bim.from_pdf") {
        for l in sp.levels.iter().filter(|l| l.sheet.is_none() && !wallgen::roof_level(&sp.levels, &l.name)) {
            if built.get(&l.name).and_then(|b| b.get("wall")).copied().unwrap_or(0) == 0 {
                failed.push(format!("{stem}: {} came out empty (no walls built)", l.name));
            }
        }
    }
    let pages = super::guided::job_plan_pages(s);
    let names: Vec<String> = pages
        .iter()
        .map(|pg| {
            let sheet = pg.get("sheet").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or("(no number)");
            match pg.get("level").and_then(Value::as_str).filter(|l| !l.is_empty()) {
                Some(l) => format!("{sheet} (level {l})"),
                None => sheet.to_string(),
            }
        })
        .collect();
    let pages_note = format!("built from {} sheet(s): {}", names.len(), names.join(", "));

    // Saved at `out` (its images go next to it) when anything was built; `force` saves a run
    // that failed too.
    let mut cdy_paths: Vec<String> = Vec::new();
    if !totals.is_empty() && (failed.is_empty() || force) {
        let cdy_str = out.to_string_lossy().to_string();
        let made = out.parent().filter(|d| !d.as_os_str().is_empty()).map_or(Ok(()), std::fs::create_dir_all);
        match made.map_err(|e| e.to_string()).and_then(|()| s.execute("saveas", &json!({ "path": cdy_str })).map_err(|e| e.to_string())) {
            Ok(_) => {
                s.echo(format!("bim.from_pdf: saved {cdy_str}"));
                cdy_paths.push(cdy_str);
            }
            Err(e) => failed.push(format!("{stem}: could not save .cdy: {e}")),
        }
    } else if !failed.is_empty() {
        warnings.push(format!("{stem}: not saved (force: true saves it anyway)"));
    }
    warnings.splice(0..0, failed.iter().cloned());

    let next_steps: Vec<String> = if cdy_paths.is_empty() {
        vec![
            "job.status {} — the job's stages, findings and the questions it took defaults for".into(),
            "import_check {} — review import quality".into(),
            "bim.from_pdf {path: \"...\", force: true} — save the model even though the run failed".into(),
        ]
    } else {
        vec![
            "render {} — render the 3D view to see the building".into(),
            "job.report {} — what the job read and assumed".into(),
            "bspec.set {spec: {...}} then model.build {} — refine the spec and rebuild".into(),
        ]
    };
    Ok(json!({
        "cdy_paths": cdy_paths,
        "pages": pages,
        "pages_note": pages_note,
        "links": status.get("links"),
        "spec_guesses": [],
        "check_results": check_results,
        "built_counts": totals,
        "levels": built.keys().collect::<Vec<_>>(),
        "built": built,
        "job": {"job_id": status.get("job_id"), "status": status.get("status"), "done": status.get("done"), "findings": status.get("findings")},
        "ok": failed.is_empty(),
        "failed": failed,
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
    /// Cached area and bounds for fast duplicate detection (Q6029).
    cached_area: f64,
    cached_bounds: cadaddy_geom::Bounds2,
    /// A corridor, stair or hoistway of the spec's circulation: its outline is its traced floor.
    circulation: bool,
    /// Traced on its storey's floor grid (inside the walls themselves), as circulation is.
    on_floor: bool,
    /// Which room of its storey's floor grid it is (its pockets join it after the trace).
    floor_room: Option<usize>,
}

/// Grid cell size for the spatial index of traced rooms (Q6029): two rooms are the same only when
/// their bounds and area match, so coarse cells (1000 units, wider than any single room on a
/// typical plan) still cut the search space by orders of magnitude.
const ROOM_GRID_CELL: f64 = 1000.0;

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
    // On the levels' own plans only: a whole print set side by side is so large its trace
    // tolerance (a fraction of the extents) opens every room.
    let regions: Vec<[f64; 4]> = sp.levels.iter().filter_map(|l| l.region).filter(|r| r.iter().all(|v| v.is_finite() && v.abs() <= 1.0e8)).collect();
    let plan = if regions.is_empty() {
        room_plan(s)?
    } else {
        let inside =
            |c: Vec2| regions.iter().any(|r| c.x >= r[0].min(r[2]) && c.x <= r[0].max(r[2]) && c.y >= r[1].min(r[3]) && c.y <= r[1].max(r[3]));
        let keep: std::collections::HashSet<Handle> =
            plan_objects(d).into_iter().filter(|e| centre(d, e).is_some_and(inside)).map(|e| e.handle).collect();
        // Those objects as `room_plan` makes a plan of them: room-bounding layers only, each
        // centreline wall as its two faces.
        match room_plan_of(s, &keep)? {
            Some(t) => match room_plan_for(&t, Some(sp.clone()))? {
                Some(faced) => Some(faced),
                None => Some(t),
            },
            None => None,
        }
    };
    // The corridors, stairs and hoistways traced before the rooms: their edges bound the rooms.
    let mut plan = plan;
    if let Some(p) = plan.as_mut() {
        add_circulation_edges(p.doc_mut()?, sp);
    }
    let plan = plan.as_ref().unwrap_or(s);
    let mut out: Vec<Traced> = Vec::new();
    // Q6029: spatial index for O(1) amortised duplicate detection instead of O(n) linear scan.
    // Key: (grid_x, grid_y) of the outline's bounds centre; value: indices into `out`.
    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> = std::collections::HashMap::new();
    let grid_key = |b: &cadaddy_geom::Bounds2| {
        let c = b.center();
        ((c.x / ROOM_GRID_CELL).floor() as i64, (c.y / ROOM_GRID_CELL).floor() as i64)
    };
    let mut outline_cache = None;
    // Each storey's floor grid, made the first time one of its rooms is traced.
    let mut floors: std::collections::HashMap<String, Option<wallgen::Floors>> = std::collections::HashMap::new();
    let mut floor_misses: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut missed_labels: std::collections::BTreeMap<String, Vec<Vec2>> = std::collections::BTreeMap::new();
    // Names first, then numbers: a number inside a room already traced from its name numbers that
    // room (a guest room's number and its unit type sit in different parts of it, and traced from
    // each the room came out twice).
    let mut claimed: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut texts = room_texts_sized(sp, &ents);
    // Untagged, the plan's room tag size: a number or room name in it that the spec's rooms
    // don't list is still a room's tag (Q6160: a closet tagged with its number only).
    // Measured on the levels' own plans: the other trades' sheets print the same rooms' numbers
    // at their own sizes.
    let tag_h = if tagged {
        None
    } else {
        let on_plans: Vec<(Handle, String, Vec2, f64)> = texts
            .iter()
            .filter(|(_, _, at, _)| {
                regions.is_empty()
                    || regions.iter().any(|r| at.x >= r[0].min(r[2]) && at.x <= r[0].max(r[2]) && at.y >= r[1].min(r[3]) && at.y <= r[1].max(r[3]))
            })
            .cloned()
            .collect();
        tag_height(sp, &on_plans)
    };
    // A room another trade's plan tags (none of the plans' labels does): traced from its tag.
    for r in &sp.rooms {
        if let (Some(at), Some(n)) = (r.at, r.number.as_deref().filter(|n| !n.trim().is_empty())) {
            texts.push((Handle(0), n.trim().to_string(), Vec2::new(at[0], at[1]), f64::NAN));
        }
    }
    texts.sort_by_key(|(_, t, _, _)| super::perceive::geometry::room_number(t));
    let spec_number = |t: &str| sp.rooms.iter().any(|r| r.number.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(t.trim())));
    // The plans' room numbers, for a room left without one after the trace.
    let number_tags: Vec<(String, Vec2)> = texts
        .iter()
        .filter(|(_, t, _, _)| super::perceive::geometry::room_number(t))
        .filter(|(_, t, _, h)| tagged || spec_number(t) || plan_room_tag(t, *h, tag_h.as_ref()))
        .map(|(_, t, at, _)| (t.trim().to_string(), *at))
        .take(MAX_ROOMS * 10)
        .collect();
    // The plans' room names (a unit type's label in every room of it), in the plan's tag size, for
    // a room the spec knows by number alone.
    let name_labels: Vec<(String, Vec2)> = texts
        .iter()
        .filter(|(_, t, _, h)| {
            !super::perceive::geometry::room_number(t)
                && room_tag_text(t)
                && super::perceive::geometry::room_name_score(t) > 0
                && super::autospec::room_like_name(t)
                && tag_h.as_ref().is_none_or(|st| h.is_finite() && *h >= TAG_HEIGHT_BAND.0 * st.height && *h <= TAG_HEIGHT_BAND.1 * st.height)
        })
        .map(|(_, t, at, _)| (t.trim().to_string(), *at))
        .take(MAX_ROOMS * 10)
        .collect();
    // Numbers that landed in a room already numbered (its own walls drawn but its door gap wider
    // than the floor trace closes): each traces its inner walls after the labels (Q6160).
    let mut carve: Vec<(String, Vec2)> = Vec::new();
    // The corridors, stairs and hoistways first: each is the room its traced floor is, the spec's
    // room of its name on its level its own.
    for c in &sp.circulation {
        let outline: Vec<Vec2> = c.outline().into_iter().map(|p| Vec2::new(p[0], p[1])).collect();
        let area = cadaddy_geom::shoelace(&outline).abs();
        if outline.len() < 3 || area < 1e-9 || out.len() >= MAX_ROOMS {
            continue;
        }
        // As a label names a spec room: the first of its name on its level, once (a corridor
        // walls cut in two is one room's).
        let room = sp.rooms.iter().position(|r| names_room(r, &c.name) && r.level.as_ref().is_none_or(|l| l.eq_ignore_ascii_case(&c.level)));
        let room = room.filter(|i| claimed.insert(*i));
        let bounds = cadaddy_geom::Bounds2::from_points(outline.iter().copied());
        out.push(Traced {
            room,
            level: c.level.clone(),
            outline,
            tag: Handle(0),
            at: Vec2::new(c.at[0], c.at[1]),
            name: Some(c.name.clone()),
            number: None,
            cached_area: area,
            cached_bounds: bounds,
            circulation: true,
            on_floor: true,
            floor_room: None,
        });
    }
    let n_circulation = out.len();
    for (h, text, at, text_h) in texts {
        if out.len() >= MAX_ROOMS {
            break;
        }
        let plan_tag = plan_room_tag(&text, text_h, tag_h.as_ref());
        if super::perceive::geometry::room_number(&text) {
            let here = level_at(&sp.levels, at.x, at.y).map(|l| l.name.clone());
            // One number per level: a room of it numbered already (by its own label on the plan,
            // read first) keeps it; another trade's tag for it that lands elsewhere numbers nothing.
            if out
                .iter()
                .any(|o| here.as_ref().is_none_or(|l| *l == o.level) && o.number.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(text.trim())))
            {
                continue;
            }
            // The smallest room traced round the number (a trace from another label that got out
            // into the next room holds it too).
            let allowed = tagged || spec_number(&text) || plan_tag;
            if let Some(o) = out
                .iter_mut()
                .filter(|o| o.number.is_none() && here.as_ref().is_none_or(|l| *l == o.level) && cadaddy_geom::point_in_polygon(&o.outline, at))
                .min_by(|a, b| a.cached_area.total_cmp(&b.cached_area))
            {
                // Two numbers in a room traced from its name: the one by the name is its own; the
                // other is a room the trace got into (its door gap left open).
                let theirs = !o.circulation
                    && number_tags.iter().any(|(n, p)| {
                        !n.eq_ignore_ascii_case(text.trim()) && cadaddy_geom::point_in_polygon(&o.outline, *p) && p.dist(o.at) < at.dist(o.at)
                    });
                if allowed && theirs && o.on_floor {
                    carve.push((text.trim().to_string(), at));
                    continue;
                }
                // On a room tag layer any number is a room's; else the spec's rooms' numbers and
                // the plan's room tags.
                if allowed {
                    o.number = Some(text.trim().to_string());
                }
                continue;
            }
        }
        // A stair's direction note names no room.
        if matches!(text.trim().trim_end_matches('.').to_ascii_uppercase().as_str(), "DN" | "UP" | "DOWN") {
            continue;
        }
        // A label on a corridor's, stair's or hoistway's floor names no other room (its own
        // label, a stair's DN, its number once it has one).
        let on_circulation = out.iter().take(n_circulation).any(|o| {
            level_at(&sp.levels, at.x, at.y).is_some_and(|l| l.name.eq_ignore_ascii_case(&o.level)) && cadaddy_geom::point_in_polygon(&o.outline, at)
        });
        if on_circulation {
            continue;
        }
        // A spec room on a level is looked for on that level's plan only (every floor has its
        // OFFICE).
        let here = level_at(&sp.levels, at.x, at.y);
        // A room traced from its own tag (`at`) is found by its number only: the plans' unit-type
        // label elsewhere names other rooms.
        let is_number = super::perceive::geometry::room_number(&text);
        let room = sp.rooms.iter().position(|r| {
            names_room(r, &text)
                && (is_number || r.at.is_none())
                && r.level.as_ref().is_none_or(|l| here.is_some_and(|h| h.name.eq_ignore_ascii_case(l)))
        });
        // Without room tag layers only the spec's rooms are looked for. On them, a room's name
        // or number only (not a grid bubble's letter, a note's word): a note, a sentence, a stamp
        // or a title block's line is never a room.
        let score = super::perceive::geometry::room_name_score(&text);
        let listed_or_tag =
            if tagged { room_tag_text(&text) && (is_number || (score > 0 && super::autospec::room_like_name(&text))) } else { plan_tag };
        if room.is_none() && !listed_or_tag {
            continue;
        }
        let Some(level) =
            room.and_then(|i| sp.rooms.get(i)).and_then(|r| r.level.as_ref()).and_then(|n| sp.level(n)).or_else(|| level_at(&sp.levels, at.x, at.y))
        else {
            continue;
        };
        // On a storey whose circulation was traced: the room on its floor (inside its walls, door
        // gaps closed, in what the circulation leaves); else the drawn outline round the tag.
        let level_key = level.name.to_ascii_uppercase();
        let on_floor = floors
            .entry(level_key.clone())
            .or_insert_with(|| wallgen::room_floors(d, sp, &level.name))
            .as_mut()
            .and_then(|f| match f.room_at(at) {
                Ok(found) => Some(found),
                Err(why) => {
                    *floor_misses.entry(why).or_default() += 1;
                    // A label that traced no room may be a room of its own: no pocket joins
                    // another room over it.
                    missed_labels.entry(level_key.clone()).or_default().push(at);
                    None
                }
            })
            .map(|(n, rects)| (n, rects_outline(rects)))
            .filter(|(_, o)| o.len() >= 3);
        let traced_on_floor = on_floor.is_some();
        let (floor_room, on_floor) = match on_floor {
            Some((n, o)) => (Some(n), Some(o)),
            None => (None, None),
        };
        let outline = match on_floor {
            Some(o) => o,
            None => {
                let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
                let Some(o) = outline_round_label(plan, at, &mut outline_cache, MIN_ROOM_SF * upf * upf) else { continue };
                o
            }
        };
        if outline.len() < 3 || cadaddy_geom::shoelace(&outline).abs() < 1e-9 {
            continue;
        }
        // A name and a number in one room: the first traced keeps it (the spec's room wins).
        // The same outline traced from another point may start at another corner: same area and
        // extents.  Q6029: use cached area/bounds from the grid instead of recomputing.
        let area = cadaddy_geom::shoelace(&outline).abs();
        let bounds = cadaddy_geom::Bounds2::from_points(outline.iter().copied());
        let same = |o: &Traced| {
            (o.cached_area - area).abs() <= 1e-6 * area.max(1.0)
                && o.cached_bounds.min.dist(bounds.min) < 1e-6
                && o.cached_bounds.max.dist(bounds.max) < 1e-6
        };
        let (name, number) = if is_number { (None, Some(text.trim().to_string())) } else { (Some(text.trim().to_string()), None) };
        // Q6029: search only the grid cell and its neighbours instead of the whole list.
        let (gx, gy) = grid_key(&bounds);
        let mut found = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(idxs) = grid.get(&(gx + dx, gy + dy)) {
                    for &idx in idxs {
                        if let Some(o) = out.get(idx)
                            && same(o)
                        {
                            found = Some(idx);
                            break 'search;
                        }
                    }
                }
            }
        }
        match found {
            Some(idx) => {
                let o = &mut out[idx];
                if o.room.is_none() {
                    o.room = room.filter(|i| claimed.insert(*i));
                }
                // The best room label names it (a room or unit-type word first).
                let better = |n: &String| {
                    o.name.as_ref().is_none_or(|x| super::perceive::geometry::room_name_score(x) < super::perceive::geometry::room_name_score(n))
                };
                if name.as_ref().is_some_and(better) {
                    o.name = name;
                }
                // A number of its own in a room numbered already: a room the trace got into.
                if let (Some(n), Some(mine)) = (number.as_ref(), o.number.as_ref())
                    && !n.eq_ignore_ascii_case(mine)
                    && o.on_floor
                    && !o.circulation
                {
                    carve.push((n.clone(), at));
                }
                o.number = o.number.take().or(number);
            }
            None => {
                // A spec room is one room: a second label naming it (its name printed twice on the
                // plan) gives another room that name, not its number.
                let room = room.filter(|i| claimed.insert(*i));
                let idx = out.len();
                grid.entry((gx, gy)).or_default().push(idx);
                out.push(Traced {
                    room,
                    level: level.name.clone(),
                    outline,
                    tag: h,
                    at,
                    name,
                    number,
                    cached_area: area,
                    cached_bounds: bounds,
                    circulation: false,
                    on_floor: traced_on_floor,
                    floor_room,
                });
            }
        }
    }
    // A number in a room numbered already: the closed outline drawn round it (its own walls, its
    // door) inside that room is a room of its own, carved out of it (Q6160).
    let carved = carve_rooms(&mut out, &carve, &mut floors, sp, plan, &mut outline_cache, &number_tags);
    if carved > 0 {
        notes.push(format!("{carved} room(s) tagged with their own number inside another room's trace were traced on their own walls."));
    }
    // The floor no label traced: a pocket a room's own door or a narrow passage cut off (its bath,
    // its entry) joins the one room it opens onto (Q6160).
    let mut joined = 0usize;
    for (key, f) in floors.iter_mut() {
        let Some(f) = f.as_mut() else { continue };
        let keep_out = missed_labels.get(key).map(Vec::as_slice).unwrap_or_default();
        for (n, rects) in f.join_pockets(keep_out) {
            let outline = rects_outline(rects);
            if outline.len() < 3 {
                continue;
            }
            for t in out.iter_mut().filter(|t| t.on_floor && !t.circulation && t.floor_room == Some(n) && t.level.eq_ignore_ascii_case(key)) {
                t.cached_area = cadaddy_geom::shoelace(&outline).abs();
                t.cached_bounds = cadaddy_geom::Bounds2::from_points(outline.iter().copied());
                t.outline = outline.clone();
                joined += 1;
            }
        }
    }
    if joined > 0 {
        notes.push(format!("{joined} room(s) took in the floor behind their own doors (a bath, an entry) that no label traced."));
    }
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let numbered = number_unnumbered(&mut out, &number_tags, &sp.levels, NUMBER_REACH_FT * upf);
    let named = name_from_labels(&mut out, &name_labels, sp);
    if named > 0 {
        notes.push(format!("{named} room(s) the spec knows by number alone took their name from the room label inside them."));
    }
    if numbered > 0 {
        notes.push(format!("{numbered} room(s) without a number took the plan's number tag nearest their middle."));
    }
    if floors.values().any(Option::is_some) {
        let on = out.iter().filter(|t| t.on_floor && !t.circulation).count();
        super::guided::progress_line(&format!(
            "  rooms traced on the storeys' floors: {on}; by the drawing: {}; labels the floors refused: {floor_misses:?}",
            out.iter().filter(|t| !t.on_floor).count()
        ));
    }
    for (i, r) in sp.rooms.iter().enumerate() {
        if !out.iter().any(|t| t.room == Some(i)) {
            notes.push(format!("Room {}: no closed outline found around its tag; it gets the level's ceiling.", room_label(r)));
        }
    }
    Ok(out)
}

/// The closed outline drawn round a room label at `at`: the smallest loop holding it, or, when
/// that is smaller than a room (`min_area`: the box a tag is drawn in), the smallest room-sized
/// loop round that box (traced from just outside each of its sides). None when nothing closes.
pub(crate) fn outline_round_label(plan: &Session, at: Vec2, cache: &mut Option<super::hatch::Outlines>, min_area: f64) -> Option<Vec<Vec2>> {
    let trace = |p: Vec2, cache: &mut Option<super::hatch::Outlines>| {
        let loops = super::hatch::loops_at_lazy(plan, p, cache).ok()?;
        let outer = loops.iter().find(|l| l.outer).or(loops.first())?;
        let poly = Polyline { vertices: outer.vertices.clone(), closed: true }.tessellate(1e-3);
        (poly.len() >= 3).then_some(poly)
    };
    let first = trace(at, cache)?;
    let area = cadaddy_geom::shoelace(&first).abs();
    if area >= min_area {
        return Some(first);
    }
    // A box no bigger than a tag's: the room is round it.
    let b = cadaddy_geom::Bounds2::from_points(first.iter().copied());
    let m = (0.05 * (b.width() + b.height())).max(min_area.sqrt() / 16.0);
    let c = b.center();
    let probes = [Vec2::new(b.min.x - m, c.y), Vec2::new(b.max.x + m, c.y), Vec2::new(c.x, b.min.y - m), Vec2::new(c.x, b.max.y + m)];
    probes
        .into_iter()
        .filter_map(|p| trace(p, cache))
        .filter(|poly| {
            // Round the box: the label in it, or the box within its extents (a box touching a
            // wall is a notch in the outline, the label outside it).
            let pb = cadaddy_geom::Bounds2::from_points(poly.iter().copied());
            let tol = 1e-6 * (pb.width() + pb.height()).max(1.0);
            let holds_box = pb.min.x <= b.min.x + tol && pb.min.y <= b.min.y + tol && pb.max.x >= b.max.x - tol && pb.max.y >= b.max.y - tol;
            cadaddy_geom::shoelace(poly).abs() >= min_area && (cadaddy_geom::point_in_polygon(poly, at) || holds_box)
        })
        .min_by(|a, b| cadaddy_geom::shoelace(a).abs().total_cmp(&cadaddy_geom::shoelace(b).abs()))
}

/// The most a carved room may take of the room it is carved from.
const CARVE_MAX_SHARE: f64 = 0.5;
/// How much of a carved outline must lie inside the room it is carved from.
const CARVE_INSIDE: f64 = 0.9;

/// Each number in `carve` (text, where) that no room on its level has yet: the closed outline the
/// plan draws round it, when that is room-sized, at most [`CARVE_MAX_SHARE`] of the floor-traced
/// room holding it, [`CARVE_INSIDE`] inside it and holding no other number tag, becomes a room of
/// that number, its floor taken out of the room's. How many.
fn carve_rooms(
    out: &mut Vec<Traced>,
    carve: &[(String, Vec2)],
    floors: &mut std::collections::HashMap<String, Option<wallgen::Floors>>,
    sp: &BuildingSpec,
    plan: &Session,
    cache: &mut Option<super::hatch::Outlines>,
    number_tags: &[(String, Vec2)],
) -> usize {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let min_area = MIN_ROOM_SF * upf * upf;
    let mut n = 0usize;
    for (num, at) in carve {
        if out.len() >= MAX_ROOMS {
            break;
        }
        let Some(level) = level_at(&sp.levels, at.x, at.y).map(|l| l.name.clone()) else { continue };
        if out.iter().any(|o| o.level.eq_ignore_ascii_case(&level) && o.number.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(num))) {
            continue;
        }
        let Some(host) = out
            .iter()
            .enumerate()
            .filter(|(_, o)| o.on_floor && !o.circulation && o.level.eq_ignore_ascii_case(&level) && cadaddy_geom::point_in_polygon(&o.outline, *at))
            .min_by(|a, b| a.1.cached_area.total_cmp(&b.1.cached_area))
            .map(|(i, _)| i)
        else {
            continue;
        };
        let Some(fr) = out.get(host).and_then(|o| o.floor_room) else { continue };
        let Some(poly) = outline_round_label(plan, *at, cache, min_area) else { continue };
        let area = cadaddy_geom::shoelace(&poly).abs();
        let Some(h) = out.get(host) else { continue };
        let others = number_tags.iter().any(|(m, p)| !m.eq_ignore_ascii_case(num) && cadaddy_geom::point_in_polygon(&poly, *p));
        if poly.len() < 3 || area < min_area || area > CARVE_MAX_SHARE * h.cached_area || share_inside(&poly, &h.outline) < CARVE_INSIDE || others {
            continue;
        }
        let Some(Some(f)) = floors.get_mut(&level.to_ascii_uppercase()) else { continue };
        let Some(new) = f.carve(fr, &poly) else { continue };
        let (Some(old_rects), Some(new_rects)) = (f.rects(fr).map(rects_outline), f.rects(new).map(rects_outline)) else { continue };
        if old_rects.len() < 3 || new_rects.len() < 3 {
            continue;
        }
        for t in out.iter_mut().filter(|t| t.on_floor && t.floor_room == Some(fr) && t.level.eq_ignore_ascii_case(&level)) {
            t.cached_area = cadaddy_geom::shoelace(&old_rects).abs();
            t.cached_bounds = cadaddy_geom::Bounds2::from_points(old_rects.iter().copied());
            t.outline = old_rects.clone();
        }
        let bounds = cadaddy_geom::Bounds2::from_points(new_rects.iter().copied());
        out.push(Traced {
            room: None,
            level: level.clone(),
            cached_area: cadaddy_geom::shoelace(&new_rects).abs(),
            cached_bounds: bounds,
            outline: new_rects,
            tag: Handle(0),
            at: *at,
            name: None,
            number: Some(num.clone()),
            circulation: false,
            on_floor: true,
            floor_room: Some(new),
        });
        n += 1;
    }
    n
}

/// How far outside its outline a room's number tag may sit, feet (a tag set in the doorway).
const NUMBER_REACH_FT: f64 = 3.0;

/// Each traced room (not circulation) without a name of its own, whose spec room is known by its
/// number alone (the rooms of a unit type: the type is every one's name), takes the best room
/// label inside its outline on its level (`labels`: text, place): a room or unit-type word
/// first, then the one nearest its tag. How many were named.
fn name_from_labels(rooms: &mut [Traced], labels: &[(String, Vec2)], sp: &BuildingSpec) -> usize {
    let mut named = 0;
    for t in rooms.iter_mut().filter(|t| !t.circulation && t.name.is_none()) {
        if t.room.and_then(|i| sp.rooms.get(i)).is_some_and(|r| !r.name.trim().is_empty()) {
            continue;
        }
        let best = labels
            .iter()
            .filter(|(_, at)| {
                level_at(&sp.levels, at.x, at.y).is_none_or(|l| l.name.eq_ignore_ascii_case(&t.level))
                    && cadaddy_geom::point_in_polygon(&t.outline, *at)
            })
            .max_by(|a, b| {
                let score = |s: &str| super::perceive::geometry::room_name_score(s);
                score(&a.0).cmp(&score(&b.0)).then(b.1.dist(t.at).total_cmp(&a.1.dist(t.at)))
            });
        if let Some((name, _)) = best {
            t.name = Some(name.clone());
            named += 1;
        }
    }
    named
}

/// Each traced room (not circulation) with no number takes the number tag (`tags`: text, place)
/// on its level nearest its middle that no room on the level has: inside its outline first, else
/// within `reach` of it and inside no other room. How many were numbered (Q6160).
fn number_unnumbered(rooms: &mut [Traced], tags: &[(String, Vec2)], levels: &[cadaddy_walk3d::Level], reach: f64) -> usize {
    let mut n = 0usize;
    for i in 0..rooms.len() {
        let Some(t) = rooms.get(i) else { continue };
        if t.circulation || t.number.is_some() {
            continue;
        }
        let (level, mid) = (t.level.clone(), t.cached_bounds.center());
        let on_level = |at: Vec2| levels.is_empty() || level_at(levels, at.x, at.y).is_some_and(|l| l.name.eq_ignore_ascii_case(&level));
        let taken = |num: &str| {
            rooms.iter().any(|o| o.level.eq_ignore_ascii_case(&level) && o.number.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(num)))
        };
        let in_other = |at: Vec2| {
            rooms.iter().enumerate().any(|(j, o)| j != i && o.level.eq_ignore_ascii_case(&level) && cadaddy_geom::point_in_polygon(&o.outline, at))
        };
        let free: Vec<&(String, Vec2)> = tags.iter().filter(|(num, at)| on_level(*at) && !taken(num)).collect();
        let inside =
            free.iter().filter(|(_, at)| cadaddy_geom::point_in_polygon(&t.outline, *at)).min_by(|a, b| a.1.dist(mid).total_cmp(&b.1.dist(mid)));
        let near = || {
            free.iter()
                .filter(|(_, at)| !in_other(*at) && super::guided::edge_distance(&t.outline, *at) <= reach)
                .min_by(|a, b| a.1.dist(mid).total_cmp(&b.1.dist(mid)))
        };
        let Some((num, _)) = inside.or_else(near).map(|x| (*x).clone()) else { continue };
        if let Some(t) = rooms.get_mut(i) {
            t.number = Some(num);
            n += 1;
        }
    }
    n
}

/// The outline of a room traced on a floor grid (its rectangles).
fn rects_outline(rects: &[[Vec2; 2]]) -> Vec<Vec2> {
    cadaddy_walk3d::Circulation { rects: rects.iter().map(|[a, b]| [[a.x, a.y], [b.x, b.y]]).collect(), ..Default::default() }
        .outline()
        .into_iter()
        .map(|p| Vec2::new(p[0], p[1]))
        .collect()
}

/// The layer room elements go on (with `rooms.build`'s volumes).
const ROOM_LAYER: &str = super::rooms3d::ROOM_LAYER;
/// The share of a room's outline that must run along its level's built walls, doors and
/// windows (the rest: openings, gaps the tracing closed).
const ROOM_WALLED: f64 = 0.5;
/// Most plan footprints and grid cells looked at per level (hostile drawings).
const MAX_BOUNDS: usize = 4_000_000;
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

/// Lines along the edges of the spec's circulation spaces (`sp.circulation`), so a room traced
/// on the drawing stops at a corridor, a stair or a hoistway.
fn add_circulation_edges(d: &mut Drawing, sp: &BuildingSpec) {
    for (a, b) in sp.circulation.iter().flat_map(|c| c.edges()) {
        let line = cadaddy_doc::Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) };
        if d.add(&Space::Model, Common::default(), EntityKind::Line(line)).is_err() {
            break;
        }
    }
}

/// A drawing of the built footprints' edges (each once), every level's where it was built, and
/// the circulation spaces' edges: the boundaries a room is traced on when its drawn outline isn't
/// one.
fn walls_session(bounds: &std::collections::BTreeMap<String, Vec<Vec<Vec2>>>, sp: &BuildingSpec) -> Option<Session> {
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
    add_circulation_edges(&mut d, sp);
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
fn walled_rooms(d: &Drawing, sp: &BuildingSpec, rooms: &mut Vec<Traced>, upf: f64, notes: &mut Vec<String>) {
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
    // Which tags those were (name, number, where), the first few: a missing room is findable.
    let mut unwalled_tags: Vec<String> = Vec::new();
    let mut retraced = 0usize;
    let mut retraced_or_tried = 0usize;
    let extents: std::collections::HashMap<&String, cadaddy_geom::Bounds2> =
        bounds.iter().map(|(l, polys)| (l, cadaddy_geom::Bounds2::from_points(polys.iter().flatten().copied()))).collect();
    let mut kept: Vec<Traced> = Vec::new();
    for mut t in std::mem::take(rooms) {
        // A corridor, stair or hoistway, or a room on its storey's floor, was traced on the walls
        // themselves.
        if t.on_floor {
            keep_room(&mut kept, t);
            continue;
        }
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
        let plan = walls_plan.get_or_insert_with(|| walls_session(&bounds, sp));
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
                // Q6029: update cached area/bounds when the outline is replaced.
                t.cached_area = cadaddy_geom::shoelace(&o).abs();
                t.cached_bounds = cadaddy_geom::Bounds2::from_points(o.iter().copied());
                t.outline = o;
                retraced += 1;
                keep_room(&mut kept, t);
            }
            None => {
                *unwalled.entry(t.level.clone()).or_default() += 1;
                if unwalled_tags.len() < 20 {
                    let what = [t.number.as_deref(), t.name.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" ");
                    unwalled_tags.push(format!("{} '{}' at [{:.0}, {:.0}]", t.level, what, t.at.x, t.at.y));
                }
            }
        }
    }
    *rooms = kept;
    if retraced > 0 {
        notes.push(format!("{retraced} room tag(s) traced again on the built walls (their drawn outline was a tag box or furniture, not the room)."));
    }
    for (level, n) in &unwalled {
        notes.push(format!("{level}: {n} room tag(s) not inside built walls (an enlarged view, a note's or the title block's box) — no room built."));
    }
    if !unwalled.is_empty() {
        super::guided::progress_line(&format!("  room tags not inside built walls (no room): {unwalled:?}: {}", unwalled_tags.join("; ")));
    }
    // One room per number and level (its tag's, else its spec room's): a corridor's, stair's or
    // hoistway's (its number tag on a piece of it the trace left out is no room of its own), else
    // the smaller outline.
    let area = |t: &Traced| cadaddy_geom::shoelace(&t.outline).abs();
    let number = |t: &Traced| {
        t.number
            .clone()
            .or_else(|| t.room.and_then(|i| sp.rooms.get(i)).and_then(|r| r.number.clone()))
            .map(|n| n.trim().to_ascii_uppercase())
            .filter(|n| !n.is_empty())
    };
    let better = |t: &Traced, o: &Traced| (t.circulation && !o.circulation) || (t.circulation == o.circulation && area(t) < area(o));
    let mut best: std::collections::HashMap<(String, String), usize> = std::collections::HashMap::new();
    for (i, t) in rooms.iter().enumerate() {
        let Some(n) = number(t) else { continue };
        let key = (t.level.trim().to_ascii_uppercase(), n);
        match best.get(&key).and_then(|j| rooms.get(*j)) {
            Some(o) if !better(t, o) => {}
            _ => {
                best.insert(key, i);
            }
        }
    }
    let before = rooms.len();
    let mut i = 0usize;
    rooms.retain(|t| {
        let keep = match number(t) {
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

/// How much of the smaller of two rooms' floors (0..1) lies inside the other's when they are one
/// room traced twice; and how big the smaller is beside the bigger at least (a closet or a
/// symbol's box drawn inside a room is a thing of its own).
const SAME_ROOM_INSIDE: f64 = 0.7;
const SAME_ROOM_SIZE: f64 = 0.4;

/// The share (0..1) of polygon `a`'s area that is inside polygon `b`, sampled on a grid over `a`.
fn share_inside(a: &[Vec2], b: &[Vec2]) -> f64 {
    const N: usize = 24;
    let (ba, bb) = (cadaddy_geom::Bounds2::from_points(a.iter().copied()), cadaddy_geom::Bounds2::from_points(b.iter().copied()));
    if ba.is_empty() || bb.is_empty() || ba.max.x < bb.min.x || bb.max.x < ba.min.x || ba.max.y < bb.min.y || bb.max.y < ba.min.y {
        return 0.0;
    }
    let (mut on_a, mut on_both) = (0usize, 0usize);
    for i in 0..N {
        for j in 0..N {
            let p = Vec2::new(
                ba.min.x + (i as f64 + 0.5) / N as f64 * (ba.max.x - ba.min.x),
                ba.min.y + (j as f64 + 0.5) / N as f64 * (ba.max.y - ba.min.y),
            );
            if cadaddy_geom::point_in_polygon(a, p) {
                on_a += 1;
                on_both += usize::from(cadaddy_geom::point_in_polygon(b, p));
            }
        }
    }
    if on_a == 0 { 0.0 } else { on_both as f64 / on_a as f64 }
}

/// One room per floor area: of two rooms of a level where the smaller lies mostly inside the
/// other ([`SAME_ROOM_INSIDE`]) and is no closet of it ([`SAME_ROOM_SIZE`]), one label's trace
/// stayed in the room and another's got out into the next. The corridor's, stair's or hoistway's
/// is the room, else the smaller; it takes the other's name, spec room and number when it has
/// none and the other's tag is on it. How many were dropped.
fn one_room_per_floor(rooms: &mut Vec<Traced>) -> usize {
    let area = |t: &Traced| cadaddy_geom::shoelace(&t.outline).abs();
    // Each pair once: (the bigger, the smaller), the biggest first.
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for i in 0..rooms.len() {
        for j in 0..rooms.len() {
            let (Some(big), Some(small)) = (rooms.get(i), rooms.get(j)) else { continue };
            let (ab, as_) = (area(big), area(small));
            if i == j || !big.level.trim().eq_ignore_ascii_case(small.level.trim()) || as_ > ab || (as_ == ab && j < i) {
                continue;
            }
            if as_ >= SAME_ROOM_SIZE * ab && share_inside(&small.outline, &big.outline) >= SAME_ROOM_INSIDE {
                pairs.push((i, j));
            }
        }
    }
    pairs.sort_by(|a, b| rooms.get(b.0).map(area).unwrap_or(0.0).total_cmp(&rooms.get(a.0).map(area).unwrap_or(0.0)));
    let mut gone = vec![false; rooms.len()];
    for (big, small) in pairs {
        if gone[big] || gone[small] {
            continue;
        }
        // A room traced on its storey's floor grid can't have got out into the next room; a
        // smaller trace by the drawing inside it (a label in its bath or entry) is the stray.
        let keep_big = (rooms[big].circulation || rooms[big].on_floor) && !rooms[small].circulation && !rooms[small].on_floor;
        let (keep, drop) = if keep_big { (big, small) } else { (small, big) };
        let (on, name, room, number) = {
            let d = &rooms[drop];
            (cadaddy_geom::point_in_polygon(&rooms[keep].outline, d.at), d.name.clone(), d.room, d.number.clone())
        };
        let k = &mut rooms[keep];
        if on {
            if k.number.is_none() && k.room.is_none() {
                k.number = number;
                k.room = room;
            }
            if k.name.is_none() {
                k.name = name;
            }
        }
        gone[drop] = true;
    }
    let mut i = 0usize;
    rooms.retain(|_| {
        i += 1;
        !gone[i - 1]
    });
    gone.iter().filter(|g| **g).count()
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
        let source = match room.filter(|r| r.ceiling_ft.is_some()) {
            Some(r) => r.ceiling_source.clone().unwrap_or_else(|| "the room's own spec".into()),
            None => format!("{}'s ceiling height (no room height printed)", level.name),
        };
        xdata.push(tg(1000, format!("ceiling_source:{}", clip(&source))));
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

/// A build summary's walls, doors and windows counted from what the model holds now (as
/// bim.tree counts them): steps after a build, and a rebuild after them, change it.
pub(crate) fn recount_built(d: &Drawing, r: &mut Value) {
    let mut held: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for e in d.model.iter() {
        if let Some(part) = wallgen::made_as(e) {
            *held.entry(part).or_default() += 1;
        }
    }
    for (key, part) in [("walls", "wall"), ("doors", "door"), ("windows", "window")] {
        r[key] = json!(held.get(part).copied().unwrap_or(0));
    }
}

/// Default time budget for model.build (Q6029 MUST 3): 20 minutes.
pub(crate) const BUILD_BUDGET_S: u64 = 1200;
/// The longest build budget a caller may give, seconds (a day).
pub(crate) const MAX_BUILD_BUDGET_S: u64 = 86_400;

/// Build from a spec: WALLGEN's walls, openings, floors and ceilings, then the spec's own parts
/// and materials. Reports progress per level (Q6029 MUST 2).
pub(crate) fn build_spec(s: &mut Session, g: &WallGen) -> Result<Value> {
    build_spec_with_budget(s, g, BUILD_BUDGET_S)
}

/// A model build step done: its `build.progress` event, and a line to the job's progress sink.
fn build_step(s: &mut Session, step: &str, started: web_time::Instant) {
    let secs = started.elapsed().as_secs_f64();
    s.event("build.progress", json!({"step": step, "elapsed_s": secs}));
    super::guided::progress_line(&format!("  model.build: {step} at {secs:.1} s"));
}

/// [`build_spec`] with an explicit time budget (seconds).
pub(crate) fn build_spec_with_budget(s: &mut Session, g: &WallGen, budget_s: u64) -> Result<Value> {
    let Some(sp) = g.spec.clone() else { return wallgen::build(s, g) };
    let deadline = web_time::Instant::now() + std::time::Duration::from_secs(budget_s);
    let build_start = web_time::Instant::now();
    // What a person set on a room element (bim.props.set) goes on the rebuilt room.
    let room_edits = room_user_edits(s.doc()?);
    let removed = wallgen::remove_made(s.doc_mut()?);
    build_step(s, "old model removed", build_start);
    let mut notes: Vec<String> = Vec::new();
    s.event("build.progress", json!({"step": "trace_rooms", "detail": "tracing room outlines"}));
    let mut rooms = trace_rooms(s, &sp, &mut notes)?;
    build_step(s, "rooms traced", build_start);
    let trace_ms = build_start.elapsed().as_millis();
    s.event("build.progress", json!({"step": "trace_rooms_done", "rooms": rooms.len(), "ms": trace_ms}));
    // Q6029 MUST 3: over budget after trace_rooms alone → stop with a finding.
    if web_time::Instant::now() > deadline {
        notes.push(format!("Build stopped: room tracing took {:.0} s, over the {} s budget.", trace_ms as f64 / 1000.0, budget_s));
        return Err(bad(
            "model.build",
            format!(
                "Time budget ({budget_s} s) exceeded during room tracing ({:.0} s). {} rooms traced so far.",
                trace_ms as f64 / 1000.0,
                rooms.len()
            ),
        ));
    }
    s.event("build.progress", json!({"step": "walls", "detail": "building walls per level"}));
    let mut r = wallgen::build_with_progress(s, g, &deadline)?;
    build_step(s, "walls built", build_start);
    let wall_ms = build_start.elapsed().as_millis();
    s.event("build.progress", json!({"step": "walls_done", "ms": wall_ms, "walls": r["walls"]}));
    // Q6029 MUST 3: budget check after walls.
    if web_time::Instant::now() > deadline {
        let secs = wall_ms as f64 / 1000.0;
        notes.push(format!("Build stopped after walls: {secs:.0} s exceeds the {budget_s} s budget."));
        return Err(bad(
            "model.build",
            format!(
                "Time budget ({budget_s} s) exceeded during wall build ({secs:.0} s). {} walls, {} levels built so far.",
                r["walls"], r["levels"]
            ),
        ));
    }
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    s.event("build.progress", json!({"step": "validate_rooms", "rooms": rooms.len()}));
    walled_rooms(s.doc_mut()?, &sp, &mut rooms, upf, &mut notes);
    let twice = one_room_per_floor(&mut rooms);
    if twice > 0 {
        notes.push(format!("{twice} room(s) traced twice over one floor (one label's trace got out into the next room): one room each."));
    }
    build_step(s, "rooms validated", build_start);
    s.event("build.progress", json!({"step": "validate_rooms_done", "rooms": rooms.len()}));
    let mark = |step: &str| super::guided::progress_line(&format!("  model.build: {step} at {:.1} s", build_start.elapsed().as_secs_f64()));
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
    // Each room as a model element of its level (a space: its floor outline, name, number,
    // area, ceiling and floor finish), what bim.tree, the takeoff and the room commands list.
    let room_elements = add_room_elements(d, &sp, &rooms, upf, &room_edits);
    mark("room ceilings and elements");
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
    mark("columns, ground and room ceilings");
    // Stairs, ramps, their handrails and the guards on their landings.
    let built_stairs = super::stairs::build(d, &sp);
    mark("stairs");
    // The outside: roofs, parapets, crickets, rooftop equipment, facade glass and materials.
    let outside = super::exterior::build(d, &sp);
    mark("exterior");
    // The reflected ceiling plans' ceilings (replacing the level's and rooms'), grids, soffits,
    // lights and diffusers.
    let rcp = super::ceilings::build(d, &sp);
    mark("reflected ceilings");
    // Duct, pipe and conduit runs of the shown systems.
    let mep = super::mep::build_with(d, &sp, true);
    mark("MEP runs");
    // The devices the readers found, each an element with what was read about it.
    let devices = super::propfill::build_devices(d, &sp);
    mark("device elements");
    // The circuits routed through the model now its devices stand (Q6152): around the ducts,
    // pipes and structure, by the corridors, into each box and the panel's entry window; the
    // drawn conduit as it is. The estimated routes give way to them for the pieces.
    // From the circuits as read (a reroute may have stored its routing in the spec).
    let sp_read = super::conduitroute::unrouted(d, &sp);
    let routed = if super::conduitroute::enabled(d) {
        super::conduitroute::route(d, &sp_read)
    } else {
        super::conduitroute::Routed {
            report: json!({"routed": false, "note": "routing switched off (model.build {route: false})"}),
            ..Default::default()
        }
    };
    let mut sp_routed = sp_read;
    super::conduitroute::apply(&mut sp_routed, &routed);
    super::conduitroute::store_report(d, &routed.report);
    mark("conduit routed");
    // The electrical runs as their pieces (sticks, fittings, boxes, cable, wire) and the count
    // index (Q6157).
    let atoms = super::mepatoms::build::build(d, &sp_routed);
    let mut report = routed.report.clone();
    super::conduitroute::check_box_fill(d, &mut report);
    super::conduitroute::store_report(d, &report);
    mark("MEP atoms");
    // Concrete: slabs, footings, pads, bases and encasement, with rebar and what is cast in.
    let concrete = super::concrete::build(d, &sp);
    mark("concrete");
    // Takeoff tables linked to the model count it again.
    let qty_tables = super::mepqty::refresh(d, &sp_routed);
    mark("takeoff tables");
    // Columns at the grid intersections.
    let grid_columns = super::grids::build(d, &sp);
    mark("grid columns");
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
    if !built_stairs.left_out.is_empty() {
        r["stairs_left_out"] = json!(built_stairs.left_out);
    }
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
    if let (Some(o), Some(x)) = (r.as_object_mut(), atoms.as_object()) {
        o.extend(x.clone());
    }
    r["gridColumns"] = json!(grid_columns);
    r["conduitRoute"] = json!({
        "circuits": routed.report["routed_circuits"], "runs": routed.report["runs"], "homeruns": routed.report["homeruns"],
        "routed_ft": routed.report["routed_ft"], "clashes": routed.report["clashes"].as_array().map_or(0, Vec::len),
        "conflicts": routed.report["conflicts"].as_array().map_or(0, Vec::len),
        "penetrations": routed.report["penetrations"].as_array().map_or(0, Vec::len),
    });
    r["deviceElements"] = json!(devices.built);
    r["deviceBuild"] = devices.to_json();
    r["qtyTables"] = json!(qty_tables);
    r["removed"] = json!(removed);
    // One building: the levels' 3D moved off their plans onto the lowest level's.
    if sp.stack {
        r["stacked"] = json!(stack_levels(d, &sp));
        mark("levels stacked");
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

    fn traced(level: &str, [x0, y0, x1, y1]: [f64; 4], at: [f64; 2], name: Option<&str>, number: Option<&str>, circulation: bool) -> Traced {
        let outline = vec![Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)];
        let bounds = cadaddy_geom::Bounds2::from_points(outline.iter().copied());
        Traced {
            room: None,
            level: level.into(),
            cached_area: cadaddy_geom::shoelace(&outline).abs(),
            outline,
            tag: Handle(0),
            at: Vec2::new(at[0], at[1]),
            name: name.map(String::from),
            number: number.map(String::from),
            cached_bounds: bounds,
            circulation,
            on_floor: circulation,
            floor_room: None,
        }
    }

    #[test]
    fn a_number_or_room_name_in_the_plans_tag_size_tags_a_room_the_spec_doesnt_list() {
        let sp = BuildingSpec::from_json(
            r#"{"units_per_foot": 12, "rooms": [{"name": "LOBBY", "number": "5010"}, {"name": "OFFICE", "number": "5011"}, {"name": "", "number": "5012"}]}"#,
        )
        .unwrap();
        let t = |s: &str, h: f64| (Handle(0), s.to_string(), Vec2::new(0.0, 0.0), h);
        // The labels naming the spec's rooms set the size: 10 units (a stray note's 30 aside).
        let texts = vec![t("LOBBY", 10.0), t("5010", 10.0), t("OFFICE", 9.5), t("5011", 10.5), t("5012", 10.0), t("STORAGE", 30.0)];
        let st = tag_height(&sp, &texts);
        assert!(st.as_ref().is_some_and(|s| (9.5..=10.5).contains(&s.height)), "{st:?}");
        let h = st.as_ref();
        // A number the spec doesn't list, in that size: a room's tag (a closet tagged with its
        // number only); a room name with a room word too.
        assert!(plan_room_tag("5015", 10.0, h));
        assert!(plan_room_tag("STORAGE", 11.0, h));
        // A read of the same number drawn far bigger (an OCR guess) or a small note is not.
        assert!(!plan_room_tag("5015", 79.0, h));
        assert!(!plan_room_tag("5015", 4.0, h));
        // A door's tag (`5015A`) is no room number of this plan's shape.
        assert!(!plan_room_tag("5015A", 10.0, h));
        // A note or a lone word without a room word, in the tag size, is not.
        assert!(!plan_room_tag("LINE OF CANOPY ABOVE", 10.0, h));
        assert!(!plan_room_tag("SEE A-201", 10.0, h));
        // Under three spec labels there is no tag size to go by: nothing new is a tag.
        let few = vec![t("LOBBY", 10.0), t("5010", 10.0)];
        assert_eq!(tag_height(&sp, &few), None);
        assert!(!plan_room_tag("5015", 10.0, None));
    }

    #[test]
    fn a_room_traced_twice_over_one_floor_is_one_room_the_tighter_trace() {
        let mut rooms = vec![
            // A trace that got out into the next room, numbered from the tag both hold; the room
            // itself traced from its name.
            traced("Level 3", [0.0, 0.0, 300.0, 100.0], [50.0, 50.0], None, Some("316"), false),
            traced("Level 3", [0.0, 0.0, 160.0, 100.0], [80.0, 50.0], Some("DOUBLE Q"), None, false),
            // A closet drawn inside a room: a thing of its own.
            traced("Level 3", [400.0, 0.0, 600.0, 200.0], [500.0, 150.0], Some("SUITE"), Some("330"), false),
            traced("Level 3", [400.0, 0.0, 450.0, 50.0], [425.0, 25.0], Some("CLOSET"), None, false),
            // A room traced over the corridor: the corridor's floor.
            traced("Level 3", [0.0, 200.0, 400.0, 260.0], [100.0, 230.0], Some("CORRIDOR"), Some("300"), true),
            traced("Level 3", [100.0, 200.0, 300.0, 260.0], [200.0, 230.0], Some("Room"), None, false),
            // The same footprints on another level are that level's.
            traced("Level 4", [0.0, 0.0, 160.0, 100.0], [80.0, 50.0], Some("DOUBLE Q"), Some("416"), false),
        ];
        assert_eq!(one_room_per_floor(&mut rooms), 2);
        let names: Vec<(Option<&str>, Option<&str>, f64)> =
            rooms.iter().map(|t| (t.name.as_deref(), t.number.as_deref(), cadaddy_geom::shoelace(&t.outline).abs())).collect();
        assert_eq!(
            names,
            vec![
                (Some("DOUBLE Q"), Some("316"), 16_000.0),
                (Some("SUITE"), Some("330"), 40_000.0),
                (Some("CLOSET"), None, 2_500.0),
                (Some("CORRIDOR"), Some("300"), 24_000.0),
                (Some("DOUBLE Q"), Some("416"), 16_000.0),
            ]
        );
    }

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

    #[test]
    fn a_room_without_a_number_takes_the_tag_nearest_its_middle() {
        let mut rooms = vec![
            traced("Level 1", [0.0, 0.0, 120.0, 240.0], [60.0, 120.0], Some("GUEST ROOM"), Some("A1"), false),
            traced("Level 1", [120.0, 0.0, 240.0, 240.0], [180.0, 120.0], Some("SUITE"), None, false),
            traced("Level 1", [240.0, 0.0, 360.0, 240.0], [300.0, 120.0], Some("STORAGE"), None, false),
            traced("Level 1", [0.0, 240.0, 360.0, 300.0], [180.0, 270.0], Some("CORRIDOR"), None, true),
        ];
        let tags = vec![
            // A1 is the first room's already; A2 sits in the second room off its middle, A3
            // nearer its middle but in the doorway of the corridor (outside, inside no other room
            // but taken by nobody: the second room's own tag inside wins).
            ("A1".to_string(), Vec2::new(60.0, 100.0)),
            ("A2".to_string(), Vec2::new(200.0, 60.0)),
            ("A3".to_string(), Vec2::new(380.0, 120.0)),
        ];
        let n = number_unnumbered(&mut rooms, &tags, &[], 36.0);
        assert_eq!(rooms[1].number.as_deref(), Some("A2"));
        // The third room has no tag inside; A3 is 20" off its edge and in no other room.
        assert_eq!(rooms[2].number.as_deref(), Some("A3"));
        // The corridor is circulation: no number from a room's tag.
        assert_eq!(rooms[3].number, None);
        assert_eq!(n, 2);
    }

    #[test]
    fn a_room_the_spec_knows_by_number_takes_the_label_inside_it() {
        // Two rooms of one unit type, the spec has them by number only; a lobby by name.
        let sp = BuildingSpec {
            rooms: vec![
                cadaddy_walk3d::SpecRoom { number: Some("5101".into()), ..Default::default() },
                cadaddy_walk3d::SpecRoom { name: "LOBBY".into(), number: Some("5100".into()), ..Default::default() },
            ],
            ..Default::default()
        };
        let mut a = traced("Level 1", [0.0, 0.0, 120.0, 240.0], [60.0, 40.0], None, Some("5101"), false);
        a.room = Some(0);
        let mut lobby = traced("Level 1", [120.0, 0.0, 360.0, 240.0], [200.0, 120.0], None, Some("5100"), false);
        lobby.room = Some(1);
        let other = traced("Level 1", [360.0, 0.0, 480.0, 240.0], [420.0, 40.0], None, Some("5102"), false);
        let mut rooms = vec![a, lobby, other];
        let labels = vec![
            ("CLOSET".to_string(), Vec2::new(20.0, 220.0)),
            ("KING SUITE".to_string(), Vec2::new(60.0, 60.0)),
            ("KING SUITE".to_string(), Vec2::new(420.0, 60.0)),
            ("KING SUITE".to_string(), Vec2::new(200.0, 140.0)),
        ];
        assert_eq!(name_from_labels(&mut rooms, &labels, &sp), 2);
        // The unit type's word beats a closet's label in the same trace.
        assert_eq!(rooms[0].name.as_deref(), Some("KING SUITE"));
        // The spec's own name stays the lobby's (the build names it from the spec).
        assert_eq!(rooms[1].name, None);
        assert_eq!(rooms[2].name.as_deref(), Some("KING SUITE"));
    }

    #[test]
    fn a_room_traced_on_its_floor_keeps_itself_over_a_stray_trace_inside_it() {
        // The guest room on its floor (with its bath); a label in the bath traced by the drawing.
        let mut room = traced("Level 1", [0.0, 0.0, 120.0, 300.0], [60.0, 200.0], Some("SUITE"), Some("B7"), false);
        room.on_floor = true;
        let stray = traced("Level 1", [0.0, 0.0, 120.0, 200.0], [60.0, 50.0], Some("STORAGE"), None, false);
        let mut rooms = vec![room, stray];
        assert_eq!(one_room_per_floor(&mut rooms), 1);
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].name.as_deref(), Some("SUITE"));
        assert_eq!(rooms[0].number.as_deref(), Some("B7"));
    }
}
