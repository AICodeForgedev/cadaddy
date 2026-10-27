//! The plans-to-model benchmark: every case run through the agent's pipeline with the heuristic
//! drafter (as CI does), and the scorer itself on perfect and deliberately broken models.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::bspec::made_level;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::cmd::p2meval::{Case, cases, perfect, score};
use cadaddy_engine::cmd::wallgen::made_as;
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: Some(|d, sp, o| cadaddy_io::pdf::plot(d, sp, o).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

/// The handles of a session's built `part`s (on `level`, or every level).
fn built(s: &Session, part: &str, level: Option<&str>) -> Vec<String> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some(part))
        .filter(|e| level.is_none_or(|l| made_level(e).as_deref() == Some(l)))
        .map(|e| e.handle.hex())
        .collect()
}

fn at(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or_else(|| panic!("no {k} in {v}"))
}

fn multi_case() -> Case {
    cases().into_iter().find(|c| c.name == "multi").unwrap()
}

fn case<'a>(r: &'a Value, name: &str) -> &'a Value {
    r["cases"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("{name}: {r}"))
}

#[test]
fn the_heuristic_pipeline_scores_every_case() {
    hooks();
    let mut s = Session::new();
    let t = std::time::Instant::now();
    let r = s.execute("p2m.bench", &json!({})).unwrap();
    eprintln!("p2m.bench in {:?}: {}", t.elapsed(), serde_json::to_string_pretty(&r).unwrap());
    for name in ["single", "multi", "messy", "pdf"] {
        let c = case(&r, name);
        assert!(c.get("skipped").is_none() && c.get("error").is_none(), "{c}");
        // Every step ran.
        let failed: Vec<&Value> = c["steps"].as_array().unwrap().iter().filter(|st| st["ok"] != json!(true)).collect();
        assert!(failed.is_empty(), "{name}: {failed:?}");
        // Each measure against the truth, not just the total.
        let sc = &c["scores"];
        let at = |k: &str| sc[k].as_f64().unwrap_or_else(|| panic!("{name}: no {k} in {sc}"));
        assert!(at("walls") >= 0.9, "{name} walls: {sc}");
        assert_eq!(at("openings"), 1.0, "{name} openings: {sc}");
        assert!(at("rooms") >= 0.9, "{name} rooms: {sc}");
        assert_eq!(at("heights"), 1.0, "{name} heights: {sc}");
        if name == "multi" {
            assert_eq!(at("stairs"), 1.0, "{name} stairs: {sc}");
        } else {
            assert!(sc.get("stairs").is_none(), "{name}: {sc}");
        }
    }
    assert!(r["mean"].as_f64().unwrap() >= 0.85, "{}", r["mean"]);
    // One case at a time.
    let one = s.execute("p2m.bench", &json!({"cases": ["single"]})).unwrap();
    assert_eq!(one["cases"].as_array().unwrap().len(), 1);
}

#[test]
fn the_scorer_on_perfect_and_broken_models() {
    hooks();
    for c in cases().iter().filter(|c| !c.pdf) {
        let s = perfect(c).unwrap();
        let (sc, total) = score(&s, &c.truth);
        assert!(total >= 0.95, "{}: a correct model scores {total}: {sc}", c.name);
        for (k, v) in sc.as_object().unwrap() {
            assert!(v.as_f64().unwrap() >= 0.9, "{} {k}: {v}", c.name);
        }
    }
    let all = cases();
    let multi = all.iter().find(|c| c.name == "multi").unwrap();
    let base = perfect(multi).unwrap();
    let (good, _) = score(&base, &multi.truth);
    // Broken: a wrong floor to floor and no stair (rebuilt from the changed spec), then half
    // the walls and a door gone.
    let mut s = perfect(multi).unwrap();
    s.execute("bspec.set", &json!({"spec": {"levels": [{"name": "Level 2", "elevation_ft": 15}], "stairs": [{"name": "S1", "remove": true}]}}))
        .unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let walls = built(&s, "wall", None);
    s.execute("erase", &json!({"handles": walls[..walls.len() / 2].to_vec()})).unwrap();
    let door: Vec<String> = built(&s, "door", None).into_iter().take(1).collect();
    s.execute("erase", &json!({"handles": door})).unwrap();
    let (bad, total) = score(&s, &multi.truth);
    assert!(at(&bad, "walls") < at(&good, "walls") - 0.3, "{bad} vs {good}");
    assert!(at(&bad, "openings") <= 0.75, "{bad}");
    assert!(at(&bad, "heights") < 0.9, "{bad}");
    assert_eq!(at(&bad, "stairs"), 0.0);
    assert!(total < 0.7, "{total}");
    // An empty drawing scores nothing, and the commands check their cases.
    let empty = Session::new();
    assert!(score(&empty, &multi.truth).1 < 0.2);
    let mut s = Session::new();
    assert!(s.execute("p2m.score", &json!({"case": "nope"})).is_err());
    let r = s.execute("p2m.case", &json!({"name": "single"})).unwrap();
    assert!(r.get("truth").is_none(), "the agent must not get the answers: {r}");
    assert!(s.doc().unwrap().model.len() > 5, "the case opened");
    let sc = s.execute("p2m.score", &json!({"case": "single"})).unwrap();
    assert!(sc["total"].as_f64().unwrap() < 0.5, "nothing built yet: {sc}");
}

#[test]
fn every_level_is_scored() {
    hooks();
    let multi = multi_case();
    let (good, _) = score(&perfect(&multi).unwrap(), &multi.truth);
    assert_eq!(at(&good, "openings"), 1.0, "{good}");
    // Only Level 2's walls gone: the walls measure sees the missing storey.
    let mut s = perfect(&multi).unwrap();
    let upper = built(&s, "wall", Some("Level 2"));
    assert!(!upper.is_empty() && upper.len() < built(&s, "wall", None).len(), "Level 2 has walls of its own");
    s.execute("erase", &json!({"handles": upper})).unwrap();
    let (sc, _) = score(&s, &multi.truth);
    assert!(at(&sc, "walls") <= at(&good, "walls") - 0.3, "{sc} vs {good}");
    // ... and its height can't be read from walls that aren't there.
    assert!(at(&sc, "heights") <= 0.5 + 1e-9, "{sc}");
    // Only a Level 2 door gone.
    let mut s = perfect(&multi).unwrap();
    let door = built(&s, "door", Some("Level 2"));
    assert_eq!(door.len(), 1, "one door on Level 2");
    s.execute("erase", &json!({"handles": door})).unwrap();
    let (sc, _) = score(&s, &multi.truth);
    assert!(at(&sc, "openings") <= at(&good, "openings") - 0.25, "{sc} vs {good}");
    assert_eq!(at(&sc, "walls"), at(&good, "walls"), "{sc}");
}

#[test]
fn stairs_and_heights_come_from_the_built_model() {
    hooks();
    let multi = multi_case();
    // The built stair erased, the spec still has it: no stair.
    let mut s = perfect(&multi).unwrap();
    let stair = built(&s, "stair", None);
    assert_eq!(stair.len(), 1, "the perfect model has its stair");
    s.execute("erase", &json!({"handles": stair})).unwrap();
    let spec = s.execute("bspec.get", &json!({})).unwrap();
    assert!(spec.to_string().contains("S1"), "the spec still has the stair: {spec}");
    let (sc, _) = score(&s, &multi.truth);
    assert_eq!(at(&sc, "stairs"), 0.0, "{sc}");
    assert_eq!(at(&sc, "heights"), 1.0, "{sc}");
    // A stair on the wrong level doesn't count for this one.
    let mut truth = multi.truth.clone();
    truth.stairs = vec![("Level 2".into(), 19)];
    let (sc, _) = score(&perfect(&multi).unwrap(), &truth);
    assert_eq!(at(&sc, "stairs"), 0.0, "{sc}");
    // The wrong riser count scores less.
    let mut truth = multi.truth.clone();
    truth.stairs = vec![("Level 1".into(), 38)];
    let (sc, _) = score(&perfect(&multi).unwrap(), &truth);
    assert!((at(&sc, "stairs") - 0.5).abs() < 1e-9, "{sc}");
    // No walls built at all: the spec's levels score no heights.
    let mut s = perfect(&multi).unwrap();
    s.execute("erase", &json!({"handles": built(&s, "wall", None)})).unwrap();
    let (sc, _) = score(&s, &multi.truth);
    assert_eq!(at(&sc, "heights"), 0.0, "{sc}");
}

#[test]
fn a_missing_ceiling_breaks_rooms() {
    hooks();
    let single = cases().into_iter().find(|c| c.name == "single").unwrap();
    let mut s = perfect(&single).unwrap();
    let ceilings = built(&s, "ceiling", None);
    assert_eq!(ceilings.len(), 2, "two rooms, two ceilings");
    s.execute("erase", &json!({"handles": ceilings[..1].to_vec()})).unwrap();
    let (sc, _) = score(&s, &single.truth);
    assert!(at(&sc, "rooms") <= 0.6, "{sc}");
}

#[test]
fn the_case_keeps_its_truth_unless_asked() {
    hooks();
    let mut s = Session::new();
    let r = s.execute("p2m.case", &json!({"name": "single"})).unwrap();
    assert!(r.get("truth").is_none(), "{r}");
    assert_eq!(r["name"], json!("single"));
    let r = s.execute("p2m.case", &json!({"name": "multi", "with_truth": true})).unwrap();
    let t = &r["truth"];
    assert_eq!(t["doors"], json!(2), "{t}");
    assert_eq!(t["levels"].as_array().unwrap().len(), 2, "{t}");
    assert_eq!(t["levels"][1]["name"], json!("Level 2"), "{t}");
    assert_eq!(t["stairs"][0], json!({"level": "Level 1", "risers": 19}), "{t}");
    // Written to a file: still no truth.
    let dir = std::env::temp_dir().join(format!("p2m-case-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("single.dxf");
    let r = s.execute("p2m.case", &json!({"name": "single", "path": path.to_string_lossy()})).unwrap();
    assert!(r.get("truth").is_none() && path.exists(), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}
