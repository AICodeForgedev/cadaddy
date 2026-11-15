//! COPY, MOVE, COPYM, COPYTOLAYER, NCOPY and a non-associative ARRAY take the height of their
//! displacement, as 3DMOVE does (Q6656): a device copied one floor up stands one floor up, not on
//! top of the original. A command that moves in plan only (STRETCH, several hot grips) refuses a
//! height instead of dropping it; an object that has no height (a spline) is counted, not
//! silently left behind.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Common, Entity, EntityKind, Handle, Mesh, MeshFace, Point};
use cadaddy_engine::geom::{Spline, Vec2, Vec3};
use serde_json::{Value, json};

/// A storey's rise in the tests (inches): any made-up floor-to-floor height.
const RISE: f64 = 126.5;
/// The device's height above its floor (inches).
const Z0: f64 = 16.0;

fn add(s: &mut Session, kind: EntityKind) -> String {
    let space = s.space();
    let d = s.doc_mut().unwrap();
    let handle = d.new_handle();
    d.space_mut(&space).unwrap().push(Entity { handle, common: Common::default(), kind });
    handle.hex()
}

/// A device-like point at (`x`, `y`, [`Z0`]).
fn device(s: &mut Session, x: f64, y: f64) -> String {
    add(s, EntityKind::Point(Point { p: Vec3::new(x, y, Z0), angle: 0.0 }))
}

/// A built element's shape: a 4" mesh box whose bottom is at [`Z0`].
fn mesh_box(s: &mut Session, x: f64, y: f64) -> String {
    let (a, b) = (Vec3::new(x, y, Z0), Vec3::new(x + 4.0, y + 4.0, Z0 + 4.0));
    let vertices = vec![
        Vec3::new(a.x, a.y, a.z),
        Vec3::new(b.x, a.y, a.z),
        Vec3::new(b.x, b.y, a.z),
        Vec3::new(a.x, b.y, a.z),
        Vec3::new(a.x, a.y, b.z),
        Vec3::new(b.x, a.y, b.z),
        Vec3::new(b.x, b.y, b.z),
        Vec3::new(a.x, b.y, b.z),
    ];
    let quads: [[u32; 4]; 6] = [[0, 1, 2, 3], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]];
    let faces = quads.iter().map(|q| MeshFace { v: q.to_vec(), hidden: 0 }).collect();
    add(s, EntityKind::Mesh(Mesh { vertices, faces, ..Default::default() }))
}

fn handles(r: &Value) -> Vec<Handle> {
    r["handles"].as_array().unwrap().iter().map(|h| Handle::parse_hex(h.as_str().unwrap()).unwrap()).collect()
}

fn hx(h: &str) -> Handle {
    Handle::parse_hex(h).unwrap()
}

/// Where a point (or a mesh's first vertex) stands.
fn at(s: &Session, h: Handle) -> Vec3 {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Point(p) => p.p,
        EntityKind::Mesh(m) => m.vertices[0],
        k => panic!("not a point or mesh: {k:?}"),
    }
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9 && (a.z - b.z).abs() < 1e-9
}

/// Every point's position, sorted by height then x.
fn points(s: &Session) -> Vec<Vec3> {
    let mut v: Vec<Vec3> = s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Point(p) = &e.kind { Some(p.p) } else { None }).collect();
    v.sort_by(|a, b| a.z.total_cmp(&b.z).then(a.x.total_cmp(&b.x)).then(a.y.total_cmp(&b.y)));
    v
}

#[test]
fn copy_with_a_rise_lands_one_floor_up() {
    // The finding: copy {delta: [0, 0, rise]} put the copy on the original.
    let mut s = Session::new();
    let h = device(&mut s, 30.0, 40.0);
    let r = s.execute("copy", &json!({"handles": [h], "delta": [0, 0, RISE]})).unwrap();
    let c = handles(&r);
    assert_eq!(c.len(), 1, "{r}");
    assert!(near(at(&s, c[0]), Vec3::new(30.0, 40.0, Z0 + RISE)), "the copy is one floor up: {:?}", at(&s, c[0]));
    assert!(near(at(&s, hx(&h)), Vec3::new(30.0, 40.0, Z0)), "the original stays");
    // A plan delta still copies in plan only.
    let r = s.execute("copy", &json!({"handles": [h], "delta": [10, 0]})).unwrap();
    assert!(near(at(&s, handles(&r)[0]), Vec3::new(40.0, 40.0, Z0)));
}

#[test]
fn copy_from_to_points_in_space_and_count() {
    let mut s = Session::new();
    let h = mesh_box(&mut s, 0.0, 0.0);
    // from/to with heights: the difference is the rise.
    let r = s.execute("copy", &json!({"handles": [h], "from": [0, 0, Z0], "to": [5, 0, Z0 + RISE]})).unwrap();
    assert!(near(at(&s, handles(&r)[0]), Vec3::new(5.0, 0.0, Z0 + RISE)));
    // count: a stack of copies, one per floor.
    let r = s.execute("copy", &json!({"handles": [h], "delta": [0, 0, RISE], "count": 3})).unwrap();
    let zs: Vec<f64> = handles(&r).iter().map(|c| at(&s, *c).z).collect();
    assert_eq!(zs, vec![Z0 + RISE, Z0 + 2.0 * RISE, Z0 + 3.0 * RISE]);
    // Every vertex rose by the same amount (the box kept its shape).
    let d = s.doc().unwrap();
    let EntityKind::Mesh(m) = &d.entity(handles(&r)[0]).unwrap().kind else { panic!("not a mesh") };
    assert!(m.vertices.iter().all(|v| v.z == Z0 + RISE || v.z == Z0 + 4.0 + RISE), "{:?}", m.vertices);
}

#[test]
fn move_with_a_rise() {
    let mut s = Session::new();
    let h = device(&mut s, 1.0, 2.0);
    let r = s.execute("move", &json!({"handles": [h], "delta": [3, 0, -Z0]})).unwrap();
    assert_eq!(r["moved"], json!(1), "{r}");
    assert!(near(at(&s, hx(&h)), Vec3::new(4.0, 2.0, 0.0)));
}

#[test]
fn a_bad_height_is_refused_not_dropped() {
    let mut s = Session::new();
    let h = device(&mut s, 0.0, 0.0);
    for bad in [json!([0, 0, "up"]), json!([0, 0, 1, 2]), json!({"x": 0, "y": 0, "z": "x"})] {
        assert!(s.execute("copy", &json!({"handles": [h], "delta": bad})).is_err(), "{bad}");
        assert!(s.execute("move", &json!({"handles": [h], "delta": bad})).is_err(), "{bad}");
    }
    assert_eq!(points(&s), vec![Vec3::new(0.0, 0.0, Z0)], "nothing copied or moved");
}

#[test]
fn an_object_without_height_is_counted() {
    let mut s = Session::new();
    let p = device(&mut s, 0.0, 0.0);
    let sp = Spline::from_fit(&[Vec2::new(0.0, 0.0), Vec2::new(5.0, 3.0), Vec2::new(10.0, 0.0)], false);
    let k = add(&mut s, EntityKind::Spline(Box::new(sp)));
    let r = s.execute("copy", &json!({"handles": [p, k], "delta": [0, 0, RISE]})).unwrap();
    assert_eq!(handles(&r).len(), 2, "both copied: {r}");
    assert_eq!(r["unraised"], json!(1), "the spline could not rise and says so: {r}");
    assert!(r["message"].as_str().unwrap().contains("no height"), "{r}");
}

#[test]
fn copym_points_repeat_and_divide_rise() {
    let mut s = Session::new();
    let h = device(&mut s, 0.0, 0.0);
    // Points in space: each copy at its point's height above the base.
    let r = s.execute("copym", &json!({"handles": [h], "base": [0, 0, Z0], "to": [[0, 0, Z0 + RISE], [10, 0, Z0]]})).unwrap();
    let got: Vec<Vec3> = handles(&r).iter().map(|c| at(&s, *c)).collect();
    assert!(near(got[0], Vec3::new(0.0, 0.0, Z0 + RISE)) && near(got[1], Vec3::new(10.0, 0.0, Z0)), "{got:?}");
    // Repeat by a rise: a stack.
    let r = s.execute("copym", &json!({"handles": [h], "base": [0, 0], "mode": "repeat", "to": [0, 0, RISE], "count": 2})).unwrap();
    let zs: Vec<f64> = handles(&r).iter().map(|c| at(&s, *c).z).collect();
    assert_eq!(zs, vec![Z0 + RISE, Z0 + 2.0 * RISE]);
    // Divide spreads in space.
    let r = s.execute("copym", &json!({"handles": [h], "base": [0, 0, 0], "mode": "divide", "to": [0, 0, 2.0 * RISE], "count": 2})).unwrap();
    let zs: Vec<f64> = handles(&r).iter().map(|c| at(&s, *c).z).collect();
    assert_eq!(zs, vec![Z0 + RISE, Z0 + 2.0 * RISE]);
    // The Array mode is a flat grid: a height is refused.
    let e = s.execute("copym", &json!({"handles": [h], "base": [0, 0], "mode": "array", "to": [5, 5, RISE], "corner": [20, 20]})).unwrap_err();
    assert!(e.to_string().contains("flat grid"), "{e}");
}

#[test]
fn copytolayer_rises() {
    let mut s = Session::new();
    let h = device(&mut s, 0.0, 0.0);
    let r = s.execute("copytolayer", &json!({"handles": [h], "layer": "UPPER", "delta": [0, 0, RISE]})).unwrap();
    assert!(near(at(&s, handles(&r)[0]), Vec3::new(0.0, 0.0, Z0 + RISE)), "{r}");
}

#[test]
fn array_copies_on_every_level_and_row_elevation() {
    let mut s = Session::new();
    let h = device(&mut s, 0.0, 0.0);
    // Not associative: separate copies, on each of 3 levels a storey apart (they used to be
    // made on the first level only).
    let r = s
        .execute(
            "arrayrect",
            &json!({"handles": [h], "rows": 1, "cols": 2, "colSpacing": 10, "levels": 3, "levelSpacing": RISE, "associative": false}),
        )
        .unwrap();
    assert_eq!(r["created"], json!(5), "{r}");
    let want: Vec<Vec3> = (0..3).flat_map(|l| [0.0, 10.0].map(|x| Vec3::new(x, 0.0, Z0 + RISE * l as f64))).collect();
    assert_eq!(points(&s), want);
    // Row elevation raises each row.
    let mut s = Session::new();
    let h = device(&mut s, 0.0, 0.0);
    s.execute("arrayrect", &json!({"handles": [h], "rows": 2, "cols": 1, "rowSpacing": 5, "rowElevation": 12, "associative": false})).unwrap();
    assert_eq!(points(&s), vec![Vec3::new(0.0, 0.0, Z0), Vec3::new(0.0, 5.0, Z0 + 12.0)]);
}

#[test]
fn plan_only_commands_refuse_a_height() {
    let mut s = Session::new();
    s.cmdline("LINE 0,0 10,0").unwrap();
    s.cmdline("").unwrap();
    let e = s.execute("stretch", &json!({"window": [[-1, -1], [1, 1]], "delta": [0, 0, RISE]})).unwrap_err();
    assert!(e.to_string().contains("3dmove"), "{e}");
    // The same height at both ends is no rise: accepted.
    s.execute("stretch", &json!({"window": [[-1, -1], [1, 1]], "from": [0, 0, 5], "to": [1, 0, 5]})).unwrap();
}

#[test]
fn typed_points_in_space_at_the_command_line() {
    // COPY: base point picked in plan, second point typed in space (relative): one floor up.
    let mut s = Session::new();
    device(&mut s, 30.0, 40.0);
    s.script(&format!("COPY ALL\n\n30,40 @0,0,{RISE}\n\n")).unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_eq!(points(&s), vec![Vec3::new(30.0, 40.0, Z0), Vec3::new(30.0, 40.0, Z0 + RISE)]);
    // MOVE by a displacement in space.
    let mut s = Session::new();
    device(&mut s, 0.0, 0.0);
    s.script(&format!("MOVE ALL\n\nD 1,2,{RISE}\n")).unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_eq!(points(&s), vec![Vec3::new(1.0, 2.0, Z0 + RISE)]);
    // Base and second point both in space: the difference of their heights.
    let mut s = Session::new();
    device(&mut s, 0.0, 0.0);
    s.script(&format!("COPY ALL\n\n0,0,{Z0} 0,0,{}\n\n", Z0 + RISE)).unwrap();
    assert_eq!(points(&s), vec![Vec3::new(0.0, 0.0, Z0), Vec3::new(0.0, 0.0, Z0 + RISE)]);
}
