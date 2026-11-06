//! The authored actions beyond stretch and move (scale, rotate, flip, array, polar stretch) and
//! the polar, XY, rotation, alignment and flip parameters: each is checked with the exact
//! geometry after a property change and after a grip drag.

use cadaddy_engine::Session;
use cadaddy_engine::doc::dynblock::{self, Value as Pv};
use cadaddy_engine::doc::{Drawing, EntityKind, Handle};
use serde_json::{Value, json};

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> String {
    s.execute("line", &json!({ "points": [a, b] })).unwrap()["handles"][0].as_str().unwrap().to_string()
}

/// Block `name` of lines (each `[ax, ay, bx, by]`), base point at the origin; returns the block
/// object handles in order.
fn block_of(s: &mut Session, name: &str, lines: &[[f64; 4]]) -> Vec<String> {
    let hs: Vec<String> = lines.iter().map(|l| line(s, [l[0], l[1]], [l[2], l[3]])).collect();
    s.execute("block", &json!({ "name": name, "base": [0, 0], "handles": hs })).unwrap();
    s.doc().unwrap().block(name).unwrap().entities.handles().iter().map(|h| h.hex()).collect()
}

fn insert(s: &mut Session, name: &str) -> String {
    s.execute("insert", &json!({ "name": name, "at": [0, 0] })).unwrap();
    s.doc().unwrap().model.handles().last().unwrap().hex()
}

/// The lines the reference `h` shows, as `[ax, ay, bx, by]` in block coordinates.
fn lines_of(d: &Drawing, h: &str) -> Vec<[f64; 4]> {
    let Some(EntityKind::Insert(i)) = d.entity(Handle::parse_hex(h).unwrap()).map(|e| &e.kind) else { panic!("not a reference") };
    d.block(&i.block)
        .unwrap()
        .entities
        .iter()
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

fn drag(s: &mut Session, h: &str, node: &Value, part: &str, to: [f64; 2]) {
    s.execute("blocks.reference.grip", &json!({ "handle": h, "node": node, "part": part, "to": to })).unwrap();
}

fn points_of(s: &Session, h: &str, node: i32) -> (cadaddy_engine::geom::Vec3, cadaddy_engine::geom::Vec3) {
    let r = dynblock::reference(s.doc().unwrap(), Handle::parse_hex(h).unwrap()).unwrap();
    match r.values.get(&node) {
        Some(Pv::Points { base, end }) => (*base, *end),
        other => panic!("no points for {node}: {other:?}"),
    }
}

#[test]
fn scale_follows_a_distance_by_the_ratio() {
    let mut s = Session::new();
    let hs = block_of(&mut s, "Bar", &[[0.0, 0.0, 4.0, 0.0], [0.0, 2.0, 4.0, 2.0]]);
    let r = s.execute("bparameter", &json!({ "block": "Bar", "type": "linear", "base": [0, 0], "end": [10, 0], "label": "Width" })).unwrap();
    let node = r["node"].clone();
    s.execute("bactiontool", &json!({ "block": "Bar", "type": "scale", "parameter": "Width", "handles": hs })).unwrap();
    let h = insert(&mut s, "Bar");
    set(&mut s, &h, "Width", "20");
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[0.0, 0.0, 8.0, 0.0], [0.0, 4.0, 8.0, 4.0]]);
    // The grip back to 5: half of the original, from the doubled one (factor 5 / 20).
    drag(&mut s, &h, &node, "end", [5.0, 0.0]);
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[0.0, 0.0, 2.0, 0.0], [0.0, 1.0, 2.0, 1.0]]);
}

#[test]
fn rotate_follows_a_rotation_parameter_about_its_base() {
    let mut s = Session::new();
    let hs = block_of(&mut s, "Arm", &[[1.0, 0.0, 3.0, 0.0]]);
    let r = s.execute("bparameter", &json!({ "block": "Arm", "type": "rotation", "base": [0, 0], "end": [5, 0], "label": "Angle1" })).unwrap();
    let node = r["node"].clone();
    s.execute("bactiontool", &json!({ "block": "Arm", "type": "rotate", "parameter": "Angle1", "handles": hs })).unwrap();
    let h = insert(&mut s, "Arm");
    assert_eq!(s.execute("blocks.reference", &json!({ "handle": h })).unwrap()["properties"][0]["value"], json!("0.0000"));
    set(&mut s, &h, "Angle1", "90");
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[0.0, 1.0, 0.0, 3.0]]);
    let (b, e) = points_of(&s, &h, node.as_i64().unwrap() as i32);
    assert!(b.x.abs() < 1e-9 && e.x.abs() < 1e-9 && (e.y - 5.0).abs() < 1e-9, "{b:?} {e:?}");
    // The grip to the left of the base: 180°.
    drag(&mut s, &h, &node, "end", [-4.0, 0.0]);
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[-1.0, 0.0, -3.0, 0.0]]);
    assert_eq!(s.execute("blocks.reference", &json!({ "handle": h })).unwrap()["properties"][0]["value"], json!("180.0000"));
}

#[test]
fn flip_mirrors_about_the_reflection_line() {
    let mut s = Session::new();
    let hs = block_of(&mut s, "Fin", &[[2.0, 1.0, 4.0, 1.0]]);
    let r = s.execute("bparameter", &json!({ "block": "Fin", "type": "flip", "base": [0, 0], "end": [0, 10], "label": "Flip" })).unwrap();
    let node = r["node"].clone();
    s.execute("bactiontool", &json!({ "block": "Fin", "type": "flip", "parameter": "Flip", "handles": hs })).unwrap();
    let h = insert(&mut s, "Fin");
    assert_eq!(s.execute("blocks.reference", &json!({ "handle": h })).unwrap()["properties"][0]["value"], json!("Not flipped"));
    set(&mut s, &h, "Flip", "Flipped");
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[-2.0, 1.0, -4.0, 1.0]]);
    // Back, by dragging its grip to the left of the line (the base state).
    drag(&mut s, &h, &node, "at", [-3.0, 5.0]);
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[2.0, 1.0, 4.0, 1.0]]);
    // And flipped again by dragging it to the right.
    drag(&mut s, &h, &node, "at", [3.0, 5.0]);
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[-2.0, 1.0, -4.0, 1.0]]);
    let bad = s.execute("blocks.reference.set", &json!({ "handle": h, "name": "Flip", "value": "Sideways" }));
    assert!(bad.is_err());
}

#[test]
fn array_makes_a_column_per_offset_and_remakes_them() {
    let mut s = Session::new();
    let hs = block_of(&mut s, "Post", &[[0.0, 1.0, 0.0, 2.0]]);
    let r = s.execute("bparameter", &json!({ "block": "Post", "type": "linear", "base": [0, 0], "end": [2, 0], "label": "Run" })).unwrap();
    let node = r["node"].clone();
    assert!(s.execute("bactiontool", &json!({ "block": "Post", "type": "array", "parameter": "Run", "handles": hs })).is_err(), "needs an offset");
    s.execute("bactiontool", &json!({ "block": "Post", "type": "array", "parameter": "Run", "handles": hs, "columnOffset": 2 })).unwrap();
    let h = insert(&mut s, "Post");
    set(&mut s, &h, "Run", "8");
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[0.0, 1.0, 0.0, 2.0], [2.0, 1.0, 2.0, 2.0], [4.0, 1.0, 4.0, 2.0], [6.0, 1.0, 6.0, 2.0]]);
    // Shorter by its grip: the earlier copies are made again, not kept.
    drag(&mut s, &h, &node, "end", [5.0, 0.0]);
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[0.0, 1.0, 0.0, 2.0], [2.0, 1.0, 2.0, 2.0]]);
    set(&mut s, &h, "Run", "2");
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[0.0, 1.0, 0.0, 2.0]]);
}

#[test]
fn polar_stretch_moves_the_end_and_turns_the_rotate_objects() {
    let mut s = Session::new();
    let hs = block_of(&mut s, "Boom", &[[0.0, 0.0, 10.0, 0.0], [1.0, 1.0, 2.0, 1.0]]);
    let r = s.execute("bparameter", &json!({ "block": "Boom", "type": "polar", "base": [0, 0], "end": [10, 0], "label": "Reach" })).unwrap();
    assert_eq!(r["dynamic"]["parameters"][0]["kind"]["angleLabel"], "Angle1");
    let node = r["node"].clone();
    let a = s
        .execute(
            "bactiontool",
            &json!({ "block": "Boom", "type": "polarstretch", "parameter": "Reach", "handles": [hs[0]], "rotate": [hs[1]], "frame": [[9, -1], [11, -1], [11, 1], [9, 1]] }),
        )
        .unwrap();
    assert_eq!(a["name"], "Polar Stretch1");
    let h = insert(&mut s, "Boom");
    set(&mut s, &h, "Reach", "20");
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[0.0, 0.0, 20.0, 0.0], [1.0, 1.0, 2.0, 1.0]]);
    // A quarter turn at 20: the end follows (the stretched vertex moves by (-20, 20)) and the
    // rotate-only line turns about the base.
    set(&mut s, &h, "Angle1", "90");
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[0.0, 0.0, 0.0, 20.0], [-1.0, 1.0, -1.0, 2.0]]);
    // The end grip back out along x at 10: distance and angle together.
    drag(&mut s, &h, &node, "end", [10.0, 0.0]);
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[0.0, 0.0, 10.0, 0.0], [1.0, 1.0, 2.0, 1.0]]);
}

#[test]
fn xy_values_move_an_object_and_their_grip_sets_both() {
    let mut s = Session::new();
    let hs = block_of(&mut s, "Slide", &[[0.0, 0.0, 1.0, 0.0]]);
    let r = s.execute("bparameter", &json!({ "block": "Slide", "type": "xy", "base": [0, 0], "end": [4, 3], "label": "Across" })).unwrap();
    let node = r["node"].clone();
    assert_eq!(r["dynamic"]["parameters"][0]["kind"]["yLabel"], "Vertical1");
    s.execute("bactiontool", &json!({ "block": "Slide", "type": "move", "parameter": "Across", "handles": hs })).unwrap();
    let h = insert(&mut s, "Slide");
    set(&mut s, &h, "Vertical1", "5");
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[0.0, 2.0, 1.0, 2.0]]);
    set(&mut s, &h, "Across", "6");
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[2.0, 2.0, 3.0, 2.0]]);
    drag(&mut s, &h, &node, "end", [7.0, 6.0]);
    assert_lines(&lines_of(s.doc().unwrap(), &h), &[[3.0, 3.0, 4.0, 3.0]]);
    let props = s.execute("blocks.reference", &json!({ "handle": h })).unwrap()["properties"].clone();
    assert_eq!((props[0]["value"].clone(), props[1]["value"].clone()), (json!("7.0000"), json!("6.0000")));
}

#[test]
fn alignment_and_polar_values_follow_property_and_grip() {
    let mut s = Session::new();
    block_of(&mut s, "Plug", &[[0.0, 0.0, 1.0, 0.0]]);
    let a = s.execute("bparameter", &json!({ "block": "Plug", "type": "alignment", "base": [0, 0], "end": [5, 0], "label": "Align" })).unwrap();
    let p = s.execute("bparameter", &json!({ "block": "Plug", "type": "polar", "base": [0, 0], "end": [0, 4], "label": "Reach" })).unwrap();
    let (an, pn) = (a["node"].as_i64().unwrap() as i32, p["node"].as_i64().unwrap() as i32);
    let h = insert(&mut s, "Plug");
    set(&mut s, &h, "Align", "90");
    let (_, e) = points_of(&s, &h, an);
    assert!(e.x.abs() < 1e-9 && (e.y - 5.0).abs() < 1e-9, "{e:?}");
    drag(&mut s, &h, &json!(an), "end", [-9.0, 0.0]);
    let (_, e) = points_of(&s, &h, an);
    assert!((e.x + 5.0).abs() < 1e-9 && e.y.abs() < 1e-9, "{e:?}");
    // The polar parameter: distance 4 at 90°; set the distance, then the angle, then drag.
    set(&mut s, &h, "Reach", "6");
    let (_, e) = points_of(&s, &h, pn);
    assert!(e.x.abs() < 1e-9 && (e.y - 6.0).abs() < 1e-9, "{e:?}");
    set(&mut s, &h, "Angle1", "0");
    let (_, e) = points_of(&s, &h, pn);
    assert!((e.x - 6.0).abs() < 1e-9 && e.y.abs() < 1e-9, "{e:?}");
    drag(&mut s, &h, &json!(pn), "end", [3.0, 4.0]);
    let (_, e) = points_of(&s, &h, pn);
    assert!((e.x - 3.0).abs() < 1e-9 && (e.y - 4.0).abs() < 1e-9, "{e:?}");
}

#[test]
fn actions_only_follow_the_parameters_they_suit() {
    let mut s = Session::new();
    let hs = block_of(&mut s, "Odd", &[[0.0, 0.0, 1.0, 0.0]]);
    s.execute("bparameter", &json!({ "block": "Odd", "type": "point", "at": [0, 0], "label": "Spot" })).unwrap();
    for t in ["scale", "rotate", "flip", "polarstretch"] {
        let r =
            s.execute("bactiontool", &json!({ "block": "Odd", "type": t, "parameter": "Spot", "handles": hs, "frame": [[0, 0], [1, 0], [1, 1]] }));
        assert!(r.is_err(), "{t}");
    }
    let r = s.execute("bactiontool", &json!({ "block": "Odd", "type": "array", "parameter": "Spot", "handles": hs, "columnOffset": 1 }));
    assert!(r.is_err());
}

#[test]
fn the_new_parameters_and_actions_survive_saving_to_dxf() {
    let mut s = Session::new();
    let hs = block_of(&mut s, "Gear", &[[1.0, 0.0, 3.0, 0.0]]);
    s.execute(
        "bparameter",
        &json!({ "block": "Gear", "type": "rotation", "base": [0, 0], "end": [5, 0], "label": "Turn", "values": { "list": [90, 180] } }),
    )
    .unwrap();
    s.execute("bparameter", &json!({ "block": "Gear", "type": "flip", "base": [0, 0], "end": [0, 9], "label": "Mirror" })).unwrap();
    s.execute("bparameter", &json!({ "block": "Gear", "type": "xy", "base": [0, 0], "end": [3, 4], "label": "Slide" })).unwrap();
    s.execute("bactiontool", &json!({ "block": "Gear", "type": "rotate", "parameter": "Turn", "handles": hs })).unwrap();
    s.execute("bactiontool", &json!({ "block": "Gear", "type": "flip", "parameter": "Mirror", "handles": hs })).unwrap();
    s.execute("bactiontool", &json!({ "block": "Gear", "type": "array", "parameter": "Slide", "handles": hs, "columnOffset": 1, "rowOffset": 2 }))
        .unwrap();
    let before = s.execute("blocks.dynamic", &json!({ "name": "Gear" })).unwrap();
    let text = cadaddy_io::write_dxf(s.doc().unwrap());
    let back = cadaddy_io::read(text.as_bytes(), "gear.dxf").unwrap();
    let mut s2 = Session::new();
    s2.open_drawing(back, "gear", None);
    let after = s2.execute("blocks.dynamic", &json!({ "name": "Gear" })).unwrap();
    // The file renumbers handles; everything else is as authored.
    let strip = |v: &Value, keys: &[&str]| -> Vec<Value> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|e| {
                let mut e = e.clone();
                for k in keys {
                    e.as_object_mut().unwrap().remove(*k);
                }
                e
            })
            .collect()
    };
    assert_eq!(strip(&before["parameters"], &["handle"]), strip(&after["parameters"], &["handle"]));
    assert_eq!(strip(&before["actions"], &["handle", "selection"]), strip(&after["actions"], &["handle", "selection"]));
    assert_eq!(after["parameters"][2]["kind"]["yLabel"], "Vertical1");
    assert_eq!(after["actions"][2]["kind"]["columnOffset"], 1.0);
    assert_eq!(after["actions"][2]["kind"]["rowOffset"], 2.0);
    // The reloaded block still behaves: the rotation value set snaps 100° to 90°, and the array
    // along the XY parameter (3 × 4 over offsets 1 × 2) makes 3 columns of 2 rows.
    let h = insert(&mut s2, "Gear");
    set(&mut s2, &h, "Turn", "100");
    assert_lines(
        &lines_of(s2.doc().unwrap(), &h),
        &[[0.0, 1.0, 0.0, 3.0], [1.0, 1.0, 1.0, 3.0], [2.0, 1.0, 2.0, 3.0], [0.0, 3.0, 0.0, 5.0], [1.0, 3.0, 1.0, 5.0], [2.0, 3.0, 2.0, 5.0]],
    );
}

/// `blocks.dynamic` of `name` without handles (a file renumbers them).
fn shape(s: &mut Session, name: &str) -> (Value, Value) {
    let strip = |v: &Value, keys: &[&str]| -> Value {
        Value::Array(
            v.as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    let mut e = e.clone();
                    for k in keys {
                        e.as_object_mut().unwrap().remove(*k);
                    }
                    e
                })
                .collect(),
        )
    };
    let r = s.execute("blocks.dynamic", &json!({ "name": name })).unwrap();
    (strip(&r["parameters"], &["handle"]), strip(&r["actions"], &["handle", "selection"]))
}

#[test]
fn an_authored_block_survives_a_dwg_round_trip() {
    use cadaddy_io::{DwgEngine, Version};
    let mut s = Session::new();
    let hs = block_of(&mut s, "Panel", &[[0.0, 0.0, 10.0, 0.0], [10.0, 0.0, 10.0, 5.0], [10.0, 5.0, 0.0, 5.0], [0.0, 5.0, 0.0, 0.0]]);
    s.execute(
        "bparameter",
        &json!({ "block": "Panel", "type": "linear", "base": [0, 0], "end": [10, 0], "label": "Width", "values": { "list": [10, 20, 30] } }),
    )
    .unwrap();
    s.execute("bparameter", &json!({ "block": "Panel", "type": "rotation", "base": [0, 0], "end": [5, 0], "label": "Tilt" })).unwrap();
    s.execute(
        "bactiontool",
        &json!({ "block": "Panel", "type": "stretch", "parameter": "Width", "handles": hs, "frame": [[8, -1], [12, -1], [12, 6], [8, 6]] }),
    )
    .unwrap();
    s.execute("bactiontool", &json!({ "block": "Panel", "type": "rotate", "parameter": "Tilt", "handles": [hs[0]] })).unwrap();
    let before = shape(&mut s, "Panel");
    assert_eq!((before.0.as_array().unwrap().len(), before.1.as_array().unwrap().len()), (2, 2));
    for version in [Version::R2004, Version::R2018] {
        for engine in [DwgEngine::Bridge, DwgEngine::Native] {
            let bytes = cadaddy_io::write_dwg(s.doc().unwrap(), version, engine).unwrap();
            let mut s2 = Session::new();
            s2.open_drawing(cadaddy_io::read_dwg(&bytes, DwgEngine::Bridge).unwrap(), "panel", None);
            let label = format!("{version:?} written by {engine:?}, read by the bridge");
            assert_eq!(shape(&mut s2, "Panel"), before, "{label}");
            // A new reference stretches to the set width, with all four sides.
            let h = insert(&mut s2, "Panel");
            set(&mut s2, &h, "Width", "30");
            assert_lines(
                &lines_of(s2.doc().unwrap(), &h),
                &[[0.0, 0.0, 30.0, 0.0], [30.0, 0.0, 30.0, 5.0], [30.0, 5.0, 0.0, 5.0], [0.0, 5.0, 0.0, 0.0]],
            );
        }
    }
}

#[test]
fn every_parameter_and_action_type_survives_a_dwg_round_trip() {
    use cadaddy_io::{DwgEngine, Version};
    let mut s = Session::new();
    let hs = block_of(&mut s, "Wheel", &[[0.0, 0.0, 10.0, 0.0], [1.0, 1.0, 2.0, 1.0], [0.0, 3.0, 4.0, 3.0]]);
    for p in [
        json!({ "type": "polar", "base": [0, 0], "end": [10, 0], "label": "Reach", "values": { "minimum": 2, "maximum": 30, "increment": 2 } }),
        json!({ "type": "xy", "base": [0, 0], "end": [6, 4], "label": "Across" }),
        json!({ "type": "alignment", "base": [0, 0], "end": [0, 5], "label": "Align" }),
        json!({ "type": "flip", "base": [0, 0], "end": [0, 9], "label": "Mirror" }),
        json!({ "type": "linear", "base": [0, 0], "end": [2, 0], "label": "Run" }),
        json!({ "type": "rotation", "base": [0, 0], "end": [5, 0], "label": "Tilt" }),
    ] {
        let mut p = p;
        p["block"] = json!("Wheel");
        s.execute("bparameter", &p).unwrap();
    }
    let frame = json!([[9, -1], [11, -1], [11, 1], [9, 1]]);
    for a in [
        json!({ "type": "polarstretch", "parameter": "Reach", "handles": [hs[0]], "rotate": [hs[1]], "frame": frame }),
        json!({ "type": "move", "parameter": "Across", "handles": [hs[2]] }),
        json!({ "type": "flip", "parameter": "Mirror", "handles": [hs[2]] }),
        json!({ "type": "scale", "parameter": "Run", "handles": [hs[1]] }),
        json!({ "type": "array", "parameter": "Run", "handles": [hs[2]], "columnOffset": 2 }),
        json!({ "type": "rotate", "parameter": "Tilt", "handles": [hs[1]] }),
    ] {
        let mut a = a;
        a["block"] = json!("Wheel");
        s.execute("bactiontool", &a).unwrap();
    }
    let before = shape(&mut s, "Wheel");
    assert_eq!((before.0.as_array().unwrap().len(), before.1.as_array().unwrap().len()), (6, 6));
    for version in [Version::R2004, Version::R2018] {
        for engine in [DwgEngine::Bridge, DwgEngine::Native] {
            let bytes = cadaddy_io::write_dwg(s.doc().unwrap(), version, engine).unwrap();
            let mut s2 = Session::new();
            s2.open_drawing(cadaddy_io::read_dwg(&bytes, DwgEngine::Bridge).unwrap(), "wheel", None);
            let (params, actions) = shape(&mut s2, "Wheel");
            let label = format!("{version:?} written by {engine:?}");
            // Parameter kinds (with their own data) and action kinds are what was authored; the
            // action parts that name objects follow the file's handles.
            assert_eq!(params, before.0, "{label}");
            let kinds = |a: &Value| {
                a.as_array().unwrap().iter().map(|x| (x["name"].clone(), x["kind"]["type"].clone(), x["connections"].clone())).collect::<Vec<_>>()
            };
            assert_eq!(kinds(&actions), kinds(&before.1), "{label}");
            // The reloaded block behaves: the polar parameter sets 20 (snapped by its value set)
            // and the rotate-only line follows a quarter turn.
            let h = insert(&mut s2, "Wheel");
            set(&mut s2, &h, "Reach", "21");
            let got = lines_of(s2.doc().unwrap(), &h);
            assert!((got[0][2] - 22.0).abs() < 1e-9, "{label}: the increment of 2 makes 21 into 22: {got:?}");
            set(&mut s2, &h, "Angle1", "90");
            let got = lines_of(s2.doc().unwrap(), &h);
            assert!((got[1][0] + 1.0).abs() < 1e-9 && (got[1][1] - 1.0).abs() < 1e-9, "{label}: {got:?}");
        }
    }
}
