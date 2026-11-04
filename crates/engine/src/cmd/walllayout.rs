//! Walls you lay out from (Q6174 walllayout): named wall faces, devices placed relative to a face,
//! and the field layout of a room.
//!
//! - **Faces** (`bim.faces`, [`faces`]): every built wall's faces, named by the room each faces and
//!   its compass side (`1201/S`; a demising wall is `1201/S` and `1203/N`), each with its wall type,
//!   length, height, openings and the faces at its two ends.
//! - **Relative placement** (`bim.layout.rehost`): each wall device's `host_face`, its height above
//!   the floor to a stated point (`height_aff` to `height_ref`: centre, or top/bottom when the
//!   documents say so), its along-face offset from a named reference face (`offset_from`,
//!   `offset_along`: from the document's dimension when one gives it, else measured from the plan
//!   and marked derived) and its depth from the finished face (`offset_depth`: flush 0, proud +,
//!   recessed −). x,y,z are derived from these: `bim.layout.follow` re-derives them after a wall
//!   moves or is re-typed, or a cabinet the device is tied to moves.
//! - **Constraints**: a placement may be tied to another object (a cabinet, an appliance) with a
//!   tolerance band — `{ref, ref_point, axis, target, plus_minus, inside}`. They come from the
//!   documents (`bim.layout.tie`, a person or an agent reading the elevation or the
//!   submittal's rough-in) or from the rules data (`layout_rules.json`: a refrigerator's
//!   receptacle is concealed behind it, a microwave's inside its cabinet, a dishwasher's in the
//!   sink base beside it), marked derived.
//! - **Faceplates**: a device's extent is its full plate (style, gangs, outer size from its product
//!   words, else the rules data's sizes, derived); ganged devices share one plate. Every fit and
//!   clearance is measured from the plate's edges.
//! - **Height is hard, along is soft**: a documented AFF is fixed within a tight tolerance; the
//!   along-wall position is a target with a drift band, solved to a stud side clear of pipes once
//!   the framing exists as objects ([`solve`]); until then the solve is `pending framing`.
//! - **G-LAYOUT** ([`checks`]): the prescreen's check of all of the above per device.
//! - **`bim.layout {room}`**: each face with its devices in field terms, and optionally a layout
//!   sheet with dimension strings from the reference faces.
//!
//! General: nothing here knows a job; sizes and bands are rules data, everything else is read
//! from the model and the documents.

mod drawn;
mod faces;
mod solve;

use std::collections::{BTreeMap, HashMap, HashSet};

use cadaddy_doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_doc::{Common, DimKind, Drawing, Entity, EntityKind, Handle, Line, RawTag, Space, Text};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_walk3d::BuildingSpec;
use serde_json::{Value, json};

pub(crate) use faces::Face;
use faces::{Piece, RoomRef};
use solve::{Obstacle, Plate, Rect, Solved};

use super::propfill::WithNote;
use super::*;
use crate::{Result, Session};

pub const FACES: &str = "bim.faces";
pub const REHOST: &str = "bim.layout.rehost";
pub const FOLLOW: &str = "bim.layout.follow";
pub const LAYOUT: &str = "bim.layout";
pub const CONSTRAIN: &str = "bim.layout.tie";
pub const OBJECT: &str = "bim.layout.object";
pub const DIMS: &str = "bim.layout.dims";
/// The prescreen rule.
pub const RULE: &str = "G-LAYOUT";

/// The placement fields this fills on every wall device (the object schema's template fields;
/// each with its source: documented, computed, or derived).
pub const LAYOUT_FIELDS: [&str; 6] = ["host_face", "height_ref", "height_aff", "offset_from", "offset_along", "offset_depth"];

/// Elements and devices looked at, at most; faces and devices listed.
const MAX_ELEMENTS: usize = 500_000;
const MAX_LISTED: usize = 50;
/// A device's face is looked for this far in front of a wall face and behind it past the wall
/// (inches).
const FACE_FRONT_IN: f64 = 6.0;
const FACE_BACK_IN: f64 = 3.0;
/// A device with no stored host takes a face this close (inches).
const FACE_NEAR_IN: f64 = 6.0;
/// A dimension's end is at a device, or on a face, this close (inches).
const DIM_AT_IN: f64 = 6.0;
const DIM_ON_IN: f64 = 2.0;
/// An object is against a face when its back is this close to it (inches), and a device is
/// tied to it when the device's plate is this close to its extent along the face.
const OBJ_BACK_IN: f64 = 12.0;
const OBJ_NEAR_IN: f64 = 6.0;
/// The layout sheet's scale (paper inches per model inch: 1/2" = 1'-0").
const SHEET_SCALE: f64 = 1.0 / 24.0;
/// Devices this close along a face (inches) are at one spot: one gang.
const STACKED_IN: f64 = 1.0;
/// A wall device's `host_face` when no named face was found (the object schema's n/a).
pub const NO_FACE: &str = "n/a";
/// Constraint slots per device.
const MAX_CONSTRAINTS: usize = 8;
/// The app tag casework and appliance objects carry (`casework.read`'s).
use super::casework::CASEWORK_APP;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(FACES, "Wall Faces", run_faces)
            .params("{room?: number or name, level?, wall?: handle, north?: degrees counter-clockwise from +Y (plan north; default 0), limit? (200)} → {count, by_level, plan_north, faces: [{name, room, side, level, wall, walls, wall_type, length_in, height_in, openings: [{kind: door|window|opening, from_in, to_in, bottom_in, top_in}], prev, next, skew_deg, order}]}: every built wall's faces named by the room each faces and its compass side (`1201/S`; a demising wall is `1201/S` on one face and `1203/N` on the other; a skewed or second face on a side gets a number, `1201/S2`)")
            .menu(&["Tools", "Building", "Wall Faces"])
            .noundo(),
        CommandSpec::new(REHOST, "Place Devices on Wall Faces", run_rehost)
            .params("{north?: degrees (plan north, default 0), plate_style?: standard|midsize|jumbo (the project's plate when no document names one)} → {faces, by_level, devices: {wall, on_face, no_face, no_face_first}, offsets: {documented, measured}, heights: {source: n}, constraints: {rule: n} (a tie whose object is not in the model: `pending object (<kind>)`), plates: {by_style, ganged, ganged_first: [{plate, gangs, devices, span_in, spacing_in, why}] (devices whose plates would overlap at one height: one box, derived), stacked}, solve: {pending framing|solved|conflict: n}, fields}: each wall device placed relative to its named face — host_face, height_aff to height_ref, offset_along from offset_from (a document's dimension, else measured from the plan, marked derived), offset_depth from the finished face — with its faceplate, its tolerance band and constraints (the rules data's concealed-behind appliances, a cabinet it sits in)")
            .menu(&["Tools", "Building", "Place Devices on Wall Faces"]),
        CommandSpec::new(FOLLOW, "Devices Follow Their Walls", run_follow)
            .params("{} → {moved, kept, lost: [{handle, why}]}: re-derives each wall device's x,y,z from its face, its reference face or object, its height and depth (after a wall moved or was re-typed, or a cabinet it is tied to moved); its dimensions stay")
            .menu(&["Tools", "Building", "Devices Follow Their Walls"]),
        CommandSpec::new(LAYOUT, "Room Layout", run_layout)
            .params("{room: number or name, level?, draw?: bool (a layout sheet with each face unfolded and dimension strings from its reference faces), north?} → {room, level, faces: [{name, wall_type, length, openings, devices: [{handle, text, ...}]}], text, sheet?}: the room's faces with their devices in field terms (\"1201/S: REC 18 in AFF ctr (hard); 15 in from 1201/E target, band ±6 in, pending framing; plate standard 1-gang 2.75 x 4.5 in\")"),
        CommandSpec::new(CONSTRAIN, "Tie a Device to an Object", run_constrain)
            .params("{handle: device, ref: object handle or tag, ref_point?: center|edge|face (center), axis?: along|vertical|both (along), target_in?: 0, plus_minus_in: number, inside?: bool, follow?: bool (true: it moves with the object), source?: where the documents say it (an elevation, a shop drawing, a submittal's rough-in)} → {handle, constraint}: ties a device's placement to another object with a tolerance band; G-LAYOUT checks it and bim.layout.follow keeps it"),
        CommandSpec::new(DIMS, "Read Drawn Dimensions", run_dims)
            .params("{sheet?: number, kind?: device|face|opening|casework|none (list only the dimensions with an end on that), north?, limit?: (20)} → {read, placed, devices_documented, devices_measured_not_fixed: {why: n}, conflicts, conflicts_first: [{handle, face, from, drawn_in, model_in, off_in, band_in, sheet, at}], by_sheet: {<sheet>: {values, read, placed, unplaced: {why: n}, values_unused: {why: n}, stretches_without_value, chains, baseline_groups, not_to_scale, scale_in_per_unit, registration, attached: {both_ends, one_end, none}, ends: {device centre|wall face|opening edge|casework edge: n}, unattached_why: {why: n}}}, dimensions_first: [{sheet, view, level, value, inches, on_sheet, model_ft, marks, to_scale, chain, baseline, ends}]}: the sheets' dimensions drawn as line work and text (a PDF's: dimension line, ticks, dots or arrows, extension lines, the value; chains and baselines) read, each witness point mapped through its sheet's registration onto the model and attached to what it measures: a device's centre, a wall face, an opening's edge, a casework edge. bim.layout.rehost documents a device's along-face offset from them; where the model's position is off the drawn value by more than the kind's band, that is a conflict for review")
            .menu(&["Tools", "Building", "Read Drawn Dimensions"])
            .noundo(),
        CommandSpec::new(OBJECT, "Add Casework or Appliance Object", run_object)
            .params("{kind: string (base_cabinet, upper_cabinet, sink_base, vanity, refrigerator, range, dishwasher, microwave, washer, dryer, tv or another casework or appliance kind), tag?, level?, box: [x0, y0, z0, x1, y1, z1] (drawing units, model), source?: where it was read (a submittal, an elevation)} → {handle}: an object other placements can be tied to (a box with its kind, tag and source)"),
    ]
}

// ─── Small helpers ──────────────────────────────────────────────────────────

fn upf_of(sp: &BuildingSpec) -> f64 {
    if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 }
}

fn level_floor(sp: &BuildingSpec, level: Option<&str>, upf: f64) -> f64 {
    level.and_then(|l| sp.level(l)).map_or(0.0, |l| l.elevation_ft * upf)
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn dot(a: Vec2, b: Vec2) -> f64 {
    a.x * b.x + a.y * b.y
}

/// A length in field terms: `15 in`, `15 1/2 in`, `9 ft 2 in` (quarter inches).
pub(crate) fn ft_in(inches: f64) -> String {
    if !inches.is_finite() {
        return "?".into();
    }
    let neg = inches < 0.0;
    let q = (inches.abs() * 4.0).round() as i64;
    let (whole, frac) = (q / 4, q % 4);
    let fr = match frac {
        1 => " 1/4",
        2 => " 1/2",
        3 => " 3/4",
        _ => "",
    };
    let s = if whole >= 24 {
        let (ft, i) = (whole / 12, whole % 12);
        if i == 0 && fr.is_empty() { format!("{ft} ft") } else { format!("{ft} ft {i}{fr} in") }
    } else {
        format!("{whole}{fr} in")
    };
    if neg { format!("-{s}") } else { s }
}

/// A height in inches as the field says it: `18 in`, `66 in`, `46 1/2 in`.
pub(crate) fn in_text(inches: f64) -> String {
    if !inches.is_finite() {
        return "?".into();
    }
    let q = (inches.abs() * 4.0).round() as i64;
    let fr = match q % 4 {
        1 => " 1/4",
        2 => " 1/2",
        3 => " 3/4",
        _ => "",
    };
    format!("{}{}{fr} in", if inches < 0.0 { "-" } else { "" }, q / 4)
}

fn parse_vec(t: &str) -> Option<Vec2> {
    let (a, b) = t.split_once(',')?;
    let v = Vec2::new(a.trim().parse().ok()?, b.trim().parse().ok()?);
    (v.is_finite() && v.len() > 1e-9).then(|| v * (1.0 / v.len()))
}

fn vec_text(v: Vec2) -> String {
    format!("{:.3},{:.3}", v.x, v.y)
}

fn prop_num(store: &Props, key: &str) -> Option<f64> {
    store.get(key).and_then(|p| p.value.as_f64().or_else(|| p.text().trim().parse().ok())).filter(|v| v.is_finite())
}

/// A mesh's box: its plan points and its bottom and top.
fn mesh_points(e: &Entity) -> Option<(Vec<Vec2>, f64, f64)> {
    let EntityKind::Mesh(m) = &e.kind else { return None };
    let mut pts = Vec::new();
    let (mut z0, mut z1) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in m.vertices.iter().take(100_000) {
        if v.x.is_finite() && v.y.is_finite() && v.z.is_finite() {
            pts.push(Vec2::new(v.x, v.y));
            z0 = z0.min(v.z);
            z1 = z1.max(v.z);
        }
    }
    (!pts.is_empty() && z0.is_finite()).then_some((pts, z0, z1))
}

fn centre(pts: &[Vec2]) -> Vec2 {
    let (mut lo, mut hi) = (Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY));
    for p in pts {
        lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    (lo + hi) * 0.5
}

/// The extent of `pts` along `dir` (min, max).
fn span(pts: &[Vec2], origin: Vec2, dir: Vec2) -> (f64, f64) {
    pts.iter().map(|p| dot(*p - origin, dir)).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), t| (a.min(t), b.max(t)))
}

/// Upper-case words split on anything but letters, digits and `/`.
fn tokens(t: &str) -> Vec<String> {
    t.to_ascii_uppercase().split(|c: char| !(c.is_ascii_alphanumeric() || c == '/')).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

fn says_any(words: &[String], list: &Value) -> bool {
    list.as_array().into_iter().flatten().filter_map(Value::as_str).any(|w| {
        let w = w.to_ascii_uppercase();
        if w.contains(' ') { words.join(" ").contains(&w) } else { words.iter().any(|x| *x == w || (w.len() >= 4 && x.starts_with(&w))) }
    })
}

/// The rooms as the faces see them.
fn room_refs(s: &mut Session) -> Vec<RoomRef> {
    super::rooms3d::rooms(s)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.outline.len() >= 3)
        .map(|r| {
            let label = match r.number.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                Some(n) => n.to_string(),
                None => r.name.trim().to_string(),
            };
            RoomRef { label, level: r.level.clone(), outline: r.outline }
        })
        .collect()
}

/// What the faces are computed from.
struct Ctx {
    sp: BuildingSpec,
    upf: f64,
    north: f64,
    faces: Vec<Face>,
    rooms: Vec<RoomRef>,
}

impl Ctx {
    fn inch(&self) -> f64 {
        self.upf / 12.0
    }
    /// The traced room holding `p` on `level`.
    fn room_at(&self, level: Option<&str>, p: Vec2) -> Option<&RoomRef> {
        self.rooms.iter().find(|r| same_level(r.level.as_deref(), level) && cadaddy_geom::point_in_polygon(&r.outline, p))
    }
    /// Why a wall device has no named face: what is on each side of its box's face.
    fn no_face_why(&self, dv: &Dev) -> String {
        let inch = self.inch();
        let c = dv.c();
        let Some(n) = dv.normal() else { return "no wall face within 6 in, and no side stored for it".into() };
        let front = self.room_at(dv.level.as_deref(), c + n * (FACE_FRONT_IN * inch)).map(|r| r.label.clone());
        let back = self.room_at(dv.level.as_deref(), c - n * (FACE_FRONT_IN * inch + 12.0 * inch)).map(|r| r.label.clone());
        match (front, back) {
            (Some(r), _) => format!("room {r} is on the side its box faces, but no face of {r} runs along its wall"),
            (None, Some(r)) => format!("the side its box faces is in no traced room; room {r} is behind its wall"),
            (None, None) => "no traced room on either side of its wall".into(),
        }
    }
    fn face(&self, level: Option<&str>, name: &str) -> Option<&Face> {
        self.faces.iter().find(|f| f.name.eq_ignore_ascii_case(name) && same_level(f.level.as_deref(), level))
    }
}

fn same_level(a: Option<&str>, b: Option<&str>) -> bool {
    a.zip(b).is_none_or(|(a, b)| a.eq_ignore_ascii_case(b))
}

fn north_param(cmd: &str, p: &Value) -> Result<f64> {
    match p.get("north") {
        None | Some(Value::Null) => Ok(0.0),
        Some(v) => v.as_f64().filter(|n| n.is_finite() && n.abs() <= 360.0).ok_or_else(|| bad(cmd, "`north` must be degrees from -360 to 360")),
    }
}

fn context(s: &mut Session, cmd: &str, north: f64) -> Result<Ctx> {
    let (sp, _) = super::bspec::current(s, cmd)?;
    let upf = upf_of(&sp);
    let rooms = room_refs(s);
    let pieces = faces::pieces(s.doc()?, upf);
    let faces = faces::faces(&rooms, &pieces, upf, north);
    Ok(Ctx { sp, upf, north, faces, rooms })
}

fn face_json(f: &Face, inch: f64) -> Value {
    json!({
        "name": f.name, "room": f.room, "side": f.side, "level": f.level,
        "wall": f.wall.hex(), "walls": f.walls.iter().map(|h| h.hex()).collect::<Vec<_>>(), "wall_type": f.wall_type,
        "length_in": r2(f.len / inch), "height_in": r2((f.top - f.floor) / inch),
        "from_ft": [r2(f.a.x / inch / 12.0), r2(f.a.y / inch / 12.0)], "to_ft": [r2(f.b.x / inch / 12.0), r2(f.b.y / inch / 12.0)],
        "inward": [r2(f.inward.x), r2(f.inward.y)],
        "openings": f.openings.iter().map(|o| json!({"kind": o.kind, "from_in": r2(o.from / inch), "to_in": r2(o.to / inch), "bottom_in": r2((o.bottom - f.floor) / inch), "top_in": r2((o.top - f.floor) / inch)})).collect::<Vec<_>>(),
        "prev": f.prev, "next": f.next, "skew_deg": r2(f.skew_deg), "order": f.order,
    })
}

// ─── bim.faces ──────────────────────────────────────────────────────────────

fn run_faces(s: &mut Session, p: &Value) -> Result<Value> {
    let north = north_param(FACES, p)?;
    let cx = context(s, FACES, north)?;
    let inch = cx.inch();
    let room = str_param(p, "room").map(str::trim).filter(|r| !r.is_empty());
    let level = str_param(p, "level").map(str::trim).filter(|r| !r.is_empty());
    let wall = str_param(p, "wall").and_then(|h| Handle::parse_hex(h.trim()));
    let limit = p.get("limit").and_then(Value::as_u64).map_or(200, |n| n.clamp(1, 5_000) as usize);
    let mut by_level: BTreeMap<String, usize> = BTreeMap::new();
    for f in &cx.faces {
        *by_level.entry(f.level.clone().unwrap_or_default()).or_default() += 1;
    }
    let pick: Vec<&Face> = cx
        .faces
        .iter()
        .filter(|f| room.is_none_or(|r| f.room.eq_ignore_ascii_case(r)))
        .filter(|f| level.is_none_or(|l| f.level.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(l))))
        .filter(|f| wall.is_none_or(|w| f.walls.contains(&w)))
        .collect();
    Ok(json!({
        "count": pick.len(),
        "total": cx.faces.len(),
        "by_level": by_level,
        "plan_north": format!("{north} degrees counter-clockwise from +Y"),
        "faces": pick.iter().take(limit).map(|f| face_json(f, inch)).collect::<Vec<_>>(),
        "more": pick.len().saturating_sub(limit),
    }))
}

// ─── Devices and objects ────────────────────────────────────────────────────

/// A built wall device, as placement sees it.
struct Dev {
    h: Handle,
    level: Option<String>,
    pts: Vec<Vec2>,
    z0: f64,
    z1: f64,
    store: Props,
    words: Vec<String>,
}

impl Dev {
    fn c(&self) -> Vec2 {
        centre(&self.pts)
    }
    fn text(&self, key: &str) -> Option<String> {
        self.store.get(key).filter(|p| p.has_value()).map(Prop::text)
    }
    /// Its outward face direction (toward its room) as the build stored it.
    fn normal(&self) -> Option<Vec2> {
        self.store.get("face_normal").and_then(|p| parse_vec(&p.text())).or_else(|| self.store.get("host_face").and_then(|p| parse_vec(&p.text())))
    }
    fn host(&self) -> Option<Handle> {
        self.text("host_wall").and_then(|h| Handle::parse_hex(h.trim()))
    }
    fn wall_mounted(&self) -> bool {
        match self.text("mounting").as_deref() {
            Some("wall") => true,
            Some(_) => false,
            None => self.host().is_some(),
        }
    }
}

fn devices(d: &Drawing) -> Vec<Dev> {
    d.model
        .iter()
        .filter(|e| super::wallgen::made_as(e).as_deref() == Some("device"))
        .take(MAX_ELEMENTS)
        .filter_map(|e| {
            let (pts, z0, z1) = mesh_points(e)?;
            let store = props::read(&e.common.xdata);
            let words = ["kind", "type", "type_mark", "submittal", "catalog_number", "description"]
                .iter()
                .filter_map(|k| store.get(k).map(Prop::text))
                .collect::<Vec<_>>()
                .join(" ");
            Some(Dev { h: e.handle, level: super::bspec::made_level(e), pts, z0, z1, words: tokens(&words), store })
        })
        .collect()
}

/// An object placements can be tied to: casework, an appliance, a fixture.
#[derive(Clone, Debug)]
struct Obj {
    h: Handle,
    name: String,
    kind: String,
    words: Vec<String>,
    level: Option<String>,
    pts: Vec<Vec2>,
    z0: f64,
    z1: f64,
    source: String,
}

fn objects(d: &Drawing, sp: &BuildingSpec, upf: f64, rules: &Value) -> Vec<Obj> {
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_ELEMENTS) {
        let x = &e.common.xdata;
        let Some(i) = x.iter().position(|t| t.code == 1001 && t.value == CASEWORK_APP) else { continue };
        let field = |k: usize| x.get(i + k).filter(|t| t.code == 1000).map(|t| t.value.clone()).unwrap_or_default();
        let (id, tag, kind, status, sheet) = (field(1), field(2), field(3), field(4), field(5));
        let store = props::read(x);
        let level = store.get("level").map(Prop::text).filter(|l| !l.trim().is_empty());
        let floor = level_floor(sp, level.as_deref(), upf);
        let (pts, z0, z1) = match mesh_points(e) {
            Some(m) => m,
            None => {
                let b = cadaddy_doc::entity_bounds(d, e, 0);
                if b.is_empty() {
                    continue;
                }
                let hz = &rules["default_heights_in"][kind.as_str()];
                let (lo, hi) = (hz[0].as_f64().unwrap_or(0.0), hz[1].as_f64().unwrap_or(36.0));
                (vec![b.min, b.max, Vec2::new(b.min.x, b.max.y), Vec2::new(b.max.x, b.min.y)], floor + lo / 12.0 * upf, floor + hi / 12.0 * upf)
            }
        };
        let name = if tag.trim().is_empty() {
            format!("{} {}", kind.to_ascii_uppercase().replace('_', " "), e.handle.hex())
        } else {
            tag.trim().to_string()
        };
        let words = tokens(&format!("{kind} {tag} {id} {}", store.get("description").map(Prop::text).unwrap_or_default()).replace('_', " "));
        let source = [status.as_str(), sheet.as_str()].iter().filter(|t| !t.trim().is_empty()).copied().collect::<Vec<_>>().join(", ");
        out.push(Obj { h: e.handle, name, kind, words, level, pts, z0, z1, source });
    }
    out
}

/// A face's line found from its wall alone (after the wall moved: the room outline may not have
/// followed): the piece of `wall` most parallel to `u` nearest `near`, its face toward `inward`.
fn wall_line(pieces: &[Piece], wall: Handle, inward: Vec2, near: Vec2, z: Option<f64>) -> Option<(Vec2, Vec2, f64)> {
    let mut best: Option<(f64, (Vec2, Vec2, f64))> = None;
    for p in pieces.iter().filter(|p| p.wall == wall) {
        let l = p.a.dist(p.b);
        if !(l.is_finite() && l > 0.0) {
            continue;
        }
        let u = (p.b - p.a) * (1.0 / l);
        let n = Vec2::new(-u.y, u.x);
        let along = (dot(n, inward)).abs();
        if along < 0.9 {
            continue;
        }
        let n_in = if dot(n, inward) >= 0.0 { n } else { n * -1.0 };
        let t = dot(near - p.a, u).clamp(0.0, l);
        let dist = (p.a + u * t).dist(near);
        // A piece at the device's height first (a header over a door is no host below it).
        let at_height = z.is_none_or(|z| z >= p.z0 - 1e-6 && z <= p.z1 + 1e-6);
        let key = dist + if at_height { 0.0 } else { 1e9 };
        if best.as_ref().is_none_or(|b| key < b.0) {
            let face_pt = p.a + n_in * p.half;
            // The face's direction keeps the stored corner order: u with the room on its left.
            let u_face = Vec2::new(n_in.y, -n_in.x);
            best = Some((key, (face_pt, u_face, p.half)));
        }
    }
    best.map(|b| b.1)
}

// ─── The relative placement of one device ───────────────────────────────────

/// A reference an offset is measured from.
#[derive(Clone, Debug)]
enum Ref {
    /// A face: its name, wall and inward direction.
    Face { name: String, wall: Handle, inward: Vec2 },
    /// The end of the host face where no wall meets it: the point and which way the device is.
    End { name: String, point: Vec2, sign: f64 },
}

impl Ref {
    fn name(&self) -> &str {
        match self {
            Ref::Face { name, .. } | Ref::End { name, .. } => name,
        }
    }
    fn to_json(&self) -> Value {
        match self {
            Ref::Face { name, wall, inward } => json!({"kind": "face", "name": name, "wall": wall.hex(), "inward": vec_text(*inward)}),
            Ref::End { name, point, sign } => json!({"kind": "end", "name": name, "point": [point.x, point.y], "sign": sign}),
        }
    }
    fn from_json(v: &Value) -> Option<Ref> {
        let name = v["name"].as_str()?.to_string();
        match v["kind"].as_str()? {
            "face" => Some(Ref::Face { name, wall: Handle::parse_hex(v["wall"].as_str()?)?, inward: parse_vec(v["inward"].as_str()?)? }),
            "end" => {
                let p = Vec2::new(v["point"][0].as_f64()?, v["point"][1].as_f64()?);
                Some(Ref::End { name, point: p, sign: v["sign"].as_f64().filter(|s| s.is_finite())?.signum() })
            }
            _ => None,
        }
    }
    /// How far `p` is from it, toward the device's side (drawing units), given the host face's
    /// line (`a`, `u`) and the walls now.
    fn offset(&self, pieces: &[Piece], a: Vec2, u: Vec2, p: Vec2) -> Option<f64> {
        match self {
            Ref::Face { wall, inward, .. } => {
                let (q, _, _) = wall_line(pieces, *wall, *inward, p, None)?;
                Some(dot(p - q, *inward))
            }
            Ref::End { point, sign, .. } => Some(dot(p - a, u) * sign - dot(*point - a, u) * sign),
        }
    }
    /// The along position on the host face (from `a`) that is `off` from it.
    fn along_for(&self, pieces: &[Piece], a: Vec2, u: Vec2, off: f64, near: Vec2) -> Option<f64> {
        match self {
            Ref::Face { wall, inward, .. } => {
                let (q, _, _) = wall_line(pieces, *wall, *inward, near, None)?;
                let k = dot(u, *inward);
                (k.abs() > 0.2).then(|| (off - dot(a - q, *inward)) / k)
            }
            Ref::End { point, sign, .. } => Some(dot(*point - a, u) + off * sign),
        }
    }
}

/// A constraint tying a device to an object.
#[derive(Clone, Debug, PartialEq)]
struct Tie {
    reference: String,
    ref_handle: Handle,
    ref_point: String,
    axis: String,
    target_in: f64,
    plus_minus_in: f64,
    inside: bool,
    zone: String,
    follow: bool,
    /// The device's centre along the face from the object's centre when it was tied (inches).
    rel_in: f64,
    source: String,
    derived: bool,
    why: String,
    /// The kind of object the rules data ties the device to when no such object is in the model
    /// (`refrigerator`, `microwave`…): checked once the object exists, never passed or failed before.
    pending: String,
    /// The kind of the object it is tied to (`upper_cabinet`, `refrigerator`…).
    ref_kind: String,
}

impl Tie {
    fn to_json(&self) -> Value {
        json!({
            "ref": self.reference, "ref_handle": self.ref_handle.hex(), "ref_point": self.ref_point, "axis": self.axis,
            "target_in": r2(self.target_in), "plus_minus_in": r2(self.plus_minus_in), "inside": self.inside, "zone": self.zone,
            "follow": self.follow, "rel_in": r2(self.rel_in), "source": self.source, "derived": self.derived, "why": self.why,
            "pending_object": self.pending, "ref_kind": self.ref_kind,
        })
    }
    fn from_json(v: &Value) -> Option<Tie> {
        Some(Tie {
            reference: v["ref"].as_str()?.to_string(),
            ref_handle: Handle::parse_hex(v["ref_handle"].as_str()?)?,
            ref_point: v["ref_point"].as_str().unwrap_or("center").to_string(),
            axis: v["axis"].as_str().unwrap_or("along").to_string(),
            target_in: v["target_in"].as_f64().filter(|x| x.is_finite()).unwrap_or(0.0),
            plus_minus_in: v["plus_minus_in"].as_f64().filter(|x| x.is_finite() && *x >= 0.0).unwrap_or(0.0),
            inside: v["inside"].as_bool().unwrap_or(false),
            zone: v["zone"].as_str().unwrap_or("").to_string(),
            follow: v["follow"].as_bool().unwrap_or(true),
            rel_in: v["rel_in"].as_f64().filter(|x| x.is_finite()).unwrap_or(0.0),
            source: v["source"].as_str().unwrap_or("").to_string(),
            derived: v["derived"].as_bool().unwrap_or(false),
            why: v["why"].as_str().unwrap_or("").to_string(),
            pending: v["pending_object"].as_str().unwrap_or("").to_string(),
            ref_kind: v["ref_kind"].as_str().unwrap_or("").to_string(),
        })
    }
    /// What a tie whose object isn't in the model reports: `pending object (<kind>)`.
    fn pending_text(&self) -> String {
        format!("pending object ({})", if self.pending.is_empty() { "object" } else { self.pending.as_str() })
    }
}

/// The kind of a tie's missing object: the rules data's concealed row its words or reason name,
/// else `object`.
fn pending_kind(rules: &Value, t: &Tie) -> String {
    if !t.ref_kind.trim().is_empty() {
        return t.ref_kind.clone();
    }
    let words = tokens(&t.reference);
    rules["concealed"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["why"].as_str() == Some(t.why.as_str()) || says_any(&words, &row["words"]))
        .and_then(|row| row["id"].as_str())
        .unwrap_or("object")
        .to_string()
}

fn ties_of(store: &Props) -> Vec<Tie> {
    (1..=MAX_CONSTRAINTS)
        .filter_map(|k| store.get(&format!("constraint_{k}")))
        .filter_map(|p| serde_json::from_str::<Value>(&p.text()).ok())
        .filter_map(|v| Tie::from_json(&v))
        .collect()
}

fn set_ties(store: &mut Props, ties: &[Tie]) {
    for k in 1..=MAX_CONSTRAINTS {
        store.remove(&format!("constraint_{k}"));
    }
    for (k, t) in ties.iter().take(MAX_CONSTRAINTS).enumerate() {
        let src = if t.derived {
            Source::from_doc(SourceKind::Rule, &format!("rules data: {}", t.why))
        } else if t.source.trim().is_empty() {
            Source::user()
        } else {
            Source::from_doc(SourceKind::Note, &t.source)
        };
        let mut p = Prop::new(t.to_json().to_string(), src);
        if t.derived {
            p.note = Some("derived (rules data), not from a document".into());
        }
        store.set(&format!("constraint_{}", k + 1), p);
    }
}

/// An object's extent on a face: along (from the face's start) and above the floor, inches; and
/// how far in front of the face its back and front are.
fn on_face(o: &Obj, a: Vec2, u: Vec2, inward: Vec2, floor: f64, inch: f64) -> (Rect, f64, f64) {
    let (t0, t1) = span(&o.pts, a, u);
    let (d0, d1) = span(&o.pts, a, inward);
    (Rect { a0: t0 / inch, a1: t1 / inch, z0: (o.z0 - floor) / inch, z1: (o.z1 - floor) / inch }, d0 / inch, d1 / inch)
}

/// The kind of a device for its band (`receptacle`, `switch`…; `default`).
fn band_kind(words: &[String], rules: &Value) -> String {
    let kinds = rules["along_band_kinds"].as_object().cloned().unwrap_or_default();
    // A switch's words first (a "switched receptacle" is a receptacle).
    for k in ["occupancy_sensor", "thermostat", "fire_alarm_device", "receptacle", "switch", "data_outlet"] {
        if kinds.get(k).is_some_and(|l| says_any(words, l)) {
            return k.to_string();
        }
    }
    "default".into()
}

/// Whether a device takes a faceplate (its words name a kind the rules data lists).
fn has_plate(words: &[String], rules: &Value) -> bool {
    says_any(words, &rules["plate_kinds"])
}

fn abbrev(words: &[String], mark: Option<&str>, rules: &Value) -> String {
    let said = rules["abbrev"].as_array().into_iter().flatten().find(|r| says_any(words, &r["words"])).and_then(|r| r["say"].as_str());
    match (said, mark.map(str::trim).filter(|m| !m.is_empty())) {
        (Some(s), Some(m)) if !s.eq_ignore_ascii_case(m) => format!("{s} ({m})"),
        (Some(s), _) => s.to_string(),
        (None, Some(m)) => m.to_string(),
        (None, None) => "DEVICE".into(),
    }
}

/// The documents' linear dimensions in model space: their two measured points and value.
fn dimensions(d: &Drawing) -> Vec<(Vec2, Vec2, f64)> {
    d.model
        .iter()
        .take(MAX_ELEMENTS * 4)
        .filter_map(|e| match &e.kind {
            EntityKind::Dimension(x) if matches!(x.kind, DimKind::Linear { .. } | DimKind::Aligned) => {
                let (a, b) = (Vec2::new(x.p13.x, x.p13.y), Vec2::new(x.p14.x, x.p14.y));
                let said = len_in(&x.text);
                let v = said.or_else(|| (x.measurement.is_finite() && x.measurement > 0.0).then_some(x.measurement)).unwrap_or_else(|| a.dist(b));
                (a.is_finite() && b.is_finite()).then_some((a, b, v))
            }
            _ => None,
        })
        .collect()
}

/// What rehosting found for one device.
struct Placed {
    face: usize,
    t: f64,
    zc: f64,
    across: f64,
    along_len: f64,
    reference: Ref,
    offset: f64,
    documented: Option<String>,
    depth_front: f64,
    /// A drawn dimension's offset: the model's own offset from the same reference (inches) and
    /// the dimension's source; or why the dimension that measures it gives no along offset.
    drawn: Option<(f64, Source)>,
    drawn_why: Option<&'static str>,
}

/// The face a device is on: its stored host wall's face toward its room first (the side its
/// box faces), else the nearest face within [`FACE_NEAR_IN`].
fn face_for(cx: &Ctx, by_wall: &HashMap<Handle, Vec<usize>>, by_level: &HashMap<String, Vec<usize>>, dv: &Dev) -> Option<usize> {
    let inch = cx.inch();
    let c = dv.c();
    let normal = dv.normal();
    let score = |k: usize, back: f64| -> Option<f64> {
        let f = cx.faces.get(k)?;
        if !same_level(f.level.as_deref(), dv.level.as_deref()) {
            return None;
        }
        // Its box overlaps the face along it (a box at a corner may reach past the face's end).
        let (t0, t1) = span(&dv.pts, f.a, f.u);
        if t1 < -2.0 * inch || t0 > f.len + 2.0 * inch {
            return None;
        }
        let t = f.along(c);
        let past_end = (-t).max(t - f.len).max(0.0);
        // Its back (the point nearest the wall): a fixture's box stands well in front of it.
        let dep = span(&dv.pts, f.a, f.inward).0;
        if dep > FACE_FRONT_IN * inch || dep < -(2.0 * f.half + back) {
            return None;
        }
        if normal.is_some_and(|n| dot(n, f.inward) < 0.5) {
            return None;
        }
        Some(dep.abs() + past_end)
    };
    let best = |ks: &mut dyn Iterator<Item = usize>, back: f64| {
        ks.filter_map(|k| score(k, back).map(|s| (k, s))).min_by(|a, b| a.1.total_cmp(&b.1)).map(|x| x.0)
    };
    let empty = Vec::new();
    if let Some(h) = dv.host()
        && let Some(k) = best(&mut by_wall.get(&h).unwrap_or(&empty).iter().copied(), FACE_BACK_IN * inch)
    {
        return Some(k);
    }
    let lv = dv.level.clone().unwrap_or_default().to_ascii_lowercase();
    best(
        &mut by_level
            .get(&lv)
            .unwrap_or(&empty)
            .iter()
            .copied()
            .filter(|&k| cx.faces.get(k).is_some_and(|f| span(&dv.pts, f.a, f.inward).0.abs() <= FACE_NEAR_IN * inch + f.half * 2.0)),
        FACE_BACK_IN * inch,
    )
}

fn place(cx: &Ctx, face_k: usize, dv: &Dev, dims: &[(Vec2, Vec2, f64)], doc: Option<&drawn::DocOffset>) -> Option<Placed> {
    let inch = cx.inch();
    let f = cx.faces.get(face_k)?;
    let c = dv.c();
    let t = f.along(c);
    let (a0, a1) = span(&dv.pts, f.a, f.u);
    let (d0, d1) = span(&dv.pts, f.a, f.inward);
    let (along_len, across) = ((a1 - a0).max(inch * 0.5), (d1 - d0).max(inch * 0.5));
    let depth_front = d1;
    let zc = (dv.z0 + dv.z1) / 2.0;
    // The reference: the face at the nearer end, else that end itself.
    // The reference: the face across the nearer corner (square to this one, the device in front
    // of it), else the one across the other corner, else the nearer end of the face itself.
    let start = t <= f.len - t;
    let near_end = |at_start: bool| if at_start { (f.prev.as_deref(), f.a, 1.0, "start") } else { (f.next.as_deref(), f.b, -1.0, "end") };
    let picked = [near_end(start), near_end(!start)].into_iter().find_map(|(nb, _, _, _)| {
        let r = nb.and_then(|n| cx.face(f.level.as_deref(), n))?;
        let off = r.depth(c);
        (dot(f.u, r.inward).abs() > 0.5 && off >= -inch).then(|| (Ref::Face { name: r.name.clone(), wall: r.wall, inward: r.inward }, off))
    });
    let (reference, offset) = picked.unwrap_or_else(|| {
        let (_, end_pt, sign, end_word) = near_end(start);
        (Ref::End { name: format!("the {end_word} of {}", f.name), point: end_pt, sign }, (t - dot(end_pt - f.a, f.u)) * sign)
    });
    // A dimension from the device to a face or an opening edge on its face: the document's offset.
    let mut documented = None;
    let mut offset = offset;
    let mut reference = reference;
    for (p, q, v) in dims {
        for (at_dev, other) in [(*p, *q), (*q, *p)] {
            if at_dev.dist(c) > DIM_AT_IN * inch {
                continue;
            }
            // The other end on a neighbouring face of the room, or on an opening's edge.
            let on_face = cx
                .faces
                .iter()
                .filter(|g| g.room == f.room && same_level(g.level.as_deref(), f.level.as_deref()) && g.name != f.name)
                .find(|g| g.depth(other).abs() <= DIM_ON_IN * inch);
            if let Some(g) = on_face {
                reference = Ref::Face { name: g.name.clone(), wall: g.wall, inward: g.inward };
                offset = *v / 12.0 * cx.upf;
                documented = Some(format!("a dimension from {} to the device", g.name));
            } else if let Some(o) =
                f.openings.iter().find(|o| (f.along(other) - o.from).abs() <= DIM_ON_IN * inch || (f.along(other) - o.to).abs() <= DIM_ON_IN * inch)
            {
                let edge = if (f.along(other) - o.from).abs() <= (f.along(other) - o.to).abs() { o.from } else { o.to };
                let sign = if t >= edge { 1.0 } else { -1.0 };
                reference = Ref::End { name: format!("the {} edge at {}", o.kind, ft_in(edge / inch)), point: f.at(edge), sign };
                offset = *v / 12.0 * cx.upf;
                documented = Some(format!("a dimension from the {} edge to the device", o.kind));
            }
        }
        if documented.is_some() {
            break;
        }
    }
    // A dimension the sheets draw as line work (a chain to a face, an opening or casework edge):
    // the document's offset when it runs along the device's face.
    let mut drawn = None;
    let mut drawn_why = None;
    if let Some(doc) = doc {
        match drawn_reference(cx, f, c, doc) {
            Ok((r, measured, said)) => {
                reference = r;
                offset = said;
                documented = Some(doc.why.clone());
                drawn = Some((measured / inch, doc.source.clone()));
            }
            Err(why) => drawn_why = Some(why),
        }
    }
    Some(Placed { face: face_k, t, zc, across, along_len, reference, offset, documented, depth_front, drawn, drawn_why })
}

/// How far a drawn dimension's reference may be off the device's face's ends (inches), and off
/// its wall's line for an opening's edge or a casework edge.
const DRAWN_REF_REACH_IN: f64 = 24.0;
const DRAWN_EDGE_OFF_IN: f64 = 12.0;
const DRAWN_CASEWORK_OFF_IN: f64 = 48.0;

/// A drawn dimension's reference as an along-face offset of the device on face `f`: the
/// reference (the face of the wall that faces the device: a demising wall's other room's face
/// gives way to its twin, the drawn value less the wall between them), the model's own offset
/// from it and the drawn one (drawing units); or why it isn't one.
fn drawn_reference(cx: &Ctx, f: &Face, c: Vec2, doc: &drawn::DocOffset) -> std::result::Result<(Ref, f64, f64), &'static str> {
    let inch = cx.inch();
    if dot(doc.dir, f.u).abs() < 0.9 {
        return Err("dimensioned across its wall, not along it");
    }
    let reach = DRAWN_REF_REACH_IN * inch;
    let on_span = |t: f64| t >= -reach && t <= f.len + reach;
    match &doc.reference {
        Ref::Face { name, .. } => {
            let g = cx.face(f.level.as_deref(), name).ok_or("its reference face is gone")?;
            let said = doc.value_in * inch;
            let (g, said) = if g.depth(c) >= -inch {
                (g, said)
            } else {
                let twin = cx
                    .faces
                    .iter()
                    .filter(|h| {
                        same_level(h.level.as_deref(), f.level.as_deref()) && dot(h.u, g.u) < -0.95 && h.walls.iter().any(|w| g.walls.contains(w))
                    })
                    .filter(|h| h.depth(c) >= -inch)
                    .min_by(|a, b| a.depth(c).total_cmp(&b.depth(c)))
                    .ok_or("its reference face faces away from it")?;
                (twin, said - dot(twin.a - g.a, g.inward).abs())
            };
            if !on_span(f.along(g.a)) {
                return Err("its reference is not at its wall");
            }
            Ok((Ref::Face { name: g.name.clone(), wall: g.wall, inward: g.inward }, g.depth(c), said))
        }
        Ref::End { name, point, .. } => {
            let off = if doc.casework { DRAWN_CASEWORK_OFF_IN } else { DRAWN_EDGE_OFF_IN } * inch;
            if f.depth(*point).abs() > off || !on_span(f.along(*point)) {
                return Err("its reference is not at its wall");
            }
            let sign = if f.along(c) >= f.along(*point) { 1.0 } else { -1.0 };
            Ok((Ref::End { name: name.clone(), point: *point, sign }, (f.along(c) - f.along(*point)) * sign, doc.value_in * inch))
        }
    }
}

/// The ties the rules data gives a device from the objects against its face.
fn derive_ties(rules: &Value, f: &Face, objs: &[&Obj], plate_rect: &Rect, inch: f64, words: &[String]) -> Vec<Tie> {
    let mut out = Vec::new();
    // Only outlets are concealed behind appliances (not a switch, a thermostat, an alarm).
    let kind = band_kind(words, rules);
    if !matches!(kind.as_str(), "receptacle" | "data_outlet" | "default") {
        return out;
    }
    let rows: Vec<Value> = rules["concealed"].as_array().cloned().unwrap_or_default();
    let against: Vec<(&Obj, Rect)> = objs
        .iter()
        .filter_map(|o| {
            let (r, b0, _) = on_face(o, f.a, f.u, f.inward, f.floor, inch);
            (b0 <= OBJ_BACK_IN && b0 >= -2.0 * f.half / inch - 1.0).then_some((*o, r))
        })
        .collect();
    let near = |r: &Rect| plate_rect.a1 > r.a0 - OBJ_NEAR_IN && plate_rect.a0 < r.a1 + OBJ_NEAR_IN;
    for (o, r) in &against {
        let Some(row) = rows.iter().find(|row| says_any(&o.words, &row["words"])) else { continue };
        let band = row["band_in"].as_f64().unwrap_or(6.0);
        let zone = row["zone"].as_str().unwrap_or("behind");
        let why = row["why"].as_str().unwrap_or("").to_string();
        let mid = (r.a0 + r.a1) / 2.0;
        let pc = (plate_rect.a0 + plate_rect.a1) / 2.0;
        match zone {
            "adjacent" => {
                // Its outlet belongs in the cabinet beside it (the sink base): an outlet behind it
                // or in that cabinet is tied to the cabinet.
                let host = against
                    .iter()
                    .filter(|(x, hr)| {
                        x.h != o.h && says_any(&x.words, &row["host_words"]) && hr.a0 <= r.a1 + OBJ_NEAR_IN && hr.a1 >= r.a0 - OBJ_NEAR_IN
                    })
                    .min_by(|x, y| ((x.1.a0 + x.1.a1) / 2.0 - mid).abs().total_cmp(&(((y.1.a0 + y.1.a1) / 2.0) - mid).abs()));
                if let Some((hx, hr)) = host {
                    let in_host = plate_rect.a1 > hr.a0 && plate_rect.a0 < hr.a1;
                    let behind = plate_rect.a1 > r.a0 && plate_rect.a0 < r.a1;
                    if !(in_host || behind) || plate_rect.z0 > hr.z1.max(r.z1) + OBJ_NEAR_IN {
                        continue;
                    }
                    let hm = (hr.a0 + hr.a1) / 2.0;
                    out.push(Tie {
                        reference: hx.name.clone(),
                        ref_handle: hx.h,
                        ref_point: "center".into(),
                        axis: "along".into(),
                        target_in: 0.0,
                        plus_minus_in: band.max((hr.a1 - hr.a0) / 2.0),
                        inside: true,
                        zone: format!("in the {} beside {}", hx.name, o.name),
                        follow: in_host,
                        rel_in: pc - hm,
                        source: String::new(),
                        pending: String::new(),
                        ref_kind: hx.kind.clone(),
                        derived: true,
                        why: why.clone(),
                    });
                }
            }
            "inside" => {
                if !(plate_rect.a1 > r.a0 && plate_rect.a0 < r.a1 && plate_rect.z1 > r.z0 && plate_rect.z0 < r.z1) {
                    continue;
                }
                out.push(Tie {
                    reference: o.name.clone(),
                    ref_handle: o.h,
                    ref_point: "center".into(),
                    axis: "along".into(),
                    target_in: 0.0,
                    plus_minus_in: band,
                    inside: true,
                    zone: format!("inside {}", o.name),
                    follow: true,
                    rel_in: pc - mid,
                    source: String::new(),
                    pending: String::new(),
                    ref_kind: o.kind.clone(),
                    derived: true,
                    why: why.clone(),
                });
            }
            _ => {
                // Behind: a receptacle low on the wall within reach of the appliance.
                if !near(r) || plate_rect.z0 > r.z1 + OBJ_NEAR_IN {
                    continue;
                }
                out.push(Tie {
                    reference: o.name.clone(),
                    ref_handle: o.h,
                    ref_point: "center".into(),
                    axis: "along".into(),
                    target_in: 0.0,
                    plus_minus_in: band,
                    inside: true,
                    zone: format!("concealed behind {}{}", o.name, if o.source.is_empty() { String::new() } else { format!(" ({})", o.source) }),
                    follow: true,
                    rel_in: pc - mid,
                    source: String::new(),
                    pending: String::new(),
                    ref_kind: o.kind.clone(),
                    derived: true,
                    why: why.clone(),
                });
            }
        }
    }
    // The device's own words name an appliance it serves (a refrigerator receptacle, a
    // microwave outlet…) but no such object is against its face: the tie waits for the object.
    for row in &rows {
        let id = row["id"].as_str().unwrap_or("object");
        let why = row["why"].as_str().unwrap_or("");
        if !says_any(words, &row["words"]) || out.iter().any(|t| t.why == why) {
            continue;
        }
        out.push(Tie {
            reference: format!("{id} (not in the model)"),
            ref_handle: Handle(0),
            ref_point: "center".into(),
            axis: "along".into(),
            target_in: 0.0,
            plus_minus_in: row["band_in"].as_f64().unwrap_or(6.0),
            inside: true,
            zone: format!("pending object ({id})"),
            follow: false,
            rel_in: 0.0,
            source: String::new(),
            pending: id.to_string(),
            ref_kind: String::new(),
            derived: true,
            why: why.to_string(),
        });
    }
    // A plate inside a cabinet's box with no appliance rule: inside it, centred within the
    // rules data's band.
    if out.iter().all(|t| !t.pending.is_empty()) {
        let band = rules["inside_cabinet_band_in"].as_f64().unwrap_or(2.0);
        for (o, r) in &against {
            let cabinet = o.words.iter().any(|w| w == "CABINET" || w.starts_with("CAB")) || o.kind.contains("cabinet");
            if cabinet && plate_rect.a1 > r.a0 && plate_rect.a0 < r.a1 && plate_rect.z1 > r.z0 && plate_rect.z0 < r.z1 && r.z0 > 6.0 {
                let mid = (r.a0 + r.a1) / 2.0;
                out.push(Tie {
                    reference: o.name.clone(),
                    ref_handle: o.h,
                    ref_point: "center".into(),
                    axis: "along".into(),
                    target_in: 0.0,
                    plus_minus_in: band,
                    inside: true,
                    zone: format!("inside {}", o.name),
                    follow: true,
                    rel_in: (plate_rect.a0 + plate_rect.a1) / 2.0 - mid,
                    source: String::new(),
                    pending: String::new(),
                    ref_kind: o.kind.clone(),
                    derived: true,
                    why: "a box inside a cabinet is centred in it".into(),
                });
            }
        }
    }
    out
}

/// A framing object (a stud) or a pipe that may stand in a wall: its plan points, centre,
/// level and what it is (`None`: a stud).
struct InWall {
    pts: Vec<Vec2>,
    c: Vec2,
    level: Option<String>,
    pipe: Option<String>,
}

/// The studs (elements whose kind is `stud`) and the vertical pipes in the model, once.
fn in_wall_items(d: &Drawing) -> Vec<InWall> {
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_ELEMENTS * 4) {
        let part = super::wallgen::made_as(e);
        let maybe = matches!(part.as_deref(), Some("stud") | Some("mep")) || e.common.xdata.iter().any(|t| t.value.contains("stud"));
        if !maybe {
            continue;
        }
        let store = props::read(&e.common.xdata);
        let kind = store.get("kind").map(Prop::text).unwrap_or_default().to_ascii_lowercase();
        let is_stud = part.as_deref() == Some("stud") || kind == "stud";
        let is_pipe = part.as_deref() == Some("mep") && kind.contains("pipe");
        if !(is_stud || is_pipe) {
            continue;
        }
        let Some((pts, z0, z1)) = mesh_points(e) else { continue };
        let c = centre(&pts);
        let pipe = if is_stud {
            None
        } else {
            // A pipe standing in a wall is vertical: taller than it is long on the plan.
            let (x0, x1) = span(&pts, c, Vec2::new(1.0, 0.0));
            let (y0, y1) = span(&pts, c, Vec2::new(0.0, 1.0));
            if (x1 - x0).max(y1 - y0) >= (z1 - z0) {
                continue;
            }
            let size = store.get("size").map(Prop::text).unwrap_or_default();
            let system = super::bimlook::made_tag(e, "system").unwrap_or("").to_string();
            Some(format!("the {} {} pipe", size.trim(), system.trim()).replace("  ", " "))
        };
        out.push(InWall { pts, c, level: super::bspec::made_level(e), pipe });
    }
    out
}

/// Studs and the things that exclude a box from a bay, along a face (inches).
fn framing(items: &[InWall], f: &Face, inch: f64) -> (Vec<f64>, Vec<Obstacle>) {
    let mut studs = Vec::new();
    let mut obstacles = Vec::new();
    for it in items.iter().filter(|it| same_level(it.level.as_deref(), f.level.as_deref())) {
        let dep = f.depth(it.c);
        let t = f.along(it.c);
        if dep > 0.5 * inch || dep < -(2.0 * f.half + 0.5 * inch) || t < -inch || t > f.len + inch {
            continue;
        }
        let (t0, t1) = span(&it.pts, f.a, f.u);
        match &it.pipe {
            None => studs.push((t0 + t1) / 2.0 / inch),
            Some(what) => obstacles.push(Obstacle { a0: t0 / inch, a1: t1 / inch, what: what.clone() }),
        }
    }
    (studs, obstacles)
}

/// A length written in a dimension's text (`9'-2"`, `9' 2 1/2"`, `15"`, `9'`), inches.
fn len_in(t: &str) -> Option<f64> {
    let t = t.trim();
    if t.is_empty() {
        return None;
    }
    match t.split_once(['\'', '\u{2032}']) {
        Some((ft, rest)) => {
            let ft: f64 = ft.trim().trim_start_matches(|c: char| !c.is_ascii_digit()).parse().ok()?;
            let inches = super::walltypes::inches_in(rest).first().copied().unwrap_or(0.0);
            let v = ft * 12.0 + inches;
            v.is_finite().then_some(v)
        }
        None => super::walltypes::inches_in(t).first().copied().filter(|v| v.is_finite()),
    }
}

// ─── bim.layout.rehost ──────────────────────────────────────────────────────

fn run_rehost(s: &mut Session, p: &Value) -> Result<Value> {
    let north = north_param(REHOST, p)?;
    let style_param = str_param(p, "plate_style").map(|t| t.trim().to_ascii_lowercase());
    rehost(s, north, style_param.as_deref())
}

/// Place every wall device relative to its named face (see the module docs); the report.
pub(crate) fn rehost(s: &mut Session, north: f64, project_style: Option<&str>) -> Result<Value> {
    let cx = context(s, REHOST, north)?;
    let rules = solve::rules();
    let inch = cx.inch();
    let d = s.doc()?;
    let mut by_wall: HashMap<Handle, Vec<usize>> = HashMap::new();
    let mut by_level: HashMap<String, Vec<usize>> = HashMap::new();
    let mut faces_by_level: BTreeMap<String, usize> = BTreeMap::new();
    for (k, f) in cx.faces.iter().enumerate() {
        for w in &f.walls {
            by_wall.entry(*w).or_default().push(k);
        }
        by_level.entry(f.level.clone().unwrap_or_default().to_ascii_lowercase()).or_default().push(k);
        *faces_by_level.entry(f.level.clone().unwrap_or_default()).or_default() += 1;
    }
    let devs: Vec<Dev> = devices(d).into_iter().filter(Dev::wall_mounted).collect();
    let objs = objects(d, &cx.sp, cx.upf, &rules);
    let dims = dimensions(d);
    // The sheets' dimensions drawn as line work, placed on the model and attached.
    let all_devs = devices(d);
    let drawn_dims = super::dimreader::read_drawing(d, &cx.sp);
    let attached = drawn::attach(&cx, &all_devs, &objs, &drawn_dims);
    let tol_h = rules["height_tolerance_in"].as_f64().unwrap_or(0.25);
    // Each device on its face.
    let mut placed: Vec<(usize, Placed)> = Vec::new();
    let mut no_face: Vec<Value> = Vec::new();
    let mut no_face_n = 0usize;
    let mut unplaced: Vec<(Handle, Props)> = Vec::new();
    let mut no_face_by: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (i, dv) in devs.iter().enumerate() {
        match face_for(&cx, &by_wall, &by_level, dv).and_then(|k| place(&cx, k, dv, &dims, attached.docs.get(&dv.h))) {
            Some(pl) => placed.push((i, pl)),
            None => {
                no_face_n += 1;
                let c = dv.c();
                let why_text = cx.no_face_why(dv);
                let why = why_text.as_str();
                let generic = if why.starts_with("room ") {
                    "a room on its side, no face of it along its wall"
                } else if why.starts_with("the side its box faces") {
                    "its side in no traced room, a room behind its wall"
                } else {
                    "no traced room on either side"
                };
                *no_face_by.entry(generic).or_default() += 1;
                // The first few of each reason.
                if no_face_by.get(generic).copied().unwrap_or(0) <= MAX_LISTED / 3 {
                    no_face.push(json!({"handle": dv.h.hex(), "level": dv.level, "at_ft": [r2(c.x / cx.upf), r2(c.y / cx.upf)], "why": why}));
                }
                let mut store = dv.store.clone();
                if let Some(n) = dv.normal() {
                    store.set(
                        "face_normal",
                        Prop::new(vec_text(n), Source::computed()).with_note("the face's outward direction (plan): toward its room"),
                    );
                }
                store.set("host_face", Prop::new(NO_FACE, Source::computed()).with_note(why));
                store.remove("layout_ref");
                unplaced.push((dv.h, store));
            }
        }
    }
    // Plates: devices side by side at one height on a face share one.
    let gang = &rules["gang"];
    let mut on_face: HashMap<usize, Vec<usize>> = HashMap::new();
    for (j, (_, pl)) in placed.iter().enumerate() {
        on_face.entry(pl.face).or_default().push(j);
    }
    let mut plate_of: HashMap<usize, (Plate, String, usize)> = HashMap::new();
    let mut ganged = 0usize;
    let mut stacked = 0usize;
    let mut ganged_list: Vec<Value> = Vec::new();
    for (fk, js) in &on_face {
        // Only devices that take a faceplate (receptacles, switches, outlets…) share one.
        let js: Vec<usize> =
            js.iter().copied().filter(|j| placed.get(*j).and_then(|(i, _)| devs.get(*i)).is_some_and(|dv| has_plate(&dv.words, &rules))).collect();
        let js = &js;
        let pts: Vec<(f64, f64)> = js.iter().filter_map(|j| placed.get(*j)).map(|(_, pl)| (pl.t / inch, pl.zc / inch)).collect();
        // Plates can't overlap: devices at one height closer than a plate's width plus the rules
        // data's gap are one ganged box. The widest plate on the face sets the spacing.
        let spacing = js
            .iter()
            .filter_map(|j| placed.get(*j))
            .filter_map(|(i, _)| devs.get(*i))
            .map(|dv| solve::gang_spacing(&rules, solve::style_said(&dv.words.join(" ")).or(project_style)))
            .fold(solve::gang_spacing(&rules, project_style), f64::max);
        for (g, grp) in solve::gangs(&pts, spacing, gang["same_height_in"].as_f64().unwrap_or(1.0)).into_iter().enumerate() {
            let members: Vec<usize> = grp.iter().filter_map(|x| js.get(*x).copied()).collect();
            // The style a member's own words name, else the project's, else the rules data's.
            let said =
                members.iter().filter_map(|j| placed.get(*j)).filter_map(|(i, _)| devs.get(*i)).find_map(|dv| solve::style_said(&dv.words.join(" ")));
            let (style, from) = match (said, project_style) {
                (Some(st), _) => (Some(st), "its product record (type, submittal) names the style"),
                (None, Some(st)) => (Some(st), "the project's plate style"),
                (None, None) => (None, "rules data (derived): the usual plate for its gangs"),
            };
            // Gangs are the distinct positions: devices at one spot (read twice, or stacked
            // from two sheets) are one gang, and said.
            let mut spots: Vec<f64> = members.iter().filter_map(|j| placed.get(*j)).map(|(_, pl)| pl.t / inch).collect();
            spots.sort_by(f64::total_cmp);
            let mut n_gangs = 0u32;
            let mut last = f64::NEG_INFINITY;
            for t in spots {
                if t - last > STACKED_IN {
                    n_gangs += 1;
                    last = t;
                }
            }
            stacked += members.len().saturating_sub(n_gangs as usize);
            let name = cx.faces.get(*fk).map_or_else(String::new, |f| format!("{}#{}", f.name, g + 1));
            let mut pl = solve::plate(&rules, style, n_gangs.max(1), from);
            if n_gangs > 1 {
                ganged += 1;
                let along: Vec<f64> = members.iter().filter_map(|j| placed.get(*j)).map(|(_, p)| p.t / inch).collect();
                let lo = along.iter().copied().fold(f64::INFINITY, f64::min);
                let hi = along.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                pl.from = format!(
                    "{}; ganged (derived: plates would overlap): {n_gangs} devices within {} in of each other at one height, closer than a plate's width + gap ({} in)",
                    pl.from,
                    r2(hi - lo),
                    r2(spacing)
                );
                if ganged_list.len() < MAX_LISTED {
                    ganged_list.push(json!({
                        "plate": name,
                        "gangs": n_gangs,
                        "devices": members.iter().filter_map(|j| placed.get(*j)).filter_map(|(i, _)| devs.get(*i)).map(|dv| dv.h.hex()).collect::<Vec<_>>(),
                        "span_in": r2(hi - lo),
                        "spacing_in": r2(spacing),
                        "why": "derived: plates would overlap",
                    }));
                }
            }
            for j in &members {
                plate_of.insert(*j, (pl.clone(), name.clone(), members.len()));
            }
        }
    }
    // Write each device's placement.
    let mut offsets: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut heights: BTreeMap<String, usize> = BTreeMap::new();
    let mut tie_count: BTreeMap<String, usize> = BTreeMap::new();
    let mut solves: BTreeMap<String, usize> = BTreeMap::new();
    let mut styles: BTreeMap<String, usize> = BTreeMap::new();
    let in_wall = in_wall_items(d);
    let mut framing_cache: HashMap<usize, (Vec<f64>, Vec<Obstacle>)> = HashMap::new();
    let mut changes: Vec<(Handle, Props)> = unplaced;
    let mut raised: Vec<(Handle, String, Vec<super::forms::Given>)> = Vec::new();
    let mut agree: Vec<Handle> = Vec::new();
    let mut conflicts_first: Vec<Value> = Vec::new();
    let mut drawn_not_used: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (j, (i, pl)) in placed.iter().enumerate() {
        let (Some(dv), Some(f)) = (devs.get(*i), cx.faces.get(pl.face)) else { continue };
        let floor = level_floor(&cx.sp, dv.level.as_deref(), cx.upf);
        let mut store = dv.store.clone();
        // The build's direction-of-face is kept under its own name; host_face is the face's name.
        if let Some(n) = dv.normal() {
            store.set("face_normal", Prop::new(vec_text(n), Source::computed()).with_note("the face's outward direction (plan): toward its room"));
        }
        store.set(
            "host_face",
            Prop::new(f.name.clone(), Source::computed())
                .with_note(&format!("the wall face it is in: the room it faces and the compass side (plan north {} degrees from +Y)", cx.north)),
        );
        // Height: the documents' number to the stated point; else what the model has.
        let datum = store.get("mount_datum").map(Prop::text).map(|t| super::mountlist::Datum::parse(&t)).unwrap_or_default();
        let z_ref = match datum {
            super::mountlist::Datum::Top => dv.z1,
            super::mountlist::Datum::Bottom => dv.z0,
            super::mountlist::Datum::Centre => pl.zc,
        };
        let (h_in, h_src, h_note) = match store
            .get("mount_height")
            .filter(|p| p.has_value())
            .and_then(|p| Some((p.value.as_f64().or_else(|| p.text().parse().ok())?, p.source.clone())))
        {
            Some((v, src)) => {
                let note = if src.kind.is_assumed() {
                    "not documented: a default (marked), the device keeps it until a document gives one"
                } else {
                    "documented: a hard number, never moved by the solve"
                };
                (v, src, note)
            }
            None => (r2((z_ref - floor) / inch), Source::computed(), "measured from the model: no document gives it"),
        };
        *heights.entry(h_src.kind.as_str().to_string()).or_default() += 1;
        store.set("height_aff", Prop::new(r2(h_in), h_src).with_unit("in").with_note(h_note));
        store.set(
            "height_ref",
            Prop::new(datum.name(), Source::computed()).with_note("the point of the device (its plate) the height is measured to"),
        );
        store.set(
            "height_tolerance",
            Prop::new(tol_h, Source::from_doc(SourceKind::Rule, "rules data: layout_rules.json height_tolerance_in"))
                .with_unit("in")
                .with_note("height is hard"),
        );
        // Along: from the reference.
        let (off_src, off_note) = match &pl.documented {
            Some(why) => {
                *offsets.entry("documented").or_default() += 1;
                let src = pl.drawn.as_ref().map_or_else(|| Source::of(SourceKind::Print), |(_, src)| src.clone());
                (src, format!("documented: {why}"))
            }
            None => {
                *offsets.entry("measured").or_default() += 1;
                (Source::computed(), "derived: measured from the plan (no document dimensions it)".to_string())
            }
        };
        store.set(
            "offset_from",
            Prop::new(pl.reference.name().to_string(), Source::computed())
                .with_note("the face (or face end, or opening edge) its along-face offset is measured from"),
        );
        store.set("offset_along", Prop::new(r2(pl.offset / inch), off_src).with_unit("in").with_note(&format!("to the device's centre; {off_note}")));
        store.set(
            "offset_depth",
            Prop::new(r2(pl.depth_front / inch), Source::computed())
                .with_unit("in")
                .with_note("its front from the finished face: flush 0, proud +, recessed −"),
        );
        // Plate.
        if let Some((plate, group, n)) = plate_of.get(&j) {
            let src = if plate.from.starts_with("rules data") {
                Source::from_doc(SourceKind::Rule, "rules data: layout_rules.json faceplates")
            } else {
                Source::computed()
            };
            store.set(
                "faceplate",
                Prop::new(format!("{}, {}-gang, {:.2} x {:.2} in", plate.style, plate.gangs, plate.w, plate.h), src.clone()).with_note(&plate.from),
            );
            store.set("faceplate_w", Prop::new(r2(plate.w), src.clone()).with_unit("in"));
            store.set("faceplate_h", Prop::new(r2(plate.h), src).with_unit("in"));
            store.set("plate_group", Prop::new(group.clone(), Source::computed()).with_note(&format!("{n} device(s) under one plate")));
            *styles.entry(format!("{} {}-gang", plate.style, plate.gangs)).or_default() += 1;
        }
        if !plate_of.contains_key(&j) {
            // No faceplate (a plumbing fixture, a luminaire, an alarm device): its box is its extent.
            let what = store.get("kind").map(Prop::text).unwrap_or_else(|| "device".into()).replace('_', " ");
            store.set("faceplate", Prop::new(NO_FACE, Source::computed()).with_note(&format!("no faceplate: a {what}; its own box is its extent")));
            store.set("faceplate_w", Prop::new(r2(pl.along_len / inch), Source::computed()).with_unit("in").with_note("its box"));
            store.set("faceplate_h", Prop::new(r2((dv.z1 - dv.z0) / inch), Source::computed()).with_unit("in").with_note("its box"));
            store.remove("plate_group");
            *styles.entry("none (no faceplate)".into()).or_default() += 1;
        }
        let (pw, ph) = plate_of.get(&j).map_or((pl.along_len / inch, (dv.z1 - dv.z0) / inch), |(p, _, _)| (p.w, p.h));
        let prect = Rect::centred(pl.t / inch, (pl.zc - floor) / inch, pw, ph);
        // Ties: a person's or a document's stay; the rules data's are derived afresh.
        let mut ties: Vec<Tie> = ties_of(&store).into_iter().filter(|t| !t.derived).collect();
        let near_objs: Vec<&Obj> = objs.iter().filter(|o| same_level(o.level.as_deref(), dv.level.as_deref())).collect();
        for t in derive_ties(&rules, f, &near_objs, &prect, inch, &dv.words) {
            let key = if t.pending.is_empty() { t.zone.split(' ').next().unwrap_or("").to_string() } else { t.zone.clone() };
            *tie_count.entry(key).or_default() += 1;
            ties.push(t);
        }
        set_ties(&mut store, &ties);
        // The band and the solve.
        let kind = band_kind(&dv.words, &rules);
        let band = ties.iter().filter(|t| t.axis != "vertical" && t.pending.is_empty()).map(|t| t.plus_minus_in).fold(f64::INFINITY, f64::min);
        let band = if band.is_finite() {
            band
        } else {
            rules["along_band_in"][kind.as_str()].as_f64().or_else(|| rules["along_band_in"]["default"].as_f64()).unwrap_or(6.0)
        };
        let band_src = if ties.iter().any(|t| !t.derived) {
            Source::user()
        } else {
            Source::from_doc(SourceKind::Rule, &format!("rules data: layout_rules.json along_band_in.{kind}"))
        };
        store.set(
            "along_band",
            Prop::new(r2(band), band_src).with_unit("in").with_note("± how far along the wall the box may drift from its target (along is soft)"),
        );
        // A drawn dimension the model's position disagrees with by more than the kind's band:
        // a conflict for review (both values, their sources).
        if let Some(why) = pl.drawn_why {
            *drawn_not_used.entry(why).or_default() += 1;
        }
        if let Some((measured_in, src)) = &pl.drawn {
            let kind_band = rules["along_band_in"][kind.as_str()].as_f64().or_else(|| rules["along_band_in"]["default"].as_f64()).unwrap_or(6.0);
            let documented_in = pl.offset / inch;
            let off = measured_in - documented_in;
            if off.abs() > kind_band + 1e-6 {
                let note = format!(
                    "the drawn dimension says {} from {}; the model has it {} from it ({} off, band ±{})",
                    ft_in(documented_in),
                    pl.reference.name(),
                    ft_in(*measured_in),
                    ft_in(off.abs()),
                    ft_in(kind_band)
                );
                store.set(
                    "offset_conflict",
                    Prop::new(note.clone(), src.clone()).with_note("a conflict for review: the documents and the model disagree"),
                );
                let what = store.get("kind").map(Prop::text).unwrap_or_else(|| "device".into());
                let mut model_note = Prop::new(r2(*measured_in), Source::computed());
                model_note.note = Some(format!("the model's position: {} from {}", ft_in(*measured_in), pl.reference.name()));
                raised.push((
                    dv.h,
                    what,
                    vec![
                        super::forms::Given { value: json!(r2(documented_in)), source: src.clone(), author: String::new() },
                        super::forms::Given { value: json!(r2(*measured_in)), source: Source::computed(), author: String::new() },
                    ],
                ));
                if conflicts_first.len() < MAX_LISTED {
                    conflicts_first.push(json!({"handle": dv.h.hex(), "face": f.name, "from": pl.reference.name(), "drawn_in": r2(documented_in), "model_in": r2(*measured_in), "off_in": r2(off), "band_in": r2(kind_band), "sheet": src.doc, "at": src.at}));
                }
            } else {
                store.remove("offset_conflict");
                agree.push(dv.h);
            }
        } else {
            store.remove("offset_conflict");
        }
        let (studs, obstacles) = framing_cache.entry(pl.face).or_insert_with(|| framing(&in_wall, f, inch)).clone();
        let box_w = rules["box"]["w_one_gang_in"].as_f64().unwrap_or(2.25)
            + rules["box"]["per_gang_in"].as_f64().unwrap_or(1.81) * (plate_of.get(&j).map_or(1, |p| p.2) as f64 - 1.0).max(0.0);
        let solved = solve::solve(pl.t / inch, band, box_w, &studs, rules["stud"]["width_in"].as_f64().unwrap_or(1.5), &obstacles);
        let (status, solved_at, drift, why) = match &solved {
            Solved::PendingFraming => {
                ("pending framing".to_string(), None, None, "no studs in the model yet: the band is kept, nothing is solved".to_string())
            }
            Solved::At { at, drift, why } => ("solved".to_string(), Some(*at), Some(*drift), why.clone()),
            Solved::Conflict { why, suggest } => ("conflict".to_string(), None, None, format!("{why}; {}", suggest.join("; "))),
        };
        *solves.entry(status.clone()).or_default() += 1;
        store.set("layout_solve", Prop::new(status, Source::computed()).with_note(&why));
        match (solved_at, drift) {
            (Some(at), Some(dr)) => {
                store.set(
                    "along_solved",
                    Prop::new(r2(pl.offset / inch + dr), Source::computed())
                        .with_unit("in")
                        .with_note(&format!("solved box centre {at:.1} in along the face")),
                );
                store.set("drift", Prop::new(r2(dr), Source::computed()).with_unit("in").with_note(&why));
            }
            _ => {
                store.remove("along_solved");
                store.remove("drift");
            }
        }
        // What follow needs to re-derive it.
        let reference = pl.reference.to_json();
        let lref = json!({"wall": f.wall.hex(), "inward": vec_text(f.inward), "u": vec_text(f.u), "ref": reference, "box": [r2(pl.along_len / inch), r2(pl.across / inch), r2((dv.z1 - dv.z0) / inch)]});
        store.set("layout_ref", Prop::new(lref.to_string(), Source::computed()).with_note("the walls and reference the placement is measured from"));
        changes.push((dv.h, store));
    }
    let dm = s.doc_mut()?;
    for (h, store) in changes {
        if let Some(e) = dm.model.get(h) {
            let xdata = props::write(&e.common.xdata, &store);
            let _ = dm.modify_entity(h, |x| x.common.xdata = xdata.into());
        }
    }
    let conflict_ids = super::forms::raise_conflicts(s, "offset_along", &raised, &agree)?;
    let placed_n = placed.len();
    let drawn_read: usize = drawn_dims.by_sheet.values().map(|r| r.read).sum();
    let mut drawn_sheets: BTreeMap<&str, usize> = BTreeMap::new();
    for p in &drawn_dims.placed {
        *drawn_sheets.entry(p.sheet.as_str()).or_default() += 1;
    }
    Ok(json!({
        "faces": cx.faces.len(),
        "by_level": faces_by_level,
        "plan_north": format!("{} degrees counter-clockwise from +Y", cx.north),
        "devices": {"wall": devs.len(), "on_face": placed_n, "no_face": no_face_n, "no_face_by": no_face_by, "no_face_first": no_face},
        "offsets": offsets,
        "documented_share_pct": if placed_n > 0 { r2(100.0 * offsets.get("documented").copied().unwrap_or(0) as f64 / placed_n as f64) } else { 0.0 },
        "heights": heights,
        "constraints": tie_count,
        "objects": objs.len(),
        "plates": {"by_style": styles, "ganged": ganged, "ganged_first": ganged_list, "stacked": stacked, "stacked_why": "devices at the same spot on a face as another (within 1 in): read twice or from two sheets; one gang"},
        "solve": solves,
        "fields": LAYOUT_FIELDS,
        "drawn_dimensions": {
            "read": drawn_read,
            "placed": drawn_dims.placed.len(),
            "placed_by_sheet": drawn_sheets,
            "devices_documented": placed.iter().filter(|(_, pl)| pl.drawn.is_some()).count(),
            "measured_not_used": drawn_not_used,
            "conflicts": raised.len(),
            "conflict_ids": conflict_ids.iter().take(MAX_LISTED).collect::<Vec<_>>(),
            "conflicts_first": conflicts_first,
            "report": format!("{DIMS} {{}}"),
        },
    }))
}

// ─── bim.layout.dims ────────────────────────────────────────────────────────

fn run_dims(s: &mut Session, p: &Value) -> Result<Value> {
    let north = north_param(DIMS, p)?;
    let limit = p.get("limit").and_then(Value::as_u64).map_or(20, |n| n.min(MAX_LISTED as u64) as usize);
    let sheet = str_param(p, "sheet").map(|t| t.trim().to_ascii_uppercase()).filter(|t| !t.is_empty());
    let kind = str_param(p, "kind").map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty());
    let cx = context(s, DIMS, north)?;
    let rules = solve::rules();
    let inch = cx.inch();
    let d = s.doc()?;
    let devs = devices(d);
    let objs = objects(d, &cx.sp, cx.upf, &rules);
    let mut drawn_dims = super::dimreader::read_drawing(d, &cx.sp);
    if let Some(want) = &sheet {
        drawn_dims.placed.retain(|x| x.sheet.eq_ignore_ascii_case(want));
        drawn_dims.by_sheet.retain(|k, _| k.eq_ignore_ascii_case(want));
    }
    let attached = drawn::attach(&cx, &devs, &objs, &drawn_dims);
    let mut by_wall: HashMap<Handle, Vec<usize>> = HashMap::new();
    let mut by_level: HashMap<String, Vec<usize>> = HashMap::new();
    for (k, f) in cx.faces.iter().enumerate() {
        for w in &f.walls {
            by_wall.entry(*w).or_default().push(k);
        }
        by_level.entry(f.level.clone().unwrap_or_default().to_ascii_lowercase()).or_default().push(k);
    }
    // The conflicts rehost would raise: each documented wall device on its face.
    let mut conflicts: Vec<Value> = Vec::new();
    for dv in devs.iter().filter(|dv| dv.wall_mounted()) {
        let Some(doc) = attached.docs.get(&dv.h) else { continue };
        let Some(pl) = face_for(&cx, &by_wall, &by_level, dv).and_then(|k| place(&cx, k, dv, &[], Some(doc))) else { continue };
        let (Some((measured, src)), Some(f)) = (&pl.drawn, cx.faces.get(pl.face)) else { continue };
        let band = rules["along_band_in"][band_kind(&dv.words, &rules).as_str()]
            .as_f64()
            .or_else(|| rules["along_band_in"]["default"].as_f64())
            .unwrap_or(6.0);
        let drawn_in = pl.offset / inch;
        if (measured - drawn_in).abs() > band + 1e-6 {
            conflicts.push(json!({"handle": dv.h.hex(), "face": f.name, "from": pl.reference.name(), "drawn_in": r2(drawn_in), "model_in": r2(*measured), "off_in": r2(measured - drawn_in), "band_in": r2(band), "sheet": src.doc, "at": src.at}));
        }
    }
    Ok(drawn::report(&drawn_dims, &attached, &devs, &cx, &conflicts, limit, kind.as_deref()))
}

// ─── bim.layout.follow ──────────────────────────────────────────────────────

fn run_follow(s: &mut Session, _p: &Value) -> Result<Value> {
    let (sp, _) = super::bspec::current(s, FOLLOW)?;
    let upf = upf_of(&sp);
    let inch = upf / 12.0;
    let d = s.doc()?;
    let pieces = faces::pieces(d, upf);
    let rules = solve::rules();
    let objs = objects(d, &sp, upf, &rules);
    let mut moves: Vec<(Handle, cadaddy_doc::Mesh, Props)> = Vec::new();
    let mut kept = 0usize;
    let mut lost: Vec<Value> = Vec::new();
    for dv in devices(d) {
        let Some(lref) = dv.text("layout_ref").and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        let (Some(wall), Some(inward), Some(reference)) =
            (lref["wall"].as_str().and_then(Handle::parse_hex), lref["inward"].as_str().and_then(parse_vec), Ref::from_json(&lref["ref"]))
        else {
            continue;
        };
        let c = dv.c();
        let zc_old = (dv.z0 + dv.z1) / 2.0;
        let Some((a, u, _)) = wall_line(&pieces, wall, inward, c, Some(zc_old)) else {
            if lost.len() < MAX_LISTED {
                lost.push(json!({"handle": dv.h.hex(), "why": "its wall is gone"}));
            }
            continue;
        };
        let floor = level_floor(&sp, dv.level.as_deref(), upf);
        let off = prop_num(&dv.store, "offset_along").unwrap_or(0.0) * inch;
        let depth = prop_num(&dv.store, "offset_depth").unwrap_or(0.0) * inch;
        let h = prop_num(&dv.store, "height_aff").unwrap_or((zc_old - floor) / inch) * inch;
        let datum = dv.text("height_ref").map(|t| super::mountlist::Datum::parse(&t)).unwrap_or_default();
        let bx = &lref["box"];
        let (blen, bw, bh) = (bx[0].as_f64().unwrap_or(4.0) * inch, bx[1].as_f64().unwrap_or(2.0) * inch, bx[2].as_f64().unwrap_or(4.0) * inch);
        // A device tied to an object moves with it; else it keeps its offset from its reference.
        let tie = ties_of(&dv.store).into_iter().find(|t| t.follow && t.axis != "vertical");
        let tied = tie.as_ref().and_then(|t| {
            objs.iter().find(|o| o.h == t.ref_handle).map(|o| {
                let (t0, t1) = span(&o.pts, a, u);
                (t0 + t1) / 2.0 + t.rel_in * inch
            })
        });
        let t_along = match tied {
            Some(along) => Some(along),
            None => reference.along_for(&pieces, a, u, off, c),
        };
        let Some(t_along) = t_along.filter(|t| t.is_finite()) else {
            if lost.len() < MAX_LISTED {
                lost.push(json!({"handle": dv.h.hex(), "why": format!("its reference {} is gone", reference.name())}));
            }
            continue;
        };
        let face_pt = a + u * t_along;
        let ctr = face_pt + inward * (depth - bw / 2.0);
        let (z0, z1) = match datum {
            super::mountlist::Datum::Top => (floor + h - bh, floor + h),
            super::mountlist::Datum::Bottom => (floor + h, floor + h + bh),
            super::mountlist::Datum::Centre => (floor + h - bh / 2.0, floor + h + bh / 2.0),
        };
        if ctr.dist(c) <= 1e-6 * upf && (z0 - dv.z0).abs() <= 1e-6 * upf {
            kept += 1;
            continue;
        }
        let mut mesh = super::wallgen::empty_mesh();
        super::wallgen::add_box(&mut mesh, ctr - u * (blen / 2.0), ctr + u * (blen / 2.0), bw, z0, z1);
        if mesh.faces.is_empty() {
            continue;
        }
        let mut store = dv.store.clone();
        store.set(
            "location",
            Prop::new(format!("{:.2}, {:.2}, {:.2}", ctr.x / upf, ctr.y / upf, (z0 + z1) / 2.0 / upf), Source::computed())
                .with_unit("ft")
                .with_note("model coordinates: derived from its face, its reference, its height and its depth"),
        );
        moves.push((dv.h, mesh, store));
    }
    let n = moves.len();
    let dm = s.doc_mut()?;
    for (h, mesh, store) in moves {
        if let Some(e) = dm.model.get(h) {
            let xdata = props::write(&e.common.xdata, &store);
            let _ = dm.modify_entity(h, |x| {
                x.kind = EntityKind::Mesh(mesh);
                x.common.xdata = xdata.into();
            });
        }
    }
    Ok(json!({"moved": n, "kept": kept, "lost": lost}))
}

// ─── G-LAYOUT ───────────────────────────────────────────────────────────────

/// One G-LAYOUT check of a device.
#[derive(Clone, Debug)]
pub(crate) struct Check {
    pub check: &'static str,
    pub verdict: &'static str,
    pub issue: String,
    pub expected: String,
    pub found: String,
    pub measured: Value,
}

fn check(check: &'static str, verdict: &'static str, issue: String, expected: String, found: String, measured: Value) -> Check {
    Check { check, verdict, issue, expected, found, measured }
}

/// The compass word of an along direction on a face (`east`), for an edge's name.
fn edge_word(u: Vec2, sign: f64, north: f64) -> &'static str {
    match faces::compass(u * sign, north).0 {
        "N" => "north",
        "E" => "east",
        "S" => "south",
        _ => "west",
    }
}

/// G-LAYOUT for every placed wall device (only `only` when given): height (hard), along (soft,
/// with the solve), each constraint, the plate's fit (other plates, door and window casings), and
/// its face.
pub(crate) fn checks(d: &Drawing, sp: &BuildingSpec, only: Option<&HashSet<String>>) -> HashMap<String, Vec<Check>> {
    let upf = upf_of(sp);
    let inch = upf / 12.0;
    let rules = solve::rules();
    let pieces = faces::pieces(d, upf);
    let objs = objects(d, sp, upf, &rules);
    let tol_h = rules["height_tolerance_in"].as_f64().unwrap_or(0.25);
    let casing = rules["casing_in"].as_f64().unwrap_or(2.5);
    let clear = rules["plate_clearance_in"].as_f64().unwrap_or(0.0);
    let devs: Vec<Dev> = devices(d).into_iter().filter(|dv| only.is_none_or(|o| o.contains(&dv.h.hex().to_ascii_uppercase()))).collect();
    // Each placed device's plate on its face, for overlaps.
    struct On {
        idx: usize,
        face: String,
        level: Option<String>,
        rect: Rect,
        group: String,
    }
    let mut ons: Vec<On> = Vec::new();
    let mut out: HashMap<String, Vec<Check>> = HashMap::new();
    for (i, dv) in devs.iter().enumerate() {
        let key = dv.h.hex().to_ascii_uppercase();
        let Some(face) = dv.text("host_face").filter(|t| parse_vec(t).is_none()) else { continue };
        if face == NO_FACE {
            let why = dv.store.get("host_face").and_then(|p| p.note.clone()).unwrap_or_default();
            out.entry(key).or_default().push(check(
                "face",
                "unsure",
                format!("no named wall face: {why}"),
                "on a named face".into(),
                "none".into(),
                json!({}),
            ));
            continue;
        }
        let Some(lref) = dv.text("layout_ref").and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        let (Some(wall), Some(inward)) = (lref["wall"].as_str().and_then(Handle::parse_hex), lref["inward"].as_str().and_then(parse_vec)) else {
            continue;
        };
        let c = dv.c();
        let floor = level_floor(sp, dv.level.as_deref(), upf);
        let zc = (dv.z0 + dv.z1) / 2.0;
        let Some((a, u, half)) = wall_line(&pieces, wall, inward, c, Some(zc)) else {
            out.entry(key).or_default().push(check(
                "face",
                "fail",
                format!("its wall {} is gone", wall.hex()),
                format!("in {face}"),
                "no wall".into(),
                json!({}),
            ));
            continue;
        };
        let mut list: Vec<Check> = Vec::new();
        let dep = dot(c - a, inward);
        list.push(check(
            "face",
            "pass",
            format!("on {face}"),
            format!("on {face}"),
            format!("{} in from its face line", r2(dep / inch)),
            json!({"face": face}),
        ));
        // Height: hard.
        let datum = dv.text("height_ref").map(|t| super::mountlist::Datum::parse(&t)).unwrap_or_default();
        let z_ref = match datum {
            super::mountlist::Datum::Top => dv.z1,
            super::mountlist::Datum::Bottom => dv.z0,
            super::mountlist::Datum::Centre => zc,
        };
        let aff = (z_ref - floor) / inch;
        if let Some(h) = prop_num(&dv.store, "height_aff") {
            let off = aff - h;
            let v = if off.abs() <= tol_h + 1e-6 { "pass" } else { "fail" };
            list.push(check(
                "height",
                v,
                if v == "pass" {
                    format!("{} AFF to its {} (hard)", in_text(h), datum.name())
                } else {
                    format!("{} AFF to its {} in the model; the documents say {} (hard, ±{tol_h} in)", in_text(aff), datum.name(), in_text(h))
                },
                format!("{} AFF ±{tol_h} in", in_text(h)),
                in_text(aff),
                json!({"aff_in": r2(aff), "expected_in": r2(h), "off_in": r2(off), "to": datum.name()}),
            ));
        }
        // Along: soft.
        let reference = Ref::from_json(&lref["ref"]);
        let target = prop_num(&dv.store, "offset_along");
        let band = prop_num(&dv.store, "along_band").unwrap_or(6.0);
        let actual = reference.as_ref().and_then(|r| r.offset(&pieces, a, u, c)).map(|x| x / inch);
        let from = dv.text("offset_from").unwrap_or_default();
        let solve_state = dv.text("layout_solve").unwrap_or_else(|| "pending framing".into());
        let documented = dv.store.get("offset_along").is_some_and(|p| p.source.kind == SourceKind::Print);
        if let (Some(t), Some(act)) = (target, actual) {
            let drift = act - t;
            let m = json!({"target_in": r2(t), "actual_in": r2(act), "drift_in": r2(drift), "band_in": r2(band), "from": from, "solve": solve_state, "documented": documented});
            if drift.abs() > band + 1e-6 {
                let what = if documented { "the document's dimension" } else { "its target" };
                list.push(check(
                    "along",
                    "fail",
                    format!("{} from {from} in the model; {what} is {} (band ±{})", ft_in(act), ft_in(t), ft_in(band)),
                    format!("{} from {from} ±{}", ft_in(t), ft_in(band)),
                    ft_in(act),
                    m,
                ));
            } else if solve_state == "solved" {
                list.push(check(
                    "along",
                    "pass",
                    format!("{} from {from}, drift {} inside ±{}", ft_in(act), ft_in(drift), ft_in(band)),
                    format!("{} from {from} ±{}", ft_in(t), ft_in(band)),
                    ft_in(act),
                    m,
                ));
            } else {
                let why = dv.store.get("layout_solve").and_then(|p| p.note.clone()).unwrap_or_default();
                let issue = if solve_state == "conflict" {
                    format!("no legal stud bay inside the band: {why}")
                } else {
                    format!("{solve_state}: target {} from {from}, band ±{} — not passed until the studs and pipes exist", ft_in(t), ft_in(band))
                };
                list.push(check(
                    "along",
                    if solve_state == "conflict" { "fail" } else { "unsure" },
                    issue,
                    format!("{} from {from} ±{}", ft_in(t), ft_in(band)),
                    ft_in(act),
                    m,
                ));
            }
        }
        // The plate and its ties.
        let pw = prop_num(&dv.store, "faceplate_w").unwrap_or(2.75);
        let ph = prop_num(&dv.store, "faceplate_h").unwrap_or(4.5);
        let t_here = dot(c - a, u) / inch;
        let rect = Rect::centred(t_here, (zc - floor) / inch, pw, ph);
        for tie in ties_of(&dv.store) {
            let found = if tie.pending.is_empty() { objs.iter().find(|o| o.h == tie.ref_handle) } else { None };
            let Some(o) = found else {
                // No object to check against (not placed yet, or deleted): never pass or fail.
                let mut t = tie.clone();
                let issue = if t.pending.is_empty() {
                    t.pending = pending_kind(&rules, &t);
                    format!("{}: {} is not in the model any more; checked once it is placed again", t.pending_text(), t.reference)
                } else {
                    format!("{}: no {} object in the model yet; checked once it is placed", t.pending_text(), t.pending.replace('_', " "))
                };
                list.push(check(
                    "constraint",
                    "unsure",
                    issue,
                    format!("{}{}", tie.zone, if tie.derived { " (rules data, derived)" } else { "" }),
                    t.pending_text(),
                    t.to_json(),
                ));
                continue;
            };
            let (r, _, _) = on_face(o, a, u, inward, floor, inch);
            let mid = (r.a0 + r.a1) / 2.0;
            let off = rect.a0 + pw / 2.0 - mid - tie.target_in;
            let mut fails: Vec<String> = Vec::new();
            if tie.axis != "vertical" && off.abs() > tie.plus_minus_in + 1e-6 {
                fails.push(format!("centre {} off {}'s centre (±{})", ft_in(off.abs()), o.name, ft_in(tie.plus_minus_in)));
            }
            if tie.inside {
                for (edge, by) in rect.outside(&r, clear) {
                    let side = match edge {
                        "left" => edge_word(u, -1.0, 0.0),
                        "right" => edge_word(u, 1.0, 0.0),
                        e => e,
                    };
                    fails.push(format!("plate {edge} edge {} past {} ({side} edge)", ft_in(by), o.name));
                }
            }
            let m = json!({"constraint": tie.to_json(), "centre_off_in": r2(off), "plate": [r2(rect.a0), r2(rect.z0), r2(rect.a1), r2(rect.z1)], "zone": [r2(r.a0), r2(r.z0), r2(r.a1), r2(r.z1)]});
            let verdict = if fails.is_empty() { "pass" } else { "fail" };
            let issue = if fails.is_empty() {
                format!("{}: within ±{} of {}'s centre, plate inside", tie.zone, ft_in(tie.plus_minus_in), o.name)
            } else {
                format!("{}: {}", tie.zone, fails.join("; "))
            };
            let derived = if tie.derived { " (rules data, derived)" } else { "" };
            list.push(check(
                "constraint",
                verdict,
                issue,
                format!("{}{derived}", tie.zone),
                format!("centre {} from {}'s centre", ft_in(off), o.name),
                m,
            ));
        }
        let plated = dv.text("faceplate").is_some_and(|p| p != NO_FACE);
        if plated {
            ons.push(On { idx: i, face: face.clone(), level: dv.level.clone(), rect, group: dv.text("plate_group").unwrap_or_else(|| key.clone()) });
        }
        // Casings beside openings on its wall: a piece over an opening starts above the floor.
        let openings: Vec<(f64, f64, f64)> = pieces
            .iter()
            .filter(|p| plated && p.wall == wall && p.z0 > floor + inch * 2.0 && dep.abs() <= half * 2.0 + inch * 3.0)
            .map(|p| {
                let (s0, s1) = (dot(p.a - a, u) / inch, dot(p.b - a, u) / inch);
                (s0.min(s1), s0.max(s1), (p.z0 - floor) / inch)
            })
            .collect();
        let casing_hit: Vec<String> = casing_hits(&rect, &openings, casing).into_iter().map(|w| format!("{w} along {face}")).collect();
        if !casing_hit.is_empty() {
            list.push(check(
                "plate",
                "fail",
                casing_hit.join("; "),
                format!("plate clear of door and window casings ({casing} in)"),
                "on a casing".into(),
                json!({"plate": [r2(rect.a0), r2(rect.z0), r2(rect.a1), r2(rect.z1)]}),
            ));
        }
        out.entry(key).or_default().extend(list);
    }
    // Plates that overlap (not one plate's ganged devices).
    for (x, p) in ons.iter().enumerate() {
        let others: Vec<&On> = ons
            .iter()
            .skip(x + 1)
            .filter(|q| q.face == p.face && same_level(q.level.as_deref(), p.level.as_deref()) && q.group != p.group && q.rect.overlaps(&p.rect))
            .collect();
        for q in others {
            let (Some(dp), Some(dq)) = (devs.get(p.idx), devs.get(q.idx)) else { continue };
            // Plates at one height were ganged; these stand at different heights (a spot read
            // twice with two heights, or two boxes stacked too close): say both.
            let zc = |r: &Rect| (r.z0 + r.z1) / 2.0;
            for ((me, mr), (other, or)) in [((dp, &p.rect), (dq, &q.rect)), ((dq, &q.rect), (dp, &p.rect))] {
                out.entry(me.h.hex().to_ascii_uppercase()).or_default().push(check(
                    "plate",
                    "fail",
                    format!(
                        "its plate (centre {} AFF) overlaps the plate of {} (centre {} AFF) on {}: not one height, so not one ganged box; which height do the documents mean?",
                        in_text(zc(mr)),
                        other.h.hex(),
                        in_text(zc(or)),
                        p.face
                    ),
                    "plates apart (ganged devices share one plate)".into(),
                    "overlapping plates".into(),
                    json!({"other": other.h.hex(), "centre_in": r2(zc(mr)), "other_centre_in": r2(zc(or))}),
                ));
            }
        }
    }
    // A plate that passed every fit says so once.
    let plated: HashSet<String> = ons.iter().filter_map(|o| devs.get(o.idx)).map(|d| d.h.hex().to_ascii_uppercase()).collect();
    for (key, list) in out.iter_mut() {
        if plated.contains(key) && list.iter().any(|c| c.check == "face" && c.verdict == "pass") && !list.iter().any(|c| c.check == "plate") {
            list.push(check(
                "plate",
                "pass",
                "its plate is clear of casings and other plates".into(),
                "plate clear".into(),
                "clear".into(),
                json!({}),
            ));
        }
    }
    out
}

/// The casings a plate (`rect`, inches on a face) lies over: each opening's `(from, to, head)`
/// with a casing `casing` wide beside it and over its head.
fn casing_hits(rect: &Rect, openings: &[(f64, f64, f64)], casing: f64) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for &(o0, o1, head) in openings {
        let zones = [
            (Rect { a0: o0 - casing, a1: o0, z0: 0.0, z1: head + casing }, "beside"),
            (Rect { a0: o1, a1: o1 + casing, z0: 0.0, z1: head + casing }, "beside"),
            (Rect { a0: o0 - casing, a1: o1 + casing, z0: head, z1: head + casing }, "over"),
        ];
        for (z, word) in zones {
            if rect.overlaps(&z) {
                let msg = format!("plate over the casing {word} the opening at {}–{}", ft_in(o0), ft_in(o1));
                if !out.contains(&msg) {
                    out.push(msg);
                }
            }
        }
    }
    out
}

// ─── bim.layout ─────────────────────────────────────────────────────────────

fn run_layout(s: &mut Session, p: &Value) -> Result<Value> {
    let room = str_param(p, "room")
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .ok_or_else(|| bad(LAYOUT, "`room` is required: its number or name"))?
        .to_string();
    let level = str_param(p, "level").map(str::trim).filter(|l| !l.is_empty()).map(str::to_string);
    let north = north_param(LAYOUT, p)?;
    // Devices not yet placed on their faces are placed first.
    let unplaced = devices(s.doc()?).iter().filter(|d| d.wall_mounted()).any(|d| d.text("host_face").is_none_or(|t| parse_vec(&t).is_some()));
    if unplaced {
        rehost(s, north, None)?;
    }
    let cx = context(s, LAYOUT, north)?;
    let inch = cx.inch();
    let rules = solve::rules();
    let mut fs: Vec<&Face> = cx
        .faces
        .iter()
        .filter(|f| {
            f.room.eq_ignore_ascii_case(&room) && level.as_deref().is_none_or(|l| f.level.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(l)))
        })
        .collect();
    if fs.is_empty() {
        return Err(bad(LAYOUT, format!("no room {room} with wall faces{}", level.as_deref().map(|l| format!(" on {l}")).unwrap_or_default())));
    }
    let levels: HashSet<Option<String>> = fs.iter().map(|f| f.level.clone()).collect();
    if levels.len() > 1 {
        return Err(bad(LAYOUT, format!("room {room} is on {} levels: give `level`", levels.len())));
    }
    fs.sort_by_key(|f| f.order);
    let lvl = fs.first().and_then(|f| f.level.clone());
    let d = s.doc()?;
    let devs = devices(d);
    let only: HashSet<String> = devs
        .iter()
        .filter(|dv| {
            dv.text("host_face").is_some_and(|n| fs.iter().any(|f| f.name.eq_ignore_ascii_case(&n)))
                && same_level(dv.level.as_deref(), lvl.as_deref())
        })
        .map(|dv| dv.h.hex())
        .collect();
    let checked = checks(d, &cx.sp, Some(&only));
    let mut lines: Vec<String> = Vec::new();
    let mut out_faces: Vec<Value> = Vec::new();
    for f in &fs {
        let mut on: Vec<(f64, &Dev)> = devs
            .iter()
            .filter(|dv| only.contains(&dv.h.hex()) && dv.text("host_face").is_some_and(|n| n.eq_ignore_ascii_case(&f.name)))
            .map(|dv| (f.along(dv.c()), dv))
            .collect();
        on.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut items: Vec<String> = Vec::new();
        let mut dj: Vec<Value> = Vec::new();
        for (t, dv) in &on {
            let what = abbrev(&dv.words, dv.text("type_mark").as_deref(), &rules);
            let h = prop_num(&dv.store, "height_aff");
            let hard = dv.store.get("height_aff").is_some_and(|p| !p.source.kind.is_assumed() && p.source.kind != SourceKind::Computed);
            let datum = match dv.text("height_ref").as_deref() {
                Some("top") => " to top",
                Some("bottom") => " to bottom",
                _ => " ctr",
            };
            let mut text = match h {
                Some(h) => format!("{what} {} AFF{datum} ({})", in_text(h), if hard { "hard" } else { "default height" }),
                None => what.clone(),
            };
            let ties = ties_of(&dv.store);
            if let Some(tie) = ties.iter().find(|t| t.pending.is_empty()) {
                text.push_str(&format!("; ctr {} ±{}", tie.zone, ft_in(tie.plus_minus_in)));
                if tie.derived {
                    text.push_str(" (rules data)");
                }
            }
            for tie in ties.iter().filter(|t| !t.pending.is_empty()) {
                text.push_str(&format!("; {}", tie.pending_text()));
            }
            if let (Some(off), Some(from)) = (prop_num(&dv.store, "offset_along"), dv.text("offset_from")) {
                let documented = dv.store.get("offset_along").is_some_and(|p| p.source.kind == SourceKind::Print);
                text.push_str(&format!("; {} from {from} {}", ft_in(off), if documented { "(documented)" } else { "target (from plan)" }));
                match dv.text("layout_solve").as_deref() {
                    Some("solved") => {
                        if let Some(sv) = prop_num(&dv.store, "along_solved") {
                            let why = dv.store.get("drift").and_then(|p| p.note.clone()).unwrap_or_default();
                            text.push_str(&format!(", solved {} ({why})", ft_in(sv)));
                        }
                    }
                    Some(st) => text.push_str(&format!(", {st}")),
                    None => {}
                }
                if let Some(b) = prop_num(&dv.store, "along_band") {
                    text.push_str(&format!(", band ±{}", ft_in(b)));
                }
            }
            if let Some(pl) = dv.text("faceplate") {
                text.push_str(&format!("; plate {pl}"));
            }
            let fails: Vec<String> =
                checked.get(&dv.h.hex()).into_iter().flatten().filter(|c| c.verdict == "fail").map(|c| c.issue.clone()).collect();
            if !fails.is_empty() {
                text.push_str(&format!(" — FAIL: {}", fails.join("; ")));
            }
            items.push(text.clone());
            dj.push(json!({"handle": dv.h.hex(), "text": text, "along_in": r2(t / inch), "height_aff_in": h, "offset_from": dv.text("offset_from"), "offset_along_in": prop_num(&dv.store, "offset_along"), "fails": fails}));
        }
        let ops: Vec<String> =
            f.openings.iter().map(|o| format!("{} {} wide at {}", o.kind, ft_in((o.to - o.from) / inch), ft_in(o.from / inch))).collect();
        let head = format!(
            "{} ({}{} long, {} high{})",
            f.name,
            f.wall_type.as_deref().map(|t| format!("wall type {t}, ")).unwrap_or_default(),
            ft_in(f.len / inch),
            ft_in((f.top - f.floor) / inch),
            if ops.is_empty() { String::new() } else { format!("; {}", ops.join(", ")) }
        );
        lines.push(if items.is_empty() { format!("{head}: no devices") } else { format!("{head}: {}", items.join("; ")) });
        let mut fj = face_json(f, inch);
        fj["devices"] = json!(dj);
        out_faces.push(fj);
    }
    let text = lines.join("\n");
    let mut out = json!({"room": room, "level": lvl, "faces": out_faces, "text": text});
    if bool_or(p, "draw", false) {
        let sheet = draw_sheet(
            s,
            &room,
            &fs.iter().map(|f| (*f).clone()).collect::<Vec<_>>(),
            &devs.into_iter().filter(|d| only.contains(&d.h.hex())).collect::<Vec<_>>(),
            inch,
        )?;
        out["sheet"] = sheet;
    }
    Ok(out)
}

/// A layout sheet for the room: its faces unfolded side by side (each its length and height,
/// openings, plates), a dimension from the reference end of each face to each device and its
/// height above the floor, at 1/2" = 1'-0".
fn draw_sheet(s: &mut Session, room: &str, fs: &[Face], devs: &[Dev], inch: f64) -> Result<Value> {
    let clean: String = room.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == ' ').take(40).collect();
    let base = format!("LAYOUT {}", clean.trim());
    let mut name = base.clone();
    for k in 2..100 {
        if !s.doc()?.layouts.iter().any(|l| l.name.eq_ignore_ascii_case(&name)) {
            break;
        }
        name = format!("{base} ({k})");
    }
    s.execute("layout.new", &json!({"name": name, "viewport": false}))?;
    let space = Space::Paper(name.clone());
    // Paper inches per drawing unit.
    let k = SHEET_SCALE / inch;
    let layer = "A-LAYOUT-FIELD";
    let mut ents: Vec<EntityKind> = Vec::new();
    let line = |a: (f64, f64), b: (f64, f64)| EntityKind::Line(Line { a: Vec3 { x: a.0, y: a.1, z: 0.0 }, b: Vec3 { x: b.0, y: b.1, z: 0.0 } });
    let text = |at: (f64, f64), h: f64, v: &str| {
        EntityKind::Text(Text {
            insert: Vec3 { x: at.0, y: at.1, z: 0.0 },
            align_pt: None,
            height: h,
            value: v.chars().take(200).collect(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: Default::default(),
            valign: Default::default(),
        })
    };
    let mut x0 = 1.0;
    let y0 = 2.0;
    let mut dims: Vec<(Vec2, Vec2, Vec2)> = Vec::new();
    ents.push(text((1.0, 0.6), 0.2, &format!("{} — WALL LAYOUT (faces unfolded in corner order, 1/2\" = 1'-0\")", room)));
    for f in fs {
        let (w, h) = (f.len * k, (f.top - f.floor) * k);
        for (a, b) in [((x0, y0), (x0 + w, y0)), ((x0 + w, y0), (x0 + w, y0 + h)), ((x0 + w, y0 + h), (x0, y0 + h)), ((x0, y0 + h), (x0, y0))] {
            ents.push(line(a, b));
        }
        ents.push(text((x0, y0 - 0.35), 0.12, &format!("{}{}", f.name, f.wall_type.as_deref().map(|t| format!(" ({t})")).unwrap_or_default())));
        for o in &f.openings {
            let (a0, a1) = (x0 + o.from * k, x0 + o.to * k);
            let (b0, b1) = (y0 + (o.bottom - f.floor) * k, y0 + (o.top - f.floor) * k);
            for (a, b) in [((a0, b0), (a1, b0)), ((a1, b0), (a1, b1)), ((a1, b1), (a0, b1)), ((a0, b1), (a0, b0))] {
                ents.push(line(a, b));
            }
        }
        for dv in devs.iter().filter(|d| d.text("host_face").is_some_and(|n| n.eq_ignore_ascii_case(&f.name))) {
            let t = f.along(dv.c());
            let zc = (dv.z0 + dv.z1) / 2.0 - f.floor;
            let pw = prop_num(&dv.store, "faceplate_w").unwrap_or(2.75) * inch * k;
            let ph = prop_num(&dv.store, "faceplate_h").unwrap_or(4.5) * inch * k;
            let (cx, cy) = (x0 + t * k, y0 + zc * k);
            for (a, b) in [
                ((cx - pw / 2.0, cy - ph / 2.0), (cx + pw / 2.0, cy - ph / 2.0)),
                ((cx + pw / 2.0, cy - ph / 2.0), (cx + pw / 2.0, cy + ph / 2.0)),
                ((cx + pw / 2.0, cy + ph / 2.0), (cx - pw / 2.0, cy + ph / 2.0)),
                ((cx - pw / 2.0, cy + ph / 2.0), (cx - pw / 2.0, cy - ph / 2.0)),
            ] {
                ents.push(line(a, b));
            }
            // From the reference end to the centre (below the face), and the floor to the centre.
            let from_start = t <= f.len - t;
            let rx = if from_start { x0 } else { x0 + w };
            dims.push((Vec2::new(rx, y0), Vec2::new(cx, cy), Vec2::new((rx + cx) / 2.0, y0 - 0.15)));
            dims.push((Vec2::new(cx, y0), Vec2::new(cx, cy), Vec2::new(cx + 0.25, (y0 + cy) / 2.0)));
            let what = abbrev(&dv.words, dv.text("type_mark").as_deref(), &solve::rules());
            ents.push(text((cx + pw, cy + ph), 0.08, &what));
        }
        x0 += w + 1.0;
    }
    let mut made = 0usize;
    let mut handles: Vec<String> = Vec::new();
    let dm = s.doc_mut()?;
    for kind in ents {
        if let Ok(h) = dm.add(&space, Common { layer: layer.into(), ..Common::default() }, kind) {
            made += 1;
            if handles.len() < 5 {
                handles.push(h.hex());
            }
        }
    }
    for (a, b, at) in &dims {
        let vertical = (a.x - b.x).abs() < 1e-9;
        let kind =
            super::annotate::layout_dim(s, DimKind::Linear { rotation: if vertical { std::f64::consts::FRAC_PI_2 } else { 0.0 } }, *at, *a, *b);
        if s.doc_mut()?.add(&space, Common { layer: layer.into(), ..Common::default() }, kind).is_ok() {
            made += 1;
        }
    }
    Ok(json!({"layout": name, "entities": made, "dimensions": dims.len()}))
}

// ─── bim.layout.tie / bim.layout.object ───────────────────────────────

fn run_constrain(s: &mut Session, p: &Value) -> Result<Value> {
    let h = str_param(p, "handle").and_then(|t| Handle::parse_hex(t.trim())).ok_or_else(|| bad(CONSTRAIN, "`handle` (the device) is required"))?;
    let reference = str_param(p, "ref")
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .ok_or_else(|| bad(CONSTRAIN, "`ref` (an object's handle or tag) is required"))?;
    let pm = p
        .get("plus_minus_in")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.0 && *v <= 1_000.0)
        .ok_or_else(|| bad(CONSTRAIN, "`plus_minus_in` must be inches from 0 to 1000"))?;
    let target = match p.get("target_in") {
        None | Some(Value::Null) => 0.0,
        Some(v) => v.as_f64().filter(|v| v.is_finite() && v.abs() <= 10_000.0).ok_or_else(|| bad(CONSTRAIN, "`target_in` must be inches"))?,
    };
    let ref_point = str_param(p, "ref_point").unwrap_or("center").to_ascii_lowercase();
    if !matches!(ref_point.as_str(), "center" | "centre" | "edge" | "face") {
        return Err(bad(CONSTRAIN, "`ref_point` is center, edge or face"));
    }
    let axis = str_param(p, "axis").unwrap_or("along").to_ascii_lowercase();
    if !matches!(axis.as_str(), "along" | "vertical" | "both") {
        return Err(bad(CONSTRAIN, "`axis` is along, vertical or both"));
    }
    let (sp, _) = super::bspec::current(s, CONSTRAIN)?;
    let upf = upf_of(&sp);
    let inch = upf / 12.0;
    let rules = solve::rules();
    let d = s.doc()?;
    let objs = objects(d, &sp, upf, &rules);
    let obj = Handle::parse_hex(reference)
        .and_then(|rh| objs.iter().find(|o| o.h == rh))
        .or_else(|| objs.iter().find(|o| o.name.eq_ignore_ascii_case(reference)))
        .ok_or_else(|| bad(CONSTRAIN, format!("no casework or appliance object {reference} (bim.layout.object adds one)")))?
        .clone();
    let e = d.model.get(h).ok_or_else(|| bad(CONSTRAIN, format!("no element {}", h.hex())))?;
    let dv = devices(d).into_iter().find(|x| x.h == h).ok_or_else(|| bad(CONSTRAIN, format!("{} is not a built device", h.hex())))?;
    let lref = dv.text("layout_ref").and_then(|t| serde_json::from_str::<Value>(&t).ok());
    let u = lref.as_ref().and_then(|l| l["u"].as_str()).and_then(parse_vec).unwrap_or(Vec2::new(1.0, 0.0));
    let c = dv.c();
    let (t0, t1) = span(&obj.pts, c, u);
    let rel = -(t0 + t1) / 2.0 / inch;
    let tie = Tie {
        reference: obj.name.clone(),
        ref_handle: obj.h,
        ref_point,
        axis,
        target_in: target,
        plus_minus_in: pm,
        inside: bool_or(p, "inside", false),
        zone: if bool_or(p, "inside", false) { format!("inside {}", obj.name) } else { format!("on {}", obj.name) },
        follow: bool_or(p, "follow", true),
        rel_in: rel,
        source: str_param(p, "source").map(|t| t.chars().take(300).collect()).unwrap_or_default(),
        pending: String::new(),
        ref_kind: obj.kind.clone(),
        derived: false,
        why: String::new(),
    };
    let mut store = props::read(&e.common.xdata);
    let mut ties: Vec<Tie> = ties_of(&store).into_iter().filter(|t| t.ref_handle != obj.h).collect();
    ties.insert(0, tie.clone());
    set_ties(&mut store, &ties);
    let xdata = props::write(&e.common.xdata, &store);
    s.doc_mut()?.modify_entity(h, |x| x.common.xdata = xdata.into())?;
    Ok(json!({"handle": h.hex(), "constraint": tie.to_json()}))
}

fn run_object(s: &mut Session, p: &Value) -> Result<Value> {
    let kind = str_param(p, "kind")
        .map(|k| k.trim().to_ascii_lowercase().replace(' ', "_"))
        .filter(|k| !k.is_empty() && k.len() <= 60)
        .ok_or_else(|| bad(OBJECT, "`kind` is required (base_cabinet, upper_cabinet, refrigerator, …)"))?;
    let b: Vec<f64> = p
        .get("box")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).filter(|v| v.is_finite() && v.abs() < 1e9).collect())
        .unwrap_or_default();
    let ([x0, y0, z0, x1, y1, z1], true) = (<[f64; 6]>::try_from(b.as_slice()).unwrap_or([0.0; 6]), b.len() == 6) else {
        return Err(bad(OBJECT, "`box` must be [x0, y0, z0, x1, y1, z1]"));
    };
    if !(x1 > x0 && y1 > y0 && z1 > z0) {
        return Err(bad(OBJECT, "`box` must have its second corner above and beyond its first"));
    }
    let tag = str_param(p, "tag").map(|t| t.trim().chars().take(60).collect::<String>()).unwrap_or_default();
    let level = str_param(p, "level").map(|t| t.trim().chars().take(60).collect::<String>()).filter(|l| !l.is_empty());
    let source = str_param(p, "source").map(|t| t.trim().chars().take(200).collect::<String>()).unwrap_or_default();
    let mut mesh = super::wallgen::empty_mesh();
    super::wallgen::add_box(&mut mesh, Vec2::new(x0, (y0 + y1) / 2.0), Vec2::new(x1, (y0 + y1) / 2.0), y1 - y0, z0, z1);
    let id = format!("object:{kind}:{tag}");
    let mut x = vec![RawTag { code: 1001, value: CASEWORK_APP.into() }];
    for v in [id.as_str(), tag.as_str(), kind.as_str(), if source.is_empty() { "user" } else { "document" }, source.as_str()] {
        x.push(RawTag { code: 1000, value: v.to_string() });
    }
    let mut store = Props::default();
    store.set("kind", Prop::new(kind.clone(), if source.is_empty() { Source::user() } else { Source::from_doc(SourceKind::Submittal, &source) }));
    if let Some(l) = &level {
        store.set("level", Prop::new(l.clone(), Source::user()));
    }
    if !tag.is_empty() {
        store.set("tag", Prop::new(tag.clone(), Source::user()));
    }
    let xdata = props::write(&x, &store);
    let h = s.doc_mut()?.add(
        &Space::Model,
        Common { layer: "A-FURN-CASEWORK".into(), xdata: xdata.into(), ..Common::default() },
        EntityKind::Mesh(mesh),
    )?;
    Ok(json!({"handle": h.hex(), "kind": kind, "tag": tag}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_lengths() {
        assert_eq!(ft_in(15.0), "15 in");
        assert_eq!(ft_in(15.5), "15 1/2 in");
        assert_eq!(ft_in(110.0), "9 ft 2 in");
        assert_eq!(ft_in(108.0), "9 ft");
        assert_eq!(in_text(66.0), "66 in");
        assert_eq!(in_text(46.5), "46 1/2 in");
        assert_eq!(len_in("9'-2\""), Some(110.0));
        assert_eq!(len_in("1'-6 1/2\""), Some(18.5));
        assert_eq!(len_in("15\""), Some(15.0));
    }

    #[test]
    fn a_plate_over_a_door_casing_fails_and_one_clear_of_it_passes() {
        // A 36" door from 30" to 66" along the face, its head at 84"; casings 2.5" wide.
        let door = [(30.0, 66.0, 84.0)];
        let on_casing = Rect::centred(28.5, 48.0, 2.75, 4.5);
        let clear = Rect::centred(24.0, 48.0, 2.75, 4.5);
        assert_eq!(casing_hits(&on_casing, &door, 2.5).len(), 1);
        assert!(casing_hits(&on_casing, &door, 2.5)[0].contains("beside"));
        assert!(casing_hits(&clear, &door, 2.5).is_empty());
    }
}
