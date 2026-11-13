//! The objects the architect draws, built and completed by the electrical sheets (Q6612): a light
//! on a reflected ceiling plan (a receptacle or a data outlet on a floor plan, a fan) is an object
//! of the building like any other drawn one, as a standing part of every job's build (nothing to
//! turn on). The finding is what the build cannot complete, never a side comparison.
//!
//! - The architect's objects are read off the architect's own sheets by their legends: each row
//!   of a titled legend (`RCP LEGEND`) whose words name a kind of [`crossdisc.json`] (rule data)
//!   is a drawing looked for on the architect's sheets of the set; each copy found is placed on its
//!   level's plan.
//! - Each must then be completed the way an installer must complete it (the kind's `requires`: a
//!   luminaire's type, circuit and control). Its completing object is the electrical sheets'
//!   object of its kind by it in its room (the job's read devices): that one is built with its
//!   type, circuit and control, and the architect's copy of it adds nothing.
//! - What no electrical sheet completes is the finding, raised in the build on those objects: an
//!   RFI per group (both sheets, the room, both counts, where, a crop of the architect's sheet)
//!   and a line of the job report. The build does not stop: the objects are built (the kind's
//!   `build` rule), inferred and unsure, completed by the next source (trade practice: the
//!   nearest light's type, the nearest circuit of the kind, that light's switch leg or a switch at
//!   the door) as a proposal with its basis, so the takeoff and the estimate carry them and the
//!   RFI asks.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::rfi::{Rfi, RfiKind, RfiSide, Severity};
use serde::Deserialize;

use super::unitmarks::{Piece, Template};

const RULES: &str = include_str!("crossdisc.json");

/// Pieces of one sheet looked at, at most.
pub(crate) const MAX_SHEET_PIECES: usize = 600_000;
/// Objects of one legend drawing looked at, at most.
const MAX_ROW_PIECES: usize = 64;
/// The scales a legend's drawing is looked for at on a plan, of its own size: the matcher takes
/// each within about a third either way, so these cover a fifth to three times its size.
const SCALES: [f64; 7] = [2.0, 1.4, 1.0, 0.7, 0.5, 0.35, 0.25];
/// Legend headings looked at on one sheet, at most.
const MAX_LEGENDS: usize = 12;
/// Objects compared per kind, at most.
const MAX_OBJECTS: usize = 200_000;
/// Rooms compared, at most.
const MAX_ROOMS: usize = 20_000;
/// Rooms one finding names, at most.
const MAX_ROOMS_NAMED: usize = 12;

/// The rules ([`crossdisc.json`]).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub architect_disciplines: Vec<String>,
    pub architect_sheet_kinds: Vec<String>,
    pub legend_title_words: Vec<String>,
    pub legend_title_max_words: usize,
    pub legend_rows_h: f64,
    pub legend_gap_h: f64,
    pub legend_drawing_h: f64,
    pub legend_row_max_words: usize,
    pub kinds: Vec<KindRule>,
    pub rooms: RoomRules,
    pub max_findings: usize,
    pub max_sketches: usize,
}

/// One compared kind.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct KindRule {
    pub kind: String,
    /// In plain words, plural (`lights`).
    pub name: String,
    pub legend_any: Vec<String>,
    pub legend_none: Vec<String>,
    pub engineer_disciplines: Vec<String>,
    pub engineer_kinds: Vec<String>,
    pub engineer_ifc: Vec<String>,
    pub engineer_kinds_none: Vec<String>,
    pub reach_ft: f64,
    /// Objects no electrical sheet completes this near each other in one room are one finding.
    pub cluster_ft: f64,
    /// What an object of the kind needs before it can be installed (`fixture type`, `circuit`,
    /// `control`): what no electrical sheet gives is the finding.
    pub requires: Vec<String>,
    /// How much an object no electrical sheet completes matters (`high`, `medium`, `low`).
    pub open_severity: String,
    pub build: Option<BuildRule>,
}

/// What the build makes of an object no electrical sheet completes.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct BuildRule {
    pub mark: String,
    pub kind: String,
    pub ifc: String,
    pub proposed_control: String,
    pub proposed_circuit: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RoomRules {
    pub reach_ft: f64,
}

pub(crate) fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

fn upper_padded(words: &str) -> String {
    format!(" {} ", words.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase())
}

impl KindRule {
    /// Whether a legend row's words name this kind.
    pub(crate) fn names(&self, words: &str) -> bool {
        let u = upper_padded(words);
        self.legend_any.iter().any(|w| u.contains(&w.to_ascii_uppercase()))
            && !self.legend_none.iter().any(|w| !w.trim().is_empty() && u.contains(&w.to_ascii_uppercase()))
    }

    /// Whether an engineer's read device (its kind and IFC class) is of this kind.
    pub(crate) fn is_engineers(&self, kind: &str, ifc: &str) -> bool {
        let k = kind.trim().to_ascii_lowercase();
        if self.engineer_kinds_none.iter().any(|w| !w.trim().is_empty() && k.contains(&w.to_ascii_lowercase())) {
            return false;
        }
        self.engineer_kinds.iter().any(|w| !w.trim().is_empty() && k.contains(&w.to_ascii_lowercase()))
            || self.engineer_ifc.iter().any(|c| !c.trim().is_empty() && c.eq_ignore_ascii_case(ifc.trim()))
    }

    /// Whether a sheet of `discipline` is one of this kind's engineers'.
    pub(crate) fn engineers_sheet(&self, discipline: &str) -> bool {
        self.engineer_disciplines.iter().any(|d| d.eq_ignore_ascii_case(discipline.trim()))
    }

    fn cluster(&self) -> f64 {
        if self.cluster_ft.is_finite() && self.cluster_ft > 0.0 { self.cluster_ft } else { 15.0 }
    }

    fn reach(&self) -> f64 {
        if self.reach_ft.is_finite() && self.reach_ft > 0.0 { self.reach_ft } else { 4.0 }
    }
}

impl Rules {
    /// Whether a sheet of `discipline` is the architect's.
    pub(crate) fn architects(&self, discipline: &str) -> bool {
        self.architect_disciplines.iter().any(|d| d.eq_ignore_ascii_case(discipline.trim()))
    }

    /// Whether the architect's sheets of `kind` are read for objects (a legend sheet only lends
    /// its rows).
    pub(crate) fn read_kind(&self, kind: &str) -> bool {
        self.architect_sheet_kinds.iter().any(|k| k.eq_ignore_ascii_case(kind.trim()))
    }

    /// The kind a legend row's words name (the first rule that names them).
    pub(crate) fn kind_of_row(&self, words: &str) -> Option<usize> {
        self.kinds.iter().position(|k| k.names(words))
    }
}

// ─── The architect's legends ────────────────────────────────────────────────

/// One row of an architect's legend: its words (wrapped lines joined), the kind rule naming them,
/// its drawing (sheet units), the height of its lettering and the legend's box.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LegendRow {
    pub words: String,
    pub kind: usize,
    pub pieces: Vec<Piece>,
    pub text_h: f64,
    pub legend: [f64; 4],
}

/// What a sheet's legends hold: the rows of the compared kinds, and every legend's box (left out
/// of the search whatever its rows).
#[derive(Clone, Debug, Default)]
pub(crate) struct Legends {
    pub rows: Vec<LegendRow>,
    pub boxes: Vec<[f64; 4]>,
    /// Rows whose words name a compared kind with no drawing read for them (none in their cell, or
    /// too many pieces): the kind rule's index and the words. The architect's count of that kind
    /// is partial.
    pub unread: Vec<(usize, String)>,
}

fn piece_box(p: &Piece) -> [f64; 4] {
    match *p {
        Piece::Seg(a, b) => [a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)],
        Piece::Round(c, r) => [c.x - r, c.y - r, c.x + r, c.y + r],
    }
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.retain(|x| x.is_finite() && *x > 0.0);
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied()
}

/// A sheet's titled legends (a short heading with `LEGEND` in it, its rows under it: a drawing
/// left of each row's words): each row's words and drawing. `drawn` is the sheet's linework.
pub(crate) fn legends(texts: &[TextItem], drawn: &[Piece], rules: &Rules) -> Legends {
    let mut out = Legends::default();
    let titles: Vec<&TextItem> = texts
        .iter()
        .filter(|t| {
            let u = t.value.trim().to_ascii_uppercase();
            let words = u.split_whitespace().count();
            t.height.is_finite()
                && t.height > 0.0
                && words >= 1
                && words <= rules.legend_title_max_words.max(1)
                && rules.legend_title_words.iter().any(|w| u.split(|c: char| !c.is_ascii_alphanumeric()).any(|x| x == w.to_ascii_uppercase()))
        })
        .take(MAX_LEGENDS)
        .collect();
    for title in titles {
        let th = title.height;
        let title_w = 0.7 * th * title.value.trim().chars().count() as f64;
        // Its rows start under it: from a heading's width left of it to one and a half right
        // (a title block's or a schedule's lettering beside it is no row of it).
        let (x_lo, x_hi) = (title.at.x - title_w.max(th), title.at.x + 1.5 * title_w.max(th));
        let depth = rules.legend_rows_h.max(1.0) * th;
        let below: Vec<&TextItem> = texts
            .iter()
            .filter(|u| {
                u.height.is_finite()
                    && u.height > 0.0
                    && u.height <= 1.05 * th
                    && u.at.y < title.at.y - 0.3 * th
                    && u.at.y > title.at.y - depth
                    && u.at.x >= x_lo
                    && u.at.x <= x_hi
                    && !std::ptr::eq(*u, title)
            })
            .collect();
        let Some(h) = median(below.iter().map(|u| u.height).collect()) else { continue };
        // The words' column: the left edge most lines start at.
        let mut columns: BTreeMap<i64, usize> = BTreeMap::new();
        for u in &below {
            let k = (u.at.x / (0.5 * h)).round() as i64;
            *columns.entry(k).or_default() += 1;
        }
        // The column of two lines or more whose first line is nearest under the heading (a table
        // under the legend, a grid bubble's letters beside it start lower); of two, the longer.
        let top = |k: i64| below.iter().filter(|u| (u.at.x / (0.5 * h)).round() as i64 == k).map(|u| u.at.y).fold(f64::NEG_INFINITY, f64::max);
        let Some((&col, _)) = columns.iter().filter(|(_, n)| **n >= 2).max_by(|a, b| top(*a.0).total_cmp(&top(*b.0)).then_with(|| a.1.cmp(b.1)))
        else {
            continue;
        };
        let x0 = col as f64 * 0.5 * h;
        let column: Vec<&TextItem> = below.into_iter().filter(|u| (u.at.x - x0).abs() <= 0.75 * h).collect();
        // Its rows' own lettering (another table's under it is set larger).
        let Some(h) = median(column.iter().map(|u| u.height).collect()) else { continue };
        // The next heading under it (words in taller lettering than its rows; a grid bubble's
        // letter is none) ends it.
        let floor = texts
            .iter()
            .filter(|u| {
                let letters = u.value.chars().filter(char::is_ascii_alphabetic).count();
                let marks = u.value.chars().filter(|c| !c.is_whitespace()).count();
                // (Words: no stroke read as lettering, no giant reading of a hatch.)
                u.height > 1.3 * h
                    && u.height <= 1.5 * th
                    && letters >= 4
                    && 10 * letters >= 6 * marks
                    && u.at.y < title.at.y - 0.3 * th
                    && u.at.y > title.at.y - depth
                    && u.at.x >= x_lo
                    && u.at.x <= x_hi
            })
            .map(|u| u.at.y)
            .fold(f64::NEG_INFINITY, f64::max);
        let mut lines: Vec<&TextItem> = column.into_iter().filter(|u| (u.at.x - x0).abs() <= 0.75 * h && u.at.y > floor).collect();
        lines.sort_by(|a, b| b.at.y.total_cmp(&a.at.y));
        // Down the column to the first wide gap (another table under it); the first row may sit
        // a few heading heights under the heading.
        let mut kept: Vec<&TextItem> = Vec::new();
        for u in lines {
            let gap_from = kept.last().map_or(title.at.y, |p| p.at.y);
            if gap_from - u.at.y > rules.legend_gap_h.max(1.0) * h + if kept.is_empty() { 4.0 * th } else { 0.0 } {
                break;
            }
            kept.push(u);
        }
        if kept.len() < 2 {
            continue;
        }
        let bottom = kept.last().map_or(title.at.y, |u| u.at.y) - 3.0 * h;
        let left = x0 - rules.legend_drawing_h.max(1.0) * h;
        let right_of_words = kept.iter().map(|u| u.at.x + 0.7 * u.height * u.value.trim().chars().count() as f64).fold(x0, f64::max);
        let legend = [left, bottom, right_of_words.max(title.at.x + title_w), title.at.y + th];
        out.boxes.push(legend);
        // The drawings left of the words, each a cell of pieces overlapping in height.
        let mut boxed: Vec<(Piece, [f64; 4])> = drawn
            .iter()
            .map(|p| (*p, piece_box(p)))
            .filter(|(_, b)| b[0] >= left && b[2] <= x0 - 0.15 * h && b[1] >= bottom && b[3] <= title.at.y - 0.3 * th)
            .collect();
        boxed.sort_by(|a, b| b.1[3].total_cmp(&a.1[3]));
        let mut cells: Vec<(f64, f64, Vec<Piece>)> = Vec::new();
        for (p, b) in boxed {
            match cells.last_mut() {
                Some((lo, _, ps)) if b[3] >= *lo - 0.1 * h => {
                    *lo = lo.min(b[1]);
                    if ps.len() <= MAX_ROW_PIECES {
                        ps.push(p);
                    }
                }
                _ => cells.push((b[1], b[3], vec![p])),
            }
        }
        // Each line to the nearest cell by height; the lines by none, wrapped lines joined.
        let mut words: Vec<Vec<&TextItem>> = vec![Vec::new(); cells.len()];
        let mut loose: Vec<(f64, String)> = Vec::new();
        for u in &kept {
            let y = u.at.y + 0.5 * u.height;
            let near = cells
                .iter()
                .enumerate()
                .map(|(i, (lo, hi, _))| {
                    (
                        i,
                        if y < *lo {
                            lo - y
                        } else if y > *hi {
                            y - hi
                        } else {
                            0.0
                        },
                    )
                })
                .min_by(|a, b| a.1.total_cmp(&b.1));
            match near {
                Some((i, d)) if d <= 3.0 * h => {
                    if let Some(w) = words.get_mut(i) {
                        w.push(u);
                    }
                }
                // (A row with no drawing by it: its words still say what the legend draws.)
                _ => match loose.last_mut() {
                    Some((y, text)) if *y - u.at.y <= 1.7 * h => {
                        text.push(' ');
                        text.push_str(u.value.trim());
                        *y = u.at.y;
                    }
                    _ => loose.push((u.at.y, u.value.trim().to_string())),
                },
            }
        }
        let short = |text: &str| text.split_whitespace().count() <= rules.legend_row_max_words.max(1);
        out.unread.extend(loose.into_iter().filter(|(_, text)| short(text)).filter_map(|(_, text)| Some((rules.kind_of_row(&text)?, text))));
        for ((_, _, pieces), lines) in cells.into_iter().zip(words) {
            if lines.is_empty() {
                continue;
            }
            let text = lines.iter().map(|u| u.value.trim()).collect::<Vec<_>>().join(" ");
            // (A paragraph of notes under a legend's heading is no row of it.)
            if !short(&text) {
                continue;
            }
            let Some(kind) = rules.kind_of_row(&text) else { continue };
            if pieces.len() > MAX_ROW_PIECES {
                out.unread.push((kind, text));
                continue;
            }
            out.rows.push(LegendRow { words: text, kind, pieces, text_h: h, legend });
        }
    }
    out
}

// ─── The drawing as the architect's symbols are matched ─────────────────────

/// Corners of one polyline looked at, at most.
const MAX_CORNERS: usize = 256;
/// A polyline of this many corners or more, all as far from their middle (within this share),
/// going round it, draws a circle.
const ROUND_CORNERS: usize = 8;
const ROUND_SHARE: f64 = 0.1;

/// A polyline's circle, when it draws one (a PDF's circle came in as its corners): its middle and
/// radius.
fn round_of(pts: &[Vec2], closed: bool) -> Option<(Vec2, f64)> {
    let mut pts: Vec<Vec2> = pts.to_vec();
    if pts.len() > 1 && pts.first().zip(pts.last()).is_some_and(|(a, b)| a.dist(*b) <= 1e-9) {
        pts.pop();
    }
    let n = pts.len();
    if n < ROUND_CORNERS {
        return None;
    }
    let c = pts.iter().fold(Vec2::new(0.0, 0.0), |a, p| a + *p) * (1.0 / n as f64);
    let r = pts.iter().map(|p| p.dist(c)).sum::<f64>() / n as f64;
    if !(r.is_finite() && r > 1e-9) || pts.iter().any(|p| (p.dist(c) - r).abs() > ROUND_SHARE * r) {
        return None;
    }
    // Round it all (a closed polyline, or one whose ends meet), never an arc of it.
    let ends_meet = pts.first().zip(pts.last()).is_some_and(|(a, b)| a.dist(*b) <= 0.9 * r);
    (closed || ends_meet).then_some((c, r))
}

/// An object's pieces as an architect's symbol is matched: as [`super::unitmarks::pieces`],
/// except that a polyline drawing a circle is that circle, and a polyline's straight run through
/// several corners (a stroke drawn with a point in its middle) is one straight piece.
pub(crate) fn pieces_of(kind: &cadaddy_doc::EntityKind, out: &mut Vec<Piece>) {
    let cadaddy_doc::EntityKind::LwPolyline(pl) = kind else {
        super::unitmarks::pieces(kind, out);
        return;
    };
    let mut pts: Vec<Vec2> = pl.vertices.iter().take(MAX_CORNERS).map(|v| Vec2::new(v.p.x, v.p.y)).collect();
    if !pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()) {
        return;
    }
    if let Some((c, r)) = round_of(&pts, pl.closed) {
        out.push(Piece::Round(c, r));
        return;
    }
    if pl.closed
        && pts.len() > 2
        && let Some(first) = pts.first().copied()
    {
        pts.push(first);
    }
    let mut it = pts.into_iter();
    let Some(mut a) = it.next() else { return };
    let mut b = a;
    for p in it {
        let (u, v) = (b - a, p - b);
        let (lu, lv) = (u.len(), v.len());
        if lv <= 1e-9 {
            continue;
        }
        let straight = lu > 1e-9 && u.dot(v) > 0.0 && (u.cross(v) / (lu * lv)).abs() <= 0.02;
        if !straight && lu > 1e-9 {
            out.push(Piece::Seg(a, b));
            a = b;
        }
        b = p;
    }
    if a.dist(b) > 1e-9 {
        out.push(Piece::Seg(a, b));
    }
}

/// A sheet's linework as [`pieces_of`] takes it (each symbol block's objects where it is
/// inserted), at most [`MAX_SHEET_PIECES`].
pub(crate) fn sheet_pieces(sub: &cadaddy_doc::Drawing) -> Vec<Piece> {
    let mut pieces: Vec<Piece> = Vec::new();
    for e in sub.model.iter() {
        if pieces.len() >= MAX_SHEET_PIECES {
            break;
        }
        match &e.kind {
            cadaddy_doc::EntityKind::Insert(_) => {
                for part in super::modify::explode_kind(sub, e).unwrap_or_default().iter().take(MAX_ROW_PIECES) {
                    pieces_of(&part.kind, &mut pieces);
                }
            }
            k => pieces_of(k, &mut pieces),
        }
    }
    pieces
}

// ─── The architect's objects on one sheet ───────────────────────────────────

/// An object found on an architect's sheet: the legend row (index) whose drawing it is, where its
/// middle is (sheet units).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Found {
    pub row: usize,
    pub at: Vec2,
}

/// The legend rows' drawings on one sheet: `pieces` its linework, `area` where its views are,
/// `exclude` its legends, title block and the like, `unit` the sheet's units per a row's own
/// units (1 on the legend's own sheet; else by their lettering), per row. A legend often draws
/// its symbols larger or smaller than the plans do (a can light twice its plan size): each drawing
/// is looked for at each of [`SCALES`] of that, never smaller than `min_size` (a share of the
/// sheet's lettering: smaller marks are lettering's strokes). Plain outlines (a box, a bar) tell
/// nothing apart and are not looked for: their rows are the second value.
pub(crate) fn find(
    rows: &[LegendRow],
    units: &[f64],
    pieces: &[Piece],
    area: [f64; 4],
    exclude: &[[f64; 4]],
    min_size: f64,
) -> (Vec<Found>, Vec<usize>) {
    let inside = |r: &[f64; 4], p: Vec2| p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3];
    let keep = |p: Vec2| inside(&area, p) && !exclude.iter().any(|r| inside(r, p));
    let mid = |p: &Piece| {
        let b = piece_box(p);
        Vec2::new(0.5 * (b[0] + b[2]), 0.5 * (b[1] + b[3]))
    };
    let mut plain: Vec<usize> = Vec::new();
    let mut templates: Vec<(usize, Template, f64)> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let unit = units.get(i).copied().filter(|u| u.is_finite() && *u > 0.0).unwrap_or(1.0);
        match Template::new_crossed(&r.words, "", "", "", &r.pieces, r.text_h) {
            Some(tp) if !tp.simple => templates.push((i, tp, unit)),
            _ => plain.push(i),
        }
    }
    if templates.is_empty() {
        return (Vec::new(), plain);
    }
    let largest = templates.iter().map(|(_, tp, u)| tp.size() * u * SCALES.iter().copied().fold(0.0, f64::max)).fold(0.0, f64::max);
    let cell = (1.5 * largest).max(1e-6);
    let kept: Vec<Piece> = pieces.iter().filter(|p| keep(mid(p))).take(MAX_SHEET_PIECES).copied().collect();
    let scene = super::unitmarks::Scene::new(kept, cell);
    // Each row alone at each scale; then the most detailed drawing kept where two overlap.
    let mut hits: Vec<(Found, usize, f64)> = Vec::new();
    for (row, tp, unit) in &templates {
        for k in SCALES {
            if tp.size() * unit * k < min_size {
                continue;
            }
            for f in super::unitmarks::find_symbols_apart(std::slice::from_ref(tp), &scene, area, tp.text_h() * unit * k) {
                if keep(f.at) {
                    hits.push((Found { row: *row, at: f.at }, tp.pieces().len(), f.size));
                }
            }
        }
    }
    hits.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.at.x.total_cmp(&b.0.at.x)).then_with(|| a.0.at.y.total_cmp(&b.0.at.y)));
    let mut out: Vec<(Found, f64)> = Vec::new();
    for (f, _, size) in hits {
        if !out.iter().any(|(k, s)| k.at.dist(f.at) <= 0.5 * s.max(size)) {
            out.push((f, size));
        }
    }
    (out.into_iter().map(|(f, _)| f).collect(), plain)
}

// ─── Completing the architect's objects ─────────────────────────────────────

/// One object of a compared kind on its level's plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Obj {
    /// The kind rule's index.
    pub kind: usize,
    pub level: String,
    /// Where on the level's plan (drawing units).
    pub at: [f64; 2],
    pub sheet: String,
    /// Drawn by the architect (else by the engineer).
    pub architect: bool,
    /// What it is on its sheet: the legend row it is drawn as, or the engineer's type mark.
    pub what: String,
    /// The engineer's circuit and switch leg, when given.
    pub circuit: String,
    pub leg: String,
}

/// A room on its level's plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Room {
    pub level: String,
    pub name: String,
    pub number: String,
    pub outline: Vec<Vec2>,
}

impl Room {
    /// Its name and number as printed (`SNACK ALCOVE 12`).
    pub(crate) fn label(&self) -> String {
        let (n, k) = (self.name.trim(), self.number.trim());
        if k.is_empty() || n.split_whitespace().any(|w| w.eq_ignore_ascii_case(k)) { n.to_string() } else { format!("{n} {k}") }
    }

    fn area(&self) -> f64 {
        let n = self.outline.len();
        let mut a = 0.0;
        for i in 0..n {
            if let (Some(p), Some(q)) = (self.outline.get(i), self.outline.get((i + 1) % n)) {
                a += p.x * q.y - q.x * p.y;
            }
        }
        (0.5 * a).abs()
    }

    fn holds(&self, p: Vec2) -> bool {
        cadaddy_geom::point_in_polygon(&self.outline, p)
    }

    fn distance(&self, p: Vec2) -> f64 {
        let n = self.outline.len();
        (0..n)
            .filter_map(|i| Some((*self.outline.get(i)?, *self.outline.get((i + 1) % n)?)))
            .map(|(a, b)| {
                let d = b - a;
                let l = d.dot(d);
                let t = if l > 0.0 { ((p - a).dot(d) / l).clamp(0.0, 1.0) } else { 0.0 };
                p.dist(a + d * t)
            })
            .fold(f64::INFINITY, f64::min)
    }
}

/// A group of the architect's objects no electrical sheet completes: none of the engineer's
/// objects of the kind is by them (in their room), so no type, circuit or control comes from any
/// electrical sheet. Its kind, level, room (none: outside the traced rooms, or the whole level when
/// no electrical sheet draws the kind there, `rooms` then the rooms it is in), its objects (indices
/// into the list), what the room as traced holds of each side, where.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Open {
    pub kind: usize,
    pub level: String,
    pub room: Option<usize>,
    pub rooms: Vec<usize>,
    pub architect: Vec<usize>,
    /// What the room as traced holds of each side (a room traced together with its neighbour
    /// holds the electrical's elsewhere in it).
    pub room_counts: Option<(usize, usize)>,
    /// No electrical sheet draws the kind on the whole level.
    pub level_wide: bool,
    pub at: [f64; 2],
    pub extent: Option<[[f64; 2]; 2]>,
}

/// What completing the architect's objects came to: the open groups, and per kind name the
/// architect's objects and the engineer's, the architect's completed from an electrical sheet.
#[derive(Clone, Debug, Default)]
pub(crate) struct Completion {
    pub open: Vec<Open>,
    pub objects: BTreeMap<String, (usize, usize)>,
    pub completed: BTreeMap<String, usize>,
    /// Objects in no traced room, per side.
    pub roomless: (usize, usize),
}

/// A level's key, whatever it is called (`1`, `LEVEL 1`, `Level 01` are one level).
pub(crate) fn level_key(l: &str) -> String {
    let digits: String = l.chars().filter(char::is_ascii_digit).collect();
    let digits = digits.trim_start_matches('0');
    if !digits.is_empty() {
        return digits.to_string();
    }
    if l.chars().any(|c| c.is_ascii_digit()) {
        return "0".into();
    }
    l.trim().to_ascii_uppercase()
}

fn bounds(pts: impl Iterator<Item = [f64; 2]>) -> Option<[[f64; 2]; 2]> {
    let mut b: Option<[[f64; 2]; 2]> = None;
    for p in pts.filter(|p| p.iter().all(|v| v.is_finite())) {
        b = Some(match b {
            None => [p, p],
            Some([lo, hi]) => [[lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]],
        });
    }
    b
}

fn middle(extent: Option<[[f64; 2]; 2]>) -> [f64; 2] {
    extent.map_or([0.0, 0.0], |[lo, hi]| [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])])
}

/// `ids` in groups: one key each (`key`), each joined to another of its group (`near`), in order.
fn groups(ids: &[usize], key: impl Fn(usize) -> Option<usize>, near: impl Fn(usize, usize) -> bool) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..ids.len()).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        let mut n = 0usize;
        while let Some(&q) = p.get(i) {
            if q == i || n > 64 {
                break;
            }
            i = q;
            n += 1;
        }
        i
    }
    for (x, &i) in ids.iter().enumerate() {
        for (y, &j) in ids.iter().enumerate().skip(x + 1) {
            if key(i) == key(j) && near(i, j) {
                let (rx, ry) = (root(&mut parent, x), root(&mut parent, y));
                if let Some(slot) = parent.get_mut(ry.max(rx)) {
                    *slot = rx.min(ry);
                }
            }
        }
    }
    let mut out: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (x, &i) in ids.iter().enumerate() {
        let r = root(&mut parent, x);
        out.entry(r).or_default().push(i);
    }
    out.into_values().collect()
}

/// Complete the architect's objects from the engineer's (see the module docs): each object the
/// architect draws is the engineer's object of its kind by it in its room (within the kind's
/// reach; one outside every traced room within a room's own reach of one in a room), which gives
/// it its type, circuit and control; the ones with none are open, grouped by room and nearness.
/// `upf`: drawing units per foot of the plans.
pub(crate) fn complete(objs: &[Obj], rooms: &[Room], rules: &Rules, upf: f64) -> Completion {
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let mut out = Completion::default();
    let rooms: Vec<&Room> = rooms.iter().filter(|r| r.outline.len() >= 3).take(MAX_ROOMS).collect();
    let objs: Vec<&Obj> = objs.iter().take(MAX_OBJECTS).collect();
    let room_reach_ft = if rules.rooms.reach_ft.is_finite() && rules.rooms.reach_ft >= 0.0 { rules.rooms.reach_ft } else { 2.0 };
    let room_reach = room_reach_ft * upf;
    // Each object's room: the smallest one holding it, else the nearest within reach.
    let room_of: Vec<Option<usize>> = objs
        .iter()
        .map(|o| {
            let p = Vec2::new(o.at[0], o.at[1]);
            let key = level_key(&o.level);
            let mine = || rooms.iter().enumerate().filter(|(_, r)| level_key(&r.level) == key);
            mine().filter(|(_, r)| r.holds(p)).min_by(|a, b| a.1.area().total_cmp(&b.1.area())).map(|(i, _)| i).or_else(|| {
                mine().map(|(i, r)| (i, r.distance(p))).filter(|(_, d)| *d <= room_reach).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i)
            })
        })
        .collect();
    out.roomless =
        objs.iter().zip(&room_of).filter(|(_, r)| r.is_none()).fold((0, 0), |(a, e), (o, _)| if o.architect { (a + 1, e) } else { (a, e + 1) });
    let max = if rules.max_findings > 0 { rules.max_findings } else { 400 };
    let room_at = |i: usize| room_of.get(i).copied().flatten();
    for (k, rule) in rules.kinds.iter().enumerate() {
        let mine: Vec<usize> = (0..objs.len()).filter(|&i| objs.get(i).is_some_and(|o| o.kind == k)).collect();
        let a_n = mine.iter().filter(|&&i| objs.get(i).is_some_and(|o| o.architect)).count();
        out.objects.insert(rule.name.clone(), (a_n, mine.len() - a_n));
        if a_n == 0 {
            continue;
        }
        let e_levels: BTreeSet<String> = mine.iter().filter_map(|&i| objs.get(i)).filter(|o| !o.architect).map(|o| level_key(&o.level)).collect();
        let reach = rule.reach() * upf;
        let mut levels: Vec<String> = mine.iter().filter_map(|&i| objs.get(i)).filter(|o| o.architect).map(|o| level_key(&o.level)).collect();
        levels.sort();
        levels.dedup();
        let mut completed = 0usize;
        for key in &levels {
            let on_level: Vec<usize> = mine.iter().copied().filter(|&i| objs.get(i).is_some_and(|o| level_key(&o.level) == *key)).collect();
            let level_name = on_level.iter().filter_map(|&i| objs.get(i)).find(|o| o.architect).map_or_else(|| key.clone(), |o| o.level.clone());
            let points = |ids: &[usize]| ids.iter().filter_map(|&i| objs.get(i)).map(|o| o.at).collect::<Vec<_>>();
            let (arch, eng): (Vec<usize>, Vec<usize>) = on_level.iter().partition(|&&i| objs.get(i).is_some_and(|o| o.architect));
            // Its completing object: the engineer's of the kind by it in its room (a light across a
            // wall lights another room; the trace's outline and the sheets' registration may put
            // one light either side of a room's edge).
            let completes = |i: usize, o: &Obj| {
                eng.iter().filter_map(|&j| Some((j, objs.get(j)?))).any(|(j, x)| {
                    let d = (x.at[0] - o.at[0]).hypot(x.at[1] - o.at[1]);
                    d <= reach && (room_at(i) == room_at(j) || ((room_at(i).is_none() || room_at(j).is_none()) && d <= room_reach))
                })
            };
            let open: Vec<usize> = arch.iter().copied().filter(|&i| objs.get(i).is_some_and(|o| !completes(i, o))).collect();
            completed += arch.len() - open.len();
            if open.is_empty() || out.open.len() >= max {
                continue;
            }
            if !e_levels.contains(key) {
                // No electrical sheet draws the kind on the whole level: one group for the level.
                let mut hit: Vec<usize> = open.iter().filter_map(|&i| room_at(i)).collect();
                hit.sort_unstable();
                hit.dedup();
                let extent = bounds(points(&open).into_iter());
                out.open.push(Open {
                    kind: k,
                    level: level_name.clone(),
                    room: None,
                    rooms: hit,
                    architect: open,
                    room_counts: None,
                    level_wide: true,
                    at: middle(extent),
                    extent,
                });
                continue;
            }
            // Each group of them (in one room, each near another): its room its label.
            let cluster = rule.cluster() * upf;
            let near = |i: usize, j: usize| match (objs.get(i), objs.get(j)) {
                (Some(a), Some(b)) => (a.at[0] - b.at[0]).hypot(a.at[1] - b.at[1]) <= cluster,
                _ => false,
            };
            for group in groups(&open, room_at, near) {
                if out.open.len() >= max {
                    break;
                }
                let room = group.first().and_then(|&i| room_at(i));
                let room_counts = room.map(|ri| {
                    let n = |architect: bool| {
                        on_level.iter().filter(|&&i| room_at(i) == Some(ri) && objs.get(i).is_some_and(|o| o.architect == architect)).count()
                    };
                    (n(true), n(false))
                });
                let extent = bounds(points(&group).into_iter());
                out.open.push(Open {
                    kind: k,
                    level: level_name.clone(),
                    room,
                    rooms: Vec::new(),
                    architect: group,
                    room_counts,
                    level_wide: false,
                    at: middle(extent),
                    extent,
                });
            }
        }
        out.completed.insert(rule.name.clone(), completed);
    }
    out.open.sort_by(|a, b| {
        level_key(&a.level).cmp(&level_key(&b.level)).then_with(|| a.at[0].total_cmp(&b.at[0])).then_with(|| a.at[1].total_cmp(&b.at[1]))
    });
    out
}

// ─── What an open group says ────────────────────────────────────────────────

/// The sheets of `ids`' objects, in order.
fn sheets_of(objs: &[Obj], ids: &[usize]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for o in ids.iter().filter_map(|&i| objs.get(i)) {
        if !o.sheet.is_empty() && !v.contains(&o.sheet) {
            v.push(o.sheet.clone());
        }
    }
    v
}

/// What the architect's objects are drawn as (`2 as RECESSED DOWN LIGHT`: its legend row's words).
fn drawn_as(objs: &[Obj], ids: &[usize]) -> String {
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for o in ids.iter().filter_map(|&i| objs.get(i)) {
        *by.entry(o.what.as_str()).or_default() += 1;
    }
    by.iter().map(|(w, n)| if w.is_empty() { format!("{n}") } else { format!("{n} as {w}") }).collect::<Vec<_>>().join(", ")
}

/// An open group's place, for its words: the room and level, or the level.
pub(crate) fn place_of(o: &Open, rooms: &[Room]) -> String {
    match o.room.and_then(|i| rooms.get(i)) {
        Some(r) => format!("{} (level {})", r.label(), o.level),
        None if o.level_wide => format!("level {}", o.level),
        None => format!("level {}, outside the traced rooms", o.level),
    }
}

/// How an open group is completed meanwhile, by the next source after the documents (trade
/// practice: its neighbours): the type of the nearest of the engineer's objects of its kind on its
/// level, the nearest circuit of its kind (and how far, feet), and its control (that object's
/// switch leg, else the kind's rule).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Proposal {
    pub type_mark: Option<(String, f64)>,
    pub circuit: Option<(String, f64)>,
    pub control: String,
}

pub(crate) fn proposal(o: &Open, objs: &[Obj], rule: &KindRule, upf: f64) -> Option<Proposal> {
    let build = rule.build.as_ref()?;
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let key = level_key(&o.level);
    let mine: Vec<[f64; 2]> = o.architect.iter().filter_map(|&i| objs.get(i)).map(|x| x.at).collect();
    let dist = |x: &Obj| mine.iter().map(|p| (p[0] - x.at[0]).hypot(p[1] - x.at[1])).fold(f64::INFINITY, f64::min) / upf;
    let theirs = || objs.iter().filter(|x| !x.architect && x.kind == o.kind && level_key(&x.level) == key);
    let type_mark = theirs().filter(|x| !x.what.trim().is_empty()).map(|x| (x, dist(x))).min_by(|a, b| a.1.total_cmp(&b.1));
    let circuited = theirs().filter(|x| !x.circuit.trim().is_empty()).map(|x| (x, dist(x))).min_by(|a, b| a.1.total_cmp(&b.1));
    let control = match circuited {
        Some((x, _)) if !x.leg.trim().is_empty() => {
            format!("switch leg {} of circuit {} (the nearest light's control)", x.leg.trim(), x.circuit.trim())
        }
        _ => build.proposed_control.clone(),
    };
    Some(Proposal {
        type_mark: type_mark.map(|(x, d)| (x.what.trim().to_string(), d)),
        circuit: circuited.map(|(x, d)| (x.circuit.trim().to_string(), d)),
        control,
    })
}

/// An open group as the RFI list's item (kind `conflict`, each side cited): what no electrical
/// sheet gives for it, both sheets and both counts, its place, how it is completed meanwhile.
pub(crate) fn to_rfi(o: &Open, objs: &[Obj], rooms: &[Room], rules: &Rules, engineer_sheets: &[String], upf: f64) -> Option<Rfi> {
    let rule = rules.kinds.get(o.kind)?;
    let place = place_of(o, rooms);
    let n = o.architect.len();
    let a_sheets = sheets_of(objs, &o.architect);
    let a_doc = if a_sheets.is_empty() { "the architectural sheets".to_string() } else { a_sheets.join(", ") };
    let e_doc = if engineer_sheets.is_empty() { "the electrical sheets".to_string() } else { engineer_sheets.join(", ") };
    let name = &rule.name;
    let needs = if rule.requires.is_empty() { "type, circuit or control".to_string() } else { rule.requires.join(", ") };
    let traced = match o.room_counts {
        Some((ra, re)) if re > 0 => {
            format!(
                "; none of the electrical's within {:.0} ft of them (the room as traced holds A: {ra}, E: {re}: the electrical's are elsewhere in it)",
                rule.reach()
            )
        }
        _ => String::new(),
    };
    let issue = if o.level_wide {
        format!(
            "{n} {name} from the architect's {a_doc} on a level no electrical sheet draws {name} on: no {needs} on any electrical sheet (A: {n}, E: 0)"
        )
    } else {
        format!("{n} {name} from the architect's {a_doc}: no {needs} on any electrical sheet (A: {n}, E: 0){traced}")
    };
    let assumed = match &rule.build {
        Some(_) => format!("built as the architect draws them: {n} {name}, inferred and unsure, completed by the proposal until answered"),
        None => format!("not built until answered: the architect's {n} {name} are listed here"),
    };
    let proposed = proposal(o, objs, rule, upf).map(|p| {
        let ty = p
            .type_mark
            .map_or_else(|| "a type from the luminaire schedule".to_string(), |(t, d)| format!("type {t} (the nearest light's, {d:.0} ft off)"));
        let circuit = p.circuit.map_or_else(
            || "a circuit of the room's panel (no circuit of the kind on the level's electrical plans)".to_string(),
            |(c, d)| format!("circuit {c} (the nearest one of its kind, {d:.0} ft off)"),
        );
        format!("{ty}; {circuit}; control: {}", p.control)
    });
    let rooms_named: Vec<String> = o.rooms.iter().filter_map(|&i| rooms.get(i)).map(Room::label).take(MAX_ROOMS_NAMED).collect();
    let about = if rooms_named.is_empty() {
        format!("{name} in {place}")
    } else {
        let more = o.rooms.len().saturating_sub(rooms_named.len());
        format!("{name} on {place}: {}{}", rooms_named.join(", "), if more > 0 { format!(" and {more} more rooms") } else { String::new() })
    };
    let topic = format!("{COMPLETE_TOPIC} {name}");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("{topic}|{about}|{}", level_key(&o.level)).bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    let mut sheets = a_sheets.clone();
    sheets.extend(engineer_sheets.iter().filter(|s| !a_sheets.contains(s)).cloned());
    sheets.truncate(8);
    Some(Rfi {
        id: format!("RFI-X{:08x}", h as u32),
        kind: Some(RfiKind::Conflict),
        severity: Severity::parse(&rule.open_severity).unwrap_or(Severity::High),
        level: o.level.clone(),
        at: o.at,
        extent: o.extent,
        sheets,
        about,
        issue,
        assumed,
        proposed,
        topic,
        sides: vec![
            RfiSide { document: format!("architectural: {a_doc}"), says: format!("{n} {name} ({})", drawn_as(objs, &o.architect)) },
            RfiSide { document: format!("electrical: {e_doc}"), says: format!("no {name} there: no {needs}") },
        ],
        ..Rfi::default()
    })
}

/// The topic every open group's RFI starts with.
pub(crate) const COMPLETE_TOPIC: &str = "complete:";

/// The group's one line for the job report (`SNACK ALCOVE 12 (level 1): lights A: 2 on A-601, E: 0 on E-101`).
pub(crate) fn line(o: &Open, objs: &[Obj], rooms: &[Room], rules: &Rules, engineer_sheets: &[String]) -> String {
    let name = rules.kinds.get(o.kind).map_or("objects", |k| k.name.as_str());
    let a = sheets_of(objs, &o.architect);
    let on = |v: &[String]| if v.is_empty() { String::new() } else { format!(" on {}", v.join(", ")) };
    format!("{}: {name} A: {}{}, E: 0{}", place_of(o, rooms), o.architect.len(), on(&a), on(engineer_sheets))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(v: &str, x: f64, y: f64, h: f64) -> TextItem {
        TextItem { value: v.into(), at: Vec2::new(x, y), height: h, rotation: 0.0 }
    }

    /// A can light as an architect draws it: a circle with a stroke across it.
    fn can(at: Vec2, r: f64) -> Vec<Piece> {
        let d = Vec2::new(r * 0.7, r * 0.7);
        vec![Piece::Round(at, r), Piece::Seg(at - d, at + d)]
    }

    fn square(lo: Vec2, s: f64) -> Vec<Piece> {
        let (a, b, c, d) = (lo, lo + Vec2::new(s, 0.0), lo + Vec2::new(s, s), lo + Vec2::new(0.0, s));
        vec![Piece::Seg(a, b), Piece::Seg(b, c), Piece::Seg(c, d), Piece::Seg(d, a)]
    }

    /// A made-up legend: a heading, a ceiling grid row (a box: no object), a supply diffuser
    /// (a box with an X: the mechanical's, not compared), a can light whose words wrap onto a
    /// second line, and an exit sign.
    fn legend_sheet() -> (Vec<TextItem>, Vec<Piece>) {
        let texts = vec![
            t("CEILING LEGEND", 100.0, 500.0, 6.0),
            t("ACOUSTICAL TILE 2X2", 130.0, 470.0, 3.0),
            t("SUPPLY DIFFUSER", 130.0, 450.0, 3.0),
            t("SEE MECHANICAL", 130.0, 446.0, 3.0),
            t("RECESSED", 130.0, 431.0, 3.0),
            t("DOWN LIGHT", 130.0, 427.0, 3.0),
            t("EXIT SIGN", 130.0, 410.0, 3.0),
            // A note far under it, another table.
            t("GENERAL NOTES", 130.0, 200.0, 3.0),
        ];
        // A title block's column of lettering beside it, longer than its rows; grid bubbles'
        // letters left of it; a table of larger lettering under it.
        let mut texts = texts;
        texts.extend((0..9).map(|i| t("FIRM LINE", 250.0, 480.0 - 8.0 * f64::from(i), 3.0)));
        texts.extend(["F", "G", "H"].iter().enumerate().map(|(i, v)| t(v, 60.0, 460.0 - 15.0 * i as f64, 5.0)));
        texts.push(t("CEILING TYPES", 100.0, 380.0, 6.0));
        // A hatch read as giant lettering at its top.
        texts.push(t("BD:::::'::.: YX", 102.0, 480.0, 16.0));
        texts.extend((0..8).map(|i| t("TYPE ROW", 110.0, 370.0 - 6.0 * f64::from(i), 4.5)));
        let mut drawn = square(Vec2::new(105.0, 467.0), 8.0);
        let mut diffuser = square(Vec2::new(105.0, 444.0), 8.0);
        diffuser.push(Piece::Seg(Vec2::new(105.0, 444.0), Vec2::new(113.0, 452.0)));
        diffuser.push(Piece::Seg(Vec2::new(105.0, 452.0), Vec2::new(113.0, 444.0)));
        drawn.extend(diffuser);
        drawn.extend(can(Vec2::new(109.0, 430.0), 2.0));
        drawn.push(Piece::Round(Vec2::new(109.0, 411.0), 2.5));
        drawn.push(Piece::Seg(Vec2::new(109.0, 408.5), Vec2::new(109.0, 413.5)));
        drawn.push(Piece::Seg(Vec2::new(106.5, 411.0), Vec2::new(111.5, 411.0)));
        (texts, drawn)
    }

    #[test]
    fn the_rules_name_lights_by_their_legend_words_and_the_engineers_by_kind() {
        let r = rules();
        let light = r.kinds.iter().position(|k| k.kind == "light").unwrap_or_else(|| panic!("no light rule"));
        assert_eq!(r.kind_of_row("CEILING CAN LIGHT"), Some(light));
        assert_eq!(r.kind_of_row("RECESSED LED DOWNLIGHT"), Some(light));
        assert_ne!(r.kind_of_row("EXIT SIGN"), Some(light));
        assert_eq!(r.kind_of_row("SUPPLY DIFFUSER SEE MECHANICAL"), None);
        assert_eq!(r.kind_of_row("LIGHT GAUGE METAL FRAMING"), None);
        let k = r.kinds.get(light).unwrap_or_else(|| panic!("no rule"));
        assert!(k.is_engineers("luminaire", "IfcLightFixture") && k.is_engineers("wall luminaire", ""));
        assert!(!k.is_engineers("exit_sign", "IfcLightFixture") && !k.is_engineers("receptacle", "IfcOutlet"));
        assert!(r.architects("A") && !r.architects("E") && k.engineers_sheet("E"));
    }

    #[test]
    fn a_titled_legend_reads_each_row_with_its_wrapped_words_and_drawing() {
        let (texts, drawn) = legend_sheet();
        let l = legends(&texts, &drawn, rules());
        let words: Vec<&str> = l.rows.iter().map(|r| r.words.as_str()).collect();
        assert_eq!(words, ["RECESSED DOWN LIGHT", "EXIT SIGN"], "{l:?}");
        assert_eq!(l.rows.first().map(|r| r.pieces.len()), Some(2));
        assert_eq!(l.boxes.len(), 1);
        // The general notes far under it are no row.
        assert!(l.boxes.iter().all(|b| b[1] > 300.0), "{:?}", l.boxes);
    }

    #[test]
    fn a_legend_drawing_is_found_on_its_plan_and_a_lone_circle_is_not() {
        let (texts, mut drawn) = legend_sheet();
        let l = legends(&texts, &drawn, rules());
        // Three cans on the plan (one turned a quarter), a lone circle, a box with an X.
        for (x, y) in [(300.0, 300.0), (340.0, 300.0), (380.0, 300.0)] {
            drawn.extend(can(Vec2::new(x, y), 2.0));
        }
        drawn.push(Piece::Round(Vec2::new(420.0, 300.0), 2.0));
        drawn.extend(square(Vec2::new(450.0, 296.0), 8.0));
        let rows: Vec<LegendRow> = l.rows.iter().filter(|r| r.words.contains("DOWN")).cloned().collect();
        let (found, plain) = find(&rows, &[1.0], &drawn, [0.0, 0.0, 1000.0, 1000.0], &l.boxes, 0.0);
        let mut xs: Vec<f64> = found.iter().map(|f| f.at.x.round()).collect();
        xs.sort_by(f64::total_cmp);
        assert_eq!(xs, [300.0, 340.0, 380.0], "{found:?}");
        assert!(plain.is_empty(), "{plain:?}");
    }

    fn polyline(pts: &[(f64, f64)], closed: bool) -> cadaddy_doc::EntityKind {
        cadaddy_doc::EntityKind::LwPolyline(cadaddy_doc::LwPolyline {
            vertices: pts
                .iter()
                .map(|(x, y)| cadaddy_geom::PolyVertex { p: Vec2::new(*x, *y), bulge: 0.0, start_width: 0.0, end_width: 0.0 })
                .collect(),
            closed,
            const_width: 0.0,
            elevation: 0.0,
            plinegen: false,
        })
    }

    /// A PDF's circle comes in as a ring of corners and its stroke with a point in its middle: the
    /// match takes them as one circle and one straight piece; an arc or a zig-zag stays pieces.
    #[test]
    fn a_ring_of_corners_is_a_circle_and_a_straight_run_one_piece() {
        let ring: Vec<(f64, f64)> =
            (0..10).map(|i| f64::from(i) * std::f64::consts::TAU / 10.0).map(|a| (5.0 + 3.0 * a.cos(), 7.0 + 3.0 * a.sin())).collect();
        let mut out = Vec::new();
        pieces_of(&polyline(&ring, true), &mut out);
        assert!(matches!(out.as_slice(), [Piece::Round(c, r)] if c.dist(Vec2::new(5.0, 7.0)) < 1e-6 && (r - 3.0).abs() < 1e-6), "{out:?}");
        let half: Vec<(f64, f64)> = ring.iter().take(6).copied().collect();
        out.clear();
        pieces_of(&polyline(&half, false), &mut out);
        assert_eq!(out.len(), 5, "{out:?}");
        out.clear();
        pieces_of(&polyline(&[(0.0, 0.0), (2.0, 2.0), (4.0, 4.0)], false), &mut out);
        assert_eq!(out, [Piece::Seg(Vec2::new(0.0, 0.0), Vec2::new(4.0, 4.0))]);
        out.clear();
        pieces_of(&polyline(&[(0.0, 0.0), (2.0, 2.0), (4.0, 0.0)], false), &mut out);
        assert_eq!(out.len(), 2);
    }

    /// A legend drawing its can light at twice the plan's size still finds the plan's cans; a
    /// smaller mark than the lettering allows is not one.
    #[test]
    fn a_legend_drawn_larger_than_its_plan_still_finds_the_plans_symbols() {
        let (texts, mut drawn) = legend_sheet();
        let l = legends(&texts, &drawn, rules());
        for (x, y) in [(300.0, 300.0), (340.0, 300.0)] {
            drawn.extend(can(Vec2::new(x, y), 1.0));
        }
        drawn.extend(can(Vec2::new(380.0, 300.0), 0.4));
        let rows: Vec<LegendRow> = l.rows.iter().filter(|r| r.words.contains("DOWN")).cloned().collect();
        let (found, _) = find(&rows, &[1.0], &drawn, [0.0, 0.0, 1000.0, 1000.0], &l.boxes, 1.5);
        let mut xs: Vec<f64> = found.iter().map(|f| f.at.x.round()).collect();
        xs.sort_by(f64::total_cmp);
        assert_eq!(xs, [300.0, 340.0], "{found:?}");
    }

    fn room(name: &str, number: &str, lo: (f64, f64), hi: (f64, f64)) -> Room {
        Room {
            level: "1".into(),
            name: name.into(),
            number: number.into(),
            outline: vec![Vec2::new(lo.0, lo.1), Vec2::new(hi.0, lo.1), Vec2::new(hi.0, hi.1), Vec2::new(lo.0, hi.1)],
        }
    }

    fn obj(architect: bool, x: f64, y: f64) -> Obj {
        Obj {
            kind: light(),
            level: "LEVEL 1".into(),
            at: [x, y],
            sheet: if architect { "A-601".into() } else { "E-101".into() },
            architect,
            what: if architect { "DOWN LIGHT".into() } else { "A".into() },
            circuit: if architect { String::new() } else { "L1-7".into() },
            leg: if architect { String::new() } else { "b".into() },
        }
    }

    fn light() -> usize {
        rules().kinds.iter().position(|k| k.kind == "light").unwrap_or(0)
    }

    /// The task's two-sheet set (12 units a foot): a small named room the architect lights with
    /// two cans and the engineer leaves bare, a corridor both light with four.
    fn two_sheets(engineer_in_room: usize) -> (Vec<Obj>, Vec<Room>) {
        let rooms = vec![room("SNACK ALCOVE", "12", (0.0, 0.0), (96.0, 96.0)), room("CORRIDOR", "10", (100.0, 0.0), (600.0, 60.0))];
        let mut objs = vec![obj(true, 30.0, 48.0), obj(true, 66.0, 48.0)];
        for x in [150.0, 250.0, 350.0, 450.0] {
            objs.push(obj(true, x, 30.0));
            objs.push(obj(false, x + 6.0, 30.0));
        }
        for i in 0..engineer_in_room {
            objs.push(obj(false, 30.0 + 36.0 * i as f64, 50.0));
        }
        (objs, rooms)
    }

    #[test]
    fn lights_no_electrical_sheet_completes_are_the_finding_with_both_counts_and_sheets() {
        let r = rules();
        let (objs, rooms) = two_sheets(0);
        let c = complete(&objs, &rooms, r, 12.0);
        assert_eq!(c.open.len(), 1, "{c:?}");
        assert_eq!(c.completed.get("lights"), Some(&4));
        let o = c.open.first().unwrap_or_else(|| panic!("none open"));
        assert_eq!((o.room, o.architect.len(), o.level_wide), (Some(0), 2, false));
        let es = ["E-101".to_string()];
        let rfi = to_rfi(o, &objs, &rooms, r, &es, 12.0).unwrap_or_else(|| panic!("no rfi"));
        assert!(rfi.issue.contains("A: 2, E: 0") && rfi.issue.contains("no fixture type, circuit, control on any electrical sheet"), "{rfi:?}");
        assert!(rfi.about.contains("SNACK ALCOVE 12"), "{rfi:?}");
        assert_eq!(rfi.sheets, ["A-601", "E-101"]);
        assert_eq!(rfi.severity, Severity::High);
        assert!(rfi.topic.starts_with(COMPLETE_TOPIC), "{rfi:?}");
        // Completed meanwhile by its neighbours: their type, circuit and switch leg.
        let p = rfi.proposed.unwrap_or_default();
        assert!(p.contains("type A") && p.contains("circuit L1-7") && p.contains("switch leg b"), "{p}");
        assert!(line(o, &objs, &rooms, r, &es).contains("SNACK ALCOVE 12 (level LEVEL 1): lights A: 2 on A-601, E: 0 on E-101"));
    }

    #[test]
    fn lights_every_electrical_sheet_completes_give_no_finding() {
        let (objs, rooms) = two_sheets(2);
        let c = complete(&objs, &rooms, rules(), 12.0);
        assert!(c.open.is_empty(), "{c:?}");
        assert_eq!(c.objects.get("lights"), Some(&(6, 6)));
        assert_eq!(c.completed.get("lights"), Some(&6));
    }

    /// The electrical sheets' lights the architect doesn't draw, or more of them, or a room no
    /// sheet lights: no requirement of an architect's object is open there, so no finding (the
    /// build needs none of it).
    #[test]
    fn the_electrical_sheets_drawing_more_than_the_architect_is_no_finding() {
        let rooms = vec![room("OFFICE", "20", (0.0, 0.0), (120.0, 120.0)), room("LOUNGE", "21", (200.0, 0.0), (440.0, 240.0))];
        let mut objs = vec![obj(false, 40.0, 60.0), obj(false, 80.0, 60.0), obj(true, 260.0, 100.0)];
        for x in [262.0, 300.0, 340.0, 420.0] {
            objs.push(obj(false, x, 100.0));
        }
        let c = complete(&objs, &rooms, rules(), 12.0);
        assert!(c.open.is_empty(), "{c:?}");
    }

    /// An alcove traced together with the lounge beside it (one room) holds the architect's three
    /// lights with none of the electrical's by them, the electrical's being in the lounge part:
    /// that group is open, labelled by the room, with what the room holds besides.
    #[test]
    fn lights_with_no_electrical_light_by_them_are_open_in_a_room_traced_with_its_neighbour() {
        let r = rules();
        let rooms = vec![room("LOUNGE", "30", (0.0, 0.0), (480.0, 240.0))];
        let mut objs: Vec<Obj> = [40.0, 80.0, 120.0].iter().map(|x| obj(true, *x, 220.0)).collect();
        for x in [300.0, 360.0, 420.0] {
            objs.push(obj(true, x, 60.0));
            objs.push(obj(false, x + 4.0, 62.0));
        }
        let c = complete(&objs, &rooms, r, 12.0);
        assert_eq!(c.open.len(), 1, "{c:?}");
        let o = c.open.first().unwrap_or_else(|| panic!("none"));
        assert_eq!((o.room, o.architect.len(), o.room_counts), (Some(0), 3, Some((6, 3))));
        let rfi = to_rfi(o, &objs, &rooms, r, &[], 12.0).unwrap_or_else(|| panic!("no rfi"));
        assert!(rfi.issue.contains("A: 3, E: 0") && rfi.issue.contains("holds A: 6, E: 3"), "{rfi:?}");
        // One outside every traced room a foot and a half from one in a room is completed by it.
        let edge = vec![obj(true, 50.0, 270.0), obj(false, 50.0, 290.0)];
        let c3 = complete(&edge, &[room("ALCOVE", "31", (0.0, 300.0), (100.0, 400.0))], r, 12.0);
        assert!(c3.open.is_empty(), "{c3:?}");
        // A light across a wall (another room's) completes none by it.
        let mut walled = vec![room("ALCOVE", "31", (0.0, 300.0), (100.0, 400.0)), room("CLOSET", "32", (104.0, 300.0), (160.0, 400.0))];
        walled.push(room("HALL", "33", (0.0, 410.0), (300.0, 470.0)));
        let across = vec![obj(true, 90.0, 350.0), obj(false, 120.0, 350.0), obj(true, 150.0, 440.0), obj(false, 151.0, 440.0)];
        let c2 = complete(&across, &walled, r, 12.0);
        let open: Vec<_> = c2.open.iter().map(|o| (o.room, o.architect.len())).collect();
        assert_eq!(open, [(Some(0), 1)], "{c2:?}");
        // Two groups far apart in one room are two findings.
        objs.push(obj(true, 40.0, 20.0));
        objs.push(obj(true, 60.0, 20.0));
        let c = complete(&objs, &rooms, r, 12.0);
        let mut sizes: Vec<usize> = c.open.iter().map(|o| o.architect.len()).collect();
        sizes.sort_unstable();
        assert_eq!(sizes, [2, 3], "{c:?}");
    }

    #[test]
    fn a_level_no_electrical_sheet_lights_is_one_finding() {
        let r = rules();
        let rooms = vec![room("ROOM", "1", (0.0, 0.0), (100.0, 100.0)), room("ROOM", "2", (200.0, 0.0), (300.0, 100.0))];
        let objs: Vec<Obj> = vec![obj(true, 50.0, 50.0), obj(true, 250.0, 50.0), obj(true, 900.0, 900.0)];
        let c = complete(&objs, &rooms, r, 12.0);
        assert_eq!(c.open.len(), 1, "{c:?}");
        let o = c.open.first().unwrap_or_else(|| panic!("none"));
        assert_eq!((o.room, o.rooms.len(), o.architect.len(), o.level_wide), (None, 2, 3, true));
        let rfi = to_rfi(o, &objs, &rooms, r, &[], 12.0).unwrap_or_else(|| panic!("no rfi"));
        assert!(rfi.about.contains("ROOM 1, ROOM 2") && rfi.issue.contains("no electrical sheet draws lights on"), "{rfi:?}");
        // Proposed: no light of the level to take a type or circuit from.
        assert!(
            rfi.proposed.as_deref().is_some_and(|p| p.contains("a type from the luminaire schedule") && p.contains("switch at the room's door")),
            "{rfi:?}"
        );
    }

    /// A legend row naming lights with no drawing the job can read (its bar left out of the row's
    /// cell) is said so; a paragraph of notes under the heading is no row.
    #[test]
    fn a_legend_row_with_no_drawing_read_is_listed_and_a_paragraph_is_no_row() {
        let (mut texts, drawn) = legend_sheet();
        texts.push(t("LINEAR LIGHT", 130.0, 395.0, 3.0));
        texts.push(t(
            "COORDINATE THE LOCATION OF WALL MOUNTED DEVICES INCLUDING RECEPTACLES SWITCHES LIGHT FIXTURES AND DATA BOXES",
            130.0,
            380.0,
            3.0,
        ));
        let l = legends(&texts, &drawn, rules());
        assert_eq!(l.unread, [(light(), "LINEAR LIGHT".to_string())], "{l:?}");
    }

    #[test]
    fn levels_are_one_whatever_they_are_called() {
        assert_eq!(level_key("LEVEL 01"), "1");
        assert_eq!(level_key("1"), "1");
        assert_eq!(level_key("Roof"), "ROOF");
    }
}
