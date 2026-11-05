//! Q6161 plumbrebuild: water mains CADaddy proposes for a level whose plans draw no water (copied
//! from a level whose layout matches) stay apart from the drawn piping: their own layer and tag,
//! their own takeoff rows, and a fixture on one says so. Made-up plans only.

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
    json!({"system": system, "level": "L1", "points": points, "size_in": [1.5, 0], "bottoms_ft": [9.0], "height_source": height_source})
}

/// A water closet at (10', 5') with its waste and vent drawn beside it, and a cold water main
/// past it that no plan draws (`cw`: its height source).
fn built(cw: &str) -> Session {
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
        run("CW", json!([[60.0, 84.0], [480.0, 84.0]]), cw),
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
    s.execute("model.build", &json!({})).expect("build");
    s
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
fn a_proposed_main_is_built_apart_counted_apart_and_named_on_its_fixtures() {
    // Drawn: one CW layer, one takeoff row, no word of a proposal.
    let mut drawn = built("tag");
    assert!(layers(&drawn).iter().any(|l| l.starts_with("3D-MEP-CW-")), "{:?}", layers(&drawn));
    assert!(!layers(&drawn).iter().any(|l| l.contains("PROPOSED")), "{:?}", layers(&drawn));
    assert_eq!(cw_rows(&mut drawn), vec![("CW".to_string(), 35.0)]);
    let p = fixture_props(&mut drawn);
    assert!(p["cold_water"]["value"].as_str().is_some(), "connected to the drawn main: {p:#}");
    assert!(!p["cold_water"]["note"].as_str().unwrap_or("").contains("proposed"), "{p:#}");

    // Proposed: its own layer (and nothing on the drawn one), its own row, the same feet.
    let mut prop = built("proposed");
    let ls = layers(&prop);
    assert!(ls.iter().any(|l| l.starts_with("3D-MEP-CW-PROPOSED-")), "{ls:?}");
    assert!(!ls.iter().any(|l| l.starts_with("3D-MEP-CW-") && !l.contains("PROPOSED")), "{ls:?}");
    let tagged = prop
        .doc()
        .expect("doc")
        .model
        .iter()
        .filter(|e| e.common.layer.contains("PROPOSED"))
        .all(|e| e.common.xdata.iter().any(|t| t.value == "status:proposed"));
    assert!(tagged, "every proposed element tagged");
    assert_eq!(cw_rows(&mut prop), vec![("CW (proposed)".to_string(), 35.0)]);
    // The fixture still connects, and says its water line is a proposal.
    let p = fixture_props(&mut prop);
    assert!(p["cold_water"]["value"].as_str().is_some(), "{p:#}");
    assert!(p["cold_water"]["note"].as_str().is_some_and(|n| n.contains("proposed")), "{p:#}");
    assert!(p.get("unconnected").is_none_or(Value::is_null), "{p:#}");
}
