//! Q6157 mepatoms: the electrical runs are built as the pieces a crew installs (sticks,
//! couplings, connectors, straps, boxes, cable, wire), each an element that knows what it
//! connects to, and counts are lookups in the model's stored index.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// A made-up one-storey building: panel LP1 (fed from MDP), a drawn 37' run of 1" conduit, and
/// circuit LP1-3's estimated route from LP1 to a receptacle: 20'-8" along the ceiling and a
/// drop to the box (25' in all).
fn building(wiring: Option<&str>) -> Session {
    building_with(wiring, "receptacle").0
}

/// [`building`] with the device at the route's end of `kind`; the build's report beside it.
fn building_with(wiring: Option<&str>, kind: &str) -> (Session, Value) {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    if let Some(w) = wiring {
        s.execute("project.rules.set", &json!({"domain": "wiring_method", "key": "general", "value": w, "tier": "spec", "source": "Spec 26 05 19", "reason": "the specification's wiring method"})).expect("rule");
    }
    let circuit = "circuit:L1:E-101:LP1-3";
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": [
            {"id": "device:L1:20.7,10.0", "kind": kind, "level": "L1", "at_ft": [248.0 / 12.0, 10.0], "fields": {
                "type": {"value": "A", "source": "plan"},
                "symbol": {"value": "1F0", "source": "plan"},
                "sheet": {"value": "E-101", "source": "plan"},
                "circuit": {"value": "LP1-3", "source": "plan"},
                "mount_height_in": {"value": "54.125", "source": "note"}
            }},
            {"id": circuit, "kind": "circuit", "level": "L1", "at_ft": [0.0, 10.0], "fields": {
                "circuit": {"value": "LP1-3", "source": "plan"},
                "panel": {"value": "LP1", "source": "plan"},
                "conductors": {"value": "2#12 + 1#12G", "source": "default"},
                "conduit": {"value": "3/4\" EMT", "source": "default"},
                "route": {"value": "estimated route", "source": "rule"}
            }}
        ],
        "mep": {
            "systems": [
                {"name": "E-COND", "layer": "E-COND", "kind": "conduit", "size_in": [1, 0], "bottom_ft": 9},
                {"name": "E-CIRCUIT-EST", "layer": "E-CIRCUIT-EST", "kind": "conduit", "size_in": [0.75, 0], "bottom_ft": 9}
            ],
            "runs": [
                {"system": "E-COND", "level": "L1", "points": [[0, 600], [444, 600]], "size_in": [1, 0], "bottoms_ft": [10], "size_source": "tag", "height_source": "tag"},
                {"system": "E-CIRCUIT-EST", "level": "L1", "points": [[0, 120], [248, 120]], "size_in": [0.75, 0], "bottoms_ft": [9],
                 "id": format!("{circuit}#home"), "height_source": "estimated route"},
                {"system": "E-CIRCUIT-EST", "level": "L1", "points": [[248, 120]], "size_in": [0.75, 0], "bottoms_ft": [56.0 / 12.0], "top_ft": 9,
                 "id": format!("{circuit}#d0"), "height_source": "estimated route"}
            ],
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, 110], "max": [-4, 130], "height_ft": 4, "bottom_ft": 2},
                {"tag": "MDP", "kind": "switchboard", "level": "L1", "min": [-200, 100], "max": [-150, 140], "height_ft": 7}
            ],
            "feeders": [{"tag": "F1", "from": "MDP", "to": "LP1", "sets": 1, "phases": 3, "size": "#1", "material": "cu", "neutrals": 1, "grounds": 1, "ground_size": "#6", "conduits": 1, "conduit_in": 1.5, "entry": "top"}]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    // The estimated route as the readers gave it (routing it is Q6152's, tests/conduitroute.rs).
    let built = s.execute("model.build", &json!({"route": false})).expect("build");
    assert!(built["atoms"].as_u64().unwrap_or(0) > 0, "{built}");
    (s, built)
}

fn count(s: &mut Session, q: Value) -> Value {
    s.execute("bim.count", &q).expect("count")
}

fn handles_of(s: &mut Session, kind: &str, size: &str) -> Vec<String> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .filter(|e| {
            let ps = cadaddy_engine::doc::props::read(&e.common.xdata);
            ps.get("atom").map(|p| p.text()).as_deref() == Some(kind) && ps.get("size").map(|p| p.text()).as_deref() == Some(size)
        })
        .map(|e| e.handle.hex())
        .collect()
}

#[test]
fn a_37_ft_run_of_1_inch_emt_is_four_sticks_three_couplings_two_connectors() {
    let mut s = building(None);
    let sticks = count(&mut s, json!({"kind": "conduit", "size": "1\"", "type": "EMT"}));
    assert_eq!(sticks["count"], json!(4), "{sticks}");
    assert_eq!(sticks["length_ft"], json!(37.0));
    assert_eq!(sticks["from"], json!("index"));
    assert_eq!(count(&mut s, json!({"kind": "coupling", "size": "1"}))["count"], json!(3));
    assert_eq!(count(&mut s, json!({"kind": "connector", "size": "1 in"}))["count"], json!(2));
    // By length: three full sticks and the 7' cut piece.
    let d = s.doc().expect("doc");
    let mut lens: Vec<f64> = d
        .model
        .iter()
        .map(|e| cadaddy_engine::doc::props::read(&e.common.xdata))
        .filter(|p| p.get("atom").map(|x| x.text()).as_deref() == Some("conduit") && p.get("size").map(|x| x.text()).as_deref() == Some("1\""))
        .filter_map(|p| p.get("length_ft").and_then(|x| x.text().parse::<f64>().ok()))
        .collect();
    lens.sort_by(f64::total_cmp);
    assert_eq!(lens, vec![7.0, 10.0, 10.0, 10.0]);
    // No longer one mesh for the run: the old segment element is gone.
    let segments: Vec<String> = d
        .model
        .iter()
        .filter(|e| cadaddy_engine::cmd::bimlook::made_tag(e, "part") == Some("segment"))
        .map(|e| format!("{} {:?}", e.common.layer, e.common.xdata))
        .collect();
    assert!(segments.is_empty(), "{segments:?}");
}

#[test]
fn a_25_ft_nm_12_2_from_a_panel_to_a_receptacle_box() {
    let mut s = building(Some("NM cable"));
    let cable = count(&mut s, json!({"kind": "cable", "circuit": "LP1-3"}));
    assert_eq!(cable["count"], json!(1), "{cable}");
    // 25' of run plus 1.5' makeup at the box (300.14's 6" and the 12" allowance) and 3.5' at
    // the panel (half its 4' can down to the breaker, and a box's makeup).
    assert_eq!(cable["length_ft"], json!(30.0), "{cable}");
    assert_eq!(cable["estimated"], json!(1));
    assert_eq!(count(&mut s, json!({"kind": "cable", "type": "NM 12/2"}))["count"], json!(1));
    // Its box: a 1 gang nonmetallic box (on cable), of 18 cu in.
    let boxes = count(&mut s, json!({"kind": "box", "circuit": "LP1-3", "group_by": "type"}));
    assert_eq!(boxes["count"], json!(1), "{boxes}");
    // The receptacle: its box and the cable feeding it; the cable: from the panel to the box.
    let dev = {
        let d = s.doc().expect("doc");
        d.model.iter().find(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device")).map(|e| e.handle.hex()).expect("device")
    };
    let el = s.execute("bim.element", &json!({"handle": dev})).expect("element");
    assert!(el["box"]["handle"].is_string(), "{el}");
    assert_eq!(el["fed_by"][0]["kind"], json!("cable"), "{el}");
    let chain = s.execute("bim.chain", &json!({"handle": dev})).expect("chain");
    let kinds: Vec<&str> = chain["path"].as_array().expect("path").iter().filter_map(|n| n["kind"].as_str()).collect();
    assert_eq!(kinds.first().copied(), Some("receptacle"), "{chain}");
    assert!(kinds.contains(&"box") && kinds.contains(&"cable"), "{chain}");
    assert_eq!(chain["source"]["tag"], json!("LP1"), "{chain}");
    assert_eq!(chain["upstream"][0]["tag"], json!("MDP"), "the feeder from MDP to LP1: {chain}");
    assert_eq!(chain["length_ft"], json!(30.0));
    // Staples within 12" of each box and every 4-1/2' at most: 1, 24 and five between.
    assert_eq!(count(&mut s, json!({"kind": "strap", "type": "NM"}))["count"], json!(7));
}

#[test]
fn an_estimated_emt_route_has_its_wires_and_lands_in_the_box() {
    let mut s = building(None);
    let est = count(&mut s, json!({"kind": "conduit", "circuit": "LP1-3"}));
    assert_eq!(est["estimated"], est["count"], "every stick of the estimated route says so: {est}");
    // 25' of route, the drop ending on the box's top (1/8" above where the route put it).
    assert!((est["length_ft"].as_f64().unwrap_or(0.0) - 25.0).abs() < 0.05, "{est}");
    // 2#12 + 1#12G on circuit 3 (phase B): black-red? red hot, white neutral, green ground.
    let wires = count(&mut s, json!({"kind": "wire", "circuit": "LP1-3", "group_by": "size"}));
    assert_eq!(wires["count"], json!(3), "{wires}");
    let colors: Vec<String> = {
        let d = s.doc().expect("doc");
        let mut c: Vec<String> = d
            .model
            .iter()
            .map(|e| cadaddy_engine::doc::props::read(&e.common.xdata))
            .filter(|p| p.get("atom").map(|x| x.text()).as_deref() == Some("wire") && p.get("circuit").map(|x| x.text()).as_deref() == Some("LP1-3"))
            .filter_map(|p| p.get("color").map(|x| x.text()))
            .collect();
        c.sort();
        c
    };
    assert_eq!(colors, vec!["green", "red", "white"]);
    // The feeder from MDP to LP1 (1-1/2", 3#1 + 1#1 + 1#6G) where it enters each: a raceway
    // landing on the gear, and one set's five conductors in it.
    let f = count(&mut s, json!({"kind": "wire", "system": "F1", "group_by": "size"}));
    assert_eq!(f["count"], json!(10), "two entries of 5 conductors: {f}");
    let rows = f["rows"].as_array().cloned().unwrap_or_default();
    assert!(rows.iter().any(|r| r["size"] == json!("#1") && r["count"] == json!(8)), "{f}");
    assert!(count(&mut s, json!({"kind": "connector", "system": "F1", "size": "1-1/2\""}))["count"].as_u64().unwrap_or(0) >= 2);
    assert_eq!(count(&mut s, json!({"kind": "bushing", "system": "F1"}))["count"], json!(4), "#1 conductors: a bushing at each end");
    // The home run turns down into the drop: 3/4" EMT is bent by hand there, no elbow.
    assert_eq!(count(&mut s, json!({"kind": "bend", "circuit": "LP1-3"}))["count"], json!(1));
    assert_eq!(count(&mut s, json!({"kind": "elbow", "circuit": "LP1-3"}))["count"], json!(0));
    // A 4" square box and its plaster ring? No wall here: the box, no ring.
    let b = count(&mut s, json!({"kind": "box", "type": "4\" square"}));
    assert_eq!(b["count"], json!(1), "{b}");
    // A stick in bim.element: its size, length, run, circuit and neighbours.
    let h = handles_of(&mut s, "conduit", "3/4\"").into_iter().next().expect("a stick");
    let el = s.execute("bim.element", &json!({"handle": h})).expect("element");
    let piece = &el["piece"];
    assert_eq!(piece["kind"], json!("conduit"), "{el}");
    assert_eq!(piece["size"], json!("3/4\""));
    assert_eq!(piece["circuit"], json!("LP1-3"));
    assert!(piece["run"].as_str().is_some_and(|r| !r.is_empty()));
    assert!(piece["length_ft"].as_f64().is_some_and(|l| l > 0.0 && l <= 10.0));
    assert!(piece["connects"].as_array().is_some_and(|c| !c.is_empty()), "{el}");
    assert_eq!(el["trade"], json!("conduit"));
}

#[test]
fn a_box_takes_the_raceway_that_lands_in_it() {
    // Q6243. LP1-3's estimated route read as 1-1/4" lands in the receptacle's box: the trade's
    // 4" square box's knockouts take 1" at most, so the box is a 4-11/16" square one (to
    // 1-1/4"), the 4" square kept beside it as the design's, with why and the rule.
    let mut s = building(None);
    let spec = s.execute("bspec.get", &json!({})).expect("spec");
    let mut spec = spec.get("spec").cloned().unwrap_or(spec);
    for r in spec["mep"]["runs"].as_array_mut().into_iter().flatten().filter(|r| r["system"] == json!("E-CIRCUIT-EST")) {
        r["size_in"] = json!([1.25, 0]);
    }
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("set");
    let built = s.execute("model.build", &json!({})).expect("build");
    assert!(built.to_string().contains("\"boxesSizedForRaceway\":1"), "{built}");
    assert_eq!(count(&mut s, json!({"kind": "box", "type": "4\" square"}))["count"], json!(0));
    assert_eq!(count(&mut s, json!({"kind": "box", "type": "4-11/16\" square"}))["count"], json!(1));
    let d = s.doc().expect("doc");
    let b = d
        .model
        .iter()
        .map(|e| cadaddy_engine::doc::props::read(&e.common.xdata))
        .find(|p| p.get("atom").map(|x| x.text()).as_deref() == Some("box"))
        .expect("the box");
    let t = |k: &str| b.get(k).map(|x| x.text()).unwrap_or_default();
    assert_eq!((t("design_value").as_str(), t("corrected_value").as_str()), ("Box, 4\" square", "Box, 4-11/16\" square"));
    assert!(t("correction_reason").contains("1 1/4\" raceway") && t("correction_rule").contains("knockout"), "{}", t("correction_reason"));
    assert_eq!(t("cubic_in"), "42");
}

#[test]
fn bim_count_equals_a_full_recount_and_an_edit_keeps_the_index_current() {
    let mut s = building(None);
    for q in [json!({}), json!({"kind": "conduit"}), json!({"level": "L1", "group_by": "kind"}), json!({"kind": "receptacle"})] {
        let mut q2 = q.clone();
        q2["verify"] = json!(true);
        let r = count(&mut s, q2);
        assert_eq!(r["verify"]["matches"], json!(true), "{q}: {r}");
        assert_eq!(r["verify"]["count"], r["count"]);
    }
    assert_eq!(count(&mut s, json!({"kind": "receptacle"}))["count"], json!(1));
    let before = count(&mut s, json!({"kind": "conduit", "size": "1\""}));
    let stick = handles_of(&mut s, "conduit", "1\"").into_iter().next().expect("stick");
    let e = s.execute("bim.atoms.edit", &json!({"handle": stick, "length_ft": 8.0})).expect("edit");
    assert_eq!(e["after"]["length_ft"], json!(8.0), "{e}");
    let after = count(&mut s, json!({"kind": "conduit", "size": "1\"", "verify": true}));
    let was = e["before"]["length_ft"].as_f64().unwrap_or(0.0);
    assert!((before["length_ft"].as_f64().unwrap_or(0.0) - was + 8.0 - after["length_ft"].as_f64().unwrap_or(0.0)).abs() < 1e-6, "{before} {after}");
    assert_eq!(after["verify"]["matches"], json!(true), "{after}");
    // A size change moves the stick to the other row.
    s.execute("bim.atoms.edit", &json!({"handle": stick, "size": "1-1/4\""})).expect("edit size");
    assert_eq!(count(&mut s, json!({"kind": "conduit", "size": "1-1/4\""}))["count"], json!(1));
    assert_eq!(count(&mut s, json!({"kind": "conduit", "size": "1\"", "verify": true}))["verify"]["matches"], json!(true));
    // Bad edits are refused.
    assert!(s.execute("bim.atoms.edit", &json!({"handle": stick, "length_ft": -3})).is_err());
    assert!(s.execute("bim.atoms.edit", &json!({"handle": stick})).is_err());
    assert!(s.execute("bim.atoms.edit", &json!({"handle": "zz"})).is_err());
    assert!(s.execute("bim.count", &json!({"group_by": "colour"})).is_err());
}

#[test]
fn the_index_is_saved_with_the_model() {
    let mut s = building(None);
    let total = count(&mut s, json!({}))["count"].clone();
    let d = s.doc().expect("doc").clone();
    assert!(d.embedded.contains_key("atoms/index.json"));
    // Without the stored index (a DWG round trip drops embedded files), bim.count recounts.
    let mut d2 = d.clone();
    d2.embedded.remove("atoms/index.json");
    let mut s2 = Session::empty();
    s2.open_drawing(d2, "Sample Floor Plan", None);
    let r = count(&mut s2, json!({}));
    assert_eq!((r["from"].clone(), r["count"].clone()), (json!("recount"), total));
    let r = count(&mut s2, json!({"refresh": true}));
    assert_eq!(r["from"], json!("index"));
}

#[test]
fn hostile_count_and_chain_params_never_panic() {
    let mut s = building(None);
    for p in [json!({"kind": 5}), json!({"size": null}), json!({"group_by": 3}), json!({"verify": "x"}), json!({"kind": "x".repeat(10_000)})] {
        let _ = s.execute("bim.count", &p);
    }
    for p in [json!({}), json!({"handle": "0"}), json!({"handle": "ffffffffffff"}), json!({"handle": "1", "limit": -5})] {
        let _ = s.execute("bim.chain", &p);
        let _ = s.execute("bim.atoms.edit", &p);
    }
    let r: Value = s.execute("bim.count", &json!({"kind": "nothing-like-this"})).expect("count");
    assert_eq!(r["count"], json!(0));
}

fn wall_line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
}

#[test]
fn a_wall_box_takes_its_ring_and_the_ring_follows_the_wall() {
    // A 20' x 10' room of 6" walls; a receptacle on its south wall on circuit LP1-3.
    let mut d = Drawing::new_imperial();
    let o = [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let i = [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    for (a, b) in o.into_iter().chain(i) {
        wall_line(&mut d, a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": [{"id": "device:L1:5.0,0.8", "kind": "receptacle", "level": "L1", "at_ft": [5.0, 0.8], "fields": {
            "type": {"value": "A", "source": "plan"}, "symbol": {"value": "1F0", "source": "plan"},
            "sheet": {"value": "E-101", "source": "plan"}, "circuit": {"value": "LP1-3", "source": "plan"}
        }}]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    let ring = count(&mut s, json!({"kind": "ring", "group_by": "size"}));
    assert_eq!(ring["count"], json!(1), "a 4\" square box on a wall takes a 1 gang ring: {ring}");
    assert_eq!(ring["rows"][0]["size"], json!("1 gang, 1/2\" deep"), "{ring}");
    // The device: in its box, its face against the wall's.
    let dev = {
        let d = s.doc().expect("doc");
        d.model.iter().find(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device")).map(|e| e.handle.hex()).expect("device")
    };
    let el = s.execute("bim.element", &json!({"handle": dev})).expect("element");
    assert!(el["box"]["handle"].is_string(), "{el}");
    // Set on the wall's face: its back flush with it.
    assert_eq!(el["wall_face"]["as"], json!("flush"), "{el}");
    assert!(el["wall_face"]["inches"].as_f64().is_some_and(|v| v.abs() < 0.125), "{el}");
    // A person says the wall's board needs a 5/8" ring: the fill carries it onto the ring.
    s.execute("bim.props.set", &json!({"handle": dev, "key": "mud_ring", "value": 0.625, "unit": "in"})).expect("set");
    s.execute("bim.props.fill", &json!({})).expect("fill");
    let ring = count(&mut s, json!({"kind": "ring", "group_by": "size", "verify": true}));
    assert_eq!(ring["rows"][0]["size"], json!("1 gang, 5/8\" deep"), "{ring}");
    assert_eq!(ring["verify"]["matches"], json!(true), "{ring}");
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    let rings: Vec<&Value> = bom["lines"].as_array().into_iter().flatten().filter(|l| l["item"] == json!("Plaster ring")).collect();
    assert_eq!(rings.len(), 1, "{rings:?}");
    assert_eq!(rings[0]["size"], json!("1 gang, 5/8\" deep"));
}

/// `bim.count` from the stored index is a recount's, row by row (the takeoff, BOM and estimate
/// against a recount: tests/takeoff_edits.rs).
fn agrees_with_a_recount(s: &mut Session, step: &str) -> Value {
    let c = count(s, json!({"verify": true}));
    assert_eq!(c["verify"]["matches"], json!(true), "{step}: bim.count is the stored index's, not a recount: {c}");
    let rows = count(s, json!({"group_by": "kind"}))["rows"].clone();
    count(s, json!({"refresh": true}));
    assert_eq!(rows, count(s, json!({"group_by": "kind"}))["rows"], "{step}");
    c
}

#[test]
fn plain_edits_of_model_pieces_keep_the_count_index_and_the_saved_model_current() {
    // Soak S5-002: ERASE, COPY, EXPLODE and MOVE of model pieces with the plain drawing commands
    // left the stored index (and so bim.count, the BOM's pieces and the takeoff) as built, and the
    // .cdy saved it so.
    let mut s = building(None);
    let n = |c: &Value| c["count"].as_u64().unwrap_or(0);
    let built = agrees_with_a_recount(&mut s, "built");
    let conduit_ft = |s: &mut Session| count(s, json!({"kind": "conduit", "size": "1\""}))["length_ft"].as_f64().unwrap_or(0.0);
    let sticks = handles_of(&mut s, "conduit", "1\"");
    assert_eq!(sticks.len(), 4);
    let ft0 = conduit_ft(&mut s);
    s.execute("erase", &json!({"handles": [sticks[0], sticks[1]]})).expect("erase");
    let erased = agrees_with_a_recount(&mut s, "erase");
    assert_eq!(n(&erased), n(&built) - 2);
    assert!(conduit_ft(&mut s) < ft0 - 1.0, "the erased sticks' length is gone");
    s.execute("copy", &json!({"handles": [sticks[2]], "delta": [0, 48]})).expect("copy");
    assert_eq!(n(&agrees_with_a_recount(&mut s, "copy")), n(&built) - 1);
    s.execute("explode", &json!({"handles": [sticks[3]]})).expect("explode");
    let exploded = agrees_with_a_recount(&mut s, "explode");
    s.execute("move", &json!({"handles": [sticks[2]], "delta": [12, 0]})).expect("move");
    assert_eq!(n(&agrees_with_a_recount(&mut s, "move")), n(&exploded));
    for (cmd, k) in [("undo", 1), ("undo", 3), ("redo", 2), ("undo", 10), ("redo", 10)] {
        s.execute(cmd, &json!({"count": k})).expect(cmd);
        agrees_with_a_recount(&mut s, &format!("{cmd} {k}"));
    }
    // Saved as .cdy and opened again: the index it keeps is the current one.
    let edited = n(&count(&mut s, json!({})));
    let bytes = cadaddy_io::write(s.doc().expect("doc"), "model.cdy").expect("save");
    let mut s2 = Session::empty();
    s2.open_drawing(cadaddy_io::read(&bytes, "model.cdy").expect("open"), "Sample Floor Plan", None);
    let reopened = count(&mut s2, json!({"verify": true}));
    assert_eq!((reopened["from"].clone(), n(&reopened)), (json!("index"), edited), "{reopened}");
    assert_eq!(reopened["verify"]["matches"], json!(true), "{reopened}");
}

#[test]
fn an_edit_off_the_model_leaves_the_stored_index_as_it_is() {
    // A line drawn on the sheet changes no piece: the index (its bytes) is not rewritten.
    let mut s = building(None);
    let before = s.doc().expect("doc").embedded.get("atoms/index.json").cloned().expect("index");
    s.execute("line", &json!({"points": [[-500, -500], [-400, -500]]})).expect("line");
    let after = s.doc().expect("doc").embedded.get("atoms/index.json").cloned().expect("index");
    assert!(std::sync::Arc::ptr_eq(&before, &after));
}

/// A 20' x 10' room of 6" walls with the devices given as (kind, x ft, y ft, mounting height in),
/// on circuit LP1-3.
fn room_with(devices: &[(&str, f64, f64, f64)]) -> Session {
    room_with_circuit(devices, "LP1-3")
}

/// [`room_with`], the devices on `circuit` (none: empty).
fn room_with_circuit(devices: &[(&str, f64, f64, f64)], circuit: &str) -> Session {
    let mut d = Drawing::new_imperial();
    let o = [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let i = [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    for (a, b) in o.into_iter().chain(i) {
        wall_line(&mut d, a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let tags: Vec<Value> = devices
        .iter()
        .map(|(kind, x, y, h)| {
            json!({"id": format!("device:L1:{x},{y}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": {
                "symbol": {"value": "1F0", "source": "plan"}, "sheet": {"value": "E-101", "source": "plan"},
                "circuit": {"value": circuit, "source": "plan"}, "mount_height_in": {"value": h.to_string(), "source": "note"}
            }})
        })
        .collect();
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "tags": tags
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

#[test]
fn devices_side_by_side_in_one_wall_share_one_box_ring_and_plate() {
    // Test finding (fix-box-count-high): every wall device got a box of its own, so a row of
    // switches at a door was bought as a box, ring and plate each. Three switches 18" apart, two
    // receptacles 12" apart, a data outlet beside them (low voltage never shares a power box)
    // and a receptacle alone on the far wall.
    let mut s = room_with(&[
        ("switch", 5.0, 0.8, 48.0),
        ("switch", 6.5, 0.8, 48.0),
        ("switch", 8.0, 0.8, 48.0),
        ("receptacle", 12.0, 0.8, 18.0),
        ("receptacle", 13.0, 0.8, 18.0),
        ("data_outlet", 14.5, 0.8, 18.0),
        ("receptacle", 10.0, 9.2, 18.0),
    ]);
    let boxes = count(&mut s, json!({"kind": "box", "group_by": "item", "verify": true}));
    assert_eq!(boxes["verify"]["matches"], json!(true), "{boxes}");
    let by = |rows: &Value, k: &str, v: &str| {
        rows["rows"].as_array().into_iter().flatten().filter(|r| r[k] == json!(v)).map(|r| r["count"].as_u64().unwrap_or(0)).sum::<u64>()
    };
    // The switches: one 3 gang box; the two receptacles: one 4" square; the data outlet and the
    // lone receptacle: a 4" square each.
    assert_eq!(boxes["count"], json!(4), "one box per gang location, not per device: {boxes}");
    assert_eq!(by(&boxes, "item", "Device box, 3 gang"), 1, "{boxes}");
    assert_eq!(by(&boxes, "item", "Box, 4\" square"), 3, "{boxes}");
    let rings = count(&mut s, json!({"kind": "ring", "group_by": "size"}));
    assert_eq!(rings["count"], json!(4), "{rings}");
    assert_eq!(by(&rings, "size", "3 gang, 1/2\" deep"), 1, "{rings}");
    assert_eq!(by(&rings, "size", "2 gang, 1/2\" deep"), 1, "{rings}");
    assert_eq!(by(&rings, "size", "1 gang, 1/2\" deep"), 2, "{rings}");
    // Every device is in a box (the shared one says which devices it holds).
    let d = s.doc().expect("doc");
    let devices: Vec<String> =
        d.model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device")).map(|e| e.handle.hex()).collect();
    assert_eq!(devices.len(), 7);
    for h in &devices {
        let el = s.execute("bim.element", &json!({"handle": h})).expect("element");
        assert!(el["box"]["handle"].is_string(), "{el}");
    }
    // The BOM: one plate per box, of its gangs.
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    let plates: Vec<(String, f64)> = bom["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| l["item"] == json!("Device plate"))
        .map(|l| (l["size"].as_str().unwrap_or_default().to_string(), l["net"].as_f64().unwrap_or(0.0)))
        .collect();
    let plate = |size: &str| plates.iter().filter(|(s, _)| s == size).map(|(_, n)| n).sum::<f64>();
    assert_eq!((plate("3 gang"), plate("2 gang")), (1.0, 1.0), "{plates:?}");
    // The data outlet's plate is the low voltage trade's (its scope), the lone receptacle's ours.
    assert!(plate("1 gang") <= 2.0, "{plates:?}");
}

#[test]
fn equipment_is_fed_in_its_own_compartment_and_its_run_ends_in_it() {
    // Test finding (fix-box-count-high): an equipment connection (a fan, a room air conditioner)
    // got a 4" square box of its own, though its raceway lands in the unit's wiring compartment.
    let (mut s, built) = building_with(None, "equipment_connection");
    assert_eq!(count(&mut s, json!({"kind": "box", "circuit": "LP1-3"}))["count"], json!(0), "no outlet box at equipment");
    let atoms = &built["atoms"];
    let report = if atoms.is_object() { atoms } else { &built };
    let text = report.to_string();
    assert!(text.contains("\"endsAtEquipment\":1"), "the route's end lands in the equipment: {text}");
    // Not an open end: only the drawn run's two ends land in nothing, as with a receptacle there.
    let (_, base) = building_with(None, "receptacle");
    assert_eq!(built["openEnds"], base["openEnds"], "{text}");
    assert_eq!(built["openEnds"], json!(2), "{text}");
    assert!(text.contains("own wiring compartment"), "it says why there is no box: {text}");
    // The chain from the equipment walks back to its panel.
    let dev = {
        let d = s.doc().expect("doc");
        d.model.iter().find(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device")).map(|e| e.handle.hex()).expect("device")
    };
    let chain = s.execute("bim.chain", &json!({"handle": dev})).expect("chain");
    assert_eq!(chain["source"]["tag"], json!("LP1"), "{chain}");
    // Its connector stays: the raceway ends in the unit's knockout.
    assert!(count(&mut s, json!({"kind": "connector", "circuit": "LP1-3"}))["count"].as_u64().unwrap_or(0) >= 2);
}

#[test]
fn a_luminaire_box_follows_the_wiring_method() {
    // On raceway: the trade's 4" square with a 4" round ring, no octagon (fix-box-count-high).
    let (mut s, _) = building_with(None, "luminaire");
    let b = count(&mut s, json!({"kind": "box", "circuit": "LP1-3", "group_by": "item"}));
    assert_eq!(b["rows"][0]["item"], json!("Box, 4\" square"), "{b}");
    assert_eq!(b["count"], json!(1), "{b}");
    let r = count(&mut s, json!({"kind": "ring", "circuit": "LP1-3", "group_by": "size"}));
    assert_eq!(r["rows"][0]["size"], json!("4\" round, 1/2\" deep"), "{r}");
    // On cable (NM): the octagon, no ring.
    let (mut s, _) = building_with(Some("NM cable"), "luminaire");
    let b = count(&mut s, json!({"kind": "box", "circuit": "LP1-3", "group_by": "item"}));
    assert_eq!(b["rows"][0]["item"], json!("Box, 4\" octagon"), "{b}");
    assert_eq!(count(&mut s, json!({"kind": "ring", "circuit": "LP1-3"}))["count"], json!(0));
}

#[test]
fn a_device_of_no_known_kind_on_no_circuit_takes_no_box() {
    let mut s = room_with(&[("device", 5.0, 0.8, 48.0)]);
    assert_eq!(count(&mut s, json!({"kind": "box"}))["count"], json!(1), "on a circuit: its box");
    let mut s = room_with_circuit(&[("device", 5.0, 0.8, 48.0)], "");
    assert_eq!(count(&mut s, json!({"kind": "box"}))["count"], json!(0), "on no circuit: not known to be wired");
}

/// Two 19'-6" x 9' rooms side by side (made-up units A and B) with one 6" wall between them,
/// every wall of type `Partition` (fire-rated when `rated`), the devices given as (kind, x ft,
/// y ft, mounting height in) on circuit LP1-3. The build's report beside it.
fn two_rooms_with(devices: &[(&str, f64, f64, f64)], rated: bool) -> (Session, Value) {
    let mut d = Drawing::new_imperial();
    let o = [((0., 0.), (40., 0.)), ((40., 0.), (40., 10.)), ((40., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let i = [((0.5, 0.5), (39.5, 0.5)), ((39.5, 0.5), (39.5, 9.5)), ((39.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    let mid = [((19.75, 0.5), (19.75, 9.5)), ((20.25, 0.5), (20.25, 9.5))];
    for (a, b) in o.into_iter().chain(i).chain(mid) {
        wall_line(&mut d, a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let tags: Vec<Value> = devices
        .iter()
        .map(|(kind, x, y, h)| {
            json!({"id": format!("device:L1:{x},{y}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": {
                "symbol": {"value": "1F0", "source": "plan"}, "sheet": {"value": "E-101", "source": "plan"},
                "circuit": {"value": "LP1-3", "source": "plan"}, "mount_height_in": {"value": h.to_string(), "source": "note"}
            }})
        })
        .collect();
    let assembly = if rated { json!({"tag": "P1", "fire_rating": "1 HR"}) } else { json!({"tag": "P1"}) };
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6, "assembly": assembly}],
        "tags": tags
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({})).expect("build");
    (s, built)
}

/// Every box atom: its handle, its device (`box_of`), its body's extent (min, max) and its props.
fn box_bodies(s: &mut Session) -> Vec<(String, String, [f64; 3], [f64; 3], cadaddy_engine::doc::props::Props)> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .filter_map(|e| {
            let ps = cadaddy_engine::doc::props::read(&e.common.xdata);
            if ps.get("atom").map(|p| p.text()).as_deref() != Some("box") {
                return None;
            }
            let EntityKind::Mesh(m) = &e.kind else { return None };
            let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
            for v in &m.vertices {
                for (k, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                    lo[k] = lo[k].min(c);
                    hi[k] = hi[k].max(c);
                }
            }
            let of = ps.get("box_of").map(|p| p.text()).unwrap_or_default();
            Some((e.handle.hex(), of, lo, hi, ps))
        })
        .collect()
}

#[test]
fn boxes_never_overlap_and_never_stand_back_to_back_in_a_rated_wall() {
    // Test finding (fix-box-collisions): a receptacle and a data outlet drawn at one spot (they
    // never gang: power and low voltage) got two coincident boxes, and the two units'
    // receptacles 2.4" apart on the two faces of the rated wall between them stood back to back
    // in one stud cavity.
    let devices = [("receptacle", 5.0, 0.8, 18.0), ("data_outlet", 5.0, 0.8, 18.0), ("receptacle", 19.6, 5.0, 18.0), ("receptacle", 20.4, 5.2, 18.0)];
    let (mut s, built) = two_rooms_with(&devices, true);
    let boxes = box_bodies(&mut s);
    assert_eq!(boxes.len(), 4, "{built}");
    // No two boxes share space.
    for (i, a) in boxes.iter().enumerate() {
        for b in boxes.iter().skip(i + 1) {
            let apart = (0..3).any(|k| a.3[k] <= b.2[k] + 1e-6 || b.3[k] <= a.2[k] + 1e-6);
            assert!(apart, "boxes {} and {} overlap: {:?}-{:?} and {:?}-{:?}", a.0, b.0, a.2, a.3, b.2, b.3);
        }
    }
    // The two in the rated wall: at least 24" apart along it, edge to edge (IBC 714.4.2).
    let in_mid: Vec<&(String, String, [f64; 3], [f64; 3], _)> = boxes.iter().filter(|b| b.2[0] > 19.0 * 12.0 && b.3[0] < 21.0 * 12.0).collect();
    assert_eq!(in_mid.len(), 2, "{boxes:?}");
    let (p, q) = (in_mid[0], in_mid[1]);
    let edge = (q.2[1] - p.3[1]).max(p.2[1] - q.3[1]);
    assert!(edge >= 24.0 - 1e-6, "back to back {edge} in apart: {:?} {:?}", p.2, q.2);
    // What moved says so: the design spot beside the one installed, the reason and the rule.
    let moved: Vec<_> = boxes.iter().filter(|b| b.4.get("design:location").is_some()).collect();
    assert_eq!(moved.len(), 2, "one box off the coincident pair, one off the back-to-back pair: {boxes:?}");
    let rules: Vec<String> = moved.iter().map(|b| b.4.get("correction:location").map(|p| p.text()).unwrap_or_default()).collect();
    assert!(rules.iter().any(|r| r.contains("IBC 714.4.2") && r.contains("back to back")), "{rules:?}");
    assert!(rules.iter().any(|r| r.contains("overlapped")), "{rules:?}");
    let clear = &built;
    // The device moved with its box, its drawn location kept.
    for b in &moved {
        let dev = Handle::parse_hex(&b.1).expect("device");
        let d = s.doc().expect("doc");
        let ps = cadaddy_engine::doc::props::read(&d.model.get(dev).expect("device").common.xdata);
        let (design, now) = (ps.get("design:location").map(|p| p.text()), ps.get("location").map(|p| p.text()));
        assert!(design.is_some() && design != now, "{design:?} {now:?} {clear}");
        // Its box holds it: the device's plan centre is inside the box's plan extent along the wall.
        let e = d.model.get(dev).expect("device");
        let EntityKind::Mesh(m) = &e.kind else { panic!("device mesh") };
        let n = m.vertices.len().max(1) as f64;
        let c = m.vertices.iter().fold([0.0; 2], |s, v| [s[0] + v.x / n, s[1] + v.y / n]);
        let inside = |k: usize| c[k] >= b.2[k] - 3.0 && c[k] <= b.3[k] + 3.0;
        assert!(inside(0) && inside(1), "device at {c:?}, box {:?}-{:?}", b.2, b.3);
    }
    // Not a rated wall: the two receptacles stay where they are drawn (only the overlap moves).
    let (mut s, _) = two_rooms_with(&devices, false);
    let boxes = box_bodies(&mut s);
    assert_eq!(boxes.iter().filter(|b| b.4.get("design:location").is_some()).count(), 1, "{boxes:?}");
}

#[test]
fn a_sconces_box_is_in_its_wall_and_two_units_sconces_never_stand_back_to_back() {
    // Test finding (fix-box-collisions): a wall luminaire's outlet box was set over the fixture
    // like a ceiling box (out of the wall), so two units' sconces at one spot on the two faces of
    // their rated wall were never seen back to back.
    let (mut s, built) = two_rooms_with(&[("wall_luminaire", 19.6, 5.0, 80.0), ("wall_luminaire", 20.4, 5.0, 80.0)], true);
    let boxes = box_bodies(&mut s);
    assert_eq!(boxes.len(), 2, "{built}");
    for b in &boxes {
        // In the wall between the rooms (its faces at 19'-9" and 20'-3"), at the fixture's height.
        assert!(b.2[0] >= 19.75 * 12.0 - 1e-6 && b.3[0] <= 20.25 * 12.0 + 1e-6, "box out of its wall: {:?}-{:?}", b.2, b.3);
        assert!(b.2[2] < 80.0 && b.3[2] > 70.0, "box not behind the fixture: {:?}-{:?}", b.2, b.3);
    }
    let (p, q) = (&boxes[0], &boxes[1]);
    let edge = (q.2[1] - p.3[1]).max(p.2[1] - q.3[1]);
    assert!(edge >= 24.0 - 1e-6, "back to back {edge} in apart");
}

#[test]
fn a_device_moved_off_a_back_to_back_box_never_gangs_with_its_new_neighbour() {
    // Test finding (fix-box-collisions, on the rebuild): a receptacle moved 2 ft along the wall
    // off its back-to-back partner landed within the gang reach of the next one on its face, and
    // the next atoms build (the router's final pass) ganged the two into one box between them,
    // off the moved device. Moved devices gang as the documents draw them.
    let devices = [("receptacle", 19.6, 5.0, 18.0), ("receptacle", 20.4, 5.2, 18.0), ("receptacle", 20.4, 8.2, 18.0)];
    let (mut s, _) = two_rooms_with(&devices, true);
    let first = box_bodies(&mut s);
    assert_eq!(first.len(), 3, "{first:?}");
    assert_eq!(first.iter().filter(|b| b.4.get("design:location").is_some()).count(), 1, "one moved: {first:?}");
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    let again = box_bodies(&mut s);
    assert_eq!(again.len(), 3, "each receptacle keeps its own box: {again:?}");
    assert!(again.iter().all(|b| b.4.get("gang").is_none()), "{again:?}");
    // Each device is in its box.
    let d = s.doc().expect("doc");
    for b in &again {
        let dev = Handle::parse_hex(&b.1).expect("device");
        let EntityKind::Mesh(m) = &d.model.get(dev).expect("device").kind else { panic!("device mesh") };
        let n = m.vertices.len().max(1) as f64;
        let y = m.vertices.iter().map(|v| v.y / n).sum::<f64>();
        assert!(y >= b.2[1] - 3.0 && y <= b.3[1] + 3.0, "device at y {y}, box {:?}-{:?}", b.2, b.3);
    }
}

fn plan_line(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = layer.into();
    d.model.push(e);
}

/// [`two_rooms_with`] (the wall between them fire-rated) with a door in that wall from `door.0`
/// to `door.1` ft (jambs, the open leaf and its swing), every device as (kind, x ft, y ft,
/// mounting height in or none: not printed). The build's report beside it.
fn two_rooms_with_door(devices: &[(&str, f64, f64, Option<f64>)], door: (f64, f64)) -> (Session, Value) {
    let mut d = Drawing::new_imperial();
    let o = [((0., 0.), (40., 0.)), ((40., 0.), (40., 10.)), ((40., 10.), (0., 10.)), ((0., 10.), (0., 0.))];
    let i = [((0.5, 0.5), (39.5, 0.5)), ((39.5, 0.5), (39.5, 9.5)), ((39.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))];
    let (y0, y1) = door;
    let mid = [
        ((19.75, 0.5), (19.75, y0)),
        ((19.75, y1), (19.75, 9.5)),
        ((20.25, 0.5), (20.25, y0)),
        ((20.25, y1), (20.25, 9.5)),
        ((19.75, y0), (20.25, y0)),
        ((19.75, y1), (20.25, y1)),
    ];
    for (a, b) in o.into_iter().chain(i).chain(mid) {
        plan_line(&mut d, "A-WALL", a, b);
    }
    plan_line(&mut d, "A-DOOR", (20.25, y1), (20.25 + (y1 - y0), y1));
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut arc = Entity::new(
        h,
        EntityKind::Arc(cadaddy_engine::doc::Arc {
            center: Vec3 { x: 20.25 * 12.0, y: y1 * 12.0, z: 0.0 },
            radius: (y1 - y0) * 12.0,
            start: 1.5 * std::f64::consts::PI,
            end: 0.0,
        }),
    );
    arc.common.layer = "A-DOOR".into();
    d.model.push(arc);
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    let tags: Vec<Value> = devices
        .iter()
        .map(|(kind, x, y, h)| {
            let mut fields = json!({
                "symbol": {"value": "1F0", "source": "plan"}, "sheet": {"value": "E-101", "source": "plan"},
                "circuit": {"value": "LP1-3", "source": "plan"}
            });
            if let (Some(h), Some(f)) = (h, fields.as_object_mut()) {
                f.insert("mount_height_in".into(), json!({"value": h.to_string(), "source": "note"}));
            }
            json!({"id": format!("device:L1:{x},{y}"), "kind": kind, "level": "L1", "at_ft": [x, y], "fields": fields})
        })
        .collect();
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}, {"layer": "A-DOOR", "role": "door"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6, "assembly": {"tag": "P1", "fire_rating": "1 HR"}}],
        "openings": {"enabled": true, "min_ft": 1.5, "max_ft": 16.0, "door_head_ft": 7.0, "window_head_ft": 7.0, "window_sill_ft": 2.0},
        "tags": tags
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({})).expect("build");
    (s, built)
}

/// A box's centre (inches) from its body.
fn mid(b: &(String, String, [f64; 3], [f64; 3], cadaddy_engine::doc::props::Props)) -> [f64; 3] {
    [(b.2[0] + b.3[0]) / 2.0, (b.2[1] + b.3[1]) / 2.0, (b.2[2] + b.3[2]) / 2.0]
}

#[test]
fn a_fire_separation_move_never_jumps_a_door_opening_it_pads_both_boxes() {
    // Test finding (fix-fire-separation-box-move-wrong): a box moved off its back-to-back partner
    // went 47.5" along its wall, through the door opening to the hinge side behind the leaf; only
    // the spot it landed on was checked. Two units' receptacles 1.2" apart on the faces of the
    // rated wall, 4.8" south of a 3' door (y 3'-6'); south the wall ends.
    let devices = [("receptacle", 19.6, 2.6, Some(18.0)), ("receptacle", 20.4, 2.7, Some(18.0))];
    let (mut s, built) = two_rooms_with_door(&devices, (3.0, 6.0));
    let d = s.doc().expect("doc");
    let doors = d.model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("door")).count();
    assert_eq!(doors, 1, "the fixture's door: {built}");
    let boxes = box_bodies(&mut s);
    assert_eq!(boxes.len(), 2, "{boxes:?}");
    for b in &boxes {
        let c = mid(b);
        // Neither box in or past the door opening (y 36"-72").
        assert!(c[1] < 36.0, "box {} moved through the door to {c:?}", b.0);
    }
    // Neither moved; both carry the listed putty pad, with why.
    assert!(boxes.iter().all(|b| b.4.get("design:location").is_none()), "{boxes:?}");
    for b in &boxes {
        let why = b.4.get("correction:firestop").map(|p| p.text()).unwrap_or_default();
        assert!(why.contains("IBC 714.4.2") && why.contains("unbroken"), "{why}");
    }
    assert!(built.to_string().contains("\"puttyPads\":2"), "{built}");
}

#[test]
fn a_switch_keeps_its_spot_and_the_box_on_the_other_face_moves_the_full_distance() {
    // Test finding: a bed-wall switch was moved 23.5" off the architectural spot behind the
    // headboard. The switch's spot wins: the receptacle behind it on the other face moves, its
    // full 24" clear (IBC 714.4.2 exception 1.1). The switch is the later device (it used to
    // move first).
    let devices = [("receptacle", 20.4, 5.0, Some(18.0)), ("switch", 19.6, 5.1, Some(46.0))];
    let (mut s, built) = two_rooms_with_door(&devices, (7.0, 9.4));
    let boxes = box_bodies(&mut s);
    assert_eq!(boxes.len(), 2, "{built}");
    let kind_of = |b: &(String, String, [f64; 3], [f64; 3], cadaddy_engine::doc::props::Props)| {
        let d = s.doc().expect("doc");
        let dev = Handle::parse_hex(&b.1).expect("device");
        cadaddy_engine::doc::props::read(&d.model.get(dev).expect("device").common.xdata).get("kind").map(|p| p.text()).unwrap_or_default()
    };
    let sw = boxes.iter().find(|b| kind_of(b) == "switch").expect("switch box");
    let rc = boxes.iter().find(|b| kind_of(b) == "receptacle").expect("receptacle box");
    assert!(sw.4.get("design:location").is_none(), "the switch stays: {:?}", sw.4.get("correction:location").map(|p| p.text()));
    assert!(rc.4.get("design:location").is_some(), "the receptacle moves: {built}");
    let edge = (sw.2[1] - rc.3[1]).max(rc.2[1] - sw.3[1]);
    assert!(edge >= 24.0 - 1e-6, "{edge} in apart");
    let report = built.to_string();
    assert!(report.contains("\"keptSpots\":1"), "{report}");
}

#[test]
fn ganged_devices_stand_at_one_height_the_one_the_documents_give() {
    // Test finding: a dimmer and a 3-way ganged by their keynote were built in one box at two
    // heights (46" and 43.75"), their plates overlapping. One box, one plate, one height: the one
    // the documents give first (here both notes: the lower), the other's kept beside.
    let devices = [("switch", 5.0, 0.8, Some(46.0)), ("switch", 5.5, 0.8, Some(43.75))];
    let (mut s, built) = two_rooms_with_door(&devices, (7.0, 9.4));
    let boxes = box_bodies(&mut s);
    assert_eq!(boxes.len(), 1, "one ganged box: {boxes:?} {built}");
    let d = s.doc().expect("doc");
    let mut zs = Vec::new();
    let mut corrected = 0;
    for e in d.model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device")) {
        let EntityKind::Mesh(m) = &e.kind else { panic!("device mesh") };
        let (lo, hi) = m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| (a.min(v.z), b.max(v.z)));
        zs.push((lo + hi) / 2.0);
        let ps = cadaddy_engine::doc::props::read(&e.common.xdata);
        if let Some(h) = ps.get("design:mount_height") {
            corrected += 1;
            assert_eq!(ps.get("mount_height").map(|p| p.text()).as_deref(), Some("43.75"), "design {}", h.text());
            let why = ps.get("correction:mount_height").map(|p| p.text()).unwrap_or_default();
            assert!(why.contains("ganged"), "{why}");
        }
    }
    assert_eq!(zs.len(), 2);
    assert!((zs[0] - zs[1]).abs() < 0.01, "one height: {zs:?}");
    assert!((zs[0] - 43.75).abs() < 0.6, "at 43.75\": {zs:?}");
    assert_eq!(corrected, 1, "the one not at it says so");
    let c = mid(&boxes[0]);
    assert!((c[2] - 43.75).abs() < 0.6, "the box at it too: {c:?}");
}
