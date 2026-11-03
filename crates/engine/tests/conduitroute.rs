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
                {"name": "RA", "layer": "M-RA", "kind": "duct", "size_in": [24, 24], "bottom_ft": 9.0}
            ],
            "runs": [
                // A 6" deep supply duct along y at x = 25': the conduit goes over it.
                {"system": "SA", "level": "L1", "points": [[300, -60], [300, 400]], "size_in": [12, 6], "bottoms_ft": [9.25], "size_source": "tag", "height_source": "tag"},
                // A return duct 24" deep from 9' to 11', along x at y = -5' from 0 to 50': it fills
                // the ceiling space; nothing passes over it or round it within 8'.
                {"system": "RA", "level": "L1", "points": [[0, -60], [600, -60]], "size_in": [24, 24], "bottoms_ft": [9.0], "size_source": "tag", "height_source": "tag"}
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
    // Over the shallow supply duct: no clash there. The deep return duct can't be passed by
    // LP1-3's homerun tap: one clash, with both objects, the clearance and a fix.
    let clashes = r["clashes"].as_array().cloned().unwrap_or_default();
    assert!(!clashes.is_empty(), "{r}");
    assert!(clashes.iter().all(|c| c["b"]["system"] == json!("RA")), "{clashes:?}");
    let c = &clashes[0];
    assert_eq!(c["kind"], json!("clash"));
    assert_eq!(c["b"]["category"], json!("duct"));
    assert_eq!(c["clearance_in"], json!(1.0));
    assert!(c["fix"].as_str().is_some_and(|f| f.contains("offset under it")), "{c}");
    assert_eq!(c["pair"], json!("electrical × mechanical"));
    assert!(r["clashes_by_pair_level"]["electrical × mechanical"]["L1"].as_u64().is_some_and(|n| n >= 1), "{r}");
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
