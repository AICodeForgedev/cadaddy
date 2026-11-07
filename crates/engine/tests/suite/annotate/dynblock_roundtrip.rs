//! Authored value sets (increment, minimum, maximum), visibility states and lookup tables after
//! a DXF save and reload, each checked by the full geometry of the reference (every line's
//! endpoints), not one extreme edge.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, EntityKind, Handle};
use serde_json::{Value, json};

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> String {
    s.execute("line", &json!({ "points": [a, b] })).unwrap()["handles"][0].as_str().unwrap().to_string()
}

/// Block `name` of lines (`[ax, ay, bx, by]`), base point at the origin.
fn block_of(s: &mut Session, name: &str, lines: &[[f64; 4]]) -> Vec<String> {
    let hs: Vec<String> = lines.iter().map(|l| line(s, [l[0], l[1]], [l[2], l[3]])).collect();
    s.execute("block", &json!({ "name": name, "base": [0, 0], "handles": hs })).unwrap();
    s.doc().unwrap().block(name).unwrap().entities.handles().iter().map(|h| h.hex()).collect()
}

fn insert(s: &mut Session, name: &str, at: [f64; 2]) -> String {
    s.execute("insert", &json!({ "name": name, "at": at })).unwrap();
    s.doc().unwrap().model.handles().last().unwrap().hex()
}

/// The lines of reference `h` that show, as `[ax, ay, bx, by]` in block coordinates.
fn shown(d: &Drawing, h: &str) -> Vec<[f64; 4]> {
    let Some(EntityKind::Insert(i)) = d.entity(Handle::parse_hex(h).unwrap()).map(|e| &e.kind) else { panic!("not a reference") };
    d.block(&i.block)
        .unwrap()
        .entities
        .iter()
        .filter(|e| e.common.visible)
        .filter_map(|e| match &e.kind {
            EntityKind::Line(l) => Some([l.a.x, l.a.y, l.b.x, l.b.y]),
            _ => None,
        })
        .collect()
}

fn assert_lines(got: &[[f64; 4]], want: &[[f64; 4]]) {
    assert_eq!(got.len(), want.len(), "{got:?} != {want:?}");
    for (g, w) in got.iter().zip(want) {
        assert!(g.iter().zip(w).all(|(a, b)| (a - b).abs() < 1e-9), "{got:?} != {want:?}");
    }
}

fn set(s: &mut Session, h: &str, name: &str, value: &str) {
    s.execute("blocks.reference.set", &json!({ "handle": h, "name": name, "value": value })).unwrap();
}

fn property(s: &mut Session, h: &str, name: &str) -> Value {
    let r = s.execute("blocks.reference", &json!({ "handle": h })).unwrap();
    r["properties"].as_array().unwrap().iter().find(|p| p["name"] == name).map(|p| p["value"].clone()).unwrap_or(Value::Null)
}

/// The session after saving the drawing as DXF and reading it back.
fn reloaded(s: &Session) -> Session {
    let text = cadaddy_io::write_dxf(s.doc().unwrap());
    let mut s2 = Session::new();
    s2.open_drawing(cadaddy_io::read(text.as_bytes(), "t.dxf").unwrap(), "t", None);
    s2
}

/// The four sides of a box 0..w wide and 5 high, in the order the block draws them.
fn sides(w: f64) -> [[f64; 4]; 4] {
    [[0.0, 0.0, w, 0.0], [w, 0.0, w, 5.0], [w, 5.0, 0.0, 5.0], [0.0, 5.0, 0.0, 0.0]]
}

/// A box 10 wide with a stretchable width: from 4 to 20 in steps of 3 (4, 7, 10, 13, 16, 19).
fn stepped() -> Session {
    let mut s = Session::new();
    let hs = block_of(&mut s, "Box", &sides(10.0));
    s.execute(
        "bparameter",
        &json!({ "block": "Box", "type": "linear", "base": [0, 0], "end": [10, 0], "label": "Width", "values": { "minimum": 4, "maximum": 20, "increment": 3 } }),
    )
    .unwrap();
    s.execute(
        "bactiontool",
        &json!({ "block": "Box", "type": "stretch", "parameter": "Width", "handles": hs, "frame": [[8, -1], [12, -1], [12, 6], [8, 6]] }),
    )
    .unwrap();
    s
}

#[test]
fn increment_minimum_and_maximum_hold_after_a_reload() {
    let mut original = stepped();
    let mut reload = reloaded(&original);
    for (label, s) in [("authored", &mut original), ("reloaded", &mut reload)] {
        let m = s.execute("blocks.dynamic", &json!({ "name": "Box" })).unwrap();
        let v = &m["parameters"][0]["kind"]["values"];
        assert_eq!(
            (v["flags"].as_i64(), v["minimum"].as_f64(), v["maximum"].as_f64(), v["increment"].as_f64()),
            (Some(7), Some(4.0), Some(20.0), Some(3.0)),
            "{label}: {v}"
        );
        // 11 is between 10 and 13: the nearest step from the minimum is 10.
        let h = insert(s, "Box", [0.0, 0.0]);
        set(s, &h, "Width", "11");
        assert_lines(&shown(s.doc().unwrap(), &h), &sides(10.0));
        assert_eq!(property(s, &h, "Width"), json!("10.0000"), "{label}");
        // 12 is nearer 13.
        set(s, &h, "Width", "12");
        assert_lines(&shown(s.doc().unwrap(), &h), &sides(13.0));
        // Past the maximum: 20 (the step grid 4 + 3n has 19 and 22; the maximum cuts it to 20).
        set(s, &h, "Width", "100");
        assert_lines(&shown(s.doc().unwrap(), &h), &sides(20.0));
        // Under the minimum: 4.
        set(s, &h, "Width", "1");
        assert_lines(&shown(s.doc().unwrap(), &h), &sides(4.0));
        assert_eq!(property(s, &h, "Width"), json!("4.0000"), "{label}");
        // Grip drags use the same set: the end grip to 15.5 is 16.
        let node = m["parameters"][0]["node"].clone();
        s.execute("blocks.reference.grip", &json!({ "handle": h, "node": node, "part": "end", "to": [15.5, 0] })).unwrap();
        assert_lines(&shown(s.doc().unwrap(), &h), &sides(16.0));
    }
}

#[test]
fn a_reference_stretched_before_the_save_is_still_stretched_after_it() {
    let mut s = stepped();
    let h = insert(&mut s, "Box", [50.0, 0.0]);
    set(&mut s, &h, "Width", "19");
    let mut s2 = reloaded(&s);
    assert_lines(&shown(s2.doc().unwrap(), &h), &sides(19.0));
    assert_eq!(property(&mut s2, &h, "Width"), json!("19.0000"));
    // And changes again from there, as the original does.
    set(&mut s2, &h, "Width", "7");
    assert_lines(&shown(s2.doc().unwrap(), &h), &sides(7.0));
}

/// Two lines that show in one state each, plus one in both.
fn signed() -> Session {
    let mut s = Session::new();
    let hs = block_of(&mut s, "Sign", &[[0.0, 0.0, 4.0, 0.0], [0.0, 1.0, 8.0, 1.0], [0.0, 2.0, 2.0, 2.0]]);
    s.execute("bparameter", &json!({ "block": "Sign", "type": "visibility", "at": [0, -1], "label": "Size" })).unwrap();
    s.execute("bvstate", &json!({ "block": "Sign", "action": "rename", "name": "VisibilityState0", "newName": "Small" })).unwrap();
    s.execute("bvstate", &json!({ "block": "Sign", "action": "set", "name": "Small", "handles": [hs[0], hs[2]] })).unwrap();
    s.execute("bvstate", &json!({ "block": "Sign", "action": "new", "name": "Large", "handles": [hs[1], hs[2]] })).unwrap();
    s
}

#[test]
fn visibility_states_show_the_same_objects_after_a_reload() {
    let mut original = signed();
    let mut reload = reloaded(&original);
    for (label, s) in [("authored", &mut original), ("reloaded", &mut reload)] {
        let h = insert(s, "Sign", [0.0, 0.0]);
        assert_eq!(property(s, &h, "Size"), json!("Small"), "{label}");
        set(s, &h, "Size", "Large");
        assert_lines(&shown(s.doc().unwrap(), &h), &[[0.0, 1.0, 8.0, 1.0], [0.0, 2.0, 2.0, 2.0]]);
        set(s, &h, "Size", "Small");
        assert_lines(&shown(s.doc().unwrap(), &h), &[[0.0, 0.0, 4.0, 0.0], [0.0, 2.0, 2.0, 2.0]]);
        let m = s.execute("bvstate", &json!({ "block": "Sign" })).unwrap();
        let names: Vec<&str> = m["states"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Small", "Large"], "{label}");
    }
}

/// A box with a width and a lookup of sizes setting it: Small = 10, Large = 30.
fn sized() -> Session {
    let mut s = Session::new();
    let hs = block_of(&mut s, "Case", &sides(10.0));
    s.execute(
        "bparameter",
        &json!({ "block": "Case", "type": "linear", "base": [0, 0], "end": [10, 0], "label": "Width", "values": { "list": [10, 20, 30] } }),
    )
    .unwrap();
    s.execute(
        "bactiontool",
        &json!({ "block": "Case", "type": "stretch", "parameter": "Width", "handles": hs, "frame": [[8, -1], [12, -1], [12, 6], [8, 6]] }),
    )
    .unwrap();
    s.execute("bparameter", &json!({ "block": "Case", "type": "lookup", "at": [5, -2], "label": "Size" })).unwrap();
    s.execute(
        "bactiontool",
        &json!({ "block": "Case", "type": "lookup", "parameter": "Size", "columns": [{ "parameter": "Width" }, { "parameter": "Size", "lookup": true }], "rows": [["10", "Small"], ["30", "Large"]] }),
    )
    .unwrap();
    s
}

#[test]
fn a_lookup_table_sets_the_same_geometry_after_a_reload() {
    let mut original = sized();
    let mut reload = reloaded(&original);
    for (label, s) in [("authored", &mut original), ("reloaded", &mut reload)] {
        let h = insert(s, "Case", [0.0, 0.0]);
        assert_eq!(property(s, &h, "Size"), json!("Small"), "{label}");
        set(s, &h, "Size", "Large");
        assert_lines(&shown(s.doc().unwrap(), &h), &sides(30.0));
        assert_eq!(property(s, &h, "Width"), json!("30.0000"), "{label}");
        // The other way: a width from the list picks its row.
        set(s, &h, "Width", "10");
        assert_lines(&shown(s.doc().unwrap(), &h), &sides(10.0));
        assert_eq!(property(s, &h, "Size"), json!("Small"), "{label}");
        // A width no row has is "Custom".
        set(s, &h, "Width", "20");
        assert_lines(&shown(s.doc().unwrap(), &h), &sides(20.0));
        assert_eq!(property(s, &h, "Size"), json!("Custom"), "{label}");
    }
}
