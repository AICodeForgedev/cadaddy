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
        (json!(4), json!(1), json!(1), json!(1)),
        "{r}: one wall per face pair, not one per line"
    );
    // Walls stand from the floor to just under the next floor (9.5' less Walk mode's 0.1' gap for
    // its structure: 112.8").
    for w in made(&s, "wall") {
        let (lo, hi) = z_range(&w);
        assert!(lo.abs() < 1e-3 && (hi - 112.8).abs() < 1e-3, "{lo} {hi}");
    }
    // The floor slab: the walls' extents (to their outside faces) 6" thick.
    let floor = made(&s, "floor");
    let f = floor.first().unwrap();
    let (lo, hi) = z_range(f);
    assert!((lo + 6.0).abs() < 1e-3 && hi.abs() < 1e-3, "{lo} {hi}");
    assert!((flat_area(f, 0.0) - 20.0 * 10.0 * 144.0).abs() < 0.5, "{}", flat_area(f, 0.0));
    // The ceiling at 8'.
    let c = made(&s, "ceiling");
    let (lo, hi) = c.first().map(z_range).unwrap();
    assert!((lo - 96.0).abs() < 1e-3 && (hi - 96.0).abs() < 1e-3);
    // On their own layers.
    let layers: Vec<String> = s.doc().unwrap().model.iter().filter(|e| made_as(e).is_some()).map(|e| e.common.layer.clone()).collect();
    assert!(layers.iter().all(|l| l.starts_with("3D-")), "{layers:?}");
    // Again: the old objects are replaced, not doubled.
    let again = s.execute("wallgen", &json!({})).unwrap();
    assert_eq!((again["walls"].clone(), again["removed"].clone()), (json!(4), json!(6)));
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
    assert_eq!(r["removed"], json!(6));
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
    assert_eq!(r["walls"], json!(4));
    // The tags survive DXF: a reopened drawing still knows what WALLGEN made.
    let d = s.doc().unwrap().clone();
    let back = cadaddy_io::read(&cadaddy_io::write(&d, "p.dxf").unwrap(), "p.dxf").unwrap();
    assert_eq!(back.model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).count(), 4);
    // Without rules: Walk mode's defaults (layers named *WALL*) find the same walls, and skip
    // what WALLGEN made (its 3D-Walls layer matches the pattern too).
    let mut t = room();
    assert_eq!(t.execute("wallgen", &json!({})).unwrap()["walls"], json!(4));
    assert_eq!(t.execute("wallgen", &json!({})).unwrap()["walls"], json!(4));
    // Errors, not panics.
    assert!(s.execute("wallgen", &json!({"rules": {"levels": "x"}})).is_err());
    assert!(s.execute("wallgen", &json!({"rulesFile": dir.join("missing.json").to_string_lossy()})).is_err());
    assert!(s.execute("wallgen", &json!({"slabThickness": -1})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The tester's plan: a room of two 6" rectangles (closed polylines) with a door drawn over the
/// south wall — an arc and its leaf line on a DOOR layer, the wall lines unbroken.
#[test]
fn a_door_drawn_over_a_whole_wall_cuts_it() {
    let mut d = Drawing::new_imperial();
    let rect = |x0: f64, y0: f64, x1: f64, y1: f64| -> LwPolyline {
        let v = |x: f64, y: f64| json!({"p": {"x": x * 12.0, "y": y * 12.0}, "bulge": 0.0, "start_width": 0.0, "end_width": 0.0});
        serde_json::from_value(json!({"vertices": [v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y1)], "closed": true})).unwrap()
    };
    for r in [rect(0.0, 0.0, 20.0, 10.0), rect(0.5, 0.5, 19.5, 9.5)] {
        let h = Handle(d.handseed.max(0x100));
        d.handseed = h.0 + 1;
        let mut e = Entity::new(h, EntityKind::LwPolyline(r));
        e.common.layer = "A-WALL".into();
        d.model.push(e);
    }
    // A 3' door hinged at (8', 0.5') swinging into the room: the arc from the closed position
    // (along the wall, to 11') up to open (8', 3.5'), and the open leaf.
    let mut add = |kind: EntityKind| {
        let h = Handle(d.handseed.max(0x100));
        d.handseed = h.0 + 1;
        let mut e = Entity::new(h, kind);
        e.common.layer = "A-DOOR".into();
        d.model.push(e);
    };
    add(EntityKind::Arc(cadaddy_engine::doc::Arc {
        center: Vec3 { x: 96.0, y: 6.0, z: 0.0 },
        radius: 36.0,
        start: 0.0,
        end: std::f64::consts::FRAC_PI_2,
    }));
    add(EntityKind::Line(Line { a: Vec3 { x: 96.0, y: 6.0, z: 0.0 }, b: Vec3 { x: 96.0, y: 42.0, z: 0.0 } }));
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    let r = s.execute("wallgen", &json!({"rules": rules()})).unwrap();
    assert_eq!((r["walls"].clone(), r["doors"].clone()), (json!(4), json!(1)), "{r}");
    // The south wall is open below the 7' head between x 8' and 11', and closed beside it.
    let walls = made(&s, "wall");
    let south = walls.iter().find(|m| m.vertices.iter().all(|v| v.y <= 6.0 + 1e-6)).expect("a south wall");
    let solid_at = |x: f64, z: f64| {
        // Inside one of the wall's boxes (each 8 vertices).
        south.vertices.chunks(8).any(|b| {
            let (lo, hi) =
                b.iter().fold((Vec3 { x: f64::MAX, y: f64::MAX, z: f64::MAX }, Vec3 { x: f64::MIN, y: f64::MIN, z: f64::MIN }), |(lo, hi), v| {
                    (Vec3 { x: lo.x.min(v.x), y: lo.y.min(v.y), z: lo.z.min(v.z) }, Vec3 { x: hi.x.max(v.x), y: hi.y.max(v.y), z: hi.z.max(v.z) })
                });
            x > lo.x && x < hi.x && z > lo.z && z < hi.z
        })
    };
    assert!(!solid_at(114.0, 40.0), "the doorway is open");
    assert!(solid_at(114.0, 100.0), "the header above it");
    assert!(solid_at(60.0, 40.0) && solid_at(200.0, 40.0), "the wall beside it");
    // The corners are closed: the south wall runs the full 20'.
    let xs = south.vertices.iter().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v.x), hi.max(v.x)));
    assert!(xs.0 <= 1e-6 && xs.1 >= 240.0 - 1e-6, "{xs:?}");
    // The leaf stands open, the 7' head high.
    let leaf = made(&s, "door");
    assert_eq!(leaf.len(), 1);
    assert!((z_range(leaf.first().unwrap()).1 - 84.0).abs() < 1e-6);
}

/// A plan with no wall layer says why WALLGEN built no walls, and where the walls seem to be;
/// `layers` builds them from there.
#[test]
fn no_wall_layers_is_reported_and_layers_picks_them() {
    let mut d = Drawing::new_imperial();
    for y in [0.0, 6.0] {
        let h = Handle(d.handseed.max(0x100));
        d.handseed = h.0 + 1;
        let mut e = Entity::new(h, EntityKind::Line(Line { a: Vec3 { x: 0.0, y, z: 0.0 }, b: Vec3 { x: 240.0, y, z: 0.0 } }));
        e.common.layer = "PARTITIONS".into();
        d.model.push(e);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    let r = s.execute("wallgen", &json!({"rules": rules()})).unwrap();
    assert_eq!(r["walls"], json!(0));
    let notes = r["notes"].to_string();
    assert!(notes.contains("no objects on the wall layers (A-WALL)") && notes.contains("PARTITIONS"), "{notes}");
    let r = s.execute("wallgen", &json!({"rules": rules(), "layers": "PART*"})).unwrap();
    assert_eq!(r["walls"], json!(1), "{r}");
}

/// A line in inches on `layer`.
fn line_in(d: &mut Drawing, layer: &str, a: (f64, f64), b: (f64, f64)) {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: Vec3 { x: a.0, y: a.1, z: 0.0 }, b: Vec3 { x: b.0, y: b.1, z: 0.0 } }));
    e.common.layer = layer.into();
    d.model.push(e);
}

/// A made-up shop plan like the tester's: a 40' × 30' outline of 6" walls on A-WALL and a 6"
/// partition at x 24'–24'6", the lines broken at four 3'-0" gaps closed by jamb lines. Three
/// gaps hold a 3' door block (DOOR36: a leaf and a 90° swing, its base point at the hinge and
/// drawn away from the origin) inserted at the hinge on the inside face; the fourth, in the north
/// wall, is a plain gap.
fn shop() -> Session {
    let mut d = Drawing::new_imperial();
    let w = |d: &mut Drawing, a: (f64, f64), b: (f64, f64)| line_in(d, "A-WALL", a, b);
    // South wall, door gap x 96–132.
    for (a, b) in [((0., 0.), (96., 0.)), ((132., 0.), (480., 0.)), ((6., 6.), (96., 6.)), ((132., 6.), (474., 6.))] {
        w(&mut d, a, b);
    }
    // East wall, door gap y 120–156.
    for (a, b) in [((480., 0.), (480., 120.)), ((480., 156.), (480., 360.)), ((474., 6.), (474., 120.)), ((474., 156.), (474., 354.))] {
        w(&mut d, a, b);
    }
    // North wall, plain gap x 360–396.
    for (a, b) in [((480., 360.), (396., 360.)), ((360., 360.), (0., 360.)), ((474., 354.), (396., 354.)), ((360., 354.), (6., 354.))] {
        w(&mut d, a, b);
    }
    // West wall, unbroken.
    w(&mut d, (0., 360.), (0., 0.));
    w(&mut d, (6., 354.), (6., 6.));
    // The partition, door gap y 240–276.
    for (a, b) in [((288., 6.), (288., 240.)), ((288., 276.), (288., 354.)), ((294., 6.), (294., 240.)), ((294., 276.), (294., 354.))] {
        w(&mut d, a, b);
    }
    // Jamb lines across every gap.
    for (a, b) in [
        ((96., 0.), (96., 6.)),
        ((132., 0.), (132., 6.)),
        ((474., 120.), (480., 120.)),
        ((474., 156.), (480., 156.)),
        ((360., 354.), (360., 360.)),
        ((396., 354.), (396., 360.)),
        ((288., 240.), (294., 240.)),
        ((288., 276.), (294., 276.)),
    ] {
        w(&mut d, a, b);
    }
    // DOOR36: hinge at the base point (1200, 0), closed along +x, open leaf along +y.
    let mut entities = cadaddy_engine::doc::EntityStore::new();
    entities
        .push(Entity::new(Handle(0x50), EntityKind::Line(Line { a: Vec3 { x: 1200.0, y: 0.0, z: 0.0 }, b: Vec3 { x: 1200.0, y: 36.0, z: 0.0 } })));
    entities.push(Entity::new(
        Handle(0x51),
        EntityKind::Arc(cadaddy_engine::doc::Arc {
            center: Vec3 { x: 1200.0, y: 0.0, z: 0.0 },
            radius: 36.0,
            start: 0.0,
            end: std::f64::consts::FRAC_PI_2,
        }),
    ));
    let block = cadaddy_engine::doc::Block {
        xref_overlay: false,
        xref_status: Default::default(),
        xref_reason: String::new(),
        name: "DOOR36".into(),
        annotative: false,
        base: Vec3 { x: 1200.0, y: 0.0, z: 0.0 },
        entities,
        description: String::new(),
        anonymous: false,
        xref_path: None,
        explodable: true,
        units: 0,
        array: None,
    };
    d.blocks.insert("DOOR36".into(), std::sync::Arc::new(block));
    for (x, y, deg) in [(96.0, 6.0, 0.0), (294.0, 240.0, 90.0), (474.0, 120.0, 90.0)] {
        let ins: cadaddy_engine::doc::Insert =
            serde_json::from_value(json!({"block": "DOOR36", "insert": {"x": x, "y": y, "z": 0.0}, "rotation": f64::to_radians(deg)})).unwrap();
        let h = Handle(d.handseed.max(0x100));
        d.handseed = h.0 + 1;
        let mut e = Entity::new(h, EntityKind::Insert(ins));
        e.common.layer = "A-DOOR".into();
        d.model.push(e);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Shop", None);
    s
}

/// The plan box of a mesh: (x0, y0, x1, y1).
fn plan_box(m: &Mesh) -> (f64, f64, f64, f64) {
    m.vertices.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |(x0, y0, x1, y1), v| (x0.min(v.x), y0.min(v.y), x1.max(v.x), y1.max(v.y)))
}

/// Whether any wall box holds the point (each box is 8 vertices).
fn wall_at(walls: &[Mesh], x: f64, y: f64, z: f64) -> bool {
    walls.iter().any(|m| {
        m.vertices.chunks(8).any(|b| {
            let lo = b.iter().fold((f64::MAX, f64::MAX, f64::MAX), |a, v| (a.0.min(v.x), a.1.min(v.y), a.2.min(v.z)));
            let hi = b.iter().fold((f64::MIN, f64::MIN, f64::MIN), |a, v| (a.0.max(v.x), a.1.max(v.y), a.2.max(v.z)));
            x > lo.0 && x < hi.0 && y > lo.1 && y < hi.1 && z > lo.2 && z < hi.2
        })
    })
}

/// The tester's door openings: the walls stop at each gap (a header above the 7' head), each door
/// block's leaf hangs from its hinge in its own opening, a gap with a door is never a window, the
/// plain gap in the outside wall is the one window, and the floor and ceiling end at the walls'
/// outside faces.
#[test]
fn door_blocks_sit_in_their_openings() {
    let mut s = shop();
    let r = s.execute("wallgen", &json!({"rules": rules()})).unwrap();
    assert_eq!((r["doors"].clone(), r["windows"].clone()), (json!(3), json!(1)), "{r}");
    let walls = made(&s, "wall");
    // Open below the head in each door gap (the middle and near both jambs), a header above.
    for (x, y) in
        [(114.0, 3.0), (98.0, 3.0), (130.0, 3.0), (477.0, 138.0), (477.0, 122.0), (477.0, 154.0), (291.0, 258.0), (291.0, 242.0), (291.0, 274.0)]
    {
        assert!(!wall_at(&walls, x, y, 40.0), "the doorway at ({x}, {y}) is open");
        assert!(wall_at(&walls, x, y, 100.0), "a header over ({x}, {y})");
    }
    // Solid right beside each opening.
    for (x, y) in [(90.0, 3.0), (138.0, 3.0), (477.0, 114.0), (477.0, 162.0), (291.0, 234.0), (291.0, 282.0)] {
        assert!(wall_at(&walls, x, y, 40.0), "the wall at ({x}, {y})");
    }
    // The plain gap: the window's sill and head, open between.
    assert!(wall_at(&walls, 378.0, 357.0, 12.0) && wall_at(&walls, 378.0, 357.0, 100.0) && !wall_at(&walls, 378.0, 357.0, 50.0));
    let glass = made(&s, "window");
    assert_eq!(glass.len(), 1);
    let (x0, y0, x1, y1) = plan_box(glass.first().unwrap());
    assert!((x0 - 360.0).abs() < 1e-6 && (x1 - 396.0).abs() < 1e-6 && y0 > 354.0 && y1 < 360.0, "{x0} {y0} {x1} {y1}");
    // Each leaf: hinged at its jamb on the inside face, inside its opening, standing open 3' into
    // the room.
    let mut leaves: Vec<(f64, f64, f64, f64)> = made(&s, "door").iter().map(plan_box).collect();
    leaves.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(leaves.len(), 3, "{leaves:?}");
    let near = |a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)| {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6 && (a.2 - b.2).abs() < 1e-6 && (a.3 - b.3).abs() < 1e-6
    };
    // South: x 96–97.75 (in the 96–132 opening), y 6–42.
    assert!(near(leaves[0], (96.0, 6.0, 97.75, 42.0)), "{:?}", leaves[0]);
    // Partition: hinge (294, 240), open west to x 258, y 240–241.75.
    assert!(near(leaves[1], (258.0, 240.0, 294.0, 241.75)), "{:?}", leaves[1]);
    // East: hinge (474, 120), open west to x 438, y 120–121.75.
    assert!(near(leaves[2], (438.0, 120.0, 474.0, 121.75)), "{:?}", leaves[2]);
    // The walls run to the outside faces, no farther.
    let all =
        walls.iter().map(plan_box).fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)));
    assert!(near(all, (0.0, 0.0, 480.0, 360.0)), "{all:?}");
    // The floor slab and the ceiling end at the outside faces (not 2' past).
    let floor = made(&s, "floor");
    let f = floor.first().unwrap();
    assert!(near(plan_box(f), (0.0, 0.0, 480.0, 360.0)), "{:?}", plan_box(f));
    assert!((flat_area(f, 0.0) - 480.0 * 360.0).abs() < 0.5, "{}", flat_area(f, 0.0));
    let ceiling = made(&s, "ceiling");
    assert!(near(plan_box(ceiling.first().unwrap()), (0.0, 0.0, 480.0, 360.0)));
}
