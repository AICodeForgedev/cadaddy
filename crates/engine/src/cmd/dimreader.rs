//! Drawn dimensions (Q6178 dimreader): dimension strings a print draws as plain line work and
//! text. A PDF sheet has no DIMENSION objects: its dimensions come in as a dimension line, its
//! witness marks (ticks, dots or arrowheads), extension lines, often dashed where they cross a
//! wall, and the value as text — sometimes two values merged into one text run.
//!
//! - **Reading** ([`read`]): every text holding a length (`8' - 3"`, `8'-3 1/2"`, `27"`, `1/2"`,
//!   `1200 mm`, `1.2 m`) is a value; the line parallel to it beside it is its dimension line; the
//!   marks on that line are the witness positions; the values go to the stretches between them by
//!   where they sit and by the sheet's drawing scale (the median value per drawn length: a value
//!   that doesn't fit its stretch is kept, marked not to scale). Each witness position follows its
//!   extension line out to its far end: the witness point, on the object measured. Stretches of
//!   one line are a chain; dimensions sharing a first witness line are a baseline group.
//! - **Mapping** ([`read_drawing`]): each witness point goes through its sheet's registration
//!   (the view's, else the sheet's) onto its level's reference plan, then by the level's
//!   stacking onto the model. A sheet with no registration borrows the one of a registered sheet
//!   that draws the same plan at the same scale (a trade's enlarged plan over the architect's):
//!   the move most of its long lines take onto lines of the same length there, kept when enough
//!   of them land — method `lines`. A view registered on another level's plan, or not at all,
//!   stays unmapped, with why.
//!
//! General: nothing here knows a job; tolerances are in text heights, so any sheet scale reads.

use std::collections::{BTreeMap, HashMap};

use cadaddy_doc::{Drawing, EntityKind, HAlign};
use cadaddy_geom::Vec2;
use cadaddy_walk3d::BuildingSpec;
use cadaddy_walk3d::levels::{SheetLink, SheetRegistration};

/// Objects of one sheet looked at, at most (hostile input), and sheets read.
const MAX_ITEMS: usize = 400_000;
const MAX_SHEETS: usize = 400;
/// Values read on one sheet, at most, and dimension strings kept per sheet.
const MAX_TOKENS: usize = 20_000;
const MAX_STRINGS: usize = 20_000;
/// A text this long is a note, not a dimension value.
const MAX_VALUE_TEXT: usize = 40;
/// Grid cells one segment is filed under, at most; a longer one is looked at every time.
const MAX_CELLS: usize = 256;
/// A segment, a mark or a text is parallel / square to a line within these sines.
const PARALLEL_SIN: f64 = 0.035;
/// A value sits this far beside its line at most (text heights); its middle within its line's
/// ends, give or take this much.
const BESIDE_H: f64 = 1.8;
const PAST_END_H: f64 = 0.6;
/// A mark is on its line within this (text heights), and is at most this big.
const ON_LINE_H: f64 = 0.4;
const DOT_MAX_H: f64 = 0.7;
const TICK_MIN_H: f64 = 0.25;
const TICK_MAX_H: f64 = 2.5;
const ARROW_MAX_H: f64 = 2.0;
/// Two marks this close along the line are one witness position (text heights).
const SAME_MARK_H: f64 = 0.4;
/// An extension line's pieces (a dashed one) are this far apart at most, and it reaches this
/// far at most (text heights).
const EXT_GAP_H: f64 = 2.0;
const EXT_MAX_H: f64 = 80.0;
/// A symbol stands at a witness point when it is at most this big and this near (text heights).
const SYMBOL_MAX_H: f64 = 6.0;
const SYMBOL_NEAR_H: f64 = 1.5;
/// ... and its middle this near the witness line (text heights, plus a share of its size).
const SYMBOL_ON_LINE_H: f64 = 0.35;
/// Collinear pieces of a dimension line this far apart are one line (text heights).
const LINE_GAP_H: f64 = 1.0;
/// A value fits its stretch when it is within this share (or this many inches) of the stretch's
/// drawn length at the sheet's scale.
const FIT_SHARE: f64 = 0.06;
const FIT_MIN_IN: f64 = 1.0;
/// A borrowed registration needs this share of the sheet's long lines on the registered
/// sheet's (moved), and at least this many: the same plan, not only the same outline (a
/// framing plan shares half its long lines with the floor plan).
const FRAME_MATCH_SHARE: f64 = 0.65;
const FRAME_MATCH_MIN: usize = 20;
/// Long lines compared: at least this share of the sheet's width; the sheet's longest this many
/// vote for the move, the other's this many at most.
const FRAME_LINE_SHARE: f64 = 0.015;
const FRAME_VOTERS: usize = 300;
const FRAME_LINES: usize = 5_000;
/// Model objects scanned for each level's stacking move, at most.
const MAX_MODEL: usize = 2_000_000;

// ─── Values ────────────────────────────────────────────────────────────────

/// A length a text gives: inches, and where in the text (char positions).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Value {
    pub inches: f64,
    pub from: usize,
    pub to: usize,
    pub raw: String,
}

fn num_at(cs: &[char], i: usize) -> Option<(f64, usize)> {
    let mut j = i;
    while cs.get(j).is_some_and(char::is_ascii_digit) {
        j += 1;
    }
    if j == i {
        return None;
    }
    if cs.get(j) == Some(&'.') && cs.get(j + 1).is_some_and(char::is_ascii_digit) {
        j += 1;
        while cs.get(j).is_some_and(char::is_ascii_digit) {
            j += 1;
        }
    }
    let t: String = cs.get(i..j)?.iter().collect();
    t.parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| (v, j))
}

fn spaces(cs: &[char], mut i: usize) -> usize {
    while cs.get(i).is_some_and(|c| *c == ' ' || *c == '\t') {
        i += 1;
    }
    i
}

/// `a/b` at `i`: its value and where it ends.
fn frac_at(cs: &[char], i: usize) -> Option<(f64, usize)> {
    let (a, j) = num_at(cs, i)?;
    if cs.get(j) != Some(&'/') {
        return None;
    }
    let (b, k) = num_at(cs, j + 1)?;
    (b > 0.0).then(|| (a / b, k))
}

/// Inches at `i`: `3`, `3 1/2`, `1/2`, then `"` (or none when `mark_optional`).
fn inches_at(cs: &[char], i: usize, mark_optional: bool) -> Option<(f64, usize)> {
    let (v, mut j) = if let Some((f, j)) = frac_at(cs, i) {
        (f, j)
    } else {
        let (n, j) = num_at(cs, i)?;
        let k = spaces(cs, j);
        match frac_at(cs, k).filter(|_| k > j || cs.get(j) == Some(&'-')) {
            Some((f, e)) => (n + f, e),
            None => match cs.get(j).filter(|c| **c == '-').and_then(|_| frac_at(cs, j + 1)) {
                Some((f, e)) => (n + f, e),
                None => (n, j),
            },
        }
    };
    let k = spaces(cs, j);
    if matches!(cs.get(k), Some('"' | '\u{2033}' | '\u{201d}')) {
        j = k + 1;
        return Some((v, j));
    }
    mark_optional.then_some((v, j))
}

fn word_end(cs: &[char], i: usize) -> bool {
    cs.get(i).is_none_or(|c| !c.is_ascii_alphanumeric())
}

/// The lengths a text gives, in reading order. Feet-inches (`8' - 3"`, `8'-3 1/2"`, `8'`),
/// inches (`27"`, `8 1/2"`, `1/2"`), metric with its unit (`1200 mm`, `120 cm`, `1.2 m`), and a
/// bare number when it is the whole text (`metric_bare`: millimetres, on a metric sheet).
pub(crate) fn values_in(text: &str, metric_bare: bool) -> Vec<Value> {
    let cs: Vec<char> = text.chars().take(MAX_VALUE_TEXT * 4).collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < cs.len() {
        let starts = cs.get(i).is_some_and(char::is_ascii_digit)
            && (i == 0 || cs.get(i - 1).is_some_and(|c| !c.is_ascii_alphanumeric() && *c != '.' && *c != '/'));
        if !starts {
            i += 1;
            continue;
        }
        let raw = |a: usize, b: usize| cs.get(a..b).map(|s| s.iter().collect::<String>()).unwrap_or_default().trim().to_string();
        // Feet, then maybe inches.
        if let Some((ft, j)) = num_at(&cs, i) {
            let k = spaces(&cs, j);
            if matches!(cs.get(k), Some('\'' | '\u{2032}' | '\u{2019}')) {
                let mut e = k + 1;
                let mut inches = 0.0;
                let a = spaces(&cs, e);
                let b = if cs.get(a) == Some(&'-') || cs.get(a) == Some(&'\u{2013}') { spaces(&cs, a + 1) } else { a };
                if let Some((v, end)) = inches_at(&cs, b, b > a || b > e) {
                    inches = v;
                    e = end;
                }
                out.push(Value { inches: ft * 12.0 + inches, from: i, to: e, raw: raw(i, e) });
                i = e;
                continue;
            }
            // Metric with its unit.
            let unit: String = cs.get(k..(k + 2).min(cs.len())).map(|s| s.iter().collect::<String>().to_ascii_lowercase()).unwrap_or_default();
            let metric = if unit.starts_with("mm") && word_end(&cs, k + 2) {
                Some((ft / 25.4, k + 2))
            } else if unit.starts_with("cm") && word_end(&cs, k + 2) {
                Some((ft / 2.54, k + 2))
            } else if unit.starts_with('m') && word_end(&cs, k + 1) {
                Some((ft / 0.0254, k + 1))
            } else {
                None
            };
            if let Some((v, e)) = metric {
                out.push(Value { inches: v, from: i, to: e, raw: raw(i, e) });
                i = e;
                continue;
            }
        }
        if let Some((v, e)) = inches_at(&cs, i, false) {
            out.push(Value { inches: v, from: i, to: e, raw: raw(i, e) });
            i = e;
            continue;
        }
        i += 1;
    }
    if out.is_empty() && metric_bare {
        let t = text.trim();
        if t.len() >= 2
            && t.chars().all(|c| c.is_ascii_digit())
            && let Ok(v) = t.parse::<f64>()
        {
            out.push(Value { inches: v / 25.4, from: 0, to: t.chars().count(), raw: t.to_string() });
        }
    }
    out.retain(|v| v.inches.is_finite() && v.inches > 0.0);
    out
}

/// An MTEXT's words without its formatting codes.
fn plain(t: &str) -> String {
    let mut out = String::new();
    let mut cs = t.chars().peekable();
    while let Some(c) = cs.next() {
        match c {
            '{' | '}' => {}
            '\\' => match cs.next() {
                Some('P' | 'p' | 'N' | '~') => out.push(' '),
                Some('\\') => out.push('\\'),
                Some('{') => out.push('{'),
                Some('}') => out.push('}'),
                Some(_) => {
                    for d in cs.by_ref() {
                        if d == ';' {
                            break;
                        }
                    }
                }
                None => {}
            },
            _ => out.push(c),
        }
    }
    out
}

// ─── What a sheet draws ────────────────────────────────────────────────────

/// A text: its words, where its middle is, its direction and height.
#[derive(Clone, Debug)]
pub(crate) struct TextItem {
    pub text: String,
    pub start: Vec2,
    pub dir: Vec2,
    pub h: f64,
    pub width: f64,
}

/// The line work, marks and texts of one sheet (sheet coordinates).
#[derive(Clone, Debug, Default)]
pub(crate) struct Items {
    pub segs: Vec<(Vec2, Vec2)>,
    /// Dots: centre and radius.
    pub dots: Vec<(Vec2, f64)>,
    /// Filled arrowheads: their tip and the middle of their back.
    pub arrows: Vec<(Vec2, Vec2)>,
    pub texts: Vec<TextItem>,
    /// Block references (their insertion point, size 0): with the round outlines bigger than a
    /// mark, what a witness on a device stands on. (Polygons are not: a plan's hatching, frames
    /// and fixtures are full of them.)
    pub shapes: Vec<(Vec2, f64)>,
}

impl Items {
    fn full(&self) -> bool {
        self.segs.len() + self.dots.len() + self.arrows.len() + self.texts.len() + self.shapes.len() >= MAX_ITEMS
    }

    /// Open polyline points: runs of collinear pieces are one segment (an extension line drawn
    /// through the dimension line, a tick drawn as two halves).
    pub fn add_path(&mut self, pts: &[Vec2]) {
        let mut start = match pts.first() {
            Some(p) => *p,
            None => return,
        };
        let mut prev = start;
        for q in pts.iter().skip(1) {
            if q.dist(prev) <= 1e-9 {
                continue;
            }
            let d0 = prev - start;
            let d1 = *q - prev;
            let collinear = d0.len() > 1e-9 && (d0.normalized().cross(d1.normalized())).abs() < 1e-3 && d0.dot(d1) > 0.0;
            if !collinear && prev.dist(start) > 1e-9 {
                self.segs.push((start, prev));
                start = prev;
            }
            prev = *q;
        }
        if prev.dist(start) > 1e-9 {
            self.segs.push((start, prev));
        }
    }

    /// A closed outline: a dot when round and small, a filled arrowhead when a small triangle,
    /// else its sides.
    pub fn add_closed(&mut self, pts: &[Vec2], filled: bool) {
        let pts: Vec<Vec2> = pts.iter().copied().filter(|p| p.is_finite()).collect();
        if pts.len() < 3 {
            self.add_path(&pts);
            return;
        }
        let c = pts.iter().fold(Vec2::ZERO, |a, p| a + *p) / pts.len() as f64;
        let rs: Vec<f64> = pts.iter().map(|p| p.dist(c)).collect();
        let (rmin, rmax) = rs.iter().fold((f64::INFINITY, 0.0f64), |(a, b), r| (a.min(*r), b.max(*r)));
        if pts.len() >= 8 && rmax > 0.0 && rmin >= 0.8 * rmax {
            self.dots.push((c, rmax));
            return;
        }
        let corners = corners(&pts);
        if filled && corners.len() == 3 {
            // The tip: the sharpest corner.
            let sharp = (0..3)
                .filter_map(|i| {
                    let (p, a, b) = (corners.get(i)?, corners.get((i + 2) % 3)?, corners.get((i + 1) % 3)?);
                    Some((i, (*a - *p).normalized().dot((*b - *p).normalized())))
                })
                .max_by(|x, y| x.1.total_cmp(&y.1));
            if let Some((i, _)) = sharp
                && let (Some(t), Some(a), Some(b)) = (corners.get(i), corners.get((i + 1) % 3), corners.get((i + 2) % 3))
            {
                self.arrows.push((*t, a.mid(*b)));
                return;
            }
        }
        let mut ring = pts.clone();
        if let Some(f) = pts.first() {
            ring.push(*f);
        }
        self.add_path(&ring);
    }

    fn add_text(&mut self, text: &str, insert: Vec2, align: Option<Vec2>, halign: HAlign, h: f64, rotation: f64, width_factor: f64) {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > MAX_VALUE_TEXT || !insert.is_finite() || !h.is_finite() || h <= 0.0 {
            return;
        }
        let dir = Vec2::from_angle(if rotation.is_finite() { rotation } else { 0.0 });
        let width = text_width(text, h) * if width_factor.is_finite() && width_factor > 0.0 { width_factor } else { 1.0 };
        let start = match (halign, align) {
            (HAlign::Center | HAlign::Middle, Some(a)) => a - dir * (width / 2.0),
            (HAlign::Right, Some(a)) => a - dir * width,
            _ => insert,
        };
        self.texts.push(TextItem { text: text.to_string(), start, dir, h, width });
    }
}

/// A text's width as plotted (an average sans letter: 0.6 of its height; narrow marks less).
fn text_width(t: &str, h: f64) -> f64 {
    t.chars().map(|c| if matches!(c, '\'' | '"' | ' ' | '-' | '/' | '.' | '1') { 0.35 } else { 0.6 }).sum::<f64>() * h
}

/// The corners of an outline (its points where it turns).
fn corners(pts: &[Vec2]) -> Vec<Vec2> {
    let n = pts.len();
    (0..n)
        .filter_map(|i| {
            let (a, p, b) = (pts.get((i + n - 1) % n)?, pts.get(i)?, pts.get((i + 1) % n)?);
            let (u, v) = ((*p - *a).normalized(), (*b - *p).normalized());
            (u.cross(v).abs() > 0.05 || u.dot(v) < 0.0).then_some(*p)
        })
        .collect()
}

/// A drawing's line work and texts in each of `rects` (an object goes to the first rect its
/// first point is in). Built objects (meshes) and block references are not a sheet's drawing.
pub(crate) fn items_by_rect(d: &Drawing, rects: &[[f64; 4]]) -> Vec<Items> {
    let mut out: Vec<Items> = rects.iter().map(|_| Items::default()).collect();
    let inside = |r: &[f64; 4], p: Vec2| p.x >= r[0].min(r[2]) && p.x <= r[0].max(r[2]) && p.y >= r[1].min(r[3]) && p.y <= r[1].max(r[3]);
    for e in d.model.iter().take(MAX_MODEL) {
        let first = match &e.kind {
            EntityKind::Line(l) => l.a.xy(),
            EntityKind::LwPolyline(p) => match p.vertices.first() {
                Some(v) => v.p,
                None => continue,
            },
            EntityKind::Circle(c) => c.center.xy(),
            EntityKind::Text(t) => t.insert.xy(),
            EntityKind::MText(t) => t.insert.xy(),
            EntityKind::Solid(s) => match s.corners.first() {
                Some(c) => c.xy(),
                None => continue,
            },
            EntityKind::Hatch(h) if h.solid => match h.loops.first().and_then(|l| l.vertices.first()) {
                Some(v) => v.p,
                None => continue,
            },
            EntityKind::Insert(i) => i.insert.xy(),
            _ => continue,
        };
        let Some(k) = rects.iter().position(|r| inside(r, first)) else { continue };
        let Some(it) = out.get_mut(k) else { continue };
        if it.full() {
            continue;
        }
        match &e.kind {
            EntityKind::Line(l) => it.add_path(&[l.a.xy(), l.b.xy()]),
            EntityKind::LwPolyline(p) => {
                let pts: Vec<Vec2> = p.vertices.iter().map(|v| v.p).collect();
                if p.closed {
                    it.add_closed(&pts, false);
                } else {
                    it.add_path(&pts);
                }
            }
            EntityKind::Circle(c) if c.radius.is_finite() && c.radius > 0.0 => it.dots.push((c.center.xy(), c.radius)),
            EntityKind::Text(t) => it.add_text(&t.value, t.insert.xy(), t.align_pt.map(|a| a.xy()), t.halign, t.height, t.rotation, t.width_factor),
            EntityKind::MText(t) => {
                // Attachment 2/5/8: centred on the insertion point.
                let ha = if matches!(t.attach, 2 | 5 | 8) { HAlign::Center } else { HAlign::Left };
                it.add_text(&plain(&t.contents), t.insert.xy(), Some(t.insert.xy()), ha, t.height, t.rotation, 1.0);
            }
            EntityKind::Solid(s) => {
                let mut pts: Vec<Vec2> = Vec::new();
                for c in &s.corners {
                    let p = c.xy();
                    if !pts.iter().any(|q| q.dist(p) <= 1e-9) {
                        pts.push(p);
                    }
                }
                it.add_closed(&pts, true);
            }
            EntityKind::Hatch(h) => {
                for l in h.loops.iter().take(4) {
                    let pts: Vec<Vec2> = l.vertices.iter().map(|v| v.p).collect();
                    it.add_closed(&pts, true);
                }
            }
            EntityKind::Insert(i) if i.insert.xy().is_finite() => it.shapes.push((i.insert.xy(), 0.0)),
            _ => {}
        }
    }
    out
}

// ─── Reading ───────────────────────────────────────────────────────────────

/// How a witness position is marked on its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    Dot,
    Tick,
    Arrow,
    /// Only an extension line crossing it.
    Extension,
}

impl Mark {
    pub fn name(self) -> &'static str {
        match self {
            Mark::Dot => "dot",
            Mark::Tick => "tick",
            Mark::Arrow => "arrow",
            Mark::Extension => "extension line",
        }
    }
}

/// One witness of a dimension: where it is on the dimension line, its mark, and the far end of
/// its extension line (the point on the object measured; the mark itself with none).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Witness {
    pub on_line: Vec2,
    pub mark: Mark,
    pub point: Vec2,
    /// The way its extension line runs from the dimension line to the object (unit; zero: none).
    pub ext: Vec2,
    /// The symbol drawn at its point, if any (a device's; a wall's witness has none).
    pub symbol: Option<Vec2>,
}

/// A dimension read off a sheet (sheet coordinates).
#[derive(Clone, Debug)]
pub(crate) struct DimString {
    pub ends: (Witness, Witness),
    /// Along the dimension line (unit), from the first end to the second.
    pub dir: Vec2,
    pub inches: f64,
    pub text: String,
    pub text_at: Vec2,
    pub h: f64,
    /// The line it is on (one id per dimension line: a chain's stretches share it) and its
    /// place along it.
    pub chain: usize,
    pub index: usize,
    /// Dimensions sharing a first witness line: one id per group.
    pub baseline: Option<usize>,
    /// Its value fits its drawn length at the sheet's scale.
    pub to_scale: bool,
}

/// What one sheet's reading found.
#[derive(Clone, Debug, Default)]
pub(crate) struct SheetRead {
    pub strings: Vec<DimString>,
    pub values: usize,
    /// Values read but used by no dimension, by why.
    pub unused: BTreeMap<&'static str, usize>,
    /// Stretches between witness marks with no value.
    pub no_value: usize,
    /// Inches per sheet unit (the median value per drawn length).
    pub scale: Option<f64>,
}

/// A value placed on the sheet: the length, its text's middle, direction and height.
#[derive(Clone, Debug)]
struct Token {
    inches: f64,
    raw: String,
    at: Vec2,
    dir: Vec2,
    h: f64,
}

/// A uniform grid over segment and point indices.
struct Grid {
    cell: f64,
    cells: HashMap<(i64, i64), Vec<usize>>,
    long: Vec<usize>,
}

impl Grid {
    fn key(&self, p: Vec2) -> (i64, i64) {
        ((p.x / self.cell).floor() as i64, (p.y / self.cell).floor() as i64)
    }
    fn segs(segs: &[(Vec2, Vec2)], cell: f64) -> Grid {
        let mut g = Grid { cell, cells: HashMap::new(), long: Vec::new() };
        for (i, (a, b)) in segs.iter().enumerate() {
            let (ka, kb) = (g.key(a.min(*b)), g.key(a.max(*b)));
            let n = (kb.0 - ka.0 + 1).saturating_mul(kb.1 - ka.1 + 1);
            if !(0..=MAX_CELLS as i64).contains(&n) {
                g.long.push(i);
                continue;
            }
            for x in ka.0..=kb.0 {
                for y in ka.1..=kb.1 {
                    g.cells.entry((x, y)).or_default().push(i);
                }
            }
        }
        g
    }
    fn points(pts: impl Iterator<Item = Vec2>, cell: f64) -> Grid {
        let mut g = Grid { cell, cells: HashMap::new(), long: Vec::new() };
        for (i, p) in pts.enumerate() {
            let k = g.key(p);
            g.cells.entry(k).or_default().push(i);
        }
        g
    }
    /// Indices filed in the cells a box `[lo, hi]` touches (each once).
    fn near(&self, lo: Vec2, hi: Vec2) -> Vec<usize> {
        let (ka, kb) = (self.key(lo), self.key(hi));
        let mut out: Vec<usize> = self.long.clone();
        if (kb.0 - ka.0 + 1).saturating_mul(kb.1 - ka.1 + 1) > 40_000 {
            return out;
        }
        for x in ka.0..=kb.0 {
            for y in ka.1..=kb.1 {
                if let Some(v) = self.cells.get(&(x, y)) {
                    out.extend_from_slice(v);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// A dimension line: a point on it, its direction (unit), its extent along it, the typical
/// value height on it.
#[derive(Clone, Debug)]
struct DimLine {
    o: Vec2,
    u: Vec2,
    t0: f64,
    t1: f64,
    h: f64,
}

impl DimLine {
    fn t(&self, p: Vec2) -> f64 {
        (p - self.o).dot(self.u)
    }
    fn off(&self, p: Vec2) -> f64 {
        (p - self.o).cross(self.u)
    }
    fn at(&self, t: f64) -> Vec2 {
        self.o + self.u * t
    }
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    v.retain(|x| x.is_finite());
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied()
}

/// The dimension strings a sheet's line work and texts draw (see the module docs).
/// `scale_hint`: inches per sheet unit when the sheet's registration knows it.
pub(crate) fn read(items: &Items, scale_hint: Option<f64>) -> SheetRead {
    let mut out = SheetRead::default();
    // The values, each at its place in its text.
    let has_imperial = items.texts.iter().any(|t| t.text.contains('\'') || t.text.contains('"'));
    let mut tokens: Vec<Token> = Vec::new();
    for t in &items.texts {
        let n = t.text.chars().count().max(1) as f64;
        for v in values_in(&t.text, !has_imperial) {
            if tokens.len() >= MAX_TOKENS {
                break;
            }
            let mid = (v.from + v.to) as f64 / 2.0 / n;
            tokens.push(Token { inches: v.inches, raw: v.raw, at: t.start + t.dir * (t.width * mid), dir: t.dir, h: t.h });
        }
    }
    out.values = tokens.len();
    if tokens.is_empty() {
        return out;
    }
    let h_med = median(tokens.iter().map(|t| t.h).collect()).unwrap_or(1.0).max(1e-6);
    let sg = Grid::segs(&items.segs, h_med * 8.0);
    // Each value's dimension line: the nearest parallel segment beside it, run out along its
    // collinear pieces.
    let mut lines: Vec<DimLine> = Vec::new();
    let mut tok_line: Vec<Option<usize>> = vec![None; tokens.len()];
    for (ti, tk) in tokens.iter().enumerate() {
        let r = tk.h * (BESIDE_H + 6.0);
        let mut best: Option<(f64, usize)> = None;
        for si in sg.near(tk.at - Vec2::new(r, r), tk.at + Vec2::new(r, r)) {
            let Some((a, b)) = items.segs.get(si) else { continue };
            let len = a.dist(*b);
            if len < tk.h * 0.5 {
                continue;
            }
            let u = (*b - *a) / len;
            if u.cross(tk.dir).abs() > PARALLEL_SIN {
                continue;
            }
            let off = (tk.at - *a).cross(u).abs();
            let t = (tk.at - *a).dot(u);
            if off > tk.h * BESIDE_H || t < -tk.h * PAST_END_H * 4.0 || t > len + tk.h * PAST_END_H * 4.0 {
                continue;
            }
            // Beside it, not through it: a value's middle is half a height off its line.
            let score = off + if t < 0.0 || t > len { tk.h } else { 0.0 };
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, si));
            }
        }
        let Some((_, si)) = best else {
            *out.unused.entry("no dimension line beside it").or_default() += 1;
            continue;
        };
        let Some((a, b)) = items.segs.get(si).copied() else { continue };
        let u0 = (b - a).normalized();
        // One direction for both senses: the text's reading direction.
        let u = if u0.dot(tk.dir) < 0.0 { u0 * -1.0 } else { u0 };
        // Is it a line found already?
        let found = lines.iter().position(|l| {
            l.u.cross(u).abs() < PARALLEL_SIN && l.off(a).abs() <= 0.2 * l.h.min(tk.h) && l.t(a) <= l.t1 + tk.h && l.t(b) >= l.t0 - tk.h
        });
        if let Some(li) = found {
            tok_line[ti] = Some(li);
            continue;
        }
        // Run it out along collinear pieces.
        let tb = (b - a).dot(u);
        let mut l = DimLine { o: a, u, t0: tb.min(0.0), t1: tb.max(0.0), h: tk.h };
        for _ in 0..16 {
            let (lo, hi) = (l.at(l.t0) - Vec2::new(tk.h, tk.h) * LINE_GAP_H, l.at(l.t1) + Vec2::new(tk.h, tk.h) * LINE_GAP_H);
            let (lo, hi) = (lo.min(hi), lo.max(hi));
            let mut grew = false;
            for sj in sg.near(lo, hi) {
                let Some((p, q)) = items.segs.get(sj) else { continue };
                if (*q - *p).normalized().cross(u).abs() > PARALLEL_SIN || l.off(*p).abs() > 0.2 * tk.h || l.off(*q).abs() > 0.2 * tk.h {
                    continue;
                }
                let (tp, tq) = (l.t(*p).min(l.t(*q)), l.t(*p).max(l.t(*q)));
                if tq < l.t0 - tk.h * LINE_GAP_H || tp > l.t1 + tk.h * LINE_GAP_H {
                    continue;
                }
                if tp < l.t0 - 1e-9 || tq > l.t1 + 1e-9 {
                    l.t0 = l.t0.min(tp);
                    l.t1 = l.t1.max(tq);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        lines.push(l);
        tok_line[ti] = Some(lines.len() - 1);
    }
    // Marks.
    let dot_grid = Grid::points(items.dots.iter().map(|d| d.0), h_med * 8.0);
    let arrow_grid = Grid::points(items.arrows.iter().map(|a| a.0), h_med * 8.0);
    let shape_grid = Grid::points(items.shapes.iter().map(|a| a.0), h_med * 8.0);
    // Each line's witness positions.
    let mut witnesses: Vec<Vec<(f64, Mark)>> = Vec::new();
    for l in &lines {
        let h = l.h;
        let (p0, p1) = (l.at(l.t0), l.at(l.t1));
        let pad = Vec2::new(h * 3.0, h * 3.0);
        let (lo, hi) = (p0.min(p1) - pad, p0.max(p1) + pad);
        let in_extent = |t: f64| t >= l.t0 - h * PAST_END_H && t <= l.t1 + h * PAST_END_H;
        let mut ws: Vec<(f64, Mark)> = Vec::new();
        for di in dot_grid.near(lo, hi) {
            let Some((c, r)) = items.dots.get(di) else { continue };
            if *r <= h * DOT_MAX_H && l.off(*c).abs() <= h * ON_LINE_H && in_extent(l.t(*c)) {
                ws.push((l.t(*c), Mark::Dot));
            }
        }
        for ai in arrow_grid.near(lo, hi) {
            let Some((tip, back)) = items.arrows.get(ai) else { continue };
            let along = (*back - *tip).normalized();
            if tip.dist(*back) <= h * ARROW_MAX_H && l.off(*tip).abs() <= h * ON_LINE_H && along.cross(l.u).abs() < 0.3 && in_extent(l.t(*tip)) {
                ws.push((l.t(*tip), Mark::Arrow));
            }
        }
        let mut crossings: Vec<f64> = Vec::new();
        for si in sg.near(lo, hi) {
            let Some((a, b)) = items.segs.get(si) else { continue };
            let len = a.dist(*b);
            if len <= 1e-9 {
                continue;
            }
            let v = (*b - *a) / len;
            let sin = v.cross(l.u).abs();
            let m = a.mid(*b);
            // A tick: short, slanted across the line, its middle on it.
            if (TICK_MIN_H * h..=TICK_MAX_H * h).contains(&len)
                && (0.42..=0.92).contains(&sin)
                && l.off(m).abs() <= h * ON_LINE_H
                && in_extent(l.t(m))
            {
                ws.push((l.t(m), Mark::Tick));
                continue;
            }
            // An extension line: square to the line, crossing it (or ending on it).
            if sin > 1.0 - PARALLEL_SIN && len >= h * 0.5 {
                let (oa, ob) = (l.off(*a), l.off(*b));
                if oa.min(ob) <= h * ON_LINE_H && oa.max(ob) >= -h * ON_LINE_H && in_extent(l.t(m)) {
                    crossings.push(l.t(m));
                }
            }
        }
        ws.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, Mark)> = Vec::new();
        for (t, m) in ws {
            match merged.last_mut() {
                Some(last) if (t - last.0).abs() <= h * SAME_MARK_H => {
                    if last.1 == Mark::Dot && m != Mark::Dot {
                        *last = (t, m);
                    }
                }
                _ => merged.push((t, m)),
            }
        }
        // No marks: witness positions where extension lines cross it (a plain dimension).
        if merged.len() < 2 {
            crossings.sort_by(f64::total_cmp);
            let mut cs: Vec<(f64, Mark)> = Vec::new();
            for t in crossings {
                if cs.last().is_none_or(|l: &(f64, Mark)| (t - l.0).abs() > h * SAME_MARK_H) {
                    cs.push((t, Mark::Extension));
                }
            }
            if cs.len() >= 2 {
                merged = cs;
            }
        }
        witnesses.push(merged);
    }
    // The sheet's scale: the median value per drawn length of values inside one stretch.
    let mut ratios = Vec::new();
    for (ti, tk) in tokens.iter().enumerate() {
        let Some(li) = tok_line.get(ti).copied().flatten() else { continue };
        let (Some(l), Some(ws)) = (lines.get(li), witnesses.get(li)) else { continue };
        let t = l.t(tk.at);
        if let Some(w) = ws.windows(2).find(|w| w.first().is_some_and(|a| a.0 <= t) && w.get(1).is_some_and(|b| b.0 >= t)) {
            let span = w.get(1).map_or(0.0, |b| b.0) - w.first().map_or(0.0, |a| a.0);
            if span > 1e-9 {
                ratios.push(tk.inches / span);
            }
        }
    }
    let scale = median(ratios).or(scale_hint).filter(|k| *k > 0.0);
    out.scale = scale;
    // Values to stretches.
    let mut used = vec![false; tokens.len()];
    let mut strings: Vec<DimString> = Vec::new();
    for (li, l) in lines.iter().enumerate() {
        let Some(ws) = witnesses.get(li) else { continue };
        let mine: Vec<usize> = (0..tokens.len()).filter(|ti| tok_line.get(*ti).copied().flatten() == Some(li)).collect();
        if ws.len() < 2 {
            for _ in &mine {
                *out.unused.entry("its line has fewer than two witness marks").or_default() += 1;
            }
            continue;
        }
        // Stretches between consecutive witness positions (indices into `ws`).
        let stretches: Vec<(usize, usize)> =
            (1..ws.len()).filter(|&i| ws.get(i).zip(ws.get(i - 1)).is_some_and(|(b, a)| b.0 - a.0 > l.h * 0.2)).map(|i| (i - 1, i)).collect();
        let pos = |i: usize| ws.get(i).map_or(f64::NAN, |w| w.0);
        let mut taken: Vec<Option<(usize, bool)>> = vec![None; stretches.len()];
        let fits = |ti: usize, si: usize| -> bool {
            let (Some(tk), Some((a, b)), Some(k)) = (tokens.get(ti), stretches.get(si), scale) else { return false };
            let drawn = (pos(*b) - pos(*a)) * k;
            (tk.inches - drawn).abs() <= (FIT_SHARE * tk.inches).max(FIT_MIN_IN)
        };
        let dist_to = |ti: usize, si: usize| -> f64 {
            let (Some(tk), Some((a, b))) = (tokens.get(ti), stretches.get(si)) else { return f64::INFINITY };
            let (a, b) = (pos(*a), pos(*b));
            let t = l.t(tk.at);
            if t < a {
                a - t
            } else if t > b {
                t - b
            } else {
                0.0
            }
        };
        // Pass 1: values that fit a stretch, nearest first; pass 2: a value inside a stretch
        // left over (kept, not to scale).
        let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
        for &ti in &mine {
            for si in 0..stretches.len() {
                if fits(ti, si) {
                    pairs.push((dist_to(ti, si), ti, si));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (d, ti, si) in pairs {
            let Some(tk) = tokens.get(ti) else { continue };
            if d > tk.h * 30.0 || used.get(ti).copied().unwrap_or(true) || taken.get(si).copied().flatten().is_some() {
                continue;
            }
            if let (Some(u), Some(s)) = (used.get_mut(ti), taken.get_mut(si)) {
                *u = true;
                *s = Some((ti, true));
            }
        }
        for &ti in &mine {
            if used.get(ti).copied().unwrap_or(true) {
                continue;
            }
            let Some(si) = (0..stretches.len()).find(|si| dist_to(ti, *si) <= 0.0 && taken.get(*si).copied().flatten().is_none()) else { continue };
            if let (Some(u), Some(s)) = (used.get_mut(ti), taken.get_mut(si)) {
                *u = true;
                *s = Some((ti, false));
            }
        }
        for &ti in &mine {
            if !used.get(ti).copied().unwrap_or(true) {
                *out.unused.entry("no stretch of its line fits it").or_default() += 1;
            }
        }
        for (si, (ia, ib)) in stretches.iter().enumerate() {
            let (Some(wa), Some(wb)) = (ws.get(*ia), ws.get(*ib)) else { continue };
            let Some((ti, fit)) = taken.get(si).copied().flatten() else {
                out.no_value += 1;
                continue;
            };
            let Some(tk) = tokens.get(ti) else { continue };
            if strings.len() >= MAX_STRINGS {
                break;
            }
            let w = |t: f64, m: Mark| witness(items, &sg, &dot_grid, &shape_grid, l, t, m);
            strings.push(DimString {
                ends: (w(wa.0, wa.1), w(wb.0, wb.1)),
                dir: l.u,
                inches: tk.inches,
                text: tk.raw.clone(),
                text_at: tk.at,
                h: l.h,
                chain: li,
                index: si,
                baseline: None,
                to_scale: fit,
            });
        }
    }
    // The same dimension read twice (a text drawn twice over itself, a line found twice): once.
    let mut kept: Vec<DimString> = Vec::new();
    for d in strings {
        let same = |k: &DimString| {
            (k.inches - d.inches).abs() < 1e-6
                && k.ends.0.on_line.dist(d.ends.0.on_line) <= d.h * 0.2
                && k.ends.1.on_line.dist(d.ends.1.on_line) <= d.h * 0.2
        };
        if !kept.iter().any(same) {
            kept.push(d);
        }
    }
    let mut strings = kept;
    // Baselines: dimensions on different lines sharing a first witness line.
    let mut group = 0usize;
    for i in 0..strings.len() {
        let Some(si) = strings.get(i).cloned() else { continue };
        if si.baseline.is_some() {
            continue;
        }
        let mates: Vec<usize> = (0..strings.len())
            .filter(|&j| {
                strings.get(j).is_some_and(|sj| {
                    sj.chain != si.chain
                        && sj.dir.cross(si.dir).abs() < PARALLEL_SIN
                        && ((sj.ends.0.on_line - si.ends.0.on_line).dot(si.dir).abs() <= si.h * SAME_MARK_H
                            || (sj.ends.1.on_line - si.ends.1.on_line).dot(si.dir).abs() <= si.h * SAME_MARK_H)
                        && (sj.ends.0.on_line - si.ends.0.on_line).cross(si.dir).abs() <= si.h * 20.0
                })
            })
            .collect();
        if mates.is_empty() {
            continue;
        }
        for j in mates.into_iter().chain(std::iter::once(i)) {
            if let Some(s) = strings.get_mut(j) {
                s.baseline = Some(group);
            }
        }
        group += 1;
    }
    out.strings = strings;
    out
}

/// The symbol a witness at `p` measures to (a circle or round outline bigger than a mark, a
/// block reference): near `p` and centred on the witness line (through `p`, square to `u`, the
/// dimension's direction) — a dimension to a device runs to its centre. The nearest one's
/// middle.
fn symbol_at(items: &Items, dot_grid: &Grid, shape_grid: &Grid, p: Vec2, u: Vec2, h: f64) -> Option<Vec2> {
    let r = h * SYMBOL_MAX_H;
    let (lo, hi) = (p - Vec2::new(r, r), p + Vec2::new(r, r));
    let near = |c: Vec2, size: f64| c.dist(p) <= size + h * SYMBOL_NEAR_H && (c - p).dot(u).abs() <= h * SYMBOL_ON_LINE_H + size * 0.15;
    let dots =
        dot_grid.near(lo, hi).into_iter().filter_map(|i| items.dots.get(i)).filter(|(c, rad)| *rad > h * DOT_MAX_H && *rad <= r && near(*c, *rad));
    let shapes = shape_grid
        .near(lo, hi)
        .into_iter()
        .filter_map(|i| items.shapes.get(i))
        .filter(|(c, size)| *size <= r && (*size == 0.0 || *size >= h * 0.3) && near(*c, *size));
    dots.chain(shapes).map(|(c, _)| *c).min_by(|a, b| a.dist(p).total_cmp(&b.dist(p)))
}

/// The witness at `t` on `l`: its extension line run out (through dashes) to its far end.
fn witness(items: &Items, sg: &Grid, dot_grid: &Grid, shape_grid: &Grid, l: &DimLine, t: f64, mark: Mark) -> Witness {
    let w = l.at(t);
    let h = l.h;
    let n = l.u.perp();
    // The pieces on the square line through `w`, as stretches along `n` from `w`.
    let reach = h * EXT_MAX_H;
    let (lo, hi) = ((w - n * reach).min(w + n * reach) - Vec2::new(h, h), (w - n * reach).max(w + n * reach) + Vec2::new(h, h));
    let mut runs: Vec<(f64, f64)> = Vec::new();
    for si in sg.near(lo, hi) {
        let Some((a, b)) = items.segs.get(si) else { continue };
        let len = a.dist(*b);
        if len <= 1e-9 || (*b - *a).normalized().cross(n).abs() > PARALLEL_SIN {
            continue;
        }
        if (*a - w).dot(l.u).abs() > h * ON_LINE_H || (*b - w).dot(l.u).abs() > h * ON_LINE_H {
            continue;
        }
        let (sa, sb) = ((*a - w).dot(n), (*b - w).dot(n));
        runs.push((sa.min(sb), sa.max(sb)));
    }
    runs.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Grow from the line both ways through gaps no wider than a dash's.
    let gap = h * EXT_GAP_H;
    // It starts at the line, or a dash's gap from it.
    let start = runs.iter().filter(|r| r.0 <= gap && r.1 >= -gap).fold(None::<(f64, f64)>, |acc, r| match acc {
        Some((a, b)) => Some((a.min(r.0), b.max(r.1))),
        None => Some(*r),
    });
    let symbol = |p: Vec2| symbol_at(items, dot_grid, shape_grid, p, l.u, h);
    let Some((mut lo_s, mut hi_s)) = start else { return Witness { on_line: w, mark, point: w, ext: Vec2::ZERO, symbol: symbol(w) } };
    loop {
        let mut grew = false;
        for r in &runs {
            if r.1 >= lo_s - gap && r.0 < lo_s - 1e-9 && r.0 >= -reach {
                lo_s = r.0;
                grew = true;
            }
            if r.0 <= hi_s + gap && r.1 > hi_s + 1e-9 && r.1 <= reach {
                hi_s = r.1;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    // The object's side: where it runs farther.
    let (far, dir) = if -lo_s >= hi_s { (lo_s, n * -1.0) } else { (hi_s, n) };
    if far.abs() <= h * 1.5 {
        return Witness { on_line: w, mark, point: w, ext: Vec2::ZERO, symbol: symbol(w) };
    }
    let point = w + n * far;
    Witness { on_line: w, mark, point, ext: dir, symbol: symbol(point) }
}

// ─── Mapping onto the model ────────────────────────────────────────────────

/// A dimension placed in the model: its witness points and directions in model coordinates,
/// where it was read, and on which level.
#[derive(Clone, Debug)]
pub(crate) struct Placed {
    pub sheet: String,
    pub view: Option<String>,
    pub level: String,
    pub s: DimString,
    /// The witness points, the dimension's direction and each extension's (model).
    pub a: Vec2,
    pub b: Vec2,
    pub dir: Vec2,
    /// The symbols drawn at the witness points, if any (model).
    pub symbols: (Option<Vec2>, Option<Vec2>),
    /// Model units per sheet unit, and the registration's error (model units, when known).
    pub scale: f64,
    pub residual: f64,
    pub method: String,
}

/// One sheet's reading, for the report.
#[derive(Clone, Debug, Default)]
pub(crate) struct SheetReport {
    pub values: usize,
    pub read: usize,
    pub placed: usize,
    pub unplaced: BTreeMap<String, usize>,
    pub unused: BTreeMap<&'static str, usize>,
    pub no_value: usize,
    pub chains: usize,
    pub baselines: usize,
    pub not_to_scale: usize,
    pub scale_in_per_unit: Option<f64>,
    pub registration: Option<String>,
}

/// Everything read off the drawing's sheets.
#[derive(Clone, Debug, Default)]
pub(crate) struct Drawn {
    pub placed: Vec<Placed>,
    pub by_sheet: BTreeMap<String, SheetReport>,
}

fn norm(r: [f64; 4]) -> [f64; 4] {
    [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]
}

fn in_rect(r: &[f64; 4], p: Vec2) -> bool {
    p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3]
}

/// A registration with `p ↦ p + shift` first.
fn shifted(r: &SheetRegistration, shift: Vec2) -> SheetRegistration {
    let mut out = r.clone();
    let moved = r.to_project([shift.x, shift.y]);
    let zero = r.to_project([0.0, 0.0]);
    out.tx += moved[0] - zero[0];
    out.ty += moved[1] - zero[1];
    out
}

/// A segment with its ends in a fixed order (by x, then y).
fn ordered((a, b): (Vec2, Vec2)) -> (Vec2, Vec2) {
    if (a.x, a.y) <= (b.x, b.y) { (a, b) } else { (b, a) }
}

fn long_lines(it: &Items, min_len: f64, n: usize) -> Vec<(Vec2, Vec2)> {
    let mut v: Vec<(Vec2, Vec2)> = it.segs.iter().filter(|(a, b)| a.dist(*b) >= min_len).copied().map(ordered).collect();
    v.sort_by(|x, y| y.0.dist(y.1).total_cmp(&x.0.dist(x.1)));
    v.truncate(n);
    v
}

/// Where `own`'s plan lies on `other`'s: the move most of its long lines take onto lines of the
/// same length and direction there, the share of them that land on one, and how many.
fn plan_move(own: &Items, other: &Items, width: f64) -> Option<(Vec2, f64, usize)> {
    let min_len = width * FRAME_LINE_SHARE;
    let tol = (width * 0.001).max(1e-9);
    let voters = long_lines(own, min_len, FRAME_VOTERS);
    let theirs = long_lines(other, min_len * 0.9, FRAME_LINES);
    if voters.len() < FRAME_MATCH_MIN || theirs.is_empty() {
        return None;
    }
    // Theirs by length (to `tol`) and direction (to a degree).
    let key = |(a, b): (Vec2, Vec2)| -> (i64, i64) {
        let ang = (b - a).angle().to_degrees().rem_euclid(180.0);
        ((a.dist(b) / tol).round() as i64, ang.round() as i64 % 180)
    };
    let mut by: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (j, s) in theirs.iter().enumerate() {
        by.entry(key(*s)).or_default().push(j);
    }
    let mut votes: HashMap<(i64, i64), usize> = HashMap::new();
    for s in &voters {
        let (l, a) = key(*s);
        for dl in -1..=1 {
            for da in [-1i64, 0, 1] {
                for j in by.get(&(l + dl, (a + da).rem_euclid(180))).into_iter().flatten() {
                    let Some(t) = theirs.get(*j) else { continue };
                    let d = t.0 - s.0;
                    *votes.entry(((d.x / (tol * 4.0)).round() as i64, (d.y / (tol * 4.0)).round() as i64)).or_default() += 1;
                }
            }
        }
    }
    let (best, _) = votes.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))?;
    let rough = Vec2::new(best.0 as f64 * tol * 4.0, best.1 as f64 * tol * 4.0);
    // The move to the matched lines' mean, then the share that land.
    let g = Grid::segs(&theirs, (width * 0.02).max(1e-6));
    let near = |s: &(Vec2, Vec2), shift: Vec2, tol: f64| -> Option<Vec2> {
        let (a, b) = (s.0 + shift, s.1 + shift);
        g.near(a.min(b) - Vec2::new(tol, tol), a.max(b) + Vec2::new(tol, tol))
            .into_iter()
            .filter_map(|j| theirs.get(j))
            .find(|t| t.0.dist(a) <= tol && t.1.dist(b) <= tol)
            .map(|t| t.0 - s.0)
    };
    let found: Vec<Vec2> = voters.iter().filter_map(|s| near(s, rough, tol * 4.0)).collect();
    if found.is_empty() {
        return None;
    }
    let shift = found.iter().fold(Vec2::ZERO, |a, d| a + *d) / found.len() as f64;
    let hit = voters.iter().filter(|s| near(s, shift, tol * 2.0).is_some()).count();
    Some((shift, hit as f64 / voters.len() as f64, hit))
}

/// Each level's stacking move (what `model.build` moved its built objects by).
fn stacking(d: &Drawing) -> HashMap<String, Vec2> {
    let mut out: HashMap<String, Vec2> = HashMap::new();
    for e in d.model.iter().take(MAX_MODEL) {
        if super::wallgen::made_as(e).is_none() {
            continue;
        }
        let Some(level) = super::bspec::made_level(e) else { continue };
        out.entry(level.to_ascii_lowercase()).or_insert_with(|| super::bspec::stacked_by(e));
    }
    out
}

/// Read every sheet's drawn dimensions and place them on the model (see the module docs).
pub(crate) fn read_drawing(d: &Drawing, sp: &BuildingSpec) -> Drawn {
    let mut out = Drawn::default();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    // The sheets: each number's own link (with its rectangle) and its views.
    let mut sheets: Vec<(&SheetLink, [f64; 4])> = Vec::new();
    for l in sp.sheets.iter().filter(|l| l.view.is_none()) {
        let Some(r) = l.rect.filter(|r| r.iter().all(|v| v.is_finite())) else { continue };
        if !sheets.iter().any(|(s, _)| s.number.eq_ignore_ascii_case(&l.number)) && sheets.len() < MAX_SHEETS {
            sheets.push((l, norm(r)));
        }
    }
    // A sheet known only by its views: their sheet rectangle.
    for l in sp.sheets.iter().filter(|l| l.view.is_some()) {
        let Some(r) = l.rect.filter(|r| r.iter().all(|v| v.is_finite())) else { continue };
        if !sheets.iter().any(|(s, _)| s.number.eq_ignore_ascii_case(&l.number)) && sheets.len() < MAX_SHEETS {
            sheets.push((l, norm(r)));
        }
    }
    let rects: Vec<[f64; 4]> = sheets.iter().map(|(_, r)| *r).collect();
    let items = items_by_rect(d, &rects);
    // Each level's reference plan, and the level a reference belongs to.
    let reference_level = |reference: &str| -> Option<String> {
        sp.sheets
            .iter()
            .find(|l| l.number.eq_ignore_ascii_case(reference) && l.registration.as_ref().is_some_and(|r| r.method == "reference"))
            .and_then(|l| l.level.clone())
            .or_else(|| sp.sheets.iter().find(|l| l.number.eq_ignore_ascii_case(reference)).and_then(|l| l.level.clone()))
    };
    let stack = stacking(d);
    // A registration for each sheet: its own, else one borrowed from a same-size sheet whose
    // lines fall on its own.
    let own_reg = |num: &str| -> Option<SheetRegistration> {
        sp.sheets.iter().filter(|l| l.number.eq_ignore_ascii_case(num) && l.view.is_none()).find_map(|l| l.registration.clone())
    };
    let any_view_reg = |num: &str| sp.sheets.iter().any(|l| l.number.eq_ignore_ascii_case(num) && l.view.is_some() && l.registration.is_some());
    let mut sheet_reg: Vec<Option<(SheetRegistration, String)>> = Vec::new();
    for (k, (link, r)) in sheets.iter().enumerate() {
        if let Some(reg) = own_reg(&link.number) {
            let m = reg.method.clone();
            sheet_reg.push(Some((reg, m)));
            continue;
        }
        if any_view_reg(&link.number) {
            sheet_reg.push(None);
            continue;
        }
        // Borrow: a registered sheet drawing the same plan at the same scale.
        let w = r[2] - r[0];
        let mut best: Option<(f64, usize, usize, Vec2)> = None;
        for (j, (other, _)) in sheets.iter().enumerate() {
            if j == k || own_reg(&other.number).is_none() {
                continue;
            }
            let (Some(mine), Some(theirs)) = (items.get(k), items.get(j)) else { continue };
            let Some((shift, share, n)) = plan_move(mine, theirs, w) else { continue };
            if share >= FRAME_MATCH_SHARE && n >= FRAME_MATCH_MIN && best.is_none_or(|(s, _, _, _)| share > s) {
                best = Some((share, n, j, shift));
            }
        }
        let borrowed = best.and_then(|(share, n, j, shift)| {
            let (other, _) = sheets.get(j)?;
            let reg = own_reg(&other.number)?;
            let mut b = shifted(&reg, shift);
            b.method = "lines".into();
            b.matched = n;
            Some((
                b,
                format!(
                    "lines: its plan is {}'s ({n} of its longest lines, {}%, on {}'s once moved), through {}'s registration",
                    other.number,
                    (share * 100.0).round(),
                    other.number,
                    other.number
                ),
            ))
        });
        sheet_reg.push(borrowed);
    }
    for (k, (link, _)) in sheets.iter().enumerate() {
        let Some(it) = items.get(k) else { continue };
        let reg = sheet_reg.get(k).cloned().flatten();
        let hint = reg.as_ref().map(|(r, _)| r.scale() * 12.0 / upf);
        let read = read(it, hint);
        let rep = out.by_sheet.entry(link.number.clone()).or_default();
        rep.values = read.values;
        rep.read = read.strings.len();
        rep.unused = read.unused.clone();
        rep.no_value = read.no_value;
        rep.scale_in_per_unit = read.scale.map(|v| (v * 1e4).round() / 1e4);
        rep.chains = {
            let mut per: BTreeMap<usize, usize> = BTreeMap::new();
            for s in &read.strings {
                *per.entry(s.chain).or_default() += 1;
            }
            per.values().filter(|n| **n >= 2).count()
        };
        rep.baselines = read.strings.iter().filter_map(|s| s.baseline).collect::<std::collections::BTreeSet<_>>().len();
        rep.not_to_scale = read.strings.iter().filter(|s| !s.to_scale).count();
        rep.registration = reg.as_ref().map(|(r, why)| if r.method == "lines" { why.clone() } else { format!("{} onto {}", r.method, r.reference) });
        for s in read.strings {
            let mid = s.ends.0.on_line.mid(s.ends.1.on_line);
            // The view it is in, else the sheet.
            let view = sp
                .sheets
                .iter()
                .filter(|l| l.number.eq_ignore_ascii_case(&link.number))
                .find(|l| l.view.as_ref().is_some_and(|v| in_rect(&norm(v.rect), mid)));
            let (registration, level, view_title) = match view {
                Some(v) => (v.registration.clone().map(|r| (r.clone(), r.method)), v.level.clone(), v.view.as_ref().map(|x| x.title.clone())),
                None => (reg.clone(), link.level.clone(), None),
            };
            let Some((r, method)) = registration else {
                let why = if view_title.is_some() {
                    "its view is not registered on a level's plan"
                } else {
                    "its sheet is not registered on a level's plan"
                };
                *rep.unplaced.entry(why.into()).or_default() += 1;
                continue;
            };
            // The level: its registration's reference plan's.
            let Some(ref_level) = reference_level(&r.reference) else {
                *rep.unplaced.entry(format!("its registration's reference plan {} has no level", r.reference)).or_default() += 1;
                continue;
            };
            if let Some(lv) = level.as_deref()
                && view_title.is_some()
                && !lv.eq_ignore_ascii_case(&ref_level)
            {
                *rep.unplaced.entry("its view's registration is another level's plan".into()).or_default() += 1;
                continue;
            }
            let shift = stack.get(&ref_level.to_ascii_lowercase()).copied().unwrap_or(Vec2::ZERO);
            let to_model = |p: Vec2| -> Vec2 {
                let q = r.to_project([p.x, p.y]);
                Vec2::new(q[0], q[1]) + shift
            };
            let (a, b) = (to_model(s.ends.0.point), to_model(s.ends.1.point));
            let along = to_model(s.ends.0.on_line + s.dir) - to_model(s.ends.0.on_line);
            if !a.is_finite() || !b.is_finite() || along.len() <= 1e-12 {
                *rep.unplaced.entry("its registration is degenerate".into()).or_default() += 1;
                continue;
            }
            // It must land on its level's plan.
            if let Some(reference_rect) =
                sp.sheets.iter().find(|l| l.number.eq_ignore_ascii_case(&r.reference) && l.view.is_none()).and_then(|l| l.rect).map(norm)
            {
                let back = |p: Vec2| p - shift;
                if !in_rect(&reference_rect, back(a)) || !in_rect(&reference_rect, back(b)) {
                    *rep.unplaced.entry("it lands outside its level's plan".into()).or_default() += 1;
                    continue;
                }
            }
            rep.placed += 1;
            out.placed.push(Placed {
                sheet: link.number.clone(),
                view: view_title,
                level: ref_level,
                dir: along.normalized(),
                a,
                b,
                symbols: (s.ends.0.symbol.map(to_model), s.ends.1.symbol.map(to_model)),
                scale: r.scale(),
                residual: r.residual.filter(|v| v.is_finite()).unwrap_or(0.0),
                method,
                s,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ins(v: &[Value]) -> Vec<f64> {
        v.iter().map(|x| (x.inches * 1000.0).round() / 1000.0).collect()
    }

    #[test]
    fn lengths_in_every_format() {
        assert_eq!(ins(&values_in("8' - 3\"", false)), vec![99.0]);
        assert_eq!(ins(&values_in("8'-3 1/2\"", false)), vec![99.5]);
        assert_eq!(ins(&values_in("12'", false)), vec![144.0]);
        assert_eq!(ins(&values_in("27\"", false)), vec![27.0]);
        assert_eq!(ins(&values_in("8 1/2\"", false)), vec![8.5]);
        assert_eq!(ins(&values_in("1/2\"", false)), vec![0.5]);
        assert_eq!(ins(&values_in("1200 mm", false)), vec![47.244]);
        assert_eq!(ins(&values_in("1.2 m", false)), vec![47.244]);
        assert_eq!(ins(&values_in("120cm", false)), vec![47.244]);
        assert_eq!(ins(&values_in("1200", true)), vec![47.244]);
        assert!(values_in("1200", false).is_empty());
        // Two runs merged into one text: both, in order, with their places.
        let two = values_in("1' - 9 1/4\" 2' - 3\"", false);
        assert_eq!(ins(&two), vec![21.25, 27.0]);
        assert!(two.first().is_some_and(|v| v.from == 0) && two.get(1).is_some_and(|v| v.from > 10), "{two:?}");
        // Not lengths: a note, a door number, a level.
        assert!(values_in("REC 18", false).is_empty());
        assert!(values_in("E9-1013", false).is_empty());
    }

    #[test]
    fn mtext_codes_are_dropped() {
        assert_eq!(plain("{\\H0.8;8'-3\"}"), "8'-3\"");
        assert_eq!(plain("A\\PB"), "A B");
    }

    /// A chain of three on a sheet at a quarter scale (1 unit = 4 in): ticks, a dot, extension
    /// lines dashed through a wall, one text holding two values.
    fn chain() -> Items {
        let mut it = Items::default();
        let y = 50.0;
        // The dimension line, past its ends.
        it.add_path(&[Vec2::new(-1.0, y), Vec2::new(31.0, y)]);
        for (x, tick) in [(0.0, true), (6.0, false), (15.0, true), (30.0, true)] {
            if tick {
                it.add_path(&[Vec2::new(x - 0.5, y - 0.5), Vec2::new(x, y), Vec2::new(x + 0.5, y + 0.5)]);
            } else {
                let ring: Vec<Vec2> =
                    (0..16).map(|i| Vec2::new(x, y) + Vec2::from_angle(f64::from(i) * std::f64::consts::TAU / 16.0) * 0.25).collect();
                it.add_closed(&ring, true);
            }
            // Extension line from above the line down toward the object at y = 20, dashed.
            it.add_path(&[Vec2::new(x, y + 1.0), Vec2::new(x, 44.0)]);
            it.add_path(&[Vec2::new(x, 43.0), Vec2::new(x, 30.0)]);
            it.add_path(&[Vec2::new(x, 29.0), Vec2::new(x, 21.0)]);
        }
        let h = 1.5;
        it.add_text("2'-0\" 3'-0\"", Vec2::new(1.0, y + 0.4), None, HAlign::Left, h, 0.0, 1.0);
        it.add_text("5'-0\"", Vec2::new(20.5, y + 0.4), None, HAlign::Left, h, 0.0, 1.0);
        it
    }

    #[test]
    fn a_chain_of_three_is_read_with_its_witness_points() {
        let r = read(&chain(), None);
        assert_eq!(r.values, 3);
        let s = &r.strings;
        assert_eq!(s.len(), 3, "{r:?}");
        let r6 = |v: f64| (v * 1e6).round() / 1e6;
        let got: Vec<(f64, f64, f64)> = s.iter().map(|d| (r6(d.ends.0.on_line.x), r6(d.ends.1.on_line.x), d.inches)).collect();
        assert_eq!(got, vec![(0.0, 6.0, 24.0), (6.0, 15.0, 36.0), (15.0, 30.0, 60.0)]);
        assert!(s.iter().all(|d| d.chain == s[0].chain && d.to_scale));
        // Witness points at the extension lines' far ends, through the dashes.
        assert!(s.iter().all(|d| (d.ends.0.point.y - 21.0).abs() < 1e-9 && (d.ends.1.point.y - 21.0).abs() < 1e-9), "{s:?}");
        assert_eq!((s[0].ends.0.mark, s[0].ends.1.mark), (Mark::Tick, Mark::Dot));
        assert_eq!(r.scale.map(|k| (k * 100.0).round() / 100.0), Some(4.0));
    }

    #[test]
    fn a_value_off_its_drawn_length_is_kept_not_to_scale_and_a_bare_line_reads_nothing() {
        let mut it = chain();
        // A second line: a 10' value over a 12-unit stretch (48 in at this scale) between ticks.
        let y = 80.0;
        it.add_path(&[Vec2::new(0.0, y), Vec2::new(12.0, y)]);
        for x in [0.0, 12.0] {
            it.add_path(&[Vec2::new(x - 0.5, y - 0.5), Vec2::new(x + 0.5, y + 0.5)]);
        }
        it.add_text("10'-0\"", Vec2::new(4.0, y + 0.4), None, HAlign::Left, 1.5, 0.0, 1.0);
        // A note beside a line with no marks.
        it.add_path(&[Vec2::new(0.0, 100.0), Vec2::new(20.0, 100.0)]);
        it.add_text("3'-0\"", Vec2::new(5.0, 100.4), None, HAlign::Left, 1.5, 0.0, 1.0);
        let r = read(&it, None);
        let off: Vec<&DimString> = r.strings.iter().filter(|d| !d.to_scale).collect();
        assert_eq!(off.len(), 1, "{r:?}");
        assert_eq!(off[0].inches, 120.0);
        assert_eq!(r.unused.get("its line has fewer than two witness marks"), Some(&1));
    }

    #[test]
    fn turned_text_and_arrowheads() {
        // A vertical dimension, text turned 90°, filled arrowheads at both ends.
        let mut it = Items::default();
        it.add_path(&[Vec2::new(10.0, 0.0), Vec2::new(10.0, 24.0)]);
        it.add_closed(&[Vec2::new(10.0, 0.0), Vec2::new(9.7, 1.0), Vec2::new(10.3, 1.0)], true);
        it.add_closed(&[Vec2::new(10.0, 24.0), Vec2::new(9.7, 23.0), Vec2::new(10.3, 23.0)], true);
        it.add_text("2'-0\"", Vec2::new(9.6, 10.0), None, HAlign::Left, 1.0, std::f64::consts::FRAC_PI_2, 1.0);
        let r = read(&it, Some(1.0));
        assert_eq!(r.strings.len(), 1, "{r:?}");
        let d = &r.strings[0];
        assert_eq!((d.ends.0.mark, d.inches), (Mark::Arrow, 24.0));
        assert!(d.dir.y > 0.99);
    }

    #[test]
    fn baselines_share_a_first_witness() {
        let mut it = Items::default();
        for (y, x1, v) in [(10.0, 6.0, "6\""), (14.0, 12.0, "12\"")] {
            it.add_path(&[Vec2::new(0.0, y), Vec2::new(x1, y)]);
            for x in [0.0, x1] {
                it.add_path(&[Vec2::new(x - 0.4, y - 0.4), Vec2::new(x + 0.4, y + 0.4)]);
            }
            it.add_text(v, Vec2::new(x1 / 2.0 - 0.5, y + 0.3), None, HAlign::Left, 1.0, 0.0, 1.0);
        }
        let r = read(&it, Some(1.0));
        assert_eq!(r.strings.len(), 2, "{r:?}");
        assert!(r.strings.iter().all(|s| s.baseline == Some(0)));
    }
}
