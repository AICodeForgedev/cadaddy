//! One room identity (Q6593): a room whose printed number was lost is built with its name only;
//! the number a person sets on it (`bim.props.set {number}`) is its number everywhere: the trade
//! pieces' unit rooms, the `room` of the elements inside it, the room lists. The unnumbered room
//! of a numbered sequence is asked about at build time.

use cadaddy_engine::Session;
use serde_json::{Value, json};

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

/// A 40' x 24' storey of two rooms split by a partition at 20'; `labels`: the west and east
/// rooms' tag texts (each line its own text, the first at the room's middle).
fn storey(s: &mut Session, ox: f64, labels: [&[&str]; 2]) {
    layer(s, "A-WALL");
    rect(s, [ox, 0.0], [ox + 480.0, 288.0]);
    rect(s, [ox + T, T], [ox + 480.0 - T, 288.0 - T]);
    let x = ox + 240.0;
    for dx in [-HALF, HALF] {
        line(s, [x + dx, T], [x + dx, 288.0 - T]);
    }
    layer(s, "A-ROOM-NAME");
    for (cx, lines) in [(ox + 120.0, labels[0]), (ox + 360.0, labels[1])] {
        for (i, t) in lines.iter().enumerate() {
            text(s, [cx, 150.0 - 12.0 * i as f64], t);
        }
    }
}

/// A device the readers put in the spec, at `at_ft` on level `level`.
fn device(id: &str, level: &str, at_ft: [f64; 2], mark: &str) -> Value {
    json!({"id": id, "kind": "device", "level": level, "at_ft": at_ft, "fields": {
        "type": {"value": mark, "source": "plan"},
        "device_type": {"value": format!("device type {mark}"), "source": "plan"},
        "symbol": {"value": format!("sym-{id}"), "source": "plan"},
    }})
}

/// Two storeys: level 1's offices numbered; level 2's two unit rooms of one type, the west one
/// labelled with its name and number, the east one with its name alone (its number lost). The
/// unit instances name both units; a device stands in each unit room.
fn building() -> Session {
    building_with([&["OFFICE", "101"], &["OFFICE", "102"]], [&["SUITE", "201"], &["SUITE"]])
}

/// [`building`] with these room labels on levels 1 and 2.
fn building_with(one: [&[&str]; 2], two: [&[&str]; 2]) -> Session {
    let mut s = Session::new();
    storey(&mut s, 0.0, one);
    storey(&mut s, 2000.0, two);
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [
            {"name": "1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000]},
            {"name": "2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9, "region": [1900, -100, 3000, 1000]}
        ],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}, {"layer": "A-ROOM-NAME", "role": "room_tag"}],
        "wall_types": [{"name": "X1", "thickness_in": 7.25, "from": "pairs"}, {"name": "P1", "thickness_in": 4.875, "from": "pairs"}],
        "walls": [
            {"level": "1", "a": [240.0, T], "b": [240.0, 288.0 - T], "wall_type": "P1"},
            {"level": "2", "a": [2240.0, T], "b": [2240.0, 288.0 - T], "wall_type": "P1"}
        ],
        "unit_instances": [
            {"number": "201", "level": "2", "type": "SUITE-A"},
            {"number": "202", "level": "2", "type": "SUITE-ADA"}
        ],
        "tags": [
            device("device:2:west", "2", [(2000.0 + 60.0) / 12.0, 60.0 / 12.0], "F"),
            device("device:2:east", "2", [(2000.0 + 420.0) / 12.0, 60.0 / 12.0], "F")
        ]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    fill(&mut s);
    s
}

/// What a job does after its build: the elements filled from the built rooms.
fn fill(s: &mut Session) {
    s.execute("bim.props.fill", &json!({"rooms": "built"})).unwrap();
}

/// The trade documents: the guest toilet accessories, one item in every unit, one in the
/// accessible unit only.
fn docs(s: &mut Session) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-roomnumber-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Sample Guest Toilet Accessories.txt"), "Robe Hook\nAC-1:  RH100   QTY-2\n1 EACH - AC-2: GB36\n").unwrap();
    s.execute("bim.atoms.docs", &json!({"paths": [dir.to_string_lossy()]})).unwrap();
    dir
}

/// The accessory rows of item `item`, by room.
fn accessories(s: &mut Session, item: &str) -> Vec<String> {
    let r = s.execute("bim.count", &json!({"trade": "specialties", "kind": "accessory", "item": item, "group_by": "room"})).unwrap();
    r["rows"].as_array().cloned().unwrap_or_default().iter().filter_map(|r| r["room"].as_str().map(str::to_string)).collect()
}

/// The `room` of the device read at plan x `x` (drawing units) on level 2.
fn device_room(s: &mut Session, x: f64) -> String {
    let all = s.execute("bim.props.find", &json!({"kind": "device", "limit": 100})).unwrap();
    for h in all["handles"].as_array().unwrap() {
        let p = s.execute("bim.props.get", &json!({"handle": h})).unwrap();
        let props = &p["elements"][0]["props"];
        if props["kind"]["source"]["at"][0].as_f64().is_some_and(|a| (a - x).abs() < 1.0) {
            return props["room"]["value"].as_str().unwrap_or_default().to_string();
        }
    }
    panic!("no device read at x {x}: {all}")
}

const WEST: f64 = 2060.0;
const EAST: f64 = 2420.0;

/// The handle of level 2's unit room with no number.
fn unnumbered_room(s: &mut Session) -> String {
    let found = s.execute("bim.props.find", &json!({"kind": "room", "where": {"name": "SUITE"}, "limit": 100})).unwrap();
    let mut out = Vec::new();
    for h in found["handles"].as_array().unwrap() {
        let p = s.execute("bim.props.get", &json!({"handle": h})).unwrap();
        if p["elements"][0]["props"]["number"]["value"].as_str().is_none_or(|n| n.trim().is_empty()) {
            out.push(h.as_str().unwrap().to_string());
        }
    }
    assert_eq!(out.len(), 1, "one unit room with no number: {found:#}");
    out.remove(0)
}

#[test]
fn a_room_number_set_by_a_person_is_the_rooms_number_everywhere() {
    let mut s = building();
    let dir = docs(&mut s);
    // As built: the east unit room has its name and no number.
    let rooms = s.execute("rooms.list", &json!({"level": "2", "outline": false})).unwrap();
    let unnumbered: Vec<&Value> = rooms["rooms"].as_array().unwrap().iter().filter(|r| r["number"].is_null() || r["number"] == json!("")).collect();
    assert_eq!(unnumbered.len(), 1, "one room on level 2 has no number: {rooms:#}");
    // The build asks which number it is (a room of a numbered type with none).
    let asks = s.execute("rfi.list", &json!({"kind": "room_unnumbered"})).unwrap();
    assert_eq!(asks["total"], json!(1), "{asks:#}");
    let ask = &asks["items"][0];
    assert_eq!(ask["level"], json!("2"), "{ask:#}");
    assert!(ask["proposed"].as_str().unwrap_or_default().contains("202"), "the unit number no room has is proposed: {ask:#}");
    s.execute("bim.atoms.build", &json!({"trades": ["specialties"]})).unwrap();
    assert_eq!(accessories(&mut s, "AC-1").len(), 1, "before the number: one unit room");
    assert_eq!(device_room(&mut s, EAST), "SUITE", "before the number: the room's name");
    // A person gives it its number.
    let unnumbered = unnumbered_room(&mut s);
    let set = s.execute("bim.props.set", &json!({"handle": unnumbered, "props": {"number": "202"}})).unwrap();
    assert_eq!(set["set"], json!(1), "{set:#}");
    // The device inside it takes the room's number as its room, as the other unit's does.
    assert_eq!(device_room(&mut s, WEST), "201");
    assert_eq!(device_room(&mut s, EAST), "202", "the device in the renumbered room");
    // Nothing in it keeps the old label; its trade pieces take its name and number, their style.
    let old = s.execute("bim.props.find", &json!({"where": {"room": "SUITE"}, "limit": 1000})).unwrap();
    assert_eq!(old["count"], json!(0), "{old:#}");
    let pieces = s.execute("bim.props.find", &json!({"where": {"room": "SUITE 202"}, "limit": 1000})).unwrap();
    assert!(pieces["count"].as_u64().unwrap_or(0) > 0, "{pieces:#}");
    // The trade pieces lay out both unit rooms: AC-1 in each, AC-2 in the accessible one.
    s.execute("bim.atoms.build", &json!({"trades": ["specialties"]})).unwrap();
    let ac1 = accessories(&mut s, "AC-1");
    assert_eq!(ac1.len(), 2, "one in each unit room: {ac1:?}");
    let ac2 = accessories(&mut s, "AC-2");
    assert_eq!(ac2.len(), 1, "{ac2:?}");
    assert!(ac2[0].contains("202"), "the accessible unit, by its number: {ac2:?}");
    // The room lists read its number.
    let rooms = s.execute("rooms.list", &json!({"level": "2", "outline": false})).unwrap();
    assert!(rooms["rooms"].as_array().unwrap().iter().any(|r| r["number"] == json!("202")), "{rooms:#}");
    // Numbered, it is no longer asked about.
    let asks = s.execute("rfi.list", &json!({"kind": "room_unnumbered", "status": "open"})).unwrap();
    assert_eq!(asks["total"], json!(0), "{asks:#}");
    // A rebuild keeps the person's number (their edit goes on the rebuilt room) and asks nothing.
    s.execute("model.build", &json!({"force": true})).unwrap();
    fill(&mut s);
    s.execute("bim.atoms.build", &json!({"trades": ["specialties"]})).unwrap();
    assert_eq!(accessories(&mut s, "AC-1").len(), 2, "after a rebuild");
    assert_eq!(device_room(&mut s, EAST), "202", "after a rebuild");
    let asks = s.execute("rfi.list", &json!({"kind": "room_unnumbered", "status": "open"})).unwrap();
    assert_eq!(asks["total"], json!(0), "{asks:#}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The handles of level `level`'s rooms of name `name` with no number.
fn unnumbered_rooms(s: &mut Session, level: &str, name: &str) -> Vec<String> {
    let found = s.execute("bim.props.find", &json!({"kind": "room", "where": {"name": name, "number": "missing"}, "limit": 100})).unwrap();
    found["elements"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["level"] == json!(level))
        .filter_map(|e| e["handle"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn two_unnumbered_rooms_of_one_name_keep_their_own_numbers_through_a_rebuild() {
    // Level 1's units numbered; both of level 2's lost their numbers on the plan.
    let mut s = building_with([&["SUITE", "101"], &["SUITE", "102"]], [&["SUITE"], &["SUITE"]]);
    let asks = s.execute("rfi.list", &json!({"kind": "room_unnumbered"})).unwrap();
    assert_eq!(asks["total"], json!(2), "{asks:#}");
    // The unit numbers no room has: 201 and 202.
    let proposed = asks["items"][0]["proposed"].as_str().unwrap_or_default().to_string();
    assert!(proposed.contains("201") && proposed.contains("202"), "{proposed}");
    let hs = unnumbered_rooms(&mut s, "2", "SUITE");
    assert_eq!(hs.len(), 2, "{hs:?}");
    for (h, n) in hs.iter().zip(["201", "202"]) {
        s.execute("bim.props.set", &json!({"handle": h, "props": {"number": n}})).unwrap();
    }
    let numbers = |s: &mut Session| -> Vec<String> {
        let rooms = s.execute("rooms.list", &json!({"level": "2", "outline": false})).unwrap();
        let mut v: Vec<String> = rooms["rooms"].as_array().unwrap().iter().filter_map(|r| r["number"].as_str().map(str::to_string)).collect();
        v.sort();
        v
    };
    assert_eq!(numbers(&mut s), ["201", "202"]);
    // Rebuilt: each room keeps its own number (not one person's edit on both).
    s.execute("model.build", &json!({"force": true})).unwrap();
    assert_eq!(numbers(&mut s), ["201", "202"], "after a rebuild");
    let asks = s.execute("rfi.list", &json!({"kind": "room_unnumbered", "status": "open"})).unwrap();
    assert_eq!(asks["total"], json!(0), "{asks:#}");
}

#[test]
fn rooms_of_unnumbered_kinds_are_not_asked_about() {
    // A corridor and a lobby carry names only on every floor: no numbered sequence to ask about.
    let mut s = Session::new();
    storey(&mut s, 0.0, [&["LOBBY"], &["CORRIDOR"]]);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}, {"layer": "A-ROOM-NAME", "role": "room_tag"}],
        "wall_types": [{"name": "X1", "thickness_in": 7.25, "from": "pairs"}, {"name": "P1", "thickness_in": 4.875, "from": "pairs"}],
        "walls": [{"level": "1", "a": [240.0, T], "b": [240.0, 288.0 - T], "wall_type": "P1"}]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let asks = s.execute("rfi.list", &json!({"kind": "room_unnumbered"})).unwrap();
    assert_eq!(asks["total"], json!(0), "{asks:#}");
}
