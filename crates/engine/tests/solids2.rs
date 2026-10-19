//! Composite solids and solid edits (Q52): UNION, SUBTRACT and INTERSECT on solids (overlapping,
//! touching, coplanar and nested), SLICE, THICKEN, FILLETEDGE, CHAMFEREDGE and SOLIDEDIT, with
//! their volumes checked; edits undo; bad input is refused.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle, Solid3d};
use serde_json::{Value, json};
use std::f64::consts::PI;

fn session() -> Session {
    let mut s = Session::new();
    s.viewport_px = (1200.0, 800.0);
    s
}

fn handle(v: &Value) -> Handle {
    let h = v["handle"].as_str().or_else(|| v["handles"][0].as_str()).unwrap_or_else(|| panic!("{v}"));
    Handle::parse_hex(h).unwrap()
}

fn solids(s: &Session) -> Vec<(Handle, Solid3d)> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter_map(|e| if let EntityKind::Solid3d(so) = &e.kind { Some((e.handle, (**so).clone())) } else { None })
        .collect()
}

fn volume(s: &mut Session, h: Handle) -> f64 {
    let r = s.execute("massprop", &json!({ "handles": [h.hex()] })).unwrap();
    r["solids"][0]["volume"].as_f64().unwrap()
}

fn near(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * b.abs().max(1e-9)
}

fn cube(s: &mut Session, at: [f64; 3], size: f64) -> Handle {
    handle(&s.execute("box", &json!({ "corner": at, "length": size })).unwrap())
}

#[test]
fn cube_minus_cylinder_and_the_classic_cases() {
    let mut s = session();
    let b = cube(&mut s, [0.0, 0.0, 0.0], 10.0);
    let c = handle(&s.execute("cylinder", &json!({ "center": [5, 5, -1], "radius": 2, "height": 12 })).unwrap());
    s.execute("subtract", &json!({ "from": [b.hex()], "remove": [c.hex()] })).unwrap();
    assert_eq!(solids(&s).len(), 1);
    assert!(near(volume(&mut s, b), 1000.0 - 40.0 * PI, 1e-3), "{}", volume(&mut s, b));
    assert_eq!(s.doc().unwrap().entity(b).unwrap().kind.type_name(), "3D Solid");
    // Undo brings both back.
    s.undo().unwrap();
    assert_eq!(solids(&s).len(), 2);

    // Touching: two cubes side by side make one block.
    let mut s = session();
    let a = cube(&mut s, [0.0, 0.0, 0.0], 10.0);
    let b = cube(&mut s, [10.0, 0.0, 0.0], 10.0);
    s.execute("union", &json!({ "handles": [a.hex(), b.hex()] })).unwrap();
    assert!(near(volume(&mut s, a), 2000.0, 1e-9));
    // Coplanar: a cube flush with the block's top, taken away.
    let c = handle(&s.execute("box", &json!({ "corner": [2, 2, 5], "length": 4, "width": 4, "height": 5 })).unwrap());
    s.execute("subtract", &json!({ "from": [a.hex()], "remove": [c.hex()] })).unwrap();
    assert!(near(volume(&mut s, a), 2000.0 - 80.0, 1e-9));
    // Nested: a ball wholly inside.
    let ball = handle(&s.execute("sphere", &json!({ "center": [15, 5, 5], "radius": 2 })).unwrap());
    s.execute("subtract", &json!({ "from": [a.hex()], "remove": [ball.hex()] })).unwrap();
    assert!(near(volume(&mut s, a), 2000.0 - 80.0 - 32.0 / 3.0 * PI, 2e-3));

    // Intersect: the overlap; apart, nothing changes.
    let mut s = session();
    let a = cube(&mut s, [0.0, 0.0, 0.0], 4.0);
    let b = cube(&mut s, [2.0, 2.0, 2.0], 4.0);
    s.execute("intersect", &json!({ "handles": [a.hex(), b.hex()] })).unwrap();
    assert!(near(volume(&mut s, a), 8.0, 1e-9));
    let far = cube(&mut s, [100.0, 0.0, 0.0], 1.0);
    assert!(s.execute("intersect", &json!({ "handles": [a.hex(), far.hex()] })).is_err());
    assert_eq!(solids(&s).len(), 2);
}

#[test]
fn slice_and_thicken() {
    let mut s = session();
    let a = cube(&mut s, [0.0, 0.0, 0.0], 10.0);
    // A vertical plane through x = 4: both sides kept.
    let r = s.execute("slice", &json!({ "handles": [a.hex()], "point": [4, 0, 0], "through": [4, 10, 0] })).unwrap();
    let other = Handle::parse_hex(r["created"][0].as_str().unwrap()).unwrap();
    let (va, vb) = (volume(&mut s, a), volume(&mut s, other));
    assert!(near(va + vb, 1000.0, 1e-9) && (near(va, 400.0, 1e-9) || near(va, 600.0, 1e-9)), "{va} {vb}");
    // The XY plane at z = 3, keeping the top.
    s.execute("slice", &json!({ "handles": [other.hex()], "point": [0, 0, 3], "plane": "xy", "keep": [0, 0, 9] })).unwrap();
    assert!(near(volume(&mut s, other), vb * 0.7, 1e-9));
    // THICKEN a region by −2 (downward).
    let sq = handle(&s.execute("rectang", &json!({ "p1": [0, 50], "p2": [4, 54] })).unwrap());
    s.execute("region", &json!({ "handles": [sq.hex()] })).unwrap();
    let reg = s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::Region(_))).unwrap().handle;
    let r = s.execute("thicken", &json!({ "handles": [reg.hex()], "thickness": -2 })).unwrap();
    let t = handle(&r);
    assert!(near(volume(&mut s, t), 32.0, 1e-9));
    let mm = s.execute("massprop", &json!({ "handles": [t.hex()] })).unwrap();
    assert!(mm["solids"][0]["centroid"][2].as_f64().unwrap() < 0.0, "{mm}");
}

#[test]
fn fillets_chamfers_and_solidedit() {
    let mut s = session();
    let a = cube(&mut s, [0.0, 0.0, 0.0], 10.0);
    // Seen from above, (5, 0) is the top front edge.
    s.execute("chamferedge", &json!({ "handle": a.hex(), "edges": [[5, 0]], "distance": 2 })).unwrap();
    assert!(near(volume(&mut s, a), 1000.0 - 20.0, 1e-9), "{}", volume(&mut s, a));
    s.execute("filletedge", &json!({ "handle": a.hex(), "edges": [[5, 10, 10]], "radius": 1 })).unwrap();
    assert!(near(volume(&mut s, a), 1000.0 - 20.0 - (1.0 - PI / 4.0) * 10.0, 1e-3), "{}", volume(&mut s, a));
    // Deleting the fillet's face takes it back off.
    s.execute("solidedit", &json!({ "handle": a.hex(), "op": "delete", "faces": [[5, 10.0 - 0.3, 10.0 - 0.3]] })).unwrap();
    assert!(near(volume(&mut s, a), 980.0, 1e-9), "{}", volume(&mut s, a));

    // A hole: offset, moved, deleted.
    let mut s = session();
    let b = cube(&mut s, [0.0, 0.0, 0.0], 10.0);
    let c = handle(&s.execute("cylinder", &json!({ "center": [3, 3, -1], "radius": 1, "height": 12 })).unwrap());
    s.execute("subtract", &json!({ "from": [b.hex()], "remove": [c.hex()] })).unwrap();
    s.execute("solidedit.offset", &json!({ "handle": b.hex(), "faces": [[4, 3, 5]], "distance": -1 })).unwrap();
    assert!(near(volume(&mut s, b), 1000.0 - 40.0 * PI, 2e-3), "{}", volume(&mut s, b));
    s.execute("solidedit.move", &json!({ "handle": b.hex(), "faces": [[5, 3, 5]], "delta": [3, 3, 0] })).unwrap();
    let r = s.execute("massprop", &json!({ "handles": [b.hex()] })).unwrap();
    let cx = r["solids"][0]["centroid"][0].as_f64().unwrap();
    assert!(cx < 5.0 && cx > 4.0, "the hole moved to (6, 6): {r}");
    s.execute("solidedit.delete", &json!({ "handle": b.hex(), "faces": [[8, 6, 5]] })).unwrap();
    assert!(near(volume(&mut s, b), 1000.0, 1e-9));
    // The box's own top up 2; its side can't be deleted.
    s.execute("solidedit.move", &json!({ "handle": b.hex(), "faces": [[5, 5]], "delta": [0, 0, 2] })).unwrap();
    assert!(near(volume(&mut s, b), 1200.0, 1e-9));
    assert!(s.execute("solidedit.delete", &json!({ "handle": b.hex(), "faces": [[5, 5]] })).is_err());
    // SHELL: 1 thick, open at the top.
    s.execute("solidedit.shell", &json!({ "handle": b.hex(), "thickness": 1, "faces": [[5, 5]] })).unwrap();
    assert!(near(volume(&mut s, b), 1200.0 - 8.0 * 8.0 * 11.0, 1e-9), "{}", volume(&mut s, b));
}

#[test]
fn the_prompts_edit_solids() {
    let mut s = session();
    let a = cube(&mut s, [0.0, 0.0, 0.0], 10.0);
    let b = cube(&mut s, [5.0, 5.0, 5.0], 10.0);
    s.set_selection(vec![a, b]);
    s.cmdline("union").unwrap();
    assert_eq!(solids(&s).len(), 1);
    assert!(near(volume(&mut s, a), 2000.0 - 125.0, 1e-9));
    // FILLETEDGE: pick the solid, an edge, Enter, the radius.
    s.set_selection(vec![]);
    s.cmdline("filletedge").unwrap();
    s.cmdline("5,5").unwrap();
    s.cmdline("5,0").unwrap();
    s.cmdline("").unwrap();
    s.cmdline("1").unwrap();
    assert!(near(volume(&mut s, a), 1875.0 - (1.0 - PI / 4.0) * 10.0, 1e-3), "{}", volume(&mut s, a));
    // SLICE along the XY plane through z = 5, keeping both.
    s.set_selection(vec![a]);
    s.cmdline("slice").unwrap();
    s.cmdline("XY").unwrap();
    s.cmdline("5").unwrap();
    s.cmdline("").unwrap();
    assert_eq!(solids(&s).len(), 2);
}

#[test]
fn hostile_edits_are_refused() {
    let mut s = session();
    let a = cube(&mut s, [0.0, 0.0, 0.0], 10.0);
    for (cmd, p) in [
        ("filletedge", json!({ "handle": a.hex(), "edges": [[500, 500, 500]], "radius": 1 })),
        ("filletedge", json!({ "handle": a.hex(), "edges": [[5, 0]], "radius": f64::NAN })),
        ("chamferedge", json!({ "handle": a.hex(), "edges": "x", "distance": 1 })),
        ("solidedit", json!({ "handle": a.hex(), "op": "explode", "faces": [[5, 5]] })),
        ("solidedit", json!({ "handle": a.hex(), "op": "offset", "faces": [[5, 5]], "distance": -20 })),
        ("solidedit", json!({ "handle": a.hex(), "op": "shell", "thickness": 50 })),
        ("slice", json!({ "handles": [a.hex()], "point": [0, 0, 0] })),
        ("thicken", json!({ "handles": [a.hex()], "thickness": 1 })),
        ("subtract", json!({ "from": [a.hex()], "remove": [a.hex()] })),
    ] {
        assert!(s.execute(cmd, &p).is_err(), "{cmd} {p}");
    }
    // Hostile edits leave the cube as it was. A 1-thick chamfer far beyond the solid's reach is
    // fine — it just finds the nearest edge — but a distance past the faces empties nothing.
    assert!(near(volume(&mut s, a), 1000.0, 1e-9));
}
