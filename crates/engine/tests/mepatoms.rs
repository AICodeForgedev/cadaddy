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
            {"id": "device:L1:20.7,10.0", "kind": "receptacle", "level": "L1", "at_ft": [248.0 / 12.0, 10.0], "fields": {
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
    s
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
    // 25' of run plus 1' makeup at the box and 3' at the panel (to its breaker).
    assert_eq!(cable["length_ft"], json!(29.0), "{cable}");
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
    assert_eq!(chain["length_ft"], json!(29.0));
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
