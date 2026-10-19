//! Single-line MEP runs to 3D: tagged duct, pipe and conduit runs read with their sizes and
//! heights (a duct dropping at a corner), built per system (one system hidden), and the
//! clashes: a duct under the ceiling, a pipe through a duct, a conduit in the slab above.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{EntityKind, Mesh};
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

fn drawing() -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [-12.0, -12.0], [492.0, 372.0]);
    rect(&mut s, [-6.0, -6.0], [486.0, 366.0]);
    // A supply duct: 24x12 at 10' along y = 60, dropping to 8' round the corner.
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("pline", &json!({"vertices": [[24, 60], [264, 60], [264, 180]]})).unwrap();
    text(&mut s, [180.0, 70.0], "24x12 BOT @ 10'-0\"");
    text(&mut s, [280.0, 140.0], "BOT @ 8'-0\"");
    // Cold water across it at 10'-6" to its centreline.
    layer(&mut s, "P-DOMW-CW");
    s.execute("line", &json!({"points": [[100, 0], [100, 120]]})).unwrap();
    text(&mut s, [104.0, 20.0], "2\" CW CL @ 10'-6\"");
    // A conduit high up, into the floor above.
    layer(&mut s, "E-COND");
    s.execute("line", &json!({"points": [[300, 300], [400, 300]]})).unwrap();
    text(&mut s, [340.0, 306.0], "1\" C BOT @ 13'-10\"");
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]},
                {"name": "L2", "elevation_ft": 14, "height_ft": 12, "ceiling_ft": 9, "region": [2000, -100, 3000, 1000]}
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s
}

fn run<'a>(r: &'a Value, system: &str) -> &'a Value {
    r["runs"].as_array().unwrap().iter().find(|x| x["system"] == system).unwrap_or_else(|| panic!("{system}: {r}"))
}

#[test]
fn tagged_runs_read_with_sizes_and_heights() {
    let mut s = drawing();
    let r = s.execute("mep.read", &json!({})).unwrap();
    let kinds: Vec<(&str, &str)> =
        r["systems"].as_array().unwrap().iter().map(|x| (x["name"].as_str().unwrap(), x["kind"].as_str().unwrap())).collect();
    assert_eq!(kinds, vec![("E-COND", "conduit"), ("M-DUCT-SUPP", "duct"), ("P-DOMW-CW", "pipe")]);
    let duct = run(&r, "M-DUCT-SUPP");
    assert_eq!(duct["size_in"], json!([24.0, 12.0]));
    assert_eq!(duct["bottoms_ft"], json!([10.0, 8.0]), "it drops at the corner: {duct}");
    assert_eq!(duct["points"].as_array().unwrap().len(), 3);
    let pipe = run(&r, "P-DOMW-CW");
    assert_eq!(pipe["size_in"], json!([2.0, 0.0]));
    assert!((pipe["bottoms_ft"][0].as_f64().unwrap() - (10.5 - 1.0 / 12.0)).abs() < 1e-3, "{pipe}");
    let conduit = run(&r, "E-COND");
    assert_eq!(conduit["size_in"], json!([1.0, 0.0]));
    assert!((conduit["bottoms_ft"][0].as_f64().unwrap() - (13.0 + 10.0 / 12.0)).abs() < 1e-3);
    // Applied, the systems and runs are in the spec, in plain language.
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    let parts = s.execute("bspec.get", &json!({})).unwrap()["parts"].clone();
    let duct_part = parts.as_array().unwrap().iter().find(|p| p["key"] == "systems/M-DUCT-SUPP").unwrap()["text"].clone();
    assert_eq!(duct_part, json!("M-DUCT-SUPP: duct on M-DUCT-SUPP, 1 runs, usually 12x12\" with its bottom at 10'-0\""));
    assert!(s.execute("mep.read", &json!({"level": "L9"})).is_err());
}

fn meshes(s: &Session) -> Vec<(String, Mesh)> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("mep"))
        .filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some((e.common.layer.clone(), m.clone())) } else { None })
        .collect()
}

#[test]
fn runs_build_per_system_and_clash() {
    let mut s = drawing();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!((r["mepRuns"].clone(), r["drops"].clone()), (json!(3), json!(1)), "{r}");
    let m = meshes(&s);
    let duct = &m.iter().find(|(l, _)| l == "3D-MEP-M-DUCT-SUPP").unwrap().1;
    let (z0, z1) = duct.vertices.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.z), hi.max(v.z)));
    assert_eq!((z0, z1), (96.0, 132.0), "8' to 11'");
    let pipe = &m.iter().find(|(l, _)| l == "3D-MEP-P-DOMW-CW").unwrap().1;
    let r_pipe = pipe.vertices.iter().map(|v| v.x).fold(f64::NEG_INFINITY, f64::max) - 100.0;
    assert!((r_pipe - 1.0).abs() < 1e-9, "a 2\" round pipe: {r_pipe}");
    // The clashes.
    let c = s.execute("model.clashes", &json!({})).unwrap();
    let kinds: Vec<&str> = c["clashes"].as_array().unwrap().iter().map(|x| x["kind"].as_str().unwrap()).collect();
    for k in ["below_ceiling", "crossing", "into_structure"] {
        assert!(kinds.contains(&k), "{k} in {c}");
    }
    let below = c["clashes"].as_array().unwrap().iter().find(|x| x["kind"] == "below_ceiling").unwrap();
    assert_eq!((below["systems"].clone(), below["by_in"].clone()), (json!(["M-DUCT-SUPP"]), json!(12.0)));
    let cross = c["clashes"].as_array().unwrap().iter().find(|x| x["kind"] == "crossing").unwrap();
    assert_eq!(cross["at"], json!([100.0, 60.0]));
    let into = c["clashes"].as_array().unwrap().iter().find(|x| x["kind"] == "into_structure").unwrap();
    assert_eq!((into["systems"].clone(), into["by_in"].clone()), (json!(["E-COND"]), json!(5.0)));
    // A tolerance forgives small ones: the 5" conduit clash stays, nothing under 6" does with 6".
    let c6 = s.execute("model.clashes", &json!({"tolerance_in": 6})).unwrap();
    assert!(!c6["clashes"].as_array().unwrap().iter().any(|x| x["kind"] == "into_structure"));
    // Hide the pipes: they aren't built, their layer stays to toggle.
    s.execute("bspec.set", &json!({"spec": {"mep": {"systems": [{"name": "P-DOMW-CW", "show": false}]}}})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(r["mepRuns"], json!(2), "{r}");
    assert!(meshes(&s).iter().all(|(l, _)| l != "3D-MEP-P-DOMW-CW"));
    let sp = s.execute("bspec.get", &json!({})).unwrap();
    assert_eq!(sp["spec"]["mep"]["systems"].as_array().unwrap().len(), 3, "the merge kept the others");
}
