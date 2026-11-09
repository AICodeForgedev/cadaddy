//! WALLGEN: the building of a 2D plan as model-space 3D objects, MESH objects on their own layers
//! at the levels' elevations. Walls are the plan's wall face pairs (`plan.walls`: two parallel
//! lines a wall's thickness apart make one wall, its ends run on into the walls they meet), cut by
//! the openings `plan.openings` finds: a door swing or door block gets a header above its head and
//! its leaf, a window block a sill, a header and glass. Levels, floors and ceilings come from Walk
//! mode's reading of the plan (`cadaddy-walk3d`); a level whose plan has no wall pairs (single-line
//! walls, or a level on another sheet) keeps Walk mode's walls, one per wall line. It uses the same rules
//! file Walk mode does (`<drawing>.walk.json` beside the drawing) and, while associative, builds
//! again whenever an object on a rules layer changes.

use std::collections::HashMap;
use std::sync::Arc;

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, MeshKind, RawTag};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::{Part, Rules, Scene};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

mod outline;
pub(crate) use outline::Floors;

/// The extended data application that marks what WALLGEN made (part, level, source object).
pub const APP: &str = "CADADDY_WALLGEN";
/// Floor slab thickness when none is given, feet.
const SLAB_FT: f64 = 0.5;
/// A door leaf's thickness, inches.
const LEAF_IN: f64 = 1.75;
/// A door or window frame: jamb and head width along the opening, and how far its casing
/// stands proud of each wall face, inches.
const FRAME_IN: f64 = 2.0;
const CASING_IN: f64 = 0.75;
/// Most objects one WALLGEN makes.
const MAX_OBJECTS: usize = 4_000_000;

/// The settings of the last WALLGEN of a drawing: what it builds again from.
#[derive(Clone, Debug, PartialEq)]
pub struct WallGen {
    pub rules: Rules,
    /// Build again when the plan changes.
    pub auto: bool,
    pub slab_ft: f64,
    /// Wall layers given to WALLGEN (wildcards, comma-separated) instead of the rules'.
    pub layers: Option<String>,
    /// The building spec `model.build` built from (its wall types, families and the parts it
    /// adds); none for a plain WALLGEN.
    pub spec: Option<cadaddy_walk3d::BuildingSpec>,
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("wallgen", "3D Building from Plan", run)
        .menu(&["Draw", "Modeling", "3D Building from Plan"])
        .params(
            "{rules?: walk rules (as in <drawing>.walk.json), rulesFile?: path, layers?: wall layers (wildcards, comma-separated; default the rules' wall layers), slabThickness?: feet (0.5), auto?: bool (default true: build again when the plan changes), clear?: true (remove what WALLGEN made)} → {walls, doors, windows, floors, ceilings, levels, removed, notes: [why walls were or weren't found]}",
        ),
        CommandSpec::new("model.update", "Update Model from Plans", run_update).long()
            .params("{} → {behind: bool (the plan had changed since the associative model was last built), rebuilt: bool}: build the model again now when edits to the plan have left it behind (it is built again anyway before the next command that reads it, and when editing stops for a moment)")
            .noundo(),
        CommandSpec::new("wallgen.plan", "Planned Walls", run_plan)
            .params("{level: name (of the building spec), window?: [[x,y],[x,y]] (only walls with an end inside), limit?: walls listed (2000, at most 20000)} → {level, walls: [{a: [x,y], b: [x,y], thickness}], total, truncated, notes}: what the level's plan was read as — the walls model.build would build there (centrelines and thickness, sheet units, before openings are cut)")
            .noundo(),
    ]
}

/// Most walls `wallgen.plan` lists.
const MAX_PLAN_LIST: usize = 20_000;

fn run_plan(s: &mut Session, p: &Value) -> Result<Value> {
    let (sp, _) = super::bspec::current(s, "wallgen.plan")?;
    let name = p.get("level").and_then(Value::as_str).ok_or_else(|| bad("wallgen.plan", "`level` is required"))?;
    let g = WallGen { rules: sp.to_rules(), auto: false, slab_ft: 0.0, layers: None, spec: Some(sp.clone()) };
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let level =
        g.rules.levels.iter().find(|l| l.name.eq_ignore_ascii_case(name.trim())).ok_or_else(|| bad("wallgen.plan", format!("no level {name:?}")))?;
    let window = p.get("window").and_then(Value::as_array).and_then(|w| {
        let pt = |v: &Value| Some(Vec2::new(v.get(0)?.as_f64()?, v.get(1)?.as_f64()?));
        let (a, b) = (pt(w.first()?)?, pt(w.get(1)?)?);
        Some((Vec2::new(a.x.min(b.x), a.y.min(b.y)), Vec2::new(a.x.max(b.x), a.y.max(b.y))))
    });
    let limit = p.get("limit").and_then(Value::as_u64).map_or(2000, |n| (n as usize).min(MAX_PLAN_LIST));
    let mut notes = Vec::new();
    let pl = plan_level(s.doc()?, &g, level, upf, &mut notes);
    let inside = |q: Vec2| window.is_none_or(|(lo, hi)| q.x >= lo.x && q.x <= hi.x && q.y >= lo.y && q.y <= hi.y);
    let walls: Vec<Value> = pl
        .walls
        .iter()
        .filter(|w| inside(w.a) || inside(w.b))
        .map(|w| json!({"a": [w.a.x, w.a.y], "b": [w.b.x, w.b.y], "thickness": w.thickness}))
        .collect();
    let total = walls.len();
    Ok(
        json!({"level": level.name, "walls": walls.into_iter().take(limit).collect::<Vec<_>>(), "total": total, "truncated": total > limit, "notes": notes}),
    )
}

/// The layer each part goes on.
pub(crate) fn layer_of(part: Part) -> &'static str {
    match part {
        Part::Wall => "3D-Walls",
        Part::Floor => "3D-Floors",
        Part::Ceiling => "3D-Ceilings",
        Part::Glass => "3D-Windows",
        Part::Door => "3D-Doors",
        Part::Device => "3D-Devices",
        Part::Stair => "3D-Stairs",
    }
}

fn part_name(part: Part) -> &'static str {
    match part {
        Part::Wall => "wall",
        Part::Floor => "floor",
        Part::Ceiling => "ceiling",
        Part::Glass => "window",
        Part::Door => "door",
        Part::Device => "device",
        Part::Stair => "stair",
    }
}

/// What WALLGEN made an object as: its part (`wall`, `floor`…), if it did.
pub fn made_as(e: &Entity) -> Option<String> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(APP))?;
    x.get(at + 1).filter(|t| t.code == 1000).map(|t| t.value.clone())
}

pub(crate) fn tag(part: Part, level: &str, source: Handle) -> Vec<RawTag> {
    let t = |code: i32, v: String| RawTag { code, value: v };
    vec![t(1001, APP.into()), t(1000, part_name(part).into()), t(1000, level.chars().take(255).collect()), t(1000, source.hex())]
}

/// Spatial grid for fast wall lookups by position.  Cell size = search radius; each wall
/// is inserted into every cell its bounding box (grown by one cell) touches, so looking up
/// the single cell at a query point finds every wall within one cell of that point.
struct WallGrid {
    cell: f64,
    map: HashMap<(i64, i64), Vec<usize>>,
}

impl WallGrid {
    fn build(walls: &[PlanWall], cell: f64) -> Self {
        let cell = cell.max(1.0);
        let mut map: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, w) in walls.iter().enumerate() {
            let (x0, x1) = (Self::k(w.a.x.min(w.b.x) - cell, cell), Self::k(w.a.x.max(w.b.x) + cell, cell));
            let (y0, y1) = (Self::k(w.a.y.min(w.b.y) - cell, cell), Self::k(w.a.y.max(w.b.y) + cell, cell));
            if (x1 - x0 + 1).unsigned_abs().saturating_mul((y1 - y0 + 1).unsigned_abs()) > 10_000 {
                continue;
            }
            for cx in x0..=x1 {
                for cy in y0..=y1 {
                    map.entry((cx, cy)).or_default().push(i);
                }
            }
        }
        Self { cell, map }
    }

    fn k(v: f64, cell: f64) -> i64 {
        (v / cell).floor() as i64
    }

    /// Wall indices whose expanded bbox covers the cell at `p`.
    fn at(&self, p: Vec2) -> &[usize] {
        self.map.get(&(Self::k(p.x, self.cell), Self::k(p.y, self.cell))).map(Vec::as_slice).unwrap_or_default()
    }
}

/// A built door's swing tag (after the source; a wall's carries its wall type, `type:<name>`, for
/// IFC's materials).
const SWING_TAG: &str = "swing:";
/// A built door's opening tag: its frame's two jambs on the plan (`x0,y0,x1,y1`).
const OPENING_TAG: &str = "opening:";

/// A built window's type mark, how it was found ([`WindowBasis`]; its provenance is the
/// `status:`/`basis:` pair, [`super::provenance::tags`]), and its sill and head above its floor
/// (sheet units).
const MARK_TAG: &str = "mark:";
const FOUND_TAG: &str = "found:";
const SILL_TAG: &str = "sill:";
const HEAD_TAG: &str = "head:";

/// A built door's opening: its frame's two jambs on its level's plan. A door shut closes it, so
/// a room traced on the built walls runs across it.
pub(crate) fn door_opening(e: &Entity) -> Option<[Vec2; 2]> {
    let v = e.common.xdata.iter().find_map(|t| t.value.strip_prefix(OPENING_TAG))?;
    let n: Vec<f64> = v.split(',').filter_map(|p| p.parse().ok()).collect();
    match n.as_slice() {
        [a, b, c, d] if n.iter().all(|x| x.is_finite()) => Some([Vec2::new(*a, *b), Vec2::new(*c, *d)]),
        _ => None,
    }
}

/// A built door's swing: hinge, open end, shut end, on its level's plan.
pub(crate) fn door_swing(e: &Entity) -> Option<[Vec2; 3]> {
    let v = e.common.xdata.iter().find_map(|t| t.value.strip_prefix(SWING_TAG))?;
    let n: Vec<f64> = v.split(',').filter_map(|p| p.parse().ok()).collect();
    match n.as_slice() {
        [a, b, c, d, e, f] if n.iter().all(|x| x.is_finite()) => Some([Vec2::new(*a, *b), Vec2::new(*c, *d), Vec2::new(*e, *f)]),
        _ => None,
    }
}

/// The rules: given, from a file, beside the drawing, from the last WALLGEN, else Walk mode's
/// defaults.
pub(crate) fn rules_for(s: &Session, p: &Value) -> Result<Rules> {
    if let Some(r) = p.get("rules").filter(|v| !v.is_null()) {
        return Rules::from_json(&r.to_string()).map_err(|e| bad("wallgen", e));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let read = |path: &std::path::Path| -> Option<std::result::Result<Rules, String>> {
            let bytes = crate::readfile::read(path).ok()?;
            Some(Rules::from_json(&String::from_utf8_lossy(&bytes)))
        };
        if let Some(f) = str_param(p, "rulesFile") {
            return match read(std::path::Path::new(f)) {
                Some(r) => r.map_err(|e| bad("wallgen", format!("{f}: {e}"))),
                None => Err(bad("wallgen", format!("{f}: the file can't be read"))),
            };
        }
        if let Some(path) = s.state()?.path.clone() {
            let side = Rules::sidecar_path(std::path::Path::new(&path));
            if let Some(r) = read(&side) {
                return r.map_err(|e| bad("wallgen", format!("{}: {e}", side.display())));
            }
        }
    }
    if let Some(g) = &s.state()?.wallgen {
        return Ok(g.rules.clone());
    }
    Ok(Rules::fallback())
}

/// One group of triangles: a part of one source object (or one floor or ceiling).
struct Group {
    part: Part,
    level: usize,
    source: Handle,
    /// Triangles, each three scene points (feet).
    tris: Vec<[[f32; 3]; 3]>,
}

/// The level a scene point (feet) belongs to: the one whose storey holds its height (at a floor
/// line, the storey above: a level's floor is at the top of the one below).
fn level_of(scene: &Scene, z: f64) -> usize {
    scene
        .levels
        .iter()
        .enumerate()
        .filter(|(_, l)| z >= l.elevation_ft - 1e-3 && z <= l.elevation_ft + l.height_ft.max(0.0) + 1e-3)
        .max_by(|a, b| a.1.elevation_ft.total_cmp(&b.1.elevation_ft))
        .map(|(i, _)| i)
        .or_else(|| {
            // Below every level (a slab's underside): the nearest above it.
            scene.levels.iter().enumerate().min_by(|a, b| (a.1.elevation_ft - z).abs().total_cmp(&(b.1.elevation_ft - z).abs())).map(|(i, _)| i)
        })
        .unwrap_or(0)
}

/// The scene's triangles grouped into objects: walls, doors and windows by their source line,
/// each floor and ceiling quad its own slab.
fn groups(scene: &Scene) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    let mut index: HashMap<(Part, u64, usize), usize> = HashMap::new();
    let n = scene.owners.len().min(scene.parts.len()).min(scene.tris.len() / 3);
    for t in 0..n {
        let (Some(owner), Some(part)) = (scene.owners.get(t), scene.parts.get(t)) else { continue };
        // Devices aren't building elements; stairs are built whole by `model.build`.
        if matches!(part, Part::Device | Part::Stair) {
            continue;
        }
        let tri = [0, 1, 2].map(|k| scene.tris.get(t * 3 + k).map_or([0.0; 3], |v| v.p));
        let zc = f64::from(tri[0][2] + tri[1][2] + tri[2][2]) / 3.0;
        let level = level_of(scene, zc);
        // Floors and ceilings: one slab per quad (two triangles in a row).
        let key = match part {
            Part::Floor | Part::Ceiling => (*part, u64::MAX - (t / 2) as u64, level),
            _ => (*part, owner.handle.0, level),
        };
        let gi = *index.entry(key).or_insert_with(|| {
            out.push(Group { part: *part, level, source: owner.handle, tris: Vec::new() });
            out.len() - 1
        });
        if let Some(g) = out.get_mut(gi) {
            g.tris.push(tri);
        }
    }
    out
}

/// A mesh from triangles mapped onto the sheet: shared corners welded, and each pair of triangles
/// that walk3d drew as one quad (`a b c`, `a c d`) put back as that quad.
fn mesh(tris: &[[[f32; 3]; 3]], to_sheet: &dyn Fn([f32; 3]) -> Vec3) -> Option<Mesh> {
    let mut vertices: Vec<Vec3> = Vec::new();
    let mut at: HashMap<[i64; 3], u32> = HashMap::new();
    let mut id = |p: [f32; 3]| -> u32 {
        let v = to_sheet(p);
        let key = [(v.x * 1e6).round() as i64, (v.y * 1e6).round() as i64, (v.z * 1e6).round() as i64];
        *at.entry(key).or_insert_with(|| {
            vertices.push(v);
            u32::try_from(vertices.len() - 1).unwrap_or(u32::MAX)
        })
    };
    let ids: Vec<[u32; 3]> = tris.iter().map(|t| [id(t[0]), id(t[1]), id(t[2])]).collect();
    let mut faces = Vec::new();
    let mut i = 0;
    while let Some(a) = ids.get(i) {
        match ids.get(i + 1) {
            Some(b) if b[0] == a[0] && b[1] == a[2] => {
                faces.push(MeshFace { v: vec![a[0], a[1], a[2], b[2]], hidden: 0 });
                i += 2;
            }
            _ => {
                faces.push(MeshFace { v: a.to_vec(), hidden: 0 });
                i += 1;
            }
        }
    }
    faces.retain(|f| {
        let mut v = f.v.clone();
        v.sort_unstable();
        v.dedup();
        v.len() >= 3
    });
    (!faces.is_empty()).then_some(Mesh { vertices, faces, kind: MeshKind::Polyface })
}

/// A floor slab: the floor quad (its top) and its underside `t` lower, closed round the sides.
fn slab(top: &Mesh, t: f64) -> Mesh {
    let n = top.vertices.len() as u32;
    let mut vertices = top.vertices.clone();
    vertices.extend(top.vertices.iter().map(|v| Vec3::new(v.x, v.y, v.z - t)));
    let mut faces = top.faces.clone();
    for f in &top.faces {
        let mut under: Vec<u32> = f.v.iter().map(|i| i + n).collect();
        under.reverse();
        faces.push(MeshFace { v: under, hidden: 0 });
        for (k, a) in f.v.iter().enumerate() {
            let b = f.v.get((k + 1) % f.v.len()).copied().unwrap_or(*a);
            faces.push(MeshFace { v: vec![b, *a, a + n, b + n], hidden: 0 });
        }
    }
    Mesh { vertices, faces, kind: MeshKind::Polyface }
}

/// A floor plate's faces at `z`: its rectangles, facing up (`down` facing down: a ceiling).
fn plate_faces(o: &outline::Outline, z: f64, down: bool) -> Mesh {
    let mut m = empty_mesh();
    for [a, b] in &o.rects {
        let base = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);
        m.vertices.extend([Vec3::new(a.x, a.y, z), Vec3::new(b.x, a.y, z), Vec3::new(b.x, b.y, z), Vec3::new(a.x, b.y, z)]);
        let mut v = vec![base, base + 1, base + 2, base + 3];
        if down {
            v.reverse();
        }
        m.faces.push(MeshFace { v, hidden: 0 });
    }
    m
}

/// A floor slab over a floor plate: its top at `z`, its underside `t` lower, closed round the
/// plate's boundary only (no sides between its rectangles).
fn plate_slab(o: &outline::Outline, z: f64, t: f64) -> Mesh {
    let mut m = plate_faces(o, z, false);
    if t <= 0.0 {
        return m;
    }
    let under = plate_faces(o, z - t, true);
    let n = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);
    m.vertices.extend(under.vertices);
    m.faces.extend(under.faces.into_iter().map(|f| MeshFace { v: f.v.iter().map(|i| i.saturating_add(n)).collect(), hidden: 0 }));
    for (a, b) in &o.edges {
        let base = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);
        m.vertices.extend([Vec3::new(a.x, a.y, z), Vec3::new(b.x, b.y, z), Vec3::new(a.x, a.y, z - t), Vec3::new(b.x, b.y, z - t)]);
        m.faces.push(MeshFace { v: vec![base + 1, base, base + 2, base + 3], hidden: 0 });
    }
    m
}

/// Remove what WALLGEN made from model space; how many objects.
pub(crate) fn remove_made(d: &mut Drawing) -> usize {
    let hs: Vec<Handle> = d.model.iter().filter(|e| made_as(e).is_some()).map(|e| e.handle).collect();
    for h in &hs {
        d.model.remove(*h);
    }
    hs.len()
}

/// How many models this process has built (WALLGEN, `model.build` and their rebuilds).
static BUILDS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many models this process has built so far (diagnostics: a rebuild nobody needed shows).
pub fn builds_so_far() -> usize {
    BUILDS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Build the drawing's 3D building from `g`; the counts.
pub(crate) fn build(s: &mut Session, g: &WallGen) -> Result<Value> {
    build_with_progress(s, g, &(web_time::Instant::now() + std::time::Duration::from_secs(3600)))
}

/// [`build`] with per-level progress events and a deadline (Q6029 MUST 2/3).
pub(crate) fn build_with_progress(s: &mut Session, g: &WallGen, deadline: &web_time::Instant) -> Result<Value> {
    BUILDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scene = cadaddy_walk3d::derive(s.doc()?, &g.rules);
    let upf = if scene.units_per_foot.is_finite() && scene.units_per_foot > 0.0 { scene.units_per_foot } else { 12.0 };
    let gs = groups(&scene);
    // Walls from the plan's face pairs, level by level (levels drawn on this sheet).
    let build_start = web_time::Instant::now();
    let mut notes = Vec::new();
    let mut planned: Vec<(usize, PlanLevel)> = Vec::new();
    let total_levels = g.rules.levels.iter().filter(|l| l.sheet.is_none()).count();
    let mut levels_done = 0usize;
    let mut total_walls = 0usize;
    let mut budget_exceeded = false;
    for (i, level) in g.rules.levels.iter().enumerate() {
        if level.sheet.is_some() {
            continue;
        }
        let Some(si) = scene.levels.iter().position(|l| l.name == level.name) else { continue };
        // A roof plan's lines are its edge, parapet and curbs, never a storey's walls: the
        // exterior build reads the roof off it.
        if roof_level(&g.rules.levels, &level.name) {
            continue;
        }
        // Q6029 MUST 3: stop if the time budget is exceeded.
        if web_time::Instant::now() > *deadline {
            let secs = build_start.elapsed().as_secs();
            notes.push(format!(
                "Build stopped at level {}/{}: {} s exceeded the time budget. {} walls built so far.",
                levels_done, total_levels, secs, total_walls
            ));
            budget_exceeded = true;
            break;
        }
        // Q6029 MUST 2: per-level progress with ETA.
        let eta_s = if levels_done > 0 {
            let avg = build_start.elapsed().as_secs_f64() / levels_done as f64;
            let remaining = total_levels.saturating_sub(levels_done);
            Some((avg * remaining as f64).round() as u64)
        } else {
            None
        };
        s.event(
            "build.progress",
            json!({
                "step": "level",
                "level": level.name,
                "level_num": levels_done + 1,
                "total_levels": total_levels,
                "walls_so_far": total_walls,
                "eta_s": eta_s,
            }),
        );
        super::guided::progress_line(&format!(
            "  model.build: walls level {} ({}/{total_levels}) at {:.1} s",
            level.name,
            levels_done + 1,
            build_start.elapsed().as_secs_f64()
        ));
        let level_start = web_time::Instant::now();
        let pl = plan_level(s.doc()?, g, level, upf, &mut notes);
        let level_ms = level_start.elapsed().as_millis();
        if !pl.walls.is_empty() {
            let walls = pl.walls.len();
            let openings = pl.leaves.len();
            total_walls += walls;
            if level_ms > 100 {
                notes.push(format!("{}: built {} walls, {} openings in {} ms.", level.name, walls, openings, level_ms));
            }
            planned.push((si, pl));
        } else if i == 0 && g.rules.levels.len() == 1 && scene.tris.is_empty() {
            notes.push("Nothing to build.".into());
        }
        levels_done += 1;
    }
    if total_levels > 1 {
        let plan_ms = build_start.elapsed().as_millis();
        if plan_ms > 200 {
            notes.push(format!("Planning {} levels took {} ms.", total_levels, plan_ms));
        }
    }
    let d = s.doc_mut()?;
    let removed = remove_made(d);
    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    for part in [Part::Wall, Part::Floor, Part::Ceiling, Part::Glass, Part::Door] {
        let name = layer_of(part);
        if d.layer(name).is_none() {
            let color = match part {
                Part::Wall => cadaddy_doc::color::Color::Index(8),
                Part::Floor => cadaddy_doc::color::Color::Index(9),
                Part::Ceiling => cadaddy_doc::color::Color::Index(254),
                Part::Glass => cadaddy_doc::color::Color::Index(4),
                _ => cadaddy_doc::color::Color::Index(32),
            };
            d.layers.push(cadaddy_doc::Layer { name: name.into(), color, ..cadaddy_doc::Layer::default() });
        }
    }
    for (si, pl) in &planned {
        let Some(level) = scene.levels.get(*si) else { continue };
        for (part, m, source, extra) in pl.meshes() {
            let mut xdata = tag(part, &level.name, source);
            xdata.extend(extra.into_iter().map(|value| RawTag { code: 1000, value }));
            let common = Common { layer: layer_of(part).into(), xdata: xdata.into(), ..Common::default() };
            if d.add(&cadaddy_doc::Space::Model, common, EntityKind::Mesh(m)).is_ok() {
                *counts.entry(part_name(part)).or_insert(0) += 1;
            }
        }
    }
    // Each planned level's floor plate: inside its exterior walls' outside faces (an L plan's
    // slab is its L), else the walls' extents when they do not close round it.
    let mut plates: HashMap<usize, outline::Outline> = HashMap::new();
    for (si, pl) in &planned {
        let Some((lo, hi)) = pl.footprint() else { continue };
        let walls: Vec<(Vec2, Vec2, f64)> = pl.walls.iter().map(|w| (w.a, w.b, w.thickness)).collect();
        match outline::building(&walls, lo, hi, upf) {
            Some(o) => {
                plates.insert(*si, o);
            }
            None => {
                if let Some(level) = scene.levels.get(*si) {
                    notes.push(format!("{}: the walls do not close round the building; its floor slab is their extents.", level.name));
                }
            }
        }
    }
    for grp in gs.iter().take(MAX_OBJECTS) {
        // Levels built from the plan's pairs keep only Walk mode's floors and ceilings.
        if matches!(grp.part, Part::Wall | Part::Door | Part::Glass) && planned.iter().any(|(si, _)| *si == grp.level) {
            continue;
        }
        let Some(level) = scene.levels.get(grp.level) else { continue };
        if roof_level(&g.rules.levels, &level.name) {
            continue;
        }
        let base = level.base;
        let to_sheet = |p: [f32; 3]| Vec3::new(base.x + f64::from(p[0]) * upf, base.y + f64::from(p[1]) * upf, f64::from(p[2]) * upf);
        let Some(mut m) = mesh(&grp.tris, &to_sheet) else { continue };
        // Walk mode pads its floors and ceilings 2' past the walls (room to walk round); a level
        // built from its plan ends them at the walls' outside faces.
        if matches!(grp.part, Part::Floor | Part::Ceiling)
            && let Some((lo, hi)) = planned.iter().find(|(si, _)| *si == grp.level).and_then(|(_, pl)| pl.footprint())
        {
            for v in &mut m.vertices {
                v.x = v.x.clamp(lo.x, hi.x);
                v.y = v.y.clamp(lo.y, hi.y);
            }
        }
        let plate = plates.get(&grp.level).filter(|_| matches!(grp.part, Part::Floor | Part::Ceiling));
        let z = m.vertices.first().map_or(0.0, |v| v.z);
        if grp.part == Part::Floor {
            m = match plate {
                Some(o) => plate_slab(o, z, g.slab_ft.max(0.0) * upf),
                None => slab(&m, g.slab_ft.max(0.0) * upf),
            };
        } else if let Some(o) = plate {
            m = plate_faces(o, z, true);
        }
        let common = Common { layer: layer_of(grp.part).into(), xdata: tag(grp.part, &level.name, grp.source).into(), ..Common::default() };
        if d.add(&cadaddy_doc::Space::Model, common, EntityKind::Mesh(m)).is_ok() {
            *counts.entry(part_name(grp.part)).or_insert(0) += 1;
        }
    }
    let c = |k: &str| counts.get(k).copied().unwrap_or(0);
    // Each planned level's window tags against its windows (Q6311).
    let window_tags: Vec<Value> = planned.iter().filter_map(|(si, pl)| scene.levels.get(*si).map(|l| pl.window_tally.to_json(&l.name))).collect();
    let mut r = json!({
        "walls": c("wall"), "doors": c("door"), "windows": c("window"), "floors": c("floor"), "ceilings": c("ceiling"),
        "levels": scene.levels.len(), "removed": removed, "notes": notes, "window_tags": window_tags,
    });
    if budget_exceeded {
        r["budget_exceeded"] = json!(true);
        r["levels_built"] = json!(levels_done);
    }
    Ok(r)
}

/// A storey before anything is built in it: its shell (the outline of the building its walls
/// stand round, rectangles in sheet units) and its corridors, stairs and elevator hoistways.
pub(crate) struct Storey {
    pub level: String,
    pub shell: Vec<[Vec2; 2]>,
    pub circulation: Vec<cadaddy_walk3d::Circulation>,
    /// The circulation labels of its plan not traced, and why.
    pub missed: Vec<String>,
}

/// Each storey's shell and circulation, before anything is built, its walls planned from the
/// plan as `model.build` plans them; the circulation traced from its labels on the plan
/// ([`circulation_label`]), and a hoistway or stair traced on one storey traced again where it
/// stands on the others (it runs through them: a storey whose plan doesn't label it still has
/// it). The roof, and a storey whose walls don't close round the building, have none.
pub(crate) fn shells(d: &Drawing, sp: &cadaddy_walk3d::BuildingSpec) -> Vec<Storey> {
    use cadaddy_walk3d::CirculationKind;
    let g = WallGen { rules: sp.to_rules(), auto: false, slab_ft: 0.0, layers: None, spec: Some(sp.clone()) };
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let mut notes = Vec::new();
    // Each storey: its walls, its shell and its circulation labels (kind, text, middle).
    let mut plans: Vec<(String, Vec<(Vec2, Vec2, f64)>, Vec<[Vec2; 2]>, Vec<(CirculationKind, String, Vec2)>)> = Vec::new();
    for level in g.rules.levels.iter().filter(|l| l.sheet.is_none() && !roof_level(&g.rules.levels, &l.name)) {
        let pl = plan_level(d, &g, level, upf, &mut notes);
        let Some((lo, hi)) = pl.footprint() else { continue };
        let walls: Vec<(Vec2, Vec2, f64)> = pl.walls.iter().map(|w| (w.a, w.b, w.thickness)).collect();
        let Some(o) = outline::building_whole(&walls, lo, hi, upf) else { continue };
        plans.push((level.name.clone(), walls, o.rects, circulation_labels(d, level)));
    }
    let trace = |walls: &[(Vec2, Vec2, f64)], shell: &[[Vec2; 2]], labels: &[(CirculationKind, String, Vec2)]| {
        let seeds: Vec<(CirculationKind, Vec2)> = labels.iter().map(|(k, _, at)| (*k, *at)).collect();
        outline::spaces(walls, shell, upf, &seeds)
    };
    let first: Vec<Vec<std::result::Result<Vec<[Vec2; 2]>, String>>> =
        plans.iter().map(|(_, walls, shell, labels)| trace(walls, shell, labels)).collect();
    let shell_list: Vec<&[[Vec2; 2]]> = plans.iter().map(|p| p.2.as_slice()).collect();
    let found: Vec<Vec<(CirculationKind, String, Option<Vec<[Vec2; 2]>>)>> = plans
        .iter()
        .zip(&first)
        .map(|(p, traced)| p.3.iter().zip(traced).map(|((k, name, _), r)| (*k, name.clone(), r.clone().ok())).collect())
        .collect();
    let mut stacked = stacked_seeds(&shell_list, &found, upf);
    let mut out = Vec::new();
    for (b, (level, walls, shell, labels)) in plans.iter().enumerate() {
        let mut labels = labels.clone();
        let own = labels.len();
        labels.extend(stacked.get_mut(b).map(std::mem::take).unwrap_or_default());
        let traced = if labels.len() > own { trace(walls, shell, &labels) } else { first.get(b).cloned().unwrap_or_default() };
        let mut missed = Vec::new();
        let mut circulation = Vec::new();
        for (n, ((kind, name, at), rects)) in labels.into_iter().zip(traced).enumerate() {
            match rects {
                Ok(rects) => circulation.push(cadaddy_walk3d::Circulation {
                    kind,
                    level: level.clone(),
                    name,
                    at: [at.x, at.y],
                    rects: rects.iter().map(|[a, b]| [[a.x, a.y], [b.x, b.y]]).collect(),
                }),
                // Another storey's stair or hoistway this one already has (or can't hold): no news.
                Err(_) if n >= own => {}
                Err(why) => missed.push(format!("'{name}' at {:?}: {why}", [at.x.round(), at.y.round()])),
            }
        }
        out.push(Storey { level: level.clone(), shell: shell.clone(), circulation, missed });
    }
    out
}

/// A storey's floor to trace its rooms on ([`Floors`]): its walls planned as `model.build` plans
/// them, inside its shell, its circulation (`sp.circulation`) taken. None for the roof, a storey
/// whose walls don't close round the building, or one whose circulation wasn't traced (its shell
/// wasn't trusted).
pub(crate) fn room_floors(d: &Drawing, sp: &cadaddy_walk3d::BuildingSpec, level: &str) -> Option<Floors> {
    if !sp.circulation.iter().any(|c| c.level.eq_ignore_ascii_case(level)) {
        return None;
    }
    let g = WallGen { rules: sp.to_rules(), auto: false, slab_ft: 0.0, layers: None, spec: Some(sp.clone()) };
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let lv = g.rules.levels.iter().find(|l| l.sheet.is_none() && l.name.eq_ignore_ascii_case(level) && !roof_level(&g.rules.levels, &l.name))?;
    let pl = plan_level(d, &g, lv, upf, &mut Vec::new());
    let (lo, hi) = pl.footprint()?;
    let walls: Vec<(Vec2, Vec2, f64)> = pl.walls.iter().map(|w| (w.a, w.b, w.thickness)).collect();
    let shell = outline::building_whole(&walls, lo, hi, upf)?;
    let taken: Vec<[Vec2; 2]> = sp
        .circulation
        .iter()
        .filter(|c| c.level.eq_ignore_ascii_case(level))
        .flat_map(|c| c.rects.iter().map(|[a, b]| [Vec2::new(a[0], a[1]), Vec2::new(b[0], b[1])]))
        .collect();
    Floors::new(&walls, &shell.rects, upf, &taken)
}

/// A level's walls as its plan reads them: centreline ends and thickness (sheet units). What a
/// wall tag on the plan types.
pub(crate) fn planned_walls(d: &Drawing, sp: &cadaddy_walk3d::BuildingSpec, level: &str) -> Vec<(Vec2, Vec2, f64)> {
    let g = WallGen { rules: sp.to_rules(), auto: false, slab_ft: 0.0, layers: None, spec: Some(sp.clone()) };
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let Some(lv) = g.rules.levels.iter().find(|l| l.sheet.is_none() && l.name.eq_ignore_ascii_case(level) && !roof_level(&g.rules.levels, &l.name))
    else {
        return Vec::new();
    };
    plan_level(d, &g, lv, upf, &mut Vec::new()).walls.iter().map(|w| (w.a, w.b, w.thickness)).collect()
}

/// How much two storeys' shells may differ in size (feet) and still be the same building, their
/// stairs and hoistways one above the other.
const STACK_TOL_FT: f64 = 2.0;

/// For each storey (its shell), where the hoistways and stairs traced on the others (`found`:
/// kind, name, floor) stand on its plan: their largest rectangle's middle moved by the shells'
/// corners. Only between storeys whose shells are the same size ([`STACK_TOL_FT`]).
fn stacked_seeds(
    shells: &[&[[Vec2; 2]]],
    found: &[Vec<(cadaddy_walk3d::CirculationKind, String, Option<Vec<[Vec2; 2]>>)>],
    upf: f64,
) -> Vec<Vec<(cadaddy_walk3d::CirculationKind, String, Vec2)>> {
    let corner = |shell: &[[Vec2; 2]]| {
        let lo = Vec2::new(shell.iter().map(|r| r[0].x).fold(f64::INFINITY, f64::min), shell.iter().map(|r| r[0].y).fold(f64::INFINITY, f64::min));
        let hi =
            Vec2::new(shell.iter().map(|r| r[1].x).fold(f64::NEG_INFINITY, f64::max), shell.iter().map(|r| r[1].y).fold(f64::NEG_INFINITY, f64::max));
        (lo, hi - lo)
    };
    let area = |r: &[Vec2; 2]| (r[1] - r[0]).x * (r[1] - r[0]).y;
    shells
        .iter()
        .enumerate()
        .map(|(b, shell_b)| {
            let (at_b, size_b) = corner(shell_b);
            let mut seeds = Vec::new();
            for (a, shell_a) in shells.iter().enumerate().filter(|(a, s)| *a != b && !s.is_empty()) {
                let (at_a, size_a) = corner(shell_a);
                if shell_b.is_empty()
                    || !matches!(
                        (size_a - size_b).len().partial_cmp(&(STACK_TOL_FT * upf)),
                        Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                    )
                {
                    continue;
                }
                for (kind, name, rects) in found.get(a).into_iter().flatten() {
                    if *kind == cadaddy_walk3d::CirculationKind::Corridor {
                        continue;
                    }
                    // Its largest rectangle's middle: on its floor whatever its shape.
                    let Some([lo, hi]) = rects.iter().flatten().max_by(|p, q| area(p).total_cmp(&area(q))) else { continue };
                    seeds.push((*kind, name.clone(), (*lo + *hi) * 0.5 - at_a + at_b));
                }
            }
            seeds
        })
        .collect()
}

/// Most circulation labels read on a level's plan.
const MAX_CIRCULATION_LABELS: usize = 200;

/// The circulation labels on a level's plan (inside its region): what each names, its text and
/// where it is set (its insertion point: a print's text box can be far bigger than its letters,
/// its middle in the next room).
fn circulation_labels(d: &Drawing, level: &cadaddy_walk3d::Level) -> Vec<(cadaddy_walk3d::CirculationKind, String, Vec2)> {
    let ents: Vec<&Arc<Entity>> =
        d.model.iter().filter(|e| made_as(e).is_none() && matches!(e.kind, EntityKind::Text(_) | EntityKind::MText(_))).collect();
    let mut out = Vec::new();
    for (_, text, at, _) in super::perceive::texts(&ents) {
        let Some(kind) = circulation_label(&text) else { continue };
        let on = level.region.is_none_or(|[x0, y0, x1, y1]| at.x >= x0.min(x1) && at.x <= x0.max(x1) && at.y >= y0.min(y1) && at.y <= y0.max(y1));
        if at.is_finite() && on {
            out.push((kind, text, at));
        }
        if out.len() >= MAX_CIRCULATION_LABELS {
            break;
        }
    }
    out
}

/// What a plan label names when it is a corridor's, a stair's or an elevator's: `CORRIDOR`,
/// `CORR.`, `HALL`, `HALLWAY`, `STAIR 01`, `STAIRS`, `STAIR #2`, `ELEV.`, `ELEVATOR 1` — the word
/// and at most a short mark. Not an elevation's `ELEV. 100'-0"` or a note about a stair.
pub(crate) fn circulation_label(text: &str) -> Option<cadaddy_walk3d::CirculationKind> {
    use cadaddy_walk3d::CirculationKind::{Corridor, Hoistway, Stair};
    let t = text.trim().to_ascii_uppercase();
    let mut words = t.split_whitespace();
    let head = words.next()?;
    let rest: Vec<&str> = words.collect();
    let mark = |m: &str| {
        let m = m.trim_start_matches('#');
        !m.is_empty() && m.len() <= 3 && m.chars().all(|c| c.is_ascii_alphanumeric())
    };
    if !(rest.is_empty() || (rest.len() == 1 && rest.iter().all(|m| mark(m)))) {
        return None;
    }
    let head = head.trim_end_matches('.');
    let kind = cadaddy_mepfix::wordrules::groups("rooms.space_kind_words").iter().find(|(_, words)| words.contains(&head)).map(|(kind, _)| *kind)?;
    match kind {
        "corridor" => Some(Corridor),
        "stair" => Some(Stair),
        "hoistway" => Some(Hoistway),
        _ => None,
    }
}

/// Whether `name` is the roof of a building with storeys under it (`Roof`, `MAIN ROOF`, `LOW
/// ROOF`): it holds the roof, its parapets and what stands on it, not walls, a floor or rooms. A
/// drawing of one level named so is still built as a storey.
pub(crate) fn roof_level(levels: &[cadaddy_walk3d::Level], name: &str) -> bool {
    levels.len() > 1 && cadaddy_walk3d::levels::canonical_level(name).is_some_and(|(_, k)| k == cadaddy_walk3d::levels::DatumKind::Roof)
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    if p.get("clear").and_then(Value::as_bool) == Some(true) {
        let n = remove_made(s.doc_mut()?);
        s.state_mut()?.wallgen = None;
        return Ok(json!({ "removed": n }));
    }
    let rules = rules_for(s, p)?;
    let slab_ft = match p.get("slabThickness") {
        None | Some(Value::Null) => s.state()?.wallgen.as_ref().map_or(SLAB_FT, |g| g.slab_ft),
        Some(v) => v
            .as_f64()
            .filter(|t| t.is_finite() && (0.0..=10.0).contains(t))
            .ok_or_else(|| bad("wallgen", "`slabThickness` must be from 0 to 10 feet"))?,
    };
    let auto = p.get("auto").and_then(Value::as_bool).unwrap_or(true);
    let layers = str_param(p, "layers").filter(|l| !l.trim().is_empty()).map(str::to_string);
    let g = WallGen { rules, auto, slab_ft, layers, spec: None };
    let r = build(s, &g)?;
    super::revise3d::record(s, &g);
    s.state_mut()?.wallgen = Some(g);
    s.echo(format!(
        "3D building: {} walls, {} doors, {} windows, {} floors, {} ceilings on {} level(s).",
        r["walls"], r["doors"], r["windows"], r["floors"], r["ceilings"], r["levels"]
    ));
    for n in r["notes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        s.echo(n.to_string());
    }
    Ok(r)
}

/// Whether `layer` holds plan objects the model is built from (the rules' layers, the spec's
/// roles other than ignored ones).
fn watched(g: &WallGen, layer: &str) -> bool {
    g.rules.layer(layer).is_some() || g.spec.as_ref().and_then(|sp| sp.role(layer)).is_some_and(|r| r.role != cadaddy_walk3d::Role::Ignore)
}

/// Whether a plan object (an object on the rules' layers that the build didn't make) differs
/// between `a` and `b`: added, removed, changed, or moved to or from a plan layer. Only the
/// objects that changed are looked at (shared chunks are skipped whole), so the cost follows the
/// edit, not the drawing's size.
fn plan_touched(a: &Drawing, b: &Drawing, g: &WallGen) -> Vec<Handle> {
    if a.model.unchanged_from(&b.model) {
        return Vec::new();
    }
    let (added, modified, erased) = a.model.changes(&b.model);
    let plan_obj = |e: &Entity| made_as(e).is_none() && watched(g, &e.common.layer);
    added
        .into_iter()
        .chain(modified)
        .chain(erased)
        .filter(|h| a.model.get(*h).is_some_and(|e| plan_obj(e)) || b.model.get(*h).is_some_and(|e| plan_obj(e)))
        .collect()
}

/// Whether object `h` is the very same in `a` and `b` (or in neither).
fn same_object(a: &Drawing, b: &Drawing, h: Handle) -> bool {
    match (a.model.get(h), b.model.get(h)) {
        (Some(x), Some(y)) => Arc::ptr_eq(x, y),
        (None, None) => true,
        _ => false,
    }
}

/// How long a person or agent must stop editing before a model the plan has moved past is built
/// again ([`Session::update_model_when_idle`] in the app's frame loop and the MCP server).
pub const IDLE_REBUILD: std::time::Duration = std::time::Duration::from_millis(1500);

/// Most drawing states remembered as behind their model.
const MAX_BEHIND: usize = 4096;

/// The drawing states (the drawing as one command left it, shared with undo) whose plan has
/// changed since the associative model was last built: an edit to the plan marks the model
/// behind instead of building it again on the spot (a large building takes minutes to build; 10
/// wall lines took 10 builds). The model is built again once, before the next command that reads
/// it, or when editing stops for [`IDLE_REBUILD`]. Undo and redo bring back states as they are,
/// so a state left behind is still known as one when undo returns to it.
#[derive(Clone, Debug, Default)]
pub struct Behind {
    /// Each state, with the build it is behind (the drawing that build left).
    states: Vec<(std::sync::Weak<Drawing>, Option<std::sync::Weak<Drawing>>)>,
    /// When the last command in a state behind the plan ran.
    last_edit: Option<web_time::Instant>,
}

fn same_state(w: &std::sync::Weak<Drawing>, d: &Arc<Drawing>) -> bool {
    std::ptr::eq(w.as_ptr(), Arc::as_ptr(d))
}

impl Behind {
    fn find(&self, d: &Arc<Drawing>) -> Option<&Option<std::sync::Weak<Drawing>>> {
        self.states.iter().find(|(w, _)| same_state(w, d)).map(|(_, b)| b)
    }
    fn holds(&self, d: &Arc<Drawing>) -> bool {
        self.find(d).is_some()
    }
    /// Whether `d` is a state behind and `built` (the last build's drawing) is newer than the
    /// build it is behind.
    fn rebuilt_since(&self, d: &Arc<Drawing>, built: Option<&Arc<Drawing>>) -> bool {
        match (self.find(d), built) {
            (Some(Some(was)), Some(b)) => !same_state(was, b),
            (Some(None), Some(_)) => true,
            _ => false,
        }
    }
    fn add(&mut self, d: &Arc<Drawing>, built: Option<&Arc<Drawing>>) {
        if self.holds(d) {
            return;
        }
        // States nothing holds any more (undo forgot them) can't come back.
        self.states.retain(|(w, _)| w.strong_count() > 0);
        if self.states.len() >= MAX_BEHIND {
            self.states.remove(0);
        }
        self.states.push((Arc::downgrade(d), built.map(Arc::downgrade)));
    }
    fn forget(&mut self, d: &Arc<Drawing>) {
        self.states.retain(|(w, _)| !same_state(w, d));
    }
    /// Whether `d` (a drawing state) is one the plan has moved past the model in.
    pub fn holds_doc(&self, d: &Arc<Drawing>) -> bool {
        self.holds(d)
    }
    /// How long since the last command in a state behind the plan (none yet: long ago).
    pub fn since_last_edit(&self) -> std::time::Duration {
        self.last_edit.map_or(std::time::Duration::MAX, |t| t.elapsed())
    }
}

/// Whether the active drawing's associative model is behind its plan (built again before the
/// next command that reads it, [`update_model`]).
pub fn model_behind(s: &Session) -> bool {
    s.state().is_ok_and(|st| st.model_behind.holds(&st.doc))
}

/// After a command: when the drawing has an associative model and an object on its rules'
/// layers changed, the model is behind the plan: marked so (built again later, [`Behind`]).
pub(crate) fn after_command(s: &mut Session, before: Option<&Arc<Drawing>>) {
    let Some(before) = before else { return };
    let Ok(st) = s.state() else { return };
    let Some(g) = st.wallgen.as_ref().filter(|g| g.auto) else { return };
    if Arc::ptr_eq(before, &st.doc) {
        touch(s);
        return;
    }
    let built = st.model_record.as_ref().and_then(|r| r.built.as_ref());
    if built.is_some_and(|b| Arc::ptr_eq(&b.0, &st.doc)) {
        return;
    }
    if st.model_behind.rebuilt_since(before, built.map(|b| &b.0)) {
        // A drawing behind, and a build newer than the one it was behind (this command's:
        // `model.update`, `model.revise`, `model.build`; or one on another branch of the undo
        // history): current when that build's plan objects are the plan's now.
        if built.is_some_and(|b| plan_touched(&b.0, &st.doc, g).is_empty()) {
            return;
        }
    } else if !st.model_behind.holds(before) {
        // Model space untouched (a command that drew on a sheet's paper space), or no plan
        // object changed: the model is as current as it was.
        let touched = plan_touched(before, &st.doc, g);
        if touched.is_empty() {
            return;
        }
        // Built during this command (a guided job builds as one of its steps) from the plan as
        // it is now: the model is current. (Only the plan objects this command changed are
        // compared with the build's: the rest of the drawing may have changed a lot since.)
        if built.is_some_and(|b| touched.iter().all(|h| same_object(&b.0, &st.doc, *h))) {
            return;
        }
    }
    // Otherwise a drawing behind stays behind whatever changed (no plan to compare).
    let built = built.map(|b| Arc::clone(&b.0));
    let Ok(st) = s.state_mut() else { return };
    let doc = Arc::clone(&st.doc);
    st.model_behind.add(&doc, built.as_ref());
    st.model_behind.last_edit = Some(web_time::Instant::now());
}

/// A command ran (undo and redo too): editing hasn't stopped, so an idle rebuild waits.
pub(crate) fn touch(s: &mut Session) {
    if let Ok(st) = s.state_mut()
        && st.model_behind.holds(&st.doc)
    {
        st.model_behind.last_edit = Some(web_time::Instant::now());
    }
}

/// Commands that run before a model behind its plan is built again: plain 2D drawing and editing,
/// layers, views, selection, undo and redo, the session's drawing list. They don't read the model,
/// so they don't wait for a build. Every other command (anything that reads, lists, counts,
/// checks, saves or plots the model) builds it first: a command missing here only waits for the
/// build, it never reads a model behind its plan.
pub(crate) fn runs_with_model_behind(id: &str) -> bool {
    const PLAIN: &[&str] = &[
        "line",
        "xline",
        "ray",
        "pline",
        "polyline",
        "lwpolyline",
        "circle",
        "arc",
        "ellipse",
        "rectang",
        "rectangle",
        "polygon",
        "donut",
        "spline",
        "point",
        "text",
        "mtext",
        "dtext",
        "leader",
        "mleader",
        "qleader",
        "hatch",
        "gradient",
        "boundary",
        "revcloud",
        "wipeout",
        "solid",
        "trace",
        "move",
        "copy",
        "rotate",
        "scale",
        "mirror",
        "offset",
        "trim",
        "extend",
        "fillet",
        "chamfer",
        "break",
        "join",
        "stretch",
        "lengthen",
        "explode",
        "erase",
        "delete",
        "oops",
        "align",
        "arrayrect",
        "arraypolar",
        "arraypath",
        "array",
        "properties.set",
        "chprop",
        "change",
        "matchprop",
        "draworder",
        "block",
        "insert",
        "attdef",
        "undo",
        "redo",
        "u",
        "mredo",
        "regen",
        "regenall",
        "redraw",
        "redrawall",
        "select",
        "pselect",
        "qselect",
        "id",
        "dist",
        "list",
        "area",
        "measuregeom",
        "cancel",
        "zoom",
        "pan",
        "view",
        "ddim",
        "dimstyle",
        "style",
        "ltscale",
        "units",
        "model.update",
        // The session's drawings: listing, switching and closing them read no model (the MCP
        // server lists them after every call).
        "document.list",
        "document.switch",
        "document.close",
        "document.closeall",
        "new",
        "qnew",
        "open",
        // A LISP routine: the commands it runs follow this list one by one; what it reads
        // itself (entget, ssget) is the drawing as it is.
        "lisp",
    ];
    // Builds that replace the model whole (building it first would be thrown away), and
    // `model.revise`, which compares the plan with the last build itself.
    const REBUILDS: &[&str] = &["model.build", "wallgen", "model.revise"];
    let id = id.to_ascii_lowercase();
    PLAIN.contains(&id.as_str())
        || REBUILDS.contains(&id.as_str())
        || ["layer.", "view.", "zoom.", "pan.", "dim", "select.", "selection.", "draw.", "osnap", "snap", "grid", "ortho", "polar"]
            .iter()
            .any(|p| id.starts_with(p))
}

/// Build the active drawing's associative model again when the plan has moved past it, keeping
/// the hand edits whose plan objects didn't change (the edits that conflict are echoed and kept
/// in the drawing's last revision, `model.revise {last: true}`). The rebuild joins the drawing
/// state it was made for: no undo step of its own (the plan edit's undo takes it back too).
/// True when it built.
pub fn update_model(s: &mut Session) -> bool {
    if s.model_updating {
        return false;
    }
    let Ok(st) = s.state() else { return false };
    if !st.model_behind.holds(&st.doc) {
        return false;
    }
    let behind = Arc::clone(&st.doc);
    let Some(g) = st.wallgen.clone().filter(|g| g.auto) else {
        if let Ok(st) = s.state_mut() {
            st.model_behind.forget(&behind);
        }
        return false;
    };
    let (old, rooms) =
        st.model_record.as_ref().map_or_else(Default::default, |r| r.plan.iter().cloned().partition::<Vec<_>, _>(|i| i.kind != "room"));
    let how = super::revise3d::Revise { by_handle: true, tolerance: None, rebuild: true, auto: true };
    s.model_updating = true;
    let r = super::revise3d::revise(s, &g, old, rooms, &how);
    s.model_updating = false;
    if let Err(e) = &r {
        s.echo(e.to_string());
    }
    // Built (the drawing is a new state now), or nothing to build, or the build failed: either
    // way this state isn't built again before every command.
    if let Ok(st) = s.state_mut()
        && Arc::ptr_eq(&st.doc, &behind)
    {
        st.model_behind.forget(&behind);
    }
    r.is_ok_and(|v| v.get("rebuilt").and_then(Value::as_bool).unwrap_or(false))
}

/// `model.update`: build an associative model behind its plan now ([`update_model`]).
fn run_update(s: &mut Session, _p: &Value) -> Result<Value> {
    let behind = model_behind(s);
    let built = update_model(s);
    Ok(json!({ "behind": behind, "rebuilt": built }))
}

// ---------- walls from the plan's face pairs ----------

/// An opening cut in a wall: from `s0` to `s1` along its centreline.
#[derive(Clone, Debug)]
struct Cut {
    s0: f64,
    s1: f64,
    window: bool,
    /// A family's head and sill above the floor (sheet units); none = the level's.
    head: Option<f64>,
    sill: Option<f64>,
}

/// A wall from a face pair: centreline (sheet units), thickness, the openings cut in it.
#[derive(Clone, Debug)]
struct PlanWall {
    a: Vec2,
    b: Vec2,
    thickness: f64,
    /// Top of the wall (sheet units).
    top: f64,
    source: Handle,
    cuts: Vec<Cut>,
    /// Boxes across the gaps after it (headers, sills): centreline ends, bottom, top.
    extra: Vec<(Vec2, Vec2, f64, f64)>,
    /// Its wall type's name: from the spec's wall marks (the tags on the plans), else its layer's.
    wall_type: Option<String>,
}

/// A door leaf (hinge to its open end) or a window pane, with what it came from.
struct Leaf {
    a: Vec2,
    b: Vec2,
    window: bool,
    /// The wall's thickness (a pane sits in the middle of it).
    thickness: f64,
    source: Handle,
    head: Option<f64>,
    sill: Option<f64>,
    /// A door swing's closing direction: from the hinge `a` along the wall to where the leaf
    /// shuts. `None` for panes and leaves drawn in the wall.
    along: Option<Vec2>,
    /// The other leaf of a pair (a double door): one door, two leaves.
    pair: Option<(Vec2, Vec2)>,
    /// The opening it fills, its ends on the wall's centreline: framed (jambs, head, a window's
    /// sill). None when another leaf frames it.
    frame: Option<(Vec2, Vec2)>,
    /// A window's type mark (its tag's, or the typical window's it stands for), and why it is a
    /// window there ([`WindowBasis`]).
    mark: Option<String>,
    basis: Option<WindowBasis>,
}

/// How a window was found where it is (`found:` tag on the built element).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowBasis {
    /// A window tag marks the plan's opening (drawn).
    Tag,
    /// Glazing lines drawn across the opening (drawn).
    Glazing,
    /// A window block or a scheduled opening's symbol (drawn).
    Symbol,
    /// The width of a window drawn or tagged elsewhere on the level, in an untagged opening
    /// (inferred: a typical window).
    Typical,
    /// A window tag by an outside wall drawn through, with no opening of its own: the tag's
    /// window cut there at its type's width (inferred position).
    TagOnWall,
}

impl WindowBasis {
    fn as_str(self) -> &'static str {
        match self {
            Self::Tag => "tag",
            Self::Glazing => "glazing",
            Self::Symbol => "symbol",
            Self::Typical => "typical",
            Self::TagOnWall => "tag_on_wall",
        }
    }

    /// Drawn on the plan (else inferred from the plan's other windows or its tag alone).
    pub(crate) fn drawn(self) -> bool {
        matches!(self, Self::Tag | Self::Glazing | Self::Symbol)
    }

    /// The basis of an inferred window (its provenance).
    fn why(self) -> &'static str {
        match self {
            Self::Typical => {
                "a typical window: the level's plan tags or glazes a window of this opening's width, this opening has no tag or glazing of its own"
            }
            Self::TagOnWall => {
                "a window tag printed by an outside wall the plan draws through: the tag's window cut there at its type's width, centred at the tag"
            }
            Self::Tag | Self::Glazing | Self::Symbol => "",
        }
    }
}

/// One level built from its plan.
struct PlanLevel {
    walls: Vec<PlanWall>,
    leaves: Vec<Leaf>,
    /// Floor, wall top, door head, window sill and head (sheet units).
    z0: f64,
    top: f64,
    door_head: f64,
    sill: f64,
    head: f64,
    /// Drawing units per inch.
    k: f64,
    /// The level's window tags against the windows built (Q6311).
    window_tally: WindowTally,
}

/// A box on the centreline `a`–`b`, `t` thick, from `z0` up to `z1`, added to `m`.
pub(crate) fn add_box(m: &mut Mesh, a: Vec2, b: Vec2, t: f64, z0: f64, z1: f64) {
    let len = a.dist(b);
    if !(len > 1e-9 && t > 1e-9 && z1 > z0) {
        return;
    }
    let u = (b - a) * (1.0 / len);
    let n = u.perp() * (t / 2.0);
    let base = u32::try_from(m.vertices.len()).unwrap_or(u32::MAX);
    for p in [a - n, b - n, b + n, a + n] {
        m.vertices.push(Vec3::new(p.x, p.y, z0));
    }
    for p in [a - n, b - n, b + n, a + n] {
        m.vertices.push(Vec3::new(p.x, p.y, z1));
    }
    let f = |v: [u32; 4]| MeshFace { v: v.iter().map(|i| base + i).collect(), hidden: 0 };
    m.faces.extend([f([3, 2, 1, 0]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])]);
}

pub(crate) fn empty_mesh() -> Mesh {
    Mesh { vertices: Vec::new(), faces: Vec::new(), kind: MeshKind::Polyface }
}

impl PlanLevel {
    /// The extents of the walls' faces (their outside faces), when it has walls.
    fn footprint(&self) -> Option<(Vec2, Vec2)> {
        let (mut lo, mut hi) = (Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY));
        for w in &self.walls {
            let len = w.a.dist(w.b);
            if !(len > 1e-9 && w.thickness.is_finite()) {
                continue;
            }
            let n = ((w.b - w.a) * (1.0 / len)).perp() * (w.thickness / 2.0);
            for p in [w.a - n, w.a + n, w.b - n, w.b + n] {
                lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
                hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
            }
        }
        (lo.x.is_finite() && lo.y.is_finite() && hi.x > lo.x && hi.y > lo.y).then_some((lo, hi))
    }

    /// The level's meshes: one per wall (its pieces, headers and sills, with its wall type), one
    /// per door leaf (with its swing and its frame's opening) and window pane (with its mark, why
    /// it is there, its sill and head): each with its extra tags.
    fn meshes(&self) -> Vec<(Part, Mesh, Handle, Vec<String>)> {
        let mut out = Vec::new();
        for w in &self.walls {
            let len = w.a.dist(w.b);
            if len <= 1e-9 {
                continue;
            }
            let u = (w.b - w.a) * (1.0 / len);
            let at = |s: f64| w.a + u * s;
            let mut m = empty_mesh();
            let mut cuts: Vec<Cut> =
                w.cuts.iter().map(|c| Cut { s0: c.s0.max(0.0), s1: c.s1.min(len), ..c.clone() }).filter(|c| c.s1 > c.s0).collect();
            cuts.sort_by(|x, y| x.s0.total_cmp(&y.s0));
            let mut s = 0.0;
            for c in &cuts {
                if c.s0 > s {
                    add_box(&mut m, at(s), at(c.s0), w.thickness, self.z0, w.top);
                }
                let (lo, hi) = if c.window {
                    (self.z0 + c.sill.unwrap_or(self.sill), self.z0 + c.head.unwrap_or(self.head))
                } else {
                    (self.z0, self.z0 + c.head.unwrap_or(self.door_head))
                };
                add_box(&mut m, at(c.s0), at(c.s1), w.thickness, self.z0, lo);
                add_box(&mut m, at(c.s0), at(c.s1), w.thickness, hi.min(w.top), w.top);
                s = s.max(c.s1);
            }
            if s < len {
                add_box(&mut m, at(s), at(len), w.thickness, self.z0, w.top);
            }
            for (a, b, lo, hi) in &w.extra {
                add_box(&mut m, *a, *b, w.thickness, *lo, *hi);
            }
            if !m.faces.is_empty() {
                let extra = w.wall_type.iter().map(|n| format!("type:{}", n.chars().take(250).collect::<String>())).collect();
                out.push((Part::Wall, m, w.source, extra));
            }
        }
        for l in &self.leaves {
            let mut m = empty_mesh();
            let (lo, hi) = if l.window {
                (self.z0 + l.sill.unwrap_or(self.sill), self.z0 + l.head.unwrap_or(self.head))
            } else {
                (self.z0, self.z0 + l.head.unwrap_or(self.door_head))
            };
            if l.window {
                add_box(&mut m, l.a, l.b, (0.5 * self.k).min(l.thickness), lo, hi);
            } else {
                for (a, b) in std::iter::once((l.a, l.b)).chain(l.pair) {
                    add_box(&mut m, a, b, LEAF_IN * self.k, lo, hi);
                }
            }
            if let Some((a, b)) = l.frame {
                self.add_frame(&mut m, a, b, l.thickness, lo, hi, l.window);
            }
            // A single door's swing as drawn, for what must keep clear of it.
            let swing = l.along.filter(|_| !l.window && l.pair.is_none()).map(|along| {
                let shut = l.a + along * l.a.dist(l.b);
                format!("{SWING_TAG}{},{},{},{},{},{}", l.a.x, l.a.y, l.b.x, l.b.y, shut.x, shut.y)
            });
            let mut extra: Vec<String> = swing.into_iter().collect();
            // A door's opening: its frame's two jambs on the plan.
            if let Some((a, b)) = l.frame.filter(|(a, b)| !l.window && a.is_finite() && b.is_finite()) {
                extra.push(format!("{OPENING_TAG}{},{},{},{}", a.x, a.y, b.x, b.y));
            }
            if l.window {
                // A window's type mark and why it is there (its provenance), its sill and head
                // above the floor (sheet units).
                extra.extend(l.mark.iter().map(|m| format!("{MARK_TAG}{}", m.chars().take(40).collect::<String>())));
                if let Some(b) = l.basis {
                    extra.push(format!("{FOUND_TAG}{}", b.as_str()));
                    let made = if b.drawn() { cadaddy_walk3d::Made::Drawn } else { cadaddy_walk3d::Made::Inferred };
                    extra.extend(super::provenance::tags(made, b.why()).into_iter().map(|t| t.value));
                }
                extra.push(format!("{SILL_TAG}{}", lo - self.z0));
                extra.push(format!("{HEAD_TAG}{}", hi - self.z0));
            }
            out.push((if l.window { Part::Glass } else { Part::Door }, m, l.source, extra));
        }
        out
    }

    /// The frame of the opening `a`–`b` (on the wall's centreline, wall `t` thick) from `lo` up
    /// to `hi`: two jambs and a head, a window's sill too, inside the opening, casings proud of
    /// both faces.
    #[allow(clippy::too_many_arguments)]
    fn add_frame(&self, m: &mut Mesh, a: Vec2, b: Vec2, t: f64, lo: f64, hi: f64, window: bool) {
        let len = a.dist(b);
        let w = FRAME_IN * self.k;
        if !(len > 2.0 * w && hi - lo > 2.0 * w && t.is_finite() && t > 0.0) {
            return;
        }
        let u = (b - a) * (1.0 / len);
        let depth = t + 2.0 * CASING_IN * self.k;
        add_box(m, a, a + u * w, depth, lo, hi);
        add_box(m, b - u * w, b, depth, lo, hi);
        add_box(m, a + u * w, b - u * w, depth, hi - w, hi);
        if window {
            add_box(m, a + u * w, b - u * w, depth, lo, lo + w);
        }
    }
}

/// Whether the wall `a` is the same wall as `b` (parallel, on the same line within their
/// thickness, overlapping over half the shorter).
fn same_wall(a: &PlanWall, b: &PlanWall) -> bool {
    let (la, lb) = (a.a.dist(a.b), b.a.dist(b.b));
    if la <= 1e-9 || lb <= 1e-9 {
        return false;
    }
    let (ua, ub) = ((a.b - a.a) * (1.0 / la), (b.b - b.a) * (1.0 / lb));
    if ua.cross(ub).abs() > 0.01 {
        return false;
    }
    if ua.perp().dot(b.a - a.a).abs() > a.thickness.max(b.thickness) / 2.0 {
        return false;
    }
    let (s0, s1) = (ua.dot(b.a - a.a), ua.dot(b.b - a.a));
    let (lo, hi) = (s0.min(s1).max(0.0), s0.max(s1).min(la));
    hi - lo > 0.5 * la.min(lb)
}

/// Joins the walls that are pieces of one: on one centreline, of one thickness, type and height,
/// overlapping, touching or a joint apart (at most [`JOINT_MAX_IN`]). A wall drawn unit by unit
/// whose faces were found piece by piece is one wall between its corners and openings, so its
/// length, the walls counted and the finishes on it are the real wall's. Each keeps its first
/// piece's source and the direction it runs. A door's jamb ends a wall: two pieces meeting where a
/// drawn swing hangs or shuts (`jambs`: its hinges and leaf ends) stay apart, as the one between
/// is a header drawn across the opening. `k` is drawing units per inch. Returns how many pieces
/// were joined away.
///
/// [`JOINT_MAX_IN`]: super::perceive::geometry::JOINT_MAX_IN
fn join_wall_pieces(walls: &mut Vec<PlanWall>, jambs: &[Vec2], k: f64) -> usize {
    use std::f64::consts::PI;
    if !(k.is_finite() && k > 0.0) {
        return 0;
    }
    let (joint, line_tol, thick_tol) = (super::perceive::geometry::JOINT_MAX_IN * k, 0.5 * k, 0.25 * k);
    // The jambs by 2' cell.
    let cell = 24.0 * k;
    let at = |p: Vec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
    let mut jamb_cells: HashMap<(i64, i64), Vec<Vec2>> = HashMap::new();
    for &p in jambs.iter().filter(|p| p.is_finite()).take(MAX_OBJECTS) {
        jamb_cells.entry(at(p)).or_default().push(p);
    }
    // A jamb within `r` (under a cell) of `q`.
    let jamb_at = |q: Vec2, r: f64| {
        let (cx, cy) = at(q);
        (cx - 1..=cx + 1).any(|x| (cy - 1..=cy + 1).any(|y| jamb_cells.get(&(x, y)).is_some_and(|ps| ps.iter().any(|p| p.dist(q) <= r))))
    };
    // The walls by line: direction (half degrees), centreline and thickness; each wall's extent
    // along the line.
    let mut lines: HashMap<(i64, i64, i64), Vec<(f64, f64, usize)>> = HashMap::new();
    for (i, w) in walls.iter().enumerate() {
        let d = w.b - w.a;
        if !(d.len() > 1e-9 && w.thickness.is_finite() && w.thickness > 0.0) {
            continue;
        }
        let half_degrees = ((d.y.atan2(d.x).rem_euclid(PI).to_degrees() * 2.0).round() as i64).rem_euclid(360);
        let ang = (half_degrees as f64 / 2.0).to_radians();
        let u = Vec2::new(ang.cos(), ang.sin());
        let off = (u.perp().dot(w.a.mid(w.b)) / line_tol).round() as i64;
        let thick = (w.thickness / thick_tol).round() as i64;
        let (s0, s1) = (u.dot(w.a).min(u.dot(w.b)), u.dot(w.a).max(u.dot(w.b)));
        lines.entry((half_degrees, off, thick)).or_default().push((s0, s1, i));
    }
    let mut gone = vec![false; walls.len()];
    let mut joined = 0usize;
    // In a fixed order: which piece keeps the wall doesn't depend on the map's.
    let mut keys: Vec<(i64, i64, i64)> = lines.keys().copied().collect();
    keys.sort_unstable();
    for key in keys {
        let Some(line) = lines.get_mut(&key) else { continue };
        if line.len() < 2 {
            continue;
        }
        line.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
        let ang = (key.0 as f64 / 2.0).to_radians();
        let u = Vec2::new(ang.cos(), ang.sin());
        // Each group: its pieces, and how far along the line it reaches.
        let mut groups: Vec<(Vec<usize>, f64, f64)> = Vec::new();
        for &(s0, s1, i) in line.iter() {
            let Some(w) = walls.get(i) else { continue };
            let fits = groups.last().is_some_and(|(g, _, end)| {
                s0 <= end + joint
                    && g.first().and_then(|&f| walls.get(f)).is_some_and(|f| {
                        // Where they meet, on the first piece's centreline.
                        let meet = f.a - u * u.dot(f.a) + u * (0.5 * (s0 + end.min(s1)));
                        f.wall_type == w.wall_type
                            && (f.top - w.top).abs() <= 1e-6 * f.top.abs().max(1.0)
                            && !jamb_at(meet, (w.thickness / 2.0 + 2.0 * k).min(cell))
                    })
            });
            match groups.last_mut() {
                Some((g, _, end)) if fits => {
                    g.push(i);
                    *end = end.max(s1);
                }
                _ => groups.push((vec![i], s0, s1)),
            }
        }
        for (g, start, end) in groups.into_iter().filter(|(g, _, _)| g.len() > 1) {
            let Some(&keep) = g.iter().min() else { continue };
            let Some(w) = walls.get_mut(keep) else { continue };
            // On the kept piece's centreline, running the way it ran.
            let c = w.a - u * u.dot(w.a);
            let (a, b) = (c + u * start, c + u * end);
            (w.a, w.b) = if u.dot(w.b - w.a) >= 0.0 { (a, b) } else { (b, a) };
            for i in g.into_iter().filter(|&i| i != keep) {
                if let Some(x) = gone.get_mut(i) {
                    *x = true;
                    joined += 1;
                }
            }
        }
    }
    let mut it = gone.iter();
    walls.retain(|_| !it.next().copied().unwrap_or(false));
    joined
}

/// Distance from `p` to the segment `a`–`b`.
fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    let t = if l2 > 1e-18 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + d * t)
}

/// Give each wall the spec's wall mark on its centreline (on this level, along it and within its
/// thickness, overlapping most of the shorter): the mark's wall type, and its height when it has
/// one (else its type's).
fn mark_walls(sp: &cadaddy_walk3d::BuildingSpec, level: &str, z0: f64, upf: f64, walls: &mut [PlanWall]) {
    let marks: Vec<&cadaddy_walk3d::WallMark> = sp.walls.iter().filter(|m| m.level.eq_ignore_ascii_case(level)).take(MAX_OBJECTS).collect();
    if marks.is_empty() {
        return;
    }
    for w in walls.iter_mut() {
        let len = w.a.dist(w.b);
        if len <= 1e-9 {
            continue;
        }
        let u = (w.b - w.a) * (1.0 / len);
        let best = marks
            .iter()
            .filter_map(|m| {
                let (a, b) = (Vec2::new(m.a[0], m.a[1]), Vec2::new(m.b[0], m.b[1]));
                let ml = a.dist(b);
                if ml.is_nan() || ml <= 1e-9 || u.cross((b - a) * (1.0 / ml)).abs() > 0.05 {
                    return None;
                }
                // Across: the mark's centreline within the wall's thickness (or 6").
                let off = u.perp().dot((a + b) * 0.5 - w.a).abs();
                if off > (w.thickness / 2.0).max(6.0 * upf / 12.0) {
                    return None;
                }
                let (s0, s1) = (u.dot(a - w.a), u.dot(b - w.a));
                let overlap = s0.max(s1).min(len) - s0.min(s1).max(0.0);
                (overlap >= 0.5 * len.min(ml)).then_some((*m, overlap))
            })
            .max_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(m, _)| m);
        let Some(m) = best else { continue };
        let Some(t) = sp.wall_types.iter().find(|t| t.name.eq_ignore_ascii_case(&m.wall_type)) else { continue };
        w.wall_type = Some(t.name.clone());
        if let Some(h) = m.height_ft.or(t.height_ft).filter(|h| h.is_finite() && *h > 0.0) {
            w.top = z0 + h * upf;
        }
    }
}

/// The spec's wall type of a wall layer.
fn wall_type<'a>(g: &'a WallGen, layer: &str) -> Option<&'a cadaddy_walk3d::WallType> {
    let sp = g.spec.as_ref()?;
    let role = sp.role(layer).filter(|r| matches!(r.role, cadaddy_walk3d::Role::Wall | cadaddy_walk3d::Role::CurtainWall))?;
    sp.wall_type_of(role)
}

/// A length in inches from an attribute value: `36`, `3'-0"`, `36"`.
fn inches(v: &str) -> Option<f64> {
    v.trim().parse::<f64>().ok().or_else(|| super::perceive::feet_inches(v)).filter(|x| x.is_finite() && *x > 0.0)
}

/// A door block's swing: the first door-sized arc (60°–100°, 16"–54") of its block as the insert
/// places it: the hinge (the arc's centre), the degrees, the arc's two ends and its radius.
fn block_swing(d: &Drawing, e: &Entity, k: f64) -> Option<(Vec2, f64, [Vec2; 2], f64)> {
    let EntityKind::Insert(ins) = &e.kind else { return None };
    let blk = d.block(&ins.block)?;
    let m = ins.transform(blk.base.xy());
    blk.entities.iter().take(10_000).find_map(|be| {
        if !matches!(be.kind, EntityKind::Arc(_)) {
            return None;
        }
        let mut kind = be.kind.clone();
        kind.transform(&m);
        // A non-uniform scale turns the arc into an ellipse: not a swing.
        let EntityKind::Arc(a) = kind else { return None };
        let mut sweep = (a.end - a.start).rem_euclid(std::f64::consts::TAU);
        if sweep < 1e-9 {
            sweep = std::f64::consts::TAU;
        }
        let deg = sweep.to_degrees();
        if !(60.0..=100.0).contains(&deg) || !(16.0 * k..=54.0 * k).contains(&a.radius) {
            return None;
        }
        let c = a.center.xy();
        Some((c, deg, [c + Vec2::from_angle(a.start) * a.radius, c + Vec2::from_angle(a.end) * a.radius], a.radius))
    })
}

/// Door and window blocks by the spec's families (and its door and window layers): their kind,
/// width (the family's, an attribute's, else as found) and head and sill heights.
pub(crate) fn fit_families(
    d: &Drawing,
    sp: &cadaddy_walk3d::BuildingSpec,
    ents: &[&Arc<Entity>],
    ops: &mut Vec<super::perceive::geometry::Opening>,
    fitted: &mut HashMap<Handle, (Option<f64>, Option<f64>)>,
    upf: f64,
) {
    use cadaddy_walk3d::{FamilyKind, Role};
    let k = upf / 12.0;
    for e in ents {
        let EntityKind::Insert(ins) = &e.kind else { continue };
        let family = sp.family(&ins.block);
        let by_layer = sp.role(&e.common.layer).map(|r| r.role).filter(|r| matches!(r, Role::Door | Role::Window));
        let window = match (family.map(|f| f.kind), by_layer) {
            (Some(kind), _) => kind == FamilyKind::Window,
            (None, Some(r)) => r == Role::Window,
            (None, None) => continue,
        };
        let attr = |tag: &Option<String>| {
            tag.as_ref().and_then(|t| ins.attribs.iter().find(|a| a.tag.eq_ignore_ascii_case(t))).and_then(|a| inches(&a.text.value))
        };
        let width = family.and_then(|f| f.width_in.or_else(|| attr(&f.width_attr))).map(|w| w * k);
        let (head, sill) = match family {
            Some(f) => {
                let sill = if window { f.sill_ft.unwrap_or(sp.openings.window_sill_ft) } else { 0.0 };
                let head = match attr(&f.height_attr) {
                    Some(h) => sill + h / 12.0,
                    None => f.head_ft.unwrap_or(if window { sp.openings.window_head_ft } else { sp.openings.door_head_ft }),
                };
                (Some(head * upf), window.then_some(sill * upf))
            }
            None => (None, None),
        };
        let kind = if window { "window" } else { "door" };
        match ops.iter_mut().find(|o| o.handle == e.handle) {
            Some(o) => {
                o.kind = kind;
                if let Some(w) = width {
                    o.width = w;
                }
            }
            None => {
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                let found = if b.is_empty() { 0.0 } else { b.width().max(b.height()) };
                ops.push(super::perceive::geometry::Opening {
                    kind,
                    at: ins.insert.xy(),
                    width: width.unwrap_or(found),
                    layer: e.common.layer.to_string(),
                    handle: e.handle,
                    swing: None,
                    block: Some(ins.block.clone()),
                    confidence: 1.0,
                    leaf: false,
                    open_end: None,
                });
            }
        }
        fitted.insert(e.handle, (head, sill));
    }
}

/// Whether `pattern` (comma-separated wildcards) matches `layer`.
fn layer_matches(pattern: &str, layer: &str) -> bool {
    pattern.split(',').any(|q| super::qselect::wildcard(q.trim(), layer))
}

/// The walls and openings of one level's plan, with notes on what was found.
/// How far past its storey's shell a wall line may reach, feet (an exterior wall's outside face is
/// the shell's edge).
const PAST_SHELL_FT: f64 = 5.0;
/// The share of a storey's wall lines the shell may leave out at most, past a few: more, and
/// the shell is what is wrong (it missed part of the building), so none are.
const MAX_PAST_SHELL_SHARE: f64 = 0.1;
const FEW_PAST_SHELL: usize = 4;

/// A storey's wall lines without those reaching more than [`PAST_SHELL_FT`] past its shell (the
/// spec's `shells`; none: all of them): either end of a line's box out there is no wall of the
/// storey (a site wall, a stray line), and it would stretch the slab and the roof.
fn inside_shell<'a>(
    d: &Drawing,
    g: &WallGen,
    level: &cadaddy_walk3d::Level,
    ents: Vec<&'a Arc<Entity>>,
    upf: f64,
    notes: &mut Vec<String>,
) -> Vec<&'a Arc<Entity>> {
    let Some(rects) = g.spec.as_ref().and_then(|sp| sp.shells.get(&level.name)).filter(|r| !r.is_empty()) else { return ents };
    let reach = PAST_SHELL_FT * upf;
    let near = |p: Vec2| {
        rects.iter().any(|r| {
            let (x0, x1, y0, y1) = (r[0].min(r[2]), r[0].max(r[2]), r[1].min(r[3]), r[1].max(r[3]));
            let dx = (x0 - p.x).max(p.x - x1).max(0.0);
            let dy = (y0 - p.y).max(p.y - y1).max(0.0);
            dx.hypot(dy) <= reach
        })
    };
    let (keep, out): (Vec<&Arc<Entity>>, Vec<&Arc<Entity>>) = ents.into_iter().partition(|e| {
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        b.is_empty() || (near(b.min) && near(b.max))
    });
    if out.is_empty() {
        return keep;
    }
    if out.len() > FEW_PAST_SHELL && out.len() as f64 > MAX_PAST_SHELL_SHARE * (keep.len() + out.len()) as f64 {
        notes.push(format!(
            "{}: {} of {} wall lines reach past the storey's shell: the shell missed part of the building, so all are built.",
            level.name,
            out.len(),
            keep.len() + out.len()
        ));
        return keep.into_iter().chain(out).collect();
    }
    notes.push(format!(
        "{}: {} wall lines reaching more than {PAST_SHELL_FT}' past the storey's shell left out (no walls of it).",
        level.name,
        out.len()
    ));
    keep
}

fn plan_level(d: &Drawing, g: &WallGen, level: &cadaddy_walk3d::Level, upf: f64, notes: &mut Vec<String>) -> PlanLevel {
    use super::perceive::geometry::{Limits, openings, pairs, segments};
    let k = upf / 12.0;
    let oz = &g.rules.openings;
    // A spec gives each wall layer its type's height (see `top_of`); the rules, one for all.
    let height_ft = g
        .rules
        .layers
        .iter()
        .filter(|_| g.spec.is_none())
        .find_map(|r| match r.kind {
            cadaddy_walk3d::LayerKind::Wall { height_ft: Some(h), .. } => Some(h),
            _ => None,
        })
        .unwrap_or(level.height_ft);
    let z0 = level.elevation_ft * upf;
    let mut pl = PlanLevel {
        walls: Vec::new(),
        leaves: Vec::new(),
        z0,
        // Just under the next floor, as Walk mode leaves 0.1' for its structure.
        top: z0 + (height_ft - 0.1).max(0.1) * upf,
        door_head: oz.door_head_ft * upf,
        sill: oz.window_sill_ft * upf,
        head: oz.window_head_ft * upf,
        k,
        window_tally: WindowTally::default(),
    };
    // The level's objects: those inside its region (all of model space without one).
    let ents: Vec<&Arc<Entity>> = d
        .model
        .iter()
        .filter(|e| made_as(e).is_none())
        .filter(|e| match level.region {
            None => true,
            Some([x0, y0, x1, y1]) => {
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                let c = b.center();
                !b.is_empty() && c.x >= x0.min(x1) && c.x <= x0.max(x1) && c.y >= y0.min(y1) && c.y <= y0.max(y1)
            }
        })
        .take(MAX_OBJECTS)
        .collect();
    let on_walls = |layer: &str| match &g.layers {
        Some(p) => layer_matches(p, layer),
        None => matches!(g.rules.layer(layer), Some(cadaddy_walk3d::LayerKind::Wall { .. })),
    };
    let wall_ents: Vec<&Arc<Entity>> = ents.iter().copied().filter(|e| on_walls(&e.common.layer)).collect();
    let wall_ents = inside_shell(d, g, level, wall_ents, upf, notes);
    // A print screens what isn't built there in grey (furniture, fixtures, grids, other trades'
    // work): on a plan with solid linework, a screened line is no wall's face.
    let solid = wall_ents.iter().filter(|e| !screened(e.common.color)).count();
    let (wall_ents, screened_ents): (Vec<&Arc<Entity>>, Vec<&Arc<Entity>>) = if solid > 0 && solid as f64 >= SOLID_SHARE * wall_ents.len() as f64 {
        wall_ents.into_iter().partition(|e| !screened(e.common.color))
    } else {
        (wall_ents, Vec::new())
    };
    let rule_names = || match &g.layers {
        Some(p) => p.clone(),
        None => g
            .rules
            .layers
            .iter()
            .filter(|r| matches!(r.kind, cadaddy_walk3d::LayerKind::Wall { .. }))
            .map(|r| r.layer.clone())
            .collect::<Vec<_>>()
            .join(", "),
    };
    let lim = Limits { min_thick: 2.5 * k, max_thick: 20.0 * k, min_len: 12.0 * k };
    if wall_ents.is_empty() {
        // Say why, and where walls seem to be.
        let all = pairs(d, &segments(&ents), &lim);
        let mut layers: Vec<String> = Vec::new();
        for p in all.iter().take(500) {
            if !layers.contains(&p.layer) && layers.len() < 5 {
                layers.push(p.layer.clone());
            }
        }
        notes.push(if layers.is_empty() {
            format!("{}: no objects on the wall layers ({}), and no wall face pairs on any layer.", level.name, rule_names())
        } else {
            format!(
                "{}: no objects on the wall layers ({}); wall face pairs are on {} — run WALLGEN with layers, or add them to the rules.",
                level.name,
                rule_names(),
                layers.join(", ")
            )
        });
        return pl;
    }
    // Layers whose wall type draws the centreline aren't paired: each line is a wall.
    let (centre_ents, pair_ents): (Vec<&Arc<Entity>>, Vec<&Arc<Entity>>) =
        wall_ents.iter().copied().partition(|e| wall_type(g, &e.common.layer).is_some_and(|t| t.from == cadaddy_walk3d::WallFrom::Centerline));
    let mut segs = segments(&pair_ents);
    // A dashed line (an overhead or hidden line drawn in pieces) is no wall's face.
    let dashes = super::perceive::geometry::drop_dashes(&mut segs, k);
    if dashes > 0 {
        notes.push(format!("{}: {dashes} dashes of dashed lines (overhead, hidden) not taken for wall faces.", level.name));
    }
    // A wall drawn unit by unit (masonry): one face, not a row of block-long walls.
    let joined = super::perceive::geometry::join_unit_runs(&mut segs, k);
    if joined > 0 {
        notes.push(format!("{}: {joined} block-long pieces of wall faces joined.", level.name));
    }
    let mut found = pairs(d, &segs, &lim);
    // A thin outline's side paired with another wall's face is the air beside that wall.
    let slivers = super::perceive::geometry::drop_sliver_pairs(&segs, &mut found, &lim);
    if slivers > 0 {
        notes.push(format!("{}: {slivers} line pairs between a thin outline (trim, a shelf) and another wall's face left out.", level.name));
    }
    // Two dimension strings side by side pair up like a wall's faces, and their extension lines too.
    let figures: Vec<Vec2> = ents
        .iter()
        .filter(|e| is_dimension_figure(e))
        .map(|e| cadaddy_doc::entity_bounds(d, e, 0))
        .filter(|b| !b.is_empty())
        .map(|b| b.center())
        .collect();
    let strings = drop_dimension_strings(&mut found, &figures, k);
    if strings > 0 {
        notes.push(format!("{}: {strings} line pairs of dimension strings and their extension lines not taken for walls.", level.name));
    }
    // A screened pair in line with a solid wall and meeting its end fills a gap in the wall (a
    // window's glazing drawn lighter): it is wall too.
    if !screened_ents.is_empty() && !found.is_empty() {
        let fills: Vec<_> =
            pairs(d, &segments(&screened_ents), &lim).into_iter().filter(|p| fills_a_wall(p.c0, p.c1, p.thickness, &found, k)).collect();
        found.extend(fills);
    }
    if found.is_empty() && centre_ents.is_empty() {
        notes.push(format!(
            "{}: {} objects on the wall layers, but no parallel faces 2½\"–20\" apart (single-line walls are built one per line).",
            level.name,
            wall_ents.len()
        ));
        return pl;
    }
    // Each wall once: the most confident, longest pair of each.
    let source = wall_ents.first().map_or(Handle(0), |e| e.handle);
    let level_top = pl.top;
    let top_of = |layer: &str| wall_type(g, layer).and_then(|t| t.height_ft).map_or(level_top, |h| z0 + h * upf);
    for p in &found {
        let w = PlanWall {
            a: p.c0,
            b: p.c1,
            thickness: p.thickness,
            top: top_of(&p.layer),
            source,
            cuts: Vec::new(),
            extra: Vec::new(),
            wall_type: wall_type(g, &p.layer).map(|t| t.name.clone()),
        };
        if !pl.walls.iter().any(|x| same_wall(x, &w)) {
            pl.walls.push(w);
        }
        if pl.walls.len() >= MAX_OBJECTS {
            break;
        }
    }
    // Pieces of one wall in line (a wall drawn unit by unit, found piece by piece): one wall.
    let jambs: Vec<Vec2> = drawn_swings(&ents, k).iter().flat_map(|sw| [sw.c, sw.ends[0], sw.ends[1]]).collect();
    let pieces = join_wall_pieces(&mut pl.walls, &jambs, k);
    if pieces > 0 {
        notes.push(format!("{}: {pieces} wall pieces in line joined into the walls they're part of.", level.name));
    }
    // A level drawn on a sheet among others (its region): what pairs up outside the building is
    // no wall (grid lines out to their bubbles, dimension strings, match lines, the border).
    if let Some(region) = level.region {
        let before = pl.walls.len();
        keep_building(&mut pl.walls, region, upf);
        if pl.walls.len() < before {
            notes.push(format!("{}: {} line pairs outside the building left out (grids, dimensions, border).", level.name, before - pl.walls.len()));
        }
    }
    // The source of each wall: the line it was found on (the nearest wall-layer object).
    // (Each object's centre worked out once: per wall and comparison it was most of a big plan's build.)
    let centres: Vec<(Vec2, Handle)> = wall_ents.iter().map(|e| (cadaddy_doc::entity_bounds(d, e, 0).center(), e.handle)).collect();
    for w in &mut pl.walls {
        let mid = (w.a + w.b) * 0.5;
        if let Some((_, h)) = centres.iter().min_by(|x, y| x.0.dist(mid).total_cmp(&y.0.dist(mid))) {
            w.source = *h;
        }
    }
    for e in &centre_ents {
        let t = wall_type(g, &e.common.layer).map_or(6.0, |t| t.thickness_in) * k;
        for sg in segments(&[*e]) {
            if pl.walls.len() < MAX_OBJECTS {
                pl.walls.push(PlanWall {
                    a: sg.a,
                    b: sg.b,
                    thickness: t,
                    top: top_of(&sg.layer),
                    source: e.handle,
                    cuts: Vec::new(),
                    extra: Vec::new(),
                    wall_type: wall_type(g, &sg.layer).map(|t| t.name.clone()),
                });
            }
        }
    }
    // Walls the plans tag: their type, and the height its rule gives there.
    if let Some(sp) = &g.spec {
        mark_walls(sp, &level.name, z0, upf, &mut pl.walls);
    }
    // Ends that meet another wall run on into it by its thickness (closing corners and tees).
    // Uses a spatial grid (cell = max wall thickness) so each endpoint only checks nearby walls.
    let snapshot = pl.walls.clone();
    let max_thick = snapshot.iter().map(|w| w.thickness).fold(0.0f64, f64::max).max(lim.max_thick);
    let ext_grid = WallGrid::build(&snapshot, 1.5 * max_thick);
    for w in &mut pl.walls {
        let len = w.a.dist(w.b);
        if len <= 1e-9 {
            continue;
        }
        let u = (w.b - w.a) * (1.0 / len);
        let meets = |p: Vec2| {
            let mut best = 0.0f64;
            for &i in ext_grid.at(p) {
                let o = &snapshot[i];
                if (o.b - o.a).cross(u).abs() <= 0.1 * o.a.dist(o.b) {
                    continue;
                }
                let dd = seg_dist(p, o.a, o.b);
                if dd <= 1.5 * o.thickness.max(w.thickness) {
                    best = best.max(o.thickness);
                }
            }
            best
        };
        let (ta, tb) = (meets(w.a), meets(w.b));
        w.a -= u * ta.min(len);
        w.b += u * tb.min(len);
    }
    // Two walls whose ends stop short of the corner they make (a storefront's corner post, a
    // column drawn between them) run on to it (Q6297).
    let corners = close_corners(&mut pl.walls, CORNER_POST_IN * k);
    if corners > 0 {
        notes.push(format!("{}: {corners} corners where two walls stopped short (a corner post between them) closed.", level.name));
    }
    // Openings: each cuts the wall it lies in.
    let all_segs = segments(&ents);
    let mut ops = openings(d, &ents, &all_segs, k);
    // A family's size and heights for the blocks it covers (sheet units above the floor).
    let mut fitted: HashMap<Handle, (Option<f64>, Option<f64>)> = HashMap::new();
    if let Some(sp) = &g.spec {
        fit_families(d, sp, &ents, &mut ops, &mut fitted, upf);
    }
    // A door block hangs from its hinge, the centre of the swing it draws (most door blocks are
    // inserted at the hinge): cut and placed as that swing, not centred on the insertion point.
    for o in ops.iter_mut().filter(|o| o.kind == "door" && o.swing.is_none() && o.block.is_some()) {
        let Some(e) = ents.iter().find(|e| e.handle == o.handle) else { continue };
        if let Some((hinge, deg, ends, radius)) = block_swing(d, e, k) {
            o.at = hinge;
            o.swing = Some((deg, ends));
            o.width = radius;
        }
    }
    // Tagged openings take their schedule row's size and heights — AFTER the swing pass so a
    // scheduled width is not overwritten by the drawn swing radius.
    let mut op_marks: HashMap<Handle, String> = HashMap::new();
    if let Some(sp) = &g.spec {
        super::schedules::size_openings(sp, &mut ops, &mut fitted, &mut op_marks, upf);
    }
    let (mut doors, mut windows) = (0, 0);
    let mut not_doors = 0usize;
    // The wall each opening is in (the hinge, or the block, on its centreline within its
    // thickness); a swing read off the lines (an arc, not a door block) only in a wall it shuts
    // along (Q5273). For a drawn swing, whether that wall runs solid through where it shuts.
    // Uses a spatial grid so each opening only checks nearby walls.
    let open_grid = WallGrid::build(&pl.walls, max_thick.max(2.0 * k));
    let placed: Vec<Option<(usize, bool)>> = ops
        .iter()
        .take(MAX_OBJECTS)
        .map(|o| {
            let drawn_swing = if o.kind == "door" && o.block.is_none() { o.swing.map(|(_, ends)| ends) } else { None };
            // The wall it opens in: the hinge (or the block) on its centreline within its
            // thickness, a swing shutting along it (`ends`: the arc ends it may shut at).
            let pick = |ends: Option<[Vec2; 2]>| -> Option<(usize, f64)> {
                // The walls by the hinge, and by the ends it may shut at (the opening's other
                // jamb).
                let mut near: Vec<usize> = open_grid.at(o.at).to_vec();
                for e in ends.into_iter().flatten() {
                    for &i in open_grid.at(e) {
                        if !near.contains(&i) {
                            near.push(i);
                        }
                    }
                }
                near.iter()
                    .filter_map(|&i| {
                        let w = pl.walls.get(i)?;
                        let len = w.a.dist(w.b);
                        if len <= 1e-9 {
                            return None;
                        }
                        let u = (w.b - w.a) * (1.0 / len);
                        let off = u.perp().dot(o.at - w.a).abs();
                        let s = u.dot(o.at - w.a);
                        let fits = ends.is_none_or(|ends| swing_shuts_along(w, o.at, ends, o.width, k));
                        // A hinge up to a leaf (and a frame: the wall's thickness) past the wall's
                        // end: the door shuts onto that end (its other jamb too short to be a wall
                        // piece).
                        let reach = o.width + w.thickness.max(2.0 * k);
                        if !(fits && off <= w.thickness.max(2.0 * k) && s >= -reach && s <= len + reach) {
                            return None;
                        }
                        // A window is the outside wall's (a line pair against its inside face,
                        // a unit's sleeve or a sill, is no host): outside walls first.
                        let inside = o.kind == "window" && !faces_outside(w.a + u * s.clamp(0.0, len), u.perp(), w.thickness, k, &pl.walls, &[]);
                        Some((i, (inside, off)))
                    })
                    .min_by(|a, b| a.1.0.cmp(&b.1.0).then(a.1.1.total_cmp(&b.1.1)))
                    .map(|(i, (_, off))| (i, off))
            };
            // A leaf drawn to one end most often stands open there (against the wall beside the
            // door), so the wall at the other end comes first; a plan drawing the leaf shut in
            // its wall still finds it by either end.
            let shut_end = drawn_swing.zip(o.open_end).map(|(ends, open)| {
                let closed = if ends[0].dist(open) <= ends[1].dist(open) { ends[1] } else { ends[0] };
                [closed, closed]
            });
            let (wi, _) = shut_end.and_then(|e| pick(Some(e))).or_else(|| pick(drawn_swing))?;
            Some((wi, drawn_swing.is_some_and(|ends| solid_through_grid(&pl.walls, Some(&open_grid), wi, o.at, ends, k))))
        })
        .collect();
    // A plan that breaks its walls for its doors (most drawn swings hang in an opening) draws
    // no door on a solid wall: a swing there, leaf or not, is a cabinet's, a shower's or a
    // fixture's. Drawings with unbroken walls keep the swings drawn with their leaf.
    let drawn = |o: &&super::perceive::geometry::Opening| o.kind == "door" && o.block.is_none() && o.swing.is_some();
    let (open_swings, solid_swings) = ops.iter().zip(&placed).filter(|(o, _)| drawn(o)).fold((0usize, 0usize), |(a, b), (_, p)| match p {
        Some((_, false)) => (a + 1, b),
        Some((_, true)) => (a, b + 1),
        None => (a, b),
    });
    let walls_broken_for_doors = open_swings >= BROKEN_WALL_SHARE * solid_swings && open_swings > 0;
    for (o, at) in ops.iter().zip(&placed) {
        let drawn_swing = if drawn(&o) { o.swing.map(|(_, ends)| ends) } else { None };
        let Some((wi, solid)) = *at else {
            not_doors += usize::from(drawn_swing.is_some());
            continue;
        };
        // A bare arc on a solid wall is no door; one with its leaf drawn is a door symbol on a
        // wall drawn unbroken (as some drawings do), cut as before.
        if drawn_swing.is_some() && solid && (!o.leaf || walls_broken_for_doors) {
            not_doors += 1;
            continue;
        }
        let Some(w) = pl.walls.get_mut(wi) else { continue };
        let len = w.a.dist(w.b);
        let u = (w.b - w.a) * (1.0 / len);
        let s_at = u.dot(o.at - w.a);
        match (o.kind, o.swing) {
            ("door", Some((_, ends))) => {
                // The closed leaf lies along the wall: the arc end most in its direction.
                let along = |e: Vec2| u.dot(e - o.at).abs();
                let (closed, open) = if along(ends[0]) >= along(ends[1]) { (ends[0], ends[1]) } else { (ends[1], ends[0]) };
                let s_end = u.dot(closed - w.a);
                let (head, _) = fitted.get(&o.handle).copied().unwrap_or((None, None));
                w.cuts.push(Cut { s0: s_at.min(s_end), s1: s_at.max(s_end), window: false, head, sill: None });
                // The leaf stands open at the jamb, its face on the opening's edge (not in the wall).
                let inset = u * (LEAF_IN * k / 2.0 * (s_end - s_at).signum());
                let along = Some(u * (s_end - s_at).signum());
                let frame = Some((w.a + u * s_at.min(s_end), w.a + u * s_at.max(s_end)));
                pl.leaves.push(Leaf {
                    a: o.at + inset,
                    b: open + inset,
                    window: false,
                    thickness: w.thickness,
                    source: o.handle,
                    head,
                    sill: None,
                    along,
                    pair: None,
                    frame,
                    mark: None,
                    basis: None,
                });
                doors += 1;
            }
            (kind, _) => {
                let window = kind == "window";
                let half = o.width / 2.0;
                let (head, sill) = fitted.get(&o.handle).copied().unwrap_or((None, None));
                w.cuts.push(Cut { s0: s_at - half, s1: s_at + half, window, head, sill });
                let on = w.a + u * s_at;
                let leaf = if window {
                    windows += 1;
                    (on - u * half, on + u * half)
                } else {
                    doors += 1;
                    (on - u * half, on + u * half)
                };
                let mark = op_marks.get(&o.handle).cloned();
                pl.leaves.push(Leaf {
                    a: leaf.0,
                    b: leaf.1,
                    window,
                    thickness: w.thickness,
                    source: o.handle,
                    head,
                    sill,
                    along: None,
                    pair: None,
                    frame: Some(leaf),
                    basis: window.then_some(if mark.is_some() { WindowBasis::Tag } else { WindowBasis::Symbol }),
                    mark: mark.filter(|_| window),
                });
            }
        }
    }
    // Gaps between two pieces of one wall line: a door where a swing hangs from a jamb (or a
    // door swing's opening covers it), a window in an outside wall, else a cased opening.
    // Uses a spatial grid so each wall only checks nearby collinear neighbors for gaps.
    let (min_gap, max_gap) = (oz.min_ft * upf, oz.max_ft * upf);
    // The level's corridors (traced before the walls are built): a gap in their air is no
    // opening (Q6160).
    let corridors: Vec<&cadaddy_walk3d::Circulation> = g
        .spec
        .iter()
        .flat_map(|sp| sp.circulation.iter())
        .filter(|c| c.kind == cadaddy_walk3d::CirculationKind::Corridor && c.level.eq_ignore_ascii_case(&level.name))
        .collect();
    let mut corridor_gaps = 0usize;
    let mut crossing_gaps: Vec<(Vec2, Vec2)> = Vec::new();
    let n = pl.walls.len().min(10_000);
    let mut gaps = Vec::new();
    let gap_cell = max_gap.max(max_thick);
    let gap_grid = WallGrid::build(&pl.walls[..n], gap_cell);
    // Slots between two pieces of one wall line narrower than any opening (a storefront's
    // mullion, a post, a joint wider than a mortar joint): nobody passes, so the run is solid
    // there, full height (Q6297). Not in a corridor's air.
    let slots: Vec<(usize, Vec2, Vec2)> = narrow_slots(&pl.walls[..n], &gap_grid, min_gap, k)
        .into_iter()
        .filter(|(i, g0, g1)| {
            let t = pl.walls.get(*i).map_or(0.0, |w| w.thickness);
            let u = (*g1 - *g0) * (1.0 / g0.dist(*g1).max(1e-12));
            !in_corridor_air(&corridors, (*g0 + *g1) * 0.5, u.perp(), t)
        })
        .collect();
    for i in 0..n {
        let a = &pl.walls[i];
        let la = a.a.dist(a.b);
        if la <= 1e-9 {
            continue;
        }
        // From either end of the piece: which end a face pair starts at is the pair's, not the
        // line's, so the two pieces either side of an opening may both start at it, or both end
        // there (Q6311: half a level's window openings were never looked at).
        for (start, end) in [(a.a, a.b), (a.b, a.a)] {
            let u = (end - start) * (1.0 / la);
            for &j in gap_grid.at(end) {
                if j >= n || i == j {
                    continue;
                }
                let b = &pl.walls[j];
                let lb = b.a.dist(b.b);
                if lb <= 1e-9
                    || u.cross((b.b - b.a) * (1.0 / lb)).abs() > 0.01
                    || u.perp().dot(b.a - start).abs() > a.thickness.max(b.thickness) / 2.0
                {
                    continue;
                }
                // `b` starts past `a`'s end, with nothing of the line between.
                let (b0, b1) = (u.dot(b.a - start).min(u.dot(b.b - start)), u.dot(b.a - start).max(u.dot(b.b - start)));
                let gap = b0 - la;
                if gap < min_gap || gap > max_gap || b1 <= la {
                    continue;
                }
                let (g0, g1) = (end, end + u * gap);
                // Each gap once (found from both its pieces when both point at it).
                let tol = a.thickness.max(b.thickness).max(k);
                if gaps.iter().any(|(_, x0, x1, ..): &(usize, Vec2, Vec2, f64, bool, bool)| {
                    (x0.dist(g0) <= tol && x1.dist(g1) <= tol) || (x0.dist(g1) <= tol && x1.dist(g0) <= tol)
                }) {
                    continue;
                }
                let gap_mid = end + u * (gap * 0.5);
                let between = gap_grid.at(gap_mid).iter().any(|&kk| {
                    kk < n && kk != i && kk != j && {
                        let c = &pl.walls[kk];
                        u.cross(c.b - c.a).abs() <= 0.01 * c.a.dist(c.b) && u.perp().dot(c.a - start).abs() <= a.thickness / 2.0 && {
                            let (c0, c1) = (u.dot(c.a - start).min(u.dot(c.b - start)), u.dot(c.a - start).max(u.dot(c.b - start)));
                            c1 > la + 1e-6 && c0 < b0 - 1e-6
                        }
                    }
                });
                if between {
                    continue;
                }
                // Another wall running across the gap (a corridor's walls between two rooms'
                // walls that happen to line up, a demising wall between two units' bath walls),
                // or the gap in a corridor's air (two units' entry or bath walls lined up across
                // it): no header spans it. A door swing hung there or the outside beyond it still
                // make it a door or a window (Q6160). Between two walls crossing its line at its
                // ends (a corridor between lined-up walls when the corridor wasn't traced): as in
                // the corridor's air.
                let corridor_air = in_corridor_air(&corridors, gap_mid, u.perp(), a.thickness);
                let crossing = !corridor_air && gap_between_crossing_walls(&pl.walls[..n], i, j, g0, g1, a.thickness.max(b.thickness));
                if crossing && crossing_gaps.len() < MAX_OBJECTS && !crossing_gaps.iter().any(|o| same_gap(*o, (g0, g1), k)) {
                    crossing_gaps.push((g0, g1));
                }
                let in_air = corridor_air || crossing;
                let no_header = in_air || gap_crossed(&pl.walls[..n], i, j, g0, g1);
                if gaps.len() < MAX_OBJECTS {
                    gaps.push((i, g0, g1, a.thickness, no_header, in_air));
                }
            }
        }
    }
    // Each slot filled full height (found from either piece: filled once).
    let mut filled: Vec<(Vec2, Vec2)> = Vec::new();
    for (i, g0, g1) in slots {
        if filled.len() >= MAX_OBJECTS || filled.iter().any(|f| same_gap(*f, (g0, g1), k)) {
            continue;
        }
        if let Some(w) = pl.walls.get_mut(i) {
            let top = w.top;
            w.extra.push((g0, g1, pl.z0, top));
            filled.push((g0, g1));
        }
    }
    if !filled.is_empty() {
        notes.push(format!(
            "{}: {} slots between pieces of a wall line narrower than an opening (a mullion, a post, a joint) built solid.",
            level.name,
            filled.len()
        ));
    }
    // Outside: no other wall, and no other wall's opening, across one side of it past its face.
    let openings_of_walls: Vec<(Vec2, Vec2, f64)> = gaps.iter().take(MAX_OBJECTS).map(|(_, g0, g1, t, ..)| (*g0, *g1, *t)).collect();
    let outside = |p: Vec2, nrm: Vec2, t: f64, walls: &[PlanWall]| faces_outside(p, nrm, t, k, walls, &openings_of_walls);
    // The swings drawn on the level (arcs and flattened arcs of a door's sweep and radius), for
    // the gaps to hang doors from.
    let swings = drawn_swings(&ents, k);
    // The window tags on the level: the schedule's tags of window rows and the texts that read
    // as window marks, each with its mark and the row's width when the schedule gives one.
    let row_width = |mark: &str| {
        g.spec
            .iter()
            .flat_map(|sp| sp.schedule.row(mark))
            .find(|r| r.kind == cadaddy_walk3d::FamilyKind::Window)
            .and_then(|r| r.width_in)
            .map(|w| w * k)
    };
    let window_tags: Vec<WindowTag> = g
        .spec
        .iter()
        .flat_map(|sp| {
            sp.schedule.tags.iter().filter(|tag| {
                tag.level.as_deref().is_none_or(|l| l.eq_ignore_ascii_case(&level.name))
                    && sp.schedule.row(&tag.mark).is_some_and(|r| r.kind == cadaddy_walk3d::FamilyKind::Window)
            })
        })
        .map(|tag| (Vec2::new(tag.at[0], tag.at[1]), tag.mark.clone()))
        .chain(window_marks_drawn(g.spec.as_ref(), &ents))
        .filter(|(p, _)| p.is_finite())
        .take(MAX_WINDOW_TAGS)
        .map(|(at, mark)| WindowTag { at, width: row_width(&mark), mark: mark.to_ascii_uppercase() })
        .collect();
    // The level's gaps in outside walls, and those a window tag marks.
    let outside_gaps: Vec<(Vec2, Vec2, f64)> = gaps
        .iter()
        .filter(|(_, g0, g1, t, _, in_air)| {
            !*in_air && outside((*g0 + *g1) * 0.5, ((*g1 - *g0) * (1.0 / g0.dist(*g1).max(1e-12))).perp(), *t, &pl.walls)
        })
        .map(|(_, g0, g1, t, ..)| (*g0, *g1, *t))
        .take(MAX_OBJECTS)
        .collect();
    // What each tag marks: the outside gap it is by (filling it, or its type's width centred in a
    // wider one), else nothing yet (the tags left over are placed after the gaps).
    let fits = window_tag_fits(&window_tags, &outside_gaps, k);
    // The window a gap holds by its tag: the tag's mark and the window's width.
    let gap_fit = |g0: Vec2, g1: Vec2| -> Option<(String, f64)> {
        window_tags.iter().zip(&fits.per_tag).find_map(|(tag, f)| {
            let f = f.as_ref()?;
            let (a, b, _) = outside_gaps.get(f.gap)?;
            same_gap((*a, *b), (g0, g1), k).then(|| (tag.mark.clone(), f.width))
        })
    };
    let gap_tagged = |g0: Vec2, g1: Vec2| gap_fit(g0, g1).is_some();
    // The outside openings the plan draws or tags a window in: a window drawn or tagged once
    // stands for the openings of its width in the level's outside walls (a typical window), of
    // its type and width (the opening's width, glazing; gap width, mark, window width).
    let typicals: Vec<(f64, Option<String>, f64)> = outside_gaps
        .iter()
        .filter_map(|(g0, g1, t)| {
            let gw = g0.dist(*g1);
            match gap_fit(*g0, *g1) {
                Some((mark, w)) => Some((gw, Some(mark), w)),
                None => glazed(&all_segs, *g0, *g1, *t, k).then_some((gw, None, gw)),
            }
        })
        .take(64)
        .collect();
    let tagged_widths: Vec<f64> = typicals.iter().map(|t| t.0).collect();
    // The typical window an opening `w` wide is: the first drawn one of its width.
    let typical_of = |w: f64| typicals.iter().find(|t| (t.0 - w).abs() <= TYPICAL_WIDTH_IN * k);
    // A mark's head and sill above the floor (drawing units), from its schedule row (and the
    // family its type names); a storefront's sill at the floor when the row gives none.
    let mark_heights = |mark: &str| -> (Option<f64>, Option<f64>) { window_heights(g.spec.as_ref(), mark, upf) };
    // (Each gap is found from both its pieces: counted once.)
    let mut bare_outside_gaps: Vec<(Vec2, Vec2)> = Vec::new();
    let (mut drawn_windows, mut typical_windows): (Vec<(Vec2, Vec2)>, Vec<(Vec2, Vec2)>) = (Vec::new(), Vec::new());
    let count_once = |v: &mut Vec<(Vec2, Vec2)>, g: (Vec2, Vec2)| {
        if v.len() < MAX_OBJECTS && !v.iter().any(|o| same_gap(*o, g, k)) {
            v.push(g);
        }
    };
    for (wi, g0, g1, t, no_header, in_air) in gaps {
        let width = g0.dist(g1);
        let u = (g1 - g0) * (1.0 / width.max(1e-12));
        // A wall meeting the gap end-on (a demising wall between two rooms' windows) parts it: a
        // pier the wall's width, full height, and an opening each side of it. Inside, a wall
        // running across the gap parts it too (a demising wall between two units' entry doors);
        // across an outside wall's gap it is a mullion's or the frame's.
        let is_outside = !in_air && outside((g0 + g1) * 0.5, u.perp(), t, &pl.walls);
        let crossing_parts = !is_outside;
        let mut piers: Vec<(f64, f64)> = pl
            .walls
            .iter()
            .enumerate()
            .filter(|(ci, c)| *ci != wi && u.cross(c.b - c.a).abs() > 0.5 * c.a.dist(c.b))
            .filter_map(|(_, c)| {
                // Where its centreline crosses the gap's line, else the end that meets it.
                let n = u.perp();
                let (da, db) = (n.dot(c.a - g0), n.dot(c.b - g0));
                let hit = if da * db < 0.0 && crossing_parts {
                    Some(c.a + (c.b - c.a) * (da / (da - db)))
                } else {
                    [c.a, c.b].into_iter().find(|e| seg_dist(*e, g0, g1) <= c.thickness / 2.0 + t)
                };
                hit.and_then(|e| {
                    let s = u.dot(e - g0);
                    (s > 0.1 * width && s < 0.9 * width).then(|| ((s - c.thickness / 2.0).max(0.0), (s + c.thickness / 2.0).min(width)))
                })
            })
            .take(16)
            .collect();
        piers.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut spans: Vec<(f64, f64)> = Vec::new();
        let mut from = 0.0;
        for (p0, p1) in &piers {
            if *p0 > from {
                spans.push((from, *p0));
            }
            from = from.max(*p1);
        }
        spans.push((from, width));
        // A door swing (an arc, or a polyline of chords) hung from a jamb of the gap or of an
        // opening a pier parts off: its hinge there, its leaf that opening wide (or half of it, a
        // pair's). Not any curve about that far from it (furniture, text, a fixture by a wide
        // opening) (Q5273).
        let mut leaves = Vec::new();
        for &(s0, s1) in &spans {
            let (j0, j1) = (g0 + u * s0, g0 + u * s1);
            let span = s1 - s0;
            let mut leaf = None;
            for sw in &swings {
                for (jamb, along) in [(j0, u), (j1, -u)] {
                    let leaf_fits = (sw.r - span).abs() <= 0.2 * span + t || (sw.r - span / 2.0).abs() <= 0.1 * span + t;
                    // It shuts across this opening: an end of its arc on the gap's line, a leaf
                    // from the hinge into the opening (not a swing hinged by the jamb that shuts
                    // along another wall, or whose open leaf rests along this line's other way).
                    let shuts = sw.ends.iter().any(|e| u.perp().dot(*e - jamb).abs() <= t + 2.0 * k && along.dot(*e - sw.c) >= 0.7 * sw.r);
                    if sw.c.dist(jamb) <= t + 2.0 * k && leaf_fits && shuts {
                        // The open end: the end farthest from the wall line.
                        let open =
                            if u.perp().dot(sw.ends[0] - jamb).abs() >= u.perp().dot(sw.ends[1] - jamb).abs() { sw.ends[0] } else { sw.ends[1] };
                        let inset = along * (LEAF_IN * k / 2.0);
                        leaf = Some((jamb + inset, jamb + inset + (open - jamb) * (span / open.dist(jamb).max(1e-12)), sw.handle, along, (j0, j1)));
                    }
                }
            }
            leaves.extend(leaf);
        }
        // Covered by a door found from its swing already.
        let covered = pl.leaves.iter().any(|l| !l.window && (seg_dist(l.a, g0, g1) <= t || l.a.dist(g0) <= t || l.a.dist(g1) <= t));
        let Some(w) = pl.walls.get(wi) else { continue };
        let source = w.source;
        let (z0, top) = (pl.z0, w.top);
        // A window only where the plan draws one (Q6293): glazing lines across the opening, or a
        // window tag by it. An outside wall's gap with neither is an opening, as inside.
        let window_spans: Vec<Option<WindowBy>> = if is_outside && !in_air && leaves.is_empty() && !covered {
            spans
                .iter()
                .map(|&(s0, s1)| {
                    if gap_tagged(g0, g1) || glazed(&all_segs, g0 + u * s0, g0 + u * s1, t, k) {
                        Some(WindowBy::Drawn)
                    } else if s1 - s0 >= MIN_WINDOW_IN * k && tagged_widths.iter().any(|w| (w - (s1 - s0)).abs() <= TYPICAL_WIDTH_IN * k) {
                        Some(WindowBy::Typical)
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut extra = Vec::new();
        if !leaves.is_empty() || covered {
            extra.push((g0, g1, z0 + pl.door_head, top));
            if !leaves.is_empty() {
                extra.extend(piers.iter().map(|(p0, p1)| (g0 + u * *p0, g0 + u * *p1, z0, top)));
            }
            for (a, b, h, along, frame) in leaves.into_iter().filter(|_| !covered) {
                pl.leaves.push(Leaf {
                    a,
                    b,
                    window: false,
                    thickness: t,
                    source: h,
                    head: None,
                    sill: None,
                    along: Some(along),
                    pair: None,
                    frame: Some(frame),
                    mark: None,
                    basis: None,
                });
                doors += 1;
            }
        } else if window_spans.iter().any(Option::is_some) {
            // (A gap in a corridor's air is inside the building, whatever a ray finds.)
            extra.extend(piers.iter().map(|(p0, p1)| (g0 + u * *p0, g0 + u * *p1, z0, top)));
            let single = spans.len() == 1;
            for ((s0, s1), by) in spans.into_iter().zip(window_spans) {
                let (a, b) = (g0 + u * s0, g0 + u * s1);
                if s1 - s0 < MIN_WINDOW_IN * k {
                    extra.push((a, b, z0, top));
                    continue;
                }
                let Some(by) = by else {
                    // Beside a window, a span with no glazing is a cased opening.
                    extra.push((a, b, z0 + pl.door_head, top));
                    continue;
                };
                match by {
                    WindowBy::Drawn => count_once(&mut drawn_windows, (a, b)),
                    WindowBy::Typical => count_once(&mut typical_windows, (a, b)),
                }
                // Its type: the tag's mark and width, else the typical window's of the opening's
                // width; glazing alone gives no type.
                let (mark, width, basis) = match by {
                    WindowBy::Drawn => match gap_fit(g0, g1) {
                        Some((m, w)) => (Some(m), w, WindowBasis::Tag),
                        None => (None, s1 - s0, WindowBasis::Glazing),
                    },
                    WindowBy::Typical => {
                        let ty = typical_of(s1 - s0);
                        (ty.and_then(|t| t.1.clone()), ty.map_or(s1 - s0, |t| t.2), WindowBasis::Typical)
                    }
                };
                // A window narrower than the opening it is in (its type's width, the plan's
                // opening wider: Q6311) stands in its middle, wall either side.
                let (a, b) = if single && width >= MIN_WINDOW_IN * k && width < (s1 - s0) - TYPICAL_WIDTH_IN * k {
                    let mid = (s0 + s1) / 2.0;
                    let (a2, b2) = (g0 + u * (mid - width / 2.0), g0 + u * (mid + width / 2.0));
                    extra.push((a, a2, z0, top));
                    extra.push((b2, b, z0, top));
                    (a2, b2)
                } else {
                    (a, b)
                };
                let (head, sill) = mark.as_deref().map_or((None, None), mark_heights);
                extra.push((a, b, z0, z0 + sill.unwrap_or(pl.sill)));
                extra.push((a, b, z0 + head.unwrap_or(pl.head), top));
                pl.leaves.push(Leaf {
                    a,
                    b,
                    window: true,
                    thickness: t,
                    source,
                    head,
                    sill,
                    along: None,
                    pair: None,
                    frame: Some((a, b)),
                    mark,
                    basis: Some(basis),
                });
                windows += 1;
            }
        } else {
            if !window_spans.is_empty() {
                count_once(&mut bare_outside_gaps, (g0, g1));
            }
            if no_header {
                corridor_gaps += 1;
            } else if width <= 8.0 * upf {
                extra.push((g0, g1, z0 + pl.door_head, top));
            }
        }
        if let Some(w) = pl.walls.get_mut(wi) {
            w.extra.extend(extra.into_iter().filter(|(_, _, lo, hi)| hi > lo));
        }
    }
    if corridor_gaps > 0 {
        notes.push(format!(
            "{}: {corridor_gaps} gaps between lined-up walls (across a corridor, or another wall across them) left open (no header).",
            level.name
        ));
    }
    if !crossing_gaps.is_empty() {
        notes.push(format!(
            "{}: {} gaps between two walls that cross their line at both ends (a corridor between lined-up walls) taken as no opening: no window, no header.",
            level.name,
            crossing_gaps.len()
        ));
    }
    let (bare_outside_gaps, drawn_windows, typical_windows) = (bare_outside_gaps.len(), drawn_windows.len(), typical_windows.len());
    if drawn_windows + typical_windows > 0 {
        notes.push(format!(
            "{}: windows in outside walls' gaps: {drawn_windows} drawn (glazing lines or a window tag by the gap), {typical_windows} inferred (no glazing or tag of their own, the width of a drawn one on the level: a typical window).",
            level.name
        ));
    }
    if bare_outside_gaps > 0 {
        notes.push(format!(
            "{}: {bare_outside_gaps} gaps in outside walls with no window drawn (no glazing lines, no window tag) left as openings, not windows.",
            level.name
        ));
    }
    // The tags no opening took (Q6311): a window built within the tag's reach with no tag of its
    // own (glazing, a typical one) is the tag's type; a window of the same mark there already is
    // the same window tagged twice; else, by an outside wall the plan draws through, the tag's
    // window is cut there at its type's width (an inferred position); else the tag has no wall to
    // host it (a finding).
    let reach = TAG_REACH_IN * k;
    let mut no_host: Vec<(String, Vec2, &'static str)> = Vec::new();
    for (tag, f) in window_tags.iter().zip(&fits.per_tag) {
        if f.is_some() {
            continue;
        }
        let near = |l: &Leaf| l.window && seg_dist(tag.at, l.a, l.b) <= reach;
        if pl.leaves.iter().any(|l| near(l) && l.mark.as_deref() == Some(tag.mark.as_str())) {
            continue;
        }
        let untyped = pl
            .leaves
            .iter()
            .enumerate()
            .filter(|(_, l)| near(l) && l.basis.is_none_or(|b| !matches!(b, WindowBasis::Tag | WindowBasis::TagOnWall)))
            .min_by(|x, y| seg_dist(tag.at, x.1.a, x.1.b).total_cmp(&seg_dist(tag.at, y.1.a, y.1.b)))
            .map(|(i, _)| i);
        if let Some(l) = untyped.and_then(|i| pl.leaves.get_mut(i)) {
            l.mark = Some(tag.mark.clone());
            l.basis = Some(WindowBasis::Tag);
            continue;
        }
        let Some(width) = fits.width_of(&tag.mark) else {
            no_host.push((tag.mark.clone(), tag.at, "no width: no schedule row and no opening of its own"));
            continue;
        };
        let Some(host) = host_wall(&pl.walls, tag.at, width, reach, k, min_gap, &|p, nrm, t| outside(p, nrm, t, &pl.walls)) else {
            no_host.push((tag.mark.clone(), tag.at, "no outside wall within its reach to hold it"));
            continue;
        };
        let (head, sill) = mark_heights(&tag.mark);
        for (j, s0, s1) in &host.spans {
            if let Some(o) = pl.walls.get_mut(*j) {
                o.cuts.push(Cut { s0: *s0, s1: *s1, window: true, head, sill });
            }
        }
        // The slots it spans were built solid: the window stands there instead.
        let half_w = width / 2.0;
        for o in pl.walls.iter_mut().take(MAX_OBJECTS) {
            let t = o.thickness;
            o.extra.retain(|(e0, e1, ..)| {
                let m = (*e0 + *e1) * 0.5 - host.mid;
                !(host.u.perp().dot(m).abs() <= t / 2.0 && host.u.dot(m).abs() < half_w)
            });
        }
        let Some(w) = pl.walls.get(host.wall) else { continue };
        let len = w.a.dist(w.b);
        let u = (w.b - w.a) * (1.0 / len.max(1e-12));
        let half = width / 2.0;
        let on = w.a + u * host.at;
        let (a, b) = (on - u * half, on + u * half);
        let (thickness, source) = (w.thickness, w.source);
        pl.leaves.push(Leaf {
            a,
            b,
            window: true,
            thickness,
            source,
            head,
            sill,
            along: None,
            pair: None,
            frame: Some((a, b)),
            mark: Some(tag.mark.clone()),
            basis: Some(WindowBasis::TagOnWall),
        });
        windows += 1;
    }
    let (doors, windows) = one_per_swing(&mut pl.leaves, 6.0 * k).unwrap_or((doors, windows));
    pl.window_tally = WindowTally::of(&window_tags, &pl.leaves, no_host, g.spec.as_ref());
    if !pl.window_tally.no_host.is_empty() {
        notes.push(format!(
            "{}: {} window tags with no window built (no opening, no outside wall to hold them).",
            level.name,
            pl.window_tally.no_host.len()
        ));
    }
    let doors = doors.saturating_sub(one_door_per_opening(&mut pl.leaves, 6.0 * k));
    notes.push(format!("{}: {} walls from {} face pairs, {} doors, {} windows.", level.name, pl.walls.len(), found.len(), doors, windows));
    if not_doors > 0 {
        notes.push(format!("{}: {not_doors} arcs not taken for doors (no open wall where they shut: fixtures, equipment, furniture).", level.name));
    }
    pl
}

/// Two walls' ends this far (inches) or less short of the corner they make are one corner: a
/// corner post or a column drawn between them, not an opening (an opening is wider).
const CORNER_POST_IN: f64 = 12.0;

/// Runs on the ends of two walls that make a corner but stop short of it: walls at least 30°
/// apart, an end of each up to `reach` (drawing units) short of where their centrelines cross,
/// neither running past it. Each end runs on to the crossing and on by half the other's
/// thickness, as the run-on step closes a corner. An end takes its nearest such corner. Returns
/// how many corners were closed.
fn close_corners(walls: &mut [PlanWall], reach: f64) -> usize {
    if !(reach.is_finite() && reach > 0.0) {
        return 0;
    }
    let max_thick = walls.iter().map(|w| w.thickness).filter(|t| t.is_finite()).fold(0.0f64, f64::max);
    let grid = WallGrid::build(walls, reach + max_thick);
    // Each end (wall, which end): how far short of a corner, the point it runs on to.
    let mut ends: HashMap<(usize, bool), (f64, Vec2)> = HashMap::new();
    let mut corners: Vec<((usize, bool), (usize, bool), f64, Vec2, Vec2)> = Vec::new();
    for (i, w) in walls.iter().enumerate() {
        let lw = w.a.dist(w.b);
        if !(lw > 1e-9 && w.thickness.is_finite()) {
            continue;
        }
        for (at_b, p, u) in [(true, w.b, (w.b - w.a) * (1.0 / lw)), (false, w.a, (w.a - w.b) * (1.0 / lw))] {
            for &j in grid.at(p) {
                let Some(o) = walls.get(j).filter(|_| j > i) else { continue };
                let lo = o.a.dist(o.b);
                if !(lo > 1e-9 && o.thickness.is_finite()) {
                    continue;
                }
                let v_ab = (o.b - o.a) * (1.0 / lo);
                let den = u.cross(v_ab);
                if den.abs() < 0.5 {
                    continue;
                }
                for (o_at_b, q, v) in [(true, o.b, v_ab), (false, o.a, -v_ab)] {
                    // The crossing: p + u·t1 = q + v·t2.
                    let den = u.cross(v);
                    let t1 = (q - p).cross(v) / den;
                    let t2 = (q - p).cross(u) / den;
                    if !(t1 > 1e-9 && t1 <= reach && t2 > 1e-9 && t2 <= reach) {
                        continue;
                    }
                    let x = p + u * t1;
                    corners.push(((i, at_b), (j, o_at_b), t1.max(t2), x + u * (o.thickness / 2.0), x + v * (w.thickness / 2.0)));
                }
            }
        }
    }
    // Nearest first: an end takes the nearest corner both its walls agree on.
    corners.sort_by(|a, b| a.2.total_cmp(&b.2));
    let mut closed = 0usize;
    for (e1, e2, d, p1, p2) in corners.into_iter().take(MAX_OBJECTS) {
        if ends.contains_key(&e1) || ends.contains_key(&e2) {
            continue;
        }
        ends.insert(e1, (d, p1));
        ends.insert(e2, (d, p2));
        closed += 1;
    }
    for ((i, at_b), (_, to)) in ends {
        if let Some(w) = walls.get_mut(i) {
            if at_b {
                w.b = to;
            } else {
                w.a = to;
            }
        }
    }
    closed
}

/// The slots between pieces of one wall line narrower than an opening (`min_gap`, drawing units),
/// wider than a hair (`k` units per inch): for each, a piece beside it and the slot's ends. A
/// piece of the line runs on past the slot (in line within the walls' half thickness, nothing of
/// the line between), whichever way either piece was drawn; each slot is listed once.
fn narrow_slots(walls: &[PlanWall], grid: &WallGrid, min_gap: f64, k: f64) -> Vec<(usize, Vec2, Vec2)> {
    let mut out: Vec<(usize, Vec2, Vec2)> = Vec::new();
    if !(min_gap.is_finite() && min_gap > 0.0 && k.is_finite() && k > 0.0) {
        return out;
    }
    let hair = 0.01 * k;
    for (i, a) in walls.iter().enumerate() {
        let la = a.a.dist(a.b);
        if la.is_nan() || la <= 1e-9 {
            continue;
        }
        // From each end, looking on along the line.
        for (end, u) in [(a.b, (a.b - a.a) * (1.0 / la)), (a.a, (a.a - a.b) * (1.0 / la))] {
            // The nearest piece of the line past this end, and how far past.
            let mut next: Option<(f64, f64)> = None;
            for &j in grid.at(end) {
                let Some(b) = walls.get(j).filter(|_| j != i) else { continue };
                let lb = b.a.dist(b.b);
                if lb.is_nan()
                    || lb <= 1e-9
                    || u.cross((b.b - b.a) * (1.0 / lb)).abs() > 0.01
                    || u.perp().dot(b.a - end).abs() > a.thickness.max(b.thickness) / 2.0
                {
                    continue;
                }
                let (s0, s1) = (u.dot(b.a - end).min(u.dot(b.b - end)), u.dot(b.a - end).max(u.dot(b.b - end)));
                if s1 <= hair {
                    continue;
                }
                // A piece overlapping the end closes it already.
                let gap = s0.max(0.0);
                if next.is_none_or(|(g, _)| gap < g) {
                    next = Some((gap, s1));
                }
            }
            let Some((gap, _)) = next else { continue };
            if gap <= hair || gap >= min_gap {
                continue;
            }
            let slot = (end, end + u * gap);
            if out.len() < MAX_OBJECTS && !out.iter().any(|(_, g0, g1)| same_gap((*g0, *g1), slot, k)) {
                out.push((i, slot.0, slot.1));
            }
        }
    }
    out
}

/// Whether a wall other than `i` and `j` (the two pieces either side of the gap `g0`–`g1`) runs
/// across the gap: its centreline crosses the gap's line between the gap's ends, away from them by
/// more than half its thickness (a wall meeting a jamb end-on is the jamb's return, not across),
/// and goes on past the line on both sides.
fn gap_crossed(walls: &[PlanWall], i: usize, j: usize, g0: Vec2, g1: Vec2) -> bool {
    let width = g0.dist(g1);
    if !(width.is_finite() && width > 1e-9) {
        return false;
    }
    let u = (g1 - g0) * (1.0 / width);
    walls.iter().enumerate().take(MAX_OBJECTS).any(|(k, c)| {
        if k == i || k == j {
            return false;
        }
        let d = c.b - c.a;
        let den = u.cross(d);
        // Parallel to the gap: along it, not across it.
        if den.abs() <= 0.5 * d.len() {
            return false;
        }
        // Where the wall's line meets the gap's (s along the gap, t along the wall).
        let s = (c.a - g0).cross(d) / den;
        let t = (c.a - g0).cross(u) / den;
        let margin = c.thickness / 2.0;
        // Through the gap's line, past it both sides by half its thickness: a wall that only
        // ends on the line (a partition meeting an outside wall in a window's gap) runs up to the
        // opening, not across it.
        let past = margin / d.len().max(1e-9);
        s > margin && s < width - margin && t > past && t < 1.0 - past
    })
}

/// Whether a wall's side faces the outside at `p` (on its centreline, the wall `own_t` thick, `k`
/// drawing units per inch): along `nrm` or along `-nrm`, the ray meets no other wall standing
/// clear past its face, and no other wall's opening (`openings`: gaps between the pieces of one
/// wall line, their ends and thickness). A room's window or door is still the building's wall: a
/// ray out through it is inside. A wall just past this one's face (a line pair against an outside
/// wall's inside face: a unit's sleeve, a sill) is a wall beyond it: this one is inside. A wall
/// overlapping this one's thickness (the same wall read twice, one pair inside the other) is not.
fn faces_outside(p: Vec2, nrm: Vec2, own_t: f64, k: f64, walls: &[PlanWall], openings: &[(Vec2, Vec2, f64)]) -> bool {
    let face = (own_t / 2.0).max(0.0) - TOUCH_IN * k;
    let meets = |dir: Vec2, q: Vec2, r: Vec2, t_other: f64| {
        let den = dir.cross(r);
        if den.abs() < 1e-12 {
            return false;
        }
        let t = (q - p).cross(r) / den;
        let v = (q - p).cross(dir) / den;
        t - t_other.max(0.0) / 2.0 > face && (0.0..=1.0).contains(&v)
    };
    let hits = |dir: Vec2| {
        walls.iter().take(MAX_OBJECTS).any(|w| meets(dir, w.a, w.b - w.a, w.thickness))
            || openings.iter().take(MAX_OBJECTS).any(|(a, b, t)| meets(dir, *a, *b - *a, *t))
    };
    !hits(nrm) || !hits(-nrm)
}

/// Two parallel walls whose faces are this close (inches) or overlap less touch: the far one
/// stands past the near one's face.
const TOUCH_IN: f64 = 0.5;

/// Whether a wall other than `i` and `j` (the pieces either side of the gap `g0`–`g1`, `t` thick)
/// runs across the gap's line at each of its ends, on through it past both sides by more than `t`:
/// the gap is the space between two walls that cross the line (a corridor between two units'
/// lined-up demising walls, each ending at a corridor wall), not an opening in one wall. A wall
/// meeting a jamb end-on from one side (a partition at an outside wall's window) doesn't count.
fn gap_between_crossing_walls(walls: &[PlanWall], i: usize, j: usize, g0: Vec2, g1: Vec2, t: f64) -> bool {
    let width = g0.dist(g1);
    if !(width.is_finite() && width > 1e-9) {
        return false;
    }
    let u = (g1 - g0) * (1.0 / width);
    let crossed_at = |end: f64| {
        walls.iter().enumerate().take(MAX_OBJECTS).any(|(kk, c)| {
            if kk == i || kk == j {
                return false;
            }
            let d = c.b - c.a;
            let len = d.len();
            let den = u.cross(d);
            // (A zero-length or NaN wall: none.)
            if len.is_nan() || len <= 1e-9 || den.abs() <= 0.5 * len {
                return false;
            }
            // Where its line meets the gap's (s along the gap from g0, w along the wall).
            let s = (c.a - g0).cross(d) / den;
            let w = (c.a - g0).cross(u) / den;
            let reach = c.thickness / 2.0 + t.max(0.0) / 2.0;
            let beyond = t.max(0.0) / len;
            (s - end).abs() <= reach && w > beyond && w < 1.0 - beyond
        })
    };
    crossed_at(0.0) && crossed_at(width)
}

/// Whether a gap's middle `mid` is in a corridor's air: the corridor's floor on both sides of the
/// gap's line (`nrm` across it), a wall's thickness `t` off it. A gap in a wall along a corridor
/// (a cased opening into a lobby) has the corridor on one side only.
fn in_corridor_air(corridors: &[&cadaddy_walk3d::Circulation], mid: Vec2, nrm: Vec2, t: f64) -> bool {
    let off = nrm * t.max(1e-6);
    let on_floor = |p: Vec2| corridors.iter().any(|c| c.holds([p.x, p.y]));
    on_floor(mid) && on_floor(mid + off) && on_floor(mid - off)
}

/// A window narrower than this (inches) beside a pier is no window: the pier's slot, filled.
const MIN_WINDOW_IN: f64 = 12.0;

/// Glazing covers at least this share of the opening it spans.
const GLAZING_COVER: f64 = 0.75;

/// An opening this close in width (inches) to a drawn window is that window again (typical).
const TYPICAL_WIDTH_IN: f64 = 1.0;

/// Why an outside wall's gap is a window: drawn there (glazing, a tag), or the width of one drawn
/// elsewhere on the level (inferred, typical).
#[derive(Clone, Copy)]
enum WindowBy {
    Drawn,
    Typical,
}

/// Longest text read as a window mark.
const MAX_MARK_LEN: usize = 8;

/// Whether `text` is a window's mark: a window row's mark in the spec's schedule, or a window
/// prefix (rule data, `openings.window_mark_prefixes`) and a number (`W5`, `W-2`, `W3A`).
fn is_window_mark(text: &str, schedule_marks: &[String]) -> bool {
    let t = text.trim();
    if t.is_empty() || t.len() > MAX_MARK_LEN {
        return false;
    }
    if schedule_marks.iter().any(|m| m.eq_ignore_ascii_case(t)) {
        return true;
    }
    mark_with_prefix(t, "openings.window_mark_prefixes") || mark_with_prefix(t, "openings.storefront_mark_prefixes")
}

/// Where the level's texts mark a window, and the mark (see [`is_window_mark`]): its texts, and
/// the visible attributes of its tag blocks (a print's tag symbols come in as blocks whose
/// attribute holds the mark, Q6311).
fn window_marks_drawn(spec: Option<&cadaddy_walk3d::BuildingSpec>, ents: &[&Arc<Entity>]) -> Vec<(Vec2, String)> {
    let schedule_marks: Vec<String> =
        spec.iter().flat_map(|sp| sp.schedule.rows.iter().filter(|r| r.kind == cadaddy_walk3d::FamilyKind::Window).map(|r| r.mark.clone())).collect();
    let attributes = ents.iter().filter_map(|e| match &e.kind {
        EntityKind::Insert(i) => Some(i.attribs.iter().filter(|a| !a.invisible).map(|a| (a.text.justify_point(), a.text.value.trim().to_string()))),
        _ => None,
    });
    super::perceive::texts(ents)
        .into_iter()
        .map(|(_, text, at, _)| (at, text.trim().to_string()))
        .chain(attributes.flatten())
        .filter(|(_, text)| is_window_mark(text, &schedule_marks))
        .take(MAX_WINDOW_TAGS)
        .collect()
}

/// A window tag's window in an outside wall's gap: the gap (index) and the window's width
/// (drawing units; the gap's, or its type's when the gap is wider).
#[derive(Clone, Debug, PartialEq)]
struct TagFit {
    gap: usize,
    width: f64,
}

/// What the level's window tags mark ([`window_tag_fits`]): each tag's fit, and each mark's window
/// width (its schedule row's, else the width its tags' openings mostly are).
struct TagFits {
    per_tag: Vec<Option<TagFit>>,
    widths: Vec<(String, f64)>,
}

impl TagFits {
    fn width_of(&self, mark: &str) -> Option<f64> {
        self.widths.iter().find(|(m, _)| m == mark).map(|(_, w)| *w)
    }
}

/// The windows the tags mark (`k` units per inch). A tag marks the outside gap nearest it, within
/// its reach. A gap its mark's width (the schedule row's, else the width most of the mark's tags
/// sit by) is that window. A gap wider than the mark's width that no tag fits is the tag's window
/// at its type's width, centred in it (the plan's opening is wider than the window: piers and
/// framing drawn as one gap). A gap narrower than the mark is no window of that mark (a `W5` by
/// the neighbouring, narrower opening is that one's tag).
fn window_tag_fits(tags: &[WindowTag], gaps: &[(Vec2, Vec2, f64)], k: f64) -> TagFits {
    // Each tag's gap: the nearest within reach.
    let nearest: Vec<Option<usize>> = tags
        .iter()
        .map(|tag| {
            gaps.iter()
                .enumerate()
                .map(|(i, (g0, g1, _))| (i, seg_dist(tag.at, *g0, *g1)))
                .filter(|(_, d)| d.is_finite())
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .and_then(|(i, _)| {
                    let (g0, g1, _) = gaps.get(i)?;
                    (tag.at.dist((*g0 + *g1) * 0.5) <= g0.dist(*g1) / 2.0 + TAG_REACH_IN * k).then_some(i)
                })
        })
        .collect();
    let width_of = |i: usize| gaps.get(i).map_or(0.0, |(g0, g1, _)| g0.dist(*g1));
    let tol = MARK_WIDTH_TOL_IN * k;
    // Each mark's width: the row's, else the width most of its tags' gaps have (the narrower on
    // a tie).
    let mut widths: Vec<(String, f64)> = Vec::new();
    for tag in tags {
        if widths.iter().any(|(m, _)| *m == tag.mark) || widths.len() >= MAX_WINDOW_TAGS {
            continue;
        }
        let w = tag.width.or_else(|| {
            let ws: Vec<f64> = tags.iter().zip(&nearest).filter(|(o, _)| o.mark == tag.mark).filter_map(|(_, g)| g.map(width_of)).collect();
            let count = |x: f64| ws.iter().filter(|y| (*y - x).abs() <= tol).count();
            ws.iter().copied().max_by(|a, b| count(*a).cmp(&count(*b)).then(b.total_cmp(a)))
        });
        if let Some(w) = w.filter(|w| w.is_finite() && *w > 0.0) {
            widths.push((tag.mark.clone(), w));
        }
    }
    let mark_width = |m: &str| widths.iter().find(|(x, _)| x == m).map(|(_, w)| *w);
    let mut per_tag: Vec<Option<TagFit>> = vec![None; tags.len()];
    // Its mark's width: the gap is the window.
    for (n, (tag, gap)) in tags.iter().zip(&nearest).enumerate() {
        let (Some(i), Some(mw)) = (*gap, mark_width(&tag.mark)) else { continue };
        let w = width_of(i);
        if (mw - w).abs() <= tol
            && let Some(slot) = per_tag.get_mut(n)
        {
            *slot = Some(TagFit { gap: i, width: w });
        }
    }
    // A gap two marks' tags fit is the nearer tag's: the other mark's tag is another window's.
    let dist_to = |n: usize, i: usize| tags.get(n).zip(gaps.get(i)).map_or(f64::INFINITY, |(t, (g0, g1, _))| seg_dist(t.at, *g0, *g1));
    let fitted: Vec<(usize, usize)> = per_tag.iter().enumerate().filter_map(|(n, f)| f.as_ref().map(|f| (n, f.gap))).collect();
    for &(n, i) in &fitted {
        let nearer_other = fitted.iter().any(|&(m, j)| {
            j == i && m != n && tags.get(m).map(|t| &t.mark) != tags.get(n).map(|t| &t.mark) && (dist_to(m, i), m) < (dist_to(n, i), n)
        });
        if nearer_other && let Some(slot) = per_tag.get_mut(n) {
            *slot = None;
        }
    }
    // Wider than its mark, and no tag's own: the window at its type's width in the middle.
    let taken: Vec<usize> = per_tag.iter().flatten().map(|f| f.gap).collect();
    for (n, (tag, gap)) in tags.iter().zip(&nearest).enumerate() {
        let (Some(i), Some(mw)) = (*gap, mark_width(&tag.mark)) else { continue };
        if per_tag.get(n).is_some_and(Option::is_some) || taken.contains(&i) || mw < MIN_WINDOW_IN * k || width_of(i) <= mw + tol {
            continue;
        }
        if let Some(slot) = per_tag.get_mut(n) {
            *slot = Some(TagFit { gap: i, width: mw });
        }
    }
    TagFits { per_tag, widths }
}

/// Where a window tag's window goes in an outside wall ([`host_wall`]): the wall piece under the
/// tag's foot (the window's leaf takes its thickness and source), where along it the window's
/// middle is, and every piece of that wall line the window crosses with the window's span along
/// each (in the piece's own direction), and the window's middle and the line's direction. A
/// plan's wall line often comes in pieces, joined end to end, overlapping where two drawings of
/// it meet, or a slot apart narrower than any opening (a joint, a post: built solid, Q6297): the
/// window spans the joints and is cut out of every piece it crosses (Q6311).
struct WindowHost {
    wall: usize,
    at: f64,
    spans: Vec<(usize, f64, f64)>,
    mid: Vec2,
    u: Vec2,
}

/// A window's ends stand on its wall's pieces within this (inches).
const JOINT_GAP_IN: f64 = 1.0;

/// The outside wall holding a window tag's window `width` wide (drawing units, `k` per inch):
/// the nearest wall piece within `reach` past its face whose foot point under the tag is on it
/// (or in a slot at its end),
/// its line (the piece and the pieces in line with it, end to end, overlapping or a slot narrower
/// than `slot` apart) holding the window centred at the tag's foot, its ends on pieces, with no
/// opening cut there yet, outside there (`outside(point, normal, the wall's thickness)`).
#[allow(clippy::too_many_arguments)]
fn host_wall(
    walls: &[PlanWall],
    at: Vec2,
    width: f64,
    reach: f64,
    k: f64,
    slot: f64,
    outside: &dyn Fn(Vec2, Vec2, f64) -> bool,
) -> Option<WindowHost> {
    if !(width.is_finite() && width > 0.0 && at.is_finite()) {
        return None;
    }
    let half = width / 2.0;
    let joint = JOINT_GAP_IN * k;
    let slot = if slot.is_finite() { slot.max(joint) } else { joint };
    let unit = |w: &PlanWall| {
        let len = w.a.dist(w.b);
        (len.is_finite() && len > 1e-9).then(|| (len, (w.b - w.a) * (1.0 / len)))
    };
    walls
        .iter()
        .enumerate()
        .take(MAX_OBJECTS)
        .filter_map(|(i, w)| {
            let (len, u) = unit(w)?;
            let s = u.dot(at - w.a);
            let off = u.perp().dot(at - w.a).abs();
            if off > w.thickness / 2.0 + reach || s < -slot || s > len + slot {
                return None;
            }
            let mid = w.a + u * s;
            // The line's pieces: parallel, their centrelines within half the thinner wall of
            // this one's, each with its stretch along this one and the window's span along it.
            let mut along: Vec<(f64, f64)> = Vec::new();
            let mut spans: Vec<(usize, f64, f64)> = Vec::new();
            for (j, o) in walls.iter().enumerate().take(MAX_OBJECTS) {
                let Some((_, ou)) = unit(o) else { continue };
                let side = |p: Vec2| u.perp().dot(p - w.a).abs();
                if u.perp().dot(ou).abs() > 0.01 || side(o.a).max(side(o.b)) > w.thickness.min(o.thickness) / 2.0 {
                    continue;
                }
                let (p0, p1) = (u.dot(o.a - w.a), u.dot(o.b - w.a));
                let (p0, p1) = (p0.min(p1), p0.max(p1));
                if p1 < s - half - slot || p0 > s + half + slot {
                    continue;
                }
                along.push((p0, p1));
                let c = ou.dot(mid - o.a);
                if o.cuts.iter().any(|cut| cut.s0 < c + half && cut.s1 > c - half) {
                    return None;
                }
                if p1.min(s + half) - p0.max(s - half) > joint {
                    spans.push((j, c - half, c + half));
                }
            }
            // The pieces cover the window's span, joint to joint, its ends on pieces.
            along.sort_by(|a, b| a.0.total_cmp(&b.0));
            if !along.iter().any(|(p0, p1)| *p0 <= s - half + joint && *p1 > s - half) {
                return None;
            }
            let mut reached = s - half;
            for (p0, p1) in &along {
                if *p0 <= reached + slot {
                    reached = reached.max(*p1);
                }
            }
            (reached >= s + half - joint && !spans.is_empty() && outside(mid, u.perp(), w.thickness))
                .then_some((off, WindowHost { wall: i, at: s, spans, mid, u }))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, h)| h)
}

/// A window mark's head and sill above its floor (drawing units, `upf` per foot): its schedule
/// row's (and the family its type names), a storefront's sill at the floor when none is given.
fn window_heights(spec: Option<&cadaddy_walk3d::BuildingSpec>, mark: &str, upf: f64) -> (Option<f64>, Option<f64>) {
    let Some(sp) = spec else { return (None, storefront_mark(mark, None).then_some(0.0)) };
    let row = sp.schedule.row(mark).filter(|r| r.kind == cadaddy_walk3d::FamilyKind::Window);
    let (mut head, mut sill) = match row {
        Some(r) => {
            let (h, s) = r.heights(sp.openings.window_sill_ft);
            (h, if r.sill_ft.is_some() { s } else { None })
        }
        None => (None, None),
    };
    if let Some(f) = row.and_then(|r| r.kind_type.as_deref()).and_then(|ty| sp.family_by_type(cadaddy_walk3d::FamilyKind::Window, ty)) {
        head = f.head_ft.or(head);
        sill = f.sill_ft.or(sill);
    }
    if sill.is_none() && storefront_mark(mark, row) {
        sill = Some(0.0);
    }
    let ok = |v: f64| (v.is_finite() && (0.0..=MAX_OPENING_FT).contains(&v)).then_some(v * upf);
    (head.and_then(ok), sill.and_then(ok))
}

/// Highest head or sill a schedule may give an opening above its floor, feet.
const MAX_OPENING_FT: f64 = 40.0;

/// Whether a window mark is a storefront's: its schedule row's type or frame says so (rule data
/// `openings.storefront_words`), or the mark has a storefront prefix (`openings.storefront_mark_prefixes`).
pub(crate) fn storefront_mark(mark: &str, row: Option<&cadaddy_walk3d::ScheduleRow>) -> bool {
    let words = cadaddy_mepfix::wordrules::words("openings.storefront_words");
    let says = |t: &Option<String>| t.as_deref().is_some_and(|t| words.iter().any(|w| t.to_ascii_uppercase().contains(&w.to_ascii_uppercase())));
    if row.is_some_and(|r| says(&r.kind_type) || says(&r.frame)) {
        return true;
    }
    mark_with_prefix(mark, "openings.storefront_mark_prefixes")
}

/// Whether `mark` is one of the rule data's prefixes (`key`) and a number of 1–3 digits, with an
/// optional letter after it (`W5`, `W-2`, `W3A`).
fn mark_with_prefix(mark: &str, key: &str) -> bool {
    let u = mark.trim().to_ascii_uppercase();
    cadaddy_mepfix::wordrules::words(key).iter().any(|p| {
        let Some(rest) = u.strip_prefix(&p.to_ascii_uppercase()) else { return false };
        let rest = rest.strip_prefix(['-', '.']).unwrap_or(rest);
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        let tail = rest.get(digits..).unwrap_or("");
        (1..=3).contains(&digits) && (tail.is_empty() || (tail.len() == 1 && tail.chars().all(|c| c.is_ascii_alphabetic())))
    })
}

/// A level's window tags against its windows (Q6311): per mark, the tags read and the windows
/// built; the windows with no type; the windows by why they are there; the tags with no window
/// and why; the marks with no schedule row.
#[derive(Clone, Debug, Default)]
pub(crate) struct WindowTally {
    pub tagged: std::collections::BTreeMap<String, usize>,
    pub built: std::collections::BTreeMap<String, usize>,
    pub untyped: usize,
    pub by_basis: std::collections::BTreeMap<&'static str, usize>,
    pub no_host: Vec<(String, Vec2, &'static str)>,
    pub storefront: usize,
    pub unscheduled: Vec<String>,
}

impl WindowTally {
    fn of(tags: &[WindowTag], leaves: &[Leaf], no_host: Vec<(String, Vec2, &'static str)>, spec: Option<&cadaddy_walk3d::BuildingSpec>) -> Self {
        let mut t = Self { no_host, ..Self::default() };
        for tag in tags {
            *t.tagged.entry(tag.mark.clone()).or_default() += 1;
        }
        for l in leaves.iter().filter(|l| l.window) {
            match &l.mark {
                Some(m) => *t.built.entry(m.clone()).or_default() += 1,
                None => t.untyped += 1,
            }
            if let Some(b) = l.basis {
                *t.by_basis.entry(b.as_str()).or_default() += 1;
            }
            let row = l.mark.as_deref().and_then(|m| spec.and_then(|sp| sp.schedule.row(m)));
            if l.mark.as_deref().is_some_and(|m| storefront_mark(m, row)) {
                t.storefront += 1;
            }
        }
        t.unscheduled = t
            .tagged
            .keys()
            .chain(t.built.keys())
            .filter(|m| !spec.is_some_and(|sp| sp.schedule.row(m).is_some_and(|r| r.kind == cadaddy_walk3d::FamilyKind::Window)))
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        t
    }

    /// The report row for `level` (positions in sheet units).
    pub(crate) fn to_json(&self, level: &str) -> Value {
        json!({
            "level": level,
            "tagged": self.tagged,
            "built": self.built,
            "untyped": self.untyped,
            "by_basis": self.by_basis,
            "storefront": self.storefront,
            "no_host": self.no_host.iter().take(MAX_LISTED_TAGS).map(|(m, at, why)| json!({"mark": m, "at": [at.x, at.y], "why": why})).collect::<Vec<_>>(),
            "no_host_count": self.no_host.len(),
            "marks_without_schedule_row": self.unscheduled,
        })
    }
}

/// Most tags with no window listed in a level's report row.
const MAX_LISTED_TAGS: usize = 200;

/// Most window tags read on a level (each is weighed against the others with its mark).
const MAX_WINDOW_TAGS: usize = 2_000;

/// A window tag marks an opening up to this far (inches) past its middle's half width.
const TAG_REACH_IN: f64 = 48.0;

/// A tag's opening is within this (inches) of its mark's width.
const MARK_WIDTH_TOL_IN: f64 = 6.0;

/// A window mark drawn on a plan: where, its mark, and its schedule row's width (drawing units)
/// when the schedule gives one.
struct WindowTag {
    at: Vec2,
    mark: String,
    width: Option<f64>,
}

/// Whether two gaps are one (the same ends, either way round).
fn same_gap(a: (Vec2, Vec2), b: (Vec2, Vec2), k: f64) -> bool {
    let tol = 0.01 * k;
    (a.0.dist(b.0) <= tol && a.1.dist(b.1) <= tol) || (a.0.dist(b.1) <= tol && a.1.dist(b.0) <= tol)
}

/// Whether the plan draws glazing in the opening `a`–`b` of a wall `t` thick (`k` units per inch):
/// a line across it, in line with the wall, between its faces, spanning most of the opening and
/// stopping at its jambs. A bare gap has none (Q6293).
fn glazed(segs: &[super::perceive::geometry::Seg], a: Vec2, b: Vec2, t: f64, k: f64) -> bool {
    let len = a.dist(b);
    if !(len.is_finite() && len > 1e-9) {
        return false;
    }
    let u = (b - a) * (1.0 / len);
    let n = u.perp();
    // Strictly between the wall's faces (a face line running through is the wall, not glass).
    let band = (t / 2.0 - 0.25 * k).max(0.0);
    let reach = t + 2.0 * k;
    segs.iter().take(MAX_OBJECTS).any(|sg| {
        let d = sg.b - sg.a;
        let l = d.len();
        if !(l.is_finite() && l >= GLAZING_COVER * len) || u.cross(d).abs() > 0.02 * l {
            return false;
        }
        if n.dot(sg.a - a).abs() >= band || n.dot(sg.b - a).abs() >= band {
            return false;
        }
        let (p0, p1) = (u.dot(sg.a - a).min(u.dot(sg.b - a)), u.dot(sg.a - a).max(u.dot(sg.b - a)));
        p0 >= -reach && p1 <= len + reach && p1.min(len) - p0.max(0.0) >= GLAZING_COVER * len
    })
}

/// A door swing drawn on a plan: its hinge (the centre), radius, two ends and source.
struct Swing {
    c: Vec2,
    r: f64,
    ends: [Vec2; 2],
    handle: Handle,
}

/// The door swings among `ents`: arcs and open polylines (a swing flattened, as prints arrive)
/// on one circle through 60–100°, a door's radius (16"–54" at `k` units per inch).
fn drawn_swings(ents: &[&Arc<Entity>], k: f64) -> Vec<Swing> {
    let (rmin, rmax) = (16.0 * k, 54.0 * k);
    let mut out = Vec::new();
    for e in ents.iter().take(MAX_OBJECTS) {
        let swing = match &e.kind {
            EntityKind::Arc(a) => {
                let mut sweep = (a.end - a.start).rem_euclid(std::f64::consts::TAU);
                if sweep < 1e-9 {
                    sweep = std::f64::consts::TAU;
                }
                let c = a.center.xy();
                (60.0..=100.0)
                    .contains(&sweep.to_degrees())
                    .then(|| (c, a.radius, [c + Vec2::from_angle(a.start) * a.radius, c + Vec2::from_angle(a.end) * a.radius]))
            }
            EntityKind::LwPolyline(p) if !p.closed && (3..=200).contains(&p.vertices.len()) => {
                let pts = cadaddy_geom::Polyline { vertices: p.vertices.clone(), closed: false }.tessellate(1e-3);
                super::perceive::geometry::flat_swing(&pts).map(|(c, r, _, ends)| (c, r, ends))
            }
            _ => None,
        };
        if let Some((c, r, ends)) = swing.filter(|(c, r, _)| c.is_finite() && (rmin..=rmax).contains(r)) {
            out.push(Swing { c, r, ends, handle: e.handle });
        }
    }
    out
}

/// The hinge points of the door swings among `ents` (`k` units per inch).
pub(crate) fn swing_hinges(ents: &[&Arc<Entity>], k: f64) -> Vec<Vec2> {
    drawn_swings(ents, k).into_iter().map(|s| s.c).collect()
}

/// Walls looked at for the building's outline, at most (a level; hostile drawings).
const MAX_CLUSTERED: usize = 20_000;

/// Whether a pair (centreline `a`–`b`, `t` thick) runs in line with one of the walls and meets
/// its end (within its thickness and 3"): the rest of that wall across a gap, not a thing beside it.
fn fills_a_wall(a: Vec2, b: Vec2, t: f64, walls: &[super::perceive::geometry::Pair], k: f64) -> bool {
    let len = a.dist(b);
    if len.is_nan() || len <= 1e-9 {
        return false;
    }
    let u = (b - a) * (1.0 / len);
    walls.iter().any(|w| {
        let wl = w.c0.dist(w.c1);
        if wl.is_nan() || wl <= 1e-9 || (w.c1 - w.c0).cross(u).abs() > 0.05 * wl {
            return false;
        }
        let off = (w.c0 - a).cross(u).abs();
        let reach = t.max(w.thickness) + 3.0 * k;
        off <= 0.5 * t.max(w.thickness) && [a, b].iter().any(|p| p.dist(w.c0).min(p.dist(w.c1)) <= reach)
    })
}

/// The share of a plan's lines (on its wall layers) drawn solid for its screened lines to be left
/// out of its walls: less, the plan is drawn in grey throughout (an underlay, a faded set).
const SOLID_SHARE: f64 = 0.2;

/// Whether a colour is a light screen: a grey nearer white than black (true colour, or ACI 9,
/// 253, 254), which a print draws what isn't built in. A mid grey (ACI 8) can be built work drawn
/// lighter (the windows in a wall).
fn screened(c: cadaddy_color::Color) -> bool {
    match c {
        cadaddy_color::Color::True(cadaddy_color::Rgb(r, g, b)) => {
            r.abs_diff(g) <= 8 && g.abs_diff(b) <= 8 && r.abs_diff(b) <= 8 && (150..=240).contains(&r)
        }
        cadaddy_color::Color::Index(i) => matches!(i, 9 | 253 | 254),
        _ => false,
    }
}

/// The root of `i` in a union-find forest (halving the path on the way).
fn root(parent: &mut [usize], mut i: usize) -> usize {
    for _ in 0..parent.len() {
        let Some(&p) = parent.get(i) else { break };
        if p == i {
            break;
        }
        let gp = parent.get(p).copied().unwrap_or(p);
        if let Some(x) = parent.get_mut(i) {
            *x = gp;
        }
        i = gp;
    }
    i
}

/// Two dimension strings side by side stand at least this far apart, inches.
const DIMENSION_STRINGS_APART_IN: f64 = 10.0;

/// A text that is a figure in feet (`12'-6"`, `38'-8 3/8"`, `3'`): a dimension's.
fn is_dimension_figure(e: &Entity) -> bool {
    let value = match &e.kind {
        EntityKind::Text(t) => t.value.as_str(),
        EntityKind::MText(t) => t.contents.as_str(),
        _ => return false,
    };
    let c: Vec<char> = value.trim().chars().collect();
    c.len() <= 16 && c.windows(2).any(|w| matches!(w, [d, '\''] if d.is_ascii_digit())) && !c.iter().any(char::is_ascii_lowercase)
}

/// Line pairs that are no wall: a pair with a dimension's figure lettered between its lines is two
/// dimension strings side by side, and a pair across one of those is its extension lines (a wall
/// stops at the building's face; a dimension string runs outside it). How many were dropped.
fn drop_dimension_strings(found: &mut Vec<super::perceive::geometry::Pair>, figures: &[Vec2], k: f64) -> usize {
    if figures.is_empty() {
        return 0;
    }
    let between = |p: &super::perceive::geometry::Pair, at: Vec2| {
        let len = p.c0.dist(p.c1);
        if len.is_nan() || len <= 0.0 {
            return false;
        }
        let u = (p.c1 - p.c0) * (1.0 / len);
        let along = (at - p.c0).dot(u);
        along >= 0.0 && along <= len && (at - p.c0).cross(u).abs() <= p.thickness / 2.0
    };
    // Two strings stand at least this far apart: closer, a figure between the lines is one
    // lettered across a wall.
    let is_string = |p: &super::perceive::geometry::Pair| p.thickness >= DIMENSION_STRINGS_APART_IN * k && figures.iter().any(|f| between(p, *f));
    let strings: Vec<(Vec2, Vec2, f64)> = found.iter().filter(|p| is_string(p)).map(|p| (p.c0, p.c1, p.thickness)).collect();
    if strings.is_empty() {
        return 0;
    }
    let before = found.len();
    let side = |p: Vec2, q: Vec2, r: Vec2| (q - p).cross(r - p);
    found.retain(|p| {
        !is_string(p)
            && !strings.iter().any(|&(a, b, t)| {
                // Reaching the string: an end within its width, or crossing it.
                let crosses = side(a, b, p.c0) * side(a, b, p.c1) < 0.0 && side(p.c0, p.c1, a) * side(p.c0, p.c1, b) < 0.0;
                crosses || seg_dist(p.c0, a, b) <= t / 2.0 || seg_dist(p.c1, a, b) <= t / 2.0
            })
    });
    before - found.len()
}

/// Keep the building on a plan drawn in `region` (sheet units, `upf` per foot). The building is
/// the walls that touch one another (within 4'), in clusters of at least a tenth of the largest's
/// length, found among the walls shorter than 40% of the region (a grid line or the border runs
/// the sheet's length). Every wall reaching more than 2' (and its thickness) out of those
/// clusters' extents goes: grid lines out to their bubbles, dimension strings and match lines
/// beside the building, the border and the title block. A cluster drawn at a larger scale (its
/// walls [`ENLARGED_THICKER`] the largest cluster's or more) is an enlarged view and goes too.
fn keep_building(walls: &mut Vec<PlanWall>, region: [f64; 4], upf: f64) {
    let span = (region[2] - region[0]).abs().max((region[3] - region[1]).abs());
    let link = 4.0 * upf;
    if !(span.is_finite() && span > 0.0 && link.is_finite() && link > 0.0) || walls.len() < 4 || walls.len() > MAX_CLUSTERED {
        return;
    }
    let short: Vec<usize> = (0..walls.len()).filter(|&i| walls.get(i).is_some_and(|w| w.a.dist(w.b) < 0.4 * span)).collect();
    if short.len() < 4 {
        return;
    }
    // Cells the link wide; each wall sits in every cell its extents (grown by the link) touch,
    // so two walls within the link share a cell.
    let cell = |v: f64| (v / link).floor() as i64;
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (k, &i) in short.iter().enumerate() {
        let Some(w) = walls.get(i) else { continue };
        let (x0, x1) = (cell(w.a.x.min(w.b.x) - link), cell(w.a.x.max(w.b.x) + link));
        let (y0, y1) = (cell(w.a.y.min(w.b.y) - link), cell(w.a.y.max(w.b.y) + link));
        if x1.saturating_sub(x0).saturating_mul(y1.saturating_sub(y0)) > 10_000 {
            continue;
        }
        for x in x0..=x1 {
            for y in y0..=y1 {
                grid.entry((x, y)).or_default().push(k);
            }
        }
    }
    let side = |p: Vec2, q: Vec2, r: Vec2| (q - p).cross(r - p);
    let gap = |a: &PlanWall, b: &PlanWall| {
        let crossing = side(b.a, b.b, a.a) * side(b.a, b.b, a.b) < 0.0 && side(a.a, a.b, b.a) * side(a.a, a.b, b.b) < 0.0;
        if crossing { 0.0 } else { seg_dist(a.a, b.a, b.b).min(seg_dist(a.b, b.a, b.b)).min(seg_dist(b.a, a.a, a.b)).min(seg_dist(b.b, a.a, a.b)) }
    };
    let mut parent: Vec<usize> = (0..short.len()).collect();
    let mut compared = 0usize;
    for members in grid.values() {
        for (n, &ka) in members.iter().enumerate() {
            for &kb in members.iter().skip(n + 1) {
                compared += 1;
                if compared > 20_000_000 {
                    return;
                }
                let (Some(a), Some(b)) = (short.get(ka).and_then(|&i| walls.get(i)), short.get(kb).and_then(|&i| walls.get(i))) else { continue };
                if gap(a, b) <= link {
                    let (ra, rb) = (root(&mut parent, ka), root(&mut parent, kb));
                    if ra != rb
                        && let Some(x) = parent.get_mut(ra)
                    {
                        *x = rb;
                    }
                }
            }
        }
    }
    let mut length: HashMap<usize, f64> = HashMap::new();
    let roots: Vec<usize> = (0..short.len()).map(|k| root(&mut parent, k)).collect();
    for (k, &i) in short.iter().enumerate() {
        let r = roots.get(k).copied().unwrap_or(k);
        *length.entry(r).or_insert(0.0) += walls.get(i).map_or(0.0, |w| w.a.dist(w.b));
    }
    let largest = length.values().copied().fold(0.0, f64::max);
    if largest <= 0.0 {
        return;
    }
    // A view drawn at a larger scale beside the plan (an enlarged unit, stair or toilet plan, a
    // detail) is no more building: its walls read thicker than the plan's own (Q5273), at
    // [`ENLARGED_THICKER`] the plan's typical wall or more. The plan is the cluster that covers
    // the most building: the longest in walls of its own thickness (an enlarged view is long on
    // the sheet only for its scale).
    let mut thick: HashMap<usize, Vec<f64>> = HashMap::new();
    for (k, &i) in short.iter().enumerate() {
        if let (Some(&r), Some(w)) = (roots.get(k), walls.get(i))
            && w.thickness.is_finite()
        {
            thick.entry(r).or_default().push(w.thickness);
        }
    }
    let median = |r: usize| {
        let mut v = thick.get(&r).cloned().unwrap_or_default();
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied()
    };
    let main =
        length.iter().filter_map(|(r, l)| median(*r).filter(|t| *t > 0.0).map(|t| (*r, l / t))).max_by(|a, b| a.1.total_cmp(&b.1)).map(|(r, _)| r);
    let main_t = main.and_then(median).filter(|t| *t > 0.0);
    let enlarged = |r: usize| Some(r) != main && main_t.zip(median(r)).is_some_and(|(m, t)| t >= ENLARGED_THICKER * m);
    let mut dropped = vec![false; walls.len()];
    let mut b = cadaddy_geom::Bounds2::EMPTY;
    for (k, &i) in short.iter().enumerate() {
        let r = roots.get(k).copied().unwrap_or(k);
        if length.get(&r).copied().unwrap_or(0.0) < 0.1 * largest {
            continue;
        }
        if enlarged(r) {
            if let Some(x) = dropped.get_mut(i) {
                *x = true;
            }
        } else if let Some(w) = walls.get(i) {
            b = b.union(&cadaddy_geom::Bounds2::from_points([w.a, w.b]));
        }
    }
    if b.is_empty() {
        return;
    }
    let inside = |p: Vec2, m: f64| p.x >= b.min.x - m && p.x <= b.max.x + m && p.y >= b.min.y - m && p.y <= b.max.y + m;
    let mut n = 0;
    walls.retain(|w| {
        let gone = dropped.get(n).copied().unwrap_or(false);
        n += 1;
        let m = 2.0 * upf + w.thickness;
        !gone && inside(w.a, m) && inside(w.b, m)
    });
}

/// How much thicker than the plan's typical wall a separate cluster's walls read when it is a
/// view at a larger scale (an enlarged plan at 1/4" beside a 1/8" plan reads twice as thick).
const ENLARGED_THICKER: f64 = 1.8;

/// Drawn swings in an opening at least this many times those on a solid wall: the plan breaks
/// its walls for its doors.
const BROKEN_WALL_SHARE: usize = 2;

/// Whether a door swing hinged at `at` (radius `width`, arc ends `ends`) shuts along the wall
/// `w`: one end of its arc lies on the wall's line (within its thickness and 2"), most of a leaf
/// away from the hinge along it. An arc that touches a wall at its centre but shuts across or
/// off it (a fixture, a piece of equipment, furniture) is no door in that wall.
fn swing_shuts_along(w: &PlanWall, at: Vec2, ends: [Vec2; 2], width: f64, k: f64) -> bool {
    let len = w.a.dist(w.b);
    if !(len > 1e-9 && width.is_finite() && width > 0.0) {
        return false;
    }
    let u = (w.b - w.a) * (1.0 / len);
    let line_tol = w.thickness.max(2.0 * k) + 2.0 * k;
    ends.iter().any(|e| u.perp().dot(*e - w.a).abs() <= line_tol && u.dot(*e - at).abs() >= 0.7 * width)
}

/// Whether the wall line of `walls[wi]` runs solid through the opening a swing hinged at `at`
/// shuts across: one piece of that line (parallel, on it within half the thicker one's
/// thickness) reaches 6" past both the hinge and the shut end. A door is drawn where the wall
/// stops; an arc drawn on a solid wall (a parapet's equipment, a fixture against a wall) is not
/// one. A piece only across the opening (a header drawn in plan) is no solid wall.
fn solid_through_grid(walls: &[PlanWall], grid: Option<&WallGrid>, wi: usize, at: Vec2, ends: [Vec2; 2], k: f64) -> bool {
    let Some(w) = walls.get(wi) else { return false };
    let len = w.a.dist(w.b);
    if len <= 1e-9 {
        return false;
    }
    let u = (w.b - w.a) * (1.0 / len);
    // The shut end: the arc's end farthest along the wall from the hinge.
    let shut = if u.dot(ends[0] - at).abs() >= u.dot(ends[1] - at).abs() { ends[0] } else { ends[1] };
    let (s0, s1) = (u.dot(at - w.a).min(u.dot(shut - w.a)), u.dot(at - w.a).max(u.dot(shut - w.a)));
    let past = 6.0 * k;
    let check = |c: &PlanWall| -> bool {
        let lc = c.a.dist(c.b);
        if lc <= 1e-9 || u.cross((c.b - c.a) * (1.0 / lc)).abs() > 0.01 || u.perp().dot(c.a - w.a).abs() > w.thickness.max(c.thickness) / 2.0 {
            return false;
        }
        let (c0, c1) = (u.dot(c.a - w.a).min(u.dot(c.b - w.a)), u.dot(c.a - w.a).max(u.dot(c.b - w.a)));
        c0 <= s0 - past && c1 >= s1 + past
    };
    if let Some(g) = grid { g.at(at).iter().any(|&i| check(&walls[i])) } else { walls.iter().any(check) }
}

/// One door per swing: a swing drawn twice (an arc and its chords, a path stroked twice on the
/// print), or found both from its swing and at a gap, is one door; the same for window panes.
/// Leaves are the same when their ends lie within `tol` of each other's. The doors and windows
/// left, when any were dropped.
fn one_per_swing(leaves: &mut Vec<Leaf>, tol: f64) -> Option<(usize, usize)> {
    if !(tol.is_finite() && tol > 0.0) {
        return None;
    }
    let cell = |p: Vec2| ((p.x / tol).floor() as i64, (p.y / tol).floor() as i64);
    let mut seen: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let mut kept: Vec<Leaf> = Vec::with_capacity(leaves.len());
    let mut dropped = false;
    for l in leaves.drain(..) {
        let same =
            |o: &Leaf| o.window == l.window && ((o.a.dist(l.a) <= tol && o.b.dist(l.b) <= tol) || (o.a.dist(l.b) <= tol && o.b.dist(l.a) <= tol));
        let (cx, cy) = cell(l.a);
        let dup =
            (-1..=1).any(|dx| (-1..=1).any(|dy| seen.get(&(cx + dx, cy + dy)).is_some_and(|ix| ix.iter().any(|&i| kept.get(i).is_some_and(same)))));
        if dup {
            dropped = true;
            continue;
        }
        seen.entry(cell(l.a)).or_default().push(kept.len());
        seen.entry(cell(l.b)).or_default().push(kept.len());
        kept.push(l);
    }
    *leaves = kept;
    dropped.then(|| (leaves.iter().filter(|l| !l.window).count(), leaves.iter().filter(|l| l.window).count()))
}

/// Door leaves looked at for openings shared by two leaves, at most (a level; hostile drawings).
const MAX_PAIRED: usize = 5_000;

/// One door per opening. Two swings on the same side of one wall line are one door: a pair
/// meeting in the middle of the opening (a double door) becomes one door with two leaves, and a
/// swing overlapping another's span (the same leaf read again from another hinge) goes. Swings to
/// either side of a wall (back-to-back connecting doors) stay doors of their own. `tol`: how near
/// a pair's closed ends meet. How many leaves were folded into another or dropped.
fn one_door_per_opening(leaves: &mut Vec<Leaf>, tol: f64) -> usize {
    if !(tol.is_finite() && tol > 0.0) || leaves.len() > MAX_PAIRED {
        return 0;
    }
    // A swing: its closing direction, opening direction and width.
    let swing = |l: &Leaf| -> Option<(Vec2, Vec2, f64)> {
        if l.window || l.pair.is_some() {
            return None;
        }
        let along = l.along?;
        let w = l.a.dist(l.b);
        if !(w.is_finite() && w > tol) {
            return None;
        }
        let v = (l.b - l.a) * (1.0 / w);
        (v.dot(along).abs() < 0.5).then_some((along, v, w))
    };
    let mut gone = vec![false; leaves.len()];
    let mut folded = 0;
    for i in 0..leaves.len() {
        if gone.get(i).copied().unwrap_or(true) {
            continue;
        }
        for j in i + 1..leaves.len() {
            let (Some(li), Some(lj)) = (leaves.get(i), leaves.get(j)) else { continue };
            let (Some((ai, vi, wi)), Some((aj, vj, wj))) = (swing(li), swing(lj)) else {
                if swing(li).is_none() {
                    break;
                }
                continue;
            };
            if gone.get(j).copied().unwrap_or(true)
                || ai.cross(aj).abs() > 0.1
                || vi.dot(vj) <= 0.5
                || ai.perp().dot(lj.a - li.a).abs() > li.thickness.max(lj.thickness) + tol
            {
                continue;
            }
            let (ci, cj) = (li.a + ai * wi, lj.a + aj * wj);
            let (p, q) = (ai.dot(lj.a - li.a), ai.dot(cj - li.a));
            let overlap = wi.min(p.max(q)) - p.min(q).max(0.0);
            let pair = (lj.a, lj.b);
            // A double door's frame spans both leaves' openings: the two ends farthest apart.
            let frame = match (li.frame, lj.frame) {
                (Some((a0, a1)), Some((b0, b1))) => {
                    let ends = [a0, a1, b0, b1];
                    let mut best = (a0, a1);
                    for (n, &p0) in ends.iter().enumerate() {
                        for &p1 in ends.iter().skip(n + 1) {
                            if p0.dist(p1) > best.0.dist(best.1) {
                                best = (p0, p1);
                            }
                        }
                    }
                    Some(best)
                }
                (f, g) => f.or(g),
            };
            if overlap > 0.5 * wi.min(wj) {
                // The same leaf again.
            } else if ai.dot(aj) < -0.9 && ci.dist(cj) <= tol {
                if let Some(l) = leaves.get_mut(i) {
                    l.pair = Some(pair);
                    l.frame = frame;
                }
            } else {
                continue;
            }
            if let Some(g) = gone.get_mut(j) {
                *g = true;
            }
            folded += 1;
        }
    }
    let mut k = 0;
    leaves.retain(|_| {
        let keep = !gone.get(k).copied().unwrap_or(false);
        k += 1;
        keep
    });
    folded
}

#[cfg(test)]
mod tests {
    fn line_pair(a: [f64; 2], b: [f64; 2], thickness: f64) -> crate::cmd::perceive::geometry::Pair {
        let (c0, c1) = (Vec2::new(a[0], a[1]), Vec2::new(b[0], b[1]));
        crate::cmd::perceive::geometry::Pair { layer: "0".into(), c0, c1, thickness, length: c0.dist(c1), confidence: 1.0, faces: [0, 1] }
    }

    #[test]
    fn dimension_strings_and_their_extension_lines_are_no_walls() {
        let mut found = vec![
            // Two strings 15 apart above a building, a figure between them.
            line_pair([0.0, 500.0], [400.0, 500.0], 15.0),
            // An extension line pair from the building's corner across them.
            line_pair([40.0, 400.0], [40.0, 520.0], 8.0),
            // The building's walls: the top one is 100 below the string.
            line_pair([0.0, 400.0], [400.0, 400.0], 8.0),
            line_pair([0.0, 0.0], [0.0, 400.0], 8.0),
        ];
        let dropped = drop_dimension_strings(&mut found, &[Vec2::new(200.0, 503.0)], 1.0);
        assert_eq!(dropped, 2);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|p| p.thickness == 8.0 && p.c1.y <= 400.0), "the two walls stay");
    }

    #[test]
    fn a_figure_outside_a_walls_faces_leaves_the_wall() {
        // A room's dimension lettered beside a wall, not between its faces.
        let mut found = vec![line_pair([0.0, 0.0], [400.0, 0.0], 8.0)];
        assert_eq!(drop_dimension_strings(&mut found, &[Vec2::new(200.0, 12.0)], 1.0), 0);
        assert_eq!(found.len(), 1);
        // A figure lettered across a wall, between its faces, leaves it too: strings stand farther apart.
        assert_eq!(drop_dimension_strings(&mut found, &[Vec2::new(200.0, 1.0)], 1.0), 0);
        assert_eq!(found.len(), 1);
    }

    use super::*;
    use cadaddy_walk3d::CirculationKind::{Corridor, Hoistway, Stair};

    /// The walls planned on a 20' × 10' room of 6" walls (solid lines, or `all_grey`) with a bed's
    /// headboard drawn in it as two screened lines 3" apart.
    fn planned_with_a_grey_headboard(all_grey: bool) -> usize {
        let mut d = Drawing::new_imperial();
        let grey = cadaddy_color::Color::True(cadaddy_color::Rgb(170, 170, 170));
        let mut add = |a: (f64, f64), b: (f64, f64), screened: bool| {
            let h = Handle(d.handseed);
            d.handseed = h.0 + 1;
            let line = cadaddy_doc::Line { a: Vec3::new(a.0 * 12.0, a.1 * 12.0, 0.0), b: Vec3::new(b.0 * 12.0, b.1 * 12.0, 0.0) };
            let mut e = Entity::new(h, EntityKind::Line(line));
            e.common.layer = "A-WALL".into();
            if screened || all_grey {
                e.common.color = grey;
            }
            d.model.push(e);
        };
        for (a, b) in [((0.0, 0.0), (20.0, 0.0)), ((20.0, 0.0), (20.0, 10.0)), ((20.0, 10.0), (0.0, 10.0)), ((0.0, 10.0), (0.0, 0.0))] {
            add(a, b, false);
        }
        for (a, b) in [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))] {
            add(a, b, false);
        }
        add((5.0, 9.0), (10.0, 9.0), true);
        add((5.0, 8.75), (10.0, 8.75), true);
        let rules: Rules = serde_json::from_value(json!({
            "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "kind": "wall"}]
        }))
        .unwrap();
        let level = rules.levels[0].clone();
        let g = WallGen { rules, auto: false, slab_ft: 0.0, layers: None, spec: None };
        plan_level(&d, &g, &level, 12.0, &mut Vec::new()).walls.len()
    }

    #[test]
    fn grey_glazing_in_line_with_a_wall_fills_its_gap_but_a_grey_headboard_is_no_wall() {
        let mut d = Drawing::new_imperial();
        let grey = cadaddy_color::Color::True(cadaddy_color::Rgb(170, 170, 170));
        let mut add = |a: (f64, f64), b: (f64, f64), screened: bool| {
            let h = Handle(d.handseed);
            d.handseed = h.0 + 1;
            let line = cadaddy_doc::Line { a: Vec3::new(a.0 * 12.0, a.1 * 12.0, 0.0), b: Vec3::new(b.0 * 12.0, b.1 * 12.0, 0.0) };
            let mut e = Entity::new(h, EntityKind::Line(line));
            e.common.layer = "A-WALL".into();
            if screened {
                e.common.color = grey;
            }
            d.model.push(e);
        };
        // The room's south wall broken for a 4' window (8'–12'), its glazing two grey lines in line.
        for (a, b) in [
            ((0.0, 0.0), (8.0, 0.0)),
            ((12.0, 0.0), (20.0, 0.0)),
            ((20.0, 0.0), (20.0, 10.0)),
            ((20.0, 10.0), (0.0, 10.0)),
            ((0.0, 10.0), (0.0, 0.0)),
        ] {
            add(a, b, false);
        }
        for (a, b) in
            [((0.5, 0.5), (8.0, 0.5)), ((12.0, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))]
        {
            add(a, b, false);
        }
        add((8.0, 0.1), (12.0, 0.1), true);
        add((8.0, 0.4), (12.0, 0.4), true);
        add((5.0, 9.0), (10.0, 9.0), true);
        add((5.0, 8.75), (10.0, 8.75), true);
        let rules: Rules = serde_json::from_value(json!({
            "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "kind": "wall"}]
        }))
        .unwrap();
        let level = rules.levels[0].clone();
        let g = WallGen { rules, auto: false, slab_ft: 0.0, layers: None, spec: None };
        let walls = plan_level(&d, &g, &level, 12.0, &mut Vec::new()).walls;
        // The two pieces of the south wall, the glazing between them, and the other three walls.
        assert_eq!(walls.len(), 6, "{:?}", walls.iter().map(|w| (w.a / 12.0, w.b / 12.0)).collect::<Vec<_>>());
        assert!(walls.iter().all(|w| w.a.y.min(w.b.y) / 12.0 < 0.5 || w.a.y.max(w.b.y) / 12.0 > 9.0 || (w.a.x - w.b.x).abs() < 1.0));
    }

    #[test]
    fn a_screened_grey_pair_is_furniture_not_a_wall_unless_the_plan_is_all_grey() {
        assert_eq!(planned_with_a_grey_headboard(false), 4);
        // An underlay drawn in grey throughout: every pair is read as before (the headboard too).
        assert!(planned_with_a_grey_headboard(true) > 4);
        assert!(screened(cadaddy_color::Color::True(cadaddy_color::Rgb(170, 170, 170))) && screened(cadaddy_color::Color::Index(9)));
        // A mid grey (a window drawn lighter in its wall) is not.
        assert!(!screened(cadaddy_color::Color::True(cadaddy_color::Rgb(128, 128, 128))) && !screened(cadaddy_color::Color::Index(8)));
        assert!(!screened(cadaddy_color::Color::Index(7)) && !screened(cadaddy_color::Color::True(cadaddy_color::Rgb(0, 0, 0))));
        assert!(!screened(cadaddy_color::Color::True(cadaddy_color::Rgb(255, 0, 0))) && !screened(cadaddy_color::Color::ByLayer));
    }

    #[test]
    fn a_label_names_a_corridor_a_stair_or_an_elevator_and_a_note_does_not() {
        for (text, kind) in [("CORRIDOR", Corridor), ("Corr.", Corridor), ("HALLWAY", Corridor), ("STAIR 01", Stair), ("STAIR #2", Stair)] {
            assert_eq!(circulation_label(text), Some(kind), "{text}");
        }
        for (text, kind) in [("STAIRS", Stair), ("ELEV.", Hoistway), ("ELEVATOR 1", Hoistway), ("Elev. B", Hoistway)] {
            assert_eq!(circulation_label(text), Some(kind), "{text}");
        }
        for text in ["ELEV. 100'-0\"", "STAIR RAIL", "ELEVATION", "SEE STAIR DETAILS", "HALL OF FAME", ""] {
            assert_eq!(circulation_label(text), None, "{text}");
        }
    }

    #[test]
    fn a_stair_or_hoistway_on_one_storey_stands_on_the_others_and_a_corridor_does_not() {
        let r = |x0: f64, y0: f64, x1: f64, y1: f64| [Vec2::new(x0, y0), Vec2::new(x1, y1)];
        // Three plans side by side: two of the same building 1000 apart, one smaller.
        let (a, b, c) = (vec![r(0.0, 0.0, 600.0, 300.0)], vec![r(1000.0, 0.0, 1600.0, 300.0)], vec![r(2000.0, 0.0, 2300.0, 300.0)]);
        let found = vec![
            vec![
                (Hoistway, "ELEV.".to_string(), Some(vec![r(100.0, 100.0, 200.0, 180.0)])),
                // An L-shaped stair: its larger rectangle's middle.
                (Stair, "STAIR 1".to_string(), Some(vec![r(400.0, 0.0, 420.0, 20.0), r(400.0, 20.0, 500.0, 200.0)])),
                (Corridor, "CORRIDOR".to_string(), Some(vec![r(0.0, 140.0, 600.0, 160.0)])),
                (Stair, "STAIR 2".to_string(), None),
            ],
            vec![],
            vec![],
        ];
        let seeds = stacked_seeds(&[&a, &b, &c], &found, 12.0);
        assert!(seeds[0].is_empty() && seeds[2].is_empty(), "{seeds:?}");
        let want = [(Hoistway, "ELEV.", Vec2::new(1150.0, 140.0)), (Stair, "STAIR 1", Vec2::new(1450.0, 110.0))];
        assert_eq!(seeds[1].len(), want.len(), "{seeds:?}");
        for ((k, n, at), (wk, wn, wat)) in seeds[1].iter().zip(want) {
            assert!(*k == wk && n == wn && at.dist(wat) < 1e-9, "{seeds:?}");
        }
    }

    fn plan_wall(a: (f64, f64), b: (f64, f64)) -> PlanWall {
        PlanWall {
            a: Vec2::new(a.0, a.1),
            b: Vec2::new(b.0, b.1),
            thickness: 6.0,
            top: 108.0,
            source: Handle(0),
            cuts: Vec::new(),
            extra: Vec::new(),
            wall_type: None,
        }
    }

    #[test]
    fn wall_pieces_in_line_of_one_thickness_are_one_wall() {
        let piece = |a: (f64, f64), b: (f64, f64), t: f64| PlanWall { thickness: t, ..plan_wall(a, b) };
        let mut walls = vec![
            // A wall in four pieces: touching, a ⅜" joint apart, overlapping, one drawn backwards.
            piece((0.0, 0.0), (16.0, 0.0), 7.6),
            piece((16.0, 0.0), (32.0, 0.0), 7.6),
            piece((32.375, 0.1), (48.0, 0.1), 7.6),
            piece((70.0, 0.0), (40.0, 0.0), 7.6),
            // In line past a door's 36" gap: a wall of its own.
            piece((106.0, 0.0), (200.0, 0.0), 7.6),
            // A thinner wall touching its end, and a 2" furring a wall's width off its line.
            piece((200.0, 0.0), (260.0, 0.0), 3.6),
            piece((0.0, 8.0), (70.0, 8.0), 2.0),
            // A wall across them.
            piece((100.0, -50.0), (100.0, 50.0), 7.6),
        ];
        assert_eq!(join_wall_pieces(&mut walls, &[], 1.0), 3);
        assert_eq!(walls.len(), 5);
        let w = &walls[0];
        assert!(w.a.dist(Vec2::new(0.0, 0.0)) < 0.2 && w.b.dist(Vec2::new(70.0, 0.0)) < 0.2, "{:?} {:?}", w.a, w.b);
        // Another wall type in line: two walls.
        let mut walls = vec![piece((0.0, 0.0), (16.0, 0.0), 7.6), PlanWall { wall_type: Some("B".into()), ..piece((16.0, 0.0), (32.0, 0.0), 7.6) }];
        assert_eq!(join_wall_pieces(&mut walls, &[], 1.0), 0);
        // In ½" units (k = 2) a ⅜" joint is 0.75 and still a joint; 6" is a gap.
        let mut walls = vec![piece((0.0, 0.0), (32.0, 0.0), 15.2), piece((32.75, 0.0), (64.0, 0.0), 15.2), piece((76.0, 0.0), (90.0, 0.0), 15.2)];
        assert_eq!(join_wall_pieces(&mut walls, &[], 2.0), 1);
        assert_eq!(walls.len(), 2);
        assert_eq!(join_wall_pieces(&mut walls, &[], f64::NAN), 0);
        // A header drawn across a door's opening, touching the wall each side: the swing hung at
        // one jamb and shutting at the other keeps three walls.
        let pieces = || vec![piece((0.0, 0.0), (100.0, 0.0), 4.0), piece((100.0, 0.0), (136.0, 0.0), 4.0), piece((136.0, 0.0), (300.0, 0.0), 4.0)];
        let mut walls = pieces();
        assert_eq!(join_wall_pieces(&mut walls, &[Vec2::new(100.0, 0.0), Vec2::new(100.0, 36.0), Vec2::new(136.0, 0.0)], 1.0), 0);
        // A swing elsewhere doesn't.
        let mut walls = pieces();
        assert_eq!(join_wall_pieces(&mut walls, &[Vec2::new(100.0, 120.0)], 1.0), 2);
    }

    /// The walls planned on a plan with one 8" block wall (7⅝" units, 15⅝" long, ⅜" joints), 21
    /// whole units and a cut one at each end, each block drawn as its own closed outline with its
    /// cores, and specks of the block's fill (a stippled plan's dots) on its faces.
    fn planned_block_wall(specks_layer: &str) -> Vec<PlanWall> {
        let mut d = Drawing::new_imperial();
        let mut add = |kind: EntityKind, layer: &str| {
            let h = Handle(d.handseed);
            d.handseed = h.0 + 1;
            let mut e = Entity::new(h, kind);
            e.common.layer = layer.into();
            d.model.push(e);
        };
        let outline = |pts: &[(f64, f64)]| {
            EntityKind::LwPolyline(cadaddy_doc::LwPolyline {
                vertices: pts
                    .iter()
                    .map(|&(x, y)| cadaddy_geom::PolyVertex { p: Vec2::new(x, y + 100.0), bulge: 0.0, start_width: 0.0, end_width: 0.0 })
                    .collect(),
                closed: true,
                const_width: 0.0,
                elevation: 0.0,
                plinegen: false,
            })
        };
        let speck = |x: f64, y: f64| EntityKind::Line(cadaddy_doc::Line { a: Vec3::new(x, y + 100.0, 0.0), b: Vec3::new(x + 0.08, y + 100.0, 0.0) });
        let mut x = 0.0;
        for len in std::iter::once(7.625).chain(std::iter::repeat_n(15.625, 21)).chain(std::iter::once(11.625)) {
            add(outline(&[(x, 0.0), (x + len, 0.0), (x + len, 7.625), (x, 7.625)]), "A-WALL");
            if len > 15.0 {
                for c in [x + 1.5, x + 8.5] {
                    add(outline(&[(c, 1.5), (c + 5.6, 1.5), (c + 5.6, 6.1), (c, 6.1)]), "A-WALL");
                }
                for (sx, sy) in [(x + 3.0, 0.0), (x + 11.0, 7.625), (x + 7.0, 3.0)] {
                    add(speck(sx, sy), specks_layer);
                }
            }
            x += len + 0.375;
        }
        let rules: Rules = serde_json::from_value(json!({
            "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "kind": "wall"}, {"layer": "FILL", "kind": "wall"}]
        }))
        .unwrap();
        let level = rules.levels[0].clone();
        let g = WallGen { rules, auto: false, slab_ft: 0.0, layers: None, spec: None };
        plan_level(&d, &g, &level, 12.0, &mut Vec::new()).walls
    }

    #[test]
    fn a_block_wall_drawn_block_by_block_on_a_stippled_plan_is_one_wall_end_to_end() {
        // The specks on the blocks' own layer, or on another wall layer (a print's geometry layer).
        for layer in ["A-WALL", "FILL"] {
            let walls = planned_block_wall(layer);
            let [w] = walls.as_slice() else { panic!("{layer}: {} walls: {:?}", walls.len(), walls.iter().map(|w| (w.a, w.b)).collect::<Vec<_>>()) };
            // From the first cut block's end to the last's: 7⅝ + 21 × 16 + 11⅝ = 355¼".
            let len = w.a.dist(w.b);
            assert!((len - 355.25).abs() < 0.5, "{layer}: {len}");
            assert!((w.thickness - 7.625).abs() < 0.1, "{layer}: {}", w.thickness);
        }
    }

    #[test]
    fn a_gap_another_wall_runs_across_is_no_opening() {
        // Two pieces of one line, x 0–100 and 172–300 (a 6' gap), inches.
        let mut walls = vec![plan_wall((0.0, 0.0), (100.0, 0.0)), plan_wall((172.0, 0.0), (300.0, 0.0))];
        let (g0, g1) = (Vec2::new(100.0, 0.0), Vec2::new(172.0, 0.0));
        // Nothing across: an opening.
        assert!(!gap_crossed(&walls, 0, 1, g0, g1));
        // A wall meeting the jamb end-on (the room's return wall) is no wall across it.
        walls.push(plan_wall((100.0, 0.0), (100.0, -96.0)));
        assert!(!gap_crossed(&walls, 0, 1, g0, g1));
        // A wall that lies along the gap's line is no wall across it either.
        walls.push(plan_wall((110.0, 0.0), (160.0, 0.0)));
        assert!(!gap_crossed(&walls, 0, 1, g0, g1));
        // A partition ending on the gap's line from one side (meeting an outside wall in a
        // window's gap) runs up to the opening, not across it.
        walls.push(plan_wall((136.0, 0.0), (136.0, 120.0)));
        assert!(!gap_crossed(&walls, 0, 1, g0, g1));
        // A demising wall running across the gap's middle: two walls lined up, no opening.
        walls.push(plan_wall((136.0, -120.0), (136.0, 120.0)));
        assert!(gap_crossed(&walls, 0, 1, g0, g1));
    }

    #[test]
    fn a_gap_across_a_corridor_is_no_opening_one_along_it_is() {
        // A corridor 66" wide (y 0–66) along x, inches.
        let corridor = cadaddy_walk3d::Circulation {
            kind: cadaddy_walk3d::CirculationKind::Corridor,
            level: "Level 2".into(),
            name: "CORRIDOR".into(),
            at: [500.0, 33.0],
            rects: vec![[[0.0, 0.0], [1000.0, 66.0]]],
        };
        let cs = [&corridor];
        // Two units' entry walls lined up across it: the gap runs y 0–66 at x 300.
        let (g0, g1) = (Vec2::new(300.0, 0.0), Vec2::new(300.0, 66.0));
        let u = (g1 - g0) * (1.0 / g0.dist(g1));
        assert!(in_corridor_air(&cs, (g0 + g1) * 0.5, u.perp(), 6.0));
        // A cased opening in the corridor's own wall (y −3, centreline): corridor one side only.
        let (h0, h1) = (Vec2::new(300.0, -3.0), Vec2::new(372.0, -3.0));
        let v = (h1 - h0) * (1.0 / h0.dist(h1));
        assert!(!in_corridor_air(&cs, (h0 + h1) * 0.5, v.perp(), 6.0));
        // A gap in a room, away from the corridor.
        let (k0, k1) = (Vec2::new(300.0, 100.0), Vec2::new(300.0, 160.0));
        let w = (k1 - k0) * (1.0 / k0.dist(k1));
        assert!(!in_corridor_air(&cs, (k0 + k1) * 0.5, w.perp(), 6.0));
        // No corridors traced: nothing is in a corridor's air.
        assert!(!in_corridor_air(&[], (g0 + g1) * 0.5, u.perp(), 6.0));
    }

    /// A 20' × 10' room of 6" walls (feet; two face lines each on A-WALL), its south and north
    /// walls broken at `south` and `north` (x from–to), with `extra` objects added (layer, kind).
    /// The level planned: (windows, notes).
    fn room_with_gaps(south: &[(f64, f64)], north: &[(f64, f64)], extra: Vec<(&str, EntityKind)>) -> (usize, Vec<String>) {
        let mut d = Drawing::new_imperial();
        let mut push = |d: &mut Drawing, layer: &str, kind: EntityKind| {
            let h = Handle(d.handseed);
            d.handseed = h.0 + 1;
            let mut e = Entity::new(h, kind);
            e.common.layer = layer.into();
            d.model.push(e);
        };
        let line = |a: (f64, f64), b: (f64, f64)| {
            EntityKind::Line(cadaddy_doc::Line { a: Vec3::new(a.0 * 12.0, a.1 * 12.0, 0.0), b: Vec3::new(b.0 * 12.0, b.1 * 12.0, 0.0) })
        };
        // A run from x0 to x1 at y, broken at the gaps.
        let run = |d: &mut Drawing, y: f64, x0: f64, x1: f64, gaps: &[(f64, f64)], push: &mut dyn FnMut(&mut Drawing, &str, EntityKind)| {
            let mut from = x0;
            for &(g0, g1) in gaps {
                push(d, "A-WALL", line((from, y), (g0, y)));
                from = g1;
            }
            push(d, "A-WALL", line((from, y), (x1, y)));
        };
        run(&mut d, 0.0, 0.0, 20.0, south, &mut push);
        run(&mut d, 0.5, 0.5, 19.5, south, &mut push);
        run(&mut d, 10.0, 0.0, 20.0, north, &mut push);
        run(&mut d, 9.5, 0.5, 19.5, north, &mut push);
        for (a, b) in [((20.0, 0.0), (20.0, 10.0)), ((0.0, 10.0), (0.0, 0.0)), ((19.5, 0.5), (19.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))] {
            push(&mut d, "A-WALL", line(a, b));
        }
        for (layer, kind) in extra {
            push(&mut d, layer, kind);
        }
        let rules: Rules = serde_json::from_value(json!({
            "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "kind": "wall"}]
        }))
        .unwrap();
        let level = rules.levels[0].clone();
        let g = WallGen { rules, auto: false, slab_ft: 0.0, layers: None, spec: None };
        let mut notes = Vec::new();
        let pl = plan_level(&d, &g, &level, 12.0, &mut notes);
        (pl.leaves.iter().filter(|l| l.window).count(), notes)
    }

    fn line_ft(a: (f64, f64), b: (f64, f64)) -> EntityKind {
        EntityKind::Line(cadaddy_doc::Line { a: Vec3::new(a.0 * 12.0, a.1 * 12.0, 0.0), b: Vec3::new(b.0 * 12.0, b.1 * 12.0, 0.0) })
    }

    fn text_ft(at: (f64, f64), value: &str) -> EntityKind {
        EntityKind::Text(cadaddy_doc::Text {
            insert: Vec3::new(at.0 * 12.0, at.1 * 12.0, 0.0),
            align_pt: None,
            height: 4.0,
            value: value.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: Default::default(),
            valign: Default::default(),
        })
    }

    #[test]
    fn a_bare_gap_in_an_outside_wall_is_no_window() {
        // The south wall broken 8'–12' with nothing drawn in the gap: an opening, not a window
        // (before Q6293 every such gap became one).
        let (windows, notes) = room_with_gaps(&[(8.0, 12.0)], &[], Vec::new());
        assert_eq!(windows, 0, "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("1 gaps in outside walls with no window drawn")), "{notes:?}");
        // Gaps in both outside walls, still nothing drawn in them.
        let (windows, notes) = room_with_gaps(&[(3.0, 7.0), (12.0, 15.0)], &[(8.0, 12.0)], Vec::new());
        assert_eq!(windows, 0, "{notes:?}");
    }

    #[test]
    fn glazing_lines_across_a_gap_make_it_a_window() {
        // Glass drawn across the gap, between the wall's faces (its own layer).
        let glass = vec![("A-GLAZ", line_ft((8.0, 0.2), (12.0, 0.2))), ("A-GLAZ", line_ft((8.0, 0.3), (12.0, 0.3)))];
        let (windows, notes) = room_with_gaps(&[(8.0, 12.0)], &[], glass);
        assert_eq!(windows, 1, "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("1 drawn") && n.contains("0 inferred")), "{notes:?}");
        // A line running through the gap and far past its jambs (a dimension's, a grid's) is no glass.
        let (windows, notes) = room_with_gaps(&[(8.0, 12.0)], &[], vec![("A-ANNO", line_ft((-5.0, 0.25), (25.0, 0.25)))]);
        assert_eq!(windows, 0, "{notes:?}");
        // A line across only a sliver of the gap is no glass either.
        let (windows, notes) = room_with_gaps(&[(8.0, 12.0)], &[], vec![("A-ANNO", line_ft((8.0, 0.25), (9.0, 0.25)))]);
        assert_eq!(windows, 0, "{notes:?}");
    }

    #[test]
    fn a_window_tag_marks_its_gap_and_its_width_stands_for_the_typical_ones() {
        // Two 4' gaps in the south wall, one tagged W5 outside it; a 6' gap in the north wall.
        let (windows, notes) = room_with_gaps(&[(3.0, 7.0), (11.0, 15.0)], &[(8.0, 14.0)], vec![("A-ANNO", text_ft((5.0, -1.5), "W5"))]);
        // The tagged gap is a window, the untagged 4' one the same window (typical, inferred);
        // the 6' gap has no window of its width drawn: an opening.
        assert_eq!(windows, 2, "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("1 drawn") && n.contains("1 inferred")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("1 gaps in outside walls with no window drawn")), "{notes:?}");
        // A text that is no window mark (a room number, a door's) marks nothing.
        for text in ["R12", "D1", "WALL", "W"] {
            let (windows, notes) = room_with_gaps(&[(3.0, 7.0)], &[], vec![("A-ANNO", text_ft((5.0, -1.5), text))]);
            assert_eq!(windows, 0, "{text}: {notes:?}");
        }
        // A tag farther than its reach from every gap marks none.
        let (windows, notes) = room_with_gaps(&[(3.0, 7.0)], &[], vec![("A-ANNO", text_ft((5.0, -9.0), "W5"))]);
        assert_eq!(windows, 0, "{notes:?}");
    }

    /// A level planned from `lines` (feet, layer A-WALL unless given): its window panes' ends
    /// (feet) and the notes.
    fn plan_lines(lines: Vec<(&str, EntityKind)>) -> (Vec<(Vec2, Vec2)>, Vec<String>) {
        let mut d = Drawing::new_imperial();
        for (layer, kind) in lines {
            let h = Handle(d.handseed);
            d.handseed = h.0 + 1;
            let mut e = Entity::new(h, kind);
            e.common.layer = layer.into();
            d.model.push(e);
        }
        let rules: Rules = serde_json::from_value(json!({
            "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "kind": "wall"}]
        }))
        .unwrap();
        let level = rules.levels[0].clone();
        let g = WallGen { rules, auto: false, slab_ft: 0.0, layers: None, spec: None };
        let mut notes = Vec::new();
        let pl = plan_level(&d, &g, &level, 12.0, &mut notes);
        (pl.leaves.iter().filter(|l| l.window).map(|l| (l.a * (1.0 / 12.0), l.b * (1.0 / 12.0))).collect(), notes)
    }

    /// A wall's two face lines (feet) along x from `x0` to `x1` at `y` (its centreline), `t_in`
    /// inches thick, broken at `gaps` (x from–to).
    fn wall_x(y: f64, t_in: f64, x0: f64, x1: f64, gaps: &[(f64, f64)]) -> Vec<(&'static str, EntityKind)> {
        let h = t_in / 24.0;
        let mut out = Vec::new();
        for face in [y - h, y + h] {
            let mut from = x0;
            for &(g0, g1) in gaps {
                out.push(("A-WALL", line_ft((from, face), (g0, face))));
                from = g1;
            }
            out.push(("A-WALL", line_ft((from, face), (x1, face))));
        }
        out
    }

    /// The same along y at `x`.
    fn wall_y(x: f64, t_in: f64, y0: f64, y1: f64, gaps: &[(f64, f64)]) -> Vec<(&'static str, EntityKind)> {
        let h = t_in / 24.0;
        let mut out = Vec::new();
        for face in [x - h, x + h] {
            let mut from = y0;
            for &(g0, g1) in gaps {
                out.push(("A-WALL", line_ft((face, from), (face, g0))));
                from = g1;
            }
            out.push(("A-WALL", line_ft((face, from), (face, y1))));
        }
        out
    }

    #[test]
    fn a_line_pair_against_an_outside_walls_inside_face_hosts_no_window() {
        // A 20' x 10' room of 6" walls, the south wall solid. Against its inside face (0.25"
        // clear) two 3'-6" through-wall unit sleeves (a line pair 5" deep each) with 6' between them,
        // a window mark by that gap. The sleeves' gap is no outside wall's: the solid south wall
        // stands just past it. Before Q6308 the window was built there, inside the building
        // (the south wall's centreline was nearer than its own thickness, so the ray skipped it).
        let mut lines = Vec::new();
        lines.extend(wall_x(0.25, 6.0, 0.0, 20.0, &[]));
        lines.extend(wall_x(9.75, 6.0, 0.0, 20.0, &[]));
        lines.extend(wall_y(0.25, 6.0, 0.0, 10.0, &[]));
        lines.extend(wall_y(19.75, 6.0, 0.0, 10.0, &[]));
        let y = 0.5 + 0.25 / 12.0 + 2.5 / 12.0;
        lines.extend(wall_x(y, 5.0, 3.0, 16.0, &[(6.5, 12.5)]));
        lines.push(("A-ANNO", text_ft((9.5, 2.0), "W5")));
        let (windows, notes) = plan_lines(lines);
        assert!(windows.is_empty(), "a window inside the building: {windows:?} {notes:?}");
        // The same gap in the outside wall itself, its window tagged the same: built there, in
        // the wall's thickness.
        let mut lines = Vec::new();
        lines.extend(wall_x(0.25, 6.0, 0.0, 20.0, &[(6.5, 12.5)]));
        lines.extend(wall_x(9.75, 6.0, 0.0, 20.0, &[]));
        lines.extend(wall_y(0.25, 6.0, 0.0, 10.0, &[]));
        lines.extend(wall_y(19.75, 6.0, 0.0, 10.0, &[]));
        lines.push(("A-ANNO", text_ft((9.5, 2.0), "W5")));
        let (windows, notes) = plan_lines(lines);
        assert_eq!(windows.len(), 1, "{notes:?}");
        for (a, b) in &windows {
            assert!((a.y - 0.25).abs() < 0.05 && (b.y - 0.25).abs() < 0.05, "pane off the outside wall's centreline: {a:?} {b:?}");
        }
    }

    #[test]
    fn a_gap_across_a_corridor_between_lined_up_walls_is_no_window() {
        // Two rooms south of a 5' corridor and two north of it (6" walls, a 20' x 25' building),
        // their demising walls lined up on x = 10 and ending at the corridor walls. The corridor
        // is open at its east end (a gap in the outside wall), so a ray along it from the gap
        // between the demising walls meets no wall. A window mark sits in the corridor by that
        // gap. Before Q6308 the gap across the corridor was taken as an outside wall's and built
        // as a window standing across the corridor.
        let mut lines = Vec::new();
        lines.extend(wall_x(0.25, 6.0, 0.0, 20.0, &[]));
        lines.extend(wall_x(24.75, 6.0, 0.0, 20.0, &[]));
        lines.extend(wall_y(0.25, 6.0, 0.0, 25.0, &[]));
        lines.extend(wall_y(19.75, 6.0, 0.0, 25.0, &[(10.25, 14.75)]));
        lines.extend(wall_x(10.0, 6.0, 0.5, 19.5, &[]));
        lines.extend(wall_x(15.0, 6.0, 0.5, 19.5, &[]));
        lines.extend(wall_y(10.0, 6.0, 0.5, 9.75, &[]));
        lines.extend(wall_y(10.0, 6.0, 15.25, 24.5, &[]));
        lines.push(("A-ANNO", text_ft((10.8, 12.5), "W5")));
        let (windows, notes) = plan_lines(lines);
        assert!(windows.is_empty(), "a window across the corridor: {windows:?} {notes:?}");
        assert!(notes.iter().any(|n| n.contains("cross their line at both ends")), "{notes:?}");
    }

    #[test]
    fn a_ray_out_through_a_rooms_window_is_still_inside_the_building() {
        // A row of rooms: the south outside wall has a 4' window (glazing drawn) at x 8-12; the
        // room's north wall (on a corridor) has a 4' opening at the same x, nothing drawn in it;
        // the corridor's north wall is the building's, solid. A ray south from the inner opening
        // leaves through the window's opening: before Q6308 that made the inner wall an outside
        // wall and the opening a typical window (the drawn one's width) inside the building.
        let mut lines = Vec::new();
        lines.extend(wall_x(0.25, 6.0, 0.0, 20.0, &[(8.0, 12.0)]));
        lines.extend(wall_x(10.0, 6.0, 0.5, 19.5, &[(8.0, 12.0)]));
        lines.extend(wall_x(15.75, 6.0, 0.0, 20.0, &[]));
        lines.extend(wall_y(0.25, 6.0, 0.0, 16.0, &[]));
        lines.extend(wall_y(19.75, 6.0, 0.0, 16.0, &[]));
        lines.push(("A-GLAZ", line_ft((8.0, 0.2), (12.0, 0.2))));
        lines.push(("A-GLAZ", line_ft((8.0, 0.3), (12.0, 0.3))));
        let (windows, notes) = plan_lines(lines);
        assert_eq!(windows.len(), 1, "one window, in the outside wall: {windows:?} {notes:?}");
        assert!(windows.iter().all(|(a, b)| a.y < 0.6 && b.y < 0.6), "{windows:?}");
    }

    #[test]
    fn faces_outside_counts_walls_clear_past_its_face_not_the_same_wall_read_twice() {
        let w = |a: (f64, f64), b: (f64, f64), t: f64| PlanWall {
            a: Vec2::new(a.0, a.1),
            b: Vec2::new(b.0, b.1),
            thickness: t,
            top: 100.0,
            source: Handle(1),
            cuts: Vec::new(),
            extra: Vec::new(),
            wall_type: None,
        };
        let (p, nrm) = (Vec2::new(50.0, 10.0), Vec2::new(0.0, 1.0));
        // Alone: outside both ways.
        assert!(faces_outside(p, nrm, 5.0, 1.0, &[], &[]));
        // A 7" wall touching its face south and a wall far north: inside.
        let touching = [w((0.0, 3.65), (100.0, 3.65), 7.3), w((0.0, 200.0), (100.0, 200.0), 6.0)];
        assert!(!faces_outside(Vec2::new(50.0, 10.2), nrm, 5.0, 1.0, &touching, &[]));
        // A 14" wall holding this 6" one inside its thickness (the same wall read twice): still
        // outside to the south.
        let holding = [w((0.0, 6.6), (100.0, 6.6), 14.0), w((0.0, 200.0), (100.0, 200.0), 6.0)];
        assert!(faces_outside(Vec2::new(50.0, 10.0), nrm, 6.0, 1.0, &holding, &[]));
        // Another wall's opening (a window's gap) south of it blocks the ray like the wall.
        let north = [w((0.0, 200.0), (100.0, 200.0), 6.0)];
        assert!(faces_outside(p, nrm, 6.0, 1.0, &north, &[]));
        assert!(!faces_outside(p, nrm, 6.0, 1.0, &north, &[(Vec2::new(40.0, -100.0), Vec2::new(60.0, -100.0), 6.0)]));
    }

    #[test]
    fn a_gap_between_walls_crossing_its_line_at_both_ends_is_found_but_not_a_jamb_return() {
        let w = |a: (f64, f64), b: (f64, f64)| PlanWall {
            a: Vec2::new(a.0, a.1),
            b: Vec2::new(b.0, b.1),
            thickness: 6.0,
            top: 100.0,
            source: Handle(1),
            cuts: Vec::new(),
            extra: Vec::new(),
            wall_type: None,
        };
        // Pieces 0 and 1 on x = 0 (y -100..-3 and 63..160); corridor walls on y = 0 and y = 60.
        let corridor =
            [w((0.0, -100.0), (0.0, -3.0)), w((0.0, 63.0), (0.0, 160.0)), w((-200.0, 0.0), (200.0, 0.0)), w((-200.0, 60.0), (200.0, 60.0))];
        assert!(gap_between_crossing_walls(&corridor, 0, 1, Vec2::new(0.0, -3.0), Vec2::new(0.0, 63.0), 6.0));
        // Partitions meeting the jambs from one side only (ending at the wall's line): no.
        let returns = [w((0.0, -100.0), (0.0, -3.0)), w((0.0, 63.0), (0.0, 160.0)), w((3.0, 0.0), (200.0, 0.0)), w((3.0, 60.0), (200.0, 60.0))];
        assert!(!gap_between_crossing_walls(&returns, 0, 1, Vec2::new(0.0, -3.0), Vec2::new(0.0, 63.0), 6.0));
        // Crossed at one end only: no.
        let one = [w((0.0, -100.0), (0.0, -3.0)), w((0.0, 63.0), (0.0, 160.0)), w((-200.0, 0.0), (200.0, 0.0))];
        assert!(!gap_between_crossing_walls(&one, 0, 1, Vec2::new(0.0, -3.0), Vec2::new(0.0, 63.0), 6.0));
    }

    #[test]
    fn window_marks_are_a_window_prefix_and_a_number_or_a_schedule_rows_mark() {
        for m in ["W5", "w2", "W-3", "W.4", "W12A", "WIN5", "WDW 6".replace(' ', "").as_str(), " W7 "] {
            assert!(is_window_mark(m, &[]), "{m}");
        }
        for m in ["W", "W1234", "W1AB", "WD1", "D1", "R12", "WALL", "W-", "WINDOW", "", "W5 W6"] {
            assert!(!is_window_mark(m, &[]), "{m}");
        }
        // A schedule's own window marks, whatever their form.
        assert!(is_window_mark("A", &["A".into(), "B".into()]) && is_window_mark("b", &["A".into(), "B".into()]));
        assert!(!is_window_mark("C", &["A".into(), "B".into()]));
    }

    #[test]
    fn a_tag_marks_its_gap_and_a_wider_gap_holds_its_types_width() {
        let k = 1.0;
        // Three gaps in a line (inches): two 72" and a 108" one.
        let gaps = vec![
            (Vec2::new(0.0, 0.0), Vec2::new(72.0, 0.0), 6.0),
            (Vec2::new(200.0, 0.0), Vec2::new(272.0, 0.0), 6.0),
            (Vec2::new(400.0, 0.0), Vec2::new(508.0, 0.0), 6.0),
        ];
        let tag = |x: f64, mark: &str, width: Option<f64>| WindowTag { at: Vec2::new(x, -30.0), mark: mark.into(), width };
        let fit = |gap: usize, width: f64| Some(TagFit { gap, width });
        // W5 by each 72" gap, and one by the 108" gap: W5 is the 72" window, and the wider gap
        // holds a 72" W5 in its middle (Q6311: the plan's opening wider than its window).
        let tags = vec![tag(36.0, "W5", None), tag(236.0, "W5", None), tag(420.0, "W5", None)];
        let f = window_tag_fits(&tags, &gaps, k);
        assert_eq!(f.per_tag, vec![fit(0, 72.0), fit(1, 72.0), fit(2, 72.0)]);
        assert_eq!(f.width_of("W5"), Some(72.0));
        // A mark by one gap only is that gap's, at its width.
        assert_eq!(window_tag_fits(&[tag(420.0, "W2", None)], &gaps, k).per_tag, vec![fit(2, 108.0)]);
        // The schedule's width decides when there is one: a narrower gap is no window of it.
        let f = window_tag_fits(&[tag(420.0, "W3", Some(108.0)), tag(36.0, "W3", Some(108.0))], &gaps, k);
        assert_eq!(f.per_tag, vec![fit(2, 108.0), None]);
        // A tag between two gaps marks the nearer only.
        assert_eq!(window_tag_fits(&[tag(100.0, "W4", None)], &gaps, k).per_tag, vec![fit(0, 72.0)]);
        // A wider gap another tag fits is that tag's.
        let f = window_tag_fits(&[tag(420.0, "W7", Some(108.0)), tag(430.0, "W5", Some(72.0))], &gaps, k);
        assert_eq!(f.per_tag, vec![fit(2, 108.0), None]);
        // Out of reach of every gap: none, and no width read for its mark.
        let far = window_tag_fits(&[WindowTag { at: Vec2::new(36.0, -200.0), mark: "W5".into(), width: None }], &gaps, k);
        assert_eq!((far.width_of("W5"), far.per_tag), (None, vec![None]));
        assert!(window_tag_fits(&tags, &[], k).per_tag.iter().all(Option::is_none));
    }
    /// Q6297: a slot between two pieces of one wall line narrower than the smallest opening is
    /// built solid (a mullion, a post); one an opening's width, a hair, a tee or a parallel wall
    /// beside it is not.
    #[test]
    fn narrow_slots_close_mullion_gaps_only() {
        let k = 1.0;
        let walls = vec![
            plan_wall((0.0, 0.0), (30.0, 0.0)),
            // A 2" mullion slot, the next piece drawn backwards.
            plan_wall((62.0, 0.0), (32.0, 0.0)),
            // A 36" door's gap: an opening, left open.
            plan_wall((98.0, 0.0), (130.0, 0.0)),
            // A hair apart: one wall already.
            plan_wall((130.005, 0.0), (160.0, 0.0)),
            // A parallel wall 24" off the line, and a tee across its end: no slot.
            plan_wall((0.0, 24.0), (30.0, 24.0)),
            plan_wall((161.0, -20.0), (161.0, 20.0)),
        ];
        let grid = WallGrid::build(&walls, 48.0);
        let slots = narrow_slots(&walls, &grid, 18.0, k);
        assert_eq!(slots.len(), 1, "{slots:?}");
        let (_, a, b) = slots[0];
        let (lo, hi) = (a.x.min(b.x), a.x.max(b.x));
        assert!((lo - 30.0).abs() < 1e-6 && (hi - 32.0).abs() < 1e-6 && a.y.abs() < 1e-6, "{slots:?}");
    }

    /// Q6297: two walls at a corner that both stop short of it (a corner post between them) run
    /// on to it; walls in line, a wall whose crossing is past the reach and a tee are left alone.
    #[test]
    fn close_corners_runs_walls_on_to_a_post_corner_only() {
        let mut walls = vec![
            // An L: the side wall stops 6" short, the frame 8" short of their crossing at (0, 0).
            plan_wall((0.0, 120.0), (0.0, 6.0)),
            plan_wall((8.0, 0.0), (120.0, 0.0)),
            // Far apart: their crossing 40" past both ends, beyond the reach.
            plan_wall((300.0, 200.0), (300.0, 40.0)),
            plan_wall((340.0, 0.0), (500.0, 0.0)),
            // In line: no corner.
            plan_wall((600.0, 0.0), (700.0, 0.0)),
            plan_wall((706.0, 0.0), (800.0, 0.0)),
        ];
        assert_eq!(close_corners(&mut walls, 12.0), 1, "{:?}", walls.iter().map(|w| (w.a, w.b)).collect::<Vec<_>>());
        // The side wall now reaches the frame's centre line plus half its thickness, the frame the
        // side wall's.
        assert!(walls[0].b.dist(Vec2::new(0.0, -3.0)) < 1e-6, "{:?}", walls[0].b);
        assert!(walls[1].a.dist(Vec2::new(-3.0, 0.0)) < 1e-6, "{:?}", walls[1].a);
        assert!(walls[2].b.dist(Vec2::new(300.0, 40.0)) < 1e-9 && walls[3].a.dist(Vec2::new(340.0, 0.0)) < 1e-9);
        assert!(walls[4].b.dist(Vec2::new(700.0, 0.0)) < 1e-9 && walls[5].a.dist(Vec2::new(706.0, 0.0)) < 1e-9);
    }
}
