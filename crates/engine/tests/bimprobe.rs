//! Q5244: the commands an agent runs on a model built from a plan, on a made-up three-room plan
//! like a PDF-built one (room tags with unit-type words and a panelboard label beside them,
//! device marks written as text, a door swing flattened to a polyline): rooms from the built
//! ceilings with their room names, device reading with no print set, plan doors counted, ceiling
//! and unit-type takeoffs from the model and the plan, the estimate chain without dead ends,
//! issues to BCF in one call, the built model as IFC elements, and same-trade wall clashes.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// Feet → inches.
fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    let p = |x: f64, y: f64| [x * 12.0, y * 12.0];
    s.execute("pline", &json!({"vertices": [p(a[0], a[1]), p(b[0], a[1]), p(b[0], b[1]), p(a[0], b[1])], "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [[a[0] * 12.0, a[1] * 12.0], [b[0] * 12.0, b[1] * 12.0]]})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": [at[0] * 12.0, at[1] * 12.0], "text": t, "height": 6})).unwrap();
}

/// A door swing as a PDF print brings it: a quarter circle flattened to a 9-point polyline,
/// hinged at `hinge` (feet) with a 3' leaf, plus the leaf line.
fn flat_swing(s: &mut Session, hinge: [f64; 2]) {
    let (cx, cy, r) = (hinge[0] * 12.0, hinge[1] * 12.0, 36.0);
    let pts: Vec<[f64; 2]> = (0..=8)
        .map(|i| {
            let a = std::f64::consts::FRAC_PI_2 * f64::from(i) / 8.0;
            [cx + r * a.cos(), cy + r * a.sin()]
        })
        .collect();
    s.execute("pline", &json!({"vertices": pts})).unwrap();
    s.execute("line", &json!({"points": [[cx, cy], [cx, cy + r]]})).unwrap();
}

/// A 45' × 12' building of 6" walls split into three rooms by centreline partitions at 15' and
/// 30'. Room 1 has a panelboard label first, then its unit type and number; room 2 is a second
/// DOUBLE Q, room 3 a SINGLE Q. Device marks OS, DH and J as plain text; a flattened door swing.
fn plan() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [45.0, 12.0]);
    rect(&mut s, [0.5, 0.5], [44.5, 11.5]);
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [15.0, 0.5], [15.0, 11.5]);
    line(&mut s, [30.0, 0.5], [30.0, 11.5]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, [2.0, 9.5], "PNLBD 'D'");
    text(&mut s, [6.0, 6.0], "DOUBLE Q");
    text(&mut s, [6.0, 4.5], "101");
    text(&mut s, [21.0, 6.0], "DOUBLE Q");
    text(&mut s, [36.0, 6.0], "SINGLE Q");
    layer(&mut s, "E-DEVICE");
    text(&mut s, [10.0, 3.0], "OS");
    text(&mut s, [25.0, 3.0], "DH");
    text(&mut s, [40.0, 3.0], "J");
    layer(&mut s, "A-DOOR");
    flat_swing(&mut s, [3.0, 1.0]);
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
            "layers": [
                {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
                {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
                {"layer": "A-ROOM-TAG", "role": "room_tag"}
            ],
            "wall_types": [
                {"name": "Exterior", "thickness_in": 6},
                {"name": "Partition", "thickness_in": 4.875, "from": "centerline"}
            ]
        }}),
    )
    .unwrap();
    s
}

fn built() -> Session {
    let mut s = plan();
    s.execute("model.build", &json!({})).unwrap();
    s
}

fn names(r: &Value) -> Vec<String> {
    let mut v: Vec<String> = r["rooms"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap().to_string()).collect();
    v.sort();
    v
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-bimprobe-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// MUST 1: one source of truth — the built model's rooms are its room ceilings, named by the
/// unit-type word inside them, never by the panelboard label the trace started from.
#[test]
fn built_rooms_are_the_room_ceilings_named_by_their_room_tags() {
    let mut s = built();
    let ceilings = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("ceiling"))
        .filter(|e| e.common.xdata.iter().any(|t| t.value.starts_with("room:")))
        .count();
    assert_eq!(ceilings, 3, "one ceiling per room");
    let r = s.execute("rooms.list", &json!({"outline": false})).unwrap();
    assert_eq!(r["count"], json!(ceilings), "{r}");
    assert_eq!(names(&r), vec!["DOUBLE Q", "DOUBLE Q", "SINGLE Q"], "{r}");
    assert!(!r.to_string().contains("PNLBD"), "a panelboard label never names a room: {r}");
    let first = r["rooms"].as_array().unwrap().iter().find(|x| x["number"] == json!("101")).unwrap();
    assert_eq!(first["name"], json!("DOUBLE Q"));
    assert_eq!(first["level"], json!("L1"));
    assert!((first["ceilingFt"].as_f64().unwrap() - 9.0).abs() < 1e-6, "{first}");
    assert!(first["areaSqFt"].as_f64().unwrap() > 100.0 && first["areaSqFt"].as_f64().unwrap() < 180.0, "{first}");
    // rooms.get finds the same rooms.
    assert_eq!(s.execute("rooms.get", &json!({"number": "101"})).unwrap()["name"], json!("DOUBLE Q"));
}

/// MUST 1 (before a build): the plan's rooms skip equipment and device tags too.
#[test]
fn plan_rooms_never_take_an_equipment_label() {
    let mut s = plan();
    let r = s.execute("rooms.list", &json!({"outline": false})).unwrap();
    assert_eq!(names(&r), vec!["DOUBLE Q", "DOUBLE Q", "SINGLE Q"], "{r}");
}

/// MUST 2: device reading works on any open drawing (no print set): the marks written as text.
#[test]
fn devices_read_from_the_open_drawing_without_a_print_set() {
    let mut s = plan();
    let r = s.execute("mep.read_devices", &json!({})).unwrap();
    assert_eq!(r["count"], json!(3), "{r}");
    assert_eq!(r["print_set"], json!("current drawing"));
    assert_eq!(r["by_type"], json!({"DH": 1, "J": 1, "OS": 1}), "{r}");
    let os = r["devices"].as_array().unwrap().iter().find(|d| d["tag"] == "OS").unwrap();
    assert_eq!(os["kind"], json!("occupancy sensor"));
    assert_eq!(os["level"], json!("L1"));
    // A drawing with no spec at all reads too.
    let mut bare = Session::new();
    bare.execute("text", &json!({"at": [0, 0], "text": "OS", "height": 6})).unwrap();
    assert_eq!(bare.execute("mep.read_devices", &json!({})).unwrap()["count"], json!(1));
}

/// MUST 3: plan doors count door swings flattened to polylines (how PDF prints draw them).
#[test]
fn plan_doors_count_flattened_swings() {
    let mut s = built();
    let c = s.execute("model.counts", &json!({})).unwrap();
    let l1 = &c["levels"][0];
    assert_eq!(l1["plan"]["door_swings_read"], json!(1), "{c}");
    assert!(l1["plan"]["doors"].as_u64().unwrap() >= 1, "{c}");
    let o = s.execute("plan.openings", &json!({})).unwrap();
    assert!(o.to_string().contains("\"swing\""), "{o}");
}

/// MUST 4: the ceiling takeoff reads the built ceilings when the spec has no ceiling types.
#[test]
fn rcp_takeoff_reads_the_built_ceilings() {
    let mut s = plan();
    assert_eq!(s.execute("rcp.takeoff", &json!({})).unwrap()["rows"], json!([]));
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("rcp.takeoff", &json!({})).unwrap();
    assert_eq!(r["source"], json!("model"), "{r}");
    let sf = r["totals"]["ceiling"].as_f64().unwrap();
    assert!(sf > 400.0 && sf < 490.0, "about 44' × 11' less the partitions: {sf}");
    assert!(r["totals"]["ceiling edge"].as_f64().unwrap() > 100.0, "{r}");
    assert!(r["rows"].as_array().unwrap().iter().all(|x| x["level"] == "L1"), "{r}");
    assert_eq!(s.execute("rcp.takeoff", &json!({"level": "L9"})).unwrap()["rows"], json!([]));
}

/// MUST 4: unit types from the plan's tags when the spec defines none, with the devices in them.
#[test]
fn unit_type_takeoff_reads_the_plan_tags() {
    let mut s = built();
    let r = s.execute("unittypes.takeoff", &json!({})).unwrap();
    let types = r["types"].as_array().unwrap();
    assert_eq!(types.len(), 2, "{r}");
    let count = |t: &str| types.iter().find(|x| x["type"] == t).map(|x| x["count"].clone());
    assert_eq!(count("DOUBLE Q"), Some(json!(2)), "{r}");
    assert_eq!(count("SINGLE Q"), Some(json!(1)), "{r}");
    assert_eq!(r["grand_total"]["devices"], json!({"DH": 1, "J": 1, "OS": 1}), "{r}");
    // Without rooms: the tags themselves.
    let mut bare = Session::new();
    for (x, t) in [(0, "DOUBLE Q"), (100, "DOUBLE Q"), (200, "ADA KING"), (300, "UNIT HEATER")] {
        bare.execute("text", &json!({"at": [x, 0], "text": t, "height": 6})).unwrap();
    }
    let r = bare.execute("unittypes.takeoff", &json!({})).unwrap();
    assert_eq!(r["types"].as_array().unwrap().len(), 2, "a unit heater is no unit type: {r}");
}

/// MUST 5: the whole-job estimate runs its prerequisite itself, and says what to do when it can't.
#[test]
fn wholejob_estimate_is_never_a_dead_end() {
    let mut s = built();
    let r = s.execute("wholejob.estimate", &json!({}));
    match r {
        Ok(v) => {
            assert!(v["quantities_from"]["command"].as_str().unwrap().contains("from_bom"), "{v}");
            assert!(v["trades"].as_array().is_some_and(|t| !t.is_empty()), "{v}");
            if v["unpriced_lines"].as_u64().unwrap() > 0 {
                assert!(v["next_step"].as_str().unwrap().contains("wholejob.prices"), "{v}");
            }
        }
        Err(e) => {
            let e = e.to_string();
            assert!(e.contains("bim.build") && e.contains("wholejob.quantities {trade"), "{e}");
        }
    }
    // `auto: false` keeps the old answer.
    let mut fresh = built();
    let e = fresh.execute("wholejob.estimate", &json!({"auto": false})).unwrap_err().to_string();
    assert!(e.contains("wholejob.quantities"), "{e}");
}

/// MUST 5: a starter cost table (CSV or xlsx) prices the estimate; unpriced lines point to it.
#[test]
fn a_cost_table_imports_from_csv() {
    let mut s = built();
    let dir = temp("costs");
    let csv = dir.join("costs.csv");
    std::fs::write(&csv, "Item,Unit,Material $,Labor Hrs\nGypsum board,sf,0.55,0.02\n\"Door, hollow metal\",each,350,2.5\nBlank row,each,,\n")
        .unwrap();
    let r = s.execute("estimate.import_costs", &json!({"path": csv.to_string_lossy(), "date": "2026-10-01"})).unwrap();
    assert_eq!(r["prices"], json!(2), "{r}");
    assert_eq!(r["skipped"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["source"], json!("costs.csv"));
    // Labour in dollars only: a note to set the crew rate.
    let usd = dir.join("usd.csv");
    std::fs::write(&usd, "item,unit,material,labor_dollars\nStud,lf,0.9,1.2\n").unwrap();
    let r = s.execute("estimate.import_costs", &json!({"path": usd.to_string_lossy()})).unwrap();
    assert_eq!(r["notes"].as_array().unwrap().len(), 1, "{r}");
    // A table with no material column says so.
    let bad = dir.join("bad.csv");
    std::fs::write(&bad, "item,unit\nX,each\n").unwrap();
    let e = s.execute("estimate.import_costs", &json!({"path": bad.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("material"), "{e}");
    assert!(s.execute("estimate.import_costs", &json!({"path": dir.join("none.csv").to_string_lossy()})).is_err());
    // The estimate's status points at the cost table while lines have no price.
    if let Ok(st) = s.execute("estimate.status", &json!({}))
        && st["summary"]["unpriced"].as_u64().unwrap_or(0) > 0
    {
        assert!(st["next_step"].as_str().unwrap().contains("estimate.import_costs"), "{st}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// MUST 6: issues become BCF topics (and a .bcf file) in one call.
#[test]
fn issues_go_to_bcf_in_one_call() {
    let mut s = plan();
    // A wall drawn twice, 2" apart: two walls overlapping.
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [15.0 + 2.0 / 12.0, 0.5], [15.0 + 2.0 / 12.0, 11.5]);
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("model.issues", &json!({"kind": "walls_overlap"})).unwrap();
    let n = r["total"].as_u64().unwrap();
    assert!(n >= 1, "{r}");
    let dir = temp("bcf");
    let path = dir.join("issues.bcf");
    let r = s.execute("model.issues", &json!({"kind": "walls_overlap", "bcf": true, "bcf_path": path.to_string_lossy()})).unwrap();
    assert_eq!(r["bcf"]["created"], json!(n), "{r}");
    assert!(path.exists() && std::fs::metadata(&path).unwrap().len() > 0);
    let list = s.execute("bcf.list", &json!({})).unwrap();
    assert_eq!(list["total"], json!(n), "{list}");
    // MUST 8: the same overlap is a same-trade clash.
    let c = s.execute("model.clashes", &json!({})).unwrap();
    let same: Vec<&Value> = c["clashes"].as_array().unwrap().iter().filter(|x| x["kind"] == "walls_overlap").collect();
    assert_eq!(same.len() as u64, n, "{c}");
    assert_eq!(same[0]["category"], json!("same_trade"));
    let off = s.execute("model.clashes", &json!({"same_trade": false})).unwrap();
    assert!(!off.to_string().contains("walls_overlap"), "{off}");
    let _ = std::fs::remove_dir_all(dir);
}

/// MUST 6: a few hundred rooms' issues come back quickly (the plan's outlines are made once).
#[test]
fn issues_on_many_rooms_are_quick() {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    let (nx, ny) = (20, 10);
    rect(&mut s, [0.0, 0.0], [f64::from(nx) * 10.0, f64::from(ny) * 10.0]);
    rect(&mut s, [0.5, 0.5], [f64::from(nx) * 10.0 - 0.5, f64::from(ny) * 10.0 - 0.5]);
    layer(&mut s, "A-WALL-CL");
    for i in 1..nx {
        line(&mut s, [f64::from(i) * 10.0, 0.5], [f64::from(i) * 10.0, f64::from(ny) * 10.0 - 0.5]);
    }
    for j in 1..ny {
        line(&mut s, [0.5, f64::from(j) * 10.0], [f64::from(nx) * 10.0 - 0.5, f64::from(j) * 10.0]);
    }
    layer(&mut s, "A-ROOM-TAG");
    for i in 0..nx {
        for j in 0..ny {
            text(&mut s, [f64::from(i) * 10.0 + 3.0, f64::from(j) * 10.0 + 5.0], if (i + j) % 2 == 0 { "DOUBLE Q" } else { "SINGLE Q" });
        }
    }
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}, {"layer": "A-WALL-CL", "role": "wall", "wall_type": "P"}, {"layer": "A-ROOM-TAG", "role": "room_tag"}],
            "wall_types": [{"name": "P", "thickness_in": 4.875, "from": "centerline"}]
        }}),
    )
    .unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let t = std::time::Instant::now();
    let r = s.execute("rooms.list", &json!({"outline": false})).unwrap();
    assert_eq!(r["count"], json!(nx * ny), "{}", r["count"]);
    let rooms_took = t.elapsed();
    let t = std::time::Instant::now();
    let i = s.execute("model.issues", &json!({})).unwrap();
    let issues_took = t.elapsed();
    eprintln!("{} rooms: rooms.list {rooms_took:?}, model.issues {issues_took:?} ({} issues)", nx * ny, i["total"]);
    assert_eq!(i["byKind"].get("leak"), None, "{i}");
    assert!(rooms_took.as_secs_f64() < 3.0 && issues_took.as_secs_f64() < 30.0, "{rooms_took:?} {issues_took:?}");
}

/// MUST 7: the built model is queryable as IFC elements in memory.
#[test]
fn the_built_model_lists_as_ifc_elements() {
    let mut s = built();
    let walls = s.doc().unwrap().model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("wall")).count();
    let r = s.execute("ifc.elements", &json!({"limit": 5000})).unwrap();
    assert!(r["built"].as_u64().unwrap() as usize > walls, "{}", r["built"]);
    assert_eq!(r["total"], r["built"]);
    assert!(r["export"].as_str().unwrap().contains(".ifc"));
    let w = s.execute("ifc.elements", &json!({"class": "IfcWall", "name": "model"})).unwrap();
    assert_eq!(w["total"].as_u64().unwrap() as usize, walls, "{w}");
    assert!(w["elements"].as_array().unwrap().iter().all(|e| e["ref"] == "model" && e["class"] == "Wall" && e["level"] == "L1"));
    let c = s.execute("ifc.elements", &json!({"class": "covering"})).unwrap();
    assert_eq!(c["total"], json!(3), "the three room ceilings: {c}");
    assert!(s.execute("ifc.elements", &json!({"name": "Nothing"})).is_err());
}

/// Hostile parameters to the new and changed commands never panic.
#[test]
fn hostile_params_never_panic() {
    let mut s = built();
    let long = "x".repeat(10_000);
    for (c, p) in [
        ("model.issues", json!({"bcf": "yes", "bcf_path": 5})),
        ("model.issues", json!({"bcf_path": ""})),
        ("model.clashes", json!({"same_trade": "no"})),
        ("mep.read_devices", json!({"limit": -1})),
        ("mep.read_devices", json!({"limit": 1e30})),
        ("unittypes.takeoff", json!({"level": long})),
        ("rcp.takeoff", json!({"level": 7})),
        ("ifc.elements", json!({"name": "model", "class": long, "limit": -5})),
        ("estimate.import_costs", json!({"path": long})),
        ("estimate.import_costs", json!({"path": null, "date": []})),
        ("wholejob.estimate", json!({"auto": "maybe"})),
        ("rooms.list", json!({"outline": "no"})),
        ("model.counts", json!({"x": 1})),
    ] {
        let _ = s.execute(c, &p);
    }
}
