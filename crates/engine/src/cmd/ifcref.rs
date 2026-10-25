//! IFC models as references for coordination: another program's architecture or structure,
//! attached beside our own MEP model so the two coordinate and clash.
//!
//! - `ifc.attach {path, name?, at?, rotation?, scale?, overlay?}`: the IFC file as an external
//!   reference (an overlay by default, so it never travels into drawings that reference this one).
//!   Its objects keep their IFC class (a layer per class, `NAME|IFC-Wall`), GlobalId, name, storey,
//!   type, systems and property sets (the io reader's `CADADDY_IFC` tag). The reference sits on its
//!   own layer `X-IFC-NAME` (turn it off to hide the whole model) at the drawing's scale: metres to
//!   drawing units from the building spec's units per foot, else INSUNITS.
//! - `ifc.refs`: the IFC references with their classes, storeys (matched to the spec's levels by
//!   name, else elevation) and whether the file changed since it was loaded.
//! - `ifc.elements {name?, class?, level?, guid?, system?, property?, limit?}`: the objects, with
//!   everything the file says about them.
//! - `ifc.reload {names?, changed_only?}`: load the files again; `changed_only` reloads only the
//!   ones whose size or time changed. The app checks every few seconds ([`poll`]) and reloads a
//!   reference whose file changed.
//! - `model.clashes` ([`super::mep`]) also reports MEP runs that cut into a reference object
//!   (`kind: reference`), with the object's class, GlobalId, name and storey.

use std::collections::{BTreeMap, HashMap};

use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Handle, Insert, XrefStatus};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::{Value, json};

use super::blocks::xref;
use super::*;
use crate::{Result, Session};

/// The extended data application the io reader tags IFC objects with.
pub const APP: &str = "CADADDY_IFC";
/// The layer an IFC reference called `name` sits on.
pub const LAYER_PREFIX: &str = "X-IFC-";
/// Classes left out of clashes unless asked for: volumes and markers, not things.
const SOFT_CLASSES: [&str; 6] = ["Space", "Zone", "SpatialZone", "Annotation", "Grid", "Site"];
/// Most reference triangles a clash run looks at (hostile or huge models).
const MAX_TRIANGLES: usize = 4_000_000;
/// Most elements `ifc.elements` lists by default, and at all.
const LIST_DEFAULT: usize = 500;
const LIST_MAX: usize = 100_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("ifc.attach", "Attach IFC Reference", run_attach)
            .params("{path: .ifc file (IFC4 or IFC2x3), name? (default: the file name), at?: [x, y, z?] (default 0,0,0), rotation?: degrees, scale?: drawing units per metre (default from the drawing's units), overlay?: bool (default true)} → {name, handle, file, scale, elements, classes: {class: count}, levels: [{name, elevation, elements, spec_level?}]}"),
        CommandSpec::new("ifc.refs", "IFC References", run_refs)
            .params("{} → {refs: [{name, path, status, changed, handles, scale, elements, classes, levels: [{name, elevation, elements, spec_level?}]}]}")
            .noundo(),
        CommandSpec::new("ifc.elements", "IFC Reference Elements", run_elements)
            .params("{name?: reference, or \"model\" (the model built in this drawing), class?: IFC class (Wall, Slab… or IfcWall), level?: storey name, guid?, system?, property?: \"Set.Name\" | \"Set.Name=value\", limit? (default 500)} → {elements: [{ref, handle, class, guid, name, level?, elevation?, type?, systems, properties: {\"Set.Name\": value}, min, max}], total, built (elements of the built model), export?}")
            .noundo(),
        CommandSpec::new("ifc.reload", "Reload IFC References", run_reload)
            .files()
            .params("{names?: [reference] | \"A,B*\" (default all), changed_only?: bool (only files changed since they were loaded; default false)} → {reloaded: [name], unchanged: [name], failed: [{name, reason}]}"),
    ]
}

// ---------------- what an IFC object carries ----------------

/// What the io reader tagged an object read from IFC with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tag {
    pub class: String,
    pub guid: String,
    pub name: String,
    pub level: Option<String>,
    /// The storey's elevation, metres.
    pub elevation: Option<f64>,
    pub ty: Option<String>,
    /// The space, building and site above the object (the IFC spatial structure).
    pub space: Option<String>,
    pub building: Option<String>,
    pub site: Option<String>,
    pub systems: Vec<String>,
    /// `(Set.Name, value)`, in the file's order.
    pub props: Vec<(String, String)>,
}

/// The IFC tag of `e`, when it was read from an IFC file.
pub fn tag(e: &Entity) -> Option<Tag> {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(APP))?;
    let mut strings = x.iter().skip(at + 1).take_while(|t| t.code != 1001).filter(|t| t.code == 1000).map(|t| t.value.as_str());
    let mut t = Tag { class: strings.next()?.to_string(), ..Tag::default() };
    t.guid = strings.next().unwrap_or_default().to_string();
    t.name = strings.next().unwrap_or_default().to_string();
    for s in strings {
        if let Some(v) = s.strip_prefix("level:") {
            t.level = Some(v.to_string());
        } else if let Some(v) = s.strip_prefix("elevation:") {
            t.elevation = v.parse::<f64>().ok().filter(|z| z.is_finite());
        } else if let Some(v) = s.strip_prefix("space:") {
            t.space = Some(v.to_string());
        } else if let Some(v) = s.strip_prefix("building:") {
            t.building = Some(v.to_string());
        } else if let Some(v) = s.strip_prefix("site:") {
            t.site = Some(v.to_string());
        } else if let Some(v) = s.strip_prefix("type:") {
            t.ty = Some(v.to_string());
        } else if let Some(v) = s.strip_prefix("system:") {
            t.systems.push(v.to_string());
        } else if let Some((k, v)) = s.strip_prefix("prop:").and_then(|p| p.split_once('=')) {
            t.props.push((k.to_string(), v.to_string()));
        }
    }
    Some(t)
}

/// `IfcWallStandardCase`, `IFCWALL` or `wall` → compared as the reader names classes.
pub(crate) fn class_matches(class: &str, asked: &str) -> bool {
    let bare = |s: &str| {
        let s = s.trim();
        if s.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("ifc")) { s.get(3..).unwrap_or_default().to_string() } else { s.to_string() }
    };
    let (c, a) = (bare(class), bare(asked));
    // Walls asked for include the standard-case walls IFC2x3 files are full of.
    c.eq_ignore_ascii_case(&a) || (a.eq_ignore_ascii_case("wall") && c.eq_ignore_ascii_case("wallstandardcase"))
}

// ---------------- the references in a drawing ----------------

/// Whether xref path `p` names an IFC file.
fn is_ifc_path(p: &str) -> bool {
    p.trim().rsplit('.').next().is_some_and(|e| e.eq_ignore_ascii_case("ifc"))
}

/// The IFC references attached to `d` (their block names).
pub fn ref_names(d: &Drawing) -> Vec<String> {
    d.blocks.values().filter(|b| !b.name.contains('|') && b.xref_path.as_deref().is_some_and(is_ifc_path)).map(|b| b.name.clone()).collect()
}

/// A reference object placed in the drawing: the reference's model-space insert, the object and
/// its tag, and how block coordinates map to the drawing's.
struct Placed<'a> {
    xref: &'a str,
    insert: Handle,
    e: &'a Entity,
    tag: Tag,
    place: Transform,
}

/// A block reference's block-to-drawing transform in 3D (plan scale and rotation, Z scaled).
#[derive(Clone, Copy)]
struct Transform {
    m: cadaddy_geom::Mat3,
    base_z: f64,
    at_z: f64,
    sz: f64,
}

impl Transform {
    fn of(ins: &Insert, base: Vec3) -> Self {
        Transform { m: ins.transform(base.xy()), base_z: base.z, at_z: ins.insert.z, sz: ins.scale.z }
    }
    fn apply(&self, v: Vec3) -> Vec3 {
        let p = self.m.apply(v.xy());
        Vec3::new(p.x, p.y, self.at_z + (v.z - self.base_z) * self.sz)
    }
}

/// Every object of every IFC reference shown in model space (first reference of each, the
/// array of an arrayed one isn't repeated).
fn placed(d: &Drawing) -> Vec<Placed<'_>> {
    let mut out = Vec::new();
    for ins_e in d.model.iter() {
        let EntityKind::Insert(ins) = &ins_e.kind else { continue };
        let Some(b) = d.block(&ins.block) else { continue };
        if !b.xref_path.as_deref().is_some_and(is_ifc_path) || b.xref_status != XrefStatus::Loaded {
            continue;
        }
        let place = Transform::of(ins, b.base);
        for e in b.entities.iter() {
            if let Some(tag) = tag(e) {
                out.push(Placed { xref: &b.name, insert: ins_e.handle, e, tag, place });
            }
        }
    }
    out
}

/// A placed object's mesh points in drawing coordinates.
fn world_points(p: &Placed) -> Vec<Vec3> {
    match &p.e.kind {
        EntityKind::Mesh(m) => m.vertices.iter().map(|v| p.place.apply(*v)).collect(),
        _ => Vec::new(),
    }
}

fn bounds3(pts: &[Vec3]) -> Option<(Vec3, Vec3)> {
    let first = *pts.first()?;
    Some(pts.iter().fold((first, first), |(lo, hi), v| {
        (Vec3::new(lo.x.min(v.x), lo.y.min(v.y), lo.z.min(v.z)), Vec3::new(hi.x.max(v.x), hi.y.max(v.y), hi.z.max(v.z)))
    }))
}

/// Drawing units per metre: from the building spec's units per foot, else INSUNITS, else
/// millimetres for a metric drawing and inches for an imperial one (as IFC export reads them).
pub fn units_per_metre(d: &Drawing) -> f64 {
    let upf = d.building_spec.as_deref().and_then(|t| cadaddy_walk3d::BuildingSpec::from_json(t).ok()).map(|sp| sp.units_per_foot);
    if let Some(upf) = upf.filter(|u| u.is_finite() && *u > 0.0) {
        return upf / 0.3048;
    }
    match d.header.i64("INSUNITS", 0) {
        0 if d.header.i64("MEASUREMENT", 1) == 1 => 1000.0,
        0 => 1.0 / 0.0254,
        u => 1.0 / cadaddy_doc::geo::metres_per_unit(u).max(1e-12),
    }
}

/// The building's levels (stored spec, else WALLGEN's rules): name and elevation in drawing units.
fn spec_levels(s: &Session) -> Vec<(String, f64)> {
    let Ok((sp, _)) = super::bspec::current(s, "ifc.refs") else { return Vec::new() };
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    sp.levels.iter().map(|l| (l.name.clone(), l.elevation_ft * upf)).collect()
}

/// The level an IFC storey stands for: the same name, else the one within half a foot of its
/// elevation (drawing units).
fn spec_level(levels: &[(String, f64)], upf: f64, name: &str, elevation: f64) -> Option<String> {
    if let Some((n, _)) = levels.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(name.trim())) {
        return Some(n.clone());
    }
    levels.iter().find(|(_, z)| (z - elevation).abs() <= 0.5 * upf).map(|(n, _)| n.clone())
}

/// Classes and storeys of reference `name`'s objects: `({class: n}, [{name, elevation, elements,
/// spec_level?}])`, storeys lowest first, elevations in drawing units.
fn summary(d: &Drawing, name: &str, spec: &[(String, f64)]) -> (BTreeMap<String, usize>, Vec<Value>) {
    let Some(b) = d.block(name) else { return (BTreeMap::new(), Vec::new()) };
    let scale = d
        .model
        .iter()
        .find_map(|e| match &e.kind {
            EntityKind::Insert(i) if i.block.eq_ignore_ascii_case(name) => Some((i.scale.z, i.insert.z)),
            _ => None,
        })
        .unwrap_or((1.0, 0.0));
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut levels: Vec<(String, f64, usize)> = Vec::new();
    for e in b.entities.iter() {
        let Some(t) = tag(e) else { continue };
        *classes.entry(t.class.clone()).or_default() += 1;
        if let Some(l) = t.level {
            let z = t.elevation.unwrap_or(0.0) * units_per_metre_of(b) * scale.0 + scale.1;
            match levels.iter_mut().find(|x| x.0 == l) {
                Some(x) => x.2 += 1,
                None => levels.push((l, z, 1)),
            }
        }
    }
    levels.sort_by(|a, b| a.1.total_cmp(&b.1));
    let levels = levels
        .into_iter()
        .map(|(n, z, count)| {
            let mut v = json!({"name": n, "elevation": r6(z), "elements": count});
            if let Some(l) = spec_level(spec, units_per_metre(d) * 0.3048, &n, z) {
                v["spec_level"] = json!(l);
            }
            v
        })
        .collect();
    (classes, levels)
}

/// Block units per metre of an IFC reference's own objects (the reader writes metres).
fn units_per_metre_of(b: &cadaddy_doc::Block) -> f64 {
    match b.units {
        0 | 6 => 1.0,
        u => 1.0 / cadaddy_doc::geo::metres_per_unit(i64::from(u)).max(1e-12),
    }
}

fn r6(v: f64) -> f64 {
    if v.is_finite() { (v * 1e6).round() / 1e6 } else { 0.0 }
}

// ---------------- file stamps (what changed on disk) ----------------

/// A referenced file's size and modification time, nanoseconds.
pub type Stamp = (u64, u128);

/// The stamp of the file xref `name` of the active drawing loads from, when it can be found.
fn stamp_of(s: &Session, name: &str) -> Option<Stamp> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let st = s.state().ok()?;
        let path = st.doc.block(name)?.xref_path.clone()?;
        let file = xref::load::find(&path, st.path.as_deref().map(std::path::Path::new), false).ok()?;
        let m = std::fs::metadata(file).ok()?;
        let t = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
        Some((m.len(), t))
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, name);
        None
    }
}

fn remember(s: &mut Session, name: &str) {
    let stamp = stamp_of(s, name);
    if let Ok(st) = s.state_mut() {
        match stamp {
            Some(t) => st.ifc_stamps.insert(name.to_ascii_uppercase(), t),
            None => st.ifc_stamps.remove(&name.to_ascii_uppercase()),
        };
    }
}

/// Whether reference `name`'s file changed since it was loaded (`None`: not known).
fn changed(s: &Session, name: &str) -> Option<bool> {
    let known = *s.state().ok()?.ifc_stamps.get(&name.to_ascii_uppercase())?;
    Some(stamp_of(s, name).is_some_and(|now| now != known))
}

// ---------------- commands ----------------

fn run_attach(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("ifc.attach", "`path` (an .ifc file) is required"))?;
    if !is_ifc_path(path) {
        return Err(bad("ifc.attach", format!("{path} is not an .ifc file")));
    }
    let path = path.to_string();
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).unwrap_or_else(|| {
        let file = path.rsplit(['/', '\\']).next().unwrap_or(&path);
        file.rsplit_once('.').map_or(file, |(stem, _)| stem).to_string()
    });
    let at = match p.get("at") {
        Some(v) => {
            let a = v.as_array().ok_or_else(|| bad("ifc.attach", "`at` must be [x, y] or [x, y, z]"))?;
            let n = |i: usize| a.get(i).and_then(Value::as_f64);
            match (n(0), n(1)) {
                (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Vec3::new(x, y, n(2).filter(|z| z.is_finite()).unwrap_or(0.0)),
                _ => return Err(bad("ifc.attach", "`at` must be [x, y] or [x, y, z]")),
            }
        }
        None => Vec3::default(),
    };
    let rotation = f64_or(p, "rotation", 0.0).to_radians();
    if !rotation.is_finite() {
        return Err(bad("ifc.attach", "`rotation` must be a number of degrees"));
    }
    let scale = match p.get("scale") {
        Some(v) => {
            v.as_f64().filter(|k| k.is_finite() && (1e-9..=1e9).contains(k)).ok_or_else(|| bad("ifc.attach", "`scale` must be a positive number"))?
        }
        None => units_per_metre(s.doc()?),
    };
    let overlay = bool_or(p, "overlay", true);
    let key = xref::attach_as(s, "ifc.attach", &name, &path, overlay)?;
    let layer = format!("{LAYER_PREFIX}{key}");
    let ins = Insert {
        block: key.clone(),
        insert: at,
        scale: Vec3::new(scale, scale, scale),
        rotation,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    };
    // In model space, on its own layer (made by `add` when new), whatever space is current.
    let h = s
        .doc_mut()?
        .add(&cadaddy_doc::Space::Model, Common { layer: layer.into(), ..Common::default() }, EntityKind::Insert(ins))
        .map_err(|e| bad("ifc.attach", e.to_string()))?;
    remember(s, &key);
    let spec = spec_levels(s);
    let d = s.doc()?;
    let (classes, levels) = summary(d, &key, &spec);
    let elements: usize = classes.values().sum();
    s.echo(format!("IFC reference \"{key}\": {elements} objects in {} classes on {} storeys.", classes.len(), levels.len()));
    Ok(json!({
        "name": key, "handle": h.hex(), "file": path, "scale": r6(scale),
        "elements": elements, "classes": classes, "levels": levels,
    }))
}

fn run_refs(s: &mut Session, _p: &Value) -> Result<Value> {
    let spec = spec_levels(s);
    let d = s.doc()?;
    let mut out = Vec::new();
    for name in ref_names(d) {
        let Some(b) = d.block(&name) else { continue };
        let handles: Vec<String> = d
            .model
            .iter()
            .filter(|e| matches!(&e.kind, EntityKind::Insert(i) if i.block.eq_ignore_ascii_case(&name)))
            .map(|e| e.handle.hex())
            .collect();
        let scale = d.model.iter().find_map(|e| match &e.kind {
            EntityKind::Insert(i) if i.block.eq_ignore_ascii_case(&name) => Some(i.scale.x),
            _ => None,
        });
        let (classes, levels) = summary(d, &name, &spec);
        out.push(json!({
            "name": name, "path": b.xref_path, "status": b.xref_status.label(),
            "reason": (!b.xref_reason.is_empty()).then_some(&b.xref_reason),
            "changed": changed(s, &name), "handles": handles, "scale": scale.map(r6),
            "elements": classes.values().sum::<usize>(), "classes": classes, "levels": levels,
        }));
    }
    Ok(json!({ "refs": out }))
}

fn run_elements(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let want = |k: &str| str_param(p, k).map(str::trim).filter(|t| !t.is_empty());
    let (name, class, level, guid, system, property) = (want("name"), want("class"), want("level"), want("guid"), want("system"), want("property"));
    if let Some(n) = name
        && !n.eq_ignore_ascii_case(MODEL_REF)
        && !ref_names(d).iter().any(|r| r.eq_ignore_ascii_case(n))
    {
        return Err(bad("ifc.elements", format!("no IFC reference named \"{n}\" (the built model is \"{MODEL_REF}\")")));
    }
    let limit = p.get("limit").and_then(Value::as_u64).map_or(LIST_DEFAULT, |l| usize::try_from(l).unwrap_or(LIST_MAX).min(LIST_MAX));
    let prop = property.map(|q| match q.split_once('=') {
        Some((k, v)) => (k.trim().to_string(), Some(v.trim().to_string())),
        None => (q.to_string(), None),
    });
    let mut total = 0usize;
    let mut out = Vec::new();
    for pl in placed(d) {
        let t = &pl.tag;
        let ok = name.is_none_or(|n| pl.xref.eq_ignore_ascii_case(n))
            && class.is_none_or(|c| class_matches(&t.class, c))
            && level.is_none_or(|l| t.level.as_deref().is_some_and(|x| x.trim().eq_ignore_ascii_case(l)))
            && guid.is_none_or(|g| t.guid == g)
            && system.is_none_or(|sy| t.systems.iter().any(|x| x.eq_ignore_ascii_case(sy)))
            && prop.as_ref().is_none_or(|(k, v)| {
                t.props.iter().any(|(pk, pv)| pk.eq_ignore_ascii_case(k) && v.as_ref().is_none_or(|v| pv.trim().eq_ignore_ascii_case(v)))
            });
        if !ok {
            continue;
        }
        total += 1;
        if out.len() >= limit {
            continue;
        }
        let (lo, hi) = bounds3(&world_points(&pl)).unwrap_or_default();
        let props: serde_json::Map<String, Value> = t.props.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        out.push(json!({
            "ref": pl.xref, "handle": pl.e.handle.hex(), "insert": pl.insert.hex(), "class": t.class, "guid": t.guid, "name": t.name,
            "level": t.level, "elevation": t.elevation, "type": t.ty, "systems": t.systems, "properties": props,
            "min": [r6(lo.x), r6(lo.y), r6(lo.z)], "max": [r6(hi.x), r6(hi.y), r6(hi.z)],
        }));
    }
    // The model built here (walls, slabs, doors, runs…) as the IFC elements an export writes.
    let mut built = 0usize;
    if name.is_none_or(|n| n.eq_ignore_ascii_case(MODEL_REF)) && guid.is_none() && property.is_none() {
        for e in d.model.iter().filter(|e| super::wallgen::made_as(e).is_some()) {
            let EntityKind::Mesh(m) = &e.kind else { continue };
            let info = built_element(e);
            let ok = class.is_none_or(|c| class_matches(&info.class, c))
                && level.is_none_or(|l| info.level.as_deref().is_some_and(|x| x.trim().eq_ignore_ascii_case(l)))
                && system.is_none_or(|sy| info.systems.iter().any(|x| x.eq_ignore_ascii_case(sy)));
            if !ok {
                continue;
            }
            total += 1;
            built += 1;
            if out.len() >= limit {
                continue;
            }
            let (lo, hi) = bounds3(&m.vertices).unwrap_or_default();
            out.push(json!({
                "ref": MODEL_REF, "handle": e.handle.hex(), "class": info.class, "guid": "", "name": info.name, "level": info.level,
                "systems": info.systems, "properties": {}, "min": [r6(lo.x), r6(lo.y), r6(lo.z)], "max": [r6(hi.x), r6(hi.y), r6(hi.z)],
            }));
        }
    }
    let mut r = json!({ "elements": out, "total": total, "built": built });
    if built > 0 {
        r["export"] = json!("export {path: model.ifc} writes the built model as IFC4");
    }
    Ok(r)
}

/// The name `ifc.elements` gives the model built in the drawing (not an attached reference).
pub const MODEL_REF: &str = "model";

/// A built object as an IFC element: its class (as an IFC export writes it, named as the reader
/// names classes: `Wall` for IfcWall), level, name and
/// systems.
struct BuiltElement {
    class: String,
    level: Option<String>,
    name: String,
    systems: Vec<String>,
}

fn built_element(e: &Entity) -> BuiltElement {
    let x = &e.common.xdata;
    let at = x.iter().position(|t| t.code == 1001 && t.value.trim().eq_ignore_ascii_case(super::wallgen::APP)).unwrap_or(0);
    let tags: Vec<&str> = x.iter().skip(at + 1).take_while(|t| t.code != 1001).map(|t| t.value.as_str()).collect();
    let part = tags.first().copied().unwrap_or_default();
    let extra = |key: &str| tags.iter().skip(3).find_map(|v| v.strip_prefix(key));
    let class = match part {
        "wall" | "parapet" => "Wall",
        "floor" => "Slab",
        "ceiling" | "soffit" => "Covering",
        "door" => "Door",
        "window" | "glass" => "Window",
        "stair" => "Stair",
        "column" => "Column",
        "curtainwall" => "CurtainWall",
        "railing" => "Railing",
        "ramp" => "Ramp",
        "roof" => "Roof",
        "ground" => "GeographicElement",
        "concrete" => "Footing",
        _ => match (extra("kind:"), extra("part:").unwrap_or("segment")) {
            (Some("equipment"), _) => "FlowTerminal",
            (Some("duct"), p) if p.starts_with("fitting") => "DuctFitting",
            (Some("pipe"), p) if p.starts_with("fitting") => "PipeFitting",
            (Some("conduit"), p) if p.starts_with("fitting") => "CableCarrierFitting",
            (_, p) if p.starts_with("terminal") => "FlowTerminal",
            (Some("duct"), _) => "DuctSegment",
            (Some("pipe"), _) => "PipeSegment",
            (Some("conduit"), _) => "CableCarrierSegment",
            _ => "BuildingElementProxy",
        },
    };
    let systems: Vec<String> = tags.iter().skip(3).filter_map(|v| v.strip_prefix("system:")).map(str::to_string).collect();
    let level = tags.get(1).map(|l| l.to_string()).filter(|l| !l.is_empty());
    let name = extra("room:")
        .filter(|r| !r.is_empty())
        .or_else(|| extra("type:"))
        .or_else(|| extra("tag:"))
        .map_or_else(|| part.to_string(), str::to_string);
    BuiltElement { class: class.to_string(), level, name, systems }
}

fn run_reload(s: &mut Session, p: &Value) -> Result<Value> {
    let all = ref_names(s.doc()?);
    let names: Vec<String> = match p.get("names") {
        Some(Value::Array(a)) => {
            let pats: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
            all.into_iter().filter(|n| pats.iter().any(|q| crate::cmd::qselect::wildcard(q, n))).collect()
        }
        Some(Value::String(t)) => {
            let pats: Vec<&str> = t.split(',').map(str::trim).filter(|q| !q.is_empty()).collect();
            all.into_iter().filter(|n| pats.iter().any(|q| crate::cmd::qselect::wildcard(q, n))).collect()
        }
        _ => all,
    };
    let changed_only = bool_or(p, "changed_only", false);
    let (mut reloaded, mut unchanged, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    let mut d: Option<Drawing> = None;
    let host = s.state()?.path.clone();
    for n in names {
        if changed_only && changed(s, &n) != Some(true) {
            unchanged.push(n);
            continue;
        }
        let doc = d.get_or_insert_with(|| s.doc().map(|x| (*x).clone()).unwrap_or_default());
        let out = xref::load::load(doc, &n, host.as_deref(), false);
        match out.first() {
            Some(o) if o.status == XrefStatus::Loaded => reloaded.push(n),
            Some(o) => failed.push(json!({"name": n, "reason": o.detail})),
            None => failed.push(json!({"name": n, "reason": "not loaded"})),
        }
    }
    if let Some(d) = d {
        *s.doc_mut()? = d;
    }
    for n in reloaded.iter().chain(failed.iter().filter_map(|f| f["name"].as_str().map(str::to_string)).collect::<Vec<_>>().iter()) {
        remember(s, n);
    }
    for n in &reloaded {
        s.echo(format!("IFC reference \"{n}\" reloaded."));
    }
    for f in &failed {
        s.echo(format!("IFC reference \"{}\" not reloaded: {}.", f["name"].as_str().unwrap_or_default(), f["reason"].as_str().unwrap_or_default()));
    }
    Ok(json!({ "reloaded": reloaded, "unchanged": unchanged, "failed": failed }))
}

/// The app's check (every few seconds, with the table data links): when no command is running, reload the active
/// drawing's IFC references whose files changed (a reference loaded before its stamp was known
/// only has it noted). Returns the references reloaded.
pub fn poll(s: &mut Session) -> Vec<String> {
    if s.running.is_some() {
        return Vec::new();
    }
    let Ok(d) = s.doc() else { return Vec::new() };
    let names = ref_names(d);
    if names.is_empty() {
        return Vec::new();
    }
    let mut stale = Vec::new();
    for n in names {
        match changed(s, &n) {
            Some(true) => stale.push(n),
            Some(false) => {}
            None => remember(s, &n),
        }
    }
    if stale.is_empty() {
        return Vec::new();
    }
    match s.execute("ifc.reload", &json!({ "names": stale, "changed_only": true })) {
        Ok(r) => r["reloaded"].as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

// ---------------- clashes against references ----------------

/// One straight piece of an MEP run for clashing, in building coordinates (drawing units).
pub(crate) struct RunPiece<'a> {
    pub system: &'a str,
    pub run: usize,
    pub a: Vec2,
    pub b: Vec2,
    pub z0: f64,
    pub z1: f64,
    /// Width across the run (round: the diameter).
    pub w: f64,
    pub handles: Vec<String>,
    /// Add to a building point to land on the run's sheet (the level's plan base).
    pub base: Vec2,
}

/// A reference object ready to clash: its triangles and box in drawing coordinates.
struct Solid {
    xref: String,
    insert: Handle,
    tag: Tag,
    tris: Vec<[Vec3; 3]>,
    lo: Vec3,
    hi: Vec3,
}

/// An oriented box: centre, axes and half sizes.
struct Obb {
    c: Vec3,
    u: [Vec3; 3],
    e: [f64; 3],
}

/// Whether triangle `t` cuts into box `b` (the separating axis test; touching isn't cutting).
fn tri_hits_box(t: &[Vec3; 3], b: &Obb) -> bool {
    const EPS: f64 = 1e-7;
    let local = |p: Vec3| {
        let d = p - b.c;
        Vec3::new(d.dot(b.u[0]), d.dot(b.u[1]), d.dot(b.u[2]))
    };
    let v = [local(t[0]), local(t[1]), local(t[2])];
    let comp = |p: Vec3, i: usize| match i {
        0 => p.x,
        1 => p.y,
        _ => p.z,
    };
    let separated = |axis: Vec3| -> bool {
        let len = axis.len();
        if len < 1e-12 || !len.is_finite() {
            return false;
        }
        let r = b.e[0] * axis.x.abs() + b.e[1] * axis.y.abs() + b.e[2] * axis.z.abs();
        let p: Vec<f64> = v.iter().map(|q| q.dot(axis)).collect();
        let (mn, mx) = p.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, z), x| (a.min(*x), z.max(*x)));
        mn >= r - EPS * len || mx <= -r + EPS * len
    };
    for i in 0..3 {
        let (mn, mx) = v.iter().map(|q| comp(*q, i)).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, z), x| (a.min(x), z.max(x)));
        if mn >= b.e[i] - EPS || mx <= -b.e[i] + EPS {
            return false;
        }
    }
    let edges = [v[1] - v[0], v[2] - v[1], v[0] - v[2]];
    if separated(edges[0].cross(edges[1])) {
        return false;
    }
    let axes = [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)];
    for a in axes {
        for e in edges {
            if separated(a.cross(e)) {
                return false;
            }
        }
    }
    true
}

/// Whether `p` is inside the closed surface `tris` (crossings of a slanted ray, counted).
fn inside(tris: &[[Vec3; 3]], p: Vec3) -> bool {
    let dir = Vec3::new(0.5773, 0.5774, 0.5775);
    let mut n = 0usize;
    for t in tris {
        let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
        let h = dir.cross(e2);
        let a = e1.dot(h);
        if a.abs() < 1e-12 {
            continue;
        }
        let f = 1.0 / a;
        let s = p - t[0];
        let u = f * s.dot(h);
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = s.cross(e1);
        let w = f * dir.dot(q);
        if w < 0.0 || u + w > 1.0 {
            continue;
        }
        if f * e2.dot(q) > 1e-9 {
            n += 1;
        }
    }
    n % 2 == 1
}

/// The reference objects to clash against, without the `ignore` classes.
fn solids(d: &Drawing, ignore: &[String]) -> Vec<Solid> {
    let mut out = Vec::new();
    let mut budget = MAX_TRIANGLES;
    for pl in placed(d) {
        if ignore.iter().any(|c| class_matches(&pl.tag.class, c)) {
            continue;
        }
        let EntityKind::Mesh(m) = &pl.e.kind else { continue };
        let pts: Vec<Vec3> = m.vertices.iter().map(|v| pl.place.apply(*v)).collect();
        let mut tris = Vec::new();
        for f in &m.faces {
            let idx: Vec<Vec3> = f.v.iter().filter_map(|i| pts.get(*i as usize).copied()).collect();
            if idx.len() < 3 || idx.len() != f.v.len() {
                continue;
            }
            for k in 1..idx.len() - 1 {
                if let (Some(a), Some(b), Some(c)) = (idx.first(), idx.get(k), idx.get(k + 1)) {
                    tris.push([*a, *b, *c]);
                }
            }
        }
        if tris.len() > budget {
            break;
        }
        budget -= tris.len();
        let Some((lo, hi)) = bounds3(&pts) else { continue };
        out.push(Solid { xref: pl.xref.to_string(), insert: pl.insert, tag: pl.tag, tris, lo, hi });
    }
    out
}

/// MEP pieces cutting into reference objects: one clash per run and object, as `model.clashes`
/// lists them (`kind: reference`). `tol` (drawing units) is the overlap forgiven; `ignore` the
/// classes left out (default [`SOFT_CLASSES`]).
pub(crate) fn clashes(d: &Drawing, pieces: &[RunPiece], tol: f64, upf: f64, ignore: Option<&[String]>) -> Vec<Value> {
    let soft: Vec<String> = SOFT_CLASSES.iter().map(|c| (*c).to_string()).collect();
    let solids = solids(d, ignore.unwrap_or(&soft));
    if solids.is_empty() || pieces.is_empty() {
        return Vec::new();
    }
    // A plan grid over the objects' boxes, so each piece looks only at its neighbours.
    let (lo, hi) = solids.iter().fold((Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY)), |(lo, hi), s| {
        (Vec2::new(lo.x.min(s.lo.x), lo.y.min(s.lo.y)), Vec2::new(hi.x.max(s.hi.x), hi.y.max(s.hi.y)))
    });
    let span = (hi.x - lo.x).max(hi.y - lo.y).max(1e-6);
    let cell = (span / 256.0).max(upf.max(1e-6));
    let key = |x: f64, y: f64| -> (i64, i64) {
        let k = |v: f64, o: f64| ((v - o) / cell).floor().clamp(-1e9, 1e9) as i64;
        (k(x, lo.x), k(y, lo.y))
    };
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    let mut big: Vec<usize> = Vec::new();
    for (i, s) in solids.iter().enumerate() {
        let (a, b) = (key(s.lo.x, s.lo.y), key(s.hi.x, s.hi.y));
        let cells = (b.0 - a.0 + 1).saturating_mul(b.1 - a.1 + 1);
        if cells > 4096 {
            big.push(i);
            continue;
        }
        for x in a.0..=b.0 {
            for y in a.1..=b.1 {
                grid.entry((x, y)).or_default().push(i);
            }
        }
    }
    let mut seen: std::collections::HashSet<(usize, usize)> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for pc in pieces {
        let len = pc.a.dist(pc.b);
        if len < 1e-9 || !len.is_finite() {
            continue;
        }
        let dir = (pc.b - pc.a) * (1.0 / len);
        let half = [len / 2.0 - tol, pc.w / 2.0 - tol, (pc.z1 - pc.z0) / 2.0 - tol];
        if half.iter().any(|h| *h <= 0.0 || !h.is_finite()) {
            continue;
        }
        let mid = (pc.a + pc.b) * 0.5;
        let obb = Obb {
            c: Vec3::new(mid.x, mid.y, (pc.z0 + pc.z1) / 2.0),
            u: [Vec3::new(dir.x, dir.y, 0.0), Vec3::new(-dir.y, dir.x, 0.0), Vec3::new(0.0, 0.0, 1.0)],
            e: half,
        };
        // The piece's box in the drawing's axes.
        let ex = Vec2::new(dir.x.abs() * half[0] + dir.y.abs() * half[1], dir.y.abs() * half[0] + dir.x.abs() * half[1]);
        let (plo, phi) = (Vec3::new(mid.x - ex.x, mid.y - ex.y, obb.c.z - half[2]), Vec3::new(mid.x + ex.x, mid.y + ex.y, obb.c.z + half[2]));
        let (a, b) = (key(plo.x, plo.y), key(phi.x, phi.y));
        let mut near: Vec<usize> = big.clone();
        if (b.0 - a.0 + 1).saturating_mul(b.1 - a.1 + 1) <= 65_536 {
            for x in a.0..=b.0 {
                for y in a.1..=b.1 {
                    if let Some(l) = grid.get(&(x, y)) {
                        near.extend(l);
                    }
                }
            }
        } else {
            near = (0..solids.len()).collect();
        }
        near.sort_unstable();
        near.dedup();
        for i in near {
            let Some(so) = solids.get(i) else { continue };
            if seen.contains(&(pc.run, i)) {
                continue;
            }
            let overlap = [phi.x.min(so.hi.x) - plo.x.max(so.lo.x), phi.y.min(so.hi.y) - plo.y.max(so.lo.y), phi.z.min(so.hi.z) - plo.z.max(so.lo.z)];
            if overlap.iter().any(|o| *o <= 0.0) {
                continue;
            }
            let hit = so.tris.iter().any(|t| tri_hits_box(t, &obb)) || inside(&so.tris, obb.c);
            if !hit {
                continue;
            }
            seen.insert((pc.run, i));
            let at = Vec2::new((phi.x.min(so.hi.x) + plo.x.max(so.lo.x)) / 2.0, (phi.y.min(so.hi.y) + plo.y.max(so.lo.y)) / 2.0) + pc.base;
            let by = overlap.iter().copied().fold(f64::INFINITY, f64::min) / upf * 12.0;
            let what = if so.tag.name.is_empty() { so.tag.class.clone() } else { format!("{} \"{}\"", so.tag.class, so.tag.name) };
            let mut handles = pc.handles.clone();
            handles.push(so.insert.hex());
            out.push(json!({
                "kind": "reference", "systems": [pc.system],
                "message": format!("{} runs into {what} of reference {}", pc.system, so.xref),
                "at": [r3(at.x), r3(at.y)], "by_in": r3(by), "handles": handles,
                "reference": {"ref": so.xref, "class": so.tag.class, "guid": so.tag.guid, "name": so.tag.name, "level": so.tag.level},
            }));
            if out.len() >= 5_000 {
                return out;
            }
        }
    }
    out
}

fn r3(v: f64) -> f64 {
    if v.is_finite() { (v * 1000.0).round() / 1000.0 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_box() -> Obb {
        Obb { c: Vec3::default(), u: [Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 0.0, 1.0)], e: [1.0, 1.0, 1.0] }
    }

    #[test]
    fn triangles_cut_boxes_and_touching_isnt_cutting() {
        let b = unit_box();
        let t = |a: (f64, f64, f64), b: (f64, f64, f64), c: (f64, f64, f64)| {
            [Vec3::new(a.0, a.1, a.2), Vec3::new(b.0, b.1, b.2), Vec3::new(c.0, c.1, c.2)]
        };
        // Through the middle.
        assert!(tri_hits_box(&t((-5.0, -5.0, 0.0), (5.0, -5.0, 0.0), (0.0, 5.0, 0.0)), &b));
        // Lying on the top face: touching.
        assert!(!tri_hits_box(&t((-5.0, -5.0, 1.0), (5.0, -5.0, 1.0), (0.0, 5.0, 1.0)), &b));
        // Beside it.
        assert!(!tri_hits_box(&t((2.0, 0.0, 0.0), (3.0, 0.0, 0.0), (2.0, 1.0, 0.0)), &b));
        // Near a corner but cut off by an edge axis: the plane x + y = 2.5 misses the box.
        assert!(!tri_hits_box(&t((2.5, 0.0, -5.0), (0.0, 2.5, -5.0), (1.25, 1.25, 5.0)), &b));
        // Degenerate triangles never panic.
        let _ = tri_hits_box(&t((0.0, 0.0, 0.0), (0.0, 0.0, 0.0), (0.0, 0.0, 0.0)), &b);
        let _ = tri_hits_box(&t((f64::NAN, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)), &b);
    }

    #[test]
    fn points_inside_closed_surfaces() {
        // A 2-unit cube's faces as triangles.
        let v = |x: f64, y: f64, z: f64| Vec3::new(x, y, z);
        let c = [v(0., 0., 0.), v(2., 0., 0.), v(2., 2., 0.), v(0., 2., 0.), v(0., 0., 2.), v(2., 0., 2.), v(2., 2., 2.), v(0., 2., 2.)];
        let quads = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]];
        let tris: Vec<[Vec3; 3]> = quads.iter().flat_map(|q| [[c[q[0]], c[q[1]], c[q[2]]], [c[q[0]], c[q[2]], c[q[3]]]]).collect();
        assert!(inside(&tris, v(1.0, 1.0, 1.0)));
        assert!(inside(&tris, v(0.1, 1.9, 0.1)));
        assert!(!inside(&tris, v(3.0, 1.0, 1.0)));
        assert!(!inside(&tris, v(1.0, 1.0, -0.5)));
    }

    #[test]
    fn classes_match_with_or_without_the_ifc_prefix() {
        assert!(class_matches("Wall", "IfcWall"));
        assert!(class_matches("WallStandardCase", "wall"));
        assert!(class_matches("Slab", "IFCSLAB"));
        assert!(!class_matches("Slab", "Wall"));
        assert!(is_ifc_path("models/Structure.IFC") && !is_ifc_path("plan.dwg"));
    }
}
