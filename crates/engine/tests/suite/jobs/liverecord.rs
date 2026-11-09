//! The live model's rebuild after a plan edit builds what a full build of the edited plan does,
//! except for what a person deleted or changed by hand (soak S6-001): after an undone rebuild,
//! after a command whose later steps changed the model it built, and when the building spec grew
//! after the build; and `bim.count`'s stored index stays equal to a recount.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::{made_as, model_behind};
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// Feet → a line in inches on `layer`, straight into the drawing.
fn line_on(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = layer.into();
    d.model.push(e);
}

fn rules() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "kind": "wall"}]
    })
}

/// Two 20' × 10' rooms side by side (a shared wall at 20'), 6" walls on A-WALL.
fn plan() -> Drawing {
    let mut d = Drawing::new_imperial();
    for (x0, x1) in [(0.0, 20.0), (20.5, 40.5)] {
        let (i0, i1) = (x0 + 0.5, x1 - 0.5);
        for (a, b) in [((x0, 0.), (x1, 0.)), ((x1, 0.), (x1, 10.)), ((x1, 10.), (x0, 10.)), ((x0, 10.), (x0, 0.))] {
            line_on(&mut d, "A-WALL", a, b);
        }
        for (a, b) in [((i0, 0.5), (i1, 0.5)), ((i1, 0.5), (i1, 9.5)), ((i1, 9.5), (i0, 9.5)), ((i0, 9.5), (i0, 0.5))] {
            line_on(&mut d, "A-WALL", a, b);
        }
    }
    d
}

/// What the build made, as a sorted list of each object's geometry and layer (handles aside).
fn model(s: &Session) -> Vec<String> {
    let mut v: Vec<String> =
        s.doc().unwrap().model.iter().filter(|e| made_as(e).is_some()).map(|e| format!("{:?}|{}", e.kind, e.common.layer)).collect();
    v.sort();
    v
}

/// The drawing's plan as it is now, without what any build made.
fn plan_now(s: &Session) -> Drawing {
    let mut d = s.doc().unwrap().clone();
    let made: Vec<Handle> = d.model.iter().filter(|e| made_as(e).is_some()).map(|e| e.handle).collect();
    for h in made {
        d.remove_entity(h);
    }
    d
}

/// The model a full WALLGEN of `s`'s plan as it is now makes (in a session of its own).
fn full_wallgen(s: &Session) -> Vec<String> {
    let mut t = Session::empty();
    t.open_drawing(plan_now(s), "Plan", None);
    t.execute("wallgen", &json!({"rules": rules(), "auto": true})).unwrap();
    model(&t)
}

/// The model a full `model.build` of `s`'s plan and building spec as they are now makes.
fn full_model_build(s: &Session) -> Vec<String> {
    let mut t = Session::empty();
    t.open_drawing(plan_now(s), "Plan", None);
    t.execute("model.build", &json!({})).unwrap();
    model(&t)
}

/// The plan lines (not built objects) whose both ends satisfy `keep` (in inches).
fn plan_lines(s: &Session, keep: impl Fn(f64, f64) -> bool) -> Vec<String> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).is_none() && matches!(&e.kind, EntityKind::Line(l) if keep(l.a.x, l.a.y) && keep(l.b.x, l.b.y)))
        .map(|e| e.handle.hex())
        .collect()
}

fn shift(s: &mut Session, handles: &[String], by_ft: (f64, f64)) {
    s.execute("move", &json!({"handles": handles, "from": [0, 0], "to": [by_ft.0 * 12.0, by_ft.1 * 12.0]})).unwrap();
}

/// The east room's east wall lines (x > 480").
fn east(s: &Session) -> Vec<String> {
    plan_lines(s, |x, _| x > 480.0)
}

/// The west room's west wall lines (x < 12").
fn west(s: &Session) -> Vec<String> {
    plan_lines(s, |x, _| x < 12.0)
}

/// The generated objects of `part`.
fn made_of(s: &Session, part: &str) -> Vec<Handle> {
    s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some(part)).map(|e| e.handle).collect()
}

fn update(s: &mut Session) -> Value {
    let r = s.execute("model.update", &json!({})).unwrap();
    assert_eq!(r["rebuilt"], json!(true), "{r}");
    assert!(!model_behind(s));
    r
}

/// `bim.count {verify}`: (stored, recounted, matches).
fn verify(s: &mut Session) -> (Value, Value, Value) {
    let r = s.execute("bim.count", &json!({"verify": true})).unwrap();
    (r["count"].clone(), r["verify"]["count"].clone(), r["verify"]["matches"].clone())
}

/// A JSON value as a Rhai literal (a script's `command(id, params)`).
fn rhai(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            format!("#{{{}}}", m.iter().map(|(k, v)| format!("{}: {}", Value::String(k.clone()), rhai(v))).collect::<Vec<_>>().join(", "))
        }
        Value::Array(a) => format!("[{}]", a.iter().map(rhai).collect::<Vec<_>>().join(", ")),
        Value::Null => "()".into(),
        other => other.to_string(),
    }
}

#[test]
fn a_rebuild_after_an_undone_rebuild_builds_what_a_full_build_does() {
    let mut s = Session::empty();
    s.open_drawing(plan(), "Plan", None);
    s.execute("wallgen", &json!({"rules": rules(), "auto": true})).unwrap();
    let original = model(&s);
    // A rebuild, undone: the drawing is back in the first build's state.
    let hs = east(&s);
    shift(&mut s, &hs, (2.0, 0.0));
    update(&mut s);
    assert_ne!(model(&s), original);
    s.execute("undo", &json!({})).unwrap();
    assert!(!model_behind(&s), "the state the first build left");
    assert_eq!(model(&s), original);
    // Another edit: its rebuild compares with the first build (the drawing's), not the undone one.
    let walls = made_of(&s, "wall").len();
    let hs = west(&s);
    shift(&mut s, &hs, (-2.0, 0.0));
    let r = update(&mut s);
    assert_eq!(made_of(&s, "wall").len(), walls, "no wall taken for deleted by hand: {r}");
    assert_eq!(model(&s), full_wallgen(&s), "the same model as a full build of the plan as it is now");
    // Redo past it is gone (a new edit), and undo of the edit is the first build again.
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(model(&s), original);
}

#[test]
fn redo_of_an_undone_rebuild_and_another_edit_keep_the_rebuilds_record() {
    let mut s = Session::empty();
    s.open_drawing(plan(), "Plan", None);
    s.execute("wallgen", &json!({"rules": rules(), "auto": true})).unwrap();
    let hs = east(&s);
    shift(&mut s, &hs, (2.0, 0.0));
    update(&mut s);
    let rebuilt = model(&s);
    s.execute("undo", &json!({})).unwrap();
    s.execute("redo", &json!({})).unwrap();
    assert!(!model_behind(&s));
    assert_eq!(model(&s), rebuilt, "redo brings back the rebuilt model");
    let hs = west(&s);
    shift(&mut s, &hs, (-2.0, 0.0));
    update(&mut s);
    assert_eq!(model(&s), full_wallgen(&s));
}

#[test]
fn what_the_building_command_did_after_its_build_is_no_hand_edit() {
    // One command builds and then changes what it built (as a job's later steps replace the
    // conduits and equipment its build placed): here a script that builds and then erases one
    // of the walls it built. The handle that wall gets, from the same build of the same plan.
    let wall = {
        let mut t = Session::empty();
        t.open_drawing(plan(), "Plan", None);
        t.execute("wallgen", &json!({"rules": rules(), "auto": true})).unwrap();
        *made_of(&t, "wall").first().unwrap()
    };
    let mut s = Session::empty();
    s.open_drawing(plan(), "Plan", None);
    let code =
        format!("command(\"wallgen\", {}); command(\"erase\", #{{handles: [\"{}\"]}});", rhai(&json!({"rules": rules(), "auto": true})), wall.hex());
    let r = s.execute("rhai", &json!({"code": code})).unwrap();
    assert!(r.get("error").is_none_or(Value::is_null), "{r}");
    assert!(s.doc().unwrap().entity(wall).is_none(), "the script's step took the wall away");
    // An edit elsewhere: the rebuild is a full build's model (the wall back, its plan unchanged);
    // nothing was deleted by hand.
    let hs = east(&s);
    shift(&mut s, &hs, (2.0, 0.0));
    let r = update(&mut s);
    assert_eq!(model(&s), full_wallgen(&s), "the wall the script's step took away is no hand deletion: {r}");
    let last = s.execute("model.revise", &json!({"last": true})).unwrap();
    assert_eq!(last["deleted"], json!(0), "{last}");
}

/// A spec of the plan's walls; its tags: device tags (a receptacle each) at `at_ft`.
fn spec(tags: &[(f64, f64)]) -> Value {
    let tags: Vec<Value> = tags
        .iter()
        .enumerate()
        .map(|(i, (x, y))| {
            json!({"id": format!("device:L1:R{i}"), "kind": "receptacle", "level": "L1", "at_ft": [x, y],
                "fields": {"type": {"value": "R", "source": "plan"}, "circuit": {"value": "", "source": "plan"}, "symbol": {"value": "", "source": "plan"}}})
        })
        .collect();
    json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"}],
        "wall_types": [{"name": "Exterior", "thickness_in": 6, "from": "pairs"}],
        "tags": tags
    })
}

#[test]
fn a_rebuild_builds_from_the_spec_as_it_is_now() {
    let mut s = Session::empty();
    s.open_drawing(plan(), "Plan", None);
    s.execute("bspec.set", &json!({"spec": spec(&[(5.0, 1.0), (15.0, 1.0), (25.0, 9.0)]), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    assert_eq!(made_of(&s, "device").len(), 3);
    // The spec grows after the build (a job's later step adds what it placed), with no build of
    // its own; then a plan edit.
    s.execute("bspec.set", &json!({"spec": spec(&[(5.0, 1.0), (15.0, 1.0), (25.0, 9.0), (35.0, 9.0)]), "replace": true})).unwrap();
    let hs = east(&s);
    shift(&mut s, &hs, (2.0, 0.0));
    update(&mut s);
    assert_eq!(made_of(&s, "device").len(), 4, "the device the spec holds now is built");
    assert_eq!(model(&s), full_model_build(&s), "the same model as model.build of the plan and spec now");
}

#[test]
fn a_rebuild_says_what_it_took_away_and_the_count_index_follows() {
    let mut s = Session::empty();
    s.open_drawing(plan(), "Plan", None);
    s.execute("bspec.set", &json!({"spec": spec(&[(5.0, 1.0), (15.0, 1.0), (25.0, 9.0)]), "replace": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s.execute("bim.count", &json!({"refresh": true})).unwrap();
    let devices = made_of(&s, "device");
    assert_eq!(devices.len(), 3);
    // A device deleted by hand, then a plan edit away from it.
    s.execute("erase", &json!({"handles": [devices[0].hex()]})).unwrap();
    let (stored, recount, ok) = verify(&mut s);
    assert_eq!(ok, json!(true), "{stored} {recount}");
    let hs = east(&s);
    shift(&mut s, &hs, (2.0, 0.0));
    // Rebuilt by the next command that reads the model (outside any command of its own).
    let (stored, recount, ok) = verify(&mut s);
    assert!(!model_behind(&s));
    assert_eq!(made_of(&s, "device").len(), 2, "the device deleted by hand stays deleted");
    assert_eq!(ok, json!(true), "the stored index is the recount after the rebuild: {stored} vs {recount}");
    let last = s.execute("model.revise", &json!({"last": true})).unwrap();
    assert!(last["deleted"].as_u64().is_some_and(|n| n >= 1), "the rebuild says it took the deleted device away: {last}");
    // Another rebuild (through model.update): still deleted, and the index still the recount.
    let hs = east(&s);
    shift(&mut s, &hs, (1.0, 0.0));
    let r = update(&mut s);
    assert_eq!(made_of(&s, "device").len(), 2, "deleted by hand stays deleted past the next rebuild: {r}");
    let (stored, recount, ok) = verify(&mut s);
    assert_eq!(ok, json!(true), "{r} {stored} {recount}");
}

#[test]
fn a_hand_edit_stays_past_more_than_one_rebuild() {
    let mut s = Session::empty();
    s.open_drawing(plan(), "Plan", None);
    s.execute("wallgen", &json!({"rules": rules(), "auto": true})).unwrap();
    // Recolour one generated wall of the west room by hand; then two plan edits on the east side.
    let w = s
        .doc()
        .unwrap()
        .model
        .iter()
        .find(|e| made_as(e).as_deref() == Some("wall") && matches!(&e.kind, EntityKind::Mesh(m) if m.vertices.iter().all(|v| v.x <= 240.0)))
        .map(|e| e.handle)
        .expect("a west room wall");
    s.execute("properties.set", &json!({"handles": [w.hex()], "color": 1})).unwrap();
    let colour = s.doc().unwrap().entity(w).unwrap().common.color;
    for dx in [2.0, 1.0] {
        let hs = east(&s);
        shift(&mut s, &hs, (dx, 0.0));
        update(&mut s);
        let now = s.doc().unwrap().model.iter().filter(|e| made_as(e).is_some() && e.common.color == colour).count();
        assert_eq!(now, 1, "the recoloured wall is kept after each rebuild");
    }
}
