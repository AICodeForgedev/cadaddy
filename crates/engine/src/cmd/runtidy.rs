//! Tidy the centrelines the print readers trace (Q6022): what an import's geometry noise puts
//! into a run (a jog of a few degrees where two line pieces meet, a short hook where the trace
//! turns back, a bend drawn as many short pieces) is not a fitting. A run read off a sheet
//! comes out with straight runs between real corners, and a turn back on itself splits it.

use cadaddy_geom::Vec2;
use cadaddy_walk3d::MepRun;

/// A turn under this many degrees is a straight run (a joint between two pieces of one line).
pub(crate) const STRAIGHT_DEG: f64 = 15.0;
/// A turn over this many degrees is no elbow: the trace turns back on itself.
pub(crate) const HAIRPIN_DEG: f64 = 135.0;
/// The most points one run keeps.
const MAX_POINTS: usize = 10_000;

/// The turn at `b` from `a→b` to `b→c`, degrees (0 straight on, 180 straight back).
fn turn_deg(a: Vec2, b: Vec2, c: Vec2) -> f64 {
    let (u, v) = (b - a, c - b);
    let (lu, lv) = (u.len(), v.len());
    if lu <= 1e-12 || lv <= 1e-12 {
        return 0.0;
    }
    (u.dot(v) / (lu * lv)).clamp(-1.0, 1.0).acos().to_degrees()
}

/// Which way the turn at `b` goes: +1 left, -1 right, 0 straight.
fn side(a: Vec2, b: Vec2, c: Vec2) -> f64 {
    let x = (b - a).cross(c - b);
    if x.abs() <= 1e-12 { 0.0 } else { x.signum() }
}

/// Where the lines through `a→b` and `c→d` cross, if they are not parallel.
fn cross_point(a: Vec2, b: Vec2, c: Vec2, d: Vec2) -> Option<Vec2> {
    let (r, s) = (b - a, d - c);
    let den = r.cross(s);
    if den.abs() <= 1e-12 * r.len().max(1e-12) * s.len().max(1e-12) {
        return None;
    }
    let t = (c - a).cross(s) / den;
    let p = a + r * t;
    p.is_finite().then_some(p)
}

/// Path length of `pts`.
fn length(pts: &[Vec2]) -> f64 {
    pts.windows(2).map(|w| w[0].dist(w[1])).sum()
}

/// How far `p` is from the segment `a..b`.
fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    if l2 <= 1e-18 {
        return p.dist(a);
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.dist(a + ab * t)
}

/// Drop points closer than `tol` to the one kept before them (the run's last point stays).
fn drop_close(pts: &mut Vec<Vec2>, tol: f64) {
    let mut out: Vec<Vec2> = Vec::with_capacity(pts.len());
    for &p in pts.iter() {
        match out.last() {
            Some(q) if q.dist(p) < tol => {}
            _ => out.push(p),
        }
    }
    if let (Some(&last), Some(&end)) = (out.last(), pts.last())
        && last.dist(end) > 1e-12
    {
        // The end moved in by less than `tol`: keep the real end.
        if out.len() >= 2 {
            out.pop();
        }
        out.push(end);
    }
    *pts = out;
}

/// Take out every inner point the path turns less than [`STRAIGHT_DEG`] at, smallest turn
/// first, so a long straight run drawn in pieces is one straight run.
fn straighten(pts: &mut Vec<Vec2>) {
    while pts.len() > 2 {
        let mut best: Option<(usize, f64)> = None;
        for i in 1..pts.len() - 1 {
            let (Some(&a), Some(&b), Some(&c)) = (pts.get(i - 1), pts.get(i), pts.get(i + 1)) else { continue };
            let t = turn_deg(a, b, c);
            if t < STRAIGHT_DEG && best.is_none_or(|(_, bt)| t < bt) {
                best = Some((i, t));
            }
        }
        let Some((i, _)) = best else { break };
        pts.remove(i);
    }
}

/// A bend drawn as several short pieces turning the same way (a radius elbow traced as a
/// polyline) is one corner: where the lines of the pieces before and after it cross.
fn join_bends(pts: &mut Vec<Vec2>, bend: f64) {
    let mut i = 1;
    while i + 1 < pts.len() {
        // The corners i..=j with every piece between them shorter than `bend`, turning one way.
        let (Some(&a), Some(&b), Some(&c)) = (pts.get(i - 1), pts.get(i), pts.get(i + 1)) else { break };
        let way = side(a, b, c);
        let mut j = i;
        while j + 2 < pts.len() {
            let (Some(&p), Some(&q), Some(&r), Some(&s)) = (pts.get(j - 1), pts.get(j), pts.get(j + 1), pts.get(j + 2)) else { break };
            let _ = p;
            if q.dist(r) >= bend || side(q, r, s) != way || way == 0.0 {
                break;
            }
            j += 1;
        }
        if j > i {
            let (Some(&a), Some(&b), Some(&c), Some(&d)) = (pts.get(i - 1), pts.get(i), pts.get(j), pts.get(j + 1)) else { break };
            let total = turn_deg(a, b, b + (d - c));
            let at = cross_point(a, b, c, d).filter(|p| p.dist(b) <= bend * 4.0 && p.dist(c) <= bend * 4.0);
            if let Some(p) = at
                && total < HAIRPIN_DEG
            {
                pts.splice(i..=j, std::iter::once(p));
            }
        }
        i += 1;
    }
}

/// Split the path where it turns back on itself (more than [`HAIRPIN_DEG`]). A leg that only
/// retraces the one before it (within `width`) is the trace doubling back and goes.
fn split_hairpins(pts: Vec<Vec2>, width: f64) -> Vec<Vec<Vec2>> {
    let mut out: Vec<Vec<Vec2>> = Vec::new();
    let mut cur: Vec<Vec2> = Vec::new();
    for (k, &p) in pts.iter().enumerate() {
        let next = pts.get(k + 1).copied();
        cur.push(p);
        let hairpin = match (cur.len().checked_sub(2).and_then(|i| cur.get(i)), next) {
            (Some(&a), Some(c)) => turn_deg(a, p, c) > HAIRPIN_DEG,
            _ => false,
        };
        if hairpin {
            let done = std::mem::take(&mut cur);
            cur.push(p);
            out.push(done);
        }
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    // Drop a leg that lies along a longer neighbour (the trace going over the same duct).
    let mut keep = vec![true; out.len()];
    for i in 0..out.len() {
        for j in [i.wrapping_sub(1), i + 1] {
            let (Some(a), Some(b)) = (out.get(i), out.get(j)) else { continue };
            if keep.get(j) != Some(&true) || length(a) > length(b) {
                continue;
            }
            let along = a.iter().all(|p| b.windows(2).any(|w| seg_dist(*p, w[0], w[1]) <= width));
            if along {
                if let Some(k) = keep.get_mut(i) {
                    *k = false;
                }
                break;
            }
        }
    }
    out.into_iter().zip(keep).filter_map(|(p, k)| k.then_some(p)).collect()
}

/// Tidy one read run (`upf` drawing units per foot): runs at one height only; one that
/// changes height keeps its points (its corners are drops and risers).
pub(crate) fn tidy_run(run: &MepRun, upf: f64) -> Vec<MepRun> {
    let upf = if upf.is_finite() && upf > 0.0 { upf } else { 12.0 };
    let level = run.bottoms_ft.first().copied().unwrap_or(0.0);
    if run.points.len() < 3 || run.bottoms_ft.iter().any(|b| (b - level).abs() > 1e-6) || run.points.len() > MAX_POINTS {
        return vec![run.clone()];
    }
    let mut pts: Vec<Vec2> = run.points.iter().map(|p| Vec2::new(p[0], p[1])).filter(|p| p.is_finite()).collect();
    let width = run.size_in[0].max(run.size_in[1]).max(4.0) / 12.0 * upf;
    // Pieces under half a foot (or the duct's width) are not a segment of their own.
    let tol = (0.5 * upf).max(width * 0.5);
    drop_close(&mut pts, tol);
    straighten(&mut pts);
    join_bends(&mut pts, (2.0 * upf).max(width * 1.5));
    straighten(&mut pts);
    let parts = split_hairpins(pts, width);
    let min_len = tol;
    parts
        .into_iter()
        .filter(|p| p.len() >= 2 && length(p) >= min_len)
        .map(|p| {
            let mut r = run.clone();
            r.bottoms_ft = vec![level; p.len().saturating_sub(1).max(1)];
            r.points = p.iter().map(|v| [v.x, v.y]).collect();
            r
        })
        .collect()
}

/// [`tidy_run`] on every run of a reading.
pub(crate) fn tidy_runs(runs: Vec<MepRun>, upf: f64) -> Vec<MepRun> {
    runs.iter().flat_map(|r| tidy_run(r, upf)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(points: &[[f64; 2]]) -> MepRun {
        MepRun {
            system: "SA".into(),
            level: "Level 1".into(),
            points: points.to_vec(),
            size_in: [12.0, 10.0],
            bottoms_ft: vec![10.0; points.len() - 1],
            objects: Vec::new(),
            id: String::new(),
            encased: false,
            size_source: "tag".into(),
            height_source: "tag".into(),
            reviewed: false,
            edited_by: String::new(),
            top_ft: None,
            source_sheet: "M-101".into(),
        }
    }

    fn turns(r: &MepRun) -> Vec<f64> {
        r.points.windows(3).map(|w| turn_deg(Vec2::new(w[0][0], w[0][1]), Vec2::new(w[1][0], w[1][1]), Vec2::new(w[2][0], w[2][1]))).collect()
    }

    #[test]
    fn a_jog_of_a_few_degrees_is_a_straight_run() {
        // 40 ft east with a 10° kink halfway (inches).
        let k = 240.0 * 10f64.to_radians().tan();
        let out = tidy_run(&run(&[[0.0, 0.0], [240.0, 0.0], [480.0, k]]), 12.0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].points.len(), 2, "{:?}", out[0].points);
        assert_eq!(out[0].bottoms_ft.len(), 1);
    }

    #[test]
    fn real_corners_stay() {
        let out = tidy_run(&run(&[[0.0, 0.0], [240.0, 0.0], [240.0, 240.0], [480.0, 480.0]]), 12.0);
        assert_eq!(out.len(), 1);
        let t = turns(&out[0]);
        assert_eq!(t.len(), 2, "{:?}", out[0].points);
        assert!((t[0] - 90.0).abs() < 1e-6 && (t[1] - 45.0).abs() < 1e-6, "{t:?}");
    }

    #[test]
    fn a_short_piece_between_two_lines_goes() {
        // A 2" stub where two pieces meet makes no corner of its own.
        let out = tidy_run(&run(&[[0.0, 0.0], [240.0, 0.0], [241.0, 2.0], [480.0, 0.0]]), 12.0);
        assert_eq!(out[0].points.len(), 2, "{:?}", out[0].points);
    }

    #[test]
    fn a_bend_traced_in_pieces_is_one_corner() {
        // East, then a 90° bend of six 15° pieces of 6", then north.
        let mut pts = vec![[0.0, 0.0], [240.0, 0.0]];
        let (mut p, mut a) = ([240.0, 0.0], 0.0f64);
        for _ in 0..6 {
            a += 15.0;
            p = [p[0] + 9.0 * a.to_radians().cos(), p[1] + 9.0 * a.to_radians().sin()];
            pts.push(p);
        }
        pts.push([p[0], p[1] + 240.0]);
        let out = tidy_run(&run(&pts), 12.0);
        assert_eq!(out.len(), 1);
        let t = turns(&out[0]);
        assert_eq!(t.len(), 1, "{:?}", out[0].points);
        assert!((t[0] - 90.0).abs() < 1.0, "{t:?}");
    }

    #[test]
    fn a_turn_back_splits_the_run_and_a_retrace_goes() {
        // 40 ft east, then 30 ft back west 2 ft north: two ducts side by side, no 175° elbow.
        let out = tidy_run(&run(&[[0.0, 0.0], [480.0, 0.0], [120.0, 24.0]]), 12.0);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out.iter().all(|r| r.points.len() == 2));
        // 40 ft east, then back 10 ft over the same duct: the retrace is the trace, not a duct.
        let out = tidy_run(&run(&[[0.0, 0.0], [480.0, 0.0], [360.0, 2.0]]), 12.0);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].points, vec![[0.0, 0.0], [480.0, 0.0]]);
    }

    #[test]
    fn a_run_that_changes_height_keeps_its_points() {
        let mut r = run(&[[0.0, 0.0], [240.0, 0.0], [480.0, 5.0]]);
        r.bottoms_ft = vec![10.0, 9.0];
        let out = tidy_run(&r, 12.0);
        assert_eq!(out, vec![r]);
    }
}
