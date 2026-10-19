//! Rooms as 3D spaces: from a synthetic plan, the rooms' count, names, numbers, areas, ceilings,
//! volumes and finishes; going to a room; building their volumes.

use cadaddy_doc::EntityKind;
use cadaddy_engine::Session;
use serde_json::{Value, json};

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).unwrap();
}

/// Two 19' × 19' rooms (inches) inside 6" walls: OFFICE 101 (no ceiling note) and STORAGE 102
/// with a 10'-0" ceiling note and a carpet finish code.
fn plan() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    rect(&mut s, [0.0, 0.0], [480.0, 240.0]);
    rect(&mut s, [6.0, 6.0], [234.0, 234.0]);
    rect(&mut s, [246.0, 6.0], [474.0, 234.0]);
    s.execute("layer.new", &json!({"name": "A-ANNO", "current": true})).unwrap();
    text(&mut s, [100.0, 120.0], "OFFICE");
    text(&mut s, [100.0, 100.0], "101");
    text(&mut s, [340.0, 120.0], "STORAGE");
    text(&mut s, [340.0, 100.0], "102");
    text(&mut s, [300.0, 60.0], "CLG 10'-0\"");
    text(&mut s, [300.0, 40.0], "FLOOR: CPT-1");
    s
}

fn room<'a>(rooms: &'a [Value], name: &str) -> &'a Value {
    rooms.iter().find(|r| r["name"] == name).unwrap_or_else(|| panic!("no {name} in {rooms:?}"))
}

#[test]
fn rooms_get_their_areas_ceilings_volumes_and_finishes() {
    let mut s = plan();
    let r = s.execute("rooms.list", &json!({})).unwrap();
    assert_eq!(r["count"], 2, "{r}");
    let rooms = r["rooms"].as_array().unwrap();
    let office = room(rooms, "OFFICE");
    assert_eq!(office["number"], "101");
    let area = 228.0 * 228.0;
    assert!((office["area"].as_f64().unwrap() - area).abs() < 1e-3);
    assert!((office["areaSqFt"].as_f64().unwrap() - area / 144.0).abs() < 0.01);
    // No note, no spec: 9'-0".
    assert!((office["ceiling"].as_f64().unwrap() - 108.0).abs() < 1e-9);
    assert!((office["volume"].as_f64().unwrap() - area * 108.0).abs() < 1.0);
    let storage = room(rooms, "STORAGE");
    assert!((storage["ceilingFt"].as_f64().unwrap() - 10.0).abs() < 1e-9, "the ceiling note: {storage}");
    assert_eq!(storage["finishes"], json!(["CPT-1"]));
    assert!(storage["outline"].as_array().unwrap().len() >= 4);
    // The building spec's ceiling for a room wins.
    s.doc_mut().unwrap().building_spec =
        Some(json!({"version": 1, "units_per_foot": 12, "rooms": [{"name": "Office", "ceiling_ft": 12, "materials": {"floor": "Oak"}}]}).to_string());
    let office = s.execute("rooms.get", &json!({"number": "101"})).unwrap();
    assert!((office["ceilingFt"].as_f64().unwrap() - 12.0).abs() < 1e-9, "{office}");
    assert_eq!(office["materials"]["floor"], "Oak");
    assert!(s.execute("rooms.get", &json!({"name": "attic"})).is_err());
    assert!(s.execute("rooms.get", &json!({})).is_err());
}

#[test]
fn going_to_a_room_and_building_its_volume() {
    let mut s = plan();
    let r = s.execute("rooms.goto", &json!({"name": "storage"})).unwrap();
    let w = r["window"].as_array().unwrap();
    assert!(w[0][0].as_f64().unwrap() < 246.0 && w[1][0].as_f64().unwrap() > 474.0, "{r}");
    let r = s.execute("rooms.build", &json!({})).unwrap();
    assert_eq!(r["built"], 2);
    let vols: Vec<_> = s.doc().unwrap().model.iter().filter(|e| e.common.layer == "3D-Rooms").cloned().collect();
    assert_eq!(vols.len(), 2);
    for v in &vols {
        assert!(cadaddy_engine::cmd::rooms3d::room_of(&v.common).is_some());
        let EntityKind::Mesh(m) = &v.kind else { panic!("not a mesh") };
        let top = m.vertices.iter().map(|p| p.z).fold(f64::MIN, f64::max);
        assert!(top > 100.0, "up to the ceiling");
        assert_eq!(m.faces.len(), 6, "floor, ceiling and four walls");
    }
    // Again: replaced, not doubled; clear removes them.
    let r = s.execute("rooms.build", &json!({})).unwrap();
    assert_eq!((r["built"].clone(), r["removed"].clone()), (json!(2), json!(2)));
    let r = s.execute("rooms.build", &json!({"clear": true})).unwrap();
    assert_eq!(r["removed"], 2);
    assert!(s.doc().unwrap().model.iter().all(|e| e.common.layer != "3D-Rooms"));
    // An empty drawing has no rooms.
    let mut e = Session::new();
    assert_eq!(e.execute("rooms.list", &json!({})).unwrap()["count"], 0);
}
