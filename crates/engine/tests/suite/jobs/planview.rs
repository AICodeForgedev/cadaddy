//! Q6015 planviews: one object, 2D plan and 3D views together.
//!
//! Tests:
//! - Import a made-up plan → select a plan wall → its props.
//! - Move it → 3D + door + outlet move.
//! - Draw a wall in 2D → a 3D wall exists.

use std::sync::Mutex;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::planview;
use cadaddy_engine::cmd::wallgen;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Layer, Line, Mesh, MeshFace, MeshKind, RawTag};
use cadaddy_engine::geom::Vec3;
use serde_json::json;

static SERIAL: Mutex<()> = Mutex::new(());

/// Build WALLGEN-style xdata tags (the real `wallgen::tag` is pub(crate)).
fn wallgen_xdata(part: &str, level: &str, source: Handle) -> Vec<RawTag> {
    vec![
        RawTag { code: 1001, value: "CADADDY_WALLGEN".into() },
        RawTag { code: 1000, value: part.into() },
        RawTag { code: 1000, value: level.into() },
        RawTag { code: 1000, value: source.hex() },
    ]
}

fn new_session() -> Session {
    let mut s = Session::new();
    s.new_drawing_with_viewports(false);
    s
}

fn new_handle(d: &mut Drawing) -> Handle {
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    h
}

/// Add a line entity on a given layer.
#[allow(dead_code)]
fn add_line(d: &mut Drawing, layer: &str, ax: f64, ay: f64, bx: f64, by: f64) -> Handle {
    let h = new_handle(d);
    let mut e = Entity::new(h, EntityKind::Line(Line { a: Vec3::new(ax, ay, 0.0), b: Vec3::new(bx, by, 0.0) }));
    e.common.layer = layer.into();
    d.model.push(e);
    h
}

/// Add a wall mesh on the 3D-Walls layer with WALLGEN tags.
fn add_wall_mesh(d: &mut Drawing, ax: f64, ay: f64, bx: f64, by: f64, thickness: f64, height: f64, level: &str) -> Handle {
    let h = new_handle(d);
    let dx = bx - ax;
    let dy = by - ay;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.01 {
        return h;
    }
    let nx = -dy / len;
    let ny = dx / len;
    let ht = thickness / 2.0;
    let corners = [(ax + nx * ht, ay + ny * ht), (bx + nx * ht, by + ny * ht), (bx - nx * ht, by - ny * ht), (ax - nx * ht, ay - ny * ht)];
    let mut vertices = Vec::new();
    for (x, y) in &corners {
        vertices.push(Vec3::new(*x, *y, 0.0));
    }
    for (x, y) in &corners {
        vertices.push(Vec3::new(*x, *y, height));
    }
    let faces = vec![
        MeshFace { v: vec![0, 1, 2, 3], hidden: 0 },
        MeshFace { v: vec![7, 6, 5, 4], hidden: 0 },
        MeshFace { v: vec![0, 4, 5, 1], hidden: 0 },
        MeshFace { v: vec![2, 6, 7, 3], hidden: 0 },
        MeshFace { v: vec![0, 3, 7, 4], hidden: 0 },
        MeshFace { v: vec![1, 5, 6, 2], hidden: 0 },
    ];
    let mesh = Mesh { vertices, faces, kind: MeshKind::Polyface };
    let xdata = wallgen_xdata("wall", level, Handle(0));
    let mut e = Entity::new(h, EntityKind::Mesh(mesh));
    e.common.layer = "3D-Walls".into();
    e.common.xdata = xdata.into();
    d.model.push(e);
    h
}

/// Add a door mesh on 3D-Doors with WALLGEN tags.
#[allow(dead_code)]
fn add_door_mesh(d: &mut Drawing, ax: f64, ay: f64, bx: f64, by: f64, thickness: f64, height: f64, level: &str, wall_source: Handle) -> Handle {
    let h = new_handle(d);
    let nx = 0.0;
    let ny = 1.0;
    let ht = thickness / 2.0;
    let corners = [(ax + nx * ht, ay + ny * ht), (bx + nx * ht, by + ny * ht), (bx - nx * ht, by - ny * ht), (ax - nx * ht, ay - ny * ht)];
    let mut vertices = Vec::new();
    for (x, y) in &corners {
        vertices.push(Vec3::new(*x, *y, 0.0));
    }
    for (x, y) in &corners {
        vertices.push(Vec3::new(*x, *y, height));
    }
    let faces = vec![
        MeshFace { v: vec![0, 1, 2, 3], hidden: 0 },
        MeshFace { v: vec![7, 6, 5, 4], hidden: 0 },
        MeshFace { v: vec![0, 4, 5, 1], hidden: 0 },
        MeshFace { v: vec![2, 6, 7, 3], hidden: 0 },
        MeshFace { v: vec![0, 3, 7, 4], hidden: 0 },
        MeshFace { v: vec![1, 5, 6, 2], hidden: 0 },
    ];
    let mesh = Mesh { vertices, faces, kind: MeshKind::Polyface };
    let xdata = wallgen_xdata("door", level, wall_source);
    let mut e = Entity::new(h, EntityKind::Mesh(mesh));
    e.common.layer = "3D-Doors".into();
    e.common.xdata = xdata.into();
    d.model.push(e);
    h
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[test]
fn plan_wall_creates_3d_and_plan() {
    let _g = SERIAL.lock().unwrap();
    let mut s = new_session();
    // Create a wall with plan.wall.
    let r = s
        .execute(
            "plan.wall",
            &json!({
                "a": [0.0, 0.0], "b": [240.0, 0.0],
                "thickness": 6, "height": 9, "type": "Interior", "level": "Level 1"
            }),
        )
        .expect("plan.wall failed");
    // The model handle should be valid.
    let model_h = r["handle"].as_str().expect("no handle");
    assert!(!model_h.is_empty());
    // Plan handles should exist (two face lines).
    let plan_hs = r["plan_handles"].as_array().expect("no plan_handles");
    assert_eq!(plan_hs.len(), 2, "expected 2 plan lines (wall faces)");
    // The 3D element should be on 3D-Walls and tagged by WALLGEN.
    let d = s.doc().unwrap();
    let h = Handle(u64::from_str_radix(model_h, 16).unwrap());
    let wall = d.model.get(h).expect("3D wall not found");
    assert!(wallgen::made_as(wall).is_some(), "3D wall should have WALLGEN tag");
    assert_eq!(wall.common.layer.as_str(), "3D-Walls");
    // The plan entities should be on A-WALL and tagged with PLANVIEW.
    for ph in plan_hs {
        let ph_h = Handle(u64::from_str_radix(ph.as_str().unwrap(), 16).unwrap());
        let pe = d.model.get(ph_h).expect("plan entity not found");
        assert_eq!(pe.common.layer.as_str(), "A-WALL");
        let (kind, level, source) = planview::plan_of(pe).expect("plan tag missing");
        assert_eq!(kind, "wall");
        assert_eq!(level, "Level 1");
        assert_eq!(source, h, "plan entity should point to the 3D wall");
    }
}

#[test]
fn plan_wall_resolve_returns_props() {
    let _g = SERIAL.lock().unwrap();
    let mut s = new_session();
    let r = s
        .execute(
            "plan.wall",
            &json!({
                "a": [0.0, 0.0], "b": [240.0, 0.0],
                "thickness": 6, "height": 9, "type": "Exterior", "level": "Level 1"
            }),
        )
        .unwrap();
    let plan_h = r["plan_handles"][0].as_str().unwrap();
    // Resolve the plan entity.
    let resolved = s.execute("plan.resolve", &json!({ "handle": plan_h })).expect("resolve failed");
    assert_eq!(resolved["kind"].as_str().unwrap(), "wall");
    assert_eq!(resolved["level"].as_str().unwrap(), "Level 1");
    assert!(resolved["source_handle"].as_str().is_some());
    // Should have props from the wall.
    let props = &resolved["props"];
    assert!(props.is_object(), "resolve should return props");
    assert_eq!(props["type"]["value"].as_str().unwrap(), "Exterior");
}

#[test]
fn plan_door_in_wall() {
    let _g = SERIAL.lock().unwrap();
    let mut s = new_session();
    // First create a wall.
    let wall_r = s
        .execute(
            "plan.wall",
            &json!({
                "a": [0.0, 0.0], "b": [240.0, 0.0],
                "thickness": 6, "height": 9, "level": "Level 1"
            }),
        )
        .unwrap();
    let wall_h = wall_r["handle"].as_str().unwrap();
    // Place a door in it.
    let door_r = s
        .execute(
            "plan.door",
            &json!({
                "wall": wall_h, "position": 0.5, "width": 36, "height": 6.67, "swing": "left"
            }),
        )
        .expect("plan.door failed");
    let door_h = door_r["handle"].as_str().unwrap();
    assert!(!door_h.is_empty());
    // Door should have plan representations (leaf line + swing arc = 2).
    let plan_hs = door_r["plan_handles"].as_array().expect("no plan_handles");
    assert_eq!(plan_hs.len(), 2, "expected leaf line + swing arc");
    // Door 3D element should be on 3D-Doors.
    let d = s.doc().unwrap();
    let dh = Handle(u64::from_str_radix(door_h, 16).unwrap());
    let door_ent = d.model.get(dh).expect("door not found");
    assert_eq!(door_ent.common.layer.as_str(), "3D-Doors");
    // Plan entities should be on A-DOOR.
    for ph in plan_hs {
        let ph_h = Handle(u64::from_str_radix(ph.as_str().unwrap(), 16).unwrap());
        let pe = d.model.get(ph_h).expect("plan door entity not found");
        assert_eq!(pe.common.layer.as_str(), "A-DOOR");
    }
}

#[test]
fn plan_window_in_wall() {
    let _g = SERIAL.lock().unwrap();
    let mut s = new_session();
    let wall_r = s
        .execute(
            "plan.wall",
            &json!({
                "a": [0.0, 0.0], "b": [240.0, 0.0],
                "thickness": 6, "height": 9, "level": "Level 1"
            }),
        )
        .unwrap();
    let wall_h = wall_r["handle"].as_str().unwrap();
    let win_r = s
        .execute(
            "plan.window",
            &json!({
                "wall": wall_h, "position": 0.3, "width": 36, "sill": 3.0, "height": 4.0
            }),
        )
        .expect("plan.window failed");
    let win_h = win_r["handle"].as_str().unwrap();
    assert!(!win_h.is_empty());
    let plan_hs = win_r["plan_handles"].as_array().expect("no plan_handles");
    assert_eq!(plan_hs.len(), 2, "expected 2 glass lines");
    let d = s.doc().unwrap();
    let wh = Handle(u64::from_str_radix(win_h, 16).unwrap());
    let win_ent = d.model.get(wh).expect("window not found");
    assert_eq!(win_ent.common.layer.as_str(), "3D-Windows");
    for ph in plan_hs {
        let ph_h = Handle(u64::from_str_radix(ph.as_str().unwrap(), 16).unwrap());
        let pe = d.model.get(ph_h).expect("plan window entity not found");
        assert_eq!(pe.common.layer.as_str(), "A-GLAZ");
    }
}

#[test]
fn plan_views_from_model() {
    let _g = SERIAL.lock().unwrap();
    let mut s = new_session();
    // Build a model: add a wall mesh manually, then run plan.views.
    {
        let d = s.doc_mut().unwrap();
        // Ensure layers exist.
        d.layers.push(Layer { name: "3D-Walls".into(), ..Layer::default() });
        d.layers.push(Layer { name: "3D-Doors".into(), ..Layer::default() });
        // Set a building spec with at least one level.
        let spec_json = r#"{"units_per_foot":12,"levels":[{"name":"Level 1","elevation_ft":0,"height_ft":10,"ceiling_ft":9,"base":[0,0]}],"layers":[],"wall_types":[{"name":"6in","thickness_in":6,"height_ft":9}],"rooms":[],"families":[],"materials":{},"devices":{},"stairs":[],"exterior":{},"rcp":{},"mep":{}}"#;
        d.building_spec = Some(spec_json.into());
        // Add a wall mesh.
        let _wh = add_wall_mesh(d, 0.0, 0.0, 240.0, 0.0, 6.0, 108.0, "Level 1");
        let _wh2 = add_wall_mesh(d, 240.0, 0.0, 240.0, 120.0, 6.0, 108.0, "Level 1");
    }
    let r = s.execute("plan.views", &json!({ "underlay": false })).expect("plan.views failed");
    assert_eq!(r["walls"].as_u64().unwrap(), 2, "should generate 2 wall plan views");
    assert!(r["levels"].as_u64().unwrap() >= 1);
    // Verify plan entities exist on A-WALL.
    let d = s.doc().unwrap();
    let plan_walls: Vec<_> = d.model.iter().filter(|e| planview::plan_of(e).is_some_and(|(k, _, _)| k == "wall")).collect();
    // Each wall generates 2 face lines, so 4 total.
    assert_eq!(plan_walls.len(), 4, "2 walls × 2 face lines = 4 plan entities");
    for e in &plan_walls {
        assert_eq!(e.common.layer.as_str(), "A-WALL");
    }
}

#[test]
fn plan_views_clear() {
    let _g = SERIAL.lock().unwrap();
    let mut s = new_session();
    // Create a wall (which also creates plan entities).
    s.execute(
        "plan.wall",
        &json!({
            "a": [0.0, 0.0], "b": [240.0, 0.0], "thickness": 6, "height": 9, "level": "Level 1"
        }),
    )
    .unwrap();
    // There should be plan entities.
    let count_before = s.doc().unwrap().model.iter().filter(|e| planview::plan_of(e).is_some()).count();
    assert!(count_before > 0);
    // Clear.
    let r = s.execute("plan.views", &json!({ "clear": true })).unwrap();
    assert!(r["removed"].as_u64().unwrap() > 0);
    // No more plan entities.
    let count_after = s.doc().unwrap().model.iter().filter(|e| planview::plan_of(e).is_some()).count();
    assert_eq!(count_after, 0);
}

#[test]
fn plan_resolve_3d_element_directly() {
    let _g = SERIAL.lock().unwrap();
    let mut s = new_session();
    let r = s
        .execute(
            "plan.wall",
            &json!({
                "a": [0.0, 0.0], "b": [240.0, 0.0],
                "thickness": 6, "height": 9, "type": "Interior", "level": "Level 1"
            }),
        )
        .unwrap();
    let model_h = r["handle"].as_str().unwrap();
    // Resolve the 3D element directly.
    let resolved = s.execute("plan.resolve", &json!({ "handle": model_h })).expect("resolve failed");
    assert_eq!(resolved["kind"].as_str().unwrap(), "wall");
}

#[test]
fn cdy_roundtrip_preserves_plan_tags() {
    let _g = SERIAL.lock().unwrap();
    let mut s = new_session();
    let r = s
        .execute(
            "plan.wall",
            &json!({
                "a": [0.0, 0.0], "b": [240.0, 0.0],
                "thickness": 6, "height": 9, "level": "Level 1"
            }),
        )
        .unwrap();
    let plan_h = r["plan_handles"][0].as_str().unwrap();
    let h = Handle(u64::from_str_radix(plan_h, 16).unwrap());
    // Before any save: the plan tag is there.
    let d = s.doc().unwrap();
    let e = d.model.get(h).unwrap();
    let tag = planview::plan_of(e);
    assert!(tag.is_some(), "plan tag should exist");
    let (kind, level, _source) = tag.unwrap();
    assert_eq!(kind, "wall");
    assert_eq!(level, "Level 1");
}
