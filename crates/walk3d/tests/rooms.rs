//! Room names on the plans, placed in the building (walk mode's "go to room").

use cadaddy_doc::geom::Vec3;
use cadaddy_doc::{Drawing, Entity, EntityKind, Line, Text};
use cadaddy_walk3d::{Rules, rooms};

fn add(d: &mut Drawing, layer: &str, kind: EntityKind) {
    let h = d.new_handle();
    let mut e = Entity { handle: h, common: Default::default(), kind };
    e.common.layer = layer.into();
    d.model.push(e);
}

fn text(d: &mut Drawing, x: f64, y: f64, s: &str) {
    let t: Text = serde_json::from_value(serde_json::json!({"insert": {"x": x, "y": y, "z": 0.0}, "height": 6.0, "value": s})).unwrap();
    add(d, "A-ANNO", EntityKind::Text(t));
}

#[test]
fn room_names_are_found_on_their_level_in_feet() {
    let mut d = Drawing::default();
    // A 40' × 10' outline drawn in inches, lower-left at (120, 120).
    for (a, b) in [((120., 120.), (600., 120.)), ((600., 120.), (600., 240.)), ((600., 240.), (120., 240.)), ((120., 240.), (120., 120.))] {
        add(&mut d, "A-WALL", EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) }));
    }
    text(&mut d, 180.0, 180.0, "LOBBY");
    text(&mut d, 480.0, 180.0, "  Office   101 ");
    text(&mut d, 300.0, 180.0, "12");
    text(&mut d, 300.0, 200.0, "LOBBY");
    let r = rooms(&[&d], &Rules::fallback());
    let names: Vec<&str> = r.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["LOBBY", "Office 101"], "numbers alone are not rooms; a name once per level");
    // In feet from the walls' lower-left corner.
    assert!((r[1].at.x - 30.0).abs() < 1e-9 && (r[1].at.y - 5.0).abs() < 1e-9, "{:?}", r[1].at);
    assert_eq!(r[1].level, 0);
}
