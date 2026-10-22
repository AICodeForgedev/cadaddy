//! Profile analysis: winding direction, area, containment.
//!
//! A *profile* is a closed [`Polyline`] of lines and arcs.  Its signed area tells us whether it
//! winds counter-clockwise (positive, an outer boundary) or clockwise (negative, a hole).
//! [`contains`] checks whether one profile lies inside another (used by [`super::CutOrder::InsideOut`]).

use cadaddy_geom::{Polyline, Segment, Vec2};

/// Signed area of a closed polyline: positive when its vertices are counter-clockwise, negative
/// when clockwise.  For a polyline with arcs the area is the shoelace sum of the line chords plus
/// the signed area of each circular segment.
pub fn signed_area(pl: &Polyline) -> f64 {
    let segs = pl.segments();
    let mut area = 0.0_f64;
    for s in &segs {
        let (a, b) = (s.start(), s.end());
        // Shoelace term for the chord.
        area += a.x * b.y - b.x * a.y;
        // Circular-segment correction for arcs.
        if let Segment::Arc { arc, ccw } = s {
            let sweep = arc.sweep();
            let seg_area = arc.radius * arc.radius * (sweep - sweep.sin()) / 2.0;
            area += if *ccw { seg_area } else { -seg_area };
        }
    }
    area / 2.0
}

/// Whether `inner` is geometrically inside `outer` (by testing a point on `inner` against
/// `outer`'s winding number).  Both must be closed.
pub fn contains(outer: &Polyline, inner: &Polyline) -> bool {
    let p = inner.vertices.first().map(|v| v.p).unwrap_or(Vec2::ZERO);
    winding_number(outer, p) != 0
}

/// Whether point `p` lies inside the closed polyline `pl` (non-zero winding).
pub fn contains_point(pl: &Polyline, p: Vec2) -> bool {
    winding_number(pl, p) != 0
}

/// Winding number of point `p` with respect to the closed polyline `pl`.  Non-zero means inside
/// (even-odd parity: odd means inside under the non-zero rule used by most CNC).
fn winding_number(pl: &Polyline, p: Vec2) -> i32 {
    let segs = pl.segments();
    let mut wn = 0i32;
    for s in &segs {
        let (a, b) = (s.start(), s.end());
        // For arcs we sample along the arc; for lines a single crossing test is enough.
        let pts = match s {
            Segment::Line(_) => vec![a, b],
            Segment::Arc { .. } => {
                let n = ((s.len() / 1.0).ceil() as usize).clamp(4, 64);
                (0..=n).map(|i| s.at(i as f64 / n as f64)).collect()
            }
        };
        for w in pts.windows(2) {
            let (Some(pa), Some(pb)) = (w.first(), w.get(1)) else { continue };
            if pa.y <= p.y {
                if pb.y > p.y && cross(*pa, *pb, p) > 0.0 {
                    wn += 1;
                }
            } else if pb.y <= p.y && cross(*pa, *pb, p) < 0.0 {
                wn -= 1;
            }
        }
    }
    wn
}

fn cross(a: Vec2, b: Vec2, p: Vec2) -> f64 {
    (b.x - a.x) * (p.y - a.y) - (p.x - a.x) * (b.y - a.y)
}

/// The arc-length of a closed polyline.
pub fn perimeter(pl: &Polyline) -> f64 {
    pl.segments().iter().map(|s| s.len()).sum()
}

/// Centroid (barycentre) of a closed polyline's boundary, weighted by segment length.  Used for
/// nearest-neighbour ordering.
pub fn centroid(pl: &Polyline) -> Vec2 {
    let segs = pl.segments();
    let mut cx = 0.0_f64;
    let mut cy = 0.0_f64;
    let mut total = 0.0_f64;
    for s in &segs {
        let l = s.len();
        let m = s.mid();
        cx += m.x * l;
        cy += m.y * l;
        total += l;
    }
    if total > 0.0 { Vec2::new(cx / total, cy / total) } else { Vec2::ZERO }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadaddy_geom::PolyVertex;

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Polyline {
        Polyline {
            vertices: vec![
                PolyVertex::new(Vec2::new(x0, y0)),
                PolyVertex::new(Vec2::new(x1, y0)),
                PolyVertex::new(Vec2::new(x1, y1)),
                PolyVertex::new(Vec2::new(x0, y1)),
            ],
            closed: true,
        }
    }

    #[test]
    fn ccw_rectangle_has_positive_area() {
        let r = rect(0.0, 0.0, 10.0, 5.0);
        let a = signed_area(&r);
        assert!((a - 50.0).abs() < 1e-9, "area = {a}");
    }

    #[test]
    fn cw_rectangle_has_negative_area() {
        // Reversing vertex order flips the sign.
        let mut r = rect(0.0, 0.0, 10.0, 5.0);
        r.vertices.reverse();
        let a = signed_area(&r);
        assert!((a + 50.0).abs() < 1e-9, "area = {a}");
    }

    #[test]
    fn containment_test() {
        let outer = rect(0.0, 0.0, 20.0, 20.0);
        let inner = rect(5.0, 5.0, 15.0, 15.0);
        let outside = rect(25.0, 25.0, 30.0, 30.0);
        assert!(contains(&outer, &inner));
        assert!(!contains(&outer, &outside));
        assert!(!contains(&inner, &outer));
    }

    #[test]
    fn perimeter_of_rectangle() {
        let r = rect(0.0, 0.0, 10.0, 5.0);
        let p = perimeter(&r);
        assert!((p - 30.0).abs() < 1e-9, "perimeter = {p}");
    }

    #[test]
    fn centroid_of_rectangle() {
        let r = rect(0.0, 0.0, 10.0, 6.0);
        let c = centroid(&r);
        assert!((c.x - 5.0).abs() < 1e-9, "cx = {}", c.x);
        assert!((c.y - 3.0).abs() < 1e-9, "cy = {}", c.y);
    }
}
