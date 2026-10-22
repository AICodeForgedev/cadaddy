//! Q329 on the synthetic hotel: its cold water piping gives the water point of entry, its IDF is
//! a data room. (The fixture hotel has no gas service, second data room or switchboard yet: this
//! test adds a gas run, a second data room and the service gear to the spec it builds.) Every
//! electrode and bond is routed and counted; a missing data-room bond and a water clamp above a
//! gypsum ceiling are caught.

use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

fn errors(r: &Value) -> Vec<(String, String)> {
    r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "error")
        .map(|f| (f["check"].as_str().unwrap().to_string(), f["about"].as_str().unwrap().to_string()))
        .collect()
}

#[test]
fn the_hotel_is_grounded_and_bonded_and_what_is_missing_is_caught() {
    let t = cadaddy_mepfix::hotel_truth_for(2, HotelSize::Full);
    // The spec in feet (one drawing unit a foot): levels, rooms, the hotel's cold water runs.
    let levels: Vec<Value> =
        t.building.levels.iter().map(|l| json!({"name": l.code, "elevation_ft": l.elevation_ft, "height_ft": 10, "ceiling_ft": 9})).collect();
    let mut rooms: Vec<Value> = t.building.rooms.iter().map(|r| json!({"name": r.name, "number": r.number, "level": r.level})).collect();
    let cw: Vec<&cadaddy_mepfix::truth::Segment> = t.segments.iter().filter(|s| s.system == "CW" && !s.vertical).collect();
    assert!(!cw.is_empty());
    let mut runs: Vec<Value> = cw
        .iter()
        .map(|s| {
            let b = s.height.as_ref().map_or(9.5, |h| h.bottom_ft);
            json!({"system": "CW", "level": s.level, "points": [[s.path_ft[0][0], s.path_ft[0][1]], [s.path_ft[1][0], s.path_ft[1][1]]], "size_in": [s.size_in[0], 0], "bottoms_ft": [b]})
        })
        .collect();
    let idf = t.building.rooms.iter().find(|r| r.name.to_uppercase().contains("IDF")).expect("the hotel's IDF");
    let l1 = idf.level.clone();
    let top = t.building.levels.iter().max_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)).unwrap().code.clone();
    // Added: the gas service, a second data room on the top floor, the switchboard.
    runs.push(json!({"system": "GAS", "level": l1, "points": [[-2.0, 4.0], [12.0, 4.0]], "size_in": [1.5, 0], "bottoms_ft": [8]}));
    rooms.push(json!({"name": "BOH DATA 2", "number": "D2", "level": top}));
    let idf_at = [(idf.min_ft[0] + idf.max_ft[0]) / 2.0, (idf.min_ft[1] + idf.max_ft[1]) / 2.0];
    let spec = |extra_bond: bool, gypsum_over_water: Option<[[f64; 2]; 4]>| -> Value {
        let mut bonds = vec![
            json!({"tag": "GAS-1", "kind": "gas", "to": "POE-G", "size": "#6"}),
            json!({"tag": "TBB-1", "kind": "telecom", "to": "TMGB", "size": "3/0"}),
        ];
        if extra_bond {
            bonds.push(json!({"tag": "TBB-2", "kind": "telecom", "from": "TMGB", "to": "TGB-2", "size": "#6"}));
        }
        let ceilings: Vec<Value> =
            gypsum_over_water.map(|o| vec![json!({"level": l1, "room": "BACK OF HOUSE", "outline": o, "height_ft": 8})]).unwrap_or_default();
        json!({
            "version": 1, "units_per_foot": 1, "levels": levels, "rooms": rooms, "rcp": {"ceilings": ceilings},
            "mep": {
                "systems": [
                    {"name": "CW", "layer": "P-CW", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 9.5},
                    {"name": "GAS", "layer": "P-GAS", "kind": "pipe", "size_in": [1.5, 0], "bottom_ft": 8}
                ],
                "runs": runs,
                "equipment": [{"tag": "MDP", "kind": "switchboard", "level": l1, "min": [idf_at[0] + 8.0, idf_at[1]], "max": [idf_at[0] + 14.0, idf_at[1] + 3.0], "bottom_ft": 0, "height_ft": 7.5}],
                "grounding": {
                    "electrodes": [
                        {"tag": "W-1", "kind": "water_pipe", "level": l1, "entry": "POE-W", "gec_size": "3/0"},
                        {"tag": "UF-1", "kind": "concrete_encased", "level": l1, "at": [idf_at[0] + 10.0, -1.0], "z_ft": -2, "gec_size": "#4", "length_ft": 20, "cover_in": 3}
                    ],
                    "bonds": bonds,
                    "busbars": [
                        {"tag": "TMGB", "kind": "tmgb", "room": idf.name, "level": l1, "at": idf_at, "size": "1/4\" x 4\" x 12\""},
                        {"tag": "TGB-2", "kind": "tgb", "room": "BOH DATA 2", "level": top, "at": idf_at, "size": "1/4\" x 2\" x 12\""}
                    ],
                    "required": [
                        {"kind": "water_pipe", "note": "BOND COLD WATER AT ENTRY"},
                        {"kind": "gas", "to": "POE-G", "note": "BOND GAS"},
                        {"kind": "concrete_encased", "note": "UFER"},
                        {"kind": "telecom", "to": "TMGB", "note": "TBB TO TMGB"}
                    ]
                }
            }
        })
    };
    // Planted: no bond to the second data room's busbar.
    let mut s = cadaddy_engine::Session::new();
    s.execute("bspec.set", &json!({"replace": true, "spec": spec(false, None)})).unwrap();
    let e = s.execute("grounding.entries", &json!({})).unwrap();
    let w = e["entries"].as_array().unwrap().iter().find(|x| x["kind"] == "water").unwrap().clone();
    assert_eq!(w["accessible"], true, "{e}");
    let r = s.execute("grounding.route", &json!({})).unwrap();
    assert_eq!(r["routed"].as_array().unwrap().len(), 4, "{r}");
    let c = s.execute("grounding.check", &json!({})).unwrap();
    assert_eq!(errors(&c), [("data room".to_string(), "BOH DATA 2".to_string())], "{c}");
    // Planted: a gypsum ceiling under the water entry (the pipe above it).
    let at = [w["at"][0].as_f64().unwrap(), w["at"][1].as_f64().unwrap()];
    let around = [[at[0] - 5.0, at[1] - 5.0], [at[0] + 5.0, at[1] - 5.0], [at[0] + 5.0, at[1] + 5.0], [at[0] - 5.0, at[1] + 5.0]];
    s.execute("bspec.set", &json!({"replace": true, "spec": spec(true, Some(around))})).unwrap();
    s.execute("grounding.route", &json!({})).unwrap();
    let c = s.execute("grounding.check", &json!({})).unwrap();
    assert_eq!(errors(&c), [("inaccessible clamp".to_string(), "W-1".to_string())], "{c}");
    // Both fixed: clean, and everything counted.
    s.execute("bspec.set", &json!({"replace": true, "spec": spec(true, None)})).unwrap();
    s.execute("grounding.route", &json!({})).unwrap();
    assert!(errors(&s.execute("grounding.check", &json!({})).unwrap()).is_empty());
    let t = s.execute("grounding.takeoff", &json!({})).unwrap();
    let names: Vec<&str> = t["items"].as_array().unwrap().iter().map(|i| i["item"].as_str().unwrap()).collect();
    for item in [
        "Grounding conductor, 3/0 copper",
        "Grounding conductor, #6 copper",
        "Grounding conductor, #4 bare copper",
        "Water pipe ground clamp",
        "Gas pipe bonding clamp",
        "Two-hole compression lug on busbar",
    ] {
        assert!(names.contains(&item), "{item}: {names:?}");
    }
}
