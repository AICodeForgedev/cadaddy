//! Tags are identities (Q6004): the same door drawn on the architectural plan and on two
//! discipline backgrounds is one door; the same number far apart on one level is a finding; a
//! room closed only by a door's swing is a room; each door knows the rooms on its two sides.
//! Made-up sheets only.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::elemprops::element_kind;
use cadaddy_engine::doc::props;
use cadaddy_engine::doc::{Arc, Common, Drawing, Entity, EntityKind, Handle, Line, Mesh, MeshFace, MeshKind, RawTag, Space, Text};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn add(d: &mut Drawing, layer: &str, kind: EntityKind) -> Handle {
    d.ensure_layer(layer);
    d.add(&Space::Model, Common { layer: layer.into(), ..Common::default() }, kind).expect("added")
}

fn line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    add(d, "PEN-1", EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) }));
}

fn text(d: &mut Drawing, v: &str, at: (f64, f64)) {
    let t = Text {
        insert: Vec3::new(at.0, at.1, 0.0),
        align_pt: None,
        height: 6.0,
        value: v.into(),
        rotation: 0.0,
        width_factor: 1.0,
        oblique: 0.0,
        style: "Standard".into(),
        halign: cadaddy_engine::doc::HAlign::default(),
        valign: cadaddy_engine::doc::VAlign::default(),
    };
    add(d, "PEN-TEXT", EntityKind::Text(t));
}

/// A door as the build makes one: a box `w` × `h` (plan) on the 3D-Doors layer with the build's
/// tag `[door, level, source]`.
fn door(d: &mut Drawing, c: (f64, f64), w: f64, h: f64, level: &str) -> Handle {
    let (x0, y0, x1, y1) = (c.0 - w / 2.0, c.1 - h / 2.0, c.0 + w / 2.0, c.1 + h / 2.0);
    let mut vertices = Vec::new();
    for z in [0.0, 84.0] {
        vertices.extend([Vec3::new(x0, y0, z), Vec3::new(x1, y0, z), Vec3::new(x1, y1, z), Vec3::new(x0, y1, z)]);
    }
    let faces = [[0, 1, 2, 3], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]]
        .iter()
        .map(|f| MeshFace { v: f.to_vec(), hidden: 0 })
        .collect();
    let h = add(d, "3D-Doors", EntityKind::Mesh(Mesh { vertices, faces, kind: MeshKind::Polyface }));
    let mut e: Entity = (**d.entity(h).expect("door")).clone();
    e.common.xdata = vec![
        RawTag { code: 1001, value: "CADADDY_WALLGEN".into() },
        RawTag { code: 1000, value: "door".into() },
        RawTag { code: 1000, value: level.into() },
        RawTag { code: 1000, value: "0".into() },
    ]
    .into();
    d.model.remove(h);
    d.model.push(e);
    h
}

fn doors(s: &Session) -> Vec<Handle> {
    s.doc().unwrap().model.iter().filter(|e| element_kind(e).as_deref() == Some("door")).map(|e| e.handle).collect()
}

/// Three plan sheets of Level 1 side by side, 1000 wide: A-101 at x 0, E-101 at x 2000 (the same
/// plan), M-101 at x 4000 drawn 3' further right on its sheet. Each prints the four room numbers;
/// door 101A sits at (300, 200) of the plan with its number lettered beside it on every sheet.
fn three_sheets() -> Session {
    let mut d = Drawing::new_imperial();
    let rooms = [("101", (200.0, 300.0)), ("102", (600.0, 300.0)), ("103", (200.0, 600.0)), ("104", (600.0, 600.0))];
    for (x0, shift) in [(0.0, 0.0), (2000.0, 0.0), (4000.0, 36.0)] {
        for (n, (x, y)) in rooms {
            text(&mut d, n, (x0 + shift + x, y));
        }
        door(&mut d, (x0 + shift + 300.0, 200.0), 36.0, 6.0, "Level 1");
        text(&mut d, "101A", (x0 + shift + 300.0, 215.0));
    }
    // Door 105 twice on the architectural plan, far apart: a print error, not one door.
    door(&mut d, (100.0, 700.0), 36.0, 6.0, "Level 1");
    text(&mut d, "105", (100.0, 715.0));
    door(&mut d, (800.0, 100.0), 36.0, 6.0, "Level 1");
    text(&mut d, "105", (800.0, 115.0));
    let sheet = |n: &str, x0: f64| json!({"number": n, "discipline": &n[..1], "level": "Level 1", "rect": [x0, 0.0, x0 + 1000.0, 800.0]});
    let spec = json!({
        "units_per_foot": 12.0,
        "sheets": [sheet("A-101", 0.0), sheet("E-101", 2000.0), sheet("M-101", 4000.0)],
    });
    d.building_spec = Some(spec.to_string());
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Set", None);
    s
}

#[test]
fn the_same_door_on_the_a_e_and_m_sheets_is_one_door() {
    let mut s = three_sheets();
    assert_eq!(doors(&s).len(), 5);
    // A report first: nothing changes.
    let r = s.execute("bim.identity", &json!({"apply": false})).unwrap();
    assert_eq!(doors(&s).len(), 5, "{r}");
    let sheets = r["sheets"].as_array().unwrap();
    let method = |n: &str| sheets.iter().find(|t| t["sheet"] == n).map(|t| t["method"].clone());
    assert_eq!(method("A-101"), Some(json!("reference")), "{r:#}");
    assert_eq!(method("E-101"), Some(json!("tags")), "{r:#}");
    assert_eq!(method("M-101"), Some(json!("tags")), "registered by its tags, not its frame (it is shifted): {r:#}");
    let m = sheets.iter().find(|t| t["sheet"] == "M-101").unwrap();
    assert!(m["residual"].as_f64().unwrap() < 1.0, "under an inch: {m}");
    assert_eq!(r["numbered_doors"], 5);
    assert_eq!(r["merged"], 2, "{r:#}");

    let r = s.execute("bim.identity", &json!({})).unwrap();
    assert_eq!(r["by_kind"]["door"], json!({"before": 5, "after": 3}), "{r:#}");
    // Each sheet's transform onto the level's plan is kept on the spec's sheet.
    let spec: Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    let m = spec["sheets"].as_array().unwrap().iter().find(|x| x["number"] == "M-101").cloned().unwrap();
    assert_eq!(m["registration"]["method"], "tags", "{m}");
    assert_eq!(m["registration"]["reference"], "A-101");
    assert!((m["registration"]["tx"].as_f64().unwrap() + 4036.0).abs() < 1.0, "{m}");
    let left = doors(&s);
    assert_eq!(left.len(), 3);
    // The architectural plan's copy is kept, lists every sheet it was seen on and has its number.
    let d = s.doc().unwrap();
    let kept = left
        .iter()
        .map(|h| props::read(&d.entity(*h).unwrap().common.xdata))
        .find(|p| p.get("number").map(|v| v.text()) == Some("101A".into()))
        .expect("door 101A");
    assert_eq!(kept.get("seen_on").map(|v| v.text()), Some("A-101, E-101, M-101".into()));
    assert_eq!(kept.get("number").map(|v| v.source.kind), Some(props::SourceKind::Print));
    assert_eq!(kept.get("number").and_then(|v| v.source.doc.clone()), Some("A-101".into()));
    // Door 105 in two places on one level: both stay, and it is a finding.
    let apart: Vec<&Value> = r["findings"].as_array().unwrap().iter().filter(|f| f["kind"] == "same_tag_apart").collect();
    assert_eq!(apart.len(), 1, "{r:#}");
    assert_eq!(apart[0]["key"], "105");
    assert_eq!(apart[0]["places"].as_array().unwrap().len(), 2);
    // Running it again changes nothing.
    let again = s.execute("bim.identity", &json!({})).unwrap();
    assert_eq!(again["merged"], 0, "{again:#}");
    assert_eq!(doors(&s).len(), 3);
}

/// Two rooms 200 × 200 side by side; the wall between them has a 36" opening whose door is
/// drawn as its swing's arc alone (no leaf line), so only the swing closes the left room.
fn rooms_with_a_door_gap() -> Drawing {
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0.0, 0.0), (400.0, 0.0)), ((400.0, 0.0), (400.0, 200.0)), ((400.0, 200.0), (0.0, 200.0)), ((0.0, 200.0), (0.0, 0.0))] {
        line(&mut d, a, b);
    }
    line(&mut d, (200.0, 0.0), (200.0, 60.0));
    line(&mut d, (200.0, 96.0), (200.0, 200.0));
    // Hinge at the opening's foot, the arc from the closed leaf (up the wall) to the open one.
    let arc = Arc { center: Vec3::new(200.0, 60.0, 0.0), radius: 36.0, start: std::f64::consts::FRAC_PI_2, end: std::f64::consts::PI };
    add(&mut d, "PEN-1", EntityKind::Arc(arc));
    text(&mut d, "OFFICE", (80.0, 150.0));
    text(&mut d, "101", (80.0, 130.0));
    text(&mut d, "STORAGE", (280.0, 150.0));
    text(&mut d, "102", (280.0, 130.0));
    d
}

#[test]
fn a_room_closed_only_by_a_door_swing_is_a_room() {
    let mut s = Session::empty();
    s.open_drawing(rooms_with_a_door_gap(), "Sample Floor Plan", None);
    let r = s.execute("plan.rooms", &json!({})).unwrap();
    let items = r["items"].as_array().cloned().unwrap_or_default();
    let names: Vec<&str> = items.iter().filter_map(|i| i["name"].as_str()).collect();
    assert!(names.contains(&"OFFICE") && names.contains(&"STORAGE"), "both rooms, each its own outline: {r:#}");
    for i in &items {
        // Each its own 200 × 200: no room runs on through the door, and the office keeps the
        // quarter circle its door swings through (the closed leaf is the boundary).
        let a = i["area"].as_f64().unwrap();
        assert!((a - 200.0 * 200.0).abs() < 1.0, "{i}");
    }
}

#[test]
fn each_door_knows_the_rooms_on_its_two_sides() {
    let mut d = rooms_with_a_door_gap();
    let h = door(&mut d, (200.0, 78.0), 6.0, 36.0, "");
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let g = s.execute("rooms.graph", &json!({})).unwrap();
    assert_eq!(g["connected"], 1, "{g:#}");
    let edges = g["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 1, "{g:#}");
    let mut pair = [edges[0]["from"].as_str().unwrap().to_string(), edges[0]["to"].as_str().unwrap().to_string()];
    pair.sort();
    assert_eq!(pair, ["101".to_string(), "102".to_string()]);
    let p = props::read(&s.doc().unwrap().entity(h).unwrap().common.xdata);
    assert_eq!(p.get("from_room").map(|v| v.text()), Some("101".into()));
    assert_eq!(p.get("to_room").map(|v| v.text()), Some("102".into()));
    assert_eq!(p.get("to_room").map(|v| v.source.kind), Some(props::SourceKind::Computed));
    // bim.props.get shows them on the door.
    let got = s.execute("bim.props.get", &json!({"handle": h.hex()})).unwrap();
    assert_eq!(got["elements"][0]["props"]["from_room"]["value"], "101", "{got:#}");
}

#[test]
fn hostile_identity_calls_never_panic() {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Empty", None);
    for p in [json!({}), json!({"apply": "yes"}), json!({"room_numbers": 5}), json!({"room_numbers": [1, null, "101"]})] {
        let _ = s.execute("bim.identity", &p);
    }
    let r = s.execute("bim.identity", &json!({})).unwrap();
    assert_eq!(r["merged"], 0);
    let g = s.execute("rooms.graph", &json!({"set": false})).unwrap();
    assert_eq!(g["connected"], 0);
    // A spec that isn't one, and sheets with no rectangle, are no sheets.
    let mut d = Drawing::new_imperial();
    d.building_spec = Some("{not json".into());
    s.open_drawing(d, "Broken", None);
    assert!(s.execute("bim.identity", &json!({})).is_ok());
}
