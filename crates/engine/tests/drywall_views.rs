//! Drywall views and takeoff tests (Q294): wall inspector, view modes, finish faces and drywall
//! takeoff, using the Q293 wall assembly fixtures.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

/// A made-up building (same as wall_assemblies.rs) with wall types and a build.
fn building() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    let t = 7.25;
    rect(&mut s, [0.0, 0.0], [480.0, 288.0]);
    rect(&mut s, [t, t], [480.0 - t, 288.0 - t]);
    let h = 4.875 / 2.0;
    line(&mut s, [240.0 - h, t], [240.0 - h, 288.0 - t]);
    line(&mut s, [240.0 + h, t], [240.0 + h, 288.0 - t]);
    let layers = |side1: Value, core: Value, side2: Value| -> Value {
        let mut v: Vec<Value> = Vec::new();
        for (side, list) in [("side1", side1), ("core", core), ("side2", side2)] {
            for l in list.as_array().cloned().unwrap_or_default() {
                let mut l = l;
                l["side"] = json!(side);
                v.push(l);
            }
        }
        json!(v)
    };
    let exterior = json!({
        "tag": "X1", "description": "EXTERIOR WALL", "height": "full", "external": true,
        "layers": layers(
            json!([{"material": "5/8\" exterior sheathing", "thickness_in": 0.625, "category": "sheathing"}]),
            json!([{"material": "6\" 18 ga metal studs @ 16\" o.c.", "thickness_in": 6.0, "category": "framing"},
                   {"material": "6\" batt insulation", "thickness_in": 6.0, "category": "insulation", "in_cavity": true}]),
            json!([{"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "category": "board"}]))
    });
    let partition = json!({
        "tag": "P1", "description": "1 HOUR RATED CORRIDOR PARTITION", "height": "deck", "fire_rating": "1 HR", "acoustic_rating": "STC 45",
        "layers": layers(
            json!([{"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "category": "board"}]),
            json!([{"material": "3 5/8\" 20 ga metal studs @ 16\" o.c.", "thickness_in": 3.625, "category": "framing"},
                   {"material": "3 1/2\" batt insulation", "thickness_in": 3.5, "category": "insulation", "in_cavity": true}]),
            json!([{"material": "5/8\" Type X gypsum board", "thickness_in": 0.625, "category": "board"}]))
    });
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}],
        "wall_types": [
            {"name": "X1", "thickness_in": 7.25, "from": "pairs", "assembly": exterior},
            {"name": "P1", "thickness_in": 4.875, "from": "pairs", "assembly": partition}
        ],
        "walls": [{"level": "1", "a": [240.0, 7.25], "b": [240.0, 280.75], "wall_type": "P1", "height_ft": 12.75}],
        "rooms": [
            {"name": "OFFICE", "level": "1", "materials": {"wall": "latex paint"}},
            {"name": "CORRIDOR", "level": "1", "materials": {"wall": "epoxy paint"}}
        ]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// Select all wall entities.
fn select_walls(s: &mut Session) -> Vec<String> {
    let d = s.doc().unwrap();
    let handles: Vec<String> =
        d.model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("wall")).map(|e| e.handle.hex()).collect();
    handles
}

// --------------------------------------------------------------------------
// wall.info
// --------------------------------------------------------------------------

#[test]
fn wall_info_returns_assembly_for_typed_walls() {
    let mut s = building();
    let handles = select_walls(&mut s);
    assert!(!handles.is_empty(), "building should have wall meshes");
    let r = s.execute("wall.info", &json!({"handles": handles})).unwrap();
    let walls = r["walls"].as_array().unwrap();
    assert!(!walls.is_empty(), "wall.info should return wall data");

    // At least one wall should have an assembly with layers.
    let has_assembly = walls.iter().any(|w| w["assembly"]["layers"].is_array());
    assert!(has_assembly, "at least one wall should have an assembly");

    // Check that the X1 wall has the exterior flag.
    let x1_walls: Vec<&Value> = walls.iter().filter(|w| w["wallType"] == "X1").collect();
    if !x1_walls.is_empty() {
        assert_eq!(x1_walls[0]["assembly"]["external"], true);
        assert!(x1_walls[0]["grossAreaSqFt"].as_f64().unwrap_or(0.0) > 0.0, "X1 wall should have area");
    }

    // Check that the P1 wall has a fire rating.
    let p1_walls: Vec<&Value> = walls.iter().filter(|w| w["wallType"] == "P1").collect();
    if !p1_walls.is_empty() {
        assert_eq!(p1_walls[0]["assembly"]["fireRating"], "1 HR");
        assert_eq!(p1_walls[0]["assembly"]["acousticRating"], "STC 45");
    }
}

#[test]
fn wall_info_no_selection_returns_empty() {
    let mut s = Session::new();
    let r = s.execute("wall.info", &json!({})).unwrap();
    assert_eq!(r["count"], 0);
}

// --------------------------------------------------------------------------
// drywall.viewmode
// --------------------------------------------------------------------------

#[test]
fn viewmode_by_type() {
    let mut s = building();
    let r = s.execute("drywall.viewmode", &json!({"mode": "type"})).unwrap();
    assert_eq!(r["mode"], "type");
    let legend = r["legend"].as_array().unwrap();
    assert!(legend.len() >= 2, "should have at least 2 wall types in legend");
    // Each legend entry has a label and a color.
    for item in legend {
        assert!(item["label"].is_string());
        assert!(item["color"].is_array());
    }
}

#[test]
fn viewmode_by_rating() {
    let mut s = building();
    let r = s.execute("drywall.viewmode", &json!({"mode": "rating"})).unwrap();
    assert_eq!(r["mode"], "rating");
    let legend = r["legend"].as_array().unwrap();
    // Should have "1 HR" and "Not rated" (exterior has no fire rating).
    let labels: Vec<&str> = legend.iter().filter_map(|l| l["label"].as_str()).collect();
    assert!(labels.contains(&"1 HR"), "legend should contain '1 HR', got {:?}", labels);
    assert!(labels.contains(&"Not rated"), "legend should contain 'Not rated', got {:?}", labels);
}

#[test]
fn viewmode_by_board_layers() {
    let mut s = building();
    let r = s.execute("drywall.viewmode", &json!({"mode": "boardLayers"})).unwrap();
    assert_eq!(r["mode"], "boardLayers");
    let legend = r["legend"].as_array().unwrap();
    assert!(!legend.is_empty());
}

#[test]
fn viewmode_by_height_rule() {
    let mut s = building();
    let r = s.execute("drywall.viewmode", &json!({"mode": "heightRule"})).unwrap();
    assert_eq!(r["mode"], "heightRule");
}

#[test]
fn viewmode_off_clears() {
    let mut s = building();
    s.execute("drywall.viewmode", &json!({"mode": "type"})).unwrap();
    assert!(s.drywall_viewmode.is_some());
    s.execute("drywall.viewmode", &json!({"mode": "off"})).unwrap();
    assert!(s.drywall_viewmode.is_none());
}

// --------------------------------------------------------------------------
// drywall.takeoff
// --------------------------------------------------------------------------

#[test]
fn takeoff_returns_board_data() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({})).unwrap();
    let rows = r["rows"].as_array().unwrap();
    assert!(!rows.is_empty(), "takeoff should produce rows");

    // Check totals.
    let totals = &r["totals"];
    assert!(totals["boardAreaSqFt"].as_f64().unwrap_or(0.0) > 0.0, "should have board area");
    assert!(totals["sheets"].as_u64().unwrap_or(0) > 0, "should have sheets");
    assert!(totals["studLengthFt"].as_f64().unwrap_or(0.0) > 0.0, "should have stud length");
    assert!(totals["trackLengthFt"].as_f64().unwrap_or(0.0) > 0.0, "should have track length");
}

#[test]
fn takeoff_group_by_level() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"groupBy": "level"})).unwrap();
    let rows = r["rows"].as_array().unwrap();
    assert!(!rows.is_empty());
    // All groups should be level names.
    for row in rows {
        assert!(row["group"].is_string());
    }
}

#[test]
fn takeoff_custom_waste() {
    let mut s = building();
    let r = s.execute("drywall.takeoff", &json!({"waste": 15})).unwrap();
    assert_eq!(r["waste"], 15.0);
}

#[test]
fn takeoff_no_building_spec_errors() {
    let mut s = Session::new();
    let r = s.execute("drywall.takeoff", &json!({}));
    assert!(r.is_err(), "takeoff without a building spec should error");
}

// --------------------------------------------------------------------------
// drywall.finishfaces
// --------------------------------------------------------------------------

#[test]
fn finishfaces_returns_board_per_side() {
    let mut s = building();
    let r = s.execute("drywall.finishfaces", &json!({})).unwrap();
    let faces = r["faces"].as_array().unwrap();
    assert!(!faces.is_empty(), "should have finish faces");
    for face in faces {
        assert!(face["wallHandle"].is_string());
        assert!(face["side"].is_string());
        assert!(face["board"].is_string());
    }
    // Each wall should produce two faces (one per side).
    let count = r["count"].as_u64().unwrap_or(0);
    assert!(count >= 2, "at least one wall with two faces, got {count}");
}

// --------------------------------------------------------------------------
// hostile parameters
// --------------------------------------------------------------------------

#[test]
fn hostile_params_never_panic() {
    let mut s = building();
    // Bad mode.
    assert!(s.execute("drywall.viewmode", &json!({"mode": "badmode"})).is_err());
    // Missing mode.
    let _ = s.execute("drywall.viewmode", &json!({}));
    // Bad waste.
    let _ = s.execute("drywall.takeoff", &json!({"waste": -100}));
    let _ = s.execute("drywall.takeoff", &json!({"waste": 99999}));
    // Bad handles.
    let _ = s.execute("wall.info", &json!({"handles": ["ZZZZ"]}));
    let _ = s.execute("wall.info", &json!({"handles": []}));
}
