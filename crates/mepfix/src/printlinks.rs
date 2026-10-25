//! The print set's links, read off real imports: callout bubbles, section and elevation marks,
//! enlarged-plan callouts and their boundaries, match lines, key plans, and written references
//! (`SEE SHEET E-501`, `SEE DETAIL 3/E-501`, `REFER TO ARCHITECTURAL`), on any layer.
//!
//! [`crate::callout_reader`] reads the fixture's own `CALLOUT` layer; a printed sheet comes in on
//! pen layers, so this reader goes by shape and lettering alone:
//! - a **bubble** is a round shape with a short mark in its top half (the detail or view
//!   number) and a sheet number in its bottom half, usually split by a horizontal divider;
//! - a long line leaving the bubble makes it a **section mark**, a pointed tip an
//!   **elevation tag**, a leader to a box (or "ENLARGED" lettering beside it) an
//!   **enlarged-plan callout** with that box as the enlarged area's **boundary**;
//! - "MATCH LINE" lettering along a long line is a **match line**, with the sheet it names;
//! - a bubble naming its own sheet beside a view title is that view's number, not a link.
//!
//! [`read_page`] reads one page into a compact [`PageRead`] (so a big set never holds every
//! page's drawing at once); [`link_set`] puts the pages together with the set's sheet index:
//! every link resolved against the sheet numbers (unresolved ones are findings, never dropped),
//! enlarged plans registered into their boundary on the overall plan of their level (shared
//! grid lines, else the boundary box and the scales), the enlarged sheet made the authority
//! inside its boundary (the overall plan's devices there are not counted again), plans split by
//! match lines stitched into one level, and the facts details print (mounting heights) put on
//! the devices their callouts point from.

use crate::device_types::{self, TagVerdict};
use crate::sheet_facts::{self, TextItem};
use crate::sheet_reader;
use cadaddy_doc::{Drawing, EntityKind};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Most callouts kept per page (hostile files).
const MAX_PAGE_CALLOUTS: usize = 2_000;
/// Most devices kept per page.
const MAX_PAGE_DEVICES: usize = 20_000;
/// Most boxes looked at per page.
const MAX_BOXES: usize = 5_000;
/// Most straight pieces looked at per page.
const MAX_PIECES: usize = 400_000;
/// Most cells the straight pieces are filed under per page (past it, by their ends only).
const MAX_SEG_CELLS: usize = 4_000_000;
/// Most texts read for references per page.
const MAX_REF_TEXTS: usize = 50_000;
/// Most callouts in a set's answer.
const MAX_SET_CALLOUTS: usize = 20_000;

/// A sheet of the set as its index lists it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SetSheet {
    pub number: String,
    pub title: String,
    pub discipline: String,
    /// `plan`, `enlarged_plan`, `detail`, `section`, `schedule`, … ([`crate::printset`]).
    #[serde(rename = "type")]
    pub sheet_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub level: String,
}

/// A link read on a page: a bubble or mark (geometry and lettering), or a written reference.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Callout {
    /// `detail_bubble`, `section_mark`, `elevation_tag`, `enlarged_plan`, `key_plan`,
    /// `match_line`, `sheet_ref`, `detail_ref`, `enlarged_ref` or `discipline_ref`.
    pub kind: String,
    /// The detail or view number (`3`), when the link names one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    /// The sheet it points at (`E-501`), or a discipline letter for `discipline_ref` (`A`).
    pub target_sheet: String,
    pub on_sheet: String,
    /// Whether the target is in the set.
    pub resolved: bool,
    /// Whether the target sheet was read and shows a view with that number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_found: Option<bool>,
    /// Where it is on its page, drawing units.
    pub at: [f64; 2],
    /// The lettering a written reference was read from.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// The bubble's radius (0 for a written reference).
    #[serde(skip)]
    pub radius: f64,
    /// A horizontal divider splits the bubble.
    #[serde(skip)]
    pub divider: bool,
    /// The box its leader reaches (an enlarged area's boundary).
    #[serde(skip)]
    pub boundary: Option<[f64; 4]>,
}

/// A view drawn on a page, from its title.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageView {
    pub number: String,
    pub title: String,
    pub scale_text: Option<String>,
    pub region: Option<[f64; 4]>,
    /// Grid lines inside the view: label → position (x lines, y lines).
    pub grid_x: Vec<(String, f64)>,
    pub grid_y: Vec<(String, f64)>,
    /// Heights the view prints (`48" AFF`), feet.
    pub mounting_ft: Vec<f64>,
}

/// A device symbol on a page: its type tag and where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct PageDevice {
    pub tag: String,
    pub at: [f64; 2],
    /// The symbol's handle (hex).
    pub handle: String,
}

/// A match line on a page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageMatchLine {
    pub target_sheet: String,
    /// `true` for a line of constant x.
    pub vertical: bool,
    /// Its x (vertical) or y.
    pub at: f64,
}

/// What one page says about its links, kept small.
#[derive(Clone, Debug, Default)]
pub struct PageRead {
    pub callouts: Vec<Callout>,
    pub views: Vec<PageView>,
    pub boxes: Vec<[f64; 4]>,
    pub devices: Vec<PageDevice>,
    /// Device types its legends and schedules list.
    pub legend_types: BTreeSet<String>,
    pub match_lines: Vec<PageMatchLine>,
    /// The page's grid lines (the whole page).
    pub grid_x: Vec<(String, f64)>,
    pub grid_y: Vec<(String, f64)>,
    /// The page's scale, paper inches per foot (0 when none was printed).
    pub scale: f64,
    pub key_plan: bool,
}

/// An enlarged plan and where it sits on its level's overall plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Enlarged {
    /// The overall plan its boundary is drawn on (empty when none was found).
    pub on_sheet: String,
    /// The sheet the enlarged plan is drawn on.
    pub target_sheet: String,
    /// Its view number on that sheet.
    pub detail: String,
    pub title: String,
    pub level: String,
    pub registered: bool,
    /// `grids` (shared grid lines) or `boundary` (fitted into the boundary box with the scales).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub method: String,
    /// Enlarged sheet units per overall plan unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// The boundary on the overall plan `[x0, y0, x1, y1]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<[f64; 4]>,
    /// Devices the enlarged plan shows (it is the authority inside its boundary).
    pub devices: usize,
    pub devices_by_type: BTreeMap<String, usize>,
    /// The overall plan's devices inside the boundary, left out of the count.
    pub overall_ignored: usize,
}

/// Two plan sheets of one level a match line splits.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MatchPair {
    pub sheets: [String; 2],
    pub level: String,
    /// Both sides name each other and line up: one level.
    pub stitched: bool,
    /// Add to the second sheet's coordinates to put it beside the first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<[f64; 2]>,
}

/// A level and the plan sheets that show it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LevelLinks {
    pub level: String,
    pub sheets: Vec<String>,
    /// Its plans are one stitched plan (one sheet, or every split joined by match lines).
    pub stitched: bool,
    /// Devices counted on the level: the overall plans' outside the enlarged boundaries, plus
    /// the enlarged plans'.
    pub devices: usize,
}

/// A fact a detail prints, put on the devices its callout points from.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PrintFact {
    pub on_sheet: String,
    pub target_sheet: String,
    pub detail: String,
    /// `mounting_height`.
    pub kind: String,
    pub value_ft: f64,
    /// The devices it applies to: `{tag, handle}`.
    pub applied_to: Vec<serde_json::Value>,
}

/// A link that points nowhere.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Unresolved {
    pub on_sheet: String,
    pub kind: String,
    pub target_sheet: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
    pub at: [f64; 2],
}

/// The set's sheet map.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SheetLinks {
    pub sheets: Vec<SetSheet>,
    pub pages_read: usize,
    pub callouts: Vec<Callout>,
    pub resolved: usize,
    pub resolved_pct: f64,
    pub enlarged: Vec<Enlarged>,
    pub enlarged_registered: usize,
    pub match_lines: Vec<MatchPair>,
    pub levels: Vec<LevelLinks>,
    pub facts: Vec<PrintFact>,
    /// Elements inside an enlarged boundary counted from both sheets.
    pub double_counted: usize,
    pub unresolved: Vec<Unresolved>,
    pub key_plans: Vec<String>,
}

// ─── Sheet numbers ──────────────────────────────────────────────────────────

/// A sheet number in one spelling: capitals and digits, no dashes or spaces, no leading zeros
/// in the number (`E-06` and `E6` are `E6`; `A5.01` stays `A5.01`).
pub fn norm_sheet(s: &str) -> String {
    let t: String = s.trim().chars().filter(|c| !matches!(c, '-' | ' ' | '_')).collect::<String>().to_ascii_uppercase();
    let letters: String = t.chars().take_while(char::is_ascii_uppercase).collect();
    let rest = t.get(letters.len()..).unwrap_or("");
    let digits = rest.trim_start_matches('0');
    let digits = if digits.is_empty() || digits.starts_with('.') { format!("0{digits}") } else { digits.to_string() };
    format!("{letters}{digits}")
}

/// Whether a mark reads as a sheet number (`E-6`, `E6`, `A501`, `A5.01`, `FP-1.1`).
fn sheet_like(s: &str) -> bool {
    let t = s.trim().trim_end_matches(['.', ',', ')', ';', ':']);
    sheet_facts::is_sheet_number(t) || sheet_facts::is_sheet_number(&t.replace(' ', ""))
}

/// The sheet number at the start of `s` and what follows it.
fn leading_sheet(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    let end = s.find(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | ')' | '(')).unwrap_or(s.len());
    let word = s.get(..end)?.trim_end_matches(['.', ':']);
    sheet_like(word).then(|| (word.to_string(), s.get(end..).unwrap_or("")))
}

/// A detail and sheet written `3/E-501` at the start of `s`.
fn leading_detail(s: &str) -> Option<(String, String)> {
    let s = s.trim_start();
    let (d, rest) = s.split_once('/')?;
    let d = d.trim();
    if d.is_empty() || d.len() > 3 || !d.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let (sheet, _) = leading_sheet(rest)?;
    // `1/2"` and other fractions: a sheet number has letters.
    sheet.chars().next().is_some_and(|c| c.is_ascii_alphabetic()).then(|| (d.to_string(), sheet))
}

/// The discipline letter a discipline word names.
fn discipline_word(w: &str) -> Option<&'static str> {
    let w = w.trim_matches(|c: char| !c.is_ascii_alphabetic());
    Some(match w {
        "ARCHITECTURAL" | "ARCH" => "A",
        "STRUCTURAL" | "STRUCT" => "S",
        "MECHANICAL" | "MECH" | "HVAC" => "M",
        "ELECTRICAL" | "ELEC" => "E",
        "PLUMBING" | "PLBG" => "P",
        "CIVIL" => "C",
        "LANDSCAPE" => "L",
        "FIRE" => "FP",
        _ => return None,
    })
}

/// The written references in one line of lettering: `(kind, detail, target)`.
pub fn text_refs(text: &str) -> Vec<(&'static str, String, String)> {
    let up = text.to_ascii_uppercase().replace("SHEET NO.", "SHEET").replace("SHT.", "SHT");
    let mut out: Vec<(&'static str, String, String)> = Vec::new();
    let mut from = 0usize;
    let words = ["SEE ", "REFER TO ", "REF. ", "REFERENCE "];
    while out.len() < 8 {
        let Some((i, w)) = words.iter().filter_map(|w| up.get(from..)?.find(w).map(|i| (from + i, *w))).min_by_key(|(i, _)| *i) else { break };
        from = i + w.len();
        // `SEE` as part of a longer word (`OVERSEE`) is no reference.
        if i > 0 && up.get(..i).and_then(|p| p.chars().last()).is_some_and(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        let rest = up.get(from..).unwrap_or("").trim_start().trim_start_matches("THE ").trim_start();
        let after = |prefixes: &[&str]| prefixes.iter().find_map(|p| rest.strip_prefix(p)).map(str::trim_start);
        if let Some(r) = after(&["ENLARGED PLAN", "ENLARGED"]) {
            let r = r.trim_start_matches('S').trim_start();
            let r = r.strip_prefix("ON SHEET").or_else(|| r.strip_prefix("ON SHT")).or_else(|| r.strip_prefix("ON")).unwrap_or(r).trim_start();
            if let Some((d, sh)) = leading_detail(r) {
                out.push(("enlarged_ref", d, sh));
            } else if let Some((sh, _)) = leading_sheet(r) {
                out.push(("enlarged_ref", String::new(), sh));
            } else {
                out.push(("enlarged_ref", String::new(), String::new()));
            }
            continue;
        }
        if let Some(r) = after(&["DETAILS", "DETAIL", "DET.", "DET", "SECTION", "SECT.", "ELEVATION"]) {
            if let Some((d, sh)) = leading_detail(r) {
                out.push(("detail_ref", d, sh));
            } else if let Some((d, r2)) = r.split_once(" ON ")
                && d.trim().len() <= 3
                && let Some((sh, _)) = leading_sheet(r2.trim_start_matches("SHEET").trim_start_matches("SHT"))
            {
                out.push(("detail_ref", d.trim().to_string(), sh));
            }
            continue;
        }
        if let Some(r) = after(&["SHEETS", "SHEET", "SHTS", "SHT", "DRAWINGS", "DRAWING", "DWG.", "DWG"]) {
            if let Some((d, sh)) = leading_detail(r) {
                out.push(("detail_ref", d, sh));
            } else if let Some((sh, _)) = leading_sheet(r) {
                out.push(("sheet_ref", String::new(), sh));
            }
            continue;
        }
        if let Some((d, sh)) = leading_detail(rest) {
            out.push(("detail_ref", d, sh));
            continue;
        }
        if let Some((sh, _)) = leading_sheet(rest)
            && sh.chars().any(|c| c.is_ascii_digit())
        {
            out.push(("sheet_ref", String::new(), sh));
            continue;
        }
        if let Some(letter) = rest.split_whitespace().next().and_then(discipline_word) {
            out.push(("discipline_ref", String::new(), letter.to_string()));
        }
    }
    out
}

// ─── One page ───────────────────────────────────────────────────────────────

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Where a line of lettering's middle is (an import puts text at its baseline's start).
fn text_middle(t: &TextItem) -> [f64; 2] {
    let w = 0.35 * t.height * t.value.chars().count() as f64;
    let (s, c) = t.rotation.sin_cos();
    let (dx, dy) = (w, 0.5 * t.height);
    [t.at.x + dx * c - dy * s, t.at.y + dx * s + dy * c]
}

/// The straight pieces of a page: lines and the straight sides of polylines.
fn pieces(d: &Drawing) -> Vec<[[f64; 2]; 2]> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if out.len() >= MAX_PIECES {
            break;
        }
        match &e.kind {
            EntityKind::Line(l) => out.push([[l.a.x, l.a.y], [l.b.x, l.b.y]]),
            EntityKind::LwPolyline(p) => {
                let n = p.vertices.len();
                let sides = if p.closed { n } else { n.saturating_sub(1) };
                for i in 0..sides.min(4_096) {
                    let (Some(a), Some(b)) = (p.vertices.get(i), p.vertices.get((i + 1) % n.max(1))) else { continue };
                    if a.bulge.abs() < 1e-9 {
                        out.push([[a.p.x, a.p.y], [b.p.x, b.p.y]]);
                    }
                }
            }
            _ => {}
        }
    }
    out.retain(|s| s.iter().flatten().all(|v| v.is_finite()));
    out
}

/// Axis-aligned boxes drawn as one closed outline (an enlarged area's boundary, a key plan's
/// frame): `[x0, y0, x1, y1]`, at least `min` across.
fn boxes(d: &Drawing, min: f64) -> Vec<[f64; 4]> {
    let mut out = Vec::new();
    for e in d.model.iter() {
        if out.len() >= MAX_BOXES {
            break;
        }
        let EntityKind::LwPolyline(p) = &e.kind else { continue };
        let mut pts: Vec<[f64; 2]> = p.vertices.iter().take(6).map(|v| [v.p.x, v.p.y]).collect();
        if p.vertices.len() > 5 || p.vertices.iter().any(|v| v.bulge.abs() > 1e-9) {
            continue;
        }
        if pts.len() == 5 && pts.first().zip(pts.last()).is_some_and(|(a, b)| dist(*a, *b) < 1e-6) {
            pts.pop();
        } else if !p.closed {
            continue;
        }
        if pts.len() != 4 {
            continue;
        }
        let xs: Vec<f64> = pts.iter().map(|q| q[0]).collect();
        let ys: Vec<f64> = pts.iter().map(|q| q[1]).collect();
        let (x0, x1) = (xs.iter().copied().fold(f64::INFINITY, f64::min), xs.iter().copied().fold(f64::NEG_INFINITY, f64::max));
        let (y0, y1) = (ys.iter().copied().fold(f64::INFINITY, f64::min), ys.iter().copied().fold(f64::NEG_INFINITY, f64::max));
        let tol = 1e-3 * (x1 - x0).max(y1 - y0);
        let on_edge =
            pts.iter().all(|q| ((q[0] - x0).abs() < tol || (q[0] - x1).abs() < tol) && ((q[1] - y0).abs() < tol || (q[1] - y1).abs() < tol));
        if on_edge && x1 - x0 >= min && y1 - y0 >= min {
            out.push([x0, y0, x1, y1]);
        }
    }
    out
}

/// How far `p` is from the outline of box `b`.
fn box_edge_dist(b: &[f64; 4], p: [f64; 2]) -> f64 {
    let inside = p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3];
    if inside {
        (p[0] - b[0]).min(b[2] - p[0]).min(p[1] - b[1]).min(b[3] - p[1])
    } else {
        let dx = (b[0] - p[0]).max(0.0).max(p[0] - b[2]);
        let dy = (b[1] - p[1]).max(0.0).max(p[1] - b[3]);
        dx.hypot(dy)
    }
}

fn inside(b: &[f64; 4], p: [f64; 2]) -> bool {
    p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3]
}

/// A mounting height a note prints (`48" AFF`, `+18" A.F.F.`, `MOUNT AT 7'-6" AFF`), feet.
pub fn mounting_height(text: &str) -> Option<f64> {
    let up = text.to_ascii_uppercase();
    let at = ["A.F.F.", "A.F.F", "AFF", "ABOVE FINISHED FLOOR", "ABOVE FINISH FLOOR"].iter().find_map(|k| up.find(k))?;
    let before = up.get(..at)?.trim_end();
    // The feet-inches group right before it (`7' - 6"` printed with spaces too).
    let joined = before.rsplit(['@', '+', '=']).next().unwrap_or(before).replace(' ', "");
    let tail = joined.rsplit(|c: char| c.is_ascii_alphabetic() || matches!(c, ':' | ',' | '(')).next().unwrap_or("");
    let v = crate::heights::parse_feet_inches(tail)?;
    (v > 0.0 && v <= 30.0).then_some(v)
}

/// Read one page: its bubbles and marks, written references, boxes, views, devices and match
/// lines. `own` is the page's sheet number (bubbles naming it beside a view title are that
/// view's number).
pub fn read_page(d: &Drawing, own: &str) -> PageRead {
    let texts = one_copy(sheet_facts::texts(d));
    let mut heights: Vec<f64> = texts.iter().map(|t| t.height).collect();
    heights.sort_by(f64::total_cmp);
    let median_h = heights.get(heights.len() / 2).copied().unwrap_or(1.0).max(1e-9);
    let rounds = sheet_reader::round_shapes(d);
    let segs = pieces(d);
    let page_boxes = boxes(d, 12.0 * median_h);
    let views_raw = sheet_facts::view_titles(d, &texts);
    let own_n = norm_sheet(own);

    // Texts by cell for the bubble search.
    let cell = (median_h * 8.0).max(1e-6);
    let key = |p: [f64; 2]| ((p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64);
    let mut by_cell: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (i, t) in texts.iter().enumerate() {
        by_cell.entry(key(text_middle(t))).or_default().push(i);
    }
    let near_texts = |c: [f64; 2], r: f64| -> Vec<usize> {
        let k = (r / cell).ceil() as i64 + 1;
        let (cx, cy) = key(c);
        let mut v = Vec::new();
        for dx in -k..=k {
            for dy in -k..=k {
                if let Some(list) = by_cell.get(&(cx + dx, cy + dy)) {
                    v.extend(list.iter().copied());
                }
            }
        }
        v
    };
    // Straight pieces by cell, for dividers, leaders and cut lines.
    let seg_cell = cell * 2.0;
    let skey = |p: [f64; 2]| ((p[0] / seg_cell).floor() as i64, (p[1] / seg_cell).floor() as i64);
    let mut seg_cells: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    let mut budget = MAX_SEG_CELLS;
    for (i, s) in segs.iter().enumerate() {
        // Every cell along the piece (a long line is found by its middle too).
        let want = (dist(s[0], s[1]) / seg_cell).ceil().clamp(1.0, 2_000.0) as usize;
        let steps = if budget >= want { want } else { 1 };
        budget = budget.saturating_sub(steps);
        for k in 0..=steps {
            let f = k as f64 / steps as f64;
            let p = [s[0][0] + (s[1][0] - s[0][0]) * f, s[0][1] + (s[1][1] - s[0][1]) * f];
            let v = seg_cells.entry(skey(p)).or_default();
            if v.last() != Some(&i) {
                v.push(i);
            }
        }
    }
    let segs_near = |c: [f64; 2], r: f64| -> Vec<usize> {
        let k = (r / seg_cell).ceil() as i64 + 1;
        let (cx, cy) = skey(c);
        let mut v = Vec::new();
        for dx in -k..=k {
            for dy in -k..=k {
                if let Some(list) = seg_cells.get(&(cx + dx, cy + dy)) {
                    v.extend(list.iter().copied());
                }
            }
        }
        v.sort_unstable();
        v.dedup();
        v
    };

    let mut out = PageRead { boxes: page_boxes.clone(), ..PageRead::default() };
    let mut used_texts: BTreeSet<usize> = BTreeSet::new();
    let mut view_marks: Vec<(String, [f64; 2])> = Vec::new();
    // What the linework around a bubble at `c` (radius `r`) makes it: its kind, the box its
    // leader reaches, and whether a divider splits it.
    let shape = |c: [f64; 2], r: f64| -> (&'static str, Option<[f64; 4]>, bool) {
        let near = segs_near(c, r * 8.0);
        let piece = |i: &usize| segs.get(*i).copied();
        let divider = near.iter().filter_map(piece).any(|s| {
            let horizontal = (s[0][1] - s[1][1]).abs() < 0.1 * r;
            let across = (s[0][0] - s[1][0]).abs() > 1.2 * r;
            horizontal
                && across
                && ((s[0][1] + s[1][1]) / 2.0 - c[1]).abs() < 0.3 * r
                && dist([(s[0][0] + s[1][0]) / 2.0, (s[0][1] + s[1][1]) / 2.0], c) < 0.5 * r
        });
        // Pieces leaving the bubble: one end on its rim, the other well outside.
        let leaving: Vec<([f64; 2], f64)> = near
            .iter()
            .filter_map(piece)
            .filter_map(|s| {
                let (da, db) = (dist(s[0], c), dist(s[1], c));
                let len = dist(s[0], s[1]);
                if (da - r).abs() < 0.35 * r && db > 1.25 * r {
                    Some((s[1], len))
                } else if (db - r).abs() < 0.35 * r && da > 1.25 * r {
                    Some((s[0], len))
                } else {
                    None
                }
            })
            .collect();
        let tip =
            leaving.iter().any(|(far, len)| *len < 2.5 * r && leaving.iter().any(|(f2, l2)| *l2 < 2.5 * r && f2 != far && dist(*far, *f2) < 0.1 * r));
        let cut = leaving.iter().any(|(_, len)| *len >= 4.0 * r);
        let enlarged_word = near_texts(c, 8.0 * r)
            .iter()
            .filter_map(|i| texts.get(*i))
            .any(|t| t.value.to_ascii_uppercase().contains("ENLARGED") && dist(text_middle(t), c) < 8.0 * r);
        let edge_box = |p: [f64; 2], within: f64| {
            page_boxes
                .iter()
                .filter(|b| box_edge_dist(b, p) < within && !inside(b, c))
                .min_by(|a, b| box_edge_dist(a, p).total_cmp(&box_edge_dist(b, p)))
                .copied()
        };
        let boundary = leaving
            .iter()
            .filter(|(_, len)| *len >= 1.5 * r)
            .find_map(|(far, _)| edge_box(*far, 0.5 * r))
            .or_else(|| if enlarged_word { edge_box(c, 3.0 * r) } else { None });
        let kind = if boundary.is_some() || enlarged_word {
            "enlarged_plan"
        } else if cut {
            "section_mark"
        } else if tip {
            "elevation_tag"
        } else {
            "detail_bubble"
        };
        (kind, boundary, divider)
    };
    let detail_like = |v: &str| !v.is_empty() && v.len() <= 4 && v.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '.');
    // A bubble naming its own sheet beside a view title numbers that view.
    let view_mark =
        |target: &str, c: [f64; 2], r: f64| !own_n.is_empty() && norm_sheet(target) == own_n && views_raw.iter().any(|v| dist(v.at, c) < 12.0 * r);
    for &(c, r) in &rounds {
        if out.callouts.len() >= MAX_PAGE_CALLOUTS {
            break;
        }
        if r < 0.8 * median_h || r > 40.0 * median_h {
            continue;
        }
        let inner: Vec<usize> = near_texts(c, r)
            .into_iter()
            .filter(|&i| texts.get(i).is_some_and(|t| dist(text_middle(t), c) <= 0.98 * r && t.height < 1.4 * r))
            .collect();
        if inner.is_empty() || inner.len() > 12 {
            continue;
        }
        // Lettering read back from strokes comes in pieces too: each half's best reading, the
        // one most copies agree on.
        let mid = |i: &usize| texts.get(*i).map_or(c, text_middle);
        let best = |upper: bool, ok: &dyn Fn(&str) -> bool| -> Option<String> {
            let mut votes: BTreeMap<String, usize> = BTreeMap::new();
            for i in inner.iter().filter(|i| (mid(i)[1] > c[1]) == upper) {
                if let Some(t) = texts.get(*i).map(|t| t.value.trim().to_string()).filter(|v| ok(v)) {
                    *votes.entry(t).or_default() += 1;
                }
            }
            votes.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(a.0.len().cmp(&b.0.len()))).map(|(v, _)| v)
        };
        let (detail, target) = match (best(true, &detail_like), best(false, &|v| sheet_like(v))) {
            (Some(dd), Some(sh)) => (dd, sh),
            (None, Some(sh)) => (String::new(), sh),
            // One line `3/E-501`.
            _ => match inner.iter().filter_map(|i| texts.get(*i)).find_map(|t| leading_detail(&t.value)) {
                Some(found) => found,
                None => continue,
            },
        };
        used_texts.extend(inner.iter().copied());
        if view_mark(&target, c, r) {
            view_marks.push((detail, c));
            continue;
        }
        let (kind, boundary, divider) = shape(c, r);
        out.callouts.push(Callout {
            kind: kind.into(),
            detail,
            target_sheet: target.trim().to_string(),
            at: c,
            radius: r,
            divider,
            boundary,
            ..Callout::default()
        });
    }
    // Bubbles whose circle the import took for lettering: a detail number right over a sheet
    // number, centred on one another.
    for (bi, b) in texts.iter().enumerate() {
        if out.callouts.len() >= MAX_PAGE_CALLOUTS {
            break;
        }
        let v = b.value.trim();
        if used_texts.contains(&bi) || v.len() > 8 || !sheet_like(v) || b.rotation.sin().abs() > 0.05 {
            continue;
        }
        let bm = text_middle(b);
        let top = near_texts(bm, 4.5 * b.height)
            .into_iter()
            .filter(|i| *i != bi && !used_texts.contains(i))
            .filter_map(|i| texts.get(i).map(|t| (i, t)))
            .filter(|(_, t)| {
                let m = text_middle(t);
                let dy = m[1] - bm[1];
                detail_like(t.value.trim())
                    && dy > 0.6 * b.height
                    && dy < 4.5 * b.height
                    && (m[0] - bm[0]).abs() < 1.5 * b.height
                    && (0.6..1.7).contains(&(t.height / b.height))
            })
            .min_by(|a, b2| (text_middle(a.1)[1] - bm[1]).total_cmp(&(text_middle(b2.1)[1] - bm[1])));
        let Some((ti, t)) = top else { continue };
        let tm = text_middle(t);
        let c = [(tm[0] + bm[0]) / 2.0, (tm[1] + bm[1]) / 2.0];
        let r = (tm[1] - bm[1]) * 1.1;
        used_texts.insert(bi);
        used_texts.insert(ti);
        if view_mark(v, c, r) {
            view_marks.push((t.value.trim().to_string(), c));
            continue;
        }
        let (kind, boundary, _) = shape(c, r);
        out.callouts.push(Callout {
            kind: kind.into(),
            detail: t.value.trim().to_string(),
            target_sheet: v.to_string(),
            at: c,
            radius: r,
            divider: true,
            boundary,
            ..Callout::default()
        });
    }

    // Written references, match lines and key plans.
    let lines: Vec<(usize, &TextItem)> = texts.iter().enumerate().take(MAX_REF_TEXTS).collect();
    let mut seen_refs: BTreeSet<(String, String, String)> = BTreeSet::new();
    for (i, t) in &lines {
        if used_texts.contains(i) || out.callouts.len() >= MAX_PAGE_CALLOUTS {
            continue;
        }
        let up = t.value.to_ascii_uppercase();
        if up.contains("KEY PLAN") {
            out.key_plan = true;
        }
        // A reference broken over two lines: the line under it finishes it.
        let mut value = t.value.clone();
        let trimmed = up.trim_end();
        if ["SEE", "SHEET", "SHT", "DETAIL", "REFER TO", "ON", "PLAN"].iter().any(|w| trimmed.ends_with(w))
            && let Some((_, next)) = lines
                .iter()
                .filter(|(_, u)| (u.at.x - t.at.x).abs() < 4.0 * t.height && t.at.y - u.at.y > 0.6 * t.height && t.at.y - u.at.y < 2.2 * t.height)
                .min_by(|a, b| (t.at.y - a.1.at.y).total_cmp(&(t.at.y - b.1.at.y)))
        {
            value = format!("{} {}", t.value, next.value);
        }
        let vu = value.to_ascii_uppercase();
        if vu.contains("MATCH LINE") || vu.contains("MATCHLINE") {
            let target = text_refs(&value).into_iter().find(|r| !r.2.is_empty()).map(|r| r.2).or_else(|| {
                // `MATCH LINE` with `SEE SHEET E-3` on its own line beside it.
                lines
                    .iter()
                    .filter(|(_, u)| dist(text_middle(u), text_middle(t)) < 4.0 * t.height)
                    .find_map(|(_, u)| text_refs(&u.value).into_iter().find(|r| !r.2.is_empty()).map(|r| r.2))
            });
            let Some(target) = target else { continue };
            // The long line it runs along.
            let m = text_middle(t);
            let along = segs_near(m, 5.0 * t.height)
                .into_iter()
                .filter_map(|k| segs.get(k).copied())
                .filter(|s| dist(s[0], s[1]) >= 15.0 * t.height)
                .filter(|s| {
                    let (a, b) = (s[0], s[1]);
                    let len = dist(a, b).max(1e-9);
                    let cross = ((b[0] - a[0]) * (m[1] - a[1]) - (b[1] - a[1]) * (m[0] - a[0])).abs() / len;
                    cross < 5.0 * t.height
                })
                .max_by(|a, b| dist(a[0], a[1]).total_cmp(&dist(b[0], b[1])));
            if let Some(s) = along {
                let vertical = (s[0][0] - s[1][0]).abs() < (s[0][1] - s[1][1]).abs();
                let at = if vertical { (s[0][0] + s[1][0]) / 2.0 } else { (s[0][1] + s[1][1]) / 2.0 };
                out.match_lines.push(PageMatchLine { target_sheet: target.clone(), vertical, at });
                out.callouts.push(Callout { kind: "match_line".into(), target_sheet: target, at: m, text: value.clone(), ..Callout::default() });
            }
            continue;
        }
        for (kind, detail, target) in text_refs(&value) {
            // Notes repeat `REFER TO ARCHITECTURAL`: one per sheet and target.
            if !seen_refs.insert((kind.to_string(), detail.clone(), target.clone())) && kind == "discipline_ref" {
                continue;
            }
            if out.callouts.len() >= MAX_PAGE_CALLOUTS {
                break;
            }
            out.callouts.push(Callout {
                kind: kind.into(),
                detail,
                target_sheet: target,
                at: text_middle(t),
                text: value.clone(),
                ..Callout::default()
            });
        }
    }

    // Views: their regions, grids and printed heights.
    let page = sheet_reader::recognise_dxf(d);
    out.scale = page.scale;
    out.grid_x = page.grid_x.iter().map(|g| (g.label.clone(), g.at_in)).collect();
    out.grid_y = page.grid_y.iter().map(|g| (g.label.clone(), g.at_in)).collect();
    let blocks = if views_raw.is_empty() { Vec::new() } else { sheet_facts::view_regions(d) };
    for v in &views_raw {
        let h = texts.iter().find(|t| t.value == v.title && [t.at.x, t.at.y] == v.at).map_or(median_h, |t| t.height);
        // The view's number: its own bubble's (a print's lettering may come back in pieces).
        let mark = view_marks
            .iter()
            .filter(|(_, c)| c[0] <= v.at[0] && dist(*c, v.at) < 12.0 * h)
            .min_by(|a, b| dist(a.1, v.at).total_cmp(&dist(b.1, v.at)));
        let mut pv = PageView {
            number: mark.map(|m| m.0.clone()).or_else(|| v.number.clone()).unwrap_or_default(),
            title: v.title.clone(),
            scale_text: v.scale_text.clone(),
            region: view_block(&blocks, v.at, h).or(v.region),
            ..PageView::default()
        };
        if let Some(rg) = pv.region {
            // Grid bubbles stand outside the walls: look a little past the view for them.
            let m = 0.15 * (rg[2] - rg[0]).max(rg[3] - rg[1]);
            let wide = [rg[0] - m, rg[1] - m, rg[2] + m, rg[3] + m];
            let sub = sheet_reader::crop(d, &|p: [f64; 2]| inside(&wide, p));
            let rec = sheet_reader::recognise_dxf(&sub);
            pv.grid_x = rec.grid_x.iter().map(|g| (g.label.clone(), g.at_in)).collect();
            pv.grid_y = rec.grid_y.iter().map(|g| (g.label.clone(), g.at_in)).collect();
            pv.mounting_ft = texts.iter().filter(|t| inside(&rg, [t.at.x, t.at.y])).filter_map(|t| mounting_height(&t.value)).take(16).collect();
        }
        out.views.push(pv);
    }

    // Devices: the import's symbols with the mark beside them; legends say which are samples.
    let legend = device_types::read_legend(&texts);
    out.legend_types = legend.types.clone();
    for e in d.model.iter() {
        if out.devices.len() >= MAX_PAGE_DEVICES {
            break;
        }
        let EntityKind::Insert(ins) = &e.kind else { continue };
        if !ins.block.starts_with("PDF-SYM-") {
            continue;
        }
        let tag = ins.attribs.iter().find(|a| a.tag == "TAG").map(|a| a.text.value.trim().to_string()).unwrap_or_default();
        let at = ins.insert.xy();
        if tag.is_empty() || legend.covers(at) || is_circuit_tag(&tag) {
            continue;
        }
        out.devices.push(PageDevice { tag, at: [at.x, at.y], handle: e.handle.hex() });
    }
    out
}

/// One copy of each line of lettering: an import may give a mark twice (the page's hidden text
/// and the strokes read back), in the same place.
fn one_copy(texts: Vec<TextItem>) -> Vec<TextItem> {
    let mut seen: BTreeSet<(String, i64, i64)> = BTreeSet::new();
    texts
        .into_iter()
        .filter(|t| {
            let q = (t.height * 0.25).max(1e-9);
            seen.insert((t.value.trim().to_string(), (t.at.x / q).round() as i64, (t.at.y / q).round() as i64))
        })
        .collect()
}

/// The block of linework a view title labels: the biggest block standing above the title
/// (or holding it) over the title's start, grown by the small blocks touching it (grid bubbles,
/// tags just outside its lines).
fn view_block(blocks: &[([f64; 4], usize)], at: [f64; 2], h: f64) -> Option<[f64; 4]> {
    let holds = |r: &[f64; 4]| inside(r, at);
    let above = |r: &[f64; 4]| r[1] >= at[1] - h && r[1] - at[1] <= 30.0 * h && r[0] <= at[0] + 4.0 * h && at[0] <= r[2];
    let (mut b, _) = blocks.iter().filter(|(r, _)| holds(r) || above(r)).max_by_key(|(_, n)| *n).copied()?;
    let pad = 2.0 * h;
    for _ in 0..4 {
        let mut grew = false;
        for (r, _) in blocks {
            let touches = r[0] <= b[2] + pad && r[2] >= b[0] - pad && r[1] <= b[3] + pad && r[3] >= b[1] - pad;
            let small = (r[2] - r[0]) * (r[3] - r[1]) < 0.1 * (b[2] - b[0]) * (b[3] - b[1]);
            let grows = r[0] < b[0] || r[1] < b[1] || r[2] > b[2] || r[3] > b[3];
            if touches && small && grows && !holds(r) {
                b = [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])];
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    Some(b)
}

/// A circuit or panel reference (`D-1`, `LP-12`): a wiring label, not a type.
fn is_circuit_tag(tag: &str) -> bool {
    tag.rsplit_once('-').is_some_and(|(head, n)| !head.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

// ─── The set ────────────────────────────────────────────────────────────────

/// A transform `p ↦ s·p + t` taking enlarged-sheet coordinates onto the overall plan.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fit {
    s: f64,
    t: [f64; 2],
}

impl Fit {
    fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        [self.s * p[0] + self.t[0], self.s * p[1] + self.t[1]]
    }
}

/// Scale and offset along one axis from grid lines both views label.
fn axis_fit(from: &[(String, f64)], to: &[(String, f64)]) -> (Option<f64>, Vec<(f64, f64)>) {
    let pairs: Vec<(f64, f64)> = from.iter().filter_map(|(l, a)| to.iter().find(|(m, _)| m == l).map(|(_, b)| (*a, *b))).collect();
    let mut scales: Vec<f64> = Vec::new();
    for (i, p) in pairs.iter().enumerate() {
        for q in pairs.iter().skip(i + 1) {
            let da = q.0 - p.0;
            if da.abs() > 1e-9 {
                scales.push((q.1 - p.1) / da);
            }
        }
    }
    scales.retain(|s| s.is_finite() && *s > 0.0);
    scales.sort_by(f64::total_cmp);
    (scales.get(scales.len() / 2).copied(), pairs)
}

/// Register an enlarged view onto an overall plan by the grid lines both label.
fn grid_fit(view: &PageView, host_x: &[(String, f64)], host_y: &[(String, f64)]) -> Option<Fit> {
    let (sx, px) = axis_fit(&view.grid_x, host_x);
    let (sy, py) = axis_fit(&view.grid_y, host_y);
    let s = match (sx, sy) {
        (Some(a), Some(b)) if (a / b - 1.0).abs() < 0.05 => (a + b) / 2.0,
        (Some(a), Some(_)) | (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };
    let tx = px.iter().map(|(a, b)| b - s * a).sum::<f64>() / px.len().max(1) as f64;
    let ty = py.iter().map(|(a, b)| b - s * a).sum::<f64>() / py.len().max(1) as f64;
    (!px.is_empty() && !py.is_empty() && s.is_finite()).then_some(Fit { s, t: [tx, ty] })
}

fn map_box(f: &Fit, b: &[f64; 4]) -> [f64; 4] {
    let (p, q) = (f.apply([b[0], b[1]]), f.apply([b[2], b[3]]));
    [p[0].min(q[0]), p[1].min(q[1]), p[0].max(q[0]), p[1].max(q[1])]
}

/// A page of the set as read: its index entry and what it says.
pub struct LinkPage {
    pub sheet: SetSheet,
    pub read: PageRead,
}

/// Put the pages together into the set's sheet map. `sheets` is every sheet of the set (the
/// index), `pages` the ones read.
pub fn link_set(sheets: &[SetSheet], pages: &[LinkPage]) -> SheetLinks {
    let numbers: BTreeSet<String> = sheets.iter().map(|s| norm_sheet(&s.number)).collect();
    let disciplines: BTreeSet<String> = sheets.iter().map(|s| s.discipline.to_ascii_uppercase()).collect();
    let by_number = |n: &str| -> Option<&LinkPage> {
        let k = norm_sheet(n);
        pages.iter().find(|p| norm_sheet(&p.sheet.number) == k)
    };
    let display = |n: &str| -> String {
        let k = norm_sheet(n);
        sheets.iter().find(|s| norm_sheet(&s.number) == k).map_or_else(|| n.trim().to_string(), |s| s.number.clone())
    };
    let mut out = SheetLinks { sheets: sheets.to_vec(), pages_read: pages.len(), ..SheetLinks::default() };

    // Enlarged views: titled ENLARGED, or the plan views of an enlarged-plan sheet.
    struct EnlView<'a> {
        page: &'a LinkPage,
        view: &'a PageView,
    }
    let mut enl_views: Vec<EnlView> = Vec::new();
    for p in pages {
        for v in &p.read.views {
            let t = v.title.to_ascii_uppercase();
            if t.contains("ENLARGED") || (p.sheet.sheet_type == "enlarged_plan" && t.contains("PLAN")) {
                enl_views.push(EnlView { page: p, view: v });
            }
        }
    }

    // Callouts, resolved.
    for p in pages {
        for c in &p.read.callouts {
            if out.callouts.len() >= MAX_SET_CALLOUTS {
                break;
            }
            let mut c = c.clone();
            c.on_sheet = p.sheet.number.clone();
            c.resolved = match c.kind.as_str() {
                "discipline_ref" => disciplines.contains(&c.target_sheet),
                "enlarged_ref" if c.target_sheet.is_empty() => {
                    // `SEE ENLARGED PLAN` naming no sheet: the set's enlarged plan of this level.
                    let found = enl_views.iter().find(|e| {
                        e.page.sheet.discipline == p.sheet.discipline && (p.sheet.level.is_empty() || level_of_view(e.page, e.view) == p.sheet.level)
                    });
                    if let Some(e) = found {
                        c.target_sheet = e.page.sheet.number.clone();
                        c.detail = e.view.number.clone();
                    }
                    found.is_some()
                }
                _ => numbers.contains(&norm_sheet(&c.target_sheet)),
            };
            // A bubble with no divider pointing nowhere is lettering in a circle, not a callout.
            if c.radius > 0.0 && !c.divider && !c.resolved {
                continue;
            }
            if c.resolved && c.kind != "discipline_ref" {
                c.target_sheet = display(&c.target_sheet);
                if !c.detail.is_empty()
                    && let Some(tp) = by_number(&c.target_sheet)
                {
                    c.detail_found = Some(tp.read.views.iter().any(|v| v.number == c.detail));
                }
            }
            if !c.resolved {
                out.unresolved.push(Unresolved {
                    on_sheet: c.on_sheet.clone(),
                    kind: c.kind.clone(),
                    target_sheet: c.target_sheet.clone(),
                    detail: c.detail.clone(),
                    at: c.at,
                });
            }
            out.callouts.push(c);
        }
        if p.read.key_plan {
            out.key_plans.push(p.sheet.number.clone());
        }
    }
    out.resolved = out.callouts.iter().filter(|c| c.resolved).count();
    out.resolved_pct = if out.callouts.is_empty() { 0.0 } else { (out.resolved as f64 / out.callouts.len() as f64 * 1000.0).round() / 1000.0 };

    // Device types: every legend read, and the set's sheet numbers are never types.
    let legend: BTreeSet<String> = pages.iter().flat_map(|p| p.read.legend_types.iter().cloned()).collect();
    let mut sheet_forms: BTreeSet<String> = BTreeSet::new();
    for s in sheets {
        sheet_forms.extend(device_types::sheet_number_forms(&s.number));
    }
    let none = BTreeSet::new();
    let device_type = |tag: &str| match device_types::tag_verdict(tag, &legend, &none, &sheet_forms) {
        TagVerdict::Device(t) => Some(t),
        _ => None,
    };

    // Enlarged plans, registered on their level's overall plan.
    let mut boundaries: BTreeMap<String, Vec<[f64; 4]>> = BTreeMap::new();
    for e in &enl_views {
        let target = e.page.sheet.number.clone();
        let detail = e.view.number.clone();
        let mut level = level_of_view(e.page, e.view);
        let mut entry = Enlarged { target_sheet: target.clone(), detail: detail.clone(), title: e.view.title.clone(), ..Enlarged::default() };
        // Its devices: the enlarged sheet's symbols inside the view.
        if let Some(rg) = e.view.region {
            // Devices just outside the walls (lights on the facade) are the view's too.
            let m = 0.08 * (rg[2] - rg[0]).max(rg[3] - rg[1]);
            let reach = [rg[0] - m, rg[1] - m, rg[2] + m, rg[3] + m];
            for dv in e.page.read.devices.iter().filter(|dv| inside(&reach, dv.at)) {
                if let Some(t) = device_type(&dv.tag) {
                    *entry.devices_by_type.entry(t).or_default() += 1;
                    entry.devices += 1;
                }
            }
        }
        // The overall plan: the sheet whose callout points here, else a plan of the level.
        let points_here =
            |c: &Callout| norm_sheet(&c.target_sheet) == norm_sheet(&target) && (c.detail.is_empty() || detail.is_empty() || c.detail == detail);
        let host_callout = pages
            .iter()
            .filter(|p| p.sheet.number != target && matches!(p.sheet.sheet_type.as_str(), "plan" | "rcp" | "enlarged_plan" | "unknown"))
            .flat_map(|p| {
                p.read
                    .callouts
                    .iter()
                    .filter(|c| points_here(c) && matches!(c.kind.as_str(), "enlarged_plan" | "enlarged_ref" | "detail_bubble" | "detail_ref"))
                    .map(move |c| (p, c))
            })
            .max_by_key(|(p, c)| {
                (c.boundary.is_some(), c.kind == "enlarged_plan", p.sheet.discipline == e.page.sheet.discipline, p.sheet.level == level)
            });
        let host = host_callout.map(|(p, _)| p).or_else(|| {
            pages
                .iter()
                .filter(|p| p.sheet.number != target && p.sheet.sheet_type == "plan" && !level.is_empty() && p.sheet.level == level)
                .max_by_key(|p| (p.sheet.discipline == e.page.sheet.discipline, p.read.devices.len()))
        });
        if let Some(h) = host {
            entry.on_sheet = h.sheet.number.clone();
            if level.is_empty() {
                level = h.sheet.level.clone();
            }
            // The boundary box: the callout's leader reaches it, or the box a reference sits by.
            let boxed = host_callout.and_then(|(_, c)| {
                c.boundary.or_else(|| {
                    let r = if c.radius > 0.0 { c.radius } else { 1.0 };
                    h.read
                        .boxes
                        .iter()
                        .filter(|b| box_edge_dist(b, c.at) < 6.0 * r)
                        .min_by(|a, b| box_edge_dist(a, c.at).total_cmp(&box_edge_dist(b, c.at)))
                        .copied()
                })
            });
            let fit = grid_fit(e.view, &h.read.grid_x, &h.read.grid_y).map(|f| (f, "grids")).or_else(|| {
                let (b, rg) = (boxed?, e.view.region?);
                let (bw, bh, rw, rh) = (b[2] - b[0], b[3] - b[1], rg[2] - rg[0], rg[3] - rg[1]);
                if !(bw > 0.0 && bh > 0.0 && rw > 0.0 && rh > 0.0) {
                    return None;
                }
                // The scales printed say how much bigger the enlarged plan is; else the box does.
                let view_scale = e.view.scale_text.as_deref().and_then(sheet_reader::parse_scale);
                let s = match view_scale {
                    Some(vs) if vs > 0.0 && h.read.scale > 0.0 => h.read.scale / vs,
                    _ => (bw / rw).min(bh / rh),
                };
                let cb = [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0];
                let cr = [(rg[0] + rg[2]) / 2.0, (rg[1] + rg[3]) / 2.0];
                Some((Fit { s, t: [cb[0] - s * cr[0], cb[1] - s * cr[1]] }, "boundary"))
            });
            if let Some((f, how)) = fit {
                entry.registered = true;
                entry.method = how.into();
                entry.scale = Some((1.0 / f.s * 1000.0).round() / 1000.0);
                let b = boxed.or_else(|| e.view.region.map(|rg| map_box(&f, &rg)));
                entry.boundary = b;
                if let Some(b) = b {
                    entry.overall_ignored = h.read.devices.iter().filter(|dv| inside(&b, dv.at) && device_type(&dv.tag).is_some()).count();
                    boundaries.entry(h.sheet.number.clone()).or_default().push(b);
                }
            }
        }
        entry.level = level;
        out.enlarged.push(entry);
    }
    out.enlarged_registered = out.enlarged.iter().filter(|e| e.registered).count();

    // Per level: the overall plans' devices outside the boundaries, plus the enlarged plans'.
    let mut levels: BTreeMap<String, LevelLinks> = BTreeMap::new();
    for p in pages.iter().filter(|p| p.sheet.sheet_type == "plan" && !p.sheet.level.is_empty()) {
        let l = levels.entry(p.sheet.level.clone()).or_insert_with(|| LevelLinks { level: p.sheet.level.clone(), ..LevelLinks::default() });
        l.sheets.push(p.sheet.number.clone());
        let bounds = boundaries.get(&p.sheet.number).cloned().unwrap_or_default();
        for dv in &p.read.devices {
            if device_type(&dv.tag).is_none() {
                continue;
            }
            if bounds.iter().any(|b| inside(b, dv.at)) {
                continue;
            }
            l.devices += 1;
        }
    }
    let mut counted_inside = 0usize;
    for e in &out.enlarged {
        if let Some(l) = levels.get_mut(&e.level) {
            l.devices += e.devices;
        }
        // What the tally kept from the overall plan inside this boundary (0: the enlarged
        // sheet is the authority there).
        if let (Some(b), Some(h)) = (e.boundary, by_number(&e.on_sheet))
            && e.registered
        {
            let kept = boundaries.get(&h.sheet.number).is_none_or(|bs| !bs.contains(&b));
            if kept {
                counted_inside += h.read.devices.iter().filter(|dv| inside(&b, dv.at) && device_type(&dv.tag).is_some()).count();
            }
        }
    }
    out.double_counted = counted_inside;

    // Match lines: two plans of one level that name each other.
    let mut pairs: Vec<MatchPair> = Vec::new();
    for p in pages {
        for m in &p.read.match_lines {
            let Some(q) = by_number(&m.target_sheet) else { continue };
            let names = [p.sheet.number.clone(), q.sheet.number.clone()];
            if pairs.iter().any(|x| x.sheets.contains(&names[0]) && x.sheets.contains(&names[1])) {
                continue;
            }
            let back = q.read.match_lines.iter().find(|n| norm_sheet(&n.target_sheet) == norm_sheet(&p.sheet.number) && n.vertical == m.vertical);
            let level = if p.sheet.level.is_empty() { q.sheet.level.clone() } else { p.sheet.level.clone() };
            let same_level = p.sheet.level.is_empty() || q.sheet.level.is_empty() || p.sheet.level == q.sheet.level;
            let offset = back.filter(|_| same_level).map(|n| {
                // Across the line: the two lines meet. Along it: shared grid lines, if any.
                let (ref_a, ref_b) = if m.vertical { (&p.read.grid_y, &q.read.grid_y) } else { (&p.read.grid_x, &q.read.grid_x) };
                let along = ref_a.iter().find_map(|(l, a)| ref_b.iter().find(|(k, _)| k == l).map(|(_, b)| a - b)).unwrap_or(0.0);
                let across = m.at - n.at;
                if m.vertical { [across, along] } else { [along, across] }
            });
            pairs.push(MatchPair { sheets: names, level, stitched: offset.is_some(), offset });
        }
    }
    for l in levels.values_mut() {
        l.stitched =
            l.sheets.len() <= 1 || l.sheets.iter().all(|s| pairs.iter().any(|m| m.stitched && m.sheets.contains(s))) || plans_of_one_page(&l.sheets);
    }
    out.match_lines = pairs;
    out.levels = levels.into_values().collect();

    // Facts the details print, on the devices their callouts point from.
    for p in pages {
        for c in p
            .read
            .callouts
            .iter()
            .filter(|c| !c.detail.is_empty() && matches!(c.kind.as_str(), "detail_bubble" | "detail_ref" | "section_mark" | "elevation_tag"))
        {
            let Some(tp) = by_number(&c.target_sheet) else { continue };
            let Some(v) = tp.read.views.iter().find(|v| v.number == c.detail) else { continue };
            let Some(&h) = v.mounting_ft.first() else { continue };
            let reach = if c.radius > 0.0 { 6.0 * c.radius } else { 12.0 };
            let mut near: Vec<&PageDevice> =
                p.read.devices.iter().filter(|dv| dist(dv.at, c.at) <= reach && device_type(&dv.tag).is_some()).collect();
            near.sort_by(|a, b| dist(a.at, c.at).total_cmp(&dist(b.at, c.at)));
            let applied: Vec<serde_json::Value> = near.iter().take(1).map(|dv| serde_json::json!({"tag": dv.tag, "handle": dv.handle})).collect();
            if out.facts.len() < MAX_SET_CALLOUTS {
                out.facts.push(PrintFact {
                    on_sheet: p.sheet.number.clone(),
                    target_sheet: tp.sheet.number.clone(),
                    detail: c.detail.clone(),
                    kind: "mounting_height".into(),
                    value_ft: (h * 1000.0).round() / 1000.0,
                    applied_to: applied,
                });
            }
        }
    }
    out
}

/// The views of one several-view sheet (`E-2/1`, `E-2/2`): one page, already one plan each.
fn plans_of_one_page(sheets: &[String]) -> bool {
    let base = |s: &String| s.split('/').next().unwrap_or(s).to_string();
    sheets.first().is_some_and(|f| sheets.iter().all(|s| base(s) == base(f)) && sheets.len() > 1 && f.contains('/'))
}

/// The level an enlarged view shows: its title's, else its sheet's.
fn level_of_view(p: &LinkPage, v: &PageView) -> String {
    sheet_reader::detect_levels(&v.title).into_iter().next().unwrap_or_else(|| p.sheet.level.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_numbers_compare_in_one_spelling() {
        assert_eq!(norm_sheet("E-6"), "E6");
        assert_eq!(norm_sheet("e 06"), "E6");
        assert_eq!(norm_sheet("A5.01"), "A5.01");
        assert_eq!(norm_sheet("FP-1.1"), "FP1.1");
        assert_eq!(norm_sheet("A-0.1"), "A0.1");
    }

    #[test]
    fn written_references_are_read() {
        assert_eq!(text_refs("SEE SHEET E-501 FOR RISER"), vec![("sheet_ref", String::new(), "E-501".into())]);
        assert_eq!(text_refs("MOUNT PER DETAIL, SEE DETAIL 3/E-501."), vec![("detail_ref", "3".into(), "E-501".into())]);
        assert_eq!(text_refs("REFER TO ARCHITECTURAL DRAWINGS FOR LOCATIONS"), vec![("discipline_ref", String::new(), "A".into())]);
        assert_eq!(text_refs("SEE ENLARGED PLAN 1/E-6"), vec![("enlarged_ref", "1".into(), "E-6".into())]);
        assert_eq!(text_refs("SEE ENLARGED PLAN ON SHEET E-6"), vec![("enlarged_ref", String::new(), "E-6".into())]);
        assert_eq!(text_refs("SEE 2/A-501"), vec![("detail_ref", "2".into(), "A-501".into())]);
        assert_eq!(text_refs("DETAIL 4 ON SHEET E-7"), vec![]);
        assert_eq!(text_refs("SEE DETAIL 4 ON SHEET E-7"), vec![("detail_ref", "4".into(), "E-7".into())]);
        // No reference: a fraction, a sheet-less note, a word containing SEE.
        assert!(text_refs("1/2\" CONDUIT, SEE NOTE 4").is_empty());
        assert!(text_refs("OVERSEEN BY OWNER").is_empty());
        assert!(text_refs("SEE SPECIFICATIONS").is_empty());
    }

    #[test]
    fn mounting_heights_are_read_from_notes() {
        assert_eq!(mounting_height("MOUNT AT 48\" AFF"), Some(4.0));
        assert_eq!(mounting_height("+18\" A.F.F."), Some(1.5));
        assert_eq!(mounting_height("7'-6\" AFF TO BOTTOM"), Some(7.5));
        assert_eq!(mounting_height("PROVIDE BOX"), None);
    }
}
