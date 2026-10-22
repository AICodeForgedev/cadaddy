//! Live measurement labels (MEASURELABELS): what DIST, AREA and MEASUREGEOM measure, labelled on
//! the drawing beside the geometry while points are picked; with MEASURELABELS 2 the final labels
//! stay as multiline text.

use cadaddy_geom::Vec2;
use serde_json::json;

use super::{fmt_a, fmt_area, fmt_d, path, path_len};
use crate::{LiveLabel, Session};

/// The distance from `a` to `b` at the middle of the segment, with its angle and X and Y
/// components. `scale`: paper units per model unit when the points were picked through a layout
/// viewport (1 otherwise); the values are model distances.
pub(crate) fn distance(s: &Session, a: Vec2, b: Vec2, scale: f64) -> LiveLabel {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let d = (b - a) / scale;
    LiveLabel::new((a + b) * 0.5)
        .line("Distance", fmt_d(s, d.len()))
        .line("Angle", format!("{}{}", fmt_a(s, d.angle()), degree_sign(s)))
        .line("Delta X", fmt_d(s, d.x))
        .line("Delta Y", fmt_d(s, d.y))
}

/// The running length of the path through `pts` (DIST's Multiple points), at the middle of its
/// last segment: that segment's length and the total. `model`: the same path in model units when
/// it was picked through a layout viewport.
pub(crate) fn running(s: &Session, pts: &[Vec2], bulges: &[f64], model: Option<&[Vec2]>) -> Vec<LiveLabel> {
    let n = pts.len();
    let (Some(a), Some(b)) = (n.checked_sub(2).and_then(|i| pts.get(i)), pts.last()) else { return Vec::new() };
    let m = model.filter(|m| m.len() == n).unwrap_or(pts);
    let tail = n.saturating_sub(2);
    let seg = path_len(m.get(tail..).unwrap_or_default(), bulges.get(tail..).unwrap_or_default());
    vec![LiveLabel::new((*a + *b) * 0.5).line("Segment", fmt_d(s, seg)).line("Total", fmt_d(s, path_len(m, bulges)))]
}

/// The area and perimeter of the closed boundary through `pts`, at its middle; `total`: the
/// running total of AREA's Add (`sign` 1) or Subtract (-1) mode this area goes into. A single
/// side is labelled with its distance.
pub(crate) fn area(s: &Session, pts: &[Vec2], bulges: &[f64], total: Option<(f64, f64)>) -> Vec<LiveLabel> {
    let arcs = bulges.iter().any(|b| *b != 0.0);
    match pts {
        [] | [_] => Vec::new(),
        [a, b] if !arcs => vec![distance(s, *a, *b, 1.0)],
        _ => {
            let g = path(pts, bulges, true);
            let (a, per) = (g.area().abs(), g.len());
            let mid = pts.iter().fold(Vec2::ZERO, |acc, p| acc + *p) / pts.len() as f64;
            let mut l = LiveLabel::new(mid).line("Area", fmt_area(s, a)).line("Perimeter", fmt_d(s, per));
            if let Some((t, sign)) = total {
                l = l.line("Total area", fmt_area(s, t + sign * a));
            }
            if !(a.is_finite() && per.is_finite() && mid.is_finite()) {
                return Vec::new();
            }
            vec![l]
        }
    }
}

/// A degree sign after angles in decimal degrees (AUNITS 0); the other formats carry their own.
fn degree_sign(s: &Session) -> &'static str {
    if s.doc().map_or(0, |d| d.header.i64("AUNITS", 0)) == 0 { "°" } else { "" }
}

/// The angle at `vertex` between the directions to `a` and `b` (at most 180°), inside the angle
/// near its vertex.
pub(crate) fn angle(s: &Session, vertex: Vec2, a: Vec2, b: Vec2) -> Vec<LiveLabel> {
    let (da, db) = (a - vertex, b - vertex);
    if da.len() < 1e-12 || db.len() < 1e-12 {
        return Vec::new();
    }
    let mut ang = cadaddy_geom::norm_angle(db.angle() - da.angle());
    if ang > cadaddy_geom::PI {
        ang = cadaddy_geom::TAU - ang;
    }
    let deg = degree_sign(s);
    let mut mid = da.normalized() + db.normalized();
    if mid.len() < 1e-9 {
        // A straight angle: beside the vertex, square to the sides.
        mid = Vec2::new(-da.y, da.x);
    }
    let at = vertex + mid.normalized() * (da.len().min(db.len()) * 0.4);
    if !at.is_finite() {
        return Vec::new();
    }
    vec![LiveLabel::new(at).line("Angle", format!("{}{deg}", fmt_a(s, ang)))]
}

/// MEASURELABELS 2: the final labels stay on the drawing as multiline text, as tall as the live
/// labels look at the current zoom, each an undo step of its own. A transparent measurement (DIST
/// inside another command) leaves nothing behind.
pub(crate) fn keep(s: &mut Session, labels: &[LiveLabel]) {
    if s.settings.measurelabels != 2 || s.suspended.is_some() {
        return;
    }
    let px = s.pixel_size();
    let h = if px.is_finite() && px > 0.0 { px * 12.0 } else { s.doc().map_or(0.2, |d| d.header.f64("TEXTSIZE", 0.2)) };
    for l in labels.iter().take(16) {
        let r = s.execute("mtext", &json!({ "at": [l.at.x, l.at.y], "text": l.text(), "height": h, "attach": 5 }));
        if let Err(e) = r {
            s.echo(e.user_message());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_follow_the_drawing_units() {
        let mut s = Session::new();
        s.execute("setvar", &json!({ "name": "LUPREC", "value": 2 })).unwrap();
        let l = distance(&s, Vec2::new(0.0, 0.0), Vec2::new(3.0, 4.0), 1.0);
        assert_eq!(l.at, Vec2::new(1.5, 2.0));
        assert_eq!(l.text(), "Distance = 5.00\nAngle = 53°\nDelta X = 3.00\nDelta Y = 4.00");
        // Picked through a viewport at 1:2: model distances.
        assert_eq!(distance(&s, Vec2::ZERO, Vec2::new(3.0, 4.0), 0.5).lines[0].1, "10.00");
        s.execute("setvar", &json!({ "name": "LUNITS", "value": 4 })).unwrap();
        let a = area(&s, &[Vec2::ZERO, Vec2::new(120.0, 0.0), Vec2::new(120.0, 96.0), Vec2::new(0.0, 96.0)], &[], None);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].at, Vec2::new(60.0, 48.0));
        assert_eq!(a[0].lines[0], ("Area", "11520.0000 square in. (80.0000 square ft.)".to_string()));
        assert_eq!(a[0].lines[1].0, "Perimeter");
        assert_eq!(a[0].lines[1].1, "36'-0\"");
    }

    #[test]
    fn area_angle_and_running_totals() {
        let s = Session::new();
        let sq = [Vec2::ZERO, Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0), Vec2::new(0.0, 10.0)];
        let t = area(&s, &sq, &[], Some((50.0, -1.0)));
        assert_eq!(t[0].lines.get(2), Some(&("Total area", "-50.0000".to_string())));
        assert!(area(&s, &sq[..1], &[], None).is_empty());
        assert_eq!(area(&s, &sq[..2], &[], None)[0].lines[0], ("Distance", "10.0000".to_string()));
        let a = angle(&s, Vec2::ZERO, Vec2::new(10.0, 0.0), Vec2::new(0.0, 5.0));
        assert_eq!(a[0].text(), "Angle = 90°");
        assert!(a[0].at.x > 0.0 && a[0].at.y > 0.0, "inside the angle");
        // Reflex sides measure the smaller angle; a straight one is 180; a zero side has none.
        assert_eq!(angle(&s, Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(1.0, -1.0))[0].text(), "Angle = 45°");
        assert_eq!(angle(&s, Vec2::ZERO, Vec2::new(1.0, 0.0), Vec2::new(-1.0, 0.0))[0].text(), "Angle = 180°");
        assert!(angle(&s, Vec2::ZERO, Vec2::ZERO, Vec2::new(1.0, 0.0)).is_empty());
        let r = running(&s, &[Vec2::ZERO, Vec2::new(3.0, 4.0), Vec2::new(3.0, 10.0)], &[0.0, 0.0, 0.0], None);
        assert_eq!(r[0].text(), "Segment = 6.0000\nTotal = 11.0000");
        assert!(running(&s, &[Vec2::ZERO], &[0.0], None).is_empty());
    }

    #[test]
    fn hostile_points_label_nothing() {
        let s = Session::new();
        let big = Vec2::new(f64::MAX, f64::MAX);
        assert!(area(&s, &[Vec2::ZERO, big, Vec2::new(-f64::MAX, f64::MAX)], &[], None).is_empty());
        assert!(angle(&s, Vec2::ZERO, big, Vec2::new(f64::NAN, 1.0)).is_empty());
        let _ = distance(&s, Vec2::ZERO, big, 0.0);
        let _ = running(&s, &[big, -big], &[f64::NAN], None);
    }
}
