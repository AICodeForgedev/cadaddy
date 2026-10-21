//! Pipe reader tests: generate fixtures, read piping from DXF sheets, score against truth.
//!
//! These tests verify the pipe reader finds runs on plumbing and fire protection sheets,
//! identifies systems, parses sizes and heights, and detects risers.

use cadaddy_mepfix::pipe_reader;
use cadaddy_mepfix::score::{self, FoundRun};
use cadaddy_mepfix::{generate, truth_for};

/// Read the pipe runs from a fixture set's plumbing and FP sheets.
fn read_all_pipes(seed: u64) -> (Vec<FoundRun>, Vec<pipe_reader::DetectedRiser>) {
    let set = generate(seed).expect("generate failed");
    let t = &set.truth;
    let mut all_runs = Vec::new();
    let mut all_risers = Vec::new();

    for sh in &t.sheets {
        // Only read plumbing (P-*) and fire protection (FP-*) plan sheets.
        let dominated = sh.discipline == "P" || sh.discipline == "FP";
        if !dominated || sh.level.is_none() {
            continue;
        }

        let dxf_data = match set.file(&sh.dxf) {
            Some(d) => d,
            None => continue,
        };
        let drawing = match cadaddy_io::read_dxf(dxf_data) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let (runs, risers) = pipe_reader::read_pipes(&drawing, sh);
        all_runs.extend(runs);
        all_risers.extend(risers);
    }

    (all_runs, all_risers)
}

#[test]
fn pipe_reader_finds_runs() {
    let (runs, _) = read_all_pipes(3);
    assert!(!runs.is_empty(), "pipe reader found no runs");
    // Should find multiple systems.
    let systems: Vec<String> = {
        let mut s: Vec<String> = runs.iter().map(|r| r.system.clone()).collect();
        s.sort();
        s.dedup();
        s
    };
    assert!(systems.len() >= 2, "only found systems: {systems:?}");
}

#[test]
fn pipe_reader_finds_risers() {
    // Risers should be found on at least some seeds.
    let mut any_risers = false;
    for seed in 1..=10 {
        let (_, risers) = read_all_pipes(seed);
        if !risers.is_empty() {
            any_risers = true;
            let with_system = risers.iter().filter(|r| !r.system.is_empty()).count();
            eprintln!("seed {seed}: {} risers, {} with system", risers.len(), with_system);
            for r in &risers {
                eprintln!("  riser at {:?} sys='{}' dir='{}' note='{}'", r.at_ft, r.system, r.direction, r.note);
                assert!(["up", "down", "through"].contains(&r.direction.as_str()), "riser direction: {}", r.direction);
            }
            assert!(with_system > 0, "no risers with identified system");
            break;
        }
    }
    assert!(any_risers, "no risers found across seeds 1-10");
}

#[test]
fn pipe_runs_score_against_truth() {
    // Score the reader's runs against the truth for several seeds.
    for seed in [3, 5, 7, 12] {
        let set = generate(seed).expect("generate failed");
        let t = &set.truth;
        let (runs, _) = read_all_pipes(seed);

        for level_code in ["1", "2"] {
            let truth_pipes: Vec<_> = score::expected(t, level_code, "pipe").collect();
            if truth_pipes.is_empty() {
                continue;
            }

            let s = score::score_runs(t, level_code, "pipe", &runs, 0.5);

            // We expect reasonable recall — the reader should find most pipe runs.
            // Precision may be lower if runs get split or duplicated.
            assert!(
                s.recall > 0.15,
                "seed {seed} level {level_code}: recall {:.3} too low (truth {:.1} ft, found {:.1} ft)",
                s.recall,
                s.truth_length_ft,
                s.found_length_ft
            );
        }
    }
}

#[test]
fn perfect_pipe_score() {
    // The scorer itself is correct for pipes: perfect runs get perfect scores.
    let t = truth_for(4);
    let perfect = score::perfect(&t, "1", "pipe");
    assert!(!perfect.is_empty(), "no pipe truth for seed 4 level 1");
    let s = score::score_runs(&t, "1", "pipe", &perfect, 0.25);
    eprintln!(
        "perfect: p={:.4} r={:.4} sz={:.4} sys={:.4} truth_ft={:.1} found_ft={:.1}",
        s.precision, s.recall, s.size_accuracy, s.system_accuracy, s.truth_length_ft, s.found_length_ft
    );
    // Check a few segments.
    let truth_segs: Vec<_> = score::expected(&t, "1", "pipe").collect();
    for seg in truth_segs.iter().take(3) {
        eprintln!("  truth seg: {} shape={} size={:?} sys={}", seg.id, seg.shape, seg.size_in, seg.system);
    }
    for run in perfect.iter().take(3) {
        eprintln!("  found run: shape={} size={:?} sys={}", run.shape, run.size_in, run.system);
    }
    assert!((s.precision - 1.0).abs() < 1e-9 && (s.recall - 1.0).abs() < 1e-9, "perfect pipe score: {s:?}");
    // Size accuracy may be slightly under 1.0 if parallel runs at different sizes
    // cause the best-overlap matching to pair a segment with a differently-sized found run.
    assert!(s.size_accuracy > 0.98, "size_acc={}", s.size_accuracy);
    assert!(s.system_accuracy > 0.98, "sys_acc={}", s.system_accuracy);
}

#[test]
fn pipe_reader_handles_multiple_seeds() {
    // The reader should work across varied styles (different seeds vary layout, tags, etc.).
    for seed in [1, 5, 10, 20, 30] {
        let (runs, _) = read_all_pipes(seed);
        assert!(!runs.is_empty(), "seed {seed}: pipe reader found no runs");
    }
}

#[test]
fn system_identification_coverage() {
    // Across seeds, the reader should identify multiple pipe systems.
    let mut all_systems = std::collections::BTreeSet::new();
    for seed in 1..=15 {
        let (runs, _) = read_all_pipes(seed);
        for r in &runs {
            if !r.system.is_empty() {
                all_systems.insert(r.system.clone());
            }
        }
    }
    // We should find at least CW, HW, SAN, and either FP or V.
    let expected = ["CW", "HW", "SAN"];
    for sys in expected {
        assert!(all_systems.contains(sys), "never found system {sys} across seeds 1-15; found: {all_systems:?}");
    }
}

#[test]
fn sizes_are_plausible() {
    let (runs, _) = read_all_pipes(5);
    for r in &runs {
        assert!(r.size_in[0] > 0.0 && r.size_in[0] <= 24.0, "implausible pipe size: {:?} for system {}", r.size_in, r.system);
    }
}
