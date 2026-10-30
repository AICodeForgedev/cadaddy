//! The exact outer loop of a pick-point boundary: the traced points' stretches along ellipses
//! and splines rebuilt as true curve pieces between the exact places the curves meet, so the
//! hatch's area and its file edges are exact rather than the chords' (the lines and arcs between
//! them stay lines and arcs).

use cadaddy_doc::{Drawing, EntityKind, Handle};
use cadaddy_geom::region::{Boundary, Run};
use cadaddy_geom::{CurveRef, Ellipse, Line, LoopEdge, Spline, Vec2, loop_area, refine_closest, refine_intersection, shoelace};

/// What a stretch of the traced loop follows.
enum Src {
    Line,
    Circle {
        center: Vec2,
        radius: f64,
    },
    Ellipse(Ellipse),
    Spline(Spline),
    /// Anything else (polylines, bridged gaps): the traced points as they are.
    Points,
}

impl Src {
    fn curve(&self) -> Option<CurveRef<'_>> {
        match self {
            Src::Circle { center, radius } => Some(CurveRef::Circle { center: *center, radius: *radius }),
            Src::Ellipse(e) => Some(CurveRef::Ellipse(*e)),
            Src::Spline(s) => Some(CurveRef::Spline(s)),
            Src::Line | Src::Points => None,
        }
    }
}

fn source_of(d: &Drawing, owners: &[Handle], run: &Run) -> Src {
    let Some(e) = run.source.and_then(|i| owners.get(i)).and_then(|h| d.entity(*h)) else { return Src::Points };
    // Curves stored in another plane are traced in the drawing plane: only the points are right.
    let x = e.common.extrusion;
    if x.x.abs() > 1e-12 || x.y.abs() > 1e-12 || x.z < 0.0 {
        return Src::Points;
    }
    match &e.kind {
        EntityKind::Line(_) => Src::Line,
        EntityKind::Circle(c) => Src::Circle { center: c.center.xy(), radius: c.radius },
        EntityKind::Arc(a) => Src::Circle { center: a.center.xy(), radius: a.radius },
        EntityKind::Ellipse(el) => {
            let ge = Ellipse { center: el.center.xy(), major: el.major.xy(), ratio: el.ratio, start: el.start, end: el.end };
            if ge.major.len() > 0.0 && ge.ratio > 0.0 && ge.ratio.is_finite() { Src::Ellipse(ge) } else { Src::Points }
        }
        EntityKind::Spline(sp) if sp.is_valid() && !sp.closed => Src::Spline(sp.as_ref().clone()),
        _ => Src::Points,
    }
}

/// `a` turned into (−π, π].
fn wrap(a: f64) -> f64 {
    (a + std::f64::consts::PI).rem_euclid(std::f64::consts::TAU) - std::f64::consts::PI
}

/// Line edges through `pts` (the stretch's traced points, its ends replaced by the exact ones).
fn chords(pts: &[Vec2], from: Vec2, to: Vec2) -> Vec<LoopEdge> {
    let mut p: Vec<Vec2> = pts.to_vec();
    if let Some(f) = p.first_mut() {
        *f = from;
    }
    if let Some(l) = p.last_mut() {
        *l = to;
    }
    p.windows(2)
        .filter_map(|w| Some(LoopEdge::Line { a: *w.first()?, b: *w.get(1)? }))
        .filter(|e| !e.start_point().near(e.end_point(), 1e-12))
        .collect()
}

/// The piece of a curve the stretch runs along, from `from` to `to` (exact points on it), going
/// the way the traced `pts` go; `None` when that can't be told for sure.
fn piece(src: &Src, pts: &[Vec2], from: Vec2, to: Vec2) -> Option<LoopEdge> {
    match src {
        Src::Line => Some(LoopEdge::Line { a: from, b: to }),
        Src::Points => None,
        Src::Circle { center, radius } => {
            let ang = |p: Vec2| (p - *center).angle();
            let travelled: f64 = pts.windows(2).filter_map(|w| Some(wrap(ang(*w.get(1)?) - ang(*w.first()?)))).sum();
            let ccw = travelled >= 0.0;
            let (s, e) = (ang(from), ang(to));
            agrees(s, e, ccw, travelled)?;
            Some(LoopEdge::Arc { center: *center, radius: *radius, start: s, end: e, ccw })
        }
        Src::Ellipse(el) => {
            let par = |p: Vec2| el.param_of(p);
            let travelled: f64 = pts.windows(2).filter_map(|w| Some(wrap(par(*w.get(1)?) - par(*w.first()?)))).sum();
            let ccw = travelled >= 0.0;
            let (s, e) =
                (refine_closest(&CurveRef::Ellipse(*el), from, Some(par(from))).1, refine_closest(&CurveRef::Ellipse(*el), to, Some(par(to))).1);
            agrees(s, e, ccw, travelled)?;
            Some(LoopEdge::Ellipse { center: el.center, major: el.major, ratio: el.ratio, start: s, end: e, ccw })
        }
        Src::Spline(sp) => {
            let c = CurveRef::Spline(sp);
            let (_, s) = refine_closest(&c, from, None);
            let (_, e) = refine_closest(&c, to, None);
            // Which way along the spline the traced points go: by the one furthest from the start.
            let far = pts.iter().copied().max_by(|a, b| a.dist(from).total_cmp(&b.dist(from)))?;
            let forward = refine_closest(&c, far, None).1 > s;
            let sub = if forward { sp.sub_between(s, e)? } else { sp.sub_between(e, s)?.reversed() };
            let edge = LoopEdge::Spline(sub);
            let size = sp.bounds().width().hypot(sp.bounds().height());
            let ok = edge.is_valid() && edge.start_point().near(from, 1e-9 * size.max(1.0)) && edge.end_point().near(to, 1e-9 * size.max(1.0));
            ok.then_some(edge)
        }
    }
}

/// Whether going `ccw` from angle/parameter `s` to `e` is the sweep the traced points made
/// (`travelled`, signed); a whole turn when the ends meet.
fn agrees(s: f64, e: f64, ccw: bool, travelled: f64) -> Option<()> {
    let sweep = if ccw { cadaddy_geom::ccw_sweep(s, e) } else { cadaddy_geom::ccw_sweep(e, s) };
    let sweep = if sweep < 1e-9 { std::f64::consts::TAU } else { sweep };
    ((sweep - travelled.abs()).abs() < 0.2).then_some(())
}

/// The outer loop of `b` as exact edges, or `None` when it follows no ellipse or spline (the
/// traced points are as exact as lines and arcs need) or can't be rebuilt safely. `reach` is how
/// far a traced corner may be from the exact place its two curves meet.
pub(super) fn outer_edges(d: &Drawing, owners: &[Handle], b: &Boundary, reach: f64) -> Option<Vec<LoopEdge>> {
    let runs = &b.outer_runs;
    let srcs: Vec<Src> = runs.iter().map(|r| source_of(d, owners, r)).collect();
    if !srcs.iter().any(|s| matches!(s, Src::Ellipse(_) | Src::Spline(_))) {
        return None;
    }
    let pts: Vec<Vec<Vec2>> = runs.iter().map(|r| r.points(&b.outer).collect()).collect();
    let n = runs.len();
    // The curve of a stretch at one end: the true curve, else the line of its end chord.
    let at = |k: usize, end: bool| -> Option<CurveRef<'_>> {
        let (src, p) = (srcs.get(k)?, pts.get(k)?);
        if let Some(c) = src.curve() {
            return Some(c);
        }
        let (a, b) = if end { (p.get(p.len().checked_sub(2)?)?, p.last()?) } else { (p.first()?, p.get(1)?) };
        Some(CurveRef::Line(Line::new(*a, *b)))
    };
    // Where stretch k starts: its traced first point moved to the exact meeting with the one before.
    let corners: Vec<Vec2> = (0..n)
        .map(|k| {
            let p0 = pts.get(k).and_then(|p| p.first()).copied()?;
            let prev = (k + n - 1) % n;
            let (a, c) = (at(prev, true)?, at(k, false)?);
            Some(refine_intersection(&a, &c, p0, reach).unwrap_or(p0))
        })
        .collect::<Option<_>>()?;
    let mut edges = Vec::new();
    for k in 0..n {
        let (p, src) = (pts.get(k)?, srcs.get(k)?);
        let (from, to) = (*corners.get(k)?, *corners.get((k + 1) % n)?);
        match piece(src, p, from, to) {
            Some(e) => edges.push(e),
            None => edges.extend(chords(p, from, to)),
        }
    }
    // Safe only when it closes up and encloses what the traced points do.
    let closed = edges.iter().zip(edges.iter().cycle().skip(1)).all(|(a, b)| a.end_point().near(b.start_point(), reach));
    let (exact, traced) = (loop_area(&edges), shoelace(&b.outer));
    (closed && edges.iter().all(LoopEdge::is_valid) && (exact - traced).abs() <= 0.05 * traced.abs()).then_some(edges)
}
