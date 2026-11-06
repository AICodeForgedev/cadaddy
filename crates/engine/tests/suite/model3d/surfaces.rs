//! NURBS surfaces (Q160): a blend between two edges is G1, trimming then untrimming restores the
//! surface, INTERFERE's volume of two overlapping boxes is the overlap box's; and the other
//! surface commands make and edit what they say.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle, Surface3d};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn handle(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap()).unwrap()
}

fn surface(s: &Session, h: Handle) -> Surface3d {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Surface(sf) => (**sf).clone(),
        k => panic!("not a surface: {k:?}"),
    }
}

/// Run a command that makes a surface, and the surface it made.
fn made(s: &mut Session, cmd: &str, p: &Value) -> Surface3d {
    let h = handle(&s.execute(cmd, p).unwrap());
    surface(s, h)
}

fn near(a: Vec3, b: Vec3, tol: f64) -> bool {
    (a - b).len() <= tol
}

/// A flat 10 × 10 square at height `z`, its corner at `x`.
fn square(s: &mut Session, x: f64, z: f64) -> Handle {
    handle(&s.execute("planesurf", &json!({"corners": [[x, 0, z], [x + 10.0, 10, z]]})).unwrap())
}

/// Four smooth edges round a 10 × 10 saddle (up along y = 0 and 10, down along x = 0 and 10),
/// nine points each, in order round the patch.
fn saddle_edges() -> Value {
    let wave = |t: f64| 2.0 * (std::f64::consts::PI * t).sin();
    let pts = |f: &dyn Fn(f64) -> [f64; 3]| -> Value { json!((0..9).map(|k| f(k as f64 / 8.0)).collect::<Vec<_>>()) };
    json!([
        pts(&|t| [10.0 * t, 0.0, wave(t)]),
        pts(&|t| [10.0, 10.0 * t, -wave(t)]),
        pts(&|t| [10.0 - 10.0 * t, 10.0, wave(t)]),
        pts(&|t| [0.0, 10.0 - 10.0 * t, -wave(t)]),
    ])
}
#[test]
fn a_blend_between_two_edges_is_g1() {
    let mut s = Session::new();
    // A flat square, and a second one tilted up 30° beyond a gap.
    let a = square(&mut s, 0.0, 0.0);
    let (c, d) = (30f64.to_radians().cos(), 30f64.to_radians().sin());
    let b = handle(
        &s.execute(
            "planesurf",
            &json!({"points": [[16, 0, 2], [16.0 + 10.0 * c, 0, 2.0 + 10.0 * d], [16.0 + 10.0 * c, 10, 2.0 + 10.0 * d], [16, 10, 2]]}),
        )
        .unwrap(),
    );
    let r = s.execute("surfblend", &json!({"first": a.hex(), "first_edge": [10, 5, 0], "second": b.hex(), "second_edge": [16, 5, 2]})).unwrap();
    assert_eq!(r["continuity"], json!("g1"));
    let bl = surface(&s, handle(&r));
    let (sa, sb) = (surface(&s, a), surface(&s, b));
    let n = &bl.nurbs;
    let (v0, v1) = n.domain_v();
    for k in 0..=8 {
        let v = v0 + (v1 - v0) * k as f64 / 8.0;
        // At the first edge: on it, and the same tangent plane as the flat square.
        let p = n.point(0.0, v).unwrap();
        assert!((p.x - 10.0).abs() < 1e-6 && p.z.abs() < 1e-6, "{p:?}");
        let nb = n.normal(0.0, v).unwrap();
        let (u, w) = sa.nurbs.closest(p).unwrap();
        let na = sa.nurbs.normal(u, w).unwrap();
        assert!(nb.cross(na).len() < 1e-3, "G1 at the first edge: {nb:?} vs {na:?}");
        // At the second edge: on the tilted square, its tangent plane too.
        let q = n.point(1.0, v).unwrap();
        let (u, w) = sb.nurbs.closest(q).unwrap();
        assert!(near(sb.nurbs.point(u, w).unwrap(), q, 1e-6), "on the second edge");
        let n2 = n.normal(1.0, v).unwrap();
        assert!(n2.cross(sb.nurbs.normal(u, w).unwrap()).len() < 1e-3, "G1 at the second edge");
    }
    // G0 meets the edges but not their tangent planes.
    let r0 = s
        .execute("surfblend", &json!({"first": a.hex(), "first_edge": [10, 5, 0], "second": b.hex(), "second_edge": [16, 5, 2], "continuity": "g0"}))
        .unwrap();
    let g0 = surface(&s, handle(&r0));
    let n0 = g0.nurbs.normal(0.0, 0.5).unwrap();
    assert!(near(g0.nurbs.point(0.0, 0.5).unwrap(), Vec3::new(10.0, 5.0, 0.0), 1e-6));
    assert!(n0.cross(Vec3::new(0.0, 0.0, 1.0)).len() > 0.05, "a straight blend is not tangent to the flat square");
}

#[test]
fn trimming_then_untrimming_restores_the_surface() {
    let mut s = Session::new();
    let h = square(&mut s, 0.0, 0.0);
    let before = surface(&s, h);
    let r = s.execute("surftrim", &json!({"handle": h.hex(), "curve": [[3, 3, 5], [7, 3, 5], [7, 7, 5], [3, 7, 5]]})).unwrap();
    assert_eq!(r["trims"], json!(1));
    let t = surface(&s, h);
    assert!(t.is_trimmed() && !t.kept(0.5, 0.5) && t.kept(0.1, 0.1));
    let area = t.area(0.001);
    assert!((area - 84.0).abs() < 3.0, "the 4 × 4 hole is cut away: {area}");
    // Keep inside a second loop as well.
    s.execute("surftrim", &json!({"handle": h.hex(), "curve": [[1, 1, 0], [9, 1, 0], [9, 9, 0], [1, 9, 0]], "keep": "inside"})).unwrap();
    assert!(!surface(&s, h).kept(0.05, 0.05));
    assert_eq!(s.execute("surfuntrim", &json!({"handle": h.hex()})).unwrap()["untrimmed"], json!(1));
    assert_eq!(surface(&s, h), before, "untrimmed exactly");
    // Undo takes the untrim back.
    s.undo().unwrap();
    assert!(surface(&s, h).is_trimmed());
}

#[test]
fn interfere_finds_the_overlap_box() {
    let mut s = Session::new();
    let mut boxes = Vec::new();
    for (at, size) in [([0.0, 0.0, 0.0], [4.0, 4.0, 4.0]), ([2.0, 1.0, 3.0], [5.0, 2.0, 6.0]), ([20.0, 0.0, 0.0], [1.0, 1.0, 1.0])] {
        let r = s.execute("box", &json!({"corner": at, "length": size[0], "width": size[1], "height": size[2]})).unwrap();
        let h = r["handles"].as_array().and_then(|a| a.first()).or(r.get("handle")).and_then(Value::as_str).unwrap().to_string();
        boxes.push(h);
    }
    let r = s.execute("interfere", &json!({"first": boxes})).unwrap();
    let pairs = r["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 1, "only the first two overlap: {r}");
    // x 2..4, y 1..3, z 3..4: 2 × 2 × 1.
    assert!((pairs[0]["volume"].as_f64().unwrap() - 4.0).abs() < 1e-9, "{r}");
    let made = Handle::parse_hex(pairs[0]["handle"].as_str().unwrap()).unwrap();
    assert!(matches!(s.doc().unwrap().entity(made).unwrap().kind, EntityKind::Solid3d(_)));
    // The inputs are kept; with create false nothing is made.
    let n = s.doc().unwrap().model.iter().count();
    let r = s.execute("interfere", &json!({"first": [boxes[0]], "second": [boxes[1], boxes[2]], "create": false})).unwrap();
    assert_eq!((r["pairs"].as_array().unwrap().len(), s.doc().unwrap().model.iter().count()), (1, n));
}

#[test]
fn planar_surfaces_patches_offsets_and_extensions() {
    let mut s = Session::new();
    // From a closed polyline: trimmed to it.
    s.execute("pline", &json!({"vertices": [[0, 0], [10, 0], [5, 8]], "closed": true})).unwrap();
    let ph = s.doc().unwrap().model.last().unwrap().handle.hex();
    let tri = made(&mut s, "planesurf", &json!({"object": ph}));
    assert!((tri.area(0.001) - 40.0).abs() < 2.0, "{}", tri.area(0.001));
    // A Coons patch on four edges: a saddle through its corners, the edges on it.
    let edges = saddle_edges();
    let patch = made(&mut s, "surfpatch", &json!({"curves": edges}));
    for (u, v, want) in [(0.0, 0.0, Vec3::ZERO), (1.0, 1.0, Vec3::new(10.0, 10.0, 0.0)), (0.5, 0.0, Vec3::new(5.0, 0.0, 2.0))] {
        assert!(near(patch.nurbs.point(u, v).unwrap(), want, 1e-6), "({u},{v})");
    }
    // Edges that don't meet are refused.
    assert!(
        s.execute("surfpatch", &json!({"curves": [[[0, 0, 0], [1, 0, 0]], [[5, 5, 0], [6, 6, 0]], [[1, 1, 0], [0, 1, 0]], [[0, 1, 0], [0, 0, 0]]]}))
            .is_err()
    );
    // Offset: every point the distance away along the normal.
    let ph = handle(&s.execute("surfpatch", &json!({"curves": edges})).unwrap());
    let off = made(&mut s, "surfoffset", &json!({"handle": ph.hex(), "distance": 1.5}));
    for (u, v) in [(0.3, 0.4), (0.5, 0.5), (0.8, 0.2)] {
        let q = off.nurbs.point(u, v).unwrap();
        let (a, b) = patch.nurbs.closest(q).unwrap();
        let d = (q - patch.nurbs.point(a, b).unwrap()).len();
        assert!((d - 1.5).abs() < 0.02, "{d}");
    }
    let flat = square(&mut s, 0.0, 0.0);
    let up = made(&mut s, "surfoffset", &json!({"handle": flat.hex(), "distance": 2}));
    assert!(near(up.nurbs.point(0.5, 0.5).unwrap(), Vec3::new(5.0, 5.0, 2.0), 1e-9));
    // Extend: the edge at x = 10 moves out 4, the surface stays flat.
    s.execute("surfextend", &json!({"handle": flat.hex(), "edge": [10, 5, 0], "distance": 4})).unwrap();
    let ex = surface(&s, flat);
    let far = ex.nurbs.isolines(4, 8).iter().flatten().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max);
    assert!((far - 14.0).abs() < 1e-6, "{far}");
    assert!(ex.nurbs.control.iter().all(|p| p.z.abs() < 1e-6));
}

#[test]
fn control_vertices_add_remove_rebuild_show() {
    let mut s = Session::new();
    let edges = saddle_edges();
    let h = handle(&s.execute("surfpatch", &json!({"curves": edges})).unwrap());
    let before = surface(&s, h);
    let r = s.execute("cvadd", &json!({"handle": h.hex(), "direction": "u", "at": 0.37})).unwrap();
    assert_eq!(r["count_u"], json!(before.nurbs.count_u + 1));
    let after = surface(&s, h);
    for (u, v) in [(0.1, 0.2), (0.37, 0.5), (0.9, 0.9)] {
        assert!(near(after.nurbs.point(u, v).unwrap(), before.nurbs.point(u, v).unwrap(), 1e-9), "adding a CV keeps the shape");
    }
    let r = s.execute("cvremove", &json!({"handle": h.hex(), "direction": "v"})).unwrap();
    assert_eq!(r["count_v"], json!(before.nurbs.count_v - 1));
    assert!(r["deviation"].as_f64().unwrap() < 0.1, "{r}");
    let r = s.execute("cvrebuild", &json!({"handle": h.hex(), "count_u": 6, "count_v": 6})).unwrap();
    assert_eq!((r["count_u"].clone(), r["count_v"].clone()), (json!(6), json!(6)));
    assert!(r["deviation"].as_f64().unwrap() < 0.1, "{r}");
    assert!(s.execute("cvrebuild", &json!({"handle": h.hex(), "count_u": 1, "count_v": 6})).is_err());
    s.execute("cvshow", &json!({"handle": h.hex()})).unwrap();
    assert!(surface(&s, h).cvs_shown);
    s.execute("cvhide", &json!({"handle": h.hex()})).unwrap();
    assert!(!surface(&s, h).cvs_shown);
}

#[test]
fn faces_meshes_and_boundaries_convert_to_surfaces() {
    let mut s = Session::new();
    let last = |s: &Session| s.doc().unwrap().model.last().unwrap().handle.hex();
    let mut hs = Vec::new();
    if s.execute("3dface", &json!({"points": [[0, 0, 0], [4, 0, 0], [4, 3, 1], [0, 3, 1]]})).is_ok() {
        hs.push(last(&s));
    }
    s.execute("circle", &json!({"center": [20, 0], "radius": 2})).unwrap();
    hs.push(last(&s));
    s.execute("line", &json!({"points": [[0, 0], [1, 1]]})).unwrap();
    hs.push(last(&s));
    let r = s.execute("convtosurface", &json!({"handles": hs})).unwrap();
    assert_eq!(r["skipped"], json!(1), "the line isn't a surface: {r}");
    let circle = Handle::parse_hex(hs[hs.len() - 2].as_str()).unwrap();
    let disk = surface(&s, circle);
    assert!((disk.area(0.001) - std::f64::consts::PI * 4.0).abs() < 0.6, "{}", disk.area(0.001));
}

/// A closed cube mesh becomes a solid with volume 1000 (10 × 10 × 10); an open box is refused.
#[test]
fn convtosolid_cube_mesh_and_open_box_refused() {
    use cadaddy_engine::doc::{Mesh, MeshFace, MeshKind};
    let mut s = Session::new();
    // Build a 10 × 10 × 10 cube mesh (8 vertices, 12 triangular faces).
    let v = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(10.0, 0.0, 0.0),
        Vec3::new(10.0, 10.0, 0.0),
        Vec3::new(0.0, 10.0, 0.0),
        Vec3::new(0.0, 0.0, 10.0),
        Vec3::new(10.0, 0.0, 10.0),
        Vec3::new(10.0, 10.0, 10.0),
        Vec3::new(0.0, 10.0, 10.0),
    ];
    let tri = |a: u32, b: u32, c: u32| MeshFace { v: vec![a, b, c], hidden: 0 };
    let faces = vec![
        // Bottom (z = 0)
        tri(0, 2, 1),
        tri(0, 3, 2),
        // Top (z = 10)
        tri(4, 5, 6),
        tri(4, 6, 7),
        // Front (y = 0)
        tri(0, 1, 5),
        tri(0, 5, 4),
        // Back (y = 10)
        tri(2, 3, 7),
        tri(2, 7, 6),
        // Left (x = 0)
        tri(0, 4, 7),
        tri(0, 7, 3),
        // Right (x = 10)
        tri(1, 2, 6),
        tri(1, 6, 5),
    ];
    let cube = Mesh { vertices: v.clone(), faces, kind: MeshKind::Subd };
    let h = s.add_entity(EntityKind::Mesh(cube)).unwrap();
    let r = s.execute("convtosolid", &json!({"handles": [h.hex()]})).unwrap();
    assert_eq!(r["converted"].as_array().unwrap().len(), 1);
    // Check volume.
    let solid = match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Solid3d(so) => (**so).clone(),
        k => panic!("expected solid, got {k:?}"),
    };
    let props = solid.props(0.01);
    assert!((props.volume - 1000.0).abs() < 1.0, "volume should be 1000, got {}", props.volume);
    // Undo restores the mesh.
    s.undo().unwrap();
    assert!(matches!(s.doc().unwrap().entity(h).unwrap().kind, EntityKind::Mesh(_)));

    // An open box (missing the top face) is refused.
    let open_faces = vec![
        tri(0, 2, 1),
        tri(0, 3, 2), // bottom
        tri(0, 1, 5),
        tri(0, 5, 4), // front
        tri(2, 3, 7),
        tri(2, 7, 6), // back
        tri(0, 4, 7),
        tri(0, 7, 3), // left
        tri(1, 2, 6),
        tri(1, 6, 5), // right
                      // top is missing
    ];
    let open = Mesh { vertices: v, faces: open_faces, kind: MeshKind::Subd };
    let h2 = s.add_entity(EntityKind::Mesh(open)).unwrap();
    let e = s.execute("convtosolid", &json!({"handles": [h2.hex()]}));
    assert!(e.is_err(), "an open box should be refused");
}

/// A flat sheet through four corners (PLANESURF `points`, so it can stand at any angle).
fn sheet(s: &mut Session, pts: [[f64; 3]; 4]) -> Handle {
    handle(&s.execute("planesurf", &json!({"points": pts})).unwrap())
}

/// Checks a fillet against the analytic one: every sample at `r` (± 1e-3) from the centre line
/// (through `centre`, along x), the arc on the corner's side of it, one boundary row on each
/// plane (within 1e-6) at its contact line, and the surface tangent to each plane there (< 0.5°).
/// Planes are given by a point on the line where they meet (the origin), their unit normal and
/// the unit direction from that line into the sheet.
fn assert_true_fillet(fillet: &Surface3d, r: f64, centre: Vec3, planes: [(Vec3, Vec3); 2], span: (f64, f64)) {
    let n = &fillet.nurbs;
    let (u0, u1) = n.domain_u();
    let (v0, v1) = n.domain_v();
    let off_axis = |p: Vec3| ((p.y - centre.y).powi(2) + (p.z - centre.z).powi(2)).sqrt();
    let corner_side = Vec3::new(0.0, -centre.y, -centre.z).normalized();
    for i in 0..=20 {
        for j in 0..=4 {
            let p = n.point(u0 + (u1 - u0) * i as f64 / 20.0, v0 + (v1 - v0) * j as f64 / 4.0).unwrap();
            assert!((off_axis(p) - r).abs() <= 1e-3, "{p:?} is {} from the centre line, not {r}", off_axis(p));
            assert!((p - Vec3::new(p.x, centre.y, centre.z)).dot(corner_side) >= -1e-9, "{p:?} is on the far side of the ball");
            assert!(p.x >= span.0 - 1e-9 && p.x <= span.1 + 1e-9, "{p:?} outside x {span:?}");
        }
    }
    // Which boundary row touches which plane: the first row is on one of them.
    let first = n.point(u0, v0).unwrap();
    let on_plane = |p: Vec3, (nrm, _): (Vec3, Vec3)| p.dot(nrm).abs() <= 1e-6;
    let (row_a, row_b) = if on_plane(first, planes[0]) { (u0, u1) } else { (u1, u0) };
    for (u, (nrm, into)) in [(row_a, planes[0]), (row_b, planes[1])] {
        // The contact line: the centre's foot on the plane.
        let foot = centre - nrm * centre.dot(nrm);
        for j in 0..=4 {
            let v = v0 + (v1 - v0) * j as f64 / 4.0;
            let p = n.point(u, v).unwrap();
            assert!(p.dot(nrm).abs() <= 1e-6, "boundary {p:?} is off its plane by {}", p.dot(nrm));
            assert!(((p - foot).dot(into)).abs() <= 1e-6, "boundary {p:?} is not on the contact line through {foot:?}");
            let sn = n.normal(u, v).unwrap();
            let angle = sn.dot(nrm).abs().min(1.0).acos().to_degrees();
            assert!(angle < 0.5, "not tangent at {p:?}: {angle}° between normals");
        }
    }
}

/// A fillet between two perpendicular sheets sharing the x-axis is a quarter cylinder of the asked
/// radius about (x, 2, 2), tangent to both sheets along y = 2 and z = 2.
#[test]
fn surffillet_between_perpendicular_planes() {
    let mut s = Session::new();
    let a = sheet(&mut s, [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]]);
    let b = sheet(&mut s, [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 10.0], [0.0, 0.0, 10.0]]);
    let r = s
        .execute("surffillet", &json!({"first": a.hex(), "second": b.hex(), "radius": 2.0, "first_edge": [5, 0, 0], "second_edge": [5, 0, 0]}))
        .unwrap();
    assert!(r.get("trimmed").is_none(), "nothing was trimmed: {r}");
    let fh = handle(&r);
    let fillet = surface(&s, fh);
    assert!(fillet.is_valid());
    let z = Vec3::new(0.0, 0.0, 1.0);
    let y = Vec3::new(0.0, 1.0, 0.0);
    assert_true_fillet(&fillet, 2.0, Vec3::new(0.0, 2.0, 2.0), [(z, y), (y, z)], (0.0, 10.0));
    // The sheets are untouched without TRIM.
    assert_eq!(surface(&s, a).nurbs.control.len(), 4);
    assert!(surface(&s, a).nurbs.control.iter().any(|q| near(*q, Vec3::new(0.0, 0.0, 0.0), 1e-12)));
    // Undo removes the fillet.
    s.undo().unwrap();
    assert!(s.doc().unwrap().entity(fh).is_none());
}

/// Two sheets 120° apart: the arc turns through 60°, radius 2 about (x, 2/√3, 2), tangent to both.
#[test]
fn surffillet_between_sheets_at_120_degrees() {
    let mut s = Session::new();
    let (c, sn) = ((120.0_f64).to_radians().cos(), (120.0_f64).to_radians().sin());
    let a = sheet(&mut s, [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]]);
    let b = sheet(&mut s, [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0 * c, 10.0 * sn], [0.0, 10.0 * c, 10.0 * sn]]);
    let fh = handle(&s.execute("surffillet", &json!({"first": a.hex(), "second": b.hex(), "radius": 2.0})).unwrap());
    let fillet = surface(&s, fh);
    // The centre is on the bisector, r / sin(60°) from the corner.
    let into_b = Vec3::new(0.0, c, sn);
    let bisector = (Vec3::new(0.0, 1.0, 0.0) + into_b).normalized();
    let centre = bisector * (2.0 / (60.0_f64).to_radians().sin());
    assert!(near(centre, Vec3::new(0.0, 2.0 / 3f64.sqrt(), 2.0), 1e-12));
    let nb = Vec3::new(0.0, -sn, c); // perpendicular to into_b and x
    assert_true_fillet(&fillet, 2.0, centre, [(Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 1.0, 0.0)), (nb, into_b)], (0.0, 10.0));
}

/// TRIM cuts both sheets back to the contact lines (y ≥ 2 and z ≥ 2), in the same undo step.
#[test]
fn surffillet_trim_cuts_both_sheets_to_the_contact_lines() {
    let mut s = Session::new();
    let a = sheet(&mut s, [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]]);
    let b = sheet(&mut s, [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 10.0], [0.0, 0.0, 10.0]]);
    let (before_a, before_b) = (surface(&s, a), surface(&s, b));
    let r = s.execute("surffillet", &json!({"first": a.hex(), "second": b.hex(), "radius": 2.0, "trim": true})).unwrap();
    assert_eq!(r["trimmed"], json!([a.hex(), b.hex()]), "{r}");
    // The kept part of each sheet, sampled.
    let kept = |sf: &Surface3d| -> Vec<Vec3> {
        let n = &sf.nurbs;
        let (u0, u1) = n.domain_u();
        let (v0, v1) = n.domain_v();
        (0..=20)
            .flat_map(|i| (0..=20).map(move |j| (u0 + (u1 - u0) * i as f64 / 20.0, v0 + (v1 - v0) * j as f64 / 20.0)))
            .filter(|(u, v)| sf.kept(*u, *v))
            .map(|(u, v)| n.point(u, v).unwrap())
            .collect()
    };
    let range = |pts: &[Vec3], f: fn(&Vec3) -> f64| pts.iter().map(f).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| (lo.min(x), hi.max(x)));
    let ka = kept(&surface(&s, a));
    let kb = kept(&surface(&s, b));
    let (ya, za, xa) = (range(&ka, |p| p.y), range(&ka, |p| p.z), range(&ka, |p| p.x));
    assert!((ya.0 - 2.0).abs() < 1e-6 && (ya.1 - 10.0).abs() < 1e-6, "first sheet y {ya:?}");
    assert!(za.0.abs() < 1e-9 && za.1.abs() < 1e-9, "first sheet left its plane: z {za:?}");
    assert!(xa.0.abs() < 1e-6 && (xa.1 - 10.0).abs() < 1e-6, "first sheet x {xa:?}");
    let (zb, yb) = (range(&kb, |p| p.z), range(&kb, |p| p.y));
    assert!((zb.0 - 2.0).abs() < 1e-6 && (zb.1 - 10.0).abs() < 1e-6, "second sheet z {zb:?}");
    assert!(yb.0.abs() < 1e-9 && yb.1.abs() < 1e-9, "second sheet left its plane: y {yb:?}");
    // The fillet's edges meet the trimmed sheets' edges.
    let fillet = surface(&s, handle(&r));
    let (u0, u1) = fillet.nurbs.domain_u();
    for u in [u0, u1] {
        let p = fillet.nurbs.point(u, 0.5).unwrap();
        assert!(near(p, Vec3::new(5.0, 2.0, 0.0), 1e-9) || near(p, Vec3::new(5.0, 0.0, 2.0), 1e-9), "{p:?}");
    }
    // One undo takes back the fillet and both trims.
    let fh = handle(&r);
    s.undo().unwrap();
    assert!(s.doc().unwrap().entity(fh).is_none());
    assert_eq!(surface(&s, a), before_a);
    assert_eq!(surface(&s, b), before_b);
}

/// A radius the sheets can't hold is refused with the largest that fits; curved and parallel
/// surfaces are refused with what to do instead; nothing is made.
#[test]
fn surffillet_refuses_what_it_cant_make() {
    let mut s = Session::new();
    let a = sheet(&mut s, [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]]);
    let b = sheet(&mut s, [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 0.0, 10.0], [0.0, 0.0, 10.0]]);
    let count = |s: &Session| s.doc().unwrap().model.iter().count();
    let n = count(&s);
    let e = s.execute("surffillet", &json!({"first": a.hex(), "second": b.hex(), "radius": 12.0, "trim": true})).unwrap_err().to_string();
    assert!(e.contains("too large") && e.contains("at most 10.0000"), "{e}");
    let flat = square(&mut s, 20.0, 5.0);
    let e = s.execute("surffillet", &json!({"first": a.hex(), "second": flat.hex(), "radius": 1.0})).unwrap_err().to_string();
    assert!(e.contains("parallel"), "{e}");
    let saddle = handle(&s.execute("surfpatch", &json!({"curves": saddle_edges()})).unwrap());
    let e = s.execute("surffillet", &json!({"first": saddle.hex(), "second": b.hex(), "radius": 1.0})).unwrap_err().to_string();
    assert!(e.contains("curved") && e.contains("SURFBLEND"), "{e}");
    assert_eq!(count(&s), n + 2, "only the two test surfaces were added");
    assert_eq!(surface(&s, a).nurbs.control.len(), 4);
}

/// A closed 10-cube whose top face is wound the wrong way is refused, naming the flipped face.
#[test]
fn convtosolid_refuses_a_cube_with_a_flipped_face() {
    use cadaddy_engine::doc::{Mesh, MeshFace, MeshKind};
    let mut s = Session::new();
    let v = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(10.0, 0.0, 0.0),
        Vec3::new(10.0, 10.0, 0.0),
        Vec3::new(0.0, 10.0, 0.0),
        Vec3::new(0.0, 0.0, 10.0),
        Vec3::new(10.0, 0.0, 10.0),
        Vec3::new(10.0, 10.0, 10.0),
        Vec3::new(0.0, 10.0, 10.0),
    ];
    let quad = |a: u32, b: u32, c: u32, d: u32| MeshFace { v: vec![a, b, c, d], hidden: 0 };
    let mut faces = vec![quad(0, 3, 2, 1), quad(4, 5, 6, 7), quad(0, 1, 5, 4), quad(2, 3, 7, 6), quad(0, 4, 7, 3), quad(1, 2, 6, 5)];
    // Consistent: a solid of volume 1000.
    let good = s.add_entity(EntityKind::Mesh(Mesh { vertices: v.clone(), faces: faces.clone(), kind: MeshKind::Subd })).unwrap();
    s.execute("convtosolid", &json!({"handles": [good.hex()]})).unwrap();
    match &s.doc().unwrap().entity(good).unwrap().kind {
        EntityKind::Solid3d(so) => assert!((so.props(0.01).volume - 1000.0).abs() < 1.0),
        k => panic!("expected a solid, got {k:?}"),
    }
    // The top face turned over: every edge round it now runs the same way in both faces.
    faces[1] = quad(4, 7, 6, 5);
    let bad = s.add_entity(EntityKind::Mesh(Mesh { vertices: v, faces, kind: MeshKind::Subd })).unwrap();
    let e = s.execute("convtosolid", &json!({"handles": [bad.hex()]})).unwrap_err().to_string();
    assert!(e.contains("4 edges") && e.contains("flipped"), "{e}");
    assert!(matches!(s.doc().unwrap().entity(bad).unwrap().kind, EntityKind::Mesh(_)), "the mesh stays a mesh");
}

/// From the menu, SURFOFFSET asks for the surface and the distance (no parameters needed).
#[test]
fn surfoffset_prompts_when_started() {
    let mut s = Session::new();
    let h = square(&mut s, 0.0, 0.0);
    let n = s.doc().unwrap().model.iter().count();
    s.start("surfoffset").unwrap();
    assert!(s.prompt_text().contains("Select surface"), "{}", s.prompt_text());
    s.input(cadaddy_engine::Input::Pick(vec![h])).unwrap();
    assert!(s.prompt_text().contains("offset distance"), "{}", s.prompt_text());
    s.input(cadaddy_engine::Input::Text("3".into())).unwrap();
    assert_eq!(s.doc().unwrap().model.iter().count(), n + 1);
    let made = s.doc().unwrap().model.last().unwrap().handle;
    assert!(near(surface(&s, made).nurbs.point(0.5, 0.5).unwrap(), Vec3::new(5.0, 5.0, 3.0), 1e-9));
}
