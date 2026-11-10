//! Q6504 fix-clash-vs-framing-tests-nothing: `bim.clash` tests the MEP against the framing the
//! trades lay out (joists, I-joists, studs, trusses) and judges each hit by the member's hole
//! rules: an allowed bored hole or a real conflict.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

use crate::conduitroute::storey_walls;

/// The stud or joist pieces of `level` (joists: the level whose floor they frame): their boxes
/// (inches, as built: stacked).
fn pieces(s: &Session, kind: &str, level: &str) -> Vec<([f64; 3], [f64; 3])> {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter(|e| {
            cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("atom")
                && e.common.xdata.iter().any(|t| t.value == format!("part:atom:{kind}"))
                && e.common.xdata.iter().any(|t| t.value == level)
        })
        .filter_map(|e| {
            let cadaddy_engine::doc::EntityKind::Mesh(m) = &e.kind else { return None };
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for v in &m.vertices {
                for (i, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                    lo[i] = lo[i].min(c);
                    hi[i] = hi[i].max(c);
                }
            }
            Some((lo, hi))
        })
        .collect()
}

fn system(name: &str, kind: &str, size: f64) -> Value {
    json!({"name": name, "layer": name, "kind": kind, "size_in": [size, 0], "bottom_ft": 9})
}

fn run(system: &str, a: [f64; 2], b: [f64; 2], size: f64, bottom_in: f64) -> Value {
    json!({"system": system, "level": "L1", "points": [a, b], "size_in": [size, 0.0], "bottoms_ft": [bottom_in / 12.0]})
}

/// Two stacked storeys (10' floor to floor), 2x6 wood stud exterior walls, the floor over L1
/// framed with I-joists at 24" (a project rule, as a joist layout would give it), and `runs`.
fn framed(runs: Vec<Value>) -> Session {
    let mut d = Drawing::new_imperial();
    storey_walls(&mut d, 0.0);
    storey_walls(&mut d, 200.0);
    let mut s = Session::empty();
    s.open_drawing(d, "Sample Floor Plan", None);
    s.execute("project.rules.set", &json!({"domain": "joist_layout", "key": "general", "value": "24", "reason": "sample joist layout"}))
        .expect("rule");
    let exterior = json!({
        "tag": "X1", "description": "EXTERIOR WALL", "source": "sample legend, mark X1",
        "layers": [
            {"material": "1/2\" wood structural panel sheathing", "thickness_in": 0.5, "category": "sheathing", "side": "side1"},
            {"material": "2x6 wood studs @ 24\" o.c.", "thickness_in": 5.5, "category": "framing", "side": "core"},
            {"material": "1/2\" gypsum board", "thickness_in": 0.5, "category": "board", "side": "side2"}
        ]
    });
    let spec = json!({
        "version": 1, "units_per_foot": 12, "stack": true,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 10, "ceiling_ft": 8, "region": [-100, -100, 1000, 1000], "base": [0, 0]},
            {"name": "L2", "elevation_ft": 10, "height_ft": 10, "ceiling_ft": 9, "region": [1900, -100, 3400, 1000], "base": [2400, 0]}
        ],
        "layers": [{"layer": "A-WALL", "role": "wall", "wall_type": "X1"}],
        "wall_types": [{"name": "X1", "thickness_in": 7.25, "from": "pairs", "assembly": exterior}],
        "mep": {"systems": [system("P-DCW", "pipe", 2.0), system("E-COND", "conduit", 1.0)], "runs": runs, "equipment": []}
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({})).expect("build");
    assert!(built["tradePieces"].as_u64().unwrap_or(0) > 0, "the build lays out the trades' pieces: {built}");
    s
}

#[test]
fn bim_clash_tests_the_framing_and_judges_each_hole_by_the_members_rules() {
    // First the bare building: where its joists and the south wall's studs stand.
    let s0 = framed(Vec::new());
    let joists = pieces(&s0, "joist", "L2");
    assert!(joists.len() > 10, "the floor over L1 is joisted: {}", joists.len());
    let (jz0, jz1) = (joists[0].0[2], joists[0].1[2]);
    // 11 7/8" I-joists under a 3/4" subfloor at 10'.
    assert!((jz1 - jz0 - 11.875).abs() < 0.01 && (jz1 - 119.25).abs() < 0.01, "{jz0} {jz1}");
    let south: Vec<_> = pieces(&s0, "stud", "L1").into_iter().filter(|(lo, hi)| hi[1] < 12.0 && lo[0] > 24.0 && hi[0] < 220.0).collect();
    assert!(south.len() >= 4, "studs in the south wall: {south:?}");
    let core_y = (south[0].0[1] + south[0].1[1]) / 2.0;
    let web = (jz0 + jz1) / 2.0;
    // A 2" water pipe across the joists at the web's middle (inside its hole zone: 1 1/2"
    // flanges, 1/8" of web kept, 2 3/8" hole far from the bearings); another one 2' south up in
    // the top flange; a 1" conduit along the south wall through its studs at 4'.
    let mut s = framed(vec![
        run("P-DCW", [30.0, 144.0], [450.0, 144.0], 2.0, web - 1.0),
        run("P-DCW", [30.0, 120.0], [450.0, 120.0], 2.0, jz1 - 2.25),
        run("E-COND", [30.0, core_y], [200.0, core_y], 1.0, 47.5),
    ]);

    let r = s.execute("bim.clash", &json!({"a": "framing"})).unwrap();
    let fr = &r["framing"];
    eprintln!("before/after: tested {} total {} framing {fr}", r["tested"], r["total"]);
    assert!(r["tested"].as_u64().unwrap_or(0) > 0, "the framing is tested: {}", short(&r));
    assert!(fr["members"].as_u64().unwrap_or(0) > joists.len() as u64, "{fr}");
    assert!(fr["tested"].as_u64().unwrap_or(0) > 0, "{fr}");
    let bores = r["bores"].as_array().cloned().unwrap_or_default();
    let clashes = r["clashes"].as_array().cloned().unwrap_or_default();
    // The web pipe: an allowed bore through every joist it crosses.
    let at_y = |v: &Value, y: f64| (v["at"][1].as_f64().unwrap_or(f64::NAN) - y).abs() < 3.0;
    let web_bores: Vec<&Value> = bores.iter().filter(|b| at_y(b, 144.0) && b["member"]["kind"] == json!("joist")).collect();
    assert!(web_bores.len() >= 15, "the web pipe bores the joists: {} of {}", web_bores.len(), short(&r));
    for b in &web_bores {
        assert_eq!(b["verdict"], json!("allowed"), "{b}");
        assert!(b["member"]["type"].as_str().unwrap_or("").contains("I-joist"), "{b}");
        assert!(b["hole_in"].as_f64().unwrap() > 2.0 && b["rule"].as_str().unwrap_or("").contains("web"), "{b}");
        assert!(b["source"].as_str().unwrap_or("").contains("I-joist"), "{b}");
    }
    assert!(!clashes.iter().any(|c| at_y(c, 144.0)), "an allowed bore is not a clash");
    // The flange pipe: a conflict at every joist, reported as a clash with its reason.
    let flange: Vec<&Value> = clashes.iter().filter(|c| at_y(c, 120.0)).collect();
    assert!(flange.len() >= 15, "the flange pipe is a conflict at each joist: {} of {}", flange.len(), short(&r));
    for c in &flange {
        assert_eq!(c["framing"]["verdict"], json!("conflict"), "{c}");
        assert!(c["framing"]["why"].as_str().unwrap_or("").contains("flange"), "{c}");
        assert_eq!(c["kind"], json!("hard"));
    }
    // The conduit: an allowed bore through each wood stud it passes (2x6, 1 3/8" hole).
    let stud_bores: Vec<&Value> = bores.iter().filter(|b| b["member"]["kind"] == json!("stud")).collect();
    assert!(stud_bores.len() >= 4, "the conduit bores the studs: {}", short(&r));
    for b in &stud_bores {
        assert_eq!(b["verdict"], json!("allowed"), "{b}");
        assert!(b["member"]["type"].as_str().unwrap_or("").contains("wood"), "{b}");
        assert!(b["rule"].as_str().unwrap_or("").contains("stud"), "{b}");
    }
    assert!(!clashes.iter().any(|c| c["b"]["category"] == json!("conduit") || c["a"]["category"] == json!("conduit")), "{}", short(&r));
    assert_eq!(fr["allowed"].as_u64().unwrap(), bores.len() as u64);
    assert_eq!(fr["conflicts"].as_u64().unwrap(), flange.len() as u64, "{fr}");

    // The same hits by trade and category name, and in the every-trade check.
    let by_kind = s.execute("bim.clash", &json!({"a": "plumbing", "b": "joists"})).unwrap();
    assert_eq!(by_kind["framing"]["conflicts"], fr["conflicts"], "{}", short(&by_kind));
    let all = s.execute("bim.clash", &json!({})).unwrap();
    let all_flange = all["clashes"].as_array().unwrap().iter().filter(|c| c["framing"]["verdict"] == json!("conflict")).count();
    assert_eq!(all_flange as u64, fr["conflicts"].as_u64().unwrap(), "{}", short(&all));
    assert!(all["bores"].as_array().map_or(0, Vec::len) >= web_bores.len() + stud_bores.len());

    // A conflict is fixed by moving the pipe (the joist stays) down out of the flange into the
    // web's hole zone, where it bores every joist it crosses as allowed.
    let id = flange[0]["id"].as_str().unwrap();
    let fix = s.execute("bim.clash.fix", &json!({"id": id})).unwrap();
    eprintln!("fix: {fix}");
    assert_eq!(fix["mover"]["category"], json!("pipe"), "{fix}");
    assert_eq!(fix["stays"]["category"], json!("joist"), "{fix}");
    assert_eq!(fix["how"], json!("into the hole zone"), "{fix}");
    let dz = fix["move"][2].as_f64().unwrap();
    assert!(dz < 0.0 && ((jz1 - 2.25 + 1.0 + dz) - web).abs() < 0.05, "to the web's middle: {fix}");
    let done = s.execute("bim.clash.fix", &json!({"id": id, "approve": true})).unwrap();
    assert_eq!(done["check"]["clear"], json!(true), "{done}");
    assert_eq!(done["check"]["new_clashes"], json!(0), "{done}");
    let again = s.execute("bim.clash", &json!({"a": "framing"})).unwrap();
    eprintln!("after the fix: framing {}", again["framing"]);
    assert_eq!(again["framing"]["conflicts"], json!(0), "{}", again["framing"]);
    assert_eq!(again["framing"]["allowed"].as_u64().unwrap(), fr["allowed"].as_u64().unwrap() + flange.len() as u64);
    assert!(again["resolved"].as_array().unwrap().iter().any(|r| r == &json!(id)), "{}", short(&again));
}

#[test]
fn no_framing_built_says_so_and_hostile_params_never_panic() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[0, 0], [480, 0], [480, 360], [0, 360]], "closed": true})).unwrap();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}],
            "mep": {"systems": [system("P-DCW", "pipe", 2.0)], "runs": [run("P-DCW", [24.0, 60.0], [300.0, 60.0], 2.0, 100.0)]}
        }}),
    )
    .unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("bim.clash", &json!({"a": "framing"})).unwrap();
    assert_eq!(r["framing"]["members"], json!(0), "{r}");
    assert!(r["framing"]["note"].as_str().unwrap_or("").contains("no framing"), "{r}");
    for p in [json!({"a": "framing", "tolerance": -1}), json!({"a": ["studs", 3], "clearance": 1e308}), json!({"a": "framing", "b": "framing"})] {
        let _ = s.execute("bim.clash", &p);
    }
}

fn short(v: &Value) -> String {
    v.to_string().chars().take(3_000).collect()
}
