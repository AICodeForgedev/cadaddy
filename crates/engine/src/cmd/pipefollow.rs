//! Following a tagged pipe past its tag (Q6181): a plan tags a pipe once and draws it on, round
//! its bends, through its dashes and past the next tag's reach. The pipe reader keeps only the
//! linework a tag reaches; this follows the rest of the drawn pipe from it.
//!
//! Untagged linework continues a tagged run when it is drawn with the same pen (layer, line
//! weight, line pattern) and either starts where the run ends (joined end to end, at any
//! angle: a bend) or carries straight on in line with it across a gap no wider than the rules
//! allow (a dashed line's dashes, a break at a fitting or a crossing). It takes the run's system
//! and size, sourced "followed from <tag>". A pipe drawn double (two lines its size apart) is
//! followed along both lines and counted once.
//!
//! Following stops at a fixture (a closed outline of a fixture's size, a small unlettered
//! circle), at a different pen, at linework a tag of another size or system reaches, and at a
//! gap over the limit. It never goes into walls (two parallel lines of the pen farther apart
//! than the pipe is drawn wide, at a wall's thickness), grids (a grid or centre line type, or a
//! line ending in a lettered bubble), dimension lines (a feet-and-inches figure along them),
//! leaders (short lines meeting the pipe at an angle from a note, nothing drawn on from their
//! note end) or fixture outlines (closed shapes).
//!
//! The limits are rules data (`pipefollow.json`), with the drafting practice they come from.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use cadaddy_doc::{Entity, EntityKind, Handle, Lineweight};
use cadaddy_geom::Vec2;
use serde::Deserialize;

const RULES: &str = include_str!("pipefollow.json");

/// Cells one segment is indexed in, at most (a very long line is looked at in its first cells).
const MAX_CELLS_PER_SEGMENT: usize = 2_000;
/// Segments looked at for walls, at most.
const MAX_SEGMENTS: usize = 400_000;
/// Text boxes looked at, at most.
const MAX_TEXTS: usize = 20_000;

/// What the following goes by: distances, the pen rules and the caps.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub(crate) struct Rules {
    pub source: String,
    /// Two ends this near (inches) are joined: a bend or a fitting drawn end to end.
    pub join_in: f64,
    /// The widest gap (feet) a line carries straight on across (its dashes, a break).
    pub gap_ft: f64,
    /// How far (degrees) a piece across a gap may turn from the run's direction.
    pub angle_deg: f64,
    /// How far (inches) a piece across a gap may sit off the run's line.
    pub lateral_in: f64,
    /// A piece joined at an angle is at least this long (feet): shorter ones are a symbol's.
    pub min_joined_piece_ft: f64,
    /// An end this near (inches) a fixture stops there.
    pub fixture_reach_in: f64,
    /// A closed shape this small across (feet) is a fixture (a larger one is a room or a box) ...
    pub fixture_max_ft: f64,
    /// ... when it is at least this wide both ways (inches): smaller is a fitting's or a hub's
    /// mark drawn on the pipe.
    pub fixture_min_in: f64,
    /// A circle this small (diameter, feet) with no lettering is a fixture (a drain, a cleanout).
    pub circle_fixture_max_ft: f64,
    /// Lettering taller than this (feet, on the plan) is no note: a title, or a stray mark
    /// the import read as text.
    pub note_max_height_ft: f64,
    /// A line starting this near a note (its text heights) is a leader ...
    pub leader_text_heights: f64,
    /// ... when it is no longer than this many text heights.
    pub leader_max_heights: f64,
    /// A feet-and-inches figure this near a line (its text heights) makes it a dimension line.
    pub dimension_text_heights: f64,
    /// A parallel line this far off (inches) ...
    pub wall_min_in: f64,
    pub wall_max_in: f64,
    /// ... along at least this share of a line's length makes it one face of a wall ...
    pub wall_overlap: f64,
    /// ... when it is at least this long (feet): a shorter pair is a fitting drawn double.
    pub wall_min_ft: f64,
    /// A twin at the run's own double-line spacing (within this, inches) is the pipe's.
    pub pipe_twin_match_in: f64,
    /// Lettering in a grid bubble: at most this many characters.
    pub bubble_text_max_chars: usize,
    /// Layers (words in their names) whose linework is never a pipe's.
    pub never_layers: Vec<String>,
    /// Line types (words in their names) of grid and centre lines.
    pub grid_linetypes: Vec<String>,
    /// Pieces followed per sheet, at most.
    pub max_followed: usize,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            source: String::new(),
            join_in: 3.0,
            gap_ft: 4.0,
            angle_deg: 5.0,
            lateral_in: 3.0,
            min_joined_piece_ft: 1.0,
            fixture_reach_in: 6.0,
            fixture_max_ft: 10.0,
            fixture_min_in: 9.0,
            circle_fixture_max_ft: 2.0,
            note_max_height_ft: 2.0,
            leader_text_heights: 1.0,
            leader_max_heights: 12.0,
            dimension_text_heights: 1.0,
            wall_min_in: 2.0,
            wall_max_in: 14.0,
            wall_overlap: 0.5,
            wall_min_ft: 3.0,
            pipe_twin_match_in: 1.0,
            bubble_text_max_chars: 3,
            never_layers: Vec::new(),
            grid_linetypes: Vec::new(),
            max_followed: 20_000,
        }
    }
}

/// The rules (`pipefollow.json`).
pub(crate) fn rules() -> &'static Rules {
    static R: std::sync::OnceLock<Rules> = std::sync::OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// The pen a line is drawn with: its layer, line weight and line pattern.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pen {
    pub layer: String,
    pub weight: Lineweight,
    pub pattern: String,
}

impl Pen {
    pub(crate) fn of(e: &Entity) -> Pen {
        Pen {
            layer: e.common.layer.to_string().to_ascii_uppercase(),
            weight: e.common.lineweight,
            pattern: e.common.linetype.to_string().to_ascii_uppercase(),
        }
    }
}

/// A run the reader tagged that following may start from: its chain, and its system and size
/// as its tag gives them (`SAN 6`).
pub(crate) struct Seed {
    pub chain: usize,
    pub kind: String,
    /// Its size, inches (0: the tag gave none).
    pub size_in: f64,
}

/// One drawn chain of the sheet: its points and its pen (its first object's).
pub(crate) struct Line<'a> {
    pub pts: &'a [Vec2],
    pub pen: Option<Pen>,
    /// It is a run a tag gave: never followed into.
    pub run: bool,
    /// The system and size of each tag that reaches it: one of another kind stops the
    /// following there.
    pub near_tags: Vec<String>,
}

/// Whether two tags name the same pipe: the same system, and the same size where both give one
/// (`SAN 6`, `SAN`).
pub(crate) fn same_kind(a: &str, b: &str) -> bool {
    let (mut x, mut y) = (a.split_whitespace(), b.split_whitespace());
    match ((x.next(), x.next()), (y.next(), y.next())) {
        ((Some(sa), za), (Some(sb), zb)) => sa.eq_ignore_ascii_case(sb) && (za.is_none() || zb.is_none() || za == zb),
        _ => false,
    }
}

/// A note's box on the sheet: left, bottom, right, top; and its text height.
struct TextBox {
    rect: [f64; 4],
    h: f64,
    text: String,
}

fn text_boxes(notes: &[(Handle, String, Vec2, f64)]) -> Vec<TextBox> {
    notes
        .iter()
        .take(MAX_TEXTS)
        .filter(|(_, t, at, h)| at.is_finite() && h.is_finite() && *h > 0.0 && !t.trim().is_empty())
        .map(|(_, t, at, h)| {
            let w = 0.6 * h * t.trim().chars().count() as f64;
            TextBox { rect: [at.x, at.y, at.x + w, at.y + h], h: *h, text: t.trim().to_string() }
        })
        .collect()
}

fn rect_dist(p: Vec2, r: &[f64; 4]) -> f64 {
    let dx = (r[0] - p.x).max(0.0).max(p.x - r[2]);
    let dy = (r[1] - p.y).max(0.0).max(p.y - r[3]);
    dx.hypot(dy)
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let d = b - a;
    let l2 = d.dot(d);
    let t = if l2 > 1e-18 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
    p.dist(a + d * t)
}

fn length(pts: &[Vec2]) -> f64 {
    pts.windows(2).map(|w| if let [a, b] = w { a.dist(*b) } else { 0.0 }).sum()
}

/// A figure in feet and inches (`12'-6"`, `3'`): a dimension's text.
fn is_dimension_text(t: &str) -> bool {
    let c: Vec<char> = t.chars().collect();
    c.len() <= 16 && c.windows(2).any(|w| matches!(w, [d, '\''] if d.is_ascii_digit()))
}

/// The ends of a chain: each end's point and the direction the line leaves through it.
fn ends(pts: &[Vec2]) -> Vec<(Vec2, Vec2)> {
    fn dir<'a>(end: Vec2, mut rest: impl Iterator<Item = &'a Vec2>) -> Option<Vec2> {
        rest.find(|q| q.dist(end) > 1e-9).map(|q| (end - *q) * (1.0 / (end - *q).len()))
    }
    let mut out = Vec::new();
    if let Some(&last) = pts.last()
        && let Some(u) = dir(last, pts.iter().rev().skip(1))
    {
        out.push((last, u));
    }
    if let Some(&first) = pts.first()
        && let Some(u) = dir(first, pts.iter().skip(1))
    {
        out.push((first, u));
    }
    out
}

/// A grid over the sheet: which items lie in each cell.
struct Grid {
    cell: f64,
    cells: BTreeMap<(i64, i64), Vec<usize>>,
}

impl Grid {
    fn new(cell: f64) -> Grid {
        Grid { cell: if cell.is_finite() && cell > 0.0 { cell } else { 1.0 }, cells: BTreeMap::new() }
    }
    fn key(&self, v: f64) -> i64 {
        let k = (v / self.cell).floor();
        if k.is_finite() { k.clamp(-1.0e12, 1.0e12) as i64 } else { 0 }
    }
    fn add_box(&mut self, lo: Vec2, hi: Vec2, item: usize) {
        let (x0, x1, y0, y1) = (self.key(lo.x), self.key(hi.x), self.key(lo.y), self.key(hi.y));
        let mut n = 0;
        for x in x0..=x1 {
            for y in y0..=y1 {
                self.cells.entry((x, y)).or_default().push(item);
                n += 1;
                if n >= MAX_CELLS_PER_SEGMENT {
                    return;
                }
            }
        }
    }
    fn near(&self, p: Vec2, r: f64) -> Vec<usize> {
        let (x0, x1, y0, y1) = (self.key(p.x - r), self.key(p.x + r), self.key(p.y - r), self.key(p.y + r));
        let mut out = Vec::new();
        for x in x0..=x1.min(x0.saturating_add(64)) {
            for y in y0..=y1.min(y0.saturating_add(64)) {
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

/// What the sheet holds that following must not pass or go into, and the lines themselves.
struct Sheet<'a> {
    rules: &'a Rules,
    upf: f64,
    lines: &'a [Line<'a>],
    texts: Vec<TextBox>,
    /// Fixtures: small closed shapes' boxes, small unlettered circles.
    fixture_boxes: Vec<[f64; 4]>,
    fixture_points: Vec<(Vec2, f64)>,
    /// Grid bubbles: lettered circles.
    bubbles: Vec<(Vec2, f64)>,
    /// Every straight segment: (line, a, b), indexed by where it lies.
    segs: Vec<(usize, Vec2, Vec2)>,
    seg_grid: Grid,
}

impl Sheet<'_> {
    fn ft(&self, ft: f64) -> f64 {
        ft * self.upf
    }
    fn inch(&self, inches: f64) -> f64 {
        inches / 12.0 * self.upf
    }

    fn closed(&self, pts: &[Vec2]) -> bool {
        pts.len() > 3 && matches!((pts.first(), pts.last()), (Some(a), Some(b)) if a.dist(*b) <= self.inch(self.rules.join_in))
    }

    /// Whether `p` is at a fixture (stops the following there).
    fn at_fixture(&self, p: Vec2) -> bool {
        let reach = self.inch(self.rules.fixture_reach_in);
        self.fixture_boxes.iter().any(|r| rect_dist(p, r) <= reach) || self.fixture_points.iter().any(|(c, r)| c.dist(p) <= r + reach)
    }

    /// The spacings (drawing units) of the parallel lines of the same pen along a line's
    /// segments, with the length each spacing covers.
    fn twins(&self, line: usize) -> Vec<(f64, f64)> {
        let Some(l) = self.lines.get(line) else { return Vec::new() };
        let (lo, hi) = (self.inch(self.rules.wall_min_in), self.inch(self.rules.wall_max_in));
        let cos_tol = self.rules.angle_deg.to_radians().cos();
        let mut out = Vec::new();
        for w in l.pts.windows(2) {
            let [a, b] = w else { continue };
            let len = a.dist(*b);
            if len <= 1e-9 {
                continue;
            }
            let u = (*b - *a) * (1.0 / len);
            let mid = (*a + *b) * 0.5;
            let mut best: Option<(f64, f64)> = None;
            for k in self.seg_grid.near(mid, len * 0.5 + hi) {
                let Some((other, c, d)) = self.segs.get(k) else { continue };
                if *other == line || self.lines.get(*other).and_then(|o| o.pen.as_ref()) != l.pen.as_ref() {
                    continue;
                }
                let olen = c.dist(*d);
                if olen <= 1e-9 || ((*d - *c) * (1.0 / olen)).dot(u).abs() < cos_tol {
                    continue;
                }
                let off = (*c - *a).cross(u).abs();
                if off < lo || off > hi {
                    continue;
                }
                // The overlap of the two along this segment.
                let (t0, t1) = ((*c - *a).dot(u), (*d - *a).dot(u));
                let overlap = (t0.max(t1).min(len) - t0.min(t1).max(0.0)).max(0.0);
                if overlap > best.map_or(0.0, |(_, o)| o) {
                    best = Some((off, overlap));
                }
            }
            if let Some(t) = best {
                out.push(t);
            }
        }
        out
    }

    /// Whether a line is a wall's face: a parallel line of its pen at a wall's thickness along
    /// most of it, farther off than the pipe is drawn wide (`pipe_width`: a pipe drawn double).
    fn is_wall(&self, line: usize, pipe_width: f64) -> bool {
        let Some(l) = self.lines.get(line) else { return false };
        let total = length(l.pts);
        if total <= 1e-9 || total < self.ft(self.rules.wall_min_ft) {
            return false;
        }
        let walled: f64 = self.twins(line).iter().filter(|(off, _)| *off > pipe_width).map(|(_, o)| o).sum();
        walled >= self.rules.wall_overlap * total
    }

    /// Whether a line runs alongside a line already followed, no farther off than the pipe is
    /// drawn wide, along most of it: the pipe's other line.
    fn twin_of_owned(&self, line: usize, pipe_width: f64, owner: &[Option<usize>]) -> bool {
        let Some(l) = self.lines.get(line) else { return false };
        let total = length(l.pts);
        if total <= 1e-9 {
            return false;
        }
        let cos_tol = self.rules.angle_deg.to_radians().cos();
        let mut along = 0.0;
        for w in l.pts.windows(2) {
            let [a, b] = w else { continue };
            let len = a.dist(*b);
            if len <= 1e-9 {
                continue;
            }
            let u = (*b - *a) * (1.0 / len);
            let mid = (*a + *b) * 0.5;
            let mut best: f64 = 0.0;
            for k in self.seg_grid.near(mid, len * 0.5 + pipe_width) {
                let Some((other, c, d)) = self.segs.get(k) else { continue };
                if *other == line || owner.get(*other).copied().flatten().is_none() {
                    continue;
                }
                let olen = c.dist(*d);
                if olen <= 1e-9 || ((*d - *c) * (1.0 / olen)).dot(u).abs() < cos_tol {
                    continue;
                }
                let off = (*c - *a).cross(u).abs();
                if off <= 1e-9 || off > pipe_width {
                    continue;
                }
                let (t0, t1) = ((*c - *a).dot(u), (*d - *a).dot(u));
                best = best.max((t0.max(t1).min(len) - t0.min(t1).max(0.0)).max(0.0));
            }
            along += best;
        }
        along >= 0.5 * total
    }

    /// Whether a line is a leader: short, with a free end (nothing drawn on from it) at a note.
    fn is_leader(&self, line: usize, end_grid: &Grid) -> bool {
        let Some(l) = self.lines.get(line) else { return false };
        let total = length(l.pts);
        let join = self.inch(self.rules.join_in);
        ends(l.pts).iter().any(|(end, _)| {
            let free = !end_grid
                .near(*end, join)
                .into_iter()
                .any(|k| k != line && self.lines.get(k).is_some_and(|o| ends(o.pts).iter().any(|(q, _)| q.dist(*end) <= join)));
            free && self.texts.iter().any(|t| {
                // A note: words or figures, not a stray mark the import read as text.
                t.text.chars().filter(char::is_ascii_alphanumeric).count() >= 3
                    && total <= self.rules.leader_max_heights * t.h
                    && rect_dist(*end, &t.rect) <= self.rules.leader_text_heights * t.h
            })
        })
    }

    /// Why a line can't be part of a pipe (none: it may be).
    fn never(&self, line: usize) -> Option<&'static str> {
        let l = self.lines.get(line)?;
        let pen = l.pen.as_ref()?;
        if self.rules.never_layers.iter().any(|w| !w.is_empty() && pen.layer.contains(&w.to_ascii_uppercase())) {
            return Some("an annotation or architectural layer");
        }
        if self.rules.grid_linetypes.iter().any(|w| !w.is_empty() && pen.pattern.contains(&w.to_ascii_uppercase())) {
            return Some("a grid line type");
        }
        if self.closed(l.pts) || super::mep::is_outline(l.pts, self.upf) {
            return Some("a fixture outline");
        }
        for (end, _) in ends(l.pts) {
            if self.bubbles.iter().any(|(c, r)| c.dist(end) <= r + self.inch(self.rules.join_in)) {
                return Some("a grid line");
            }
        }
        let dim = self.texts.iter().filter(|t| is_dimension_text(&t.text)).any(|t| {
            let reach = self.rules.dimension_text_heights * t.h;
            l.pts.windows(2).any(|w| match w {
                [a, b] => {
                    let mid = Vec2::new((t.rect[0] + t.rect[2]) * 0.5, (t.rect[1] + t.rect[3]) * 0.5);
                    seg_dist(mid, *a, *b) <= reach + t.h * 0.5
                }
                _ => false,
            })
        });
        dim.then_some("a dimension line")
    }
}

/// A line followed from a seed.
pub(crate) struct Followed {
    pub line: usize,
    /// The seed whose run it continues.
    pub seed: usize,
    /// The point it was followed from (the end of the line before it), when it carries straight
    /// on from there (in line, from its own end, or joined end to end): the run is unbroken
    /// across the dash's gap or the fitting. None: followed some other way (by a stretch in
    /// its middle), left as drawn.
    pub from: Option<Vec2>,
}

/// The lines followed from the seeds.
///
/// `ents` the sheet's objects (fixtures: circles and symbols), `lines` its chains, `notes` its
/// texts, `upf` drawing units per foot.
pub(crate) fn follow(ents: &[&Arc<Entity>], lines: &[Line], seeds: &[Seed], notes: &[(Handle, String, Vec2, f64)], upf: f64) -> Vec<Followed> {
    let rules = rules();
    if seeds.is_empty() || lines.is_empty() || !(upf.is_finite() && upf > 0.0) {
        return Vec::new();
    }
    let texts: Vec<TextBox> = text_boxes(notes).into_iter().filter(|t| t.h <= rules.note_max_height_ft * upf).collect();
    let mut sheet = Sheet {
        rules,
        upf,
        lines,
        texts,
        fixture_boxes: Vec::new(),
        fixture_points: Vec::new(),
        bubbles: Vec::new(),
        segs: Vec::new(),
        seg_grid: Grid::new(upf * 2.0),
    };
    for e in ents {
        match &e.kind {
            EntityKind::Circle(c) if c.center.is_finite() && c.radius.is_finite() && c.radius > 0.0 => {
                let at = c.center.xy();
                let lettered = sheet
                    .texts
                    .iter()
                    .any(|t| t.text.chars().count() <= rules.bubble_text_max_chars && rect_dist(at, &t.rect) <= c.radius && t.h <= 2.0 * c.radius);
                if lettered {
                    sheet.bubbles.push((at, c.radius));
                } else if 2.0 * c.radius <= sheet.ft(rules.circle_fixture_max_ft) {
                    sheet.fixture_points.push((at, c.radius));
                }
            }
            _ => {}
        }
        if sheet.fixture_points.len() + sheet.bubbles.len() >= MAX_TEXTS {
            break;
        }
    }
    for (k, l) in lines.iter().enumerate() {
        if sheet.closed(l.pts) {
            let (mut lo, mut hi) = (Vec2::new(f64::INFINITY, f64::INFINITY), Vec2::new(f64::NEG_INFINITY, f64::NEG_INFINITY));
            for p in l.pts {
                lo = Vec2::new(lo.x.min(p.x), lo.y.min(p.y));
                hi = Vec2::new(hi.x.max(p.x), hi.y.max(p.y));
            }
            let (small, large) = (sheet.ft(rules.fixture_max_ft), sheet.inch(rules.fixture_min_in));
            if (large..=small).contains(&(hi.x - lo.x)) && (large..=small).contains(&(hi.y - lo.y)) {
                sheet.fixture_boxes.push([lo.x, lo.y, hi.x, hi.y]);
            }
        }
        for w in l.pts.windows(2) {
            if let [a, b] = w
                && sheet.segs.len() < MAX_SEGMENTS
            {
                sheet.seg_grid.add_box(Vec2::new(a.x.min(b.x), a.y.min(b.y)), Vec2::new(a.x.max(b.x), a.y.max(b.y)), sheet.segs.len());
                sheet.segs.push((k, *a, *b));
            }
        }
    }
    // Where each line's ends are.
    let join = sheet.inch(rules.join_in);
    let gap = sheet.ft(rules.gap_ft).max(join);
    let mut end_grid = Grid::new(gap);
    for (k, l) in lines.iter().enumerate() {
        if l.pen.is_none() {
            continue;
        }
        for (p, _) in ends(l.pts) {
            end_grid.add_box(p, p, k);
        }
    }
    let lateral = sheet.inch(rules.lateral_in);
    let cos_tol = rules.angle_deg.to_radians().cos();
    let min_joined = sheet.ft(rules.min_joined_piece_ft);
    let tol = sheet.inch(rules.pipe_twin_match_in);
    let mut owner: Vec<Option<usize>> = vec![None; lines.len()];
    let mut never: Vec<Option<bool>> = vec![None; lines.len()];
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
    // How wide each seed's pipe is drawn (its tag's size): a parallel line of its pen no farther
    // off is the pipe's other line (a pipe drawn double, to scale), not a wall.
    let mut pipe_width: Vec<f64> = Vec::new();
    for (s, seed) in seeds.iter().enumerate() {
        if let Some(o) = owner.get_mut(seed.chain)
            && o.is_none()
        {
            *o = Some(s);
            queue.push_back((seed.chain, s));
        }
        pipe_width.push(sheet.inch(seed.size_in) + tol);
    }
    let mut out = Vec::new();
    while let Some((line, s)) = queue.pop_front() {
        let Some(l) = lines.get(line) else { continue };
        let Some(seed) = seeds.get(s) else { continue };
        let Some(seed_pen) = lines.get(seed.chain).and_then(|sl| sl.pen.clone()) else { continue };
        let width = pipe_width.get(s).copied().unwrap_or(tol);
        // Where the pipe may go on from this line: its two ends (joined at any angle, or in
        // line), and each straight stretch's ends (in line only: a pipe line glued to a
        // fitting's outline still carries on straight past the fitting).
        let mut fronts: Vec<(Vec2, Vec2, bool)> = ends(l.pts).into_iter().map(|(p, u)| (p, u, true)).collect();
        for w in l.pts.windows(2) {
            if let [a, b] = w
                && a.dist(*b) >= min_joined
            {
                let u = (*b - *a) * (1.0 / a.dist(*b));
                fronts.push((*b, u, false));
                fronts.push((*a, u * -1.0, false));
            }
        }
        for (end, u, chain_end) in fronts {
            // A fixture ends the pipe here.
            if sheet.at_fixture(end) {
                continue;
            }
            let mut found: Vec<(usize, bool, Option<Vec2>)> = Vec::new();
            // In line: a straight stretch of the candidate ahead of the end, on its line,
            // heading the same way, starting within the gap.
            for k in sheet.seg_grid.near(end, gap) {
                let Some((cl, c, d)) = sheet.segs.get(k) else { continue };
                let len = c.dist(*d);
                if *cl == line || len <= 1e-9 || ((*d - *c) * (1.0 / len)).dot(u).abs() < cos_tol {
                    continue;
                }
                let near = if c.dist(end) <= d.dist(end) { *c } else { *d };
                let w = near - end;
                if near.dist(end) <= gap && w.dot(u) >= -join && w.cross(u).abs() <= lateral {
                    // Its own end carries straight on from this one: a gap the pipe crosses.
                    let own_end = lines.get(*cl).is_some_and(|c| ends(c.pts).iter().any(|(q, _)| q.dist(near) <= 1e-6));
                    found.push((*cl, true, (own_end && chain_end).then_some(end)));
                }
            }
            // Joined end to end at an angle (a bend): only from the line's own ends.
            if chain_end {
                for k in end_grid.near(end, join) {
                    if k != line && lines.get(k).is_some_and(|c| length(c.pts) >= min_joined && ends(c.pts).iter().any(|(q, _)| q.dist(end) <= join))
                    {
                        found.push((k, false, Some(end)));
                    }
                }
            }
            found.sort_unstable_by_key(|(k, in_line, from)| (*k, !*in_line, from.is_none()));
            found.dedup_by_key(|(k, _, _)| *k);
            for (k, in_line, from) in found {
                if owner.get(k).is_none_or(|o| o.is_some()) {
                    continue;
                }
                let Some(cand) = lines.get(k) else { continue };
                // Another tag's run, or linework a tag of another size or system reaches.
                if cand.run || cand.near_tags.iter().any(|t| !same_kind(t, &seed.kind)) {
                    continue;
                }
                if cand.pen.as_ref() != Some(&seed_pen) {
                    continue;
                }
                // A leader meets a pipe at an angle, from its note.
                if !in_line && sheet.is_leader(k, &end_grid) {
                    continue;
                }
                let bad = match never.get(k).copied().flatten() {
                    Some(b) => b,
                    None => {
                        let b = sheet.never(k).is_some() || sheet.is_wall(k, width);
                        if let Some(slot) = never.get_mut(k) {
                            *slot = Some(b);
                        }
                        b
                    }
                };
                if bad {
                    continue;
                }
                if let Some(o) = owner.get_mut(k) {
                    *o = Some(s);
                }
                queue.push_back((k, s));
                // The pipe's other line (drawn double): the same pipe, counted once.
                if sheet.twin_of_owned(k, width, &owner) {
                    continue;
                }
                out.push(Followed { line: k, seed: s, from });
                if out.len() >= rules.max_followed {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_doc::{Circle, Common, Drawing, Space};
    use cadaddy_geom::Vec3;
    use cadaddy_walk3d::{BuildingSpec, Level, MepRun};

    /// A made-up plumbing plan, drawn in inches (12 units a foot), its linework on one generic
    /// layer as a PDF import gives it.
    struct Plan(Drawing);

    impl Plan {
        fn new() -> Plan {
            let mut p = Plan(Drawing::default());
            // A far corner: the sheet is a plan's size, not a sheet of paper's.
            p.line([0.0, 2000.0], [1.0, 2000.0], 9, "");
            p
        }
        fn line(&mut self, a: [f64; 2], b: [f64; 2], weight: u16, pattern: &str) -> &mut Plan {
            let mut c = Common { layer: "PDF-GEOMETRY".into(), lineweight: Lineweight::Mm100(weight), ..Common::default() };
            if !pattern.is_empty() {
                c.linetype = pattern.into();
            }
            let kind = EntityKind::Line(cadaddy_doc::Line { a: Vec3::new(a[0], a[1], 0.0), b: Vec3::new(b[0], b[1], 0.0) });
            assert!(self.0.add(&Space::Model, c, kind).is_ok());
            self
        }
        fn path(&mut self, pts: &[[f64; 2]], weight: u16) -> &mut Plan {
            for w in pts.windows(2) {
                self.line(w[0], w[1], weight, "");
            }
            self
        }
        fn text(&mut self, at: [f64; 2], t: &str, h: f64) -> &mut Plan {
            let s = crate::Session::empty();
            let kind =
                super::super::draw::make_text(&s, Vec2::new(at[0], at[1]), t, h, 0.0, (cadaddy_doc::HAlign::Left, cadaddy_doc::VAlign::Baseline));
            assert!(self.0.add(&Space::Model, Common { layer: "PDF-TEXT".into(), ..Common::default() }, kind).is_ok());
            self
        }
        fn circle(&mut self, at: [f64; 2], r: f64) -> &mut Plan {
            let kind = EntityKind::Circle(Circle { center: Vec3::new(at[0], at[1], 0.0), radius: r });
            assert!(self.0.add(&Space::Model, Common { layer: "PDF-GEOMETRY".into(), ..Common::default() }, kind).is_ok());
            self
        }
        /// The pipe runs the reader makes of it.
        fn read(&self) -> Vec<MepRun> {
            let mut t = crate::Session::empty();
            t.open_drawing(self.0.clone(), "P-101", None);
            let spec = BuildingSpec {
                units_per_foot: 12.0,
                levels: vec![Level {
                    name: "Level 1".into(),
                    elevation_ft: 0.0,
                    height_ft: 10.0,
                    ceiling_ft: 8.0,
                    region: None,
                    base: None,
                    attr: None,
                    sheet: None,
                }],
                ..BuildingSpec::default()
            };
            match super::super::mep::read(&t, &spec, None) {
                Ok((_, runs)) => runs,
                Err(e) => panic!("the pipe reader failed: {e}"),
            }
        }
    }

    fn followed(runs: &[MepRun]) -> Vec<&MepRun> {
        runs.iter().filter(|r| r.size_source.starts_with("followed from")).collect()
    }

    /// The followed runs' points: does any reach `x` (within an inch)?
    fn reaches(runs: &[&MepRun], x: f64, y: f64) -> bool {
        runs.iter().any(|r| r.points.iter().any(|p| (p[0] - x).abs() <= 1.0 && (p[1] - y).abs() <= 1.0))
    }

    /// A 4" sanitary drain, tagged once at its start.
    fn tagged_drain(p: &mut Plan) {
        p.line([0.0, 0.0], [240.0, 0.0], 18, "").text([100.0, 6.0], "4\" SAN", 6.0);
    }

    #[test]
    fn the_rules_load_with_their_source() {
        let r = rules();
        assert!(r.source.contains("drafting practice"));
        assert!(r.gap_ft > 0.0 && r.join_in > 0.0 && r.max_followed > 0);
        assert!(r.never_layers.iter().any(|l| l == "WALL"));
    }

    #[test]
    fn dimension_figures_are_feet_and_inches() {
        assert!(is_dimension_text("12'-6\""));
        assert!(is_dimension_text("3'"));
        assert!(!is_dimension_text("6\" S"));
        assert!(!is_dimension_text("24\" INVERT"));
    }

    #[test]
    fn tags_of_one_system_and_size_are_the_same_pipe() {
        assert!(same_kind("SAN 4", "SAN 4"));
        assert!(same_kind("SAN", "SAN 4"));
        assert!(!same_kind("SAN 3", "SAN 4"));
        assert!(!same_kind("ST 4", "SAN 4"));
    }

    /// MUST test 1: a tagged drain carries on untagged past a bend, and the bend's leg and the
    /// run beyond it take its system and size, sourced from its tag.
    #[test]
    fn a_tagged_drain_is_followed_past_a_bend() {
        let mut p = Plan::new();
        tagged_drain(&mut p);
        // Drawn on after the tag's reach: down round a bend (its ends a fraction apart, as an
        // import leaves them), then across again.
        p.line([240.5, -0.5], [240.5, -300.0], 18, "").line([241.0, -300.5], [600.0, -300.5], 18, "");
        let runs = p.read();
        let f = followed(&runs);
        assert_eq!(f.len(), 2, "both pieces past the tag are followed: {runs:?}");
        for r in &f {
            assert_eq!(r.system, "SAN");
            assert_eq!(r.size_in[0], 4.0);
            assert_eq!(r.size_source, "followed from 4\" SAN");
        }
        assert!(reaches(&f, 600.0, -300.5));
    }

    /// MUST test 2: an untagged line of another weight touching the drain is not taken, in line
    /// or at an angle; neither is one of another line pattern.
    #[test]
    fn a_line_of_another_weight_or_pattern_is_not_taken() {
        let mut p = Plan::new();
        tagged_drain(&mut p);
        p.line([241.0, 0.0], [600.0, 0.0], 35, "").line([240.5, 0.5], [240.5, 300.0], 35, "").line([240.5, -0.5], [240.5, -300.0], 18, "HIDDEN");
        let runs = p.read();
        assert!(followed(&runs).is_empty(), "{runs:?}");
        // The same lines in the drain's pen are its.
        let mut q = Plan::new();
        tagged_drain(&mut q);
        q.line([241.0, 0.0], [600.0, 0.0], 18, "").line([240.5, 0.5], [240.5, 300.0], 18, "");
        assert_eq!(followed(&q.read()).len(), 2);
    }

    /// MUST test 3: two tags of different sizes meet: each run keeps its own size, the linework
    /// between goes to one of them only, and following never crosses the other tag's run or
    /// takes linework a tag of another size reaches.
    #[test]
    fn two_tags_of_different_sizes_meet() {
        let mut p = Plan::new();
        tagged_drain(&mut p);
        // Untagged between the two, then a 3" run, then untagged beyond it.
        p.line([242.0, 0.0], [480.0, 0.0], 18, "").line([482.0, 0.0], [720.0, 0.0], 18, "").text([600.0, 6.0], "3\" SAN", 6.0);
        p.line([722.0, 0.0], [960.0, 0.0], 18, "");
        // A short piece a 2" tag reaches (too short to be a run of its own), then more beyond.
        p.line([1000.0, 0.0], [1018.0, 0.0], 18, "").text([1004.0, 4.0], "2\" SAN", 6.0);
        let runs = p.read();
        let tagged: Vec<&MepRun> = runs.iter().filter(|r| r.size_source == "tag").collect();
        assert!(tagged.iter().any(|r| r.size_in[0] == 4.0) && tagged.iter().any(|r| r.size_in[0] == 3.0), "{runs:?}");
        let f = followed(&runs);
        assert_eq!(f.len(), 2, "{f:?}");
        let between: Vec<&&MepRun> = f.iter().filter(|r| r.points.iter().any(|q| q[0] > 241.0 && q[0] < 481.0)).collect();
        assert_eq!(between.len(), 1, "the piece between is taken once");
        let beyond: Vec<&&MepRun> = f.iter().filter(|r| r.points.iter().any(|q| q[0] > 721.0 && q[0] < 961.0)).collect();
        assert_eq!(beyond.len(), 1);
        assert_eq!(beyond.first().map(|r| r.size_in[0]), Some(3.0), "beyond the 3\" run is 3\"");
        assert!(!reaches(&f, 1018.0, 0.0), "the 2\" tag's linework is not taken");
    }

    /// MUST test 4: a dashed pattern continues: its dashes are followed across their gaps, and a
    /// line drawn on a dashed line type is followed by a line of the same type.
    #[test]
    fn a_dashed_pattern_continues() {
        let mut p = Plan::new();
        p.line([0.0, 0.0], [120.0, 0.0], 18, "").text([40.0, 6.0], "6\" S", 6.0);
        let mut x = 150.0;
        while x < 900.0 {
            p.line([x, 0.0], [x + 60.0, 0.0], 18, "");
            x += 90.0;
        }
        let runs = p.read();
        let f = followed(&runs);
        assert_eq!(f.len(), 9, "every dash: {runs:?}");
        assert!(f.iter().all(|r| r.system == "SAN" && r.size_in[0] == 6.0));
        // Unbroken across the gaps: the dashes' runs carry on from the line before them, so the
        // drain is one length from the tag's line to the last dash (120" to 930").
        let ft: f64 = f.iter().map(|r| r.points.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum::<f64>()).sum();
        assert!((ft - 810.0).abs() < 1.0, "{ft}");
        // Past a gap over the rules' limit, it stops.
        let mut q = Plan::new();
        q.line([0.0, 0.0], [120.0, 0.0], 18, "").text([40.0, 6.0], "6\" S", 6.0).line(
            [120.0 + 12.0 * rules().gap_ft + 12.0, 0.0],
            [400.0, 0.0],
            18,
            "",
        );
        assert!(followed(&q.read()).is_empty());
        // A line type: dashed on dashed.
        let mut d = Plan::new();
        d.line([0.0, 0.0], [240.0, 0.0], 18, "DASHED").text([100.0, 6.0], "4\" SAN", 6.0).line([241.0, 0.0], [600.0, 0.0], 18, "DASHED");
        assert_eq!(followed(&d.read()).len(), 1);
    }

    #[test]
    fn a_pipe_drawn_double_is_followed_and_counted_once() {
        let mut p = Plan::new();
        // A 4" drain drawn as two lines 4" apart, tagged at its start.
        p.line([0.0, 0.0], [240.0, 0.0], 18, "").line([0.0, 4.0], [240.0, 4.0], 18, "").text([100.0, 10.0], "4\" SAN", 6.0);
        p.line([250.0, 0.0], [600.0, 0.0], 18, "").line([250.0, 4.0], [600.0, 4.0], 18, "");
        let runs = p.read();
        let f = followed(&runs);
        assert_eq!(f.len(), 1, "one of the two lines past the tag: {runs:?}");
    }

    #[test]
    fn it_never_follows_into_a_wall() {
        let mut p = Plan::new();
        tagged_drain(&mut p);
        // A wall's two faces, 8" apart, drawn in the drain's pen, from the drain's end.
        p.line([240.5, -0.5], [240.5, -400.0], 18, "").line([248.5, -0.5], [248.5, -400.0], 18, "");
        let runs = p.read();
        assert!(followed(&runs).is_empty(), "{runs:?}");
    }

    #[test]
    fn it_never_follows_into_a_grid_line() {
        // In line with the drain, ending in a lettered bubble.
        let mut p = Plan::new();
        tagged_drain(&mut p);
        p.line([260.0, 0.0], [800.0, 0.0], 18, "").circle([830.0, 0.0], 30.0).text([822.0, -6.0], "C", 12.0);
        assert!(followed(&p.read()).is_empty());
        // On a centre line type.
        let mut q = Plan::new();
        q.line([0.0, 0.0], [240.0, 0.0], 18, "CENTER").text([100.0, 6.0], "4\" SAN", 6.0).line([260.0, 0.0], [800.0, 0.0], 18, "CENTER");
        assert!(followed(&q.read()).is_empty());
    }

    #[test]
    fn it_never_follows_into_a_dimension_line() {
        let mut p = Plan::new();
        tagged_drain(&mut p);
        p.line([260.0, 0.0], [600.0, 0.0], 18, "").text([400.0, 3.0], "28'-4\"", 6.0);
        assert!(followed(&p.read()).is_empty());
    }

    #[test]
    fn it_never_follows_into_a_leader() {
        let mut p = Plan::new();
        tagged_drain(&mut p);
        // From the drain's end up to a note.
        p.line([240.5, 0.5], [300.0, 95.0], 18, "").text([300.0, 100.0], "SEE NOTE 3", 12.0);
        assert!(followed(&p.read()).is_empty());
    }

    #[test]
    fn it_never_follows_into_a_fixture_and_stops_there() {
        let mut p = Plan::new();
        tagged_drain(&mut p);
        // A floor drain's outline (2' square) at the drain's end, and a line of the drain's pen
        // carrying on beyond it.
        p.path(&[[242.0, -12.0], [266.0, -12.0], [266.0, 12.0], [242.0, 12.0], [242.0, -12.0]], 18);
        p.line([270.0, 0.0], [600.0, 0.0], 18, "");
        let runs = p.read();
        assert!(followed(&runs).is_empty(), "{runs:?}");
    }
}
