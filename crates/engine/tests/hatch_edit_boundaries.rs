//! Hatch editing leftovers (gaps F13): HATCHEDIT adds boundaries (internal points, closed
//! objects) and removes them (a point on the loop, or its index), keeping associativity; HATCH
//! and GRADIENT inherit another hatch's properties (`inherit`, typed Inherit).

use cadaddy_engine::Session;
use cadaddy_engine::doc::color::Color;
use cadaddy_engine::doc::{EntityKind, Handle, Hatch, Transparency};
use cadaddy_engine::geom::Vec2;
use serde_json::{Value, json};

fn hatch(s: &Session, h: &str) -> Hatch {
    match &s.doc().unwrap().entity(Handle::parse_hex(h).unwrap()).unwrap().kind {
        EntityKind::Hatch(x) => x.clone(),
        k => panic!("not a hatch: {k:?}"),
    }
}

fn handle(v: &Value) -> String {
    v["handle"].as_str().unwrap().to_string()
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> String {
    handle(&s.execute("rectang", &json!({ "p1": a, "p2": b })).unwrap())
}

/// Two separate squares and a small square inside the first; the first square hatched
/// (associative) without the island... then edited.
fn session() -> (Session, String, String, String, String) {
    let mut s = Session::new();
    s.viewport_px = (1600.0, 900.0);
    let a = rect(&mut s, [0.0, 0.0], [10.0, 10.0]);
    let b = rect(&mut s, [20.0, 0.0], [30.0, 10.0]);
    let i = rect(&mut s, [4.0, 4.0], [6.0, 6.0]);
    let h = handle(&s.execute("hatch", &json!({ "handles": [a], "associative": true })).unwrap());
    (s, h, a, b, i)
}

fn loops_with(h: &Hatch, src: &str) -> usize {
    h.loops.iter().filter(|l| l.sources.iter().any(|x| x.hex() == src)).count()
}

#[test]
fn add_boundaries_by_object_and_by_point() {
    let (mut s, h, a, b, i) = session();
    assert_eq!(hatch(&s, &h).loops.len(), 1);
    // By object: the second square joins the hatch, linked to it.
    s.execute("hatchedit", &json!({ "handles": [h], "addHandles": [b] })).unwrap();
    let x = hatch(&s, &h);
    assert_eq!(x.loops.len(), 2);
    assert!(x.associative);
    assert_eq!(loops_with(&x, &b), 1);
    assert!(x.loops.iter().all(|l| l.outer), "two separate areas are both outer loops");
    // The island by object: it becomes an island (inside the first square).
    s.execute("hatchedit", &json!({ "handles": [h], "addHandles": [i] })).unwrap();
    let x = hatch(&s, &h);
    assert_eq!(x.loops.len(), 3);
    let island = x.loops.iter().find(|l| l.sources.iter().any(|y| y.hex() == i)).unwrap();
    assert!(!island.outer);
    // Adding a boundary the hatch already has changes nothing.
    s.execute("hatchedit", &json!({ "handles": [h], "addHandles": [a] })).unwrap();
    assert_eq!(hatch(&s, &h).loops.len(), 3);
    // Associativity holds: moving the added square moves its loop.
    s.execute("move", &json!({ "handles": [b], "delta": [0, 5] })).unwrap();
    let x = hatch(&s, &h);
    assert!(x.associative);
    let moved = x.loops.iter().find(|l| l.sources.iter().any(|y| y.hex() == b)).unwrap();
    assert!(moved.vertices.iter().any(|v| v.p.near(Vec2::new(20.0, 5.0), 1e-9)), "{:?}", moved.vertices);
}

#[test]
fn add_by_internal_point() {
    let (mut s, h, _a, b, _i) = session();
    s.execute("hatchedit", &json!({ "handles": [h], "addPoints": [[25, 5]] })).unwrap();
    let x = hatch(&s, &h);
    assert_eq!(x.loops.len(), 2);
    assert_eq!(loops_with(&x, &b), 1);
    assert!(s.execute("hatchedit", &json!({ "handles": [h], "addPoints": [[100, 100]] })).is_err(), "no boundary there");
    assert_eq!(hatch(&s, &h).loops.len(), 2, "a refused edit changes nothing");
}

#[test]
fn remove_boundaries_by_point_and_index() {
    let (mut s, h, _a, b, i) = session();
    s.execute("hatchedit", &json!({ "handles": [h], "addHandles": [b, i] })).unwrap();
    assert_eq!(hatch(&s, &h).loops.len(), 3);
    // A point on the island's edge removes the island.
    s.execute("hatchedit", &json!({ "handles": [h], "remove": [[5, 4]], "tolerance": 0.1 })).unwrap();
    let x = hatch(&s, &h);
    assert_eq!(x.loops.len(), 2);
    assert_eq!(loops_with(&x, &i), 0);
    assert!(x.associative);
    // Nothing near the point: refused, unchanged.
    assert!(s.execute("hatchedit", &json!({ "handles": [h], "remove": [[15, 5]], "tolerance": 0.1 })).is_err());
    assert_eq!(hatch(&s, &h).loops.len(), 2);
    // By index.
    s.execute("hatchedit", &json!({ "handles": [h], "removeLoops": [1] })).unwrap();
    assert_eq!(hatch(&s, &h).loops.len(), 1);
    // The last loop can't go, and an index out of range is refused.
    assert!(s.execute("hatchedit", &json!({ "handles": [h], "removeLoops": [0] })).is_err());
    assert!(s.execute("hatchedit", &json!({ "handles": [h], "removeLoops": [5] })).is_err());
    assert_eq!(hatch(&s, &h).loops.len(), 1);
    // One undo step per edit.
    s.undo().unwrap();
    assert_eq!(hatch(&s, &h).loops.len(), 2);
}

#[test]
fn boundary_edits_are_one_hatch_at_a_time_and_validated() {
    let (mut s, h, _a, b, _i) = session();
    let h2 = handle(&s.execute("hatch", &json!({ "handles": [b] })).unwrap());
    assert!(s.execute("hatchedit", &json!({ "handles": [h, h2], "removeLoops": [0] })).is_err());
    for bad in [
        json!({ "handles": [h], "addPoints": "x" }),
        json!({ "handles": [h], "addHandles": [1] }),
        json!({ "handles": [h], "remove": [[0, 0]], "tolerance": -1 }),
        json!({ "handles": [h], "removeLoops": [-1] }),
        json!({ "handles": [h], "remove": [["a", 0]] }),
    ] {
        assert!(s.execute("hatchedit", &bad).is_err(), "{bad}");
    }
    assert_eq!(hatch(&s, &h).loops.len(), 1);
}

#[test]
fn a_non_associative_hatch_takes_loops_unlinked() {
    let (mut s, _h, a, b, _i) = session();
    let n = handle(&s.execute("hatch", &json!({ "handles": [a], "associative": false })).unwrap());
    s.execute("hatchedit", &json!({ "handles": [n], "addHandles": [b] })).unwrap();
    let x = hatch(&s, &n);
    assert!(!x.associative);
    assert!(x.loops.iter().all(|l| l.sources.is_empty()));
}

#[test]
fn typed_hatchedit_adds_and_removes_boundaries() {
    let (mut s, h, _a, _b, _i) = session();
    s.set_selection(vec![Handle::parse_hex(&h).unwrap()]);
    s.cmdline("-hatchedit").unwrap();
    s.input(cadaddy_engine::Input::Keyword("ADd boundaries".into())).unwrap();
    s.input(cadaddy_engine::Input::Keyword("Internal point".into())).unwrap();
    s.input(cadaddy_engine::Input::Point(Vec2::new(25.0, 5.0))).unwrap();
    s.input(cadaddy_engine::Input::Enter).unwrap();
    assert!(!s.prompting());
    assert_eq!(hatch(&s, &h).loops.len(), 2);
    s.set_selection(vec![Handle::parse_hex(&h).unwrap()]);
    s.cmdline("-hatchedit").unwrap();
    s.input(cadaddy_engine::Input::Keyword("Remove boundaries".into())).unwrap();
    // On the second square's left edge (a pick box at this zoom).
    s.input(cadaddy_engine::Input::Point(Vec2::new(20.0, 5.0))).unwrap();
    s.input(cadaddy_engine::Input::Enter).unwrap();
    assert!(!s.prompting());
    assert_eq!(hatch(&s, &h).loops.len(), 1);
}

#[test]
fn hatch_inherits_another_hatchs_properties() {
    let (mut s, h, _a, b, _i) = session();
    s.execute(
        "hatchedit",
        &json!({ "handles": [h], "pattern": "ANSI37", "scale": 2.5, "angle": 30, "style": "outer", "origin": [1, 2], "originMode": "current" }),
    )
    .unwrap();
    let hh = Handle::parse_hex(&h).unwrap();
    s.doc_mut()
        .unwrap()
        .modify_entity(hh, |e| {
            e.common.color = Color::Index(3);
            e.common.transparency = Transparency::Percent(40);
        })
        .unwrap();
    let n = handle(&s.execute("hatch", &json!({ "handles": [b], "inherit": h })).unwrap());
    let x = hatch(&s, &n);
    assert_eq!((x.pattern.as_str(), x.scale, x.style), ("ANSI37", 2.5, 1));
    assert!((x.angle - 30f64.to_radians()).abs() < 1e-12);
    assert!(x.origin.near(Vec2::new(1.0, 2.0), 1e-12));
    let e = s.doc().unwrap().entity(Handle::parse_hex(&n).unwrap()).unwrap().clone();
    assert_eq!(e.common.color, Color::Index(3));
    assert_eq!(e.common.transparency, Transparency::Percent(40));
    // Explicit values win over the inherited ones.
    let m = handle(&s.execute("hatch", &json!({ "handles": [b], "inherit": h, "scale": 7 })).unwrap());
    assert_eq!(hatch(&s, &m).scale, 7.0);
    assert_eq!(hatch(&s, &m).pattern, "ANSI37");
    // A gradient is inherited as a gradient.
    s.execute("hatchedit", &json!({ "handles": [h], "gradient": { "name": "CYLINDER", "color1": "1", "color2": "2" } })).unwrap();
    let g = handle(&s.execute("hatch", &json!({ "handles": [b], "inherit": h })).unwrap());
    assert_eq!(hatch(&s, &g).gradient.map(|g| g.name), Some("CYLINDER".to_string()));
    // Not a hatch, or no such object: refused.
    assert!(s.execute("hatch", &json!({ "handles": [b], "inherit": b })).is_err());
    assert!(s.execute("hatch", &json!({ "handles": [b], "inherit": "FFFFF" })).is_err());
    assert!(s.execute("hatch", &json!({ "handles": [b], "inherit": 12 })).is_err());
}

#[test]
fn typed_hatch_inherit_option() {
    let (mut s, h, _a, _b, _i) = session();
    s.execute("hatchedit", &json!({ "handles": [h], "pattern": "ANSI37", "scale": 3 })).unwrap();
    let before: Vec<Handle> = s.doc().unwrap().model.iter().map(|e| e.handle).collect();
    s.cmdline("-hatch").unwrap();
    s.input(cadaddy_engine::Input::Keyword("Inherit".into())).unwrap();
    s.input(cadaddy_engine::Input::Pick(vec![Handle::parse_hex(&h).unwrap()])).unwrap();
    s.input(cadaddy_engine::Input::Point(Vec2::new(25.0, 5.0))).unwrap();
    s.input(cadaddy_engine::Input::Enter).unwrap();
    assert!(!s.prompting());
    let new = s.doc().unwrap().model.iter().find(|e| !before.contains(&e.handle)).map(|e| e.kind.clone());
    match new {
        Some(EntityKind::Hatch(x)) => assert_eq!((x.pattern.as_str(), x.scale), ("ANSI37", 3.0)),
        k => panic!("no new hatch: {k:?}"),
    }
}
