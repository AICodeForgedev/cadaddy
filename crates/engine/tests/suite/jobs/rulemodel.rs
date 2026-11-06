//! Q5248 rule-based BIM on a synthetic 2-level job (architecture, mechanical, plumbing and
//! electrical runs, a short spec): the rules evaluate every element of the built model, raise
//! exactly the planted findings, the clash loop settles what the yield rules decide, and a
//! scripted agent using only direct model actions reaches 0 open findings and the known-correct
//! model; a re-run asks nothing again.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

/// Two levels, each a 40 x 30 ft building of typed 6 in walls; level 2 has one interior wall
/// on a layer with no wall type (planted). Runs: documented supply ducts on both levels, a
/// guessed supply duct on level 1 (planted), a conduit crossing level 1's duct (the yield rules
/// settle it) and a return duct crossing level 2's supply duct at the same height (a judgement
/// call: both are ducts).
fn job() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    for x0 in [0.0, 2000.0] {
        rect(&mut s, [x0 - 6.0, -6.0], [x0 + 486.0, 366.0]);
        rect(&mut s, [x0, 0.0], [x0 + 480.0, 360.0]);
    }
    layer(&mut s, "A-WALL-NT");
    s.execute("line", &json!({"points": [[2240, 0], [2240, 360]]})).unwrap();
    s.execute("line", &json!({"points": [[2246, 0], [2246, 360]]})).unwrap();
    let tag = |system: &str, level: &str, points: Value, size: [f64; 2], bottom: f64, documented: bool| {
        let mut r = json!({"system": system, "level": level, "points": points, "size_in": size, "bottoms_ft": [bottom]});
        if documented {
            r["size_source"] = json!("tag");
            r["height_source"] = json!("tag");
        }
        r
    };
    let runs = vec![
        tag("M-SA", "L1", json!([[24, 60], [400, 60]]), [14.0, 10.0], 9.0, true),
        tag("M-SA", "L1", json!([[24, 200], [400, 200]]), [12.0, 12.0], 9.0, false),
        tag("E-PWR", "L1", json!([[200, 20], [200, 100]]), [1.0, 0.0], 9.2, true),
        tag("P-DCW", "L1", json!([[24, 300], [400, 300]]), [1.0, 0.0], 9.5, true),
        tag("M-SA", "L2", json!([[2024, 60], [2400, 60]]), [14.0, 10.0], 9.0, true),
        tag("M-RA", "L2", json!([[2200, 20], [2200, 100]]), [12.0, 10.0], 9.0, true),
    ];
    let runs: Vec<Value> = runs
        .into_iter()
        .enumerate()
        .map(|(i, mut r)| {
            r["id"] = json!(format!("R{}", i + 1));
            r
        })
        .collect();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 8, "region": [-100, -100, 1000, 1000], "base": [0, 0]},
                {"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 8, "region": [1900, -100, 3000, 1000], "base": [2000, 0]}
            ],
            "wall_types": [{"name": "Partition", "thickness_in": 6}],
            "layers": [
                {"layer": "A-WALL", "role": "wall", "wall_type": "Partition"},
                {"layer": "A-WALL-NT", "role": "wall"}
            ],
            "mep": {
                "systems": [
                    {"name": "M-SA", "layer": "M-SA", "kind": "duct", "size_in": [12, 12], "bottom_ft": 9, "duct_type": "supply"},
                    {"name": "M-RA", "layer": "M-RA", "kind": "duct", "size_in": [12, 12], "bottom_ft": 9, "duct_type": "return"},
                    {"name": "E-PWR", "layer": "E-PWR", "kind": "conduit", "size_in": [1, 0], "bottom_ft": 9},
                    {"name": "P-DCW", "layer": "P-DCW", "kind": "pipe", "size_in": [1, 0], "bottom_ft": 9.5}
                ],
                "runs": runs
            }
        }}),
    )
    .unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// Every object the model build made.
fn model_objects(s: &Session) -> usize {
    s.doc().unwrap().model.iter().filter(|e| e.common.xdata.iter().any(|t| t.code == 1001 && t.value == "CADADDY_WALLGEN")).count()
}

/// The open findings as (kind, level, rule) — the rule from the detail's "Rule <id>".
fn open_findings(s: &mut Session) -> Vec<(String, String, String)> {
    let w = s.execute("rules.worklist", &json!({"limit": 100})).unwrap();
    let mut v: Vec<(String, String, String)> = w["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let rule = f["detail"].as_str().unwrap().strip_prefix("Rule ").and_then(|t| t.split_whitespace().next()).unwrap_or("").to_string();
            (f["kind"].as_str().unwrap().to_string(), f["level"].as_str().unwrap().to_string(), rule)
        })
        .collect();
    v.sort();
    v
}

fn elements(s: &mut Session) -> Vec<Value> {
    let r = s.execute("rules.action", &json!({"action": "select", "params": {"limit": 100}})).unwrap();
    r["delta"]["result"]["items"].as_array().unwrap().clone()
}

fn run(s: &mut Session, id: &str) -> Value {
    let l = s.execute("mep.list", &json!({"limit": 200})).unwrap();
    l["items"].as_array().unwrap().iter().find(|r| r["id"] == json!(id)).cloned().unwrap()
}

fn approx(a: &Value, b: f64) -> bool {
    a.as_f64().is_some_and(|v| (v - b).abs() < 1e-3)
}

#[test]
fn the_rules_evaluate_every_model_element_and_raise_exactly_the_planted_findings() {
    let mut s = job();
    let n = model_objects(&s);
    assert!(n >= 15, "the build made the model: {n}");
    let r = s.execute("rules.evaluate", &json!({})).unwrap();
    // Real model elements, not rule definitions.
    assert_eq!(r["elements"].as_u64().unwrap() as usize, n, "{r}");
    let open = open_findings(&mut s);
    let want: Vec<(String, String, String)> = vec![
        ("completeness".into(), "L1".into(), String::new()),
        ("completeness".into(), "L2".into(), String::new()),
        ("low_confidence".into(), "L1".into(), "mech-assumed".into()),
        ("missing_input".into(), "L2".into(), "arch-wall-type".into()),
    ];
    assert_eq!(open, want);
    // Rules alone against the known-correct model: an element is judged right when the rules
    // flag it exactly when it is one of the planted wrong ones (the guessed duct, the untyped
    // wall).
    let w = s.execute("rules.worklist", &json!({"limit": 100})).unwrap();
    let flagged: Vec<String> = w["items"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|f| f["elements"].as_array().unwrap().iter().map(|e| e.as_str().unwrap().to_string()))
        .collect();
    let els = elements(&mut s);
    let planted = |e: &Value| e["inputs"]["run"] == json!("R2") || (e["category"] == json!("wall") && e["inputs"].get("type").is_none());
    let right = els.iter().filter(|e| flagged.contains(&e["id"].as_str().unwrap().to_string()) == planted(e)).count();
    assert_eq!(els.iter().filter(|e| planted(e)).count(), 2);
    assert!(right as f64 / els.len() as f64 >= 0.95, "{right} of {}", els.len());
    assert_eq!(right, els.len());
    // Every element says which rule placed it, with what inputs and how sure.
    for e in &els {
        assert!(e["ruleId"].as_str().is_some_and(|r| !r.is_empty() && r != "none"), "{e}");
        assert!(e["confidence"].as_f64().is_some_and(|c| (0.0..=1.0).contains(&c)));
    }
    let rep = s.execute("rules.report", &json!({})).unwrap();
    assert_eq!(rep["elements"].as_u64().unwrap() as usize, n);
    let duct = rep["lines"].as_array().unwrap().iter().find(|l| l["category"] == json!("duct")).unwrap();
    assert_eq!(duct["count"], json!(4));
    assert_eq!(duct["openAssumptions"], json!(1));
    let score = s.execute("rules.score", &json!({})).unwrap();
    assert!(score["overall"].as_f64().unwrap() >= 0.9);
    assert_eq!(score["audit"]["lowConfidence"].as_array().unwrap().len(), 1, "{score}");
}

#[test]
fn the_clash_loop_settles_what_the_yield_rules_decide_and_asks_the_rest() {
    let mut s = job();
    let c = s.execute("rules.clashloop", &json!({})).unwrap();
    assert_eq!(c["before"], json!(2), "{c}");
    assert_eq!(c["auto_resolved"], json!(1), "{c}");
    assert_eq!(c["remaining"], json!(1), "{c}");
    // Conduit yields to duct: below it, clear by an inch.
    let r3 = run(&mut s, "R3");
    assert!(approx(&r3["bottoms_ft"][0], 9.0 - 1.0 / 12.0 - 1.0 / 12.0), "{r3}");
    let left = s.execute("model.clashes", &json!({})).unwrap();
    assert_eq!(left["total"], json!(1), "{left}");
    // Two ducts at one height: equal bands, a judgement call for the agent, with options.
    let w = s.execute("rules.worklist", &json!({"kind": "judgement"})).unwrap();
    let f = &w["items"][0];
    assert_eq!(f["level"], json!("L2"));
    assert!(f["alternatives"].as_array().unwrap().len() >= 3);
}

#[test]
fn a_scripted_agent_with_direct_actions_reaches_zero_open_findings_and_the_known_model() {
    let mut s = job();
    s.execute("rules.evaluate", &json!({})).unwrap();
    s.execute("rules.clashloop", &json!({})).unwrap();
    for _ in 0..20 {
        let w = s.execute("rules.worklist", &json!({"limit": 1})).unwrap();
        let Some(f) = w["items"].as_array().unwrap().first().cloned() else { break };
        let id = f["id"].as_str().unwrap();
        let p = match f["kind"].as_str().unwrap() {
            "missing_input" => {
                json!({"action": "swap_type", "finding": id, "params": {"type": "Partition"}, "citation": "drawing A-101 wall type legend"})
            }
            "low_confidence" => {
                let el = f["elements"][0].as_str().unwrap();
                let sel = s.execute("rules.action", &json!({"action": "select", "params": {"ids": [el]}})).unwrap();
                let run = sel["delta"]["result"]["items"][0]["inputs"]["run"].as_str().unwrap().to_string();
                s.execute("rules.action", &json!({"action": "resize", "target": run, "params": {"size_in": [12, 8]}})).unwrap();
                json!({"action": "change_elevation", "target": run, "finding": id, "params": {"bottom_ft": 9.5}, "citation": "schedule M-601"})
            }
            "judgement" => json!({"action": "change_elevation", "target": "R6", "finding": id, "params": {"bottom_ft": 10.0}}),
            "completeness" => json!({"action": "accept", "finding": id, "citation": "drawing A-101: open office, no doors"}),
            k => panic!("unexpected finding {k}: {f}"),
        };
        s.execute("rules.action", &p).unwrap();
        let after = s.execute("rules.worklist", &json!({})).unwrap()["total"].as_u64().unwrap();
        assert!(after < w["total"].as_u64().unwrap(), "{f}");
    }
    let c = s.execute("rules.clashloop", &json!({})).unwrap();
    assert_eq!(c["remaining"], json!(0), "{c}");
    let r = s.execute("rules.evaluate", &json!({})).unwrap();
    assert_eq!(r["open"], json!(0), "{r}");
    assert!(approx(&r["score"], 1.0), "{r}");
    // The known-correct model.
    let r2 = run(&mut s, "R2");
    assert_eq!(r2["size_in"], json!([12.0, 8.0]));
    assert!(approx(&r2["bottoms_ft"][0], 9.5));
    assert!(approx(&run(&mut s, "R6")["bottoms_ft"][0], 10.0));
    assert!(approx(&run(&mut s, "R3")["bottoms_ft"][0], 9.0 - 2.0 / 12.0));
    assert!(elements(&mut s).iter().filter(|e| e["category"] == json!("wall")).all(|e| e["inputs"]["type"] == json!("Partition")));
    assert_eq!(s.execute("model.clashes", &json!({})).unwrap()["total"], json!(0));
    // A re-run (and a rebuild) asks nothing again.
    s.execute("model.build", &json!({})).unwrap();
    let again = s.execute("rules.evaluate", &json!({})).unwrap();
    assert_eq!(again["open"], json!(0), "{again}");
    assert_eq!(s.execute("rules.clashloop", &json!({})).unwrap()["remaining"], json!(0));
    let rep = s.execute("rules.report", &json!({})).unwrap();
    assert!(rep["lines"].as_array().unwrap().iter().any(|l| l["pctAgentDecided"].as_f64().unwrap() > 0.0), "{rep}");
}

#[test]
fn saved_answers_become_project_rules_and_are_not_asked_again() {
    let mut s = job();
    s.execute("rules.evaluate", &json!({})).unwrap();
    s.execute("rules.clashloop", &json!({})).unwrap();
    // Accept the duct-on-duct clash as field coordination, kept as a rule.
    let w = s.execute("rules.worklist", &json!({"kind": "judgement"})).unwrap();
    let id = w["items"][0]["id"].as_str().unwrap().to_string();
    s.execute("rules.action", &json!({"action": "accept", "finding": id, "save_as_rule": true})).unwrap();
    // A rule written directly: walls on L2 needn't carry a type.
    s.execute(
        "rules.action",
        &json!({"action": "write_rule", "params": {"rule": {"id": "l2-walls-untyped-ok", "condition": {"category": "wall", "properties": {"level": "L2"}}, "action": {"kind": "waive", "value": "arch-wall-type"}}}}),
    )
    .unwrap();
    let packs = s.execute("rules.packs", &json!({})).unwrap();
    assert_eq!(packs["projectRules"], json!(2), "{packs}");
    let c = s.execute("rules.clashloop", &json!({})).unwrap();
    assert_eq!(c["remaining"], json!(0), "{c}");
    s.execute("rules.evaluate", &json!({})).unwrap();
    let open = open_findings(&mut s);
    assert!(!open.iter().any(|(k, _, _)| k == "judgement" || k == "missing_input"), "{open:?}");
    // Rebuilt from the prints: still not asked.
    s.execute("model.build", &json!({})).unwrap();
    s.execute("rules.evaluate", &json!({})).unwrap();
    assert!(!open_findings(&mut s).iter().any(|(k, _, _)| k == "missing_input"));
}

#[test]
fn an_owner_edited_pack_replaces_the_built_in_one() {
    let mut s = job();
    let packs = s.execute("rules.packs", &json!({})).unwrap();
    assert!(packs["loaded"].as_array().unwrap().len() >= 7);
    // Mechanical: an assumed duct is fine on this job.
    let pack = json!({"id": "mechanical-v1", "label": "Mechanical (project)", "trade": "mechanical",
        "rules": [{"id": "mech-all", "priority": "spec", "condition": {"trade": "mechanical"}, "action": {"kind": "classify"}, "confidence": 0.9}]});
    let r = s.execute("rules.packs", &json!({ "pack": pack })).unwrap();
    assert!(r["loaded"].as_array().unwrap().iter().any(|p| p["id"] == json!("mechanical-v1") && p["edited"] == json!(true)));
    s.execute("rules.evaluate", &json!({})).unwrap();
    assert!(!open_findings(&mut s).iter().any(|(k, _, _)| k == "low_confidence"));
    s.execute("rules.packs", &json!({"remove": "mechanical-v1"})).unwrap();
    s.execute("rules.evaluate", &json!({})).unwrap();
    assert!(open_findings(&mut s).iter().any(|(k, _, _)| k == "low_confidence"));
    assert!(s.execute("rules.packs", &json!({"pack": {"id": "", "rules": []}})).is_err());
}

#[test]
fn the_rules_state_survives_the_review_queue_saving_beside_it() {
    let mut s = job();
    s.execute("rules.evaluate", &json!({})).unwrap();
    let before = s.execute("rules.worklist", &json!({})).unwrap()["total"].clone();
    s.execute("project.note", &json!({"text": "rules evaluated"})).unwrap();
    s.execute("bim.build", &json!({"force": true})).unwrap();
    assert_eq!(s.execute("rules.worklist", &json!({})).unwrap()["total"], before);
}

#[test]
fn rules_on_an_empty_drawing_and_hostile_params_never_panic() {
    let mut s = Session::new();
    let r = s.execute("rules.evaluate", &json!({})).unwrap();
    assert_eq!(r["elements"], json!(0));
    let mut s = job();
    let hostile = [
        json!(null),
        json!({"threshold": "x", "level": 7, "trade": [1]}),
        json!({"threshold": 1e308, "cursor": -1, "limit": 1e20}),
        json!({"action": "move", "target": "ZZZ", "params": {"dx": 1e300}}),
        json!({"action": "move", "target": "106", "params": {"dx": f64::MAX}}),
        json!({"action": "resize", "target": "R1", "params": {"size_in": [1e9, -1]}}),
        json!({"action": "reroute", "target": "R1", "params": {"points": [[0, 0]]}}),
        json!({"action": "split", "target": "R1", "params": {"at": 999999}}),
        json!({"action": "join", "target": "R1", "params": {"with": "R1"}}),
        json!({"action": "add", "params": {"system": "nope", "level": "L1", "points": [[0, 0], [1, 1]]}}),
        json!({"action": "write_rule", "params": {"rule": {"id": "", "condition": 5}}}),
        json!({"action": "accept", "finding": "nope"}),
        json!({"action": "teach_symbol"}),
        json!({"action": "bogus"}),
        json!({"pack": {"id": "x", "rules": [{"id": "r", "priority": "spec", "condition": {}, "action": {"kind": "check"}, "confidence": 7}]}}),
        json!({"remove": "not-there"}),
        json!({"max_iterations": 1e9, "sample": -5}),
    ];
    for cmd in ["rules.packs", "rules.evaluate", "rules.worklist", "rules.action", "rules.clashloop", "rules.score", "rules.report"] {
        for p in &hostile {
            let _ = s.execute(cmd, p);
        }
    }
    // Still works after all that.
    assert!(s.execute("rules.evaluate", &json!({})).unwrap()["elements"].as_u64().unwrap() > 0);
}
