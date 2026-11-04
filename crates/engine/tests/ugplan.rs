//! The underground plan (Q6148) on a made-up slab-on-grade building: buried work is only what is
//! under the slab level (a homerun under an upper floor is in the ceiling below), each trade gets
//! its own generated sheet registered to the slab level, with the documents' callouts, its
//! legend, stub-ups and sleeves; pipes below a footing's bottom inside its bearing plane are
//! flagged; and the buried work's quantities come apart from the rest of the takeoff.

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// Feet to sheet units (inches).
fn ft(v: f64) -> f64 {
    v * 12.0
}

/// A 100' × 60' slab level and a level above it; a switchboard bottom-fed into a panel across
/// the slab, a conduit under an upper floor, a stack's base and a sanitary line below the slab,
/// a grade beam across the building, a footing beside the sanitary line.
fn spec() -> Value {
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": [
            {"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9},
            {"name": "Level 2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9}
        ],
        "shells": {"Level 1": [[0, 0, ft(100.0), 0], [ft(100.0), 0, ft(100.0), ft(60.0)], [ft(100.0), ft(60.0), 0, ft(60.0)], [0, ft(60.0), 0, 0]]},
        "walls": [
            {"level": "Level 1", "wall_type": "W1", "a": [ft(50.0), 0], "b": [ft(50.0), ft(60.0)]},
            {"level": "Level 2", "wall_type": "W1", "a": [ft(20.0), 0], "b": [ft(20.0), ft(60.0)]}
        ],
        "mep": {
            "systems": [
                {"name": "E-UG", "layer": "E-UG", "kind": "conduit", "size_in": [3, 3], "bottom_ft": -1.75},
                {"name": "E-BRANCH", "layer": "E-BRANCH", "kind": "conduit", "size_in": [0.75, 0.75], "bottom_ft": 9},
                {"name": "P-UG-SAN", "layer": "P-UG-SAN", "kind": "pipe", "size_in": [4, 0], "bottom_ft": -2},
                {"name": "SAN", "layer": "P-SAN", "kind": "pipe", "size_in": [4, 0], "bottom_ft": 0}
            ],
            "equipment": [
                {"tag": "SB1", "kind": "switchboard", "level": "Level 1", "min": [ft(90.0), ft(30.0)], "max": [ft(93.0), ft(33.0)], "bottom_ft": 0, "height_ft": 7},
                {"tag": "P2", "kind": "panel", "level": "Level 1", "min": [ft(10.0), ft(30.0)], "max": [ft(11.5), ft(30.5)], "bottom_ft": 4, "height_ft": 4}
            ],
            "feeders": [{"tag": "F1", "from": "SB1", "to": "P2", "conduit_in": 3, "conduits": 2, "sets": 2, "size": "350", "material": "cu", "entry": "bottom"}],
            "runs": [
                // The feeder under the slab across the grade beam, and its stub-ups.
                {"system": "E-UG", "level": "Level 1", "source_sheet": "feeder F1 SB1→P2", "height_source": "proposed",
                 "points": [[ft(91.5), ft(31.5)], [ft(10.75), ft(31.5)], [ft(10.75), ft(30.25)]], "size_in": [3, 3], "bottoms_ft": [-1.75, -1.75]},
                {"system": "E-UG", "level": "Level 1", "source_sheet": "feeder F1 SB1→P2", "height_source": "proposed",
                 "points": [[ft(91.5), ft(31.5)]], "size_in": [3, 3], "bottoms_ft": [-1.75], "top_ft": 0.5},
                // A homerun under Level 2's floor: Level 1's ceiling, not the ground.
                {"system": "E-BRANCH", "level": "Level 2", "points": [[ft(5.0), ft(5.0)], [ft(40.0), ft(5.0)]], "size_in": [0.75, 0.75], "bottoms_ft": [-1.0]},
                // The stack's base down to the building drain, and the building drain below the slab.
                {"system": "P-UG-SAN", "level": "Level 1", "source_sheet": "P-101", "points": [[ft(30.0), ft(10.0)]], "size_in": [4, 0], "bottoms_ft": [-2], "top_ft": 0},
                {"system": "SAN", "level": "Level 1", "source_sheet": "P-101", "points": [[ft(30.0), ft(10.0)], [ft(30.0), ft(-6.0)]], "size_in": [4, 0], "bottoms_ft": [-4.5]}
            ]
        },
        "concrete": {
            "elements": [
                {"id": "GB-1", "kind": "grade_beam", "level": "Level 1", "path": [[ft(50.0), 0], [ft(50.0), ft(60.0)]], "width_in": 16, "depth_in": 30, "top_ft": -0.33},
                {"id": "F-2", "kind": "footing", "level": "Level 1", "min": [ft(31.0), ft(2.0)], "max": [ft(34.0), ft(5.0)], "depth_in": 12, "top_ft": -2.0}
            ]
        }
    })
}

fn session() -> Session {
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": spec()})).unwrap();
    s
}

fn texts_of(s: &Session, layout: &str) -> Vec<String> {
    let d = s.doc().unwrap();
    d.space(&cadaddy_engine::doc::Space::Paper(layout.to_string()))
        .unwrap()
        .iter()
        .filter_map(|e| match &e.kind {
            cadaddy_engine::doc::EntityKind::Text(t) => Some(t.value.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn only_work_under_the_slab_level_is_buried_and_sleeves_bearing_and_takeoff_come_apart() {
    let mut s = session();
    let r = s.execute("ugsheets.check", &json!({})).unwrap();
    // The feeder is buried; the homerun under Level 2 is not.
    let runs = r["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "{r}");
    assert!(runs.iter().all(|x| x["level"] == "Level 1"), "{r}");
    // The building drain is the plumbing's, below the slab.
    assert_eq!(r["pipes"].as_array().unwrap().len(), 1, "{r}");
    // Stub-ups: the switchboard's (the run's end and its riser are one), the panel's, the stack's.
    let stubs = r["stubups"].as_array().unwrap();
    let p_stub: Vec<&Value> = stubs.iter().filter(|x| x["trade"] == "P").collect();
    assert_eq!(p_stub.len(), 1, "{r}");
    assert_eq!(p_stub[0]["feeds"], "SAN stack", "{r}");
    assert!(p_stub[0]["x"].is_object() && p_stub[0]["y"].is_object(), "dimensioned: {r}");
    assert_eq!(stubs.iter().filter(|x| x["trade"] == "E" && x["feeds"] == "SB1").count(), 1, "{r}");
    // The feeder crosses the grade beam: sleeved one inch over its raceway.
    let sleeves = r["sleeves"].as_array().unwrap();
    let gb: Vec<&Value> = sleeves.iter().filter(|x| x["through"].as_str().unwrap_or_default().contains("GB-1")).collect();
    assert_eq!(gb.len(), 1, "{r}");
    assert_eq!((gb[0]["trade"].clone(), gb[0]["size_in"].clone()), (json!("E"), json!(4.0)), "{r}");
    // The building drain leaves the slab: sleeved through its edge.
    assert!(sleeves.iter().any(|x| x["trade"] == "P" && x["through"] == "slab edge"), "{r}");
    // The drain is 4.5' + 4" deep, a foot from a footing whose bottom is 3' down: in its bearing plane.
    let plane: Vec<&Value> = r["findings"].as_array().unwrap().iter().filter(|f| f["check"] == "bearing_plane").collect();
    assert_eq!(plane.len(), 1, "{r}");
    assert!(plane[0]["message"].as_str().unwrap().contains("IPC 307.5"), "{r}");
    // The takeoff of the buried work, by trade.
    let e = &r["takeoff"]["E"];
    assert!(e["ft_proposed"].as_f64().unwrap() > 80.0 && e["ft_called"] == json!(0.0), "{e}");
    assert!(e["trench_ft"].as_f64().unwrap() > 80.0 && e["stub_ups"].as_u64().unwrap() >= 2 && e["sleeves"] == json!(1), "{e}");
    assert!(e["lines"].as_array().unwrap().iter().any(|l| l["size"] == "3\"" && l["elbows_90"].as_u64().unwrap() >= 2), "{e}");
    let p = &r["takeoff"]["P"];
    assert_eq!((p["ft_called"].clone(), p["stub_ups"].clone()), (json!(16.0), json!(1)), "{p}");
}

#[test]
fn each_trade_gets_its_sheet_registered_to_the_slab_level_with_its_sources_and_legend() {
    let mut s = session();
    let sources = json!([{"sheet": "E-1 note", "text": "TWO (2) 4\" SCH. 40 PVC CONDUIT, MIN. 36\" BELOW GRADE [PLACED]"}]);
    let alts = json!([{"subject": "feeder F9 SB1→P2", "points": [[ft(91.0), ft(20.0)], [ft(11.0), ft(20.0)]], "size_in": 2, "conduits": 1}, {"subject": "bad", "points": [[f64::NAN, 0]]}]);
    let b = s
        .execute(
            "ugsheets.build",
            &json!({"trade": "E", "number": "E-UG", "title": "ELECTRICAL UNDERGROUND PLAN (GENERATED)", "level": "Level 1", "scale": "1/8\" = 1'-0\"", "sources": sources, "alternatives": alts}),
        )
        .unwrap();
    let sh = &b["sheets"][0];
    assert_eq!(
        (sh["number"].clone(), sh["trade"].clone(), sh["registered"].clone(), sh["alternatives"].clone()),
        (json!("E-UG"), json!("E"), json!(true), json!(1)),
        "{sh}"
    );
    // The level's plan's scale when the plan fits at it.
    assert_eq!(sh["scale"], "1/8\" = 1'-0\"", "{sh}");
    let texts = texts_of(&s, sh["layout"].as_str().unwrap()).join(" | ");
    for want in [
        "ELECTRICAL UNDERGROUND PLAN (GENERATED)",
        "GENERATED BY CADADDY - NOT FOR CONSTRUCTION WITHOUT ENGINEER REVIEW",
        "LEGEND",
        "CALLED BY THE DOCUMENTS",
        "PROPOSED BY CADADDY",
        "CALLED (E-1 note): TWO (2) 4\" SCH. 40 PVC CONDUIT",
        "PROPOSED ALTERNATIVE - RFI",
        "SLEEVES - STRUCTURAL ENGINEER TO CONFIRM",
    ] {
        assert!(texts.contains(want), "{want}: {texts}");
    }
    // Walls in the background: the slab level's, not the level above's.
    let d = s.doc().unwrap();
    let paper: Vec<_> = d.space(&cadaddy_engine::doc::Space::Paper(sh["layout"].as_str().unwrap().into())).unwrap().iter().cloned().collect();
    // (The slab level's one wall and the legend's sample line.)
    assert_eq!(paper.iter().filter(|e| e.common.layer == "E-UG-BKGD" && matches!(e.kind, cadaddy_engine::doc::EntityKind::Line(_))).count(), 2);
    // Registered in the set's sheet list: a sheet point lands on the level's plan.
    let sp: Value = serde_json::from_str(d.building_spec.as_deref().unwrap()).unwrap();
    let link = sp["sheets"].as_array().unwrap().iter().find(|l| l["number"] == "E-UG").unwrap().clone();
    assert_eq!((link["level"].clone(), link["discipline"].clone()), (json!("Level 1"), json!("E")), "{link}");
    let reg = &link["registration"];
    let a = reg["a"].as_f64().unwrap();
    // 1/8" = 1'-0": a paper inch is 8 ft, 96 sheet units.
    assert!((a - 96.0).abs() < 1e-9, "{reg}");

    // The plumbing sheet: its own layout and layers, the stack's stub-up, the feeder for
    // coordination only.
    let b = s
        .execute("ugsheets.build", &json!({"trade": "P", "number": "P-UG", "title": "PLUMBING UNDERGROUND PLAN (GENERATED)", "level": "Level 1"}))
        .unwrap();
    let ps = &b["sheets"][0];
    assert_eq!((ps["number"].clone(), ps["runs"].clone(), ps["stubups"].clone()), (json!("P-UG"), json!(1), json!(1)), "{ps}");
    let d = s.doc().unwrap();
    let paper: Vec<_> = d.space(&cadaddy_engine::doc::Space::Paper(ps["layout"].as_str().unwrap().into())).unwrap().iter().cloned().collect();
    let on = |layer: &str| paper.iter().filter(|e| e.common.layer == layer).count();
    assert!(on("P-UG-COND") >= 1 && on("P-UG-STUB") >= 1 && on("P-UG-OTHR") >= 1, "the drain, the stack and the feeder");
    assert_eq!(on("E-UG-COND"), 0);
    let sp: Value = serde_json::from_str(d.building_spec.as_deref().unwrap()).unwrap();
    assert_eq!(sp["sheets"].as_array().unwrap().iter().filter(|l| l["source"].as_str().unwrap_or_default().starts_with("generated:")).count(), 2);

    // Built again: replaced, not added.
    s.execute("ugsheets.build", &json!({"trade": "P", "number": "P-UG", "level": "Level 1"})).unwrap();
    let sp: Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    assert_eq!(sp["sheets"].as_array().unwrap().iter().filter(|l| l["number"] == "P-UG").count(), 1);
}

#[test]
fn bad_parameters_are_refused_and_the_plan_report_needs_a_job() {
    let mut s = session();
    assert!(s.execute("ugsheets.build", &json!({"trade": "X"})).is_err());
    assert!(s.execute("ugsheets.plan", &json!({})).is_err());
    // A scale that doesn't fit falls back to one that does; a made-up one is ignored.
    for scale in ["1/4\" = 1'-0\"", "7/9\" = 1'-0\"", ""] {
        let b = s.execute("ugsheets.build", &json!({"scale": scale, "level": "Level 1"})).unwrap();
        assert_ne!(b["sheets"][0]["scale"], "1/4\" = 1'-0\"", "{b}");
    }
}
