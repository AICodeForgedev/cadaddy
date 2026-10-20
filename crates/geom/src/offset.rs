//! Offsetting a polyline of lines and arcs exactly: the raw offset (each segment offset, its
//! neighbours joined at their intersection, or by an arc about the corner where they don't
//! meet), then clipped where it crosses itself, keeping the pieces that are at the offset distance
//! from the polyline — the pair-wise clipping of the raw offset curve of B. K. Choi and S. C.
//! Park, "A pair-wise offset algorithm for 2D point-sequence curve", Computer-Aided Design 31(12)
//! (1999), here on lines and arcs instead of point sequences, so the result is exact (offset lines
//! and arcs are lines and arcs).
//!
//! Every tolerance is relative to the polyline's size, so a drawing in millimetres, in miles, far
//! from the origin or tiny gives the same result scaled.

use crate::{Arc, Line, PolyVertex, Polyline, Segment, Vec2, arc_to_bulge, ccw_sweep, intersect_segments, intersect_segments_ext};

/// Segments clipped against each other at most (past it, the raw offset is returned unclipped).
const MAX_CLIPPED: usize = 4000;

/// One polyline of an offset, with the segment of the source polyline each of its segments came
/// from (`None` for a join arc), for the widths.
#[derive(Clone, Debug, PartialEq)]
pub struct OffsetPiece {
    pub polyline: Polyline,
    pub sources: Vec<Option<usize>>,
}

/// A segment's endpoints and direction tolerance for a polyline of size `scale`.
fn tolerance(scale: f64) -> f64 {
    scale * 1e-9
}

/// `s` from point `a` to point `b` on it (both on its line or circle), in its direction.
fn sub(s: &Segment, a: Vec2, b: Vec2) -> Segment {
    match *s {
        Segment::Line(_) => Segment::Line(Line::new(a, b)),
        Segment::Arc { arc, ccw } => {
            let (sa, sb) = (arc.center.angle_to(a), arc.center.angle_to(b));
            let arc = if ccw { Arc::new(arc.center, arc.radius, sa, sb) } else { Arc::new(arc.center, arc.radius, sb, sa) };
            Segment::Arc { arc, ccw }
        }
    }
}

/// How far along `s` (from its start, in its direction) point `p` is: a length for a line, a
/// sweep for an arc. Used only to order points on one segment.
fn along(s: &Segment, p: Vec2) -> f64 {
    match *s {
        Segment::Line(l) => (p - l.a).dot(l.b - l.a),
        Segment::Arc { arc, ccw } => {
            let a = arc.center.angle_to(p);
            let from = arc.center.angle_to(s.start());
            if ccw { ccw_sweep(from, a) } else { ccw_sweep(a, from) }
        }
    }
}

/// The distance from `p` to the polyline's segments.
fn dist_to(segs: &[Segment], p: Vec2) -> f64 {
    segs.iter().map(|s| s.closest(p).dist(p)).fold(f64::INFINITY, f64::min)
}

/// Join offset segment `a` to `b` around source corner `v`: the point where they meet (extended
/// as lines or full circles), or an arc about `v` (radius `|d|`) where they don't.
fn join(a: &Segment, b: &Segment, v: Vec2, d: f64, eps: f64) -> (Segment, Option<Segment>, Segment) {
    if a.end().near(b.start(), eps) {
        return (*a, None, *b);
    }
    let hits = intersect_segments_ext(a, b, true);
    let reach = (d.abs() * 1e3).max(a.len().max(b.len()) * 4.0);
    if let Some(p) = hits.into_iter().filter(|p| p.is_finite() && p.dist(v) <= reach).min_by(|x, y| x.dist(v).total_cmp(&y.dist(v))) {
        return (sub(a, a.start(), p), None, sub(b, p, b.end()));
    }
    // Round join: the short way round the corner, at the offset distance.
    let (pa, pb) = (a.end(), b.start());
    let ccw = (pa - v).cross(pb - v) > 0.0;
    let (sa, sb) = (v.angle_to(pa), v.angle_to(pb));
    let arc = if ccw { Arc::new(v, d.abs(), sa, sb) } else { Arc::new(v, d.abs(), sb, sa) };
    (*a, Some(Segment::Arc { arc, ccw }), *b)
}

/// The offset of `pl` by `d` (positive: to the left of its direction of travel): one or more
/// polylines (a shape can split in two when offset inward), empty when nothing is left at that
/// distance.
pub fn offset_polyline(pl: &Polyline, d: f64) -> Vec<OffsetPiece> {
    let src: Vec<(usize, Segment)> = pl.segments().into_iter().enumerate().filter(|(_, s)| s.len() > 0.0).collect();
    if src.is_empty() || !d.is_finite() || d == 0.0 {
        return Vec::new();
    }
    let source: Vec<Segment> = src.iter().map(|(_, s)| *s).collect();
    let b = source.iter().fold(source[0].bounds(), |acc, s| acc.union(&s.bounds()));
    let scale = b.width().hypot(b.height()).max(b.min.x.abs().max(b.min.y.abs()).max(b.max.x.abs().max(b.max.y.abs())) * 1e-6) + d.abs();
    let eps = tolerance(scale);
    // Each segment offset (an arc whose radius would go to nothing has no offset).
    let offs: Vec<(usize, Option<Segment>, Vec2)> = src
        .iter()
        .map(|(i, s)| {
            let o = s.offset(d).filter(|o| match o {
                Segment::Arc { arc, .. } => arc.radius > eps,
                Segment::Line(_) => true,
            });
            (*i, o, s.end())
        })
        .collect();
    let kept: Vec<(usize, Segment, Vec2)> = offs.iter().filter_map(|(i, o, v)| o.map(|o| (*i, o, *v))).collect();
    if kept.is_empty() {
        return Vec::new();
    }
    // The raw offset: neighbours joined (the last to the first when closed).
    let n = kept.len();
    let mut segs: Vec<Segment> = kept.iter().map(|(_, s, _)| *s).collect();
    let mut joins: Vec<Option<Segment>> = vec![None; n];
    let pairs = if pl.closed { n } else { n - 1 };
    for k in 0..pairs {
        let (a, b) = (k, (k + 1) % n);
        let v = kept[a].2;
        let (na, mid, nb) = join(&segs[a], &segs[b], v, d, eps);
        segs[a] = na;
        segs[b] = nb;
        joins[a] = mid;
    }
    let mut raw: Vec<(Segment, Option<usize>)> = Vec::with_capacity(2 * n);
    for k in 0..n {
        raw.push((segs[k], Some(kept[k].0)));
        if let Some(j) = joins[k] {
            raw.push((j, None));
        }
    }
    raw.retain(|(s, _)| s.len() > eps);
    if raw.is_empty() {
        return Vec::new();
    }
    // Clip: split where the raw offset crosses itself, keep the pieces at the offset distance.
    let mut pieces: Vec<(Segment, Option<usize>)> = Vec::new();
    if raw.len() <= MAX_CLIPPED {
        let m = raw.len();
        let mut cuts: Vec<Vec<Vec2>> = vec![Vec::new(); m];
        for i in 0..m {
            for j in i + 1..m {
                let adjacent = j == i + 1 || (pl.closed && i == 0 && j == m - 1);
                for p in intersect_segments(&raw[i].0, &raw[j].0) {
                    // Neighbours meet at their shared end: not a crossing.
                    if adjacent && (p.near(raw[i].0.end(), eps * 10.0) || p.near(raw[i].0.start(), eps * 10.0)) {
                        continue;
                    }
                    cuts[i].push(p);
                    cuts[j].push(p);
                }
                // Where one ends on the other (they overlap along a line or meet in a T), the other
                // is cut there too: crossing tests find no point on overlapping lines.
                for (a, b) in [(i, j), (j, i)] {
                    for p in [raw[b].0.start(), raw[b].0.end()] {
                        if raw[a].0.closest(p).near(p, eps * 10.0) {
                            cuts[a].push(p);
                        }
                    }
                }
            }
        }
        for (k, (s, from)) in raw.iter().enumerate() {
            let mut at: Vec<Vec2> = cuts[k].iter().copied().filter(|p| !p.near(s.start(), eps) && !p.near(s.end(), eps)).collect();
            at.sort_by(|x, y| along(s, *x).total_cmp(&along(s, *y)));
            let mut start = s.start();
            for p in at.into_iter().chain(std::iter::once(s.end())) {
                if !p.near(start, eps) {
                    pieces.push((sub(s, start, p), *from));
                }
                start = p;
            }
        }
        let keep = d.abs() * (1.0 - 1e-6) - eps * 10.0;
        pieces.retain(|(s, _)| dist_to(&source, s.mid()) >= keep);
        // Where two kept pieces overlap (the same stretch, twice), one is enough.
        let same = |a: &Segment, b: &Segment| {
            a.start().near(b.start(), eps * 10.0) && a.end().near(b.end(), eps * 10.0) && a.mid().near(b.mid(), eps * 10.0)
        };
        let mut unique: Vec<(Segment, Option<usize>)> = Vec::with_capacity(pieces.len());
        for p in pieces {
            if !unique.iter().any(|(q, _)| same(&p.0, q)) {
                unique.push(p);
            }
        }
        pieces = unique;
    } else {
        pieces = raw;
    }
    chain(pieces, pl.closed, eps * 100.0)
}

/// The kept pieces joined end to start into polylines, in the raw offset's order.
fn chain(pieces: Vec<(Segment, Option<usize>)>, closed: bool, eps: f64) -> Vec<OffsetPiece> {
    let n = pieces.len();
    let mut used = vec![false; n];
    let mut out = Vec::new();
    for first in 0..n {
        if used[first] {
            continue;
        }
        used[first] = true;
        let mut run = vec![first];
        let mut cur = first;
        loop {
            let end = pieces[cur].0.end();
            if run.len() > 1 && end.near(pieces[first].0.start(), eps) {
                break;
            }
            // The next piece starting here, the nearest after this one in the raw order.
            let next = (1..n).map(|k| (cur + k) % n).find(|k| !used[*k] && pieces[*k].0.start().near(end, eps));
            match next {
                Some(k) => {
                    used[k] = true;
                    run.push(k);
                    cur = k;
                }
                None => break,
            }
        }
        // An open run that another one continues (the first of an open polyline's chain wasn't its
        // start): joined later by its start.
        let is_closed = run.len() > 1 && pieces[cur].0.end().near(pieces[first].0.start(), eps);
        let segs: Vec<&(Segment, Option<usize>)> = run.iter().map(|k| &pieces[*k]).collect();
        out.push((segs.iter().map(|(s, f)| (*s, *f)).collect::<Vec<_>>(), is_closed));
    }
    // Open runs whose end meets another's start become one.
    let mut merged: Vec<(Vec<(Segment, Option<usize>)>, bool)> = Vec::new();
    for (run, is_closed) in out {
        if !is_closed && let Some(m) = merged.iter_mut().find(|(r, c)| !*c && r.last().is_some_and(|(s, _)| s.end().near(run[0].0.start(), eps))) {
            m.0.extend(run);
            continue;
        }
        if !is_closed
            && let Some(m) = merged.iter_mut().find(|(r, c)| !*c && r[0].0.start().near(run.last().map_or(Vec2::ZERO, |(s, _)| s.end()), eps))
        {
            let mut joined = run;
            joined.append(&mut m.0);
            m.0 = joined;
            continue;
        }
        merged.push((run, is_closed));
    }
    let _ = closed;
    merged
        .into_iter()
        .map(|(run, is_closed)| (simplify(run, is_closed, eps), is_closed))
        .map(|(run, is_closed)| {
            let mut vertices: Vec<PolyVertex> = run
                .iter()
                .map(|(s, _)| {
                    let bulge = match s {
                        Segment::Line(_) => 0.0,
                        Segment::Arc { arc, ccw } => arc_to_bulge(if *ccw { arc.sweep() } else { -arc.sweep() }),
                    };
                    PolyVertex::with_bulge(s.start(), bulge)
                })
                .collect();
            if !is_closed && let Some((s, _)) = run.last() {
                vertices.push(PolyVertex::new(s.end()));
            }
            OffsetPiece { polyline: Polyline { vertices, closed: is_closed }, sources: run.iter().map(|(_, f)| *f).collect() }
        })
        .collect()
}

/// Consecutive pieces that continue one line or one arc (split where nothing crossed in the end)
/// made one again.
fn simplify(run: Vec<(Segment, Option<usize>)>, closed: bool, eps: f64) -> Vec<(Segment, Option<usize>)> {
    let continues = |a: &Segment, b: &Segment| match (a, b) {
        (Segment::Line(x), Segment::Line(y)) => {
            let (u, v) = (x.b - x.a, y.b - y.a);
            u.dot(v) > 0.0 && u.cross(v).abs() <= eps * (u.len() + v.len()).max(eps)
        }
        (Segment::Arc { arc: x, ccw: cx }, Segment::Arc { arc: y, ccw: cy }) => {
            cx == cy && x.center.near(y.center, eps * 10.0) && (x.radius - y.radius).abs() <= eps * 10.0
        }
        _ => false,
    };
    let join = |a: &Segment, b: &Segment| sub(a, a.start(), b.end());
    let mut out: Vec<(Segment, Option<usize>)> = Vec::with_capacity(run.len());
    for (s, from) in run {
        match out.last_mut() {
            // One line or arc again (its widths are the first piece's source's).
            Some((last, _)) if continues(last, &s) => *last = join(last, &s),
            _ => out.push((s, from)),
        }
    }
    // Round the seam of a closed run.
    if closed && out.len() > 2 {
        let (first, last) = (out[0], out[out.len() - 1]);
        if continues(&last.0, &first.0) {
            out[0] = (join(&last.0, &first.0), last.1);
            out.pop();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poly(pts: &[(f64, f64, f64)], closed: bool) -> Polyline {
        Polyline { vertices: pts.iter().map(|&(x, y, b)| PolyVertex::with_bulge(Vec2::new(x, y), b)).collect(), closed }
    }

    /// No point along the offset is nearer the source than `d` (a mitred corner is farther), and
    /// the offset runs at exactly `d` along it.
    fn at_distance(src: &Polyline, out: &[OffsetPiece], d: f64) {
        let segs = src.segments();
        let tol = 1e-9 * (1.0 + d.abs()) * 100.0;
        let mut exact = 0;
        for piece in out {
            for s in piece.polyline.segments() {
                for i in 0..=20 {
                    let p = s.at(i as f64 / 20.0);
                    let e = dist_to(&segs, p) - d.abs();
                    assert!(e > -tol, "nearer than d by {} at {p:?}", -e);
                    exact += usize::from(e.abs() < tol);
                }
            }
        }
        assert!(exact > 0, "nowhere at exactly d");
    }

    #[test]
    fn a_rectangle_offsets_out_and_in() {
        let r = poly(&[(0.0, 0.0, 0.0), (10.0, 0.0, 0.0), (10.0, 5.0, 0.0), (0.0, 5.0, 0.0)], true);
        // Counter-clockwise: left is inside.
        let inside = offset_polyline(&r, 1.0);
        assert_eq!(inside.len(), 1);
        assert_eq!(inside[0].polyline.vertices.len(), 4);
        at_distance(&r, &inside, 1.0);
        let outside = offset_polyline(&r, -1.0);
        assert_eq!(outside[0].polyline.vertices.len(), 4, "mitred corners, as AutoCAD extends lines");
        // Inward past half the height: nothing left.
        assert!(offset_polyline(&r, 3.0).is_empty());
    }

    #[test]
    fn a_notch_narrower_than_twice_the_distance_is_clipped() {
        // A U of two tall arms with a gap of 1 between them: offset outward by 2 the arms' inner
        // offsets cross, and the loop between them goes.
        let u = poly(&[(0.0, 10.0, 0.0), (0.0, 0.0, 0.0), (5.0, 0.0, 0.0), (5.0, 10.0, 0.0), (6.0, 10.0, 0.0), (6.0, -1.0, 0.0)], false);
        let out = offset_polyline(&u, -0.75);
        assert!(!out.is_empty());
        at_distance(&u, &out, 0.75);
        // Without clipping, the raw offset would cross itself.
        for piece in &out {
            let segs = piece.polyline.segments();
            for i in 0..segs.len() {
                for j in i + 2..segs.len() {
                    let hits = intersect_segments(&segs[i], &segs[j]);
                    assert!(hits.iter().all(|p| p.near(segs[i].end(), 1e-6) || p.near(segs[i].start(), 1e-6)), "crosses itself: {hits:?}");
                }
            }
        }
    }

    #[test]
    fn arcs_offset_exactly_and_join_round_where_they_dont_meet() {
        // A line into a semicircle and back: offset outward, the arc grows by d.
        let p = poly(&[(0.0, 0.0, 0.0), (10.0, 0.0, 1.0), (10.0, 6.0, 0.0), (0.0, 6.0, 0.0)], false);
        let out = offset_polyline(&p, -1.0);
        assert_eq!(out.len(), 1);
        at_distance(&p, &out, 1.0);
        // A corner whose offsets don't meet when extended (an arc beside a line) joins with an arc.
        let q = poly(&[(0.0, 0.0, 0.5), (10.0, 0.0, 0.0), (10.0, 10.0, 0.0)], false);
        let out = offset_polyline(&q, 1.0);
        at_distance(&q, &out, 1.0);
    }

    #[test]
    fn an_arc_that_collapses_is_left_out() {
        // A small arc (radius 0.5) offset inward by 1: gone, its neighbours meet.
        let p = poly(&[(0.0, 0.0, 0.0), (10.0, 0.0, 1.0), (10.0, 1.0, 0.0), (0.0, 1.0, 0.0)], false);
        let out = offset_polyline(&p, 1.0);
        for piece in &out {
            assert!(piece.sources.iter().all(|s| *s != Some(1)), "{:?}", piece.sources);
        }
    }

    #[test]
    fn a_dumbbell_offset_inward_splits_in_two() {
        // Two 10×10 squares joined by a neck 1 wide; inward by 1 the neck closes.
        let db = poly(
            &[
                (0.0, 0.0, 0.0),
                (10.0, 0.0, 0.0),
                (10.0, 4.5, 0.0),
                (14.0, 4.5, 0.0),
                (14.0, 0.0, 0.0),
                (24.0, 0.0, 0.0),
                (24.0, 10.0, 0.0),
                (14.0, 10.0, 0.0),
                (14.0, 5.5, 0.0),
                (10.0, 5.5, 0.0),
                (10.0, 10.0, 0.0),
                (0.0, 10.0, 0.0),
            ],
            true,
        );
        let out = offset_polyline(&db, 1.0);
        assert_eq!(out.len(), 2, "{out:#?}");
        let brief: Vec<(bool, Vec<(f64, f64)>)> =
            out.iter().map(|p| (p.polyline.closed, p.polyline.vertices.iter().map(|v| (v.p.x, v.p.y)).collect())).collect();
        assert!(out.iter().all(|p| p.polyline.closed && p.polyline.vertices.len() == 4), "{brief:?}");
        at_distance(&db, &out, 1.0);
        // Not far enough to close the neck: one piece.
        assert_eq!(offset_polyline(&db, 0.25).len(), 1);
    }

    #[test]
    fn huge_and_tiny_drawings_give_the_same_offset() {
        let shapes = [
            (poly(&[(0.0, 0.0, 0.0), (10.0, 0.0, 0.0), (10.0, 5.0, 0.0), (0.0, 5.0, 0.0)], true), 1.0),
            (poly(&[(0.0, 10.0, 0.0), (0.0, 0.0, 0.0), (5.0, 0.0, 0.0), (5.0, 10.0, 0.0), (6.0, 10.0, 0.0), (6.0, -1.0, 0.0)], false), -0.75),
            (poly(&[(0.0, 0.0, 0.0), (10.0, 0.0, 1.0), (10.0, 6.0, 0.0), (0.0, 6.0, 0.0)], false), -1.0),
            (poly(&[(0.0, 0.0, 0.3), (8.0, 2.0, -0.4), (12.0, 9.0, 0.0), (3.0, 7.0, 0.2)], true), 0.6),
        ];
        for (shape, d) in &shapes {
            let base = offset_polyline(shape, *d);
            for (scale, shift) in [(1e-6, 0.0), (1e6, 0.0), (1.0, 1e7), (1e3, -5e8)] {
                let moved = Polyline {
                    vertices: shape.vertices.iter().map(|v| PolyVertex::with_bulge(v.p * scale + Vec2::new(shift, shift), v.bulge)).collect(),
                    closed: shape.closed,
                };
                let got = offset_polyline(&moved, d * scale);
                assert_eq!(got.len(), base.len(), "pieces at scale {scale} shift {shift}");
                for (g, b) in got.iter().zip(&base) {
                    assert_eq!(g.polyline.vertices.len(), b.polyline.vertices.len(), "vertices at scale {scale} shift {shift}");
                    for (gv, bv) in g.polyline.vertices.iter().zip(&b.polyline.vertices) {
                        let back = (gv.p - Vec2::new(shift, shift)) / scale;
                        let size = 1.0 + shift.abs() / scale;
                        assert!(back.dist(bv.p) < 1e-9 * size * 10.0, "{back:?} vs {:?} at scale {scale} shift {shift}", bv.p);
                        assert!((gv.bulge - bv.bulge).abs() < 1e-6);
                    }
                }
            }
        }
    }
}
