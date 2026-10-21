//! The everyday AutoCAD commands added in the parity2 pass: OOPS, MULTIPLE, COPYTOLAYER,
//! CUTBASE, DIMCENTER, SOLID, FILL, QTEXT, -PAN, VPMAX / VPMIN, FILTER, ATTREDEF, DXFOUT /
//! DXFIN, the New UCS menu items and UCSMAN, and the View menu option items.

use cadaddy_engine::doc::{EntityKind, Space};
use cadaddy_engine::geom::Vec2;
use cadaddy_engine::render::{Kind, Options, build};
use cadaddy_engine::{Input, Session};
use serde_json::{Value, json};

fn count(s: &Session, pred: impl Fn(&EntityKind) -> bool) -> usize {
    s.doc().unwrap().model.iter().filter(|e| pred(&e.kind)).count()
}

fn handle(v: &Value) -> String {
    v["handle"].as_str().or_else(|| v["handles"][0].as_str()).unwrap().to_string()
}

fn circle(s: &mut Session, x: f64, y: f64, r: f64) -> String {
    handle(&s.execute("circle", &json!({ "center": [x, y], "radius": r })).unwrap())
}

fn lines(s: &Session) -> Vec<(Vec2, Vec2)> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Line(l) => Some((l.a.xy(), l.b.xy())),
            _ => None,
        })
        .collect()
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

// ---------------- OOPS ----------------

#[test]
fn oops_brings_back_what_erase_removed_once() {
    let mut s = Session::new();
    let a = circle(&mut s, 0.0, 0.0, 1.0);
    let b = circle(&mut s, 5.0, 0.0, 1.0);
    s.execute("erase", &json!({ "handles": [a, b] })).unwrap();
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Circle(_))), 0);
    // OOPS isn't undo: a command run since doesn't stop it.
    s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    let r = s.execute("oops", &json!({})).unwrap();
    assert_eq!(r["restored"], 2);
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Circle(_))), 2);
    assert!(s.doc().unwrap().entity(cadaddy_engine::doc::Handle::parse_hex(&a).unwrap()).is_some(), "the same handle comes back");
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Line(_))), 1, "the line drawn since stays");
    // Once only, and OOPS is undoable.
    assert!(s.execute("oops", &json!({})).is_err());
    s.undo().unwrap();
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Circle(_))), 0);
}

#[test]
fn oops_after_undo_does_not_duplicate_and_clear_counts() {
    let mut s = Session::new();
    let a = circle(&mut s, 0.0, 0.0, 1.0);
    s.execute("erase", &json!({ "handles": [a] })).unwrap();
    s.undo().unwrap();
    let r = s.execute("oops", &json!({})).unwrap();
    assert_eq!(r["restored"], 0, "already back by undo");
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Circle(_))), 1);
    // Edit ▸ Clear (Delete key) erases too.
    let h = cadaddy_engine::doc::Handle::parse_hex(&a).unwrap();
    s.set_selection(vec![h]);
    s.execute("erase.selection", &json!({})).unwrap();
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Circle(_))), 0);
    s.cmdline("OOPS").unwrap();
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Circle(_))), 1, "{:?}", s.log);
}

#[test]
fn oops_puts_paper_space_objects_back_in_their_layout() {
    let mut s = Session::new();
    s.execute("layout.new", &json!({ "name": "Sheet", "viewport": false })).unwrap();
    s.execute("layout.set", &json!({ "name": "Sheet" })).unwrap();
    let h = circle(&mut s, 1.0, 1.0, 0.5);
    s.execute("erase", &json!({ "handles": [h] })).unwrap();
    s.execute("layout.set", &json!({ "name": "Model" })).unwrap();
    s.execute("oops", &json!({})).unwrap();
    let d = s.doc().unwrap();
    let hh = cadaddy_engine::doc::Handle::parse_hex(&h).unwrap();
    assert_eq!(d.space_of(hh), Some(Space::Paper("Sheet".into())));
}

// ---------------- MULTIPLE ----------------

#[test]
fn multiple_repeats_until_escape() {
    let mut s = Session::new();
    s.script("MULTIPLE CIRCLE 0,0 1 5,5 2 10,10 3\n").unwrap();
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Circle(_))), 3, "{:?}", s.log);
    assert!(s.running.as_ref().is_some_and(|r| r.id == "circle"), "CIRCLE asks again: {}", s.prompt_text());
    s.input(Input::Cancel).unwrap();
    assert!(s.running.is_none());
    // Esc ended the repeating: the next CIRCLE runs once.
    s.script("CIRCLE 20,20 1\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Circle(_))), 4);
}

#[test]
fn multiple_line_repeats_after_enter_and_another_command_stops_it() {
    let mut s = Session::new();
    s.script("MULTIPLE LINE 0,0 1,0  2,0 3,0  \n").unwrap();
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Line(_))), 2, "{:?}", s.log);
    assert!(s.running.as_ref().is_some_and(|r| r.id == "line"));
    // Starting another command ends the repeating.
    s.start("circle").unwrap();
    s.script("0,0 1\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
}

#[test]
fn multiple_refuses_what_it_cannot_repeat() {
    let mut s = Session::new();
    s.cmdline("MULTIPLE").unwrap();
    s.cmdline("REGEN").unwrap();
    assert!(s.log.iter().any(|l| l.contains("doesn't ask for input")), "{:?}", s.log);
    s.cmdline("NOSUCHCOMMAND").unwrap();
    assert!(s.log.iter().any(|l| l.contains("Unknown command")), "{:?}", s.log);
    s.cmdline("UNDO").unwrap();
    assert!(s.log.iter().any(|l| l.contains("does not repeat")), "{:?}", s.log);
    s.input(Input::Cancel).unwrap();
    assert!(s.running.is_none());
    assert!(s.execute("multiple", &json!({ "command": "circle" })).is_err(), "typed only");
}

// ---------------- COPYTOLAYER ----------------

#[test]
fn copytolayer_json_copies_to_a_new_layer_moved() {
    let mut s = Session::new();
    let a = circle(&mut s, 0.0, 0.0, 1.0);
    let r = s.execute("copytolayer", &json!({ "handles": [a], "layer": "Copies", "delta": [10, 0] })).unwrap();
    assert_eq!(r["layer"], "Copies");
    let d = s.doc().unwrap();
    assert!(d.layer("Copies").is_some());
    let c = d.entity(cadaddy_engine::doc::Handle::parse_hex(r["handles"][0].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(c.common.layer, "Copies");
    match &c.kind {
        EntityKind::Circle(ci) => assert!(near(ci.center.x, 10.0)),
        k => panic!("{k:?}"),
    }
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Circle(_))), 2, "the original stays");
    assert!(s.execute("copytolayer", &json!({ "handles": [a], "layer": "Nope", "create": false })).is_err());
    assert!(s.execute("copytolayer", &json!({ "handles": [a], "layer": "bad<name" })).is_err());
    assert!(s.execute("copytolayer", &json!({ "handles": [a] })).is_err());
    assert!(s.execute("copytolayer", &json!({ "handles": [a], "layer": "X", "delta": [1e308, 1e308] })).is_err());
}

#[test]
fn copytolayer_typed_by_name_and_by_object() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({ "name": "Walls" })).unwrap();
    let a = circle(&mut s, 0.0, 0.0, 1.0);
    let ha = cadaddy_engine::doc::Handle::parse_hex(&a).unwrap();
    // By name, then a displacement by two points.
    s.set_selection(vec![ha]);
    s.script("COPYTOLAYER N Walls 0,0 0,5\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    let on_walls: Vec<_> = s.doc().unwrap().model.iter().filter(|e| e.common.layer == "Walls").cloned().collect();
    assert_eq!(on_walls.len(), 1, "{:?}", s.log);
    match &on_walls[0].kind {
        EntityKind::Circle(c) => assert!(near(c.center.y, 5.0)),
        k => panic!("{k:?}"),
    }
    // By picking an object on the destination layer, in place (Enter = eXit).
    let line = handle(&s.execute("line", &json!({ "points": [[20, 0], [30, 0]] })).unwrap());
    s.execute("chprop", &json!({ "handles": [line], "layer": "Walls" })).ok();
    let line_layer = s.doc().unwrap().entity(cadaddy_engine::doc::Handle::parse_hex(&line).unwrap()).unwrap().common.layer.clone();
    s.set_selection(vec![ha]);
    s.start("copytolayer").unwrap();
    s.input(Input::Point(Vec2::new(25.0, 0.0))).unwrap();
    s.input(Input::Enter).unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_eq!(
        s.doc().unwrap().model.iter().filter(|e| e.common.layer == line_layer && matches!(e.kind, EntityKind::Circle(_))).count(),
        if line_layer == "Walls" { 2 } else { 1 }
    );
    assert!(s.log.iter().any(|l| l.contains("copied and placed on layer")), "{:?}", s.log);
}

// ---------------- CUTBASE ----------------

#[test]
fn cutbase_cuts_with_the_base_point() {
    let mut s = Session::new();
    let a = circle(&mut s, 3.0, 4.0, 1.0);
    s.execute("cutbase", &json!({ "handles": [a], "base": [3, 4] })).unwrap();
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Circle(_))), 0);
    let r = s.execute("pasteclip", &json!({ "at": [10, 10] })).unwrap();
    let h = cadaddy_engine::doc::Handle::parse_hex(r["handles"][0].as_str().unwrap()).unwrap();
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Circle(c) => assert!(near(c.center.x, 10.0) && near(c.center.y, 10.0), "the base point lands at `at`"),
        k => panic!("{k:?}"),
    }
    assert!(s.execute("cutbase", &json!({ "handles": [r["handles"][0]] })).is_err(), "base is required");
    // Typed: base point first, then the objects.
    s.script("CUTBASE 10,10 L \n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Circle(_))), 0, "{:?}", s.log);
    let r = s.execute("pasteclip", &json!({ "at": [0, 0] })).unwrap();
    let h = cadaddy_engine::doc::Handle::parse_hex(r["handles"][0].as_str().unwrap()).unwrap();
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Circle(c) => assert!(near(c.center.x, 0.0) && near(c.center.y, 0.0)),
        k => panic!("{k:?}"),
    }
}

// ---------------- DIMCENTER ----------------

#[test]
fn dimcenter_follows_dimcen() {
    let mut s = Session::new();
    s.execute("setvar", &json!({ "name": "DIMCEN", "value": 0.5 })).unwrap();
    let c = circle(&mut s, 0.0, 0.0, 4.0);
    let r = s.execute("dimcenter", &json!({ "handle": c })).unwrap();
    assert_eq!(r["handles"].as_array().unwrap().len(), 2, "a cross");
    let ls = lines(&s);
    assert!(ls.iter().any(|(a, b)| near(a.x, -0.5) && near(b.x, 0.5) && near(a.y, 0.0)), "{ls:?}");
    // Negative: the cross and center lines from twice DIMCEN to DIMCEN past the circle.
    s.execute("setvar", &json!({ "name": "DIMCEN", "value": -0.5 })).unwrap();
    let c2 = circle(&mut s, 20.0, 0.0, 4.0);
    let r = s.execute("dimcenter", &json!({ "handle": c2 })).unwrap();
    assert_eq!(r["handles"].as_array().unwrap().len(), 6);
    let ls = lines(&s);
    assert!(ls.iter().any(|(a, b)| near(a.x, 21.0) && near(b.x, 24.5)), "{ls:?}");
    // 0: nothing.
    s.execute("setvar", &json!({ "name": "DIMCEN", "value": 0 })).unwrap();
    let r = s.execute("dimcenter", &json!({ "handle": c2 })).unwrap();
    assert!(r["handles"].as_array().unwrap().is_empty());
    let l = handle(&s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap());
    assert!(s.execute("dimcenter", &json!({ "handle": l })).is_err());
    // Typed: pick the arc or circle.
    s.execute("setvar", &json!({ "name": "DIMCEN", "value": 0.25 })).unwrap();
    let before = lines(&s).len();
    s.start("dimcenter").unwrap();
    s.input(Input::Point(Vec2::new(4.0, 0.0))).unwrap();
    assert!(s.running.is_none());
    assert_eq!(lines(&s).len(), before + 2, "{:?}", s.log);
}

// ---------------- SOLID ----------------

fn solids(s: &Session) -> Vec<[Vec2; 4]> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Solid(so) => Some(so.corners.map(|c| c.xy())),
            _ => None,
        })
        .collect()
}

#[test]
fn solid_pieces_share_their_last_edge() {
    let mut s = Session::new();
    let r = s.execute("solid", &json!({ "points": [[0, 0], [10, 0], [0, 5], [10, 5], [0, 10], [10, 10]] })).unwrap();
    assert_eq!(r["handles"].as_array().unwrap().len(), 2);
    let so = solids(&s);
    assert_eq!(so[1][0], Vec2::new(0.0, 5.0));
    assert_eq!(so[1][3], Vec2::new(10.0, 10.0));
    // Three points: a triangle (fourth corner = third).
    s.execute("solid", &json!({ "points": [[20, 0], [30, 0], [25, 5]] })).unwrap();
    let so = solids(&s);
    assert_eq!(so[2][2], so[2][3]);
    for bad in [json!({}), json!({ "points": [[0, 0], [1, 1]] }), json!({ "points": [[0, 0], [1, 1], ["x", 0]] })] {
        assert!(s.execute("solid", &bad).is_err(), "{bad}");
    }
}

#[test]
fn solid_typed_with_triangle_and_continue() {
    let mut s = Session::new();
    // 1,2,3,4 → a quad; 5 then Enter at "fourth point" → a triangle; Enter at "third" ends.
    s.script("SOLID 0,0 10,0 0,5 10,5 5,9  \n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    let so = solids(&s);
    assert_eq!(so.len(), 2, "{:?}", s.log);
    assert_eq!(so[1][0], Vec2::new(0.0, 5.0));
    assert_eq!(so[1][2], Vec2::new(5.0, 9.0));
    assert_eq!(so[1][3], Vec2::new(5.0, 9.0));
}

// ---------------- FILL / QTEXT (drawn) ----------------

fn display(s: &Session) -> cadaddy_engine::render::DisplayList {
    build(s.doc().unwrap(), &Space::Model, &Options::default())
}

#[test]
fn fill_off_outlines_solids_and_solid_hatches() {
    let mut s = Session::new();
    s.execute("solid", &json!({ "points": [[0, 0], [10, 0], [0, 5], [10, 5]] })).unwrap();
    s.execute("rectang", &json!({ "p1": [20, 0], "p2": [30, 10] })).ok();
    s.execute("hatch", &json!({ "points": [[25, 5]], "pattern": "SOLID" })).ok();
    let tris = |s: &Session| display(s).prims.iter().filter(|p| p.kind == Kind::Tris).count();
    assert!(tris(&s) >= 1);
    let r = s.execute("fill", &json!({ "mode": "off" })).unwrap();
    assert_eq!(r["fillmode"], 0);
    assert_eq!(tris(&s), 0, "FILL off: no filled triangles");
    assert_eq!(s.execute("fill", &json!({})).unwrap()["fillmode"], 0, "no mode reports it");
    s.script("FILL ON\n").unwrap();
    assert_eq!(s.doc().unwrap().header.i64("FILLMODE", 1), 1);
    assert!(tris(&s) >= 1);
    assert!(s.execute("fill", &json!({ "mode": "sideways" })).is_err());
}

#[test]
fn qtext_draws_text_as_its_box() {
    let mut s = Session::new();
    s.execute("text", &json!({ "at": [0, 0], "text": "QUICK", "height": 2.5 })).unwrap();
    s.execute("text", &json!({ "at": [20, 0], "text": "UP", "height": 2.5, "rotation": 90 })).unwrap();
    s.execute("mtext", &json!({ "at": [0, 20], "text": "two\\Plines", "height": 1 })).ok();
    let full = display(&s).prims.len();
    s.execute("qtext", &json!({ "mode": "on" })).unwrap();
    let l = display(&s);
    let boxes: Vec<Vec<Vec2>> = l.prims.iter().filter(|p| p.kind == Kind::Polyline).map(|p| l.points(p).to_vec()).collect();
    assert!(l.prims.len() < full, "boxes instead of strokes ({} → {})", full, l.prims.len());
    assert!(boxes.len() >= 2 && boxes.iter().all(|b| b.len() == 5 && b.first() == b.last()), "{boxes:?}");
    // The horizontal text's box is wider than tall; the turned one taller than wide.
    let size = |b: &[Vec2]| {
        let (xs, ys): (Vec<f64>, Vec<f64>) = b.iter().map(|p| (p.x, p.y)).unzip();
        let span = |v: &[f64]| v.iter().cloned().fold(f64::MIN, f64::max) - v.iter().cloned().fold(f64::MAX, f64::min);
        (span(&xs), span(&ys))
    };
    let horizontal = boxes.iter().find(|b| b.iter().all(|p| p.x < 15.0 && p.y < 10.0)).expect("QUICK's box");
    let (w, h) = size(horizontal);
    assert!(w > h && h > 1.0, "{w} × {h}");
    let turned = boxes.iter().find(|b| b.iter().all(|p| p.x > 15.0)).expect("UP's box");
    let (w, h) = size(turned);
    assert!(h > w, "{w} × {h}");
    s.script("QTEXT OFF\n").unwrap();
    assert_eq!(display(&s).prims.len(), full);
}

// ---------------- -PAN, View and Tools menu items ----------------

#[test]
fn dash_pan_moves_the_view_by_the_displacement() {
    let mut s = Session::new();
    let c0 = s.state().unwrap().view().center;
    s.execute("-pan", &json!({ "delta": [5, 2] })).unwrap();
    let c1 = s.state().unwrap().view().center;
    assert!(near(c1.x, c0.x - 5.0) && near(c1.y, c0.y - 2.0));
    s.script("-PAN 0,0 1,1\n").unwrap();
    let c2 = s.state().unwrap().view().center;
    assert!(near(c2.x, c1.x - 1.0) && near(c2.y, c1.y - 1.0), "{c1:?} {c2:?}");
    // Hostile: refused or done, never a panic.
    let _ = s.execute("-pan", &json!({ "delta": [1e308, 1e308] }));
    assert!(s.execute("-pan", &json!({})).is_err());
}

#[test]
fn view_menu_items_set_attdisp_and_ucsicon() {
    let mut s = Session::new();
    for (id, mode) in [("attdisp.on", 2), ("attdisp.off", 0), ("attdisp.normal", 1)] {
        s.execute(id, &json!({})).unwrap();
        assert_eq!(s.doc().unwrap().header.i64("ATTMODE", 1), mode, "{id}");
    }
    let icon = |s: &Session| s.doc().unwrap().header.i64("UCSICON", 3);
    let start = icon(&s);
    s.execute("ucsicon.on", &json!({})).unwrap();
    assert_eq!(icon(&s) & 1, (start & 1) ^ 1);
    s.execute("ucsicon.on", &json!({})).unwrap();
    s.execute("ucsicon.origin", &json!({})).unwrap();
    assert_eq!(icon(&s) & 2, (start & 2) ^ 2);
}

#[test]
fn new_ucs_menu_items_and_three_points() {
    let mut s = Session::new();
    s.execute("ucs.origin", &json!({ "origin": [5, 5] })).unwrap();
    assert_eq!(s.ucs().origin, Vec2::new(5.0, 5.0));
    s.execute("ucs.z", &json!({ "angle": 30 })).unwrap();
    assert!(near(s.ucs().angle.to_degrees(), 30.0));
    s.execute("ucs.3point", &json!({ "origin": [1, 1], "xpoint": [1, 3], "ypoint": [0, 1] })).unwrap();
    assert_eq!(s.ucs().origin, Vec2::new(1.0, 1.0));
    assert!(near(s.ucs().angle.to_degrees(), 90.0));
    // A Y point on the clockwise side would turn the plan over; in line, no plane.
    assert!(s.execute("ucs.3point", &json!({ "origin": [0, 0], "xpoint": [1, 0], "ypoint": [0, -1] })).is_err());
    assert!(s.execute("ucs.3point", &json!({ "origin": [0, 0], "xpoint": [1, 0], "ypoint": [2, 0] })).is_err());
    assert!(s.execute("ucs.3point", &json!({ "origin": [0, 0], "xpoint": [0, 0] })).is_err());
    // Typed from the menu: three points.
    s.start("ucs.3point").unwrap();
    for p in [(2.0, 2.0), (2.0, 0.0), (5.0, 2.0)] {
        s.input(Input::Point(Vec2::new(p.0, p.1))).unwrap();
    }
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert!(near(s.ucs().angle.to_degrees().rem_euclid(360.0), 270.0), "{}", s.ucs().angle.to_degrees());
    s.start("ucs.origin").unwrap();
    s.input(Input::Point(Vec2::new(7.0, 8.0))).unwrap();
    assert_eq!(s.ucs().origin, Vec2::new(7.0, 8.0));
    s.script("UCS.Z 45\n").unwrap();
    assert!(near(s.ucs().angle.to_degrees().rem_euclid(360.0), 315.0), "{}", s.ucs().angle.to_degrees());
}

#[test]
fn ucsman_lists_restores_renames_and_deletes() {
    let mut s = Session::new();
    s.execute("ucs", &json!({ "option": "set", "origin": [3, 4], "angle": 10 })).unwrap();
    s.execute("ucs", &json!({ "option": "save", "name": "Grid A" })).unwrap();
    s.execute("ucs", &json!({ "option": "world" })).unwrap();
    let r = s.execute("ucsman", &json!({})).unwrap();
    assert_eq!(r["ucs"][0]["name"], "Grid A");
    s.execute("ucsman", &json!({ "option": "restore", "name": "grid a" })).unwrap();
    assert_eq!(s.ucs().origin, Vec2::new(3.0, 4.0));
    s.execute("ucsman", &json!({ "option": "rename", "name": "Grid A", "newName": "Grid B" })).unwrap();
    assert_eq!(s.doc().unwrap().ucss[0].name, "Grid B");
    assert!(s.execute("ucsman", &json!({ "option": "rename", "name": "Grid B", "newName": "World" })).is_err());
    assert!(s.execute("ucsman", &json!({ "option": "rename", "name": "Nope", "newName": "X" })).is_err());
    s.script("UCSMAN REN \"Grid B\" Grid C\n").ok();
    s.input(Input::Cancel).ok();
    s.execute("ucsman", &json!({ "option": "delete", "name": "Grid*" })).unwrap();
    assert!(s.doc().unwrap().ucss.is_empty());
}

// ---------------- VPMAX / VPMIN ----------------

#[test]
fn vpmax_fills_the_screen_with_the_viewport_and_vpmin_puts_the_layout_back() {
    let mut s = Session::new();
    circle(&mut s, 0.0, 0.0, 5.0);
    assert!(s.execute("vpmax", &json!({})).is_err(), "not on the Model tab");
    s.execute("layout.new", &json!({ "name": "Sheet" })).unwrap();
    s.execute("layout.set", &json!({ "name": "Sheet" })).unwrap();
    let before = s.state().unwrap().paper_view();
    let r = s.execute("vpmax", &json!({})).unwrap();
    let vp = cadaddy_engine::doc::Handle::parse_hex(r["viewport"].as_str().unwrap()).unwrap();
    let st = s.state().unwrap();
    assert_eq!(st.mspace, Some(vp), "its model space is active");
    let v = match &st.doc.space(&st.space).unwrap().get(vp).unwrap().kind {
        EntityKind::Viewport(v) => v.clone(),
        k => panic!("{k:?}"),
    };
    let pv = st.paper_view();
    assert!(near(pv.center.x, v.center.x) && near(pv.center.y, v.center.y));
    assert!(pv.height >= v.height - 1e-9 && pv.height < before.height);
    // VPMIN: the sheet as it was, in paper space.
    s.execute("vpmin", &json!({})).unwrap();
    let st = s.state().unwrap();
    assert_eq!(st.mspace, None);
    assert_eq!(st.paper_view(), before);
    assert_eq!(st.space, Space::Paper("Sheet".into()));
    assert!(s.execute("vpmin", &json!({})).is_err(), "nothing maximized");
    // Typed: with one viewport VPMAX acts at once.
    s.cmdline("VPMAX").unwrap();
    assert!(s.running.is_none());
    assert_eq!(s.state().unwrap().mspace, Some(vp));
    assert!(s.execute("vpmax", &json!({ "handle": "FFFFFF" })).is_err());
}

// ---------------- FILTER ----------------

#[test]
fn filter_selects_by_conditions_joined_with_and_or_not() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({ "name": "Walls" })).unwrap();
    let small = circle(&mut s, 0.0, 0.0, 1.0);
    let big = circle(&mut s, 10.0, 0.0, 5.0);
    let line = handle(&s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap());
    s.execute("chprop", &json!({ "handles": [line], "layer": "Walls" })).unwrap();
    let pick = |s: &mut Session, f: Value| -> Vec<String> {
        let r = s.execute("filter", &json!({ "filter": f })).unwrap();
        let mut v: Vec<String> = r["handles"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_string()).collect();
        v.sort();
        v
    };
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    assert_eq!(pick(&mut s, json!("type = Circle and radius > 2")), vec![big.clone()]);
    assert_eq!(pick(&mut s, json!("type = circle and radius <= 1")), vec![small.clone()]);
    assert_eq!(pick(&mut s, json!("layer = Walls or (type = Circle and radius >= 5)")), sorted(vec![line.clone(), big.clone()]));
    assert_eq!(pick(&mut s, json!("not type = Circle")), vec![line.clone()]);
    assert_eq!(pick(&mut s, json!("layer *= W*")), vec![line.clone()]);
    assert_eq!(pick(&mut s, json!("type = LINE xor layer = Walls")), Vec::<String>::new());
    assert_eq!(pick(&mut s, json!("layer = \"Walls\" and type != Circle")), vec![line.clone()]);
    // The JSON tree.
    let tree = json!({ "or": [{ "property": "radius", "op": ">", "value": 2 }, { "not": { "property": "type", "value": "Circle" } }] });
    assert_eq!(pick(&mut s, tree), sorted(vec![line.clone(), big.clone()]));
    // It selects them, for the next command.
    assert_eq!(s.selection().len(), 2);
    // Typed.
    s.script("FILTER type = Circle\n").unwrap();
    assert!(s.running.is_none());
    assert_eq!(s.selection().len(), 2, "{:?}", s.log);
}

#[test]
fn filter_refuses_bad_and_hostile_filters() {
    let mut s = Session::new();
    circle(&mut s, 0.0, 0.0, 1.0);
    let deep = format!("{}type = Circle{}", "(".repeat(500), ")".repeat(500));
    let many = vec!["type = Circle"; 1000].join(" and ");
    let mut tree = json!({ "property": "type", "value": "Circle" });
    for _ in 0..100 {
        tree = json!({ "not": tree });
    }
    for f in [
        json!(""),
        json!("type ="),
        json!("type Circle"),
        json!("(type = Circle"),
        json!("type = \"Circle"),
        json!("type = Circle extra"),
        json!("type ?? Circle"),
        json!(deep),
        json!(many),
        json!("x".repeat(20_000)),
        tree,
        json!({ "and": [] }),
        json!({ "property": "type" }),
        json!(5),
    ] {
        assert!(s.execute("filter", &json!({ "filter": f })).is_err(), "{f}");
    }
    assert!(s.execute("filter", &json!({})).is_err());
    assert!(s.execute("filter", &json!({ "filter": "type = Circle", "mode": "sideways" })).is_err());
}

// ---------------- ATTREDEF ----------------

#[test]
fn attredef_keeps_values_of_attributes_still_defined() {
    let mut s = Session::new();
    let a = handle(&s.execute("attdef", &json!({ "tag": "ROOM", "default": "101", "at": [0, 0] })).unwrap());
    let b = handle(&s.execute("attdef", &json!({ "tag": "OLD", "default": "x", "at": [0, -1] })).unwrap());
    let box_ = handle(&s.execute("rectang", &json!({ "p1": [-1, -2], "p2": [5, 2] })).unwrap());
    s.execute("block", &json!({ "name": "TAG", "base": [0, 0], "handles": [a, b, box_], "keep": "delete" })).unwrap();
    let ins = handle(&s.execute("insert", &json!({ "name": "TAG", "at": [10, 10], "attribs": { "ROOM": "205", "OLD": "y" } })).unwrap());
    // The new definition: ROOM stays, OLD goes, AREA is new.
    let a2 = handle(&s.execute("attdef", &json!({ "tag": "ROOM", "default": "000", "at": [0, 0] })).unwrap());
    let c2 = handle(&s.execute("attdef", &json!({ "tag": "AREA", "default": "12 m2", "at": [0, -1] })).unwrap());
    let circ = circle(&mut s, 0.0, 0.0, 3.0);
    let r = s.execute("attredef", &json!({ "name": "tag", "handles": [a2, c2, circ], "base": [0, 0] })).unwrap();
    assert!(r["message"].as_str().unwrap().contains("redefined"));
    let d = s.doc().unwrap();
    let blk = d.block("TAG").unwrap();
    assert!(blk.entities.iter().any(|e| matches!(e.kind, EntityKind::Circle(_))));
    match &d.entity(cadaddy_engine::doc::Handle::parse_hex(&ins).unwrap()).unwrap().kind {
        EntityKind::Insert(i) => {
            let get = |t: &str| i.attribs.iter().find(|a| a.tag == t).map(|a| a.text.value.clone());
            assert_eq!(get("ROOM").as_deref(), Some("205"), "kept");
            assert_eq!(get("AREA").as_deref(), Some("12 m2"), "new: its default");
            assert_eq!(get("OLD"), None, "removed");
        }
        k => panic!("{k:?}"),
    }
    assert_eq!(count(&s, |k| matches!(k, EntityKind::Circle(_))), 0, "the selected objects went into the block");
    assert!(s.execute("attredef", &json!({ "name": "NOBLOCK", "handles": [ins], "base": [0, 0] })).is_err());
    assert!(s.execute("attredef", &json!({ "name": "TAG", "base": [0, 0], "handles": [] })).is_err());
}

#[test]
fn attredef_typed() {
    let mut s = Session::new();
    let a = handle(&s.execute("attdef", &json!({ "tag": "N", "default": "1", "at": [0, 0] })).unwrap());
    s.execute("block", &json!({ "name": "B1", "base": [0, 0], "handles": [a], "keep": "delete" })).unwrap();
    s.execute("insert", &json!({ "name": "B1", "at": [5, 5], "attribs": { "N": "7" } })).unwrap();
    let a2 = handle(&s.execute("attdef", &json!({ "tag": "N", "default": "1", "at": [0, 0] })).unwrap());
    let l = handle(&s.execute("line", &json!({ "points": [[0, 0], [2, 0]] })).unwrap());
    s.start("attredef").unwrap();
    s.input(Input::Text("B1".into())).unwrap();
    let hs = [a2, l].iter().map(|h| cadaddy_engine::doc::Handle::parse_hex(h).unwrap()).collect();
    s.input(Input::Pick(hs)).unwrap();
    s.input(Input::Enter).unwrap();
    s.input(Input::Point(Vec2::ZERO)).unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert!(s.doc().unwrap().block("B1").unwrap().entities.iter().any(|e| matches!(e.kind, EntityKind::Line(_))), "{:?}", s.log);
}

// ---------------- DXFOUT / DXFIN ----------------

#[test]
fn dxfout_writes_dxf_and_dxfin_opens_it() {
    let dir = std::env::temp_dir().join(format!("cadaddy-parity2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    let mut s = Session::new();
    circle(&mut s, 1.0, 2.0, 3.0);
    let base = dir.join("plan_out");
    let r = s.execute("dxfout", &json!({ "path": base.to_string_lossy() })).unwrap();
    let written = dir.join("plan_out.dxf");
    assert!(written.exists(), "{r}");
    assert!(std::fs::read_to_string(&written).unwrap().contains("CIRCLE"));
    assert!(s.execute("dxfout", &json!({ "path": dir.join("x.dwg").to_string_lossy() })).is_err());
    assert!(s.execute("dxfout", &json!({})).is_err());
    let mut t = Session::new();
    t.execute("dxfin", &json!({ "path": written.to_string_lossy() })).unwrap();
    assert_eq!(count(&t, |k| matches!(k, EntityKind::Circle(_))), 1);
    assert!(t.execute("dxfin", &json!({ "path": dir.join("x.dwg").to_string_lossy() })).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
