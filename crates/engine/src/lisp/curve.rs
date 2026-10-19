//! The `vlax-curve-` functions: points, parameters, distances, derivatives and areas along
//! LINE, ARC, CIRCLE, ELLIPSE, LWPOLYLINE / POLYLINE (2D and 3D), SPLINE, RAY and XLINE, with
//! each type's parameter as AutoCAD defines it:
//!
//! - LINE, RAY, XLINE: the distance from the start (the base point; an XLINE runs from −1e99 to
//!   1e99, a RAY to 1e99);
//! - ARC, CIRCLE: the angle (radians, counterclockwise from +X);
//! - ELLIPSE: the parametric angle (the point is `center + major·cos t + minor·sin t`);
//! - polylines: the vertex index, with the fraction along each segment (by angle on an arc
//!   segment);
//! - SPLINE: the knot value.
//!
//! Lengths of ellipses and splines are integrated (Gauss–Legendre); closest points on them are
//! found by sampling and refining. Objects are passed as entity names.

use cadaddy_doc::EntityKind;
use cadaddy_geom::{Spline as GSpline, Vec2, Vec3};
use std::f64::consts::{PI, TAU};

use super::LispError;
use super::builtins::arity;
use super::eval::{Ctx, Flow};
use super::value::Value;

type R = Result<Flow, LispError>;

/// The parameter at the ends of a RAY or an XLINE, as AutoCAD reports it.
const INFINITE: f64 = 1e99;
/// Gauss–Legendre nodes and weights on [−1, 1] (8 points).
const GL_X: [f64; 8] = [
    -0.960_289_856_497_536_2,
    -0.796_666_477_413_626_7,
    -0.525_532_409_916_329,
    -0.183_434_642_495_649_8,
    0.183_434_642_495_649_8,
    0.525_532_409_916_329,
    0.796_666_477_413_626_7,
    0.960_289_856_497_536_2,
];
const GL_W: [f64; 8] = [
    0.101_228_536_290_376_3,
    0.222_381_034_453_374_5,
    0.313_706_645_877_887_3,
    0.362_683_783_378_362,
    0.362_683_783_378_362,
    0.313_706_645_877_887_3,
    0.222_381_034_453_374_5,
    0.101_228_536_290_376_3,
];
/// Most vertices or control points a curve may have for these functions.
const MAX_POINTS: usize = 1_000_000;

/// A curve as these functions see it.
enum Curve {
    Line {
        a: Vec3,
        u: Vec3,
        len: f64,
    },
    /// A circle when `full`; `t1 > t0`.
    Arc {
        c: Vec3,
        r: f64,
        t0: f64,
        t1: f64,
        full: bool,
    },
    Ellipse {
        c: Vec3,
        major: Vec3,
        minor: Vec3,
        t0: f64,
        t1: f64,
        full: bool,
    },
    /// Vertices with the bulge of the segment each starts (0 on a 3D polyline).
    Poly {
        pts: Vec<Vec3>,
        bulges: Vec<f64>,
        closed: bool,
        flat: bool,
    },
    Spline {
        s: GSpline,
        z: f64,
    },
    Ray {
        base: Vec3,
        u: Vec3,
    },
    XLine {
        base: Vec3,
        u: Vec3,
    },
}

fn v3(p: Vec2, z: f64) -> Vec3 {
    Vec3::new(p.x, p.y, z)
}

fn unit(v: Vec3) -> Option<Vec3> {
    let l = v.len();
    (l > 1e-300 && l.is_finite()).then(|| v * (1.0 / l))
}

/// The arc of a bulged polyline segment: centre, radius, start angle and signed sweep.
fn bulge_arc(p0: Vec3, p1: Vec3, bulge: f64) -> Option<(Vec3, f64, f64, f64)> {
    let chord = Vec2::new(p1.x - p0.x, p1.y - p0.y);
    let c = chord.len();
    if bulge.abs() < 1e-12 || c < 1e-300 || !bulge.is_finite() {
        return None;
    }
    let sweep = 4.0 * bulge.atan();
    let half = sweep / 2.0;
    let n = Vec2::new(-chord.y, chord.x) * (1.0 / c);
    let m = Vec2::new((p0.x + p1.x) / 2.0, (p0.y + p1.y) / 2.0);
    let center = m + n * ((c / 2.0) / half.tan());
    let r = c / (2.0 * half.sin().abs());
    let a0 = (Vec2::new(p0.x, p0.y) - center).angle();
    Some((v3(center, p0.z), r, a0, sweep))
}

fn on_circle(c: Vec3, r: f64, a: f64) -> Vec3 {
    Vec3::new(c.x + r * a.cos(), c.y + r * a.sin(), c.z)
}

impl Curve {
    fn of(kind: &EntityKind) -> Option<Curve> {
        Some(match kind {
            EntityKind::Line(l) => {
                let d = l.b - l.a;
                Curve::Line { a: l.a, u: unit(d).unwrap_or(Vec3::new(1.0, 0.0, 0.0)), len: d.len() }
            }
            EntityKind::Circle(c) => Curve::Arc { c: c.center, r: c.radius, t0: 0.0, t1: TAU, full: true },
            EntityKind::Arc(a) => {
                let t0 = a.start.rem_euclid(TAU);
                let mut t1 = a.end.rem_euclid(TAU);
                if t1 <= t0 {
                    t1 += TAU;
                }
                Curve::Arc { c: a.center, r: a.radius, t0, t1, full: false }
            }
            EntityKind::Ellipse(e) => {
                let minor = Vec3::new(-e.major.y, e.major.x, 0.0) * e.ratio;
                let full = ((e.end - e.start).abs() - TAU).abs() < 1e-9 || (e.end - e.start).abs() < 1e-12;
                let (t0, t1) = if full {
                    (e.start, e.start + TAU)
                } else {
                    let t0 = e.start;
                    let mut t1 = e.end;
                    while t1 <= t0 {
                        t1 += TAU;
                    }
                    (t0, t1)
                };
                Curve::Ellipse { c: e.center, major: e.major, minor, t0, t1, full }
            }
            EntityKind::LwPolyline(p) => {
                if p.vertices.is_empty() || p.vertices.len() > MAX_POINTS {
                    return None;
                }
                let pts = p.vertices.iter().map(|v| v3(v.p, p.elevation)).collect();
                let bulges = p.vertices.iter().map(|v| v.bulge).collect();
                Curve::Poly { pts, bulges, closed: p.closed, flat: true }
            }
            EntityKind::Polyline3d(p) => {
                if p.points.is_empty() || p.points.len() > MAX_POINTS {
                    return None;
                }
                Curve::Poly { pts: p.points.clone(), bulges: vec![0.0; p.points.len()], closed: p.closed, flat: false }
            }
            EntityKind::Spline(s) => {
                if !s.is_valid() || s.control.len() > MAX_POINTS {
                    return None;
                }
                Curve::Spline { s: s.clone(), z: 0.0 }
            }
            EntityKind::Ray(r) => Curve::Ray { base: r.base, u: unit(r.dir)? },
            EntityKind::XLine(r) => Curve::XLine { base: r.base, u: unit(r.dir)? },
            _ => return None,
        })
    }

    fn segments(&self) -> usize {
        match self {
            Curve::Poly { pts, closed, .. } => {
                if *closed && pts.len() > 1 {
                    pts.len()
                } else {
                    pts.len().saturating_sub(1)
                }
            }
            _ => 0,
        }
    }

    fn domain(&self) -> (f64, f64) {
        match self {
            Curve::Line { len, .. } => (0.0, *len),
            Curve::Arc { t0, t1, .. } | Curve::Ellipse { t0, t1, .. } => (*t0, *t1),
            Curve::Poly { .. } => (0.0, self.segments() as f64),
            Curve::Spline { s, .. } => s.domain(),
            Curve::Ray { .. } => (0.0, INFINITE),
            Curve::XLine { .. } => (-INFINITE, INFINITE),
        }
    }

    /// Whether `t` lies in the domain (allowing rounding).
    fn holds(&self, t: f64) -> bool {
        let (a, b) = self.domain();
        let tol = 1e-10 * (b - a).abs().clamp(1.0, 1e6);
        t.is_finite() && t >= a - tol && t <= b + tol
    }

    fn clamp(&self, t: f64) -> f64 {
        let (a, b) = self.domain();
        t.clamp(a, b)
    }

    /// The polyline segment at `t` and the fraction along it.
    fn seg_at(&self, t: f64) -> (usize, f64) {
        let n = self.segments();
        if n == 0 {
            return (0, 0.0);
        }
        let i = (t.floor().max(0.0) as usize).min(n - 1);
        (i, t - i as f64)
    }

    fn seg_ends(&self, i: usize) -> Option<(Vec3, Vec3, f64)> {
        let Curve::Poly { pts, bulges, .. } = self else { return None };
        let p0 = *pts.get(i)?;
        let p1 = *pts.get((i + 1) % pts.len())?;
        Some((p0, p1, bulges.get(i).copied().unwrap_or(0.0)))
    }

    /// Point, first and second derivative with respect to the parameter.
    fn eval(&self, t: f64) -> (Vec3, Vec3, Vec3) {
        match self {
            Curve::Line { a, u, .. } | Curve::Ray { base: a, u } | Curve::XLine { base: a, u } => (*a + *u * t, *u, Vec3::ZERO),
            Curve::Arc { c, r, .. } => {
                let (s, k) = t.sin_cos();
                (on_circle(*c, *r, t), Vec3::new(-r * s, r * k, 0.0), Vec3::new(-r * k, -r * s, 0.0))
            }
            Curve::Ellipse { c, major, minor, .. } => {
                let (s, k) = t.sin_cos();
                (*c + *major * k + *minor * s, *minor * k - *major * s, -(*major * k + *minor * s))
            }
            Curve::Poly { pts, .. } => {
                if self.segments() == 0 {
                    return (pts.first().copied().unwrap_or(Vec3::ZERO), Vec3::ZERO, Vec3::ZERO);
                }
                let (i, f) = self.seg_at(t);
                let Some((p0, p1, b)) = self.seg_ends(i) else { return (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO) };
                match bulge_arc(p0, p1, b) {
                    None => (p0 + (p1 - p0) * f, p1 - p0, Vec3::ZERO),
                    Some((c, r, a0, sw)) => {
                        let a = a0 + f * sw;
                        let (s, k) = a.sin_cos();
                        let z = p0.z + (p1.z - p0.z) * f;
                        (
                            Vec3::new(c.x + r * k, c.y + r * s, z),
                            Vec3::new(-r * s * sw, r * k * sw, p1.z - p0.z),
                            Vec3::new(-r * k * sw * sw, -r * s * sw * sw, 0.0),
                        )
                    }
                }
            }
            Curve::Spline { s, z } => {
                let at = |t: f64| v3(s.eval(t), *z);
                let (lo, hi) = s.domain();
                let span = (hi - lo).abs().max(1e-12);
                // Derivatives by finite differences, the stencil kept inside the domain.
                let h = 1e-5 * span;
                let d1 = if t - h < lo {
                    (at(t) * -3.0 + at(t + h) * 4.0 - at(t + 2.0 * h)) * (1.0 / (2.0 * h))
                } else if t + h > hi {
                    (at(t) * 3.0 - at(t - h) * 4.0 + at(t - 2.0 * h)) * (1.0 / (2.0 * h))
                } else {
                    (at(t + h) - at(t - h)) * (1.0 / (2.0 * h))
                };
                let h2 = 1e-3 * span;
                let mid = t.clamp(lo + h2, hi - h2);
                let d2 = (at(mid + h2) - at(mid) * 2.0 + at(mid - h2)) * (1.0 / (h2 * h2));
                (at(t), d1, d2)
            }
        }
    }

    fn point(&self, t: f64) -> Vec3 {
        self.eval(t).0
    }

    fn integrate_speed(&self, a: f64, b: f64, pieces: usize) -> f64 {
        if b <= a {
            return 0.0;
        }
        let w = (b - a) / pieces as f64;
        let mut sum = 0.0;
        for k in 0..pieces {
            let (lo, hi) = (a + w * k as f64, a + w * (k + 1) as f64);
            let (m, h) = ((lo + hi) / 2.0, (hi - lo) / 2.0);
            for (x, wt) in GL_X.iter().zip(GL_W) {
                sum += wt * h * self.eval(m + h * x).1.len();
            }
        }
        sum
    }

    /// The distance along the curve from its start to `t`.
    fn dist(&self, t: f64) -> f64 {
        let t = self.clamp(t);
        match self {
            Curve::Line { .. } | Curve::Ray { .. } | Curve::XLine { .. } => t,
            Curve::Arc { r, t0, .. } => r * (t - t0),
            Curve::Ellipse { t0, .. } => self.integrate_speed(*t0, t, 64),
            Curve::Poly { .. } => {
                let (i, f) = self.seg_at(t);
                (0..i).map(|k| self.seg_len(k)).sum::<f64>() + f * self.seg_len(i)
            }
            Curve::Spline { s, .. } => {
                // Gauss–Legendre on each knot span (the curve is smooth inside them).
                let (lo, _) = s.domain();
                let mut breaks: Vec<f64> = s.knots.iter().copied().filter(|k| *k > lo && *k < t).collect();
                breaks.dedup_by(|a, b| (*a - *b).abs() < 1e-14);
                let mut sum = 0.0;
                let mut a = lo;
                for k in breaks.into_iter().take(MAX_POINTS).chain(std::iter::once(t)) {
                    sum += self.integrate_speed(a, k, 4);
                    a = k;
                }
                sum
            }
        }
    }

    fn seg_len(&self, i: usize) -> f64 {
        match self.seg_ends(i) {
            Some((p0, p1, b)) => match bulge_arc(p0, p1, b) {
                Some((_, r, _, sw)) => {
                    let flat = r * sw.abs();
                    (flat * flat + (p1.z - p0.z).powi(2)).sqrt()
                }
                None => (p1 - p0).len(),
            },
            None => 0.0,
        }
    }

    fn length(&self) -> f64 {
        self.dist(self.domain().1)
    }

    /// The parameter `d` along the curve from its start; `None` past either end.
    fn param_at_dist(&self, d: f64) -> Option<f64> {
        if !d.is_finite() {
            return None;
        }
        match self {
            Curve::Line { len, .. } => (d >= -1e-12 && d <= len + 1e-9 * len.max(1.0)).then(|| d.clamp(0.0, *len)),
            Curve::Ray { .. } => (d >= 0.0).then_some(d),
            Curve::XLine { .. } => Some(d),
            Curve::Arc { r, t0, t1, .. } => {
                let t = t0 + d / r;
                (d >= -1e-12 && t <= t1 + 1e-12).then(|| t.min(*t1))
            }
            Curve::Poly { .. } => {
                let total = self.length();
                if d < -1e-12 || d > total + 1e-9 * total.max(1.0) {
                    return None;
                }
                let mut left = d.max(0.0);
                let n = self.segments();
                for i in 0..n {
                    let l = self.seg_len(i);
                    if left <= l || i + 1 == n {
                        return Some(i as f64 + if l > 0.0 { (left / l).min(1.0) } else { 0.0 });
                    }
                    left -= l;
                }
                Some(0.0)
            }
            Curve::Ellipse { .. } | Curve::Spline { .. } => {
                let total = self.length();
                if d < -1e-12 || d > total + 1e-9 * total.max(1.0) {
                    return None;
                }
                let (mut lo, mut hi) = self.domain();
                for _ in 0..80 {
                    let mid = (lo + hi) / 2.0;
                    if self.dist(mid) < d {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                Some((lo + hi) / 2.0)
            }
        }
    }

    /// The parameter of the point closest to `p`, measured after `flat` (which drops what the
    /// projection direction removes; the identity for plain distances). `extend` lets lines,
    /// rays, arcs and the end segments of an open polyline run on past their ends.
    fn closest(&self, p: Vec3, extend: bool, flat: &dyn Fn(Vec3) -> Vec3) -> f64 {
        let d2 = |t: f64| {
            let v = flat(self.point(t) - p);
            v.dot(v)
        };
        match self {
            Curve::Line { a, u, .. } | Curve::Ray { base: a, u } | Curve::XLine { base: a, u } => {
                let (fu, fp) = (flat(*u), flat(p - *a));
                let k = fu.dot(fu);
                let t = if k > 1e-300 { fu.dot(fp) / k } else { 0.0 };
                match self {
                    Curve::Line { len, .. } if !extend => t.clamp(0.0, *len),
                    Curve::Ray { .. } if !extend => t.max(0.0),
                    _ => t,
                }
            }
            Curve::Arc { c, t0, t1, full, .. } if flat(Vec3::Z).len() < 1e-12 || flat(Vec3::Z).len() > 0.999_999 => {
                // In the circle's plane (projections along Z or none at all).
                let a = (p.y - c.y).atan2(p.x - c.x);
                if *full || extend {
                    let mut t = a;
                    while t < *t0 {
                        t += TAU;
                    }
                    while t >= t0 + TAU {
                        t -= TAU;
                    }
                    return t;
                }
                let mut t = a;
                while t < *t0 {
                    t += TAU;
                }
                if t <= *t1 {
                    t
                } else if d2(*t0) <= d2(*t1) {
                    *t0
                } else {
                    *t1
                }
            }
            Curve::Poly { .. } => {
                let n = self.segments();
                let mut best = (f64::MAX, 0.0);
                for i in 0..n {
                    let lo = if extend && i == 0 && !self.is_closed() { -1e6 } else { 0.0 };
                    let hi = if extend && i + 1 == n && !self.is_closed() { 1e6 } else { 1.0 };
                    let f = self.closest_on_segment(i, p, lo, hi, flat);
                    let t = i as f64 + f;
                    let dd = {
                        let v = flat(self.eval_unclamped(i, f) - p);
                        v.dot(v)
                    };
                    if dd < best.0 {
                        best = (dd, t);
                    }
                }
                best.1
            }
            _ => {
                // Sample, then narrow the best bracket by golden sections.
                let (lo, hi) = self.domain();
                let samples = match self {
                    Curve::Spline { s, .. } => (s.control.len() * 16).clamp(64, 4096),
                    _ => 256,
                };
                let step = (hi - lo) / samples as f64;
                let mut best = (f64::MAX, lo);
                for k in 0..=samples {
                    let t = lo + step * k as f64;
                    let dd = d2(t);
                    if dd < best.0 {
                        best = (dd, t);
                    }
                }
                let (mut a, mut b) = ((best.1 - step).max(lo), (best.1 + step).min(hi));
                let g = (5f64.sqrt() - 1.0) / 2.0;
                for _ in 0..100 {
                    let (x1, x2) = (b - g * (b - a), a + g * (b - a));
                    if d2(x1) < d2(x2) {
                        b = x2;
                    } else {
                        a = x1;
                    }
                }
                (a + b) / 2.0
            }
        }
    }

    /// A polyline segment's point at fraction `f`, also past its ends.
    fn eval_unclamped(&self, i: usize, f: f64) -> Vec3 {
        let Some((p0, p1, b)) = self.seg_ends(i) else { return Vec3::ZERO };
        match bulge_arc(p0, p1, b) {
            None => p0 + (p1 - p0) * f,
            Some((c, r, a0, sw)) => {
                let mut q = on_circle(c, r, a0 + f * sw);
                q.z = p0.z + (p1.z - p0.z) * f;
                q
            }
        }
    }

    fn closest_on_segment(&self, i: usize, p: Vec3, lo: f64, hi: f64, flat: &dyn Fn(Vec3) -> Vec3) -> f64 {
        let Some((p0, p1, b)) = self.seg_ends(i) else { return 0.0 };
        match bulge_arc(p0, p1, b) {
            None => {
                let (d, w) = (flat(p1 - p0), flat(p - p0));
                let k = d.dot(d);
                if k > 1e-300 { (d.dot(w) / k).clamp(lo, hi) } else { 0.0 }
            }
            Some((c, _, a0, sw)) => {
                // The angle of `p` about the centre, as a fraction of the sweep; outside the arc
                // the nearer end.
                let a = (p.y - c.y).atan2(p.x - c.x);
                let mut rel = (a - a0) * sw.signum();
                rel = rel.rem_euclid(TAU);
                let f = rel / sw.abs();
                if (lo..=hi).contains(&f) {
                    return f;
                }
                let end = |f: f64| {
                    let v = flat(self.eval_unclamped(i, f) - p);
                    v.dot(v)
                };
                let back = f - TAU / sw.abs();
                let candidates = [lo.max(-1e6), hi.min(1e6), back];
                candidates
                    .into_iter()
                    .filter(|f| *f >= lo && *f <= hi)
                    .fold((f64::MAX, 0.0), |acc, f| if end(f) < acc.0 { (end(f), f) } else { acc })
                    .1
            }
        }
    }

    fn is_closed(&self) -> bool {
        match self {
            Curve::Arc { full, .. } | Curve::Ellipse { full, .. } => *full,
            Curve::Poly { closed, pts, .. } => *closed && pts.len() > 1,
            Curve::Spline { s, .. } => {
                let (a, b) = s.domain();
                s.closed || s.eval(a).dist(s.eval(b)) < 1e-10 * s.bounds().width().max(s.bounds().height()).max(1.0)
            }
            _ => false,
        }
    }

    fn is_periodic(&self) -> bool {
        match self {
            Curve::Arc { full, .. } | Curve::Ellipse { full, .. } => *full,
            Curve::Spline { s, .. } => s.is_periodic(),
            _ => false,
        }
    }

    fn is_planar(&self) -> bool {
        match self {
            Curve::Poly { pts, flat: false, .. } => {
                let Some(n) = unit(newell(pts)) else { return true };
                let Some(o) = pts.first() else { return true };
                let size = pts.iter().map(|q| (*q - *o).len()).fold(0.0, f64::max).max(1e-9);
                pts.iter().all(|q| (*q - *o).dot(n).abs() <= 1e-9 * size.max(1.0))
            }
            _ => true,
        }
    }

    /// The area the curve encloses (closed by the chord from its end to its start).
    fn area(&self) -> f64 {
        match self {
            Curve::Line { .. } | Curve::Ray { .. } | Curve::XLine { .. } => 0.0,
            Curve::Arc { r, t0, t1, full, .. } => {
                if *full {
                    PI * r * r
                } else {
                    let d = t1 - t0;
                    r * r / 2.0 * (d - d.sin())
                }
            }
            Curve::Ellipse { major, minor, t0, t1, full, .. } => {
                let ab = major.len() * minor.len();
                if *full {
                    PI * ab
                } else {
                    let d = t1 - t0;
                    ab / 2.0 * (d - d.sin())
                }
            }
            Curve::Poly { pts, bulges, flat, .. } => {
                if !flat {
                    return newell(pts).len() / 2.0;
                }
                let n = pts.len();
                let mut a = 0.0;
                for i in 0..n {
                    let (Some(p), Some(q)) = (pts.get(i), pts.get((i + 1) % n)) else { continue };
                    a += p.x * q.y - q.x * p.y;
                }
                a /= 2.0;
                // Each bulged segment adds (or takes away) its circular segment; the closing
                // segment of an open polyline is straight.
                let segs = self.segments();
                for i in 0..segs {
                    let (Some(p0), Some(p1)) = (pts.get(i), pts.get((i + 1) % n)) else { continue };
                    if let Some((_, r, _, sw)) = bulge_arc(*p0, *p1, bulges.get(i).copied().unwrap_or(0.0)) {
                        a += r * r / 2.0 * (sw - sw.sin());
                    }
                }
                a.abs()
            }
            Curve::Spline { s, .. } => {
                let pts = s.tessellate((s.bounds().width().max(s.bounds().height()) * 1e-6).max(1e-9));
                let n = pts.len();
                let mut a = 0.0;
                for i in 0..n {
                    let (Some(p), Some(q)) = (pts.get(i), pts.get((i + 1) % n)) else { continue };
                    a += p.cross(*q);
                }
                (a / 2.0).abs()
            }
        }
    }

    fn start_point(&self) -> Option<Vec3> {
        match self {
            Curve::XLine { .. } => None,
            _ => Some(self.point(self.domain().0)),
        }
    }

    fn end_point(&self) -> Option<Vec3> {
        match self {
            Curve::Ray { .. } | Curve::XLine { .. } => None,
            _ => Some(self.point(self.domain().1)),
        }
    }

    /// How far a point may be from the curve and still be on it.
    fn tolerance(&self) -> f64 {
        let size = match self {
            Curve::Line { len, .. } => *len,
            Curve::Arc { r, .. } => *r,
            Curve::Ellipse { major, .. } => major.len(),
            Curve::Poly { pts, .. } => pts.iter().map(|q| q.x.abs().max(q.y.abs())).fold(0.0, f64::max),
            Curve::Spline { s, .. } => s.bounds().width().max(s.bounds().height()),
            Curve::Ray { base, .. } | Curve::XLine { base, .. } => base.x.abs().max(base.y.abs()),
        };
        1e-6 * size.max(1.0)
    }
}

/// Twice the area vector of a polygon (Newell's method).
fn newell(pts: &[Vec3]) -> Vec3 {
    let n = pts.len();
    let mut v = Vec3::ZERO;
    for i in 0..n {
        let (Some(p), Some(q)) = (pts.get(i), pts.get((i + 1) % n)) else { continue };
        v = v + p.cross(*q);
    }
    v
}

// ---------- the functions ----------

fn curve(ctx: &Ctx<'_>, v: &Value) -> Result<Curve, LispError> {
    let Value::Ename(h) = v else { return Err(LispError::bad_arg("lentityp", v)) };
    let d = ctx.s.doc().map_err(|e| LispError::new(e.to_string()))?;
    let e = d.entity(*h).ok_or_else(|| LispError::bad_arg("lentityp", v))?;
    Curve::of(&e.kind).ok_or_else(|| LispError::new(format!("not a curve: {}", e.kind.dxf_name())))
}

fn number(v: &Value) -> Result<f64, LispError> {
    v.as_number().ok_or_else(|| LispError::bad_arg("numberp", v))
}

fn point_arg(v: &Value) -> Result<Vec3, LispError> {
    let [x, y, z] = v.as_point().ok_or_else(|| LispError::bad_arg("2D/3D point", v))?;
    Ok(Vec3::new(x, y, z))
}

fn pt(p: Vec3) -> Value {
    Value::point(&[p.x, p.y, p.z])
}

fn opt_pt(p: Option<Vec3>) -> Flow {
    Flow::Value(p.filter(|p| p.is_finite()).map(pt).unwrap_or_default())
}

fn real(r: f64) -> Flow {
    Flow::Value(if r.is_finite() { Value::Real(r) } else { Value::Nil })
}

fn one(ctx: &Ctx<'_>, a: &[Value]) -> Result<Curve, LispError> {
    arity(a, 1, 1)?;
    curve(ctx, a.first().unwrap_or(&Value::Nil))
}

fn with_number(ctx: &Ctx<'_>, a: &[Value]) -> Result<(Curve, f64), LispError> {
    arity(a, 2, 2)?;
    let c = curve(ctx, a.first().unwrap_or(&Value::Nil))?;
    Ok((c, number(a.get(1).unwrap_or(&Value::Nil))?))
}

fn with_point(ctx: &Ctx<'_>, a: &[Value]) -> Result<(Curve, Vec3), LispError> {
    arity(a, 2, 2)?;
    let c = curve(ctx, a.first().unwrap_or(&Value::Nil))?;
    Ok((c, point_arg(a.get(1).unwrap_or(&Value::Nil))?))
}

/// The parameter of `p` when it is on the curve.
fn param_of(c: &Curve, p: Vec3) -> Option<f64> {
    let t = c.closest(p, false, &|v| v);
    ((c.point(t) - p).len() <= c.tolerance()).then_some(t)
}

pub fn start_point(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    Ok(opt_pt(one(ctx, &a)?.start_point()))
}

pub fn end_point(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    Ok(opt_pt(one(ctx, &a)?.end_point()))
}

pub fn start_param(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    Ok(real(one(ctx, &a)?.domain().0))
}

pub fn end_param(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    Ok(real(one(ctx, &a)?.domain().1))
}

pub fn area(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    Ok(real(one(ctx, &a)?.area()))
}

pub fn is_closed(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    Ok(Flow::Value(Value::bool(one(ctx, &a)?.is_closed())))
}

pub fn is_periodic(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    Ok(Flow::Value(Value::bool(one(ctx, &a)?.is_periodic())))
}

pub fn is_planar(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    Ok(Flow::Value(Value::bool(one(ctx, &a)?.is_planar())))
}

pub fn dist_at_param(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    let (c, t) = with_number(ctx, &a)?;
    Ok(if c.holds(t) { real(c.dist(t)) } else { Flow::Value(Value::Nil) })
}

pub fn param_at_dist(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    let (c, d) = with_number(ctx, &a)?;
    Ok(c.param_at_dist(d).map_or(Flow::Value(Value::Nil), real))
}

pub fn point_at_param(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    let (c, t) = with_number(ctx, &a)?;
    Ok(opt_pt(c.holds(t).then(|| c.point(c.clamp(t)))))
}

pub fn point_at_dist(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    let (c, d) = with_number(ctx, &a)?;
    Ok(opt_pt(c.param_at_dist(d).map(|t| c.point(t))))
}

pub fn dist_at_point(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    let (c, p) = with_point(ctx, &a)?;
    Ok(param_of(&c, p).map_or(Flow::Value(Value::Nil), |t| real(c.dist(t))))
}

pub fn param_at_point(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    let (c, p) = with_point(ctx, &a)?;
    Ok(param_of(&c, p).map_or(Flow::Value(Value::Nil), real))
}

pub fn first_deriv(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    let (c, t) = with_number(ctx, &a)?;
    Ok(opt_pt(c.holds(t).then(|| c.eval(c.clamp(t)).1)))
}

pub fn second_deriv(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    let (c, t) = with_number(ctx, &a)?;
    Ok(opt_pt(c.holds(t).then(|| c.eval(c.clamp(t)).2)))
}

/// `(vlax-curve-getClosestPointTo curve point [extend])`.
pub fn closest_point_to(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    arity(&a, 2, 3)?;
    let c = curve(ctx, a.first().unwrap_or(&Value::Nil))?;
    let p = point_arg(a.get(1).unwrap_or(&Value::Nil))?;
    let extend = a.get(2).is_some_and(|v| !v.is_nil());
    let t = c.closest(p, extend, &|v| v);
    Ok(opt_pt(Some(point_on(&c, t, extend))))
}

/// A point at `t`, which may be past the ends when extending.
fn point_on(c: &Curve, t: f64, extend: bool) -> Vec3 {
    match c {
        Curve::Poly { .. } if extend => {
            let n = c.segments();
            if n == 0 {
                return c.point(0.0);
            }
            let i = if t < 0.0 { 0 } else { (t.floor().max(0.0) as usize).min(n - 1) };
            c.eval_unclamped(i, t - i as f64)
        }
        Curve::Arc { .. } | Curve::Line { .. } | Curve::Ray { .. } | Curve::XLine { .. } => c.point(t),
        _ => c.point(c.clamp(t)),
    }
}

/// `(vlax-curve-getClosestPointToProjection curve point normal [extend])`: the point of the curve
/// nearest `point` when both are seen along `normal`.
pub fn closest_point_to_projection(ctx: &mut Ctx<'_>, a: Vec<Value>) -> R {
    arity(&a, 3, 4)?;
    let c = curve(ctx, a.first().unwrap_or(&Value::Nil))?;
    let p = point_arg(a.get(1).unwrap_or(&Value::Nil))?;
    let n = unit(point_arg(a.get(2).unwrap_or(&Value::Nil))?).ok_or_else(|| LispError::new("the projection direction has no length"))?;
    let extend = a.get(3).is_some_and(|v| !v.is_nil());
    let flat = move |v: Vec3| v - n * v.dot(n);
    // Seen along Z, a curve in a plane parallel to XY is nearest where it is in plan.
    let t = if c.is_planar() && n.z.abs() > 1.0 - 1e-12 && !matches!(c, Curve::Line { .. } | Curve::Ray { .. } | Curve::XLine { .. }) {
        let z = c.point(c.domain().0).z;
        c.closest(Vec3::new(p.x, p.y, z), extend, &flat)
    } else {
        c.closest(p, extend, &flat)
    };
    Ok(opt_pt(Some(point_on(&c, t, extend))))
}
