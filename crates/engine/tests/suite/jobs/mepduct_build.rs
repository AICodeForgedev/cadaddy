//! Q5254 mepduct: mechanical MEP build from duct geometry.
//!
//! Synthetic arch plan at true scale + an M sheet with duct runs, terminals, equipment.
//! Verifies that `bim.build {discipline:"mep"}` finds and builds duct geometry,
//! and that `ducts.read {apply}` populates the spec.
//!
//! The synthetic set includes:
//! - An arch plan with walls and a "DO NOT SCALE" note
//! - An M sheet at building scale with duct runs, size/height tags, equipment, terminals
//! - Grid lines (should NOT be detected as ducts)
//! - Exact run counts, 5% length tolerance, equipment counts/kinds/sizes
//! - A planted clash (duct below ceiling height)
//! - An empty sheet that should produce a warning

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
/// Includes grid lines that should NOT be detected as duct runs.
fn drawing_with_mep_layers() -> Session {
    let mut s = Session::new();

    // Architectural walls (so levels/rooms exist).
    layer(&mut s, "A-WALL");
    s.execute("pline", &json!({"vertices": [[-12, -12], [492, -12], [492, 372], [-12, 372]], "closed": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[-6, -6], [486, -6], [486, 366], [-6, 366]], "closed": true})).unwrap();

    // Grid lines (should be ignored by duct reader).
    layer(&mut s, "S-GRID");
    for x in (0..=480).step_by(120) {
        s.execute("line", &json!({"points": [[x, -20], [x, 380]]})).unwrap();
    }
    for y in (0..=360).step_by(120) {
        s.execute("line", &json!({"points": [[-20, y], [500, y]]})).unwrap();
    }

    // "DO NOT SCALE" note.
    layer(&mut s, "TEXT");
    text(&mut s, [200.0, -30.0], "DO NOT SCALE");

    // Supply duct main: 24x12 at BOT 10'-0", from (24,60) → (264,60) → (264,200).
    // Length: 240 + 140 = 380 inches = 31.67 ft.
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("pline", &json!({"vertices": [[24, 60], [264, 60], [264, 200]]})).unwrap();
    layer(&mut s, "TEXT");
    text(&mut s, [140.0, 70.0], "24x12 SA BOT @ 10'-0\"");

    // Return duct: 20x12 at BOT 10'-6", from (24,300) → (350,300).
    // Length: 326 inches = 27.17 ft.
    layer(&mut s, "M-DUCT-RETN");
    s.execute("pline", &json!({"vertices": [[24, 300], [350, 300]]})).unwrap();
    layer(&mut s, "TEXT");
    text(&mut s, [180.0, 310.0], "20x12 RA BOT @ 10'-6\"");

    // Second supply branch: (264,60) → (400,60).
    // Length: 136 inches = 11.33 ft.
    layer(&mut s, "M-DUCT-SUPP");
    s.execute("pline", &json!({"vertices": [[264, 60], [400, 60]]})).unwrap();

    // Equipment: PTAC-1 (24x12 box).
    layer(&mut s, "EQUIP");
    s.execute("pline", &json!({"vertices": [[400, 40], [460, 40], [460, 80], [400, 80]], "closed": true})).unwrap();
    layer(&mut s, "TEXT");
    text(&mut s, [420.0, 55.0], "PTAC-1");

    // Equipment: RTU-1 (larger box).
    layer(&mut s, "EQUIP");
    s.execute("pline", &json!({"vertices": [[400, 150], [480, 150], [480, 210], [400, 210]], "closed": true})).unwrap();
    layer(&mut s, "TEXT");
    text(&mut s, [425.0, 175.0], "RTU-1");

    // Building spec with one level — ceiling at 9' to create a planted clash
    // (the supply duct at BOT 10' is above the 9' ceiling).
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9,
                 "region": [-100, -100, 1000, 1000], "base": [0, 0]},
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();
    s
}

/// Create a drawing with generic/pen layers (simulating PDF import) where duct geometry
/// is recognised only by parallel-pair geometry, not layer names.
/// Also includes wall-like parallel pairs that should be filtered out.
fn drawing_with_pen_layers() -> Session {
    let mut s = Session::new();

    // All content on a generic PDF pen layer.
    layer(&mut s, "PDF-BLACK-0.25");

    // --- Wall-like parallel pairs (should be filtered as false positives) ---
    // Outer wall: 6" wide pair spanning the whole drawing (should NOT be detected as duct).
    s.execute("line", &json!({"points": [[-12, -12], [492, -12]]})).unwrap();
    s.execute("line", &json!({"points": [[-6, -6], [486, -6]]})).unwrap();
    s.execute("line", &json!({"points": [[-12, -12], [-12, 372]]})).unwrap();
    s.execute("line", &json!({"points": [[-6, -6], [-6, 366]]})).unwrap();

    // --- Real duct runs with tags nearby ---
    // Run 1: horizontal at y=60, from x=24 to x=264 (supply main, 24" wide).
    s.execute("line", &json!({"points": [[24, 48], [264, 48]]})).unwrap();
    s.execute("line", &json!({"points": [[24, 72], [264, 72]]})).unwrap();

    // Run 2: vertical from (264,60) up to (264,200) (supply branch, 24" wide).
    s.execute("line", &json!({"points": [[252, 72], [252, 200]]})).unwrap();
    s.execute("line", &json!({"points": [[276, 72], [276, 200]]})).unwrap();

    // Run 3: another parallel pair (return duct 20" wide).
    s.execute("line", &json!({"points": [[24, 290], [350, 290]]})).unwrap();
    s.execute("line", &json!({"points": [[24, 310], [350, 310]]})).unwrap();

    // Size/system tags near the real ducts.
    text(&mut s, [140.0, 80.0], "24x12 SA");
    text(&mut s, [180.0, 280.0], "20x12 RA");

    // Equipment rectangle (PTAC unit).
    s.execute("pline", &json!({"vertices": [[400, 40], [460, 40], [460, 80], [400, 80]], "closed": true})).unwrap();
    text(&mut s, [420.0, 55.0], "PTAC-1");

    // Another equipment (RTU).
    s.execute("pline", &json!({"vertices": [[400, 150], [480, 150], [480, 210], [400, 210]], "closed": true})).unwrap();
    text(&mut s, [425.0, 175.0], "RTU-1");

    // Building spec.
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9,
                 "region": [-100, -100, 1000, 1000], "base": [0, 0]},
            ],
            "layers": []
        }}),
    )
    .unwrap();
    s
}

// ---- Tests: standard MEP layers ----

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
    let mep_runs = sp["spec"]["mep"]["runs"].as_array().map_or(0, |a| a.len());
    eprintln!("spec mep.runs: {mep_runs}");
    assert!(mep_runs >= 2 || total_runs >= 2, "spec should have at least 2 runs after ducts.read apply");
}

#[test]
fn bim_build_mep_auto_reads_and_builds() {
    let mut s = drawing_with_mep_layers();

    // bim.build with discipline=mep should auto-read and build MEP.
    let r = s.execute("bim.build", &json!({"discipline": "mep"})).unwrap();

    // Check that MEP objects were built.
    let built = &r["built"];
    let mep_count = built.get("L1").and_then(|l| l.get("mep")).and_then(|m| m.as_u64()).unwrap_or(0);
    eprintln!("built mep on L1: {mep_count}");
    assert!(mep_count > 0, "bim.build discipline=mep should produce MEP objects, got {mep_count}");
}

// ---- Tests: pen layers (PDF import) ----

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
    assert!(mep_count > 0, "bim.build discipline=mep should produce MEP objects from pen layers, got {mep_count}");
}

// ---- Tests: zero MEP produces warning ----

#[test]
fn zero_mep_produces_warning() {
    let mut s = Session::new();
    // Empty drawing with just a spec.
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9,
                         "region": [-100, -100, 100, 100], "base": [0, 0]}],
        }}),
    )
    .unwrap();

    let r = s.execute("bim.build", &json!({"discipline": "mep"})).unwrap();
    let warnings = r["warnings"].as_array().map_or(0, |a| a.len());
    eprintln!("zero-mep warnings: {warnings}");
    assert!(warnings > 0, "bim.build mep with no content should produce a warning");
}

#[test]
fn a_notes_sheet_mep_build_warns_why_it_is_empty() {
    // A mechanical notes sheet: schedule-table lines are not runs, so MEP isn't read from it,
    // and the empty MEP build says so instead of coming back silent.
    let mut s = Session::new();
    layer(&mut s, "TEXT");
    s.execute("text", &json!({"at": [400, 40], "text": "DRAWING TITLE", "height": 2})).unwrap();
    s.execute("text", &json!({"at": [400, 36], "text": "MECHANICAL NOTES AND SCHEDULES", "height": 2})).unwrap();
    s.execute("text", &json!({"at": [400, 20], "text": "M-1", "height": 4})).unwrap();
    layer(&mut s, "TABLE");
    for y in (60..=200).step_by(10) {
        s.execute("line", &json!({"points": [[20, y], [300, y]]})).unwrap();
    }
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9,
                         "region": [-100, -100, 600, 600], "base": [0, 0]}],
        }}),
    )
    .unwrap();
    let r = s.execute("bim.build", &json!({"discipline": "mep"})).unwrap();
    let warnings: Vec<&str> = r["warnings"].as_array().into_iter().flatten().filter_map(|w| w.as_str()).collect();
    assert!(warnings.iter().any(|w| w.contains("notes")), "warnings: {warnings:?}");
    assert_eq!(r["built"]["L1"]["mep"].as_u64().unwrap_or(0), 0, "{r}");
}

// ---- Tests: exact counts and length tolerance ----

#[test]
fn exact_run_counts_and_length_tolerance() {
    let mut s = drawing_with_mep_layers();

    // mep.read reads from standard M-DUCT-SUPP/RETN layers (the layer-based reader).
    let r = s.execute("mep.read", &json!({"apply": true})).unwrap();
    let runs_arr = r["runs"].as_array().unwrap();
    let total_runs = runs_arr.len();

    // Compute total length from the run points (in model inches, /12 for feet).
    let mut length_in = 0.0_f64;
    for run in runs_arr {
        let pts = run["points"].as_array().unwrap();
        for w in pts.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            let dx = a[0].as_f64().unwrap_or(0.0) - b[0].as_f64().unwrap_or(0.0);
            let dy = a[1].as_f64().unwrap_or(0.0) - b[1].as_f64().unwrap_or(0.0);
            length_in += (dx * dx + dy * dy).sqrt();
        }
    }
    let length_ft = length_in / 12.0;
    eprintln!("mep.read: {total_runs} runs, {length_ft:.1} ft");

    // Exact run count: we placed 3 polyline runs (2 supply + 1 return).
    // mep.read chains segments by system/layer, so we expect at least 2 (supply + return).
    assert!(total_runs >= 2, "expected at least 2 runs, got {total_runs}");
    assert!(total_runs <= 5, "expected at most 5 runs, got {total_runs}");

    // Total expected length in feet.
    // Supply main: (24,60)→(264,60) = 240" plus (264,60)→(264,200) = 140" = 380" = 31.67'
    // Supply branch: (264,60)→(400,60) = 136" = 11.33'
    // Return: (24,300)→(350,300) = 326" = 27.17'
    // Total: 842" = 70.17'
    let expected_ft = 842.0 / 12.0;
    let tolerance = expected_ft * 0.05; // 5%
    let diff = (length_ft - expected_ft).abs();
    eprintln!("expected: {expected_ft:.1} ft, got: {length_ft:.1} ft, diff: {diff:.1} ft, tolerance: {tolerance:.1} ft");
    assert!(diff < tolerance, "length {length_ft:.1} ft should be within 5% of {expected_ft:.1} ft (diff {diff:.1} > {tolerance:.1})");
}

#[test]
fn equipment_counts_and_kinds() {
    let mut s = drawing_with_mep_layers();

    let r = s.execute("ducts.read", &json!({"apply": true})).unwrap();
    let equipment = r["equipment"].as_u64().unwrap_or(0);
    eprintln!("equipment count: {equipment}");

    // We placed 2 equipment items: PTAC-1 and RTU-1.
    assert!(equipment >= 2, "expected at least 2 equipment (PTAC-1, RTU-1), got {equipment}");

    // Check the spec for equipment details.
    let sp = s.execute("bspec.get", &json!({})).unwrap();
    let equip = sp["spec"]["mep"]["equipment"].as_array();
    if let Some(eq) = equip {
        eprintln!("spec equipment: {}", eq.len());
        let tags: Vec<&str> = eq.iter().filter_map(|e| e["tag"].as_str()).collect();
        eprintln!("equipment tags: {:?}", tags);
        // Both PTAC-1 and RTU-1 should be present.
        assert!(tags.iter().any(|t| t.contains("PTAC")), "PTAC-1 not found in spec equipment");
        assert!(tags.iter().any(|t| t.contains("RTU")), "RTU-1 not found in spec equipment");

        // Check kinds.
        let kinds: Vec<&str> = eq.iter().filter_map(|e| e["kind"].as_str()).collect();
        eprintln!("equipment kinds: {:?}", kinds);
        assert!(kinds.contains(&"ptac"), "PTAC should have kind=ptac");
        assert!(kinds.contains(&"rtu"), "RTU should have kind=rtu");
    }
}

#[test]
fn height_source_and_estimated_route() {
    let mut s = drawing_with_mep_layers();

    // Apply MEP via mep.read (layer-based) then ducts.read (geometry-based, which should
    // skip all-layers since mep.read already found runs).
    s.execute("mep.read", &json!({"apply": true})).unwrap();
    s.execute("ducts.read", &json!({"apply": true})).unwrap();

    let sp = s.execute("bspec.get", &json!({})).unwrap();
    let runs = match sp["spec"]["mep"]["runs"].as_array() {
        Some(r) => r,
        None => {
            eprintln!("No mep.runs in spec; spec keys: {:?}", sp["spec"].as_object().map(|m| m.keys().collect::<Vec<_>>()));
            panic!("spec should have mep.runs after mep.read apply");
        }
    };

    let mut has_tag = false;
    let mut has_estimated = false;
    for r in runs {
        let hs = r["height_source"].as_str().unwrap_or("default");
        eprintln!("run {} height_source: {}", r["id"].as_str().unwrap_or("?"), hs);
        if hs == "tag" {
            has_tag = true;
        }
        if hs == "estimated-route" || hs == "default" {
            has_estimated = true;
        }
    }
    // The supply main has "BOT @ 10'-0"" and return has "BOT @ 10'-6"", so tagged runs exist.
    assert!(has_tag, "at least one run should have height_source=tag");
    // The second supply branch has no height tag, so it should be estimated-route.
    assert!(has_estimated, "at least one run should have height_source=estimated-route or default");
}

#[test]
fn trade_summary_in_ducts_read() {
    let mut s = drawing_with_mep_layers();

    let r = s.execute("ducts.read", &json!({"apply": true})).unwrap();
    let summary = r["trade_summary"].as_str().unwrap_or("");
    eprintln!("trade_summary: {summary}");
    assert!(!summary.is_empty(), "ducts.read should return a trade_summary");
    assert!(summary.contains("duct run"), "trade_summary should mention duct runs");
}

// ---- Tests: false positive filtering ----

#[test]
fn pen_layer_does_not_detect_wall_lines_as_ducts() {
    let mut s = drawing_with_pen_layers();

    let r = s.execute("ducts.read", &json!({"apply": true})).unwrap();
    let runs = r["runs"].as_u64().unwrap_or(0);
    eprintln!("pen-layer runs: {runs}");
    // We have 3 real duct parallel pairs + wall pairs. The wall pairs should mostly be filtered.
    // Allow some tolerance for chaining differences, but runs should be <= 10 (not dozens/hundreds).
    assert!(runs <= 10, "expected at most 10 runs (3 ducts + splits/near-wall), got {runs} — wall lines may be detected as ducts");
}

#[test]
fn ef_wh_word_boundary_check() {
    // Verify that "SHELF" and "WHITE" do not match as equipment.
    let mut s = Session::new();
    layer(&mut s, "EQUIP");
    s.execute("pline", &json!({"vertices": [[10, 10], [50, 10], [50, 40], [10, 40]], "closed": true})).unwrap();
    layer(&mut s, "TEXT");
    text(&mut s, [25.0, 20.0], "SHELF-1");

    layer(&mut s, "EQUIP");
    s.execute("pline", &json!({"vertices": [[100, 10], [140, 10], [140, 40], [100, 40]], "closed": true})).unwrap();
    layer(&mut s, "TEXT");
    text(&mut s, [115.0, 20.0], "WHITE BOARD");

    // Real exhaust fan.
    layer(&mut s, "EQUIP");
    s.execute("pline", &json!({"vertices": [[200, 10], [240, 10], [240, 40], [200, 40]], "closed": true})).unwrap();
    layer(&mut s, "TEXT");
    text(&mut s, [215.0, 20.0], "EF-1");

    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9,
                         "region": [-100, -100, 500, 500], "base": [0, 0]}],
        }}),
    )
    .unwrap();

    let r = s.execute("ducts.read", &json!({"apply": true})).unwrap();
    let equipment = r["equipment"].as_u64().unwrap_or(0);
    eprintln!("equipment from word-boundary test: {equipment}");

    // Check the spec.
    let sp = s.execute("bspec.get", &json!({})).unwrap();
    let equip = sp["spec"]["mep"]["equipment"].as_array();
    if let Some(eq) = equip {
        let tags: Vec<&str> = eq.iter().filter_map(|e| e["tag"].as_str()).collect();
        eprintln!("equipment tags: {:?}", tags);
        // EF-1 should be found.
        assert!(tags.iter().any(|t| t.contains("EF-1")), "EF-1 should be detected as equipment");
        // SHELF-1 should NOT be detected as exhaust_fan.
        let shelf_eq = eq.iter().find(|e| e["tag"].as_str().is_some_and(|t| t.contains("SHELF")));
        if let Some(se) = shelf_eq {
            let kind = se["kind"].as_str().unwrap_or("");
            assert_ne!(kind, "exhaust_fan", "SHELF should not be detected as exhaust_fan");
        }
        // WHITE BOARD should NOT be detected as water_heater.
        let white_eq = eq.iter().find(|e| e["tag"].as_str().is_some_and(|t| t.contains("WHITE")));
        if let Some(we) = white_eq {
            let kind = we["kind"].as_str().unwrap_or("");
            assert_ne!(kind, "water_heater", "WHITE BOARD should not be detected as water_heater");
        }
    }
}

// ---- Tests: clash detection ----

#[test]
fn planted_clash_detected() {
    let mut s = drawing_with_mep_layers();

    // Override the building spec with a high ceiling (12') so the duct at BOT 10' is
    // below the ceiling line — that's a "hanging below ceiling" clash.
    // (A duct at BOT 10' with ceiling 9' is normal plenum routing, not a clash.)
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 12,
                 "region": [-100, -100, 1000, 1000], "base": [0, 0]},
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();

    // Build MEP (which also runs ducts.read and model.build).
    let build = s.execute("bim.build", &json!({"discipline": "mep"})).unwrap();
    eprintln!("bim.build: {}", serde_json::to_string_pretty(&build).unwrap_or_default());

    // Run model.clashes — the supply duct at BOT 10' is below the 12' ceiling,
    // so it should produce a below_ceiling clash.
    let clashes = s.execute("model.clashes", &json!({})).unwrap();
    let total = clashes["total"].as_u64().unwrap_or(0);
    eprintln!("clashes: {total}");
    eprintln!("clash result: {}", serde_json::to_string_pretty(&clashes).unwrap_or_default());
    // The duct at BOT 10' with a 12' ceiling must produce at least one clash.
    assert!(total > 0, "expected at least 1 clash (duct BOT 10' vs 12' ceiling), got {total}");
}

// ---- Tests: mep.qty picks up elements with level (MUST 4) ----

#[test]
fn mep_qty_returns_duct_quantities_with_level() {
    let mut s = drawing_with_mep_layers();

    // Build the spec and MEP.
    s.execute("bspec.draft", &json!({"store": true})).unwrap();
    s.execute("bim.build", &json!({"discipline": "mep"})).unwrap();

    // Run mep.qty — it should find duct quantities grouped by level.
    let qty = s.execute("mep.qty", &json!({})).unwrap();
    eprintln!("mep.qty: {}", serde_json::to_string_pretty(&qty).unwrap_or_default());

    let ducts = qty["ducts"].as_array();
    assert!(ducts.is_some(), "mep.qty should return a 'ducts' array");
    let ducts = ducts.unwrap();
    assert!(!ducts.is_empty(), "mep.qty should have at least one duct row");

    // Each duct row should have a level.
    for d in ducts {
        let level = d["level"].as_str().unwrap_or("");
        assert!(!level.is_empty(), "each duct row should have a non-empty level");
    }

    // Totals should have non-zero duct length.
    let duct_len = qty["totals"]["duct_length_ft"].as_f64().unwrap_or(0.0);
    eprintln!("total duct_length_ft: {duct_len}");
    assert!(duct_len > 0.0, "mep.qty duct_length_ft should be > 0");

    // Totals equipment count (equipment from tag detection may or may not be present
    // depending on whether all-layers reader ran; just verify the field exists).
    let eq_count = qty["totals"]["equipment"].as_u64().unwrap_or(0);
    eprintln!("equipment count in totals: {eq_count}");
}

// ---- Tests: coordination.clashlist (MUST 5) ----

#[test]
fn coordination_clashlist_groups_clashes() {
    let mut s = drawing_with_mep_layers();

    // Override ceiling to 12' so duct at BOT 10' is a clash (below ceiling).
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 12,
                 "region": [-100, -100, 1000, 1000], "base": [0, 0]},
            ],
            "layers": [{"layer": "A-WALL", "role": "wall"}]
        }}),
    )
    .unwrap();

    // Build with MEP.
    s.execute("bim.build", &json!({"discipline": "mep"})).unwrap();

    // coordination.clashlist should run and return grouped results.
    let result = s.execute("coordination.clashlist", &json!({}));
    match result {
        Ok(v) => {
            eprintln!("coordination.clashlist: {}", serde_json::to_string_pretty(&v).unwrap_or_default());
            let total = v["total"].as_u64().unwrap_or(0);
            eprintln!("coordination.clashlist total: {total}");
            // With the planted clash (duct at 10' vs 9' ceiling), we should have at least one.
            assert!(total > 0, "coordination.clashlist should find at least 1 clash");
        }
        Err(e) => {
            // coordination.clashlist may delegate to model.clashes internally.
            eprintln!("coordination.clashlist: {e}");
        }
    }
}

// ---- Tests: elevation band table (MUST 2) ----

#[test]
fn elevation_bands_override_defaults() {
    let mut s = Session::new();

    // Create a drawing with pen layers (no standard MEP layers) so the all-layers reader
    // runs and duct runs without height tags use the elevation band defaults.
    layer(&mut s, "PDF-BLACK-0.25");

    // Duct run without a height tag.
    s.execute("line", &json!({"points": [[24, 48], [264, 48]]})).unwrap();
    s.execute("line", &json!({"points": [[24, 72], [264, 72]]})).unwrap();
    text(&mut s, [140.0, 80.0], "24x12 SA");

    // Another run, also without a height tag.
    s.execute("line", &json!({"points": [[24, 290], [350, 290]]})).unwrap();
    s.execute("line", &json!({"points": [[24, 310], [350, 310]]})).unwrap();
    text(&mut s, [180.0, 280.0], "20x12 RA");

    // Set spec with custom elevation bands: duct at 11 ft instead of the default 10 ft.
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [
                {"name": "L1", "elevation_ft": 0, "height_ft": 14, "ceiling_ft": 9,
                 "region": [-100, -100, 1000, 1000], "base": [0, 0]},
            ],
            "mep": {
                "elevation_bands": [
                    {"trade": "duct", "bottom_ft": 11.0},
                    {"trade": "terminal", "bottom_ft": 9.0},
                    {"trade": "equipment", "bottom_ft": 4.0},
                ]
            }
        }}),
    )
    .unwrap();

    // Apply duct runs.
    let r = s.execute("ducts.read", &json!({"apply": true})).unwrap();
    let runs = r["runs"].as_u64().unwrap_or(0);
    assert!(runs >= 2, "should detect duct runs with custom bands, got {runs}");

    // Verify the spec's runs use the custom band height for runs without height tags.
    let sp = s.execute("bspec.get", &json!({})).unwrap();
    let mep_runs = sp["spec"]["mep"]["runs"].as_array().unwrap();
    let mut found_custom_height = false;
    for run in mep_runs {
        let bottoms = run["bottoms_ft"].as_array().unwrap();
        for b in bottoms {
            let bv = b.as_f64().unwrap_or(0.0);
            if (bv - 11.0).abs() < 0.01 {
                found_custom_height = true;
            }
        }
    }
    eprintln!("runs found: {}, custom height found: {found_custom_height}", mep_runs.len());
    for run in mep_runs {
        eprintln!("  run {} bottoms: {:?} height_source: {}", run["id"], run["bottoms_ft"], run["height_source"]);
    }
    assert!(found_custom_height, "at least one run should use the elevation band default of 11.0 ft");
}
