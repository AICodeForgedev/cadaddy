//! model.revise: the plan compared with the one the model was built from (or another version of
//! it), the model rebuilt, hand edits kept where they don't conflict and listed where they do.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// Feet → a line in inches on A-WALL.
fn line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
}

fn rules() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "kind": "wall"}]
    })
}

/// A 20' × 10' room of 6" walls, built (not associative unless `auto`).
fn built(auto: bool) -> Session {
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))] {
        line(&mut d, a, b);
    }
    for (a, b) in [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))] {
        line(&mut d, a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    s.execute("wallgen", &json!({"rules": rules(), "auto": auto})).unwrap();
    s
}

/// The plan lines on the east side (x > 200").
fn east(s: &Session) -> Vec<String> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).is_none() && matches!(&e.kind, EntityKind::Line(l) if l.a.x > 200.0 && l.b.x > 200.0))
        .map(|e| e.handle.hex())
        .collect()
}

/// The generated wall whose vertices all lie south (y ≤ 6"), and the one east (x ≥ 234").
fn wall(s: &Session, south: bool) -> Handle {
    s.doc()
        .unwrap()
        .model
        .iter()
        .find(|e| {
            made_as(e).as_deref() == Some("wall")
                && matches!(&e.kind, EntityKind::Mesh(m) if m.vertices.iter().all(|v| if south { v.y <= 6.0 + 1e-6 } else { v.x >= 234.0 - 1e-6 }))
        })
        .map(|e| e.handle)
        .expect("that wall")
}

fn recolor(s: &mut Session, h: Handle, aci: u8) {
    s.doc_mut().unwrap().modify_entity(h, |e| e.common.color = cadaddy_doc::color::Color::Index(aci)).unwrap();
}

#[test]
fn an_unchanged_plan_changes_nothing_and_a_moved_wall_is_reported_and_rebuilt() {
    let mut s = built(false);
    let r = s.execute("model.revise", &json!({})).unwrap();
    assert_eq!(
        (r["added"].as_array().unwrap().len(), r["removed"].as_array().unwrap().len(), r["moved"].as_array().unwrap().len()),
        (0, 0, 0),
        "{r}"
    );
    assert_eq!(r["rebuilt"], false);
    assert!(r["unchanged"].as_u64().unwrap() >= 8);
    // Move the east wall 2' further east.
    s.execute("move", &json!({"handles": east(&s), "from": [0, 0], "to": [24, 0]})).unwrap();
    let r = s.execute("model.revise", &json!({})).unwrap();
    let moved = r["moved"].as_array().unwrap();
    assert_eq!(moved.len(), 2, "both east wall lines: {r}");
    assert!(moved.iter().all(|m| m["kind"] == "wall" && m["at"][0].as_f64().unwrap() > m["from"][0].as_f64().unwrap()));
    assert_eq!(r["rebuilt"], true);
    // The rebuilt east wall reaches the new line, and is selected to show it.
    let changed = r["changed"].as_array().unwrap();
    assert!(!changed.is_empty());
    assert_eq!(s.selection().len(), changed.len());
    let max_x = s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).flat_map(|e| match &e.kind {
        EntityKind::Mesh(m) => m.vertices.iter().map(|v| v.x).collect::<Vec<_>>(),
        _ => vec![],
    });
    assert!(max_x.fold(f64::MIN, f64::max) >= 264.0 - 1e-6);
    // Revised again: nothing more.
    let r = s.execute("model.revise", &json!({})).unwrap();
    assert_eq!(r["rebuilt"], false, "{r}");
}

#[test]
fn hand_edits_stay_unless_their_plan_objects_changed() {
    let mut s = built(false);
    // Recolour the south wall (its plan lines won't change) and the east wall (they will).
    let (south, eastw) = (wall(&s, true), wall(&s, false));
    recolor(&mut s, south, 1);
    recolor(&mut s, eastw, 3);
    s.execute("move", &json!({"handles": east(&s), "from": [0, 0], "to": [24, 0]})).unwrap();
    let r = s.execute("model.revise", &json!({})).unwrap();
    let kept: Vec<&str> = r["kept"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(kept, vec![south.hex().as_str()], "{r}");
    let e = s.doc().unwrap().entity(south).unwrap().clone();
    assert_eq!(e.common.color, cadaddy_doc::color::Color::Index(1), "the edit is kept");
    // Only one south wall: the rebuilt twin gave way.
    let souths = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|x| made_as(x).as_deref() == Some("wall") && matches!(&x.kind, EntityKind::Mesh(m) if m.vertices.iter().all(|v| v.y <= 6.0 + 1e-6)))
        .count();
    assert_eq!(souths, 1);
    let conflicts = r["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1, "{r}");
    assert_eq!(conflicts[0]["change"], "edited");
}

#[test]
fn a_part_deleted_by_hand_stays_deleted() {
    let mut s = built(false);
    let south = wall(&s, true);
    let walls = |s: &Session| s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).count();
    let before = walls(&s);
    s.doc_mut().unwrap().remove_entity(south);
    s.execute("move", &json!({"handles": east(&s), "from": [0, 0], "to": [24, 0]})).unwrap();
    s.execute("model.revise", &json!({})).unwrap();
    assert_eq!(walls(&s), before - 1, "the deleted south wall isn't rebuilt");
}

#[test]
fn another_version_of_the_plan_and_associative_rebuilds() {
    // Against a saved earlier version: the line added since shows as added.
    let mut s = built(false);
    let dir = std::env::temp_dir().join(format!("cadaddy-revise-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let earlier = dir.join("earlier.dxf");
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    std::fs::write(&earlier, cadaddy_io::write(s.doc().unwrap(), "x.dxf").unwrap()).unwrap();
    line(s.doc_mut().unwrap(), (5.0, 0.5), (5.0, 9.5));
    line(s.doc_mut().unwrap(), (5.5, 0.5), (5.5, 9.5));
    let r = s.execute("model.revise", &json!({"against": earlier.display().to_string(), "rebuild": false})).unwrap();
    assert_eq!(r["added"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(r["moved"].as_array().unwrap().len(), 0);
    assert!(s.execute("model.revise", &json!({"against": dir.join("none.dxf").display().to_string()})).is_err());
    // Associative: a plan edit rebuilds at once, keeping a hand edit elsewhere.
    let mut a = built(true);
    let south = wall(&a, true);
    recolor(&mut a, south, 5);
    a.execute("move", &json!({"handles": east(&a), "from": [0, 0], "to": [24, 0]})).unwrap();
    assert_eq!(a.doc().unwrap().entity(south).map(|e| e.common.color), Some(cadaddy_doc::color::Color::Index(5)), "kept through the rebuild");
    // No model yet: an error.
    let mut n = Session::new();
    assert!(n.execute("model.revise", &json!({})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
