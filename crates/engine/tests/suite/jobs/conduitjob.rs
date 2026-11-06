//! Conduit and circuits (Q6007): `conduit.read` on an electrical plan with its devices in the
//! model reads the drawn conduit, the homerun and the panel, routes each circuit (an estimated
//! route, marked), and the takeoff has conduit, wire, boxes and fittings by level and circuit.

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// A made-up power plan, inches: panel P1 (a box lettered `PANEL P1`), receptacles R1–R3 on
/// circuit P1-2 with a homerun (and its `2#10, 1#10G` note) at R3, a switch S1 on P1-4 with no
/// homerun, and a tagged 1" conduit; the devices are in the model as `mep.read_devices` puts them.
fn plan(s: &mut Session) {
    s.execute("pline", &json!({"points": [[0, 0], [24, 0], [24, 12], [0, 12]], "closed": true})).expect("panel box");
    s.execute("text", &json!({"at": [0, 16], "text": "PANEL P1", "height": 6})).expect("panel tag");
    s.execute("line", &json!({"points": [[480, 240], [510, 270]]})).expect("homerun");
    s.execute("text", &json!({"at": [512, 268], "text": "P1-2", "height": 6})).expect("label");
    s.execute("text", &json!({"at": [512, 258], "text": "2#10, 1#10G", "height": 4})).expect("note");
    s.execute("line", &json!({"points": [[0, 600], [600, 600]]})).expect("conduit");
    s.execute("text", &json!({"at": [240, 604], "text": "1\"C", "height": 6})).expect("conduit tag");
    // A wall line, untagged: not conduit.
    s.execute("line", &json!({"points": [[-100, 900], [1500, 900]]})).expect("wall");
    let dev = |id: &str, kind: &str, ty: &str, x: f64, y: f64, ckt: &str| {
        json!({"id": format!("device:Level 1:{id}"), "kind": kind, "level": "Level 1", "at_ft": [x / 12.0, y / 12.0],
            "fields": {"type": {"value": ty, "source": "plan"}, "circuit": {"value": ckt, "source": "plan"}, "symbol": {"value": "", "source": "plan"}}})
    };
    let spec = json!({
        "units_per_foot": 12,
        "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9}],
        "tags": [
            dev("R1", "receptacle", "R", 240.0, 240.0, "P1-2"),
            dev("R2", "receptacle", "R", 240.0, 480.0, "P1-2"),
            dev("R3", "receptacle", "R", 480.0, 240.0, "P1-2"),
            dev("S1", "switch", "S", 960.0, 120.0, "P1-4"),
        ]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
}

fn lines<'a>(r: &'a Value, circuit: &str) -> Vec<&'a Value> {
    r["lines"].as_array().into_iter().flatten().filter(|l| l["circuit"] == circuit).collect()
}

#[test]
fn conduit_read_routes_each_circuit_and_the_takeoff_counts_it_by_circuit() {
    let mut s = Session::new();
    plan(&mut s);
    let r = s.execute("conduit.read", &json!({})).expect("conduit.read");
    assert_eq!(r["source"], json!("current drawing"), "{r:#}");
    assert_eq!((r["conduits"].clone(), r["circuits"].clone(), r["homeruns"]["read"].clone()), (json!(1), json!(2), json!(1)), "{r:#}");
    assert_eq!(r["homeruns"]["routed"], json!(2), "both circuits reach panel P1: {r:#}");
    assert_eq!(r["applied"], json!(false));
    // Not applied: the spec is as it was.
    let before = s.execute("bim.bom", &json!({"group_by": "circuit"})).expect("bom");
    assert!(lines(&before, "P1-2").iter().all(|l| l["category"] != "conduit"), "{before:#}");

    let r = s.execute("conduit.read", &json!({"apply": true})).expect("apply");
    assert_eq!(r["applied"], json!(true), "{r:#}");
    // Twice: replaced, not doubled.
    s.execute("conduit.read", &json!({"apply": true})).expect("apply again");
    let bom = s.execute("bim.bom", &json!({"group_by": "circuit"})).expect("bom");
    let p12 = lines(&bom, "P1-2");
    let get = |ls: &[&Value], item: &str, size: &str| -> f64 {
        ls.iter().filter(|l| l["item"] == item && (size.is_empty() || l["size"] == size)).filter_map(|l| l["net"].as_f64()).sum()
    };
    // P1-2: from R3 (the homerun's) to R1, R2; home run R3 → panel centre (12, 6): (480-12) + (240-6)
    // = 702"; chain 240 + 240 = 480"; three drops from 1'-6" to 9'-3" (just above the 9' ceiling).
    let conduit = get(&p12, "EMT conduit", "");
    let want = (702.0 + 480.0) / 12.0 + 3.0 * (9.25 - 1.5);
    assert!((conduit - want).abs() < 0.01, "{conduit} vs {want}: {p12:#?}");
    // Wire: the homerun note's 2#10 + 1#10G over the conduit plus 1' at each end of its 6 pulls.
    assert!((get(&p12, "THHN copper", "#10") - 2.0 * (want + 12.0)).abs() < 0.01, "{p12:#?}");
    assert!((get(&p12, "Copper ground, green", "#10") - (want + 12.0)).abs() < 0.01, "{p12:#?}");
    assert_eq!(get(&p12, "Box, 4\" square", ""), 3.0, "{p12:#?}");
    assert!(get(&p12, "EMT 90° elbow", "") >= 3.0, "a turn on the home run and one atop each drop: {p12:#?}");
    // P1-4: the switch's home run and drop, the default makeup.
    let p14 = lines(&bom, "P1-4");
    assert!(get(&p14, "THHN copper", "#12") > 0.0, "{p14:#?}");
    // The drawn 1" conduit: on no circuit, as modelled.
    let drawn: Vec<&Value> = bom["lines"].as_array().into_iter().flatten().filter(|l| l["item"] == "EMT conduit" && l["size"] == "1\"").collect();
    assert_eq!(drawn.len(), 1, "{bom:#}");
    assert_eq!((drawn[0]["net"].clone(), drawn[0]["status"].clone()), (json!(50.0), json!("model")));
    // Grouped by job, the circuits' conduit merges into one line per size.
    let job = s.execute("bim.bom", &json!({})).expect("bom");
    let total: f64 = job["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| l["item"] == "EMT conduit" && l["size"] == "3/4\"")
        .filter_map(|l| l["net"].as_f64())
        .sum();
    assert!((total - conduit - get(&p14, "EMT conduit", "")).abs() < 0.01);

    // The takeoff by circuit: a circuit column, a circuit filter.
    let t = s.execute("bim.takeoff", &json!({"group_by": "circuit", "filter": {"circuit": "P1-2"}, "keep": false})).expect("takeoff");
    let tl = t["lines"].as_array().cloned().unwrap_or_default();
    assert!(!tl.is_empty() && tl.iter().all(|l| l["circuit"] == "P1-2" && l["level"] == "Level 1"), "{t:#}");
    assert!(t["csv"].as_str().unwrap_or_default().starts_with("level,circuit,trade"), "{}", t["csv"]);
    assert!(s.execute("bim.bom", &json!({"group_by": "panel"})).is_err());
}

#[test]
fn a_device_with_no_circuit_gets_no_route_and_hostile_labels_are_ignored() {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [1200, 0]]})).expect("line");
    for (i, t) in ["E-101", "NOTE-1", "D-999", "P1-", "/3", "LP1-1,3,5,7"].iter().enumerate() {
        let x = 100.0 * i as f64;
        s.execute("line", &json!({"points": [[x, 100], [x + 30.0, 130]]})).expect("arrow");
        s.execute("text", &json!({"at": [x + 32.0, 128], "text": t, "height": 6})).expect("label");
    }
    let spec = json!({
        "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9}],
        "tags": [{"id": "device:Level 1:a", "kind": "receptacle", "level": "Level 1", "at_ft": [1, 1], "fields": {"type": {"value": "R", "source": "plan"}}}]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let r = s.execute("conduit.read", &json!({"apply": true})).expect("conduit.read");
    assert_eq!((r["circuits"].clone(), r["homeruns"]["read"].clone(), r["devices_without_circuit"].clone()), (json!(0), json!(0), json!(1)), "{r:#}");
    assert_eq!(r["applied"], json!(false), "nothing to put in: {r:#}");
}
