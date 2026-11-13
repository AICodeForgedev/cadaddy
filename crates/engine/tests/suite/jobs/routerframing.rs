//! Q6622 fix-router-ignores-framing-hole-rules: the conduit router keeps to the framing's hole
//! rules, so `bim.clash` finds no conflict between the routed runs and the framing.

use std::collections::BTreeMap;

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

use crate::conduitroute::storey_walls;

fn device(id: &str, kind: &str, x: f64, y: f64, circuit: &str, mount_in: Option<f64>) -> Value {
    let mut f = json!({
        "type": {"value": "A", "source": "plan"},
        "symbol": {"value": id, "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
    });
    if !circuit.is_empty() {
        f["circuit"] = json!({"value": circuit, "source": "plan"});
    }
    if let Some(m) = mount_in {
        f["mount_height_in"] = json!({"value": m.to_string(), "source": "note"});
    }
    json!({"id": format!("device:L1:{id}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": f})
}

fn circuit(label: &str) -> Value {
    json!({"id": format!("circuit:L1:E-101:{label}"), "kind": "circuit", "level": "L1", "at_ft": [0.0, 0.0], "fields": {
        "circuit": {"value": label, "source": "plan"},
        "panel": {"value": "LP1", "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "conductors": {"value": "2#12 + 1#12G", "source": "default"},
        "conduit": {"value": "3/4\" EMT", "source": "default"},
        "route": {"value": "estimated route", "source": "rule"}
    }})
}

/// Two stacked storeys (10' floor to floor, 2x6 wood stud walls, the floor over L1 framed with
/// 11 7/8" I-joists at 24"); on L1 a ceiling light and its switch on opposite sides of the
/// room, receptacles along the walls, and the panel at the west wall: every homerun crosses the
/// joists.
fn framed() -> Session {
    let mut d = Drawing::new_imperial();
    storey_walls(&mut d, 0.0);
    storey_walls(&mut d, 200.0);
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    s.execute("project.rules.set", &json!({"domain": "joist_layout", "key": "general", "value": "24", "reason": "sample joist layout"}))
        .expect("rule");
    let exterior = json!({
        "tag": "X1", "description": "EXTERIOR WALL", "source": "sample legend, mark X1",
        "layers": [
            {"material": "1/2\" wood structural panel sheathing", "thickness_in": 0.5, "category": "sheathing", "side": "side1"},
            {"material": "2x6 wood studs @ 16\" o.c.", "thickness_in": 5.5, "category": "framing", "side": "core"},
            {"material": "1/2\" gypsum board", "thickness_in": 0.5, "category": "board", "side": "side2"}
        ]
    });
    let tags = vec![
        device("L1", "luminaire", 30.0, 12.0, "LP1-1", None),
        device("L2", "luminaire", 10.0, 12.0, "LP1-1", None),
        device("S1", "switch", 1.0, 12.0, "", Some(48.0)),
        device("R1", "receptacle", 39.0, 6.0, "LP1-3", Some(18.0)),
        device("R2", "receptacle", 39.0, 18.0, "LP1-3", Some(18.0)),
        device("R3", "receptacle", 20.0, 23.0, "LP1-3", Some(18.0)),
        circuit("LP1-1"),
        circuit("LP1-3"),
    ];
    let spec = json!({
        "version": 1, "units_per_foot": 12, "stack": true,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]},
            {"name": "L2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9, "region": [1900, -100, 3400, 1000], "base": [2400, 0]}
        ],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}],
        "wall_types": [{"name": "X1", "thickness_in": 7.25, "from": "pairs", "assembly": exterior}],
        "tags": tags,
        "mep": {"systems": [], "runs": [], "equipment": [
            {"tag": "LP1", "kind": "panel", "level": "L1", "min": [12, 120], "max": [32, 124], "height_ft": 4, "bottom_ft": 2.5,
             "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
        ]}
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({})).expect("build");
    assert!(built["tradePieces"].as_u64().unwrap_or(0) > 0, "the build lays out the trades' pieces: {built}");
    s
}

/// The framing conflicts `bim.clash` finds, by member kind and rule; and the allowed bores by
/// member kind.
fn conflicts(s: &mut Session) -> (Value, BTreeMap<String, usize>, BTreeMap<String, usize>) {
    let r = s.execute("bim.clash", &json!({"a": "framing"})).expect("clash");
    let mut by: BTreeMap<String, usize> = BTreeMap::new();
    for c in r["clashes"].as_array().into_iter().flatten().filter(|c| c["framing"]["verdict"] == json!("conflict")) {
        let kind = c["framing"]["member"]["kind"].as_str().unwrap_or("");
        *by.entry(format!("{kind}: {}", c["framing"]["rule"].as_str().unwrap_or(""))).or_default() += 1;
    }
    let mut bores: BTreeMap<String, usize> = BTreeMap::new();
    for b in r["bores"].as_array().into_iter().flatten() {
        *bores.entry(b["member"]["kind"].as_str().unwrap_or("").to_string()).or_default() += 1;
    }
    (r["framing"].clone(), by, bores)
}

/// Q6622: after `conduit.reroute`, the routed runs cross the floor framing over L1 only where the
/// members' hole rules allow: each bent stick judged by its legs (a stick that crosses a joist
/// with one leg and keeps to the bay with the other is no run along the joist, and its bend is no
/// 8" hole), every joist hole in the web's hole zone.
#[test]
fn routed_runs_keep_to_the_floor_framings_hole_rules() {
    let mut s = framed();
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    let rep = s.execute("conduit.route", &json!({"max": 2000})).expect("report");
    let fr = &rep["framing"];
    assert_eq!(fr["blocked_pieces"], json!(0), "{fr}");
    assert!(fr["bored_holes"].as_u64().unwrap_or(0) > 0, "the homeruns cross the joists: {fr}");
    assert!(fr["members_by_kind"]["rim"].as_u64().unwrap_or(0) > 0, "the router knows the rim boards: {fr}");
    let (clash, by, bores) = conflicts(&mut s);
    eprintln!("framing {clash}\nconflicts {by:#?}\nbores {bores:?}");
    let floor: BTreeMap<&String, &usize> = by.iter().filter(|(k, _)| !k.starts_with("stud:")).collect();
    assert!(floor.is_empty(), "the router's own runs break no joist, rim or truss rule: {floor:#?}");
    // Every hole the router bores, bim.clash allows.
    assert!(bores.get("joist").copied().unwrap_or(0) as u64 >= fr["bored_holes"].as_u64().unwrap_or(0), "{bores:?} {fr}");
}
