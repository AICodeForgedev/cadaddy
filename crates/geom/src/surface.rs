//! NURBS surfaces: a tensor product of rational B-splines in `u` and `v`.
//!
//! Published methods (L. Piegl and W. Tiller, *The NURBS Book*, 2nd ed., Springer 1997):
//! - knot span and basis functions, A2.1 and A2.2 (shared with [`crate::spline`]); first
//!   derivatives of the basis functions, A2.3;
//! - a surface point and its first partial derivatives in homogeneous form with the quotient rule
//!   for the rational case, A4.3 and eq. (4.20);
//! - knot insertion on a surface, row by row (Boehm's algorithm), A5.3;
//! - global curve interpolation through points with chord-length parameters and averaged knots,
//!   A9.1, eqs. (9.5) and (9.8); surface interpolation as curve interpolation of the rows, then
//!   of the columns, A9.4;
//! - least-squares approximation with the knots of eq. (9.68), §9.4.1; surfaces by rows then
//!   columns, §9.4.3;
//! - closest point by Newton iteration from the nearest sample, §6.1, eq. (6.6).
//!
//! Sizes are capped ([`MAX_CVS`], [`MAX_DEGREE`]); every function returns `None` rather than
//! panicking on a bad definition.

use serde::{Deserialize, Serialize};

use crate::Vec3;
use crate::mat::Mat4;
use crate::spline::{basis, find_span, solve};

/// Control vertices in one surface, at most.
pub const MAX_CVS: usize = 10_000;
/// Degree in either direction, at most.
pub const MAX_DEGREE: usize = 11;

/// A NURBS surface. Control vertex `(i, j)` (row `i` along `u`, column `j` along `v`) is
/// `control[i * count_v + j]`; `weights` is empty for a non-rational surface.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NurbsSurface {
    pub degree_u: usize,
    pub degree_v: usize,
    pub count_u: usize,
    pub count_v: usize,
    pub control: Vec<Vec3>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub weights: Vec<f64>,
    pub knots_u: Vec<f64>,
    pub knots_v: Vec<f64>,
}

/// Clamped knots on [0, 1] for `n` control points of degree `p`, evenly spaced inside.
pub fn clamped_knots(n: usize, p: usize) -> Vec<f64> {
    let mut k = vec![0.0; p + 1];
    let inner = n.saturating_sub(p + 1);
    for i in 1..=inner {
        k.push(i as f64 / (inner + 1) as f64);
    }
    k.extend(std::iter::repeat_n(1.0, p + 1));
    k
}

fn finite3(p: Vec3) -> bool {
    p.x.is_finite() && p.y.is_finite() && p.z.is_finite()
}

/// Basis functions and their first derivatives at `u` (A2.3, first order).
fn ders(span: usize, u: f64, p: usize, knots: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let n = basis(span, u, p, knots);
    if p == 0 {
        return (n, vec![0.0]);
    }
    // The degree p - 1 functions give the derivatives: N'_{i,p} = p (N_{i,p-1} / (u_{i+p} - u_i)
    // - N_{i+1,p-1} / (u_{i+p+1} - u_{i+1})).
    let lower = basis(span, u, p - 1, knots);
    let k = |i: isize| knots.get(i.max(0) as usize).copied().unwrap_or(0.0);
    let mut d = vec![0.0; p + 1];
    for (r, dr) in d.iter_mut().enumerate() {
        let i = span as isize - p as isize + r as isize;
        let left = if r >= 1 { lower.get(r - 1).copied().unwrap_or(0.0) } else { 0.0 };
        let right = lower.get(r).copied().unwrap_or(0.0);
        let a = k(i + p as isize) - k(i);
        let b = k(i + p as isize + 1) - k(i + 1);
        let ta = if a.abs() > 1e-300 { left / a } else { 0.0 };
        let tb = if b.abs() > 1e-300 && r < p { right / b } else { 0.0 };
        *dr = p as f64 * (ta - tb);
    }
    (n, d)
}

impl NurbsSurface {
    /// A non-rational surface on clamped uniform knots.
    pub fn new(degree_u: usize, degree_v: usize, count_u: usize, count_v: usize, control: Vec<Vec3>) -> Option<NurbsSurface> {
        let s = NurbsSurface {
            degree_u,
            degree_v,
            count_u,
            count_v,
            knots_u: clamped_knots(count_u, degree_u),
            knots_v: clamped_knots(count_v, degree_v),
            control,
            weights: Vec::new(),
        };
        s.is_valid().then_some(s)
    }

    /// The bilinear patch through four corners (`p00` at (0, 0), `p10` at (1, 0), …).
    pub fn bilinear(p00: Vec3, p10: Vec3, p01: Vec3, p11: Vec3) -> Option<NurbsSurface> {
        NurbsSurface::new(1, 1, 2, 2, vec![p00, p01, p10, p11])
    }

    pub fn is_valid(&self) -> bool {
        let (du, dv, cu, cv) = (self.degree_u, self.degree_v, self.count_u, self.count_v);
        let knots_ok = |k: &[f64], n: usize, p: usize| {
            k.len() == n + p + 1 && k.iter().all(|x| x.is_finite()) && k.windows(2).all(|w| w[0] <= w[1]) && k.first() < k.last()
        };
        (1..=MAX_DEGREE).contains(&du)
            && (1..=MAX_DEGREE).contains(&dv)
            && cu > du
            && cv > dv
            && cu.checked_mul(cv).is_some_and(|n| n <= MAX_CVS && n == self.control.len())
            && self.control.iter().all(|p| finite3(*p))
            && (self.weights.is_empty() || (self.weights.len() == self.control.len() && self.weights.iter().all(|w| w.is_finite() && *w > 0.0)))
            && knots_ok(&self.knots_u, cu, du)
            && knots_ok(&self.knots_v, cv, dv)
    }

    pub fn domain_u(&self) -> (f64, f64) {
        (self.knots_u.get(self.degree_u).copied().unwrap_or(0.0), self.knots_u.get(self.count_u).copied().unwrap_or(1.0))
    }

    pub fn domain_v(&self) -> (f64, f64) {
        (self.knots_v.get(self.degree_v).copied().unwrap_or(0.0), self.knots_v.get(self.count_v).copied().unwrap_or(1.0))
    }

    /// Control vertex `(i, j)`.
    pub fn cv(&self, i: usize, j: usize) -> Option<Vec3> {
        self.control.get(i.checked_mul(self.count_v)?.checked_add(j)?).copied()
    }

    fn w(&self, i: usize, j: usize) -> f64 {
        if self.weights.is_empty() { 1.0 } else { self.weights.get(i * self.count_v + j).copied().unwrap_or(1.0) }
    }

    /// The point and its first partials `S_u`, `S_v` at `(u, v)` (clamped to the domain).
    pub fn derivs(&self, u: f64, v: f64) -> Option<(Vec3, Vec3, Vec3)> {
        if !self.is_valid() || !u.is_finite() || !v.is_finite() {
            return None;
        }
        let (u0, u1) = self.domain_u();
        let (v0, v1) = self.domain_v();
        let (u, v) = (u.clamp(u0, u1), v.clamp(v0, v1));
        let su = find_span(self.count_u - 1, self.degree_u, u, &self.knots_u);
        let sv = find_span(self.count_v - 1, self.degree_v, v, &self.knots_v);
        let (nu, du) = ders(su, u, self.degree_u, &self.knots_u);
        let (nv, dv) = ders(sv, v, self.degree_v, &self.knots_v);
        // Homogeneous sums: A = Σ N N w P, W = Σ N N w, and their u and v derivatives.
        let (mut a, mut au, mut av) = (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
        let (mut w, mut wu, mut wv) = (0.0, 0.0, 0.0);
        for k in 0..=self.degree_u {
            let i = (su + k).checked_sub(self.degree_u)?;
            for l in 0..=self.degree_v {
                let j = (sv + l).checked_sub(self.degree_v)?;
                let p = self.cv(i, j)?;
                let wt = self.w(i, j);
                let (bu, bdu) = (nu.get(k).copied().unwrap_or(0.0), du.get(k).copied().unwrap_or(0.0));
                let (bv, bdv) = (nv.get(l).copied().unwrap_or(0.0), dv.get(l).copied().unwrap_or(0.0));
                a = a + p * (bu * bv * wt);
                au = au + p * (bdu * bv * wt);
                av = av + p * (bu * bdv * wt);
                w += bu * bv * wt;
                wu += bdu * bv * wt;
                wv += bu * bdv * wt;
            }
        }
        if w.abs() < 1e-300 {
            return None;
        }
        let s = a * (1.0 / w);
        let s_u = (au - s * wu) * (1.0 / w);
        let s_v = (av - s * wv) * (1.0 / w);
        (finite3(s) && finite3(s_u) && finite3(s_v)).then_some((s, s_u, s_v))
    }

    pub fn point(&self, u: f64, v: f64) -> Option<Vec3> {
        self.derivs(u, v).map(|d| d.0)
    }

    /// The unit normal `S_u × S_v` (None where the surface is degenerate).
    pub fn normal(&self, u: f64, v: f64) -> Option<Vec3> {
        let (_, a, b) = self.derivs(u, v)?;
        let n = a.cross(b);
        (n.len() > 1e-12).then(|| n.normalized())
    }

    /// Points on an `(nu + 1) × (nv + 1)` grid over the domain, row by row (`u` outer).
    pub fn grid(&self, nu: usize, nv: usize) -> Option<Vec<Vec<Vec3>>> {
        let (nu, nv) = (nu.clamp(1, 400), nv.clamp(1, 400));
        let (u0, u1) = self.domain_u();
        let (v0, v1) = self.domain_v();
        let mut rows = Vec::with_capacity(nu + 1);
        for a in 0..=nu {
            let u = u0 + (u1 - u0) * a as f64 / nu as f64;
            let mut row = Vec::with_capacity(nv + 1);
            for b in 0..=nv {
                row.push(self.point(u, v0 + (v1 - v0) * b as f64 / nv as f64)?);
            }
            rows.push(row);
        }
        Some(rows)
    }

    /// The surface with every control vertex mapped by `m` (B-splines are affine invariant).
    pub fn transformed(&self, m: &Mat4) -> NurbsSurface {
        let mut s = self.clone();
        for p in &mut s.control {
            *p = m.apply(*p);
        }
        s
    }

    /// Insert knot `t` once in `u` (`along_u`) or `v` (A5.3): the shape is unchanged.
    pub fn insert_knot(&self, along_u: bool, t: f64) -> Option<NurbsSurface> {
        if !self.is_valid() || !t.is_finite() {
            return None;
        }
        let (p, n, knots) = if along_u { (self.degree_u, self.count_u, &self.knots_u) } else { (self.degree_v, self.count_v, &self.knots_v) };
        let (lo, hi) = (knots.get(p).copied()?, knots.get(n).copied()?);
        if !(t > lo && t < hi) || self.control.len() + if along_u { self.count_v } else { self.count_u } > MAX_CVS {
            return None;
        }
        let k = find_span(n - 1, p, t, knots);
        let alpha = |i: usize| -> f64 {
            let (ki, kip) = (knots.get(i).copied().unwrap_or(0.0), knots.get(i + p).copied().unwrap_or(1.0));
            if (kip - ki).abs() < 1e-300 { 0.0 } else { (t - ki) / (kip - ki) }
        };
        // One curve per line of control vertices across the insertion direction.
        let lines = if along_u { self.count_v } else { self.count_u };
        let mut out_pts: Vec<Vec<(Vec3, f64)>> = Vec::with_capacity(lines);
        for line in 0..lines {
            let hp = |i: usize| -> (Vec3, f64) {
                let (a, b) = if along_u { (i, line) } else { (line, i) };
                let w = self.w(a, b);
                (self.cv(a, b).unwrap_or(Vec3::ZERO) * w, w)
            };
            let mut q = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let h = if i + p <= k {
                    hp(i)
                } else if i > k {
                    hp(i - 1)
                } else {
                    let a = alpha(i);
                    let (p1, w1) = hp(i);
                    let (p0, w0) = hp(i - 1);
                    (p1 * a + p0 * (1.0 - a), w1 * a + w0 * (1.0 - a))
                };
                q.push(h);
            }
            out_pts.push(q);
        }
        let mut s = self.clone();
        let mut nk = knots.clone();
        nk.insert(k + 1, t);
        if along_u {
            s.count_u += 1;
            s.knots_u = nk;
        } else {
            s.count_v += 1;
            s.knots_v = nk;
        }
        let rational = !self.weights.is_empty();
        s.control = vec![Vec3::ZERO; s.count_u * s.count_v];
        s.weights = if rational { vec![1.0; s.count_u * s.count_v] } else { Vec::new() };
        for (line, q) in out_pts.iter().enumerate() {
            for (i, (hpnt, w)) in q.iter().enumerate() {
                let (a, b) = if along_u { (i, line) } else { (line, i) };
                let idx = a * s.count_v + b;
                if let Some(c) = s.control.get_mut(idx) {
                    *c = if w.abs() > 1e-300 { *hpnt * (1.0 / *w) } else { *hpnt };
                }
                if rational && let Some(x) = s.weights.get_mut(idx) {
                    *x = *w;
                }
            }
        }
        s.is_valid().then_some(s)
    }

    /// The parameters of the point on the surface closest to `p` (sample search, then Newton).
    pub fn closest(&self, p: Vec3) -> Option<(f64, f64)> {
        const N: usize = 24;
        let (u0, u1) = self.domain_u();
        let (v0, v1) = self.domain_v();
        let mut best = (u0, v0, f64::INFINITY);
        for a in 0..=N {
            for b in 0..=N {
                let (u, v) = (u0 + (u1 - u0) * a as f64 / N as f64, v0 + (v1 - v0) * b as f64 / N as f64);
                if let Some(q) = self.point(u, v) {
                    let d = (q - p).len();
                    if d < best.2 {
                        best = (u, v, d);
                    }
                }
            }
        }
        let (mut u, mut v) = (best.0, best.1);
        for _ in 0..20 {
            let Some((s, su, sv)) = self.derivs(u, v) else { break };
            let r = s - p;
            // Gauss–Newton on |S - P|²: the 2 × 2 normal equations of the tangent plane.
            let (a, b, c) = (su.dot(su), su.dot(sv), sv.dot(sv));
            let (f, g) = (-r.dot(su), -r.dot(sv));
            let det = a * c - b * b;
            if det.abs() < 1e-300 {
                break;
            }
            let (du, dv) = ((f * c - b * g) / det, (a * g - b * f) / det);
            u = (u + du).clamp(u0, u1);
            v = (v + dv).clamp(v0, v1);
            if du.abs() < 1e-12 && dv.abs() < 1e-12 {
                break;
            }
        }
        (u.is_finite() && v.is_finite()).then_some((u, v))
    }

    /// Lines of constant `u` (and of constant `v`), `count` each way, each `steps` pieces.
    pub fn isolines(&self, count: usize, steps: usize) -> Vec<Vec<Vec3>> {
        let (count, steps) = (count.clamp(2, 64), steps.clamp(2, 400));
        let (u0, u1) = self.domain_u();
        let (v0, v1) = self.domain_v();
        let mut out = Vec::new();
        for along_u in [true, false] {
            for c in 0..count {
                let f = c as f64 / (count - 1) as f64;
                let line: Option<Vec<Vec3>> = (0..=steps)
                    .map(|s| {
                        let g = s as f64 / steps as f64;
                        if along_u { self.point(u0 + (u1 - u0) * f, v0 + (v1 - v0) * g) } else { self.point(u0 + (u1 - u0) * g, v0 + (v1 - v0) * f) }
                    })
                    .collect();
                if let Some(l) = line {
                    out.push(l);
                }
            }
        }
        out
    }

    /// The surface interpolating a grid of points (rows along `u`), degrees capped by the counts
    /// (A9.4: the rows, then the columns of their control vertices).
    pub fn interpolate(rows: &[Vec<Vec3>], degree_u: usize, degree_v: usize) -> Option<NurbsSurface> {
        let cu = rows.len();
        let cv = rows.first()?.len();
        if cu < 2 || cv < 2 || rows.iter().any(|r| r.len() != cv) || cu.checked_mul(cv)? > MAX_CVS {
            return None;
        }
        let (du, dv) = (degree_u.clamp(1, cu - 1).min(MAX_DEGREE), degree_v.clamp(1, cv - 1).min(MAX_DEGREE));
        // Parameters averaged over the rows (and the columns), eq. (9.8)'s knots from them.
        let params = |lines: &[Vec<Vec3>]| -> Vec<f64> {
            let n = lines.first().map_or(0, Vec::len);
            let mut acc = vec![0.0; n];
            for l in lines {
                let t = chord_params(l);
                for (a, b) in acc.iter_mut().zip(t) {
                    *a += b / lines.len() as f64;
                }
            }
            acc
        };
        let cols: Vec<Vec<Vec3>> = (0..cv).map(|j| rows.iter().map(|r| r.get(j).copied().unwrap_or(Vec3::ZERO)).collect()).collect();
        let tv = params(rows);
        let tu = params(&cols);
        let kv = averaged_knots(&tv, dv);
        let ku = averaged_knots(&tu, du);
        // Rows: interpolate along v; then each column of those control vertices along u.
        let mut r1: Vec<Vec<Vec3>> = Vec::with_capacity(cu);
        for r in rows {
            r1.push(interpolate_with(r, &tv, &kv, dv)?);
        }
        let mut control = vec![Vec3::ZERO; cu * cv];
        for j in 0..cv {
            let col: Vec<Vec3> = r1.iter().map(|r| r.get(j).copied().unwrap_or(Vec3::ZERO)).collect();
            let c = interpolate_with(&col, &tu, &ku, du)?;
            for (i, p) in c.into_iter().enumerate() {
                if let Some(x) = control.get_mut(i * cv + j) {
                    *x = p;
                }
            }
        }
        let s = NurbsSurface { degree_u: du, degree_v: dv, count_u: cu, count_v: cv, control, weights: Vec::new(), knots_u: ku, knots_v: kv };
        s.is_valid().then_some(s)
    }

    /// The least-squares surface with `count_u × count_v` control vertices through a grid of
    /// points (§9.4.3): its ends interpolated, the inside fitted.
    pub fn fit(rows: &[Vec<Vec3>], degree_u: usize, degree_v: usize, count_u: usize, count_v: usize) -> Option<NurbsSurface> {
        let m = rows.len();
        let n = rows.first()?.len();
        if rows.iter().any(|r| r.len() != n) || count_u < 2 || count_v < 2 || count_u > m || count_v > n || count_u.checked_mul(count_v)? > MAX_CVS {
            return None;
        }
        let (du, dv) = (degree_u.clamp(1, count_u - 1).min(MAX_DEGREE), degree_v.clamp(1, count_v - 1).min(MAX_DEGREE));
        let tv: Vec<f64> = (0..n).map(|j| j as f64 / (n - 1) as f64).collect();
        let tu: Vec<f64> = (0..m).map(|i| i as f64 / (m - 1) as f64).collect();
        let kv = fit_knots(&tv, count_v, dv);
        let ku = fit_knots(&tu, count_u, du);
        let mut r1: Vec<Vec<Vec3>> = Vec::with_capacity(m);
        for r in rows {
            r1.push(least_squares(r, &tv, &kv, dv, count_v)?);
        }
        let mut control = vec![Vec3::ZERO; count_u * count_v];
        for j in 0..count_v {
            let col: Vec<Vec3> = r1.iter().map(|r| r.get(j).copied().unwrap_or(Vec3::ZERO)).collect();
            let c = least_squares(&col, &tu, &ku, du, count_u)?;
            for (i, p) in c.into_iter().enumerate() {
                if let Some(x) = control.get_mut(i * count_v + j) {
                    *x = p;
                }
            }
        }
        let s = NurbsSurface { degree_u: du, degree_v: dv, count_u, count_v, control, weights: Vec::new(), knots_u: ku, knots_v: kv };
        s.is_valid().then_some(s)
    }
}

/// Chord-length parameters on [0, 1] (eq. 9.5); even spacing for a degenerate line.
pub fn chord_params(pts: &[Vec3]) -> Vec<f64> {
    let n = pts.len();
    let d: Vec<f64> = pts.windows(2).map(|w| (w[1] - w[0]).len()).collect();
    let total: f64 = d.iter().sum();
    if !(total.is_finite() && total > 1e-12) || n < 2 {
        return (0..n).map(|i| if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 }).collect();
    }
    let mut t = Vec::with_capacity(n);
    let mut acc = 0.0;
    t.push(0.0);
    for x in &d {
        acc += x;
        t.push(acc / total);
    }
    if let Some(last) = t.last_mut() {
        *last = 1.0;
    }
    t
}

/// Knots averaged from the parameters (eq. 9.8).
fn averaged_knots(t: &[f64], p: usize) -> Vec<f64> {
    let n = t.len();
    let mut k = vec![0.0; p + 1];
    for j in 1..n.saturating_sub(p) {
        k.push(t.get(j..j + p).map_or(0.0, |s| s.iter().sum::<f64>() / p as f64));
    }
    k.extend(std::iter::repeat_n(1.0, p + 1));
    k
}

/// Knots for a least-squares fit of `count` control points to parameters `t` (eq. 9.68).
fn fit_knots(t: &[f64], count: usize, p: usize) -> Vec<f64> {
    let m = t.len();
    let mut k = vec![0.0; p + 1];
    let inner = count.saturating_sub(p + 1);
    let d = m as f64 / (count - p) as f64;
    for j in 1..=inner {
        let i = (j as f64 * d).floor() as usize;
        let a = j as f64 * d - i as f64;
        let ti = t.get(i).copied().unwrap_or(1.0);
        let tim = t.get(i.saturating_sub(1)).copied().unwrap_or(0.0);
        k.push((1.0 - a) * tim + a * ti);
    }
    k.extend(std::iter::repeat_n(1.0, p + 1));
    k
}

/// Basis matrix rows: `N_{j,p}(t_k)` for every control point `j`.
fn basis_row(t: f64, knots: &[f64], p: usize, count: usize) -> Vec<f64> {
    let span = find_span(count - 1, p, t, knots);
    let b = basis(span, t, p, knots);
    let mut row = vec![0.0; count];
    for (r, v) in b.into_iter().enumerate() {
        if let Some(x) = (span + r).checked_sub(p).and_then(|j| row.get_mut(j)) {
            *x = v;
        }
    }
    row
}

/// Control points of the curve of degree `p` on `knots` through `pts` at `t` (A9.1).
fn interpolate_with(pts: &[Vec3], t: &[f64], knots: &[f64], p: usize) -> Option<Vec<Vec3>> {
    let n = pts.len();
    let a: Vec<Vec<f64>> = t.iter().map(|x| basis_row(*x, knots, p, n)).collect();
    let mut out = vec![Vec3::ZERO; n];
    let xs = solve(a.clone(), pts.iter().map(|q| q.x).collect())?;
    let ys = solve(a.clone(), pts.iter().map(|q| q.y).collect())?;
    let zs = solve(a, pts.iter().map(|q| q.z).collect())?;
    for (i, o) in out.iter_mut().enumerate() {
        *o = Vec3::new(*xs.get(i)?, *ys.get(i)?, *zs.get(i)?);
    }
    out.iter().all(|q| finite3(*q)).then_some(out)
}

/// `count` control points of degree `p` on `knots` fitting `pts` at `t` in the least-squares
/// sense, the end points interpolated (§9.4.1, the normal equations).
fn least_squares(pts: &[Vec3], t: &[f64], knots: &[f64], p: usize, count: usize) -> Option<Vec<Vec3>> {
    let (first, last) = (*pts.first()?, *pts.last()?);
    if count == pts.len() {
        return interpolate_with(pts, t, knots, p);
    }
    let rows: Vec<Vec<f64>> = t.iter().map(|x| basis_row(*x, knots, p, count)).collect();
    let inner = count - 2;
    let mut nn = vec![vec![0.0; inner]; inner];
    let mut rhs = vec![Vec3::ZERO; inner];
    for (k, r) in rows.iter().enumerate().skip(1).take(pts.len().saturating_sub(2)) {
        let q = *pts.get(k)? - first * r.first().copied().unwrap_or(0.0) - last * r.get(count - 1).copied().unwrap_or(0.0);
        for a in 0..inner {
            let na = r.get(a + 1).copied().unwrap_or(0.0);
            if let Some(x) = rhs.get_mut(a) {
                *x = *x + q * na;
            }
            for b in 0..inner {
                if let Some(x) = nn.get_mut(a).and_then(|row| row.get_mut(b)) {
                    *x += na * r.get(b + 1).copied().unwrap_or(0.0);
                }
            }
        }
    }
    let mut out = vec![first];
    if inner > 0 {
        let xs = solve(nn.clone(), rhs.iter().map(|q| q.x).collect())?;
        let ys = solve(nn.clone(), rhs.iter().map(|q| q.y).collect())?;
        let zs = solve(nn, rhs.iter().map(|q| q.z).collect())?;
        for i in 0..inner {
            out.push(Vec3::new(*xs.get(i)?, *ys.get(i)?, *zs.get(i)?));
        }
    }
    out.push(last);
    out.iter().all(|q| finite3(*q)).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Vec3, b: Vec3, tol: f64) -> bool {
        (a - b).len() <= tol
    }

    /// A curved test surface: z = sin(x) cos(y) sampled on a grid and interpolated.
    fn wavy() -> NurbsSurface {
        let rows: Vec<Vec<Vec3>> =
            (0..7).map(|i| (0..6).map(|j| Vec3::new(i as f64, j as f64, (i as f64 * 0.5).sin() * (j as f64 * 0.4).cos())).collect()).collect();
        NurbsSurface::interpolate(&rows, 3, 3).unwrap()
    }

    #[test]
    fn a_bilinear_patch_is_its_corners() {
        let s = NurbsSurface::bilinear(Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0), Vec3::new(0.0, 5.0, 0.0), Vec3::new(10.0, 5.0, 2.0)).unwrap();
        assert!(near(s.point(1.0, 1.0).unwrap(), Vec3::new(10.0, 5.0, 2.0), 1e-12));
        assert!(near(s.point(0.5, 0.5).unwrap(), Vec3::new(5.0, 2.5, 0.5), 1e-12));
        let (_, su, sv) = s.derivs(0.0, 0.0).unwrap();
        assert!(near(su, Vec3::new(10.0, 0.0, 0.0), 1e-9) && near(sv, Vec3::new(0.0, 5.0, 0.0), 1e-9));
    }

    #[test]
    fn interpolation_passes_through_the_grid_and_derivatives_match_differences() {
        let s = wavy();
        let rows: Vec<Vec<Vec3>> =
            (0..7).map(|i| (0..6).map(|j| Vec3::new(i as f64, j as f64, (i as f64 * 0.5).sin() * (j as f64 * 0.4).cos())).collect()).collect();
        // Through every point at the chord parameters (the grid is even in x and y).
        for (i, r) in rows.iter().enumerate() {
            for (j, q) in r.iter().enumerate() {
                let p = s.closest(*q).unwrap();
                assert!(near(s.point(p.0, p.1).unwrap(), *q, 1e-6), "({i},{j})");
            }
        }
        let (u, v, h) = (0.37, 0.61, 1e-6);
        let (_, su, sv) = s.derivs(u, v).unwrap();
        let fu = (s.point(u + h, v).unwrap() - s.point(u - h, v).unwrap()) * (0.5 / h);
        let fv = (s.point(u, v + h).unwrap() - s.point(u, v - h).unwrap()) * (0.5 / h);
        assert!(near(su, fu, 1e-5) && near(sv, fv, 1e-5), "{su:?} {fu:?} / {sv:?} {fv:?}");
    }

    #[test]
    fn rational_derivatives_match_differences() {
        let mut s = wavy();
        s.weights = (0..s.control.len()).map(|i| 1.0 + 0.3 * ((i % 5) as f64)).collect();
        let (u, v, h) = (0.42, 0.33, 1e-6);
        let (_, su, sv) = s.derivs(u, v).unwrap();
        let fu = (s.point(u + h, v).unwrap() - s.point(u - h, v).unwrap()) * (0.5 / h);
        let fv = (s.point(u, v + h).unwrap() - s.point(u, v - h).unwrap()) * (0.5 / h);
        assert!(near(su, fu, 1e-5) && near(sv, fv, 1e-5));
    }

    #[test]
    fn knot_insertion_keeps_the_shape() {
        let s = wavy();
        for along_u in [true, false] {
            let t = s.insert_knot(along_u, 0.4).unwrap();
            assert_eq!(t.control.len(), s.control.len() + if along_u { s.count_v } else { s.count_u });
            for (u, v) in [(0.1, 0.2), (0.4, 0.4), (0.77, 0.93)] {
                assert!(near(t.point(u, v).unwrap(), s.point(u, v).unwrap(), 1e-9));
            }
        }
        assert!(s.insert_knot(true, 1.5).is_none() && s.insert_knot(true, f64::NAN).is_none());
    }

    #[test]
    fn a_fit_with_fewer_vertices_stays_close() {
        let s = wavy();
        let g = s.grid(20, 20).unwrap();
        let f = NurbsSurface::fit(&g, 3, 3, 6, 5).unwrap();
        assert_eq!((f.count_u, f.count_v), (6, 5));
        let worst = (0..=10)
            .flat_map(|a| (0..=10).map(move |b| (a as f64 / 10.0, b as f64 / 10.0)))
            .map(|(u, v)| (f.point(u, v).unwrap() - s.point(u, v).unwrap()).len())
            .fold(0.0, f64::max);
        assert!(worst < 0.05, "{worst}");
    }

    #[test]
    fn bad_definitions_are_refused() {
        assert!(NurbsSurface::new(3, 3, 2, 2, vec![Vec3::ZERO; 4]).is_none());
        assert!(NurbsSurface::new(1, 1, 2, 2, vec![Vec3::new(f64::NAN, 0.0, 0.0); 4]).is_none());
        let mut s = wavy();
        s.knots_u.reverse();
        assert!(s.point(0.5, 0.5).is_none());
        assert!(NurbsSurface::interpolate(&[vec![Vec3::ZERO]], 3, 3).is_none());
    }
}
