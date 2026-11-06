//! An associative model the plan's edits leave behind is built again once, not after every edit
//! (soak S5-004): edits to the plan mark it behind; it is built before the next command that reads
//! it, when editing stops for a moment, or on `model.update`, and comes out as a full build of the
//! plan as it is then would.

use std::time::Duration;

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

/// A 20' × 10' room of 6" walls on A-WALL.
fn plan() -> Drawing {
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))] {
        line_on(&mut d, "A-WALL", a, b);
    }
    for (a, b) in [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))] {
        line_on(&mut d, "A-WALL", a, b);
    }
    d
}

/// The plan, built by an associative WALLGEN.
fn built() -> Session {
    let mut s = Session::empty();
    s.open_drawing(plan(), "Plan", None);
    s.execute("wallgen", &json!({"rules": rules(), "auto": true})).unwrap();
    assert!(!model_behind(&s));
    s
}

/// What the build made, as a sorted list of each object's geometry and layer (handles aside).
fn model(s: &Session) -> Vec<String> {
    let mut v: Vec<String> =
        s.doc().unwrap().model.iter().filter(|e| made_as(e).is_some()).map(|e| format!("{:?}|{}", e.kind, e.common.layer)).collect();
    v.sort();
    v
}

/// The model a full build of `s`'s plan as it is now makes (in a session of its own).
fn full_build(s: &Session) -> Vec<String> {
    let mut d = s.doc().unwrap().clone();
    let made: Vec<Handle> = d.model.iter().filter(|e| made_as(e).is_some()).map(|e| e.handle).collect();
    for h in made {
        d.remove_entity(h);
    }
    let mut t = Session::empty();
    t.open_drawing(d, "Plan", None);
    t.execute("wallgen", &json!({"rules": rules(), "auto": true})).unwrap();
    model(&t)
}

fn count(s: &Session) -> usize {
    s.doc().unwrap().model.len()
}

/// Draw a line (feet) on the current layer with the LINE command.
fn draw(s: &mut Session, a: (f64, f64), b: (f64, f64)) {
    s.execute("line", &json!({"points": [[a.0 * 12.0, a.1 * 12.0], [b.0 * 12.0, b.1 * 12.0]]})).unwrap();
}

fn current(s: &mut Session, layer: &str) {
    if s.execute("layer.current", &json!({"name": layer})).is_err() {
        s.execute("layer.new", &json!({"name": layer, "current": true})).unwrap();
    }
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

#[test]
fn plan_edits_leave_the_model_behind_and_one_update_builds_what_a_full_build_does() {
    let mut s = built();
    let before = model(&s);
    // A wall across the room (two face lines 6" apart), the east wall moved 2', a line on 0.
    current(&mut s, "A-WALL");
    let n = count(&s);
    draw(&mut s, (10.0, 0.5), (10.0, 9.5));
    assert!(model_behind(&s), "a plan line leaves the model behind");
    assert_eq!(count(&s), n + 1, "the line, and nothing rebuilt with it");
    draw(&mut s, (10.5, 0.5), (10.5, 9.5));
    s.execute("move", &json!({"handles": east(&s), "from": [0, 0], "to": [24, 0]})).unwrap();
    current(&mut s, "0");
    draw(&mut s, (30.0, 30.0), (31.0, 30.0));
    assert_eq!(model(&s), before, "no build ran during the edits");
    assert!(model_behind(&s));
    // One update: the model a full build of the edited plan makes.
    let r = s.execute("model.update", &json!({})).unwrap();
    assert_eq!(r["behind"], json!(true), "{r}");
    assert_eq!(r["rebuilt"], json!(true), "{r}");
    assert!(!model_behind(&s));
    let after = model(&s);
    assert_ne!(after, before, "the new wall and the moved one are built");
    assert_eq!(after, full_build(&s), "the same model as a full build of the plan as it is now");
    // Nothing behind: an update does nothing.
    let r = s.execute("model.update", &json!({})).unwrap();
    assert_eq!(r["rebuilt"], json!(false), "{r}");
}

#[test]
fn edits_off_the_plan_layers_leave_the_model_current() {
    let mut s = built();
    current(&mut s, "0");
    draw(&mut s, (30.0, 30.0), (31.0, 30.0));
    s.execute("circle", &json!({"center": [400, 400], "radius": 6})).unwrap();
    assert!(!model_behind(&s), "nothing on a plan layer changed");
}

#[test]
fn a_command_that_reads_the_model_builds_it_first() {
    let mut s = built();
    current(&mut s, "A-WALL");
    draw(&mut s, (10.0, 0.5), (10.0, 9.5));
    draw(&mut s, (10.5, 0.5), (10.5, 9.5));
    assert!(model_behind(&s));
    // Listing objects reads the model: it is built first, and the list holds the new wall.
    let r = s.execute("entities", &json!({"limit": 1})).unwrap();
    assert!(r["count"].as_u64().is_some(), "{r}");
    assert!(!model_behind(&s), "built before the listing");
    assert_eq!(model(&s), full_build(&s));
}

#[test]
fn the_model_is_built_when_editing_stops() {
    let mut s = built();
    let before = model(&s);
    current(&mut s, "A-WALL");
    draw(&mut s, (10.0, 0.5), (10.0, 9.5));
    draw(&mut s, (10.5, 0.5), (10.5, 9.5));
    // Editing stopped only just now: it waits, and says how long.
    let wait = s.update_model_when_idle(Duration::from_secs(3600));
    assert!(wait.is_some_and(|w| w > Duration::from_secs(3000)), "{wait:?}");
    assert!(model_behind(&s));
    assert_eq!(model(&s), before);
    // The pause long enough: built, and nothing waits any more.
    assert_eq!(s.update_model_when_idle(Duration::ZERO), None);
    assert!(!model_behind(&s));
    assert_eq!(model(&s), full_build(&s));
    assert_eq!(s.update_model_when_idle(Duration::ZERO), None);
}

#[test]
fn undo_back_into_a_state_behind_is_known_as_one_and_the_built_states_stay_built() {
    let mut s = built();
    let original = model(&s);
    current(&mut s, "A-WALL");
    draw(&mut s, (10.0, 0.5), (10.0, 9.5));
    draw(&mut s, (10.5, 0.5), (10.5, 9.5));
    s.execute("model.update", &json!({})).unwrap();
    let both = model(&s);
    assert_eq!(both, full_build(&s));
    // Undo the second line: the state after the first, which the model was behind in.
    s.execute("undo", &json!({})).unwrap();
    assert!(model_behind(&s), "undo returned to a state the model was behind in");
    s.execute("model.update", &json!({})).unwrap();
    assert!(!model_behind(&s));
    assert_eq!(model(&s), full_build(&s), "built for the one line");
    // Redo brings back the built state with both lines.
    s.execute("redo", &json!({})).unwrap();
    assert!(!model_behind(&s));
    assert_eq!(model(&s), both);
    // Undo past both lines (and the layer change): the original build, current as it was.
    for _ in 0..3 {
        s.execute("undo", &json!({})).unwrap();
    }
    assert!(!model_behind(&s));
    assert_eq!(model(&s), original);
}

#[test]
fn a_batch_of_edits_builds_once() {
    let mut s = built();
    current(&mut s, "A-WALL");
    let cmds: Vec<Value> = (0..10).map(|i| json!({"command": "line", "params": {"points": [[600 + 12 * i, 600], [600 + 12 * i, 660]]}})).collect();
    let n = count(&s);
    s.execute_many(&cmds, true).unwrap();
    assert_eq!(count(&s), n + 10, "ten lines, no rebuild in between");
    assert!(model_behind(&s));
    assert!(s.execute("model.update", &json!({})).is_ok());
    assert!(!model_behind(&s));
    assert_eq!(model(&s), full_build(&s));
}

#[test]
fn a_plain_wallgen_off_auto_never_falls_behind() {
    let mut s = Session::empty();
    s.open_drawing(plan(), "Plan", None);
    s.execute("wallgen", &json!({"rules": rules(), "auto": false})).unwrap();
    current(&mut s, "A-WALL");
    draw(&mut s, (10.0, 0.5), (10.0, 9.5));
    assert!(!model_behind(&s), "not associative: edits don't rebuild it");
    let r = s.execute("model.update", &json!({})).unwrap();
    assert_eq!(r, json!({"behind": false, "rebuilt": false}));
}

#[test]
fn an_edit_off_the_plan_in_a_state_behind_stays_behind_after_undo_and_a_newer_build() {
    let mut s = built();
    current(&mut s, "A-WALL");
    draw(&mut s, (10.0, 0.5), (10.0, 9.5));
    draw(&mut s, (10.5, 0.5), (10.5, 9.5));
    s.execute("model.update", &json!({})).unwrap();
    // Back in the state after the first line (behind an older build than the last one).
    s.execute("undo", &json!({})).unwrap();
    assert!(model_behind(&s));
    // Nothing on a plan layer changes, but the drawing it changes is behind: so is the new one.
    current(&mut s, "0");
    draw(&mut s, (30.0, 30.0), (31.0, 30.0));
    assert!(model_behind(&s), "still behind the plan");
    s.execute("model.update", &json!({})).unwrap();
    assert!(!model_behind(&s));
    assert_eq!(model(&s), full_build(&s));
}
