//! Rooms as 3D spaces: from a synthetic plan, the rooms' count, names, numbers, areas, ceilings,
//! volumes and finishes; going to a room; building their volumes; finish schedules; duplicate
//! names; ceiling source.

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

// ---- MUST 1: Finish schedule -----------------------------------------------------------------

/// A plan with a finish schedule table: the schedule's finishes and ceiling height are applied.
fn plan_with_schedule() -> Session {
    let mut s = plan();
    // Add a room finish schedule table above the rooms. The table parser needs:
    // 1. A title text ("ROOM FINISH SCHEDULE")
    // 2. Header cells inside boxes (NO., NAME, FLOOR, BASE, WALL, CEILING, CLG HT)
    // 3. Data rows in the same boxes
    // We draw the table at y=300..420, well above the rooms.

    // Title (not inside a box)
    text(&mut s, [10.0, 420.0], "ROOM FINISH SCHEDULE");

    // Draw header row boxes and text. Each cell is a box with text inside.
    let box_h = 18.0;
    let header_y = 395.0;
    let cols: &[(f64, f64, &str)] = &[
        (10.0, 60.0, "RM NO."),
        (60.0, 160.0, "NAME"),
        (160.0, 220.0, "FLOOR"),
        (220.0, 280.0, "BASE"),
        (280.0, 340.0, "WALL"),
        (340.0, 400.0, "CEILING"),
        (400.0, 470.0, "CLG HT"),
    ];
    s.execute("layer.new", &json!({"name": "SCHED", "current": true})).unwrap();
    for &(x0, x1, hdr) in cols {
        // Box for header
        rect(&mut s, [x0, header_y], [x1, header_y + box_h]);
        text(&mut s, [x0 + 2.0, header_y + 4.0], hdr);
    }
    // Row 1: 101 OFFICE VCT-1/RB-1/PT-1/ACT-1 8'-6"
    let row1_y = header_y - box_h;
    let row1: &[&str] = &["101", "OFFICE", "VCT-1", "RB-1", "PT-1", "ACT-1", "8'-6\""];
    for (i, &(x0, x1, _)) in cols.iter().enumerate() {
        rect(&mut s, [x0, row1_y], [x1, row1_y + box_h]);
        text(&mut s, [x0 + 2.0, row1_y + 4.0], row1[i]);
    }
    // Row 2: 102 STORAGE SC/RB-1/PT-2/EXP 10'-0"
    let row2_y = row1_y - box_h;
    let row2: &[&str] = &["102", "STORAGE", "SC", "RB-1", "PT-2", "EXP", "10'-0\""];
    for (i, &(x0, x1, _)) in cols.iter().enumerate() {
        rect(&mut s, [x0, row2_y], [x1, row2_y + box_h]);
        text(&mut s, [x0 + 2.0, row2_y + 4.0], row2[i]);
    }
    s
}

#[test]
fn finish_schedule_applies_finishes_and_ceiling() {
    let mut s = plan_with_schedule();
    let r = s.execute("rooms.list", &json!({})).unwrap();
    assert_eq!(r["count"], 2, "{r}");
    let rooms = r["rooms"].as_array().unwrap();

    // OFFICE 101: ceiling from schedule = 8'-6" = 8.5 ft = 102 inches.
    let office = room(rooms, "OFFICE");
    let area = 228.0 * 228.0;
    assert!((office["ceilingFt"].as_f64().unwrap() - 8.5).abs() < 1e-9, "schedule ceiling: {office}");
    assert!((office["volume"].as_f64().unwrap() - area * 102.0).abs() < 1.0, "volume = area × 102: {office}");
    // Finishes contain the four schedule codes.
    let fin = office["finishes"].as_array().unwrap();
    assert!(fin.contains(&json!("VCT-1")), "floor finish: {fin:?}");
    assert!(fin.contains(&json!("RB-1")), "base finish: {fin:?}");
    assert!(fin.contains(&json!("PT-1")), "wall finish: {fin:?}");
    assert!(fin.contains(&json!("ACT-1")), "ceiling finish: {fin:?}");
    // finishDetail has the four codes in floor/base/wall/ceiling slots.
    let detail = office["finishDetail"].as_array().unwrap();
    assert!(detail.iter().any(|d| d["slot"] == "floor" && d["code"] == "VCT-1"), "floor slot: {detail:?}");
    assert!(detail.iter().any(|d| d["slot"] == "base" && d["code"] == "RB-1"), "base slot: {detail:?}");
    assert!(detail.iter().any(|d| d["slot"] == "wall" && d["code"] == "PT-1"), "wall slot: {detail:?}");
    assert!(detail.iter().any(|d| d["slot"] == "ceiling" && d["code"] == "ACT-1"), "ceiling slot: {detail:?}");

    // STORAGE 102: schedule says 10'-0" — same as the CLG note, so no change to ceiling.
    let storage = room(rooms, "STORAGE");
    assert!((storage["ceilingFt"].as_f64().unwrap() - 10.0).abs() < 1e-9, "storage ceiling: {storage}");
    // Has both schedule finishes and the in-room CPT-1.
    let sfin = storage["finishes"].as_array().unwrap();
    assert!(sfin.contains(&json!("CPT-1")), "in-room code kept: {sfin:?}");
    assert!(sfin.contains(&json!("SC")), "schedule floor: {sfin:?}");
}

#[test]
fn plan_without_schedule_is_unchanged() {
    // The original plan has no schedule — existing asserts pass.
    let mut s = plan();
    let r = s.execute("rooms.list", &json!({})).unwrap();
    let rooms = r["rooms"].as_array().unwrap();
    let storage = room(rooms, "STORAGE");
    assert_eq!(storage["finishes"], json!(["CPT-1"]), "no schedule: original finishes");
    assert!((storage["ceilingFt"].as_f64().unwrap() - 10.0).abs() < 1e-9, "no schedule: ceiling from note");
    let office = room(rooms, "OFFICE");
    assert!((office["ceiling"].as_f64().unwrap() - 108.0).abs() < 1e-9, "no schedule: default 9'");
}

// ---- MUST 2: Unambiguous room lookup ---------------------------------------------------------

/// A plan with two rooms both named "GUEST ROOM" (201 and 202).
fn plan_duplicate_names() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    rect(&mut s, [0.0, 0.0], [480.0, 240.0]);
    rect(&mut s, [6.0, 6.0], [234.0, 234.0]);
    rect(&mut s, [246.0, 6.0], [474.0, 234.0]);
    s.execute("layer.new", &json!({"name": "A-ANNO", "current": true})).unwrap();
    text(&mut s, [100.0, 120.0], "GUEST ROOM");
    text(&mut s, [100.0, 100.0], "201");
    text(&mut s, [340.0, 120.0], "GUEST ROOM");
    text(&mut s, [340.0, 100.0], "202");
    s
}

#[test]
fn duplicate_name_returns_error_listing_candidates() {
    let mut s = plan_duplicate_names();
    let err = s.execute("rooms.get", &json!({"name": "guest room"}));
    assert!(err.is_err(), "ambiguous name must fail");
    let msg = format!("{}", err.unwrap_err());
    assert!(msg.contains("201"), "error lists 201: {msg}");
    assert!(msg.contains("202"), "error lists 202: {msg}");
}

#[test]
fn number_and_label_select_exact_room() {
    let mut s = plan_duplicate_names();
    // By number.
    let r = s.execute("rooms.get", &json!({"number": "202"})).unwrap();
    assert_eq!(r["number"], "202");
    assert_eq!(r["name"], "GUEST ROOM");
    // By label (get 201's label and use it for goto).
    let r201 = s.execute("rooms.get", &json!({"number": "201"})).unwrap();
    let label = r201["label"].as_str().unwrap();
    let g = s.execute("rooms.goto", &json!({"label": label})).unwrap();
    // The window should be around room 201 (left side: x < 234).
    assert_eq!(g["name"], "GUEST ROOM");
    let w = g["window"].as_array().unwrap();
    // Room 201 is on the left (outline ends at 234); the window centre must be left of midpoint 240.
    let centre_x = (w[0][0].as_f64().unwrap() + w[1][0].as_f64().unwrap()) / 2.0;
    assert!(centre_x < 240.0, "goto label 201 should be centred on the left: {g}");
}

// ---- MUST 3: Ceiling source ------------------------------------------------------------------

#[test]
fn ceiling_source_default_and_note() {
    let mut s = plan();
    let r = s.execute("rooms.list", &json!({})).unwrap();
    let rooms = r["rooms"].as_array().unwrap();
    // OFFICE: no note, no spec → default.
    let office = room(rooms, "OFFICE");
    assert_eq!(office["ceilingSource"], "default", "OFFICE has no ceiling source: {office}");
    // STORAGE: has CLG note → note.
    let storage = room(rooms, "STORAGE");
    assert_eq!(storage["ceilingSource"], "note", "STORAGE has a ceiling note: {storage}");
}

#[test]
fn ceiling_source_spec() {
    let mut s = plan();
    s.doc_mut().unwrap().building_spec =
        Some(json!({"version": 1, "units_per_foot": 12, "rooms": [{"name": "Office", "ceiling_ft": 12}]}).to_string());
    let office = s.execute("rooms.get", &json!({"number": "101"})).unwrap();
    assert_eq!(office["ceilingSource"], "spec", "spec ceiling: {office}");
}

#[test]
fn ceiling_source_schedule() {
    let mut s = plan_with_schedule();
    let office = s.execute("rooms.get", &json!({"number": "101"})).unwrap();
    assert_eq!(office["ceilingSource"], "schedule", "schedule ceiling: {office}");
}

#[test]
fn finish_detail_carries_source() {
    let mut s = plan();
    let r = s.execute("rooms.list", &json!({})).unwrap();
    let rooms = r["rooms"].as_array().unwrap();
    let storage = room(rooms, "STORAGE");
    let detail = storage["finishDetail"].as_array().unwrap();
    // CPT-1 from the in-room text note.
    assert!(detail.iter().any(|d| d["code"] == "CPT-1" && d["source"] == "note"), "CPT-1 source is note: {detail:?}");
    // With a schedule, schedule finishes carry source "schedule".
    let mut s2 = plan_with_schedule();
    let office = s2.execute("rooms.get", &json!({"number": "101"})).unwrap();
    let od = office["finishDetail"].as_array().unwrap();
    assert!(od.iter().any(|d| d["slot"] == "floor" && d["source"] == "schedule"), "schedule finish source: {od:?}");
}

/// A guest room as the hotel's mirrored units print it: the name's insertion point lands on a
/// desk's outline, and the number lies on the bed drawn against the headboard wall (its outline
/// cuts into the room's). The room is still traced and numbered.
#[test]
fn a_room_is_traced_past_the_furniture_under_its_name_and_numbered_off_its_bed() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    rect(&mut s, [0.0, 0.0], [252.0, 252.0]);
    rect(&mut s, [6.0, 6.0], [246.0, 246.0]);
    s.execute("layer.new", &json!({"name": "A-FURN", "current": true})).unwrap();
    // A desk (2' x 2') under the name's start; a bed against the bottom wall: its outline
    // touches the wall, so the room's traced boundary goes round it.
    rect(&mut s, [96.0, 150.0], [120.0, 174.0]);
    s.execute("pline", &json!({"vertices": [[60.0, 6.0], [60.0, 150.0], [180.0, 150.0], [180.0, 6.0]], "closed": false})).unwrap();
    s.execute("layer.new", &json!({"name": "A-ANNO", "current": true})).unwrap();
    text(&mut s, [100.0, 160.0], "DOUBLE Q");
    text(&mut s, [110.0, 145.0], "205");
    let r = s.execute("rooms.list", &json!({})).unwrap();
    let rooms = r["rooms"].as_array().unwrap();
    let q = room(rooms, "DOUBLE Q");
    assert_eq!(q["number"], "205", "{r}");
    assert!(q["areaSqFt"].as_f64().unwrap() > 200.0, "the room, not the desk: {q}");
}
