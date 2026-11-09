//! Q6332: the OCR review layer (readings kept for a person to check) is never the room tag
//! layer, and a room tag layer that holds few of the plan's room labels doesn't make room
//! tracing skip the labels the other layers carry.

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

fn text(s: &mut Session, at: [f64; 2], t: &str, h: f64) {
    s.execute("text", &json!({"at": at, "text": t, "height": h})).unwrap();
}

const ROOMS: [(&str, &str, [f64; 2]); 5] = [
    ("OFFICE", "101", [100.0, 120.0]),
    ("STORAGE", "102", [340.0, 120.0]),
    ("LOBBY", "103", [580.0, 120.0]),
    ("CONFERENCE", "104", [820.0, 120.0]),
    ("BREAK ROOM", "105", [220.0, 360.0]),
];

/// Six rooms in 6" walls (inches). Five are labelled, name over number, in plain text on the
/// wall layer (a print's pen layer holds the walls and the lettering alike). The sixth holds
/// nothing but an OCR misread of linework, on the OCR review layer (off, as the import leaves
/// it), and so does one more spot in the office.
fn plan() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [960.0, 480.0]);
    for x in [6.0, 246.0, 486.0, 726.0] {
        rect(&mut s, [x, 6.0], [x + 228.0, 234.0]);
    }
    rect(&mut s, [6.0, 246.0], [474.0, 474.0]);
    rect(&mut s, [486.0, 246.0], [954.0, 474.0]);
    for (name, number, at) in ROOMS {
        text(&mut s, at, name, 6.0);
        text(&mut s, [at[0], at[1] - 20.0], number, 6.0);
    }
    layer(&mut s, "PDF-OCR-REVIEW");
    text(&mut s, [700.0, 360.0], "L---j", 12.0);
    text(&mut s, [60.0, 200.0], "Ij", 12.0);
    layer(&mut s, "0");
    s.execute("layer.set", &json!({"name": "PDF-OCR-REVIEW", "on": false})).unwrap();
    s
}

fn role<'a>(spec: &'a Value, layer: &str) -> Option<&'a Value> {
    spec["layers"].as_array().unwrap().iter().find(|l| l["layer"] == layer)
}

/// The rooms the job's rooms stage read, as it puts them in the spec.
fn read_rooms() -> Value {
    Value::Array(ROOMS.iter().map(|(name, number, _)| json!({"name": name, "number": number})).collect())
}

fn built_rooms(s: &mut Session) -> Vec<String> {
    let b = s.execute("model.build", &json!({})).unwrap();
    let list = s.execute("rooms.list", &json!({"outline": false})).unwrap();
    let mut names: Vec<String> = list["rooms"].as_array().unwrap().iter().filter_map(|r| r["name"].as_str().map(str::to_ascii_uppercase)).collect();
    names.sort_unstable();
    assert_eq!(b["rooms"].as_u64(), Some(names.len() as u64), "{b}");
    names
}

fn expected() -> Vec<String> {
    let mut v: Vec<String> = ROOMS.iter().map(|(n, _, _)| n.to_string()).collect();
    v.sort_unstable();
    v
}

#[test]
fn the_ocr_review_layer_is_never_the_room_tag_layer() {
    let mut s = plan();
    let r = s.execute("bspec.draft", &json!({})).unwrap();
    let sp = &r["spec"];
    assert!(role(sp, "PDF-OCR-REVIEW").is_none(), "the review layer takes no role: {sp}");
    assert!(
        !sp["rooms"].as_array().unwrap().iter().any(|x| x["name"].as_str().is_some_and(|n| n.contains("---") || n.eq_ignore_ascii_case("ij"))),
        "an OCR misread names no room: {sp}"
    );
    // Stored, with the rooms the job read: every labelled room is traced.
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    s.execute("bspec.set", &json!({"spec": {"rooms": read_rooms()}})).unwrap();
    assert_eq!(built_rooms(&mut s), expected());
}

/// A spec drafted before (it names the review layer as the room tag layer, as a checkpoint may
/// hold): room tracing still reads the plan's real labels, and the build says why.
#[test]
fn a_stored_review_room_tag_layer_does_not_hide_the_room_labels() {
    let mut s = plan();
    let mut sp = s.execute("bspec.draft", &json!({})).unwrap()["spec"].clone();
    sp["layers"].as_array_mut().unwrap().push(json!({"layer": "PDF-OCR-REVIEW", "role": "room_tag"}));
    sp["rooms"] = read_rooms();
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).unwrap();
    let b = s.execute("model.build", &json!({})).unwrap();
    let notes = b["notes"].to_string();
    assert!(notes.contains("PDF-OCR-REVIEW") && notes.contains("few of the plan's room labels"), "{notes}");
    s.undo().unwrap();
    assert_eq!(built_rooms(&mut s), expected());
}

/// A named room tag layer holding one of the plan's five room names: the plan is traced from all
/// its text. Holding them all, it is the only layer read (a name in a note elsewhere names no
/// room).
#[test]
fn a_room_tag_layer_with_few_of_the_labels_is_not_read_alone() {
    let mut s = plan();
    layer(&mut s, "A-AREA-IDEN");
    text(&mut s, [400.0, 400.0], "STORAGE", 6.0);
    let mut sp = s.execute("bspec.draft", &json!({})).unwrap()["spec"].clone();
    assert_eq!(role(&sp, "A-AREA-IDEN").unwrap()["role"], json!("room_tag"), "{sp}");
    sp["rooms"] = read_rooms();
    s.execute("bspec.set", &json!({"replace": true, "spec": sp.clone()})).unwrap();
    assert_eq!(built_rooms(&mut s), expected());

    // The tag layer carrying the labels: read alone.
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [480.0, 240.0]);
    rect(&mut s, [6.0, 6.0], [234.0, 234.0]);
    rect(&mut s, [246.0, 6.0], [474.0, 234.0]);
    text(&mut s, [300.0, 60.0], "KITCHEN", 6.0);
    layer(&mut s, "A-AREA-IDEN");
    text(&mut s, [100.0, 120.0], "OFFICE", 6.0);
    text(&mut s, [340.0, 120.0], "LOBBY", 6.0);
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    assert_eq!(built_rooms(&mut s), vec!["LOBBY".to_string(), "OFFICE".to_string()]);
}
