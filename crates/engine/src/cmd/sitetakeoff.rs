//! Sitework quantities read off the civil sheets (Q339): the grading plan's contours and spot
//! grades become an existing and a proposed surface (TINs), and cut AND fill come from the
//! difference between them over the graded area (where both surfaces are known). The site plan's
//! paving, curbs and fences, the utility plan's structures (with their RIM and INV notes) and the
//! utility runs (system, size, material, depth) become the rest of the takeoff.
//!
//! Every quantity says where it came from: the sheets, a status (`read`: measured off the
//! drawing; `rule`: derived from read values by a stated rule; `assumed`) and the drawing objects
//! that make it up, each with its share, so a total always explains back to the objects. A
//! surface that isn't on the sheets gives "insufficient data", never a number.
//!
//! Recognition is by layer and geometry first, and the label second:
//! - contours: lines and polylines on a grading or topography layer with an elevation written
//!   on them; existing when the layer says so (`EX`, `EXST`, `EG`…) or the line is dashed;
//! - spot grades: `EX 99.50`, `FG 101.25`, `TC …` texts;
//! - paving: a closed outline that is hatched or whose label names the material (`ASPHALT`,
//!   `CONC`, `WALK`); curbs: a thin outline labelled `CURB`; fences: a line with X marks along it;
//!   gates: a circle on a fence line;
//! - structures: a circle with a cross (manhole), a square with an X (catch basin), a diamond
//!   (valve box); a plain circle, square or triangle only by its tag (cleanout, junction box,
//!   meter, backflow preventer), otherwise `unknown`;
//! - utility runs: the long lines on a utility layer, with the system, `12" RCP` and `6' DEEP`
//!   written beside them.
//!
//! - `site.takeoff {path? | paths?, units_per_foot?, offset?, limit?}`: the quantity list (paged),
//!   what was read and what stayed unknown.

use std::collections::BTreeMap;

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use serde::Serialize;
use serde_json::{Value, json};

use super::*;
use crate::{Result, Session};

const CMD: &str = "site.takeoff";
/// Most sheets read at once and objects looked at per sheet (hostile files).
const MAX_SHEETS: usize = 200;
const MAX_OBJECTS: usize = 200_000;
/// Most points in one surface (contour vertices after densifying, plus spot grades).
const MAX_TIN_POINTS: usize = 2_000;
/// Grid cells along the graded area's longer side for the volume.
const GRID_CELLS: usize = 240;
/// Most quantity lines on one page.
const MAX_PAGE: usize = 500;
/// Most elements listed per quantity line.
const MAX_ELEMENTS: usize = 2_000;
/// Trench rule: width = the pipe plus this much each side, feet.
pub const TRENCH_SIDE_FT: f64 = 1.0;

// ---------------- what is read ----------------

/// Which ground a contour or spot grade describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    Existing,
    Proposed,
}

/// A contour line: its elevation and vertices (feet).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Contour {
    pub id: String,
    pub sheet: String,
    pub surface: Surface,
    pub elevation_ft: f64,
    pub points_ft: Vec<[f64; 2]>,
}

/// A spot grade.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Spot {
    pub id: String,
    pub sheet: String,
    pub surface: Surface,
    pub elevation_ft: f64,
    pub at_ft: [f64; 2],
}

/// A site feature: `paving` (material `asphalt`, `concrete` or `unknown`), `building_pad`,
/// `curb`, `fence`, `gate`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Feature {
    pub id: String,
    pub sheets: Vec<String>,
    pub kind: String,
    pub material: String,
    pub tag: String,
    pub at_ft: [f64; 2],
    pub length_ft: f64,
    pub area_sf: f64,
}

/// A utility structure: `manhole`, `catch_basin`, `cleanout`, `valve_box`, `junction_box`,
/// `meter`, `backflow_preventer` or `unknown`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Structure {
    pub id: String,
    pub sheets: Vec<String>,
    pub kind: String,
    /// `storm`, `sanitary`, `water`, `electric`, … or `unknown`.
    pub system: String,
    pub tag: String,
    pub at_ft: [f64; 2],
    pub rim_ft: Option<f64>,
    pub invert_ft: Option<f64>,
}

/// A utility run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Pipe {
    pub id: String,
    pub sheets: Vec<String>,
    /// `storm`, `sanitary`, `water`, `gas`, `electric`, `telecom` or `unknown`.
    pub system: String,
    pub tag: String,
    pub size_in: Option<f64>,
    pub material: String,
    pub length_ft: f64,
    pub depth_ft: Option<f64>,
}

/// Something on a civil layer that wasn't recognised.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Unknown {
    pub id: String,
    pub sheet: String,
    pub what: String,
    pub tag: String,
    pub at_ft: [f64; 2],
}

/// Everything read off one or more civil sheets.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CivilRead {
    pub contours: Vec<Contour>,
    pub spots: Vec<Spot>,
    pub features: Vec<Feature>,
    pub structures: Vec<Structure>,
    pub pipes: Vec<Pipe>,
    pub unknown: Vec<Unknown>,
}

/// One object's share of a quantity.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Part {
    pub id: String,
    pub quantity: f64,
}

/// A takeoff line.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Qty {
    pub csi: String,
    pub description: String,
    /// `None` only with status `insufficient data`.
    pub quantity: Option<f64>,
    pub unit: String,
    /// `read`, `rule`, `assumed` or `insufficient data`.
    pub status: String,
    pub sources: Vec<String>,
    pub elements: Vec<Part>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub note: String,
}

// ---------------- reading a sheet ----------------

#[derive(Clone, Debug)]
enum Shape {
    Path(Vec<Vec2>),
    Closed(Vec<Vec2>),
    Circle(Vec2, f64),
}

#[derive(Clone, Debug)]
struct Prim {
    id: String,
    layer: Vec<String>,
    layer_name: String,
    dashed: bool,
    shape: Shape,
}

#[derive(Clone, Debug)]
struct Txt {
    text: String,
    at: Vec2,
    h: f64,
    layer_name: String,
}

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect()
}

fn any_word(ws: &[String], list: &[&str]) -> bool {
    ws.iter().any(|w| list.contains(&w.as_str()))
}

const CONTOUR_LAYER: &[&str] = &["TOPO", "GRAD", "GRADE", "GRADING", "CONT", "CNTR", "CONTOUR", "CONTOURS", "EG", "FG"];
const EXISTING_WORDS: &[&str] = &["EX", "EXST", "EXIST", "EXISTING", "EG", "ORIG", "OG"];
const UTILITY_LAYER: &[&str] =
    &["UNDR", "UTIL", "UTILITY", "UG", "STRM", "STORM", "SSWR", "SANI", "SAN", "SEWR", "SEWER", "WATR", "WATER", "WTR", "SD", "SS", "PIPE"];
const STRUCTURE_LAYER: &[&str] = &["STRC", "STRUC", "STRUCT", "STRUCTURE", "STRUCTURES", "MH", "MANH", "INLET", "INLETS", "CB"];
const FENCE_LAYER: &[&str] = &["FNCE", "FENC", "FENCE", "FENCING"];
/// The sheet's frame and title block: never site work.
const FRAME_LAYER: &[&str] = &["BORDER", "BRDR", "TTLB", "TITLE", "TBLK", "TBLOCK", "FRAME", "VPORT", "VIEWPORT", "DEFPOINTS"];

/// Drawing units per foot (INSUNITS; unitless follows MEASUREMENT, else inches).
pub fn units_per_foot(d: &Drawing) -> f64 {
    match d.header.i64("INSUNITS", 0) {
        2 => 1.0,
        4 => 304.8,
        5 => 30.48,
        6 => 0.3048,
        1 => 12.0,
        _ if d.header.i64("MEASUREMENT", 0) == 1 => 304.8,
        _ => 12.0,
    }
}

fn dashed_name(n: &str) -> bool {
    let u = n.to_ascii_uppercase();
    u.contains("DASH") || u.contains("HIDDEN")
}

fn dist(a: Vec2, b: Vec2) -> f64 {
    (b - a).len()
}

/// Distance from `p` to the segment `a`–`b`.
fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.x * d.x + d.y * d.y;
    if l2 <= 1e-18 {
        return dist(p, a);
    }
    let t = (((p - a).x * d.x + (p - a).y * d.y) / l2).clamp(0.0, 1.0);
    dist(p, a + d * t)
}

fn path_dist(p: Vec2, pts: &[Vec2], closed: bool) -> f64 {
    let mut best = f64::INFINITY;
    for w in pts.windows(2) {
        if let [a, b] = w {
            best = best.min(seg_dist(p, *a, *b));
        }
    }
    if closed && let (Some(a), Some(b)) = (pts.last(), pts.first()) {
        best = best.min(seg_dist(p, *a, *b));
    }
    if pts.len() == 1
        && let Some(a) = pts.first()
    {
        best = dist(p, *a);
    }
    best
}

fn path_len(pts: &[Vec2]) -> f64 {
    pts.windows(2).map(|w| if let [a, b] = w { dist(*a, *b) } else { 0.0 }).sum()
}

fn shoelace(pts: &[Vec2]) -> f64 {
    let n = pts.len();
    let mut s = 0.0;
    for (i, a) in pts.iter().enumerate() {
        if let Some(b) = pts.get((i + 1) % n.max(1)) {
            s += a.x * b.y - b.x * a.y;
        }
    }
    s.abs() / 2.0
}

fn inside(p: Vec2, poly: &[Vec2]) -> bool {
    let mut c = false;
    let n = poly.len();
    for i in 0..n {
        let (Some(a), Some(b)) = (poly.get(i), poly.get((i + n - 1) % n)) else { continue };
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            c = !c;
        }
    }
    c
}

fn centroid(pts: &[Vec2]) -> Vec2 {
    let n = pts.len().max(1) as f64;
    let s = pts.iter().fold(Vec2::new(0.0, 0.0), |acc, p| acc + *p);
    s * (1.0 / n)
}

/// A plain number (an elevation label): `100`, `100.5`, `+101.25`.
fn plain_number(s: &str) -> Option<f64> {
    let t = s.trim().trim_start_matches('+').trim_end_matches('\'');
    if t.is_empty() || t.len() > 12 || !t.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-') {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// The first number in `s` after `start` (`RIM 100.5` → 100.5).
fn number_after(s: &str, start: usize) -> Option<f64> {
    let rest = s.get(start..)?;
    let b = rest.find(|c: char| c.is_ascii_digit() || c == '-' || c == '.')?;
    let tail = rest.get(b..)?;
    let e = tail.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(tail.len());
    tail.get(..e)?.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A spot grade text: `EX 99.50`, `EG=99.5`, `FG 101.25`, `TC 101.0`, `99.50 EX`.
fn spot_grade(s: &str) -> Option<(Surface, f64)> {
    let kind = |w: &str| match w.to_ascii_uppercase().as_str() {
        "EX" | "EG" | "EXG" | "EXIST" | "EXISTING" => Some(Surface::Existing),
        "FG" | "FS" | "TC" | "TP" | "PROP" | "PG" => Some(Surface::Proposed),
        _ => None,
    };
    // The word and an elevation to the hundredth or tenth (`FG 101.25`, `99.50 EX`), apart: a
    // tag such as `TC-1` is not a spot grade.
    let elevation = |w: &str| w.contains('.').then(|| w.parse::<f64>().ok()).flatten().filter(|v| v.is_finite());
    match s.split_whitespace().collect::<Vec<_>>().as_slice() {
        [a, b] => kind(a).zip(elevation(b)).or_else(|| kind(b).zip(elevation(a))),
        _ => None,
    }
}

/// A pipe size note: `12" RCP`, `8 IN PVC`, `6"`.
fn pipe_size(s: &str) -> Option<f64> {
    let u = s.to_ascii_uppercase();
    for (i, c) in u.char_indices() {
        if c != '"' && !u.get(i..).is_some_and(|r| r.starts_with(" IN") || r.starts_with("IN ") || r.starts_with("-IN")) {
            continue;
        }
        let head = u.get(..i)?.trim_end();
        let start = head.rfind(|c: char| !(c.is_ascii_digit() || c == '.')).map_or(0, |p| p + 1);
        if let Some(v) = head.get(start..).and_then(|n| n.parse::<f64>().ok()).filter(|v| *v > 0.0 && *v < 200.0) {
            return Some(v);
        }
    }
    None
}

const MATERIALS: &[&str] = &["PVC", "RCP", "DIP", "DI", "HDPE", "CPVC", "CU", "COPPER", "PE", "CMP", "VCP", "ABS", "STEEL", "CI", "SDR35", "C900"];

fn pipe_material(s: &str) -> Option<String> {
    words(s).into_iter().find(|w| MATERIALS.contains(&w.as_str()))
}

/// `6' DEEP`, `DEPTH 6'`.
fn depth_of(s: &str) -> Option<f64> {
    let u = s.to_ascii_uppercase();
    if !u.contains("DEEP") && !u.contains("DEPTH") && !u.contains("COVER") {
        return None;
    }
    number_after(&u, 0).filter(|v| *v > 0.0 && *v < 100.0)
}

fn system_of(ws: &[String]) -> Option<&'static str> {
    for w in ws {
        let s = match w.as_str() {
            "STORM" | "SD" | "STM" | "STMH" | "SDMH" | "DRAIN" | "DRAINAGE" => "storm",
            "SEWER" | "SANITARY" | "SAN" | "SS" | "SSWR" | "SSMH" | "SMH" | "SSCO" => "sanitary",
            "WATER" | "WTR" | "DW" | "DOMESTIC" | "WM" | "BFP" | "RPZ" | "VB" | "WV" => "water",
            "GAS" | "GV" => "gas",
            "ELECTRIC" | "ELEC" | "UGE" | "JB" | "PB" => "electric",
            "TELECOM" | "COMM" | "TEL" | "FO" | "HH" => "telecom",
            _ => continue,
        };
        return Some(s);
    }
    None
}

/// The letters a tag starts with (`STMH-1` → `STMH`, `CB 3` → `CB`).
fn tag_prefix(tag: &str) -> String {
    tag.trim().chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_uppercase()
}

fn collect(d: &Drawing, sheet: &str) -> (Vec<Prim>, Vec<Txt>) {
    let layer_lt: BTreeMap<String, bool> = d.layers.iter().map(|l| (l.name.to_ascii_uppercase(), dashed_name(&l.linetype))).collect();
    let mut prims = Vec::new();
    let mut texts = Vec::new();
    for e in d.model.iter().take(MAX_OBJECTS) {
        let layer_name = e.common.layer.to_ascii_uppercase();
        if any_word(&words(&layer_name), FRAME_LAYER) {
            continue;
        }
        let lt = e.common.linetype.to_ascii_uppercase();
        let dashed = if lt.is_empty() || lt == "BYLAYER" { layer_lt.get(&layer_name).copied().unwrap_or(false) } else { dashed_name(&lt) };
        let id = format!("{sheet}#{}", e.handle.hex());
        let shape = match &e.kind {
            EntityKind::Line(l) => Shape::Path(vec![l.a.xy(), l.b.xy()]),
            EntityKind::LwPolyline(p) => {
                let pts: Vec<Vec2> = p.vertices.iter().map(|v| v.p).collect();
                if pts.len() < 2 {
                    continue;
                }
                if p.closed { Shape::Closed(pts) } else { Shape::Path(pts) }
            }
            EntityKind::Circle(c) => Shape::Circle(c.center.xy(), c.radius),
            EntityKind::Text(t) => {
                texts.push(Txt { text: t.value.trim().to_ascii_uppercase(), at: t.justify_point(), h: t.height.abs(), layer_name });
                continue;
            }
            EntityKind::MText(t) => {
                let plain = cadaddy_fonts::plain_text(&t.contents).split_whitespace().collect::<Vec<_>>().join(" ");
                texts.push(Txt { text: plain.to_ascii_uppercase(), at: t.insert.xy(), h: t.height.abs(), layer_name });
                continue;
            }
            _ => continue,
        };
        let finite = match &shape {
            Shape::Path(p) | Shape::Closed(p) => p.iter().all(|v| v.x.is_finite() && v.y.is_finite()),
            Shape::Circle(c, r) => c.x.is_finite() && c.y.is_finite() && r.is_finite(),
        };
        if finite {
            prims.push(Prim { id, layer: words(&layer_name), layer_name, dashed, shape });
        }
    }
    (prims, texts)
}

/// Join open paths whose ends meet into longer paths (contours and runs drawn a segment at a
/// time). The id list keeps every object a chain is made of.
fn chain(paths: Vec<(Vec<String>, Vec<Vec2>)>, eps: f64) -> Vec<(Vec<String>, Vec<Vec2>)> {
    let mut out: Vec<(Vec<String>, Vec<Vec2>)> = Vec::new();
    for (ids, pts) in paths {
        let (Some(a), Some(b)) = (pts.first().copied(), pts.last().copied()) else { continue };
        let mut joined = false;
        for (oids, o) in out.iter_mut() {
            let (Some(oa), Some(ob)) = (o.first().copied(), o.last().copied()) else { continue };
            if dist(ob, a) < eps {
                o.extend(pts.iter().skip(1));
            } else if dist(ob, b) < eps {
                o.extend(pts.iter().rev().skip(1));
            } else if dist(oa, b) < eps {
                let mut v = pts.clone();
                v.extend(o.iter().skip(1));
                *o = v;
            } else if dist(oa, a) < eps {
                let mut v: Vec<Vec2> = pts.iter().rev().copied().collect();
                v.extend(o.iter().skip(1));
                *o = v;
            } else {
                continue;
            }
            oids.extend(ids.iter().cloned());
            joined = true;
            break;
        }
        if !joined {
            out.push((ids, pts));
        }
    }
    out
}

/// Read one civil sheet. `units_per_ft`: drawing units per foot (12 for inches).
pub fn read_sheet(d: &Drawing, sheet: &str, units_per_ft: f64) -> CivilRead {
    let k = 1.0 / units_per_ft.max(1e-9);
    let ft = |v: Vec2| [v.x * k, v.y * k];
    let (prims, texts) = collect(d, sheet);
    let mut r = CivilRead::default();
    let ext = prims
        .iter()
        .flat_map(|p| match &p.shape {
            Shape::Path(v) | Shape::Closed(v) => v.clone(),
            Shape::Circle(c, _) => vec![*c],
        })
        .fold([f64::INFINITY, f64::NEG_INFINITY], |acc, v| [acc[0].min(v.x.min(v.y)), acc[1].max(v.x.max(v.y))]);
    let span = if ext[1] > ext[0] { ext[1] - ext[0] } else { 1.0 };
    let eps = span * 1e-6;
    let utility = |p: &Prim| any_word(&p.layer, UTILITY_LAYER);
    let structure = |p: &Prim| any_word(&p.layer, STRUCTURE_LAYER);
    let contour_layer = |p: &Prim| any_word(&p.layer, CONTOUR_LAYER);

    // Contours.
    let paths: Vec<(Vec<String>, Vec<Vec2>, bool)> = prims
        .iter()
        .filter(|p| contour_layer(p) && !utility(p) && !structure(p))
        .filter_map(|p| {
            if let Shape::Path(v) = &p.shape { Some((vec![p.id.clone()], v.clone(), p.dashed || any_word(&p.layer, EXISTING_WORDS))) } else { None }
        })
        .collect();
    for existing in [true, false] {
        let set: Vec<(Vec<String>, Vec<Vec2>)> = paths.iter().filter(|p| p.2 == existing).map(|p| (p.0.clone(), p.1.clone())).collect();
        let chains = chain(set, eps);
        // Each elevation label belongs to the contour it sits on.
        let mut labels: Vec<Vec<f64>> = vec![Vec::new(); chains.len()];
        for t in texts.iter().filter(|t| any_word(&words(&t.layer_name), CONTOUR_LAYER)) {
            let Some(v) = plain_number(&t.text) else { continue };
            let tol = (t.h * 1.5).max(eps);
            let best = chains
                .iter()
                .enumerate()
                .map(|(i, c)| (i, path_dist(t.at, &c.1, false)))
                .filter(|(_, dd)| *dd <= tol)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((i, _)) = best
                && let Some(l) = labels.get_mut(i)
            {
                l.push(v);
            }
        }
        for ((ids, pts), elev) in chains.into_iter().zip(labels) {
            let id = ids.first().cloned().unwrap_or_default();
            // A line on a grading layer without an elevation (hatching, a wall, a pad edge) is
            // not a contour.
            if let Some(e) = elev.first() {
                r.contours.push(Contour {
                    id,
                    sheet: sheet.to_string(),
                    surface: if existing { Surface::Existing } else { Surface::Proposed },
                    elevation_ft: *e,
                    points_ft: pts.iter().map(|v| ft(*v)).collect(),
                });
            }
        }
    }
    for t in &texts {
        if let Some((surface, v)) = spot_grade(&t.text) {
            r.spots.push(Spot {
                id: format!("{sheet}#spot{}", r.spots.len() + 1),
                sheet: sheet.to_string(),
                surface,
                elevation_ft: v,
                at_ft: ft(t.at),
            });
        }
    }

    let segs: Vec<(&Prim, Vec2, Vec2)> = prims
        .iter()
        .filter_map(|p| match &p.shape {
            Shape::Path(v) if v.len() == 2 => Some((p, *v.first()?, *v.last()?)),
            _ => None,
        })
        .collect();

    // Fences: a long line with X marks along it.
    let mut fence_lines: Vec<(Vec2, Vec2)> = Vec::new();
    for (p, a, b) in segs.iter().filter(|(p, _, _)| !utility(p) && !structure(p)) {
        let len = dist(*a, *b);
        if len <= eps {
            continue;
        }
        let dir = (*b - *a) * (1.0 / len);
        let marks = segs
            .iter()
            .filter(|(q, c, e)| {
                let l = dist(*c, *e);
                if q.layer_name != p.layer_name || l <= eps || l * 8.0 > len {
                    return false;
                }
                let m = (*c + *e) * 0.5;
                let cos = (((*e - *c).x * dir.x + (*e - *c).y * dir.y) / l).abs();
                seg_dist(m, *a, *b) < l * 0.25 && (0.5..0.87).contains(&cos)
            })
            .count();
        if marks >= 4 || (marks >= 2 && any_word(&p.layer, FENCE_LAYER)) {
            fence_lines.push((*a, *b));
            let tag = texts
                .iter()
                .filter(|t| t.layer_name == p.layer_name && words(&t.text).iter().any(|w| w.starts_with("FENC")))
                .map(|t| (seg_dist(t.at, *a, *b), t))
                .filter(|(dd, t)| *dd < t.h * 4.0)
                .min_by(|x, y| x.0.total_cmp(&y.0))
                .map(|(_, t)| t.text.clone())
                .unwrap_or_default();
            r.features.push(Feature {
                id: p.id.clone(),
                sheets: vec![sheet.to_string()],
                kind: "fence".into(),
                material: String::new(),
                tag,
                at_ft: ft((*a + *b) * 0.5),
                length_ft: len * k,
                area_sf: 0.0,
            });
        }
    }

    // Hatching: at least two parallel lines inside the outline, end to end across it (an X
    // through a pad or a box is a symbol, not hatching).
    let tol = (span * 1e-4).max(eps);
    let hatched_outline = |p: &Prim, pts: &[Vec2]| {
        let dirs: Vec<Vec2> = segs
            .iter()
            .filter(|(q, a, b)| {
                q.layer_name == p.layer_name && path_dist(*a, pts, true) < tol && path_dist(*b, pts, true) < tol && inside((*a + *b) * 0.5, pts)
            })
            .map(|(_, a, b)| *b - *a)
            .collect();
        dirs.iter().enumerate().any(|(i, u)| dirs.iter().skip(i + 1).any(|v| (u.x * v.y - u.y * v.x).abs() < 1e-3 * u.len() * v.len()))
    };

    // Closed outlines: paving, pads, curbs.
    struct Cand {
        f: Feature,
        pts: Vec<Vec2>,
        area: f64,
        hatched: bool,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for p in prims.iter().filter(|p| !utility(p) && !structure(p)) {
        let Shape::Closed(pts) = &p.shape else { continue };
        let area = shoelace(pts);
        let mut ring = pts.clone();
        if let Some(f) = pts.first() {
            ring.push(*f);
        }
        let perim = path_len(&ring);
        if perim <= eps {
            continue;
        }
        let width = area / (perim / 2.0).max(eps);
        let thin = width * k < 1.0;
        let c = centroid(pts);
        let near: Vec<&Txt> = texts.iter().filter(|t| if thin { path_dist(t.at, pts, true) < t.h * 2.0 } else { inside(t.at, pts) }).collect();
        let keyword = |t: &Txt| {
            let ws = words(&t.text);
            any_word(
                &ws,
                &[
                    "ASPHALT",
                    "ASPH",
                    "AC",
                    "HMA",
                    "BITUMINOUS",
                    "CONCRETE",
                    "CONC",
                    "PCC",
                    "WALK",
                    "SIDEWALK",
                    "WALKWAY",
                    "CURB",
                    "GUTTER",
                    "PAD",
                    "PAVING",
                    "PAVEMENT",
                ],
            )
        };
        let label = near
            .iter()
            .filter(|t| keyword(t))
            .min_by(|a, b| dist(a.at, c).total_cmp(&dist(b.at, c)))
            .or_else(|| near.iter().min_by(|a, b| dist(a.at, c).total_cmp(&dist(b.at, c))))
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let ws = words(&label);
        let hatched = !thin && hatched_outline(p, pts);
        let material = if any_word(&ws, &["ASPHALT", "ASPH", "AC", "HMA", "BITUMINOUS"]) {
            "asphalt"
        } else if any_word(&ws, &["CONCRETE", "CONC", "PCC", "WALK", "SIDEWALK", "WALKWAY"]) {
            "concrete"
        } else {
            "unknown"
        };
        let (kind, length, area_sf) = if thin && any_word(&ws, &["CURB", "GUTTER"]) {
            ("curb", perim / 2.0 * k, 0.0)
        } else if thin {
            continue;
        } else if hatched && any_word(&ws, &["PAD"]) {
            ("building_pad", 0.0, area * k * k)
        } else if hatched || material != "unknown" || any_word(&ws, &["PAVING", "PAVEMENT"]) {
            ("paving", 0.0, area * k * k)
        } else {
            continue;
        };
        cands.push(Cand {
            f: Feature {
                id: p.id.clone(),
                sheets: vec![sheet.to_string()],
                kind: kind.into(),
                material: if kind == "paving" { material.into() } else { String::new() },
                tag: label,
                at_ft: ft(c),
                length_ft: length,
                area_sf,
            },
            pts: pts.clone(),
            area,
            hatched,
        });
    }
    // An area inside hatched paving that only repeats that paving's label (or names no
    // material) is part of it (a drive aisle, a stall), already counted with it. The paving's
    // hatching runs through it, so it looks hatched itself.
    let nested = |i: usize, x: &Cand| {
        x.f.kind == "paving"
            && cands.iter().enumerate().any(|(j, o)| {
                j != i
                    && o.hatched
                    && o.f.kind == "paving"
                    && o.area > x.area
                    && (x.f.material == "unknown" || x.f.tag == o.f.tag)
                    && x.pts.iter().all(|v| inside(*v, &o.pts) || path_dist(*v, &o.pts, true) < tol)
            })
    };
    let keep: Vec<bool> = cands.iter().enumerate().map(|(i, x)| !nested(i, x)).collect();
    r.features.extend(cands.into_iter().zip(keep).filter(|(_, k)| *k).map(|(x, _)| x.f));

    // Gates: circles on a fence line.
    for p in prims.iter().filter(|p| !utility(p) && !structure(p)) {
        let Shape::Circle(c, rad) = &p.shape else { continue };
        if fence_lines.iter().any(|(a, b)| seg_dist(*c, *a, *b) < rad * 0.1 + eps) {
            let tag = texts
                .iter()
                .filter(|t| dist(t.at, *c) < rad * 4.0 + t.h * 4.0)
                .min_by(|a, b| dist(a.at, *c).total_cmp(&dist(b.at, *c)))
                .map(|t| t.text.clone())
                .unwrap_or_default();
            r.features.push(Feature {
                id: p.id.clone(),
                sheets: vec![sheet.to_string()],
                kind: "gate".into(),
                material: String::new(),
                tag,
                at_ft: ft(*c),
                length_ft: 0.0,
                area_sf: 0.0,
            });
        }
    }

    read_structures(&prims, &texts, sheet, k, eps, &mut r);
    read_runs(&prims, &texts, sheet, k, eps, &mut r);
    r
}

fn read_structures(prims: &[Prim], texts: &[Txt], sheet: &str, k: f64, eps: f64, r: &mut CivilRead) {
    let on = |p: &Prim| any_word(&p.layer, STRUCTURE_LAYER) || any_word(&p.layer, UTILITY_LAYER);
    let segs: Vec<(&Prim, Vec2, Vec2)> = prims
        .iter()
        .filter(|p| on(p))
        .filter_map(|p| match &p.shape {
            Shape::Path(v) if v.len() == 2 => Some((p, *v.first()?, *v.last()?)),
            _ => None,
        })
        .collect();
    // Each symbol: its centre, size and shape.
    struct Sym<'a> {
        p: &'a Prim,
        c: Vec2,
        r: f64,
        shape: &'static str,
    }
    let mut syms: Vec<Sym> = Vec::new();
    for p in prims.iter().filter(|p| any_word(&p.layer, STRUCTURE_LAYER) || (any_word(&p.layer, UTILITY_LAYER) && !matches!(p.shape, Shape::Path(_))))
    {
        match &p.shape {
            Shape::Circle(c, rad) => {
                // A cross: two perpendicular lines through the centre, about a diameter long.
                let through: Vec<Vec2> = segs
                    .iter()
                    .filter(|(q, a, b)| {
                        q.layer_name == p.layer_name && dist((*a + *b) * 0.5, *c) < rad * 0.2 && (dist(*a, *b) - 2.0 * rad).abs() < rad * 0.4
                    })
                    .map(|(_, a, b)| *b - *a)
                    .collect();
                let cross = through.iter().any(|u| through.iter().any(|v| (u.x * v.x + u.y * v.y).abs() < 0.2 * u.len() * v.len()));
                syms.push(Sym { p, c: *c, r: *rad, shape: if cross { "circle_cross" } else { "circle" } });
            }
            Shape::Closed(pts) if pts.len() == 3 => {
                syms.push(Sym { p, c: centroid(pts), r: pts.iter().map(|v| dist(*v, centroid(pts))).fold(0.0, f64::max), shape: "triangle" })
            }
            Shape::Closed(pts) if pts.len() == 4 => {
                let c = centroid(pts);
                let rad = pts.iter().map(|v| dist(*v, c)).fold(0.0, f64::max);
                let (Some(a), Some(b)) = (pts.first(), pts.get(1)) else { continue };
                let axis = (a.x - b.x).abs() < rad * 0.05 || (a.y - b.y).abs() < rad * 0.05;
                let sides: Vec<f64> = (0..4).filter_map(|i| Some(dist(*pts.get(i)?, *pts.get((i + 1) % 4)?))).collect();
                let square = sides.iter().copied().fold(0.0, f64::max) <= sides.iter().copied().fold(f64::INFINITY, f64::min) * 1.2;
                if !square {
                    continue;
                }
                if axis {
                    let diag = segs
                        .iter()
                        .filter(|(q, s, e)| {
                            q.layer_name == p.layer_name && dist((*s + *e) * 0.5, c) < rad * 0.2 && (dist(*s, *e) - 2.0 * rad).abs() < rad * 0.2
                        })
                        .count();
                    syms.push(Sym { p, c, r: rad / std::f64::consts::SQRT_2, shape: if diag >= 2 { "square_x" } else { "square" } });
                } else {
                    syms.push(Sym { p, c, r: rad, shape: "diamond" });
                }
            }
            _ => {}
        }
    }
    if syms.is_empty() {
        return;
    }
    // Label distances are measured in the typical symbol's size (a cleanout is drawn smaller).
    let mut sizes: Vec<f64> = syms.iter().map(|s| s.r).collect();
    sizes.sort_by(f64::total_cmp);
    let r_ref = sizes.get(sizes.len() / 2).copied().unwrap_or(1.0).max(eps);
    for s in &syms {
        let rs = s.r.max(r_ref);
        let own: Vec<&Txt> = texts.iter().filter(|t| t.layer_name == s.p.layer_name).collect();
        let pool: Vec<&Txt> = if own.is_empty() { texts.iter().collect() } else { own };
        // The label block reads to the right of the symbol, top down: tag, RIM, INV.
        let block: Vec<&Txt> = pool
            .iter()
            .copied()
            .filter(|t| {
                let d = t.at - s.c;
                d.x >= 0.0 && d.x <= rs * 2.5 && d.y <= rs && d.y >= -rs * 4.0
            })
            .collect();
        let block = if block.is_empty() { pool.iter().copied().filter(|t| dist(t.at, s.c) < rs * 6.0).collect() } else { block };
        let elev = |key: &str| {
            block
                .iter()
                .filter(|t| t.text.starts_with(key))
                .min_by(|a, b| dist(a.at, s.c).total_cmp(&dist(b.at, s.c)))
                .and_then(|t| number_after(&t.text, key.len()))
        };
        let rim = elev("RIM");
        let inv = elev("INV").or_else(|| elev("IE"));
        let tag = block
            .iter()
            .filter(|t| !t.text.starts_with("RIM") && !t.text.starts_with("INV") && !t.text.starts_with("IE ") && plain_number(&t.text).is_none())
            .min_by(|a, b| dist(a.at, s.c).total_cmp(&dist(b.at, s.c)))
            .map(|t| t.text.clone())
            .unwrap_or_default();
        let pre = tag_prefix(&tag);
        let tw = words(&tag);
        let by_tag = |list: &[&str], wordlist: &[&str]| list.contains(&pre.as_str()) || any_word(&tw, wordlist);
        let kind = match s.shape {
            "circle_cross" => "manhole",
            "square_x" => "catch_basin",
            "diamond" => "valve_box",
            "circle" if by_tag(&["CO", "SSCO", "CLNO"], &["CLEANOUT"]) => "cleanout",
            "circle" if by_tag(&["STMH", "SDMH", "SSMH", "SMH", "MH"], &["MANHOLE"]) => "manhole",
            "square" if by_tag(&["JB", "PB", "HH"], &["JUNCTION", "PULL", "HANDHOLE"]) => "junction_box",
            "square" if by_tag(&["CB", "CI", "INL"], &["INLET"]) => "catch_basin",
            "triangle" if by_tag(&["WM"], &["METER"]) => "meter",
            "triangle" if by_tag(&["BFP", "RPZ", "BFA"], &["BACKFLOW"]) => "backflow_preventer",
            _ => "unknown",
        };
        let system = match kind {
            "catch_basin" => "storm",
            "meter" | "backflow_preventer" | "valve_box" => system_of(std::slice::from_ref(&pre)).or_else(|| system_of(&tw)).unwrap_or("water"),
            _ => system_of(std::slice::from_ref(&pre)).or_else(|| system_of(&tw)).unwrap_or("unknown"),
        };
        if kind == "unknown" {
            r.unknown.push(Unknown {
                id: s.p.id.clone(),
                sheet: sheet.to_string(),
                what: format!("structure symbol ({})", s.shape.replace('_', " ")),
                tag,
                at_ft: [s.c.x * k, s.c.y * k],
            });
            continue;
        }
        r.structures.push(Structure {
            id: s.p.id.clone(),
            sheets: vec![sheet.to_string()],
            kind: kind.into(),
            system: system.into(),
            tag,
            at_ft: [s.c.x * k, s.c.y * k],
            rim_ft: rim,
            invert_ft: inv,
        });
    }
}

fn read_runs(prims: &[Prim], texts: &[Txt], sheet: &str, k: f64, eps: f64, r: &mut CivilRead) {
    let paths: Vec<(&Prim, Vec<Vec2>)> = prims
        .iter()
        .filter(|p| any_word(&p.layer, UTILITY_LAYER) && !any_word(&p.layer, STRUCTURE_LAYER))
        .filter_map(|p| if let Shape::Path(v) = &p.shape { Some((p, v.clone())) } else { None })
        .collect();
    if paths.is_empty() {
        return;
    }
    let layer_texts: Vec<&Txt> = texts.iter().filter(|t| any_word(&words(&t.layer_name), UTILITY_LAYER)).collect();
    let mut hs: Vec<f64> = layer_texts.iter().map(|t| t.h).filter(|h| *h > 0.0).collect();
    hs.sort_by(f64::total_cmp);
    let longest = paths.iter().map(|(_, v)| path_len(v)).fold(0.0, f64::max);
    // A run is long: end ticks and arrows are a few text heights.
    let min_len = hs.get(hs.len() / 2).map_or(longest * 0.1, |h| h * 10.0);
    let runs: Vec<(Vec<String>, Vec<Vec2>)> =
        chain(paths.iter().filter(|(_, v)| path_len(v) >= min_len).map(|(p, v)| (vec![p.id.clone()], v.clone())).collect(), eps);
    if runs.is_empty() {
        return;
    }
    // Each note belongs to the run it is nearest.
    let mut notes: Vec<Vec<&Txt>> = vec![Vec::new(); runs.len()];
    for t in &layer_texts {
        let best = runs.iter().enumerate().map(|(i, (_, v))| (i, path_dist(t.at, v, false))).min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, dd)) = best
            && dd < t.h * 6.0 + eps
            && let Some(n) = notes.get_mut(i)
        {
            n.push(t);
        }
    }
    for ((ids, pts), ns) in runs.into_iter().zip(notes) {
        let all: Vec<String> = ns.iter().flat_map(|t| words(&t.text)).collect();
        let system = system_of(&all).unwrap_or("unknown");
        let size = ns.iter().find_map(|t| pipe_size(&t.text));
        let material = ns.iter().find_map(|t| pipe_material(&t.text)).unwrap_or_default();
        let depth = ns.iter().find_map(|t| depth_of(&t.text));
        let tag = ns.iter().find(|t| system_of(&words(&t.text)).is_some()).map(|t| t.text.clone()).unwrap_or_default();
        r.pipes.push(Pipe {
            id: ids.first().cloned().unwrap_or_default(),
            sheets: vec![sheet.to_string()],
            system: system.into(),
            tag,
            size_in: size,
            material,
            length_ft: path_len(&pts) * k,
            depth_ft: depth,
        });
    }
}

/// Put the sheets' reads together: a feature, structure or run drawn on two sheets (the pad on
/// the site and grading plans, the runs on the site and utility plans) is counted once, with both
/// sheets as its sources.
pub fn merge(reads: Vec<CivilRead>) -> CivilRead {
    let mut out = CivilRead::default();
    let close = |a: f64, b: f64| (a - b).abs() <= 0.005 * a.abs().max(b.abs()) + 1e-6;
    for r in reads {
        out.contours.extend(r.contours);
        out.spots.extend(r.spots);
        out.unknown.extend(r.unknown);
        for f in r.features {
            let dup = out.features.iter_mut().find(|o| {
                !o.sheets.iter().any(|s| f.sheets.contains(s))
                    && o.kind == f.kind
                    && o.material == f.material
                    && o.tag == f.tag
                    && close(o.length_ft, f.length_ft)
                    && close(o.area_sf, f.area_sf)
            });
            match dup {
                Some(o) => o.sheets.extend(f.sheets),
                None => out.features.push(f),
            }
        }
        for s in r.structures {
            let dup = out.structures.iter_mut().find(|o| {
                !o.sheets.iter().any(|x| s.sheets.contains(x))
                    && !s.tag.is_empty()
                    && o.kind == s.kind
                    && o.tag == s.tag
                    && o.rim_ft == s.rim_ft
                    && o.invert_ft == s.invert_ft
            });
            match dup {
                Some(o) => o.sheets.extend(s.sheets),
                None => out.structures.push(s),
            }
        }
        for p in r.pipes {
            let dup = out.pipes.iter_mut().find(|o| {
                !o.sheets.iter().any(|x| p.sheets.contains(x))
                    && o.system == p.system
                    && o.size_in == p.size_in
                    && o.material == p.material
                    && o.tag == p.tag
                    && close(o.length_ft, p.length_ft)
            });
            match dup {
                Some(o) => {
                    o.sheets.extend(p.sheets);
                    if o.depth_ft.is_none() {
                        o.depth_ft = p.depth_ft;
                    }
                }
                None => out.pipes.push(p),
            }
        }
    }
    out
}

// ---------------- surfaces and earthwork ----------------

/// A triangulated surface.
struct Tin {
    pts: Vec<[f64; 3]>,
    owner: Vec<usize>,
    tris: Vec<[usize; 3]>,
    min: [f64; 2],
    max: [f64; 2],
    cell: [f64; 2],
    n: usize,
    buckets: Vec<Vec<usize>>,
}

fn circum(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> Option<([f64; 2], f64)> {
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-12 {
        return None;
    }
    let a2 = a[0] * a[0] + a[1] * a[1];
    let b2 = b[0] * b[0] + b[1] * b[1];
    let c2 = c[0] * c[0] + c[1] * c[1];
    let ux = (a2 * (b[1] - c[1]) + b2 * (c[1] - a[1]) + c2 * (a[1] - b[1])) / d;
    let uy = (a2 * (c[0] - b[0]) + b2 * (a[0] - c[0]) + c2 * (b[0] - a[0])) / d;
    let r2 = (a[0] - ux).powi(2) + (a[1] - uy).powi(2);
    Some(([ux, uy], r2))
}

/// Delaunay triangulation (Bowyer–Watson) of `pts`; triangles index `pts`.
fn delaunay(pts: &[[f64; 2]]) -> Vec<[usize; 3]> {
    let n = pts.len();
    if n < 3 {
        return Vec::new();
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in pts {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-9);
    let mid = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let mut all: Vec<[f64; 2]> = pts.to_vec();
    all.push([mid[0] - 50.0 * span, mid[1] - 40.0 * span]);
    all.push([mid[0] + 50.0 * span, mid[1] - 40.0 * span]);
    all.push([mid[0], mid[1] + 50.0 * span]);
    let get = |i: usize, all: &[[f64; 2]]| all.get(i).copied().unwrap_or([0.0, 0.0]);
    let mut tris: Vec<([usize; 3], [f64; 2], f64)> = Vec::new();
    if let Some((c, r2)) = circum(get(n, &all), get(n + 1, &all), get(n + 2, &all)) {
        tris.push(([n, n + 1, n + 2], c, r2));
    }
    for i in 0..n {
        let p = get(i, &all);
        let mut edges: Vec<[usize; 2]> = Vec::new();
        tris.retain(|(t, c, r2)| {
            let bad = (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) < *r2 * (1.0 - 1e-12);
            if bad {
                for e in [[t[0], t[1]], [t[1], t[2]], [t[2], t[0]]] {
                    edges.push(e);
                }
            }
            !bad
        });
        // The hole's boundary: the edges only one removed triangle had.
        let mut boundary: Vec<[usize; 2]> = Vec::new();
        for e in &edges {
            let shared = edges.iter().filter(|o| (o[0] == e[0] && o[1] == e[1]) || (o[0] == e[1] && o[1] == e[0])).count();
            if shared == 1 {
                boundary.push(*e);
            }
        }
        for e in boundary {
            if let Some((c, r2)) = circum(get(e[0], &all), get(e[1], &all), p) {
                tris.push(([e[0], e[1], i], c, r2));
            }
        }
    }
    tris.into_iter().map(|(t, _, _)| t).filter(|t| t.iter().all(|v| *v < n)).collect()
}

impl Tin {
    /// `pts`: x, y, z and the index of the object each point came from.
    fn new(pts: Vec<([f64; 3], usize)>) -> Option<Tin> {
        let xy: Vec<[f64; 2]> = pts.iter().map(|(p, _)| [p[0], p[1]]).collect();
        let tris = delaunay(&xy);
        if tris.is_empty() {
            return None;
        }
        let (mut min, mut max) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for p in &xy {
            min = [min[0].min(p[0]), min[1].min(p[1])];
            max = [max[0].max(p[0]), max[1].max(p[1])];
        }
        let n = ((tris.len() as f64).sqrt() as usize).clamp(1, 200);
        let cell = [((max[0] - min[0]) / n as f64).max(1e-9), ((max[1] - min[1]) / n as f64).max(1e-9)];
        let mut buckets = vec![Vec::new(); n * n];
        for (ti, t) in tris.iter().enumerate() {
            let vs: Vec<[f64; 2]> = t.iter().filter_map(|i| xy.get(*i).copied()).collect();
            let (lo, hi) = vs.iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), p| {
                ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])
            });
            let ix = |v: f64, o: f64, c: f64| (((v - o) / c).floor().max(0.0) as usize).min(n - 1);
            for bx in ix(lo[0], min[0], cell[0])..=ix(hi[0], min[0], cell[0]) {
                for by in ix(lo[1], min[1], cell[1])..=ix(hi[1], min[1], cell[1]) {
                    if let Some(b) = buckets.get_mut(by * n + bx) {
                        b.push(ti);
                    }
                }
            }
        }
        Some(Tin { owner: pts.iter().map(|(_, o)| *o).collect(), pts: pts.into_iter().map(|(p, _)| p).collect(), tris, min, max, cell, n, buckets })
    }

    /// The surface's elevation at `p` and the object of the nearest corner, if `p` is on it.
    fn at(&self, p: [f64; 2]) -> Option<(f64, usize)> {
        let tol = 1e-9 * (self.max[0] - self.min[0] + self.max[1] - self.min[1]);
        if p[0] < self.min[0] - tol || p[1] < self.min[1] - tol || p[0] > self.max[0] + tol || p[1] > self.max[1] + tol {
            return None;
        }
        let ix = |v: f64, o: f64, c: f64| (((v - o) / c).floor().max(0.0) as usize).min(self.n - 1);
        let b = self.buckets.get(ix(p[1], self.min[1], self.cell[1]) * self.n + ix(p[0], self.min[0], self.cell[0]))?;
        for ti in b {
            let t = self.tris.get(*ti)?;
            let (a, bb, c) = (self.pts.get(t[0])?, self.pts.get(t[1])?, self.pts.get(t[2])?);
            let det = (bb[1] - c[1]) * (a[0] - c[0]) + (c[0] - bb[0]) * (a[1] - c[1]);
            if det.abs() < 1e-18 {
                continue;
            }
            let l1 = ((bb[1] - c[1]) * (p[0] - c[0]) + (c[0] - bb[0]) * (p[1] - c[1])) / det;
            let l2 = ((c[1] - a[1]) * (p[0] - c[0]) + (a[0] - c[0]) * (p[1] - c[1])) / det;
            let l3 = 1.0 - l1 - l2;
            let e = -1e-9;
            if l1 >= e && l2 >= e && l3 >= e {
                let z = l1 * a[2] + l2 * bb[2] + l3 * c[2];
                let corner = if l1 >= l2 && l1 >= l3 {
                    t[0]
                } else if l2 >= l3 {
                    t[1]
                } else {
                    t[2]
                };
                return Some((z, self.owner.get(corner).copied().unwrap_or(0)));
            }
        }
        None
    }
}

/// ∫ max(f, 0) over a triangle where `f` is linear with values `f` at its corners.
fn positive_part(p: [[f64; 2]; 3], f: [f64; 3]) -> f64 {
    let mut poly: Vec<([f64; 2], f64)> = Vec::with_capacity(4);
    for i in 0..3 {
        let (a, fa) = (p[i], f[i]);
        let (b, fb) = (p[(i + 1) % 3], f[(i + 1) % 3]);
        if fa >= 0.0 {
            poly.push((a, fa));
        }
        if (fa >= 0.0) != (fb >= 0.0) {
            let t = fa / (fa - fb);
            poly.push(([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t], 0.0));
        }
    }
    let Some(&(o, fo)) = poly.first() else { return 0.0 };
    let mut s = 0.0;
    for w in poly.windows(2).skip(1) {
        if let [(a, fa), (b, fb)] = w {
            let area = ((a[0] - o[0]) * (b[1] - o[1]) - (b[0] - o[0]) * (a[1] - o[1])).abs() / 2.0;
            s += area * (fo + fa + fb) / 3.0;
        }
    }
    s
}

/// Cut and fill between two surfaces.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Earthwork {
    pub cut_cy: f64,
    pub fill_cy: f64,
    /// Where both surfaces are known, square feet.
    pub graded_area_sf: f64,
    /// Each proposed contour's or spot's share (the nearest TIN corner's object).
    pub cut_parts: Vec<Part>,
    pub fill_parts: Vec<Part>,
}

fn surface_points(contours: &[&Contour], spots: &[&Spot]) -> Vec<([f64; 3], usize)> {
    let total: f64 = contours.iter().map(|c| path_len(&c.points_ft.iter().map(|p| Vec2::new(p[0], p[1])).collect::<Vec<_>>())).sum();
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in contours.iter().flat_map(|c| c.points_ft.iter()).chain(spots.iter().map(|s| &s.at_ft)) {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2)).sqrt();
    let budget = MAX_TIN_POINTS.saturating_sub(spots.len()).max(3) as f64;
    let step = (diag / 80.0).max(total / budget * 1.5).max(1e-6);
    let mut out: Vec<([f64; 3], usize)> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut push = |p: [f64; 3], o: usize, out: &mut Vec<([f64; 3], usize)>| {
        let key = ((p[0] / step * 1e3).round() as i64, (p[1] / step * 1e3).round() as i64);
        if out.len() < MAX_TIN_POINTS && seen.insert(key) {
            out.push((p, o));
        }
    };
    for (ci, c) in contours.iter().enumerate() {
        for w in c.points_ft.windows(2) {
            let [a, b] = w else { continue };
            let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
            let n = ((len / step).ceil() as usize).clamp(1, MAX_TIN_POINTS);
            for i in 0..=n {
                let t = i as f64 / n as f64;
                push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, c.elevation_ft], ci, &mut out);
            }
        }
        if c.points_ft.len() == 1
            && let Some(a) = c.points_ft.first()
        {
            push([a[0], a[1], c.elevation_ft], ci, &mut out);
        }
    }
    for (si, s) in spots.iter().enumerate() {
        push([s.at_ft[0], s.at_ft[1], s.elevation_ft], contours.len() + si, &mut out);
    }
    out
}

/// Cut and fill between the existing and the proposed surface, each made from its contours and
/// spot grades, over the area where both are known. `Err` names what is missing.
pub fn earthwork(contours: &[Contour], spots: &[Spot]) -> std::result::Result<Earthwork, String> {
    let pick = |s: Surface| -> (Vec<&Contour>, Vec<&Spot>) {
        (
            contours.iter().filter(|c| c.surface == s && c.elevation_ft.is_finite()).collect(),
            spots.iter().filter(|p| p.surface == s && p.elevation_ft.is_finite()).collect(),
        )
    };
    let (ec, es) = pick(Surface::Existing);
    let (pc, ps) = pick(Surface::Proposed);
    let ex = Tin::new(surface_points(&ec, &es));
    let pr = Tin::new(surface_points(&pc, &ps));
    let (ex, pr) = match (ex, pr) {
        (Some(e), Some(p)) => (e, p),
        (None, None) => return Err("no existing and no proposed surface (contours or spot grades) on the sheets".into()),
        (None, _) => return Err("no existing surface (existing contours or spot grades) on the sheets".into()),
        (_, None) => return Err("no proposed surface (proposed contours or spot grades) on the sheets".into()),
    };
    let ids: Vec<String> = pc.iter().map(|c| c.id.clone()).chain(ps.iter().map(|s| s.id.clone())).collect();
    let lo = [ex.min[0].max(pr.min[0]), ex.min[1].max(pr.min[1])];
    let hi = [ex.max[0].min(pr.max[0]), ex.max[1].min(pr.max[1])];
    if hi[0] <= lo[0] || hi[1] <= lo[1] {
        return Err("the existing and proposed surfaces don't overlap".into());
    }
    let h = (hi[0] - lo[0]).max(hi[1] - lo[1]) / GRID_CELLS as f64;
    let nx = ((hi[0] - lo[0]) / h).ceil().max(1.0) as usize;
    let ny = ((hi[1] - lo[1]) / h).ceil().max(1.0) as usize;
    let (hx, hy) = ((hi[0] - lo[0]) / nx as f64, (hi[1] - lo[1]) / ny as f64);
    let node = |i: usize, j: usize| [lo[0] + i as f64 * hx, lo[1] + j as f64 * hy];
    // Existing minus proposed at each node: positive is cut.
    let mut dz: Vec<Option<(f64, usize)>> = Vec::with_capacity((nx + 1) * (ny + 1));
    for j in 0..=ny {
        for i in 0..=nx {
            let p = node(i, j);
            dz.push(match (ex.at(p), pr.at(p)) {
                (Some((e, _)), Some((q, o))) => Some((e - q, o)),
                _ => None,
            });
        }
    }
    let at = |i: usize, j: usize| dz.get(j * (nx + 1) + i).copied().flatten();
    let (mut cut, mut fill, mut area) = (0.0, 0.0, 0.0);
    let mut cut_by: BTreeMap<usize, f64> = BTreeMap::new();
    let mut fill_by: BTreeMap<usize, f64> = BTreeMap::new();
    for j in 0..ny {
        for i in 0..nx {
            let (Some(a), Some(b), Some(c), Some(d)) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)) else { continue };
            let (pa, pb, pc, pd) = (node(i, j), node(i + 1, j), node(i + 1, j + 1), node(i, j + 1));
            let mut cc = 0.0;
            let mut ff = 0.0;
            for (p, f) in [([pa, pb, pc], [a.0, b.0, c.0]), ([pa, pc, pd], [a.0, c.0, d.0])] {
                cc += positive_part(p, f);
                ff += positive_part(p, [-f[0], -f[1], -f[2]]);
            }
            area += hx * hy;
            cut += cc;
            fill += ff;
            // The cell's share goes to the proposed object nearest its centre.
            let owner = pr.at([(pa[0] + pc[0]) / 2.0, (pa[1] + pc[1]) / 2.0]).map_or(a.1, |(_, o)| o);
            *cut_by.entry(owner).or_default() += cc;
            *fill_by.entry(owner).or_default() += ff;
        }
    }
    if area <= 0.0 {
        return Err("the existing and proposed surfaces don't overlap".into());
    }
    let parts = |m: BTreeMap<usize, f64>| -> Vec<Part> {
        m.into_iter()
            .filter(|(_, v)| *v > 0.0)
            .map(|(o, v)| Part { id: ids.get(o).cloned().unwrap_or_default(), quantity: round2(v / 27.0) })
            .collect()
    };
    let cut_parts = parts(cut_by);
    let fill_parts = parts(fill_by);
    // A sliver of rounding noise is no cut or fill (and never -0).
    let cy = |v: f64| if v > 0.0 { v } else { 0.0 };
    Ok(Earthwork {
        cut_cy: cy(if cut_parts.is_empty() { round2(cut / 27.0) } else { sum(&cut_parts) }),
        fill_cy: cy(if fill_parts.is_empty() { round2(fill / 27.0) } else { sum(&fill_parts) }),
        graded_area_sf: round2(area),
        cut_parts,
        fill_parts,
    })
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn sum(parts: &[Part]) -> f64 {
    // `+ 0.0`: an empty sum is -0.0.
    round2(parts.iter().map(|p| p.quantity).sum::<f64>() + 0.0)
}

/// Trench excavation and backfill for a run, cubic yards: the trench is the pipe plus
/// [`TRENCH_SIDE_FT`] each side wide and `depth_ft` deep; backfill is the trench less the pipe.
pub fn trench_cy(length_ft: f64, size_in: f64, depth_ft: f64) -> (f64, f64) {
    let od_ft = size_in.max(0.0) / 12.0;
    let width = od_ft + 2.0 * TRENCH_SIDE_FT;
    let exc = length_ft * width * depth_ft / 27.0;
    let pipe = length_ft * std::f64::consts::PI * (od_ft / 2.0).powi(2) / 27.0;
    (exc, (exc - pipe).max(0.0))
}

fn system_name(s: &str) -> &'static str {
    match s {
        "storm" => "STORM",
        "sanitary" => "SANITARY SEWER",
        "water" => "WATER",
        _ => "",
    }
}

fn pipe_csi(s: &str) -> &'static str {
    match s {
        "storm" => "33 41 00",
        "sanitary" => "33 31 00",
        _ => "33 11 00",
    }
}

/// A structure's takeoff line: CSI and description.
pub fn structure_line(kind: &str, system: &str) -> Option<(&'static str, String)> {
    Some(match kind {
        "manhole" => match system {
            "storm" => ("33 41 13", "STORM MANHOLE".into()),
            "sanitary" => ("33 31 13", "SANITARY MANHOLE".into()),
            _ => ("33 05 13", "MANHOLE, SYSTEM NOT SHOWN".into()),
        },
        "catch_basin" => ("33 41 13", "CATCH BASIN".into()),
        "cleanout" => ("33 31 13", "CLEANOUT".into()),
        "valve_box" => ("33 11 13", "VALVE BOX".into()),
        "junction_box" => ("33 71 00", "ELECTRIC JUNCTION BOX".into()),
        "meter" => ("33 11 13", "WATER METER".into()),
        "backflow_preventer" => ("33 11 13", "BACKFLOW PREVENTER".into()),
        _ => return None,
    })
}

fn sources<'a>(it: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut v: Vec<String> = it.cloned().collect();
    v.sort();
    v.dedup();
    v
}

fn line(csi: &str, description: impl Into<String>, unit: &str, status: &str, srcs: Vec<String>, elements: Vec<Part>) -> Qty {
    Qty {
        csi: csi.into(),
        description: description.into(),
        quantity: Some(sum(&elements)),
        unit: unit.into(),
        status: status.into(),
        sources: srcs,
        elements,
        note: String::new(),
    }
}

/// The takeoff from what was read. Every line's elements add up to its quantity.
pub fn takeoff(c: &CivilRead) -> Vec<Qty> {
    let mut out = Vec::new();
    let grading_sheets = sources(c.contours.iter().map(|x| &x.sheet).chain(c.spots.iter().map(|x| &x.sheet)));
    match earthwork(&c.contours, &c.spots) {
        Ok(e) => {
            for (desc, parts) in [("EARTHWORK, CUT (EXCAVATION)", e.cut_parts), ("EARTHWORK, FILL (COMPACTED)", e.fill_parts)] {
                let mut q = line("31 22 00", desc, "CY", "read", grading_sheets.clone(), parts);
                q.note = format!("existing vs proposed surface (TIN) over {:.0} SF graded", e.graded_area_sf);
                out.push(q);
            }
        }
        Err(why) => out.push(Qty {
            csi: "31 22 00".into(),
            description: "EARTHWORK, CUT AND FILL".into(),
            quantity: None,
            unit: "CY".into(),
            status: "insufficient data".into(),
            sources: grading_sheets,
            elements: Vec::new(),
            note: why,
        }),
    }
    let feats = |kind: &str, material: Option<&str>| -> Vec<&Feature> {
        c.features.iter().filter(|f| f.kind == kind && material.is_none_or(|m| f.material == m)).collect()
    };
    let asphalt = feats("paving", Some("asphalt"));
    if !asphalt.is_empty() {
        let parts: Vec<Part> = asphalt.iter().map(|f| Part { id: f.id.clone(), quantity: round2(f.area_sf / 9.0) }).collect();
        let srcs = sources(asphalt.iter().flat_map(|f| f.sheets.iter()));
        let mut base = line("32 11 23", "AGGREGATE BASE COURSE (UNDER ASPHALT)", "SY", "rule", srcs.clone(), parts.clone());
        base.note = "same area as the asphalt paving".into();
        out.push(base);
        out.push(line("32 12 16", "ASPHALT PAVING", "SY", "read", srcs, parts));
    }
    let concrete = feats("paving", Some("concrete"));
    if !concrete.is_empty() {
        let parts = concrete.iter().map(|f| Part { id: f.id.clone(), quantity: round2(f.area_sf) }).collect();
        out.push(line("32 13 13", "CONCRETE PAVING AND WALKS", "SF", "read", sources(concrete.iter().flat_map(|f| f.sheets.iter())), parts));
    }
    let unknown_paving = feats("paving", Some("unknown"));
    if !unknown_paving.is_empty() {
        let parts = unknown_paving.iter().map(|f| Part { id: f.id.clone(), quantity: round2(f.area_sf) }).collect();
        let mut q = line("32 10 00", "PAVING, MATERIAL NOT SHOWN", "SF", "read", sources(unknown_paving.iter().flat_map(|f| f.sheets.iter())), parts);
        q.note = "hatched paving without a material label".into();
        out.push(q);
    }
    for (kind, csi, desc, unit) in
        [("curb", "32 16 13", "CONCRETE CURB AND GUTTER", "LF"), ("fence", "32 31 13", "FENCE", "LF"), ("gate", "32 31 13", "FENCE GATE", "EA")]
    {
        let fs = feats(kind, None);
        if fs.is_empty() {
            continue;
        }
        let parts = fs.iter().map(|f| Part { id: f.id.clone(), quantity: if unit == "EA" { 1.0 } else { round2(f.length_ft) } }).collect();
        out.push(line(csi, desc, unit, "read", sources(fs.iter().flat_map(|f| f.sheets.iter())), parts));
    }
    // Utility piping by system, size and material, then its trench.
    let mut groups: BTreeMap<(String, String, String), Vec<&Pipe>> = BTreeMap::new();
    for p in c.pipes.iter().filter(|p| matches!(p.system.as_str(), "storm" | "sanitary" | "water")) {
        let size = p.size_in.map_or(String::new(), |s| format!("{s:06.2}"));
        groups.entry((p.system.clone(), size, p.material.clone())).or_default().push(p);
    }
    for ((system, _, material), ps) in &groups {
        let size = ps.first().and_then(|p| p.size_in);
        let desc = format!(
            "{} PIPE, {}{}",
            system_name(system),
            size.map_or("SIZE NOT SHOWN".to_string(), |s| format!("{} IN.", trim_num(s))),
            if material.is_empty() { String::new() } else { format!(" {material}") }
        );
        let parts = ps.iter().map(|p| Part { id: p.id.clone(), quantity: round2(p.length_ft) }).collect();
        out.push(line(pipe_csi(system), desc, "LF", "read", sources(ps.iter().flat_map(|p| p.sheets.iter())), parts));
    }
    let mut trench: BTreeMap<String, (Vec<Part>, Vec<Part>, Vec<String>, Vec<String>)> = BTreeMap::new();
    for p in c.pipes.iter().filter(|p| matches!(p.system.as_str(), "storm" | "sanitary" | "water")) {
        let e = trench.entry(p.system.clone()).or_default();
        match (p.size_in, p.depth_ft) {
            (Some(s), Some(d)) => {
                let (x, b) = trench_cy(p.length_ft, s, d);
                e.0.push(Part { id: p.id.clone(), quantity: round2(x) });
                e.1.push(Part { id: p.id.clone(), quantity: round2(b) });
                e.2.extend(p.sheets.iter().cloned());
            }
            _ => e.3.push(p.id.clone()),
        }
    }
    for (system, (exc, back, srcs, missing)) in trench {
        let note = format!("trench = pipe + {TRENCH_SIDE_FT} ft each side, to the depth shown; backfill = trench less the pipe");
        if !exc.is_empty() {
            let srcs = sources(srcs.iter());
            let mut q = line("31 23 16", format!("TRENCH EXCAVATION, {} PIPE", system_name(&system)), "CY", "rule", srcs.clone(), exc);
            q.note = note.clone();
            out.push(q);
            let mut q = line("31 23 23", format!("TRENCH BACKFILL, {} PIPE", system_name(&system)), "CY", "rule", srcs, back);
            q.note = note;
            out.push(q);
        }
        if !missing.is_empty() {
            out.push(Qty {
                csi: "31 23 16".into(),
                description: format!("TRENCH, {} PIPE WITHOUT SIZE OR DEPTH", system_name(&system)),
                quantity: None,
                unit: "CY".into(),
                status: "insufficient data".into(),
                sources: Vec::new(),
                elements: missing.into_iter().map(|id| Part { id, quantity: 0.0 }).collect(),
                note: "the run's size or depth isn't written on the sheets".into(),
            });
        }
    }
    let mut by_kind: BTreeMap<(String, String), Vec<&Structure>> = BTreeMap::new();
    for s in &c.structures {
        if let Some((csi, desc)) = structure_line(&s.kind, &s.system) {
            by_kind.entry((csi.to_string(), desc)).or_default().push(s);
        }
    }
    for ((csi, desc), ss) in by_kind {
        let parts = ss.iter().map(|s| Part { id: s.id.clone(), quantity: 1.0 }).collect();
        out.push(line(&csi, desc, "EA", "read", sources(ss.iter().flat_map(|s| s.sheets.iter())), parts));
    }
    out
}

fn trim_num(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

// ---------------- the command ----------------

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Site Takeoff", run_takeoff)
            .params("{path?: string | {path, sheet?}, paths?: [string | {path, sheet?}] (civil sheets, DXF/DWG; none: the open drawing), sheet?, units_per_foot?: drawing units per foot (default from INSUNITS), offset?, limit? (default 100, at most 500), elements?: bool (list each line's objects and shares; default true)} → {sheets, total, offset, quantities: [{csi, description, quantity, unit, status: read|rule|assumed|insufficient data, sources, elements: [{id, quantity}], note}], earthwork: {status, cut_cy, fill_cy, graded_area_sf, why}, read: {contours: {existing, proposed}, spots, paving, curbs, fences, gates, structures, pipes}, unknown: [{id, sheet, what, tag}]}: sitework quantities read off the grading, site and utility plans; earthwork is the cut and fill between the existing and proposed surfaces")
            .enabled(always)
            .noundo(),
    ]
}

struct SheetIn {
    name: String,
    drawing: Drawing,
}

fn sheets(s: &Session, p: &Value) -> Result<Vec<SheetIn>> {
    let mut wanted: Vec<(String, Option<String>)> = Vec::new();
    let entry = |v: &Value| -> Option<(String, Option<String>)> {
        match v {
            Value::String(path) => Some((path.clone(), None)),
            Value::Object(o) => Some((o.get("path")?.as_str()?.to_string(), o.get("sheet").and_then(Value::as_str).map(str::to_string))),
            _ => None,
        }
    };
    if let Some(a) = p.get("paths").filter(|v| !v.is_null()) {
        let a = a.as_array().ok_or_else(|| bad(CMD, "`paths` is a list of sheets"))?;
        for v in a.iter().take(MAX_SHEETS + 1) {
            wanted.push(entry(v).ok_or_else(|| bad(CMD, "each of `paths` is a path or {path, sheet?}"))?);
        }
        if wanted.len() > MAX_SHEETS {
            return Err(bad(CMD, format!("at most {MAX_SHEETS} sheets at once")));
        }
    } else if let Some(v) = p.get("path").filter(|v| !v.is_null()) {
        wanted.push(entry(v).ok_or_else(|| bad(CMD, "`path` is a path or {path, sheet?}"))?);
    }
    if wanted.is_empty() {
        let name = str_param(p, "sheet").map(str::trim).filter(|v| !v.is_empty()).unwrap_or("current drawing").to_string();
        return Ok(vec![SheetIn { name, drawing: s.doc()?.clone() }]);
    }
    let hooks = super::file::io().ok_or_else(|| bad(CMD, "file formats are not available in this build"))?;
    let mut out = Vec::new();
    for (path, sheet) in wanted {
        let bytes = crate::readfile::read(path.trim()).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        let file = std::path::Path::new(path.trim()).file_name().and_then(|f| f.to_str()).unwrap_or("sheet.dxf").to_string();
        let drawing = (hooks.read)(&bytes, &file).map_err(|e| bad(CMD, format!("{path}: {e}")))?;
        let stem = std::path::Path::new(&file).file_stem().and_then(|f| f.to_str()).unwrap_or_default().to_string();
        out.push(SheetIn { name: sheet.filter(|v| !v.trim().is_empty()).unwrap_or(stem), drawing });
    }
    Ok(out)
}

fn run_takeoff(s: &mut Session, p: &Value) -> Result<Value> {
    let upf = match p.get("units_per_foot").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_f64().filter(|u| u.is_finite() && *u > 0.0).ok_or_else(|| bad(CMD, "`units_per_foot` is a positive number"))?),
        None => None,
    };
    let offset = match p.get("offset").filter(|v| !v.is_null()) {
        Some(v) => v.as_u64().ok_or_else(|| bad(CMD, "`offset` is a whole number"))? as usize,
        None => 0,
    };
    let limit = match p.get("limit").filter(|v| !v.is_null()) {
        Some(v) => (v.as_u64().ok_or_else(|| bad(CMD, "`limit` is a whole number"))? as usize).min(MAX_PAGE),
        None => 100,
    };
    let with_elements = bool_or(p, "elements", true);
    let input = sheets(s, p)?;
    let names: Vec<String> = input.iter().map(|x| x.name.clone()).collect();
    let civil = merge(input.iter().map(|x| read_sheet(&x.drawing, &x.name, upf.unwrap_or_else(|| units_per_foot(&x.drawing)))).collect());
    let lines = takeoff(&civil);
    let earth = match earthwork(&civil.contours, &civil.spots) {
        Ok(e) => json!({"status": "read", "cut_cy": e.cut_cy, "fill_cy": e.fill_cy, "graded_area_sf": e.graded_area_sf}),
        Err(why) => json!({"status": "insufficient data", "why": why}),
    };
    let total = lines.len();
    let page: Vec<Value> = lines
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|q| {
            let mut v =
                json!({"csi": q.csi, "description": q.description, "quantity": q.quantity, "unit": q.unit, "status": q.status, "sources": q.sources});
            if with_elements {
                v["elements"] = json!(q.elements.iter().take(MAX_ELEMENTS).map(|e| json!({"id": e.id, "quantity": e.quantity})).collect::<Vec<_>>());
                if q.elements.len() > MAX_ELEMENTS {
                    v["elements_total"] = json!(q.elements.len());
                }
            } else {
                v["element_count"] = json!(q.elements.len());
            }
            if !q.note.is_empty() {
                v["note"] = json!(q.note);
            }
            v
        })
        .collect();
    let count = |kind: &str| civil.features.iter().filter(|f| f.kind == kind).count();
    let mut out = json!({
        "sheets": names,
        "total": total,
        "offset": offset,
        "quantities": page,
        "earthwork": earth,
        "read": {
            "contours": {
                "existing": civil.contours.iter().filter(|c| c.surface == Surface::Existing).count(),
                "proposed": civil.contours.iter().filter(|c| c.surface == Surface::Proposed).count(),
            },
            "spots": civil.spots.len(),
            "paving": count("paving"),
            "curbs": count("curb"),
            "fences": count("fence"),
            "gates": count("gate"),
            "structures": civil.structures.len(),
            "pipes": civil.pipes.len(),
        },
        "unknown": civil.unknown.iter().take(200).map(|u| json!({"id": u.id, "sheet": u.sheet, "what": u.what, "tag": u.tag})).collect::<Vec<_>>(),
    });
    if offset + limit < total {
        out["next_offset"] = json!(offset + limit);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contour(id: &str, s: Surface, z: f64, a: [f64; 2], b: [f64; 2]) -> Contour {
        Contour { id: id.into(), sheet: "C-1".into(), surface: s, elevation_ft: z, points_ft: vec![a, b] }
    }

    /// Existing ground 100 + 0.03x, proposed 101 + 0.01y over a 100 ft square: the cut/fill line
    /// runs on the slant x = (1 + 0.01y)/0.03, and cut = fill = 3888.9 ft³ (144.03 CY).
    #[test]
    fn planes_with_a_slanted_daylight_line() {
        let mut cs = Vec::new();
        for (i, z) in [100.0, 101.0, 102.0, 103.0].iter().enumerate() {
            let x = (z - 100.0) / 0.03;
            cs.push(contour(&format!("e{i}"), Surface::Existing, *z, [x, 0.0], [x, 100.0]));
        }
        cs.push(contour("p0", Surface::Proposed, 101.0, [0.0, 0.0], [100.0, 0.0]));
        cs.push(contour("p1", Surface::Proposed, 102.0, [0.0, 100.0], [100.0, 100.0]));
        let e = earthwork(&cs, &[]).unwrap();
        let want = 3888.889 / 27.0;
        assert!((e.cut_cy - want).abs() / want < 0.02, "{e:?}");
        assert!((e.fill_cy - want).abs() / want < 0.02, "{e:?}");
        assert!((e.graded_area_sf - 10_000.0).abs() < 1.0, "{e:?}");
        assert!((sum(&e.cut_parts) - e.cut_cy).abs() < 1e-6);
    }

    #[test]
    fn number_parsers() {
        assert_eq!(plain_number("100.5"), Some(100.5));
        assert_eq!(plain_number("BLDG PAD"), None);
        assert_eq!(spot_grade("EX 99.50"), Some((Surface::Existing, 99.5)));
        assert_eq!(spot_grade("FG 101.25"), Some((Surface::Proposed, 101.25)));
        assert_eq!(spot_grade("SEE DETAIL 4 ON C-501"), None);
        // A tag is not a spot grade.
        assert_eq!(spot_grade("TC-1"), None);
        assert_eq!(spot_grade("TC 1"), None);
        assert_eq!(spot_grade("99.50 EX"), Some((Surface::Existing, 99.5)));
        assert_eq!(pipe_size("12\" RCP"), Some(12.0));
        assert_eq!(pipe_size("8 IN PVC"), Some(8.0));
        assert_eq!(pipe_material("12\" RCP"), Some("RCP".into()));
        assert_eq!(depth_of("6' DEEP"), Some(6.0));
        assert_eq!(number_after("RIM 100.5", 3), Some(100.5));
        assert_eq!(trim_num(12.0), "12");
    }

    #[test]
    fn positive_part_of_a_triangle() {
        let p = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        assert!((positive_part(p, [1.0, 1.0, 1.0]) - 0.5).abs() < 1e-12);
        assert!(positive_part(p, [-1.0, -1.0, -1.0]).abs() < 1e-12);
        // f = x - 0.5 is positive on the corner triangle x > 0.5: area 1/8, mean 1/6 → 1/48.
        assert!((positive_part(p, [-0.5, 0.5, -0.5]) - 1.0 / 48.0).abs() < 1e-12);
    }
}
