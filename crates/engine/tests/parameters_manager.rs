//! The Parameters Manager's engine side: expressions drive geometry through user parameters,
//! a parameter another one uses can't be deleted, renames carry into the expressions, and the
//! AutoConstrain settings (priority, shared intersections, name format).

use cadaddy_doc::{ConstraintKind as K, EntityKind, Handle};
use cadaddy_engine::Session;
use serde_json::{Value, json};

fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd} {p}: {e}"))
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) -> Handle {
    run(s, "line", json!({ "points": [a, b] }));
    s.doc().unwrap().model.iter().last().map(|e| e.handle).unwrap()
}

fn length(s: &Session, h: Handle) -> f64 {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Line(l) => l.a.xy().dist(l.b.xy()),
        k => panic!("{k:?}"),
    }
}

fn names(s: &mut Session) -> Vec<String> {
    run(s, "parameters", json!({}))["parameters"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap().to_string()).collect()
}

#[test]
fn expressions_drive_geometry_and_used_parameters_stay() {
    let mut s = Session::new();
    let l = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    run(&mut s, "dclinear", json!({ "h1": l.hex(), "value": 10, "name": "d1" }));
    run(&mut s, "parameters", json!({ "name": "w", "expr": "25", "description": "Overall width" }));
    run(&mut s, "parameters", json!({ "name": "d1", "expr": "w/2" }));
    assert!((length(&s, l) - 12.5).abs() < 1e-6, "{}", length(&s, l));
    run(&mut s, "parameters", json!({ "name": "w", "expr": "40" }));
    assert!((length(&s, l) - 20.0).abs() < 1e-6, "the line follows w");

    // w is used by d1: deleting it is refused, with the reason.
    let e = s.execute("parameters", &json!({ "delete": "w" })).unwrap_err().to_string();
    assert!(e.contains("`w` is used by `d1`"), "{e}");
    assert!(names(&mut s).contains(&"w".to_string()));
    // A parameter using d1 keeps d1 (and its constraint) too.
    run(&mut s, "parameters", json!({ "name": "h", "expr": "d1*2" }));
    let e = s.execute("parameters", &json!({ "delete": "d1" })).unwrap_err().to_string();
    assert!(e.contains("used by `h`"), "{e}");

    // A rename carries into every expression; the geometry stays.
    run(&mut s, "parameters", json!({ "rename": ["w", "width"] }));
    let rows = run(&mut s, "parameters", json!({}))["parameters"].clone();
    let d1 = rows.as_array().unwrap().iter().find(|r| r["name"] == "d1").unwrap().clone();
    assert_eq!(d1["expr"], "width/2");
    assert!((length(&s, l) - 20.0).abs() < 1e-6);
    for bad in
        [json!({ "rename": ["width", "h"] }), json!({ "rename": ["width", "2x"] }), json!({ "rename": ["nope", "x"] }), json!({ "rename": "x" })]
    {
        assert!(s.execute("parameters", &bad).is_err(), "{bad}");
    }
    // The description alone.
    run(&mut s, "parameters", json!({ "name": "width", "description": "Panel width" }));
    let rows = run(&mut s, "parameters", json!({}))["parameters"].clone();
    assert!(rows.as_array().unwrap().iter().any(|r| r["name"] == "width" && r["description"] == "Panel width" && r["expr"] == "40"));

    // Once nothing uses them, both go; deleting d1 deletes its constraint.
    run(&mut s, "parameters", json!({ "delete": "h" }));
    run(&mut s, "parameters", json!({ "delete": "d1" }));
    assert!(s.doc().unwrap().constraints.iter().all(|c| c.name != "d1"));
    run(&mut s, "parameters", json!({ "delete": "width" }));
    assert!(names(&mut s).is_empty());
    assert!(s.execute("parameters", &json!({ "delete": "width" })).is_err());
}

fn kinds(s: &Session) -> Vec<K> {
    let mut v: Vec<K> = s.doc().unwrap().constraints.iter().map(|c| c.kind).collect();
    v.sort_by_key(|k| format!("{k:?}"));
    v
}

#[test]
fn autoconstrain_follows_priority_and_the_intersection_rule() {
    // An L: AutoCAD's default priority (Perpendicular before Horizontal and Vertical) keeps
    // Perpendicular; Vertical is then redundant.
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let b = line(&mut s, [10.0, 0.0], [10.0, 8.0]);
    run(&mut s, "autoconstrain", json!({ "handles": [a.hex(), b.hex()] }));
    let default = kinds(&s);
    assert!(default.contains(&K::Perpendicular) && !default.contains(&K::Vertical), "{default:?}");
    // Horizontal and Vertical first: both are applied, and Perpendicular is the redundant one.
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let b = line(&mut s, [10.0, 0.0], [10.0, 8.0]);
    run(&mut s, "constraintsettings", json!({ "autoTypes": ["Coincident", "Horizontal", "Vertical", "Perpendicular"] }));
    run(&mut s, "autoconstrain", json!({ "handles": [a.hex(), b.hex()] }));
    let first = kinds(&s);
    assert!(first.contains(&K::Vertical) && first.contains(&K::Horizontal) && !first.contains(&K::Perpendicular), "{first:?}");

    // Perpendicular lines that don't meet: only without the shared-intersection rule.
    let mut s = Session::new();
    let a = line(&mut s, [0.0, 0.0], [10.0, 0.0]);
    let b = line(&mut s, [20.0, 5.0], [20.0, 15.0]);
    run(&mut s, "constraintsettings", json!({ "autoTypes": ["Perpendicular"] }));
    run(&mut s, "autoconstrain", json!({ "handles": [a.hex(), b.hex()] }));
    assert!(kinds(&s).is_empty(), "{:?}", kinds(&s));
    let r = run(&mut s, "constraintsettings", json!({ "perpendicularIntersect": false }));
    assert_eq!(r["perpendicularIntersect"], false);
    run(&mut s, "autoconstrain", json!({ "handles": [a.hex(), b.hex()] }));
    assert_eq!(kinds(&s), vec![K::Perpendicular]);
}

#[test]
fn constraint_name_format() {
    let mut s = Session::new();
    assert_eq!(run(&mut s, "getvar", json!({ "name": "CONSTRAINTNAMEFORMAT" }))["value"], 2);
    run(&mut s, "setvar", json!({ "name": "CONSTRAINTNAMEFORMAT", "value": 1 }));
    assert_eq!(s.doc().unwrap().parametric.settings.name_format, 1);
    assert_eq!(run(&mut s, "constraintsettings", json!({ "nameFormat": 0 }))["nameFormat"], 0);
    assert!(s.execute("setvar", &json!({ "name": "CONSTRAINTNAMEFORMAT", "value": 3 })).is_err());
    assert!(s.execute("constraintsettings", &json!({ "nameFormat": 9 })).is_err());
}
