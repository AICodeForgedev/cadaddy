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

use std::collections::{BTreeMap, HashMap, HashSet};
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
            .params("{kind?: leak|opening_not_in_wall|window_not_in_wall|window_inside_building (a built window in no wall, or in a wall inside the building, not an outside wall)|wall_without_height|walls_overlap|fixture_above_ceiling|fixture_below_floor|level_unmatched|not_built|outside_building|roof_off_walls (a roof standing over its walls on nothing, or off their outline), limit?: 1–1000 (100), cursor?: from `next`, bcf?: bool (a BCF topic per matching issue, up to 500), bcf_path?: .bcf file (those topics exported)} → {issues: [{kind, message, level?, at?, handles}] (a page: `limit` from `cursor`), total, byKind, next? (the cursor of the next page), bcf?: {created, topics, skipped, path?}}")
            .files(),
        CommandSpec::new("model.render", "Render Model View", run_render)
            .params("{force?: bool (replace an existing file; otherwise it is refused), path?: PNG file (else the PNG comes back as base64), output?: same as path, width?, height? (800×600), style?: bim (default: trade colours, shaded with edges, the shell ghosted from outside)|realistic|shaded|shadedwithedges|hidden|wireframe, ghost?: bool, level?: name (an axonometric of its storey: what stands between its floor and the floor above, from every level, runs cut at the floor above), room?: name or number, eye?: [x,y,z], target?: [x,y,z] (a camera pose), dir?: [x,y,z] (view direction towards the viewer), around?: [x0,y0,x1,y1] (only the model inside this plan box: a close-up), mark?: [x,y] (a red post at this plan point on `level`'s floor), fidelity?: auto|building|floor|room|detail (draw the built model as the 3D view's fidelity tier does: auto by the view's height), view?: systems|architectural|mep|floor|room (draw this BIM view without changing the drawing's), section?: {min: [x,y,z], max: [x,y,z]} | bool (only the model inside this box, drawing units; default the coordination section box while it is on, false: none)} → {path?|png, width, height, triangles, fidelity?, storey?: {level, floor_ft, top_ft} (a level's or room's: the floor above it is cut at, null at the top), view?, drawn?, switches?, legend?: [{key, label, group, color, count}]}: the whole building (or a `view`) exactly as the 3D BIM view draws it (bim.view and bim.look)")
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
    /// The name it was built under (a roof's, for its deck, parapets, roof walls; a member's
    /// starts with its roof's).
    name: String,
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
            let name = x.get(at + 4).map(|t| t.value.trim().to_string()).unwrap_or_default();
            Some(Built { handle: e.handle, part, level: made_level(e).unwrap_or_default(), name, source, mesh: m.clone() })
        })
        .collect()
}

/// The parts a building stands as: what a whole-building view frames, and what each level's
/// other built objects should stand in.
const SHELL: &[&str] = &["wall", "floor", "curtainwall", "column", "roof"];

/// The parts a roof is built of: its deck, parapets, the walls carried up round its framing, and
/// the framing.
const ROOF_PARTS: &[&str] = &["roof", "parapet", super::exterior::ROOF_WALL, super::exterior::ROOF_MEMBER];
/// Most a roof's underside may stand over the top of the walls under it, feet (a sill or a
/// bearing pad; more is a roof standing on nothing).
const ROOF_GAP_FT: f64 = 0.5;
/// Most a roof's outline may run past the walls under it, or fall short of them, on a side,
/// feet (an eave's overhang).
const ROOF_OFF_FT: f64 = 3.0;

/// The plan box and height range of meshes.
fn mesh_box<'a>(ms: impl Iterator<Item = &'a Mesh>) -> (Bounds2, f64, f64) {
    let (mut b, mut lo, mut hi) = (Bounds2::EMPTY, f64::INFINITY, f64::NEG_INFINITY);
    for m in ms {
        for v in m.vertices.iter().filter(|v| v.x.is_finite() && v.y.is_finite() && v.z.is_finite()) {
            b = b.union(&Bounds2::new(v.xy(), v.xy()));
            lo = lo.min(v.z);
            hi = hi.max(v.z);
        }
    }
    (b, lo, hi)
}

/// How far a wall's face may stand off a roof's outline and still be under it, feet (a wall
/// built flush with the roof's edge).
const ROOF_EDGE_FT: f64 = 0.5;
/// A roof deck's plan faces looked at, at most.
const MAX_ROOF_FACES: usize = 20_000;
/// The spacing of the points looked at along a wall's faces, feet, and their most on one face.
const WALL_POINT_FT: f64 = 1.0;
const WALL_FACE_POINTS: usize = 64;
/// The sides a roof can be off its walls on, in the order the message lists them; then a notch
/// or hole in it.
const SIDES: [&str; 4] = ["west", "east", "south", "north"];
const OPENING: usize = SIDES.len();

/// One roof of a roof level: its decks and the parts built round them (its parapets, roof walls
/// and framing), the plan its decks cover, its plan box, underside and the decks' high point.
struct RoofPiece<'a> {
    name: String,
    parts: Vec<&'a Built>,
    cover: Vec<(Bounds2, Vec<Vec2>)>,
    bounds: Bounds2,
    lo: f64,
    top: f64,
}

impl RoofPiece<'_> {
    /// Whether its decks cover a plan point (within `tol`).
    fn covers(&self, p: Vec2, tol: f64) -> bool {
        self.cover
            .iter()
            .any(|(b, q)| b.expand(tol).contains(p) && (cadaddy_geom::point_in_polygon(q, p) || edges(q).any(|(a, c)| seg_dist(p, a, c) <= tol)))
    }

    /// The point of its decks' outline nearest a plan point.
    fn nearest(&self, p: Vec2) -> Option<Vec2> {
        self.cover.iter().flat_map(|(_, q)| edges(q)).map(|(a, b)| closest_on_seg(p, a, b)).min_by(|x, y| x.dist(p).total_cmp(&y.dist(p)))
    }
}

/// A mesh's faces as they lie in plan (faces standing on edge dropped).
fn plan_faces(m: &Mesh) -> Vec<(Bounds2, Vec<Vec2>)> {
    m.faces
        .iter()
        .take(MAX_ROOF_FACES)
        .filter_map(|f| {
            let q: Vec<Vec2> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize)).map(|v| v.xy()).collect();
            let b = Bounds2::from_points(q.iter().copied());
            let area = cadaddy_geom::shoelace(&q).abs();
            (q.len() >= 3 && q.iter().all(|v| v.is_finite()) && area > 1e-12 && area > 1e-6 * b.width() * b.height()).then_some((b, q))
        })
        .collect()
}

/// A roof level's roof parts as roofs: one for each roof's decks (by the name they were built
/// under); a parapet, roof wall or member joins the roof it was built for (its name, or its
/// name's start for a member), else the roof whose decks are under its middle, else the nearest.
/// No decks: one roof of all the parts.
fn roof_pieces<'a>(parts: &[&'a Built]) -> Vec<RoofPiece<'a>> {
    let piece = |b: &'a Built| RoofPiece {
        name: b.name.clone(),
        parts: vec![b],
        cover: plan_faces(&b.mesh),
        bounds: mesh_box(std::iter::once(&b.mesh)).0,
        lo: f64::INFINITY,
        top: f64::NEG_INFINITY,
    };
    let is_deck = |b: &Built| b.part.eq_ignore_ascii_case("roof");
    let mut pieces: Vec<RoofPiece<'a>> = Vec::new();
    for &b in parts.iter().filter(|b| is_deck(b)) {
        match pieces.iter_mut().find(|p| p.name.eq_ignore_ascii_case(&b.name)) {
            Some(p) => {
                p.parts.push(b);
                p.cover.extend(plan_faces(&b.mesh));
                p.bounds = p.bounds.union(&mesh_box(std::iter::once(&b.mesh)).0);
            }
            None => pieces.push(piece(b)),
        }
    }
    for &b in parts.iter().filter(|b| !is_deck(b)) {
        let name = b.name.to_ascii_lowercase();
        let own = |p: &RoofPiece| {
            let roof = p.name.to_ascii_lowercase();
            !roof.is_empty() && (name == roof || name.strip_prefix(&roof).is_some_and(|rest| rest.starts_with(' ')))
        };
        let mid = mesh_box(std::iter::once(&b.mesh)).0.center();
        let at = pieces.iter().position(own).or_else(|| pieces.iter().position(|p| p.covers(mid, 0.0))).or_else(|| {
            pieces.iter().enumerate().min_by(|(_, x), (_, y)| shell_dist(&x.bounds, mid).total_cmp(&shell_dist(&y.bounds, mid))).map(|(i, _)| i)
        });
        match at.and_then(|i| pieces.get_mut(i)) {
            Some(p) => p.parts.push(b),
            None => pieces.push(piece(b)),
        }
    }
    for p in &mut pieces {
        let (b, lo, hi) = mesh_box(p.parts.iter().map(|x| &x.mesh));
        let (_, _, deck_hi) = mesh_box(p.parts.iter().filter(|x| is_deck(x)).map(|x| &x.mesh));
        p.bounds = b;
        p.lo = lo;
        p.top = if deck_hi.is_finite() { deck_hi } else { hi };
    }
    pieces
}

/// Points along the faces of a built wall's boxes (every [`WALL_POINT_FT`]), else its vertices.
fn face_points(m: &Mesh, upf: f64) -> Vec<Vec2> {
    let step = WALL_POINT_FT * upf;
    let mut out = Vec::new();
    for [p0, p1, p2, p3] in m.vertices.chunks_exact(8).take(MAX_BOXES).filter_map(box_plan) {
        for (a, b) in [(p0, p1), (p3, p2)] {
            let n = (a.dist(b) / step).ceil();
            let n = if n.is_finite() { (n as usize).clamp(1, WALL_FACE_POINTS) } else { 1 };
            out.extend((0..=n).map(|i| a + (b - a) * (i as f64 / n as f64)));
        }
    }
    if out.is_empty() {
        out = m.vertices.iter().map(|v| v.xy()).filter(|v| v.is_finite()).take(MAX_BOXES).collect();
    }
    out
}

/// A storey's built wall as a roof's check sees it: its storey's elevation, its top, plan box and
/// the points along its faces.
struct WallFaces<'a> {
    built: &'a Built,
    elevation: f64,
    top: f64,
    bounds: Bounds2,
    points: Vec<Vec2>,
}

/// Roofs that don't meet their walls, each roof against the walls under its own decks (a stepped
/// building's low roof on the wing's walls, its high roof on the upper storey's): a roof whose
/// underside stands more than [`ROOF_GAP_FT`] over the top of the walls it bears on (the highest
/// storey's walls under it; nothing between them), whose outline runs more than [`ROOF_OFF_FT`]
/// past them on a side, or that stops that far short of a wall of a storey the roofs bear on
/// that no roof covers (nor a storey over it). A roof over none of the walls: off the highest
/// walled storey's walls as a whole.
fn roofs_off_walls(levels: &[cadaddy_walk3d::Level], made: &[Built], upf: f64) -> Vec<Finding> {
    let mut out = Vec::new();
    let is = |b: &Built, level: &str, parts: &[&str]| b.level.eq_ignore_ascii_case(level) && parts.iter().any(|p| b.part.eq_ignore_ascii_case(p));
    let edge = ROOF_EDGE_FT * upf;
    let off_ft = ROOF_OFF_FT * upf;
    for roof in levels.iter().filter(|l| super::wallgen::roof_level(levels, &l.name)).take(100) {
        let parts: Vec<&Built> = made.iter().filter(|b| is(b, &roof.name, ROOF_PARTS)).collect();
        if parts.is_empty() {
            continue;
        }
        // The storeys under it and their walls.
        let below = |name: &str| {
            levels
                .iter()
                .find(|l| l.name.eq_ignore_ascii_case(name))
                .filter(|l| l.elevation_ft < roof.elevation_ft && !super::wallgen::roof_level(levels, &l.name))
                .map(|l| l.elevation_ft)
        };
        let walls: Vec<WallFaces> = made
            .iter()
            .filter(|b| b.part.eq_ignore_ascii_case("wall"))
            .take(MAX_WALLS)
            .filter_map(|b| {
                let elevation = below(&b.level)?;
                let (bounds, _, top) = mesh_box(std::iter::once(&b.mesh));
                (!bounds.is_empty() && top.is_finite()).then(|| WallFaces { built: b, elevation, top, bounds, points: face_points(&b.mesh, upf) })
            })
            .collect();
        if walls.is_empty() {
            continue;
        }
        let floors: Vec<(f64, Footprint)> =
            made.iter().filter(|b| b.part.eq_ignore_ascii_case("floor")).filter_map(|b| Some((below(&b.level)?, footprint(&b.mesh)))).collect();
        let pieces = roof_pieces(&parts);
        // Each roof's sides off its walls: (side, past) → how far, whose walls and, in an opening,
        // where.
        let mut offs: Vec<BTreeMap<(usize, bool), (f64, String, Option<Vec2>)>> = vec![BTreeMap::new(); pieces.len()];
        let mut bears: Vec<String> = vec![String::new(); pieces.len()];
        let mut borne: Vec<f64> = Vec::new();
        let label = |p: &RoofPiece| if pieces.len() > 1 && !p.name.is_empty() { format!(" ({})", p.name) } else { String::new() };
        let handles = |p: &RoofPiece| p.parts.iter().map(|b| b.handle).take(MAX_HANDLES).collect::<Vec<Handle>>();
        for (i, p) in pieces.iter().enumerate() {
            if p.bounds.is_empty() || !p.lo.is_finite() {
                continue;
            }
            // The walls under its decks: up to their high point (an upper storey's walls rising
            // past a low roof stand beside it, not under it).
            let reach = p.bounds.expand(edge);
            let under: Vec<(&WallFaces, Vec<Vec2>)> = walls
                .iter()
                .filter(|w| w.top <= p.top + 1e-6 * upf.max(1.0) && w.bounds.intersects(&reach))
                .filter_map(|w| {
                    let on: Vec<Vec2> = w.points.iter().copied().filter(|q| p.covers(*q, edge)).collect();
                    (!on.is_empty()).then_some((w, on))
                })
                .collect();
            // It bears on the highest storey of them; under none, on the highest walled storey's
            // walls as a whole.
            let (on, held) = match under.iter().map(|(w, _)| w.elevation).max_by(f64::total_cmp) {
                Some(e) => {
                    borne.push(e);
                    let on: Vec<&(&WallFaces, Vec<Vec2>)> = under.iter().filter(|(w, _)| w.elevation == e).collect();
                    (on.iter().map(|(w, _)| *w).collect::<Vec<_>>(), Bounds2::from_points(on.iter().flat_map(|(_, q)| q.iter().copied())))
                }
                None => {
                    let Some(e) = walls.iter().map(|w| w.elevation).max_by(f64::total_cmp) else { continue };
                    let on: Vec<&WallFaces> = walls.iter().filter(|w| w.elevation == e).collect();
                    let held = on.iter().fold(Bounds2::EMPTY, |b, w| b.union(&w.bounds));
                    (on, held)
                }
            };
            let Some(first) = on.first() else { continue };
            let storey = first.built.level.clone();
            let w_hi = on.iter().map(|w| w.top).fold(f64::NEG_INFINITY, f64::max);
            let gap = p.lo - w_hi;
            if gap > ROOF_GAP_FT * upf {
                out.push(Finding {
                    kind: "roof_off_walls",
                    message: format!(
                        "{}'s roof{} stands {} over the top of {}'s walls with nothing between them (no framing, roof walls or parapet down to the walls)",
                        roof.name,
                        label(p),
                        feet(gap, upf),
                        storey
                    ),
                    level: Some(roof.name.clone()),
                    at: Some(p.bounds.center()),
                    handles: handles(p),
                });
            }
            // Each side: how far the roof runs past the walls it holds (+) or falls short of them (-).
            let rb = p.bounds;
            let sides = [held.min.x - rb.min.x, rb.max.x - held.max.x, held.min.y - rb.min.y, rb.max.y - held.max.y];
            if let Some(o) = offs.get_mut(i) {
                for (k, v) in sides.iter().enumerate().filter(|(_, v)| v.abs() > off_ft) {
                    o.insert((k, *v > 0.0), (v.abs(), storey.clone(), None));
                }
            }
            if let Some(b) = bears.get_mut(i) {
                *b = storey;
            }
        }
        // The walls of the storeys the roofs bear on that no roof covers, nor a storey over them:
        // the nearest roof stops short of them, or (inside its box) leaves them under a notch or
        // hole in it.
        for w in walls.iter().filter(|w| borne.contains(&w.elevation)) {
            for q in &w.points {
                if pieces.iter().any(|p| p.covers(*q, edge)) || floors.iter().any(|(e, f)| *e > w.elevation && f.covers(*q, edge)) {
                    continue;
                }
                let Some((i, n)) = pieces
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| p.nearest(*q).map(|n| (i, n)))
                    .min_by(|(_, a), (_, b)| a.dist(*q).total_cmp(&b.dist(*q)))
                else {
                    continue;
                };
                let d = *q - n;
                let inside = pieces.get(i).is_some_and(|p| p.bounds.expand(edge).contains(*q));
                let (k, v) = if inside {
                    (OPENING, d.len())
                } else if d.x.abs() >= d.y.abs() {
                    (usize::from(d.x > 0.0), d.x.abs())
                } else {
                    (2 + usize::from(d.y > 0.0), d.y.abs())
                };
                if v > off_ft
                    && let Some(o) = offs.get_mut(i)
                {
                    let e = o.entry((k, false)).or_insert((0.0, w.built.level.clone(), None));
                    if v > e.0 {
                        *e = (v, w.built.level.clone(), Some(*q));
                    }
                }
            }
        }
        for ((p, o), storey) in pieces.iter().zip(&offs).zip(&bears) {
            if o.is_empty() {
                continue;
            }
            let off: Vec<String> = o
                .iter()
                .map(|((k, past), (v, whose, at))| {
                    let them = if whose.eq_ignore_ascii_case(storey) { "them".to_string() } else { format!("{whose}'s walls") };
                    match (SIDES.get(*k), at) {
                        (Some(side), _) => format!("{} {them} on the {side} by {}", if *past { "past" } else { "short of" }, feet(*v, upf)),
                        (None, Some(q)) => format!("open over {them} at {} ({} from its edge)", fmt_pt(*q), feet(*v, upf)),
                        (None, None) => format!("open over {them}"),
                    }
                })
                .collect();
            out.push(Finding {
                kind: "roof_off_walls",
                message: format!("{}'s roof{} is off {}'s walls: {}", roof.name, label(p), storey, off.join(", ")),
                level: Some(roof.name.clone()),
                at: Some(p.bounds.center()),
                handles: handles(p),
            });
        }
    }
    out
}

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

/// A built mesh's boxes in plan (each eight corners: four at its bottom, the same four at its
/// top, as wallgen's `add_box` makes them): centreline ends and thickness. Other meshes give none.
fn plan_boxes(m: &Mesh) -> Vec<(Vec2, Vec2, f64)> {
    m.vertices
        .chunks_exact(8)
        .take(MAX_BOXES)
        .filter_map(|c| {
            let [p0, p1, p2, p3] = box_plan(c)?;
            let a = (p0 + p3) * 0.5;
            let b = (p1 + p2) * 0.5;
            let t = p0.dist(p3);
            (a.is_finite() && b.is_finite() && a.dist(b) > 1e-9 && t.is_finite() && t > 0.0).then_some((a, b, t))
        })
        .collect()
}

/// The plan corners of one of wallgen's boxes (eight corners: four at its bottom, the same four
/// at its top), else none.
fn box_plan(c: &[Vec3]) -> Option<[Vec2; 4]> {
    let [p0, p1, p2, p3, q0, q1, q2, q3] = c else { return None };
    let same = |p: &Vec3, q: &Vec3| p.xy().dist(q.xy()) <= 1e-6 * (1.0 + p.xy().len());
    (same(p0, q0) && same(p1, q1) && same(p2, q2) && same(p3, q3)).then(|| [p0.xy(), p1.xy(), p2.xy(), p3.xy()])
}

/// Boxes read off one built mesh, at most.
const MAX_BOXES: usize = 4_096;
/// Built windows checked, at most.
const MAX_WINDOWS: usize = 20_000;
/// How far (inches) a window's pane may stand off its host wall's faces, and past its own face a
/// side is looked at for the level's floor.
const WINDOW_TOL_IN: f64 = 0.5;
const PAST_FACE_IN: f64 = 3.0;

/// The built windows not in an outside wall (Q6308): a window's pane lies in the thickness of a
/// wall parallel to it, the wall's extent across it, a full-height cut's too (Q6461)
/// ("window_not_in_wall" when none holds it), and that wall has the outside
/// beyond one face: no wall past it that way, or no floor just past that face (a courtyard)
/// ("window_inside_building" when walls stand past both faces over the level's floor: a corridor,
/// a line pair against the outside wall's inside face). `upf`: drawing units per foot.
fn windows_off_walls(made: &[Built], upf: f64) -> Vec<Finding> {
    let k = upf / 12.0;
    let walls: Vec<(&str, Vec<(Vec2, Vec2, f64)>)> =
        made.iter().filter(|b| b.part == "wall").take(MAX_WALLS).map(|b| (b.level.as_str(), plan_boxes(&b.mesh))).collect();
    let floors: Vec<(&str, Footprint)> = made.iter().filter(|b| b.part == "floor").map(|b| (b.level.as_str(), footprint(&b.mesh))).collect();
    let mut out = Vec::new();
    for b in made.iter().filter(|b| b.part == "window").take(MAX_WINDOWS) {
        let Some(&(a, e, _)) = plan_boxes(&b.mesh).first() else { continue };
        let len = a.dist(e);
        let u = (e - a) * (1.0 / len);
        let (mid, nrm) = ((a + e) * 0.5, u.perp());
        let level_walls = || walls.iter().filter(|(l, _)| l.eq_ignore_ascii_case(&b.level)).flat_map(|(_, bx)| bx.iter());
        // Its host is the wall it is cut in, read off the wall's extent, not the solid left by
        // the cut: a box along it whose thickness holds the pane's ends and whose length holds
        // its middle (the sill or head box across the cut); or, where the cut leaves no box
        // across it (a storefront, glazing floor to top: no sill, the head at the top), wall
        // standing at both its jambs (the wall each side of the cut, two walls in line either
        // side of the gap, the wall across a corner), one of them a box along it.
        let tol = WINDOW_TOL_IN * k;
        let holds = |&(wa, wb, wt): &(Vec2, Vec2, f64), p: Vec2| {
            let wl = wa.dist(wb);
            let wu = (wb - wa) * (1.0 / wl);
            let s = wu.dot(p - wa);
            wl > 1e-12 && wu.perp().dot(p - wa).abs() <= wt / 2.0 + tol && s >= -tol && s <= wl + tol
        };
        let along = |&(wa, wb, wt): &(Vec2, Vec2, f64)| {
            let wu = (wb - wa) * (1.0 / wa.dist(wb));
            let within = |p: Vec2| wu.perp().dot(p - wa).abs() <= wt / 2.0 + tol;
            u.cross(wu).abs() <= 0.09 && within(a) && within(e)
        };
        // (The jambs only where no box spans it: a thicker wall abutting a jamb of a window whose
        // own wall holds it isn't its host.)
        let thickest = |m: Option<f64>, &(_, _, t): &(Vec2, Vec2, f64)| Some(m.map_or(t, |x: f64| x.max(t)));
        let host = level_walls().filter(|w| along(w) && holds(w, mid)).fold(None, thickest).or_else(|| {
            let jambed = level_walls().any(|w| holds(w, a)) && level_walls().any(|w| holds(w, e));
            level_walls().filter(|w| jambed && along(w) && (holds(w, a) || holds(w, e))).fold(None, thickest)
        });
        let at = Some(mid).filter(|p| p.is_finite());
        let Some(host_t) = host else {
            out.push(Finding {
                kind: "window_not_in_wall",
                message: format!("a window at {} is in no wall: no wall holds it within its frame depth", fmt_pt(mid)),
                level: Some(b.level.clone()),
                at,
                handles: vec![b.handle, b.source],
            });
            continue;
        };
        // Each side: a wall past its face that way (a centreline the ray meets), and floor just
        // past its face.
        let side = |dir: Vec2| {
            // (A wall overlapping the host's thickness is the host read twice, not one past it.)
            let walled = level_walls().any(|(q, r_end, wt)| {
                let r = *r_end - *q;
                let den = dir.cross(r);
                if den.abs() < 1e-12 {
                    return false;
                }
                let t = (*q - mid).cross(r) / den;
                let v = (*q - mid).cross(dir) / den;
                t - wt / 2.0 > host_t / 2.0 - WINDOW_TOL_IN * k && (0.0..=1.0).contains(&v)
            });
            let probe = mid + dir * (host_t / 2.0 + PAST_FACE_IN * k);
            let level_floors: Vec<&Footprint> = floors.iter().filter(|(l, _)| l.eq_ignore_ascii_case(&b.level)).map(|(_, f)| f).collect();
            let floored = level_floors.is_empty() || level_floors.iter().any(|f| f.covers(probe, 0.0));
            walled && floored
        };
        if side(nrm) && side(-nrm) {
            out.push(Finding {
                kind: "window_inside_building",
                message: format!(
                    "a window at {} is in a wall inside the building (walls and floor past both its faces), not an outside wall",
                    fmt_pt(mid)
                ),
                level: Some(b.level.clone()),
                at,
                handles: vec![b.handle, b.source],
            });
        }
    }
    out
}

fn edges(q: &[Vec2]) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    q.iter().copied().zip(q.iter().copied().cycle().skip(1))
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    p.dist(closest_on_seg(p, a, b))
}

/// The point of segment `a`–`b` nearest `p`.
fn closest_on_seg(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let d = b - a;
    let l2 = d.dot(d);
    let t = if l2 > 1e-18 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    a + d * t
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
/// without the generated walls closing the gaps. All of it: what the view hid (object isolation,
/// a clash shown alone) is still the plan.
pub(crate) fn plan_session(s: &Session) -> Result<Session> {
    let mut d = (*s.doc()?).clone();
    d.hidden_objects.clear();
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
    let tagged = super::bspec::tags_carry_labels(sp, &ents);
    // The plan's outlines are made once for every tag (not once per tag: 60k objects).
    let mut cache = None;
    for (h, t, at) in room_texts(sp, &ents) {
        // A unit tag in a room (`UNIT A`, `UNIT A (MIRROR)`) names the room's unit type and hand,
        // as the unit plans read it (`units.detect`): the room's, not another room.
        if cadaddy_mepfix::unit_reader::unit_tag(&t).is_some() {
            continue;
        }
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
    let site: std::collections::BTreeSet<String> = sp
        .mep
        .equipment
        .iter()
        .filter(|e| super::utilityxfmr::is_site(e) || e.sources.contains_key(super::sitework::MARK))
        .flat_map(|e| [e.tag.trim().to_ascii_uppercase(), format!("PAD-{}", e.tag.trim().to_ascii_uppercase())])
        .collect();
    let mut off: Vec<((String, String, String), Vec<Handle>, Vec2, f64)> = Vec::new();
    for e in d.model.iter() {
        let Some(part) = made_as(e) else { continue };
        if part.eq_ignore_ascii_case("ground") || SHELL.iter().any(|p| part.eq_ignore_ascii_case(p)) {
            continue;
        }
        // Site work stands on the site, outside the walls by design (a pole light, an EV charger,
        // the utility's transformer and their pads).
        if cadaddy_doc::props::read(&e.common.xdata).get(super::sitework::MARK).is_some()
            || super::bimlook::made_tag(e, "tag").is_some_and(|t| site.contains(&t.trim().to_ascii_uppercase()))
        {
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
    for f in roofs_off_walls(&sp.levels, &made, upf) {
        push(f);
    }
    step("roofs on their walls");
    // Rooms that leak.
    let rs = rooms(&plan, &sp)?;
    let mut reported: Vec<(Handle, Handle)> = Vec::new();
    // A roof plan has no rooms (the build makes none from it): its roof areas' and details'
    // labels are no leaks.
    let on_roof = |name: &Option<String>| name.as_deref().is_some_and(|n| super::wallgen::roof_level(&sp.levels, n));
    for r in &rs {
        let level = level_at(&sp.levels, r.at.x, r.at.y).map(|l| l.name.clone());
        if on_roof(&level) {
            continue;
        }
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
    // Doors and windows of a storey's plan that no wall took. Only those the build reads: on a
    // storey's plan, within its walls' extents (an opening on another sheet or view of the same
    // building is that view's copy, not another door); and not those the build judged no door
    // (a fixture's or furniture's arc) or the same opening as a built one.
    if !made.is_empty() {
        let fates = super::wallgen::opening_fates(pd, &sp);
        let taken: HashSet<Handle> = made.iter().filter(|b| matches!(b.part.as_str(), "door" | "window")).map(|b| b.source).collect();
        for o in plan_openings(pd, &sp, &ents) {
            if taken.contains(&o.handle) {
                continue;
            }
            let Some(level) = level_at(&sp.levels, o.at.x, o.at.y) else { continue };
            // A roof plan's lines are its edge, parapet and curbs: the build reads no doors off it.
            if super::wallgen::roof_level(&sp.levels, &level.name) {
                continue;
            }
            let storey = fates.iter().find(|f| f.level.eq_ignore_ascii_case(&level.name));
            let off_plan = storey.and_then(|f| f.footprint).is_some_and(|(lo, hi)| shell_dist(&Bounds2::new(lo, hi), o.at) > OUTSIDE_FT * upf);
            if level.sheet.is_some() || off_plan {
                continue;
            }
            let why = storey.and_then(|f| f.skipped.iter().find(|x| x.source == o.handle)).map(|x| x.why);
            if matches!(why, Some(super::wallgen::Skip::NotADoor | super::wallgen::Skip::Same)) {
                continue;
            }
            push(Finding {
                kind: "opening_not_in_wall",
                message: format!(
                    "a {} at {} is drawn where no wall was built (the wall it is drawn in is missing), so it wasn't built",
                    o.kind,
                    fmt_pt(o.at)
                ),
                level: Some(level.name.clone()),
                at: Some(o.at),
                handles: vec![o.handle],
            });
        }
    }
    step("rooms that leak");
    // Windows not in an outside wall: a window is hosted by the outside wall that holds it.
    for w in windows_off_walls(&made, upf) {
        push(w);
    }
    step("windows");
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
    let wanted_room = number_or_name_param(p, "room");
    if let Some(name) = wanted_room.as_deref()
        && let Some(r) = super::bspec::room_elements(d).into_iter().find(|r| {
            r.outline.len() >= 3
                && (r.name.eq_ignore_ascii_case(name.trim()) || r.number.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name.trim())))
        })
    {
        // A built room has its outline: no plan trace of every room.
        room = Some((r.outline.clone(), r.level.clone()));
    } else if let Some(name) = wanted_room.as_deref() {
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
    // A level's (or a room's level's) view is its storey (Q6660): its runs cut at the floor
    // above, and the pieces of other levels' runs standing in it drawn (cut to it), whether the
    // build cut its risers at each floor or kept them whole.
    let storey = only.as_ref().and_then(|l| super::storey::Storey::of(d, l));
    let carried_ok = |e: &Entity| {
        storey.is_some()
            && only.as_ref().is_some_and(|l| made_level(e).is_some_and(|m| !m.eq_ignore_ascii_case(l)))
            && made_as(e).is_some_and(|part| part != "ceiling" && part != "ground")
    };
    // The whole building: its storeys stacked, framed on what it stands as (walls, floors,
    // columns, roofs); a reader's stray object off the building (model.issues lists it as
    // outside_building) never shrinks the building to a speck in the corner of the view.
    let whole = only.is_none() && eye.is_none() && p.get("around").is_none_or(Value::is_null) && p.get("mark").is_none_or(Value::is_null);
    // The BIM view drawn (Q6617): the whole building as the 3D view draws it (its active view,
    // switches and look), or the view asked for, without changing the drawing's.
    let asked = match p.get("view").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => Some(
            v.as_str()
                .and_then(super::bimviews::View::parse)
                .ok_or_else(|| bad("model.render", format!("`view` must be systems, architectural, mep, floor or room, got {v}")))?,
        ),
    };
    let mut drew: Option<(super::bimlook::Look, super::bimviews::View, super::bimlook::Drew)> = None;
    let md = if bim && (whole || asked.is_some()) && !super::bimlook::Look::of(d).off {
        let mut look = super::bimlook::Look::of(d);
        if let Some(v) = asked {
            super::bimviews::set_view(&mut look, v);
            if v.per_level() {
                look.level = only.clone().or(look.level);
            }
        }
        if let Some(g) = p.get("ghost").and_then(Value::as_bool) {
            look.ghost = Some(g);
        }
        let keep: std::collections::HashSet<Handle> = keep.iter().copied().collect();
        let all = whole;
        let (mut md, what) =
            super::bimlook::display_summary_in(d, &look, whole, &|e, _| all || keep.contains(&e.handle) || carried_ok(e), storey.as_ref())
                .ok_or_else(|| bad("model.render", "nothing has been built yet (run model.build)"))?;
        // The model as built: a view's object isolation (a clash shown alone) hides none of it.
        md.hidden_objects.clear();
        let view = super::bimviews::active_in(d, &look);
        drew = Some((look, view, what));
        md
    } else if bim {
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
        let mut md = super::bimlook::compose_in(d, &look, ghost, whole, storey.as_ref(), &|e, _| keep.contains(&e.handle) || carried_ok(e));
        // The model as built: a view's object isolation (a clash shown alone) hides none of it.
        md.hidden_objects.clear();
        md
    } else {
        let mut md = Drawing::new_imperial();
        md.layers = d.layers.clone();
        let offsets = if whole { super::bimlook::stack_offsets(d) } else { Vec::new() };
        // The storey's other levels' objects after its own.
        let from_others: Vec<Handle> = match &storey {
            Some(st) => {
                made.iter().filter(|b| d.model.get(b.handle).is_some_and(|e| carried_ok(e) && st.stands_in(e, &b.level))).map(|b| b.handle).collect()
            }
            None => Vec::new(),
        };
        for h in keep.iter().chain(from_others.iter()) {
            if let Some(e) = d.model.get(*h) {
                let level = made_level(e).unwrap_or_default();
                let mut c = match &storey {
                    Some(st) => {
                        let from = (!only.as_ref().is_some_and(|l| l.eq_ignore_ascii_case(&level))).then_some(level.as_str());
                        match st.place(e, from, true) {
                            None => continue,
                            Some(c) => c.into_owned(),
                        }
                    }
                    None => (**e).clone(),
                };
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
    // Fidelity (as the 3D view draws it): the built model by tier, at the one asked for (auto:
    // the one the view's height calls for).
    let fidelity = match p.get("fidelity").filter(|v| !v.is_null()) {
        None => None,
        Some(v) => match v.as_str().map(str::trim) {
            Some(t) if t.eq_ignore_ascii_case("auto") => Some(None),
            Some(t) => {
                Some(Some(model3::Tier::parse(t).ok_or_else(|| bad("model.render", "`fidelity` must be auto, building, floor, room or detail"))?))
            }
            None => return Err(bad("model.render", "`fidelity` must be auto, building, floor, room or detail")),
        },
    };
    let m3 = match fidelity {
        Some(_) => {
            let md = std::sync::Arc::new(md.clone());
            let classify = super::fidelity::classifier(&md);
            model3::build_fid(std::sync::Arc::clone(&md), &Space::Model, tol, bg, &progress, &total, &[], &classify, model3::FidOptions::default())
        }
        None => model3::build(&md, &Space::Model, tol, bg, &progress, &total),
    };
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
    if let Some((lo, hi)) = section_box(d, p)? {
        keep_in.extend([
            (Vec3::new(1.0, 0.0, 0.0), lo.x),
            (Vec3::new(-1.0, 0.0, 0.0), -hi.x),
            (Vec3::new(0.0, 1.0, 0.0), lo.y),
            (Vec3::new(0.0, -1.0, 0.0), -hi.y),
            (Vec3::new(0.0, 0.0, 1.0), lo.z),
            (Vec3::new(0.0, 0.0, -1.0), -hi.z),
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
    let mut tier_drawn = None;
    let mut sc = match (&initial_view, fidelity) {
        (Some(iv), Some(fixed)) => {
            let auto = super::fidelity::Rules::of(&md).0.auto.tiers();
            let tier = fixed.unwrap_or_else(|| auto.tier(iv.height / super::fidelity::units_per_foot(&md), None));
            tier_drawn = Some(tier);
            let vis = m3.visible_at(iv, tier, None);
            let mut sc = m3.flatten_visible(&vis);
            let mut st = model3::Stream::default();
            st.update(&m3, iv, tier, f64::INFINITY);
            for k in st.visible(iv) {
                if let Some(c) = st.cells.get(&k) {
                    sc.tris.extend_from_slice(&c.scene.tris);
                    sc.lines.extend_from_slice(&c.scene.lines);
                }
            }
            sc
        }
        (Some(iv), None) => {
            let vis = m3.visible(iv);
            m3.flatten_visible(&vis)
        }
        (None, _) => m3.flatten(),
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
    let systems = drew.as_ref().is_some_and(|(_, v, _)| *v == super::bimviews::View::Systems);
    let raster = if systems {
        let min_px = super::bimviews::Rules::of(d).0.systems_min_px;
        cadaddy_render::bimview::rasterize_bim(&sc, &view, bg, &cadaddy_render::bimview::BimShade::systems(min_px))
    } else if bim {
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
    if let Some(t) = tier_drawn {
        out["fidelity"] = json!(t.name());
    }
    if outliers > 0 {
        out["outliers"] = json!(outliers);
    }
    // The storey drawn (Q6660): its floor and the floor above it is cut at (none: the top).
    if let Some(st) = &storey {
        out["storey"] = json!({"level": st.level, "floor_ft": st.lo / upf, "top_ft": st.hi.map(|h| h / upf)});
    }
    // What view it drew: the elements, the switches, the palette (the Systems view's legend).
    if let Some((look, v, t)) = &drew {
        out["view"] = json!(v.name());
        out["drawn"] = json!(t.drawn);
        out["full"] = json!(look.full_mep);
        out["ghost"] = json!(look.ghost);
        out["switches"] = super::bimviews::SWITCHES.iter().map(|k| (k.to_string(), json!(super::bimviews::switch_on(look, k)))).collect();
        if *v == super::bimviews::View::Systems {
            out["legend"] = json!(t.legend);
        }
    }
    deliver(png, p, &mut out)?;
    Ok(out)
}

/// The section box a render keeps to (Q6660): `section: {min: [x, y, z], max: [x, y, z]}`
/// (drawing units, as the render places the model), else the saved coordination section box
/// (`coordination.sectionbox`) while it is on; `section: false` draws without one.
fn section_box(d: &Drawing, p: &Value) -> Result<Option<(Vec3, Vec3)>> {
    let corner = |v: &Value, key: &str| -> Option<Vec3> {
        let a: Vec<f64> = v.get(key)?.as_array()?.iter().filter_map(Value::as_f64).collect();
        match a[..] {
            [x, y, z] if x.is_finite() && y.is_finite() && z.is_finite() => Some(Vec3::new(x, y, z)),
            _ => None,
        }
    };
    let both = |v: &Value| {
        corner(v, "min")
            .zip(corner(v, "max"))
            .map(|(a, b)| (Vec3::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)), Vec3::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z))))
    };
    match p.get("section") {
        Some(Value::Bool(false)) => Ok(None),
        None | Some(Value::Null) | Some(Value::Bool(true)) => {
            let saved = crate::specread::get(d, "coord_sectionbox");
            Ok(if saved.get("on").and_then(Value::as_bool) == Some(true) { both(&saved) } else { None })
        }
        Some(v) if v.is_object() => both(v).map(Some).ok_or_else(|| bad("model.render", "`section` must be {min: [x, y, z], max: [x, y, z]}")),
        Some(_) => Err(bad("model.render", "`section` must be {min: [x, y, z], max: [x, y, z]} or true/false")),
    }
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

    /// A built box mesh of `part` on L1: centreline `a`–`b` (inches), `t` thick.
    fn built_box(part: &str, h: u64, boxes: &[((f64, f64), (f64, f64), f64)]) -> Built {
        let mut m = super::super::wallgen::empty_mesh();
        for (a, b, t) in boxes {
            super::super::wallgen::add_box(&mut m, Vec2::new(a.0, a.1), Vec2::new(b.0, b.1), *t, 0.0, 100.0);
        }
        Built { handle: Handle(h), part: part.into(), level: "L1".into(), name: String::new(), source: Handle(h + 1000), mesh: m }
    }

    #[test]
    fn a_window_inside_the_building_or_in_no_wall_is_a_finding_one_in_an_outside_wall_is_not() {
        // A 240" x 240" building of 6" walls, a corridor wall across it at y = 120, its floor.
        let shell = |extra: Vec<Built>| {
            let mut v = vec![
                built_box("wall", 1, &[((0.0, 0.0), (240.0, 0.0), 6.0)]),
                built_box("wall", 2, &[((0.0, 240.0), (240.0, 240.0), 6.0)]),
                built_box("wall", 3, &[((0.0, 0.0), (0.0, 240.0), 6.0)]),
                built_box("wall", 4, &[((240.0, 0.0), (240.0, 240.0), 6.0)]),
                built_box("wall", 5, &[((3.0, 120.0), (237.0, 120.0), 6.0)]),
                built_box("floor", 6, &[((-3.0, 120.0), (243.0, 120.0), 246.0)]),
            ];
            v.extend(extra);
            v
        };
        let kinds = |made: &[Built]| windows_off_walls(made, 12.0).into_iter().map(|f| f.kind).collect::<Vec<_>>();
        // In the south outside wall (its sill box under it): none.
        let ok =
            shell(vec![built_box("wall", 7, &[((100.0, 0.0), (148.0, 0.0), 6.0)]), built_box("window", 8, &[((100.0, 0.0), (148.0, 0.0), 0.5)])]);
        assert!(kinds(&ok).is_empty(), "{:?}", kinds(&ok));
        // In the corridor wall: walls and floor past both faces.
        let corridor = shell(vec![built_box("window", 8, &[((100.0, 120.0), (148.0, 120.0), 0.5)])]);
        assert_eq!(kinds(&corridor), vec!["window_inside_building"]);
        // In a 5" line pair against the south wall's inside face.
        let sleeve =
            shell(vec![built_box("wall", 7, &[((100.0, 5.75), (148.0, 5.75), 5.0)]), built_box("window", 8, &[((100.0, 5.75), (148.0, 5.75), 0.5)])]);
        assert_eq!(kinds(&sleeve), vec!["window_inside_building"]);
        // Free in a room: in no wall.
        let free = shell(vec![built_box("window", 8, &[((100.0, 60.0), (148.0, 60.0), 0.5)])]);
        assert_eq!(kinds(&free), vec!["window_not_in_wall"]);
        // A courtyard's wall: walls past both faces, but no floor past one: outside.
        let court = vec![
            built_box("wall", 1, &[((0.0, 0.0), (240.0, 0.0), 6.0)]),
            built_box("wall", 2, &[((0.0, 240.0), (240.0, 240.0), 6.0)]),
            built_box("wall", 5, &[((0.0, 120.0), (240.0, 120.0), 6.0)]),
            built_box("floor", 6, &[((0.0, 60.0), (240.0, 60.0), 126.0)]),
            built_box("window", 8, &[((100.0, 120.0), (148.0, 120.0), 0.5)]),
        ];
        assert!(kinds(&court).is_empty(), "{:?}", kinds(&court));
    }

    /// What a build makes of the wall `a`–`b` (6" thick, 100" high) with one window cut in it
    /// (`cut`: from, to, sill, head), as built objects on L1.
    fn built_wall_with_window(h: u64, a: (f64, f64), b: (f64, f64), cut: (f64, f64, f64, f64)) -> Vec<Built> {
        super::super::wallgen::wall_with_window(Vec2::new(a.0, a.1), Vec2::new(b.0, b.1), 6.0, 100.0, cut)
            .into_iter()
            .zip(h..)
            .map(|((part, mesh), h)| Built {
                handle: Handle(h),
                part: part.into(),
                level: "L1".into(),
                name: String::new(),
                source: Handle(h + 1000),
                mesh,
            })
            .collect()
    }

    #[test]
    fn a_window_cut_full_height_in_an_outside_wall_is_in_that_wall() {
        // The building of the test above, its south wall as a build makes it with a window cut.
        let shell = |south: Vec<Built>, extra: Vec<Built>| {
            let mut v = vec![
                built_box("wall", 2, &[((0.0, 240.0), (240.0, 240.0), 6.0)]),
                built_box("wall", 3, &[((0.0, 0.0), (0.0, 240.0), 6.0)]),
                built_box("wall", 4, &[((240.0, 0.0), (240.0, 240.0), 6.0)]),
                built_box("wall", 5, &[((3.0, 120.0), (237.0, 120.0), 6.0)]),
                built_box("floor", 6, &[((-3.0, 120.0), (243.0, 120.0), 246.0)]),
            ];
            v.extend(south);
            v.extend(extra);
            v
        };
        let kinds = |made: &[Built]| windows_off_walls(made, 12.0).into_iter().map(|f| f.kind).collect::<Vec<_>>();
        // Storefront and floor-to-top glazing (sill 0, head at or over the top: no box left across
        // the cut), a window to the top, and a common one: each in the wall it is cut in. Then
        // full height at the wall's end, against the corner wall across it; and full height in the
        // gap between two walls in line, each built as a wall of its own.
        let mut cases: Vec<(String, Vec<Built>)> = [(0.0, 100.0), (0.0, 120.0), (36.0, 100.0), (36.0, 84.0)]
            .into_iter()
            .map(|(sill, head)| {
                let south = built_wall_with_window(10, (0.0, 0.0), (240.0, 0.0), (100.0, 148.0, sill, head));
                (format!("sill {sill} head {head}"), shell(south, Vec::new()))
            })
            .collect();
        cases.push(("at the corner".into(), shell(built_wall_with_window(10, (0.0, 0.0), (240.0, 0.0), (192.0, 240.0, 0.0, 100.0)), Vec::new())));
        cases.push((
            "between two walls".into(),
            shell(
                vec![built_box("wall", 1, &[((0.0, 0.0), (100.0, 0.0), 6.0)]), built_box("wall", 7, &[((148.0, 0.0), (240.0, 0.0), 6.0)])],
                vec![built_box("window", 8, &[((100.0, 0.0), (148.0, 0.0), 0.5)])],
            ),
        ));
        let wrong: Vec<(String, Vec<&str>)> = cases
            .iter()
            .inspect(|(c, made)| assert!(made.iter().any(|b| b.part == "window"), "{c}: the window was built"))
            .map(|(c, made)| (c.clone(), kinds(made)))
            .filter(|(_, k)| !k.is_empty())
            .collect();
        assert!(wrong.is_empty(), "{} of {} windows in a wall reported: {wrong:?}", wrong.len(), cases.len());
        // Still in no wall: in line with a stub wall but clear of it, and against it at one end
        // with nothing at the other.
        let south = || vec![built_box("wall", 1, &[((0.0, 0.0), (240.0, 0.0), 6.0)])];
        let stub = || built_box("wall", 7, &[((3.0, 60.0), (60.0, 60.0), 6.0)]);
        let clear = shell(south(), vec![stub(), built_box("window", 8, &[((100.0, 60.0), (148.0, 60.0), 0.5)])]);
        assert_eq!(kinds(&clear), vec!["window_not_in_wall"]);
        let one_end = shell(south(), vec![stub(), built_box("window", 8, &[((60.0, 60.0), (108.0, 60.0), 0.5)])]);
        assert_eq!(kinds(&one_end), vec!["window_not_in_wall"]);
        // A window its own wall holds across (the 5" line pair against the south wall's inside
        // face) keeps that wall as its host when a thicker wall abuts one of its jambs: still
        // inside the building.
        let abutted = shell(
            south(),
            vec![
                built_box("wall", 7, &[((100.0, 5.75), (148.0, 5.75), 5.0)]),
                built_box("wall", 9, &[((148.0, 5.75), (200.0, 5.75), 16.0)]),
                built_box("window", 8, &[((100.0, 5.75), (148.0, 5.75), 0.5)]),
            ],
        );
        assert_eq!(kinds(&abutted), vec!["window_inside_building"]);
    }

    /// A box of `part` on `level` (feet, 12 units a foot): centreline `a`–`b`, `t` thick, from
    /// `z0` to `z1`, built for the roof named `name`.
    fn on(part: &str, level: &str, name: &str, h: u64, a: (f64, f64), b: (f64, f64), t: f64, z: (f64, f64)) -> Built {
        let mut m = super::super::wallgen::empty_mesh();
        let ft = |v: (f64, f64)| Vec2::new(v.0 * 12.0, v.1 * 12.0);
        super::super::wallgen::add_box(&mut m, ft(a), ft(b), t * 12.0, z.0 * 12.0, z.1 * 12.0);
        Built { handle: Handle(h), part: part.into(), level: level.into(), name: name.into(), source: Handle(0), mesh: m }
    }

    /// The four 6" walls of a `w` × `d` ft box on `level`, from `z0` to `z1` ft.
    fn storey(level: &str, h: u64, w: f64, d: f64, z: (f64, f64)) -> Vec<Built> {
        let s = |i: u64, a, b| on("wall", level, "", h + i, a, b, 0.5, z);
        vec![s(0, (0.0, 0.0), (w, 0.0)), s(1, (w, 0.0), (w, d)), s(2, (w, d), (0.0, d)), s(3, (0.0, d), (0.0, 0.0))]
    }

    /// A 1 ft roof deck over `x0`–`x1` × `y0`–`y1` ft on ROOF, its underside at `z` ft.
    fn deck(name: &str, h: u64, (x0, x1): (f64, f64), (y0, y1): (f64, f64), z: f64) -> Built {
        let ym = (y0 + y1) / 2.0;
        on("roof", "ROOF", name, h, (x0, ym), (x1, ym), y1 - y0, (z, z + 1.0))
    }

    fn levels() -> Vec<cadaddy_walk3d::Level> {
        serde_json::from_value(json!([
            {"name": "L1", "elevation_ft": 0, "height_ft": 10},
            {"name": "L2", "elevation_ft": 10, "height_ft": 10},
            {"name": "ROOF", "elevation_ft": 20, "height_ft": 4},
        ]))
        .unwrap_or_default()
    }

    fn roof_messages(made: &[Built]) -> Vec<String> {
        roofs_off_walls(&levels(), made, 12.0).into_iter().map(|f| f.message).collect()
    }

    #[test]
    fn each_roof_piece_is_checked_against_the_walls_under_its_own_footprint() {
        // A stepped building: Level 1 is 100' × 30', Level 2 the west 40' of it (6" walls on
        // those lines: their outer faces 3" out). One roof level
        // holds both roofs: the high one on Level 2's walls, the low one over the wing on Level
        // 1's.
        let stepped = |high_z: f64, low: (f64, f64)| {
            let mut v = storey("L1", 10, 100.0, 30.0, (0.0, 10.0));
            v.extend(storey("L2", 20, 40.0, 30.0, (10.0, 20.0)));
            v.push(deck("Roof 1", 30, (0.0, 40.0), (0.0, 30.0), high_z));
            v.push(deck("Roof 2", 31, low, (0.0, 30.0), 10.0));
            v
        };
        // Each on its walls: nothing (the level-wide boxes read the roof 60' past the walls).
        let ok = stepped(20.0, (40.0, 100.0));
        assert_eq!(roof_messages(&ok), Vec::<String>::new());
        // The high roof 2' off its walls: a gap (the low roof's underside hid it).
        let lifted = stepped(22.0, (40.0, 100.0));
        let found = roof_messages(&lifted);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("Roof 1") && found[0].contains("2'-0\" over the top of L2's walls"), "{found:?}");
        // The low roof stops 10' short of the wing's east wall: off its walls on the east.
        let short = stepped(20.0, (40.0, 90.0));
        let found = roof_messages(&short);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("Roof 2") && found[0].contains("short of them on the east by 10'-3\""), "{found:?}");
        // One roof over both parts at the high roof's height: past Level 2's walls by the wing,
        // standing on nothing over it.
        let mut one = storey("L1", 10, 100.0, 30.0, (0.0, 10.0));
        one.extend(storey("L2", 20, 40.0, 30.0, (10.0, 20.0)));
        one.push(deck("Roof 1", 30, (0.0, 100.0), (0.0, 30.0), 20.0));
        let found = roof_messages(&one);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("off L2's walls: past them on the east by 59'-9\""), "{found:?}");
    }

    #[test]
    fn a_roof_shifted_off_its_one_storey_is_past_its_walls_on_one_side_and_short_on_the_other() {
        let lv: Vec<cadaddy_walk3d::Level> = serde_json::from_value(json!([
            {"name": "L1", "elevation_ft": 0, "height_ft": 10},
            {"name": "ROOF", "elevation_ft": 10, "height_ft": 4},
        ]))
        .unwrap_or_default();
        let mut v = storey("L1", 10, 40.0, 30.0, (0.0, 10.0));
        v.push(deck("Roof 1", 30, (10.0, 50.0), (0.0, 30.0), 10.0));
        let found: Vec<String> = roofs_off_walls(&lv, &v, 12.0).into_iter().map(|f| f.message).collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("short of them on the west by 10'-3\"") && found[0].contains("past them on the east by 9'-9\""), "{found:?}");
        // Over the whole storey but for a notch from the north over a stub wall in a room: open
        // over it (the roof's box holds every wall).
        let mut v = storey("L1", 10, 40.0, 30.0, (0.0, 10.0));
        v.push(on("wall", "L1", "", 20, (20.0, 10.0), (20.0, 13.0), 0.5, (0.0, 10.0)));
        v.push(deck("Roof 1", 30, (0.0, 15.0), (0.0, 30.0), 10.0));
        v.push(deck("Roof 1", 31, (15.0, 25.0), (0.0, 5.0), 10.0));
        v.push(deck("Roof 1", 32, (25.0, 40.0), (0.0, 30.0), 10.0));
        let found: Vec<String> = roofs_off_walls(&lv, &v, 12.0).into_iter().map(|f| f.message).collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("ROOF's roof is off L1's walls: open over them at "), "{found:?}");
        // Wholly off the building: still off its walls (the storey's walls as a whole).
        let mut v = storey("L1", 10, 40.0, 30.0, (0.0, 10.0));
        v.push(deck("Roof 1", 30, (60.0, 100.0), (0.0, 30.0), 10.0));
        let found: Vec<String> = roofs_off_walls(&lv, &v, 12.0).into_iter().map(|f| f.message).collect();
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("off L1's walls") && found[0].contains("past them on the east by 59'-9\""), "{found:?}");
    }

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
