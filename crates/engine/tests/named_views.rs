//! Named views (#463): VIEW / -VIEW Save, Window, Restore, Delete, ? and the layer snapshot
//! setting; Zoom Realtime and Dynamic; ZOOMFACTOR. (The DXF round trip of views is tested in
//! `cadaddy-io`'s `view_tables_roundtrip`.)

use cadaddy_engine::doc::Space;
use cadaddy_engine::geom::Vec2;
use cadaddy_engine::{Session, View};
use serde_json::json;

fn session() -> Session {
    let mut s = Session::new();
    s.viewport_px = (1200.0, 800.0);
    s.execute("line", &json!({ "points": [[0, 0], [100, 50]] })).unwrap();
    s
}

/// Type each line at the command line.
fn typed(s: &mut Session, lines: &[&str]) {
    for l in lines {
        s.cmdline(l).unwrap();
    }
}

fn view(s: &Session) -> View {
    s.state().unwrap().view()
}

fn set_view(s: &mut Session, cx: f64, cy: f64, h: f64) {
    s.execute("view.set", &json!({ "center": [cx, cy], "height": h })).unwrap();
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9 * (1.0 + a.abs().max(b.abs()))
}

#[test]
fn typed_save_restore_list_delete() {
    let mut s = session();
    set_view(&mut s, 10.0, 20.0, 30.0);
    typed(&mut s, &["-view", "s", "Front"]);
    assert!(s.running.is_none(), "{}", s.prompt_text());
    let v = s.doc().unwrap().views.first().cloned().unwrap();
    assert_eq!((v.name.as_str(), v.center, v.height, v.layout.as_deref()), ("Front", Vec2::new(10.0, 20.0), 30.0, None));
    assert!(close(v.width, 45.0), "width {}", v.width);
    assert!(v.layer_state.is_some(), "a layer snapshot by default");
    // Saving is an undo step; a view change is not.
    set_view(&mut s, 0.0, 0.0, 500.0);
    typed(&mut s, &["view", "r", "front"]);
    assert_eq!(view(&s), View { center: Vec2::new(10.0, 20.0), height: 30.0 });
    // Zoom Previous goes back to the view before the restore.
    s.execute("zoom.previous", &json!({})).unwrap();
    assert_eq!(view(&s).height, 500.0);
    typed(&mut s, &["-view", "?", ""]);
    assert!(s.log.iter().any(|l| l.contains("Front") && l.contains("Model")), "{:?}", s.log);
    typed(&mut s, &["-view", "d", "fr*"]);
    assert!(s.doc().unwrap().views.is_empty());
    assert!(s.doc().unwrap().layer_states.is_empty(), "the snapshot goes with the view");
    s.undo().unwrap();
    assert_eq!(s.doc().unwrap().views.len(), 1);
}

#[test]
fn window_option_and_json_forms() {
    let mut s = session();
    typed(&mut s, &["-view", "w", "Plan", "0,0", "40,10"]);
    let v = s.doc().unwrap().views.first().cloned().unwrap();
    assert_eq!((v.center, v.height, v.width), (Vec2::new(20.0, 5.0), 10.0, 40.0));
    // Restoring fits the whole window: the width decides here (40 / 1.5 > 10).
    s.execute("view", &json!({ "option": "restore", "name": "PLAN" })).unwrap();
    let r = view(&s);
    assert_eq!(r.center, Vec2::new(20.0, 5.0));
    assert!(close(r.height, 40.0 / 1.5), "{}", r.height);
    // JSON save replaces a view of the same name; list reports it.
    s.execute("view", &json!({ "option": "save", "name": "plan", "layerSnapshot": false })).unwrap();
    let list = s.execute("view", &json!({ "option": "list" })).unwrap();
    let rows = list["views"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0]["name"].as_str(), rows[0]["layerSnapshot"].as_bool()), (Some("plan"), Some(false)));
    // Bad names and missing views are errors, not panics.
    for bad in [json!({ "option": "save", "name": "a*b" }), json!({ "option": "save" }), json!({ "option": "restore", "name": "nope" })] {
        assert!(s.execute("view", &bad).is_err(), "{bad}");
    }
    assert!(s.execute("view", &json!({ "option": "window", "name": "Flat", "p1": [0, 0], "p2": [10, 0] })).is_err());
    assert!(s.execute("view", &json!({ "option": "orthographic" })).is_err());
}

#[test]
fn layer_snapshot_restores_layers_and_the_setting_turns_it_off() {
    let mut s = session();
    s.doc_mut().unwrap().layers.push(cadaddy_engine::doc::Layer { name: "Walls".into(), ..Default::default() });
    let walls_on = |s: &Session| s.doc().unwrap().layer("Walls").map(|l| l.on);
    typed(&mut s, &["-view", "s", "All"]);
    s.doc_mut().unwrap().layer_mut("Walls").unwrap().on = false;
    typed(&mut s, &["-view", "r", "All"]);
    assert_eq!(walls_on(&s), Some(true));
    // sEttings > Layer snapshot > No: views are saved without one.
    typed(&mut s, &["-view", "e", "l", "n"]);
    assert!(!s.settings.view_layer_snapshot);
    typed(&mut s, &["-view", "s", "Bare"]);
    let bare = s.doc().unwrap().views.iter().find(|v| v.name == "Bare").cloned().unwrap();
    assert_eq!(bare.layer_state, None);
}

#[test]
fn paper_space_view_switches_to_its_layout() {
    let mut s = session();
    let layout = s.doc().unwrap().layouts.first().map(|l| l.name.clone()).unwrap();
    s.execute("layout.set", &json!({ "name": layout })).unwrap();
    set_view(&mut s, 5.0, 4.0, 6.0);
    s.execute("view", &json!({ "option": "save", "name": "Sheet" })).unwrap();
    assert_eq!(s.doc().unwrap().views.first().and_then(|v| v.layout.clone()), Some(layout.clone()));
    s.execute("layout.set", &json!({ "name": "Model" })).unwrap();
    s.execute("view", &json!({ "option": "restore", "name": "Sheet" })).unwrap();
    assert_eq!(s.layout_space(), Space::Paper(layout));
    assert_eq!(view(&s), View { center: Vec2::new(5.0, 4.0), height: 6.0 });
}

#[test]
fn zoom_realtime_drags_and_enter_ends() {
    let mut s = session();
    set_view(&mut s, 0.0, 0.0, 100.0);
    s.state_mut().unwrap().view_history.clear();
    // Enter at ZOOM's first prompt starts Realtime.
    typed(&mut s, &["zoom", ""]);
    assert!(s.realtime_zoom_active(), "{}", s.prompt_text());
    // Up half the height (400 of 800 px): 2x in; then down a quarter: back by √2.
    s.realtime_zoom(-400.0).unwrap();
    assert!(close(view(&s).height, 50.0), "{}", view(&s).height);
    s.realtime_zoom(200.0).unwrap();
    assert!(close(view(&s).height, 50.0 * 2f64.sqrt()), "{}", view(&s).height);
    s.realtime_zoom(f64::NAN).unwrap();
    typed(&mut s, &[""]);
    assert!(s.running.is_none() && !s.realtime_zoom_active());
    // Outside Realtime a drag does nothing; Zoom Previous returns to the view before the drags.
    s.realtime_zoom(-400.0).unwrap();
    assert!(close(view(&s).height, 50.0 * 2f64.sqrt()));
    s.execute("zoom.previous", &json!({})).unwrap();
    assert_eq!(view(&s).height, 100.0);
    // The JSON form zooms by a factor about the centre.
    s.execute("zoom.realtime", &json!({ "factor": 4 })).unwrap();
    assert_eq!(view(&s), View { center: Vec2::ZERO, height: 25.0 });
}

#[test]
fn zoom_dynamic_places_and_sizes_the_view_box() {
    let mut s = session();
    set_view(&mut s, 10.0, 10.0, 20.0);
    typed(&mut s, &["zoom", "d"]);
    // The overview shows the extents (0,0 to 100,50) and the old view.
    let over = view(&s);
    assert!(over.height > 50.0, "{over:?}");
    assert!(!s.preview(Vec2::new(50.0, 25.0)).is_empty());
    // Place the box at 60,30, size it to 60 wide (left edge 60 - 15 = 45 → 105), accept.
    typed(&mut s, &["60,30", "105,0", ""]);
    assert!(s.running.is_none(), "{}", s.prompt_text());
    let v = view(&s);
    assert!(close(v.center.x, 75.0) && close(v.center.y, 30.0), "{v:?}");
    assert!(close(v.height, 40.0), "{v:?}");
    // Zoom Previous skips the overview.
    s.execute("zoom.previous", &json!({})).unwrap();
    assert_eq!(view(&s), View { center: Vec2::new(10.0, 10.0), height: 20.0 });
    // JSON: like Center.
    s.execute("zoom.dynamic", &json!({ "center": [1, 2], "height": 3 })).unwrap();
    assert_eq!(view(&s), View { center: Vec2::new(1.0, 2.0), height: 3.0 });
}

#[test]
fn zoomfactor_scales_the_wheel() {
    let mut s = session();
    assert_eq!(s.execute("getvar", &json!({ "name": "ZOOMFACTOR" })).unwrap()["value"], json!(60));
    let at60 = s.wheel_zoom_factor(50.0);
    for bad in [json!(2), json!(101), json!("x")] {
        assert!(s.execute("setvar", &json!({ "name": "ZOOMFACTOR", "value": bad })).is_err());
    }
    s.execute("setvar", &json!({ "name": "ZOOMFACTOR", "value": 100 })).unwrap();
    let at100 = s.wheel_zoom_factor(50.0);
    s.execute("setvar", &json!({ "name": "ZOOMFACTOR", "value": 3 })).unwrap();
    let at3 = s.wheel_zoom_factor(50.0);
    assert!(at100 > at60 && at60 > at3 && at3 > 1.0, "{at3} {at60} {at100}");
    assert!(close(s.wheel_zoom_factor(-50.0), 1.0 / at3));
    assert_eq!(s.wheel_zoom_factor(f64::INFINITY), 1.0);
}

// ---- Views II (gaps U11): orthographic presets, UCS and background kept with a view ----

use cadaddy_engine::doc::color::{Color, Rgb};
use cadaddy_engine::doc::{HVal, Ucs, ViewUcs};
use cadaddy_engine::geom::Vec3;
use cadaddy_engine::geom::view3;

fn saved(s: &Session, name: &str) -> cadaddy_engine::doc::NamedView {
    s.doc().unwrap().views.iter().find(|v| v.name == name).cloned().unwrap()
}

fn current_ucs(s: &Session) -> ViewUcs {
    ViewUcs::current(&s.doc().unwrap().header)
}

#[test]
fn orthographic_views_are_saved_and_restored() {
    let mut s = session();
    let front = view3::preset("front").unwrap();
    // A preset view shown, then saved: it keeps its direction.
    s.execute("view.front", &json!({})).unwrap();
    s.execute("view", &json!({ "option": "save", "name": "F" })).unwrap();
    let f = saved(&s, "F");
    assert!(f.dir.is_some_and(|d| (d - front.normalized()).len() < 1e-9), "{:?}", f.dir);
    s.execute("plan", &json!({})).unwrap();
    s.execute("view", &json!({ "option": "save", "name": "P" })).unwrap();
    assert_eq!(saved(&s, "P").dir, None, "the plan view keeps no direction");
    // A preset saved without leaving the plan view: the preset's view of the extents.
    let before = view(&s);
    s.execute("view", &json!({ "option": "save", "name": "Iso", "orthographic": "swiso" })).unwrap();
    assert_eq!(view(&s), before, "saving doesn't change what is shown");
    assert!(s.state().unwrap().projection().is_none());
    let iso = saved(&s, "Iso");
    assert!(iso.dir.is_some_and(|d| view3::preset_label(d) == Some("SW Isometric")), "{:?}", iso.dir);
    assert!(iso.height > 0.0 && iso.width > 0.0);
    // Restoring turns the view; restoring a plan view turns it back.
    s.execute("view", &json!({ "option": "restore", "name": "F" })).unwrap();
    assert!((s.state().unwrap().view_dir - front.normalized()).len() < 1e-9);
    assert!(close(view(&s).center.x, f.center.x) && close(view(&s).center.y, f.center.y));
    s.execute("view", &json!({ "option": "restore", "name": "P" })).unwrap();
    assert_eq!(s.state().unwrap().view_dir, Vec3::Z);
    // The list names the direction.
    let r = s.execute("view", &json!({ "option": "list" })).unwrap();
    let dirs: Vec<_> = r["views"].as_array().unwrap().iter().map(|v| v["direction"].clone()).collect();
    assert_eq!(dirs, [json!("Front"), json!(null), json!("SW Isometric")]);
    assert!(s.execute("view", &json!({ "option": "save", "name": "X", "orthographic": "sideways" })).is_err());
}

#[test]
fn orthographic_presets_round_trip_through_dxf() {
    let mut s = session();
    s.execute(
        "view",
        &json!({ "option": "save", "name": "Iso", "orthographic": "neiso", "ucs": { "origin": [5, 6], "angle": 90 }, "background": "#336699" }),
    )
    .unwrap();
    let text = cadaddy_io::write_dxf(s.doc().unwrap());
    let back = cadaddy_io::read_dxf(text.as_bytes()).unwrap();
    let (a, b) = (saved(&s, "Iso"), back.views.iter().find(|v| v.name == "Iso").cloned().unwrap());
    assert!(a.dir.zip(b.dir).is_some_and(|(x, y)| (x - y).len() < 1e-9), "{:?} {:?}", a.dir, b.dir);
    let (ua, ub) = (a.ucs.unwrap(), b.ucs.unwrap());
    assert!((ua.origin - ub.origin).len() < 1e-9 && (ua.x_axis - ub.x_axis).len() < 1e-9 && (ua.y_axis - ub.y_axis).len() < 1e-9);
    assert_eq!(b.background, Some(Color::True(Rgb(0x33, 0x66, 0x99))));
}

#[test]
fn a_ucs_is_saved_and_restored_with_a_view() {
    let mut s = session();
    // A UCS given with the view: restoring makes it current.
    s.execute("view", &json!({ "option": "save", "name": "A", "ucs": { "origin": [5, 5], "angle": 30 } })).unwrap();
    assert!(current_ucs(&s).is_world(), "saving doesn't change the UCS");
    s.execute("view", &json!({ "option": "restore", "name": "A" })).unwrap();
    let u = current_ucs(&s);
    assert!((u.origin - Vec3::new(5.0, 5.0, 0.0)).len() < 1e-9 && (u.angle().to_degrees() - 30.0).abs() < 1e-9, "{u:?}");
    // UCSVIEW (1 by default): a view saved now keeps the current UCS…
    s.execute("view", &json!({ "option": "save", "name": "B" })).unwrap();
    assert!(saved(&s, "B").ucs.is_some_and(|b| (b.angle().to_degrees() - 30.0).abs() < 1e-9));
    // …and none with UCSVIEW 0, or with `ucs: none`.
    std::sync::Arc::make_mut(&mut s.state_mut().unwrap().doc).header.set("UCSVIEW", HVal::Int(0));
    s.execute("view", &json!({ "option": "save", "name": "C" })).unwrap();
    s.execute("view", &json!({ "option": "save", "name": "D", "ucs": "none" })).unwrap();
    assert!(saved(&s, "C").ucs.is_none() && saved(&s, "D").ucs.is_none());
    // A view without a UCS leaves the current one; a World view restores the world.
    s.execute("view", &json!({ "option": "restore", "name": "C" })).unwrap();
    assert!(!current_ucs(&s).is_world());
    s.execute("view", &json!({ "option": "save", "name": "W", "ucs": "world" })).unwrap();
    s.execute("view", &json!({ "option": "restore", "name": "W" })).unwrap();
    assert!(current_ucs(&s).is_world());
    // A named UCS keeps its name; restoring is one undo step.
    std::sync::Arc::make_mut(&mut s.state_mut().unwrap().doc).ucss.push(Ucs {
        name: "Site".into(),
        origin: Vec3::new(100.0, 0.0, 0.0),
        x_axis: Vec3::new(0.0, 1.0, 0.0),
        y_axis: Vec3::new(-1.0, 0.0, 0.0),
    });
    s.execute("view", &json!({ "option": "settings", "name": "C", "ucs": "site" })).unwrap();
    assert_eq!(saved(&s, "C").ucs.unwrap().name, "Site");
    s.execute("view", &json!({ "option": "restore", "name": "C" })).unwrap();
    assert_eq!(current_ucs(&s).name, "Site");
    s.undo().unwrap();
    assert!(current_ucs(&s).is_world());
    // Typed: -VIEW sEttings Ucs <view> World.
    typed(&mut s, &["-view", "e", "u", "C", "w"]);
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert!(saved(&s, "C").ucs.unwrap().is_world());
    for bad in [json!({ "option": "save", "name": "Z", "ucs": "nope" }), json!({ "option": "save", "name": "Z", "ucs": { "origin": [1] } })] {
        assert!(s.execute("view", &bad).is_err(), "{bad}");
    }
}

#[test]
fn a_view_shows_its_background() {
    let mut s = session();
    s.execute("view", &json!({ "option": "save", "name": "Blue", "background": "0,0,128" })).unwrap();
    s.execute("view", &json!({ "option": "save", "name": "Plain" })).unwrap();
    assert_eq!(s.state().unwrap().view_background, None, "saving doesn't show it");
    s.execute("view", &json!({ "option": "restore", "name": "Blue" })).unwrap();
    assert_eq!(s.state().unwrap().view_background, Some(Color::True(Rgb(0, 0, 128))));
    // It stays through zooming, and a view without one takes it away.
    set_view(&mut s, 1.0, 1.0, 10.0);
    assert!(s.state().unwrap().view_background.is_some());
    s.execute("view", &json!({ "option": "restore", "name": "Plain" })).unwrap();
    assert_eq!(s.state().unwrap().view_background, None);
    // Typed: -VIEW sEttings Background <view> <colour>; None clears it.
    typed(&mut s, &["-view", "e", "b", "Plain", "3"]);
    assert_eq!(saved(&s, "Plain").background, Some(Color::Index(3)));
    typed(&mut s, &["-view", "e", "b", "Plain", "none"]);
    assert_eq!(saved(&s, "Plain").background, None);
    let r = s.execute("view", &json!({ "option": "list" })).unwrap();
    assert_eq!(r["views"][0]["background"], "0,0,128");
    for bad in ["bylayer", "plaid", "300"] {
        assert!(s.execute("view", &json!({ "option": "settings", "name": "Plain", "background": bad })).is_err(), "{bad}");
    }
    assert!(s.execute("view", &json!({ "option": "settings", "name": "Nope", "background": "red" })).is_err());
}
