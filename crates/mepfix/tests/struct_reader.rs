//! Tests for the structural reader against the Q254 fixture generator's ground truth.
//!
//! Seeds 1–5, no `if let` skips: every expected value is asserted.

use cadaddy_mepfix::sheet_reader;
use cadaddy_mepfix::struct_reader::{self, parse_feet_inches};
use cadaddy_mepfix::{generate, truth_for};

/// Generate a fixture set, read back the DXFs, and return (truth, structural model).
fn fixture(seed: u64) -> (cadaddy_mepfix::truth::Truth, struct_reader::StructuralModel) {
    let set = generate(seed).unwrap_or_else(|e| panic!("generate({seed}): {e}"));
    let dxf_files: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();

    let sheet_map = sheet_reader::recognise_fixture_set(&dxf_files);

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

// ---- MUST 1: feet-inches parser unit tests ----------------------------------------------

#[test]
fn parse_feet_inches_basic() {
    assert_eq!(parse_feet_inches("12'-6\""), Some(12.5));
    assert_eq!(parse_feet_inches("13'-0\""), Some(13.0));
    assert_eq!(parse_feet_inches("0'-0\""), Some(0.0));
}

#[test]
fn parse_feet_inches_trailing_text() {
    // The elevation note has trailing text after the inch mark.
    assert_eq!(parse_feet_inches("12'-6\" U.N.O."), Some(12.5));
    assert_eq!(parse_feet_inches("13'-6\"."), Some(13.5));
}

#[test]
fn parse_feet_inches_fractional() {
    assert_eq!(parse_feet_inches("13'-4 1/2\""), Some(13.375));
    assert_eq!(parse_feet_inches("0'-6 1/2\""), Some(6.5 / 12.0));
}

#[test]
fn parse_feet_inches_negative() {
    assert_eq!(parse_feet_inches("-2'-0\""), Some(-2.0));
    assert_eq!(parse_feet_inches("-1'-6\""), Some(-1.5));
}

#[test]
fn parse_feet_inches_no_inches() {
    // Just feet, no inches.
    assert_eq!(parse_feet_inches("12'-\""), Some(12.0));
    assert_eq!(parse_feet_inches("12'"), Some(12.0));
}

// ---- Truth generator checks -------------------------------------------------------------

#[test]
fn truth_has_slabs_openings_and_joists() {
    for seed in 1..=5 {
        let t = truth_for(seed);
        assert_eq!(t.building.slabs.len(), 2, "seed {seed}: expected 2 slabs");
        assert!(t.building.slabs.iter().any(|s| s.kind == "composite"), "seed {seed}: no composite slab");
        assert!(t.building.slabs.iter().any(|s| s.kind == "roof"), "seed {seed}: no roof slab");
        assert!(!t.building.openings.is_empty(), "seed {seed}: no openings");
        assert!(!t.building.joists.is_empty(), "seed {seed}: no joists in truth");
    }
}

// ---- MUST 2: real assertions, no `if let` skips ----------------------------------------

#[test]
fn columns_are_detected() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        assert!(!model.columns.is_empty(), "seed {seed}: no columns detected");
        let mut found = 0;
        for tc in &truth.building.columns {
            if model.columns.iter().any(|c| c.grid == tc.grid) {
                found += 1;
            }
        }
        let recall = found as f64 / truth.building.columns.len() as f64;
        assert!(recall >= 0.8, "seed {seed}: column recall {recall:.2} ({found}/{})", truth.building.columns.len());
    }
}

#[test]
fn column_size_is_present_and_correct() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let truth_size = &truth.building.columns[0].size;
        let det_size = model.column_size.as_ref().unwrap_or_else(|| panic!("seed {seed}: column_size is None"));
        assert!(
            det_size.eq_ignore_ascii_case(truth_size),
            "seed {seed}: column size {det_size:?} vs truth {truth_size:?}"
        );
    }
}

#[test]
fn tos_present_on_every_framed_level() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        for level in truth.building.levels.iter().filter(|l| l.code != "1") {
            let truth_tos = truth.building.beams.iter().find(|b| b.level == level.code).map(|b| b.tos_ft);
            let truth_tos = truth_tos.unwrap_or_else(|| panic!("seed {seed}: no truth TOS for level {}", level.code));
            let det_tos = model.tos_ft.get(&level.code).unwrap_or_else(|| panic!("seed {seed}: TOS not read for level {}", level.code));
            assert!(
                (truth_tos - det_tos).abs() < 0.1,
                "seed {seed}: level {} TOS {det_tos:.3} vs truth {truth_tos:.3}",
                level.code
            );
        }
    }
}

#[test]
fn slabs_composite_and_roof_present() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        // Composite slab.
        let ts_comp = truth.building.slabs.iter().find(|s| s.kind == "composite").unwrap();
        let ds_comp = model.slabs.iter().find(|s| s.kind == "composite")
            .unwrap_or_else(|| panic!("seed {seed}: no composite slab detected"));
        assert!(
            (ds_comp.top_ft - ts_comp.top_ft).abs() < 0.1,
            "seed {seed}: composite slab top_ft {:.3} vs truth {:.3}", ds_comp.top_ft, ts_comp.top_ft
        );
        assert!(
            (ds_comp.concrete_in - ts_comp.concrete_in).abs() < 0.01,
            "seed {seed}: concrete {:.1} vs truth {:.1}", ds_comp.concrete_in, ts_comp.concrete_in
        );
        // Roof slab.
        let ts_roof = truth.building.slabs.iter().find(|s| s.kind == "roof").unwrap();
        let ds_roof = model.slabs.iter().find(|s| s.kind == "roof")
            .unwrap_or_else(|| panic!("seed {seed}: no roof slab detected"));
        assert!(
            (ds_roof.top_ft - ts_roof.top_ft).abs() < 0.1,
            "seed {seed}: roof slab top_ft {:.3} vs truth {:.3}", ds_roof.top_ft, ts_roof.top_ft
        );
    }
}

#[test]
fn beam_recall_and_size_accuracy() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let score = struct_reader::score_structure(&model, &truth);
        assert!(score.beams_total > 0, "seed {seed}: no truth beams");
        let recall = score.beams_found as f64 / score.beams_total as f64;
        assert!(recall >= 0.9, "seed {seed}: beam recall {recall:.2} ({}/{})", score.beams_found, score.beams_total);
        let size_acc = score.beam_size_correct as f64 / score.beams_found.max(1) as f64;
        assert!(size_acc >= 0.9, "seed {seed}: beam size accuracy {size_acc:.2} ({}/{})", score.beam_size_correct, score.beams_found);
    }
}

#[test]
fn opening_recall() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let score = struct_reader::score_structure(&model, &truth);
        assert!(score.openings_total > 0, "seed {seed}: no truth openings");
        let recall = score.openings_found as f64 / score.openings_total as f64;
        assert!(recall >= 0.9, "seed {seed}: opening recall {recall:.2} ({}/{})", score.openings_found, score.openings_total);
    }
}

// ---- MUST 3: beam depth and joist tests -------------------------------------------------

#[test]
fn beam_depth_matches_truth() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let score = struct_reader::score_structure(&model, &truth);
        assert!(score.beams_found > 0, "seed {seed}: no beams found");
        let depth_acc = score.beam_depth_correct as f64 / score.beams_found as f64;
        assert!(
            depth_acc >= 0.9,
            "seed {seed}: beam depth accuracy {depth_acc:.2} ({}/{})",
            score.beam_depth_correct, score.beams_found
        );
    }
}

#[test]
fn joist_recall_and_size() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let score = struct_reader::score_structure(&model, &truth);
        assert!(score.joists_total > 0, "seed {seed}: no truth joists");
        let recall = score.joists_found as f64 / score.joists_total as f64;
        assert!(recall >= 0.8, "seed {seed}: joist recall {recall:.2} ({}/{})", score.joists_found, score.joists_total);
        if score.joists_found > 0 {
            let size_acc = score.joist_size_correct as f64 / score.joists_found as f64;
            assert!(size_acc >= 0.8, "seed {seed}: joist size accuracy {size_acc:.2} ({}/{})", score.joist_size_correct, score.joists_found);
        }
    }
}

// ---- MUST 4: 3D structural model + IFC round-trip test ----------------------------------

#[test]
fn ifc_round_trip_entity_counts() {
    let seed = 1;
    let (truth, model) = fixture(seed);
    // Build 3D meshes from the truth.
    let d = struct_reader::build_3d(&model, &truth);
    // Write to IFC.
    let ifc_bytes = cadaddy_io::ifc::write_ifc(&d, "StructTest").unwrap_or_else(|e| panic!("write_ifc: {e}"));
    // Read back.
    let read_back = cadaddy_io::ifc::read_ifc(&ifc_bytes).unwrap_or_else(|e| panic!("read_ifc: {e}"));

    // Count entities by class.
    let mut columns = 0usize;
    let mut beams = 0usize;
    let mut slabs = 0usize;
    let mut members = 0usize;
    let mut class_counts = std::collections::BTreeMap::<String, usize>::new();
    for e in read_back.model.iter() {
        let info = cadaddy_io::ifc::element_info(e);
        *class_counts.entry(info.class.clone()).or_default() += 1;
        match info.class.as_str() {
            "IfcColumn" => columns += 1,
            "IfcBeam" => beams += 1,
            "IfcSlab" => slabs += 1,
            "IfcMember" => members += 1,
            _ => {}
        }
    }
    assert_eq!(columns, truth.building.columns.len(), "IFC column count mismatch; classes: {class_counts:?}");
    assert_eq!(beams, truth.building.beams.len(), "IFC beam count mismatch; classes: {class_counts:?}");
    assert_eq!(slabs, truth.building.slabs.len(), "IFC slab count mismatch; classes: {class_counts:?}");
    assert_eq!(members, truth.building.joists.len(), "IFC member (joist) count mismatch; classes: {class_counts:?}");

    // Check one beam's underside elevation: TOS - depth within 0.05 ft.
    // IFC stores coordinates in metres; the read-back drawing is in metres.
    let ft_to_m = 0.3048;
    let tb = &truth.building.beams[0];
    let expected_bottom_m = (tb.tos_ft - tb.depth_in / 12.0) * ft_to_m;
    let beam_mesh = read_back.model.iter().find(|e| {
        cadaddy_io::ifc::element_info(e).class == "IfcBeam"
    });
    let beam_mesh = beam_mesh.expect("no beam in read-back IFC");
    if let cadaddy_doc::EntityKind::Mesh(m) = &beam_mesh.kind {
        let z_min = m.vertices.iter().map(|v| v.z).fold(f64::MAX, f64::min);
        assert!(
            (z_min - expected_bottom_m).abs() < 0.05,
            "beam underside {z_min:.4} m vs expected {expected_bottom_m:.4} m",
        );
    } else {
        panic!("IFC beam is not a mesh");
    }
}

// ---- Overall score (kept, but with higher threshold) ------------------------------------

#[test]
fn overall_score_is_high() {
    for seed in 1..=5 {
        let (truth, model) = fixture(seed);
        let score = struct_reader::score_structure(&model, &truth);
        let recall = score.recall();
        assert!(
            recall >= 0.7,
            "seed {seed}: overall recall {recall:.2}\n  columns: {}/{}\n  beams: {}/{}\n  joists: {}/{}\n  tos: {}/{}\n  slabs: {}/{}\n  openings: {}/{}",
            score.columns_found, score.columns_total,
            score.beams_found, score.beams_total,
            score.joists_found, score.joists_total,
            score.tos_correct, score.tos_total,
            score.slabs_found, score.slabs_total,
            score.openings_found, score.openings_total,
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
    assert_eq!(score.joists_total, truth.building.joists.len(), "joists_total mismatch");
}
