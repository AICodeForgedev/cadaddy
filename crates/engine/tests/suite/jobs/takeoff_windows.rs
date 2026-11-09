//! Every built window is taken off (Q6371): `bim.bom`, `bim.takeoff` and `bim.estimate` carry
//! the windows per level (count and area) beside the doors, each line with its own windows, and
//! a cost row prices them.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// The second floor's plan, this far up the sheet (feet).
const UP: f64 = 100.0;

fn line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = layer.into();
    d.model.push(e);
}

/// A 30' × 12' floor of 6" walls at `oy` (feet): its south wall has `n` 4' openings (from x 4',
/// 8' apart), each with its jambs and its glazing drawn across it.
fn floor(d: &mut Drawing, oy: f64, n: usize) {
    let gaps: Vec<(f64, f64)> = (0..n).map(|i| (4.0 + 8.0 * i as f64, 8.0 + 8.0 * i as f64)).collect();
    for (y, x0, x1) in [(0.0, 0.0, 30.0), (0.5, 0.5, 29.5)] {
        let mut at = x0;
        for (a, b) in &gaps {
            line(d, "A-WALL", (at, oy + y), (*a, oy + y));
            at = *b;
        }
        line(d, "A-WALL", (at, oy + y), (x1, oy + y));
    }
    for (a, b) in &gaps {
        for x in [*a, *b] {
            line(d, "A-WALL", (x, oy), (x, oy + 0.5));
        }
        for y in [0.2, 0.3] {
            line(d, "A-GLAZ", (*a, oy + y), (*b, oy + y));
        }
    }
    for (a, b) in [((30., 0.), (30., 12.)), ((30., 12.), (0., 12.)), ((0., 12.), (0., 0.))] {
        line(d, "A-WALL", (a.0, oy + a.1), (b.0, oy + b.1));
    }
    for (a, b) in [((29.5, 0.5), (29.5, 11.5)), ((29.5, 11.5), (0.5, 11.5)), ((0.5, 11.5), (0.5, 0.5))] {
        line(d, "A-WALL", (a.0, oy + a.1), (b.0, oy + b.1));
    }
}

/// Two floors, 2 windows on the first and 3 on the second, built.
fn building() -> Session {
    let mut d = Drawing::new_imperial();
    floor(&mut d, 0.0, 2);
    floor(&mut d, UP, 3);
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let r = |oy: f64| json!([-120.0, (oy - 10.0) * 12.0, 480.0, (oy + 30.0) * 12.0]);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": r(0.0), "base": [0, 0]},
            {"name": "L2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9, "region": r(UP), "base": [0, UP * 12.0]},
        ],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"}, {"layer": "A-GLAZ", "role": "window"}],
        "wall_types": [{"name": "Exterior", "thickness_in": 6}],
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

fn lines(v: &Value) -> Vec<&Value> {
    v["lines"].as_array().map(|a| a.iter().collect()).unwrap_or_default()
}

fn windows_of(s: &Session) -> Vec<(String, String)> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("window"))
        .map(|e| (e.handle.hex(), cadaddy_engine::cmd::bspec::made_level(e).unwrap_or_default()))
        .collect()
}

#[test]
fn every_built_window_is_taken_off_per_level_with_its_area() {
    let mut s = building();
    let count = s.execute("bim.count", &json!({"kind": "window", "group_by": "level"})).expect("count");
    assert_eq!(count["count"], json!(5), "{count}");
    let built = windows_of(&s);
    assert_eq!(built.len(), 5, "{built:?}");
    let area = count["area_sf"].as_f64().unwrap_or(0.0);
    assert!(area > 0.0, "the windows have their area: {count}");

    // The BOM: a window line per level, counting that level's windows.
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    let bw: Vec<&Value> = lines(&bom).into_iter().filter(|l| l["category"] == "windows").collect();
    let each: f64 = bw.iter().filter(|l| l["unit"] == "each").filter_map(|l| l["qty"].as_f64().or(l["net"].as_f64())).sum();
    assert_eq!(each, 5.0, "the BOM counts every window: {bw:?}");

    // The takeoff: per level, count and area, each line with exactly its level's windows.
    let t = s.execute("bim.takeoff", &json!({"group_by": "level"})).expect("takeoff");
    for (level, n) in [("L1", 2usize), ("L2", 3)] {
        let on: Vec<&Value> = lines(&t).into_iter().filter(|l| l["category"] == "windows" && l["level"] == level).collect();
        let count: f64 = on.iter().filter(|l| l["unit"] == "each").filter_map(|l| l["qty"].as_f64()).sum();
        assert_eq!(count, n as f64, "{level} windows counted: {on:?}");
        let sf: f64 = on.iter().filter(|l| l["unit"] == "sq ft").filter_map(|l| l["qty"].as_f64()).sum();
        assert!(sf > 0.0, "{level} window area: {on:?}");
        let mine: Vec<&String> = built.iter().filter(|(_, lv)| lv == level).map(|(h, _)| h).collect();
        for l in on.iter().filter(|l| l["unit"] == "each") {
            let els: Vec<&str> = l["elements"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            assert!(!els.is_empty() && els.iter().all(|h| mine.iter().any(|m| m.as_str() == *h)), "{level}: {l}");
        }
    }
    let total_sf: f64 = lines(&t).into_iter().filter(|l| l["category"] == "windows" && l["unit"] == "sq ft").filter_map(|l| l["qty"].as_f64()).sum();
    assert!((total_sf - area).abs() < 0.5, "the takeoff's window area is the model's ({area}): {total_sf}");
    let only = s.execute("bim.takeoff", &json!({"filter": {"category": "windows"}, "keep": false})).expect("filtered");
    assert!(!lines(&only).is_empty(), "{only}");

    // The estimate: a cost row prices the windows like the doors.
    let est = s
        .execute(
            "bim.estimate",
            &json!({"rows": [{"item": "Window", "unit": "each", "material": 400.0, "labor_hrs": 2.0}, {"item": "Window area", "unit": "sq ft", "material": 0.0, "labor_hrs": 0.0}]}),
        )
        .expect("estimate");
    let priced: f64 = lines(&est).into_iter().filter(|l| l["cost_item"] == "Window").filter_map(|l| l["qty"].as_f64()).sum();
    assert_eq!(priced, 5.0, "{est}");
    let unmatched: Vec<&Value> = est["unmatched"]
        .as_array()
        .map(|a| a.iter().filter(|l| l["item"].as_str().is_some_and(|i| i.starts_with("Window"))).collect())
        .unwrap_or_default();
    assert!(unmatched.is_empty(), "{unmatched:?}");

    // With no cost table of the job's own, the starter book prices each window like each door,
    // and no row of another element (a wall's area) prices the windows' area.
    let est = s.execute("bim.estimate", &json!({})).expect("starter estimate");
    let of =
        |item: &str| lines(&est).into_iter().filter(|l| l["item"] == item).map(|l| (l["cost_item"].clone(), l["qty"].clone())).collect::<Vec<_>>();
    assert_eq!(of("Window"), vec![(json!("Window"), json!(5.0))], "{est}");
    assert!(of("Window area").iter().all(|(row, _)| row.as_str().is_some_and(|r| r.starts_with("Window"))), "{est}");
    assert!(!of("Door").is_empty() || lines(&t).iter().all(|l| l["category"] != "doors"), "{est}");
    for l in lines(&est) {
        let (row, cat) = (l["cost_item"].as_str().unwrap_or_default().to_ascii_lowercase(), l["category"].as_str().unwrap_or_default());
        assert!(!(cat == "windows" && row.contains("wall")), "a window line priced as a wall: {l}");
    }
}

/// Window blocks of a spec family set in walls drawn through: each a window, taken off.
#[test]
fn window_blocks_of_a_family_are_taken_off() {
    use cadaddy_engine::doc::{Block, Insert};
    use std::sync::Arc;
    let mut d = Drawing::new_imperial();
    for (x0, y0, x1, y1) in [(0.0, 0.0, 30.0, 12.0), (0.5, 0.5, 29.5, 11.5)] {
        for (a, b) in [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))] {
            line(&mut d, "A-WALL", a, b);
        }
    }
    let mut b = Block::new("WIN-48");
    let h = d.new_handle();
    b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3 { x: -24.0, y: 0.0, z: 0.0 }, b: Vec3 { x: 24.0, y: 0.0, z: 0.0 } })));
    d.blocks.insert("WIN-48".into(), Arc::new(b));
    for x in [5.0, 13.0, 21.0] {
        let ins: Insert = serde_json::from_value(json!({"block": "WIN-48", "insert": {"x": x * 12.0, "y": 3.0, "z": 0.0}})).expect("insert");
        let h = d.new_handle();
        let mut e = Entity::new(h, EntityKind::Insert(ins));
        e.common.layer = "A-GLAZ".into();
        d.model.push(e);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"}, {"layer": "A-GLAZ", "role": "window"}],
        "wall_types": [{"name": "Exterior", "thickness_in": 6}],
        "families": [{"name": "Window", "kind": "window", "block": "WIN*", "sill_ft": 3, "head_ft": 7}],
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let count = s.execute("bim.count", &json!({"kind": "window"})).expect("count");
    assert_eq!(count["count"], json!(3), "{count}");
    let t = s.execute("bim.takeoff", &json!({"filter": {"category": "windows"}, "keep": false})).expect("takeoff");
    let each: f64 = lines(&t).into_iter().filter(|l| l["unit"] == "each").filter_map(|l| l["qty"].as_f64()).sum();
    let sf: f64 = lines(&t).into_iter().filter(|l| l["unit"] == "sq ft").filter_map(|l| l["qty"].as_f64()).sum();
    assert_eq!(each, 3.0, "{t}");
    assert!(sf > 0.0 && (sf - count["area_sf"].as_f64().unwrap_or(0.0)).abs() < 0.5, "{sf}: {count} {t}");
}
