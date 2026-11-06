//! Q6236 fix-underground-pvc-missing: raceway below the slab or in the earth is built and counted
//! as the job's underground raceway (the spec's wiring-methods table, else Schedule 40 PVC by the
//! rules), with its elbows and couplings, no straps in the ground, and a transition where it
//! comes up into the raceway above the floor; a callout's steel elbows and risers are steel.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use cadaddy_engine::doc::props::{self, Prop};
use serde_json::{Value, json};

/// A made-up one-storey building: a drawn 20' run of 3/4" conduit over the ceiling, and buried
/// work of system `system`: 40' of 1" two feet under the floor, stubbed up at both ends to a foot
/// above it (each stub its own riser run). `why`: the buried runs' height source.
fn building(system: &str, why: &str, methods: Option<Value>) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    if let Some(m) = methods {
        let d = s.doc_mut().expect("doc");
        d.embedded.insert("route/methods.json".into(), std::sync::Arc::new(serde_json::to_vec(&m).expect("json")));
    }
    let ug = |pts: Value, bottom: f64, top: Option<f64>| {
        json!({"system": system, "level": "L1", "points": pts, "size_in": [1, 0], "bottoms_ft": [bottom], "top_ft": top,
               "size_source": "note", "height_source": why, "source_sheet": "Plan 1"})
    };
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "mep": {
            "systems": [
                {"name": "E-COND", "layer": "E-COND", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 10},
                {"name": system, "layer": system, "kind": "conduit", "size_in": [1, 0], "bottom_ft": -2}
            ],
            "runs": [
                {"system": "E-COND", "level": "L1", "points": [[0, 600], [240, 600]], "size_in": [0.75, 0], "bottoms_ft": [10], "size_source": "tag", "height_source": "tag"},
                ug(json!([[0, 0]]), -2.0, Some(1.0)),
                ug(json!([[0, 0], [480, 0]]), -2.0, None),
                ug(json!([[480, 0]]), -2.0, Some(1.0))
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({"route": false})).expect("build");
    assert!(built["atoms"].as_u64().unwrap_or(0) > 0, "{built}");
    s
}

/// The buried system's pieces: (kind, item, type, length ft, the source of its type, the rule
/// that made it).
fn pieces(s: &Session, system: &str) -> Vec<(String, String, String, f64, String, String)> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .map(|e| props::read(&e.common.xdata))
        .filter(|p| p.get("system").map(Prop::text).as_deref() == Some(system) && p.get("atom").is_some())
        .map(|p| {
            let g = |k: &str| p.get(k).map(Prop::text).unwrap_or_default();
            let src = p.get("type").and_then(|x| x.source.doc.clone()).unwrap_or_default();
            (g("atom"), g("item"), g("type"), g("length_ft").parse::<f64>().unwrap_or(0.0), src, g("source_rule"))
        })
        .collect()
}

fn count(s: &mut Session, q: Value) -> Value {
    s.execute("bim.count", &q).expect("count")
}

#[test]
fn buried_conduit_is_built_as_pvc_with_its_elbows_couplings_and_a_transition_where_it_comes_up() {
    let mut s = building("E-UG", "called: Plan 1 note (telecom service)", None);
    let ps = pieces(&s, "E-UG");
    let sticks: Vec<_> = ps.iter().filter(|p| p.0 == "conduit").collect();
    // 40' across and the two 3' stubs, less what its two elbows take: all PVC, none EMT.
    let ft: f64 = sticks.iter().map(|p| p.3).sum();
    assert!(ft > 40.0 && ft <= 46.0 + 1e-6, "{ps:?}");
    assert!(sticks.iter().all(|p| p.2 == "PVC" && p.1 == "PVC conduit"), "{ps:?}");
    assert!(sticks.iter().all(|p| p.4.contains("below the slab or in the earth")), "the rule is the type's source: {ps:?}");
    assert_eq!(count(&mut s, json!({"kind": "conduit", "system": "E-UG", "type": "EMT"}))["count"], json!(0));
    assert_eq!(count(&mut s, json!({"kind": "elbow", "system": "E-UG", "type": "PVC"}))["count"], json!(2));
    assert!(count(&mut s, json!({"kind": "coupling", "system": "E-UG", "item": "PVC coupling"}))["count"].as_u64().unwrap_or(0) >= 4);
    // Where it comes up a foot above the floor it runs on in the raceway above: a transition at
    // each stub, no connector or locknut there.
    let tr = count(&mut s, json!({"kind": "coupling", "system": "E-UG", "item": "PVC to EMT transition adapter"}));
    assert_eq!(tr["count"], json!(2), "{ps:?}");
    assert!(ps.iter().filter(|p| p.1.contains("transition")).all(|p| p.5.contains("comes up through the floor")), "{ps:?}");
    assert_eq!(count(&mut s, json!({"kind": "connector", "system": "E-UG"}))["count"], json!(0));
    assert_eq!(count(&mut s, json!({"kind": "locknut", "system": "E-UG"}))["count"], json!(0));
    // In the ground nothing is strapped (the stubs above the floor are a foot long).
    assert_eq!(count(&mut s, json!({"kind": "strap", "system": "E-UG"}))["count"], json!(0), "{ps:?}");
    // The run over the ceiling stays EMT, strapped.
    assert_eq!(count(&mut s, json!({"kind": "conduit", "system": "E-COND", "type": "EMT"}))["length_ft"], json!(20.0));
    assert!(count(&mut s, json!({"kind": "strap", "system": "E-COND"}))["count"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn the_takeoff_has_its_pvc_lines_with_the_fittings() {
    let mut s = building("E-UG", "proposed", None);
    let bom = s.execute("bim.bom", &json!({"group_by": "job"})).expect("bom");
    let lines = bom["lines"].as_array().cloned().unwrap_or_default();
    let net = |item: &str| lines.iter().filter(|l| l["item"] == json!(item)).map(|l| l["net"].as_f64().unwrap_or(0.0)).sum::<f64>();
    let pvc = net("PVC conduit");
    assert!(pvc > 40.0 && pvc <= 46.0 + 1e-6, "{lines:?}");
    assert!((net("EMT conduit") - 20.0).abs() < 1e-6, "only the run over the ceiling is EMT: {lines:?}");
    assert_eq!(net("PVC 90° elbow"), 2.0, "{lines:?}");
    assert_eq!(net("PVC to EMT transition adapter"), 2.0, "{lines:?}");
    assert!(net("PVC coupling") >= 4.0, "{lines:?}");
}

#[test]
fn a_run_below_the_slab_on_any_system_is_buried_work() {
    // A conduit drawn on the plans' own layer, read two feet under the slab level's floor.
    let mut s = building("E-COND-2", "tag", None);
    assert!(count(&mut s, json!({"kind": "conduit", "system": "E-COND-2", "type": "PVC"}))["length_ft"].as_f64().unwrap_or(0.0) > 40.0);
    assert_eq!(count(&mut s, json!({"kind": "conduit", "system": "E-COND-2", "type": "EMT"}))["count"], json!(0));
}

#[test]
fn the_specs_underground_method_wins_over_the_rules() {
    for (app, methods, want) in
        [("Underground, below slab", vec!["Rigid galvanized steel (RGS)"], "RMC"), ("BELOW GRADE AND IN SLAB", vec!["Schedule 80 PVC", "RGS"], "PVC")]
    {
        let m = json!({"applications": [
            {"section": "26 05 33", "item": "3.1", "application": "Exposed, dry", "methods": ["EMT"]},
            {"section": "26 05 33", "item": "3.1", "application": app, "methods": methods}
        ]});
        let s = building("E-UG", "proposed", Some(m));
        let ps = pieces(&s, "E-UG");
        let sticks: Vec<_> = ps.iter().filter(|p| p.0 == "conduit").collect();
        assert!(!sticks.is_empty() && sticks.iter().all(|p| p.2 == want), "{app}: {ps:?}");
        assert!(sticks.iter().all(|p| p.4.contains("spec 26 05 33")), "{app}: the spec is the source: {ps:?}");
    }
}

#[test]
fn a_callouts_steel_elbows_and_risers_are_steel() {
    let mut s = building("E-UG", "called: Plan 1 note (service_secondary, steel elbows and risers)", None);
    let ps = pieces(&s, "E-UG");
    assert_eq!(count(&mut s, json!({"kind": "elbow", "system": "E-UG", "type": "RMC"}))["count"], json!(2), "{ps:?}");
    let ft = |ty: &str| ps.iter().filter(|p| p.0 == "conduit" && p.2 == ty).map(|p| p.3).sum::<f64>();
    // The 40' across is PVC; the risers are steel.
    assert!(ft("PVC") > 30.0 && ft("PVC") <= 40.0 + 1e-6, "{ps:?}");
    assert!(ft("RMC") > 0.0 && ft("RMC") <= 6.0 + 1e-6, "{ps:?}");
    assert!(ps.iter().filter(|p| p.2 == "RMC").all(|p| p.5.contains("steel elbows and risers")), "{ps:?}");
}

#[test]
fn a_spec_not_built_into_pieces_takes_off_its_buried_runs_as_pvc_unstrapped() {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "mep": {
            "systems": [
                {"name": "E-COND", "layer": "E-COND", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 10},
                {"name": "E-UG", "layer": "E-UG", "kind": "conduit", "size_in": [1, 0], "bottom_ft": -2}
            ],
            "runs": [
                {"system": "E-COND", "level": "L1", "points": [[0, 600], [240, 600]], "size_in": [0.75, 0], "bottoms_ft": [10]},
                {"system": "E-UG", "level": "L1", "points": [[0, 0], [480, 0]], "size_in": [1, 0], "bottoms_ft": [-2]}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let bom = s.execute("bim.bom", &json!({"group_by": "job"})).expect("bom");
    let lines = bom["lines"].as_array().cloned().unwrap_or_default();
    let net = |item: &str| lines.iter().filter(|l| l["item"] == json!(item)).map(|l| l["net"].as_f64().unwrap_or(0.0)).sum::<f64>();
    assert!((net("PVC conduit") - 40.0).abs() < 1e-6, "{lines:?}");
    assert!((net("EMT conduit") - 20.0).abs() < 1e-6, "{lines:?}");
    assert_eq!(net("PVC coupling"), 3.0, "{lines:?}");
    // The straps are the run over the ceiling's only.
    assert_eq!(net("Conduit strap"), 3.0, "{lines:?}");
}
