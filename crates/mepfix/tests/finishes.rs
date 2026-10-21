//! Q338: Finish takeoff — regression tests.
//!
//! Verifies quantities by room and trade from the architecture truth, with waste and pattern
//! rules, per-trade takeoff ready for pricing.

use cadaddy_mepfix::finishes::{
    aggregate_by_description, aggregate_by_trade, compute_takeoff, room_area_sf, FinishTakeoff,
};
use cadaddy_mepfix::truth::Truth;
use cadaddy_mepfix::truth_for;
use std::collections::BTreeSet;

/// Seeds to test: the same ones Q307 uses, plus a few extras.
const SEEDS: [u64; 7] = [1, 2, 3, 4, 5, 6, 17];

fn takeoff_for(seed: u64) -> (Truth, FinishTakeoff) {
    let t = truth_for(seed);
    let tk = compute_takeoff(&t);
    (t, tk)
}

#[test]
fn every_room_has_floor_base_wall_ceiling() {
    for seed in SEEDS {
        let (t, tk) = takeoff_for(seed);
        let arch = t.arch.as_ref().expect("arch truth");
        for rf in &arch.finishes {
            let key = format!("{}-{}", rf.level, rf.number);
            let room_items = tk.by_room.get(&key);
            assert!(room_items.is_some(), "seed {seed}: room {key} missing from takeoff");
            let items = room_items.unwrap();
            // Floor
            assert!(
                items.iter().any(|i| i.trade == "flooring" && i.unit == "SF" && i.finish_code.as_deref() == Some(&rf.floor)),
                "seed {seed}: room {key} missing floor finish"
            );
            // Base
            assert!(
                items.iter().any(|i| i.trade == "flooring" && i.unit == "LF" && i.finish_code.as_deref() == Some(&rf.base)),
                "seed {seed}: room {key} missing base"
            );
            // PlanWall
            assert!(
                items.iter().any(|i| i.trade == "painting" && i.finish_code.as_deref() == Some(&rf.wall)),
                "seed {seed}: room {key} missing wall finish"
            );
            // Ceiling
            assert!(
                items.iter().any(|i| i.trade == "ceilings" && i.finish_code.as_deref() == Some(&rf.ceiling)),
                "seed {seed}: room {key} missing ceiling"
            );
        }
    }
}

#[test]
fn floor_areas_are_positive_and_reasonable() {
    for seed in SEEDS {
        let (t, tk) = takeoff_for(seed);
        for item in &tk.items {
            if item.trade == "flooring" && item.unit == "SF" {
                assert!(item.quantity > 0.0, "seed {seed}: {}: floor area = {}", item.room, item.quantity);
                // A room shouldn't be bigger than the whole building.
                assert!(item.quantity < 50000.0, "seed {seed}: {}: floor area too big = {}", item.room, item.quantity);
            }
        }
        // Total floor area should roughly equal sum of room areas.
        let total_from_items: f64 = tk.items.iter()
            .filter(|i| i.trade == "flooring" && i.unit == "SF")
            .map(|i| i.quantity)
            .sum();
        let total_from_rooms: f64 = t.building.rooms.iter().map(room_area_sf).sum();
        assert!(
            (total_from_items - total_from_rooms).abs() < 1.0,
            "seed {seed}: floor area mismatch: items {total_from_items} vs rooms {total_from_rooms}"
        );
    }
}

#[test]
fn base_perimeters_are_positive() {
    for seed in SEEDS {
        let (_, tk) = takeoff_for(seed);
        for item in &tk.items {
            if item.trade == "flooring" && item.unit == "LF" {
                assert!(item.quantity >= 0.0, "seed {seed}: {}: base = {}", item.room, item.quantity);
            }
        }
    }
}

#[test]
fn every_door_produces_three_items() {
    for seed in SEEDS {
        let (t, tk) = takeoff_for(seed);
        let arch = t.arch.as_ref().expect("arch truth");
        let door_items: Vec<_> = tk.items.iter().filter(|i| i.trade == "doors_frames_hardware").collect();
        // Each door → 1 door leaf + 1 frame + 1 hardware set = 3 items.
        assert_eq!(
            door_items.len(),
            arch.doors.len() * 3,
            "seed {seed}: expected {} door/frame/hw items, got {}",
            arch.doors.len() * 3,
            door_items.len()
        );
        let door_leaves: Vec<_> = door_items.iter().filter(|i| i.category == "door").collect();
        let frames: Vec<_> = door_items.iter().filter(|i| i.category == "frame").collect();
        let hw_sets: Vec<_> = door_items.iter().filter(|i| i.category == "hardware").collect();
        assert_eq!(door_leaves.len(), arch.doors.len(), "seed {seed}");
        assert_eq!(frames.len(), arch.doors.len(), "seed {seed}");
        assert_eq!(hw_sets.len(), arch.doors.len(), "seed {seed}");
    }
}

#[test]
fn window_glazing_items_match_types() {
    for seed in SEEDS {
        let (t, tk) = takeoff_for(seed);
        let arch = t.arch.as_ref().expect("arch truth");
        let glazing_items: Vec<_> = tk.items.iter().filter(|i| i.trade == "glazing").collect();
        // One window-count item + one glazing-area item per window type that has instances.
        let types_with_windows: BTreeSet<_> = arch.windows.iter().map(|w| w.mark.as_str()).collect();
        let expected = types_with_windows.len() * 2;
        assert_eq!(
            glazing_items.len(), expected,
            "seed {seed}: expected {expected} glazing items, got {}",
            glazing_items.len()
        );
        // Total window count matches.
        let total_windows: f64 = glazing_items.iter()
            .filter(|i| i.category == "window")
            .map(|i| i.quantity)
            .sum();
        assert_eq!(total_windows as usize, arch.windows.len(), "seed {seed}");
    }
}

#[test]
fn wet_rooms_get_partitions_and_accessories() {
    for seed in SEEDS {
        let (t, tk) = takeoff_for(seed);
        let arch = t.arch.as_ref().expect("arch truth");
        let wet: Vec<_> = arch.finishes.iter()
            .filter(|f| matches!(f.name.as_str(), "MEN" | "WOMEN" | "TOILET" | "RESTROOM"))
            .collect();
        for rf in &wet {
            let key = format!("{}-{}", rf.level, rf.number);
            let items = tk.by_room.get(&key).expect(&key);
            assert!(
                items.iter().any(|i| i.description.contains("TOILET PARTITION")),
                "seed {seed}: wet room {key} missing toilet partitions"
            );
            assert!(
                items.iter().any(|i| i.description.contains("GRAB BAR")),
                "seed {seed}: wet room {key} missing grab bars"
            );
            assert!(
                items.iter().any(|i| i.description.contains("SOAP DISPENSER")),
                "seed {seed}: wet room {key} missing soap dispensers"
            );
            assert!(
                items.iter().any(|i| i.description.contains("PAPER TOWEL")),
                "seed {seed}: wet room {key} missing paper towels"
            );
            assert!(
                items.iter().any(|i| i.description.contains("MIRROR")),
                "seed {seed}: wet room {key} missing mirror"
            );
        }
    }
}

#[test]
fn corridors_and_mech_get_fire_extinguisher() {
    for seed in SEEDS {
        let (t, tk) = takeoff_for(seed);
        let arch = t.arch.as_ref().expect("arch truth");
        for rf in arch.finishes.iter().filter(|f| f.name == "CORRIDOR" || f.name == "MECH") {
            let key = format!("{}-{}", rf.level, rf.number);
            let items = tk.by_room.get(&key).expect(&key);
            assert!(
                items.iter().any(|i| i.description.contains("FIRE EXTINGUISHER")),
                "seed {seed}: room {key} ({}) missing fire extinguisher",
                rf.name,
            );
        }
    }
}

#[test]
fn rooms_with_doors_get_signage() {
    for seed in SEEDS {
        let (t, tk) = takeoff_for(seed);
        let arch = t.arch.as_ref().expect("arch truth");
        for rf in &arch.finishes {
            let has_door = arch.doors.iter().any(|d| {
                d.level == rf.level
                    && (d.swings_into.as_deref() == Some(&rf.number) || d.from_room.as_deref() == Some(&rf.number))
            });
            if has_door {
                let key = format!("{}-{}", rf.level, rf.number);
                let items = tk.by_room.get(&key).expect(&key);
                assert!(
                    items.iter().any(|i| i.description.contains("ROOM IDENTIFICATION SIGN")),
                    "seed {seed}: room {key} has a door but no signage"
                );
            }
        }
    }
}

#[test]
fn waste_factors_applied_correctly() {
    for seed in SEEDS {
        let (_, tk) = takeoff_for(seed);
        for item in &tk.items {
            if item.waste_factor > 0.0 {
                // EA items may use .ceil() rounding, so allow 1.0 tolerance for those.
                let tol = if item.unit == "EA" { 1.0 } else { 0.02 };
                let expected = (item.quantity * (1.0 + item.waste_factor) * 100.0).round() / 100.0;
                assert!(
                    (item.with_waste - expected).abs() < tol,
                    "seed {seed}: {}: waste mismatch: {} vs expected {}",
                    item.description, item.with_waste, expected
                );
                assert!(item.with_waste >= item.quantity, "seed {seed}: {}: with_waste < quantity", item.description);
            } else {
                // with_waste should equal quantity when waste is 0.
                assert!(
                    (item.with_waste - item.quantity).abs() < 0.02,
                    "seed {seed}: {}: zero waste but with_waste {} != quantity {}",
                    item.description, item.with_waste, item.quantity
                );
            }
        }
    }
}

#[test]
fn all_six_trades_present() {
    for seed in SEEDS {
        let (_, tk) = takeoff_for(seed);
        let trade_names: BTreeSet<_> = tk.trades.iter().map(|t| t.trade.as_str()).collect();
        for expected in &["flooring", "painting", "ceilings", "doors_frames_hardware", "specialties", "glazing"] {
            assert!(trade_names.contains(expected), "seed {seed}: trade {expected} missing");
        }
    }
}

#[test]
fn aggregate_by_description_sums_correctly() {
    let (_, tk) = takeoff_for(1);
    let agg = aggregate_by_description(&tk.items);
    // Every description should have positive totals.
    for (desc, qty, with_waste, _unit) in &agg {
        assert!(*qty > 0.0, "aggregate {desc}: qty = {qty}");
        assert!(*with_waste >= *qty, "aggregate {desc}: with_waste {with_waste} < qty {qty}");
    }
}

#[test]
fn aggregate_by_trade_counts_match() {
    let (_, tk) = takeoff_for(1);
    let agg = aggregate_by_trade(&tk);
    let total: usize = agg.iter().map(|(_, c)| c).sum();
    assert_eq!(total, tk.items.len(), "trade aggregate total mismatch");
}

#[test]
fn act_ceilings_get_grid_and_wire() {
    for seed in SEEDS {
        let (t, tk) = takeoff_for(seed);
        let arch = t.arch.as_ref().expect("arch truth");
        for rf in &arch.finishes {
            let room = t.building.rooms.iter().find(|r| r.level == rf.level && r.number == rf.number);
            let is_act = room.is_some_and(|r| r.ceiling.as_ref().is_some_and(|c| c.kind == "ACT"));
            if is_act {
                let key = format!("{}-{}", rf.level, rf.number);
                let items = tk.by_room.get(&key).expect(&key);
                assert!(
                    items.iter().any(|i| i.description.contains("CEILING GRID")),
                    "seed {seed}: ACT room {key} missing grid"
                );
                assert!(
                    items.iter().any(|i| i.description.contains("WALL ANGLE")),
                    "seed {seed}: ACT room {key} missing wall angle"
                );
                assert!(
                    items.iter().any(|i| i.description.contains("SUSPENSION WIRE")),
                    "seed {seed}: ACT room {key} missing wire"
                );
            }
        }
    }
}

#[test]
fn tile_height_rooms_get_split_wall_items() {
    // Wet rooms (restrooms) should have wall tile code that triggers the split.
    for seed in SEEDS {
        let (t, tk) = takeoff_for(seed);
        let arch = t.arch.as_ref().expect("arch truth");
        for rf in arch.finishes.iter().filter(|f| matches!(f.name.as_str(), "MEN" | "WOMEN" | "TOILET" | "RESTROOM")) {
            let wall_code = &rf.wall;
            let code_desc = arch.finish_codes.iter().find(|c| c.code == *wall_code);
            if let Some(cd) = code_desc
                && cd.description.to_ascii_uppercase().contains("TILE TO")
            {
                let key = format!("{}-{}", rf.level, rf.number);
                let items = tk.by_room.get(&key).expect(&key);
                assert!(
                    items.iter().any(|i| i.description.contains("WALL TILE")),
                    "seed {seed}: wet room {key} should have wall tile item"
                );
            }
        }
    }
}

#[test]
fn no_negative_quantities() {
    for seed in SEEDS {
        let (_, tk) = takeoff_for(seed);
        for item in &tk.items {
            assert!(item.quantity >= 0.0, "seed {seed}: negative quantity in {}", item.description);
            assert!(item.with_waste >= 0.0, "seed {seed}: negative with_waste in {}", item.description);
        }
    }
}

#[test]
fn hostile_empty_arch() {
    let t = Truth::default();
    let tk = compute_takeoff(&t);
    assert!(tk.items.is_empty(), "no arch should produce no items");
}

#[test]
fn hostile_empty_rooms() {
    let mut t = truth_for(1);
    if let Some(arch) = t.arch.as_mut() {
        arch.finishes.clear();
    }
    let tk = compute_takeoff(&t);
    // Should still have door items (doors exist even without finish schedule rows).
    let door_items: Vec<_> = tk.items.iter().filter(|i| i.trade == "doors_frames_hardware").collect();
    assert!(!door_items.is_empty());
}
