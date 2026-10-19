//! Stairs and ramps from synthetic plans: a straight flight and a switchback with a landing
//! are detected (treads, direction from the UP note, width), their risers fall within the code
//! range, a ramp is found from its note, the spec takes and confirms them, `model.build` builds
//! steps, landings and handrails, and Walk climbs the stair to the next level.

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
    assert_eq!(by_name(&l, "S1")["warnings"], json!([]));
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
    // Walk: from S1's foot, walking east up the flight reaches L2.
    let spec: cadaddy_engine::walk3d::BuildingSpec =
        cadaddy_engine::walk3d::BuildingSpec::from_json(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    let scene = cadaddy_engine::walk3d::derive(s.doc().unwrap(), &spec.to_rules());
    assert!(scene.slopes.len() >= 4, "{}", scene.slopes.len());
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
