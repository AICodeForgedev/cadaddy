//! Q6520 underceiling: a wall-mounted device sits on the wall below its room's finished ceiling.
//! Its top is at least its own clearance under the ceiling (a legend's `x" BELOW CEILING`, else
//! the rule's 2 in). A wall-switch sensor goes in a switch box at switch height. A device whose
//! height would put any part of it above the ceiling is set at the highest allowed place, and
//! its design height is kept beside the corrected one. Made-up plans only.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle};
use serde_json::{Value, json};

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).unwrap();
}

/// A read device: its kind, its mark, where (feet) and other fields.
fn tag(n: usize, kind: &str, mark: &str, at: [f64; 2], fields: &[(&str, &str)]) -> Value {
    let mut f = json!({
        "type": {"value": mark, "source": "plan"},
        "sheet": {"value": "X-1", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x900 + n), "source": "plan"},
    });
    for (k, v) in fields {
        f[*k] = json!({"value": v, "source": "plan"});
    }
    json!({"id": format!("device:L1:{:.1},{:.1}:{mark}-{n}", at[0], at[1]), "kind": kind, "level": "L1", "at_ft": at, "fields": f})
}

/// Two 19' x 19' rooms inside 6" walls (inches): OFFICE 101 with a 7'-6" ceiling, STORAGE 102
/// with a 10'-0" one, under a 9' level ceiling. A legend that says what the marks are.
fn plan() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    rect(&mut s, [0.0, 0.0], [480.0, 240.0]);
    rect(&mut s, [6.0, 6.0], [234.0, 234.0]);
    rect(&mut s, [246.0, 6.0], [474.0, 234.0]);
    s.execute("layer.new", &json!({"name": "A-ANNO", "current": true})).unwrap();
    text(&mut s, [100.0, 120.0], "OFFICE");
    text(&mut s, [100.0, 100.0], "101");
    text(&mut s, [340.0, 120.0], "STORAGE");
    text(&mut s, [340.0, 100.0], "102");
    // The legend, well clear of the rooms.
    text(&mut s, [1200.0, 600.0], "ELECTRICAL LEGEND");
    for (i, (mark, words)) in [
        ("OS", "WALL SWITCH OCCUPANCY SENSOR, SWITCH BOX TYPE"),
        ("OS2", "OCCUPANCY SENSOR, WALL MOUNTED"),
        ("EM", "EMERGENCY LIGHT, WALL MOUNTED 4\" BELOW CEILING"),
    ]
    .iter()
    .enumerate()
    {
        let y = 580.0 - 14.0 * i as f64;
        text(&mut s, [1200.0, y], mark);
        text(&mut s, [1240.0, y], words);
    }
    s
}

/// The plan built: in OFFICE 101 (7'-6") a wall-switch sensor by the door, a wall sensor and a
/// wall emergency light; in STORAGE 102 (10') the same wall sensor; a receptacle in each.
fn built() -> Session {
    let mut s = plan();
    let tags = vec![
        tag(1, "occupancy sensor", "OS", [0.8, 10.0], &[]),
        tag(2, "occupancy sensor", "OS2", [0.8, 14.0], &[]),
        tag(3, "emergency light", "EM", [10.0, 19.2], &[("mount", "wall")]),
        tag(4, "occupancy sensor", "OS2", [20.8, 14.0], &[]),
        tag(5, "receptacle", "", [10.0, 0.8], &[]),
        tag(6, "receptacle", "", [30.0, 0.8], &[]),
    ];
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "Partition"}],
        "wall_types": [{"name": "Partition", "thickness_in": 6}],
        "rooms": [
            {"name": "OFFICE", "number": "101", "level": "L1", "ceiling_ft": 7.5, "ceiling_source": "a made-up RCP tag"},
            {"name": "STORAGE", "number": "102", "level": "L1", "ceiling_ft": 10.0, "ceiling_source": "a made-up RCP tag"},
        ],
        "tags": tags,
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({})).expect("build");
    s
}

/// Each built device by its spec id's number: (handle, top in inches over the floor, props).
fn devices(s: &mut Session) -> Vec<(usize, Handle, f64, Value)> {
    let r = s.execute("bim.props.find", &json!({"kind": "device", "limit": 1000})).unwrap();
    let mut hs: Vec<String> = r["handles"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_string()).collect();
    let r = s.execute("bim.props.find", &json!({"kind": "light_fixture", "limit": 1000})).unwrap();
    hs.extend(r["handles"].as_array().unwrap().iter().map(|h| h.as_str().unwrap().to_string()));
    let mut out = Vec::new();
    for h in hs {
        let handle = Handle::parse_hex(&h).unwrap();
        let props = s.execute("bim.props.get", &json!({"handle": h})).unwrap()["elements"][0]["props"].clone();
        let d = s.doc().unwrap();
        let e = d.entity(handle).unwrap();
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let top = m.vertices.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max);
        let n = (1..=6).find(|n| e.common.xdata.iter().any(|t| t.value.ends_with(&format!("-{n}")) && t.value.contains("device:L1"))).unwrap_or(0);
        out.push((n, handle, top, props));
    }
    out.sort_by_key(|x| x.0);
    out
}

fn device(all: &[(usize, Handle, f64, Value)], n: usize) -> (Handle, f64, Value) {
    let (_, h, top, p) = all.iter().find(|x| x.0 == n).unwrap_or_else(|| panic!("no device {n}: {:?}", all.iter().map(|x| x.0).collect::<Vec<_>>()));
    (*h, *top, p.clone())
}

#[test]
fn wall_devices_stay_below_their_rooms_ceiling() {
    let mut s = built();
    let all = devices(&mut s);
    assert_eq!(all.len(), 6, "{:?}", all.iter().map(|x| (x.0, x.2)).collect::<Vec<_>>());
    // In OFFICE 101 (ceiling 90"): nothing's top above 90" less its clearance.
    let (_, top, p) = device(&all, 1);
    assert!(top <= 88.0 + 1e-6, "the wall-switch sensor's top at {top} in, the ceiling at 90 in: {p:#}");
    assert!(
        (p["mount_height"]["value"].as_f64().unwrap() - 48.0).abs() < 1e-6,
        "a wall-switch sensor goes in its switch box at switch height: {p:#}"
    );
    let (_, top, p) = device(&all, 2);
    assert!((top - 88.0).abs() < 1e-6, "the wall sensor's top set 2 in under the 7'-6\" ceiling, not {top}: {p:#}");
    assert_eq!(p["design:mount_height"]["value"].as_f64(), Some(108.0), "its design height kept: {p:#}");
    assert!(p["correction:mount_height"]["value"].as_str().is_some_and(|c| c.starts_with("E-WALL-UNDER-CEILING")), "{p:#}");
    assert!(p["mount_height"]["value"].as_f64().is_some_and(|v| v < 108.0), "{p:#}");
    let (_, top, p) = device(&all, 3);
    assert!((top - 86.0).abs() < 1e-6, "the emergency light's top 4 in under the ceiling as its legend says, not {top}: {p:#}");
    assert!(p["design:mount_height"].is_object() && p["correction:mount_height"].is_object(), "{p:#}");
    // In STORAGE 102 (ceiling 120"): the same sensor where its height puts it, nothing corrected.
    let (_, top, p) = device(&all, 4);
    assert!((top - 110.0).abs() < 1e-6, "the sensor under a 10' ceiling keeps its height: {top} {p:#}");
    assert!(p.get("design:mount_height").is_none(), "{p:#}");
    for n in [5, 6] {
        let (_, top, p) = device(&all, n);
        assert!(top < 30.0 && p.get("design:mount_height").is_none(), "a receptacle stays low: {top} {p:#}");
    }
}

/// The prescreen's count of wall devices over their place under the ceiling.
fn screened(s: &mut Session) -> (u64, u64, Vec<Value>) {
    let r = s.execute("validate.prescreen", &json!({"rules": ["G-CEILING-CLEAR"], "all": true, "limit": 50})).unwrap();
    let c = &r["checked"]["G-CEILING-CLEAR"];
    let fails = r["findings"].as_array().unwrap().iter().filter(|f| f["rule"] == "G-CEILING-CLEAR" && f["verdict"] == "fail").cloned().collect();
    (c["pass"].as_u64().unwrap_or(0), c["fail"].as_u64().unwrap_or(0), fails)
}

#[test]
fn validate_counts_no_wall_device_above_its_ceiling_and_finds_one_put_back() {
    let mut s = built();
    let (pass, fail, fails) = screened(&mut s);
    assert_eq!(fail, 0, "after the build no wall device is over its place: {fails:#?}");
    assert!(pass >= 4, "the sensors and the emergency light are measured: {pass}");
    // The wall sensor put back at its design height (its top at 110 in under the 90 in ceiling).
    let all = devices(&mut s);
    let (h, top, _) = device(&all, 2);
    let up = 110.0 - top;
    s.doc_mut()
        .unwrap()
        .modify_entity(h, |e| {
            if let EntityKind::Mesh(m) = &mut e.kind {
                for v in &mut m.vertices {
                    v.z += up;
                }
            }
        })
        .unwrap();
    let (_, fail, fails) = screened(&mut s);
    assert_eq!(fail, 1, "{fails:#?}");
    let f = &fails[0];
    assert_eq!(f["handle"], h.hex());
    assert_eq!(f["measured"]["above_ceiling"], true, "{f:#}");
    assert!((f["measured"]["ceiling_in"].as_f64().unwrap() - 90.0).abs() < 1e-6, "{f:#}");
    assert!((f["measured"]["top_in"].as_f64().unwrap() - 110.0).abs() < 1e-6, "{f:#}");
}
