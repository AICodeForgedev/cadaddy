//! Revision currency between disciplines (Q6650): a revision that moves or adds an object on one
//! discipline's sheet must reach every other sheet of that level that draws the object or its
//! supports.
//!
//! - Each sheet's revisions are read off its title block's revision table: rows of a mark, a
//!   description and a date in columns of any order, under or over a header (`REV DESCRIPTION
//!   DATE`) or with no header read at all when two rows or more line up. The latest printed date
//!   alone (the index's `issued`) can't tell them apart: every sheet of a set carries the set's
//!   plot or issue date.
//! - A sheet's revised places are its revision clouds: a closed chain of short arcs (how a PDF
//!   plots one, each scallop a curve of its own), a closed polyline of bulged segments (a CAD
//!   cloud), or one closed outline of scallops. The delta by a cloud names its revision (a text
//!   equal to a row's mark on the cloud's edge); with none, the sheet's latest revision.
//! - The objects a cloud moves or adds are the equipment marks inside it (`RTU-2`, or `CU` over
//!   `1`), by the trades' common prefixes ([`revcurrency.json`]: rule data).
//! - Every other discipline's sheet of a level the revised sheet shows, whose table has no row of
//!   that revision or later, is checked: where it draws the same mark, its place is compared
//!   (the two sheets aligned by the notes and grid labels they share); where it draws no mark but
//!   draws the objects' supports (curbs, walk pads, penetrations: the rule data's words), it is
//!   not revised for a revision that moved what it supports. Each such sheet and revision is one
//!   RFI naming both sheets and the revision; the model keeps the revised sheet's places.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use cadaddy_doc::{Drawing, EntityKind};
use cadaddy_geom::Vec2;
use cadaddy_mepfix::sheet_facts::TextItem;
use cadaddy_walk3d::rfi::{Rfi, RfiKind, RfiSide};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::guided::SheetRegion;

const RULES: &str = include_str!("revcurrency.json");

/// Sheets compared, at most.
const MAX_SHEETS: usize = 400;
/// Texts kept per sheet, at most.
const MAX_SHEET_TEXTS: usize = 100_000;
/// Curves kept per sheet, at most.
const MAX_SHEET_CURVES: usize = 200_000;
/// Points of one curve looked at, at most.
const MAX_CURVE_POINTS: usize = 4_000;
/// Rows of one revision table, at most.
const MAX_ROWS: usize = 40;
/// Clouds of one sheet, at most.
const MAX_CLOUDS: usize = 200;
/// Marks of one cloud or sheet, at most.
const MAX_MARKS: usize = 2_000;
/// Objects one finding names, at most.
const MAX_NAMED: usize = 16;
/// The topic of this check's RFIs (the build replaces its own items on each run).
pub(crate) const TOPIC: &str = "revision currency: a sheet not revised for another discipline's revision";
/// Id prefix of this check's RFIs.
pub(crate) const ID_PREFIX: &str = "RFI-RV";

/// The rules ([`revcurrency.json`]).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub header_date_words: Vec<String>,
    pub header_other_words: Vec<String>,
    pub mark_max_chars: usize,
    pub row_gap_h: f64,
    pub min_cloud_pieces: usize,
    /// A scallop's chord as a share of its sheet's drawing area width, at most.
    pub scallop_max_share: f64,
    pub equipment_prefixes: Vec<String>,
    pub support_words: Vec<String>,
    pub same_place_h: f64,
    pub align_min_votes: usize,
    pub max_findings: usize,
}

pub(crate) fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// One row of a sheet's revision table.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub(crate) struct Revision {
    pub mark: String,
    pub description: String,
    /// `YYYY-MM-DD`.
    pub date: String,
}

impl Revision {
    /// `revision 1 (ASI 07, 2024-11-18)`.
    pub fn label(&self) -> String {
        if self.description.is_empty() {
            format!("revision {} ({})", self.mark, self.date)
        } else {
            format!("revision {} ({}, {})", self.mark, self.description, self.date)
        }
    }

    /// Whether this row brings a sheet up to `rev`: the same revision (its description) or a
    /// later one.
    fn reaches(&self, rev: &Revision) -> bool {
        self.date >= rev.date || (!rev.description.is_empty() && norm(&self.description) == norm(&rev.description))
    }
}

fn norm(s: &str) -> String {
    s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ").to_ascii_uppercase()
}

// ─── The revision table ─────────────────────────────────────────────────────

/// One field of a line of text: a text item, or a part of one split at a run of spaces (a table
/// row plotted as one text: `0   Issued for Permit   05/02/24`).
#[derive(Clone, Debug)]
struct Field {
    text: String,
    x: f64,
    y: f64,
    h: f64,
}

/// A letter's advance in its text's height, about (drafting fonts run 0.6 to 0.9).
const CHAR_ADVANCE: f64 = 0.6;

fn horizontal(t: &TextItem) -> bool {
    t.rotation.sin().abs() < 0.05 && t.rotation.cos() > 0.0
}

fn fields(texts: &[TextItem]) -> Vec<Field> {
    let mut out = Vec::new();
    for t in texts.iter().filter(|t| horizontal(t)) {
        let chars: Vec<char> = t.value.chars().take(400).collect();
        let mut i = 0usize;
        while i < chars.len() {
            // Skip the gap, then take a part up to the next run of two spaces or more.
            while chars.get(i).is_some_and(|c| c.is_whitespace()) {
                i += 1;
            }
            let start = i;
            while i < chars.len() {
                let gap = chars.get(i).is_some_and(|c| *c == '\t')
                    || (chars.get(i).is_some_and(|c| c.is_whitespace()) && chars.get(i + 1).is_some_and(|c| c.is_whitespace()));
                if gap {
                    break;
                }
                i += 1;
            }
            let part: String = chars.get(start..i).map(|s| s.iter().collect()).unwrap_or_default();
            let part = part.trim().to_string();
            if !part.is_empty() {
                out.push(Field { text: part, x: t.at.x + start as f64 * CHAR_ADVANCE * t.height, y: t.at.y, h: t.height });
            }
        }
    }
    out
}

/// A field that is a date and nothing else (`05/02/24`, `2024-11-18`): the date.
fn date_field(s: &str) -> Option<String> {
    let s = s.trim();
    if s.chars().count() > 12 || s.chars().any(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    cadaddy_mepfix::printset::latest_date(s)
}

/// A field that is a revision mark (`0`, `1`, `A`, `12`).
fn mark_field(s: &str, max: usize) -> bool {
    let n = s.chars().count();
    n >= 1 && n <= max.max(1) && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Lines of fields, top first: fields within a little of one baseline.
fn lines(mut fs: Vec<Field>) -> Vec<Vec<Field>> {
    fs.sort_by(|a, b| b.y.total_cmp(&a.y).then(a.x.total_cmp(&b.x)));
    let mut out: Vec<Vec<Field>> = Vec::new();
    for f in fs {
        match out.last_mut() {
            Some(l) if l.first().is_some_and(|g| (g.y - f.y).abs() <= 0.4 * g.h.max(f.h)) => l.push(f),
            _ => out.push(vec![f]),
        }
    }
    for l in &mut out {
        l.sort_by(|a, b| a.x.total_cmp(&b.x));
    }
    out
}

struct Row {
    rev: Revision,
    mark_x: f64,
    date_x: f64,
    y: f64,
    h: f64,
}

/// A line as a revision row: one date, a mark (the first mark field) and the rest its
/// description.
fn row(line: &[Field], max_mark: usize) -> Option<Row> {
    let dates: Vec<(usize, String)> = line.iter().enumerate().filter_map(|(i, f)| date_field(&f.text).map(|d| (i, d))).collect();
    let [(di, date)] = dates.as_slice() else { return None };
    let mi = line.iter().enumerate().position(|(i, f)| i != *di && mark_field(&f.text, max_mark))?;
    let description = line.iter().enumerate().filter(|(i, _)| *i != *di && *i != mi).map(|(_, f)| f.text.as_str()).collect::<Vec<_>>().join(" ");
    let (m, d) = (line.get(mi)?, line.get(*di)?);
    Some(Row {
        rev: Revision { mark: m.text.to_ascii_uppercase(), description: description.trim().to_string(), date: date.clone() },
        mark_x: m.x,
        date_x: d.x,
        y: m.y,
        h: m.h.max(d.h),
    })
}

/// Whether a line is a revision table's header: a date word and another of the header's words.
fn header(line: &[Field], r: &Rules) -> bool {
    let words: Vec<String> =
        line.iter().flat_map(|f| f.text.split_whitespace().map(|w| w.trim_end_matches(':').to_ascii_uppercase()).collect::<Vec<_>>()).collect();
    let has = |list: &[String]| words.iter().any(|w| list.iter().any(|x| x.eq_ignore_ascii_case(w)));
    has(&r.header_date_words) && has(&r.header_other_words)
}

/// A sheet's revision table from the texts of its title block, oldest first: the rows that line
/// up (marks in one column, dates in another, a row's height or so apart) next to a header, or
/// two rows or more with no header read. Empty when none reads.
pub(crate) fn revision_table(texts: &[TextItem]) -> Vec<Revision> {
    let r = rules();
    let ls = lines(fields(texts));
    let headers: Vec<(f64, f64, f64, f64)> = ls
        .iter()
        .filter(|l| header(l, r))
        .filter_map(|l| {
            let (a, b) = (l.first()?, l.last()?);
            Some((a.y, a.x, b.x + b.text.chars().count() as f64 * CHAR_ADVANCE * b.h, a.h))
        })
        .collect();
    // Rows in groups: a row joins the group above when its columns line up and it is near.
    let mut groups: Vec<Vec<Row>> = Vec::new();
    for rw in ls.iter().filter_map(|l| row(l, r.mark_max_chars)) {
        let joins = groups.last().and_then(|g| g.last()).is_some_and(|p| {
            (p.mark_x - rw.mark_x).abs() <= 1.5 * p.h && (p.date_x - rw.date_x).abs() <= 4.0 * p.h && p.y - rw.y <= r.row_gap_h * p.h
        });
        match groups.last_mut() {
            Some(g) if joins && g.len() < MAX_ROWS => g.push(rw),
            _ => groups.push(vec![rw]),
        }
    }
    let headed = |g: &[Row]| {
        let (Some(top), Some(bottom)) = (g.first(), g.last()) else { return false };
        let (x0, x1) = (top.mark_x.min(top.date_x), top.mark_x.max(top.date_x));
        headers.iter().any(|&(y, hx0, hx1, h)| {
            let near = (y > top.y && y - top.y <= r.row_gap_h * h) || (y < bottom.y && bottom.y - y <= r.row_gap_h * h);
            near && hx0 <= x1 + 2.0 * h && hx1 >= x0 - 2.0 * h
        })
    };
    let best = groups.into_iter().filter(|g| headed(g) || g.len() >= 2).max_by_key(|g| (headed(g), g.len()));
    let mut rows: Vec<Revision> = best.map(|g| g.into_iter().map(|w| w.rev).collect()).unwrap_or_default();
    rows.sort_by(|a, b| a.date.cmp(&b.date).then(a.mark.cmp(&b.mark)));
    rows
}

// ─── Clouds ─────────────────────────────────────────────────────────────────

/// A curve of a sheet: its points, whether it closes, its bulged segments.
#[derive(Clone, Debug)]
pub(crate) struct Curve {
    pts: Vec<Vec2>,
    closed: bool,
    bulges: usize,
}

/// A revision cloud's extent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Cloud {
    pub min: Vec2,
    pub max: Vec2,
}

impl Cloud {
    fn contains(&self, p: Vec2, pad: f64) -> bool {
        p.x >= self.min.x - pad && p.x <= self.max.x + pad && p.y >= self.min.y - pad && p.y <= self.max.y + pad
    }

    /// How far `p` is from the cloud's edge (0 on it).
    fn edge_distance(&self, p: Vec2) -> f64 {
        let dx = (self.min.x - p.x).max(p.x - self.max.x);
        let dy = (self.min.y - p.y).max(p.y - self.max.y);
        if dx <= 0.0 && dy <= 0.0 { dx.max(dy).abs() } else { dx.max(0.0).hypot(dy.max(0.0)) }
    }
}

fn bounds(pts: &[Vec2]) -> Option<Cloud> {
    let first = pts.first()?;
    let mut c = Cloud { min: *first, max: *first };
    for p in pts {
        c.min = Vec2::new(c.min.x.min(p.x), c.min.y.min(p.y));
        c.max = Vec2::new(c.max.x.max(p.x), c.max.y.max(p.y));
    }
    Some(c)
}

/// How far a curve's points stand off its chord, at most.
fn sagitta(pts: &[Vec2]) -> f64 {
    let (Some(a), Some(b)) = (pts.first(), pts.last()) else { return 0.0 };
    let d = *b - *a;
    let len = d.len();
    if len <= 0.0 {
        return 0.0;
    }
    pts.iter().map(|p| ((*p - *a).x * d.y - (*p - *a).y * d.x).abs() / len).fold(0.0, f64::max)
}

/// Turns sharper than this at a vertex are a scallop's cusp (degrees).
const CUSP_DEG: f64 = 50.0;

/// One closed outline of scallops: cusps all round, each scallop a curve of a few points.
fn scalloped(pts: &[Vec2], min_pieces: usize) -> bool {
    let n = pts.len();
    if n < 3 * min_pieces {
        return false;
    }
    let mut cusps = 0usize;
    for i in 0..n {
        let (Some(a), Some(b), Some(c)) = (pts.get((i + n - 1) % n), pts.get(i), pts.get((i + 1) % n)) else { continue };
        let (u, v) = (*b - *a, *c - *b);
        let (lu, lv) = (u.len(), v.len());
        if lu <= 0.0 || lv <= 0.0 {
            continue;
        }
        let cos = ((u.x * v.x + u.y * v.y) / (lu * lv)).clamp(-1.0, 1.0);
        if cos.acos().to_degrees() > CUSP_DEG {
            cusps += 1;
        }
    }
    cusps >= min_pieces && n >= 3 * cusps
}

/// Gaps in one cloud's outline, at most.
const MAX_CLOUD_GAPS: usize = 3;
/// A cloud's gaps together, as a share of its outline, at most.
const MAX_GAP_SHARE: f64 = 0.15;

/// The open ends of a chain paired nearest first: their gaps' length together.
fn gaps(ends: &[Vec2]) -> f64 {
    let mut left: Vec<Vec2> = ends.to_vec();
    let mut total = 0.0;
    while let Some(a) = left.pop() {
        let Some((i, d)) = left.iter().enumerate().map(|(i, b)| (i, a.dist(*b))).min_by(|x, y| x.1.total_cmp(&y.1)) else { return f64::INFINITY };
        total += d;
        left.swap_remove(i);
    }
    total
}

/// The revision clouds among a sheet's curves. `width` is the sheet's drawing area width.
pub(crate) fn clouds(curves: &[Curve], width: f64) -> Vec<Cloud> {
    let r = rules();
    let min_pieces = r.min_cloud_pieces.max(3);
    let max_chord = width.abs() * r.scallop_max_share.max(0.0);
    let mut out = Vec::new();
    // Scallops: short open arcs, each standing off its chord like an arc does.
    let mut pieces: Vec<(Vec2, Vec2, f64)> = Vec::new();
    for c in curves {
        let ends_meet = match (c.pts.first(), c.pts.last()) {
            (Some(a), Some(b)) => a.dist(*b) <= 1e-9 * width.abs().max(1.0),
            _ => false,
        };
        if c.closed || ends_meet {
            let pieces_bulged = c.bulges >= min_pieces;
            if (pieces_bulged || scalloped(&c.pts, min_pieces))
                && let Some(b) = bounds(&c.pts)
            {
                out.push(b);
            }
            continue;
        }
        if c.pts.len() < 4 {
            continue;
        }
        let (Some(a), Some(b)) = (c.pts.first(), c.pts.last()) else { continue };
        let chord = a.dist(*b);
        if chord <= 0.0 || chord > max_chord {
            continue;
        }
        let s = sagitta(&c.pts);
        if s >= 0.1 * chord && s <= 0.75 * chord {
            pieces.push((*a, *b, chord));
        }
    }
    if pieces.len() >= min_pieces {
        let mut chords: Vec<f64> = pieces.iter().map(|p| p.2).collect();
        chords.sort_by(f64::total_cmp);
        let median = chords.get(chords.len() / 2).copied().unwrap_or(1.0);
        let tol = (0.15 * median).max(1e-9);
        let key = |p: Vec2| ((p.x / tol).floor() as i64, (p.y / tol).floor() as i64);
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, (a, b, _)) in pieces.iter().enumerate() {
            for p in [a, b] {
                grid.entry(key(*p)).or_default().push(i);
            }
        }
        // Pieces joined end to end, and the ends no other piece meets.
        let mut parent: Vec<usize> = (0..pieces.len()).collect();
        fn find(parent: &mut [usize], mut i: usize) -> usize {
            let mut guard = 0usize;
            while let Some(&p) = parent.get(i) {
                if p == i || guard > parent.len() {
                    break;
                }
                i = p;
                guard += 1;
            }
            i
        }
        let mut open: Vec<Vec<Vec2>> = vec![Vec::new(); pieces.len()];
        for (i, (a, b, _)) in pieces.iter().enumerate() {
            for p in [*a, *b] {
                let (kx, ky) = key(p);
                let mut met = false;
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        for &j in grid.get(&(kx + dx, ky + dy)).into_iter().flatten() {
                            let Some((ja, jb, _)) = pieces.get(j) else { continue };
                            if j != i && (ja.dist(p) <= tol || jb.dist(p) <= tol) {
                                met = true;
                                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                                if ri != rj
                                    && let Some(x) = parent.get_mut(ri)
                                {
                                    *x = rj;
                                }
                            }
                        }
                    }
                }
                if !met && let Some(o) = open.get_mut(i) {
                    o.push(p);
                }
            }
        }
        let mut comps: BTreeMap<usize, (Vec<Vec2>, Vec<Vec2>, usize)> = BTreeMap::new();
        for (i, (a, b, _)) in pieces.iter().enumerate() {
            let root = find(&mut parent, i);
            let e = comps.entry(root).or_default();
            e.0.extend([*a, *b]);
            e.1.extend(open.get(i).into_iter().flatten().copied());
            e.2 += 1;
        }
        for (pts, ends, n) in comps.values() {
            if *n < min_pieces || out.len() >= MAX_CLOUDS {
                continue;
            }
            // Closed: every end met, or the gaps left (a leader or a masked note breaking the
            // outline) short beside the whole outline.
            let closed = ends.len() % 2 == 0 && ends.len() <= 2 * MAX_CLOUD_GAPS && gaps(ends) <= MAX_GAP_SHARE * *n as f64 * median;
            let Some(b) = bounds(pts) else { continue };
            if closed && b.max.x - b.min.x >= 2.0 * median && b.max.y - b.min.y >= 2.0 * median {
                out.push(b);
            }
        }
    }
    out.truncate(MAX_CLOUDS);
    out
}

// ─── Marks ──────────────────────────────────────────────────────────────────

/// A mark's letters and number as one word: `RTU-2`, `AHU-1`, `EF2` → (`EF`, `2`).
fn split_mark(w: &str) -> Option<(String, String)> {
    let w = w.trim().to_ascii_uppercase();
    let letters: String = w.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let rest = w.get(letters.len()..)?.trim_start_matches('-');
    let number_ok = !rest.is_empty()
        && rest.len() <= 4
        && rest.chars().next().is_some_and(|c| c.is_ascii_digit())
        && rest.chars().all(|c| c.is_ascii_alphanumeric());
    (!letters.is_empty() && number_ok).then(|| (letters, rest.to_string()))
}

fn prefix_known(p: &str, r: &Rules) -> bool {
    r.equipment_prefixes.iter().any(|x| x.eq_ignore_ascii_case(p))
}

/// Where a text's lettering is centred, about.
fn text_center(t: &TextItem) -> Vec2 {
    Vec2::new(t.at.x + t.value.chars().count() as f64 * CHAR_ADVANCE * t.height / 2.0, t.at.y + t.height / 2.0)
}

/// The equipment marks among `texts` (a mark as one word, or its letters over its number, as a
/// stacked tag prints it), with where each is. Numbers a stacked mark took are left out of
/// `used` too.
fn marks(texts: &[TextItem], r: &Rules) -> Vec<(String, Vec2, Vec<usize>)> {
    let mut out: Vec<(String, Vec2, Vec<usize>)> = Vec::new();
    for (i, t) in texts.iter().enumerate() {
        if out.len() >= MAX_MARKS {
            break;
        }
        let v = t.value.trim().to_ascii_uppercase();
        if let Some((p, n)) = split_mark(&v) {
            if prefix_known(&p, r) {
                out.push((format!("{p}-{n}"), text_center(t), vec![i]));
            }
            continue;
        }
        if !(2..=5).contains(&v.len()) || !v.chars().all(|c| c.is_ascii_alphabetic()) || !prefix_known(&v, r) {
            continue;
        }
        // Its number under it: digits (maybe a letter after), within a few heights.
        let c = text_center(t);
        let below = texts
            .iter()
            .enumerate()
            .filter(|(j, n)| {
                let s = n.value.trim();
                *j != i
                    && !s.is_empty()
                    && s.len() <= 4
                    && s.chars().next().is_some_and(|ch| ch.is_ascii_digit())
                    && s.chars().all(|ch| ch.is_ascii_alphanumeric())
            })
            .filter(|(_, n)| {
                let nc = text_center(n);
                let dy = c.y - nc.y;
                dy > 0.3 * t.height && dy <= 3.0 * t.height && (nc.x - c.x).abs() <= 2.0 * t.height
            })
            .min_by(|a, b| text_center(a.1).dist(c).total_cmp(&text_center(b.1).dist(c)));
        if let Some((j, n)) = below {
            out.push((format!("{v}-{}", n.value.trim().to_ascii_uppercase()), c, vec![i, j]));
        }
    }
    out
}

// ─── Sheets ─────────────────────────────────────────────────────────────────

/// One sheet of the set as this check reads it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Sheet {
    pub number: String,
    pub discipline: String,
    pub levels: BTreeSet<String>,
    rect: [f64; 4],
    area: [f64; 4],
    title_block: Option<[f64; 4]>,
    texts: Vec<TextItem>,
    curves: Vec<Curve>,
}

fn norm_rect(r: [f64; 4]) -> [f64; 4] {
    [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

fn inside(r: &[f64; 4], p: Vec2) -> bool {
    p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3]
}

/// The plan sheets of the job's regions (one per sheet number, every level its regions cover),
/// with the texts and curves the drawing has on each.
pub(crate) fn sheets(d: &Drawing, regions: &[SheetRegion]) -> Vec<Sheet> {
    let mut out: Vec<Sheet> = Vec::new();
    for r in regions.iter().filter(|r| r.not_built.is_empty() && matches!(r.kind.as_str(), "plan" | "enlarged_plan" | "rcp")) {
        let levels = r.all_levels();
        if levels.is_empty() {
            continue;
        }
        if let Some(s) = out.iter_mut().find(|s| s.number == r.number) {
            s.levels.extend(levels);
            continue;
        }
        if out.len() >= MAX_SHEETS {
            break;
        }
        let rect = norm_rect(r.rect);
        out.push(Sheet {
            number: r.number.clone(),
            discipline: r.discipline.clone(),
            levels: levels.into_iter().collect(),
            rect,
            area: norm_rect(r.area.unwrap_or(r.rect)),
            title_block: r.title_block.map(norm_rect),
            ..Sheet::default()
        });
    }
    fill(d, &mut out);
    out
}

/// Each sheet's texts (block attributes too) and curves, in one pass over the drawing.
fn fill(d: &Drawing, sheets: &mut [Sheet]) {
    if sheets.is_empty() {
        return;
    }
    // A grid over the sheets' rectangles finds the one a point is on.
    let mut widths: Vec<f64> =
        sheets.iter().map(|s| (s.rect[2] - s.rect[0]).max(s.rect[3] - s.rect[1])).filter(|w| *w > 0.0 && w.is_finite()).collect();
    widths.sort_by(f64::total_cmp);
    let cell = widths.get(widths.len() / 2).copied().unwrap_or(1.0).max(1e-6);
    let cell_of = |v: f64| (v / cell).floor() as i64;
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, s) in sheets.iter().enumerate() {
        let (x0, y0, x1, y1) = (cell_of(s.rect[0]), cell_of(s.rect[1]), cell_of(s.rect[2]), cell_of(s.rect[3]));
        if (x1 - x0).abs() > 64 || (y1 - y0).abs() > 64 {
            continue;
        }
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                grid.entry((cx, cy)).or_default().push(i);
            }
        }
    }
    let rects: Vec<[f64; 4]> = sheets.iter().map(|s| s.rect).collect();
    let areas: Vec<[f64; 4]> = sheets.iter().map(|s| s.area).collect();
    let on = |p: Vec2| -> Option<usize> {
        if !p.is_finite() {
            return None;
        }
        grid.get(&(cell_of(p.x), cell_of(p.y)))?.iter().copied().find(|&i| rects.get(i).is_some_and(|r| inside(r, p)))
    };
    let text = |value: &str, at: Vec2, height: f64, rotation: f64| -> Option<TextItem> {
        let value = value.trim();
        (!value.is_empty() && height.is_finite() && height > 0.0).then(|| TextItem {
            value: value.to_string(),
            at,
            height,
            rotation: if rotation.is_finite() { rotation } else { 0.0 },
        })
    };
    for e in d.model.iter() {
        let mut items: Vec<TextItem> = Vec::new();
        let mut curve: Option<Curve> = None;
        match &e.kind {
            EntityKind::Text(t) => items.extend(text(&t.value, t.insert.xy(), t.height, t.rotation)),
            EntityKind::MText(t) => items.extend(text(&t.contents, t.insert.xy(), t.height, t.rotation)),
            EntityKind::Insert(ins) => {
                for a in ins.attribs.iter().filter(|a| !a.invisible).take(16) {
                    items.extend(text(&a.text.value, a.text.insert.xy(), a.text.height, a.text.rotation));
                }
            }
            EntityKind::LwPolyline(p) => {
                let pts: Vec<Vec2> = p.vertices.iter().take(MAX_CURVE_POINTS).map(|v| v.p).collect();
                let bulges = p.vertices.iter().take(MAX_CURVE_POINTS).filter(|v| v.bulge.abs() >= 0.2).count();
                curve = Some(Curve { pts, closed: p.closed, bulges });
            }
            EntityKind::Arc(a) => {
                let sweep = (a.end - a.start).rem_euclid(std::f64::consts::TAU);
                if a.radius.is_finite() && a.radius > 0.0 && sweep > 0.0 {
                    let steps = 6usize;
                    let pts = (0..=steps)
                        .map(|k| {
                            let t = a.start + sweep * k as f64 / steps as f64;
                            Vec2::new(a.center.x + a.radius * t.cos(), a.center.y + a.radius * t.sin())
                        })
                        .collect();
                    curve = Some(Curve { pts, closed: false, bulges: 0 });
                }
            }
            _ => {}
        }
        for t in items {
            if let Some(s) = on(t.at).and_then(|i| sheets.get_mut(i))
                && s.texts.len() < MAX_SHEET_TEXTS
            {
                s.texts.push(t);
            }
        }
        if let Some(c) = curve
            && let Some(first) = c.pts.first().copied()
            && let Some(i) = on(first)
            && areas.get(i).is_some_and(|a| inside(a, first))
            && let Some(s) = sheets.get_mut(i)
            && s.curves.len() < MAX_SHEET_CURVES
        {
            s.curves.push(c);
        }
    }
}

impl Sheet {
    fn area_texts(&self) -> Vec<TextItem> {
        self.texts.iter().filter(|t| inside(&self.area, t.at)).cloned().collect()
    }

    /// The texts of its title block (outside its drawing area when no title block was found).
    fn title_texts(&self) -> Vec<TextItem> {
        match self.title_block {
            Some(tb) => self.texts.iter().filter(|t| inside(&tb, t.at)).cloned().collect(),
            None => self.texts.iter().filter(|t| !inside(&self.area, t.at)).cloned().collect(),
        }
    }

    /// The usual height of its drawing area's lettering.
    fn text_height(texts: &[TextItem]) -> f64 {
        let mut hs: Vec<f64> = texts.iter().map(|t| t.height).filter(|h| h.is_finite() && *h > 0.0).collect();
        hs.sort_by(f64::total_cmp);
        hs.get(hs.len() / 2).copied().unwrap_or(1.0)
    }
}

/// How far `b`'s drawing is moved from `a`'s (`b − a`), by the texts each prints once in its
/// drawing area (grid labels, room names, notes): the offset most of them agree on, when it
/// stands clear of any other. None when they don't agree.
fn alignment(a: &[TextItem], b: &[TextItem], h: f64, min_votes: usize) -> Option<Vec2> {
    let once = |ts: &[TextItem]| -> HashMap<String, Vec2> {
        let mut seen: HashMap<String, (Vec2, usize)> = HashMap::new();
        for t in ts {
            let e = seen.entry(t.value.trim().to_ascii_uppercase()).or_insert((t.at, 0));
            e.1 += 1;
        }
        seen.into_iter().filter(|(_, (_, n))| *n == 1).map(|(k, (p, _))| (k, p)).collect()
    };
    let (ma, mb) = (once(a), once(b));
    let cell = (2.0 * h).max(1e-6);
    let mut votes: HashMap<(i64, i64), Vec<Vec2>> = HashMap::new();
    for (k, pa) in &ma {
        if let Some(pb) = mb.get(k) {
            let d = *pb - *pa;
            votes.entry(((d.x / cell).round() as i64, (d.y / cell).round() as i64)).or_default().push(d);
        }
    }
    let near = |c: (i64, i64)| -> Vec<Vec2> {
        let mut v = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                v.extend(votes.get(&(c.0 + dx, c.1 + dy)).into_iter().flatten().copied());
            }
        }
        v
    };
    let (best, members) = votes.keys().map(|c| (*c, near(*c))).max_by_key(|(c, m)| (m.len(), std::cmp::Reverse(*c)))?;
    let rival = votes.keys().filter(|c| (c.0 - best.0).abs() > 2 || (c.1 - best.1).abs() > 2).map(|c| near(*c).len()).max().unwrap_or(0);
    if members.len() < min_votes.max(1) || (members.len() as f64) < 1.5 * rival as f64 {
        return None;
    }
    let n = members.len() as f64;
    let sum = members.iter().fold(Vec2::new(0.0, 0.0), |s, d| s + *d);
    Some(Vec2::new(sum.x / n, sum.y / n))
}

/// The supports' words a sheet's drawing area prints (rule data), as printed in the rule.
fn support_words(texts: &[TextItem], r: &Rules) -> Vec<String> {
    let padded: Vec<String> = texts.iter().map(|t| format!(" {} ", norm(&t.value))).collect();
    r.support_words
        .iter()
        .filter(|w| {
            let w = format!(" {} ", norm(w));
            // A word or its plural (`CURBS`).
            let plural = format!("{}S ", w.trim_end());
            padded.iter().any(|p| p.contains(&w) || p.contains(&plural))
        })
        .cloned()
        .collect()
}

// ─── The check ──────────────────────────────────────────────────────────────

/// What one revised sheet's revision moved or added, as another sheet sees it.
#[derive(Default)]
struct Group {
    level: String,
    revision: Revision,
    /// Revised sheets and the marks each clouds.
    revised: BTreeMap<String, BTreeSet<String>>,
    /// Marks the other sheet shows elsewhere: (mark, distance in drawing units).
    moved: BTreeMap<String, f64>,
    /// Marks the other sheet draws where the two sheets could not be aligned.
    unplaced: BTreeSet<String>,
    /// Marks whose supports the other sheet draws (no mark of its own).
    supported: BTreeSet<String>,
    words: Vec<String>,
    latest: Option<Revision>,
}

/// What the check read and found.
#[derive(Default)]
pub(crate) struct Report {
    pub rfis: Vec<Rfi>,
    /// Each sheet's revisions and clouds, for the job's audit.
    pub sheets: Vec<Value>,
    pub clouds: usize,
    pub clouded_marks: usize,
}

impl Report {
    pub fn json(&self) -> Value {
        json!({
            "sheets_read": self.sheets.len(),
            "sheets_with_revisions": self.sheets.iter().filter(|s| s["revisions"].as_array().is_some_and(|a| !a.is_empty())).count(),
            "clouds": self.clouds,
            "clouded_marks": self.clouded_marks,
            "rfis": self.rfis.len(),
            "sheets": self.sheets.iter().filter(|s| s["revisions"].as_array().is_some_and(|a| a.len() > 1) || s["clouds"].as_u64().is_some_and(|n| n > 0)).take(60).collect::<Vec<_>>(),
            "findings": self.rfis.iter().map(|r| json!({"id": r.id, "level": r.level, "sheets": r.sheets, "about": r.about})).collect::<Vec<_>>(),
        })
    }
}

/// The check over the job's sheets: the RFIs (with this check's [`ID_PREFIX`] and [`TOPIC`]).
/// `upf` is the drawing's units per foot (for the distances an item quotes).
pub(crate) fn check(d: &Drawing, regions: &[SheetRegion], upf: f64) -> Report {
    check_sheets(&sheets(d, regions), upf)
}

pub(crate) fn check_sheets(sheets: &[Sheet], upf: f64) -> Report {
    let r = rules();
    let mut report = Report::default();
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    // What each sheet reads: its revisions, its drawing area's texts, its clouds.
    struct Read {
        revisions: Vec<Revision>,
        texts: Vec<TextItem>,
        h: f64,
        clouds: Vec<(Cloud, usize)>,
        words: Vec<String>,
        marks: BTreeMap<String, Vec2>,
    }
    let mut read: Vec<Read> = Vec::new();
    for s in sheets {
        let revisions = revision_table(&s.title_texts());
        let texts = s.area_texts();
        let h = Sheet::text_height(&texts);
        let width = s.area[2] - s.area[0];
        let mut cl: Vec<(Cloud, usize)> = Vec::new();
        if revisions.len() >= 2 {
            let latest = revisions.len() - 1;
            let stacked: BTreeSet<usize> = marks(&texts, r).into_iter().flat_map(|m| m.2).collect();
            for c in clouds(&s.curves, width) {
                // The delta on its edge names its revision; with none, the latest.
                let delta = texts
                    .iter()
                    .enumerate()
                    .filter(|(i, t)| !stacked.contains(i) && c.contains(t.at, 3.0 * h) && c.edge_distance(text_center(t)) <= 3.0 * h)
                    .filter_map(|(_, t)| revisions.iter().position(|rv| rv.mark.eq_ignore_ascii_case(t.value.trim())))
                    .max();
                let at = delta.unwrap_or(latest);
                // The first issue is no revision of the sheet.
                if at > 0 {
                    cl.push((c, at));
                }
            }
        }
        report.clouds += cl.len();
        let words = support_words(&texts, r);
        let mut ms: BTreeMap<String, Vec2> = BTreeMap::new();
        for (m, p, _) in marks(&texts, r) {
            ms.entry(m).or_insert(p);
        }
        report.sheets.push(json!({
            "sheet": s.number, "discipline": s.discipline, "levels": s.levels,
            "revisions": revisions, "clouds": cl.len(),
        }));
        read.push(Read { revisions, texts, h, clouds: cl, words, marks: ms });
    }
    let mut groups: BTreeMap<(String, String, String), Group> = BTreeMap::new();
    let mut aligned: HashMap<(usize, usize), Option<Vec2>> = HashMap::new();
    for (ri, (rs, rr)) in sheets.iter().zip(&read).enumerate() {
        for (cloud, at) in &rr.clouds {
            let Some(rev) = rr.revisions.get(*at) else { continue };
            let in_cloud: Vec<(String, Vec2)> =
                marks(&rr.texts, r).into_iter().filter(|(_, p, _)| cloud.contains(*p, 0.5 * rr.h)).map(|(m, p, _)| (m, p)).collect();
            report.clouded_marks += in_cloud.len();
            if in_cloud.is_empty() {
                continue;
            }
            for (oi, (os, or)) in sheets.iter().zip(&read).enumerate() {
                if oi == ri || os.number == rs.number || os.discipline.eq_ignore_ascii_case(&rs.discipline) || or.revisions.is_empty() {
                    continue;
                }
                let Some(level) = os.levels.intersection(&rs.levels).next() else { continue };
                if or.revisions.iter().any(|x| x.reaches(rev)) {
                    continue;
                }
                let offset = *aligned.entry((ri, oi)).or_insert_with(|| alignment(&rr.texts, &or.texts, rr.h.max(or.h), r.align_min_votes));
                let key = (os.number.clone(), rev.date.clone(), norm(&rev.description));
                for (m, p) in &in_cloud {
                    let g = groups.entry(key.clone()).or_default();
                    match (or.marks.get(m), offset) {
                        (Some(op), Some(off)) => {
                            let dist = op.dist(*p + off);
                            if dist <= r.same_place_h * rr.h.max(or.h) {
                                continue;
                            }
                            g.moved.insert(m.clone(), dist);
                        }
                        (Some(_), None) => {
                            g.unplaced.insert(m.clone());
                        }
                        (None, _) if !or.words.is_empty() => {
                            g.supported.insert(m.clone());
                        }
                        (None, _) => continue,
                    }
                    g.level = level.clone();
                    g.revision = rev.clone();
                    g.words = or.words.clone();
                    g.latest = or.revisions.last().cloned();
                    g.revised.entry(rs.number.clone()).or_default().insert(m.clone());
                }
            }
        }
    }
    for ((other, _, _), g) in groups.into_iter().filter(|(_, g)| !g.revised.is_empty()).take(r.max_findings.max(1)) {
        report.rfis.push(item(&other, &g, upf));
    }
    report
}

fn named(set: impl IntoIterator<Item = String>) -> String {
    let all: Vec<String> = set.into_iter().collect();
    let mut s = all.iter().take(MAX_NAMED).cloned().collect::<Vec<_>>().join(", ");
    if all.len() > MAX_NAMED {
        s.push_str(&format!(" and {} more", all.len() - MAX_NAMED));
    }
    s
}

fn item(other: &str, g: &Group, upf: f64) -> Rfi {
    let rev = g.revision.label();
    let objects: BTreeSet<String> = g.revised.values().flatten().cloned().collect();
    let revised: Vec<String> = g.revised.keys().cloned().collect();
    let latest = g.latest.as_ref().map_or_else(|| "no revision read".to_string(), Revision::label);
    let mut drawn: Vec<String> = Vec::new();
    if !g.moved.is_empty() {
        drawn.push(format!(
            "shows {} at the old place ({})",
            named(g.moved.keys().cloned()),
            g.moved.iter().take(MAX_NAMED).map(|(m, d)| format!("{m} {:.1} ft off", d / upf)).collect::<Vec<_>>().join(", ")
        ));
    }
    if !g.unplaced.is_empty() {
        drawn.push(format!("draws {} (its place could not be compared: the sheets share too few notes to align)", named(g.unplaced.iter().cloned())));
    }
    if !g.supported.is_empty() {
        drawn.push(format!(
            "draws what carries {} ({})",
            named(g.supported.iter().cloned()),
            g.words.iter().map(|w| w.to_ascii_lowercase()).collect::<Vec<_>>().join(", ")
        ));
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("revcurrency|{other}|{}|{}", g.revision.date, norm(&g.revision.description)).bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
    }
    let kind = RfiKind::Conflict;
    let mut sheets = revised.clone();
    sheets.push(other.to_string());
    let mut sides: Vec<RfiSide> = g
        .revised
        .iter()
        .map(|(s, ms)| RfiSide { document: format!("{s}, {rev}"), says: format!("{} at the revised places (clouded)", named(ms.iter().cloned())) })
        .collect();
    sides.push(RfiSide { document: format!("{other}, latest {latest}"), says: format!("not revised for it; {}", drawn.join("; ")) });
    Rfi {
        id: format!("{ID_PREFIX}{:08x}", h as u32),
        kind: Some(kind),
        severity: kind.severity(),
        level: g.level.clone(),
        sheets,
        about: format!("{other}: not revised for {rev}"),
        issue: format!(
            "{} carr{} {rev}, which moves or adds {}; {other} (latest {latest}) is not revised for it and {}.",
            revised.join(" and "),
            if revised.len() == 1 { "ies" } else { "y" },
            named(objects.iter().cloned()),
            drawn.join("; ")
        ),
        assumed: format!(
            "The model follows {} (the later revision governs): the objects stand where it draws them; what {other} draws for them is taken to follow until the design team says otherwise.",
            revised.join(" and ")
        ),
        proposed: Some(format!(
            "Revise {other} for {rev} (what it draws for {}: curbs, pads, supports, penetrations), or confirm the places it shows",
            named(objects.iter().cloned())
        )),
        topic: TOPIC.into(),
        sides,
        ..Rfi::default()
    }
}

#[cfg(test)]
#[path = "revcurrency_tests.rs"]
mod tests;
