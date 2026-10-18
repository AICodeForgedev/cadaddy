//! Making blocks dynamic: BPARAMETER, BACTIONTOOL and BVSTATE author parameters, actions and
//! visibility states; references then behave as authored, and do so again after saving to DXF
//! and reading the file back.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, EntityKind, Handle};
use serde_json::{Value, json};

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Value {
    s.execute("line", &json!({ "points": [a, b] })).unwrap()["handles"][0].clone()
}

/// Block "Box": a 10 × 5 rectangle of lines with its base point at the origin.
fn boxed() -> Session {
    let mut s = Session::new();
    let ls = vec![
        line(&mut s, [0.0, 0.0], [10.0, 0.0]),
        line(&mut s, [10.0, 0.0], [10.0, 5.0]),
        line(&mut s, [10.0, 5.0], [0.0, 5.0]),
        line(&mut s, [0.0, 5.0], [0.0, 0.0]),
    ];
    s.execute("block", &json!({ "name": "Box", "base": [0, 0], "handles": ls })).unwrap();
    s
}

fn block_handles(s: &Session, name: &str) -> Vec<String> {
    s.doc().unwrap().block(name).unwrap().entities.handles().iter().map(|h| h.hex()).collect()
}

fn insert(s: &mut Session, name: &str, at: [f64; 2]) -> String {
    s.execute("insert", &json!({ "name": name, "at": at })).unwrap();
    s.doc().unwrap().model.handles().last().unwrap().hex()
}

/// The largest x of any line end of the block reference `h` shows (block coordinates).
fn right_edge(d: &Drawing, h: &str) -> f64 {
    let Some(EntityKind::Insert(i)) = d.entity(Handle::parse_hex(h).unwrap()).map(|e| &e.kind) else { panic!("not a reference") };
    d.block(&i.block)
        .unwrap()
        .entities
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Line(l) => Some(l.a.x.max(l.b.x)),
            _ => None,
        })
        .fold(f64::MIN, f64::max)
}

fn property(s: &mut Session, h: &str, name: &str) -> Value {
    let r = s.execute("blocks.reference", &json!({ "handle": h })).unwrap();
    r["properties"].as_array().unwrap().iter().find(|p| p["name"] == name).map(|p| p["value"].clone()).unwrap_or(Value::Null)
}

/// A stretchable box: a distance across it with a list of widths, and a stretch of its right side.
fn stretchable() -> Session {
    let mut s = boxed();
    let r = s
        .execute(
            "bparameter",
            &json!({ "block": "Box", "type": "linear", "base": [0, 0], "end": [10, 0], "label": "Width", "values": { "list": [10, 20, 30] } }),
        )
        .unwrap();
    assert_eq!(r["label"], "Width");
    let hs = block_handles(&s, "Box");
    let r = s
        .execute(
            "bactiontool",
            &json!({ "block": "Box", "type": "stretch", "parameter": "Width", "handles": hs, "frame": [[8, -1], [12, -1], [12, 6], [8, 6]] }),
        )
        .unwrap();
    assert_eq!(r["name"], "Stretch1");
    let m = &r["dynamic"];
    assert_eq!(m["parameters"].as_array().unwrap().len(), 1);
    assert_eq!(m["actions"].as_array().unwrap().len(), 1);
    s
}

#[test]
fn an_authored_stretch_follows_the_distance_and_its_value_list() {
    let mut s = stretchable();
    let h = insert(&mut s, "Box", [100.0, 0.0]);
    assert_eq!(property(&mut s, &h, "Width"), json!("10.0000"));
    s.execute("blocks.reference.set", &json!({ "handle": h, "name": "Width", "value": "20" })).unwrap();
    assert!((right_edge(s.doc().unwrap(), &h) - 20.0).abs() < 1e-9);
    // 27 isn't in the list: the nearest width is taken.
    s.execute("blocks.reference.set", &json!({ "handle": h, "name": "Width", "value": "27" })).unwrap();
    assert!((right_edge(s.doc().unwrap(), &h) - 30.0).abs() < 1e-9);
    assert_eq!(property(&mut s, &h, "Width"), json!("30.0000"));
    // Its grips: the distance's two arrows.
    let g = s.execute("blocks.reference.grips", &json!({ "handle": h })).unwrap();
    assert_eq!(g.as_array().unwrap().len(), 2, "{g}");
    // Authoring is one undo step per command.
    s.execute("undo", &json!({})).unwrap();
    s.execute("undo", &json!({})).unwrap();
    s.execute("undo", &json!({})).unwrap();
    s.execute("undo", &json!({})).unwrap();
    let r = s.execute("blocks.dynamic", &json!({ "name": "Box" })).unwrap();
    assert_eq!(r["actions"].as_array().map_or(0, Vec::len), 0, "{r}");
}

#[test]
fn an_authored_dynamic_block_survives_saving_to_dxf() {
    let mut s = stretchable();
    let h = insert(&mut s, "Box", [100.0, 0.0]);
    s.execute("blocks.reference.set", &json!({ "handle": h, "name": "Width", "value": "20" })).unwrap();
    let text = cadaddy_io::write_dxf(s.doc().unwrap());
    let back = cadaddy_io::read(text.as_bytes(), "box.dxf").unwrap();
    let mut s2 = Session::new();
    s2.open_drawing(back, "box", None);
    // The stretched reference is still one, at its width.
    assert_eq!(property(&mut s2, &h, "Width"), json!("20.0000"));
    assert!((right_edge(s2.doc().unwrap(), &h) - 20.0).abs() < 1e-9);
    // A new reference stretches as authored.
    let h2 = insert(&mut s2, "Box", [0.0, 50.0]);
    s2.execute("blocks.reference.set", &json!({ "handle": h2, "name": "Width", "value": "30" })).unwrap();
    assert!((right_edge(s2.doc().unwrap(), &h2) - 30.0).abs() < 1e-9);
    // And it changes again from there (the copies keep their element tags).
    s2.execute("blocks.reference.set", &json!({ "handle": h2, "name": "Width", "value": "10" })).unwrap();
    assert!((right_edge(s2.doc().unwrap(), &h2) - 10.0).abs() < 1e-9);
}

#[test]
fn a_move_action_carries_objects_with_a_point() {
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [4.0, 0.0]);
    let b = line(&mut s, [0.0, 1.0], [0.0, 3.0]);
    s.execute("block", &json!({ "name": "Tag", "base": [0, 0], "handles": [a, b] })).unwrap();
    let hs = block_handles(&s, "Tag");
    let r = s.execute("bparameter", &json!({ "block": "Tag", "type": "point", "at": [0, 2] })).unwrap();
    assert_eq!(r["label"], "Position1");
    s.execute("bactiontool", &json!({ "block": "Tag", "type": "move", "parameter": "Position1", "handles": [hs[1]] })).unwrap();
    let h = insert(&mut s, "Tag", [0.0, 0.0]);
    let g = s.execute("blocks.reference.grips", &json!({ "handle": h })).unwrap();
    assert_eq!(g.as_array().unwrap().len(), 1, "{g}");
    let node = r["node"].clone();
    s.execute("blocks.reference.grip", &json!({ "handle": h, "node": node, "part": "at", "to": [5, 2] })).unwrap();
    assert!((right_edge(s.doc().unwrap(), &h) - 5.0).abs() < 1e-9, "the short line moved 5 to the right");
}

#[test]
fn visibility_states_are_authored_and_switched() {
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [4.0, 0.0]);
    let b = line(&mut s, [0.0, 1.0], [8.0, 1.0]);
    s.execute("block", &json!({ "name": "Sign", "base": [0, 0], "handles": [a, b] })).unwrap();
    let hs = block_handles(&s, "Sign");
    s.execute("bparameter", &json!({ "block": "Sign", "type": "visibility", "at": [0, -1] })).unwrap();
    let r = s.execute("bvstate", &json!({ "block": "Sign", "action": "rename", "name": "VisibilityState0", "newName": "Long" })).unwrap();
    assert_eq!(r["states"][0]["name"], "Long");
    s.execute("bvstate", &json!({ "block": "Sign", "action": "new", "name": "Short", "handles": [hs[0]] })).unwrap();
    let r = s.execute("bvstate", &json!({ "block": "Sign" })).unwrap();
    assert_eq!(r["states"].as_array().unwrap().len(), 2);
    assert!(s.execute("bvstate", &json!({ "block": "Sign", "action": "new", "name": "short" })).is_err());
    assert!(s.execute("bparameter", &json!({ "block": "Sign", "type": "visibility", "at": [0, 0] })).is_err(), "one visibility parameter");

    let h = insert(&mut s, "Sign", [0.0, 0.0]);
    assert_eq!(property(&mut s, &h, "Visibility1"), json!("Long"));
    s.execute("blocks.reference.set", &json!({ "handle": h, "name": "Visibility1", "value": "Short" })).unwrap();
    let d = s.doc().unwrap();
    let Some(EntityKind::Insert(i)) = d.entity(Handle::parse_hex(&h).unwrap()).map(|e| &e.kind) else { panic!() };
    let shown: Vec<f64> = d
        .block(&i.block)
        .unwrap()
        .entities
        .iter()
        .filter(|e| e.common.visible)
        .filter_map(|e| match &e.kind {
            EntityKind::Line(l) => Some(l.b.x),
            _ => None,
        })
        .collect();
    assert_eq!(shown, vec![4.0]);
    // Deleting a state; the last one stays.
    s.execute("bvstate", &json!({ "block": "Sign", "action": "delete", "name": "Short" })).unwrap();
    assert!(s.execute("bvstate", &json!({ "block": "Sign", "action": "delete", "name": "Long" })).is_err());
}

#[test]
fn a_lookup_table_sets_a_distance() {
    let mut s = stretchable();
    s.execute("bparameter", &json!({ "block": "Box", "type": "lookup", "at": [5, -2], "label": "Size" })).unwrap();
    s.execute(
        "bactiontool",
        &json!({ "block": "Box", "type": "lookup", "parameter": "Size", "columns": [{ "parameter": "Width" }, { "parameter": "Size", "lookup": true }], "rows": [["10", "Small"], ["30", "Large"]] }),
    )
    .unwrap();
    let h = insert(&mut s, "Box", [0.0, 0.0]);
    let r = s.execute("blocks.reference.set", &json!({ "handle": h, "name": "Size", "value": "Large" }));
    assert!(r.is_ok(), "{r:?} {}", s.execute("blocks.dynamic", &json!({ "name": "Box" })).unwrap());
    assert!((right_edge(s.doc().unwrap(), &h) - 30.0).abs() < 1e-9);
    // Removing the distance takes the stretch and the lookup table with it.
    s.execute("bremove", &json!({ "block": "Box", "element": "Width" })).unwrap();
    let r = s.execute("blocks.dynamic", &json!({ "name": "Box" })).unwrap();
    assert_eq!((r["parameters"].as_array().unwrap().len(), r["actions"].as_array().unwrap().len()), (1, 0), "{r}");
}

#[test]
fn bad_authoring_input_is_refused() {
    let mut s = boxed();
    let hs = block_handles(&s, "Box");
    for p in [
        json!({ "block": "Nope", "type": "point", "at": [0, 0] }),
        json!({ "block": "Box", "type": "linear", "base": [0, 0], "end": [0, 0] }),
        json!({ "block": "Box", "type": "linear", "base": [0, 0], "end": [1, 0], "values": { "list": [-1] } }),
        json!({ "block": "Box", "type": "linear", "base": [0, 0], "end": [1, 0], "values": { "minimum": 5, "maximum": 1 } }),
        json!({ "block": "Box", "type": "wobble", "at": [0, 0] }),
        json!({ "block": "Box", "type": "point" }),
        json!({ "block": "*Model_Space", "type": "point", "at": [0, 0] }),
    ] {
        assert!(s.execute("bparameter", &p).is_err(), "{p}");
    }
    assert!(
        s.execute("bactiontool", &json!({ "block": "Box", "type": "move", "parameter": "Position1", "handles": hs })).is_err(),
        "no parameter yet"
    );
    s.execute("bparameter", &json!({ "block": "Box", "type": "point", "at": [0, 0] })).unwrap();
    for p in [
        json!({ "block": "Box", "type": "move", "parameter": "Position1", "handles": [] }),
        json!({ "block": "Box", "type": "move", "parameter": "Position1", "handles": ["FFFFF"] }),
        json!({ "block": "Box", "type": "move", "parameter": "Nope", "handles": hs }),
        json!({ "block": "Box", "type": "stretch", "parameter": "Position1", "handles": hs, "frame": [[0, 0], [1, 1]] }),
        json!({ "block": "Box", "type": "lookup", "parameter": "Position1", "columns": [{ "parameter": "Position1" }], "rows": [["1", "2"]] }),
    ] {
        assert!(s.execute("bactiontool", &p).is_err(), "{p}");
    }
    assert!(s.execute("bvstate", &json!({ "block": "Box" })).is_err(), "no visibility parameter");
}
