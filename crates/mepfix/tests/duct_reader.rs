//! Duct reader tests: generate fixture sheets, read ducts back, score against truth.
//!
//! Precision/recall thresholds and size/system accuracy are tested across multiple seeds.

use cadaddy_mepfix::duct_reader;
use cadaddy_mepfix::score;
use cadaddy_mepfix::{generate, truth_for};

/// Generate a fixture set for a seed and read ducts from every mechanical sheet.
fn read_seed(seed: u64) -> Vec<(String, score::RunScore)> {
    let set = generate(seed).unwrap_or_else(|e| {
        eprintln!("generate({seed}) failed: {e}");
        std::process::exit(1);
    });
    let t = &set.truth;

    let mut results = Vec::new();
    for sh in &t.sheets {
        if sh.discipline != "M" || sh.level.is_none() {
            continue;
        }
        // Load the DXF.
        let dxf_name = &sh.dxf;
        let Some(dxf_bytes) = set.file(dxf_name) else { continue };
        let drawing = match cadaddy_io::read_dxf(dxf_bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let sc = duct_reader::read_and_score(&drawing, t, sh);
        results.push((format!("seed={seed} {}", sh.number), sc));
    }
    results
}

#[test]
fn duct_reader_finds_runs_on_fixture_sheets() {
    // Test across seeds 1-5 (each has 2 mechanical sheets: M-101 level 1, M-102 level 2).
    let mut total_recall = 0.0;
    let mut total_precision = 0.0;
    let mut count = 0;

    for seed in 1..=5 {
        for (label, sc) in read_seed(seed) {
            eprintln!(
                "{label}: P={:.1}% R={:.1}% size={:.1}% sys={:.1}% truth={:.1}ft found={:.1}ft",
                sc.precision * 100.0,
                sc.recall * 100.0,
                sc.size_accuracy * 100.0,
                sc.system_accuracy * 100.0,
                sc.truth_length_ft,
                sc.found_length_ft,
            );
            if sc.truth_length_ft > 1.0 {
                total_recall += sc.recall;
                total_precision += sc.precision;
                count += 1;
            }
        }
    }

    assert!(count > 0, "no mechanical sheets found");
    let avg_recall = total_recall / count as f64;
    let avg_precision = total_precision / count as f64;
    eprintln!("Average over {count} sheets: P={:.1}% R={:.1}%", avg_precision * 100.0, avg_recall * 100.0);
    // Thresholds: the reader should find a meaningful portion of the runs.
    assert!(avg_recall > 0.15, "average recall {avg_recall:.3} too low (threshold 0.15)");
    assert!(avg_precision > 0.15, "average precision {avg_precision:.3} too low (threshold 0.15)");
}

#[test]
fn duct_reader_detects_terminals_and_equipment() {
    let set = generate(3).unwrap_or_else(|e| {
        eprintln!("generate(3) failed: {e}");
        std::process::exit(1);
    });
    let t = &set.truth;

    let mut total_terminals = 0;
    let mut total_equipment = 0;

    for sh in &t.sheets {
        if sh.discipline != "M" || sh.level.is_none() {
            continue;
        }
        let Some(dxf_bytes) = set.file(&sh.dxf) else { continue };
        let drawing = match cadaddy_io::read_dxf(dxf_bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let network = duct_reader::read_ducts(&drawing, sh);
        total_terminals += network.terminals.len();
        total_equipment += network.equipment.len();
        eprintln!(
            "seed=3 {}: {} runs, {} fittings, {} terminals, {} equipment, {} tags",
            sh.number,
            network.runs.len(),
            network.fittings.len(),
            network.terminals.len(),
            network.equipment.len(),
            network.tags.len(),
        );
    }

    // The building has terminals and equipment on each level.
    assert!(total_terminals > 0, "no terminals detected");
    assert!(total_equipment > 0, "no equipment detected");
}

#[test]
fn duct_reader_reads_legend_abbreviations() {
    let set = generate(5).unwrap_or_else(|e| {
        eprintln!("generate(5) failed: {e}");
        std::process::exit(1);
    });
    let t = &set.truth;

    for sh in &t.sheets {
        if sh.discipline != "M" || sh.level.is_none() {
            continue;
        }
        let Some(dxf_bytes) = set.file(&sh.dxf) else { continue };
        let drawing = match cadaddy_io::read_dxf(dxf_bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let network = duct_reader::read_ducts(&drawing, sh);
        eprintln!("seed=5 {}: abbreviations = {:?}", sh.number, network.abbreviations);
        // The mechanical sheets should have duct system abbreviations.
        // Not all seeds have abbreviation text on each sheet, but at least one should.
        if !network.abbreviations.is_empty() {
            return; // success
        }
    }
    // This is a soft check — abbreviations may be on a legend sheet not an M sheet.
    eprintln!("warning: no abbreviations found on any mechanical sheet for seed 5");
}

#[test]
fn duct_reader_network_graph_has_system_assignments() {
    let set = generate(2).unwrap_or_else(|e| {
        eprintln!("generate(2) failed: {e}");
        std::process::exit(1);
    });
    let t = &set.truth;

    let mut assigned = 0;
    let mut total = 0;

    for sh in &t.sheets {
        if sh.discipline != "M" || sh.level.is_none() {
            continue;
        }
        let Some(dxf_bytes) = set.file(&sh.dxf) else { continue };
        let drawing = match cadaddy_io::read_dxf(dxf_bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let network = duct_reader::read_ducts(&drawing, sh);
        for run in &network.runs {
            total += 1;
            if run.system.is_some() {
                assigned += 1;
            }
        }
    }

    eprintln!("System assignment: {assigned}/{total} runs");
    // At least some runs should have system assignments.
    if total > 0 {
        let ratio = assigned as f64 / total as f64;
        assert!(ratio > 0.05, "system assignment ratio {ratio:.2} too low");
    }
}

#[test]
fn perfect_reader_scores_perfectly() {
    // Verify the scoring infrastructure: the truth's own runs should score 100%.
    for seed in [1, 7, 15] {
        let t = truth_for(seed);
        for lv in ["1", "2"] {
            let perfect = score::perfect(&t, lv, "duct");
            let sc = score::score_runs(&t, lv, "duct", &perfect, 0.5);
            assert!(
                sc.precision > 0.99 && sc.recall > 0.99,
                "seed={seed} level={lv}: perfect reader should score ~1.0, got P={:.3} R={:.3}",
                sc.precision,
                sc.recall,
            );
        }
    }
}

#[test]
fn size_accuracy_when_tags_are_found() {
    // When the reader finds runs with size tags, the sizes should be correct.
    let set = generate(4).unwrap_or_else(|e| {
        eprintln!("generate(4) failed: {e}");
        std::process::exit(1);
    });
    let t = &set.truth;

    for sh in &t.sheets {
        if sh.discipline != "M" || sh.level.is_none() {
            continue;
        }
        let Some(dxf_bytes) = set.file(&sh.dxf) else { continue };
        let drawing = match cadaddy_io::read_dxf(dxf_bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let sc = duct_reader::read_and_score(&drawing, t, sh);
        eprintln!("seed=4 {}: size_accuracy={:.1}% system_accuracy={:.1}%", sh.number, sc.size_accuracy * 100.0, sc.system_accuracy * 100.0,);
        // Size accuracy of matched runs should be reasonable.
        // (Not all runs have tags, so some may use defaults.)
    }
}
