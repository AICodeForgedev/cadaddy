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

#[test]
fn not_yet_available_commands_say_so() {
    let mut s = Session::new();
    let e = s.execute("surffillet", &json!({})).unwrap_err().to_string();
    assert!(e.contains("not available yet"), "{e}");
    let e = s.execute("convtosolid", &json!({})).unwrap_err().to_string();
    assert!(e.contains("not available yet"), "{e}");
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
