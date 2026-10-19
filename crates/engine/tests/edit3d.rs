//! 3D editing (Q61): 3DMOVE, 3DROTATE, 3DSCALE with axis constraints, 3DALIGN, 3DMIRROR, 3DARRAY,
//! 3D object snaps (vertex, midpoint, face centre, nearest, perpendicular), points in space at
//! the prompts, and dynamic UCS drawing on a face.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle};
use cadaddy_engine::geom::{Vec2, Vec3};
use cadaddy_engine::snap3::Snap3Kind;
use serde_json::{Value, json};

fn session() -> Session {
    let mut s = Session::new();
    s.viewport_px = (1000.0, 800.0);
    s
}

fn handle(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().or_else(|| v["handles"][0].as_str()).unwrap_or_else(|| panic!("{v}"))).unwrap()
}

/// A 10 cube at the origin.
fn cube(s: &mut Session) -> Handle {
    handle(&s.execute("box", &json!({ "corner": [0, 0, 0], "length": 10 })).unwrap())
}

fn bounds(s: &mut Session, h: Handle) -> (Vec3, Vec3) {
    let r = s.execute("massprop", &json!({ "handles": [h.hex()] })).unwrap();
    let b = &r["solids"][0]["boundingBox"];
    let v = |i: usize| Vec3::new(b[i][0].as_f64().unwrap(), b[i][1].as_f64().unwrap(), b[i][2].as_f64().unwrap());
    (v(0), v(1))
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a - b).len() < 1e-6
}

#[test]
fn move_rotate_scale_in_space() {
    let mut s = session();
    let b = cube(&mut s);
    let c = handle(&s.execute("circle", &json!({ "center": [50, 0], "radius": 2 })).unwrap());
    s.execute("3dmove", &json!({ "handles": [b.hex(), c.hex()], "delta": [0, 0, 5] })).unwrap();
    assert!(near(bounds(&mut s, b).0, Vec3::new(0.0, 0.0, 5.0)));
    assert!(matches!(&s.doc().unwrap().entity(c).unwrap().kind, EntityKind::Circle(cc) if cc.center.z == 5.0));
    // A quarter turn about X through the origin: the cube now spans y −15…−5 and z 0…10; the
    // circle can't tilt and stays.
    let r = s.execute("3drotate", &json!({ "handles": [b.hex(), c.hex()], "base": [0, 0, 0], "axis": "x", "angle": 90 })).unwrap();
    assert_eq!(r["unchanged"], 1, "{r}");
    let (lo, hi) = bounds(&mut s, b);
    assert!(near(lo, Vec3::new(0.0, -15.0, 0.0)) && near(hi, Vec3::new(10.0, -5.0, 10.0)), "{lo:?} {hi:?}");
    // Stretched along Z only.
    s.execute("3dscale", &json!({ "handles": [b.hex()], "base": [0, 0, 0], "factor": 2, "axis": "z" })).unwrap();
    let (lo, hi) = bounds(&mut s, b);
    assert!(near(lo, Vec3::new(0.0, -15.0, 0.0)) && near(hi, Vec3::new(10.0, -5.0, 20.0)), "{lo:?} {hi:?}");
    let v = s.execute("massprop", &json!({ "handles": [b.hex()] })).unwrap()["solids"][0]["volume"].as_f64().unwrap();
    assert!((v - 2000.0).abs() < 1e-6);
    s.undo().unwrap();
    assert!(near(bounds(&mut s, b).1, Vec3::new(10.0, -5.0, 10.0)));
}

#[test]
fn align_mirror_and_array() {
    let mut s = session();
    let b = cube(&mut s);
    // The cube's corner edge onto a new place, stood up: x → z.
    s.execute(
        "3dalign",
        &json!({ "handles": [b.hex()], "source": [[0, 0, 0], [10, 0, 0], [0, 10, 0]], "dest": [[100, 0, 0], [100, 0, 10], [100, 10, 0]] }),
    )
    .unwrap();
    let (lo, hi) = bounds(&mut s, b);
    assert!(near(lo, Vec3::new(90.0, 0.0, 0.0)) && near(hi, Vec3::new(100.0, 10.0, 10.0)), "{lo:?} {hi:?}");
    // Mirrored in the XY plane at z = 0: a copy below.
    let r = s.execute("3dmirror", &json!({ "handles": [b.hex()], "plane": "xy", "point": [0, 0, 0] })).unwrap();
    let m = handle(&r);
    let (lo, _) = bounds(&mut s, m);
    assert!(near(lo, Vec3::new(90.0, 0.0, -10.0)), "{lo:?}");
    // A 2 × 2 × 2 array: 7 copies; polar 4 about Z: 3 more.
    let mut t = session();
    let c = cube(&mut t);
    let r = t
        .execute(
            "3darray",
            &json!({ "handles": [c.hex()], "rows": 2, "columns": 2, "levels": 2, "rowSpacing": 20, "columnSpacing": 20, "levelSpacing": 20 }),
        )
        .unwrap();
    assert_eq!(r["copies"], 7);
    let r = t.execute("3darray", &json!({ "handles": [c.hex()], "polar": { "count": 4, "center": [-50, 0, 0], "axisEnd": [-50, 0, 1] } })).unwrap();
    assert_eq!(r["copies"], 3);
    assert!(t.execute("3darray", &json!({ "handles": [c.hex()], "rows": 1000, "columns": 1000 })).is_err());
}

#[test]
fn the_prompts_take_points_in_space_and_axes() {
    let mut s = session();
    let b = cube(&mut s);
    // 3DMOVE: base typed in space, held to Z, a distance.
    s.set_selection(vec![b]);
    s.cmdline("3dmove").unwrap();
    s.cmdline("0,0,0").unwrap();
    let p = s.current_prompt().unwrap();
    assert!(p.point3 && p.gizmo.is_some());
    s.cmdline("Z").unwrap();
    assert_eq!(s.current_prompt().unwrap().gizmo.unwrap().axis, Some(2));
    s.cmdline("7").unwrap();
    assert!(near(bounds(&mut s, b).0, Vec3::new(0.0, 0.0, 7.0)));
    // 3DROTATE about X by 90.
    s.set_selection(vec![b]);
    s.cmdline("3drotate").unwrap();
    s.cmdline("0,0,0").unwrap();
    s.cmdline("X").unwrap();
    s.cmdline("90").unwrap();
    let (_, hi) = bounds(&mut s, b);
    assert!(near(hi, Vec3::new(10.0, -7.0, 10.0)), "{hi:?}");
    // LINE keeps the heights typed.
    s.cmdline("line").unwrap();
    s.cmdline("0,0,5").unwrap();
    s.cmdline("10,0,8").unwrap();
    s.cmdline("").unwrap();
    assert!(s.doc().unwrap().model.iter().any(|e| matches!(&e.kind, EntityKind::Line(l) if l.a.z == 5.0 && l.b.z == 8.0)));
}

#[test]
fn snaps_in_space() {
    let mut s = session();
    cube(&mut s);
    s.cmdline("line").unwrap();
    // From above at the corner (10, 10): the top corner (nearer the viewer).
    let v = s.osnap3_at(Vec2::new(10.0, 10.0)).unwrap();
    assert_eq!(v.kind, Snap3Kind::Vertex);
    assert!(near(v.at, Vec3::new(10.0, 10.0, 10.0)), "{v:?}");
    // The middle of the top's edge, and the top's centre.
    assert!(s.osnap3_at(Vec2::new(5.0, 0.0)).is_some_and(|m| m.kind == Snap3Kind::Midpoint && near(m.at, Vec3::new(5.0, 0.0, 10.0))));
    let c = s.osnap3_at(Vec2::new(5.0, 5.0)).unwrap();
    assert!(c.kind == Snap3Kind::Center && near(c.at, Vec3::new(5.0, 5.0, 10.0)), "{c:?}");
    // Nearest on a face, with only that snap on.
    s.execute("3dosnap", &json!({ "modes": ["nearest"] })).unwrap();
    let n = s.osnap3_at(Vec2::new(3.0, 4.0)).unwrap();
    assert!(n.kind == Snap3Kind::Nearest && near(n.at, Vec3::new(3.0, 4.0, 10.0)));
    // Off: nothing.
    s.execute("3dosnap", &json!({ "on": false })).unwrap();
    assert!(s.osnap3_at(Vec2::new(10.0, 10.0)).is_none());
    // Dynamic UCS: in a 3D view, LINE's points land on the face under the cursor.
    s.execute("3dosnap", &json!({ "on": true, "modes": [] })).unwrap();
    s.cancel();
    s.execute("view.seiso", &json!({})).unwrap();
    s.cmdline("line").unwrap();
    let proj = s.state().unwrap().projection().unwrap();
    let on_top = proj.project(Vec3::new(4.0, 6.0, 10.0));
    let f = s.osnap3_at(on_top).unwrap();
    assert!(f.kind == Snap3Kind::OnFace && near(f.at, Vec3::new(4.0, 6.0, 10.0)), "{f:?}");
}
