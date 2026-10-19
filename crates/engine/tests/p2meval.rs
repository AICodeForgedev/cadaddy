//! The plans-to-model benchmark: every case run through the agent's pipeline with the heuristic
//! drafter (as CI does), and the scorer itself on perfect and deliberately broken models.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use cadaddy_engine::cmd::p2meval::{cases, perfect, score};
use cadaddy_engine::cmd::wallgen::made_as;
use serde_json::{Value, json};

fn hooks() {
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: Some(|d, sp, o| cadaddy_io::pdf::plot(d, sp, o).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
    });
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
        assert!(c["total"].as_f64().unwrap() >= 0.8, "{name} scored {c}");
    }
    // What the heuristics get exactly.
    assert_eq!(case(&r, "single")["scores"]["openings"], json!(1.0));
    assert_eq!(case(&r, "multi")["scores"]["heights"], json!(1.0));
    assert_eq!(case(&r, "multi")["scores"]["stairs"], json!(1.0));
    assert!(case(&r, "multi")["scores"]["rooms"].as_f64().unwrap() >= 0.9);
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
    // Broken: half the walls gone, a wrong floor to floor, no stair, a door missing.
    let mut s = perfect(multi).unwrap();
    let walls: Vec<String> = s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).map(|e| e.handle.hex()).collect();
    s.execute("erase", &json!({"handles": walls[..walls.len() / 2].to_vec()})).unwrap();
    let door: Vec<String> = s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("door")).map(|e| e.handle.hex()).take(1).collect();
    s.execute("erase", &json!({"handles": door})).unwrap();
    s.execute("bspec.set", &json!({"spec": {"levels": [{"name": "Level 2", "elevation_ft": 15}], "stairs": [{"name": "S1", "remove": true}]}}))
        .unwrap();
    let (bad, total) = score(&s, &multi.truth);
    let at = |v: &Value, k: &str| v[k].as_f64().unwrap();
    assert!(at(&bad, "walls") < at(&good, "walls") - 0.3, "{bad} vs {good}");
    assert!(at(&bad, "openings") <= 0.5, "{bad}");
    assert!(at(&bad, "heights") < 0.9, "{bad}");
    assert_eq!(at(&bad, "stairs"), 0.0);
    assert!(total < 0.7, "{total}");
    // An empty drawing scores nothing, and the commands check their cases.
    let empty = Session::new();
    assert!(score(&empty, &multi.truth).1 < 0.2);
    let mut s = Session::new();
    assert!(s.execute("p2m.score", &json!({"case": "nope"})).is_err());
    let r = s.execute("p2m.case", &json!({"name": "single"})).unwrap();
    assert_eq!(r["truth"]["doors"], json!(1));
    assert!(s.doc().unwrap().model.len() > 5, "the case opened");
    let sc = s.execute("p2m.score", &json!({"case": "single"})).unwrap();
    assert!(sc["total"].as_f64().unwrap() < 0.5, "nothing built yet: {sc}");
}
