//! Q6493: a vertical MEP run through several storeys is built, counted and clash-checked a piece
//! per storey, each on its own level, the pieces one run (the riser's name on each), each floor it
//! passes a penetration with its firestop.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bimlook::{made_prop, made_tag};
use cadaddy_engine::cmd::wallgen::made_as;
use serde_json::{Value, json};

/// Four storeys of 10 ft (and, with `apart`, each level's plan drawn in its own place), a
/// sanitary stack from under the slab up through the roof, and a 2" vent on level 3 beside it.
fn model(apart: bool) -> Session {
    let mut s = Session::new();
    let level = |name: &str, i: u32| {
        let mut l = json!({"name": name, "elevation_ft": f64::from(i) * 10.0, "height_ft": 10.0, "ceiling_ft": 8.0});
        if apart {
            let x0 = f64::from(i) * 2000.0;
            l["region"] = json!([x0, 0.0, x0 + 1000.0, 1000.0]);
        }
        l
    };
    let x = |i: u32| if apart { f64::from(i) * 2000.0 } else { 0.0 };
    let spec = json!({
        "units_per_foot": 12,
        "stack": false,
        "levels": [level("Level 1", 0), level("Level 2", 1), level("Level 3", 2), level("Level 4", 3)],
        "mep": {
            "systems": [
                {"name": "SAN", "layer": "P-SAN", "kind": "pipe", "size_in": [3, 0], "bottom_ft": 0},
                {"name": "V", "layer": "P-VENT", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 9}
            ],
            "runs": [
                {"system": "SAN", "level": "Level 1", "points": [[100.0, 100.0]], "size_in": [3, 0], "bottoms_ft": [-1.5], "top_ft": 41.0, "objects": ["5A"]},
                {"system": "V", "level": "Level 3", "points": [[x(2) + 40.0, 100.0], [x(2) + 300.0, 100.0]], "size_in": [2, 0], "bottoms_ft": [9.0]}
            ]
        }
    });
    s.execute("bspec.set", &json!({"spec": spec, "replace": true})).unwrap();
    s.execute("model.build", &json!({"echo": false})).unwrap();
    s
}

/// The built SAN segments: (level, bottom z, top z in feet, the riser's name, piece, floors).
fn san_pieces(s: &Session) -> Vec<(String, f64, f64, String, String, String)> {
    let d = s.doc().unwrap();
    let mut out: Vec<_> = d
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("mep") && made_tag(e, "system") == Some("SAN") && made_tag(e, "part") == Some("segment"))
        .map(|e| {
            let cadaddy_engine::doc::EntityKind::Mesh(m) = &e.kind else { panic!("a mesh") };
            let (lo, hi) = m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(v.z), b.max(v.z)));
            (
                cadaddy_engine::cmd::bspec::made_level(e).unwrap_or_default(),
                (lo / 12.0 * 100.0).round() / 100.0,
                (hi / 12.0 * 100.0).round() / 100.0,
                made_tag(e, "riser").unwrap_or_default().to_string(),
                made_prop(e, "RISER_PIECE").unwrap_or_default().to_string(),
                made_prop(e, "FLOOR_PENETRATION").unwrap_or_default().to_string(),
            )
        })
        .collect();
    out.sort_by(|a, b| a.1.total_cmp(&b.1));
    out
}

fn tree_count(s: &mut Session, level: &str, trade: &str) -> u64 {
    let t = s.execute("bim.tree", &json!({"level": level})).unwrap();
    t["levels"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| l["name"] == level)
        .flat_map(|l| l["trades"].as_array().cloned().unwrap_or_default())
        .filter(|tr| tr["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(trade)))
        .map(|tr| tr["count"].as_u64().unwrap_or(0))
        .sum()
}

fn san_length_by_level(s: &mut Session) -> Vec<(String, f64)> {
    let q = s.execute("mep.qty", &json!({"group_by": "level", "system": "SAN"})).unwrap();
    let mut out: Vec<(String, f64)> = q["pipes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["level"].as_str().unwrap_or_default().to_string(), (r["length_ft"].as_f64().unwrap_or(0.0) * 100.0).round() / 100.0))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn a_stack_through_four_storeys_is_a_piece_on_each() {
    let mut s = model(false);
    let ps = san_pieces(&s);
    let got: Vec<(&str, f64, f64)> = ps.iter().map(|p| (p.0.as_str(), p.1, p.2)).collect();
    assert_eq!(got, vec![("Level 1", -1.5, 10.0), ("Level 2", 10.0, 20.0), ("Level 3", 20.0, 30.0), ("Level 4", 30.0, 41.0)], "{ps:#?}");
    // One run: the riser's name on each piece, which piece it is, the floor each passes.
    assert!(ps.iter().all(|p| !p.3.is_empty() && p.3 == ps[0].3), "{ps:#?}");
    assert_eq!(ps.iter().map(|p| p.4.as_str()).collect::<Vec<_>>(), vec!["1 of 4", "2 of 4", "3 of 4", "4 of 4"]);
    assert_eq!(ps.iter().map(|p| p.5.as_str()).collect::<Vec<_>>(), vec!["Level 1", "Level 2", "Level 3", "Level 4"]);
    // Each level's tree holds its storey's piece.
    for l in ["Level 1", "Level 2", "Level 3", "Level 4"] {
        assert!(tree_count(&mut s, l, "plumbing") >= 1, "{l} has no plumbing");
    }
    // The takeoff: each storey's length on its level, 42.5 ft in all.
    assert_eq!(
        san_length_by_level(&mut s),
        vec![("Level 1".into(), 11.5), ("Level 2".into(), 10.0), ("Level 3".into(), 10.0), ("Level 4".into(), 11.0)]
    );
    // A floor firestop where it passes each floor over the lowest, on that floor.
    let d = s.doc().unwrap();
    let mut stops: Vec<String> = d
        .model
        .iter()
        .filter(|e| made_tag(e, "part") == Some("atom:firestop") && made_tag(e, "type").is_some_and(|t| t.contains("floor")))
        .filter_map(|e| cadaddy_engine::cmd::bspec::made_level(e))
        .collect();
    stops.sort();
    stops.dedup();
    assert_eq!(stops, vec!["Level 2".to_string(), "Level 3".into(), "Level 4".into()]);
}

#[test]
fn a_stack_on_plans_drawn_apart_is_on_each_levels_plan() {
    let s = model(true);
    let d = s.doc().unwrap();
    // Each level's piece over the stack's spot on its own level's plan.
    let mut at: Vec<(String, f64)> = d
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("mep") && made_tag(e, "system") == Some("SAN"))
        .filter_map(|e| {
            let b = cadaddy_engine::doc::entity_bounds(d, e, 0);
            Some((cadaddy_engine::cmd::bspec::made_level(e)?, b.center().x.round()))
        })
        .collect();
    at.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(at, vec![("Level 1".into(), 100.0), ("Level 2".into(), 2100.0), ("Level 3".into(), 4100.0), ("Level 4".into(), 6100.0)]);
}

#[test]
fn the_stack_clashes_in_each_storeys_scope_and_not_with_its_own_floors() {
    let mut s = model(false);
    let c = s.execute("model.clashes", &json!({})).unwrap();
    let all: Vec<&Value> = c["clashes"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let san: Vec<&&Value> = all.iter().filter(|x| x["systems"].as_array().is_some_and(|s| s.iter().any(|v| v == "SAN"))).collect();
    // Its pieces pass their floors by design: no "into the floor structure" and no "below the
    // ceiling" for a riser.
    assert!(san.iter().all(|x| x["kind"] != "into_structure" && x["kind"] != "below_ceiling"), "{san:#?}");
    // The vent across it on level 3 meets level 3's piece.
    assert!(san.iter().any(|x| x["kind"] == "crossing" && x["level"] == "Level 3"), "the vent through the stack on level 3: {all:#?}");
}

/// Q6505: a stack drawn a storey at a time (a run per level, each from its floor up to the floor
/// over it) passes through each storey: not "below the ceiling", not "into the floor structure".
/// A riser that stops short of the floor over it and a duct hung under the ceiling still are.
#[test]
fn a_stack_drawn_a_storey_at_a_time_passes_through_each_storey() {
    let mut s = Session::new();
    let level = |name: &str, i: u32| json!({"name": name, "elevation_ft": f64::from(i) * 10.0, "height_ft": 10.0, "ceiling_ft": 8.0});
    let piece = |lv: &str| json!({"system": "SAN", "level": lv, "points": [[100.0, 100.0]], "size_in": [4, 0], "bottoms_ft": [0.0], "top_ft": 10.0});
    let spec = json!({
        "units_per_foot": 12,
        "stack": false,
        "levels": [level("Level 1", 0), level("Level 2", 1), level("Level 3", 2), level("Roof", 3)],
        "mep": {
            "systems": [
                {"name": "SAN", "layer": "P-SAN", "kind": "pipe", "size_in": [4, 0], "bottom_ft": 0},
                {"name": "CW", "layer": "P-CW", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 0},
                {"name": "SA", "layer": "M-SA", "kind": "duct", "size_in": [12, 8], "bottom_ft": 9}
            ],
            "runs": [
                piece("Level 1"), piece("Level 2"), piece("Level 3"),
                {"system": "CW", "level": "Level 2", "points": [[400.0, 400.0]], "size_in": [1, 0], "bottoms_ft": [0.0], "top_ft": 5.0},
                {"system": "SA", "level": "Level 2", "points": [[600.0, 40.0], [900.0, 40.0]], "size_in": [12, 8], "bottoms_ft": [6.0]}
            ]
        }
    });
    s.execute("bspec.set", &json!({"spec": spec, "replace": true})).unwrap();
    s.execute("model.build", &json!({"echo": false})).unwrap();
    let c = s.execute("model.clashes", &json!({})).unwrap();
    let all: Vec<&Value> = c["clashes"].as_array().map(|a| a.iter().collect()).unwrap_or_default();
    let of = |sys: &str, kind: &str| all.iter().filter(|x| x["kind"] == kind && x["systems"][0] == sys).count();
    assert_eq!((of("SAN", "below_ceiling"), of("SAN", "into_structure")), (0, 0), "{all:#?}");
    assert_eq!(of("CW", "below_ceiling"), 1, "a riser ending 5 ft up, in mid-storey: {all:#?}");
    assert_eq!(of("SA", "below_ceiling"), 1, "the duct hung 6 ft up: {all:#?}");
}
