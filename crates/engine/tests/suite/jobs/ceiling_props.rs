//! A built ceiling's properties (Q6595): every built ceiling (a reflected ceiling plan's area, a
//! room's own ceiling, a level's) carries its room (number and name), its ceiling type, its height
//! above its level, its finish and assembly, each with where it came from, as derived properties
//! from the same data the build used, so `bim.props.find {kind: ceiling, where: {room: N}}`
//! reaches it and `bim.completeness` counts it filled. Made-up buildings.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], value: &str) {
    s.execute("text", &json!({"at": at, "text": value, "height": 6.0})).unwrap();
}

fn sq(x0: f64, y0: f64, x1: f64, y1: f64) -> Value {
    json!([[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

fn board(m: &str) -> Value {
    json!({"material": m, "thickness_in": 0.625, "category": "board"})
}

/// Ceiling types of a made-up ceiling legend: a gypsum board one and a tile one.
fn ceiling_types() -> Value {
    json!([
        {"name": "A", "kind": "gypsum", "layers": [board("5/8\" gypsum board"),
            {"material": "7/8\" furring channel @ 16\" o.c.", "thickness_in": 0.875, "category": "furring", "spacing_in": 16}]},
        {"name": "D", "description": "2X2 ACOUSTICAL TILE IN EXPOSED GRID", "kind": "grid", "grid_in": [24, 24],
         "layers": [{"material": "5/8\" acoustical ceiling tile", "thickness_in": 0.625, "category": "tile"},
                    {"material": "exposed tee grid", "thickness_in": 1.5, "category": "grid"}]},
        {"name": "G", "description": "GYPSUM BOARD CEILING, ONE LAYER", "kind": "gypsum", "layers": [board("1/2\" gypsum board")]}
    ])
}

/// A made-up 40' x 24' storey (inches) with two rooms on its reflected ceiling plan: room 101 a
/// type A ceiling at 8'-0" (the room finish schedule paints it), room 102 a type D tile ceiling
/// at 9'-0" in a 2x2 grid, a soffit in room 102.
fn rcp_building() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    let t = 7.25;
    rect(&mut s, [0.0, 0.0], [480.0, 288.0]);
    rect(&mut s, [t, t], [480.0 - t, 288.0 - t]);
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 10}],
        "layers": [{"layer": "A-WALL", "role": "wall"}],
        "ceiling_types": ceiling_types(),
        "soffit_types": [{"name": "S1", "description": "GYPSUM BOARD SOFFIT", "hangs_from": "ceiling",
                          "bottom": [board("5/8\" gypsum board")], "sides": [board("5/8\" gypsum board")],
                          "framing": [{"material": "3 5/8\" metal studs @ 16\" o.c.", "thickness_in": 3.625, "category": "framing", "spacing_in": 16}]}],
        "rooms": [
            {"name": "OFFICE", "number": "101", "level": "1", "materials": {"ceiling": "PT-1 PAINT"}},
            {"name": "TOILET", "number": "102", "level": "1"}
        ],
        "rcp": {
            "ceilings": [
                {"level": "1", "room": "101", "outline": sq(7.25, 7.25, 240.0, 280.75), "height_ft": 8, "ceiling_type": "A",
                 "source": "8'-0\" ceiling tag on sheet RCP-1"},
                {"level": "1", "room": "102", "outline": sq(240.0, 7.25, 472.75, 280.75), "height_ft": 9, "ceiling_type": "D",
                 "grid": {"spacing_in": [24, 24], "origin": [240, 7.25]}, "source": "9'-0\" ceiling tag on sheet RCP-1"}
            ],
            "soffits": [{"level": "1", "outline": sq(240.0, 244.75, 472.75, 280.75), "height_ft": 7.5, "soffit_type": "S1", "top_ft": 9}]
        }
    });
    let r = s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    assert!(r["errors"].as_array().is_none_or(|e| e.is_empty()), "{r}");
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// The handles `bim.props.find` gives for kind ceiling with `where`.
fn find(s: &mut Session, wanted: Value) -> Vec<String> {
    let r = s.execute("bim.props.find", &json!({"kind": "ceiling", "where": wanted, "limit": 1000})).unwrap();
    r["handles"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_string()).collect()
}

/// Each element's properties as `bim.props.get` gives them: key → (value, source kind, doc).
fn props(s: &mut Session, handles: &[String]) -> Vec<serde_json::Map<String, Value>> {
    let r = s.execute("bim.props.get", &json!({"handles": handles})).unwrap();
    r["elements"].as_array().unwrap().iter().map(|e| e["props"].as_object().cloned().unwrap_or_default()).collect()
}

fn value(p: &serde_json::Map<String, Value>, key: &str) -> Value {
    p.get(key).map_or(Value::Null, |v| v["value"].clone())
}

fn source(p: &serde_json::Map<String, Value>, key: &str) -> (String, String) {
    let src = p.get(key).map_or(Value::Null, |v| v["source"].clone());
    (src["kind"].as_str().unwrap_or_default().to_string(), src["doc"].as_str().unwrap_or_default().to_string())
}

fn part(s: &Session, handle: &str) -> String {
    let d = s.doc().unwrap();
    let e = d.model.iter().find(|e| e.handle.hex() == handle).unwrap();
    cadaddy_engine::cmd::wallgen::made_as(e).unwrap_or_default()
}

#[test]
fn rcp_ceilings_carry_their_room_type_height_and_finish() {
    let mut s = rcp_building();
    let all = find(&mut s, json!({}));
    let parts: Vec<String> = all.iter().map(|h| part(&s, h)).collect();
    assert_eq!(parts.iter().filter(|p| *p == "ceiling").count(), 2, "{parts:?}");

    // A room-keyed filter reaches each room's ceiling.
    let in_101 = find(&mut s, json!({"room": "101"}));
    assert_eq!(in_101.len(), 1, "room 101's ceiling: {in_101:?}");
    let p = props(&mut s, &in_101).remove(0);
    assert_eq!(value(&p, "type"), json!("A"), "{p:?}");
    assert_eq!(source(&p, "type").0, "print", "the RCP types it: {p:?}");
    assert_eq!(value(&p, "height"), json!(8.0), "{p:?}");
    assert_eq!(source(&p, "height"), ("print".into(), "8'-0\" ceiling tag on sheet RCP-1".into()), "where it was read: {p:?}");
    assert_eq!(value(&p, "finish"), json!("PT-1 PAINT"), "the room finish schedule's: {p:?}");
    assert_eq!(source(&p, "finish").0, "schedule", "{p:?}");
    assert_eq!(value(&p, "assembly"), json!("5/8\" gypsum board / 7/8\" furring channel @ 16\" o.c."), "its type's layers: {p:?}");
    assert_eq!(source(&p, "assembly"), ("legend".into(), "ceiling type A".into()), "{p:?}");
    assert_eq!(value(&p, "room"), json!("101"), "{p:?}");
    assert_eq!(value(&p, "room_name"), json!("OFFICE"), "{p:?}");
    assert_eq!(value(&p, "level"), json!("1"), "{p:?}");
    let area = value(&p, "area").as_f64().unwrap();
    assert!((area - (232.75 * 273.5 / 144.0)).abs() < 0.05, "its plan area: {area}");

    // Room 102: its ceiling, the grid hung in it and the soffit in it.
    let in_102 = find(&mut s, json!({"room": "102"}));
    let parts_102: Vec<String> = in_102.iter().map(|h| part(&s, h)).collect();
    let mut by_part: Vec<(String, serde_json::Map<String, Value>)> = parts_102.into_iter().zip(props(&mut s, &in_102)).collect();
    by_part.sort_by(|a, b| a.0.cmp(&b.0));
    let got: Vec<&str> = by_part.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(got, vec!["ceiling", "grid", "soffit"], "{by_part:?}");
    let (_, c) = &by_part[0];
    assert_eq!((value(c, "type"), value(c, "height"), value(c, "room_name")), (json!("D"), json!(9.0), json!("TOILET")), "{c:?}");
    assert_eq!(value(c, "finish"), json!("5/8\" acoustical ceiling tile"), "no schedule finish: its type's face: {c:?}");
    assert_eq!(source(c, "finish").0, "legend", "{c:?}");
    assert_eq!(value(c, "assembly"), json!("2X2 ACOUSTICAL TILE IN EXPOSED GRID"), "{c:?}");
    let (_, g) = &by_part[1];
    assert_eq!((value(g, "type"), value(g, "height")), (json!("D"), json!(9.0)), "the grid is its ceiling's: {g:?}");
    let (_, sf) = &by_part[2];
    assert_eq!((value(sf, "type"), value(sf, "height")), (json!("S1"), json!(7.5)), "{sf:?}");
    assert_eq!(value(sf, "assembly"), json!("GYPSUM BOARD SOFFIT"), "{sf:?}");

    // Every ceiling's type, height, finish and room are filled.
    let r = s.execute("bim.completeness", &json!({"kind": "ceiling"})).unwrap();
    let fields = &r["kinds"]["ceiling"]["fields"];
    for key in ["type", "height", "finish", "room", "level"] {
        assert_eq!(fields[key]["pct"].as_f64(), Some(100.0), "{key}: {}", fields[key]);
    }
}

const T: f64 = 7.25;
const HALF: f64 = 4.875 / 2.0;

/// A made-up 40' x 24' storey with two traced rooms and no reflected ceiling plan: room 201's
/// ceiling height is read from an enlarged plan, room 202 has none (the level's height).
fn room_building() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [480.0, 288.0]);
    rect(&mut s, [T, T], [480.0 - T, 288.0 - T]);
    for dx in [-HALF, HALF] {
        line(&mut s, [240.0 + dx, T], [240.0 + dx, 288.0 - T]);
    }
    layer(&mut s, "A-ROOM-NAME");
    text(&mut s, [120.0, 144.0], "201");
    text(&mut s, [360.0, 144.0], "202");
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [{"name": "2", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9}],
        "layers": [{"layer": "A-WALL", "role": "wall"}],
        "ceiling_types": ceiling_types(),
        "rooms": [
            {"name": "OFFICE", "number": "201", "level": "2", "ceiling_ft": 8.5, "ceiling_source": "8'-6\" on sheet ENLARGED PLAN-1",
             "ceiling_type": "A", "ceiling_type_source": "the room's ceiling type"},
            {"name": "STORAGE", "number": "202", "level": "2"}
        ]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

#[test]
fn room_ceilings_carry_their_room_height_and_type() {
    let mut s = room_building();
    let in_201 = find(&mut s, json!({"room": "201"}));
    assert_eq!(in_201.len(), 1, "room 201's ceiling: {in_201:?}");
    let p = props(&mut s, &in_201).remove(0);
    assert_eq!((value(&p, "room"), value(&p, "room_name")), (json!("201"), json!("OFFICE")), "{p:?}");
    assert_eq!(value(&p, "height"), json!(8.5), "{p:?}");
    assert_eq!(source(&p, "height"), ("print".into(), "8'-6\" on sheet ENLARGED PLAN-1".into()), "{p:?}");
    assert_eq!(value(&p, "type"), json!("A"), "{p:?}");
    assert_eq!(value(&p, "finish"), json!("5/8\" gypsum board"), "{p:?}");

    // Room 202: nothing gives its height, so the level's, marked a default (not a reading).
    let in_202 = find(&mut s, json!({"room": "202"}));
    assert_eq!(in_202.len(), 1, "{in_202:?}");
    let p = props(&mut s, &in_202).remove(0);
    assert_eq!(value(&p, "height"), json!(9.0), "{p:?}");
    assert_eq!(source(&p, "height").0, "default", "{p:?}");
    assert!(source(&p, "height").1.contains("level"), "{p:?}");
    assert_eq!(value(&p, "type"), Value::Null, "no document types it: {p:?}");

    // `rcp.type` retypes it: the type and its assembly and finish follow.
    let r = s.execute("rcp.type", &json!({"type": "G", "rooms": ["202"], "relayout": false})).unwrap();
    assert_eq!(r["ceilings"].as_u64(), Some(1), "{r}");
    let p = props(&mut s, &in_202).remove(0);
    assert_eq!(value(&p, "type"), json!("G"), "{p:?}");
    assert_eq!(source(&p, "type").0, "user", "{p:?}");
    assert_eq!(value(&p, "assembly"), json!("GYPSUM BOARD CEILING, ONE LAYER"), "{p:?}");
    assert_eq!(value(&p, "finish"), json!("1/2\" gypsum board"), "{p:?}");
    let r = s.execute("rcp.type", &json!({"type": "D", "rooms": ["202"], "force": true, "relayout": false})).unwrap();
    assert_eq!(r["ceilings"].as_u64(), Some(1), "{r}");
    let p = props(&mut s, &in_202).remove(0);
    assert_eq!((value(&p, "type"), value(&p, "assembly")), (json!("D"), json!("2X2 ACOUSTICAL TILE IN EXPOSED GRID")), "{p:?}");
}

#[test]
fn a_levels_own_ceiling_carries_the_levels_height_as_a_default() {
    // No traced rooms and no reflected ceiling plan: the level's ceiling.
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [480.0, 288.0]);
    rect(&mut s, [T, T], [480.0 - T, 288.0 - T]);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9.5}],
        "layers": [{"layer": "A-WALL", "role": "wall"}]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let all = find(&mut s, json!({}));
    assert!(!all.is_empty(), "the level has a ceiling");
    for p in props(&mut s, &all) {
        assert_eq!(value(&p, "height"), json!(9.5), "{p:?}");
        assert_eq!(source(&p, "height").0, "default", "{p:?}");
    }
}
