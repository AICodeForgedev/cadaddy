//! Editing a mixed selection from the Properties palette or Quick Properties: one type at a time
//! (`type`), and one coordinate at a time on several objects, each keeping its others.

use cadaddy_doc::EntityKind;
use cadaddy_engine::Session;
use serde_json::{Value, json};

fn h(r: &Value) -> String {
    r.get("handle").or_else(|| r["handles"].get(0)).and_then(Value::as_str).unwrap().to_string()
}

#[test]
fn a_type_in_a_mixed_selection_changes_only_those_objects() {
    let mut s = Session::new();
    let c1 = h(&s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap());
    let c2 = h(&s.execute("circle", &json!({"center": [10, 5], "radius": 2})).unwrap());
    let l1 = h(&s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap());
    let l2 = h(&s.execute("line", &json!({"points": [[0, 3], [5, 8]]})).unwrap());
    let all = json!([c1, c2, l1, l2]);
    // Radius on the circles only; the lines don't take it (nor anything else).
    let before_lines: Vec<_> =
        [&l1, &l2].iter().map(|x| s.doc().unwrap().entity(cadaddy_doc::Handle::parse_hex(x).unwrap()).unwrap().clone()).collect();
    s.execute("properties.set", &json!({"handles": all, "type": "Circle", "radius": 7})).unwrap();
    let get = |s: &Session, x: &str| s.doc().unwrap().entity(cadaddy_doc::Handle::parse_hex(x).unwrap()).unwrap().clone();
    for c in [&c1, &c2] {
        assert!(matches!(&get(&s, c).kind, EntityKind::Circle(k) if k.radius == 7.0));
    }
    assert_eq!(vec![get(&s, &l1), get(&s, &l2)], before_lines);
    // Colour on the lines only (case doesn't matter).
    s.execute("properties.set", &json!({"handles": all, "type": "line", "color": 1})).unwrap();
    assert_eq!(get(&s, &l1).common.color, cadaddy_color::Color::Index(1));
    assert_ne!(get(&s, &c1).common.color, cadaddy_color::Color::Index(1));
    // "All" (or no type): every object.
    s.execute("properties.set", &json!({"handles": all, "type": "All", "layer": "Shared"})).unwrap();
    assert!([&c1, &c2, &l1, &l2].iter().all(|x| get(&s, x).common.layer == "Shared"));
    // A type that isn't selected: said, not a parameter error, and nothing changes.
    let e = s.execute("properties.set", &json!({"handles": all, "type": "Text", "color": 3})).unwrap_err();
    assert!(e.to_string().contains("No Text objects are selected"), "{e}");
    assert!(!matches!(e, cadaddy_engine::EngineError::BadParams { .. }));
}

#[test]
fn one_coordinate_on_several_objects_keeps_each_ones_others() {
    let mut s = Session::new();
    let c1 = h(&s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap());
    let c2 = h(&s.execute("circle", &json!({"center": [10, 5], "radius": 2})).unwrap());
    let l1 = h(&s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap());
    let l2 = h(&s.execute("line", &json!({"points": [[1, 3], [5, 8]]})).unwrap());
    let get = |s: &Session, x: &str| s.doc().unwrap().entity(cadaddy_doc::Handle::parse_hex(x).unwrap()).unwrap().kind.clone();
    s.execute("properties.set", &json!({"handles": [c1, c2], "centerX": 4})).unwrap();
    assert!(matches!(get(&s, &c1), EntityKind::Circle(c) if (c.center.x, c.center.y, c.radius) == (4.0, 0.0, 1.0)));
    assert!(matches!(get(&s, &c2), EntityKind::Circle(c) if (c.center.x, c.center.y, c.radius) == (4.0, 5.0, 2.0)));
    s.execute("properties.set", &json!({"handles": [l1, l2], "startY": -2, "endX": 20})).unwrap();
    assert!(matches!(get(&s, &l1), EntityKind::Line(l) if (l.a.x, l.a.y, l.b.x, l.b.y) == (0.0, -2.0, 20.0, 0.0)));
    assert!(matches!(get(&s, &l2), EntityKind::Line(l) if (l.a.x, l.a.y, l.b.x, l.b.y) == (1.0, -2.0, 20.0, 8.0)));
}
