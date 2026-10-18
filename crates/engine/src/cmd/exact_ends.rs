//! TRIM and EXTEND against an ellipse or a spline. Cutting and boundary edges reach TRIM and
//! EXTEND as segments, so an ellipse or a spline arrives as the chords of its tessellation and a
//! new end lands on a chord, a little off the curve. This moves every new end onto the curve
//! itself ([`cadaddy_geom::refine_intersection`]).

use cadaddy_doc::{EntityKind, Prim};
use cadaddy_geom::{CurveRef, Ellipse, Line, TAU, Vec2, arc_to_bulge, bulge_to_arc, norm_angle, refine_intersection};

#[derive(Clone, Copy, PartialEq)]
enum End {
    Start,
    End,
}

/// The two ends of an object TRIM or EXTEND can leave with a new end (`None`: nothing to polish,
/// a closed curve or a spline).
fn ends(k: &EntityKind) -> Option<(Vec2, Vec2)> {
    match k {
        EntityKind::Line(l) => Some((l.a.xy(), l.b.xy())),
        EntityKind::Arc(a) => {
            let c = a.center.xy();
            Some((c + Vec2::new(a.start.cos(), a.start.sin()) * a.radius, c + Vec2::new(a.end.cos(), a.end.sin()) * a.radius))
        }
        EntityKind::Ellipse(e) => {
            let ge = ellipse(e);
            (!ge.is_full()).then(|| (ge.at_param(ge.start), ge.at_param(ge.end)))
        }
        EntityKind::LwPolyline(p) if !p.closed => Some((p.vertices.first()?.p, p.vertices.last()?.p)),
        _ => None,
    }
}

fn ellipse(e: &cadaddy_doc::Ellipse) -> Ellipse {
    Ellipse { center: e.center.xy(), major: e.major.xy(), ratio: e.ratio, start: e.start, end: e.end }
}

/// The curve an end lies on, as an equation (for a polyline, its first or last segment).
fn curve_at(k: &EntityKind, which: End) -> Option<CurveRef<'static>> {
    match k {
        EntityKind::Line(l) => Some(CurveRef::Line(Line::new(l.a.xy(), l.b.xy()))),
        EntityKind::Arc(a) => Some(CurveRef::Circle { center: a.center.xy(), radius: a.radius }),
        EntityKind::Ellipse(e) => Some(CurveRef::Ellipse(Ellipse { start: 0.0, end: TAU, ..ellipse(e) })),
        EntityKind::LwPolyline(p) => {
            let n = p.vertices.len();
            let (i, j) = if which == End::Start { (0, 1) } else { (n.checked_sub(2)?, n.checked_sub(1)?) };
            let (a, b) = (p.vertices.get(i)?, p.vertices.get(j)?);
            match bulge_to_arc(a.p, b.p, a.bulge) {
                Some((arc, _)) => Some(CurveRef::Circle { center: arc.center, radius: arc.radius }),
                None => Some(CurveRef::Line(Line::new(a.p, b.p))),
            }
        }
        _ => None,
    }
}

/// Move one end of `k` to `p` (a point on the same curve, very near the old end).
fn set_end(k: &mut EntityKind, which: End, p: Vec2) {
    match k {
        EntityKind::Line(l) => {
            let v = if which == End::Start { &mut l.a } else { &mut l.b };
            v.x = p.x;
            v.y = p.y;
        }
        EntityKind::Arc(a) => {
            let ang = norm_angle(a.center.xy().angle_to(p));
            if which == End::Start {
                a.start = ang;
            } else {
                a.end = ang;
            }
        }
        EntityKind::Ellipse(e) => {
            let t = norm_angle(ellipse(e).param_of(p));
            if which == End::Start {
                e.start = t;
            } else {
                e.end = t;
            }
        }
        EntityKind::LwPolyline(pl) => {
            let n = pl.vertices.len();
            let (i, j) = if which == End::Start { (0, 1) } else { (n.saturating_sub(2), n.saturating_sub(1)) };
            let (Some(a), Some(b)) = (pl.vertices.get(i).copied(), pl.vertices.get(j).copied()) else { return };
            // An arc segment keeps its centre and direction; only its sweep changes.
            let bulge = match bulge_to_arc(a.p, b.p, a.bulge) {
                Some((arc, ccw)) => {
                    let (from, to) = if which == End::Start { (p, b.p) } else { (a.p, p) };
                    let (fa, ta) = (arc.center.angle_to(from), arc.center.angle_to(to));
                    let sweep = if ccw { norm_angle(ta - fa) } else { -norm_angle(fa - ta) };
                    arc_to_bulge(sweep)
                }
                None => a.bulge,
            };
            let moved = if which == End::Start { i } else { j };
            if let Some(v) = pl.vertices.get_mut(moved) {
                v.p = p;
            }
            if let Some(v) = pl.vertices.get_mut(i) {
                v.bulge = bulge;
            }
        }
        _ => {}
    }
}

fn near(a: Vec2, b: Vec2, tol: f64) -> bool {
    a.dist(b) <= tol
}

/// Move each new end of `pieces` (an end `orig` didn't have) onto the ellipse or spline among
/// `curved` it was cut against, when there is one.
pub(crate) fn polish(mut pieces: Vec<EntityKind>, orig: &EntityKind, curved: &[Prim]) -> Vec<EntityKind> {
    if curved.is_empty() {
        return pieces;
    }
    let old = ends(orig);
    let size = |c: &Prim| match c {
        Prim::Ellipse(e) => e.major.len(),
        Prim::Spline(s) => {
            let b = s.bounds();
            b.width().hypot(b.height())
        }
        _ => 0.0,
    };
    for k in &mut pieces {
        let Some((s, e)) = ends(k) else { continue };
        for (which, p) in [(End::Start, s), (End::End, e)] {
            let scale = p.len().max(1.0);
            if old.is_some_and(|(a, b)| near(p, a, 1e-9 * scale) || near(p, b, 1e-9 * scale)) {
                continue;
            }
            let Some(mine) = curve_at(k, which) else { continue };
            // The nearest exact crossing with any curved edge, within the chords' error.
            let best = curved
                .iter()
                .filter_map(|c| {
                    let cr = crate::snap::curve_ref(c)?;
                    refine_intersection(&mine, &cr, p, (size(c) * 2e-3).max(1e-9))
                })
                .min_by(|a, b| a.dist(p).total_cmp(&b.dist(p)));
            if let Some(q) = best {
                set_end(k, which, q);
            }
        }
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use serde_json::json;

    /// How far a point is from the ellipse (exact, in its own frame).
    fn off_ellipse(e: &Ellipse, p: Vec2) -> f64 {
        let ux = e.major / e.major.len();
        let l = p - e.center;
        let (x, y) = (l.dot(ux) / e.major.len(), l.dot(ux.perp()) / (e.major.len() * e.ratio));
        ((x * x + y * y).sqrt() - 1.0).abs() * e.major.len()
    }

    #[test]
    fn trimming_a_line_at_an_ellipse_ends_exactly_on_it() {
        let mut s = Session::new();
        s.new_drawing(false);
        s.execute("ellipse", &json!({"center": [3.0, -2.0], "major": [8.0, 3.0], "ratio": 0.45})).unwrap();
        s.execute("line", &json!({"points": [[-10.0, 1.3], [15.0, -0.7]]})).unwrap();
        let ge = s
            .doc()
            .unwrap()
            .space(&s.space())
            .unwrap()
            .iter()
            .find_map(|e| match &e.kind {
                EntityKind::Ellipse(el) => Some(ellipse(el)),
                _ => None,
            })
            .unwrap();
        let h = s.doc().unwrap().space(&s.space()).unwrap().iter().find(|e| matches!(e.kind, EntityKind::Line(_))).unwrap().handle.hex();
        // Trim off the part of the line inside the ellipse.
        s.execute("trim", &json!({"handle": h, "pick": [3.0, 0.26]})).unwrap();
        let ends: Vec<Vec2> = s
            .doc()
            .unwrap()
            .space(&s.space())
            .unwrap()
            .iter()
            .filter_map(|e| match &e.kind {
                EntityKind::Line(l) => Some([l.a.xy(), l.b.xy()]),
                _ => None,
            })
            .flatten()
            .filter(|p| p.x > -9.0 && p.x < 14.0)
            .collect();
        assert_eq!(ends.len(), 2, "{ends:?}");
        for p in ends {
            assert!(off_ellipse(&ge, p) < 1e-10, "{} off the ellipse", off_ellipse(&ge, p));
        }
    }
}
