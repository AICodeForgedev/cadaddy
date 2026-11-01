//! Q6076 (battle test 5): reports say what they hold. A takeoff stopped at its cap says so;
//! BIM.BUILD is one undo step; the build's counts are the model's; model checks are found once
//! per model revision and a cap on them says how many there were.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Block, Common, EntityKind, Insert, Line, Space};
use cadaddy_engine::geom::Vec3;
use serde_json::json;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).expect("layer");
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).expect("pline");
}

/// A 40' × 30' box of 6" walls split in two by a partition, with its spec (inches).
fn plan() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [0.0, 0.0], [480.0, 360.0]);
    rect(&mut s, [6.0, 6.0], [474.0, 354.0]);
    s.execute("line", &json!({"points": [[240.0, 6.0], [240.0, 354.0]]})).expect("line");
    s.execute("line", &json!({"points": [[246.0, 6.0], [246.0, 354.0]]})).expect("line");
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "version": 1, "units_per_foot": 12,
            "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "region": [-100, -100, 600, 500], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .expect("spec");
    s
}

#[test]
fn a_takeoff_stopped_at_its_cap_says_so() {
    let mut s = Session::new();
    let d = s.doc_mut().expect("drawing");
    let mut b = Block::new("TICK");
    b.entities.push(cadaddy_engine::doc::Entity {
        handle: d.new_handle(),
        common: Common::default(),
        kind: EntityKind::Line(Line { a: Vec3::ZERO, b: Vec3::new(1.0, 0.0, 0.0) }),
    });
    d.blocks.insert("TICK".into(), std::sync::Arc::new(b));
    // 800 × 800 copies: more shapes than a takeoff measures.
    let grid = Insert {
        block: "TICK".into(),
        insert: Vec3::ZERO,
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: Vec::new(),
        cols: 800,
        rows: 800,
        col_spacing: 2.0,
        row_spacing: 2.0,
        clip: None,
    };
    d.add(&Space::Model, Common::default(), EntityKind::Insert(grid)).expect("minsert");
    let r = s.execute("takeoff", &json!({})).expect("takeoff");
    assert_eq!(r["truncated"], json!(true), "{}", r["message"]);
    assert_eq!(r["limit"], json!(500_000));
    assert!(r["message"].as_str().is_some_and(|m| m.contains("stopped at 500000 shapes")), "{}", r["message"]);
    // Under the cap: nothing said.
    let mut small = Session::new();
    small.execute("line", &json!({"points": [[0, 0], [10, 0]]})).expect("line");
    let r = small.execute("takeoff", &json!({})).expect("takeoff");
    assert_eq!(r["truncated"], json!(false));
    assert!(r.get("limit").is_none());
}

#[test]
fn bim_build_is_one_undo_step_and_its_counts_are_the_models() {
    let mut s = plan();
    let steps = |s: &Session| s.state().map(|st| st.undo.len()).unwrap_or(0);
    let before = steps(&s);
    let r = s.execute("bim.build", &json!({"force": true})).expect("bim.build");
    assert_eq!(steps(&s), before + 1, "one U undoes the build: {r}");
    let walls = s.doc().expect("drawing").model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).count();
    assert!(walls > 0);
    let b = s.execute("model.build", &json!({})).expect("model.build");
    let now = s.doc().expect("drawing").model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).count();
    assert_eq!(b["walls"], json!(now), "the build's wall count is the model's");
    s.execute("u", &json!({})).ok();
    s.execute("u", &json!({})).ok();
    let undone = s.doc().expect("drawing").model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).count();
    assert_eq!(undone, 0, "two U take both builds away");
}

#[test]
fn model_issues_are_found_once_per_revision_and_paged_from_there() {
    let mut s = plan();
    s.execute("model.build", &json!({})).expect("model.build");
    let first = s.execute("model.issues", &json!({"limit": 1})).expect("issues");
    let t = std::time::Instant::now();
    let again = s.execute("model.issues", &json!({"limit": 1, "cursor": 0})).expect("issues again");
    let cached = t.elapsed();
    assert_eq!(first["total"], again["total"]);
    assert_eq!(first["byKind"], again["byKind"]);
    // A change to the model: found again.
    s.execute("line", &json!({"points": [[1000, 1000], [1010, 1000]]})).expect("line");
    let t = std::time::Instant::now();
    s.execute("model.issues", &json!({"limit": 1})).expect("issues after a change");
    assert!(cached <= t.elapsed() * 4 + std::time::Duration::from_millis(5), "a page of the same revision is read from the cache: {cached:?}");
}
