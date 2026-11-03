//! Pipe drawn as two lines. A plumbing plan printed from a model draws each pipe at its scale: its
//! two outside edges, with the fittings drawn across them where lengths join. Read line by line,
//! every pipe would be two runs (and each fitting a few more). Here each pair of edges becomes one
//! run along the pipe's middle, as wide as the pipe is drawn.
//!
//! A sheet is read this way only when it draws its pipes so: a good share of its parallel pairs
//! are closed across by a fitting's or cap's line ([`is_twin_drawn`]). Two single-line pipes run
//! side by side (cold and hot water a few inches apart) are a parallel pair too, but nothing is
//! drawn across them.

use cadaddy_geom::Vec2;
use std::collections::HashMap;

/// The narrowest pipe drawn as two lines (inches between the lines, at full size).
const MIN_WIDTH_IN: f64 = 0.4;
/// The widest (inches): an 8" pipe's outside, with room.
const MAX_WIDTH_IN: f64 = 9.0;
/// The shortest edge read as a pipe's (feet): shorter is a fitting's or a symbol's.
const MIN_EDGE_FT: f64 = 0.75;
/// How far off parallel two edges of one pipe may be (sine of the angle).
const PARALLEL_SIN: f64 = 0.035;
/// A second partner of an edge is the same pipe when its gap is within this share of the best.
const SAME_GAP: f64 = 1.15;
/// The share of the paired edges' length that must be closed across for a sheet to draw its
/// pipes as two lines.
const TWIN_SHARE: f64 = 0.25;
/// A long straight line (feet), and the share of their length a plan that draws its pipes as two
/// lines has in pairs.
const LONG_LINE_FT: f64 = 3.0;
const LONG_PAIRED_SHARE: f64 = 0.6;
/// The least paired length (feet) of a plan that draws its pipes as two lines (an enlarged room's
/// few feet of pipe is some), and the length past which its share of the long lines doesn't matter
/// (a floor plan's long lines include its lettering's and symbols').
const MIN_TWIN_FT: f64 = 8.0;
const PLENTY_TWIN_FT: f64 = 50.0;
/// A line across a pipe is square to it within this (cosine of its angle with the pipe).
const SQUARE_COS: f64 = 0.2;
/// The pieces across at one place (within this share of the pipe's width along it) together
/// cover at least [`CLOSED_COVER`] of its width to close it.
const CLOSE_STEP: f64 = 0.2;
const CLOSED_COVER: f64 = 0.7;
/// Grid cell for finding neighbouring edges (feet).
const CELL_FT: f64 = 4.0;
/// Most edges looked at on one sheet.
const MAX_EDGES: usize = 200_000;
/// Most candidates one edge is compared with.
const MAX_NEAR: usize = 400;
/// How far apart (feet) the ends of two middle lines may be and still be one run (through the
/// elbow or coupling drawn between them).
pub(crate) const JOIN_FT: f64 = 1.5;
/// A middle line turning back on the path more than this (cosine of 120°) is a pipe beside it.
const TURN_BACK_COS: f64 = -0.5;

/// A length of pipe's middle line: from `a` to `b`, drawn `width` across (drawing units), from
/// the edges `edges` (indices into the segments given).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Middle {
    pub a: Vec2,
    pub b: Vec2,
    pub width: f64,
    pub edges: [usize; 2],
}

/// One pair of parallel edges: the indices, the gap and where along the first they overlap.
struct Pair {
    i: usize,
    j: usize,
    gap: f64,
    t0: f64,
    t1: f64,
    /// The signed offset of the second edge from the first at `t0` and `t1`.
    o0: f64,
    o1: f64,
}

fn cells(a: Vec2, b: Vec2, pad: f64, cell: f64) -> impl Iterator<Item = (i64, i64)> {
    let (lo, hi) = (a.min(b), a.max(b));
    let x0 = ((lo.x - pad) / cell).floor() as i64;
    let x1 = ((hi.x + pad) / cell).floor() as i64;
    let y0 = ((lo.y - pad) / cell).floor() as i64;
    let y1 = ((hi.y + pad) / cell).floor() as i64;
    // A very long edge spans many cells: capped (it is still found from its partners' cells).
    let x1 = x1.min(x0 + 256);
    let y1 = y1.min(y0 + 256);
    (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
}

/// The parallel pairs among `segs` (drawing units, `upf` per foot) that could be one pipe's edges.
fn pairs(segs: &[(Vec2, Vec2)], upf: f64) -> Vec<Pair> {
    let (min_w, max_w) = (MIN_WIDTH_IN / 12.0 * upf, MAX_WIDTH_IN / 12.0 * upf);
    let min_len = MIN_EDGE_FT * upf;
    let cell = CELL_FT * upf;
    let long: Vec<usize> = segs.iter().enumerate().filter(|(_, (a, b))| a.dist(*b) >= min_len).map(|(i, _)| i).take(MAX_EDGES).collect();
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for &i in &long {
        let Some((a, b)) = segs.get(i) else { continue };
        for c in cells(*a, *b, 0.0, cell) {
            grid.entry(c).or_default().push(i);
        }
    }
    let mut out = Vec::new();
    for &i in &long {
        let Some(&(a, b)) = segs.get(i) else { continue };
        let len = a.dist(b);
        let u = (b - a) / len;
        let n = u.perp();
        let mut near: Vec<usize> = cells(a, b, max_w, cell).filter_map(|c| grid.get(&c)).flatten().copied().filter(|j| *j > i).collect();
        near.sort_unstable();
        near.dedup();
        for &j in near.iter().take(MAX_NEAR) {
            let Some(&(c, d)) = segs.get(j) else { continue };
            let lj = c.dist(d);
            let v = (d - c) / lj;
            if u.cross(v).abs() > PARALLEL_SIN {
                continue;
            }
            let (oc, od) = ((c - a).dot(n), (d - a).dot(n));
            if oc.signum() != od.signum() {
                continue;
            }
            let gap = 0.5 * (oc.abs() + od.abs());
            if !(min_w..=max_w).contains(&gap) || (oc - od).abs() > 0.3 * gap.max(min_w) {
                continue;
            }
            let (tc, td) = ((c - a).dot(u), (d - a).dot(u));
            let (t0, t1) = (tc.min(td).max(0.0), tc.max(td).min(len));
            if t1 - t0 < min_len {
                continue;
            }
            // The other edge's offset at t0 and t1 (it may lean a little).
            let at = |t: f64| if (td - tc).abs() > 1e-12 { oc + (od - oc) * (t - tc) / (td - tc) } else { oc };
            out.push(Pair { i, j, gap, t0, t1, o0: at(t0), o1: at(t1) });
        }
    }
    out
}

/// Each pipe's middle lines among `segs`, when the sheet draws its pipes as two lines; empty
/// when it doesn't ([`is_twin_drawn`]).
pub(crate) fn middles(segs: &[(Vec2, Vec2)], upf: f64) -> Vec<Middle> {
    let all = pairs(segs, upf);
    // An edge's nearest partner; a pair is one pipe when it is (about) both edges' nearest.
    let mut best: HashMap<usize, f64> = HashMap::new();
    for p in &all {
        for k in [p.i, p.j] {
            let e = best.entry(k).or_insert(f64::INFINITY);
            *e = e.min(p.gap);
        }
    }
    let tol = 1e-6 * upf.max(1.0);
    let kept: Vec<&Pair> = all.iter().filter(|p| [p.i, p.j].iter().all(|k| best.get(k).is_some_and(|g| p.gap <= g * SAME_GAP + tol))).collect();
    // Pipes drawn to scale: closed across by their fittings, and either plenty of them or most of
    // the plan's long lines.
    let paired: f64 = kept.iter().map(|p| p.t1 - p.t0).sum();
    if !is_twin_drawn(segs, &kept, upf) || (paired < PLENTY_TWIN_FT * upf && !mostly_paired(segs, &kept, upf)) {
        return Vec::new();
    }
    kept.iter()
        .filter_map(|p| {
            let (a, b) = *segs.get(p.i)?;
            let u = (b - a).normalized();
            let n = u.perp();
            Some(Middle { a: a + u * p.t0 + n * (p.o0 / 2.0), b: a + u * p.t1 + n * (p.o1 / 2.0), width: p.gap, edges: [p.i, p.j] })
        })
        .collect()
}

/// Whether the plan's long straight lines are mostly pipe edges in pairs ([`LONG_PAIRED_SHARE`] of
/// their length): on a plan that draws its pipes to scale nearly every long line is one, while
/// single-line piping run side by side pairs only here and there.
fn mostly_paired(segs: &[(Vec2, Vec2)], pairs: &[&Pair], upf: f64) -> bool {
    let paired: std::collections::HashSet<usize> = pairs.iter().flat_map(|p| [p.i, p.j]).collect();
    let long = LONG_LINE_FT * upf;
    let (mut all, mut mine) = (0.0, 0.0);
    for (k, (a, b)) in segs.iter().enumerate().take(MAX_EDGES) {
        let l = a.dist(*b);
        if l >= long {
            all += l;
            if paired.contains(&k) {
                mine += l;
            }
        }
    }
    all > 0.0 && mine >= LONG_PAIRED_SHARE * all
}

/// Whether the pairs are drawn pipes: at least [`TWIN_SHARE`] of their length belongs to pairs
/// closed across somewhere (a fitting's band, a cap: lines square to the pipe covering most of the
/// way from one edge to the other at one place along it; a print's import often leaves such a line
/// in short pieces).
fn is_twin_drawn(segs: &[(Vec2, Vec2)], pairs: &[&Pair], upf: f64) -> bool {
    let total: f64 = pairs.iter().map(|p| p.t1 - p.t0).sum();
    // A few parallel lines (a pair of pipes side by side, a symbol's sides) are no pipes drawn to
    // scale: a plan that draws them so has its piping so.
    if total < MIN_TWIN_FT * upf {
        return false;
    }
    let cell = CELL_FT * upf;
    let max_w = MAX_WIDTH_IN / 12.0 * upf;
    // The short lines, by cell.
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (k, (a, b)) in segs.iter().enumerate().take(MAX_EDGES) {
        if a.dist(*b) <= 1.5 * max_w {
            let m = a.mid(*b);
            grid.entry(((m.x / cell).floor() as i64, (m.y / cell).floor() as i64)).or_default().push(k);
        }
    }
    let closed: f64 = pairs
        .iter()
        .filter(|p| {
            let Some(&(a, b)) = segs.get(p.i) else { return false };
            let len = a.dist(b);
            let u = (b - a) / len;
            let n = u.perp();
            let (lo, hi) = (p.o0.min(0.0), p.o0.max(0.0));
            let step = (CLOSE_STEP * p.gap).max(1e-9);
            // How much of the way across the square pieces cover, at each place along the pipe.
            let mut cover: HashMap<i64, f64> = HashMap::new();
            for k in cells(a, b, p.gap, cell).filter_map(|c| grid.get(&c)).flatten() {
                let Some(&(c, d)) = segs.get(*k) else { continue };
                let l = c.dist(d);
                if l <= 0.0 || ((d - c).dot(u) / l).abs() > SQUARE_COS {
                    continue;
                }
                let t = (c.mid(d) - a).dot(u);
                if t < -p.gap || t > len + p.gap {
                    continue;
                }
                let (oc, od) = ((c - a).dot(n), (d - a).dot(n));
                let (s0, s1) = (oc.min(od).max(lo), oc.max(od).min(hi));
                if s1 > s0 {
                    *cover.entry((t / step).round() as i64).or_default() += s1 - s0;
                }
            }
            cover.values().any(|v| *v >= CLOSED_COVER * p.gap)
        })
        .map(|p| p.t1 - p.t0)
        .sum();
    closed >= TWIN_SHARE * total
}

/// The middle lines joined end to end into paths: two ends within [`JOIN_FT`] are one path
/// through the fitting between them; where three or more meet (a tee) the nearest two join and
/// the third starts its own path. Each path with the middles it took.
pub(crate) fn paths(mids: &[Middle], upf: f64) -> Vec<(Vec<Vec2>, Vec<usize>)> {
    let reach = JOIN_FT * upf;
    let cell = reach.max(1e-9);
    let key = |p: Vec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
    let mut ends: HashMap<(i64, i64), Vec<(usize, bool)>> = HashMap::new();
    for (i, m) in mids.iter().enumerate() {
        ends.entry(key(m.a)).or_default().push((i, false));
        ends.entry(key(m.b)).or_default().push((i, true));
    }
    let end_of = |i: usize, at_b: bool| mids.get(i).map(|m| if at_b { m.b } else { m.a });
    let mut used = vec![false; mids.len()];
    // The unused middle with an end nearest `p` (within reach) that goes on from the path's last
    // direction `dir` (not back the way it came: a pipe beside it), and whether that end is its `b`.
    let nearest = |p: Vec2, dir: Vec2, used: &[bool]| -> Option<(usize, bool)> {
        let (kx, ky) = key(p);
        let mut best: Option<(f64, usize, bool)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &(i, at_b) in ends.get(&(kx + dx, ky + dy)).into_iter().flatten() {
                    if used.get(i).copied().unwrap_or(true) {
                        continue;
                    }
                    let (Some(q), Some(far)) = (end_of(i, at_b), end_of(i, !at_b)) else { continue };
                    if (far - q).normalized().dot(dir) < TURN_BACK_COS {
                        continue;
                    }
                    let d = q.dist(p);
                    if d <= reach && best.is_none_or(|(bd, ..)| d < bd) {
                        best = Some((d, i, at_b));
                    }
                }
            }
        }
        best.map(|(_, i, at_b)| (i, at_b))
    };
    let mut out = Vec::new();
    for start in 0..mids.len() {
        if used.get(start).copied().unwrap_or(true) {
            continue;
        }
        let Some(m) = mids.get(start) else { continue };
        if let Some(u) = used.get_mut(start) {
            *u = true;
        }
        let mut pts = std::collections::VecDeque::from(vec![m.a, m.b]);
        let mut took = vec![start];
        // Grow at the tail, then at the head.
        for tail in [true, false] {
            // The path's end there, and the point before it.
            let end = |pts: &std::collections::VecDeque<Vec2>| -> Option<(Vec2, Vec2)> {
                if tail { Some((*pts.back()?, *pts.get(pts.len().saturating_sub(2))?)) } else { Some((*pts.front()?, *pts.get(1)?)) }
            };
            while let Some((p, before)) = end(&pts) {
                let Some((i, at_b)) = nearest(p, (p - before).normalized(), &used) else { break };
                let Some(n) = mids.get(i) else { break };
                if let Some(u) = used.get_mut(i) {
                    *u = true;
                }
                took.push(i);
                // Enter at the near end, leave at the far one.
                let (near, far) = if at_b { (n.b, n.a) } else { (n.a, n.b) };
                if tail {
                    if near.dist(p) > 1e-9 {
                        pts.push_back(near);
                    }
                    pts.push_back(far);
                } else {
                    if near.dist(p) > 1e-9 {
                        pts.push_front(near);
                    }
                    pts.push_front(far);
                }
                if took.len() >= MAX_EDGES {
                    break;
                }
            }
        }
        out.push((simplify(pts.into_iter().collect(), 1e-3 * upf), took));
    }
    out
}

/// A label (a pipe tag's index) for each path that has none, from the labelled paths it joins:
/// an end of one within `reach` of the other's line (a branch at its tee, a length at its
/// coupling), breadth first from the labelled ones. A water plan tags a main once and its branches
/// not at all; each branch is the system of the main it comes off.
pub(crate) fn spread(paths: &[Vec<Vec2>], seed: &[Option<usize>], reach: f64) -> Vec<Option<usize>> {
    let mut out: Vec<Option<usize>> = (0..paths.len()).map(|i| seed.get(i).copied().flatten()).collect();
    let cell = reach.max(1e-9) * 4.0;
    let key = |p: Vec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<(usize, Vec2, Vec2)>> = HashMap::new();
    for (i, pts) in paths.iter().enumerate() {
        for w in pts.windows(2) {
            let [a, b] = w else { continue };
            for c in cells(*a, *b, reach, cell) {
                grid.entry(c).or_default().push((i, *a, *b));
            }
        }
    }
    let dist = |p: Vec2, a: Vec2, b: Vec2| {
        let d = b - a;
        let l2 = d.dot(d);
        let t = if l2 > 1e-18 { ((p - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
        p.dist(a + d * t)
    };
    // Who joins whom (both ways).
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); paths.len()];
    for (i, pts) in paths.iter().enumerate() {
        for end in [pts.first(), pts.last()].into_iter().flatten() {
            for &(j, a, b) in grid.get(&key(*end)).into_iter().flatten() {
                if j != i && dist(*end, a, b) <= reach {
                    if let Some(v) = adj.get_mut(i) {
                        v.push(j);
                    }
                    if let Some(v) = adj.get_mut(j) {
                        v.push(i);
                    }
                }
            }
        }
    }
    let mut queue: std::collections::VecDeque<usize> = (0..paths.len()).filter(|i| out.get(*i).copied().flatten().is_some()).collect();
    while let Some(i) = queue.pop_front() {
        let Some(label) = out.get(i).copied().flatten() else { continue };
        for &j in adj.get(i).into_iter().flatten() {
            if let Some(slot) = out.get_mut(j)
                && slot.is_none()
            {
                *slot = Some(label);
                queue.push_back(j);
            }
        }
    }
    out
}

/// A path without its in-line points (each within `tol` of the line through its neighbours).
fn simplify(pts: Vec<Vec2>, tol: f64) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(pts.len());
    for p in pts {
        while out.len() >= 2 {
            let (Some(&a), Some(&b)) = (out.get(out.len() - 2), out.last()) else { break };
            let ab = p - a;
            let l = ab.len();
            if l > 0.0 && (b - a).cross(ab).abs() / l <= tol && (b - a).dot(ab) >= 0.0 && (b - a).len() <= l {
                out.pop();
            } else {
                break;
            }
        }
        if out.last().is_none_or(|q| q.dist(p) > tol) {
            out.push(p);
        }
    }
    out
}

/// The nominal pipe size (inches) whose outside diameter is drawn `width_in` across (full-size
/// inches), when one is within a fifth of it: the steel/plastic pipe outside diameters of ASME
/// B36.10 / ASTM D1785 (copper tube is 1/8" over its nominal size; the nearest still comes out).
pub(crate) fn nominal_from_width(width_in: f64) -> Option<f64> {
    const OD: &[(f64, f64)] = &[
        (0.5, 0.84),
        (0.75, 1.05),
        (1.0, 1.315),
        (1.25, 1.66),
        (1.5, 1.9),
        (2.0, 2.375),
        (2.5, 2.875),
        (3.0, 3.5),
        (4.0, 4.5),
        (5.0, 5.563),
        (6.0, 6.625),
        (8.0, 8.625),
    ];
    OD.iter()
        .min_by(|a, b| (a.1 - width_in).abs().total_cmp(&(b.1 - width_in).abs()))
        .filter(|(_, od)| (od - width_in).abs() <= 0.2 * od)
        .map(|(n, _)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UPF: f64 = 12.0;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    /// A 4" pipe (4.5" outside) along x from 0 to 960 at y=100, turning up at x=960 to y=300,
    /// with couplings drawn across every 40 ft and its two ends capped.
    fn drawn_pipe() -> Vec<(Vec2, Vec2)> {
        let w = 4.5;
        let mut s = vec![
            (v(0.0, 100.0), v(960.0 - 6.0, 100.0)),
            (v(0.0, 100.0 + w), v(960.0 - 6.0, 100.0 + w)),
            (v(960.0, 106.0), v(960.0, 300.0)),
            (v(960.0 - w, 106.0 + w), v(960.0 - w, 300.0)),
            // The elbow, drawn as two short diagonals.
            (v(954.0, 100.0), v(960.0, 106.0)),
            (v(954.0, 100.0 + w), v(960.0 - w, 106.0 + w)),
            // Caps.
            (v(0.0, 100.0), v(0.0, 100.0 + w)),
            (v(960.0, 300.0), v(960.0 - w, 300.0)),
        ];
        for k in 1..=2 {
            let x = 480.0 * f64::from(k) - 1.0;
            s.push((v(x, 100.0), v(x, 100.0 + w)));
        }
        s
    }

    #[test]
    fn a_pipe_drawn_as_two_lines_is_one_run_along_its_middle() {
        let segs = drawn_pipe();
        let mids = middles(&segs, UPF);
        assert_eq!(mids.len(), 2, "{mids:?}");
        for m in &mids {
            assert!((m.width - 4.5).abs() < 1e-9);
        }
        let ps = paths(&mids, UPF);
        assert_eq!(ps.len(), 1, "one run through the elbow: {ps:?}");
        let (pts, took) = &ps[0];
        assert_eq!(took.len(), 2);
        let len: f64 = pts.windows(2).map(|w| w[0].dist(w[1])).sum();
        // 954 + 194 along the middles plus the elbow between them: about 80 + 16 ft, once.
        assert!((len / UPF - 96.0).abs() < 0.5, "{len}");
        assert!(pts.iter().all(|p| (p.y - 102.25).abs() < 1e-6 || (p.x - 957.75).abs() < 1e-6), "{pts:?}");
    }

    #[test]
    fn a_coupling_drawn_in_short_pieces_still_closes_the_pipe() {
        // A 4" pipe whose couplings' lines across came in as quarter-inch pieces.
        let w = 4.5;
        let mut segs = vec![(v(0.0, 0.0), v(960.0, 0.0)), (v(0.0, w), v(960.0, w))];
        let mut y = 0.0;
        while y < w {
            segs.push((v(480.0, y), v(480.0, (y + 0.25).min(w))));
            y += 0.25;
        }
        assert_eq!(middles(&segs, UPF).len(), 1);
    }

    #[test]
    fn a_few_short_pairs_are_not_a_plan_of_pipes_drawn_to_scale() {
        // 6 ft of a pair closed across (a symbol's sides, a short sleeve): the plan's single lines stay.
        let segs = vec![(v(0.0, 0.0), v(72.0, 0.0)), (v(0.0, 3.0), v(72.0, 3.0)), (v(36.0, 0.0), v(36.0, 3.0))];
        assert!(middles(&segs, UPF).is_empty());
        // 10 ft of one among long single lines (40 ft each side by side, a plan of single-line pipes).
        let mut more = vec![(v(0.0, 0.0), v(120.0, 0.0)), (v(0.0, 3.0), v(120.0, 3.0)), (v(60.0, 0.0), v(60.0, 3.0))];
        more.extend([(v(0.0, 200.0), v(480.0, 200.0)), (v(0.0, 300.0), v(480.0, 300.0))]);
        assert!(middles(&more, UPF).is_empty());
        // An enlarged room's 12 ft of pipe drawn to scale, its plan's only long lines: a pipe.
        let room = vec![(v(0.0, 0.0), v(144.0, 0.0)), (v(0.0, 2.4), v(144.0, 2.4)), (v(72.0, 0.0), v(72.0, 2.4))];
        assert_eq!(middles(&room, UPF).len(), 1);
    }

    #[test]
    fn two_single_line_pipes_side_by_side_are_not_one_pipe() {
        // Cold and hot water 4" apart, nothing drawn across them.
        let segs = vec![(v(0.0, 0.0), v(240.0, 0.0)), (v(0.0, 4.0), v(240.0, 4.0))];
        assert!(middles(&segs, UPF).is_empty());
    }

    #[test]
    fn two_pipes_side_by_side_each_keep_their_own_edges() {
        // Two 2" pipes (2.375" outside) 3" apart, each with a coupling across.
        let w = 2.375;
        let mut segs = Vec::new();
        for y0 in [0.0, w + 3.0] {
            segs.push((v(0.0, y0), v(960.0, y0)));
            segs.push((v(0.0, y0 + w), v(960.0, y0 + w)));
            segs.push((v(479.0, y0), v(479.0, y0 + w)));
        }
        let mids = middles(&segs, UPF);
        assert_eq!(mids.len(), 2, "{mids:?}");
        let mut ys: Vec<f64> = mids.iter().map(|m| m.a.y).collect();
        ys.sort_by(f64::total_cmp);
        assert!((ys[0] - w / 2.0).abs() < 1e-9 && (ys[1] - (w + 3.0 + w / 2.0)).abs() < 1e-9, "{ys:?}");
        assert_eq!(paths(&mids, UPF).len(), 2);
    }

    #[test]
    fn a_branch_takes_the_tag_of_the_main_it_comes_off() {
        let main = vec![v(0.0, 0.0), v(240.0, 0.0)];
        // Off the main at its tee (ending 6" short of the main's middle), and on from the branch.
        let branch = vec![v(120.0, 6.0), v(120.0, 120.0)];
        let on = vec![v(120.0, 130.0), v(120.0, 200.0)];
        let apart = vec![v(400.0, 0.0), v(500.0, 0.0)];
        let out = spread(&[main, branch, on, apart], &[Some(7), None, None, None], 12.0);
        assert_eq!(out, [Some(7), Some(7), Some(7), None]);
    }

    #[test]
    fn a_drawn_width_gives_the_nominal_size() {
        assert_eq!(nominal_from_width(4.5), Some(4.0));
        assert_eq!(nominal_from_width(3.6), Some(3.0));
        assert_eq!(nominal_from_width(1.1), Some(0.75));
        assert_eq!(nominal_from_width(14.0), None);
    }
}
