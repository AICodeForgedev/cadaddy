//! An elevator's hoistway, pit and overrun built from its documents (Q6608): the shaft at the
//! clear size the submittal gives, where the plan's shaft walls are, never along the car outline
//! drawn inside it; a side the plan draws no wall on walled in the shaft's type; the pit below
//! the lowest landing to its depth; the overrun above the roof to the overhead, the roof unit
//! over it standing on its roof (its design base kept beside).

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// A line in inches on `layer`.
fn line_in(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: Vec3 { x: a.0, y: a.1, z: 0.0 }, b: Vec3 { x: b.0, y: b.1, z: 0.0 } }));
    e.common.layer = layer.into();
    d.model.push(e);
}

/// The sides of the rectangle `lo..hi` (inches) whose `keep` is true (south, east, north, west).
fn sides(d: &mut Drawing, layer: &str, (x0, y0): (f64, f64), (x1, y1): (f64, f64), keep: [bool; 4]) {
    let all = [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))];
    for ((a, b), k) in all.into_iter().zip(keep) {
        if k {
            line_in(d, layer, a, b);
        }
    }
}

/// Plans side by side: storey `i`'s at x = 3000 i.
const STEP: f64 = 3000.0;
/// The shaft's clear box on each plan (inches, before the plan's x offset): 7'-4" x 6'-2".
const CLEAR: ((f64, f64), (f64, f64)) = ((300.0, 200.0), (388.0, 274.0));

/// A storey: a 40' x 30' building of 8" walls, the hoistway's 8" walls (`east`: whether its east
/// wall is drawn) and the car drawn inside it as a 4" double outline.
fn storey(d: &mut Drawing, ox: f64, east: bool) {
    let all = [true; 4];
    sides(d, "A-WALL", (ox, 0.0), (ox + 480.0, 360.0), all);
    sides(d, "A-WALL", (ox + 8.0, 8.0), (ox + 472.0, 352.0), all);
    let ((x0, y0), (x1, y1)) = CLEAR;
    let keep = [true, east, true, true];
    sides(d, "A-WALL", (ox + x0 - 8.0, y0 - 8.0), (ox + x1 + 8.0, y1 + 8.0), keep);
    sides(d, "A-WALL", (ox + x0, y0), (ox + x1, y1), keep);
    // The car: 4" walls, 12" off the shaft's faces.
    sides(d, "A-WALL", (ox + x0 + 12.0, y0 + 12.0), (ox + x1 - 12.0, y1 - 12.0), all);
    sides(d, "A-WALL", (ox + x0 + 16.0, y0 + 16.0), (ox + x1 - 16.0, y1 - 16.0), all);
}

/// The test building, its spec stored; the session.
fn building() -> Session {
    let mut d = Drawing::default();
    storey(&mut d, 0.0, true);
    storey(&mut d, STEP, false);
    storey(&mut d, 2.0 * STEP, true);
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let ((x0, y0), (x1, y1)) = CLEAR;
    let mut levels = Vec::new();
    let mut circulation = Vec::new();
    for (i, (name, elev)) in [("Level 1", 0.0), ("Level 2", 10.0), ("Level 3", 20.0), ("Roof", 30.0)].into_iter().enumerate() {
        let ox = STEP * i as f64;
        levels.push(json!({"name": name, "elevation_ft": elev, "height_ft": 10, "ceiling_ft": 9, "base": [ox, 0], "region": [ox - 100.0, -100, ox + 1000.0, 1000]}));
        if i < 3 {
            // The trace a plan tracer makes of it: the car's inside.
            circulation.push(json!({"kind": "hoistway", "level": name, "name": "ELEV.", "at": [ox + 344.0, 237.0],
                "rects": [[[ox + x0 + 16.0, y0 + 16.0], [ox + x1 - 16.0, y1 - 16.0]]]}));
        }
    }
    // A unit on the roof plan over the shaft.
    let r = 3.0 * STEP;
    let unit = [[r + 320.0, 220.0], [r + 360.0, 220.0], [r + 360.0, 250.0], [r + 320.0, 250.0]];
    let spec = json!({
        "version": 1, "units_per_foot": 12, "stack": false,
        "levels": levels,
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "CMU-8"}],
        "wall_types": [{"name": "CMU-8", "thickness_in": 8}],
        "circulation": circulation,
        "exterior": {"equipment": [{"name": "EF-X", "level": "Roof", "outline": unit, "height_ft": 3.0}]},
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s
}

/// The built elements made as `part` on `level`: (handle, plan bounds, z range).
fn made(s: &Session, part: &str, level: &str) -> Vec<(Handle, [f64; 4], (f64, f64), Entity)> {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some(part))
        .filter(|e| cadaddy_engine::cmd::bspec::made_level(e).is_some_and(|l| l == level))
        .filter_map(|e| {
            let EntityKind::Mesh(m) = &e.kind else { return None };
            let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            let mut z = (f64::INFINITY, f64::NEG_INFINITY);
            for v in &m.vertices {
                b = [b[0].min(v.x), b[1].min(v.y), b[2].max(v.x), b[3].max(v.y)];
                z = (z.0.min(v.z), z.1.max(v.z));
            }
            Some((e.handle, b, z, (**e).clone()))
        })
        .collect()
}

fn xdata_has(e: &Entity, what: &str) -> bool {
    e.common.xdata.iter().any(|t| t.value.contains(what))
}

/// The walls of a storey inside the shaft's clear box (less 3").
fn walls_inside(s: &Session, level: &str, ox: f64) -> usize {
    let ((x0, y0), (x1, y1)) = CLEAR;
    made(s, "wall", level)
        .iter()
        .filter(|(_, b, _, _)| {
            let c = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
            c.0 > ox + x0 + 3.0 && c.0 < ox + x1 - 3.0 && c.1 > y0 + 3.0 && c.1 < y1 - 3.0
        })
        .count()
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn the_shaft_pit_and_overrun_are_built_from_the_elevator_submittal() {
    let mut s = building();
    let read = s
        .execute("elevator.read", &json!({"text": "--- page 2 of 5 ---\nHOISTWAY 7'-4\" x 6'-2\", PIT 4'-8\", OVERHEAD 13'-10\"\n"}))
        .expect("elevator.read");
    assert_eq!(read["applied"], json!(true), "{read}");
    let built = s.execute("model.build", &json!({})).expect("build");
    let h = &built["hoistways"];
    assert_eq!(h["hoistways"], json!(3), "{h}");
    // Each storey's shaft is the submittal's clear size, at the shaft's walls, not the car's.
    for sh in h["shafts"].as_array().unwrap() {
        let c = sh["clear_ft"].as_array().unwrap();
        assert!(near(c[0].as_f64().unwrap(), 88.0 / 12.0, 0.02) && near(c[1].as_f64().unwrap(), 74.0 / 12.0, 0.02), "{sh}");
        assert_eq!(sh["size_from"], json!(["submittal", "submittal"]), "{sh}");
        let b = &sh["box"];
        let ox = if sh["level"] == json!("Level 2") {
            STEP
        } else if sh["level"] == json!("Level 3") {
            2.0 * STEP
        } else {
            0.0
        };
        assert!(near(b[0][0].as_f64().unwrap(), ox + 300.0, 0.5) && near(b[0][1].as_f64().unwrap(), 200.0, 0.5), "{sh}");
    }
    // No wall inside the shaft on any storey: the car's outline is no wall.
    for (i, l) in ["Level 1", "Level 2", "Level 3"].iter().enumerate() {
        assert_eq!(walls_inside(&s, l, STEP * i as f64), 0, "{l}: {h}");
    }
    // Level 2 draws no east wall: the shaft's wall stands there, of the shaft's type.
    let east: Vec<_> = made(&s, "wall", "Level 2").into_iter().filter(|(_, _, _, e)| xdata_has(e, "hoistway:ELEV.")).collect();
    assert_eq!(east.len(), 1, "{h}");
    let (_, b, z, e) = &east[0];
    assert!(near(b[0], STEP + 388.0, 0.5) && near(b[2], STEP + 396.0, 0.5) && near(b[1], 192.0, 0.5) && near(b[3], 282.0, 0.5), "{b:?}");
    assert!(near(z.0, 120.0, 0.01) && near(z.1, 240.0, 0.01), "{z:?}");
    assert!(xdata_has(e, "type:CMU-8") && xdata_has(e, "derived"), "{:?}", e.common.xdata);
    // The pit: 4'-8" below Level 1 (and its slab under it).
    let pits = made(&s, "hoistway-pit", "Level 1");
    assert_eq!(pits.len(), 1, "{h}");
    let (_, _, z, e) = &pits[0];
    assert!(near(z.1, 0.0, 0.01) && near(z.0, -56.0 - 12.0, 0.01), "{z:?}");
    assert!(xdata_has(e, "derived") && xdata_has(e, "pit_depth"), "{:?}", e.common.xdata);
    // The overrun: on the roof plan over the shaft, from the roof (30') to the overhead over
    // Level 3 (20' + 13'-10"), roofed (1').
    let over = made(&s, "elevator-overrun", "Roof");
    assert_eq!(over.len(), 1, "{h}");
    let (_, b, z, e) = &over[0];
    let top = (20.0 + 13.0 + 10.0 / 12.0) * 12.0;
    assert!(near(z.0, 360.0, 0.01) && near(z.1, top + 12.0, 0.01), "{z:?}");
    assert!(b[0] < 3.0 * STEP + 300.0 && b[2] > 3.0 * STEP + 388.0, "{b:?}");
    assert!(xdata_has(e, "derived"), "{:?}", e.common.xdata);
    // The roof unit over it stands on its roof, its design base kept.
    let units = made(&s, "equipment", "Roof");
    assert_eq!(units.len(), 1, "{built}");
    let (_, _, z, e) = &units[0];
    assert!(near(z.0, top + 12.0, 0.01), "{z:?}");
    assert!(xdata_has(e, "base_elevation_ft"), "{:?}", e.common.xdata);
    assert_eq!(h["equipmentOnOverrun"], json!(1), "{h}");
}

#[test]
fn with_no_submittal_the_shaft_is_the_plans_and_the_pit_and_overrun_follow_trade_practice() {
    let mut s = building();
    let built = s.execute("model.build", &json!({})).expect("build");
    let h = &built["hoistways"];
    assert_eq!(h["hoistways"], json!(3), "{h}");
    // The shaft's walls round the car, never the car.
    let l1 = h["shafts"].as_array().unwrap().iter().find(|x| x["level"] == json!("Level 1")).unwrap();
    let c = l1["clear_ft"].as_array().unwrap();
    assert!(near(c[0].as_f64().unwrap(), 88.0 / 12.0, 0.02) && near(c[1].as_f64().unwrap(), 74.0 / 12.0, 0.02), "{l1}");
    assert_eq!(walls_inside(&s, "Level 1", 0.0), 0, "{h}");
    let pits = made(&s, "hoistway-pit", "Level 1");
    assert_eq!(pits.len(), 1, "{h}");
    assert!(xdata_has(&pits[0].3, "inferred") && xdata_has(&pits[0].3, "trade practice"), "{:?}", pits[0].3.common.xdata);
    let over = made(&s, "elevator-overrun", "Roof");
    assert_eq!(over.len(), 1, "{h}");
    assert!(xdata_has(&over[0].3, "inferred"), "{:?}", over[0].3.common.xdata);
    let _: &Value = &built;
}

#[test]
fn elevator_read_needs_a_source_and_reads_a_submittal_file() {
    let mut s = building();
    assert!(s.execute("elevator.read", &json!({})).is_err());
    assert!(s.execute("elevator.read", &json!({"files": 3})).is_err());
    let dir = std::env::temp_dir().join(format!("cadaddy-hoistway-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Elevator Shop Drawings.txt"), "CLEAR HOISTWAY WIDTH 7'-4\"\nCLEAR HOISTWAY DEPTH 6'-2\"\n").unwrap();
    std::fs::write(dir.join("Lighting Package.txt"), "HOISTWAY 9'-0\" x 9'-0\"\n").unwrap();
    let r = s.execute("elevator.read", &json!({"files": [dir.to_string_lossy()]})).expect("read");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(r["files"], json!(["Elevator Shop Drawings.txt"]), "{r}");
    assert_eq!(r["elevator_data"]["width_ft"].as_array().map(Vec::len), Some(1), "{r}");
    assert!(near(r["elevator_data"]["depth_ft"][0].as_f64().unwrap(), 74.0 / 12.0, 1e-6), "{r}");
    let spec = s.execute("bspec.get", &json!({})).unwrap();
    assert!(spec["spec"]["elevator_data"]["width_ft"].is_array(), "{spec}");
}
