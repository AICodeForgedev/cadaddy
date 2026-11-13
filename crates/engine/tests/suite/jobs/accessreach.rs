//! Q6555 accessible reach: in a room the documents mark accessible, a panel, thermostat or
//! switch is set within the accessible reach (a printed accessible maximum first, else the
//! code's 48 in to its top), its design height kept beside the corrected one. The same controls
//! in a standard room keep their heights; a list note that gives only an accessible height gives
//! it to every room's item. Made-up plans only.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle};
use serde_json::{Value, json};

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).unwrap();
}

/// A read device: its kind, its mark, where (feet), its room and other fields.
fn tag(n: usize, kind: &str, mark: &str, at: [f64; 2], room: &str) -> Value {
    let f = json!({
        "type": {"value": mark, "source": "plan"},
        "sheet": {"value": "X-1", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x900 + n), "source": "plan"},
        "room": {"value": room, "source": "plan"},
    });
    json!({"id": format!("device:L1:{:.1},{:.1}:{mark}-{n}", at[0], at[1]), "kind": kind, "level": "L1", "at_ft": at, "fields": f})
}

/// Two 19' x 19' rooms inside 6" walls (inches): SUITE ADA 101 and SUITE 102. In each a
/// thermostat and a switch at the usual height (48" to their middle, their tops at 50"), a
/// panel topped at 6'-6"; a receptacle in 101. The general notes list one accessible height:
/// thermostats and controls 46" at most to the top.
fn built() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    rect(&mut s, [0.0, 0.0], [480.0, 240.0]);
    rect(&mut s, [6.0, 6.0], [234.0, 234.0]);
    rect(&mut s, [246.0, 6.0], [474.0, 234.0]);
    s.execute("layer.new", &json!({"name": "A-ANNO", "current": true})).unwrap();
    text(&mut s, [100.0, 120.0], "SUITE ADA");
    text(&mut s, [100.0, 100.0], "101");
    text(&mut s, [340.0, 120.0], "SUITE");
    text(&mut s, [340.0, 100.0], "102");
    let tags = vec![
        tag(1, "thermostat", "T", [0.8, 10.0], "101"),
        tag(2, "thermostat", "T", [20.8, 10.0], "102"),
        tag(3, "switch", "S", [0.8, 14.0], "101"),
        tag(4, "switch", "S", [20.8, 14.0], "102"),
        tag(5, "receptacle", "", [10.0, 0.8], "101"),
    ];
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "rooms": [
            {"name": "SUITE ADA", "number": "101", "level": "L1"},
            {"name": "SUITE", "number": "102", "level": "L1"},
        ],
        "mount_notes": [{
            "sheet": "G-0", "subject": "THERMOSTATS AND CONTROLS", "ada_inches": 46, "datum": "top",
            "text": "THERMOSTATS AND CONTROLS (TO TOP): ADA FORWARD REACH = 46\" MAX."
        }],
        "mep": {"equipment": [
            {"tag": "LP-A", "kind": "panel", "level": "L1", "min": [6, 150], "max": [10, 170], "height_ft": 2, "bottom_ft": 4.5},
            {"tag": "LP-B", "kind": "panel", "level": "L1", "min": [246, 150], "max": [250, 170], "height_ft": 2, "bottom_ft": 4.5},
        ]},
        "tags": tags,
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": false})).expect("build");
    s
}

/// A built element's bottom and top (inches over the floor) and its properties.
fn element(s: &mut Session, h: Handle) -> (f64, f64, Value) {
    let props = s.execute("bim.props.get", &json!({"handle": h.hex()})).unwrap()["elements"][0]["props"].clone();
    let d = s.doc().unwrap();
    let EntityKind::Mesh(m) = &d.entity(h).unwrap().kind else { panic!("not a mesh") };
    let lo = m.vertices.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
    let hi = m.vertices.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max);
    (lo, hi, props)
}

/// The built element whose build tags hold `key` (its spec id's end, its equipment tag).
fn find(s: &mut Session, kind: &str, key: &str) -> Handle {
    let r = s.execute("bim.props.find", &json!({"kind": kind, "limit": 1000})).unwrap();
    let d = s.doc().unwrap();
    r["handles"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|h| Handle::parse_hex(h.as_str().unwrap()))
        .find(|h| d.entity(*h).is_some_and(|e| e.common.xdata.iter().any(|t| t.value.ends_with(key))))
        .unwrap_or_else(|| panic!("no {kind} {key}"))
}

#[test]
fn controls_in_an_accessible_room_are_within_reach() {
    let mut s = built();
    // A thermostat in the standard room takes the notes' only height: 46" to its top.
    let h = find(&mut s, "device", "T-2");
    let (_, top, p) = element(&mut s, h);
    assert!((top - 46.0).abs() < 1e-6, "the list's accessible height is every thermostat's when it gives no other: top {top} {p:#}");
    assert!(p.get("design:mount_height").is_none(), "documented, not corrected: {p:#}");
    // The accessible room's thermostat: within the printed maximum already.
    let h = find(&mut s, "device", "T-1");
    let (_, top, p) = element(&mut s, h);
    assert!((top - 46.0).abs() < 1e-6, "top {top} {p:#}");
    assert!(p.get("design:mount_height").is_none(), "{p:#}");
    // The accessible room's switch at the usual height: its top comes down to the code's reach.
    let h = find(&mut s, "device", "S-3");
    let (_, top, p) = element(&mut s, h);
    assert!((top - 48.0).abs() < 1e-6, "the switch's top at the 48\" reach, not {top}: {p:#}");
    assert_eq!(p["design:mount_height"]["value"].as_f64(), Some(48.0), "its design height kept: {p:#}");
    assert_eq!(p["mount_height"]["value"].as_f64(), Some(46.0), "{p:#}");
    let why = p["correction:mount_height"]["value"].as_str().unwrap_or_default();
    assert!(why.starts_with("A-REACH") && why.contains("accessible room (101)"), "{p:#}");
    // The standard room's switch keeps the usual height.
    let h = find(&mut s, "device", "S-4");
    let (_, top, p) = element(&mut s, h);
    assert!((top - 50.0).abs() < 1e-6 && p.get("design:mount_height").is_none(), "top {top} {p:#}");
    // A receptacle is no control: it stays low.
    let h = find(&mut s, "device", "-5");
    let (_, top, p) = element(&mut s, h);
    assert!(top < 30.0 && p.get("design:mount_height").is_none(), "{top} {p:#}");
    // The accessible room's panel: topped at the reach, its design height and record beside.
    let h = find(&mut s, "equipment", "tag:LP-A");
    let (lo, top, p) = element(&mut s, h);
    assert!((top - 48.0).abs() < 1e-6 && (lo - 24.0).abs() < 1e-6, "the panel from 24\" to 48\", not {lo}-{top}: {p:#}");
    assert_eq!(p["design:mount_height"]["value"].as_f64(), Some(78.0), "its top as built kept: {p:#}");
    assert!(p["correction:mount_height"]["value"].as_str().unwrap_or_default().starts_with("A-REACH"), "{p:#}");
    let spec = s.execute("bspec.get", &json!({})).unwrap();
    let rec = |t: &str| spec["spec"]["mep"]["equipment"].as_array().unwrap().iter().find(|e| e["tag"] == t).cloned().unwrap();
    assert_eq!(rec("LP-A")["bottom_ft"].as_f64(), Some(2.0), "its spec record follows: {:#}", rec("LP-A"));
    assert!(rec("LP-A")["sources"]["bottom"].as_str().unwrap_or_default().starts_with("A-REACH"), "{:#}", rec("LP-A"));
    // The standard room's panel stays at 6'-6".
    let h = find(&mut s, "equipment", "tag:LP-B");
    let (_, top, p) = element(&mut s, h);
    assert!((top - 78.0).abs() < 1e-6 && p.get("design:mount_height").is_none(), "{top} {p:#}");
    assert_eq!(rec("LP-B")["bottom_ft"].as_f64(), Some(4.5));
}
