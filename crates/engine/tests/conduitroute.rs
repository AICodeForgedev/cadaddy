//! Q6152 conduitrouting: the circuits are routed through the model the build holds — square,
//! around the other trades, into each box and the panel's entry window — checked against the
//! code (fill, derating, voltage drop, box fill), and taken off from their pieces.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

/// A device tag: a receptacle at (x, y) feet on `circuit`.
fn receptacle(n: usize, x: f64, y: f64, circuit: &str) -> Value {
    json!({"id": format!("device:L1:{n}"), "kind": "receptacle", "level": "L1", "at_ft": [x, y], "fields": {
        "type": {"value": "A", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x1F0 + n), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "circuit": {"value": circuit, "source": "plan"},
        "mount_height_in": {"value": "18", "source": "note"}
    }})
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

/// A made-up one-storey building (12' floor to floor, 9' ceiling): panel LP1 with a 10" x 4" top
/// conduit area; circuit LP1-1's three receptacles east of it across a shallow supply duct the
/// conduit can pass over; circuit LP1-3's two receptacles south of a deep return duct that fills
/// the ceiling space (its homerun shares LP1-1's and taps across the duct); circuit LP1-5's one
/// receptacle 130' away.
fn building(extra: impl FnOnce(&mut Session)) -> (Session, Value) {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    extra(&mut s);
    let mut tags = vec![
        receptacle(1, 20.0, 10.0, "LP1-1"),
        receptacle(2, 30.0, 10.0, "LP1-1"),
        receptacle(3, 30.0, 25.0, "LP1-1"),
        receptacle(4, 20.0, -15.0, "LP1-3"),
        receptacle(5, 26.0, -15.0, "LP1-3"),
        receptacle(6, 130.0, 10.0, "LP1-5"),
    ];
    tags.extend([circuit("LP1-1"), circuit("LP1-3"), circuit("LP1-5")]);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": tags,
        "mep": {
            "systems": [
                {"name": "SA", "layer": "M-SA", "kind": "duct", "size_in": [12, 6], "bottom_ft": 9.25},
                {"name": "RA", "layer": "M-RA", "kind": "duct", "size_in": [24, 24], "bottom_ft": 9.0},
                {"name": "CW", "layer": "P-CW", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 8.0}
            ],
            "runs": [
                // A 6" deep supply duct along y at x = 25': the conduit goes over it.
                {"system": "SA", "level": "L1", "points": [[300, -60], [300, 400]], "size_in": [12, 6], "bottoms_ft": [9.25], "size_source": "tag", "height_source": "tag"},
                // A return duct 24" deep from 9' to 11', along x at y = -5' from 0 to 50': it fills
                // the ceiling space; nothing passes over it or round it within 8'.
                {"system": "RA", "level": "L1", "points": [[0, -60], [600, -60]], "size_in": [24, 24], "bottoms_ft": [9.0], "size_source": "tag", "height_source": "tag"},
                // A cold water pipe over the panel, in its dedicated space (to 6' above its top).
                {"system": "CW", "level": "L1", "points": [[-14, -120], [-14, -24]], "size_in": [2, 0], "bottoms_ft": [8.0], "size_source": "tag", "height_source": "tag"},
                {"system": "CW", "level": "L1", "points": [[-14, -24], [-14, 24]], "size_in": [2, 0], "bottoms_ft": [8.0], "size_source": "tag", "height_source": "tag"}
            ],
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5,
                 "properties": {"TOP CONDUIT AREA": "10 W x 4 D in, 40 sq in; derived: the box less 2 x 1.5 in margins (NEC 110.26(A))"},
                 "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({})).expect("build");
    (s, built)
}

fn count(s: &mut Session, q: Value) -> Value {
    s.execute("bim.count", &q).expect("count")
}

/// Every conduit stick's ends (feet).
fn sticks(s: &Session) -> Vec<(String, [f64; 3], [f64; 3])> {
    let d = s.doc().expect("doc");
    let xyz = |t: String| {
        let v: Vec<f64> = t.split(',').filter_map(|x| x.trim().parse::<f64>().ok()).collect();
        [v.first().copied().unwrap_or(0.0) / 12.0, v.get(1).copied().unwrap_or(0.0) / 12.0, v.get(2).copied().unwrap_or(0.0) / 12.0]
    };
    d.model
        .iter()
        .map(|e| cadaddy_engine::doc::props::read(&e.common.xdata))
        .filter(|p| p.get("atom").map(|x| x.text()).as_deref() == Some("conduit"))
        .map(|p| {
            (
                p.get("circuit").map(|x| x.text()).unwrap_or_default(),
                xyz(p.get("start").map(|x| x.text()).unwrap_or_default()),
                xyz(p.get("end").map(|x| x.text()).unwrap_or_default()),
            )
        })
        .collect()
}

#[test]
fn circuits_are_routed_square_around_the_ducts_and_into_every_box() {
    let (mut s, built) = building(|_| {});
    let cr = &built["conduitRoute"];
    assert_eq!(cr["circuits"], json!(3), "{built}");
    let r = s.execute("conduit.route", &json!({})).expect("report");
    assert_eq!(r["routed"], json!(true), "{r}");
    // No diagonal: every routed piece runs along x or y, or straight up (a stick's own ends can
    // differ in two axes: a field bend or an offset turns inside one stick).
    assert_eq!(r["diagonal_pieces"], json!(0), "{r}");
    let st = sticks(&s);
    assert!(!st.is_empty());
    // Every piece keeps its circuit; the homerun of LP1-1 and LP1-3 (their first devices within
    // 30' of each other) is one raceway carrying both: `LP1-1,3`.
    assert!(st.iter().all(|(c, _, _)| !c.is_empty()), "{st:?}");
    assert!(st.iter().any(|(c, _, _)| c == "LP1-1,3"), "{:?}", st.iter().map(|x| &x.0).collect::<std::collections::BTreeSet<_>>());
    let home_wires = count(&mut s, json!({"kind": "wire", "circuit": "LP1-1,3"}));
    assert_eq!(home_wires["count"], json!(5), "4#12 + 1#12G: {home_wires}");
    // Every receptacle has a raceway landing in its box, and its drop ends on the box's top.
    let devices: Vec<String> = {
        let d = s.doc().expect("doc");
        d.model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device")).map(|e| e.handle.hex()).collect()
    };
    assert_eq!(devices.len(), 6);
    for h in &devices {
        let el = s.execute("bim.element", &json!({"handle": h})).expect("element");
        assert!(el["fed_by"].as_array().is_some_and(|f| !f.is_empty()), "{h}: {el}");
    }
    let d = s.doc().expect("doc");
    let gaps = d.model.iter().filter(|e| cadaddy_engine::doc::props::read(&e.common.xdata).get("entry_gap_ft").is_some()).count();
    assert_eq!(gaps, 0, "every drop reaches its box");
    // Box fill checked at every box a raceway enters, against the box it has: none over here
    // (boxes chained in and out).
    assert_eq!(r["box_fill"]["checked"], json!(6), "{r}");
    assert_eq!(r["box_fill"]["over"], json!([]), "{r}");
    // Over the shallow supply duct: no clash there. The deep return duct can't be passed by
    // LP1-3's homerun tap: one clash, with both objects, the clearance and a fix.
    let all = r["clashes"].as_array().cloned().unwrap_or_default();
    // The pipe over the panel is in its dedicated space: a clash item of its own.
    let space: Vec<&Value> = all.iter().filter(|c| c["a"]["what"] == json!("gear dedicated space")).collect();
    assert_eq!(space.len(), 1, "{all:?}");
    assert_eq!((space[0]["b"]["system"].clone(), space[0]["a"]["gear"].clone()), (json!("CW"), json!("LP1")));
    assert!(space[0]["fix"].as_str().is_some_and(|f| f.contains("110.26(E)")), "{:?}", space[0]);
    assert_eq!(r["gear_space_clashes"], json!(1));
    let clashes: Vec<Value> = all.iter().filter(|c| c["a"]["what"] != json!("gear dedicated space")).cloned().collect();
    assert!(!clashes.is_empty(), "{r}");
    assert!(clashes.iter().all(|c| c["b"]["system"] == json!("RA")), "{clashes:?}");
    let c = &clashes[0];
    assert_eq!(c["kind"], json!("clash"));
    assert_eq!(c["b"]["category"], json!("duct"));
    assert_eq!(c["clearance_in"], json!(1.0));
    assert!(c["fix"].as_str().is_some_and(|f| f.contains("offset under it")), "{c}");
    assert_eq!(c["pair"], json!("electrical × mechanical"));
    assert!(r["clashes_by_pair_level"]["electrical × mechanical"]["L1"].as_u64().is_some_and(|n| n >= 1), "{r}");
    assert_eq!(r["clashes_by_pair_level"]["electrical × plumbing"]["L1"], json!(1), "{r}");
}

#[test]
fn the_homerun_lands_in_the_panels_top_entry_window_one_slot_each() {
    let (s, _) = building(|_| {});
    // LP1's top: 10" x 4" centred on its box (-24..-4, -6..6 in): x -19..-9 in.
    let tops: Vec<[f64; 3]> = sticks(&s)
        .into_iter()
        .flat_map(|(_, a, b)| [a, b])
        .filter(|p| (p[2] - 6.5).abs() < 0.2 && p[0] * 12.0 > -25.0 && p[0] * 12.0 < -3.0 && (p[1] * 12.0).abs() < 7.0)
        .collect();
    assert!(!tops.is_empty(), "a raceway ends on LP1's top");
    for p in &tops {
        assert!((-19.0..=-9.0).contains(&(p[0] * 12.0)), "inside the window: {p:?}");
    }
    // Two homeruns (LP1-1,3 and LP1-5), two slots, not the same point.
    let mut xs: Vec<i64> = tops.iter().map(|p| (p[0] * 120.0).round() as i64).collect();
    xs.sort_unstable();
    xs.dedup();
    assert!(xs.len() >= 2, "{tops:?}");
}

#[test]
fn a_long_homerun_is_upsized_for_voltage_drop_and_fill_sizes_the_raceway() {
    let (mut s, _) = building(|_| {});
    let r = s.execute("conduit.route", &json!({"section": "conductors_upsized"})).expect("report");
    let up = r["conductors_upsized"].as_array().cloned().unwrap_or_default();
    let five = up.iter().find(|u| u["circuit"] == json!("LP1-5")).cloned().unwrap_or_default();
    assert!(five["to"].as_str().is_some_and(|t| t.contains("#8")), "130' at 16 A: #8 for 3%: {r}");
    assert!(five["why"].as_str().is_some_and(|w| w.contains("210.19")), "{five}");
    let wires = count(&mut s, json!({"kind": "wire", "circuit": "LP1-5", "group_by": "size"}));
    assert!(wires["rows"].as_array().is_some_and(|rows| rows.iter().any(|x| x["size"] == json!("#8"))), "{wires}");
    // Fill: the shared homerun's four conductors and a ground (upsized for their lengths) in 3/4"
    // EMT, the minimum (a company default).
    let f = s.execute("conduit.route", &json!({"section": "fill"})).expect("fill");
    let shared = f["fill"].as_array().and_then(|a| a.iter().find(|x| x["circuits"] == json!(["LP1-1", "LP1-3"])).cloned()).unwrap_or_default();
    let parts: u64 =
        shared["conductors"].as_str().unwrap_or_default().split('+').filter_map(|p| p.trim().split('#').next()?.parse::<u64>().ok()).sum();
    assert_eq!(parts, 5, "{f}");
    assert_eq!(shared["size_in"], json!(0.75), "{f}");
    assert!(shared["source"].as_str().is_some_and(|x| x.contains("Chapter 9")), "{shared}");
}

#[test]
fn the_wiring_method_comes_from_the_specifications_table_by_area() {
    // The specification lists MC for concealed branch circuits only (no raceway): the routed
    // circuits are MC cable, with the table as the source.
    let (mut s, _) = building(|s| {
        let methods = json!({"applications": [
            {"section": "26 05 19", "item": "A", "application": "Feeders Concealed in Ceilings and Walls", "methods": ["raceway"]},
            {"section": "26 05 19", "item": "C", "application": "Branch Circuits Concealed in Ceilings, Walls, and Partitions", "methods": ["mc"]}
        ]});
        if let Ok(d) = s.doc_mut() {
            d.embedded.insert("route/methods.json".into(), std::sync::Arc::new(serde_json::to_vec(&methods).unwrap_or_default()));
        }
    });
    let r = s.execute("conduit.route", &json!({"section": "methods"})).expect("methods");
    let m = r["methods"].as_array().cloned().unwrap_or_default();
    assert!(m.iter().any(|x| x["area_method"] == json!("concealed: MC") && x["source"].as_str().is_some_and(|t| t.contains("26 05 19 C"))), "{r}");
    let cable = count(&mut s, json!({"kind": "cable", "circuit": "LP1-1"}));
    assert!(cable["count"].as_u64().unwrap_or(0) >= 3, "MC cable from box to box: {cable}");
    assert_eq!(count(&mut s, json!({"kind": "conduit", "circuit": "LP1-1"}))["count"], json!(0));
}

#[test]
fn the_takeoff_comes_from_the_pieces_and_is_written_with_their_handles() {
    let (mut s, _) = building(|_| {});
    let dir = std::env::temp_dir().join(format!("cadaddy-conduit-takeoff-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("Sample Floor Plan-conduit-takeoff.csv");
    let t =
        s.execute("conduit.takeoff", &json!({"group_by": ["level", "circuit"], "file": file.to_string_lossy(), "price": false})).expect("takeoff");
    let lines = t["lines"].as_array().cloned().unwrap_or_default();
    // Conduit feet equal the model's count of its sticks.
    let ft: f64 = lines.iter().filter(|l| l["kind"] == json!("conduit")).map(|l| l["qty"].as_f64().unwrap_or(0.0)).sum();
    let counted = count(&mut s, json!({"kind": "conduit"}))["length_ft"].as_f64().unwrap_or(0.0);
    assert!(ft > 0.0 && (ft - counted).abs() <= 0.01 * counted + 0.05, "{ft} vs {counted}");
    for k in ["coupling", "connector", "box", "strap", "wire"] {
        assert!(lines.iter().any(|l| l["kind"] == json!(k)), "{k}: {lines:?}");
    }
    // Wire by size and colour; each line names its elements.
    assert!(lines.iter().any(|l| l["kind"] == json!("wire") && l["type"].as_str().is_some_and(|x| x.contains("white"))), "{lines:?}");
    assert!(lines.iter().all(|l| l["elements"].as_u64().unwrap_or(0) > 0 && l["handles"].as_array().is_some_and(|h| !h.is_empty())));
    let csv = std::fs::read_to_string(&file).expect("csv written");
    assert!(csv.starts_with("level,area,circuit,kind,item,size,type,qty,unit,material,labor_dollars,labor_hours,labor_basis,via,elements,handles"));
    assert_eq!(csv.lines().count(), lines.len() + 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_cost_db_prices_labor_in_dollars_and_shows_the_hours() {
    let (mut s, _) = building(|_| {});
    let dir = std::env::temp_dir().join(format!("cadaddy-conduit-costdb-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let db = dir.join("costs.csv");
    std::fs::write(&db, "item,size,unit,material_unit,labor_unit\nEMT conduit,3/4\",ft,0.80,2.60\n").expect("write");
    let t = s.execute("conduit.takeoff", &json!({"cost_db": db.to_string_lossy(), "labor_rate": 65.0})).expect("takeoff");
    let emt = t["lines"]
        .as_array()
        .and_then(|a| a.iter().find(|l| l["kind"] == json!("conduit") && l["size"] == json!("3/4\"")).cloned())
        .unwrap_or_default();
    let qty = emt["qty"].as_f64().unwrap_or(0.0);
    assert!(qty > 0.0, "{t}");
    // $2.60 of labor a foot is 0.04 h a foot at $65/h.
    assert!((emt["labor_dollars"].as_f64().unwrap_or(0.0) - 2.6 * qty).abs() < 0.02, "{emt}");
    assert!((emt["labor_hours"].as_f64().unwrap_or(0.0) - 2.6 * qty / 65.0).abs() < 0.002, "{emt}");
    assert!(emt["labor_basis"].as_str().is_some_and(|b| b.contains("dollars") && b.contains("65")), "{emt}");
    assert!(s.execute("conduit.takeoff", &json!({"cost_db": dir.join("missing.csv").to_string_lossy()})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn routing_can_be_switched_off_and_hostile_params_never_panic() {
    let (mut s, _) = building(|_| {});
    let off = s.execute("model.build", &json!({"route": false})).expect("build");
    assert_eq!(off["conduitRoute"]["circuits"], Value::Null, "{}", off["conduitRoute"]);
    assert!(count(&mut s, json!({"kind": "conduit", "circuit": "LP1-1,3"}))["count"] == json!(0));
    for p in [json!({"section": 5}), json!({"section": "nope"}), json!({"max": -1}), json!({"max": 1e300})] {
        let _ = s.execute("conduit.route", &p);
    }
    for p in [json!({"group_by": 3}), json!({"group_by": ["room"]}), json!({"labor_rate": "x"}), json!({"file": ""}), json!({"cost_db": 7})] {
        let _ = s.execute("conduit.takeoff", &p);
    }
    assert!(s.execute("model.build", &json!({"route": "yes"})).is_err());
}

#[test]
fn rerouting_after_the_build_works_on_stacked_levels_and_is_stable() {
    // Two storeys drawn side by side (L2's plan 200' east), stacked by the build: L2's circuit
    // and panel move onto L1's plan; the reroute routes on L2's own plan and cuts the pieces where
    // the stacked model has them.
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let dev = |n: usize, level: &str, x: f64, y: f64, c: &str| {
        json!({"id": format!("device:{level}:{n}"), "kind": "receptacle", "level": level, "at_ft": [x, y], "fields": {
            "type": {"value": "A", "source": "plan"}, "symbol": {"value": format!("{:X}", 0x2F0 + n), "source": "plan"},
            "circuit": {"value": c, "source": "plan"}, "mount_height_in": {"value": "18", "source": "note"}}})
    };
    let ckt = |level: &str, label: &str, panel: &str| {
        json!({"id": format!("circuit:{level}:E-1:{label}"), "kind": "circuit", "level": level, "at_ft": [0.0, 0.0], "fields": {
            "circuit": {"value": label, "source": "plan"}, "panel": {"value": panel, "source": "plan"},
            "conductors": {"value": "2#12 + 1#12G", "source": "default"}}})
    };
    let spec = json!({
        "version": 1, "units_per_foot": 12, "stack": true,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]},
            {"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "base": [2400, 0]}
        ],
        "tags": [dev(1, "L1", 20.0, 10.0, "P1-1"), dev(2, "L1", 30.0, 10.0, "P1-1"), dev(3, "L2", 220.0, 10.0, "P2-1"), dev(4, "L2", 230.0, 10.0, "P2-1"),
                 ckt("L1", "P1-1", "P1"), ckt("L2", "P2-1", "P2")],
        "mep": {"equipment": [
            {"tag": "P1", "kind": "panel", "level": "L1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5},
            {"tag": "P2", "kind": "panel", "level": "L2", "min": [2376, -6], "max": [2396, 6], "height_ft": 4, "bottom_ft": 2.5}
        ]}
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({})).expect("build");
    assert_eq!(built["conduitRoute"]["circuits"], json!(2), "{built}");
    let before = count(&mut s, json!({"kind": "conduit"}));
    let r1 = s.execute("conduit.reroute", &json!({})).expect("reroute");
    assert_eq!(r1["circuits"], json!(2), "{r1}");
    assert_eq!(r1["unrouted"], json!(0), "{r1}");
    let after = count(&mut s, json!({"kind": "conduit"}));
    assert_eq!(after["count"], before["count"], "{before} {after}");
    assert!((after["length_ft"].as_f64().unwrap_or(0.0) - before["length_ft"].as_f64().unwrap_or(0.0)).abs() < 0.05);
    // Every device still has its raceway landing in its box (L2's on the stacked plan).
    let d = s.doc().expect("doc");
    let devices: Vec<String> =
        d.model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device")).map(|e| e.handle.hex()).collect();
    assert_eq!(devices.len(), 4);
    assert_eq!(d.model.iter().filter(|e| cadaddy_engine::doc::props::read(&e.common.xdata).get("entry_gap_ft").is_some()).count(), 0);
    for h in &devices {
        let el = s.execute("bim.element", &json!({"handle": h})).expect("element");
        assert!(el["fed_by"].as_array().is_some_and(|f| !f.is_empty()), "{h}: {el}");
    }
    // Again: the same, no doubled homerun tags.
    let r2 = s.execute("conduit.reroute", &json!({})).expect("again");
    assert_eq!((r2["atoms"].clone(), r2["routed_ft"].clone()), (r1["atoms"].clone(), r1["routed_ft"].clone()));
    let spec = s.execute("bspec.get", &json!({})).expect("spec");
    let homes = spec["spec"]["tags"]
        .as_array()
        .or_else(|| spec["tags"].as_array())
        .map_or(0, |t| t.iter().filter(|t| t["id"].as_str().is_some_and(|i| i.contains(":home:"))).count());
    assert_eq!(homes, 2, "one homerun per panel");
}

#[test]
fn a_typical_units_circuit_goes_to_each_units_own_panel() {
    // Two units 40' apart, each with its own panel of the typical schedule G (G-1A, G-1B), and
    // circuit G-1 printed in both: two circuits in fact, each routed to its own unit's panel.
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let mut tags =
        vec![receptacle(1, 5.0, 5.0, "G-1"), receptacle(2, 12.0, 5.0, "G-1"), receptacle(3, 45.0, 5.0, "G-1"), receptacle(4, 52.0, 5.0, "G-1")];
    let mut c = circuit("G-1");
    c["fields"]["panel"] = json!({"value": "G", "source": "plan"});
    tags.push(c);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": tags,
        "mep": {"equipment": [
            {"tag": "G-1A", "kind": "panel", "level": "L1", "min": [0, 100], "max": [14, 104], "height_ft": 2, "bottom_ft": 4.5},
            {"tag": "G-1B", "kind": "panel", "level": "L1", "min": [480, 100], "max": [494, 104], "height_ft": 2, "bottom_ft": 4.5}
        ]}
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let r = s.execute("conduit.route", &json!({})).expect("report");
    assert_eq!(r["homeruns"], json!(2), "{r}");
    assert_eq!(r["routed_circuits"], json!(2), "{r}");
    // No joint between the units: each part is its 7' joint (up from one box at 9'-3", down into
    // the next: about 22') and its homerun (about 18'), some 81' in all; one chain through both
    // units would add the 33' between them and two more drops (some 110').
    assert!(r["routed_ft"].as_f64().is_some_and(|f| f > 70.0 && f < 95.0), "{}", r["routed_ft"]);
    // Their panels' boxes give no conduit area: the top less its margins, asked about.
    assert_eq!(r["entry_unknown"], json!(["G-1A on L1", "G-1B on L1"]), "{r}");
    // Rerouting from the stored parts keeps them apart.
    let again = s.execute("conduit.reroute", &json!({})).expect("reroute");
    assert_eq!((again["homeruns"].clone(), again["circuits"].clone()), (json!(2), json!(2)), "{again}");
}

#[test]
fn the_panel_schedules_load_sets_the_voltage_drop() {
    // LP1-5 is 130' long: at 80% of a 20 A breaker it needs #8; its schedule says 500 VA (about
    // 4 A), for which #12 holds the drop under 3%.
    let (mut s, _) = building(|_| {});
    let sched = json!({"panel_schedules": [{"panel": "LP1", "source": "E-101", "circuits": [
        {"number": "5", "description": "RECEPTACLES", "breaker": "20", "poles": 1, "load_va": 500.0, "source": "E-101"}
    ]}]});
    let mut spec = s.execute("bspec.get", &json!({})).expect("spec");
    let spec = spec.get_mut("spec").cloned().unwrap_or(spec);
    let mut spec = spec;
    spec["mep"]["panel_schedules"] = sched["panel_schedules"].clone();
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("set");
    s.execute("model.build", &json!({})).expect("build");
    let r = s.execute("conduit.route", &json!({"section": "conductors_upsized"})).expect("report");
    let five = r["conductors_upsized"].as_array().and_then(|a| a.iter().find(|u| u["circuit"] == json!("LP1-5")).cloned());
    assert!(five.is_none(), "500 VA over 130': #12 is enough: {r}");
    let wires = count(&mut s, json!({"kind": "wire", "circuit": "LP1-5", "group_by": "size"}));
    assert!(wires["rows"].as_array().is_some_and(|rows| rows.iter().all(|x| x["size"] == json!("#12"))), "{wires}");
}

/// Every conduit stick's ends (feet) with its circuit, filtered by circuit.
fn sticks_of<'a>(st: &'a [(String, [f64; 3], [f64; 3])], circuit: &str) -> Vec<&'a (String, [f64; 3], [f64; 3])> {
    st.iter().filter(|(c, _, _)| c == circuit).collect()
}

#[test]
fn a_circuit_whose_panel_is_on_the_storey_below_rises_through_the_rated_floor_into_its_top() {
    // Two storeys drawn side by side (L2's plan 200' east), stacked by the build. Panel P1 is on
    // L1 only; L2's circuit P1-3 is fed from it: its homerun runs on L2 to the point over P1,
    // drops through L2's floor (a 1-hour floor/ceiling: L1's ceiling there is rated) into P1's
    // top entry window.
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let dev = |n: usize, level: &str, x: f64, y: f64, c: &str| {
        json!({"id": format!("device:{level}:{n}"), "kind": "receptacle", "level": level, "at_ft": [x, y], "fields": {
            "type": {"value": "A", "source": "plan"}, "symbol": {"value": format!("{:X}", 0x3F0 + n), "source": "plan"},
            "circuit": {"value": c, "source": "plan"}, "mount_height_in": {"value": "18", "source": "note"}}})
    };
    let ckt = |level: &str, label: &str| {
        json!({"id": format!("circuit:{level}:E-1:{label}"), "kind": "circuit", "level": level, "at_ft": [0.0, 0.0], "fields": {
            "circuit": {"value": label, "source": "plan"}, "panel": {"value": "P1", "source": "plan"},
            "conductors": {"value": "2#12 + 1#12G", "source": "default"}}})
    };
    let sq = |x0: f64, y0: f64, x1: f64, y1: f64| json!([[x0, y0], [x1, y0], [x1, y1], [x0, y1]]);
    let spec = json!({
        "version": 1, "units_per_foot": 12, "stack": true,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]},
            {"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "base": [2400, 0]}
        ],
        "tags": [dev(1, "L1", 20.0, 10.0, "P1-1"), dev(2, "L1", 30.0, 10.0, "P1-1"), dev(3, "L2", 220.0, 10.0, "P1-3"), dev(4, "L2", 230.0, 10.0, "P1-3"),
                 ckt("L1", "P1-1"), ckt("L2", "P1-3")],
        "ceiling_types": [{"name": "C1", "kind": "gypsum", "fire_rating": "1 HR", "layers": []}],
        "rcp": {"ceilings": [{"level": "L1", "outline": sq(-240.0, -240.0, 480.0, 480.0), "height_ft": 9, "ceiling_type": "C1"}]},
        "mep": {"equipment": [
            {"tag": "P1", "kind": "panel", "level": "L1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5,
             "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
        ]}
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let rr = s.execute("conduit.reroute", &json!({})).expect("reroute");
    assert_eq!(rr["circuits"], json!(2), "{rr}");
    assert_eq!(rr["unrouted"], json!(0), "the riser is routed: {rr}");
    let r = s.execute("conduit.route", &json!({})).expect("report");
    let risers = r["risers"].as_array().cloned().unwrap_or_default();
    assert_eq!(risers.len(), 1, "{r}");
    assert_eq!((risers[0]["panel"].clone(), risers[0]["panel_level"].clone(), risers[0]["level"].clone()), (json!("P1"), json!("L1"), json!("L2")));
    assert_eq!((risers[0]["floors"].clone(), risers[0]["side"].clone()), (json!(["L2"]), json!("top")));
    // L2's floor penetration, rated from L1's ceiling under it, with its firestop.
    let floors: Vec<Value> = r["penetrations"].as_array().into_iter().flatten().filter(|p| p.get("floor").is_some()).cloned().collect();
    assert_eq!(floors.len(), 1, "{r}");
    assert_eq!((floors[0]["floor"].clone(), floors[0]["rating"].clone()), (json!("L2"), json!("1 HR")));
    assert!(floors[0]["firestop"].as_str().is_some_and(|f| f.contains("through the L2 floor")), "{}", floors[0]);
    // The pieces: P1-3's raceway reaches P1's top (stacked plan: x -2'..-0.33', y ±0.5', top 6.5')
    // and one stick passes the 12' floor.
    let st = sticks(&s);
    let p13 = sticks_of(&st, "P1-3");
    assert!(!p13.is_empty(), "{:?}", st.iter().map(|x| &x.0).collect::<std::collections::BTreeSet<_>>());
    let at_panel = |p: &[f64; 3]| (-2.01..=-0.32).contains(&p[0]) && (-0.51..=0.51).contains(&p[1]) && (p[2] - 6.5).abs() < 0.05;
    assert!(p13.iter().any(|(_, a, b)| at_panel(a) || at_panel(b)), "{p13:?}");
    assert!(p13.iter().any(|(_, a, b)| a[2].min(b[2]) < 12.0 && a[2].max(b[2]) > 12.0), "{p13:?}");
    // Every device fed; the takeoff has the floor's firestop.
    let devices: Vec<String> = {
        let d = s.doc().expect("doc");
        d.model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device")).map(|e| e.handle.hex()).collect()
    };
    assert_eq!(devices.len(), 4);
    for h in &devices {
        let el = s.execute("bim.element", &json!({"handle": h})).expect("element");
        assert!(el["fed_by"].as_array().is_some_and(|f| !f.is_empty()), "{h}: {el}");
    }
    let tk = s.execute("conduit.takeoff", &json!({})).expect("takeoff");
    let fs: Vec<&Value> =
        tk["lines"].as_array().into_iter().flatten().filter(|l| l["kind"] == json!("firestop") && l["type"] == json!("1 HR")).collect();
    assert_eq!(fs.len(), 1, "{tk}");
    // Rerouting again is stable.
    let rr2 = s.execute("conduit.reroute", &json!({})).expect("again");
    assert_eq!((rr2["atoms"].clone(), rr2["routed_ft"].clone()), (rr["atoms"].clone(), rr["routed_ft"].clone()));
}

#[test]
fn a_drop_blocked_over_its_box_rises_along_the_wall_and_one_that_cant_is_a_clash() {
    // Three receptacles on LP1-1. A pipe runs along x right over the first, at the ceiling (in
    // its drop's way): that drop moves along the wall to clear it. A wide duct hangs low over the
    // third: no clear spot within 2' of it, so its drop is a clash item.
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    // A fourth sits in the wall between the panel and a second panel LP2 (2' off), inside LP2's
    // working space: its drop is in the wall, not in that space.
    let tags = vec![
        receptacle(1, 20.0, 10.0, "LP1-1"),
        receptacle(2, 30.0, 10.0, "LP1-1"),
        receptacle(3, 60.0, 10.0, "LP1-1"),
        receptacle(4, 1.0, 0.0, "LP1-1"),
        circuit("LP1-1"),
    ];
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": tags,
        "mep": {
            "systems": [
                {"name": "CW", "layer": "P-CW", "kind": "pipe", "size_in": [2, 0], "bottom_ft": 9.0},
                {"name": "SA", "layer": "M-SA", "kind": "duct", "size_in": [72, 12], "bottom_ft": 8.5}
            ],
            "runs": [
                {"system": "CW", "level": "L1", "points": [[180, 120], [300, 120]], "size_in": [2, 0], "bottoms_ft": [9.0], "size_source": "tag", "height_source": "tag"},
                {"system": "SA", "level": "L1", "points": [[660, 120], [780, 120]], "size_in": [72, 12], "bottoms_ft": [8.5], "size_source": "tag", "height_source": "tag"}
            ],
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5,
                 "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}},
                {"tag": "LP2", "kind": "panel", "level": "L1", "min": [36, -6], "max": [56, 6], "height_ft": 4, "bottom_ft": 2.5}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let r = s.execute("conduit.route", &json!({})).expect("report");
    assert!(r["drops_moved_along_wall"].as_u64().is_some_and(|n| n >= 1), "{r}");
    let all = r["clashes"].as_array().cloned().unwrap_or_default();
    // No clash with the pipe: the first box's drop stands clear of it.
    assert!(!all.iter().any(|c| c["b"]["system"] == json!("CW")), "{all:?}");
    assert!(!all.iter().any(|c| c["b"]["category"] == json!("working space")), "{all:?}");
    // The third box's drop under the wide duct: a clash item of kind `drop`, with a fix.
    let drops: Vec<&Value> = all.iter().filter(|c| c["a"]["what"] == json!("drop")).collect();
    assert!(!drops.is_empty(), "{all:?}");
    assert!(drops.iter().all(|c| c["b"]["system"] == json!("SA") && c["b"]["category"] == json!("duct")), "{drops:?}");
    assert!(drops[0]["fix"].as_str().is_some_and(|f| f.contains("along its wall")), "{}", drops[0]);
    assert!((drops[0]["at"][0].as_f64().unwrap_or(0.0) - 720.0).abs() < 1.0, "at the third box: {}", drops[0]);
    // The moved drop: from the first box (x 20', y 10') the raceway jogs in the wall off the
    // pipe's line (y 10') before it rises: every stick leaving that box ends off it.
    let st = sticks(&s);
    let at_box = |p: &[f64; 3]| (p[0] - 20.0).abs() < 0.01 && (p[1] - 10.0).abs() < 0.01 && p[2] < 3.0;
    let leaving: Vec<[f64; 3]> = st
        .iter()
        .filter_map(|(_, a, b)| {
            if at_box(a) {
                Some(*b)
            } else if at_box(b) {
                Some(*a)
            } else {
                None
            }
        })
        .collect();
    assert!(!leaving.is_empty(), "{st:?}");
    assert!(leaving.iter().all(|p| (p[1] - 10.0).abs() >= 0.4), "{leaving:?}");
    // Still square, every box fed, no drop short of its box.
    assert_eq!(r["diagonal_pieces"], json!(0), "{r}");
    let d = s.doc().expect("doc");
    assert_eq!(d.model.iter().filter(|e| cadaddy_engine::doc::props::read(&e.common.xdata).get("entry_gap_ft").is_some()).count(), 0);
}

/// A line on `layer` from `a` to `b` (feet).
fn wall_line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| cadaddy_engine::geom::Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = d.new_handle();
    let mut e = cadaddy_engine::doc::Entity::new(h, cadaddy_engine::doc::EntityKind::Line(cadaddy_engine::doc::Line { a: p(a), b: p(b) }));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
}

/// A mesh's plan box (feet): [x0, y0, x1, y1].
fn plan_box(e: &cadaddy_engine::doc::Entity) -> [f64; 4] {
    let cadaddy_engine::doc::EntityKind::Mesh(m) = &e.kind else { return [f64::NAN; 4] };
    m.vertices.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, v| {
        [b[0].min(v.x / 12.0), b[1].min(v.y / 12.0), b[2].max(v.x / 12.0), b[3].max(v.y / 12.0)]
    })
}

#[test]
fn a_device_set_in_its_wall_again_after_a_merge_takes_its_box_and_its_drop_along() {
    // Two rooms, a 6" partition between them (19.75'–20.25'). A receptacle on LP1-1 by the
    // partition's west face; the panel in the west room. The job's identity merge then keeps a
    // copy of the partition 12" west of it (the same wall read off two sheets): the device goes in
    // the wall there now, and its box and the raceway's drop must go with it, not stay in the room.
    let mut d = Drawing::new_imperial();
    let outer = [((0., 0.), (40.5, 0.)), ((40.5, 0.), (40.5, 10.)), ((40.5, 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let west = [((0.5, 0.5), (19.75, 0.5)), ((19.75, 0.5), (19.75, 9.5)), ((19.75, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    let east = [((20.25, 0.5), (40.0, 0.5)), ((40.0, 0.5), (40.0, 9.5)), ((40.0, 9.5), (20.25, 9.5)), ((20.25, 9.5), (20.25, 0.5))];
    for (a, b) in outer.into_iter().chain(west).chain(east) {
        wall_line(&mut d, a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": [receptacle(1, 19.6, 5.0, "LP1-1"), circuit("LP1-1")],
        "mep": {"systems": [], "runs": [], "equipment": [
            {"tag": "LP1", "kind": "panel", "level": "L1", "min": [12, 7], "max": [32, 11], "height_ft": 4, "bottom_ft": 2.5,
             "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
        ]}
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let dev = {
        let d = s.doc().expect("doc");
        d.model.iter().find(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device")).map(|e| e.handle).expect("device")
    };
    let host = |s: &Session| {
        let d = s.doc().expect("doc");
        let ps = cadaddy_engine::doc::props::read(&d.model.get(dev).expect("device").common.xdata);
        ps.get("host_wall").map(|p| p.text()).unwrap_or_default()
    };
    let wall = cadaddy_engine::doc::Handle::parse_hex(host(&s).trim()).expect("a host wall");
    // The box of the device, and the conduit ends at it (feet).
    let box_of = |s: &Session| -> [f64; 4] {
        let d = s.doc().expect("doc");
        d.model
            .iter()
            .find(|e| {
                let ps = cadaddy_engine::doc::props::read(&e.common.xdata);
                ps.get("atom").map(|p| p.text()).as_deref() == Some("box") && ps.get("box_of").map(|p| p.text()) == Some(dev.hex())
            })
            .map(|e| plan_box(e))
            .expect("its box")
    };
    let centre = |b: [f64; 4]| ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
    let (dx0, _) = centre(plan_box(s.doc().expect("doc").model.get(dev).expect("device")));
    let (bx0, _) = centre(box_of(&s));
    assert!((bx0 - dx0).abs() < 0.25, "built: the box behind its device ({bx0} vs {dx0})");
    // The merge: the partition replaced by its copy 12" west.
    {
        let d = s.doc_mut().expect("doc");
        let mut e = (**d.entity(wall).expect("wall")).clone();
        if let cadaddy_engine::doc::EntityKind::Mesh(m) = &mut e.kind {
            m.vertices.iter_mut().for_each(|v| v.x -= 12.0);
        }
        e.handle = d.new_handle();
        d.model.push(e);
        d.model.remove(wall);
    }
    let r = s.execute("bim.props.fill", &json!({})).expect("fill");
    assert_eq!(r["reseated"], json!(1), "{r:#}");
    let (dx, dy) = centre(plan_box(s.doc().expect("doc").model.get(dev).expect("device")));
    assert!((dx - dx0).abs() > 0.3, "the device moved with its wall: {dx0} → {dx}");
    let b = box_of(&s);
    let (bx, by) = centre(b);
    assert!(
        ((bx - bx0) - (dx - dx0)).abs() < 0.05 && (by - dy).abs() < 0.25,
        "its box went with it: box {bx0} → ({bx}, {by}), device {dx0} → ({dx}, {dy})"
    );
    // The raceway still lands on the box: a stick ends over it, and none ends where the box was.
    let st = sticks(&s);
    let on = |p: &[f64; 3], x: f64| (p[0] - x).abs() < 0.1 && (p[1] - by).abs() < 0.1 && p[2] < 3.0;
    assert!(st.iter().any(|(_, a, e)| on(a, bx) || on(e, bx)), "a stick ends at the box: {st:?}");
    assert!(!st.iter().any(|(_, a, e)| on(a, bx0) || on(e, bx0)), "no stick left at the box's old spot: {st:?}");
    let left = s.doc().expect("doc").model.iter().filter(|e| cadaddy_engine::doc::props::read(&e.common.xdata).get("entry_gap_ft").is_some()).count();
    assert_eq!(left, 0, "every drop reaches its box");
}
