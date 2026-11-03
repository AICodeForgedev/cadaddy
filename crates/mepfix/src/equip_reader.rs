//! Equipment from schedules: reads the equipment schedules on the sheets (tables of tag,
//! description, size, weight, CFM…), finds each scheduled tag on the plans and places the item as
//! a sized box there, joined to the duct and pipe runs that end at it.
//!
//! A schedule is a table drawn as cells (closed boxes, or grid lines, on any layer): a header row
//! holding the column names, one of which is the tag (`TAG`, `MARK`), a title just above it, and
//! rows of cells below. What
//! the rows are comes from the title (`AIR HANDLING UNIT SCHEDULE`), else from the tag's prefix
//! (`AHU-`, `RTU-`, `VAV-`, `EF-`, `P-`, `WH-`, `CU-`…). Tables of things that aren't equipment
//! (air devices, plumbing fixtures, the sheet index) are left out.
//!
//! On a plan the tag is a text equal to the schedule's tag, beside the item's outline (a box or,
//! for tanks, a circle) or joined to it by a leader. Outlines on an equipment layer (`EQUIP`,
//! `M-EQPM`) are taken first; a closed shape on another layer only when it is the printed size
//! (flagged). The outline gives the footprint;
//! the schedule gives the height (and the footprint when the plan has no outline); the rest comes
//! from rules (per kind), and every value says where it came from so guesses show:
//! `outline` / `schedule` / `tag` / `rule` / `run`.
//!
//! Heights: floor- and pad-mounted kinds stand on their level; boxes hung in a run (VAV boxes,
//! in-line pumps and fans) are centred on the run's height when the reader of that run knows it,
//! else on a rule height. Coordinates are building feet (see [`crate::truth`]); z is the elevation.

use crate::duct_reader;
use crate::score::{self, FoundRun};
use crate::truth::{Level, Sheet, Truth};
use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

// ---- Limits (hostile drawings) ---------------------------------------------------------------

/// Most texts, boxes and lines looked at on one sheet.
const MAX_ITEMS: usize = 200_000;
/// Most tables on one sheet, and rows in one table.
const MAX_TABLES: usize = 200;
const MAX_ROWS: usize = 2_000;
/// Most columns a table has.
const MAX_COLS: usize = 64;
/// Longest cell text kept, characters.
const MAX_CELL: usize = 200;

// ---- What the reader gives -------------------------------------------------------------------

/// One schedule table as read.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Table {
    pub sheet: String,
    pub title: String,
    pub columns: Vec<String>,
    /// Each row: column name → cell text (empty cells left out).
    pub rows: Vec<BTreeMap<String, String>>,
}

/// One scheduled piece of equipment.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Scheduled {
    pub tag: String,
    /// `ahu`, `rtu`, `vav`, `exhaust_fan`, `fan`, `pump`, `water_heater`, `condensing_unit`,
    /// `gas_meter`.
    pub kind: String,
    /// Where the kind came from: `title` or `tag` (its prefix).
    pub kind_from: String,
    pub sheet: String,
    pub title: String,
    /// The row as printed (column → text).
    pub properties: BTreeMap<String, String>,
    /// The printed size, feet: `[length, width, height]` (a missing part is 0); round items
    /// (`DIA`) have `[diameter, diameter, height]`.
    pub size_ft: Option<[f64; 3]>,
    pub round: bool,
    pub cfm: Option<f64>,
}

/// A run joined to a piece of equipment.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Connection {
    /// `duct` or `pipe`.
    pub kind: String,
    /// The system code the run reader gave it (`SA`, `HWR`…; empty when it didn't know).
    pub system: String,
    /// Where the run meets the item, building feet.
    pub at_ft: [f64; 2],
    /// `end`: the run ends at the item; `over`: it crosses the item's footprint (a drop into a
    /// tank or a unit below).
    pub via: String,
    /// The run's centreline above the floor, feet, when its reader worked out its height.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub centre_ft: Option<f64>,
}

/// A piece of equipment placed in the model.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Placed {
    pub tag: String,
    pub kind: String,
    /// Level code (`1`, `2`, `R`).
    pub level: String,
    /// The plan it was found on.
    pub sheet: String,
    /// The box, building feet (z: elevation).
    pub min_ft: [f64; 3],
    pub max_ft: [f64; 3],
    pub round: bool,
    /// Plan position and footprint: `outline` (drawn on the plan), `tag` (a box of the
    /// schedule's size at the tag; which way it faces is a guess) or `rule` (no size printed).
    pub footprint_from: String,
    /// `schedule` or `rule`.
    pub height_from: String,
    /// Where it hangs or stands: `floor` (on its level), `run` (centred on the run it is in) or
    /// `rule` (a hung item whose run has no known height).
    pub elevation_from: String,
    /// 0–1: high when the outline matches the printed size and the height is printed.
    pub confidence: f64,
    /// What the reader guessed, in words.
    pub flags: Vec<String>,
    pub connections: Vec<Connection>,
    pub title: String,
    pub properties: BTreeMap<String, String>,
}

impl Placed {
    /// The systems joined to it, sorted.
    pub fn systems(&self) -> Vec<String> {
        self.connections.iter().map(|c| c.system.clone()).filter(|s| !s.is_empty()).collect::<BTreeSet<_>>().into_iter().collect()
    }

    /// The building spec's form (`mep.equipment[]` of the walk3d building spec), for a model
    /// drawn in building inches (12 units per foot) with the levels named as their codes.
    pub fn spec_json(&self, level_elevation_ft: f64) -> serde_json::Value {
        serde_json::json!({
            "tag": self.tag,
            "kind": self.kind,
            "level": self.level,
            "min": [self.min_ft[0] * 12.0, self.min_ft[1] * 12.0],
            "max": [self.max_ft[0] * 12.0, self.max_ft[1] * 12.0],
            "bottom_ft": self.min_ft[2] - level_elevation_ft,
            "height_ft": self.max_ft[2] - self.min_ft[2],
            "round": self.round,
            "systems": self.systems(),
            "title": self.title,
            "properties": self.properties,
            "sources": {"footprint": self.footprint_from, "height": self.height_from, "elevation": self.elevation_from},
            "confidence": self.confidence,
        })
    }
}

/// Everything the reader found.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct EquipmentRead {
    pub tables: Vec<Table>,
    pub scheduled: Vec<Scheduled>,
    pub placed: Vec<Placed>,
    /// Scheduled tags not found on any plan, with why.
    pub unplaced: Vec<(String, String)>,
    /// What the reader couldn't do, in words (no schedule found, and why).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// The run graphs the connections were traced on, per level.
    #[serde(skip)]
    pub runs: BTreeMap<String, RunGraph>,
}

/// Printed system codes and the words a building spec's system names use for them (layer-name
/// parts: `M-DUCT-SUPP`, `P-DOMW-HW`, `Supply Air`), and words that rule a name out.
const SYSTEM_WORDS: &[(&str, &str, &[&str], &[&str])] = &[
    ("SA", "duct", &["SA", "SUPP", "SUPPLY", "SAS"], &[]),
    ("RA", "duct", &["RA", "RETN", "RETURN", "RTN", "RET"], &[]),
    ("EA", "duct", &["EA", "EXHS", "EXHAUST", "EXH"], &[]),
    ("OA", "duct", &["OA", "OUTSIDE", "OAIR", "FRESH"], &[]),
    ("CW", "pipe", &["CW", "DCW", "COLD"], &["CHW", "CHWS", "CHWR"]),
    ("HW", "pipe", &["HW", "DHW", "HWS", "HOT"], &["HWR", "HWC", "DHWR", "RETURN", "RECIRC", "RET", "CIRC"]),
    ("HWR", "pipe", &["HWR", "HWC", "DHWR", "HWRC", "RECIRC", "CIRC"], &[]),
    ("SAN", "pipe", &["SAN", "SANR", "SANI", "SS", "WAST", "WASTE", "SW"], &[]),
    ("V", "pipe", &["V", "VENT", "VNT"], &[]),
    ("G", "pipe", &["G", "GAS", "NG", "NGAS"], &[]),
    ("FP", "pipe", &["FP", "FIRE", "SPRK", "SPRINKLER", "SP"], &[]),
    ("CHW", "pipe", &["CHW", "CHWS"], &["CHWR", "RETURN"]),
    ("CHWR", "pipe", &["CHWR"], &[]),
    ("ST", "pipe", &["ST", "STRM", "STORM"], &[]),
];

/// The building spec's name for a system code the reader gave a `duct` or `pipe` run: the
/// spec's system of that name, else the one whose name has the code's words (`SA` →
/// `M-DUCT-SUPP`, `HW` → `P-DOMW-HW`). `systems` are the spec's (name, kind) pairs; `None`
/// when none of them is that system.
pub fn spec_system_name(code: &str, kind: &str, systems: &[(&str, &str)]) -> Option<String> {
    if let Some((n, _)) = systems.iter().find(|(n, _)| n.trim().eq_ignore_ascii_case(code.trim())) {
        return Some(n.to_string());
    }
    let (_, want, words, not) = SYSTEM_WORDS.iter().find(|(c, k, _, _)| c.eq_ignore_ascii_case(code.trim()) && *k == kind)?;
    systems
        .iter()
        .filter(|(_, k)| k.eq_ignore_ascii_case(want))
        .find(|(n, _)| {
            let up = n.to_ascii_uppercase();
            let parts: Vec<&str> = up.split(['-', '_', ' ', '.', '/']).filter(|w| !w.is_empty()).collect();
            parts.iter().any(|w| words.contains(w)) && !parts.iter().any(|w| not.contains(w))
        })
        .map(|(n, _)| n.to_string())
}

/// The placed equipment in the building spec's form (`mep.equipment[]` for `bspec.set`, drawing
/// inches = building feet × 12), with level codes turned into the spec's level names
/// (`level_names`: code → name) and the systems it joins into the spec's system names
/// (`systems`: the spec's (name, kind) pairs, see [`spec_system_name`]). Items on a level the
/// spec doesn't name are left out, and a system the spec doesn't have keeps its code; both are
/// said in the notes it gives back.
pub fn spec_equipment(
    read: &EquipmentRead,
    levels: &[Level],
    level_names: &[(&str, &str)],
    systems: &[(&str, &str)],
) -> (Vec<serde_json::Value>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    for p in read.placed.iter().take(MAX_ITEMS) {
        let Some((_, name)) = level_names.iter().find(|(c, _)| c.eq_ignore_ascii_case(&p.level)) else {
            notes.push(format!("{} is on level {}, which the building spec doesn't name: left out", p.tag, p.level));
            continue;
        };
        let mut names: Vec<String> = Vec::new();
        for c in p.connections.iter().filter(|c| !c.system.is_empty()) {
            let n = spec_system_name(&c.system, &c.kind, systems).unwrap_or_else(|| {
                let note = format!("{} joins {} {}, which isn't a system of the building spec: kept as {}", p.tag, c.kind, c.system, c.system);
                if !notes.contains(&note) {
                    notes.push(note);
                }
                c.system.clone()
            });
            if !names.contains(&n) {
                names.push(n);
            }
        }
        names.sort();
        let mut v = p.spec_json(elevation(levels, &p.level));
        v["level"] = serde_json::json!(name);
        v["systems"] = serde_json::json!(names);
        out.push(v);
    }
    (out, notes)
}

// ---- Text helpers ------------------------------------------------------------------------------

struct Txt {
    value: String,
    at: Vec2,
    h: f64,
    /// Radians.
    rot: f64,
    layer: String,
}

fn texts(d: &Drawing) -> Vec<Txt> {
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_ITEMS) {
        if let EntityKind::Text(t) = &e.kind {
            let at = t.align_pt.unwrap_or(t.insert).xy();
            let value: String = t.value.trim().chars().take(MAX_CELL).collect();
            if at.is_finite() && t.height.is_finite() && t.height > 0.0 && !value.is_empty() {
                out.push(Txt {
                    value,
                    at,
                    h: t.height,
                    rot: if t.rotation.is_finite() { t.rotation } else { 0.0 },
                    layer: e.common.layer.to_string(),
                });
            }
        }
    }
    out
}

/// Axis-aligned closed 4-point polylines: (min, max, layer).
fn boxes(d: &Drawing) -> Vec<(Vec2, Vec2, String)> {
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_ITEMS) {
        let EntityKind::LwPolyline(p) = &e.kind else { continue };
        if !p.closed || p.vertices.len() != 4 {
            continue;
        }
        let pts: Vec<Vec2> = p.vertices.iter().map(|v| v.p).collect();
        if !pts.iter().all(|q| q.is_finite()) {
            continue;
        }
        let (lo, hi) = bounds(&pts);
        // Every corner on the bounding box's corners: axis-aligned.
        let on = |q: &Vec2| ((q.x - lo.x).abs() < 1e-6 || (q.x - hi.x).abs() < 1e-6) && ((q.y - lo.y).abs() < 1e-6 || (q.y - hi.y).abs() < 1e-6);
        if pts.iter().all(on) && hi.x - lo.x > 1e-9 && hi.y - lo.y > 1e-9 {
            out.push((lo, hi, e.common.layer.to_string()));
        }
    }
    out
}

fn bounds(pts: &[Vec2]) -> (Vec2, Vec2) {
    let lo = Vec2::new(pts.iter().map(|p| p.x).fold(f64::INFINITY, f64::min), pts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min));
    let hi = Vec2::new(pts.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max), pts.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max));
    (lo, hi)
}

fn inside(p: Vec2, lo: Vec2, hi: Vec2) -> bool {
    p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y
}

/// Distance from `p` to a box (0 inside).
fn box_dist(p: Vec2, lo: Vec2, hi: Vec2) -> f64 {
    let dx = (lo.x - p.x).max(0.0).max(p.x - hi.x);
    let dy = (lo.y - p.y).max(0.0).max(p.y - hi.y);
    (dx * dx + dy * dy).sqrt()
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

/// A layer whose name says it holds equipment: `EQUIP`, `M-EQPM`, `P-EQPT`, `M-HVAC-EQUIP`.
/// Names are only a hint: shapes on other layers are read too, more carefully.
fn equipment_layer(layer: &str) -> bool {
    let up = layer.to_ascii_uppercase();
    up.split(['-', '_', ' ', '|', '$']).any(|w| w.starts_with("EQUIP") || w == "EQPM" || w == "EQPT" || w == "EQP")
}

/// A layer whose lines may be leaders from tags: text and annotation layers.
fn annotation_layer(layer: &str) -> bool {
    let up = layer.to_ascii_uppercase();
    up.split(['-', '_', ' ', '|', '$']).any(|w| ["TEXT", "ANNO", "NOTE", "NOTES", "LEADER", "LDR", "TAG", "TAGS", "IDEN", "SYMB"].contains(&w))
}

// ---- Schedules -----------------------------------------------------------------------------------

/// Read every table with a tag column on a sheet.
pub fn read_tables(d: &Drawing, sheet: &str) -> Vec<Table> {
    read_tables_keyed(d, sheet, is_tag_column)
}

/// Most work (line pairs looked at) spent finding the cells of line-drawn tables on one sheet.
const MAX_GRID_WORK: usize = 5_000_000;

/// Horizontal (`y, x0, x1`) and vertical (`x, y0, y1`) straight lines of a drawing, any layer:
/// lines and the straight edges of polylines, those along a side merged into one (a rule drawn
/// in pieces, cell by cell, is one line).
fn rules(d: &Drawing, eps: f64) -> (Vec<[f64; 3]>, Vec<[f64; 3]>) {
    let (mut h, mut v) = (Vec::new(), Vec::new());
    let mut add = |a: Vec2, b: Vec2| {
        if !(a.is_finite() && b.is_finite()) || h.len() + v.len() >= MAX_ITEMS {
            return;
        }
        let (dx, dy) = ((b.x - a.x).abs(), (b.y - a.y).abs());
        let tol = 1e-6 * dx.max(dy);
        if dx > eps && dy <= tol {
            h.push([(a.y + b.y) / 2.0, a.x.min(b.x), a.x.max(b.x)]);
        } else if dy > eps && dx <= tol {
            v.push([(a.x + b.x) / 2.0, a.y.min(b.y), a.y.max(b.y)]);
        }
    };
    for e in d.model.iter().take(MAX_ITEMS) {
        match &e.kind {
            EntityKind::Line(l) => add(l.a.xy(), l.b.xy()),
            EntityKind::LwPolyline(p) => {
                let n = p.vertices.len();
                for k in 0..if p.closed { n } else { n.saturating_sub(1) } {
                    if let (Some(x), Some(y)) = (p.vertices.get(k), p.vertices.get((k + 1) % n.max(1)))
                        && x.bulge == 0.0
                    {
                        add(x.p, y.p);
                    }
                }
            }
            _ => {}
        }
    }
    let merge = |mut ls: Vec<[f64; 3]>| -> Vec<[f64; 3]> {
        ls.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
        let mut out: Vec<[f64; 3]> = Vec::with_capacity(ls.len());
        for l in ls {
            match out.last_mut() {
                Some(o) if near(o[0], l[0], eps * 0.1) && l[1] <= o[2] + eps * 0.1 => o[2] = o[2].max(l[2]),
                _ => out.push(l),
            }
        }
        out
    };
    (merge(h), merge(v))
}

/// The cells of tables drawn as grid lines: between two neighbouring vertical rules that both
/// cross just below a horizontal one, down to the next horizontal rule that spans them and that
/// both reach. Cells already drawn as boxes come out once (`have` holds them).
fn grid_cells(d: &Drawing, eps: f64, have: &[(Vec2, Vec2, String)]) -> Vec<(Vec2, Vec2, String)> {
    let (h, v) = rules(d, eps);
    if h.len() < 2 || v.len() < 2 {
        return Vec::new();
    }
    // Horizontal rules from the top down.
    let mut down: Vec<&[f64; 3]> = h.iter().collect();
    down.sort_by(|a, b| b[0].total_cmp(&a[0]));
    // Vertical rules by x (already sorted by `rules`).
    let key = |lo: Vec2, hi: Vec2| [lo.x, lo.y, hi.x, hi.y].map(|c| (c / (eps * 0.1).max(1e-9)).round() as i64);
    let mut seen: BTreeSet<[i64; 4]> = have.iter().map(|(lo, hi, _)| key(*lo, *hi)).collect();
    let mut out = Vec::new();
    let mut work = 0usize;
    for (ti, top) in down.iter().enumerate() {
        let y = top[0];
        let from = v.partition_point(|l| l[0] < top[1] - eps);
        let mut xs: Vec<&[f64; 3]> = Vec::new();
        for l in v.iter().skip(from) {
            work += 1;
            if l[0] > top[2] + eps || work > MAX_GRID_WORK {
                break;
            }
            if l[1] < y - eps && l[2] >= y - eps && xs.last().is_none_or(|p| !near(p[0], l[0], eps * 0.1)) {
                xs.push(l);
            }
        }
        for pair in xs.windows(2) {
            let [a, b] = pair else { continue };
            // The first rule below spanning a..b that both sides reach.
            let bottom = down.iter().skip(ti + 1).find(|r| {
                work += 1;
                r[0] < y - eps && r[1] <= a[0] + eps && r[2] >= b[0] - eps
            });
            if work > MAX_GRID_WORK || out.len() >= MAX_ITEMS {
                return out;
            }
            let Some(bot) = bottom else { continue };
            if a[1] > bot[0] + eps || b[1] > bot[0] + eps {
                continue;
            }
            let (lo, hi) = (Vec2::new(a[0], bot[0]), Vec2::new(b[0], y));
            if seen.insert(key(lo, hi)) {
                out.push((lo, hi, String::new()));
            }
        }
    }
    out
}

/// Texts sorted by x, for finding the one in a cell without looking at them all.
struct TextIndex<'a> {
    by_x: Vec<(usize, &'a Txt)>,
}

impl<'a> TextIndex<'a> {
    fn new(txt: &'a [Txt]) -> Self {
        let mut by_x: Vec<(usize, &Txt)> = txt.iter().enumerate().collect();
        by_x.sort_by(|a, b| a.1.at.x.total_cmp(&b.1.at.x));
        TextIndex { by_x }
    }

    /// The text in a cell: of those whose point is inside it, the first drawn.
    fn in_cell(&self, lo: Vec2, hi: Vec2) -> Option<&'a Txt> {
        let from = self.by_x.partition_point(|(_, t)| t.at.x < lo.x);
        self.by_x.iter().skip(from).take_while(|(_, t)| t.at.x <= hi.x).filter(|(_, t)| inside(t.at, lo, hi)).min_by_key(|(i, _)| *i).map(|(_, t)| *t)
    }
}

/// The cells of the tables a drawing draws, `[x0, y0, x1, y1]`: closed boxes and the spaces
/// between grid lines (as [`read_tables`] finds them), on any layer, keyed or not. Empty for a
/// drawing of no lettering.
pub fn table_cells(d: &Drawing) -> Vec<[f64; 4]> {
    let txt = texts(d);
    let h = txt.iter().map(|t| t.h).fold(f64::INFINITY, f64::min);
    if !h.is_finite() {
        return Vec::new();
    }
    let mut all = boxes(d);
    let grid = grid_cells(d, h * 0.3, &all);
    all.extend(grid);
    all.into_iter().map(|(lo, hi, _)| [lo.x, lo.y, hi.x, hi.y]).collect()
}

/// As [`read_tables`], with tables keyed by the columns `is_key` accepts (a fixture schedule's
/// `TYPE`, say).
///
/// A table is a header row of cells, one holding a key column's name, with rows of cells under
/// it. Cells are closed boxes or the spaces between grid lines, on any layer (drafters put
/// schedules on `BORDER`, `M-ANNO-SCHD`, `G-ANNO-TABL`…); each table is found from its key
/// cell, so a sheet's other boxes and lines cost little.
pub fn read_tables_keyed(d: &Drawing, sheet: &str, is_key: fn(&str) -> bool) -> Vec<Table> {
    let txt = texts(d);
    let h = txt.iter().map(|t| t.h).fold(f64::INFINITY, f64::min);
    if !h.is_finite() {
        return Vec::new();
    }
    let eps = h * 0.3;
    let mut all = boxes(d);
    let grid = grid_cells(d, eps, &all);
    all.extend(grid);
    let index = TextIndex::new(&txt);
    // Cells sorted by their left and right sides.
    let mut by_lo: Vec<usize> = (0..all.len()).collect();
    by_lo.sort_by(|a, b| all.get(*a).map(|c| c.0.x).unwrap_or(0.0).total_cmp(&all.get(*b).map(|c| c.0.x).unwrap_or(0.0)));
    let mut by_hi: Vec<usize> = (0..all.len()).collect();
    by_hi.sort_by(|a, b| all.get(*a).map(|c| c.1.x).unwrap_or(0.0).total_cmp(&all.get(*b).map(|c| c.1.x).unwrap_or(0.0)));
    // The cells whose left (right) side is at x.
    let left_at = |x: f64| {
        let from = by_lo.partition_point(|i| all.get(*i).is_some_and(|c| c.0.x < x - eps));
        by_lo.iter().skip(from).filter_map(|i| all.get(*i)).take_while(move |c| c.0.x <= x + eps)
    };
    let right_at = |x: f64| {
        let from = by_hi.partition_point(|i| all.get(*i).is_some_and(|c| c.1.x < x - eps));
        by_hi.iter().skip(from).filter_map(|i| all.get(*i)).take_while(move |c| c.1.x <= x + eps)
    };
    let same_row = |a: &(Vec2, Vec2, String), lo: Vec2, hi: Vec2| near(a.0.y, lo.y, eps) && near(a.1.y, hi.y, eps);
    // A header cell: a cell in the row holding a text.
    let header = |c: &(Vec2, Vec2, String)| index.in_cell(c.0, c.1).map(|t| (c.0, c.1, t.value.clone()));
    // Key cells: the smallest cell around each key column's name, top to bottom.
    let mut keys: Vec<&(Vec2, Vec2, String)> = txt
        .iter()
        .filter(|t| is_key(&t.value))
        .filter_map(|t| {
            all.iter()
                .filter(|(lo, hi, _)| inside(t.at, *lo, *hi) && hi.y - lo.y > eps)
                .min_by(|a, b| ((a.1.x - a.0.x) * (a.1.y - a.0.y)).total_cmp(&((b.1.x - b.0.x) * (b.1.y - b.0.y))))
        })
        .collect();
    keys.sort_by(|a, b| b.0.y.total_cmp(&a.0.y).then(a.0.x.total_cmp(&b.0.x)));
    let mut starts: Vec<(f64, f64, f64)> = Vec::new();
    let mut out = Vec::new();
    for key in keys {
        if out.len() >= MAX_TABLES {
            break;
        }
        let Some(seed) = header(key) else { continue };
        // The row: cells joined edge to edge, walked left to its first, then right.
        let mut first = seed.clone();
        for _ in 0..MAX_COLS {
            let Some(c) = right_at(first.0.x).filter(|c| same_row(c, first.0, first.1)).find_map(header) else { break };
            first = c;
        }
        if starts.iter().any(|s| near(s.0, first.0.x, eps) && near(s.1, first.0.y, eps) && near(s.2, first.1.y, eps)) {
            continue;
        }
        starts.push((first.0.x, first.0.y, first.1.y));
        let mut cols: Vec<(Vec2, Vec2, String)> = vec![first.clone()];
        while cols.len() < MAX_COLS {
            let Some(last) = cols.last() else { break };
            let Some(c) = left_at(last.1.x).filter(|c| same_row(c, last.0, last.1)).find_map(header) else { break };
            cols.push(c);
        }
        let names: Vec<String> = cols.iter().map(|c| c.2.clone()).collect();
        if !names.iter().any(|n| is_key(n)) {
            continue;
        }
        // The title: the lowest text just above the header row.
        let row_h = first.1.y - first.0.y;
        let right = cols.last().map_or(first.1.x, |c| c.1.x);
        let title = txt
            .iter()
            .filter(|t| t.at.y > first.1.y - eps && t.at.y < first.1.y + row_h * 2.5 && t.at.x >= first.0.x - eps && t.at.x < right)
            .filter(|t| !cols.iter().any(|c| inside(t.at, c.0, c.1)))
            .min_by(|a, b| a.at.y.total_cmp(&b.at.y))
            .map(|t| t.value.clone())
            .unwrap_or_default();
        // Rows: cells under each column, stacked edge to edge.
        let mut rows = Vec::new();
        let mut top = first.0.y;
        while rows.len() < MAX_ROWS {
            let below =
                |c: &(Vec2, Vec2, String)| left_at(c.0.x).find(|(lo, hi, _)| near(hi.x, c.1.x, eps) && near(hi.y, top, eps) && hi.y - lo.y > eps);
            let Some(first_cell) = cols.first().and_then(below) else { break };
            let mut row = BTreeMap::new();
            for c in &cols {
                if let Some((lo, hi, _)) = below(c)
                    && let Some(t) = index.in_cell(*lo, *hi)
                {
                    row.insert(c.2.clone(), t.value.clone());
                }
            }
            top = first_cell.0.y;
            if !row.is_empty() {
                rows.push(row);
            }
        }
        out.push(Table { sheet: sheet.to_string(), title, columns: names, rows });
    }
    out
}

fn is_tag_column(n: &str) -> bool {
    matches!(n.trim().to_ascii_uppercase().as_str(), "TAG" | "MARK" | "UNIT TAG" | "EQUIPMENT TAG" | "SYMBOL")
}

/// What a schedule's rows are, from its title; `Some("")` for tables that aren't equipment.
pub fn kind_from_title(title: &str) -> Option<&'static str> {
    let t = title.to_ascii_uppercase();
    let has = |w: &str| t.contains(w);
    if has("AIR DEVICE") || has("DIFFUSER") || has("GRILLE") || has("FIXTURE") || has("SHEET INDEX") || has("LIGHT") || has("DOOR") || has("WINDOW") {
        return Some("");
    }
    // Electrical gear, its lugs and feeders: the gear reader's (`crate::gear_reader`).
    if has("ELECTRICAL") || has("LUG") || has("FEEDER") || has("PANEL") || has("SWITCHBOARD") {
        return Some("");
    }
    Some(if has("ROOFTOP") || has("RTU") || has("PACKAGED") {
        "rtu"
    } else if has("AIR HANDL") || has("AHU") {
        "ahu"
    } else if has("VAV") || has("TERMINAL UNIT") || has("VARIABLE AIR") || has("FAN POWERED") {
        "vav"
    } else if has("CONDENSING") || has("CONDENSER") {
        "condensing_unit"
    } else if has("EXHAUST FAN") {
        "exhaust_fan"
    } else if has("FAN") {
        "fan"
    } else if has("PUMP") {
        "pump"
    } else if has("WATER HEATER") {
        "water_heater"
    } else if has("GAS METER") {
        "gas_meter"
    } else {
        return None;
    })
}

/// What a tag names, from its prefix (`AHU-1` → `ahu`).
pub fn kind_from_tag(tag: &str) -> Option<&'static str> {
    let prefix: String = tag.trim().to_ascii_uppercase().chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    Some(match prefix.as_str() {
        "AHU" => "ahu",
        "RTU" => "rtu",
        "VAV" | "FPB" | "VVT" | "FPVAV" => "vav",
        "EF" => "exhaust_fan",
        "SF" | "RF" | "F" | "TF" | "IF" | "CF" => "fan",
        "P" | "CP" | "HWP" | "CHWP" | "CWP" | "RP" => "pump",
        "WH" | "EWH" | "GWH" | "DWH" => "water_heater",
        "CU" | "ACCU" => "condensing_unit",
        "GM" => "gas_meter",
        _ => return None,
    })
}

/// Feet from a printed length: `12'-0"`, `8'-7 1/2"`, `2'`, `30"`, or a bare number in `unit`
/// (inches when the column says `(IN)`, else feet).
pub fn parse_length_ft(s: &str, inches_default: bool) -> Option<f64> {
    let s = s.trim().replace(['\u{2019}', '\u{2032}'], "'").replace(['\u{201d}', '\u{2033}'], "\"");
    if s.starts_with('-') {
        return None;
    }
    let num = |t: &str| -> Option<f64> {
        let t = t.trim().trim_end_matches('"').trim();
        if t.is_empty() {
            return Some(0.0);
        }
        // `7 1/2`, `1/2`, `7-1/2`, `7.5`.
        let mut total = 0.0;
        for part in t.split([' ', '-']).filter(|p| !p.is_empty()) {
            total += match part.split_once('/') {
                Some((a, b)) => {
                    let (a, b) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
                    if b == 0.0 {
                        return None;
                    }
                    a / b
                }
                None => part.parse::<f64>().ok()?,
            };
        }
        Some(total)
    };
    let v = if let Some((f, i)) = s.split_once('\'') {
        num(f)? + num(i.trim_start_matches('-'))? / 12.0
    } else if s.ends_with('"') {
        num(&s)? / 12.0
    } else {
        let v = num(&s)?;
        if inches_default { v / 12.0 } else { v }
    };
    (v.is_finite() && v > 0.0 && v < 1_000.0).then_some(v)
}

/// A printed size from its column name and cell: `SIZE (L x W x H)` = `12'-0" x 6'-0" x 5'-0"`,
/// `SIZE (DIA x H)` = `2'-0" x 5'-0"`, `DIMENSIONS (IN)` = `36 x 30 x 33`.
pub fn parse_size(column: &str, cell: &str) -> Option<([f64; 3], bool)> {
    let col = column.to_ascii_uppercase();
    let inches = col.contains("(IN)") || col.contains("INCH");
    let labels: Vec<String> =
        col.split_once('(').map(|(_, r)| r.trim_end_matches(')').split(['x', 'X', '×']).map(|p| p.trim().to_string()).collect()).unwrap_or_default();
    let parts: Vec<f64> = cell.split(['x', 'X', '×']).map(|p| parse_length_ft(p, inches)).collect::<Option<Vec<_>>>()?;
    let round = labels.first().is_some_and(|l| l.starts_with("DIA") || l == "D" || l == "Ø");
    match (round, parts.as_slice()) {
        (true, [d, h]) => Some(([*d, *d, *h], true)),
        (true, [d]) => Some(([*d, *d, 0.0], true)),
        (false, [l, w, h]) => Some(([*l, *w, *h], false)),
        (false, [l, w]) => Some(([*l, *w, 0.0], false)),
        _ => None,
    }
}

/// The scheduled equipment in the tables (one per tag; the first table naming a tag wins).
pub fn scheduled(tables: &[Table]) -> Vec<Scheduled> {
    let mut out: Vec<Scheduled> = Vec::new();
    for t in tables {
        let by_title = kind_from_title(&t.title);
        if by_title == Some("") {
            continue;
        }
        let Some(tag_col) = t.columns.iter().find(|c| is_tag_column(c)) else { continue };
        for row in &t.rows {
            let Some(tag) = row.get(tag_col).map(|s| s.trim().to_ascii_uppercase()).filter(|s| !s.is_empty()) else { continue };
            if out.iter().any(|s| s.tag == tag) {
                continue;
            }
            let (kind, from) = match (by_title, kind_from_tag(&tag)) {
                // A fan schedule's exhaust fans.
                (Some("fan"), Some("exhaust_fan")) => ("exhaust_fan", "title"),
                (Some(k), _) => (k, "title"),
                (None, Some(k)) => (k, "tag"),
                (None, None) => continue,
            };
            let col = |c: &str| row.iter().find(|(k, _)| k.trim().eq_ignore_ascii_case(c)).map(|(_, v)| v);
            let size = row
                .iter()
                .filter(|(c, _)| {
                    let c = c.to_ascii_uppercase();
                    c.contains("SIZE") || c.contains("DIMENSION")
                })
                .find_map(|(c, v)| parse_size(c, v));
            let cfm = ["CFM", "MAX CFM", "SUPPLY CFM", "AIRFLOW (CFM)", "CFM (MAX)"]
                .iter()
                .find_map(|c| col(c))
                .and_then(|v| v.replace(',', "").trim().parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0);
            out.push(Scheduled {
                tag,
                kind: kind.to_string(),
                kind_from: from.to_string(),
                sheet: t.sheet.clone(),
                title: t.title.clone(),
                properties: row.clone(),
                size_ft: size.map(|s| s.0),
                round: size.is_some_and(|s| s.1),
                cfm,
            });
        }
    }
    out
}

// ---- Rules -----------------------------------------------------------------------------------------

/// A kind's usual footprint (feet) and height when the schedule prints no size.
fn rule_size(kind: &str) -> [f64; 3] {
    match kind {
        "ahu" => [8.0, 5.0, 5.0],
        "rtu" => [10.0, 6.0, 4.5],
        "vav" => [2.5, 1.5, 1.25],
        "exhaust_fan" | "fan" => [2.0, 2.0, 2.5],
        "pump" => [1.0, 0.5, 0.5],
        "water_heater" => [2.0, 2.0, 5.0],
        "condensing_unit" => [3.0, 3.0, 3.0],
        "gas_meter" => [1.5, 1.0, 3.0],
        _ => [2.0, 2.0, 2.0],
    }
}

/// Whether a kind hangs in its run (centred on it) rather than standing on the floor.
fn hangs(s: &Scheduled) -> bool {
    let ty = s.properties.iter().find(|(k, _)| k.trim().eq_ignore_ascii_case("TYPE")).map(|(_, t)| t.to_ascii_uppercase()).unwrap_or_default();
    match s.kind.as_str() {
        "vav" => true,
        "pump" | "fan" => ty.contains("IN-LINE") || ty.contains("INLINE") || ty.contains("CIRCULATOR"),
        _ => false,
    }
}

/// Centre height above the floor for a hung item whose run's height isn't known, feet.
const HUNG_CENTRE_FT: f64 = 10.0;

// ---- Placing -------------------------------------------------------------------------------------

/// The sheets of a set as the reader gets them: each with its (aligned) map and drawing.
pub struct SheetIn<'a> {
    pub sheet: &'a Sheet,
    pub drawing: &'a Drawing,
}

/// Read the schedules on every sheet, place the scheduled equipment on the plans and join it to
/// the runs. `levels` gives each level's elevation (the cover sheet's level list).
pub fn read_equipment(sheets: &[SheetIn], levels: &[Level]) -> EquipmentRead {
    let mut tables = Vec::new();
    for s in sheets {
        if s.sheet.level.is_none() {
            tables.extend(read_tables(s.drawing, &s.sheet.number));
        }
    }
    let sched = scheduled(&tables);
    let mut notes = Vec::new();
    let schedule_sheets = sheets.iter().filter(|s| s.sheet.level.is_none()).count();
    if tables.is_empty() {
        notes.push(if schedule_sheets == 0 {
            "no schedule sheets: every sheet given is a plan of a level".to_string()
        } else {
            format!(
                "no schedule found on {schedule_sheets} schedule sheet(s): no table has a header row with a TAG or MARK column (cells drawn as boxes or grid lines)"
            )
        });
    } else if sched.is_empty() {
        notes.push(format!("{} table(s) read, none of them an equipment schedule (by title or tag prefix)", tables.len()));
    }

    // The runs per level (full plans only; enlarged plans repeat them).
    let mut graphs: BTreeMap<String, RunGraph> = BTreeMap::new();
    // The duct lines as drawn, for runs the duct reader missed.
    let mut drawn: BTreeMap<String, RunGraph> = BTreeMap::new();
    for s in sheets.iter().filter(|s| s.sheet.window_ft.is_none()) {
        let Some(lv) = &s.sheet.level else { continue };
        let g = run_graph(s.drawing, s.sheet);
        if s.sheet.discipline == "M" {
            let raw = drawn.entry(lv.clone()).or_default();
            raw.pieces.extend(drawn_ducts(s.drawing, s.sheet));
            raw.marks.extend(g.marks.iter().filter(|m| m.kind == "duct").cloned());
            raw.heights.extend(g.heights.iter().cloned());
        }
        let all = graphs.entry(lv.clone()).or_default();
        all.pieces.extend(g.pieces);
        all.marks.extend(g.marks);
        all.heights.extend(g.heights);
    }
    for g in graphs.values_mut().chain(drawn.values_mut()) {
        g.own();
    }
    // Tags on the plans: full plans first, then enlarged ones.
    // Not the architectural roof plans (Q307/Q337): their tags label the curbs, under them, not
    // where the equipment is.
    let arch_roof = |s: &SheetIn| s.sheet.discipline == "A" && s.sheet.title.to_ascii_uppercase().contains("ROOF");
    let mut plans: Vec<&SheetIn> = sheets.iter().filter(|s| s.sheet.level.is_some() && !arch_roof(s)).collect();
    plans.sort_by_key(|s| s.sheet.window_ft.is_some());
    let mut placed: Vec<Placed> = Vec::new();
    for s in &plans {
        let todo: Vec<&Scheduled> = sched.iter().filter(|x| !placed.iter().any(|p| p.tag == x.tag)).collect();
        if todo.is_empty() {
            break;
        }
        placed.extend(place_on_sheet(s, &todo, levels));
    }
    for p in &mut placed {
        let hung = sched.iter().find(|x| x.tag == p.tag).is_some_and(hangs);
        if let Some(g) = graphs.get(&p.level) {
            join_runs(p, g, hung);
        }
        let ducts = !matches!(p.kind.as_str(), "pump" | "water_heater" | "gas_meter" | "condensing_unit");
        if ducts
            && !p.connections.iter().any(|c| c.kind == "duct")
            && let Some(g) = drawn.get(&p.level)
        {
            let pipes = std::mem::take(&mut p.connections);
            join_runs(p, g, hung);
            if !p.connections.is_empty() {
                p.flags.push("its ducts were traced from the drawn lines (the duct reader didn't find them)".into());
                p.confidence = (p.confidence - 0.1).max(0.05);
            }
            p.connections.extend(pipes);
        }
        if hung {
            set_hung_height(p, levels);
        }
    }
    let unplaced = sched
        .iter()
        .filter(|x| !placed.iter().any(|p| p.tag == x.tag))
        .map(|x| (x.tag.clone(), "its tag isn't on any plan of the set (a roof plan or a detail?)".to_string()))
        .collect();
    EquipmentRead { tables, scheduled: sched, placed, unplaced, notes, runs: graphs }
}

/// Printed pipe system letters → our codes.
const PIPE_ABBR: &[(&str, &str)] = &[
    ("CW", "CW"),
    ("DCW", "CW"),
    ("HW", "HW"),
    ("DHW", "HW"),
    ("HWR", "HWR"),
    ("HWC", "HWR"),
    ("HRW", "HWR"),
    ("DHWR", "HWR"),
    ("SAN", "SAN"),
    ("SS", "SAN"),
    ("W", "SAN"),
    ("V", "V"),
    ("G", "G"),
    ("GAS", "G"),
    ("NG", "G"),
    ("FP", "FP"),
    ("SP", "FP"),
    ("F", "FP"),
    ("CHW", "CHW"),
    ("CHWR", "CHWR"),
    ("ST", "ST"),
];

fn elevation(levels: &[Level], code: &str) -> f64 {
    levels.iter().find(|l| l.code == code).map_or(0.0, |l| l.elevation_ft)
}

/// An outline on the plan: a box or a circle (model units).
#[derive(Clone, Copy)]
enum Outline {
    Box(Vec2, Vec2),
    Circle(Vec2, f64),
}

impl Outline {
    fn dist(&self, p: Vec2) -> f64 {
        match self {
            Outline::Box(lo, hi) => box_dist(p, *lo, *hi),
            Outline::Circle(c, r) => (p.dist(*c) - r).max(0.0),
        }
    }
    fn corners(&self) -> [[f64; 2]; 2] {
        match self {
            Outline::Box(lo, hi) => [[lo.x, lo.y], [hi.x, hi.y]],
            Outline::Circle(c, r) => [[c.x - r, c.y - r], [c.x + r, c.y + r]],
        }
    }
}

fn place_on_sheet(s: &SheetIn, todo: &[&Scheduled], levels: &[Level]) -> Vec<Placed> {
    let d = s.drawing;
    let sh = s.sheet;
    let level = sh.level.clone().unwrap_or_default();
    let txt = texts(d);
    // Outlines on an equipment layer (`EQUIP`, `M-EQPM`, `P-EQPT`…) first; closed shapes on other
    // layers are only taken when they are the printed size (see below).
    let (mut outlines, mut others): (Vec<Outline>, Vec<(Outline, String)>) = (Vec::new(), Vec::new());
    for (lo, hi, l) in boxes(d) {
        if equipment_layer(&l) {
            outlines.push(Outline::Box(lo, hi));
        } else {
            others.push((Outline::Box(lo, hi), l));
        }
    }
    let mut leaders: Vec<(Vec2, Vec2)> = Vec::new();
    for e in d.model.iter().take(MAX_ITEMS) {
        let layer = e.common.layer.as_str();
        match &e.kind {
            EntityKind::Circle(c) if c.center.xy().is_finite() && c.radius.is_finite() && c.radius > 0.0 => {
                if equipment_layer(layer) {
                    outlines.push(Outline::Circle(c.center.xy(), c.radius));
                } else {
                    others.push((Outline::Circle(c.center.xy(), c.radius), layer.to_string()));
                }
            }
            EntityKind::Line(l) if annotation_layer(layer) => leaders.push((l.a.xy(), l.b.xy())),
            _ => {}
        }
    }
    let primary = outlines.len();
    outlines.extend(others.iter().map(|(o, _)| *o));
    // Candidates: (distance, tag index, outline index or none, anchor).
    let mut cands: Vec<(f64, usize, Option<usize>, Vec2)> = Vec::new();
    let mut found_tag = vec![None; todo.len()];
    for (ti, x) in todo.iter().enumerate() {
        for t in txt.iter().filter(|t| t.value.eq_ignore_ascii_case(&x.tag)) {
            // The tag's point and the far ends of leaders starting at it.
            let mut anchors = vec![t.at];
            for (a, b) in &leaders {
                if a.dist(t.at) < t.h * 1.5 {
                    anchors.push(*b);
                } else if b.dist(t.at) < t.h * 1.5 {
                    anchors.push(*a);
                }
            }
            if found_tag.get(ti).is_some_and(Option::is_none)
                && let Some(f) = found_tag.get_mut(ti)
            {
                *f = Some((*anchors.last().unwrap_or(&t.at), t.h, t.layer.clone()));
            }
            let reach = t.h * 6.0 + 24.0;
            for (oi, o) in outlines.iter().enumerate() {
                let dist = anchors.iter().map(|a| o.dist(*a)).fold(f64::INFINITY, f64::min);
                if dist <= reach {
                    cands.push((dist, ti, Some(oi), t.at));
                }
            }
        }
    }
    // Outlines on equipment layers before the others, each the nearest first.
    cands.sort_by(|a, b| a.2.is_some_and(|o| o >= primary).cmp(&b.2.is_some_and(|o| o >= primary)).then(a.0.total_cmp(&b.0)));
    let mut took_outline = vec![false; outlines.len()];
    let mut done = vec![false; todo.len()];
    let mut out = Vec::new();
    let el = elevation(levels, &level);
    let feet = |o: &Outline| {
        let [lo, hi] = o.corners();
        let (a, b) = (score::building_from_model(sh, lo), score::building_from_model(sh, hi));
        ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])])
    };
    for (dist, ti, oi, _) in &cands {
        let (Some(x), Some(oi)) = (todo.get(*ti), oi) else { continue };
        if done.get(*ti).copied().unwrap_or(true) || took_outline.get(*oi).copied().unwrap_or(true) {
            continue;
        }
        let Some(o) = outlines.get(*oi) else { continue };
        let (min, max) = feet(o);
        let other = oi.checked_sub(primary).and_then(|k| others.get(k));
        // A shape on another layer: only the printed size (either way round), or, with no size
        // printed, the shape the tag (or its leader) is in.
        let mut why = "";
        if other.is_some() {
            let (dx, dy) = (max[0] - min[0], max[1] - min[1]);
            let fits = |a: f64, b: f64| (a - b).abs() <= 0.1 * b.max(1.0);
            let ok = match x.size_ft {
                Some(s) if s[0] > 0.0 && s[1] > 0.0 => {
                    why = "it is the printed size";
                    x.round == matches!(o, Outline::Circle(..)) && ((fits(dx, s[0]) && fits(dy, s[1])) || (fits(dy, s[0]) && fits(dx, s[1])))
                }
                _ => {
                    why = "its tag is inside it";
                    *dist <= 0.0
                }
            };
            if !ok {
                continue;
            }
        }
        if let Some(t) = took_outline.get_mut(*oi) {
            *t = true;
        }
        if let Some(t) = done.get_mut(*ti) {
            *t = true;
        }
        let mut p = make(x, &level, &sh.number, min, max, "outline", matches!(o, Outline::Circle(..)), el);
        if let Some((_, layer)) = other {
            p.flags.push(format!("its outline is on layer {layer}, not an equipment layer: taken because {why}"));
            p.confidence = (p.confidence - 0.1).max(0.05);
        }
        out.push(p);
    }
    // Tags with no outline near: a box of the printed (or usual) size at the tag.
    for (ti, x) in todo.iter().enumerate() {
        if done.get(ti).copied().unwrap_or(true) {
            continue;
        }
        let Some(Some((at, _, _))) = found_tag.get(ti) else { continue };
        let c = score::building_from_model(sh, [at.x, at.y]);
        let (sz, from) = match x.size_ft {
            Some(s) if s[0] > 0.0 && s[1] > 0.0 => (s, "tag"),
            _ => (rule_size(&x.kind), "rule"),
        };
        let min = [c[0] - sz[0] / 2.0, c[1] - sz[1] / 2.0];
        let max = [c[0] + sz[0] / 2.0, c[1] + sz[1] / 2.0];
        let mut p = make(x, &level, &sh.number, min, max, from, x.round, el);
        p.flags.push("no outline on the plan: placed at its tag, which way it faces is a guess".into());
        out.push(p);
    }
    out
}

/// A placed item from its schedule row and footprint (building feet), standing on its level.
#[allow(clippy::too_many_arguments)]
fn make(x: &Scheduled, level: &str, sheet: &str, min: [f64; 2], max: [f64; 2], footprint_from: &str, round: bool, el: f64) -> Placed {
    let mut flags = Vec::new();
    let mut confidence: f64 = 0.9;
    let inlet = x
        .properties
        .iter()
        .find(|(k, _)| k.to_ascii_uppercase().contains("INLET"))
        .and_then(|(_, v)| v.trim().trim_end_matches('"').parse::<f64>().ok())
        .filter(|v| *v > 0.0 && *v < 60.0);
    let (h, height_from) = match (x.size_ft, inlet) {
        (Some(s), _) if s[2] > 0.0 => (s[2], "schedule"),
        (_, Some(i)) if x.kind == "vav" => {
            // A terminal box's casing is about 4" deeper than its inlet, 12" at the least.
            let h = (i + 4.0).max(12.0) / 12.0;
            flags.push(format!("height not printed: {:.2} ft from its {i}\" inlet", h));
            confidence -= 0.15;
            (h, "rule")
        }
        _ => {
            flags.push(format!("height not printed: {} ft is the usual for a {}", rule_size(&x.kind)[2], x.kind.replace('_', " ")));
            confidence -= 0.3;
            (rule_size(&x.kind)[2], "rule")
        }
    };
    match footprint_from {
        "outline" => {
            // The drawn outline against the printed size (either way round).
            if let Some(s) = x.size_ft.filter(|s| s[0] > 0.0) {
                let (dx, dy) = (max[0] - min[0], max[1] - min[1]);
                let fits = |a: f64, b: f64| (a - b).abs() <= 0.1 * b.max(1.0);
                let ok = (fits(dx, s[0]) && (s[1] <= 0.0 || fits(dy, s[1]))) || (fits(dy, s[0]) && (s[1] <= 0.0 || fits(dx, s[1])));
                if !ok {
                    flags.push(format!("the plan's outline ({:.1} x {:.1} ft) differs from the printed size", dx, dy));
                    confidence -= 0.2;
                }
            }
        }
        "tag" => confidence -= 0.3,
        _ => {
            flags.push(format!("no size printed: the usual size for a {}", x.kind.replace('_', " ")));
            confidence -= 0.5;
        }
    }
    if x.kind_from == "tag" {
        flags.push("kind from the tag's prefix (the schedule's title doesn't say)".into());
        confidence -= 0.1;
    }
    Placed {
        tag: x.tag.clone(),
        kind: x.kind.clone(),
        level: level.to_string(),
        sheet: sheet.to_string(),
        min_ft: [min[0], min[1], el],
        max_ft: [max[0], max[1], el + h],
        round,
        footprint_from: footprint_from.to_string(),
        height_from: height_from.to_string(),
        elevation_from: "floor".into(),
        confidence: confidence.clamp(0.05, 1.0),
        flags,
        connections: Vec::new(),
        title: x.title.clone(),
        properties: x.properties.clone(),
    }
}

/// Distance from a point to a plan rectangle (0 inside), feet.
fn rect_dist(p: [f64; 2], min: [f64; 3], max: [f64; 3]) -> f64 {
    box_dist(Vec2::new(p[0], p[1]), Vec2::new(min[0], min[1]), Vec2::new(max[0], max[1]))
}

/// A run's centreline above the floor, feet: its bottom plus half its depth.
fn centre_of(r: &FoundRun) -> Option<f64> {
    let depth = if r.shape == "rect" && r.size_in[1] > 0.0 { r.size_in[1] } else { r.size_in[0] };
    r.bottom_ft.filter(|b| b.is_finite()).map(|b| b + depth.max(0.0) / 24.0)
}

/// Most pieces one level's run graph holds, and pieces one trace visits.
const MAX_PIECES: usize = 50_000;
const MAX_TRACE: usize = 5_000;

/// One level's runs as a graph of straight pieces (building feet) for tracing from equipment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunGraph {
    pub pieces: Vec<Piece>,
    pub marks: Vec<Mark>,
    /// Per piece: the marks that belong to it and how far off they are.
    pub owned: Vec<Vec<(f64, usize)>>,
    /// Height notes: where, the kind of run, the run's centreline above the floor (feet).
    pub heights: Vec<([f64; 2], String, f64)>,
    /// Per piece: the height note nearest it (within reach), if any, and how far.
    pub height_of: Vec<Option<(f64, f64)>>,
}

/// Something on a plan that says what system a run is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mark {
    pub at: [f64; 2],
    pub system: String,
    /// The kind of run it marks: `duct` / `pipe`.
    pub kind: String,
    /// How far from a piece it may be and still belong to it, feet.
    pub reach: f64,
    /// Letters in a break of the line: they belong to the pieces they are in line with.
    pub in_line: bool,
    /// A tag's reading direction on the building plan, radians: the run it names is under it.
    pub rot: Option<f64>,
}

impl RunGraph {
    /// Give each mark to the pieces it belongs to: letters to the pieces either side of their
    /// break; a tag to the parallel run just under it (else the nearest); a device to the nearest.
    fn own(&mut self) {
        self.height_of = vec![None; self.pieces.len()];
        for (at, kind, centre) in &self.heights {
            let near = self
                .pieces
                .iter()
                .enumerate()
                .filter(|(_, pc)| pc.kind == *kind)
                .map(|(i, pc)| (i, to_segment(*at, pc.a, pc.b)))
                .filter(|(_, d)| *d <= 3.0)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((i, d)) = near
                && let Some(h) = self.height_of.get_mut(i)
                && h.is_none_or(|(o, _)| d < o)
            {
                *h = Some((d, *centre));
            }
        }
        self.owned = vec![Vec::new(); self.pieces.len()];
        for (mi, m) in self.marks.iter().enumerate() {
            let of_kind = || self.pieces.iter().enumerate().filter(|(_, pc)| pc.kind == m.kind);
            let mut offs: Vec<(usize, f64)> = if m.in_line {
                of_kind().filter_map(|(i, pc)| in_line(m.at, pc.a, pc.b, m.reach, 1.5).map(|o| (i, o))).collect()
            } else {
                of_kind().filter_map(|(i, pc)| Some(to_segment(m.at, pc.a, pc.b)).filter(|d| *d <= m.reach).map(|o| (i, o))).collect()
            };
            if let Some(r) = m.rot {
                let (u, n) = ([r.cos(), r.sin()], [-r.sin(), r.cos()]);
                let under: Vec<(usize, f64)> = offs
                    .iter()
                    .copied()
                    .filter(|(i, _)| {
                        self.pieces.get(*i).is_some_and(|pc| {
                            let len = dist2(pc.a, pc.b).max(1e-9);
                            let d = [(pc.b[0] - pc.a[0]) / len, (pc.b[1] - pc.a[1]) / len];
                            let parallel = (d[0] * u[1] - d[1] * u[0]).abs() < 0.1;
                            let above = (m.at[0] - pc.a[0]) * n[0] + (m.at[1] - pc.a[1]) * n[1] > 0.0;
                            parallel && above
                        })
                    })
                    .collect();
                if !under.is_empty() {
                    offs = under;
                }
            }
            let best = offs.iter().map(|(_, o)| *o).fold(f64::INFINITY, f64::min);
            for (i, o) in offs {
                if o <= best + 0.05
                    && let Some(v) = self.owned.get_mut(i)
                {
                    v.push((o, mi));
                }
            }
        }
    }
}

/// A straight piece of a run.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Piece {
    /// `duct` or `pipe`.
    pub kind: String,
    pub a: [f64; 2],
    pub b: [f64; 2],
    /// Half its width, feet (how far apart two pieces' ends may be and still join).
    pub half_ft: f64,
    /// The system its reader gave it (empty when unknown).
    pub system: String,
    pub centre_ft: Option<f64>,
}

fn dist2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Distance from `p` to segment a–b, and how far along it (0–len) the nearest point is.
fn to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 1e-18 { (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    dist2(p, [a[0] + d[0] * t, a[1] + d[1] * t])
}

/// Whether `p` lies in line with a–b (off its line by under `across`) and within `past` of it
/// along the line.
fn in_line(p: [f64; 2], a: [f64; 2], b: [f64; 2], across: f64, past: f64) -> Option<f64> {
    let len = dist2(a, b);
    if len < 1e-9 {
        return None;
    }
    let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
    let v = [p[0] - a[0], p[1] - a[1]];
    let along = v[0] * u[0] + v[1] * u[1];
    let off = (v[0] * u[1] - v[1] * u[0]).abs();
    if off > across || along < -past || along > len + past {
        return None;
    }
    Some(if along < 0.0 {
        -along
    } else if along > len {
        along - len
    } else {
        0.0
    })
}

/// The runs of one plan sheet as pieces and system marks: ducts from the duct reader (marked by
/// the terminals at their ends: diffusers supply, return grilles return, exhaust grilles exhaust,
/// louvers outside air — by symbol or by their type tag), pipes straight from the drawing (marked
/// by the letters printed in their lines and in their size tags).
pub fn run_graph(d: &Drawing, sh: &Sheet) -> RunGraph {
    let mut g = RunGraph::default();
    let bf = |p: Vec2| score::building_from_model(sh, [p.x, p.y]);
    let txt = texts(d);
    match sh.discipline.as_str() {
        "M" => {
            let net = duct_reader::read_ducts(d, sh);
            for r in net.to_found_runs(sh).into_iter().take(MAX_PIECES) {
                let half = r.size_in[0].max(0.0) / 24.0;
                g.pieces.push(Piece {
                    kind: "duct".into(),
                    a: r.from_ft,
                    b: r.to_ft,
                    half_ft: half,
                    system: r.system.clone(),
                    centre_ft: centre_of(&r),
                });
            }
            for (at, code) in air_devices(d) {
                g.marks.push(Mark { at: bf(at), system: code.to_string(), kind: "duct".into(), reach: 2.0, in_line: false, rot: None });
            }
            for t in &txt {
                let prefix: String = t.value.to_ascii_uppercase().chars().take_while(|c| c.is_ascii_alphabetic()).collect();
                let has_num = t.value.chars().any(|c| c.is_ascii_digit());
                let code = match prefix.as_str() {
                    "CD" | "SD" | "LD" | "SR" => "SA",
                    "RG" | "RR" => "RA",
                    "EG" | "ER" => "EA",
                    "L" | "OAL" | "IL" => "OA",
                    _ => continue,
                };
                if has_num && t.value.len() <= 6 && t.value.contains('-') {
                    g.marks.push(Mark { at: bf(t.at), system: code.to_string(), kind: "duct".into(), reach: 2.5, in_line: false, rot: None });
                }
            }
        }
        "P" | "FP" => {
            for e in d.model.iter().take(MAX_ITEMS) {
                let layer = e.common.layer.as_str();
                if !["PIPE", "HEAVY", "DASHED"].iter().any(|l| layer.eq_ignore_ascii_case(l)) {
                    continue;
                }
                let mut add = |a: Vec2, b: Vec2| {
                    if g.pieces.len() < MAX_PIECES && a.is_finite() && b.is_finite() && a.dist(b) > 1e-9 {
                        g.pieces.push(Piece { kind: "pipe".into(), a: bf(a), b: bf(b), half_ft: 0.1, system: String::new(), centre_ft: None });
                    }
                };
                match &e.kind {
                    EntityKind::Line(l) => add(l.a.xy(), l.b.xy()),
                    EntityKind::LwPolyline(p) => {
                        for w in p.vertices.windows(2) {
                            if let [x, y] = w {
                                add(x.p, y.p);
                            }
                        }
                    }
                    _ => {}
                }
            }
            for t in &txt {
                let up = t.value.to_ascii_uppercase();
                let h = t.h / 12.0;
                if let Some((_, code)) = PIPE_ABBR.iter().find(|(a, _)| a.eq_ignore_ascii_case(up.trim())) {
                    // Letters in a break of the line.
                    g.marks.push(Mark { at: bf(t.at), system: code.to_string(), kind: "pipe".into(), reach: h * 0.75, in_line: true, rot: None });
                } else if up.contains('"')
                    && let Some(code) =
                        up.split_whitespace().nth(1).and_then(|w| PIPE_ABBR.iter().find(|(a, _)| a.eq_ignore_ascii_case(w)).map(|(_, c)| *c))
                {
                    // A size tag beside the run (`1" CW`).
                    let rot = t.rot - sh.model_from_building.rotation_deg.to_radians();
                    g.marks.push(Mark {
                        at: bf(t.at),
                        system: code.to_string(),
                        kind: "pipe".into(),
                        reach: h * 3.0,
                        in_line: false,
                        rot: Some(rot),
                    });
                }
            }
        }
        _ => {}
    }
    // Height notes, with the size printed with them (a run's centre is half its depth above its
    // bottom, half below its top).
    let kind = if sh.discipline == "M" { "duct" } else { "pipe" };
    for t in &txt {
        let Some((which, ft)) = parse_height_note(&t.value) else { continue };
        let depth_in = txt
            .iter()
            .filter(|o| o.at.dist(t.at) < t.h * 3.0 && o.at.dist(t.at) > 1e-9)
            .min_by(|a, b| a.at.dist(t.at).total_cmp(&b.at.dist(t.at)))
            .and_then(|o| size_depth_in(&o.value));
        let half = depth_in.unwrap_or(0.0) / 24.0;
        let centre = match which {
            'B' => ft + half,
            'T' => ft - half,
            _ => ft,
        };
        g.heights.push((bf(t.at), kind.to_string(), centre));
    }
    g
}

/// A height note: `BOD @ 10'-6"`, `B.O.D. 10'-6" AFF`, `BOT EL 9'-0"`, `TOD @ 11'-8"`,
/// `CL @ 11'-0"`, `BOP @ 9'-6"`, `B.O.P. 9'-6" AFF` → (`B`ottom / `T`op / `C`entre, feet).
pub fn parse_height_note(s: &str) -> Option<(char, f64)> {
    let up = s.to_ascii_uppercase().replace('.', "");
    let words: Vec<&str> = up.split_whitespace().collect();
    let which = match words.first().copied()? {
        "BOD" | "BOT" | "BOP" | "BOTTOM" => 'B',
        "TOD" | "TOP" => 'T',
        "CL" | "C/L" => 'C',
        _ => return None,
    };
    // The first word with a foot mark, with an inch fraction after it (`8'-7 1/2"`).
    let at = words.iter().position(|w| w.contains('\''))?;
    let mut v = words.get(at).map(|w| w.to_string()).unwrap_or_default();
    if let Some(next) = words.get(at + 1).filter(|w| w.contains('/') && w.ends_with('"')) {
        v.push(' ');
        v.push_str(next);
    }
    parse_length_ft(&v, false).map(|ft| (which, ft))
}

/// The depth a size tag gives, inches: `24x14` → 14, `24"x14"`, `24/14`, `12"Ø` → 12, `3/4" HWR`
/// → 0.75.
pub fn size_depth_in(s: &str) -> Option<f64> {
    let up = s.trim().to_ascii_uppercase();
    let first = up.split_whitespace().next()?;
    // `24x14`, `24"x14"`; `24/14` when there is no inch mark (`3/4"` is a pipe size).
    let seps: &[char] = if first.contains('X') {
        &['X']
    } else if first.contains('"') {
        &[]
    } else {
        &['/']
    };
    for sep in seps {
        if let Some((a, b)) = first.split_once(*sep) {
            let (a, b) = (a.trim_end_matches('"'), b.trim_end_matches('"'));
            if let (Ok(_), Ok(d)) = (a.parse::<f64>(), b.parse::<f64>()) {
                return Some(d).filter(|d| *d > 0.0 && *d < 200.0);
            }
        }
    }
    if first.ends_with('"') || first.ends_with("\"Ø") || first.ends_with('Ø') {
        let num = first.trim_end_matches('Ø').trim_end_matches('"');
        return parse_length_ft(&format!("{num}\""), true).map(|f| f * 12.0).filter(|d| *d < 200.0);
    }
    None
}

/// The duct lines of a mechanical plan as pieces: the sides (`DUCT`) and centrelines (`CENTRE`).
fn drawn_ducts(d: &Drawing, sh: &Sheet) -> Vec<Piece> {
    let mut out = Vec::new();
    for e in d.model.iter().take(MAX_ITEMS) {
        let layer = e.common.layer.as_str();
        if !(layer.eq_ignore_ascii_case("DUCT") || layer.eq_ignore_ascii_case("CENTRE")) {
            continue;
        }
        if let EntityKind::Line(l) = &e.kind
            && out.len() < MAX_PIECES
            && l.a.xy().is_finite()
            && l.b.xy().is_finite()
        {
            let (a, b) = (score::building_from_model(sh, [l.a.x, l.a.y]), score::building_from_model(sh, [l.b.x, l.b.y]));
            if dist2(a, b) > 1e-6 {
                out.push(Piece { kind: "duct".into(), a, b, half_ft: 0.4, system: String::new(), centre_ft: None });
            }
        }
    }
    out
}

/// The air devices on a mechanical plan by their symbols (on `DEVICE`): a box with an X and a
/// box inside supplies (`SA`), a box of bars across returns (`RA`), a box with one diagonal and
/// two half ones exhausts (`EA`), a thin box of slanted bars is a louver (`OA`).
fn air_devices(d: &Drawing) -> Vec<(Vec2, &'static str)> {
    let rects: Vec<(Vec2, Vec2)> = boxes(d).into_iter().filter(|(_, _, l)| l.eq_ignore_ascii_case("DEVICE")).map(|(lo, hi, _)| (lo, hi)).collect();
    let lines: Vec<(Vec2, Vec2)> = d
        .model
        .iter()
        .take(MAX_ITEMS)
        .filter(|e| e.common.layer.eq_ignore_ascii_case("DEVICE"))
        .filter_map(|e| if let EntityKind::Line(l) = &e.kind { Some((l.a.xy(), l.b.xy())) } else { None })
        .collect();
    let mut out = Vec::new();
    for (lo, hi) in &rects {
        let (w, h) = (hi.x - lo.x, hi.y - lo.y);
        // Inner boxes (a diffuser's face) are read with their outer box.
        if rects.iter().any(|(a, b)| a.x < lo.x - 1e-6 && a.y < lo.y - 1e-6 && b.x > hi.x + 1e-6 && b.y > hi.y + 1e-6) {
            continue;
        }
        let pad = Vec2::new(0.05 * w.max(h), 0.05 * w.max(h));
        let inner: Vec<&(Vec2, Vec2)> = lines.iter().filter(|(a, b)| inside(*a, *lo - pad, *hi + pad) && inside(*b, *lo - pad, *hi + pad)).collect();
        let square = |a: &Vec2, b: &Vec2| (a.x - b.x).abs() < 1e-6 || (a.y - b.y).abs() < 1e-6;
        let bars = inner.iter().filter(|(a, b)| square(a, b)).count();
        let diag = inner.len() - bars;
        let has_face = rects.iter().any(|(a, b)| a.x > lo.x + 1e-6 && a.y > lo.y + 1e-6 && b.x < hi.x - 1e-6 && b.y < hi.y - 1e-6);
        let thin = w.min(h) < 0.4 * w.max(h);
        let code = if thin && diag >= 2 {
            "OA"
        } else if has_face && diag >= 2 {
            "SA"
        } else if bars >= 3 {
            "RA"
        } else if diag >= 1 {
            "EA"
        } else {
            continue;
        };
        out.push(((*lo + *hi) * 0.5, code));
    }
    out
}

/// Join the runs that end at (or drop into) the item: each piece of the graph with an end on or
/// near its outline starts a trace along the run (through joined ends, and across the breaks the
/// system letters sit in) to the nearest system mark; a run whose pieces only pass by isn't
/// joined. Hung items don't take pipes that merely end inside their outline from above.
fn join_runs(p: &mut Placed, g: &RunGraph, hung: bool) {
    let (min, max) = (p.min_ft, p.max_ft);
    // What a kind takes: air handlers ducts and their gas or water, terminal boxes and fans
    // ducts only, the rest pipes only.
    let takes = |kind: &str| match p.kind.as_str() {
        "ahu" | "rtu" => true,
        "vav" | "exhaust_fan" | "fan" => kind == "duct",
        _ => kind == "pipe",
    };
    let mut conns: Vec<Connection> = Vec::new();
    let mut taken = vec![false; g.pieces.len()];
    for (i, pc) in g.pieces.iter().enumerate() {
        if taken.get(i).copied().unwrap_or(true) || !takes(&pc.kind) {
            continue;
        }
        let tol = if pc.kind == "duct" { 1.0 + pc.half_ft } else { 0.6 };
        let end = [pc.a, pc.b].into_iter().find(|e| rect_dist(*e, min, max) <= tol);
        let Some(at) = end else { continue };
        // A pipe passing over a hung box isn't in it; one ending in it is.
        if hung && pc.kind == "pipe" && rect_dist(at, min, max) > 0.0 && rect_dist(at, min, max) > 0.3 {
            continue;
        }
        let (members, system, noted) = trace(g, i, min, max);
        for m in &members {
            if let Some(t) = taken.get_mut(*m) {
                *t = true;
            }
        }
        let centre = noted.or_else(|| members.iter().filter_map(|m| g.pieces.get(*m)).find_map(|q| q.centre_ft));
        if !conns.iter().any(|c| c.system == system && c.kind == pc.kind && dist2(c.at_ft, at) < 1.0) {
            conns.push(Connection { kind: pc.kind.clone(), system, at_ft: at, via: "end".into(), centre_ft: centre });
        }
    }
    p.connections = conns;
}

/// The pieces reachable from piece `start` without going back into the outline, and the system
/// of the nearest mark among them (else the most common system the run reader gave them).
fn trace(g: &RunGraph, start: usize, min: [f64; 3], max: [f64; 3]) -> (Vec<usize>, String, Option<f64>) {
    let mut seen = vec![false; g.pieces.len()];
    let mut queue = std::collections::VecDeque::new();
    let mut out = Vec::new();
    if let Some(s) = seen.get_mut(start) {
        *s = true;
    }
    queue.push_back(start);
    while let Some(i) = queue.pop_front() {
        if out.len() >= MAX_TRACE {
            break;
        }
        out.push(i);
        let Some(pc) = g.pieces.get(i) else { continue };
        // Pieces joining this one at an end outside the outline (or across a lettered break).
        for (j, q) in g.pieces.iter().enumerate() {
            if seen.get(j).copied().unwrap_or(true) || q.kind != pc.kind {
                continue;
            }
            let join = (pc.half_ft.max(q.half_ft) * 1.5).max(0.05);
            let outside = |e: &[f64; 2]| rect_dist(*e, min, max) > 1e-6;
            let touches = [pc.a, pc.b]
                .iter()
                .filter(|e| outside(e))
                .any(|e| [q.a, q.b].iter().any(|f| dist2(*e, *f) <= join || (pc.kind == "duct" && to_segment(*e, q.a, q.b) <= join * 0.5)));
            // A pipe broken for its letters (or a crossing): the next piece on the same line, a
            // short gap on, the gap clear of the outline.
            let across_break = pc.kind == "pipe"
                && in_line(q.a, pc.a, pc.b, 0.05, 4.0).is_some()
                && in_line(q.b, pc.a, pc.b, 0.05, 4.0 + dist2(q.a, q.b)).is_some()
                && [pc.a, pc.b].iter().filter(|e| outside(e)).any(|e| {
                    [q.a, q.b].iter().filter(|f| outside(f)).any(|f| {
                        let mid = [(e[0] + f[0]) / 2.0, (e[1] + f[1]) / 2.0];
                        dist2(*e, *f) < 4.0 && outside(&mid)
                    })
                });
            if touches || across_break {
                if let Some(s) = seen.get_mut(j) {
                    *s = true;
                }
                queue.push_back(j);
            }
        }
    }
    // Letters in the line before tags beside it, the nearest along the run first (the trace is
    // breadth first), then the closest.
    let mut system = String::new();
    for want_line in [true, false] {
        let best = out
            .iter()
            .enumerate()
            .flat_map(|(k, i)| g.owned.get(*i).into_iter().flatten().map(move |(o, mi)| (k, *o, *mi)))
            .filter_map(|(k, o, mi)| g.marks.get(mi).filter(|m| m.in_line == want_line).map(|m| (k, o, m.system.as_str())))
            .min_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        if let Some((_, _, c)) = best {
            system = c.to_string();
            break;
        }
    }
    if system.is_empty() {
        let mut count: BTreeMap<&str, usize> = BTreeMap::new();
        for q in out.iter().filter_map(|i| g.pieces.get(*i)).filter(|q| !q.system.is_empty()) {
            *count.entry(q.system.as_str()).or_default() += 1;
        }
        if let Some((s, _)) = count.into_iter().max_by_key(|(_, n)| *n) {
            system = s.to_string();
        }
    }
    // The nearest height note along the run.
    let centre = out.iter().find_map(|i| g.height_of.get(*i).copied().flatten()).map(|(_, c)| c);
    (out, system, centre)
}

/// Hung items: centred on their run's height when it is known.
fn set_hung_height(p: &mut Placed, levels: &[Level]) {
    let el = elevation(levels, &p.level);
    let h = p.max_ft[2] - p.min_ft[2];
    p.elevation_from = "rule".into();
    let mut centre = el + HUNG_CENTRE_FT;
    // The runs it is in (ending at it): the mean of their centrelines.
    let known: Vec<f64> = p.connections.iter().filter(|c| c.via == "end").filter_map(|c| c.centre_ft).collect();
    if !known.is_empty() {
        centre = el + known.iter().sum::<f64>() / known.len() as f64;
        p.elevation_from = "run".into();
    }
    p.min_ft[2] = centre - h / 2.0;
    p.max_ft[2] = centre + h / 2.0;
    if p.elevation_from == "rule" {
        p.flags.push(format!("hung in its run, whose height isn't known: centred {HUNG_CENTRE_FT} ft above the floor"));
        p.confidence = (p.confidence - 0.2).max(0.05);
    }
}

/// Read a generated fixture set (its DXF sheets, aligned by the truth's sheet maps; the levels
/// from the truth, as the cover sheet lists them).
pub fn read_fixture_set(set: &crate::FixtureSet) -> EquipmentRead {
    let t = &set.truth;
    let drawings: Vec<(&Sheet, Drawing)> =
        t.sheets.iter().filter_map(|sh| set.file(&sh.dxf).and_then(|b| cadaddy_io::read_dxf(b).ok()).map(|d| (sh, d))).collect();
    let ins: Vec<SheetIn> = drawings.iter().map(|(sheet, drawing)| SheetIn { sheet, drawing }).collect();
    read_equipment(&ins, &t.building.levels)
}

// ---- Scoring -----------------------------------------------------------------------------------

/// How well a read matches the truth.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct EquipScore {
    /// Distinct tags in the truth's equipment, and how many the schedules gave.
    pub truth_tags: usize,
    pub scheduled_found: usize,
    /// Scheduled items the reader gave a kind the truth doesn't have.
    pub kind_wrong: Vec<String>,
    /// Of the truth's schedule cells, the share read exactly.
    pub cell_accuracy: f64,
    /// Items on a level with plans (the truth's), and how many were placed.
    pub expected_placed: usize,
    pub placed: usize,
    /// Placed tags the truth doesn't have (or not on that level).
    pub placed_wrong: Vec<String>,
    /// Plan centre error, feet: mean and worst.
    pub centre_err_mean_ft: f64,
    pub centre_err_max_ft: f64,
    /// Plan footprint overlap (intersection over union), mean.
    pub footprint_iou_mean: f64,
    /// Bottom and top elevation error, feet, mean and worst (over both).
    pub z_err_mean_ft: f64,
    pub z_err_max_ft: f64,
    /// Systems joined: share of the truth's connected systems found, and of those found that
    /// are right (over placed items).
    pub system_recall: f64,
    pub system_precision: f64,
    /// Per item: tag → (centre error, IoU, z error, truth systems, found systems).
    pub items: BTreeMap<String, (f64, f64, f64, Vec<String>, Vec<String>)>,
}

/// Score a read against the truth. Expected placements: equipment on a level that has a
/// mechanical, plumbing or fire protection plan in the set; joined systems: the systems of the truth's nodes at the item that a plan
/// shows (horizontal runs, not below the floor).
pub fn score(read: &EquipmentRead, t: &Truth) -> EquipScore {
    let mut sc = EquipScore::default();
    let mut tags: BTreeMap<&str, &crate::truth::Equipment> = BTreeMap::new();
    for e in &t.equipment {
        tags.entry(e.tag.as_str()).or_insert(e);
    }
    sc.truth_tags = tags.len();
    let (mut cells, mut cells_ok) = (0usize, 0usize);
    for (tag, e) in &tags {
        let Some(s) = read.scheduled.iter().find(|s| s.tag.eq_ignore_ascii_case(tag)) else {
            cells += e.schedule.len();
            continue;
        };
        sc.scheduled_found += 1;
        if s.kind != e.kind {
            sc.kind_wrong.push(format!("{tag}: {} (truth {})", s.kind, e.kind));
        }
        for (k, v) in &e.schedule {
            cells += 1;
            if s.properties.get(k).is_some_and(|x| x.trim() == v.trim()) {
                cells_ok += 1;
            }
        }
    }
    sc.cell_accuracy = if cells == 0 { 1.0 } else { cells_ok as f64 / cells as f64 };

    let plan_levels: BTreeSet<&str> =
        t.sheets.iter().filter(|s| ["M", "P", "FP"].contains(&s.discipline.as_str())).filter_map(|s| s.level.as_deref()).collect();
    let expected: Vec<&crate::truth::Equipment> = t.equipment.iter().filter(|e| plan_levels.contains(e.level.as_str())).collect();
    sc.expected_placed = expected.len();
    let (mut ce, mut iou, mut ze) = (Vec::new(), Vec::new(), Vec::new());
    let (mut sys_truth, mut sys_found, mut sys_ok) = (0usize, 0usize, 0usize);
    for p in &read.placed {
        let Some(e) = expected.iter().find(|e| e.tag.eq_ignore_ascii_case(&p.tag) && e.level == p.level) else {
            sc.placed_wrong.push(p.tag.clone());
            continue;
        };
        sc.placed += 1;
        let c = |lo: [f64; 3], hi: [f64; 3]| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
        let (a, b) = (c(p.min_ft, p.max_ft), c(e.min_ft, e.max_ft));
        let err = (a[0] - b[0]).hypot(a[1] - b[1]);
        let ix = (p.max_ft[0].min(e.max_ft[0]) - p.min_ft[0].max(e.min_ft[0])).max(0.0);
        let iy = (p.max_ft[1].min(e.max_ft[1]) - p.min_ft[1].max(e.min_ft[1])).max(0.0);
        let area = |lo: [f64; 3], hi: [f64; 3]| (hi[0] - lo[0]) * (hi[1] - lo[1]);
        let union = area(p.min_ft, p.max_ft) + area(e.min_ft, e.max_ft) - ix * iy;
        let o = if union > 1e-9 { ix * iy / union } else { 0.0 };
        let z = (p.min_ft[2] - e.min_ft[2]).abs().max((p.max_ft[2] - e.max_ft[2]).abs());
        ce.push(err);
        iou.push(o);
        ze.push(z);
        // The truth's systems at the item that a plan shows.
        let shown: BTreeSet<String> = e
            .nodes
            .iter()
            .filter_map(|n| t.node(n))
            .filter(|n| n.segments.iter().filter_map(|s| t.segment(s)).any(|s| !s.below_floor && (!s.vertical || reaches_plan_run(t, s))))
            .map(|n| n.system.clone())
            .collect();
        let found: BTreeSet<String> = p.systems().into_iter().collect();
        sys_truth += shown.len();
        sys_found += found.len();
        sys_ok += shown.intersection(&found).count();
        sc.items.insert(p.tag.clone(), (err, o, z, shown.into_iter().collect(), found.into_iter().collect()));
    }
    let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    sc.centre_err_mean_ft = mean(&ce);
    sc.centre_err_max_ft = ce.iter().copied().fold(0.0, f64::max);
    sc.footprint_iou_mean = mean(&iou);
    sc.z_err_mean_ft = mean(&ze);
    sc.z_err_max_ft = ze.iter().copied().fold(0.0, f64::max);
    sc.system_recall = if sys_truth == 0 { 1.0 } else { sys_ok as f64 / sys_truth as f64 };
    sc.system_precision = if sys_found == 0 { 1.0 } else { sys_ok as f64 / sys_found as f64 };
    sc
}

/// A vertical segment (a drop into a unit) whose other end meets a horizontal run a plan shows.
fn reaches_plan_run(t: &Truth, s: &crate::truth::Segment) -> bool {
    [&s.from, &s.to]
        .iter()
        .filter_map(|n| t.node(n))
        .any(|n| n.segments.iter().filter(|id| **id != s.id).filter_map(|id| t.segment(id)).any(|o| !o.vertical && !o.below_floor))
}
