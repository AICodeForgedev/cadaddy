//! Q6242 fix-raceway-geometry-unbuildable: a derived raceway runs plumb in the wall from its box
//! and square at its route's height above the ceiling: no stick cuts a chord through the room at
//! a box's height, crosses diagonally, or runs exposed under the finished ceiling.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, EntityKind};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn device(n: usize, x: f64, y: f64, circuit: &str, mount_in: f64) -> Value {
    json!({"id": format!("device:L1:{n}"), "kind": "receptacle", "level": "L1", "at_ft": [x, y], "fields": {
        "type": {"value": "A", "source": "plan"},
        "symbol": {"value": format!("{:X}", 0x2A0 + n), "source": "plan"},
        "sheet": {"value": "E-101", "source": "plan"},
        "circuit": {"value": circuit, "source": "plan"},
        "mount_height_in": {"value": format!("{mount_in}"), "source": "note"}
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

/// A made-up one-storey building (12' floor to floor, 9' ceiling), panel LP1 at the west wall:
/// circuits LP1-1 and LP1-3 each one receptacle, 2'-3" apart (one homerun for both: LP1-3 taps
/// into LP1-1's box); a receptacle of another panel's circuit (not routed here) 2' from LP1's
/// side, at 54".
fn building() -> Session {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let tags = vec![
        device(1, 20.0, 10.0, "LP1-1", 18.0),
        device(2, 22.0, 11.0, "LP1-3", 18.0),
        device(3, 0.5, 2.0, "LP9-1", 54.0),
        circuit("LP1-1"),
        circuit("LP1-3"),
    ];
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}],
        "tags": tags,
        "mep": {
            "systems": [],
            "runs": [],
            "equipment": [
                {"tag": "LP1", "kind": "panel", "level": "L1", "min": [-24, -6], "max": [-4, 6], "height_ft": 4, "bottom_ft": 2.5,
                 "enclosure": {"top_entry_in": [10.0, 4.0], "bottom_entry_in": [10.0, 4.0], "gutter_in": [0.0, 0.0]}}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({})).expect("build");
    assert!(built["atoms"].as_u64().unwrap_or(0) > 0, "{built}");
    s
}

/// Each conduit stick's body as built: its circuit and the centreline of each tube of its mesh
/// (inches).
fn tubes(s: &Session) -> Vec<(String, Vec3, Vec3)> {
    let d = s.doc().expect("doc");
    let mut out = Vec::new();
    for e in d.model.iter() {
        let ps = cadaddy_engine::doc::props::read(&e.common.xdata);
        if ps.get("atom").map(|x| x.text()).as_deref() != Some("conduit") {
            continue;
        }
        let circuit = ps.get("circuit").map(|x| x.text()).unwrap_or_default();
        let EntityKind::Mesh(m) = &e.kind else { continue };
        let centre = |vs: &[cadaddy_engine::geom::Vec3]| vs.iter().fold(Vec3::new(0.0, 0.0, 0.0), |a, v| a + *v) * (1.0 / vs.len().max(1) as f64);
        for tube in m.vertices.chunks(16).filter(|t| t.len() == 16) {
            let (r0, r1) = tube.split_at(8);
            out.push((circuit.clone(), centre(r0), centre(r1)));
        }
    }
    out
}

/// A tube that slopes, runs diagonally in plan, or runs level under the 9' ceiling (none of
/// this building's boxes is in a wall: every level piece is in the ceiling space).
fn unbuildable(t: &[(String, Vec3, Vec3)]) -> Vec<String> {
    let tol = 0.25;
    t.iter()
        .filter_map(|(c, a, b)| {
            let (dx, dy, dz) = (b.x - a.x, b.y - a.y, b.z - a.z);
            let plan = dx.hypot(dy);
            let why = if plan <= tol {
                return None;
            } else if dz.abs() > tol {
                "sloped"
            } else if dx.abs() > tol && dy.abs() > tol {
                "diagonal"
            } else if a.z < 9.0 * 12.0 {
                "level under the ceiling"
            } else {
                return None;
            };
            Some(format!("{c}: {why} {:.1},{:.1},{:.1} → {:.1},{:.1},{:.1}", a.x, a.y, a.z, b.x, b.y, b.z))
        })
        .collect()
}

#[test]
fn a_tap_into_the_lead_box_drops_plumb_into_it_and_never_cuts_back_to_its_own_box() {
    let s = building();
    let t = tubes(&s);
    assert!(t.iter().any(|(c, _, _)| c == "LP1-3"), "LP1-3's tap is built: {t:?}");
    let bad = unbuildable(&t);
    assert!(bad.is_empty(), "{bad:#?}");
    // LP1-3's tap ends in the lead's box (LP1-1's, at 20', 10'): its last tube drops plumb onto
    // that box's top, not across to its own box 2' away at the boxes' height.
    let tap: Vec<&(String, Vec3, Vec3)> = t.iter().filter(|(c, _, _)| c == "LP1-3").collect();
    let near = |p: &Vec3, x: f64, y: f64| (p.x - x * 12.0).abs() < 6.0 && (p.y - y * 12.0).abs() < 6.0;
    assert!(
        tap.iter().any(|(_, a, b)| (a.x - b.x).abs() < 0.25 && (a.y - b.y).abs() < 0.25 && near(a, 20.0, 10.0) && a.z.min(b.z) < 30.0),
        "a plumb drop into LP1-1's box: {tap:?}"
    );
}

#[test]
fn a_homerun_lands_on_its_panel_not_in_another_circuits_box_beside_it() {
    let s = building();
    let t = tubes(&s);
    // Nothing reaches the other panel's receptacle (at 0.5', 2' and 54"): no tube ends near it.
    let near_other: Vec<&(String, Vec3, Vec3)> =
        t.iter().filter(|(_, a, b)| [a, b].iter().any(|p| (p.x - 6.0).abs() < 4.0 && (p.y - 24.0).abs() < 4.0)).collect();
    assert!(near_other.is_empty(), "{near_other:?}");
    let bad = unbuildable(&t);
    assert!(bad.is_empty(), "{bad:#?}");
}
