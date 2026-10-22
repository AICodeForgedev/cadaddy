//! Q332: the plan from the bill of materials (conduit by floor, panels), marks by element, and bad
//! input refused. (The hand-calculated earned value and rates: mepfix tests/progress_hotel.rs.)

use cadaddy_engine::Session;
use serde_json::json;

#[test]
fn the_plan_comes_from_the_bom_and_elements_mark_progress() {
    let mut s = Session::new();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}, {"name": "L2", "elevation_ft": 12, "height_ft": 12}],
            "mep": {
                "systems": [{"name": "EMT-3/4", "layer": "E-CONDUIT", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 10}],
                "runs": [
                    {"system": "EMT-3/4", "level": "L1", "points": [[0, 0], [1200, 0]], "size_in": [0.75, 0], "bottoms_ft": [10]},
                    {"system": "EMT-3/4", "level": "L2", "points": [[0, 0], [600, 0]], "size_in": [0.75, 0], "bottoms_ft": [10]}
                ],
                "equipment": [
                    {"tag": "P1", "kind": "panel", "level": "L1", "min": [0, 0], "max": [18, 6], "bottom_ft": 1.5, "height_ft": 5},
                    {"tag": "P2", "kind": "panel", "level": "L2", "min": [0, 0], "max": [18, 6], "bottom_ft": 1.5, "height_ft": 5}
                ]
            }
        }}),
    )
    .unwrap();
    let p = s.execute("progress.plan", &json!({"from_bom": true})).unwrap();
    let qty =
        |lv: &str, d: &str| p["plan"].as_array().unwrap().iter().find(|x| x["level"] == lv && x["driver"] == d).map(|x| x["qty"].as_f64().unwrap());
    assert_eq!(qty("L1", "panels"), Some(1.0), "{p}");
    assert_eq!(qty("L2", "panels"), Some(1.0));
    assert!(qty("L1", "conduit_ft").is_some_and(|v| (v - 100.0).abs() < 1.0), "{p}");
    assert!(qty("L2", "conduit_ft").is_some_and(|v| (v - 50.0).abs() < 1.0), "{p}");
    // Elements mark progress by count; the book rate is the company's when given.
    s.execute("progress.plan", &json!({"rates": {"panels": 10}})).unwrap();
    s.execute("progress.mark", &json!({"phase": "panel", "level": "L1", "driver": "panels", "elements": ["P1"], "date": "2026-11-01"})).unwrap();
    s.execute("progress.hours", &json!({"rows": [{"date": "2026-11-01", "name": "Crew 1", "hours": "12", "phase": "panel", "level": "L1"}]}))
        .unwrap();
    let st = s.execute("progress.status", &json!({"date": "2026-11-02"})).unwrap();
    let panel = st["phases"].as_array().unwrap().iter().find(|x| x["phase"] == "panel").unwrap();
    assert_eq!(
        (panel["budget_hours"].as_f64(), panel["earned_hours"].as_f64(), panel["actual_hours"].as_f64()),
        (Some(20.0), Some(10.0), Some(12.0)),
        "{st}"
    );
    assert_eq!(panel["percent_complete"], 50.0);
    assert_eq!(panel["earned_value"], serde_json::Value::Null, "no crew rate: hours only");
    let r = s.execute("progress.rates", &json!({"date": "2026-11-02"})).unwrap();
    assert_eq!(r["rates"][0]["actual_rate"], 12.0);
    assert_eq!(r["rates"][0]["ratio"], 1.2);
}

#[test]
fn bad_input_is_refused() {
    let mut s = Session::new();
    assert!(s.execute("progress.status", &json!({})).is_err(), "no spec");
    s.execute("bspec.set", &json!({"replace": true, "spec": {"units_per_foot": 12, "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}]}}))
        .unwrap();
    assert!(s.execute("progress.status", &json!({})).is_err(), "no plan");
    for (c, p) in [
        ("progress.plan", json!({"drivers": [{"level": "L1", "driver": "widgets", "qty": 1}]})),
        ("progress.plan", json!({"drivers": [{"level": "L1", "driver": "boxes", "qty": -1}]})),
        ("progress.plan", json!({"rates": {"boxes": 0}})),
        ("progress.mark", json!({"phase": "lunch"})),
        ("progress.mark", json!({"phase": "rough", "fraction": 1.5})),
        ("progress.mark", json!({"phase": "rough", "count": 3})),
        ("progress.mark", json!({"phase": "rough", "count": 3, "driver": "devices"})),
        ("progress.mark", json!({"phase": "rough", "date": "someday"})),
        ("progress.hours", json!({})),
        ("progress.observe", json!({"path": "obs.txt"})),
        ("progress.observe", json!({"path": "obs.csv"})),
    ] {
        assert!(s.execute(c, &p).is_err(), "{c} {p}");
    }
    let r = s.execute("progress.hours", &json!({"rows": [{"date": "x", "hours": "1e400", "phase": "rough"}, {"hours": "3"}]})).unwrap();
    assert_eq!((r["added"].as_u64(), r["skipped"].as_array().unwrap().len()), (Some(0), 2));
}
