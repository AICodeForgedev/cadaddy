//! Q6206 branchgap: branch wiring as installed. A circuit label with no tag on its level is read
//! through the panel schedules, another level's tag and the riser; a switch's leg carries its
//! loop; homeruns sharing a raceway are sized by fill and derated together; a cable circuit is
//! its own cable from the panel through each box; a device with no circuit is proposed apart.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

/// A device tag on `level`: a `kind` at (x, y) feet on `circuit` (none: no circuit).
fn device(id: &str, kind: &str, level: &str, x: f64, y: f64, circuit: Option<&str>, mount_in: Option<f64>) -> Value {
    let mut f = json!({
        "type": {"value": "A", "source": "plan"},
        "symbol": {"value": id, "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
    });
    if let Some(c) = circuit {
        f["circuit"] = json!({"value": c, "source": "plan"});
    }
    if let Some(m) = mount_in {
        f["mount_height_in"] = json!({"value": format!("{m}"), "source": "note"});
    }
    json!({"id": format!("device:{level}:{id}"), "kind": kind, "level": level, "at_ft": [x, y], "fields": f})
}

fn circuit_tag(level: &str, label: &str, conductors: &str) -> Value {
    json!({"id": format!("circuit:{level}:E-101:{label}"), "kind": "circuit", "level": level, "at_ft": [0.0, 0.0], "fields": {
        "circuit": {"value": label, "source": "plan"},
        "panel": {"value": label.split('-').next().unwrap_or_default(), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "conductors": {"value": conductors, "source": "plan"},
        "conduit": {"value": "3/4\" EMT", "source": "default"},
        "route": {"value": "estimated route", "source": "rule"}
    }})
}

/// A schedule row: circuit `n`, its breaker amps and wire, a light load (180 VA: no voltage drop
/// upsizing on these short runs).
fn row(n: u32, what: &str, amps: u32, wire: &str) -> Value {
    json!({"number": n.to_string(), "description": what, "room": "", "breaker": format!("{amps}A"), "poles": 1, "wire": wire, "load_va": 180.0, "source": "E-601"})
}

/// A made-up two-storey building (12' floor to floor, 9' ceilings): panel LP1 on L1 (its top at
/// 6'-6", a 10" x 4" top conduit area), its schedule `rows`, the `tags`; `methods`: the
/// specification's wiring methods as the job reads them.
fn building(tags: Vec<Value>, rows: Vec<Value>, methods: Option<Value>) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    if let (Some(m), Ok(d)) = (methods, s.doc_mut()) {
        d.embedded.insert("route/methods.json".into(), std::sync::Arc::new(serde_json::to_vec(&m).unwrap_or_default()));
    }
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]},
            {"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}
        ],
        "tags": tags,
        "mep": {
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5,
                 "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
            ],
            "panel_schedules": [{"panel": "LP1", "source": "E-601", "circuits": rows}]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

fn report(s: &mut Session) -> Value {
    s.execute("conduit.route", &json!({"max": 2000})).expect("report")
}

fn spec_tags(s: &mut Session) -> Vec<Value> {
    let sp = s.execute("bspec.get", &json!({})).expect("spec");
    sp["spec"]["tags"].as_array().or(sp["tags"].as_array()).cloned().unwrap_or_default()
}

/// Each built device's handle and kind.
fn devices(s: &Session) -> Vec<(String, String)> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device"))
        .map(|e| (e.handle.hex(), cadaddy_engine::doc::props::read(&e.common.xdata).get("kind").map(|p| p.text()).unwrap_or_default()))
        .collect()
}

#[test]
fn a_label_tagged_only_on_another_levels_plan_or_schedule_is_routed_on_its_own_level() {
    // LP1-11 is tagged on L1's plan only; its two receptacles on L2 print the label with no tag
    // there; the schedule lists circuit 11. LP1-13: on L2 only, in the schedule. LP1-15: tagged
    // on L1, not in the schedule (its panel the schedule's). XQ-2: no schedule, riser or panel
    // names an XQ (an equipment mark, not a circuit).
    let tags = vec![
        device("R1", "receptacle", "L1", 10.0, 5.0, Some("LP1-11"), Some(18.0)),
        device("R2", "receptacle", "L2", 10.0, 5.0, Some("LP1-11"), Some(18.0)),
        device("R3", "receptacle", "L2", 18.0, 5.0, Some("LP1-11"), Some(18.0)),
        device("R4", "receptacle", "L2", 10.0, -12.0, Some("LP1-13"), Some(18.0)),
        device("R5", "receptacle", "L1", 30.0, 5.0, Some("LP1-15"), Some(18.0)),
        device("R6", "receptacle", "L2", 30.0, 5.0, Some("LP1-15"), Some(18.0)),
        device("R7", "receptacle", "L2", 40.0, 5.0, Some("XQ-2"), Some(18.0)),
        circuit_tag("L1", "LP1-11", "2#12 + 1#12G"),
        circuit_tag("L1", "LP1-15", "2#10 + 1#10G"),
    ];
    let mut s = building(tags, vec![row(11, "RECEPTACLES", 20, "12"), row(13, "RECEPTACLES", 20, "10")], None);
    let r = report(&mut s);
    assert_eq!(r["devices_without_tag_before"], json!(5), "R2, R3, R4, R6, R7: {}", r["tags_resolved"]);
    assert_eq!(r["devices_without_tag"], json!(1), "XQ-2 stays: {}", r["tags_unresolved"]);
    let items = r["tags_resolved"]["items"].as_array().cloned().unwrap_or_default();
    let by = |c: &str| items.iter().find(|x| x["circuit"] == json!(c) && x["level"] == json!("L2")).cloned().unwrap_or_default();
    assert_eq!((by("LP1-11")["by"].as_str(), by("LP1-11")["devices"].as_u64()), (Some("schedule"), Some(2)), "{items:?}");
    assert!(by("LP1-11")["source"].as_str().is_some_and(|t| t.contains("E-601") && t.contains("circuit 11")), "{items:?}");
    assert_eq!(by("LP1-13")["by"], json!("schedule"));
    assert_eq!(by("LP1-15")["by"], json!("plan"), "the L1 tag, its panel named by its schedule: {items:?}");
    let un = r["tags_unresolved"].as_array().cloned().unwrap_or_default();
    assert!(un.iter().any(|x| x["circuit"] == json!("XQ-2") && x["reason"].as_str().is_some_and(|w| w.contains("XQ"))), "{un:?}");
    // Each resolved circuit is routed on L2: up through the floor to LP1 on L1 (a reroute stores
    // its routing in the spec; the build keeps the spec as read).
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    let tags = spec_tags(&mut s);
    let on_l2 = |label: &str| {
        tags.iter().find(|t| t["kind"] == json!("circuit") && t["level"] == json!("L2") && t["fields"]["circuit"]["value"] == json!(label)).cloned()
    };
    for label in ["LP1-11", "LP1-13", "LP1-15"] {
        let t = on_l2(label).unwrap_or_else(|| panic!("{label} on L2: {tags:?}"));
        assert_eq!(t["fields"]["route"]["value"], json!("routed"), "{t}");
        assert!(t["fields"]["resolved"]["value"].as_str().is_some_and(|v| !v.is_empty()), "{t}");
    }
    // The schedule's wire on LP1-13 (#10); the L1 tag's conductors on LP1-15.
    assert!(on_l2("LP1-13").is_some_and(|t| t["fields"]["conductors"]["value"].as_str().is_some_and(|c| c.contains("#10"))));
    assert!(on_l2("LP1-15").is_some_and(|t| t["fields"]["conductors"]["value"].as_str().is_some_and(|c| c.contains("#10"))));
    assert!(r["risers"].as_array().is_some_and(|a| a.iter().any(|x| x["level"] == json!("L2"))), "{}", r["risers"]);
    // Routing again starts from the labels, not from the last routing's resolved tags.
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    let again = report(&mut s);
    assert_eq!(again["devices_without_tag_before"], json!(5), "{}", again["tags_resolved"]);
    assert_eq!(again["tags_resolved"]["devices"], r["tags_resolved"]["devices"]);
    let n = spec_tags(&mut s)
        .iter()
        .filter(|t| t["kind"] == json!("circuit") && t["level"] == json!("L2") && t["fields"]["circuit"]["value"] == json!("LP1-11"))
        .count();
    assert_eq!(n, 1);
}

#[test]
fn a_switch_loop_carries_its_conductors_three_way_travelers_included() {
    // LP1-9's luminaire; a single-pole switch beside it controls it; LP1-7's luminaire with a
    // pair of 3-way switches.
    let tags = vec![
        device("L1", "luminaire", "L1", 20.0, 10.0, Some("LP1-9"), None),
        device("S1", "switch", "L1", 14.0, 10.0, None, Some(48.0)),
        device("L2", "luminaire", "L1", 20.0, -20.0, Some("LP1-7"), None),
        device("S2", "switch", "L1", 12.0, -20.0, None, Some(48.0)),
        device("S3", "switch", "L1", 28.0, -20.0, None, Some(48.0)),
        circuit_tag("L1", "LP1-9", "2#12 + 1#12G"),
        circuit_tag("L1", "LP1-7", "2#12 + 1#12G"),
    ];
    let mut s = building(tags, vec![row(7, "LIGHTING", 20, "12"), row(9, "LIGHTING", 20, "12")], None);
    let dv = devices(&s);
    let d = s.doc().expect("doc");
    let at = |kind: &str, x: f64, y: f64| {
        dv.iter()
            .filter(|v| v.1 == kind)
            .find(|v| {
                let h = u64::from_str_radix(&v.0, 16).ok().map(cadaddy_engine::doc::Handle);
                h.and_then(|h| d.model.get(h)).is_some_and(|e| {
                    let b = cadaddy_engine::doc::entity_bounds(d, e, 0);
                    ((b.min.x + b.max.x) / 24.0 - x).abs() < 1.0 && ((b.min.y + b.max.y) / 24.0 - y).abs() < 1.0
                })
            })
            .map(|v| v.0.clone())
            .unwrap_or_else(|| panic!("{kind} at {x},{y}: {dv:?}"))
    };
    let (l1, s1, l2, s2, s3) =
        (at("luminaire", 20.0, 10.0), at("switch", 14.0, 10.0), at("luminaire", 20.0, -20.0), at("switch", 12.0, -20.0), at("switch", 28.0, -20.0));
    for (h, k, v) in
        [(&s1, "controls", &l1), (&s2, "controls", &l2), (&s3, "controls", &l2), (&s2, "three_way_with", &s3), (&s3, "three_way_with", &s2)]
    {
        s.execute("bim.props.set", &json!({"handle": h, "key": k, "value": v})).expect("set");
    }
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    let r = report(&mut s);
    assert_eq!(r["switch_legs"], json!(3), "{r}");
    assert_eq!(r["switch_loop_legs"], json!(3), "each switch's leg: {r}");
    let tags = spec_tags(&mut s);
    let loop_of = |label: &str| {
        tags.iter()
            .filter(|t| t["fields"]["circuit"]["value"] == json!(label) && t["fields"]["switch_loop_of"].is_object())
            .map(|t| t["fields"]["conductors"]["value"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>()
    };
    // Single-pole: line, switched leg and the grounded conductor (404.2(C)), and the ground.
    assert_eq!(loop_of("LP1-9"), vec!["3#12 + 1#12G".to_string()], "{tags:?}");
    // 3-way: the common, two travelers and the grounded conductor.
    assert_eq!(loop_of("LP1-7"), vec!["4#12 + 1#12G".to_string()], "{tags:?}");
    // The takeoff's wire on the loops: the loop's conductors.
    let w = s.execute("bim.count", &json!({"kind": "wire", "circuit": "LP1-7"})).expect("count");
    assert!(w["count"].as_u64().unwrap_or(0) >= 5, "{w}");
}

#[test]
fn a_four_circuit_homerun_is_sized_by_fill_and_its_conductors_derated() {
    // Four 30 A circuits of 2#10 (schedule) a few feet apart, the specification letting four share
    // a homerun: 8 current-carrying conductors ⟹ 70 % (310.15(C)(1)): #10 THHN 40 A × 0.7 = 28 A
    // under its 30 A breaker ⟹ #8 (55 A × 0.7 = 38.5 A, the 75 °C column 50 A); the ground up
    // with them (250.122(B)): 8#8 + 1#8G, 9 × 0.0366 = 0.329 sq in ⟹ 1" EMT (0.864 × 40 % =
    // 0.346; 3/4" holds 0.213).
    let tags = vec![
        device("R1", "receptacle", "L1", 8.0, 6.0, Some("LP1-1"), Some(18.0)),
        device("R3", "receptacle", "L1", 10.0, 6.0, Some("LP1-3"), Some(18.0)),
        device("R5", "receptacle", "L1", 12.0, 6.0, Some("LP1-5"), Some(18.0)),
        device("R7", "receptacle", "L1", 14.0, 6.0, Some("LP1-7"), Some(18.0)),
        circuit_tag("L1", "LP1-1", "2#10 + 1#10G"),
        circuit_tag("L1", "LP1-3", "2#10 + 1#10G"),
        circuit_tag("L1", "LP1-5", "2#10 + 1#10G"),
        circuit_tag("L1", "LP1-7", "2#10 + 1#10G"),
    ];
    let methods = json!({"applications": [], "max_circuits": 4, "max_circuits_source": "spec 26 05 19: up to four circuits per homerun"});
    let rows = (1..=7).step_by(2).map(|n| row(n, "RECEPTACLES", 30, "10")).collect();
    let mut s = building(tags, rows, Some(methods));
    let r = report(&mut s);
    assert_eq!(r["homeruns"], json!(1), "{r}");
    let fill = r["fill"].as_array().and_then(|a| a.first()).cloned().unwrap_or_default();
    assert_eq!(fill["conductors"], json!("8#8 + 1#8G"), "{fill}");
    assert_eq!(fill["size_in"], json!(1.0), "{fill}");
    let der = r["derated"].as_array().and_then(|a| a.first()).cloned().unwrap_or_default();
    assert_eq!((der["ccc"].as_u64(), der["factor"].as_f64(), der["upsized"].as_bool()), (Some(8), Some(0.7), Some(true)), "{der}");
    assert!(der["allowed_a"].as_f64().is_some_and(|a| a >= 30.0), "{der}");
    // The homerun's pieces: 1" conduit carrying #8.
    let c = s.execute("bim.count", &json!({"kind": "conduit", "group_by": "size"})).expect("count");
    assert!(c["rows"].as_array().is_some_and(|rows| rows.iter().any(|x| x["size"] == json!("1\""))), "{c}");
}

#[test]
fn an_nm_circuit_through_three_boxes_is_its_own_cable_from_the_panel_exactly() {
    // The specification's branch circuits in NM cable; LP1-21's receptacles at 20', 30' and 40'
    // along y = 10'. With no walls to run in, each leg goes up from its box to the framing over
    // the 9' ceiling (the first tier, 9'-3"), across and down into the next box; the cable's own
    // homerun up from the first box, across to the panel's entry slot and down onto its top at
    // 6'-6"; 1.5' of makeup at each box end, 3.5' at the panel (half its 4' can and a box's).
    let tags = vec![
        device("R1", "receptacle", "L1", 20.0, 10.0, Some("LP1-21"), Some(18.0)),
        device("R2", "receptacle", "L1", 30.0, 10.0, Some("LP1-21"), Some(18.0)),
        device("R3", "receptacle", "L1", 40.0, 10.0, Some("LP1-21"), Some(18.0)),
        circuit_tag("L1", "LP1-21", "2#12 + 1#12G"),
    ];
    let methods = json!({"applications": [
        {"section": "26 05 19", "item": "B", "application": "Branch Circuits Concealed in Ceilings, Walls, and Partitions", "methods": ["nm"]}
    ]});
    let mut s = building(tags, vec![row(21, "RECEPTACLES", 20, "12")], Some(methods));
    let r = report(&mut s);
    let c = r["circuit_lengths"].as_array().and_then(|a| a.iter().find(|x| x["circuit"] == json!("LP1-21"))).cloned().unwrap_or_default();
    assert_eq!(c["method"], json!("NM"), "{c}");
    // The box tops: a receptacle's box, its top over 18" (the built device's).
    let d = s.doc().expect("doc");
    let top = d
        .model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device"))
        .filter_map(|e| match &e.kind {
            cadaddy_engine::doc::EntityKind::Mesh(m) => Some(m.vertices.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max)),
            _ => None,
        })
        .fold(f64::NEG_INFINITY, f64::max);
    let z0 = 111.0;
    let rise = (z0 - top) / 12.0;
    let leg = 10.0 + 2.0 * rise;
    let land = (-14.0 - 3.875, -0.875);
    let home = rise + ((240.0 - land.0) + (120.0 - land.1)) / 12.0 + (z0 - 78.0) / 12.0;
    let want = 2.0 * (leg + 2.0 * 1.5) + (home + 1.5 + 3.5);
    let got = c["cable_ft"].as_f64().unwrap_or(0.0);
    assert!((got - want).abs() < 0.06, "cable {got} vs {want}: {c}");
    assert!(c["cable_ft_by_size"]["12/2 w/G"].as_f64().is_some_and(|v| (v - want).abs() < 0.06), "{c}");
    // Per level and in all, by size.
    assert!(r["lengths"]["levels"][0]["cable_ft_by_size"]["12/2 w/G"].as_f64().is_some_and(|v| (v - want).abs() < 0.06), "{}", r["lengths"]);
    // The takeoff's cable: its legs and homerun with the makeup.
    let k = s.execute("bim.count", &json!({"kind": "cable", "circuit": "LP1-21"})).expect("count");
    assert!(k["length_ft"].as_f64().is_some_and(|x| (x - want).abs() < 0.5), "{want} vs {k}");
}

#[test]
fn a_receptacle_with_no_circuit_is_proposed_on_a_spare_not_drawn_and_priced_apart() {
    let tags = vec![
        device("R1", "receptacle", "L1", 20.0, 10.0, Some("LP1-1"), Some(18.0)),
        device("R9", "receptacle", "L1", 25.0, -8.0, None, Some(18.0)),
        device("F1", "smoke_detector", "L1", 25.0, -10.0, None, None),
        circuit_tag("L1", "LP1-1", "2#12 + 1#12G"),
    ];
    let mut s = building(tags, vec![row(1, "RECEPTACLES", 20, "12"), row(3, "SPARE", 20, ""), row(5, "SPACE", 0, "")], None);
    let r = report(&mut s);
    let p = &r["proposed_circuits"];
    assert_eq!(p["total"], json!(1), "the receptacle only (a smoke detector is not a branch power device): {p}");
    let it = &p["items"][0];
    assert_eq!((it["panel"].as_str(), it["breaker"].as_str(), it["drawn"].as_bool()), (Some("LP1"), Some("spare 3"), Some(false)), "{it}");
    assert!(it["estimated_ft"].as_f64().is_some_and(|f| f > 30.0), "{it}");
    assert!(it["review"].as_str().is_some_and(|t| t.contains("no circuit")) && it["rfi"].as_str().is_some_and(|t| t.starts_with("Confirm")), "{it}");
    assert_eq!(p["by_level_kind"]["L1"]["receptacle"], json!(1));
    // Not drawn: only LP1-1 is routed, and no circuit tag stands for the proposal.
    assert_eq!(r["routed_circuits"], json!(1), "{r}");
    assert!(spec_tags(&mut s).iter().all(|t| t["kind"] != json!("circuit") || t["fields"]["circuit"]["value"] == json!("LP1-1")));
}

#[test]
fn each_leg_is_sized_by_its_fill_and_each_box_by_what_enters_it() {
    // LP1-31: two receptacles on 4#6 + 1#6G (a multiwire circuit the plan notes). Each leg carries
    // 5 conductors of 0.0507 sq in (0.254): more than 3/4" EMT's 40 % (0.213), so 1". The far
    // box takes 4#6 and a ground and a yoke, 35 cu in (314.16(B): 5 each, the ground one, the
    // yoke two): more than a 4" square (30.3), a 4-11/16" square (42).
    let tags = vec![
        device("R1", "receptacle", "L1", 10.0, 6.0, Some("LP1-31"), Some(18.0)),
        device("R2", "receptacle", "L1", 22.0, 6.0, Some("LP1-31"), Some(18.0)),
        circuit_tag("L1", "LP1-31", "4#6 + 1#6G"),
    ];
    let mut s = building(tags, vec![row(31, "RECEPTACLES", 20, "6")], None);
    let r = report(&mut s);
    assert!(r["legs_fill_sized"].as_u64().is_some_and(|n| n >= 1), "{r}");
    let c = s.execute("bim.count", &json!({"kind": "conduit", "circuit": "LP1-31", "group_by": "size"})).expect("count");
    let rows = c["rows"].as_array().cloned().unwrap_or_default();
    assert!(rows.iter().all(|x| x["size"] != json!("3/4\"") && x["size"] != json!("1/2\"")), "no leg under 1\": {c}");
    let b = s.execute("bim.count", &json!({"kind": "box", "circuit": "LP1-31", "group_by": "size"})).expect("count");
    let rows = b["rows"].as_array().cloned().unwrap_or_default();
    assert!(rows.iter().any(|x| x["size"] == json!("4-11/16\" square")), "{b}");
}
