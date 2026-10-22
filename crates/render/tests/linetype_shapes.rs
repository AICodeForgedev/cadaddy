//! Shapes in complex linetypes (Q152): CADaddy's own shape file draws FENCELINE2's boxes, TRACKS'
//! ties and BATTING's loops along the line, at the element's size, turning with the path; text
//! linetypes keep their spacing round a polyline's corners; a shape file that isn't there draws
//! nothing and leaves the gap.

use cadaddy_doc::{Common, DashElement, Drawing, EntityKind, Linetype, LwPolyline, Space, library};
use cadaddy_geom::{Bounds2, PolyVertex, Vec2};
use cadaddy_render::{Kind, Options, build};

fn standard(name: &str) -> Linetype {
    library::standard_linetypes().into_iter().find(|l| l.name == name).unwrap()
}

/// A drawing with one polyline through `pts` in linetype `lt`.
fn drawing(lt: Linetype, pts: &[Vec2]) -> Drawing {
    let mut d = Drawing::new_imperial();
    let name = lt.name.clone();
    d.linetypes.push(lt);
    let common = Common { linetype: name.into(), ..Default::default() };
    let vertices = pts.iter().map(|p| PolyVertex { p: *p, bulge: 0.0, start_width: 0.0, end_width: 0.0 }).collect();
    d.add(&Space::Model, common, EntityKind::LwPolyline(LwPolyline { vertices, closed: false, const_width: 0.0, elevation: 0.0, plinegen: true }))
        .unwrap();
    d
}

/// The drawn strokes that aren't on the polyline itself (the shapes and texts), as bounds.
fn marks(d: &Drawing, pts: &[Vec2]) -> Vec<Bounds2> {
    let l = build(d, &Space::Model, &Options::default());
    let on_path = |q: Vec2| {
        pts.windows(2).any(|w| {
            let (a, b) = (w[0], w[1]);
            let t = ((q - a).dot(b - a) / (b - a).dot(b - a)).clamp(0.0, 1.0);
            (a + (b - a) * t - q).len() < 1e-6
        })
    };
    l.prims
        .iter()
        .filter(|p| p.kind == Kind::Polyline)
        .map(|p| l.points(p))
        .filter(|s| s.iter().any(|q| !on_path(*q)))
        .map(|s| Bounds2::from_points(s.iter().copied()))
        .collect()
}

#[test]
fn fenceline2_draws_a_box_in_every_gap() {
    let pts = [Vec2::ZERO, Vec2::new(9.5, 0.0)];
    let m = marks(&drawing(standard("FENCELINE2"), &pts), &pts);
    // Pattern 0.25 on, 0.1 off (the box where it ends), 0.1 off, 0.5 on = 0.95: ten repeats.
    assert_eq!(m.len(), 10, "{m:?}");
    for (i, b) in m.iter().enumerate() {
        let c = i as f64 * 0.95 + 0.35;
        assert!((b.center().x - c).abs() < 1e-9 && b.center().y.abs() < 1e-9, "box {i}: {b:?}");
        assert!((b.width() - 0.1).abs() < 1e-9 && (b.height() - 0.1).abs() < 1e-9, "box {i}: {b:?}");
    }
}

#[test]
fn track_ties_cross_the_line_and_turn_with_the_path() {
    // Along x, then up: the ties are vertical on the first leg, horizontal on the second.
    let pts = [Vec2::ZERO, Vec2::new(3.0, 0.0), Vec2::new(3.0, 3.0)];
    let m = marks(&drawing(standard("TRACKS"), &pts), &pts);
    // A tie every 0.3 along the 6 units of path.
    assert_eq!(m.len(), 20, "{m:?}");
    let first = m.iter().filter(|b| b.center().y.abs() < 1e-9).count();
    let second = m.iter().filter(|b| (b.center().x - 3.0).abs() < 1e-9 && b.center().y > 0.1).count();
    assert_eq!(first + second, 20);
    for b in &m {
        if b.center().y.abs() < 1e-9 {
            assert!(b.width() < 1e-9 && (b.height() - 0.25).abs() < 1e-9, "{b:?}");
        } else {
            assert!(b.height() < 1e-9 && (b.width() - 0.25).abs() < 1e-9, "{b:?}");
        }
    }
}

#[test]
fn shapes_grow_with_the_linetype_scale_and_a_missing_file_draws_nothing() {
    let pts = [Vec2::ZERO, Vec2::new(20.0, 0.0)];
    let mut d = drawing(standard("FENCELINE2"), &pts);
    d.header.set_f64("LTSCALE", 2.0);
    let m = marks(&d, &pts);
    assert!(m.iter().all(|b| (b.width() - 0.2).abs() < 1e-9), "{m:?}");
    // The same linetype naming a shape file this computer doesn't have: the gaps stay empty.
    let mut lt = standard("FENCELINE2");
    for e in &mut lt.pattern {
        if e.shape.is_some() {
            e.style = Some("no-such-shapes.shx".into());
        }
    }
    assert!(marks(&drawing(lt, &pts), &pts).is_empty());
}

#[test]
fn text_keeps_its_spacing_round_a_polylines_corners() {
    // ---- G ---- every 0.95 of path, measured along the path through two corners.
    let gas = Linetype {
        name: "GAS".into(),
        description: String::new(),
        pattern: vec![
            DashElement::dash(0.5),
            DashElement { text: Some("G".into()), style: Some("Standard".into()), scale: 0.1, ..DashElement::dash(-0.2) },
            DashElement::dash(-0.25),
        ],
    };
    let pts = [Vec2::ZERO, Vec2::new(2.0, 0.0), Vec2::new(2.0, 2.0), Vec2::new(0.0, 2.0)];
    let m = marks(&drawing(gas, &pts), &pts);
    // Anchors at 0.7, 1.65, 2.6, 3.55, 4.5, 5.45 along the 6 units of path.
    let at = |s: f64| {
        if s <= 2.0 {
            Vec2::new(s, 0.0)
        } else if s <= 4.0 {
            Vec2::new(2.0, s - 2.0)
        } else {
            Vec2::new(6.0 - s, 2.0)
        }
    };
    assert_eq!(m.len(), 6, "{m:?}");
    for (i, b) in m.iter().enumerate() {
        let a = at(0.7 + 0.95 * i as f64);
        // The text starts at its anchor, along the leg it is on.
        assert!((b.min - a).len() < 0.12 || (b.max - a).len() < 0.12 || b.contains(a), "text {i} at {b:?}, anchor {a:?}");
    }
}
