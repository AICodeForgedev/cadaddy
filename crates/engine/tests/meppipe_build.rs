//! Plumbing pipe detection and build: pipe runs with system tags are read, built into 3D
//! and their quantities counted. Tests both standard-layer mode (P-* layers) and the
//! text-tag fallback mode (PDF-imported drawings with generic layer names).

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 4})).unwrap();
}

/// A drawing with piping on standard P-* layers.
fn pipe_drawing() -> Session {
    let mut s = Session::new();
    // Walls.
    layer(&mut s, "A-WALL");
    s.execute("pline", &json!({"vertices": [[-12, -12], [492, -12], [492, 372], [-12, 372]], "closed": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[-6, -6], [486, -6], [486, 366], [-6, 366]], "closed": true})).unwrap();

    // Cold water main along the corridor, 1" CW at 10' height.
    layer(&mut s, "P-DOMW-CW");
    s.execute("pline", &json!({"vertices": [[24, 180], [240, 180], [240, 60], [480, 60]]})).unwrap();
    text(&mut s, [120.0, 190.0], "1\" CW BOP @ 10'-0\"");

    // Hot water parallel to CW, 3/4" HW at 9'-6".
    layer(&mut s, "P-DOMW-HW");
    s.execute("pline", &json!({"vertices": [[24, 174], [240, 174], [240, 54], [480, 54]]})).unwrap();
    text(&mut s, [120.0, 164.0], "3/4\" HW CL @ 9'-6\"");

    // Sanitary waste below floor, 4" SAN with slope.
    layer(&mut s, "P-SANW");
    s.execute("pline", &json!({"vertices": [[60, 300], [240, 300], [240, 360]]})).unwrap();
    text(&mut s, [140.0, 310.0], "4\" SAN @ 1/4\"/FT");

    // Vent, 2" V at 10'.
    layer(&mut s, "P-VENT");
    s.execute("line", &json!({"points": [[300, 300], [300, 360]]})).unwrap();
    text(&mut s, [304.0, 330.0], "2\" V");

    // Fire protection main, 2" FP.
    layer(&mut s, "P-FIRE");
    s.execute("pline", &json!({"vertices": [[24, 240], [480, 240]]})).unwrap();
    text(&mut s, [200.0, 250.0], "2\" FP BOP @ 12'-0\"");

    // Building spec with one level.
    s.execute("bspec.set", &json!({"replace": true, "spec": {
        "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9,
                     "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "role": "wall"}]
    }})).unwrap();
    s
}

#[test]
fn pipe_systems_read_from_standard_layers() {
    let mut s = pipe_drawing();
    let r = s.execute("mep.read", &json!({})).unwrap();
    let sys_names: Vec<&str> = r["systems"].as_array().unwrap().iter()
        .filter_map(|x| x["name"].as_str())
        .collect();
    // Should find pipe systems on P-* layers.
    assert!(sys_names.iter().any(|n| n.contains("CW") || n.contains("DOMW")),
        "expected a CW system: {sys_names:?}");
    assert!(r["runs"].as_array().unwrap().len() >= 3, "expected at least 3 pipe runs");
}

#[test]
fn pipe_build_creates_3d_elements() {
    let mut s = pipe_drawing();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    let _r = s.execute("model.build", &json!({})).unwrap();
    // Check that MEP elements were built.
    let mep_count: usize = s.execute("bim.build", &json!({"discipline": "mep"})).unwrap()["built"]
        .as_object().unwrap().values()
        .filter_map(|v| v.get("mep").and_then(Value::as_u64))
        .map(|n| n as usize)
        .sum();
    assert!(mep_count >= 5, "expected at least 5 MEP elements, got {mep_count}");
}

#[test]
fn pipe_qty_reports_pipe_footage() {
    let mut s = pipe_drawing();
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("model.build", &json!({})).unwrap();
    let r = s.execute("mep.qty", &json!({})).unwrap();
    let pipe_ft = r["totals"]["pipe_length_ft"].as_f64().unwrap_or(0.0);
    let fittings = r["totals"]["fittings"].as_u64().unwrap_or(0);
    assert!(pipe_ft > 10.0, "expected pipe_length_ft > 10, got {pipe_ft}");
    assert!(fittings >= 2, "expected at least 2 fittings, got {fittings}");
}

/// A drawing with pipe tags on generic (PDF-import) layers: the tag-based fallback must
/// detect them.
fn pdf_pipe_drawing() -> Session {
    let mut s = Session::new();
    // Walls on a generic PDF layer.
    layer(&mut s, "PDF-BLACK-0.09");
    s.execute("pline", &json!({"vertices": [[-12, -12], [492, -12], [492, 372], [-12, 372]], "closed": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[-6, -6], [486, -6], [486, 366], [-6, 366]], "closed": true})).unwrap();

    // Cold water pipe as a line on a generic layer.
    layer(&mut s, "PDF-BLACK-0.18");
    s.execute("pline", &json!({"vertices": [[24, 180], [240, 180], [240, 60], [480, 60]]})).unwrap();

    // Tag identifying it as CW (on a text layer).
    layer(&mut s, "PDF-TEXT-BLACK");
    text(&mut s, [120.0, 190.0], "1\" CW");

    // Sanitary waste.
    layer(&mut s, "PDF-BLACK-0.18");
    s.execute("pline", &json!({"vertices": [[60, 300], [240, 300], [240, 360]]})).unwrap();
    layer(&mut s, "PDF-TEXT-BLACK");
    text(&mut s, [140.0, 310.0], "4\" SAN");

    // FP main.
    layer(&mut s, "PDF-BLACK-0.40");
    s.execute("pline", &json!({"vertices": [[24, 240], [480, 240]]})).unwrap();
    layer(&mut s, "PDF-TEXT-BLACK");
    text(&mut s, [200.0, 250.0], "2\" FP");

    // Building spec.
    s.execute("bspec.set", &json!({"replace": true, "spec": {
        "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9,
                     "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
        "layers": [{"layer": "PDF-BLACK-0.09", "role": "wall"}]
    }})).unwrap();
    s
}

#[test]
fn text_tag_fallback_detects_pipes_on_pdf_layers() {
    let mut s = pdf_pipe_drawing();
    let r = s.execute("mep.read", &json!({})).unwrap();
    let sys_names: Vec<&str> = r["systems"].as_array().unwrap().iter()
        .filter_map(|x| x["name"].as_str())
        .collect();
    // Should find pipe systems from text tags even without standard layer names.
    assert!(sys_names.iter().any(|n| n == &"CW"), "expected CW system from tags: {sys_names:?}");
    assert!(sys_names.iter().any(|n| n == &"SAN"), "expected SAN system from tags: {sys_names:?}");
    assert!(sys_names.iter().any(|n| n == &"FP"), "expected FP system from tags: {sys_names:?}");
    let runs = r["runs"].as_array().unwrap().len();
    assert!(runs >= 3, "expected at least 3 runs from tags, got {runs}");
}

#[test]
fn bim_build_mep_auto_reads_pipes() {
    let mut s = pipe_drawing();
    // bim.build with discipline=mep should auto-call mep.read.
    let r = s.execute("bim.build", &json!({"discipline": "mep"})).unwrap();
    let mep_count: usize = r["built"].as_object().unwrap().values()
        .filter_map(|v| v.get("mep").and_then(Value::as_u64))
        .map(|n| n as usize)
        .sum();
    assert!(mep_count >= 5, "bim.build mep should auto-read and build pipe runs, got {mep_count}");
}
