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

/// A wall's tag with its wall type (`type:<name>`, after the source), for IFC's materials.
fn wall_tag(level: &str, source: Handle, wall_type: Option<&str>) -> Vec<RawTag> {
    let mut x = tag(Part::Wall, level, source);
    if let Some(n) = wall_type {
        x.push(RawTag { code: 1000, value: format!("type:{}", n.chars().take(250).collect::<String>()) });
    }
    x
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
        for (part, m, source, wall_type) in pl.meshes() {
            let xdata = if part == Part::Wall { wall_tag(&level.name, source, wall_type.as_deref()) } else { tag(part, &level.name, source) };
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
    let mut r = json!({
        "walls": c("wall"), "doors": c("door"), "windows": c("window"), "floors": c("floor"), "ceilings": c("ceiling"),
        "levels": scene.levels.len(), "removed": removed, "notes": notes,
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
    match head.trim_end_matches('.') {
        "CORRIDOR" | "CORR" | "HALL" | "HALLWAY" => Some(Corridor),
        "STAIR" | "STAIRS" | "STAIRWAY" | "STAIRWELL" => Some(Stair),
        "ELEV" | "ELEVATOR" => Some(Hoistway),
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

/// The objects on the rules' layers, by handle, as the drawing holds them (shared pointers: an
/// edit replaces the object's).
fn plan(d: &Drawing, g: &WallGen) -> Vec<(Handle, *const Entity)> {
    let watched = |layer: &str| {
        g.rules.layer(layer).is_some() || g.spec.as_ref().and_then(|sp| sp.role(layer)).is_some_and(|r| r.role != cadaddy_walk3d::Role::Ignore)
    };
    d.model.iter().filter(|e| made_as(e).is_none() && watched(&e.common.layer)).map(|e| (e.handle, Arc::as_ptr(e))).collect()
}

/// After a command: when the drawing has an associative WALLGEN and an object on its rules'
/// layers changed, build again (in the same undo step).
pub(crate) fn after_command(s: &mut Session, before: Option<&Arc<Drawing>>) {
    let Some(before) = before else { return };
    let Ok(st) = s.state() else { return };
    let Some(g) = st.wallgen.clone().filter(|g| g.auto) else { return };
    if Arc::ptr_eq(before, &st.doc) || plan(before, &g) == plan(&st.doc, &g) {
        return;
    }
    // Built during this command (a guided job builds as one of its steps) from the plan as it
    // is now: the model is current.
    let built = st.model_record.as_ref().and_then(|r| r.built.as_ref());
    if built.is_some_and(|b| Arc::ptr_eq(&b.0, &st.doc) || plan(&b.0, &g) == plan(&st.doc, &g)) {
        return;
    }
    // Rebuilt keeping the hand edits whose plan objects didn't change; the edits that conflict
    // are echoed and kept in the drawing's last revision (`model.revise {last: true}`).
    let (old, rooms) =
        st.model_record.as_ref().map_or_else(Default::default, |r| r.plan.iter().cloned().partition::<Vec<_>, _>(|i| i.kind != "room"));
    let how = super::revise3d::Revise { by_handle: true, tolerance: None, rebuild: true, auto: true };
    if let Err(e) = super::revise3d::revise(s, &g, old, rooms, &how) {
        s.echo(e.to_string());
    }
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
    /// per door leaf and window pane.
    fn meshes(&self) -> Vec<(Part, Mesh, Handle, Option<String>)> {
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
                out.push((Part::Wall, m, w.source, w.wall_type.clone()));
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
            out.push((if l.window { Part::Glass } else { Part::Door }, m, l.source, None));
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
    let segs = segments(&pair_ents);
    let mut found = pairs(d, &segs, &lim);
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
    // Openings: each cuts the wall it lies in.
    let mut ops = openings(d, &ents, &segments(&ents), k);
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
    if let Some(sp) = &g.spec {
        super::schedules::size_openings(sp, &mut ops, &mut fitted, upf);
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
            let (wi, _) = open_grid
                .at(o.at)
                .iter()
                .filter_map(|&i| {
                    let w = &pl.walls[i];
                    let len = w.a.dist(w.b);
                    if len <= 1e-9 {
                        return None;
                    }
                    let u = (w.b - w.a) * (1.0 / len);
                    let off = u.perp().dot(o.at - w.a).abs();
                    let s = u.dot(o.at - w.a);
                    let fits = drawn_swing.is_none_or(|ends| swing_shuts_along(w, o.at, ends, o.width, k));
                    (fits && off <= w.thickness.max(2.0 * k) && s >= -o.width && s <= len + o.width).then_some((i, off))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))?;
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
                });
            }
        }
    }
    // Gaps between two pieces of one wall line: a door where a swing hangs from a jamb (or a
    // door swing's opening covers it), a window in an outside wall, else a cased opening.
    // Uses a spatial grid so each wall only checks nearby collinear neighbors for gaps.
    let (min_gap, max_gap) = (oz.min_ft * upf, oz.max_ft * upf);
    let n = pl.walls.len().min(10_000);
    let mut gaps = Vec::new();
    let gap_cell = max_gap.max(max_thick);
    let gap_grid = WallGrid::build(&pl.walls[..n], gap_cell);
    for i in 0..n {
        let a = &pl.walls[i];
        let la = a.a.dist(a.b);
        if la <= 1e-9 {
            continue;
        }
        let u = (a.b - a.a) * (1.0 / la);
        for &j in gap_grid.at(a.b) {
            if j >= n || i == j {
                continue;
            }
            let b = &pl.walls[j];
            let lb = b.a.dist(b.b);
            if lb <= 1e-9 || u.cross((b.b - b.a) * (1.0 / lb)).abs() > 0.01 || u.perp().dot(b.a - a.a).abs() > a.thickness.max(b.thickness) / 2.0 {
                continue;
            }
            // `b` starts past `a`'s end, with nothing of the line between.
            let (b0, b1) = (u.dot(b.a - a.a).min(u.dot(b.b - a.a)), u.dot(b.a - a.a).max(u.dot(b.b - a.a)));
            let gap = b0 - la;
            if gap < min_gap || gap > max_gap || b1 <= la {
                continue;
            }
            let gap_mid = a.b + u * (gap * 0.5);
            let between = gap_grid.at(gap_mid).iter().any(|&kk| {
                kk < n && kk != i && kk != j && {
                    let c = &pl.walls[kk];
                    u.cross(c.b - c.a).abs() <= 0.01 * c.a.dist(c.b) && u.perp().dot(c.a - a.a).abs() <= a.thickness / 2.0 && {
                        let (c0, c1) = (u.dot(c.a - a.a).min(u.dot(c.b - a.a)), u.dot(c.a - a.a).max(u.dot(c.b - a.a)));
                        c1 > la + 1e-6 && c0 < b0 - 1e-6
                    }
                }
            });
            // Another wall running across the gap (a corridor's walls between two rooms' walls
            // that happen to line up, a demising wall between two units' bath walls): the two
            // pieces are two walls, the gap no opening; nothing the plan draws spans it (Q6160).
            if !between && !gap_crossed(&pl.walls[..n], i, j, a.b, a.b + u * gap) {
                gaps.push((i, a.b, a.b + u * gap, a.thickness));
            }
        }
    }
    // Outside: no other wall across one side of it.
    let outside = |p: Vec2, nrm: Vec2, walls: &[PlanWall]| {
        let hits = |dir: Vec2| {
            walls.iter().any(|w| {
                let (q, r) = (w.a, w.b - w.a);
                let den = dir.cross(r);
                if den.abs() < 1e-12 {
                    return false;
                }
                let t = (q - p).cross(r) / den;
                let v = (q - p).cross(dir) / den;
                t > w.thickness && (0.0..=1.0).contains(&v)
            })
        };
        !hits(nrm) || !hits(-nrm)
    };
    // The swings drawn on the level (arcs and flattened arcs of a door's sweep and radius), for
    // the gaps to hang doors from.
    let swings = drawn_swings(&ents, k);
    for (wi, g0, g1, t) in gaps {
        let width = g0.dist(g1);
        let u = (g1 - g0) * (1.0 / width.max(1e-12));
        // A door swing (an arc, or a polyline of chords) hung from a jamb: its hinge there, its
        // leaf the gap wide (or half of it, a pair's). Not any curve about that far from it
        // (furniture, text, a fixture by a wide opening) (Q5273).
        let mut leaf = None;
        for sw in &swings {
            for jamb in [g0, g1] {
                let leaf_fits = (sw.r - width).abs() <= 0.2 * width + t || (sw.r - width / 2.0).abs() <= 0.1 * width + t;
                if sw.c.dist(jamb) <= t + 2.0 * k && leaf_fits {
                    // The open end: the end farthest from the wall line.
                    let open = if u.perp().dot(sw.ends[0] - jamb).abs() >= u.perp().dot(sw.ends[1] - jamb).abs() { sw.ends[0] } else { sw.ends[1] };
                    let along = if jamb == g0 { u } else { -u };
                    let inset = along * (LEAF_IN * k / 2.0);
                    leaf = Some((jamb + inset, jamb + inset + (open - jamb) * (width / open.dist(jamb).max(1e-12)), sw.handle, along));
                }
            }
        }
        // Covered by a door found from its swing already.
        let covered = pl.leaves.iter().any(|l| !l.window && (seg_dist(l.a, g0, g1) <= t || l.a.dist(g0) <= t || l.a.dist(g1) <= t));
        let Some(w) = pl.walls.get(wi) else { continue };
        let source = w.source;
        let is_outside = outside((g0 + g1) * 0.5, u.perp(), &pl.walls);
        let (z0, top) = (pl.z0, w.top);
        let mut extra = Vec::new();
        if leaf.is_some() || covered {
            extra.push((g0, g1, z0 + pl.door_head, top));
            if let Some((a, b, h, along)) = leaf.filter(|_| !covered) {
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
                    frame: Some((g0, g1)),
                });
                doors += 1;
            }
        } else if is_outside {
            // A wall meeting the gap end-on (a demising wall between two rooms' windows) parts
            // it: a pier the wall's width, full height, and a window each side of it.
            let mut piers: Vec<(f64, f64)> = pl
                .walls
                .iter()
                .enumerate()
                .filter(|(ci, c)| *ci != wi && u.cross(c.b - c.a).abs() > 0.5 * c.a.dist(c.b))
                .filter_map(|(_, c)| {
                    [c.a, c.b].into_iter().find_map(|e| {
                        let s = u.dot(e - g0);
                        (seg_dist(e, g0, g1) <= c.thickness / 2.0 + t && s > 0.1 * width && s < 0.9 * width)
                            .then(|| ((s - c.thickness / 2.0).max(0.0), (s + c.thickness / 2.0).min(width)))
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
                extra.push((g0 + u * *p0, g0 + u * *p1, z0, top));
                from = from.max(*p1);
            }
            spans.push((from, width));
            for (s0, s1) in spans {
                let (a, b) = (g0 + u * s0, g0 + u * s1);
                if s1 - s0 < MIN_WINDOW_IN * k {
                    extra.push((a, b, z0, top));
                    continue;
                }
                extra.push((a, b, z0, z0 + pl.sill));
                extra.push((a, b, z0 + pl.head, top));
                pl.leaves.push(Leaf {
                    a,
                    b,
                    window: true,
                    thickness: t,
                    source,
                    head: None,
                    sill: None,
                    along: None,
                    pair: None,
                    frame: Some((a, b)),
                });
                windows += 1;
            }
        } else if width <= 8.0 * upf {
            extra.push((g0, g1, z0 + pl.door_head, top));
        }
        if let Some(w) = pl.walls.get_mut(wi) {
            w.extra.extend(extra.into_iter().filter(|(_, _, lo, hi)| hi > lo));
        }
    }
    let (doors, windows) = one_per_swing(&mut pl.leaves, 6.0 * k).unwrap_or((doors, windows));
    let doors = doors.saturating_sub(one_door_per_opening(&mut pl.leaves, 6.0 * k));
    notes.push(format!("{}: {} walls from {} face pairs, {} doors, {} windows.", level.name, pl.walls.len(), found.len(), doors, windows));
    if not_doors > 0 {
        notes.push(format!("{}: {not_doors} arcs not taken for doors (no open wall where they shut: fixtures, equipment, furniture).", level.name));
    }
    pl
}

/// Whether a wall other than `i` and `j` (the two pieces either side of the gap `g0`–`g1`) runs
/// across the gap: its centreline crosses the gap's line between the gap's ends, away from them by
/// more than half its thickness (a wall meeting a jamb end-on is the jamb's return, not across).
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
        s > margin && s < width - margin && (0.0..=1.0).contains(&t)
    })
}

/// A window narrower than this (inches) beside a pier is no window: the pier's slot, filled.
const MIN_WINDOW_IN: f64 = 12.0;

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
        // A demising wall running across the gap's middle: two walls lined up, no opening.
        walls.push(plan_wall((136.0, -120.0), (136.0, 120.0)));
        assert!(gap_crossed(&walls, 0, 1, g0, g1));
    }
}
