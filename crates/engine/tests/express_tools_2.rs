//! Express Tools II: BREAKLINE (a break line with a zigzag symbol), BLOCKREPLACE (every
//! reference to one block becomes another, keeping placement, properties and attribute values
//! by tag) and MOCORO (move, copy, rotate and scale in one command).

use cadaddy_engine::Session;
use cadaddy_engine::doc::geom::Vec2;
use cadaddy_engine::doc::{EntityKind, Handle, Insert};
use serde_json::{Value, json};

fn session() -> Session {
    let mut s = Session::new();
    s.new_drawing(false);
    s.viewport_px = (1600.0, 900.0);
    s
}

fn h(v: &Value) -> Handle {
    Handle::parse_hex(v.as_str().unwrap()).unwrap()
}

fn kind(s: &Session, h: Handle) -> EntityKind {
    s.doc().unwrap().entity(h).unwrap().kind.clone()
}

fn close(a: Vec2, b: Vec2) -> bool {
    a.near(b, 1e-9)
}

// ---------- BREAKLINE ----------

fn polyline(s: &Session, h: Handle) -> Vec<Vec2> {
    match kind(s, h) {
        EntityKind::LwPolyline(p) => {
            assert!(!p.closed);
            p.vertices.iter().map(|v| v.p).collect()
        }
        k => panic!("not a polyline: {k:?}"),
    }
}

#[test]
fn breakline_draws_a_zigzag_polyline_on_the_current_layer() {
    let mut s = session();
    s.execute("layer", &json!({"option": "make", "name": "BREAKS"})).ok();
    let r = s.execute("breakline", &json!({"p1": [0, 0], "p2": [10, 0], "size": 2, "extension": 1})).unwrap();
    let pl = h(&r["handle"]);
    let pts = polyline(&s, pl);
    // Extended 1 past both ends, the symbol 2 long centred on the midpoint.
    assert_eq!(pts.len(), 6);
    assert!(close(pts[0], Vec2::new(-1.0, 0.0)) && close(pts[5], Vec2::new(11.0, 0.0)));
    assert!(close(pts[1], Vec2::new(4.0, 0.0)) && close(pts[4], Vec2::new(6.0, 0.0)));
    // The zigzag goes one unit to each side.
    assert!((pts[2].y - 1.0).abs() < 1e-9 && (pts[3].y + 1.0).abs() < 1e-9);
    let layer = s.doc().unwrap().entity(pl).unwrap().common.layer.clone();
    assert_eq!(layer, s.doc().unwrap().header.str("CLAYER", "0"));
    // The symbol goes where it's asked, projected onto the line.
    let r = s.execute("breakline", &json!({"p1": [0, 0], "p2": [0, 10], "at": [3, 2], "size": 1, "extension": 0})).unwrap();
    let pts = polyline(&s, h(&r["handle"]));
    assert!(close(pts[1], Vec2::new(0.0, 1.5)) && close(pts[4], Vec2::new(0.0, 2.5)));
    // Imperial defaults: size 0.5, extension 0.125.
    let r = s.execute("breakline", &json!({"p1": [0, 0], "p2": [4, 0]})).unwrap();
    let pts = polyline(&s, h(&r["handle"]));
    assert!(close(pts[0], Vec2::new(-0.125, 0.0)) && close(pts[1], Vec2::new(1.75, 0.0)));
}

#[test]
fn breakline_typed_with_options() {
    let mut s = session();
    let n0 = s.doc().unwrap().model.len();
    s.cmdline("breakline").unwrap();
    s.cmdline("s").unwrap();
    s.cmdline("3").unwrap();
    s.cmdline("e").unwrap();
    s.cmdline("0.5").unwrap();
    s.cmdline("0,0").unwrap();
    s.cmdline("20,0").unwrap();
    // Enter: the midpoint.
    s.cmdline("").unwrap();
    assert!(!s.prompting());
    let d = s.doc().unwrap();
    assert_eq!(d.model.len(), n0 + 1);
    let last = d.model.iter().last().unwrap().handle;
    let pts = polyline(&s, last);
    assert!(close(pts[0], Vec2::new(-0.5, 0.0)) && close(pts[1], Vec2::new(8.5, 0.0)) && close(pts[4], Vec2::new(11.5, 0.0)));
}

#[test]
fn breakline_refuses_bad_input() {
    let mut s = session();
    for p in [
        json!({"p1": [0, 0], "p2": [0, 0]}),
        json!({"p1": [0, 0]}),
        json!({"p1": [0, 0], "p2": [1, 0], "size": 0}),
        json!({"p1": [0, 0], "p2": [1, 0], "size": -1}),
        json!({"p1": [0, 0], "p2": [1, 0], "size": 1e300}),
        json!({"p1": [0, 0], "p2": [1, 0], "extension": "x"}),
        json!({"p1": [0, 0], "p2": [1e308, 1e308]}),
    ] {
        let n = s.doc().unwrap().model.len();
        let _ = s.execute("breakline", &p);
        let added = s.doc().unwrap().model.len() - n;
        if added == 1 {
            // Only a finite result is ever added.
            let last = s.doc().unwrap().model.iter().last().unwrap().handle;
            assert!(polyline(&s, last).iter().all(|q| q.is_finite()), "{p}");
        }
    }
}

// ---------- BLOCKREPLACE ----------

/// Blocks OLD (a circle, attribute TAG and NOTE) and NEW (a square, attributes TAG and SIZE),
/// two references to OLD (one on layer A, scaled and turned, with values) and a block HOLDER
/// containing a reference to OLD.
fn blocks() -> (Session, Vec<Handle>) {
    let mut s = session();
    s.execute("layer", &json!({"option": "make", "name": "0"})).ok();
    let c = s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    let a1 = s.execute("attdef", &json!({"tag": "TAG", "default": "T0", "at": [0, -2]})).unwrap();
    let a2 = s.execute("attdef", &json!({"tag": "NOTE", "default": "N0", "at": [0, -3]})).unwrap();
    let hs: Vec<Value> = [&c, &a1, &a2].iter().map(|r| r["handle"].clone()).collect();
    s.execute("block", &json!({"name": "OLD", "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    let sq = s.execute("rectang", &json!({"p1": [-1, -1], "p2": [1, 1]})).unwrap();
    let b1 = s.execute("attdef", &json!({"tag": "TAG", "default": "X", "at": [2, 0]})).unwrap();
    let b2 = s.execute("attdef", &json!({"tag": "SIZE", "default": "S0", "at": [2, -1]})).unwrap();
    let hs: Vec<Value> = [&sq, &b1, &b2].iter().map(|r| r.get("handle").cloned().unwrap_or_else(|| r["handles"][0].clone())).collect();
    s.execute("block", &json!({"name": "NEW", "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    let i1 = s.execute("insert", &json!({"name": "OLD", "at": [10, 10], "attribs": {"TAG": "A-1", "NOTE": "hello"}})).unwrap();
    s.execute("layer", &json!({"option": "make", "name": "A"})).ok();
    let i2 = s.execute("insert", &json!({"name": "OLD", "at": [20, 0], "scale": 2, "rotation": 90, "attribs": {"TAG": "B-2"}})).unwrap();
    let refs = vec![h(&i1["handle"]), h(&i2["handle"])];
    // A block containing a reference to OLD.
    let inner = s.execute("insert", &json!({"name": "OLD", "at": [50, 50]})).unwrap();
    s.execute("block", &json!({"name": "HOLDER", "base": [50, 50], "handles": [inner["handle"]], "keep": "delete"})).unwrap();
    s.execute("insert", &json!({"name": "HOLDER", "at": [100, 0]})).unwrap();
    (s, refs)
}

fn insert(s: &Session, h: Handle) -> Insert {
    match kind(s, h) {
        EntityKind::Insert(i) => i,
        k => panic!("not a block reference: {k:?}"),
    }
}

#[test]
fn blockreplace_keeps_placement_properties_and_attribute_values() {
    let (mut s, refs) = blocks();
    let before: Vec<(Insert, String)> = refs.iter().map(|r| (insert(&s, *r), s.doc().unwrap().entity(*r).unwrap().common.layer.clone())).collect();
    let r = s.execute("blockreplace", &json!({"from": "OLD", "to": "NEW"})).unwrap();
    // Both references and the one inside HOLDER.
    assert_eq!(r["replaced"], 3);
    for (h, (old, layer)) in refs.iter().zip(before) {
        let i = insert(&s, *h);
        assert_eq!(i.block, "NEW");
        assert_eq!((i.insert, i.scale, i.rotation), (old.insert, old.scale, old.rotation));
        assert_eq!(s.doc().unwrap().entity(*h).unwrap().common.layer, layer);
        // NEW's attributes: TAG keeps its value, SIZE takes its default; NOTE (not in NEW) goes.
        let tags: Vec<(&str, &str)> = i.attribs.iter().map(|a| (a.tag.as_str(), a.text.value.as_str())).collect();
        let old_tag = old.attribs.iter().find(|a| a.tag == "TAG").unwrap().text.value.clone();
        assert_eq!(tags, vec![("TAG", old_tag.as_str()), ("SIZE", "S0")]);
    }
    // The attribute sits where NEW defines it, through the reference's transform.
    let i = insert(&s, refs[0]);
    let tag = i.attribs.iter().find(|a| a.tag == "TAG").unwrap();
    assert!(tag.text.insert.xy().near(Vec2::new(12.0, 10.0), 1e-9), "{:?}", tag.text.insert);
    let holder = s.doc().unwrap().block("HOLDER").unwrap().clone();
    assert!(holder.entities.iter().all(|e| !matches!(&e.kind, EntityKind::Insert(i) if i.block == "OLD")));
    // OLD was purged (nothing refers to it any more).
    assert_eq!(r["purged"], true);
    assert!(s.doc().unwrap().block("OLD").is_none());
    // One undo step puts it all back.
    s.undo().unwrap();
    assert_eq!(insert(&s, refs[0]).block, "OLD");
    assert!(s.doc().unwrap().block("OLD").is_some());
}

#[test]
fn blockreplace_without_purge_and_refusals() {
    let (mut s, refs) = blocks();
    let r = s.execute("blockreplace", &json!({"from": "old", "to": "new", "purge": false})).unwrap();
    assert_eq!(r["purged"], false);
    assert!(s.doc().unwrap().block("OLD").is_some());
    assert_eq!(insert(&s, refs[1]).block, "NEW");
    // Same block, missing blocks, and a replacement that contains the block it replaces.
    assert!(s.execute("blockreplace", &json!({"from": "NEW", "to": "NEW"})).is_err());
    assert!(s.execute("blockreplace", &json!({"from": "NOPE", "to": "NEW"})).is_err());
    assert!(s.execute("blockreplace", &json!({"from": "NEW", "to": "NOPE"})).is_err());
    assert!(s.execute("blockreplace", &json!({"from": "NEW", "to": "HOLDER"})).is_err());
    assert!(s.execute("blockreplace", &json!({})).is_err());
}

#[test]
fn blockreplace_typed_by_name_and_by_pick() {
    let (mut s, refs) = blocks();
    s.cmdline("blockreplace").unwrap();
    // Pick a reference of OLD (its circle passes through 11,10).
    s.input(cadaddy_engine::Input::Point(Vec2::new(11.0, 10.0))).unwrap();
    s.cmdline("NEW").unwrap();
    s.cmdline("n").unwrap();
    assert!(!s.prompting());
    assert_eq!(insert(&s, refs[0]).block, "NEW");
    assert!(s.doc().unwrap().block("OLD").is_some());
}

// ---------- MOCORO ----------

fn line_ends(s: &Session, h: Handle) -> (Vec2, Vec2) {
    match kind(s, h) {
        EntityKind::Line(l) => (l.a.xy(), l.b.xy()),
        k => panic!("not a line: {k:?}"),
    }
}

#[test]
fn mocoro_moves_copies_rotates_and_scales_in_one_step() {
    let mut s = session();
    let l = h(&s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap()["handles"][0]);
    let steps = json!([{"move": [5, 5]}, {"copy": [5, 20]}, {"rotate": 90}, {"scale": 2}]);
    let r = s.execute("mocoro", &json!({"handles": [l.hex()], "base": [0, 0], "steps": steps})).unwrap();
    // Moved by 5,5 (base now 5,5), a copy left at base 5,20, turned 90° about 5,5, doubled.
    let (a, b) = line_ends(&s, l);
    assert!(close(a, Vec2::new(5.0, 5.0)) && close(b, Vec2::new(5.0, 25.0)), "{a:?} {b:?}");
    let hs = r["handles"].as_array().unwrap();
    assert_eq!(hs.len(), 2);
    let (ca, cb) = line_ends(&s, h(&hs[1]));
    assert!(close(ca, Vec2::new(5.0, 20.0)) && close(cb, Vec2::new(15.0, 20.0)));
    assert_eq!(r["base"], json!([5.0, 5.0]));
    s.undo().unwrap();
    assert_eq!(line_ends(&s, l), (Vec2::ZERO, Vec2::new(10.0, 0.0)));
    assert!(s.doc().unwrap().entity(h(&hs[1])).is_none());
}

#[test]
fn mocoro_typed_with_modes_base_and_undo() {
    let mut s = session();
    let l = h(&s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap()["handles"][0]);
    s.set_selection(vec![l]);
    s.cmdline("mocoro").unwrap();
    s.cmdline("0,0").unwrap();
    // Move mode: a point moves (the base follows).
    s.cmdline("0,10").unwrap();
    assert_eq!(line_ends(&s, l), (Vec2::new(0.0, 10.0), Vec2::new(10.0, 10.0)));
    // Rotate by a typed angle about the moved base.
    s.cmdline("r").unwrap();
    s.cmdline("90").unwrap();
    let (a, b) = line_ends(&s, l);
    assert!(close(a, Vec2::new(0.0, 10.0)) && close(b, Vec2::new(0.0, 20.0)), "{a:?} {b:?}");
    // Undo the rotation, then a new base and a scale by 3 about it.
    s.cmdline("u").unwrap();
    assert_eq!(line_ends(&s, l), (Vec2::new(0.0, 10.0), Vec2::new(10.0, 10.0)));
    s.cmdline("b").unwrap();
    s.cmdline("10,10").unwrap();
    s.cmdline("s").unwrap();
    s.cmdline("3").unwrap();
    let (a, b) = line_ends(&s, l);
    assert!(close(a, Vec2::new(-20.0, 10.0)) && close(b, Vec2::new(10.0, 10.0)), "{a:?} {b:?}");
    // Copy mode leaves copies; eXit ends.
    let n = s.doc().unwrap().model.len();
    s.cmdline("c").unwrap();
    s.cmdline("10,0").unwrap();
    s.cmdline("x").unwrap();
    assert!(!s.prompting());
    assert_eq!(s.doc().unwrap().model.len(), n + 1);
    // The whole command is one undo step.
    s.undo().unwrap();
    assert_eq!(line_ends(&s, l), (Vec2::ZERO, Vec2::new(10.0, 0.0)));
    assert_eq!(s.doc().unwrap().model.len(), n);
}

#[test]
fn mocoro_refuses_bad_steps() {
    let mut s = session();
    let l = s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap()["handles"][0].clone();
    for steps in
        [json!([{"scale": 0}]), json!([{"scale": -2}]), json!([{"rotate": "x"}]), json!([{"jump": [1, 1]}]), json!("move"), json!([{"scale": 1e300}])]
    {
        assert!(s.execute("mocoro", &json!({"handles": [l], "base": [0, 0], "steps": steps})).is_err(), "{steps}");
    }
    assert!(s.execute("mocoro", &json!({"handles": [l], "steps": []})).is_err());
    assert!(s.execute("mocoro", &json!({"handles": [], "base": [0, 0], "steps": []})).is_err());
    let many: Vec<Value> = (0..1001).map(|_| json!({"move": [0, 0]})).collect();
    assert!(s.execute("mocoro", &json!({"handles": [l], "base": [0, 0], "steps": many})).is_err());
}

#[test]
fn express_tools_are_on_the_menu() {
    for (id, label) in [("breakline", "Break-line Symbol"), ("blockreplace", "Replace Block..."), ("mocoro", "Move/Copy/Rotate")] {
        let spec = cadaddy_engine::cmd::find_command(id).unwrap();
        assert_eq!(spec.menu, &["Tools", "Express Tools", label][..], "{id}");
    }
}
