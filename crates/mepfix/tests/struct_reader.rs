//! Tests for the structural reader against the Q254 fixture generator's ground truth.
//!
//! The fixture set generates DXF structural framing plans with known columns, beams, slabs
//! and openings. The structural reader must recover those from the DXF geometry and text,
//! using the sheet alignment computed by the sheet reader.

use cadaddy_mepfix::sheet_reader;
use cadaddy_mepfix::struct_reader;
use cadaddy_mepfix::{generate, truth_for};

/// Generate a fixture set, read back the DXFs, and return (truth, structural model).
fn fixture(seed: u64) -> (cadaddy_mepfix::truth::Truth, struct_reader::StructuralModel) {
    let set = generate(seed).unwrap_or_else(|e| panic!("generate({seed}): {e}"));
    let dxf_files: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();

    // Build sheet map (needed for alignment).
    let sheet_map = sheet_reader::recognise_fixture_set(&dxf_files);

    // Parse the DXF drawings for the struct reader.
    let drawings: Vec<(String, cadaddy_doc::Drawing)> = dxf_files
        .iter()
        .filter_map(|(name, bytes)| {
            let num = name.trim_end_matches(".dxf");
            cadaddy_io::read_dxf(bytes).ok().map(|d| (num.to_string(), d))
        })
        .collect();
    let refs: Vec<(String, &cadaddy_doc::Drawing)> = drawings.iter().map(|(n, d)| (n.clone(), d)).collect();

    let model = struct_reader::read_structure(&sheet_map, &refs);
    (set.truth, model)
}

#[test]
fn truth_has_slabs_and_openings() {
    // Verify the fixture generator now produces slabs and openings.
    for seed in 1..=5 {
        let t = truth_for(seed);
        assert!(!t.building.slabs.is_empty(), "seed {seed}: truth should have slabs");
        assert!(!t.building.openings.is_empty(), "seed {seed}: truth should have openings");
        // Two slabs: level 2 (composite) and roof.
        assert_eq!(t.building.slabs.len(), 2, "seed {seed}: expected 2 slabs, got {}", t.building.slabs.len());
        let composite = t.building.slabs.iter().find(|s| s.kind == "composite");
        let roof = t.building.slabs.iter().find(|s| s.kind == "roof");
        assert!(composite.is_some(), "seed {seed}: no composite slab");
        assert!(roof.is_some(), "seed {seed}: no roof slab");
        let c = composite.unwrap();
        assert!(c.concrete_in > 0.0, "seed {seed}: composite slab has no concrete");
        assert!(c.deck_in > 0.0, "seed {seed}: composite slab has no deck");
    }
}

#[test]
fn columns_are_detected() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let ctx = format!("seed {seed}");
        assert!(!model.columns.is_empty(), "{ctx}: no columns detected (truth has {})", truth.building.columns.len());
        // Every truth column should be found by grid intersection.
        let mut found = 0;
        for tc in &truth.building.columns {
            if model.columns.iter().any(|c| c.grid == tc.grid) {
                found += 1;
            }
        }
        let recall = found as f64 / truth.building.columns.len() as f64;
        assert!(recall >= 0.8, "{ctx}: column recall {recall:.2} ({found}/{}) is too low", truth.building.columns.len());
    }
}

#[test]
fn column_footprints_are_reasonable() {
    // The drawn column size includes a minimum display size for readability, so the
    // detected footprint may be larger than the actual section. We check that the
    // detected size is at least as large as the truth's max dimension and not wildly off.
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let ctx = format!("seed {seed}");
        let mut reasonable = 0;
        let mut total = 0;
        for tc in &truth.building.columns {
            if let Some(dc) = model.columns.iter().find(|c| c.grid == tc.grid) {
                total += 1;
                let max_truth = tc.footprint_in[0].max(tc.footprint_in[1]);
                let max_det = dc.footprint_in[0].max(dc.footprint_in[1]);
                // Display size is >= truth and within 3x (scale-dependent enlargement).
                if max_det >= max_truth * 0.8 && max_det < max_truth * 3.5 {
                    reasonable += 1;
                }
            }
        }
        if total > 0 {
            let acc = reasonable as f64 / total as f64;
            assert!(acc >= 0.8, "{ctx}: column footprint reasonableness {acc:.2} ({reasonable}/{total}) is too low");
        }
    }
}

#[test]
fn column_size_is_read_from_notes() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let ctx = format!("seed {seed}");
        let truth_size = &truth.building.columns[0].size;
        if let Some(det_size) = &model.column_size {
            assert!(det_size.eq_ignore_ascii_case(truth_size), "{ctx}: column size {det_size:?} vs truth {truth_size:?}");
        }
    }
}

#[test]
fn beams_are_detected() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let ctx = format!("seed {seed}");
        // Structural sheets are for levels 2 and R (the framing above).
        for level in truth.building.levels.iter().filter(|l| l.code != "1") {
            let truth_beams: Vec<_> = truth.building.beams.iter().filter(|b| b.level == level.code).collect();
            if truth_beams.is_empty() {
                continue;
            }
            let detected = model.beams.get(&level.code);
            assert!(detected.is_some(), "{ctx}: no beams detected for level {} (truth has {})", level.code, truth_beams.len());
            let det = detected.unwrap();
            assert!(!det.is_empty(), "{ctx}: level {} has 0 detected beams (truth has {})", level.code, truth_beams.len());
        }
    }
}

#[test]
fn beam_sizes_are_read() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let ctx = format!("seed {seed}");
        let score = struct_reader::score_structure(&model, &truth);
        if score.beams_found > 0 {
            let acc = score.beam_size_correct as f64 / score.beams_found as f64;
            assert!(acc >= 0.5, "{ctx}: beam size accuracy {acc:.2} ({}/{})", score.beam_size_correct, score.beams_found);
        }
    }
}

#[test]
fn tos_is_read_from_notes() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let ctx = format!("seed {seed}");
        // Each structural sheet should have a TOS note.
        for level in truth.building.levels.iter().filter(|l| l.code != "1") {
            let truth_tos = truth.building.beams.iter().find(|b| b.level == level.code).map(|b| b.tos_ft);
            if let (Some(tt), Some(dt)) = (truth_tos, model.tos_ft.get(&level.code)) {
                assert!((tt - dt).abs() < 0.2, "{ctx}: level {} TOS {dt:.3} vs truth {tt:.3}", level.code);
            }
        }
    }
}

#[test]
fn slab_notes_are_read() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let ctx = format!("seed {seed}");
        // The reader should find at least the composite slab from the level 2 sheet.
        let composite = model.slabs.iter().find(|s| s.kind == "composite");
        assert!(composite.is_some(), "{ctx}: no composite slab detected from notes (truth has {} slabs)", truth.building.slabs.len());
        if let Some(ds) = composite {
            let ts = truth.building.slabs.iter().find(|s| s.kind == "composite").unwrap();
            assert!((ds.concrete_in - ts.concrete_in).abs() < 1.0, "{ctx}: slab concrete {} vs truth {}", ds.concrete_in, ts.concrete_in);
        }
    }
}

#[test]
fn openings_are_detected() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let ctx = format!("seed {seed}");
        if truth.building.openings.is_empty() {
            continue;
        }
        // At least some openings should be found.
        let total_detected: usize = model.openings.values().map(|v| v.len()).sum();
        assert!(total_detected > 0, "{ctx}: no openings detected (truth has {})", truth.building.openings.len());
    }
}

#[test]
fn overall_score_is_reasonable() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let ctx = format!("seed {seed}");
        let score = struct_reader::score_structure(&model, &truth);
        let recall = score.recall();
        assert!(
            recall >= 0.3,
            "{ctx}: overall structural recall {recall:.2} is too low\n  columns: {}/{}\n  beams: {}/{}\n  tos: {}/{}\n  slabs: {}/{}\n  openings: {}/{}",
            score.columns_found,
            score.columns_total,
            score.beams_found,
            score.beams_total,
            score.tos_correct,
            score.tos_total,
            score.slabs_found,
            score.slabs_total,
            score.openings_found,
            score.openings_total,
        );
    }
}

#[test]
fn score_structure_matches_truth_counts() {
    let (truth, model) = fixture(3);
    let score = struct_reader::score_structure(&model, &truth);
    assert_eq!(score.columns_total, truth.building.columns.len(), "columns_total mismatch");
    assert_eq!(score.slabs_total, truth.building.slabs.len(), "slabs_total mismatch");
    assert_eq!(score.openings_total, truth.building.openings.len(), "openings_total mismatch");
}
