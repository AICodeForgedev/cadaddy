//! Q6381: an approved `bim.clash.fix` is a model edit that persists: the spec keeps it, so
//! `model.clashes`, `bim.clash` and every rebuild agree with it.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn system(name: &str, kind: &str, size: [f64; 2]) -> Value {
    json!({"name": name, "layer": name, "kind": kind, "size_in": size, "bottom_ft": 10})
}

fn run(system: &str, a: [f64; 2], b: [f64; 2], size: [f64; 2], bottom: f64) -> Value {
    json!({"system": system, "level": "Level 1", "points": [a, b], "size_in": size, "bottoms_ft": [bottom]})
}

/// One level, a 40' x 30' room (inches), the given systems and runs, built to 3D.
fn building(systems: Vec<Value>, runs: Vec<Value>) -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[0, 0], [480, 0], [480, 360], [0, 360]], "closed": true})).unwrap();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-1000, -1000, 1500, 1500], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}],
            "mep": {"systems": systems, "runs": runs}
        }}),
    )
    .unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

fn crossings(s: &mut Session) -> Vec<Value> {
    let r = s.execute("model.clashes", &json!({})).unwrap();
    r["clashes"].as_array().unwrap().iter().filter(|c| c["kind"] == "crossing").cloned().collect()
}

fn crossings_of(s: &mut Session, system: &str) -> usize {
    crossings(s).iter().filter(|c| c["systems"].as_array().unwrap().iter().any(|x| x == system)).count()
}

fn clashes(s: &mut Session, a: &str, b: &str) -> Value {
    s.execute("bim.clash", &json!({"a": a, "b": b})).unwrap()
}

fn listed(s: &mut Session, system: &str) -> Value {
    s.execute("mep.list", &json!({"system": system})).unwrap()["items"][0].clone()
}

/// Every check agrees the clash is gone: bim.clash, model.clashes, a build that keeps the model
/// and one that lays it out again.
fn stays_fixed(s: &mut Session, a: &str, b: &str, system: &str) {
    assert_eq!(clashes(s, a, b)["total"], 0, "bim.clash after the fix");
    assert_eq!(crossings_of(s, system), 0, "model.clashes after the fix: {:?}", crossings(s));
    let kept = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(clashes(s, a, b)["total"], 0, "after model.build: {}", kept["rebuild"]);
    s.execute("model.build", &json!({"force": true})).unwrap();
    assert_eq!(clashes(s, a, b)["total"], 0, "after a forced rebuild");
    assert_eq!(crossings_of(s, system), 0, "model.clashes after a forced rebuild");
}

/// The finding: a 12x8 duct (bottom 9.6') crossing a 3/4" conduit (bottom 10'). The fix raises
/// the conduit (its wiring chain's pieces); the run is raised in the spec, kept as an edit.
#[test]
fn an_approved_raise_of_a_conduit_is_kept_in_the_spec_and_survives_every_build() {
    let mut s = building(
        vec![system("M-SUPPLY", "duct", [12.0, 8.0]), system("E-COND", "conduit", [0.75, 0.0])],
        vec![run("M-SUPPLY", [24.0, 60.0], [264.0, 60.0], [12.0, 8.0], 9.6), run("E-COND", [200.0, 24.0], [200.0, 150.0], [0.75, 0.0], 10.0)],
    );
    let c = clashes(&mut s, "duct", "conduit");
    assert_eq!(c["total"], 1, "{c}");
    assert_eq!(crossings_of(&mut s, "E-COND"), 1);
    let id = c["clashes"][0]["id"].as_str().unwrap().to_string();
    let f = s.execute("bim.clash.fix", &json!({"id": id, "approve": true})).unwrap();
    assert_eq!((f["how"].as_str(), f["check"]["clear"].as_bool()), (Some("raise"), Some(true)), "{f}");
    let up_in = f["move"][2].as_f64().unwrap();
    assert!(up_in > 0.0, "{f}");
    assert_eq!(f["kept"]["kept_by"], "building spec", "{f}");
    let edits = f["kept"]["edits"].as_array().unwrap().clone();
    assert_eq!(edits.len(), 1, "one run raised: {f}");
    assert!(f["pieces_moved"].as_u64().unwrap() > 1, "the chain's pieces move together: {f}");
    // The run as the builds lay it: raised, the documents' height beside it, with why.
    let r = listed(&mut s, "E-COND");
    let bottom = r["bottoms_ft"][0].as_f64().unwrap();
    assert!((bottom - (10.0 + up_in / 12.0)).abs() < 1e-6, "{r}");
    assert_eq!(r["height_source"], "override");
    let corr = &r["corrections"][0];
    assert_eq!((corr["key"].as_str(), corr["design"].as_str()), (Some("bottom_ft"), Some("10")), "{r}");
    assert!(corr["reason"].as_str().unwrap().contains(&id) && corr["reason"].as_str().unwrap().contains("M-SUPPLY"), "{corr}");
    let me = s.execute("mep.edits", &json!({})).unwrap();
    assert_eq!(
        (me["total"].as_u64(), me["edits"][0]["id"].as_str(), me["edits"][0]["status"].as_str()),
        (Some(1), edits[0].as_str(), Some("applied")),
        "{me}"
    );
    // The duct stays where the documents put it.
    assert_eq!(listed(&mut s, "M-SUPPLY")["bottoms_ft"], json!([9.6]));
    stays_fixed(&mut s, "duct", "conduit", "E-COND");
    // Taken back, the documents' height and the clash return.
    s.execute("mep.edits", &json!({"revert": edits[0]})).unwrap();
    assert_eq!(listed(&mut s, "E-COND")["bottoms_ft"], json!([10.0]));
    assert_eq!(clashes(&mut s, "duct", "conduit")["total"], 1);
    assert_eq!(crossings_of(&mut s, "E-COND"), 1);
}

/// A pipe whose two segments sit at different heights (a drop between them) raised over a duct:
/// both segments go up by the move, the drop between them kept; its fittings move with it.
#[test]
fn a_raised_pipe_keeps_its_heights_relative_to_each_other() {
    let mut s = building(
        vec![system("M-SUPPLY", "duct", [12.0, 8.0]), system("P-DCW", "pipe", [2.0, 0.0])],
        vec![
            run("M-SUPPLY", [24.0, 60.0], [264.0, 60.0], [12.0, 8.0], 10.0),
            json!({"system": "P-DCW", "level": "Level 1", "points": [[100.0, 24.0], [100.0, 120.0], [180.0, 120.0]], "size_in": [2.0, 0.0], "bottoms_ft": [10.2, 10.5]}),
        ],
    );
    let c = clashes(&mut s, "P-DCW", "duct");
    assert_eq!(c["total"], 1, "{c}");
    let id = c["clashes"][0]["id"].as_str().unwrap().to_string();
    let f = s.execute("bim.clash.fix", &json!({"id": id, "strategy": "raise", "approve": true})).unwrap();
    assert_eq!((f["mover"]["system"].as_str(), f["check"]["clear"].as_bool()), (Some("P-DCW"), Some(true)), "{f}");
    assert_eq!(f["kept"]["kept_by"], "building spec", "{f}");
    let up_ft = f["move"][2].as_f64().unwrap() / 12.0;
    assert!(f["pieces_moved"].as_u64().unwrap() >= 2, "the segment and its elbow: {f}");
    let b = listed(&mut s, "P-DCW")["bottoms_ft"].clone();
    let (b0, b1) = (b[0].as_f64().unwrap(), b[1].as_f64().unwrap());
    assert!((b0 - (10.2 + up_ft)).abs() < 1e-6 && (b1 - (10.5 + up_ft)).abs() < 1e-6, "both raised by {up_ft}: {b}");
    stays_fixed(&mut s, "P-DCW", "duct", "P-DCW");
}

/// A short pipe stub across a duct's end, moved sideways off it: the run's path moves in the
/// spec, the edit finding it again by where it was.
#[test]
fn an_approved_offset_moves_the_run_in_plan_for_every_build() {
    let mut s = building(
        vec![system("M-SUPPLY", "duct", [12.0, 8.0]), system("P-DCW", "pipe", [2.0, 0.0])],
        vec![run("M-SUPPLY", [100.0, 0.0], [100.0, 200.0], [12.0, 8.0], 10.0), run("P-DCW", [80.0, 195.0], [130.0, 195.0], [2.0, 0.0], 10.2)],
    );
    let c = clashes(&mut s, "P-DCW", "duct");
    assert_eq!(c["total"], 1, "{c}");
    let id = c["clashes"][0]["id"].as_str().unwrap().to_string();
    // 8" clear of the duct's end: model.clashes measures runs in plan as round-ended (their
    // half-widths), so a pipe just past the square end still counts there.
    let f = s.execute("bim.clash.fix", &json!({"id": id, "strategy": "offset", "clearance_in": 8, "approve": true, "by": "agent"})).unwrap();
    assert_eq!((f["how"].as_str(), f["check"]["clear"].as_bool()), (Some("offset"), Some(true)), "{f}");
    let dy = f["move"][1].as_f64().unwrap();
    assert!(dy > 0.0 && f["move"][2] == 0.0, "{f}");
    let r = listed(&mut s, "P-DCW");
    assert_eq!(r["edited_by"], "agent", "{r}");
    assert_eq!(r["corrections"][0]["key"], "position", "{r}");
    assert_eq!(r["bottoms_ft"], json!([10.2]), "a move in plan keeps the height");
    stays_fixed(&mut s, "P-DCW", "duct", "P-DCW");
    // The moved pipe is still a spec run's: a second fix of it is kept the same way.
    let model: Vec<Value> = s.execute("bim.clash", &json!({"a": "P-DCW"})).unwrap()["clashes"].as_array().unwrap().clone();
    assert!(model.is_empty(), "{model:?}");
    let me = s.execute("mep.edits", &json!({})).unwrap();
    assert_eq!(me["applied"], 1, "{me}");
}

#[test]
fn a_raise_given_to_mep_move_keeps_a_runs_slope_and_bad_values_are_refused() {
    let mut s = building(
        vec![system("P-DCW", "pipe", [2.0, 0.0])],
        vec![
            json!({"system": "P-DCW", "level": "Level 1", "points": [[100.0, 24.0], [100.0, 120.0], [180.0, 120.0]], "size_in": [2.0, 0.0], "bottoms_ft": [9.0, 9.25]}),
        ],
    );
    let id = listed(&mut s, "P-DCW")["id"].as_str().unwrap().to_string();
    let m = s.execute("mep.move", &json!({"run": id, "rise_ft": 0.5, "reason": "over the beam"})).unwrap();
    assert_eq!(m["status"], "applied", "{m}");
    assert_eq!(listed(&mut s, "P-DCW")["bottoms_ft"], json!([9.5, 9.75]));
    for bad in [json!({"run": id, "rise_ft": "up"}), json!({"run": id, "rise_ft": 1e9}), json!({"run": id, "rise_ft": 1, "bottom_ft": 9})] {
        assert!(s.execute("mep.move", &bad).is_err(), "{bad}");
    }
}

/// The plan y of each built piece of `system`'s segments, by level.
fn piece_ys(s: &Session, system: &str) -> Vec<(String, f64)> {
    use cadaddy_engine::cmd::bimlook::made_tag;
    let d = s.doc().unwrap();
    let mut out: Vec<(String, f64)> = d
        .model
        .iter()
        .filter(|e| made_tag(e, "system") == Some(system) && made_tag(e, "part") == Some("segment"))
        .filter_map(|e| {
            let cadaddy_engine::doc::EntityKind::Mesh(m) = &e.kind else { return None };
            let y = m.vertices.iter().map(|v| v.y).sum::<f64>() / m.vertices.len().max(1) as f64;
            Some((cadaddy_engine::cmd::bspec::made_level(e).unwrap_or_default(), (y * 100.0).round() / 100.0))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A water riser through two storeys crossing a duct on the first, moved off it: the riser moves
/// on every storey it passes, now and in every build.
#[test]
fn an_approved_move_of_a_riser_moves_it_on_every_storey() {
    let mut s = Session::new();
    let level = |name: &str, i: u32| json!({"name": name, "elevation_ft": f64::from(i) * 10.0, "height_ft": 10.0, "ceiling_ft": 8.0});
    let spec = json!({
        "units_per_foot": 12, "stack": false,
        "levels": [level("Level 1", 0), level("Level 2", 1)],
        "mep": {
            "systems": [system("M-SUPPLY", "duct", [12.0, 8.0]), system("P-DCW", "pipe", [2.0, 0.0])],
            "runs": [
                {"system": "P-DCW", "level": "Level 1", "points": [[100.0, 100.0]], "size_in": [2, 0], "bottoms_ft": [0.5], "top_ft": 19.0},
                run("M-SUPPLY", [0.0, 100.0], [300.0, 100.0], [12.0, 8.0], 8.5)
            ]
        }
    });
    s.execute("bspec.set", &json!({"spec": spec, "replace": true})).unwrap();
    s.execute("model.build", &json!({"echo": false})).unwrap();
    let before = piece_ys(&s, "P-DCW");
    assert_eq!(before, vec![("Level 1".to_string(), 100.0), ("Level 2".to_string(), 100.0)], "a piece per storey");
    let c = clashes(&mut s, "P-DCW", "duct");
    assert_eq!(c["total"], 1, "{c}");
    let id = c["clashes"][0]["id"].as_str().unwrap().to_string();
    let f = s.execute("bim.clash.fix", &json!({"id": id, "strategy": "offset", "clearance_in": 8, "approve": true})).unwrap();
    assert_eq!((f["check"]["clear"].as_bool(), f["kept"]["kept_by"].as_str()), (Some(true), Some("building spec")), "{f}");
    let y = 100.0 + f["move"][1].as_f64().unwrap();
    assert!(y > 100.0, "{f}");
    let now = piece_ys(&s, "P-DCW");
    assert!(now.len() == 2 && now.iter().all(|(_, py)| (py - y).abs() < 0.02), "both storeys' pieces moved to y {y}: {now:?}");
    stays_fixed(&mut s, "P-DCW", "duct", "P-DCW");
    let rebuilt = piece_ys(&s, "P-DCW");
    assert!(rebuilt.len() == 2 && rebuilt.iter().all(|(_, py)| (py - y).abs() < 0.02), "a rebuild lays it where the fix put it: {rebuilt:?}");
}
