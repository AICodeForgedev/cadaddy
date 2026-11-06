//! The 3D furniture and fixture library: every model has a 3D body with extents, standing at its
//! mounting height, and a 2D plan view; placing one defines its block at the drawing's units, and
//! placed 2D symbols get their 3D twins at mounting height, once.

use cadaddy_engine::Session;
use cadaddy_engine::doc::library::models3d;
use cadaddy_engine::doc::{EntityKind, Handle};
use serde_json::{Value, json};

/// Z range and plan extents of a list of 3DFACEs.
fn body_extents(es: &[cadaddy_engine::doc::Entity]) -> (f64, f64, f64, f64) {
    let (mut z0, mut z1, mut w, mut d) = (f64::INFINITY, f64::NEG_INFINITY, (f64::INFINITY, f64::NEG_INFINITY), (f64::INFINITY, f64::NEG_INFINITY));
    for e in es {
        let EntityKind::Face3d(f) = &e.kind else { panic!("the body is 3DFACEs: {:?}", e.kind) };
        for c in f.corners {
            z0 = z0.min(c.z);
            z1 = z1.max(c.z);
            w = (w.0.min(c.x), w.1.max(c.x));
            d = (d.0.min(c.y), d.1.max(c.y));
        }
    }
    (z0, z1, w.1 - w.0, d.1 - d.0)
}

#[test]
fn every_model_has_a_body_with_extents_a_mounting_height_and_a_plan_view() {
    let all = models3d::models();
    assert!(all.len() >= 30, "{}", all.len());
    for cat in ["Furniture", "Plumbing fixtures", "Doors and windows", "Lighting", "Power and devices"] {
        assert!(all.iter().any(|m| m.category == cat), "{cat}");
    }
    for m in all {
        let body = m.body();
        assert!(!body.is_empty(), "{}: no body", m.name);
        let (z0, z1, w, d) = body_extents(&body);
        assert!(m.mount_in >= 0.0 && m.mount_in < 120.0, "{}: mounting height {}", m.name, m.mount_in);
        // The body stands on its mounting height.
        assert!((z0 - m.mount_in).abs() < 1e-9, "{}: bottom {z0} vs mount {}", m.name, m.mount_in);
        assert!(z1 > z0 && w > 0.0 && d > 0.0, "{}: flat body", m.name);
        assert!(w < 120.0 && d < 120.0 && z1 - z0 < 100.0, "{}: implausible size {w}×{d}×{}", m.name, z1 - z0);
        let plan = m.plan();
        assert!(!plan.is_empty(), "{}: no plan view", m.name);
        for e in &plan {
            let b = cadaddy_engine::doc::entity_bounds(&cadaddy_engine::doc::Drawing::default(), e, 0);
            assert!(!b.is_empty(), "{}: empty plan object", m.name);
            assert!(!matches!(e.kind, EntityKind::Face3d(_)), "{}: plan view is 2D", m.name);
        }
    }
}

fn h(v: &Value) -> Handle {
    Handle(u64::from_str_radix(v["handle"].as_str().unwrap(), 16).unwrap())
}

#[test]
fn placing_defines_the_block_at_the_drawing_units() {
    let mut s = Session::new();
    let r = s.execute("models3d.place", &json!({"name": "m3_desk", "at": [100, 50], "rotation": 90})).unwrap();
    assert_eq!(r["block"], "M3_DESK");
    let d = s.doc().unwrap();
    let EntityKind::Insert(i) = &d.entity(h(&r)).unwrap().kind else { panic!() };
    assert_eq!((i.scale.x, i.rotation.to_degrees().round()), (1.0, 90.0));
    let blk = d.block("M3_DESK").unwrap();
    assert!(blk.entities.iter().any(|e| matches!(e.kind, EntityKind::Face3d(_))));
    assert!(blk.entities.iter().any(|e| matches!(e.kind, EntityKind::LwPolyline(_))));
    // A metric drawing places it in millimetres.
    let mut m = Session::new();
    m.execute("setvar", &json!({"name": "INSUNITS", "value": 4})).unwrap();
    let r = m.execute("models3d.place", &json!({"name": "M3_BED_QUEEN", "at": [0, 0, 3000]})).unwrap();
    let EntityKind::Insert(i) = &m.doc().unwrap().entity(h(&r)).unwrap().kind else { panic!() };
    assert_eq!((i.scale.x, i.insert.z), (25.4, 3000.0));
    // The list.
    let l = s.execute("models3d.list", &json!({"query": "toilet"})).unwrap();
    assert_eq!(l["models"][0]["name"], "M3_WATER_CLOSET");
    assert_eq!(l["models"][0]["twinOf"], "PL_WATER_CLOSET");
    assert_eq!(s.execute("models3d.list", &json!({"category": "Lighting"})).unwrap()["models"].as_array().unwrap().len(), 6);
    // Hostile.
    for bad in [json!({"name": "NOPE", "at": [0, 0]}), json!({"name": "M3_DESK"}), json!({"name": "M3_DESK", "at": ["x", 0]})] {
        assert!(s.execute("models3d.place", &bad).is_err(), "{bad}");
    }
}

#[test]
fn placed_symbols_get_their_3d_twins_once() {
    let mut s = Session::new();
    s.execute("symbol", &json!({"name": "PL_WATER_CLOSET", "at": [0, 0]})).unwrap();
    s.execute("symbol", &json!({"name": "EL_RECEPTACLE_DUPLEX", "at": [50, 0], "rotation": 180})).unwrap();
    s.execute("symbol", &json!({"name": "PL_GATE_VALVE", "at": [90, 0]})).unwrap();
    let r = s.execute("models3d.twins", &json!({})).unwrap();
    assert_eq!(r["added"], 2, "{r}");
    let d = s.doc().unwrap();
    let twins: Vec<_> = d
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Insert(i) if i.block.ends_with("_BODY") => Some(i.clone()),
            _ => None,
        })
        .collect();
    let rec = twins.iter().find(|i| i.block == "M3_RECEPTACLE_BODY").unwrap();
    assert_eq!((rec.insert.x, rec.rotation.to_degrees().round()), (50.0, 180.0));
    // The plate is centred 18" above the floor.
    let body = d.block("M3_RECEPTACLE_BODY").unwrap();
    assert!(body.entities.iter().all(|e| matches!(e.kind, EntityKind::Face3d(_))), "the twin is the body only");
    let (z0, z1, _, _) = body_extents(&body.entities.iter().map(|e| (**e).clone()).collect::<Vec<_>>());
    assert!(((z0 + z1) / 2.0 - 18.0).abs() < 1e-9, "{z0}..{z1}");
    // Again: nothing more.
    assert_eq!(s.execute("models3d.twins", &json!({})).unwrap()["added"], 0);
    // Undo removes the twins in one step.
    s.execute("undo", &json!({})).unwrap();
    s.execute("undo", &json!({})).unwrap();
    assert!(!s.doc().unwrap().model.iter().any(|e| matches!(&e.kind, EntityKind::Insert(i) if i.block.ends_with("_BODY"))));
}
