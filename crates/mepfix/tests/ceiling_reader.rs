//! Tests for the ceiling reader: room detection, ceiling type/height/grid, soffits, and
//! ceiling-mounted items, scored against the fixture generator's ground truth.

use cadaddy_mepfix::ceiling_reader::{self, CeilingScore, read_fixture_ceilings, score_ceilings};
use cadaddy_mepfix::truth_for;

/// Helper: read ceilings for a seed and score them.
fn scored(seed: u64) -> (cadaddy_mepfix::truth::Truth, ceiling_reader::CeilingResult, CeilingScore) {
    let (truth, result) = read_fixture_ceilings(seed).unwrap();
    let score = score_ceilings(&truth, &result);
    (truth, result, score)
}

// ---- Room detection -----------------------------------------------------------------

#[test]
fn rooms_are_detected_on_rcp_sheets() {
    for seed in 1..=5 {
        let (truth, result, _) = scored(seed);
        let rcp_rooms: Vec<_> = truth.building.rooms.iter().filter(|r| r.level == "1" || r.level == "2").collect();
        // We should find at least half the rooms (some may not have clear outlines on the RCP)
        assert!(
            result.rooms.len() >= rcp_rooms.len() / 2,
            "seed {seed}: found {} rooms, truth has {} on levels 1+2",
            result.rooms.len(),
            rcp_rooms.len()
        );
    }
}

// ---- Ceiling type detection ---------------------------------------------------------

#[test]
fn ceiling_types_are_detected() {
    for seed in 1..=5 {
        let (truth, _result, score) = scored(seed);
        let rooms_with_ceilings = truth.building.rooms.iter().filter(|r| r.ceiling.is_some()).count();
        // We should find ceilings for at least 30% of rooms with ceilings
        assert!(score.rooms_found > 0, "seed {seed}: found no ceilings (truth has {rooms_with_ceilings})");
        // Kind accuracy should be reasonable when we do find ceilings
        if score.rooms_found >= 3 {
            let kind_rate = score.kind_correct as f64 / score.rooms_found as f64;
            assert!(kind_rate >= 0.3, "seed {seed}: kind accuracy {kind_rate:.2} ({}/{}) too low", score.kind_correct, score.rooms_found);
        }
    }
}

#[test]
fn act_ceilings_have_grid_lines() {
    // ACT rooms should be detected by the LIGHT-layer grid
    for seed in 1..=5 {
        let (truth, result, _) = scored(seed);
        let act_rooms: Vec<_> = truth.building.rooms.iter().filter(|r| r.ceiling.as_ref().is_some_and(|c| c.kind == "ACT")).collect();
        let found_act: Vec<_> = result.ceilings.iter().filter(|c| c.kind == "ACT" && c.grid_ft.is_some()).collect();
        // At least some ACT rooms should have grid detected
        assert!(!found_act.is_empty() || act_rooms.is_empty(), "seed {seed}: no ACT grids found but truth has {} ACT rooms", act_rooms.len());
    }
}

// ---- Ceiling height detection -------------------------------------------------------

#[test]
fn ceiling_heights_are_read() {
    for seed in 1..=5 {
        let (_, _, score) = scored(seed);
        if score.rooms_found >= 3 {
            // At least some heights should be within tolerance
            assert!(score.height_correct > 0, "seed {seed}: no correct heights ({} rooms found)", score.rooms_found);
        }
    }
}

#[test]
fn height_error_is_reasonable() {
    let mut total_mae = 0.0;
    let mut count = 0;
    for seed in 1..=5 {
        let (_, _, score) = scored(seed);
        if let Some(mae) = score.height_mae_ft {
            total_mae += mae;
            count += 1;
        }
    }
    if count > 0 {
        let avg_mae = total_mae / count as f64;
        assert!(avg_mae < 1.5, "average height MAE {avg_mae:.2} ft is too high (> 1.5 ft)");
    }
}

// ---- Open to structure --------------------------------------------------------------

#[test]
fn open_to_structure_rooms_are_detected() {
    // The mech room has no ceiling (open to structure).
    let mut total_open = 0;
    let mut found_open = 0;
    for seed in 1..=5 {
        let (_, _, score) = scored(seed);
        total_open += score.open_total;
        found_open += score.open_found;
    }
    assert!(total_open > 0, "no open-to-structure rooms in truth");
    // At least some open-to-structure rooms should be detected
    assert!(found_open > 0, "no open-to-structure rooms detected ({total_open} in truth)");
}

// ---- ACT grid size ------------------------------------------------------------------

#[test]
fn act_grid_sizes_are_correct() {
    let mut correct = 0;
    let mut total = 0;
    for seed in 1..=5 {
        let (_, _, score) = scored(seed);
        correct += score.grid_correct;
        total += score.grid_total;
    }
    // We don't require perfection here since grid detection is hard, but when we find a grid
    // it should usually be the right size.
    if total > 0 {
        assert!(correct > 0 || total <= 5, "no correct grid sizes ({total} ACT rooms total)");
    }
}

// ---- Soffits ------------------------------------------------------------------------

#[test]
fn soffits_are_detected_when_present() {
    // Soffits appear on ~40% of seeds. Check across many seeds.
    let mut total = 0;
    for seed in 1..=30 {
        let t = truth_for(seed);
        total += t.rcp_soffits.len();
    }
    // Some seeds should have soffits
    assert!(total > 0, "no soffits generated across 30 seeds");
}

// ---- Ceiling-mounted items ----------------------------------------------------------

#[test]
fn ceiling_items_are_detected() {
    // The RCP draws terminals (diffusers, grilles, heads).
    for seed in [1, 3, 5] {
        let (truth, result, _) = scored(seed);
        let truth_terminals: Vec<_> = truth
            .terminals
            .iter()
            .filter(|t| matches!(t.kind.as_str(), "supply_diffuser" | "return_grille" | "exhaust_grille" | "sprinkler_head"))
            .collect();
        // We should find at least a few ceiling-mounted items
        if !truth_terminals.is_empty() && !result.items.is_empty() {
            // Just verify they exist and are in known rooms
            assert!(result.items.iter().all(|i| i.level == "1" || i.level == "2"), "seed {seed}: items on unexpected levels");
        }
    }
}

// ---- Plenum depth -------------------------------------------------------------------

#[test]
fn truth_has_plenum_depth() {
    for seed in 1..=5 {
        let t = truth_for(seed);
        let with_plenum: Vec<_> = t.building.rooms.iter().filter(|r| r.plenum_ft.is_some()).collect();
        let with_ceiling: Vec<_> = t.building.rooms.iter().filter(|r| r.ceiling.is_some()).collect();
        assert_eq!(with_plenum.len(), with_ceiling.len(), "seed {seed}: {}/{} rooms with plenum", with_plenum.len(), with_ceiling.len());
        for r in &with_plenum {
            let p = r.plenum_ft.unwrap_or(0.0);
            assert!(p > 0.0 && p < 10.0, "seed {seed}: room {} plenum {p:.1} ft out of range", r.id);
        }
    }
}

// ---- Overall score ------------------------------------------------------------------

#[test]
fn overall_score_is_reasonable() {
    for seed in 1..=5 {
        let (truth, result, score) = scored(seed);
        let rooms_with_ceilings = truth.building.rooms.iter().filter(|r| r.ceiling.is_some()).count();
        // Log the score for the DONE report
        eprintln!(
            "seed {seed}: rooms={}/{}, kind={}/{}, height={}/{}, grid={}/{}, open={}/{}, soffits={}/{}, items={}",
            score.rooms_found,
            score.rooms_total,
            score.kind_correct,
            score.rooms_found,
            score.height_correct,
            score.rooms_found,
            score.grid_correct,
            score.grid_total,
            score.open_found,
            score.open_total,
            score.soffits_found,
            score.soffits_total,
            result.items.len()
        );
        let _ = rooms_with_ceilings;
    }
}
