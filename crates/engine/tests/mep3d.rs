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
        .filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some((e.common.layer.to_string(), m.clone())) } else { None })
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
    let (z0, z1) =
        duct_meshes.iter().flat_map(|m| m.vertices.iter()).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v.z), hi.max(v.z)));
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
        .flat_map(|e| e.common.xdata.iter().filter(|t| t.code == 1000 && t.value.starts_with(prefix)).map(|t| t.value.clone()).collect::<Vec<_>>())
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
    assert!(layers.contains(&"3D-MEP-M-DUCT-SUPP-L1"), "duct layer has level: {layers:?}");
    assert!(layers.contains(&"3D-MEP-P-DOMW-CW-L1"), "pipe layer has level: {layers:?}");
    assert!(layers.contains(&"3D-MEP-E-COND-L1"), "conduit layer has level: {layers:?}");
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
    s.execute(
        "bspec.set",
        &json!({"spec": {"mep": {"systems": [
            {"name": "M-DUCT-SUPP", "color_index": 1}
        ]}}}),
    )
    .unwrap();
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
    let term_meshes: Vec<&Mesh> = m
        .iter()
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

// ---------------- fittings, shapes and revisions (from the network) ----------------

/// One level, the given MEP systems, runs and terminals, and (when `walls`) a room's walls on
/// A-WALL to rebuild from. Sheet units are inches.
fn mep_spec(systems: Value, runs: Value, terminals: Value, walls: bool) -> Session {
    let mut s = Session::new();
    if walls {
        layer(&mut s, "A-WALL");
        rect(&mut s, [-120.0, -360.0], [1200.0, 600.0]);
        rect(&mut s, [-114.0, -354.0], [1194.0, 594.0]);
    }
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-1000, -1000, 3000, 3000], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}],
            "mep": {"systems": systems, "runs": runs, "terminals": terminals}
        }}),
    )
    .unwrap();
    s
}

fn duct_system(name: &str, size: [f64; 2]) -> Value {
    json!({"name": name, "layer": name, "kind": "duct", "size_in": size, "bottom_ft": 10})
}

/// Values of a generated MEP object's xdata.
fn tags_of(e: &cadaddy_engine::doc::Entity) -> Vec<String> {
    e.common.xdata.iter().filter(|t| t.code == 1000).map(|t| t.value.clone()).collect()
}

/// The MEP meshes (handle, mesh) carrying all the tags given, found by tag, not by position.
fn tagged(s: &Session, want: &[&str]) -> Vec<(cadaddy_engine::doc::Handle, Mesh)> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).as_deref() == Some("mep"))
        .filter(|e| {
            let t = tags_of(e);
            want.iter().all(|w| t.iter().any(|x| x == w))
        })
        .filter_map(|e| if let EntityKind::Mesh(m) = &e.kind { Some((e.handle, m.clone())) } else { None })
        .collect()
}

/// A mesh's bounds: ([x0, y0, z0], [x1, y1, z1]).
fn bbox(m: &Mesh) -> ([f64; 3], [f64; 3]) {
    m.vertices.iter().fold(([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]), |(lo, hi), v| {
        ([lo[0].min(v.x), lo[1].min(v.y), lo[2].min(v.z)], [hi[0].max(v.x), hi[1].max(v.y), hi[2].max(v.z)])
    })
}

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn round_drop_round_diffuser_and_rect_elbow_have_their_real_dimensions() {
    let mut s = mep_spec(
        json!([duct_system("M-DUCT-RND", [10.0, 0.0]), duct_system("M-DUCT-RECT", [24.0, 12.0])]),
        json!([
            // A 10" round duct at 10' dropping to 8' at its corner.
            {"system": "M-DUCT-RND", "level": "L1", "points": [[0, 0], [120, 0], [120, 120]], "size_in": [10, 0], "bottoms_ft": [10, 8]},
            // A 24x12 duct at 10' turning from east to north.
            {"system": "M-DUCT-RECT", "level": "L1", "points": [[0, 200], [240, 200], [240, 440]], "size_in": [24, 12], "bottoms_ft": [10, 10]}
        ]),
        json!([{"system": "M-DUCT-RND", "level": "L1", "at": [300, 0], "size_in": [12, 0], "bottom_ft": 9, "terminal_kind": "diffuser"}]),
        false,
    );
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!((r["drops"].clone(), r["elbows"].clone(), r["terminals"].clone()), (json!(1), json!(1), json!(1)), "{r}");

    // The round drop: an upright 10" tube from 8' (96") to the top of the duct at 10' (130").
    let drop = tagged(&s, &["part:fitting:drop", "system:M-DUCT-RND"]);
    assert_eq!(drop.len(), 1, "one drop object");
    let (lo, hi) = bbox(&drop[0].1);
    assert!(near(lo[2], 96.0, 1e-6) && near(hi[2], 130.0, 1e-6), "z {} → {}", lo[2], hi[2]);
    assert!(near(hi[2] - lo[2], 24.0 + 10.0, 1e-6), "z span = the drop + the diameter");
    assert!(near(hi[0] - lo[0], 10.0, 1e-6) && near(hi[1] - lo[1], 10.0, 1e-6), "xy span = the diameter: {lo:?} {hi:?}");
    assert!(near((lo[0] + hi[0]) / 2.0, 120.0, 1e-6) && near((lo[1] + hi[1]) / 2.0, 0.0, 1e-6), "at the corner");

    // The round diffuser: an upright squat cylinder 12" across and 6" tall at 9'.
    let diffuser = tagged(&s, &["part:terminal:diffuser"]);
    assert_eq!(diffuser.len(), 1);
    let (lo, hi) = bbox(&diffuser[0].1);
    assert!(near(hi[0] - lo[0], 12.0, 1e-6) && near(hi[1] - lo[1], 12.0, 1e-6), "xy span = the diameter: {lo:?} {hi:?}");
    assert!(near(lo[2], 108.0, 1e-6) && near(hi[2] - lo[2], 6.0, 1e-6), "6\" tall from 9': {lo:?} {hi:?}");

    // The 24x12 elbow: as wide as the duct (24") at the turn, its centreline an arc from the east
    // run (y = 200) to the north one (x = 240): the outer edges are those of the two runs.
    let elbow = tagged(&s, &["part:fitting:elbow", "system:M-DUCT-RECT"]);
    assert_eq!(elbow.len(), 1);
    let (lo, hi) = bbox(&elbow[0].1);
    assert!(near(lo[1], 200.0 - 12.0, 0.25), "the outside of the east run's 24\" width: {lo:?}");
    assert!(near(hi[0], 240.0 + 12.0, 0.25), "the outside of the north run's 24\" width: {hi:?}");
    assert!((24.0..27.0).contains(&(hi[0] - lo[0])) && (24.0..27.0).contains(&(hi[1] - lo[1])), "24\" across the turn: {lo:?} {hi:?}");
    assert!(near(lo[2], 120.0, 1e-6) && near(hi[2] - lo[2], 12.0, 1e-6), "z span = the 12\" height: {lo:?} {hi:?}");
    // The centreline's middle (45° round the arc, radius 12 about (228, 212)) is inside it.
    let mid = (228.0 + 12.0 * std::f64::consts::FRAC_1_SQRT_2, 212.0 - 12.0 * std::f64::consts::FRAC_1_SQRT_2);
    assert!(lo[0] < mid.0 && mid.0 < hi[0] && lo[1] < mid.1 && mid.1 < hi[1]);
}

#[test]
fn each_fitting_is_its_own_object_with_its_kind() {
    // A 24x12 run: an elbow, a drop from 10' to 8', another elbow.
    let mut s = mep_spec(
        json!([duct_system("M-DUCT-SUPP", [24.0, 12.0])]),
        json!([{"system": "M-DUCT-SUPP", "level": "L1", "points": [[0, 0], [240, 0], [240, 240], [480, 240], [480, 480]], "size_in": [24, 12], "bottoms_ft": [10, 10, 8, 8]}]),
        json!([]),
        false,
    );
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!((r["elbows"].clone(), r["drops"].clone(), r["fittings"].clone()), (json!(2), json!(1), json!(3)), "{r}");
    let elbows = tagged(&s, &["part:fitting:elbow"]);
    let drops = tagged(&s, &["part:fitting:drop"]);
    assert_eq!((elbows.len(), drops.len()), (2, 1), "one object per fitting, no merged 'dominant' kind");
    // Each where its corner is.
    let centre = |m: &Mesh| {
        let (lo, hi) = bbox(m);
        ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0)
    };
    let (dx, dy) = centre(&drops[0].1);
    assert!(near(dx, 240.0, 1e-6) && near(dy, 240.0, 1e-6), "the drop at (240, 240): {dx}, {dy}");
    // The rectangular drop runs along the way the duct comes in (north): 24" across (x), 12" along (y).
    let (lo, hi) = bbox(&drops[0].1);
    assert!(near(hi[0] - lo[0], 24.0, 1e-6) && near(hi[1] - lo[1], 12.0, 1e-6), "{lo:?} {hi:?}");
    let mut at: Vec<(i64, i64)> =
        elbows.iter().map(|(_, m)| centre(m)).map(|(x, y)| ((x / 120.0).round() as i64, (y / 120.0).round() as i64)).collect();
    at.sort();
    assert_eq!(at, vec![(2, 0), (4, 2)], "the elbows at (240, 0) and (480, 240)");
}

#[test]
fn tees_and_transitions_are_built_where_runs_meet() {
    let runs = json!([
        // A 24x12 main, a 12x8 branch off its side, and an 18x12 run on from its end.
        {"system": "M-DUCT-SUPP", "level": "L1", "points": [[0, 0], [480, 0]], "size_in": [24, 12], "bottoms_ft": [10]},
        {"system": "M-DUCT-SUPP", "level": "L1", "points": [[240, 0], [240, -240]], "size_in": [12, 8], "bottoms_ft": [10]},
        {"system": "M-DUCT-SUPP", "level": "L1", "points": [[480, 0], [720, 0]], "size_in": [18, 12], "bottoms_ft": [10]},
        // A branch under the main, at 6': it passes below, no tee.
        {"system": "M-DUCT-SUPP", "level": "L1", "points": [[360, 0], [360, 240]], "size_in": [12, 8], "bottoms_ft": [6]},
        // 12" round to 8" round end to end: a reducer.
        {"system": "M-DUCT-SUPP", "level": "L1", "points": [[0, 300], [240, 300]], "size_in": [12, 0], "bottoms_ft": [10]},
        {"system": "M-DUCT-SUPP", "level": "L1", "points": [[240, 300], [480, 300]], "size_in": [8, 0], "bottoms_ft": [10]},
        // Three 10" round runs ending at one point: a tee.
        {"system": "M-DUCT-SUPP", "level": "L1", "points": [[600, 300], [720, 300]], "size_in": [10, 0], "bottoms_ft": [10]},
        {"system": "M-DUCT-SUPP", "level": "L1", "points": [[720, 300], [840, 300]], "size_in": [10, 0], "bottoms_ft": [10]},
        {"system": "M-DUCT-SUPP", "level": "L1", "points": [[720, 300], [720, 420]], "size_in": [10, 0], "bottoms_ft": [10]},
        // Another system's duct ending on the main: not a tee of this one.
        {"system": "M-DUCT-RETN", "level": "L1", "points": [[120, 0], [120, 240]], "size_in": [12, 8], "bottoms_ft": [10]}
    ]);
    let mut s = mep_spec(json!([duct_system("M-DUCT-SUPP", [24.0, 12.0]), duct_system("M-DUCT-RETN", [12.0, 8.0])]), runs, json!([]), false);
    let r = s.execute("model.build", &json!({})).unwrap();
    assert_eq!(
        (r["tees"].clone(), r["transitions"].clone(), r["reducers"].clone(), r["crosses"].clone(), r["elbows"].clone()),
        (json!(2), json!(1), json!(1), json!(0), json!(0)),
        "{r}"
    );
    let tees = tagged(&s, &["part:fitting:tee"]);
    assert_eq!(tees.len(), 2);
    let transition = tagged(&s, &["part:fitting:transition"]);
    let reducer = tagged(&s, &["part:fitting:reducer"]);
    assert_eq!((transition.len(), reducer.len()), (1, 1));
    // The side tee sits on the main at (240, 0), as wide as the main along it, with the main's
    // and the branch's sizes.
    let side = tees.iter().find(|(_, m)| near((bbox(m).0[0] + bbox(m).1[0]) / 2.0, 240.0, 1e-6)).expect("the side tee");
    let (lo, hi) = bbox(&side.1);
    assert!(near(lo[2], 120.0, 1e-6) && near(hi[2], 132.0, 1e-6), "the main's height: {lo:?} {hi:?}");
    assert!(near(lo[1], -(12.0 + 2.0), 1e-6) && near(hi[1], 12.0, 1e-6), "the branch stub reaches past the main's side: {lo:?} {hi:?}");
    let e = s.doc().unwrap().entity(side.0).unwrap().clone();
    let t = tags_of(&e);
    assert!(t.contains(&"size:24x12".to_string()) && t.contains(&"size2:12x8".to_string()), "{t:?}");
    // The transition: centred at (480, 0), from 24" wide to 18".
    let (lo, hi) = bbox(&transition[0].1);
    assert!(near(lo[0], 480.0 - 6.0, 1e-6) && near(hi[0], 480.0 + 6.0, 1e-6) && near(hi[1] - lo[1], 24.0, 1e-6), "{lo:?} {hi:?}");
    let e = s.doc().unwrap().entity(transition[0].0).unwrap().clone();
    assert!(tags_of(&e).contains(&"size2:18x12".to_string()), "{:?}", tags_of(&e));
}

#[test]
fn a_hand_edited_segment_keeps_its_runs_fittings_on_revise() {
    let mut s = mep_spec(
        json!([duct_system("M-DUCT-SUPP", [24.0, 12.0])]),
        json!([{"system": "M-DUCT-SUPP", "level": "L1", "points": [[0, 0], [240, 0], [240, 240], [480, 240]], "size_in": [24, 12], "bottoms_ft": [10, 10, 10], "objects": ["FFFF0"]}]),
        json!([]),
        true,
    );
    // Not associative: model.revise does the rebuild.
    s.execute("model.build", &json!({"auto": false})).unwrap();
    assert_eq!(tagged(&s, &["part:fitting:elbow"]).len(), 2);
    // Recolour the duct segment by hand.
    let seg = tagged(&s, &["part:segment"]);
    assert_eq!(seg.len(), 1);
    s.doc_mut().unwrap().modify_entity(seg[0].0, |e| e.common.color = cadaddy_engine::doc::color::Color::Index(1)).unwrap();
    // The plan changes (a wall moves), so the model is built again.
    let east: Vec<String> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).is_none() && e.common.layer.to_string() == "A-WALL")
        .map(|e| e.handle.hex())
        .take(1)
        .collect();
    s.execute("move", &json!({"handles": east, "from": [0, 0], "to": [24, 0]})).unwrap();
    let r = s.execute("model.revise", &json!({})).unwrap();
    assert_eq!(r["rebuilt"], true, "{r}");
    assert_eq!(r["kept"].as_array().map(Vec::len), Some(1), "the edited segment is kept: {r}");
    let seg = tagged(&s, &["part:segment"]);
    assert_eq!(seg.len(), 1, "one segment: the rebuilt one gave way");
    assert_eq!(s.doc().unwrap().entity(seg[0].0).unwrap().common.color, cadaddy_engine::doc::color::Color::Index(1));
    assert_eq!(tagged(&s, &["part:fitting:elbow"]).len(), 2, "the run's fittings are still there");
}

#[test]
/// MUST 1: A segment's height slot is filled only by a text that parses as a height; when
/// several height texts reach one segment, the nearest wins. Size-only texts and room names
/// never block a height.
fn separate_height_tag_wins_over_size_and_room_name() {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    rect(&mut s, [-12.0, -12.0], [492.0, 372.0]);
    rect(&mut s, [-6.0, -6.0], [486.0, 366.0]);
    layer(&mut s, "M-DUCT-SUPP");
    // A straight duct along y=60.
    s.execute("pline", &json!({"vertices": [[24, 60], [264, 60]]})).unwrap();
    // Size text drawn first (closer to the segment than the height text).
    text(&mut s, [144.0, 70.0], "24x12");
    // A room-name text nearer the segment than both tags.
    text(&mut s, [144.0, 64.0], "GUEST ROOM 101");
    // Separate elevation tag, farther than the size and room texts.
    text(&mut s, [144.0, 90.0], "BOT @ 10'-6\"");
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9, "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    let r = s.execute("mep.read", &json!({})).unwrap();
    let duct = run(&r, "M-DUCT-SUPP");
    assert_eq!(duct["size_in"], json!([24.0, 12.0]), "size from the 24x12 tag: {duct}");
    assert_eq!(duct["bottoms_ft"], json!([10.5]), "bottom from separate BOT @ 10'-6\": {duct}");
}

#[test]
/// MUST 2: model.clashes reports the real count when it truncates and sets truncated flag.
fn clash_total_is_honest_when_truncated() {
    // The existing small fixture: truncated should be false, total == clashes.len().
    let mut s = drawing();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let c = s.execute("model.clashes", &json!({})).unwrap();
    let clashes = c["clashes"].as_array().unwrap();
    assert_eq!(c["total"].as_u64().unwrap() as usize, clashes.len(), "total == len when not truncated: {c}");
    assert_eq!(c["truncated"], json!(false), "not truncated: {c}");

    // Lower the cap to 1 so the existing clashes exceed it.
    let c = s.execute("model.clashes", &json!({"max_clashes": 1})).unwrap();
    let clashes = c["clashes"].as_array().unwrap();
    assert!(c["total"].as_u64().unwrap() as usize > clashes.len(), "total > len: {c}");
    assert_eq!(c["truncated"], json!(true), "truncated flag: {c}");
    assert_eq!(clashes.len(), 1, "list capped at 1: {c}");
}

#[test]
fn deleting_one_of_two_sourceless_terminals_keeps_the_other_on_revise() {
    let mut s = mep_spec(
        json!([duct_system("M-DUCT-SUPP", [24.0, 12.0])]),
        json!([{"system": "M-DUCT-SUPP", "level": "L1", "points": [[0, 0], [480, 0]], "size_in": [24, 12], "bottoms_ft": [10]}]),
        json!([
            {"system": "M-DUCT-SUPP", "level": "L1", "at": [120, 0], "size_in": [24, 24], "bottom_ft": 9, "terminal_kind": "diffuser"},
            {"system": "M-DUCT-SUPP", "level": "L1", "at": [360, 0], "size_in": [24, 24], "bottom_ft": 9, "terminal_kind": "diffuser"}
        ]),
        true,
    );
    // Not associative: model.revise does the rebuild.
    s.execute("model.build", &json!({"auto": false})).unwrap();
    let terms = tagged(&s, &["part:terminal:diffuser"]);
    assert_eq!(terms.len(), 2);
    let west = terms.iter().find(|(_, m)| bbox(m).1[0] < 240.0).unwrap().0;
    s.execute("erase", &json!({"handles": [west.hex()]})).unwrap();
    assert_eq!(tagged(&s, &["part:terminal:diffuser"]).len(), 1);
    let wall: Vec<String> = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).is_none() && e.common.layer.to_string() == "A-WALL")
        .map(|e| e.handle.hex())
        .take(1)
        .collect();
    s.execute("move", &json!({"handles": wall, "from": [0, 0], "to": [24, 0]})).unwrap();
    let r = s.execute("model.revise", &json!({})).unwrap();
    assert_eq!(r["rebuilt"], true, "{r}");
    let left = tagged(&s, &["part:terminal:diffuser"]);
    assert_eq!(left.len(), 1, "the deleted terminal stays deleted, the other stays: {r}");
    assert!(bbox(&left[0].1).0[0] > 240.0, "the east one is the one left");
    // The duct (also read from no plan object) is still there too.
    assert_eq!(tagged(&s, &["part:segment"]).len(), 1);
}

/// An elevator hoistway of the spec's circulation is closed to MEP: a run through it is a
/// `hoistway` clash; one beside it is not.
#[test]
fn a_run_through_an_elevator_hoistway_is_a_clash() {
    let mut s = Session::new();
    let spec = json!({
        "levels": [{"name": "Level 1", "elevation_ft": 0.0, "height_ft": 12.0, "ceiling_ft": 9.0}],
        "mep": {
            "systems": [{"name": "SA", "layer": "M-SUPP", "kind": "duct", "size_in": [12, 8], "bottom_ft": 9.5}],
            "runs": [
                {"system": "SA", "level": "Level 1", "points": [[0, 100], [300, 100]], "size_in": [12, 8], "bottoms_ft": [9.5]},
                {"system": "SA", "level": "Level 1", "points": [[0, 200], [300, 200]], "size_in": [12, 8], "bottoms_ft": [9.5]}
            ]
        },
        "circulation": [{"kind": "hoistway", "level": "Level 1", "name": "ELEV.", "at": [120, 100], "rects": [[[60, 60], [156, 156]]]}]
    });
    s.execute("bspec.set", &json!({"spec": spec})).unwrap();
    let r = s.execute("model.clashes", &json!({})).unwrap();
    let shaft: Vec<&Value> = r["clashes"].as_array().unwrap().iter().filter(|c| c["kind"] == json!("hoistway")).collect();
    assert_eq!(shaft.len(), 1, "{r}");
    assert_eq!(shaft[0]["at"], json!([108.0, 108.0]), "{r}");
}

/// A branch circuit's drop down a wall to its outlet (a conduit riser from the ceiling to the
/// box's height) is below the ceiling by design: no clash. A duct hanging low is one, and so is
/// a conduit run across the room at that height.
#[test]
fn a_drop_to_an_outlet_is_no_clash_and_a_low_duct_is() {
    let mut s = Session::new();
    let spec = json!({
        "levels": [{"name": "Level 1", "elevation_ft": 0.0, "height_ft": 12.0, "ceiling_ft": 9.0}],
        "mep": {
            "systems": [
                {"name": "SA", "layer": "M-SUPP", "kind": "duct", "size_in": [12, 8], "bottom_ft": 9.5},
                {"name": "E-CIRCUIT-EST", "layer": "E-COND", "kind": "conduit", "size_in": [0.75, 0.75], "bottom_ft": 9.5}
            ],
            "runs": [
                {"system": "E-CIRCUIT-EST", "level": "Level 1", "points": [[50, 50]], "size_in": [0.75, 0.75], "bottoms_ft": [1.5], "top_ft": 9.5},
                {"system": "SA", "level": "Level 1", "points": [[0, 200], [300, 200]], "size_in": [12, 8], "bottoms_ft": [7.0]},
                {"system": "E-CIRCUIT-EST", "level": "Level 1", "points": [[0, 400], [300, 400]], "size_in": [0.75, 0.75], "bottoms_ft": [7.0]}
            ]
        }
    });
    s.execute("bspec.set", &json!({"spec": spec})).unwrap();
    let r = s.execute("model.clashes", &json!({})).unwrap();
    let low: Vec<(&str, f64)> = r["clashes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == json!("below_ceiling"))
        .map(|c| (c["systems"][0].as_str().unwrap_or(""), c["at"][1].as_f64().unwrap_or(f64::NAN)))
        .collect();
    assert_eq!(low.len(), 2, "the duct and the low conduit run, not the drop: {r}");
    assert!(low.iter().any(|(sys, y)| *sys == "SA" && (*y - 200.0).abs() < 1e-6), "{r}");
    assert!(low.iter().all(|(_, y)| (*y - 50.0).abs() > 1e-6), "the drop is no clash: {r}");
    // Each clash says where it is.
    assert!(r["clashes"].as_array().unwrap().iter().all(|c| c["at"].as_array().is_some_and(|a| a.len() == 2)), "{r}");
}
