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
use crate::linefit::{self, AxisSeg, LineFit};
use crate::sheet_facts::{self, TextItem};
use crate::sheet_reader;
use crate::tagid::{self, Sim};
use cadaddy_doc::{Drawing, EntityKind};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Most callouts kept per page (hostile files).
const MAX_PAGE_CALLOUTS: usize = 2_000;
/// Most devices kept per page.
const MAX_PAGE_DEVICES: usize = 20_000;
/// Most door and room numbers kept per page.
const MAX_PAGE_TAGS: usize = 20_000;
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
/// View-title words that never name a plan.
const NOT_PLAN_WORDS: [&str; 11] =
    ["SCHEDULE", "DIAGRAM", "RISER", "SECTION", "ELEVATION", "NOTES", "LEGEND", "DETAIL", "ONE-LINE", "ONE LINE", "KEY PLAN"];

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
    /// The bubble's radius, or a written reference's lettering height.
    #[serde(skip)]
    pub size: f64,
    /// What its shape alone makes it (a bubble whose leader reaches a box is only an enlarged
    /// plan's callout if the sheet it names is one).
    #[serde(skip)]
    pub shape_kind: String,
    /// "ENLARGED" is lettered beside it.
    #[serde(skip)]
    pub enlarged_word: bool,
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
    /// Boxes drawn dashed (rows of dashes): how an enlarged area's boundary is usually drawn.
    pub dash_boxes: Vec<[f64; 4]>,
    pub devices: Vec<PageDevice>,
    /// Where the symbols with no mark beside them are (legend samples left out).
    pub untagged: Vec<[f64; 2]>,
    /// Device types its legends and schedules list.
    pub legend_types: BTreeSet<String>,
    pub match_lines: Vec<PageMatchLine>,
    /// The page's grid lines (the whole page).
    pub grid_x: Vec<(String, f64)>,
    pub grid_y: Vec<(String, f64)>,
    /// The page's scale, paper inches per foot (0 when none was printed).
    pub scale: f64,
    pub key_plan: bool,
    /// The whole drawing as one view: for an enlarged-plan sheet whose views carry no title
    /// this reader knows (its biggest block of linework).
    pub whole: Option<PageView>,
    /// No view title read claims `whole`: it is a view of its own beside the titled ones.
    pub whole_free: bool,
    /// Door and room numbers ([`tagid::tag_key`] spelling) and where they are: an enlarged
    /// plan registers on its overall plan by the ones both print.
    pub tags: Vec<(String, [f64; 2])>,
    /// The page's longest straight pieces along an axis (walls, mostly): a view no grid line,
    /// tag or box places registers where its linework lies on its overall plan's.
    pub lines: Vec<AxisSeg>,
}

/// An enlarged plan and where it sits on its level's overall plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Enlarged {
    /// The sheet the enlarged plan is drawn on (`on_sheet` with `detail`: "E-6 detail 1").
    #[serde(rename = "on_sheet")]
    pub sheet: String,
    /// The overall plan its boundary is drawn on, where it registers (empty when none was found).
    #[serde(rename = "target_sheet")]
    pub overall_sheet: String,
    /// Its view number on that sheet.
    pub detail: String,
    pub title: String,
    pub level: String,
    pub registered: bool,
    /// `grids` (shared grid lines), `tags` (door and room numbers both sheets print),
    /// `boundary` (fitted into the boundary box with the scales) or `lines` (its walls laid on
    /// the overall plan's).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub method: String,
    /// Registered by tags: how many agree, and their root-mean-square error after the fit
    /// (overall plan units).
    #[serde(skip_serializing_if = "is_zero")]
    pub matched_tags: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub residual: Option<f64>,
    /// Registered by linework: the share of the enlarged plan's lines that lie on the overall
    /// plan's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_coverage: Option<f64>,
    /// The enlarged plan's turn on the overall plan, degrees (only when not 0).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_deg: Option<f64>,
    /// Enlarged sheet units per overall plan unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// The boundary on the overall plan `[x0, y0, x1, y1]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<[f64; 4]>,
    /// Devices the enlarged plan shows (it is the authority inside its boundary).
    pub devices: usize,
    pub devices_by_type: BTreeMap<String, usize>,
    /// What `devices` were counted in: `view` (the view's linework on its sheet) or `boundary`
    /// (the box drawn round the enlarged area on the overall plan, through the registration).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub devices_from: String,
    /// Symbols there whose mark is not a device type (by mark), symbols with no mark, and
    /// devices left to a smaller enlarged view of the sheet: what `devices` left out.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub uncounted_tags: BTreeMap<String, usize>,
    pub untagged_symbols: usize,
    pub in_smaller_views: usize,
    /// The overall plan's devices inside the boundary, left out of the count.
    pub overall_ignored: usize,
    /// The handles of the devices counted in `devices` (symbols on the enlarged sheet).
    #[serde(skip)]
    pub device_handles: Vec<String>,
    /// The handles of the overall plan's devices inside the boundary.
    #[serde(skip)]
    pub ignored_handles: Vec<String>,
    /// Enlarged sheet to overall plan (scale, rotation, offset).
    #[serde(skip)]
    pub fit: Option<Sim>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl Enlarged {
    /// Where a point of the enlarged sheet lands on the overall plan, once registered.
    pub fn place(&self, p: [f64; 2]) -> Option<[f64; 2]> {
        Some(self.fit.filter(|_| self.registered)?.apply(p))
    }
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

/// Boxes drawn dashed: four rows of dashes (short straight pieces, gaps between them) meeting
/// at the corners. A side may run on along a dashed grid line it lies on.
fn dash_boxes(segs: &[[[f64; 2]; 2]], h: f64) -> Vec<[f64; 4]> {
    // Runs of dashes per line: (horizontal?, cross-axis coordinate rounded) → pieces.
    let tol = (0.3 * h).max(1e-9);
    let mut lines: BTreeMap<(bool, i64), Vec<(f64, f64)>> = BTreeMap::new();
    for s in segs.iter().take(MAX_PIECES) {
        let len = dist(s[0], s[1]);
        if len < 0.2 * h || len > 8.0 * h {
            continue;
        }
        if (s[0][1] - s[1][1]).abs() < 0.05 * len {
            lines.entry((true, ((s[0][1] + s[1][1]) / 2.0 / tol).round() as i64)).or_default().push((s[0][0].min(s[1][0]), s[0][0].max(s[1][0])));
        } else if (s[0][0] - s[1][0]).abs() < 0.05 * len {
            lines.entry((false, ((s[0][0] + s[1][0]) / 2.0 / tol).round() as i64)).or_default().push((s[0][1].min(s[1][1]), s[0][1].max(s[1][1])));
        }
    }
    // Each line's runs: dashes no farther apart than a few dashes, dashed (not solid) on the
    // whole: (horizontal?, at, lo, hi, typical dash).
    struct Row {
        horizontal: bool,
        at: f64,
        lo: f64,
        hi: f64,
        dash: f64,
        /// Every dash about as long as the others (a dash-dot grid line is not).
        even: bool,
    }
    let mut rows: Vec<Row> = Vec::new();
    for ((horizontal, k), mut v) in lines {
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut run: Vec<(f64, f64)> = Vec::new();
        let flush = |run: &mut Vec<(f64, f64)>, rows: &mut Vec<Row>| {
            let (Some(first), Some(last)) = (run.first(), run.last()) else { return };
            let (lo, hi) = (first.0, last.1.max(first.1));
            let mut lens: Vec<f64> = run.iter().map(|(a, b)| b - a).collect();
            let inked: f64 = lens.iter().sum();
            lens.sort_by(f64::total_cmp);
            let dash = lens.get(lens.len() / 2).copied().unwrap_or(0.0);
            // The end dashes may be cut short at the corners.
            // Most dashes about the typical length (a stray piece on the line may differ; a
            // dash-dot line alternates two lengths).
            let alike = lens.iter().filter(|l| (**l / dash.max(1e-9) - 1.0).abs() <= 0.3).count();
            let even = alike * 5 >= lens.len() * 4;
            if run.len() >= 5 && hi > lo && (0.25..=0.92).contains(&(inked / (hi - lo))) && rows.len() < 20_000 {
                rows.push(Row { horizontal, at: k as f64 * tol, lo, hi, dash, even });
            }
            run.clear();
        };
        for d in v {
            let gap = run.last().map_or(0.0, |l| d.0 - l.1);
            if gap > 3.0 * h {
                flush(&mut run, &mut rows);
            }
            run.push(d);
        }
        flush(&mut run, &mut rows);
    }
    // Two opposite sides drawn alike (the same dashes, ends matching, shorter than the plan's
    // dashed grid lines) and a dashed line across them at one end (a side may lie on a grid line
    // or a wall drawn some other way).
    let near = |a: f64, b: f64| (a - b).abs() <= 1.5 * h;
    let mut out: Vec<[f64; 4]> = Vec::new();
    for horizontal in [true, false] {
        // Plan-wide dashed lines (three or more nearly as long as the longest) are a grid.
        let longest = rows.iter().filter(|r| r.horizontal == horizontal).map(|r| r.hi - r.lo).fold(0.0, f64::max);
        let grid = rows.iter().filter(|r| r.horizontal == horizontal && r.hi - r.lo > 0.6 * longest).count() >= 3;
        let pairs: Vec<&Row> = rows.iter().filter(|r| r.horizontal == horizontal && r.even && (!grid || r.hi - r.lo <= 0.6 * longest)).collect();
        let cross: Vec<&Row> = rows.iter().filter(|r| r.horizontal != horizontal).collect();
        for (i, a) in pairs.iter().enumerate() {
            for b in pairs.iter().skip(i + 1) {
                if out.len() >= MAX_BOXES {
                    return out;
                }
                let (c0, c1) = (a.at.min(b.at), a.at.max(b.at));
                let alike = (a.dash / b.dash.max(1e-9)).ln().abs() < 0.3;
                if c1 - c0 < 6.0 * h || !near(a.lo, b.lo) || !near(a.hi, b.hi) || a.hi - a.lo < 6.0 * h || !alike {
                    continue;
                }
                let (lo, hi) = ((a.lo + b.lo) / 2.0, (a.hi + b.hi) / 2.0);
                // A side: dashed runs along it covering half of it or more (a grid line breaks
                // where it crosses walls).
                let side = |at: f64| {
                    let mut spans: Vec<(f64, f64)> =
                        cross.iter().filter(|v| near(v.at, at)).map(|v| (v.lo.max(c0), v.hi.min(c1))).filter(|(a, b)| b > a).collect();
                    spans.sort_by(|x, y| x.0.total_cmp(&y.0));
                    let (mut covered, mut end) = (0.0, c0);
                    for (a, b) in spans {
                        if b > end {
                            covered += b - a.max(end);
                            end = b;
                        }
                    }
                    covered >= 0.5 * (c1 - c0)
                };
                if !(side(lo) || side(hi)) {
                    continue;
                }
                let bx = if horizontal { [lo, c0, hi, c1] } else { [c0, lo, c1, hi] };
                if !out.iter().any(|o| near(o[0], bx[0]) && near(o[1], bx[1]) && near(o[2], bx[2]) && near(o[3], bx[3])) {
                    out.push(bx);
                }
            }
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
    let solid_boxes = boxes(d, 12.0 * median_h);
    let dashed_boxes = dash_boxes(&segs, median_h);
    let page_boxes: Vec<[f64; 4]> = dashed_boxes.iter().chain(&solid_boxes).copied().collect();
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

    let mut out = PageRead {
        boxes: solid_boxes.clone(),
        dash_boxes: dashed_boxes.clone(),
        lines: linefit::axis_segments(&segs, 4.0 * median_h, linefit::MAX_PAGE_LINES),
        ..PageRead::default()
    };
    let mut used_texts: BTreeSet<usize> = BTreeSet::new();
    let mut view_marks: Vec<(String, [f64; 2])> = Vec::new();
    // What the linework around a bubble at `c` (radius `r`) makes it: its kind, the box its
    // leader reaches, and whether a divider splits it.
    let shape = |c: [f64; 2], r: f64| -> (&'static str, &'static str, Option<[f64; 4]>, bool, bool) {
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
        let shape_kind = if cut {
            "section_mark"
        } else if tip {
            "elevation_tag"
        } else {
            "detail_bubble"
        };
        let kind = if boundary.is_some() || enlarged_word { "enlarged_plan" } else { shape_kind };
        (kind, shape_kind, boundary, divider, enlarged_word)
    };
    let detail_like = |v: &str| !v.is_empty() && v.len() <= 4 && v.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '.');
    // A bubble naming its own sheet beside a view title numbers that view.
    let level_titles = sheet_reader::plan_views(d);
    let view_mark = |target: &str, c: [f64; 2], r: f64| {
        !own_n.is_empty()
            && norm_sheet(target) == own_n
            && (views_raw.iter().any(|v| dist(v.at, c) < 12.0 * r) || level_titles.iter().any(|v| dist(v.at, c) < 12.0 * r))
    };
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
        let (kind, shape_kind, boundary, divider, enlarged_word) = shape(c, r);
        out.callouts.push(Callout {
            kind: kind.into(),
            detail,
            target_sheet: target.trim().to_string(),
            at: c,
            radius: r,
            size: r,
            divider,
            boundary,
            shape_kind: shape_kind.into(),
            enlarged_word,
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
        let (kind, shape_kind, boundary, _, enlarged_word) = shape(c, r);
        out.callouts.push(Callout {
            kind: kind.into(),
            detail: t.value.trim().to_string(),
            target_sheet: v.to_string(),
            at: c,
            radius: r,
            size: r,
            divider: true,
            boundary,
            shape_kind: shape_kind.into(),
            enlarged_word,
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
                out.callouts.push(Callout {
                    kind: "match_line".into(),
                    target_sheet: target,
                    at: m,
                    text: value.clone(),
                    size: t.height,
                    ..Callout::default()
                });
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
                size: t.height,
                ..Callout::default()
            });
        }
    }

    // Views: their regions, grids and printed heights.
    let page = sheet_reader::recognise_dxf(d);
    out.scale = page.scale;
    out.grid_x = page.grid_x.iter().map(|g| (g.label.clone(), g.at_in)).collect();
    out.grid_y = page.grid_y.iter().map(|g| (g.label.clone(), g.at_in)).collect();
    let blocks = sheet_facts::view_regions(d);
    // View titles: over a scale line, or a plan view's title naming its level (lettering well
    // above the sheet's usual size) where no scale line is printed.
    let mut titles: Vec<(Option<String>, String, Option<String>, [f64; 2], Option<[f64; 4]>)> =
        views_raw.iter().map(|v| (v.number.clone(), v.title.clone(), v.scale_text.clone(), v.at, v.region)).collect();
    for v in level_titles {
        if !titles.iter().any(|t| t.1 == v.title || dist(t.3, v.at) < v.height) {
            titles.push((None, v.title, None, v.at, None));
        }
    }
    let enlarged_page = texts.iter().any(|t| t.value.to_ascii_uppercase().contains("ENLARGED"));
    // Grid lines are read for enlarged plans only (they register by them): on a whole set,
    // cropping every view of every plan costs more than the rest of the reading.
    let view_of = |region: Option<[f64; 4]>, grids: bool| -> PageView {
        let mut pv = PageView { region, ..PageView::default() };
        if let Some(rg) = region {
            pv.mounting_ft = texts.iter().filter(|t| inside(&rg, [t.at.x, t.at.y])).filter_map(|t| mounting_height(&t.value)).take(16).collect();
        }
        if let Some(rg) = region.filter(|_| grids) {
            // Grid bubbles stand outside the walls: look a little past the view for them.
            let m = 0.15 * (rg[2] - rg[0]).max(rg[3] - rg[1]);
            let wide = [rg[0] - m, rg[1] - m, rg[2] + m, rg[3] + m];
            let sub = sheet_reader::crop(d, &|p: [f64; 2]| inside(&wide, p));
            let rec = sheet_reader::recognise_dxf(&sub);
            pv.grid_x = rec.grid_x.iter().map(|g| (g.label.clone(), g.at_in)).collect();
            pv.grid_y = rec.grid_y.iter().map(|g| (g.label.clone(), g.at_in)).collect();
        }
        pv
    };
    let spots: Vec<[f64; 2]> = titles.iter().map(|t| t.3).collect();
    for (number, title, scale_text, at, region) in titles.into_iter().take(32) {
        let h = texts.iter().find(|t| t.value == title && dist([t.at.x, t.at.y], at) < 1e-6).map_or(median_h, |t| t.height);
        // The view's number: its own bubble's (a print's lettering may come back in pieces).
        let mark = view_marks.iter().filter(|(_, c)| c[0] <= at[0] && dist(*c, at) < 12.0 * h).min_by(|a, b| dist(a.1, at).total_cmp(&dist(b.1, at)));
        let others: Vec<[f64; 2]> = spots.iter().copied().filter(|o| *o != at).collect();
        let mut pv = view_of(view_block(&blocks, at, h, &others).or(region), enlarged_page);
        pv.number = mark.map(|m| m.0.clone()).or(number).unwrap_or_default();
        pv.title = title;
        pv.scale_text = scale_text;
        out.views.push(pv);
    }
    if enlarged_page {
        // A titled plan view may come apart into blocks too: the pieces beside it that no other
        // view claims are its.
        let gap = sheet_width(&blocks) * MAIN_PLAN_GAP;
        let all: Vec<[f64; 4]> = blocks.iter().map(|(b, _)| *b).collect();
        let regions: Vec<Option<[f64; 4]>> = out.views.iter().map(|v| v.region).collect();
        for i in 0..out.views.len() {
            let Some(v) = out.views.get(i) else { continue };
            let Some(r) = v.region else { continue };
            if NOT_PLAN_WORDS.iter().any(|w| v.title.to_ascii_uppercase().contains(w)) {
                continue;
            }
            let claimed: Vec<[f64; 4]> = regions.iter().enumerate().filter(|(j, _)| *j != i).filter_map(|(_, g)| *g).collect();
            let titles: Vec<[f64; 2]> = spots.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, t)| *t).collect();
            let grown = grow_plan(r, &all, &claimed, &titles, gap);
            if grown != r {
                let mut w = view_of(Some(grown), true);
                if let Some(v) = out.views.get_mut(i) {
                    w.number = std::mem::take(&mut v.number);
                    w.title = std::mem::take(&mut v.title);
                    w.scale_text = v.scale_text.take();
                    *v = w;
                }
            }
        }
        // Its biggest block of linework is the enlarged plan: the sheet's only view when no title
        // was read, or its main plan beside small titled details when no title claims it (a
        // title printed where this reader doesn't look for one). The title block's strip at the
        // right edge is never it.
        let x_hi = blocks.iter().map(|(r, _)| r[2]).fold(f64::NEG_INFINITY, f64::max);
        let x_lo = blocks.iter().map(|(r, _)| r[0]).fold(f64::INFINITY, f64::min);
        let in_title_strip = |r: &[f64; 4]| x_hi > x_lo && (r[0] + r[2]) / 2.0 > x_lo + 0.85 * (x_hi - x_lo);
        let main = blocks.iter().filter(|(r, _)| !in_title_strip(r)).max_by_key(|(_, n)| *n).map(|(r, _)| *r);
        if let Some(r) = main {
            let mid = [(r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0];
            out.whole_free = !out.views.iter().filter_map(|v| v.region).any(|g| overlap_ratio(&g, &r) >= 0.3 || inside(&g, mid));
            // A big plan may come apart into several blocks (a gap in the linework, walls too
            // long to join them): the pieces beside it that no titled view claims are its too.
            let claimed: Vec<[f64; 4]> = out.views.iter().filter_map(|v| v.region).collect();
            let r = if out.whole_free {
                let free: Vec<[f64; 4]> = blocks.iter().map(|(b, _)| *b).filter(|b| !in_title_strip(b)).collect();
                grow_plan(r, &free, &claimed, &spots, gap)
            } else {
                r
            };
            let mut w = view_of(Some(r), true);
            // Under the plan: its number bubble (a bubble naming this sheet) and its title.
            let (bw, bh) = (r[2] - r[0], r[3] - r[1]);
            let under = |p: [f64; 2], rad: f64| {
                p[0] >= r[0] - 0.1 * bw && p[0] <= r[2] && p[1] >= r[1] - (0.25 * bh).max(12.0 * rad) && p[1] <= r[1] + 0.15 * bh
            };
            let corner = [r[0], r[1]];
            let mark = out
                .callouts
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.radius > 0.0 && !c.detail.is_empty() && !own_n.is_empty() && norm_sheet(&c.target_sheet) == own_n && under(c.at, c.radius)
                })
                .min_by(|a, b| dist(a.1.at, corner).total_cmp(&dist(b.1.at, corner)))
                .map(|(i, c)| (i, c.detail.clone(), c.at, c.radius));
            if out.whole_free
                && let Some((i, number, at, rad)) = mark
            {
                // It numbers the view: not a link.
                out.callouts.remove(i);
                w.number = number;
                // The title lettering beside the bubble.
                w.title = texts
                    .iter()
                    .filter(|t| {
                        t.height >= 1.4 * median_h && t.rotation.sin().abs() < 0.05 && t.at.x > at[0] && t.at.x - at[0] < 4.0 * rad + 2.0 * t.height
                    })
                    .filter(|t| (text_middle(t)[1] - at[1]).abs() < 2.0 * rad.max(t.height))
                    .min_by(|a, b| (a.at.x - at[0]).total_cmp(&(b.at.x - at[0])))
                    .map(|t| t.value.trim().to_string())
                    .unwrap_or_default();
            }
            out.whole = Some(w);
        }
    }

    // Door and room numbers: plan lettering (not titles) that reads as one.
    for t in &texts {
        if out.tags.len() >= MAX_PAGE_TAGS {
            break;
        }
        if t.height > 3.0 * median_h {
            continue;
        }
        if let Some(k) = tagid::tag_key(&t.value) {
            out.tags.push((k, text_middle(t)));
        }
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
        if tag.is_empty() && !legend.covers(at) && out.untagged.len() < MAX_PAGE_DEVICES {
            out.untagged.push([at.x, at.y]);
        }
        if tag.is_empty() || legend.covers(at) || is_circuit_tag(&tag) {
            continue;
        }
        out.devices.push(PageDevice { tag, at: [at.x, at.y], handle: e.handle.hex() });
    }
    out
}

/// One copy of each line of lettering: an import may give a mark twice (the page's hidden text
/// and the strokes read back), at about the same place (the OCR copy sits on the strokes, a
/// fraction of a letter off the hidden text), or the OCR copy in pieces (see [`drop_pieces`]).
fn one_copy(texts: Vec<TextItem>) -> Vec<TextItem> {
    let mut seen: BTreeMap<String, Vec<([f64; 2], f64)>> = BTreeMap::new();
    let kept = texts
        .into_iter()
        .filter(|t| {
            let key = t.value.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase();
            let at = [t.at.x, t.at.y];
            let copies = seen.entry(key).or_default();
            let near = copies.iter().any(|(p, h)| {
                let reach = 0.75 * h.max(t.height);
                (p[0] - at[0]).abs() <= reach && (p[1] - at[1]).abs() <= reach
            });
            if !near {
                copies.push((at, t.height));
            }
            !near
        })
        .collect();
    drop_pieces(kept)
}

/// Texts in a longer text's footprint whose words it holds: the OCR copy of a line the hidden
/// text gives whole, read in pieces (`MATCH LINE` and `SEE SHEET E-2` along `MATCH LINE - SEE
/// SHEET E-2`) or cut short (`FIRST FLOOR POWER PLAN` over `... - AREA A`).
fn drop_pieces(texts: Vec<TextItem>) -> Vec<TextItem> {
    let words = |t: &TextItem| t.value.split_whitespace().map(str::to_ascii_uppercase).collect::<Vec<_>>();
    let all: Vec<Vec<String>> = texts.iter().map(words).collect();
    let mut heights: Vec<f64> = texts.iter().map(|t| t.height).collect();
    heights.sort_by(f64::total_cmp);
    let cell = 8.0 * heights.get(heights.len() / 2).copied().unwrap_or(1.0).max(1e-6);
    let key = |x: f64, y: f64| ((x / cell).floor() as i64, (y / cell).floor() as i64);
    // Each text of two words or more by the cells its footprint covers.
    let foot = |t: &TextItem, n: usize| {
        let len = n as f64 * t.height;
        let (c, s) = (t.rotation.cos(), t.rotation.sin());
        let ends = [[t.at.x, t.at.y], [t.at.x + len * c, t.at.y + len * s]];
        let pad = 1.5 * t.height;
        [ends[0][0].min(ends[1][0]) - pad, ends[0][1].min(ends[1][1]) - pad, ends[0][0].max(ends[1][0]) + pad, ends[0][1].max(ends[1][1]) + pad]
    };
    let mut grid: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (i, t) in texts.iter().enumerate() {
        if all[i].len() < 2 {
            continue;
        }
        let b = foot(t, t.value.chars().count());
        let (k0, k1) = (key(b[0], b[1]), key(b[2], b[3]));
        if (k1.0 - k0.0 + 1) * (k1.1 - k0.1 + 1) > 400 {
            continue;
        }
        for x in k0.0..=k1.0 {
            for y in k0.1..=k1.1 {
                grid.entry((x, y)).or_default().push(i);
            }
        }
    }
    let holds = |big: &[String], small: &[String]| small.len() < big.len() && big.windows(small.len()).any(|w| w == small);
    let inside_of = |t: &TextItem, u: &TextItem| {
        let (c, s) = (u.rotation.cos(), u.rotation.sin());
        let (dx, dy) = (t.at.x - u.at.x, t.at.y - u.at.y);
        let (along, across) = (dx * c + dy * s, -dx * s + dy * c);
        let h = u.height.max(t.height);
        let len = u.value.chars().count() as f64 * u.height;
        along >= -h && along <= len + h && across.abs() <= h
    };
    texts
        .iter()
        .enumerate()
        .filter(|(i, t)| {
            let piece =
                grid.get(&key(t.at.x, t.at.y)).is_some_and(|c| c.iter().any(|&j| j != *i && holds(&all[j], &all[*i]) && inside_of(t, &texts[j])));
            !piece
        })
        .map(|(_, t)| t.clone())
        .collect()
}

/// The block of linework a view title labels: the biggest block standing above the title
/// (or holding it) over the title's start, grown by the small blocks touching it (grid bubbles,
/// tags just outside its lines).
fn view_block(blocks: &[([f64; 4], usize)], at: [f64; 2], h: f64, others: &[[f64; 2]]) -> Option<[f64; 4]> {
    let holds = |r: &[f64; 4]| inside(r, at);
    let above = |r: &[f64; 4]| r[1] >= at[1] - h && r[1] - at[1] <= 30.0 * h && r[0] <= at[0] + 4.0 * h && at[0] <= r[2];
    let most = blocks.iter().map(|(_, n)| *n).max().unwrap_or(0);
    let big = |n: usize| n.saturating_mul(10) >= most;
    // The title's own bar (its underline and bubble): a flat block holding it, never the plan.
    let bar = |r: &[f64; 4]| holds(r) && r[3] - r[1] < 0.2 * (r[2] - r[0]) && r[3] - r[1] < 15.0 * h;
    let near = blocks.iter().filter(|(r, _)| !bar(r) && (holds(r) || above(r))).max_by_key(|(_, n)| *n).copied();
    // The title's width: its bar, else its start.
    let span = blocks
        .iter()
        .filter(|(r, _)| bar(r))
        .map(|(r, _)| [r[0], r[2]])
        .min_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0])))
        .unwrap_or([at[0], at[0]]);
    // A title printed well below its plan: the nearest big block above it, with no other view
    // title between them.
    let far = || {
        blocks
            .iter()
            .filter(|(r, n)| {
                let w = r[2] - r[0];
                // With other views on the sheet, the plan stands over its title (its bar reaching
                // under the plan); alone, any big block above it is.
                let over = others.is_empty() || (span[1] >= r[0] - 0.25 * w && span[0] <= r[2]);
                big(*n)
                    && r[1] >= at[1] - h
                    && over
                    && !others.iter().any(|o| o[1] > at[1] + h && o[1] < r[1] && o[0] >= r[0] - 0.25 * w && o[0] <= r[2])
            })
            .min_by(|a, b| (a.0[1] - at[1]).total_cmp(&(b.0[1] - at[1])))
            .copied()
    };
    let (mut b, _) = match near {
        Some(nb) if big(nb.1) => nb,
        _ => far().or(near)?,
    };
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

/// Count into `entry` the page's devices at a place `holds` takes, leaving out those inside the
/// sheet's `smaller` enlarged views.
fn count_devices(
    entry: &mut Enlarged,
    page: &LinkPage,
    smaller: &[[f64; 4]],
    holds: &dyn Fn([f64; 2]) -> bool,
    device_type: &dyn Fn(&BTreeSet<String>, &str) -> Option<String>,
) {
    for dv in page.read.devices.iter().filter(|dv| holds(dv.at) && !smaller.iter().any(|r| inside(r, dv.at))) {
        match device_type(&page.read.legend_types, &dv.tag) {
            Some(t) => {
                *entry.devices_by_type.entry(t).or_default() += 1;
                entry.devices += 1;
                entry.device_handles.push(dv.handle.clone());
            }
            None if entry.uncounted_tags.len() < MAX_UNCOUNTED_TAGS || entry.uncounted_tags.contains_key(&dv.tag) => {
                *entry.uncounted_tags.entry(dv.tag.clone()).or_default() += 1;
            }
            None => {}
        }
    }
    entry.untagged_symbols = page.read.untagged.iter().filter(|p| holds(**p) && !smaller.iter().any(|r| inside(r, **p))).count();
    entry.in_smaller_views = page.read.devices.iter().filter(|dv| holds(dv.at) && smaller.iter().any(|r| inside(r, dv.at))).count();
}

/// Most tags listed in an enlarged plan's `uncounted_tags`.
const MAX_UNCOUNTED_TAGS: usize = 24;

/// How far apart (share of the sheet's width) two blocks of one plan may lie.
const MAIN_PLAN_GAP: f64 = 0.03;

/// The main plan `r` with the blocks beside it (within `gap`) that are not a titled view's:
/// neither overlapping a view's region nor holding a view title.
/// The width the sheet's blocks of linework span.
fn sheet_width(blocks: &[([f64; 4], usize)]) -> f64 {
    let hi = blocks.iter().map(|(r, _)| r[2]).fold(f64::NEG_INFINITY, f64::max);
    let lo = blocks.iter().map(|(r, _)| r[0]).fold(f64::INFINITY, f64::min);
    if hi > lo { hi - lo } else { 0.0 }
}

fn grow_plan(r: [f64; 4], blocks: &[[f64; 4]], claimed: &[[f64; 4]], titles: &[[f64; 2]], gap: f64) -> [f64; 4] {
    if !(gap.is_finite() && gap > 0.0) {
        return r;
    }
    let free = |b: &[f64; 4]| !claimed.iter().any(|g| covered(g, b) >= 0.3 || covered(b, g) >= 0.3) && !titles.iter().any(|t| inside(b, *t));
    // Flat strips (a title's bar, a border) are never plan.
    let shaped = |b: &[f64; 4]| {
        let (w, h) = (b[2] - b[0], b[3] - b[1]);
        w > 0.0 && h > 0.0 && w.min(h) >= 0.1 * w.max(h)
    };
    let mut out = r;
    for _ in 0..8 {
        let mut grew = false;
        for b in blocks.iter().filter(|b| free(b) && shaped(b)) {
            let near = b[0] <= out[2] + gap && b[2] >= out[0] - gap && b[1] <= out[3] + gap && b[3] >= out[1] - gap;
            let adds = b[0] < out[0] || b[1] < out[1] || b[2] > out[2] || b[3] > out[3];
            let grown = [out[0].min(b[0]), out[1].min(b[1]), out[2].max(b[2]), out[3].max(b[3])];
            // Growing must not swallow a titled view.
            if near && adds && !claimed.iter().any(|g| covered(g, &grown) >= 0.5 && covered(g, &out) < 0.5) {
                out = grown;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    out
}

/// A circuit or panel reference (`D-1`, `LP-12`): a wiring label, not a type.
fn is_circuit_tag(tag: &str) -> bool {
    tag.rsplit_once('-').is_some_and(|(head, n)| !head.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

// ─── The set ────────────────────────────────────────────────────────────────

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
fn grid_fit(view: &PageView, host_x: &[(String, f64)], host_y: &[(String, f64)]) -> Option<Sim> {
    let (sx, px) = axis_fit(&view.grid_x, host_x);
    let (sy, py) = axis_fit(&view.grid_y, host_y);
    let s = match (sx, sy) {
        (Some(a), Some(b)) if (a / b - 1.0).abs() < 0.05 => (a + b) / 2.0,
        (Some(a), Some(_)) | (Some(a), None) => a,
        (None, Some(b)) => b,
        // One line each way: both sheets are imported at their scale notes (building units), so
        // one to one, if the lines that are shared say so.
        (None, None) => 1.0,
    };
    let tx = px.iter().map(|(a, b)| b - s * a).sum::<f64>() / px.len().max(1) as f64;
    let ty = py.iter().map(|(a, b)| b - s * a).sum::<f64>() / py.len().max(1) as f64;
    (!px.is_empty() && !py.is_empty() && s.is_finite()).then_some(Sim::scale_offset(s, [tx, ty]))
}

/// The overlap of two boxes over their union (0 when apart).
/// The share of `a`'s area that lies in `b`.
fn covered(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let area = (a[2] - a[0]).max(0.0) * (a[3] - a[1]).max(0.0);
    if area > 0.0 { w * h / area } else { 0.0 }
}

fn overlap_ratio(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let area = |r: &[f64; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let both = w * h;
    let union = area(a) + area(b) - both;
    if union > 0.0 { both / union } else { 0.0 }
}

fn map_box(f: &Sim, b: &[f64; 4]) -> [f64; 4] {
    let corners = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]].map(|p| f.apply(p));
    let (xs, ys) = (corners.map(|p| p[0]), corners.map(|p| p[1]));
    let lo = |v: [f64; 4]| v.into_iter().fold(f64::INFINITY, f64::min);
    let hi = |v: [f64; 4]| v.into_iter().fold(f64::NEG_INFINITY, f64::max);
    [lo(xs), lo(ys), hi(xs), hi(ys)]
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
        /// Its view number: the one printed, or "1" for the only view of a sheet that numbers none.
        detail: String,
    }
    let mut enl_views: Vec<EnlView> = Vec::new();
    // Titles that never name a plan, even on an enlarged-plan sheet.
    let not_plan = |t: &str| NOT_PLAN_WORDS.iter().any(|w| t.contains(w));
    for p in pages {
        let before = enl_views.len();
        let enlarged_sheet = p.sheet.sheet_type == "enlarged_plan"
            || p.sheet.title.to_ascii_uppercase().contains("ENLARGED")
            || p.read.views.iter().any(|v| v.title.to_ascii_uppercase().contains("ENLARGED"));
        for v in &p.read.views {
            let t = v.title.to_ascii_uppercase();
            // On an enlarged-plan sheet a view is a plan unless its title says otherwise (a core
            // plan may be titled by its area alone: `COMMON AREA - POWER`, over its scale line;
            // a bare level name is a second title of a view, not one of its own).
            let scaled = v.scale_text.is_some();
            if t.contains("ENLARGED") || (enlarged_sheet && (t.contains("PLAN") || (scaled && !not_plan(&t))) && !t.contains("KEY PLAN")) {
                enl_views.push(EnlView { page: p, view: v, detail: v.number.clone() });
            }
        }
        // An enlarged-plan sheet whose views this reader found no title for: the sheet's
        // biggest block of linework is its plan. Beside titled views, the biggest block no
        // title claims is a plan of its own.
        if (enl_views.len() == before || p.read.whole_free)
            && enlarged_sheet
            && let Some(w) = &p.read.whole
        {
            let mut detail = w.number.clone();
            // Unnumbered beside numbered views: the sheet's main plan is its view 1, when no
            // other view is.
            if detail.trim().is_empty() && enl_views.len() > before && !p.read.views.iter().any(|v| v.number.trim() == "1") {
                detail = "1".into();
            }
            enl_views.push(EnlView { page: p, view: w, detail });
        }
        if enl_views.len() == before + 1
            && p.read.views.iter().all(|v| v.number.trim().is_empty())
            && let Some(e) = enl_views.last_mut()
            && e.detail.trim().is_empty()
        {
            e.detail = "1".into();
        }
    }
    // One entry per view: a sheet printed twice in the set, or one view whose title was read
    // twice, would count its devices twice.
    let mut unique: Vec<EnlView> = Vec::with_capacity(enl_views.len());
    for e in enl_views {
        let overlapping = |u: &EnlView| match (u.view.region, e.view.region) {
            (Some(a), Some(b)) => overlap_ratio(&a, &b) >= 0.5,
            _ => false,
        };
        let same = |u: &EnlView| {
            norm_sheet(&u.page.sheet.number) == norm_sheet(&e.page.sheet.number)
                && ((u.detail == e.detail && (!std::ptr::eq(u.page, e.page) || overlapping(u) || u.view.region.is_none() || e.view.region.is_none()))
                    // An unnumbered second reading of a numbered view on the same page.
                    || (std::ptr::eq(u.page, e.page) && (u.detail.is_empty() || e.detail.is_empty()) && overlapping(u)))
        };
        if !unique.iter().any(same) {
            unique.push(e);
        }
    }
    let enl_views = unique;

    let enlarged_sheets: BTreeSet<String> = enl_views
        .iter()
        .map(|e| norm_sheet(&e.page.sheet.number))
        .chain(sheets.iter().filter(|x| x.sheet_type == "enlarged_plan").map(|x| norm_sheet(&x.number)))
        .collect();
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
                        c.detail = e.detail.clone();
                    }
                    found.is_some()
                }
                _ => numbers.contains(&norm_sheet(&c.target_sheet)),
            };
            // A bubble with no divider pointing nowhere is lettering in a circle, not a callout; so
            // is a circled mark that can't be one of this set's sheet numbers (`RG-2`, `L1`, `P1`
            // where the set numbers its sheets `P-101`).
            if c.radius > 0.0 && !c.resolved && (!c.divider || !sheet_shaped(&c.target_sheet, sheets)) {
                continue;
            }
            // A leader reaching a box makes an enlarged plan's callout only when the sheet named
            // is one (an elevation mark's tip touches the building outline too).
            if c.kind == "enlarged_plan" && !c.enlarged_word && !enlarged_sheets.contains(&norm_sheet(&c.target_sheet)) && !c.shape_kind.is_empty() {
                c.kind = c.shape_kind.clone();
                c.boundary = None;
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

    // Device types: the page's own legends and schedules (as a job reading that sheet alone
    // sorts them), and the set's sheet numbers are never types. Every legend in the set
    // together would not do: a lighting schedule or a fan schedule on another sheet lists its
    // own types only, and every power device mark it doesn't list would be dropped.
    let mut sheet_forms: BTreeSet<String> = BTreeSet::new();
    for s in sheets {
        sheet_forms.extend(device_types::sheet_number_forms(&s.number));
    }
    let none = BTreeSet::new();
    let device_type = |legend: &BTreeSet<String>, tag: &str| match device_types::tag_verdict(tag, legend, &none, &sheet_forms) {
        TagVerdict::Device(t) => Some(t),
        _ => None,
    };

    // Enlarged plans, registered on their level's overall plan.
    let mut boundaries: BTreeMap<String, Vec<[f64; 4]>> = BTreeMap::new();
    for e in &enl_views {
        let target = e.page.sheet.number.clone();
        let detail = e.detail.clone();
        let mut level = level_of_view(e.page, e.view);
        let mut entry = Enlarged { sheet: target.clone(), detail: detail.clone(), title: e.view.title.clone(), ..Enlarged::default() };
        // Its devices: the enlarged sheet's symbols inside the view. A device inside a smaller
        // enlarged view of the same sheet is that view's (the main plan's block can take in the
        // details beside it).
        let area = |r: &[f64; 4]| (r[2] - r[0]) * (r[3] - r[1]);
        let smaller: Vec<[f64; 4]> = match e.view.region {
            Some(rg) => enl_views
                .iter()
                .filter(|o| std::ptr::eq(o.page, e.page))
                .filter_map(|o| o.view.region)
                .filter(|r| *r != rg && area(r) < area(&rg))
                .collect(),
            None => Vec::new(),
        };
        if let Some(rg) = e.view.region {
            // Devices just outside the walls (lights on the facade) are the view's too.
            let m = 0.08 * (rg[2] - rg[0]).max(rg[3] - rg[1]);
            let reach = [rg[0] - m, rg[1] - m, rg[2] + m, rg[3] + m];
            count_devices(&mut entry, e.page, &smaller, &|p| inside(&reach, p), &device_type);
            entry.devices_from = "view".into();
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
                    .filter(|c| {
                        points_here(c) && matches!(c.kind.as_str(), "enlarged_plan" | "enlarged_ref" | "detail_bubble" | "detail_ref" | "sheet_ref")
                    })
                    .map(move |c| (p, c))
            })
            .max_by_key(|(p, c)| {
                (c.boundary.is_some(), c.kind == "enlarged_plan", p.sheet.discipline == e.page.sheet.discipline, p.sheet.level == level)
            });
        // Grid lines the view and a plan both label (the plan of the same part of the building).
        let shared = |p: &LinkPage| {
            let n = |a: &[(String, f64)], b: &[(String, f64)]| a.iter().filter(|(l, _)| b.iter().any(|(m, _)| m == l)).count();
            n(&e.view.grid_x, &p.read.grid_x).min(4) + n(&e.view.grid_y, &p.read.grid_y).min(4)
        };
        let host = host_callout.map(|(p, _)| p).or_else(|| {
            pages
                .iter()
                .filter(|p| p.sheet.number != target && p.sheet.sheet_type == "plan" && !level.is_empty() && p.sheet.level == level)
                .max_by_key(|p| (p.sheet.discipline == e.page.sheet.discipline, shared(p), p.read.devices.len()))
        });
        // No callout and no level: the plan of the view's discipline sharing the most grid lines.
        let mut host = host.or_else(|| {
            pages
                .iter()
                .filter(|p| p.sheet.number != target && p.sheet.sheet_type == "plan" && shared(p) >= 2)
                .max_by_key(|p| (p.sheet.discipline == e.page.sheet.discipline, shared(p), std::cmp::Reverse(level_key(&p.sheet.level))))
        });
        // Door and room numbers the view prints: it registers where its overall plan prints the
        // same ones (any discipline's plan: the architectural background carries them).
        // A sheet number printed in a bubble is a link, not a tag.
        let not_sheet = |t: &&(String, [f64; 2])| !numbers.contains(&norm_sheet(&t.0));
        let view_tags: Vec<(String, [f64; 2])> =
            e.view.region.map(|rg| e.page.read.tags.iter().filter(|(_, p)| inside(&rg, *p)).filter(not_sheet).cloned().collect()).unwrap_or_default();
        let tag_tol = e.view.region.map_or(0.0, |rg| 0.03 * (rg[2] - rg[0]).hypot(rg[3] - rg[1]));
        let by_tags = |h: &LinkPage| -> Option<tagid::TagFit> {
            if view_tags.len() < 3 || h.read.tags.len() < 3 {
                return None;
            }
            let theirs: Vec<(String, [f64; 2])> = h.read.tags.iter().filter(not_sheet).cloned().collect();
            // Both sheets are read at their scale notes: the scale is near one, or the printed
            // scales' ratio; anything from a fifth to five times is let through.
            tagid::fit_by_tags(&view_tags, &theirs, tag_tol, (0.2, 5.0))
        };
        let placed_on_host = host.is_some_and(|h| grid_fit(e.view, &h.read.grid_x, &h.read.grid_y).is_some() || by_tags(h).is_some());
        if !placed_on_host && view_tags.len() >= 3 {
            // No grid line or tag places it on the plan found: the plan sharing the most of its
            // numbers that they place it on (the same level first).
            let mine: BTreeSet<&str> = view_tags.iter().map(|t| t.0.as_str()).collect();
            let mut cands: Vec<(&LinkPage, usize)> = pages
                .iter()
                .filter(|p| {
                    p.sheet.number != target
                        && !enlarged_sheets.contains(&norm_sheet(&p.sheet.number))
                        && matches!(p.sheet.sheet_type.as_str(), "plan" | "unknown" | "rcp")
                })
                .map(|p| (p, p.read.tags.iter().map(|t| t.0.as_str()).filter(|t| mine.contains(t)).collect::<BTreeSet<_>>().len()))
                .filter(|(_, n)| *n >= 3)
                .collect();
            cands.sort_by_key(|(p, n)| (std::cmp::Reverse(!level.is_empty() && p.sheet.level == level), std::cmp::Reverse(*n)));
            if let Some((p, _)) = cands.into_iter().take(4).find(|(p, _)| by_tags(p).is_some()) {
                host = Some(p);
            }
        }
        // The boundary box on a plan: the callout's leader reaches it, or the box a reference
        // sits by.
        let boxed_on = |h: &LinkPage| -> Option<[f64; 4]> {
            host_callout.filter(|(p, _)| std::ptr::eq(*p, h)).and_then(|(_, c)| {
                c.boundary.or_else(|| {
                    // The box a note or bubble sits by: dashed boxes first (how a boundary is
                    // drawn), never the sheet's border.
                    let reach = 6.0 * if c.size > 0.0 { c.size } else { 1.0 };
                    let widest = h.read.boxes.iter().chain(&h.read.dash_boxes).map(|b| b[2] - b[0]).fold(0.0, f64::max);
                    let by = |list: &[[f64; 4]]| {
                        list.iter()
                            .filter(|b| box_edge_dist(b, c.at) < reach && b[2] - b[0] < 0.6 * widest)
                            .min_by(|a, b| box_edge_dist(a, c.at).total_cmp(&box_edge_dist(b, c.at)))
                            .copied()
                    };
                    by(&h.read.dash_boxes).or_else(|| by(&h.read.boxes))
                })
            })
        };
        // The view's linework, laid on a plan's: one to one (both sheets read at their scale
        // notes) or the printed scales' ratio; inside the callout's box, or by the callout, when
        // the plan has one.
        let view_lines: Vec<AxisSeg> =
            e.view.region.map(|rg| e.page.read.lines.iter().filter(|l| inside(&rg, l.mid())).copied().collect()).unwrap_or_default();
        let by_lines = |h: &LinkPage| -> Option<LineFit> {
            let rg = e.view.region?;
            let tol = 0.004 * (rg[2] - rg[0]).hypot(rg[3] - rg[1]);
            let printed = e
                .view
                .scale_text
                .as_deref()
                .and_then(sheet_reader::parse_scale)
                .filter(|v| *v > 0.0 && h.read.scale > 0.0)
                .map(|v| h.read.scale / v)
                .filter(|r| (r - 1.0).abs() > 0.02);
            let scales: Vec<f64> = [Some(1.0), printed].into_iter().flatten().collect();
            let boxed = boxed_on(h);
            let near = host_callout.filter(|(p, _)| std::ptr::eq(*p, h)).map(|(_, c)| c.at);
            let accept = |f: &Sim| {
                let placed = map_box(f, &rg);
                match (boxed, near) {
                    (Some(b), _) => overlap_ratio(&b, &placed) >= 0.3,
                    (None, Some(c)) => inside(&placed, c) || box_edge_dist(&placed, c) <= 0.5 * (placed[2] - placed[0]).hypot(placed[3] - placed[1]),
                    (None, None) => true,
                }
            };
            linefit::fit_by_lines(&view_lines, &h.read.lines, &scales, tol, &accept)
        };
        // (plan, fit, how, its box, the tag fit, the line fit)
        type Chosen<'p> = (&'p LinkPage, Option<(Sim, &'static str)>, Option<[f64; 4]>, Option<tagid::TagFit>, Option<LineFit>);
        let mut chosen: Option<Chosen> = None;
        if let Some(h) = host {
            let boxed = boxed_on(h);
            let tag_fit = by_tags(h);
            let fit = grid_fit(e.view, &h.read.grid_x, &h.read.grid_y).map(|f| (f, "grids"));
            let fit = fit.or_else(|| tag_fit.as_ref().map(|f| (f.sim, "tags")));
            let boundary_fit = || {
                let (b, rg) = (boxed?, e.view.region?);
                let (bw, bh, rw, rh) = (b[2] - b[0], b[3] - b[1], rg[2] - rg[0], rg[3] - rg[1]);
                if !(bw > 0.0 && bh > 0.0 && rw > 0.0 && rh > 0.0) {
                    return None;
                }
                // The plan fitted into its box; a scale near one to one (both sheets read at
                // their scale notes) or near the printed scales' ratio is taken as exact.
                let fitted = (bw / rw).min(bh / rh);
                let printed = e
                    .view
                    .scale_text
                    .as_deref()
                    .and_then(sheet_reader::parse_scale)
                    .filter(|v| *v > 0.0 && h.read.scale > 0.0)
                    .map(|v| h.read.scale / v);
                let s = [Some(1.0), printed]
                    .into_iter()
                    .flatten()
                    .filter(|k| (fitted / k).ln().abs() < 0.3)
                    .min_by(|a, b| (fitted / a).ln().abs().total_cmp(&(fitted / b).ln().abs()))
                    .unwrap_or(fitted);
                let cb = [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0];
                let cr = [(rg[0] + rg[2]) / 2.0, (rg[1] + rg[3]) / 2.0];
                Some((Sim::scale_offset(s, [cb[0] - s * cr[0], cb[1] - s * cr[1]]), "boundary"))
            };
            let fit = fit.or_else(boundary_fit);
            let line_fit = if fit.is_none() { by_lines(h) } else { None };
            let fit = fit.or_else(|| line_fit.map(|l| (l.sim, "lines")));
            chosen = Some((h, fit, boxed, tag_fit, line_fit));
        }
        // Nothing places it on that plan (or none was found): the plans of its level (any level
        // when it names none) whose linework it lies on. Two levels' plans both taking it (typical
        // floors) place it on neither.
        if chosen.as_ref().is_none_or(|c| c.1.is_none()) && view_lines.len() >= 6 {
            let mut cands: Vec<&LinkPage> = pages
                .iter()
                .filter(|p| {
                    !host.is_some_and(|h| std::ptr::eq(h, *p))
                        && p.sheet.number != target
                        && !enlarged_sheets.contains(&norm_sheet(&p.sheet.number))
                        && p.sheet.sheet_type == "plan"
                        && (level.is_empty() || p.sheet.level == level)
                })
                .collect();
            cands.sort_by_key(|p| (p.sheet.discipline != e.page.sheet.discipline, p.sheet.discipline != "A", std::cmp::Reverse(p.read.lines.len())));
            let fits: Vec<(&LinkPage, LineFit)> = cands.into_iter().take(4).filter_map(|p| Some((p, by_lines(p)?))).collect();
            let best = fits.iter().max_by(|a, b| a.1.coverage.total_cmp(&b.1.coverage));
            if let Some(&(p, lf)) = best {
                let rival = fits.iter().any(|(q, o)| q.sheet.level != p.sheet.level && o.coverage >= 0.85 * lf.coverage);
                if !rival {
                    chosen = Some((p, Some((lf.sim, "lines")), boxed_on(p), None, Some(lf)));
                }
            }
        }
        if let Some((h, fit, boxed, tag_fit, line_fit)) = chosen {
            entry.overall_sheet = h.sheet.number.clone();
            if level.is_empty() {
                level = h.sheet.level.clone();
            }
            if let Some((f, how)) = fit {
                entry.registered = true;
                entry.method = how.into();
                entry.scale = Some((1.0 / f.scale() * 1000.0).round() / 1000.0);
                entry.fit = Some(f);
                let turn = f.rotation_deg();
                entry.rotation_deg = (turn.abs() > 0.05).then(|| (turn * 100.0).round() / 100.0);
                if how == "tags"
                    && let Some(t) = &tag_fit
                {
                    entry.matched_tags = t.matched.len();
                    entry.residual = Some((t.residual * 100.0).round() / 100.0);
                }
                if how == "lines"
                    && let Some(l) = &line_fit
                {
                    entry.line_coverage = Some((l.coverage * 1000.0).round() / 1000.0);
                }
                // Placed by the grid, the plan's own outline is the boundary, unless a box drawn
                // round about the same area says it more exactly.
                let placed = e.view.region.map(|rg| map_box(&f, &rg));
                // The plan read off its sheet may be only part of the area boxed (a big plan comes
                // apart into pieces): lying mostly inside the box, the box is the boundary.
                let b = match (how, boxed, placed) {
                    ("grids" | "tags" | "lines", Some(bx), Some(pl)) if overlap_ratio(&bx, &pl) >= 0.5 || covered(&pl, &bx) >= 0.7 => Some(bx),
                    ("grids" | "tags" | "lines", _, Some(pl)) => Some(pl),
                    _ => boxed.or(placed),
                };
                entry.boundary = b;
                // A box drawn on the overall plan round the enlarged area says what the
                // enlarged plan covers better than the blocks of linework read off its sheet (a
                // big plan comes apart into pieces): every device of the sheet that lands in it.
                if let (Some(bx), Some(rg)) = (boxed.filter(|bx| b == Some(*bx)), e.view.region) {
                    let pad = 0.02 * (bx[2] - bx[0]).hypot(bx[3] - bx[1]);
                    let wide = [bx[0] - pad, bx[1] - pad, bx[2] + pad, bx[3] + pad];
                    let mut by_box = Enlarged::default();
                    count_devices(&mut by_box, e.page, &smaller, &|p| inside(&wide, f.apply(p)), &device_type);
                    // Only more of the same plan, never a swap for somewhere else: most of what the
                    // view counted lands in the box too.
                    let kept = entry.device_handles.iter().filter(|h| by_box.device_handles.contains(h)).count();
                    if by_box.devices > entry.devices && kept * 2 >= entry.devices && covered(&map_box(&f, &rg), &bx) >= 0.7 {
                        entry.devices = by_box.devices;
                        entry.devices_by_type = by_box.devices_by_type;
                        entry.device_handles = by_box.device_handles;
                        entry.devices_from = "boundary".into();
                    }
                }
                if let Some(b) = b {
                    entry.ignored_handles = h
                        .read
                        .devices
                        .iter()
                        .filter(|dv| inside(&b, dv.at) && device_type(&h.read.legend_types, &dv.tag).is_some())
                        .map(|dv| dv.handle.clone())
                        .collect();
                    entry.overall_ignored = entry.ignored_handles.len();
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
            if device_type(&p.read.legend_types, &dv.tag).is_none() {
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
        if let (Some(b), Some(h)) = (e.boundary, by_number(&e.overall_sheet))
            && e.registered
        {
            let kept = boundaries.get(&h.sheet.number).is_none_or(|bs| !bs.contains(&b));
            if kept {
                counted_inside +=
                    h.read.devices.iter().filter(|dv| inside(&b, dv.at) && device_type(&h.read.legend_types, &dv.tag).is_some()).count();
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
                p.read.devices.iter().filter(|dv| dist(dv.at, c.at) <= reach && device_type(&p.read.legend_types, &dv.tag).is_some()).collect();
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

/// Whether a mark could be one of the set's sheet numbers: its letters are a discipline the set
/// has, written the way the set writes that discipline's numbers (with a dash or without, no
/// spaces).
fn sheet_shaped(mark: &str, sheets: &[SetSheet]) -> bool {
    let m = mark.trim();
    let letters: String = m.chars().take_while(char::is_ascii_alphabetic).collect::<String>().to_ascii_uppercase();
    let same: Vec<&str> = sheets
        .iter()
        .map(|x| x.number.as_str())
        .filter(|n| n.chars().take_while(char::is_ascii_alphabetic).collect::<String>().eq_ignore_ascii_case(&letters))
        .collect();
    if letters.is_empty() || same.is_empty() || m.contains(' ') {
        return false;
    }
    let dashed = same.iter().filter(|n| n.contains('-')).count();
    let dash = m.contains('-');
    (dash && dashed > 0) || (!dash && dashed < same.len())
}

/// The views of one several-view sheet (`E-2/1`, `E-2/2`): one page, already one plan each.
fn plans_of_one_page(sheets: &[String]) -> bool {
    let base = |s: &String| s.split('/').next().unwrap_or(s).to_string();
    sheets.first().is_some_and(|f| sheets.iter().all(|s| base(s) == base(f)) && sheets.len() > 1 && f.contains('/'))
}

/// Levels in building order (numbered floors, then the roof; unnamed last).
fn level_key(l: &str) -> (bool, bool, u32) {
    (l.is_empty(), l == "R", l.parse::<u32>().unwrap_or(u32::MAX))
}

/// The level an enlarged view shows: its title's, else its sheet's.
fn level_of_view(p: &LinkPage, v: &PageView) -> String {
    sheet_reader::detect_levels(&v.title).into_iter().next().unwrap_or_else(|| p.sheet.level.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str, x: f64, y: f64, height: f64, rotation: f64) -> TextItem {
        TextItem { value: value.into(), at: cadaddy_geom::Vec2::new(x, y), height, rotation }
    }

    #[test]
    fn the_ocr_copy_of_a_line_counts_once() {
        let up = std::f64::consts::FRAC_PI_2;
        let texts = vec![
            // The hidden text, whole, and the OCR copy a little off it.
            text("SEE SHEET E-4 FOR MOUNTING HEIGHTS", 96.0, 2073.6, 6.87, 0.0),
            text("SEE SHEET E-4  FOR MOUNTING HEIGHTS", 97.5, 2074.4, 9.6, 0.0),
            // A line reading up the sheet, read by the OCR in two pieces along it.
            text("MATCH LINE - SEE SHEET E-2", 1785.6, 576.0, 6.87, up),
            text("MATCH LINE", 1790.4, 576.0, 9.6, up),
            text("SEE SHEET E-2", 1790.4, 681.6, 9.6, up),
            // Cut short.
            text("FIRST FLOOR POWER PLAN - AREA A", 384.0, 1776.0, 17.18, 0.0),
            text("FIRST FLOOR POWER PLAN", 384.0, 1776.0, 24.0, 0.0),
            // The same words elsewhere are another note.
            text("SEE SHEET E-2", 600.0, 900.0, 9.6, 0.0),
            text("E-4", 1351.1, 1571.5, 8.25, 0.0),
        ];
        let kept: Vec<(String, f64)> = one_copy(texts).into_iter().map(|t| (t.value, t.at.x)).collect();
        let want = [
            ("SEE SHEET E-4 FOR MOUNTING HEIGHTS", 96.0),
            ("MATCH LINE - SEE SHEET E-2", 1785.6),
            ("FIRST FLOOR POWER PLAN - AREA A", 384.0),
            ("SEE SHEET E-2", 600.0),
            ("E-4", 1351.1),
        ];
        assert_eq!(kept, want.map(|(v, x)| (v.to_string(), x)).to_vec());
    }

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

    /// A dashed box (dashes 1 long, gaps 0.5) whose right side runs along a solid line, and a
    /// solid outline of dashes' size pieces that is not dashed.
    #[test]
    fn a_dashed_box_is_found_from_its_dashes() {
        let mut segs: Vec<[[f64; 2]; 2]> = Vec::new();
        let mut dashes = |a: [f64; 2], b: [f64; 2]| {
            let len = dist(a, b);
            let n = (len / 1.5).floor() as usize;
            for k in 0..n {
                let f0 = k as f64 * 1.5 / len;
                let f1 = (k as f64 * 1.5 + 1.0) / len;
                segs.push([[a[0] + (b[0] - a[0]) * f0, a[1] + (b[1] - a[1]) * f0], [a[0] + (b[0] - a[0]) * f1, a[1] + (b[1] - a[1]) * f1]]);
            }
        };
        dashes([10.0, 10.0], [40.0, 10.0]);
        dashes([10.0, 30.0], [40.0, 30.0]);
        dashes([10.0, 10.0], [10.0, 30.0]);
        segs.push([[40.0, 0.0], [40.0, 50.0]]);
        // Solid: pieces end to end.
        for k in 0..20 {
            let x = 60.0 + k as f64;
            segs.push([[x, 10.0], [x + 1.0, 10.0]]);
            segs.push([[x, 30.0], [x + 1.0, 30.0]]);
        }
        let found = dash_boxes(&segs, 1.0);
        assert_eq!(found.len(), 1, "{found:?}");
        let b = found[0];
        for (got, want) in b.iter().zip([10.0, 10.0, 40.0, 30.0]) {
            assert!((got - want).abs() < 0.6, "{b:?}");
        }
    }

    #[test]
    fn mounting_heights_are_read_from_notes() {
        assert_eq!(mounting_height("MOUNT AT 48\" AFF"), Some(4.0));
        assert_eq!(mounting_height("+18\" A.F.F."), Some(1.5));
        assert_eq!(mounting_height("7'-6\" AFF TO BOTTOM"), Some(7.5));
        assert_eq!(mounting_height("PROVIDE BOX"), None);
    }
}
