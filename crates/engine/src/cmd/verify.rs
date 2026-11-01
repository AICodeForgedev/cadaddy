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

use std::collections::{BTreeMap, HashMap};
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

/// Findings kept at most (of each kind: a filter by kind sees all of its own up to this), and
/// handles per finding.
const MAX_FINDINGS: usize = 20_000;
const MAX_HANDLES: usize = 50;
/// Walls compared for overlaps, at most (hostile drawings).
const MAX_WALLS: usize = 20_000;
/// Largest image side.
const MAX_PX: u32 = 4096;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("model.issues", "Model Issues", run_issues)
            .params("{kind?: leak|opening_not_in_wall|wall_without_height|walls_overlap|fixture_above_ceiling|fixture_below_floor|level_unmatched|not_built|outside_building, limit?: 1–1000 (100), cursor?: from `next`, bcf?: bool (a BCF topic per matching issue, up to 500), bcf_path?: .bcf file (those topics exported)} → {issues: [{kind, message, level?, at?, handles}] (a page: `limit` from `cursor`), total, byKind, next? (the cursor of the next page), bcf?: {created, topics, skipped, path?}}")
            .files(),
        CommandSpec::new("model.render", "Render Model View", run_render)
            .params("{force?: bool (replace an existing file; otherwise it is refused), path?: PNG file (else the PNG comes back as base64), output?: same as path, width?, height? (800×600), style?: bim (default: trade colours, shaded with edges, the shell ghosted from outside)|realistic|shaded|shadedwithedges|hidden|wireframe, ghost?: bool, level?: name (an axonometric of it), room?: name or number, eye?: [x,y,z], target?: [x,y,z] (a camera pose), dir?: [x,y,z] (view direction towards the viewer), around?: [x0,y0,x1,y1] (only the model inside this plan box: a close-up), mark?: [x,y] (a red post at this plan point on `level`'s floor)} → {path?|png, width, height, triangles}")
            .noundo(),
        CommandSpec::new("model.overlay", "Model Overlay", run_overlay)
            .params("{force?: bool (replace an existing file; otherwise it is refused), path?: PNG file (else base64), output?: same as path, width?, height? (1200×900), level?: name} → {path?|png, footprints, differences: [{kind: wall_not_built|opening_not_built|wall_not_in_plan, handle, at}]}")
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

/// The parts a building stands as: what a whole-building view frames, and what each level's
/// other built objects should stand in.
const SHELL: &[&str] = &["wall", "floor", "curtainwall", "column", "roof"];

/// How far past its level's shell a built object stands before it is a finding (a wall pack, a
/// condenser on its pad right by the wall are within it), feet.
const OUTSIDE_FT: f64 = 5.0;

/// How far past its level's shell a whole-building view frames (a condenser on its pad, a
/// canopy): 10 feet, or 5% of the shell's size when that is more.
fn shell_margin(b: &Bounds2, upf: f64) -> f64 {
    (10.0 * upf).max(0.05 * b.width().max(b.height()))
}

/// The plan box of each level's shell (its walls, floors, columns and roofs) where they are
/// built, and of all levels together.
pub(crate) fn shell_boxes(d: &Drawing) -> (Vec<(String, Bounds2)>, Bounds2) {
    let mut by: Vec<(String, Bounds2)> = Vec::new();
    let mut all = Bounds2::EMPTY;
    for e in d.model.iter() {
        let Some(part) = made_as(e) else { continue };
        if !SHELL.iter().any(|p| part.eq_ignore_ascii_case(p)) {
            continue;
        }
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if b.is_empty() || !(b.min.is_finite() && b.max.is_finite()) {
            continue;
        }
        all = all.union(&b);
        let level = made_level(e).unwrap_or_default();
        match by.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(&level)) {
            Some((_, x)) => *x = x.union(&b),
            None => by.push((level, b)),
        }
    }
    (by, all)
}

/// What a built object was read from, for a message: its sheet (`prop:SOURCE sheet=`), else its
/// own tag, else the text or layer of the plan object it was built from.
fn built_from(d: &Drawing, e: &Entity) -> String {
    let tag = super::bimlook::made_tag;
    let sheet = tag(e, "prop").and_then(|v| v.split_whitespace().find_map(|w| w.strip_prefix("sheet="))).map(str::to_string);
    let what = tag(e, "tag").map(str::to_string).filter(|t| !t.is_empty()).or_else(|| tag(e, "system").map(|v| format!("system {v}")));
    let source = super::bimlook::made_source(e).filter(|h| h.0 != 0).and_then(|h| d.model.get(h));
    let text = source.and_then(|x| match &x.kind {
        EntityKind::Text(t) => Some(t.value.clone()),
        EntityKind::MText(t) => Some(t.contents.clone()),
        _ => None,
    });
    let mut out: Vec<String> = Vec::new();
    if let Some(w) = text.or(what) {
        out.push(format!("\"{}\"", w.chars().take(40).collect::<String>()));
    }
    match (sheet, source) {
        (Some(sh), _) => out.push(format!("from {sh}")),
        (None, Some(x)) => out.push(format!("from layer {}", x.common.layer)),
        _ => {}
    }
    out.join(" ")
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

/// Two built walls overlapping (more than walls meeting at a corner or a tee share).
pub(crate) struct Overlap {
    pub level: String,
    pub area: f64,
    pub at: Vec2,
    pub handles: [Handle; 2],
}

/// Built walls overlapping each other: their boxes swept along x (each wall meets only the walls
/// whose boxes reach it), then the common area of their footprints.
fn overlaps(walls: &[(&Built, Footprint)]) -> Vec<Overlap> {
    let boxes: Vec<Bounds2> = walls.iter().map(|(_, f)| f.bounds()).collect();
    let mut order: Vec<usize> = (0..walls.len()).filter(|i| boxes.get(*i).is_some_and(|b| !b.is_empty() && b.min.is_finite())).collect();
    order.sort_by(|a, b| boxes.get(*a).map_or(0.0, |x| x.min.x).total_cmp(&boxes.get(*b).map_or(0.0, |x| x.min.x)));
    let mut out = Vec::new();
    for (k, &i) in order.iter().enumerate() {
        let (Some((a, fa)), Some(ba)) = (walls.get(i), boxes.get(i)) else { continue };
        for &j in order.iter().skip(k + 1) {
            let (Some((b, fb)), Some(bb)) = (walls.get(j), boxes.get(j)) else { continue };
            if bb.min.x > ba.max.x {
                break;
            }
            if a.level != b.level || !ba.intersects(bb) || fa.z1 <= fb.z0 || fb.z1 <= fa.z0 {
                continue;
            }
            let area: f64 = fa.polys.iter().flat_map(|p| fb.polys.iter().map(move |q| cadaddy_geom::shoelace(&clip_convex(p, q)).abs())).sum();
            // Walls meeting at a corner or a tee share about one thickness squared.
            let (ta, tb) = (fa.thickness(), fb.thickness());
            if ta.is_finite() && tb.is_finite() && area > 1.5 * ta.max(tb) * ta.max(tb) {
                out.push(Overlap { level: a.level.clone(), area, at: ba.intersection(bb).center(), handles: [a.handle, b.handle] });
                if out.len() >= MAX_FINDINGS {
                    return out;
                }
            }
        }
    }
    out
}

/// The built walls of a drawing that overlap each other (`model.clashes` reports them as
/// same-trade clashes).
pub(crate) fn wall_overlaps(d: &Drawing) -> Vec<Overlap> {
    let made = built(d);
    let walls: Vec<(&Built, Footprint)> = made.iter().filter(|b| b.part == "wall").take(MAX_WALLS).map(|b| (b, footprint(&b.mesh))).collect();
    overlaps(&walls)
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

/// A room traced around its name or number tag.
struct Room {
    tag: Handle,
    name: String,
    at: Vec2,
    outline: Option<Vec<Vec2>>,
    /// Tagged by its number only (`101`, `B12`): named by that number.
    number: bool,
}

/// Every room tag and the outline around it, if it closes: names, and numbers (`101`, `2104`,
/// `B12`) on a room tag layer or numbering a spec room. A number whose room a name tag also tags
/// (the name inside its outline, or near it when it has none) is that room's number, not another
/// room: the name's tag is traced instead.
fn rooms(plan: &Session, sp: &BuildingSpec) -> Result<Vec<Room>> {
    let d = plan.doc()?;
    let ents = plan_objects(d);
    let walls = super::bspec::room_plan(plan)?;
    let walls = walls.as_ref().unwrap_or(plan);
    let mut out = Vec::new();
    let tagged = sp.layers.iter().any(|r| r.role == Role::RoomTag);
    // The plan's outlines are made once for every tag (not once per tag: 60k objects).
    let mut cache = None;
    for (h, t, at) in room_texts(sp, &ents) {
        // Equipment and device tags (a panelboard label on a room tag layer) are no rooms.
        let named = super::perceive::geometry::room_name_score(&t) > 0;
        let number = !named && super::perceive::geometry::room_number(&t) && !super::perceive::geometry::equipment_tag(&t);
        if !(named || number) || (!tagged && !sp.rooms.iter().any(|r| names_room(r, &t))) {
            continue;
        }
        if out.len() >= 2_000 {
            break;
        }
        let outline = super::hatch::loops_at_lazy(walls, at, &mut cache)
            .ok()
            .and_then(|loops| loops.iter().find(|l| l.outer).or(loops.first()).cloned())
            .map(|l| cadaddy_geom::Polyline { vertices: l.vertices, closed: true }.tessellate(1e-3))
            .filter(|q| q.len() >= 3);
        out.push(Room { tag: h, name: t, at, outline, number });
    }
    // Tags of one room sit within a few feet of each other.
    let near = 4.0 * units(sp);
    let names: Vec<Vec2> = out.iter().filter(|r| !r.number).map(|r| r.at).collect();
    out.retain(|r| {
        !r.number
            || !names.iter().any(|&at| match &r.outline {
                Some(q) => cadaddy_geom::point_in_polygon(q, at),
                None => (at - r.at).len() <= near,
            })
    });
    Ok(out)
}

// ---------------- model.issues ----------------

#[derive(Clone)]
pub(crate) struct Finding {
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

fn issues(s: &Session) -> Result<(Vec<Finding>, BTreeMap<&'static str, usize>)> {
    // Each check's end on the progress sink (a big model's first check takes minutes).
    let clock = web_time::Instant::now();
    let step = |what: &str| super::guided::progress_line(&format!("  model.issues: {what} checked at {:.1} s", clock.elapsed().as_secs_f64()));
    let (sp, _) = current(s, "model.issues")?;
    let upf = units(&sp);
    let plan = plan_session(s)?;
    let pd = plan.doc()?;
    let d = s.doc()?;
    let ents = plan_objects(pd);
    let made = built(d);
    let mut kept = Kept::default();
    let mut push = |f: Finding| kept.push(f);
    if made.is_empty() {
        push(Finding {
            kind: "not_built",
            message: "Nothing has been built from the plan yet (run model.build).".into(),
            level: None,
            at: None,
            handles: Vec::new(),
        });
    }
    // Built objects off their level's building (a legend's equipment, a ceiling traced round a
    // grid bubble): each listed with what it was read from.
    let (shells, _) = shell_boxes(d);
    let mut off: Vec<((String, String, String), Vec<Handle>, Vec2, f64)> = Vec::new();
    for e in d.model.iter() {
        let Some(part) = made_as(e) else { continue };
        if part.eq_ignore_ascii_case("ground") || SHELL.iter().any(|p| part.eq_ignore_ascii_case(p)) {
            continue;
        }
        let level = made_level(e).unwrap_or_default();
        let Some((_, shell)) = shells.iter().find(|(n, _)| n.eq_ignore_ascii_case(&level)) else { continue };
        let Some(c) = centre(d, e) else { continue };
        let by = shell_dist(shell, c);
        if !by.is_finite() || by <= OUTSIDE_FT * upf {
            continue;
        }
        // MEP by what it is (duct, pipe, equipment).
        let part = super::bimlook::made_tag(e, "kind").filter(|k| !k.is_empty()).map_or(part, str::to_string);
        let key = (level, part, built_from(d, e));
        match off.iter_mut().find(|o| o.0 == key) {
            Some(o) => {
                o.1.push(e.handle);
                o.3 = o.3.max(by);
            }
            None => off.push((key, vec![e.handle], c, by)),
        }
    }
    for ((level, part, from), hs, at, by) in off {
        let what = if from.is_empty() { String::new() } else { format!(" {from}") };
        push(Finding {
            kind: "outside_building",
            message: format!("{} {part}{what} built {} outside {level}'s walls and floors", hs.len(), feet(by, upf)),
            level: Some(level),
            at: Some(at),
            handles: hs,
        });
    }
    step("objects outside the building");
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
                // Names run into names, numbers into numbers.
                let joined = |o: &&Room| {
                    o.tag != r.tag && o.number == r.number && !o.name.eq_ignore_ascii_case(&r.name) && cadaddy_geom::point_in_polygon(q, o.at)
                };
                for o in rs.iter().filter(joined) {
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
    step("rooms that leak");
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
    for o in overlaps(&walls) {
        push(Finding {
            kind: "walls_overlap",
            message: format!("two walls overlap over {:.1} sq ft", o.area / (upf * upf)),
            level: Some(o.level),
            at: Some(o.at).filter(|c| c.is_finite()),
            handles: o.handles.to_vec(),
        });
    }
    step("walls");
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
    step("fixtures");
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
    Ok((kept.out, kept.total))
}

/// Findings kept: every one counted by its kind, each kind kept up to [`MAX_FINDINGS`] (a kind
/// found past the cap doesn't crowd out the others).
#[derive(Default)]
struct Kept {
    out: Vec<Finding>,
    total: BTreeMap<&'static str, usize>,
}

impl Kept {
    fn push(&mut self, f: Finding) {
        let n = self.total.entry(f.kind).or_insert(0);
        *n += 1;
        if *n <= MAX_FINDINGS {
            self.out.push(f);
        }
    }
}

/// `model.issues`' findings for one revision of one drawing (the session keeps the last).
pub(crate) struct IssuesCache {
    uid: u64,
    revision: u64,
    found: Vec<Finding>,
    /// Every finding by its kind, also those past [`MAX_FINDINGS`].
    total: BTreeMap<&'static str, usize>,
}

/// The findings of the drawing as it is now: from the session's cache when it was checked at
/// this revision, else found and kept.
fn issues_now(s: &mut Session) -> Result<(Vec<Finding>, BTreeMap<&'static str, usize>)> {
    let (uid, revision) = s.state().map(|st| (st.uid, st.revision))?;
    if let Some(c) = s.issues_cache.as_ref().filter(|c| c.uid == uid && c.revision == revision) {
        return Ok((c.found.clone(), c.total.clone()));
    }
    let (found, total) = issues(s)?;
    s.issues_cache = Some(Box::new(IssuesCache { uid, revision, found: found.clone(), total: total.clone() }));
    Ok((found, total))
}

/// How far a plan point is outside a box (0 inside).
fn shell_dist(b: &Bounds2, p: Vec2) -> f64 {
    let dx = (b.min.x - p.x).max(p.x - b.max.x).max(0.0);
    let dy = (b.min.y - p.y).max(p.y - b.max.y).max(0.0);
    dx.hypot(dy)
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
    // Counted over every finding (kept or past the cap), filtered by kind before any cap.
    let (all, by) = issues_now(s)?;
    // A page at a time: a big model's issues would flood an agent's context.
    let limit =
        p.get("limit").and_then(Value::as_u64).map_or(ISSUES_PAGE, |n| usize::try_from(n).unwrap_or(MAX_ISSUES_PAGE).clamp(1, MAX_ISSUES_PAGE));
    let from = p.get("cursor").and_then(Value::as_u64).map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
    let of_kind = |k: &str| kind.is_none_or(|want| want.eq_ignore_ascii_case(k));
    let matching: Vec<&Finding> = all.iter().filter(|f| of_kind(f.kind)).collect();
    let found: usize = by.iter().filter(|(k, _)| of_kind(k)).map(|(_, n)| *n).sum();
    let list: Vec<Value> = matching.iter().skip(from).take(limit).map(|f| f.json()).collect();
    let mut out = json!({ "total": found, "issues": list, "byKind": by });
    // Past the cap: how many there are, and that the list stops.
    if found > matching.len() {
        out["truncated"] = json!({"found": found, "listed": matching.len(), "note": format!("only the first {MAX_FINDINGS} issues of each kind are kept (all of them are counted); fix those, then check again")});
    }
    if from.saturating_add(limit) < matching.len() {
        out["next"] = json!(from + limit);
    }
    // BCF in the same call: a topic per matching issue (its objects selected), then the file.
    let path = str_param(p, "bcf_path").map(str::trim).filter(|x| !x.is_empty()).map(str::to_string);
    if bool_or(p, "bcf", false) || path.is_some() {
        let picked: Vec<Value> = matching
            .iter()
            .take(MAX_BCF)
            .map(|f| {
                let mut labels = vec![json!(f.kind)];
                if let Some(l) = &f.level {
                    labels.push(json!(format!("level {l}")));
                }
                json!({
                    "title": f.message.chars().take(200).collect::<String>(), "description": f.message, "type": "Issue", "labels": labels,
                    "selection": f.handles.iter().filter(|h| h.0 != 0).take(MAX_HANDLES).map(|h| h.hex()).collect::<Vec<_>>(), "snapshot": false,
                })
            })
            .collect();
        let mut topics: Vec<Value> = Vec::new();
        for t in &picked {
            let r = s.execute("bcf.create", t).map_err(|e| bad("model.issues", format!("BCF: {e}")))?;
            topics.push(r["guid"].clone());
        }
        let mut bcf = json!({ "created": topics.len(), "topics": topics, "skipped": matching.len().saturating_sub(picked.len()) });
        if let Some(path) = path {
            let r =
                s.execute("bcf.export", &json!({ "path": path, "topics": topics })).map_err(|e| bad("model.issues", format!("BCF export: {e}")))?;
            bcf["path"] = r["path"].clone();
        }
        out["bcf"] = bcf;
    }
    Ok(out)
}

/// BCF topics one `model.issues` call makes, at most.
const MAX_BCF: usize = 500;

/// Issues per `model.issues` answer by default, and at most.
const ISSUES_PAGE: usize = 100;
const MAX_ISSUES_PAGE: usize = 1_000;

// ---------------- images ----------------

fn size(cmd: &str, p: &Value, w: u32, h: u32) -> Result<(u32, u32)> {
    let get = |k: &str, d: u32| -> Result<u32> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(d),
            Some(v) => v
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| (16..=MAX_PX).contains(v))
                .ok_or_else(|| bad(cmd, format!("`{k}` is 16 to {MAX_PX} pixels"))),
        }
    };
    Ok((get("width", w)?, get("height", h)?))
}

/// The PNG written to `path`, or returned as base64.
pub(crate) fn deliver(png: Vec<u8>, p: &Value, out: &mut Value) -> Result<()> {
    match str_param(p, "path").or_else(|| str_param(p, "output")) {
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
    super::exchange::guard_params(s, "model.render", p, &["path", "output"])?;
    use cadaddy_render::model3::{self, Aabb};
    use cadaddy_render::shade::{VisualStyle, fit, fit_bounds, rasterize};
    use std::sync::atomic::AtomicUsize;
    let (w, h) = size("model.render", p, 800, 600)?;
    // The BIM view's look unless another style is asked for.
    let bim = str_param(p, "style").is_none_or(|n| n.trim().eq_ignore_ascii_case("bim"));
    let style = match str_param(p, "style") {
        Some(n) if !bim => VisualStyle::parse(n).ok_or_else(|| bad("model.render", format!("unknown style {n} (bim, realistic, shaded…)")))?,
        _ => VisualStyle::ShadedEdges,
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
    if let Some(name) = str_param(p, "room")
        && let Some(r) = super::bspec::room_elements(d).into_iter().find(|r| {
            r.outline.len() >= 3
                && (r.name.eq_ignore_ascii_case(name.trim()) || r.number.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name.trim())))
        })
    {
        // A built room has its outline: no plan trace of every room.
        room = Some((r.outline.clone(), r.level.clone()));
    } else if let Some(name) = str_param(p, "room") {
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
    // A level's floor: the slab over it (the next level's floor, tagged with this one) is left
    // out like its ceilings.
    let floor_z = only.as_ref().and_then(|l| sp.level(l)).map(|l| l.elevation_ft * upf);
    let over = |b: &Built| {
        let z0 = b.mesh.vertices.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
        b.part == "floor" && floor_z.is_some_and(|f| z0.is_finite() && z0 > f + upf)
    };
    let keep: Vec<Handle> = made
        .iter()
        .filter(|b| only.as_ref().is_none_or(|l| b.level.eq_ignore_ascii_case(l) && b.part != "ceiling" && b.part != "ground" && !over(b)))
        .map(|b| b.handle)
        .collect();
    // The whole building: its storeys stacked, framed on what it stands as (walls, floors,
    // columns, roofs); a reader's stray object off the building (model.issues lists it as
    // outside_building) never shrinks the building to a speck in the corner of the view.
    let whole = only.is_none() && eye.is_none() && p.get("around").is_none_or(Value::is_null) && p.get("mark").is_none_or(Value::is_null);
    let md = if bim {
        // Trade colours; the shell ghosted for a view from outside (`ghost`, else as the view
        // has it), solid for a camera or a room seen from inside.
        let mut look = super::bimlook::Look::of(d);
        look.level = None;
        look.sheet = false;
        let inside = room.is_some() || eye.is_some();
        let ghost = match p.get("ghost").and_then(Value::as_bool) {
            Some(g) => g,
            None => !inside && super::bimlook::ghosting(d, &look),
        };
        // The whole building stacks its storeys; a level, a room, a plan box, a mark or a camera
        // are on the sheet's (and the built model's) own coordinates.
        let keep: std::collections::HashSet<Handle> = keep.iter().copied().collect();
        super::bimlook::compose(d, &look, ghost, whole, &|e, _| keep.contains(&e.handle))
    } else {
        let mut md = Drawing::new_imperial();
        md.layers = d.layers.clone();
        let offsets = if whole { super::bimlook::stack_offsets(d) } else { Vec::new() };
        for h in &keep {
            if let Some(e) = d.model.get(*h) {
                let mut c = (**e).clone();
                let level = made_level(e).unwrap_or_default();
                if let Some(o) = super::bimlook::stack_move(offsets.iter().find(|(n, _)| n.eq_ignore_ascii_case(&level)).map(|o| o.1), e) {
                    c.kind.transform(&cadaddy_geom::Mat3::translate(o));
                }
                md.model.push(c);
            }
        }
        md
    };
    // Frame for outlier-resistant view fitting: the building's shell (walls, floors, columns,
    // roofs) on the whole building or on a single level, expanded by a margin.  Elements outside
    // the frame are outliers (model.issues lists them as outside_building); the frame keeps the
    // building in the centre of the image and outliers don't shrink it to a speck.
    let (frame, outliers) = if whole || only.is_some() {
        let (by_level, all_shell) = shell_boxes(&md);
        let shell = if whole {
            all_shell
        } else if let Some(ln) = only.as_ref() {
            by_level.iter().find(|(n, _)| n.eq_ignore_ascii_case(ln)).map_or(Bounds2::EMPTY, |(_, b)| *b)
        } else {
            Bounds2::EMPTY
        };
        let framed = (!shell.is_empty()).then(|| shell.expand(shell_margin(&shell, upf)));
        // Count outliers: built objects whose plan bounds are entirely outside the frame.
        let n_outliers = framed.as_ref().map_or(0, |f| {
            md.model
                .iter()
                .filter(|e| {
                    let b = cadaddy_doc::entity_bounds(&md, e, 0);
                    !b.is_empty() && (b.max.x < f.min.x || b.min.x > f.max.x || b.max.y < f.min.y || b.min.y > f.max.y)
                })
                .count()
        });
        (framed, n_outliers)
    } else {
        (None, 0)
    };
    // Build the 3D model with instancing, LOD and spatial chunking instead of tessellating every
    // entity into a flat triangle list.  Block references that share a block become instances
    // (one set of triangles placed many times); each prototype has three levels of detail (full,
    // simplified, bounding box); the base is split into spatial chunks for view culling.
    let ext = md.extents(&Space::Model);
    let size = ext.width().max(ext.height());
    let tol = if size.is_finite() && size > 0.0 { size / 1500.0 } else { 1.0 };
    let bg = cadaddy_color::Rgb(255, 255, 255);
    let (progress, total) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let m3 = model3::build(&md, &Space::Model, tol, bg, &progress, &total);
    // Half-spaces `n · p >= c` the view keeps: a room's box (a foot round its outline), the
    // space in front of a camera, the building's frame.
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
    if let Some(b) = &frame {
        keep_in.extend([
            (Vec3::new(1.0, 0.0, 0.0), b.min.x),
            (Vec3::new(-1.0, 0.0, 0.0), -b.max.x),
            (Vec3::new(0.0, 1.0, 0.0), b.min.y),
            (Vec3::new(0.0, -1.0, 0.0), -b.max.y),
        ]);
    }
    // A plan box round a place: only the model inside it.
    if let Some(a) = p.get("around").filter(|v| !v.is_null()) {
        let n: Vec<f64> = a.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
        let [x0, y0, x1, y1] = n[..] else { return Err(bad("model.render", "`around` must be [x0, y0, x1, y1]")) };
        if !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite()) {
            return Err(bad("model.render", "`around` must be [x0, y0, x1, y1]"));
        }
        keep_in.extend([
            (Vec3::new(1.0, 0.0, 0.0), x0.min(x1)),
            (Vec3::new(-1.0, 0.0, 0.0), -x0.max(x1)),
            (Vec3::new(0.0, 1.0, 0.0), y0.min(y1)),
            (Vec3::new(0.0, -1.0, 0.0), -y0.max(y1)),
        ]);
    }
    let proj = Projection::new(dir);
    // Compute the view from the frame (if framing) or the model's bounds, not from the full
    // triangle list — this lets us pick LODs and cull before flattening the scene.
    let view_bounds = frame
        .as_ref()
        .map(|f| Aabb {
            lo: Vec3::new(f.min.x, f.min.y, if m3.bounds.lo.z.is_finite() { m3.bounds.lo.z } else { 0.0 }),
            hi: Vec3::new(f.max.x, f.max.y, if m3.bounds.hi.z.is_finite() { m3.bounds.hi.z } else { 1.0 }),
        })
        .unwrap_or(m3.bounds);
    let initial_view = fit_bounds(&view_bounds, proj, w, h);
    // Cull and pick LODs; flatten only the visible parts into a triangle list.
    let mut sc = if let Some(iv) = &initial_view {
        let vis = m3.visible(iv);
        m3.flatten_visible(&vis)
    } else {
        m3.flatten()
    };
    // Clip by the half-spaces (room box, camera front, building frame, plan box).
    if !keep_in.is_empty() {
        let tris = std::mem::take(&mut sc.tris);
        for t in tris {
            let poly = clip_tri(&t.p, &keep_in);
            for k in 1..poly.len().saturating_sub(1) {
                if let (Some(a), Some(b), Some(c)) = (poly.first(), poly.get(k), poly.get(k + 1)) {
                    sc.tris.push(if bim {
                        cadaddy_render::shade::Tri { p: [*a, *b, *c], ..t }
                    } else {
                        cadaddy_render::shade::Tri::plain([*a, *b, *c], t.color)
                    });
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
    if sc.tris.is_empty() && sc.lines.is_empty() {
        return Err(bad("model.render", "nothing of the model is in view"));
    }
    // A red post where something is (an RFI's place): 8" square, 10' up from the level's floor.
    if let Some(m) = point_param(p, "mark").filter(|m| m.x.is_finite() && m.y.is_finite()) {
        let z0 = level.as_ref().and_then(|l| sp.level(l)).map_or(0.0, |l| l.elevation_ft * upf);
        let (r, top) = (4.0 / 12.0 * upf, z0 + 10.0 * upf);
        let red = cadaddy_color::Rgb(220, 30, 30);
        let c =
            [Vec3::new(m.x - r, m.y - r, 0.0), Vec3::new(m.x + r, m.y - r, 0.0), Vec3::new(m.x + r, m.y + r, 0.0), Vec3::new(m.x - r, m.y + r, 0.0)];
        let at = |v: Vec3, z: f64| Vec3::new(v.x, v.y, z);
        for k in 0..4 {
            let (Some(a), Some(b)) = (c.get(k), c.get((k + 1) % 4)) else { continue };
            sc.tris.push(cadaddy_render::shade::Tri::plain([at(*a, z0), at(*b, z0), at(*b, top)], red));
            sc.tris.push(cadaddy_render::shade::Tri::plain([at(*a, z0), at(*b, top), at(*a, top)], red));
        }
        let [a, b, cc, dd] = c;
        sc.tris.push(cadaddy_render::shade::Tri::plain([at(a, top), at(b, top), at(cc, top)], red));
        sc.tris.push(cadaddy_render::shade::Tri::plain([at(a, top), at(cc, top), at(dd, top)], red));
    }
    // Final fit on the clipped scene for pixel-precise framing.
    let view = fit(&sc, proj, w, h).ok_or_else(|| bad("model.render", "nothing to fit in the view"))?;
    let raster = if bim {
        cadaddy_render::bimview::rasterize_bim(&sc, &view, bg, &cadaddy_render::bimview::BimShade::default())
    } else {
        rasterize(&sc, &view, style, bg)
    };
    let png = raster.encode_png().ok_or_else(|| bad("model.render", "the image couldn't be encoded"))?;
    let mut out = json!({
        "width": view.width_px,
        "height": view.height_px,
        "triangles": sc.tris.len(),
        "instances": m3.instances.len(),
        "prototypes": m3.protos.len(),
        "style": if bim { "bim" } else { style.name() },
    });
    if outliers > 0 {
        out["outliers"] = json!(outliers);
    }
    deliver(png, p, &mut out)?;
    Ok(out)
}

/// The part of a triangle in all the half-spaces (a convex polygon; clipped plane by plane).
pub(crate) fn clip_tri(t: &[Vec3; 3], planes: &[(Vec3, f64)]) -> Vec<Vec3> {
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
pub(crate) fn keep_seg(mut a: Vec3, mut b: Vec3, planes: &[(Vec3, f64)]) -> Option<(Vec3, Vec3)> {
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
    super::exchange::guard_params(s, "model.overlay", p, &["path", "output"])?;
    let (w, h) = size("model.overlay", p, 1200, 900)?;
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
    let plan_handles: std::collections::HashSet<Handle> = ents.iter().map(|e| e.handle).collect();
    let mut all_match = true;
    for (li, l) in sp.levels.iter().enumerate() {
        let here = |p: Vec2| level_at(&sp.levels, p.x, p.y).is_some_and(|x| x.name.eq_ignore_ascii_case(&l.name));
        // The plan's doors: the swings and door blocks read here, and the swings the build read
        // (a built door's source, when it is a plan object) — one door per plan object.
        let mut door_objects: std::collections::HashSet<Handle> = ops.iter().filter(|o| o.kind == "door" && here(o.at)).map(|o| o.handle).collect();
        let read_here = door_objects.len();
        for b in made.iter().filter(|b| b.part == "door" && b.level.eq_ignore_ascii_case(&l.name) && b.source.0 != 0) {
            if plan_handles.contains(&b.source) {
                door_objects.insert(b.source);
            }
        }
        let plan_doors = door_objects.len();
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
            "plan": {"doors": plan_doors, "windows": plan_windows, "fixtures": plan_fixtures, "door_swings_read": read_here},
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
    fn a_kind_past_the_cap_leaves_room_for_the_others_and_every_one_is_counted() {
        let f = |kind: &'static str| Finding { kind, message: String::new(), level: None, at: None, handles: Vec::new() };
        let mut k = Kept::default();
        for _ in 0..MAX_FINDINGS + 5 {
            k.push(f("overlap"));
        }
        k.push(f("not_closed"));
        assert_eq!((k.total["overlap"], k.total["not_closed"]), (MAX_FINDINGS + 5, 1));
        assert_eq!(k.out.iter().filter(|x| x.kind == "not_closed").count(), 1, "kept past the other kind's cap");
        assert_eq!(k.out.len(), MAX_FINDINGS + 1);
    }

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
