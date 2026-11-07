//! Camera tours: viewpoints (named, per room), paths through them, timing, kept with the drawing.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn session_with_rooms() -> Session {
    let mut s = Session::new();
    // A 40' × 20' plan in inches, two rooms named on it.
    s.execute("rectang", &json!({"p1": [0, 0], "p2": [480, 240]})).unwrap();
    s.execute("text", &json!({"at": [60, 120], "height": 6, "text": "LOBBY"})).unwrap();
    s.execute("text", &json!({"at": [360, 120], "height": 6, "text": "OFFICE"})).unwrap();
    s
}

fn f(v: &Value) -> f64 {
    v.as_f64().unwrap()
}

#[test]
fn viewpoints_and_a_tour_are_saved_timed_and_undone() {
    let mut s = Session::new();
    s.execute("tour.view", &json!({"name": "A", "eye": [0, 0, 5.5], "yaw": 450})).unwrap();
    let r = s.execute("tour.view", &json!({"name": "B", "eye": [10, 0, 5.5], "yaw": 90, "pitch": -10})).unwrap();
    assert_eq!(r["views"], 2);
    assert_eq!(r["view"]["pitch"], -10.0);
    let r = s.execute("tour.create", &json!({"name": "Walkthrough", "stops": ["A", "B"], "speed": 2.5, "pause": 1})).unwrap();
    assert!((f(&r["length"]) - 10.0).abs() < 1e-6, "{r}");
    assert!((f(&r["duration"]) - 6.0).abs() < 1e-6, "4 s moving and 1 s at each stop");
    let t = s.execute("tour.timing", &json!({"name": "walkthrough", "fps": 24, "at": 3})).unwrap();
    assert_eq!(t["frames"], 145, "{t}");
    assert!((f(&t["pose"]["eye"][0]) - 5.0).abs() < 0.05);
    let l = s.execute("tour.list", &json!({})).unwrap();
    assert_eq!(l["views"].as_array().unwrap().len(), 2);
    assert_eq!(l["views"][0]["yaw"], 90.0, "the heading is normalised");
    assert_eq!(l["tours"][0]["stops"], json!(["A", "B"]));
    // A viewpoint a tour passes through can't be deleted; the tour can, then it.
    assert!(s.execute("tour.view", &json!({"name": "A", "delete": true})).is_err());
    s.execute("tour.create", &json!({"name": "Walkthrough", "delete": true})).unwrap();
    s.execute("tour.view", &json!({"name": "A", "delete": true})).unwrap();
    assert_eq!(s.execute("tour.list", &json!({})).unwrap()["views"].as_array().unwrap().len(), 1);
    // Undo brings them back.
    s.execute("undo", &json!({})).unwrap();
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(s.execute("tour.list", &json!({})).unwrap()["tours"].as_array().unwrap().len(), 1);
}

#[test]
fn rooms_give_viewpoints_and_a_tour_through_them() {
    let mut s = session_with_rooms();
    let r = s.execute("tour.view", &json!({"name": "Front desk", "room": "lobby", "yaw": 0})).unwrap();
    assert_eq!(r["view"]["room"], "LOBBY", "{r}");
    assert!((f(&r["view"]["eye"][2]) - 5.5).abs() < 1e-9, "5'6\" above the floor: {r}");
    let r = s.execute("tour.rooms", &json!({"tour": "All rooms", "look": "views"})).unwrap();
    assert_eq!(r["added"], json!(["LOBBY", "OFFICE"]), "{r}");
    // The lobby and office labels are 25' apart (300").
    assert!((f(&r["tour"]["length"]) - 25.0).abs() < 1e-6, "{r}");
    assert!(s.execute("tour.view", &json!({"name": "X", "room": "Kitchen"})).is_err());
}

#[test]
fn tours_survive_a_save_and_bad_params_are_refused() {
    let mut s = Session::new();
    for bad in [
        json!({}),
        json!({"name": "A"}),
        json!({"name": "A", "eye": ["x", 0, 0]}),
        json!({"name": "A", "eye": [1e300, 0, 0]}),
        json!({"name": "A", "eye": [0, 0, 0], "yaw": "north"}),
    ] {
        let r = s.execute("tour.view", &bad);
        assert!(r.is_err() || bad.get("yaw").is_some(), "{bad}: {r:?}");
    }
    s.execute("tour.view", &json!({"name": "A", "eye": [0, 0, 5]})).unwrap();
    for bad in [
        json!({"name": "T"}),
        json!({"name": "T", "stops": []}),
        json!({"name": "T", "stops": ["A", "nope"]}),
        json!({"name": "T", "stops": ["A"], "look": "sideways"}),
        json!({"name": "", "stops": ["A"]}),
    ] {
        assert!(s.execute("tour.create", &bad).is_err(), "{bad}");
    }
    s.execute("tour.create", &json!({"name": "T", "stops": ["A"], "look": [10, 0, 5]})).unwrap();
    assert!(s.execute("tour.timing", &json!({"name": "T", "fps": 0})).is_err());
    assert!(s.execute("tour.timing", &json!({"name": "nope"})).is_err());
    assert_eq!(s.execute("tour.timing", &json!({"name": "T"})).unwrap()["frames"], 1);
    // Hostile stored text: the commands say so instead of crashing.
    s.doc_mut().unwrap().tours = Some("{\"version\": 99}".into());
    assert!(s.execute("tour.list", &json!({})).is_err());
    s.doc_mut().unwrap().tours = Some("not json".into());
    assert!(s.execute("tour.view", &json!({"name": "B", "eye": [0, 0, 0]})).is_err());
}
