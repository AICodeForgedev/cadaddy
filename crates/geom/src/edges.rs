//! Exact boundary edges: the lines, circular arcs, elliptical arcs and splines a hatch or region
//! loop is made of (the DXF Reference's hatch edge types 1–4), kept as curves rather than as the
//! points that approximate them. A loop's area comes from its edges exactly: Green's theorem with
//! closed forms for lines and arcs and Gauss–Legendre quadrature (exact for polynomial splines)
//! for splines.

use serde::{Deserialize, Serialize};

use crate::{Ellipse, Mat3, PolyVertex, Spline, TAU, Vec2, ccw_sweep};

/// One edge of a closed boundary loop, run from its start to its end.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LoopEdge {
    Line {
        a: Vec2,
        b: Vec2,
    },
    /// A circular arc from angle `start` to angle `end` (radians), counterclockwise when `ccw`.
    Arc {
        center: Vec2,
        radius: f64,
        start: f64,
        end: f64,
        ccw: bool,
    },
    /// An elliptical arc from parameter `start` to `end` (radians, on the ellipse with semi-major
    /// axis `major` and minor/major `ratio`), the parameter increasing when `ccw`.
    Ellipse {
        center: Vec2,
        major: Vec2,
        ratio: f64,
        start: f64,
        end: f64,
        ccw: bool,
    },
    /// A spline run over its whole parameter domain.
    Spline(Spline),
}

/// Most quadrature points one spline area takes (hostile knot vectors).
const MAX_QUADRATURE: usize = 200_000;

/// Gauss–Legendre nodes and weights on [-1, 1] for `n` points (1..=8), as the standard tables
/// give them.
#[allow(clippy::excessive_precision)]
fn gauss(n: usize) -> &'static [(f64, f64)] {
    const G2: [(f64, f64); 2] = [(-0.577_350_269_189_625_8, 1.0), (0.577_350_269_189_625_8, 1.0)];
    const G3: [(f64, f64); 3] =
        [(-0.774_596_669_241_483_4, 0.555_555_555_555_555_6), (0.0, 0.888_888_888_888_888_9), (0.774_596_669_241_483_4, 0.555_555_555_555_555_6)];
    const G4: [(f64, f64); 4] = [
        (-0.861_136_311_594_052_6, 0.347_854_845_137_453_9),
        (-0.339_981_043_584_856_3, 0.652_145_154_862_546_1),
        (0.339_981_043_584_856_3, 0.652_145_154_862_546_1),
        (0.861_136_311_594_052_6, 0.347_854_845_137_453_9),
    ];
    const G6: [(f64, f64); 6] = [
        (-0.932_469_514_203_152_1, 0.171_324_492_379_170_3),
        (-0.661_209_386_466_264_5, 0.360_761_573_048_138_6),
        (-0.238_619_186_083_196_9, 0.467_913_934_572_691_0),
        (0.238_619_186_083_196_9, 0.467_913_934_572_691_0),
        (0.661_209_386_466_264_5, 0.360_761_573_048_138_6),
        (0.932_469_514_203_152_1, 0.171_324_492_379_170_3),
    ];
    const G8: [(f64, f64); 8] = [
        (-0.960_289_856_497_536_3, 0.101_228_536_290_376_3),
        (-0.796_666_477_413_626_7, 0.222_381_034_453_374_5),
        (-0.525_532_409_916_329_0, 0.313_706_645_877_887_3),
        (-0.183_434_642_495_649_8, 0.362_683_783_378_362_0),
        (0.183_434_642_495_649_8, 0.362_683_783_378_362_0),
        (0.525_532_409_916_329_0, 0.313_706_645_877_887_3),
        (0.796_666_477_413_626_7, 0.222_381_034_453_374_5),
        (0.960_289_856_497_536_3, 0.101_228_536_290_376_3),
    ];
    match n {
        0..=2 => &G2,
        3 => &G3,
        4 => &G4,
        5 | 6 => &G6,
        _ => &G8,
    }
}

impl LoopEdge {
    /// A whole ellipse as one edge, counterclockwise.
    pub fn full_ellipse(center: Vec2, major: Vec2, ratio: f64) -> LoopEdge {
        LoopEdge::Ellipse { center, major, ratio, start: 0.0, end: TAU, ccw: true }
    }

    /// The signed sweep of an arc or elliptical arc (radians; negative clockwise).
    fn sweep(start: f64, end: f64, ccw: bool) -> f64 {
        // Equal ends: a full turn.
        let turn = |s: f64| if s < 1e-12 { TAU } else { s };
        if ccw { turn(ccw_sweep(start, end)) } else { -turn(ccw_sweep(end, start)) }
    }

    /// The point at an ellipse parameter.
    fn ellipse_at(center: Vec2, major: Vec2, ratio: f64, t: f64) -> Vec2 {
        center + major * t.cos() + major.perp() * ratio * t.sin()
    }

    pub fn start_point(&self) -> Vec2 {
        match self {
            LoopEdge::Line { a, .. } => *a,
            LoopEdge::Arc { center, radius, start, .. } => Vec2::polar(*center, *radius, *start),
            LoopEdge::Ellipse { center, major, ratio, start, .. } => Self::ellipse_at(*center, *major, *ratio, *start),
            LoopEdge::Spline(s) => s.eval(s.domain().0),
        }
    }

    pub fn end_point(&self) -> Vec2 {
        match self {
            LoopEdge::Line { b, .. } => *b,
            LoopEdge::Arc { center, radius, start, end, ccw } => Vec2::polar(*center, *radius, *start + Self::sweep(*start, *end, *ccw)),
            LoopEdge::Ellipse { center, major, ratio, start, end, ccw } => {
                Self::ellipse_at(*center, *major, *ratio, *start + Self::sweep(*start, *end, *ccw))
            }
            LoopEdge::Spline(s) => s.eval(s.domain().1),
        }
    }

    /// Whether the numbers are usable (finite, positive radii, a valid spline).
    pub fn is_valid(&self) -> bool {
        match self {
            LoopEdge::Line { a, b } => a.is_finite() && b.is_finite(),
            LoopEdge::Arc { center, radius, start, end, .. } => {
                center.is_finite() && radius.is_finite() && *radius > 0.0 && start.is_finite() && end.is_finite()
            }
            LoopEdge::Ellipse { center, major, ratio, start, end, .. } => {
                center.is_finite()
                    && major.is_finite()
                    && major.len() > 0.0
                    && ratio.is_finite()
                    && *ratio > 0.0
                    && start.is_finite()
                    && end.is_finite()
            }
            LoopEdge::Spline(s) => s.is_valid() && s.control.iter().all(|p| p.is_finite()) && s.knots.iter().all(|k| k.is_finite()),
        }
    }

    /// Points along the edge within `tol`, from its start to its end (both included).
    pub fn tessellate(&self, tol: f64, out: &mut Vec<Vec2>) {
        match self {
            LoopEdge::Line { a, b } => out.extend([*a, *b]),
            LoopEdge::Arc { center, radius, start, end, ccw } => {
                Self::Ellipse { center: *center, major: Vec2::X * *radius, ratio: 1.0, start: *start, end: *end, ccw: *ccw }.tessellate(tol, out)
            }
            LoopEdge::Ellipse { center, major, ratio, start, end, ccw } => {
                let sweep = Self::sweep(*start, *end, *ccw);
                let mut pts = Vec::new();
                Ellipse { center: *center, major: *major, ratio: *ratio, start: 0.0, end: sweep.abs() }.tessellate(tol, &mut pts);
                // Re-run along the edge's own direction from its start parameter.
                let n = pts.len().saturating_sub(1).max(1);
                out.extend((0..=n).map(|i| Self::ellipse_at(*center, *major, *ratio, *start + sweep * i as f64 / n as f64)));
            }
            LoopEdge::Spline(s) => out.extend(s.tessellate(tol)),
        }
    }

    /// The edge as polyline vertices (without its end point): a line its start, a circular arc its
    /// start with the arc's bulge (a full circle as two halves), other curves points within `tol`.
    pub fn vertices(&self, tol: f64, out: &mut Vec<PolyVertex>) {
        match self {
            LoopEdge::Line { a, .. } => out.push(PolyVertex::new(*a)),
            LoopEdge::Arc { center, radius, start, end, ccw } => {
                let sweep = Self::sweep(*start, *end, *ccw);
                if (sweep.abs() - TAU).abs() < 1e-9 {
                    let b = crate::arc_to_bulge(sweep / 2.0);
                    out.push(PolyVertex::with_bulge(Vec2::polar(*center, *radius, *start), b));
                    out.push(PolyVertex::with_bulge(Vec2::polar(*center, *radius, *start + sweep / 2.0), b));
                } else {
                    out.push(PolyVertex::with_bulge(Vec2::polar(*center, *radius, *start), crate::arc_to_bulge(sweep)));
                }
            }
            _ => {
                let mut pts = Vec::new();
                self.tessellate(tol, &mut pts);
                pts.pop();
                out.extend(pts.into_iter().map(PolyVertex::new));
            }
        }
    }

    /// ∮ (x dy − y dx) along the edge: twice its contribution to the enclosed signed area.
    pub fn area2(&self) -> f64 {
        match self {
            LoopEdge::Line { a, b } => a.cross(*b),
            LoopEdge::Arc { center, radius, start, end, ccw } => {
                Self::Ellipse { center: *center, major: Vec2::X * *radius, ratio: 1.0, start: *start, end: *end, ccw: *ccw }.area2()
            }
            LoopEdge::Ellipse { center, major, ratio, start, end, ccw } => {
                // P(t) = c + M cos t + N sin t: P × P' = c × (−M sin t + N cos t) + M × N.
                let n = major.perp() * *ratio;
                let (t0, t1) = (*start, *start + Self::sweep(*start, *end, *ccw));
                center.cross(*major * (t1.cos() - t0.cos()) + n * (t1.sin() - t0.sin())) + major.cross(n) * (t1 - t0)
            }
            LoopEdge::Spline(s) => spline_area2(s),
        }
    }

    /// Transform the edge by `m`. An arc stays an arc under rotations, mirrors and uniform scales
    /// and becomes an elliptical arc otherwise; ellipses and splines stay exact under any affine
    /// map.
    pub fn transform(&mut self, m: &Mat3) {
        let mirrored = m.det() < 0.0;
        *self = match std::mem::replace(self, LoopEdge::Line { a: Vec2::ZERO, b: Vec2::ZERO }) {
            LoopEdge::Line { a, b } => LoopEdge::Line { a: m.apply(a), b: m.apply(b) },
            LoopEdge::Arc { center, radius, start, end, ccw } => {
                let (u, v) = (m.apply_vec(Vec2::X), m.apply_vec(Vec2::Y));
                let similar = (u.len() - v.len()).abs() <= 1e-9 * u.len().max(1e-300) && u.dot(v).abs() <= 1e-9 * u.len() * v.len();
                if similar {
                    let c = m.apply(center);
                    let r = radius * u.len();
                    let ang = |a: f64| (m.apply(Vec2::polar(center, radius, a)) - c).angle();
                    LoopEdge::Arc { center: c, radius: r, start: ang(start), end: ang(start + Self::sweep(start, end, ccw)), ccw: ccw != mirrored }
                } else {
                    let mut e = LoopEdge::Ellipse { center, major: Vec2::X * radius, ratio: 1.0, start, end, ccw };
                    e.transform(m);
                    e
                }
            }
            LoopEdge::Ellipse { center, major, ratio, start, end, ccw } => {
                // P(t) = c + U cos t + V sin t after the map, U and V conjugate: the principal
                // axes are at the parameter shift t0 that makes them perpendicular.
                let (u, v) = (m.apply_vec(major), m.apply_vec(major.perp() * ratio));
                let t0 = 0.5 * (2.0 * u.dot(v)).atan2(u.dot(u) - v.dot(v));
                let (a, b) = (u * t0.cos() + v * t0.sin(), -u * t0.sin() + v * t0.cos());
                let (major2, minor2, shift) = if a.len() >= b.len() { (a, b, t0) } else { (b, -a, t0 + TAU / 4.0) };
                let ratio2 = if major2.len() > 0.0 { minor2.len() / major2.len() } else { 1.0 };
                // On the new axes the parameter runs t − shift, backwards when the minor axis
                // turned clockwise from the major.
                let flip = major2.cross(minor2) < 0.0;
                let sweep = Self::sweep(start, end, ccw);
                let map_t = |t: f64| if flip { -(t - shift) } else { t - shift };
                let s2 = map_t(start);
                let e2 = map_t(start + sweep);
                LoopEdge::Ellipse { center: m.apply(center), major: major2, ratio: ratio2.min(1.0), start: s2, end: e2, ccw: (sweep > 0.0) != flip }
            }
            LoopEdge::Spline(mut s) => {
                for p in &mut s.control {
                    *p = m.apply(*p);
                }
                for p in &mut s.fit {
                    *p = m.apply(*p);
                }
                LoopEdge::Spline(s)
            }
        };
    }
}

/// ∮ (x dy − y dx) along a spline: Gauss–Legendre on each knot span with the analytic
/// derivative (exact for non-rational splines: the integrand is a polynomial of degree 2p − 1).
fn spline_area2(s: &Spline) -> f64 {
    if !s.is_valid() {
        return 0.0;
    }
    let (lo, hi) = s.domain();
    let rule = gauss(if s.weights.is_empty() { s.degree + 1 } else { 8 });
    let mut spans: Vec<f64> = s.knots.iter().copied().filter(|k| *k >= lo && *k <= hi).collect();
    spans.dedup_by(|a, b| (*a - *b).abs() < 1e-15);
    let mut total = 0.0;
    let mut used = 0usize;
    for w in spans.windows(2) {
        let (Some(&a), Some(&b)) = (w.first(), w.get(1)) else { continue };
        let (mid, half) = ((a + b) / 2.0, (b - a) / 2.0);
        for (x, wt) in rule {
            used += 1;
            if used > MAX_QUADRATURE {
                return total;
            }
            let t = mid + half * x;
            total += wt * half * s.eval(t).cross(s.derivative(t));
        }
    }
    total
}

/// The signed area of a closed loop of edges (positive counterclockwise).
pub fn loop_area(edges: &[LoopEdge]) -> f64 {
    edges.iter().map(LoopEdge::area2).sum::<f64>() / 2.0
}

/// The loop's polyline vertices: lines and arcs exactly (bulges), other curves within `tol`.
pub fn loop_vertices(edges: &[LoopEdge], tol: f64) -> Vec<PolyVertex> {
    let mut out = Vec::new();
    for e in edges {
        e.vertices(tol, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PI;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol * (1.0 + a.abs().max(b.abs()))
    }

    #[test]
    fn full_ellipse_area_is_exact() {
        let e = LoopEdge::full_ellipse(Vec2::new(3.0, -2.0), Vec2::new(4.0, 3.0), 0.4);
        // a = 5, b = 2: πab.
        assert!(
            close(loop_area(&[e]), PI * 5.0 * 2.0, 1e-14),
            "{}",
            loop_area(&[LoopEdge::full_ellipse(Vec2::new(3.0, -2.0), Vec2::new(4.0, 3.0), 0.4)])
        );
        let cw = LoopEdge::Ellipse { center: Vec2::ZERO, major: Vec2::X * 5.0, ratio: 0.4, start: 0.0, end: 0.0, ccw: false };
        assert!(close(loop_area(&[cw]), -PI * 10.0, 1e-14), "clockwise is negative");
    }

    #[test]
    fn half_ellipse_closed_by_its_diameter() {
        let half = LoopEdge::Ellipse { center: Vec2::ZERO, major: Vec2::X * 5.0, ratio: 0.5, start: 0.0, end: PI, ccw: true };
        let back = LoopEdge::Line { a: Vec2::new(-5.0, 0.0), b: Vec2::new(5.0, 0.0) };
        assert!(close(loop_area(&[half.clone(), back]), PI * 5.0 * 2.5 / 2.0, 1e-14));
        assert!(half.end_point().near(Vec2::new(-5.0, 0.0), 1e-12));
    }

    #[test]
    fn arcs_lines_and_bulge_vertices() {
        // A half disc of radius 2 on the x axis.
        let arc = LoopEdge::Arc { center: Vec2::ZERO, radius: 2.0, start: 0.0, end: PI, ccw: true };
        let line = LoopEdge::Line { a: Vec2::new(-2.0, 0.0), b: Vec2::new(2.0, 0.0) };
        let edges = [arc, line];
        assert!(close(loop_area(&edges), PI * 2.0, 1e-14));
        let vs = loop_vertices(&edges, 1e-3);
        assert_eq!(vs.len(), 2);
        let pl = crate::Polyline { vertices: vs, closed: true };
        assert!(close(pl.area(), PI * 2.0, 1e-12), "the bulge polyline has the same area");
    }

    #[test]
    fn polynomial_spline_area_is_exact() {
        // The cubic Bezier (0,0),(0,4),(4,4),(4,0) closed by the x axis: x = 12t² − 8t³,
        // y = 12t(1 − t), so the area is ∫y dx = 288 ∫t²(1 − t)² dt = 9.6.
        let s = Spline {
            degree: 3,
            knots: vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
            control: vec![Vec2::ZERO, Vec2::new(0.0, 4.0), Vec2::new(4.0, 4.0), Vec2::new(4.0, 0.0)],
            ..Spline::default()
        };
        let edges = [LoopEdge::Spline(s.clone()), LoopEdge::Line { a: Vec2::new(4.0, 0.0), b: Vec2::ZERO }];
        assert!(close(loop_area(&edges), -9.6, 1e-13), "clockwise: {}", loop_area(&edges));
        // A rational quarter-circle spline: numerical, but close.
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let q = Spline {
            degree: 2,
            knots: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            control: vec![Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)],
            weights: vec![1.0, w, 1.0],
            ..Spline::default()
        };
        let edges = [LoopEdge::Line { a: Vec2::ZERO, b: Vec2::X }, LoopEdge::Spline(q), LoopEdge::Line { a: Vec2::Y, b: Vec2::ZERO }];
        assert!(close(loop_area(&edges), PI / 4.0, 1e-8), "{}", loop_area(&edges));
    }

    #[test]
    fn transforms_keep_curves_exact() {
        let e = LoopEdge::full_ellipse(Vec2::new(1.0, 1.0), Vec2::new(3.0, 0.0), 0.5);
        let a0 = loop_area(std::slice::from_ref(&e));
        // Rotate + scale 2: area × 4.
        let mut r = e.clone();
        r.transform(&Mat3::rotate(0.7).then_before(Mat3::scale(2.0, 2.0)));
        assert!(close(loop_area(&[r]), a0 * 4.0, 1e-12));
        // Mirror: the area keeps its size, its sign flips (the loop now runs clockwise).
        let mut mm = e.clone();
        mm.transform(&Mat3::mirror(Vec2::ZERO, Vec2::Y));
        assert!(close(loop_area(&[mm]), -a0, 1e-12));
        // A non-uniform scale (an arc becomes an elliptical arc): area × sx × sy.
        let disc = LoopEdge::Arc { center: Vec2::ZERO, radius: 1.0, start: 0.0, end: 0.0, ccw: true };
        let mut s = disc.clone();
        s.transform(&Mat3::scale(3.0, 0.5));
        assert!(matches!(s, LoopEdge::Ellipse { .. }));
        assert!(close(loop_area(&[s]), PI * 1.5, 1e-12));
        // A shear: area unchanged, still one exact ellipse.
        let mut sh = e.clone();
        sh.transform(&Mat3 { a: 1.0, b: 0.0, c: 0.8, d: 1.0, e: 0.0, f: 0.0 });
        assert!(close(loop_area(&[sh.clone()]), a0, 1e-12), "{} vs {a0}", loop_area(&[sh]));
    }

    #[test]
    fn transformed_partial_arcs_keep_their_ends() {
        let half = LoopEdge::Ellipse { center: Vec2::ZERO, major: Vec2::X * 5.0, ratio: 0.5, start: 0.3, end: 2.0, ccw: true };
        for m in [
            Mat3::rotate(1.1),
            Mat3::mirror(Vec2::ZERO, Vec2::new(1.0, 2.0)),
            Mat3::scale(2.0, 0.7),
            Mat3 { a: 1.0, b: 0.3, c: 0.8, d: 1.0, e: 4.0, f: -1.0 },
        ] {
            let mut t = half.clone();
            t.transform(&m);
            assert!(t.start_point().near(m.apply(half.start_point()), 1e-9), "{m:?}: {:?}", t.start_point());
            assert!(t.end_point().near(m.apply(half.end_point()), 1e-9), "{m:?}: {:?}", t.end_point());
        }
    }

    #[test]
    fn hostile_edges_never_panic() {
        let bad = [
            LoopEdge::Arc { center: Vec2::new(f64::NAN, 0.0), radius: -1.0, start: 0.0, end: 1.0, ccw: true },
            LoopEdge::Ellipse { center: Vec2::ZERO, major: Vec2::ZERO, ratio: f64::INFINITY, start: 0.0, end: 1.0, ccw: false },
            LoopEdge::Spline(Spline { degree: 9, knots: vec![0.0], control: vec![Vec2::ZERO], ..Spline::default() }),
        ];
        for e in &bad {
            assert!(!e.is_valid());
            let _ = e.area2();
            let mut pts = Vec::new();
            e.tessellate(1e-3, &mut pts);
            let mut t = e.clone();
            t.transform(&Mat3::scale(0.0, 1.0));
        }
    }
}
