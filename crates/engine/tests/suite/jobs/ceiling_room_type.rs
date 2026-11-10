//! Ceiling types by room (Q6511): a built ceiling with no ceiling type is the membrane of the
//! rated floor/ceiling assembly over its level when the documents give one (its board layers,
//! marked as an assumption), else the trade rules' default; `rcp.type` gives the built ceilings
//! of rooms a type by room name or number, unit type or the basis of their ceiling height, and
//! the type reaches the trade pieces and survives a rebuild. A made-up two-storey building.

use cadaddy_engine::Session;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], value: &str) {
    s.execute("text", &json!({"at": at, "text": value, "height": 6.0})).unwrap();
}

const T: f64 = 7.25;
const HALF: f64 = 4.875 / 2.0;

/// A 40' x 24' storey at `ox`: exterior walls, a partition at 20', two numbered rooms.
fn storey(s: &mut Session, ox: f64, n: u32) {
    layer(s, "A-WALL");
    rect(s, [ox, 0.0], [ox + 480.0, 288.0]);
    rect(s, [ox + T, T], [ox + 480.0 - T, 288.0 - T]);
    for dx in [-HALF, HALF] {
        line(s, [ox + 240.0 + dx, T], [ox + 240.0 + dx, 288.0 - T]);
    }
    layer(s, "A-ROOM-NAME");
    text(s, [ox + 120.0, 144.0], &format!("{n}01"));
    text(s, [ox + 360.0, 144.0], &format!("{n}02"));
}

/// Two storeys; a ceiling legend with a rated floor/ceiling membrane (two layers of Type X on
/// resilient channel) and a plain one-layer ceiling; no reflected ceiling plan areas. The lower
/// rooms' ceiling heights come from the floor/ceiling assembly over them.
fn building(extra_types: Value) -> Session {
    let mut s = Session::new();
    storey(&mut s, 0.0, 1);
    storey(&mut s, 2000.0, 2);
    let gyp = |m: &str, t: f64| json!({"material": m, "thickness_in": t, "category": "board"});
    let mut types = vec![
        json!({"name": "FC-1", "description": "1 HR FLOOR/CEILING ASSEMBLY", "kind": "gypsum", "fire_rating": "1 HR",
               "layers": [gyp("5/8\" Type X gypsum board", 0.625), gyp("5/8\" Type X gypsum board", 0.625),
                          {"material": "resilient channel @ 16\" o.c.", "thickness_in": 0.5, "category": "furring", "spacing_in": 16.0}]}),
        json!({"name": "GB-1", "description": "GYPSUM BOARD CEILING", "kind": "gypsum",
               "layers": [gyp("1/2\" gypsum board", 0.5)]}),
    ];
    types.extend(extra_types.as_array().cloned().unwrap_or_default());
    let derived = "derived: underside of the floor/ceiling assembly over level 1 (no height on the ceiling plan)";
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [
            {"name": "1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000]},
            {"name": "2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9, "region": [1900, -100, 3000, 1000]}
        ],
        "layers": [{"layer": "A-WALL", "role": "wall"}],
        "ceiling_types": types,
        "rooms": [
            {"name": "OFFICE", "number": "101", "level": "1", "ceiling_ft": 9.0, "ceiling_source": derived},
            {"name": "SUITE", "number": "102", "level": "1", "ceiling_ft": 9.0, "ceiling_source": derived},
            {"name": "OFFICE", "number": "201", "level": "2"},
            {"name": "SUITE", "number": "202", "level": "2"}
        ],
        "unit_instances": [
            {"number": "201", "level": "2", "type": "UNIT-A"},
            {"number": "102", "level": "1", "type": "UNIT-B"}
        ]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// A room's ceiling sheets: by board layer (1 = the room face), their area (sf), the materials
/// and the source of what they are.
#[derive(Debug, Default)]
struct Boards {
    area_by_layer: BTreeMap<u32, f64>,
    items: Vec<String>,
    sources: Vec<(String, String)>,
}

fn ceiling_boards(s: &Session, room_number: &str) -> Boards {
    let d = s.doc().unwrap();
    let mut b = Boards::default();
    for e in d.model.iter() {
        if cadaddy_engine::cmd::wallgen::made_as(e).as_deref() != Some("atom") {
            continue;
        }
        if !e.common.xdata.iter().any(|t| t.value == "part:atom:sheet") {
            continue;
        }
        let ps = cadaddy_doc::props::read(&e.common.xdata);
        let get = |k: &str| ps.get(k).map(|p| p.text()).unwrap_or_default();
        if get("face") != "ceiling" || !get("room").contains(room_number) {
            continue;
        }
        let layer: u32 = get("layer").parse().unwrap_or(0);
        let area: f64 = get("area_sf").parse().unwrap_or(0.0);
        *b.area_by_layer.entry(layer).or_default() += area;
        let item = get("item");
        if !b.items.contains(&item) {
            b.items.push(item);
        }
        if let Some(p) = ps.get("item") {
            let src = (format!("{:?}", p.source.kind).to_ascii_lowercase(), p.source.doc.clone().unwrap_or_default());
            if !b.sources.contains(&src) {
                b.sources.push(src);
            }
        }
    }
    b
}

fn layers(b: &Boards) -> Vec<u32> {
    b.area_by_layer.keys().copied().collect()
}

/// Every layer covers the same ceiling.
fn same_area(b: &Boards) {
    let a: Vec<f64> = b.area_by_layer.values().copied().collect();
    assert!(a.iter().all(|x| *x > 1.0), "{b:?}");
    assert!(a.windows(2).all(|w| (w[0] - w[1]).abs() < 0.5), "every layer covers the ceiling: {b:?}");
}

fn ceiling_type_tags(s: &Session, room_number: &str) -> Vec<String> {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("ceiling"))
        .filter(|e| e.common.xdata.iter().any(|t| t.value.starts_with("room:") && t.value.contains(room_number)))
        .flat_map(|e| e.common.xdata.iter().filter_map(|t| t.value.strip_prefix("type:").map(str::to_string)).collect::<Vec<_>>())
        .collect()
}

#[test]
fn an_untyped_ceiling_under_a_rated_floor_is_its_membrane() {
    let s = building(json!([]));
    // Level 1 is under the floor/ceiling assembly the legend rates: its untyped ceilings are that
    // assembly's membrane, two layers of Type X, as an assumption with its basis.
    for room in ["101", "102"] {
        let b = ceiling_boards(&s, room);
        assert_eq!(layers(&b), vec![1, 2], "room {room}: the rated membrane's two layers: {b:?}");
        same_area(&b);
        assert!(b.items.iter().all(|i| i.contains("Type X")), "{b:?}");
        let (kind, doc) = b.sources.first().cloned().unwrap_or_default();
        assert_eq!(b.sources.len(), 1, "{b:?}");
        assert_eq!(kind, "rule", "an assumption, marked: {b:?}");
        for w in ["assumed", "FC-1", "floor/ceiling", "1 HR", "alternative"] {
            assert!(doc.contains(w), "the basis says {w:?}: {doc}");
        }
    }
    // Level 2 is the top storey: over it is the roof, and no roof/ceiling assembly is rated in
    // the documents (the floor/ceiling one is not the roof's), so the rule's default: one layer.
    let b = ceiling_boards(&s, "201");
    assert_eq!(layers(&b), vec![1], "{b:?}");
    let (kind, doc) = b.sources.first().cloned().unwrap_or_default();
    assert_eq!(kind, "default", "{b:?}");
    assert!(doc.contains("company default"), "{doc}");
}

#[test]
fn a_rated_roof_ceiling_assembly_is_the_top_storeys_membrane() {
    let gyp = json!({"material": "5/8\" Type C gypsum board", "thickness_in": 0.625, "category": "board"});
    let s = building(json!([{"name": "RC-1", "description": "1 HR ROOF/CEILING ASSEMBLY", "kind": "gypsum", "fire_rating": "1 HR",
                              "layers": [gyp.clone(), gyp.clone(), gyp]}]));
    let b = ceiling_boards(&s, "202");
    assert_eq!(layers(&b), vec![1, 2, 3], "the roof/ceiling membrane's three layers: {b:?}");
    same_area(&b);
    let doc = b.sources.first().map(|x| x.1.clone()).unwrap_or_default();
    assert!(doc.contains("RC-1") && doc.contains("roof/ceiling"), "{doc}");
    // The floor below keeps the floor/ceiling one.
    let b = ceiling_boards(&s, "101");
    assert_eq!(layers(&b), vec![1, 2], "{b:?}");
    assert!(b.sources.first().is_some_and(|x| x.1.contains("FC-1")), "{b:?}");
}

#[test]
fn rcp_type_gives_rooms_their_ceiling_type_and_it_reaches_the_pieces() {
    let mut s = building(json!([]));
    // By room number: room 101 is the plain one-layer ceiling.
    let r = s.execute("rcp.type", &json!({"type": "GB-1", "rooms": ["101"]})).unwrap();
    assert_eq!(r["rooms"].as_array().map(Vec::len), Some(1), "{r}");
    assert_eq!(r["ceilings"].as_u64(), Some(1), "its built ceiling retagged: {r}");
    assert_eq!(ceiling_type_tags(&s, "101"), vec!["GB-1".to_string()]);
    s.execute("bim.atoms.build", &json!({"trades": ["drywall"]})).unwrap();
    let b = ceiling_boards(&s, "101");
    assert_eq!(layers(&b), vec![1], "{b:?}");
    assert!(b.items.iter().all(|i| i.contains("1/2\"")), "{b:?}");
    let (kind, doc) = b.sources.first().cloned().unwrap_or_default();
    assert_eq!(kind, "user", "{b:?}");
    assert!(doc.contains("GB-1"), "{doc}");
    // The other room keeps its rated membrane.
    assert_eq!(layers(&ceiling_boards(&s, "102")), vec![1, 2]);
    // By unit type: the top storey's room of unit type UNIT-A takes the rated type.
    let r = s.execute("rcp.type", &json!({"type": "FC-1", "unit_type": "UNIT-A"})).unwrap();
    assert_eq!(r["rooms"][0]["number"], json!("201"), "{r}");
    s.execute("bim.atoms.build", &json!({"trades": ["drywall"]})).unwrap();
    let b = ceiling_boards(&s, "201");
    assert_eq!(layers(&b), vec![1, 2], "{b:?}");
    same_area(&b);
    // By the basis of the rooms' ceiling heights: the rooms under the floor/ceiling assembly.
    // Room 101 already has a type the command gave it: kept (no force), listed.
    let r = s.execute("rcp.type", &json!({"type": "FC-1", "basis": "floor/ceiling assembly"})).unwrap();
    let nums: Vec<&str> = r["rooms"].as_array().unwrap().iter().filter_map(|x| x["number"].as_str()).collect();
    assert_eq!(nums, vec!["102"], "{r}");
    assert_eq!(r["kept"][0]["number"], json!("101"), "{r}");
    assert_eq!(r["kept"][0]["type"], json!("GB-1"), "{r}");
    // A preview changes nothing.
    let r = s.execute("rcp.type", &json!({"type": "FC-1", "rooms": ["101"], "force": true, "preview": true})).unwrap();
    assert_eq!(r["rooms"].as_array().map(Vec::len), Some(1), "{r}");
    assert_eq!(ceiling_type_tags(&s, "101"), vec!["GB-1".to_string()]);
    // The types are the spec's rooms': a rebuild keeps them.
    s.execute("model.build", &json!({})).unwrap();
    assert_eq!(ceiling_type_tags(&s, "101"), vec!["GB-1".to_string()]);
    assert_eq!(ceiling_type_tags(&s, "102"), vec!["FC-1".to_string()]);
    assert_eq!(ceiling_type_tags(&s, "201"), vec!["FC-1".to_string()]);
    assert!(ceiling_type_tags(&s, "202").is_empty());
    assert_eq!(layers(&ceiling_boards(&s, "101")), vec![1]);
    let b = ceiling_boards(&s, "102");
    assert_eq!(layers(&b), vec![1, 2]);
    assert_eq!(b.sources.first().map(|x| x.0.as_str()), Some("user"), "{b:?}");
    let spec = s.execute("bspec.get", &json!({})).unwrap();
    let text = spec.to_string();
    assert!(text.contains("\"ceiling_type\":\"GB-1\""), "the spec's room keeps its type");
    // Clearing a room's type: back to the rated membrane by assumption.
    let r = s.execute("rcp.type", &json!({"clear": true, "rooms": ["101"]})).unwrap();
    assert_eq!(r["rooms"].as_array().map(Vec::len), Some(1), "{r}");
    assert!(ceiling_type_tags(&s, "101").is_empty());
    s.execute("bim.atoms.build", &json!({"trades": ["drywall"]})).unwrap();
    assert_eq!(layers(&ceiling_boards(&s, "101")), vec![1, 2]);
}

#[test]
fn rcp_type_refuses_what_it_cannot_do() {
    let mut s = building(json!([]));
    for p in [
        json!({"type": "NOPE", "rooms": ["101"]}),
        json!({"type": "GB-1"}),
        json!({"rooms": ["101"]}),
        json!({"type": "GB-1", "rooms": "101"}),
        json!({"type": 7, "rooms": ["101"]}),
        json!({"type": "GB-1", "rooms": [{"a": 1}]}),
    ] {
        assert!(s.execute("rcp.type", &p).is_err(), "{p}");
    }
    // A selector that finds no room is an answer, not an error.
    let r = s.execute("rcp.type", &json!({"type": "GB-1", "rooms": ["999"]})).unwrap();
    assert_eq!(r["rooms"], json!([]), "{r}");
    let long = "X".repeat(100_000);
    let _ = s.execute("rcp.type", &json!({"type": long, "rooms": [long], "unit_type": long, "basis": long}));
    let _: Value = s.execute("rcp.type", &json!({"type": "GB-1", "basis": ""})).unwrap_or_default();
}
