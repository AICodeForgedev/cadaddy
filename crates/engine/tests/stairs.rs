//! Stairs and ramps from synthetic plans: a straight flight and a switchback with a landing
//! are detected (treads, direction from the UP note, width), their risers fall within the code
//! range, a ramp is found from its note (and SLOPE TO DRAIN or 50% OPEN outlines are not ramps),
//! the spec takes and confirms them and checks them against the commercial code (IBC) unless a
//! stair says `code: "irc"`, `model.build` builds steps, landings, handrails and 42" guards, and
//! Walk climbs the stair to the next level.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{EntityKind, Mesh};
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn line(s: &mut Session, a: [f64; 2], b: [f64; 2]) {
    s.execute("line", &json!({"points": [a, b]})).unwrap();
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 6})).unwrap();
}

/// A run of `treads` treads 11" deep and 44" wide from x0 along x (`dir` ±1) at y0..y0+44, with
/// its stringers.
fn run(s: &mut Session, x0: f64, y0: f64, treads: u32, dir: f64) {
    for i in 0..=treads {
        let x = x0 + dir * 11.0 * f64::from(i);
        line(s, [x, y0], [x, y0 + 44.0]);
    }
    let x1 = x0 + dir * 11.0 * f64::from(treads);
    line(s, [x0, y0], [x1, y0]);
    line(s, [x0, y0 + 44.0], [x1, y0 + 44.0]);
}

/// Two levels 12' apart (L2's plan in its own region far to the east); on L1: S1, a straight
/// flight of 18 treads (UP at its foot); S2, two runs of 9 turning on a landing; a ramp outline
/// with RAMP 1:12 and UP.
fn plan() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-FLOR-STRS");
    run(&mut s, 100.0, 0.0, 18, 1.0);
    text(&mut s, [70.0, 20.0], "UP");
    run(&mut s, 100.0, 200.0, 9, 1.0);
    text(&mut s, [70.0, 220.0], "UP");
    run(&mut s, 199.0, 250.0, 9, -1.0);
    s.execute("pline", &json!({"vertices": [[100, 400], [388, 400], [388, 436], [100, 436]], "closed": true})).unwrap();
    text(&mut s, [200.0, 415.0], "RAMP 1:12");
    text(&mut s, [110.0, 415.0], "UP");
    layer(&mut s, "A-WALL");
    line(&mut s, [5000.0, 0.0], [5100.0, 0.0]);
    // Not ramps: a toilet room floor sloped to its drain, and a 50% open screen.
    s.execute("pline", &json!({"vertices": [[1000, 0], [1096, 0], [1096, 120], [1000, 120]], "closed": true})).unwrap();
    text(&mut s, [1010.0, 60.0], "SLOPE TO DRAIN");
    s.execute("pline", &json!({"vertices": [[1200, 0], [1300, 0], [1300, 60], [1200, 60]], "closed": true})).unwrap();
    text(&mut s, [1210.0, 30.0], "50% OPEN");
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "region": [-500, -500, 2000, 2000], "base": [0, 0]},
                {"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "region": [4000, -500, 6000, 2000], "base": [4000, 0]}
            ],
            "layers": [{"layer": "A-FLOR-STRS", "role": "stair"}, {"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s
}

fn by_name<'a>(r: &'a Value, name: &str) -> &'a Value {
    r["stairs"].as_array().unwrap().iter().find(|x| x["name"] == name).unwrap_or_else(|| panic!("{name} in {r}"))
}

#[test]
fn stairs_and_a_ramp_are_detected_with_code_risers() {
    let mut s = plan();
    let r = s.execute("stairs.detect", &json!({})).unwrap();
    assert_eq!(r["stairs"].as_array().unwrap().len(), 3, "{r}");
    // S1: one run of 18 treads, climbing east from its UP note, 44" wide, 19 risers of 7.58".
    let s1 = by_name(&r, "S1");
    assert_eq!(s1["runs"], json!([{"from": [100.0, 22.0], "to": [298.0, 22.0], "treads": 18}]), "{s1}");
    assert_eq!((s1["width_in"].clone(), s1["risers"].clone(), s1["direction"].clone()), (json!(44.0), json!(19), json!("text")));
    let riser = s1["riser_in"].as_f64().unwrap();
    assert!((4.0..=7.75).contains(&riser), "{riser}");
    assert_eq!(s1["tread_in"], json!(11.0));
    assert!(s1["confidence"].as_f64().unwrap() >= 0.9);
    // S2: two runs, the second turned back west from beside the first one's head.
    let s2 = by_name(&r, "S2");
    let runs = s2["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2, "{s2}");
    assert_eq!(runs[0]["from"], json!([100.0, 222.0]));
    assert_eq!((runs[1]["from"].clone(), runs[1]["to"].clone()), (json!([199.0, 272.0]), json!([100.0, 272.0])));
    assert_eq!(s2["risers"], json!(20));
    assert!((4.0..=7.75).contains(&s2["riser_in"].as_f64().unwrap()));
    // The ramp: its long side, up from the UP note's end, 36" wide.
    let r1 = by_name(&r, "R1");
    assert_eq!(r1["ramp"], json!(true));
    assert_eq!(r1["runs"], json!([{"from": [100.0, 418.0], "to": [388.0, 418.0], "treads": 0}]));
    assert_eq!(r1["width_in"], json!(36.0));
    // Nothing stored yet.
    assert_eq!(s.execute("stairs.list", &json!({})).unwrap()["stairs"], json!([]));
}

#[test]
fn the_spec_takes_confirms_and_checks_them() {
    let mut s = plan();
    s.execute("stairs.detect", &json!({"store": true})).unwrap();
    let l = s.execute("stairs.list", &json!({})).unwrap();
    assert_eq!(l["stairs"].as_array().unwrap().len(), 3);
    assert_eq!(by_name(&l, "S1")["to"], json!("L2"));
    // S1's 19 risers of 7.58" break the commercial code's 7" (IBC 1011.5.2), the default.
    let w = by_name(&l, "S1")["warnings"].clone();
    assert_eq!(w.as_array().map(Vec::len), Some(1), "{w}");
    let w = w[0].as_str().unwrap().to_string();
    assert!(w.contains("19 risers of 7 5/8\"") && w.contains("the 7\" limit") && w.contains("IBC 1011.5.2"), "{w}");
    // The ramp climbs 12' over 24': far steeper than 1:12.
    let w = by_name(&l, "R1")["warnings"][0].as_str().unwrap().to_string();
    assert!(w.contains("steeper than 1:12"), "{w}");
    // Plain language.
    let parts = s.execute("bspec.get", &json!({})).unwrap()["parts"].clone();
    let p = parts.as_array().unwrap().iter().find(|p| p["key"] == "stairs/S1").unwrap()["text"].as_str().unwrap().to_string();
    assert_eq!(p, "S1: stair from L1 to L2, 1 run, 19 risers of 7 5/8\", 11\" treads, 44\" wide (detected; confirm)");
    // Confirm S1; detecting again keeps it and replaces the rest.
    assert_eq!(s.execute("stairs.confirm", &json!({"names": ["s1"]})).unwrap()["confirmed"], json!(["S1"]));
    assert!(s.execute("stairs.confirm", &json!({"names": ["S9"]})).is_err());
    assert!(s.execute("stairs.confirm", &json!({})).is_err());
    s.execute("stairs.detect", &json!({"store": true})).unwrap();
    let l = s.execute("stairs.list", &json!({})).unwrap();
    let names: Vec<&str> = l["stairs"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(by_name(&l, "S1")["confirmed"], json!(true));
    // A run too short for its rise: risers over 7¾".
    s.execute("bspec.set", &json!({"spec": {"stairs": [{"name": "S1", "runs": [{"from": [100, 22], "to": [210, 22], "treads": 10}]}]}})).unwrap();
    let w = by_name(&s.execute("stairs.list", &json!({})).unwrap(), "S1")["warnings"][0].as_str().unwrap().to_string();
    assert!(w.contains("11 risers of 13 1/8\"") && w.contains("7¾"), "{w}");
    // Broken stairs are errors.
    let v = s
        .execute(
            "bspec.validate",
            &json!({"spec": {"stairs": [{"name": "S1", "level": "L9"}, {"name": "S2", "runs": [{"from": [0, 0], "to": [0, 0], "treads": 0}]}]}}),
        )
        .unwrap();
    let paths: Vec<&str> = v["errors"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert!(paths.contains(&"stairs[0].level") && paths.contains(&"stairs[1].runs[0]") && paths.contains(&"stairs[1].runs[0].treads"), "{paths:?}");
}

#[test]
fn stairs_are_held_to_the_commercial_code_unless_they_say_irc() {
    let mut s = plan();
    s.execute("stairs.detect", &json!({"store": true})).unwrap();
    // The same S1 under the residential code (7¾", 10"): no warning.
    s.execute("bspec.set", &json!({"spec": {"stairs": [{"name": "S1", "code": "irc"}]}})).unwrap();
    let l = s.execute("stairs.list", &json!({})).unwrap();
    assert_eq!(by_name(&l, "S1")["warnings"], json!([]), "{l}");
    assert_eq!(by_name(&l, "S1")["code"], json!("irc"));
    // S2 (20 risers of 7.2") still has the default code, and warns.
    let w = by_name(&l, "S2")["warnings"][0].as_str().unwrap_or_default().to_string();
    assert!(w.contains("20 risers of 7 1/4\"") && w.contains("IBC 1011.5.2"), "{w}");
    // S1 re-specced to 7" or less (20 treads, 21 risers of 6.86") passes under the default.
    let mut s = plan();
    s.execute("stairs.detect", &json!({"store": true})).unwrap();
    s.execute("bspec.set", &json!({"spec": {"stairs": [{"name": "S1", "runs": [{"from": [100, 22], "to": [320, 22], "treads": 20}]}]}})).unwrap();
    let l = s.execute("stairs.list", &json!({})).unwrap();
    let s1 = by_name(&l, "S1");
    assert_eq!((s1["risers"].clone(), s1["warnings"].clone()), (json!(21), json!([])), "{s1}");
    assert!(s1.get("code").is_none(), "{s1}");
    // Treads under 11" warn under the default (10.5" is enough for IRC).
    s.execute("bspec.set", &json!({"spec": {"stairs": [{"name": "S1", "runs": [{"from": [100, 22], "to": [310, 22], "treads": 20}]}]}})).unwrap();
    let w = by_name(&s.execute("stairs.list", &json!({})).unwrap(), "S1")["warnings"].clone();
    assert_eq!(w.as_array().map(Vec::len), Some(1), "{w}");
    assert!(w[0].as_str().unwrap().contains("treads of 10 1/2\": under the 11\" minimum (IBC 1011.5.2"), "{w}");
    s.execute("bspec.set", &json!({"spec": {"stairs": [{"name": "S1", "code": "IRC"}]}})).unwrap();
    assert_eq!(by_name(&s.execute("stairs.list", &json!({})).unwrap(), "S1")["warnings"], json!([]));
    // An unknown code is an error.
    let v = s.execute("bspec.validate", &json!({"spec": {"stairs": [{"name": "S1", "code": "ubc"}]}})).unwrap();
    let paths: Vec<&str> = v["errors"].as_array().unwrap().iter().map(|e| e["path"].as_str().unwrap()).collect();
    assert!(paths.contains(&"stairs[0].code"), "{v}");
}

#[test]
fn slope_and_percent_notes_are_not_ramps() {
    // The fixture's SLOPE TO DRAIN and 50% OPEN rooms (on A-WALL) give no ramp.
    let mut s = plan();
    let r = s.execute("stairs.detect", &json!({})).unwrap();
    let names: Vec<&str> = r["stairs"].as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["S1", "S2", "R1"], "{r}");
    let r1 = by_name(&r, "R1");
    assert_eq!(r1["runs"], json!([{"from": [100.0, 418.0], "to": [388.0, 418.0], "treads": 0}]));
    assert_eq!((r1["width_in"].clone(), r1["direction"].clone()), (json!(36.0), json!("text")));
    // Nor a SLOPE TO DRAIN outline on the stair layer (the note decides), nor a RAMP note in an
    // outline on the wall layer (with layer roles, only stair layers hold ramps).
    layer(&mut s, "A-FLOR-STRS");
    s.execute("pline", &json!({"vertices": [[1400, 0], [1496, 0], [1496, 120], [1400, 120]], "closed": true})).unwrap();
    text(&mut s, [1410.0, 60.0], "SLOPE 1:12 TO DRAIN");
    layer(&mut s, "A-WALL");
    s.execute("pline", &json!({"vertices": [[1000, 300], [1288, 300], [1288, 336], [1000, 336]], "closed": true})).unwrap();
    text(&mut s, [1100.0, 315.0], "RAMP 1:12");
    let r = s.execute("stairs.detect", &json!({})).unwrap();
    assert_eq!(r["stairs"].as_array().unwrap().len(), 3, "{r}");
    assert_eq!(by_name(&r, "R1")["runs"], r1["runs"]);
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

fn z_max(m: &Mesh) -> f64 {
    m.vertices.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max)
}

#[test]
fn model_build_makes_them_and_walk_climbs_them() {
    let mut s = plan();
    s.execute("stairs.detect", &json!({"store": true})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!((r["stairs"].clone(), r["ramps"].clone(), r["railings"].clone()), (json!(2), json!(1), json!(3)), "{r}");
    // S1's top step is one riser under L2's floor; the handrails 36" over the nosings.
    let stairs = made(&s, "stair");
    let top = stairs.iter().map(z_max).fold(f64::INFINITY, f64::min);
    assert!((top - 144.0 * 18.0 / 19.0).abs() < 1e-6, "{top}");
    let rail = made(&s, "railing").iter().map(z_max).fold(f64::INFINITY, f64::min);
    assert!((rail - (144.0 + 36.0 + 0.9)).abs() < 1e-6, "{rail}");
    assert!((z_max(&made(&s, "ramp")[0]) - 144.0).abs() < 1e-6);
    // Guards: S2's landing (10 of its 20 risers up: 72") gets one guard mesh on 3D-Railings,
    // its top exactly 42" above the landing (IBC 1015.3), from S2's plan objects.
    assert_eq!(r["guards"], json!(1), "{r}");
    let guards: Vec<_> = s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("guard")).cloned().collect();
    assert_eq!(guards.len(), 1);
    assert_eq!(guards[0].common.layer, "3D-Railings");
    assert!(guards[0].common.xdata.iter().any(|t| t.value == "stair:S2"), "{:?}", guards[0].common.xdata);
    let g = &made(&s, "guard")[0];
    assert!((z_max(g) - (72.0 + 42.0)).abs() < 1e-6, "{}", z_max(g));
    let z_min = g.vertices.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
    assert!((z_min - 72.0).abs() < 1e-6, "{z_min}");
    // On the landing's outer edge, 44" past the first run's head (x 199 → 243).
    assert!(g.vertices.iter().any(|v| (v.x - 243.0).abs() < 1.0) && g.vertices.iter().all(|v| v.x > 197.0 && v.x < 245.0), "{g:?}");
    // Walk: from S1's foot, walking east up the flight reaches L2.
    let spec: cadaddy_engine::walk3d::BuildingSpec =
        cadaddy_engine::walk3d::BuildingSpec::from_json(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    let scene = cadaddy_engine::walk3d::derive(s.doc().unwrap(), &spec.to_rules());
    assert!(scene.slopes.len() >= 4, "{}", scene.slopes.len());
    // The guards are drawn in Walk: their top rails, 42" over the landing (6' + 3'-6").
    let top = scene
        .edges
        .chunks(2)
        .filter(|e| e.iter().all(|v| (f64::from(v.p[2]) - 9.5).abs() < 1e-4 && f64::from(v.p[0]) > 16.0 && f64::from(v.p[1]) > 16.0))
        .count();
    assert_eq!(top, 3, "outer edge and both ends of S2's landing");
    let mut w = cadaddy_engine::walk3d::Walker::spawn(&scene, 0);
    w.eye = cadaddy_engine::walk3d::V3 { x: 7.0, y: 22.0 / 12.0, z: w.eye_height };
    w.yaw = 0.0;
    let mut heights = Vec::new();
    for _ in 0..80 {
        w.step(&scene, 0.25, 0.0, 0.0);
        heights.push(w.eye.z - w.eye_height);
    }
    assert!((w.eye.z - w.eye_height - 12.0).abs() < 1e-9, "{:?}", w.eye);
    assert_eq!(w.level, 1);
    assert!(heights.windows(2).all(|p| p[1] >= p[0] - 1e-9), "always climbing");
    // A ramp too: up the 1:2 slope (steep, but it's a slope).
    let mut w = cadaddy_engine::walk3d::Walker::spawn(&scene, 0);
    w.eye = cadaddy_engine::walk3d::V3 { x: 100.0 / 12.0 - 0.5, y: 418.0 / 12.0, z: w.eye_height };
    w.yaw = 0.0;
    for _ in 0..60 {
        w.step(&scene, 0.5, 0.0, 0.0);
    }
    assert!((w.eye.z - w.eye_height - 12.0).abs() < 1e-9, "{:?}", w.eye);
}
