//! Quantities from the MEP model (`mep.qty`): a made-up two-level model whose takeoff is worked
//! out by hand — duct area and weight by gauge, pipe and conduit length by system and size (a
//! drop counted in), fittings by type and size (elbows, an offset, a tee, a transition, a
//! reducer), terminals, the equipment list and hangers — grouped by level and system, exported
//! as CSV and XLSX, and a takeoff table in the drawing that follows an edit of the model.

use cadaddy_engine::Session;
use cadaddy_engine::doc::EntityKind;
use serde_json::{Value, json};

fn model() -> Session {
    let mut s = Session::new();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-1000, -1000, 1000, 1000]},
                {"name": "L2", "elevation_ft": 14, "height_ft": 12, "ceiling_ft": 9, "region": [2000, -1000, 4000, 1000]}
            ],
            "mep": {
                "systems": [
                    {"name": "SA", "layer": "M-DUCT-SUPP", "kind": "duct", "size_in": [24, 12], "bottom_ft": 10, "duct_type": "supply"},
                    {"name": "RA", "layer": "M-DUCT-RET", "kind": "duct", "size_in": [12, 0], "bottom_ft": 9},
                    {"name": "CW", "layer": "P-DOMW-CW", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 10},
                    {"name": "HW", "layer": "P-DOMW-HW", "kind": "pipe", "size_in": [0.75, 0], "bottom_ft": 10},
                    {"name": "EC", "layer": "E-COND", "kind": "conduit", "size_in": [1, 0], "bottom_ft": 11}
                ],
                "runs": [
                    // 0: main, 20' then a 90° turn and 10'.
                    {"system": "SA", "level": "L1", "points": [[0, 0], [240, 0], [240, 120]], "size_in": [24, 12], "bottoms_ft": [10, 10]},
                    // 1: a branch off the main's side (a tee), 8'.
                    {"system": "SA", "level": "L1", "points": [[120, 0], [120, -96]], "size_in": [12, 8], "bottoms_ft": [10]},
                    // 2: on from the main's end, smaller (a transition), 10'.
                    {"system": "SA", "level": "L1", "points": [[240, 120], [360, 120]], "size_in": [18, 12], "bottoms_ft": [10]},
                    // 3, 4: round return, 12" then 8" (a reducer), 10' each.
                    {"system": "RA", "level": "L1", "points": [[0, 200], [120, 200]], "size_in": [12, 0], "bottoms_ft": [9]},
                    {"system": "RA", "level": "L1", "points": [[120, 200], [240, 200]], "size_in": [8, 0], "bottoms_ft": [9]},
                    // 5: cold water dropping 2' at its corner: 10' + 8'-4" + 2'.
                    {"system": "CW", "level": "L1", "points": [[0, 300], [120, 300], [120, 400]], "size_in": [2, 0], "bottoms_ft": [10, 8]},
                    // 6: hot water upstairs, 5'.
                    {"system": "HW", "level": "L2", "points": [[2000, 0], [2060, 0]], "size_in": [0.75, 0], "bottoms_ft": [10]},
                    // 7: conduit, 10'.
                    {"system": "EC", "level": "L1", "points": [[0, 500], [120, 500]], "size_in": [1, 0], "bottoms_ft": [11]}
                ],
                "terminals": [
                    {"system": "SA", "level": "L1", "at": [60, -24], "size_in": [24, 24], "bottom_ft": 9, "terminal_kind": "diffuser"},
                    {"system": "SA", "level": "L1", "at": [180, -24], "size_in": [24, 24], "bottom_ft": 9, "terminal_kind": "diffuser"},
                    {"system": "RA", "level": "L1", "at": [60, 224], "size_in": [12, 0], "bottom_ft": 9, "terminal_kind": "grille"}
                ],
                "equipment": [
                    {"tag": "AHU-1", "kind": "ahu", "level": "L1", "min": [-120, -120], "max": [-72, -84], "bottom_ft": 0, "height_ft": 5,
                     "systems": ["SA", "RA"], "title": "AIR HANDLING UNIT SCHEDULE", "properties": {"CFM": "2400"}},
                    {"tag": "WH-1", "kind": "water_heater", "level": "L2", "min": [2100, 0], "max": [2124, 24], "bottom_ft": 0, "height_ft": 5, "round": true,
                     "systems": ["CW", "HW"]}
                ]
            }
        }}),
    )
    .unwrap();
    s
}

fn row(list: &Value, f: impl Fn(&Value) -> bool) -> &Value {
    list.as_array().unwrap().iter().find(|r| f(r)).unwrap_or_else(|| panic!("no such row in {list}"))
}

fn near(v: &Value, want: f64) -> bool {
    (v.as_f64().unwrap() - want).abs() < 0.011
}

fn count(q: &Value, ty: &str, size: &str) -> u64 {
    q["fittings"].as_array().unwrap().iter().filter(|f| f["type"] == ty && f["size"] == size).map(|f| f["count"].as_u64().unwrap()).sum()
}

#[test]
fn duct_area_and_weight_by_gauge() {
    let mut s = model();
    let q = s.execute("mep.qty", &json!({})).unwrap();
    let main = row(&q["ducts"], |r| r["size"] == "24x12");
    assert_eq!((main["level"].as_str(), main["system"].as_str(), main["shape"].as_str()), (Some("L1"), Some("SA"), Some("rectangular")));
    assert!(near(&main["length_ft"], 30.0), "{main}");
    // Perimeter 2 × (24 + 12) = 72" = 6 ft; 24" longer side is 24 ga at 1.156 lb/sq ft.
    assert!(near(&main["area_sqft"], 180.0), "{main}");
    assert_eq!(main["gauge"], 24);
    assert!(near(&main["weight_lb"], 208.08), "{main}");
    let branch = row(&q["ducts"], |r| r["size"] == "12x8");
    assert_eq!(branch["gauge"], 26);
    assert!(near(&branch["area_sqft"], 40.0 / 12.0 * 8.0), "{branch}");
    let round = row(&q["ducts"], |r| r["size"] == "12\"Ø");
    assert_eq!(round["shape"], "round");
    assert!(near(&round["area_sqft"], std::f64::consts::PI * 10.0), "{round}");
    assert_eq!(round["runs"], json!([3]), "rows name the runs they count (mep.list ids)");
    // A seam allowance and our own gauge table.
    let a = s.execute("mep.qty", &json!({"allowance_pct": 10, "gauges": [{"gauge": 20, "lb_per_sqft": 2.0}]})).unwrap();
    let main = row(&a["ducts"], |r| r["size"] == "24x12");
    assert!(near(&main["area_sqft"], 198.0) && near(&main["weight_lb"], 396.0) && main["gauge"] == 20, "{main}");
    let t = &q["totals"];
    let want_area = 180.0 + 40.0 / 12.0 * 8.0 + 60.0 / 12.0 * 10.0 + std::f64::consts::PI * 10.0 + std::f64::consts::PI * 8.0 / 12.0 * 10.0;
    assert!(near(&t["duct_area_sqft"], want_area), "{t}");
}

#[test]
fn pipe_and_conduit_length_by_system_and_size() {
    let mut s = model();
    let q = s.execute("mep.qty", &json!({})).unwrap();
    let cw = row(&q["pipes"], |r| r["system"] == "CW");
    assert_eq!(cw["size"], "2\"");
    assert!(near(&cw["length_ft"], 10.0 + 100.0 / 12.0 + 2.0), "the drop counts: {cw}");
    let hw = row(&q["pipes"], |r| r["system"] == "HW");
    assert_eq!((hw["level"].as_str(), hw["size"].as_str()), (Some("L2"), Some("3/4\"")));
    assert!(near(&hw["length_ft"], 5.0));
    let ec = row(&q["pipes"], |r| r["system"] == "EC");
    assert_eq!(ec["kind"], "conduit");
    assert!(near(&q["totals"]["pipe_length_ft"], 17.0 + 100.0 / 12.0));
    assert!(near(&q["totals"]["conduit_length_ft"], 10.0));
}

#[test]
fn fittings_by_type_and_size() {
    let mut s = model();
    let q = s.execute("mep.qty", &json!({})).unwrap();
    assert_eq!(count(&q, "elbow 90°", "24x12"), 1, "{}", q["fittings"]);
    assert_eq!(count(&q, "tee", "24x12 x 12x8"), 1, "{}", q["fittings"]);
    assert_eq!(count(&q, "transition", "24x12 to 18x12"), 1, "{}", q["fittings"]);
    assert_eq!(count(&q, "reducer", "12\"Ø to 8\"Ø"), 1, "{}", q["fittings"]);
    assert_eq!(count(&q, "offset", "2\""), 1, "{}", q["fittings"]);
    assert_eq!(q["totals"]["fittings"], 5, "nothing else: {}", q["fittings"]);
    // Three runs meeting at one end: a tee, by the larger size and the smallest branch.
    s.execute(
        "bspec.set",
        &json!({"spec": {"mep": {"runs": [
            {"system": "CW", "level": "L1", "points": [[0, 600], [120, 600]], "size_in": [2, 0], "bottoms_ft": [10]},
            {"system": "CW", "level": "L1", "points": [[120, 600], [240, 600]], "size_in": [1.5, 0], "bottoms_ft": [10]},
            {"system": "CW", "level": "L1", "points": [[120, 600], [120, 700]], "size_in": [1, 0], "bottoms_ft": [10]}
        ]}}}),
    )
    .unwrap();
    let q = s.execute("mep.qty", &json!({"system": "CW"})).unwrap();
    assert_eq!(count(&q, "tee", "2\" x 1\""), 1, "{}", q["fittings"]);
}

#[test]
fn terminals_equipment_and_hangers() {
    let mut s = model();
    let q = s.execute("mep.qty", &json!({})).unwrap();
    let d = row(&q["terminals"], |r| r["kind"] == "diffuser");
    assert_eq!((d["count"].as_u64(), d["size"].as_str()), (Some(2), Some("24x24")));
    assert_eq!(row(&q["terminals"], |r| r["kind"] == "grille")["size"], "12\"Ø");
    let ahu = row(&q["equipment"], |r| r["tag"] == "AHU-1");
    assert_eq!(ahu["size"], "4'-0\" x 3'-0\" x 5'-0\" H");
    assert_eq!(row(&q["equipment"], |r| r["tag"] == "WH-1")["size"], "2'-0\"Ø x 5'-0\" H");
    assert_eq!(q["totals"]["equipment_by_kind"], json!({"ahu": 1, "water_heater": 1}));
    // Main duct at 8': 20' → 3, 10' → 2; rods from its top (11') to the floor above (14').
    let h = row(&q["hangers"], |r| r["size"] == "24x12");
    assert_eq!((h["count"].as_u64(), h["spacing_ft"].as_f64()), (Some(5), Some(8.0)));
    assert!(near(&h["rod_ft"], 15.0), "{h}");
    // 2" pipe at 8': 10' → 2, 8'-4" → 2.
    assert_eq!(row(&q["hangers"], |r| r["size"] == "2\"")["count"], 4);
    // 3/4" at 6': 5' → 1.
    assert_eq!(row(&q["hangers"], |r| r["size"] == "3/4\"")["count"], 1);
    let wide = s.execute("mep.qty", &json!({"hanger_spacing_ft": {"duct": 10}})).unwrap();
    assert_eq!(row(&wide["hangers"], |r| r["size"] == "24x12")["count"], 3);
}

#[test]
fn grouped_and_filtered_by_level_and_system() {
    let mut s = model();
    let l2 = s.execute("mep.qty", &json!({"level": "L2"})).unwrap();
    assert_eq!(l2["ducts"], json!([]));
    assert_eq!(l2["pipes"].as_array().unwrap().len(), 1);
    assert_eq!(l2["equipment"].as_array().unwrap().len(), 1);
    let sa = s.execute("mep.qty", &json!({"system": "SA"})).unwrap();
    assert!(sa["pipes"].as_array().unwrap().is_empty() && sa["ducts"].as_array().unwrap().len() == 3);
    assert_eq!(sa["equipment"].as_array().unwrap().len(), 1, "the equipment joined to SA");
    // The same pipe size on two levels is one row by system (a spec patch replaces the runs: the
    // whole spec goes back with one more).
    let mut spec = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    spec["mep"]["runs"]
        .as_array_mut()
        .unwrap()
        .push(json!({"system": "HW", "level": "L1", "points": [[0, 0], [36, 0]], "size_in": [0.75, 0], "bottoms_ft": [10]}));
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    let by_sys = s.execute("mep.qty", &json!({"group_by": "system", "system": "HW"})).unwrap();
    let hw = by_sys["pipes"].as_array().unwrap();
    assert_eq!(hw.len(), 1, "{hw:?}");
    assert_eq!(hw[0]["level"], "All");
    assert!(near(&hw[0]["length_ft"], 8.0), "{hw:?}");
    assert!(s.execute("mep.qty", &json!({"level": "Roof"})).is_err());
    assert!(s.execute("mep.qty", &json!({"group_by": "room"})).is_err());
}

#[test]
fn csv_and_xlsx_export() {
    let mut s = model();
    let dir = std::env::temp_dir().join(format!("cadaddy-mepqty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = |n: &str| dir.join(n).to_string_lossy().to_string();
    let q = s.execute("mep.qty", &json!({"file": path("Sample Takeoff.csv")})).unwrap();
    assert_eq!(q["format"], "csv");
    let csv = std::fs::read_to_string(path("Sample Takeoff.csv")).unwrap();
    assert_eq!(csv.lines().count() as u64, q["rows"].as_u64().unwrap() + 1);
    assert!(csv.starts_with("Category,Level,System,Item,Size,Gauge,Quantity,Unit,Area (sq ft),Weight (lb)"));
    assert!(csv.contains("Duct,L1,SA,rectangular duct,24x12,24,30,ft,180,208.08"), "{csv}");
    assert!(csv.contains("Fitting,L1,SA,tee,24x12 x 12x8,,1,ea,,"), "{csv}");
    assert!(csv.contains("Equipment,L1,\"SA, RA\",AHU-1 ahu"), "{csv}");
    assert_eq!(q["csv"].as_str(), Some(csv.as_str()));
    let x = s.execute("mep.qty", &json!({"file": path("Sample Takeoff.xlsx")})).unwrap();
    assert_eq!(x["format"], "xlsx");
    let xb = std::fs::read(path("Sample Takeoff.xlsx")).unwrap();
    assert!(xb.starts_with(b"PK"));
    assert!(s.execute("mep.qty", &json!({"file": path("x.doc")})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The text of a table's cells, row by row.
fn cells(s: &Session, h: &str) -> Vec<Vec<String>> {
    let d = s.doc().unwrap();
    let e = d.entity(cadaddy_engine::doc::Handle::parse_hex(h).unwrap()).unwrap();
    let EntityKind::Table(t) = &e.kind else { panic!("not a table") };
    t.cells.iter().map(|r| r.iter().map(|c| c.text.clone()).collect()).collect()
}

#[test]
fn a_takeoff_table_follows_edits_of_the_model() {
    let mut s = model();
    let q = s.execute("mep.qty", &json!({"system": "SA", "table": {"at": [0, -3000], "part": "ducts"}})).unwrap();
    let h = q["table"].as_str().unwrap().to_string();
    let before = cells(&s, &h);
    assert_eq!(before[0][0], "MEP Quantities — SA");
    assert_eq!(before[1][0], "Category");
    assert!(before.iter().any(|r| r[4] == "24x12" && r[6] == "30"), "{before:?}");
    assert_eq!(before.len(), 2 + 3, "title, header and the three duct sizes");
    // The branch (run 1) becomes 14x10; the rebuild counts again.
    let e = s.execute("mep.edit", &json!({"id": 1, "size_in": [14, 10], "rebuild": true})).unwrap();
    assert_eq!(e["rebuilt"], true);
    let after = cells(&s, &h);
    assert!(after.iter().any(|r| r[4] == "14x10"), "{after:?}");
    assert!(!after.iter().any(|r| r[4] == "12x8"), "{after:?}");
    // A run added by hand to the spec shows after model.build.
    s.execute(
        "bspec.set",
        &json!({"spec": {"mep": {"runs": [{"system": "SA", "level": "L1", "points": [[0, 50], [96, 50]], "size_in": [10, 6], "bottoms_ft": [10]}]}}}),
    )
    .unwrap();
    let b = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(b["qtyTables"], 1);
    assert!(cells(&s, &h).iter().any(|r| r[4] == "10x6" && r[6] == "8"));
    // Undo the build: the table is as it was.
    s.execute("undo", &json!({})).unwrap();
    assert!(!cells(&s, &h).iter().any(|r| r[4] == "10x6"));
    assert_eq!(s.execute("mep.qty.refresh", &json!({})).unwrap()["tables"], 1);
    assert!(s.execute("mep.qty", &json!({"table": {"at": [0, 0], "part": "everything"}})).is_err());
}

#[test]
fn read_from_the_plan_then_counted() {
    // Drawn single-line, read into the spec, then counted: the takeoff is of the model.
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "M-DUCT-SUPP", "current": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[24, 60], [264, 60], [264, 180]]})).unwrap();
    s.execute("text", &json!({"at": [180, 70], "text": "24x12 BOT @ 10'-0\"", "height": 4})).unwrap();
    s.execute("text", &json!({"at": [280, 140], "text": "BOT @ 8'-0\"", "height": 4})).unwrap();
    s.execute("bspec.set", &json!({"replace": true, "spec": {"levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9}]}}))
        .unwrap();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    let q = s.execute("mep.qty", &json!({})).unwrap();
    let d = row(&q["ducts"], |r| r["size"] == "24x12");
    assert!(near(&d["length_ft"], 32.0), "20' + 10' + a 2' drop: {d}");
    assert_eq!(count(&q, "drop", "24x12"), 1, "{}", q["fittings"]);
}

#[test]
fn hostile_models_never_panic() {
    let mut s = model();
    let bad = json!({"mep": {"runs": [
        {"system": "SA", "level": "L1", "points": [[1e308, -1e308], [-1e308, 1e308], [0, 0]], "size_in": [1e308, 1e308], "bottoms_ft": [1e308]},
        {"system": "SA", "level": "L1", "points": [], "size_in": [24, 12], "bottoms_ft": []},
        {"system": "SA", "level": "L1", "points": [[0, 0]], "size_in": [-5, 0], "bottoms_ft": [10, 10, 10]},
        {"system": "nowhere", "level": "nowhere", "points": [[0, 0], [1, 1]], "size_in": [1, 0], "bottoms_ft": [0]}
    ]}});
    let _ = s.execute("bspec.set", &json!({"spec": bad}));
    let _ = s.execute("mep.qty", &json!({}));
    let _ = s.execute("mep.qty", &json!({"gauges": [{"max_in": -1, "gauge": 0}], "hanger_spacing_ft": {"pipe": 1e308}}));
    let _ = s.execute("mep.qty", &json!({"table": {"at": [1e308, 1e308]}}));
    let _ = s.execute("mep.qty.refresh", &json!({}));
}
