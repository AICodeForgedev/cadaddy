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
const MAX_OBJECTS: usize = 200_000;

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
        )]
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
    BUILDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let scene = cadaddy_walk3d::derive(s.doc()?, &g.rules);
    let upf = if scene.units_per_foot.is_finite() && scene.units_per_foot > 0.0 { scene.units_per_foot } else { 12.0 };
    let gs = groups(&scene);
    // Walls from the plan's face pairs, level by level (levels drawn on this sheet).
    let mut notes = Vec::new();
    let mut planned: Vec<(usize, PlanLevel)> = Vec::new();
    for (i, level) in g.rules.levels.iter().enumerate() {
        if level.sheet.is_some() {
            continue;
        }
        let Some(si) = scene.levels.iter().position(|l| l.name == level.name) else { continue };
        let pl = plan_level(s.doc()?, g, level, upf, &mut notes);
        if !pl.walls.is_empty() {
            planned.push((si, pl));
        } else if i == 0 && g.rules.levels.len() == 1 && scene.tris.is_empty() {
            notes.push("Nothing to build.".into());
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
    for grp in gs.iter().take(MAX_OBJECTS) {
        // Levels built from the plan's pairs keep only Walk mode's floors and ceilings.
        if matches!(grp.part, Part::Wall | Part::Door | Part::Glass) && planned.iter().any(|(si, _)| *si == grp.level) {
            continue;
        }
        let Some(level) = scene.levels.get(grp.level) else { continue };
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
        if grp.part == Part::Floor {
            m = slab(&m, g.slab_ft.max(0.0) * upf);
        }
        let common = Common { layer: layer_of(grp.part).into(), xdata: tag(grp.part, &level.name, grp.source).into(), ..Common::default() };
        if d.add(&cadaddy_doc::Space::Model, common, EntityKind::Mesh(m)).is_ok() {
            *counts.entry(part_name(grp.part)).or_insert(0) += 1;
        }
    }
    let c = |k: &str| counts.get(k).copied().unwrap_or(0);
    Ok(json!({
        "walls": c("wall"), "doors": c("door"), "windows": c("window"), "floors": c("floor"), "ceilings": c("ceiling"),
        "levels": scene.levels.len(), "removed": removed, "notes": notes,
    }))
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
    // Rebuilt keeping the hand edits whose plan objects didn't change (those did: see
    // `model.revise`, which reports them).
    let changed = changed_sources(s, &g);
    if let Err(e) = super::revise3d::rebuild_keeping_edits(s, &g, &changed) {
        s.echo(e.to_string());
    }
}

/// The plan objects that changed since the model was last built (by handle and geometry).
fn changed_sources(s: &Session, g: &WallGen) -> std::collections::HashSet<Handle> {
    let (Ok(st), Ok(d)) = (s.state(), s.doc()) else { return Default::default() };
    let Some(rec) = &st.model_record else { return Default::default() };
    let old: Vec<_> = rec.plan.iter().filter(|i| i.kind != "room").cloned().collect();
    let now = super::revise3d::plan_items(d, g);
    let (added, removed, moved, _) = super::revise3d::diff(&old, &now, true, f64::MAX);
    added.iter().map(|i| i.handle).chain(removed.iter().map(|i| i.handle)).chain(moved.iter().flat_map(|(o, n)| [o.handle, n.handle])).collect()
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
                add_box(&mut m, l.a, l.b, LEAF_IN * self.k, lo, hi);
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
    let found = pairs(d, &segs, &lim);
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
    let snapshot = pl.walls.clone();
    for w in &mut pl.walls {
        let len = w.a.dist(w.b);
        if len <= 1e-9 {
            continue;
        }
        let u = (w.b - w.a) * (1.0 / len);
        let meets = |p: Vec2| {
            snapshot
                .iter()
                .filter(|o| (o.b - o.a).cross(u).abs() > 0.1 * o.a.dist(o.b))
                .map(|o| (seg_dist(p, o.a, o.b), o.thickness))
                .filter(|(dd, t)| *dd <= 1.5 * t.max(w.thickness))
                .map(|(_, t)| t)
                .fold(0.0, f64::max)
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
        // Tagged openings take their schedule row's size and heights.
        super::schedules::size_openings(sp, &mut ops, &mut fitted, upf);
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
    let (mut doors, mut windows) = (0, 0);
    for o in ops.iter().take(MAX_OBJECTS) {
        // The wall it is in: the hinge (or the block) on its centreline within its thickness.
        let best = pl
            .walls
            .iter()
            .enumerate()
            .filter_map(|(i, w)| {
                let len = w.a.dist(w.b);
                if len <= 1e-9 {
                    return None;
                }
                let u = (w.b - w.a) * (1.0 / len);
                let off = u.perp().dot(o.at - w.a).abs();
                let s = u.dot(o.at - w.a);
                (off <= w.thickness.max(2.0 * k) && s >= -o.width && s <= len + o.width).then_some((i, off))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let Some((wi, _)) = best else { continue };
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
                let frame = Some((w.a + u * s_at.min(s_end), w.a + u * s_at.max(s_end)));
                pl.leaves.push(Leaf {
                    a: o.at + inset,
                    b: open + inset,
                    window: false,
                    thickness: w.thickness,
                    source: o.handle,
                    head,
                    sill: None,
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
                pl.leaves.push(Leaf { a: leaf.0, b: leaf.1, window, thickness: w.thickness, source: o.handle, head, sill, frame: Some(leaf) });
            }
        }
    }
    // Gaps between two pieces of one wall line: a door where a swing hangs from a jamb (or a
    // door swing's opening covers it), a window in an outside wall, else a cased opening.
    let (min_gap, max_gap) = (oz.min_ft * upf, oz.max_ft * upf);
    let n = pl.walls.len().min(10_000);
    let mut gaps = Vec::new();
    for i in 0..n {
        for j in 0..n {
            let (Some(a), Some(b)) = (pl.walls.get(i), pl.walls.get(j)) else { continue };
            let la = a.a.dist(a.b);
            if i == j || la <= 1e-9 {
                continue;
            }
            let u = (a.b - a.a) * (1.0 / la);
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
            let between = pl.walls.iter().enumerate().any(|(k, c)| {
                k != i && k != j && u.cross(c.b - c.a).abs() <= 0.01 * c.a.dist(c.b) && u.perp().dot(c.a - a.a).abs() <= a.thickness / 2.0 && {
                    let (c0, c1) = (u.dot(c.a - a.a).min(u.dot(c.b - a.a)), u.dot(c.a - a.a).max(u.dot(c.b - a.a)));
                    c1 > la + 1e-6 && c0 < b0 - 1e-6
                }
            });
            if !between {
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
    for (wi, g0, g1, t) in gaps {
        let width = g0.dist(g1);
        let u = (g1 - g0) * (1.0 / width.max(1e-12));
        // A door swing (an arc, or a polyline of chords) hanging from a jamb, a gap wide.
        let mut leaf = None;
        for e in &ents {
            let pts: Vec<Vec2> = match &e.kind {
                EntityKind::Arc(a) => {
                    let c = a.center.xy();
                    vec![
                        c + Vec2::from_angle(a.start) * a.radius,
                        c + Vec2::from_angle((a.start + a.end) / 2.0) * a.radius,
                        c + Vec2::from_angle(a.end) * a.radius,
                    ]
                }
                EntityKind::LwPolyline(p) if p.vertices.len() >= 3 => p.vertices.iter().map(|v| v.p).collect(),
                _ => continue,
            };
            for jamb in [g0, g1] {
                let near = |p: &Vec2| (p.dist(jamb) - width).abs() <= 0.2 * width + t;
                if pts.iter().all(near) {
                    // The open end: the point farthest from the wall line.
                    let open =
                        pts.iter().copied().max_by(|a, b| u.perp().dot(*a - jamb).abs().total_cmp(&u.perp().dot(*b - jamb).abs())).unwrap_or(jamb);
                    let inset = (if jamb == g0 { u } else { -u }) * (LEAF_IN * k / 2.0);
                    leaf = Some((jamb + inset, jamb + inset + (open - jamb) * (width / open.dist(jamb).max(1e-12)), e.handle));
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
            if let Some((a, b, h)) = leaf.filter(|_| !covered) {
                pl.leaves.push(Leaf { a, b, window: false, thickness: t, source: h, head: None, sill: None, frame: Some((g0, g1)) });
                doors += 1;
            }
        } else if is_outside {
            extra.push((g0, g1, z0, z0 + pl.sill));
            extra.push((g0, g1, z0 + pl.head, top));
            pl.leaves.push(Leaf { a: g0, b: g1, window: true, thickness: t, source, head: None, sill: None, frame: Some((g0, g1)) });
            windows += 1;
        } else if width <= 8.0 * upf {
            extra.push((g0, g1, z0 + pl.door_head, top));
        }
        if let Some(w) = pl.walls.get_mut(wi) {
            w.extra.extend(extra.into_iter().filter(|(_, _, lo, hi)| hi > lo));
        }
    }
    let (doors, windows) = one_per_swing(&mut pl.leaves, 6.0 * k).unwrap_or((doors, windows));
    notes.push(format!("{}: {} walls from {} face pairs, {} doors, {} windows.", level.name, pl.walls.len(), found.len(), doors, windows));
    pl
}

/// Walls looked at for the building's outline, at most (a level; hostile drawings).
const MAX_CLUSTERED: usize = 20_000;

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
/// beside the building, the border and the title block.
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
    let mut b = cadaddy_geom::Bounds2::EMPTY;
    for (k, &i) in short.iter().enumerate() {
        let r = roots.get(k).copied().unwrap_or(k);
        if length.get(&r).copied().unwrap_or(0.0) >= 0.1 * largest
            && let Some(w) = walls.get(i)
        {
            b = b.union(&cadaddy_geom::Bounds2::from_points([w.a, w.b]));
        }
    }
    if b.is_empty() {
        return;
    }
    let inside = |p: Vec2, m: f64| p.x >= b.min.x - m && p.x <= b.max.x + m && p.y >= b.min.y - m && p.y <= b.max.y + m;
    walls.retain(|w| {
        let m = 2.0 * upf + w.thickness;
        inside(w.a, m) && inside(w.b, m)
    });
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
