//! The ruled cells of a page: a schedule's grid drawn as lines (Q6516). A table's rows are ruled
//! by long lines of one extent, three or more (its top, the rule under its header, its bottom,
//! the rules between rows), and its columns by lines between them (a column drawn a piece per
//! row, its ends on the row rules); four rules close a cell round the lettering in it. A plan's
//! walls seldom line up so: rooms are no cells. A cell is a small closed
//! problem for the stroke OCR: everything in it is one entry (a breaker size, a dash for none,
//! a pole count of `1`), so its lone glyphs are read where loose linework would drop them, its
//! words never run on into the next column, and the rules themselves are never read as letters
//! (a column piece reads like an `I`).

use std::collections::{HashMap, HashSet};

/// How far (pt) a piece of a stroke may lean off the axis and still be a rule.
const AXIS_TOL: f64 = 0.25;
/// Collinear pieces this close (pt) are one line; a line's end this close to another line
/// touches it.
const JOIN_TOL: f64 = 0.75;
/// A line at least this long (pt, half an inch) is a rule on its own.
const LONG_PT: f64 = 36.0;
/// Pieces shorter than this (pt) are never rules (a letter's bar, a tick).
const MIN_PT: f64 = 4.0;
/// Most axis-aligned pieces looked at on a page (hostile files).
const MAX_PIECES: usize = 400_000;
/// Most lines one cell search looks at in each direction.
const MAX_SCAN: usize = 4_000;
/// A cell is at most this many of its lettering's heights tall (a wrapped entry of a few lines;
/// a room on a plan, also closed by four lines, is far taller than its tag).
const MAX_CELL_HEIGHTS: f64 = 6.0;
/// A cell is at most this many of its lettering's heights wide (a long description column).
const MAX_CELL_WIDTHS: f64 = 90.0;
/// A table's row rules start and end within this (pt) of each other.
const EXTENT_TOL: f64 = 2.0;
/// Row rules of one extent a table has at least (its top, the rule under its header, its bottom).
const MIN_TABLE_RULES: usize = 3;

/// One straight line of a page along an axis: its position across (`y` of a horizontal line),
/// its extent along, and whether it is one of the page's rules.
#[derive(Clone, Copy, Debug)]
struct Line {
    at: f64,
    lo: f64,
    hi: f64,
    rule: bool,
}

/// The page's rules, horizontal and vertical (each sorted by position), and the strokes that
/// are all rule (never lettering).
#[derive(Debug, Default)]
pub(crate) struct Cells {
    horizontal: Vec<Line>,
    vertical: Vec<Line>,
    /// Strokes drawn entirely along rules.
    pub(crate) rule_strokes: HashSet<usize>,
}

/// An axis-aligned piece of a stroke: (across, from, to, stroke).
type Piece = (f64, f64, f64, usize);

/// Collinear pieces joined into lines, each with the strokes it holds; sorted by position.
fn join(mut pieces: Vec<Piece>) -> Vec<(Line, Vec<usize>)> {
    pieces.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    // Group by position (within the tolerance of the group's first), then merge along.
    let mut out: Vec<(Line, Vec<usize>)> = Vec::new();
    let mut i = 0;
    while i < pieces.len() {
        let Some(&(at0, _, _, _)) = pieces.get(i) else { break };
        let mut j = i;
        while pieces.get(j).is_some_and(|p| p.0 - at0 <= AXIS_TOL * 2.0) {
            j += 1;
        }
        let mut group: Vec<Piece> = pieces.get(i..j).map(<[Piece]>::to_vec).unwrap_or_default();
        group.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut cur: Option<(Line, Vec<usize>)> = None;
        for (at, lo, hi, si) in group {
            match cur.as_mut() {
                Some((l, s)) if lo <= l.hi + JOIN_TOL => {
                    l.hi = l.hi.max(hi);
                    if s.last() != Some(&si) {
                        s.push(si);
                    }
                }
                _ => {
                    if let Some(done) = cur.take() {
                        out.push(done);
                    }
                    cur = Some((Line { at, lo, hi, rule: false }, vec![si]));
                }
            }
        }
        if let Some(done) = cur {
            out.push(done);
        }
        i = j.max(i + 1);
    }
    out.sort_by(|a, b| a.0.at.total_cmp(&b.0.at));
    out
}

/// A line's extent, in [`EXTENT_TOL`] steps: the table its row rules are.
fn extent_key(l: &Line) -> (i64, i64) {
    ((l.lo / EXTENT_TOL).round() as i64, (l.hi / EXTENT_TOL).round() as i64)
}

/// The lines of `across` (sorted by position) whose position lies within `lo..=hi`.
fn between(lines: &[Line], lo: f64, hi: f64) -> &[Line] {
    let a = lines.partition_point(|l| l.at < lo);
    let b = lines.partition_point(|l| l.at <= hi);
    lines.get(a..b.max(a)).unwrap_or_default()
}

/// Whether `l` touches (crosses or ends on) at least two long lines of `long` (the other
/// axis, sorted by position).
fn touches_two(l: &Line, long: &[Line]) -> bool {
    between(long, l.lo - JOIN_TOL, l.hi + JOIN_TOL)
        .iter()
        .take(MAX_SCAN)
        .filter(|p| l.at >= p.lo - JOIN_TOL && l.at <= p.hi + JOIN_TOL)
        .nth(1)
        .is_some()
}

impl Cells {
    /// The rules among `strokes` (each its points, page pt, and whether it is closed).
    pub(crate) fn of<'a>(strokes: impl Iterator<Item = (&'a [(f64, f64)], bool)>) -> Cells {
        let mut h: Vec<Piece> = Vec::new();
        let mut v: Vec<Piece> = Vec::new();
        // per stroke: whether every piece of it is axis-aligned (a candidate rule stroke)
        let mut all_axis: Vec<bool> = Vec::new();
        for (si, (pts, closed)) in strokes.enumerate() {
            let closing = if closed && pts.len() > 2 { pts.last().zip(pts.first()) } else { None };
            let mut axis = !pts.is_empty();
            for (&a, &b) in pts.iter().zip(pts.iter().skip(1)).chain(closing) {
                if h.len() + v.len() >= MAX_PIECES {
                    axis = false;
                    break;
                }
                let (dx, dy) = ((b.0 - a.0).abs(), (b.1 - a.1).abs());
                if !(dx.is_finite() && dy.is_finite()) {
                    axis = false;
                } else if dy <= AXIS_TOL && dx >= MIN_PT {
                    h.push(((a.1 + b.1) / 2.0, a.0.min(b.0), a.0.max(b.0), si));
                } else if dx <= AXIS_TOL && dy >= MIN_PT {
                    v.push(((a.0 + b.0) / 2.0, a.1.min(b.1), a.1.max(b.1), si));
                } else if dx > AXIS_TOL || dy > AXIS_TOL {
                    axis = false;
                }
            }
            all_axis.push(axis);
        }
        let (mut hs, mut vs) = (join(h), join(v));
        // Row rules: long horizontal lines, three or more of one extent.
        let mut by_extent: HashMap<(i64, i64), usize> = HashMap::new();
        let key = extent_key;
        for (l, _) in hs.iter().filter(|(l, _)| l.hi - l.lo >= LONG_PT) {
            *by_extent.entry(key(l)).or_default() += 1;
        }
        let family = |l: &Line| {
            let (a, b) = key(l);
            let mut n = 0usize;
            for da in -1..=1 {
                for db in -1..=1 {
                    n += by_extent.get(&(a + da, b + db)).copied().unwrap_or(0);
                }
            }
            n
        };
        for (l, _) in hs.iter_mut() {
            l.rule = l.hi - l.lo >= LONG_PT && family(l) >= MIN_TABLE_RULES;
        }
        // Column rules: vertical lines between two row rules (a column drawn a piece per row, a
        // cell's side).
        let rows: Vec<Line> = hs.iter().filter(|(l, _)| l.rule).map(|(l, _)| *l).collect();
        for (l, _) in vs.iter_mut() {
            l.rule = touches_two(l, &rows);
        }
        // A stroke all of whose pieces lie on rules is rule.
        let mut on_rule: HashSet<usize> = HashSet::new();
        let mut off_rule: HashSet<usize> = HashSet::new();
        for (l, strokes) in hs.iter().chain(vs.iter()) {
            for &si in strokes {
                if l.rule {
                    on_rule.insert(si)
                } else {
                    off_rule.insert(si)
                };
            }
        }
        let rule_strokes = on_rule.into_iter().filter(|si| !off_rule.contains(si) && all_axis.get(*si).copied().unwrap_or(false)).collect();
        let keep = |lines: Vec<(Line, Vec<usize>)>| -> Vec<Line> { lines.into_iter().filter(|(l, _)| l.rule).map(|(l, _)| l).collect() };
        Cells { horizontal: keep(hs), vertical: keep(vs), rule_strokes }
    }

    /// Whether the page has rules at all.
    pub(crate) fn is_empty(&self) -> bool {
        self.horizontal.is_empty() || self.vertical.is_empty()
    }

    /// The cell (`[x0, y0, x1, y1]`, pt) four rules close round a glyph box `bb` of lettering
    /// `h` tall: the nearest rule above, below, left and right that spans it, each reaching the
    /// others' corners; and its table (the extent of its row rules). `None`: the glyph is in no
    /// cell.
    pub(crate) fn cell_of(&self, bb: [f64; 4], h: f64) -> Option<([f64; 4], (i64, i64))> {
        if self.is_empty() || !(h.is_finite() && h > 0.0) {
            return None;
        }
        let (cx, cy) = ((bb[0] + bb[2]) / 2.0, (bb[1] + bb[3]) / 2.0);
        let max_h = MAX_CELL_HEIGHTS * h;
        let spans_x = |l: &&Line, x: f64| x >= l.lo - JOIN_TOL && x <= l.hi + JOIN_TOL;
        let top_rule = between(&self.horizontal, bb[3] - JOIN_TOL, cy + max_h).iter().take(MAX_SCAN).find(|l| spans_x(l, cx))?;
        let (top, table) = (top_rule.at, extent_key(top_rule));
        let bottom = between(&self.horizontal, cy - max_h, bb[1] + JOIN_TOL).iter().rev().take(MAX_SCAN).find(|l| spans_x(l, cx))?.at;
        if top - bottom > max_h || top - bottom < h * 1.05 || bb[3] > top + JOIN_TOL || bb[1] < bottom - JOIN_TOL {
            return None;
        }
        // the sides run from the bottom rule to the top one
        let side = |l: &&Line| l.lo <= bottom + JOIN_TOL * 2.0 && l.hi >= top - JOIN_TOL * 2.0;
        let max_w = MAX_CELL_WIDTHS * h;
        let left = between(&self.vertical, cx - max_w, bb[0] + JOIN_TOL).iter().rev().take(MAX_SCAN).find(side)?.at;
        let right = between(&self.vertical, bb[2] - JOIN_TOL, cx + max_w).iter().take(MAX_SCAN).find(side)?.at;
        // and the top and bottom rules reach both sides
        let reach = |y: f64| between(&self.horizontal, y, y).iter().any(|l| l.lo <= left + JOIN_TOL * 2.0 && l.hi >= right - JOIN_TOL * 2.0);
        (right - left >= h * 0.5 && reach(top) && reach(bottom)).then_some(([left, bottom, right, top], table))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(a: (f64, f64), b: (f64, f64)) -> Vec<(f64, f64)> {
        vec![a, b]
    }

    /// A table 3 columns × 2 rows, rows 18 pt, the columns drawn a piece per row.
    fn table() -> Vec<Vec<(f64, f64)>> {
        let mut s = Vec::new();
        for y in [100.0, 118.0, 136.0] {
            s.push(line((100.0, y), (400.0, y)));
        }
        for x in [100.0, 160.0, 300.0, 400.0] {
            s.push(line((x, 100.0), (x, 118.0)));
            s.push(line((x, 118.0), (x, 136.0)));
        }
        s
    }

    fn cells(strokes: &[Vec<(f64, f64)>]) -> Cells {
        Cells::of(strokes.iter().map(|s| (s.as_slice(), false)))
    }

    #[test]
    fn a_glyph_between_four_rules_is_in_their_cell() {
        let c = cells(&table());
        let cell = |bb, h| c.cell_of(bb, h).map(|(r, _)| r);
        assert_eq!(cell([170.0, 104.0, 176.0, 113.0], 9.0), Some([160.0, 100.0, 300.0, 118.0]));
        assert_eq!(cell([110.0, 122.0, 116.0, 131.0], 9.0), Some([100.0, 118.0, 160.0, 136.0]));
        // one table: its row rules' extent
        assert_eq!(c.cell_of([170.0, 104.0, 176.0, 113.0], 9.0).map(|(_, t)| t), c.cell_of([110.0, 122.0, 116.0, 131.0], 9.0).map(|(_, t)| t));
        // outside the table
        assert_eq!(c.cell_of([500.0, 104.0, 506.0, 113.0], 9.0), None);
        // every rule piece is rule, never lettering
        assert_eq!(c.rule_strokes.len(), table().len());
    }

    #[test]
    fn lettering_strokes_are_no_rules() {
        let mut s = table();
        // an `E` and an `I` (stem and serifs) in the first cell, 9 pt tall
        let first = s.len();
        s.push(line((170.0, 104.0), (170.0, 113.0)));
        s.push(line((170.0, 113.0), (175.0, 113.0)));
        s.push(line((170.0, 108.5), (174.0, 108.5)));
        s.push(line((170.0, 104.0), (175.0, 104.0)));
        s.push(vec![(180.0, 104.0), (180.0, 113.0)]);
        let c = cells(&s);
        assert!((first..s.len()).all(|i| !c.rule_strokes.contains(&i)), "{:?}", c.rule_strokes);
        assert_eq!(c.cell_of([170.0, 104.0, 175.0, 113.0], 9.0).map(|(r, _)| r), Some([160.0, 100.0, 300.0, 118.0]));
    }

    #[test]
    fn a_room_on_a_plan_is_no_cell_of_its_tag() {
        // four walls 4 in apart round a 9 pt tag
        let s = vec![
            line((0.0, 0.0), (288.0, 0.0)),
            line((0.0, 288.0), (288.0, 288.0)),
            line((0.0, 0.0), (0.0, 288.0)),
            line((288.0, 0.0), (288.0, 288.0)),
        ];
        assert_eq!(cells(&s).cell_of([140.0, 140.0, 160.0, 149.0], 9.0), None);
    }

    #[test]
    fn short_boxes_alone_make_no_rules() {
        // a tag's box: four short sides, no long line
        let s = vec![line((0.0, 0.0), (20.0, 0.0)), line((0.0, 12.0), (20.0, 12.0)), line((0.0, 0.0), (0.0, 12.0)), line((20.0, 0.0), (20.0, 12.0))];
        let c = cells(&s);
        assert!(c.rule_strokes.is_empty());
        assert_eq!(c.cell_of([4.0, 2.0, 10.0, 10.0], 8.0), None);
    }

    #[test]
    fn hostile_numbers_are_no_rules() {
        let s = vec![vec![(f64::NAN, 0.0), (10.0, f64::INFINITY)], vec![(0.0, 0.0)], vec![]];
        let c = cells(&s);
        assert!(c.is_empty());
        assert_eq!(c.cell_of([f64::NAN; 4], f64::NAN), None);
    }
}
