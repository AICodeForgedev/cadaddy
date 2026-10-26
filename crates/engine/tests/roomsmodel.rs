//! Rooms become model elements (Q5291): `model.build` makes each room a `room` element of the
//! level whose plan it was read on (name, number, area, ceiling, floor finish, outline), listed
//! in `bim.tree`, `rooms.list` and the takeoff (`bim.bom`, `bim.takeoff`). A note sentence is
//! never a room; a room traced in an enlarged view of the plan (its walls are not built) is
//! no room and no second copy of the plan's room.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, x0: f64, y0: f64, x1: f64, y1: f64) {
    line(s, [x0, y0], [x1, y0]);
    line(s, [x1, y0], [x1, y1]);
    line(s, [x1, y1], [x0, y1]);
    line(s, [x0, y1], [x0, y0]);
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).unwrap();
}

/// A 40' × 30' floor (inches) at (`ox`, `oy`), `scale` times: 6" outside walls and a 4" partition
/// from wall to wall at 20', so two closed rooms; `tags` written in the west and east rooms.
fn floor(s: &mut Session, ox: f64, oy: f64, scale: f64, west: &[&str], east: &[&str]) {
    let p = |x: f64, y: f64| [ox + x * scale, oy + y * scale];
    s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    let r = |s: &mut Session, a: [f64; 2], b: [f64; 2]| rect(s, a[0], a[1], b[0], b[1]);
    r(s, p(0.0, 0.0), p(480.0, 360.0));
    r(s, p(6.0, 6.0), p(474.0, 354.0));
    for x in [238.0, 242.0] {
        line(s, p(x, 6.0), p(x, 354.0));
    }
    s.execute("layer.current", &json!({"name": "A-AREA-IDEN"})).unwrap();
    for (i, t) in west.iter().enumerate() {
        text(s, p(100.0, 220.0 - 30.0 * i as f64), t);
    }
    for (i, t) in east.iter().enumerate() {
        text(s, p(340.0, 220.0 - 30.0 * i as f64), t);
    }
}

/// The office (a 20' × 15' part of the plan) drawn at twice the scale at (`ox`, `oy`): its 6"
/// walls read 12", tagged OFFICE 101 again (an enlarged plan beside the floor plan).
fn enlarged_office(s: &mut Session, ox: f64, oy: f64) {
    s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    rect(s, ox, oy, ox + 480.0, oy + 360.0);
    rect(s, ox + 12.0, oy + 12.0, ox + 468.0, oy + 348.0);
    s.execute("layer.current", &json!({"name": "A-AREA-IDEN"})).unwrap();
    text(s, [ox + 200.0, oy + 200.0], "OFFICE");
    text(s, [ox + 200.0, oy + 160.0], "101");
}

const L1: [f64; 4] = [-200.0, -200.0, 2400.0, 1200.0];
const L2: [f64; 4] = [-200.0, 1800.0, 2400.0, 3200.0];

/// Level 1: OFFICE 101 and STORAGE 102, a note sentence in the office, and an enlarged plan of
/// the office beside it. Level 2:
/// GUEST ROOM 201, and a room with only a note in it. A closed room off every level's plan.
fn job() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("layer.new", &json!({"name": "A-AREA-IDEN"})).unwrap();
    floor(&mut s, 0.0, 0.0, 1.0, &["OFFICE", "101", "PROVIDE BLOCKING AT ALL TV MOUNTS."], &["STORAGE", "102"]);
    enlarged_office(&mut s, 1000.0, 0.0);
    floor(&mut s, 0.0, 2000.0, 1.0, &["GUEST ROOM", "201"], &["SEE NOTE 4, TYP."]);
    floor(&mut s, 6000.0, 0.0, 1.0, &["LOBBY"], &[]);
    s
}

fn draft(s: &mut Session) {
    let levels = json!([
        {"name": "Level 1", "elevation_ft": 0.0, "height_ft": 10.0, "ceiling_ft": 9.0, "region": L1},
        {"name": "Level 2", "elevation_ft": 10.0, "height_ft": 10.0, "ceiling_ft": 8.0, "region": L2}
    ]);
    s.execute("bspec.draft", &json!({"store": true, "levels": levels, "stack": true})).unwrap();
}

fn built() -> (Session, Value) {
    let mut s = job();
    draft(&mut s);
    let r = s.execute("model.build", &json!({})).unwrap();
    (s, r)
}

fn rooms_of<'a>(tree: &'a Value, level: &str) -> Option<&'a Value> {
    let lv = tree["levels"].as_array()?.iter().find(|l| l["name"] == json!(level))?;
    lv["trades"].as_array()?.iter().filter_map(|t| t["categories"].as_array()).flatten().find(|c| c["name"] == json!("Rooms"))
}

#[test]
fn rooms_are_model_elements_on_their_levels() {
    let (mut s, r) = built();
    assert_eq!(r["rooms"], json!(3), "OFFICE, STORAGE, GUEST ROOM only: {r}");
    assert_eq!(r["roomsByLevel"], json!({"Level 1": 2, "Level 2": 1}), "{r}");
    // bim.tree: Level ▸ Architecture ▸ Rooms.
    let tree = s.execute("bim.tree", &json!({})).unwrap();
    assert_eq!(rooms_of(&tree, "Level 1").map(|c| c["count"].clone()), Some(json!(2)), "{tree}");
    assert_eq!(rooms_of(&tree, "Level 2").map(|c| c["count"].clone()), Some(json!(1)), "{tree}");
    let types: Vec<String> =
        rooms_of(&tree, "Level 1").unwrap()["types"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(types, vec!["Office", "Storage"]);
    // rooms.list reads the elements: names, numbers, levels, areas.
    let list = s.execute("rooms.list", &json!({"outline": false})).unwrap();
    let rooms = list["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 3, "{list}");
    let get = |n: &str| rooms.iter().find(|x| x["number"] == json!(n)).unwrap_or_else(|| panic!("room {n}: {list}"));
    assert_eq!((get("101")["name"].clone(), get("101")["level"].clone()), (json!("Office"), json!("Level 1")));
    assert_eq!((get("102")["name"].clone(), get("102")["level"].clone()), (json!("Storage"), json!("Level 1")));
    assert_eq!((get("201")["name"].clone(), get("201")["level"].clone()), (json!("Guest Room"), json!("Level 2")));
    // 19'-4" × 29' inside: the plan's room, not the enlarged view's (four times that).
    let area = get("101")["areaSqFt"].as_f64().unwrap();
    assert!((area - 232.0 * 348.0 / 144.0).abs() < 1.0, "{area}");
    assert!((get("201")["ceilingFt"].as_f64().unwrap() - 8.0).abs() < 1e-6, "{list}");
    // Stacked: the second floor's room moved with its level over the first's.
    assert!(get("201")["at"][1].as_f64().unwrap() < 400.0, "{list}");
    // One element: bim.element knows it as a room with its area.
    let h = rooms_of(&tree, "Level 2").unwrap()["types"][0]["elements"][0]["handle"].as_str().unwrap().to_string();
    let el = s.execute("bim.element", &json!({"handle": h})).unwrap();
    assert!(el.to_string().contains("Rooms"), "{el}");
}

#[test]
fn rooms_are_in_the_takeoff() {
    let (mut s, _) = built();
    let bom = s.execute("bim.bom", &json!({"group_by": "level"})).unwrap();
    let lines: Vec<&Value> = bom["lines"].as_array().unwrap().iter().filter(|l| l["category"] == json!("rooms")).collect();
    let line = |item: &str, level: &str| {
        lines.iter().find(|l| l["item"] == json!(item) && l["level"] == json!(level)).unwrap_or_else(|| panic!("{item} on {level}: {lines:?}"))
    };
    assert_eq!(line("Room", "Level 1")["net"].as_f64(), Some(2.0));
    assert_eq!(line("Room", "Level 2")["net"].as_f64(), Some(1.0));
    let one = 232.0 * 348.0 / 144.0;
    assert!((line("Room floor area", "Level 1")["net"].as_f64().unwrap() - 2.0 * one).abs() < 2.0, "{lines:?}");
    assert!((line("Room ceiling area", "Level 2")["net"].as_f64().unwrap() - one).abs() < 1.0, "{lines:?}");
    assert_eq!(line("Room ceiling area", "Level 2")["size"], json!("8'-0\" ceiling"));
    assert!(line("Room", "Level 1")["elements"].as_array().is_some_and(|e| e.len() == 2), "{lines:?}");
    // bim.takeoff links the room lines to the room elements of their level.
    let t = s.execute("bim.takeoff", &json!({"group_by": "level", "filter": {"category": "rooms"}})).unwrap();
    let text = t.to_string();
    assert!(text.contains("Level 1") && text.contains("Level 2") && text.contains("Room floor area"), "{t}");
}

#[test]
fn notes_enlarged_views_and_rooms_off_the_plans_are_no_rooms() {
    let (mut s, r) = built();
    let notes = r["notes"].to_string();
    assert!(notes.contains("not inside built walls"), "the enlarged view's rooms go: {notes}");
    let list = s.execute("rooms.list", &json!({"outline": false})).unwrap();
    let names: Vec<String> = list["rooms"].as_array().unwrap().iter().filter_map(|x| x["name"].as_str().map(str::to_string)).collect();
    assert!(!names.iter().any(|n| n.contains("NOTE") || n.contains("PROVIDE")), "{names:?}");
    assert!(!names.iter().any(|n| n == "LOBBY"), "a room off every level's plan is not built: {names:?}");
    // Before a build, a room off every level's region has no level (never Level 1's).
    let mut s = job();
    draft(&mut s);
    let list = s.execute("rooms.list", &json!({"outline": false})).unwrap();
    let lobby = list["rooms"].as_array().unwrap().iter().find(|x| x["name"] == json!("LOBBY")).cloned().unwrap_or_else(|| panic!("{list}"));
    assert_eq!(lobby["level"], Value::Null, "{lobby}");
    let guest = list["rooms"].as_array().unwrap().iter().find(|x| x["name"] == json!("GUEST ROOM")).cloned().unwrap();
    assert_eq!(guest["level"], json!("Level 2"));
}

#[test]
fn a_rebuild_keeps_one_element_per_room_and_clear_removes_them() {
    let (mut s, _) = built();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["rooms"], json!(3), "{r}");
    let count = |s: &Session| s.doc().unwrap().model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("room")).count();
    assert_eq!(count(&s), 3);
    s.execute("model.build", &json!({"clear": true})).unwrap();
    assert_eq!(count(&s), 0);
}

#[test]
fn hostile_room_tags_never_panic() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("layer.new", &json!({"name": "A-AREA-IDEN"})).unwrap();
    floor(&mut s, 0.0, 0.0, 1.0, &["", "(1)", "1. NOTE", "\u{0}\u{7f}", &"X".repeat(5000)], &["0000000", "A'-0\"", "ROOM ROOM ROOM ROOM ROOM ROOM"]);
    // A sliver and a degenerate room.
    s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    rect(&mut s, 600.0, 0.0, 600.001, 1e6);
    draft(&mut s);
    let r = s.execute("model.build", &json!({})).unwrap();
    assert!(r["rooms"].as_u64().is_some_and(|n| n <= 2), "{r}");
    s.execute("bim.bom", &json!({"group_by": "level"})).unwrap();
    s.execute("rooms.list", &json!({})).unwrap();
}
