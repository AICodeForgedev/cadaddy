//! Exact-to-tolerance points on ellipses and splines.
//!
//! Intersections and closest points involving an ellipse or a spline start from a tessellation
//! (fast and robust: it finds every candidate), which is only as accurate as its chords. These
//! functions polish such an approximate answer with Newton's method on the curves' own
//! equations, so a snapped intersection or a trim point lies on both curves to about 1e-12 of
//! the drawing's size instead of 1e-3 of the curve's.

use crate::{Ellipse, Line, Spline, Vec2};

/// A curve as an equation in one parameter: what [`refine_intersection`] and
/// [`refine_closest`] work on. Lines and circles are infinite and full (the caller has already
/// chosen the candidate on the finite piece).
#[derive(Clone, Copy, Debug)]
pub enum CurveRef<'a> {
    Line(Line),
    Circle {
        center: Vec2,
        radius: f64,
    },
    /// The whole ellipse (parameters are not limited to the arc's sweep).
    Ellipse(Ellipse),
    Spline(&'a Spline),
}

impl CurveRef<'_> {
    pub fn eval(&self, t: f64) -> Vec2 {
        match self {
            CurveRef::Line(l) => l.a + (l.b - l.a) * t,
            CurveRef::Circle { center, radius } => *center + Vec2::new(t.cos(), t.sin()) * *radius,
            CurveRef::Ellipse(e) => e.at_param(t),
            CurveRef::Spline(s) => s.eval(t),
        }
    }

    pub fn d1(&self, t: f64) -> Vec2 {
        match self {
            CurveRef::Line(l) => l.b - l.a,
            CurveRef::Circle { radius, .. } => Vec2::new(-t.sin(), t.cos()) * *radius,
            CurveRef::Ellipse(e) => e.derivative(t),
            CurveRef::Spline(s) => s.derivative(t),
        }
    }

    fn d2(&self, t: f64) -> Vec2 {
        match self {
            CurveRef::Line(_) => Vec2::ZERO,
            CurveRef::Circle { radius, .. } => Vec2::new(-t.cos(), -t.sin()) * *radius,
            CurveRef::Ellipse(e) => (e.at_param(t) - e.center) * -1.0,
            CurveRef::Spline(s) => {
                let (lo, hi) = s.domain();
                let h = ((hi - lo) * 1e-6).max(1e-12);
                let (a, b) = ((t - h).max(lo), (t + h).min(hi));
                if b - a > 0.0 { (s.derivative(b) - s.derivative(a)) / (b - a) } else { Vec2::ZERO }
            }
        }
    }

    /// Keep a spline's parameter on its domain (the other curves are unbounded or periodic).
    fn clamp(&self, t: f64) -> f64 {
        match self {
            CurveRef::Spline(s) => {
                let (lo, hi) = s.domain();
                t.clamp(lo, hi)
            }
            _ => t,
        }
    }

    /// The curve's own size (tolerances scale with it).
    fn size(&self) -> f64 {
        match self {
            CurveRef::Line(l) => l.len(),
            CurveRef::Circle { radius, .. } => *radius,
            CurveRef::Ellipse(e) => e.major.len(),
            CurveRef::Spline(s) => {
                let b = s.bounds();
                b.width().hypot(b.height())
            }
        }
    }

    /// How far from the origin it lies: floating point can't resolve finer than this allows.
    fn magnitude(&self) -> f64 {
        match self {
            CurveRef::Line(l) => l.a.len().max(l.b.len()),
            CurveRef::Circle { center, radius } => center.len() + *radius,
            CurveRef::Ellipse(e) => e.center.len() + e.major.len(),
            CurveRef::Spline(s) => s.control.iter().map(|c| c.len()).fold(0.0, f64::max),
        }
    }

    /// The parameter of the point on the curve nearest `p`, to seed Newton: exact for lines,
    /// circles and points on an ellipse; the nearest of a dense sample on a spline.
    fn seed(&self, p: Vec2) -> f64 {
        match self {
            CurveRef::Line(l) => {
                let d = l.b - l.a;
                let len2 = d.len2();
                if len2 > 0.0 { (p - l.a).dot(d) / len2 } else { 0.0 }
            }
            CurveRef::Circle { center, .. } => center.angle_to(p),
            CurveRef::Ellipse(e) => e.param_of(p),
            CurveRef::Spline(s) => {
                let (lo, hi) = s.domain();
                let spans = s.knots.windows(2).filter(|w| w.first() < w.get(1)).count().max(1);
                let n = (spans * 24).clamp(64, 4096);
                let mut best = (f64::INFINITY, lo);
                for i in 0..=n {
                    let t = lo + (hi - lo) * i as f64 / n as f64;
                    let d = s.eval(t).dist(p);
                    if d < best.0 {
                        best = (d, t);
                    }
                }
                best.1
            }
        }
    }
}

/// The point on curve `c` nearest `p`, starting from parameter `t0` (or a seed when `None`).
/// Returns the point and its parameter. Newton on the perpendicularity condition
/// (C(t) - p) . C'(t) = 0, keeping only steps that bring the point closer.
pub fn refine_closest(c: &CurveRef, p: Vec2, t0: Option<f64>) -> (Vec2, f64) {
    let mut t = c.clamp(t0.unwrap_or_else(|| c.seed(p)));
    let mut best = c.eval(t).dist(p);
    for _ in 0..40 {
        let q = c.eval(t);
        let d1 = c.d1(t);
        let d2 = c.d2(t);
        let g = (q - p).dot(d1);
        let gp = d1.len2() + (q - p).dot(d2);
        if !(g.is_finite() && gp.is_finite()) || gp.abs() < 1e-300 {
            break;
        }
        let mut step = -g / gp;
        // Damp: halve the step until the point gets no farther away.
        let mut moved = false;
        for _ in 0..30 {
            let tn = c.clamp(t + step);
            let dn = c.eval(tn).dist(p);
            if dn <= best {
                moved = (tn - t).abs() > 0.0;
                t = tn;
                best = dn;
                break;
            }
            step *= 0.5;
        }
        if !moved || step.abs() < 1e-15 * t.abs().max(1.0) {
            break;
        }
    }
    (c.eval(t), t)
}

/// Polish an approximate intersection `p0` of curves `a` and `b`. Newton on A(s) - B(t) = 0;
/// near a tangency, where that system is singular, alternating closest-point projections
/// instead. Returns `None` when the curves don't meet within tolerance near `p0`, or when the
/// polished point wanders off (more than `reach` from `p0`, so it can't jump to another
/// intersection).
pub fn refine_intersection(a: &CurveRef, b: &CurveRef, p0: Vec2, reach: f64) -> Option<Vec2> {
    if !p0.is_finite() {
        return None;
    }
    let size = a.size().max(b.size());
    let ulp = 16.0 * f64::EPSILON * a.magnitude().max(b.magnitude());
    // Converged: as exact as the curves' size and their place in floating point allow.
    let tol = (1e-13 * size).max(ulp);
    // Accepted: still on both curves to well under a millionth of their size.
    let accept = (1e-9 * size).max(64.0 * ulp);
    let (mut s, mut t) = (a.clamp(a.seed(p0)), b.clamp(b.seed(p0)));
    // A spline seed is the nearest of a sample: start Newton from the nearest point itself.
    s = refine_closest(a, p0, Some(s)).1;
    t = refine_closest(b, p0, Some(t)).1;
    let gap = |s: f64, t: f64| (a.eval(s) - b.eval(t)).len();
    let mut err = gap(s, t);
    for _ in 0..60 {
        if err <= tol {
            break;
        }
        let f = a.eval(s) - b.eval(t);
        let (da, db) = (a.d1(s), b.d1(t));
        // Solve [da, -db] [ds, dt]^T = -f.
        let det = da.cross(db * -1.0);
        let lens = da.len() * db.len();
        let (mut ds, mut dt) = if det.abs() > 1e-9 * lens && lens > 0.0 {
            (-f.cross(db * -1.0) / det, -da.cross(f) / det)
        } else {
            // Nearly tangent: project each onto the other.
            let (_, s2) = refine_closest(a, b.eval(t), Some(s));
            let (_, t2) = refine_closest(b, a.eval(s2), Some(t));
            (s2 - s, t2 - t)
        };
        let mut improved = false;
        for _ in 0..30 {
            let (sn, tn) = (a.clamp(s + ds), b.clamp(t + dt));
            let en = gap(sn, tn);
            if en < err {
                s = sn;
                t = tn;
                err = en;
                improved = true;
                break;
            }
            ds *= 0.5;
            dt *= 0.5;
        }
        if !improved {
            break;
        }
    }
    let p = (a.eval(s) + b.eval(t)) * 0.5;
    (err <= accept && p.dist(p0) <= reach && p.is_finite()).then_some(p)
}

/// Curve `c` offset by `dist` (positive: to the left of its direction of travel) over
/// parameters `t0..t1`, as a cubic spline through exact offset points. Points are added until the
/// spline stays within `tol` of the true offset curve everywhere between them (AutoCAD also
/// offsets ellipses and splines to splines). `None` where the curve has no direction (a cusp).
pub fn offset_spline(c: &CurveRef, t0: f64, t1: f64, dist: f64, closed: bool, tol: f64) -> Option<Spline> {
    if !(t0.is_finite() && t1.is_finite() && dist.is_finite() && t1 > t0) {
        return None;
    }
    let off = |t: f64| {
        let n = c.d1(t).perp().normalized();
        (n != Vec2::ZERO).then(|| c.eval(t) + n * dist)
    };
    let tol = tol.max(1e-12 * c.size().max(dist.abs()));
    let mut n = 16usize;
    loop {
        let ts: Vec<f64> = (0..=n).map(|i| t0 + (t1 - t0) * i as f64 / n as f64).collect();
        let mut pts: Vec<Vec2> = ts.iter().map(|t| off(*t)).collect::<Option<_>>()?;
        if closed {
            pts.pop();
        }
        let sp = Spline::from_fit(&pts, closed);
        if !sp.is_valid() {
            return None;
        }
        // Halfway between neighbouring fit points, how far is the spline from the true offset?
        let fit_param = |i: usize, p: Vec2| {
            let (lo, hi) = sp.domain();
            refine_closest(&CurveRef::Spline(&sp), p, Some(lo + (hi - lo) * i as f64 / n as f64)).1
        };
        let us: Vec<f64> = pts.iter().enumerate().map(|(i, p)| fit_param(i, *p)).collect();
        // At the quarter points of each span, how far is the spline from the true offset (its
        // distance from `c` less |dist|)? Fitting through exact offset points and refining until
        // the deviation is within tolerance is the offset approximation of W. Tiller and E. G.
        // Hanson, "Offsets of two-dimensional profiles", IEEE CG&A 4(9) (1984), and of L. Piegl and
        // W. Tiller, "Computing offsets of NURBS curves and surfaces", CAD 31(2) (1999).
        let mut worst: f64 = 0.0;
        for i in 0..n {
            let (Some(&ua), Some(&ta), Some(&tb)) = (us.get(i), ts.get(i), ts.get(i + 1)) else { continue };
            let ub = us.get(i + 1).copied().unwrap_or_else(|| sp.domain().1);
            for f in [0.25, 0.5, 0.75] {
                let q = sp.eval(ua + (ub - ua) * f);
                let (on_c, _) = refine_closest(c, q, Some(ta + (tb - ta) * f));
                worst = worst.max((on_c.dist(q) - dist.abs()).abs());
            }
        }
        if worst <= tol || n >= 1024 {
            return Some(sp);
        }
        n *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Circle, TAU, line_circle};

    fn ellipse() -> Ellipse {
        Ellipse::full(Vec2::new(3.0, -2.0), Vec2::new(8.0, 3.0), 0.45)
    }

    /// The distance from a point to an ellipse, measured exactly in its own frame.
    fn off_ellipse(e: &Ellipse, p: Vec2) -> f64 {
        let ux = e.major / e.major.len();
        let l = p - e.center;
        let (x, y) = (l.dot(ux) / e.major.len(), l.dot(ux.perp()) / (e.major.len() * e.ratio));
        ((x * x + y * y).sqrt() - 1.0).abs() * e.major.len()
    }

    #[test]
    fn a_line_and_an_ellipse_meet_exactly() {
        let e = ellipse();
        let line = Line::new(Vec2::new(-10.0, 1.3), Vec2::new(15.0, -0.7));
        // The tessellated answer, as callers get it today.
        let mut pts = Vec::new();
        e.tessellate(e.major.len() * 1e-3, &mut pts);
        let rough: Vec<Vec2> = pts.windows(2).filter_map(|w| crate::line_line(&Line::new(w[0], w[1]), &line)).collect();
        assert_eq!(rough.len(), 2);
        for p0 in rough {
            let p = refine_intersection(&CurveRef::Ellipse(e), &CurveRef::Line(line), p0, 1.0).unwrap_or(p0);
            assert!(off_ellipse(&e, p) < 1e-11, "{}", off_ellipse(&e, p));
            assert!(line.dist(p) < 1e-11);
            assert!(off_ellipse(&e, p0) > 1e-6, "the rough point really was rough");
        }
    }

    #[test]
    fn a_circle_and_an_ellipse_meet_exactly() {
        let e = ellipse();
        // Centred on the outline, so it crosses it twice.
        let c = Circle::new(e.at_param(0.7), 2.0);
        let mut pts = Vec::new();
        e.tessellate(e.major.len() * 1e-3, &mut pts);
        let mut n = 0;
        for w in pts.windows(2) {
            for (p0, t) in line_circle(&Line::new(w[0], w[1]), &c) {
                if !(0.0..=1.0).contains(&t) {
                    continue;
                }
                let p = refine_intersection(&CurveRef::Ellipse(e), &CurveRef::Circle { center: c.center, radius: c.radius }, p0, 1.0).unwrap_or(p0);
                assert!(off_ellipse(&e, p) < 1e-11);
                assert!((p.dist(c.center) - c.radius).abs() < 1e-11);
                n += 1;
            }
        }
        assert!(n >= 2);
    }

    #[test]
    fn a_spline_and_a_line_meet_on_both() {
        let s = Spline::from_control(vec![Vec2::new(0.0, 0.0), Vec2::new(2.0, 5.0), Vec2::new(5.0, -3.0), Vec2::new(9.0, 4.0)], 3);
        let line = Line::new(Vec2::new(-1.0, 1.0), Vec2::new(10.0, 1.2));
        let pts = s.tessellate(1e-3);
        let rough: Vec<Vec2> = pts.windows(2).filter_map(|w| crate::line_line(&Line::new(w[0], w[1]), &line)).collect();
        assert!(!rough.is_empty());
        for p0 in rough {
            let p = refine_intersection(&CurveRef::Spline(&s), &CurveRef::Line(line), p0, 1.0).unwrap_or(p0);
            let (on, _) = refine_closest(&CurveRef::Spline(&s), p, None);
            assert!(on.dist(p) < 1e-10, "{}", on.dist(p));
            assert!(line.dist(p) < 1e-10);
        }
    }

    #[test]
    fn a_tangent_line_touches_an_ellipse_once() {
        // The horizontal line through the top of an axis-aligned ellipse.
        let e = Ellipse::full(Vec2::ZERO, Vec2::new(5.0, 0.0), 0.4);
        let line = Line::new(Vec2::new(-9.0, 2.0), Vec2::new(9.0, 2.0));
        let p = refine_intersection(&CurveRef::Ellipse(e), &CurveRef::Line(line), Vec2::new(0.01, 2.0), 0.5);
        let p = p.expect("a tangency still refines");
        assert!(p.dist(Vec2::new(0.0, 2.0)) < 1e-5, "{p:?}");
    }

    #[test]
    fn curves_that_miss_are_not_made_to_meet() {
        let e = ellipse();
        let line = Line::new(Vec2::new(-10.0, 40.0), Vec2::new(10.0, 41.0));
        assert!(refine_intersection(&CurveRef::Ellipse(e), &CurveRef::Line(line), Vec2::new(0.0, 40.0), 5.0).is_none());
    }

    #[test]
    fn the_closest_point_on_an_ellipse_is_perpendicular() {
        let e = ellipse();
        for k in 0..12 {
            let a = TAU * k as f64 / 12.0;
            let p = e.center + Vec2::new(a.cos(), a.sin()) * 11.0;
            let (q, t) = refine_closest(&CurveRef::Ellipse(e), p, None);
            assert!(off_ellipse(&e, q) < 1e-11);
            assert!((q - p).dot(e.derivative(t)).abs() < 1e-9 * e.major.len().powi(2));
            // No sampled point is closer.
            let mut pts = Vec::new();
            e.tessellate(1e-4, &mut pts);
            assert!(pts.iter().all(|s| s.dist(p) >= q.dist(p) - 1e-12));
        }
    }

    #[test]
    fn far_from_the_origin_it_stays_exact() {
        // State-plane coordinates: an ellipse two million feet out.
        let e = Ellipse::full(Vec2::new(2_400_000.0, 7_000_000.0), Vec2::new(30.0, 10.0), 0.5);
        let line = Line::new(e.center - Vec2::new(50.0, 3.0), e.center + Vec2::new(50.0, 3.0));
        let mut pts = Vec::new();
        e.tessellate(e.major.len() * 1e-3, &mut pts);
        for w in pts.windows(2) {
            if let Some(p0) = crate::line_line(&Line::new(w[0], w[1]), &line) {
                let p = refine_intersection(&CurveRef::Ellipse(e), &CurveRef::Line(line), p0, 1.0).expect("refines");
                assert!(off_ellipse(&e, p) < 1e-6, "{}", off_ellipse(&e, p));
                assert!(line.dist(p) < 1e-6);
            }
        }
    }

    #[test]
    fn an_offset_ellipse_stays_at_the_distance() {
        let e = ellipse();
        let sp = offset_spline(&CurveRef::Ellipse(e), 0.0, TAU, 1.5, true, 1e-6 * e.major.len()).unwrap();
        // Everywhere along the result, 1.5 from the ellipse (outside: an ellipse runs CCW).
        let (lo, hi) = sp.domain();
        for i in 0..400 {
            let q = sp.eval(lo + (hi - lo) * i as f64 / 400.0);
            let (on, _) = refine_closest(&CurveRef::Ellipse(e), q, None);
            assert!((on.dist(q) - 1.5).abs() < 2.0 * 1e-6 * e.major.len(), "{} at {i}", on.dist(q));
            assert!(off_ellipse(&e, q) > 1.0);
        }
        assert!(sp.closed);
    }

    #[test]
    fn an_offset_spline_follows_the_curve() {
        let s = Spline::from_control(vec![Vec2::new(0.0, 0.0), Vec2::new(2.0, 5.0), Vec2::new(5.0, -3.0), Vec2::new(9.0, 4.0)], 3);
        let (lo, hi) = s.domain();
        let sp = offset_spline(&CurveRef::Spline(&s), lo, hi, -0.4, false, 1e-6).unwrap();
        let (a, b) = sp.domain();
        for i in 0..=300 {
            let q = sp.eval(a + (b - a) * i as f64 / 300.0);
            let (on, _) = refine_closest(&CurveRef::Spline(&s), q, None);
            assert!((on.dist(q) - 0.4).abs() < 2e-6, "{} at {i}", on.dist(q));
        }
    }

    #[test]
    fn huge_and_tiny_ellipses_offset_alike() {
        // The same ellipse and distance in a drawing a million times smaller, larger, and far from
        // the origin: the same relative accuracy.
        for (scale, shift) in [(1e-6, 0.0), (1.0, 0.0), (1e6, 0.0), (1.0, 1e7)] {
            let e = Ellipse::full(Vec2::new(3.0, -2.0) * scale + Vec2::new(shift, shift), Vec2::new(8.0, 3.0) * scale, 0.45);
            let d = 1.5 * scale;
            let tol = 1e-6 * e.major.len();
            let sp = offset_spline(&CurveRef::Ellipse(e), 0.0, TAU, d, true, tol).unwrap();
            let (lo, hi) = sp.domain();
            for i in 0..400 {
                let q = sp.eval(lo + (hi - lo) * i as f64 / 400.0);
                let (on, _) = refine_closest(&CurveRef::Ellipse(e), q, None);
                assert!((on.dist(q) - d).abs() < 2.0 * tol + 1e-15 * shift, "scale {scale}: {} at {i}", (on.dist(q) - d).abs() / scale);
            }
        }
    }
}
