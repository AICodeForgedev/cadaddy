//! Q6166 breakers: each panel-schedule row is a breaker inside its panel, with its trip, poles,
//! type, circuit, load and source row; typical room panels each take their schedule's set; spares
//! and spaces are counted; the BOM lists breakers by type, trip and poles per panel type and
//! level; plan circuits and schedule rows that don't meet are review items. Made-up buildings
//! only, in several schedule styles (a legend or none, a header count or none, a typical
//! schedule, circuits kept on the equipment record).

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

fn ckt(n: &str, desc: &str, amps: &str, poles: u32, codes: &[&str]) -> Value {
    json!({"number": n, "description": desc, "breaker": amps, "poles": poles, "protection": codes, "source": "X-2"})
}

fn device(id: &str, kind: &str, x: f64, circuit: &str) -> Value {
    json!({"id": id, "kind": kind, "level": "L1", "at_ft": [x, 10.0], "fields": {
        "type": {"value": "A", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x1F0 + (x * 10.0) as u64), "source": "plan"},
        "sheet": {"value": "X-101", "source": "plan"},
        "circuit": {"value": circuit, "source": "plan"}
    }})
}

/// A made-up building: panel LP1 (fed from MDP) with a legend and a header count, two typical
/// room panels U-101 and U-102 on a schedule `TYPICAL OF 3`, panel LP2 whose circuits are on
/// its equipment record only, and a schedule for a panel P9 the model doesn't have.
fn building() -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": [
            device("device:L1:1", "luminaire", 5.0, "LP1-1"),
            device("device:L1:2", "receptacle", 6.0, "LP1-2"),
            device("device:L1:3", "receptacle", 7.0, "LP1-2"),
            device("device:L1:4", "receptacle", 8.0, "LP1-9"),
            device("device:L1:5", "receptacle", 30.0, "U-1"),
            device("device:L1:6", "receptacle", 31.0, "U-2"),
            device("device:L1:7", "receptacle", 50.0, "U-1"),
            device("device:L1:8", "receptacle", 51.0, "U-13,15"),
            device("device:L1:9", "receptacle", 70.0, "U-1"),
            device("device:L1:10", "receptacle", 90.0, "LP2-1"),
            {"id": "circuit:L1:X-101:LP1-7", "kind": "circuit", "level": "L1", "at_ft": [2.0, 10.0], "fields": {
                "circuit": {"value": "LP1-7", "source": "plan"}, "panel": {"value": "LP1", "source": "plan"}
            }}
        ],
        "mep": {
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, 110], "max": [-4, 130], "height_ft": 4, "bottom_ft": 2},
                {"tag": "U-101", "kind": "panel", "level": "L1", "min": [300, 110], "max": [314, 114], "height_ft": 2, "bottom_ft": 4,
                 "properties": {"PANEL": "U", "ROOM": "101"}},
                {"tag": "U-102", "kind": "panel", "level": "L1", "min": [500, 110], "max": [514, 114], "height_ft": 2, "bottom_ft": 4,
                 "properties": {"PANEL": "U", "ROOM": "102"}},
                {"tag": "LP2", "kind": "panel", "level": "L1", "min": [1000, 110], "max": [1020, 130], "height_ft": 3, "bottom_ft": 2,
                 "circuits": [
                     {"number": "1", "description": "OFFICE RCPT", "breaker": "20", "poles": 1, "source": "X-3"},
                     {"number": "3", "description": "WATER HEATER", "breaker": "30A", "poles": 2, "source": "X-3"},
                     {"number": "5", "description": "", "breaker": "", "poles": 1, "source": "X-3"}
                 ]},
                {"tag": "DPB-1", "kind": "damper_power_box", "level": "L1", "min": [700, 110], "max": [706, 114], "height_ft": 1, "bottom_ft": 9,
                 "properties": {"PANEL": "LP1", "CIRCUIT": "7"}},
                {"tag": "RTU-1", "kind": "rooftop_unit", "level": "L1", "min": [-300, 300], "max": [-200, 400], "height_ft": 4},
                {"tag": "MDP", "kind": "switchboard", "level": "L1", "min": [-200, 100], "max": [-150, 140], "height_ft": 7}
            ],
            "feeders": [{"tag": "F1", "from": "MDP", "to": "LP1", "sets": 1, "phases": 3, "size": "#1", "material": "cu", "neutrals": 1, "grounds": 1, "ground_size": "#6", "conduits": 1, "conduit_in": 1.5, "entry": "top"}],
            "panel_schedules": [
                {"panel": "LP1", "source": "X-2",
                 "header": {"aic rating": "10,000", "circuits": "12", "volts/phase": "208Y/120V, 3PH, 4W"},
                 "legend": {"GF": "GROUND FAULT CIRCUIT INTERRUPTER", "ST": "SHUNT TRIP", "LT": "LOCKING TAB"},
                 "circuits": [
                     ckt("1", "LIGHTS", "20", 1, &["LT"]),
                     ckt("2", "BATH RCPT", "20", 1, &["GF"]),
                     ckt("3", "RTU-1 (HACR)", "40", 2, &[]),
                     ckt("4", "SPARE", "20", 1, &[]),
                     ckt("6", "SPACE", "", 1, &[]),
                     ckt("7", "COPIER", "20", 1, &["ST"]),
                     ckt("8", "SIGN", "20", 1, &[])
                 ]},
                {"panel": "U", "typical_of": 3, "source": "X-2",
                 "legend": {"AF": "COMBINATION TYPE ARC FAULT CIRCUIT INTERRUPTER", "GF": "GROUND FAULT INTERRUPTER"},
                 "circuits": [
                     ckt("1", "ROOM RCPT", "20", 1, &["AF"]),
                     ckt("2", "BATH RCPT", "20", 1, &["AF", "GF"]),
                     ckt("13", "ROOM HEAT PUMP", "20", 2, &[]),
                     ckt("4", "SPACE ONLY", "", 1, &[])
                 ]},
                {"panel": "MDP", "source": "X-2", "circuits": [
                    ckt("1", "PANELBOARD LP1", "100", 3, &[]), ckt("2", "PANELBOARD LP2", "100", 3, &[]), ckt("3", "SPACE", "", 3, &[])
                ]},
                {"panel": "P9", "source": "X-4", "circuits": [ckt("1", "PUMP", "15", 1, &[])]}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": false})).expect("build");
    s
}

fn device_handles(s: &mut Session, circuit: &str) -> Vec<String> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device"))
        .filter(|e| cadaddy_engine::doc::props::read(&e.common.xdata).get("circuit").map(|p| p.text()).as_deref() == Some(circuit))
        .map(|e| e.handle.hex())
        .collect()
}

fn panel_handle(s: &mut Session, tag: &str) -> String {
    let d = s.doc().expect("doc");
    d.model.iter().find(|e| cadaddy_engine::cmd::bimlook::made_tag(e, "tag") == Some(tag)).map(|e| e.handle.hex()).expect("panel")
}

/// The room each typical device stands in, as the fill would give it.
fn rooms(s: &mut Session) {
    let mut by_x: Vec<(f64, String)> = {
        let d = s.doc().expect("doc");
        d.model
            .iter()
            .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device"))
            .filter(|e| cadaddy_engine::doc::props::read(&e.common.xdata).get("panel").map(|p| p.text()).as_deref() == Some("U"))
            .map(|e| (cadaddy_engine::doc::entity_bounds(d, e, 0).min.x, e.handle.hex()))
            .collect()
    };
    by_x.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (x, h) in by_x {
        let room = if x < 40.0 * 12.0 {
            "101"
        } else if x < 60.0 * 12.0 {
            "102"
        } else {
            "103"
        };
        s.execute("bim.props.set", &json!({"handles": [h], "key": "room", "value": room})).expect("room");
    }
}

fn props_of(s: &mut Session, h: &str) -> Value {
    let v = s.execute("bim.props.get", &json!({"handles": [h]})).expect("props");
    v["elements"][0]["props"].clone()
}

fn breakers_of(s: &mut Session, panel: &str) -> Vec<Value> {
    let d = s.doc().expect("doc");
    let hs: Vec<String> = d
        .model
        .iter()
        .filter(|e| cadaddy_engine::cmd::bimlook::made_tag(e, "part") == Some("atom:breaker"))
        .filter(|e| cadaddy_engine::doc::props::read(&e.common.xdata).get("panel").map(|p| p.text()).as_deref() == Some(panel))
        .map(|e| e.handle.hex())
        .collect();
    hs.iter().map(|h| props_of(s, h)).collect()
}

fn val<'a>(p: &'a Value, k: &str) -> &'a Value {
    &p[k]["value"]
}

#[test]
fn each_schedule_row_is_a_breaker_inside_its_panel_with_what_the_row_says() {
    let mut s = building();
    let lp1 = breakers_of(&mut s, "LP1");
    // Seven rows: six breakers (one a spare), one space.
    assert_eq!(lp1.len(), 6, "{lp1:?}");
    let by_circuit = |c: &str| lp1.iter().find(|b| val(b, "circuit") == c).cloned().unwrap_or(Value::Null);
    let bath = by_circuit("LP1-2");
    assert_eq!(val(&bath, "type"), "GFCI", "{bath}");
    assert_eq!(val(&bath, "trip_a"), 20.0);
    assert_eq!(val(&bath, "poles"), 1);
    assert_eq!(val(&bath, "load"), "BATH RCPT");
    assert_eq!(val(&bath, "aic"), "10,000", "the panel header's rating: {bath}");
    assert_eq!(bath["type"]["source"]["kind"], "schedule");
    assert!(bath["type"]["source"]["doc"].as_str().is_some_and(|t| t.contains("GF: GROUND FAULT CIRCUIT INTERRUPTER")), "{bath}");
    assert!(val(&bath, "source_row").as_str().is_some_and(|t| t.starts_with("X-2 panel LP1 row 2")), "{bath}");
    // A locking tab is no protection; a description's HACR and a legend's shunt trip are types.
    assert_eq!(val(&by_circuit("LP1-1"), "type"), "standard");
    let rtu = by_circuit("LP1-3,5");
    assert_eq!((val(&rtu, "type"), val(&rtu, "poles"), val(&rtu, "size")), (&json!("HACR"), &json!(2), &json!("40A 2P")), "{rtu}");
    assert_eq!(val(&by_circuit("LP1-7"), "type"), "shunt");
    assert_eq!(val(&by_circuit("LP1-4"), "spare"), "true");
    // The panel: its breakers among what it connects to, its spares, spaces and poles.
    let ph = panel_handle(&mut s, "LP1");
    let pp = props_of(&mut s, &ph);
    assert_eq!((val(&pp, "breakers"), val(&pp, "spares"), val(&pp, "spaces")), (&json!(5), &json!(1), &json!(1)), "{pp}");
    assert_eq!(val(&pp, "poles_used"), 7, "1+1+2+1+1+1: {pp}");
    assert_eq!(val(&pp, "poles_total"), 12, "the header's count: {pp}");
    let cn = val(&pp, "connects").as_str().unwrap_or_default().to_string();
    assert_eq!(lp1.iter().filter(|b| b["connects"]["value"].as_str().is_some_and(|c| c.contains(&ph))).count(), 6);
    assert!(cn.split(',').count() >= 6, "{cn}");
    // Gear whose `PANEL` is the panel feeding it (a damper's power box) holds no breakers; it is
    // a load on its circuit, linked to the breaker.
    assert!(breakers_of(&mut s, "DPB-1").is_empty());
    let dpb = panel_handle(&mut s, "DPB-1");
    let copier = by_circuit("LP1-7");
    assert!(val(&copier, "connects").as_str().is_some_and(|c| c.split(',').any(|h| h == dpb)), "{copier}");
    let b = val(&props_of(&mut s, &dpb), "breaker_handle").as_str().unwrap_or_default().to_string();
    assert_eq!(val(&props_of(&mut s, &b), "circuit"), "LP1-7");
    // A switchboard numbering its breakers (1, 2, 3 of three poles each), not their poles.
    let mh = panel_handle(&mut s, "MDP");
    let mp = props_of(&mut s, &mh);
    assert_eq!(
        (val(&mp, "breakers"), val(&mp, "spaces"), val(&mp, "poles_used"), val(&mp, "poles_total")),
        (&json!(2), &json!(3), &json!(6), &json!(9)),
        "{mp}"
    );
    assert!(breakers_of(&mut s, "MDP").iter().any(|b| val(b, "circuit") == "MDP-2" && val(b, "poles") == 3));
    // A panel whose circuits are on its equipment record: those rows; a blank row is a space.
    let lp2 = breakers_of(&mut s, "LP2");
    assert_eq!(lp2.len(), 2, "{lp2:?}");
    assert!(lp2.iter().any(|b| val(b, "size") == "30A 2P" && val(b, "type") == "standard" && val(b, "load") == "WATER HEATER"), "{lp2:?}");
    // Counted in the model's index.
    let n = s.execute("bim.count", &json!({"kind": "breakers", "group_by": "type"})).expect("count");
    assert_eq!(n["from"], "index");
    let typed: Vec<(String, u64)> = n["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| (r["type"].as_str().unwrap_or_default().to_string(), r["count"].as_u64().unwrap_or(0)))
        .collect();
    assert!(typed.contains(&("GFCI".into(), 1)) && typed.contains(&("shunt".into(), 1)), "{n}");
}

#[test]
fn typical_room_panels_each_take_the_schedules_breakers_and_their_rooms_devices() {
    let mut s = building();
    for tag in ["U-101", "U-102"] {
        let b = breakers_of(&mut s, tag);
        // Three breakers each; the 2-pole one takes 13 and 15; SPACE ONLY is a space.
        assert_eq!(b.len(), 3, "{tag}: {b:?}");
        assert!(b.iter().any(|x| val(x, "type") == "dual-function" && val(x, "circuit") == "U-2"), "{b:?}");
        assert!(b.iter().any(|x| val(x, "type") == "AFCI" && val(x, "room") == tag.trim_start_matches("U-")), "{b:?}");
        assert!(b.iter().any(|x| val(x, "circuit") == "U-13,15" && val(x, "poles") == 2), "{b:?}");
        let pp = {
            let h = panel_handle(&mut s, tag);
            props_of(&mut s, &h)
        };
        assert_eq!((val(&pp, "poles_used"), val(&pp, "spaces"), val(&pp, "poles_total")), (&json!(4), &json!(1), &json!(16)), "{pp}");
    }
    // The rooms known (the fill's), each device links to its room panel's breaker.
    rooms(&mut s);
    let r = s.execute("bim.breakers", &json!({})).expect("breakers");
    let u101 = breakers_of(&mut s, "U-101");
    let room_rcpt = u101.iter().find(|x| val(x, "circuit") == "U-1").cloned().unwrap_or_default();
    assert_eq!(val(&room_rcpt, "devices"), 1, "{room_rcpt}");
    for h in device_handles(&mut s, "U-1") {
        let p = props_of(&mut s, &h);
        let room = val(&p, "room").as_str().unwrap_or_default().to_string();
        let b = val(&p, "breaker_handle").as_str().map(str::to_string);
        if room == "103" {
            assert!(b.is_none(), "no panel U-103 in the model: {p}");
        } else {
            let bp = props_of(&mut s, b.as_deref().unwrap_or_default());
            assert_eq!(val(&bp, "panel"), format!("U-{room}").as_str(), "{bp}");
            assert_eq!(val(&bp, "circuit"), "U-1");
        }
    }
    // The 2-pole label `U-13,15` finds its breaker by its first number.
    let ptac = device_handles(&mut s, "U-13,15");
    assert_eq!(ptac.len(), 1);
    let p = props_of(&mut s, &ptac[0]);
    assert!(val(&p, "breaker_handle").is_string(), "{p}");
    // bim.chain from a device names its breaker.
    let chain = s.execute("bim.chain", &json!({"handle": ptac[0]})).expect("chain");
    assert_eq!(chain["breaker"]["poles"], 2, "{chain}");
    assert_eq!(chain["breaker"]["panel"], "U-102", "{chain}");
    // Typical of 3, two built; room 103's devices have no panel.
    let t = r["typical"].as_array().expect("typical").iter().find(|t| t["panel_type"] == "U").cloned().unwrap_or_default();
    assert_eq!((t["typical_of"].as_u64(), t["panels_built"].as_u64(), t["rooms_without_panel"].as_u64()), (Some(3), Some(2), Some(1)), "{r}");
    assert_eq!(t["rooms"], json!(["103"]));
}

#[test]
fn the_bom_lists_breakers_by_type_trip_and_poles_per_panel_type_and_level() {
    let mut s = building();
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    let lines: Vec<&Value> = bom["lines"].as_array().expect("lines").iter().filter(|l| l["category"] == "breakers").collect();
    let line = |item: &str, size: &str| lines.iter().find(|l| l["item"] == item && l["size"] == size).map(|l| l["net"].as_f64().unwrap_or(0.0));
    // The typical room panels' breakers: one set per built room panel.
    assert_eq!(line("Circuit breaker, AFCI, panel U", "20A 1P"), Some(2.0), "{lines:?}");
    assert_eq!(line("Circuit breaker, dual-function AFCI/GFCI, panel U", "20A 1P"), Some(2.0));
    assert_eq!(line("Circuit breaker, panel U", "20A 2P"), Some(2.0));
    // LP1: lights, the spare and the sign are standard 20 A single-pole breakers.
    assert_eq!(line("Circuit breaker, panel LP1", "20A 1P"), Some(3.0), "{lines:?}");
    assert_eq!(line("Circuit breaker, GFCI, panel LP1", "20A 1P"), Some(1.0));
    assert_eq!(line("Circuit breaker, HACR, panel LP1", "40A 2P"), Some(1.0));
    assert_eq!(line("Circuit breaker, shunt trip, panel LP1", "20A 1P"), Some(1.0));
    assert!(lines.iter().all(|l| l["level"] == "L1" && l["trade"] == "electrical" && l["unit"] == "each"), "{lines:?}");
    // The switchboard's feeder breakers: MDP is gear of the model too.
    assert_eq!(line("Circuit breaker, panel MDP", "100A 3P"), Some(2.0));
}

#[test]
fn plan_circuits_and_schedule_rows_that_dont_meet_are_review_items() {
    let mut s = building();
    let r = s.execute("bim.breakers", &json!({"rebuild": false})).expect("breakers");
    let m: Vec<(String, String, String)> = r["mismatches"]
        .as_array()
        .expect("mismatches")
        .iter()
        .map(|m| {
            (
                m["kind"].as_str().unwrap_or_default().into(),
                m["panel"].as_str().unwrap_or_default().into(),
                m["circuit"].as_str().unwrap_or_default().into(),
            )
        })
        .collect();
    // A device on LP1-9: no row 9.
    assert!(m.contains(&("plan_no_row".into(), "LP1".into(), "LP1-9".into())), "{m:?}");
    // The sign's row: nothing on the plans; the copier's row has a homerun tag; RTU-1's row
    // names equipment the model has; MDP's row feeds a panel; spares and spaces never count.
    assert!(m.contains(&("row_no_plan".into(), "LP1".into(), "LP1-8".into())), "{m:?}");
    for c in ["LP1-7", "LP1-3", "LP1-4", "LP1-6", "MDP-1", "MDP-2", "LP1-1", "LP1-2"] {
        assert!(!m.iter().any(|x| x.2 == c), "{c}: {m:?}");
    }
    // LP2's water heater row: nothing on the plans either.
    assert!(m.contains(&("row_no_plan".into(), "LP2".into(), "LP2-3".into())), "{m:?}");
    // A schedule with no panel in the model.
    assert!(m.iter().any(|x| x.0 == "no_panel" && x.1 == "P9"), "{m:?}");
    assert_eq!(r["schedules_without_panel"], json!(["P9"]));
    // In the review queue as conflicts; resolving one records the decision.
    let list = s.execute("review.list", &json!({"kind": "conflict", "limit": 50})).expect("list");
    let items = list["items"].as_array().expect("items");
    let plan = items.iter().find(|i| i["title"].as_str().is_some_and(|t| t.contains("LP1-9"))).cloned().expect("LP1-9 item");
    assert!(plan["title"].as_str().is_some_and(|t| t.contains("no breaker")), "{plan}");
    let next = s.execute("review.next", &json!({"id": plan["id"]})).expect("next");
    assert_eq!(next["item"]["detail"]["breakers"], "plan_no_row", "{next}");
    let done = s
        .execute("review.resolve", &json!({"id": plan["id"], "answer": "the plans are right", "citation": {"kind": "drawing", "document": "X-101"}}))
        .expect("resolve");
    assert_eq!(done["resolved"], plan["id"], "{done}");
}

#[test]
fn a_rebuild_replaces_the_breakers_and_keeps_the_count_index_true() {
    let mut s = building();
    let first = s.execute("bim.count", &json!({"kind": "breaker"})).expect("count")["count"].clone();
    s.execute("bim.breakers", &json!({})).expect("again");
    s.execute("bim.breakers", &json!({})).expect("again");
    let n = s.execute("bim.count", &json!({"kind": "breaker", "verify": true})).expect("count");
    assert_eq!(n["count"], first, "{n}");
    assert_eq!(n["verify"]["matches"], true, "{n}");
    // 6 in LP1, 3 in each room panel, 2 in LP2, 2 in MDP.
    assert_eq!(first, json!(16));
    // Each breaker's body is inside its panel's box.
    let d = s.doc().expect("doc");
    for e in d.model.iter().filter(|e| cadaddy_engine::cmd::bimlook::made_tag(e, "part") == Some("atom:breaker")) {
        let ps = cadaddy_engine::doc::props::read(&e.common.xdata);
        let panel = ps.get("panel").map(|p| p.text()).unwrap_or_default();
        let p = d.model.iter().find(|x| cadaddy_engine::cmd::bimlook::made_tag(x, "tag") == Some(panel.as_str())).expect("its panel");
        let (b, pb) = (cadaddy_engine::doc::entity_bounds(d, e, 0), cadaddy_engine::doc::entity_bounds(d, p, 0));
        assert!(
            b.min.x >= pb.min.x - 1e-6 && b.max.x <= pb.max.x + 1e-6 && b.min.y >= pb.min.y - 1e-6 && b.max.y <= pb.max.y + 1e-6,
            "{panel}: {b:?} in {pb:?}"
        );
    }
}
