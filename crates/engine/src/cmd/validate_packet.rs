//! The evidence packet for one area (Q6115): what an agent judging the model against the prints
//! looks at, for one room, one stretch of a corridor or one equipment room at a time.
//!
//! - `validate.areas {level?, kind?, area?, cursor?, limit?}`: the areas to check. Every model
//!   room (the build's room elements) with its number, name, level and outline; a room or
//!   corridor longer than about 40 ft is split into stretches of at most 40 ft; a custom window
//!   `{area: [x0, y0, x1, y1] (building feet), level}` is listed as a `zone`. Ids are stable:
//!   `<level>/<room number or name>`, `#n` for a stretch, `<level>/win:x0,y0,x1,y1` for a window.
//! - `validate.packet {area, trades?, out_dir, force?, margin_ft?, dpi?, max_images?}` writes a
//!   folder and returns its manifest:
//!   - `prints/`: a PNG crop of every sheet that shows the area: each trade's plan, enlarged
//!     plans, the RCP (the area's box, plus a margin, carried onto the sheet by the sheet's
//!     registration on its level's plan, else by the view placement the job kept, else the plan
//!     itself), interior elevations naming the room, the partition legend and the schedule or
//!     legend rows of the type marks present, general-notes blocks, and the submittal pages of
//!     the products present. A crop is at least 150 dpi of the printed sheet (the paper size is
//!     taken from the sheet's shape, ARCH/ANSI/ISO), in tiles when large; the manifest gives
//!     each crop's sheet rectangle and pixel scale, so a sheet point is at
//!     `((x − x0)·s, (y1 − y)·s)` pixels.
//!   - `model/`: the model over each plan crop (the print in grey, the model by trade colour:
//!     architecture blue, electrical red, mechanical green, plumbing cyan, fire magenta), a
//!     clean plan of the model alone, and when the model is built a 3D view of the area and an
//!     elevation of each of its longest walls (`model.render` with a camera).
//!   - `objects.json`: every model element in the area (handle, kind, trade, mark, tag, name,
//!     level, room, position, height above the floor, host wall and face, mounting, size,
//!     system, circuit, switch leg, maker, catalog number, submittal, source sheet and point, and
//!     the `bim.validate` failures already found for it), the runs, walls and openings.
//!   - `rules.json`: the validation rules (`validate.rules`) that apply to the kinds present,
//!     with the project's overrides applied.
//!   - `prompt.md`: the judging agent's instructions, made from those rules.
//!
//! The same drawing and area give byte-identical `manifest.json`, `objects.json`, `rules.json`
//! and `prompt.md` (no times, no absolute paths, sorted, rounded); the pictures are capped
//! (`max_images`, default 40) and what was left out is listed with why.
//!
//! Where crops come from where the job does not store a view (interior elevations, schedule and
//! legend rows, general notes), the crop is a fixed share of the sheet round the text that names
//! the room or the mark: a reading aid, never a measurement.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cadaddy_doc::color::Color;
use cadaddy_doc::props::SourceKind;
use cadaddy_doc::{Common, Drawing, Entity, EntityKind, Space};
use cadaddy_geom::{Bounds2, Vec2, Vec3};
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::levels::{SheetLink, SheetRegistration};
use cadaddy_walk3d::validate::{self as v, Model, Mounting, Prints, Scope};
use serde_json::{Value, json};

use super::bspec::current;
use super::validate::{Frame, PROV_APP, model_of, prints_from_job, prints_of};
use super::validate_prescreen::{self as ps, UnitOf};
use super::validate_rules::{self as vr, Effective};
use super::*;
use crate::{Result, Session};

pub const CMD_AREAS: &str = "validate.areas";
pub const CMD_PACKET: &str = "validate.packet";

/// The longest stretch of one area, feet.
const STRETCH_FT: f64 = 40.0;
/// A room this much longer than a stretch is split.
const SPLIT_OVER_FT: f64 = 44.0;
const DEFAULT_MARGIN_FT: f64 = 5.0;
const DEFAULT_DPI: f64 = 150.0;
const MAX_DPI: f64 = 600.0;
/// Largest side of one picture, pixels: bigger crops are tiled.
const TILE_PX: u32 = 2000;
/// Most tiles of one crop (the resolution drops below the asked dpi past it, and says so).
const MAX_TILES_SIDE: u32 = 4;
const DEFAULT_IMAGES: usize = 40;
const MAX_IMAGES: usize = 200;
/// Objects looked at, at most.
const MAX_ENTITIES: usize = 2_000_000;
/// Objects drawn in one picture, at most.
const MAX_CROP_ENTITIES: usize = 400_000;
const MAX_AREAS: usize = 50_000;
const DEFAULT_AREA_LIMIT: usize = 100;
const MAX_AREA_LIMIT: usize = 500;
/// Row and note crops of one sheet, at most.
const MAX_TEXT_CROPS_PER_SHEET: usize = 6;
const MAX_ELEVATIONS: usize = 6;
/// Enlarged views of a room's unit type, and riser diagram crops, in one packet at most.
const MAX_UNIT_VIEWS: usize = 6;
/// The views a unit type lists (its plans, RCP, trades' plans, elevations) taken at most.
const MAX_TYPE_VIEWS: usize = 40;
const MAX_RISERS: usize = 4;
/// Elements, runs, walls listed in objects.json, at most (each).
const MAX_OBJECTS: usize = 5_000;
/// Outline points listed for an area.
const MAX_OUTLINE: usize = 200;
/// A submittal folder's files looked through.
const MAX_FOLDER_FILES: usize = 2_000;
/// The long side of a submittal page picture, pixels (≈ 150 dpi of a letter page).
const SUBMITTAL_PX: u32 = 1650;
/// What a model element may sit off its wall's centreline and still be hosted by it, feet.
const HOST_REACH_FT: f64 = 1.5;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD_AREAS, "Validation Areas", run_areas)
            .params("{level?: name, kind?: room|zone|equipment_room|corridor, area?: [x0,y0,x1,y1] (building feet: a custom window, listed as a zone; needs `level` when the building has several), cursor?: n, limit?: n (100, at most 500)} → {total, cursor, areas: [{id, kind, level, number?, name?, room?, stretch?: [i, n], outline: [[x,y]] (feet), box: [x0,y0,x1,y1] (feet)}], next?, levels}: the areas to check, one room, corridor stretch (≤ 40 ft) or equipment room each; ids are stable")
            .noundo(),
        CommandSpec::new(CMD_PACKET, "Validation Evidence Packet", run_packet)
            .params("{area: id from validate.areas | {area: [x0,y0,x1,y1] (building feet), level?}, out_dir: folder, trades?: [architecture|electrical|mechanical|plumbing|fire|general] (default all), rules?: [rule id] (only these of the rules that apply; default all), force?: bool (replace files of an earlier packet; otherwise they are refused), margin_ft?: 0–50 (5), dpi?: 150–600 (150), max_images?: 1–200 (40)} → the manifest: {packet, area, prints: [{file, sheet, title, kind, trade, sheet_rect, px, px_per_unit, dpi, paper, tile?, placed_by}], model: [{file, kind, of?, sheet_rect?, px?, wall?}], submittals, objects, rules, prompt, counts, left_out: [{what, why}]}: writes prints/, model/, objects.json, rules.json, prompt.md and manifest.json into out_dir")
            .noundo()
            .files(),
    ]
}

// ---------------------------------------------------------------- small helpers

/// Rounded for stable output: hundredths, and no negative zero.
pub(crate) fn r2(x: f64) -> f64 {
    let y = (x * 100.0).round() / 100.0;
    if y == 0.0 || !y.is_finite() { 0.0 } else { y }
}

fn r4(x: f64) -> f64 {
    let y = (x * 10_000.0).round() / 10_000.0;
    if y == 0.0 || !y.is_finite() { 0.0 } else { y }
}

pub(crate) fn pt(p: [f64; 2]) -> Value {
    json!([r2(p[0]), r2(p[1])])
}

fn rect_json(r: [f64; 4]) -> Value {
    json!([r2(r[0]), r2(r[1]), r2(r[2]), r2(r[3])])
}

pub(crate) fn bbox(pts: &[[f64; 2]]) -> Option<[f64; 4]> {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in pts.iter().filter(|p| p[0].is_finite() && p[1].is_finite()) {
        b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
    }
    (b[0] <= b[2] && b[1] <= b[3]).then_some(b)
}

pub(crate) fn grow(r: [f64; 4], by: f64) -> [f64; 4] {
    [r[0] - by, r[1] - by, r[2] + by, r[3] + by]
}

fn intersect(a: [f64; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

pub(crate) fn inside(r: [f64; 4], p: [f64; 2]) -> bool {
    p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3]
}

fn corners(r: [f64; 4]) -> [[f64; 2]; 4] {
    [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]]
}

fn norm_rect(r: [f64; 4]) -> [f64; 4] {
    [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

/// A file-name-safe piece of a sheet number or id.
fn safe(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).take(40).collect();
    if t.is_empty() { "x".into() } else { t }
}

pub(crate) fn upper_words(s: &str) -> String {
    s.to_ascii_uppercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------------------------------------------------------------- areas

/// One area to check, in building feet on its level's plan.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Area {
    pub id: String,
    pub kind: &'static str,
    pub level: String,
    pub number: Option<String>,
    pub name: Option<String>,
    /// The room element's handle.
    pub room: Option<String>,
    /// `[i, n]`: stretch i (from 1) of n.
    pub stretch: Option<[usize; 2]>,
    pub outline: Vec<[f64; 2]>,
    pub bbox: [f64; 4],
}

impl Area {
    fn to_json(&self) -> Value {
        let mut o = json!({
            "id": self.id,
            "kind": self.kind,
            "level": self.level,
            "outline": self.outline.iter().take(MAX_OUTLINE).map(|p| pt(*p)).collect::<Vec<_>>(),
            "box": rect_json(self.bbox),
        });
        if let Some(m) = o.as_object_mut() {
            if let Some(n) = &self.number {
                m.insert("number".into(), json!(n));
            }
            if let Some(n) = &self.name {
                m.insert("name".into(), json!(n));
            }
            if let Some(h) = &self.room {
                m.insert("room".into(), json!(h));
            }
            if let Some(s) = self.stretch {
                m.insert("stretch".into(), json!(s));
            }
        }
        o
    }
}

/// What a room is for the packet, by its name.
fn room_kind(name: &str) -> &'static str {
    let n = format!(" {} ", upper_words(&name.replace(['.', '-', '/', '_'], " ")));
    const CORRIDOR: [&str; 5] = [" CORRIDOR ", " CORR ", " HALL ", " HALLWAY ", " PASSAGE "];
    const EQUIPMENT: [&str; 19] = [
        " ELEC ",
        " ELECTRICAL ",
        " MECH ",
        " MECHANICAL ",
        " EQUIP ",
        " EQUIPMENT ",
        " PUMP ",
        " BOILER ",
        " IDF ",
        " MDF ",
        " TELECOM ",
        " DATA ",
        " SERVER ",
        " RISER ",
        " SPRINKLER ",
        " MACHINE ",
        " GENERATOR ",
        " TRANSFORMER ",
        " METER ",
    ];
    if CORRIDOR.iter().any(|k| n.contains(k)) {
        "corridor"
    } else if EQUIPMENT.iter().any(|k| n.contains(k)) {
        "equipment_room"
    } else {
        "room"
    }
}

pub(crate) fn point_in(poly: &[[f64; 2]], p: [f64; 2]) -> bool {
    let pts: Vec<Vec2> = poly.iter().map(|q| Vec2::new(q[0], q[1])).collect();
    pts.len() >= 3 && cadaddy_geom::point_in_polygon(&pts, Vec2::new(p[0], p[1]))
}

/// Whether a cell of a room's box holds any of the room.
fn cell_touches(poly: &[[f64; 2]], cell: [f64; 4]) -> bool {
    let c = [(cell[0] + cell[2]) / 2.0, (cell[1] + cell[3]) / 2.0];
    point_in(poly, c) || poly.iter().any(|p| inside(cell, *p)) || corners(cell).iter().any(|q| point_in(poly, *q))
}

/// The model's rooms as areas, long ones in stretches (sorted by level, then id).
pub(crate) fn model_areas(d: &Drawing, frame: &Frame) -> Vec<Area> {
    let mut rooms = super::bspec::room_elements(d);
    rooms.sort_by(|a, b| {
        (a.level.as_str(), a.number.as_deref().unwrap_or(&a.name), a.handle.0).cmp(&(
            b.level.as_str(),
            b.number.as_deref().unwrap_or(&b.name),
            b.handle.0,
        ))
    });
    let mut out: Vec<Area> = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for r in rooms.iter().filter(|r| r.outline.len() >= 3) {
        let outline: Vec<[f64; 2]> = r.outline.iter().take(10_000).map(|p| frame.to_ft_on(&r.level, *p)).map(|p| [r4(p[0]), r4(p[1])]).collect();
        let Some(b) = bbox(&outline) else { continue };
        let key = r.number.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(r.name.trim());
        let mut id = format!("{}/{}", r.level.trim(), key.split_whitespace().collect::<Vec<_>>().join(" "));
        let n = seen.entry(id.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            id = format!("{id}~{n}");
        }
        let kind = room_kind(&r.name);
        let (w, h) = (b[2] - b[0], b[3] - b[1]);
        let split = w.max(h) > SPLIT_OVER_FT || (kind == "corridor" && w.max(h) > STRETCH_FT);
        let base = Area {
            id: id.clone(),
            kind,
            level: r.level.clone(),
            number: r.number.clone(),
            name: Some(r.name.clone()).filter(|n| !n.trim().is_empty()),
            room: Some(r.handle.hex()),
            stretch: None,
            outline: outline.clone(),
            bbox: b,
        };
        if !split {
            out.push(base);
        } else {
            let nx = ((w / STRETCH_FT).ceil() as usize).clamp(1, 100);
            let ny = ((h / STRETCH_FT).ceil() as usize).clamp(1, 100);
            let (cw, ch) = (w / nx as f64, h / ny as f64);
            let mut cells = Vec::new();
            for j in 0..ny {
                for i in 0..nx {
                    let cell = [b[0] + cw * i as f64, b[1] + ch * j as f64, b[0] + cw * (i + 1) as f64, b[1] + ch * (j + 1) as f64];
                    if cell_touches(&outline, cell) {
                        cells.push(cell);
                    }
                }
            }
            let total = cells.len();
            for (k, cell) in cells.into_iter().enumerate() {
                out.push(Area {
                    id: format!("{id}#{}", k + 1),
                    stretch: Some([k + 1, total]),
                    outline: corners(cell).iter().map(|p| [r4(p[0]), r4(p[1])]).collect(),
                    bbox: cell,
                    ..base.clone()
                });
            }
        }
        if out.len() >= MAX_AREAS {
            break;
        }
    }
    out.truncate(MAX_AREAS);
    out
}

/// A custom window on a level.
fn window_area(level: &str, r: [f64; 4]) -> Area {
    let r = norm_rect(r);
    Area {
        id: format!("{}/win:{:.2},{:.2},{:.2},{:.2}", level.trim(), r2(r[0]), r2(r[1]), r2(r[2]), r2(r[3])),
        kind: "zone",
        level: level.to_string(),
        number: None,
        name: None,
        room: None,
        stretch: None,
        outline: corners(r).to_vec(),
        bbox: r,
    }
}

fn four(cmd: &str, v: &Value, what: &str) -> Result<[f64; 4]> {
    let n: Vec<f64> = v.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    match n[..] {
        [a, b, c, d] if [a, b, c, d].iter().all(|x| x.is_finite() && x.abs() < 1e9) && a != c && b != d => Ok(norm_rect([a, b, c, d])),
        _ => Err(bad(cmd, format!("`{what}` is [x0, y0, x1, y1] in building feet (finite, not empty)"))),
    }
}

/// The level a call names (else the only one), checked against the spec.
fn level_param(cmd: &str, sp: &BuildingSpec, p: &Value, need: bool) -> Result<Option<String>> {
    match str_param(p, "level").map(str::trim).filter(|l| !l.is_empty()) {
        Some(l) => sp.level(l).map(|x| Some(x.name.clone())).ok_or_else(|| bad(cmd, format!("there is no level named {l}"))),
        None if need => match sp.levels.as_slice() {
            [only] => Ok(Some(only.name.clone())),
            [] => Err(bad(cmd, "the building spec has no levels yet")),
            _ => Err(bad(cmd, "say which `level` the window is on")),
        },
        None => Ok(None),
    }
}

fn run_areas(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = CMD_AREAS;
    let kind = match str_param(p, "kind").map(|k| k.trim().to_ascii_lowercase()) {
        None => None,
        Some(k) if ["room", "zone", "equipment_room", "corridor"].contains(&k.as_str()) => Some(k),
        Some(k) => return Err(bad(CMD, format!("unknown kind {k} (room, zone, equipment_room, corridor)"))),
    };
    let cursor = match p.get("cursor") {
        None | Some(Value::Null) => 0,
        Some(c) => c.as_u64().map(|c| c.min(MAX_AREAS as u64) as usize).ok_or_else(|| bad(CMD, "`cursor` is a number from `next`"))?,
    };
    let limit = match p.get("limit") {
        None | Some(Value::Null) => DEFAULT_AREA_LIMIT,
        Some(l) => {
            l.as_u64().filter(|l| *l >= 1).map(|l| l.min(MAX_AREA_LIMIT as u64) as usize).ok_or_else(|| bad(CMD, "`limit` is a number from 1"))?
        }
    };
    let (sp, _) = current(s, CMD)?;
    let window = p.get("area").filter(|v| !v.is_null()).map(|a| four(CMD, a, "area")).transpose()?;
    let level = level_param(CMD, &sp, p, window.is_some())?;
    let d = s.doc()?;
    let (_, frame) = model_of(d, &sp);
    let mut areas = model_areas(d, &frame);
    if let (Some(w), Some(l)) = (window, level.as_ref()) {
        areas.push(window_area(l, w));
    }
    areas.retain(|a| level.as_ref().is_none_or(|l| a.level.eq_ignore_ascii_case(l)) && kind.as_deref().is_none_or(|k| a.kind == k));
    let total = areas.len();
    let mut listed: Vec<Value> = Vec::new();
    let mut used = 200;
    for a in areas.iter().skip(cursor).take(limit) {
        let j = a.to_json();
        let n = j.to_string().len() + 1;
        if used + n > super::agentready::ANSWER_MAX && !listed.is_empty() {
            break;
        }
        used += n;
        listed.push(j);
    }
    let next = cursor + listed.len();
    let mut out = json!({
        "total": total,
        "cursor": cursor,
        "areas": listed,
        "levels": sp.levels.iter().map(|l| l.name.clone()).collect::<Vec<_>>(),
    });
    if next < total {
        out["next"] = json!(next);
    }
    if total == 0 && window.is_none() {
        out["note"] = json!("no model rooms yet (model.build makes them from the plan's room tags); give a window with `area` and `level`");
    }
    Ok(out)
}

/// The area a packet call names.
pub(crate) fn area_param(cmd: &str, sp: &BuildingSpec, p: &Value, d: &Drawing, frame: &Frame) -> Result<Area> {
    match p.get("area") {
        Some(Value::String(id)) => {
            let id = id.trim();
            if let Some((lv, w)) = id.split_once("/win:") {
                let n: Vec<f64> = w.split(',').filter_map(|x| x.trim().parse::<f64>().ok()).collect();
                let lv = sp.level(lv).map(|l| l.name.clone()).ok_or_else(|| bad(cmd, format!("there is no level named {lv}")))?;
                return Ok(window_area(&lv, four(cmd, &json!(n), "area")?));
            }
            model_areas(d, frame)
                .into_iter()
                .find(|a| a.id.eq_ignore_ascii_case(id))
                .ok_or_else(|| bad(cmd, format!("there is no area {id} (validate.areas lists them)")))
        }
        Some(o @ Value::Object(_)) => {
            let w = four(cmd, o.get("area").unwrap_or(&Value::Null), "area.area")?;
            let lv = level_param(cmd, sp, o, true)?.unwrap_or_default();
            Ok(window_area(&lv, w))
        }
        _ => Err(bad(cmd, "`area` is an id from validate.areas, or {area: [x0, y0, x1, y1] (building feet), level}")),
    }
}

// ---------------------------------------------------------------- sheets

/// The trade of a sheet discipline.
pub(crate) fn trade_of_discipline(d: &str) -> &'static str {
    match d.trim().to_ascii_uppercase().as_str() {
        "A" | "ID" | "I" | "AD" => "architecture",
        "E" | "EL" | "FA" | "T" | "TC" | "LV" => "electrical",
        "M" | "H" | "HVAC" | "ME" => "mechanical",
        "P" | "PL" | "MP" => "plumbing",
        "FP" | "F" | "FS" => "fire",
        _ => "general",
    }
}

/// What a sheet (or one view of it) is, by its title.
pub(crate) fn sheet_kind(title: &str, has_level: bool) -> &'static str {
    let t = format!(" {} ", upper_words(&title.replace(['.', '-', '/', '_', ','], " ")));
    let has = |k: &str| t.contains(k);
    if has(" REFLECTED ") || has(" RCP ") || has(" CEILING PLAN ") {
        "rcp"
    } else if has(" INTERIOR ELEV") {
        "interior_elevation"
    } else if has(" PARTITION ") || has(" WALL TYPE") {
        "partition_legend"
    } else if has(" RISER DIAGRAM") || has(" RISERS ") || has(" ISOMETRIC") {
        "riser_diagram"
    } else if has(" SCHEDULE") {
        "schedule"
    } else if has(" LEGEND") || has(" SYMBOL") || has(" ABBREVIATION") {
        "legend"
    } else if has(" GENERAL NOTES ") || (!has_level && has(" NOTES ")) {
        "general_notes"
    } else if has(" SPECIFICATION") {
        "spec"
    } else if has(" ENLARGED ") || has(" UNIT PLAN") || has(" PARTIAL ") {
        "enlarged_plan"
    } else if has_level {
        "plan"
    } else {
        "other"
    }
}

/// The paper a sheet of this shape is printed on: (long side in inches, name).
fn paper_of(rect: [f64; 4]) -> (f64, &'static str) {
    const PAPERS: [(f64, f64, &str); 6] = [
        (36.0, 24.0, "ARCH D 24x36"),
        (42.0, 30.0, "ARCH E1 30x42"),
        (48.0, 36.0, "ARCH E 36x48"),
        (34.0, 22.0, "ANSI D 22x34"),
        (33.11, 23.39, "ISO A1"),
        (17.0, 11.0, "ANSI B 11x17"),
    ];
    let (w, h) = ((rect[2] - rect[0]).abs(), (rect[3] - rect[1]).abs());
    let aspect = if w.min(h) > 0.0 { w.max(h) / w.min(h) } else { 1.5 };
    let mut best = (f64::INFINITY, 36.0, "ARCH D 24x36");
    for (l, s, n) in PAPERS {
        let e = (aspect - l / s).abs();
        if e + 1e-9 < best.0 {
            best = (e, l, n);
        }
    }
    (best.1, best.2)
}

/// How a crop's sheet rectangle maps to the project's (the level's reference plan's).
#[derive(Clone, Debug)]
enum Place {
    /// The plan itself (the model is drawn on it).
    Same,
    /// Sheet → project.
    Fit(SheetRegistration),
}

impl Place {
    fn to_sheet(&self, p: [f64; 2]) -> Option<[f64; 2]> {
        match self {
            Place::Same => Some(p),
            Place::Fit(r) => r.to_sheet(p),
        }
    }
    /// Sheet units per project unit.
    fn scale(&self) -> f64 {
        match self {
            Place::Same => 1.0,
            Place::Fit(r) => {
                let s = r.scale();
                if s.is_finite() && s > 1e-12 { 1.0 / s } else { 1.0 }
            }
        }
    }
}

/// One picture of a print: a crop (or a tile of one) of a sheet.
#[derive(Clone, Debug)]
struct Crop {
    file: String,
    sheet: String,
    title: String,
    kind: &'static str,
    trade: &'static str,
    view: Option<String>,
    rect: [f64; 4],
    ppu: f64,
    px: [u32; 2],
    dpi: f64,
    paper: &'static str,
    tile: Option<[u32; 2]>,
    placed_by: &'static str,
    /// For area crops: how to draw the model on it.
    place: Option<Place>,
    /// Why it is there (a mark, the room's number, "GENERAL NOTES").
    shows: Option<String>,
    /// How the view is turned and mirrored against the room on the plan.
    orientation: Option<Value>,
}

impl Crop {
    fn to_json(&self) -> Value {
        let mut o = json!({
            "file": self.file,
            "sheet": self.sheet,
            "title": self.title,
            "kind": self.kind,
            "trade": self.trade,
            "sheet_rect": json!([r4(self.rect[0]), r4(self.rect[1]), r4(self.rect[2]), r4(self.rect[3])]),
            "px": self.px,
            "px_per_unit": r4(self.ppu),
            "dpi": r2(self.dpi),
            "paper": self.paper,
            "placed_by": self.placed_by,
        });
        if let Some(m) = o.as_object_mut() {
            if let Some(vw) = &self.view {
                m.insert("view".into(), json!(vw));
            }
            if let Some(t) = self.tile {
                m.insert("tile".into(), json!(t));
            }
            if let Some(sh) = &self.shows {
                m.insert("shows".into(), json!(sh));
            }
            if let Some(or) = &self.orientation {
                m.insert("orientation".into(), or.clone());
            }
        }
        o
    }
}

/// A turn, degrees counter-clockwise in [0, 360), to a tenth.
fn turn_deg(rad: f64) -> f64 {
    let d = (rad.to_degrees().rem_euclid(360.0) * 10.0).round() / 10.0;
    if d >= 360.0 || !d.is_finite() { 0.0 } else { r2(d) }
}

/// How a crop reads against the plan, in words, when it is turned or mirrored.
fn turn_words(o: &Value) -> Option<String> {
    let deg = o.get("rotation_deg").and_then(Value::as_f64);
    let mirrored = o.get("mirrored").and_then(Value::as_bool).unwrap_or(false);
    let turned = deg.filter(|d| d.abs() > 0.05 && (d - 360.0).abs() > 0.05);
    match (turned, mirrored) {
        (None, false) if deg.is_some() => None,
        (None, false) => Some("its turn against the room is not known: match it by the walls and doors".into()),
        (Some(d), false) => Some(format!("drawn turned {d}° from the room as the plan shows it")),
        (None, true) => Some("drawn mirrored (the room is the opposite hand)".into()),
        (Some(d), true) => Some(format!("drawn mirrored and turned {d}° from the room as the plan shows it")),
    }
}

/// A view's turn against the plan from its registration (a fit has no mirror).
fn orientation_of(r: &SheetRegistration, by: &str) -> Value {
    let mut o = json!({"rotation_deg": turn_deg(r.b.atan2(r.a)), "mirrored": false, "scale": r4(r.scale()), "by": by});
    if let Some(w) = turn_words(&o) {
        o["read"] = json!(w);
    }
    o
}

/// How a unit type's layout (its items at (u, v), 0–1 across and up its enlarged plan) lies in
/// this room: the quarter turn and mirror that bring the items nearest the room's elements of
/// the same kind (ties keep the plain layout).
pub(crate) fn layout_fit(ty: &Value, area: &Area, elements: &[(String, [f64; 2])]) -> Option<Value> {
    let items: Vec<(String, [f64; 2])> = ty
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(500)
        .filter_map(|i| {
            let (u, v) = (i.get("u")?.as_f64()?, i.get("v")?.as_f64()?);
            let k = i.get("kind")?.as_str()?.trim().to_ascii_lowercase();
            (u.is_finite() && v.is_finite() && (-0.5..=1.5).contains(&u) && (-0.5..=1.5).contains(&v) && !k.is_empty()).then_some((k, [u, v]))
        })
        .collect();
    if items.len() < 2 || elements.is_empty() {
        return None;
    }
    let b = area.bbox;
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    let same = |a: &str, b: &str| {
        let (a, b) = (format!("_{}_", a.replace([' ', '-', '.'], "_")), format!("_{}_", b.replace([' ', '-', '.'], "_")));
        a.contains(&b) || b.contains(&a)
    };
    let mut best: Option<(usize, f64, u32, bool)> = None;
    for mirrored in [false, true] {
        for k in 0..4u32 {
            let (mut sum, mut n) = (0.0, 0usize);
            for (kind, [u, v]) in &items {
                let u = if mirrored { 1.0 - u } else { *u };
                let (x, y) = (u - 0.5, v - 0.5);
                let (x, y) = match k {
                    1 => (-y, x),
                    2 => (-x, -y),
                    3 => (y, -x),
                    _ => (x, y),
                };
                let p = [b[0] + (x + 0.5) * w, b[1] + (y + 0.5) * h];
                let near = elements
                    .iter()
                    .filter(|(ek, _)| same(&ek.to_ascii_lowercase(), kind))
                    .map(|(_, q)| (q[0] - p[0]).hypot(q[1] - p[1]))
                    .fold(f64::INFINITY, f64::min);
                if near.is_finite() {
                    sum += near;
                    n += 1;
                }
            }
            if n < 2 {
                continue;
            }
            let mean = sum / n as f64;
            if best.is_none_or(|(bn, bm, _, _)| n > bn || (n == bn && mean < bm - 1e-6)) {
                best = Some((n, mean, k, mirrored));
            }
        }
    }
    best.map(|(n, mean, k, mirrored)| json!({"rotation_deg": f64::from(k * 90), "mirrored": mirrored, "fit_ft": r2(mean), "matched": n, "of": items.len()}))
}

/// A sheet rectangle cut into pictures at `ppu` pixels per unit: whole pixels, tiles of at most
/// [`TILE_PX`]; past [`MAX_TILES_SIDE`] tiles a side the scale drops (returned).
fn tiles(rect: [f64; 4], ppu: f64) -> (f64, Vec<([f64; 4], [u32; 2], [u32; 2])>) {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let longest = w.max(h) * ppu;
    let cap = f64::from(TILE_PX * MAX_TILES_SIDE);
    let ppu = if longest > cap { ppu * cap / longest } else { ppu };
    let wpx = ((w * ppu).ceil().max(16.0)).min(cap) as u32;
    let hpx = ((h * ppu).ceil().max(16.0)).min(cap) as u32;
    let nx = wpx.div_ceil(TILE_PX).max(1);
    let ny = hpx.div_ceil(TILE_PX).max(1);
    let mut out = Vec::new();
    let tile_u = f64::from(TILE_PX) / ppu;
    for j in 0..ny {
        for i in 0..nx {
            let tw = (wpx - i * TILE_PX).min(TILE_PX);
            let th = (hpx - j * TILE_PX).min(TILE_PX);
            let x0 = rect[0] + f64::from(i) * tile_u;
            let y1 = rect[3] - f64::from(j) * tile_u;
            let r = [x0, y1 - f64::from(th) / ppu, x0 + f64::from(tw) / ppu, y1];
            out.push((r, [tw, th], [j * nx + i + 1, nx * ny]));
        }
    }
    (ppu, out)
}

/// The sheets' texts (upper case), where and how tall, for the row and note crops.
struct Texts(Vec<(String, [f64; 2], f64)>);

impl Texts {
    fn of(d: &Drawing, within: &[[f64; 4]]) -> Texts {
        let mut out = Vec::new();
        for e in d.model.iter().take(MAX_ENTITIES) {
            let (value, at, h) = match &e.kind {
                EntityKind::Text(t) => (t.value.as_str(), t.insert.xy(), t.height),
                EntityKind::MText(t) => (t.contents.as_str(), t.insert.xy(), t.height),
                _ => continue,
            };
            let at = [at.x, at.y];
            if !at.iter().all(|v| v.is_finite()) || !h.is_finite() || !within.iter().any(|r| inside(*r, at)) {
                continue;
            }
            out.push((upper_words(&value.chars().take(400).collect::<String>()), at, h.abs()));
        }
        Texts(out)
    }
    /// Texts in `rect` that equal `key` (or, with `contains`, hold it), in a stable order.
    fn find(&self, rect: [f64; 4], key: &str, contains: bool) -> Vec<([f64; 2], f64)> {
        let k = upper_words(key);
        if k.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<([f64; 2], f64)> = self
            .0
            .iter()
            .filter(|(t, at, _)| inside(rect, *at) && if contains { format!(" {t} ").contains(&format!(" {k} ")) } else { *t == k })
            .map(|(_, at, h)| (*at, *h))
            .collect();
        // Top to bottom, left to right.
        hits.sort_by(|a, b| b.0[1].total_cmp(&a.0[1]).then(a.0[0].total_cmp(&b.0[0])));
        hits
    }
}

// ---------------------------------------------------------------- the model in the area

/// The trade an element kind belongs to: the first trade rule that applies, else by its words.
fn trade_of_kind(kind: &str, rules: &[Effective]) -> &'static str {
    if let Some(r) =
        rules.iter().find(|r| r.rule.trade != "general" && r.rule.applies_to.kinds.iter().all(|k| k != "*") && r.rule.applies_to_kind(kind))
    {
        return vr::TRADES.iter().find(|t| **t == r.rule.trade).copied().unwrap_or("general");
    }
    let k = format!("_{}_", kind.to_ascii_lowercase().replace([' ', '-', '.'], "_"));
    let has = |w: &[&str]| w.iter().any(|x| k.contains(&format!("_{x}_")) || k.contains(x));
    if has(&["wall", "door", "window", "room", "ceiling", "floor", "stair", "column", "roof"]) {
        "architecture"
    } else if has(&["sprinkler", "standpipe"]) {
        "fire"
    } else if has(&["duct", "diffuser", "grille", "fan", "ahu", "rtu", "vav", "damper", "louver"]) {
        "mechanical"
    } else if has(&["pipe", "plumbing", "lavatory", "sink", "water", "toilet", "drain", "fixture"]) {
        "plumbing"
    } else if has(&[
        "receptacle",
        "outlet",
        "switch",
        "panel",
        "light",
        "luminaire",
        "conduit",
        "cable",
        "detector",
        "strobe",
        "junction",
        "disconnect",
        "transformer",
    ]) {
        "electrical"
    } else {
        "general"
    }
}

fn trade_colour(trade: &str) -> u8 {
    match trade {
        "architecture" => 5,
        "electrical" => 1,
        "mechanical" => 3,
        "plumbing" => 4,
        "fire" => 6,
        _ => 30,
    }
}

/// Whether an object is the model's (the build's, or an element with properties or provenance)
/// rather than the prints' linework.
fn is_model_object(e: &Entity) -> bool {
    super::wallgen::made_as(e).is_some()
        || e.common.xdata.iter().any(|t| {
            t.code == 1001 && {
                let a = t.value.trim();
                a.eq_ignore_ascii_case(PROV_APP) || a.eq_ignore_ascii_case(cadaddy_doc::props::APP)
            }
        })
}

pub(crate) fn seg_dist(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, [f64; 2]) {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 1e-12 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let q = [a[0] + t * dx, a[1] + t * dy];
    ((p[0] - q[0]).hypot(p[1] - q[1]), q)
}

pub(crate) fn compass(dx: f64, dy: f64) -> &'static str {
    if dx.abs() >= dy.abs() {
        if dx >= 0.0 { "east" } else { "west" }
    } else if dy >= 0.0 {
        "north"
    } else {
        "south"
    }
}

fn segment_touches(r: [f64; 4], a: [f64; 2], b: [f64; 2]) -> bool {
    if inside(r, a) || inside(r, b) {
        return true;
    }
    // Sampled: a run or wall crossing the box.
    (1..32).any(|i| {
        let t = f64::from(i) / 32.0;
        inside(r, [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t])
    })
}

/// A property's text, the first of `keys` the element's store or its type has.
fn field(props: &[(String, String)], fields: &BTreeMap<String, String>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| {
            props
                .iter()
                .find(|(pk, _)| pk.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.clone())
                .or_else(|| fields.iter().find(|(fk, _)| fk.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone()))
        })
        .map(|v| v.trim().chars().take(200).collect::<String>())
        .filter(|v| !v.is_empty())
}

/// A part the build made for the MEP trades (a duct, pipe or conduit piece, a terminal, a piece
/// of equipment, a device or light it modelled), on the area's level near it.
struct BuiltPart {
    handle: String,
    kind: String,
    /// Its plan box, building feet.
    rect: [f64; 4],
    /// Its bottom and top above the floor, inches.
    z_in: Option<[f64; 2]>,
    /// The plan object it was built from.
    from: Option<String>,
}

/// The build's MEP parts on `area`'s level within `reach` feet of it.
fn built_parts(d: &Drawing, sp: &BuildingSpec, frame: &Frame, area: &Area, reach: f64) -> Vec<BuiltPart> {
    const MEP_KINDS: [&str; 7] = ["duct", "pipe", "conduit", "equipment", "fixture", "device", "light_fixture"];
    let upf = frame.upf();
    let floor = sp.level(&area.level).map_or(0.0, |l| l.elevation_ft) * upf;
    let r = grow(area.bbox, reach);
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_ENTITIES) {
        if super::wallgen::made_as(e).is_none() {
            continue;
        }
        let Some(kind) = super::elemprops::element_kind(e).filter(|k| MEP_KINDS.contains(&k.as_str())) else { continue };
        if !super::elemprops::level_of(e).is_some_and(|l| l.eq_ignore_ascii_case(&area.level)) {
            continue;
        }
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if b.is_empty() {
            continue;
        }
        let lo = frame.to_ft_on(&area.level, b.min);
        let hi = frame.to_ft_on(&area.level, b.max);
        let rect = [lo[0], lo[1], hi[0], hi[1]];
        if intersect(rect, r).is_none() && !inside(r, lo) {
            continue;
        }
        let z_in = match &e.kind {
            EntityKind::Mesh(m) if !m.vertices.is_empty() => {
                let (z0, z1) = m.vertices.iter().take(1_000_000).fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(v.z), b.max(v.z)));
                (z0.is_finite() && z1.is_finite()).then(|| [r2((z0 - floor) / upf * 12.0), r2((z1 - floor) / upf * 12.0)])
            }
            _ => None,
        };
        let kind = super::elemprops::reported_kind(e, kind);
        out.push(BuiltPart { handle: e.handle.hex(), kind, rect, z_in, from: super::bimlook::made_source(e).map(|h| h.hex()) });
        if out.len() >= MAX_OBJECTS {
            break;
        }
    }
    out.sort_by(|a, b| (a.kind.as_str(), a.handle.as_str()).cmp(&(b.kind.as_str(), b.handle.as_str())));
    out
}

/// What the packet knows of the model in the area.
struct InArea {
    objects: Value,
    kinds: BTreeSet<String>,
    marks: BTreeSet<String>,
    wall_types: BTreeSet<String>,
    submittals: BTreeSet<String>,
    counts: Value,
}

fn in_area(d: &Drawing, model: &Model, prints: Option<&Prints>, area: &Area, rules: &[Effective], built: &[BuiltPart]) -> InArea {
    let upf = super::elemprops::units_per_foot(d);
    let reach = grow(area.bbox, 1.0);
    let lvl = |l: &str| l.eq_ignore_ascii_case(&area.level);
    // The failures bim.validate finds round the area.
    let failures: Vec<v::Failure> = prints
        .map(|pr| {
            let sc = Scope { areas: vec![(area.level.clone(), [[reach[0], reach[1]], [reach[2], reach[3]]])], ..Scope::default() };
            v::validate(pr, model, &sc).failures
        })
        .unwrap_or_default();
    let fail_json = |f: &v::Failure| json!({"id": f.id, "check": serde_json::to_value(f.check).unwrap_or(Value::Null), "rule": f.rule, "expected": f.expected, "found": f.found, "severity": serde_json::to_value(f.severity).unwrap_or(Value::Null), "at": f.at.map(pt)});
    let walls: Vec<&v::ModelWall> = model.walls.iter().filter(|w| lvl(&w.level) && segment_touches(reach, w.a, w.b)).take(MAX_OBJECTS).collect();
    let centre = [(area.bbox[0] + area.bbox[2]) / 2.0, (area.bbox[1] + area.bbox[3]) / 2.0];
    let mut kinds = BTreeSet::new();
    let mut marks = BTreeSet::new();
    let mut submittals = BTreeSet::new();
    let mut elements: Vec<Value> = Vec::new();
    let mut els: Vec<&v::Element> = model.elements.iter().filter(|e| lvl(&e.level) && inside(reach, e.at)).collect();
    els.sort_by(|a, b| (a.kind.as_str(), a.id.as_str()).cmp(&(b.kind.as_str(), b.id.as_str())));
    let total_elements = els.len();
    for e in els.into_iter().take(MAX_OBJECTS) {
        kinds.insert(e.kind.clone());
        if let Some(m) = &e.mark {
            marks.insert(m.clone());
        }
        let ent = cadaddy_doc::Handle::parse_hex(&e.id).and_then(|h| d.model.get(h));
        let mut props: Vec<(String, String)> = Vec::new();
        let mut print_src: Option<Value> = None;
        if let Some(ent) = ent {
            let kind = super::elemprops::element_kind(ent).unwrap_or_else(|| "device".into());
            for (k, pr, _) in super::elemprops::effective(ent, &kind, upf).into_iter().take(200) {
                if pr.has_value() {
                    if print_src.is_none() && pr.source.kind == SourceKind::Print {
                        print_src = Some(json!({"sheet": pr.source.doc, "at": pr.source.at.map(pt)}));
                    }
                    props.push((k, pr.text()));
                }
            }
        }
        props.sort();
        let sub = field(&props, &e.fields, &["submittal"]);
        if let Some(sb) = &sub {
            submittals.insert(sb.clone());
        }
        let mounting = field(&props, &e.fields, &["mounting"])
            .unwrap_or_else(|| serde_json::to_value(e.mounting).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default());
        // The wall it is on: the nearest wall of the level within reach of its centreline.
        let host = if e.mounting == Mounting::Wall || e.mounting == Mounting::Unknown {
            model
                .walls
                .iter()
                .filter(|w| lvl(&w.level))
                .map(|w| (seg_dist(e.at, w.a, w.b), w))
                .filter(|((dist, _), w)| *dist <= HOST_REACH_FT + w.thickness_in / 24.0)
                .min_by(|a, b| a.0.0.total_cmp(&b.0.0).then(a.1.id.cmp(&b.1.id)))
                .map(|((dist, q), w)| {
                    let (nx, ny) = (e.at[0] - q[0], e.at[1] - q[1]);
                    let (nx, ny) = if nx.hypot(ny) > 1e-6 { (nx, ny) } else { (centre[0] - q[0], centre[1] - q[1]) };
                    let toward_area = nx * (centre[0] - q[0]) + ny * (centre[1] - q[1]) >= 0.0;
                    json!({"wall": w.id, "type": w.wall_type, "off_centreline_in": r2(dist * 12.0), "face": compass(nx, ny), "faces_area": toward_area})
                })
        } else {
            None
        };
        let room = if point_in(&area.outline, e.at) { area.number.clone().or_else(|| area.name.clone()) } else { None }
            .or_else(|| field(&props, &e.fields, &["room", "room_number"]));
        let mine: Vec<Value> =
            failures.iter().filter(|f| f.element.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(&e.id))).map(fail_json).collect();
        elements.push(json!({
            "handle": e.id,
            "kind": e.kind,
            "trade": trade_of_kind(&e.kind, rules),
            "mark": e.mark,
            "tag": e.tag,
            "name": field(&props, &e.fields, &["name", "description"]),
            "level": e.level,
            "room": room,
            "at_ft": pt(e.at),
            "height_aff_in": e.z_in.map(r2),
            "mounting": mounting,
            "host_wall": host,
            "size": field(&props, &e.fields, &["size", "dimensions"]),
            "system": field(&props, &e.fields, &["system"]),
            "circuit": field(&props, &e.fields, &["circuit"]),
            "switch_leg": field(&props, &e.fields, &["switch_leg", "switchleg", "switch"]),
            "manufacturer": field(&props, &e.fields, &["manufacturer", "maker"]),
            "catalog": field(&props, &e.fields, &["catalog_number", "catalogNumber", "catalog"]),
            "submittal": sub,
            "source": {"print": print_src, "note": e.source},
            "status": e.status.map(|x| x.name()),
            "failures": mine,
        }));
    }
    // What the build made for the MEP trades: on the element it was built from, else listed.
    let mut built_listed = 0usize;
    for b in built.iter().filter(|b| intersect(b.rect, reach).is_some() || inside(reach, [b.rect[0], b.rect[1]])) {
        if let Some(from) = &b.from
            && let Some(el) = elements.iter_mut().find(|x| x["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(from)))
        {
            if let Some(list) = el.get_mut("built_as").and_then(Value::as_array_mut) {
                list.push(json!(b.handle));
            } else {
                el["built_as"] = json!([b.handle]);
            }
            continue;
        }
        if built_listed >= MAX_OBJECTS {
            break;
        }
        built_listed += 1;
        kinds.insert(b.kind.clone());
        let ent = cadaddy_doc::Handle::parse_hex(&b.handle).and_then(|h| d.model.get(h));
        let mut props: Vec<(String, String)> = Vec::new();
        let mut print_src: Option<Value> = None;
        if let Some(ent) = ent {
            for (k, pr, _) in super::elemprops::effective(ent, &b.kind, upf).into_iter().take(200) {
                if pr.has_value() {
                    if print_src.is_none() && pr.source.kind == SourceKind::Print {
                        print_src = Some(json!({"sheet": pr.source.doc, "at": pr.source.at.map(pt)}));
                    }
                    props.push((k, pr.text()));
                }
            }
        }
        props.sort();
        let none = BTreeMap::new();
        let mark = field(&props, &none, &["mark", "type_mark", "type"]);
        if let Some(m) = &mark {
            marks.insert(m.clone());
        }
        let sub = field(&props, &none, &["submittal"]);
        if let Some(sb) = &sub {
            submittals.insert(sb.clone());
        }
        let c = [(b.rect[0] + b.rect[2]) / 2.0, (b.rect[1] + b.rect[3]) / 2.0];
        let mine: Vec<Value> =
            failures.iter().filter(|f| f.element.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(&b.handle))).map(fail_json).collect();
        elements.push(json!({
            "handle": b.handle,
            "kind": b.kind,
            "built": true,
            "trade": trade_of_kind(&b.kind, rules),
            "mark": mark,
            "name": field(&props, &none, &["name", "description"]),
            "level": area.level,
            "room": if point_in(&area.outline, c) { area.number.clone().or_else(|| area.name.clone()) } else { field(&props, &none, &["room"]) },
            "at_ft": pt(c),
            "plan_box_ft": rect_json(b.rect),
            "height_aff_in": b.z_in.map(|z| z[0]),
            "top_aff_in": b.z_in.map(|z| z[1]),
            "mounting": field(&props, &none, &["mounting"]),
            "size": field(&props, &none, &["size", "dimensions"]),
            "system": field(&props, &none, &["system"]),
            "circuit": field(&props, &none, &["circuit"]),
            "switch_leg": field(&props, &none, &["switch_leg", "switchleg", "switch"]),
            "manufacturer": field(&props, &none, &["manufacturer", "maker"]),
            "catalog": field(&props, &none, &["catalog_number", "catalogNumber", "catalog"]),
            "submittal": sub,
            "source": {"print": print_src, "built_from": b.from},
            "failures": mine,
        }));
    }
    let mut runs: Vec<&v::Run> =
        model.runs.iter().filter(|r| lvl(&r.level) && r.path.windows(2).any(|w| matches!(w, [a, b] if segment_touches(reach, *a, *b)))).collect();
    runs.sort_by(|a, b| (a.kind.as_str(), a.id.as_str()).cmp(&(b.kind.as_str(), b.id.as_str())));
    let total_runs = runs.len();
    let runs_json: Vec<Value> = runs
        .into_iter()
        .take(MAX_OBJECTS)
        .map(|r| {
            kinds.insert(r.kind.clone());
            if let Some(m) = &r.mark {
                marks.insert(m.clone());
            }
            let props: Vec<(String, String)> = Vec::new();
            let len: f64 = r.path.windows(2).map(|w| if let [a, b] = w { (a[0] - b[0]).hypot(a[1] - b[1]) } else { 0.0 }).sum();
            let mine: Vec<Value> =
                failures.iter().filter(|f| f.element.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(&r.id))).map(fail_json).collect();
            json!({
                "handle": r.id,
                "kind": r.kind,
                "trade": trade_of_kind(&r.kind, rules),
                "mark": r.mark,
                "level": r.level,
                "path_ft": r.path.iter().take(MAX_OUTLINE).map(|p| pt(*p)).collect::<Vec<_>>(),
                "length_ft": r2(len),
                "size": field(&props, &r.fields, &["size", "ductSize", "pipeSize", "tradeSize"]),
                "system": field(&props, &r.fields, &["system"]),
                "homerun": r.homerun,
                "source": {"note": r.source},
                "failures": mine,
            })
        })
        .collect();
    let mut wall_types = BTreeSet::new();
    let mut ws: Vec<&&v::ModelWall> = walls.iter().collect();
    ws.sort_by(|a, b| a.id.cmp(&b.id));
    let walls_json: Vec<Value> = ws
        .iter()
        .map(|w| {
            if let Some(t) = &w.wall_type {
                wall_types.insert(t.clone());
            }
            json!({"handle": w.id, "type": w.wall_type, "a_ft": pt(w.a), "b_ft": pt(w.b), "thickness_in": r2(w.thickness_in), "stud_in": w.stud_in.map(r2)})
        })
        .collect();
    if !walls_json.is_empty() {
        kinds.insert("wall".into());
    }
    let mut ops: Vec<&v::ModelOpening> =
        model.openings.iter().filter(|o| lvl(&o.level) && (inside(reach, o.min) || inside(reach, o.max))).take(MAX_OBJECTS).collect();
    ops.sort_by(|a, b| a.id.cmp(&b.id));
    let openings_json: Vec<Value> = ops
        .iter()
        .map(|o| {
            kinds.insert(o.kind.clone());
            json!({"handle": o.id, "kind": o.kind, "min_ft": pt(o.min), "max_ft": pt(o.max)})
        })
        .collect();
    let loose: Vec<Value> = failures.iter().filter(|f| f.element.is_none()).take(MAX_OBJECTS).map(fail_json).collect();
    let counts = json!({"elements": total_elements + built_listed, "runs": total_runs, "walls": walls_json.len(), "openings": openings_json.len(), "failures": failures.len()});
    let mut objects = json!({
        "area": area.id,
        "level": area.level,
        "units": "positions in building feet on the level's plan; heights in inches above the floor",
        "prints_known": prints.is_some(),
        "elements": elements,
        "runs": runs_json,
        "walls": walls_json,
        "openings": openings_json,
        "failures_without_element": loose,
        "counts": counts,
    });
    if total_elements > MAX_OBJECTS || total_runs > MAX_OBJECTS {
        objects["left_out"] = json!(format!("listed at most {MAX_OBJECTS} elements and runs each"));
    }
    InArea { objects, kinds, marks, wall_types, submittals, counts }
}

// ---------------------------------------------------------------- rules and prompt

fn rules_for<'a>(kinds: &BTreeSet<String>, all: &'a [Effective]) -> (Vec<&'a Effective>, Vec<String>) {
    let applies = |r: &Effective| kinds.iter().any(|k| r.rule.applies_to_kind(k));
    let on: Vec<&Effective> = all.iter().filter(|r| r.enabled && applies(r)).collect();
    let off: Vec<String> = all.iter().filter(|r| !r.enabled && applies(r)).map(|r| r.rule.id.clone()).collect();
    (on, off)
}

fn prompt_md(area: &Area, crops: &[Crop], model_imgs: &[ModelImage], rules: &[&Effective], counts: &Value) -> String {
    let mut s = String::new();
    let label = match (&area.number, &area.name) {
        (Some(n), Some(m)) => format!("{n} {m}"),
        (Some(n), None) => n.clone(),
        (None, Some(m)) => m.clone(),
        _ => area.id.clone(),
    };
    s.push_str(&format!("# Check the model against the prints: {} ({}, level {})\n\n", label, area.kind, area.level));
    s.push_str("You are checking a building model against its construction prints, one area at a time. ");
    s.push_str("Judge only this area. Use only the files of this packet.\n\n");
    s.push_str("## The packet\n\n");
    s.push_str(&format!(
        "- `objects.json`: what the model holds here ({} elements, {} runs, {} walls, {} openings), each with its handle, kind, mark, position (building feet), height above the floor (inches), host wall, mounting and source sheet, and the failures the deterministic checks already found.\n",
        counts.get("elements").and_then(Value::as_u64).unwrap_or(0),
        counts.get("runs").and_then(Value::as_u64).unwrap_or(0),
        counts.get("walls").and_then(Value::as_u64).unwrap_or(0),
        counts.get("openings").and_then(Value::as_u64).unwrap_or(0),
    ));
    s.push_str("- `objects.json` `prescreen`: what was measured without you (G-HOST: each wall item's distance to its wall face; G-HEIGHT: its height above the floor against the documents; G-PRESENT: the room type's expected set against what the room holds). Start from these numbers; overturn one only with a print image that shows otherwise.\n");
    s.push_str("- `rules.json`: the rules below, in full.\n");
    s.push_str("- `manifest.json`: every picture with its sheet, view, and the sheet rectangle it shows.\n");
    s.push_str("- Print crops (what the documents say):\n");
    for c in crops {
        s.push_str(&format!(
            "  - `{}`: sheet {} {}, {}{}{}\n",
            c.file,
            c.sheet,
            c.title,
            c.kind.replace('_', " "),
            c.shows.as_ref().map(|x| format!(" ({x})")).unwrap_or_default(),
            c.orientation.as_ref().and_then(turn_words).map(|w| format!("; {w}")).unwrap_or_default()
        ));
    }
    s.push_str("- Model pictures (what was built):\n");
    for m in model_imgs {
        s.push_str(&format!("  - `{}`: {}\n", m.file, m.what));
    }
    s.push_str("\nIn an overlay the print is grey and the model is coloured by trade: architecture blue, electrical red, mechanical green, plumbing cyan, fire magenta; each model element is a small square with its mark or kind.\n\n");
    s.push_str("## Rules to check\n\n");
    for e in rules {
        let r = &e.rule;
        s.push_str(&format!("### {} ({}, severity {})\n\n{}\n\n", r.id, r.trade, r.severity, r.check));
        for (i, step) in r.how.iter().enumerate() {
            s.push_str(&format!("{}. {}\n", i + 1, step));
        }
        let docs: Vec<&str> = r.documents.iter().map(String::as_str).collect();
        let images: Vec<&str> =
            crops.iter().filter(|c| docs.contains(&c.kind) || (c.kind == "rcp" && docs.contains(&"plan"))).map(|c| c.file.as_str()).collect();
        if !images.is_empty() {
            s.push_str(&format!("\nLook at: {}\n", images.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")));
        } else {
            s.push_str(&format!(
                "\nDocuments it needs: {} (none of them is in this packet: say UNSURE for what they would decide)\n",
                r.documents.join(", ")
            ));
        }
        if !r.tolerance.is_empty() {
            let t: Vec<String> = r.tolerance.iter().map(|(k, v)| format!("{k} {v}")).collect();
            s.push_str(&format!("Tolerance: {}\n", t.join(", ")));
        }
        s.push_str(&format!("Pass looks like: {}\nFail looks like: {}\n\n", r.examples.pass, r.examples.fail));
    }
    s.push_str("## What to send back\n\n");
    s.push_str("Answer with JSON only, for `validate.report`:\n\n");
    s.push_str("```json\n{\"area\": \"");
    s.push_str(&area.id);
    s.push_str("\", \"findings\": [{\"rule\": \"<rule id>\", \"handle\": \"<model handle, if one>\", \"verdict\": \"pass|fail|missing|extra|unsure\", \"issue\": \"<one sentence>\", \"expected\": \"<what the print says>\", \"found\": \"<what the model has>\", \"evidence\": [{\"image\": \"<packet file>\", \"sheet\": \"<sheet number>\", \"what\": \"<what that image shows>\"}], \"confidence\": 0.0, \"severity\": \"high|medium|low\"}]}\n```\n\n");
    s.push_str("- Every finding cites at least one print image of this packet and says what it shows there.\n");
    s.push_str("- `missing`: the print shows it, the model has nothing (no handle). `extra`: the model has it, no print shows it.\n");
    s.push_str(
        "- Say UNSURE rather than guess: when the image is unreadable, the document is not in the packet, or the print and the notes disagree.\n",
    );
    s.push_str("- Check every rule above for every element it applies to; a pass is a finding too.\n");
    s
}

// ---------------------------------------------------------------- rendering

/// A model picture of the packet.
#[derive(Clone, Debug)]
struct ModelImage {
    file: String,
    kind: &'static str,
    what: String,
    of: Option<String>,
    rect: Option<[f64; 4]>,
    px: Option<[u32; 2]>,
    wall: Option<String>,
    /// For a picture `model.render` makes: its parameters.
    render: Option<Value>,
    /// For an overlay: the crop it lies over.
    over: Option<usize>,
}

impl ModelImage {
    fn to_json(&self) -> Value {
        let mut o = json!({"file": self.file, "kind": self.kind, "what": self.what});
        if let Some(m) = o.as_object_mut() {
            if let Some(x) = &self.of {
                m.insert("of".into(), json!(x));
            }
            if let Some(r) = self.rect {
                m.insert("sheet_rect".into(), json!([r4(r[0]), r4(r[1]), r4(r[2]), r4(r[3])]));
            }
            if let Some(p) = self.px {
                m.insert("px".into(), json!(p));
            }
            if let Some(w) = &self.wall {
                m.insert("wall".into(), json!(w));
            }
        }
        o
    }
}

fn view_of(rect: [f64; 4], px: [u32; 2]) -> cadaddy_render::raster::View {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let scale = if w > 0.0 { f64::from(px[0]) / w } else { 1.0 };
    let scale = if h > 0.0 && w <= 0.0 { f64::from(px[1]) / h } else { scale };
    cadaddy_render::raster::View { center: Vec2::new((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0), scale, width: px[0], height: px[1] }
}

fn png_of(d: &Drawing, ents: &[&Entity], rect: [f64; 4], px: [u32; 2]) -> Option<Vec<u8>> {
    let list = cadaddy_render::build_entities(d, ents.iter().copied(), &cadaddy_render::Options::default());
    let ro = cadaddy_render::raster::RasterOptions { background: cadaddy_color::Rgb(255, 255, 255), hairline: 1.2, px_per_mm: 0.0, antialias: true };
    cadaddy_render::raster::render_png(&list, &view_of(rect, px), &ro)
}

/// The prints' objects once, with their boxes, for the crops.
struct PrintIndex<'a>(Vec<(Bounds2, &'a Entity)>);

impl<'a> PrintIndex<'a> {
    fn of(d: &'a Drawing, within: &[[f64; 4]]) -> PrintIndex<'a> {
        let boxes: Vec<Bounds2> = within.iter().map(|r| Bounds2::new(Vec2::new(r[0], r[1]), Vec2::new(r[2], r[3]))).collect();
        let mut out = Vec::new();
        for e in d.model.iter().take(MAX_ENTITIES) {
            if is_model_object(e) {
                continue;
            }
            let b = cadaddy_doc::entity_bounds(d, e, 0);
            if b.is_empty() || !boxes.iter().any(|x| x.intersects(&b)) {
                continue;
            }
            out.push((b, e.as_ref()));
        }
        PrintIndex(out)
    }
    fn within(&self, r: [f64; 4]) -> Vec<&'a Entity> {
        let b = Bounds2::new(Vec2::new(r[0], r[1]), Vec2::new(r[2], r[3]));
        self.0.iter().filter(|(eb, _)| eb.intersects(&b)).map(|(_, e)| *e).take(MAX_CROP_ENTITIES).collect()
    }
}

/// The model drawn onto a sheet (`place`: project → sheet), by trade colour; with `print`, the
/// print's objects under it in grey.
#[allow(clippy::too_many_arguments)]
fn model_drawing(
    d: &Drawing,
    print: &[&Entity],
    model: &Model,
    area: &Area,
    frame: &Frame,
    place: &Place,
    rules: &[Effective],
    built: &[BuiltPart],
    upf: f64,
) -> Drawing {
    let mut od = Drawing::new_imperial();
    od.blocks = d.blocks.clone();
    for e in print {
        let mut c = (*e).clone();
        c.common.color = Color::Index(8);
        c.common.layer = "0".into();
        let _ = od.add(&Space::Model, c.common, c.kind);
    }
    let to = |q: [f64; 2]| -> Option<Vec3> {
        let p = frame.to_sheet(&area.level, q);
        place.to_sheet([p.x, p.y]).map(|s| Vec3::new(s[0], s[1], 0.0))
    };
    let sc = place.scale();
    let put = |od: &mut Drawing, kind: EntityKind, color: u8| {
        let _ = od.add(&Space::Model, Common { color: Color::Index(color), ..Common::default() }, kind);
    };
    let line = |a: Vec3, b: Vec3| EntityKind::Line(cadaddy_doc::Line { a, b });
    let lvl = |l: &str| l.eq_ignore_ascii_case(&area.level);
    let reach = grow(area.bbox, 30.0);
    // The area's outline.
    for w in area.outline.iter().zip(area.outline.iter().cycle().skip(1)).take(MAX_OUTLINE) {
        if let (Some(a), Some(b)) = (to(*w.0), to(*w.1)) {
            put(&mut od, line(a, b), 200);
        }
    }
    for wl in model.walls.iter().filter(|w| lvl(&w.level) && segment_touches(reach, w.a, w.b)).take(MAX_OBJECTS) {
        let (dx, dy) = (wl.b[0] - wl.a[0], wl.b[1] - wl.a[1]);
        let l = dx.hypot(dy);
        let half = (wl.thickness_in / 24.0).max(0.0);
        if l < 1e-9 {
            continue;
        }
        let (nx, ny) = (-dy / l * half, dx / l * half);
        for s in [-1.0, 1.0] {
            if let (Some(a), Some(b)) = (to([wl.a[0] + nx * s, wl.a[1] + ny * s]), to([wl.b[0] + nx * s, wl.b[1] + ny * s])) {
                put(&mut od, line(a, b), 5);
            }
            if half <= 0.0 {
                break;
            }
        }
    }
    for o in model.openings.iter().filter(|o| lvl(&o.level) && (inside(reach, o.min) || inside(reach, o.max))).take(MAX_OBJECTS) {
        let c = corners([o.min[0], o.min[1], o.max[0], o.max[1]]);
        for w in c.iter().zip(c.iter().cycle().skip(1)) {
            if let (Some(a), Some(b)) = (to(*w.0), to(*w.1)) {
                put(&mut od, line(a, b), 5);
            }
        }
    }
    for r in model.runs.iter().filter(|r| lvl(&r.level) && r.path.iter().any(|q| inside(reach, *q))).take(MAX_OBJECTS) {
        let col = trade_colour(trade_of_kind(&r.kind, rules));
        for w in r.path.windows(2).take(10_000) {
            if let [a, b] = w
                && let (Some(a), Some(b)) = (to(*a), to(*b))
            {
                put(&mut od, line(a, b), col);
            }
        }
    }
    // The build's MEP parts: their plan boxes.
    for b in built {
        let col = trade_colour(trade_of_kind(&b.kind, rules));
        let c = corners(b.rect);
        for w in c.iter().zip(c.iter().cycle().skip(1)) {
            if let (Some(a), Some(z)) = (to(*w.0), to(*w.1)) {
                put(&mut od, line(a, z), col);
            }
        }
    }
    let half = 0.3 * upf * sc;
    let text_h = 0.35 * upf * sc;
    for e in model.elements.iter().filter(|e| lvl(&e.level) && inside(reach, e.at)).take(MAX_OBJECTS) {
        let Some(c) = to(e.at) else { continue };
        let col = trade_colour(trade_of_kind(&e.kind, rules));
        let vertices = [(-half, -half), (half, -half), (half, half), (-half, half)]
            .iter()
            .map(|(dx, dy)| cadaddy_geom::PolyVertex { p: Vec2::new(c.x + dx, c.y + dy), bulge: 0.0, start_width: 0.0, end_width: 0.0 })
            .collect();
        put(
            &mut od,
            EntityKind::LwPolyline(cadaddy_doc::LwPolyline { vertices, closed: true, const_width: 0.0, elevation: 0.0, plinegen: false }),
            col,
        );
        let label: String = e.mark.clone().or_else(|| e.tag.clone()).unwrap_or_else(|| e.kind.clone()).chars().take(24).collect();
        put(
            &mut od,
            EntityKind::Text(cadaddy_doc::Text {
                insert: Vec3::new(c.x + half * 1.4, c.y + half * 1.4, 0.0),
                align_pt: None,
                height: text_h,
                value: label,
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: Default::default(),
                valign: Default::default(),
            }),
            col,
        );
    }
    od
}

fn render_drawing(od: &Drawing, rect: [f64; 4], px: [u32; 2]) -> Option<Vec<u8>> {
    let all: Vec<&Entity> = od.model.iter().map(|e| e.as_ref()).collect();
    png_of(od, &all, rect, px)
}

// ---------------------------------------------------------------- planning the crops

/// Whether a link shows `level`.
pub(crate) fn shows_level(l: &SheetLink, level: &str) -> bool {
    l.level.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(level)) || l.levels.iter().any(|x| x.eq_ignore_ascii_case(level))
}

struct Planned {
    crops: Vec<Crop>,
    left_out: Vec<(String, String)>,
}

#[allow(clippy::too_many_arguments)]
fn plan_crops(
    sp: &BuildingSpec,
    frame: &Frame,
    area: &Area,
    margin: f64,
    dpi: f64,
    trades: &[String],
    texts: &Texts,
    keys: &InArea,
    unit: Option<&UnitOf>,
    fit: Option<&Value>,
) -> Planned {
    let mut crops: Vec<Crop> = Vec::new();
    let mut left_out: Vec<(String, String)> = Vec::new();
    let want = |t: &str| trades.is_empty() || t == "architecture" || t == "general" || trades.iter().any(|x| x == t);
    // The area's box on the reference plan.
    let ft_box = grow(area.bbox, margin);
    let proj: Vec<[f64; 2]> = corners(ft_box)
        .iter()
        .map(|q| {
            let p = frame.to_sheet(&area.level, *q);
            [p.x, p.y]
        })
        .collect();
    let Some(proj_box) = bbox(&proj) else {
        return Planned { crops, left_out };
    };
    let mut links: Vec<&SheetLink> = sp.sheets.iter().filter(|l| l.rect.is_some_and(|r| r.iter().all(|v| v.is_finite()))).collect();
    links.sort_by(|a, b| a.number.cmp(&b.number).then(a.view.as_ref().map(|v| v.title.as_str()).cmp(&b.view.as_ref().map(|v| v.title.as_str()))));
    let mut n = 0usize;
    let mut push = |crops: &mut Vec<Crop>,
                    l: &SheetLink,
                    kind: &'static str,
                    trade: &'static str,
                    rect: [f64; 4],
                    placed_by: &'static str,
                    place: Option<Place>,
                    shows: Option<String>,
                    orientation: Option<Value>| {
        let sheet_rect = l.rect.map(norm_rect).unwrap_or(rect);
        let (paper_in, paper) = paper_of(sheet_rect);
        let long = (sheet_rect[2] - sheet_rect[0]).max(sheet_rect[3] - sheet_rect[1]);
        if long.is_nan() || long <= 0.0 {
            return;
        }
        let ppu = dpi * paper_in / long;
        let (ppu, parts) = tiles(rect, ppu);
        let got_dpi = ppu * long / paper_in;
        n += 1;
        let many = parts.len() > 1;
        for (r, px, t) in parts {
            let file = if many {
                format!("prints/{n:02}_{}_{}_t{}.png", safe(&l.number), kind, t[0])
            } else {
                format!("prints/{n:02}_{}_{}.png", safe(&l.number), kind)
            };
            crops.push(Crop {
                file,
                sheet: l.number.clone(),
                title: l.title.clone(),
                kind,
                trade,
                view: l.view.as_ref().map(|v| v.title.clone()),
                rect: r,
                ppu,
                px,
                dpi: got_dpi,
                paper,
                tile: many.then_some(t),
                placed_by,
                place: place.clone(),
                shows: shows.clone(),
                orientation: orientation.clone(),
            });
        }
    };
    // The sheets of the room's unit type's own views (placed by the type, in 1b).
    let unit_sheets: BTreeSet<String> = unit
        .and_then(|u| u.ty.get("views"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_TYPE_VIEWS)
        .filter_map(|v| v.get("sheet").and_then(Value::as_str).map(str::to_ascii_uppercase))
        .collect();
    // 1. The plans, enlarged plans and RCPs of the area's level.
    for l in links.iter().filter(|l| shows_level(l, &area.level)) {
        let title = format!("{} {}", l.title, l.view.as_ref().map_or("", |v| v.title.as_str()));
        let kind = sheet_kind(&title, true);
        if !matches!(kind, "plan" | "rcp" | "enlarged_plan") {
            continue;
        }
        let trade = trade_of_discipline(&l.discipline.clone().unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(&l.number)));
        if !want(trade) {
            continue;
        }
        let Some(sheet_rect) = l.rect.map(norm_rect) else { continue };
        let clip = l.view.as_ref().map_or(sheet_rect, |v| norm_rect(v.rect));
        let (place, by) = match &l.registration {
            Some(r) if r.method.eq_ignore_ascii_case("reference") => (Place::Same, "reference"),
            Some(r) if l.view.is_some() => (Place::Fit(r.clone()), "view_placement"),
            Some(r) => (Place::Fit(r.clone()), "registration"),
            None if intersect(proj_box, clip).is_some() => (Place::Same, "same_sheet"),
            None if unit_sheets.contains(&l.number.to_ascii_uppercase()) => continue,
            None => {
                left_out.push((
                    format!("sheet {} {}", l.number, l.title),
                    "it shows this level but is not placed on its plan yet (bim.identity registers it)".into(),
                ));
                continue;
            }
        };
        let on_sheet: Vec<[f64; 2]> = corners(proj_box).iter().filter_map(|q| place.to_sheet(*q)).collect();
        let Some(b) = bbox(&on_sheet).filter(|_| on_sheet.len() == 4) else { continue };
        let Some(r) = intersect(b, clip) else { continue };
        let orientation = match &place {
            Place::Fit(reg) if kind == "enlarged_plan" || turn_deg(reg.b.atan2(reg.a)).abs() > 0.05 => Some(orientation_of(reg, by)),
            _ => None,
        };
        push(&mut crops, l, kind, trade, r, by, Some(place), None, orientation);
    }
    // 1b. The enlarged views of the room's unit type (every trade's: the typical room's
    //     mechanical, plumbing and power plans), turned and mirrored as the room is.
    if let Some(u) = unit {
        let source = u.ty.get("source").and_then(|x| x.get("sheet")).and_then(Value::as_str).unwrap_or_default().to_string();
        let names: Vec<&str> = [u.id.as_str(), u.name.as_str()].into_iter().filter(|n| n.trim().len() >= 2).collect();
        let names_it = |t: &str| {
            let t = format!(" {} ", upper_words(&t.replace(['.', '-', '/', '_', ','], " ")));
            names.iter().any(|n| t.contains(&format!(" {} ", upper_words(&n.replace(['.', '-', '/', '_', ','], " ")))))
        };
        let mut n_unit = 0usize;
        // The type's own views (the job lists them: sheet, title, where), each into the packet
        // whatever its kind (plans, RCP, elevations), turned and mirrored as the room is.
        let rect4 = |v: &Value| -> Option<[f64; 4]> {
            let r = v.as_array()?;
            let n = |i: usize| r.get(i).and_then(Value::as_f64).filter(|x| x.is_finite());
            Some([n(0)?, n(1)?, n(2)?, n(3)?])
        };
        let listed: Vec<(String, String, [f64; 4], Option<[f64; 4]>)> =
            u.ty.get("views")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(MAX_TYPE_VIEWS)
                // A view of one floor's guest rooms (its `levels`): only for a room on that floor.
                .filter(|v| {
                    let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
                    let levels: Vec<String> =
                        v.get("levels").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(digits).collect();
                    levels.is_empty() || levels.iter().any(|l| !l.is_empty() && *l == digits(&area.level))
                })
                .filter_map(|v| {
                    Some((
                        v.get("sheet")?.as_str()?.to_string(),
                        v.get("title")?.as_str()?.to_string(),
                        rect4(v.get("rect")?)?,
                        v.get("sheet_rect").and_then(rect4),
                    ))
                })
                .collect();
        for (sheet, title, vr, sheet_rect) in &listed {
            // Its sheet as the set links it, else (a unit sheet of no level: A821) as the job
            // read it.
            let found = links
                .iter()
                .find(|l| l.number.eq_ignore_ascii_case(sheet) && l.rect.map(norm_rect).is_some_and(|sr| intersect(norm_rect(*vr), sr).is_some()))
                .copied();
            let made;
            let l: &SheetLink = match (found, sheet_rect) {
                (Some(l), _) => l,
                (None, Some(sr)) => {
                    made = SheetLink {
                        number: sheet.clone(),
                        title: String::new(),
                        discipline: Some(cadaddy_mepfix::sheet_facts::discipline_of(sheet)),
                        level: None,
                        source: "unit type view".into(),
                        rect: Some(*sr),
                        levels: Vec::new(),
                        registration: None,
                        view: None,
                    };
                    &made
                }
                (None, None) => continue,
            };
            let trade = trade_of_discipline(&l.discipline.clone().unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(&l.number)));
            if !want(trade) {
                continue;
            }
            let Some(rect) = l.rect.map(norm_rect).and_then(|sr| intersect(norm_rect(*vr), sr)) else { continue };
            let up = title.to_ascii_uppercase();
            let kind = if up.contains(" WALL") || up.contains("ELEVATION") { "elevation" } else { "enlarged_plan" };
            let unit_fit = Some(&u.fit).filter(|f| f.is_object());
            let mut o = match (fit, unit_fit) {
                (Some(f), _) => {
                    json!({"rotation_deg": f["rotation_deg"], "mirrored": f["mirrored"], "by": "layout_fit", "fit_ft": f["fit_ft"], "matched": f["matched"]})
                }
                (None, Some(f)) => json!({"rotation_deg": f["rotation_deg"], "mirrored": f["mirrored"], "by": f["by"]}),
                (None, None) => json!({"rotation_deg": null, "mirrored": u.hand == "mirrored", "by": "unit hand"}),
            };
            o["hand"] = json!(u.hand);
            o["unit_type"] = json!(u.id);
            if let Some(w) = turn_words(&o) {
                o["read"] = json!(w);
            }
            push(&mut crops, l, kind, trade, rect, "unit_type", None, Some(format!("unit type {}: {title}", u.name)), Some(o));
        }
        for l in links.iter().filter(|l| !listed.iter().any(|(sh, ..)| l.number.eq_ignore_ascii_case(sh))) {
            if n_unit >= MAX_UNIT_VIEWS {
                break;
            }
            let title = format!("{} {}", l.title, l.view.as_ref().map_or("", |v| v.title.as_str()));
            if sheet_kind(&title, true) != "enlarged_plan" || !(names_it(&title) || (!source.is_empty() && l.number.eq_ignore_ascii_case(&source))) {
                continue;
            }
            let trade = trade_of_discipline(&l.discipline.clone().unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(&l.number)));
            if !want(trade) {
                continue;
            }
            let view_title = l.view.as_ref().map(|v| v.title.clone());
            if crops.iter().any(|c| c.sheet == l.number && c.view == view_title) {
                continue;
            }
            let Some(sr) = l.rect.map(norm_rect) else { continue };
            let rect = match &l.view {
                Some(vw) => intersect(norm_rect(vw.rect), sr),
                None => names
                    .iter()
                    .find_map(|n| texts.find(sr, n, true).into_iter().next())
                    .map(|(at, _)| {
                        let (w, h) = (sr[2] - sr[0], sr[3] - sr[1]);
                        [at[0] - 0.15 * w, at[1] - 0.04 * h, at[0] + 0.15 * w, at[1] + 0.25 * h]
                    })
                    .and_then(|r| intersect(r, sr)),
            };
            let Some(rect) = rect else { continue };
            let mut o = match (fit, &l.registration) {
                (Some(f), _) => {
                    json!({"rotation_deg": f["rotation_deg"], "mirrored": f["mirrored"], "by": "layout_fit", "fit_ft": f["fit_ft"], "matched": f["matched"]})
                }
                (None, Some(reg)) => {
                    let mut o = orientation_of(reg, "view_placement");
                    o["mirrored"] = json!(u.hand == "mirrored");
                    o["by"] = json!(if u.hand == "mirrored" { "view_placement + unit hand" } else { "view_placement" });
                    o
                }
                (None, None) => json!({"rotation_deg": null, "mirrored": u.hand == "mirrored", "by": "unit hand"}),
            };
            o["hand"] = json!(u.hand);
            o["unit_type"] = json!(u.id);
            if let Some(w) = turn_words(&o) {
                o["read"] = json!(w);
            }
            n_unit += 1;
            push(&mut crops, l, "enlarged_plan", trade, rect, "unit_type", None, Some(format!("unit type {}", u.name)), Some(o));
        }
    }
    // 2. Interior elevations naming the room, the partition legend, the schedule and legend rows
    //    of the marks present, general notes.
    let mut room_keys: Vec<String> = [area.number.clone(), area.name.clone().filter(|n| n.trim().len() >= 3)].into_iter().flatten().collect();
    // A typical room's interior elevations are titled by its unit type.
    if let Some(u) = unit {
        for k in [u.name.clone(), u.id.clone()] {
            if k.trim().len() >= 3 && !room_keys.contains(&k) {
                room_keys.push(k);
            }
        }
    }
    let plan_sheets: BTreeSet<String> = crops.iter().map(|c| c.sheet.clone()).collect();
    for l in links.iter().filter(|l| l.view.is_none()) {
        let Some(sr) = l.rect.map(norm_rect) else { continue };
        let kind = sheet_kind(&l.title, l.level.is_some() || !l.levels.is_empty());
        let trade = trade_of_discipline(&l.discipline.clone().unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(&l.number)));
        if !want(trade) {
            continue;
        }
        let (w, h) = (sr[2] - sr[0], sr[3] - sr[1]);
        let mut found: Vec<[f64; 4]> = Vec::new();
        let mut add = |crops: &mut Vec<Crop>, kind: &'static str, r: [f64; 4], shows: String| {
            if found.len() >= MAX_TEXT_CROPS_PER_SHEET {
                return;
            }
            // One text can name the room twice (its number and its name): one crop.
            if let Some(r) = intersect(r, sr).filter(|r| !found.iter().any(|f| f.iter().zip(r).all(|(a, b)| (a - b).abs() < 1e-6))) {
                found.push(r);
                push(crops, l, kind, trade, r, "text", None, Some(shows), None);
            }
        };
        match kind {
            "interior_elevation" => {
                for k in &room_keys {
                    for (at, _) in texts.find(sr, k, true).into_iter().take(2) {
                        // The view stands over its title.
                        add(&mut crops, kind, [at[0] - 0.15 * w, at[1] - 0.04 * h, at[0] + 0.15 * w, at[1] + 0.25 * h], format!("room {k}"));
                    }
                }
            }
            "partition_legend" => {
                for t in &keys.wall_types {
                    for (at, _) in texts.find(sr, t, false).into_iter().take(1) {
                        add(&mut crops, kind, [at[0] - 0.1 * w, at[1] - 0.1 * h, at[0] + 0.1 * w, at[1] + 0.1 * h], format!("wall type {t}"));
                    }
                }
            }
            "schedule" | "legend" => {
                for m in &keys.marks {
                    for (at, th) in texts.find(sr, m, false).into_iter().take(1) {
                        let band = (th * 3.0).max(0.01 * h);
                        add(&mut crops, kind, [at[0] - 0.05 * w, at[1] - band, at[0] + 0.45 * w, at[1] + band * 2.0], format!("mark {m}"));
                    }
                }
            }
            _ => {}
        }
        // General notes: on a notes sheet, and on the plans in the packet.
        if kind == "general_notes" || plan_sheets.contains(&l.number) {
            for (at, _) in texts.find(sr, "GENERAL NOTES", true).into_iter().take(1) {
                add(&mut crops, "general_notes", [at[0] - 0.02 * w, at[1] - 0.4 * h, at[0] + 0.33 * w, at[1] + 0.03 * h], "GENERAL NOTES".into());
            }
        }
    }
    // 3. Riser diagrams: a riser view, else round the text naming the room's unit type or number,
    //    else round the diagram's title (a reading aid, a few at most).
    let mut risers = 0usize;
    let mut riser_keys: Vec<String> = unit.map(|u| vec![u.name.clone(), u.id.clone()]).unwrap_or_default();
    riser_keys.extend(area.number.clone());
    riser_keys.retain(|k| k.trim().len() >= 2);
    for l in &links {
        if risers >= MAX_RISERS {
            break;
        }
        let title = format!("{} {}", l.title, l.view.as_ref().map_or("", |v| v.title.as_str()));
        if sheet_kind(&title, false) != "riser_diagram" {
            continue;
        }
        let trade = trade_of_discipline(&l.discipline.clone().unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(&l.number)));
        if !want(trade) {
            continue;
        }
        let Some(sr) = l.rect.map(norm_rect) else { continue };
        let (w, h) = (sr[2] - sr[0], sr[3] - sr[1]);
        let (rect, shows) = match &l.view {
            Some(vw) => (intersect(norm_rect(vw.rect), sr), "riser diagram".to_string()),
            None => match riser_keys.iter().find_map(|k| texts.find(sr, k, true).into_iter().next().map(|hit| (hit, k.clone()))) {
                Some(((at, _), k)) => {
                    (intersect([at[0] - 0.2 * w, at[1] - 0.2 * h, at[0] + 0.2 * w, at[1] + 0.2 * h], sr), format!("riser diagram ({k})"))
                }
                None => (
                    texts
                        .find(sr, "RISER", true)
                        .into_iter()
                        .next()
                        .and_then(|(at, _)| intersect([at[0] - 0.2 * w, at[1] - 0.02 * h, at[0] + 0.2 * w, at[1] + 0.4 * h], sr)),
                    "riser diagram".to_string(),
                ),
            },
        };
        let Some(rect) = rect else { continue };
        if crops.iter().any(|c| c.sheet == l.number && c.rect.iter().zip(rect).all(|(a, b)| (a - b).abs() < 1e-6)) {
            continue;
        }
        risers += 1;
        push(&mut crops, l, "riser_diagram", trade, rect, "text", None, Some(shows), None);
    }
    Planned { crops, left_out }
}

// ---------------------------------------------------------------- submittal pages

/// A submittal's file and page from its `name p.N` reference.
fn submittal_ref(s: &str) -> (String, u32) {
    let t = s.trim();
    if let Some((name, page)) = t.rsplit_once(" p.")
        && let Ok(n) = page.trim().parse::<u32>()
    {
        return (name.trim().to_string(), n.max(1));
    }
    (t.to_string(), 1)
}

/// Where a submittal file named `name` is, among the job's submittal paths (files or folders).
fn find_submittal(paths: &[String], name: &str) -> Option<std::path::PathBuf> {
    let want = name.trim().to_ascii_lowercase();
    // Its file name, or the name without its extension (the elements carry the submittal's stem:
    // `Submittal #265119-1 Lighting Package`).
    let is = |f: &std::path::Path| {
        f.file_name().is_some_and(|x| x.to_string_lossy().to_ascii_lowercase() == want)
            || f.file_stem().is_some_and(|x| x.to_string_lossy().to_ascii_lowercase() == want)
    };
    for p in paths.iter().take(MAX_FOLDER_FILES) {
        let path = std::path::Path::new(p);
        if path.is_file() {
            if is(path) {
                return Some(path.to_path_buf());
            }
        } else if let Ok(rd) = std::fs::read_dir(path) {
            let mut hits: Vec<std::path::PathBuf> = rd.take(MAX_FOLDER_FILES).filter_map(|e| e.ok()).map(|e| e.path()).filter(|f| is(f)).collect();
            hits.sort();
            if let Some(h) = hits.into_iter().next() {
                return Some(h);
            }
        }
    }
    None
}

fn submittal_png(path: &std::path::Path, page: u32) -> std::result::Result<Vec<u8>, String> {
    let hook = super::file::io().and_then(|h| h.read_pdf).ok_or_else(|| "PDF pages can't be read in this build".to_string())?;
    let bytes = crate::readfile::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 200 * 1024 * 1024 {
        return Err("the file is over 200 MB".into());
    }
    let (pd, _) = hook(&bytes, &json!({"page": page}))?;
    let ext = pd.extents(&Space::Model);
    if ext.is_empty() {
        return Err("the page is empty".into());
    }
    let (w, h) = (ext.width().max(1e-9), ext.height().max(1e-9));
    let s = f64::from(SUBMITTAL_PX) / w.max(h);
    let px = [((w * s).ceil() as u32).clamp(16, SUBMITTAL_PX), ((h * s).ceil() as u32).clamp(16, SUBMITTAL_PX)];
    render_drawing(&pd, [ext.min.x, ext.min.y, ext.max.x, ext.max.y], px).ok_or_else(|| "the page couldn't be drawn".into())
}

// ---------------------------------------------------------------- the packet

fn run_packet(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = CMD_PACKET;
    let start = web_time::Instant::now();
    let out_dir = str_param(p, "out_dir")
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .ok_or_else(|| bad(CMD, "`out_dir` (the folder to write the packet into) is required"))?
        .to_string();
    let trades: Vec<String> = match p.get("trades") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .take(16)
            .map(|t| {
                t.as_str()
                    .map(|x| x.trim().to_ascii_lowercase())
                    .filter(|x| vr::TRADES.contains(&x.as_str()))
                    .ok_or_else(|| bad(CMD, format!("unknown trade {t} ({})", vr::TRADES.join(", "))))
            })
            .collect::<Result<Vec<_>>>()?,
        Some(_) => return Err(bad(CMD, "`trades` is a list of trades")),
    };
    let num = |key: &str, lo: f64, hi: f64, def: f64| -> Result<f64> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(def),
            Some(v) => {
                v.as_f64().filter(|x| x.is_finite() && *x >= lo && *x <= hi).ok_or_else(|| bad(CMD, format!("`{key}` is a number from {lo} to {hi}")))
            }
        }
    };
    let margin = num("margin_ft", 0.0, 50.0, DEFAULT_MARGIN_FT)?;
    let dpi = num("dpi", DEFAULT_DPI, MAX_DPI, DEFAULT_DPI)?;
    let max_images = num("max_images", 1.0, MAX_IMAGES as f64, DEFAULT_IMAGES as f64)?.floor() as usize;
    let doc: Arc<Drawing> = Arc::clone(&s.state()?.doc);
    let d: &Drawing = &doc;
    let (sp, _) = current(s, CMD)?;
    let (model, frame) = model_of(d, &sp);
    let area = area_param(CMD, &sp, p, d, &frame)?;
    let prints: Option<Prints> = prints_of(&sp).cloned().or_else(|| prints_from_job(s, &frame));
    let job_submittals: Vec<String> = super::guided::load_job(s).ok().flatten().map(|j| j.submittals).unwrap_or_default();
    let (all_rules, rejected) = vr::effective(&d.project_rules).map_err(|e| bad(CMD, e))?;
    let only_rules: Option<BTreeSet<String>> = match p.get("rules") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => Some(
            a.iter()
                .take(100)
                .map(|r| {
                    r.as_str()
                        .map(|x| x.trim().to_ascii_uppercase())
                        .filter(|x| all_rules.iter().any(|e| e.rule.id == *x))
                        .ok_or_else(|| bad(CMD, format!("unknown rule {r} (validate.rules lists them)")))
                })
                .collect::<Result<BTreeSet<_>>>()?,
        ),
        Some(_) => return Err(bad(CMD, "`rules` is a list of rule ids (validate.rules)")),
    };
    let upf = frame.upf();

    // What the model holds here, and the rules that apply.
    let built = built_parts(d, &sp, &frame, &area, 30.0);
    let mut here = in_area(d, &model, prints.as_ref(), &area, &all_rules, &built);
    // The deterministic prescreen of the area: what was measured, for the judge to start from.
    let pre = ps::screen(&ps::Scope {
        d,
        sp: &sp,
        model: &model,
        frame: &frame,
        prints: prints.as_ref(),
        rules: &all_rules,
        areas: std::slice::from_ref(&area),
        whole_levels: false,
        only: &[],
    });
    here.objects["prescreen"] = json!({
        "about": "measured without an agent: G-HOST the distance to the nearest wall face (inches, negative inside the wall), G-HEIGHT the height above the floor against the documents, G-PRESENT the room type's expected set (project rules)",
        "checked": pre.checked,
        "findings": pre.findings.iter().take(MAX_OBJECTS).collect::<Vec<_>>(),
    });
    // The room's unit type, and how its typical layout lies in the room.
    let unit = ps::unit_of(d, &area.level, area.number.as_deref());
    let placed: Vec<(String, [f64; 2])> = here.objects["elements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let at = e.get("at_ft")?.as_array()?;
            Some((e.get("kind")?.as_str()?.to_string(), [at.first()?.as_f64()?, at.get(1)?.as_f64()?]))
        })
        .collect();
    let fit = unit.as_ref().and_then(|u| layout_fit(&u.ty, &area, &placed));
    if let Some(u) = &unit {
        let mut j = json!({"type": u.id, "name": u.name, "hand": u.hand});
        if let Some(f) = &fit {
            j["layout_fit"] = f.clone();
        }
        here.objects["unit"] = j;
    }
    let (mut rules_on, mut rules_off) = rules_for(&here.kinds, &all_rules);
    // Only the rules a validate run asked for (`rules`): the others are listed as left out.
    if let Some(only) = &only_rules {
        rules_off.extend(rules_on.iter().filter(|r| !only.contains(&r.rule.id)).map(|r| r.rule.id.clone()));
        rules_on.retain(|r| only.contains(&r.rule.id));
    }
    let rules_json = json!({
        "area": area.id,
        "kinds": here.kinds.iter().collect::<Vec<_>>(),
        "rules": rules_on.iter().map(|r| r.to_json(true)).collect::<Vec<_>>(),
        "disabled": rules_off,
        "rejected_overrides": rejected.iter().map(|r| json!({"key": r.key, "value": r.value, "why": r.why})).collect::<Vec<_>>(),
    });

    // The print crops.
    let within: Vec<[f64; 4]> = sp.sheets.iter().filter_map(|l| l.rect.map(norm_rect)).collect();
    let texts = Texts::of(d, &within);
    let Planned { crops, mut left_out } = plan_crops(&sp, &frame, &area, margin, dpi, &trades, &texts, &here, unit.as_ref(), fit.as_ref());
    if sp.sheets.is_empty() {
        left_out.push(("print crops".into(), "the building spec lists no sheets (levels.sheets finds them)".into()));
    }

    // The model pictures: over each area crop, the plan alone, 3D and wall elevations.
    let mut model_imgs: Vec<ModelImage> = Vec::new();
    for (i, c) in crops.iter().enumerate().filter(|(_, c)| c.place.is_some()) {
        model_imgs.push(ModelImage {
            file: c.file.replacen("prints/", "model/", 1).replacen(".png", "_overlay.png", 1),
            kind: "overlay",
            what: format!("the model over {} (sheet {})", c.file, c.sheet),
            of: Some(c.file.clone()),
            rect: Some(c.rect),
            px: Some(c.px),
            wall: None,
            render: None,
            over: Some(i),
        });
    }
    let proj_box = {
        let pts: Vec<[f64; 2]> = corners(grow(area.bbox, margin))
            .iter()
            .map(|q| {
                let v = frame.to_sheet(&area.level, *q);
                [v.x, v.y]
            })
            .collect();
        bbox(&pts).unwrap_or([0.0, 0.0, 1.0, 1.0])
    };
    {
        let px_per_unit = (24.0 / upf).min(f64::from(TILE_PX) / (proj_box[2] - proj_box[0]).max(proj_box[3] - proj_box[1]).max(1e-9));
        let px = [
            (((proj_box[2] - proj_box[0]) * px_per_unit).ceil() as u32).clamp(16, TILE_PX),
            (((proj_box[3] - proj_box[1]) * px_per_unit).ceil() as u32).clamp(16, TILE_PX),
        ];
        let rect = [proj_box[0], proj_box[3] - f64::from(px[1]) / px_per_unit, proj_box[0] + f64::from(px[0]) / px_per_unit, proj_box[3]];
        model_imgs.push(ModelImage {
            file: "model/plan.png".into(),
            kind: "plan",
            what: "the model alone on the plan (project coordinates)".into(),
            of: None,
            rect: Some(rect),
            px: Some(px),
            wall: None,
            render: None,
            over: None,
        });
    }
    let is_built = d.model.iter().take(MAX_ENTITIES).any(|e| super::wallgen::made_as(e).is_some_and(|m| m == "wall"));
    let level_spec = sp.level(&area.level);
    if !is_built {
        left_out.push(("3D view and wall elevations".into(), "the model is not built yet (model.build)".into()));
    } else {
        let around = json!([r2(proj_box[0]), r2(proj_box[1]), r2(proj_box[2]), r2(proj_box[3])]);
        model_imgs.push(ModelImage {
            file: "model/3d.png".into(),
            kind: "3d",
            what: "the area in 3D (level's model inside the box, from the south-west, above)".into(),
            of: None,
            rect: None,
            px: Some([1200, 900]),
            wall: None,
            render: Some(json!({"width": 1200, "height": 900, "level": area.level, "around": around})),
            over: None,
        });
        let z = level_spec.map_or(0.0, |l| l.elevation_ft) * upf + 5.0 * upf;
        let centre = [(area.bbox[0] + area.bbox[2]) / 2.0, (area.bbox[1] + area.bbox[3]) / 2.0];
        let mut walls: Vec<&v::ModelWall> = model
            .walls
            .iter()
            .filter(|w| {
                w.level.eq_ignore_ascii_case(&area.level)
                    && segment_touches(grow(area.bbox, 1.0), w.a, w.b)
                    && (w.a[0] - w.b[0]).hypot(w.a[1] - w.b[1]) >= 2.0
            })
            .collect();
        // Longest along the area first.
        let along = |w: &v::ModelWall| {
            let near = grow(area.bbox, 1.0);
            let n = (0..=64)
                .filter(|i| inside(near, [w.a[0] + (w.b[0] - w.a[0]) * f64::from(*i) / 64.0, w.a[1] + (w.b[1] - w.a[1]) * f64::from(*i) / 64.0]))
                .count();
            (w.a[0] - w.b[0]).hypot(w.a[1] - w.b[1]) * n as f64 / 65.0
        };
        walls.sort_by(|a, b| along(b).total_cmp(&along(a)).then(a.id.cmp(&b.id)));
        for (k, w) in walls.into_iter().take(MAX_ELEVATIONS).enumerate() {
            let (dx, dy) = (w.b[0] - w.a[0], w.b[1] - w.a[1]);
            // The part of the wall along the area: its middle is what the camera looks at.
            let near = grow(area.bbox, 1.0);
            let ts: Vec<f64> = (0..=64).map(|i| f64::from(i) / 64.0).filter(|t| inside(near, [w.a[0] + dx * t, w.a[1] + dy * t])).collect();
            let (t0, t1) = (ts.first().copied().unwrap_or(0.0), ts.last().copied().unwrap_or(1.0));
            let tm = (t0 + t1) / 2.0;
            let m = [w.a[0] + dx * tm, w.a[1] + dy * tm];
            let l = (dx.hypot(dy) * (t1 - t0)).max(1e-9);
            let full = dx.hypot(dy).max(1e-9);
            let (mut nx, mut ny) = (-dy / full, dx / full);
            if nx * (centre[0] - m[0]) + ny * (centre[1] - m[1]) < 0.0 {
                nx = -nx;
                ny = -ny;
            }
            let back = 6.0;
            let eye_ft = [m[0] + nx * back, m[1] + ny * back];
            let e = frame.to_sheet(&area.level, eye_ft);
            let t = frame.to_sheet(&area.level, m);
            model_imgs.push(ModelImage {
                file: format!("model/elevation_{:02}.png", k + 1),
                kind: "elevation",
                what: format!("the wall {} seen from inside the area (on its {} side, {:.1} ft along it)", w.id, compass(-nx, -ny), l),
                of: None,
                rect: None,
                px: Some([1200, 900]),
                wall: Some(w.id.clone()),
                render: Some(json!({"width": 1200, "height": 900, "level": area.level, "eye": [r2(e.x), r2(e.y), r2(z)], "target": [r2(t.x), r2(t.y), r2(z)], "around": [r2(proj_box[0]), r2(proj_box[1]), r2(proj_box[2]), r2(proj_box[3])]})),
                over: None,
            });
        }
    }

    // Submittal pages of the products present.
    let mut subs: Vec<(String, String, u32, Option<std::path::PathBuf>)> = Vec::new();
    for (i, sb) in here.submittals.iter().enumerate() {
        let (name, page) = submittal_ref(sb);
        let path = find_submittal(&job_submittals, &name);
        if path.is_none() {
            left_out.push((format!("submittal {sb}"), "its file is not among the job's submittals (bim.submittals paths)".into()));
            continue;
        }
        subs.push((format!("prints/s{:02}_submittal_{}_p{page}.png", i + 1, safe(&name)), sb.clone(), page, path));
    }

    // The cap: print crops first (each followed by its overlay), then submittals, then the rest.
    let mut order: Vec<(u8, usize)> = Vec::new();
    for (i, c) in crops.iter().enumerate() {
        order.push((0, i));
        if c.place.is_some()
            && let Some(j) = model_imgs.iter().position(|m| m.over == Some(i))
        {
            order.push((1, j));
        }
    }
    for (j, m) in model_imgs.iter().enumerate().filter(|(_, m)| m.over.is_none() && m.kind == "plan") {
        let _ = m;
        order.push((1, j));
    }
    for k in 0..subs.len() {
        order.push((2, k));
    }
    for (j, m) in model_imgs.iter().enumerate().filter(|(_, m)| m.over.is_none() && m.kind != "plan") {
        let _ = m;
        order.push((1, j));
    }
    let kept: BTreeSet<(u8, usize)> = order.iter().take(max_images).copied().collect();
    for (t, i) in order.iter().skip(max_images) {
        let f = match t {
            0 => crops.get(*i).map(|c| c.file.clone()),
            1 => model_imgs.get(*i).map(|m| m.file.clone()),
            _ => subs.get(*i).map(|x| x.0.clone()),
        };
        if let Some(f) = f {
            left_out.push((f, format!("over the packet's cap of {max_images} pictures (max_images)")));
        }
    }
    let crops: Vec<(usize, &Crop)> = crops.iter().enumerate().filter(|(i, _)| kept.contains(&(0, *i))).collect();
    let model_imgs_kept: Vec<&ModelImage> = model_imgs.iter().enumerate().filter(|(j, _)| kept.contains(&(1, *j))).map(|(_, m)| m).collect();
    let subs: Vec<&(String, String, u32, Option<std::path::PathBuf>)> =
        subs.iter().enumerate().filter(|(k, _)| kept.contains(&(2, *k))).map(|(_, x)| x).collect();

    // Every file is checked before any is written.
    let dir = std::path::Path::new(&out_dir);
    let full = |f: &str| dir.join(f).to_string_lossy().into_owned();
    let mut files: Vec<String> = ["manifest.json", "objects.json", "rules.json", "prompt.md"].iter().map(|f| full(f)).collect();
    files.extend(crops.iter().map(|(_, c)| full(&c.file)));
    files.extend(model_imgs_kept.iter().map(|m| full(&m.file)));
    files.extend(subs.iter().map(|x| full(&x.0)));
    for f in &files {
        super::exchange::guard_output(s, CMD, p, f)?;
    }
    if cfg!(target_arch = "wasm32") {
        return Err(bad(CMD, "a packet is a folder of files: not available on the web"));
    }
    for sub in ["", "prints", "model"] {
        let pth = dir.join(sub);
        crate::writefile::check(&pth).map_err(|e| bad(CMD, format!("{}: {e}", pth.display())))?;
        std::fs::create_dir_all(&pth).map_err(|e| bad(CMD, format!("{}: {e}", pth.display())))?;
    }

    // The pictures.
    let all_rects: Vec<[f64; 4]> = crops.iter().map(|(_, c)| c.rect).collect();
    let index = PrintIndex::of(d, &all_rects);
    let write_png = |file: &str, png: Option<Vec<u8>>, left_out: &mut Vec<(String, String)>| -> Result<bool> {
        match png {
            Some(b) => {
                super::exchange::write_file(CMD, &full(file), &b)?;
                Ok(true)
            }
            None => {
                left_out.push((file.to_string(), "the picture couldn't be drawn".into()));
                Ok(false)
            }
        }
    };
    let mut prints_out: Vec<Value> = Vec::new();
    for (_, c) in &crops {
        let ents = index.within(c.rect);
        if write_png(&c.file, png_of(d, &ents, c.rect, c.px), &mut left_out)? {
            prints_out.push(c.to_json());
        }
    }
    let mut model_out: Vec<Value> = Vec::new();
    for m in &model_imgs_kept {
        let ok = match (m.kind, m.over, &m.render) {
            ("overlay", Some(i), _) => {
                let Some(c) = crops.iter().find(|(k, _)| *k == i).map(|(_, c)| *c) else { continue };
                let place = c.place.clone().unwrap_or(Place::Same);
                let od = model_drawing(d, &index.within(c.rect), &model, &area, &frame, &place, &all_rules, &built, upf);
                write_png(&m.file, render_drawing(&od, c.rect, c.px), &mut left_out)?
            }
            ("plan", _, _) => {
                let (Some(rect), Some(px)) = (m.rect, m.px) else { continue };
                let od = model_drawing(d, &[], &model, &area, &frame, &Place::Same, &all_rules, &built, upf);
                write_png(&m.file, render_drawing(&od, rect, px), &mut left_out)?
            }
            (_, _, Some(r)) => {
                let mut q = r.clone();
                q["path"] = json!(full(&m.file));
                q["force"] = json!(true);
                match s.execute("model.render", &q) {
                    Ok(_) => true,
                    Err(e) => {
                        left_out.push((m.file.clone(), format!("model.render: {e}")));
                        false
                    }
                }
            }
            _ => false,
        };
        if ok {
            model_out.push(m.to_json());
        }
    }
    let mut subs_out: Vec<Value> = Vec::new();
    for (file, sb, page, path) in &subs {
        let Some(path) = path else { continue };
        match submittal_png(path, *page) {
            Ok(b) => {
                super::exchange::write_file(CMD, &full(file), &b)?;
                subs_out.push(json!({"file": file, "submittal": sb, "page": page}));
            }
            Err(e) => left_out.push((file.clone(), format!("submittal {sb}: {e}"))),
        }
    }

    // The texts.
    let kept_crops: Vec<Crop> = crops.iter().map(|(_, c)| (*c).clone()).filter(|c| prints_out.iter().any(|j| j["file"] == json!(c.file))).collect();
    let kept_models: Vec<ModelImage> =
        model_imgs_kept.iter().map(|m| (*m).clone()).filter(|m| model_out.iter().any(|j| j["file"] == json!(m.file))).collect();
    let prompt = prompt_md(&area, &kept_crops, &kept_models, &rules_on, &here.counts);
    left_out.sort();
    left_out.dedup();
    let manifest = json!({
        "packet": 1,
        "area": area.to_json(),
        "margin_ft": r2(margin),
        "dpi": r2(dpi),
        "trades": trades,
        "pixel_of_sheet_point": "x = (sheet_x - sheet_rect[0]) * px_per_unit, y = (sheet_rect[3] - sheet_y) * px_per_unit",
        "prints": prints_out,
        "model": model_out,
        "submittals": subs_out,
        "objects": "objects.json",
        "rules": "rules.json",
        "prompt": "prompt.md",
        "counts": {
            "elements": here.counts.get("elements"),
            "runs": here.counts.get("runs"),
            "walls": here.counts.get("walls"),
            "openings": here.counts.get("openings"),
            "rules": rules_on.len(),
            "pictures": prints_out.len() + model_out.len() + subs_out.len(),
        },
        "left_out": left_out.iter().map(|(w, y)| json!({"what": w, "why": y})).collect::<Vec<_>>(),
    });
    let pretty = |v: &Value| serde_json::to_string_pretty(v).unwrap_or_default() + "\n";
    super::exchange::write_file(CMD, &full("objects.json"), pretty(&here.objects).as_bytes())?;
    super::exchange::write_file(CMD, &full("rules.json"), pretty(&rules_json).as_bytes())?;
    super::exchange::write_file(CMD, &full("prompt.md"), prompt.as_bytes())?;
    super::exchange::write_file(CMD, &full("manifest.json"), pretty(&manifest).as_bytes())?;
    let mut out = manifest;
    let ms = start.elapsed().as_millis() as u64;
    if out.to_string().len() > super::agentready::ANSWER_MAX {
        out = json!({
            "packet": 1,
            "area": out["area"].clone(),
            "counts": out["counts"].clone(),
            "prints": out["prints"].as_array().map(|a| a.iter().map(|c| c["file"].clone()).collect::<Vec<_>>()),
            "model": out["model"].as_array().map(|a| a.iter().map(|c| c["file"].clone()).collect::<Vec<_>>()),
            "left_out": out["left_out"].as_array().map_or(0, Vec::len),
            "note": "the full manifest is manifest.json in out_dir",
        });
    }
    out["out_dir"] = json!(out_dir);
    out["ms"] = json!(ms);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms_are_told_apart_by_name() {
        assert_eq!(room_kind("CORRIDOR"), "corridor");
        assert_eq!(room_kind("Elec. Room"), "equipment_room");
        assert_eq!(room_kind("MECH/ELEC"), "equipment_room");
        assert_eq!(room_kind("Office"), "room");
        assert_eq!(room_kind("Electronics Lab"), "room");
    }

    #[test]
    fn sheets_are_told_apart_by_title() {
        assert_eq!(sheet_kind("FIRST FLOOR REFLECTED CEILING PLAN", true), "rcp");
        assert_eq!(sheet_kind("POWER PLAN - LEVEL 1", true), "plan");
        assert_eq!(sheet_kind("ENLARGED PLANS", true), "enlarged_plan");
        assert_eq!(sheet_kind("INTERIOR ELEVATIONS", false), "interior_elevation");
        assert_eq!(sheet_kind("PARTITION TYPES", false), "partition_legend");
        assert_eq!(sheet_kind("LIGHTING FIXTURE SCHEDULE", false), "schedule");
        assert_eq!(sheet_kind("SYMBOLS AND ABBREVIATIONS", false), "legend");
        assert_eq!(sheet_kind("GENERAL NOTES", false), "general_notes");
        assert_eq!(trade_of_discipline("E"), "electrical");
        assert_eq!(trade_of_discipline("fp"), "fire");
    }

    #[test]
    fn paper_is_read_from_the_sheet_shape() {
        assert_eq!(paper_of([0.0, 0.0, 3456.0, 2304.0]).0, 36.0);
        assert_eq!(paper_of([0.0, 0.0, 42.0, 30.0]).0, 42.0);
        assert_eq!(paper_of([0.0, 0.0, 36.0, 48.0]).0, 48.0);
    }

    #[test]
    fn tiles_cover_the_crop_in_whole_pixels() {
        let (ppu, t) = tiles([0.0, 0.0, 100.0, 50.0], 10.0);
        assert_eq!(ppu, 10.0);
        assert_eq!(t.len(), 1);
        assert_eq!(t.first().map(|x| x.1), Some([1000, 500]));
        let (ppu, t) = tiles([0.0, 0.0, 500.0, 100.0], 10.0);
        assert_eq!(ppu, 10.0);
        assert_eq!(t.len(), 3, "5000 px wide in 2000 px tiles");
        let w: u32 = t.iter().map(|x| x.1[0]).sum();
        assert_eq!(w, 5000);
        assert!(t.iter().all(|x| (x.0[2] - x.0[0]) * ppu - f64::from(x.1[0]) < 1e-6));
        // Too big for 4×4 tiles: the scale drops.
        let (ppu, t) = tiles([0.0, 0.0, 10_000.0, 100.0], 10.0);
        assert!(ppu < 10.0 && t.len() <= 4, "{ppu} {}", t.len());
    }

    #[test]
    fn submittal_references_split_into_file_and_page() {
        assert_eq!(submittal_ref("Lighting.pdf p.3"), ("Lighting.pdf".into(), 3));
        assert_eq!(submittal_ref("Panels.pdf"), ("Panels.pdf".into(), 1));
    }

    #[test]
    fn a_submittal_is_found_by_its_file_name_or_its_stem() {
        let dir = std::env::temp_dir().join(format!("cadaddy-subs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("Submittal #265119-1 Lighting Package.pdf");
        std::fs::write(&f, b"%PDF-1.4").unwrap();
        let paths = vec![dir.to_string_lossy().to_string()];
        assert_eq!(find_submittal(&paths, "Submittal #265119-1 Lighting Package"), Some(f.clone()));
        assert_eq!(find_submittal(&paths, "submittal #265119-1 lighting package.PDF"), Some(f.clone()));
        assert_eq!(find_submittal(&[f.to_string_lossy().to_string()], "Submittal #265119-1 Lighting Package"), Some(f.clone()));
        assert_eq!(find_submittal(&paths, "Lighting"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_window_id_round_trips() {
        let a = window_area("L1", [10.0, 4.0, 2.0, 0.0]);
        assert_eq!(a.id, "L1/win:2.00,0.00,10.00,4.00");
        assert_eq!(a.bbox, [2.0, 0.0, 10.0, 4.0]);
    }
}
