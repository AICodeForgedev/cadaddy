//! BLOCKIFY: copies of a group found moved, turned and uniformly scaled; near misses left alone;
//! the group and its copies replaced with references to one block.

use cadaddy_doc::{EntityKind, Handle};
use cadaddy_engine::Session;
use cadaddy_geom::{Similarity, Vec2};
use serde_json::{Value, json};

fn handle(r: &Value) -> String {
    r["handle"].as_str().or_else(|| r["handles"][0].as_str()).unwrap().to_string()
}

/// A symbol: a 4 × 2 rectangle, a circle of radius 0.5 at (1, 1) and a line from (2.5, 0.5) to
/// (3.5, 1.5), carried by `s`; its objects' handles. `circle_dx` nudges the circle (a near miss),
/// `skip_line` leaves the line out.
fn symbol(sess: &mut Session, s: Similarity, circle_dx: f64, skip_line: bool) -> Vec<String> {
    let p = |x: f64, y: f64| {
        let q = s.apply(Vec2::new(x, y));
        json!([q.x, q.y])
    };
    let mut hs =
        vec![handle(&sess.execute("pline", &json!({"points": [p(0.0, 0.0), p(4.0, 0.0), p(4.0, 2.0), p(0.0, 2.0)], "closed": true})).unwrap())];
    hs.push(handle(&sess.execute("circle", &json!({"center": p(1.0 + circle_dx, 1.0), "radius": 0.5 * s.scale})).unwrap()));
    if !skip_line {
        hs.push(handle(&sess.execute("line", &json!({"points": [p(2.5, 0.5), p(3.5, 1.5)]})).unwrap()));
    }
    hs
}

fn sim(scale: f64, deg: f64, x: f64, y: f64) -> Similarity {
    Similarity { scale, angle: deg.to_radians(), offset: Vec2::new(x, y) }
}

fn inserts(s: &Session) -> Vec<cadaddy_doc::Insert> {
    s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Insert(i) = &e.kind { Some(i.clone()) } else { None }).collect()
}

#[test]
fn moved_turned_and_scaled_copies_are_found_and_near_misses_are_not() {
    let mut s = Session::new();
    let template = symbol(&mut s, Similarity::IDENTITY, 0.0, false);
    symbol(&mut s, sim(1.0, 0.0, 20.0, 0.0), 0.0, false);
    symbol(&mut s, sim(1.0, 37.0, 0.0, 20.0), 0.0, false);
    symbol(&mut s, sim(2.5, 200.0, 40.0, 40.0), 0.0, false);
    let near_circle = symbol(&mut s, sim(1.0, 0.0, 60.0, 0.0), 0.2, false);
    let no_line = symbol(&mut s, sim(1.0, 90.0, 80.0, 0.0), 0.0, true);
    let r = s.execute("blockify", &json!({"handles": template, "replace": false})).unwrap();
    let m = r["matches"].as_array().unwrap();
    assert_eq!(m.len(), 3, "{r}");
    let mut found: Vec<(f64, f64)> = m.iter().map(|x| (x["rotation"].as_f64().unwrap(), x["scale"].as_f64().unwrap())).collect();
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    let want = [(0.0, 1.0), (37.0, 1.0), (200.0, 2.5)];
    for (f, w) in found.iter().zip(want) {
        assert!((f.0 - w.0).abs() < 1e-6 && (f.1 - w.1).abs() < 1e-9, "{found:?}");
    }
    let matched: Vec<&str> = m.iter().flat_map(|x| x["handles"].as_array().unwrap().iter().filter_map(Value::as_str)).collect();
    assert!(near_circle.iter().chain(&no_line).all(|h| !matched.contains(&h.as_str())), "near misses stay out");
    // Replace: the group and its three copies become four references.
    let before = s.doc().unwrap().model.len();
    let r = s.execute("blockify", &json!({"handles": template, "name": "SYM"})).unwrap();
    assert_eq!((r["block"].clone(), r["replaced"].clone()), (json!("SYM"), json!(3)));
    let ins = inserts(&s);
    assert_eq!(ins.len(), 4);
    assert!(ins.iter().all(|i| i.block == "SYM"));
    assert_eq!(s.doc().unwrap().model.len(), before - 12 + 4, "the 12 objects became 4 references; near misses remain");
    assert_eq!(s.doc().unwrap().block("SYM").unwrap().entities.len(), 3);
    // Each reference draws where its copy was: the scaled one at 2.5, turned 200°.
    let big = ins.iter().find(|i| (i.scale.x - 2.5).abs() < 1e-9).unwrap();
    assert!((big.rotation.to_degrees().rem_euclid(360.0) - 200.0).abs() < 1e-6);
    assert!(big.insert.xy().dist(Vec2::new(40.0, 40.0)) < 1e-9, "the block's base is the group's lower left");
    // One undo takes it all back.
    s.undo().unwrap();
    assert_eq!(s.doc().unwrap().model.len(), before);
}

#[test]
fn an_existing_block_finds_its_exploded_copies() {
    let mut s = Session::new();
    let t = symbol(&mut s, Similarity::IDENTITY, 0.0, false);
    s.execute("block", &json!({"name": "B", "base": [0, 0], "handles": t, "keep": "delete"})).unwrap();
    symbol(&mut s, sim(0.5, -90.0, 10.0, 10.0), 0.0, false);
    let r = s.execute("blockify", &json!({"block": "B"})).unwrap();
    assert_eq!(r["replaced"], 1);
    let ins = inserts(&s);
    assert_eq!(ins.len(), 1);
    assert!((ins[0].scale.x - 0.5).abs() < 1e-9 && ins[0].insert.xy().dist(Vec2::new(10.0, 10.0)) < 1e-9);
    // `within` limits where copies are looked for.
    symbol(&mut s, sim(1.0, 0.0, 30.0, 0.0), 0.0, false);
    let r = s.execute("blockify", &json!({"block": "B", "within": [], "replace": false})).unwrap();
    assert_eq!(r["matches"].as_array().unwrap().len(), 0);
}

#[test]
fn circles_and_odd_groups() {
    let mut s = Session::new();
    // Two circles: their spacing and sizes fix the copy's turn and scale.
    let a = handle(&s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap());
    let b = handle(&s.execute("circle", &json!({"center": [5, 0], "radius": 2})).unwrap());
    s.execute("circle", &json!({"center": [10, 10], "radius": 2})).unwrap();
    s.execute("circle", &json!({"center": [10, 20], "radius": 4})).unwrap();
    s.execute("circle", &json!({"center": [30, 0], "radius": 2})).unwrap();
    s.execute("circle", &json!({"center": [35, 0], "radius": 1})).unwrap();
    // Not a copy: the spacing is right but the sizes are swapped the wrong way for it.
    s.execute("circle", &json!({"center": [60, 0], "radius": 1})).unwrap();
    s.execute("circle", &json!({"center": [65, 0], "radius": 1})).unwrap();
    let r = s.execute("blockify", &json!({"handles": [a, b], "replace": false})).unwrap();
    let m = r["matches"].as_array().unwrap();
    // Up and twice the size; and the last pair, the group turned half round.
    assert_eq!(m.len(), 2, "{r}");
    let has = |scale: f64, rot: f64| {
        m.iter().any(|x| (x["scale"].as_f64().unwrap() - scale).abs() < 1e-9 && (x["rotation"].as_f64().unwrap() - rot).abs() < 1e-6)
    };
    assert!(has(2.0, 90.0) && has(1.0, 180.0), "{r}");
    // Unsupported objects are refused, and an empty selection.
    let h = s.execute("hatch", &json!({"points": [[0, 0], [1, 0], [1, 1]]}));
    if let Ok(h) = h {
        assert!(s.execute("blockify", &json!({"handles": [handle(&h)]})).is_err());
    }
    assert!(s.execute("blockify", &json!({"handles": []})).is_err());
    assert!(s.execute("blockify", &json!({"handles": ["ZZZ"]})).is_err());
    assert!(s.execute("blockify", &json!({"block": "nope"})).is_err());
}

#[test]
fn typed_blockify_previews_then_replaces() {
    let mut s = Session::new();
    let t = symbol(&mut s, Similarity::IDENTITY, 0.0, false);
    symbol(&mut s, sim(1.0, 45.0, 20.0, 0.0), 0.0, false);
    let hs: Vec<Handle> = t.iter().map(|h| Handle::parse_hex(h).unwrap()).collect();
    s.set_selection(hs);
    s.cmdline("blockify").unwrap();
    assert_eq!(s.selection().len(), 3, "the copy is selected to show it");
    assert!(s.current_prompt().unwrap().message.starts_with("1 copies found"), "{}", s.current_prompt().unwrap().message);
    s.cmdline("").unwrap();
    s.cmdline("MARK").unwrap();
    assert!(s.running.is_none());
    assert_eq!(inserts(&s).len(), 2);
    assert!(s.doc().unwrap().block("MARK").is_some());
}
