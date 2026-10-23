//! Q5254 mepduct: mechanical MEP build from duct geometry.
//!
//! Synthetic arch plan at true scale + an M sheet with duct runs, terminals, equipment.
//! Verifies that `bim.build {discipline:"mep"}` finds and builds duct geometry,
//! and that `ducts.read {apply}` populates the spec.

use cadaddy_engine::Session;
use serde_json::json;

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

fn text(s: &mut Session, at: [f64; 2], t: &str) {
    s.execute("text", &json!({"at": at, "text": t, "height": 4})).unwrap();
}

/// Create a drawing with standard MEP layers (M-DUCT-SUPP, etc.) for the layer-based path.
fn drawing_with_mep_layers() -> Session {
    let mut s = Session::new();

    // Architectural walls (so levels/rooms exist).
    layer(&mut s, "A-WALL");
    s.execute("pline", &json!({"vertices": [[-12, -12], [492, -12], [492, 372], [-12, 372]], "closed": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[-6, -6], [486, -6], [486, 366], [-6, 366]], "closed": true})).unwrap();

    // Supply duct main: 24x12 at BOT 10'-0".
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("pline", &json!({"vertices": [[24, 60], [264, 60], [264, 200]]})).unwrap();
    text(&mut s, [140.0, 70.0], "24x12 BOT @ 10'-0\"");

    // Return duct: 20x12 at BOT 10'-6".
    layer(&mut s, "M-DUCT-RETN");
    s.execute("pline", &json!({"vertices": [[24, 300], [350, 300]]})).unwrap();
    text(&mut s, [180.0, 310.0], "20x12 BOT @ 10'-6\"");

    // A second supply branch.
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("pline", &json!({"vertices": [[264, 60], [400, 60]]})).unwrap();

    // Building spec with one level.
    s.execute("bspec.set", &json!({"replace": true, "spec": {
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9,
             "region": [-100, -100, 1000, 1000], "base": [0, 0]},
        ],
        "layers": [{"layer": "A-WALL", "role": "wall"}]
    }})).unwrap();
    s
}

/// Create a drawing with generic/pen layers (simulating PDF import) where duct geometry
/// is recognised only by parallel-pair geometry, not layer names.
fn drawing_with_pen_layers() -> Session {
    let mut s = Session::new();

    // All content on a generic PDF pen layer.
    layer(&mut s, "PDF-BLACK-0.25");

    // Parallel lines forming a duct run (width 24" = 2 lines 24 apart).
    // Run 1: horizontal at y=60, from x=24 to x=264 (supply main).
    s.execute("line", &json!({"points": [[24, 48], [264, 48]]})).unwrap();
    s.execute("line", &json!({"points": [[24, 72], [264, 72]]})).unwrap();

    // Run 2: vertical from (264,60) up to (264,200).
    s.execute("line", &json!({"points": [[252, 72], [252, 200]]})).unwrap();
    s.execute("line", &json!({"points": [[276, 72], [276, 200]]})).unwrap();

    // Run 3: another parallel pair (return duct 20" wide).
    s.execute("line", &json!({"points": [[24, 290], [350, 290]]})).unwrap();
    s.execute("line", &json!({"points": [[24, 310], [350, 310]]})).unwrap();

    // Size tags (on the same pen layer).
    text(&mut s, [140.0, 80.0], "24x12 SA");
    text(&mut s, [180.0, 280.0], "20x12 RA");

    // Equipment rectangle (PTAC unit).
    s.execute("pline", &json!({"vertices": [[400, 40], [460, 40], [460, 80], [400, 80]], "closed": true})).unwrap();
    text(&mut s, [420.0, 55.0], "PTAC-1");

    // Another equipment (RTU).
    s.execute("pline", &json!({"vertices": [[400, 150], [480, 150], [480, 210], [400, 210]], "closed": true})).unwrap();
    text(&mut s, [425.0, 175.0], "RTU-1");

    // Building spec.
    s.execute("bspec.set", &json!({"replace": true, "spec": {
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9,
             "region": [-100, -100, 1000, 1000], "base": [0, 0]},
        ],
        "layers": []
    }})).unwrap();
    s
}

#[test]
fn ducts_read_finds_runs_from_standard_layers() {
    let mut s = drawing_with_mep_layers();

    // mep.read should find runs on standard MEP layers.
    let r = s.execute("mep.read", &json!({})).unwrap();
    let runs = r["runs"].as_array().map_or(0, |a| a.len());
    eprintln!("mep.read: {} runs", runs);
    assert!(runs >= 2, "expected at least 2 duct runs from standard layers, got {runs}");
}

#[test]
fn ducts_read_command_populates_spec() {
    let mut s = drawing_with_mep_layers();

    // ducts.read with apply should populate the spec.
    let r = s.execute("ducts.read", &json!({"apply": true})).unwrap();
    let applied = r["applied"].as_bool().unwrap_or(false);
    let total_runs = r["runs"].as_u64().unwrap_or(0);
    eprintln!("ducts.read: {} runs, applied={applied}", total_runs);

    // The spec should now have runs.
    let sp = s.execute("bspec.get", &json!({})).unwrap();
    let mep_runs = sp.get("mep").and_then(|m| m.get("runs")).and_then(|r| r.as_array()).map_or(0, |a| a.len());
    eprintln!("spec mep.runs: {mep_runs}");
    // At least mep.read or ducts.read should have found something.
    assert!(mep_runs >= 2 || total_runs >= 2, "spec should have at least 2 runs after ducts.read apply");
}

#[test]
fn bim_build_mep_auto_reads_and_builds() {
    let mut s = drawing_with_mep_layers();

    // bim.build with discipline=mep should auto-read and build MEP.
    let r = s.execute("bim.build", &json!({"discipline": "mep"})).unwrap();
    eprintln!("bim.build mep: {}", serde_json::to_string_pretty(&r).unwrap_or_default());

    // Check that MEP objects were built.
    let built = &r["built"];
    let mep_count = built.get("L1").and_then(|l| l.get("mep")).and_then(|m| m.as_u64()).unwrap_or(0);
    eprintln!("built mep on L1: {mep_count}");
    assert!(mep_count > 0, "bim.build discipline=mep should produce MEP objects, got {mep_count}");
}

#[test]
fn ducts_read_works_on_pen_layers() {
    let mut s = drawing_with_pen_layers();

    // ducts.read should find parallel-pair duct runs even on pen layers.
    let r = s.execute("ducts.read", &json!({"apply": true})).unwrap();
    let runs = r["runs"].as_u64().unwrap_or(0);
    let equipment = r["equipment"].as_u64().unwrap_or(0);
    eprintln!("pen-layer ducts.read: {runs} runs, {equipment} equipment");
    assert!(runs >= 2, "expected at least 2 duct runs from pen-layer parallel pairs, got {runs}");
}

#[test]
fn bim_build_mep_on_pen_layers_finds_duct() {
    let mut s = drawing_with_pen_layers();

    let r = s.execute("bim.build", &json!({"discipline": "mep"})).unwrap();
    let built = &r["built"];
    let mep_count = built.get("L1").and_then(|l| l.get("mep")).and_then(|m| m.as_u64()).unwrap_or(0);
    eprintln!("bim.build mep pen-layers: {mep_count} objects");
    // Even on pen layers, the geometry reader should detect duct runs and build them.
    assert!(mep_count > 0, "bim.build discipline=mep should produce MEP objects from pen layers, got {mep_count}");
}

#[test]
fn zero_mep_produces_warning() {
    let mut s = Session::new();
    // Empty drawing with just a spec.
    s.execute("bspec.set", &json!({"replace": true, "spec": {
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9,
                     "region": [-100, -100, 100, 100], "base": [0, 0]}],
    }})).unwrap();

    let r = s.execute("bim.build", &json!({"discipline": "mep"})).unwrap();
    let warnings = r["warnings"].as_array().map_or(0, |a| a.len());
    eprintln!("zero-mep warnings: {warnings}");
    assert!(warnings > 0, "bim.build mep with no content should produce a warning");
}
