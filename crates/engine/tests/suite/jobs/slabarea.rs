//! Floor slabs are the building's outline, not the sheet's or the walls' extents (Q6023): an L
//! plan's slab is its L, inside the exterior walls' outside faces, and the takeoff's slab square
//! feet match it (the slab's top counted once, not its top and underside).

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{EntityKind, Mesh};
use serde_json::{Value, json};

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn ring(s: &mut Session, pts: &[[f64; 2]]) {
    for k in 0..pts.len() {
        line(s, pts[k], pts[(k + 1) % pts.len()]);
    }
}

/// An L (inches): 100' × 40' with a 40' × 60' leg, 6" outside walls (both faces), a 4"
/// partition across the long arm with a 3' door, and the sheet's double-ruled border round it.
fn l_plan() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    ring(&mut s, &[[0.0, 0.0], [1200.0, 0.0], [1200.0, 480.0], [480.0, 480.0], [480.0, 1200.0], [0.0, 1200.0]]);
    ring(&mut s, &[[6.0, 6.0], [1194.0, 6.0], [1194.0, 474.0], [474.0, 474.0], [474.0, 1194.0], [6.0, 1194.0]]);
    for x in [838.0, 842.0] {
        line(&mut s, [x, 6.0], [x, 200.0]);
        line(&mut s, [x, 236.0], [x, 474.0]);
    }
    s.execute("layer.new", &json!({"name": "A-DOOR", "current": true})).unwrap();
    s.execute("arc", &json!({"center": [840.0, 200.0], "radius": 36.0, "start": 0.0, "end": 90.0})).unwrap();
    for m in [0.0, 6.0] {
        ring(&mut s, &[[-300.0 + m, -300.0 + m], [1700.0 - m, -300.0 + m], [1700.0 - m, 1500.0 - m], [-300.0 + m, 1500.0 - m]]);
    }
    s
}

fn floors(s: &Session) -> Vec<Mesh> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("floor"))
        .filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None })
        .collect()
}

/// Plan area of a mesh's faces lying flat at `z`, square feet.
fn flat_sf(m: &Mesh, z: f64) -> f64 {
    m.faces
        .iter()
        .filter(|f| f.v.iter().all(|i| m.vertices.get(*i as usize).is_some_and(|v| (v.z - z).abs() < 1e-3)))
        .map(|f| {
            let pts: Vec<_> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize)).collect();
            let mut a = 0.0;
            for k in 0..pts.len() {
                let (p, q) = (pts[k], pts[(k + 1) % pts.len()]);
                a += p.x * q.y - q.x * p.y;
            }
            a.abs() / 2.0
        })
        .sum::<f64>()
        / 144.0
}

const L_SF: f64 = 100.0 * 40.0 + 40.0 * 60.0;

#[test]
fn an_l_plans_slab_is_its_l_and_the_takeoff_matches() {
    let mut s = l_plan();
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert!(r["walls"].as_u64().unwrap_or(0) >= 6, "{r}");
    let fl = floors(&s);
    assert_eq!(fl.len(), 1, "{r}");
    let m = &fl[0];
    let top = flat_sf(m, 0.0);
    assert!((top - L_SF).abs() < 0.01 * L_SF, "slab top {top} sf, the L is {L_SF} sf (its extents 10000, the sheet's far more)");
    // Every slab vertex lies on the building, inside the walls' outside faces.
    for v in &m.vertices {
        assert!(v.x >= -1e-6 && v.x <= 1200.0 + 1e-6 && v.y >= -1e-6 && v.y <= 1200.0 + 1e-6, "{v:?}");
        assert!(!(v.x > 480.0 + 6.0 && v.y > 480.0 + 6.0), "slab over the L's notch at {v:?}");
    }
    // The slab is 6" thick: its underside has the same plate.
    let under = flat_sf(m, -6.0);
    assert!((under - top).abs() < 1e-6, "{under} {top}");

    // The takeoff: the slab's square feet once.
    let bom = s.execute("bim.bom", &json!({"group_by": "level"})).unwrap();
    let slab: f64 = bom["lines"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|l| l["item"] == "Floor slab")
        .filter_map(|l| l["net"].as_f64().or_else(|| l["quantity"].as_f64()))
        .sum();
    assert!((slab - L_SF).abs() < 0.01 * L_SF, "takeoff slab {slab} sf, want {L_SF}: {}", lines(&bom));
}

fn lines(bom: &Value) -> String {
    bom["lines"].as_array().into_iter().flatten().filter(|l| l["category"] == "floors").map(Value::to_string).collect::<Vec<_>>().join("\n")
}

#[test]
fn walls_that_do_not_close_keep_their_extents_and_say_so() {
    // Two parallel walls 30' apart: no building to find inside them.
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    for y in [0.0, 6.0, 360.0, 366.0] {
        line(&mut s, [0.0, y], [600.0, y]);
    }
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    for m in floors(&s) {
        let top = flat_sf(&m, 0.0);
        assert!((top - 50.0 * 30.5).abs() < 1.0, "{top}: {r}");
    }
    let notes = r["notes"].to_string();
    assert!(floors(&s).is_empty() || notes.contains("do not close"), "{r}");
}
