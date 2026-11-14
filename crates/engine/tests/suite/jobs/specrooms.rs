//! Spec rooms are keyed by (level, number) when they have a number, else by their name (Q6426):
//! rooms known by number alone never collapse into one, a per-room edit touches only its room,
//! and an entry can correct the name or number a plan label gives a room.

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

/// One 40' × 30' floor (inches): 6" outside walls, a 4" partition, a room each side tagged
/// `left` and `right` on the room tag layer.
fn plan(left: &str, right: &str) -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    rect(&mut s, 0.0, 0.0, 480.0, 360.0);
    rect(&mut s, 6.0, 6.0, 474.0, 354.0);
    for x in [238.0, 242.0] {
        line(&mut s, [x, 6.0], [x, 354.0]);
    }
    s.execute("layer.new", &json!({"name": "A-AREA-IDEN", "current": true})).unwrap();
    s.execute("text", &json!({"at": [100.0, 170.0], "text": left, "height": 8.0})).unwrap();
    s.execute("text", &json!({"at": [340.0, 170.0], "text": right, "height": 8.0})).unwrap();
    let levels = json!([{"name": "Level 1", "elevation_ft": 0.0, "height_ft": 12.0, "ceiling_ft": 10.0}]);
    s.execute("bspec.draft", &json!({"store": true, "levels": levels})).unwrap();
    s
}

fn spec_rooms(s: &mut Session) -> Vec<Value> {
    let r = s.execute("bspec.get", &json!({})).unwrap();
    r["spec"]["rooms"].as_array().cloned().unwrap_or_default()
}

fn set_rooms(s: &mut Session, rooms: Value) {
    let r = s.execute("bspec.set", &json!({"spec": {"rooms": rooms}})).unwrap();
    assert_eq!(r["errors"], json!([]), "{r}");
}

/// The spec's entries numbered `n`.
fn numbered<'a>(rooms: &'a [Value], n: &str) -> Vec<&'a Value> {
    rooms.iter().filter(|r| r["number"] == n).collect()
}

/// The built room `n`: its name and ceiling, feet.
fn built(s: &mut Session, n: &str) -> (String, f64) {
    let r = s.execute("rooms.get", &json!({"number": n})).unwrap_or_else(|e| panic!("room {n}: {e}"));
    (r["name"].as_str().unwrap_or_default().to_string(), r["ceilingFt"].as_f64().unwrap_or(f64::NAN))
}

#[test]
fn rooms_known_by_number_alone_stay_one_entry_each() {
    let mut s = plan("101", "102");
    let rooms = json!([{"name": "", "number": "101", "ceiling_ft": 9}, {"name": "", "number": "102", "ceiling_ft": 8}]);
    set_rooms(&mut s, rooms.clone());
    let got = spec_rooms(&mut s);
    assert_eq!(numbered(&got, "101").len(), 1, "{got:?}");
    assert_eq!(numbered(&got, "102").len(), 1, "{got:?}");
    // Setting the same list again changes nothing.
    set_rooms(&mut s, rooms);
    let again = spec_rooms(&mut s);
    assert_eq!(again, got);
    // One room's edit touches that room only.
    set_rooms(&mut s, json!([{"name": "", "number": "102", "ceiling_ft": 7.5}]));
    let edited = spec_rooms(&mut s);
    assert_eq!(edited.len(), got.len(), "{edited:?}");
    assert_eq!(numbered(&edited, "101")[0]["ceiling_ft"], 9.0);
    assert_eq!(numbered(&edited, "102")[0]["ceiling_ft"], 7.5);
    // The build takes each room's own ceiling.
    s.execute("model.build", &json!({})).unwrap();
    assert!((built(&mut s, "101").1 - 9.0).abs() < 1e-6, "{:?}", built(&mut s, "101"));
    assert!((built(&mut s, "102").1 - 7.5).abs() < 1e-6, "{:?}", built(&mut s, "102"));
}

#[test]
fn an_entry_corrects_a_plan_labels_name_and_number() {
    // A misread name label and a room left as just its number.
    let mut s = plan("ST0RAGE", "104");
    set_rooms(
        &mut s,
        json!([
            {"name": "STORAGE", "label": "ST0RAGE", "number": "101"},
            {"name": "OFFICE", "number": "105", "label": "104", "ceiling_ft": 9}
        ]),
    );
    // Naming a number-only room by its number.
    set_rooms(&mut s, json!([{"name": "", "number": "105", "ceiling_ft": 8.5}]));
    let got = spec_rooms(&mut s);
    assert_eq!(numbered(&got, "105").len(), 1, "{got:?}");
    assert_eq!(numbered(&got, "105")[0]["name"], "OFFICE", "{got:?}");
    s.execute("model.build", &json!({})).unwrap();
    assert_eq!(built(&mut s, "101").0, "STORAGE");
    let (name, ceiling) = built(&mut s, "105");
    assert_eq!(name, "OFFICE");
    assert!((ceiling - 8.5).abs() < 1e-6, "{ceiling}");
    assert!(s.execute("rooms.get", &json!({"number": "104"})).is_err(), "the plan's number was corrected");
    // Renaming through the spec keeps the room's other values.
    set_rooms(&mut s, json!([{"name": "STAFF", "number": "105"}]));
    let got = spec_rooms(&mut s);
    assert_eq!(numbered(&got, "105").len(), 1, "{got:?}");
    assert_eq!(numbered(&got, "105")[0]["name"], "STAFF");
    assert_eq!(numbered(&got, "105")[0]["ceiling_ft"], 8.5);
}
