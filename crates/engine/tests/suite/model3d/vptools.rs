//! Viewport and navigation tools (Q162): the steering wheel's actions (zoom about a point, pan,
//! orbit about the pivot, center, look, rewind), the ViewCube's settings, cameras and their views
//! (also kept in DXF), MVSETUP (title block, viewport grid, scale, align, a model border), the quick
//! views' lists and thumbnails, and TILEMODE typed at the command line.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Space};
use cadaddy_engine::geom::{Vec2, Vec3};
use serde_json::{Value, json};

fn session() -> Session {
    let mut s = Session::new();
    s.new_drawing(false);
    s.viewport_px = (1200.0, 800.0);
    s
}

fn view(s: &Session) -> (Vec2, f64, Vec3) {
    let st = s.state().unwrap();
    let v = st.view();
    (v.center, v.height, st.view_dir)
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6 * (1.0 + a.abs().max(b.abs()))
}

fn near2(a: Vec2, b: Vec2) -> bool {
    near(a.x, b.x) && near(a.y, b.y)
}

/// Where drawing point `q` is on the view plane (the canvas maps that linearly to the screen).
fn on_screen(s: &Session, q: Vec3) -> Vec2 {
    let st = s.state().unwrap();
    let v = st.view();
    let at = st.projection().map_or(q.xy(), |p| p.project(q));
    (at - v.center) * (1.0 / v.height)
}

#[test]
fn wheel_zoom_keeps_the_point_and_pan_moves_the_view() {
    let mut s = session();
    s.execute("rectang", &json!({"p1": [0, 0], "p2": [100, 60]})).unwrap();
    s.execute("zoom", &json!({"mode": "extents"})).unwrap();
    let (c0, h0, _) = view(&s);
    let at = Vec3::new(80.0, 50.0, 0.0);
    let before = on_screen(&s, at);
    let r = s.execute("navswheel.zoom", &json!({"factor": 2, "at": [80, 50]})).unwrap();
    assert_eq!(r["rewind"], 1);
    let (_, h1, _) = view(&s);
    assert!(near(h1, h0 / 2.0));
    // The point stays where it was on screen.
    assert!(near2(on_screen(&s, at), before), "{:?} {:?}", on_screen(&s, at), before);
    let (c1, ..) = view(&s);
    s.execute("navswheel.pan", &json!({"by": [10, -5]})).unwrap();
    let (c2, ..) = view(&s);
    assert!(near2(c2, c1 - Vec2::new(10.0, -5.0)), "the drawing moves right and down");
    // Rewind goes back through the views each action started from.
    s.execute("navswheel.rewind", &json!({})).unwrap();
    assert!(near(view(&s).1, h1));
    s.execute("navswheel.rewind", &json!({})).unwrap();
    assert!(near2(view(&s).0, c0) && near(view(&s).1, h0));
    assert!(s.execute("navswheel.rewind", &json!({})).is_err(), "nothing left to rewind");
    // A drag's later steps don't record: one rewind undoes the whole drag.
    s.execute("navswheel.zoom", &json!({"factor": 1.1})).unwrap();
    for _ in 0..5 {
        s.execute("navswheel.zoom", &json!({"factor": 1.1, "record": false})).unwrap();
    }
    s.execute("navswheel.rewind", &json!({})).unwrap();
    assert!(near(view(&s).1, h0));
    for bad in [json!({"factor": 0}), json!({"factor": -2}), json!({}), json!({"factor": "x"})] {
        assert!(s.execute("navswheel.zoom", &bad).is_err(), "{bad}");
    }
    assert!(s.execute("navswheel.pan", &json!({"by": [1]})).is_err());
}

#[test]
fn wheel_orbit_turns_about_the_pivot_and_center_sets_it() {
    let mut s = session();
    s.execute("box", &json!({"corner": [0, 0, 0], "length": 10, "width": 10, "height": 10})).unwrap();
    s.execute("view.dir", &json!({"preset": "swiso"})).unwrap();
    let pivot = Vec3::new(10.0, 10.0, 10.0);
    s.execute("navswheel.center", &json!({"point": [10, 10, 10]})).unwrap();
    assert!(near2(on_screen(&s, pivot), Vec2::ZERO), "Center puts the point in the middle");
    let (_, _, d0) = view(&s);
    let r = s.execute("navswheel.orbit", &json!({"by": [30, 10]})).unwrap();
    let (_, _, d1) = view(&s);
    assert!((d1 - d0).len() > 0.1, "the direction turned: {r}");
    // The pivot (from Center) stays in the middle of the view while orbiting.
    assert!(near2(on_screen(&s, pivot), Vec2::ZERO), "{:?}", on_screen(&s, pivot));
    // Elevation stops short of straight down; an explicit pivot stays put too.
    let other = Vec3::new(0.0, 0.0, 0.0);
    let before = on_screen(&s, other);
    s.execute("navswheel.orbit", &json!({"by": [-45, 200], "pivot": [0, 0, 0]})).unwrap();
    assert!(near2(on_screen(&s, other), before));
    assert!(view(&s).2.z < 1.0 && view(&s).2.z > 0.99);
    // Look turns the line of sight the other way round to orbiting.
    s.execute("view.dir", &json!({"preset": "front"})).unwrap();
    let r = s.execute("navswheel.look", &json!({"by": [20, 0]})).unwrap();
    let d = r["dir"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
    // Looking left from the front: the line of sight turns towards -X, so the direction back to
    // the viewer turns towards +X.
    assert!(d[0] > 0.2 && d[1] < -0.9, "{r}");
    // Rewind returns to the front view.
    s.execute("navswheel.rewind", &json!({})).unwrap();
    assert!(near(view(&s).2.y, -1.0), "{:?}", view(&s).2);
    // Orbit and Look are for the Model tab.
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    assert!(s.execute("navswheel.orbit", &json!({"by": [10, 0]})).is_err());
    assert!(s.execute("navswheel.zoom", &json!({"factor": 2})).is_ok(), "zoom works on the sheet");
}

#[test]
fn the_wheel_and_the_cube_settings() {
    let mut s = session();
    let r = s.execute("navswheel", &json!({})).unwrap();
    let wedges: Vec<&str> = r["wedges"].as_array().unwrap().iter().map(|w| w["label"].as_str().unwrap()).collect();
    assert_eq!(wedges, ["ZOOM", "REWIND", "PAN", "ORBIT", "CENTER", "WALK", "LOOK"]);
    assert_eq!(s.execute("navswheel.walk", &json!({})).unwrap()["mode"], "walk");
    let r = s.execute("navvcube", &json!({})).unwrap();
    assert_eq!(r, json!({"display": 3, "location": "topright", "size": "auto", "opacity": 50, "orient": "wcs"}));
    let r = s.execute("navvcube", &json!({"location": "bottom-left", "size": "large", "opacity": 80, "display": "3d"})).unwrap();
    assert_eq!(
        (r["location"].as_str(), r["size"].as_str(), r["opacity"].as_u64(), r["display"].as_u64()),
        (Some("bottomleft"), Some("large"), Some(80), Some(2))
    );
    // The same settings as system variables.
    assert_eq!(s.execute("getvar", &json!({"name": "NAVVCUBELOCATION"})).unwrap()["value"], 2);
    s.execute("setvar", &json!({"name": "NAVVCUBEDISPLAY", "value": 0})).unwrap();
    assert_eq!(s.settings.navvcube_display, 0);
    s.execute("setvar", &json!({"name": "NAVSWHEELSIZEBIG", "value": 2})).unwrap();
    assert_eq!(s.execute("navswheel", &json!({})).unwrap()["size"], 2);
    for (name, v) in [("NAVVCUBEOPACITY", 101), ("NAVVCUBESIZE", 5), ("NAVSWHEELOPACITYBIG", 10), ("NAVVCUBELOCATION", -1)] {
        assert!(s.execute("setvar", &json!({"name": name, "value": v})).is_err(), "{name} {v}");
    }
    for bad in [json!({"location": "middle"}), json!({"opacity": 120}), json!({"size": 9})] {
        assert!(s.execute("navvcube", &bad).is_err(), "{bad}");
    }
}

#[test]
fn cameras_place_views_and_round_trip_through_dxf() {
    let mut s = session();
    s.execute("box", &json!({"corner": [0, 0, 0], "length": 20, "width": 20, "height": 8})).unwrap();
    assert_eq!(s.execute("getvar", &json!({"name": "CAMERADISPLAY"})).unwrap()["value"], 0);
    s.execute("setvar", &json!({"name": "CAMERAHEIGHT", "value": 5.5})).unwrap();
    let r = s.execute("camera", &json!({"position": [-30, -40], "target": [10, 10, 4]})).unwrap();
    assert_eq!(r["name"], "Camera1");
    assert_eq!(s.execute("getvar", &json!({"name": "CAMERADISPLAY"})).unwrap()["value"], 1, "placing a camera shows the glyphs");
    s.execute("camera", &json!({"position": [50, 50, 30], "target": [0, 0, 0], "lens": 35, "name": "Overview"})).unwrap();
    let list = s.execute("camera.list", &json!({})).unwrap()["cameras"].as_array().unwrap().clone();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["position"], json!([-30.0, -40.0, 5.5]), "a 2D location stands CAMERAHEIGHT up");
    assert!(near(list[1]["fieldOfView"].as_f64().unwrap(), 2.0 * (18.0f64 / 35.0).atan().to_degrees()));
    // Its view looks from the camera at the target, the target in the middle.
    s.execute("camera.view", &json!({"name": "camera1"})).unwrap();
    let (_, h, dir) = view(&s);
    let want = (Vec3::new(-30.0, -40.0, 5.5) - Vec3::new(10.0, 10.0, 4.0)).normalized();
    assert!((dir - want).len() < 1e-9, "{dir:?}");
    assert!(near2(on_screen(&s, Vec3::new(10.0, 10.0, 4.0)), Vec2::ZERO));
    // A 50 mm lens frames 36/50 of the distance across.
    let dist = (Vec3::new(-30.0, -40.0, 5.5) - Vec3::new(10.0, 10.0, 4.0)).len();
    assert!(near(h * 1.5, dist * 36.0 / 50.0), "{h}");
    // Changing it: a longer lens frames less; renaming keeps it a camera.
    s.execute("camera.set", &json!({"name": "Camera1", "lens": 100, "rename": "Gate"})).unwrap();
    let names: Vec<String> = s.doc().unwrap().views.iter().map(|v| v.name.clone()).collect();
    assert_eq!(names, ["Gate", "Overview"]);
    // In DXF the camera views keep their cameras.
    let dir_ = std::env::temp_dir().join(format!("cadaddy-cam-{}", std::process::id()));
    std::fs::create_dir_all(&dir_).unwrap();
    let path = dir_.join("cams.dxf");
    let bytes = cadaddy_io::write(s.doc().unwrap(), "cams.dxf").unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let back = cadaddy_io::read(&bytes, "cams.dxf").unwrap();
    let cam = back.views.iter().find(|v| v.name == "Gate").and_then(|v| v.camera).unwrap();
    assert!(
        (cam.position - Vec3::new(-30.0, -40.0, 5.5)).len() < 1e-9 && (cam.target - Vec3::new(10.0, 10.0, 4.0)).len() < 1e-9 && near(cam.lens, 100.0)
    );
    assert!(back.views.iter().find(|v| v.name == "Overview").is_some_and(|v| v.camera.is_some()));
    s.execute("camera.delete", &json!({"name": "Overview"})).unwrap();
    assert_eq!(s.execute("camera.list", &json!({})).unwrap()["cameras"].as_array().unwrap().len(), 1);
    for bad in [
        json!({"position": [0, 0, 0], "target": [0, 0, 0]}),
        json!({"position": [0, 0]}),
        json!({"position": [1, 2], "target": [3, 4], "lens": 0}),
        json!({"position": [1, 2], "target": [3, 4], "name": "Gate"}),
        json!({"position": [1, "a"], "target": [3, 4]}),
    ] {
        assert!(s.execute("camera", &bad).is_err(), "{bad}");
    }
    assert!(s.execute("camera.view", &json!({"name": "nope"})).is_err());
    // Typed: the location, then the target.
    s.cmdline("camera").unwrap();
    s.cmdline("0,0").unwrap();
    s.cmdline("5,5").unwrap();
    assert_eq!(s.execute("camera.list", &json!({})).unwrap()["cameras"].as_array().unwrap().len(), 2);
}

fn viewports(s: &Session) -> Vec<(cadaddy_engine::doc::Handle, cadaddy_engine::doc::Viewport)> {
    let d = s.doc().unwrap();
    d.space(&s.layout_space())
        .unwrap()
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Viewport(v) if v.id != 1 => Some((e.handle, v.as_ref().clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn mvsetup_title_block_grid_scale_and_align() {
    let mut s = session();
    s.execute("rectang", &json!({"p1": [0, 0], "p2": [400, 300]})).unwrap();
    s.execute("circle", &json!({"center": [100, 100], "radius": 10})).unwrap();
    // On the Model tab: a border at the drawing scale, the limits set to it.
    let r = s.execute("mvsetup", &json!({"option": "model", "size": "ansi-b", "scale": 48})).unwrap();
    assert_eq!(r["limits"], json!([[0.0, 0.0], [816.0, 528.0]]));
    assert!(s.execute("mvsetup", &json!({"option": "create"})).is_err(), "viewports are laid out in a layout");
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    for (h, _) in viewports(&s) {
        s.execute("erase", &json!({"handles": [h.hex()]})).unwrap();
    }
    let r = s.execute("mvsetup", &json!({"option": "title", "size": "A"})).unwrap();
    assert_eq!(r["block"], "MVSETUP-ANSI-A");
    let blk = s.doc().unwrap().block("MVSETUP-ANSI-A").unwrap().clone();
    let texts: Vec<String> =
        blk.entities.iter().filter_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.value.clone()) } else { None }).collect();
    assert_eq!(texts, ["TITLE", "DRAWN", "DATE", "SCALE", "SHEET"]);
    let area = r["area"].clone();
    let (a, b) = (
        Vec2::new(area[0][0].as_f64().unwrap(), area[0][1].as_f64().unwrap()),
        Vec2::new(area[1][0].as_f64().unwrap(), area[1][1].as_f64().unwrap()),
    );
    // Two rows of three viewports above the title block, 0.25 apart, at 1:100.
    let r = s
        .execute("mvsetup", &json!({"option": "create", "rows": 2, "cols": 3, "gap": 0.25, "p1": [a.x, a.y], "p2": [b.x, b.y], "scale": "1:100"}))
        .unwrap();
    assert_eq!(r["viewports"].as_array().unwrap().len(), 6);
    let vps = viewports(&s);
    assert_eq!(vps.len(), 6);
    for (_, v) in &vps {
        assert!(near(v.height / v.view_height, 0.01));
        assert!(v.center.x - v.width / 2.0 >= a.x - 1e-9 && v.center.y + v.height / 2.0 <= b.y + 1e-9);
    }
    let w = (b.x - a.x - 0.5) / 3.0;
    assert!(near(vps[0].1.width, w));
    // Scale: all of them to 1:50.
    s.execute("mvsetup", &json!({"option": "scale", "scale": "1:50"})).unwrap();
    assert!(viewports(&s).iter().all(|(_, v)| near(v.height / v.view_height, 0.02)));
    // Align: the circle's centre in the second viewport level with it in the first.
    let (h0, h1) = (vps[0].0, vps[1].0);
    s.execute("mvsetup", &json!({"option": "align", "base_viewport": h0.hex(), "base": [100, 100], "handle": h1.hex(), "point": [100, 100]}))
        .unwrap();
    let vps = viewports(&s);
    let (p0, p1) = (vps[0].1.to_paper(Vec2::new(100.0, 100.0)).unwrap(), vps[1].1.to_paper(Vec2::new(100.0, 100.0)).unwrap());
    assert!(near(p0.y, p1.y), "{p0:?} {p1:?}");
    s.execute(
        "mvsetup",
        &json!({"option": "align", "mode": "vertical", "base_viewport": h0.hex(), "base": [100, 100], "handle": vps[3].0.hex(), "point": [100, 100]}),
    )
    .unwrap();
    let vps = viewports(&s);
    assert!(near(vps[0].1.to_paper(Vec2::new(100.0, 100.0)).unwrap().x, vps[3].1.to_paper(Vec2::new(100.0, 100.0)).unwrap().x));
    for bad in [
        json!({"option": "title", "size": "B9"}),
        json!({"option": "create", "rows": 0}),
        json!({"option": "create", "rows": 9}),
        json!({"option": "create", "cols": 4, "gap": 100}),
        json!({"option": "scale"}),
        json!({"option": "align", "base_viewport": h0.hex(), "base": [0, 0], "handle": h0.hex(), "point": [0, 0]}),
        json!({"option": "nope"}),
    ] {
        assert!(s.execute("mvsetup", &bad).is_err(), "{bad}");
    }
    assert!(s.execute("mvsetup", &json!({})).unwrap()["sheets"].as_array().unwrap().len() >= 10);
    // Without a size, the title block fits the layout's paper.
    let page = s.execute("layout.list", &json!({})).unwrap();
    let r = s.execute("mvsetup", &json!({"option": "title"})).unwrap();
    assert!(r["block"].as_str().unwrap().starts_with("MVSETUP-"), "{r} {page}");
    let sheet = cadaddy_engine::render::sheet(s.doc().unwrap(), "Layout1").unwrap();
    let area = &r["area"];
    assert!(
        area[1][0].as_f64().unwrap() <= sheet.origin.x + sheet.size.x + 1e-9 && area[1][1].as_f64().unwrap() <= sheet.origin.y + sheet.size.y + 1e-9,
        "{r}"
    );
}

#[test]
fn mvsetup_typed() {
    let mut s = session();
    s.execute("rectang", &json!({"p1": [0, 0], "p2": [40, 30]})).unwrap();
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    let before = viewports(&s).len();
    for l in ["mvsetup", "c", "2", "2", "s", "1:4", "t", "iso-a3", ""] {
        s.cmdline(l).unwrap();
    }
    assert_eq!(viewports(&s).len(), before + 4);
    assert!(viewports(&s).iter().all(|(_, v)| near(v.height / v.view_height, 0.25)));
    assert!(s.doc().unwrap().block("MVSETUP-ISO-A3").is_some());
}

#[test]
fn quick_views_list_switch_and_draw_thumbnails() {
    let mut s = session();
    s.execute("circle", &json!({"center": [0, 0], "radius": 5})).unwrap();
    let r = s.execute("qvlayout", &json!({"thumbnails": {"width": 64, "height": 48}})).unwrap();
    let names: Vec<&str> = r["layouts"].as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap()).collect();
    assert_eq!(names[0], "Model");
    assert!(names.len() >= 2);
    let png = cadaddy_engine::cmd::file::base64_decode(r["layouts"][0]["thumbnail"].as_str().unwrap()).unwrap();
    assert_eq!(&png[..4], b"\x89PNG");
    let r = s.execute("qvlayout", &json!({"open": names[1]})).unwrap();
    assert_eq!(s.layout_space(), Space::Paper(names[1].to_string()));
    assert!(r["layouts"][0].get("thumbnail").is_none());
    assert_eq!(r["layouts"][1]["current"], true);
    // The model's thumbnail shows the circle: not all background.
    let px = cadaddy_engine::cmd::quickview::thumbnail_rgba(s.doc().unwrap(), &Space::Model, 40, 30).unwrap();
    assert_eq!(px.len(), 40 * 30 * 4);
    assert!(px.chunks(4).any(|p| p != &px[..4]));
    s.new_drawing(false);
    let r = s.execute("qvdrawing", &json!({"thumbnails": true})).unwrap();
    let ds = r["drawings"].as_array().unwrap();
    assert_eq!(ds.len(), 3);
    assert_eq!(ds[2]["current"], true);
    assert!(ds.iter().all(|d| d["thumbnail"].is_string()));
    s.execute("qvdrawing", &json!({"open": 1})).unwrap();
    assert_eq!(s.active, 1);
    assert!(s.execute("qvdrawing", &json!({"open": 9})).is_err());
    assert!(s.execute("qvlayout", &json!({"open": "No such layout"})).is_err());
    let _: Value = json!(null);
}

#[test]
fn tilemode_typed_switches_tabs() {
    let mut s = session();
    s.cmdline("tilemode").unwrap();
    s.cmdline("0").unwrap();
    assert!(matches!(s.layout_space(), Space::Paper(_)));
    s.cmdline("tilemode").unwrap();
    s.cmdline("1").unwrap();
    assert_eq!(s.layout_space(), Space::Model);
}

#[test]
fn vpmax_then_vpmin_and_the_wheel_inside() {
    let mut s = session();
    s.execute("circle", &json!({"center": [10, 10], "radius": 5})).unwrap();
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    if viewports(&s).is_empty() {
        s.execute("mview", &json!({})).unwrap();
    }
    let h = viewports(&s)[0].0;
    s.execute("vpmax", &json!({"handle": h.hex()})).unwrap();
    assert!(s.state().unwrap().vpmax.is_some());
    // The wheel zooms the maximized viewport's model view.
    let before = viewports(&s)[0].1.view_height;
    s.execute("navswheel.zoom", &json!({"factor": 2})).unwrap();
    assert!(near(viewports(&s)[0].1.view_height, before / 2.0));
    s.execute("navswheel.rewind", &json!({})).unwrap();
    assert!(near(viewports(&s)[0].1.view_height, before));
    s.execute("vpmin", &json!({})).unwrap();
    assert!(s.state().unwrap().vpmax.is_none());
}
