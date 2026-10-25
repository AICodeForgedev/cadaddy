//! Casework and plumbing fixture reader: recognises cabinets, counters, vanities, appliances,
//! backsplashes and wardrobes, and plumbing fixtures (water closets, lavatories, sinks, urinals,
//! showers, mop sinks, bathtubs, water heaters) on architectural floor plan DXF sheets (A-10x),
//! and reads the casework / plumbing fixture schedule that names each tag.
//!
//! - Casework symbols are rectangles on the `CASEWORK` layer with a tag and an AFF height note.
//!   Stacked items (a base cabinet, its countertop and the upper cabinet over it) share one
//!   footprint, so each rectangle takes the tag and note drawn with it (the entities after it,
//!   before the next rectangle), else the nearest tag nobody else took. A rectangle drawn inside
//!   another with the same inset on every side is the outer one's decoration (an upper cabinet's
//!   inner line), not an item.
//! - Fixture symbols are drawn on a layer named like `*FIXT*`, each followed by its tag; the
//!   fixture sits at the centre of the symbol drawn before its tag.
//! - The schedule (layer [`SCHEDULE_LAYER`]) gives each tag its kind, size, height and spec
//!   section; when a plan tag has a row, kind and height come from it (status `schedule`), else
//!   from the plan's note and the tag's prefix (status `note`), else from the symbol's pattern
//!   (status `pattern`).
//!
//! All coordinates here are model inches (the sheets draw the building at full size).
//! Output feeds device placement zones (counter receptacles over the backsplash,
//! [`crate::elec_reader::counter_receptacle_zones`]) and in-wall clash detection behind sinks
//! and dishwashers ([`crate::clash::casework_clashes`]).

use crate::score;
use crate::truth::{Sheet, Truth};
use cadaddy_doc::{Drawing, EntityKind};
use std::collections::HashMap;

/// The layer the casework / plumbing fixture schedule is drawn on.
pub const SCHEDULE_LAYER: &str = "CASEWORK-SCHEDULE";
/// The schedule's title.
pub const SCHEDULE_TITLE: &str = "CASEWORK AND PLUMBING FIXTURE SCHEDULE";
/// The layer plumbing fixture symbols are drawn on (the reader takes any `*FIXT*` layer).
pub const FIXTURE_LAYER: &str = "PLUMB-FIXT";

/// Most entities of one kind looked at (hostile drawings).
const MAX_ITEMS: usize = 20_000;
/// Largest believable item side, model inches (100 ft).
const MAX_SIDE: f64 = 1200.0;
/// Smallest item side, model inches.
const MIN_SIDE: f64 = 1.0;
/// A tag or note further than this from its item (model inches) is not its own.
const NEAR: f64 = 36.0;
/// Grid cell for the spatial lookups, model inches.
const CELL: f64 = 48.0;

/// A casework item detected on the sheet (model-inch coordinates).
#[derive(Clone, Debug, Default)]
pub struct DetectedCasework {
    /// Bounding-box minimum in model inches.
    pub min_in: [f64; 2],
    /// Bounding-box maximum in model inches.
    pub max_in: [f64; 2],
    /// Classified kind: `base_cabinet`, `upper_cabinet`, `counter`, `vanity`, `appliance`,
    /// `backsplash`, `wardrobe`.
    pub kind: String,
    /// Tag text found on or near the item (e.g. `BC-1`, `V-1`, `CT-1`).
    pub tag: String,
    /// Height in inches AFF (from the schedule, else the plan's note), or 0 if none found.
    pub height_in: f64,
    /// Depth in inches: the schedule's, else the footprint's short side.
    pub depth_in: f64,
    /// The schedule's description of the tag (empty without a schedule row).
    pub description: String,
    /// The schedule's spec section.
    pub spec_section: Option<String>,
    /// Where kind and height come from: `schedule`, `note` (plan tag and AFF note) or
    /// `pattern` (the symbol's hatch only).
    pub status: String,
}

impl DetectedCasework {
    /// Whether the item is (or holds) a sink: plumbing rough-ins sit in the wall behind it.
    pub fn is_sink(&self) -> bool {
        let t = self.tag.to_ascii_uppercase();
        t.starts_with("SK") || self.description.to_ascii_uppercase().contains("SINK")
    }

    /// Whether the item is a dishwasher.
    pub fn is_dishwasher(&self) -> bool {
        self.tag.to_ascii_uppercase().starts_with("DW") || self.description.to_ascii_uppercase().contains("DISHWASHER")
    }
}

/// A plumbing fixture detected on the sheet (model-inch coordinates).
#[derive(Clone, Debug, Default)]
pub struct DetectedFixture {
    /// The fixture's centre (the centre of its symbol), model inches.
    pub at_in: [f64; 2],
    /// `water_closet`, `lavatory`, `sink`, `urinal`, `shower`, `mop_sink`, `bathtub`,
    /// `floor_drain`, `water_heater`.
    pub kind: String,
    pub tag: String,
    /// Mounting height in inches AFF from the schedule, or 0 when it gives none.
    pub height_in: f64,
    pub description: String,
    pub spec_section: Option<String>,
    /// `schedule` (kind from the schedule row), `tag` (from the tag's prefix).
    pub status: String,
}

/// One row of the casework / plumbing fixture schedule.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScheduleRow {
    pub tag: String,
    /// The kind its description names (see [`kind_from_description`]); empty if none.
    pub kind: String,
    pub description: String,
    /// Width in inches (`None` when the schedule says VARIES or gives none).
    pub width_in: Option<f64>,
    pub depth_in: Option<f64>,
    /// Height in inches AFF.
    pub height_in: Option<f64>,
    pub spec_section: Option<String>,
}

/// Everything read from one sheet.
#[derive(Clone, Debug, Default)]
pub struct Reading {
    pub casework: Vec<DetectedCasework>,
    pub fixtures: Vec<DetectedFixture>,
    pub schedule: Vec<ScheduleRow>,
    /// Plan tags with no schedule row (when the sheet has a schedule).
    pub unscheduled: Vec<String>,
}

/// Read casework from a DXF drawing (an architectural floor plan sheet).
///
/// Returns all detected casework items in model-inch coordinates.
pub fn read_casework(d: &Drawing) -> Vec<DetectedCasework> {
    read(d).casework
}

/// Read casework, plumbing fixtures and the schedule from one sheet.
pub fn read(d: &Drawing) -> Reading {
    let schedule = read_schedule(d);
    let rows: HashMap<String, &ScheduleRow> = schedule.iter().map(|r| (norm_tag(&r.tag), r)).collect();
    let casework = read_items(d, &rows);
    let fixtures = read_fixtures(d, &rows);
    let mut unscheduled: Vec<String> = Vec::new();
    if !rows.is_empty() {
        for t in casework.iter().map(|c| &c.tag).chain(fixtures.iter().map(|f| &f.tag)) {
            if !t.is_empty() && !rows.contains_key(&norm_tag(t)) && !unscheduled.contains(t) {
                unscheduled.push(t.clone());
            }
        }
    }
    Reading { casework, fixtures, schedule, unscheduled }
}

// ---- Casework ----------------------------------------------------------------------------

/// What the CASEWORK layer holds, in drawing order.
enum Piece {
    Rect([f64; 2], [f64; 2]),
    Line([f64; 2], [f64; 2]),
    Tag(String, [f64; 2]),
    Note(f64, [f64; 2]),
}

/// An item while it is being read.
#[derive(Default)]
struct Draft {
    min: [f64; 2],
    max: [f64; 2],
    diagonals: Vec<([f64; 2], [f64; 2])>,
    inner: bool,
    tag: Option<(String, [f64; 2])>,
    note: Option<f64>,
}

fn read_items(d: &Drawing, rows: &HashMap<String, &ScheduleRow>) -> Vec<DetectedCasework> {
    let pieces = casework_pieces(d);
    // The rectangles first: which are decorations of another (same inset on every side).
    let rects: Vec<([f64; 2], [f64; 2])> = pieces
        .iter()
        .filter_map(|p| match p {
            Piece::Rect(a, b) => Some((*a, *b)),
            _ => None,
        })
        .collect();
    let deco = decorations(&rects);

    let mut drafts: Vec<Draft> = Vec::new();
    let mut loose_tags: Vec<(String, [f64; 2])> = Vec::new();
    let mut loose_notes: Vec<(f64, [f64; 2])> = Vec::new();
    let mut loose_lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut loose_inner: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut rect_i = 0usize;
    for p in &pieces {
        match p {
            Piece::Rect(min, max) => {
                let is_deco = deco.get(rect_i).copied().unwrap_or(false);
                rect_i += 1;
                if is_deco {
                    match drafts.last_mut() {
                        Some(cur) if contains(cur.min, cur.max, *min, 0.5) && contains(cur.min, cur.max, *max, 0.5) => cur.inner = true,
                        _ => loose_inner.push((*min, *max)),
                    }
                    continue;
                }
                let (w, h) = (max[0] - min[0], max[1] - min[1]);
                if !(MIN_SIDE..=MAX_SIDE).contains(&w) || !(MIN_SIDE..=MAX_SIDE).contains(&h) {
                    continue;
                }
                if drafts.len() < MAX_ITEMS {
                    drafts.push(Draft { min: *min, max: *max, ..Draft::default() });
                }
            }
            Piece::Line(a, b) => {
                if is_axis_aligned(*a, *b) {
                    continue;
                }
                match drafts.last_mut() {
                    Some(cur) if inside_rect(*a, cur.min, cur.max, 2.0) && inside_rect(*b, cur.min, cur.max, 2.0) => cur.diagonals.push((*a, *b)),
                    _ => loose_lines.push((*a, *b)),
                }
            }
            Piece::Tag(t, at) => match drafts.last_mut() {
                Some(cur) if cur.tag.is_none() && inside_rect(*at, cur.min, cur.max, NEAR / 3.0) => cur.tag = Some((t.clone(), *at)),
                _ => loose_tags.push((t.clone(), *at)),
            },
            Piece::Note(v, at) => match drafts.last_mut() {
                Some(cur) if cur.note.is_none() && cur.tag.as_ref().is_some_and(|(_, tp)| dist(*tp, *at) <= NEAR) => cur.note = Some(*v),
                _ => loose_notes.push((*v, *at)),
            },
        }
    }

    // Drawn out of order: hatch lines and inner lines inside an item without them, tags and
    // notes to the nearest item still without one.
    for (a, b) in loose_lines {
        if let Some(cur) = drafts
            .iter_mut()
            .filter(|c| inside_rect(a, c.min, c.max, 2.0) && inside_rect(b, c.min, c.max, 2.0))
            .min_by(|x, y| area(x).total_cmp(&area(y)))
        {
            cur.diagonals.push((a, b));
        }
    }
    for (min, max) in loose_inner {
        if let Some(cur) = drafts
            .iter_mut()
            .filter(|c| contains(c.min, c.max, min, 0.5) && contains(c.min, c.max, max, 0.5))
            .min_by(|x, y| area(x).total_cmp(&area(y)))
        {
            cur.inner = true;
        }
    }
    let tag_grid = Grid::new(loose_tags.iter().map(|(_, p)| *p));
    let mut tag_taken = vec![false; loose_tags.len()];
    for cur in drafts.iter_mut().filter(|c| c.tag.is_none()) {
        let c = centre(cur.min, cur.max);
        let reach = (cur.max[0] - cur.min[0]).max(cur.max[1] - cur.min[1]) / 2.0 + NEAR / 3.0;
        let best = tag_grid
            .near(c, reach)
            .into_iter()
            .filter(|&i| !tag_taken.get(i).copied().unwrap_or(true))
            .filter_map(|i| loose_tags.get(i).map(|(_, p)| (i, *p)))
            .filter(|(_, p)| inside_rect(*p, cur.min, cur.max, NEAR / 3.0))
            .min_by(|a, b| dist(a.1, c).total_cmp(&dist(b.1, c)).then(a.0.cmp(&b.0)));
        if let Some((i, _)) = best
            && let (Some(taken), Some(t)) = (tag_taken.get_mut(i), loose_tags.get(i))
        {
            *taken = true;
            cur.tag = Some(t.clone());
        }
    }
    let note_grid = Grid::new(loose_notes.iter().map(|(_, p)| *p));
    let mut note_taken = vec![false; loose_notes.len()];
    for cur in drafts.iter_mut().filter(|c| c.note.is_none()) {
        // The note nearest the item's own tag (or centre).
        let from = cur.tag.as_ref().map_or_else(|| centre(cur.min, cur.max), |(_, p)| *p);
        let best = note_grid
            .near(from, NEAR)
            .into_iter()
            .filter(|&i| !note_taken.get(i).copied().unwrap_or(true))
            .filter_map(|i| loose_notes.get(i).map(|(_, p)| (i, *p)))
            .filter(|(_, p)| dist(*p, from) <= NEAR)
            .min_by(|a, b| dist(a.1, from).total_cmp(&dist(b.1, from)).then(a.0.cmp(&b.0)));
        if let Some((i, _)) = best
            && let (Some(taken), Some(n)) = (note_taken.get_mut(i), loose_notes.get(i))
        {
            *taken = true;
            cur.note = Some(n.0);
        }
    }

    drafts
        .into_iter()
        .map(|dr| {
            let (w, h) = (dr.max[0] - dr.min[0], dr.max[1] - dr.min[1]);
            let has_x = dr.diagonals.len() == 2 && dr.diagonals.iter().all(|(a, b)| (b[0] - a[0]).abs() > w * 0.6 && (b[1] - a[1]).abs() > h * 0.6);
            let pattern = if has_x {
                "appliance"
            } else if dr.inner && dr.diagonals.is_empty() {
                "upper_cabinet"
            } else if !dr.diagonals.is_empty() {
                "base_cabinet"
            } else {
                "counter"
            };
            let tag = dr.tag.map(|(t, _)| t).unwrap_or_default();
            let row = rows.get(&norm_tag(&tag)).filter(|_| !tag.is_empty());
            let short = w.min(h);
            match row {
                Some(r) if !r.kind.is_empty() => DetectedCasework {
                    min_in: dr.min,
                    max_in: dr.max,
                    kind: r.kind.clone(),
                    tag,
                    height_in: r.height_in.or(dr.note).unwrap_or(0.0),
                    depth_in: r.depth_in.unwrap_or(short),
                    description: r.description.clone(),
                    spec_section: r.spec_section.clone(),
                    status: "schedule".into(),
                },
                _ => {
                    let kind = if tag.is_empty() || has_x { pattern } else { classify_by_tag(&tag, pattern) };
                    DetectedCasework {
                        min_in: dr.min,
                        max_in: dr.max,
                        kind: kind.to_string(),
                        status: if dr.note.is_some() && !tag.is_empty() { "note" } else { "pattern" }.into(),
                        tag,
                        height_in: dr.note.unwrap_or(0.0),
                        depth_in: short,
                        description: String::new(),
                        spec_section: None,
                    }
                }
            }
        })
        .collect()
}

/// The CASEWORK layer's rectangles, hatch lines, tags and AFF notes (the notes may sit on the
/// TEXT layer), in drawing order.
fn casework_pieces(d: &Drawing) -> Vec<Piece> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if out.len() >= MAX_ITEMS * 8 {
            break;
        }
        let layer = &e.common.layer;
        let on_casework = layer.eq_ignore_ascii_case("CASEWORK");
        if !on_casework && !layer.eq_ignore_ascii_case("TEXT") {
            continue;
        }
        match &e.kind {
            EntityKind::LwPolyline(p) if on_casework => {
                if !p.closed || p.vertices.len() != 4 {
                    continue;
                }
                let pts: Vec<[f64; 2]> = p.vertices.iter().map(|v| [v.p.x, v.p.y]).collect();
                if pts.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
                    continue;
                }
                let (min, max) = bounds(&pts);
                out.push(Piece::Rect(min, max));
            }
            EntityKind::Line(l) if on_casework => {
                let a = [l.a.x, l.a.y];
                let b = [l.b.x, l.b.y];
                if a[0].is_finite() && a[1].is_finite() && b[0].is_finite() && b[1].is_finite() {
                    out.push(Piece::Line(a, b));
                }
            }
            EntityKind::Text(_) | EntityKind::MText(_) => {
                let Some((s, at)) = text_of(&e.kind) else { continue };
                if let Some(h) = parse_height_note(&s) {
                    out.push(Piece::Note(h, at));
                } else if on_casework && is_tag(&s) {
                    out.push(Piece::Tag(s.trim().to_string(), at));
                }
            }
            _ => {}
        }
    }
    out
}

/// For each rectangle: whether it is drawn inside another with the same inset on every side
/// (at most a foot), i.e. that one's decoration.
fn decorations(rects: &[([f64; 2], [f64; 2])]) -> Vec<bool> {
    let grid = Grid::new(rects.iter().map(|(min, _)| *min));
    rects
        .iter()
        .enumerate()
        .map(|(i, (min, max))| {
            grid.near(*min, 12.0 * std::f64::consts::SQRT_2 + 1.0).into_iter().filter(|&j| j != i).any(|j| {
                let Some((omin, omax)) = rects.get(j) else { return false };
                let ins = [min[0] - omin[0], min[1] - omin[1], omax[0] - max[0], omax[1] - max[1]];
                let lo = ins.iter().copied().fold(f64::INFINITY, f64::min);
                let hi = ins.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                lo > 0.2 && hi <= 12.0 && hi - lo < 0.5
            })
        })
        .collect()
}

// ---- Plumbing fixtures ---------------------------------------------------------------------

/// Grows a symbol's bounds by `pts` (non-finite points are left out).
fn grow(sym: &mut Option<([f64; 2], [f64; 2])>, pts: &[[f64; 2]]) {
    if pts.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
        return;
    }
    let (a, b) = bounds(pts);
    *sym = Some(match *sym {
        Some((lo, hi)) => ([lo[0].min(a[0]), lo[1].min(a[1])], [hi[0].max(b[0]), hi[1].max(b[1])]),
        None => (a, b),
    });
}

fn read_fixtures(d: &Drawing, rows: &HashMap<String, &ScheduleRow>) -> Vec<DetectedFixture> {
    let mut out = Vec::new();
    // The symbol drawn since the last tag: its bounds.
    let mut sym: Option<([f64; 2], [f64; 2])> = None;
    for e in d.model.iter() {
        if out.len() >= MAX_ITEMS {
            break;
        }
        if !e.common.layer.to_ascii_uppercase().contains("FIXT") {
            continue;
        }
        match &e.kind {
            EntityKind::Line(l) => grow(&mut sym, &[[l.a.x, l.a.y], [l.b.x, l.b.y]]),
            EntityKind::LwPolyline(p) => {
                let pts: Vec<[f64; 2]> = p.vertices.iter().map(|v| [v.p.x, v.p.y]).collect();
                if !pts.is_empty() {
                    grow(&mut sym, &pts);
                }
            }
            EntityKind::Circle(c) if c.radius.is_finite() && c.radius > 0.0 && c.radius < MAX_SIDE => {
                grow(&mut sym, &[[c.center.x - c.radius, c.center.y - c.radius], [c.center.x + c.radius, c.center.y + c.radius]]);
            }
            EntityKind::Arc(c) if c.radius.is_finite() && c.radius > 0.0 && c.radius < MAX_SIDE => {
                grow(&mut sym, &[[c.center.x - c.radius, c.center.y - c.radius], [c.center.x + c.radius, c.center.y + c.radius]]);
            }
            EntityKind::Text(_) | EntityKind::MText(_) => {
                let Some((s, at)) = text_of(&e.kind) else { continue };
                if !is_tag(&s) || !at[0].is_finite() || !at[1].is_finite() {
                    continue;
                }
                let tag = s.trim().to_string();
                // The symbol drawn just before the tag, when the tag is on or beside it.
                let at_in = match sym.take() {
                    Some((lo, hi)) if inside_rect(at, lo, hi, NEAR / 3.0) && (hi[0] - lo[0]) <= MAX_SIDE && (hi[1] - lo[1]) <= MAX_SIDE => {
                        centre(lo, hi)
                    }
                    _ => at,
                };
                let row = rows.get(&norm_tag(&tag));
                let (kind, status) = match row {
                    Some(r) if !r.kind.is_empty() => (r.kind.clone(), "schedule"),
                    _ => (fixture_kind_by_tag(&tag).to_string(), "tag"),
                };
                if kind.is_empty() {
                    continue;
                }
                out.push(DetectedFixture {
                    at_in,
                    kind,
                    tag,
                    height_in: row.and_then(|r| r.height_in).unwrap_or(0.0),
                    description: row.map(|r| r.description.clone()).unwrap_or_default(),
                    spec_section: row.and_then(|r| r.spec_section.clone()),
                    status: status.into(),
                });
            }
            _ => {}
        }
    }
    out
}

/// A fixture's kind from its tag's prefix (`WC-1`, `LAV`, `UR-1`…), or "" when it names none.
pub fn fixture_kind_by_tag(tag: &str) -> &'static str {
    let t = tag.trim().to_ascii_uppercase();
    let p: String = t.chars().take_while(char::is_ascii_alphabetic).collect();
    match p.as_str() {
        "WC" => "water_closet",
        "LAV" | "L" => "lavatory",
        "S" | "SK" | "SNK" => "sink",
        "UR" | "U" => "urinal",
        "SH" | "SHW" => "shower",
        "MS" | "MOP" => "mop_sink",
        "TUB" | "BT" => "bathtub",
        "FD" => "floor_drain",
        "WH" | "EWH" => "water_heater",
        _ => "",
    }
}

// ---- The schedule ------------------------------------------------------------------------

/// The casework / plumbing fixture schedule: rows under a header with a `TAG` column, on
/// [`SCHEDULE_LAYER`] (any layer named like `*CASEWORK*SCHED*`).
pub fn read_schedule(d: &Drawing) -> Vec<ScheduleRow> {
    let mut cells: Vec<(String, [f64; 2], f64)> = Vec::new();
    for e in d.model.iter() {
        if cells.len() >= MAX_ITEMS {
            break;
        }
        let l = e.common.layer.to_ascii_uppercase();
        if !(l.contains("CASEWORK") && l.contains("SCHED")) {
            continue;
        }
        let h = match &e.kind {
            EntityKind::Text(t) => t.height,
            EntityKind::MText(t) => t.height,
            _ => continue,
        };
        if let Some((s, at)) = text_of(&e.kind)
            && at[0].is_finite()
            && at[1].is_finite()
            && h.is_finite()
            && h > 0.0
        {
            cells.push((s.trim().to_string(), at, h));
        }
    }
    // Rows: cells on one baseline, left to right; rows top to bottom.
    cells.sort_by(|a, b| b.1[1].total_cmp(&a.1[1]).then(a.1[0].total_cmp(&b.1[0])));
    let mut lines: Vec<Vec<(String, f64)>> = Vec::new();
    let mut last_y = f64::NAN;
    for (s, at, h) in cells {
        if lines.is_empty() || (last_y - at[1]).abs() > h * 0.5 {
            lines.push(Vec::new());
            last_y = at[1];
        }
        if let Some(l) = lines.last_mut() {
            l.push((s, at[0]));
        }
    }
    let Some(head_i) = lines.iter().position(|l| l.iter().any(|(s, _)| s.eq_ignore_ascii_case("TAG"))) else { return Vec::new() };
    let Some(head) = lines.get(head_i) else { return Vec::new() };
    let mut cols: Vec<(String, f64)> = head.iter().map(|(s, x)| (s.to_ascii_uppercase(), *x)).collect();
    cols.sort_by(|a, b| a.1.total_cmp(&b.1));
    let col_of = |x: f64| -> Option<&str> {
        // The header whose column starts at or left of the cell (a little slack).
        let slack = cols.windows(2).filter_map(|w| Some(w.get(1)?.1 - w.first()?.1)).fold(f64::INFINITY, f64::min).min(1e9) * 0.25;
        cols.iter().rev().find(|(_, cx)| *cx <= x + slack).map(|(n, _)| n.as_str())
    };
    let mut out = Vec::new();
    for line in lines.iter().skip(head_i + 1) {
        let mut row = ScheduleRow::default();
        for (s, x) in line {
            match col_of(*x) {
                Some("TAG") => row.tag = s.clone(),
                Some(c) if c.starts_with("DESC") => row.description = s.clone(),
                Some(c) if c.starts_with("SIZE") => {
                    let (w, dp) = parse_size(s);
                    row.width_in = w;
                    row.depth_in = dp;
                }
                Some(c) if c.starts_with("HEIGHT") || c.starts_with("MTG") || c.starts_with("MOUNT") => {
                    row.height_in = parse_height_note(s).or_else(|| parse_inches(s))
                }
                Some(c) if c.starts_with("SPEC") => row.spec_section = Some(s.clone()).filter(|v| v != "-" && !v.is_empty()),
                _ => {}
            }
        }
        if row.tag.is_empty() || !is_tag(&row.tag) {
            continue;
        }
        row.kind = kind_from_description(&row.description).to_string();
        if out.len() < MAX_ITEMS {
            out.push(row);
        }
    }
    out
}

/// The schedule description for a kind (what the drawing prints).
pub fn description_for(kind: &str) -> &'static str {
    match kind {
        "base_cabinet" => "BASE CABINET",
        "upper_cabinet" => "UPPER CABINET",
        "counter" => "COUNTERTOP",
        "vanity" => "VANITY",
        "backsplash" => "BACKSPLASH",
        "appliance" => "APPLIANCE, REFRIGERATOR",
        "wardrobe" => "WARDROBE",
        "water_closet" => "WATER CLOSET",
        "lavatory" => "LAVATORY",
        "sink" => "SINK, STAINLESS STEEL",
        "urinal" => "URINAL",
        "shower" => "SHOWER",
        "mop_sink" => "MOP SINK",
        "bathtub" => "BATHTUB",
        "floor_drain" => "FLOOR DRAIN",
        "water_heater" => "WATER HEATER",
        _ => "",
    }
}

/// The kind a schedule description names: the first of these words it contains.
pub fn kind_from_description(desc: &str) -> &'static str {
    let d = desc.to_ascii_uppercase();
    const WORDS: [(&str, &str); 22] = [
        ("MOP SINK", "mop_sink"),
        ("SERVICE SINK", "mop_sink"),
        ("WATER CLOSET", "water_closet"),
        ("URINAL", "urinal"),
        ("SHOWER", "shower"),
        ("BATHTUB", "bathtub"),
        ("WATER HEATER", "water_heater"),
        ("FLOOR DRAIN", "floor_drain"),
        ("BASE CABINET", "base_cabinet"),
        ("UPPER CABINET", "upper_cabinet"),
        ("WALL CABINET", "upper_cabinet"),
        ("COUNTER", "counter"),
        ("VANITY", "vanity"),
        ("BACKSPLASH", "backsplash"),
        ("WARDROBE", "wardrobe"),
        ("REFRIGERATOR", "appliance"),
        ("DISHWASHER", "appliance"),
        ("MICROWAVE", "appliance"),
        ("APPLIANCE", "appliance"),
        ("CREDENZA", "base_cabinet"),
        ("LAVATORY", "lavatory"),
        ("SINK", "sink"),
    ];
    WORDS.iter().find(|(w, _)| d.contains(w)).map_or("", |(_, k)| k)
}

/// `24" x 24"`, `VARIES x 24"` → (width, depth) in inches.
fn parse_size(s: &str) -> (Option<f64>, Option<f64>) {
    let u = s.to_ascii_uppercase();
    let mut parts = u.split(" X ");
    let w = parts.next().and_then(parse_inches);
    let d = parts.next().and_then(parse_inches);
    (w, d)
}

/// `24"`, `34.5"`, `2'-6"` → inches.
fn parse_inches(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some((ft, rest)) = s.split_once('\'') {
        let f: f64 = ft.trim().parse().ok()?;
        let i = rest.trim_matches(|c: char| c == '-' || c == '"' || c.is_whitespace());
        let i: f64 = if i.is_empty() { 0.0 } else { i.parse().ok()? };
        let v = f * 12.0 + i;
        return (v.is_finite() && v > 0.0 && v < 1e5).then_some(v);
    }
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    let v: f64 = digits.parse().ok()?;
    (s[digits.len()..].trim_start().starts_with('"') && v.is_finite() && v > 0.0 && v < 1e5).then_some(v)
}

// ---- Scoring -----------------------------------------------------------------------------

/// Convert detected casework to found items for scoring.
pub fn to_found(items: &[DetectedCasework], sh: &Sheet) -> Vec<score::FoundCaseworkItem> {
    let level = sh.level.clone().unwrap_or_default();
    items
        .iter()
        .map(|it| {
            let min_ft = score::building_from_model(sh, it.min_in);
            let max_ft = score::building_from_model(sh, it.max_in);
            let (lo, hi) = ([min_ft[0].min(max_ft[0]), min_ft[1].min(max_ft[1])], [min_ft[0].max(max_ft[0]), min_ft[1].max(max_ft[1])]);
            score::FoundCaseworkItem {
                level: level.clone(),
                kind: it.kind.clone(),
                tag: it.tag.clone(),
                min_ft: lo,
                max_ft: hi,
                height_in: it.height_in,
            }
        })
        .collect()
}

/// Read casework from a drawing and score against truth.
pub fn read_and_score(d: &Drawing, t: &Truth, sh: &Sheet) -> score::CaseworkScore {
    let items = read_casework(d);
    let found = to_found(&items, sh);
    let level = sh.level.clone().unwrap_or_default();
    score::score_casework(t, &level, &found, 3.0)
}

/// A plumbing fixture a reader found (building feet).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FoundFixture {
    pub level: String,
    pub kind: String,
    pub tag: String,
    pub at_ft: [f64; 2],
}

/// Score for the fixture reader.
#[derive(Clone, Debug, Default)]
pub struct FixtureScore {
    pub true_positives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
    pub precision: f64,
    pub recall: f64,
    /// Of matched fixtures, share with the right kind.
    pub kind_accuracy: f64,
    /// Mean centre error of matched fixtures, feet.
    pub position_mae_ft: f64,
}

/// Detected fixtures in building feet.
pub fn fixtures_to_found(items: &[DetectedFixture], sh: &Sheet) -> Vec<FoundFixture> {
    let level = sh.level.clone().unwrap_or_default();
    items
        .iter()
        .map(|f| FoundFixture { level: level.clone(), kind: f.kind.clone(), tag: f.tag.clone(), at_ft: score::building_from_model(sh, f.at_in) })
        .collect()
}

/// The fixtures of a level an A-plan shows: the plumbing fixtures and the water heaters
/// (kind, tag, plan centre in building feet).
pub fn truth_fixtures(t: &Truth, level: &str) -> Vec<(String, String, [f64; 2])> {
    let mut v: Vec<(String, String, [f64; 2])> =
        t.fixtures.iter().filter(|f| f.level == level).map(|f| (f.kind.clone(), f.tag.clone(), [f.at_ft[0], f.at_ft[1]])).collect();
    v.extend(
        t.equipment
            .iter()
            .filter(|e| e.level == level && e.kind == "water_heater")
            .map(|e| (e.kind.clone(), e.tag.clone(), [(e.min_ft[0] + e.max_ft[0]) / 2.0, (e.min_ft[1] + e.max_ft[1]) / 2.0])),
    );
    v
}

/// Score found fixtures against the truth's fixtures and water heaters on one level: each found
/// fixture takes the nearest unmatched truth fixture within `tol_ft`, the same tag first.
pub fn score_fixtures(t: &Truth, level: &str, found: &[FoundFixture], tol_ft: f64) -> FixtureScore {
    let truth = truth_fixtures(t, level);
    let mut matched = vec![false; truth.len()];
    let (mut tp, mut fp, mut kind_ok, mut pos) = (0usize, 0usize, 0usize, 0.0f64);
    for f in found.iter().filter(|f| f.level == level) {
        let best = truth
            .iter()
            .enumerate()
            .filter(|(i, _)| !matched.get(*i).copied().unwrap_or(true))
            .map(|(i, (_, tag, at))| (i, dist(*at, f.at_ft), !tag.eq_ignore_ascii_case(&f.tag)))
            .filter(|(_, d, _)| *d < tol_ft)
            .min_by(|a, b| a.2.cmp(&b.2).then(a.1.total_cmp(&b.1)));
        match best {
            Some((i, d, _)) => {
                if let Some(m) = matched.get_mut(i) {
                    *m = true;
                }
                tp += 1;
                pos += d;
                if truth.get(i).is_some_and(|(k, _, _)| *k == f.kind) {
                    kind_ok += 1;
                }
            }
            None => fp += 1,
        }
    }
    let fn_ = truth.len() - tp;
    let ratio = |a: usize, b: usize| if b > 0 { a as f64 / b as f64 } else { 0.0 };
    FixtureScore {
        true_positives: tp,
        false_positives: fp,
        false_negatives: fn_,
        precision: ratio(tp, tp + fp),
        recall: ratio(tp, tp + fn_),
        kind_accuracy: ratio(kind_ok, tp),
        position_mae_ft: if tp > 0 { pos / tp as f64 } else { 0.0 },
    }
}

/// What a reading puts in the walls, for [`crate::clash::casework_clashes`]: its sinks (counters
/// with a sink, sink fixtures), dishwashers and the fixtures with rough-in rules, in building
/// feet. Ids are `<sheet>:<tag>#<n>` (`n` counts the reading's items from 1).
pub fn plumbing_items(r: &Reading, sh: &Sheet) -> Vec<crate::clash::ReadPlumbing> {
    let level = sh.level.clone().unwrap_or_default();
    let mut out = Vec::new();
    let found = to_found(&r.casework, sh);
    for (i, (c, f)) in r.casework.iter().zip(found.iter()).enumerate() {
        let kind = if c.is_dishwasher() {
            "dishwasher"
        } else if c.is_sink() {
            "sink"
        } else {
            continue;
        };
        out.push(crate::clash::ReadPlumbing {
            id: format!("{}:{}#{}", sh.number, c.tag, i + 1),
            kind: kind.into(),
            tag: c.tag.clone(),
            level: level.clone(),
            min_ft: f.min_ft,
            max_ft: f.max_ft,
        });
    }
    let n = r.casework.len();
    for (i, f) in r.fixtures.iter().enumerate() {
        if !matches!(f.kind.as_str(), "sink" | "lavatory" | "water_closet" | "urinal" | "shower" | "mop_sink") {
            continue;
        }
        let at = score::building_from_model(sh, f.at_in);
        out.push(crate::clash::ReadPlumbing {
            id: format!("{}:{}#{}", sh.number, f.tag, n + i + 1),
            kind: f.kind.clone(),
            tag: f.tag.clone(),
            level: level.clone(),
            min_ft: [at[0] - 1.0, at[1] - 1.0],
            max_ft: [at[0] + 1.0, at[1] + 1.0],
        });
    }
    out
}

// ---- Internal helpers ------------------------------------------------------------------

/// A text entity's content and anchor point.
fn text_of(k: &EntityKind) -> Option<(String, [f64; 2])> {
    match k {
        EntityKind::Text(t) => {
            let pt = t.align_pt.map_or([t.insert.x, t.insert.y], |p| [p.x, p.y]);
            Some((t.value.clone(), pt))
        }
        EntityKind::MText(t) => Some((t.contents.clone(), [t.insert.x, t.insert.y])),
        _ => None,
    }
}

/// A tag: 1–4 letters, then optionally a number (`BC-1`, `V-2`, `WC`, `LAV-1`, `UC1A`).
fn is_tag(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.len() > 8 {
        return false;
    }
    let b = s.as_bytes();
    let letters = b.iter().take_while(|c| c.is_ascii_uppercase()).count();
    if !(1..=4).contains(&letters) {
        return false;
    }
    let rest = b.get(letters..).unwrap_or_default();
    let rest = rest.strip_prefix(b"-").unwrap_or(rest);
    if rest.is_empty() {
        return b.len() == letters;
    }
    let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    (1..=3).contains(&digits) && rest.get(digits..).is_some_and(|t| t.is_empty() || (t.len() == 1 && t.iter().all(u8::is_ascii_uppercase)))
}

/// A tag compared loosely: upper case, no dash or spaces.
fn norm_tag(t: &str) -> String {
    t.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

fn bounds(pts: &[[f64; 2]]) -> ([f64; 2], [f64; 2]) {
    let min = [pts.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min), pts.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min)];
    let max = [pts.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max), pts.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max)];
    (min, max)
}

fn centre(min: [f64; 2], max: [f64; 2]) -> [f64; 2] {
    [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0]
}

fn area(d: &Draft) -> f64 {
    (d.max[0] - d.min[0]) * (d.max[1] - d.min[1])
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Is `p` inside the rectangle (with tolerance)?
fn contains(min: [f64; 2], max: [f64; 2], p: [f64; 2], tol: f64) -> bool {
    inside_rect(p, min, max, tol)
}

/// Is a point inside a rectangle (with tolerance)?
fn inside_rect(p: [f64; 2], min: [f64; 2], max: [f64; 2], tol: f64) -> bool {
    p[0] >= min[0] - tol && p[0] <= max[0] + tol && p[1] >= min[1] - tol && p[1] <= max[1] + tol
}

/// Is a line segment roughly axis-aligned?
fn is_axis_aligned(a: [f64; 2], b: [f64; 2]) -> bool {
    let dx = (b[0] - a[0]).abs();
    let dy = (b[1] - a[1]).abs();
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-6 {
        return true;
    }
    dx / len < 0.05 || dy / len < 0.05
}

/// Parse a height note like "34\" AFF" or "34\"" → height in inches.
fn parse_height_note(s: &str) -> Option<f64> {
    let s = s.trim().to_uppercase();
    // Match patterns: "34\" AFF", "34\"", "34 AFF"
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    if digits.is_empty() {
        return None;
    }
    let val: f64 = digits.parse().ok()?;
    if val > 0.0 && val <= 120.0 && (s.contains("AFF") || s.contains('"') || s.contains('\"')) { Some(val) } else { None }
}

/// Classify kind by tag prefix when the hatch pattern alone is ambiguous.
fn classify_by_tag<'a>(tag: &str, default: &'a str) -> &'a str {
    let t = tag.to_uppercase();
    if t.starts_with("BC") || t.starts_with("CR") {
        "base_cabinet"
    } else if t.starts_with("UC") {
        "upper_cabinet"
    } else if t.starts_with("CT") || t.starts_with("SK") || t.starts_with("KC") || t.starts_with("FD") {
        "counter"
    } else if t.starts_with('V') {
        "vanity"
    } else if t.starts_with("WR") {
        "wardrobe"
    } else if t.starts_with("RF") || t.starts_with("MW") || t.starts_with("DW") || t.starts_with("AP") {
        "appliance"
    } else if t.starts_with("BS") {
        "backsplash"
    } else {
        default
    }
}

/// Points bucketed in [`CELL`] squares, for "what is near here".
struct Grid {
    cells: HashMap<(i64, i64), Vec<usize>>,
}

impl Grid {
    fn new(pts: impl Iterator<Item = [f64; 2]>) -> Grid {
        let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, p) in pts.enumerate() {
            if let Some(c) = cell(p) {
                cells.entry(c).or_default().push(i);
            }
        }
        Grid { cells }
    }

    /// The points within `r` of `p` (maybe a few more), in index order.
    fn near(&self, p: [f64; 2], r: f64) -> Vec<usize> {
        let (Some(lo), Some(hi)) = (cell([p[0] - r, p[1] - r]), cell([p[0] + r, p[1] + r])) else { return Vec::new() };
        // A huge reach is capped: what lies further than ~100 cells is not "near".
        let mut out = Vec::new();
        for x in lo.0.max(hi.0 - 100)..=hi.0 {
            for y in lo.1.max(hi.1 - 100)..=hi.1 {
                if let Some(v) = self.cells.get(&(x, y)) {
                    out.extend_from_slice(v);
                }
            }
        }
        out.sort_unstable();
        out
    }
}

fn cell(p: [f64; 2]) -> Option<(i64, i64)> {
    let (x, y) = ((p[0] / CELL).floor(), (p[1] / CELL).floor());
    (x.is_finite() && y.is_finite() && x.abs() < 1e12 && y.abs() < 1e12).then_some((x as i64, y as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_height_note_basic() {
        assert_eq!(parse_height_note("34\" AFF"), Some(34.0));
        assert_eq!(parse_height_note("36\""), Some(36.0));
        assert_eq!(parse_height_note("HELLO"), None);
        assert_eq!(parse_height_note(""), None);
    }

    #[test]
    fn classify_by_tag_prefixes() {
        assert_eq!(classify_by_tag("BC-1", "counter"), "base_cabinet");
        assert_eq!(classify_by_tag("UC-1", "counter"), "upper_cabinet");
        assert_eq!(classify_by_tag("V-1", "counter"), "vanity");
        assert_eq!(classify_by_tag("CT-1", "base_cabinet"), "counter");
        assert_eq!(classify_by_tag("RF-1", "counter"), "appliance");
        assert_eq!(classify_by_tag("BS-1", "counter"), "backsplash");
        assert_eq!(classify_by_tag("WR-1", "counter"), "wardrobe");
        assert_eq!(classify_by_tag("UNKNOWN", "counter"), "counter");
    }

    #[test]
    fn inside_rect_works() {
        assert!(inside_rect([5.0, 5.0], [0.0, 0.0], [10.0, 10.0], 0.0));
        assert!(!inside_rect([15.0, 5.0], [0.0, 0.0], [10.0, 10.0], 0.0));
        assert!(inside_rect([10.5, 5.0], [0.0, 0.0], [10.0, 10.0], 1.0));
    }

    #[test]
    fn is_axis_aligned_works() {
        assert!(is_axis_aligned([0.0, 0.0], [10.0, 0.0]));
        assert!(is_axis_aligned([0.0, 0.0], [0.0, 10.0]));
        assert!(!is_axis_aligned([0.0, 0.0], [10.0, 10.0]));
    }

    #[test]
    fn tags_and_sizes() {
        for t in ["BC-1", "V-2", "WC", "LAV-1", "UC1A", "TUB", "WH-1"] {
            assert!(is_tag(t), "{t}");
        }
        for t in ["34\" AFF", "BREAK ROOM", "", "-1", "ABCDE-1", "BC-1234", "bc-1", "1A"] {
            assert!(!is_tag(t), "{t}");
        }
        assert_eq!(parse_size("VARIES x 24\""), (None, Some(24.0)));
        assert_eq!(parse_size("24\" x 22\""), (Some(24.0), Some(22.0)));
        assert_eq!(parse_inches("2'-6\""), Some(30.0));
        assert_eq!(parse_inches("34.5\""), Some(34.5));
        assert_eq!(parse_inches("-"), None);
        assert_eq!(kind_from_description("COUNTERTOP WITH SINK"), "counter");
        assert_eq!(kind_from_description("MOP SINK"), "mop_sink");
        assert_eq!(kind_from_description("SINK, STAINLESS STEEL"), "sink");
        for k in [
            "base_cabinet",
            "upper_cabinet",
            "counter",
            "vanity",
            "backsplash",
            "appliance",
            "wardrobe",
            "water_closet",
            "lavatory",
            "sink",
            "urinal",
            "shower",
            "mop_sink",
            "bathtub",
            "floor_drain",
            "water_heater",
        ] {
            assert_eq!(kind_from_description(description_for(k)), k, "{k}");
        }
        assert_eq!(fixture_kind_by_tag("WC-1"), "water_closet");
        assert_eq!(fixture_kind_by_tag("LAV"), "lavatory");
        assert_eq!(fixture_kind_by_tag("ZZ-1"), "");
    }
}
