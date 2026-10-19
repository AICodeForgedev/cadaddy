//! Door and window schedules: a door schedule TABLE and a window schedule drawn as text, read
//! into rows (sizes from SIZE, door codes, WIDTH/HEIGHT/SILL); tags on the plan matched to them,
//! a tag with no row and a row with no tag flagged; the built openings sized from their rows.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{EntityKind, Mesh};
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": at, "text": t, "height": h})).unwrap();
}

/// A block of one line `w` long, inserted at `at` turned `rotation` degrees.
fn block(s: &mut Session, name: &str, w: f64, at: [f64; 2], rotation: f64) {
    if s.doc().unwrap().block(name).is_none() {
        let before: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
        s.execute("line", &json!({"points": [[-w / 2.0, 0.0], [w / 2.0, 0.0]]})).unwrap();
        let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).filter(|h| !before.contains(h)).collect();
        s.execute("block", &json!({"name": name, "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    }
    s.execute("insert", &json!({"name": name, "at": at, "rotation": rotation})).unwrap();
}

fn tag(s: &mut Session, at: [f64; 2], mark: &str) {
    s.execute("circle", &json!({"center": at, "radius": 9})).unwrap();
    text(s, [at[0] - 6.0, at[1] - 3.0], mark, 6.0);
}

fn drawing() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [480.0, 240.0]);
    rect(&mut s, [6.0, 6.0], [234.0, 234.0]);
    rect(&mut s, [246.0, 6.0], [474.0, 234.0]);
    layer(&mut s, "A-DOOR");
    block(&mut s, "DOOR-36", 36.0, [240.0, 60.0], 90.0);
    block(&mut s, "DOOR-36", 36.0, [240.0, 180.0], 90.0);
    block(&mut s, "DOOR-36", 36.0, [120.0, 237.0], 0.0);
    layer(&mut s, "A-GLAZ");
    block(&mut s, "WIN-48", 48.0, [360.0, 3.0], 0.0);
    layer(&mut s, "A-ANNO-TAGS");
    tag(&mut s, [262.0, 60.0], "101");
    tag(&mut s, [262.0, 180.0], "102");
    tag(&mut s, [120.0, 262.0], "103");
    tag(&mut s, [360.0, -25.0], "W1");
    // The door schedule, a TABLE.
    layer(&mut s, "A-SCHED");
    s.execute(
        "table",
        &json!({"at": [700, 900], "rows": 3, "cols": 6, "title": "DOOR SCHEDULE", "header": ["MARK", "SIZE", "TYPE", "FRAME", "FIRE RATING", "HDW SET"],
            "cells": [["101", "3'-0\" x 7'-0\"", "A", "HM", "20 MIN", "1"], ["102", "2868", "B", "WD", "", "2"], ["104", "3070", "A", "HM", "90 MIN", "3"]]}),
    )
    .unwrap();
    // The window schedule, drawn as text.
    text(&mut s, [700.0, 330.0], "WINDOW SCHEDULE", 8.0);
    for (x, h) in [(700.0, "MARK"), (760.0, "WIDTH"), (820.0, "HEIGHT"), (880.0, "SILL")] {
        text(&mut s, [x, 300.0], h, 6.0);
    }
    for (x, v) in [(700.0, "W1"), (760.0, "4'-0\""), (820.0, "5'-0\""), (880.0, "2'-6\"")] {
        text(&mut s, [x, 285.0], v, 6.0);
    }
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": [-100, -100, 600, 600], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}, {"layer": "A-DOOR", "role": "door"}, {"layer": "A-GLAZ", "role": "window"}]
        }}),
    )
    .unwrap();
    s
}

fn schedule<'a>(r: &'a Value, kind: &str) -> &'a Value {
    r["schedules"].as_array().unwrap().iter().find(|x| x["kind"] == kind).unwrap_or_else(|| panic!("{kind}: {r}"))
}

#[test]
fn schedules_and_tags_are_read_and_checked() {
    let mut s = drawing();
    let r = s.execute("schedules.read", &json!({})).unwrap();
    let doors = schedule(&r, "door");
    assert_eq!((doors["title"].clone(), doors["from"].clone()), (json!("DOOR SCHEDULE"), json!("table")));
    let rows = doors["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "{doors}");
    assert_eq!(
        rows[0],
        json!({"kind": "door", "mark": "101", "width_in": 36.0, "height_in": 84.0, "type": "A", "frame": "HM", "fire_rating": "20 MIN", "hardware": "1"})
    );
    assert_eq!((rows[1]["width_in"].clone(), rows[1]["height_in"].clone()), (json!(32.0), json!(80.0)), "the door code 2868");
    let windows = schedule(&r, "window");
    assert_eq!((windows["title"].clone(), windows["from"].clone()), (json!("WINDOW SCHEDULE"), json!("text")));
    assert_eq!(windows["rows"], json!([{"kind": "window", "mark": "W1", "width_in": 48.0, "height_in": 60.0, "sill_ft": 2.5}]));
    // Tags: four, of which 103 has no row; 104 has no tag.
    let marks: Vec<&str> = r["tags"].as_array().unwrap().iter().map(|t| t["mark"].as_str().unwrap()).collect();
    assert_eq!(marks.len(), 4, "{r}");
    assert_eq!(r["flags"]["tags_without_row"].as_array().unwrap().iter().map(|t| t["mark"].as_str().unwrap()).collect::<Vec<_>>(), vec!["103"]);
    assert_eq!(r["flags"]["rows_without_tag"], json!([{"kind": "door", "mark": "104"}]));
    // Applied: in the spec, in plain language.
    s.execute("schedules.read", &json!({"apply": true})).unwrap();
    let parts = s.execute("bspec.get", &json!({})).unwrap()["parts"].clone();
    let text =
        |k: &str| parts.as_array().unwrap().iter().find(|p| p["key"] == k).map(|p| p["text"].as_str().unwrap().to_string()).unwrap_or_default();
    assert_eq!(text("schedule/doors"), "Door schedule: 101 3'-0\"×7'-0\", 102 2'-8\"×6'-8\", 104 3'-0\"×7'-0\"");
    assert_eq!(text("schedule/windows"), "Window schedule: W1 4'-0\"×5'-0\"");
}

fn meshes(s: &Session, part: &str) -> Vec<Mesh> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some(part))
        .filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None })
        .collect()
}

fn range(m: &Mesh, axis: usize) -> (f64, f64) {
    m.vertices.iter().map(|v| [v.x, v.y, v.z][axis]).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)))
}

#[test]
fn built_openings_take_their_schedule_sizes() {
    let mut s = drawing();
    s.execute("schedules.read", &json!({"apply": true})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!((r["doors"].clone(), r["windows"].clone()), (json!(3), json!(1)), "{r}");
    let doors = meshes(&s, "door");
    // 101: 3'-0" × 7'-0" at y 42–78; 102: 2'-8" × 6'-8" at y 164–196; 103: no row, as drawn.
    let at = |y: f64| {
        doors
            .iter()
            .find(|m| {
                let (a, b) = range(m, 1);
                a <= y && b >= y && range(m, 0).0 > 200.0
            })
            .unwrap_or_else(|| panic!("a door at y {y}"))
    };
    let d101 = at(60.0);
    assert_eq!(range(d101, 2), (0.0, 84.0));
    assert!((range(d101, 1).1 - range(d101, 1).0 - 36.0).abs() < 1e-6, "{:?}", range(d101, 1));
    let d102 = at(180.0);
    assert_eq!(range(d102, 2), (0.0, 80.0));
    assert!((range(d102, 1).1 - range(d102, 1).0 - 32.0).abs() < 1e-6, "{:?}", range(d102, 1));
    // The window: 4'-0" wide from its 2'-6" sill to 7'-6".
    let glass = &meshes(&s, "window")[0];
    assert_eq!(range(glass, 2), (30.0, 90.0));
    assert!((range(glass, 0).1 - range(glass, 0).0 - 48.0).abs() < 1e-6);
}
