//! Checking a model built from plans (`model.build` or WALLGEN) against the plan and the
//! building spec, for people and agents:
//!
//! - `model.issues`: rooms that leak (no closed outline around the tag, or one running into
//!   another room), doors and windows of the plan that no wall took, walls without height,
//!   walls overlapping each other, fixtures above their room's ceiling or below the floor, and
//!   levels that don't match (no plan objects, a gap between storeys, objects built for a level
//!   the spec doesn't have, levels on other sheets);
//! - `model.render`: an image of the model, of one level as an axonometric, of a named room, or
//!   from a camera pose (a parallel projection along the camera's direction, keeping what is in
//!   front of it), drawn by the shaded 3D renderer;
//! - `model.overlay`: the plan in grey with the model's footprints drawn over it and the
//!   differences in red (plan walls nothing was built on, openings that weren't built, walls
//!   built where the plan has none);
//! - `model.counts`: doors, windows and fixtures per level, the plan's against the model's.
//!
//! Wall overlap is the area common to two convex footprints, clipped after I. E. Sutherland and
//! G. W. Hodgman, "Reentrant Polygon Clipping", Communications of the ACM 17(1), 1974.

use std::collections::HashMap;
use std::sync::Arc;

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Mesh, Space};
use cadaddy_geom::view3::Projection;
use cadaddy_geom::{Bounds2, PolyVertex, Vec2, Vec3};
use cadaddy_walk3d::{BuildingSpec, Role, level_at};
use serde_json::{Value, json};

use super::bspec::{centre, current, made_level, names_room, plan_objects, room_texts};
use super::wallgen::made_as;
use super::*;
use crate::{Result, Session};

/// Findings listed at most, and handles per finding.
const MAX_FINDINGS: usize = 2_000;
const MAX_HANDLES: usize = 50;
/// Walls compared for overlaps, at most (hostile drawings).
const MAX_WALLS: usize = 20_000;
/// Largest image side.
const MAX_PX: u32 = 4096;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("model.issues", "Model Issues", run_issues)
            .params("{kind?: leak|opening_not_in_wall|wall_without_height|walls_overlap|fixture_above_ceiling|fixture_below_floor|level_unmatched|not_built} → {issues: [{kind, message, level?, at?, handles}], total, byKind}")
            .noundo(),
        CommandSpec::new("model.render", "Render Model View", run_render)
            .params("{path?: PNG file (else the PNG comes back as base64), width?, height? (800×600), style?: realistic|shaded|conceptual|hidden|wireframe, level?: name (an axonometric of it), room?: name or number, eye?: [x,y,z], target?: [x,y,z] (a camera pose), dir?: [x,y,z] (view direction towards the viewer)} → {path?|png, width, height, triangles}")
            .noundo(),
        CommandSpec::new("model.overlay", "Model Overlay", run_overlay)
            .params("{path?: PNG file (else base64), width?, height? (1200×900), level?: name} → {path?|png, footprints, differences: [{kind: wall_not_built|opening_not_built|wall_not_in_plan, handle, at}]}")
            .noundo(),
        CommandSpec::new("model.counts", "Model Counts", run_counts)
            .params("{} → {levels: [{level, plan: {doors, windows, fixtures}, model: {doors, windows, fixtures}, match}], match}")
            .noundo(),
    ]
}

// ---------------- what was built ----------------

/// An object a build made: its part, level, source object and mesh.
struct Built {
    handle: Handle,
    part: String,
    level: String,
    source: Handle,
    mesh: Mesh,
}

fn built(d: &Drawing) -> Vec<Built> {
    d.model
        .iter()
        .filter_map(|e| {
            let part = made_as(e)?;
            let EntityKind::Mesh(m) = &e.kind else { return None };
            let x = &e.common.xdata;
            let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP))?;
            let source = x.get(at + 3).and_then(|t| u64::from_str_radix(t.value.trim(), 16).ok()).map_or(Handle(0), Handle);
            Some(Built { handle: e.handle, part, level: made_level(e).unwrap_or_default(), source, mesh: m.clone() })
        })
        .collect()
}

/// A mesh's plan footprint: its bottom faces (convex polygons), else the bottom edges of its
/// faces; and its height range.
struct Footprint {
    polys: Vec<Vec<Vec2>>,
    segs: Vec<(Vec2, Vec2)>,
    z0: f64,
    z1: f64,
}

fn footprint(m: &Mesh) -> Footprint {
    let (z0, z1) = m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.z), hi.max(v.z)));
    let tol = ((z1 - z0).abs() * 1e-6).max(1e-9);
    let at = |i: &u32| m.vertices.get(*i as usize).copied();
    let mut polys = Vec::new();
    let mut segs = Vec::new();
    for f in &m.faces {
        let vs: Vec<Vec3> = f.v.iter().filter_map(at).collect();
        let low: Vec<Vec2> = vs.iter().filter(|v| (v.z - z0).abs() <= tol).map(|v| v.xy()).collect();
        if low.len() == vs.len() && low.len() >= 3 {
            polys.push(low);
        } else if let [a, b] = low[..]
            && a.dist(b) > 1e-9
        {
            segs.push((a, b));
        }
    }
    if !polys.is_empty() {
        segs.clear();
    }
    Footprint { polys, segs, z0, z1 }
}

impl Footprint {
    /// Whether a plan point lies on it (within `tol`).
    fn covers(&self, p: Vec2, tol: f64) -> bool {
        self.polys.iter().any(|q| cadaddy_geom::point_in_polygon(q, p) || edges(q).any(|(a, b)| seg_dist(p, a, b) <= tol))
            || self.segs.iter().any(|(a, b)| seg_dist(p, *a, *b) <= tol)
    }
    fn bounds(&self) -> Bounds2 {
        Bounds2::from_points(self.polys.iter().flatten().copied().chain(self.segs.iter().flat_map(|(a, b)| [*a, *b])))
    }
    /// Its thickness: the shortest edge of its polygons.
    fn thickness(&self) -> f64 {
        self.polys.iter().flat_map(|q| edges(q).map(|(a, b)| a.dist(b))).filter(|l| *l > 1e-9).fold(f64::INFINITY, f64::min)
    }
}

fn edges(q: &[Vec2]) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    q.iter().copied().zip(q.iter().copied().cycle().skip(1))
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    let t = if l2 > 1e-18 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + d * t)
}

/// The part of convex polygon `subject` inside convex polygon `clip` (Sutherland–Hodgman).
fn clip_convex(subject: &[Vec2], clip: &[Vec2]) -> Vec<Vec2> {
    let ccw = cadaddy_geom::shoelace(clip) >= 0.0;
    let inside = |a: Vec2, b: Vec2, p: Vec2| {
        let c = (b - a).cross(p - a);
        if ccw { c >= -1e-12 } else { c <= 1e-12 }
    };
    let mut out: Vec<Vec2> = subject.to_vec();
    for (a, b) in edges(clip) {
        if out.is_empty() {
            break;
        }
        let input = std::mem::take(&mut out);
        for (k, &p) in input.iter().enumerate() {
            let q = input.get((k + input.len() - 1) % input.len()).copied().unwrap_or(p);
            let (pin, qin) = (inside(a, b, p), inside(a, b, q));
            let cross = |q: Vec2, p: Vec2| {
                let (r, s) = (p - q, b - a);
                let den = r.cross(s);
                if den.abs() < 1e-18 { p } else { q + r * ((a - q).cross(s) / den) }
            };
            if pin {
                if !qin {
                    out.push(cross(q, p));
                }
                out.push(p);
            } else if qin {
                out.push(cross(q, p));
            }
        }
    }
    out
}

// ---------------- the plan ----------------

/// A session holding the plan alone (nothing a build made), for tracing rooms around their tags
/// without the generated walls closing the gaps.
pub(crate) fn plan_session(s: &Session) -> Result<Session> {
    let mut d = (*s.doc()?).clone();
    let made: Vec<Handle> = d.model.iter().filter(|e| made_as(e).is_some()).map(|e| e.handle).collect();
    for h in made {
        d.model.remove(h);
    }
    let mut t = Session::empty();
    t.open_drawing(d, "Plan", None);
    Ok(t)
}

fn units(sp: &BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

/// The plan's doors and windows as WALLGEN reads them (swings, blocks by name, the spec's
/// families and door and window layers).
fn plan_openings(d: &Drawing, sp: &BuildingSpec, ents: &[&Arc<Entity>]) -> Vec<super::perceive::geometry::Opening> {
    use super::perceive::geometry::{openings, segments};
    let upf = units(sp);
    let mut ops = openings(d, ents, &segments(ents), upf / 12.0);
    let mut fitted = HashMap::new();
    super::wallgen::fit_families(d, sp, ents, &mut ops, &mut fitted, upf);
    ops
}

/// A room traced around its name tag.
struct Room {
    tag: Handle,
    name: String,
    at: Vec2,
    outline: Option<Vec<Vec2>>,
}

/// Every room name tag (names, not numbers) and the outline around it, if it closes.
fn rooms(plan: &Session, sp: &BuildingSpec) -> Result<Vec<Room>> {
    let d = plan.doc()?;
    let ents = plan_objects(d);
    let walls = super::bspec::room_plan(plan)?;
    let walls = walls.as_ref().unwrap_or(plan);
    let mut out = Vec::new();
    for (h, t, at) in room_texts(sp, &ents) {
        let tagged = sp.layers.iter().any(|r| r.role == Role::RoomTag);
        let named = t.chars().filter(|c| c.is_alphabetic()).count() >= 2 && !super::perceive::geometry::room_number(&t);
        if !named || (!tagged && !sp.rooms.iter().any(|r| names_room(r, &t))) {
            continue;
        }
        if out.len() >= 2_000 {
            break;
        }
        let outline = super::hatch::loops_at(walls, at)
            .ok()
            .and_then(|loops| loops.iter().find(|l| l.outer).or(loops.first()).cloned())
            .map(|l| cadaddy_geom::Polyline { vertices: l.vertices, closed: true }.tessellate(1e-3))
            .filter(|q| q.len() >= 3);
        out.push(Room { tag: h, name: t, at, outline });
    }
    Ok(out)
}

// ---------------- model.issues ----------------

struct Finding {
    kind: &'static str,
    message: String,
    level: Option<String>,
    at: Option<Vec2>,
    handles: Vec<Handle>,
}

impl Finding {
    fn json(&self) -> Value {
        let mut v = json!({
            "kind": self.kind,
            "message": self.message,
            "handles": self.handles.iter().take(MAX_HANDLES).map(|h| h.hex()).collect::<Vec<_>>(),
        });
        if let Some(l) = &self.level {
            v["level"] = json!(l);
        }
        if let Some(p) = self.at {
            v["at"] = super::perceive::pt(p);
        }
        v
    }
}

fn issues(s: &Session) -> Result<Vec<Finding>> {
    let (sp, _) = current(s, "model.issues")?;
    let upf = units(&sp);
    let plan = plan_session(s)?;
    let pd = plan.doc()?;
    let d = s.doc()?;
    let ents = plan_objects(pd);
    let made = built(d);
    let mut out: Vec<Finding> = Vec::new();
    let mut push = |f: Finding| {
        if out.len() < MAX_FINDINGS {
            out.push(f);
        }
    };
    if made.is_empty() {
        push(Finding {
            kind: "not_built",
            message: "Nothing has been built from the plan yet (run model.build).".into(),
            level: None,
            at: None,
            handles: Vec::new(),
        });
    }
    // Rooms that leak.
    let rs = rooms(&plan, &sp)?;
    let mut reported: Vec<(Handle, Handle)> = Vec::new();
    for r in &rs {
        let level = level_at(&sp.levels, r.at.x, r.at.y).map(|l| l.name.clone());
        match &r.outline {
            None => push(Finding {
                kind: "leak",
                message: format!("{}: no closed outline around the room (a gap in its walls lets it out)", r.name),
                level,
                at: Some(r.at),
                handles: vec![r.tag],
            }),
            Some(q) => {
                for o in rs.iter().filter(|o| o.tag != r.tag && !o.name.eq_ignore_ascii_case(&r.name) && cadaddy_geom::point_in_polygon(q, o.at)) {
                    let key = if r.tag.0 < o.tag.0 { (r.tag, o.tag) } else { (o.tag, r.tag) };
                    if reported.contains(&key) {
                        continue;
                    }
                    reported.push(key);
                    push(Finding {
                        kind: "leak",
                        message: format!("{} runs into {}: their outline is one (a gap joins them)", r.name, o.name),
                        level: level.clone(),
                        at: Some(r.at),
                        handles: vec![r.tag, o.tag],
                    });
                }
            }
        }
    }
    // Doors and windows of the plan that no wall took.
    if !made.is_empty() {
        for o in plan_openings(pd, &sp, &ents) {
            let level = level_at(&sp.levels, o.at.x, o.at.y);
            if level.is_some_and(|l| l.sheet.is_some()) {
                continue;
            }
            let taken = made.iter().any(|b| b.source == o.handle && matches!(b.part.as_str(), "door" | "window"));
            if !taken {
                push(Finding {
                    kind: "opening_not_in_wall",
                    message: format!("a {} at {} isn't in any wall, so it wasn't built", o.kind, fmt_pt(o.at)),
                    level: level.map(|l| l.name.clone()),
                    at: Some(o.at),
                    handles: vec![o.handle],
                });
            }
        }
    }
    // Walls without height, and walls overlapping each other.
    let walls: Vec<(&Built, Footprint)> = made.iter().filter(|b| b.part == "wall").take(MAX_WALLS).map(|b| (b, footprint(&b.mesh))).collect();
    for (b, f) in &walls {
        if f.z1 - f.z0 < upf || !(f.z1 - f.z0).is_finite() {
            push(Finding {
                kind: "wall_without_height",
                message: format!("a wall {} high", feet(f.z1 - f.z0, upf)),
                level: Some(b.level.clone()),
                at: Some(f.bounds().center()).filter(|c| c.is_finite()),
                handles: vec![b.handle, b.source],
            });
        }
    }
    let boxes: Vec<Bounds2> = walls.iter().map(|(_, f)| f.bounds()).collect();
    for i in 0..walls.len() {
        for j in i + 1..walls.len() {
            let (Some((a, fa)), Some((b, fb)), Some(ba), Some(bb)) = (walls.get(i), walls.get(j), boxes.get(i), boxes.get(j)) else { continue };
            if a.level != b.level || !ba.intersects(bb) || fa.z1 <= fb.z0 || fb.z1 <= fa.z0 {
                continue;
            }
            let area: f64 = fa.polys.iter().flat_map(|p| fb.polys.iter().map(move |q| cadaddy_geom::shoelace(&clip_convex(p, q)).abs())).sum();
            // Walls meeting at a corner or a tee share about one thickness squared.
            let (ta, tb) = (fa.thickness(), fb.thickness());
            if ta.is_finite() && tb.is_finite() && area > 1.5 * ta.max(tb) * ta.max(tb) {
                push(Finding {
                    kind: "walls_overlap",
                    message: format!("two walls overlap over {:.1} sq ft", area / (upf * upf)),
                    level: Some(a.level.clone()),
                    at: Some(ba.intersection(bb).center()).filter(|c| c.is_finite()),
                    handles: vec![a.handle, b.handle],
                });
            }
        }
    }
    // Fixtures above their room's ceiling or below the floor.
    let scene = cadaddy_walk3d::derive(pd, &sp.to_rules());
    let suf = if scene.units_per_foot.is_finite() && scene.units_per_foot > 0.0 { scene.units_per_foot } else { upf };
    for dev in scene.devices.iter().take(100_000) {
        let Some(l) = scene.levels.get(dev.level) else { continue };
        let sheet = l.base + Vec2::new(dev.pos.x, dev.pos.y) * suf;
        // The room it's in (of rooms sharing an outline, the one whose tag is nearest).
        let room = rs
            .iter()
            .filter(|r| r.outline.as_ref().is_some_and(|q| cadaddy_geom::point_in_polygon(q, sheet)))
            .min_by(|a, b| a.at.dist(sheet).total_cmp(&b.at.dist(sheet)));
        let ceiling = room.and_then(|r| sp.rooms.iter().find(|x| names_room(x, &r.name))).and_then(|x| x.ceiling_ft).unwrap_or(l.ceiling_ft);
        let h = dev.pos.z - l.elevation_ft;
        let at = room.map_or(String::new(), |r| format!(" in {}", r.name));
        if h > ceiling + 0.05 {
            push(Finding {
                kind: "fixture_above_ceiling",
                message: format!("{} at {} is above the {} ceiling{at}", dev.block, feet_str(h), feet_str(ceiling)),
                level: Some(l.name.clone()),
                at: Some(sheet),
                handles: vec![dev.handle],
            });
        } else if h < -0.05 {
            push(Finding {
                kind: "fixture_below_floor",
                message: format!("{} at {} is below the floor{at}", dev.block, feet_str(h)),
                level: Some(l.name.clone()),
                at: Some(sheet),
                handles: vec![dev.handle],
            });
        }
    }
    // Levels that don't match.
    let mut levels: Vec<&cadaddy_walk3d::Level> = sp.levels.iter().collect();
    levels.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    for w in levels.windows(2) {
        if let [lo, hi] = w {
            let gap = hi.elevation_ft - (lo.elevation_ft + lo.height_ft);
            if gap > 0.01 {
                push(Finding {
                    kind: "level_unmatched",
                    message: format!("{} starts {} above the top of {}", hi.name, feet_str(gap), lo.name),
                    level: Some(hi.name.clone()),
                    at: None,
                    handles: Vec::new(),
                });
            }
        }
    }
    for l in &sp.levels {
        if l.sheet.is_some() {
            push(Finding {
                kind: "level_unmatched",
                message: format!("{} is on another sheet and isn't built here", l.name),
                level: Some(l.name.clone()),
                at: None,
                handles: Vec::new(),
            });
            continue;
        }
        let on = ents
            .iter()
            .filter(|e| sp.role(&e.common.layer).is_some_and(|r| r.role != Role::Ignore))
            .any(|e| centre(pd, e).and_then(|c| level_at(&sp.levels, c.x, c.y)).is_some_and(|x| x.name.eq_ignore_ascii_case(&l.name)));
        if !on {
            push(Finding {
                kind: "level_unmatched",
                message: format!("{} has no plan objects (check its region and the layer roles)", l.name),
                level: Some(l.name.clone()),
                at: None,
                handles: Vec::new(),
            });
        }
    }
    let mut stale: HashMap<String, Vec<Handle>> = HashMap::new();
    for b in &made {
        if !b.level.is_empty() && sp.level(&b.level).is_none() {
            stale.entry(b.level.clone()).or_default().push(b.handle);
        }
    }
    let mut stale: Vec<(String, Vec<Handle>)> = stale.into_iter().collect();
    stale.sort();
    for (name, hs) in stale {
        push(Finding {
            kind: "level_unmatched",
            message: format!("{} objects were built for level {name}, which the spec doesn't have (build again)", hs.len()),
            level: Some(name),
            at: None,
            handles: hs,
        });
    }
    Ok(out)
}

fn feet(v: f64, upf: f64) -> String {
    feet_str(v / upf)
}

/// Feet as feet and inches (`9'-6"`).
fn feet_str(ft: f64) -> String {
    if !ft.is_finite() {
        return "?".into();
    }
    let inches = (ft.abs() * 12.0).round();
    let sign = if ft < 0.0 && inches > 0.0 { "-" } else { "" };
    format!("{sign}{}'-{}\"", (inches / 12.0).floor(), inches % 12.0)
}

fn fmt_pt(p: Vec2) -> String {
    format!("{:.1},{:.1}", p.x, p.y)
}

fn run_issues(s: &mut Session, p: &Value) -> Result<Value> {
    let kind = str_param(p, "kind");
    let all = issues(s)?;
    let mut by: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for f in &all {
        *by.entry(f.kind).or_insert(0) += 1;
    }
    let list: Vec<Value> = all.iter().filter(|f| kind.is_none_or(|k| k.eq_ignore_ascii_case(f.kind))).map(Finding::json).collect();
    Ok(json!({ "total": list.len(), "issues": list, "byKind": by }))
}

// ---------------- images ----------------

fn size(p: &Value, w: u32, h: u32) -> (u32, u32) {
    let get = |k: &str, d: u32| p.get(k).and_then(Value::as_u64).map_or(d, |v| u32::try_from(v).unwrap_or(MAX_PX)).clamp(16, MAX_PX);
    (get("width", w), get("height", h))
}

/// The PNG written to `path`, or returned as base64.
fn deliver(png: Vec<u8>, p: &Value, out: &mut Value) -> Result<()> {
    match str_param(p, "path") {
        Some(path) => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                crate::writefile::write(path, &png).map_err(|e| crate::EngineError::Other(format!("{path}: {e}")))?;
                out["path"] = json!(path);
            }
            #[cfg(target_arch = "wasm32")]
            {
                let _ = path;
                out["png"] = json!(super::file::base64_encode(&png));
            }
        }
        None => out["png"] = json!(super::file::base64_encode(&png)),
    }
    Ok(())
}

fn vec3(p: &Value, key: &str) -> Result<Option<Vec3>> {
    let Some(v) = p.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    let n: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    match n[..] {
        [x, y, z] if x.is_finite() && y.is_finite() && z.is_finite() => Ok(Some(Vec3::new(x, y, z))),
        [x, y] if x.is_finite() && y.is_finite() => Ok(Some(Vec3::new(x, y, 0.0))),
        _ => Err(bad("model.render", format!("`{key}` must be [x, y, z]"))),
    }
}

fn run_render(s: &mut Session, p: &Value) -> Result<Value> {
    use cadaddy_render::shade::{VisualStyle, fit, rasterize, scene};
    let (w, h) = size(p, 800, 600);
    let style = match str_param(p, "style") {
        Some(n) => VisualStyle::parse(n).ok_or_else(|| bad("model.render", format!("unknown style {n}")))?,
        None => VisualStyle::Realistic,
    };
    let (sp, _) = current(s, "model.render")?;
    let upf = units(&sp);
    let d = s.doc()?;
    let made = built(d);
    if made.is_empty() {
        return Err(bad("model.render", "nothing has been built yet (run model.build)"));
    }
    let level = str_param(p, "level")
        .map(|n| sp.level(n).map(|l| l.name.clone()).ok_or_else(|| bad("model.render", format!("there is no level named {n}"))))
        .transpose()?;
    // A room: its outline on the plan, and its level.
    let mut room: Option<(Vec<Vec2>, String)> = None;
    if let Some(name) = str_param(p, "room") {
        let plan = plan_session(s)?;
        let rs = rooms(&plan, &sp)?;
        let spec_room = sp.rooms.iter().find(|r| names_room(r, name));
        let found = rs
            .iter()
            .find(|r| r.name.eq_ignore_ascii_case(name.trim()) || spec_room.is_some_and(|x| names_room(x, &r.name)))
            .ok_or_else(|| bad("model.render", format!("no room tag reads {name}")))?;
        let outline = found.outline.clone().ok_or_else(|| bad("model.render", format!("{name} has no closed outline (see model.issues)")))?;
        let lv = spec_room
            .and_then(|r| r.level.clone())
            .or_else(|| level_at(&sp.levels, found.at.x, found.at.y).map(|l| l.name.clone()))
            .unwrap_or_default();
        room = Some((outline, lv));
    }
    let eye = vec3(p, "eye")?;
    let target = vec3(p, "target")?;
    // Only the built model: a level's or room's objects without their ceilings (to see in).
    let only = level.clone().or_else(|| room.as_ref().map(|r| r.1.clone()));
    let keep: Vec<Handle> = made
        .iter()
        .filter(|b| only.as_ref().is_none_or(|l| b.level.eq_ignore_ascii_case(l) && b.part != "ceiling" && b.part != "ground"))
        .map(|b| b.handle)
        .collect();
    let mut md = Drawing::new_imperial();
    md.layers = d.layers.clone();
    for h in &keep {
        if let Some(e) = d.model.get(*h) {
            md.model.push((**e).clone());
        }
    }
    let ext = md.extents(&Space::Model);
    let size = ext.width().max(ext.height());
    let tol = if size.is_finite() && size > 0.0 { size / 1500.0 } else { 1.0 };
    let bg = cadaddy_color::Rgb(255, 255, 255);
    let mut sc = scene(&md, &Space::Model, tol, bg);
    // Half-spaces `n · p >= c` the view keeps: a room's box (a foot round its outline), the
    // space in front of a camera.
    let mut keep_in: Vec<(Vec3, f64)> = Vec::new();
    if let Some((outline, _)) = &room {
        let b = Bounds2::from_points(outline.iter().copied()).expand(upf);
        keep_in.extend([
            (Vec3::new(1.0, 0.0, 0.0), b.min.x),
            (Vec3::new(-1.0, 0.0, 0.0), -b.max.x),
            (Vec3::new(0.0, 1.0, 0.0), b.min.y),
            (Vec3::new(0.0, -1.0, 0.0), -b.max.y),
        ]);
    }
    let dir = match (eye, target) {
        (Some(e), Some(t)) => {
            let fwd = (t - e).normalized();
            if !fwd.is_finite() {
                return Err(bad("model.render", "`eye` and `target` must differ"));
            }
            keep_in.push((fwd, fwd.dot(e)));
            e - t
        }
        (Some(_), None) | (None, Some(_)) => return Err(bad("model.render", "a camera pose needs both `eye` and `target`")),
        _ => vec3(p, "dir")?.unwrap_or(Vec3::new(-1.0, -1.0, 1.0)),
    };
    if !keep_in.is_empty() {
        let tris = std::mem::take(&mut sc.tris);
        for t in tris {
            let poly = clip_tri(&t.p, &keep_in);
            for k in 1..poly.len().saturating_sub(1) {
                if let (Some(a), Some(b), Some(c)) = (poly.first(), poly.get(k), poly.get(k + 1)) {
                    sc.tris.push(cadaddy_render::shade::Tri::plain([*a, *b, *c], t.color));
                }
            }
        }
        let lines = std::mem::take(&mut sc.lines);
        for l in lines {
            if let Some((a, b)) = keep_seg(l.a, l.b, &keep_in) {
                sc.lines.push(cadaddy_render::shade::Line3 { a, b, ..l });
            }
        }
    }
    let proj = Projection::new(dir);
    if sc.tris.is_empty() && sc.lines.is_empty() {
        return Err(bad("model.render", "nothing of the model is in view"));
    }
    let view = fit(&sc, proj, w, h).ok_or_else(|| bad("model.render", "nothing to fit in the view"))?;
    let raster = rasterize(&sc, &view, style, bg);
    let png = raster.encode_png().ok_or_else(|| bad("model.render", "the image couldn't be encoded"))?;
    let mut out = json!({ "width": view.width_px, "height": view.height_px, "triangles": sc.tris.len() });
    deliver(png, p, &mut out)?;
    Ok(out)
}

/// The part of a triangle in all the half-spaces (a convex polygon; clipped plane by plane).
fn clip_tri(t: &[Vec3; 3], planes: &[(Vec3, f64)]) -> Vec<Vec3> {
    let mut poly: Vec<Vec3> = t.to_vec();
    for (n, c) in planes {
        if poly.is_empty() {
            break;
        }
        let input = std::mem::take(&mut poly);
        for (k, &p) in input.iter().enumerate() {
            let q = input.get((k + input.len() - 1) % input.len()).copied().unwrap_or(p);
            let (dp, dq) = (n.dot(p) - c, n.dot(q) - c);
            if (dp >= 0.0) != (dq >= 0.0) {
                poly.push(q + (p - q) * (dq / (dq - dp)));
            }
            if dp >= 0.0 {
                poly.push(p);
            }
        }
    }
    poly
}

/// The part of a segment in all the half-spaces.
fn keep_seg(mut a: Vec3, mut b: Vec3, planes: &[(Vec3, f64)]) -> Option<(Vec3, Vec3)> {
    for (n, c) in planes {
        let (da, db) = (n.dot(a) - c, n.dot(b) - c);
        match (da >= 0.0, db >= 0.0) {
            (false, false) => return None,
            (true, false) => b = a + (b - a) * (da / (da - db)),
            (false, true) => a = a + (b - a) * (da / (da - db)),
            _ => {}
        }
    }
    Some((a, b))
}

// ---------------- model.overlay ----------------

fn run_overlay(s: &mut Session, p: &Value) -> Result<Value> {
    use cadaddy_doc::color::Color;
    let (w, h) = size(p, 1200, 900);
    let (sp, _) = current(s, "model.overlay")?;
    let upf = units(&sp);
    let only = str_param(p, "level")
        .map(|n| sp.level(n).map(|l| l.name.clone()).ok_or_else(|| bad("model.overlay", format!("there is no level named {n}"))))
        .transpose()?;
    let d = s.doc()?;
    let made: Vec<Built> = built(d).into_iter().filter(|b| only.as_ref().is_none_or(|l| b.level.eq_ignore_ascii_case(l))).collect();
    let ents: Vec<&Arc<Entity>> = plan_objects(d)
        .into_iter()
        .filter(|e| {
            only.as_ref().is_none_or(|l| centre(d, e).and_then(|c| level_at(&sp.levels, c.x, c.y)).is_some_and(|x| x.name.eq_ignore_ascii_case(l)))
        })
        .collect();
    let walls: Vec<(&Built, Footprint)> = made.iter().filter(|b| b.part == "wall").map(|b| (b, footprint(&b.mesh))).collect();
    let tol = upf / 12.0;
    let mut diffs: Vec<Value> = Vec::new();
    let mut red: Vec<EntityKind> = Vec::new();
    // Plan walls nothing was built on.
    let wall_ents: Vec<&Arc<Entity>> = ents.iter().copied().filter(|e| sp.role(&e.common.layer).is_some_and(|r| r.role == Role::Wall)).collect();
    if !made.is_empty() {
        for e in &wall_ents {
            for sg in super::perceive::geometry::segments(&[*e]) {
                let mid = (sg.a + sg.b) * 0.5;
                if !walls.iter().any(|(_, f)| f.covers(mid, tol)) && diffs.len() < MAX_FINDINGS {
                    diffs.push(json!({"kind": "wall_not_built", "handle": e.handle.hex(), "at": super::perceive::pt(mid)}));
                    red.push(EntityKind::Line(cadaddy_doc::Line { a: Vec3::new(sg.a.x, sg.a.y, 0.0), b: Vec3::new(sg.b.x, sg.b.y, 0.0) }));
                }
            }
        }
        // Openings that weren't built.
        for o in plan_openings(d, &sp, &ents) {
            if !made.iter().any(|b| b.source == o.handle && matches!(b.part.as_str(), "door" | "window")) && diffs.len() < MAX_FINDINGS {
                diffs.push(json!({"kind": "opening_not_built", "handle": o.handle.hex(), "at": super::perceive::pt(o.at)}));
                red.push(EntityKind::Circle(cadaddy_doc::Circle { center: Vec3::new(o.at.x, o.at.y, 0.0), radius: (o.width / 2.0).max(upf / 2.0) }));
            }
        }
        // Walls built where the plan has no wall line.
        let segs = super::perceive::geometry::segments(&wall_ents);
        for (b, f) in &walls {
            let c = f.bounds().center();
            let reach = f.thickness().min(2.0 * upf).max(tol);
            if c.is_finite() && !segs.iter().any(|sg| seg_dist(c, sg.a, sg.b) <= reach * 1.5) && diffs.len() < MAX_FINDINGS {
                diffs.push(json!({"kind": "wall_not_in_plan", "handle": b.handle.hex(), "at": super::perceive::pt(c)}));
                red.push(EntityKind::Circle(cadaddy_doc::Circle { center: Vec3::new(c.x, c.y, 0.0), radius: upf }));
            }
        }
    }
    // The picture: the plan in grey, footprints in blue (walls) and green (doors, windows), the
    // differences in red.
    let mut od = Drawing::new_imperial();
    let put = |od: &mut Drawing, kind: EntityKind, color: u8| {
        let _ = od.add(&Space::Model, Common { color: Color::Index(color), ..Common::default() }, kind);
    };
    for e in &ents {
        let mut c = (***e).clone();
        c.common.color = Color::Index(8);
        c.common.layer = "0".into();
        if !matches!(c.kind, EntityKind::Insert(_)) {
            let _ = od.add(&Space::Model, c.common, c.kind);
        }
    }
    od.blocks = d.blocks.clone();
    for e in ents.iter().filter(|e| matches!(e.kind, EntityKind::Insert(_))) {
        let mut c = (***e).clone();
        c.common.color = Color::Index(8);
        c.common.layer = "0".into();
        let _ = od.add(&Space::Model, c.common, c.kind);
    }
    let mut footprints = 0;
    for b in &made {
        let color = match b.part.as_str() {
            "wall" | "column" => 5,
            "door" | "window" | "curtainwall" => 3,
            _ => continue,
        };
        let f = footprint(&b.mesh);
        for q in f.polys.iter().take(10_000) {
            let vertices = q.iter().map(|v| PolyVertex { p: *v, bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
            put(
                &mut od,
                EntityKind::LwPolyline(cadaddy_doc::LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }),
                color,
            );
        }
        for (a, z) in f.segs.iter().take(10_000) {
            put(&mut od, EntityKind::Line(cadaddy_doc::Line { a: Vec3::new(a.x, a.y, 0.0), b: Vec3::new(z.x, z.y, 0.0) }), color);
        }
        footprints += 1;
    }
    for k in red {
        put(&mut od, k, 1);
    }
    let ext = od.extents(&Space::Model);
    if ext.is_empty() {
        return Err(bad("model.overlay", "there is no plan to draw"));
    }
    let all: Vec<&Entity> = od.model.iter().map(|e| e.as_ref()).collect();
    let list = cadaddy_render::build_entities(&od, all, &cadaddy_render::Options::default());
    let view = cadaddy_render::raster::View::fit(&ext, w, h, 0.03);
    let ro = cadaddy_render::raster::RasterOptions { background: cadaddy_color::Rgb(255, 255, 255), hairline: 1.5, px_per_mm: 0.0, antialias: true };
    let png = cadaddy_render::raster::render_png(&list, &view, &ro).ok_or_else(|| bad("model.overlay", "the image couldn't be drawn"))?;
    let mut out = json!({ "width": w, "height": h, "footprints": footprints, "differences": diffs });
    deliver(png, p, &mut out)?;
    Ok(out)
}

// ---------------- model.counts ----------------

fn run_counts(s: &mut Session, _p: &Value) -> Result<Value> {
    let (sp, _) = current(s, "model.counts")?;
    let plan = plan_session(s)?;
    let pd = plan.doc()?;
    let d = s.doc()?;
    let ents = plan_objects(pd);
    let made = built(d);
    let scene = cadaddy_walk3d::derive(pd, &sp.to_rules());
    let mut rows: Vec<Value> = Vec::new();
    let ops = plan_openings(pd, &sp, &ents);
    let mut all_match = true;
    for (li, l) in sp.levels.iter().enumerate() {
        let here = |p: Vec2| level_at(&sp.levels, p.x, p.y).is_some_and(|x| x.name.eq_ignore_ascii_case(&l.name));
        let plan_doors = ops.iter().filter(|o| o.kind == "door" && here(o.at)).count();
        let plan_windows = ops.iter().filter(|o| o.kind == "window" && here(o.at)).count();
        let plan_fixtures = ents
            .iter()
            .filter(|e| matches!(e.kind, EntityKind::Insert(_)))
            .filter(|e| sp.role(&e.common.layer).is_some_and(|r| matches!(r.role, Role::Fixture | Role::Device)))
            .filter(|e| centre(pd, e).is_some_and(here))
            .count();
        let on = |part: &str| made.iter().filter(|b| b.part == part && b.level.eq_ignore_ascii_case(&l.name)).count();
        let model_fixtures =
            scene.levels.iter().position(|x| x.name == l.name).map_or(0, |i| scene.devices.iter().filter(|dv| dv.level == i).count());
        let _ = li;
        let ok = plan_doors == on("door") && plan_windows == on("window") && plan_fixtures == model_fixtures;
        all_match &= ok;
        rows.push(json!({
            "level": l.name,
            "plan": {"doors": plan_doors, "windows": plan_windows, "fixtures": plan_fixtures},
            "model": {"doors": on("door"), "windows": on("window"), "fixtures": model_fixtures},
            "match": ok,
        }));
    }
    Ok(json!({ "levels": rows, "match": all_match }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_squares() {
        let sq = |x: f64, y: f64, s: f64| vec![Vec2::new(x, y), Vec2::new(x + s, y), Vec2::new(x + s, y + s), Vec2::new(x, y + s)];
        let a = sq(0.0, 0.0, 2.0);
        assert!((cadaddy_geom::shoelace(&clip_convex(&a, &sq(1.0, 1.0, 2.0))).abs() - 1.0).abs() < 1e-9);
        let mut cw = sq(1.0, 0.0, 4.0);
        cw.reverse();
        assert!((cadaddy_geom::shoelace(&clip_convex(&a, &cw)).abs() - 2.0).abs() < 1e-9);
        assert!(clip_convex(&a, &sq(5.0, 5.0, 1.0)).len() < 3);
        assert_eq!(feet_str(9.5), "9'-6\"");
        assert_eq!(feet_str(-0.5), "-0'-6\"");
    }
}
