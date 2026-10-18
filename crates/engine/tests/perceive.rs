//! Perception tools for agents on synthetic plans with known answers: wall pairs and their
//! thickness, door swings and window blocks, room outlines with their names and numbers, notes
//! (ceiling heights, scales, finish codes, levels), sheets from title blocks and text, layer
//! summaries with PNG crops, and paging.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).unwrap();
}

/// Two 19' × 19' rooms (OFFICE 101, LOBBY 102) inside 6" outer walls, a 12" wall between them.
fn plan() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    rect(&mut s, [0.0, 0.0], [480.0, 240.0]);
    rect(&mut s, [6.0, 6.0], [234.0, 234.0]);
    rect(&mut s, [246.0, 6.0], [474.0, 234.0]);
    s.execute("layer.new", &json!({"name": "A-ANNO", "current": true})).unwrap();
    text(&mut s, [100.0, 120.0], "OFFICE");
    text(&mut s, [100.0, 100.0], "101");
    text(&mut s, [340.0, 120.0], "LOBBY");
    text(&mut s, [340.0, 100.0], "102");
    s
}

fn items(r: &Value) -> Vec<Value> {
    r["items"].as_array().unwrap().clone()
}

#[test]
fn rooms_are_the_outlines_around_their_names_with_their_numbers() {
    let mut s = plan();
    let r = s.execute("plan.rooms", &json!({})).unwrap();
    let rooms = items(&r);
    assert_eq!(rooms.len(), 2, "{r}");
    let office = rooms.iter().find(|x| x["name"] == "OFFICE").unwrap();
    assert_eq!(office["number"], "101");
    assert!((office["area"].as_f64().unwrap() - 228.0 * 228.0).abs() < 1.0, "{office}");
    assert!((office["perimeter"].as_f64().unwrap() - 4.0 * 228.0).abs() < 1.0);
    assert!(office["confidence"].as_f64().unwrap() >= 0.9);
    assert!(office["outline"].as_array().unwrap().len() >= 4);
    let lobby = rooms.iter().find(|x| x["name"] == "LOBBY").unwrap();
    assert_eq!(lobby["number"], "102");
    // Without outlines (cheaper), paged one at a time.
    let one = s.execute("plan.rooms", &json!({"outline": false, "limit": 1})).unwrap();
    assert_eq!((one["items"].as_array().unwrap().len(), one["total"].as_u64(), one["next"].as_u64()), (1, Some(2), Some(1)));
    assert!(one["items"][0].get("outline").is_none());
}

#[test]
fn walls_are_parallel_face_pairs_a_wall_apart() {
    let mut s = plan();
    let r = s.execute("plan.walls", &json!({"limit": 200})).unwrap();
    let walls = items(&r);
    let near = |t: f64, l: f64| walls.iter().any(|w| (w["thickness"].as_f64().unwrap() - t).abs() < 1e-6 && w["length"].as_f64().unwrap() >= l);
    assert!(near(6.0, 200.0), "the 6\" outer walls: {walls:?}");
    assert!(near(12.0, 228.0), "the 12\" wall between the rooms");
    // All on A-WALL, centrelines between the faces.
    assert!(walls.iter().all(|w| w["layer"] == "A-WALL"));
    let mid = walls.iter().find(|w| (w["thickness"].as_f64().unwrap() - 12.0).abs() < 1e-6).unwrap();
    assert!((mid["centerline"][0][0].as_f64().unwrap() - 240.0).abs() < 1e-6, "{mid}");
    // Limits: nothing as thin as 1".
    let thin = s.execute("plan.walls", &json!({"minThickness": 0.1, "maxThickness": 1})).unwrap();
    assert_eq!(thin["total"], 0);
    // Only the layers asked for.
    assert_eq!(s.execute("plan.walls", &json!({"layers": "A-ANNO"})).unwrap()["total"], 0);
}

#[test]
fn openings_are_door_swings_and_door_or_window_blocks() {
    let mut s = Session::new();
    // A 36" door hinged at (100, 0): leaf up to (100, 36), swing to (136, 0).
    s.execute("arc", &json!({"center": [100, 0], "radius": 36, "start": 0, "end": 90})).unwrap();
    line(&mut s, [100.0, 0.0], [100.0, 36.0]);
    // A small arc (a detail) is not a door.
    s.execute("arc", &json!({"center": [0, 0], "radius": 4, "start": 0, "end": 90})).unwrap();
    // A window block.
    let w = s.execute("line", &json!({"points": [[0, 0], [36, 0]]})).unwrap();
    let hs = vec![w["handles"][0].as_str().unwrap().to_string()];
    s.execute("block", &json!({"name": "WINDOW-36", "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    s.execute("insert", &json!({"name": "WINDOW-36", "at": [300, 0]})).unwrap();
    let r = s.execute("plan.openings", &json!({})).unwrap();
    let o = items(&r);
    assert_eq!(o.len(), 2, "{r}");
    let door = o.iter().find(|x| x["kind"] == "door").unwrap();
    assert_eq!((door["width"].as_f64(), door["swing"].as_f64()), (Some(36.0), Some(90.0)));
    assert!(door["confidence"].as_f64().unwrap() >= 0.8, "the leaf is there: {door}");
    let win = o.iter().find(|x| x["kind"] == "window").unwrap();
    assert_eq!((win["width"].as_f64(), win["at"][0].as_f64()), (Some(36.0), Some(300.0)));
}

#[test]
fn notes_find_ceilings_scales_finishes_and_levels() {
    let mut s = Session::new();
    for (i, t) in
        ["CLG 9'-0\" AFF", "SCALE: 1/4\" = 1'-0\"", "FLOOR: CPT-1, BASE RB-2", "LEVEL 2 EL. 112'-0\"", "SEE A-101", "NOT TO SCALE"].iter().enumerate()
    {
        text(&mut s, [0.0, 20.0 * i as f64], t);
    }
    let r = s.execute("plan.notes", &json!({"limit": 50})).unwrap();
    let n = items(&r);
    let has = |k: &str, v: Value| n.iter().any(|x| x["kind"] == k && x["value"] == v);
    assert!(has("ceiling", json!(108.0)), "{n:?}");
    assert!(has("scale", json!("1/4\" = 1'-0\"")));
    assert!(has("scale", json!("NTS")));
    assert!(has("finish", json!("CPT-1")) && has("finish", json!("RB-2")));
    assert!(has("level", json!(1344.0)));
    assert!(!n.iter().any(|x| x["value"] == "A-101"), "a sheet reference is not a finish code");
    assert!(s.execute("plan.notes", &json!({"kind": "finish"})).unwrap()["items"].as_array().unwrap().iter().all(|x| x["kind"] == "finish"));
}

#[test]
fn sheets_come_from_title_blocks_and_sheet_number_text() {
    let mut s = Session::new();
    // A title block with attributes, inserted on Layout1.
    s.execute("attdef", &json!({"tag": "SHEET_NO", "default": "", "at": [0, 0], "height": 0.2})).unwrap();
    s.execute("attdef", &json!({"tag": "TITLE", "default": "", "at": [0, 1], "height": 0.2})).unwrap();
    rect(&mut s, [0.0, 0.0], [10.0, 2.0]);
    let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
    s.execute("block", &json!({"name": "TB", "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    s.execute("layout.set", &json!({"name": "Layout1"})).ok();
    s.cmdline("layout s Layout1").ok();
    s.execute("insert", &json!({"name": "TB", "at": [0, 0], "attribs": {"SHEET_NO": "A101", "TITLE": "FLOOR PLAN"}})).unwrap();
    let r = s.execute("plan.sheets", &json!({})).unwrap();
    let sh = items(&r);
    let tb = sh.iter().find(|x| x["number"] == "A101").unwrap_or_else(|| panic!("{r}"));
    assert_eq!((tb["title"].as_str(), tb["from"].as_str()), (Some("FLOOR PLAN"), Some("attribute")));
    assert!(tb["confidence"].as_f64().unwrap() >= 0.9);
}

#[test]
fn layers_summarise_counts_pairs_and_write_crops() {
    let mut s = plan();
    let dir = std::env::temp_dir().join(format!("cadaddy-perceive-{}", std::process::id()));
    let r = s.execute("plan.layers", &json!({"crops": dir.to_str().unwrap()})).unwrap();
    let l = items(&r);
    let wall = l.iter().find(|x| x["layer"] == "A-WALL").unwrap();
    assert_eq!(wall["counts"]["Polyline"], 3);
    assert_eq!(wall["segments"], 12);
    assert!(wall["pairs"].as_u64().unwrap() > 0 && wall["pairSpacing"].as_f64().is_some(), "{wall}");
    let crop = wall["crop"].as_str().unwrap();
    let png = std::fs::read(crop).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    let anno = l.iter().find(|x| x["layer"] == "A-ANNO").unwrap();
    assert_eq!(anno["counts"]["Text"], 4);
    let _ = std::fs::remove_dir_all(&dir);
    // Hostile paging.
    let r = s.execute("plan.layers", &json!({"offset": 1_000_000, "limit": 0})).unwrap();
    assert_eq!(r["items"].as_array().unwrap().len(), 0);
}
