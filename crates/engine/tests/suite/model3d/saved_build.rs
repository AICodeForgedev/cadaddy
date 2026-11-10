//! A saved model is what its spec builds (Q6403): `model.build` on a model saved and opened again
//! with nothing changed keeps it as it is, steps after its build included; a change to one
//! level's entry or to a roof replaces only those levels' elements (and the storey under a roof);
//! and a rebuild says what it changed per level and part, and why.

use std::collections::BTreeMap;
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bspec::made_level;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Block, Entity, EntityKind, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
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

fn ft(p: [f64; 2]) -> [f64; 2] {
    [p[0] * 12.0, p[1] * 12.0]
}

/// Two storeys and a roof drawn side by side (100' apart): each storey a 30' × 12' box of 6"
/// walls split by a partition with two room tags; the roof plan's outline.
fn plan() -> Session {
    let mut s = Session::new();
    let mut levels = Vec::new();
    for (i, name) in ["Level 1", "Level 2"].into_iter().enumerate() {
        let ox = i as f64 * 100.0;
        layer(&mut s, "A-WALL");
        rect(&mut s, [ox, 0.0], [ox + 30.0, 12.0]);
        rect(&mut s, [ox + 0.5, 0.5], [ox + 29.5, 11.5]);
        layer(&mut s, "A-WALL-CL");
        line(&mut s, [ox + 15.0, 0.5], [ox + 15.0, 11.5]);
        layer(&mut s, "A-ROOM-TAG");
        text(&mut s, [ox + 5.0, 6.0], "OFFICE");
        text(&mut s, [ox + 20.0, 6.0], "STORAGE");
        levels.push(json!({"name": name, "elevation_ft": i as f64 * 10.0, "height_ft": 10, "ceiling_ft": 8,
            "region": ([ft([ox - 10.0, -10.0]), ft([ox + 40.0, 22.0])].concat()), "base": ft([ox, 0.0])}));
    }
    levels.push(json!({"name": "Roof", "elevation_ft": 20, "height_ft": 4, "ceiling_ft": 4,
        "region": ([ft([190.0, -10.0]), ft([240.0, 22.0])].concat()), "base": ft([200.0, 0.0])}));
    let spec = json!({
        "version": 1,
        "units_per_foot": 12,
        "levels": levels,
        "layers": [
            {"layer": "A-WALL", "role": "wall", "wall_type": "Exterior"},
            {"layer": "A-WALL-CL", "role": "wall", "wall_type": "Partition"},
            {"layer": "A-ROOM-TAG", "role": "room_tag"}
        ],
        "wall_types": [
            {"name": "Exterior", "thickness_in": 6, "from": "pairs"},
            {"name": "Partition", "height_ft": 9, "thickness_in": 4.875, "from": "centerline"}
        ],
        "exterior": {
            "roofs": [{"name": "Main Roof", "level": "Roof", "outline": [ft([200.0, 0.0]), ft([230.0, 0.0]), ft([230.0, 12.0]), ft([200.0, 12.0])], "parapet_ft": 3}]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).unwrap();
    // A symbol block of many objects, as a print set's PDF symbols are (the block's object store
    // must not make the drawing read as changed once it is saved and opened again).
    let mut d = (*s.doc().unwrap()).clone();
    let mut b = Block::new("SYM-1");
    for i in 0..60 {
        let h = d.new_handle();
        b.entities.push(Entity::new(h, EntityKind::Line(Line { a: Vec3::new(f64::from(i), 0.0, 0.0), b: Vec3::new(f64::from(i), 6.0, 0.0) })));
    }
    d.blocks.insert("SYM-1".into(), Arc::new(b));
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    s
}

/// The model's built elements by `level|part`.
fn counts(s: &Session) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for e in s.doc().unwrap().model.iter() {
        if let Some(part) = made_as(e) {
            *out.entry(format!("{}|{part}", made_level(e).unwrap_or_default())).or_default() += 1;
        }
    }
    out
}

/// Every built element of `level` as it stands (handle, layer, geometry).
fn elements_of(s: &Session, level: &str) -> Vec<String> {
    let mut v: Vec<String> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).is_some() && made_level(e).as_deref() == Some(level))
        .map(|e| format!("{} {} {:?}", e.handle.hex(), e.common.layer, e.kind))
        .collect();
    v.sort();
    v
}

/// The drawing saved and opened again: a new session, nothing of this one's state.
fn reopen(s: &Session) -> Session {
    let bytes = cadaddy_io::write(s.doc().unwrap(), "model.cdy").unwrap();
    let d = cadaddy_io::read(&bytes, "model.cdy").unwrap();
    let mut t = Session::empty();
    t.open_drawing(d, "model", None);
    t
}

/// Built, then a step after the build (one partition of `level` erased, as a job's tag identity
/// merge takes a wall away), saved and opened again.
fn built_with_a_step(level: &str) -> Session {
    let mut s = plan();
    s.execute("model.build", &json!({})).unwrap();
    let h = s
        .doc()
        .unwrap()
        .model
        .iter()
        .find(|e| made_as(e).as_deref() == Some("wall") && made_level(e).as_deref() == Some(level))
        .map(|e| e.handle)
        .unwrap();
    s.execute("erase", &json!({"handles": [h.hex()]})).unwrap();
    reopen(&s)
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

#[test]
fn a_saved_model_built_again_unchanged_is_kept_with_its_steps() {
    let mut s = built_with_a_step("Level 1");
    let saved = counts(&s);
    let l1 = elements_of(&s, "Level 1");
    let r = s.execute("model.build", &json!({})).unwrap();
    // Nothing the build reads changed: kept, not laid out again without the step.
    assert_eq!(counts(&s), saved, "the erased wall came back");
    assert_eq!(elements_of(&s, "Level 1"), l1);
    assert_eq!(r["kept"], json!(true), "{r}");
    assert_eq!(r["rebuild"]["changed_categories"], json!(0), "{r}");
    let steps = r["rebuild"]["steps_kept"].as_array().unwrap();
    assert!(steps.iter().any(|st| st["step"] == json!("erase") && st["changed"]["Level 1|wall"] == json!(-1)), "{r}");
    // The record went with the drawing: the spec keeps it.
    let spec: Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    assert!(spec["build_record"]["engine"].as_str().is_some_and(|e| !e.is_empty()), "{}", spec["build_record"]);
    assert_eq!(spec["build_record"]["now"]["Level 1|wall"], json!(saved["Level 1|wall"]));

    // Forced: laid out again, and the report says what that changed and why.
    let r = s.execute("model.build", &json!({"force": true})).unwrap();
    assert!(r.get("kept").is_none(), "{r}");
    let changed = r["rebuild"]["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1, "{r}");
    assert_eq!(changed[0], json!({"level": "Level 1", "part": "wall", "before": saved["Level 1|wall"], "after": saved["Level 1|wall"] + 1}));
    let why = strs(&r["rebuild"]["why"]);
    assert!(why.iter().any(|w| w == "force: true"), "{why:?}");
    assert!(why.iter().any(|w| w.contains("not repeated by a rebuild: erase")), "{why:?}");
    assert_eq!(r["rebuild"]["steps_not_repeated"][0]["by"], json!("erase"), "{r}");
    // The forced build's own record has no steps: built again unchanged, it is kept.
    let again = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(again["kept"], json!(true), "{again}");
}

#[test]
fn a_changed_plan_rebuilds_the_whole_model_and_says_why() {
    let mut s = built_with_a_step("Level 1");
    let saved = counts(&s);
    // One more partition on Level 2's plan.
    layer(&mut s, "A-WALL-CL");
    line(&mut s, [107.0, 0.5], [107.0, 11.5]);
    let r = s.execute("model.build", &json!({})).unwrap();
    assert!(r.get("kept").is_none() && r.get("partial").is_none(), "{r}");
    let why = strs(&r["rebuild"]["why"]);
    assert!(why.iter().any(|w| w.starts_with("the plan")), "{why:?}");
    // Level 1 got its erased wall back (the step isn't repeated) and Level 2 its new partition.
    let now = counts(&s);
    assert_eq!(now["Level 1|wall"], saved["Level 1|wall"] + 1);
    assert!(now["Level 2|wall"] > saved["Level 2|wall"], "{now:?} {saved:?}");
    let changed = r["rebuild"]["changed"].as_array().unwrap();
    assert!(changed.iter().any(|c| c["level"] == json!("Level 2") && c["part"] == json!("wall")), "{r}");
}

#[test]
fn a_changed_ceiling_rebuilds_only_its_level() {
    let mut s = built_with_a_step("Level 1");
    let l1 = elements_of(&s, "Level 1");
    let l2 = elements_of(&s, "Level 2");
    let roof = elements_of(&s, "Roof");
    s.execute("levels.set", &json!({"name": "Level 2", "ceiling_ft": 9})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    // Level 1 and the roof are as they were (the step after the build included); Level 2 is new.
    assert_eq!(elements_of(&s, "Level 1"), l1, "Level 1 was laid out again");
    assert_eq!(elements_of(&s, "Roof"), roof);
    assert_eq!(r["partial"]["rebuilt_levels"], json!(["level 2"]), "{r}");
    assert_ne!(elements_of(&s, "Level 2"), l2);
    let ceiling_top = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("ceiling") && made_level(e).as_deref() == Some("Level 2"))
        .filter_map(|e| if let cadaddy_engine::doc::EntityKind::Mesh(m) = &e.kind { m.vertices.iter().map(|v| v.z).reduce(f64::min) } else { None })
        .reduce(f64::min)
        .unwrap();
    assert!((ceiling_top - (10.0 + 9.0) * 12.0).abs() < 1e-6, "{ceiling_top}");
    // The record keeps the step on Level 1, and a second build changes nothing.
    let again = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(again["kept"], json!(true), "{again}");
    assert!(again["rebuild"]["steps_kept"].as_array().unwrap().iter().any(|st| st["step"] == json!("erase")), "{again}");
    assert_eq!(elements_of(&s, "Level 1"), l1);
}

#[test]
fn a_changed_roof_rebuilds_the_roof_and_the_storey_under_it() {
    let mut s = built_with_a_step("Level 1");
    let l1 = elements_of(&s, "Level 1");
    let roof = elements_of(&s, "Roof");
    s.execute(
        "bspec.set",
        &json!({"spec": {"exterior": {"roofs": [{"name": "Main Roof", "level": "Roof", "outline": [ft([200.0, 0.0]), ft([230.0, 0.0]), ft([230.0, 12.0]), ft([200.0, 12.0])], "parapet_ft": 4}]}}}),
    )
    .unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(elements_of(&s, "Level 1"), l1, "Level 1 was laid out again");
    assert_eq!(r["partial"]["rebuilt_levels"], json!(["level 2", "roof"]), "{r}");
    assert_ne!(elements_of(&s, "Roof"), roof);
}

#[test]
fn a_moved_level_rebuilds_it_and_the_storey_it_now_tops() {
    let mut s = built_with_a_step("Level 1");
    let l1 = elements_of(&s, "Level 1");
    // The roof 1' higher: Level 2's floor to floor follows it (both entries change).
    let set = s.execute("levels.set", &json!({"name": "Roof", "elevation_ft": 21})).unwrap();
    assert!(set["changes"].to_string().contains("Level 2"), "{set}");
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(elements_of(&s, "Level 1"), l1, "Level 1 was laid out again");
    assert_eq!(r["partial"]["rebuilt_levels"], json!(["level 2", "roof"]), "{}", r["rebuild"]);
}
