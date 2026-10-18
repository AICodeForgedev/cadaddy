//! Boolean operations (union, difference, intersection) on areas bounded by closed loops of
//! lines and circular arcs (polyline bulges), exact on both: arcs stay arcs.
//!
//! Every edge of both operands is split where it meets an edge of the other (crossings,
//! touching points, the ends of shared stretches), the pieces' ends are merged within a
//! tolerance, and each piece is kept when the result differs on its two sides (tested just left
//! and right of its middle, with an exact even-odd crossing count over the operand's loops). A
//! kept piece is turned so the result lies on its left; the pieces are then chained into loops,
//! taking the leftmost turn where several leave one point, so areas that only touch at a corner
//! stay apart. Shared edges need no special case: on a shared stretch the side test sees both
//! operands at once.

use std::collections::HashMap;

use crate::{PolyVertex, TAU, Vec2};

/// Which boolean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolOp {
    Union,
    /// The first area less the second.
    Difference,
    Intersection,
}

impl BoolOp {
    fn keep(self, a: bool, b: bool) -> bool {
        match self {
            BoolOp::Union => a || b,
            BoolOp::Difference => a && !b,
            BoolOp::Intersection => a && b,
        }
    }
}

/// Why a boolean was not done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolError {
    /// More edge pairs than [`MAX_PAIRS`] to compare.
    TooComplex,
}

/// Most edge pairs compared.
pub const MAX_PAIRS: usize = 50_000_000;
/// Most edges after splitting, and most steps chaining them.
const MAX_EDGES: usize = 2_000_000;

/// A line, or an arc from angle `a0` turning by `sweep` (counter-clockwise when positive).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Geom {
    Line,
    Arc { c: Vec2, r: f64, a0: f64, sweep: f64 },
}

#[derive(Clone, Copy, Debug)]
struct Edge {
    a: Vec2,
    b: Vec2,
    g: Geom,
}

impl Edge {
    /// The edge from `a` to `b` with polyline bulge `bulge` (0: a line).
    fn from_bulge(a: Vec2, b: Vec2, bulge: f64) -> Option<Edge> {
        let d = b - a;
        let len = d.len();
        if !(len.is_finite() && len > 0.0) {
            return None;
        }
        if !bulge.is_finite() || bulge.abs() < 1e-12 {
            return Some(Edge { a, b, g: Geom::Line });
        }
        let sweep = 4.0 * bulge.atan();
        let r = len * (1.0 + bulge * bulge) / (4.0 * bulge.abs());
        let c = (a + b) * 0.5 + d.perp() * ((1.0 - bulge * bulge) / (4.0 * bulge));
        let a0 = (a - c).angle();
        (r.is_finite() && c.is_finite()).then_some(Edge { a, b, g: Geom::Arc { c, r, a0, sweep } })
    }
    fn point(&self, t: f64) -> Vec2 {
        match self.g {
            Geom::Line => self.a + (self.b - self.a) * t,
            Geom::Arc { c, r, a0, sweep } => c + Vec2::from_angle(a0 + sweep * t) * r,
        }
    }
    /// The unit direction of travel at `t`.
    fn tangent(&self, t: f64) -> Vec2 {
        match self.g {
            Geom::Line => (self.b - self.a).normalized(),
            Geom::Arc { a0, sweep, .. } => {
                let n = Vec2::from_angle(a0 + sweep * t).perp();
                if sweep >= 0.0 { n } else { n * -1.0 }
            }
        }
    }
    fn length(&self) -> f64 {
        match self.g {
            Geom::Line => self.a.dist(self.b),
            Geom::Arc { r, sweep, .. } => r * sweep.abs(),
        }
    }
    /// The parameter of a point on (or next to) the edge, unclamped.
    fn param(&self, p: Vec2) -> f64 {
        match self.g {
            Geom::Line => {
                let d = self.b - self.a;
                let l2 = d.dot(d);
                if l2 > 0.0 { (p - self.a).dot(d) / l2 } else { 0.0 }
            }
            Geom::Arc { c, a0, sweep, .. } => {
                if sweep == 0.0 {
                    return 0.0;
                }
                let mut rel = ((p - c).angle() - a0) * sweep.signum();
                rel = rel.rem_euclid(TAU);
                // Just before the start reads as a little below 0, not near a full turn.
                if rel > sweep.abs() + (TAU - sweep.abs()) / 2.0 {
                    rel -= TAU;
                }
                rel / sweep.abs()
            }
        }
    }
    /// The part from `t0` to `t1`, between the given end points.
    fn part(&self, t0: f64, t1: f64, a: Vec2, b: Vec2) -> Edge {
        let g = match self.g {
            Geom::Line => Geom::Line,
            Geom::Arc { c, r, a0, sweep } => Geom::Arc { c, r, a0: a0 + sweep * t0, sweep: sweep * (t1 - t0) },
        };
        Edge { a, b, g }
    }
    fn reversed(&self) -> Edge {
        let g = match self.g {
            Geom::Line => Geom::Line,
            Geom::Arc { c, r, a0, sweep } => Geom::Arc { c, r, a0: a0 + sweep, sweep: -sweep },
        };
        Edge { a: self.b, b: self.a, g }
    }
    fn bulge(&self) -> f64 {
        match self.g {
            Geom::Line => 0.0,
            Geom::Arc { sweep, .. } => (sweep / 4.0).tan(),
        }
    }
    /// Crossings of the ray from `p` towards +X (half-open in y, so a point on a vertex counts
    /// once).
    fn crossings(&self, p: Vec2) -> usize {
        let cross = |u: Vec2, v: Vec2, x_at: &dyn Fn() -> f64| -> usize { usize::from((u.y > p.y) != (v.y > p.y) && x_at() > p.x) };
        match self.g {
            Geom::Line => {
                let (a, b) = (self.a, self.b);
                cross(a, b, &|| a.x + (p.y - a.y) * (b.x - a.x) / (b.y - a.y))
            }
            Geom::Arc { c, r, a0, sweep } => {
                // Split at the top and bottom of the circle: each piece is monotone in y and on
                // one side of the centre.
                let mut cuts = vec![0.0];
                for k in [std::f64::consts::FRAC_PI_2, 3.0 * std::f64::consts::FRAC_PI_2] {
                    let mut rel = ((k - a0) * sweep.signum()).rem_euclid(TAU);
                    while rel < sweep.abs() {
                        cuts.push(rel / sweep.abs());
                        rel += TAU;
                        if cuts.len() > 8 {
                            break;
                        }
                    }
                }
                cuts.push(1.0);
                cuts.sort_by(f64::total_cmp);
                let mut n = 0;
                for w in cuts.windows(2) {
                    let (Some(&t0), Some(&t1)) = (w.first(), w.get(1)) else { continue };
                    if t1 - t0 <= 0.0 {
                        continue;
                    }
                    // The edge's own end points where the pieces end there: the next edge starts at
                    // exactly the same point, so a ray through it counts once.
                    let at = |t: f64| {
                        if t <= 0.0 {
                            self.a
                        } else if t >= 1.0 {
                            self.b
                        } else {
                            self.point(t)
                        }
                    };
                    let (u, v) = (at(t0), at(t1));
                    let right = (a0 + sweep * (t0 + t1) / 2.0).cos() >= 0.0;
                    n += cross(u, v, &|| {
                        let dy = p.y - c.y;
                        let dx = (r * r - dy * dy).max(0.0).sqrt();
                        if right { c.x + dx } else { c.x - dx }
                    });
                }
                n
            }
        }
    }
}

/// The edges of closed loops (each vertex's bulge runs to the next).
fn edges_of(loops: &[Vec<PolyVertex>]) -> Vec<Edge> {
    let mut out = Vec::new();
    for l in loops {
        let n = l.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            let (Some(v), Some(w)) = (l.get(i), l.get((i + 1) % n)) else { continue };
            if let Some(e) = Edge::from_bulge(v.p, w.p, v.bulge) {
                out.push(e);
            }
        }
    }
    out
}

/// Inside the area of `edges` (even-odd).
fn inside(edges: &[Edge], p: Vec2) -> bool {
    edges.iter().map(|e| e.crossings(p)).sum::<usize>() % 2 == 1
}

/// Points where `e` and `f` meet: crossings, touching points and the ends of a shared stretch.
fn meets(e: &Edge, f: &Edge, tol: f64) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::new();
    let on = |x: &Edge, p: Vec2| {
        let t = x.param(p);
        (-1e-9..=1.0 + 1e-9).contains(&t) && x.point(t.clamp(0.0, 1.0)).dist(p) <= tol
    };
    // Ends of either lying on the other (shared stretches, T-junctions, touching ends).
    for p in [e.a, e.b] {
        if on(f, p) {
            out.push(p);
        }
    }
    for p in [f.a, f.b] {
        if on(e, p) {
            out.push(p);
        }
    }
    let mut add = |p: Vec2| {
        if p.is_finite() && on(e, p) && on(f, p) {
            out.push(p);
        }
    };
    match (e.g, f.g) {
        (Geom::Line, Geom::Line) => {
            let (d1, d2) = (e.b - e.a, f.b - f.a);
            let den = d1.cross(d2);
            if den.abs() > 1e-15 * d1.len() * d2.len() {
                let t = (f.a - e.a).cross(d2) / den;
                add(e.a + d1 * t);
            }
        }
        (Geom::Line, Geom::Arc { c, r, .. }) | (Geom::Arc { c, r, .. }, Geom::Line) => {
            let l = if matches!(e.g, Geom::Line) { e } else { f };
            let d = l.b - l.a;
            let (a, b, cc) = (d.dot(d), 2.0 * d.dot(l.a - c), (l.a - c).dot(l.a - c) - r * r);
            if a > 0.0 {
                let disc = b * b - 4.0 * a * cc;
                if disc >= 0.0 {
                    let s = disc.sqrt();
                    add(l.a + d * ((-b - s) / (2.0 * a)));
                    add(l.a + d * ((-b + s) / (2.0 * a)));
                } else {
                    // A tangent missed by rounding: the closest point.
                    let t = -b / (2.0 * a);
                    let q = l.a + d * t;
                    if ((q - c).len() - r).abs() <= tol {
                        add(c + (q - c).normalized() * r);
                    }
                }
            }
        }
        (Geom::Arc { c: c1, r: r1, .. }, Geom::Arc { c: c2, r: r2, .. }) => {
            let dv = c2 - c1;
            let dd = dv.len();
            if dd > tol && dd <= r1 + r2 + tol && dd >= (r1 - r2).abs() - tol {
                let x = (dd * dd + r1 * r1 - r2 * r2) / (2.0 * dd);
                let h = (r1 * r1 - x * x).max(0.0).sqrt();
                let u = dv / dd;
                let m = c1 + u * x;
                add(m + u.perp() * h);
                add(m - u.perp() * h);
            }
        }
    }
    out
}

/// Merged points: each point within `tol` of an earlier one takes its index.
struct Points {
    pts: Vec<Vec2>,
    grid: HashMap<(i64, i64), Vec<usize>>,
    cell: f64,
}

impl Points {
    fn new(tol: f64) -> Self {
        Points { pts: Vec::new(), grid: HashMap::new(), cell: tol * 4.0 }
    }
    fn key(&self, p: Vec2) -> (i64, i64) {
        ((p.x / self.cell).floor() as i64, (p.y / self.cell).floor() as i64)
    }
    fn id(&mut self, p: Vec2, tol: f64) -> usize {
        let (kx, ky) = self.key(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(ids) = self.grid.get(&(kx + dx, ky + dy)) {
                    for &i in ids {
                        if self.pts.get(i).is_some_and(|q| q.dist(p) <= tol) {
                            return i;
                        }
                    }
                }
            }
        }
        self.pts.push(p);
        let i = self.pts.len() - 1;
        self.grid.entry((kx, ky)).or_default().push(i);
        i
    }
    fn at(&self, i: usize) -> Vec2 {
        self.pts.get(i).copied().unwrap_or(Vec2::ZERO)
    }
}

/// A piece of an edge between two merged points.
#[derive(Clone, Copy, Debug)]
struct Piece {
    va: usize,
    vb: usize,
    e: Edge,
}

/// Split the edges of both operands where they meet each other (and themselves).
fn split(all: &[Edge], tol: f64, pts: &mut Points) -> Result<Vec<Piece>, BoolError> {
    if all.len().saturating_mul(all.len()) / 2 > MAX_PAIRS {
        return Err(BoolError::TooComplex);
    }
    let mut cuts: Vec<Vec<f64>> = all.iter().map(|_| vec![0.0, 1.0]).collect();
    // Bounding boxes keep the pairwise test cheap.
    let boxes: Vec<crate::Bounds2> = all
        .iter()
        .map(|e| match e.g {
            Geom::Line => crate::Bounds2::from_points([e.a, e.b]),
            Geom::Arc { c, r, .. } => crate::Bounds2::new(c - Vec2::new(r, r), c + Vec2::new(r, r)),
        })
        .collect();
    for i in 0..all.len() {
        for j in i + 1..all.len() {
            let (Some(e), Some(f), Some(bi), Some(bj)) = (all.get(i), all.get(j), boxes.get(i), boxes.get(j)) else { continue };
            if !bi.expand(tol).intersects(&bj.expand(tol)) {
                continue;
            }
            for p in meets(e, f, tol) {
                if let Some(c) = cuts.get_mut(i) {
                    c.push(e.param(p).clamp(0.0, 1.0));
                }
                if let Some(c) = cuts.get_mut(j) {
                    c.push(f.param(p).clamp(0.0, 1.0));
                }
            }
        }
    }
    let mut out = Vec::new();
    for (e, ts) in all.iter().zip(cuts.iter_mut()) {
        ts.sort_by(f64::total_cmp);
        let mut prev: Option<(f64, usize)> = None;
        for &t in ts.iter() {
            let id = pts.id(e.point(t), tol);
            if let Some((t0, v0)) = prev {
                if v0 == id {
                    continue;
                }
                let piece = e.part(t0, t, pts.at(v0), pts.at(id));
                if piece.length() > tol {
                    out.push(Piece { va: v0, vb: id, e: piece });
                    if out.len() > MAX_EDGES {
                        return Err(BoolError::TooComplex);
                    }
                }
            }
            prev = Some((t, id));
        }
    }
    Ok(out)
}

/// `a` op `b`: the areas bounded by the loops of `a` and of `b` (each operand even-odd), as
/// closed loops with bulges, the result's inside on each loop's left (outer loops
/// counter-clockwise, holes clockwise). Points closer than `tol` are one.
pub fn boolean(a: &[Vec<PolyVertex>], b: &[Vec<PolyVertex>], op: BoolOp, tol: f64) -> Result<Vec<Vec<PolyVertex>>, BoolError> {
    let (ea, eb) = (edges_of(a), edges_of(b));
    let all: Vec<Edge> = ea.iter().chain(eb.iter()).copied().collect();
    let bounds = all.iter().fold(crate::Bounds2::EMPTY, |acc, e| acc.union(&crate::Bounds2::from_points([e.a, e.b, e.point(0.5)])));
    let scale = (bounds.width() + bounds.height()).max(1e-9);
    let tol = if tol.is_finite() && tol > 0.0 { tol } else { scale * 1e-9 };
    let mut pts = Points::new(tol);
    let pieces = split(&all, tol, &mut pts)?;
    // The same piece twice (a shared edge): once is enough.
    let mut seen: HashMap<(usize, usize), Vec<Vec2>> = HashMap::new();
    let mut kept: Vec<Piece> = Vec::new();
    for p in pieces {
        let key = (p.va.min(p.vb), p.va.max(p.vb));
        let mid = p.e.point(0.5);
        let dup = seen.get(&key).is_some_and(|ms| ms.iter().any(|m| m.dist(mid) <= tol * 16.0 + p.e.length() * 1e-9));
        if dup {
            continue;
        }
        seen.entry(key).or_default().push(mid);
        // The result just left and right of the piece's middle.
        let off = (p.e.length() * 1e-3).min(scale * 1e-6).max(tol * 4.0);
        let n = p.e.tangent(0.5).perp();
        let side = |q: Vec2| op.keep(inside(&ea, q), inside(&eb, q));
        let (l, r) = (side(mid + n * off), side(mid - n * off));
        match (l, r) {
            (true, false) => kept.push(p),
            (false, true) => kept.push(Piece { va: p.vb, vb: p.va, e: p.e.reversed() }),
            _ => {}
        }
    }
    Ok(chain(&kept, &pts))
}

/// Chain pieces (the inside on their left) into closed loops, leftmost turn first.
fn chain(kept: &[Piece], pts: &Points) -> Vec<Vec<PolyVertex>> {
    let mut from: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, p) in kept.iter().enumerate() {
        from.entry(p.va).or_default().push(i);
    }
    let mut used = vec![false; kept.len()];
    let mut loops = Vec::new();
    let mut steps = 0usize;
    for start in 0..kept.len() {
        if used.get(start).copied().unwrap_or(true) {
            continue;
        }
        let mut path: Vec<usize> = Vec::new();
        let mut cur = start;
        let closed = loop {
            steps += 1;
            if steps > MAX_EDGES {
                break false;
            }
            let Some(p) = kept.get(cur) else { break false };
            if let Some(u) = used.get_mut(cur) {
                *u = true;
            }
            path.push(cur);
            let Some(first) = path.first().and_then(|i| kept.get(*i)) else { break false };
            if p.vb == first.va {
                break true;
            }
            let din = p.e.tangent(1.0);
            let next = from.get(&p.vb).into_iter().flatten().copied().filter(|j| !used.get(*j).copied().unwrap_or(true)).max_by(|x, y| {
                let turn = |j: usize| kept.get(j).map_or(f64::NEG_INFINITY, |q| din.cross(q.e.tangent(0.0)).atan2(din.dot(q.e.tangent(0.0))));
                turn(*x).total_cmp(&turn(*y))
            });
            match next {
                Some(j) => cur = j,
                None => break false,
            }
        };
        if !closed {
            continue;
        }
        let mut out: Vec<PolyVertex> =
            path.iter().filter_map(|i| kept.get(*i)).map(|p| PolyVertex { bulge: p.e.bulge(), ..PolyVertex::new(pts.at(p.va)) }).collect();
        merge(&mut out);
        if out.len() >= 2 && !(out.len() == 2 && out.iter().all(|v| v.bulge == 0.0)) {
            loops.push(out);
        }
    }
    loops
}

/// Join consecutive straight segments that run on in the same direction (pieces of one edge
/// split where the other operand touched it).
fn merge(l: &mut Vec<PolyVertex>) {
    let mut changed = true;
    while changed && l.len() > 3 {
        changed = false;
        let n = l.len();
        for i in 0..n {
            let (Some(a), Some(b), Some(c)) = (l.get(i).copied(), l.get((i + 1) % n).copied(), l.get((i + 2) % n).copied()) else { continue };
            if a.bulge != 0.0 || b.bulge != 0.0 {
                continue;
            }
            let (u, v) = (b.p - a.p, c.p - b.p);
            if u.cross(v).abs() <= 1e-12 * u.len() * v.len() && u.dot(v) > 0.0 {
                l.remove((i + 1) % n);
                changed = true;
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shoelace;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<PolyVertex> {
        [Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)].into_iter().map(PolyVertex::new).collect()
    }

    /// A circle as two half arcs.
    fn circle(c: Vec2, r: f64) -> Vec<PolyVertex> {
        vec![PolyVertex { bulge: 1.0, ..PolyVertex::new(c - Vec2::new(r, 0.0)) }, PolyVertex { bulge: 1.0, ..PolyVertex::new(c + Vec2::new(r, 0.0)) }]
    }

    /// The exact area of loops with bulges (outer loops count, holes subtract by orientation).
    fn area(loops: &[Vec<PolyVertex>]) -> f64 {
        loops
            .iter()
            .map(|l| {
                let pts: Vec<Vec2> = l.iter().map(|v| v.p).collect();
                let mut a = shoelace(&pts);
                let n = l.len();
                for i in 0..n {
                    let (v, w) = (l[i], l[(i + 1) % n]);
                    if v.bulge != 0.0 {
                        let chord = v.p.dist(w.p);
                        let th = 4.0 * v.bulge.atan();
                        let r = chord / (2.0 * (th / 2.0).sin()).abs();
                        a += 0.5 * r * r * (th - th.sin());
                    }
                }
                a
            })
            .sum()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6 * (1.0 + b.abs())
    }

    #[test]
    fn overlapping_squares() {
        let (a, b) = (vec![rect(0.0, 0.0, 2.0, 2.0)], vec![rect(1.0, 1.0, 3.0, 3.0)]);
        let u = boolean(&a, &b, BoolOp::Union, 0.0).unwrap_or_default();
        assert_eq!(u.len(), 1);
        assert!(close(area(&u), 7.0), "{}", area(&u));
        assert_eq!(u[0].len(), 8);
        let d = boolean(&a, &b, BoolOp::Difference, 0.0).unwrap_or_default();
        assert!(close(area(&d), 3.0));
        let i = boolean(&a, &b, BoolOp::Intersection, 0.0).unwrap_or_default();
        assert!(close(area(&i), 1.0));
        assert_eq!(i[0].len(), 4);
    }

    #[test]
    fn shared_edges() {
        // Side by side: the union is one rectangle (no seam), the intersection empty.
        let (a, b) = (vec![rect(0.0, 0.0, 1.0, 1.0)], vec![rect(1.0, 0.0, 2.0, 1.0)]);
        let u = boolean(&a, &b, BoolOp::Union, 0.0).unwrap_or_default();
        assert_eq!(u.len(), 1);
        assert_eq!(u[0].len(), 4, "{:?}", u[0]);
        assert!(close(area(&u), 2.0));
        assert!(boolean(&a, &b, BoolOp::Intersection, 0.0).unwrap_or_default().is_empty());
        assert!(close(area(&boolean(&a, &b, BoolOp::Difference, 0.0).unwrap_or_default()), 1.0));
        // A partly shared edge.
        let c = vec![rect(1.0, 0.5, 2.0, 2.0)];
        let u = boolean(&a, &c, BoolOp::Union, 0.0).unwrap_or_default();
        assert_eq!(u.len(), 1);
        assert!(close(area(&u), 2.5));
    }

    #[test]
    fn touching_corners_stay_apart() {
        let (a, b) = (vec![rect(0.0, 0.0, 1.0, 1.0)], vec![rect(1.0, 1.0, 2.0, 2.0)]);
        let u = boolean(&a, &b, BoolOp::Union, 0.0).unwrap_or_default();
        assert_eq!(u.len(), 2);
        assert!(u.iter().all(|l| l.len() == 4));
        assert!(close(area(&u), 2.0));
        assert!(boolean(&a, &b, BoolOp::Intersection, 0.0).unwrap_or_default().is_empty());
    }

    #[test]
    fn circles_stay_arcs() {
        let (a, b) = (vec![circle(Vec2::ZERO, 1.0)], vec![circle(Vec2::new(1.0, 0.0), 1.0)]);
        let i = boolean(&a, &b, BoolOp::Intersection, 0.0).unwrap_or_default();
        assert_eq!(i.len(), 1);
        // A lens: two arcs, no straight segments.
        assert!(i[0].iter().all(|v| v.bulge != 0.0), "{:?}", i[0]);
        let lens = 2.0 * std::f64::consts::PI / 3.0 - 3f64.sqrt() / 2.0;
        assert!(close(area(&i), lens), "{} vs {lens}", area(&i));
        let u = boolean(&a, &b, BoolOp::Union, 0.0).unwrap_or_default();
        assert!(close(area(&u), 2.0 * std::f64::consts::PI - lens));
        // A square with a circle cut from its middle: a hole, clockwise.
        let sq = vec![rect(-2.0, -2.0, 2.0, 2.0)];
        let d = boolean(&sq, &a, BoolOp::Difference, 0.0).unwrap_or_default();
        assert_eq!(d.len(), 2);
        assert!(close(area(&d), 16.0 - std::f64::consts::PI));
    }

    #[test]
    fn holes_are_kept() {
        // A square with a hole, unioned with a bar across the hole: the hole is cut in two.
        let a = vec![rect(0.0, 0.0, 6.0, 6.0), rect(2.0, 2.0, 4.0, 4.0)];
        let bar = vec![rect(1.0, 2.8, 5.0, 3.2)];
        let u = boolean(&a, &bar, BoolOp::Union, 0.0).unwrap_or_default();
        assert_eq!(u.len(), 3, "outer and two hole halves");
        assert!(close(area(&u), 36.0 - 4.0 + 0.8));
        // Subtracting something lying in the hole changes nothing.
        let inner = vec![rect(2.5, 2.5, 3.5, 3.5)];
        let d = boolean(&a, &inner, BoolOp::Difference, 0.0).unwrap_or_default();
        assert!(close(area(&d), 32.0));
        // Nested: intersecting with the hole region is empty.
        assert!(boolean(&a, &inner, BoolOp::Intersection, 0.0).unwrap_or_default().is_empty());
    }

    #[test]
    fn areas_add_up() {
        // |A ∪ B| = |A| + |B| - |A ∩ B| and |A - B| = |A| - |A ∩ B|, over shapes that overlap,
        // touch and share edges in many ways.
        let shapes: Vec<Vec<Vec<PolyVertex>>> = vec![
            vec![rect(0.0, 0.0, 3.0, 2.0)],
            vec![rect(1.0, -1.0, 2.0, 3.0)],
            vec![rect(3.0, 0.0, 4.0, 2.0)],
            vec![rect(0.5, 0.5, 2.5, 1.5)],
            vec![circle(Vec2::new(1.5, 1.0), 1.0)],
            vec![circle(Vec2::new(3.0, 1.0), 1.0)],
            vec![rect(0.0, 0.0, 3.0, 2.0), rect(1.0, 0.5, 2.0, 1.5)],
            vec![circle(Vec2::new(1.5, 1.0), 2.0), circle(Vec2::new(1.5, 1.0), 0.5)],
        ];
        for (i, a) in shapes.iter().enumerate() {
            for (j, b) in shapes.iter().enumerate() {
                let one = |op| area(&boolean(a, b, op, 0.0).unwrap_or_default());
                let (u, n, d) = (one(BoolOp::Union), one(BoolOp::Intersection), one(BoolOp::Difference));
                let (aa, ab) =
                    (area(&boolean(a, &[], BoolOp::Union, 0.0).unwrap_or_default()), area(&boolean(b, &[], BoolOp::Union, 0.0).unwrap_or_default()));
                assert!(close(u, aa + ab - n), "{i} ∪ {j}: {u} vs {aa} + {ab} - {n}");
                assert!(close(d, aa - n), "{i} - {j}: {d} vs {aa} - {n}");
                assert!(n >= -1e-9 && n <= aa.min(ab) + 1e-6, "{i} ∩ {j}: {n}");
            }
        }
    }

    #[test]
    fn concentric_and_tangent_circles() {
        let (big, small) = (vec![circle(Vec2::ZERO, 2.0)], vec![circle(Vec2::ZERO, 1.0)]);
        let ring = boolean(&big, &small, BoolOp::Difference, 0.0).unwrap_or_default();
        assert_eq!(ring.len(), 2);
        assert!(close(area(&ring), 3.0 * std::f64::consts::PI));
        // Touching from outside at one point: two loops, nothing in common.
        let side = vec![circle(Vec2::new(3.0, 0.0), 1.0)];
        assert_eq!(boolean(&big, &side, BoolOp::Union, 0.0).unwrap_or_default().len(), 2);
        assert!(boolean(&big, &side, BoolOp::Intersection, 0.0).unwrap_or_default().is_empty());
        // Touching from inside: the small circle is inside, the union is the big one.
        let inner = vec![circle(Vec2::new(1.0, 0.0), 1.0)];
        assert!(close(area(&boolean(&big, &inner, BoolOp::Union, 0.0).unwrap_or_default()), 4.0 * std::f64::consts::PI));
    }

    #[test]
    fn degenerate_input() {
        let empty: Vec<Vec<PolyVertex>> = Vec::new();
        assert!(boolean(&empty, &empty, BoolOp::Union, 0.0).unwrap_or_default().is_empty());
        let a = vec![rect(0.0, 0.0, 1.0, 1.0)];
        let u = boolean(&a, &empty, BoolOp::Union, 0.0).unwrap_or_default();
        assert!(close(area(&u), 1.0));
        // Zero-length edges, a lone point, NaN bulges and coincident copies.
        let junk = vec![vec![PolyVertex::new(Vec2::ZERO)], vec![PolyVertex::new(Vec2::ZERO), PolyVertex::new(Vec2::ZERO)], {
            let mut r = rect(0.0, 0.0, 1.0, 1.0);
            r[0].bulge = f64::NAN;
            r
        }];
        let _ = boolean(&junk, &a, BoolOp::Union, 0.0);
        let same = boolean(&a, &a, BoolOp::Union, 0.0).unwrap_or_default();
        assert!(close(area(&same), 1.0));
        assert!(boolean(&a, &a, BoolOp::Difference, 0.0).unwrap_or_default().is_empty());
    }
}
