//! Q6386: `model.clashes` on a stacked building names the objects of every clash and flags only
//! what really hangs below a ceiling: measured against the room's own built ceiling on each
//! level's plan (the build stacks the upper storeys, the runs keep their own plans), a drain
//! under the floor against the storey under it, a raceway inside a wall and a stub-up into gear
//! where they belong. A made-up two-storey building, its plans drawn side by side.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bimlook::made_tag;
use cadaddy_engine::cmd::wallgen::made_as;
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

const T: f64 = 7.25;
const HALF: f64 = 4.875 / 2.0;
/// Where level 2's plan is drawn (inches right of level 1's).
const OX: f64 = 2000.0;

/// A 40' x 24' storey at `ox`: exterior walls, a partition at 20', two numbered rooms.
fn storey(s: &mut Session, ox: f64, n: u32) {
    layer(s, "A-WALL");
    rect(s, [ox, 0.0], [ox + 480.0, 288.0]);
    rect(s, [ox + T, T], [ox + 480.0 - T, 288.0 - T]);
    for dx in [-HALF, HALF] {
        line(s, [ox + 240.0 + dx, T], [ox + 240.0 + dx, 288.0 - T]);
    }
    layer(s, "A-ROOM-NAME");
    for (x, k) in [(120.0, 1), (360.0, 2)] {
        s.execute("text", &json!({"at": [ox + x, 144.0], "text": format!("{n}0{k}"), "height": 6.0})).unwrap();
    }
}

/// Two storeys of 10 ft, the rooms' ceilings at 8 ft (the levels' own default 7 ft), stacked,
/// and the runs on each level's own plan.
fn building() -> Session {
    let mut s = Session::new();
    storey(&mut s, 0.0, 1);
    storey(&mut s, OX, 2);
    let room = |n: &str, l: &str| json!({"name": "ROOM", "number": n, "level": l, "ceiling_ft": 8.0});
    let run = |sys: &str, l: &str, pts: Value, size: [f64; 2], bottom: f64| json!({"system": sys, "level": l, "points": pts, "size_in": size, "bottoms_ft": [bottom]});
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "stack": true,
        "levels": [
            {"name": "Level 1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 7, "region": [-100, -100, 1000, 1000]},
            {"name": "Level 2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 7, "region": [OX - 100.0, -100, OX + 1000.0, 1000]},
            {"name": "Roof", "elevation_ft": 20, "height_ft": 10, "ceiling_ft": 8, "region": [2.0 * OX - 100.0, -100, 2.0 * OX + 1000.0, 1000]}
        ],
        "layers": [{"layer": "A-WALL", "role": "wall"}],
        "rooms": [room("101", "Level 1"), room("102", "Level 1"), room("201", "Level 2"), room("202", "Level 2")],
        "mep": {
            "systems": [
                {"name": "SA", "layer": "M-SUPP", "kind": "duct", "size_in": [12, 8], "bottom_ft": 9},
                {"name": "SAN", "layer": "P-SAN", "kind": "pipe", "size_in": [4, 0], "bottom_ft": -1},
                {"name": "E-COND", "layer": "E-COND", "kind": "conduit", "size_in": [0.75, 0.75], "bottom_ft": 8.5},
                {"name": "RL", "layer": "M-REFR", "kind": "pipe", "size_in": [0.875, 0.0], "bottom_ft": 1.5}
            ],
            "runs": [
                // Hung 6" under room 201's 8 ft ceiling (over the level's 7 ft default).
                run("SA", "Level 2", json!([[OX + 60.0, 100.0], [OX + 200.0, 100.0]]), [12.0, 8.0], 7.5),
                // Under level 2's floor, over level 1's ceiling: where a drain belongs.
                run("SAN", "Level 2", json!([[OX + 60.0, 200.0], [OX + 200.0, 200.0]]), [4.0, 0.0], -1.0),
                // Under level 2's floor but a foot under level 1's ceiling: hangs in level 1.
                run("SAN", "Level 2", json!([[OX + 300.0, 200.0], [OX + 440.0, 200.0]]), [4.0, 0.0], -3.0),
                // Along the partition's studs between two boxes 4 ft up: inside the wall.
                run("E-COND", "Level 2", json!([[OX + 240.0, 40.0], [OX + 240.0, 240.0]]), [0.75, 0.75], 4.0),
                // Along the exterior wall's inside face an inch off it, 3 ft up: at the wall.
                run("E-COND", "Level 2", json!([[OX + 60.0, T + 1.375], [OX + 200.0, T + 1.375]]), [0.75, 0.75], 3.0),
                // Across room 201 at the same height: exposed, a clash.
                run("E-COND", "Level 2", json!([[OX + 60.0, 60.0], [OX + 200.0, 60.0]]), [0.75, 0.75], 4.0),
                // On supports 18" over the roof deck: a roof has no ceiling.
                run("RL", "Roof", json!([[2.0 * OX + 60.0, 100.0], [2.0 * OX + 200.0, 100.0]]), [0.875, 0.0], 1.5),
                // Up out of the floor into gear 4.5 ft up (a riser from the floor).
                {"system": "E-COND", "level": "Level 1", "points": [[300.0, 40.0]], "size_in": [0.75, 0.75], "bottoms_ft": [0.0], "top_ft": 4.5}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    s.execute("model.build", &json!({"echo": false})).unwrap();
    s
}

fn clashes(s: &mut Session) -> Vec<Value> {
    s.execute("model.clashes", &json!({})).unwrap()["clashes"].as_array().cloned().unwrap_or_default()
}

#[test]
fn below_the_ceiling_is_measured_where_the_run_really_is() {
    let mut s = building();
    let all = clashes(&mut s);
    let mut low: Vec<(String, String, f64, f64)> = all
        .iter()
        .filter(|c| c["kind"] == "below_ceiling")
        .map(|c| {
            (
                c["systems"][0].as_str().unwrap_or("").to_string(),
                c["level"].as_str().unwrap_or("").to_string(),
                c["at"][1].as_f64().unwrap_or(f64::NAN),
                c["by_in"].as_f64().unwrap_or(f64::NAN),
            )
        })
        .collect();
    low.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.total_cmp(&b.2)));
    let got: Vec<(&str, &str, f64)> = low.iter().map(|x| (x.0.as_str(), x.1.as_str(), x.2)).collect();
    assert_eq!(
        got,
        vec![("E-COND", "Level 2", 60.0), ("SA", "Level 2", 100.0), ("SAN", "Level 1", 200.0)],
        "the duct under the room's ceiling, the exposed conduit and the low drain (in level 1's storey), not the drain over \
         level 1's ceiling, the raceways in and along the walls, the roof's piping or the stub-up into gear: {all:#?}"
    );
    let by = |sys: &str| low.iter().find(|x| x.0 == sys).map_or(f64::NAN, |x| x.3);
    assert!((by("SA") - 6.0).abs() < 0.01, "6\" under the room's 8 ft ceiling: {low:?}");
    assert!((by("SAN") - 12.0).abs() < 0.01, "a foot under level 1's ceiling: {low:?}");
}

#[test]
fn every_clash_names_its_built_objects() {
    let mut s = building();
    let all = clashes(&mut s);
    assert!(!all.is_empty());
    let missing: Vec<&Value> = all.iter().filter(|c| c["handles"].as_array().is_none_or(Vec::is_empty)).collect();
    assert!(missing.is_empty(), "a clash with no objects can't be found or shown: {missing:#?}");
    // The duct's clash names the duct as built on level 2 (the object the model shows), and every
    // handle named is an object of the drawing.
    let d = s.doc().unwrap();
    let duct: Vec<String> = d
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("mep") && made_tag(e, "system") == Some("SA"))
        .filter(|e| cadaddy_engine::cmd::bspec::made_level(e).as_deref() == Some("Level 2"))
        .map(|e| e.handle.hex())
        .collect();
    let sa = all.iter().find(|c| c["kind"] == "below_ceiling" && c["systems"][0] == "SA").expect("the duct hangs low");
    let named: Vec<&str> = sa["handles"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(named.iter().any(|h| duct.iter().any(|x| x == h)), "{named:?} names none of the duct's pieces {duct:?}");
    for c in &all {
        for h in c["handles"].as_array().unwrap().iter().filter_map(Value::as_str) {
            let h = cadaddy_engine::doc::Handle::parse_hex(h).expect("a hex handle");
            assert!(d.entity(h).is_some(), "{c}");
        }
    }
}
