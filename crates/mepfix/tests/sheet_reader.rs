//! Tests for sheet recognition and alignment against the Q254 fixture generator's ground truth.
//!
//! The fixture set generates DXF files with known title blocks, scales, grid positions and
//! coordinate transforms. The sheet reader must recover those from the DXF text and geometry
//! alone, and its alignment must map model coordinates back to building coordinates accurately.

use cadaddy_mepfix::sheet_reader;
use cadaddy_mepfix::{generate, layout};

/// Generate a fixture set, read back the DXFs, and return (truth, sheet map).
fn fixture(seed: u64) -> (cadaddy_mepfix::truth::Truth, sheet_reader::SheetMap) {
    let set = generate(seed).unwrap_or_else(|e| {
        // Don't panic in non-test code; this IS test code.
        panic!("generate({seed}): {e}");
    });
    let dxf_files: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = sheet_reader::recognise_fixture_set(&dxf_files);
    (set.truth, map)
}

#[test]
fn every_sheet_is_recognised() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        let ctx = format!("seed {seed}");
        // Every truth sheet should appear in the map by number.
        for ts in &truth.sheets {
            assert!(
                map.sheets.iter().any(|s| s.number == ts.number),
                "{ctx}: sheet {} not found in map (found: {:?})",
                ts.number,
                map.sheets.iter().map(|s| &s.number).collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn sheet_numbers_are_correct() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        let ctx = format!("seed {seed}");
        for ts in &truth.sheets {
            let found = map.sheets.iter().find(|s| s.number == ts.number);
            assert!(found.is_some(), "{ctx}: {} not found", ts.number);
        }
    }
}

#[test]
fn disciplines_are_correct() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        let ctx = format!("seed {seed}");
        for ts in &truth.sheets {
            if let Some(f) = map.sheets.iter().find(|s| s.number == ts.number) {
                assert_eq!(f.discipline, ts.discipline, "{ctx}: {} discipline", ts.number);
            }
        }
    }
}

#[test]
fn levels_are_correct() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        let ctx = format!("seed {seed}");
        for ts in &truth.sheets {
            if let Some(f) = map.sheets.iter().find(|s| s.number == ts.number) {
                assert_eq!(f.level, ts.level, "{ctx}: {} level", ts.number);
            }
        }
    }
}

#[test]
fn scales_are_detected() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        let ctx = format!("seed {seed}");
        for ts in &truth.sheets {
            if let (Some(truth_scale), Some(f)) = (&ts.scale_text, map.sheets.iter().find(|s| s.number == ts.number)) {
                assert!(f.scale_text.is_some(), "{ctx}: {} has no detected scale (truth: {truth_scale})", ts.number,);
                assert_eq!(f.scale_text.as_deref(), Some(truth_scale.as_str()), "{ctx}: {} scale text", ts.number,);
                assert!((f.scale - ts.scale).abs() < 0.01, "{ctx}: {} scale value {:.4} vs truth {:.4}", ts.number, f.scale, ts.scale,);
            }
        }
    }
}

#[test]
fn grid_is_detected_on_plan_sheets() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        let ctx = format!("seed {seed}");
        let n_x = truth.building.grid.x.len();
        let n_y = truth.building.grid.y.len();
        for ts in &truth.sheets {
            if ts.level.is_none() {
                continue; // Covers and schedules have no grid.
            }
            // Enlarged plans clip the drawing, so their grid bubbles may be partially or
            // fully outside the window; skip them.
            if ts.window_ft.is_some() {
                continue;
            }
            if let Some(f) = map.sheets.iter().find(|s| s.number == ts.number) {
                assert!(!f.grid_x.is_empty() || !f.grid_y.is_empty(), "{ctx}: {} has no grid detected (truth has {n_x} x, {n_y} y)", ts.number,);
            }
        }
    }
}

#[test]
fn alignment_recovers_building_coordinates() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        let ctx = format!("seed {seed}");
        let scores = sheet_reader::score_sheet_map(&map, &truth.sheets, &truth.building.grid.x, &truth.building.grid.y);
        for (ts, sc) in truth.sheets.iter().zip(&scores) {
            if ts.level.is_none() {
                continue;
            }
            if let Some(err) = sc.grid_error_ft {
                assert!(err < 1.0, "{ctx}: {} alignment error {err:.4} ft (too large)", ts.number,);
            }
        }
    }
}

#[test]
fn rotated_sheets_are_aligned() {
    // Find seeds with rotated sheets and verify alignment still works.
    let mut tested_rotated = false;
    for seed in 1..=30 {
        let (truth, map) = fixture(seed);
        for ts in &truth.sheets {
            if ts.model_from_building.rotation_deg.abs() > 1.0 {
                tested_rotated = true;
                if let Some(f) = map.sheets.iter().find(|s| s.number == ts.number)
                    && let Some(alignment) = &f.alignment
                {
                    for gx in &truth.building.grid.x {
                        for gy in &truth.building.grid.y {
                            let building_pt = [gx.at_ft, gy.at_ft];
                            let model_pt = layout::to_model(ts, building_pt);
                            let back = sheet_reader::building_from_model(alignment, model_pt);
                            let err = ((back[0] - building_pt[0]).powi(2) + (back[1] - building_pt[1]).powi(2)).sqrt();
                            assert!(err < 1.0, "seed {seed}: {} rotated alignment error {err:.4} ft at grid {}-{}", ts.number, gx.label, gy.label,);
                        }
                    }
                }
            }
        }
    }
    assert!(tested_rotated, "no rotated sheets found in seeds 1..=30");
}

#[test]
fn offset_sheets_are_aligned() {
    // Find seeds with world offsets and verify alignment.
    let mut tested_offset = false;
    for seed in 1..=30 {
        let (truth, map) = fixture(seed);
        for ts in &truth.sheets {
            if ts.paper_from_model.origin_in != [0.0, 0.0] {
                tested_offset = true;
                if let Some(f) = map.sheets.iter().find(|s| s.number == ts.number)
                    && let Some(alignment) = &f.alignment
                {
                    let gx = &truth.building.grid.x[0];
                    let gy = &truth.building.grid.y[0];
                    let building_pt = [gx.at_ft, gy.at_ft];
                    let model_pt = layout::to_model(ts, building_pt);
                    let back = sheet_reader::building_from_model(alignment, model_pt);
                    let err = ((back[0] - building_pt[0]).powi(2) + (back[1] - building_pt[1]).powi(2)).sqrt();
                    assert!(err < 1.0, "seed {seed}: {} offset alignment error {err:.4} ft", ts.number,);
                }
            }
        }
    }
    assert!(tested_offset, "no offset sheets found in seeds 1..=30");
}

#[test]
fn enlarged_plan_scales_are_detected() {
    // Enlarged plans (P-401, P-402, P-403) have different scales from the rest.
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        for ts in &truth.sheets {
            if ts.window_ft.is_some()
                && let Some(f) = map.sheets.iter().find(|s| s.number == ts.number)
            {
                assert!(f.scale_text.is_some(), "seed {seed}: enlarged plan {} has no scale detected", ts.number,);
                assert!((f.scale - ts.scale).abs() < 0.01, "seed {seed}: enlarged plan {} scale {:.4} vs truth {:.4}", ts.number, f.scale, ts.scale,);
            }
        }
    }
}

#[test]
fn cover_and_schedule_sheets_have_no_level() {
    for seed in 1..=5 {
        let (truth, map) = fixture(seed);
        for ts in &truth.sheets {
            if ts.level.is_none()
                && let Some(f) = map.sheets.iter().find(|s| s.number == ts.number)
            {
                assert_eq!(f.level, None, "seed {seed}: non-plan sheet {} should have no level but got {:?}", ts.number, f.level,);
            }
        }
    }
}

#[test]
fn score_sheet_map_reports_correct_fields() {
    let (truth, map) = fixture(3);
    let scores = sheet_reader::score_sheet_map(&map, &truth.sheets, &truth.building.grid.x, &truth.building.grid.y);
    assert_eq!(scores.len(), truth.sheets.len());
    // Count how many plan sheets have alignment errors < 1 foot.
    let plan_sheets = truth.sheets.iter().filter(|s| s.level.is_some()).count();
    let aligned = scores.iter().zip(&truth.sheets).filter(|(sc, ts)| ts.level.is_some() && sc.grid_error_ft.is_some_and(|e| e < 1.0)).count();
    assert!(aligned >= plan_sheets / 2, "only {aligned}/{plan_sheets} plan sheets aligned to < 1 ft error",);
}
