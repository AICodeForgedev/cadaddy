//! Fidelity layers (Q6163): built elements get a tier from the rules data by what they are and
//! how big; the 3D view's level is Auto or fixed; a click on a piece the view draws only as part
//! of its run picks the run; counts and the takeoff are the same at every level; the project
//! edits the rules; the report measures what each tier holds and draws.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Handle;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn rect(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("pline", &json!({"vertices": [a, [b[0], a[1]], b, [a[0], b[1]]], "closed": true})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 4})).unwrap();
}

/// A made-up floor: walls, a 24x12 supply main, a 2" and a 3/4" water line (the small one round
/// a corner), a 1" conduit run.
fn built() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [-12.0, -12.0], [492.0, 372.0]);
    rect(&mut s, [-6.0, -6.0], [486.0, 366.0]);
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("pline", &json!({"vertices": [[24, 60], [264, 60]]})).unwrap();
    text(&mut s, [180.0, 70.0], "24x12 BOT @ 10'-0\"");
    layer(&mut s, "P-DOMW-CW");
    s.execute("line", &json!({"points": [[100, 0], [100, 120]]})).unwrap();
    text(&mut s, [104.0, 20.0], "2\" CW CL @ 10'-6\"");
    layer(&mut s, "P-DOMW-HW");
    s.execute("pline", &json!({"vertices": [[150, 200], [300, 200], [300, 320]]})).unwrap();
    text(&mut s, [200.0, 206.0], "3/4\" HW CL @ 9'-6\"");
    layer(&mut s, "E-COND");
    s.execute("line", &json!({"points": [[300, 300], [400, 300]]})).unwrap();
    text(&mut s, [340.0, 306.0], "1\" C BOT @ 10'-0\"");
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

fn hex(h: Handle) -> String {
    format!("{:X}", h.0)
}

/// The built elements whose layer starts with `prefix`.
fn on_layer(s: &Session, prefix: &str) -> Vec<Handle> {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).is_some() && e.common.layer.to_ascii_uppercase().starts_with(prefix))
        .map(|e| e.handle)
        .collect()
}

fn tiers_of(s: &mut Session, hs: &[Handle]) -> Vec<Value> {
    let of: Vec<String> = hs.iter().map(|h| hex(*h)).collect();
    let r = s.execute("view.fidelity", &json!({"of": of})).unwrap();
    r["elements"].as_array().unwrap().clone()
}

fn tier_of(s: &mut Session, prefix: &str) -> Vec<String> {
    let hs = on_layer(s, prefix);
    assert!(!hs.is_empty(), "nothing built on {prefix}");
    let mut t: Vec<String> = tiers_of(s, &hs).iter().map(|e| e["tier"].as_str().unwrap_or("none").to_string()).collect();
    t.sort();
    t.dedup();
    t
}

#[test]
fn tiers_by_kind_and_size() {
    let mut s = built();
    // A 24x12 duct is a main: building. The 2" line is mid-size: floor. The 3/4" line is small:
    // room, drawn as a line at floor scale. A conduit run: floor. Walls: floor.
    let d = s.doc().unwrap();
    let layers: std::collections::BTreeSet<String> =
        d.model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).is_some()).map(|e| e.common.layer.to_string()).collect();
    let duct = layers.iter().find(|l| l.to_ascii_uppercase().contains("DUCT")).cloned().unwrap_or_else(|| panic!("{layers:?}"));
    assert_eq!(tier_of(&mut s, &duct.to_ascii_uppercase()), vec!["building"], "{layers:?}");
    let hs = on_layer(&s, "");
    let els = tiers_of(&mut s, &hs);
    let find = |kind: &str, size: f64| -> Vec<String> {
        let mut t: Vec<String> = els
            .iter()
            .filter(|e| e["kind"] == kind && e["size_in"].as_f64().is_some_and(|z| (z - size).abs() < 1e-6))
            .map(|e| e["tier"].as_str().unwrap().to_string())
            .collect();
        t.sort();
        t.dedup();
        t
    };
    assert_eq!(find("pipe", 2.0), vec!["floor"], "{els:?}");
    assert_eq!(find("pipe", 0.75), vec!["room"]);
    assert!(els.iter().filter(|e| e["kind"] == "pipe" && e["size_in"] == json!(0.75)).all(|e| e["line_from"] == "floor"));
    assert_eq!(find("conduit", 1.0), vec!["floor"]);
    let walls: Vec<&Value> = els.iter().filter(|e| e["kind"] == "wall").collect();
    assert!(!walls.is_empty() && walls.iter().all(|e| e["tier"] == "floor"), "{walls:?}");
    // Every element names the rule that placed it.
    assert!(els.iter().filter(|e| e["tier"].is_string()).all(|e| e["rule"].is_string()));
}

#[test]
fn level_modes_and_marks() {
    let mut s = built();
    let r = s.execute("view.fidelity", &json!({})).unwrap();
    assert_eq!(r["level"], "auto");
    assert_eq!(r["shown"], Value::Null);
    assert_eq!(r["rules"]["source"], "default");
    let r = s.execute("view.fidelity", &json!({"level": "Room"})).unwrap();
    assert_eq!((r["level"].as_str(), r["shown"].as_str()), (Some("room"), Some("room")));
    let r = s.execute("view.fidelity", &json!({"level": "auto", "show": {"kind": "receptacle", "level": "L1"}})).unwrap();
    assert_eq!(r["level"], "auto");
    assert_eq!(r["show"], json!({"kind": "receptacle", "level": "L1"}));
    assert_eq!(s.execute("view.fidelity", &json!({"show": null})).unwrap()["show"], Value::Null);
    assert!(s.execute("view.fidelity", &json!({"level": "street"})).is_err());
    assert!(s.execute("view.fidelity", &json!({"show": {"level": "L1"}})).is_err());
    assert!(s.execute("view.fidelity", &json!({"of": "zz"})).is_err());
}

#[test]
fn a_coarse_click_picks_the_run() {
    let mut s = built();
    let small: Vec<Handle> = {
        let hs = on_layer(&s, "");
        let els = tiers_of(&mut s, &hs);
        hs.into_iter().zip(els).filter(|(_, e)| e["kind"] == "pipe" && e["size_in"] == json!(0.75)).map(|(h, _)| h).collect()
    };
    assert!(small.len() >= 2, "the 3/4\" line round its corner is pieces: {}", small.len());
    let first = small[0];
    // In plan (no 3D view): the piece.
    assert_eq!(cadaddy_engine::cmd::fidelity::pick(&s, first), vec![first]);
    // The 3D view at the floor tier draws the small line as one line: a click takes all of it.
    s.execute("bim.look", &json!({"on": true})).unwrap();
    s.execute("view.fidelity", &json!({"level": "floor"})).unwrap();
    let mut run = cadaddy_engine::cmd::fidelity::pick(&s, first);
    run.sort();
    let mut want = small.clone();
    want.sort();
    assert_eq!(run, want);
    // Zoomed in to the room tier: the piece alone.
    s.execute("view.fidelity", &json!({"level": "room"})).unwrap();
    assert_eq!(cadaddy_engine::cmd::fidelity::pick(&s, first), vec![first]);
    // A duct main is drawn at every tier: itself.
    s.execute("view.fidelity", &json!({"level": "building"})).unwrap();
    let wall = on_layer(&s, "").into_iter().find(|h| tiers_of(&mut s, &[*h])[0]["kind"] == "wall").unwrap();
    assert_eq!(cadaddy_engine::cmd::fidelity::pick(&s, wall), vec![wall]);
}

#[test]
fn counts_are_the_same_at_every_level() {
    let mut s = built();
    s.execute("bim.look", &json!({"on": true})).unwrap();
    let count = |s: &mut Session| {
        let pipes = s.execute("bim.props.find", &json!({"kind": "pipe", "limit": 1})).unwrap()["count"].clone();
        let take = s.execute("bim.takeoff", &json!({})).unwrap();
        (pipes, take["csv"].to_string(), take["totals"].to_string())
    };
    let before = count(&mut s);
    for level in ["building", "floor", "room", "detail", "auto"] {
        s.execute("view.fidelity", &json!({"level": level, "show": "pipe"})).unwrap();
        assert_eq!(count(&mut s), before, "{level}");
    }
}

#[test]
fn project_rules_change_tiers_and_undo() {
    let mut s = built();
    let duct = on_layer(&s, "").into_iter().find(|h| tiers_of(&mut s, &[*h])[0]["kind"] == "duct").unwrap();
    assert_eq!(tiers_of(&mut s, &[duct])[0]["tier"], "building");
    // The project says ducts are floor-tier whatever their size.
    let r = s
        .execute("view.fidelity.rules", &json!({"rules": [{"kinds": ["duct"], "tier": "floor", "why": "this job"}], "auto": {"room_ft": 20}}))
        .unwrap();
    assert_eq!(r["source"], "project");
    assert_eq!(r["auto"]["room_ft"], json!(20.0));
    assert_eq!(r["rules"].as_array().unwrap().len(), 1);
    assert_eq!(tiers_of(&mut s, &[duct])[0]["tier"], "floor");
    // Kept in the drawing, so undo takes it back.
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(tiers_of(&mut s, &[duct])[0]["tier"], "building");
    assert!(s.execute("view.fidelity.rules", &json!({"rules": [{"tier": "street"}]})).is_err());
    assert!(s.execute("view.fidelity.rules", &json!({"auto": {"room_ft": -1}})).is_err());
    s.execute("view.fidelity.rules", &json!({"default_tier": "room"})).unwrap();
    let r = s.execute("view.fidelity.rules", &json!({"reset": true})).unwrap();
    assert_eq!(r["source"], "default");
    assert!(r["rules"].as_array().unwrap().len() > 10);
}

#[test]
fn report_measures_each_tier() {
    let mut s = built();
    let r = s.execute("view.fidelity.report", &json!({"width": 400, "height": 300})).unwrap();
    let n = r["elements"].as_u64().unwrap();
    let per: u64 = ["building", "floor", "room", "detail"].iter().map(|t| r["tiers"][t]["elements"].as_u64().unwrap()).sum();
    assert_eq!(n, per, "{r}");
    assert!(r["tiers"]["building"]["elements"].as_u64().unwrap() >= 1);
    let at = &r["whole"]["at"];
    let drawn = |t: &str| at[t]["tris_drawn"].as_u64().unwrap();
    assert!(drawn("building") <= drawn("floor") && drawn("floor") <= drawn("detail"), "{at}");
    assert!(drawn("building") < drawn("detail"));
    assert!(r["build_ms"].as_f64().unwrap() >= 0.0);
}

#[test]
fn model_render_draws_a_tier() {
    let mut s = built();
    let tris = |s: &mut Session, f: &str| -> (u64, String) {
        let r = s.execute("model.render", &json!({"width": 320, "height": 200, "fidelity": f})).unwrap();
        (r["triangles"].as_u64().unwrap(), r["fidelity"].as_str().unwrap_or("").to_string())
    };
    let (b, bt) = tris(&mut s, "building");
    let (fl, _) = tris(&mut s, "floor");
    let (d, dt) = tris(&mut s, "detail");
    assert_eq!((bt.as_str(), dt.as_str()), ("building", "detail"));
    assert!(b < fl && fl <= d, "{b} {fl} {d}");
    // Without `fidelity`: everything, as before.
    let all = s.execute("model.render", &json!({"width": 320, "height": 200})).unwrap();
    assert!(all.get("fidelity").is_none());
    assert_eq!(all["triangles"].as_u64().unwrap(), d);
    assert!(s.execute("model.render", &json!({"width": 32, "height": 32, "fidelity": "street"})).is_err());
}
