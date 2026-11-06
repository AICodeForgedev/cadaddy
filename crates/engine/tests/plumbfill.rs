//! Q6161 plumbrebuild: water CADaddy fills in for a level whose plans draw none (derived from the
//! floor its rooms stack on, or inferred from a rule) stays apart from the drawn piping: its own
//! layer, `status` and `basis` tags, its own takeoff rows, and a fixture on it says so. Nothing
//! filled in reports as drawn. Made-up plans only.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// One made-up room, 6" walls: 0.5–39.5' by 0.5–19.5'.
fn room() -> Drawing {
    let mut d = Drawing::new_imperial();
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let sides = [(0.0, 0.0), (40.0, 0.0), (40.0, 20.0), (0.0, 20.0)];
    let inner = [(0.5, 0.5), (39.5, 0.5), (39.5, 19.5), (0.5, 19.5)];
    for ring in [sides, inner] {
        for i in 0..4 {
            let h = Handle(d.handseed.max(0x100));
            d.handseed = h.0 + 1;
            let mut e = Entity::new(h, EntityKind::Line(Line { a: p(ring[i]), b: p(ring[(i + 1) % 4]) }));
            e.common.layer = "A-WALL".into();
            d.model.push(e);
        }
    }
    d
}

fn pipe(name: &str) -> Value {
    json!({"name": name, "layer": name, "kind": "pipe", "size_in": [2, 0], "bottom_ft": 9})
}

fn run(system: &str, points: Value, height_source: &str) -> Value {
    let basis = match height_source {
        "derived" => "derived from L2's drawn main where this floor's rooms stack on its: trade practice",
        "inferred" => "inferred: trade practice: a branch teed off the nearest main",
        _ => "tag",
    };
    json!({"system": system, "level": "L1", "points": points, "size_in": [1.5, 0], "bottoms_ft": [9.0], "height_source": height_source, "size_source": basis})
}

/// A water closet at (10', 5') with its waste and vent drawn beside it, and a cold water main
/// past it that no plan draws (`cw`: its height source).
fn built(cw: &str) -> Session {
    built_with(cw).0
}

/// [`built`], with a washer box beside the water closet, and the build's result.
fn built_with(cw: &str) -> (Session, Value) {
    let mut s = Session::empty();
    s.open_drawing(room(), "Plumbing Room", None);
    let fixture = json!({
        "id": "device:L1:10.0,5.0:WC-9", "kind": "plumbing_fixture", "level": "L1", "at_ft": [10.0, 5.0],
        "fields": {
            "type": {"value": "WC-9", "source": "plan"},
            "fixture": {"value": "WATER CLOSET", "source": "schedule"},
            "device_type": {"value": "WATER CLOSET", "source": "schedule"},
            "sheet": {"value": "P-101", "source": "plan"},
            "symbol": {"value": "1F8", "source": "plan"},
        },
    });
    // No schedule, detail or note gives its height.
    let washer = json!({
        "id": "device:L1:14.0,0.8:WB-7", "kind": "plumbing_fixture", "level": "L1", "at_ft": [14.0, 0.8],
        "fields": {
            "type": {"value": "WB-7", "source": "plan"},
            "fixture": {"value": "WASHING MACHINE BOX", "source": "schedule"},
            "device_type": {"value": "WASHING MACHINE BOX", "source": "schedule"},
            "sheet": {"value": "P-101", "source": "plan"},
            "symbol": {"value": "1F9", "source": "plan"},
        },
    });
    let runs = json!([
        run("SAN", json!([[96.0, 48.0], [96.0, 240.0]]), "default"),
        run("V", json!([[132.0, 48.0], [132.0, 240.0]]), "default"),
        run("CW", json!([[60.0, 84.0], [480.0, 84.0]]), cw),
    ]);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": [fixture, washer],
        "mep": {"systems": [pipe("SAN"), pipe("V"), pipe("CW")], "runs": runs},
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let r = s.execute("model.build", &json!({})).expect("build");
    (s, r)
}

fn layers(s: &Session) -> Vec<String> {
    let mut v: Vec<String> = s.doc().expect("doc").model.iter().map(|e| e.common.layer.to_string()).filter(|l| l.starts_with("3D-MEP-")).collect();
    v.sort();
    v.dedup();
    v
}

fn fixture_props(s: &mut Session) -> Value {
    let r = s.execute("bim.props.find", &json!({"kind": "device", "where": {"type_mark": "WC-9"}, "limit": 10})).expect("find");
    let Some(h) = r["handles"][0].as_str().map(str::to_string) else {
        let all = s.execute("bim.props.find", &json!({"limit": 50})).expect("all");
        panic!("one fixture: {r}\n{all}")
    };
    s.execute("bim.props.get", &json!({"handle": h})).expect("get")["elements"][0]["props"].clone()
}

fn cw_rows(s: &mut Session) -> Vec<(String, f64)> {
    let q = s.execute("mep.qty", &json!({})).expect("qty");
    q["pipes"]
        .as_array()
        .expect("pipes")
        .iter()
        .filter(|r| r["system"].as_str().is_some_and(|x| x.starts_with("CW")))
        .map(|r| (r["system"].as_str().unwrap_or("").to_string(), r["length_ft"].as_f64().unwrap_or(0.0)))
        .collect()
}

#[test]
fn a_filled_in_main_is_built_apart_counted_apart_and_named_on_its_fixtures() {
    // Drawn: one CW layer, one takeoff row, no word of filled-in work.
    let mut drawn = built("tag");
    assert!(layers(&drawn).iter().any(|l| l.starts_with("3D-MEP-CW-")), "{:?}", layers(&drawn));
    assert!(!layers(&drawn).iter().any(|l| l.contains("DERIVED") || l.contains("INFERRED")), "{:?}", layers(&drawn));
    assert_eq!(cw_rows(&mut drawn), vec![("CW".to_string(), 35.0)]);
    let p = fixture_props(&mut drawn);
    assert!(p["cold_water"]["value"].as_str().is_some(), "connected to the drawn main: {p:#}");
    assert!(!p["cold_water"]["note"].as_str().unwrap_or("").contains("no plan draws it"), "{p:#}");
    let no_status = drawn.doc().expect("doc").model.iter().all(|e| !e.common.xdata.iter().any(|t| t.value.starts_with("status:")));
    assert!(no_status, "drawn work carries no filled-in status");

    // Derived or inferred: its own layer (and nothing on the drawn one), its status and basis, its
    // own row, the same feet. Never reported as drawn.
    for (what, upper) in [("derived", "DERIVED"), ("inferred", "INFERRED")] {
        let mut s = built(what);
        let ls = layers(&s);
        assert!(ls.iter().any(|l| l.starts_with(&format!("3D-MEP-CW-{upper}-"))), "{ls:?}");
        assert!(!ls.iter().any(|l| l.starts_with("3D-MEP-CW-") && !l.contains(upper)), "{ls:?}");
        let on: Vec<_> = s.doc().expect("doc").model.iter().filter(|e| e.common.layer.contains(upper)).collect();
        assert!(!on.is_empty());
        for e in &on {
            assert!(e.common.xdata.iter().any(|t| t.value == format!("status:{what}")), "every {what} element tagged");
            assert!(e.common.xdata.iter().any(|t| t.value.starts_with("basis:") && t.value.len() > 10), "every {what} element has its basis");
            assert!(!e.common.xdata.iter().any(|t| t.value == "status:drawn"));
        }
        assert_eq!(cw_rows(&mut s), vec![(format!("CW ({what})"), 35.0)]);
        // The fixture still connects, and says its water line is filled in.
        let p = fixture_props(&mut s);
        assert!(p["cold_water"]["value"].as_str().is_some(), "{p:#}");
        assert!(p["cold_water"]["note"].as_str().is_some_and(|n| n.contains(&format!("{what}: no plan draws it"))), "{p:#}");
        assert!(p.get("unconnected").is_none_or(Value::is_null), "{p:#}");
    }
}

/// The water closet with a CW run 6' from it (`far`: its height source) and one 2' from it (`near`).
fn nearer(far: &str, near: &str) -> (Value, Value) {
    let mut s = Session::empty();
    s.open_drawing(room(), "Plumbing Room", None);
    let fixture = json!({
        "id": "device:L1:10.0,5.0:WC-9", "kind": "plumbing_fixture", "level": "L1", "at_ft": [10.0, 5.0],
        "fields": {
            "type": {"value": "WC-9", "source": "plan"},
            "fixture": {"value": "WATER CLOSET", "source": "schedule"},
            "device_type": {"value": "WATER CLOSET", "source": "schedule"},
            "sheet": {"value": "P-101", "source": "plan"},
            "symbol": {"value": "1F8", "source": "plan"},
        },
    });
    let runs = json!([
        run("SAN", json!([[96.0, 48.0], [96.0, 240.0]]), "default"),
        run("V", json!([[132.0, 48.0], [132.0, 240.0]]), "default"),
        run("CW", json!([[60.0, 132.0], [300.0, 132.0]]), far),
        run("CW", json!([[60.0, 84.0], [480.0, 84.0]]), near),
    ]);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": [fixture],
        "mep": {"systems": [pipe("SAN"), pipe("V"), pipe("CW")], "runs": runs},
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let r = s.execute("model.build", &json!({})).expect("build");
    (fixture_props(&mut s), r["plumbing"].clone())
}

#[test]
fn a_drawn_pipe_in_reach_comes_before_a_nearer_derived_one_and_a_derived_before_an_inferred() {
    let (p, pl) = nearer("tag", "derived");
    assert!(!p["cold_water"]["note"].as_str().unwrap_or("").contains("no plan draws it"), "the drawn branch: {p:#}");
    if !pl.is_null() {
        assert_eq!(pl["fully_connected_drawn"], json!(1), "{pl}");
        assert_eq!(pl["fully_connected_with_derived"], json!(0), "{pl}");
    }
    let (p, pl) = nearer("derived", "inferred");
    assert!(p["cold_water"]["note"].as_str().is_some_and(|n| n.contains("derived: no plan draws it")), "{p:#}");
    if !pl.is_null() {
        assert_eq!(pl["fully_connected_with_derived"], json!(1), "{pl}");
        assert_eq!(pl["fully_connected_with_inferred"], json!(0), "{pl}");
        assert_eq!(pl["by_system"]["CW"]["to_derived"], json!(1), "{pl}");
    }
}

#[test]
fn a_fixture_type_with_no_documented_height_is_listed_by_type() {
    let (_, r) = built_with("tag");
    let by = &r["deviceBuild"]["fixture_defaults_by_type"];
    assert_eq!(by["WB-7 WASHING MACHINE BOX"], json!(1), "{by}");
    // The water closet stands on the floor: no default.
    assert!(by.as_object().is_some_and(|o| o.len() == 1), "{by}");
}
