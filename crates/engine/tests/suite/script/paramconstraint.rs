//! PARAMCONSTRAINT: associative dimensions become dimensional constraints and back, and the
//! geometry stays where it is.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{ConstraintKind, DistAxis, EntityKind, Handle};
use serde_json::{Value, json};

fn handle(v: &Value) -> String {
    v["handle"].as_str().unwrap().to_string()
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> String {
    s.execute("line", &json!({ "points": [a, b] })).unwrap()["handles"][0].as_str().unwrap().to_string()
}

fn dims(s: &Session) -> usize {
    s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Dimension(_))).count()
}

fn kinds(s: &Session) -> Vec<ConstraintKind> {
    s.doc().unwrap().constraints.iter().filter(|c| c.kind.is_dimensional()).map(|c| c.kind).collect()
}

#[test]
fn dimensions_become_constraints() {
    let mut s = Session::new();
    let base = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let up = line(&mut s, [0.0, 0.0], [3.0, 4.0]);
    let c = handle(&s.execute("circle", &json!({ "center": [20, 0], "radius": 2 })).unwrap());
    let h = handle(&s.execute("dimlinear", &json!({ "p1": [0, 0], "p2": [10, 0], "at": [5, -2] })).unwrap());
    let a = handle(&s.execute("dimaligned", &json!({ "p1": [0, 0], "p2": [3, 4], "at": [0, 5] })).unwrap());
    let r = handle(&s.execute("dimradius", &json!({ "handle": c, "at": [23, 2] })).unwrap());
    let c2 = handle(&s.execute("circle", &json!({ "center": [30, 0], "radius": 1.5 })).unwrap());
    let d = handle(&s.execute("dimdiameter", &json!({ "handle": c2, "at": [28, -2] })).unwrap());
    assert_eq!(dims(&s), 4);
    let out = s.execute("paramconstraint", &json!({ "handles": [h, a, r, d] })).unwrap();
    // A second dimension on a constrained circle would over-constrain it: it stays a dimension.
    let again = handle(&s.execute("dimdiameter", &json!({ "handle": c, "at": [17, -2] })).unwrap());
    let twice = s.execute("paramconstraint", &json!({ "handles": [again] })).unwrap();
    assert_eq!(twice["converted"], 0);
    assert_eq!(twice["skipped"].as_array().unwrap().len(), 1);
    s.execute("erase", &json!({ "handles": [again] })).unwrap();
    assert_eq!(out["converted"], 4, "{out}");
    assert_eq!(dims(&s), 0, "the dimensions are replaced");
    let k = kinds(&s);
    assert!(k.contains(&ConstraintKind::Distance(DistAxis::Horizontal)));
    assert!(k.contains(&ConstraintKind::Distance(DistAxis::Aligned)));
    assert!(k.contains(&ConstraintKind::Radius) && k.contains(&ConstraintKind::Diameter));
    let values: Vec<String> = out["constraints"].as_array().unwrap().iter().map(|c| c["expr"].as_str().unwrap().to_string()).collect();
    assert_eq!(values, vec!["10", "5", "2", "3"]);
    // The geometry didn't move.
    let l = s.doc().unwrap().entity(Handle::parse_hex(&up).unwrap()).unwrap().kind.clone();
    let EntityKind::Line(l) = l else { panic!("a line") };
    assert!((l.b.x - 3.0).abs() < 1e-9 && (l.b.y - 4.0).abs() < 1e-9);
    // A constraint now drives the geometry: changing it moves the line.
    let name = out["constraints"][1]["name"].as_str().unwrap().to_string();
    s.execute("parameters", &json!({ "name": name, "expr": "10" })).unwrap();
    let line_of = |s: &Session, h: &str| match s.doc().unwrap().entity(Handle::parse_hex(h).unwrap()).unwrap().kind.clone() {
        EntityKind::Line(l) => l,
        other => panic!("not a line: {other:?}"),
    };
    // The aligned constraint measures from the first line's start to the second line's end.
    let (first, second) = (line_of(&s, &base), line_of(&s, &up));
    assert!((first.a.xy().dist(second.b.xy()) - 10.0).abs() < 1e-6);
}

#[test]
fn plain_and_unmatched_dimensions_are_skipped() {
    let mut s = Session::new();
    s.execute("setvar", &json!({ "name": "DIMASSOC", "value": 0 })).unwrap();
    line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let plain = handle(&s.execute("dimlinear", &json!({ "p1": [0, 0], "p2": [10, 0], "at": [5, -2] })).unwrap());
    let other = line(&mut s, [0.0, 5.0], [1.0, 5.0]);
    let out = s.execute("paramconstraint", &json!({ "handles": [plain, other] })).unwrap();
    assert_eq!(out["converted"], 0);
    assert_eq!(out["skipped"].as_array().unwrap().len(), 2);
    assert_eq!(dims(&s), 1, "nothing removed");
    assert!(s.execute("paramconstraint", &json!({ "handles": [] })).is_err());
    assert!(s.execute("paramconstraint", &json!({ "handles": "x" })).is_err());
    assert!(s.execute("paramconstraint", &json!({ "to": "sideways" })).is_err());
}

#[test]
fn constraints_become_associative_dimensions() {
    let mut s = Session::new();
    let l = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let c = handle(&s.execute("circle", &json!({ "center": [20, 0], "radius": 2 })).unwrap());
    s.execute("dimconstraint", &json!({ "type": "horizontal", "h1": l, "expr": "10" })).unwrap();
    s.execute("dimconstraint", &json!({ "type": "radius", "h1": c, "expr": "2" })).unwrap();
    let out = s.execute("paramconstraint", &json!({ "to": "dimension" })).unwrap();
    assert_eq!(out["converted"], 2, "{out}");
    assert!(kinds(&s).is_empty());
    assert_eq!(dims(&s), 2);
    // The new dimensions are associative: moving the line's end moves the dimension.
    let d = s.doc().unwrap().model.iter().find_map(|e| match &e.kind {
        EntityKind::Dimension(dm) if !dm.assoc.is_empty() && matches!(dm.kind, cadaddy_engine::doc::DimKind::Linear { .. }) => Some(e.handle),
        _ => None,
    });
    let d = d.expect("an associative linear dimension");
    s.execute("properties.set", &json!({ "handles": [l], "end": [12, 0] })).unwrap();
    let EntityKind::Dimension(dm) = &s.doc().unwrap().entity(d).unwrap().kind else { panic!() };
    assert!((dm.p14.x - 12.0).abs() < 1e-9 || (dm.p13.x - 12.0).abs() < 1e-9);
}

#[test]
fn a_constraint_another_one_uses_stays() {
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let b = line(&mut s, [0.0, 5.0], [5.0, 5.0]);
    s.execute("dimconstraint", &json!({ "type": "horizontal", "h1": a, "expr": "10", "name": "d1" })).unwrap();
    s.execute("dimconstraint", &json!({ "type": "horizontal", "h1": b, "expr": "d1/2", "name": "d2" })).unwrap();
    let out = s.execute("paramconstraint.todimension", &json!({ "constraints": ["d1", "d2"] })).unwrap();
    assert_eq!(out["converted"], 1, "{out}");
    assert_eq!(out["skipped"][0]["item"], "d1");
    assert!(s.execute("paramconstraint.todimension", &json!({ "constraints": ["nope"] })).is_err());
    // Round trip: the dimension made from d2 converts back.
    let d = s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::Dimension(_))).map(|e| e.handle.hex()).unwrap();
    let out = s.execute("paramconstraint", &json!({ "handles": [d] })).unwrap();
    assert_eq!(out["converted"], 1, "{out}");
    assert_eq!(out["constraints"][0]["expr"], "5");
}
