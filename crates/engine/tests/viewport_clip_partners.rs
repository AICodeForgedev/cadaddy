//! A clipped viewport and its clip boundary act as one object (gaps F15): MOVE, COPY, ROTATE,
//! SCALE, MIRROR and ERASE of either take both, a copy gets its own linked pair, reshaping the
//! boundary by its grips refits the viewport, and one undo restores both.

use cadaddy_doc::{EntityKind, Handle, Space, Viewport};
use cadaddy_engine::Session;
use cadaddy_geom::{Bounds2, Vec2};
use serde_json::json;

fn layout_viewports(s: &Session) -> Vec<(Handle, Viewport)> {
    let d = s.doc().unwrap();
    d.layout("Layout1")
        .unwrap()
        .entities
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Viewport(v) if v.id != 1 => Some((e.handle, v.clone())),
            _ => None,
        })
        .collect()
}

fn viewport(s: &Session, h: Handle) -> Viewport {
    match s.doc().unwrap().entity(h).map(|e| &e.kind) {
        Some(EntityKind::Viewport(v)) => v.clone(),
        _ => panic!("no viewport {}", h.hex()),
    }
}

/// The boundary polyline's vertices.
fn outline(s: &Session, h: Handle) -> Vec<Vec2> {
    match s.doc().unwrap().entity(h).map(|e| &e.kind) {
        Some(EntityKind::LwPolyline(p)) => p.vertices.iter().map(|v| v.p).collect(),
        _ => panic!("no boundary {}", h.hex()),
    }
}

fn exists(s: &Session, h: Handle) -> bool {
    s.doc().unwrap().entity(h).is_some()
}

/// Model lines on a grid, Layout1 current with its default viewport erased, and a triangular
/// polygonal viewport. Returns (viewport, boundary).
fn session() -> (Session, Handle, Handle) {
    let mut s = Session::new();
    s.new_drawing(true);
    s.viewport_px = (1600.0, 900.0);
    for i in 0..=10 {
        let y = f64::from(i) * 10.0;
        s.execute("line", &json!({"points": [[0, y], [100, y]]})).unwrap();
    }
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    let old: Vec<String> = layout_viewports(&s).iter().map(|(h, _)| h.hex()).collect();
    if !old.is_empty() {
        s.execute("erase", &json!({"handles": old})).unwrap();
    }
    let r = s.execute("mview", &json!({"option": "polygonal", "points": [[10, 10], [110, 10], [60, 90]]})).unwrap();
    let vh = Handle::parse_hex(r["viewport"].as_str().unwrap()).unwrap();
    let bh = Handle::parse_hex(r["boundary"].as_str().unwrap()).unwrap();
    (s, vh, bh)
}

fn close(a: Vec2, b: Vec2) -> bool {
    a.near(b, 1e-6)
}

/// The viewport's rectangle sits on the boundary's extents.
fn fitted(s: &Session, vh: Handle, bh: Handle) -> bool {
    let v = viewport(s, vh);
    let b = Bounds2::from_points(outline(s, bh));
    close(v.center.xy(), b.center()) && (v.width - b.width()).abs() < 1e-6 && (v.height - b.height()).abs() < 1e-6
}

#[test]
fn moving_either_moves_both_and_the_view_goes_along() {
    let (mut s, vh, bh) = session();
    let before = viewport(&s, vh);
    let shown = before.to_model(Vec2::new(60.0, 40.0)).unwrap();
    // Select only the viewport: the boundary comes along.
    s.execute("move", &json!({"handles": [vh.hex()], "delta": [5, -3]})).unwrap();
    assert!(close(outline(&s, bh)[0], Vec2::new(15.0, 7.0)));
    assert!(fitted(&s, vh, bh));
    let v = viewport(&s, vh);
    assert_eq!(v.clip, Some(bh));
    // Model space moved with the sheet objects: the same model point is shown 5,-3 along.
    assert!(close(v.to_model(Vec2::new(65.0, 37.0)).unwrap(), shown));
    // Select only the boundary: the viewport comes along.
    s.execute("move", &json!({"handles": [bh.hex()], "delta": [-5, 3]})).unwrap();
    assert!(close(outline(&s, bh)[0], Vec2::new(10.0, 10.0)));
    assert!(close(viewport(&s, vh).center.xy(), before.center.xy()));
    assert!(fitted(&s, vh, bh));
}

#[test]
fn copy_makes_a_linked_pair() {
    let (mut s, vh, bh) = session();
    let r = s.execute("copy", &json!({"handles": [bh.hex()], "delta": [150, 0]})).unwrap();
    let made: Vec<Handle> = r["handles"].as_array().unwrap().iter().map(|h| Handle::parse_hex(h.as_str().unwrap()).unwrap()).collect();
    assert_eq!(made.len(), 2, "the copy is a viewport and its boundary");
    let vps = layout_viewports(&s);
    assert_eq!(vps.len(), 2);
    let (nvh, nv) = vps.iter().find(|(h, _)| *h != vh).cloned().unwrap();
    let nbh = nv.clip.expect("the copy is clipped");
    assert!(made.contains(&nvh) && made.contains(&nbh) && nbh != bh);
    assert!(close(outline(&s, nbh)[0], Vec2::new(160.0, 10.0)));
    assert!(fitted(&s, nvh, nbh));
    // The original keeps its own boundary, and the copy has its own id.
    assert_eq!(viewport(&s, vh).clip, Some(bh));
    assert_ne!(nv.id, viewport(&s, vh).id);
    // It shows the same model view.
    assert!(close(nv.view_center, viewport(&s, vh).view_center));
}

#[test]
fn rotating_turns_the_view_with_the_boundary() {
    let (mut s, vh, bh) = session();
    let before = viewport(&s, vh);
    let p = Vec2::new(60.0, 40.0);
    let shown = before.to_model(p).unwrap();
    s.execute("rotate", &json!({"handles": [vh.hex()], "base": [60, 40], "angle": 90})).unwrap();
    // The boundary turned about the base; the viewport took its extents and a 90° twist.
    assert!(close(outline(&s, bh)[0], Vec2::new(90.0, -10.0)));
    assert!(fitted(&s, vh, bh));
    let v = viewport(&s, vh);
    assert!((v.twist - std::f64::consts::FRAC_PI_2).abs() < 1e-9, "twist {}", v.twist);
    // The model point under the base point is still there, and one 10 units to the right on
    // the sheet before is now 10 units up.
    assert!(close(v.to_model(p).unwrap(), shown));
    let right = before.to_model(p + Vec2::new(10.0, 0.0)).unwrap();
    assert!(close(v.to_model(p + Vec2::new(0.0, 10.0)).unwrap(), right));
    // The view is drawn turned: a horizontal model line now runs vertically on the sheet.
    let d = s.doc().unwrap();
    let l = cadaddy_render::build(d, &Space::Paper("Layout1".into()), &cadaddy_render::Options::default());
    let segs: Vec<Vec<Vec2>> = l.prims.iter().filter(|q| q.handle == cadaddy_render::VIEWPORT_CONTENT).map(|q| l.points(q).to_vec()).collect();
    assert!(!segs.is_empty());
    assert!(segs.iter().filter(|q| q.len() >= 2).all(|q| (q[0].x - q[q.len() - 1].x).abs() < 1e-6), "content is not vertical");
}

#[test]
fn scaling_and_mirroring_keep_the_pair() {
    let (mut s, vh, bh) = session();
    s.execute("scale", &json!({"handles": [bh.hex()], "base": [10, 10], "factor": 0.5})).unwrap();
    assert!(close(outline(&s, bh)[1], Vec2::new(60.0, 10.0)));
    assert!(fitted(&s, vh, bh));
    let r = s.execute("mirror", &json!({"handles": [vh.hex()], "p1": [150, 0], "p2": [150, 10]})).unwrap();
    assert_eq!(r["handles"].as_array().unwrap().len(), 2);
    let (mvh, mv) = layout_viewports(&s).into_iter().find(|(h, _)| *h != vh).unwrap();
    let mbh = mv.clip.unwrap();
    assert!(close(outline(&s, mbh)[0], Vec2::new(290.0, 10.0)));
    assert!(fitted(&s, mvh, mbh));
    // A mirrored viewport isn't twisted (AutoCAD doesn't mirror what a viewport shows).
    assert_eq!(mv.twist, 0.0);
}

#[test]
fn erasing_either_erases_both_and_undo_restores_both() {
    let (mut s, vh, bh) = session();
    let r = s.execute("erase", &json!({"handles": [bh.hex()]})).unwrap();
    assert_eq!(r["erased"], 2);
    assert!(!exists(&s, vh) && !exists(&s, bh));
    s.undo().unwrap();
    assert!(exists(&s, vh) && exists(&s, bh));
    assert_eq!(viewport(&s, vh).clip, Some(bh));
    s.execute("erase", &json!({"handles": [vh.hex()]})).unwrap();
    assert!(!exists(&s, vh) && !exists(&s, bh));
    s.undo().unwrap();
    assert!(exists(&s, vh) && exists(&s, bh));
}

#[test]
fn undo_puts_a_moved_pair_back_in_one_step() {
    let (mut s, vh, bh) = session();
    let (v0, b0) = (viewport(&s, vh), outline(&s, bh));
    s.execute("rotate", &json!({"handles": [bh.hex()], "base": [0, 0], "angle": 30})).unwrap();
    s.undo().unwrap();
    assert_eq!(viewport(&s, vh), v0);
    assert_eq!(outline(&s, bh), b0);
}

#[test]
fn grip_stretching_the_boundary_refits_the_viewport() {
    let (mut s, vh, bh) = session();
    let before = viewport(&s, vh);
    let p = Vec2::new(60.0, 40.0);
    let shown = before.to_model(p).unwrap();
    // Drag the apex from 60,90 up to 60,120.
    let apex = s.grips_of(bh).unwrap().iter().position(|g| close(*g, Vec2::new(60.0, 90.0))).unwrap();
    s.execute("grip.stretch", &json!({"handle": bh.hex(), "index": apex, "to": [60, 120]})).unwrap();
    assert!(close(outline(&s, bh)[2], Vec2::new(60.0, 120.0)));
    assert!(fitted(&s, vh, bh));
    let v = viewport(&s, vh);
    assert!((v.height - 110.0).abs() < 1e-6);
    // Model space stays where it was on the sheet; only more of it shows.
    assert!(close(v.to_model(p).unwrap(), shown));
    assert!((v.view_height / v.height - before.view_height / before.height).abs() < 1e-9);
    // One undo step.
    s.undo().unwrap();
    assert_eq!(viewport(&s, vh), before);
}

#[test]
fn an_unclipped_copy_gets_its_own_id() {
    let (mut s, vh, _) = session();
    let r = s.execute("mview", &json!({"points": [[200, 10], [260, 60]]})).unwrap();
    let rh = r["viewport"].as_str().or_else(|| r["viewports"][0].as_str()).map(String::from).unwrap();
    s.execute("copy", &json!({"handles": [rh], "delta": [0, 100]})).unwrap();
    let mut ids: Vec<u32> = layout_viewports(&s).into_iter().map(|(_, v)| v.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 3, "ids {ids:?}");
    assert!(exists(&s, vh));
}
