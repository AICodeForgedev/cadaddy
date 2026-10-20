//! End to end: 3D editing — a box made, moved up with 3DMOVE, turned with 3DROTATE, and joined
//! with another (UNION); checked through MASSPROP (where the solid is, its volume) and the shaded
//! view rendering it.

mod e2e_common;

use e2e_common::E2e;
use serde_json::{Value, json};

fn solid(e: &mut E2e, h: &str) -> Value {
    e.exec("massprop", json!({ "handles": [h] }))["solids"][0].clone()
}

#[test]
fn move_rotate_and_join_solids() {
    let mut e = E2e::new();
    let b = e.exec("box", json!({ "corner": [0, 0, 0], "length": 10, "width": 10, "height": 10 }));
    let h = b["handle"].as_str().unwrap().to_string();
    let p = solid(&mut e, &h);
    let z0 = p["centroid"][2].as_f64().unwrap_or_else(|| panic!("{p}"));
    assert!((z0 - 5.0).abs() < 1e-6, "{p}");
    // 3DMOVE up 20.
    e.exec("3dmove", json!({ "handles": [h], "delta": [0, 0, 20] }));
    let z1 = solid(&mut e, &h)["centroid"][2].as_f64().unwrap();
    assert!((z1 - 25.0).abs() < 1e-6, "moved up: {z1}");
    // 3DROTATE 90° about X through the origin: the centre swings to y = -25, z = 5.
    e.exec("3drotate", json!({ "handles": [h], "base": [0, 0, 0], "axis": "x", "angle": 90 }));
    let c = solid(&mut e, &h)["centroid"].clone();
    assert!((c[1].as_f64().unwrap() + 25.0).abs() < 1e-6 && (c[2].as_f64().unwrap() - 5.0).abs() < 1e-6, "{c}");
    // UNION with an overlapping box: one solid of 1500.
    let b2 = e.exec("box", json!({ "corner": [5, -30, 0], "length": 10, "width": 10, "height": 10 }));
    let h2 = b2["handle"].as_str().unwrap().to_string();
    e.exec("union", json!({ "handles": [h, h2] }));
    let solids = e.exec("entities", json!({ "type": "3D Solid" }));
    assert_eq!(solids["count"], 1, "{solids}");
    let joined = solids["entities"][0]["handle"].as_str().unwrap().to_string();
    let v = solid(&mut e, &joined)["volume"].as_f64().unwrap();
    assert!((v - 1500.0).abs() < 1e-3, "volume {v}");
    // The shaded view shows it.
    e.typed("vscurrent shaded");
    assert!(e.inked(400, 300) > 500);
}
