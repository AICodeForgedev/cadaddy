//! COMPARE compares model space and every layout at once and lists differences per layout;
//! COMPAREEXPORT writes the comparison as an HTML report.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

/// "Drawing1": a line in model space, a circle on Layout1. Then a copy "B" opened and edited: a
/// second line in model space, the circle moved, a layout "Sheet" only in B, and Layout2 deleted.
fn compared() -> (Session, Value) {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    s.execute("circle", &json!({"center": [5, 5], "radius": 1})).unwrap();
    s.execute("layout.set", &json!({"name": "Model"})).unwrap();
    let d: Drawing = s.doc().unwrap().clone();
    s.open_drawing(d, "B", None);
    s.execute("line", &json!({"points": [[0, 5], [10, 5]]})).unwrap();
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    let circle = s.doc().unwrap().layout("Layout1").unwrap().entities.iter().find(|e| e.kind.type_name() == "Circle").unwrap().handle.hex();
    s.execute("move", &json!({"handles": [circle], "from": [0, 0], "to": [2, 0]})).unwrap();
    s.execute("layout.new", &json!({"name": "Sheet", "viewport": false})).unwrap();
    s.execute("layout.delete", &json!({"name": "Layout2"})).unwrap();
    s.execute("layout.set", &json!({"name": "Model"})).unwrap();
    let r = s.execute("compare", &json!({"document": "drawing1"})).unwrap();
    (s, r)
}

fn layout<'a>(r: &'a Value, name: &str) -> &'a Value {
    r["layouts"].as_array().unwrap().iter().find(|l| l["layout"] == name).unwrap_or_else(|| panic!("no {name} in {r}"))
}

#[test]
fn compare_lists_differences_per_layout() {
    let (_, r) = compared();
    // The tab shown (Model) is still the top-level result.
    assert_eq!(r["counts"]["added"], 1, "{r}");
    let names: Vec<&str> = r["layouts"].as_array().unwrap().iter().map(|l| l["layout"].as_str().unwrap()).collect();
    assert_eq!(names, ["Model", "Layout1", "Sheet", "Layout2"], "model first, this drawing's tabs, then the other's");
    assert_eq!(layout(&r, "Model")["counts"]["added"], 1);
    // The moved circle on Layout1 is a change in that layout.
    let l1 = layout(&r, "Layout1");
    assert_eq!(l1["counts"]["changed"], 1, "{l1}");
    assert_eq!(l1["changed"][0]["type"], "Circle");
    assert_eq!(l1["changed"][0]["what"], "geometry");
    // A layout only one drawing has.
    assert_eq!(layout(&r, "Sheet")["onlyIn"], "this");
    assert_eq!(layout(&r, "Layout2")["onlyIn"], "other");
    assert_eq!(layout(&r, "Layout1")["onlyIn"], Value::Null);
    // Totals over every layout.
    assert_eq!(r["total"]["changed"], 1);
    assert!(r["total"]["added"].as_u64().unwrap() >= 1);
    let msg = r["message"].as_str().unwrap();
    assert!(msg.contains("Layout1 (1)") && msg.contains("Model (1)"), "{msg}");
}

#[test]
fn compare_export_writes_an_html_report() {
    let (mut s, _) = compared();
    // Without a path the report comes back.
    let r = s.execute("compareexport", &json!({})).unwrap();
    let html = r["html"].as_str().unwrap();
    assert!(html.starts_with("<!DOCTYPE html>"));
    for col in
        ["<th>Layout</th>", "<th>Status</th>", "<th>Object</th>", "<th>Handle</th>", "<th>Other handle</th>", "<th>Layer</th>", "<th>Change</th>"]
    {
        assert!(html.contains(col), "{col}");
    }
    assert!(html.contains("Compared with: <b>Drawing1</b>"));
    assert!(html.contains("<td>Layout1</td><td>Changed</td><td>Circle</td>"), "{html}");
    assert!(html.contains("<td>Model</td><td>Added</td><td>Line</td>"), "{html}");
    assert!(html.contains("Layout2 (only in the other drawing)"));
    assert!(r["rows"].as_u64().unwrap() >= 2);

    // To a file.
    let dir = std::env::temp_dir().join(format!("cadaddy-compare-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("report.html");
    let w = s.execute("compareexport", &json!({ "path": path.to_string_lossy() })).unwrap();
    assert_eq!(w["rows"], r["rows"]);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), html);
    std::fs::remove_dir_all(&dir).ok();

    // Names are escaped.
    let mut s2 = Session::new();
    s2.execute("layout.new", &json!({"name": "R&D 'A'", "viewport": false})).unwrap();
    let d: Drawing = s2.doc().unwrap().clone();
    s2.open_drawing(d, "C", None);
    s2.execute("layout.set", &json!({"name": "R&D 'A'"})).unwrap();
    s2.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
    s2.execute("compare", &json!({"document": "drawing1"})).unwrap();
    let html = s2.execute("compareexport", &json!({})).unwrap()["html"].as_str().unwrap().to_string();
    assert!(html.contains("R&amp;D &#39;A&#39;") && !html.contains("R&D 'A'"), "escaped");

    // Without a comparison it is an error; COMPARECLOSE ends it.
    s.execute("compareclose", &json!({})).unwrap();
    assert!(s.execute("compareexport", &json!({})).is_err());
}
