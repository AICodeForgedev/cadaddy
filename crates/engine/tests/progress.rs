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

/// A one-floor job: four receptacles (a box each), a 100 ft conduit run and a panel on L1.
fn job(s: &mut Session) {
    let tag = |id: &str, x: f64| json!({"id": id, "kind": "receptacle", "level": "L1", "at_ft": [x, 1], "fields": {"device_type": {"value": "duplex receptacle", "source": "plan"}}});
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "units_per_foot": 12,
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}, {"name": "L2", "elevation_ft": 12, "height_ft": 12}],
            "mep": {
                "systems": [{"name": "EMT-3/4", "layer": "E-CONDUIT", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 10}],
                "runs": [{"system": "EMT-3/4", "level": "L1", "points": [[0, 0], [1200, 0]], "size_in": [0.75, 0], "bottoms_ft": [10]}],
                "equipment": [{"tag": "P1", "kind": "panel", "level": "L1", "min": [0, 0], "max": [18, 6], "bottom_ft": 1.5, "height_ft": 5}]
            },
            "tags": [tag("R1", 3.0), tag("R2", 4.0), tag("R3", 5.0), tag("R4", 6.0)]
        }}),
    )
    .unwrap();
}

fn phase(st: &serde_json::Value, p: &str) -> serde_json::Value {
    st["phases"].as_array().unwrap().iter().find(|x| x["phase"] == p).cloned().unwrap_or_else(|| panic!("no {p} in {st}"))
}

fn installed(st: &serde_json::Value, p: &str, lv: &str, d: &str) -> f64 {
    let ph = phase(st, p);
    let fl = ph["floors"].as_array().unwrap().iter().find(|f| f["level"] == lv).cloned().unwrap();
    fl["drivers"].as_array().unwrap().iter().find(|x| x["driver"] == d).unwrap()["installed"].as_f64().unwrap()
}

#[test]
fn the_budget_comes_from_the_estimate_labor_hours() {
    let mut s = Session::new();
    job(&mut s);
    s.execute("progress.plan", &json!({"from_bom": true})).unwrap();
    // Without an estimate: the book rates, said to be an assumption (4 × 0.25 + 100 × 0.04 = 5 h).
    let st = s.execute("progress.status", &json!({"date": "2026-11-01"})).unwrap();
    assert_eq!(
        (phase(&st, "rough")["budget_source"].as_str(), phase(&st, "rough")["budget_hours"].as_f64()),
        (Some("book_rate_assumption"), Some(5.0)),
        "{st}"
    );
    // The estimate prices the box at 1.0 h (not the book's 0.25) and conduit at 0.1 h/ft.
    for (item, unit, h) in [("Box, 4\" square", "each", 1.0), ("EMT conduit", "ft", 0.1)] {
        s.execute(
            "estimate.set_price",
            &json!({"item": item, "unit": unit, "material": 2.0, "labor_hours": h, "date": "2026-10-01", "source": "Invented Supply"}),
        )
        .unwrap();
    }
    s.execute("estimate.set_scope", &json!({"trade": "electrical", "scope": "in_contract", "reason": "Division 26 is ours"})).unwrap();
    // Rough half done with 6 h spent.
    s.execute("progress.mark", &json!({"phase": "rough", "level": "L1", "fraction": 0.5, "date": "2026-11-02"})).unwrap();
    s.execute("progress.hours", &json!({"rows": [{"date": "2026-11-02", "name": "Crew 1", "hours": "6", "phase": "rough", "level": "L1"}]})).unwrap();
    // Hand calculation from the estimate's hours: boxes 4 × 1.0 = 4 h, conduit 100 × 0.1 = 10 h;
    // budget 14 h, earned 7 h at half, projected 6 / 0.5 = 12 h ≤ 14 × 1.1 = 15.4: not over. (On
    // the book rates it would be: 12 h against 5.5.)
    let st = s.execute("progress.status", &json!({"date": "2026-11-03"})).unwrap();
    let rough = phase(&st, "rough");
    assert_eq!(rough["budget_source"], "estimate", "{rough}");
    assert_eq!(
        (rough["budget_hours"].as_f64(), rough["earned_hours"].as_f64(), rough["percent_complete"].as_f64(), rough["projected_hours"].as_f64()),
        (Some(14.0), Some(7.0), Some(50.0), Some(12.0)),
        "{rough}"
    );
    let drivers = rough["floors"][0]["drivers"].as_array().unwrap();
    let d = |n: &str| drivers.iter().find(|x| x["driver"] == n).cloned().unwrap();
    assert_eq!((d("boxes")["budget_hours"].as_f64(), d("boxes")["budget_source"].as_str()), (Some(4.0), Some("estimate")));
    assert_eq!((d("conduit_ft")["budget_hours"].as_f64(), d("conduit_ft")["earned_hours"].as_f64()), (Some(10.0), Some(5.0)));
    assert!(st["flags"].as_array().unwrap().is_empty(), "{st}");
    // Terminations have no estimate line: the book rate, and makeup says so.
    assert_eq!(phase(&st, "makeup")["budget_source"], "book_rate_assumption");
    // The estimate's own labor on lines of no driver (the device, fittings) is shown apart.
    assert!(st["estimate_hours_outside_drivers"].is_number(), "{st}");
    // 3 h more: projected 9 / 0.5 = 18 h > 15.4: rough is running over its estimate.
    s.execute("progress.hours", &json!({"rows": [{"date": "2026-11-03", "name": "Crew 1", "hours": "3", "phase": "rough", "level": "L1"}]})).unwrap();
    let st = s.execute("progress.status", &json!({"date": "2026-11-03"})).unwrap();
    let flags = st["flags"].as_array().unwrap();
    assert_eq!(flags.len(), 1, "{st}");
    assert_eq!(flags[0], "rough is running over: 18 h projected against 14 h budgeted at 50% complete (9 h spent for 7 h earned)");
}

#[test]
fn rates_are_observed_from_coded_hours_and_apportioned_from_shared_ones() {
    let plan = json!({"drivers": [
        {"level": "L1", "driver": "boxes", "qty": 40}, {"level": "L1", "driver": "conduit_ft", "qty": 500},
        {"level": "L1", "driver": "terminations", "qty": 120}
    ]});
    let spec = json!({"replace": true, "spec": {"units_per_foot": 12, "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}]}});
    let mark = |s: &mut Session| {
        s.execute("progress.mark", &json!({"phase": "rough", "level": "L1", "date": "2026-11-02"})).unwrap();
        s.execute("progress.mark", &json!({"phase": "makeup", "level": "L1", "date": "2026-11-02"})).unwrap();
    };
    // Hours coded to each driver: a driver column, and a cost code naming the driver.
    let mut s = Session::new();
    s.execute("bspec.set", &spec).unwrap();
    s.execute("progress.plan", &plan).unwrap();
    mark(&mut s);
    let r = s
        .execute(
            "progress.hours",
            &json!({"rows": [
                {"date": "2026-11-01", "name": "Crew 1", "hours": "15", "phase": "rough", "driver": "boxes", "level": "L1"},
                {"date": "2026-11-01", "name": "Crew 2", "hours": "25", "cost code": "EL-ROUGH-CONDUIT", "level": "L1"},
                {"date": "2026-11-01", "name": "Crew 2", "hours": "1", "phase": "rough", "driver": "devices", "level": "L1"}
            ]}),
        )
        .unwrap();
    assert_eq!((r["added"].as_u64(), r["skipped"].as_array().unwrap().len()), (Some(2), 1), "a trim driver on rough hours is refused: {r}");
    let rates = s.execute("progress.rates", &json!({"date": "2026-11-30"})).unwrap();
    let rr = |d: &str| rates["rates"].as_array().unwrap().iter().find(|x| x["driver"] == d).cloned().unwrap_or_else(|| panic!("{d}: {rates}"));
    // Boxes 15 h / 40 = 0.375 h (1.5 × the book's 0.25); conduit 25 h / 500 ft = 0.05 (1.25 × 0.04).
    assert_eq!(
        (rr("boxes")["actual_rate"].as_f64(), rr("boxes")["ratio"].as_f64(), rr("boxes")["basis"].as_str()),
        (Some(0.375), Some(1.5), Some("observed"))
    );
    assert_eq!(
        (rr("conduit_ft")["actual_rate"].as_f64(), rr("conduit_ft")["ratio"].as_f64(), rr("conduit_ft")["basis"].as_str()),
        (Some(0.05), Some(1.25), Some("observed"))
    );
    // Phase-only hours: shared by earned hours (boxes 10, conduit 20 of 30), the same ratio for
    // both, and said to be apportioned. Makeup has one driver: its hours are its own.
    let mut s = Session::new();
    s.execute("bspec.set", &spec).unwrap();
    s.execute("progress.plan", &plan).unwrap();
    mark(&mut s);
    s.execute(
        "progress.hours",
        &json!({"rows": [
            {"date": "2026-11-01", "name": "Crew 1", "hours": "40", "phase": "rough", "level": "L1"},
            {"date": "2026-11-01", "name": "Crew 1", "hours": "12", "phase": "makeup", "level": "L1"}
        ]}),
    )
    .unwrap();
    let rates = s.execute("progress.rates", &json!({"date": "2026-11-30"})).unwrap();
    let rr = |d: &str| rates["rates"].as_array().unwrap().iter().find(|x| x["driver"] == d).cloned().unwrap();
    assert_eq!(
        (rr("boxes")["hours"].as_f64(), rr("boxes")["ratio"].as_f64(), rr("boxes")["basis"].as_str()),
        (Some(13.33), Some(1.33), Some("apportioned"))
    );
    assert_eq!(
        (rr("conduit_ft")["hours"].as_f64(), rr("conduit_ft")["ratio"].as_f64(), rr("conduit_ft")["basis"].as_str()),
        (Some(26.67), Some(1.33), Some("apportioned"))
    );
    assert_eq!((rr("terminations")["actual_rate"].as_f64(), rr("terminations")["basis"].as_str()), (Some(0.1), Some("observed")));
    // The private observations carry the basis.
    let dir = std::env::temp_dir().join(format!("cadaddy-progress-basis-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let obs = dir.join("field_observations.csv");
    let _ = std::fs::remove_file(&obs);
    s.execute("progress.observe", &json!({"path": obs.display().to_string(), "date": "2026-11-30"})).unwrap();
    let text = std::fs::read_to_string(&obs).unwrap();
    let basis: Vec<(&str, &str)> = text
        .lines()
        .skip(1)
        .map(|l| {
            let c: Vec<&str> = l.split(',').collect();
            (c[3], c[12])
        })
        .collect();
    assert_eq!(basis, [("boxes", "apportioned"), ("conduit_ft", "apportioned"), ("terminations", "observed")], "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn marks_never_do_nothing_or_count_twice() {
    let mut s = Session::new();
    job(&mut s);
    s.execute("progress.plan", &json!({"from_bom": true})).unwrap();
    // A count with no floor is refused, with what to do.
    let e = s.execute("progress.mark", &json!({"phase": "trim", "count": 20, "driver": "devices"})).unwrap_err().to_string();
    assert!(e.contains("needs the floor it was installed on (level)"), "{e}");
    // Unknown elements are named.
    let e = s.execute("progress.mark", &json!({"phase": "rough", "elements": ["R1", "R9"]})).unwrap_err().to_string();
    assert!(e.contains("no element R9 in the model"), "{e}");
    // An element on another floor, or of another driver, is refused.
    let e = s.execute("progress.mark", &json!({"phase": "rough", "level": "L2", "elements": ["R1"]})).unwrap_err().to_string();
    assert!(e.contains("element R1 is on level L1, not L2"), "{e}");
    let e = s.execute("progress.mark", &json!({"phase": "rough", "driver": "conduit_ft", "elements": ["R1"]})).unwrap_err().to_string();
    assert!(e.contains("element R1 isn't conduit_ft"), "{e}");
    let e = s.execute("progress.mark", &json!({"phase": "panel", "elements": ["R1"]})).unwrap_err().to_string();
    assert!(e.contains("element R1 has no panel work"), "{e}");
    let st = s.execute("progress.status", &json!({"date": "2026-11-30"}));
    assert!(st.is_err() || st.unwrap()["phases"].as_array().unwrap().iter().all(|p| p["earned_hours"] == 0.0), "refused marks add nothing");
    // The same element twice (in one mark and again later) counts once; driver and floor come
    // from the model: R1 is a box in rough, three terminations in makeup; the run is its feet.
    let r = s.execute("progress.mark", &json!({"phase": "rough", "elements": ["R1", "R1"], "date": "2026-11-01"})).unwrap();
    assert_eq!(r["new_elements"], 1);
    let r = s.execute("progress.mark", &json!({"phase": "rough", "elements": ["R1", "run:0"], "date": "2026-11-02"})).unwrap();
    assert_eq!(r["new_elements"], 1, "R1 is already in");
    s.execute("progress.mark", &json!({"phase": "makeup", "elements": ["R1"], "date": "2026-11-02"})).unwrap();
    s.execute("progress.mark", &json!({"phase": "panel", "elements": ["P1"], "date": "2026-11-02"})).unwrap();
    let st = s.execute("progress.status", &json!({"date": "2026-11-30"})).unwrap();
    assert_eq!(installed(&st, "rough", "L1", "boxes"), 1.0, "{st}");
    assert_eq!(installed(&st, "rough", "L1", "conduit_ft"), 100.0);
    assert_eq!(installed(&st, "makeup", "L1", "terminations"), 3.0);
    assert_eq!(installed(&st, "panel", "L1", "panels"), 1.0);
    // A device in trim: R2 is a device (the trim phase's devices, not luminaires).
    s.execute("progress.mark", &json!({"phase": "trim", "elements": ["R2"], "date": "2026-11-02"})).unwrap();
    s.execute("progress.mark", &json!({"phase": "trim", "level": "L1", "driver": "devices", "count": 1, "date": "2026-11-02"})).unwrap();
    let st = s.execute("progress.status", &json!({"date": "2026-11-30"})).unwrap();
    assert_eq!(installed(&st, "trim", "L1", "devices"), 2.0, "an element and a count of one more");
}

#[test]
fn a_damaged_progress_record_is_an_error_and_left_alone() {
    let mut s = Session::new();
    let bad = json!({"plan": [{"level": "L1", "driver": "boxes", "qty": 10}], "marks": "not a list", "hours": [{"date": "2026-11-01", "hours": 8, "phase": "rough"}]});
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {"units_per_foot": 12, "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}], "progress": bad}}),
    )
    .unwrap();
    let stored = |s: &mut Session| s.execute("bspec.get", &json!({})).unwrap()["spec"]["progress"].clone();
    assert_eq!(stored(&mut s), bad, "bspec keeps the record as given");
    for (c, p) in [
        ("progress.mark", json!({"phase": "rough", "level": "L1"})),
        ("progress.hours", json!({"rows": [{"date": "2026-11-02", "hours": "4", "phase": "rough", "level": "L1"}]})),
        ("progress.status", json!({})),
        ("progress.plan", json!({"drivers": [{"level": "L1", "driver": "boxes", "qty": 12}]})),
        ("progress.rates", json!({})),
    ] {
        let e = s.execute(c, &p).unwrap_err().to_string();
        assert!(e.contains("the stored progress record can't be read"), "{c}: {e}");
        assert_eq!(stored(&mut s), bad, "{c} left the record as it was");
    }
}
