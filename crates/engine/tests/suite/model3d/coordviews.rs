//! Coordination views: the level and system filters hide the MEP objects they leave out, mode,
//! isolate, colour load and view restore are undoable and give the layers back as they were,
//! a saved view restores all of its state (mode off included), and the section box's face drag
//! maps a pointer drag to the new min/max.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::coordination::{self, BoxFace};
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::Entity;
use serde_json::{Value, json};

fn duct(name: &str) -> Value {
    json!({"name": name, "layer": name, "kind": "duct", "size_in": [12, 8], "bottom_ft": 10})
}

fn pipe(name: &str) -> Value {
    json!({"name": name, "layer": name, "kind": "pipe", "size_in": [2, 0], "bottom_ft": 11})
}

fn run(system: &str, level: &str, a: [f64; 2], b: [f64; 2]) -> Value {
    json!({"system": system, "level": level, "points": [a, b], "size_in": [12, 8], "bottoms_ft": [10]})
}

/// Two levels side by side on the sheet (inches), a wall layer the user made 40% transparent,
/// a supply duct on both levels and a water pipe on level 1, built to 3D.
fn building() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[0, 0], [480, 0], [480, 360], [0, 360]], "closed": true})).unwrap();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "Level 1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-1000, -1000, 1500, 1500], "base": [0, 0]},
                {"name": "Level 2", "elevation_ft": 14, "height_ft": 12, "ceiling_ft": 9, "region": [1500, -1000, 4000, 1500], "base": [2000, 0]}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}],
            "mep": {
                "systems": [duct("M-SUPPLY"), pipe("P-WATER")],
                "runs": [
                    run("M-SUPPLY", "Level 1", [24.0, 60.0], [264.0, 60.0]),
                    run("M-SUPPLY", "Level 2", [2024.0, 60.0], [2264.0, 60.0]),
                    run("P-WATER", "Level 1", [100.0, 0.0], [100.0, 200.0])
                ]
            }
        }}),
    )
    .unwrap();
    s.execute("model.build", &json!({})).unwrap();
    if let Some(l) = s.doc_mut().unwrap().layers.iter_mut().find(|l| l.name == "A-WALL") {
        l.transparency = 40;
    }
    s
}

/// A generated MEP object's (level, system).
fn mep_of(e: &Entity) -> Option<(String, String)> {
    if made_as(e).as_deref() != Some("mep") {
        return None;
    }
    let tags: Vec<&str> = e.common.xdata.iter().filter(|t| t.code == 1000).map(|t| t.value.as_str()).collect();
    let level = tags.get(1)?.to_string();
    let system = tags.iter().find_map(|t| t.strip_prefix("system:"))?.to_string();
    Some((level, system))
}

/// How many MEP objects of `system` (any when `None`) on `level` are visible, and how many there are.
fn shown(s: &Session, level: &str, system: Option<&str>) -> (usize, usize) {
    let d = s.doc().unwrap();
    let mine: Vec<&Entity> =
        d.model.iter().filter(|e| mep_of(e).is_some_and(|(l, sy)| l == level && system.is_none_or(|w| w == sy))).map(|e| &**e).collect();
    (mine.iter().filter(|e| d.is_visible(e)).count(), mine.len())
}

fn transparency(s: &Session, layer: &str) -> u8 {
    s.doc().unwrap().layers.iter().find(|l| l.name == layer).map_or(255, |l| l.transparency)
}

#[test]
fn isolate_level_hides_the_other_levels_and_reset_shows_them() {
    let mut s = building();
    let (l1, n1) = shown(&s, "Level 1", None);
    let (l2, n2) = shown(&s, "Level 2", None);
    assert!(n1 > 0 && n2 > 0 && l1 == n1 && l2 == n2, "both levels built and shown: {n1} {n2}");

    let r = s.execute("coordination.isolate", &json!({"level": "level 2"})).unwrap();
    assert_eq!(r["isolated"]["level"], "Level 2", "{r}");
    assert_eq!(shown(&s, "Level 1", None).0, 0, "level 1's MEP objects are hidden");
    assert_eq!(shown(&s, "Level 2", None), (n2, n2), "level 2's stay");
    assert_eq!(r["hidden"].as_u64(), Some(n1 as u64));
    // The architecture is not an MEP object: it stays.
    let d = s.doc().unwrap();
    assert!(d.model.iter().filter(|e| e.common.layer.as_ref() == "A-WALL").all(|e| d.is_visible(e)));

    // Isolating again replaces the last one.
    s.execute("coordination.isolate", &json!({"level": "Level 1"})).unwrap();
    assert_eq!(shown(&s, "Level 1", None), (n1, n1));
    assert_eq!(shown(&s, "Level 2", None).0, 0);

    s.execute("coordination.isolate", &json!({"reset": true})).unwrap();
    assert_eq!(shown(&s, "Level 1", None), (n1, n1));
    assert_eq!(shown(&s, "Level 2", None), (n2, n2));
}

#[test]
fn isolate_system_on_a_level() {
    let mut s = building();
    let (_, water) = shown(&s, "Level 1", Some("P-WATER"));
    let (_, supply1) = shown(&s, "Level 1", Some("M-SUPPLY"));
    assert!(water > 0 && supply1 > 0);
    s.execute("coordination.isolate", &json!({"system": "m-supply", "level": "Level 1"})).unwrap();
    assert_eq!(shown(&s, "Level 1", Some("M-SUPPLY")), (supply1, supply1));
    assert_eq!(shown(&s, "Level 1", Some("P-WATER")).0, 0, "the other system is hidden");
    assert_eq!(shown(&s, "Level 2", Some("M-SUPPLY")).0, 0, "the system on the other level is hidden");
}

#[test]
fn isolate_refuses_what_the_building_does_not_have() {
    let mut s = building();
    let e = s.execute("coordination.isolate", &json!({"level": "Level 9"})).unwrap_err().to_string();
    assert!(e.contains("Level 9") && e.contains("Level 1") && e.contains("Level 2"), "{e}");
    let e = s.execute("coordination.isolate", &json!({"system": "Fire"})).unwrap_err().to_string();
    assert!(e.contains("M-SUPPLY"), "{e}");
    assert!(s.execute("coordination.isolate", &json!({})).is_err(), "nothing to isolate");
    // Nothing was hidden by the refused calls.
    let (l1, n1) = shown(&s, "Level 1", None);
    assert_eq!(l1, n1);
}

#[test]
fn isolate_is_undoable() {
    let mut s = building();
    let (_, n1) = shown(&s, "Level 1", None);
    s.execute("coordination.isolate", &json!({"level": "Level 2"})).unwrap();
    assert_eq!(shown(&s, "Level 1", None).0, 0);
    s.undo().unwrap();
    assert_eq!(shown(&s, "Level 1", None), (n1, n1), "undo shows level 1 again");
}

#[test]
fn mode_off_gives_back_the_layers_own_transparency() {
    let mut s = building();
    assert_eq!(transparency(&s, "A-WALL"), 40);
    let mep = s.doc().unwrap().layers.iter().find(|l| l.name.starts_with("3D-MEP-")).map(|l| l.name.clone()).unwrap();
    s.execute("coordination.mode", &json!({"on": true, "ghost_opacity": 0.2})).unwrap();
    assert_eq!(transparency(&s, "A-WALL"), 72, "ghosted");
    assert_eq!(transparency(&s, &mep), 0, "trades solid");
    // Changing the ghost while on keeps the user's own value to go back to.
    s.execute("coordination.mode", &json!({"ghost_opacity": 0.5})).unwrap();
    assert_eq!(transparency(&s, "A-WALL"), 45);
    s.execute("coordination.mode", &json!({"on": false})).unwrap();
    assert_eq!(transparency(&s, "A-WALL"), 40, "the user's 40 is back, not 0");
    // Off again changes nothing.
    s.execute("coordination.mode", &json!({"on": false})).unwrap();
    assert_eq!(transparency(&s, "A-WALL"), 40);
}

#[test]
fn mode_colour_load_and_restore_are_undoable() {
    let mut s = building();
    // Mode.
    s.execute("coordination.mode", &json!({"on": true})).unwrap();
    assert_ne!(transparency(&s, "A-WALL"), 40);
    s.undo().unwrap();
    assert_eq!(transparency(&s, "A-WALL"), 40, "undo takes the ghosting away");
    assert!(!coordination::coord_render_state(s.doc().unwrap()).mode_on);

    // Colour scheme load: the system's layers (every level's) take the colour; undo puts it back.
    s.execute("coordination.colors", &json!({"save": {"name": "Review", "colors": {"M-SUPPLY": [300, -5, 128.4]}}})).unwrap();
    let before: Vec<_> = s.doc().unwrap().layers.iter().filter(|l| l.name.starts_with("3D-MEP-M-SUPPLY-")).map(|l| l.color).collect();
    assert_eq!(before.len(), 2, "one layer per level");
    s.execute("coordination.colors", &json!({"load": "Review"})).unwrap();
    let loaded: Vec<_> = s.doc().unwrap().layers.iter().filter(|l| l.name.starts_with("3D-MEP-M-SUPPLY-")).map(|l| l.color).collect();
    let want = cadaddy_engine::doc::color::Color::True(cadaddy_engine::doc::color::Rgb(255, 0, 128));
    assert!(loaded.iter().all(|c| *c == want), "clamped, not wrapped: {loaded:?}");
    s.undo().unwrap();
    let back: Vec<_> = s.doc().unwrap().layers.iter().filter(|l| l.name.starts_with("3D-MEP-M-SUPPLY-")).map(|l| l.color).collect();
    assert_eq!(back, before);
    let e = s.execute("coordination.colors", &json!({"load": "Nope"})).unwrap_err().to_string();
    assert!(e.contains("Review"), "names the schemes: {e}");

    // View restore: one undo step takes it all back.
    s.execute("coordination.mode", &json!({"on": true})).unwrap();
    s.execute("coordination.views", &json!({"save": {"name": "Ghosted"}})).unwrap();
    s.execute("coordination.mode", &json!({"on": false})).unwrap();
    assert_eq!(transparency(&s, "A-WALL"), 40);
    s.execute("coordination.views", &json!({"restore": "Ghosted"})).unwrap();
    assert_ne!(transparency(&s, "A-WALL"), 40);
    s.undo().unwrap();
    assert_eq!(transparency(&s, "A-WALL"), 40, "one undo takes the whole restore back");
    // Listing changes nothing, so it leaves no undo step.
    let steps = s.state().unwrap().undo.len();
    s.execute("coordination.views", &json!({"list": true})).unwrap();
    s.execute("coordination.colors", &json!({"list": true})).unwrap();
    assert_eq!(s.state().unwrap().undo.len(), steps);
}

#[test]
fn a_saved_view_restores_its_whole_state() {
    let mut s = building();
    // The view: from the south-west, the box round level 1 (on), the water pipe hidden, mode off.
    s.execute("vpoint", &json!({"dir": [-1, -1, 1]})).unwrap();
    s.execute("zoom", &json!({"mode": "center", "center": [200, 150], "height": 300})).unwrap();
    s.execute("coordination.sectionbox", &json!({"on": true, "min": [-10, -10, -10], "max": [500, 400, 170]})).unwrap();
    s.execute("coordination.trades", &json!({"set": [{"system": "P-WATER", "visible": false}]})).unwrap();
    let water_layers =
        |s: &Session| -> Vec<bool> { s.doc().unwrap().layers.iter().filter(|l| l.name.starts_with("3D-MEP-P-WATER-")).map(|l| l.on).collect() };
    assert_eq!(water_layers(&s), vec![false], "trades{{set}} reaches the per-level layer");
    let (cam, dir) = {
        let st = s.state().unwrap();
        (st.zoom_frame(), st.view_dir)
    };
    s.execute("coordination.views", &json!({"save": {"name": "Level 1 box"}})).unwrap();

    // Change all of it.
    s.execute("vpoint", &json!({"dir": [1, 1, 1]})).unwrap();
    s.execute("zoom", &json!({"mode": "center", "center": [2100, 50], "height": 900})).unwrap();
    s.execute("coordination.sectionbox", &json!({"on": false, "min": [0, 0, 0], "max": [1, 1, 1]})).unwrap();
    s.execute("coordination.trades", &json!({"set": [{"system": "P-WATER", "visible": true}]})).unwrap();
    s.execute("coordination.mode", &json!({"on": true})).unwrap();
    assert_ne!(transparency(&s, "A-WALL"), 40);

    let r = s.execute("coordination.views", &json!({"restore": "Level 1 box"})).unwrap();
    assert_eq!(r["warnings"], json!([]), "{r}");
    let st = s.state().unwrap();
    let now = st.zoom_frame();
    assert!((now.center.x - cam.center.x).abs() < 1e-6 && (now.center.y - cam.center.y).abs() < 1e-6, "{:?} vs {:?}", now.center, cam.center);
    assert!((now.height - cam.height).abs() < 1e-6);
    assert!((st.view_dir - dir).len() < 1e-9, "{:?} vs {dir:?}", st.view_dir);
    let crs = coordination::coord_render_state(s.doc().unwrap());
    assert!(crs.sbox_on);
    assert_eq!((crs.sbox_min, crs.sbox_max), ([-10.0, -10.0, -10.0], [500.0, 400.0, 170.0]));
    assert_eq!(water_layers(&s), vec![false], "the hidden trade is hidden again");
    assert!(!crs.mode_on, "mode off as saved");
    assert_eq!(transparency(&s, "A-WALL"), 40, "un-ghosted to the user's own transparency");

    let e = s.execute("coordination.views", &json!({"restore": "Elsewhere"})).unwrap_err().to_string();
    assert!(e.contains("Level 1 box"), "names the saved views: {e}");
}

#[test]
fn section_box_face_drag() {
    let (lo, hi) = ([0.0, 0.0, 0.0], [100.0, 50.0, 20.0]);
    assert_eq!(coordination::sbox_face_centre(lo, hi, BoxFace::MaxX), [100.0, 25.0, 10.0]);
    assert_eq!(coordination::sbox_face_centre(lo, hi, BoxFace::MinZ), [50.0, 25.0, 0.0]);
    // The +X face pulled out 10, the -Y face pushed in 5.
    assert_eq!(coordination::sbox_drag_face(lo, hi, BoxFace::MaxX, 10.0), ([0.0, 0.0, 0.0], [110.0, 50.0, 20.0]));
    assert_eq!(coordination::sbox_drag_face(lo, hi, BoxFace::MinY, 5.0), ([0.0, 5.0, 0.0], [100.0, 50.0, 20.0]));
    // A face stops at the one opposite; nonsense leaves the box.
    assert_eq!(coordination::sbox_drag_face(lo, hi, BoxFace::MaxZ, -50.0), ([0.0, 0.0, 0.0], [100.0, 50.0, 0.0]));
    assert_eq!(coordination::sbox_drag_face(lo, hi, BoxFace::MinX, 500.0).0, [100.0, 0.0, 0.0]);
    assert_eq!(coordination::sbox_drag_face(lo, hi, BoxFace::MinX, f64::NAN), (lo, hi));
    // On screen: 100 units along X show as 200 px to the right. A 40 px drag right moves the
    // face 20 units; straight down (across the axis) moves it none; a diagonal by its share.
    let d = |px: [f64; 2]| coordination::sbox_drag_delta([200.0, 0.0], 100.0, px);
    assert_eq!(d([40.0, 0.0]), Some(20.0));
    assert_eq!(d([0.0, 30.0]), Some(0.0));
    assert_eq!(d([-40.0, 25.0]), Some(-20.0));
    // Z goes up the screen (y down): dragging up raises the top.
    assert_eq!(coordination::sbox_drag_delta([0.0, -100.0], 20.0, [0.0, -50.0]), Some(10.0));
    // An axis pointing at the viewer can't be dragged.
    assert_eq!(coordination::sbox_drag_delta([0.5, 0.5], 100.0, [40.0, 0.0]), None);
    assert_eq!(coordination::sbox_drag_delta([200.0, 0.0], f64::NAN, [40.0, 0.0]), None);
    // A drag mapped and applied gives the box the command is sent.
    let delta = d([40.0, 0.0]).unwrap();
    let (nlo, nhi) = coordination::sbox_drag_face(lo, hi, BoxFace::MaxX, delta);
    let mut s = Session::new();
    s.execute("coordination.sectionbox", &json!({"on": true, "min": nlo, "max": nhi})).unwrap();
    let crs = coordination::coord_render_state(s.doc().unwrap());
    assert_eq!((crs.sbox_min, crs.sbox_max), ([0.0, 0.0, 0.0], [120.0, 50.0, 20.0]));
}
