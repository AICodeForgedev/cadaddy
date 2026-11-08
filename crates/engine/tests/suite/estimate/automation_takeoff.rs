//! The Automation tab's Takeoff (`automation.takeoff`, Q6117): with a built BIM model the model's
//! takeoff per level, trade and type; without one the 2D takeoff by layer; written as CSV or XLSX
//! through the output guard. Made-up drawings only.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, x0: f64, y0: f64, x1: f64, y1: f64) {
    s.execute("pline", &json!({"vertices": [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], "closed": true})).unwrap();
}

/// A made-up 30' × 12' plan: outside walls, a tagged office, built into a model.
fn modelled() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, 0.0, 0.0, 360.0, 144.0);
    rect(&mut s, 6.0, 6.0, 354.0, 138.0);
    layer(&mut s, "A-AREA-IDEN");
    s.execute("text", &json!({"at": [120.0, 72.0], "text": "OFFICE", "height": 8.0})).unwrap();
    s.execute("text", &json!({"at": [120.0, 56.0], "text": "101", "height": 6.0})).unwrap();
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-automation-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rows(r: &Value) -> Vec<Vec<String>> {
    r["rows"].as_array().unwrap().iter().map(|row| row.as_array().unwrap().iter().map(|c| c.as_str().unwrap().to_string()).collect()).collect()
}

#[test]
fn with_a_model_it_is_the_models_takeoff_per_level_trade_and_type() {
    let mut s = modelled();
    let r = s.execute("automation.takeoff", &json!({})).unwrap();
    assert_eq!(r["source"], "model", "{r}");
    assert_eq!(r["command"], "bim.takeoff");
    let cols: Vec<&str> = r["columns"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(cols, ["Level", "Trade", "System", "Category", "Type", "Size", "Unit", "Qty", "Elements"]);
    let rows = rows(&r);
    assert!(!rows.is_empty() && rows.len() == r["total_rows"].as_u64().unwrap() as usize, "{r}");
    assert!(rows.iter().all(|row| row.len() == cols.len()));
    // The same lines bim.takeoff gives, in its order.
    let t = s.execute("bim.takeoff", &json!({"group_by": "level", "keep": false})).unwrap();
    let lines = t["lines"].as_array().unwrap();
    assert_eq!(lines.len(), rows.len());
    for (l, row) in lines.iter().zip(&rows) {
        assert_eq!(l["level"].as_str().unwrap_or_default(), row[0]);
        assert_eq!(l["trade"].as_str().unwrap_or_default(), row[1]);
        assert_eq!(l["item"].as_str().unwrap_or_default(), row[4]);
    }
    assert!(rows.iter().any(|row| row[3].contains("wall")), "walls are taken off: {rows:?}");
    assert!(r["by_trade"].as_object().is_some_and(|o| !o.is_empty()), "{r}");
    // It reports; it keeps nothing in the drawing.
    assert!(r.get("note").is_none());
}

#[test]
fn without_a_model_it_is_the_2d_takeoff_by_layer() {
    let mut s = Session::new();
    layer(&mut s, "E-POWR");
    s.execute("line", &json!({"points": [[0, 0], [120, 0]]})).unwrap();
    s.execute("line", &json!({"points": [[0, 10], [0, 70]]})).unwrap();
    let r = s.execute("automation.takeoff", &json!({})).unwrap();
    assert_eq!(r["source"], "drawing", "{r}");
    assert_eq!(r["command"], "takeoff");
    assert!(r["note"].as_str().is_some_and(|n| n.contains("no BIM model")), "{r}");
    let rows = rows(&r);
    let powr = rows.iter().find(|row| row[0] == "E-POWR").unwrap_or_else(|| panic!("{rows:?}"));
    assert_eq!(powr[1], "2");
    assert_eq!(powr[2], "180");
    assert_eq!(rows.last().map(|row| row[0].as_str()), Some("Total"));
}

#[test]
fn it_exports_csv_and_xlsx_through_the_output_guard() {
    let d = dir("export");
    let mut s = modelled();
    let csv = d.join("Takeoff A.csv");
    let r = s.execute("automation.takeoff", &json!({"path": csv.to_string_lossy()})).unwrap();
    assert_eq!(r["format"], "csv");
    let text = std::fs::read_to_string(&csv).unwrap();
    assert!(text.starts_with("Level,Trade,System,Category,Type,Size,Unit,Qty,Elements\n"), "{text}");
    assert_eq!(text.lines().count(), r["total_rows"].as_u64().unwrap() as usize + 1);
    // An existing file is refused without force, replaced with it.
    let e = s.execute("automation.takeoff", &json!({"path": csv.to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("force"), "{e}");
    s.execute("automation.takeoff", &json!({"path": csv.to_string_lossy(), "force": true})).unwrap();
    // XLSX by format (the extension is added).
    let base = d.join("Takeoff B");
    let r = s.execute("automation.takeoff", &json!({"path": base.to_string_lossy(), "format": "xlsx"})).unwrap();
    let path = r["path"].as_str().unwrap();
    assert!(path.ends_with("Takeoff B.xlsx"), "{r}");
    let bytes = std::fs::read(path).unwrap();
    assert!(bytes.starts_with(b"PK"), "an XLSX is a zip");
    // Never a drawing's name, never another kind of file.
    let e = s.execute("automation.takeoff", &json!({"path": d.join("Takeoff.dwg").to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains("drawing file name"), "{e}");
    let e = s.execute("automation.takeoff", &json!({"path": d.join("Takeoff.pdf").to_string_lossy()})).unwrap_err().to_string();
    assert!(e.contains(".csv or .xlsx"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}

/// Q6271: a two-level model whose ducts name their levels as the model does (`Level 1`), by a
/// bare number (`2` for `Level 2`), and by a name no level of the model has. Every row is on one
/// of the model's levels, as the model names it (the `2` duct joins Level 2 with its elements);
/// the duct on no level of the model is under the explicit unassigned group, counted; no row is
/// on a raw number or a blank.
#[test]
fn every_row_is_on_one_of_the_models_levels_or_the_unassigned_group() {
    let mut s = Session::new();
    let duct = |level: &str, y: f64| json!({"system": "SA", "level": level, "points": [[0, y], [240, y]], "size_in": [12, 8], "bottoms_ft": [9]});
    let spec = |runs: Vec<Value>| {
        json!({"replace": true, "spec": {
            "levels": [
                {"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9},
                {"name": "Level 2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9}
            ],
            "mep": {
                "systems": [{"name": "SA", "layer": "M-DUCT-SUPP", "kind": "duct", "size_in": [12, 8], "bottom_ft": 9, "duct_type": "supply"}],
                "runs": runs
            }
        }})
    };
    let built = vec![duct("Level 1", 0.0), duct("Level 2", 100.0), duct("2", 200.0)];
    s.execute("bspec.set", &spec(built.clone())).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    // A run added later on a level the model doesn't have (the spec keeps it, flagged).
    s.execute("bspec.set", &spec([built, vec![duct("Penthouse Z", 300.0)]].concat())).unwrap();
    let r = s.execute("automation.takeoff", &json!({})).unwrap();
    assert_eq!(r["source"], "model", "{r}");
    let rows = rows(&r);
    let levels: Vec<&str> = r["levels"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(levels, ["(unassigned level)", "Level 1", "Level 2"], "{rows:?}");
    assert!(rows.iter().all(|row| ["Level 1", "Level 2", "(unassigned level)"].contains(&row[0].as_str())), "{rows:?}");
    // The `2` duct is Level 2's: one line of its kind there, both runs' length, with elements.
    let t = s.execute("bim.takeoff", &json!({"group_by": "level", "keep": false})).unwrap();
    let ducts = |level: &str| -> Vec<Value> {
        t["lines"].as_array().unwrap().iter().filter(|l| l["level"] == level && l["unit"] == "ft" && l["category"] == "duct").cloned().collect()
    };
    let (one, two) = (ducts("Level 1"), ducts("Level 2"));
    assert_eq!((one.len(), two.len()), (1, 1), "{t}");
    assert!((two[0]["qty"].as_f64().unwrap() - 2.0 * one[0]["qty"].as_f64().unwrap()).abs() < 0.01, "{two:?} {one:?}");
    assert!(two[0]["element_count"].as_u64().unwrap() >= one[0]["element_count"].as_u64().unwrap() && one[0]["element_count"] != 0, "{two:?}");
    // The duct on a level the model doesn't have: unassigned, counted.
    let lost = ducts("(unassigned level)");
    assert_eq!(lost.len(), 1, "{t}");
    assert_eq!(r["unassigned_level"]["level"], "(unassigned level)", "{r}");
    assert!(r["unassigned_level"]["lines"].as_u64().unwrap() >= 1, "{r}");
    assert_eq!(t["unassigned_level"], r["unassigned_level"]);
}
