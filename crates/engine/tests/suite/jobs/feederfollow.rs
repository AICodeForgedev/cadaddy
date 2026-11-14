//! Q6523: feeders follow their gear. A panel moved by setting its place in the equipment list
//! again (across its room, or to another floor) gets its feeder routed again from its source to
//! where it is now: in the spec (bspec.set), in the model (model.build) and by conduit.reroute;
//! a route that ends where the gear was is never kept. Made-up building only.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

/// A feeder run as the router stores it: `pts` (inches) at `z` feet over its level's floor, or a
/// riser at one point from `z` up to `top`.
fn run(level: &str, pts: &[[f64; 2]], z: f64, top: Option<f64>) -> Value {
    let mut r = json!({"system": "E-FEEDER", "level": level, "points": pts, "size_in": [2, 0], "bottoms_ft": vec![z; pts.len().saturating_sub(1).max(1)],
        "size_source": "feeder schedule", "height_source": "proposed", "source_sheet": "feeder F1 SB1→P"});
    if let Some(t) = top {
        r["top_ft"] = json!(t);
    }
    r
}

fn sb1() -> Value {
    json!({"tag": "SB1", "kind": "switchboard", "level": "L1", "min": [0, 0], "max": [48, 24], "height_ft": 7, "bottom_ft": 0})
}

fn panel(level: &str, at: [f64; 2]) -> Value {
    json!({"tag": "P", "kind": "panel", "level": level, "min": [at[0] - 12.0, at[1] - 12.0], "max": [at[0] + 12.0, at[1] + 12.0], "height_ft": 3, "bottom_ft": 4})
}

/// Three made-up storeys (12' floor to floor, 9' ceilings): switchboard SB1 on the first, panel P
/// on the second at (252, 12), feeder F1 between them routed (two conduits, each up out of SB1,
/// across, up a floor beside P and over into its top).
fn routed() -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Three Storeys", None);
    let mut runs = Vec::new();
    for y in [8.0, 16.0] {
        runs.push(run("L1", &[[24.0, y]], 7.0, Some(10.0)));
        runs.push(run("L1", &[[24.0, y], [270.0, y]], 10.0, None));
        runs.push(run("L1", &[[270.0, y]], 10.0, Some(22.0)));
        runs.push(run("L2", &[[270.0, y], [252.0, y]], 10.0, None));
        runs.push(run("L2", &[[252.0, y]], 7.0, Some(10.0)));
    }
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]},
            {"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]},
            {"name": "L3", "elevation_ft": 24, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}
        ],
        "mep": {
            "systems": [{"name": "E-FEEDER", "layer": "E-FEEDER", "kind": "conduit", "size_in": [2, 0], "bottom_ft": 10}],
            "runs": runs,
            "equipment": [sb1(), panel("L2", [252.0, 12.0])],
            "feeders": [{"tag": "F1", "from": "SB1", "to": "P", "sets": 2, "phases": 3, "size": "3/0", "material": "cu", "neutrals": 1, "grounds": 1,
                "ground_size": "#6", "conduits": 2, "conduit_in": 2, "entry": "top", "source": "made-up feeder schedule"}]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

/// The spec's feeder runs with an end in P's outline on P's level (grown `reach` inches), and
/// all its feeder runs.
fn spec_ends_at_p(s: &mut Session, level: &str, at: [f64; 2], reach: f64) -> (usize, usize) {
    let sp = s.execute("bspec.get", &json!({})).expect("get")["spec"].clone();
    let runs: Vec<Value> = sp["mep"]["runs"].as_array().cloned().unwrap_or_default().into_iter().filter(|r| r["system"] == "E-FEEDER").collect();
    let inside = |p: &Value| {
        let (x, y) = (p[0].as_f64().unwrap_or(f64::NAN), p[1].as_f64().unwrap_or(f64::NAN));
        (x - at[0]).abs() <= 12.0 + reach && (y - at[1]).abs() <= 12.0 + reach
    };
    let at_p = runs
        .iter()
        .filter(|r| r["level"] == level)
        .filter(|r| {
            let pts = r["points"].as_array().cloned().unwrap_or_default();
            pts.first().is_some_and(inside) || pts.last().is_some_and(inside)
        })
        .count();
    (at_p, runs.len())
}

/// The model's feeder conduit pieces in the storey of `level` (its floor up 12') that reach P's
/// outline at `at` (grown 18").
fn model_at_p(s: &Session, level: &str, at: [f64; 2]) -> usize {
    let floor = match level {
        "L1" => 0.0,
        "L2" => 144.0,
        _ => 288.0,
    };
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .filter(|e| e.common.xdata.iter().any(|t| t.value == "system:E-FEEDER"))
        .filter(|e| {
            let cadaddy_engine::doc::EntityKind::Mesh(m) = &e.kind else { return false };
            m.vertices.iter().any(|v| (v.x - at[0]).abs() <= 30.0 && (v.y - at[1]).abs() <= 30.0 && v.z > floor + 1.0 && v.z < floor + 143.0)
        })
        .count()
}

fn move_p(s: &mut Session, level: &str, at: [f64; 2]) -> Value {
    s.execute("bspec.set", &json!({"spec": {"mep": {"equipment": [sb1(), panel(level, at)]}}})).expect("move")
}

#[test]
fn a_panel_moved_across_its_room_gets_its_feeder_routed_to_its_new_place() {
    let mut s = routed();
    let old = [252.0, 12.0];
    assert_eq!(spec_ends_at_p(&mut s, "L2", old, 0.0).0, 4, "routed into P where it is");
    assert!(model_at_p(&s, "L2", old) > 0);
    // P to the far side of its room.
    let new = [612.0, 300.0];
    let set = move_p(&mut s, "L2", new);
    let (at_new, all) = spec_ends_at_p(&mut s, "L2", new, 18.0);
    eprintln!("after bspec.set: {at_new} of {all} feeder runs end at P's new place; {}", set["feeders"]);
    assert!(at_new >= 2, "each of the two conduits ends in P where it is now: {}", set["feeders"]);
    assert_eq!(spec_ends_at_p(&mut s, "L2", old, 18.0).0, 0, "none is kept ending where P was");
    assert_eq!(set["feeders"]["rerouted"], 1, "{set}");
    assert_eq!(set["feeders"]["off_gear_after"], 0, "{set}");
    s.execute("model.build", &json!({})).expect("rebuild");
    let built = model_at_p(&s, "L2", new);
    eprintln!("after model.build: {built} feeder conduit objects reach P's new place, {} its old", model_at_p(&s, "L2", old));
    assert!(built > 0);
    assert_eq!(model_at_p(&s, "L2", old), 0);
    let r = s.execute("conduit.reroute", &json!({})).expect("reroute");
    assert_eq!(r["feeders"]["off_gear_before"], 0, "already followed: {r}");
    assert!(model_at_p(&s, "L2", new) > 0, "{r}");
    assert_eq!(model_at_p(&s, "L2", old), 0);
}

#[test]
fn a_panel_moved_to_another_floor_gets_its_feeder_risen_to_that_floor() {
    let mut s = routed();
    let at = [252.0, 12.0];
    move_p(&mut s, "L3", at);
    let (on_l3, all) = spec_ends_at_p(&mut s, "L3", at, 18.0);
    eprintln!("after bspec.set: {on_l3} of {all} feeder runs end at P on L3");
    assert!(on_l3 >= 2, "both conduits end in P on its new floor");
    // On the old floor only the riser on up passes, floor to ceiling: no run stops in P's old place.
    let sp = s.execute("bspec.get", &json!({})).expect("get")["spec"].clone();
    let on_l2: Vec<Value> =
        sp["mep"]["runs"].as_array().cloned().unwrap_or_default().into_iter().filter(|r| r["system"] == "E-FEEDER" && r["level"] == "L2").collect();
    assert!(
        !on_l2.is_empty()
            && on_l2.iter().all(|r| r["points"].as_array().map_or(0, Vec::len) == 1 && r["bottoms_ft"][0] == 0.0 && r["top_ft"] == 12.0),
        "{on_l2:?}"
    );
    s.execute("model.build", &json!({})).expect("rebuild");
    let built = model_at_p(&s, "L3", at);
    eprintln!("after model.build: {built} feeder conduit objects at P on L3");
    assert!(built > 0, "the feeder's conduit is built on P's new floor");
}

#[test]
fn reroute_follows_a_panel_moved_by_another_command_and_routes_feeders_on_demand() {
    // Gear moved where bspec.set doesn't see it (a command writing the spec straight): conduit.reroute follows it.
    let mut s = routed();
    let new = [612.0, 300.0];
    let d = s.doc_mut().expect("doc");
    let mut sp: Value = serde_json::from_str(d.building_spec.as_deref().unwrap_or("{}")).expect("spec json");
    sp["mep"]["equipment"] = json!([sb1(), panel("L2", new)]);
    d.building_spec = Some(sp.to_string()).into();
    let r = s.execute("conduit.reroute", &json!({})).expect("reroute");
    eprintln!("conduit.reroute: {}", r["feeders"]);
    assert_eq!(r["feeders"]["rerouted"], 1, "{r}");
    assert_eq!((r["feeders"]["off_gear_before"].as_u64(), r["feeders"]["off_gear_after"].as_u64()), (Some(1), Some(0)), "{r}");
    assert!(spec_ends_at_p(&mut s, "L2", new, 18.0).0 >= 2);
    assert!(model_at_p(&s, "L2", new) > 0, "its pieces cut again where P is");
    // No route at all: routed on demand.
    let mut s = routed();
    s.execute("bspec.set", &json!({"spec": {"mep": {"runs": []}}})).expect("no runs");
    assert_eq!(spec_ends_at_p(&mut s, "L2", [252.0, 12.0], 18.0).1, 0);
    assert!(s.execute("conduit.reroute", &json!({"feeders": "bogus"})).is_err());
    let r = s.execute("conduit.reroute", &json!({"feeders": "all"})).expect("route all");
    assert_eq!(r["feeders"]["unrouted_before"], 1, "{r}");
    assert!(spec_ends_at_p(&mut s, "L2", [252.0, 12.0], 18.0).0 >= 2, "{r}");
}
