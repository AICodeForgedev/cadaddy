//! `bspec.draft` on synthetic plans in two layer conventions (US National CAD Standard names in
//! inches; plain names in millimetres, with centreline walls and an unnamed layer of wall faces):
//! the drafted spec has the right layer roles, wall types, rooms, ceilings and families, marks
//! what it guessed, and builds.

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

fn text(s: &mut Session, at: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": at, "text": t, "height": h})).unwrap();
}

/// A block of one line `w` long, inserted at `at` on the current layer.
fn block(s: &mut Session, name: &str, w: f64, at: [f64; 2]) {
    if s.doc().unwrap().block(name).is_none() {
        let before: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).collect();
        line(s, [-w / 2.0, 0.0], [w / 2.0, 0.0]);
        let hs: Vec<String> = s.doc().unwrap().model.iter().map(|e| e.handle.hex()).filter(|h| !before.contains(h)).collect();
        s.execute("block", &json!({"name": name, "base": [0, 0], "handles": hs, "keep": "delete"})).unwrap();
    }
    s.execute("insert", &json!({"name": name, "at": at})).unwrap();
}

/// NCS names, inches: OFFICE 101 (CLG 9'-0") and LOBBY 102 inside 6" walls with a 12" wall
/// between; a door block and swing, a window block, a light, dimensions.
fn ncs() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [480.0, 240.0]);
    rect(&mut s, [6.0, 6.0], [234.0, 234.0]);
    rect(&mut s, [246.0, 6.0], [474.0, 234.0]);
    layer(&mut s, "A-DOOR");
    block(&mut s, "DOOR-36", 36.0, [120.0, 3.0]);
    layer(&mut s, "A-GLAZ");
    block(&mut s, "WIN-48", 48.0, [360.0, 3.0]);
    layer(&mut s, "A-AREA-IDEN");
    text(&mut s, [100.0, 120.0], "OFFICE", 6.0);
    text(&mut s, [100.0, 100.0], "101", 6.0);
    text(&mut s, [340.0, 120.0], "LOBBY", 6.0);
    text(&mut s, [340.0, 100.0], "102", 6.0);
    layer(&mut s, "A-ANNO-NOTE");
    text(&mut s, [100.0, 60.0], "CLG 9'-0\"", 4.0);
    layer(&mut s, "E-LITE");
    block(&mut s, "LT-2X4", 24.0, [60.0, 180.0]);
    layer(&mut s, "A-ANNO-DIMS");
    line(&mut s, [0.0, -24.0], [480.0, -24.0]);
    s
}

/// Plain names, millimetres: centreline walls on "Walls" (a 6 m × 4 m flat split into KITCHEN
/// and BEDROOM), a pair of faces 100 mm apart on an unnamed layer, door swings on "Doors",
/// "Room Names", "Lighting".
fn plain() -> Session {
    let mut s = Session::new();
    s.execute("setvar", &json!({"name": "INSUNITS", "value": 4})).unwrap();
    layer(&mut s, "Walls");
    rect(&mut s, [0.0, 0.0], [6000.0, 4000.0]);
    line(&mut s, [3000.0, 0.0], [3000.0, 4000.0]);
    layer(&mut s, "P1");
    line(&mut s, [7000.0, 0.0], [7000.0, 3000.0]);
    line(&mut s, [7100.0, 0.0], [7100.0, 3000.0]);
    line(&mut s, [8000.0, 0.0], [8000.0, 3000.0]);
    line(&mut s, [8100.0, 0.0], [8100.0, 3000.0]);
    layer(&mut s, "Doors");
    s.execute("arc", &json!({"center": [1000.0, 0.0], "radius": 900.0, "start": 0.0, "end": 90.0})).unwrap();
    line(&mut s, [1000.0, 0.0], [1000.0, 900.0]);
    layer(&mut s, "Room Names");
    text(&mut s, [1500.0, 2000.0], "KITCHEN", 150.0);
    text(&mut s, [4500.0, 2000.0], "BEDROOM", 150.0);
    layer(&mut s, "Lighting");
    s.execute("circle", &json!({"center": [1500.0, 3000.0], "radius": 150.0})).unwrap();
    s
}

fn role<'a>(spec: &'a Value, layer: &str) -> Option<&'a Value> {
    spec["layers"].as_array().unwrap().iter().find(|l| l["layer"] == layer)
}

fn guessed(r: &Value, key: &str) -> bool {
    r["guesses"].as_array().unwrap().iter().any(|g| g["key"] == key)
}

#[test]
fn ncs_names_draft_the_spec() {
    let mut s = ncs();
    let r = s.execute("bspec.draft", &json!({})).unwrap();
    let sp = &r["spec"];
    assert_eq!(r["stored"], json!(false));
    assert_eq!(s.execute("bspec.get", &json!({})).unwrap()["stored"], json!(false), "not stored unless asked");
    let roles: Vec<(String, String)> =
        sp["layers"].as_array().unwrap().iter().map(|l| (l["layer"].as_str().unwrap().into(), l["role"].as_str().unwrap().into())).collect();
    assert_eq!(
        roles,
        [("A-WALL", "wall"), ("A-DOOR", "door"), ("A-GLAZ", "window"), ("A-AREA-IDEN", "room_tag"), ("E-LITE", "fixture")]
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .to_vec(),
        "{sp}"
    );
    // The wall type is measured: the 6" faces (more of them than the 12" wall's).
    assert_eq!(role(sp, "A-WALL").unwrap()["wall_type"], json!("6\" walls"));
    assert_eq!(sp["wall_types"], json!([{"name": "6\" walls", "thickness_in": 6.0, "from": "pairs"}]));
    assert!(!guessed(&r, "wall_types/6\" walls"));
    // Rooms with their numbers; the office's ceiling from its note, the lobby's guessed.
    let rooms = sp["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 2, "{sp}");
    let office = rooms.iter().find(|x| x["name"] == "Office").unwrap();
    assert_eq!((office["number"].clone(), office["ceiling_ft"].clone()), (json!("101"), json!(9.0)));
    let lobby = rooms.iter().find(|x| x["name"] == "Lobby").unwrap();
    assert_eq!((lobby["number"].clone(), lobby.get("ceiling_ft").cloned()), (json!("102"), None));
    assert!(guessed(&r, "rooms/Lobby") && !guessed(&r, "rooms/Office"));
    // The level's ceiling from the notes; floor to floor assumed.
    assert_eq!(sp["levels"][0]["ceiling_ft"], json!(9.0));
    assert_eq!(sp["units_per_foot"], json!(12.0));
    assert!(guessed(&r, "levels/Level 1") && guessed(&r, "layers/A-WALL") && guessed(&r, "openings"));
    // Families from the blocks.
    let fam: Vec<(String, String)> =
        sp["families"].as_array().unwrap().iter().map(|f| (f["block"].as_str().unwrap().into(), f["kind"].as_str().unwrap().into())).collect();
    assert_eq!(fam, vec![("DOOR-36".to_string(), "door".to_string()), ("WIN-48".to_string(), "window".to_string())]);
    // The parts say what is a guess.
    let wall = r["parts"].as_array().unwrap().iter().find(|p| p["key"] == "layers/A-WALL").unwrap();
    assert_eq!(wall["text"], json!("A-WALL = full height walls 6\" thick (line pairs)"));
    assert_eq!(wall["guess"], json!("NCS layer name (A-WALL)"));
    // Stored, it validates and builds.
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    let got = s.execute("bspec.get", &json!({})).unwrap();
    assert_eq!(got["stored"], json!(true));
    assert!(got["parts"].as_array().unwrap().iter().any(|p| p.get("guess").is_some()));
    let v = s.execute("bspec.validate", &json!({})).unwrap();
    assert_eq!(v["ok"], json!(true), "{v}");
    let b = s.execute("model.build", &json!({})).unwrap();
    assert!(b["walls"].as_u64().unwrap() >= 4 && b["rooms"] == json!(2) && b["windows"] == json!(1), "{b}");
    // Undo takes the stored draft back (and the build first).
    s.undo().unwrap();
    s.undo().unwrap();
    assert_eq!(s.execute("bspec.get", &json!({})).unwrap()["stored"], json!(false));
}

#[test]
fn plain_names_in_millimetres_draft_the_spec() {
    let mut s = plain();
    let r = s.execute("bspec.draft", &json!({})).unwrap();
    let sp = &r["spec"];
    assert_eq!(sp["units_per_foot"], json!(304.8));
    assert_eq!(role(sp, "Walls").unwrap()["role"], json!("wall"));
    assert_eq!(role(sp, "Doors").unwrap()["role"], json!("door"));
    assert_eq!(role(sp, "Room Names").unwrap()["role"], json!("room_tag"));
    assert_eq!(role(sp, "Lighting").unwrap()["role"], json!("fixture"));
    // No face pairs on Walls: centrelines, the thickness guessed.
    let walls_type = role(sp, "Walls").unwrap()["wall_type"].as_str().unwrap().to_string();
    let t = sp["wall_types"].as_array().unwrap().iter().find(|t| t["name"] == walls_type.as_str()).unwrap();
    assert_eq!(t["from"], json!("centerline"));
    assert!(guessed(&r, &format!("wall_types/{walls_type}")));
    assert_eq!(role(sp, "Walls").unwrap()["wall_type"], json!("Centreline walls"));
    // The unnamed layer of faces 100 mm apart is walls, by what it holds, about 3 7/8" thick.
    let p1 = role(sp, "P1").expect("P1 found by its face pairs");
    assert_eq!(p1["role"], json!("wall"));
    let why = r["guesses"].as_array().unwrap().iter().find(|g| g["key"] == "layers/P1").unwrap()["why"].as_str().unwrap().to_string();
    assert!(why.contains("wall face pairs"), "{why}");
    let ty = sp["wall_types"].as_array().unwrap().iter().find(|t| t["name"] == p1["wall_type"]).unwrap();
    assert_eq!(ty["thickness_in"], json!(3.875), "100 mm (3.94\") to the nearest eighth: {ty}");
    // Rooms in the centreline rooms.
    let names: Vec<&str> = sp["rooms"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Kitchen", "Bedroom"]);
    assert!(guessed(&r, "rooms/Kitchen"));
    assert_eq!(sp["levels"][0]["ceiling_ft"], json!(8.0));
    // It builds: the centreline walls stand up.
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    let b = s.execute("model.build", &json!({})).unwrap();
    assert!(b["walls"].as_u64().unwrap() >= 5, "{b}");
    assert_eq!(b["rooms"], json!(2), "{b}");
}

#[test]
fn an_empty_drawing_drafts_a_minimal_spec() {
    let mut s = Session::new();
    let r = s.execute("bspec.draft", &json!({})).unwrap();
    assert_eq!(r["spec"]["layers"], json!([]));
    assert_eq!(r["spec"]["levels"].as_array().unwrap().len(), 1);
    assert!(guessed(&r, "levels/Level 1"));
}
