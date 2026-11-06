//! Lasso selection (a freehand window, crossing or fence path) and selection cycling
//! (SELECTIONCYCLING: stepping through the objects under one pick).

use cadaddy_doc::Handle;
use cadaddy_engine::select::{self, LassoMode};
use cadaddy_engine::{Input, Session};
use cadaddy_geom::Vec2;
use serde_json::{Value, json};

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Handle {
    s.execute("line", &json!({ "points": [a, b] })).unwrap();
    s.doc().unwrap().model.last().unwrap().handle
}

fn pts(v: &[[f64; 2]]) -> Vec<Vec2> {
    v.iter().map(|p| Vec2::new(p[0], p[1])).collect()
}

/// An L-shaped (concave) lasso: its bounding square is (0,0)-(10,10), its notch the top right.
const L: [[f64; 2]; 6] = [[0.0, 0.0], [10.0, 0.0], [10.0, 4.0], [4.0, 4.0], [4.0, 10.0], [0.0, 10.0]];

/// `inside` lies in the L, `crossing` crosses its right edge, `notch` lies in the notch (inside
/// the bounding square, outside the lasso), `away` is far off.
struct Scene {
    s: Session,
    inside: Handle,
    crossing: Handle,
    notch: Handle,
    away: Handle,
}

fn scene() -> Scene {
    let mut s = Session::new();
    let inside = line(&mut s, [1.0, 1.0], [3.0, 1.0]);
    let crossing = line(&mut s, [8.0, 2.0], [12.0, 2.0]);
    let notch = line(&mut s, [7.0, 7.0], [9.0, 9.0]);
    let away = line(&mut s, [20.0, 20.0], [25.0, 20.0]);
    Scene { s, inside, crossing, notch, away }
}

fn hexes(hs: &[Handle]) -> Value {
    json!(hs.iter().map(|h| h.hex()).collect::<Vec<_>>())
}

#[test]
fn a_window_lasso_selects_what_is_entirely_inside_its_outline_not_its_bounding_box() {
    let sc = scene();
    let d = sc.s.doc().unwrap();
    let got = select::select_lasso(d, &cadaddy_doc::Space::Model, &pts(&L), LassoMode::Window);
    assert_eq!(got, vec![sc.inside]);
    // A rectangle over the same square would also take the line in the notch.
    let rect = select::select_window(d, &cadaddy_doc::Space::Model, cadaddy_geom::Bounds2::new(Vec2::ZERO, Vec2::new(10.0, 10.0)), false);
    assert!(rect.contains(&sc.notch));
}

#[test]
fn a_crossing_lasso_also_selects_what_crosses_its_outline() {
    let sc = scene();
    let got = select::select_lasso(sc.s.doc().unwrap(), &cadaddy_doc::Space::Model, &pts(&L), LassoMode::Crossing);
    assert_eq!(got, vec![sc.inside, sc.crossing]);
    assert!(!got.contains(&sc.notch) && !got.contains(&sc.away));
}

#[test]
fn a_fence_lasso_selects_what_its_path_crosses() {
    let sc = scene();
    let got = select::select_lasso(sc.s.doc().unwrap(), &cadaddy_doc::Space::Model, &pts(&[[9.0, 0.0], [9.0, 5.0]]), LassoMode::Fence);
    assert_eq!(got, vec![sc.crossing]);
}

#[test]
fn space_cycles_window_crossing_fence_and_the_drag_direction_picks_the_first() {
    assert_eq!(LassoMode::Window.next(), LassoMode::Crossing);
    assert_eq!(LassoMode::Crossing.next(), LassoMode::Fence);
    assert_eq!(LassoMode::Fence.next(), LassoMode::Window);
    assert_eq!(LassoMode::from_direction(&pts(&[[0.0, 0.0], [0.0, 3.0], [2.0, 5.0]])), LassoMode::Window, "first sideways move is right");
    assert_eq!(LassoMode::from_direction(&pts(&[[5.0, 0.0], [5.0, 3.0], [2.0, 5.0], [9.0, 9.0]])), LassoMode::Crossing);
    assert_eq!(LassoMode::from_direction(&[]), LassoMode::Window);
    assert_eq!(LassoMode::from_name(" Crossing "), Some(LassoMode::Crossing));
    assert_eq!(LassoMode::from_name("f"), Some(LassoMode::Fence));
    assert_eq!(LassoMode::from_name("polygon"), None);
}

#[test]
fn degenerate_and_hostile_lasso_paths_select_nothing_and_never_panic() {
    let sc = scene();
    let d = sc.s.doc().unwrap();
    let m = cadaddy_doc::Space::Model;
    for mode in [LassoMode::Window, LassoMode::Crossing, LassoMode::Fence] {
        assert!(select::select_lasso(d, &m, &[], mode).is_empty());
        assert!(select::select_lasso(d, &m, &pts(&[[1.0, 1.0]]), mode).is_empty());
        let nan = [Vec2::new(f64::NAN, 0.0), Vec2::new(f64::INFINITY, 1.0), Vec2::new(0.0, f64::NEG_INFINITY)];
        assert!(select::select_lasso(d, &m, &nan, mode).is_empty());
    }
    // A huge path is capped, not refused.
    let big: Vec<Vec2> = (0..50_000).map(|i| Vec2::new(f64::from(i % 7), f64::from(i % 11))).collect();
    let _ = select::select_lasso(d, &m, &big, LassoMode::Crossing);
}

#[test]
fn the_select_command_takes_a_lasso() {
    let mut sc = scene();
    let r = sc.s.execute("select", &json!({ "lasso": L, "mode": "window" })).unwrap();
    assert_eq!(r["handles"], hexes(&[sc.inside]));
    // No mode: the direction decides. This path first heads left: a crossing lasso.
    let leftward = [[10.0, 0.0], [0.0, 0.0], [0.0, 10.0], [4.0, 10.0], [4.0, 4.0], [10.0, 4.0]];
    let r = sc.s.execute("select", &json!({ "lasso": leftward })).unwrap();
    assert_eq!(r["handles"], hexes(&[sc.inside, sc.crossing]));
    let r = sc.s.execute("select", &json!({ "lasso": [[9, 0], [9, 5]], "mode": "fence", "add": true })).unwrap();
    assert_eq!(r["selected"], 2, "the fence's line was already selected: {r}");
    assert!(sc.s.execute("select", &json!({ "lasso": [[0, 0], [5, 5]] })).is_err(), "a window lasso needs three points");
    assert!(sc.s.execute("select", &json!({ "lasso": [[0, 0]], "mode": "fence" })).is_err());
    assert!(sc.s.execute("select", &json!({ "lasso": L, "mode": "polygon" })).is_err());
    // A point that isn't one never panics.
    let _ = sc.s.execute("select", &json!({ "lasso": [[0, 0], ["x", 1], [3, 3]] }));
}

#[test]
fn an_idle_lasso_adds_to_the_selection_and_shift_removes() {
    let mut sc = scene();
    sc.s.lasso(&pts(&L), LassoMode::Crossing, false).unwrap();
    assert_eq!(sc.s.selection(), vec![sc.inside, sc.crossing]);
    sc.s.lasso(&pts(&[[9.0, 0.0], [9.0, 5.0]]), LassoMode::Fence, true).unwrap();
    assert_eq!(sc.s.selection(), vec![sc.inside]);
    // It ends a pending window.
    assert!(sc.s.begin_window(Vec2::new(-5.0, -5.0)));
    sc.s.lasso(&pts(&L), LassoMode::Window, false).unwrap();
    assert!(sc.s.pending_window.is_none());
}

#[test]
fn a_lasso_at_select_objects_hands_its_objects_to_the_command() {
    let mut sc = scene();
    sc.s.start("erase").unwrap();
    sc.s.lasso(&pts(&L), LassoMode::Crossing, false).unwrap();
    sc.s.input(Input::Enter).unwrap();
    let left: Vec<Handle> = sc.s.doc().unwrap().model.iter().map(|e| e.handle).collect();
    assert_eq!(left, vec![sc.notch, sc.away]);
}

#[test]
fn a_lasso_while_a_command_asks_for_a_point_does_nothing() {
    let mut sc = scene();
    sc.s.start("line").unwrap();
    sc.s.lasso(&pts(&L), LassoMode::Crossing, false).unwrap();
    assert!(sc.s.selection().is_empty());
    assert_eq!(sc.s.doc().unwrap().model.len(), 4);
    assert!(sc.s.running.is_some(), "LINE still asks for its first point");
}

/// Two lines crossing at (5,5), the second drawn on top, and a third just above that point.
fn overlap() -> (Session, Handle, Handle, Handle) {
    let mut s = Session::new();
    let under = line(&mut s, [0.0, 0.0], [10.0, 10.0]);
    let top = line(&mut s, [0.0, 10.0], [10.0, 0.0]);
    let near = line(&mut s, [0.0, 5.2], [10.0, 5.2]);
    let _far = line(&mut s, [0.0, 9.0], [10.0, 9.0]);
    (s, under, top, near)
}

#[test]
fn pick_all_lists_every_object_under_the_pick_nearest_first_then_topmost() {
    let (s, under, top, near) = overlap();
    let got = select::pick_all(s.doc().unwrap(), &cadaddy_doc::Space::Model, Vec2::new(5.0, 5.0), 0.5);
    assert_eq!(got, vec![top, under, near]);
    assert_eq!(
        select::pick(s.doc().unwrap(), &cadaddy_doc::Space::Model, Vec2::new(5.0, 5.0), 0.5),
        got.first().copied(),
        "a plain pick takes the first candidate"
    );
    assert!(select::pick_all(s.doc().unwrap(), &cadaddy_doc::Space::Model, Vec2::new(50.0, 50.0), 0.5).is_empty());
    assert!(select::pick_all(s.doc().unwrap(), &cadaddy_doc::Space::Model, Vec2::new(f64::NAN, 5.0), 0.5).is_empty());
    assert!(select::pick_all(s.doc().unwrap(), &cadaddy_doc::Space::Model, Vec2::new(5.0, 5.0), f64::INFINITY).is_empty());
}

#[test]
fn pick_all_is_capped() {
    let mut s = Session::new();
    for _ in 0..(select::MAX_PICK_CANDIDATES + 20) {
        line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    }
    assert_eq!(select::pick_all(s.doc().unwrap(), &cadaddy_doc::Space::Model, Vec2::new(5.0, 0.0), 0.5).len(), select::MAX_PICK_CANDIDATES);
}

#[test]
fn the_select_command_cycles_through_the_objects_at_a_point() {
    let (mut s, under, top, near) = overlap();
    let all = hexes(&[top, under, near]);
    for (n, want) in [(0, top), (1, under), (2, near), (3, top), (7, under)] {
        let r = s.execute("select", &json!({ "at": [5, 5], "aperture": 0.5, "cycle": n })).unwrap();
        assert_eq!(r["handles"], hexes(&[want]), "cycle {n}");
        assert_eq!(r["candidates"], all);
    }
    let r = s.execute("select", &json!({ "at": [50, 50], "aperture": 0.5, "cycle": 1 })).unwrap();
    assert_eq!(r["selected"], 0);
    assert_eq!(r["candidates"], json!([]));
    assert!(s.execute("select", &json!({ "at": [5, 5], "cycle": -1 })).is_err());
    assert!(s.execute("select", &json!({ "at": [5, 5], "cycle": "next" })).is_err());
    let r = s.execute("select", &json!({ "at": [5, 5], "aperture": 0.5 })).unwrap();
    assert!(r.get("candidates").is_none(), "no cycle asked, no list: {r}");
}

#[test]
fn a_cycled_pick_goes_to_the_selection_or_to_the_command() {
    let (mut s, under, top, near) = overlap();
    s.execute("view.set", &json!({ "center": [5, 5], "height": 20 })).unwrap();
    s.viewport_px = (800.0, 800.0);
    let cands = s.pick_candidates(Vec2::new(5.0, 5.0)).unwrap();
    assert_eq!(cands.first(), Some(&top));
    assert!(cands.contains(&under));
    assert!(!cands.contains(&near), "outside the pick box at this zoom");
    s.pick_handle(under, false).unwrap();
    assert_eq!(s.selection(), vec![under]);
    s.pick_handle(under, true).unwrap();
    assert!(s.selection().is_empty(), "Shift removes it, as for any pick");
    assert!(s.pick_handle(Handle(0xDEAD_BEEF), false).is_err());
    // At "Select objects" the chosen object goes to the command.
    s.start("erase").unwrap();
    s.pick_handle(under, false).unwrap();
    s.input(Input::Enter).unwrap();
    assert!(s.doc().unwrap().entity(under).is_none());
    assert!(s.doc().unwrap().entity(top).is_some());
}

fn getvar(s: &mut Session, name: &str) -> Value {
    s.execute("getvar", &json!({ "name": name })).unwrap()["value"].clone()
}

#[test]
fn selectioncycling_defaults_to_the_list_and_the_toggle_remembers_its_mode() {
    let mut s = Session::new();
    assert_eq!(getvar(&mut s, "SELECTIONCYCLING"), json!(2));
    s.execute("setvar", &json!({ "name": "SELECTIONCYCLING", "value": 1 })).unwrap();
    assert_eq!(getvar(&mut s, "SELECTIONCYCLING"), json!(1));
    for bad in [json!(3), json!(-3), json!("x")] {
        assert!(s.execute("setvar", &json!({ "name": "SELECTIONCYCLING", "value": bad })).is_err(), "{bad}");
    }
    let r = s.execute("selectioncycling", &json!({})).unwrap();
    assert_eq!(r["on"], false);
    assert_eq!(getvar(&mut s, "SELECTIONCYCLING"), json!(-1), "off, remembering mode 1");
    s.execute("selectioncycling", &json!({})).unwrap();
    assert_eq!(getvar(&mut s, "SELECTIONCYCLING"), json!(1));
    s.execute("selectioncycling", &json!({ "on": true })).unwrap();
    assert_eq!(getvar(&mut s, "SELECTIONCYCLING"), json!(1), "already on");
    s.execute("setvar", &json!({ "name": "SELECTIONCYCLING", "value": 0 })).unwrap();
    s.execute("selectioncycling", &json!({})).unwrap();
    assert_eq!(getvar(&mut s, "SELECTIONCYCLING"), json!(2), "on from 0 shows the list");
    // A user profile setting, kept between sessions.
    let profile = s.execute("options", &json!({})).unwrap();
    assert_eq!(profile["SELECTIONCYCLING"], json!(2), "{profile}");
}
