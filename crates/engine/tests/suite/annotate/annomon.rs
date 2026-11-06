//! The annotation monitor (ANNOMONITOR): a dimension whose object is erased loses its
//! association and carries a badge; DIMREASSOCIATE (attached again) or DIMDISASSOCIATE
//! (accepted) clears it.

use cadaddy_engine::Session;
use cadaddy_engine::assoc;
use cadaddy_engine::doc::{Dimension, EntityKind, Handle, Space};
use serde_json::{Value, json};

fn handle(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap()).unwrap()
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Handle {
    let v = s.execute("line", &json!({ "points": [a, b] })).unwrap();
    Handle::parse_hex(v["handles"][0].as_str().unwrap()).unwrap()
}

fn dim(s: &Session, h: Handle) -> Dimension {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Dimension(d) => (**d).clone(),
        other => panic!("not a dimension: {other:?}"),
    }
}

fn badges(s: &Session) -> Vec<Handle> {
    assoc::orphans(s.doc().unwrap(), &Space::Model).into_iter().map(|(h, _)| h).collect()
}

fn monitor(s: &mut Session) -> i64 {
    s.execute("getvar", &json!({ "name": "ANNOMONITOR" })).unwrap()["value"].as_i64().unwrap()
}

/// A line from (0,0) to (10,0) with an associative linear dimension on it.
fn dimensioned(s: &mut Session) -> (Handle, Handle) {
    let l = line(s, [0.0, 0.0], [10.0, 0.0]);
    let d = handle(&s.execute("dimlinear", &json!({ "p1": [0, 0], "p2": [10, 0], "at": [5, -2] })).unwrap());
    assert_eq!(dim(s, d).assoc.len(), 2);
    (l, d)
}

#[test]
fn erasing_the_object_raises_the_badge_and_turns_the_monitor_on() {
    let mut s = Session::new();
    assert_eq!(monitor(&mut s), -2, "off until a dimension loses its association");
    let (l, d) = dimensioned(&mut s);
    assert!(badges(&s).is_empty());
    s.execute("erase", &json!({ "handles": [l.hex()] })).unwrap();
    let dm = dim(&s, d);
    assert!(dm.assoc.is_empty());
    assert_eq!(dm.marks.orphaned, vec!["p13".to_string(), "p14".to_string()]);
    assert_eq!(badges(&s), vec![d]);
    assert_eq!(monitor(&mut s), 2);
    assert!(assoc::monitor_on(s.doc().unwrap()));
    // UNDO brings the line and the association back, and the badge goes.
    s.execute("undo", &json!({})).unwrap();
    assert!(badges(&s).is_empty());
    assert_eq!(dim(&s, d).assoc.len(), 2);
}

#[test]
fn reassociating_clears_the_badge() {
    let mut s = Session::new();
    let (l, d) = dimensioned(&mut s);
    s.execute("erase", &json!({ "handles": [l.hex()] })).unwrap();
    // Nothing under the points yet: still lost.
    let r = s.execute("dimreassociate", &json!({ "handles": [d.hex()] })).unwrap();
    assert_eq!(r["associated"], 0);
    assert_eq!(r["orphaned"], 1);
    assert_eq!(badges(&s), vec![d]);
    // A new line under the points: reassociating attaches them and clears the badge.
    let m = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let r = s.execute("dimreassociate", &json!({ "handles": [d.hex()] })).unwrap();
    assert_eq!(r["associated"], 1);
    assert_eq!(r["orphaned"], 0);
    let dm = dim(&s, d);
    assert!(dm.marks.orphaned.is_empty());
    assert!(dm.assoc.iter().all(|a| a.handle == m));
    assert!(badges(&s).is_empty());
}

#[test]
fn disassociating_accepts_the_lost_association() {
    let mut s = Session::new();
    let (l, d) = dimensioned(&mut s);
    s.execute("erase", &json!({ "handles": [l.hex()] })).unwrap();
    assert_eq!(badges(&s), vec![d]);
    let r = s.execute("dimdisassociate", &json!({ "handles": [d.hex()] })).unwrap();
    assert_eq!(r["disassociated"], 1);
    assert!(badges(&s).is_empty());
}

#[test]
fn erasing_one_of_two_objects_flags_only_that_point() {
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [0.0, 5.0]);
    let b = line(&mut s, [10.0, 0.0], [10.0, 5.0]);
    let d = handle(&s.execute("dimlinear", &json!({ "p1": [0, 0], "p2": [10, 0], "at": [5, -2] })).unwrap());
    assert_eq!(dim(&s, d).assoc.len(), 2);
    s.execute("erase", &json!({ "handles": [b.hex()] })).unwrap();
    let dm = dim(&s, d);
    assert_eq!(dm.marks.orphaned, vec!["p14".to_string()]);
    assert_eq!(dm.assoc.len(), 1);
    assert_eq!(dm.assoc.first().map(|x| x.handle), Some(a));
    // The remaining attachment still follows its line.
    s.execute("move", &json!({ "handles": [a.hex()], "from": [0, 0], "to": [1, 0] })).unwrap();
    assert!((dim(&s, d).p13.x - 1.0).abs() < 1e-9);
    assert_eq!(badges(&s), vec![d]);
}

#[test]
fn radius_dimension_of_an_erased_circle_is_flagged() {
    let mut s = Session::new();
    let c = handle(&s.execute("circle", &json!({ "center": [0, 0], "radius": 3 })).unwrap());
    let d = handle(&s.execute("dimradius", &json!({ "handle": c.hex(), "at": [5, 5] })).unwrap());
    assert!(!dim(&s, d).assoc.is_empty());
    s.execute("erase", &json!({ "handles": [c.hex()] })).unwrap();
    assert_eq!(badges(&s), vec![d]);
}

#[test]
fn stretching_a_point_off_its_object_is_not_a_lost_association() {
    let mut s = Session::new();
    let (l, d) = dimensioned(&mut s);
    // Moving the line along with the dimension keeps the association; editing the line follows it.
    s.execute("properties.set", &json!({ "handles": [l.hex()], "end": [12, 0] })).unwrap();
    assert!(badges(&s).is_empty());
    assert_eq!(dim(&s, d).assoc.len(), 2);
}

#[test]
fn annomonitor_off_keeps_the_flag_but_stays_off() {
    let mut s = Session::new();
    s.execute("setvar", &json!({ "name": "ANNOMONITOR", "value": 0 })).unwrap();
    let (l, d) = dimensioned(&mut s);
    s.execute("erase", &json!({ "handles": [l.hex()] })).unwrap();
    assert_eq!(monitor(&mut s), 0, "0 is off for good");
    assert!(!assoc::monitor_on(s.doc().unwrap()));
    // The flag is kept, so turning the monitor on shows the badge.
    assert_eq!(badges(&s), vec![d]);
    s.execute("setvar", &json!({ "name": "ANNOMONITOR", "value": 1 })).unwrap();
    assert!(assoc::monitor_on(s.doc().unwrap()));
    assert!(s.execute("setvar", &json!({ "name": "ANNOMONITOR", "value": 3 })).is_err());
}
