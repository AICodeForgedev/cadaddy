//! IFC models opened as models (not drawings): the objects of an IFC4 or IFC2x3 file, each with
//! everything the file says about it (the io reader's `CADADDY_IFC` tag: class, GlobalId, name,
//! site, building, storey, space, type, systems, property and quantity sets).
//!
//! - `ifc.open {path}`: open the file and look at it as a model (a 3D view, shaded).
//! - `ifc.tree`: the spatial structure, site → building → storey → space → class → objects.
//! - `ifc.properties {handle? | guid?}`: one object's property and quantity sets, grouped.
//! - `ifc.filter {class?, level?, space?, system?, type?, property?, name?, action?}`: the objects
//!   that match, listed, selected, isolated or hidden.
//! - `ifc.sectionbox {on?, min?, max?, fit?, pad?}`: only what is in the box shows (objects wholly
//!   outside it are hidden; `ifc.render` and BCF snapshots cut exactly at the box).
//! - `ifc.measure {from, to} | {a, b}`: a distance in 3D, between two points or two objects.
//! - `ifc.camera {eye, target}`: look from a place (BCF viewpoints restore through it).
//! - `ifc.render {path?, …}`: the model as it shows (hidden objects out, the selection marked, cut
//!   at the section box), as a PNG.
//! - Walk ([`walk_scene`]): the model as a scene to walk through, storeys as levels.
//!
//! BCF issues on the model are [`super::bcf`].

use std::collections::{BTreeMap, BTreeSet};

use cadaddy_color::Rgb;
use cadaddy_doc::{Drawing, Entity, EntityKind, Handle, Space};
use cadaddy_geom::Vec3;
use serde_json::{Value, json};

use super::ifcref::{self, Tag};
use super::*;
use crate::{Result, Session};

/// Most objects one tree node or a filter lists by default, and at all.
const LIST_DEFAULT: usize = 200;
const LIST_MAX: usize = 100_000;
/// Most objects a model holds that these commands look at (hostile files).
const MAX_ELEMENTS: usize = 2_000_000;
/// The colour the selection is marked in (renders and snapshots).
pub const SELECTED: Rgb = Rgb(255, 140, 0);

/// The viewer's state of one open drawing (not saved with it).
#[derive(Default)]
pub struct ViewState {
    /// The section box (drawing units, min and max corners).
    pub section: Option<(Vec3, Vec3)>,
    /// The objects the section box hid (shown again when it goes off or moves).
    pub section_hidden: Vec<Handle>,
    /// The camera last set (`ifc.camera`, a BCF viewpoint): eye and target, drawing units.
    pub camera: Option<(Vec3, Vec3)>,
    /// Cached element index: (revision, index). Rebuilt when the drawing revision changes.
    element_cache: Option<(u64, std::sync::Arc<ElementIndex>)>,
}

impl Clone for ViewState {
    fn clone(&self) -> Self {
        Self { section: self.section, section_hidden: self.section_hidden.clone(), camera: self.camera, element_cache: self.element_cache.clone() }
    }
}

impl std::fmt::Debug for ViewState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewState")
            .field("section", &self.section)
            .field("section_hidden", &self.section_hidden)
            .field("camera", &self.camera)
            .field("element_cache", &self.element_cache.as_ref().map(|(r, _)| r))
            .finish()
    }
}

impl PartialEq for ViewState {
    fn eq(&self, other: &Self) -> bool {
        self.section == other.section && self.section_hidden == other.section_hidden && self.camera == other.camera
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("ifc.open", "Open IFC Model", run_open)
            .menu(&["File", "Open IFC Model..."])
            .enabled(always)
            .params("{path: .ifc file (IFC4 or IFC2x3)} → {name, elements, classes: {class: count}, storeys: [name], sites, buildings} — opens it as a model: a 3D view, shaded")
            .noundo(),
        CommandSpec::new("ifc.tree", "IFC Model Tree", run_tree)
            .params("{limit?: objects listed per class (default 200)} → {tree: [{kind: site|building|storey|space|class, name, elevation?, count, children?, elements?: [{handle, guid, name}]}], elements}")
            .noundo(),
        CommandSpec::new("ifc.properties", "IFC Object Properties", run_properties)
            .params("{handle? | guid? (default: the one selected object)} → {handle, class, guid, name, type?, site?, building?, storey?, elevation?, space?, systems, property_sets: {set: {name: value}}, quantity_sets: {set: {name: value}}, min, max, size}")
            .noundo(),
        CommandSpec::new("ifc.filter", "Filter IFC Objects", run_filter)
            .params("{class?, level? (storey), space?, system?, type?, property?: \"Set.Name\" | \"Set.Name=value\", name?: part of the name, guids?: [GlobalId], action?: list|select|isolate|hide|show (show: every object again; default list), limit? (default 200)} → {matched, handles, action, hidden?}")
            .noundo(),
        CommandSpec::new("ifc.sectionbox", "IFC Section Box", run_sectionbox)
            .params("{on?: bool, min?: [x,y,z], max?: [x,y,z], fit?: selection|all, pad?: drawing units (default 1 ft)} → {on, min?, max?, hidden}")
            .noundo(),
        CommandSpec::new("ifc.measure", "Measure in 3D", run_measure)
            .params("{from: [x,y,z], to: [x,y,z]} | {a, b: handle or GlobalId} (default the two selected objects) → {distance, dx, dy, dz, metres; objects: centres, gap (between their boxes)}")
            .noundo(),
        CommandSpec::new("ifc.camera", "IFC Camera", run_camera)
            .params("{eye: [x,y,z], target: [x,y,z]} | {fit: selection|all, dir?: [x,y,z]} → {eye, target} — looks from `eye` at `target` (the view direction and the zoom)")
            .noundo(),
        CommandSpec::new("ifc.render", "Render IFC Model", run_render)
            .params("{path?: PNG file (else base64), width?, height? (800×600), style?: shaded|shadedwithedges|hidden|wireframe|realistic, eye?, target?, dir? (default the camera last set, else the view), highlight?: bool (the selection in orange, default true)} → {path?|png, width, height, triangles}")
            .noundo(),
    ]
}

/// One object of the model: its handle, tag and box (drawing units).
#[derive(Clone, Debug)]
pub struct Element {
    pub handle: Handle,
    pub tag: Tag,
    pub min: Vec3,
    pub max: Vec3,
}

impl Element {
    fn centre(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
}

fn bounds(pts: &[Vec3]) -> Option<(Vec3, Vec3)> {
    let mut it = pts.iter().filter(|p| p.x.is_finite() && p.y.is_finite() && p.z.is_finite());
    let first = *it.next()?;
    Some(it.fold((first, first), |(lo, hi), p| {
        (Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z)), Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z)))
    }))
}

/// Bounding box of an entity's mesh by iterating vertices (no clone).
fn entity_bounds(e: &Entity) -> Option<(Vec3, Vec3)> {
    match &e.kind {
        EntityKind::Mesh(m) => bounds(&m.vertices),
        _ => None,
    }
}

/// The IFC objects in model space (read from an IFC file), in drawing order.
pub fn elements(d: &Drawing) -> Vec<Element> {
    ELEMENTS_CALL_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    d.model
        .iter()
        .take(MAX_ELEMENTS)
        .filter_map(|e| {
            let tag = ifcref::tag(e)?;
            let (min, max) = entity_bounds(e).unwrap_or_default();
            Some(Element { handle: e.handle, tag, min, max })
        })
        .collect()
}

/// A cached element index: elements, a GUID→index map and a Handle→index map, keyed on drawing
/// revision. Avoids rebuilding elements() and doing linear lookups every frame / every command.
pub struct ElementIndex {
    pub els: Vec<Element>,
    guid_map: std::collections::HashMap<String, usize>,
    handle_map: std::collections::HashMap<Handle, usize>,
}

impl ElementIndex {
    /// Build the index from a drawing (calls `elements()` once).
    pub fn build(d: &Drawing) -> Self {
        let els = elements(d);
        let mut guid_map = std::collections::HashMap::with_capacity(els.len());
        let mut handle_map = std::collections::HashMap::with_capacity(els.len());
        for (i, e) in els.iter().enumerate() {
            if !e.tag.guid.is_empty() {
                guid_map.insert(e.tag.guid.clone(), i);
            }
            handle_map.insert(e.handle, i);
        }
        Self { els, guid_map, handle_map }
    }

    /// Look up by GlobalId.
    pub fn by_guid(&self, guid: &str) -> Option<usize> {
        self.guid_map.get(guid).copied()
    }

    /// Look up by Handle.
    pub fn by_handle(&self, h: Handle) -> Option<usize> {
        self.handle_map.get(&h).copied()
    }

    /// Look up by either GlobalId or Handle hex string.
    pub fn by_guid_or_handle(&self, q: &str) -> Option<usize> {
        self.by_guid(q).or_else(|| Handle::parse_hex(q).and_then(|h| self.by_handle(h)))
    }
}

/// Get (or build) the cached element index for the current drawing. Rebuilds when the drawing
/// revision changes. Returns an `Arc` so callers can hold it across borrow boundaries.
pub fn cached_index(s: &mut Session) -> Result<std::sync::Arc<ElementIndex>> {
    let rev = s.state()?.revision;
    if let Some((r, ref idx)) = s.state()?.ifc_view.element_cache
        && r == rev
    {
        return Ok(std::sync::Arc::clone(idx));
    }
    let idx = std::sync::Arc::new(ElementIndex::build(s.doc()?));
    s.state_mut()?.ifc_view.element_cache = Some((rev, std::sync::Arc::clone(&idx)));
    Ok(idx)
}

/// Get the cached element index, returning an error if the model is empty.
fn need_index(cmd: &str, s: &mut Session) -> Result<std::sync::Arc<ElementIndex>> {
    let idx = cached_index(s)?;
    if idx.els.is_empty() {
        return Err(bad(cmd, "the drawing has no IFC objects: open an IFC model first (ifc.open)"));
    }
    Ok(idx)
}

/// Counter for how many times `elements()` is called (test instrumentation).
pub static ELEMENTS_CALL_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn r6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

fn v3(v: Vec3) -> Value {
    json!([r6(v.x), r6(v.y), r6(v.z)])
}

fn vec3_param(cmd: &str, p: &Value, key: &str) -> Result<Option<Vec3>> {
    let Some(v) = p.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    let n: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    match n[..] {
        [x, y, z] if x.is_finite() && y.is_finite() && z.is_finite() && x.abs() < 1e12 && y.abs() < 1e12 && z.abs() < 1e12 => {
            Ok(Some(Vec3::new(x, y, z)))
        }
        _ => Err(bad(cmd, format!("`{key}` must be [x, y, z]"))),
    }
}

fn limit(p: &Value) -> usize {
    p.get("limit").and_then(Value::as_u64).map_or(LIST_DEFAULT, |l| usize::try_from(l).unwrap_or(LIST_MAX).min(LIST_MAX))
}

// ---------------- opening ----------------

fn run_open(s: &mut Session, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").map(str::trim).filter(|t| !t.is_empty()).ok_or_else(|| bad("ifc.open", "path required"))?;
    if !path.rsplit('.').next().is_some_and(|e| e.eq_ignore_ascii_case("ifc")) {
        return Err(bad("ifc.open", format!("{path} isn't an IFC file (.ifc)")));
    }
    s.execute("open", &json!({ "path": path }))?;
    let els = elements(s.doc()?);
    if els.is_empty() {
        return Err(bad("ifc.open", format!("{path} has no building elements with a body")));
    }
    // A model: seen from the south-west, above, shaded.
    s.execute("vpoint", &json!({ "dir": [-1, -1, 0.8] }))?;
    s.execute("vscurrent", &json!({ "style": "shaded" }))?;
    let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
    let mut storeys: Vec<(f64, String)> = Vec::new();
    let (mut sites, mut buildings) = (BTreeSet::new(), BTreeSet::new());
    for e in &els {
        *classes.entry(e.tag.class.as_str()).or_default() += 1;
        if let Some(l) = &e.tag.level
            && !storeys.iter().any(|(_, n)| n == l)
        {
            storeys.push((e.tag.elevation.unwrap_or(0.0), l.clone()));
        }
        sites.extend(e.tag.site.clone());
        buildings.extend(e.tag.building.clone());
    }
    storeys.sort_by(|a, b| a.0.total_cmp(&b.0));
    let name = std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(json!({
        "name": name, "elements": els.len(), "classes": classes,
        "storeys": storeys.into_iter().map(|(_, n)| n).collect::<Vec<_>>(), "sites": sites, "buildings": buildings,
    }))
}

// ---------------- the spatial tree ----------------

#[derive(Default)]
struct Node {
    kind: &'static str,
    name: String,
    elevation: Option<f64>,
    count: usize,
    children: Vec<Node>,
    elements: Vec<usize>,
}

impl Node {
    /// The child `kind`/`name`, made when it isn't there yet.
    fn child(&mut self, kind: &'static str, name: &str) -> Option<&mut Node> {
        if !self.children.iter().any(|c| c.kind == kind && c.name == name) {
            self.children.push(Node { kind, name: name.to_string(), ..Node::default() });
        }
        self.children.iter_mut().find(|c| c.kind == kind && c.name == name)
    }

    /// Object `i` under the nodes of `path` (made as needed), counted on the way down.
    fn insert(&mut self, path: &[(&'static str, &str, Option<f64>)], i: usize) {
        self.count += 1;
        match path.split_first() {
            Some(((kind, name, z), rest)) => {
                if let Some(c) = self.child(kind, name) {
                    if z.is_some() {
                        c.elevation = *z;
                    }
                    c.insert(rest, i);
                }
            }
            None => self.elements.push(i),
        }
    }

    fn sort(&mut self) {
        // Storeys bottom up; everything else by name, classes after the spatial parts.
        let rank = |k: &str| if k == "class" { 1 } else { 0 };
        self.children.sort_by(|a, b| {
            rank(a.kind)
                .cmp(&rank(b.kind))
                .then_with(|| a.elevation.unwrap_or(0.0).total_cmp(&b.elevation.unwrap_or(0.0)))
                .then_with(|| a.name.cmp(&b.name))
        });
        for c in &mut self.children {
            c.sort();
        }
    }

    fn json(&self, els: &[Element], limit: usize) -> Value {
        let mut o = json!({ "kind": self.kind, "name": self.name, "count": self.count });
        if let Some(z) = self.elevation {
            o["elevation"] = json!(r6(z));
        }
        if !self.children.is_empty() {
            o["children"] = Value::Array(self.children.iter().map(|c| c.json(els, limit)).collect());
        }
        if !self.elements.is_empty() {
            o["elements"] = Value::Array(
                self.elements
                    .iter()
                    .take(limit)
                    .filter_map(|i| els.get(*i))
                    .map(|e| json!({ "handle": e.handle.hex(), "guid": e.tag.guid, "name": e.tag.name }))
                    .collect(),
            );
        }
        o
    }
}

/// The spatial tree of the model's objects.
fn tree(els: &[Element]) -> Node {
    let mut root = Node::default();
    for (i, e) in els.iter().enumerate() {
        let t = &e.tag;
        let mut path: Vec<(&'static str, &str, Option<f64>)> = Vec::new();
        for (kind, name) in [("site", &t.site), ("building", &t.building), ("storey", &t.level), ("space", &t.space)] {
            if let Some(n) = name {
                path.push((kind, n.as_str(), if kind == "storey" { t.elevation } else { None }));
            }
        }
        path.push(("class", t.class.as_str(), None));
        root.insert(&path, i);
    }
    root.sort();
    root
}

fn run_tree(s: &mut Session, p: &Value) -> Result<Value> {
    let idx = need_index("ifc.tree", s)?;
    let root = tree(&idx.els);
    let lim = limit(p);
    Ok(json!({ "tree": root.children.iter().map(|c| c.json(&idx.els, lim)).collect::<Vec<_>>(), "elements": idx.els.len() }))
}

// ---------------- one object's properties ----------------

/// Quantity sets by their name (IFC's `Qto_…` and `BaseQuantities` sets).
fn is_quantity_set(set: &str) -> bool {
    set.starts_with("Qto_") || set.contains("Quantities")
}

/// The object `p` names (`handle` or `guid`), else the one selected. Uses the index for O(1) lookup.
fn one_indexed(cmd: &str, s: &Session, idx: &ElementIndex, p: &Value, hkey: &str) -> Result<usize> {
    if let Some(q) = str_param(p, hkey).or_else(|| if hkey == "handle" { str_param(p, "guid") } else { None }) {
        let q = q.trim();
        return idx
            .by_guid_or_handle(q)
            .ok_or_else(|| bad(cmd, format!("no IFC object has the handle or GlobalId {}", q.chars().take(40).collect::<String>())));
    }
    let sel = s.selection();
    let picked: Vec<usize> = sel.iter().filter_map(|h| idx.by_handle(*h)).collect();
    match picked[..] {
        [i] => Ok(i),
        [] => Err(bad(cmd, "select an IFC object, or give its handle or GlobalId")),
        _ => Err(bad(cmd, "more than one object is selected: give the handle or GlobalId")),
    }
}

pub(crate) fn properties_json(e: &Element) -> Value {
    let t = &e.tag;
    let (mut psets, mut qsets) =
        (BTreeMap::<String, serde_json::Map<String, Value>>::new(), BTreeMap::<String, serde_json::Map<String, Value>>::new());
    for (k, v) in &t.props {
        let (set, name) = k.split_once('.').unwrap_or(("", k.as_str()));
        let to = if is_quantity_set(set) { &mut qsets } else { &mut psets };
        to.entry(set.to_string()).or_default().insert(name.to_string(), json!(v));
    }
    json!({
        "handle": e.handle.hex(), "class": t.class, "guid": t.guid, "name": t.name, "type": t.ty,
        "site": t.site, "building": t.building, "storey": t.level, "elevation": t.elevation, "space": t.space,
        "systems": t.systems, "property_sets": psets, "quantity_sets": qsets,
        "min": v3(e.min), "max": v3(e.max), "size": v3(e.max - e.min),
    })
}

fn run_properties(s: &mut Session, p: &Value) -> Result<Value> {
    let idx = need_index("ifc.properties", s)?;
    let i = one_indexed("ifc.properties", s, &idx, p, "handle")?;
    idx.els.get(i).map(properties_json).ok_or_else(|| bad("ifc.properties", "no such object"))
}

// ---------------- filters ----------------

/// The objects matching the filter in `p` (indices into `els`); `None` when `p` names no filter.
fn matching(els: &[Element], p: &Value) -> Option<Vec<usize>> {
    let want = |k: &str| str_param(p, k).map(str::trim).filter(|t| !t.is_empty());
    let (class, level, space, system, ty, property, name) =
        (want("class"), want("level"), want("space"), want("system"), want("type"), want("property"), want("name"));
    let guids: Option<BTreeSet<&str>> = p.get("guids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).take(LIST_MAX).collect());
    if [class, level, space, system, ty, property, name].iter().all(Option::is_none) && guids.is_none() {
        return None;
    }
    let eq = |a: Option<&String>, b: &str| a.is_some_and(|x| x.trim().eq_ignore_ascii_case(b));
    let prop = property.map(|q| match q.split_once('=') {
        Some((k, v)) => (k.trim().to_string(), Some(v.trim().to_string())),
        None => (q.to_string(), None),
    });
    let name = name.map(str::to_lowercase);
    Some(
        els.iter()
            .enumerate()
            .filter(|(_, e)| {
                let t = &e.tag;
                class.is_none_or(|c| ifcref::class_matches(&t.class, c))
                    && level.is_none_or(|l| eq(t.level.as_ref(), l))
                    && space.is_none_or(|x| eq(t.space.as_ref(), x))
                    && system.is_none_or(|sy| t.systems.iter().any(|x| x.eq_ignore_ascii_case(sy)))
                    && ty.is_none_or(|x| eq(t.ty.as_ref(), x))
                    && prop.as_ref().is_none_or(|(k, v)| {
                        t.props.iter().any(|(pk, pv)| pk.eq_ignore_ascii_case(k) && v.as_ref().is_none_or(|v| pv.trim().eq_ignore_ascii_case(v)))
                    })
                    && name.as_ref().is_none_or(|n| t.name.to_lowercase().contains(n.as_str()))
                    && guids.as_ref().is_none_or(|g| g.contains(t.guid.as_str()))
            })
            .map(|(i, _)| i)
            .collect(),
    )
}

fn hexes(hs: &[Handle]) -> Vec<String> {
    hs.iter().map(|h| h.hex()).collect()
}

fn run_filter(s: &mut Session, p: &Value) -> Result<Value> {
    let action = str_param(p, "action").unwrap_or("list").trim().to_ascii_lowercase();
    if action == "show" {
        let n = s.doc()?.hidden_objects.len();
        if n > 0 {
            s.execute("unisolateobjects", &json!({}))?;
        }
        s.state_mut()?.ifc_view.section_hidden.clear();
        return Ok(json!({ "action": "show", "shown": n }));
    }
    let idx = need_index("ifc.filter", s)?;
    let els = &idx.els;
    let Some(found) = matching(els, p) else {
        return Err(bad("ifc.filter", "give a filter: class, level, space, system, type, property, name or guids"));
    };
    let handles: Vec<Handle> = found.iter().filter_map(|i| els.get(*i)).map(|e| e.handle).collect();
    let lim = limit(p);
    let mut out = json!({ "matched": handles.len(), "handles": hexes(&handles[..handles.len().min(lim)]), "action": action });
    match action.as_str() {
        "list" => {
            out["elements"] = Value::Array(
                found
                    .iter()
                    .take(lim)
                    .filter_map(|i| els.get(*i))
                    .map(|e| json!({"handle": e.handle.hex(), "class": e.tag.class, "guid": e.tag.guid, "name": e.tag.name, "storey": e.tag.level}))
                    .collect(),
            );
        }
        "select" => s.set_selection(handles),
        "isolate" | "hide" if handles.is_empty() => return Err(bad("ifc.filter", "nothing matches the filter")),
        "isolate" => {
            // Shows what was hidden first: isolating is "only these", not "these of what shows".
            if !s.doc()?.hidden_objects.is_empty() {
                s.execute("unisolateobjects", &json!({}))?;
            }
            let r = s.execute("isolateobjects", &json!({ "handles": hexes(&handles) }))?;
            out["hidden"] = r["hidden"].clone();
        }
        "hide" => {
            let r = s.execute("hideobjects", &json!({ "handles": hexes(&handles) }))?;
            out["hidden"] = r["hidden"].clone();
        }
        other => {
            return Err(bad(
                "ifc.filter",
                format!("unknown action {} (list, select, isolate, hide or show)", other.chars().take(20).collect::<String>()),
            ));
        }
    }
    Ok(out)
}

// ---------------- the section box ----------------

fn boxes_meet(a: (Vec3, Vec3), b: (Vec3, Vec3)) -> bool {
    a.0.x <= b.1.x && b.0.x <= a.1.x && a.0.y <= b.1.y && b.0.y <= a.1.y && a.0.z <= b.1.z && b.0.z <= a.1.z
}

/// Show again what the section box hid.
fn section_off(s: &mut Session) -> Result<()> {
    let hid = std::mem::take(&mut s.state_mut()?.ifc_view.section_hidden);
    if !hid.is_empty() {
        let d = s.doc_mut()?;
        for h in hid {
            d.hidden_objects.remove(&h);
        }
    }
    Ok(())
}

fn run_sectionbox(s: &mut Session, p: &Value) -> Result<Value> {
    let idx = need_index("ifc.sectionbox", s)?;
    let els = &idx.els;
    let cur = s.state()?.ifc_view.section;
    let (mut lo, mut hi) = cur.unwrap_or_default();
    let mut on = cur.is_some();
    if let Some(v) = vec3_param("ifc.sectionbox", p, "min")? {
        lo = v;
        on = true;
    }
    if let Some(v) = vec3_param("ifc.sectionbox", p, "max")? {
        hi = v;
        on = true;
    }
    if let Some(f) = str_param(p, "fit") {
        let pick: Vec<&Element> = match f.trim() {
            "selection" => {
                let sel = s.selection();
                els.iter().filter(|e| sel.contains(&e.handle)).collect()
            }
            "all" => els.iter().collect(),
            other => return Err(bad("ifc.sectionbox", format!("fit is selection or all, not {}", other.chars().take(20).collect::<String>()))),
        };
        let pts: Vec<Vec3> = pick.iter().flat_map(|e| [e.min, e.max]).collect();
        let (a, b) = bounds(&pts).ok_or_else(|| bad("ifc.sectionbox", "select the objects to fit the box to"))?;
        let upm = ifcref::units_per_metre(s.doc()?);
        let pad = match p.get("pad").and_then(Value::as_f64) {
            Some(v) if v.is_finite() && (0.0..1e9).contains(&v) => v,
            Some(_) => return Err(bad("ifc.sectionbox", "pad must be a distance of 0 or more")),
            None => 0.3048 * upm,
        };
        (lo, hi, on) = (a - Vec3::new(pad, pad, pad), b + Vec3::new(pad, pad, pad), true);
    }
    if let Some(v) = p.get("on").and_then(Value::as_bool) {
        on = v;
    }
    let (lo, hi) = (Vec3::new(lo.x.min(hi.x), lo.y.min(hi.y), lo.z.min(hi.z)), Vec3::new(lo.x.max(hi.x), lo.y.max(hi.y), lo.z.max(hi.z)));
    section_off(s)?;
    if !on {
        s.state_mut()?.ifc_view.section = None;
        return Ok(json!({ "on": false, "hidden": 0 }));
    }
    if !((hi - lo).x > 0.0 || (hi - lo).y > 0.0 || (hi - lo).z > 0.0) {
        return Err(bad("ifc.sectionbox", "the box is empty: give min and max, or fit it to the selection or all"));
    }
    let d = s.doc()?;
    let hide: Vec<Handle> = els.iter().filter(|e| !boxes_meet((e.min, e.max), (lo, hi)) && !d.is_hidden_object(e.handle)).map(|e| e.handle).collect();
    let n = hide.len();
    s.doc_mut()?.hidden_objects.extend(hide.iter().copied());
    let st = s.state_mut()?;
    st.ifc_view.section = Some((lo, hi));
    st.ifc_view.section_hidden = hide;
    Ok(json!({ "on": true, "min": v3(lo), "max": v3(hi), "hidden": n }))
}

// ---------------- measuring ----------------

fn run_measure(s: &mut Session, p: &Value) -> Result<Value> {
    let d = s.doc()?;
    let upm = ifcref::units_per_metre(d);
    let span = |a: Vec3, b: Vec3| {
        let v = b - a;
        let dist = v.len();
        json!({ "distance": r6(dist), "dx": r6(v.x), "dy": r6(v.y), "dz": r6(v.z), "metres": r6(dist / upm) })
    };
    if let (Some(a), Some(b)) = (vec3_param("ifc.measure", p, "from")?, vec3_param("ifc.measure", p, "to")?) {
        return Ok(span(a, b));
    }
    if p.get("from").is_some() || p.get("to").is_some() {
        return Err(bad("ifc.measure", "a distance between points needs both `from` and `to`"));
    }
    let idx = need_index("ifc.measure", s)?;
    let els = &idx.els;
    let (ia, ib) = match (str_param(p, "a"), str_param(p, "b")) {
        (Some(_), Some(_)) => (one_indexed("ifc.measure", s, &idx, p, "a")?, one_indexed("ifc.measure", s, &idx, p, "b")?),
        (None, None) => {
            let sel = s.selection();
            let picked: Vec<usize> = sel.iter().filter_map(|h| idx.by_handle(*h)).collect();
            match picked[..] {
                [a, b] => (a, b),
                _ => return Err(bad("ifc.measure", "select two IFC objects, or give `a` and `b` (or points `from` and `to`)")),
            }
        }
        _ => return Err(bad("ifc.measure", "give both `a` and `b`")),
    };
    let (Some(a), Some(b)) = (els.get(ia), els.get(ib)) else { return Err(bad("ifc.measure", "no such object")) };
    let mut out = span(a.centre(), b.centre());
    // The gap between their boxes, axis by axis (0 where they overlap).
    let g = |lo1: f64, hi1: f64, lo2: f64, hi2: f64| (lo2 - hi1).max(lo1 - hi2).max(0.0);
    let gap = Vec3::new(g(a.min.x, a.max.x, b.min.x, b.max.x), g(a.min.y, a.max.y, b.min.y, b.max.y), g(a.min.z, a.max.z, b.min.z, b.max.z));
    out["gap"] = json!(r6(gap.len()));
    out["gap_metres"] = json!(r6(gap.len() / upm));
    out["a"] = json!({ "handle": a.handle.hex(), "class": a.tag.class, "guid": a.tag.guid, "name": a.tag.name, "centre": v3(a.centre()) });
    out["b"] = json!({ "handle": b.handle.hex(), "class": b.tag.class, "guid": b.tag.guid, "name": b.tag.name, "centre": v3(b.centre()) });
    Ok(out)
}

// ---------------- the camera ----------------

/// Look from `eye` at `target`: the view direction, zoomed to what is round the target.
pub(crate) fn set_camera(s: &mut Session, eye: Vec3, target: Vec3) -> Result<()> {
    let dir = eye - target;
    if dir.len().is_nan() || dir.len() <= 1e-9 {
        return Err(bad("ifc.camera", "`eye` and `target` must differ"));
    }
    s.execute("vpoint", &json!({ "dir": [dir.x, dir.y, dir.z] }))?;
    s.state_mut()?.ifc_view.camera = Some((eye, target));
    Ok(())
}

fn run_camera(s: &mut Session, p: &Value) -> Result<Value> {
    let (eye, target) = match (vec3_param("ifc.camera", p, "eye")?, vec3_param("ifc.camera", p, "target")?) {
        (Some(e), Some(t)) => (e, t),
        (None, None) => {
            let idx = need_index("ifc.camera", s)?;
            let sel = s.selection();
            let pick: Vec<Vec3> = match str_param(p, "fit").unwrap_or("all") {
                "selection" => idx.els.iter().filter(|e| sel.contains(&e.handle)).flat_map(|e| [e.min, e.max]).collect(),
                _ => idx.els.iter().flat_map(|e| [e.min, e.max]).collect(),
            };
            let (lo, hi) = bounds(&pick).ok_or_else(|| bad("ifc.camera", "select the objects to look at"))?;
            let dir = vec3_param("ifc.camera", p, "dir")?.unwrap_or(Vec3::new(-1.0, -1.0, 0.8));
            let n = dir * (1.0 / dir.len().max(1e-12));
            let target = (lo + hi) * 0.5;
            (target + n * ((hi - lo).len().max(1e-6) * 1.5), target)
        }
        _ => return Err(bad("ifc.camera", "a camera needs both `eye` and `target`")),
    };
    set_camera(s, eye, target)?;
    Ok(json!({ "eye": v3(eye), "target": v3(target) }))
}

// ---------------- rendering ----------------

/// The colour an IFC class is drawn in (renders, snapshots and Walk).
pub fn class_colour(class: &str) -> Rgb {
    let c = class.to_ascii_lowercase();
    let has = |k: &str| c.contains(k);
    if has("window") || has("curtainwall") || has("plate") {
        Rgb(150, 190, 220)
    } else if has("door") {
        Rgb(150, 110, 70)
    } else if has("slab") || has("roof") || has("covering") {
        Rgb(185, 182, 175)
    } else if has("column") || has("beam") || has("member") || has("footing") {
        Rgb(175, 170, 160)
    } else if has("duct") || has("air") {
        Rgb(165, 190, 215)
    } else if has("pipe") || has("valve") || has("pump") || has("sanitary") {
        Rgb(80, 135, 200)
    } else if has("cable") || has("light") || has("electric") || has("outlet") || has("switch") {
        Rgb(225, 190, 60)
    } else if has("furnish") || has("furniture") {
        Rgb(160, 130, 100)
    } else if has("space") {
        Rgb(200, 225, 200)
    } else {
        Rgb(222, 218, 210)
    }
}

/// The model as it shows (hidden objects and spaces out), each object in its class colour and the
/// selection marked, cut at the section box; seen from `eye`/`target`, along `dir`, from the
/// camera last set, else along the view direction. A PNG.
pub(crate) fn render_png(
    s: &Session,
    eye: Option<Vec3>,
    target: Option<Vec3>,
    dir: Option<Vec3>,
    size: (u32, u32),
    style: cadaddy_render::shade::VisualStyle,
    highlight: bool,
) -> Result<(Vec<u8>, Value)> {
    use cadaddy_geom::view3::Projection;
    use cadaddy_render::shade::{Tri, fit, rasterize, scene};
    let d = s.doc()?;
    let st = s.state()?;
    let sel: BTreeSet<Handle> = if highlight { s.selection().into_iter().collect() } else { BTreeSet::new() };
    let mut md = Drawing::new_metric();
    md.layers = d.layers.clone();
    for e in d.model.iter().take(MAX_ELEMENTS) {
        if d.is_hidden_object(e.handle) {
            continue;
        }
        let Some(t) = ifcref::tag(e) else { continue };
        if t.class.eq_ignore_ascii_case("Space") {
            continue;
        }
        let mut c = (**e).clone();
        c.common.color = cadaddy_color::Color::True(if sel.contains(&e.handle) { SELECTED } else { class_colour(&t.class) });
        md.model.push(c);
    }
    if md.model.is_empty() {
        return Err(bad("ifc.render", "nothing of the model shows (everything is hidden)"));
    }
    let ext = md.extents(&Space::Model);
    let span = ext.width().max(ext.height());
    let tol = if span.is_finite() && span > 0.0 { span / 1500.0 } else { 1.0 };
    let bg = Rgb(255, 255, 255);
    let mut sc = scene(&md, &Space::Model, tol, bg);
    let mut keep_in: Vec<(Vec3, f64)> = Vec::new();
    if let Some((lo, hi)) = st.ifc_view.section {
        keep_in.extend([
            (Vec3::new(1.0, 0.0, 0.0), lo.x),
            (Vec3::new(-1.0, 0.0, 0.0), -hi.x),
            (Vec3::new(0.0, 1.0, 0.0), lo.y),
            (Vec3::new(0.0, -1.0, 0.0), -hi.y),
            (Vec3::new(0.0, 0.0, 1.0), lo.z),
            (Vec3::new(0.0, 0.0, -1.0), -hi.z),
        ]);
    }
    let pose = match (eye, target) {
        (Some(e), Some(t)) => Some((e, t)),
        (None, None) if dir.is_none() => st.ifc_view.camera,
        (None, None) => None,
        _ => return Err(bad("ifc.render", "a camera pose needs both `eye` and `target`")),
    };
    let view_dir = match pose {
        Some((e, t)) => {
            let fwd = t - e;
            if fwd.len().is_nan() || fwd.len() <= 1e-9 {
                return Err(bad("ifc.render", "`eye` and `target` must differ"));
            }
            let f = fwd * (1.0 / fwd.len());
            // What is behind the eye is cut away (a camera inside the building sees the room).
            keep_in.push((f, f.dot(e)));
            e - t
        }
        None => dir.unwrap_or(st.view_dir),
    };
    let view_dir = if view_dir.len() > 1e-9 && !(view_dir.x == 0.0 && view_dir.y == 0.0) { view_dir } else { Vec3::new(-1.0, -1.0, 0.8) };
    if !keep_in.is_empty() {
        let tris = std::mem::take(&mut sc.tris);
        for t in tris {
            let poly = super::verify::clip_tri(&t.p, &keep_in);
            for k in 1..poly.len().saturating_sub(1) {
                if let (Some(a), Some(b), Some(c)) = (poly.first(), poly.get(k), poly.get(k + 1)) {
                    sc.tris.push(Tri::plain([*a, *b, *c], t.color));
                }
            }
        }
        let lines = std::mem::take(&mut sc.lines);
        for l in lines {
            if let Some((a, b)) = super::verify::keep_seg(l.a, l.b, &keep_in) {
                sc.lines.push(cadaddy_render::shade::Line3 { a, b, ..l });
            }
        }
    }
    if sc.tris.is_empty() && sc.lines.is_empty() {
        return Err(bad("ifc.render", "nothing of the model is in view"));
    }
    // When there is a selection, fit the view to the selected (orange) objects' bounds (padded)
    // instead of the entire scene, so snapshots/renders show the subject up close.
    let has_selection = highlight && sc.tris.iter().any(|t| t.color == SELECTED);
    let proj = Projection::new(view_dir);
    let view = if has_selection {
        use cadaddy_geom::Vec2;
        let base = cadaddy_render::shade::ShadeView::new(proj, Vec2::ZERO, 1.0, size.0, size.1);
        let on_plane = |q: Vec3| Vec2::new(q.dot(base.right), q.dot(base.up));
        // Bounds of only the selected triangles on the view plane.
        let mut lo = Vec2::new(f64::MAX, f64::MAX);
        let mut hi = Vec2::new(f64::MIN, f64::MIN);
        for t in &sc.tris {
            if t.color == SELECTED {
                for p in &t.p {
                    let v = on_plane(*p);
                    lo = Vec2::new(lo.x.min(v.x), lo.y.min(v.y));
                    hi = Vec2::new(hi.x.max(v.x), hi.y.max(v.y));
                }
            }
        }
        let w = hi.x - lo.x;
        let h = hi.y - lo.y;
        if w.is_finite() && h.is_finite() && w >= 0.0 && h >= 0.0 {
            let pad = (w.max(h)) * 0.15 + 1e-6; // 15% padding around the selection
            let center = Vec2::new((lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5);
            let sel_h = (h + 2.0 * pad).max((w + 2.0 * pad) * f64::from(size.1) / f64::from(size.0));
            cadaddy_render::shade::ShadeView::new(proj, center, sel_h, size.0, size.1)
        } else {
            fit(&sc, proj, size.0, size.1).ok_or_else(|| bad("ifc.render", "nothing to fit in the view"))?
        }
    } else {
        fit(&sc, proj, size.0, size.1).ok_or_else(|| bad("ifc.render", "nothing to fit in the view"))?
    };
    let raster = rasterize(&sc, &view, style, bg);
    let png = raster.encode_png().ok_or_else(|| bad("ifc.render", "the image couldn't be encoded"))?;
    let info = json!({ "width": view.width_px, "height": view.height_px, "triangles": sc.tris.len() });
    Ok((png, info))
}

fn run_render(s: &mut Session, p: &Value) -> Result<Value> {
    use cadaddy_render::shade::VisualStyle;
    need_index("ifc.render", s)?;
    let dim = |k: &str, def: u32| p.get(k).and_then(Value::as_u64).map_or(def, |v| u32::try_from(v).unwrap_or(4096).clamp(16, 4096));
    let style = match str_param(p, "style") {
        Some(n) => VisualStyle::parse(n).ok_or_else(|| bad("ifc.render", format!("unknown style {}", n.chars().take(20).collect::<String>())))?,
        None => VisualStyle::ShadedEdges,
    };
    let (eye, target, dir) = (vec3_param("ifc.render", p, "eye")?, vec3_param("ifc.render", p, "target")?, vec3_param("ifc.render", p, "dir")?);
    let (png, mut out) = render_png(s, eye, target, dir, (dim("width", 800), dim("height", 600)), style, bool_or(p, "highlight", true))?;
    super::verify::deliver(png, p, &mut out)?;
    Ok(out)
}

// ---------------- Walk ----------------

/// The model as a scene to walk through, or `None` when the drawing has no IFC objects: every
/// object but spaces and openings (feet), storeys as levels, the sides of walls, columns and
/// other upright faces to bump into.
pub fn walk_scene(d: &Drawing) -> Option<cadaddy_walk3d::Scene> {
    use cadaddy_walk3d::{LevelInfo, Owner, Part, Scene, Seg, V3, Vertex};
    let els = elements(d);
    if els.is_empty() {
        return None;
    }
    let upm = ifcref::units_per_metre(d);
    let ft = 1.0 / (upm * 0.3048);
    // Storeys bottom up, by elevation (feet).
    let mut storeys: Vec<(String, f64)> = Vec::new();
    for e in &els {
        if let Some(l) = &e.tag.level
            && !storeys.iter().any(|(n, _)| n == l)
        {
            storeys.push((l.clone(), e.tag.elevation.unwrap_or(0.0) / 0.3048));
        }
    }
    storeys.sort_by(|a, b| a.1.total_cmp(&b.1));
    if storeys.is_empty() {
        let z = els.iter().map(|e| e.min.z * ft).fold(f64::INFINITY, f64::min);
        storeys.push(("Model".into(), if z.is_finite() { z } else { 0.0 }));
    }
    let level_of = |t: &Tag| t.level.as_ref().and_then(|l| storeys.iter().position(|(n, _)| n == l)).unwrap_or(0);
    let mut sc = Scene::default();
    sc.units_per_foot = upm * 0.3048;
    let mut lo: Vec<(f64, f64, f64, f64)> = vec![(f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY); storeys.len()];
    let p = |v: Vec3| V3 { x: v.x * ft, y: v.y * ft, z: v.z * ft };
    for e in d.model.iter().take(MAX_ELEMENTS) {
        let Some(t) = ifcref::tag(e) else { continue };
        if t.class.eq_ignore_ascii_case("Space") || t.class.eq_ignore_ascii_case("OpeningElement") {
            continue;
        }
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let lv = level_of(&t);
        let c = class_colour(&t.class);
        let part = {
            let k = t.class.to_ascii_lowercase();
            if k.contains("slab") || k.contains("covering") {
                Part::Floor
            } else if k.contains("window") || k.contains("plate") {
                Part::Glass
            } else if k.contains("stair") || k.contains("ramp") {
                Part::Stair
            } else if k.contains("wall") || k.contains("column") {
                Part::Wall
            } else {
                Part::Device
            }
        };
        for f in &m.faces {
            let vs: Vec<Vec3> =
                f.v.iter()
                    .filter_map(|i| m.vertices.get(*i as usize).copied())
                    .filter(|v| v.x.is_finite() && v.y.is_finite() && v.z.is_finite())
                    .collect();
            let (Some(a), true) = (vs.first().copied(), vs.len() >= 3) else { continue };
            let n = vs.windows(2).skip(1).fold(Vec3::ZERO, |acc, w| match w {
                [b, c] => acc + (*b - a).cross(*c - a),
                _ => acc,
            });
            let nl = n.len();
            if nl.is_nan() || nl <= 0.0 {
                continue;
            }
            let n = n * (1.0 / nl);
            let nv = [n.x as f32, n.y as f32, n.z as f32];
            for w in vs.windows(2).skip(1) {
                let [b, cc] = w else { continue };
                for q in [a, *b, *cc] {
                    let q = p(q);
                    sc.tris.push(Vertex {
                        p: [q.x as f32, q.y as f32, q.z as f32],
                        n: nv,
                        c: [c.0, c.1, c.2, if part == Part::Glass { 110 } else { 255 }],
                    });
                }
                sc.owners.push(Owner { src: 0, handle: e.handle });
                sc.parts.push(part);
            }
            // Upright faces of walls and columns are what one bumps into: their foot on the plan.
            if n.z.abs() < 0.2 && part == Part::Wall {
                let (lo_z, pts): (f64, Vec<V3>) = (vs.iter().map(|v| v.z).fold(f64::INFINITY, f64::min), vs.iter().map(|v| p(*v)).collect());
                let foot: Vec<&V3> = pts.iter().filter(|q| (q.z - lo_z * ft).abs() < 0.05).collect();
                if let (Some(a), Some(b)) = (foot.first(), foot.last())
                    && ((a.x - b.x).hypot(a.y - b.y)) > 0.05
                {
                    sc.walls.push(Seg { a: cadaddy_geom::Vec2::new(a.x, a.y), b: cadaddy_geom::Vec2::new(b.x, b.y), level: lv });
                }
            }
            if let Some(b) = lo.get_mut(lv) {
                for v in &vs {
                    let q = p(*v);
                    *b = (b.0.min(q.x), b.1.min(q.y), b.2.max(q.x), b.3.max(q.y));
                }
            }
        }
    }
    if sc.tris.is_empty() {
        return None;
    }
    let n = storeys.len();
    for (i, (name, z)) in storeys.iter().enumerate() {
        let next = storeys.get(i + 1).map(|s| s.1);
        let height = next.map_or(10.0, |nz| (nz - z).max(7.0));
        let (x0, y0, x1, y1) = lo.get(i).copied().unwrap_or((0.0, 0.0, 0.0, 0.0));
        let ok = x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite();
        sc.levels.push(LevelInfo {
            name: name.clone(),
            elevation_ft: *z,
            height_ft: height,
            ceiling_ft: (height - 1.0).max(7.0),
            min: if ok { cadaddy_geom::Vec2::new(x0, y0) } else { cadaddy_geom::Vec2::ZERO },
            max: if ok { cadaddy_geom::Vec2::new(x1, y1) } else { cadaddy_geom::Vec2::ZERO },
            base: cadaddy_geom::Vec2::ZERO,
        });
    }
    debug_assert_eq!(sc.levels.len(), n);
    Some(sc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_and_quantity_sets() {
        let a = (Vec3::ZERO, Vec3::new(1.0, 1.0, 1.0));
        assert!(boxes_meet(a, (Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 1.0, 1.0))), "touching meets");
        assert!(!boxes_meet(a, (Vec3::new(1.1, 0.0, 0.0), Vec3::new(2.0, 1.0, 1.0))));
        assert!(is_quantity_set("Qto_WallBaseQuantities") && is_quantity_set("BaseQuantities") && !is_quantity_set("Pset_WallCommon"));
        assert_eq!(bounds(&[Vec3::new(f64::NAN, 0.0, 0.0)]), None);
    }
}
