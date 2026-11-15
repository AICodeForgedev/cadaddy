//! What each object needs before it can be installed (Q6619): a made-up two-level model whose
//! electrical objects carry their properties; one set plants the objects the documents leave
//! incomplete (a luminaire with no circuit, type or control, a receptacle with no circuit, a switch
//! controlling nothing, a heat pump with no circuit, a panel no feeder reaches, a fan on a circuit
//! only a rule assumed), the other leaves them complete. `rfi.read` measures each requirement over
//! the model, raises one RFI per requirement and space, and (apply) completes each object with
//! the build's proposal, marked unsure.

use cadaddy_engine::Session;
use cadaddy_engine::doc::props::{self, Prop, Props, Source, SourceKind};
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Mesh, MeshFace, RawTag};
use cadaddy_engine::geom::Vec3;
use cadaddy_engine::walk3d::BuildingSpec;
use serde_json::{Value, json};

fn print() -> Source {
    Source::print("Sample Sheet E-101", None)
}

/// An element as the build makes them: a box (feet; the drawing is in inches), the build's tag
/// and its properties, each from `src` unless given as `(value, source)`.
fn el(d: &mut Drawing, part: &str, level: &str, layer: &str, at: [f64; 3], ps: &[(&str, Value, Source)]) -> Handle {
    let v = |x: f64, y: f64, z: f64| Vec3 { x: x * 12.0, y: y * 12.0, z: z * 12.0 };
    let (lo, hi) = (at, [at[0] + 1.0, at[1] + 1.0, at[2] + 0.5]);
    let vertices = vec![
        v(lo[0], lo[1], lo[2]),
        v(hi[0], lo[1], lo[2]),
        v(hi[0], hi[1], lo[2]),
        v(lo[0], hi[1], lo[2]),
        v(lo[0], lo[1], hi[2]),
        v(hi[0], lo[1], hi[2]),
        v(hi[0], hi[1], hi[2]),
        v(lo[0], hi[1], hi[2]),
    ];
    let f = |a: [u32; 4]| MeshFace { v: a.to_vec(), hidden: 0 };
    let faces = vec![f([0, 3, 2, 1]), f([4, 5, 6, 7]), f([0, 1, 5, 4]), f([1, 2, 6, 5]), f([2, 3, 7, 6]), f([3, 0, 4, 7])];
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Mesh(Mesh { vertices, faces, ..Default::default() }));
    e.common.layer = layer.into();
    let x = vec![
        RawTag { code: 1001, value: "CADADDY_WALLGEN".into() },
        RawTag { code: 1000, value: part.into() },
        RawTag { code: 1000, value: level.into() },
        RawTag { code: 1000, value: "0".into() },
    ];
    let mut store = Props::default();
    for (k, val, src) in ps {
        store.set(k, Prop::new(val.clone(), src.clone()));
    }
    e.common.xdata = props::write(&x, &store).into();
    d.ensure_layer(layer);
    d.model.push(e);
    h
}

fn p(k: &'static str, v: &str) -> (&'static str, Value, Source) {
    (k, json!(v), print())
}

fn sched(k: &'static str, v: &str) -> (&'static str, Value, Source) {
    (k, json!(v), Source::of(SourceKind::Schedule))
}

fn rule(k: &'static str, v: &str) -> (&'static str, Value, Source) {
    (k, json!(v), Source::of(SourceKind::Rule))
}

/// A luminaire the documents complete: its circuit, its schedule row and its switch.
fn light(d: &mut Drawing, level: &str, room: &str, x: f64, y: f64) -> Handle {
    let z = if level == "L2" { 19.0 } else { 9.0 };
    el(
        d,
        "light",
        level,
        "E-LITE",
        [x, y, z],
        &[
            p("room", room),
            p("circuit", "LP1-1"),
            p("type_mark", "A"),
            sched("schedule_row", "A DOWNLIGHT"),
            p("controlled_by", "S1"),
            p("sheet", "E-101"),
        ],
    )
}

fn device(d: &mut Drawing, level: &str, kind: &str, room: &str, x: f64, y: f64, more: &[(&'static str, Value, Source)]) -> Handle {
    let z = if level == "L2" { 11.5 } else { 1.5 };
    let mut ps = vec![p("kind", kind), p("room", room), p("sheet", "E-101")];
    ps.extend(more.iter().cloned());
    el(d, "device", level, "E-POWER", [x, y, z], &ps)
}

struct Planted {
    dark_light: Handle,
    bare_receptacle: Handle,
    idle_switch: Handle,
    heat_pump: Handle,
    _unit_panel: Handle,
    unfed_panel: Handle,
    assumed_fan: Handle,
}

/// The made-up model; `plant` leaves the planted objects incomplete.
fn model(plant: bool) -> (Session, Option<Planted>) {
    let mut d = Drawing::new_imperial();
    // Level 1: room 101 with three lights, receptacles and a switch; the panels.
    for i in 0..3 {
        light(&mut d, "L1", "101", 4.0 + 6.0 * f64::from(i), 5.0);
    }
    for i in 0..2 {
        device(&mut d, "L1", "receptacle", "101", 2.0 + 8.0 * f64::from(i), 0.5, &[p("circuit", "LP1-3")]);
    }
    device(&mut d, "L1", "switch", "101", 1.0, 9.0, &[p("controls", "lights of 101")]);
    el(
        &mut d,
        "equipment",
        "L1",
        "E-EQPM",
        [30.0, 0.0, 0.0],
        &[p("tag", "LP1"), sched("bus_amps", "225"), sched("main", "MLO"), p("fed_from", "MSB")],
    );
    // Level 2: room 201, the second panel (a feeder of the spec reaches it), a fan and a heat pump.
    for i in 0..2 {
        light(&mut d, "L2", "201", 4.0 + 6.0 * f64::from(i), 5.0);
    }
    device(&mut d, "L2", "receptacle", "201", 2.0, 0.5, &[p("circuit", "LP2-1")]);
    el(&mut d, "equipment", "L2", "E-EQPM", [30.0, 0.0, 10.0], &[p("tag", "LP2"), sched("bus_amps", "100"), sched("main", "MLO")]);
    // A unit panel down a feeder that loops through panels (the feeder run's chain reaches it).
    el(&mut d, "equipment", "L2", "E-EQPM", [34.0, 0.0, 10.0], &[p("tag", "LP4-2"), sched("bus_amps", "100"), sched("main", "MLO")]);
    el(&mut d, "equipment", "L2", "M-EQPM", [20.0, 20.0, 10.0], &[p("type_mark", "EF-9"), p("room", "201"), p("circuit", "LP2-5")]);
    el(&mut d, "equipment", "L2", "M-EQPM", [40.0, 20.0, 10.0], &[p("type_mark", "HP-9"), p("circuit", "LP2-7")]);
    let planted = plant.then(|| Planted {
        // A luminaire the reflected ceiling plan draws and no electrical sheet completes.
        dark_light: el(&mut d, "light", "L1", "E-LITE", [16.0, 12.0, 9.0], &[p("room", "101"), p("type_mark", "Z"), p("sheet", "A-121")]),
        bare_receptacle: device(&mut d, "L1", "receptacle", "101", 18.0, 0.5, &[]),
        // A switch in a room with no light.
        idle_switch: device(&mut d, "L2", "switch", "202", 25.0, 9.0, &[]),
        heat_pump: el(&mut d, "equipment", "L2", "M-EQPM", [60.0, 20.0, 10.0], &[p("type_mark", "HP-8")]),
        // Another unit's own panel right by it: never the one a new circuit goes on.
        _unit_panel: el(
            &mut d,
            "equipment",
            "L2",
            "E-EQPM",
            [58.0, 22.0, 10.0],
            &[p("tag", "LP5-205"), p("room", "205"), sched("bus_amps", "100"), sched("main", "MLO"), p("fed_from", "LP2")],
        ),
        unfed_panel: el(&mut d, "equipment", "L2", "E-EQPM", [50.0, 0.0, 10.0], &[p("tag", "LP3"), sched("bus_amps", "100"), sched("main", "MLO")]),
        // A fan whose circuit only a rule gave.
        assumed_fan: el(
            &mut d,
            "equipment",
            "L2",
            "M-EQPM",
            [24.0, 20.0, 10.0],
            &[p("type_mark", "EF-8"), p("room", "201"), rule("circuit", "LP2-9")],
        ),
    });
    let mut sp = BuildingSpec {
        levels: vec![
            serde_json::from_value(json!({"name": "L1", "elevation_ft": 0.0, "height_ft": 10.0})).unwrap(),
            serde_json::from_value(json!({"name": "L2", "elevation_ft": 10.0, "height_ft": 10.0})).unwrap(),
        ],
        ..BuildingSpec::default()
    };
    // The schedules: what the fans and heat pumps are; the feeder schedule reaches LP2.
    for (tag, kind) in [("EF-9", "exhaust_fan"), ("EF-8", "exhaust_fan"), ("HP-9", "heat_pump"), ("HP-8", "heat_pump")] {
        sp.mep.equipment.push(
            serde_json::from_value(
                json!({"tag": tag, "kind": kind, "level": "L2", "min": [0.0, 0.0], "max": [12.0, 12.0], "bottom_ft": 0.0, "height_ft": 3.0}),
            )
            .unwrap(),
        );
    }
    sp.mep.feeders.push(serde_json::from_value(json!({"tag": "F2", "from": "LP1", "to": "LP2", "sets": 1, "phases": 3})).unwrap());
    sp.mep.runs.push(
        serde_json::from_value(json!({"system": "E-FEEDER", "level": "L2", "source_sheet": "feeder 9 LP1\u{2192}LP4-1\u{2192}LP4-2", "points": [[0, 0], [400, 0]], "size_in": [2, 0], "bottoms_ft": [10]}))
            .unwrap(),
    );
    d.building_spec = Some(sp.to_json()).into();
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Building", None);
    (s, planted)
}

fn check<'a>(r: &'a Value, id: &str) -> &'a Value {
    r["requirements"]["checks"].as_array().unwrap().iter().find(|c| c["id"] == id).unwrap_or_else(|| panic!("no check {id}: {}", r["requirements"]))
}

fn requirement_items(r: &Value) -> Vec<&Value> {
    r["items"].as_array().unwrap().iter().filter(|i| i["kind"] == "requirement").collect()
}

fn prop(s: &mut Session, h: Handle, key: &str) -> Value {
    let g = s.execute("bim.props.get", &json!({"handle": h.hex()})).unwrap();
    g["elements"][0]["props"][key].clone()
}

#[test]
fn a_planted_set_raises_one_rfi_per_open_requirement_and_space() {
    let (mut s, planted) = model(true);
    let pl = planted.unwrap();
    let r = s.execute("rfi.read", &json!({})).unwrap();
    assert_eq!(r["built"], true);
    // Every requirement looked at its objects; the planted ones are open.
    let want = [
        ("E-LUM-CIRCUIT", 6, 1),
        ("E-LUM-TYPE", 6, 1),
        ("E-LUM-CONTROL", 6, 1),
        ("E-REC-CIRCUIT", 4, 1),
        ("E-SW-LOAD", 2, 1),
        ("E-EQ-CIRCUIT", 4, 2),
        ("E-PANEL-FEED", 5, 1),
    ];
    for (id, objects, open) in want {
        let c = check(&r, id);
        assert_eq!((c["objects"].as_u64(), c["open_objects"].as_u64()), (Some(objects), Some(open)), "{id}: {c}");
        assert_eq!(c["ran"], true, "{id}");
        // One RFI per space: the heat pump on the roof and the fan in room 201 are two.
        assert_eq!(c["findings"].as_u64(), Some(if id == "E-EQ-CIRCUIT" { 2 } else { 1 }), "{id}");
    }
    // The fan's circuit is the build's assumption: completed by a rule, still open.
    assert_eq!(check(&r, "E-EQ-CIRCUIT")["completed_by_rule"], 1);
    assert_eq!(check(&r, "E-EQ-CIRCUIT")["missing"], 1);
    // No equipment connection device: that requirement did not run (never "clean").
    let conn = check(&r, "E-CONN-CIRCUIT");
    assert_eq!((conn["ran"].as_bool(), conn["objects"].as_u64()), (Some(false), Some(0)));
    assert!(conn["status"].as_str().unwrap().starts_with("did not run"), "{conn}");
    // Nor did the breakers' requirements or the schedules' arithmetic: no panel schedule built.
    assert_eq!(r["requirements"]["did_not_run"], 6, "{}", r["requirements"]);
    assert_eq!(check(&r, "E-CIRCUIT-SCHEDULED")["status"], "did not run: no circuit on the plans in the model");

    // The RFIs, written like a contractor's: where, what each document shows, the question, the
    // proposal, the step they block.
    let items = requirement_items(&r);
    let lum = items.iter().find(|i| i["topic"].as_str().unwrap().starts_with("requirement: E-LUM-CIRCUIT")).unwrap();
    assert_eq!(lum["level"], "L1");
    assert_eq!(lum["severity"], "high");
    assert_eq!(lum["before"], "rough_in");
    assert!(lum["issue"].as_str().unwrap().contains("room 101"), "{lum}");
    assert!(lum["proposed"].as_str().unwrap().contains("Which circuit"), "{lum}");
    assert!(lum["assumed"].as_str().unwrap().contains("LP1-1"), "its room's luminaire's circuit: {lum}");
    assert_eq!(lum["elements"], json!([pl.dark_light.hex()]));
    assert!(lum["sheets"].as_array().unwrap().iter().any(|s| s == "A-121"), "the sheet that draws it: {lum}");
    assert!(lum["sides"].as_array().unwrap().iter().any(|s| s["document"].as_str().unwrap().contains("NEC 210.11")), "{lum}");
    let eq: Vec<&&Value> = items.iter().filter(|i| i["topic"].as_str().unwrap().starts_with("requirement: E-EQ-CIRCUIT")).collect();
    let of = |h: Handle| eq.iter().find(|i| i["elements"] == json!([h.hex()])).unwrap_or_else(|| panic!("{h:?}: {eq:?}"));
    // The heat pump: a new circuit on the nearest panel of its level; the fan: the rule's circuit.
    assert!(of(pl.heat_pump)["assumed"].as_str().unwrap().contains("spare of panel LP3"), "{}", of(pl.heat_pump));
    assert!(of(pl.assumed_fan)["issue"].as_str().unwrap().contains("assumed one by a rule"), "{}", of(pl.assumed_fan));
    assert!(of(pl.assumed_fan)["assumed"].as_str().unwrap().contains("LP2-9"));
    assert!(r["headline"].as_str().unwrap().ends_with("high"), "{}", r["headline"]);
    assert!(r["open"].as_u64().unwrap() >= 7, "{}", r["open"]);
    // Read only: nothing changed yet.
    assert!(prop(&mut s, pl.dark_light, "circuit").is_null());

    // Apply: each object gets its proposal, unsure, and the open mark; the RFIs go in the spec.
    let a = s.execute("rfi.read", &json!({"apply": true})).unwrap();
    assert!(a["applied"].as_u64().unwrap() >= 9, "{}", a["applied"]);
    let c = prop(&mut s, pl.dark_light, "circuit");
    assert_eq!(c["value"], "LP1-1", "{c}");
    assert_eq!(c["source"]["kind"], "rule");
    assert!(c["confidence"].as_f64().unwrap() < 0.5);
    assert!(c["note"].as_str().unwrap().starts_with("proposed, unsure"), "{c}");
    assert_eq!(prop(&mut s, pl.dark_light, "type_mark")["value"], "Z", "a value the drawing gives is never overwritten");
    assert!(prop(&mut s, pl.dark_light, "open:E-LUM-CIRCUIT")["value"].as_str().unwrap().starts_with("RFI-R"));
    assert!(prop(&mut s, pl.unfed_panel, "open:E-PANEL-FEED")["value"].is_string());
    assert!(prop(&mut s, pl.idle_switch, "open:E-SW-LOAD")["value"].is_string());
    assert!(prop(&mut s, pl.bare_receptacle, "open:E-REC-CIRCUIT")["value"].is_string());
    let list = s.execute("rfi.list", &json!({"kind": "requirement"})).unwrap();
    let n = list["total"].as_u64().unwrap();
    assert_eq!(n, 8, "{}", list["csv"]);
    assert_eq!(list["items"][0]["before"], "rough_in");

    // A proposed value is the build's: the luminaire's circuit now reads as completed by a rule,
    // still open, and the same RFI (its id survives).
    let again = s.execute("rfi.read", &json!({})).unwrap();
    assert_eq!(check(&again, "E-LUM-CIRCUIT")["completed_by_rule"], 1);
    let id = lum["id"].as_str().unwrap();
    assert!(requirement_items(&again).iter().any(|i| i["id"] == id));
    // An answered item keeps its answer through another pass.
    s.execute("rfi.set", &json!({"id": id, "status": "answered", "answer": "On the room's circuit."})).unwrap();
    s.execute("rfi.read", &json!({"apply": true})).unwrap();
    let kept = s.execute("rfi.list", &json!({"kind": "requirement", "status": "answered"})).unwrap();
    assert_eq!(kept["total"], 1);
    assert_eq!(kept["items"][0]["answer"], "On the room's circuit.");
    assert_eq!(s.execute("rfi.list", &json!({"kind": "requirement"})).unwrap()["total"], n, "no item twice");
}

#[test]
fn a_complete_set_has_nothing_open_and_every_check_says_what_it_looked_at() {
    let (mut s, _) = model(false);
    let r = s.execute("rfi.read", &json!({"apply": true})).unwrap();
    assert_eq!(r["requirements"]["findings"], 0, "{}", r["requirements"]);
    assert_eq!(r["requirements"]["open_objects"], 0);
    assert!(r["requirements"]["objects_checked"].as_u64().unwrap() >= 20);
    for id in ["E-LUM-CIRCUIT", "E-LUM-TYPE", "E-LUM-CONTROL", "E-REC-CIRCUIT", "E-SW-LOAD", "E-EQ-CIRCUIT", "E-PANEL-FEED"] {
        let c = check(&r, id);
        assert_eq!(c["ran"], true, "{id}");
        assert_eq!(c["objects"], c["met_by_documents"], "{id}: {c}");
    }
    assert!(requirement_items(&r).is_empty());
    assert_eq!(r["headline"], "0 things to ask before you build: 0 high");
    assert_eq!(s.execute("rfi.list", &json!({"kind": "requirement"})).unwrap()["total"], 0);
}

#[test]
fn with_no_model_built_rfi_read_says_so_instead_of_an_empty_list() {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let r = s.execute("rfi.read", &json!({})).unwrap();
    assert_eq!(r["built"], false);
    assert!(r["message"].as_str().unwrap().contains("no model built"), "{r}");
    assert_eq!(r["total"], 0);
}
