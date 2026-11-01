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

/// A room's tag drawn inside its own box (a tag box or a piece of furniture on the wall layer):
/// the box is no room; the room is traced again on the built walls, at its full size, and keeps
/// its name and number.
#[test]
fn a_tag_in_a_box_is_traced_again_on_the_built_walls() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("layer.new", &json!({"name": "A-AREA-IDEN"})).unwrap();
    floor(&mut s, 0.0, 0.0, 1.0, &["OFFICE", "101"], &[]);
    s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    rect(&mut s, 320.0, 150.0, 390.0, 240.0);
    s.execute("layer.current", &json!({"name": "A-AREA-IDEN"})).unwrap();
    text(&mut s, [330.0, 220.0], "STORAGE");
    text(&mut s, [330.0, 190.0], "102");
    let levels = json!([{"name": "Level 1", "elevation_ft": 0.0, "height_ft": 10.0, "ceiling_ft": 9.0, "region": L1}]);
    s.execute("bspec.draft", &json!({"store": true, "levels": levels})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["rooms"], json!(2), "{r}");
    assert!(r["notes"].to_string().contains("traced again on the built walls"), "{r}");
    let list = s.execute("rooms.list", &json!({"outline": false})).unwrap();
    let rooms = list["rooms"].as_array().unwrap();
    let storage = rooms.iter().find(|x| x["number"] == json!("102")).unwrap_or_else(|| panic!("{list}"));
    assert_eq!(storage["name"], json!("Storage"), "{list}");
    let area = storage["areaSqFt"].as_f64().unwrap();
    assert!(area > 0.9 * 232.0 * 348.0 / 144.0, "the room, not the box: {area}");
}

#[test]
fn a_room_element_holds_its_properties_once_and_keeps_user_edits_through_a_rebuild() {
    let (mut s, _) = built();
    // bim.props: each room element is a room object with its name, number, area and ceiling.
    let found = s.execute("bim.props.find", &json!({"kind": "room", "where": {"number": "101"}})).unwrap();
    assert_eq!(found["count"], json!(1), "{found}");
    let h = found["handles"][0].as_str().unwrap().to_string();
    let g = s.execute("bim.props.get", &json!({"handle": h})).unwrap();
    let props = g["elements"][0]["props"].clone();
    let props = if props.is_null() { g["props"].clone() } else { props };
    assert_eq!(props["name"]["value"], json!("Office"), "{g}");
    assert_eq!(props["level"]["value"], json!("Level 1"), "{g}");
    assert_eq!(props["area"]["unit"], json!("sf"), "{g}");
    assert!((props["area"]["value"].as_f64().unwrap() - 232.0 * 348.0 / 144.0).abs() < 1.0, "{g}");
    assert_eq!(props["ceiling_height"]["value"].as_f64(), Some(9.0), "{g}");
    // rooms.build's volumes are the rooms' 3D views: still three rooms, not six.
    let built = s.execute("rooms.build", &json!({})).unwrap();
    assert_eq!(built["built"], json!(3), "{built}");
    let all = s.execute("bim.props.find", &json!({"kind": "room"})).unwrap();
    assert_eq!(all["count"], json!(3), "{all}");
    // A person's edit stays on the room when the model is built again from the prints.
    s.execute("bim.props.set", &json!({"handle": h, "props": {"floor_finish": "VCT-1"}})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let again = s.execute("bim.props.find", &json!({"kind": "room", "where": {"floor_finish": "VCT-1"}})).unwrap();
    assert_eq!(again["count"], json!(1), "{again}");
    let h2 = again["handles"][0].as_str().unwrap().to_string();
    assert_ne!(h2, h, "the rebuild made a new element");
    let g2 = s.execute("bim.props.get", &json!({"handle": h2})).unwrap();
    assert!(g2.to_string().contains("\"user\""), "{g2}");
    assert!(g2.to_string().contains("\"101\""), "{g2}");
}

/// A room the architectural plan doesn't label (another trade's plan tags it): its spec room
/// carries where the tag is, and the build traces and numbers the room from there. A name label
/// elsewhere on the plan never claims it.
#[test]
fn a_room_only_another_trade_tags_is_traced_from_its_tag() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("layer.new", &json!({"name": "A-AREA-IDEN"})).unwrap();
    floor(&mut s, 0.0, 0.0, 1.0, &["STORAGE", "101"], &[]);
    let levels = json!([{"name": "Level 1", "elevation_ft": 0.0, "height_ft": 10.0, "ceiling_ft": 9.0, "region": L1}]);
    s.execute("bspec.draft", &json!({"store": true, "levels": levels, "stack": true})).unwrap();
    let mut spec = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    spec["rooms"] = json!([
        {"name": "STORAGE", "number": "101"},
        {"name": "STORAGE", "number": "102", "at": [340.0, 200.0]}
    ]);
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let list = s.execute("rooms.list", &json!({"outline": false})).unwrap();
    let rooms = list["rooms"].as_array().unwrap();
    let get = |n: &str| rooms.iter().find(|x| x["number"] == json!(n)).unwrap_or_else(|| panic!("room {n}: {list}"));
    // The east room, traced from the other trade's tag; the west one from its own labels.
    assert!(get("102")["at"][0].as_f64().unwrap() > 240.0, "{list}");
    assert!(get("101")["at"][0].as_f64().unwrap() < 240.0, "{list}");
    assert_eq!(rooms.len(), 2, "{list}");
}

/// A corridor, stair or hoistway of the spec's circulation (traced before the rooms) is the room
/// its traced floor is: lines across it don't cut it, it keeps its number even when its number
/// tag sits on a piece of it past a wall line (no "Room 100" there), and a stair's DN is no room.
#[test]
fn a_corridor_of_the_circulation_is_its_floors_room_whatever_lines_cross_it() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("layer.new", &json!({"name": "A-AREA-IDEN"})).unwrap();
    floor(&mut s, 0.0, 0.0, 1.0, &["STORAGE", "101"], &[]);
    // Across the east room: lines on the wall layer; the corridor traced west of the one at 330.
    s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
    line(&mut s, [330.0, 6.0], [330.0, 354.0]);
    line(&mut s, [242.0, 120.0], [330.0, 120.0]);
    s.execute("layer.current", &json!({"name": "A-AREA-IDEN"})).unwrap();
    text(&mut s, [280.0, 200.0], "CORRIDOR");
    text(&mut s, [400.0, 150.0], "100");
    text(&mut s, [420.0, 60.0], "DN");
    let levels = json!([{"name": "Level 1", "elevation_ft": 0.0, "height_ft": 10.0, "ceiling_ft": 9.0, "region": L1}]);
    s.execute("bspec.draft", &json!({"store": true, "levels": levels, "stack": true})).unwrap();
    let mut spec = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    spec["rooms"] = json!([{"name": "STORAGE", "number": "101"}, {"name": "CORRIDOR", "number": "100"}]);
    spec["circulation"] =
        json!([{"kind": "corridor", "level": "Level 1", "name": "CORRIDOR", "at": [280.0, 200.0], "rects": [[[242.0, 6.0], [330.0, 354.0]]]}]);
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let list = s.execute("rooms.list", &json!({"outline": false})).unwrap();
    let rooms = list["rooms"].as_array().unwrap();
    let corridor = rooms.iter().find(|x| x["number"] == json!("100")).unwrap_or_else(|| panic!("{list}"));
    assert_eq!(corridor["name"], json!("CORRIDOR"), "{list}");
    let want = 88.0 * 348.0 / 144.0;
    assert!((corridor["areaSqFt"].as_f64().unwrap() - want).abs() < 0.01 * want, "{list}");
    // The storage room, and nothing traced from DN or the corridor's number tag past the line.
    assert!(rooms.iter().any(|x| x["number"] == json!("101")), "{list}");
    assert_eq!(rooms.len(), 2, "{list}");
}
