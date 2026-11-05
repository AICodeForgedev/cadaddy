//! The load calculation from the model (Q6194, `bim.loadcalc`) on made-up buildings: an office
//! building's service and two panels (an overloaded lighting circuit, a receptacle circuit over
//! its breaker, a kitchen panel over its bus and its feeder, phases out of balance, a schedule
//! whose totals aren't its circuits', feeder rows swapped), and a lodging building whose typical
//! unit panels roll up through their feeders with the code sheet's occupancy.

use cadaddy_engine::Session;
use cadaddy_engine::doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, RawTag};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// A device as the build makes them: a small box with the build's tag and its property store.
fn dev(d: &mut Drawing, x: f64, ps: &[(&str, Value)]) -> Handle {
    let v = |x: f64, y: f64, z: f64| Vec3 { x: x * 12.0, y: y * 12.0, z: z * 12.0 };
    let (lo, hi) = ([x, 0.0, 1.0], [x + 0.3, 0.2, 1.3]);
    let vertices = vec![
        v(lo[0], lo[1], lo[2]),
        v(hi[0], lo[1], lo[2]),
        v(hi[0], hi[1], lo[2]),
        v(lo[0], hi[1], lo[2]),
        v(lo[0], lo[1], hi[2]),
        v(hi[0], lo[1], hi[2]),
        v(hi[0], hi[1], hi[2]),
        v(lo[0], hi[1], hi[2]),
    ];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])];
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Mesh(Mesh { vertices, faces, ..Default::default() }));
    e.common.layer = "E-POWER".into();
    let x = vec![
        RawTag { code: 1001, value: "CADADDY_WALLGEN".into() },
        RawTag { code: 1000, value: "device".into() },
        RawTag { code: 1000, value: "L1".into() },
        RawTag { code: 1000, value: "0".into() },
    ];
    let mut store = Props::default();
    for (k, val) in ps {
        let src = if *k == "fixture_va" { Source::from_doc(SourceKind::Schedule, "X-2 LIGHT FIXTURE SCHEDULE type L1") } else { Source::print("Sample Sheet E-101", None) };
        store.set(k, Prop::new(val.clone(), src));
    }
    e.common.xdata = props::write(&x, &store).into();
    d.ensure_layer("E-POWER");
    d.model.push(e);
    h
}

fn luminaires(d: &mut Drawing, panel: &str, circuit: &str, n: usize, va: f64, room: &str) {
    for i in 0..n {
        dev(d, i as f64, &[("kind", json!("luminaire")), ("panel", json!(panel)), ("circuit", json!(circuit)), ("fixture_va", json!(va)), ("room", json!(room))]);
    }
}

fn devices(d: &mut Drawing, kind: &str, panel: &str, circuit: &str, n: usize, room: &str) {
    for i in 0..n {
        dev(d, 50.0 + i as f64, &[("kind", json!(kind)), ("panel", json!(panel)), ("circuit", json!(circuit)), ("room", json!(room))]);
    }
}

fn ckt(n: &str, desc: &str, amps: &str, poles: u32, va: Option<f64>) -> Value {
    json!({"number": n, "description": desc, "breaker": amps, "poles": poles, "load_va": va, "source": "X-2"})
}

fn session(spec: Value, d: Drawing) -> Session {
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Building", None);
    s.execute("bspec.set", &json!({"spec": spec, "replace": true})).unwrap();
    s
}

fn office_spec() -> Value {
    let three = "208Y/120V, 3PH, 4W";
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}],
        "mep": {
            "feeders": [
                {"tag": "F1", "from": "MSB", "to": "LP1", "sets": 1, "phases": 3, "size": "#1", "material": "cu", "source": "FEEDER SCHEDULE X-1"},
                {"tag": "F2", "from": "MSB", "to": "LP2", "sets": 1, "phases": 3, "size": "#3", "material": "cu", "source": "FEEDER SCHEDULE X-1"}
            ],
            "equipment_electrical": [{"tag": "AHU-1", "description": "AIR HANDLER", "source_panel": "LP2", "drawing_ocpd": "60A/3P", "volts": 208.0, "phase": 3, "mca": 45.0, "source": "Sample submittal p.3"}],
            "panel_schedules": [
                {"panel": "MSB", "source": "X-2", "header": {"volts/phase": three, "main size / type": "400A MCB", "bus amps": "400A"},
                 "circuits": [ckt("1", "PANEL LP1", "100", 3, Some(64_212.0)), ckt("2", "PANEL LP2", "100", 3, Some(20_000.0))]},
                {"panel": "LP1", "source": "X-2", "header": {"volts/phase": three, "bus amps": "100A", "main size / type": "MLO"},
                 "circuits": [
                    ckt("1", "LIGHTING OFFICES", "20", 1, Some(1_500.0)),
                    ckt("2", "WATER HEATER", "30", 3, Some(9_000.0)),
                    ckt("3", "RECEPTACLES OPEN OFFICE", "20", 1, Some(1_800.0)),
                    ckt("5", "RECEPTACLES COPY ROOM", "20", 1, Some(900.0)),
                    ckt("7", "LIGHTING CORRIDOR", "20", 1, Some(300.0)),
                    ckt("8", "SPARE", "20", 1, Some(0.0))
                 ],
                 "totals": {"connected_va": 20_000.0, "phase_a_va": 9_000.0, "phase_b_va": 2_000.0, "phase_c_va": 2_200.0}},
                {"panel": "LP2", "source": "X-2", "header": {"volts/phase": three, "bus amps": "100A"},
                 "circuits": [
                    ckt("1", "AHU-1", "60", 3, Some(16_212.0)),
                    ckt("2", "KITCHEN FRYER", "50", 3, Some(15_000.0)),
                    ckt("7", "KITCHEN OVEN", "50", 3, Some(12_000.0)),
                    ckt("8", "KITCHEN KETTLE", "50", 3, Some(12_000.0)),
                    ckt("13", "KITCHEN WARMER", "40", 3, Some(9_000.0))
                 ],
                 "totals": {"connected_va": 64_212.0}}
            ],
            "load_summaries": [{"title": "SERVICE LOAD SUMMARY", "source": "X-2", "lines": [
                {"label": "SUB-TOTAL (VA)", "values": [84_212.0, 60_000.0]},
                {"label": "SUB-TOTAL (AMPS)", "values": [167.0]},
                {"label": "SERVICE SIZE (AMPS)", "values": [400.0]}
            ]}]
        }
    })
}

fn office() -> Session {
    let mut d = Drawing::new_imperial();
    luminaires(&mut d, "LP1", "LP1-1", 20, 100.0, "101");
    devices(&mut d, "receptacle", "LP1", "LP1-3", 14, "102");
    devices(&mut d, "receptacle", "LP1", "LP1-5", 2, "103");
    devices(&mut d, "junction_box", "LP1", "LP1-5", 1, "103");
    devices(&mut d, "switch", "LP1", "LP1-1", 2, "101");
    devices(&mut d, "receptacle", "LP1", "LP1-7", 12, "104");
    session(office_spec(), d)
}

fn find<'a>(v: &'a Value, rule: &str, panel: &str, circuit: &str) -> Vec<&'a Value> {
    v["findings"].as_array().unwrap().iter().filter(|f| f["rule"] == rule && f["panel"] == panel && f["circuit"] == circuit).collect()
}

#[test]
fn the_model_s_loads_are_checked_circuit_by_circuit_against_breakers_and_the_schedule() {
    let mut s = office();
    let v = s.execute("bim.loadcalc", &json!({"code_texts": ["USE AND OCCUPANCY CLASSIFICATION", "GROUP B OCCUPANCY (304.1)"]})).unwrap();
    assert_eq!(v["occupancy"]["id"], "commercial", "{}", v["occupancy"]);
    let lp1 = v["panels"].as_array().unwrap().iter().find(|p| p["panel"] == "LP1").unwrap();
    // 20 x 100 VA luminaires + 16 receptacles at 180 VA + the schedule's water heater; the switches
    // and the junction box carry nothing.
    assert_eq!(lp1["connected_va"]["model"], json!(2_000.0 + 14.0 * 180.0 + 2.0 * 180.0 + 9_000.0 + 12.0 * 180.0), "{lp1}");
    // The lighting circuit: 2,000 VA continuous is 83% of its 20 A breaker (over 80%).
    let c1 = find(&v, "E-LOAD-CIRCUIT", "LP1", "1");
    assert_eq!(c1.len(), 1, "{v:#}");
    assert!(c1[0]["message"].as_str().unwrap().contains("83%"), "{}", c1[0]);
    // The open-office receptacles: 2,520 VA is 21 A on 20 A.
    let c3 = find(&v, "E-LOAD-CIRCUIT", "LP1", "3");
    assert!(c3.iter().any(|f| f["message"].as_str().unwrap().contains("21.0 A on a 20 A")), "{c3:?}");
    // The model against the schedule: documented fixtures say the schedule is wrong; a circuit
    // with a device of no load says the model is; rule-valued receptacles say either.
    let s1 = find(&v, "E-LOAD-SCHEDULE", "LP1", "1");
    assert_eq!(s1.first().map(|f| f["likely_wrong"].clone()), Some(json!("schedule")), "{s1:?}");
    let s5 = find(&v, "E-LOAD-SCHEDULE", "LP1", "5");
    assert_eq!(s5.first().map(|f| f["likely_wrong"].clone()), Some(json!("model")), "{s5:?}");
    let s3 = find(&v, "E-LOAD-SCHEDULE", "LP1", "3");
    assert!(s3.first().is_some_and(|f| f["likely_wrong"] == "either" && f["message"].as_str().unwrap().contains("129 VA a receptacle")), "{s3:?}");
    // The schedule prints 20,000 VA; its circuits add to 13,500.
    let tot = find(&v, "E-LOAD-SCHEDULE", "LP1", "");
    assert!(tot.iter().any(|f| f["message"].as_str().unwrap().contains("circuits read add to 13,500 VA") && f["likely_wrong"] == "schedule"), "{tot:?}");
    // A lighting circuit the model puts receptacles on: over its breaker by the model only, the
    // model likely wrong (the schedule's 300 VA is well under).
    let c7 = find(&v, "E-LOAD-CIRCUIT", "LP1", "7");
    assert!(c7.first().is_some_and(|f| f["likely_wrong"] == "model" && f["message"].as_str().unwrap().contains("12 receptacle(s) on this lighting circuit")), "{c7:?}");
    let s7 = find(&v, "E-LOAD-SCHEDULE", "LP1", "7");
    assert_eq!(s7.first().map(|f| f["likely_wrong"].clone()), Some(json!("model")), "{s7:?}");
    // No finding on the spare.
    assert!(find(&v, "E-LOAD-CIRCUIT", "LP1", "8").is_empty());
}

#[test]
fn a_panel_over_its_bus_a_feeder_over_its_breaker_and_conductors_and_swapped_rows_are_found() {
    let mut s = office();
    let v = s.execute("bim.loadcalc", &json!({"occupancy": "B"})).unwrap();
    let lp2 = v["panels"].as_array().unwrap().iter().find(|p| p["panel"] == "LP2").unwrap();
    // The air handler from its submittal: 208 V x 45 A x 1.732.
    assert_eq!(lp2["from_model"], json!(1), "{lp2}");
    let steps = lp2["demand_steps"].as_array().unwrap();
    let kitchen = steps.iter().find(|s| s["what"].as_str().unwrap().starts_with("kitchen")).unwrap();
    assert!(kitchen["cite"].as_str().unwrap().contains("220.56"), "{kitchen}");
    assert_eq!(kitchen["demand_va"], json!(38_400.0), "four kitchen units at 80%: {kitchen}");
    assert!(steps.iter().any(|s| s["cite"].as_str().unwrap().contains("430.24")), "the largest motor's 25%: {steps:?}");
    assert!(lp2["demand_a"].as_f64().unwrap() > 100.0, "{lp2}");
    let over = find(&v, "E-LOAD-RATING", "LP2", "");
    assert!(over.iter().any(|f| f["message"].as_str().unwrap().contains("over its 100 A rating (bus 100A)")), "{over:?}");
    // MSB's row 2 feeds LP2 on 100 A, with #3 cu (100 A at 75 C).
    let feeder = find(&v, "E-LOAD-RATING", "MSB", "2");
    assert!(feeder.iter().any(|f| f["message"].as_str().unwrap().contains("on a 100 A breaker")), "{feeder:?}");
    assert!(feeder.iter().any(|f| f["message"].as_str().unwrap().contains("#3 cu carry 100 A")), "{feeder:?}");
    // MSB's rows carry each other's panel totals.
    let swapped = find(&v, "E-LOAD-SCHEDULE", "MSB", "1");
    assert!(swapped.iter().any(|f| f["message"].as_str().unwrap().contains("look swapped") && f["likely_wrong"] == "schedule"), "{swapped:?}");
    // The phases: LP1's schedule prints 9,000 / 2,000 / 2,200 VA.
    let ph = find(&v, "E-LOAD-PHASE", "LP1", "");
    assert!(ph.iter().any(|f| f["message"].as_str().unwrap().starts_with("LP1: the schedule's phase loads") && f["likely_wrong"] == "schedule"), "{ph:?}");
    // The service: MSB, nothing feeds it; its documents' summary read beside it.
    assert_eq!(v["service"]["panel"], "MSB");
    assert_eq!(v["service"]["documents"][0]["service_size_a"], json!(400.0));
    assert_eq!(v["service"]["level"], "service");
}

#[test]
fn the_phases_of_the_model_follow_the_circuit_numbers() {
    // All the load on phase A (circuits 1 and 2), nothing on B and C.
    let spec = json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}],
        "mep": {"panel_schedules": [{"panel": "LP9", "source": "X-3", "header": {"volts/phase": "480Y/277V, 3PH, 4W", "bus amps": "225A"},
            "circuits": [ckt("1", "LIGHTING NORTH", "20", 1, Some(2_000.0)), ckt("2", "LIGHTING SOUTH", "20", 1, Some(2_000.0)), ckt("3", "SPARE", "20", 1, None)]}]}
    });
    let mut d = Drawing::new_imperial();
    luminaires(&mut d, "LP9", "LP9-1", 10, 200.0, "1");
    luminaires(&mut d, "LP9", "LP9-2", 10, 200.0, "2");
    let mut s = session(spec, d);
    let v = s.execute("bim.loadcalc", &json!({})).unwrap();
    assert_eq!(v["occupancy"]["id"], "commercial", "no code sheet: commercial");
    let p = &v["panels"][0];
    assert_eq!(p["phases_va"]["model"], json!([4_000.0, 0.0, 0.0]), "{p}");
    let ph = find(&v, "E-LOAD-PHASE", "LP9", "");
    assert!(ph.iter().any(|f| f["likely_wrong"] == "design" && f["message"].as_str().unwrap().contains("200% out of balance")), "{ph:?}");
    // 277 V lighting: 2,000 VA continuous is 7.2 A of 20 A, no circuit finding.
    assert!(find(&v, "E-LOAD-CIRCUIT", "LP9", "1").is_empty(), "{v:#}");
}

fn lodging_spec(units_on_feeders: [u32; 2]) -> Value {
    let unit_total = 1_000.0 + 900.0 + 3_000.0 + 1_500.0;
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}],
        "mep": {
            "panel_schedules": [
                {"panel": "DP", "source": "X-4", "header": {"volts/phase": "208Y/120V, 3PH, 4W", "bus amps": "400A"},
                 "circuits": [
                    ckt("1", &format!("PANELBOARD U ({} UNITS)", units_on_feeders[0]), "100", 2, Some(unit_total * f64::from(units_on_feeders[0]))),
                    ckt("2", &format!("PANELBOARD U ({} UNITS)", units_on_feeders[1]), "100", 2, Some(unit_total * f64::from(units_on_feeders[1])))
                 ]},
                {"panel": "U", "typical_of": 6, "source": "X-4", "header": {"volts/phase": "208Y/120V, 1PH, 3W", "bus amps": "100A"},
                 "circuits": [ckt("1", "LIGHTING AND RECEPTACLES", "20", 1, Some(1_000.0)), ckt("2", "REFRIGERATOR", "20", 1, Some(900.0)), ckt("3", "AC UNIT", "20", 2, Some(3_000.0)), ckt("4", "KITCHEN COUNTER", "20", 1, Some(1_500.0))],
                 "totals": {"connected_va": unit_total}}
            ],
            "load_summaries": [{"title": "PANELBOARD DP DEMAND CALCULATION", "panel": "DP", "basis": "220.84", "source": "X-4", "lines": [
                {"label": "TOTAL AMPS AT 208V, 3PH:", "values": [90.0]}
            ]}]
        }
    })
}

fn lodging(units_on_feeders: [u32; 2]) -> Session {
    let mut d = Drawing::new_imperial();
    for room in ["201", "202", "203", "204", "205", "206"] {
        luminaires(&mut d, "U", "U-1", 4, 150.0, room);
        devices(&mut d, "receptacle", "U", "U-1", 2, room);
        devices(&mut d, "receptacle", "U", "U-2", 1, room);
        devices(&mut d, "receptacle", "U", "U-4", 2, room);
    }
    session(lodging_spec(units_on_feeders), d)
}

#[test]
fn typical_unit_panels_roll_up_through_their_feeders_with_the_code_sheet_s_occupancy() {
    let mut s = lodging([3, 3]);
    let code = json!(["USE AND OCCUPANCY CLASSIFICATION", "RESIDENTIAL GROUP R-1 OCCUPANCY (310.2)", "GROUP B OCCUPANCY"]);
    let v = s.execute("bim.loadcalc", &json!({"code_texts": code, "code_sheet": "X-0"})).unwrap();
    assert_eq!(v["occupancy"]["id"], "hotel", "a unit occupancy before the others: {}", v["occupancy"]);
    assert!(v["occupancy"]["from"].as_str().unwrap().contains("code sheet X-0"), "{}", v["occupancy"]);
    let u = v["panels"].as_array().unwrap().iter().find(|p| p["panel"] == "U").unwrap();
    assert_eq!(u["units_in_model"], json!(6));
    // One unit: 4 x 150 VA + 2 x 180 VA on circuit 1; circuit 2 names its appliance, whose
    // receptacle is its connection: the schedule's 900 VA; the schedule's 3,000 VA unit; the
    // kitchen counter's small-appliance circuit at 1,500 VA by rule.
    assert_eq!(u["connected_va"]["model"], json!(600.0 + 360.0 + 900.0 + 3_000.0 + 1_500.0), "{u}");
    let u4 = v["panels"].as_array().unwrap().iter().find(|p| p["panel"] == "U").unwrap();
    assert_eq!(u4["from_model"], json!(3), "{u4}");
    assert!(find(&v, "E-LOAD-SCHEDULE", "U", "2").is_empty(), "the appliance's load is the schedule's: {v:#}");
    let dp = v["panels"].as_array().unwrap().iter().find(|p| p["panel"] == "DP").unwrap();
    assert_eq!(dp["units_under"], json!(6), "{dp}");
    assert_eq!(dp["connected_va"]["model"], json!(6.0 * 6_360.0), "{dp}");
    let steps = dp["demand_steps"].as_array().unwrap();
    let general = steps.iter().find(|s| s["what"].as_str().unwrap().starts_with("general lighting")).unwrap();
    assert!(general["cite"].as_str().unwrap().contains("Table 220.42, hotels"), "{general}");
    assert_eq!(general["connected_va"], json!(6.0 * 2_460.0), "the units' receptacles and small-appliance circuits in their general lighting: {general}");
    assert_eq!(general["demand_va"], json!(7_380.0), "{general}");
    // The documents' demand calculation uses the multifamily optional method on a hotel.
    let doc = find(&v, "E-LOAD-SCHEDULE", "DP", "");
    assert!(doc.iter().any(|f| f["likely_wrong"] == "documents" && f["message"].as_str().unwrap().contains("220.84")), "{doc:?}");
    assert!(doc.iter().any(|f| f["message"].as_str().unwrap().contains("6 units at 44%")), "Table 220.84 on the model's loads: {doc:?}");
    // The feeders' unit counts are the typical panel's.
    assert!(find(&v, "E-LOAD-SCHEDULE", "U", "").iter().all(|f| !f["message"].as_str().unwrap().contains("typical of")), "{v:#}");
}

#[test]
fn the_dwelling_rules_and_a_unit_count_the_feeders_dont_match() {
    let mut s = lodging([3, 2]);
    let v = s.execute("bim.loadcalc", &json!({"occupancy": "R-2"})).unwrap();
    assert_eq!(v["occupancy"]["id"], "dwelling");
    let dp = v["panels"].as_array().unwrap().iter().find(|p| p["panel"] == "DP").unwrap();
    let general = dp["demand_steps"].as_array().unwrap().iter().find(|s| s["what"].as_str().unwrap().starts_with("general lighting")).unwrap();
    // 5 units x 2,460 VA: 3,000 VA at 100%, the rest at 35%.
    assert_eq!(general["demand_va"], json!(6_255.0), "{general}");
    // A dwelling occupancy's own optional method is no finding.
    assert!(find(&v, "E-LOAD-SCHEDULE", "DP", "").iter().all(|f| f["likely_wrong"] != "documents"), "{v:#}");
    let typ = find(&v, "E-LOAD-SCHEDULE", "U", "");
    assert!(typ.iter().any(|f| f["message"].as_str().unwrap().contains("typical of 6; the feeder rows feed 5 units")), "{typ:?}");
}

#[test]
fn the_report_is_written_as_csv_and_text_with_a_source_on_every_line() {
    let dir = std::env::temp_dir().join(format!("cadaddy-loadcalc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (csv, txt) = (dir.join("loads.csv"), dir.join("loads.txt"));
    let mut s = office();
    let v = s.execute("bim.loadcalc", &json!({"csv": csv.to_string_lossy(), "text": txt.to_string_lossy()})).unwrap();
    assert_eq!(v["csv"], json!(csv.to_string_lossy()));
    let c = std::fs::read_to_string(&csv).unwrap();
    let head = c.lines().next().unwrap();
    assert!(head.starts_with("level,panel,circuit,") && head.ends_with(",findings,sources"), "{head}");
    let circuits: Vec<&str> = c.lines().filter(|l| l.starts_with("circuit,LP1,")).collect();
    assert_eq!(circuits.len(), 6, "{c}");
    // Every circuit line with a load says where it came from.
    assert!(circuits.iter().filter(|l| !l.contains(",SPARE,")).all(|l| l.contains("X-2 LIGHT FIXTURE SCHEDULE") || l.contains("220.14") || l.contains("schedule")), "{c}");
    assert!(c.lines().any(|l| l.starts_with("service,MSB,")), "{c}");
    assert!(c.lines().any(|l| l.starts_with("finding,") && l.contains("E-LOAD-CIRCUIT")), "{c}");
    let t = std::fs::read_to_string(&txt).unwrap();
    assert!(t.contains("SERVICE / PANEL MSB") && t.contains("PANEL LP1") && t.contains("FINDINGS:"), "{t}");
    assert!(t.contains("[NEC 220.44"), "each demand step with its cite: {t}");
    // Again without force: refused, the file kept.
    let e = s.execute("bim.loadcalc", &json!({"csv": csv.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("force"), "{e}");
    let one = s.execute("bim.loadcalc", &json!({"panel": "lp2"})).unwrap();
    assert_eq!(one["panels"].as_array().unwrap().len(), 1);
    assert!(s.execute("bim.loadcalc", &json!({"panel": "NOPE"})).is_err());
    assert!(s.execute("bim.loadcalc", &json!({"occupancy": "Z-9"})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_schedules_or_hostile_values_never_panic() {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let v = s.execute("bim.loadcalc", &json!({})).unwrap();
    assert!(v["note"].as_str().unwrap().contains("no panel schedules"));
    // A panel feeding itself, a feeder loop, absurd numbers.
    let spec = json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12}],
        "mep": {"panel_schedules": [
            {"panel": "P1", "header": {"volts/phase": "nonsense", "bus amps": "-5A"}, "circuits": [ckt("1", "PANEL P1", "1e308", 9, Some(1e300)), ckt("2", "PANEL P2 (99999 UNITS)", "x", 0, None)]},
            {"panel": "P2", "typical_of": 4_000_000_000u32, "circuits": [ckt("1", "PANEL P1", "20", 2, Some(f64::MAX)), ckt("0", "", "", 1, Some(-5.0))]}
        ]}
    });
    let mut d = Drawing::new_imperial();
    dev(&mut d, 0.0, &[("kind", json!("luminaire")), ("panel", json!("P2")), ("circuit", json!("P2-")), ("fixture_va", json!(-1))]);
    dev(&mut d, 1.0, &[("kind", json!("luminaire")), ("panel", json!("P2")), ("circuit", json!("P2-1")), ("wattage", json!("lots"))]);
    let mut s = session(spec, d);
    let v = s.execute("bim.loadcalc", &json!({"code_texts": [1, null, "x".repeat(100_000)]})).unwrap();
    assert!(v["panels"].as_array().unwrap().len() == 2, "{v}");
}
