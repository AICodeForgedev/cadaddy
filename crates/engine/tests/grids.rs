//! Structural grids: two floors drawn at different origins of the sheet align exactly from their
//! grid intersections (a third drawn at half size is reported, not moved), and the columns drawn
//! at the intersections become columns through the storey.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::EntityKind;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 8})).unwrap();
}

/// A floor at `o`, scaled `k`: grids A and B (x 0 and 300) bubbled below, 1 and 2 (y 0 and
/// 240) bubbled at the left; a wall line along grid 1; 12" columns at the four intersections.
fn floor(s: &mut Session, o: [f64; 2], k: f64) {
    let p = |x: f64, y: f64| [o[0] + x * k, o[1] + y * k];
    layer(s, "S-GRID");
    for (x, label) in [(0.0, "A"), (300.0, "B")] {
        line(s, p(x, -96.0 + 18.0), p(x, 300.0));
        s.execute("circle", &json!({"center": p(x, -96.0), "radius": 18.0 * k})).unwrap();
        text(s, p(x - 4.0, -100.0), label);
    }
    for (y, label) in [(0.0, "1"), (240.0, "2")] {
        line(s, p(-96.0 + 18.0, y), p(360.0, y));
        s.execute("circle", &json!({"center": p(-96.0, y), "radius": 18.0 * k})).unwrap();
        text(s, p(-100.0, y - 4.0), label);
    }
    layer(s, "S-COLS");
    for (x, y) in [(0.0, 0.0), (300.0, 0.0), (0.0, 240.0), (300.0, 240.0)] {
        let (a, b) = (p(x - 6.0, y - 6.0), p(x + 6.0, y + 6.0));
        s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
    }
    layer(s, "A-WALL");
    line(s, p(0.0, 0.0), p(300.0, 0.0));
}

fn drawing() -> Session {
    let mut s = Session::new();
    floor(&mut s, [0.0, 0.0], 1.0);
    floor(&mut s, [2100.0, 50.0], 1.0);
    floor(&mut s, [5000.0, 0.0], 0.5);
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 12, "region": [-200, -200, 1000, 1000]},
                {"name": "L2", "elevation_ft": 12, "height_ft": 12, "region": [1800, -200, 3000, 1000]},
                {"name": "L3", "elevation_ft": 24, "height_ft": 12, "region": [4800, -200, 6000, 1000]}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s
}

fn level<'a>(r: &'a Value, name: &str) -> &'a Value {
    r["levels"].as_array().unwrap().iter().find(|l| l["level"] == name).unwrap_or_else(|| panic!("{name}: {r}"))
}

#[test]
fn grids_are_found_with_their_intersections() {
    let mut s = drawing();
    let r = s.execute("grids.detect", &json!({"level": "L2"})).unwrap();
    let l2 = level(&r, "L2");
    let labels: Vec<&str> = l2["grids"].as_array().unwrap().iter().map(|g| g["label"].as_str().unwrap()).collect();
    assert_eq!(labels.len(), 4, "{r}");
    let xs: Vec<(String, Value)> =
        l2["intersections"].as_array().unwrap().iter().map(|x| (x["label"].as_str().unwrap().to_string(), x["at"].clone())).collect();
    assert_eq!(
        xs,
        vec![
            ("A-1".to_string(), json!([2100.0, 50.0])),
            ("A-2".to_string(), json!([2100.0, 290.0])),
            ("B-1".to_string(), json!([2400.0, 50.0])),
            ("B-2".to_string(), json!([2400.0, 290.0]))
        ]
    );
    assert!(s.execute("grids.detect", &json!({"level": "L9"})).is_err());
}

#[test]
fn two_floors_at_different_origins_align_exactly() {
    let mut s = drawing();
    let r = s.execute("levels.align", &json!({})).unwrap();
    assert_eq!(r["reference"], json!("L1"));
    let l2 = level(&r, "L2");
    assert_eq!(
        (l2["matched"].clone(), l2["scale"].clone(), l2["offset"].clone(), l2["residual"].clone()),
        (json!(4), json!(1.0), json!([-2100.0, -50.0]), json!(0.0))
    );
    assert_eq!(l2["base"], json!([2100.0, 50.0]));
    // The half-size floor is found at its scale and left alone.
    let l3 = level(&r, "L3");
    assert_eq!(l3["scale"], json!(2.0));
    assert!(l3["note"].as_str().unwrap().contains("scale"), "{l3}");
    assert!(l3.get("base").is_none());
    s.execute("levels.align", &json!({"apply": true})).unwrap();
    let sp = s.execute("bspec.get", &json!({})).unwrap()["spec"].clone();
    assert_eq!((sp["levels"][0]["base"].clone(), sp["levels"][1]["base"].clone()), (json!([0.0, 0.0]), json!([2100.0, 50.0])));
    assert!(sp["levels"][2]["base"].is_null());
    // Stacked: Walk mode's wall along grid 1 is in the same place on both floors.
    let spec = cadaddy_engine::walk3d::BuildingSpec::from_json(&sp.to_string()).unwrap();
    let scene = cadaddy_engine::walk3d::derive(s.doc().unwrap(), &spec.to_rules());
    let wall = |lv: usize| scene.walls.iter().find(|w| w.level == lv).map(|w| (w.a.x, w.a.y, w.b.x, w.b.y)).unwrap();
    assert_eq!(wall(0), wall(1));
    assert!(s.execute("levels.align", &json!({"reference": "L9"})).is_err());
}

#[test]
fn columns_at_the_intersections() {
    let mut s = drawing();
    let r = s.execute("grids.columns", &json!({"level": "L1", "to_level": "L2", "apply": true})).unwrap();
    let cols = r["columns"].as_array().unwrap();
    assert_eq!(cols.len(), 4, "{r}");
    assert!(cols.iter().all(|c| c["size_in"] == json!([12.0, 12.0]) && c["round"] == json!(false)));
    assert_eq!(cols[0]["grid"], json!("A-1"));
    let b = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(b["gridColumns"], json!(4), "{b}");
    let heights: Vec<(f64, f64)> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("column"))
        .filter_map(|e| {
            if let EntityKind::Mesh(m) = &e.kind {
                Some(m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.z), hi.max(v.z))))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(heights, vec![(0.0, 288.0); 4], "through L1 and L2");
    assert!(s.execute("grids.columns", &json!({"level": "L1", "to_level": "L9"})).is_err());
}
