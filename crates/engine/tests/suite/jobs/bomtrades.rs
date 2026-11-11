//! Q6528: each takeoff line takes the trade of its element's kind and system, the trade the
//! model tree shows. Pull boxes, wireways, poles, EV chargers and BDA boxes are electrical;
//! sprinkler pipe, fittings and heads fire protection; a conduit support is counted once, by
//! the electrical trade, however its run was made (drawn or routed by rule); a bid of other
//! trades files their lines under their own trades, never an electrical system.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

/// A made-up one-storey building: panel LP1 feeding a pull box on a feeder raceway, a light pole,
/// an EV charger and a BDA riser box; an estimated (rule-made) branch conduit beside a drawn one;
/// a sprinkler line with an elbow and two heads; a supply duct with a diffuser on an air handler;
/// a water heater on the cold water line.
fn building() -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "mep": {
            "systems": [
                {"name": "E-COND", "layer": "E-COND", "kind": "conduit", "size_in": [1, 0], "bottom_ft": 10},
                {"name": "E-CIRCUIT-EST", "layer": "E-CIRCUIT-EST", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9},
                {"name": "E-FDR", "layer": "E-FDR", "kind": "conduit", "size_in": [2, 0], "bottom_ft": 10},
                {"name": "FP", "layer": "F-SPRK", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 10},
                {"name": "DCW", "layer": "P-DCW", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 10},
                {"name": "SA", "layer": "M-SUPP", "kind": "duct", "size_in": [12, 10], "bottom_ft": 10}
            ],
            "runs": [
                {"system": "E-COND", "level": "L1", "points": [[0, 600], [444, 600]], "size_in": [1, 0], "bottoms_ft": [10], "size_source": "tag", "height_source": "tag"},
                {"system": "E-CIRCUIT-EST", "level": "L1", "points": [[0, 120], [480, 120]], "size_in": [0.75, 0], "bottoms_ft": [9], "height_source": "estimated route"},
                {"system": "E-FDR", "level": "L1", "points": [[-30, 300], [240, 300]], "size_in": [2, 0], "bottoms_ft": [10], "size_source": "tag", "height_source": "tag"},
                {"system": "FP", "level": "L1", "points": [[0, 360], [360, 360], [360, 600]], "size_in": [2, 0], "bottoms_ft": [10, 10], "size_source": "tag", "height_source": "tag"},
                {"system": "DCW", "level": "L1", "points": [[0, 420], [300, 420]], "size_in": [1, 0], "bottoms_ft": [10], "size_source": "tag", "height_source": "tag"},
                {"system": "SA", "level": "L1", "points": [[0, 480], [360, 480]], "size_in": [12, 10], "bottoms_ft": [10], "size_source": "tag", "height_source": "tag"}
            ],
            "terminals": [
                {"system": "FP", "level": "L1", "at": [120, 360], "size_in": [0.5, 0], "bottom_ft": 9, "terminal_kind": "sprinkler_pendent"},
                {"system": "FP", "level": "L1", "at": [240, 360], "size_in": [0.5, 0], "bottom_ft": 9, "terminal_kind": "sprinkler_pendent"},
                {"system": "SA", "level": "L1", "at": [300, 480], "size_in": [12, 12], "bottom_ft": 9, "terminal_kind": "diffuser"}
            ],
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, 110], "max": [-4, 130], "height_ft": 4, "bottom_ft": 2},
                {"tag": "PB-1", "kind": "pull_box", "level": "L1", "min": [240, 290], "max": [252, 302], "height_ft": 1, "bottom_ft": 10, "systems": ["E-FDR"]},
                {"tag": "WW-1", "kind": "wireway", "level": "L1", "min": [-60, 100], "max": [-30, 106], "height_ft": 0.5, "bottom_ft": 7},
                {"tag": "SL-1", "kind": "pole", "level": "L1", "min": [900, 900], "max": [905, 905], "height_ft": 20, "bottom_ft": 0, "round": true},
                {"tag": "EV-1", "kind": "ev_charger", "level": "L1", "min": [800, 900], "max": [812, 908], "height_ft": 5, "bottom_ft": 0},
                {"tag": "BDA-1", "kind": "bda_riser_box", "level": "L1", "min": [100, 100], "max": [110, 104], "height_ft": 0.9, "bottom_ft": 8},
                {"tag": "AHU-1", "kind": "ahu", "level": "L1", "min": [-120, 460], "max": [-60, 500], "height_ft": 5, "systems": ["SA"]},
                {"tag": "WH-1", "kind": "water_heater", "level": "L1", "min": [-60, 400], "max": [-36, 424], "height_ft": 5, "systems": ["DCW"], "round": true}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": false})).expect("build");
    s
}

fn lines(s: &mut Session) -> Vec<Value> {
    let bom = s.execute("bim.bom", &json!({"group_by": "trade"})).expect("bom");
    bom["lines"].as_array().cloned().unwrap_or_default()
}

fn trades_of(ls: &[Value], pick: impl Fn(&Value) -> bool) -> Vec<(&str, &str)> {
    ls.iter().filter(|l| pick(l)).map(|l| (l["trade"].as_str().unwrap_or_default(), l["item"].as_str().unwrap_or_default())).collect()
}

#[test]
fn electrical_equipment_is_electrical_and_other_equipment_is_its_trades() {
    let mut s = building();
    let ls = lines(&mut s);
    let equipment = |tag: &str| trades_of(&ls, |l| l["category"] == "equipment" && l["item"].as_str().is_some_and(|i| i.ends_with(tag)));
    for tag in ["LP1", "PB-1", "WW-1", "SL-1", "EV-1", "BDA-1"] {
        let got = equipment(tag);
        assert!(!got.is_empty() && got.iter().all(|(t, _)| *t == "electrical"), "{tag}: {got:?}");
    }
    assert_eq!(equipment("AHU-1").first().map(|x| x.0), Some("mechanical"));
    assert_eq!(equipment("WH-1").first().map(|x| x.0), Some("plumbing"), "a water heater on the water line is the plumber's");
}

#[test]
fn a_conduit_support_is_counted_once_by_the_electrical_trade() {
    let mut s = building();
    let ls = lines(&mut s);
    let conduit_hangers =
        trades_of(&ls, |l| l["category"] == "hangers" && l["item"].as_str().is_some_and(|i| i.contains("conduit") || i.contains("cable")));
    assert!(conduit_hangers.is_empty(), "conduit hangers beside the electrical straps (counted twice): {conduit_hangers:?}");
    // Nor the routed (rule-made) conduit's fittings and pipe-style lines again.
    let raceway_twice = trades_of(&ls, |l| {
        l["trade"] != "electrical" && l["item"].as_str().is_some_and(|i| i.contains("E-CIRCUIT-EST") || i.contains("E-COND") || i.contains("E-FDR"))
    });
    assert!(raceway_twice.is_empty(), "{raceway_twice:?}");
    let straps = trades_of(&ls, |l| l["item"].as_str().is_some_and(|i| i.to_ascii_lowercase().contains("strap")));
    assert!(!straps.is_empty() && straps.iter().all(|(t, _)| *t == "electrical"), "the conduit's own supports: {straps:?}");
    // The other trades' supports stay theirs.
    let duct = trades_of(&ls, |l| l["category"] == "hangers" && l["item"].as_str().is_some_and(|i| i.contains("duct")));
    assert!(!duct.is_empty() && duct.iter().all(|(t, _)| *t == "mechanical"), "{duct:?}");
    let pipe = trades_of(&ls, |l| l["category"] == "hangers" && l["item"].as_str().is_some_and(|i| i.contains("pipe")));
    let mut pt: Vec<&str> = pipe.iter().map(|x| x.0).collect();
    pt.sort_unstable();
    pt.dedup();
    assert_eq!(pt, ["fire protection", "plumbing"], "{pipe:?}");
}

#[test]
fn sprinkler_pipe_fittings_and_heads_are_fire_protection() {
    let mut s = building();
    let ls = lines(&mut s);
    let fp = trades_of(&ls, |l| {
        l["item"].as_str().is_some_and(|i| i.starts_with("FP "))
            || l["category"] == "terminals" && l["item"].as_str().is_some_and(|i| i.contains("sprinkler"))
    });
    assert!(fp.iter().any(|(_, i)| i.contains("pipe")), "the sprinkler pipe: {fp:?}");
    assert!(fp.iter().any(|(_, i)| i.contains("elbow")), "its elbow: {fp:?}");
    assert!(fp.iter().any(|(_, i)| i.contains("sprinkler")), "its heads: {fp:?}");
    assert!(fp.iter().all(|(t, _)| *t == "fire protection"), "{fp:?}");
    let cw = trades_of(&ls, |l| l["item"].as_str().is_some_and(|i| i.starts_with("DCW ")));
    assert!(!cw.is_empty() && cw.iter().all(|(t, _)| *t == "plumbing"), "{cw:?}");
    let diffuser = trades_of(&ls, |l| l["category"] == "terminals" && l["item"] == "diffuser");
    assert_eq!(diffuser.first().map(|x| x.0), Some("mechanical"));
}

fn handle_of(s: &mut Session, tag: &str) -> String {
    let d = s.doc().expect("doc");
    let want = format!("tag:{tag}");
    d.model.iter().find(|e| e.common.xdata.iter().any(|t| t.value == want)).map(|e| e.handle.hex()).unwrap_or_default()
}

#[test]
fn the_model_tree_shows_the_same_trade() {
    let mut s = building();
    for (tag, group) in [("PB-1", "Electrical"), ("LP1", "Electrical"), ("EV-1", "Electrical"), ("AHU-1", "Mechanical"), ("WH-1", "Plumbing")] {
        let h = handle_of(&mut s, tag);
        assert!(!h.is_empty(), "{tag} built");
        let el = s.execute("bim.element", &json!({"handle": h})).expect("element");
        assert_eq!(el["group"], group, "{tag}: {el}");
    }
}

#[test]
fn a_bid_of_other_trades_files_their_lines_under_their_own_trades() {
    let mut s = building();
    let bid = s
        .execute("bim.bid", &json!({"trades": ["mechanical", "plumbing", "fire protection"], "keep": false, "alternates": false, "underground": false, "today": "2026-01-15"}))
        .expect("bid");
    let systems: Vec<&str> = bid["by_system"].as_array().into_iter().flatten().filter_map(|x| x["system"].as_str()).collect();
    assert!(!systems.is_empty(), "{bid}");
    for electrical in ["power", "gear", "lighting", "fire_alarm", "low_voltage", "site"] {
        assert!(!systems.contains(&electrical), "a {electrical} system in a bid of other trades: {systems:?}");
    }
    assert!(systems.contains(&"trade_mechanical") && systems.contains(&"trade_fire_protection"), "{systems:?}");
    // The electrical bid keeps its own systems.
    let ours = s.execute("bim.bid", &json!({"keep": false, "alternates": false, "underground": false, "today": "2026-01-15"})).expect("bid");
    let ours: Vec<&str> = ours["by_system"].as_array().into_iter().flatten().filter_map(|x| x["system"].as_str()).collect();
    assert!(ours.contains(&"gear") && !ours.iter().any(|x| x.starts_with("trade_")), "{ours:?}");
}
