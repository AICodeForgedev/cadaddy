//! Every door schedule row becomes a door (Q6609): a door the schedule lists and a plan tags
//! where the plan draws no opening (a counter shutter over a counter top, a sliding door drawn
//! as a line, a door in a wall drawn through) is built at its tag, at the schedule's size, its
//! sill and how it opens from the schedule's words else its size, marked inferred with its basis;
//! a person places one by mark, position and size (`plan.door`), and every build builds it again.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Arc, Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = layer.into();
    d.model.push(e);
}

fn text(s: &mut Session, layer: &str, at: [f64; 2], t: &str) {
    if s.execute("layer.new", &json!({"name": layer, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": layer})).expect("layer");
    }
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
}

/// A 30' × 16' building of 6" walls (feet) with a 6" partition at x = 15' drawn through from wall
/// to wall (no opening in it), a front desk's counter edge drawn as a line in the east room at
/// y = 4' (x 20–28'), no door swing anywhere. `rows`: the door schedule; `tags`: its tags (mark,
/// x, y feet). Built, its rooms traced and its properties filled.
fn building(rows: Value, tags: &[(&str, f64, f64)]) -> (Session, Value) {
    building_with(rows, tags, false)
}

/// [`building`]; with `swing`, the partition has a 3' door drawn in it at y 12–15' (jambs, the
/// open leaf and its swing into the east room).
fn building_with(rows: Value, tags: &[(&str, f64, f64)], swing: bool) -> (Session, Value) {
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0., 0.), (30., 0.)), ((30., 0.), (30., 16.)), ((30., 16.), (0., 16.)), ((0., 16.), (0., 0.))] {
        line(&mut d, "A-WALL", a, b);
    }
    for (a, b) in [((0.5, 0.5), (14.75, 0.5)), ((15.25, 0.5), (29.5, 0.5)), ((29.5, 0.5), (29.5, 15.5)), ((29.5, 15.5), (15.25, 15.5))] {
        line(&mut d, "A-WALL", a, b);
    }
    for (a, b) in [((14.75, 15.5), (0.5, 15.5)), ((0.5, 15.5), (0.5, 0.5))] {
        line(&mut d, "A-WALL", a, b);
    }
    let breaks: &[(f64, f64)] = if swing { &[(0.5, 12.0), (15.0, 15.5)] } else { &[(0.5, 15.5)] };
    for x in [14.75, 15.25] {
        for (y0, y1) in breaks {
            line(&mut d, "A-WALL", (x, *y0), (x, *y1));
        }
    }
    if swing {
        line(&mut d, "A-WALL", (14.75, 12.0), (15.25, 12.0));
        line(&mut d, "A-WALL", (14.75, 15.0), (15.25, 15.0));
        line(&mut d, "A-DOOR", (15.25, 15.0), (18.25, 15.0));
        let h = Handle(d.handseed.max(0x100));
        d.handseed = h.0 + 1;
        let mut e = Entity::new(
            h,
            EntityKind::Arc(Arc {
                center: Vec3 { x: 15.25 * 12.0, y: 15.0 * 12.0, z: 0.0 },
                radius: 36.0,
                start: 1.5 * std::f64::consts::PI,
                end: 0.0,
            }),
        );
        e.common.layer = "A-DOOR".into();
        d.model.push(e);
    }
    line(&mut d, "A-FLOR-CASE", (20.0, 4.0), (28.0, 4.0));
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    text(&mut s, "A-ROOM-TAG", [60.0, 100.0], "LOBBY");
    text(&mut s, "A-ROOM-TAG", [66.0, 88.0], "101");
    text(&mut s, "A-ROOM-TAG", [260.0, 140.0], "OFFICE");
    text(&mut s, "A-ROOM-TAG", [266.0, 128.0], "102");
    let tags: Vec<Value> = tags.iter().map(|(m, x, y)| json!({"mark": m, "at": [x * 12.0, y * 12.0], "level": "L1"})).collect();
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-DOOR", "role": "door"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "schedule": {"rows": rows, "tags": tags},
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({})).expect("build");
    s.execute("rooms.build", &json!({})).expect("rooms");
    s.execute("bim.props.fill", &json!({})).expect("fill");
    (s, built)
}

/// The doors built: (handle, props).
fn doors(s: &mut Session) -> Vec<(String, Value)> {
    let hs: Vec<String> = s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("door")).map(|e| e.handle.hex()).collect();
    hs.into_iter().map(|h| (h.clone(), s.execute("bim.props.get", &json!({"handle": h})).expect("get")["elements"][0]["props"].clone())).collect()
}

fn value(props: &Value, key: &str) -> Value {
    props[key]["value"].clone()
}

/// The door numbered `mark`.
fn door<'a>(ds: &'a [(String, Value)], mark: &str) -> Option<&'a Value> {
    ds.iter().map(|(_, p)| p).find(|p| value(p, "number") == json!(mark))
}

/// The rows: a counter shutter (its words say rolling; lower than a passage door), a sliding door.
fn rows() -> Value {
    json!([
        {"kind": "door", "mark": "X1", "width_in": 126, "height_in": 58, "type": "ALUM. OVERHEAD ROLLING GATE", "hardware": "H-9"},
        {"kind": "door", "mark": "X2", "width_in": 96, "height_in": 84, "type": "SLIDING GLASS"},
    ])
}

#[test]
fn a_scheduled_door_with_no_opening_drawn_is_built_at_its_tag_with_the_schedules_size_and_its_sill() {
    // X1 tagged beside the partition (drawn through); X2 tagged by the counter edge, no wall near.
    let (mut s, built) = building(rows(), &[("X1", 13.5, 8.0), ("X2", 24.0, 5.0)]);
    let ds = doors(&mut s);
    assert_eq!(ds.len(), 2, "both scheduled doors are built: {built}");
    let x1 = door(&ds, "X1").unwrap_or_else(|| panic!("X1 is a door: {ds:?}"));
    assert_eq!(value(x1, "width"), json!(126.0), "{x1}");
    assert_eq!(value(x1, "height"), json!(58.0), "{x1}");
    assert_eq!(value(x1, "operation"), json!("rolling"), "{x1}");
    assert_eq!(x1["operation"]["source"]["kind"], json!("schedule"), "the schedule's words say it: {x1}");
    // Lower than a passage door: it closes onto a counter, its sill the rule's counter top.
    assert_eq!(value(x1, "sill_height"), json!(42.0), "{x1}");
    assert_eq!(x1["sill_height"]["source"]["kind"], json!("rule"), "{x1}");
    assert_eq!(value(x1, "over_counter"), json!(true), "{x1}");
    assert_eq!(value(x1, "hardware_set"), json!("H-9"), "its hardware set goes with it: {x1}");
    let x2 = door(&ds, "X2").unwrap_or_else(|| panic!("X2 is a door: {ds:?}"));
    assert_eq!((value(x2, "operation"), value(x2, "width")), (json!("sliding"), json!(96.0)), "{x2}");
    assert!(x2.get("sill_height").is_none_or(|v| v["value"].is_null()), "a passage door stands on the floor: {x2}");
    // The report: each built where it was placed.
    let row = &built["schedule_doors"][0];
    assert_eq!(row["built_count"], json!(2), "{built}");
    let placed: Vec<(Value, Value)> = row["built"].as_array().unwrap().iter().map(|b| (b["mark"].clone(), b["placed"].clone())).collect();
    assert!(placed.contains(&(json!("X1"), json!("schedule_tag_on_wall"))), "{placed:?}");
    assert!(placed.contains(&(json!("X2"), json!("schedule_tag_on_line"))), "{placed:?}");
    // Never passing for drawn: inferred, the rule its basis.
    let r = s.execute("bim.provenance", &json!({"part": "door", "list": 10})).expect("provenance");
    let listed = r["listed"].as_array().cloned().unwrap_or_default();
    assert_eq!(listed.len(), 2, "{r}");
    for l in &listed {
        assert_eq!(l["made"], json!("inferred"), "{l}");
        assert!(l["basis"].as_str().is_some_and(|b| b.contains("door schedule mark")), "{l}");
    }
    // The partition is cut for the shutter: the wall stays under the counter top and over its
    // head, the opening between (its 3D under 42" and over 100" only, where the door stands).
    let d = s.doc().unwrap();
    let door_z: Vec<(f64, f64)> = d
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("door"))
        .filter_map(|e| match &e.kind {
            EntityKind::Mesh(m) => {
                let zs = m.vertices.iter().map(|v| v.z);
                Some((zs.clone().fold(f64::INFINITY, f64::min), zs.fold(f64::NEG_INFINITY, f64::max)))
            }
            _ => None,
        })
        .collect();
    assert!(door_z.iter().any(|(lo, hi)| (*lo - 42.0).abs() < 0.01 && (*hi - 100.0).abs() < 0.01), "the shutter from 42\" to 100\": {door_z:?}");
}

#[test]
fn a_door_a_tag_marks_is_built_again_by_every_build() {
    let (mut s, _) = building(rows(), &[("X1", 13.5, 8.0), ("X2", 24.0, 5.0)]);
    let again = s.execute("model.build", &json!({"force": true})).expect("build again");
    assert_eq!(again["schedule_doors"][0]["built_count"], json!(2), "{again}");
    s.execute("bim.props.fill", &json!({})).expect("fill");
    let ds = doors(&mut s);
    assert_eq!(ds.len(), 2, "{ds:?}");
    assert!(door(&ds, "X1").is_some() && door(&ds, "X2").is_some(), "{ds:?}");
    // A second print of a tag a foot away is the same door.
    let (mut s, built) = building(rows(), &[("X1", 13.5, 8.0), ("X1", 13.5, 9.0)]);
    assert_eq!(doors(&mut s).len(), 1, "{built}");
}

#[test]
fn a_tag_by_a_door_drawn_in_its_gap_is_that_doors_and_builds_no_second_one() {
    let rows = json!([{"kind": "door", "mark": "X4", "width_in": 36, "height_in": 84, "type": "WD"}]);
    let (mut s, built) = building_with(rows.clone(), &[], true);
    assert_eq!(doors(&mut s).len(), 1, "the drawn door: {built}");
    let (mut s, built) = building_with(rows, &[("X4", 13.5, 13.5)], true);
    let ds = doors(&mut s);
    assert_eq!(ds.len(), 1, "the tag is the drawn door's: {built}");
    assert_eq!(value(&ds[0].1, "number"), json!("X4"), "{ds:?}");
    assert_eq!(built["schedule_doors"][0]["answered"], json!(1), "{built}");
    assert_eq!(built["schedule_doors"][0]["built_count"], json!(0), "{built}");
}

#[test]
fn a_person_places_a_door_by_mark_position_and_size_and_it_survives_rebuilds() {
    let (mut s, _) = building(json!([]), &[]);
    assert!(doors(&mut s).is_empty());
    // A door with no schedule row yet, by the north wall (drawn through).
    let r = s.execute("plan.door", &json!({"mark": "X3", "at": [96.0, 174.0], "width": 36, "height": 7})).expect("place by mark");
    assert_eq!((r["built"].clone(), r["level"].clone(), r["found"].clone()), (json!(true), json!("L1"), json!("schedule_tag_on_wall")), "{r}");
    assert_eq!((r["operation"].clone(), r["head_in"].clone()), (json!("swing"), json!(84.0)), "{r}");
    // In the spec: a row and a tag, so a build from scratch builds it again.
    let sp = s.execute("bspec.get", &json!({})).expect("spec")["spec"].clone();
    assert_eq!(sp["schedule"]["rows"][0]["mark"], json!("X3"), "{sp}");
    assert_eq!(sp["schedule"]["tags"][0]["status"], json!("user"), "{sp}");
    s.execute("model.build", &json!({"force": true})).expect("rebuild");
    s.execute("bim.props.fill", &json!({})).expect("fill");
    let ds = doors(&mut s);
    assert_eq!(ds.len(), 1, "{ds:?}");
    assert_eq!(value(&ds[0].1, "width"), json!(36.0), "{:?}", ds[0]);
    // Placed again a few feet along: moved, not a second door; a counter shutter's words and size.
    let r = s
        .execute("plan.door", &json!({"mark": "X3", "at": [132.0, 174.0], "width": 120, "height": 4, "type": "COUNTER SHUTTER"}))
        .expect("move by mark");
    assert_eq!((r["operation"].clone(), r["sill_in"].clone()), (json!("rolling"), json!(42.0)), "{r}");
    assert_eq!(doors(&mut s).len(), 1);
    // A window's mark, or no position, is refused.
    s.execute("bspec.set", &json!({"spec": {"schedule": {"rows": [{"kind": "door", "mark": "X3", "width_in": 120.0, "height_in": 48.0}, {"kind": "window", "mark": "W1"}]}}})).expect("rows");
    assert!(s.execute("plan.door", &json!({"mark": "W1", "at": [96.0, 174.0]})).is_err());
    assert!(s.execute("plan.door", &json!({"mark": "X4"})).is_err());
}
