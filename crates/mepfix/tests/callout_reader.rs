//! Tests for callout reading and resolution against the Q254 fixture generator's ground truth.
//!
//! The fixture set generates DXF files with callout symbols (section marks, elevation tags,
//! detail bubbles, enlarged-plan boundaries, key plans) on the CALLOUT layer. The callout
//! reader must find them, read their detail/sheet numbers, and resolve them against the set.
//! Planted broken callouts must be reported as unresolved.

use cadaddy_mepfix::callout_reader;
use cadaddy_mepfix::generate;

/// Generate a fixture set, read back the DXFs, and return (truth, resolved, broken).
fn fixture(seed: u64) -> (cadaddy_mepfix::truth::Truth, Vec<callout_reader::ResolvedCallout>, Vec<callout_reader::ResolvedCallout>) {
    let set = generate(seed).unwrap_or_else(|e| {
        panic!("generate({seed}): {e}");
    });
    let dxf_files: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let sheet_numbers: Vec<String> = set.truth.sheets.iter().map(|s| s.number.clone()).collect();
    let (resolved, broken) = callout_reader::resolve_callouts(&dxf_files, &sheet_numbers);
    (set.truth, resolved, broken)
}

#[test]
fn callout_truth_is_nonempty() {
    for seed in 1..=3 {
        let set = generate(seed).unwrap_or_else(|e| panic!("generate({seed}): {e}"));
        assert!(!set.truth.callouts.is_empty(), "seed {seed}: no callout truth entries generated");
    }
}

#[test]
fn valid_callouts_resolve() {
    for seed in 1..=3 {
        let (truth, resolved, _broken) = fixture(seed);
        let valid_count = truth.callouts.iter().filter(|c| !c.broken).count();
        assert!(!resolved.is_empty(), "seed {seed}: no callouts resolved (truth has {valid_count} valid callouts)");
    }
}

#[test]
fn broken_callouts_are_reported() {
    for seed in 1..=3 {
        let (truth, _resolved, broken) = fixture(seed);
        let planted_broken = truth.callouts.iter().filter(|c| c.broken).count();
        assert!(planted_broken > 0, "seed {seed}: no planted broken callouts in truth");
        assert!(!broken.is_empty(), "seed {seed}: no broken callouts detected (truth has {planted_broken} planted)");
    }
}

#[test]
fn section_marks_are_found() {
    for seed in 1..=3 {
        let (truth, resolved, broken) = fixture(seed);
        let truth_sections = truth.callouts.iter().filter(|c| c.kind == "section_mark" && !c.broken).count();
        let found_sections = resolved.iter().chain(broken.iter()).filter(|r| r.callout.kind == "section_mark").count();
        if truth_sections > 0 {
            assert!(found_sections > 0, "seed {seed}: {truth_sections} section marks in truth, none found by reader");
        }
    }
}

#[test]
fn elevation_tags_are_found() {
    for seed in 1..=3 {
        let (truth, resolved, broken) = fixture(seed);
        let all_found: Vec<_> = resolved.iter().chain(broken.iter()).collect();
        let truth_elevations: Vec<_> = truth.callouts.iter().filter(|c| c.kind == "elevation_tag" && !c.broken).collect();
        // Elevation tags have a small arrow that may be hard to distinguish from
        // detail bubbles; count matches by target sheet and detail number.
        let matched = truth_elevations
            .iter()
            .filter(|et| {
                all_found.iter().any(|r| r.on_sheet == et.on_sheet && r.callout.target_sheet == et.target_sheet && r.callout.detail == et.detail)
            })
            .count();
        if !truth_elevations.is_empty() {
            assert!(
                matched > 0,
                "seed {seed}: {} elevation tags in truth, none matched by reader (found kinds on same sheets: {:?})",
                truth_elevations.len(),
                all_found
                    .iter()
                    .filter(|r| truth_elevations.iter().any(|et| r.on_sheet == et.on_sheet))
                    .map(|r| format!("{}:{}", r.callout.kind, r.callout.target_sheet))
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn detail_bubbles_are_found() {
    for seed in 1..=3 {
        let (truth, resolved, broken) = fixture(seed);
        let truth_details = truth.callouts.iter().filter(|c| c.kind == "detail_bubble" && !c.broken).count();
        let found_details = resolved.iter().chain(broken.iter()).filter(|r| r.callout.kind == "detail_bubble").count();
        if truth_details > 0 {
            assert!(found_details > 0, "seed {seed}: {truth_details} detail bubbles in truth, none found by reader");
        }
    }
}

#[test]
fn key_plans_are_found() {
    for seed in 1..=3 {
        let (truth, resolved, _broken) = fixture(seed);
        let truth_key_plans = truth.callouts.iter().filter(|c| c.kind == "key_plan").count();
        let found_key_plans = resolved.iter().filter(|r| r.callout.kind == "key_plan").count();
        if truth_key_plans > 0 {
            assert!(found_key_plans > 0, "seed {seed}: {truth_key_plans} key plans in truth, none found by reader");
        }
    }
}

#[test]
fn broken_target_sheets_do_not_exist_in_set() {
    for seed in 1..=3 {
        let (truth, _resolved, broken) = fixture(seed);
        let sheet_numbers: Vec<&str> = truth.sheets.iter().map(|s| s.number.as_str()).collect();
        for b in &broken {
            if !b.callout.target_sheet.is_empty() {
                assert!(
                    !sheet_numbers.contains(&b.callout.target_sheet.as_str()),
                    "seed {seed}: broken callout target '{}' exists in sheet set",
                    b.callout.target_sheet
                );
            }
        }
    }
}

#[test]
fn enlarged_plan_callouts_are_found() {
    for seed in 1..=3 {
        let (truth, resolved, broken) = fixture(seed);
        let truth_enlarged = truth.callouts.iter().filter(|c| c.kind == "enlarged_plan" && !c.broken).count();
        let found_enlarged = resolved.iter().chain(broken.iter()).filter(|r| r.callout.kind == "enlarged_plan").count();
        if truth_enlarged > 0 {
            assert!(found_enlarged > 0, "seed {seed}: {truth_enlarged} enlarged plan callouts in truth, none found by reader");
        }
    }
}

#[test]
fn resolved_callouts_have_valid_sheet_numbers() {
    for seed in 1..=3 {
        let (_truth, resolved, _broken) = fixture(seed);
        for r in &resolved {
            if r.callout.kind != "key_plan" {
                assert!(!r.callout.target_sheet.is_empty(), "seed {seed}: resolved callout on {} has empty target sheet", r.on_sheet);
            }
        }
    }
}
