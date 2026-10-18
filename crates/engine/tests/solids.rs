//! 3D solids (Q51): the primitives and EXTRUDE, REVOLVE, SWEEP, LOFT and PRESSPULL make exact
//! solids; MASSPROP reports their volume, area and centroid; grips and Properties set their
//! sizes; they move, undo, explode into a mesh and refuse bad input without panicking.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle, Solid3d, SolidShape};
use cadaddy_engine::geom::{Vec2, Vec3};
use serde_json::{Value, json};
use std::f64::consts::PI;

fn session() -> Session {
    let mut s = Session::new();
    s.viewport_px = (1200.0, 800.0);
    s
}

/// The object a command made (`handle`, or the first of `handles`).
fn handle(v: &Value) -> Handle {
    let h = v["handle"].as_str().or_else(|| v["handles"][0].as_str()).unwrap_or_else(|| panic!("{v}"));
    Handle::parse_hex(h).unwrap()
}

fn solid(s: &Session, h: Handle) -> Solid3d {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Solid3d(so) => (**so).clone(),
        k => panic!("{k:?}"),
    }
}

fn solids(s: &Session) -> Vec<Solid3d> {
    s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Solid3d(so) = &e.kind { Some((**so).clone()) } else { None }).collect()
}

/// MASSPROP's (volume, surface area, centroid) for one object.
fn massprop(s: &mut Session, h: Handle) -> (f64, f64, Vec3) {
    let r = s.execute("massprop", &json!({ "handles": [h.hex()] })).unwrap();
    let m = &r["solids"][0];
    let c = &m["centroid"];
    (
        m["volume"].as_f64().unwrap(),
        m["surfaceArea"].as_f64().unwrap(),
        Vec3::new(c[0].as_f64().unwrap(), c[1].as_f64().unwrap(), c[2].as_f64().unwrap()),
    )
}

fn exact(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * (1.0 + b.abs())
}

#[test]
fn primitives_have_exact_volumes_areas_and_centroids() {
    let mut s = session();
    let cases: Vec<(&str, Value, f64, f64, Vec3)> = vec![
        ("box", json!({ "corner": [1, 2, 3], "length": 4, "width": 5, "height": 6 }), 120.0, 2.0 * (20.0 + 24.0 + 30.0), Vec3::new(3.0, 4.5, 6.0)),
        ("box", json!({ "center": [0, 0, 0], "length": 2 }), 8.0, 24.0, Vec3::ZERO),
        ("box", json!({ "corner": [0, 0], "opposite": [3, -2], "height": -1 }), 6.0, 2.0 * (6.0 + 3.0 + 2.0), Vec3::new(1.5, -1.0, -0.5)),
        ("wedge", json!({ "corner": [0, 0], "length": 3, "width": 2, "height": 4 }), 12.0, 36.0, Vec3::new(1.0, 1.0, 4.0 / 3.0)),
        ("cylinder", json!({ "center": [5, 5, 0], "radius": 2, "height": 10 }), 40.0 * PI, 8.0 * PI + 40.0 * PI, Vec3::new(5.0, 5.0, 5.0)),
        ("cylinder", json!({ "center": [0, 0, 0], "diameter": 2, "height": -3 }), 3.0 * PI, 2.0 * PI + 6.0 * PI, Vec3::new(0.0, 0.0, -1.5)),
        ("cone", json!({ "center": [0, 0, 0], "radius": 3, "height": 4 }), 12.0 * PI, 9.0 * PI + 15.0 * PI, Vec3::new(0.0, 0.0, 1.0)),
        (
            "cone",
            json!({ "center": [0, 0, 0], "radius": 2, "topRadius": 1, "height": 3 }),
            7.0 * PI,
            5.0 * PI + 3.0 * PI * 10f64.sqrt(),
            Vec3::new(0.0, 0.0, 3.0 * 11.0 / 28.0),
        ),
        ("sphere", json!({ "center": [1, 1, 1], "radius": 3 }), 36.0 * PI, 36.0 * PI, Vec3::new(1.0, 1.0, 1.0)),
        ("torus", json!({ "center": [0, 0, 0], "radius": 5, "tube": 1 }), 10.0 * PI * PI, 20.0 * PI * PI, Vec3::ZERO),
        // A square base 2 × 2 (radius to the edges' middles) rising to a point 3 up.
        ("pyramid", json!({ "center": [0, 0, 0], "radius": 1, "height": 3 }), 4.0, 4.0 + 4.0 * 10f64.sqrt(), Vec3::new(0.0, 0.0, 0.75)),
    ];
    for (cmd, p, v, a, c) in cases {
        let h = handle(&s.execute(cmd, &p).unwrap());
        let (mv, ma, mc) = massprop(&mut s, h);
        assert!(exact(mv, v), "{cmd} {p}: volume {mv} vs {v}");
        assert!(exact(ma, a), "{cmd} {p}: area {ma} vs {a}");
        assert!((mc - c).len() < 1e-9, "{cmd} {p}: centroid {mc:?} vs {c:?}");
    }
    // A square pyramid sits square to the axes.
    let h = handle(&s.execute("pyramid", &json!({ "center": [0, 0, 0], "radius": 1, "height": 3 })).unwrap());
    let r = s.execute("massprop", &json!({ "handles": [h.hex()] })).unwrap();
    assert!(exact(r["solids"][0]["boundingBox"][1][0].as_f64().unwrap(), 1.0), "{r}");
    assert!(r["message"].as_str().unwrap().contains("Volume:"));
}

#[test]
fn extrude_makes_solids_from_closed_profiles_and_erases_them() {
    let mut s = session();
    let rect = handle(&s.execute("rectang", &json!({ "p1": [0, 0], "p2": [4, 3] })).unwrap());
    let circ = handle(&s.execute("circle", &json!({ "center": [10, 0], "radius": 1 })).unwrap());
    let r = s.execute("extrude", &json!({ "handles": [rect.hex(), circ.hex()], "height": 2 })).unwrap();
    let made: Vec<Handle> = r["handles"].as_array().unwrap().iter().map(|v| Handle::parse_hex(v.as_str().unwrap()).unwrap()).collect();
    assert_eq!(made.len(), 2);
    assert!(exact(massprop(&mut s, made[0]).0, 24.0));
    assert!(exact(massprop(&mut s, made[1]).0, 2.0 * PI));
    assert!(s.doc().unwrap().entity(rect).is_none() && s.doc().unwrap().entity(circ).is_none());
    // Down, tapered, kept.
    let sq = handle(&s.execute("rectang", &json!({ "p1": [0, 20], "p2": [4, 24] })).unwrap());
    let r = s.execute("extrude", &json!({ "handles": [sq.hex()], "height": -1, "taper": 45, "keep": true })).unwrap();
    let h = handle(&r);
    let (v, _, c) = massprop(&mut s, h);
    assert!((v - 28.0 / 3.0).abs() < 1e-6, "{v}");
    assert!(c.z < 0.0 && c.z > -1.0, "{c:?}");
    assert!(s.doc().unwrap().entity(sq).is_some());
    // Along a direction (sheared): the profile's area times the rise.
    let sq2 = handle(&s.execute("rectang", &json!({ "p1": [0, 40], "p2": [2, 42] })).unwrap());
    let h = handle(&s.execute("extrude", &json!({ "handles": [sq2.hex()], "direction": [3, 0, 2] })).unwrap());
    assert!((massprop(&mut s, h).0 - 8.0).abs() < 1e-6);
    // Open curves are not profiles.
    let l = handle(&s.execute("line", &json!({ "points": [[0, 0], [5, 5]] })).unwrap());
    assert!(s.execute("extrude", &json!({ "handles": [l.hex()], "height": 1 })).is_err());
}

#[test]
fn revolve_follows_pappus_both_ways_and_refuses_a_crossing_profile() {
    let mut s = session();
    // A unit square 2..3 from the Y axis: 2π · 2.5.
    let sq = handle(&s.execute("rectang", &json!({ "p1": [2, 0], "p2": [3, 1] })).unwrap());
    let h = handle(&s.execute("revolve", &json!({ "handles": [sq.hex()], "axisStart": [0, 0], "axisEnd": [0, 1], "keep": true })).unwrap());
    let (v, _, c) = massprop(&mut s, h);
    assert!((v - 5.0 * PI).abs() < 5e-3 * v, "{v}");
    assert!(c.x.abs() < 1e-6 && (c.y - 0.5).abs() < 1e-6 && c.z.abs() < 1e-6, "{c:?}");
    // A quarter turn each way lands on either side of the profile's plane.
    let a =
        handle(&s.execute("revolve", &json!({ "handles": [sq.hex()], "axisStart": [0, 0], "axisEnd": [0, 1], "angle": 90, "keep": true })).unwrap());
    let b =
        handle(&s.execute("revolve", &json!({ "handles": [sq.hex()], "axisStart": [0, 0], "axisEnd": [0, 1], "angle": -90, "keep": true })).unwrap());
    let (va, _, ca) = massprop(&mut s, a);
    let (vb, _, cb) = massprop(&mut s, b);
    assert!((va - 1.25 * PI).abs() < 5e-3 * va && (vb - va).abs() < 1e-9 * va);
    assert!(ca.z * cb.z < 0.0, "{ca:?} {cb:?}");
    // Right-handed about +Y: X turns toward −Z.
    assert!(ca.z < 0.0);
    // A profile on the other side revolves too; one across the axis can't.
    let left = handle(&s.execute("rectang", &json!({ "p1": [-3, 0], "p2": [-2, 1] })).unwrap());
    assert!(s.execute("revolve", &json!({ "handles": [left.hex()], "axisStart": [0, 0], "axisEnd": [0, 1] })).is_ok());
    let across = handle(&s.execute("rectang", &json!({ "p1": [-1, 0], "p2": [1, 1] })).unwrap());
    assert!(s.execute("revolve", &json!({ "handles": [across.hex()], "axisStart": [0, 0], "axisEnd": [0, 1] })).is_err());
}

#[test]
fn sweep_and_loft_follow_their_curves() {
    let mut s = session();
    // A circle of radius 1 swept along a 10-long line: a cylinder.
    let c = handle(&s.execute("circle", &json!({ "center": [50, 50], "radius": 1 })).unwrap());
    let l = handle(&s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap());
    let h = handle(&s.execute("sweep", &json!({ "handles": [c.hex()], "path": l.hex() })).unwrap());
    let (v, _, cen) = massprop(&mut s, h);
    assert!((v - 10.0 * PI).abs() < 2e-3 * v, "{v}");
    assert!((cen - Vec3::new(5.0, 0.0, 0.0)).len() < 1e-3, "{cen:?}");
    assert!(s.doc().unwrap().entity(c).is_none(), "the profile is used up");
    assert!(s.doc().unwrap().entity(l).is_some(), "the path stays");
    // Round an L-shaped polyline: about the profile's area times the path length.
    let sq = handle(&s.execute("rectang", &json!({ "p1": [60, 60], "p2": [60.5, 60.5] })).unwrap());
    let p = handle(&s.execute("pline", &json!({ "points": [[0, 10], [10, 10], [10, 20]] })).unwrap());
    let h = handle(&s.execute("sweep", &json!({ "handles": [sq.hex()], "path": p.hex() })).unwrap());
    let v = massprop(&mut s, h).0;
    assert!((v - 0.25 * 20.0).abs() < 0.05 * 5.0, "{v}");
    // A loft between circles of radius 2 and 1, 3 apart: a frustum.
    let c1 = handle(&s.execute("circle", &json!({ "center": [0, 0], "radius": 2 })).unwrap());
    let c2 = handle(&s.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap());
    s.execute("move", &json!({ "handles": [c2.hex()], "delta": [0, 0, 3] })).ok();
    s.doc_mut()
        .unwrap()
        .modify_entity(c2, |e| {
            if let EntityKind::Circle(ci) = &mut e.kind {
                ci.center.z = 3.0
            }
        })
        .unwrap();
    let h = handle(&s.execute("loft", &json!({ "handles": [c1.hex(), c2.hex()] })).unwrap());
    let v = massprop(&mut s, h).0;
    assert!((v - 7.0 * PI).abs() < 5e-3 * v, "{v}");
    assert!(s.execute("loft", &json!({ "handles": [] })).is_err());
}

#[test]
fn presspull_extrudes_the_area_around_a_point() {
    let mut s = session();
    for pts in [[[0, 0], [6, 0]], [[6, 0], [6, 4]], [[6, 4], [0, 4]], [[0, 4], [0, 0]]] {
        s.execute("line", &json!({ "points": pts })).unwrap();
    }
    s.execute("circle", &json!({ "center": [3, 2], "radius": 1 })).unwrap();
    // Between the square and the circle: the square less the hole.
    let h = handle(&s.execute("presspull", &json!({ "point": [0.5, 0.5], "height": 2 })).unwrap());
    let v = massprop(&mut s, h).0;
    assert!((v - (24.0 - PI) * 2.0).abs() < 1e-6, "{v}");
    assert!(s.execute("presspull", &json!({ "point": [50, 50], "height": 2 })).is_err());
}

#[test]
fn the_prompts_build_solids() {
    let mut s = session();
    for line in ["box 0,0 4,3 2", "cylinder 10,0 1 5", "cone 20,0 D 4 3", "sphere 30,0 2", "torus 40,0 5 1", "pyramid 50,0 1 3", "wedge 60,0 L 3 2 4"]
    {
        s.cmdline(line).unwrap();
    }
    let vols: Vec<f64> = solids(&s).iter().map(|so| so.props(1e-4).volume).collect();
    let want = [24.0, 5.0 * PI, 4.0 * PI, 32.0 / 3.0 * PI, 10.0 * PI * PI, 4.0, 12.0];
    assert_eq!(vols.len(), want.len(), "{vols:?}");
    for (v, w) in vols.iter().zip(want) {
        assert!(exact(*v, w), "{vols:?}");
    }
    // EXTRUDE at the prompts: select, then a height.
    let r = handle(&s.execute("rectang", &json!({ "p1": [0, 100], "p2": [2, 102] })).unwrap());
    s.set_selection(vec![r]);
    s.cmdline("extrude 3").unwrap();
    assert_eq!(solids(&s).len(), want.len() + 1);
    assert!(exact(solids(&s).last().unwrap().props(1e-4).volume, 12.0));
}

#[test]
fn grips_and_properties_set_sizes_and_edits_undo() {
    let mut s = session();
    let h = handle(&s.execute("cylinder", &json!({ "center": [0, 0, 0], "radius": 1, "height": 4 })).unwrap());
    // Grip 1 is the base radius, on +X; drag it out to 2.
    s.grip_edit(h, 1, Vec2::new(2.0, 0.0), cadaddy_engine::grips::GripMode::Stretch).unwrap();
    assert!(matches!(solid(&s, h).shape, SolidShape::Frustum { base_radius, .. } if (base_radius - 2.0).abs() < 1e-12));
    s.execute("properties.set", &json!({ "handles": [h.hex()], "height": 10 })).unwrap();
    assert!(exact(massprop(&mut s, h).0, 40.0 * PI));
    let props = s.execute("properties", &json!({ "handle": h.hex() })).unwrap();
    assert!(props.to_string().contains("Cylinder") && props.to_string().contains("volume"), "{props}");
    // The base grip moves it; MOVE and ROTATE keep its size.
    s.grip_edit(h, 0, Vec2::new(5.0, 5.0), cadaddy_engine::grips::GripMode::Stretch).unwrap();
    s.execute("rotate", &json!({ "handles": [h.hex()], "base": [0, 0], "angle": 90 })).unwrap();
    let (v, _, c) = massprop(&mut s, h);
    assert!(exact(v, 40.0 * PI) && (c - Vec3::new(-5.0, 5.0, 5.0)).len() < 1e-9, "{v} {c:?}");
    s.execute("scale", &json!({ "handles": [h.hex()], "base": [-5, 5], "factor": 2 })).unwrap();
    assert!(exact(massprop(&mut s, h).0, 320.0 * PI));
    s.undo().unwrap();
    assert!(exact(massprop(&mut s, h).0, 40.0 * PI));
    // EXPLODE gives the mesh of its surfaces.
    s.execute("explode", &json!({ "handles": [h.hex()] })).unwrap();
    assert!(s.doc().unwrap().entity(h).is_none());
    assert_eq!(s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Mesh(_))).count(), 1);
}

#[test]
fn hostile_parameters_are_refused() {
    let mut s = session();
    for (cmd, p) in [
        ("box", json!({ "corner": [0, 0], "length": -1 })),
        ("box", json!({ "corner": [0, 0], "length": 1e300 })),
        ("box", json!({ "corner": [0, 0], "length": 1, "height": 0 })),
        ("cylinder", json!({ "center": [0, 0], "radius": "x", "height": 1 })),
        ("sphere", json!({ "center": [0, 0] })),
        ("torus", json!({ "center": [0, 0], "radius": 1, "tube": 2 })),
        ("pyramid", json!({ "center": [0, 0], "radius": 1, "height": 1, "sides": 1000 })),
        ("cone", json!({ "center": ["a", 0], "radius": 1, "height": 1 })),
        ("extrude", json!({ "handles": ["zz"], "height": 1 })),
        ("extrude", json!({ "height": 1, "taper": 95 })),
        ("revolve", json!({ "axisStart": [0, 0], "axisEnd": [0, 0] })),
        ("revolve", json!({ "axisStart": [0, 0], "axisEnd": [0, 1], "angle": 1e9 })),
        ("sweep", json!({ "path": "FFFFFF" })),
        ("presspull", json!({ "point": [0, 0], "height": f64::MAX })),
    ] {
        assert!(s.execute(cmd, &p).is_err(), "{cmd} {p}");
    }
    assert!(solids(&s).is_empty());
}
