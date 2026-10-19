//! WALLGEN: the building of a 2D plan as model-space 3D objects. Walk mode's reading of the plan
//! (`cadaddy-walk3d`: levels, layer rules, wall faces paired into solid walls, door and window
//! openings with heads and sills, floors and ceilings) becomes MESH objects on their own layers,
//! one per wall line, door leaf, window and slab, at the levels' elevations. It uses the same rules
//! file Walk mode does (`<drawing>.walk.json` beside the drawing) and, while associative, builds
//! again whenever an object on a rules layer changes.

use std::collections::HashMap;
use std::sync::Arc;

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, MeshKind, RawTag};
use cadaddy_geom::Vec3;
use cadaddy_walk3d::{Part, Rules, Scene};
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

/// The extended data application that marks what WALLGEN made (part, level, source object).
pub const APP: &str = "CADADDY_WALLGEN";
/// Floor slab thickness when none is given, feet.
const SLAB_FT: f64 = 0.5;
/// Most objects one WALLGEN makes.
const MAX_OBJECTS: usize = 200_000;

/// The settings of the last WALLGEN of a drawing: what it builds again from.
#[derive(Clone, Debug, PartialEq)]
pub struct WallGen {
    pub rules: Rules,
    /// Build again when the plan changes.
    pub auto: bool,
    pub slab_ft: f64,
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec::new("wallgen", "3D Building from Plan", run)
        .menu(&["Draw", "Modeling", "3D Building from Plan"])
        .params(
            "{rules?: walk rules (as in <drawing>.walk.json), rulesFile?: path, slabThickness?: feet (0.5), auto?: bool (default true: build again when the plan changes), clear?: true (remove what WALLGEN made)} → {walls, doors, windows, floors, ceilings, levels, removed}",
        )]
}

/// The layer each part goes on.
fn layer_of(part: Part) -> &'static str {
    match part {
        Part::Wall => "3D-Walls",
        Part::Floor => "3D-Floors",
        Part::Ceiling => "3D-Ceilings",
        Part::Glass => "3D-Windows",
        Part::Door => "3D-Doors",
        Part::Device => "3D-Devices",
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
    }
}

/// What WALLGEN made an object as: its part (`wall`, `floor`…), if it did.
pub fn made_as(e: &Entity) -> Option<String> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(APP))?;
    x.get(at + 1).filter(|t| t.code == 1000).map(|t| t.value.clone())
}

fn tag(part: Part, level: &str, source: Handle) -> Vec<RawTag> {
    let t = |code: i32, v: String| RawTag { code, value: v };
    vec![t(1001, APP.into()), t(1000, part_name(part).into()), t(1000, level.chars().take(255).collect()), t(1000, source.hex())]
}

/// The rules: given, from a file, beside the drawing, from the last WALLGEN, else Walk mode's
/// defaults.
fn rules_for(s: &Session, p: &Value) -> Result<Rules> {
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

/// The level a scene point (feet) belongs to: the one whose storey holds its height.
fn level_of(scene: &Scene, z: f64) -> usize {
    scene
        .levels
        .iter()
        .position(|l| z >= l.elevation_ft - 1e-3 && z <= l.elevation_ft + l.height_ft.max(0.0) + 1e-3)
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
        if *part == Part::Device {
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
fn remove_made(d: &mut Drawing) -> usize {
    let hs: Vec<Handle> = d.model.iter().filter(|e| made_as(e).is_some()).map(|e| e.handle).collect();
    for h in &hs {
        d.model.remove(*h);
    }
    hs.len()
}

/// Build the drawing's 3D building from `g`; the counts.
fn build(s: &mut Session, g: &WallGen) -> Result<Value> {
    let scene = cadaddy_walk3d::derive(s.doc()?, &g.rules);
    let upf = if scene.units_per_foot.is_finite() && scene.units_per_foot > 0.0 { scene.units_per_foot } else { 12.0 };
    let gs = groups(&scene);
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
    for grp in gs.iter().take(MAX_OBJECTS) {
        let Some(level) = scene.levels.get(grp.level) else { continue };
        let base = level.base;
        let to_sheet = |p: [f32; 3]| Vec3::new(base.x + f64::from(p[0]) * upf, base.y + f64::from(p[1]) * upf, f64::from(p[2]) * upf);
        let Some(mut m) = mesh(&grp.tris, &to_sheet) else { continue };
        if grp.part == Part::Floor {
            m = slab(&m, g.slab_ft.max(0.0) * upf);
        }
        let common = Common { layer: layer_of(grp.part).into(), xdata: tag(grp.part, &level.name, grp.source), ..Common::default() };
        if d.add(&cadaddy_doc::Space::Model, common, EntityKind::Mesh(m)).is_ok() {
            *counts.entry(part_name(grp.part)).or_insert(0) += 1;
        }
    }
    let c = |k: &str| counts.get(k).copied().unwrap_or(0);
    Ok(json!({
        "walls": c("wall"), "doors": c("door"), "windows": c("window"), "floors": c("floor"), "ceilings": c("ceiling"),
        "levels": scene.levels.len(), "removed": removed,
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
    let g = WallGen { rules, auto, slab_ft };
    let r = build(s, &g)?;
    s.state_mut()?.wallgen = Some(g);
    s.echo(format!(
        "3D building: {} walls, {} doors, {} windows, {} floors, {} ceilings on {} level(s).",
        r["walls"], r["doors"], r["windows"], r["floors"], r["ceilings"], r["levels"]
    ));
    Ok(r)
}

/// The objects on the rules' layers, by handle, as the drawing holds them (shared pointers: an
/// edit replaces the object's).
fn plan(d: &Drawing, rules: &Rules) -> Vec<(Handle, *const Entity)> {
    d.model.iter().filter(|e| made_as(e).is_none() && rules.layer(&e.common.layer).is_some()).map(|e| (e.handle, Arc::as_ptr(e))).collect()
}

/// After a command: when the drawing has an associative WALLGEN and an object on its rules'
/// layers changed, build again (in the same undo step).
pub(crate) fn after_command(s: &mut Session, before: Option<&Arc<Drawing>>) {
    let Some(before) = before else { return };
    let Ok(st) = s.state() else { return };
    let Some(g) = st.wallgen.clone().filter(|g| g.auto) else { return };
    if Arc::ptr_eq(before, &st.doc) || plan(before, &g.rules) == plan(&st.doc, &g.rules) {
        return;
    }
    if let Err(e) = build(s, &g) {
        s.echo(e.to_string());
    }
}
