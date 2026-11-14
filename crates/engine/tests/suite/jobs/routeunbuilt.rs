//! Q6489 fix-route-for-unbuilt-devices: a circuit none of whose devices is in the model is not
//! routed (its estimated route connects nothing: no raceway built or taken off, listed and asked
//! for review), and a roof level, which has no ceiling, routes its circuits on the deck.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

/// A weatherproof receptacle tag on `level` at (x, y) feet on `circuit`, 24" up.
fn receptacle(n: usize, level: &str, x: f64, y: f64, circuit: &str) -> Value {
    json!({"id": format!("device:{level}:{n}"), "kind": "receptacle", "level": level, "at_ft": [x, y], "fields": {
        "type": {"value": "WP", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x2A0 + n), "source": "plan"},
        "sheet": {"value": "E-9", "source": "plan"},
        "circuit": {"value": circuit, "source": "plan"},
        "mount_height_in": {"value": "24", "source": "note"}
    }})
}

fn circuit(level: &str, label: &str) -> Value {
    json!({"id": format!("circuit:{level}:E-9:{label}"), "kind": "circuit", "level": level, "at_ft": [0.0, 0.0], "fields": {
        "circuit": {"value": label, "source": "plan"},
        "panel": {"value": "PA", "source": "plan"},
        "sheet": {"value": "E-9", "source": "plan"},
        "conductors": {"value": "2#12 + 1#12G", "source": "default"},
        "conduit": {"value": "3/4\" EMT", "source": "default"},
        "route": {"value": "estimated route", "source": "rule"}
    }})
}

/// An estimated route piece (as the plan reader chains it) of `label` on `level`, sheet inches,
/// at `bottom` feet over the level's floor.
fn estimate(level: &str, label: &str, n: usize, points: Value, bottom: f64) -> Value {
    let segs = points.as_array().map_or(1, |p| p.len().saturating_sub(1).max(1));
    json!({"system": "E-CIRCUIT-EST", "level": level, "id": format!("circuit:{level}:E-9:{label}#{n}"), "points": points,
        "size_in": [0.75, 0], "bottoms_ft": vec![bottom; segs], "size_source": "default", "height_source": "estimated route", "source_sheet": "E-9"})
}

/// A made-up two-level building: a 12' storey (9' ceiling, panel PA) and its roof over it at
/// 12'. On the storey, PA-1's two receptacles. On the roof: PA-3 and PA-5 printed on the plan
/// with an estimated route each (one 58' long) but none of their devices built, and PA-7's two
/// receptacles by a unit. `extra` adds tags.
fn job(extra: Vec<Value>) -> (Session, Value) {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let mut tags = vec![
        receptacle(1, "Level 1", 20.0, 10.0, "PA-1"),
        receptacle(2, "Level 1", 30.0, 10.0, "PA-1"),
        receptacle(3, "Roof", 40.0, 20.0, "PA-7"),
        receptacle(4, "Roof", 52.0, 20.0, "PA-7"),
        circuit("Level 1", "PA-1"),
        circuit("Roof", "PA-3"),
        circuit("Roof", "PA-5"),
        circuit("Roof", "PA-7"),
    ];
    tags.extend(extra);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [
            {"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]},
            {"name": "Roof", "elevation_ft": 12, "height_ft": 10, "base": [0, 0]}
        ],
        "tags": tags,
        "mep": {
            "systems": [{"name": "E-CIRCUIT-EST", "layer": "E-CIRCUIT-EST", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 8.25}],
            "runs": [
                // PA-3: 58' along x then y over the roof, at the default ceiling over it.
                estimate("Roof", "PA-3", 1, json!([[120, 120], [720, 120], [720, 216]]), 8.25),
                estimate("Roof", "PA-5", 1, json!([[120, 360], [480, 360]]), 8.25),
                estimate("Roof", "PA-7", 1, json!([[480, 240], [624, 240]]), 8.25)
            ],
            "equipment": [
                {"tag": "PA", "kind": "panel", "level": "Level 1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5,
                 "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({})).expect("build");
    (s, built)
}

/// Every conduit stick of `circuit`: its ends (feet).
fn sticks(s: &Session, circuit: &str) -> Vec<([f64; 3], [f64; 3])> {
    let d = s.doc().expect("doc");
    let xyz = |t: String| {
        let v: Vec<f64> = t.split(',').filter_map(|x| x.trim().parse::<f64>().ok()).collect();
        [v.first().copied().unwrap_or(0.0) / 12.0, v.get(1).copied().unwrap_or(0.0) / 12.0, v.get(2).copied().unwrap_or(0.0) / 12.0]
    };
    d.model
        .iter()
        .map(|e| cadaddy_engine::doc::props::read(&e.common.xdata))
        .filter(|p| p.get("atom").map(|x| x.text()).as_deref() == Some("conduit"))
        .filter(|p| p.get("circuit").map(|x| x.text()).as_deref() == Some(circuit))
        .map(|p| (xyz(p.get("start").map(|x| x.text()).unwrap_or_default()), xyz(p.get("end").map(|x| x.text()).unwrap_or_default())))
        .collect()
}

fn conduit_count(s: &mut Session, circuit: &str) -> u64 {
    s.execute("bim.count", &json!({"kind": "conduit", "circuit": circuit})).expect("count")["count"].as_u64().unwrap_or(0)
}

fn no_device_listed(r: &Value) -> Vec<String> {
    r["unrouted"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|u| u["no_device"] == json!(true))
        .filter_map(|u| u["circuit"].as_str().map(str::to_string))
        .collect()
}

#[test]
fn a_circuit_with_no_device_built_gets_no_raceway_and_is_listed_and_asked() {
    let (mut s, _) = job(Vec::new());
    // No raceway for the circuits whose devices were never built: none in the model, none in
    // the takeoff.
    for c in ["PA-3", "PA-5"] {
        assert_eq!(conduit_count(&mut s, c), 0, "{c}: an estimated route that connects nothing is no raceway: {:?}", sticks(&s, c));
    }
    let tk = s.execute("conduit.takeoff", &json!({"group_by": ["circuit"]})).expect("takeoff");
    let lines: Vec<&Value> =
        tk["lines"].as_array().into_iter().flatten().filter(|l| matches!(l["circuit"].as_str(), Some("PA-3" | "PA-5"))).collect();
    assert!(lines.is_empty(), "{lines:?}");
    // Listed as not routed for want of a device; the routed circuits still route.
    let r = s.execute("conduit.route", &json!({})).expect("report");
    let mut listed = no_device_listed(&r);
    listed.sort();
    assert_eq!(listed, vec!["PA-3".to_string(), "PA-5".to_string()], "{}", r["unrouted"]);
    assert!(conduit_count(&mut s, "PA-1") > 0 && conduit_count(&mut s, "PA-7") > 0, "{}", r["unrouted"]);
    // A question for review each, never a raceway.
    let q = s.execute("review.list", &json!({"kind": "question", "limit": 50})).expect("review");
    let titles: Vec<&str> = q["items"].as_array().into_iter().flatten().filter_map(|i| i["title"].as_str()).collect();
    for c in ["PA-3", "PA-5"] {
        assert!(titles.iter().any(|t| t.contains(&format!("Circuit {c} on Roof is not routed"))), "{c}: {titles:?}");
    }
    let id = q["items"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| i["title"].as_str().is_some_and(|t| t.contains("PA-3")))
        .and_then(|i| i["id"].as_str())
        .expect("item")
        .to_string();
    let item = s.execute("review.next", &json!({"id": id})).expect("next");
    assert_eq!(item["item"]["detail"]["reason"].as_str().map(|r| r.starts_with("circuit not routed: no device")), Some(true), "{item}");
    // A reroute and a build after it leave it so (the routing starts from the circuits as read);
    // the routed spec it stores says the circuit is not routed, and why.
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    assert_eq!(conduit_count(&mut s, "PA-3"), 0);
    let sp = s.execute("bspec.get", &json!({})).expect("spec");
    let tag = sp["spec"]["tags"].as_array().into_iter().flatten().find(|t| t["id"] == json!("circuit:Roof:E-9:PA-3")).cloned().unwrap_or_default();
    assert!(tag["issues"].as_array().is_some_and(|i| i.iter().any(|x| x == "no_device_built")), "{tag}");
    assert_eq!(tag["fields"]["route"]["value"], json!("not routed"), "{tag}");
    assert!(
        !sp["spec"]["mep"]["runs"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|r| r["id"].as_str().is_some_and(|i| i.starts_with("circuit:Roof:E-9:PA-3#"))),
        "its estimated route is set aside"
    );
    s.execute("model.build", &json!({})).expect("again");
    assert_eq!(conduit_count(&mut s, "PA-3"), 0);
    let r2 = s.execute("conduit.route", &json!({})).expect("report");
    let mut again = no_device_listed(&r2);
    again.sort();
    assert_eq!(again, listed, "{}", r2["unrouted"]);
}

#[test]
fn once_its_devices_are_built_the_circuit_routes() {
    let (mut s, _) = job(vec![receptacle(5, "Roof", 10.0, 10.0, "PA-3"), receptacle(6, "Roof", 60.0, 10.0, "PA-3")]);
    assert!(conduit_count(&mut s, "PA-3") > 0);
    let r = s.execute("conduit.route", &json!({})).expect("report");
    assert_eq!(no_device_listed(&r), vec!["PA-5".to_string()], "{}", r["unrouted"]);
}

#[test]
fn a_roof_circuit_routes_on_the_deck_not_a_ceilings_height_over_it() {
    let (s, _) = job(Vec::new());
    let st = sticks(&s, "PA-7");
    // Its horizontal pieces over the roof (above the storey's 12' floor to floor): every one on
    // the deck band (supports 4" up, no higher than a 14" curb), none in the air.
    let flat: Vec<&([f64; 3], [f64; 3])> = st.iter().filter(|(a, b)| (a[2] - b[2]).abs() < 1e-6 && a[2] > 11.0).collect();
    assert!(!flat.is_empty(), "{st:?}");
    for (a, b) in &flat {
        assert!((12.0 + 4.0 / 12.0 - 1e-3..=12.0 + 14.0 / 12.0 + 1e-3).contains(&a[2]), "on the deck band: {a:?} {b:?}");
    }
    assert!(st.iter().all(|(a, b)| a[2].max(b[2]) <= 12.0 + 3.0), "nothing a ceiling's height over the roof: {st:?}");
}
