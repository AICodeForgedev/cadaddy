//! View twist (gaps U11): the model plan view and layout viewports keep a twist angle
//! (VIEWTWIST, DXF 51) and show model space turned by it; picks and snaps follow the turned
//! view; named views save and restore it.

use cadaddy_doc::{EntityKind, Viewport};
use cadaddy_engine::{Session, View};
use cadaddy_geom::{Vec2, Vec3};
use serde_json::json;

fn close(a: Vec2, b: Vec2) -> bool {
    a.near(b, 1e-6)
}

fn session() -> Session {
    let mut s = Session::new();
    s.new_drawing(true);
    s.viewport_px = (1600.0, 900.0);
    s.execute("line", &json!({"points": [[0, 0], [100, 0]]})).unwrap();
    s.execute("circle", &json!({"center": [50, 50], "radius": 10})).unwrap();
    s
}

/// The model point shown at view-plane point `q` (the screen's world coordinates).
fn model_at(s: &Session, q: Vec2) -> Vec2 {
    let st = s.state().unwrap();
    match st.projection() {
        Some(p) => p.unproject(q, 0.0).unwrap(),
        None => q,
    }
}

#[test]
fn twisting_the_plan_view_turns_it_about_the_screen_centre() {
    let mut s = session();
    s.state_mut().unwrap().set_view(View { center: Vec2::new(50.0, 0.0), height: 100.0 });
    let r = s.execute("view.twist", &json!({"angle": 90})).unwrap();
    assert!((r["twist"].as_f64().unwrap() - 90.0).abs() < 1e-9);
    let st = s.state().unwrap();
    let p = st.projection().expect("a twisted plan view is drawn on its view plane");
    assert!((st.twist() - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    // The point at the centre of the screen stayed; model +X now points up the screen.
    let c = st.view().center;
    assert!(close(model_at(&s, c), Vec2::new(50.0, 0.0)));
    assert!(close(p.project(Vec3::new(60.0, 0.0, 0.0)) - p.project(Vec3::new(50.0, 0.0, 0.0)), Vec2::new(0.0, 10.0)));
    // `by` adds; a full turn is no twist (and no projection).
    s.execute("view.twist", &json!({"by": 270})).unwrap();
    assert!(s.state().unwrap().projection().is_none());
    assert!(close(s.state().unwrap().view().center, Vec2::new(50.0, 0.0)));
    assert!(s.execute("view.twist", &json!({})).is_err());
    assert!(s.execute("view.twist", &json!({"angle": "x"})).is_err());
    assert!(s.execute("view.twist", &json!({"angle": 1e300})).is_err());
}

#[test]
fn zoom_extents_and_snaps_work_in_a_twisted_view() {
    let mut s = session();
    s.execute("view.twist", &json!({"angle": 30})).unwrap();
    s.zoom_extents().unwrap();
    let st = s.state().unwrap();
    let p = st.projection().unwrap();
    let v = st.view();
    // Both objects are on screen.
    let half = Vec2::new(v.height * 1600.0 / 900.0, v.height) / 2.0;
    for m in [Vec2::ZERO, Vec2::new(100.0, 0.0), Vec2::new(50.0, 60.0)] {
        let q = p.project(m.to3(0.0)) - v.center;
        assert!(q.x.abs() <= half.x && q.y.abs() <= half.y, "{m:?} off screen");
    }
}

#[test]
fn preset_views_and_plan_reset_the_twist() {
    let mut s = session();
    s.execute("view.twist", &json!({"angle": 45})).unwrap();
    s.execute("plan", &json!({})).unwrap();
    assert_eq!(s.state().unwrap().twist(), 0.0);
    assert!(s.state().unwrap().projection().is_none());
    s.execute("view.twist", &json!({"angle": 45})).unwrap();
    s.execute("view.dir", &json!({"preset": "swiso"})).unwrap();
    // No twist in 3D views (yet): refused there.
    assert!(s.execute("view.twist", &json!({"angle": 10})).is_err());
    s.execute("view.dir", &json!({"preset": "top"})).unwrap();
    assert_eq!(s.state().unwrap().twist(), 0.0);
}

#[test]
fn named_views_save_and_restore_the_twist() {
    let mut s = session();
    s.state_mut().unwrap().set_view(View { center: Vec2::new(20.0, 0.0), height: 50.0 });
    s.execute("view.twist", &json!({"angle": 60})).unwrap();
    let saved = s.state().unwrap().view();
    s.execute("view", &json!({"option": "save", "name": "TURNED"})).unwrap();
    let nv = s.doc().unwrap().views.iter().find(|v| v.name == "TURNED").cloned().unwrap();
    assert!((nv.twist - 60f64.to_radians()).abs() < 1e-12);
    s.execute("plan", &json!({})).unwrap();
    s.execute("view", &json!({"option": "restore", "name": "TURNED"})).unwrap();
    let st = s.state().unwrap();
    assert!((st.twist() - 60f64.to_radians()).abs() < 1e-9);
    assert!(close(st.view().center, saved.center));
    // The model point at the centre is the one that was there.
    assert!(close(model_at(&s, st.view().center), Vec2::new(20.0, 0.0)));
}

fn viewport(s: &Session, h: &str) -> Viewport {
    match s.doc().unwrap().layout("Layout1").unwrap().entities.iter().find(|e| e.handle.hex() == h).map(|e| &e.kind) {
        Some(EntityKind::Viewport(v)) => v.clone(),
        _ => panic!("not a viewport"),
    }
}

/// Layout1 with one 100 × 80 viewport showing model 0,0 at 1:1, entered with MSPACE.
fn in_viewport() -> (Session, String) {
    let mut s = session();
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    let r = s.execute("mview", &json!({"p1": [20, 20], "p2": [120, 100]})).unwrap();
    let h = r["viewports"][0].as_str().unwrap().to_string();
    s.execute("viewport.set", &json!({"handle": h, "scale": 1, "center": [0, 0]})).unwrap();
    s.execute("mspace", &json!({"handle": h})).unwrap();
    (s, h)
}

#[test]
fn a_twisted_viewport_maps_the_screen_through_its_twist() {
    let (mut s, h) = in_viewport();
    s.execute("view.twist", &json!({"angle": 90})).unwrap();
    let v = viewport(&s, &h);
    assert!((v.twist - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    // Turned about the viewport centre: the model point there stays.
    assert!(close(v.view_center, Vec2::ZERO));
    let st = s.state().unwrap();
    // The screen (view-plane) point over paper point P shows the model point the viewport
    // shows there.
    let pv = st.paper_view();
    let view = st.view();
    let k = view.height / pv.height;
    for paper in [Vec2::new(70.0, 60.0), Vec2::new(80.0, 60.0), Vec2::new(70.0, 90.0)] {
        let q = view.center + (paper - pv.center) * k;
        assert!(close(model_at(&s, q), v.to_model(paper).unwrap()), "{paper:?}");
    }
    // A pan stores the turned view back on the viewport: panning by +10 on screen moves the
    // view centre by the model vector that shows as +10 on screen.
    let before = viewport(&s, &h).view_center;
    let mut nv = st.view();
    nv.center += Vec2::new(10.0, 0.0);
    s.state_mut().unwrap().set_view(nv);
    let after = viewport(&s, &h).view_center;
    assert!(close(after - before, Vec2::new(10.0, 0.0).rotate(-std::f64::consts::FRAC_PI_2)), "{:?}", after - before);
    // ZOOM Extents fits the turned model in the viewport.
    s.cmdline("zoom e").unwrap();
    let v = viewport(&s, &h);
    for m in [Vec2::ZERO, Vec2::new(100.0, 0.0), Vec2::new(50.0, 60.0)] {
        let p = v.to_paper(m).unwrap();
        assert!((20.0 - 1e-6..=120.0 + 1e-6).contains(&p.x) && (20.0 - 1e-6..=100.0 + 1e-6).contains(&p.y), "{m:?} at {p:?}");
    }
    // Back on paper the sheet itself isn't turned.
    s.execute("pspace", &json!({})).unwrap();
    assert!(s.state().unwrap().projection().is_none());
}

/// An endpoint or nearest snap on Layout1's sheet near `at`.
fn snap(s: &Session, at: Vec2) -> Option<cadaddy_engine::snap::SnapHit> {
    cadaddy_engine::snap::osnap(s.doc().unwrap(), &cadaddy_doc::Space::Paper("Layout1".into()), at, 2.0, 1 | 512, None, false)
}

#[test]
fn snaps_through_a_twisted_viewport_land_on_paper_where_drawn() {
    let (mut s, h) = in_viewport();
    s.execute("view.twist", &json!({"angle": 90})).unwrap();
    s.execute("pspace", &json!({})).unwrap();
    let v = viewport(&s, &h);
    // The model line from 0,0 along +X runs up the sheet from the viewport's centre.
    let start = v.to_paper(Vec2::ZERO).unwrap();
    assert!(close(start, Vec2::new(70.0, 60.0)));
    let hit = snap(&s, start + Vec2::new(0.3, -0.3)).expect("an endpoint snap through the viewport");
    assert!(close(hit.point, start), "{:?}", hit.point);
    assert!(hit.viewport.is_some_and(|vp| close(vp.point, Vec2::ZERO)));
    // A nearest snap beside the (now vertical) line lands on it.
    let hit = snap(&s, Vec2::new(70.6, 85.0)).expect("a nearest snap through the viewport");
    assert!(close(hit.point, Vec2::new(70.0, 85.0)), "{:?}", hit.point);
    assert!(hit.viewport.is_some_and(|vp| close(vp.point, Vec2::new(25.0, 0.0))));
}

#[test]
fn selection_windows_follow_the_screen_in_a_twisted_view() {
    let mut s = session();
    s.state_mut().unwrap().set_view(View { center: Vec2::new(50.0, 25.0), height: 200.0 });
    s.execute("view.twist", &json!({"angle": 90})).unwrap();
    let line = s.doc().unwrap().model.iter().next().unwrap().handle;
    // Model +X is up the screen and screen x is -model y.
    let (a, b) = (Vec2::new(-5.0, 5.0), Vec2::new(105.0, -5.0));
    s.begin_window(a);
    s.idle_click(b, false).unwrap();
    // Screen x of a = -5, of b = 5: left to right, a window that holds the whole line.
    assert_eq!(s.selection(), vec![line]);
    s.set_selection(Vec::new());
    // A window that is narrow on screen around the line's middle: crossing (right to left).
    s.begin_window(Vec2::new(40.0, -5.0));
    s.idle_click(Vec2::new(60.0, 5.0), false).unwrap();
    assert_eq!(s.selection(), vec![line], "a crossing window takes the line it crosses");
    s.set_selection(Vec::new());
    // The same corners left to right on screen are a window: the line isn't inside.
    s.begin_window(Vec2::new(60.0, 5.0));
    s.idle_click(Vec2::new(40.0, -5.0), false).unwrap();
    assert!(s.selection().is_empty());
}
