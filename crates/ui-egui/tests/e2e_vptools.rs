//! The viewport and navigation tools in the app (Q162), driven through the control channel as an
//! agent or a user would: the steering wheel (typed NAVSWHEEL, dragging ZOOM and ORBIT, REWIND,
//! closing), the quick views' strip (QVLAYOUT, a click on a layout), the ViewCube's settings
//! (moved, hidden), and a camera glyph that looks from its camera.

mod e2e_common;

use cadaddy_ui_egui::navwheel::{Wedge, radius, wedge_center};
use e2e_common::E2e;
use serde_json::json;

fn view(e: &E2e) -> (f64, f64, f64) {
    let v = e.app.session.state().unwrap().view();
    (v.center.x, v.center.y, v.height)
}

fn wedge(e: &E2e, w: Wedge) -> (f64, f64) {
    let c = e.app.wheel.as_ref().expect("the wheel is open").center;
    let p = wedge_center(c, radius(&e.app), w);
    (f64::from(p.x), f64::from(p.y))
}

/// A drag or click's events arrive one a frame: let them all through.
fn settle(e: &mut E2e) {
    for _ in 0..5 {
        e.pump();
    }
}

fn element(e: &mut E2e, label: &str) -> Option<[f64; 4]> {
    let el = e.elements().into_iter().find(|x| x["label"].as_str() == Some(label))?;
    let r: Vec<f64> = el["rect"].as_array()?.iter().filter_map(|v| v.as_f64()).collect();
    Some([*r.first()?, *r.get(1)?, *r.get(2)?, *r.get(3)?])
}

#[test]
fn the_steering_wheel_zooms_orbits_rewinds_and_closes() {
    let mut e = E2e::new();
    e.exec("rectang", json!({"p1": [0, 0], "p2": [100, 60]}));
    e.exec("zoom", json!({"mode": "extents"}));
    let before = view(&e);
    e.typed("navswheel");
    assert!(e.app.wheel.is_some(), "typing NAVSWHEEL opens the wheel");
    assert!(e.shows("SteeringWheel"));
    // Drag ZOOM up: zoomed in.
    let (x, y) = wedge(&e, Wedge::Zoom);
    e.ok("ui.drag", json!({"x": x, "y": y, "to": [x, y - 80.0]}));
    settle(&mut e);
    let zoomed = view(&e);
    assert!(zoomed.2 < before.2 * 0.7, "{zoomed:?} {before:?}");
    // REWIND: back to where the drag started, in one step.
    let (x, y) = wedge(&e, Wedge::Rewind);
    e.ok("ui.click", json!({"x": x, "y": y}));
    settle(&mut e);
    let back = view(&e);
    assert!((back.2 - before.2).abs() < 1e-9 && (back.0 - before.0).abs() < 1e-9, "{back:?} {before:?}");
    assert!(e.app.session.selection().is_empty() && e.app.session.pending_window.is_none(), "the wheel kept the clicks");
    // Drag ORBIT: the plan view turns into a 3D view.
    let (x, y) = wedge(&e, Wedge::Orbit);
    e.ok("ui.drag", json!({"x": x, "y": y, "to": [x + 40.0, y - 60.0]}));
    settle(&mut e);
    let dir = e.app.session.state().unwrap().view_dir;
    assert!(dir.z < 0.999, "{dir:?}");
    // PAN moves the view.
    let (cx, cy, _) = view(&e);
    let (x, y) = wedge(&e, Wedge::Pan);
    e.ok("ui.drag", json!({"x": x, "y": y, "to": [x + 50.0, y]}));
    settle(&mut e);
    let (nx, ny, _) = view(&e);
    assert!((nx - cx).abs() + (ny - cy).abs() > 1e-6);
    // A right click closes it.
    let (x, y) = wedge(&e, Wedge::Zoom);
    e.ok("ui.click", json!({"x": x, "y": y, "button": "right"}));
    settle(&mut e);
    assert!(e.app.wheel.is_none());
    // So does Esc.
    e.typed("navswheel");
    assert!(e.app.wheel.is_some());
    e.ok("ui.key", json!({"key": "Escape"}));
    assert!(e.app.wheel.is_none());
}

#[test]
fn quick_view_layouts_switch_with_a_click() {
    let mut e = E2e::new();
    e.exec("circle", json!({"center": [0, 0], "radius": 10}));
    e.typed("qvlayout");
    assert!(e.app.quickview.is_some());
    assert!(e.shows("Model"), "a Model tile");
    let layout = e.app.session.doc().unwrap().layouts.first().unwrap().name.clone();
    assert!(e.click(&layout), "a tile for {layout}");
    assert_eq!(e.app.session.layout_space(), cadaddy_engine::doc::Space::Paper(layout));
    assert!(e.app.quickview.is_some(), "the strip stays open after switching");
    e.ok("ui.key", json!({"key": "Escape"}));
    assert!(e.app.quickview.is_none());
    // Drawings: one tile per open drawing.
    e.exec("new", json!({}));
    e.typed("qvdrawing");
    let titles: Vec<String> = e.app.session.docs.iter().map(|d| d.title.clone()).collect();
    assert!(titles.len() >= 2);
    assert!(e.click(&titles[0]));
    assert_eq!(e.app.session.active, 0);
    e.typed("qvdrawingclose");
    assert!(e.app.quickview.is_none());
}

#[test]
fn the_viewcube_follows_its_settings() {
    let mut e = E2e::new();
    let canvas = e.app.canvas.rect.unwrap();
    let top_right = element(&mut e, "ViewCube").expect("the cube");
    assert!(top_right[0] > f64::from(canvas.center().x));
    e.exec("navvcube", json!({"location": "bottomleft", "size": "small"}));
    let moved = element(&mut e, "ViewCube").expect("the cube");
    assert!(moved[0] < f64::from(canvas.center().x) && moved[1] > f64::from(canvas.center().y), "{moved:?}");
    assert!(moved[2] < top_right[2], "smaller");
    // Shown only in 3D views: gone from the plan view, back in an isometric view.
    e.exec("setvar", json!({"name": "NAVVCUBEDISPLAY", "value": 2}));
    assert!(element(&mut e, "ViewCube").is_none());
    e.exec("view.dir", json!({"preset": "swiso"}));
    assert!(element(&mut e, "ViewCube").is_some());
}

#[test]
fn a_camera_glyph_looks_from_its_camera() {
    let mut e = E2e::new();
    e.exec("rectang", json!({"p1": [0, 0], "p2": [100, 60]}));
    e.exec("camera", json!({"position": [20, 10, 5], "target": [80, 50, 0]}));
    e.exec("zoom", json!({"mode": "extents"}));
    let r = element(&mut e, "Camera1").expect("the camera's glyph");
    let (x, y) = (r[0] + r[2] / 2.0, r[1] + r[3] / 2.0);
    // Two clicks in quick succession (no settling in between: that would be too slow).
    e.ok("ui.click", json!({"x": x, "y": y}));
    e.ok("ui.click", json!({"x": x, "y": y}));
    settle(&mut e);
    let dir = e.app.session.state().unwrap().view_dir;
    let want = (cadaddy_engine::geom::Vec3::new(20.0, 10.0, 5.0) - cadaddy_engine::geom::Vec3::new(80.0, 50.0, 0.0)).normalized();
    assert!((dir - want).len() < 1e-9, "a double click looks from the camera: {dir:?}");
    // CAMERADISPLAY 0 hides the glyphs.
    e.exec("plan", json!({}));
    e.exec("setvar", json!({"name": "CAMERADISPLAY", "value": 0}));
    assert!(element(&mut e, "Camera1").is_none());
}

/// Pictures of the wheel, the quick views, a camera and VPMAX's border, for looking at
/// (`VPTOOLS_SHOTS=dir`).
#[test]
fn shots() {
    let Some(dir) = std::env::var_os("VPTOOLS_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).unwrap();
    use cadaddy_engine::Session;
    use cadaddy_ui_egui::{CadApp, Services};
    let mut app = CadApp::new(Session::new(), Services::default());
    let _ = app.run("rectang", json!({"p1": [0, 0], "p2": [100, 60]}));
    let _ = app.run("circle", json!({"center": [30, 30], "radius": 12}));
    let _ = app.run("camera", json!({"position": [70, 15, 5], "target": [30, 30, 0]}));
    let _ = app.run("zoom", json!({"mode": "extents"}));
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1280.0, 800.0)).wgpu().build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    let _ = h.state_mut().run("navswheel", json!({}));
    h.run_steps(4);
    h.render().unwrap().save(dir.join("wheel.png")).unwrap();
    h.state_mut().wheel = None;
    let _ = h.state_mut().run("qvlayout", json!({}));
    h.run_steps(4);
    h.render().unwrap().save(dir.join("qvlayout.png")).unwrap();
    h.state_mut().quickview = None;
    let _ = h.state_mut().run("navvcube", json!({"location": "bottomright", "opacity": 100}));
    let _ = h.state_mut().run("view.dir", json!({"preset": "seiso"}));
    h.run_steps(4);
    h.render().unwrap().save(dir.join("cube-moved-3d.png")).unwrap();
    let _ = h.state_mut().run("plan", json!({}));
    let _ = h.state_mut().run("layout.set", json!({"name": "Layout1"}));
    let _ = h.state_mut().run("mvsetup", json!({"option": "title"}));
    let _ = h.state_mut().run("mvsetup", json!({"option": "create", "rows": 1, "cols": 2, "gap": 0.2, "p1": [0.8, 1.9], "p2": [10.0, 7.8]}));
    h.run_steps(4);
    h.render().unwrap().save(dir.join("mvsetup-title.png")).unwrap();
    let vp = h.state().session.doc().unwrap().layouts.first().and_then(|_| {
        h.state().session.doc().unwrap().space(&h.state().session.layout_space()).and_then(|st| {
            st.iter().find_map(|e| match &e.kind {
                cadaddy_engine::doc::EntityKind::Viewport(v) if v.id != 1 => Some(e.handle),
                _ => None,
            })
        })
    });
    if let Some(vp) = vp {
        let _ = h.state_mut().run("vpmax", json!({"handle": vp.hex()}));
        h.run_steps(4);
        h.render().unwrap().save(dir.join("vpmax.png")).unwrap();
    }
}
