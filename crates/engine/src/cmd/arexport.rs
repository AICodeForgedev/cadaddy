//! EXPORT FOR AR (`bim.export.ar`, Q6207): a part of the built model as files a phone or tablet
//! opens in its own augmented-reality viewer (AR Quick Look takes a `.usdz` from Files, Mail or
//! AirDrop; any glTF viewer takes the `.glb`), at true size, to be placed on the real floor by
//! matching one corner.
//!
//! - **Scope:** one or more rooms (default: one room), a level, or a box on a level (building
//!   feet), narrowed by trade and kind.
//! - **Frame:** metres, Y up (glTF 2.0 §3.4; USD `upAxis = "Y"`, `metersPerUnit = 1`). The origin
//!   is the anchor (a room corner, by default the one nearest the room's entry; a gridline
//!   intersection; or a named point) on its level's finished floor, so the floor is at y = 0 and
//!   each element stands at its model elevation above it. Plan x runs along +X, plan y along −Z.
//! - **Elements:** one node each, named by kind and tag, carrying its id, kind, tag,
//!   circuit/panel, mounting height, product, trade and source (glTF node `extras`, USD prim
//!   `customData`, and the sidecar's `elements` list for viewers that don't expose either).
//!   Repeated parts share one mesh (glTF meshes referenced by many nodes; USD instanceable
//!   references to one prototype).
//! - **Readable on a phone:** colours by system and trade, the shell translucent (or off), the
//!   trades' loose pieces left out, and triangle and byte caps; all of it rules data
//!   (`arexport.json`, overridden per job under `ar_export` in the building spec). Over a cap the
//!   context is simplified first, then dropped, then the elements furthest from the anchor; the
//!   answer and the sidecar say what went.
//! - **Files:** `NAME.glb`, `NAME.usdz` (an uncompressed zip, 64-byte aligned, the root layer
//!   first: the USDZ specification) and `NAME.json`, the sidecar naming the anchor (what it is,
//!   where, its plan coordinates), the scope, the counts and the elements.
//!
//! Read-only: the drawing is not changed.

mod glb;
mod usdz;

use std::collections::{BTreeMap, HashMap};

use cadaddy_doc::{Drawing, Entity, EntityKind, Handle};
use cadaddy_geom::Vec2;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::bimlook::{Class, classify, made_source, made_tag};
use super::validate::Frame;
use super::wallgen::made_as;
use super::*;
use crate::{Result, Session};

const CMD: &str = "bim.export.ar";
const RULES_CMD: &str = "bim.export.ar.rules";
const RULES: &str = include_str!("arexport.json");
/// The building spec's key for a job's own rules.
pub const KEY: &str = "ar_export";
/// The sidecar's schema name.
pub const SCHEMA: &str = "cadaddy.ar/1";
const M_PER_FT: f64 = 0.3048;
/// Most rooms a scope names, corners listed per room, faces read per mesh, vertices per face.
const MAX_SCOPE_ROOMS: usize = 200;
const MAX_CORNERS: usize = 64;
const MAX_FACES: usize = 200_000;
const MAX_FACE_VERTICES: usize = 1_024;
/// Most objects looked at (a print set's linework comes before the built model).
const MAX_SCAN: usize = 5_000_000;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Export for AR", run)
            .params(
                "{room?: number or name, rooms?: [number or name] (default: one room, the first built), level?: name (all of it, or the level of `box`/`room`), box?: {min: [x, y], max: [x, y], z?: [from, to]} (building feet on the level; z feet above its floor), trades?: [trade or system] (electrical, plumbing, lights, conduit), kinds?: [kind] (receptacle, device, duct), anchor?: {corner: n (1-based, the sidecar's points) | grid: \"A-1\" | name: text, at?: [x, y] (building feet; without it the plan text `name`)} (default: the room corner nearest its entry), context?: \"translucent\"|\"solid\"|\"off\", small?: bool (keep the trades' loose pieces), clip?: bool (default true: cut what runs past the rooms or box, as a wall running on down the corridor), formats?: [\"glb\", \"usdz\"] (default both), max_triangles?, max_bytes? (lower the rules' caps), path?: where to write (NAME.glb / NAME.usdz / NAME.json beside each other; default beside the drawing), force?: bool (replace existing files)} → {files: {glb?, usdz?, sidecar}, bytes, nodes, meshes, triangles, kinds, trades, anchor, scope, dropped, simplified, cut_to_scope, seconds, checks}",
            )
            .noundo()
            .files(),
        CommandSpec::new(RULES_CMD, "AR Export Rules", run_rules).params(
            "{rules?: object (the job's own, merged over the shipped arexport.json: caps, context, skip_parts, small_parts, colors, scope), reset?: bool (back to the shipped rules)} → {rules: the rules in effect, own: bool (the job has its own), shipped: the shipped rules}",
        ),
    ]
}

// ─── Rules ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Rules {
    pub version: u32,
    pub caps: Caps,
    pub context: Context,
    pub skip_parts: Parts,
    pub small_parts: Small,
    pub colors: Colors,
    pub scope: ScopeRules,
    pub kind_trades: Vec<KindTrade>,
}

/// An element kind's trade and system when the BIM view's classification (by layer) doesn't
/// say it: a plumbing fixture on a device layer is still plumbing.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct KindTrade {
    /// A word of the kind (`plumbing` matches `plumbing_fixture`).
    pub kind: String,
    pub trade: String,
    pub system: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Caps {
    pub max_triangles: u64,
    pub max_bytes: u64,
    pub max_elements: u64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Context {
    pub parts: Vec<String>,
    pub mode: String,
    pub opacity: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Parts {
    pub parts: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Small {
    pub parts: Vec<String>,
    pub mep_pieces: Vec<String>,
    pub max_size_mm: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Colors {
    pub by_system: BTreeMap<String, [f64; 3]>,
    pub by_trade: BTreeMap<String, [f64; 3]>,
    pub default: Option<[f64; 3]>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ScopeRules {
    pub room_margin_ft: f64,
    pub entry_probe_ft: Vec<f64>,
}

impl Rules {
    /// The shipped rules with the job's own (`ar_export` in the building spec) merged over them;
    /// whether the job has its own.
    pub fn of(d: &Drawing) -> (Rules, bool) {
        let mut v: Value = serde_json::from_str(RULES).unwrap_or(Value::Null);
        let own =
            d.building_spec.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()).and_then(|s| s.get(KEY).cloned()).filter(Value::is_object);
        if let Some(o) = &own {
            cadaddy_walk3d::merge(&mut v, o);
        }
        (serde_json::from_value(v).unwrap_or_default(), own.is_some())
    }

    fn margin_ft(&self) -> f64 {
        Some(self.scope.room_margin_ft).filter(|m| m.is_finite() && *m >= 0.0).unwrap_or(1.0).min(20.0)
    }

    fn probes_ft(&self) -> Vec<f64> {
        let p: Vec<f64> = self.scope.entry_probe_ft.iter().copied().filter(|v| v.is_finite() && *v > 0.0 && *v < 50.0).take(8).collect();
        if p.is_empty() { vec![0.5, 1.5, 3.0] } else { p }
    }

    fn opacity(&self) -> f64 {
        Some(self.context.opacity).filter(|o| o.is_finite() && *o > 0.0).unwrap_or(0.25).clamp(0.05, 1.0)
    }

    /// RGB 0–1 of an element's system, else its trade, else the default.
    fn rgb(&self, system: &str, trade: &str) -> [f32; 3] {
        let pick = self
            .colors
            .by_system
            .get(system)
            .or_else(|| self.colors.by_trade.get(trade))
            .copied()
            .or(self.colors.default)
            .unwrap_or([200.0, 200.0, 200.0]);
        pick.map(|v| if v.is_finite() { (v / 255.0).clamp(0.0, 1.0) as f32 } else { 0.8 })
    }

    /// The trade and system of an element of `kind` classified as `c`.
    fn trade_of(&self, kind: &str, c: &Class) -> (String, String) {
        let k = slug(kind);
        for r in self.kind_trades.iter().take(500) {
            let w = slug(&r.kind);
            if !w.is_empty() && !r.trade.trim().is_empty() && k.contains(&w) {
                let system = if r.system.trim().is_empty() { c.key.to_string() } else { r.system.trim().to_string() };
                return (slug(&r.trade), system);
            }
        }
        (slug(c.group), c.key.to_string())
    }

    fn is(list: &[String], word: &str) -> bool {
        list.iter().any(|w| w.trim().eq_ignore_ascii_case(word.trim()))
    }
}

/// Most bytes of a job's own rules.
const MAX_OWN_RULES: usize = 200_000;

/// The job's own rules (`ar_export` in the building spec): set (merged over what it has), or
/// reset; the rules in effect.
fn run_rules(s: &mut Session, p: &Value) -> Result<Value> {
    let reset = bool_param(RULES_CMD, p, "reset")?.unwrap_or(false);
    let patch = p.get("rules").filter(|v| !v.is_null());
    if patch.is_some_and(|v| !v.is_object()) {
        return Err(bad(RULES_CMD, "`rules` must be an object (caps, context, skip_parts, small_parts, colors, scope)"));
    }
    if reset || patch.is_some() {
        let d = s.doc()?;
        let mut spec: Value = d.building_spec.as_deref().and_then(|t| serde_json::from_str(t).ok()).filter(Value::is_object).unwrap_or(json!({}));
        let mut own = if reset { json!({}) } else { spec.get(KEY).cloned().filter(Value::is_object).unwrap_or(json!({})) };
        if let Some(x) = patch {
            cadaddy_walk3d::merge(&mut own, x);
            // What the reader would make of it: refused when it doesn't read.
            let mut all: Value = serde_json::from_str(RULES).unwrap_or(Value::Null);
            cadaddy_walk3d::merge(&mut all, &own);
            serde_json::from_value::<Rules>(all).map_err(|e| bad(RULES_CMD, format!("`rules`: {e}")))?;
            if own.to_string().len() > MAX_OWN_RULES {
                return Err(bad(RULES_CMD, "`rules` is too large"));
            }
        }
        if let Some(o) = spec.as_object_mut() {
            if own.as_object().is_some_and(|o| o.is_empty()) {
                o.remove(KEY);
            } else {
                o.insert(KEY.into(), own);
            }
        }
        s.doc_mut()?.building_spec = Some(spec.to_string());
    }
    let d = s.doc()?;
    let (_, own) = Rules::of(d);
    let mut effective: Value = serde_json::from_str(RULES).unwrap_or(Value::Null);
    if let Some(o) = d.building_spec.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()).and_then(|v| v.get(KEY).cloned()) {
        cadaddy_walk3d::merge(&mut effective, &o);
    }
    Ok(json!({"rules": effective, "own": own, "shipped": serde_json::from_str::<Value>(RULES).unwrap_or(Value::Null)}))
}

/// `Fire Protection` → `fire_protection`.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for w in s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()) {
        if !out.is_empty() {
            out.push('_');
        }
        out.push_str(&w.to_ascii_lowercase());
    }
    out
}

// ─── What gets written ───────────────────────────────────────────────────────

/// One shared mesh: triangles in the node's own frame (metres, Y up), flat normals.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Geometry {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub material: usize,
}

impl Geometry {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for p in &self.positions {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        if self.positions.is_empty() { ([0.0; 3], [0.0; 3]) } else { (lo, hi) }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Material {
    pub name: String,
    pub rgb: [f32; 3],
    pub opacity: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub name: String,
    pub geometry: usize,
    /// Where the node's frame sits (metres, Y up, from the anchor).
    pub translation: [f64; 3],
    pub meta: Map<String, Value>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Scene {
    pub nodes: Vec<Node>,
    pub geometries: Vec<Geometry>,
    pub materials: Vec<Material>,
    /// What the files say about themselves (anchor, units, scope): glTF `asset.extras`, USD
    /// `customLayerData`.
    pub info: Value,
}

impl Scene {
    /// Triangles drawn: each node's mesh once per node (an instance costs its triangles too).
    pub fn drawn_triangles(&self) -> usize {
        self.nodes.iter().map(|n| self.geometries.get(n.geometry).map_or(0, Geometry::triangles)).sum()
    }
}

// ─── Where things are ────────────────────────────────────────────────────────

/// A room of the built model in building feet.
#[derive(Clone, Debug)]
struct Room {
    handle: Handle,
    level: String,
    name: String,
    number: Option<String>,
    outline: Vec<[f64; 2]>,
    area: f64,
}

impl Room {
    fn label(&self) -> String {
        match &self.number {
            Some(n) if !n.eq_ignore_ascii_case(&self.name) => format!("{n} {}", self.name),
            Some(n) => n.clone(),
            None => self.name.clone(),
        }
    }
}

fn area(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    let mut a = 0.0;
    for i in 0..n {
        let (Some(p), Some(q)) = (poly.get(i), poly.get((i + 1) % n)) else { continue };
        a += p[0] * q[1] - q[0] * p[1];
    }
    (a * 0.5).abs()
}

fn inside(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let pts: Vec<Vec2> = poly.iter().map(|q| Vec2::new(q[0], q[1])).collect();
    cadaddy_geom::point_in_polygon(&pts, Vec2::new(p[0], p[1]))
}

fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p[0] - (a[0] + t * dx)).hypot(p[1] - (a[1] + t * dy))
}

fn edge_dist(poly: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let n = poly.len();
    (0..n).filter_map(|i| Some(seg_dist(p, *poly.get(i)?, *poly.get((i + 1) % n)?))).fold(f64::INFINITY, f64::min)
}

fn segs_cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    let o = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    let (d1, d2, d3, d4) = (o(c, d, a), o(c, d, b), o(a, b, c), o(a, b, d));
    ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0))
}

/// Whether a plan box (building feet) touches a polygon.
fn box_meets(poly: &[[f64; 2]], lo: [f64; 2], hi: [f64; 2]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let corners = [[lo[0], lo[1]], [hi[0], lo[1]], [hi[0], hi[1]], [lo[0], hi[1]]];
    if corners.iter().any(|c| inside(poly, *c)) || inside(poly, [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0]) {
        return true;
    }
    if poly.iter().any(|p| p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1]) {
        return true;
    }
    let n = poly.len();
    (0..n).any(|i| {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + 1) % n)) else { return false };
        (0..4).any(|k| corners.get(k).zip(corners.get((k + 1) % 4)).is_some_and(|(c, d)| segs_cross(*a, *b, *c, *d)))
    })
}

/// The corners of an outline: the vertices where it turns (more than 20°), in order.
fn corners(poly: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let n = poly.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (Some(p), Some(c), Some(q)) = (poly.get((i + n - 1) % n), poly.get(i), poly.get((i + 1) % n)) else { continue };
        let (ax, ay, bx, by) = (c[0] - p[0], c[1] - p[1], q[0] - c[0], q[1] - c[1]);
        let (la, lb) = (ax.hypot(ay), bx.hypot(by));
        if la < 1e-6 || lb < 1e-6 {
            continue;
        }
        let cos = (ax * bx + ay * by) / (la * lb);
        if cos < 20f64.to_radians().cos() {
            out.push(*c);
        }
        if out.len() >= MAX_CORNERS {
            break;
        }
    }
    if out.len() < 3 { poly.iter().take(MAX_CORNERS).copied().collect() } else { out }
}

/// The building's rooms (its room elements) in building feet.
fn rooms(d: &Drawing, frame: &Frame) -> Vec<Room> {
    super::bspec::room_elements(d)
        .into_iter()
        .filter_map(|r| {
            let e = d.model.get(r.handle)?;
            let outline: Vec<[f64; 2]> =
                r.outline.iter().map(|p| frame.built_on(&r.level, e, *p)).filter(|p| p[0].is_finite() && p[1].is_finite()).collect();
            if outline.len() < 3 {
                return None;
            }
            let area = area(&outline);
            Some(Room { handle: r.handle, level: r.level, name: r.name, number: r.number, outline, area })
        })
        .collect()
}

/// A built element's box in building feet (z above the building's datum).
fn box_ft(frame: &Frame, level: &str, e: &Entity) -> Option<([f64; 3], [f64; 3])> {
    let (lo, hi) = super::fieldview::mesh_box(e)?;
    let a = frame.built_on(level, e, Vec2::new(lo[0], lo[1]));
    let b = frame.built_on(level, e, Vec2::new(hi[0], hi[1]));
    let upf = frame.upf();
    let out = ([a[0].min(b[0]), a[1].min(b[1]), lo[2] / upf], [a[0].max(b[0]), a[1].max(b[1]), hi[2] / upf]);
    out.0.iter().chain(out.1.iter()).all(|v| v.is_finite()).then_some(out)
}

// ─── The scope ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
struct Scope {
    rooms: Vec<Room>,
    level: Option<String>,
    /// Plan box (building feet) and z range above the level's floor (feet).
    region: Option<([f64; 2], [f64; 2], Option<[f64; 2]>)>,
    trades: Vec<String>,
    kinds: Vec<String>,
    defaulted: bool,
}

fn list_param(p: &Value, key: &str) -> Result<Vec<String>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(s)) => Ok(vec![s.trim().to_string()].into_iter().filter(|s| !s.is_empty()).collect()),
        Some(Value::Array(a)) => a
            .iter()
            .take(1_000)
            .map(|v| v.as_str().map(|s| s.trim().to_string()).ok_or_else(|| bad(CMD, format!("`{key}` must be a list of text"))))
            .collect::<Result<Vec<_>>>()
            .map(|v| v.into_iter().filter(|s| !s.is_empty()).collect()),
        Some(_) => Err(bad(CMD, format!("`{key}` must be text or a list of text"))),
    }
}

fn find_room<'a>(all: &'a [Room], want: &str, level: Option<&str>) -> Result<&'a Room> {
    let w = want.trim();
    let on = |r: &&Room| level.is_none_or(|l| r.level.eq_ignore_ascii_case(l));
    let by_number: Vec<&Room> = all.iter().filter(on).filter(|r| r.number.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(w))).collect();
    let by_handle = || all.iter().filter(on).filter(|r| r.handle.hex().eq_ignore_ascii_case(w)).collect::<Vec<_>>();
    let by_name = || all.iter().filter(on).filter(|r| r.name.trim().eq_ignore_ascii_case(w) || r.label().eq_ignore_ascii_case(w)).collect::<Vec<_>>();
    let found = if by_number.is_empty() { by_handle() } else { by_number };
    let found = if found.is_empty() { by_name() } else { found };
    match found.as_slice() {
        [r] => Ok(r),
        [] => {
            let some: Vec<String> = all.iter().filter(on).take(12).map(Room::label).collect();
            Err(bad(
                CMD,
                format!(
                    "there is no room `{w}`{} in the built model (rooms: {}{})",
                    level.map(|l| format!(" on {l}")).unwrap_or_default(),
                    some.join(", "),
                    if all.len() > 12 { ", …" } else { "" }
                ),
            ))
        }
        many => {
            let levels: Vec<&str> = many.iter().map(|r| r.level.as_str()).take(8).collect();
            Err(bad(CMD, format!("{} rooms match `{w}` (levels {}): give its number, or `level`", many.len(), levels.join(", "))))
        }
    }
}

fn pair(v: &Value) -> Option<[f64; 2]> {
    let a = v.as_array()?;
    let x = a.first()?.as_f64()?;
    let y = a.get(1)?.as_f64()?;
    (x.is_finite() && y.is_finite()).then_some([x, y])
}

fn scope_of(p: &Value, all: &[Room], sp: &cadaddy_walk3d::BuildingSpec) -> Result<Scope> {
    let mut sc = Scope { trades: list_param(p, "trades")?, kinds: list_param(p, "kinds")?, ..Scope::default() };
    if let Some(l) = p.get("level").filter(|v| !v.is_null()) {
        let l = l.as_str().map(str::trim).filter(|l| !l.is_empty()).ok_or_else(|| bad(CMD, "`level` must be a level's name"))?;
        let lv = sp.level(l).ok_or_else(|| {
            let names: Vec<&str> = sp.levels.iter().map(|l| l.name.as_str()).take(20).collect();
            bad(CMD, format!("there is no level `{l}` (levels: {})", names.join(", ")))
        })?;
        sc.level = Some(lv.name.clone());
    }
    let mut wanted = list_param(p, "rooms")?;
    if let Some(r) = p.get("room").filter(|v| !v.is_null()) {
        let r = r
            .as_str()
            .map(str::to_string)
            .or_else(|| r.as_u64().map(|n| n.to_string()))
            .ok_or_else(|| bad(CMD, "`room` must be a room's number or name"))?;
        wanted.insert(0, r);
    }
    if wanted.len() > MAX_SCOPE_ROOMS {
        return Err(bad(CMD, format!("{} rooms: at most {MAX_SCOPE_ROOMS} in one export", wanted.len())));
    }
    for w in &wanted {
        let r = find_room(all, w, sc.level.as_deref())?;
        if !sc.rooms.iter().any(|x| x.handle == r.handle) {
            sc.rooms.push(r.clone());
        }
    }
    if let Some(b) = p.get("box").filter(|v| !v.is_null()) {
        let (Some(a), Some(c)) = (b.get("min").and_then(pair), b.get("max").and_then(pair)) else {
            return Err(bad(CMD, "`box` needs min: [x, y] and max: [x, y] (building feet)"));
        };
        let z = match b.get("z") {
            None | Some(Value::Null) => None,
            Some(v) => Some(pair(v).ok_or_else(|| bad(CMD, "`box.z` must be [from, to] (feet above the floor)"))?),
        };
        let lo = [a[0].min(c[0]), a[1].min(c[1])];
        let hi = [a[0].max(c[0]), a[1].max(c[1])];
        if hi[0] - lo[0] > 1e5 || hi[1] - lo[1] > 1e5 {
            return Err(bad(CMD, "`box` is over 100,000 ft across"));
        }
        sc.region = Some((lo, hi, z.map(|z| [z[0].min(z[1]), z[0].max(z[1])])));
        if sc.level.is_none() {
            sc.level = sc.rooms.first().map(|r| r.level.clone()).or_else(|| sp.levels.first().map(|l| l.name.clone()));
        }
    }
    if sc.rooms.is_empty() && sc.level.is_none() && sc.region.is_none() {
        // The default: one room, the first one built (lowest level, then by number).
        let elev = |r: &Room| sp.level(&r.level).map_or(0.0, |l| l.elevation_ft);
        let first = all.iter().min_by(|a, b| {
            elev(a).total_cmp(&elev(b)).then_with(|| a.number.as_deref().unwrap_or(&a.name).cmp(b.number.as_deref().unwrap_or(&b.name)))
        });
        let r = first.ok_or_else(|| bad(CMD, "the built model has no rooms: give `level` or `box` (run model.build first)"))?;
        sc.rooms.push(r.clone());
        sc.defaulted = true;
    }
    Ok(sc)
}

// ─── The anchor ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Anchor {
    kind: &'static str,
    name: String,
    level: String,
    room: Option<String>,
    at: [f64; 2],
    floor_ft: f64,
    how: String,
    entry: Option<Value>,
}

/// The doors of a level next to a room, the room on each one's far side, and the entry: the
/// door whose far side is the biggest room (a corridor or a lobby beats a closet), then one to
/// the outside, then the widest.
fn entry_door(d: &Drawing, frame: &Frame, rules: &Rules, room: &Room, all: &[Room]) -> Option<Entry> {
    let mut best: Option<(f64, f64, Entry)> = None;
    for e in d.model.iter().take(MAX_SCAN) {
        if made_as(e).as_deref() != Some("door") {
            continue;
        }
        let level = super::bspec::made_level(e).unwrap_or_default();
        if !level.eq_ignore_ascii_case(&room.level) {
            continue;
        }
        let Some((lo, hi)) = box_ft(frame, &level, e) else { continue };
        let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
        let (half, width, axis) = if w <= h { (w / 2.0, h, [1.0, 0.0]) } else { (h / 2.0, w, [0.0, 1.0]) };
        if edge_dist(&room.outline, c) > half + 1.0 && !inside(&room.outline, c) {
            continue;
        }
        // The far side: the probes of the side outside this room.
        let probe = |sign: f64, t: f64| [c[0] + axis[0] * sign * (half + t), c[1] + axis[1] * sign * (half + t)];
        let probes = rules.probes_ft();
        let side_out = [1.0, -1.0].into_iter().find(|s| probes.iter().all(|t| !inside(&room.outline, probe(*s, *t))));
        let Some(sign) = side_out else { continue };
        let far = probes.iter().find_map(|t| {
            let q = probe(sign, *t);
            all.iter()
                .filter(|r| r.handle != room.handle && r.level.eq_ignore_ascii_case(&room.level) && inside(&r.outline, q))
                .min_by(|a, b| a.area.total_cmp(&b.area))
        });
        // Bigger far rooms first; the outside after any room; then the wider door.
        let score = far.map_or(0.0, |r| 1.0 + r.area);
        let better = best.as_ref().is_none_or(|b| score > b.0 + 1e-9 || ((score - b.0).abs() <= 1e-9 && width > b.1));
        if better {
            best = Some((score, width, Entry { door: e.handle, at: c, to: far.map(Room::label), lo: [lo[0], lo[1]], hi: [hi[0], hi[1]] }));
        }
    }
    best.map(|b| b.2)
}

/// A room's entry door: its centre, the room past it and its plan box (building feet).
struct Entry {
    door: Handle,
    at: [f64; 2],
    to: Option<String>,
    lo: [f64; 2],
    hi: [f64; 2],
}

fn floor_of(sp: &cadaddy_walk3d::BuildingSpec, level: &str) -> f64 {
    sp.level(level).map(|l| l.elevation_ft).filter(|e| e.is_finite()).unwrap_or(0.0)
}

fn anchor_of(
    s: &mut Session,
    p: &Value,
    sc: &Scope,
    sp: &cadaddy_walk3d::BuildingSpec,
    frame: &Frame,
    rules: &Rules,
    all: &[Room],
) -> Result<Anchor> {
    let a = p.get("anchor").filter(|v| !v.is_null());
    if a.is_some_and(|a| !a.is_object()) {
        return Err(bad(CMD, "`anchor` must be an object: {corner: n} | {grid: \"A-1\"} | {name, at?}"));
    }
    let level = sc
        .rooms
        .first()
        .map(|r| r.level.clone())
        .or_else(|| sc.level.clone())
        .or_else(|| sp.levels.first().map(|l| l.name.clone()))
        .unwrap_or_default();
    let floor_ft = floor_of(sp, &level);
    let room = sc.rooms.first();
    if let Some(g) = a.and_then(|a| a.get("grid")).filter(|v| !v.is_null()) {
        let g = g
            .as_str()
            .map(str::trim)
            .filter(|g| !g.is_empty())
            .ok_or_else(|| bad(CMD, "`anchor.grid` must be an intersection's label, such as \"A-1\""))?;
        let found = s.execute("grids.detect", &json!({"level": level})).map_err(|e| bad(CMD, format!("the gridlines could not be read: {e}")))?;
        let mut labels = Vec::new();
        let norm = |t: &str| t.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase();
        for l in found["levels"].as_array().into_iter().flatten().take(100) {
            for x in l["intersections"].as_array().into_iter().flatten().take(10_000) {
                let label = x["label"].as_str().unwrap_or_default();
                if labels.len() < 12 {
                    labels.push(label.to_string());
                }
                if norm(label) == norm(g)
                    && let Some(at) = x.get("at").and_then(pair)
                {
                    let lvl = l["level"].as_str().unwrap_or(&level).to_string();
                    let ft = frame.to_ft_on(&lvl, Vec2::new(at[0], at[1]));
                    return Ok(Anchor {
                        kind: "grid_intersection",
                        name: label.to_string(),
                        floor_ft: floor_of(sp, &lvl),
                        level: lvl,
                        room: room.map(Room::label),
                        at: ft,
                        how: "gridline intersection (grids.detect)".into(),
                        entry: None,
                    });
                }
            }
        }
        return Err(bad(
            CMD,
            format!("no gridline intersection `{g}` on {level} (found: {})", if labels.is_empty() { "none".into() } else { labels.join(", ") }),
        ));
    }
    if let Some(name) = a.and_then(|a| a.get("name")).filter(|v| !v.is_null()) {
        let name = name.as_str().map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(CMD, "`anchor.name` must be text"))?.to_string();
        if let Some(at) = a.and_then(|a| a.get("at")).filter(|v| !v.is_null()) {
            let at = pair(at).ok_or_else(|| bad(CMD, "`anchor.at` must be [x, y] (building feet)"))?;
            return Ok(Anchor {
                kind: "named_point",
                name,
                level,
                room: room.map(Room::label),
                at,
                floor_ft,
                how: "named point given".into(),
                entry: None,
            });
        }
        // A plan text that says the name, on the anchor's level.
        let d = s.doc()?;
        for e in d.model.iter().take(MAX_SCAN) {
            let (text, at) = match &e.kind {
                EntityKind::Text(t) => (t.value.as_str(), t.insert.xy()),
                EntityKind::MText(t) => (t.contents.as_str(), t.insert.xy()),
                _ => continue,
            };
            if plain(text).eq_ignore_ascii_case(&name) {
                let (lvl, ft) = frame.to_ft(at);
                if lvl.eq_ignore_ascii_case(&level) || sp.levels.len() <= 1 {
                    return Ok(Anchor {
                        kind: "named_point",
                        name,
                        level,
                        room: room.map(Room::label),
                        at: ft,
                        floor_ft,
                        how: "plan text naming the point".into(),
                        entry: None,
                    });
                }
            }
        }
        return Err(bad(CMD, format!("no plan text `{name}` on {level}: give `anchor.at` [x, y] (building feet) for a named point")));
    }
    let Some(room) = room else {
        // A level or box: the box's corner, else the level's origin.
        let (at, name) = match &sc.region {
            Some((lo, _, _)) => (*lo, "box corner"),
            None => ([0.0, 0.0], "level origin"),
        };
        return Ok(Anchor {
            kind: "named_point",
            name: name.into(),
            level,
            room: None,
            at,
            floor_ft,
            how: format!("{name} (no room in the scope)"),
            entry: None,
        });
    };
    let cs = corners(&room.outline);
    if let Some(n) = a.and_then(|a| a.get("corner")).filter(|v| !v.is_null()) {
        let n = n.as_u64().filter(|n| *n >= 1).ok_or_else(|| bad(CMD, "`anchor.corner` must be a whole number from 1"))?;
        let at = usize::try_from(n - 1)
            .ok()
            .and_then(|i| cs.get(i))
            .copied()
            .ok_or_else(|| bad(CMD, format!("room {} has {} corners", room.label(), cs.len())))?;
        return Ok(Anchor {
            kind: "room_corner",
            name: format!("{} corner {n}", room.label()),
            level,
            room: Some(room.label()),
            at,
            floor_ft,
            how: "room corner given".into(),
            entry: None,
        });
    }
    if a.is_some_and(|a| a.as_object().is_some_and(|o| !o.is_empty())) {
        return Err(bad(CMD, "`anchor` takes corner, grid or name (with at)"));
    }
    // A room cut by a box (a corridor's segment): the room's corner nearest the box's middle.
    if let Some((lo, hi, _)) = &sc.region {
        let mid = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        let dist = |q: &[f64; 2]| (q[0] - mid[0]).hypot(q[1] - mid[1]);
        if let Some((i, at)) = cs.iter().enumerate().min_by(|a, b| dist(a.1).total_cmp(&dist(b.1))) {
            return Ok(Anchor {
                kind: "room_corner",
                name: format!("{} corner {}", room.label(), i + 1),
                level,
                room: Some(room.label()),
                at: *at,
                floor_ft,
                how: "the room's corner nearest the middle of the box".into(),
                entry: None,
            });
        }
    }
    let d = s.doc()?;
    let entry = entry_door(d, frame, rules, room, all);
    let (pick, how, entry_json) = match &entry {
        Some(en) => {
            // The corners of the opening itself (a room traced into its doorway) are no corner
            // a user would measure from.
            let off = |q: &[f64; 2]| q[0] < en.lo[0] - 0.25 || q[0] > en.hi[0] + 0.25 || q[1] < en.lo[1] - 0.25 || q[1] > en.hi[1] + 0.25;
            let dist = |q: &[f64; 2]| (q[0] - en.at[0]).hypot(q[1] - en.at[1]);
            let near = cs.iter().enumerate().filter(|(_, q)| off(q)).min_by(|a, b| dist(a.1).total_cmp(&dist(b.1)));
            let near = near.or_else(|| cs.iter().enumerate().min_by(|a, b| dist(a.1).total_cmp(&dist(b.1))));
            (
                near.map(|(i, _)| i),
                "the room corner nearest its entry door".to_string(),
                Some(json!({"door": en.door.hex(), "at": [r4(en.at[0]), r4(en.at[1])], "to": en.to})),
            )
        }
        // No door: the south-west corner.
        None => (
            cs.iter().enumerate().min_by(|a, b| (a.1[0] + a.1[1]).total_cmp(&(b.1[0] + b.1[1]))).map(|(i, _)| i),
            "the room's south-west corner (no door found on its outline)".to_string(),
            None,
        ),
    };
    let i = pick.unwrap_or(0);
    let at = cs.get(i).copied().unwrap_or([0.0, 0.0]);
    Ok(Anchor {
        kind: "room_corner",
        name: format!("{} corner {}", room.label(), i + 1),
        level,
        room: Some(room.label()),
        at,
        floor_ft,
        how,
        entry: entry_json,
    })
}

/// MTEXT contents without their codes (`\\P`, `{\\f…;`, braces), trimmed.
fn plain(t: &str) -> String {
    let mut out = String::new();
    let mut it = t.chars();
    while let Some(c) = it.next() {
        match c {
            '{' | '}' => {}
            '\\' => match it.next() {
                Some('P') | Some('~') => out.push(' '),
                Some('\\') => out.push('\\'),
                Some(_) => {
                    for x in it.by_ref() {
                        if x == ';' {
                            break;
                        }
                    }
                }
                None => {}
            },
            c => out.push(c),
        }
    }
    out.trim().to_string()
}

fn r4(v: f64) -> f64 {
    let r = (v * 10_000.0).round() / 10_000.0;
    // No `-0` in the files.
    if r.is_finite() && r != 0.0 { r } else { 0.0 }
}

// ─── Elements ────────────────────────────────────────────────────────────────

/// A built element in the scope.
struct Item<'a> {
    e: &'a Entity,
    class: Class,
    part: String,
    kind: String,
    level: String,
    lo: [f64; 3],
    hi: [f64; 3],
    context: bool,
    room: Option<String>,
    trade: String,
    system: String,
}

#[derive(Default)]
struct Dropped {
    small: BTreeMap<String, usize>,
    context: usize,
    not_mesh: usize,
    filtered: usize,
    capped_elements: usize,
}

fn mep_piece(e: &Entity) -> Option<String> {
    made_tag(e, "part").and_then(|p| p.strip_prefix("atom:")).map(str::to_ascii_lowercase)
}

/// What an element is to a reader: a run's piece (`box`, `strap`), a device's own kind, else the
/// model's kind, else its part.
fn kind_of(e: &Entity, part: &str) -> String {
    if let Some(piece) = mep_piece(e).filter(|p| !p.is_empty()) {
        return piece;
    }
    let base = super::elemprops::element_kind(e).unwrap_or_else(|| part.to_string());
    super::elemprops::reported_kind(e, base)
}

fn matches_filter(list: &[String], words: &[&str]) -> bool {
    list.is_empty() || list.iter().any(|w| words.iter().any(|x| !x.is_empty() && (slug(x) == slug(w) || x.eq_ignore_ascii_case(w))))
}

fn collect<'a>(
    d: &'a Drawing,
    frame: &Frame,
    sp: &cadaddy_walk3d::BuildingSpec,
    rules: &Rules,
    sc: &Scope,
    mode: &str,
    small: bool,
    dropped: &mut Dropped,
) -> Vec<Item<'a>> {
    let margin = rules.margin_ft();
    let cap = usize::try_from(rules.caps.max_elements).unwrap_or(usize::MAX).clamp(1, 1_000_000);
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_SCAN) {
        let Some(part) = made_as(e).map(|p| p.to_ascii_lowercase()) else { continue };
        if Rules::is(&rules.skip_parts.parts, &part) {
            continue;
        }
        let level = super::bspec::made_level(e).unwrap_or_default();
        if let Some(l) = &sc.level
            && !l.eq_ignore_ascii_case(&level)
            && sc.rooms.is_empty()
        {
            continue;
        }
        let Some(class) = classify(d, e) else { continue };
        let Some((lo, hi)) = box_ft(frame, &level, e) else {
            if !matches!(e.kind, EntityKind::Mesh(_)) {
                dropped.not_mesh += 1;
            }
            continue;
        };
        let (plo, phi) = ([lo[0] - margin, lo[1] - margin], [hi[0] + margin, hi[1] + margin]);
        let room = sc.rooms.iter().find(|r| r.level.eq_ignore_ascii_case(&level) && box_meets(&r.outline, plo, phi));
        if !sc.rooms.is_empty() && room.is_none() {
            continue;
        }
        if let Some((blo, bhi, z)) = &sc.region {
            if sc.level.as_deref().is_some_and(|l| !l.eq_ignore_ascii_case(&level)) {
                continue;
            }
            if hi[0] < blo[0] || lo[0] > bhi[0] || hi[1] < blo[1] || lo[1] > bhi[1] {
                continue;
            }
            let floor = floor_of(sp, &level);
            if let Some(z) = z
                && (hi[2] - floor < z[0] || lo[2] - floor > z[1])
            {
                continue;
            }
        }
        let kind = kind_of(e, &part);
        let context = Rules::is(&rules.context.parts, &part);
        let (trade, system) = rules.trade_of(&kind, &class);
        if !matches_filter(&sc.trades, &[&trade, &system, class.group, class.key, &class.category])
            || !matches_filter(&sc.kinds, &[&kind, &part, &class.category, &class.ty])
        {
            // The shell stays as context unless it is filtered out by name.
            if !(context && sc.kinds.is_empty() && mode != "off") {
                dropped.filtered += 1;
                continue;
            }
        }
        if !small {
            let piece = mep_piece(e);
            let size_m = (0..3).map(|i| (hi[i] - lo[i]) * M_PER_FT).fold(0.0, f64::max);
            let why = if Rules::is(&rules.small_parts.parts, &part) {
                Some(format!("{part}:{}", made_tag(e, "part").and_then(|p| p.strip_prefix("atom:")).unwrap_or("piece")))
            } else if let Some(pc) = piece.as_deref().filter(|pc| Rules::is(&rules.small_parts.mep_pieces, pc)) {
                Some(format!("mep:{pc}"))
            } else if rules.small_parts.max_size_mm > 0.0 && size_m * 1000.0 < rules.small_parts.max_size_mm {
                Some(format!("under {} mm: {kind}", rules.small_parts.max_size_mm))
            } else {
                None
            };
            if let Some(w) = why {
                *dropped.small.entry(w).or_default() += 1;
                continue;
            }
        }
        if context && mode == "off" {
            dropped.context += 1;
            continue;
        }
        if out.len() >= cap {
            dropped.capped_elements += 1;
            continue;
        }
        out.push(Item { e, class, part, kind, level, lo, hi, context, room: room.map(Room::label), trade, system });
    }
    out
}

/// An element's tag: the build's tag or name, else its type mark (its own, else its plan
/// object's).
fn tag_of(d: &Drawing, e: &Entity, store: &cadaddy_doc::props::Props) -> String {
    let text = |k: &str| store.get(k).filter(|p| p.has_value()).map(|p| p.text().trim().to_string()).filter(|t| !t.is_empty());
    made_tag(e, "tag")
        .map(str::to_string)
        .or_else(|| super::bimlook::made_name(e).map(str::to_string))
        .or_else(|| text("type_mark"))
        .or_else(|| cadaddy_doc::bimspec::entity_type_mark(&e.common.xdata).map(str::to_string))
        .or_else(|| {
            made_source(e).and_then(|h| d.model.get(h)).and_then(|s| cadaddy_doc::bimspec::entity_type_mark(&s.common.xdata).map(str::to_string))
        })
        .unwrap_or_default()
}

fn circuit_of(d: &Drawing, e: &Entity, store: &cadaddy_doc::props::Props) -> String {
    if let Some(c) = store.get("circuit").filter(|p| p.has_value()).map(|p| p.text().trim().to_string()).filter(|t| !t.is_empty()) {
        return c;
    }
    if let Some(c) = made_tag(e, "circuit").filter(|c| !c.is_empty()) {
        return c.to_string();
    }
    let Some(EntityKind::Insert(i)) = made_source(e).and_then(|h| d.model.get(h)).map(|s| &s.kind) else { return String::new() };
    i.attribs
        .iter()
        .take(200)
        .find(|a| matches!(a.tag.to_ascii_uppercase().as_str(), "CIRCUIT" | "CKT" | "CIRC" | "CIR"))
        .map(|a| a.text.value.trim().to_string())
        .unwrap_or_default()
}

/// The element's metadata (the keys a viewer reads).
fn meta_of(d: &Drawing, it: &Item, floor_ft: f64) -> Map<String, Value> {
    let e = it.e;
    let store = cadaddy_doc::props::read(&e.common.xdata);
    let text =
        |k: &str| store.get(k).filter(|p| p.has_value()).map(|p| p.text().trim().chars().take(200).collect::<String>()).filter(|t| !t.is_empty());
    let mut m = Map::new();
    let mut put = |k: &str, v: Value| {
        if !(v.is_null() || v.as_str().is_some_and(str::is_empty)) {
            m.insert(k.into(), v);
        }
    };
    put("id", json!(e.handle.hex()));
    put("kind", json!(it.kind));
    put("tag", json!(tag_of(d, e, &store)));
    put("circuit", json!(circuit_of(d, e, &store)));
    put("panel", json!(text("panel")));
    let bottom_in = (it.lo[2] - floor_ft) * 12.0;
    let top_in = (it.hi[2] - floor_ft) * 12.0;
    // The documented mounting height, else the model's (its bottom above the floor).
    let documented =
        text("mount_height").and_then(|t| t.trim_end_matches(['"', ' ']).trim_end_matches("in").trim().parse::<f64>().ok()).filter(|v| v.is_finite());
    let (height_in, from) = match documented {
        Some(h) => (h, "documented"),
        None => (bottom_in, "model bottom"),
    };
    if !it.context {
        put("mounting_height_in", json!(r4(height_in)));
        put("mounting_height_m", json!(r4(height_in * 0.0254)));
        put("mounting_height_from", json!(from));
        put("mount_datum", json!(text("mount_datum")));
    }
    put("bottom_aff_in", json!(r4(bottom_in)));
    put("top_aff_in", json!(r4(top_in)));
    let product = super::bom::element_product(e);
    let catalog = if product.catalog.is_empty() {
        super::bom::catalog_product(d, &product.type_mark).map(|c| c.catalog).unwrap_or_default()
    } else {
        product.catalog.clone()
    };
    put("catalog", json!(catalog));
    put("manufacturer", json!(product.manufacturer));
    put("type_mark", json!(product.type_mark));
    put("product_source", json!(product.source));
    put("trade", json!(it.trade));
    put("system", json!(it.system));
    put("category", json!(it.class.category));
    put("type", json!(it.class.ty));
    put("level", json!(it.level));
    put("room", json!(it.room));
    put("devid", json!(made_tag(e, "devid")));
    let src = made_source(e).and_then(|h| d.model.get(h).map(|s| (h, s)));
    put(
        "source",
        json!(match src {
            Some((h, s)) => format!("model.build from the plan's {} {} (layer {})", s.kind.type_name(), h.hex(), s.common.layer),
            None => format!("model.build ({}, layer {})", it.part, e.common.layer),
        }),
    );
    put("part", json!(it.part));
    if it.context {
        put("context", json!(true));
    }
    m
}

/// A node name: `kind_tag`, letters, digits and `_` only, not starting with a digit.
fn node_name(kind: &str, tag: &str) -> String {
    let raw = if tag.is_empty() { kind.to_string() } else { format!("{kind}_{tag}") };
    let mut out: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).take(60).collect();
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches('_').to_string();
    match out.chars().next() {
        None => "element".into(),
        Some(c) if c.is_ascii_digit() => format!("e_{out}"),
        _ => out,
    }
}

// ─── Geometry ────────────────────────────────────────────────────────────────

/// The output frame: building feet to metres from the anchor, Y up, plan y along −Z.
struct Place<'a> {
    frame: &'a Frame,
    anchor: [f64; 2],
    floor_ft: f64,
    /// The plan box everything is cut to (output x0, x1, z0, z1, metres), when the scope has one.
    clip: Option<[f64; 4]>,
}

impl Place<'_> {
    fn point(&self, level: &str, e: &Entity, v: cadaddy_geom::Vec3) -> Option<[f64; 3]> {
        let ft = self.frame.built_on(level, e, v.xy());
        let z = v.z / self.frame.upf() - self.floor_ft;
        let out = [(ft[0] - self.anchor[0]) * M_PER_FT, z * M_PER_FT, -(ft[1] - self.anchor[1]) * M_PER_FT];
        out.iter().all(|c| c.is_finite() && c.abs() < 1e7).then_some(out)
    }
    /// A point in building feet (z above the datum) in the output frame.
    fn of_ft(&self, ft: [f64; 3]) -> [f64; 3] {
        [(ft[0] - self.anchor[0]) * M_PER_FT, (ft[2] - self.floor_ft) * M_PER_FT, -(ft[1] - self.anchor[1]) * M_PER_FT]
    }
}

/// A polygon cut to `p[k] ≥ v` (`sign` 1) or `p[k] ≤ v` (`sign` −1).
fn clip_poly(poly: &[[f64; 3]], k: usize, v: f64, sign: f64) -> Vec<[f64; 3]> {
    let inside = |p: &[f64; 3]| (p.get(k).copied().unwrap_or(0.0) - v) * sign >= 0.0;
    let mut out = Vec::with_capacity(poly.len() + 2);
    let n = poly.len();
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + 1) % n)) else { continue };
        if inside(a) {
            out.push(*a);
        }
        if inside(a) != inside(b) {
            let (ak, bk) = (a.get(k).copied().unwrap_or(0.0), b.get(k).copied().unwrap_or(0.0));
            let t = (v - ak) / (bk - ak);
            if t.is_finite() {
                out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]);
            }
        }
    }
    out
}

/// Triangles cut to the plan box `[x0, x1, z0, z1]` (output metres); whether any was cut.
fn clip_triangles(tris: Vec<([[f64; 3]; 3], [f64; 3])>, b: [f64; 4]) -> (Vec<([[f64; 3]; 3], [f64; 3])>, bool) {
    let mut out = Vec::with_capacity(tris.len());
    let mut cut = false;
    for (t, n) in tris {
        if t.iter().all(|p| p[0] >= b[0] && p[0] <= b[1] && p[2] >= b[2] && p[2] <= b[3]) {
            out.push((t, n));
            continue;
        }
        cut = true;
        let mut poly = t.to_vec();
        for (k, v, sign) in [(0, b[0], 1.0), (0, b[1], -1.0), (2, b[2], 1.0), (2, b[3], -1.0)] {
            poly = clip_poly(&poly, k, v, sign);
            if poly.len() < 3 {
                break;
            }
        }
        let Some(first) = poly.first().copied() else { continue };
        for w in poly.get(1..).unwrap_or(&[]).windows(2) {
            if let [p, q] = w {
                let (e1, e2) = ([p[0] - first[0], p[1] - first[1], p[2] - first[2]], [q[0] - first[0], q[1] - first[1], q[2] - first[2]]);
                let c = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                if (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt() > 2e-7 {
                    out.push(([first, *p, *q], n));
                }
            }
        }
    }
    (out, cut)
}

/// A face's triangles: quads and triangles fanned, larger faces ear-clipped in their plane.
fn face_triangles(pts: &[[f64; 3]]) -> Vec<[usize; 3]> {
    let n = pts.len();
    if n < 3 {
        return Vec::new();
    }
    if n <= 4 {
        return (1..n - 1).map(|k| [0, k, k + 1]).collect();
    }
    let nrm = newell(pts);
    let (ax, ay) = if nrm[0].abs() >= nrm[1].abs() && nrm[0].abs() >= nrm[2].abs() {
        (1, 2)
    } else if nrm[1].abs() >= nrm[2].abs() {
        (2, 0)
    } else {
        (0, 1)
    };
    let flat: Vec<Vec2> = pts.iter().map(|p| Vec2::new(p.get(ax).copied().unwrap_or(0.0), p.get(ay).copied().unwrap_or(0.0))).collect();
    let tris = cadaddy_doc::triangulate(&[flat]);
    if tris.is_empty() { (1..n - 1).map(|k| [0, k, k + 1]).collect() } else { tris }
}

fn newell(pts: &[[f64; 3]]) -> [f64; 3] {
    let n = pts.len();
    let mut out = [0.0; 3];
    for i in 0..n {
        let (Some(a), Some(b)) = (pts.get(i), pts.get((i + 1) % n)) else { continue };
        out[0] += (a[1] - b[1]) * (a[2] + b[2]);
        out[1] += (a[2] - b[2]) * (a[0] + b[0]);
        out[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    out
}

fn unit(v: [f64; 3]) -> Option<[f64; 3]> {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (l.is_finite() && l > 1e-12).then(|| [v[0] / l, v[1] / l, v[2] / l])
}

/// An element's triangles in the output frame (metres), each with its face's normal.
fn element_triangles(place: &Place, level: &str, e: &Entity) -> Vec<([[f64; 3]; 3], [f64; 3])> {
    let EntityKind::Mesh(m) = &e.kind else { return Vec::new() };
    let mut out = Vec::new();
    for f in m.faces.iter().take(MAX_FACES) {
        let pts: Option<Vec<[f64; 3]>> =
            f.v.iter().take(MAX_FACE_VERTICES).map(|i| m.vertices.get(*i as usize).and_then(|v| place.point(level, e, *v))).collect();
        let Some(pts) = pts else { continue };
        let Some(n) = unit(newell(&pts)) else { continue };
        for t in face_triangles(&pts) {
            let (Some(a), Some(b), Some(c)) = (pts.get(t[0]), pts.get(t[1]), pts.get(t[2])) else { continue };
            let cr = [
                (b[1] - a[1]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[1] - a[1]),
                (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]),
                (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]),
            ];
            // Degenerate (under ~0.1 mm²) triangles are left out.
            let len = (cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]).sqrt();
            if len.is_nan() || len <= 2e-7 {
                continue;
            }
            // Wound to face the way the face's normal does.
            let tri = if cr[0] * n[0] + cr[1] * n[1] + cr[2] * n[2] >= 0.0 { [*a, *b, *c] } else { [*a, *c, *b] };
            out.push((tri, n));
        }
    }
    out
}

/// A box's triangles (an element simplified to its bounds).
fn box_triangles(lo: [f64; 3], hi: [f64; 3]) -> Vec<([[f64; 3]; 3], [f64; 3])> {
    let c =
        |x: usize, y: usize, z: usize| [if x == 0 { lo[0] } else { hi[0] }, if y == 0 { lo[1] } else { hi[1] }, if z == 0 { lo[2] } else { hi[2] }];
    let quads: [([usize; 3], [usize; 3], [usize; 3], [usize; 3], [f64; 3]); 6] = [
        ([0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0], [-1.0, 0.0, 0.0]),
        ([1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1], [1.0, 0.0, 0.0]),
        ([0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1], [0.0, -1.0, 0.0]),
        ([0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0], [0.0, 1.0, 0.0]),
        ([0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0], [0.0, 0.0, -1.0]),
        ([0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1], [0.0, 0.0, 1.0]),
    ];
    let mut out = Vec::new();
    for (a, b, cc, d, n) in quads {
        let (a, b, cc, d) = (c(a[0], a[1], a[2]), c(b[0], b[1], b[2]), c(cc[0], cc[1], cc[2]), c(d[0], d[1], d[2]));
        out.push(([a, b, cc], n));
        out.push(([a, cc, d], n));
    }
    out
}

/// Triangles to a node: its frame at their low corner and the mesh in it.
fn to_geometry(tris: &[([[f64; 3]; 3], [f64; 3])], material: usize) -> Option<([f64; 3], Geometry)> {
    if tris.is_empty() {
        return None;
    }
    let mut lo = [f64::INFINITY; 3];
    for (t, _) in tris {
        for p in t {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
            }
        }
    }
    // The frame at the shape's own low corner: one part placed twice has the same local
    // coordinates both times (to rounding), so it is one mesh.
    let mut g = Geometry { material, ..Geometry::default() };
    for (t, n) in tris {
        let base = u32::try_from(g.positions.len()).ok()?;
        for p in t {
            g.positions.push([(p[0] - lo[0]) as f32, (p[1] - lo[1]) as f32, (p[2] - lo[2]) as f32]);
            g.normals.push([n[0] as f32, n[1] as f32, n[2] as f32]);
        }
        g.indices.extend([base, base + 1, base + 2]);
    }
    weld(&mut g);
    Some((lo, g))
}

/// Shares a face's corners between its triangles (same point and normal).
fn weld(g: &mut Geometry) {
    let key = |p: &[f32; 3], n: &[f32; 3]| (p.map(f32::to_bits), n.map(f32::to_bits));
    let mut seen: HashMap<([u32; 3], [u32; 3]), u32> = HashMap::new();
    let (mut pos, mut nrm) = (Vec::new(), Vec::new());
    let mut remap = Vec::with_capacity(g.positions.len());
    for (p, n) in g.positions.iter().zip(&g.normals) {
        let next = u32::try_from(pos.len()).unwrap_or(u32::MAX);
        let i = *seen.entry(key(p, n)).or_insert_with(|| {
            pos.push(*p);
            nrm.push(*n);
            next
        });
        remap.push(i);
    }
    for ix in &mut g.indices {
        *ix = remap.get(*ix as usize).copied().unwrap_or(0);
    }
    g.positions = pos;
    g.normals = nrm;
}

/// Quantized shape for instancing: 0.1 mm grid.
fn shape_key(g: &Geometry) -> Vec<i64> {
    let q = |v: f32| (f64::from(v) * 10_000.0).round() as i64;
    let mut k = Vec::with_capacity(g.positions.len() * 6 + g.indices.len() + 1);
    k.push(g.material as i64);
    for (p, n) in g.positions.iter().zip(&g.normals) {
        k.extend(p.iter().map(|v| q(*v)));
        k.extend(n.iter().map(|v| (f64::from(*v) * 1000.0).round() as i64));
    }
    k.extend(g.indices.iter().map(|i| i64::from(*i)));
    k
}

// ─── Building the scene ──────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    Full,
    ContextBoxes,
    ContextOff,
    /// The far elements dropped (nearest the anchor kept).
    Far,
}

struct Built {
    scene: Scene,
    dropped_far: BTreeMap<String, usize>,
    /// Elements cut at the scope's plan box (a corridor wall running past the room).
    clipped: usize,
}

fn materials_for(items: &[Item], rules: &Rules, mode: &str) -> (Vec<Material>, Vec<usize>) {
    let mut mats: Vec<Material> = Vec::new();
    let mut of = Vec::with_capacity(items.len());
    let opacity = if mode == "solid" { 1.0 } else { rules.opacity() as f32 };
    for it in items {
        let rgb = rules.rgb(&it.system, &it.trade);
        let op = if it.context { opacity } else { 1.0 };
        let name = format!("{}{}", it.system, if it.context && op < 1.0 { "_context" } else { "" });
        let i = match mats.iter().position(|m| m.name == name) {
            Some(i) => i,
            None => {
                mats.push(Material { name, rgb, opacity: op });
                mats.len() - 1
            }
        };
        of.push(i);
    }
    (mats, of)
}

fn build_scene(d: &Drawing, items: &[Item], place: &Place, rules: &Rules, mode: &str, step: Step, tri_cap: usize) -> Built {
    let (materials, mat_of) = materials_for(items, rules, mode);
    let mut scene = Scene { materials, ..Scene::default() };
    let mut shapes: HashMap<Vec<i64>, usize> = HashMap::new();
    let mut names: HashMap<String, usize> = HashMap::new();
    // Nearest the anchor first, so a cap drops the far ones.
    let mut order: Vec<usize> = (0..items.len()).collect();
    let dist = |it: &Item| {
        let cx = (it.lo[0] + it.hi[0]) / 2.0 - place.anchor[0];
        let cy = (it.lo[1] + it.hi[1]) / 2.0 - place.anchor[1];
        cx.hypot(cy)
    };
    order.sort_by(|a, b| {
        let (ia, ib) = (items.get(*a), items.get(*b));
        ia.map(|x| x.context).cmp(&ib.map(|x| x.context)).then_with(|| ia.map_or(0.0, dist).total_cmp(&ib.map_or(0.0, dist)))
    });
    let mut drawn = 0usize;
    let mut clipped = 0usize;
    let mut dropped_far: BTreeMap<String, usize> = BTreeMap::new();
    for i in order {
        let (Some(it), Some(&mat)) = (items.get(i), mat_of.get(i)) else { continue };
        if it.context && step >= Step::ContextOff {
            *dropped_far.entry("context (over the caps)".into()).or_default() += 1;
            continue;
        }
        let as_box = it.context && step >= Step::ContextBoxes;
        let tris = if as_box {
            let (a, b) = (place.of_ft(it.lo), place.of_ft(it.hi));
            box_triangles([a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])], [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])])
        } else {
            element_triangles(place, &it.level, it.e)
        };
        let (tris, cut) = match place.clip {
            Some(b) => clip_triangles(tris, b),
            None => (tris, false),
        };
        let Some((at, g)) = to_geometry(&tris, mat) else { continue };
        if drawn + g.triangles() > tri_cap {
            *dropped_far.entry(it.kind.clone()).or_default() += 1;
            continue;
        }
        drawn += g.triangles();
        let key = shape_key(&g);
        let gi = match shapes.get(&key) {
            Some(gi) => *gi,
            None => {
                scene.geometries.push(g);
                let gi = scene.geometries.len() - 1;
                shapes.insert(key, gi);
                gi
            }
        };
        let mut meta = meta_of(d, it, place.floor_ft);
        let base = node_name(&it.kind, meta.get("tag").and_then(Value::as_str).unwrap_or_default());
        let n = names.entry(base.clone()).or_insert(0);
        *n += 1;
        let name = if *n == 1 { base } else { format!("{base}_{n}") };
        meta.insert("node".into(), json!(name));
        if as_box {
            meta.insert("simplified".into(), json!("box"));
        }
        if cut {
            meta.insert("cut_to_scope".into(), json!(true));
            clipped += 1;
        }
        scene.nodes.push(Node { name, geometry: gi, translation: at, meta });
    }
    Built { scene, dropped_far, clipped }
}

// ─── The command ─────────────────────────────────────────────────────────────

/// The files to write: NAME.glb / NAME.usdz / NAME.json from `path` (any of those extensions, or
/// none).
fn stem_of(s: &Session, p: &Value, scope_name: &str) -> Result<(String, bool)> {
    if let Some(x) = str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()) {
        let pb = std::path::Path::new(x);
        let ext = pb.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let stem = if matches!(ext.as_str(), "glb" | "usdz" | "json") {
            pb.with_extension("").to_string_lossy().into_owned()
        } else if ext.is_empty() {
            x.to_string()
        } else {
            return Err(bad(CMD, format!("`path` ends in .{ext}: give NAME, NAME.glb, NAME.usdz or NAME.json")));
        };
        return Ok((stem, false));
    }
    let st = s.state()?;
    let clean: String = scope_name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).take(40).collect();
    let base = match &st.path {
        Some(path) => {
            let pb = std::path::Path::new(path);
            let stem = pb.file_stem().map_or("Drawing".into(), |s| s.to_string_lossy().into_owned());
            pb.with_file_name(format!("{stem}-ar-{clean}")).to_string_lossy().into_owned()
        }
        None => format!("{}-ar-{clean}", std::path::Path::new(&st.title).file_stem().map_or("Drawing".into(), |s| s.to_string_lossy().into_owned())),
    };
    // Never over another export: -2, -3… until all three names are free.
    for n in 1..=1000 {
        let stem = if n == 1 { base.clone() } else { format!("{base}-{n}") };
        if ["glb", "usdz", "json"].iter().all(|e| !std::path::Path::new(&format!("{stem}.{e}")).exists()) {
            return Ok((stem, n > 1));
        }
    }
    Ok((base, false))
}

fn formats(p: &Value) -> Result<(bool, bool)> {
    let f = list_param(p, "formats")?;
    if f.is_empty() {
        return Ok((true, true));
    }
    let (mut glb, mut usdz) = (false, false);
    for x in &f {
        match x.to_ascii_lowercase().trim_start_matches('.') {
            "glb" | "gltf" => glb = true,
            "usdz" | "usd" => usdz = true,
            other => return Err(bad(CMD, format!("format `{other}`: glb or usdz"))),
        }
    }
    Ok((glb, usdz))
}

fn cap_param(p: &Value, key: &str, rule: u64) -> Result<u64> {
    let rule = if rule == 0 { u64::MAX } else { rule };
    Ok(count_param(CMD, p, key, 1, u64::MAX)?.map_or(rule, |v| v.min(rule)))
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let started = web_time::Instant::now();
    let (want_glb, want_usdz) = formats(p)?;
    let mode = match str_param(p, "context").map(str::trim).map(str::to_ascii_lowercase) {
        None => None,
        Some(m) if matches!(m.as_str(), "translucent" | "solid" | "off") => Some(m),
        Some(m) => return Err(bad(CMD, format!("context `{m}`: translucent, solid or off"))),
    };
    if p.get("context").is_some_and(|v| !v.is_null() && !v.is_string()) {
        return Err(bad(CMD, "`context` must be translucent, solid or off"));
    }
    let small = bool_param(CMD, p, "small")?.unwrap_or(false);
    let clip = bool_param(CMD, p, "clip")?.unwrap_or(true);
    // The paths first: nothing is read or written for a refused file name.
    if let Some(x) = str_param(p, "path").map(str::trim).filter(|x| !x.is_empty()) {
        super::exchange::guard_output(s, CMD, p, x)?;
    }
    let (sp, _) = super::bspec::current(s, CMD)?;
    let d = s.doc()?;
    if !super::bimlook::has_model(d) {
        return Err(bad(CMD, "nothing is built yet: run model.build first"));
    }
    let (rules, own_rules) = Rules::of(d);
    let mode = mode.unwrap_or_else(|| {
        Some(rules.context.mode.to_ascii_lowercase())
            .filter(|m| matches!(m.as_str(), "translucent" | "solid" | "off"))
            .unwrap_or_else(|| "translucent".into())
    });
    let max_tris = cap_param(p, "max_triangles", rules.caps.max_triangles)?;
    let max_bytes = cap_param(p, "max_bytes", rules.caps.max_bytes)?;
    let frame = Frame::of_spec(d, &sp);
    let all = rooms(d, &frame);
    let sc = scope_of(p, &all, &sp)?;
    let anchor = anchor_of(s, p, &sc, &sp, &frame, &rules, &all)?;
    let scope_name = match (sc.rooms.as_slice(), &sc.level) {
        ([r], _) => r.label(),
        ([r, ..], _) => format!("{}-and-{}-more", r.label(), sc.rooms.len() - 1),
        ([], Some(l)) => l.clone(),
        ([], None) => "model".into(),
    };
    let (stem, renamed) = stem_of(s, p, &scope_name)?;
    let paths: Vec<String> =
        [(want_glb, "glb"), (want_usdz, "usdz"), (true, "json")].iter().filter(|(w, _)| *w).map(|(_, e)| format!("{stem}.{e}")).collect();
    for path in &paths {
        super::exchange::guard_output(s, CMD, p, path)?;
    }
    let d = s.doc()?;
    let mut dropped = Dropped::default();
    let items = collect(d, &frame, &sp, &rules, &sc, &mode, small, &mut dropped);
    if items.is_empty() {
        return Err(bad(CMD, format!("nothing built in that scope ({}): check the room, level, box and filters", scope_name)));
    }
    // Cut to the scope's plan box: the rooms (and the margin), else the box; a level is whole.
    let clip_ft = if clip { clip_box(&sc, rules.margin_ft()) } else { None };
    let clip_m = clip_ft.map(|(lo, hi)| {
        let (ax, ay) = (anchor.at[0], anchor.at[1]);
        [(lo[0] - ax) * M_PER_FT, (hi[0] - ax) * M_PER_FT, -(hi[1] - ay) * M_PER_FT, -(lo[1] - ay) * M_PER_FT]
    });
    let place = Place { frame: &frame, anchor: anchor.at, floor_ft: anchor.floor_ft, clip: clip_m };
    let stem_name = std::path::Path::new(&stem).file_name().map_or("model".into(), |n| n.to_string_lossy().into_owned());
    // Over a cap: the context as boxes, then none, then the elements furthest from the anchor
    // (the triangle cap drops those as the scene is built, nearest first).
    let mut step = Step::Full;
    let mut tri_cap = usize::try_from(max_tris).unwrap_or(usize::MAX);
    let mut tries = 0;
    let (built, glb_bytes, usdz_bytes, sidecar) = loop {
        tries += 1;
        let cap = if step == Step::Far { tri_cap } else { usize::MAX };
        let mut built = build_scene(d, &items, &place, &rules, &mode, step, cap);
        if built.scene.drawn_triangles() > tri_cap && step < Step::Far {
            step = next_step(step);
            continue;
        }
        if built.scene.nodes.is_empty() {
            return Err(bad(CMD, format!("nothing in that scope fits the caps ({max_tris} triangles, {max_bytes} bytes)")));
        }
        let summary = summary(&anchor, &sc, &rules, own_rules, &mode, small);
        built.scene.info = summary.clone();
        let glb_bytes = if want_glb { Some(glb::write(&built.scene)) } else { None };
        let usdz_bytes = if want_usdz { Some(usdz::write(&built.scene)) } else { None };
        let size = glb_bytes.as_ref().map_or(0, Vec::len).max(usdz_bytes.as_ref().map_or(0, Vec::len));
        let size = u64::try_from(size).unwrap_or(u64::MAX);
        if size > max_bytes {
            if step < Step::Far {
                step = next_step(step);
                continue;
            }
            // Fewer triangles in proportion, a few times at most.
            if tries < 12 {
                let ratio = max_bytes as f64 / size as f64 * 0.9;
                tri_cap = ((built.scene.drawn_triangles() as f64) * ratio).floor() as usize;
                continue;
            }
            return Err(bad(CMD, format!("the export is {size} bytes, over the {max_bytes}-byte cap: choose a smaller scope")));
        }
        let points = reference_points(&sc, &place);
        let sidecar = sidecar_json(&built, &summary, &points, &stem_name, want_glb, want_usdz, &dropped, step);
        break (built, glb_bytes, usdz_bytes, sidecar);
    };
    let sidecar_text = serde_json::to_string_pretty(&sidecar).map_err(|e| bad(CMD, e.to_string()))?;
    let mut files = Map::new();
    let mut bytes = Map::new();
    for (ext, data) in [("glb", glb_bytes.as_deref()), ("usdz", usdz_bytes.as_deref()), ("json", Some(sidecar_text.as_bytes()))] {
        let Some(data) = data else { continue };
        let path = format!("{stem}.{ext}");
        crate::writefile::write_atomic(&path, data).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        let key = if ext == "json" { "sidecar" } else { ext };
        files.insert(key.into(), json!(path));
        bytes.insert(key.into(), json!(data.len()));
    }
    if renamed {
        s.echo("An AR export of that name already exists, so it was kept.".to_string());
    }
    let scene = &built.scene;
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut trades: BTreeMap<String, usize> = BTreeMap::new();
    for n in &scene.nodes {
        *kinds.entry(n.meta.get("kind").and_then(Value::as_str).unwrap_or("element").to_string()).or_default() += 1;
        *trades.entry(n.meta.get("trade").and_then(Value::as_str).unwrap_or("other").to_string()).or_default() += 1;
    }
    let seconds = (started.elapsed().as_secs_f64() * 100.0).round() / 100.0;
    s.echo(format!(
        "AR export: {} objects, {} triangles → {}",
        scene.nodes.len(),
        scene.drawn_triangles(),
        files.values().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
    ));
    Ok(json!({
        "files": files,
        "bytes": bytes,
        "nodes": scene.nodes.len(),
        "meshes": scene.geometries.len(),
        "triangles": scene.drawn_triangles(),
        "mesh_triangles": scene.geometries.iter().map(Geometry::triangles).sum::<usize>(),
        "kinds": kinds,
        "trades": trades,
        "anchor": sidecar["anchor"],
        "scope": sidecar["scope"],
        "dropped": sidecar["dropped"],
        "simplified": sidecar["simplified"],
        "cut_to_scope": sidecar["cut_to_scope"],
        "seconds": seconds,
        "checks": {
            "usdz": "uncompressed zip, 64-byte aligned, root layer first (checked on write)",
            "usdchecker": "not checkable here — the coordinator checks on the Mac",
        },
    }))
}

/// The plan box (building feet) a scope cuts to: its rooms' (grown by `margin`), within its box.
fn clip_box(sc: &Scope, margin: f64) -> Option<([f64; 2], [f64; 2])> {
    let mut b: Option<([f64; 2], [f64; 2])> = None;
    for p in sc.rooms.iter().flat_map(|r| r.outline.iter()) {
        let (lo, hi) = b.get_or_insert((*p, *p));
        *lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        *hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    let b = b.map(|(lo, hi)| ([lo[0] - margin, lo[1] - margin], [hi[0] + margin, hi[1] + margin]));
    match (b, &sc.region) {
        (Some((lo, hi)), Some((rlo, rhi, _))) => {
            let (lo, hi) = ([lo[0].max(rlo[0]), lo[1].max(rlo[1])], [hi[0].min(rhi[0]), hi[1].min(rhi[1])]);
            (lo[0] < hi[0] && lo[1] < hi[1]).then_some((lo, hi))
        }
        (Some(b), None) => Some(b),
        (None, Some((rlo, rhi, _))) => Some((*rlo, *rhi)),
        (None, None) => None,
    }
}

fn next_step(s: Step) -> Step {
    match s {
        Step::Full => Step::ContextBoxes,
        Step::ContextBoxes => Step::ContextOff,
        Step::ContextOff | Step::Far => Step::Far,
    }
}

/// The anchor, units and scope: in both files and the sidecar.
fn summary(a: &Anchor, sc: &Scope, rules: &Rules, own_rules: bool, mode: &str, small: bool) -> Value {
    json!({
        "schema": SCHEMA,
        "generator": "CADaddy bim.export.ar",
        "units": "m",
        "meters_per_unit": 1,
        "up_axis": "Y",
        "axes": "x = plan x (east), y = up from the anchor level's finished floor, z = minus plan y",
        "anchor": {
            "kind": a.kind,
            "name": a.name,
            "level": a.level,
            "room": a.room,
            "plan_x": r4(a.at[0]),
            "plan_y": r4(a.at[1]),
            "plan_units": "ft",
            "floor_elevation_ft": r4(a.floor_ft),
            "how": a.how,
            "entry": a.entry,
            "model": [0, 0, 0],
        },
        "scope": {
            "rooms": sc.rooms.iter().map(|r| json!({"label": r.label(), "number": r.number, "name": r.name, "level": r.level})).collect::<Vec<_>>(),
            "level": sc.level,
            "box": sc.region.as_ref().map(|(lo, hi, z)| json!({"min": lo, "max": hi, "z": z})),
            "trades": sc.trades,
            "kinds": sc.kinds,
            "default_room": sc.defaulted,
            "context": mode,
            "small_parts": small,
        },
        "rules": {"source": if own_rules { "job (building spec `ar_export`) over the shipped rules" } else { "shipped rules (arexport.json)" }, "version": rules.version},
    })
}

/// Named points a viewer can align on: the scope rooms' corners (metres, floor level).
fn reference_points(sc: &Scope, place: &Place) -> Vec<Value> {
    let mut out = Vec::new();
    for r in &sc.rooms {
        for (i, c) in corners(&r.outline).iter().enumerate() {
            let pos = [(c[0] - place.anchor[0]) * M_PER_FT, 0.0, -(c[1] - place.anchor[1]) * M_PER_FT];
            out.push(json!({"name": format!("{} corner {}", r.label(), i + 1), "kind": "room_corner", "level": r.level, "plan": [r4(c[0]), r4(c[1])], "position": pos.map(r4)}));
        }
        if out.len() >= 1_000 {
            break;
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn sidecar_json(built: &Built, summary: &Value, points: &[Value], stem: &str, glb: bool, usdz: bool, dropped: &Dropped, step: Step) -> Value {
    let scene = &built.scene;
    let mut v = summary.clone();
    v["version"] = json!(1);
    v["model"] = json!(if usdz { format!("{stem}.usdz") } else { format!("{stem}.glb") });
    v["glb"] = if glb { json!(format!("{stem}.glb")) } else { Value::Null };
    v["usdz"] = if usdz { json!(format!("{stem}.usdz")) } else { Value::Null };
    v["counts"] = json!({"nodes": scene.nodes.len(), "meshes": scene.geometries.len(), "triangles": scene.drawn_triangles()});
    v["dropped"] = json!({
        "small_parts": dropped.small,
        "context_off": dropped.context,
        "filtered": dropped.filtered,
        "not_meshes": dropped.not_mesh,
        "over_element_cap": dropped.capped_elements,
        "over_caps": built.dropped_far,
    });
    v["cut_to_scope"] = json!(built.clipped);
    v["simplified"] = json!(match step {
        Step::Full => "nothing",
        Step::ContextBoxes => "context as boxes",
        Step::ContextOff => "context left out",
        Step::Far => "context left out, the elements furthest from the anchor left out",
    });
    v["points"] = json!(points);
    v["elements"] = json!(
        scene
            .nodes
            .iter()
            .map(|n| {
                let mut m = n.meta.clone();
                m.insert("position".into(), json!(n.translation.map(r4)));
                Value::Object(m)
            })
            .collect::<Vec<_>>()
    );
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_rules_read() {
        let r: Rules = serde_json::from_str(RULES).unwrap_or_default();
        assert_eq!(r.caps.max_triangles, 300_000);
        assert_eq!(r.caps.max_bytes, 50_000_000);
        assert!(r.small_parts.mep_pieces.iter().any(|p| p == "strap"));
        assert!(r.context.parts.iter().any(|p| p == "wall"));
        assert!(r.colors.by_system.contains_key("devices"));
        assert_eq!(r.margin_ft(), 1.0);
    }

    #[test]
    fn node_names_are_identifiers() {
        assert_eq!(node_name("receptacle", "R-1"), "receptacle_R_1");
        assert_eq!(node_name("12 in duct", ""), "e_12_in_duct");
        assert_eq!(node_name("", ""), "element");
        assert_eq!(node_name("light_fixture", "F1 (a)"), "light_fixture_F1_a");
    }

    #[test]
    fn a_box_meets_a_polygon_it_overlaps_only() {
        let sq = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!(box_meets(&sq, [9.0, 9.0], [12.0, 12.0]));
        assert!(box_meets(&sq, [-1.0, 4.0], [11.0, 5.0]));
        assert!(box_meets(&sq, [2.0, 2.0], [3.0, 3.0]));
        assert!(!box_meets(&sq, [11.0, 0.0], [12.0, 10.0]));
    }

    #[test]
    fn an_l_shaped_face_is_ear_clipped() {
        let l = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, 2.0, 0.0], [0.0, 2.0, 0.0]];
        let t = face_triangles(&l);
        assert_eq!(t.len(), 4);
        let area: f64 = t
            .iter()
            .map(|t| {
                let (a, b, c) = (l[t[0]], l[t[1]], l[t[2]]);
                ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
            })
            .sum();
        assert!((area - 3.0).abs() < 1e-9, "{area}");
    }

    #[test]
    fn corners_skip_straight_vertices() {
        let poly = [[0.0, 0.0], [5.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert_eq!(corners(&poly).len(), 4);
    }

    #[test]
    fn welded_box_has_24_corners() {
        let tris = box_triangles([0.0; 3], [1.0, 2.0, 3.0]);
        let (_, g) = to_geometry(&tris, 0).unwrap();
        assert_eq!(g.triangles(), 12);
        assert_eq!(g.positions.len(), 24);
    }

    #[test]
    fn mtext_codes_are_dropped() {
        assert_eq!(plain("{\\fArial|b1;CP-1}"), "CP-1");
        assert_eq!(plain("A\\PB"), "A B");
    }
}
