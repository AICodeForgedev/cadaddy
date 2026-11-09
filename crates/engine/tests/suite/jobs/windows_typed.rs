//! Window tags become typed windows in the outside walls that hold them (Q6311): the opening a
//! tag marks is found whichever way its wall pieces were drawn, a tag by a wall drawn through
//! gets its window cut there (inferred), each window carries its type, size, sill and room, and
//! the build reports the tags against the windows per level and type.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Insert, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
}

fn text(s: &mut Session, layer: &str, at: [f64; 2], t: &str) {
    if s.execute("layer.new", &json!({"name": layer, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": layer})).expect("layer");
    }
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
}

/// A 24' × 12' office of 6" walls (feet). Its south wall has a 4' opening (x 10–14), each face
/// drawn as two pieces that start at the opening and run away from it (so neither piece ends at
/// it), its jambs drawn and glazing across it; its north wall runs through. `tags`: (x, y feet, text) on the plan; `rows`: the window
/// schedule's rows. Built, its rooms traced and its properties filled.
fn office(tags: &[(f64, f64, &str)], rows: Value) -> (Session, Value) {
    office_with(tags, &[], rows)
}

/// [`office`], with `symbols`: tag symbols as blocks whose attribute holds the mark (x, y feet,
/// mark), as a print's tag symbols come in.
fn office_with(tags: &[(f64, f64, &str)], symbols: &[(f64, f64, &str)], rows: Value) -> (Session, Value) {
    build(tags, symbols, rows, North::Through)
}

/// [`office`], its north wall drawn as two pieces in line that meet at x = 6' (a 6" piece west of
/// it, a 5 1/2" piece east of it, their inside faces apart), as two drawings of one wall meet.
fn office_north_in_pieces(tags: &[(f64, f64, &str)], rows: Value) -> (Session, Value) {
    build(tags, &[], rows, North::Pieces)
}

/// [`office`], its north wall drawn as two pieces a 3" slot apart at x = 6' (a joint, a post),
/// narrower than any opening.
fn office_north_slot(tags: &[(f64, f64, &str)], rows: Value) -> (Session, Value) {
    build(tags, &[], rows, North::Slot)
}

/// [`office`], with a 6" piece of wall 4' long standing 1' inside its north wall at x 4–8'.
fn office_north_sleeve(tags: &[(f64, f64, &str)], rows: Value) -> (Session, Value) {
    build(tags, &[], rows, North::ThroughWithSleeve)
}

/// How the office's north wall is drawn.
#[derive(Clone, Copy, PartialEq)]
enum North {
    Through,
    /// Drawn through, a 6" piece of wall 4' long standing 1' inside it at x 4–8' (a unit's
    /// sleeve, a furred stretch).
    ThroughWithSleeve,
    Pieces,
    Slot,
}

fn build(tags: &[(f64, f64, &str)], symbols: &[(f64, f64, &str)], rows: Value, north: North) -> (Session, Value) {
    let mut d = Drawing::new_imperial();
    for (x, y, mark) in symbols {
        let at = json!({"x": x * 12.0, "y": y * 12.0, "z": 0.0});
        let insert: Insert = serde_json::from_value(json!({
            "block": "TAG-SYMBOL", "insert": at,
            "attribs": [{"tag": "TAG", "text": {"insert": at, "height": 6.0, "value": mark}}]
        }))
        .expect("insert");
        let h = Handle(d.handseed.max(0x100));
        d.handseed = h.0 + 1;
        d.model.push(Entity::new(h, EntityKind::Insert(insert)));
    }
    for y in [0.0, 0.5] {
        line(&mut d, (10.0, y), (if y == 0.0 { 0.0 } else { 0.5 }, y));
        line(&mut d, (14.0, y), (if y == 0.0 { 24.0 } else { 23.5 }, y));
    }
    // The opening's jambs.
    line(&mut d, (10.0, 0.0), (10.0, 0.5));
    line(&mut d, (14.0, 0.0), (14.0, 0.5));
    let slot = (6.0 - 1.5 / 12.0, 6.0 + 1.5 / 12.0);
    if north == North::Slot {
        for (a, b) in [((24., 0.), (24., 12.)), ((24., 12.), (slot.1, 12.)), ((slot.0, 12.), (0., 12.)), ((0., 12.), (0., 0.))] {
            line(&mut d, a, b);
        }
    } else {
        for (a, b) in [((24., 0.), (24., 12.)), ((24., 12.), (0., 12.)), ((0., 12.), (0., 0.))] {
            line(&mut d, a, b);
        }
    }
    if north == North::Slot {
        for (a, b) in [((23.5, 0.5), (23.5, 11.5)), ((23.5, 11.5), (slot.1, 11.5)), ((slot.0, 11.5), (0.5, 11.5)), ((0.5, 11.5), (0.5, 0.5))] {
            line(&mut d, a, b);
        }
    } else if north == North::Pieces {
        for (a, b) in [
            ((23.5, 0.5), (23.5, 11.5 + 1.0 / 24.0)),
            ((23.5, 11.5 + 1.0 / 24.0), (6.0, 11.5 + 1.0 / 24.0)),
            ((6.0, 11.5), (0.5, 11.5)),
            ((0.5, 11.5), (0.5, 0.5)),
        ] {
            line(&mut d, a, b);
        }
    } else {
        for (a, b) in [((23.5, 0.5), (23.5, 11.5)), ((23.5, 11.5), (0.5, 11.5)), ((0.5, 11.5), (0.5, 0.5))] {
            line(&mut d, a, b);
        }
    }
    if north == North::ThroughWithSleeve {
        for (a, b) in [((4.0, 10.5), (8.0, 10.5)), ((8.0, 10.0), (4.0, 10.0)), ((4.0, 10.0), (4.0, 10.5)), ((8.0, 10.0), (8.0, 10.5))] {
            line(&mut d, a, b);
        }
    }
    // The window's glazing drawn across the opening, between the wall's faces.
    for y in [0.2, 0.3] {
        let h = Handle(d.handseed.max(0x100));
        d.handseed = h.0 + 1;
        let mut e = Entity::new(h, EntityKind::Line(Line { a: Vec3 { x: 120.0, y: y * 12.0, z: 0.0 }, b: Vec3 { x: 168.0, y: y * 12.0, z: 0.0 } }));
        e.common.layer = "A-GLAZ".into();
        d.model.push(e);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    text(&mut s, "A-ROOM-TAG", [120.0, 80.0], "OFFICE");
    text(&mut s, "A-ROOM-TAG", [126.0, 68.0], "905");
    for (x, y, t) in tags {
        text(&mut s, "A-ANNO", [x * 12.0, y * 12.0], t);
    }
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"}, {"layer": "A-GLAZ", "role": "window"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Exterior", "thickness_in": 6}],
        "schedule": {"rows": rows},
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({})).expect("build");
    s.execute("rooms.build", &json!({})).expect("rooms");
    s.execute("bim.props.fill", &json!({})).expect("fill");
    (s, built)
}

/// The windows built: (handle, props).
fn windows(s: &mut Session) -> Vec<(String, Value)> {
    let hs: Vec<String> = s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("window")).map(|e| e.handle.hex()).collect();
    hs.into_iter().map(|h| (h.clone(), s.execute("bim.props.get", &json!({"handle": h})).expect("get")["elements"][0]["props"].clone())).collect()
}

fn value(props: &Value, key: &str) -> Value {
    props[key]["value"].clone()
}

#[test]
fn a_tagged_opening_is_found_whichever_way_its_pieces_run_and_the_window_is_typed() {
    let rows = json!([{"kind": "window", "mark": "W1", "width_in": 48, "height_in": 48, "sill_ft": 3, "type": "FIXED ALUMINUM", "frame": "AL"}]);
    let (mut s, built) = office(&[(11.0, -1.5, "W1")], rows);
    let ws = windows(&mut s);
    assert_eq!(ws.len(), 1, "the tagged opening is a window: {built}");
    let p = &ws[0].1;
    assert_eq!(value(p, "type_mark"), json!("W1"), "{p}");
    assert_eq!(value(p, "width"), json!(48.0), "{p}");
    assert_eq!(value(p, "sill_height"), json!(36.0), "{p}");
    assert_eq!(value(p, "height"), json!(48.0), "{p}");
    assert_eq!(p["sill_height"]["source"]["kind"], json!("schedule"), "{p}");
    assert_eq!(value(p, "type"), json!("FIXED ALUMINUM"), "{p}");
    assert_eq!(value(p, "frame"), json!("AL"), "{p}");
    assert!(value(p, "room").as_str().is_some_and(|r| r.contains("905")), "the room it lights: {p}");
    assert_eq!(value(p, "level"), json!("L1"), "{p}");
    // The report: one W1 tagged, one built.
    let row = &built["window_tags"][0];
    assert_eq!((row["level"].clone(), row["tagged"].clone(), row["built"].clone()), (json!("L1"), json!({"W1": 1}), json!({"W1": 1})), "{row}");
    assert_eq!(row["no_host_count"], json!(0), "{row}");
    assert_eq!(row["marks_without_schedule_row"], json!([]), "{row}");
}

#[test]
fn a_tag_by_a_wall_drawn_through_is_its_window_there_and_a_tag_with_no_wall_is_reported() {
    // W1 by the opening, W1 again by the north wall (drawn through), W9 in the middle of the room.
    let (mut s, built) = office(&[(11.0, -1.5, "W1"), (5.0, 13.0, "W1"), (12.0, 6.0, "W9")], json!([]));
    let ws = windows(&mut s);
    assert_eq!(ws.len(), 2, "{built}");
    // No schedule row: the width is the tagged opening's (4'), sill and height the defaults.
    for (_, p) in &ws {
        assert_eq!(value(p, "type_mark"), json!("W1"), "{p}");
        assert_eq!(value(p, "width"), json!(48.0), "{p}");
        assert_eq!(p["sill_height"]["source"]["kind"], json!("default"), "{p}");
    }
    // The north one is inferred: its provenance says so, and why.
    let r = s.execute("bim.provenance", &json!({"part": "window", "list": 10})).expect("provenance");
    let made: Vec<(String, String)> = r["listed"]
        .as_array()
        .expect("listed")
        .iter()
        .map(|e| (e["made"].as_str().unwrap_or("").to_string(), e["basis"].as_str().unwrap_or("").to_string()))
        .collect();
    assert_eq!(made.iter().filter(|(m, _)| m == "inferred").count(), 1, "{made:?}");
    assert!(made.iter().any(|(m, b)| m == "inferred" && b.contains("window tag")), "{made:?}");
    let row = &built["window_tags"][0];
    assert_eq!((row["tagged"].clone(), row["built"].clone()), (json!({"W1": 2, "W9": 1}), json!({"W1": 2})), "{row}");
    assert_eq!(row["by_basis"], json!({"tag": 1, "tag_on_wall": 1}), "{row}");
    assert_eq!(row["no_host_count"], json!(1), "{row}");
    assert_eq!(row["no_host"][0]["mark"], json!("W9"), "{row}");
    assert_eq!(row["marks_without_schedule_row"], json!(["W1", "W9"]), "{row}");
}

#[test]
fn a_storefront_tag_stands_on_the_floor() {
    let (mut s, built) = office(&[(11.0, -1.5, "SF1")], json!([]));
    let ws = windows(&mut s);
    assert_eq!(ws.len(), 1, "{built}");
    let p = &ws[0].1;
    assert_eq!(value(p, "assembly"), json!("storefront"), "{p}");
    assert_eq!(value(p, "sill_height"), json!(0.0), "{p}");
    assert_eq!(built["window_tags"][0]["storefront"], json!(1), "{built}");
}

#[test]
fn a_tag_symbols_attribute_is_a_window_tag() {
    // The mark in a tag block's attribute, not a text (Q6311: the print's window tags all came in
    // so, and none was read).
    let (mut s, built) = office_with(&[], &[(11.0, -1.5, "W1")], json!([]));
    let ws = windows(&mut s);
    assert_eq!(ws.len(), 1, "{built}");
    assert_eq!(value(&ws[0].1, "type_mark"), json!("W1"), "{}", ws[0].1);
    assert_eq!(built["window_tags"][0]["tagged"], json!({"W1": 1}), "{built}");
}

#[test]
fn a_tag_where_two_pieces_of_its_wall_meet_is_its_window_across_the_joint() {
    // W1 by the south opening (4'), W1 again by the north wall at x = 6', where its two pieces
    // meet: the window (4..8') spans the joint, cut out of both pieces (Q6311: the print's
    // outside walls came in pieces of a few feet, and tags by a joint found no wall).
    let (mut s, built) = office_north_in_pieces(&[(11.0, -1.5, "W1"), (6.0, 13.0, "W1")], json!([]));
    // The north wall line is read as more than one piece (else the test shows nothing).
    let plan = s.execute("wallgen.plan", &json!({"level": "L1"})).expect("plan");
    let north: Vec<&Value> = plan["walls"]
        .as_array()
        .expect("walls")
        .iter()
        .filter(|w| w["a"][1].as_f64().zip(w["b"][1].as_f64()).is_some_and(|(a, b)| a > 130.0 && b > 130.0))
        .collect();
    assert!(north.len() >= 2, "the north wall in pieces: {north:?}");
    let ws = windows(&mut s);
    assert_eq!(ws.len(), 2, "{built}");
    let row = &built["window_tags"][0];
    assert_eq!(row["no_host_count"], json!(0), "{row}");
    assert_eq!((row["tagged"].clone(), row["built"].clone()), (json!({"W1": 2}), json!({"W1": 2})), "{row}");
    for (_, p) in &ws {
        assert_eq!(value(p, "width"), json!(48.0), "{p}");
    }
}

#[test]
fn a_tag_at_a_slot_between_two_pieces_of_its_wall_is_its_window_across_the_slot() {
    // W1 by the south opening (4'), W1 again by the north wall at x = 6', where its two pieces
    // stand a 3" slot apart (narrower than any opening: built solid): the window (4..8') spans the
    // slot (Q6311: a print's outside wall pieces a few inches apart, every tag there had no wall).
    let (mut s, built) = office_north_slot(&[(11.0, -1.5, "W1"), (6.0, 13.0, "W1")], json!([]));
    let ws = windows(&mut s);
    assert_eq!(ws.len(), 2, "{built}");
    let row = &built["window_tags"][0];
    assert_eq!(row["no_host_count"], json!(0), "{row}");
    assert_eq!(row["by_basis"], json!({"tag": 1, "tag_on_wall": 1}), "{row}");
}

#[test]
fn a_window_behind_a_piece_of_wall_inside_lights_the_room_past_it() {
    // W1 by the north wall drawn through, a piece of wall standing 1' inside it there: the room's
    // outline keeps off the wall at the window, the window still lights that room (Q6311: a
    // print's windows by units' sleeves had no room).
    let (mut s, built) = office_north_sleeve(&[(11.0, -1.5, "W1"), (6.0, 13.0, "W1")], json!([]));
    let ws = windows(&mut s);
    assert_eq!(ws.len(), 2, "{built}");
    for (_, p) in &ws {
        assert!(value(p, "room").as_str().is_some_and(|r| r.contains("905")), "the room it lights: {p}");
    }
}
