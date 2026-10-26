//! Q5121 `finishes.takeoff`: the finish takeoff from a print set's DXF sheets, through the
//! command. Seed 3's sheets are written to a folder, read by the command (never the answer key),
//! and its lines per trade match the answer key's takeoff; the answer is paged with per-trade
//! totals first and every line's source and status; unit types come from the building spec; a
//! set with no architectural sheets gets the guiding message, not an error.

use cadaddy_engine::Session;
use cadaddy_mepfix::finishes::compute_takeoff;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Seed 3's DXF sheets (only those `keep` accepts) in a fresh folder.
fn sheets(name: &str, keep: fn(&str) -> bool) -> (cadaddy_mepfix::truth::Truth, PathBuf) {
    let set = cadaddy_mepfix::generate(3).expect("seed 3");
    let dir = std::env::temp_dir().join(format!("cadaddy-finishes-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    for (n, b) in set.files.iter().filter(|(n, _)| n.ends_with(".dxf") && keep(n)) {
        std::fs::write(dir.join(n), b).expect("write");
    }
    (set.truth, dir)
}

fn lines_per_trade(r: &Value) -> BTreeMap<String, u64> {
    r["totals"]
        .as_array()
        .expect("totals")
        .iter()
        .map(|t| (t["trade"].as_str().unwrap_or_default().to_string(), t["lines"].as_u64().unwrap_or(0)))
        .collect()
}

#[test]
fn takeoff_from_the_sheets_matches_the_answer_key_per_trade() {
    let (truth, dir) = sheets("all", |_| true);
    let mut s = Session::new();
    let r = s.execute("finishes.takeoff", &json!({"dir": dir.display().to_string()})).expect("takeoff");
    assert!(r.get("message").is_none(), "{r}");
    // Per-trade line counts equal the answer key's takeoff.
    let key = compute_takeoff(&truth);
    let want: BTreeMap<String, u64> = key.trades.iter().map(|t| (t.trade.clone(), t.line_count as u64)).collect();
    assert_eq!(lines_per_trade(&r), want, "{}", r["totals"]);
    assert_eq!(r["count"].as_u64(), Some(key.items.len() as u64));
    // Totals come first, in trade order, with quantities by unit.
    let trades: Vec<&str> = r["totals"].as_array().unwrap().iter().map(|t| t["trade"].as_str().unwrap()).collect();
    assert_eq!(trades, ["flooring", "painting", "ceilings", "doors_frames_hardware", "specialties", "glazing"]);
    assert_eq!(r["totals"][0]["quantity"]["SF"], json!(25200.0));
    // A page of 50 lines, each with its source and status.
    let lines = r["lines"].as_array().expect("lines");
    assert_eq!(lines.len(), 50);
    assert_eq!(r["page"], 1);
    assert_eq!(r["next_page"], 2);
    for l in lines {
        assert!(l["source"].as_str().is_some_and(|s| !s.is_empty()), "{l}");
        assert!(["drawing", "schedule", "rule", "guess"].contains(&l["status"].as_str().unwrap_or("")), "{l}");
        for k in ["trade", "description", "room", "qty", "unit", "with_waste"] {
            assert!(!l[k].is_null(), "{k} in {l}");
        }
    }
    assert!(
        r["missing"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m.as_str().unwrap_or("").contains("legend sample") || m.as_str().unwrap_or("").contains("unmarked door symbol")),
        "{}",
        r["missing"]
    );

    // Filters: one room, one trade.
    let r = s
        .execute("finishes.takeoff", &json!({"dir": dir.display().to_string(), "level": "1", "room": "105", "trade": "flooring"}))
        .expect("filtered");
    let got: Vec<(String, f64)> =
        r["lines"].as_array().unwrap().iter().map(|l| (l["description"].as_str().unwrap().to_string(), l["qty"].as_f64().unwrap())).collect();
    assert_eq!(
        got,
        [
            ("CPT-1: CARPET TILE".to_string(), 780.0),
            ("RB-1: RUBBER BASE, 4\"".to_string(), 109.0),
            ("FLOOR TRANSITION, CARPET EDGE REDUCER: CPT-1 TO LVT-1".to_string(), 1.0)
        ]
    );
    assert_eq!(r["lines"][0]["source"], "A-101 plan; A-603 ROOM FINISHES row 105");
    // The last page, and past it.
    let r = s.execute("finishes.takeoff", &json!({"dir": dir.display().to_string(), "trade": "glazing", "page": 2})).expect("past the end");
    assert!(r["message"].as_str().is_some_and(|m| m.contains("past the last page")), "{r}");
}

#[test]
fn unit_types_from_the_building_spec_group_the_takeoff() {
    let (_, dir) = sheets("units", |_| true);
    let mut s = Session::new();
    // Two rooms of one (made-up) unit type, as unittypes.read would store them.
    let spec = json!({"unit_instances": [
        {"id": "u-1-105", "type": "OFFICE-A", "level": "1", "number": "105"},
        {"id": "u-2-205", "type": "OFFICE-A", "level": "2", "number": "205"},
    ]});
    s.doc_mut().unwrap().building_spec = Some(spec.to_string());
    let r = s.execute("finishes.takeoff", &json!({"dir": dir.display().to_string(), "unit_type": "OFFICE-A", "trade": "flooring"})).expect("takeoff");
    let rooms: std::collections::BTreeSet<&str> = r["lines"].as_array().unwrap().iter().map(|l| l["room"].as_str().unwrap()).collect();
    assert_eq!(rooms, ["105", "205"].into_iter().collect());
    let lines = r["by_unit_type"]["OFFICE-A"].as_array().expect("roll-up");
    let floor = lines.iter().find(|l| l["trade"] == "flooring" && l["unit"] == "SF").expect("floor");
    assert_eq!(floor["rooms"], 2);
    assert_eq!(floor["quantity"], json!(2.0 * 780.0));
}

#[test]
fn no_architectural_sheets_gives_the_guiding_message() {
    // Only the mechanical and electrical sheets: no A plan, no finish schedule.
    let (_, dir) = sheets("no-a", |n| n.starts_with("M-") || n.starts_with("E-"));
    let mut s = Session::new();
    let r = s.execute("finishes.takeoff", &json!({"dir": dir.display().to_string()})).expect("no error");
    assert!(r["message"].as_str().is_some_and(|m| m.contains("run arch.read first")), "{r}");
    assert_eq!(r["count"], 0);
    assert!(r["sheets_read"].as_u64().unwrap_or(0) > 0, "{r}");
    // No sheets and no print set at all.
    let r = s.execute("finishes.takeoff", &json!({})).expect("no error");
    assert!(r["message"].as_str().is_some_and(|m| m.contains("run arch.read first")), "{r}");
}
