//! XPS path geometry (Microsoft XML Paper Specification 1.0, §5.5 "Geometries and Figures" and
//! §5.5.2 "Abbreviated Geometry Syntax"; the same in ECMA-388 §14): the `Data`/`Figures`
//! mini-language and the `PathGeometry`/`PathFigure` elements, read into figures (polylines in
//! page units, curves and arcs flattened), and the `RenderTransform` matrix.
//!
//! Abbreviated syntax: `F 0|1` (fill rule, ignored), `M`/`m` move, `L`/`l` line, `H`/`h` and
//! `V`/`v` horizontal and vertical lines, `C`/`c` cubic, `Q`/`q` quadratic and `S`/`s` smooth
//! cubic Béziers, `A`/`a` elliptical arcs, `Z`/`z` close. Upper case is absolute, lower case
//! relative to the current point; a command's operands may repeat (after a move, extra pairs are
//! lines); numbers may be packed without spaces (`M1-2L.5.5`).

use super::xml::Element;
use cadaddy_geom::Vec2;

/// Most points one path makes (hostile files).
pub(crate) const MAX_POINTS: usize = 4_000_000;
/// Segments a Bézier curve is flattened into.
const CURVE_STEPS: usize = 16;

/// One subpath: its points in order and whether it closes back to the first.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Figure {
    pub points: Vec<Vec2>,
    pub closed: bool,
}

/// An affine matrix in XPS order `m11,m12,m21,m22,offsetX,offsetY`: `x' = m11·x + m21·y + ox`,
/// `y' = m12·x + m22·y + oy`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Matrix(pub [f64; 6]);

impl Matrix {
    pub const IDENTITY: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn apply(&self, p: Vec2) -> Vec2 {
        let [a, b, c, d, e, f] = self.0;
        Vec2::new(a * p.x + c * p.y + e, b * p.x + d * p.y + f)
    }
    /// `self` first, then `outer`.
    pub fn then(&self, outer: &Matrix) -> Matrix {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = outer.0;
        Matrix([a * a2 + b * c2, a * b2 + b * d2, c * a2 + d * c2, c * b2 + d * d2, e * a2 + f * c2 + e2, e * b2 + f * d2 + f2])
    }
    /// `m11,m12,m21,m22,ox,oy` (commas and/or spaces); `None` unless six finite numbers.
    pub fn parse(s: &str) -> Option<Matrix> {
        let v: Vec<f64> = s.split([',', ' ', '\t', '\n', '\r']).filter(|t| !t.is_empty()).map(|t| t.parse::<f64>().ok()).collect::<Option<_>>()?;
        let a: [f64; 6] = v.try_into().ok()?;
        a.iter().all(|x| x.is_finite()).then_some(Matrix(a))
    }
}

/// The transform an element sets: its `RenderTransform` attribute, else a
/// `<X.RenderTransform><MatrixTransform Matrix="…"/></X.RenderTransform>` child.
pub(crate) fn render_transform(el: &Element) -> Option<Matrix> {
    if let Some(m) = el.attr("RenderTransform").and_then(Matrix::parse) {
        return Some(m);
    }
    let prop = format!("{}.RenderTransform", el.name);
    el.child(&prop).and_then(|p| p.child("MatrixTransform")).and_then(|m| m.attr("Matrix")).and_then(Matrix::parse)
}

/// Tokens of the abbreviated syntax: command letters and numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok {
    Cmd(u8),
    Num(f64),
}

fn tokens(s: &str) -> Result<Vec<Tok>, String> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        if c.is_ascii_whitespace() || c == b',' {
            i += 1;
        } else if c.is_ascii_alphabetic() && !matches!(c, b'e' | b'E') {
            out.push(Tok::Cmd(c));
            i += 1;
        } else if c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.') {
            // [sign] digits [. digits] [e [sign] digits]: a second `.` or a sign starts the next number.
            let start = i;
            if matches!(c, b'+' | b'-') {
                i += 1;
            }
            let mut dot = false;
            while let Some(&d) = b.get(i) {
                if d.is_ascii_digit() {
                    i += 1;
                } else if d == b'.' && !dot {
                    dot = true;
                    i += 1;
                } else {
                    break;
                }
            }
            if matches!(b.get(i), Some(b'e' | b'E')) {
                let mut j = i + 1;
                if matches!(b.get(j), Some(b'+' | b'-')) {
                    j += 1;
                }
                if b.get(j).is_some_and(u8::is_ascii_digit) {
                    while b.get(j).is_some_and(u8::is_ascii_digit) {
                        j += 1;
                    }
                    i = j;
                }
            }
            let t = s.get(start..i).unwrap_or("");
            let v = t.parse::<f64>().map_err(|_| format!("bad number `{t}` in path data"))?;
            if !v.is_finite() {
                return Err(format!("bad number `{t}` in path data"));
            }
            out.push(Tok::Num(v));
        } else {
            return Err(format!("unexpected `{}` in path data", char::from(c)));
        }
        if out.len() > MAX_POINTS * 2 {
            return Err("path data too long".into());
        }
    }
    Ok(out)
}

/// Collects figures while a path is read.
#[derive(Default)]
struct Pen {
    figures: Vec<Figure>,
    cur: Vec<Vec2>,
    at: Vec2,
    start: Vec2,
    total: usize,
}

impl Pen {
    fn flush(&mut self, closed: bool) {
        let pts = std::mem::take(&mut self.cur);
        if pts.len() >= 2 {
            self.figures.push(Figure { points: pts, closed });
        }
    }
    fn move_to(&mut self, p: Vec2) {
        self.flush(false);
        self.at = p;
        self.start = p;
    }
    fn line_to(&mut self, p: Vec2) -> Result<(), String> {
        if self.cur.is_empty() {
            self.cur.push(self.at);
        }
        self.cur.push(p);
        self.at = p;
        self.total += 1;
        if self.total > MAX_POINTS { Err("path has too many points".into()) } else { Ok(()) }
    }
    fn close(&mut self) {
        let closed = self.cur.len() >= 2;
        // A closed figure keeps its points once: the closing segment is implied.
        if closed && self.cur.len() > 2 && self.cur.last().zip(self.cur.first()).is_some_and(|(a, b)| a.dist(*b) < 1e-12) {
            self.cur.pop();
        }
        self.flush(closed);
        self.at = self.start;
    }
    fn cubic(&mut self, c1: Vec2, c2: Vec2, to: Vec2) -> Result<(), String> {
        let p0 = self.at;
        for k in 1..=CURVE_STEPS {
            let t = k as f64 / CURVE_STEPS as f64;
            let u = 1.0 - t;
            let p = if k == CURVE_STEPS { to } else { p0 * (u * u * u) + c1 * (3.0 * u * u * t) + c2 * (3.0 * u * t * t) + to * (t * t * t) };
            self.line_to(p)?;
        }
        Ok(())
    }
    fn quad(&mut self, c: Vec2, to: Vec2) -> Result<(), String> {
        let p0 = self.at;
        for k in 1..=CURVE_STEPS {
            let t = k as f64 / CURVE_STEPS as f64;
            let u = 1.0 - t;
            let p = if k == CURVE_STEPS { to } else { p0 * (u * u) + c * (2.0 * u * t) + to * (t * t) };
            self.line_to(p)?;
        }
        Ok(())
    }
    /// An elliptical arc to `to` (SVG/XPS endpoint form), flattened to at most 5° steps; its
    /// end point is exact.
    fn arc(&mut self, radii: Vec2, rot_deg: f64, large: bool, sweep: bool, to: Vec2) -> Result<(), String> {
        let from = self.at;
        let (mut rx, mut ry) = (radii.x.abs(), radii.y.abs());
        if from.dist(to) < 1e-12 {
            return Ok(());
        }
        if rx < 1e-12 || ry < 1e-12 {
            return self.line_to(to);
        }
        let (sn, cs) = rot_deg.to_radians().sin_cos();
        let d = (from - to) * 0.5;
        let x1 = cs * d.x + sn * d.y;
        let y1 = -sn * d.x + cs * d.y;
        let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
        if lambda > 1.0 {
            let k = lambda.sqrt();
            rx *= k;
            ry *= k;
        }
        let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
        let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
        let mut coef = if den > 0.0 { (num / den).max(0.0).sqrt() } else { 0.0 };
        if large == sweep {
            coef = -coef;
        }
        let cx1 = coef * rx * y1 / ry;
        let cy1 = -coef * ry * x1 / rx;
        let mid = (from + to) * 0.5;
        let center = Vec2::new(cs * cx1 - sn * cy1 + mid.x, sn * cx1 + cs * cy1 + mid.y);
        let ang = |ux: f64, uy: f64| uy.atan2(ux);
        let t1 = ang((x1 - cx1) / rx, (y1 - cy1) / ry);
        let t2 = ang((-x1 - cx1) / rx, (-y1 - cy1) / ry);
        let mut dt = t2 - t1;
        let tau = std::f64::consts::TAU;
        // Sweep 1: angles increase (clockwise on a y-down page).
        if sweep && dt < 0.0 {
            dt += tau;
        } else if !sweep && dt > 0.0 {
            dt -= tau;
        }
        let steps = ((dt.abs() / 5f64.to_radians()).ceil() as usize).clamp(1, 360);
        for k in 1..=steps {
            let p = if k == steps {
                to
            } else {
                let t = t1 + dt * k as f64 / steps as f64;
                let (ex, ey) = (rx * t.cos(), ry * t.sin());
                Vec2::new(cs * ex - sn * ey + center.x, sn * ex + cs * ey + center.y)
            };
            self.line_to(p)?;
        }
        Ok(())
    }
}

/// The figures of abbreviated path data, in the path's own coordinates.
pub(crate) fn parse_data(s: &str) -> Result<Vec<Figure>, String> {
    let toks = tokens(s)?;
    let mut pen = Pen::default();
    let mut i = 0usize;
    let mut cmd: Option<u8> = None;
    // The last cubic's second control point, for `S`.
    let mut last_c2: Option<Vec2> = None;
    let num = |i: &mut usize| -> Result<f64, String> {
        match toks.get(*i) {
            Some(Tok::Num(v)) => {
                *i += 1;
                Ok(*v)
            }
            _ => Err("path data: a command is missing a number".into()),
        }
    };
    while i < toks.len() {
        let c = match toks.get(i) {
            Some(Tok::Cmd(c)) => {
                i += 1;
                *c
            }
            Some(Tok::Num(_)) => match cmd {
                // Operands repeat the last command; extra pairs after a move are lines.
                Some(b'M') => b'L',
                Some(b'm') => b'l',
                Some(c) if !matches!(c, b'Z' | b'z' | b'F' | b'f') => c,
                _ => return Err("path data starts with a number".into()),
            },
            None => break,
        };
        cmd = Some(c);
        let rel = c.is_ascii_lowercase();
        let base = |pen: &Pen| if rel { pen.at } else { Vec2::ZERO };
        let pt = |i: &mut usize, pen: &Pen| -> Result<Vec2, String> {
            let (x, y) = (num(i)?, num(i)?);
            Ok(base(pen) + Vec2::new(x, y))
        };
        let mut c2 = None;
        match c.to_ascii_uppercase() {
            b'F' => {
                num(&mut i)?;
            }
            b'M' => {
                let p = pt(&mut i, &pen)?;
                pen.move_to(p);
            }
            b'L' => {
                let p = pt(&mut i, &pen)?;
                pen.line_to(p)?;
            }
            b'H' => {
                let x = num(&mut i)? + if rel { pen.at.x } else { 0.0 };
                pen.line_to(Vec2::new(x, pen.at.y))?;
            }
            b'V' => {
                let y = num(&mut i)? + if rel { pen.at.y } else { 0.0 };
                pen.line_to(Vec2::new(pen.at.x, y))?;
            }
            b'C' => {
                let (a, b2, to) = (pt(&mut i, &pen)?, pt(&mut i, &pen)?, pt(&mut i, &pen)?);
                pen.cubic(a, b2, to)?;
                c2 = Some(b2);
            }
            b'S' => {
                let a = last_c2.map(|q| pen.at * 2.0 - q).unwrap_or(pen.at);
                let (b2, to) = (pt(&mut i, &pen)?, pt(&mut i, &pen)?);
                pen.cubic(a, b2, to)?;
                c2 = Some(b2);
            }
            b'Q' => {
                let (a, to) = (pt(&mut i, &pen)?, pt(&mut i, &pen)?);
                pen.quad(a, to)?;
            }
            b'A' => {
                let radii = Vec2::new(num(&mut i)?, num(&mut i)?);
                let rot = num(&mut i)?;
                let (large, sweep) = (num(&mut i)? != 0.0, num(&mut i)? != 0.0);
                let to = pt(&mut i, &pen)?;
                pen.arc(radii, rot, large, sweep, to)?;
            }
            b'Z' => pen.close(),
            other => return Err(format!("unknown path command `{}`", char::from(other))),
        }
        last_c2 = c2;
    }
    pen.flush(false);
    Ok(pen.figures)
}

fn point(s: &str) -> Option<Vec2> {
    let mut it = s.split([',', ' ']).filter(|t| !t.is_empty()).map(|t| t.parse::<f64>().ok().filter(|v| v.is_finite()));
    Some(Vec2::new(it.next()??, it.next()??))
}

fn points(s: &str) -> Result<Vec<Vec2>, String> {
    let toks = tokens(s)?;
    let nums: Vec<f64> =
        toks.iter().map(|t| if let Tok::Num(v) = t { Ok(*v) } else { Err("a letter in a point list".to_string()) }).collect::<Result<_, _>>()?;
    Ok(nums.chunks_exact(2).map(|c| Vec2::new(c.first().copied().unwrap_or(0.0), c.get(1).copied().unwrap_or(0.0))).collect())
}

/// The figures of a `<PathGeometry>` element: its `Figures` (abbreviated syntax) and its
/// `PathFigure` children (`StartPoint`, `IsClosed`; `PolyLineSegment`, `PolyBezierSegment`,
/// `PolyQuadraticBezierSegment` and `ArcSegment`), through its `Transform` matrix.
pub(crate) fn parse_geometry(g: &Element) -> Result<Vec<Figure>, String> {
    let mut out = match g.attr("Figures") {
        Some(f) => parse_data(f)?,
        None => Vec::new(),
    };
    for f in g.named("PathFigure") {
        let mut pen = Pen::default();
        pen.move_to(f.attr("StartPoint").and_then(point).unwrap_or(Vec2::ZERO));
        for seg in &f.children {
            let pts = seg.attr("Points").map(points).transpose()?.unwrap_or_default();
            match seg.name.as_str() {
                "PolyLineSegment" => {
                    for p in pts {
                        pen.line_to(p)?;
                    }
                }
                "PolyBezierSegment" => {
                    for c in pts.chunks_exact(3) {
                        if let [a, b, to] = c {
                            pen.cubic(*a, *b, *to)?;
                        }
                    }
                }
                "PolyQuadraticBezierSegment" => {
                    for c in pts.chunks_exact(2) {
                        if let [a, to] = c {
                            pen.quad(*a, *to)?;
                        }
                    }
                }
                "ArcSegment" => {
                    let to = seg.attr("Point").and_then(point).ok_or("an ArcSegment without a Point")?;
                    let size = seg.attr("Size").and_then(point).unwrap_or(Vec2::ZERO);
                    let rot = seg.attr("RotationAngle").and_then(|r| r.trim().parse::<f64>().ok()).filter(|r| r.is_finite()).unwrap_or(0.0);
                    let large = seg.attr("IsLargeArc").is_some_and(|v| v.trim().eq_ignore_ascii_case("true"));
                    let sweep = seg.attr("SweepDirection").is_some_and(|v| v.trim() == "Clockwise");
                    pen.arc(size, rot, large, sweep, to)?;
                }
                _ => {}
            }
        }
        if f.attr("IsClosed").is_some_and(|v| v.trim().eq_ignore_ascii_case("true")) {
            pen.close();
        }
        pen.flush(false);
        out.extend(pen.figures);
    }
    if let Some(m) = g.attr("Transform").and_then(Matrix::parse) {
        for f in &mut out {
            for p in &mut f.points {
                *p = m.apply(*p);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(f: &Figure) -> Vec<(f64, f64)> {
        f.points.iter().map(|p| ((p.x * 1e6).round() / 1e6, (p.y * 1e6).round() / 1e6)).collect()
    }

    #[test]
    fn absolute_relative_and_packed() {
        let f = parse_data("F1M10,20L30,20 30,40Zm5,5l10,0h5v5H0V0").unwrap();
        assert_eq!(f.len(), 2, "{f:?}");
        assert_eq!(pts(&f[0]), [(10.0, 20.0), (30.0, 20.0), (30.0, 40.0)]);
        assert!(f[0].closed);
        // After Z the pen is back at 10,20; `m5,5` is relative to it.
        assert_eq!(pts(&f[1]), [(15.0, 25.0), (25.0, 25.0), (30.0, 25.0), (30.0, 30.0), (0.0, 30.0), (0.0, 0.0)]);
        assert!(!f[1].closed);
        // Packed numbers: signs and second dots split them; implicit lines after a move.
        let f = parse_data("M.5.5-1-1 2e1,3E-1").unwrap();
        assert_eq!(pts(&f[0]), [(0.5, 0.5), (-1.0, -1.0), (20.0, 0.3)]);
    }

    #[test]
    fn curves_and_arcs_end_exactly() {
        let f = parse_data("M0,0 C0,10 10,10 10,0 S20,-10 20,0 Q25,5 30,0").unwrap();
        let p = &f[0].points;
        assert_eq!(p.len(), 1 + 3 * CURVE_STEPS);
        assert_eq!(p[CURVE_STEPS], Vec2::new(10.0, 0.0));
        assert_eq!(*p.last().unwrap(), Vec2::new(30.0, 0.0));
        // A half circle of radius 10 from (0,0) to (20,0): every point 10 from (10,0).
        let f = parse_data("M0,0 A10,10 0 0 1 20,0").unwrap();
        let p = &f[0].points;
        assert_eq!(p.len(), 1 + 36);
        assert!(p.iter().all(|q| (q.dist(Vec2::new(10.0, 0.0)) - 10.0).abs() < 1e-9), "{p:?}");
        assert_eq!(*p.last().unwrap(), Vec2::new(20.0, 0.0));
        // Sweep 1 goes through y < 0 (clockwise on a y-down page); relative `a` the same.
        assert!(p[18].y < -9.99, "{:?}", p[18]);
        let r = parse_data("m0,0a10,10,0,0,1,20,0").unwrap();
        assert_eq!(r[0].points, *p);
        // Radii too small are scaled up to reach the end point.
        let f = parse_data("M0,0 A1,1 0 0 0 20,0").unwrap();
        assert!(f[0].points[18].y > 9.99);
    }

    #[test]
    fn bad_data_is_an_error() {
        assert!(parse_data("10,10").is_err());
        assert!(parse_data("M10").is_err());
        assert!(parse_data("M1,1 X2,2").is_err());
        assert!(parse_data("M1,1 L1e999,0").is_err());
    }

    #[test]
    fn matrices_compose_child_first() {
        let child = Matrix::parse("1,0,0,1,10,0").unwrap();
        let parent = Matrix::parse("2 0 0 2 0 5").unwrap();
        assert_eq!(child.then(&parent).apply(Vec2::new(1.0, 1.0)), Vec2::new(22.0, 7.0));
        let rot = Matrix::parse("0,1,-1,0,0,0").unwrap();
        assert_eq!(rot.apply(Vec2::new(1.0, 0.0)), Vec2::new(0.0, 1.0));
        assert!(Matrix::parse("1,0,0,1").is_none());
        assert!(Matrix::parse("1,0,0,1,0,NaN").is_none());
    }

    #[test]
    fn path_geometry_elements() {
        let g = super::super::xml::parse(
            "<PathGeometry Transform='1,0,0,1,100,0'><PathFigure StartPoint='0,0' IsClosed='true'>\
             <PolyLineSegment Points='10,0 10,10'/></PathFigure>\
             <PathFigure StartPoint='0,0'><ArcSegment Point='20,0' Size='10,10' SweepDirection='Clockwise'/></PathFigure></PathGeometry>",
        )
        .unwrap();
        let f = parse_geometry(&g).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(pts(&f[0]), [(100.0, 0.0), (110.0, 0.0), (110.0, 10.0)]);
        assert!(f[0].closed && !f[1].closed);
        assert_eq!(*f[1].points.last().unwrap(), Vec2::new(120.0, 0.0));
    }
}
