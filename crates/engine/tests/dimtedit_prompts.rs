//! DIMTEDIT at the command line and in scripts: "Select dimension", then a new text location or
//! Left/Right/Center/Home/Angle.

use cadcraft_engine::Session;
use cadcraft_engine::doc::{Dimension, EntityKind};
use cadcraft_engine::geom::Vec2;

fn the_dim(s: &Session) -> Dimension {
    s.doc()
        .unwrap()
        .model
        .iter()
        .find_map(|e| match &e.kind {
            EntityKind::Dimension(d) => Some(d.clone()),
            _ => None,
        })
        .unwrap()
}

#[test]
fn dimtedit_moves_justifies_rotates_and_homes_typed_text() {
    let mut s = Session::new();
    s.script("dimlinear 0,0 10,0 5,3\n").unwrap();
    // Pick the dimension on its dimension line, then a new text location.
    s.script("dimtedit 2,3\n").unwrap();
    let p = s.prompt_text();
    assert!(p.contains("Specify new location for dimension text") && p.contains("[Left/Right/Center/Home/Angle]"), "{p}");
    s.script("7,6\n").unwrap();
    assert!(s.running.is_none());
    let d = the_dim(&s);
    assert!(d.user_text_pos && d.text_mid.xy().dist(Vec2::new(7.0, 6.0)) < 1e-9, "{:?}", d.text_mid);

    s.script("dimtedit 2,3 h\n").unwrap();
    assert!(!the_dim(&s).user_text_pos, "Home puts the text back");

    s.script("dimtedit 2,3 l\n").unwrap();
    assert_eq!(the_dim(&s).overrides.get("textJust").and_then(|v| v.as_i64()), Some(1));

    s.script("dimtedit 2,3 a 30\n").unwrap();
    assert!(s.running.is_none());
    assert!((the_dim(&s).text_rotation - 30f64.to_radians()).abs() < 1e-9);

    // Something other than a dimension is refused and the prompt stays.
    s.script("line 20,20 30,20\n\ndimtedit 25,20\n").unwrap();
    assert!(s.prompt_text().contains("Select dimension"), "{}", s.prompt_text());
    s.script("2,3 c\n").unwrap();
    assert!(s.running.is_none());
    assert_eq!(the_dim(&s).overrides.get("textJust").and_then(|v| v.as_i64()), Some(0));
}
