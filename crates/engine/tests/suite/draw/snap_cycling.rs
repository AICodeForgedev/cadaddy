//! Tab snap cycling offers every snap point under the aperture, and object snap tracking shows
//! every acquired point's path through the tracking point.

use cadaddy_engine::Session;
use cadaddy_engine::geom::Vec2;
use cadaddy_engine::snap::mode;
use cadaddy_engine::snap::tracking::{Acquired, CursorQuery};
use serde_json::json;

fn line(s: &mut Session, a: (f64, f64), b: (f64, f64)) {
    s.execute("line", &json!({ "points": [[a.0, a.1], [b.0, b.1]] })).unwrap();
}

#[test]
fn tab_cycles_every_snap_point_under_the_aperture() {
    let mut s = Session::new();
    // Three line ends close together, a midpoint near too.
    line(&mut s, (0.0, 0.0), (10.0, 0.0));
    line(&mut s, (0.2, 0.1), (5.0, 5.0));
    line(&mut s, (-0.15, 0.2), (-5.0, 3.0));
    line(&mut s, (-0.3, -0.1), (0.5, -0.1));
    let st = s.state().unwrap();
    let space = st.edit_space();
    let cursor = Vec2::new(0.05, 0.05);
    let cands = cadaddy_engine::pointmod::snap_candidates(&st.doc, &space, cursor, 0.5, mode::END | mode::MID, None, false);
    let ends: Vec<Vec2> = cands.iter().filter(|h| h.mode == mode::END).map(|h| h.point).collect();
    for want in [Vec2::new(0.0, 0.0), Vec2::new(0.2, 0.1), Vec2::new(-0.15, 0.2), Vec2::new(-0.3, -0.1)] {
        assert!(ends.iter().any(|p| p.near(want, 1e-9)), "{want:?} not among {ends:?}");
    }
    assert!(cands.iter().any(|h| h.mode == mode::MID && h.point.near(Vec2::new(0.1, -0.1), 1e-9)), "the short line's midpoint");
    // The first is the snap the cursor takes without Tab; nothing twice.
    let best = cadaddy_engine::snap::osnap(&st.doc, &space, cursor, 0.5, mode::END | mode::MID, None, false).unwrap();
    assert!(cands[0].point.near(best.point, 1e-12));
    for (i, a) in cands.iter().enumerate() {
        assert!(!cands.iter().skip(i + 1).any(|b| b.mode == a.mode && b.point.near(a.point, 1e-9)));
    }
    // Only what is inside the aperture, and nothing with snaps off.
    let all = cadaddy_engine::snap::osnap_all(&st.doc, &space, cursor, 0.5, mode::END);
    assert!(all.iter().all(|h| h.point.dist(cursor) <= 0.5));
    assert!(cadaddy_engine::snap::osnap_all(&st.doc, &space, cursor, 0.5, 0).is_empty());
    assert!(cadaddy_engine::snap::osnap_all(&st.doc, &space, Vec2::new(f64::NAN, 0.0), 0.5, mode::END).is_empty());
}

#[test]
fn tracking_shows_every_acquired_points_path_through_the_point() {
    let mut s = Session::new();
    s.execute("setvar", &json!({ "name": "OSMODE", "value": mode::END })).unwrap();
    // Two acquired points on one horizontal line, and one off it.
    for at in [Vec2::new(0.0, 0.0), Vec2::new(20.0, 0.0), Vec2::new(5.0, 7.0)] {
        s.tracking.acquired.push(Acquired::Point { at, name: "Endpoint", ext: None });
    }
    let q = CursorQuery { base: None, deferred: false, corner: false, aperture: 0.5, tol: 0.1, now: 100.0, shift: false };
    let r = s.snap_cursor(Vec2::new(10.0, 0.05), &q);
    let t = r.track.expect("a tracking path");
    assert_eq!(t.paths.len(), 1, "the cursor slides on one path");
    assert!((t.point.y).abs() < 1e-9);
    let froms: Vec<Vec2> = t.paths.iter().chain(&t.also).map(|p| p.from).collect();
    assert!(froms.iter().any(|f| f.near(Vec2::new(0.0, 0.0), 1e-9)) && froms.iter().any(|f| f.near(Vec2::new(20.0, 0.0), 1e-9)), "{froms:?}");
    assert!(!froms.iter().any(|f| f.near(Vec2::new(5.0, 7.0), 1e-9)), "a path not through the point stays out");
    // Typed distances still go along the one path.
    assert!(t.along().is_some());
}
