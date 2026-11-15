//! Q6664: the feeder route follows the model's feed data. Two stacks of feed-through unit panels
//! (copies of one typical panel), one fed bottom-up from a distribution panel on the first floor,
//! the other top-down from one on the third, each source standing beside the OTHER stack. Each
//! copy's feed set on it (`fed_from`): conduit.reroute lays one feeder per stack from its breaker
//! and one link per feed-through in the feed's direction, never the nearest stack's, and a route
//! laid before the feed was set is routed again as the feed says. Made-up building only.

use std::collections::BTreeSet;

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

fn panel(tag: &str, level: &str, at: [f64; 2], fed_from: Option<&str>) -> Value {
    let mut p = json!({"tag": tag, "kind": "panel", "level": level, "min": [at[0] - 6.0, at[1] - 3.0], "max": [at[0] + 6.0, at[1] + 3.0],
        "height_ft": 3, "bottom_ft": 4});
    if let Some(f) = fed_from {
        p["properties"] = json!({"fed_from": f});
    }
    p
}

/// P1 (first floor, beside stack b), P2 (third floor, over stack a); stacks a (x 1000) and b
/// (x 3000) of copies U-1xx..U-3xx; `fed`: with each copy's feed set.
fn equipment(fed: bool) -> Value {
    let f = |v: &'static str| fed.then_some(v);
    json!([
        panel("P1", "L1", [3100.0, 100.0], None),
        panel("P2", "L3", [900.0, 100.0], None),
        panel("U-101", "L1", [1000.0, 600.0], f("P1")),
        panel("U-201", "L2", [1000.0, 600.0], f("U-101")),
        panel("U-301", "L3", [1000.0, 600.0], f("U-201")),
        panel("U-302", "L3", [3000.0, 600.0], f("P2")),
        panel("U-202", "L2", [3000.0, 600.0], f("U-302")),
        panel("U-102", "L1", [3000.0, 600.0], f("U-202")),
    ])
}

fn stacks(fed: bool) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Stacked Units", None);
    let feeder = |from: &str| {
        json!({"tag": "F6", "from": from, "to": "U", "sets": 1, "phases": 1, "size": "#2", "material": "cu", "neutrals": 1, "grounds": 1,
            "ground_size": "#8", "conduits": 1, "conduit_in": 1.25, "entry": "top", "source": format!("made-up feeder schedule; panel {from} schedule circuit 1")})
    };
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]},
            {"name": "L2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]},
            {"name": "L3", "elevation_ft": 20, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}
        ],
        "mep": {
            "systems": [{"name": "E-FEEDER", "layer": "E-FEEDER", "kind": "conduit", "size_in": [1.25, 0], "bottom_ft": 9}],
            "equipment": equipment(fed),
            "panel_schedules": [
                {"panel": "P1", "circuits": [{"number": "1", "description": "PANELBOARD U (3 UNITS)"}]},
                {"panel": "P2", "circuits": [{"number": "1", "description": "PANELBOARD U (3 UNITS)"}]},
                {"panel": "U", "typical_of": 6}
            ],
            "feeders": [feeder("P1"), feeder("P2")]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s
}

/// The chains the spec's feeder runs lay (`feeder F6 P1→U-101→U-201→U-301`).
fn chains(s: &mut Session) -> BTreeSet<String> {
    let sp = s.execute("bspec.get", &json!({})).expect("get")["spec"].clone();
    sp["mep"]["runs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r["system"] == "E-FEEDER")
        .filter_map(|r| r["source_sheet"].as_str().map(str::to_string))
        .collect()
}

/// Each copy's raceways in, by the chains: (feeders leaving the sources, links in the feed's
/// direction as (from, to)).
fn feeds(chains: &BTreeSet<String>) -> (usize, BTreeSet<(String, String)>) {
    let mut links = BTreeSet::new();
    for c in chains {
        let names: Vec<&str> = c.split_once(' ').map_or("", |x| x.1).split_once(' ').map_or("", |x| x.1).split('→').collect();
        for w in names.windows(2) {
            links.insert((w[0].to_string(), w[1].to_string()));
        }
    }
    (chains.len(), links)
}

fn as_fed() -> BTreeSet<(String, String)> {
    [("P1", "U-101"), ("U-101", "U-201"), ("U-201", "U-301"), ("P2", "U-302"), ("U-302", "U-202"), ("U-202", "U-102")]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

#[test]
fn reroute_lays_one_feeder_per_stack_and_each_feed_through_in_the_feeds_direction() {
    let mut s = stacks(true);
    let r = s.execute("conduit.reroute", &json!({"feeders": "all"})).expect("route all");
    eprintln!("conduit.reroute: {}", r["feeders"]);
    let laid = chains(&mut s);
    let (from_sources, links) = feeds(&laid);
    assert_eq!(from_sources, 2, "one feeder per stack from its breaker: {laid:#?}");
    assert_eq!(links, as_fed(), "{laid:#?}");
    // gear.list: each copy fed once, by the feeder of its own source.
    let g = s.execute("gear.list", &json!({})).expect("gear");
    let fed: Vec<&Value> = g["gear"].as_array().map(|v| v.iter().filter(|e| e["tag"].as_str().is_some_and(|t| t.starts_with("U-"))).collect()).unwrap_or_default();
    assert_eq!(fed.len(), 6);
    assert!(fed.iter().all(|e| e["feeders_in"].as_array().map_or(0, Vec::len) == 1), "{g}");
}

#[test]
fn a_route_laid_before_the_feed_was_set_is_routed_again_as_the_feed_says() {
    let mut s = stacks(false);
    s.execute("conduit.reroute", &json!({"feeders": "all"})).expect("route by the nearest stack");
    let (_, near) = feeds(&chains(&mut s));
    assert!(near.contains(&("P1".to_string(), "U-102".to_string())), "P1 took the stack beside it: {near:?}");
    // The panels' feed set on them: the route follows it at once (bspec.set), not both.
    let set = s.execute("bspec.set", &json!({"spec": {"mep": {"equipment": equipment(true)}}})).expect("feeds set");
    eprintln!("bspec.set: {}", set["feeders"]);
    assert_eq!(set["feeders"]["off_feed_before"], 2, "{set}");
    assert_eq!(set["feeders"]["off_feed_after"], 0, "{set}");
    let laid = chains(&mut s);
    assert_eq!(feeds(&laid), (2, as_fed()), "{laid:#?}");
    // conduit.reroute finds it followed: no route laid twice, none reversed.
    let r = s.execute("conduit.reroute", &json!({"feeders": "all"})).expect("again");
    assert_eq!((r["feeders"]["rerouted"].as_u64(), r["feeders"]["off_feed_before"].as_u64()), (Some(0), Some(0)), "{r}");
    assert_eq!(feeds(&chains(&mut s)), (2, as_fed()));
}
