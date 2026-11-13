//! Q6624 (test finding fix-equipment-circuit-not-routed): equipment that carries its circuit (a
//! rooftop condensing unit whose plan tag names its circuit) was never routed when no
//! connection device stood at it: the route list said "no device of this circuit is in the
//! model" although the unit had its circuit, and its drawn disconnect was not built either. A
//! piece of equipment with a circuit is a load: its disconnect (drawn, else built beside it by
//! rule `E-EQUIP-DISCONNECT`, NEC 440.14 / 430.102) is its connection, the circuit is routed to
//! it, then on to the unit's terminal box.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bimlook::made_tag;
use cadaddy_engine::doc::Drawing;
use cadaddy_engine::doc::props;
use serde_json::{Value, json};

/// The made-up unit's circuit on panel QP, and the unit.
const CIRCUIT: &str = "QP-12,14";
const UNIT: &str = "CU-7";
/// The unit's plan box on the roof (sheet units, 12 a foot): 3 ft square.
const UNIT_MIN: [f64; 2] = [480.0, 240.0];
const UNIT_MAX: [f64; 2] = [516.0, 276.0];

/// How the unit carries its circuit.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Via {
    /// Its build record (the plan tag's circuit read into the equipment's properties).
    Record,
    /// Its element's properties, set after the build (a fill or an edit).
    Props,
}

/// What stands at the unit besides it.
#[derive(Clone, Copy, PartialEq, Debug)]
enum AtUnit {
    Nothing,
    /// A disconnect drawn beside it with no circuit of its own.
    DrawnDisconnect,
    /// An equipment connection of its circuit, drawn at it.
    Connection,
}

/// A two-storey made-up building: panel QP on L1, a condensing unit on the roof carrying
/// circuit QP-12,14 (its circuit tag on the roof plan, the panel's schedule listing it), and a
/// receptacle on L1 on QP-1. Built and routed; `via` Props sets the unit's circuit after the
/// build and routes again.
fn building(via: Via, at_unit: AtUnit) -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let mut unit = json!({"tag": UNIT, "kind": "condensing_unit", "level": "Roof", "min": UNIT_MIN, "max": UNIT_MAX, "height_ft": 3, "bottom_ft": 1, "title": "CONDENSING UNIT SCHEDULE"});
    if via == Via::Record {
        unit["properties"] = json!({"CIRCUIT": CIRCUIT, "PANEL": "QP"});
    }
    let row = |n: &str, d: &str, a: &str, p: u32| json!({"number": n, "description": d, "breaker": a, "poles": p, "protection": [], "source": "X-2"});
    let mut tags = vec![
        json!({"id": "device:L1:5.0,30.0", "kind": "receptacle", "level": "L1", "at_ft": [5.0, 30.0], "fields": {
            "symbol": {"value": "R-sym", "source": "plan"}, "sheet": {"value": "X-101", "source": "plan"}, "circuit": {"value": "QP-1", "source": "plan"}}}),
        json!({"id": "circuit:L1:X-101:QP-1", "kind": "circuit", "level": "L1", "at_ft": [4.0, 30.0], "fields": {
            "circuit": {"value": "QP-1", "source": "plan"}, "panel": {"value": "QP", "source": "plan"}}}),
        json!({"id": format!("circuit:Roof:X-301:{CIRCUIT}"), "kind": "circuit", "level": "Roof", "at_ft": [44.0, 21.0], "fields": {
            "circuit": {"value": CIRCUIT, "source": "plan"}, "panel": {"value": "QP", "source": "plan"}}}),
    ];
    match at_unit {
        AtUnit::Nothing => {}
        AtUnit::DrawnDisconnect => tags.push(json!({"id": "device:Roof:44.5,21.5", "kind": "disconnect", "level": "Roof", "at_ft": [44.5, 21.5], "fields": {
            "type": {"value": "DS", "source": "plan"}, "symbol": {"value": "DS-sym", "source": "plan"}, "sheet": {"value": "X-301", "source": "plan"}}})),
        AtUnit::Connection => tags.push(json!({"id": "device:Roof:41.5,21.5", "kind": "equipment_connection", "level": "Roof", "at_ft": [41.5, 21.5], "fields": {
            "type": {"value": UNIT, "source": "plan"}, "symbol": {"value": "EC-sym", "source": "plan"}, "sheet": {"value": "X-301", "source": "plan"},
            "circuit": {"value": CIRCUIT, "source": "plan"}}})),
    }
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]},
            {"name": "Roof", "elevation_ft": 12, "height_ft": 10, "ceiling_ft": 8, "base": [0, 0]}
        ],
        "tags": tags,
        "mep": {
            "equipment": [
                {"tag": "QP", "kind": "panel", "level": "L1", "min": [-24, 110], "max": [-4, 130], "height_ft": 4, "bottom_ft": 2},
                unit
            ],
            "panel_schedules": [{"panel": "QP", "source": "X-2", "circuits": [row("1", "RCPT", "20", 1), row("12", "CU-7", "30", 2)]}]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": true})).expect("build");
    if via == Via::Props {
        let h = unit_handle(&s);
        s.execute("bim.props.set", &json!({"handle": h, "key": "circuit", "value": CIRCUIT})).expect("circuit");
        s.execute("bim.props.set", &json!({"handle": h, "key": "panel", "value": "QP"})).expect("panel");
    }
    s.execute("conduit.reroute", &json!({})).expect("reroute");
    s
}

fn unit_handle(s: &Session) -> String {
    let d = s.doc().expect("doc");
    d.model.iter().find(|e| made_tag(e, "tag") == Some(UNIT)).map(|e| e.handle.hex()).expect("the unit is built")
}

/// The model's devices on the unit's circuit: (handle, kind, placed by the rule, status tag,
/// basis tag, serves, plan centre).
fn circuit_devices(s: &Session) -> Vec<(String, String, bool, String, String, String, [f64; 2])> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("device"))
        .filter_map(|e| {
            let ps = props::read(&e.common.xdata);
            let text = |k: &str| ps.get(k).map(|p| p.text()).unwrap_or_default();
            (text("circuit") == CIRCUIT).then(|| {
                let cadaddy_engine::doc::EntityKind::Mesh(m) = &e.kind else { return None };
                let xs = || m.vertices.iter().map(|v| v.x);
                let ys = || m.vertices.iter().map(|v| v.y);
                let (x0, x1) = (xs().fold(f64::INFINITY, f64::min), xs().fold(f64::NEG_INFINITY, f64::max));
                let (y0, y1) = (ys().fold(f64::INFINITY, f64::min), ys().fold(f64::NEG_INFINITY, f64::max));
                Some((
                    e.handle.hex(),
                    text("kind"),
                    text("placed_by") == "E-EQUIP-DISCONNECT",
                    made_tag(e, "status").unwrap_or_default().to_string(),
                    made_tag(e, "basis").unwrap_or_default().to_string(),
                    text("serves"),
                    [(x0 + x1) / 2.0, (y0 + y1) / 2.0],
                ))
            })?
        })
        .collect()
}

fn unrouted_circuits(s: &mut Session) -> Vec<String> {
    let u = s.execute("conduit.route", &json!({"section": "unrouted"})).expect("unrouted");
    u["unrouted"].as_array().into_iter().flatten().filter_map(|x| x["circuit"].as_str().map(str::to_string)).collect()
}

fn conduit_on_circuit(s: &mut Session) -> u64 {
    let found = s.execute("bim.props.find", &json!({"kind": "conduit", "where": {"circuit": CIRCUIT}, "limit": 500})).expect("find");
    found["count"].as_u64().unwrap_or(0)
}

/// How far a plan point is from the unit's box (0 inside it), sheet units.
fn gap_to_unit(p: [f64; 2]) -> f64 {
    let dx = (UNIT_MIN[0] - p[0]).max(p[0] - UNIT_MAX[0]).max(0.0);
    let dy = (UNIT_MIN[1] - p[1]).max(p[1] - UNIT_MAX[1]).max(0.0);
    (dx * dx + dy * dy).sqrt()
}

#[test]
fn equipment_carrying_its_circuit_gets_a_disconnect_and_is_routed() {
    for via in [Via::Record, Via::Props] {
        let mut s = building(via, AtUnit::Nothing);
        let unrouted = unrouted_circuits(&mut s);
        assert!(!unrouted.iter().any(|c| c == CIRCUIT), "{via:?}: the unit's circuit is routed, not listed: {unrouted:?}");
        let devs = circuit_devices(&s);
        assert_eq!(devs.len(), 1, "{via:?}: one connection on the unit's circuit: {devs:?}");
        let (_, kind, placed, status, basis, serves, at) = &devs[0];
        assert_eq!((kind.as_str(), *placed, serves.as_str()), ("disconnect", true, UNIT), "{via:?}: {devs:?}");
        // Never passed off as drawn: inferred, the rule's basis on it.
        assert_eq!(status, "inferred", "{via:?}: {devs:?}");
        assert!(basis.contains("E-EQUIP-DISCONNECT") && basis.contains("440.14"), "{via:?}: {basis}");
        // Beside the housing, within sight (a couple of feet off it), not inside it.
        let gap = gap_to_unit(*at);
        assert!(gap > 0.0 && gap <= 3.0 * 12.0, "{via:?}: the disconnect stands beside the unit: {gap} in off it at {at:?}");
        assert!(conduit_on_circuit(&mut s) > 0, "{via:?}: the unit's circuit has raceway");
        // The circuit goes on from the disconnect to the unit's terminal box.
        let report = s.execute("conduit.route", &json!({})).expect("report");
        let whips = report["equipment_whips"]["items"].as_array().cloned().unwrap_or_default();
        assert!(whips.iter().any(|w| w["equipment"] == json!(UNIT) && w["circuit"] == json!(CIRCUIT)), "{via:?}: {:#}", report["equipment_whips"]);
        // Routed again: the same one disconnect (the earlier pass's goes first).
        s.execute("conduit.reroute", &json!({})).expect("reroute");
        let again = circuit_devices(&s);
        assert_eq!(again.len(), 1, "{via:?}: still one connection after routing again: {again:?}");
        assert!(!unrouted_circuits(&mut s).iter().any(|c| c == CIRCUIT), "{via:?}");
    }
}

#[test]
fn a_drawn_disconnect_beside_the_equipment_takes_its_circuit() {
    let mut s = building(Via::Record, AtUnit::DrawnDisconnect);
    let devs = circuit_devices(&s);
    assert_eq!(devs.len(), 1, "the drawn disconnect is the unit's connection, no other built: {devs:?}");
    let (_, kind, placed, status, _, serves, _) = &devs[0];
    assert_eq!((kind.as_str(), *placed, serves.as_str()), ("disconnect", false, UNIT), "{devs:?}");
    assert_ne!(status, "inferred", "the drawn disconnect stays drawn: {devs:?}");
    assert!(!unrouted_circuits(&mut s).iter().any(|c| c == CIRCUIT));
    assert!(conduit_on_circuit(&mut s) > 0);
}

#[test]
fn equipment_with_a_connection_of_its_circuit_at_it_gets_nothing_more() {
    let mut s = building(Via::Record, AtUnit::Connection);
    let devs = circuit_devices(&s);
    assert_eq!(devs.len(), 1, "the drawn connection only: {devs:?}");
    assert!(!devs[0].2, "no disconnect built where a connection of its circuit stands: {devs:?}");
    assert!(!unrouted_circuits(&mut s).iter().any(|c| c == CIRCUIT));
    let r: Value = s.execute("conduit.reroute", &json!({})).expect("reroute");
    assert_eq!(r["equipment_connections"]["connected"], json!(1), "{:#}", r["equipment_connections"]);
}
