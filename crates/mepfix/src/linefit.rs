//! Registering one sheet's view on another by the linework both draw: the walls of an enlarged
//! plan are the walls of its part of the overall plan.
//!
//! Both sheets are read at their scale notes, so the scale is known (one to one, or the printed
//! scales' ratio) and the view may be turned a quarter at a time to fit its sheet. What is left
//! is where it goes: every pair of straight pieces of about the same length (one per sheet, the
//! same way round) votes for the offset that lays one on the other, the offsets most pieces vote
//! for are scored by how much of the view's linework lands on the other sheet's, and the best
//! is taken only when it covers most of the view and no other place covers nearly as much (a
//! typical room drawn many times along a corridor places nowhere).

use crate::tagid::Sim;

/// Most pieces kept per page.
pub const MAX_PAGE_LINES: usize = 8_000;
/// Most pieces of a view that vote.
const MAX_VOTERS: usize = 120;
/// Most pieces of a view scored.
const MAX_SCORED: usize = 600;
/// Most host pieces each voter pairs with.
const MAX_PAIRS: usize = 200;
/// Most offsets whose neighbours' votes are added in.
const MAX_SMOOTHED: usize = 400;
/// Most offsets scored per turn and scale.
const MAX_CANDIDATES: usize = 12;
/// Of the view's linework (by length), the share that must land on the other sheet's.
pub const MIN_COVERAGE: f64 = 0.45;
/// The second-best place (somewhere else) must cover less than this share of the best.
const MAX_RUNNER_UP: f64 = 0.85;

/// A straight piece along an axis: `horiz` pieces run in x at y = `at`, from `lo` to `hi`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AxisSeg {
    pub horiz: bool,
    pub at: f64,
    pub lo: f64,
    pub hi: f64,
}

impl AxisSeg {
    pub fn length(&self) -> f64 {
        self.hi - self.lo
    }

    pub fn mid(&self) -> [f64; 2] {
        let m = (self.lo + self.hi) / 2.0;
        if self.horiz { [m, self.at] } else { [self.at, m] }
    }

    /// The piece through `f` (a turn by quarters keeps it on an axis).
    fn mapped(&self, f: &Sim) -> AxisSeg {
        let (p, q) = if self.horiz { ([self.lo, self.at], [self.hi, self.at]) } else { ([self.at, self.lo], [self.at, self.hi]) };
        let (p, q) = (f.apply(p), f.apply(q));
        let horiz = (q[1] - p[1]).abs() <= (q[0] - p[0]).abs();
        if horiz {
            AxisSeg { horiz, at: (p[1] + q[1]) / 2.0, lo: p[0].min(q[0]), hi: p[0].max(q[0]) }
        } else {
            AxisSeg { horiz, at: (p[0] + q[0]) / 2.0, lo: p[1].min(q[1]), hi: p[1].max(q[1]) }
        }
    }
}

/// The page's straight pieces along an axis, at least `min_len` long, pieces of one line that
/// touch or overlap joined into one, the longest `cap` kept.
pub fn axis_segments(segs: &[[[f64; 2]; 2]], min_len: f64, cap: usize) -> Vec<AxisSeg> {
    if !(min_len.is_finite() && min_len > 0.0) {
        return Vec::new();
    }
    let mut out: Vec<AxisSeg> = Vec::new();
    for s in segs {
        let ([x0, y0], [x1, y1]) = (s[0], s[1]);
        let (dx, dy) = ((x1 - x0).abs(), (y1 - y0).abs());
        // Within about half a degree of an axis.
        if dy <= 0.01 * dx {
            out.push(AxisSeg { horiz: true, at: (y0 + y1) / 2.0, lo: x0.min(x1), hi: x0.max(x1) });
        } else if dx <= 0.01 * dy {
            out.push(AxisSeg { horiz: false, at: (x0 + x1) / 2.0, lo: y0.min(y1), hi: y0.max(y1) });
        }
    }
    out.retain(|a| [a.at, a.lo, a.hi].iter().all(|v| v.is_finite()));
    // Join the pieces of one line (a wall drawn in pieces, a polyline's sides).
    let eps = 0.01 * min_len;
    out.sort_by(|a, b| a.horiz.cmp(&b.horiz).then(a.at.total_cmp(&b.at)).then(a.lo.total_cmp(&b.lo)));
    let mut joined: Vec<AxisSeg> = Vec::with_capacity(out.len());
    for s in out {
        if let Some(last) = joined.last_mut()
            && last.horiz == s.horiz
            && (last.at - s.at).abs() <= eps
            && s.lo <= last.hi + eps
        {
            last.hi = last.hi.max(s.hi);
            continue;
        }
        joined.push(s);
    }
    joined.retain(|s| s.length() >= min_len);
    joined.sort_by(|a, b| b.length().total_cmp(&a.length()));
    joined.truncate(cap);
    joined
}

/// A view laid on another sheet by its linework.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineFit {
    pub sim: Sim,
    /// The share of the view's linework (by length) that lands on the other sheet's.
    pub coverage: f64,
    /// The same for the best place somewhere else (0 when none).
    pub runner_up: f64,
}

/// The other sheet's pieces, filed for lookups by line.
struct Host<'a> {
    /// Per direction (horizontal, vertical): pieces sorted by `at`.
    by_dir: [Vec<&'a AxisSeg>; 2],
    /// Per direction: pieces sorted by length (for the votes).
    by_len: [Vec<&'a AxisSeg>; 2],
}

impl<'a> Host<'a> {
    fn new(segs: &'a [AxisSeg]) -> Self {
        let mut by_dir: [Vec<&AxisSeg>; 2] = [Vec::new(), Vec::new()];
        for s in segs {
            by_dir[usize::from(!s.horiz)].push(s);
        }
        let mut by_len = by_dir.clone();
        for v in &mut by_dir {
            v.sort_by(|a, b| a.at.total_cmp(&b.at));
        }
        for v in &mut by_len {
            v.sort_by(|a, b| a.length().total_cmp(&b.length()));
        }
        Host { by_dir, by_len }
    }

    /// How much of `s` lies on this sheet's pieces (within `tol` across).
    fn covered(&self, s: &AxisSeg, tol: f64) -> f64 {
        let list = &self.by_dir[usize::from(!s.horiz)];
        let start = list.partition_point(|h| h.at < s.at - tol);
        let mut spans: Vec<(f64, f64)> = Vec::new();
        for h in list.iter().skip(start).take_while(|h| h.at <= s.at + tol).take(64) {
            let (a, b) = (h.lo.max(s.lo - tol), h.hi.min(s.hi + tol));
            if b > a {
                spans.push((a.max(s.lo), b.min(s.hi)));
            }
        }
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut total, mut reach) = (0.0, f64::NEG_INFINITY);
        for (a, b) in spans {
            let a = a.max(reach);
            if b > a {
                total += b - a;
            }
            reach = reach.max(b);
        }
        total.min(s.length())
    }
}

/// The share of `view` (through `f`) lying on the host's pieces.
fn coverage(view: &[AxisSeg], f: &Sim, host: &Host, tol: f64) -> f64 {
    let (mut on, mut all) = (0.0, 0.0);
    for s in view {
        let m = s.mapped(f);
        all += m.length();
        on += host.covered(&m, tol);
    }
    if all > 0.0 { on / all } else { 0.0 }
}

/// Where `view`'s linework lies on `host`'s: the scale is one of `scales` (view units to host
/// units), the turn a multiple of 90°. `tol` is how far apart (host units) two drawings of one
/// line may be. `accept` says which placements are possible at all (inside a boundary box, by
/// the callout). None unless one place covers at least [`MIN_COVERAGE`] of the view and no other
/// place nearly as much.
pub fn fit_by_lines(view: &[AxisSeg], host: &[AxisSeg], scales: &[f64], tol: f64, accept: &dyn Fn(&Sim) -> bool) -> Option<LineFit> {
    if !(tol.is_finite() && tol > 0.0) || view.len() < 6 || host.len() < 6 {
        return None;
    }
    let mut view: Vec<AxisSeg> = view.to_vec();
    view.sort_by(|a, b| b.length().total_cmp(&a.length()));
    view.truncate(MAX_SCORED);
    // The view's extent: two places closer than a tenth of it are one place.
    let (lo, hi) = view.iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), s| {
        let ends = if s.horiz { [[s.lo, s.at], [s.hi, s.at]] } else { [[s.at, s.lo], [s.at, s.hi]] };
        let mut lo = lo;
        let mut hi = hi;
        for e in ends {
            lo = [lo[0].min(e[0]), lo[1].min(e[1])];
            hi = [hi[0].max(e[0]), hi[1].max(e[1])];
        }
        (lo, hi)
    });
    let extent = (hi[0] - lo[0]).hypot(hi[1] - lo[1]);
    if !(extent.is_finite() && extent > 0.0) {
        return None;
    }
    let h = Host::new(host);
    let voters: Vec<AxisSeg> = view.iter().take(MAX_VOTERS).copied().collect();
    // Every placement scored: (fit, coverage).
    let mut scored: Vec<(Sim, f64)> = Vec::new();
    for &s in scales.iter().filter(|s| s.is_finite() && **s > 0.0) {
        let bin = 2.0 * tol;
        for quarter in 0..4 {
            let (c, sn) = match quarter {
                0 => (1.0, 0.0),
                1 => (0.0, 1.0),
                2 => (-1.0, 0.0),
                _ => (0.0, -1.0),
            };
            let turn = Sim { a: s * c, b: s * sn, tx: 0.0, ty: 0.0 };
            // Votes: a pair of pieces of about one length, the same way round, lays one on the
            // other by matching their ends (both ends: a wall may be drawn a little longer on one
            // sheet).
            let mut cast: Vec<((i64, i64), f64)> = Vec::new();
            for v in &voters {
                let m = v.mapped(&turn);
                let list = &h.by_len[usize::from(!m.horiz)];
                let (lo_len, hi_len) = (0.8 * m.length(), 1.25 * m.length());
                // The host pieces nearest its length, either way (a common length may have
                // hundreds).
                let at = list.partition_point(|x| x.length() < m.length());
                let shorter = list.get(..at).unwrap_or_default().iter().rev().take_while(|x| x.length() >= lo_len).take(MAX_PAIRS / 2);
                let longer = list.get(at..).unwrap_or_default().iter().take_while(|x| x.length() <= hi_len).take(MAX_PAIRS / 2);
                for hseg in shorter.chain(longer) {
                    let across = hseg.at - m.at;
                    for along in [hseg.lo - m.lo, hseg.hi - m.hi] {
                        let (dx, dy) = if m.horiz { (along, across) } else { (across, along) };
                        let k = ((dx / bin).round() as i64, (dy / bin).round() as i64);
                        cast.push((k, m.length().min(hseg.length())));
                    }
                }
            }
            // The votes per offset, sorted by offset.
            cast.sort_unstable_by_key(|a| a.0);
            let mut votes: Vec<((i64, i64), f64)> = Vec::with_capacity(cast.len());
            for (k, w) in cast {
                match votes.last_mut() {
                    Some(last) if last.0 == k => last.1 += w,
                    _ => votes.push((k, w)),
                }
            }
            let vote = |k: (i64, i64)| votes.binary_search_by(|v| v.0.cmp(&k)).ok().and_then(|i| votes.get(i)).map_or(0.0, |v| v.1);
            if votes.is_empty() {
                continue;
            }
            // The offsets most voted for (with their neighbours' votes: one offset may fall
            // between two bins).
            let mut raw: Vec<((i64, i64), f64)> = votes.clone();
            raw.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            raw.truncate(MAX_SMOOTHED);
            let mut ranked: Vec<((i64, i64), f64)> = raw
                .into_iter()
                .map(|((x, y), _)| {
                    let mut w = 0.0;
                    for ddx in -1..=1 {
                        for ddy in -1..=1 {
                            w += vote((x + ddx, y + ddy));
                        }
                    }
                    ((x, y), w)
                })
                .collect();
            ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            let mut tried: Vec<(i64, i64)> = Vec::new();
            for ((x, y), _) in ranked {
                if tried.len() >= MAX_CANDIDATES {
                    break;
                }
                if tried.iter().any(|t| (t.0 - x).abs() <= 1 && (t.1 - y).abs() <= 1) {
                    continue;
                }
                tried.push((x, y));
                let f = Sim { tx: x as f64 * bin, ty: y as f64 * bin, ..turn };
                let f = refine(&view, f, &h, tol);
                if !accept(&f) {
                    continue;
                }
                let cov = coverage(&view, &f, &h, tol);
                scored.push((f, cov));
            }
        }
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    let (best, cov) = *scored.first()?;
    if cov < MIN_COVERAGE {
        return None;
    }
    // Another place: turned otherwise, or its middle a tenth of the view away.
    let mid = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let (pb, turn_b) = (best.apply(mid), best.rotation_deg());
    let runner_up = scored
        .iter()
        .skip(1)
        .filter(|(f, _)| {
            let p = f.apply(mid);
            (p[0] - pb[0]).hypot(p[1] - pb[1]) > 0.1 * extent * best.scale() || (f.rotation_deg() - turn_b).abs() > 1.0
        })
        .map(|(_, c)| *c)
        .fold(0.0, f64::max);
    (runner_up < MAX_RUNNER_UP * cov).then_some(LineFit { sim: best, coverage: cov, runner_up })
}

/// The placement made exact: each of the view's pieces lying near a host piece pairs its line
/// with that piece's; the offset (and a scale a few percent off one to one: two sheets' scale
/// notes may not quite agree) is the least-squares fit of those pairs, twice over.
fn refine(view: &[AxisSeg], f: Sim, host: &Host, tol: f64) -> Sim {
    let mut f = f;
    for _ in 0..2 {
        // (horizontal?, the view piece's line, the host piece's line)
        let mut pairs: Vec<(bool, f64, f64)> = Vec::new();
        for s in view {
            let m = s.mapped(&f);
            let list = &host.by_dir[usize::from(!m.horiz)];
            let start = list.partition_point(|h| h.at < m.at - 2.0 * tol);
            let near = list
                .iter()
                .skip(start)
                .take_while(|h| h.at <= m.at + 2.0 * tol)
                .filter(|h| h.lo < m.hi && h.hi > m.lo)
                .min_by(|a, b| (a.at - m.at).abs().total_cmp(&(b.at - m.at).abs()));
            if let Some(hs) = near {
                pairs.push((m.horiz, m.at, hs.at));
            }
        }
        // The scale: each axis's slope where the lines spread out enough to say.
        let slope = |horiz: bool| -> Option<f64> {
            let pts: Vec<(f64, f64)> = pairs.iter().filter(|p| p.0 == horiz).map(|p| (p.1, p.2)).collect();
            let n = pts.len() as f64;
            if pts.len() < 3 {
                return None;
            }
            let (mx, my) = (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n);
            let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
            let sxy: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
            (sxx > 100.0 * tol * tol).then(|| sxy / sxx).filter(|k| k.is_finite())
        };
        let k = match (slope(true), slope(false)) {
            (Some(a), Some(b)) => (a + b) / 2.0,
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => 1.0,
        };
        let k = if (k - 1.0).abs() <= 0.03 { k } else { 1.0 };
        let median = |horiz: bool| {
            let mut v: Vec<f64> = pairs.iter().filter(|p| p.0 == horiz).map(|p| p.2 - k * p.1).collect();
            v.sort_by(f64::total_cmp);
            v.get(v.len() / 2).copied()
        };
        // y' = k·y + cy for the horizontal pieces' lines, x' = k·x + cx for the vertical ones'.
        let cy = median(true).unwrap_or((1.0 - k) * f.ty);
        let cx = median(false).unwrap_or((1.0 - k) * f.tx);
        let g = Sim { a: k * f.a, b: k * f.b, tx: k * f.tx + cx, ty: k * f.ty + cy };
        if !g.is_finite() {
            break;
        }
        f = g;
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up floor: an outline, a corridor, rooms of different widths along it.
    fn floor() -> Vec<[[f64; 2]; 2]> {
        let mut s = vec![
            [[0.0, 0.0], [1200.0, 0.0]],
            [[1200.0, 0.0], [1200.0, 400.0]],
            [[1200.0, 400.0], [0.0, 400.0]],
            [[0.0, 400.0], [0.0, 0.0]],
            [[0.0, 180.0], [1200.0, 180.0]],
            [[0.0, 230.0], [1200.0, 230.0]],
        ];
        let mut x = 0.0;
        for (i, w) in [140.0, 95.0, 170.0, 120.0, 210.0, 80.0, 150.0, 115.0].iter().enumerate() {
            x += w;
            s.push([[x, 0.0], [x, 180.0]]);
            if i % 2 == 0 {
                s.push([[x, 230.0], [x, 400.0]]);
            }
            // A closet in some rooms.
            if i % 3 == 0 {
                s.push([[x - 40.0, 120.0], [x, 120.0]]);
                s.push([[x - 40.0, 120.0], [x - 40.0, 180.0]]);
            }
        }
        s
    }

    fn turned(segs: &[[[f64; 2]; 2]], f: &Sim) -> Vec<[[f64; 2]; 2]> {
        segs.iter().map(|s| [f.apply(s[0]), f.apply(s[1])]).collect()
    }

    /// The plan's pieces clipped to a box (they all run along an axis).
    fn crop(segs: &[[[f64; 2]; 2]], b: [f64; 4]) -> Vec<[[f64; 2]; 2]> {
        let clip = |v: f64, lo: f64, hi: f64| v.clamp(lo, hi);
        segs.iter()
            .filter(|s| {
                let (x0, x1) = (s[0][0].min(s[1][0]), s[0][0].max(s[1][0]));
                let (y0, y1) = (s[0][1].min(s[1][1]), s[0][1].max(s[1][1]));
                x1 >= b[0] && x0 <= b[2] && y1 >= b[1] && y0 <= b[3]
            })
            .map(|s| s.map(|p| [clip(p[0], b[0], b[2]), clip(p[1], b[1], b[3])]))
            .filter(|s| (s[0][0] - s[1][0]).hypot(s[0][1] - s[1][1]) > 1.0)
            .collect()
    }

    #[test]
    fn a_part_of_the_plan_drawn_elsewhere_and_turned_lands_where_it_came_from() {
        let plan = floor();
        let host = axis_segments(&plan, 10.0, MAX_PAGE_LINES);
        // The enlarged part: x 300..760 of the floor, turned a quarter and moved off.
        let part = crop(&plan, [300.0, -1.0, 760.0, 401.0]);
        let away = Sim { a: 0.0, b: 1.0, tx: 5_000.0, ty: -2_000.0 };
        let view = axis_segments(&turned(&part, &away), 10.0, MAX_PAGE_LINES);
        let fit = fit_by_lines(&view, &host, &[1.0], 1.5, &|_| true).expect("placed");
        assert!(fit.coverage > 0.9, "{fit:?}");
        // Back where it came from: the inverse of `away`.
        for p in [[300.0, 0.0], [760.0, 400.0], [500.0, 180.0]] {
            let q = fit.sim.apply(away.apply(p));
            assert!((q[0] - p[0]).abs() < 1.0 && (q[1] - p[1]).abs() < 1.0, "{p:?} -> {q:?}");
        }
    }

    #[test]
    fn a_view_whose_scale_note_is_a_little_off_still_lands() {
        let plan = floor();
        let host = axis_segments(&plan, 10.0, MAX_PAGE_LINES);
        // Read 2% large and moved off.
        let part = crop(&plan, [250.0, -1.0, 820.0, 401.0]);
        let off = Sim::scale_offset(1.02, [-900.0, 3_000.0]);
        let view = axis_segments(&turned(&part, &off), 10.0, MAX_PAGE_LINES);
        let fit = fit_by_lines(&view, &host, &[1.0], 2.0, &|_| true).expect("placed");
        assert!((fit.sim.scale() * 1.02 - 1.0).abs() < 0.002, "{fit:?}");
        for p in [[250.0, 0.0], [820.0, 400.0]] {
            let q = fit.sim.apply(off.apply(p));
            assert!((q[0] - p[0]).abs() < 1.5 && (q[1] - p[1]).abs() < 1.5, "{p:?} -> {q:?}");
        }
    }

    #[test]
    fn a_room_repeated_along_the_corridor_places_nowhere() {
        // Ten equal rooms: one of them fits ten places.
        let mut plan = vec![[[0.0, 0.0], [1000.0, 0.0]], [[0.0, 180.0], [1000.0, 180.0]]];
        for i in 0..=10 {
            let x = 100.0 * f64::from(i);
            plan.push([[x, 0.0], [x, 180.0]]);
            plan.push([[x + 20.0, 120.0], [x + 60.0, 120.0]]);
            plan.push([[x + 60.0, 120.0], [x + 60.0, 180.0]]);
        }
        let host = axis_segments(&plan, 10.0, MAX_PAGE_LINES);
        let part = crop(&plan, [300.0, -1.0, 400.0, 181.0]);
        let view = axis_segments(&turned(&part, &Sim::scale_offset(1.0, [3_000.0, 0.0])), 10.0, MAX_PAGE_LINES);
        assert!(fit_by_lines(&view, &host, &[1.0], 1.5, &|_| true).is_none());
        // Told where it is (its callout's box), it places.
        let near = |f: &Sim| (f.apply([3_350.0, 90.0])[0] - 350.0).abs() < 60.0;
        let fit = fit_by_lines(&view, &host, &[1.0], 1.5, &near).expect("placed by its box");
        assert!((fit.sim.tx + 3_000.0).abs() < 1.0, "{fit:?}");
    }

    #[test]
    fn a_view_of_somewhere_else_never_places() {
        let host = axis_segments(&floor(), 10.0, MAX_PAGE_LINES);
        let other: Vec<[[f64; 2]; 2]> =
            (0..12).map(|i| [[f64::from(i) * 37.0, 13.0 * f64::from(i)], [f64::from(i) * 37.0 + 55.0, 13.0 * f64::from(i)]]).collect();
        let view = axis_segments(&other, 10.0, MAX_PAGE_LINES);
        assert!(fit_by_lines(&view, &host, &[1.0], 1.5, &|_| true).is_none());
        // Hostile: nothing, NaN, no tolerance.
        assert!(fit_by_lines(&[], &host, &[1.0], 1.5, &|_| true).is_none());
        let nan = vec![[[f64::NAN, 0.0], [1.0, 0.0]]; 10];
        assert!(axis_segments(&nan, 0.5, 10).is_empty());
        assert!(fit_by_lines(&host, &host, &[1.0], f64::NAN, &|_| true).is_none());
    }

    #[test]
    fn pieces_of_one_line_are_one_piece() {
        let s = vec![[[0.0, 5.0], [40.0, 5.0]], [[40.0, 5.0], [100.0, 5.0]], [[100.0, 5.0], [130.0, 5.0]], [[0.0, 0.0], [0.0, 3.0]]];
        let a = axis_segments(&s, 10.0, 10);
        assert_eq!(a.len(), 1);
        assert!((a[0].length() - 130.0).abs() < 1e-9);
    }
}
