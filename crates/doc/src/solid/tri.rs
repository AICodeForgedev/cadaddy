//! Triangles for planar faces with holes: holes are joined to their outer boundary by a bridge,
//! then ears are clipped. Indices refer to the loops' points numbered one loop after another.
//!
//! Implements ear clipping with hole bridging after Eberly, "Triangulation by Ear Clipping"
//! (Geometric Tools, 2002).

use cadaddy_geom::{Vec2, point_in_polygon};

/// Most points a face is triangulated from; beyond that, each loop is fanned.
const MAX_POINTS: usize = 20_000;

fn area2(p: &[Vec2], idx: &[usize]) -> f64 {
    let n = idx.len();
    (0..n).map(|i| p[idx[i]].cross(p[idx[(i + 1) % n]])).sum()
}

/// Triangles covering the region the loops bound (even-odd: loops inside an odd number of
/// others are holes), counter-clockwise.
pub fn triangulate(loops: &[Vec<Vec2>]) -> Vec<[usize; 3]> {
    let mut pts: Vec<Vec2> = Vec::new();
    let mut rings: Vec<Vec<usize>> = Vec::new();
    for l in loops {
        let base = pts.len();
        pts.extend(l.iter().copied());
        if l.len() >= 3 {
            rings.push((base..base + l.len()).collect());
        }
    }
    if pts.len() > MAX_POINTS || pts.iter().any(|p| !p.is_finite()) {
        return rings.iter().flat_map(|r| (1..r.len().saturating_sub(1)).map(|k| [r[0], r[k], r[k + 1]])).collect();
    }
    let polys: Vec<Vec<Vec2>> = rings.iter().map(|r| r.iter().map(|&i| pts[i]).collect()).collect();
    let depth: Vec<usize> =
        rings.iter().enumerate().map(|(i, r)| polys.iter().enumerate().filter(|(j, o)| *j != i && point_in_polygon(o, pts[r[0]])).count()).collect();
    let mut out = Vec::new();
    for (i, r) in rings.iter().enumerate() {
        if depth[i] % 2 == 1 {
            continue;
        }
        let mut outer = r.clone();
        if area2(&pts, &outer) < 0.0 {
            outer.reverse();
        }
        // Its holes: one level deeper and inside it.
        let mut holes: Vec<Vec<usize>> = rings
            .iter()
            .enumerate()
            .filter(|(j, h)| depth[*j] == depth[i] + 1 && point_in_polygon(&polys[i], pts[h[0]]))
            .map(|(_, h)| {
                let mut h = h.clone();
                if area2(&pts, &h) > 0.0 {
                    h.reverse();
                }
                h
            })
            .collect();
        holes.sort_by(|a, b| {
            let mx = |h: &Vec<usize>| h.iter().map(|&k| pts[k].x).fold(f64::MIN, f64::max);
            mx(b).total_cmp(&mx(a))
        });
        for h in holes {
            outer = bridge(&pts, outer, &h);
        }
        clip_ears(&pts, outer, &mut out);
    }
    out
}

/// The outer ring with `hole` joined in by a bridge from the hole's rightmost point to a point
/// of the ring it can see.
fn bridge(p: &[Vec2], outer: Vec<usize>, hole: &[usize]) -> Vec<usize> {
    let Some((hk, &m)) = hole.iter().enumerate().max_by(|a, b| p[*a.1].x.total_cmp(&p[*b.1].x)) else { return outer };
    let pm = p[m];
    // The nearest crossing of the ray to +X with the ring.
    let n = outer.len();
    let mut best: Option<(f64, usize)> = None;
    for i in 0..n {
        let (a, b) = (p[outer[i]], p[outer[(i + 1) % n]]);
        if (a.y > pm.y) == (b.y > pm.y) && a.y != pm.y && b.y != pm.y {
            continue;
        }
        if (b.y - a.y).abs() < 1e-300 {
            continue;
        }
        let t = (pm.y - a.y) / (b.y - a.y);
        if !(0.0..=1.0).contains(&t) {
            continue;
        }
        let x = a.x + t * (b.x - a.x);
        if x >= pm.x - 1e-12 && best.is_none_or(|(bx, _)| x < bx) {
            // The edge's end further right is the candidate.
            let k = if a.x > b.x { i } else { (i + 1) % n };
            best = Some((x, k));
        }
    }
    let Some((ix, mut k)) = best else { return outer };
    let cand = p[outer[k]];
    // A reflex point inside the triangle (M, I, P) would block the bridge: take the one at the
    // smallest angle instead.
    let tri = [pm, Vec2::new(ix, pm.y), cand];
    let mut best_ang = f64::MAX;
    for (j, &v) in outer.iter().enumerate() {
        let q = p[v];
        let prev = p[outer[(j + n - 1) % n]];
        let next = p[outer[(j + 1) % n]];
        let reflex = (q - prev).cross(next - q) <= 0.0;
        if j == k || !reflex || q.x < pm.x || !in_triangle(q, tri) {
            continue;
        }
        let ang = (q - pm).y.abs().atan2((q - pm).x);
        if ang < best_ang {
            best_ang = ang;
            k = j;
        }
    }
    let mut ring = Vec::with_capacity(n + hole.len() + 2);
    ring.extend_from_slice(&outer[..=k]);
    for t in 0..=hole.len() {
        ring.push(hole[(hk + t) % hole.len()]);
    }
    ring.push(outer[k]);
    ring.extend_from_slice(&outer[k + 1..]);
    ring
}

fn in_triangle(q: Vec2, t: [Vec2; 3]) -> bool {
    let d = |a: Vec2, b: Vec2| (b - a).cross(q - a);
    let (d1, d2, d3) = (d(t[0], t[1]), d(t[1], t[2]), d(t[2], t[0]));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

fn clip_ears(p: &[Vec2], mut ring: Vec<usize>, out: &mut Vec<[usize; 3]>) {
    let mut guard = ring.len() * ring.len() + 16;
    while ring.len() > 3 && guard > 0 {
        guard -= 1;
        let n = ring.len();
        let mut clipped = false;
        for i in 0..n {
            let (a, b, c) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
            let (pa, pb, pc) = (p[a], p[b], p[c]);
            if (pb - pa).cross(pc - pb) <= 0.0 {
                continue;
            }
            let blocked = ring.iter().any(|&v| {
                let q = p[v];
                v != a && v != b && v != c && q != pa && q != pb && q != pc && in_triangle(q, [pa, pb, pc])
            });
            if !blocked {
                out.push([a, b, c]);
                ring.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            // Nothing clean left (a degenerate ring): drop the flattest corner.
            let n = ring.len();
            let i = (0..n)
                .max_by(|&i, &j| {
                    let turn = |i: usize| (p[ring[i]] - p[ring[(i + n - 1) % n]]).cross(p[ring[(i + 1) % n]] - p[ring[i]]);
                    turn(i).total_cmp(&turn(j))
                })
                .unwrap_or(0);
            let (a, b, c) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
            if (p[b] - p[a]).cross(p[c] - p[b]) > 0.0 {
                out.push([a, b, c]);
            }
            ring.remove(i);
        }
    }
    if let [a, b, c] = ring[..]
        && (p[b] - p[a]).cross(p[c] - p[b]) > 0.0
    {
        out.push([a, b, c]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(loops: &[Vec<Vec2>], t: &[[usize; 3]]) -> f64 {
        let pts: Vec<Vec2> = loops.iter().flatten().copied().collect();
        t.iter().map(|[a, b, c]| (pts[*b] - pts[*a]).cross(pts[*c] - pts[*a]) / 2.0).sum()
    }

    #[test]
    fn holes_and_concave_outlines() {
        let sq = |a: f64, b: f64| vec![Vec2::new(a, a), Vec2::new(b, a), Vec2::new(b, b), Vec2::new(a, b)];
        // A square with two holes, one wound the wrong way.
        let mut h2 = vec![Vec2::new(6.0, 6.0), Vec2::new(8.0, 6.0), Vec2::new(8.0, 8.0), Vec2::new(6.0, 8.0)];
        h2.reverse();
        let loops = vec![sq(0.0, 10.0), sq(1.0, 3.0), h2];
        let t = triangulate(&loops);
        assert!((area(&loops, &t) - (100.0 - 4.0 - 4.0)).abs() < 1e-9, "{}", area(&loops, &t));
        // An L, clockwise.
        let mut l =
            vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0), Vec2::new(4.0, 1.0), Vec2::new(1.0, 1.0), Vec2::new(1.0, 4.0), Vec2::new(0.0, 4.0)];
        l.reverse();
        let t = triangulate(&[l.clone()]);
        assert!((area(&[l], &t) - 7.0).abs() < 1e-9);
        // An island inside a hole is a region of its own.
        let loops = vec![sq(0.0, 10.0), sq(2.0, 8.0), sq(4.0, 6.0)];
        assert!((area(&loops, &triangulate(&loops)) - (100.0 - 36.0 + 4.0)).abs() < 1e-9);
    }
}
