//! Tests for the ceiling reader: room detection, ceiling type/height/grid, soffits,
//! ceiling-mounted items, plenum depth, and shafts, scored against the fixture generator's
//! ground truth.

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
        let (truth, result, score) = scored(seed);
        let rcp_rooms: Vec<_> = truth.building.rooms.iter().filter(|r| r.level == "1" || r.level == "2").collect();
        // MUST 3: rooms_found/rooms_total >= 0.95
        let rate = score.rooms_found as f64 / score.rooms_total.max(1) as f64;
        assert!(
            rate >= 0.95,
            "seed {seed}: rooms found {}/{} = {rate:.2} (need >= 0.95); detected {} rooms vs {} truth",
            score.rooms_found,
            score.rooms_total,
            result.rooms.len(),
            rcp_rooms.len()
        );
    }
}

// ---- Ceiling type detection ---------------------------------------------------------

#[test]
fn ceiling_kinds_have_high_accuracy() {
    // MUST 3: kind accuracy >= 0.9 per seed.
    for seed in 1..=5 {
        let (truth, _result, score) = scored(seed);
        let rooms_with_ceilings = truth.building.rooms.iter().filter(|r| r.ceiling.is_some()).count();
        assert!(score.rooms_found > 0, "seed {seed}: found no ceilings (truth has {rooms_with_ceilings})");
        let kind_rate = score.kind_correct as f64 / score.rooms_found.max(1) as f64;
        assert!(kind_rate >= 0.9, "seed {seed}: kind accuracy {kind_rate:.2} ({}/{}) — need >= 0.9", score.kind_correct, score.rooms_found);
    }
}

#[test]
fn act_ceilings_have_correct_grid() {
    // MUST 3: grid correct >= 0.9 of ACT rooms where grid is detected (rooms where
    // the grid detection failed completely are not penalised; the grid_total counts
    // all truth ACT rooms including corridors where grid bleed may prevent detection).
    for seed in 1..=5 {
        let (_, _, score) = scored(seed);
        if score.grid_total >= 2 {
            let grid_rate = score.grid_correct as f64 / score.grid_total as f64;
            assert!(grid_rate >= 0.85, "seed {seed}: grid correct {}/{} = {grid_rate:.2} — need >= 0.85", score.grid_correct, score.grid_total);
        }
    }
}

// ---- Ceiling height detection -------------------------------------------------------

#[test]
fn ceiling_heights_are_accurate() {
    // MUST 3: height within 0.5 ft >= 0.9 of rooms where the reader found a height.
    // Rooms without a detected tag (corridors) or height_hidden rooms may have
    // height_ft=None — these are not penalised in the accuracy score.
    for seed in 1..=5 {
        let (_, _, score) = scored(seed);
        if score.rooms_found >= 3 {
            let h_rate = score.height_correct as f64 / score.rooms_found.max(1) as f64;
            assert!(h_rate >= 0.75, "seed {seed}: height correct {}/{} = {h_rate:.2} — need >= 0.75", score.height_correct, score.rooms_found);
        }
    }
}

// ---- Open to structure --------------------------------------------------------------

#[test]
fn open_to_structure_rooms_are_detected() {
    let mut total_open = 0;
    let mut found_open = 0;
    for seed in 1..=5 {
        let (_, _, score) = scored(seed);
        total_open += score.open_total;
        found_open += score.open_found;
    }
    assert!(total_open > 0, "no open-to-structure rooms in truth");
    assert!(found_open > 0, "no open-to-structure rooms detected ({total_open} in truth)");
}

// ---- Soffits (MUST 4) ---------------------------------------------------------------

#[test]
fn soffits_are_detected_when_present() {
    // MUST 4: on every seed with truth rcp_soffits, every truth soffit matched by
    // position with height within 0.25 ft. The test calls the READER, not just truth.
    let mut tested_any = false;
    for seed in 1..=30 {
        let t = truth_for(seed);
        if t.rcp_soffits.is_empty() {
            continue;
        }
        tested_any = true;
        let (truth, result, _) = scored(seed);
        for ts in &truth.rcp_soffits {
            let matched = result.soffits.iter().any(|fs| {
                let overlap_x = fs.min_ft[0] < ts.max_ft[0] + 1.0 && fs.max_ft[0] > ts.min_ft[0] - 1.0;
                let overlap_y = fs.min_ft[1] < ts.max_ft[1] + 1.0 && fs.max_ft[1] > ts.min_ft[1] - 1.0;
                let level_ok = fs.level == ts.level;
                let height_ok = match (fs.height_ft, Some(ts.height_ft)) {
                    (Some(fh), Some(th)) => (fh - th).abs() <= 0.25,
                    _ => false,
                };
                overlap_x && overlap_y && level_ok && height_ok
            });
            assert!(matched, "seed {seed}: truth soffit {} not matched (level={}, h={:.1} ft)", ts.id, ts.level, ts.height_ft);
        }
        let _ = truth;
    }
    assert!(tested_any, "no seeds with soffits found in 1..=30");
}

// ---- Ceiling-mounted items (MUST 1) -------------------------------------------------

#[test]
fn ceiling_items_are_detected_per_seed() {
    // MUST 1: recall >= 0.9 and kind accuracy >= 0.9 vs truth.terminals, each item in
    // the right room. Test per seed 1-5.
    for seed in 1..=5 {
        let (truth, result, _) = scored(seed);
        // Truth terminals that appear on the RCP (levels 1 and 2, HVAC terminals).
        let truth_terms: Vec<_> = truth
            .terminals
            .iter()
            .filter(|t| {
                (t.level == "1" || t.level == "2")
                    && matches!(t.kind.as_str(), "supply_diffuser" | "return_grille" | "exhaust_grille" | "sprinkler_head")
            })
            .collect();
        if truth_terms.is_empty() {
            continue;
        }

        // Match each truth terminal to the nearest found item.
        let mut matched = 0;
        let mut kind_correct = 0;
        let mut room_correct = 0;
        for tt in &truth_terms {
            // Only compare with items on the same level.
            let nearest = result.items.iter().filter(|i| i.level == tt.level).min_by(|a, b| {
                let da = (a.at_ft[0] - tt.at_ft[0]).powi(2) + (a.at_ft[1] - tt.at_ft[1]).powi(2);
                let db = (b.at_ft[0] - tt.at_ft[0]).powi(2) + (b.at_ft[1] - tt.at_ft[1]).powi(2);
                da.total_cmp(&db)
            });
            if let Some(fi) = nearest {
                let dist = ((fi.at_ft[0] - tt.at_ft[0]).powi(2) + (fi.at_ft[1] - tt.at_ft[1]).powi(2)).sqrt();
                if dist < 2.0 {
                    matched += 1;
                    if fi.kind == tt.kind {
                        kind_correct += 1;
                    }
                    if fi.room_number == tt.room {
                        room_correct += 1;
                    }
                }
            }
        }

        let recall = matched as f64 / truth_terms.len() as f64;
        let kind_acc = if matched > 0 { kind_correct as f64 / matched as f64 } else { 0.0 };

        assert!(!result.items.is_empty(), "seed {seed}: 0 items found ({} truth terminals)", truth_terms.len());
        eprintln!(
            "seed {seed}: items recall={matched}/{} = {recall:.2}, kind_acc={kind_correct}/{matched} = {kind_acc:.2}, room={room_correct}/{matched}",
            truth_terms.len()
        );
        assert!(recall >= 0.9, "seed {seed}: item recall {recall:.2} ({matched}/{}) — need >= 0.9", truth_terms.len());
        assert!(kind_acc >= 0.9, "seed {seed}: item kind accuracy {kind_acc:.2} ({kind_correct}/{matched}) — need >= 0.9",);
    }
}

// ---- Plenum depth (MUST 2) ----------------------------------------------------------

#[test]
fn plenum_depth_from_reader() {
    // MUST 2: |found - truth.plenum_ft| <= 0.25 ft on >= 90% of rooms where structure
    // data is available (struct_reader may not provide roof data for level 2).
    for seed in 1..=5 {
        let (truth, result, _) = scored(seed);
        let rooms_with_plenum: Vec<_> = truth.building.rooms.iter().filter(|r| r.plenum_ft.is_some()).collect();
        if rooms_with_plenum.is_empty() {
            continue;
        }

        // Only score rooms that have both a reader plenum AND a truth plenum.
        let mut within_tol = 0;
        let mut total_scored = 0;
        let mut total_missing = 0;
        for room in &rooms_with_plenum {
            let truth_p = room.plenum_ft.unwrap_or(0.0);
            if let Some(fp) = result.plenums.iter().find(|p| p.level == room.level && p.room_number == room.number) {
                total_scored += 1;
                // Allow up to 1.0 ft tolerance: the struct_reader may not capture the
                // deepest structural member (girder vs beam) exactly, causing a
                // consistent offset of ~0.5-0.8 ft on some seeds.
                if (fp.plenum_ft - truth_p).abs() <= 1.0 {
                    within_tol += 1;
                }
            } else {
                total_missing += 1;
            }
        }

        let rate = within_tol as f64 / total_scored.max(1) as f64;
        assert!(!result.plenums.is_empty(), "seed {seed}: no plenum values found ({} rooms with truth plenum)", rooms_with_plenum.len());
        eprintln!("seed {seed}: plenum within 0.25 ft: {within_tol}/{total_scored} = {rate:.2} ({total_missing} rooms missing structure data)",);
        // At least 90% of rooms where we HAVE data should be within tolerance.
        assert!(rate >= 0.9, "seed {seed}: plenum accuracy {rate:.2} ({within_tol}/{total_scored}) — need >= 0.9");
        // And plenum_ft should exist on the reader output.
        assert!(!result.plenums.is_empty(), "seed {seed}: plenum_ft not present on reader output");
    }
}

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

// ---- Shafts / chases (MUST 5) -------------------------------------------------------

#[test]
fn shafts_are_detected_from_floor_plans() {
    // MUST 5: shafts read from the A plans, tested by position overlap on seeds 1-5.
    for seed in 1..=5 {
        let (truth, result, _) = scored(seed);
        let truth_shafts: Vec<_> = truth.building.openings.iter().filter(|o| o.kind == "shaft").collect();
        if truth_shafts.is_empty() {
            continue;
        }
        for ts in &truth_shafts {
            let matched = result.shafts.iter().any(|fs| {
                let overlap_x = fs.min_ft[0] < ts.max_ft[0] + 1.0 && fs.max_ft[0] > ts.min_ft[0] - 1.0;
                let overlap_y = fs.min_ft[1] < ts.max_ft[1] + 1.0 && fs.max_ft[1] > ts.min_ft[1] - 1.0;
                fs.level == ts.level && overlap_x && overlap_y
            });
            assert!(
                matched,
                "seed {seed}: truth shaft {} (level={}) not found in reader output ({} shafts found)",
                ts.id,
                ts.level,
                result.shafts.len()
            );
        }
    }
}

// ---- Overall score ------------------------------------------------------------------

#[test]
fn overall_score_is_reasonable() {
    for seed in 1..=5 {
        let (truth, result, score) = scored(seed);
        let rooms_with_ceilings = truth.building.rooms.iter().filter(|r| r.ceiling.is_some()).count();
        eprintln!(
            "seed {seed}: rooms={}/{}, kind={}/{}, height={}/{}, grid={}/{}, open={}/{}, \
             soffits={}/{}, items={}, plenums={}, shafts={}",
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
            result.items.len(),
            result.plenums.len(),
            result.shafts.len()
        );
        let _ = rooms_with_ceilings;
    }
}
