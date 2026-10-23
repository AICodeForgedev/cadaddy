//! Architecture reader (Q307): doors, windows, the stair and elevator, slab edges, the roof and
//! rooms with their finishes, from the architectural sheets.
//!
//! - **Schedules** (tables): a title (`DOOR SCHEDULE`, `HARDWARE GROUPS`, `WINDOW SCHEDULE`,
//!   `ROOM FINISHES`, `FINISH LEGEND`; A-601's `ROOM FINISH SCHEDULE` is the ceilings'), the header row under it and the rows under that;
//!   columns from the header cells' boxes, matched by their words (`MARK` / `DOOR NO.`, `WD` /
//!   `WIDTH`, `RATING` / `LABEL (MIN)`, `HW SET` / `HARDWARE`, `ROOM NO.` / `NO.` / `RM #`, …).
//! - **Doors** (`… FLOOR PLAN`): each swing arc with its leaf line: the hinge (the arc's centre),
//!   the leaf's tip (which side it swings to), the closed position (the latch jamb). The rooms
//!   either side come from Q264's ceiling reader (outside the slab edge = out of the building);
//!   the handing follows the hardware rule (seen from the exterior, else the corridor). The mark
//!   is the nearest door schedule mark on the plan; its row gives size, type, frame, rating and
//!   hardware group, and the group says whether it has a card reader.
//! - **Windows**: the frame boxes in the exterior walls, and the nearest window schedule mark.
//! - **Stair**: the flights (boxes with treads across them), the landing, the `UP 24R @ 7"`
//!   note and the tag; **elevator**: the hoistway box with its X, tag, capacity and `PIT` note;
//!   both on every level they're drawn on.
//! - **Slab edges**: the slab edge outline on each floor plan, the roof edge on the roof plan.
//! - **Roof**: edge, parapet note, slope note, drains (circles marked `RD`), curbs, hatch and
//!   elevator overrun (boxes with their tag and height note beneath).
//! - **Rooms**: the name over the number on the floor plans, and the finish schedule's row and
//!   legend (with device plate colours).
//!
//! Everything is in building feet (sheet alignment from Q255).

use crate::arch::{hand, outside_side};
use crate::ceiling_reader::CeilingResult;
use crate::sheet_reader::{self, Alignment, SheetMap};
use crate::truth::{FinishCode, HardwareGroup, Truth, WindowType};
use cadaddy_doc::{Drawing, EntityKind};
use serde_json::{Value, json};

/// Most entities read from one sheet (hostile input).
const MAX_ENTITIES: usize = 50_000;
/// Most rows read from one table.
const MAX_ROWS: usize = 2_000;
/// Most vertices of a closed polyline kept as a shape.
const MAX_POLY: usize = 64;

// ---- Results -----------------------------------------------------------------------------------

/// A door schedule row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DoorRow {
    pub mark: String,
    pub width_in: Option<f64>,
    pub height_in: Option<f64>,
    pub door_type: String,
    pub material: String,
    pub frame: String,
    pub fire_rating: Option<String>,
    pub hardware: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundDoor {
    pub level: String,
    pub mark: Option<String>,
    /// Middle of the opening on the face it swings from, building feet.
    pub at_ft: [f64; 2],
    pub hinge_ft: [f64; 2],
    pub latch_ft: [f64; 2],
    /// As drawn, inches.
    pub width_in: f64,
    pub swings_into: Option<String>,
    pub from_room: Option<String>,
    pub hand: String,
    /// Its schedule row (by mark).
    pub row: Option<DoorRow>,
    pub card_reader: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundWindow {
    pub level: String,
    pub mark: Option<String>,
    pub at_ft: [f64; 2],
    pub width_in: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundStair {
    pub tag: String,
    pub levels: Vec<String>,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    pub width_in: f64,
    pub risers: u32,
    pub riser_in: Option<f64>,
    pub tread_in: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundElevator {
    pub tag: String,
    pub levels: Vec<String>,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    pub pit_ft: Option<f64>,
    pub capacity_lb: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundSlabEdge {
    pub level: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundCurb {
    pub tag: String,
    pub kind: String,
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    pub height_in: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundRoof {
    pub min_ft: [f64; 2],
    pub max_ft: [f64; 2],
    pub parapet_in: Option<f64>,
    pub slope_in_ft: Option<f64>,
    pub drains: Vec<[f64; 2]>,
    pub curbs: Vec<FoundCurb>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundRoom {
    pub level: String,
    pub number: String,
    pub name: String,
    /// Where its label is, building feet.
    pub at_ft: [f64; 2],
    pub floor: Option<String>,
    pub base: Option<String>,
    pub wall: Option<String>,
    pub ceiling: Option<String>,
    pub ceiling_ft: Option<f64>,
    /// Device plate colour from its wall finish.
    pub plates: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ArchResult {
    pub doors: Vec<FoundDoor>,
    pub door_rows: Vec<DoorRow>,
    pub hardware: Vec<HardwareGroup>,
    pub windows: Vec<FoundWindow>,
    pub window_types: Vec<WindowType>,
    pub stairs: Vec<FoundStair>,
    pub elevators: Vec<FoundElevator>,
    pub slab_edges: Vec<FoundSlabEdge>,
    pub roof: Option<FoundRoof>,
    pub rooms: Vec<FoundRoom>,
    pub finish_codes: Vec<FinishCode>,
}

// ---- Geometry from a sheet ---------------------------------------------------------------------

#[derive(Clone, Debug)]
pub(crate) struct Txt {
    pub(crate) s: String,
    /// Building feet (plans) or model inches (schedules).
    pub(crate) at: [f64; 2],
    pub(crate) h: f64,
    pub(crate) grid: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Shapes {
    pub(crate) texts: Vec<Txt>,
    /// (layer, a, b)
    pub(crate) lines: Vec<(String, [f64; 2], [f64; 2])>,
    /// Axis-aligned boxes: (layer, min, max).
    pub(crate) boxes: Vec<(String, [f64; 2], [f64; 2])>,
    /// (layer, centre, radius, start point, end point)
    pub(crate) arcs: Vec<(String, [f64; 2], f64, [f64; 2], [f64; 2])>,
    pub(crate) circles: Vec<(String, [f64; 2], f64)>,
    /// Closed polylines of any shape: (layer, vertices).
    pub(crate) polys: Vec<(String, Vec<[f64; 2]>)>,
}

fn finite(p: [f64; 2]) -> bool {
    p.iter().all(|v| v.is_finite())
}

/// A sheet's entities, through `map` (model → building feet, or kept as model inches).
pub(crate) fn shapes(d: &Drawing, map: &dyn Fn([f64; 2]) -> [f64; 2], scale: f64) -> Shapes {
    let mut s = Shapes { texts: Vec::new(), lines: Vec::new(), boxes: Vec::new(), arcs: Vec::new(), circles: Vec::new(), polys: Vec::new() };
    for e in d.model.iter().take(MAX_ENTITIES) {
        let layer = e.common.layer.to_ascii_uppercase();
        let grid = layer.contains("GRID");
        match &e.kind {
            EntityKind::Text(t) => {
                let p = t.align_pt.unwrap_or(t.insert);
                let at = map([p.x, p.y]);
                let v = t.value.trim();
                if !v.is_empty() && finite(at) && t.height.is_finite() {
                    s.texts.push(Txt { s: v.to_string(), at, h: t.height * scale, grid });
                }
            }
            EntityKind::MText(t) => {
                let at = map([t.insert.x, t.insert.y]);
                let v = t.contents.trim();
                if !v.is_empty() && finite(at) && t.height.is_finite() {
                    s.texts.push(Txt { s: v.to_string(), at, h: t.height * scale, grid });
                }
            }
            EntityKind::Line(l) => {
                let (a, b) = (map([l.a.x, l.a.y]), map([l.b.x, l.b.y]));
                if finite(a) && finite(b) {
                    s.lines.push((layer, a, b));
                }
            }
            EntityKind::LwPolyline(p) if p.closed && (3..=MAX_POLY).contains(&p.vertices.len()) => {
                let pts: Vec<[f64; 2]> = p.vertices.iter().map(|v| map([v.p.x, v.p.y])).collect();
                if pts.iter().all(|p| finite(*p)) {
                    s.polys.push((layer.clone(), pts.clone()));
                }
                if pts.len() != 4 {
                    continue;
                }
                if pts.iter().all(|p| finite(*p)) {
                    let lo = pts.iter().fold([f64::MAX; 2], |m, p| [m[0].min(p[0]), m[1].min(p[1])]);
                    let hi = pts.iter().fold([f64::MIN; 2], |m, p| [m[0].max(p[0]), m[1].max(p[1])]);
                    // Axis-aligned only: every vertex on the box's edge.
                    let on_edge = pts.iter().all(|p| {
                        let tol = 1e-6 * (1.0 + hi[0].abs() + hi[1].abs());
                        ((p[0] - lo[0]).abs() < tol || (p[0] - hi[0]).abs() < tol) && ((p[1] - lo[1]).abs() < tol || (p[1] - hi[1]).abs() < tol)
                    });
                    if on_edge {
                        s.boxes.push((layer, lo, hi));
                    }
                }
            }
            EntityKind::Arc(a) => {
                let c = [a.center.x, a.center.y];
                let ps = [c[0] + a.radius * a.start.cos(), c[1] + a.radius * a.start.sin()];
                let pe = [c[0] + a.radius * a.end.cos(), c[1] + a.radius * a.end.sin()];
                let (c, ps, pe) = (map(c), map(ps), map(pe));
                if finite(c) && finite(ps) && finite(pe) && a.radius.is_finite() {
                    s.arcs.push((layer, c, a.radius * scale, ps, pe));
                }
            }
            EntityKind::Circle(ci) => {
                let c = map([ci.center.x, ci.center.y]);
                if finite(c) && ci.radius.is_finite() {
                    s.circles.push((layer, c, ci.radius * scale));
                }
            }
            _ => {}
        }
    }
    s
}

pub(crate) fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

pub(crate) fn contains(lo: [f64; 2], hi: [f64; 2], p: [f64; 2], tol: f64) -> bool {
    p[0] >= lo[0] - tol && p[0] <= hi[0] + tol && p[1] >= lo[1] - tol && p[1] <= hi[1] + tol
}

// ---- Words -------------------------------------------------------------------------------------

/// `5/8`, `3-5/8`, `3 5/8`, `7`, `7.09` → a number.
pub(crate) fn number(s: &str) -> Option<f64> {
    let s = s.trim();
    let frac = |f: &str| -> Option<f64> {
        let (a, b) = f.split_once('/')?;
        let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
        (b > 0.0).then(|| a / b)
    };
    if let Some(v) = frac(s) {
        return Some(v);
    }
    for sep in ['-', ' '] {
        if let Some((w, f)) = s.split_once(sep)
            && let (Ok(w), Some(f)) = (w.trim().parse::<f64>(), frac(f))
        {
            return Some(w + f);
        }
    }
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `3'-0"`, `8'-6 1/2"`, `36"`, `4'` → inches.
pub(crate) fn length_in(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some((f, rest)) = s.split_once('\'') {
        let f: f64 = f.trim().parse().ok()?;
        let rest = rest.trim_start_matches('-').trim();
        let i = rest.split('"').next().map(str::trim).filter(|x| !x.is_empty()).map_or(Some(0.0), number)?;
        let v = f * 12.0 + i;
        return v.is_finite().then_some(v);
    }
    number(s.trim_end_matches('"'))
}

/// The first number of inches in `s` after `key` (`ON 14" CURB` → 14).
pub(crate) fn inches_in(s: &str) -> Option<f64> {
    let i = s.find('"')?;
    let head = s.get(..i)?;
    let start = head.rfind(|c: char| !(c.is_ascii_digit() || c == '.' || c == '/' || c == '-' || c == '\'' || c == ' ')).map_or(0, |k| k + 1);
    length_in(&format!("{}\"", head.get(start..)?.trim()))
}

pub(crate) fn upper(s: &str) -> String {
    s.to_ascii_uppercase()
}

// ---- Tables ------------------------------------------------------------------------------------

/// A table read from a schedule sheet: its title, header and rows (cells by column).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadTable {
    pub title: String,
    pub header: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl ReadTable {
    pub(crate) fn col(&self, f: &dyn Fn(&str) -> bool) -> Option<usize> {
        self.header.iter().position(|h| f(&upper(h)))
    }
}

/// Every table on a sheet whose title has one of `titles` in it.
pub(crate) fn tables(s: &Shapes, titles: &[&str]) -> Vec<ReadTable> {
    let mut out = Vec::new();
    for t in s.texts.iter().filter(|t| titles.iter().any(|k| upper(&t.s).contains(k))) {
        // A title sits above its table, not in a cell of one.
        if s.boxes.iter().any(|(_, lo, hi)| contains(*lo, *hi, t.at, 0.0) && hi[1] - lo[1] < 4.0 * t.h) {
            continue;
        }
        // The header row: the first row of texts under the title, starting at its left.
        let below: Vec<&Txt> =
            s.texts.iter().filter(|x| x.at[1] < t.at[1] - 0.3 * t.h && x.at[1] > t.at[1] - 5.0 * t.h && x.at[0] > t.at[0] - t.h).collect();
        // Its first cell sits right under the title's start.
        let Some(top) = below.iter().filter(|x| x.at[0] < t.at[0] + 3.0 * t.h).map(|x| x.at[1]).reduce(f64::max) else { continue };
        let h = below.iter().find(|x| (x.at[1] - top).abs() < 1e-9).map_or(t.h * 0.7, |x| x.h);
        let mut header: Vec<&Txt> = below.iter().copied().filter(|x| (x.at[1] - top).abs() < 0.5 * h).collect();
        header.sort_by(|a, b| a.at[0].total_cmp(&b.at[0]));
        // Each header cell's box: the table ends at the last box's right edge.
        let cell = |x: &Txt| {
            s.boxes
                .iter()
                .filter(|(_, lo, hi)| contains(*lo, *hi, x.at, 0.0) && hi[1] - lo[1] < 6.0 * h)
                .min_by(|a, b| ((a.2[0] - a.1[0]) * (a.2[1] - a.1[1])).total_cmp(&((b.2[0] - b.1[0]) * (b.2[1] - b.1[1]))))
                .map(|(_, lo, hi)| (*lo, *hi))
        };
        let mut cols: Vec<(f64, String)> = Vec::new();
        let mut right = f64::MIN;
        let mut row_h = 2.7 * h;
        for (i, x) in header.iter().enumerate() {
            let Some((lo, hi)) = cell(x) else {
                if i == 0 {
                    break;
                }
                continue;
            };
            // Contiguous cells only (the next table along starts after a gap).
            if i > 0 && lo[0] > right + 0.5 * h {
                break;
            }
            cols.push((lo[0], x.s.clone()));
            right = hi[0];
            row_h = hi[1] - lo[1];
        }
        let Some(left) = cols.first().map(|c| c.0) else { continue };
        let mut cells: Vec<&Txt> = s.texts.iter().filter(|x| x.at[0] > left && x.at[0] < right && x.at[1] < top - 0.5 * h).collect();
        cells.sort_by(|a, b| b.at[1].total_cmp(&a.at[1]));
        let mut rows: Vec<Vec<String>> = Vec::new();
        let mut y = top;
        let mut i = 0;
        while let Some(first) = cells.get(i) {
            if y - first.at[1] > 1.6 * row_h || rows.len() >= MAX_ROWS {
                break;
            }
            let ry = first.at[1];
            let mut row = vec![String::new(); cols.len()];
            while let Some(c) = cells.get(i) {
                if (c.at[1] - ry).abs() > 0.5 * h {
                    break;
                }
                if let Some(k) = cols.iter().rposition(|(x, _)| *x <= c.at[0])
                    && let Some(slot) = row.get_mut(k)
                {
                    if !slot.is_empty() {
                        slot.push(' ');
                    }
                    slot.push_str(&c.s);
                }
                i += 1;
            }
            rows.push(row);
            y = ry;
        }
        out.push(ReadTable { title: t.s.clone(), header: cols.into_iter().map(|c| c.1).collect(), rows });
    }
    out
}

pub(crate) fn cell(row: &[String], k: Option<usize>) -> String {
    k.and_then(|k| row.get(k)).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn door_rows(t: &ReadTable) -> Vec<DoorRow> {
    let mark = t.col(&|h| h == "MARK" || h.starts_with("DOOR NO") || h == "NO." || h == "DOOR");
    let width = t.col(&|h| h == "WD" || h.starts_with("WIDTH"));
    let height = t.col(&|h| h == "HT" || h.starts_with("HEIGHT"));
    let ty = t.col(&|h| h.contains("TYPE"));
    let matl = t.col(&|h| h.starts_with("MAT"));
    let frame = t.col(&|h| h.starts_with("FRAME"));
    let rating = t.col(&|h| h.contains("RATING") || h.starts_with("LABEL"));
    let hw = t.col(&|h| h.starts_with("HW") || h.starts_with("HARDWARE") || h.starts_with("HDW"));
    t.rows
        .iter()
        .filter_map(|r| {
            let m = cell(r, mark);
            if m.is_empty() {
                return None;
            }
            let rt = cell(r, rating);
            let fire_rating = match rt.trim_end_matches("MIN").trim().parse::<u32>() {
                Ok(n) if n > 0 => Some(format!("{n} MIN")),
                _ => None,
            };
            Some(DoorRow {
                mark: m,
                width_in: length_in(&cell(r, width)),
                height_in: length_in(&cell(r, height)),
                door_type: cell(r, ty),
                material: cell(r, matl),
                frame: cell(r, frame),
                fire_rating,
                hardware: cell(r, hw),
            })
        })
        .collect()
}

fn hardware_groups(t: &ReadTable) -> Vec<HardwareGroup> {
    t.rows
        .iter()
        .filter_map(|r| {
            let g = r.first()?.trim().to_string();
            let items: Vec<String> = r.get(1)?.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            (!g.is_empty()).then(|| HardwareGroup { card_reader: items.iter().any(|i| upper(i).contains("CARD READER")), group: g, items })
        })
        .collect()
}

fn window_types(t: &ReadTable) -> Vec<WindowType> {
    let mark = t.col(&|h| h == "MARK" || h == "TYPE");
    let desc = t.col(&|h| h.starts_with("DESC"));
    let width = t.col(&|h| h.starts_with("WIDTH") || h == "WD");
    let height = t.col(&|h| h.starts_with("HEIGHT") || h == "HT");
    let sill = t.col(&|h| h.starts_with("SILL"));
    let frame = t.col(&|h| h.starts_with("FRAME"));
    let glazing = t.col(&|h| h.starts_with("GLAZ"));
    t.rows
        .iter()
        .filter_map(|r| {
            let m = cell(r, mark);
            let d = cell(r, desc);
            (!m.is_empty()).then(|| WindowType {
                mark: m,
                storefront: upper(&d).contains("STOREFRONT"),
                description: d,
                width_in: length_in(&cell(r, width)).unwrap_or(0.0),
                height_in: length_in(&cell(r, height)).unwrap_or(0.0),
                sill_in: length_in(&cell(r, sill)).unwrap_or(0.0),
                frame: cell(r, frame),
                glazing: cell(r, glazing),
            })
        })
        .collect()
}

/// A finish schedule row: (number, name, floor, base, wall, ceiling, ceiling height).
type FinishRow = (String, String, String, String, String, String, Option<f64>);

fn finish_rows(t: &ReadTable) -> Vec<FinishRow> {
    let num = t.col(&|h| h.contains("NO") || h.contains('#'));
    let name = t.col(&|h| h.contains("NAME") || h == "ROOM");
    let floor = t.col(&|h| h.starts_with("FL"));
    let base = t.col(&|h| h.starts_with("BASE"));
    let wall = t.col(&|h| h.starts_with("WALL"));
    let ceiling = t.col(&|h| (h.starts_with("CEIL") || h.starts_with("CLG")) && !h.contains("HT") && !h.contains("HEIGHT"));
    let height = t.col(&|h| h.contains("HT") || h.contains("HEIGHT"));
    t.rows
        .iter()
        .filter_map(|r| {
            let n = cell(r, num);
            (!n.is_empty()).then(|| {
                (n, cell(r, name), cell(r, floor), cell(r, base), cell(r, wall), cell(r, ceiling), length_in(&cell(r, height)).map(|v| v / 12.0))
            })
        })
        .collect()
}

fn finish_legend(t: &ReadTable) -> Vec<(String, String, Option<String>)> {
    let code = t.col(&|h| h == "CODE" || h == "MARK");
    let desc = t.col(&|h| h.starts_with("DESC"));
    let plates = t.col(&|h| h.contains("PLATE"));
    t.rows
        .iter()
        .filter_map(|r| {
            let c = cell(r, code);
            if c.is_empty() {
                return None;
            }
            let mut d = cell(r, desc);
            let mut p = Some(cell(r, plates)).filter(|s| !s.is_empty());
            let key = "; DEVICE PLATES:";
            if let Some(i) = upper(&d).find(key) {
                p = d.get(i + key.len()..).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                d = d.get(..i).unwrap_or_default().trim().to_string();
            }
            Some((c, d, p))
        })
        .collect()
}

// ---- Plans -------------------------------------------------------------------------------------

/// The smallest room holding a point (an outline round the whole floor holds them all).
fn room_at<'a>(cl: &'a CeilingResult, level: &str, p: [f64; 2]) -> Option<&'a crate::ceiling_reader::FoundRoom> {
    let area = |r: &&crate::ceiling_reader::FoundRoom| (r.max_ft[0] - r.min_ft[0]) * (r.max_ft[1] - r.min_ft[1]);
    cl.rooms.iter().filter(|r| r.level == level && contains(r.min_ft, r.max_ft, p, 0.0)).min_by(|a, b| area(a).total_cmp(&area(b)))
}

fn read_doors(s: &Shapes, level: &str, cl: &CeilingResult, edge: Option<&FoundSlabEdge>, marks: &[String]) -> Vec<FoundDoor> {
    let mut out = Vec::new();
    for (layer, c, r, ps, pe) in s.arcs.iter().filter(|a| a.0.contains("DOOR")) {
        let r_ft = *r;
        if !(1.0..=5.0).contains(&r_ft) {
            continue;
        }
        // The leaf: a line on the door layer from the hinge.
        let tip = s.lines.iter().filter(|(l, _, _)| l == layer).find_map(|(_, a, b)| {
            if dist(*a, *c) < 0.02 * r_ft {
                Some(*b)
            } else if dist(*b, *c) < 0.02 * r_ft {
                Some(*a)
            } else {
                None
            }
        });
        let Some(tip) = tip else { continue };
        let closed = if dist(*ps, tip) > dist(*pe, tip) { *ps } else { *pe };
        let w = dist(closed, *c);
        if w <= 1e-9 {
            continue;
        }
        let u = [(closed[0] - c[0]) / w, (closed[1] - c[1]) / w];
        let n = [-u[1], u[0]];
        let lt = dist(tip, *c).max(1e-9);
        let into_v = [(tip[0] - c[0]) / lt, (tip[1] - c[1]) / lt];
        let into = if into_v[0] * n[0] + into_v[1] * n[1] >= 0.0 { 1.0 } else { -1.0 };
        let mid = [(c[0] + closed[0]) / 2.0, (c[1] + closed[1]) / 2.0];
        let out_of_building = |p: [f64; 2]| edge.is_some_and(|e| !contains(e.min_ft, e.max_ft, p, 0.0));
        let side = |sgn: f64, d: f64| {
            let p = [mid[0] + n[0] * sgn * d, mid[1] + n[1] * sgn * d];
            if out_of_building(p) { None } else { room_at(cl, level, p) }
        };
        // The swing side's face is the hinge's; the other room is past the wall.
        let (plus, minus) = if into > 0.0 { (side(1.0, 2.0), side(-1.0, 2.5)) } else { (side(1.0, 2.5), side(-1.0, 2.0)) };
        let (into_room, other) = if into > 0.0 { (plus, minus) } else { (minus, plus) };
        let outside = outside_side(plus.map(|r| r.name.to_ascii_uppercase()).as_deref(), minus.map(|r| r.name.to_ascii_uppercase()).as_deref(), into);
        let hd = hand(*c, closed, [n[0] * into, n[1] * into], [n[0] * outside, n[1] * outside]);
        out.push(FoundDoor {
            level: level.into(),
            mark: None,
            at_ft: mid,
            hinge_ft: *c,
            latch_ft: closed,
            width_in: (w * 12.0).round(),
            swings_into: into_room.map(|r| r.number.clone()),
            from_room: other.map(|r| r.number.clone()),
            hand: hd,
            row: None,
            card_reader: false,
        });
    }
    // Marks: each schedule mark on the plan goes to the nearest door without one.
    let mut cands: Vec<(f64, usize, String)> = Vec::new();
    for t in s.texts.iter().filter(|t| !t.grid && marks.iter().any(|m| m.eq_ignore_ascii_case(&t.s))) {
        for (i, d) in out.iter().enumerate() {
            let dd = dist(t.at, d.at_ft);
            if dd < 4.0 {
                cands.push((dd, i, t.s.clone()));
            }
        }
    }
    cands.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut used: Vec<String> = Vec::new();
    for (_, i, m) in cands {
        if used.contains(&m) {
            continue;
        }
        if let Some(d) = out.get_mut(i)
            && d.mark.is_none()
        {
            d.mark = Some(m.clone());
            used.push(m);
        }
    }
    out
}

fn read_windows(s: &Shapes, level: &str, marks: &[String]) -> Vec<FoundWindow> {
    let mut out: Vec<FoundWindow> = Vec::new();
    for (_, lo, hi) in s.boxes.iter().filter(|b| b.0.contains("GLAZ") || b.0.contains("WIND")) {
        let (dx, dy) = (hi[0] - lo[0], hi[1] - lo[1]);
        let width = dx.max(dy);
        if !(1.0..=40.0).contains(&width) || dx.min(dy) > 2.0 {
            continue;
        }
        out.push(FoundWindow {
            level: level.into(),
            mark: None,
            at_ft: [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0],
            width_in: (width * 12.0).round(),
        });
    }
    for w in out.iter_mut() {
        w.mark = s
            .texts
            .iter()
            .filter(|t| !t.grid && marks.iter().any(|m| m.eq_ignore_ascii_case(&t.s)))
            .map(|t| (dist(t.at, w.at_ft), t))
            .filter(|(d, _)| *d < 5.0)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, t)| t.s.clone());
    }
    out
}

/// `UP 24R @ 7-1/16"` → (24, 7.0625).
fn stair_note(s: &str) -> Option<(u32, Option<f64>)> {
    let u = upper(s);
    let i = u.find('R')?;
    let n: u32 = u.get(..i)?.split_whitespace().last()?.parse().ok()?;
    let riser = u.split_once('@').and_then(|(_, r)| length_in(r.trim()));
    Some((n, riser))
}

fn read_stairs(s: &Shapes, level: &str) -> Vec<FoundStair> {
    let boxes: Vec<([f64; 2], [f64; 2])> = s.boxes.iter().filter(|b| b.0.contains("STRS") || b.0.contains("STAIR")).map(|b| (b.1, b.2)).collect();
    let lines: Vec<([f64; 2], [f64; 2])> = s.lines.iter().filter(|l| l.0.contains("STRS") || l.0.contains("STAIR")).map(|l| (l.1, l.2)).collect();
    // Group the boxes that touch into stairs.
    let mut groups: Vec<Vec<([f64; 2], [f64; 2])>> = Vec::new();
    for b in boxes.iter().take(1000) {
        let near = |g: &Vec<([f64; 2], [f64; 2])>| {
            g.iter().any(|o| b.0[0] <= o.1[0] + 1.0 && o.0[0] <= b.1[0] + 1.0 && b.0[1] <= o.1[1] + 1.0 && o.0[1] <= b.1[1] + 1.0)
        };
        match groups.iter_mut().find(|g| near(g)) {
            Some(g) => g.push(*b),
            None => groups.push(vec![*b]),
        }
    }
    let mut out = Vec::new();
    for g in groups {
        let lo = g.iter().fold([f64::MAX; 2], |m, b| [m[0].min(b.0[0]), m[1].min(b.0[1])]);
        let hi = g.iter().fold([f64::MIN; 2], |m, b| [m[0].max(b.1[0]), m[1].max(b.1[1])]);
        // Flights: boxes with treads across them (lines as long as the box is narrow).
        let mut width = 0.0;
        let mut treads_total = 0u32;
        let mut tread = 0.0;
        let mut flights = 0u32;
        for (blo, bhi) in &g {
            let (dx, dy) = (bhi[0] - blo[0], bhi[1] - blo[1]);
            let narrow = dx.min(dy);
            let inner: Vec<f64> = lines
                .iter()
                .filter(|(a, b)| {
                    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                    contains(*blo, *bhi, mid, 1e-6) && (dist(*a, *b) - narrow).abs() < 0.05
                })
                .map(|(a, _)| if dx < dy { a[1] } else { a[0] })
                .collect();
            if inner.is_empty() {
                continue;
            }
            flights += 1;
            width = narrow;
            let n = inner.len() as u32 + 1;
            treads_total += n;
            tread = dx.max(dy) / n as f64;
        }
        if flights == 0 {
            continue;
        }
        let tag = s
            .texts
            .iter()
            .filter(|t| upper(&t.s).starts_with("STAIR") && contains(lo, hi, t.at, 1.0))
            .map(|t| t.s.clone())
            .next()
            .unwrap_or_else(|| "STAIR".into());
        let note = s.texts.iter().filter(|t| contains(lo, hi, t.at, 3.0)).find_map(|t| {
            let u = upper(&t.s);
            if u.starts_with("UP") || u.starts_with("DN") { stair_note(&u) } else { None }
        });
        let (risers, riser_in) = note.unwrap_or((treads_total + flights, None));
        out.push(FoundStair {
            tag,
            levels: vec![level.into()],
            min_ft: lo,
            max_ft: hi,
            width_in: (width * 12.0).round(),
            risers,
            riser_in,
            tread_in: tread * 12.0,
        });
    }
    out
}

fn read_elevators(s: &Shapes, level: &str) -> Vec<FoundElevator> {
    let boxes: Vec<([f64; 2], [f64; 2])> = s.boxes.iter().filter(|b| b.0.contains("ELEV")).map(|b| (b.1, b.2)).collect();
    let lines: Vec<([f64; 2], [f64; 2])> = s.lines.iter().filter(|l| l.0.contains("ELEV")).map(|l| (l.1, l.2)).collect();
    let mut out = Vec::new();
    for (lo, hi) in &boxes {
        // The hoistway has the X: a line from corner to corner.
        let x = lines.iter().any(|(a, b)| (dist(*a, *lo) < 0.05 && dist(*b, *hi) < 0.05) || (dist(*b, *lo) < 0.05 && dist(*a, *hi) < 0.05));
        if !x {
            continue;
        }
        let words: Vec<String> = s
            .texts
            .iter()
            .filter(|t| t.at[0] > lo[0] - 3.0 && t.at[0] < hi[0] + 3.0 && t.at[1] > lo[1] - 4.0 && t.at[1] < hi[1] + 1.0)
            .map(|t| upper(&t.s))
            .collect();
        let tag = words.iter().find(|w| w.starts_with("ELEV")).cloned().unwrap_or_else(|| "ELEV.".into());
        let capacity_lb = words.iter().find_map(|w| w.strip_suffix(" LB").and_then(|n| n.trim().parse::<u32>().ok()));
        let pit_ft = words.iter().find_map(|w| w.strip_prefix("PIT").and_then(|r| length_in(r.trim_start_matches(':').trim())).map(|v| v / 12.0));
        out.push(FoundElevator { tag, levels: vec![level.into()], min_ft: *lo, max_ft: *hi, pit_ft, capacity_lb });
    }
    out
}

fn read_slab_edge(s: &Shapes, level: &str) -> Option<FoundSlabEdge> {
    s.boxes
        .iter()
        .filter(|b| b.0.contains("SLAB"))
        .max_by(|a, b| ((a.2[0] - a.1[0]) * (a.2[1] - a.1[1])).total_cmp(&((b.2[0] - b.1[0]) * (b.2[1] - b.1[1]))))
        .map(|(_, lo, hi)| FoundSlabEdge { level: level.into(), min_ft: *lo, max_ft: *hi })
}

fn read_roof(s: &Shapes) -> Option<FoundRoof> {
    let area = |b: &&(String, [f64; 2], [f64; 2])| (b.2[0] - b.1[0]) * (b.2[1] - b.1[1]);
    let mut roof_boxes: Vec<&(String, [f64; 2], [f64; 2])> = s.boxes.iter().filter(|b| b.0.contains("ROOF") && !b.0.contains("CURB")).collect();
    roof_boxes.sort_by(|a, b| area(b).total_cmp(&area(a)));
    let (_, lo, hi) = roof_boxes.first()?;
    let mut roof = FoundRoof { min_ft: *lo, max_ft: *hi, ..FoundRoof::default() };
    for t in &s.texts {
        let u = upper(&t.s);
        if u.contains("PARAPET") {
            roof.parapet_in = inches_in(&u);
        }
        if let Some(r) = u.strip_prefix("SLOPE") {
            roof.slope_in_ft = r.split('/').next().and_then(|_| {
                let v = r.trim().trim_end_matches("/FT").trim_end_matches(" PER FT");
                length_in(v)
            });
        }
    }
    let rd: Vec<[f64; 2]> = s.texts.iter().filter(|t| upper(&t.s) == "RD").map(|t| t.at).collect();
    for (_, c, _) in s.circles.iter().filter(|c| c.0.contains("ROOF") || c.0.contains("DRAN") || c.0.contains("DRAIN")) {
        if rd.iter().any(|p| dist(*p, *c) < 3.0) {
            roof.drains.push(*c);
        }
    }
    for (_, lo, hi) in s.boxes.iter().filter(|b| b.0.contains("CURB")) {
        let mid = (lo[0] + hi[0]) / 2.0;
        let mut words: Vec<&Txt> = s.texts.iter().filter(|t| (t.at[0] - mid).abs() < 4.0 && t.at[1] < lo[1] && t.at[1] > lo[1] - 3.0).collect();
        words.sort_by(|a, b| b.at[1].total_cmp(&a.at[1]));
        let tag = words.first().map(|t| t.s.clone()).unwrap_or_default();
        let note = words.get(1).map(|t| upper(&t.s)).unwrap_or_default();
        let tu = upper(&tag);
        let kind = if tu.contains("HATCH") {
            "hatch"
        } else if tu.contains("OVERRUN") {
            "overrun"
        } else {
            "equipment"
        };
        let height_in =
            if kind == "overrun" { length_in(note.trim_start_matches('+').split(" ABOVE").next().unwrap_or_default()) } else { inches_in(&note) };
        roof.curbs.push(FoundCurb { tag, kind: kind.into(), min_ft: *lo, max_ft: *hi, height_in });
    }
    Some(roof)
}

/// Room labels: a name over a number.
fn read_rooms(s: &Shapes, level: &str) -> Vec<FoundRoom> {
    let mut out = Vec::new();
    for n in s.texts.iter().filter(|t| !t.grid && (2..=4).contains(&t.s.len()) && t.s.chars().all(|c| c.is_ascii_digit())) {
        let name = s
            .texts
            .iter()
            .filter(|t| (t.at[0] - n.at[0]).abs() < 0.3 * n.h.max(0.1) && t.at[1] > n.at[1] && t.at[1] < n.at[1] + 2.5 * n.h.max(0.1))
            .find(|t| t.s.chars().any(|c| c.is_ascii_alphabetic()));
        if let Some(name) = name {
            out.push(FoundRoom { level: level.into(), number: n.s.clone(), name: name.s.clone(), at_ft: n.at, ..FoundRoom::default() });
        }
    }
    out
}

/// Merge what one level found of a stair or elevator with what the others found (same place).
fn merge_levels<T>(all: &mut Vec<T>, found: Vec<T>, rect: impl Fn(&T) -> ([f64; 2], [f64; 2]), levels: impl Fn(&mut T) -> &mut Vec<String>) {
    for mut f in found {
        let (lo, hi) = rect(&f);
        match all.iter_mut().find(|o| {
            let (a, b) = rect(o);
            dist(a, lo) < 1.0 && dist(b, hi) < 1.0
        }) {
            Some(o) => {
                let lv: Vec<String> = levels(&mut f).clone();
                let ol = levels(o);
                for l in lv {
                    if !ol.contains(&l) {
                        ol.push(l);
                    }
                }
                ol.sort();
            }
            None => all.push(f),
        }
    }
}

/// Read the architecture of a set.
pub fn read_arch(dxf: &[(String, Vec<u8>)], map: &SheetMap, ceilings: &CeilingResult) -> ArchResult {
    let mut res = ArchResult::default();
    let mut plans: Vec<(String, Shapes)> = Vec::new();
    let mut roof_plan: Option<Shapes> = None;
    let mut finishes: Vec<FinishRow> = Vec::new();
    let mut legend: Vec<(String, String, Option<String>)> = Vec::new();
    for (name, bytes) in dxf {
        let stem = name.trim_end_matches(".dxf");
        let Some(sh) = map.sheets.iter().find(|s| s.number == stem) else { continue };
        if sh.discipline != "A" {
            continue;
        }
        let title = upper(&sh.title);
        let plan = title.contains("FLOOR PLAN") || title.contains("ROOF PLAN");
        let schedule = title.contains("SCHEDULE") && !title.contains("PARTITION") && !title.contains("WALL TYPE");
        if !plan && !schedule {
            continue;
        }
        let Ok(d) = cadaddy_io::read_dxf(bytes) else { continue };
        if plan {
            let (Some(level), Some(al)) = (&sh.level, &sh.alignment) else { continue };
            let al: Alignment = al.clone();
            let s = shapes(&d, &|p| sheet_reader::building_from_model(&al, p), 1.0 / 12.0);
            if title.contains("ROOF") {
                roof_plan = Some(s);
            } else {
                plans.push((level.clone(), s));
            }
            continue;
        }
        let s = shapes(&d, &|p| p, 1.0);
        for t in tables(&s, &["DOOR SCHEDULE", "HARDWARE", "WINDOW SCHEDULE", "ROOM FINISHES", "FINISH LEGEND"]) {
            let tt = upper(&t.title);
            if tt.contains("DOOR SCHEDULE") {
                res.door_rows.extend(door_rows(&t));
            } else if tt.contains("HARDWARE") {
                res.hardware.extend(hardware_groups(&t));
            } else if tt.contains("WINDOW") {
                res.window_types.extend(window_types(&t));
            } else if tt.contains("ROOM FINISHES") {
                finishes.extend(finish_rows(&t));
            } else {
                legend.extend(finish_legend(&t));
            }
        }
    }
    let door_marks: Vec<String> = res.door_rows.iter().map(|r| r.mark.clone()).collect();
    let window_marks: Vec<String> = res.window_types.iter().map(|w| w.mark.clone()).collect();
    plans.sort_by(|a, b| a.0.cmp(&b.0));
    for (level, s) in &plans {
        let edge = read_slab_edge(s, level);
        let mut doors = read_doors(s, level, ceilings, edge.as_ref(), &door_marks);
        for d in doors.iter_mut() {
            d.row = d.mark.as_ref().and_then(|m| res.door_rows.iter().find(|r| r.mark.eq_ignore_ascii_case(m))).cloned();
            d.card_reader = d.row.as_ref().is_some_and(|r| res.hardware.iter().any(|g| g.group.eq_ignore_ascii_case(&r.hardware) && g.card_reader));
        }
        res.doors.extend(doors);
        res.windows.extend(read_windows(s, level, &window_marks));
        let stairs = read_stairs(s, level);
        merge_levels(&mut res.stairs, stairs, |x| (x.min_ft, x.max_ft), |x| &mut x.levels);
        let elevators = read_elevators(s, level);
        for e in &elevators {
            if let Some(o) = res.elevators.iter_mut().find(|o| dist(o.min_ft, e.min_ft) < 1.0 && dist(o.max_ft, e.max_ft) < 1.0) {
                o.pit_ft = o.pit_ft.or(e.pit_ft);
                o.capacity_lb = o.capacity_lb.or(e.capacity_lb);
            }
        }
        merge_levels(&mut res.elevators, elevators, |x| (x.min_ft, x.max_ft), |x| &mut x.levels);
        res.slab_edges.extend(edge);
        res.rooms.extend(read_rooms(s, level));
    }
    if let Some(s) = &roof_plan {
        res.roof = read_roof(s);
        if let Some(r) = &res.roof {
            res.slab_edges.push(FoundSlabEdge { level: "R".into(), min_ft: r.min_ft, max_ft: r.max_ft });
        }
    }
    // Finishes onto the rooms, and each legend code's kind from the column that uses it.
    for rm in res.rooms.iter_mut() {
        if let Some(f) = finishes.iter().find(|f| f.0 == rm.number) {
            rm.floor = Some(f.2.clone());
            rm.base = Some(f.3.clone());
            rm.wall = Some(f.4.clone());
            rm.ceiling = Some(f.5.clone());
            rm.ceiling_ft = f.6;
            rm.plates = legend.iter().find(|l| l.0 == f.4).and_then(|l| l.2.clone());
        }
    }
    for (code, description, plates) in legend {
        let kind = if finishes.iter().any(|f| f.2 == code) {
            "floor"
        } else if finishes.iter().any(|f| f.3 == code) {
            "base"
        } else if finishes.iter().any(|f| f.4 == code) {
            "wall"
        } else if finishes.iter().any(|f| f.5 == code) {
            "ceiling"
        } else {
            ""
        };
        res.finish_codes.push(FinishCode { code, kind: kind.into(), description, plates });
    }
    res
}

/// Generate a set, recognise its sheets, read ceilings (for the rooms), then the architecture.
pub fn read_fixture_arch(seed: u64) -> Result<(Truth, ArchResult), String> {
    let set = crate::generate(seed)?;
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = sheet_reader::recognise_fixture_set(&dxf);
    let ceilings = crate::ceiling_reader::read_ceilings(&dxf, &map, None);
    Ok((set.truth, read_arch(&dxf, &map, &ceilings)))
}

// ---- Scoring -----------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct ArchScore {
    pub doors_total: usize,
    pub doors_found: usize,
    pub door_marks: usize,
    pub door_hands: usize,
    pub door_rooms: usize,
    /// Found doors whose schedule row (size, type, material, frame, rating, hardware) is right.
    pub door_rows: usize,
    pub door_card_readers: usize,
    pub hardware_total: usize,
    pub hardware_correct: usize,
    pub windows_total: usize,
    pub windows_found: usize,
    pub window_marks: usize,
    pub window_types_total: usize,
    pub window_types_correct: usize,
    pub stairs_total: usize,
    pub stairs_correct: usize,
    pub elevators_total: usize,
    pub elevators_correct: usize,
    pub slab_edges_total: usize,
    pub slab_edges_correct: usize,
    pub roof_correct: bool,
    pub drains_total: usize,
    pub drains_found: usize,
    pub curbs_total: usize,
    pub curbs_correct: usize,
    pub rooms_total: usize,
    pub rooms_found: usize,
    pub finishes_correct: usize,
    pub codes_total: usize,
    pub codes_correct: usize,
    /// What didn't match, for test messages.
    pub misses: Vec<String>,
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn rect_near(a: ([f64; 2], [f64; 2]), b: ([f64; 2], [f64; 2]), tol: f64) -> bool {
    dist(a.0, b.0) <= tol && dist(a.1, b.1) <= tol
}

pub fn score_arch(t: &Truth, r: &ArchResult) -> ArchScore {
    let mut sc = ArchScore::default();
    let Some(a) = t.arch.as_ref() else { return sc };
    sc.doors_total = a.doors.len();
    for d in &a.doors {
        let Some(f) = r.doors.iter().filter(|f| f.level == d.level).min_by(|x, y| dist(x.at_ft, d.at_ft).total_cmp(&dist(y.at_ft, d.at_ft))) else {
            sc.misses.push(format!("door {} not found", d.mark));
            continue;
        };
        if dist(f.at_ft, d.at_ft) > 1.5 || !near(f.width_in, d.width_in, 1.0) {
            sc.misses.push(format!("door {} not found", d.mark));
            continue;
        }
        sc.doors_found += 1;
        if f.mark.as_deref() == Some(d.mark.as_str()) {
            sc.door_marks += 1;
        } else {
            sc.misses.push(format!("door {} read as {:?}", d.mark, f.mark));
        }
        if f.hand == d.hand && dist(f.hinge_ft, d.hinge_ft) < 0.1 {
            sc.door_hands += 1;
        } else {
            sc.misses.push(format!("door {} hand {} read {}", d.mark, d.hand, f.hand));
        }
        if f.swings_into == d.swings_into && f.from_room == d.from_room {
            sc.door_rooms += 1;
        } else {
            sc.misses.push(format!("door {} rooms {:?}/{:?} read {:?}/{:?}", d.mark, d.swings_into, d.from_room, f.swings_into, f.from_room));
        }
        let row_ok = f.row.as_ref().is_some_and(|row| {
            row.width_in.is_some_and(|w| near(w, d.width_in, 0.5))
                && row.height_in.is_some_and(|h| near(h, d.height_in, 0.5))
                && row.door_type == d.door_type
                && row.material == d.material
                && row.frame == d.frame
                && row.fire_rating == d.fire_rating
                && row.hardware == d.hardware
        });
        if row_ok {
            sc.door_rows += 1;
        } else {
            sc.misses.push(format!("door {} row {:?}", d.mark, f.row));
        }
        let card = a.hardware.iter().any(|g| g.group == d.hardware && g.card_reader);
        if f.card_reader == card {
            sc.door_card_readers += 1;
        }
    }
    sc.hardware_total = a.hardware.len();
    sc.hardware_correct = a.hardware.iter().filter(|g| r.hardware.iter().any(|f| f == *g)).count();
    sc.windows_total = a.windows.len();
    for w in &a.windows {
        let Some(f) = r.windows.iter().filter(|f| f.level == w.level).find(|f| dist(f.at_ft, w.at_ft) < 1.0) else {
            sc.misses.push(format!("window at {:?} not found", w.at_ft));
            continue;
        };
        let width = a.window_types.iter().find(|x| x.mark == w.mark).map_or(0.0, |x| x.width_in);
        if near(f.width_in, width, 1.0) {
            sc.windows_found += 1;
        }
        if f.mark.as_deref() == Some(w.mark.as_str()) {
            sc.window_marks += 1;
        } else {
            sc.misses.push(format!("window {} read as {:?}", w.mark, f.mark));
        }
    }
    sc.window_types_total = a.window_types.len();
    sc.window_types_correct = a.window_types.iter().filter(|w| r.window_types.iter().any(|f| f == *w)).count();
    sc.stairs_total = a.stairs.len();
    for s in &a.stairs {
        let ok = r.stairs.iter().any(|f| {
            rect_near((f.min_ft, f.max_ft), (s.min_ft, s.max_ft), 0.1)
                && near(f.width_in, s.width_in, 1.0)
                && f.risers == s.risers
                && f.riser_in.is_some_and(|x| near(x, s.riser_in, 0.07))
                && near(f.tread_in, s.tread_in, 0.25)
                && f.levels == s.levels
        });
        if ok {
            sc.stairs_correct += 1;
        } else {
            sc.misses.push(format!("stair {} {:?} read {:?}", s.tag, s, r.stairs));
        }
    }
    sc.elevators_total = a.elevators.len();
    for e in &a.elevators {
        let ok = r.elevators.iter().any(|f| {
            rect_near((f.min_ft, f.max_ft), (e.min_ft, e.max_ft), 0.1)
                && f.pit_ft.is_some_and(|p| near(p, e.pit_ft, 0.01))
                && f.capacity_lb == Some(e.capacity_lb)
                && f.levels == e.levels
                && f.tag == e.tag
        });
        if ok {
            sc.elevators_correct += 1;
        } else {
            sc.misses.push(format!("elevator {} {:?} read {:?}", e.tag, e, r.elevators));
        }
    }
    sc.slab_edges_total = a.slab_edges.len();
    for se in &a.slab_edges {
        if r.slab_edges.iter().any(|f| f.level == se.level && rect_near((f.min_ft, f.max_ft), (se.min_ft, se.max_ft), 0.1)) {
            sc.slab_edges_correct += 1;
        } else {
            sc.misses.push(format!("slab edge {} {:?}", se.level, se));
        }
    }
    let rf = &a.roof;
    if let Some(f) = &r.roof {
        sc.roof_correct = rect_near((f.min_ft, f.max_ft), (rf.min_ft, rf.max_ft), 0.1)
            && f.parapet_in == rf.parapet_in
            && f.slope_in_ft.is_some_and(|s| near(s, rf.slope_in_ft, 1e-6));
        if !sc.roof_correct {
            sc.misses.push(format!(
                "roof {:?} read {:?} {:?} {:?}",
                (rf.min_ft, rf.max_ft, rf.parapet_in, rf.slope_in_ft),
                (f.min_ft, f.max_ft),
                f.parapet_in,
                f.slope_in_ft
            ));
        }
        sc.drains_total = rf.drains.len();
        sc.drains_found = rf.drains.iter().filter(|d| f.drains.iter().any(|x| dist(*x, **d) < 0.5)).count();
        sc.curbs_total = rf.curbs.len();
        for c in &rf.curbs {
            let ok = f.curbs.iter().any(|x| {
                x.tag == c.tag
                    && x.kind == c.kind
                    && rect_near((x.min_ft, x.max_ft), (c.min_ft, c.max_ft), 0.1)
                    && x.height_in.is_some_and(|h| near(h, c.height_in, 0.5))
            });
            if ok {
                sc.curbs_correct += 1;
            } else {
                sc.misses.push(format!("curb {:?} read {:?}", c, f.curbs));
            }
        }
    } else {
        sc.drains_total = rf.drains.len();
        sc.curbs_total = rf.curbs.len();
        sc.misses.push("no roof plan read".into());
    }
    sc.rooms_total = a.finishes.len();
    for fin in &a.finishes {
        let Some(rm) = r.rooms.iter().find(|x| x.level == fin.level && x.number == fin.number) else {
            sc.misses.push(format!("room {} not found", fin.number));
            continue;
        };
        let truth_room = t.building.rooms.iter().find(|x| x.level == fin.level && x.number == fin.number);
        let placed = truth_room.is_some_and(|tr| contains(tr.min_ft, tr.max_ft, rm.at_ft, 0.0));
        if rm.name == fin.name && placed {
            sc.rooms_found += 1;
        } else {
            sc.misses.push(format!("room {} {} read {} at {:?}", fin.number, fin.name, rm.name, rm.at_ft));
        }
        let plates = a.finish_codes.iter().find(|c| c.code == fin.wall).and_then(|c| c.plates.clone());
        let ok = rm.floor.as_deref() == Some(fin.floor.as_str())
            && rm.base.as_deref() == Some(fin.base.as_str())
            && rm.wall.as_deref() == Some(fin.wall.as_str())
            && rm.ceiling.as_deref() == Some(fin.ceiling.as_str())
            && match (rm.ceiling_ft, fin.ceiling_ft) {
                (Some(x), Some(y)) => near(x, y, 0.01),
                (None, None) => true,
                _ => false,
            }
            && rm.plates == plates;
        if ok {
            sc.finishes_correct += 1;
        } else {
            sc.misses.push(format!("room {} finishes {:?} read {:?}", fin.number, fin, rm));
        }
    }
    sc.codes_total = a.finish_codes.len();
    sc.codes_correct = a.finish_codes.iter().filter(|c| r.finish_codes.iter().any(|f| f == *c)).count();
    sc
}

// ---- Building spec -----------------------------------------------------------------------------

/// The building spec patch for what was read: rooms with their finishes and ceiling heights,
/// the door and window schedule rows, and a tag per door and window found (its mark at its
/// place). `to_plan` maps a level's building feet to its plan drawing's coordinates (none: the
/// level isn't built, so its rooms and tags are left out).
pub fn spec_patch(r: &ArchResult, to_plan: &dyn Fn(&str, [f64; 2]) -> Option<[f64; 2]>) -> Value {
    let rooms: Vec<Value> = r
        .rooms
        .iter()
        .filter(|rm| to_plan(&rm.level, rm.at_ft).is_some())
        .map(|rm| {
            let mut v = json!({"name": rm.name, "number": rm.number, "level": rm.level});
            if let Some(c) = rm.ceiling_ft {
                v["ceiling_ft"] = json!(c);
            }
            let mut m = json!({});
            let desc = |code: &Option<String>| {
                code.as_ref().map(|c| r.finish_codes.iter().find(|f| &f.code == c).map_or_else(|| c.clone(), |f| format!("{c} {}", f.description)))
            };
            if let Some(f) = desc(&rm.floor) {
                m["floor"] = json!(f);
            }
            if let Some(w) = desc(&rm.wall) {
                m["wall"] = json!(w);
            }
            if let Some(c) = desc(&rm.ceiling) {
                m["ceiling"] = json!(c);
            }
            v["materials"] = m;
            v
        })
        .collect();
    let mut rows: Vec<Value> = r
        .door_rows
        .iter()
        .map(|d| {
            let mut v = json!({"kind": "door", "mark": d.mark, "type": d.door_type, "frame": d.frame, "hardware": d.hardware});
            if let Some(w) = d.width_in {
                v["width_in"] = json!(w);
            }
            if let Some(h) = d.height_in {
                v["height_in"] = json!(h);
            }
            if let Some(f) = &d.fire_rating {
                v["fire_rating"] = json!(f);
            }
            v
        })
        .collect();
    rows.extend(r.window_types.iter().map(|w| {
        json!({"kind": "window", "mark": w.mark, "width_in": w.width_in, "height_in": w.height_in, "sill_ft": w.sill_in / 12.0, "type": w.description, "frame": w.frame})
    }));
    let mut tags: Vec<Value> = Vec::new();
    for d in &r.doors {
        if let (Some(m), Some(p)) = (&d.mark, to_plan(&d.level, d.at_ft)) {
            tags.push(json!({"mark": m, "at": p}));
        }
    }
    for w in &r.windows {
        if let (Some(m), Some(p)) = (&w.mark, to_plan(&w.level, w.at_ft)) {
            tags.push(json!({"mark": m, "at": p}));
        }
    }
    json!({"rooms": rooms, "schedule": {"rows": rows, "tags": tags}})
}
