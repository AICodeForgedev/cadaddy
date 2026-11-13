//! Q6621 fix-clash-counts-normal-construction: `bim.clash` leaves out what the trades install
//! that way on purpose (a drop in the cavity of the wall its device is in, breakers in a panel
//! recessed in its wall, a pipe through a floor) and lists the penetrations on their own, each
//! with its sleeve and seal; the real conflicts stay hard clashes.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, RawTag};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

use crate::conduitroute::storey_walls;

/// Two stacked storeys of 2x6 wood-stud walls (`rating`: the wall type's fire rating), 10' floor
/// to floor; on L1 a receptacle at 18" in the south wall fed by a drop down the wall's cavity, a
/// 2" vent stack in the same wall, a recessed panel with ten breakers in the west wall (`can_in`: its depth); a 4" sanitary pipe
/// down through L2's floor; and a 1" conduit across the pipe's path (a real clash).
fn building(rating: Option<&str>, can_in: f64) -> Session {
    let mut d = Drawing::new_imperial();
    storey_walls(&mut d, 0.0);
    storey_walls(&mut d, 200.0);
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let mut assembly = json!({
        "tag": "X1", "description": "EXTERIOR WALL", "source": "sample legend, mark X1",
        "layers": [
            {"material": "1/2\" wood structural panel sheathing", "thickness_in": 0.5, "category": "sheathing", "side": "side1"},
            {"material": "2x6 wood studs @ 16\" o.c.", "thickness_in": 5.5, "category": "framing", "side": "core"},
            {"material": "5/8\" gypsum board", "thickness_in": 0.625, "category": "board", "side": "side2"}
        ]
    });
    if let Some(r) = rating {
        assembly["fire_rating"] = json!(r);
    }
    let row = |n: u32| json!({"number": n.to_string(), "description": "RCPT", "breaker": "20", "poles": 1, "protection": [], "source": "X-2"});
    let circuit = "circuit:L1:E-101:LP1-3";
    // The south wall's middle line (its outer face at y = 0, 7 1/4" thick).
    let wall_mid_in = 7.25 / 2.0;
    let spec = json!({
        "version": 1, "units_per_foot": 12, "stack": true,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]},
            {"name": "L2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9, "region": [1900, -100, 3400, 1000], "base": [2400, 0]}
        ],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}],
        "wall_types": [{"name": "X1", "thickness_in": 7.25, "from": "pairs", "assembly": assembly}],
        "tags": [
            {"id": "device:L1:10.0,0.6", "kind": "receptacle", "level": "L1", "at_ft": [10.0, 0.6], "fields": {
                "symbol": {"value": "R-sym", "source": "plan"}, "sheet": {"value": "E-101", "source": "plan"},
                "circuit": {"value": "LP1-3", "source": "plan"}, "mount_height_in": {"value": "18", "source": "note"}
            }},
            {"id": circuit, "kind": "circuit", "level": "L1", "at_ft": [1.0, 10.0], "fields": {
                "circuit": {"value": "LP1-3", "source": "plan"}, "panel": {"value": "LP1", "source": "plan"},
                "conductors": {"value": "2#12 + 1#12G", "source": "default"}, "conduit": {"value": "3/4\" EMT", "source": "default"},
                "route": {"value": "estimated route", "source": "rule"}
            }}
        ],
        "mep": {
            "systems": [
                {"name": "P-SAN", "layer": "P-SAN", "kind": "pipe", "size_in": [4, 0], "bottom_ft": 8},
                {"name": "P-VENT", "layer": "P-VENT", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 8},
                {"name": "E-COND", "layer": "E-COND", "kind": "conduit", "size_in": [1, 0], "bottom_ft": 9},
                {"name": "E-CIRCUIT-EST", "layer": "E-CIRCUIT-EST", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9}
            ],
            "runs": [
                // The homerun along L1's ceiling space and its drop down the south wall's cavity.
                {"system": "E-CIRCUIT-EST", "level": "L1", "points": [[12, 120], [120, 120], [120, 24], [120, wall_mid_in]], "size_in": [0.75, 0], "bottoms_ft": [9.25, 9.25, 9.25],
                 "id": format!("{circuit}#home"), "height_source": "estimated route"},
                {"system": "E-CIRCUIT-EST", "level": "L1", "points": [[120, wall_mid_in]], "size_in": [0.75, 0], "bottoms_ft": [22.0 / 12.0], "top_ft": 9.25,
                 "id": format!("{circuit}#d0"), "height_source": "estimated route"},
                // A 2" vent stack in the south wall's cavity.
                {"system": "P-VENT", "level": "L1", "points": [[200, wall_mid_in]], "size_in": [2, 0], "bottoms_ft": [1.0], "top_ft": 8.0},
                // A 4" stack down through L2's floor into L1's ceiling space.
                {"system": "P-SAN", "level": "L2", "points": [[2700, 150]], "size_in": [4, 0], "bottoms_ft": [-3.0], "top_ft": 4.0},
                // A conduit across the stack in L1.s ceiling space: a real conflict.
                {"system": "E-COND", "level": "L1", "points": [[240, 150], [360, 150]], "size_in": [1, 0], "bottoms_ft": [8.5]}
            ],
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [7.25, 110], "max": [7.25 + can_in, 130], "height_ft": 3, "bottom_ft": 3.5,
                 "properties": {"MOUNTING": "RECESSED"}}
            ],
            "panel_schedules": [{"panel": "LP1", "source": "X-2", "circuits": (1..=10).map(row).collect::<Vec<_>>()}]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": false})).expect("build");
    s
}

fn cats(c: &Value) -> (String, String) {
    (c["a"]["category"].as_str().unwrap_or_default().to_string(), c["b"]["category"].as_str().unwrap_or_default().to_string())
}

fn has(c: &Value, x: &str, y: &str) -> bool {
    let (a, b) = cats(c);
    (a == x && b == y) || (a == y && b == x)
}

fn short(r: &Value) -> String {
    let groups: Vec<String> = r["groups"].as_array().into_iter().flatten().map(|g| format!("{}={}", g["key"], g["count"])).collect();
    let pens: Vec<String> = r["penetrations"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| format!("{} {} {} through {}", p["kind"], p["a"]["category"], p["a"]["system"], p["b"]["category"]))
        .collect();
    format!("total {} groups {groups:?} penetrations {pens:?} summary {}", r["total"], r["normal"])
}

#[test]
fn normal_construction_is_not_a_clash_and_penetrations_are_listed_with_their_seal() {
    let mut s = building(Some("1 HR"), 4.0);
    let r = s.execute("bim.clash", &json!({})).unwrap();
    eprintln!("bim.clash: {}", short(&r));
    let clashes = r["clashes"].as_array().cloned().unwrap_or_default();
    let arch = |c: &Value| c["a"]["trade"] == json!("architecture") || c["b"]["trade"] == json!("architecture");
    // The drop in its wall's cavity, the breakers and the recessed can in their wall, the stack
    // through the floor: none of them is a clash.
    for c in &clashes {
        assert!(!(arch(c) && (has(c, "conduit", "wall") || has(c, "wire", "wall") || has(c, "fitting", "wall"))), "a drop in its wall: {c}");
        assert!(!has(c, "breaker", "wall") && !has(c, "equipment", "wall"), "gear recessed in its wall: {c}");
        assert!(!has(c, "pipe", "floor") && !has(c, "pipe", "ceiling"), "a pipe through the floor: {c}");
        assert_ne!(cats(c).0, "breaker", "a breaker is inside its panel: {c}");
        assert_ne!(cats(c).1, "breaker", "a breaker is inside its panel: {c}");
    }
    // The real conflict stays: the conduit across the stack.
    assert!(clashes.iter().any(|c| has(c, "pipe", "conduit") && c["kind"] == json!("hard")), "{}", short(&r));
    // The stack through L2's floor is a penetration: its sleeve and its seal, with the rule.
    let pens = r["penetrations"].as_array().cloned().unwrap_or_default();
    let floor: Vec<&Value> = pens.iter().filter(|p| p["a"]["system"] == json!("P-SAN") && p["b"]["category"] == json!("floor")).collect();
    assert_eq!(floor.len(), 1, "one penetration of the floor by the stack (its pieces cut at the floor): {}", short(&r));
    let floor = floor[0];
    assert_eq!(floor["kind"], json!("through"), "{floor}");
    assert!((floor["size_in"].as_f64().unwrap_or(0.0) - 4.0).abs() < 0.2, "{floor}");
    assert!(floor["sleeve"].as_str().is_some_and(|x| !x.is_empty()), "{floor}");
    assert!(floor["seal"].as_str().is_some_and(|x| !x.is_empty()) && floor["firestop"] == json!(true), "{floor}");
    assert!(floor["source"].as_str().is_some_and(|x| x.contains("IBC")), "{floor}");
    // The homerun entering the top of the rated wall for its drop: a penetration that needs the
    // wall's firestop.
    let wall = pens
        .iter()
        .find(|p| p["b"]["category"] == json!("wall") && p["a"]["trade"] == json!("electrical"))
        .unwrap_or_else(|| panic!("{}", short(&r)));
    assert_eq!(wall["a"]["category"], json!("conduit"), "named by the raceway, not its wire: {wall}");
    assert_eq!(wall["kind"], json!("into"), "{wall}");
    assert_eq!(wall["firestop"], json!(true), "{wall}");
    assert!(wall["rating"].as_str().is_some_and(|x| x.contains("1 HR")), "{wall}");
    // The vent stack in the wall is in its cavity: no clash, no penetration.
    assert!(!pens.iter().any(|p| p["a"]["system"] == json!("P-VENT")), "{}", short(&r));
    assert!(!clashes.iter().any(|c| c["a"]["system"] == json!("P-VENT") || c["b"]["system"] == json!("P-VENT")), "{}", short(&r));
    // What was left out, counted by reason.
    let normal = &r["normal"];
    assert!(normal["in_cavity"].as_u64().unwrap_or(0) > 0, "{normal}");
    assert_eq!(normal["recessed"].as_u64(), Some(1), "{normal}");
    assert!(normal["in_gear"].as_u64().unwrap_or(0) >= 10, "the ten breakers: {normal}");
    assert_eq!(normal["penetrations"].as_u64(), Some(pens.len() as u64), "{normal}");

    // `hosted: true` reports normal construction as clashes, as before.
    let all = s.execute("bim.clash", &json!({"hosted": true})).unwrap();
    assert!(all["total"].as_u64().unwrap_or(0) >= r["total"].as_u64().unwrap_or(0) + 12, "{} vs {}", short(&all), short(&r));
    assert!(all["penetrations"].as_array().is_some_and(Vec::is_empty), "{}", short(&all));

    // The same building, its walls not rated: the wall penetration needs no firestop.
    let mut s = building(None, 4.0);
    let r = s.execute("bim.clash", &json!({})).unwrap();
    let pens = r["penetrations"].as_array().cloned().unwrap_or_default();
    let wall = pens
        .iter()
        .find(|p| p["b"]["category"] == json!("wall") && p["a"]["trade"] == json!("electrical"))
        .unwrap_or_else(|| panic!("{}", short(&r)));
    assert_eq!(wall["firestop"], json!(false), "{wall}");
}

#[test]
fn a_recessed_can_deeper_than_its_wall_is_still_a_clash() {
    // A 10" can recessed in a 7 1/4" wall comes out of the wall's far face.
    let mut s = building(None, 10.0);
    let r = s.execute("bim.clash", &json!({})).unwrap();
    eprintln!("bim.clash: {}", short(&r));
    let clashes = r["clashes"].as_array().cloned().unwrap_or_default();
    let can = clashes.iter().find(|c| has(c, "equipment", "wall")).unwrap_or_else(|| panic!("{}", short(&r)));
    assert_eq!(can["kind"], json!("hard"), "{can}");
    assert!(can["message"].as_str().unwrap_or_default().contains("deeper than"), "{can}");
}

/// A box (inches) on L1 tagged as a build tags its elements: part, level, source, then `tags`.
fn built_box(s: &mut Session, part: &str, tags: &[&str], lo: [f64; 3], hi: [f64; 3]) {
    let d = s.doc_mut().unwrap();
    let v = |x: usize, y: usize, z: usize| Vec3 { x: [lo[0], hi[0]][x], y: [lo[1], hi[1]][y], z: [lo[2], hi[2]][z] };
    let vertices = vec![v(0, 0, 0), v(1, 0, 0), v(1, 1, 0), v(0, 1, 0), v(0, 0, 1), v(1, 0, 1), v(1, 1, 1), v(0, 1, 1)];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([2, 3, 7, 6]), f([1, 2, 6, 5]), f([3, 0, 4, 7])];
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Mesh(Mesh { vertices, faces, ..Default::default() }));
    e.common.layer = "3D-Other".into();
    let mut x = vec![RawTag { code: 1001, value: "CADADDY_WALLGEN".into() }];
    for t in [part, "L1", "0"].into_iter().chain(tags.iter().copied()) {
        x.push(RawTag { code: 1000, value: t.into() });
    }
    e.common.xdata = x.into();
    d.model.push(e);
}

#[test]
fn devices_in_casework_and_a_through_wall_unit_are_construction() {
    let mut s = building(None, 4.0);
    // A base cabinet's layout box over the receptacle and its drop (against the south wall,
    // reaching into it), and a packaged terminal unit through the south wall in its sleeve.
    built_box(&mut s, "casework", &["type:Base cabinet"], [100.0, 2.0, 0.0], [140.0, 31.25, 34.5]);
    built_box(&mut s, "mep", &["kind:equipment", "part:equipment:ptac", "tag:U-1"], [300.0, -3.0, 4.0], [342.0, 14.0, 20.0]);
    let r = s.execute("bim.clash", &json!({})).unwrap();
    eprintln!("bim.clash: {}", short(&r));
    let clashes = r["clashes"].as_array().cloned().unwrap_or_default();
    for c in &clashes {
        assert!(c["a"]["category"] != json!("casework") && c["b"]["category"] != json!("casework"), "hosted by the casework: {c}");
        assert!(!has(c, "equipment", "wall"), "the unit is in its wall opening: {c}");
    }
    assert!(r["normal"]["hosted"].as_u64().unwrap_or(0) >= 2, "the device and its box: {}", r["normal"]);
    let pens = r["penetrations"].as_array().cloned().unwrap_or_default();
    let unit = pens.iter().find(|p| p["kind"] == json!("unit")).unwrap_or_else(|| panic!("{}", short(&r)));
    assert_eq!((unit["a"]["category"].as_str(), unit["b"]["category"].as_str()), (Some("equipment"), Some("wall")), "{unit}");
    assert!(unit["sleeve"].as_str().unwrap_or_default().contains("sleeve"), "{unit}");
    // The same unit with no through-wall words is equipment run into the wall: a clash.
    let mut s = building(None, 4.0);
    built_box(&mut s, "mep", &["kind:equipment", "part:equipment:cabinet_heater", "tag:U-1"], [300.0, -3.0, 4.0], [342.0, 14.0, 20.0]);
    let r = s.execute("bim.clash", &json!({})).unwrap();
    let clashes = r["clashes"].as_array().cloned().unwrap_or_default();
    assert!(clashes.iter().any(|c| has(c, "equipment", "wall")), "{}", short(&r));
}
