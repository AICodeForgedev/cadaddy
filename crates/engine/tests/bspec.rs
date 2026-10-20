//! The building spec: stored in the drawing and round-tripped through DXF and DWG, merged by
//! `bspec.set`, checked by `bspec.validate` (errors and warnings with the objects' handles), and
//! built by `model.build` — deterministically, with centreline walls, door and window families,
//! columns, curtain walls, room ceilings at their own heights, materials and the ground.

use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Attrib, AttribModes, AttribProps, Block, Entity, EntityKind, Insert, Line, Mesh};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    s.execute("layer.new", &json!({"name": name, "current": true})).unwrap();
}

/// A closed rectangle, feet → inches.
fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    let p = |x: f64, y: f64| [x * 12.0, y * 12.0];
    s.execute("pline", &json!({"vertices": [p(a[0], a[1]), p(b[0], a[1]), p(b[0], b[1]), p(a[0], b[1])], "closed": true})).unwrap();
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [[a[0] * 12.0, a[1] * 12.0], [b[0] * 12.0, b[1] * 12.0]]})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": [at[0] * 12.0, at[1] * 12.0], "text": t, "height": 6})).unwrap();
}

/// A 30' × 12' building of 6" walls: OFFICE (west) and STORAGE (east) split by a centreline
/// partition at x = 15', a 24" column, a curtain wall line inside the north wall, a 36" window
/// block (W36, its width in the WIDTH attribute) in the south wall, and a stair line.
fn plan() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [30.0, 12.0]);
    rect(&mut s, [0.5, 0.5], [29.5, 11.5]);
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [15.0, 0.5], [15.0, 11.5]);
    layer(&mut s, "S-COLS");
    s.execute("circle", &json!({"center": [60.0, 60.0], "radius": 12.0})).unwrap();
    layer(&mut s, "A-GLAZ-CURT");
    line(&mut s, [20.0, 11.0], [28.0, 11.0]);
    layer(&mut s, "A-FLOR-STRS");
    line(&mut s, [25.0, 2.0], [28.0, 2.0]);
    layer(&mut s, "A-ROOM-TAG");
    text(&mut s, [5.0, 6.0], "OFFICE");
    text(&mut s, [20.0, 6.0], "STORAGE");
    layer(&mut s, "A-WIN");
    // The window block (a 36" line) and its reference with a WIDTH attribute.
    let mut d = (*s.doc().unwrap()).clone();
    let tag_text = d.model.iter().find_map(|e| if let EntityKind::Text(t) = &e.kind { Some(t.clone()) } else { None }).unwrap();
    let mut b = Block::new("W36");
    let h = d.new_handle();
    b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(-18.0, 0.0, 0.0), b: Vec3::new(18.0, 0.0, 0.0) })));
    d.blocks.insert("W36".into(), Arc::new(b));
    let attrib = Attrib {
        tag: "WIDTH".into(),
        text: cadaddy_engine::doc::Text { value: "36".into(), ..tag_text },
        invisible: true,
        constant: false,
        prompt: String::new(),
        props: AttribProps::default(),
        multiline: None,
        modes: AttribModes::default(),
    };
    let ins = Insert {
        block: "W36".into(),
        insert: Vec3::new(10.0 * 12.0, 3.0, 0.0),
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: vec![attrib],
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    };
    let h = d.new_handle();
    let mut e = Entity::new(h, EntityKind::Insert(ins));
    e.common.layer = "A-WIN".into();
    d.model.push(e);
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    s
}

fn spec() -> Value {
    json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "A-WIN", "role": "window"},
            {"layer": "A-GLAZ-CURT", "role": "curtain_wall"},
            {"layer": "S-COLS", "role": "column"},
            {"layer": "A-FLOR-STRS", "role": "stair"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6, "from": "pairs"},
            {"name": "Partition", "height_ft": 9, "thickness_in": 4.875, "from": "centerline"}
        ],
        "rooms": [{"name": "Office", "ceiling_ft": 9, "materials": {"ceiling": "Acoustic tile"}}, {"name": "Storage"}],
        "families": [{"name": "Window 3'", "kind": "window", "block": "W*", "width_attr": "WIDTH", "sill_ft": 3, "head_ft": 7}],
        "materials": {"wall": "Painted gypsum", "floor": "Concrete"},
        "site": {"elevation_ft": -0.5, "margin_ft": 10}
    })
}

fn made(s: &Session, part: &str) -> Vec<(Mesh, Vec<String>)> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some(part))
        .filter_map(|e| {
            if let EntityKind::Mesh(m) = &e.kind { Some((m.clone(), e.common.xdata.iter().map(|t| t.value.clone()).collect())) } else { None }
        })
        .collect()
}

fn z_range(m: &Mesh) -> (f64, f64) {
    m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.z), hi.max(v.z)))
}

/// Whether a z range is `lo`–`hi` (to a millionth).
fn spans(m: &Mesh, lo: f64, hi: f64) -> bool {
    let (a, b) = z_range(m);
    (a - lo).abs() < 1e-6 && (b - hi).abs() < 1e-6
}

fn x_range(m: &Mesh) -> (f64, f64) {
    m.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.x), hi.max(v.x)))
}

/// Everything a build made, without handles: layer, xdata, geometry, in drawing order.
fn model(s: &Session) -> Vec<String> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).is_some())
        .map(|e| {
            // The source handle (fourth tag) names plan objects, the same in both sessions.
            let tags: Vec<&str> = e.common.xdata.iter().map(|t| t.value.as_str()).collect();
            format!("{} {:?} {:?}", e.common.layer, tags, e.kind)
        })
        .collect()
}

#[test]
fn the_spec_round_trips_through_the_drawing_dxf_and_dwg() {
    let mut s = plan();
    // No spec yet: one is drafted from Walk mode's defaults.
    let r = s.execute("bspec.get", &json!({})).unwrap();
    assert_eq!(r["stored"], json!(false));
    assert_eq!(r["spec"]["layers"][0]["layer"], json!("*WALL*"));
    let set = s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    assert_eq!(set["errors"], json!([]), "{set}");
    let got = s.execute("bspec.get", &json!({})).unwrap();
    assert_eq!(got["stored"], json!(true));
    assert_eq!(got["spec"], set["spec"]);
    assert_eq!(got["spec"]["wall_types"][1]["from"], json!("centerline"));
    // Merge: one level field and one room change; the rest stays.
    let m = s
        .execute(
            "bspec.set",
            &json!({"spec": {"levels": [{"name": "l1", "ceiling_ft": 8.5}], "rooms": [{"name": "Storage", "remove": true}], "site": null}}),
        )
        .unwrap();
    assert_eq!(m["spec"]["levels"][0]["name"], json!("L1"));
    assert_eq!(m["spec"]["levels"][0]["ceiling_ft"], json!(8.5));
    assert_eq!(m["spec"]["levels"][0]["height_ft"], json!(10.0));
    assert_eq!(m["spec"]["rooms"].as_array().unwrap().len(), 1);
    assert_eq!(m["spec"]["site"], Value::Null);
    assert_eq!(m["spec"]["families"], got["spec"]["families"]);
    // Undo takes the merge back.
    s.undo().unwrap();
    assert_eq!(s.execute("bspec.get", &json!({})).unwrap()["spec"], got["spec"]);
    // The drawing written as DXF and as DWG reads back with the same spec.
    for name in ["x.dxf", "x.dwg"] {
        let bytes = cadaddy_io::write(s.doc().unwrap(), name).unwrap();
        let d = cadaddy_io::read(&bytes, name).unwrap();
        let mut back = Session::empty();
        back.open_drawing(d, "Back", None);
        let r = back.execute("bspec.get", &json!({})).unwrap();
        assert_eq!(r["stored"], json!(true), "{name}");
        assert_eq!(r["spec"], got["spec"], "{name}");
    }
}

#[test]
fn broken_specs_give_errors_and_warnings_with_handles() {
    let mut s = plan();
    let mut bad = spec();
    bad["levels"] = json!([
        {"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 12},
        {"name": "l1", "elevation_ft": 5, "height_ft": 0}
    ]);
    bad["layers"][0]["wall_type"] = json!("Nope");
    bad["rooms"] = json!([{"name": "Office", "level": "L9"}, {"name": "Lab"}]);
    bad["families"][0]["sill_ft"] = json!(8);
    bad["families"].as_array_mut().unwrap().push(json!({"name": "Door", "kind": "door", "block": "D-*"}));
    let r = s.execute("bspec.validate", &json!({"spec": bad, "replace": true})).unwrap();
    assert_eq!(r["ok"], json!(false));
    let paths = |k: &str| r[k].as_array().unwrap().iter().map(|i| i["path"].as_str().unwrap().to_string()).collect::<Vec<_>>();
    let errors = paths("errors");
    for p in ["levels[0].ceiling_ft", "levels[1].name", "levels[1].height_ft", "layers[0].wall_type", "rooms[0].level", "families[0].sill_ft"] {
        assert!(errors.iter().any(|e| e == p), "{p} not in {errors:?}");
    }
    let warnings = paths("warnings");
    for p in ["rooms[1]", "families[1].block", "layers[5]"] {
        assert!(warnings.iter().any(|w| w == p), "{p} not in {warnings:?}");
    }
    // The stair warning names the stair line.
    let stair = r["warnings"].as_array().unwrap().iter().find(|w| w["path"] == "layers[5]").unwrap();
    let h = stair["handles"][0].as_str().unwrap();
    let e = s.doc().unwrap().model.iter().find(|e| e.handle.hex() == h).unwrap().clone();
    assert_eq!(e.common.layer, "A-FLOR-STRS");
    // A building with errors isn't built.
    let err = s.execute("model.build", &json!({"spec": bad, "replace": true})).unwrap_err().to_string();
    assert!(err.contains("levels[0].ceiling_ft"), "{err}");
    assert!(made(&s, "wall").is_empty());
    // A spec that isn't one is refused outright, and nothing is stored.
    assert!(s.execute("bspec.set", &json!({"spec": {"version": 99}})).is_err());
    assert!(s.execute("bspec.set", &json!({"spec": {"levels": "L1"}})).is_err());
    assert!(s.execute("bspec.set", &json!({"spec": [1]})).is_err());
    assert!(s.execute("bspec.set", &json!({})).is_err());
    assert_eq!(s.execute("bspec.get", &json!({})).unwrap()["stored"], json!(false));
    // Objects outside every level's region.
    let mut v = spec();
    v["levels"][0]["region"] = json!([0, 0, 120, 200]);
    let r = s.execute("bspec.validate", &json!({"spec": v, "replace": true})).unwrap();
    let out = r["warnings"].as_array().unwrap().iter().find(|w| w["path"] == "levels").unwrap();
    assert!(!out["handles"].as_array().unwrap().is_empty(), "{out}");
    // The good spec is clean but for the stair warning.
    let r = s.execute("bspec.validate", &json!({"spec": spec(), "replace": true})).unwrap();
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(paths_of(&r["warnings"]), vec!["layers[5]"]);
}

fn paths_of(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|i| i["path"].as_str().unwrap().to_string()).collect()
}

#[test]
fn model_build_follows_the_spec_and_is_deterministic() {
    let mut s = plan();
    s.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    let t = std::time::Instant::now();
    let r = s.execute("model.build", &json!({})).unwrap();
    let took = t.elapsed();
    assert_eq!(r["columns"], json!(1), "{r}");
    assert_eq!(r["curtainWalls"], json!(1), "{r}");
    assert_eq!(r["windows"], json!(1), "{r}");
    assert_eq!(r["rooms"], json!(2), "{r}");
    assert_eq!(r["ceilings"], json!(2), "{r}");
    assert_eq!(r["ground"], json!(1), "{r}");
    assert!(r["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("stairs are drawn but the spec has none")), "{r}");
    // Walls: the exterior pair's 4 at floor to floor (less Walk mode's 0.1'), the partition from
    // its centreline at its type's 9' and 4⅞" thick.
    let walls = made(&s, "wall");
    assert_eq!(walls.len(), 5, "{r}");
    let partition = walls.iter().find(|(m, _)| (x_range(m).1 - x_range(m).0 - 4.875).abs() < 1e-6).expect("the partition");
    assert!(spans(&partition.0, 0.0, 108.0));
    assert_eq!(walls.iter().filter(|(m, _)| spans(m, 0.0, 118.8)).count(), 4);
    assert!(walls.iter().all(|(_, x)| x.contains(&"material:Painted gypsum".to_string())));
    // The window: 36" wide (its attribute) from the family's 3' sill to its 7' head.
    let glass = made(&s, "window");
    assert_eq!(glass.len(), 1);
    assert!(spans(&glass[0].0, 36.0, 84.0), "{:?}", z_range(&glass[0].0));
    assert!((x_range(&glass[0].0).1 - x_range(&glass[0].0).0 - 36.0).abs() < 1e-6, "{:?}", x_range(&glass[0].0));
    // Ceilings: the office at its 9', storage at the level's 8'; the office's own material.
    let mut ceilings: Vec<(f64, Vec<String>)> = made(&s, "ceiling").into_iter().map(|(m, x)| (z_range(&m).0, x)).collect();
    ceilings.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(ceilings.iter().map(|c| c.0).collect::<Vec<_>>(), vec![96.0, 108.0]);
    assert!(
        ceilings[1].1.contains(&"room:Office".to_string()) && ceilings[1].1.contains(&"material:Acoustic tile".to_string()),
        "{:?}",
        ceilings[1].1
    );
    // The column: a 24" round prism floor to wall top; the curtain wall's glass full height.
    let col = &made(&s, "column")[0].0;
    assert!(spans(col, 0.0, 118.8));
    assert!((x_range(col).1 - x_range(col).0 - 24.0).abs() < 1e-6);
    assert!(spans(&made(&s, "curtainwall")[0].0, 0.0, 118.8));
    // The ground: 10' past the building (its floor slab, which ends at the walls' outside faces)
    // all round, its top at the site elevation.
    let ground = &made(&s, "ground")[0].0;
    assert!(spans(ground, -18.0, -6.0), "{:?}", z_range(ground));
    let (gx0, gx1) = x_range(ground);
    assert!((gx0 + 120.0).abs() < 1e-6 && (gx1 - 480.0).abs() < 1e-6, "{gx0} {gx1}");
    assert!(made(&s, "floor").iter().all(|(_, x)| x.contains(&"material:Concrete".to_string())));
    // Same spec and drawing → the same model: built again here, and in a fresh session.
    let first = model(&s);
    let again = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(again["removed"], json!(first.len()));
    assert_eq!(model(&s), first);
    let mut other = plan();
    other.execute("bspec.set", &json!({"spec": spec(), "replace": true})).unwrap();
    other.execute("model.build", &json!({})).unwrap();
    assert_eq!(model(&other), first);
    // Associative: moving the partition rebuilds it; clear removes everything.
    let h = s.doc().unwrap().model.iter().find(|e| e.common.layer == "A-WALL-CL").unwrap().handle;
    s.execute("move", &json!({"handles": [h.hex()], "from": [0, 0], "to": [24, 0]})).unwrap();
    let moved = made(&s, "wall");
    assert!(moved.iter().any(|(m, _)| (x_range(m).0 - (17.0 * 12.0 - 2.4375)).abs() < 1e-6), "partition not rebuilt");
    let n = s.execute("model.build", &json!({"clear": true})).unwrap()["removed"].as_u64().unwrap();
    assert_eq!(n as usize, first.len());
    eprintln!("model.build: {} objects in {:?}", first.len(), took);
}
