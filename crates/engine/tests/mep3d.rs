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
    // Layers are now per system per level: 3D-MEP-<system>-<level>.
    // Combine all meshes for the duct system (segments + fittings) for height check.
    let duct_meshes: Vec<&Mesh> = m.iter().filter(|(l, _)| l.starts_with("3D-MEP-M-DUCT-SUPP")).map(|(_, m)| m).collect();
    assert!(!duct_meshes.is_empty(), "duct meshes exist");
    let (z0, z1) = duct_meshes.iter().flat_map(|m| m.vertices.iter()).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.z), hi.max(v.z)));
    assert_eq!((z0, z1), (96.0, 132.0), "8' to 11'");
    let pipe = &m.iter().find(|(l, _)| l.starts_with("3D-MEP-P-DOMW-CW")).unwrap().1;
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
    assert!(meshes(&s).iter().all(|(l, _)| !l.starts_with("3D-MEP-P-DOMW-CW")));
    let sp = s.execute("bspec.get", &json!({})).unwrap();
    assert_eq!(sp["spec"]["mep"]["systems"].as_array().unwrap().len(), 3, "the merge kept the others");
}

/// Xdata tags on MEP meshes: segments are tagged `part:segment`, fittings `part:fitting:*`.
fn xdata_tag(s: &Session, prefix: &str) -> Vec<String> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("mep"))
        .flat_map(|e| {
            e.common.xdata.iter().filter(|t| t.code == 1000 && t.value.starts_with(prefix)).map(|t| t.value.clone()).collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn fittings_are_separate_meshes_with_xdata_tags() {
    let mut s = drawing();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();

    // The duct run has a corner: 1 segment mesh + 1 fitting mesh.
    let parts = xdata_tag(&s, "part:");
    let segment_count = parts.iter().filter(|p| p.as_str() == "part:segment").count();
    let fitting_count = parts.iter().filter(|p| p.starts_with("part:fitting:")).count();
    assert!(segment_count >= 3, "at least 3 segment meshes (duct+pipe+conduit): {segment_count}");
    assert!(fitting_count >= 1, "at least 1 fitting mesh (the duct elbow): {fitting_count}");

    // Fitting tags name the fitting type. The duct corner also drops in height → drop fitting.
    let fitting_types: Vec<&str> = parts.iter().filter(|p| p.starts_with("part:fitting:")).map(|p| p.as_str()).collect();
    assert!(
        fitting_types.contains(&"part:fitting:elbow") || fitting_types.contains(&"part:fitting:drop"),
        "elbow or drop fitting: {fitting_types:?}"
    );
}

#[test]
fn per_system_per_level_layers() {
    let mut s = drawing();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();

    // Each layer includes the level name.
    let m = meshes(&s);
    let layers: Vec<&str> = m.iter().map(|(l, _)| l.as_str()).collect();
    // The duct is on L1, so its layer is 3D-MEP-M-DUCT-SUPP-L1.
    assert!(layers.iter().any(|l| *l == "3D-MEP-M-DUCT-SUPP-L1"), "duct layer has level: {layers:?}");
    assert!(layers.iter().any(|l| *l == "3D-MEP-P-DOMW-CW-L1"), "pipe layer has level: {layers:?}");
    assert!(layers.iter().any(|l| *l == "3D-MEP-E-COND-L1"), "conduit layer has level: {layers:?}");
}

#[test]
fn duct_type_inference_and_configurable_colors() {
    let mut s = drawing();
    let r = s.execute("mep.read", &json!({})).unwrap();

    // M-DUCT-SUPP is inferred as supply duct.
    let duct_sys = r["systems"].as_array().unwrap().iter().find(|x| x["name"] == "M-DUCT-SUPP").unwrap();
    assert_eq!(duct_sys["duct_type"], json!("supply"), "inferred as supply");

    // Apply and check that the system is stored with its duct_type.
    s.execute("mep.read", &json!({"apply": true})).unwrap();

    // Set a custom colour for the duct system.
    s.execute("bspec.set", &json!({"spec": {"mep": {"systems": [
        {"name": "M-DUCT-SUPP", "color_index": 1}
    ]}}})).unwrap();
    s.execute("model.build", &json!({})).unwrap();

    // The duct layer should exist with the custom colour (red = ACI 1).
    let d = s.doc().unwrap();
    let duct_layer = d.layers.iter().find(|l| l.name.starts_with("3D-MEP-M-DUCT-SUPP")).unwrap();
    assert_eq!(duct_layer.color, cadaddy_engine::doc::color::Color::Index(1), "custom colour applied");
}

#[test]
fn terminals_build_as_simple_shapes() {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [-12.0, -12.0], [492.0, 372.0]);
    rect(&mut s, [-6.0, -6.0], [486.0, 366.0]);
    // A supply duct with a diffuser.
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("line", &json!({"points": [[24, 60], [264, 60]]})).unwrap();
    text(&mut s, [180.0, 70.0], "24x12 BOT @ 10'-0\"");

    s.execute("bspec.set", &json!({"replace": true, "spec": {
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}
        ],
        "layers": [{"layer": "A-WALL", "role": "wall"}],
        "mep": {
            "terminals": [
                {"system": "M-DUCT-SUPP", "level": "L1", "at": [144.0, 60.0], "size_in": [24.0, 24.0], "bottom_ft": 9.0, "terminal_kind": "diffuser"}
            ]
        }
    }})).unwrap();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    let r = s.execute("model.build", &json!({})).unwrap();
    assert!(r["terminals"].as_u64().unwrap_or(0) >= 1, "terminal built: {r}");

    // The terminal mesh is on the duct system's layer and is tagged as a terminal.
    let parts = xdata_tag(&s, "part:terminal:");
    assert!(!parts.is_empty(), "terminal xdata present: {parts:?}");
    assert!(parts.iter().any(|p| p == "part:terminal:diffuser"), "diffuser: {parts:?}");

    // The terminal mesh geometry: it should be at x~144, z starts at 9' = 108 inches.
    let m = meshes(&s);
    let term_meshes: Vec<&Mesh> = m.iter()
        .filter(|(l, _)| l.starts_with("3D-MEP-M-DUCT-SUPP"))
        .filter(|(_, mesh)| {
            let center_x = mesh.vertices.iter().map(|v| v.x).sum::<f64>() / mesh.vertices.len().max(1) as f64;
            (center_x - 144.0).abs() < 24.0
        })
        .map(|(_, m)| m)
        .collect();
    assert!(!term_meshes.is_empty(), "terminal mesh near x=144");
    let z0 = term_meshes.iter().flat_map(|m| m.vertices.iter()).map(|v| v.z).fold(f64::INFINITY, f64::min);
    assert!((z0 - 108.0).abs() < 1.0, "terminal bottom at 9' (108\"): {z0}");
}

#[test]
fn geometry_truth_rectangular_duct_dimensions() {
    // Verify that a straight rectangular duct segment has the correct width and height.
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [-12.0, -12.0], [492.0, 372.0]);
    rect(&mut s, [-6.0, -6.0], [486.0, 366.0]);
    layer(&mut s, "M-DUCT-SUPP");
    // A straight 24x12 duct at 10' along y=60.
    s.execute("line", &json!({"points": [[24, 60], [264, 60]]})).unwrap();
    text(&mut s, [144.0, 70.0], "24x12 BOT @ 10'-0\"");
    s.execute("bspec.set", &json!({"replace": true, "spec": {
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall"}]
    }})).unwrap();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();

    let m = meshes(&s);
    let duct = m.iter().filter(|(l, _)| l.starts_with("3D-MEP-M-DUCT-SUPP")).map(|(_, m)| m).next().unwrap();

    // Duct width = 24" = 2' along Y, so span in Y should be 24 inches centred on y=60.
    let y_min = duct.vertices.iter().map(|v| v.y).fold(f64::INFINITY, f64::min);
    let y_max = duct.vertices.iter().map(|v| v.y).fold(f64::NEG_INFINITY, f64::max);
    let y_span = y_max - y_min;
    // The duct is 24" wide but the box uses the width parameter (24/12 * upf = 24 inches in sheet).
    // Actually width = size_in[0] / 12 * upf = 24/12 * 12 = 24 sheet units. So span should be ~24.
    assert!((y_span - 24.0).abs() < 1.0, "duct width ~24 sheet units (24\"): {y_span}");

    // Height = 12" = 1', so z span = 12 sheet units.
    let z_min = duct.vertices.iter().map(|v| v.z).fold(f64::INFINITY, f64::min);
    let z_max = duct.vertices.iter().map(|v| v.z).fold(f64::NEG_INFINITY, f64::max);
    let z_span = z_max - z_min;
    assert!((z_span - 12.0).abs() < 1.0, "duct height ~12 sheet units (12\"): {z_span}");

    // Bottom at 10' = 120 sheet units.
    assert!((z_min - 120.0).abs() < 1.0, "duct bottom at 10' (120): {z_min}");
}
