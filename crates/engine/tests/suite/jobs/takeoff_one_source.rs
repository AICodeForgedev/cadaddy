//! One surface, one layout; one slab, one source; one row per line
//! (fix-takeoff-ceiling-board-sheets-27x). A made-up one-storey building, one room deep and long
//! and narrow (46' x 19'): its ceilings' board sheets add up to the ceilings (the count is the
//! sheets bought, laid once over each ceiling and never once per face of its mesh), its slab's
//! floor element, pour and vapor barrier share one thickness and one area, and its takeoff
//! writes no two rows alike.

use std::collections::BTreeSet;

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

fn text(s: &mut Session, at: [f64; 2], value: &str) {
    s.execute("text", &json!({"at": at, "text": value, "height": 6.0})).unwrap();
}

const T: f64 = 7.25;
const HALF: f64 = 4.875 / 2.0;
/// The building: 46' x 19' outside.
const W: f64 = 552.0;
const D: f64 = 228.0;

/// The made-up building: its shell, two partitions (three rooms), no slab or ceiling given.
fn building() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [W, D]);
    rect(&mut s, [T, T], [W - T, D - T]);
    for x in [200.0, 380.0] {
        for dx in [-HALF, HALF] {
            line(&mut s, [x + dx, T], [x + dx, D - T]);
        }
    }
    layer(&mut s, "A-ROOM-NAME");
    for (x, n) in [(100.0, "101"), (290.0, "102"), (460.0, "103")] {
        text(&mut s, [x, 114.0], n);
    }
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000]}],
        "layers": [{"layer": "A-WALL", "role": "wall"}],
        "rooms": [
            {"name": "SALES", "number": "101", "level": "1"},
            {"name": "PREP", "number": "102", "level": "1"},
            {"name": "STORAGE", "number": "103", "level": "1"}
        ]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert!(r["tradePieces"].as_u64().unwrap_or(0) > 0, "the build lays out trade pieces: {r}");
    s
}

fn count(s: &mut Session, q: Value) -> Value {
    s.execute("bim.count", &q).unwrap()
}

fn n(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or(0.0)
}

/// A built element's build tag `key:` value.
fn tag(e: &cadaddy_doc::Entity, key: &str) -> Option<String> {
    e.common.xdata.iter().find_map(|t| t.value.strip_prefix(&format!("{key}:")).map(str::to_string))
}

/// Each built ceiling's underside: its area (sq ft) and the sheets a 4' x 12' layout from the
/// corner of its extents puts on it (the long way along its long side), an upper bound.
fn ceilings(s: &Session) -> Vec<(f64, f64)> {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("ceiling"))
        .filter_map(|e| {
            let cadaddy_doc::EntityKind::Mesh(m) = &e.kind else { return None };
            let z = m.vertices.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
            let mut area = 0.0;
            let mut faces = 0;
            for f in &m.faces {
                let p: Vec<_> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize)).collect();
                if p.len() < 3 || p.iter().any(|v| (v.z - z).abs() > 1e-6) {
                    continue;
                }
                faces += 1;
                let n = p.len();
                area += (0..n).map(|i| p[i].x * p[(i + 1) % n].y - p[(i + 1) % n].x * p[i].y).sum::<f64>().abs() / 2.0;
            }
            let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
            for v in &m.vertices {
                lo = [lo[0].min(v.x), lo[1].min(v.y)];
                hi = [hi[0].max(v.x), hi[1].max(v.y)];
            }
            let (a, b) = ((hi[0] - lo[0]) / 12.0, (hi[1] - lo[1]) / 12.0);
            let cells = (a.max(b) / 12.0 - 1e-9).ceil() * (a.min(b) / 4.0 - 1e-9).ceil();
            (faces > 0).then_some((area / 144.0, cells))
        })
        .collect()
}

#[test]
fn a_ceilings_board_sheets_add_up_to_it_and_count_the_sheets_bought() {
    let mut s = building();
    let ceil = ceilings(&s);
    assert!(!ceil.is_empty(), "the rooms have ceilings");
    let area: f64 = ceil.iter().map(|c| c.0).sum();
    let most: f64 = ceil.iter().map(|c| c.1).sum();
    // The ceiling board: the rules' gypsum board ceiling (no ceiling type given).
    let rows = count(&mut s, json!({"trade": "drywall", "kind": "sheet", "group_by": "item"}));
    let board: Vec<&Value> = rows["rows"].as_array().unwrap().iter().filter(|r| r["item"].as_str().is_some_and(|i| i.contains("ceiling"))).collect();
    assert!(!board.is_empty(), "{rows}");
    let sheets: f64 = board.iter().map(|r| n(r, "count")).sum();
    let laid: f64 = board.iter().map(|r| n(r, "area_sf")).sum();
    // What the sheets cover is the ceilings' area; the count is no more than one layout over
    // each ceiling puts on it (a mesh of many faces is one surface).
    assert!((laid - area).abs() < 0.01 * area.max(1.0), "sheets cover {laid} sq ft of {area} sq ft of ceiling");
    assert!(sheets <= most, "{sheets} sheets laid where one 4x12 layout per ceiling takes at most {most}: {rows}");
    assert!(sheets * 48.0 >= area, "{sheets} sheets of 48 sq ft cover {area} sq ft");
    assert!(sheets * 48.0 <= 2.0 * area + 48.0 * ceil.len() as f64, "{sheets} sheets of 48 sq ft for {area} sq ft");
}

#[test]
fn one_slab_is_one_thickness_and_its_vapor_barrier_covers_it() {
    let mut s = building();
    // The floor element of the storey on grade: its thickness as built.
    let (thick, source) = {
        let d = s.doc().unwrap();
        let floor = d.model.iter().find(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("floor")).expect("the storey has its floor");
        (tag(floor, "thick_in").and_then(|t| t.parse::<f64>().ok()).expect("thick_in"), tag(floor, "thick_basis").unwrap_or_default())
    };
    let pour = count(&mut s, json!({"trade": "concrete", "kind": "pour"}));
    assert_eq!(n(&pour, "count"), 1.0, "{pour}");
    // The vapor barrier under it covers its whole area (19' across: two 12' strips, lapped).
    let slab = n(&pour, "area_sf");
    let vb = count(&mut s, json!({"trade": "concrete", "kind": "vapor_barrier"}));
    let vb_sf = n(&vb, "area_sf");
    assert!(vb_sf >= slab - 1.0, "vapor barrier {vb_sf} sq ft under a {slab} sq ft slab: {vb}");
    assert!(vb_sf <= slab * 1.1, "vapor barrier {vb_sf} sq ft under a {slab} sq ft slab (laps only): {vb}");
    // Nothing gives the slab: the trade's slab on grade (company default 4"), marked a default.
    assert_eq!(thick, 4.0, "{source}");
    assert!(source.contains("slab on grade"), "{source}");
    let poured = n(&pour, "volume_cf") * 12.0 / n(&pour, "area_sf");
    assert!((poured - thick).abs() < 0.01, "the pour is {poured}\", the floor {thick}\": {pour}");
}

#[test]
fn a_documented_slab_is_the_floors_and_the_pours() {
    let mut s = building();
    // A note on the plan gives the slab: the floor is built to it, and the pour is the floor's.
    layer(&mut s, "A-ANNO");
    text(&mut s, [100.0, 60.0], "5\" CONCRETE SLAB ON GRADE");
    s.execute("model.build", &json!({})).unwrap();
    let thick = {
        let d = s.doc().unwrap();
        let floor = d.model.iter().find(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("floor")).expect("floor");
        tag(floor, "thick_in").and_then(|t| t.parse::<f64>().ok()).expect("thick_in")
    };
    assert_eq!(thick, 5.0);
    let pour = count(&mut s, json!({"trade": "concrete", "kind": "pour"}));
    let poured = n(&pour, "volume_cf") * 12.0 / n(&pour, "area_sf");
    assert!((poured - 5.0).abs() < 0.01, "{pour}");
}

/// Two receptacle marks of one device type, two of each: two lines of one item.
fn two_marks() -> Session {
    let mut s = Session::new();
    let dev = |id: &str, mark: &str, x: f64, y: f64| {
        json!({"id": format!("device:Level 1:{id}"), "kind": "receptacle", "level": "Level 1", "at_ft": [x, y],
            "fields": {"type": {"value": mark, "source": "plan"}, "device_type": {"value": "duplex receptacle", "source": "legend"},
                "symbol": {"value": "", "source": "plan"}}})
    };
    let spec = json!({
        "units_per_foot": 12,
        "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9}],
        "tags": [dev("A1", "RA", 10.0, 10.0), dev("A2", "RA", 20.0, 10.0), dev("X1", "RX", 10.0, 30.0), dev("X2", "RX", 20.0, 30.0)]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({"route": false})).unwrap();
    s
}

#[test]
fn a_takeoff_writes_no_two_rows_alike() {
    let mut s = two_marks();
    let t = s.execute("bim.takeoff", &json!({"group_by": "level", "keep": false})).unwrap();
    let lines: Vec<&Value> = t["lines"].as_array().unwrap().iter().filter(|l| l["item"] == "duplex receptacle").collect();
    assert_eq!(lines.len(), 2, "two marks stay two lines: {t}");
    let csv = t["csv"].as_str().unwrap();
    let mut rows = csv.lines();
    let head: Vec<&str> = rows.next().unwrap().split(',').collect();
    assert_eq!(head.get(..9).map(|h| h.join(",")).as_deref(), Some("level,trade,system,category,item,size,unit,qty,elements"), "{csv}");
    assert!(head.contains(&"made") && head.contains(&"mark"), "{csv}");
    let rows: Vec<&str> = rows.collect();
    assert_eq!(rows.len(), t["lines"].as_array().unwrap().len(), "{csv}");
    let distinct: BTreeSet<&&str> = rows.iter().collect();
    assert_eq!(distinct.len(), rows.len(), "no two rows alike: {csv}");
    let mark = head.iter().position(|h| *h == "mark").unwrap();
    let marks: BTreeSet<&str> = rows.iter().filter(|r| r.contains(",duplex receptacle,")).filter_map(|r| r.split(',').nth(mark)).collect();
    assert_eq!(marks, BTreeSet::from(["RA", "RX"]), "{csv}");
    // The Automation tab's table the same.
    let a = s.execute("automation.takeoff", &json!({})).unwrap();
    let rows: Vec<String> = a["rows"].as_array().unwrap().iter().map(|r| r.to_string()).collect();
    let distinct: BTreeSet<&String> = rows.iter().collect();
    assert_eq!(distinct.len(), rows.len(), "{a}");
}
