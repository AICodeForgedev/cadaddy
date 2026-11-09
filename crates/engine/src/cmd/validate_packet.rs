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
//!     itself), every sheet an element of the area was read from (Q6134: placed when it can
//!     be, else round the element's source point on it, else round the room's tag, else the
//!     whole view in tiles marked `unregistered`), the level's sheets by their level tag or a
//!     title naming the level (offered the same way; a partial view that names neither the
//!     room's number nor its name is left out, saying so), interior elevations naming the room, the partition legend and the schedule or
//!     legend rows of the type marks present, general-notes blocks, and the submittal pages of
//!     the products present and of the job's submittals for the area's marks and kinds (one the
//!     job applied but does not list is named in `submittals_unlisted`). No two crops are the
//!     same view, none is narrower than 320 px. A crop is at least 150 dpi of the printed sheet (the paper size is
//!     taken from the sheet's shape, ARCH/ANSI/ISO), in tiles when large; the manifest gives
//!     each crop's sheet rectangle and pixel scale, so a sheet point is at
//!     `((x − x0)·s, (y1 − y)·s)` pixels. A crop is drawn as its PDF page prints (Q6135): the
//!     lettering the import read off strokes as those strokes (the readings, often at a glyph
//!     cluster's size, are left out), no masks (the import's merged masks sit later than what
//!     they covered), nothing on a layer that is off; each picture is labelled by the view of
//!     its own sheet it overlaps most; each crop is compared with its sheet's PDF page read
//!     again (the job's file and page, `prints` when the job's folder isn't here) at the same
//!     rectangle and pixels, the similarity in the manifest, a low one flagged there and in
//!     `prompt.md`. Only pictures whose files were written are listed. Each crop placed on the
//!     level's plan has its registration checked against the model's walls (Q6141,
//!     [`super::validate_register`]): the residual, turn and scale of the sheet's wall lines
//!     against the model's walls in the area; a crop off by more than the threshold (1 ft, the
//!     project rule `validate.registration.residual_ft`) is placed again from the wall lines
//!     when they fit well, else marked `misregistered` with the numbers (manifest
//!     `registration`, and in `prompt.md`).
//!   - `model/`: the model over each plan crop (the print in grey, the model by trade colour:
//!     architecture blue, electrical red, mechanical green, plumbing cyan, fire magenta), a
//!     clean plan of the model alone, and when the model is built a 3D view of the area and an
//!     elevation of each side of it, the camera inside the area (one per direction, the longest
//!     sides first; `model.render` with a camera).
//!   - `objects.json`: every model element in the area (handle, kind, trade, mark, tag, name,
//!     level, room, position, height above the floor, host wall and face, mounting, size,
//!     system, circuit, switch leg, maker, catalog number, submittal, source sheet and point, and
//!     the `bim.validate` failures already found for it), the runs, walls and openings. A
//!     generic element's kind (`device`) is what its records, the job's legend and tables or
//!     the project say it is ([`super::validate_kind`]; `kind_read` the kind as built,
//!     `kind_from` where the kind came from), its `category` the IFC class it is written as,
//!     and its trade that category's (an air terminal and an exhaust fan are mechanical).
//!     Each element and run carries its `provenance` (drawn, derived or inferred, with its
//!     basis) and its `corrections` (each value the build corrected for the field: the design
//!     value, the corrected value, the reason and the rule), and `project_rules` lists the
//!     project's active rules with tier and source ([`super::validate_basis`], Q6259).
//!   - `rules.json`: the validation rules (`validate.rules`) that apply to the kinds, BIM
//!     categories and marks present, with the project's overrides applied.
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
use super::validate_basis as vb;
use super::validate_kind::{Kinds, Resolved};
use super::validate_prescreen::{self as ps, UnitOf};
use super::validate_register as vreg;
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
/// Objects looked at, at most: every one (as [`super::validate::MAX_ENTITIES`]).
const MAX_ENTITIES: usize = super::validate::MAX_ENTITIES;
/// Objects drawn in one picture, at most.
const MAX_CROP_ENTITIES: usize = 400_000;
/// Generated underground sheets drawn per packet, at most, and their long side (pixels).
const MAX_UNDERGROUND_SHEETS: usize = 4;
const MAX_UNDERGROUND_PX: f64 = 6_000.0;
const MAX_AREAS: usize = 50_000;
const DEFAULT_AREA_LIMIT: usize = 100;
const MAX_AREA_LIMIT: usize = 500;
/// Row and note crops of one sheet, at most.
const MAX_TEXT_CROPS_PER_SHEET: usize = 6;
const MAX_ELEVATIONS: usize = 6;
/// The sheets an area's elements cite, and the source points kept per sheet.
const MAX_CITED_SHEETS: usize = 200;
const MAX_CITED_POINTS: usize = 500;
/// The shortest side of a print crop, pixels (a narrower view is widened round its centre).
const MIN_CROP_PX: f64 = 320.0;
/// A view of a sheet smaller than this share of it is a partial view (a stair, an elevator, one
/// unit), not the level's plan (a sheet of two or four floors' plans gives each a fifth or more).
const PARTIAL_VIEW: f64 = 0.10;
/// Two crops of one sheet sharing this much of their union are the same view.
const SAME_VIEW: f64 = 0.9;
/// Tiles a side of a whole view of a sheet that isn't placed on the plan.
const WHOLE_TILES_SIDE: u32 = 2;
/// Tiles a side of a whole schedule, legend or notes sheet (a 42" sheet at about 143 dpi).
const DOC_TILES_SIDE: u32 = 3;
/// The submittal pages offered for a kind no element of the area carries a submittal for.
const MAX_KIND_PAGES: usize = 3;
/// The submittal pages looked at across the building.
const MAX_SUBMITTAL_PAGES: usize = 2_000;
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
/// How far outside every room's outline an element still belongs to the nearest room (an
/// item on the wall line of its room, feet), and how far round the area the packet lists the
/// elements beside it (`nearby`).
const EDGE_FT: f64 = 1.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD_AREAS, "Validation Areas", run_areas)
            .params("{level?: name, kind?: room|zone|equipment_room|corridor, area?: [x0,y0,x1,y1] (building feet: a custom window, listed as a zone; needs `level` when the building has several), cursor?: n, limit?: n (100, at most 500)} → {total, cursor, areas: [{id, kind, level, number?, name?, room?, stretch?: [i, n], outline: [[x,y]] (feet), box: [x0,y0,x1,y1] (feet)}], next?, levels}: the areas to check, one room, corridor stretch (≤ 40 ft) or equipment room each; ids are stable")
            .noundo(),
        CommandSpec::new(CMD_PACKET, "Validation Evidence Packet", run_packet).long()
            .params("{area?: id from validate.areas | {area: [x0,y0,x1,y1] (building feet), level?} | [x0,y0,x1,y1] (with level), room?: number or name (its one area; with level?: on that level), out_dir: folder, trades?: [architecture|electrical|mechanical|plumbing|fire|general] (default all), rules?: [rule id] (only these of the rules that apply; default all), force?: bool (replace files of an earlier packet; otherwise they are refused), margin_ft?: 0–50 (5), dpi?: 150–600 (150), max_images?: 1–200 (40), compare?: bool (true: each crop against its PDF page), prints?: folder of the job's PDFs (when the job's isn't on this machine), specs?: [path] (specification section PDFs or folders, besides the job's)} → the manifest: {packet, area, prints: [{file, sheet, title, view?, kind, trade, sheet_rect, px, px_per_unit, dpi, paper, tile?, placed_by: reference|registration|view_placement|same_sheet|unit_type|text|source_point|room_tag|unregistered|keynote|gear (a gear_riser, panel_schedule or feeder_schedule sheet shown whole for the gear in the area), unregistered?: true (not placed on the plan: no overlay), cited?: true (an element here was read from it), page?, similarity?: 0–1 (to its PDF page), similarity_low?: true, registration?: {status: ok|refitted|misregistered|not_checked, residual_ft, rotation_deg, scale, shift_ft, fit_ft, coverage, coverage_before, walls_ft, lines, threshold_ft, why?, read?} (a placed crop's registration against the model's walls: refitted crops are placed again from the sheet's wall lines)}], registration: {about, threshold_ft (1, project rule validate.registration.residual_ft), threshold_from, counts, by_residual: [{file, sheet, view?, status, residual_ft}] (worst first)}, similarity: {about, low_under, compared, low: [{file, similarity}], not_compared: [{sheet, why}]}, model: [{file, kind, of?, sheet_rect?, px?, wall?}], submittals: [{file, submittal, page, for}], submittals_unlisted: [{submittal, pages, for, applied_to, marks}] (applied by the job, its file not among the job's submittals), specs: [{file, section, page, product, for, cropped_to_product}] (the specification pages the area's product records cite, cut to the product's paragraph when a line on the page names it), objects, rules, prompt, counts, left_out: [{what, why}]}; every sheet an element of the area was read from is in it (placed, else round the source points, else round the room's tag, else the whole view in tiles), and every sheet of the level (by its tag or its title) unless it doesn't show the area (said in left_out); one wall elevation per side direction, from inside the area: writes prints/, model/, objects.json, rules.json, prompt.md and manifest.json into out_dir")
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

pub(crate) fn intersect(a: [f64; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r[0] < r[2] && r[1] < r[3]).then_some(r)
}

pub(crate) fn inside(r: [f64; 4], p: [f64; 2]) -> bool {
    p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3]
}

pub(crate) fn corners(r: [f64; 4]) -> [[f64; 2]; 4] {
    [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]]
}

pub(crate) fn norm_rect(r: [f64; 4]) -> [f64; 4] {
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

/// The part of the polygon `poly` inside the rectangle `r` ([x0, y0, x1, y1]): the polygon
/// clipped against each of the rectangle's four sides in turn.
pub(crate) fn clip_to_rect(poly: &[[f64; 2]], r: [f64; 4]) -> Vec<[f64; 2]> {
    // Each side: which coordinate, the bound, and whether inside is above it.
    let sides = [(0usize, r[0], true), (0, r[2], false), (1, r[1], true), (1, r[3], false)];
    let mut pts: Vec<[f64; 2]> = poly.to_vec();
    for (axis, bound, above) in sides {
        let inside = |p: &[f64; 2]| if above { p[axis] >= bound } else { p[axis] <= bound };
        let cross = |p: &[f64; 2], q: &[f64; 2]| {
            let (pa, qa) = (p[axis], q[axis]);
            let t = if (qa - pa).abs() > 1e-12 { (bound - pa) / (qa - pa) } else { 0.0 };
            [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]
        };
        let n = pts.len();
        let mut next = Vec::with_capacity(n + 4);
        for i in 0..n {
            let (Some(p), Some(q)) = (pts.get(i), pts.get((i + 1) % n)) else { continue };
            match (inside(p), inside(q)) {
                (true, true) => next.push(*q),
                (true, false) => next.push(cross(p, q)),
                (false, true) => {
                    next.push(cross(p, q));
                    next.push(*q);
                }
                (false, false) => {}
            }
        }
        pts = next;
        if pts.is_empty() {
            break;
        }
    }
    // Drop repeated points (a corner on the rectangle's side).
    pts.dedup_by(|a, b| (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
    pts
}

/// A polygon's area (absolute), its points' units squared.
fn polygon_area(poly: &[[f64; 2]]) -> f64 {
    let n = poly.len();
    (0..n).filter_map(|i| Some((poly.get(i)?, poly.get((i + 1) % n)?))).map(|(a, b)| a[0] * b[1] - b[0] * a[1]).sum::<f64>().abs() / 2.0
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
        let Some(e) = d.model.get(r.handle) else { continue };
        let outline: Vec<[f64; 2]> = r.outline.iter().take(10_000).map(|p| frame.built_on(&r.level, e, *p)).map(|p| [r4(p[0]), r4(p[1])]).collect();
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
            // Each stretch is the room's floor in its cell, not the cell: a cell of a big room's
            // box reaches over its neighbours, and an element there is theirs (Q6160).
            let pieces: Vec<(Vec<[f64; 2]>, [f64; 4])> = cells
                .into_iter()
                .filter_map(|cell| {
                    let piece: Vec<[f64; 2]> = clip_to_rect(&outline, cell).into_iter().map(|p| [r4(p[0]), r4(p[1])]).collect();
                    let b = bbox(&piece)?;
                    (piece.len() >= 3 && polygon_area(&piece) > 1e-6).then_some((piece, b))
                })
                .collect();
            let total = pieces.len();
            for (k, (piece, b)) in pieces.into_iter().enumerate() {
                out.push(Area { id: format!("{id}#{}", k + 1), stretch: Some([k + 1, total]), outline: piece, bbox: b, ..base.clone() });
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

/// Whether an area is (a stretch of) the room a call names by its number, name or id.
pub(crate) fn names_room(a: &Area, r: &str) -> bool {
    let base = a.id.split('#').next().unwrap_or(&a.id);
    a.number.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(r))
        || a.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(r))
        || a.id.eq_ignore_ascii_case(r)
        || base.eq_ignore_ascii_case(r)
}

/// The area a packet call names: `area` (an id, a window with its level), else the one area of
/// the `room` it names (on `level` when given), as `validate` takes its scope.
pub(crate) fn area_param(cmd: &str, sp: &BuildingSpec, p: &Value, d: &Drawing, frame: &Frame) -> Result<Area> {
    if p.get("area").is_none_or(Value::is_null)
        && let Some(r) = str_param(p, "room").map(str::trim).filter(|r| !r.is_empty())
    {
        let level = level_param(cmd, sp, p, false)?;
        let found: Vec<Area> = model_areas(d, frame)
            .into_iter()
            .filter(|a| names_room(a, r) && level.as_ref().is_none_or(|l| a.level.eq_ignore_ascii_case(l)))
            .collect();
        let r = r.chars().take(40).collect::<String>();
        return match found.len() {
            0 => Err(bad(cmd, format!("no model room is numbered or named {r} (validate.areas lists them)"))),
            1 => found.into_iter().next().ok_or_else(|| bad(cmd, "no area")),
            n => {
                let ids: Vec<&str> = found.iter().take(12).map(|a| a.id.as_str()).collect();
                Err(bad(cmd, format!("room {r} is {n} areas (a packet is one): give `area`, one of {}", ids.join(", "))))
            }
        };
    }
    match p.get("area") {
        Some(a @ Value::Array(_)) => {
            let w = four(cmd, a, "area")?;
            let lv = level_param(cmd, sp, p, true)?.unwrap_or_default();
            Ok(window_area(&lv, w))
        }
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
        _ if str_param(p, "level").is_some() => {
            Err(bad(cmd, "a packet is one area: give `area` (validate.areas {level} lists the level's) or `room`"))
        }
        _ => Err(bad(cmd, "`area` is an id from validate.areas, or {area: [x0, y0, x1, y1] (building feet), level}; or give `room`")),
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
    /// Placed again: `c` carries a point this placement puts on the sheet to where the sheet
    /// really has it (a wall-line fit, [`super::validate_register`]).
    fn corrected(&self, c: &vreg::Sim, reference: &str, matched: usize, fit: f64) -> Option<Place> {
        let to_project = match self {
            Place::Same => vreg::Sim::IDENTITY,
            Place::Fit(r) => vreg::Sim { a: r.a, b: r.b, tx: r.tx, ty: r.ty },
        };
        let m = to_project.after(&c.inverse()?);
        let reference = match self {
            Place::Fit(r) => r.reference.clone(),
            Place::Same => reference.to_string(),
        };
        Some(Place::Fit(SheetRegistration { reference, method: "walls".into(), a: m.a, b: m.b, tx: m.tx, ty: m.ty, matched, residual: Some(fit) }))
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
    /// The sheet is not placed on the level's plan: the model can't be drawn over it.
    unregistered: bool,
    /// An element of the area was read from this sheet.
    cited: bool,
    /// The whole view on the sheet (its tiles share it).
    whole: [f64; 4],
    /// A placed crop's registration against the model's walls (Q6141).
    registration: Option<Value>,
    /// Why it decides heights, types or ratings (Q6261): the prescreen cites it, or it is a
    /// schedule, legend, notes sheet or keynote list of the area's trades. The picture cap never
    /// drops it.
    deciding: Option<String>,
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
            if self.unregistered {
                m.insert("unregistered".into(), json!(true));
            }
            if self.cited {
                m.insert("cited".into(), json!(true));
            }
            if let Some(r) = &self.registration {
                m.insert("registration".into(), r.clone());
            }
            if let Some(w) = &self.deciding {
                m.insert("deciding".into(), json!(w));
            }
        }
        o
    }
}

/// `r` widened round its centre to at least `min` a side, kept on `sheet` where it can be.
fn widen(r: [f64; 4], min: f64, sheet: [f64; 4]) -> [f64; 4] {
    let mut out = r;
    for (lo, hi) in [(0usize, 2usize), (1, 3)] {
        let (a, b) = (r[lo], r[hi]);
        let (s0, s1) = (sheet[lo], sheet[hi]);
        if !(min.is_finite() && b - a < min) {
            continue;
        }
        let half = min.min((s1 - s0).max(b - a)) / 2.0;
        let c = (a + b) / 2.0;
        let (mut x0, mut x1) = (c - half, c + half);
        if x0 < s0 {
            x1 += s0 - x0;
            x0 = s0;
        }
        if x1 > s1 {
            x0 = (x0 - (x1 - s1)).max(s0);
            x1 = s1;
        }
        out[lo] = x0;
        out[hi] = x1;
    }
    out
}

/// How much of `a` and `b` is shared: the overlap over the union of the two rectangles.
fn overlap(a: [f64; 4], b: [f64; 4]) -> f64 {
    let area = |r: [f64; 4]| ((r[2] - r[0]) * (r[3] - r[1])).max(0.0);
    let i = intersect(a, b).map_or(0.0, area);
    let u = area(a) + area(b) - i;
    if u > 0.0 { i / u } else { 0.0 }
}

/// A level's number from its name (`Level 2`, `L2`, `02`), if it has one.
fn level_number(level: &str) -> Option<u32> {
    let digits: String = level.chars().filter(char::is_ascii_digit).take(3).collect();
    digits.parse().ok()
}

/// The floor a word counts (`2ND`, `SECOND`, `2`), if it is one.
fn ordinal(word: &str) -> Option<u32> {
    const WORDS: [&str; 12] = ["FIRST", "SECOND", "THIRD", "FOURTH", "FIFTH", "SIXTH", "SEVENTH", "EIGHTH", "NINTH", "TENTH", "ELEVENTH", "TWELFTH"];
    if let Some(i) = WORDS.iter().position(|w| *w == word) {
        return u32::try_from(i + 1).ok();
    }
    let digits = word.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    let suffix = word.get(digits.len()..).unwrap_or("");
    if digits.is_empty() || digits.len() > 3 || !digits.chars().all(|c| c.is_ascii_digit()) || !matches!(suffix, "" | "ST" | "ND" | "RD" | "TH") {
        return None;
    }
    digits.parse().ok()
}

/// Whether a sheet's (or view's) title names `level`: `LEVEL 2`, `2ND FLOOR`, `SECOND FLOOR`,
/// `2ND-4TH FLOOR`, `FLOOR 2`; a level with no number by its name (`ROOF PLAN`).
pub(crate) fn title_names_level(title: &str, level: &str) -> bool {
    let up = upper_words(&title.replace(['.', '-', '/', '_', ',', '(', ')', '&'], " "));
    let words: Vec<&str> = up.split_whitespace().take(200).collect();
    let Some(n) = level_number(level) else {
        let name = upper_words(level);
        return name.len() >= 3 && format!(" {up} ").contains(&format!(" {name} "));
    };
    const JOIN: [&str; 4] = ["THRU", "THROUGH", "TO", "AND"];
    for (i, w) in words.iter().enumerate() {
        if !matches!(*w, "FLOOR" | "FLOORS" | "LEVEL" | "LEVELS") {
            continue;
        }
        // `LEVEL 2`, `FLOOR 2`.
        if words.get(i + 1).and_then(|x| x.parse::<u32>().ok()) == Some(n) {
            return true;
        }
        // `2ND FLOOR`, `SECOND THRU FOURTH FLOORS`, `2ND-4TH FLOOR`.
        let mut nums: Vec<u32> = Vec::new();
        for x in words.iter().take(i).rev().take(4) {
            match ordinal(x) {
                Some(k) => nums.push(k),
                None if JOIN.contains(x) => {}
                None => break,
            }
        }
        match nums.as_slice() {
            [one] if *one == n => return true,
            [hi, lo, ..] if (*lo.min(hi)..=*lo.max(hi)).contains(&n) => return true,
            _ => {}
        }
    }
    false
}

/// A turn, degrees counter-clockwise in [0, 360), to a tenth.
fn turn_deg(rad: f64) -> f64 {
    let d = (rad.to_degrees().rem_euclid(360.0) * 10.0).round() / 10.0;
    if d >= 360.0 || !d.is_finite() { 0.0 } else { r2(d) }
}

/// A layout fit matched by fewer of the type's items than this is no certain turn.
const MIN_FIT_ITEMS: u64 = 3;

/// Why a crop's turn and mirror against the room are not certain: no turn known (only the
/// unit's hand), or a layout fit that matched too few items. None when they are.
fn turn_doubt(o: &Value) -> Option<String> {
    if o.get("rotation_deg").is_none_or(Value::is_null) {
        return Some(if o.get("mirrored").and_then(Value::as_bool).unwrap_or(false) {
            "only the unit's hand says it is mirrored; its turn is not known".into()
        } else {
            "its turn against the room is not known".into()
        });
    }
    let n = o.get("matched").and_then(Value::as_u64)?;
    (n < MIN_FIT_ITEMS).then(|| format!("its turn was fitted by {n} of {} items only", o.get("of").and_then(Value::as_u64).unwrap_or(0)))
}

/// How a crop reads against the plan, in words, when it is turned or mirrored; a turn that is
/// not certain says so (positions read off it are unsure).
fn turn_words(o: &Value) -> Option<String> {
    let deg = o.get("rotation_deg").and_then(Value::as_f64);
    let mirrored = o.get("mirrored").and_then(Value::as_bool).unwrap_or(false);
    let turned = deg.filter(|d| d.abs() > 0.05 && (d - 360.0).abs() > 0.05);
    if let Some(why) = turn_doubt(o) {
        return Some(format!("{why}: match it to the room by the walls and doors; a position read off it is unsure"));
    }
    // A measured view that draws more than the room (two mirrored rooms side by side): which
    // part of it the room is.
    let part = o
        .get("room_in_view_ft")
        .and_then(Value::as_array)
        .and_then(|v| Some([v.first()?.as_f64()?, v.get(1)?.as_f64()?]))
        .filter(|v| v[0].hypot(v[1]) >= ROOM_OFF_MIDDLE_FT)
        .map(|v| {
            let side = if v[0].abs() >= v[1].abs() {
                if v[0] < 0.0 { "left" } else { "right" }
            } else if v[1] < 0.0 {
                "lower"
            } else {
                "upper"
            };
            format!("the room is the {side} part of the view ({:.1} ft off its middle)", v[0].hypot(v[1]))
        });
    let words = match (turned, mirrored) {
        (None, false) => None,
        (Some(d), false) => Some(format!("drawn turned {d}° from the room as the plan shows it")),
        (None, true) => Some("drawn mirrored (the room is the opposite hand)".into()),
        (Some(d), true) => Some(format!("drawn mirrored and turned {d}° from the room as the plan shows it")),
    };
    match (words, part) {
        (Some(w), Some(p)) => Some(format!("{w}; {p}")),
        (None, Some(p)) => Some(format!("drawn as the plan shows the room; {p}")),
        (w, None) => w,
    }
}

/// A room this far (feet) off a measured view's middle is said to be one part of it.
const ROOM_OFF_MIDDLE_FT: f64 = 3.0;

/// A view's turn against the plan from its registration (a fit has no mirror).
fn orientation_of(r: &SheetRegistration, by: &str) -> Value {
    let mut o = json!({"rotation_deg": turn_deg(r.b.atan2(r.a)), "mirrored": false, "scale": r4(r.scale()), "by": by});
    if let Some(w) = turn_words(&o) {
        o["read"] = json!(w);
    }
    o
}

/// The sheet of the view a job's stamping fit (`{.., view: "<sheet> <title>"}`) was measured on,
/// none when the fit names no view.
pub(crate) fn fit_sheet(fit: &Value) -> Option<String> {
    let v = fit.get("view")?.as_str()?.trim();
    let first = v.split_whitespace().next()?;
    Some(first.split('/').next().unwrap_or(first).trim().to_ascii_uppercase())
}

/// Whether the stamping fit `fit` tells the turn of a view on `sheet`: a view of the sheet it was
/// measured on (drawn with it), or any when it names no view. A view on another sheet (another
/// trade's enlarged plan) may be drawn turned another way (Q6339).
pub(crate) fn stamped_on(fit: &Value, sheet: &str) -> bool {
    let base = sheet.split('/').next().unwrap_or(sheet).trim().to_ascii_uppercase();
    fit_sheet(fit).is_none_or(|s| s == base)
}

/// A unit type's view's turn and mirror against the room (Q6339): a plan view's measured from its
/// lines on the room's model walls (`measure`) when that is certain, else `fallback` (what the
/// job's fits say) with the measurement beside it.
fn measured_turn(
    fallback: Value,
    plan_like: bool,
    rect: [f64; 4],
    u: &UnitOf,
    measure: &dyn Fn([f64; 4]) -> std::result::Result<vreg::Turned, String>,
) -> Value {
    let measured = plan_like.then(|| measure(rect));
    let mut o = match &measured {
        Some(Ok(t)) if t.certain => vreg::turn_json(t),
        Some(Ok(t)) => {
            let mut o = fallback;
            o["wall_fit"] = vreg::turn_json(t);
            o
        }
        Some(Err(why)) => {
            let mut o = fallback;
            o["wall_fit"] = json!({"why": why});
            o
        }
        None => fallback,
    };
    o["hand"] = json!(u.hand);
    o["unit_type"] = json!(u.id);
    o["certain"] = json!(turn_doubt(&o).is_none());
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
#[cfg(test)]
fn tiles(rect: [f64; 4], ppu: f64) -> (f64, Vec<([f64; 4], [u32; 2], [u32; 2])>) {
    tiles_max(rect, ppu, MAX_TILES_SIDE)
}

/// [`tiles`] with at most `side` tiles a side.
fn tiles_max(rect: [f64; 4], ppu: f64, side: u32) -> (f64, Vec<([f64; 4], [u32; 2], [u32; 2])>) {
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let longest = w.max(h) * ppu;
    let cap = f64::from(TILE_PX * side.clamp(1, MAX_TILES_SIDE));
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
    /// The block of lettering `seed` starts (a notes or keynote list), whole (Q6261): grown down
    /// and to the right over the lines that carry on its column (a line starting within it, no
    /// more than three line heights below it), each line as wide as its letters, within `bound`.
    /// A reading aid's rectangle, never a measurement.
    fn block(&self, seed: [f64; 4], bound: [f64; 4]) -> [f64; 4] {
        let near: Vec<&(String, [f64; 2], f64)> = self.0.iter().filter(|(_, at, h)| *h > 0.0 && inside(bound, *at)).collect();
        let mut hs: Vec<f64> = near.iter().filter(|(_, at, _)| inside(seed, *at)).map(|t| t.2).collect();
        hs.sort_by(f64::total_cmp);
        let Some(h) = hs.get(hs.len() / 2).copied() else { return seed };
        let mut r = seed;
        let mut used = vec![false; near.len()];
        for _ in 0..400 {
            let mut grew = false;
            for (i, (t, at, th)) in near.iter().enumerate() {
                if used.get(i).copied().unwrap_or(true) || *th > 2.0 * h || *th < 0.4 * h {
                    continue;
                }
                let line = [at[0], at[1] - 0.3 * th, at[0] + 0.9 * th * t.chars().count() as f64, at[1] + 1.2 * th];
                if !(at[0] >= r[0] - h && at[0] <= r[2] && line[3] >= r[1] - 3.0 * h && line[1] <= r[3]) {
                    continue;
                }
                if let Some(u) = used.get_mut(i) {
                    *u = true;
                }
                let g = [r[0].min(line[0]), r[1].min(line[1]), r[2].max(line[2]), r[3].max(line[3])];
                let g = intersect(g, bound).unwrap_or(r);
                if g.iter().zip(r).any(|(a, b)| (a - b).abs() > 1e-9) {
                    r = g;
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        r
    }
}

// ---------------------------------------------------------------- the model in the area

/// The trade of an element: the trade of the first rule naming its BIM category (an air terminal
/// and an exhaust fan are mechanical), else of its kind.
fn trade_of(r: &Resolved, rules: &[Effective]) -> &'static str {
    let by_category = r
        .category
        .as_deref()
        .and_then(|c| rules.iter().find(|x| x.rule.trade != "general" && x.rule.applies_to.categories.iter().any(|k| k.eq_ignore_ascii_case(c))));
    match by_category {
        Some(x) => vr::TRADES.iter().find(|t| **t == x.rule.trade).copied().unwrap_or("general"),
        None => trade_of_kind(&r.kind, rules),
    }
}

/// A model element's kind, resolved ([`super::validate_kind`]).
fn resolve_element(d: &Drawing, kinds: &Kinds, e: &v::Element) -> Resolved {
    let ent = cadaddy_doc::Handle::parse_hex(&e.id).and_then(|h| d.model.get(h)).map(|a| a.as_ref());
    kinds.resolve(&e.kind, e.mark.as_deref().or(e.tag.as_deref()), ent)
}

/// The trade an element kind belongs to: the first trade rule that applies, else by its words.
pub(crate) fn trade_of_kind(kind: &str, rules: &[Effective]) -> &'static str {
    if let Some(r) =
        rules.iter().find(|r| r.rule.trade != "general" && r.rule.applies_to.kinds.iter().all(|k| k != "*") && r.rule.applies_to_kind(kind))
    {
        return vr::TRADES.iter().find(|t| **t == r.rule.trade).copied().unwrap_or("general");
    }
    let k = format!("_{}_", kind.to_ascii_lowercase().replace([' ', '-', '.'], "_"));
    let has = |w: &[&str]| w.iter().any(|x| k.contains(&format!("_{x}_")) || k.contains(x));
    if has(&["door_holder"]) {
        // A fire alarm device on the door, not the door.
        "electrical"
    } else if has(&["wall", "door", "window", "room", "ceiling", "floor", "stair", "column", "roof"]) {
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
    /// What it is, resolved ([`super::validate_kind`]).
    kind: String,
    /// As the build tags it, when that differs (`device`, `fixture`).
    kind_read: Option<String>,
    kind_from: &'static str,
    category: Option<String>,
    mark: Option<String>,
    /// Its plan box, building feet.
    rect: [f64; 4],
    /// Its bottom and top above the floor, inches.
    z_in: Option<[f64; 2]>,
    /// The plan object it was built from.
    from: Option<String>,
}

/// The build's MEP parts on `area`'s level within `reach` feet of it: the nearest ones when
/// there are more than [`MAX_BUILT`] (a dense floor's raceway pieces), so a part in the area is
/// never left out for one farther off that came first in the drawing.
fn built_parts(d: &Drawing, sp: &BuildingSpec, frame: &Frame, area: &Area, reach: f64, kinds: &Kinds) -> Vec<BuiltPart> {
    const MEP_KINDS: [&str; 8] = ["duct", "pipe", "conduit", "equipment", "fixture", "device", "light_fixture", "plumbing_fixture"];
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
        let lo = frame.built_on(&area.level, e, b.min);
        let hi = frame.built_on(&area.level, e, b.max);
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
        let mark = super::validate_kind::mark_of(e);
        let r = kinds.resolve(&kind, mark.as_deref(), Some(e));
        out.push(BuiltPart {
            handle: e.handle.hex(),
            kind_read: (r.kind != kind).then_some(kind),
            kind: r.kind,
            kind_from: r.from,
            category: r.category,
            mark,
            rect,
            z_in,
            from: super::bimlook::made_source(e).map(|h| h.hex()),
        });
        if out.len() >= MAX_ENTITIES {
            break;
        }
    }
    if out.len() > MAX_BUILT {
        let off = |p: &BuiltPart| {
            let dx = (area.bbox[0] - p.rect[2]).max(p.rect[0] - area.bbox[2]).max(0.0);
            let dy = (area.bbox[1] - p.rect[3]).max(p.rect[1] - area.bbox[3]).max(0.0);
            dx.hypot(dy)
        };
        out.sort_by(|a, b| off(a).total_cmp(&off(b)).then(a.handle.cmp(&b.handle)));
        out.truncate(MAX_BUILT);
    }
    out.sort_by(|a, b| (a.kind.as_str(), a.handle.as_str()).cmp(&(b.kind.as_str(), b.handle.as_str())));
    out
}

/// The build's parts a packet looks at round its area, at most (the nearest kept).
const MAX_BUILT: usize = 4 * MAX_OBJECTS;

/// One wall of a packet: a built wall's pieces along one line (its openings split it) as one,
/// from end to end.
struct WholeWall<'a> {
    of: &'a v::ModelWall,
    a: [f64; 2],
    b: [f64; 2],
    pieces: usize,
}

impl WholeWall<'_> {
    /// Its handle, type, thickness, centreline and both face lines (each named by the way it
    /// faces).
    fn to_json(&self) -> Value {
        let w = self.of;
        let (dx, dy) = (self.b[0] - self.a[0], self.b[1] - self.a[1]);
        let len = dx.hypot(dy);
        let half = (w.thickness_in / 24.0).max(0.0);
        let n = if len > 1e-9 { [-dy / len * half, dx / len * half] } else { [0.0, 0.0] };
        let face = |k: f64| json!({"face": compass(n[0] * k, n[1] * k), "a_ft": pt([self.a[0] + n[0] * k, self.a[1] + n[1] * k]), "b_ft": pt([self.b[0] + n[0] * k, self.b[1] + n[1] * k])});
        json!({
            "handle": w.id, "type": w.wall_type, "thickness_in": r2(w.thickness_in), "stud_in": w.stud_in.map(r2),
            "a_ft": pt(self.a), "b_ft": pt(self.b), "length_ft": r2(len), "faces": [face(1.0), face(-1.0)], "pieces": self.pieces,
        })
    }
}

/// `pieces` as whole walls: the pieces of one wall (one handle, as thick) on one line are one
/// wall from its first end to its last; walls in handle order.
fn whole_walls<'a>(pieces: &[&'a v::ModelWall]) -> Vec<WholeWall<'a>> {
    const ON_LINE_FT: f64 = 1.0 / 12.0;
    let mut ps: Vec<&v::ModelWall> = pieces.to_vec();
    ps.sort_by(|a, b| a.id.cmp(&b.id).then(a.a[0].total_cmp(&b.a[0])).then(a.a[1].total_cmp(&b.a[1])));
    let mut out: Vec<WholeWall> = Vec::new();
    for p in ps {
        let joined = out.iter_mut().rev().take_while(|w| w.of.id == p.id).find(|w| {
            let (dx, dy) = (w.b[0] - w.a[0], w.b[1] - w.a[1]);
            let len = dx.hypot(dy);
            if len <= 1e-9 || (w.of.thickness_in - p.thickness_in).abs() > 0.5 {
                return false;
            }
            let off = |q: [f64; 2]| ((q[0] - w.a[0]) * dy - (q[1] - w.a[1]) * dx).abs() / len;
            off(p.a) <= ON_LINE_FT && off(p.b) <= ON_LINE_FT
        });
        match joined {
            Some(w) => {
                let (dx, dy) = (w.b[0] - w.a[0], w.b[1] - w.a[1]);
                let len2 = dx * dx + dy * dy;
                let s = |q: [f64; 2]| ((q[0] - w.a[0]) * dx + (q[1] - w.a[1]) * dy) / len2;
                let (s0, s1) = [s(p.a), s(p.b)].iter().fold((0.0_f64, 1.0_f64), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
                let a0 = w.a;
                w.a = [a0[0] + dx * s0, a0[1] + dy * s0];
                w.b = [a0[0] + dx * s1, a0[1] + dy * s1];
                w.pieces += 1;
            }
            None => out.push(WholeWall { of: p, a: p.a, b: p.b, pieces: 1 }),
        }
    }
    out
}

/// Which area of a level a point belongs to: the first whose outline holds it, else the one
/// whose outline is nearest, within [`EDGE_FT`] (an item on its room's wall line).
pub(crate) fn home_of(areas: &[&Area], p: [f64; 2]) -> Option<usize> {
    if let Some(i) = areas.iter().position(|a| a.outline.len() >= 3 && point_in(&a.outline, p)) {
        return Some(i);
    }
    areas
        .iter()
        .enumerate()
        .filter_map(|(i, a)| {
            let d = a
                .outline
                .iter()
                .zip(a.outline.iter().cycle().skip(1))
                .take(MAX_EDGES)
                .map(|(x, y)| seg_dist(p, *x, *y).0)
                .fold(f64::INFINITY, f64::min);
            (d <= EDGE_FT).then_some((i, d))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)))
        .map(|(i, _)| i)
}

/// Outline edges looked at per area, at most.
const MAX_EDGES: usize = 10_000;

/// Whose a point near an area is: the area's own (inside its outline, or on its wall line and
/// no other room's), or another room's (with its label), or no room's.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Whose {
    Own,
    Other { area: String, room: Option<String> },
    NoRoom,
}

/// The areas of the level an area is on, the area among them, for [`Membership::whose`].
pub(crate) struct Membership<'a> {
    area: &'a Area,
    level: Vec<&'a Area>,
}

impl<'a> Membership<'a> {
    pub(crate) fn new(area: &'a Area, all: &'a [Area]) -> Membership<'a> {
        let mut level: Vec<&Area> = all.iter().filter(|a| a.level.eq_ignore_ascii_case(&area.level)).collect();
        if !level.iter().any(|a| a.id == area.id) {
            level.push(area);
        }
        Membership { area, level }
    }

    pub(crate) fn whose(&self, p: [f64; 2]) -> Whose {
        // A custom window holds what is inside it.
        if self.area.room.is_none() {
            return if inside(self.area.bbox, p) { Whose::Own } else { Whose::NoRoom };
        }
        match home_of(&self.level, p).and_then(|i| self.level.get(i)) {
            Some(a) if a.id == self.area.id => Whose::Own,
            Some(a) => Whose::Other {
                area: a.id.clone(),
                room: match (&a.number, &a.name) {
                    (Some(n), Some(m)) => Some(format!("{n} {m}")),
                    (Some(n), None) => Some(n.clone()),
                    (None, Some(m)) => Some(m.clone()),
                    _ => None,
                },
            },
            None => Whose::NoRoom,
        }
    }
}

/// The room of one of the area's own elements at `at` and what said it (Q6263): the area's when
/// the point is inside its outline (`outline`) or, for a room's area, within [`EDGE_FT`] of it and
/// in no other room (`wall_line`: [`Membership::whose`] made it the area's; with how far outside
/// the outline it is, feet), else the element's own record (`record`).
fn own_room(area: &Area, at: [f64; 2], record: Option<String>) -> (Option<String>, Value) {
    let label = area.number.clone().or_else(|| area.name.clone());
    match (label, point_in(&area.outline, at), area.room.is_some()) {
        (Some(r), true, _) => (Some(r), json!("outline")),
        (Some(r), false, true) => {
            let off = area
                .outline
                .iter()
                .zip(area.outline.iter().cycle().skip(1))
                .take(MAX_EDGES)
                .map(|(x, y)| seg_dist(at, *x, *y).0)
                .fold(f64::INFINITY, f64::min);
            (Some(r), json!({"wall_line": true, "outside_outline_ft": if off.is_finite() { json!(r2(off)) } else { Value::Null }}))
        }
        _ => match record {
            Some(r) => (Some(r), json!("record")),
            None => (None, Value::Null),
        },
    }
}

/// An element beside the area, for `objects.json` `nearby`.
fn nearby_json(handle: &str, r: &Resolved, mark: Option<&str>, at: [f64; 2], whose: &Whose) -> Value {
    let (area, room) = match whose {
        Whose::Other { area, room } => (Some(area.as_str()), room.as_deref()),
        _ => (None, None),
    };
    json!({
        "handle": handle,
        "kind": r.kind,
        "kind_from": r.from,
        "category": r.category,
        "mark": mark,
        "at_ft": pt(at),
        "room": room,
        "area": area,
        "why": match room.or(area) {
            Some(x) => format!("outside this area's outline, in {x}: judged there, not here"),
            None => "outside this area's outline and every room's: not judged here".into(),
        },
    })
}

/// What the packet knows of the model in the area.
struct InArea {
    objects: Value,
    kinds: BTreeSet<String>,
    /// The BIM categories (IFC classes) of what is in the area.
    categories: BTreeSet<String>,
    marks: BTreeSet<String>,
    wall_types: BTreeSet<String>,
    counts: Value,
    /// The handles of the elements beside the area (not its own).
    nearby: BTreeSet<String>,
    /// The sheets the area's elements were read from (sheet number, upper case): how many
    /// elements cite each, and the points they were read at.
    cited: BTreeMap<String, Cited>,
    /// The area's own elements: kind, mark, submittal (for the submittals' matching).
    products: Vec<(String, Option<String>, Option<String>)>,
    /// The area holds luminaires or lighting controls: its packet carries the lighting-control
    /// legend.
    lighting: bool,
    /// The lighting types' cut-sheet pages (`<file> p.<n>`) the packet carries.
    cut_sheets: BTreeSet<String>,
}

/// The elements of an area read from one sheet.
#[derive(Clone, Debug, Default)]
struct Cited {
    elements: usize,
    points: Vec<[f64; 2]>,
}

/// The sheet an element's print source names (`E-9/1`: sheet E-9, its first view or page), upper
/// case; none for what is not a sheet number (`room 512`).
pub(crate) fn sheet_key(doc: &str) -> Option<String> {
    let t = doc.trim();
    let t = match t.rsplit_once('/') {
        Some((a, b)) if !b.is_empty() && b.chars().all(|c| c.is_ascii_digit()) => a.trim(),
        _ => t,
    };
    let ok = !t.is_empty() && t.len() <= 24 && !t.contains(char::is_whitespace) && t.chars().any(|c| c.is_ascii_digit());
    ok.then(|| t.to_ascii_uppercase())
}

/// Note an element's print source in `cited`.
fn cite(cited: &mut BTreeMap<String, Cited>, src: Option<&Value>) {
    let Some(key) = src.and_then(|v| v.get("sheet")).and_then(Value::as_str).and_then(sheet_key) else { return };
    if cited.len() >= MAX_CITED_SHEETS && !cited.contains_key(&key) {
        return;
    }
    let c = cited.entry(key).or_default();
    c.elements += 1;
    let at = src.and_then(|v| v.get("at")).and_then(Value::as_array).and_then(|a| Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?]));
    if let Some(p) = at.filter(|p| p.iter().all(|v| v.is_finite()))
        && c.points.len() < MAX_CITED_POINTS
    {
        c.points.push(p);
    }
}

#[allow(clippy::too_many_arguments)]
fn in_area(
    d: &Drawing,
    model: &Model,
    prints: Option<&Prints>,
    area: &Area,
    rules: &[Effective],
    built: &[BuiltPart],
    kinds_of: &Kinds,
    member: &Membership,
) -> InArea {
    let upf = super::elemprops::units_per_foot(d);
    let reach = grow(area.bbox, EDGE_FT);
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
    let mut categories = BTreeSet::new();
    let mut marks = BTreeSet::new();
    let mut elements: Vec<Value> = Vec::new();
    let mut nearby: Vec<Value> = Vec::new();
    let mut nearby_handles: BTreeSet<String> = BTreeSet::new();
    let mut cited: BTreeMap<String, Cited> = BTreeMap::new();
    let mut products: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    let mut els: Vec<&v::Element> = model.elements.iter().filter(|e| lvl(&e.level) && inside(reach, e.at)).collect();
    els.sort_by(|a, b| (a.kind.as_str(), a.id.as_str()).cmp(&(b.kind.as_str(), b.id.as_str())));
    // The area's own: inside its outline (or on its wall line, no other room's); the rest beside it.
    let (els, beside): (Vec<&v::Element>, Vec<&v::Element>) = els.into_iter().partition(|e| member.whose(e.at) == Whose::Own);
    for e in beside.into_iter().take(MAX_OBJECTS) {
        let rk = resolve_element(d, kinds_of, e);
        nearby_handles.insert(e.id.to_ascii_uppercase());
        nearby.push(nearby_json(&e.id, &rk, e.mark.as_deref(), e.at, &member.whose(e.at)));
    }
    let total_elements = els.len();
    for e in els.into_iter().take(MAX_OBJECTS) {
        let rk = resolve_element(d, kinds_of, e);
        kinds.insert(rk.kind.clone());
        if let Some(c) = &rk.category {
            categories.insert(c.clone());
        }
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
        cite(&mut cited, print_src.as_ref());
        products.push((rk.kind.clone(), e.mark.clone(), sub.clone()));
        let mounting = field(&props, &e.fields, &["mounting"])
            .unwrap_or_else(|| serde_json::to_value(e.mounting).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default());
        // The wall it is on: its stored host wall's nearest piece when the model has that wall,
        // else the nearest wall of the level within reach of its centreline (a piece over an
        // opening says so, with the heights it spans).
        let host = if e.mounting == Mounting::Wall || e.mounting == Mounting::Unknown {
            let pieces: Vec<(&v::ModelWall, Option<(f64, f64)>)> = model
                .walls
                .iter()
                .map(|w| (w, None))
                .chain(model.wall_spans.iter().map(|s| (&s.wall, Some((s.bottom_in, s.top_in)))))
                .filter(|(w, _)| lvl(&w.level))
                .collect();
            let stored = ent.and_then(|x| super::validate::stored_host(x));
            let mine: Vec<(&v::ModelWall, Option<(f64, f64)>)> =
                stored.as_deref().map(|h| pieces.iter().copied().filter(|(w, _)| w.id.eq_ignore_ascii_case(h)).collect()).unwrap_or_default();
            let (from, pieces) = if mine.is_empty() { ("nearest", pieces) } else { ("stored", mine) };
            pieces
                .into_iter()
                .map(|(w, band)| (seg_dist(e.at, w.a, w.b), w, band))
                .filter(|((dist, _), w, _)| from == "stored" || *dist <= HOST_REACH_FT + w.thickness_in / 24.0)
                .min_by(|a, b| a.0.0.total_cmp(&b.0.0).then(a.1.id.cmp(&b.1.id)))
                .map(|((dist, q), w, band)| {
                    let (nx, ny) = (e.at[0] - q[0], e.at[1] - q[1]);
                    let (nx, ny) = if nx.hypot(ny) > 1e-6 { (nx, ny) } else { (centre[0] - q[0], centre[1] - q[1]) };
                    let toward_area = nx * (centre[0] - q[0]) + ny * (centre[1] - q[1]) >= 0.0;
                    let mut j = json!({"wall": w.id, "type": w.wall_type, "off_centreline_in": r2(dist * 12.0), "face": compass(nx, ny), "faces_area": toward_area, "from": from});
                    if let Some((bottom, top)) = band {
                        j["over_opening"] = json!({"wall_from_aff_in": r2(bottom), "wall_to_aff_in": r2(top)});
                    }
                    j
                })
        } else {
            None
        };
        let (room, room_by) = own_room(area, e.at, field(&props, &e.fields, &["room", "room_number"]));
        let mine: Vec<Value> =
            failures.iter().filter(|f| f.element.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(&e.id))).map(fail_json).collect();
        elements.push(json!({
            "handle": e.id,
            "kind": rk.kind,
            "kind_read": (rk.kind != e.kind).then_some(&e.kind),
            "kind_from": rk.from,
            "category": rk.category,
            "trade": trade_of(&rk, rules),
            "mark": e.mark,
            "tag": e.tag,
            "name": field(&props, &e.fields, &["name", "description"]),
            "level": e.level,
            "room": room,
            "room_by": room_by,
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
    // A built part made from one of the plan's elements is that element's (its `built_as`); one
    // made from another built part (a box built for a built device) is listed itself, whatever
    // order the kinds sort in (Q6263).
    let plan_listed = elements.len();
    for b in built.iter().filter(|b| intersect(b.rect, reach).is_some() || inside(reach, [b.rect[0], b.rect[1]])) {
        if let Some(from) = &b.from
            && let Some(el) = elements.iter_mut().take(plan_listed).find(|x| x["handle"].as_str().is_some_and(|h| h.eq_ignore_ascii_case(from)))
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
        let c = [(b.rect[0] + b.rect[2]) / 2.0, (b.rect[1] + b.rect[3]) / 2.0];
        let whose = member.whose(c);
        if whose != Whose::Own {
            if nearby.len() < MAX_OBJECTS && nearby_handles.insert(b.handle.to_ascii_uppercase()) {
                let r = Resolved { kind: b.kind.clone(), from: b.kind_from, category: b.category.clone() };
                nearby.push(nearby_json(&b.handle, &r, b.mark.as_deref(), c, &whose));
            }
            continue;
        }
        built_listed += 1;
        kinds.insert(b.kind.clone());
        if let Some(c) = &b.category {
            categories.insert(c.clone());
        }
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
        let mark = b.mark.clone().or_else(|| field(&props, &none, &["mark", "type_mark", "type"]));
        if let Some(m) = &mark {
            marks.insert(m.clone());
        }
        let sub = field(&props, &none, &["submittal"]);
        cite(&mut cited, print_src.as_ref());
        products.push((b.kind.clone(), mark.clone(), sub.clone()));
        let mine: Vec<Value> =
            failures.iter().filter(|f| f.element.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(&b.handle))).map(fail_json).collect();
        let (room, room_by) = own_room(area, c, field(&props, &none, &["room"]));
        elements.push(json!({
            "handle": b.handle,
            "kind": b.kind,
            "kind_read": b.kind_read,
            "kind_from": b.kind_from,
            "category": b.category,
            "built": true,
            "trade": trade_of(&Resolved { kind: b.kind.clone(), from: b.kind_from, category: b.category.clone() }, rules),
            "mark": mark,
            "name": field(&props, &none, &["name", "description"]),
            "level": area.level,
            "room": room,
            "room_by": room_by,
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
    let walls_json: Vec<Value> = whole_walls(&walls)
        .iter()
        .map(|w| {
            if let Some(t) = &w.of.wall_type {
                wall_types.insert(t.clone());
            }
            w.to_json()
        })
        .collect();
    if !walls_json.is_empty() {
        kinds.insert("wall".into());
    }
    // The pieces of wall over an opening (the plan shows a gap there) and the walls that can't
    // be measured, so an element on one is never judged against nothing.
    let mut spans: Vec<&v::WallSpan> =
        model.wall_spans.iter().filter(|s| lvl(&s.wall.level) && segment_touches(reach, s.wall.a, s.wall.b)).take(MAX_OBJECTS).collect();
    spans.sort_by(|a, b| a.wall.id.cmp(&b.wall.id).then(a.wall.a[0].total_cmp(&b.wall.a[0])).then(a.wall.a[1].total_cmp(&b.wall.a[1])));
    let spans_json: Vec<Value> = spans
        .iter()
        .map(|s| {
            json!({"handle": s.wall.id, "type": s.wall.wall_type, "a_ft": pt(s.wall.a), "b_ft": pt(s.wall.b), "thickness_in": r2(s.wall.thickness_in), "from_aff_in": r2(s.bottom_in), "to_aff_in": r2(s.top_in), "why": "over an opening"})
        })
        .collect();
    let unmeasured_json: Vec<Value> = {
        let ids: BTreeSet<&str> = walls.iter().map(|w| w.id.as_str()).chain(spans.iter().map(|s| s.wall.id.as_str())).collect();
        model
            .unmeasured_walls
            .iter()
            .filter(|u| lvl(&u.level) && inside(reach, u.at) && !ids.contains(u.id.as_str()))
            .take(MAX_OBJECTS)
            .map(|u| json!({"handle": u.id, "at_ft": pt(u.at), "why": u.why}))
            .collect()
    };
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
    let counts = json!({"elements": total_elements + built_listed, "nearby": nearby.len(), "runs": total_runs, "walls": walls_json.len(), "wall_spans": spans_json.len(), "walls_unmeasured": unmeasured_json.len(), "openings": openings_json.len(), "failures": failures.len()});
    let mut objects = json!({
        "area": area.id,
        "level": area.level,
        "units": "positions in building feet on the level's plan; heights in inches above the floor",
        "prints_known": prints.is_some(),
        "elements": elements,
        "nearby": nearby,
        "nearby_about": "elements just outside this area's outline (in the margin), each with the room it is in: not this area's, never judged here",
        "runs": runs_json,
        "walls": walls_json,
        "wall_spans": spans_json,
        "walls_unmeasured": unmeasured_json,
        "openings": openings_json,
        "failures_without_element": loose,
        "counts": counts,
    });
    if total_elements > MAX_OBJECTS || total_runs > MAX_OBJECTS {
        objects["left_out"] = json!(format!("listed at most {MAX_OBJECTS} elements and runs each"));
    }
    InArea {
        objects,
        kinds,
        categories,
        marks,
        wall_types,
        counts,
        nearby: nearby_handles,
        cited,
        products,
        lighting: false,
        cut_sheets: BTreeSet::new(),
    }
}

// ---------------------------------------------------------------- rules and prompt

/// The rules that apply to what is in the area, by its kinds, BIM categories and marks: the
/// ones in use, and the ones the project turned off.
fn rules_for<'a>(here: &InArea, all: &'a [Effective]) -> (Vec<&'a Effective>, Vec<String>) {
    let applies = |r: &Effective| {
        here.kinds.iter().any(|k| r.rule.applies_to_kind(k))
            || here.categories.iter().any(|c| r.rule.applies_to_element("", Some(c), None))
            || here.marks.iter().any(|m| r.rule.applies_to_element("", None, Some(m)))
    };
    let on: Vec<&Effective> = all.iter().filter(|r| r.enabled && applies(r)).collect();
    let off: Vec<String> = all.iter().filter(|r| !r.enabled && applies(r)).map(|r| r.rule.id.clone()).collect();
    (on, off)
}

/// Write each element's and run's `rules` in `objects` (the rules of `rules` that ask about it,
/// [`vr::Rule::asks`], with the prescreen's findings naming it) and count the pairs per rule.
pub(crate) fn ask_rules(objects: &mut Value, rules: &[&Effective]) -> BTreeMap<String, usize> {
    let named = vr::named_by(objects.pointer("/prescreen/findings").and_then(Value::as_array).into_iter().flatten());
    let mut asked: BTreeMap<String, usize> = rules.iter().map(|r| (r.rule.id.clone(), 0)).collect();
    for k in ["elements", "runs"] {
        let Some(list) = objects.get_mut(k).and_then(Value::as_array_mut) else { continue };
        for e in list.iter_mut() {
            let subject = vr::Subject::of_json(e, true, &named);
            let ids: Vec<&str> = rules.iter().filter(|r| r.rule.asks(&subject)).map(|r| r.rule.id.as_str()).collect();
            for id in &ids {
                if let Some(n) = asked.get_mut(*id) {
                    *n += 1;
                }
            }
            e["rules"] = json!(ids);
        }
    }
    asked
}

/// Does a packet keep a rule: any rule but one asked only where its prescreen names an element
/// (`only_if` over every kind) that asks about none here.
fn vr_keeps(r: &Effective, asked: &BTreeMap<String, usize>) -> bool {
    let everywhere = r.rule.applies_to.only_if.as_ref().is_some_and(|o| o.kinds.iter().any(|k| k == "*"));
    !everywhere || asked.get(&r.rule.id).is_some_and(|n| *n > 0)
}

/// The kind of document a crop is, as the rules name the documents they need (a gear's panel
/// schedule is a schedule, a keynote list notes, a unit type's wall a room's interior elevation).
fn doc_kind(kind: &str) -> &str {
    match kind {
        "panel_schedule" | "feeder_schedule" => "schedule",
        "keynotes" => "general_notes",
        "gear_riser" => "riser_diagram",
        "elevation" => "interior_elevation",
        k => k,
    }
}

/// The pictures a rule's "Look at" names, at most.
const MAX_LOOK_AT: usize = 24;

fn prompt_md(
    area: &Area,
    crops: &[Crop],
    model_imgs: &[ModelImage],
    rules: &[&Effective],
    counts: &Value,
    pages: &[(String, &'static str)],
) -> String {
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
    s.push_str("- `objects.json` `prescreen`: what was measured without you (G-HOST: each wall item's distance to its wall face; G-HEIGHT: its height above the floor against the documents, each element's `height_expected` naming the sheet, point and words that gave it, `default` when its model height is a default no document gives; G-PRESENT: each symbol the room's enlarged plans, else its level's trade plans, show in it against the model's elements of the same kind (`missing` names the sheet and the point), elements no symbol stands for `extra`, and the room type's project-rule set; E-RECEPT-TR: each receptacle's tamper-resistance fact and its source where the code wants tamper-resistant receptacles, `unsure` when nothing says; E-LUM-TYPE, E-LUM-CONTROL, E-SWITCH-LEG, E-SWITCH-DIM, E-SWITCH-3WAY, E-LUM-COUNT: the luminaires' types against the fixture schedule and submittal, their controls by switch leg, dimmers against their drivers, 3-way pairs, and the plan's symbol counts per room; G-PRODUCT: each product's record field by field (maker, catalog, colour, configuration, rating, features) against its submittal and specification section, `unsure` when the model has no product data; G-PRODUCT-CODE: each receptacle's code-required features where it is: tamper-resistant, GFCI judged on its circuit with the breaker's schedule row, or the project's as-built breaker rule where it outranks the schedule, weather-resistant). Start from these numbers; overturn one only with a print image that shows otherwise. A `conflict` finding means the documents disagree (both are cited): report that element `unsure`, citing both documents, unless a print image settles it.\n- `objects.json` `lighting`: each luminaire type present with its fixture schedule rows and submittal product (its cut-sheet pages are among the submittal pictures), and the legend rows of the controls present; each luminaire and control carries its `lighting` facts (switch leg, dimming, dimmer, 3-way).\n- `objects.json` elements: a product's `product_record` (what the specification requires, what the submittal approved, what it resolves to) and its `product_checks` (G-PRODUCT, G-PRODUCT-CODE); the specification pages the records cite are among the pictures (`manifest.json` `specs`).\n");
    s.push_str("- `objects.json` `wall_spans`: wall pieces over an opening (a door's header), with the heights they span above the floor; the plan shows a gap there, and an item below one is in the opening. `walls_unmeasured`: walls whose footprint gives no centreline, with why.\n");
    if counts.get("raceways").and_then(Value::as_u64).unwrap_or(0) > 0 {
        s.push_str("- `objects.json` `raceways`: the routed raceway runs of this area (circuit, panel, raceway, sticks, length, the conductors pulled in it with each wire's handle); each raceway piece among the elements carries its `raceway` facts: a stick's `ends_ft` and `ends_aff_in`, what each end `touches` (boxes, the next stick or fitting) and its `links`, the `wires` in it; a box's `cubic_in`, `box_of` and the sticks `entering` it; a wire's `in_raceway`; a fitting's `touches`. The conduit rules in `prescreen` (E-CONDUIT-CONTINUITY, E-CONDUIT-FILL, E-BOX-FILL, E-DERATE, E-VDROP, E-METHOD, G-CLASH, E-TAKEOFF) were measured on these pieces; each finding's `handles` lists every piece of this area it covers: report it once with those `handles`, and overturn it only with evidence from the pieces or the prints.\n");
    }
    if counts.get("keynote_markers").and_then(Value::as_u64).unwrap_or(0) > 0 {
        s.push_str("- `objects.json` `keynotes`: the keynote lists of the sheets that mark this area (number → note, and what each note calls for: a box of a size, a device, conduit of a size and route, a sleeve, a fire rating) and each keynote marker in or by it, with its crop (`keynote_marker` crops; the `keynotes` crops are the lists). G-KEYNOTE in `prescreen` matched each call to the model at its marker; check what it could not measure on the crops.\n");
    }
    if counts.get("underground_callouts").and_then(Value::as_u64).unwrap_or(0) > 0
        || counts.get("underground_sheets").and_then(Value::as_u64).unwrap_or(0) > 0
    {
        s.push_str("- `objects.json` `underground`: the documents' callouts for work below grade or under the slab (what each calls for: trade, count, size, material, least depth, what it serves), this level's underground sheets (the set's own, and the ones CADaddy generated: whole, as `underground_sheet` prints, their run and stub-up schedules down the side) and the model's buried runs and stub-ups. G-UNDERGROUND in `prescreen` matched each callout to a run and read each stub-up's dimensions off the schedule; dashed PROPOSED runs are CADaddy's, not the documents'.\n");
    }
    if counts.get("gear").and_then(Value::as_u64).unwrap_or(0) > 0 {
        s.push_str("- `objects.json` `gear`: the electrical gear here — its box as built and as the gear submittal gives it (re-read), its working space and placement (`sources.status` proposed: CADaddy laid it out, the plans don't locate it), its lugs and enclosure, and every feeder that lands on it with its schedule row and one-line tag. The riser or one-line (`gear_riser`), the panel and feeder schedules (`panel_schedule`, `feeder_schedule`) and the gear submittal pages are among the pictures. E-GEAR-SPACE, E-GEAR-FILL and E-GEAR-SIZE in `prescreen` recomputed the working space, the raceways on each side's conduit area, the gutter, bending space and lugs, and the box from these documents; check on the pictures what they mark unsure (missing gutter or bending dimensions, a door's swing, the condition of what faces the gear).\n");
    }
    if counts.get("nearby").and_then(Value::as_u64).unwrap_or(0) > 0 {
        s.push_str("- `objects.json` `nearby`: elements just outside this area's outline (the next room's, each with its room). They are not this area's: don't judge them here; a report naming one is refused.\n");
    }
    s.push_str("- `objects.json` elements and runs: each one's `rules` are the rules asked of it; report only those pairs (a rule for every element asks about a raceway piece only where a print draws it or a marker names it, a value-engineering rule only where an option does; `rules.json` `pairs_asked` counts them).\n");
    s.push_str("- `rules.json`: the rules below, in full.\n");
    s.push_str("- `manifest.json`: every picture with its sheet, view, and the sheet rectangle it shows.\n");
    s.push_str("- Print crops (what the documents say):\n");
    for c in crops {
        s.push_str(&format!(
            "  - `{}`: sheet {}{}, {}{}{}{}\n",
            c.file,
            c.sheet,
            c.view.as_ref().map_or_else(|| format!(" {}", c.title).trim_end().to_string(), |v| format!(" view {v}")),
            c.kind.replace('_', " "),
            c.shows.as_ref().map(|x| format!(" ({x})")).unwrap_or_default(),
            c.orientation.as_ref().and_then(turn_words).map(|w| format!("; {w}")).unwrap_or_default(),
            if c.unregistered { "; NOT placed on the plan: no overlay, find the area on it by its walls, doors and tags" } else { "" }
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
        let images: Vec<&str> = crops
            .iter()
            .filter(|c| {
                let k = doc_kind(c.kind);
                docs.contains(&k) || (k == "rcp" && docs.contains(&"plan"))
            })
            .map(|c| c.file.as_str())
            .chain(pages.iter().filter(|(_, k)| docs.contains(k)).map(|(f, _)| f.as_str()))
            .collect();
        if !images.is_empty() {
            let more = images.len().saturating_sub(MAX_LOOK_AT);
            s.push_str(&format!(
                "\nLook at: {}{}\n",
                images.iter().take(MAX_LOOK_AT).map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", "),
                if more > 0 { format!(" and {more} more of its documents (`manifest.json`)") } else { String::new() }
            ));
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
    s.push_str("- Every finding cites at least one picture of this packet's documents (`prints/…`: a plan, schedule, legend, notes, specification or submittal page) and says what it shows there.\n");
    s.push_str("- `missing`: the print shows it, the model has nothing (no handle). `extra`: the model has it, no print shows it.\n");
    s.push_str(
        "- Say UNSURE rather than guess: when the image is unreadable, the document is not in the packet, or the print and the notes disagree.\n",
    );
    s.push_str("- Check every rule above for every element it applies to; a pass is a finding too.\n");
    s.push_str("- Elements with the same verdict on the same evidence may share one finding: `\"handles\": [\"<handle>\", …]` (it counts once per handle). Send each rule and element once: a repeat is merged into the first.\n");
    s.push_str("- More than 1000 findings: report them in parts (`\"part\": 1`, `\"part\": 2`, …; the last one with `\"final\": true`); the area is done on the final part.\n");
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
    kinds: &Kinds,
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
        let col = trade_colour(trade_of(&Resolved { kind: b.kind.clone(), from: b.kind_from, category: b.category.clone() }, rules));
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
        let col = trade_colour(trade_of(&resolve_element(d, kinds, e), rules));
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

// ---------------------------------------------------------------- a print as its page shows it

/// The layer (off) the PDF import keeps lettering strokes on once it has read them as text, and
/// the text style of a reading no hidden text confirms (`cadaddy_io::pdf_read`; that crate
/// reaches this one only through the read hook, so a test checks the names agree).
const SHX_SOURCE_LAYER: &str = "PDF-SHX-SOURCE";
const OCR_TEXT_STYLE: &str = "PDF-SHX-OCR";
/// The import names a style after the PDF font its text was set in (`PDF-Arial`).
const PDF_STYLE_PREFIX: &str = "PDF-";
/// Under this, a crop is flagged as not looking like its PDF page.
const MIN_SIMILARITY: f64 = 0.6;
/// PDF pages read again per packet to compare its crops with.
const MAX_COMPARE_SHEETS: usize = 24;
/// A printed line counts as matched within this many pixels.
const INK_REACH_PX: i64 = 2;
/// A pixel this dark (0–255) or darker is ink.
const INK_LEVEL: u32 = 200;

fn on_layer(e: &Entity, name: &str) -> bool {
    e.common.layer.eq_ignore_ascii_case(name)
}

/// The drawing's tables for drawing a print: the lettering strokes the import read as text
/// shown again (the page shows them, not the reading).
fn print_tables(d: &Drawing) -> Drawing {
    let mut t = d.clone();
    for l in t.layers.iter_mut().filter(|l| l.name.eq_ignore_ascii_case(SHX_SOURCE_LAYER)) {
        l.on = true;
        l.frozen = false;
    }
    t
}

/// What of a print's objects its page shows. The lettering the import read off strokes is
/// drawn as those strokes (`PDF-SHX-SOURCE`), not as the reading: an unconfirmed reading
/// (`PDF-SHX-OCR`) is left out (its strokes stayed linework), and where the crop holds the kept
/// strokes, so is every text not set in a PDF font (the readings the hidden text confirmed,
/// often at a cluster's size, not the lettering's). Masks (wipeouts) are left out: on white
/// paper they only hide, and the import's merged masks sit later than what they covered
/// (a block of notes, half a plan). Objects on layers that are off or frozen stay out.
fn as_printed<'a>(d: &Drawing, ents: &[&'a Entity]) -> Vec<&'a Entity> {
    let strokes = ents.iter().any(|e| on_layer(e, SHX_SOURCE_LAYER));
    let shown = |e: &Entity| on_layer(e, SHX_SOURCE_LAYER) || d.layer(&e.common.layer).is_none_or(|l| l.visible());
    ents.iter()
        .copied()
        .filter(|e| shown(e))
        .filter(|e| match &e.kind {
            EntityKind::Wipeout(_) => false,
            EntityKind::Text(t) => {
                let style = t.style.to_ascii_uppercase();
                style != OCR_TEXT_STYLE && (!strokes || style.starts_with(PDF_STYLE_PREFIX))
            }
            _ => true,
        })
        .collect()
}

/// A picture's ink: one bit per pixel, row by row.
struct Ink {
    w: u32,
    h: u32,
    bits: Vec<u64>,
    count: usize,
}

impl Ink {
    /// From a picture's RGBA bytes (opaque: the page is white).
    fn of(w: u32, h: u32, rgba: &[u8]) -> Ink {
        let n = (w as usize).saturating_mul(h as usize).min(rgba.len() / 4);
        let mut bits = vec![0u64; n.div_ceil(64)];
        let mut count = 0usize;
        for (i, p) in rgba.chunks_exact(4).enumerate().take(n) {
            let lum = (u32::from(p[0]) * 3 + u32::from(p[1]) * 6 + u32::from(p[2])) / 10;
            if lum <= INK_LEVEL
                && let Some(b) = bits.get_mut(i / 64)
            {
                *b |= 1 << (i % 64);
                count += 1;
            }
        }
        Ink { w, h, bits, count }
    }
    fn at(&self, x: i64, y: i64) -> bool {
        if x < 0 || y < 0 || x >= i64::from(self.w) || y >= i64::from(self.h) {
            return false;
        }
        let i = (y as usize) * (self.w as usize) + x as usize;
        self.bits.get(i / 64).is_some_and(|b| b & (1 << (i % 64)) != 0)
    }
    fn near(&self, x: i64, y: i64) -> bool {
        (-INK_REACH_PX..=INK_REACH_PX).any(|dy| (-INK_REACH_PX..=INK_REACH_PX).any(|dx| self.at(x + dx, y + dy)))
    }
    /// The share of this picture's ink that `other` has within reach.
    fn found_in(&self, other: &Ink) -> f64 {
        if self.count == 0 {
            return 1.0;
        }
        let mut hit = 0usize;
        for (k, b) in self.bits.iter().enumerate() {
            let mut v = *b;
            while v != 0 {
                let i = k * 64 + v.trailing_zeros() as usize;
                v &= v - 1;
                let (x, y) = ((i % self.w.max(1) as usize) as i64, (i / self.w.max(1) as usize) as i64);
                if other.near(x, y) {
                    hit += 1;
                }
            }
        }
        hit as f64 / self.count as f64
    }
}

/// How alike two pictures of one rectangle are, 0–1: the harmonic mean of the share of each
/// one's ink the other has within [`INK_REACH_PX`] (both blank: 1).
fn similarity(a: &Ink, b: &Ink) -> f64 {
    if a.count == 0 && b.count == 0 {
        return 1.0;
    }
    if a.count == 0 || b.count == 0 {
        return 0.0;
    }
    let (p, r) = (a.found_in(b), b.found_in(a));
    if p + r <= 0.0 { 0.0 } else { 2.0 * p * r / (p + r) }
}

/// `ents` drawn over `rect` at `px` (as [`png_of`]): the PNG, and its ink.
fn png_and_ink(d: &Drawing, ents: &[&Entity], rect: [f64; 4], px: [u32; 2]) -> Option<(Vec<u8>, Ink)> {
    let list = cadaddy_render::build_entities(d, ents.iter().copied(), &cadaddy_render::Options::default());
    let ro = cadaddy_render::raster::RasterOptions { background: cadaddy_color::Rgb(255, 255, 255), hairline: 1.2, px_per_mm: 0.0, antialias: true };
    let pm = cadaddy_render::raster::render(&list, &view_of(rect, px), &ro)?;
    let ink = Ink::of(pm.width(), pm.height(), pm.data());
    Some((pm.encode_png().ok()?, ink))
}

/// Where a sheet's PDF page is and where the job put it in the drawing.
#[derive(Clone, Debug)]
struct PageRef {
    path: std::path::PathBuf,
    file: String,
    page: u64,
    scale: Option<f64>,
    region: [f64; 4],
}

/// Each imported sheet's page (by sheet number, upper case), from the job: its classified
/// sheets (file, page, scale) and where each landed (`sheet_regions`). `prints` overrides the
/// folder the job read them from.
fn page_refs(job: &super::guided::GuidedJob, prints: Option<&str>) -> BTreeMap<String, std::result::Result<PageRef, String>> {
    let mut out = BTreeMap::new();
    let entries: Vec<&Value> =
        job.sheets.as_ref().and_then(|v| v.get("sheets")).and_then(Value::as_array).into_iter().flatten().take(MAX_FOLDER_FILES).collect();
    let folder = prints.map(str::to_string).unwrap_or_else(|| job.pdfs_path.clone());
    let dir = local_path(&folder);
    let dir = if dir.is_file() { dir.parent().map(std::path::Path::to_path_buf).unwrap_or_default() } else { dir };
    for r in job.sheet_regions.iter().take(MAX_FOLDER_FILES) {
        let key = r.number.trim().to_ascii_uppercase();
        if key.is_empty() || out.contains_key(&key) || !r.rect.iter().all(|v| v.is_finite()) {
            continue;
        }
        let current = |sh: &&&Value| sh.get("superseded_by").and_then(Value::as_str).is_none_or(str::is_empty);
        // A sheet printing several views is classified once per view (`P-301/2`).
        let is_it = |n: &str| n.split('/').next().is_some_and(|x| x.trim().eq_ignore_ascii_case(&key));
        let Some(sh) = entries.iter().filter(current).find(|sh| sh.get("number").and_then(Value::as_str).is_some_and(is_it)) else {
            continue;
        };
        let file = sh.get("file").and_then(Value::as_str).unwrap_or_default().to_string();
        let page = sh.get("page").and_then(Value::as_u64).filter(|p| *p >= 1).unwrap_or(1);
        let scale = sh.get("scale").and_then(Value::as_f64).filter(|k| k.is_finite() && *k > 1.0);
        let named = local_path(&file);
        let path = if named.is_absolute() && named.is_file() { named } else { dir.join(last_part(&file)) };
        let got = if file.is_empty() {
            Err("the job keeps no PDF file for it".to_string())
        } else if path.is_file() {
            Ok(PageRef { path, file: last_part(&file), page, scale, region: norm_rect(r.rect) })
        } else {
            Err(format!("its PDF ({}) isn't on this machine: give the prints folder as `prints`", last_part(&file)))
        };
        out.insert(key, got);
    }
    out
}

/// A sheet's PDF page read again as the page shows it (no text recovery, no masks, no
/// images), and the map from the drawing's sheet rectangle onto it.
struct PageRead {
    d: Drawing,
    /// drawing point → page point: `[sx, sy, x0, y0, px0, py0]`, `p = (q − q0)·s + p0`.
    map: [f64; 6],
}

fn read_page(r: &PageRef) -> std::result::Result<PageRead, String> {
    let hook = super::file::io().and_then(|h| h.read_pdf).ok_or_else(|| "PDF pages can't be read in this build".to_string())?;
    let bytes = crate::readfile::read(&r.path).map_err(|e| e.to_string())?;
    if bytes.len() > 200 * 1024 * 1024 {
        return Err("the file is over 200 MB".into());
    }
    let mut q = json!({"page": r.page, "images": false, "masks": false, "recoverText": false});
    if let Some(k) = r.scale {
        q["scale"] = json!(k);
    }
    let (d, rep) = hook(&bytes, &q)?;
    let sheet = |i: usize| rep.get("sheet").and_then(|v| v.get(i)).and_then(Value::as_f64).filter(|x| x.is_finite() && *x > 0.0);
    let k = rep.get("scale").and_then(Value::as_f64).filter(|k| k.is_finite() && *k > 0.0).unwrap_or(1.0);
    let (Some(w), Some(h)) = (sheet(0), sheet(1)) else { return Err("the page has no size".into()) };
    let (rw, rh) = (r.region[2] - r.region[0], r.region[3] - r.region[1]);
    if rw <= 0.0 || rh <= 0.0 {
        return Err("the sheet has no size in the drawing".into());
    }
    Ok(PageRead { d, map: [w * k / rw, h * k / rh, r.region[0], r.region[1], 0.0, 0.0] })
}

impl PageRead {
    fn rect(&self, r: [f64; 4]) -> [f64; 4] {
        let [sx, sy, x0, y0, px, py] = self.map;
        [(r[0] - x0) * sx + px, (r[1] - y0) * sy + py, (r[2] - x0) * sx + px, (r[3] - y0) * sy + py]
    }
    fn ink(&self, rect: [f64; 4], px: [u32; 2]) -> Option<Ink> {
        let r = self.rect(rect);
        let b = Bounds2::new(Vec2::new(r[0], r[1]), Vec2::new(r[2], r[3]));
        let ents: Vec<&Entity> = self
            .d
            .model
            .iter()
            .map(|e| e.as_ref())
            .filter(|e| b.intersects(&cadaddy_doc::entity_bounds(&self.d, e, 0)))
            .take(MAX_CROP_ENTITIES)
            .collect();
        png_and_ink(&self.d, &ents, r, px).map(|(_, ink)| ink)
    }
}

/// The view a picture of `rect` on `sheet` is of: of the views the spec and the unit types
/// place on that sheet, the one it overlaps most (its centre nearest on a tie).
fn view_at(views: &[(String, String, [f64; 4])], sheet: &str, rect: [f64; 4]) -> Option<String> {
    let c = [(rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0];
    views
        .iter()
        .filter(|(s, ..)| s.eq_ignore_ascii_case(sheet.trim()))
        .filter_map(|(_, t, r)| {
            let i = intersect(rect, *r)?;
            let area = (i[2] - i[0]) * (i[3] - i[1]);
            let d = ((r[0] + r[2]) / 2.0 - c[0]).hypot((r[1] + r[3]) / 2.0 - c[1]);
            (area > 0.0).then_some((area, d, t))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1.total_cmp(&a.1)).then(b.2.cmp(a.2)))
        .map(|(_, _, t)| t.clone())
}

/// Every view the spec's sheet links and its unit types place on a sheet: (sheet, title, rect).
fn sheet_views(d: &Drawing, sp: &BuildingSpec) -> Vec<(String, String, [f64; 4])> {
    let mut out: Vec<(String, String, [f64; 4])> =
        sp.sheets.iter().filter_map(|l| l.view.as_ref().map(|v| (l.number.trim().to_string(), v.title.clone(), norm_rect(v.rect)))).collect();
    let spec = d.building_spec.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok());
    for v in spec
        .as_ref()
        .and_then(|s| s.get("unit_types"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(1_000)
        .filter_map(|t| t.get("views").and_then(Value::as_array))
        .flatten()
        .take(MAX_FOLDER_FILES)
    {
        let r = v.get("rect").and_then(Value::as_array);
        let n = |i: usize| r.and_then(|r| r.get(i)).and_then(Value::as_f64).filter(|x| x.is_finite());
        if let (Some(s), Some(t), Some(a), Some(b), Some(c), Some(e)) =
            (v.get("sheet").and_then(Value::as_str), v.get("title").and_then(Value::as_str), n(0), n(1), n(2), n(3))
        {
            out.push((s.trim().to_string(), t.to_string(), norm_rect([a, b, c, e])));
        }
    }
    out.sort_by(|a, b| {
        a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.iter().zip(b.2.iter()).fold(std::cmp::Ordering::Equal, |o, (x, y)| o.then(x.total_cmp(y))))
    });
    out.dedup();
    out
}

// ---------------------------------------------------------------- planning the crops

/// Whether a link shows `level`.
/// Whether a sheet (or its view) is of `level`: its level tag, else a title naming the level.
fn names_level(l: &SheetLink, level: &str) -> bool {
    shows_level(l, level)
        || (l.level.is_none()
            && l.levels.is_empty()
            && title_names_level(&format!("{} {}", l.title, l.view.as_ref().map_or("", |v| v.title.as_str())), level))
}

pub(crate) fn shows_level(l: &SheetLink, level: &str) -> bool {
    l.level.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(level)) || l.levels.iter().any(|x| x.eq_ignore_ascii_case(level))
}

/// `a`–`b` cut to `r`, if any of it is inside.
/// The model's wall faces round the area (both faces of each wall, its centreline when it has no
/// thickness), carried onto a sheet as a crop is placed and cut to the crop's rectangle.
fn wall_faces(model: &Model, frame: &Frame, area: &Area, margin: f64, place: &Place, r: [f64; 4]) -> Vec<vreg::Seg> {
    let near = grow(area.bbox, margin + vreg::REACH_FT);
    let to = |q: [f64; 2]| {
        let p = frame.to_sheet(&area.level, q);
        place.to_sheet([p.x, p.y])
    };
    let mut out = Vec::new();
    for w in model.walls.iter().filter(|w| w.level.eq_ignore_ascii_case(&area.level) && segment_touches(near, w.a, w.b)).take(MAX_OBJECTS) {
        let (dx, dy) = (w.b[0] - w.a[0], w.b[1] - w.a[1]);
        let l = dx.hypot(dy);
        if !(l.is_finite() && l > 1e-9) {
            continue;
        }
        let half = (w.thickness_in / 24.0).max(0.0);
        let (nx, ny) = (-dy / l * half, dx / l * half);
        let sides: &[f64] = if half > 0.0 { &[-1.0, 1.0] } else { &[0.0] };
        for s in sides {
            let (Some(a), Some(b)) = (to([w.a[0] + nx * s, w.a[1] + ny * s]), to([w.b[0] + nx * s, w.b[1] + ny * s])) else { continue };
            if let Some(seg) = vreg::clip_seg(a, b, r) {
                out.push(seg);
            }
        }
        if out.len() >= vreg::MAX_MODEL_LINES {
            break;
        }
    }
    out
}

/// The model's wall faces on `level` (feet: both faces of each wall, its centreline when it has
/// no thickness) cut to `near`.
pub(crate) fn faces_round(model: &Model, level: &str, near: [f64; 4]) -> Vec<vreg::Seg> {
    let mut out = Vec::new();
    for w in model.walls.iter().filter(|w| w.level.eq_ignore_ascii_case(level) && segment_touches(near, w.a, w.b)).take(MAX_OBJECTS) {
        let (dx, dy) = (w.b[0] - w.a[0], w.b[1] - w.a[1]);
        let l = dx.hypot(dy);
        if !(l.is_finite() && l > 1e-9) {
            continue;
        }
        let half = (w.thickness_in / 24.0).max(0.0);
        let (nx, ny) = (-dy / l * half, dx / l * half);
        let sides: &[f64] = if half > 0.0 { &[-1.0, 1.0] } else { &[0.0] };
        for s in sides {
            if let Some(seg) = vreg::clip_seg([w.a[0] + nx * s, w.a[1] + ny * s], [w.b[0] + nx * s, w.b[1] + ny * s], near) {
                out.push(seg);
            }
        }
        if out.len() >= vreg::MAX_MODEL_LINES {
            break;
        }
    }
    out
}

/// The rectangles of a unit type's own views (`views[].rect`, and the sheet each is on), sheet units.
pub(crate) fn type_view_rects(ty: &Value) -> Vec<[f64; 4]> {
    let rect4 = |v: Option<&Value>| -> Option<[f64; 4]> {
        let r = v?.as_array()?;
        let n = |i: usize| r.get(i).and_then(Value::as_f64).filter(|x| x.is_finite());
        Some(norm_rect([n(0)?, n(1)?, n(2)?, n(3)?]))
    };
    ty.get("views")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_TYPE_VIEWS)
        .flat_map(|v| [rect4(v.get("rect")), rect4(v.get("sheet_rect"))])
        .flatten()
        .collect()
}

/// How far round a room (feet) the model's walls are looked at to lay a view on it.
const TURN_WALLS_FT: f64 = 50.0;
/// Views whose lines are kept read, at most.
const MAX_READ_VIEWS: usize = 2_000;

/// The unit types' views' lines, each read once (Q6339): how a view is turned and mirrored on a
/// room is measured from them on the room's model walls ([`vreg::view_turn`]), never assumed
/// from another view of the type.
pub(crate) struct UnitViews<'a> {
    d: &'a Drawing,
    index: PrintIndex<'a>,
    upf: f64,
    read: std::cell::RefCell<BTreeMap<[u64; 4], Option<std::rc::Rc<vreg::ViewLines>>>>,
}

impl<'a> UnitViews<'a> {
    /// For views lying in `within` (sheet rectangles), `upf` drawing units a foot.
    pub(crate) fn of(d: &'a Drawing, within: &[[f64; 4]], upf: f64) -> UnitViews<'a> {
        let index = if within.is_empty() { PrintIndex(Vec::new()) } else { PrintIndex::of(d, within) };
        UnitViews { d, index, upf, read: std::cell::RefCell::new(BTreeMap::new()) }
    }

    /// How the view `view` (a sheet rectangle) lies on the room whose box is `room` (feet) on
    /// `level`, by the model's walls. Err: why it can't be measured.
    pub(crate) fn turn(&self, view: [f64; 4], model: &Model, level: &str, room: [f64; 4]) -> std::result::Result<vreg::Turned, String> {
        let view = norm_rect(view);
        let key = view.map(f64::to_bits);
        let known = self.read.borrow().get(&key).cloned();
        let lines = match known {
            Some(l) => l,
            None => {
                let l = vreg::ViewLines::new(&sheet_lines(self.d, &self.index, view), self.upf, view).map(std::rc::Rc::new);
                let mut read = self.read.borrow_mut();
                if read.len() < MAX_READ_VIEWS {
                    read.insert(key, l.clone());
                }
                l
            }
        };
        let Some(v) = lines else { return Err("the view holds no straight lines of 1 ft or more".into()) };
        vreg::view_turn(&v, &faces_round(model, level, grow(room, TURN_WALLS_FT)), room)
    }
}

/// The sheet's straight lines in `r` as the page prints them: lines, polyline pieces and the
/// edges of solid fills (a wall's poche), cut to `r`.
fn sheet_lines(d: &Drawing, index: &PrintIndex, r: [f64; 4]) -> Vec<vreg::Seg> {
    let ents = as_printed(d, &index.within(r));
    let mut out: Vec<vreg::Seg> = Vec::new();
    let cap = vreg::MAX_PRINT_LINES * 4;
    let mut add = |out: &mut Vec<vreg::Seg>, a: [f64; 2], b: [f64; 2]| {
        if out.len() < cap
            && let Some(s) = vreg::clip_seg(a, b, r)
        {
            out.push(s);
        }
    };
    let ring =
        |out: &mut Vec<vreg::Seg>, add: &mut dyn FnMut(&mut Vec<vreg::Seg>, [f64; 2], [f64; 2]), v: &[cadaddy_geom::PolyVertex], closed: bool| {
            let n = v.len().min(100_000);
            for i in 0..n {
                let (Some(p), Some(q)) = (v.get(i), v.get(if i + 1 < n { i + 1 } else { 0 })) else { continue };
                if (i + 1 >= n && !closed) || p.bulge.abs() > 1e-9 {
                    continue;
                }
                add(out, [p.p.x, p.p.y], [q.p.x, q.p.y]);
            }
        };
    for e in ents {
        if out.len() >= cap {
            break;
        }
        match &e.kind {
            EntityKind::Line(l) => add(&mut out, [l.a.x, l.a.y], [l.b.x, l.b.y]),
            EntityKind::LwPolyline(p) => ring(&mut out, &mut add, &p.vertices, p.closed),
            EntityKind::Polyline3d(p) => {
                for w in p.points.windows(2).take(100_000) {
                    if let [a, b] = w {
                        add(&mut out, [a.x, a.y], [b.x, b.y]);
                    }
                }
            }
            EntityKind::Hatch(h) if h.solid => {
                for lp in h.loops.iter().take(10_000) {
                    ring(&mut out, &mut add, &lp.vertices, true);
                }
            }
            _ => {}
        }
    }
    out
}

struct Planned {
    crops: Vec<Crop>,
    left_out: Vec<(String, String)>,
    /// Deciding documents the packet can't show (a cited sheet the drawing doesn't hold): what, why.
    not_shown: Vec<(String, String)>,
}

/// The documents that decide (Q6261, [`super::validate_docs`]) a packet's crops are asked for.
struct DocInput<'a> {
    /// The schedule, legend and notes sheets of the area's trades, shown whole.
    sheets: &'a [super::validate_docs::DocSheet],
    /// The sheets the area's prescreen cites, with the points it read on them.
    cites: &'a BTreeMap<String, super::validate_docs::Cites>,
    /// The job's imported sheets that are no plan link: number, kind, rectangle.
    regions: &'a [(String, String, [f64; 4])],
}

/// A sheet as a crop names it when the building spec has no link for it: the job's rectangle.
fn own_link(number: &str, title: &str, rect: [f64; 4]) -> SheetLink {
    SheetLink {
        number: number.to_string(),
        title: title.to_string(),
        discipline: None,
        level: None,
        source: "job".into(),
        rect: Some(rect),
        levels: Vec::new(),
        registration: None,
        view: None,
    }
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
    keynotes: &[super::validate_keynote::CropAsk],
    docs: &DocInput,
    register: &dyn Fn(&Place, [f64; 4]) -> vreg::Registered,
    measure: &dyn Fn([f64; 4]) -> std::result::Result<vreg::Turned, String>,
) -> Planned {
    let cited = &keys.cited;
    let mut crops: Vec<Crop> = Vec::new();
    let mut left_out: Vec<(String, String)> = Vec::new();
    let mut not_shown: Vec<(String, String)> = Vec::new();
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
        return Planned { crops, left_out, not_shown };
    };
    let mut links: Vec<&SheetLink> = sp.sheets.iter().filter(|l| l.rect.is_some_and(|r| r.iter().all(|v| v.is_finite()))).collect();
    links.sort_by(|a, b| a.number.cmp(&b.number).then(a.view.as_ref().map(|v| v.title.as_str()).cmp(&b.view.as_ref().map(|v| v.title.as_str()))));
    let mut n = 0usize;
    let mut same: Vec<(String, String)> = Vec::new();
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
        // Never a sliver: at least MIN_CROP_PX a side, widened round its centre on the sheet.
        let rect = widen(rect, MIN_CROP_PX / ppu, sheet_rect);
        // No two pictures of one view.
        if let Some(k) = crops.iter().find(|c| c.sheet.eq_ignore_ascii_case(&l.number) && overlap(c.whole, rect) >= SAME_VIEW) {
            let what = shows.clone().or_else(|| l.view.as_ref().map(|v| v.title.clone())).unwrap_or_else(|| kind.replace('_', " "));
            same.push((format!("sheet {} {what}", l.number), format!("the same view of sheet {} as {}", l.number, k.file)));
            return;
        }
        // A whole view of a sheet that isn't placed is for finding the area on it: fewer tiles; a
        // whole schedule or notes sheet is read, so at about the packet's dpi.
        let side = match placed_by {
            "unregistered" => WHOLE_TILES_SIDE,
            "document" => DOC_TILES_SIDE,
            _ => MAX_TILES_SIDE,
        };
        let (ppu, parts) = tiles_max(rect, ppu, side);
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
                unregistered: false,
                cited: false,
                whole: rect,
                registration: None,
                deciding: None,
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
    // 1. The plans, enlarged plans and RCPs of the area's level (its level tag, or a title naming
    //    the level), placed by their registration; an unplaced one is offered in 1c.
    let names_level = |l: &SheetLink| names_level(l, &area.level);
    let mut pending: Vec<&SheetLink> = Vec::new();
    // Placed, but the placement puts the area outside the view: looked for by the room's tag.
    let mut missed: Vec<&SheetLink> = Vec::new();
    let mut misses: Vec<(String, String, String)> = Vec::new();
    for l in links.iter().filter(|l| names_level(l)) {
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
                pending.push(*l);
                continue;
            }
        };
        let on_sheet: Vec<[f64; 2]> = corners(proj_box).iter().filter_map(|q| place.to_sheet(*q)).collect();
        let Some(mut r) = bbox(&on_sheet).filter(|_| on_sheet.len() == 4).and_then(|b| intersect(b, clip)) else {
            misses.push((
                l.number.clone(),
                view_label(l),
                "it is placed on the level's plan, its view does not reach this area, and the room's tag is not on it".into(),
            ));
            missed.push(*l);
            continue;
        };
        // Its registration against the model's walls (Q6141): a crop placed well off them is
        // placed again from the sheet's wall lines when they fit, else marked.
        let mut place = place;
        let mut reg = register(&place, r);
        if let Some(c) = reg.correction {
            let fit = reg.json["fit_ft"].as_f64().unwrap_or(0.0) * frame.upf() * place.scale();
            let reference = l.registration.as_ref().map_or_else(String::new, |x| x.reference.clone());
            let refit = place.corrected(&c, &reference, reg.json["lines"].as_u64().unwrap_or(0) as usize, fit).and_then(|p| {
                let on: Vec<[f64; 2]> = corners(proj_box).iter().filter_map(|q| p.to_sheet(*q)).collect();
                bbox(&on).filter(|_| on.len() == 4).and_then(|b| intersect(b, clip)).map(|b| (p, b))
            });
            match refit {
                Some((p, b)) => {
                    place = p;
                    r = b;
                }
                None => {
                    reg.json["status"] = json!("misregistered");
                    reg.json["why"] = json!("the wall-line fit puts the area outside this view");
                    reg.json["read"] =
                        json!("misregistered: the wall-line fit puts the area outside this view: measure from the walls, not from the overlay");
                }
            }
        }
        let orientation = match &place {
            Place::Fit(reg) if kind == "enlarged_plan" || turn_deg(reg.b.atan2(reg.a)).abs() > 0.05 => Some(orientation_of(reg, by)),
            _ => None,
        };
        let before = crops.len();
        push(&mut crops, l, kind, trade, r, by, Some(place), None, orientation);
        for c in crops.iter_mut().skip(before) {
            c.registration = Some(reg.json.clone());
        }
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
            // The stamping's turn only for a view of the sheet it was fitted on.
            let unit_fit = Some(&u.fit).filter(|f| f.is_object() && stamped_on(f, &l.number));
            let fallback = match (fit, unit_fit) {
                (Some(f), _) => {
                    json!({"rotation_deg": f["rotation_deg"], "mirrored": f["mirrored"], "by": "layout_fit", "fit_ft": f["fit_ft"], "matched": f["matched"]})
                }
                (None, Some(f)) => json!({"rotation_deg": f["rotation_deg"], "mirrored": f["mirrored"], "by": f["by"]}),
                (None, None) => json!({"rotation_deg": null, "mirrored": u.hand == "mirrored", "by": "unit hand"}),
            };
            let o = measured_turn(fallback, kind == "enlarged_plan", rect, u, measure);
            push(&mut crops, l, kind, trade, rect, "unit_type", None, Some(format!("unit type {}: {title}", u.name)), Some(o));
        }
        for l in links.iter().filter(|l| !listed.iter().any(|(sh, ..)| l.number.eq_ignore_ascii_case(sh))) {
            if n_unit >= MAX_UNIT_VIEWS {
                break;
            }
            let title = format!("{} {}", l.title, l.view.as_ref().map_or("", |v| v.title.as_str()));
            // A view of the type's own sheet is the type's when its title names the type, or the
            // sheet has no titled views: another titled view there (a core area's, a corridor's)
            // is not the room's.
            let titled = l.view.as_ref().is_some_and(|v| !v.title.trim().is_empty());
            let its_sheet = !titled && !source.is_empty() && l.number.eq_ignore_ascii_case(&source);
            if sheet_kind(&title, true) != "enlarged_plan" || !(names_it(&title) || its_sheet) {
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
            let fallback = match (fit, &l.registration) {
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
            let o = measured_turn(fallback, true, rect, u, measure);
            n_unit += 1;
            push(&mut crops, l, "enlarged_plan", trade, rect, "unit_type", None, Some(format!("unit type {}", u.name)), Some(o));
        }
    }
    // 1c. The sheets the area's elements were read from (always in the packet), then the level's
    //     sheets not placed on its plan: round the elements' source points on the sheet, else
    //     round the room's tag on it, else the whole view in tiles, marked `unregistered`.
    let mut todo: Vec<(&SheetLink, Option<&Cited>, bool)> = Vec::new();
    for (key, c) in cited {
        if crops.iter().any(|x| x.sheet.eq_ignore_ascii_case(key)) {
            for x in crops.iter_mut().filter(|x| x.sheet.eq_ignore_ascii_case(key)) {
                x.cited = true;
            }
            continue;
        }
        let of_sheet: Vec<&SheetLink> = links.iter().filter(|l| l.number.eq_ignore_ascii_case(key)).copied().collect();
        // Its view of the area's level, else the whole sheet, else its first view.
        let Some(l) = of_sheet.iter().find(|l| names_level(l)).or_else(|| of_sheet.iter().find(|l| l.view.is_none())).or(of_sheet.first()).copied()
        else {
            left_out.push((
                format!("sheet {key}"),
                format!("{} element(s) here were read from it, but the building spec lists no such sheet (levels.sheets)", c.elements),
            ));
            continue;
        };
        let trade = trade_of_discipline(&l.discipline.clone().unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(&l.number)));
        if want(trade) {
            todo.push((l, Some(c), missed.iter().any(|m| std::ptr::eq(*m, l))));
        }
    }
    for (l, was_missed) in pending.into_iter().map(|l| (l, false)).chain(missed.iter().map(|l| (*l, true))) {
        if !todo.iter().any(|(t, ..)| std::ptr::eq(*t, l)) {
            todo.push((l, None, was_missed));
        }
    }
    // The reference plan's sheet, for the scale of a sheet that isn't placed (the same paper
    // scale: its size against the reference sheet's).
    let centre = [(area.bbox[0] + area.bbox[2]) / 2.0, (area.bbox[1] + area.bbox[3]) / 2.0];
    let centre_on_plan = {
        let p = frame.to_sheet(&area.level, centre);
        [p.x, p.y]
    };
    let ref_long = links
        .iter()
        .filter_map(|l| l.rect.map(norm_rect))
        .find(|r| inside(*r, centre_on_plan))
        .map(|r| (r[2] - r[0]).max(r[3] - r[1]))
        .filter(|x| x.is_finite() && *x > 0.0);
    let room_words: Vec<String> = [area.number.clone(), area.name.clone()].into_iter().flatten().filter(|k| !k.trim().is_empty()).collect();
    for (l, c, was_missed) in todo {
        let Some(sr) = l.rect.map(norm_rect) else { continue };
        let clip = l.view.as_ref().and_then(|v| intersect(norm_rect(v.rect), sr)).unwrap_or(sr);
        let long = (sr[2] - sr[0]).max(sr[3] - sr[1]);
        if !(long.is_finite() && long > 0.0) {
            continue;
        }
        let title = format!("{} {}", l.title, l.view.as_ref().map_or("", |v| v.title.as_str()));
        let kind = sheet_kind(&title, true);
        let trade = trade_of_discipline(&l.discipline.clone().unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(&l.number)));
        // The area's size on this sheet: the plan's units per foot, scaled as the sheet is to the
        // reference sheet; generous (an enlarged view is drawn larger).
        let per_ft = frame.upf() * ref_long.map_or(1.0, |rl| long / rl);
        let half = [
            (((area.bbox[2] - area.bbox[0]) / 2.0 + margin) * per_ft * 1.6).max(0.06 * long),
            (((area.bbox[3] - area.bbox[1]) / 2.0 + margin) * per_ft * 1.6).max(0.06 * long),
        ];
        let around = |p: [f64; 2]| [p[0] - half[0], p[1] - half[1], p[0] + half[0], p[1] + half[1]];
        let start = crops.len();
        let points: Vec<[f64; 2]> = c.map(|c| c.points.iter().filter(|p| inside(sr, **p)).copied().collect()).unwrap_or_default();
        let read = c.map(|c| format!("{} element(s) here were read from it", c.elements));
        let shows = |what: String| Some(read.as_ref().map_or(what.clone(), |r| format!("{what}; {r}")));
        if let Some(r) = bbox(&points).and_then(|b| intersect([b[0] - half[0], b[1] - half[1], b[2] + half[0], b[3] + half[1]], sr)) {
            push(&mut crops, l, kind, trade, r, "source_point", None, shows("round the points the elements were read at".into()), None);
        } else if let Some((at, word)) = room_tag(texts, clip, &room_words, long)
            && let Some(r) = intersect(around(at), clip)
        {
            push(&mut crops, l, kind, trade, r, "room_tag", None, shows(format!("round {word} (its tag on the sheet)")), None);
        } else {
            if was_missed && c.is_none() {
                // Its placement and its texts agree the area is not on it (said in `misses`).
                continue;
            }
            let (cw, ch) = (clip[2] - clip[0], clip[3] - clip[1]);
            let share = (cw * ch) / ((sr[2] - sr[0]) * (sr[3] - sr[1])).max(1e-9);
            // Only a view that is clearly part of the level (an enlarged plan, a small view) and
            // names nothing of the room is left out; a level's plan is offered whole.
            if c.is_none() && !room_words.is_empty() && (share < PARTIAL_VIEW || kind == "enlarged_plan") {
                misses.push((
                    l.number.clone(),
                    view_label(l),
                    format!(
                        "a partial view of the level ({}, {:.0}% of the sheet) that names neither {}: not this area",
                        kind.replace('_', " "),
                        share * 100.0,
                        room_words.join(" nor ")
                    ),
                ));
                continue;
            }
            push(
                &mut crops,
                l,
                kind,
                trade,
                clip,
                "unregistered",
                None,
                shows("the whole view: it is not placed on the level's plan yet (bim.identity)".into()),
                None,
            );
        }
        for x in crops.iter_mut().skip(start) {
            x.unregistered = true;
            x.cited = c.is_some();
        }
    }
    for (number, label, why) in misses {
        if !crops.iter().any(|x| x.sheet.eq_ignore_ascii_case(&number)) {
            left_out.push((label, why));
        }
    }
    // 1d. The print round each keynote marker in or by the area, and the keynote lists of the
    //     sheets that mark it.
    for k in keynotes {
        // A gear sheet the spec has no link for (a schedule or diagram the job imported): its
        // rectangle, as a link of its own.
        let own = (!k.kind.starts_with("keynote")).then(|| SheetLink {
            number: k.sheet.clone(),
            title: k.shows.clone(),
            discipline: None,
            level: None,
            source: "job".into(),
            rect: Some(k.rect),
            levels: Vec::new(),
            registration: None,
            view: None,
        });
        let Some(l) = links
            .iter()
            .copied()
            .find(|l| l.number.eq_ignore_ascii_case(&k.sheet) && l.view.is_none())
            .or_else(|| links.iter().copied().find(|l| l.number.eq_ignore_ascii_case(&k.sheet)))
            .or(own.as_ref())
        else {
            continue;
        };
        let trade = trade_of_discipline(&l.discipline.clone().unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(&l.number)));
        if !want(trade) {
            continue;
        }
        let Some(sr) = l.rect.map(norm_rect) else { continue };
        let Some(mut rect) = intersect(norm_rect(k.rect), sr) else { continue };
        // A keynote list whole: its reader's rectangle grown over the lines that carry it on.
        if k.kind == "keynotes" {
            rect = texts.block(rect, sr);
        }
        let by = if k.kind.starts_with("keynote") { "keynote" } else { "gear" };
        let before = crops.len();
        push(&mut crops, l, k.kind, trade, rect, by, None, Some(k.shows.clone()), None);
        let why = match k.kind {
            "keynote_marker" => None,
            "keynotes" => Some("the keynote list of a sheet that marks this area, whole"),
            _ => Some("the gear's riser or schedule, whole"),
        };
        for c in crops.iter_mut().skip(before) {
            c.deciding = why.map(str::to_string);
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
    let plan_sheets: BTreeSet<String> = crops.iter().filter(|c| !c.kind.starts_with("keynote")).map(|c| c.sheet.clone()).collect();
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
        // The lighting-control legend, where the area holds lighting.
        if keys.lighting && matches!(kind, "legend" | "schedule" | "general_notes" | "spec" | "other") {
            for key in ["LIGHTING CONTROL", "LIGHTING CONTROLS"] {
                for (at, _) in texts.find(sr, key, true).into_iter().take(1) {
                    add(
                        &mut crops,
                        "legend",
                        [at[0] - 0.02 * w, at[1] - 0.3 * h, at[0] + 0.33 * w, at[1] + 0.03 * h],
                        "lighting control legend".into(),
                    );
                }
            }
        }
        // General notes: on a notes sheet, and on the plans in the packet; the whole block.
        if kind == "general_notes" || plan_sheets.contains(&l.number) {
            for (at, _) in texts.find(sr, "GENERAL NOTES", true).into_iter().take(1) {
                let seed = [at[0] - 0.02 * w, at[1] - 0.4 * h, at[0] + 0.33 * w, at[1] + 0.03 * h];
                let before = crops.len();
                add(&mut crops, "general_notes", texts.block(seed, sr), "GENERAL NOTES".into());
                for c in crops.iter_mut().skip(before) {
                    c.deciding = Some(format!("the general notes of sheet {}, whole", l.number));
                }
            }
        }
    }
    // 2b. The schedule, legend and notes sheets of the area's trades, whole (Q6261), after the
    //     rows and notes read on them.
    for doc in docs.sheets {
        let Some(rect) = doc.rect.filter(|_| want(doc.trade)) else { continue };
        let made;
        let l: &SheetLink = match links.iter().find(|l| l.number.eq_ignore_ascii_case(&doc.number) && l.view.is_none()) {
            Some(l) => l,
            None => {
                made = own_link(&doc.number, &doc.title, rect);
                &made
            }
        };
        let before = crops.len();
        let title = if doc.title.trim().is_empty() { String::new() } else { format!(" {}", doc.title.trim()) };
        push(&mut crops, l, doc.kind, doc.trade, rect, "document", None, Some(format!("the whole sheet{title}")), None);
        for c in crops.iter_mut().skip(before) {
            c.deciding = Some(format!("the {} sheet of this area's trades ({}), whole", doc.kind.replace('_', " "), doc.trade));
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
    // 4. The sheets the area's prescreen cites (Q6261): the pictures of them that hold a cited
    //    point decide; a point no picture holds gets a crop round it; a sheet cited with no point
    //    is shown whole unless the packet shows it already.
    for (number, c) in docs.cites {
        let why = format!("the prescreen cites it ({})", c.rules.iter().take(8).cloned().collect::<Vec<_>>().join(", "));
        let mine: Vec<usize> = crops.iter().enumerate().filter(|(_, x)| x.sheet.eq_ignore_ascii_case(number)).map(|(i, _)| i).collect();
        let held = |p: &[f64; 2]| mine.iter().any(|i| crops.get(*i).is_some_and(|x| inside(x.rect, *p)));
        let open: Vec<[f64; 2]> = c.points.iter().filter(|p| !held(p)).copied().collect();
        for i in &mine {
            if let Some(x) = crops.get_mut(*i)
                && x.deciding.is_none()
                && (c.points.is_empty() || c.points.iter().any(|p| inside(x.rect, *p)))
            {
                x.deciding = Some(why.clone());
            }
        }
        if !mine.is_empty() && open.is_empty() {
            continue;
        }
        // The sheet: the view holding the points, else the whole sheet, else the job's rectangle.
        let of_sheet: Vec<&SheetLink> = links.iter().filter(|l| l.number.eq_ignore_ascii_case(number)).copied().collect();
        let region = docs.regions.iter().find(|r| r.0.eq_ignore_ascii_case(number));
        let made;
        let l: &SheetLink = match of_sheet
            .iter()
            .find(|l| l.view.as_ref().is_some_and(|v| open.iter().any(|p| inside(norm_rect(v.rect), *p))))
            .or_else(|| of_sheet.iter().find(|l| l.view.is_none()))
            .or(of_sheet.first())
        {
            Some(l) => l,
            None => match region {
                Some(r) => {
                    made = own_link(&r.0, &r.1.replace('_', " "), norm_rect(r.2));
                    &made
                }
                None => {
                    not_shown.push((format!("sheet {number}"), format!("{why}, but the drawing holds no such sheet")));
                    continue;
                }
            },
        };
        let Some(sr) = l.rect.map(norm_rect) else { continue };
        let trade = trade_of_discipline(&l.discipline.clone().unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(&l.number)));
        let title = format!("{} {}", l.title, l.view.as_ref().map_or("", |v| v.title.as_str()));
        let kind = sheet_kind(&title, l.level.is_some() || !l.levels.is_empty());
        let before = crops.len();
        match bbox(&open) {
            Some(b) => {
                let long = (sr[2] - sr[0]).max(sr[3] - sr[1]);
                let r = intersect(grow(b, 0.06 * long), sr).unwrap_or(sr);
                push(
                    &mut crops,
                    l,
                    kind,
                    trade,
                    r,
                    "cited_point",
                    None,
                    Some(format!("round the {} point(s) the prescreen read on it", open.len())),
                    None,
                );
            }
            None => push(&mut crops, l, kind, trade, sr, "document", None, Some("the whole sheet: the prescreen cites it".into()), None),
        }
        for x in crops.iter_mut().skip(before) {
            x.deciding = Some(why.clone());
        }
    }
    left_out.extend(same);
    Planned { crops, left_out, not_shown }
}

/// A sheet and its view, as `left_out` names it.
fn view_label(l: &SheetLink) -> String {
    match &l.view {
        Some(v) => format!("sheet {} view {}", l.number, v.title),
        None => format!("sheet {} {}", l.number, l.title).trim_end().to_string(),
    }
}

/// Where the room's tag is in `clip` of a sheet: its number (beside its name when the sheet has
/// several), else its name when the sheet names it once.
fn room_tag(texts: &Texts, clip: [f64; 4], words: &[String], long: f64) -> Option<([f64; 2], String)> {
    let hits: Vec<(&String, Vec<[f64; 2]>)> = words.iter().map(|w| (w, texts.find(clip, w, false).into_iter().map(|h| h.0).collect())).collect();
    // A tag `STUDIO 410` in one text.
    if let (Some(n), Some(nm)) = (words.first(), words.get(1)) {
        let both = format!("{nm} {n}");
        let one: Vec<[f64; 2]> = texts.find(clip, &both, true).into_iter().chain(texts.find(clip, &format!("{n} {nm}"), true)).map(|h| h.0).collect();
        if let Some(p) = one.first() {
            return Some((*p, format!("room {n} {nm}")));
        }
    }
    let near = 0.05 * long;
    let (first, rest) = hits.split_first()?;
    let names: Vec<[f64; 2]> = rest.iter().flat_map(|(_, h)| h.iter().copied()).collect();
    let numbered = first.1.iter().find(|p| names.iter().any(|q| (p[0] - q[0]).hypot(p[1] - q[1]) <= near)).or(if first.1.len() == 1 {
        first.1.first()
    } else {
        None
    });
    if let Some(p) = numbered {
        return Some((*p, format!("room {}", first.0)));
    }
    match names.as_slice() {
        [one] => Some((*one, format!("room {}", rest.first().map_or("", |r| r.0.as_str())))),
        _ => None,
    }
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
        let local = local_path(p);
        let path = local.as_path();
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

/// A path the job kept, as this machine reaches it: a Windows drive path (`C:\\x\\y`) that isn't
/// here is tried where WSL mounts the drive (`/mnt/c/x/y`).
fn local_path(p: &str) -> std::path::PathBuf {
    let path = std::path::PathBuf::from(p);
    if path.exists() || cfg!(windows) {
        return path;
    }
    let b = p.as_bytes();
    match (b.first(), b.get(1), b.get(2)) {
        (Some(d), Some(b':'), Some(b'\\' | b'/')) if d.is_ascii_alphabetic() => {
            let rest = p.get(3..).unwrap_or("").replace('\\', "/");
            let wsl = std::path::PathBuf::from(format!("/mnt/{}/{rest}", char::from(d.to_ascii_lowercase())));
            if wsl.exists() { wsl } else { path }
        }
        _ => path,
    }
}

/// The last part of a path the job kept (either slash).
fn last_part(p: &str) -> String {
    p.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(p).to_string()
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

/// What the job applied one submittal page to, across the building.
#[derive(Default)]
struct Applied {
    marks: BTreeSet<String>,
    /// Each kind it was applied to, and to how many.
    kinds: BTreeMap<String, usize>,
    elements: usize,
}

impl Applied {
    /// Whether `kind` is what this page is mostly for (half its elements or more): a page applied
    /// to a few elements of another kind (a sign typed like a fixture) is not offered for it.
    fn is_for(&self, kind: &str) -> bool {
        self.kinds.get(kind).is_some_and(|n| n * 2 >= self.elements)
    }
}

/// Every submittal page the building's elements carry (`name p.N`; the plan elements and what
/// the build modelled), with the marks (upper case) and kinds it was applied to.
fn applied_submittals(d: &Drawing, kinds: &Kinds) -> BTreeMap<String, Applied> {
    let mut out: BTreeMap<String, Applied> = BTreeMap::new();
    for e in d.model.iter().take(MAX_ENTITIES) {
        if e.common.xdata.is_empty() {
            continue;
        }
        let store = cadaddy_doc::props::read(&e.common.xdata);
        let Some(sb) = store.get("submittal").filter(|p| p.has_value()).map(|p| p.text().trim().to_string()).filter(|x| !x.is_empty()) else {
            continue;
        };
        if out.len() >= MAX_SUBMITTAL_PAGES && !out.contains_key(&sb) {
            continue;
        }
        let kind = super::elemprops::element_kind(e).unwrap_or_else(|| "device".into());
        let mark = super::validate_kind::mark_of(e);
        let a = out.entry(sb).or_default();
        a.elements += 1;
        if let Some(m) = mark.as_ref().filter(|m| !m.trim().is_empty()) {
            a.marks.insert(m.trim().to_ascii_uppercase());
        }
        *a.kinds.entry(kinds.resolve(&kind, mark.as_deref(), Some(e)).kind).or_insert(0) += 1;
    }
    out
}

/// Specification pages a packet carries, at most.
const MAX_SPEC_PAGES: usize = 8;

/// A specification page a packet shows: the section's file and page, and the product it is for.
struct SpecPage {
    file: String,
    section: String,
    page: u32,
    path: std::path::PathBuf,
    product: String,
    kinds: Vec<String>,
}

/// The digits of a section number or a file name's start (`26 27 26` → `262726`).
fn section_digits(s: &str) -> String {
    s.chars().take_while(|c| c.is_ascii_digit() || c.is_whitespace() || matches!(c, '.' | '-' | '_')).filter(char::is_ascii_digit).take(12).collect()
}

/// The file among `files` named for section `section` (its name starts with the section's
/// number, spaced or not).
fn spec_file<'f>(files: &'f [String], section: &str) -> Option<&'f String> {
    let want = section_digits(section);
    if want.len() < 5 {
        return None;
    }
    files.iter().take(MAX_FOLDER_FILES).find(|f| {
        let stem = std::path::Path::new(f.as_str()).file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
        let have = section_digits(stem.trim_start_matches(|c: char| !c.is_ascii_digit()));
        have.len() >= want.len() && have.starts_with(&want)
    })
}

/// A page number from a page as a record gives it: a footer (`262726 - 2` → 2) or a number.
fn footer_page(page: &str) -> Option<u32> {
    let t = page.trim();
    let last = t.rsplit(['-', ' ', '.']).find(|x| !x.trim().is_empty()).unwrap_or(t).trim();
    last.parse::<u32>().ok().filter(|n| *n >= 1 && *n <= 5_000)
}

/// A specification page as a picture: cut to its product's paragraph (the band from a line
/// naming the product down a third of the page) when a text on it names the product, else whole.
fn spec_png(path: &std::path::Path, page: u32, product: &str) -> std::result::Result<(Vec<u8>, bool), String> {
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
    // The product's heading words (`TOGGLE SWITCHES` of `TOGGLE SWITCHES: Switches, 20 A`).
    let head = product.split([':', ',']).next().unwrap_or_default().trim().to_ascii_uppercase();
    let at = (head.len() >= 4)
        .then(|| {
            pd.model.iter().take(MAX_ENTITIES).find_map(|e| match &e.kind {
                EntityKind::Text(t) if t.value.to_ascii_uppercase().contains(&head) => Some(t.insert.y),
                EntityKind::MText(t) if t.contents.to_ascii_uppercase().contains(&head) => Some(t.insert.y),
                _ => None,
            })
        })
        .flatten()
        .filter(|y| y.is_finite());
    let rect = match at {
        Some(y) => [ext.min.x, (y - h / 3.0).max(ext.min.y), ext.max.x, (y + h / 30.0).min(ext.max.y)],
        None => [ext.min.x, ext.min.y, ext.max.x, ext.max.y],
    };
    let (rw, rh) = ((rect[2] - rect[0]).max(1e-9), (rect[3] - rect[1]).max(1e-9));
    let scale = f64::from(SUBMITTAL_PX) / w.max(h);
    let px = [((rw * scale).ceil() as u32).clamp(16, SUBMITTAL_PX), ((rh * scale).ceil() as u32).clamp(16, SUBMITTAL_PX)];
    let png = render_drawing(&pd, rect, px).ok_or_else(|| "the page couldn't be drawn".to_string())?;
    Ok((png, at.is_some()))
}

/// Whether a packet asked for `trades` shows a sheet of `t` (the architecture and the general
/// sheets always).
fn want_trade(trades: &[String], t: &str) -> bool {
    trades.is_empty() || t == "architecture" || t == "general" || trades.iter().any(|x| x == t)
}

/// PDF pages of deciding sheets one packet shows, at most.
const MAX_PDF_SHEETS: usize = 16;

/// A deciding sheet the drawing doesn't hold, shown as its PDF page (Q6261): the page read once,
/// cut in tiles at the packet's dpi.
struct PdfDoc {
    number: String,
    kind: &'static str,
    trade: &'static str,
    /// `<file> p.<n>`, as the job's index gives it.
    page: String,
    why: String,
    pd: Drawing,
    ppu: f64,
    dpi: f64,
    paper: &'static str,
    parts: Vec<([f64; 4], [u32; 2], [u32; 2])>,
}

/// The PDF pages of `asks` (sheet number, kind, trade, why), found among `search` (the job's
/// prints folder, the packet's `prints`) by the file the job's index names; what can't be shown
/// goes in `not_shown` with why. Each file is read once.
fn pdf_pages(
    asks: &[(String, &'static str, &'static str, String)],
    job_pages: &[super::validate_gear::JobPage],
    search: &[String],
    dpi: f64,
    not_shown: &mut Vec<(String, String)>,
) -> Vec<PdfDoc> {
    let mut out: Vec<PdfDoc> = Vec::new();
    let Some(hook) = super::file::io().and_then(|h| h.read_pdf) else {
        not_shown.extend(asks.iter().map(|a| (format!("sheet {}", a.0), format!("{}: PDF pages can't be read in this build", a.3))));
        return out;
    };
    let mut files: BTreeMap<std::path::PathBuf, Option<Vec<u8>>> = BTreeMap::new();
    for (number, kind, trade, why) in asks {
        let what = format!("sheet {number}");
        if out.len() >= MAX_PDF_SHEETS {
            not_shown.push((what, format!("{why}: over the packet's {MAX_PDF_SHEETS} PDF pages")));
            continue;
        }
        let Some(p) = job_pages.iter().find(|p| p.number.eq_ignore_ascii_case(number)) else {
            not_shown.push((what, format!("{why}: the drawing doesn't hold it and the job's sheet index has no page for it")));
            continue;
        };
        let name = last_part(&p.file);
        let page = format!("{name} p.{}", p.page);
        let Some(path) = find_submittal(search, &name) else {
            not_shown.push((what, format!("{why}: its PDF page ({page}) is not on this machine (give the prints folder as `prints`)")));
            continue;
        };
        let bytes = files.entry(path.clone()).or_insert_with(|| crate::readfile::read(&path).ok().filter(|b| b.len() <= 200 * 1024 * 1024));
        let Some(bytes) = bytes.as_ref() else {
            not_shown.push((what, format!("{why}: {page} can't be read (or is over 200 MB)")));
            continue;
        };
        let pd = match hook(bytes, &json!({"page": p.page})) {
            Ok((pd, _)) => pd,
            Err(e) => {
                not_shown.push((what, format!("{why}: {page}: {e}")));
                continue;
            }
        };
        let ext = pd.extents(&Space::Model);
        let rect = [ext.min.x, ext.min.y, ext.max.x, ext.max.y];
        if ext.is_empty() || !rect.iter().all(|v| v.is_finite()) || ext.width() <= 0.0 || ext.height() <= 0.0 {
            not_shown.push((what, format!("{why}: {page} is empty")));
            continue;
        }
        let (paper_in, paper) = paper_of(rect);
        let long = ext.width().max(ext.height());
        let (ppu, parts) = tiles_max(rect, dpi * paper_in / long, DOC_TILES_SIDE);
        out.push(PdfDoc { number: number.clone(), kind, trade, page, why: why.clone(), pd, ppu, dpi: ppu * long / paper_in, paper, parts });
    }
    out
}

// ---------------------------------------------------------------- the packet

/// How far in from the outermost edge (feet) an edge facing the same way still is that side
/// of the area (a ragged outline's jogs round columns and chases).
const SIDE_DEPTH_FT: f64 = 1.5;

/// One side of an area for its model elevation: the way a camera inside looks at it (unit), its
/// edges (longest first), their total length, its first edge's index in the outline.
#[derive(Clone, Debug, PartialEq)]
struct Side {
    dir: [f64; 2],
    edges: Vec<([f64; 2], [f64; 2])>,
    len: f64,
    first: usize,
}

/// The sides of an outline (feet), longest first: its edges grouped by the way they face (out of
/// the area), each way's outermost edges (within [`SIDE_DEPTH_FT`] of the farthest) one side.
fn area_sides(outline: &[[f64; 2]]) -> Vec<Side> {
    // Each edge: the way out of the area across it, how far out that way it lies, its length.
    let mut edges: Vec<([f64; 2], f64, f64, usize, [f64; 2], [f64; 2])> = Vec::new();
    for (i, (a, b)) in outline.iter().zip(outline.iter().cycle().skip(1)).enumerate().take(MAX_OUTLINE) {
        let len = (b[0] - a[0]).hypot(b[1] - a[1]);
        if !(len.is_finite() && len >= 0.1) {
            continue;
        }
        let (dx, dy) = ((b[0] - a[0]) / len, (b[1] - a[1]) / len);
        let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
        let step = (len / 4.0).min(0.5);
        let Some(out) = [[dy, -dx], [-dy, dx]].into_iter().find(|n| point_in(outline, [m[0] - n[0] * step, m[1] - n[1] * step])) else { continue };
        edges.push((out, m[0] * out[0] + m[1] * out[1], len, i, *a, *b));
    }
    let mut ways: Vec<[f64; 2]> = Vec::new();
    for (out, ..) in &edges {
        if !ways.iter().any(|w| w[0] * out[0] + w[1] * out[1] > 0.98) {
            ways.push(*out);
        }
    }
    let mut sides: Vec<Side> = ways
        .into_iter()
        .filter_map(|w| {
            let mine: Vec<&([f64; 2], f64, f64, usize, [f64; 2], [f64; 2])> = edges.iter().filter(|e| w[0] * e.0[0] + w[1] * e.0[1] > 0.98).collect();
            let far = mine.iter().map(|e| e.1).fold(f64::NEG_INFINITY, f64::max);
            let mut outer: Vec<&&([f64; 2], f64, f64, usize, [f64; 2], [f64; 2])> = mine.iter().filter(|e| e.1 >= far - SIDE_DEPTH_FT).collect();
            outer.sort_by(|x, y| y.2.total_cmp(&x.2).then(x.3.cmp(&y.3)));
            let len: f64 = outer.iter().map(|e| e.2).sum();
            let first = outer.iter().map(|e| e.3).min()?;
            let edges = outer.iter().filter(|e| e.2 >= 1.0).map(|e| (e.4, e.5)).collect::<Vec<_>>();
            (len >= 2.0 && !edges.is_empty()).then_some(Side { dir: w, edges, len, first })
        })
        .collect();
    sides.sort_by(|x, y| y.len.total_cmp(&x.len).then(x.first.cmp(&y.first)));
    sides
}

/// The model wall on `level` a camera at `eye` looking along `dir` (unit) faces: the first whose
/// centreline its line of sight crosses within `reach` (feet), a wall over an opening (a window
/// head) too, and how far the wall's face is.
fn faced_wall<'m>(model: &'m Model, level: &str, eye: [f64; 2], dir: [f64; 2], reach: f64) -> Option<(&'m v::ModelWall, f64)> {
    let cross = |p: [f64; 2], q: [f64; 2]| p[0] * q[1] - p[1] * q[0];
    model
        .walls
        .iter()
        .chain(model.wall_spans.iter().map(|s| &s.wall))
        .filter(|w| w.level.eq_ignore_ascii_case(level))
        .take(100_000)
        .filter_map(|w| {
            let ab = [w.b[0] - w.a[0], w.b[1] - w.a[1]];
            let den = cross(dir, ab);
            if !den.is_finite() || den.abs() < 1e-9 {
                return None;
            }
            let ae = [w.a[0] - eye[0], w.a[1] - eye[1]];
            let (t, u) = (cross(ae, ab) / den, cross(ae, dir) / den);
            let face = t - (w.thickness_in / 24.0).max(0.0);
            ((0.0..=1.0).contains(&u) && t > 0.05 && t <= reach && face.is_finite()).then_some((w, face.max(0.0)))
        })
        .min_by(|x, y| x.1.total_cmp(&y.1).then(x.0.id.cmp(&y.0.id)))
}

/// The kind of the model's opening (`door`, `window`) a camera at `eye` looking along `dir`
/// sees within `reach` (feet): the first its line of sight passes through.
fn faced_opening<'m>(model: &'m Model, level: &str, eye: [f64; 2], dir: [f64; 2], reach: f64) -> Option<&'m str> {
    model
        .openings
        .iter()
        .filter(|o| o.level.eq_ignore_ascii_case(level))
        .take(100_000)
        .filter_map(|o| {
            // The ray through the opening's box (slab test), its entry distance.
            let (mut t0, mut t1) = (0.0f64, reach);
            for k in 0..2 {
                let (lo, hi) = (o.min[k].min(o.max[k]), o.min[k].max(o.max[k]));
                let (e, d) = (*eye.get(k)?, *dir.get(k)?);
                if d.abs() < 1e-12 {
                    if e < lo || e > hi {
                        return None;
                    }
                } else {
                    let (a, b) = ((lo - e) / d, (hi - e) / d);
                    t0 = t0.max(a.min(b));
                    t1 = t1.min(a.max(b));
                }
            }
            (t0 <= t1 && t0.is_finite()).then_some((t0, o.kind.as_str()))
        })
        .min_by(|x, y| x.0.total_cmp(&y.0))
        .map(|(_, k)| k)
}

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
    let job = super::guided::load_job(s).ok().flatten();
    let (job_submittals, job_prints): (Vec<String>, Option<String>) =
        job.as_ref().map(|j| (j.submittals.clone(), Some(j.pdfs_path.clone()).filter(|x| !x.trim().is_empty()))).unwrap_or_default();
    // The crops are compared with their PDF pages (`compare`, default on), read from `prints`
    // when the job's folder isn't on this machine.
    let compare = match p.get("compare") {
        None | Some(Value::Null) => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(CMD, "`compare` is true or false")),
    };
    let prints_dir: Option<String> = match p.get("prints") {
        None | Some(Value::Null) => None,
        Some(Value::String(x)) if !x.trim().is_empty() && x.len() <= 4096 => Some(x.trim().to_string()),
        Some(_) => return Err(bad(CMD, "`prints` is the folder (or PDF) the job's prints are in")),
    };
    let given_specs: Vec<String> = match p.get("specs") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(x)) if !x.trim().is_empty() && x.len() <= 4096 => vec![x.trim().to_string()],
        Some(Value::Array(a)) => a
            .iter()
            .take(50)
            .map(|x| {
                x.as_str()
                    .map(str::trim)
                    .filter(|t| !t.is_empty() && t.len() <= 4096)
                    .map(str::to_string)
                    .ok_or_else(|| bad(CMD, "`specs` is a list of specification files or folders"))
            })
            .collect::<Result<Vec<_>>>()?,
        Some(_) => return Err(bad(CMD, "`specs` is a list of specification files or folders")),
    };
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
    let kinds = Kinds::of(d, &sp, prints.as_ref());
    let built = built_parts(d, &sp, &frame, &area, 30.0, &kinds);
    let level_areas = model_areas(d, &frame);
    let member = Membership::new(&area, &level_areas);
    let mut here = in_area(d, &model, prints.as_ref(), &area, &all_rules, &built, &kinds, &member);
    // Each receptacle's tamper-resistance fact and where it came from (E-RECEPT-TR).
    if let Some(Value::Array(els)) = here.objects.get_mut("elements") {
        super::validate_tr::annotate(d, &sp, &model, &kinds, prints.as_ref(), &all_rules, &area, els);
    }
    // The lighting documents of the types present (fixture schedule rows, submittal products and
    // their cut-sheet pages, the control legend), and each luminaire's and control's facts.
    let lighting = match here.objects.get_mut("elements") {
        Some(Value::Array(els)) => super::validate_lighting::annotate(d, &model, &kinds, prints.as_ref(), &all_rules, els),
        _ => None,
    };
    if let Some((summary, pages)) = lighting {
        here.objects["lighting"] = summary;
        here.cut_sheets.extend(pages);
        here.lighting = true;
    }
    // The gear submittals, re-read when the area holds gear (E-GEAR-*'s documents).
    let gear_docs = if super::validate_gear::has_gear(d, &sp, &frame, &area) && super::validate_gear::wanted(&all_rules, &[]) {
        super::validate_gear::read_docs(s, &sp)
    } else {
        None
    };
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
        gear_docs: gear_docs.as_ref(),
        conduit: false,
        conduit_takeoff: None,
        value: None,
        equip_docs: None,
        fixture_docs: None,
    });
    // The routed raceway (the conduit rules), measured on the model's own pieces over the area's
    // level: each finding names the area's pieces it covers (`handles`), so a run's, a circuit's
    // or a box's verdict reaches each of its sticks, fittings and wires; each piece carries its
    // run geometry (`raceway`).
    let raceway = {
        use super::validate_conduit as vc;
        let wanted = |id: &str| only_rules.as_ref().is_none_or(|o| o.contains(id));
        let which = vc::On::of(|id| all_rules.iter().find(|r| r.enabled && r.rule.id == id && wanted(id)));
        let own: BTreeSet<cadaddy_doc::Handle> = here.objects["elements"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["handle"].as_str().and_then(cadaddy_doc::Handle::parse_hex))
            .collect();
        let net = vc::Net::of(d, &sp);
        let only: Vec<String> = only_rules.iter().flatten().cloned().collect();
        let takeoff = vc::read_takeoff(s, &all_rules, &only);
        let cx = vc::Ctx { d, sp: &sp, frame: &frame, areas: std::slice::from_ref(&area), whole_levels: true, takeoff: takeoff.as_ref() };
        let got = vc::check_net(&cx, which, &net);
        let facts = vc::packet_raceways(&net, &sp, &frame, &own, vc::touch_in(which.continuity));
        // The findings about the area's own pieces, each naming only those (the rest are judged
        // in their own areas' packets).
        let mut checked: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
        for id in vc::ALL.iter().filter(|id| all_rules.iter().any(|r| r.enabled && r.rule.id == **id && wanted(id))) {
            checked.insert(id.to_string(), vc::VERDICTS.iter().map(|v| (v.to_string(), 0)).collect());
        }
        let mut findings: Vec<Value> = Vec::new();
        for (_, rule, verdict, mut f) in got.findings {
            let named: Vec<String> = f["handles"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
            let mine: Vec<String> = named.iter().filter(|h| cadaddy_doc::Handle::parse_hex(h).is_some_and(|x| own.contains(&x))).cloned().collect();
            let lead = f["handle"].as_str().and_then(cadaddy_doc::Handle::parse_hex).filter(|h| own.contains(h));
            if mine.is_empty() && lead.is_none() {
                continue;
            }
            let elsewhere = named.len().saturating_sub(mine.len()) + f["handles_more"].as_u64().unwrap_or(0) as usize;
            if lead.is_none()
                && let Some(first) = mine.first()
            {
                f["measured_at"] = f["handle"].clone();
                f["handle"] = json!(first);
            }
            f["handles"] = json!(mine);
            if let Some(o) = f.as_object_mut() {
                o.remove("handles_more");
            }
            if elsewhere > 0 {
                f["handles_elsewhere"] = json!(elsewhere);
            }
            *checked.entry(rule.to_string()).or_default().entry(verdict.to_string()).or_default() += 1;
            if findings.len() < MAX_OBJECTS {
                findings.push(f);
            }
        }
        (checked, got.unmeasured, findings, facts)
    };
    let (raceway_checked, raceway_unmeasured, raceway_findings, raceway_facts) = raceway;
    if let Some(list) = here.objects.get_mut("elements").and_then(Value::as_array_mut) {
        for el in list.iter_mut() {
            if let Some(f) = el["handle"].as_str().and_then(cadaddy_doc::Handle::parse_hex).and_then(|h| raceway_facts.facts.get(&h)) {
                el["raceway"] = f.clone();
            }
        }
    }
    let n_raceways = raceway_facts.runs.len();
    here.objects["raceways"] = json!(raceway_facts.runs);
    here.objects["counts"]["raceways"] = json!(n_raceways);
    here.counts["raceways"] = json!(n_raceways);
    // Each element's expected height and where the documents give it (G-HEIGHT's reading).
    if let Some(list) = here.objects.get_mut("elements").and_then(Value::as_array_mut) {
        for el in list.iter_mut() {
            let Some(h) = el.get("handle").and_then(Value::as_str).map(str::to_string) else { continue };
            if let Some(f) = pre.findings.iter().find(|f| f["rule"] == "G-HEIGHT" && f["handle"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(&h)))
            {
                el["height_expected"] = f["measured"].get("expected_source").cloned().unwrap_or(Value::Null);
                el["height_model_from"] = f["measured"].get("model_height_from").cloned().unwrap_or(Value::Null);
                el["height_verdict"] = f["verdict"].clone();
            }
        }
    }
    // Each product's record and its G-PRODUCT / G-PRODUCT-CODE verdicts; the spec pages the
    // records cite (for the pictures).
    let spec_pages = match here.objects.get_mut("elements") {
        Some(Value::Array(els)) => super::validate_product::annotate(d, els, &pre.findings),
        _ => Vec::new(),
    };
    here.objects["prescreen"] = json!({
        "about": "measured without an agent: G-HOST the distance to the nearest wall face (inches, negative inside the wall), G-HEIGHT the height above the floor against the height the documents give (an AFF note by it, the interior elevation, the legend, a schedule, the general notes, then a project rule validate.height.<kind|mark>; `expected_source` says the sheet, the point and the words; `default`: a model height no document gives), G-PRESENT each symbol the room's enlarged plans (else its level's trade plans) show against the elements of the same kind within plan_in (missing: sheet and point), elements no symbol stands for (extra), and the room type's project-rule set, E-RECEPT-TR each receptacle's tamper-resistance where the code wants it (its fact and source are on the element too, `tamper_resistant`); the lighting rules: E-LUM-TYPE each luminaire's type against the fixture schedule and the submittal, E-LUM-CONTROL its control, E-SWITCH-LEG each control's switch leg, E-SWITCH-DIM a dimmer's drivers, E-SWITCH-3WAY a 3-way switch's mate, E-LUM-COUNT the plan's symbols per room and type against the model's, G-KEYNOTE each keynote marker's called box, device, conduit or sleeve against the model at the marker (its note, size, route and the rated walls and floors keynote conduit passes), G-PRODUCT each product's record field by field (maker, catalog, colour, configuration, rating, features) against its submittal and specification section (`check` the field; unsure: no product data), G-PRODUCT-CODE each receptacle's code-required features where it is (tamper-resistant, GFCI judged on its circuit with the breaker's schedule row or the project's as-built breaker rule that outranks it, weather-resistant); each element's `product_record` and `product_checks` are on it too; G-UNDERGROUND each callout for buried work against the model's runs below the slab and the underground sheets (size, count, material, least depth; `proposed`: a run CADaddy proposes, never a pass, with its review item; `extra`: a buried run nothing accounts for) and each stub-up's two dimensions from a slab edge or a grid line, E-GEAR-SPACE each piece of gear's working space (depth by its voltage to ground and condition, width, height, door swings), headroom and dedicated space, E-GEAR-FILL its raceways on each side's conduit area, gutter fill, bending space and lugs against the feeders that land on it (missing gutter or bending dimensions: unsure), E-GEAR-SIZE its built box against the submittal's; the routed raceway on the model's own pieces (each piece's `raceway`: a stick's ends and heights, what each end touches, the wires in it; a box's volume and the sticks entering it), E-CONDUIT-CONTINUITY each device joined end to end to its panel, E-CONDUIT-FILL and E-DERATE each run's conductors, E-BOX-FILL each box's conductors against its volume, E-VDROP each circuit's length and conductor, E-METHOD each run's wiring method, G-CLASH each stick's clearances, E-TAKEOFF the takeoff against a recount; each of these names in `handles` every piece of this area it covers (`handles_elsewhere`: the pieces of other areas it covers). A `conflict` verdict: the documents disagree (both are cited in `documents`). A finding on an element carries its `provenance` (when not drawn) and `corrections`; `judge_on_basis`: a fail that was only the correction itself (the documents' design value against the corrected one), made unsure, to be judged on the correction's basis",
        "checked": pre.checked.iter().chain(raceway_checked.iter()).collect::<BTreeMap<_, _>>(),
        "unmeasured": pre.unmeasured.iter().chain(raceway_unmeasured.iter()).collect::<BTreeMap<_, _>>(),
        // The area's own: an element beside it is measured in its own area's packet.
        "findings": pre
            .findings
            .iter()
            .filter(|f| f["handle"].as_str().is_none_or(|h| !here.nearby.contains(&h.to_ascii_uppercase())))
            .take(MAX_OBJECTS)
            .chain(raceway_findings.iter())
            .collect::<Vec<_>>(),
    });
    // What each element and run is by the documents and what the build corrected, and the
    // project's rules with their tiers (Q6259): the judge judges a correction on its basis.
    let basis_counts = vb::Basis::of(d).annotate(&mut here.objects);
    let (project_rules, project_rules_total) = vb::project_rules(d);
    here.objects["project_rules"] = json!(project_rules);
    here.objects["counts"]["project_rules"] = json!(project_rules_total);
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
    // The gear in the area: its facts, the riser and schedules to show whole, its submittal pages;
    // its kinds bring in the gear rules.
    let mut gear_crops: Vec<super::validate_keynote::CropAsk> = Vec::new();
    let mut gear_pages: Vec<(String, String)> = Vec::new();
    // The job's imported sheets that are no plan link (schedules, diagrams): number, kind, rectangle.
    let regions: Vec<(String, String, [f64; 4])> = {
        let mut seen: BTreeSet<String> = sp.sheets.iter().map(|l| l.number.to_ascii_uppercase()).collect();
        job.as_ref()
            .map(|j| j.sheet_regions.as_slice())
            .unwrap_or_default()
            .iter()
            .filter(|r| seen.insert(r.number.to_ascii_uppercase()))
            .map(|r| (r.number.clone(), r.kind.clone(), r.rect))
            .collect()
    };
    if let Some((gear_json, asks, pages, kinds)) = super::validate_gear::for_packet(
        d,
        &sp,
        &frame,
        &area,
        gear_docs.as_ref(),
        &regions,
        &super::validate_gear::job_pages(job.as_ref().and_then(|j| j.sheets.as_ref())),
    ) {
        let n = gear_json["gear"].as_array().map_or(0, Vec::len);
        here.objects["gear"] = gear_json;
        here.objects["counts"]["gear"] = json!(n);
        here.counts["gear"] = json!(n);
        here.kinds.extend(kinds);
        gear_crops = asks;
        gear_pages = pages;
    }
    let (mut rules_on, mut rules_off) = rules_for(&here, &all_rules);
    // Only the rules a validate run asked for (`rules`): the others are listed as left out.
    if let Some(only) = &only_rules {
        rules_off.extend(rules_on.iter().filter(|r| !only.contains(&r.rule.id)).map(|r| r.rule.id.clone()));
        rules_on.retain(|r| only.contains(&r.rule.id));
    }
    // Each element's and run's `rules` (Q6263): the element × rule pairs this packet asks the
    // judge for, by the one applicability table (`Rule::asks`) the report and the status use. A
    // rule asked of no element only where its prescreen names one (value engineering with no
    // option here) is left out of the packet.
    let mut asked = ask_rules(&mut here.objects, &rules_on);
    let not_asked: Vec<String> = rules_on.iter().filter(|r| !vr_keeps(r, &asked)).map(|r| r.rule.id.clone()).collect();
    rules_on.retain(|r| vr_keeps(r, &asked));
    asked.retain(|id, _| !not_asked.contains(id));
    let rules_json = json!({
        "area": area.id,
        "kinds": here.kinds.iter().collect::<Vec<_>>(),
        "pairs_asked": asked,
        "not_asked": not_asked,
        "rules": rules_on.iter().map(|r| r.to_json(true)).collect::<Vec<_>>(),
        "disabled": rules_off,
        "rejected_overrides": rejected.iter().map(|r| json!({"key": r.key, "value": r.value, "why": r.why})).collect::<Vec<_>>(),
    });

    // The documents that decide (Q6261): the sheets the area's prescreen cites, the schedule,
    // legend and notes sheets of its trades, and its ceiling height.
    let job_pages = super::validate_gear::job_pages(job.as_ref().and_then(|j| j.sheets.as_ref()));
    let numbers = super::validate_docs::numbers(
        sp.sheets.iter().map(|l| l.number.as_str()).chain(regions.iter().map(|r| r.0.as_str())).chain(job_pages.iter().map(|p| p.number.as_str())),
    );
    let sheet_rects = |n: &str| -> Vec<[f64; 4]> {
        sp.sheets
            .iter()
            .filter(|l| l.number.eq_ignore_ascii_case(n))
            .filter_map(|l| l.rect.map(norm_rect))
            .chain(regions.iter().filter(|r| r.0.eq_ignore_ascii_case(n)).map(|r| norm_rect(r.2)))
            .collect()
    };
    let cites = super::validate_docs::cited(here.objects["prescreen"]["findings"].as_array().into_iter().flatten(), &numbers, &sheet_rects);
    let trades_here: BTreeSet<String> = ["elements", "runs"]
        .iter()
        .flat_map(|k| here.objects[*k].as_array().into_iter().flatten())
        .filter_map(|e| e.get("trade").and_then(Value::as_str).map(str::to_string))
        .collect();
    let doc_sheets = {
        let title_of = |n: &str| job_pages.iter().find(|p| p.number.eq_ignore_ascii_case(n)).map(|p| p.words.clone()).unwrap_or_default();
        let job_kind = |n: &str| {
            job.as_ref().and_then(|j| j.sheet_regions.iter().find(|r| r.number.eq_ignore_ascii_case(n))).map(|r| r.kind.clone()).unwrap_or_default()
        };
        let mut all: Vec<super::validate_docs::SheetOf> = sp
            .sheets
            .iter()
            .filter(|l| l.view.is_none())
            .filter_map(|l| {
                Some(super::validate_docs::SheetOf {
                    number: l.number.clone(),
                    title: format!("{} {}", l.title, title_of(&l.number)),
                    job_kind: job_kind(&l.number),
                    discipline: l.discipline.clone().unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(&l.number)),
                    rect: Some(l.rect?),
                    has_level: l.level.is_some() || !l.levels.is_empty(),
                })
            })
            .collect();
        for r in job.as_ref().map(|j| j.sheet_regions.as_slice()).unwrap_or_default().iter().take(MAX_FOLDER_FILES) {
            if sp.sheets.iter().any(|l| l.number.eq_ignore_ascii_case(&r.number)) {
                continue;
            }
            all.push(super::validate_docs::SheetOf {
                number: r.number.clone(),
                title: title_of(&r.number),
                job_kind: r.kind.clone(),
                discipline: if r.discipline.trim().is_empty() { cadaddy_mepfix::sheet_facts::discipline_of(&r.number) } else { r.discipline.clone() },
                rect: Some(r.area.unwrap_or(r.rect)),
                has_level: matches!(r.kind.as_str(), "plan" | "enlarged_plan" | "rcp"),
            });
        }
        // The sheets only the job's index has (the job imported only its plans): their PDF pages.
        let held: BTreeSet<String> = all.iter().map(|x| x.number.to_ascii_uppercase()).collect();
        for p in job_pages.iter().take(MAX_FOLDER_FILES) {
            if held.contains(&p.number.to_ascii_uppercase()) {
                continue;
            }
            all.push(super::validate_docs::SheetOf {
                number: p.number.clone(),
                title: p.words.clone(),
                job_kind: String::new(),
                discipline: if p.discipline.trim().is_empty() { cadaddy_mepfix::sheet_facts::discipline_of(&p.number) } else { p.discipline.clone() },
                rect: None,
                // The index's type words follow its title: a plan's page is never a schedule.
                has_level: p.words.split_whitespace().rev().take(2).any(|w| matches!(w, "plan" | "enlarged_plan" | "rcp")),
            });
        }
        super::validate_docs::sheets(&all, &trades_here)
    };
    let ceiling = super::validate_docs::ceiling(&sp, &frame, &area);
    here.objects["ceiling"] = ceiling.clone();

    // The print crops.
    let mut within: Vec<[f64; 4]> = sp.sheets.iter().filter_map(|l| l.rect.map(norm_rect)).collect();
    within.extend(regions.iter().map(|r| norm_rect(r.2)));
    let texts = Texts::of(d, &within);
    // The keynote lists and markers that mark the area (G-KEYNOTE's reading).
    let keynotes = super::validate_keynote::Keynotes::read(d, &sp, &frame);
    let level_plans: BTreeSet<String> = sp
        .sheets
        .iter()
        .filter(|l| shows_level(l, &area.level))
        .filter(|l| {
            matches!(sheet_kind(&format!("{} {}", l.title, l.view.as_ref().map_or("", |v| v.title.as_str())), true), "plan" | "rcp" | "enlarged_plan")
        })
        .map(|l| l.number.to_ascii_uppercase())
        .collect();
    let (keynotes_json, mut keynote_crops) = super::validate_keynote::for_packet(&keynotes, &area, margin, &level_plans);
    keynote_crops.extend(gear_crops);
    let marker_count = keynotes_json["markers"].as_array().map_or(0, Vec::len);
    if marker_count > 0 || keynotes_json["lists"].as_array().is_some_and(|l| !l.is_empty()) {
        here.objects["keynotes"] = keynotes_json;
        here.objects["counts"]["keynote_markers"] = json!(marker_count);
        here.counts["keynote_markers"] = json!(marker_count);
    }
    // The level's buried work and its underground sheets (G-UNDERGROUND's reading): the
    // generated sheets go in whole as pictures.
    let mut ug_layouts: Vec<(String, String)> = Vec::new();
    if let Some((ug_json, layouts)) = super::validate_underground::for_packet(d, &sp, &frame, &area.level) {
        let callouts = ug_json["callouts"].as_array().map_or(0, Vec::len);
        here.objects["underground"] = ug_json;
        here.objects["counts"]["underground_callouts"] = json!(callouts);
        here.counts["underground_callouts"] = json!(callouts);
        here.counts["underground_sheets"] = json!(layouts.len());
        ug_layouts = layouts;
    }
    // Each placed crop's registration against the model's walls (Q6141).
    let (reg_threshold, reg_rule) = vreg::threshold(&d.project_rules);
    // The level's sheets' objects, once, for their straight lines.
    let level_sheets: Vec<[f64; 4]> = sp.sheets.iter().filter(|l| names_level(l, &area.level)).filter_map(|l| l.rect.map(norm_rect)).collect();
    let line_index = PrintIndex::of(d, &level_sheets);
    let register = |place: &Place, r: [f64; 4]| -> vreg::Registered {
        let unit = upf * place.scale();
        vreg::judge(
            vreg::fit_walls(
                &wall_faces(&model, &frame, &area, margin, place, r),
                &sheet_lines(d, &line_index, grow(r, vreg::REACH_FT * unit)),
                unit,
                [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0],
            ),
            reg_threshold,
        )
    };
    // A unit type's views, each measured on the room's walls for its turn and mirror (Q6339).
    let view_rects: Vec<[f64; 4]> = unit.as_ref().map_or_else(Vec::new, |u| {
        let mut r: Vec<[f64; 4]> = sp.sheets.iter().filter_map(|l| l.rect.map(norm_rect)).collect();
        r.extend(type_view_rects(&u.ty));
        r
    });
    let unit_views = UnitViews::of(d, &view_rects, upf);
    let measure = |r: [f64; 4]| unit_views.turn(r, &model, &area.level, area.bbox);
    let in_drawing: Vec<super::validate_docs::DocSheet> = doc_sheets.0.iter().filter(|x| x.rect.is_some()).cloned().collect();
    let docs = DocInput { sheets: &in_drawing, cites: &cites, regions: &regions };
    let Planned { mut crops, mut left_out, mut not_shown } =
        plan_crops(&sp, &frame, &area, margin, dpi, &trades, &texts, &here, unit.as_ref(), fit.as_ref(), &keynote_crops, &docs, &register, &measure);
    left_out.extend(doc_sheets.1.iter().cloned());
    // The deciding sheets the drawing doesn't hold (cited, or a schedule, legend or notes sheet
    // only the job's index has): their PDF pages, whole, at the packet's dpi.
    let mut pdf_asks: Vec<(String, &'static str, &'static str, String)> = Vec::new();
    not_shown.retain(|(what, why)| {
        let n = what.trim_start_matches("sheet ").trim();
        match job_pages.iter().find(|p| p.number.eq_ignore_ascii_case(n)) {
            Some(p) => {
                let trade = trade_of_discipline(&if p.discipline.trim().is_empty() {
                    cadaddy_mepfix::sheet_facts::discipline_of(n)
                } else {
                    p.discipline.clone()
                });
                pdf_asks.push((
                    p.number.clone(),
                    sheet_kind(&p.words, false),
                    trade,
                    why.trim_end_matches(", but the drawing holds no such sheet").to_string(),
                ));
                false
            }
            None => true,
        }
    });
    for x in doc_sheets.0.iter().filter(|x| x.rect.is_none() && want_trade(&trades, x.trade)) {
        if !pdf_asks.iter().any(|a| a.0.eq_ignore_ascii_case(&x.number)) {
            pdf_asks.push((
                x.number.clone(),
                x.kind,
                x.trade,
                format!("the {} sheet of this area's trades ({}), whole", x.kind.replace('_', " "), x.trade),
            ));
        }
    }
    let mut pdf_search: Vec<String> = job_prints.iter().cloned().collect();
    pdf_search.extend(prints_dir.clone());
    let pdf_docs = pdf_pages(&pdf_asks, &job_pages, &pdf_search, dpi, &mut not_shown);
    // Each picture is labelled by the view it shows on its own sheet (a tile by the view it
    // lies in), not by the link that brought it in.
    let views = sheet_views(d, &sp);
    for c in crops.iter_mut() {
        if let Some(v) = view_at(&views, &c.sheet, c.rect).or_else(|| view_at(&views, &c.sheet, c.whole)) {
            c.view = Some(v);
        }
    }
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
        // The level's built model is drawn where the build put it (stacked over the lowest
        // level's plan when the build stacked the levels), not over the level's own sheet.
        let built_box = {
            let pts: Vec<[f64; 2]> = corners(grow(area.bbox, margin))
                .iter()
                .map(|q| {
                    let v = frame.to_built(&area.level, *q);
                    [v.x, v.y]
                })
                .collect();
            bbox(&pts).unwrap_or(proj_box)
        };
        let around = json!([r2(built_box[0]), r2(built_box[1]), r2(built_box[2]), r2(built_box[3])]);
        model_imgs.push(ModelImage {
            file: "model/3d.png".into(),
            kind: "3d",
            what: "the area in 3D (level's model inside the box, from the south-west, above)".into(),
            of: None,
            rect: None,
            px: Some([1200, 900]),
            wall: None,
            render: Some(json!({"width": 1200, "height": 900, "level": area.level, "around": around.clone()})),
            over: None,
        });
        let z = level_spec.map_or(0.0, |l| l.elevation_ft) * upf + 5.0 * upf;
        // One elevation per side of the area, the camera inside the area looking square at it: a
        // side is the outline's edges facing one way that lie outermost that way (within
        // SIDE_DEPTH_FT: a ragged outline's jogs), longest side first; a wall fin or notch inside
        // the area is no side (Q6339). The camera stands before the side's longest edge it can.
        let outline: Vec<[f64; 2]> =
            if area.outline.len() >= 3 { area.outline.iter().take(MAX_OUTLINE).copied().collect() } else { corners(area.bbox).to_vec() };
        let sides = area_sides(&outline);
        let mut cameras: Vec<([f64; 2], [f64; 2])> = Vec::new();
        for side in sides {
            let len = side.len;
            let label = format!("side {} of the outline ({len:.1} ft)", side.first + 1);
            // How far back the camera can stand inside the area before one of the side's edges.
            let placed = side.edges.iter().find_map(|&(a, b)| {
                let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                let (nx, ny) = (-side.dir[0], -side.dir[1]);
                let back = [6.0, 4.0, 3.0, 2.0, 1.0]
                    .into_iter()
                    .find(|d| (1..=4).all(|k| point_in(&outline, [m[0] + nx * d * f64::from(k) / 4.0, m[1] + ny * d * f64::from(k) / 4.0])))?;
                Some((m, back))
            });
            let Some((m, back)) = placed else {
                left_out.push((format!("model elevation of {label}"), "the area is too narrow there for a camera inside it".into()));
                continue;
            };
            let (nx, ny) = (-side.dir[0], -side.dir[1]);
            let eye_ft = [m[0] + nx * back, m[1] + ny * back];
            if cameras.len() >= MAX_ELEVATIONS {
                left_out.push((
                    format!("model elevation of {label}"),
                    format!("over the packet's {MAX_ELEVATIONS} wall elevations (the longest sides are drawn)"),
                ));
                continue;
            }
            cameras.push((eye_ft, side.dir));
            // The model wall the camera faces: the first its line of sight crosses.
            let faced = faced_wall(&model, &area.level, eye_ft, side.dir, back + SIDE_DEPTH_FT + 1.0);
            let wall = faced.map(|(w, _)| w.id.clone());
            let opening = faced_opening(&model, &area.level, eye_ft, side.dir, back + SIDE_DEPTH_FT + 1.0)
                .filter(|k| !k.trim().is_empty())
                .map(|k| format!(", a {} in it", k.trim().to_ascii_lowercase()))
                .unwrap_or_default();
            let on_wall = match faced {
                None => format!(", no model wall on it{opening}"),
                Some((w, at)) if at >= back - SIDE_DEPTH_FT - 0.5 => format!(", wall {}{opening}", w.id),
                Some((w, at)) => format!(", facing wall {} {at:.1} ft from the camera, in front of the side", w.id),
            };
            let e = frame.to_built(&area.level, eye_ft);
            let t = frame.to_built(&area.level, m);
            let k = cameras.len();
            model_imgs.push(ModelImage {
                file: format!("model/elevation_{k:02}.png"),
                kind: "elevation",
                what: format!("the {} side of the area ({len:.1} ft{on_wall}) seen from inside it, {back:.0} ft back", compass(side.dir[0], side.dir[1])),
                of: None,
                rect: None,
                px: Some([1200, 900]),
                wall,
                render: Some(json!({"width": 1200, "height": 900, "level": area.level, "eye": [r2(e.x), r2(e.y), r2(z)], "target": [r2(t.x), r2(t.y), r2(z)], "around": around.clone()})),
                over: None,
            });
        }
    }

    // Submittal pages: those the area's elements carry, and the job's submittals matched to the
    // area's marks and kinds (what the job applied to elements of the same type or kind
    // elsewhere in the building).
    let applied = applied_submittals(d, &kinds);
    let mut wanted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (kind, mark, sub) in &here.products {
        let why = mark.as_ref().map_or_else(|| format!("kind {kind}"), |m| format!("mark {m}"));
        if let Some(sb) = sub {
            wanted.entry(sb.clone()).or_default().insert(why);
            continue;
        }
        let by_mark: Vec<&String> = mark
            .as_ref()
            .map(|m| applied.iter().filter_map(|(r, a)| a.marks.contains(&m.to_ascii_uppercase()).then_some(r)).collect())
            .unwrap_or_default();
        if !by_mark.is_empty() {
            for r in by_mark.into_iter().take(MAX_KIND_PAGES) {
                wanted.entry(r.clone()).or_default().insert(format!("{why} (the job's submittal for its type; this element carries none)"));
            }
            continue;
        }
        for r in applied.iter().filter(|(_, a)| a.is_for(kind)).map(|(r, _)| r).take(MAX_KIND_PAGES) {
            wanted
                .entry(r.clone())
                .or_default()
                .insert(format!("kind {kind} (the job's submittal for that kind elsewhere; no element here carries one)"));
        }
    }
    // The cut-sheet pages of the luminaire types present.
    for page in &here.cut_sheets {
        wanted.entry(page.clone()).or_default().insert("a luminaire type's cut sheet".to_string());
    }
    // The gear submittal pages of the gear here.
    for (page, why) in &gear_pages {
        wanted.entry(page.clone()).or_default().insert(why.clone());
    }
    let mut search: Vec<String> = job_submittals.clone();
    search.extend(job_prints.clone());
    let mut subs: Vec<(String, String, u32, Option<std::path::PathBuf>, Vec<String>)> = Vec::new();
    // A submittal the job applied but does not list: which, and what it was applied to.
    let mut unlisted: BTreeMap<String, (BTreeSet<u32>, BTreeSet<String>)> = BTreeMap::new();
    for (i, (sb, why)) in wanted.iter().enumerate() {
        let (name, page) = submittal_ref(sb);
        match find_submittal(&search, &name) {
            Some(path) => subs.push((
                format!("prints/s{:02}_submittal_{}_p{page}.png", i + 1, safe(&name)),
                sb.clone(),
                page,
                Some(path),
                why.iter().cloned().collect(),
            )),
            None => {
                let u = unlisted.entry(name).or_default();
                u.0.insert(page);
                u.1.extend(why.iter().cloned());
            }
        }
    }
    // What the job lists: names only (never a whole path), and whether this machine has it.
    let listed: Vec<String> = job_submittals
        .iter()
        .take(20)
        .map(|p| format!("{}{}", last_part(p), if local_path(p).exists() { "" } else { " (not found on this machine)" }))
        .collect();
    let mut unlisted_json: Vec<Value> = Vec::new();
    for (name, (pages, why)) in &unlisted {
        let on = applied.iter().filter(|(r, _)| submittal_ref(r).0 == *name).fold((0usize, BTreeSet::new()), |(n, mut m), (_, a)| {
            m.extend(a.marks.iter().cloned());
            (n + a.elements, m)
        });
        let marks: Vec<String> = on.1.into_iter().take(20).collect();
        left_out.push((
            format!("submittal {name}"),
            format!(
                "the job applied it to {} element(s) (marks {}), but its file is not among the job's submittals ({}): give it to bim.submittals {{paths}}",
                on.0,
                if marks.is_empty() { "none".into() } else { marks.join(", ") },
                if listed.is_empty() { "the job lists none".to_string() } else { format!("the job lists {}", listed.join(", ")) }
            ),
        ));
        unlisted_json
            .push(json!({"submittal": name, "pages": pages, "for": why.iter().take(20).collect::<Vec<_>>(), "applied_to": on.0, "marks": marks}));
    }

    // The specification pages the area's product records cite: a section's own file (named for
    // its number), at the page its footer gives.
    let mut spec_paths: Vec<String> = given_specs.clone();
    spec_paths.extend(job.as_ref().map(|j| j.specs.iter().chain(j.specs_found.iter().flatten()).cloned().collect::<Vec<_>>()).unwrap_or_default());
    let spec_files = if spec_pages.is_empty() || spec_paths.is_empty() { Vec::new() } else { super::products::spec_files(&spec_paths, &[]) };
    let mut specs: Vec<SpecPage> = Vec::new();
    for (section, page, product, kinds) in spec_pages.iter().take(MAX_SPEC_PAGES) {
        let what = format!("spec {section} p.{page}");
        let file = spec_file(&spec_files, section);
        let n = footer_page(page);
        match (file, n) {
            (Some(path), Some(n)) => specs.push(SpecPage {
                file: format!("prints/p{:02}_spec_{}_p{n}.png", specs.len() + 1, safe(section)),
                section: section.clone(),
                page: n,
                path: std::path::PathBuf::from(path),
                product: product.clone(),
                kinds: kinds.iter().cloned().collect(),
            }),
            (None, _) => left_out.push((
                what,
                if spec_paths.is_empty() {
                    "the job keeps no specification files".to_string()
                } else {
                    format!("no file of section {section} among the job's specifications (a project manual's pages aren't located)")
                },
            )),
            (Some(_), None) => left_out.push((what, "its page isn't a page number".to_string())),
        }
    }
    for x in spec_pages.iter().skip(MAX_SPEC_PAGES) {
        left_out.push((format!("spec {} p.{}", x.0, x.1), format!("over the packet's {MAX_SPEC_PAGES} specification pages")));
    }

    // The cap (Q6261): the documents that decide (the crops marked `deciding`, the submittal and
    // specification pages) outrank it, up to MAX_IMAGES pictures in all. Under the cap
    // (`max_images`): the area's own crops (placed, the unit type's views, the keynote markers),
    // each followed by its overlay; the model alone, in 3D and its elevations; the other crops;
    // last, a level's sheet shown whole that no element here was read from.
    let mut deciding: Vec<(u8, usize)> = Vec::new();
    let mut order: Vec<(u8, usize)> = Vec::new();
    let whole_only = |c: &Crop| c.placed_by == "unregistered" && !c.cited;
    let own = |c: &Crop| !matches!(c.placed_by, "unregistered" | "text" | "document" | "cited_point");
    for (i, c) in crops.iter().enumerate() {
        if c.deciding.is_some() {
            deciding.push((0, i));
        }
    }
    for k in 0..subs.len() {
        deciding.push((2, k));
    }
    for k in 0..specs.len() {
        deciding.push((3, k));
    }
    // The PDF pages' tiles: (doc, part, file).
    let pdf_tiles: Vec<(usize, usize, String)> = pdf_docs
        .iter()
        .enumerate()
        .flat_map(|(i, doc)| {
            let many = doc.parts.len() > 1;
            (0..doc.parts.len()).map(move |k| {
                let t = doc.parts.get(k).map_or(0, |p| p.2[0]);
                let file = if many {
                    format!("prints/d{:02}_{}_{}_t{t}.png", i + 1, safe(&doc.number), doc.kind)
                } else {
                    format!("prints/d{:02}_{}_{}.png", i + 1, safe(&doc.number), doc.kind)
                };
                (i, k, file)
            })
        })
        .collect();
    for k in 0..pdf_tiles.len() {
        deciding.push((4, k));
    }
    // A deciding crop is kept already; its overlay waits its turn under the cap.
    let with_overlay = |order: &mut Vec<(u8, usize)>, i: usize, c: &Crop| {
        if c.deciding.is_none() {
            order.push((0, i));
        }
        if c.place.is_some()
            && let Some(j) = model_imgs.iter().position(|m| m.over == Some(i))
        {
            order.push((1, j));
        }
    };
    for (i, c) in crops.iter().enumerate().filter(|(_, c)| own(c) && !whole_only(c)) {
        with_overlay(&mut order, i, c);
    }
    for (j, _) in model_imgs.iter().enumerate().filter(|(_, m)| m.over.is_none() && m.kind == "plan") {
        order.push((1, j));
    }
    for (j, _) in model_imgs.iter().enumerate().filter(|(_, m)| m.over.is_none() && m.kind != "plan") {
        order.push((1, j));
    }
    for (i, c) in crops.iter().enumerate().filter(|(_, c)| !own(c) && !whole_only(c)) {
        with_overlay(&mut order, i, c);
    }
    for (i, c) in crops.iter().enumerate().filter(|(_, c)| whole_only(c)) {
        with_overlay(&mut order, i, c);
    }
    let n_deciding = deciding.len().min(MAX_IMAGES);
    let room = max_images.min(MAX_IMAGES - n_deciding);
    let mut kept: BTreeSet<(u8, usize)> = deciding.iter().take(n_deciding).copied().collect();
    kept.extend(order.iter().filter(|x| !kept.contains(x)).take(room).copied().collect::<Vec<_>>());
    let file_of = |t: u8, i: usize| match t {
        0 => crops.get(i).map(|c| c.file.clone()),
        1 => model_imgs.get(i).map(|m| m.file.clone()),
        3 => specs.get(i).map(|x| x.file.clone()),
        4 => pdf_tiles.get(i).map(|x| x.2.clone()),
        _ => subs.get(i).map(|x| x.0.clone()),
    };
    for (t, i) in deciding.iter().skip(n_deciding) {
        if let Some(f) = file_of(*t, *i) {
            left_out.push((f, format!("a deciding document over the packet's most of {MAX_IMAGES} pictures")));
        }
    }
    for (t, i) in order.iter().filter(|x| !kept.contains(x)) {
        if let Some(f) = file_of(*t, *i) {
            left_out.push((f, format!("over the packet's cap of {max_images} pictures (max_images; the deciding documents are kept beside it)")));
        }
    }
    let crops: Vec<(usize, &Crop)> = crops.iter().enumerate().filter(|(i, _)| kept.contains(&(0, *i))).collect();
    let model_imgs_kept: Vec<&ModelImage> = model_imgs.iter().enumerate().filter(|(j, _)| kept.contains(&(1, *j))).map(|(_, m)| m).collect();
    let subs: Vec<&(String, String, u32, Option<std::path::PathBuf>, Vec<String>)> =
        subs.iter().enumerate().filter(|(k, _)| kept.contains(&(2, *k))).map(|(_, x)| x).collect();
    let specs: Vec<&SpecPage> = specs.iter().enumerate().filter(|(k, _)| kept.contains(&(3, *k))).map(|(_, x)| x).collect();
    let pdf_tiles: Vec<&(usize, usize, String)> = pdf_tiles.iter().enumerate().filter(|(k, _)| kept.contains(&(4, *k))).map(|(_, x)| x).collect();

    // Every file is checked before any is written.
    let dir = std::path::Path::new(&out_dir);
    let full = |f: &str| dir.join(f).to_string_lossy().into_owned();
    let mut files: Vec<String> = ["manifest.json", "objects.json", "rules.json", "prompt.md"].iter().map(|f| full(f)).collect();
    files.extend(crops.iter().map(|(_, c)| full(&c.file)));
    files.extend(model_imgs_kept.iter().map(|m| full(&m.file)));
    files.extend(subs.iter().map(|x| full(&x.0)));
    files.extend(specs.iter().map(|x| full(&x.file)));
    files.extend(pdf_tiles.iter().map(|x| full(&x.2)));
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
    // A picture is listed only once its file is there.
    let written = |file: &str| std::fs::metadata(full(file)).is_ok_and(|m| m.is_file() && m.len() > 0);
    let write_png = |file: &str, png: Option<Vec<u8>>, left_out: &mut Vec<(String, String)>| -> Result<bool> {
        match png {
            Some(b) => {
                super::exchange::write_file(CMD, &full(file), &b)?;
                if written(file) {
                    Ok(true)
                } else {
                    left_out.push((file.to_string(), "the picture was drawn but its file isn't there".into()));
                    Ok(false)
                }
            }
            None => {
                left_out.push((file.to_string(), "the picture couldn't be drawn".into()));
                Ok(false)
            }
        }
    };
    // The prints as their pages show them.
    let tables = print_tables(d);
    let mut prints_out: Vec<Value> = Vec::new();
    let mut inks: Vec<(usize, &Crop, Ink)> = Vec::new();
    for (_, c) in &crops {
        let ents = as_printed(d, &index.within(c.rect));
        match png_and_ink(&tables, &ents, c.rect, c.px) {
            Some((png, ink)) => {
                if write_png(&c.file, Some(png), &mut left_out)? {
                    prints_out.push(c.to_json());
                    inks.push((prints_out.len() - 1, *c, ink));
                }
            }
            None => {
                write_png(&c.file, None, &mut left_out)?;
            }
        }
    }
    // The generated underground sheets of the area's level, whole (no PDF page to compare).
    for (k, (number, layout)) in ug_layouts.iter().enumerate().take(MAX_UNDERGROUND_SHEETS) {
        let Some(lay) = d.layouts.iter().find(|l| &l.name == layout) else { continue };
        let ents: Vec<&Entity> = lay.entities.iter().map(|e| e.as_ref()).take(MAX_CROP_ENTITIES).collect();
        let b = ents.iter().map(|e| cadaddy_doc::entity_bounds(d, e, 0)).filter(|b| !b.is_empty()).reduce(|a, b| a.union(&b));
        let Some(b) = b.filter(|b| b.width() > 0.0 && b.height() > 0.0 && b.width().is_finite() && b.height().is_finite()) else { continue };
        // Paper inches at the packet's dpi, the long side at most MAX_UNDERGROUND_PX.
        let ppu = dpi.min(MAX_UNDERGROUND_PX / b.width().max(b.height()));
        let px = [(b.width() * ppu).round().max(1.0) as u32, (b.height() * ppu).round().max(1.0) as u32];
        let rect = [b.min.x, b.min.y, b.max.x, b.max.y];
        let file = format!("prints/ug{:02}_{}_underground_sheet.png", k + 1, safe(number));
        if write_png(&file, png_of(d, &ents, rect, px), &mut left_out)? {
            prints_out.push(json!({
                "file": file, "sheet": number, "title": layout, "kind": "underground_sheet", "trade": "electrical", "layout": layout,
                "sheet_rect": [r4(rect[0]), r4(rect[1]), r4(rect[2]), r4(rect[3])], "px": px, "px_per_unit": r4(ppu), "dpi": r2(ppu),
                "placed_by": "generated", "generated": true,
            }));
        }
    }
    // The deciding sheets the drawing doesn't hold, as their PDF pages (no page to compare: they
    // are the page).
    for (i, k, file) in &pdf_tiles {
        let (Some(doc), Some(part)) = (pdf_docs.get(*i), pdf_docs.get(*i).and_then(|d| d.parts.get(*k))) else { continue };
        let (r, px, t) = *part;
        if write_png(file, render_drawing(&doc.pd, r, px), &mut left_out)? {
            let mut o = json!({
                "file": file, "sheet": doc.number, "title": doc.page, "kind": doc.kind, "trade": doc.trade,
                "sheet_rect": [r4(r[0]), r4(r[1]), r4(r[2]), r4(r[3])], "px": px, "px_per_unit": r4(doc.ppu), "dpi": r2(doc.dpi),
                "paper": doc.paper, "placed_by": "pdf_page", "page": doc.page, "page_units": true,
                "shows": "the whole sheet, as its PDF page (sheet_rect in the page's own units)", "deciding": doc.why,
            });
            if doc.parts.len() > 1 {
                o["tile"] = json!(t);
            }
            prints_out.push(o);
        }
    }
    // Each crop against its sheet's PDF page read again, at the same rectangle and pixels.
    let similarity_json = {
        let refs = job.as_ref().map(|j| page_refs(j, prints_dir.as_deref())).unwrap_or_default();
        let sheets: BTreeSet<String> = inks.iter().map(|(_, c, _)| c.sheet.trim().to_ascii_uppercase()).collect();
        let mut low: Vec<Value> = Vec::new();
        let mut not_compared: Vec<Value> = Vec::new();
        let mut compared = 0usize;
        let mut read = 0usize;
        for sheet in &sheets {
            let why = if !compare {
                Some("not asked (compare: false)".to_string())
            } else {
                match refs.get(sheet) {
                    None => Some("the job keeps no PDF page for this sheet".to_string()),
                    Some(Err(e)) => Some(e.clone()),
                    Some(Ok(_)) if read >= MAX_COMPARE_SHEETS => Some(format!("over the {MAX_COMPARE_SHEETS} pages a packet reads again")),
                    Some(Ok(r)) => {
                        read += 1;
                        match read_page(r) {
                            Err(e) => Some(format!("{} p.{}: {e}", r.file, r.page)),
                            Ok(page) => {
                                for (k, c, ink) in inks.iter().filter(|(_, c, _)| c.sheet.trim().eq_ignore_ascii_case(sheet)) {
                                    let Some(o) = prints_out.get_mut(*k) else { continue };
                                    match page.ink(c.rect, c.px) {
                                        Some(pi) => {
                                            let sim = r2(similarity(ink, &pi));
                                            o["similarity"] = json!(sim);
                                            o["page"] = json!(format!("{} p.{}", r.file, r.page));
                                            compared += 1;
                                            if sim < MIN_SIMILARITY {
                                                o["similarity_low"] = json!(true);
                                                low.push(json!({"file": c.file, "similarity": sim}));
                                            }
                                        }
                                        None => o["similarity_why"] = json!("the page couldn't be drawn at this rectangle"),
                                    }
                                }
                                None
                            }
                        }
                    }
                }
            };
            if let Some(why) = why {
                not_compared.push(json!({"sheet": sheet, "why": why}));
            }
        }
        json!({
            "about": "each print crop against its sheet's PDF page read again as the page shows it (no text read off strokes, no masks), at the same rectangle and pixels: the harmonic mean of the share of each picture's ink the other has within 2 px (1 = alike, 0 = nothing in common)",
            "low_under": MIN_SIMILARITY,
            "compared": compared,
            "low": low,
            "not_compared": not_compared,
        })
    };
    let mut model_out: Vec<Value> = Vec::new();
    for m in &model_imgs_kept {
        let ok = match (m.kind, m.over, &m.render) {
            ("overlay", Some(i), _) => {
                let Some(c) = crops.iter().find(|(k, _)| *k == i).map(|(_, c)| *c) else { continue };
                let place = c.place.clone().unwrap_or(Place::Same);
                let od = model_drawing(d, &as_printed(d, &index.within(c.rect)), &model, &area, &frame, &place, &all_rules, &built, upf, &kinds);
                write_png(&m.file, render_drawing(&od, c.rect, c.px), &mut left_out)?
            }
            ("plan", _, _) => {
                let (Some(rect), Some(px)) = (m.rect, m.px) else { continue };
                let od = model_drawing(d, &[], &model, &area, &frame, &Place::Same, &all_rules, &built, upf, &kinds);
                write_png(&m.file, render_drawing(&od, rect, px), &mut left_out)?
            }
            (_, _, Some(r)) => {
                let mut q = r.clone();
                q["path"] = json!(full(&m.file));
                q["force"] = json!(true);
                match s.execute("model.render", &q) {
                    Ok(_) if written(&m.file) => true,
                    Ok(_) => {
                        left_out.push((m.file.clone(), "model.render wrote no file".into()));
                        false
                    }
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
    for (file, sb, page, path, why) in &subs {
        let Some(path) = path else { continue };
        match submittal_png(path, *page) {
            Ok(b) => {
                if write_png(file, Some(b), &mut left_out)? {
                    subs_out.push(json!({"file": file, "submittal": sb, "page": page, "for": why}));
                }
            }
            Err(e) => left_out.push((file.clone(), format!("submittal {sb}: {e}"))),
        }
    }
    let mut specs_out: Vec<Value> = Vec::new();
    for sp_page in &specs {
        match spec_png(&sp_page.path, sp_page.page, &sp_page.product) {
            Ok((b, cropped)) => {
                if write_png(&sp_page.file, Some(b), &mut left_out)? {
                    specs_out.push(json!({
                        "file": sp_page.file, "section": sp_page.section, "page": sp_page.page, "product": sp_page.product,
                        "for": sp_page.kinds, "cropped_to_product": cropped,
                    }));
                }
            }
            Err(e) => left_out.push((sp_page.file.clone(), format!("spec {} p.{}: {e}", sp_page.section, sp_page.page))),
        }
    }

    // The texts.
    let kept_crops: Vec<Crop> = crops.iter().map(|(_, c)| (*c).clone()).filter(|c| prints_out.iter().any(|j| j["file"] == json!(c.file))).collect();
    let kept_models: Vec<ModelImage> =
        model_imgs_kept.iter().map(|m| (*m).clone()).filter(|m| model_out.iter().any(|j| j["file"] == json!(m.file))).collect();
    let pages: Vec<(String, &'static str)> = subs_out
        .iter()
        .filter_map(|x| x["file"].as_str().map(|f| (f.to_string(), "submittal")))
        .chain(specs_out.iter().filter_map(|x| x["file"].as_str().map(|f| (f.to_string(), "spec"))))
        .chain(
            pdf_tiles
                .iter()
                .filter(|t| prints_out.iter().any(|o| o["file"] == json!(t.2)))
                .filter_map(|t| pdf_docs.get(t.0).map(|d| (t.2.clone(), d.kind))),
        )
        .collect();
    let mut prompt = prompt_md(&area, &kept_crops, &kept_models, &rules_on, &here.counts, &pages);
    // The documents that decide, and the ceiling height, before the rules (Q6261).
    let deciding_list: Vec<(String, String)> = prints_out
        .iter()
        .filter_map(|o| {
            let w = o["deciding"].as_str()?;
            let page = o["page"].as_str().map(|p| format!(" (PDF page {p})")).unwrap_or_default();
            Some((o["file"].as_str()?.to_string(), format!("sheet {} {}{page}: {w}", o["sheet"].as_str()?, o["kind"].as_str()?.replace('_', " "))))
        })
        .chain(
            subs_out.iter().map(|x| {
                (x["file"].as_str().unwrap_or_default().to_string(), format!("submittal page {}", x["submittal"].as_str().unwrap_or_default()))
            }),
        )
        .chain(specs_out.iter().map(|x| {
            (
                x["file"].as_str().unwrap_or_default().to_string(),
                format!(
                    "specification section {} p.{} ({})",
                    x["section"].as_str().unwrap_or_default(),
                    x["page"],
                    x["product"].as_str().unwrap_or_default()
                ),
            )
        }))
        .collect();
    let section = super::validate_docs::prompt_section(&deciding_list, &ceiling, &not_shown);
    match prompt.find("## Rules to check") {
        Some(at) => prompt.insert_str(at, &section),
        None => prompt.push_str(&section),
    }
    prompt = vb::with_section(prompt, &vb::prompt_section(&project_rules, project_rules_total, &basis_counts));
    if let Some(low) = similarity_json["low"].as_array().filter(|l| !l.is_empty()) {
        prompt.push_str("\n## Crops that may not look like their PDF page\n\n");
        for l in low {
            prompt.push_str(&format!(
                "- `{}`: similarity {} to the page: read it with care, and say so when a finding rests on it.\n",
                l["file"].as_str().unwrap_or_default(),
                l["similarity"]
            ));
        }
    }
    let registration_json = vreg::summary(&prints_out, reg_threshold, reg_rule.as_deref());
    prompt.push_str(&vreg::prompt_section(&prints_out));
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
        "submittals_unlisted": unlisted_json,
        "specs": specs_out,
        "similarity": similarity_json,
        "registration": registration_json,
        "documents": {
            "about": "the documents that decide heights, types and ratings (Q6261): every sheet the area's prescreen cites (`cited`: the rules citing it, the points it read, the pictures that show it), every schedule, legend and notes sheet of the area's trades, whole (`sheets`), and the ceiling height with its source; with the keynote lists, the gear's schedules and the submittal and specification pages they are the `deciding` pictures, which the picture cap never drops",
            "cited": cites.iter().map(|(n, c)| json!({
                "sheet": n,
                "rules": c.rules,
                "findings": c.findings,
                "points": c.points.len(),
                "files": prints_out.iter().filter(|x| x["sheet"].as_str().is_some_and(|s| s.eq_ignore_ascii_case(n)) && x.get("deciding").is_some()).filter_map(|x| x["file"].as_str()).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "sheets": doc_sheets.0.iter().map(|d| json!({
                "sheet": d.number,
                "kind": d.kind,
                "trade": d.trade,
                "files": prints_out.iter().filter(|x| x["sheet"].as_str().is_some_and(|s| s.eq_ignore_ascii_case(&d.number)) && (x["placed_by"] == "document" || x["placed_by"] == "pdf_page")).filter_map(|x| x["file"].as_str()).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "not_shown": not_shown.iter().map(|(w, y)| json!({"what": w, "why": y})).collect::<Vec<_>>(),
            "ceiling": ceiling,
        },
        "objects": "objects.json",
        "rules": "rules.json",
        "prompt": "prompt.md",
        "counts": {
            "elements": here.counts.get("elements"),
            "runs": here.counts.get("runs"),
            "walls": here.counts.get("walls"),
            "openings": here.counts.get("openings"),
            "rules": rules_on.len(),
            "provenance": basis_counts,
            "project_rules": project_rules_total,
            "pictures": prints_out.len() + model_out.len() + subs_out.len() + specs_out.len(),
            "deciding": deciding_list.len(),
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
    fn a_door_holder_is_the_fire_alarm_side_not_the_door() {
        assert_eq!(trade_of_kind("door_holder", &[]), "electrical");
        assert_eq!(trade_of_kind("door", &[]), "architecture");
        assert_eq!(trade_of_kind("duct_smoke_detector", &[]), "mechanical");
    }

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
    fn a_source_names_its_sheet_without_its_view() {
        assert_eq!(sheet_key("E-9/1").as_deref(), Some("E-9"));
        assert_eq!(sheet_key(" p-8/12 ").as_deref(), Some("P-8"));
        assert_eq!(sheet_key("A901").as_deref(), Some("A901"));
        assert_eq!(sheet_key("M-8/x").as_deref(), Some("M-8/X"));
        assert_eq!(sheet_key("room 512"), None);
        assert_eq!(sheet_key("NOTES"), None);
        assert_eq!(sheet_key(""), None);
    }

    #[test]
    fn a_title_names_its_level() {
        for (t, l) in [
            ("SECURITY - 1ST FLOOR", "Level 1"),
            ("LIGHTING PLAN - LEVEL 2", "L2"),
            ("EGRESS PLAN - 2ND-5TH FLOOR", "Level 3"),
            ("SIGNAGE - SECOND-FIFTH FLOOR PLAN", "Level 4"),
            ("SECOND FLOOR SLAB PLAN", "Level 2"),
            ("DATA - 2ND FLOOR (3RD FLOOR SIM.)", "Level 3"),
            ("POWER PLAN FLOOR 3", "03"),
            ("ROOF PLAN", "Roof"),
        ] {
            assert!(title_names_level(t, l), "{t} names {l}");
        }
        for (t, l) in [
            ("SECURITY - 1ST FLOOR", "Level 2"),
            ("EGRESS PLAN - 2ND-5TH FLOOR", "Level 1"),
            ("PANEL SCHEDULE 2", "Level 2"),
            ("ENLARGED UNIT PLAN", "Level 1"),
            ("ROOF DRAIN DETAILS", "Level 1"),
            ("1ST FLOOR", "T"),
        ] {
            assert!(!title_names_level(t, l), "{t} does not name {l}");
        }
    }

    #[test]
    fn a_narrow_crop_is_widened_on_its_sheet_and_views_are_compared() {
        let sheet = [0.0, 0.0, 100.0, 50.0];
        // A 2 × 40 sliver becomes 20 wide round its centre; one at the edge stays on the sheet.
        assert_eq!(widen([49.0, 5.0, 51.0, 45.0], 20.0, sheet), [40.0, 5.0, 60.0, 45.0]);
        assert_eq!(widen([0.0, 5.0, 2.0, 45.0], 20.0, sheet), [0.0, 5.0, 20.0, 45.0]);
        assert_eq!(widen([10.0, 10.0, 40.0, 40.0], 20.0, sheet), [10.0, 10.0, 40.0, 40.0]);
        // Never wider than the sheet.
        assert_eq!(widen([10.0, 20.0, 12.0, 22.0], 500.0, sheet), [0.0, 0.0, 100.0, 50.0]);
        assert!((overlap([0.0, 0.0, 10.0, 10.0], [0.0, 0.0, 10.0, 10.0]) - 1.0).abs() < 1e-12);
        assert!((overlap([0.0, 0.0, 10.0, 10.0], [5.0, 0.0, 15.0, 10.0]) - 1.0 / 3.0).abs() < 1e-12);
        assert_eq!(overlap([0.0, 0.0, 1.0, 1.0], [2.0, 2.0, 3.0, 3.0]), 0.0);
        // A whole view in at most 2 × 2 tiles.
        let (_, t) = tiles_max([0.0, 0.0, 10_000.0, 8_000.0], 1.0, 2);
        assert_eq!(t.len(), 4);
    }

    #[test]
    fn a_room_is_found_on_a_sheet_by_its_tag() {
        let texts = Texts(vec![
            ("410".into(), [10.0, 10.0], 1.0),
            ("STUDIO".into(), [10.0, 12.0], 1.0),
            ("410".into(), [90.0, 90.0], 1.0),
            ("CORRIDOR".into(), [50.0, 50.0], 1.0),
            ("CORRIDOR".into(), [60.0, 50.0], 1.0),
            ("LIBRARY 101".into(), [30.0, 70.0], 1.0),
        ]);
        let all = [0.0, 0.0, 100.0, 100.0];
        let w = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // The number beside the name, not the other 410.
        assert_eq!(room_tag(&texts, all, &w(&["410", "Studio"]), 100.0).map(|x| x.0), Some([10.0, 10.0]));
        // One tag holding both.
        assert_eq!(room_tag(&texts, all, &w(&["101", "Library"]), 100.0).map(|x| x.0), Some([30.0, 70.0]));
        // A name the sheet gives twice, with no number, is not a place.
        assert_eq!(room_tag(&texts, all, &w(&["Corridor"]), 100.0), None);
        // Only inside the view.
        assert_eq!(room_tag(&texts, [80.0, 80.0, 100.0, 100.0], &w(&["410", "Studio"]), 100.0).map(|x| x.0), Some([90.0, 90.0]));
        assert_eq!(room_tag(&texts, all, &w(&["999"]), 100.0), None);
        assert_eq!(room_tag(&texts, all, &[], 100.0), None);
    }

    #[test]
    fn a_submittal_page_is_offered_for_the_kind_it_is_mostly_for() {
        let mut a = Applied { elements: 10, ..Applied::default() };
        a.kinds.insert("luminaire".into(), 9);
        a.kinds.insert("fire_alarm_device".into(), 1);
        assert!(a.is_for("luminaire"));
        assert!(!a.is_for("fire_alarm_device"));
        assert!(!a.is_for("receptacle"));
    }

    #[test]
    fn a_kept_path_is_named_by_its_last_part_and_read_where_this_machine_has_it() {
        assert_eq!(last_part("C:\\Jobs\\Sample\\Submittals"), "Submittals");
        assert_eq!(last_part("/jobs/sample/Lighting.pdf"), "Lighting.pdf");
        assert_eq!(last_part("C:\\Jobs\\Sample\\"), "Sample");
        assert_eq!(last_part("Lighting.pdf"), "Lighting.pdf");
        // Not here, and no drive mount: as given.
        let p = "Q:\\No Such Folder\\Submittals";
        assert_eq!(local_path(p), std::path::PathBuf::from(p));
        assert_eq!(local_path("relative/none"), std::path::PathBuf::from("relative/none"));
        // A path that is here stays.
        let here = std::env::temp_dir();
        assert_eq!(local_path(&here.to_string_lossy()), here);
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

    #[test]
    fn a_stretch_is_the_room_in_its_cell_not_the_cell() {
        // An L: 40 × 10 with a 10 × 20 leg up from its left end.
        let l = [[0.0, 0.0], [40.0, 0.0], [40.0, 10.0], [10.0, 10.0], [10.0, 30.0], [0.0, 30.0]];
        // The cell over the leg's corner holds the room's leg and floor, not the cell's empty part.
        let piece = clip_to_rect(&l, [0.0, 0.0, 20.0, 20.0]);
        assert!((polygon_area(&piece) - (20.0 * 10.0 + 10.0 * 10.0)).abs() < 1e-6, "{piece:?}");
        assert!(!point_in(&piece, [15.0, 15.0]), "the cell's corner off the room is no room");
        assert!(point_in(&piece, [5.0, 15.0]) && point_in(&piece, [15.0, 5.0]));
        // A cell off the room clips to nothing.
        assert!(polygon_area(&clip_to_rect(&l, [20.0, 20.0, 40.0, 30.0])) < 1e-9);
        // A cell wholly inside is the cell.
        assert!((polygon_area(&clip_to_rect(&l, [20.0, 0.0, 30.0, 10.0])) - 100.0).abs() < 1e-9);
    }

    #[test]
    fn the_print_layer_and_style_names_are_the_pdf_readers() {
        assert_eq!(SHX_SOURCE_LAYER, cadaddy_io::pdf_read::SHX_SOURCE_LAYER);
        assert_eq!(OCR_TEXT_STYLE, cadaddy_io::pdf_read::OCR_TEXT_STYLE);
    }

    /// A `w` × `h` white picture with black pixels at `dots`.
    fn picture(w: u32, h: u32, dots: &[(u32, u32)]) -> Ink {
        let mut rgba = vec![255u8; (w * h * 4) as usize];
        for (x, y) in dots {
            let i = ((y * w + x) * 4) as usize;
            rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
        }
        Ink::of(w, h, &rgba)
    }

    #[test]
    fn similarity_forgives_a_pixel_or_two_and_nothing_more() {
        let line: Vec<(u32, u32)> = (10..90).map(|x| (x, 50)).collect();
        let a = picture(100, 100, &line);
        assert_eq!(a.count, 80);
        assert!((similarity(&a, &a) - 1.0).abs() < 1e-12);
        // Two pixels down: the same line.
        let b = picture(100, 100, &(10..90).map(|x| (x, 52)).collect::<Vec<_>>());
        assert!((similarity(&a, &b) - 1.0).abs() < 1e-12);
        // Five down: another one.
        let c = picture(100, 100, &(10..90).map(|x| (x, 55)).collect::<Vec<_>>());
        assert_eq!(similarity(&a, &c), 0.0);
        // Half of it: found entirely in the whole, holding half of the whole (and the two
        // pixels past its end within reach).
        let d = picture(100, 100, &(10..50).map(|x| (x, 50)).collect::<Vec<_>>());
        let r = 42.0 / 80.0;
        assert!((similarity(&a, &d) - 2.0 * r / (1.0 + r)).abs() < 1e-9);
        // Blank against blank is alike; blank against ink is not.
        let blank = picture(100, 100, &[]);
        assert_eq!(similarity(&blank, &blank), 1.0);
        assert_eq!(similarity(&blank, &a), 0.0);
        // A grey (light) pixel isn't ink.
        let mut rgba = vec![255u8; 4];
        rgba[..3].copy_from_slice(&[230, 230, 230]);
        assert_eq!(Ink::of(1, 1, &rgba).count, 0);
        // Hostile sizes never index past the bytes.
        assert_eq!(Ink::of(1000, 1000, &[0; 8]).count, 2);
    }

    #[test]
    fn a_picture_takes_the_view_it_overlaps_most_on_its_own_sheet() {
        let views = vec![
            ("P-1".to_string(), "PLAN - SANITARY".to_string(), [0.0, 0.0, 10.0, 10.0]),
            ("P-1".to_string(), "PLAN - WATER".to_string(), [10.0, 0.0, 20.0, 10.0]),
            ("P-2".to_string(), "OTHER SHEET".to_string(), [0.0, 0.0, 30.0, 30.0]),
        ];
        assert_eq!(view_at(&views, "P-1", [11.0, 1.0, 19.0, 9.0]).as_deref(), Some("PLAN - WATER"));
        assert_eq!(view_at(&views, "p-1 ", [2.0, 1.0, 13.0, 9.0]).as_deref(), Some("PLAN - SANITARY"));
        assert_eq!(view_at(&views, "P-1", [40.0, 40.0, 50.0, 50.0]), None);
        assert_eq!(view_at(&views, "P-3", [0.0, 0.0, 5.0, 5.0]), None);
    }

    /// A made-up 12' × 29.5' guest room as a ragged trace: jogs round a column on the north side,
    /// a chase on the west, and a bath partition (a wall fin) running in from the east wall,
    /// the outline wrapping both its faces.
    fn ragged_room() -> Vec<[f64; 2]> {
        vec![
            [0.0, 0.0],
            [12.0, 0.0],
            [12.0, 8.5],
            [5.0, 8.5],
            [5.0, 8.75],
            [12.0, 8.75],
            [12.0, 29.5],
            [8.0, 29.5],
            [8.0, 28.25],
            [4.0, 28.25],
            [4.0, 29.25],
            [0.0, 29.25],
            [0.0, 20.0],
            [0.25, 20.0],
            [0.25, 14.0],
            [0.0, 14.0],
        ]
    }

    fn wall(id: &str, a: [f64; 2], b: [f64; 2], thickness_in: f64) -> v::ModelWall {
        v::ModelWall { id: id.into(), level: "L1".into(), a, b, thickness_in, stud_in: None, wall_type: None }
    }

    #[test]
    fn an_areas_sides_are_its_outermost_edges_each_way_not_a_wall_fin_inside_it() {
        let sides = area_sides(&ragged_room());
        let ways: Vec<&str> = sides.iter().map(|s| compass(s.dir[0], s.dir[1])).collect();
        assert_eq!(ways, ["east", "west", "south", "north"], "{sides:?}");
        let len = |w: &str| sides.iter().find(|s| compass(s.dir[0], s.dir[1]) == w).map(|s| s.len).unwrap();
        // The east wall less the fin's root; the north side all three of its jogged pieces.
        assert!((len("east") - 29.25).abs() < 1e-6, "{sides:?}");
        assert!((len("north") - 12.0).abs() < 1e-6, "{sides:?}");
        assert!((len("south") - 12.0).abs() < 1e-6, "{sides:?}");
        // The fin's faces (7 ft, facing north and south mid-room) are no side.
        assert!(sides.iter().all(|s| s.edges.iter().all(|(a, b)| !((a[1] - 8.5).abs() < 1e-9 && (b[1] - 8.5).abs() < 1e-9))), "{sides:?}");
        assert!(sides.iter().all(|s| s.edges.iter().all(|(a, b)| !((a[1] - 8.75).abs() < 1e-9 && (b[1] - 8.75).abs() < 1e-9))), "{sides:?}");
        // The camera before each side's longest edge first.
        let north = sides.iter().find(|s| compass(s.dir[0], s.dir[1]) == "north").unwrap();
        assert!(north.edges.len() == 3, "{north:?}");
        // Hostile outlines give no side and never panic.
        assert!(area_sides(&[]).is_empty());
        assert!(area_sides(&[[0.0, 0.0], [f64::NAN, 1.0], [1.0, 1.0]]).iter().all(|s| s.len.is_finite()));
    }

    #[test]
    fn a_camera_names_the_wall_it_faces() {
        let mut model = Model::default();
        // The north wall (6"), and a 4" partition 3 ft in front of a camera looking east.
        model.walls.push(wall("N", [0.0, 29.75], [12.0, 29.75], 6.0));
        model.walls.push(wall("P", [9.0, 10.0], [9.0, 20.0], 4.0));
        model.walls.push(wall("E", [12.25, 0.0], [12.25, 29.5], 6.0));
        model.walls.push(v::ModelWall { level: "L2".into(), ..wall("UP", [0.0, 25.0], [12.0, 25.0], 6.0) });
        let (w, at) = faced_wall(&model, "L1", [6.0, 23.5], [0.0, 1.0], 10.0).unwrap();
        assert_eq!(w.id, "N", "another level's wall is no wall here");
        assert!((at - 6.0).abs() < 1e-6, "{at}");
        let (w, at) = faced_wall(&model, "L1", [6.0, 15.0], [1.0, 0.0], 10.0).unwrap();
        assert_eq!(w.id, "P");
        assert!((at - (3.0 - 2.0 / 12.0)).abs() < 1e-6, "{at}");
        assert!(faced_wall(&model, "L1", [6.0, 5.0], [-1.0, 0.0], 10.0).is_none(), "nothing west of it");
        assert!(faced_wall(&model, "L1", [6.0, 23.5], [0.0, 1.0], 2.0).is_none(), "out of reach");
        // The south wall is a window: the piece over it is the wall the camera faces.
        let head = wall("HEAD", [0.0, -0.25], [12.0, -0.25], 6.0);
        model.wall_spans.push(v::WallSpan { wall: head, bottom_in: 84.0, top_in: 108.0 });
        model.openings.push(v::ModelOpening {
            id: "W1".into(),
            level: "L1".into(),
            kind: "window".into(),
            min: [2.0, -0.5],
            max: [10.0, 0.0],
            ..Default::default()
        });
        let (w, _) = faced_wall(&model, "L1", [6.0, 5.0], [0.0, -1.0], 10.0).unwrap();
        assert_eq!(w.id, "HEAD");
        assert_eq!(faced_opening(&model, "L1", [6.0, 5.0], [0.0, -1.0], 10.0), Some("window"));
        assert_eq!(faced_opening(&model, "L1", [11.0, 5.0], [0.0, -1.0], 10.0), None, "beside the window");
        assert_eq!(faced_opening(&model, "L1", [6.0, 5.0], [0.0, 1.0], 10.0), None, "behind the camera");
    }
}
