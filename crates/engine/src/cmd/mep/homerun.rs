//! Tubing run home from a manifold. An enlarged plan often draws each fixture's water as its own
//! tube from a manifold (PEX/polyethylene "home runs"), in a patterned linestyle (a zigzag or a
//! chain of diamonds), and tags the tubes to a fixture with both their systems at once
//! (`1/2" HW & CW TO LAVATORY`). Read line by line, a tube is its pattern's strokes, far longer
//! than the tube, and every tube under such a tag takes its first system. Here:
//! - a run drawn in a pattern is measured along its middle ([`centreline`]);
//! - a tag's leader takes the tag to what its arrow points at ([`leader_tip`]);
//! - a tube under a two-system tag takes the system of the manifold it comes from, the one a
//!   single-system tag points at ([`resolve_pairs`]); one that reaches no known manifold is
//!   shared out between the tag's two systems, as the tag says each fixture gets one of each.

use super::twinline;
use cadaddy_geom::Vec2;
use std::collections::HashMap;

/// A step of a pattern's strokes is under this (feet): no pipe drawing's own segments are.
const PATTERN_STEP_FT: f64 = 1.0 / 24.0;
/// The fewest points a patterned run has.
const PATTERN_MIN_POINTS: usize = 12;
/// The share of a patterned run's turns that turn back the other way from the last.
const PATTERN_ZIGZAG_SHARE: f64 = 0.5;
/// Points each side averaged into the middle line.
const SMOOTH_HALF: usize = 4;
/// The middle line is kept to within this (feet).
const SIMPLIFY_FT: f64 = 1.0 / 48.0;
/// A leader's arrowhead is at most this many text heights across.
const ARROW_MAX_H: f64 = 3.0;
/// A leader is this long at least and at most, in text heights.
const LEADER_MIN_H: f64 = 1.5;
const LEADER_MAX_H: f64 = 80.0;
/// Most lines and polylines followed along one leader.
const LEADER_HOPS: usize = 3;
/// Ends of tubes this close (feet) are at one manifold's ports.
const PORT_GAP_FT: f64 = 0.4;
/// A manifold has at least this many tubes.
const MIN_PORTS: usize = 4;
/// A single-system tag points at a manifold within this (feet) of its ports.
const MANIFOLD_TAG_FT: f64 = 2.5;
/// Two manifolds this close (feet) are a pair: no two of one system.
const MANIFOLD_PAIR_FT: f64 = 4.0;
/// Most tube ends clustered on one sheet.
const MAX_ENDS: usize = 100_000;

/// The middle line of a run drawn in a pattern (its strokes a small zigzag about it), or `None`
/// when the run is drawn plain (its segments longer than a pattern's, or turning one way, as a
/// curve's do).
pub(crate) fn centreline(pts: &[Vec2], upf: f64) -> Option<Vec<Vec2>> {
    if pts.len() < PATTERN_MIN_POINTS || !(upf.is_finite() && upf > 0.0) {
        return None;
    }
    let mut steps: Vec<f64> = pts.windows(2).filter_map(|w| Some(w.first()?.dist(*w.get(1)?))).collect();
    steps.sort_by(f64::total_cmp);
    let median = *steps.get(steps.len() / 2)?;
    if !(median.is_finite() && median > 0.0 && median < PATTERN_STEP_FT * upf) {
        return None;
    }
    let turns: Vec<f64> = pts
        .windows(3)
        .filter_map(|w| match w {
            [a, b, c] => Some((*b - *a).cross(*c - *b)),
            _ => None,
        })
        .filter(|t| t.abs() > 1e-12)
        .collect();
    let back = turns.windows(2).filter(|w| matches!(w, [a, b] if a.signum() != b.signum())).count();
    if turns.len() < 2 || (back as f64) < PATTERN_ZIGZAG_SHARE * (turns.len() - 1) as f64 {
        return None;
    }
    let n = pts.len();
    let mid: Vec<Vec2> = (0..n)
        .map(|i| {
            let (lo, hi) = (i.saturating_sub(SMOOTH_HALF), (i + SMOOTH_HALF + 1).min(n));
            let w = pts.get(lo..hi).unwrap_or_default();
            let k = w.len().max(1) as f64;
            w.iter().fold(Vec2::new(0.0, 0.0), |s, p| s + *p) * (1.0 / k)
        })
        .collect();
    let out = twinline::simplify(mid, SIMPLIFY_FT * upf);
    (out.len() >= 2).then_some(out)
}

/// A line or polyline that may be a leader: its points.
pub(crate) type Stroke = Vec<Vec2>;

/// Where the leader of a tag at `at` (`h` high, `chars` long) points: the far end of a line or
/// polyline that starts beside the tag's first line (either end of it) and ends in an arrowhead
/// (one of `arrows`, as `[min, max]` boxes). `None` when the tag has no such leader.
pub(crate) fn leader_tip(at: Vec2, h: f64, chars: usize, strokes: &[Stroke], arrows: &[[Vec2; 2]]) -> Option<Vec2> {
    if !(h.is_finite() && h > 0.0 && at.is_finite()) {
        return None;
    }
    // The tag's first line, left, centred or right of `at` (its justification is not known here).
    let w = 0.9 * h * chars as f64;
    let beside = |p: Vec2| p.y >= at.y - h && p.y <= at.y + 2.0 * h && [at.x - w, at.x, at.x + w].iter().any(|x| (p.x - x).abs() <= 2.0 * h);
    let len = |s: &Stroke| s.windows(2).filter_map(|w| Some(w.first()?.dist(*w.get(1)?))).sum::<f64>();
    let arrow_at = |p: Vec2| {
        arrows.iter().any(|[lo, hi]| {
            (hi.x - lo.x).max(hi.y - lo.y) <= ARROW_MAX_H * h
                && p.x >= lo.x - 0.5 * h
                && p.x <= hi.x + 0.5 * h
                && p.y >= lo.y - 0.5 * h
                && p.y <= hi.y + 0.5 * h
        })
    };
    let mut best: Option<(f64, Vec2)> = None;
    for s in strokes {
        let (Some(&a), Some(&b)) = (s.first(), s.last()) else { continue };
        for (start, far) in [(a, b), (b, a)] {
            if !beside(start) {
                continue;
            }
            // Follow lines joined end to end from the far end.
            let (mut tip, mut total, mut from) = (far, len(s), start);
            for _ in 0..LEADER_HOPS {
                if arrow_at(tip) || total > LEADER_MAX_H * h {
                    break;
                }
                let next = strokes.iter().find_map(|t| {
                    let (Some(&c), Some(&d)) = (t.first(), t.last()) else { return None };
                    if c.dist(tip) <= 0.1 * h && d.dist(from) > 0.1 * h {
                        Some((d, len(t)))
                    } else if d.dist(tip) <= 0.1 * h && c.dist(from) > 0.1 * h {
                        Some((c, len(t)))
                    } else {
                        None
                    }
                });
                let Some((d, l)) = next.filter(|(d, _)| d.dist(tip) > 0.1 * h) else { break };
                from = tip;
                tip = d;
                total += l;
            }
            if total >= LEADER_MIN_H * h && total <= LEADER_MAX_H * h && arrow_at(tip) {
                let d = start.dist(at);
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((d, tip));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// A tag as [`resolve_pairs`] reads it: where it is (or points), its system and its second
/// system when it names two.
#[derive(Clone, Debug)]
pub(crate) struct Tag {
    pub at: Vec2,
    pub system: &'static str,
    pub also: Option<&'static str>,
}

/// The system of each run whose own tag (`own`, an index into `tags`) names two systems, and of
/// each run tagged by a manifold's own tag that comes from that manifold: `(system, from its
/// manifold)`. A run that comes from a manifold takes the manifold's system; a manifold is where
/// [`MIN_PORTS`] or more tagged runs end together, and its system is the one a single-system tag
/// points at nearest it (two manifolds side by side are one of each). A two-system run from no
/// known manifold is given the one of its tag's two systems that has the less length so far under
/// that tag, longest first.
pub(crate) fn resolve_pairs(paths: &[Vec<Vec2>], own: &[Option<usize>], tags: &[Tag], upf: f64) -> Vec<Option<(&'static str, bool)>> {
    let mut out: Vec<Option<(&'static str, bool)>> = vec![None; paths.len()];
    if !tags.iter().any(|t| t.also.is_some()) || !(upf.is_finite() && upf > 0.0) {
        return out;
    }
    // Tagged runs' ends, clustered where they meet.
    let mut ends: Vec<(usize, Vec2)> = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        if own.get(i).copied().flatten().is_none() {
            continue;
        }
        if let (Some(&a), Some(&b)) = (p.first(), p.last()) {
            ends.push((i, a));
            ends.push((i, b));
        }
        if ends.len() >= MAX_ENDS {
            break;
        }
    }
    let gap = PORT_GAP_FT * upf;
    let key = |p: Vec2| ((p.x / gap).floor() as i64, (p.y / gap).floor() as i64);
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (k, (_, p)) in ends.iter().enumerate() {
        if p.is_finite() {
            grid.entry(key(*p)).or_default().push(k);
        }
    }
    let mut parent: Vec<usize> = (0..ends.len()).collect();
    fn root(parent: &mut [usize], mut k: usize) -> usize {
        while let Some(&p) = parent.get(k) {
            if p == k {
                break;
            }
            let gp = parent.get(p).copied().unwrap_or(p);
            if let Some(slot) = parent.get_mut(k) {
                *slot = gp;
            }
            k = p;
        }
        k
    }
    for (k, (_, p)) in ends.iter().enumerate() {
        if !p.is_finite() {
            continue;
        }
        let (cx, cy) = key(*p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &j in grid.get(&(cx + dx, cy + dy)).map(Vec::as_slice).unwrap_or_default() {
                    if j > k && ends.get(j).is_some_and(|(_, q)| q.dist(*p) <= gap) {
                        let (a, b) = (root(&mut parent, k), root(&mut parent, j));
                        if a != b
                            && let Some(slot) = parent.get_mut(a)
                        {
                            *slot = b;
                        }
                    }
                }
            }
        }
    }
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for k in 0..ends.len() {
        let r = root(&mut parent, k);
        members.entry(r).or_default().push(k);
    }
    // Manifolds: enough distinct runs, at least one of them under a two-system tag.
    let tag_of = |i: usize| own.get(i).copied().flatten().and_then(|t| tags.get(t));
    let mut manifolds: Vec<(usize, Vec<usize>, Vec<&'static str>)> = Vec::new();
    for (r, ks) in &members {
        let mut runs: Vec<usize> = ks.iter().filter_map(|k| ends.get(*k).map(|(i, _)| *i)).collect();
        runs.sort_unstable();
        runs.dedup();
        let mut cands: Vec<&'static str> = runs.iter().filter_map(|i| tag_of(*i)).filter_map(|t| t.also.map(|b| [t.system, b])).flatten().collect();
        cands.sort_unstable();
        cands.dedup();
        if runs.len() >= MIN_PORTS && !cands.is_empty() {
            manifolds.push((*r, ks.clone(), cands));
        }
    }
    manifolds.sort_by_key(|(r, ..)| *r);
    let near = |ks: &[usize], p: Vec2| ks.iter().filter_map(|k| ends.get(*k)).map(|(_, q)| q.dist(p)).fold(f64::INFINITY, f64::min);
    // Single-system tags pointing at a manifold, nearest first.
    let mut offers: Vec<(f64, usize, &'static str)> = Vec::new();
    for (m, (_, ks, cands)) in manifolds.iter().enumerate() {
        for t in tags.iter().filter(|t| t.also.is_none() && cands.contains(&t.system)) {
            let d = near(ks, t.at);
            if d <= MANIFOLD_TAG_FT * upf {
                offers.push((d, m, t.system));
            }
        }
    }
    offers.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(b.2)));
    let mut system: Vec<Option<&'static str>> = vec![None; manifolds.len()];
    for (_, m, sys) in offers {
        if system.get(m).is_some_and(Option::is_some) {
            continue;
        }
        let Some((_, ks, _)) = manifolds.get(m) else { continue };
        let taken = manifolds.iter().enumerate().any(|(o, (_, ko, _))| {
            o != m
                && system.get(o).copied().flatten() == Some(sys)
                && ko.iter().filter_map(|k| ends.get(*k)).any(|(_, q)| near(ks, *q) <= MANIFOLD_PAIR_FT * upf)
        });
        if !taken && let Some(slot) = system.get_mut(m) {
            *slot = Some(sys);
        }
    }
    // A manifold's tag: a single-system tag within reach of a manifold's ports.
    let manifold_tag: Vec<bool> =
        tags.iter().map(|t| t.also.is_none() && manifolds.iter().any(|(_, ks, _)| near(ks, t.at) <= MANIFOLD_TAG_FT * upf)).collect();
    // Each run from a known manifold.
    let mut from: HashMap<usize, &'static str> = HashMap::new();
    for ((_, ks, _), sys) in manifolds.iter().zip(&system) {
        let Some(sys) = sys else { continue };
        for (i, _) in ks.iter().filter_map(|k| ends.get(*k)) {
            from.insert(*i, sys);
        }
    }
    let length = |p: &[Vec2]| p.windows(2).filter_map(|w| Some(w.first()?.dist(*w.get(1)?))).sum::<f64>();
    let mut shared: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut under: HashMap<(usize, &'static str), f64> = HashMap::new();
    for (i, p) in paths.iter().enumerate() {
        let Some(ti) = own.get(i).copied().flatten() else { continue };
        let Some(t) = tags.get(ti) else { continue };
        let pair = t.also.map(|b| [t.system, b]);
        let fits = |s: &str| pair.is_none_or(|[a, b]| s == a || s == b);
        match from.get(&i) {
            Some(s) if (pair.is_some() || manifold_tag.get(ti).copied().unwrap_or(false)) && fits(s) => {
                if let Some(slot) = out.get_mut(i) {
                    *slot = Some((s, true));
                }
                *under.entry((ti, s)).or_default() += length(p);
            }
            _ if pair.is_some() => shared.entry(ti).or_default().push(i),
            _ => {}
        }
    }
    let mut keys: Vec<usize> = shared.keys().copied().collect();
    keys.sort_unstable();
    for ti in keys {
        let (Some(t), Some(runs)) = (tags.get(ti), shared.get(&ti)) else { continue };
        let Some(b) = t.also else { continue };
        let mut runs = runs.clone();
        runs.sort_by(|x, y| {
            let (lx, ly) = (paths.get(*x).map_or(0.0, |p| length(p)), paths.get(*y).map_or(0.0, |p| length(p)));
            ly.total_cmp(&lx).then(x.cmp(y))
        });
        for i in runs {
            let (la, lb) = (under.get(&(ti, t.system)).copied().unwrap_or(0.0), under.get(&(ti, b)).copied().unwrap_or(0.0));
            let s = if lb < la { b } else { t.system };
            *under.entry((ti, s)).or_default() += paths.get(i).map_or(0.0, |p| length(p));
            if let Some(slot) = out.get_mut(i) {
                *slot = Some((s, false));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    /// A zigzag about the line from `a` to `b`, `amp` either side, a stroke every `step`.
    fn zigzag(a: Vec2, b: Vec2, step: f64, amp: f64) -> Vec<Vec2> {
        let d = b - a;
        let l = d.len();
        let (u, n) = (d * (1.0 / l), Vec2::new(-d.y / l, d.x / l));
        let k = (l / step) as usize;
        (0..=k).map(|i| a + u * (i as f64 * step) + n * if i % 2 == 0 { amp } else { -amp }).collect()
    }

    #[test]
    fn a_tube_drawn_as_a_zigzag_is_measured_along_its_middle() {
        // Paper points at 1/4" = 1'-0" (18 a foot): a 10' tube drawn as a zigzag 1/4 pt a stroke.
        let upf = 18.0;
        let z = zigzag(v(0.0, 0.0), v(180.0, 0.0), 0.25, 0.12);
        let zig_len: f64 = z.windows(2).map(|w| w[0].dist(w[1])).sum();
        assert!(zig_len / upf > 13.0, "the strokes are much longer than the tube: {zig_len}");
        let mid = centreline(&z, upf).expect("a patterned run");
        let len: f64 = mid.windows(2).map(|w| w[0].dist(w[1])).sum();
        assert!((len / upf - 10.0).abs() < 0.2, "measured along its middle: {} ft", len / upf);
        assert!(mid.iter().all(|p| p.y.abs() < 0.2), "on the tube's line");
        assert!(mid.len() < 10, "kept to the few points the line needs: {}", mid.len());
        // Turned, at another scale (1/8"), the same.
        let z = zigzag(v(5.0, 5.0), v(5.0 + 63.6, 5.0 + 63.6), 0.2, 0.1);
        let mid = centreline(&z, 9.0).expect("a patterned run");
        let len: f64 = mid.windows(2).map(|w| w[0].dist(w[1])).sum();
        assert!((len / 9.0 - 10.0).abs() < 0.2, "{}", len / 9.0);
    }

    #[test]
    fn a_plain_line_or_a_smooth_curve_is_not_a_pattern() {
        let upf = 18.0;
        // A straight run in long segments.
        let plain: Vec<Vec2> = (0..20).map(|i| v(i as f64 * 9.0, 0.0)).collect();
        assert!(centreline(&plain, upf).is_none());
        // An arc in fine steps, turning one way.
        let arc: Vec<Vec2> = (0..200).map(|i| (i as f64 * 0.005_f64).sin_cos()).map(|(s, c)| v(50.0 * c, 50.0 * s)).collect();
        assert!(centreline(&arc, upf).is_none(), "a curve's fine steps turn one way");
        // Too few points, and hostile values.
        assert!(centreline(&zigzag(v(0.0, 0.0), v(2.0, 0.0), 0.25, 0.1), upf).is_none());
        assert!(centreline(&[v(f64::NAN, 0.0); 30], upf).is_none());
        assert!(centreline(&zigzag(v(0.0, 0.0), v(180.0, 0.0), 0.25, 0.12), f64::NAN).is_none());
        assert!(centreline(&zigzag(v(0.0, 0.0), v(180.0, 0.0), 0.25, 0.12), 0.0).is_none());
    }

    #[test]
    fn a_tag_points_where_its_leader_arrow_is() {
        let h = 4.5;
        let at = v(100.0, 100.0);
        // Shoulder left of the tag, then down to an arrowhead.
        let leader = vec![v(95.0, 102.0), v(85.0, 102.0), v(60.0, 70.0)];
        let arrow = [v(58.0, 68.0), v(62.0, 73.0)];
        assert_eq!(leader_tip(at, h, 9, std::slice::from_ref(&leader), &[arrow]), Some(v(60.0, 70.0)));
        // From the tag's right end (9 characters), in two lines joined end to end.
        let a = vec![v(100.0 + 0.9 * h * 9.0 + 3.0, 102.0), v(160.0, 102.0)];
        let b = vec![v(160.0, 102.0), v(170.0, 130.0)];
        let arrow2 = [v(168.0, 128.0), v(172.0, 132.0)];
        assert_eq!(leader_tip(at, h, 9, &[a.clone(), b.clone()], &[arrow2]), Some(v(170.0, 130.0)));
        // No arrowhead: no leader (a pipe passing the tag is not one).
        assert_eq!(leader_tip(at, h, 9, &[a, b], &[]), None);
        // A line far from the tag is not its leader; an arrowhead too big is not one.
        assert_eq!(leader_tip(at, h, 9, &[vec![v(300.0, 100.0), v(260.0, 70.0)]], &[[v(258.0, 68.0), v(262.0, 72.0)]]), None);
        assert_eq!(leader_tip(at, h, 9, &[leader.clone()], &[[v(40.0, 50.0), v(80.0, 90.0)]]), None);
        // Hostile values.
        assert_eq!(leader_tip(v(f64::NAN, 0.0), h, 9, &[leader.clone()], &[arrow]), None);
        assert_eq!(leader_tip(at, 0.0, 9, &[leader], &[arrow]), None);
    }

    /// Tubes from a manifold at `port` (`n` ports 2" apart, 18 units a foot), each to `to`.
    fn home_runs(port: Vec2, n: usize, to: &[Vec2]) -> Vec<Vec<Vec2>> {
        (0..n).map(|k| vec![port + v(k as f64 * 3.0, 0.0), *to.get(k).unwrap_or(&v(0.0, 0.0))]).collect()
    }

    #[test]
    fn each_tube_takes_the_system_of_the_manifold_it_comes_from() {
        let upf = 18.0;
        // Two manifolds 2' apart; fixtures below. Hot (left) feeds two sinks, a lav and a tub;
        // cold (right) the same four and the water closet.
        let fixtures = [v(0.0, -150.0), v(60.0, -160.0), v(-80.0, -120.0), v(-40.0, -170.0), v(120.0, -140.0)];
        let mut paths = home_runs(v(0.0, 0.0), 4, &fixtures);
        paths.extend(home_runs(v(36.0, 0.0), 5, &fixtures));
        // Tags: "HW & CW" at the sinks, lav and tub; "CW" at the water closet; the manifolds'
        // tags point just over each manifold.
        let tags = vec![
            Tag { at: v(0.0, -150.0), system: "HW", also: Some("CW") },
            Tag { at: v(60.0, -160.0), system: "HW", also: Some("CW") },
            Tag { at: v(-80.0, -120.0), system: "HW", also: Some("CW") },
            Tag { at: v(-40.0, -170.0), system: "HW", also: Some("CW") },
            Tag { at: v(120.0, -140.0), system: "CW", also: None },
            Tag { at: v(2.0, 20.0), system: "HW", also: None },
            Tag { at: v(44.0, 18.0), system: "CW", also: None },
        ];
        // Each tube's own tag is the one at its fixture.
        let own = vec![Some(0), Some(1), Some(2), Some(3), Some(0), Some(1), Some(2), Some(3), Some(4)];
        let got = resolve_pairs(&paths, &own, &tags, upf);
        let sys: Vec<Option<&str>> = got.iter().map(|g| g.map(|(s, _)| s)).collect();
        let (hot, cold) = (Some("HW"), Some("CW"));
        assert_eq!(sys, vec![hot, hot, hot, hot, cold, cold, cold, cold, None], "{got:?}");
        assert!(got.iter().flatten().all(|(_, m)| *m), "all from their manifold");
        // The manifolds' tags nearer the other way round: still one of each, nearest first.
        let mut swapped = tags.clone();
        swapped[5].system = "CW";
        swapped[6].system = "HW";
        let got = resolve_pairs(&paths, &own, &swapped, upf);
        let sys: Vec<Option<&str>> = got.iter().map(|g| g.map(|(s, _)| s)).collect();
        assert_eq!(sys, vec![cold, cold, cold, cold, hot, hot, hot, hot, None]);
        // A tube from a manifold whose own tag is the manifold's takes the manifold's system.
        let mut own2 = own.clone();
        own2[0] = Some(6);
        let got = resolve_pairs(&paths, &own2, &tags, upf);
        assert_eq!(got[0], Some(("HW", true)), "{got:?}");
    }

    #[test]
    fn tubes_under_a_two_system_tag_from_no_known_manifold_are_shared_between_its_systems() {
        let upf = 18.0;
        // Two pieces of tube, each a pair, under one "HW & CW" tag; no manifold drawn.
        let paths = vec![
            vec![v(0.0, 0.0), v(100.0, 0.0)],
            vec![v(0.0, 3.0), v(100.0, 3.0)],
            vec![v(0.0, 40.0), v(50.0, 40.0)],
            vec![v(0.0, 43.0), v(50.0, 43.0)],
        ];
        let tags = vec![Tag { at: v(50.0, 20.0), system: "HW", also: Some("CW") }];
        let own = vec![Some(0); 4];
        let got = resolve_pairs(&paths, &own, &tags, upf);
        let sys: Vec<Option<&str>> = got.iter().map(|g| g.map(|(s, _)| s)).collect();
        assert_eq!(sys, vec![Some("HW"), Some("CW"), Some("HW"), Some("CW")]);
        assert!(got.iter().flatten().all(|(_, m)| !*m));
        // No two-system tag: nothing to resolve. Hostile scale: nothing.
        let single = vec![Tag { at: v(50.0, 20.0), system: "HW", also: None }];
        assert!(resolve_pairs(&paths, &own, &single, upf).iter().all(Option::is_none));
        assert!(resolve_pairs(&paths, &own, &tags, f64::NAN).iter().all(Option::is_none));
        let nan = vec![vec![v(f64::NAN, 0.0), v(f64::INFINITY, 1.0)]; 6];
        assert_eq!(resolve_pairs(&nan, &[Some(0); 6], &tags, upf).len(), 6);
    }
}
