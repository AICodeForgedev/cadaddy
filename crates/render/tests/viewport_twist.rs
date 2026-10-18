//! A layout viewport with a view twist (DXF 51, VIEWTWIST) draws model space turned by it, on
//! screen and in plots (gaps U11).

use cadaddy_doc::{Common, Drawing, EntityKind, Line, Space, Viewport};
use cadaddy_geom::{Vec2, Vec3};
use cadaddy_render::{Options, VIEWPORT_CONTENT, build, build_plot};

/// A model line from 0,0 to 10,0 seen 1:1 through a 40 × 40 viewport centred on paper 50,50
/// and on model 5,0, turned by `twist` degrees.
fn drawing(twist: f64) -> Drawing {
    let mut d = Drawing::new_metric();
    d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(10.0, 0.0, 0.0) })).unwrap();
    let vp = Viewport {
        center: Vec3::new(50.0, 50.0, 0.0),
        width: 40.0,
        height: 40.0,
        view_center: Vec2::new(5.0, 0.0),
        view_height: 40.0,
        id: 2,
        twist: twist.to_radians(),
        ..Viewport::default()
    };
    d.add(&Space::Paper("Layout1".into()), Common::default(), EntityKind::Viewport(vp)).unwrap();
    d
}

/// The drawn ends of the model line on paper, on screen and plotted.
fn ends(d: &Drawing) -> Vec<Vec<Vec2>> {
    let space = Space::Paper("Layout1".into());
    [build(d, &space, &Options::default()), build_plot(d, &space, &Options::default())]
        .iter()
        .map(|l| l.prims.iter().filter(|p| p.handle == VIEWPORT_CONTENT).flat_map(|p| l.points(p).to_vec()).collect())
        .collect()
}

fn has(pts: &[Vec2], p: Vec2) -> bool {
    pts.iter().any(|q| q.near(p, 1e-6))
}

#[test]
fn twisted_viewport_draws_rotated() {
    for pts in ends(&drawing(0.0)) {
        assert!(has(&pts, Vec2::new(45.0, 50.0)) && has(&pts, Vec2::new(55.0, 50.0)), "{pts:?}");
    }
    // Turned 90° counterclockwise about the view centre: the line now runs up the sheet.
    for pts in ends(&drawing(90.0)) {
        assert!(has(&pts, Vec2::new(50.0, 45.0)) && has(&pts, Vec2::new(50.0, 55.0)), "{pts:?}");
    }
    for pts in ends(&drawing(30.0)) {
        let a = Vec2::new(-5.0, 0.0).rotate(30f64.to_radians()) + Vec2::new(50.0, 50.0);
        assert!(has(&pts, a), "{pts:?}");
    }
}

#[test]
fn twisted_viewport_still_clips_to_its_rectangle() {
    // A long line through a turned viewport is cut at the rectangle, not at the turned window.
    let mut d = drawing(45.0);
    d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(-500.0, 0.0, 0.0), b: Vec3::new(500.0, 0.0, 0.0) })).unwrap();
    for pts in ends(&d) {
        assert!(pts.iter().all(|p| p.x >= 30.0 - 1e-6 && p.x <= 70.0 + 1e-6 && p.y >= 30.0 - 1e-6 && p.y <= 70.0 + 1e-6), "{pts:?}");
        // The diagonal of the square is drawn (corner to corner).
        assert!(has(&pts, Vec2::new(30.0, 30.0)) && has(&pts, Vec2::new(70.0, 70.0)), "{pts:?}");
    }
}

#[test]
fn hostile_twist_does_not_panic() {
    for t in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 1e300] {
        let mut d = drawing(0.0);
        let sheet = Space::Paper("Layout1".into());
        let h = d.space(&sheet).unwrap().iter().next().unwrap().handle;
        d.space_mut(&sheet).unwrap().modify(h, |e| {
            if let EntityKind::Viewport(v) = &mut e.kind {
                v.twist = t;
            }
        });
        let _ = ends(&d);
    }
}
