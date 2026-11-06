//! The takeoff of a model drafted from a sheet that prints two floor plans (`3RD FLOOR PLAN`,
//! `4TH FLOOR PLAN`) has a line set per level, each with its elements (Q5242 MUST 3 on the
//! real-print gate's path: `bspec.draft` → `model.build` → `bim.takeoff {group_by: level}`;
//! the gate's `work.takeoff_levels`).

use cadaddy_engine::Session;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// The second plan's offset up the sheet (inches).
const UP: f64 = 1000.0;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn rect(s: &mut Session, x0: f64, y0: f64, x1: f64, y1: f64) {
    line(s, [x0, y0], [x1, y0]);
    line(s, [x1, y0], [x1, y1]);
    line(s, [x1, y1], [x0, y1]);
    line(s, [x0, y1], [x0, y0]);
}

/// A 40' × 30' floor at height `oy` on the sheet: 6" outside walls, a 4" partition with a 3'
/// door, a tagged room each side, and the view's title under it.
fn floor(s: &mut Session, oy: f64, number: usize, title: &str) {
    layer(s, "A-WALL");
    rect(s, 0.0, oy, 480.0, oy + 360.0);
    rect(s, 6.0, oy + 6.0, 474.0, oy + 354.0);
    for x in [238.0, 242.0] {
        line(s, [x, oy + 6.0], [x, oy + 150.0]);
        line(s, [x, oy + 186.0], [x, oy + 354.0]);
    }
    layer(s, "A-DOOR");
    s.execute("arc", &json!({"center": [240.0, oy + 150.0], "radius": 36.0, "start": 0.0, "end": 90.0})).unwrap();
    layer(s, "A-AREA-IDEN");
    for (x, name, n) in [(100.0, "OFFICE", 1), (340.0, "LOBBY", 2)] {
        s.execute("text", &json!({"at": [x, oy + 180.0], "text": name, "height": 8.0})).unwrap();
        s.execute("text", &json!({"at": [x, oy + 160.0], "text": format!("{number}0{n}"), "height": 6.0})).unwrap();
    }
    layer(s, "A-ANNO-TTLB");
    s.execute("text", &json!({"at": [0.0, oy - 40.0], "text": title, "height": 24.0})).unwrap();
}

fn lines(t: &Value) -> &Vec<Value> {
    t["lines"].as_array().unwrap()
}

fn elements(l: &Value) -> u64 {
    l["element_count"].as_u64().unwrap_or(0)
}

#[test]
fn a_sheet_of_two_floor_plans_takes_off_by_level() {
    let mut s = Session::new();
    floor(&mut s, 0.0, 3, "3RD FLOOR PLAN");
    floor(&mut s, UP, 4, "4TH FLOOR PLAN");
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let t = s.execute("bim.takeoff", &json!({"group_by": "level"})).unwrap();
    // The gate's count: the distinct levels the lines name.
    let levels: BTreeSet<&str> = lines(&t).iter().filter_map(|l| l["level"].as_str()).filter(|l| !l.is_empty()).collect();
    assert_eq!(levels.iter().copied().collect::<Vec<_>>(), ["Level 3", "Level 4"], "{t}");
    for level in ["Level 3", "Level 4"] {
        let on: Vec<&Value> = lines(&t).iter().filter(|l| l["level"] == level).collect();
        // Walls from the plan's linework, with the walls as elements.
        let wall = on.iter().find(|l| l["item"] == "Wall" && l["unit"] == "lf").unwrap_or_else(|| panic!("{level} wall line: {t}"));
        assert!(elements(wall) >= 4, "{level}: {wall}");
        assert!(on.iter().filter(|l| elements(l) > 0).count() >= 2, "{level}: {on:?}");
    }
    assert!(lines(&t).iter().filter(|l| elements(l) > 0).count() >= 4, "{t}");
}
