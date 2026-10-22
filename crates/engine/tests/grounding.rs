//! Grounding and bonding (Q329) on a made-up hotel corner: cold water and gas entering on level 1
//! (the water above the laundry's gypsum ceiling, the gas below it), the switchboard MDP and a
//! transformer, a concrete-encased electrode in a footing, building steel, and two BOH data rooms.
//! Every electrode and bond is routed and counted; a missing data-room bond and an inaccessible
//! water clamp are caught; the Ufer is checked against its note and its pour hold point; a missed
//! Ufer gets its alternative electrode and takeoff.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn spec() -> Value {
    json!({
        "version": 1, "units_per_foot": 12,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9},
            {"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9}
        ],
        "rooms": [
            {"name": "LAUNDRY", "number": "101", "level": "L1"},
            {"name": "BOH DATA 1", "number": "105", "level": "L1"},
            {"name": "BOH DATA 2", "number": "205", "level": "L2"},
            {"name": "GUEST ROOM", "number": "201", "level": "L2"}
        ],
        "rcp": {"ceilings": [
            {"level": "L1", "room": "LAUNDRY", "outline": [[-24, 480], [240, 480], [240, 720], [-24, 720]], "height_ft": 9}
        ]},
        "mep": {
            "systems": [
                {"name": "CW", "layer": "P-DOM-CW", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 10},
                {"name": "GAS", "layer": "P-GAS", "kind": "pipe", "size_in": [1.5, 0], "bottom_ft": 8}
            ],
            "runs": [
                {"system": "CW", "level": "L1", "points": [[0, 600], [300, 600], [300, 300]], "size_in": [2, 0], "bottoms_ft": [10, 10]},
                {"system": "GAS", "level": "L1", "points": [[0, 100], [200, 100]], "size_in": [1.5, 0], "bottoms_ft": [8]}
            ],
            "equipment": [
                {"tag": "MDP", "kind": "switchboard", "level": "L1", "min": [360, 0], "max": [432, 36], "bottom_ft": 0, "height_ft": 7.5},
                {"tag": "TX-1", "kind": "transformer", "level": "L1", "min": [480, 0], "max": [498, 12], "bottom_ft": 0, "height_ft": 2.5}
            ],
            "grounding": {
                "service": "MDP",
                "electrodes": [
                    {"tag": "W-1", "kind": "water_pipe", "level": "L1", "entry": "POE-W", "gec_size": "3/0"},
                    {"tag": "UF-1", "kind": "concrete_encased", "level": "L1", "at": [400, -24], "z_ft": -2, "gec_size": "#4", "length_ft": 20, "conductor": "#4", "material": "bare_cu", "cover_in": 3, "earth_contact": true, "pour": "pending", "in_conduit": true},
                    {"tag": "STL-1", "kind": "building_steel", "level": "L1", "at": [600, 120], "z_ft": 1, "gec_size": "3/0"}
                ],
                "bonds": [
                    {"tag": "GAS-1", "kind": "gas", "to": "POE-G", "size": "#6"},
                    {"tag": "MBJ", "kind": "main_bonding_jumper", "to": "MDP", "size": "3/0"},
                    {"tag": "SDS-1", "kind": "separately_derived", "from": "TX-1", "to": "STL-1", "size": "#2"},
                    {"tag": "TBB-1", "kind": "telecom", "to": "TMGB", "size": "3/0"}
                ],
                "busbars": [
                    {"tag": "TMGB", "kind": "tmgb", "room": "BOH DATA 1", "level": "L1", "at": [600, 300], "size": "1/4\" x 4\" x 12\""},
                    {"tag": "TGB-2", "kind": "tgb", "room": "BOH DATA 2", "level": "L2", "at": [600, 300], "size": "1/4\" x 2\" x 12\""}
                ],
                "required": [
                    {"kind": "water_pipe", "note": "BOND TO COLD WATER WITHIN 5' OF ENTRY", "sheet": "E-001"},
                    {"kind": "gas", "to": "POE-G", "note": "BOND GAS PIPING", "sheet": "E-001"},
                    {"kind": "concrete_encased", "note": "CONCRETE-ENCASED ELECTRODE, 20' OF #4 BARE CU", "sheet": "E-501"},
                    {"kind": "building_steel", "note": "BOND BUILDING STEEL", "sheet": "E-501"},
                    {"kind": "telecom", "to": "TMGB", "note": "3/0 TBB TO TMGB", "sheet": "T-001"},
                    {"kind": "telecom", "to": "TGB-2", "note": "#6 BC TO TGB IN BOH DATA 2", "sheet": "T-001"}
                ]
            }
        }
    })
}

fn session() -> Session {
    let mut s = Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": spec()})).unwrap();
    s
}

fn errors(r: &Value) -> Vec<(String, String)> {
    r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "error")
        .map(|f| (f["check"].as_str().unwrap().to_string(), f["about"].as_str().unwrap().to_string()))
        .collect()
}

fn set_grounding(s: &mut Session, f: impl FnOnce(&mut Value)) {
    let mut sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    f(&mut sp["mep"]["grounding"]);
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).unwrap();
}

#[test]
fn every_electrode_and_bond_is_routed_and_what_is_missing_or_unreachable_is_caught() {
    let mut s = session();
    // The points of entry, from the piping: the water above the laundry's gypsum ceiling.
    let e = s.execute("grounding.entries", &json!({})).unwrap();
    let w = &e["entries"][0];
    assert_eq!(
        (w["tag"].as_str(), w["at"].clone(), w["z_ft"].as_f64(), w["accessible"].as_bool()),
        (Some("POE-W"), json!([0.0, 600.0]), Some(10.0), Some(false)),
        "{e}"
    );
    assert!(w["why"].as_str().unwrap().contains("gypsum ceiling of LAUNDRY"));
    assert_eq!((e["entries"][1]["tag"].as_str(), e["entries"][1]["accessible"].as_bool()), (Some("POE-G"), Some(true)));
    // Before routing: what the notes call for isn't routed.
    let c = s.execute("grounding.check", &json!({})).unwrap();
    assert!(errors(&c).iter().any(|(k, _)| k == "not routed"), "{c}");
    // Routed: risers and all.
    let r = s.execute("grounding.route", &json!({})).unwrap();
    assert_eq!(r["routed"].as_array().unwrap().len(), 7, "{r}");
    assert!(r["failed"].as_array().unwrap().is_empty());
    let len = |tag: &str| r["routed"].as_array().unwrap().iter().find(|x| x["tag"] == tag).unwrap()["length_ft"].as_f64().unwrap();
    // W-1: MDP's ground bar (396, 18) at 0.5' up to 9.5', across 33' and 48.5', to the clamp at 10'.
    assert!((len("W-1") - (9.0 + 33.0 + 48.5 + 0.5)).abs() < 0.11, "{}", len("W-1"));
    // TBB-1 goes along the floor plan; the Ufer's GEC goes down into the footing.
    assert!(len("UF-1") > 11.5, "{}", len("UF-1"));
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    let uf = &sp["mep"]["grounding"]["electrodes"][1]["path"];
    assert_eq!(uf.as_array().unwrap().last().unwrap()[2], -2.0, "down to the electrode in the footing: {uf}");
    // Caught: the inaccessible water clamp and the data room without its bond (both ways).
    let c = s.execute("grounding.check", &json!({})).unwrap();
    let e = errors(&c);
    assert!(e.contains(&("inaccessible clamp".into(), "W-1".into())), "{e:?}");
    assert!(e.contains(&("not modelled".into(), "telecom to TGB-2".into())), "{e:?}");
    assert!(e.contains(&("data room".into(), "BOH DATA 2".into())), "{e:?}");
    assert_eq!(e.len(), 3, "{e:?}");
    assert!(c["findings"].as_array().unwrap().iter().any(|f| f["check"] == "pour" && f["severity"] == "warning"), "the pour is a hold point");
    // Each is an RFI on review (once).
    let rv = s.execute("grounding.check", &json!({"review": true})).unwrap();
    assert_eq!(rv["rfis"].as_array().unwrap().len(), 3);
    assert_eq!(s.execute("rfi.list", &json!({"kind": "grounding"})).unwrap()["total"], 3);
    assert_eq!(s.execute("grounding.check", &json!({"review": true})).unwrap()["rfis"].as_array().unwrap().len(), 0);
    // Fixed: the bond to BOH DATA 2's busbar, and the water clamp at an accessible spot.
    set_grounding(&mut s, |g| {
        g["bonds"].as_array_mut().unwrap().push(json!({"tag": "TBB-2", "kind": "telecom", "from": "TMGB", "to": "TGB-2", "size": "#6"}));
        g["entries"] = json!([{"tag": "POE-W", "kind": "water", "level": "L1", "at": [24, 600], "z_ft": 4, "accessible": true}]);
    });
    s.execute("grounding.route", &json!({"all": true})).unwrap();
    let c = s.execute("grounding.check", &json!({})).unwrap();
    assert!(errors(&c).is_empty(), "{c}");
    // TBB-2 rises from L1 to L2 at the busbar.
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    let tbb2 = sp["mep"]["grounding"]["bonds"][4]["path"].as_array().unwrap().clone();
    assert!(tbb2.iter().any(|p| p[2].as_f64().unwrap() > 20.0), "{tbb2:?}");
}

#[test]
fn the_ufer_is_checked_against_its_note_and_a_missed_one_gets_its_alternative() {
    let mut s = session();
    s.execute("grounding.route", &json!({})).unwrap();
    set_grounding(&mut s, |g| {
        let u = &mut g["electrodes"][1];
        u["length_ft"] = json!(15);
        u["cover_in"] = json!(1.5);
        u["conductor"] = json!("#6");
        u["earth_contact"] = json!(false);
        u["connection_accessible"] = json!(false);
    });
    let c = s.execute("grounding.check", &json!({})).unwrap();
    let ufer: Vec<String> =
        c["findings"].as_array().unwrap().iter().filter(|f| f["check"] == "ufer").map(|f| f["message"].as_str().unwrap().to_string()).collect();
    assert_eq!(ufer.len(), 5, "{ufer:#?}");
    assert!(ufer.iter().any(|m| m.contains("15 ft in the footing, the note calls for 20 ft")));
    assert!(ufer.iter().any(|m| m.contains("#6 bare copper, at least #4")));
    // Rebar instead: its diameter is what counts.
    set_grounding(&mut s, |g| {
        let u = &mut g["electrodes"][1];
        u["length_ft"] = json!(20);
        u["cover_in"] = json!(3);
        u["material"] = json!("rebar");
        u["rebar_in"] = json!(0.375);
        u["earth_contact"] = json!(true);
        u["connection_accessible"] = json!(true);
    });
    let c = s.execute("grounding.check", &json!({})).unwrap();
    let ufer: Vec<&Value> = c["findings"].as_array().unwrap().iter().filter(|f| f["check"] == "ufer").collect();
    assert_eq!(ufer.len(), 1, "{c}");
    assert!(ufer[0]["message"].as_str().unwrap().contains("0.375\" rebar"));
    // The pour is past and nothing documents it: flagged, then the alternative modelled.
    set_grounding(&mut s, |g| {
        let u = &mut g["electrodes"][1];
        u["material"] = json!("bare_cu");
        u["pour"] = json!("poured");
        u["documented"] = json!(false);
    });
    let c = s.execute("grounding.check", &json!({})).unwrap();
    assert!(errors(&c).contains(&("pour".into(), "UF-1".into())), "{c}");
    let before = s.execute("grounding.takeoff", &json!({})).unwrap();
    let a = s.execute("grounding.alternative", &json!({"electrode": "UF-1"})).unwrap();
    assert_eq!((a["tag"].as_str(), a["kind"].as_str()), (Some("UF-1-ALT"), Some("ground_rod")));
    assert!(s.execute("grounding.alternative", &json!({"electrode": "UF-1"})).is_err(), "once");
    s.execute("grounding.route", &json!({})).unwrap();
    let c = s.execute("grounding.check", &json!({})).unwrap();
    assert!(!errors(&c).iter().any(|(k, _)| k == "pour"), "{c}");
    // Its takeoff: two rods, two test wells, two welds, its GEC: on top of what was there.
    let after = s.execute("grounding.takeoff", &json!({})).unwrap();
    let qty = |t: &Value, item: &str| {
        t["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["item"].as_str().unwrap().starts_with(item))
            .map(|i| i["qty"].as_f64().unwrap())
            .sum::<f64>()
    };
    assert_eq!(qty(&after, "Ground rod"), 2.0);
    assert_eq!(qty(&after, "Ground test well"), 2.0);
    assert_eq!(qty(&after, "Exothermic weld, cable to rod"), 2.0);
    assert!(qty(&after, "Grounding conductor, #4 copper") > qty(&before, "Grounding conductor, #4 copper"));
    let rod = after["items"].as_array().unwrap().iter().find(|i| i["item"].as_str().unwrap().starts_with("Ground rod")).unwrap();
    assert_eq!(rod["note"], "alternative to missed UF-1");
}

#[test]
fn the_takeoff_counts_everything_routed_and_it_builds_in_3d() {
    let mut s = session();
    set_grounding(&mut s, |g| {
        g["bonds"].as_array_mut().unwrap().push(json!({"tag": "TBB-2", "kind": "telecom", "from": "TMGB", "to": "TGB-2", "size": "#6"}));
    });
    s.execute("grounding.route", &json!({})).unwrap();
    let t = s.execute("grounding.takeoff", &json!({})).unwrap();
    let items = t["items"].as_array().unwrap();
    let qty = |item: &str| items.iter().find(|i| i["item"] == item).map_or(0.0, |i| i["qty"].as_f64().unwrap());
    println!("{}", serde_json::to_string_pretty(&t).unwrap());
    assert_eq!(qty("Water pipe ground clamp"), 1.0);
    assert_eq!(qty("Gas pipe bonding clamp"), 1.0);
    assert_eq!(qty("Grounding conductor, #4 bare copper"), 20.0, "the Ufer's conductor in the footing");
    assert!(qty("Grounding conductor, 3/0 copper") > 100.0, "W-1, STL-1, MBJ, TBB-1 with makeup");
    assert!(qty("Grounding conductor, #6 copper") > 12.0, "GAS-1 and TBB-2 (its riser too)");
    assert_eq!(qty("Grounding busbar (TMGB), 1/4\" x 4\" x 12\""), 1.0);
    assert_eq!(qty("Grounding busbar (TGB), 1/4\" x 2\" x 12\""), 1.0);
    assert_eq!(qty("Two-hole compression lug on busbar"), 3.0, "TBB-1 at the TMGB, TBB-2 at both");
    assert_eq!(qty("Exothermic weld, cable to cable"), 1.0, "the Ufer's stub-up");
    assert_eq!(qty("Exothermic weld, cable to steel"), 1.0);
    assert_eq!(qty("Weld metal cartridge"), 2.0);
    assert_eq!(qty("Mould handle clamp and flint igniter (tool)"), 1.0);
    assert_eq!(qty("Bonding bushing (GEC in metal raceway)"), 2.0, "the Ufer's GEC in conduit");
    assert!(qty("Conduit for grounding conductors") > 10.0);
    assert_eq!(qty("Bonding jumper connection (bonding jumper lugs)"), 4.0, "MBJ and the transformer's");
    // Compression instead of exothermic: a crimper, no moulds.
    s.execute("project.rules.set", &json!({"domain": "custom", "key": "ground_connection", "value": "compression"})).unwrap();
    let t = s.execute("grounding.takeoff", &json!({})).unwrap();
    let names: Vec<&str> = t["items"].as_array().unwrap().iter().map(|i| i["item"].as_str().unwrap()).collect();
    assert!(names.contains(&"Compression crimper with dies (tool)") && !names.iter().any(|n| n.contains("mould")), "{names:?}");
    // On the MEP takeoff.
    let q = s.execute("mep.qty", &json!({})).unwrap();
    assert!(q["csv"].as_str().unwrap().lines().any(|l| l.starts_with("Grounding,,,Water pipe ground clamp")), "{}", q["csv"]);
    // 3D: conductors, the Ufer, busbars.
    let b = s.execute("model.build", &json!({})).unwrap();
    assert!(b["grounding"].as_u64().unwrap() >= 10, "{b}");
    if let Ok(dir) = std::env::var("GROUND_SHOTS") {
        let png = std::path::Path::new(&dir).join("grounding-3d.png");
        s.execute(
            "model.render",
            &json!({"path": png.display().to_string(), "style": "shaded", "width": 1000, "height": 700, "dir": [-1, -1.4, 0.9]}),
        )
        .unwrap();
    }
}

#[test]
fn hostile_grounding_never_panics() {
    let mut s = Session::new();
    assert!(s.execute("grounding.check", &json!({})).is_err(), "no spec");
    let mut sp = spec();
    sp["mep"]["grounding"]["electrodes"] = json!([
        {"tag": "X", "kind": "ground_rod", "level": "NOPE", "at": [1e308, -1e308], "count": 4_000_000_000u64, "rod_length_ft": 1e300},
        {"tag": "Y", "kind": "concrete_encased", "level": "L1", "length_ft": -5, "conductor": "huge", "entry": "NOWHERE"},
        {"tag": "Z", "kind": "ground_ring", "level": "L1", "ring_length_ft": 1e12}
    ]);
    sp["mep"]["grounding"]["bonds"] = json!([{"tag": "B", "from": "GHOST", "to": "", "level": "L9"}]);
    s.execute("bspec.set", &json!({"replace": true, "spec": sp})).unwrap();
    for c in ["grounding.entries", "grounding.route", "grounding.check", "grounding.takeoff", "mep.qty", "model.build"] {
        let _ = s.execute(c, &json!({}));
    }
    for p in [json!({}), json!({"electrode": "NOPE"}), json!({"electrode": "Y", "kind": "volcano"})] {
        assert!(s.execute("grounding.alternative", &p).is_err(), "{p}");
    }
    let c = s.execute("grounding.check", &json!({})).unwrap();
    assert!(errors(&c).iter().any(|(k, a)| k == "unknown" && a == "Y"), "{c}");
}
