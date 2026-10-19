//! WALLGEN: a 2D plan built as model-space 3D objects (walls, door leaves, windows, floor slabs,
//! ceilings) by Walk mode's rules, kept associative with the plan.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line, LwPolyline, Mesh};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// Feet → a line in inches on A-WALL.
fn line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
}

fn rules() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "kind": "wall", "skip_curves": true}]
    })
}

/// A 20' × 10' room of 6" walls.
fn room() -> Session {
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))] {
        line(&mut d, a, b);
    }
    for (a, b) in [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))] {
        line(&mut d, a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    s
}

fn made(s: &Session, part: &str) -> Vec<Mesh> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some(part))
        .filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some(m.clone()) } else { None })
        .collect()
}

fn z_range(m: &Mesh) -> (f64, f64) {
    m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.z), hi.max(v.z)))
}

/// Area of the faces lying flat at height `z`.
fn flat_area(m: &Mesh, z: f64) -> f64 {
    m.faces
        .iter()
        .filter(|f| f.v.iter().all(|i| m.vertices.get(*i as usize).is_some_and(|v| (v.z - z).abs() < 1e-3)))
        .map(|f| {
            let pts: Vec<&Vec3> = f.v.iter().filter_map(|i| m.vertices.get(*i as usize)).collect();
            let mut a = 0.0;
            for k in 0..pts.len() {
                let (p, q) = (pts[k], pts[(k + 1) % pts.len()]);
                a += p.x * q.y - q.x * p.y;
            }
            a.abs() / 2.0
        })
        .sum()
}

#[test]
fn a_room_becomes_walls_a_floor_slab_and_a_ceiling() {
    let mut s = room();
    let r = s.execute("wallgen", &json!({"rules": rules()})).unwrap();
    assert_eq!(
        (r["walls"].clone(), r["floors"].clone(), r["ceilings"].clone(), r["levels"].clone()),
        (json!(8), json!(1), json!(1), json!(1)),
        "{r}"
    );
    // Walls stand from the floor to just under the next floor (9.5' less Walk mode's 0.1' gap for
    // its structure: 112.8").
    for w in made(&s, "wall") {
        let (lo, hi) = z_range(&w);
        assert!(lo.abs() < 1e-3 && (hi - 112.8).abs() < 1e-3, "{lo} {hi}");
    }
    // The floor slab: Walk mode's floor (the walls' extents padded 2' all round) 6" thick.
    let floor = made(&s, "floor");
    let f = floor.first().unwrap();
    let (lo, hi) = z_range(f);
    assert!((lo + 6.0).abs() < 1e-3 && hi.abs() < 1e-3, "{lo} {hi}");
    assert!((flat_area(f, 0.0) - 24.0 * 14.0 * 144.0).abs() < 0.5, "{}", flat_area(f, 0.0));
    // The ceiling at 8'.
    let c = made(&s, "ceiling");
    let (lo, hi) = c.first().map(z_range).unwrap();
    assert!((lo - 96.0).abs() < 1e-3 && (hi - 96.0).abs() < 1e-3);
    // On their own layers.
    let layers: Vec<String> = s.doc().unwrap().model.iter().filter(|e| made_as(e).is_some()).map(|e| e.common.layer.clone()).collect();
    assert!(layers.iter().all(|l| l.starts_with("3D-")), "{layers:?}");
    // Again: the old objects are replaced, not doubled.
    let again = s.execute("wallgen", &json!({})).unwrap();
    assert_eq!((again["walls"].clone(), again["removed"].clone()), (json!(8), json!(10)));
}

#[test]
fn doors_and_windows_take_their_opening_sizes() {
    // Two rooms: a 4' window gap in the south wall (x 8–12) and a 3' doorway in the middle wall
    // (y 3–6) with its swing.
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0., 0.), (8., 0.)), ((12., 0.), (40., 0.)), ((40., 0.), (40., 10.)), ((40., 10.), (0., 10.)), ((0., 10.), (0., 0.))] {
        line(&mut d, a, b);
    }
    for (a, b) in
        [((0.5, 0.5), (8., 0.5)), ((12., 0.5), (39.5, 0.5)), ((39.5, 0.5), (39.5, 9.5)), ((39.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))]
    {
        line(&mut d, a, b);
    }
    for x in [20.0, 20.5] {
        line(&mut d, (x, 0.5), (x, 3.0));
        line(&mut d, (x, 6.0), (x, 9.5));
    }
    let swing: Vec<_> = (0..=9)
        .map(|i| f64::from(i) * std::f64::consts::FRAC_PI_2 / 9.0)
        .map(|a| json!({"p": {"x": (20.0 - 3.0 * a.sin()) * 12.0, "y": (3.0 + 3.0 * a.cos()) * 12.0}, "bulge": 0.0, "start_width": 0.0, "end_width": 0.0}))
        .collect();
    let pl: LwPolyline = serde_json::from_value(json!({"vertices": swing, "closed": false})).unwrap();
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::LwPolyline(pl));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    let r = s.execute("wallgen", &json!({"rules": rules()})).unwrap();
    assert_eq!((r["doors"].clone(), r["windows"].clone()), (json!(1), json!(1)), "{r}");
    // The window: glass from the 2' sill to the 7' head across the 4' gap.
    let glass = made(&s, "window");
    let g = glass.first().unwrap();
    let (lo, hi) = z_range(g);
    assert!((lo - 24.0).abs() < 1e-3 && (hi - 84.0).abs() < 1e-3, "{lo} {hi}");
    let xs = g.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.x), hi.max(v.x)));
    assert!((xs.0 - 96.0).abs() < 1e-3 && (xs.1 - 144.0).abs() < 1e-3, "{xs:?}");
    // The door leaf: the 7' head high, standing open.
    let door = made(&s, "door");
    assert!((z_range(door.first().unwrap()).1 - 84.0).abs() < 1e-3);
}

#[test]
fn the_building_follows_the_plan_while_associative() {
    let mut s = room();
    s.execute("wallgen", &json!({"rules": rules()})).unwrap();
    let wall_x = |s: &Session| made(s, "wall").iter().flat_map(|m| m.vertices.iter().map(|v| v.x)).fold(f64::NEG_INFINITY, f64::max);
    assert!((wall_x(&s) - 240.0).abs() < 1e-6);
    // Stretch the room east by 2': both east wall lines move.
    let east: Vec<String> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| matches!(&e.kind, EntityKind::Line(l) if l.a.x > 200.0 && l.b.x > 200.0))
        .map(|e| e.handle.hex())
        .collect();
    assert_eq!(east.len(), 2);
    s.execute("move", &json!({"handles": east, "from": [0, 0], "to": [24, 0]})).unwrap();
    assert!((wall_x(&s) - 264.0).abs() < 1e-6, "rebuilt with the plan: {}", wall_x(&s));
    // One undo takes back the move and the rebuild together.
    s.undo().unwrap();
    assert!((wall_x(&s) - 240.0).abs() < 1e-6);
    // Not associative: the plan moves alone.
    s.execute("wallgen", &json!({"auto": false})).unwrap();
    let east: Vec<String> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).is_none() && matches!(&e.kind, EntityKind::Line(l) if l.a.x > 200.0))
        .map(|e| e.handle.hex())
        .collect();
    s.execute("move", &json!({"handles": east, "from": [0, 0], "to": [24, 0]})).unwrap();
    assert!((wall_x(&s) - 240.0).abs() < 1e-6);
    // Clear removes everything WALLGEN made.
    let r = s.execute("wallgen", &json!({"clear": true})).unwrap();
    assert_eq!(r["removed"], json!(10));
    assert!(s.doc().unwrap().model.iter().all(|e| made_as(e).is_none()));
}

#[test]
fn rules_files_saving_and_bad_input() {
    let dir = std::env::temp_dir().join(format!("cadaddy-wallgen-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("Plan.walk.json");
    std::fs::write(&f, rules().to_string()).unwrap();
    let mut s = room();
    let r = s.execute("wallgen", &json!({"rulesFile": f.to_string_lossy()})).unwrap();
    assert_eq!(r["walls"], json!(8));
    // The tags survive DXF: a reopened drawing still knows what WALLGEN made.
    let d = s.doc().unwrap().clone();
    let back = cadaddy_io::read(&cadaddy_io::write(&d, "p.dxf").unwrap(), "p.dxf").unwrap();
    assert_eq!(back.model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).count(), 8);
    // Without rules: Walk mode's defaults (layers named *WALL*) find the same walls, and skip
    // what WALLGEN made (its 3D-Walls layer matches the pattern too).
    let mut t = room();
    assert_eq!(t.execute("wallgen", &json!({})).unwrap()["walls"], json!(8));
    assert_eq!(t.execute("wallgen", &json!({})).unwrap()["walls"], json!(8));
    // Errors, not panics.
    assert!(s.execute("wallgen", &json!({"rules": {"levels": "x"}})).is_err());
    assert!(s.execute("wallgen", &json!({"rulesFile": dir.join("missing.json").to_string_lossy()})).is_err());
    assert!(s.execute("wallgen", &json!({"slabThickness": -1})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
