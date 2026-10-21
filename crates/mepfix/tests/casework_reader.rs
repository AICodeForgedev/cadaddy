//! Tests for the casework reader: reads casework from architectural floor plan DXF sheets
//! and scores against the truth.

use cadaddy_mepfix::{casework_reader, generate, score, truth_for};

/// The office building design generates casework (break rooms, toilets, conference rooms).
#[test]
fn office_has_casework() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        assert!(!t.casework.is_empty(), "seed {seed}: office building should have casework");
    }
}

/// Casework items have valid fields: non-empty kind, level, room, and positive dimensions.
#[test]
fn casework_items_have_valid_fields() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        for cw in &t.casework {
            assert!(!cw.kind.is_empty(), "seed {seed}: casework {}: empty kind", cw.id);
            assert!(!cw.level.is_empty(), "seed {seed}: casework {}: empty level", cw.id);
            assert!(cw.min_ft[0] < cw.max_ft[0], "seed {seed}: casework {}: min_x >= max_x", cw.id);
            assert!(cw.min_ft[1] < cw.max_ft[1], "seed {seed}: casework {}: min_y >= max_y", cw.id);
            assert!(cw.height_in > 0.0, "seed {seed}: casework {}: height_in <= 0", cw.id);
            assert!(cw.depth_in > 0.0, "seed {seed}: casework {}: depth_in <= 0", cw.id);
        }
    }
}

/// The perfect casework scorer round-trips: passing truth back gets 100% precision and recall.
#[test]
fn perfect_casework_scorer_round_trips() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        let perfect = score::perfect_casework(&t);
        for level in ["1", "2"] {
            let s = score::score_casework(&t, level, &perfect, 5.0);
            let truth_n = t.casework.iter().filter(|c| c.level == level).count();
            assert_eq!(
                s.true_positives, truth_n,
                "seed {seed} level {level}: perfect scorer should find all {truth_n} items, got {}",
                s.true_positives
            );
            assert_eq!(s.false_positives, 0, "seed {seed} level {level}: perfect scorer should have 0 FP");
            assert_eq!(s.false_negatives, 0, "seed {seed} level {level}: perfect scorer should have 0 FN");
            if truth_n > 0 {
                assert!((s.precision - 1.0).abs() < 1e-9, "seed {seed} level {level}: precision should be 1.0");
                assert!((s.recall - 1.0).abs() < 1e-9, "seed {seed} level {level}: recall should be 1.0");
                assert!((s.kind_accuracy - 1.0).abs() < 1e-9, "seed {seed} level {level}: kind_accuracy should be 1.0");
                assert!(s.height_mae_in < 1e-9, "seed {seed} level {level}: height MAE should be 0");
                assert!(s.position_mae_ft < 1e-9, "seed {seed} level {level}: position MAE should be 0");
            }
        }
    }
}

/// The reader finds casework on the generated A-sheet DXFs and scores above a threshold.
#[test]
fn reader_finds_casework_on_floor_plans() {
    for seed in [1, 3, 5, 7, 9] {
        let set = generate(seed).unwrap_or_else(|e| panic!("seed {seed}: generate failed: {e}"));
        let t = &set.truth;

        for sh in &t.sheets {
            // Only floor plan sheets (A-10x).
            if !sh.number.starts_with("A-10") {
                continue;
            }
            let level = sh.level.clone().unwrap_or_default();
            let truth_n = t.casework.iter().filter(|c| c.level == level).count();
            if truth_n == 0 {
                continue;
            }

            let dxf_bytes = set.file(&sh.dxf).unwrap_or_else(|| panic!("seed {seed}: missing DXF {}", sh.dxf));
            let drawing = cadaddy_io::read_dxf(dxf_bytes).unwrap_or_else(|e| panic!("seed {seed}: read_dxf failed: {e}"));

            let s = casework_reader::read_and_score(&drawing, t, sh);
            assert!(
                s.recall >= 0.3,
                "seed {seed} sheet {}: recall {:.2} < 0.3 (TP={}, FN={})",
                sh.number,
                s.recall,
                s.true_positives,
                s.false_negatives
            );
        }
    }
}

/// The reader detects the right kinds of casework items.
#[test]
fn reader_classifies_kinds_correctly() {
    let set = generate(1).unwrap_or_else(|e| panic!("generate failed: {e}"));
    let t = &set.truth;

    let mut total_tp = 0usize;
    let mut total_kind_ok = 0usize;

    for sh in &t.sheets {
        if !sh.number.starts_with("A-10") {
            continue;
        }
        let dxf_bytes = match set.file(&sh.dxf) {
            Some(b) => b,
            None => continue,
        };
        let drawing = cadaddy_io::read_dxf(dxf_bytes).unwrap_or_else(|e| panic!("read_dxf failed: {e}"));
        let s = casework_reader::read_and_score(&drawing, t, sh);
        total_tp += s.true_positives;
        total_kind_ok += (s.kind_accuracy * s.true_positives as f64).round() as usize;
    }

    if total_tp > 0 {
        let accuracy = total_kind_ok as f64 / total_tp as f64;
        assert!(accuracy >= 0.5, "kind accuracy {accuracy:.2} < 0.5 ({total_kind_ok}/{total_tp})");
    }
}

/// Casework items are placed inside rooms (bounding-box containment check).
#[test]
fn casework_is_inside_rooms() {
    for seed in 1..=5 {
        let t = truth_for(seed);
        for cw in &t.casework {
            let room = t.building.rooms.iter().find(|r| r.number == cw.room && r.level == cw.level);
            if let Some(rm) = room {
                // Casework centre should be inside the room (with some tolerance).
                let cx = (cw.min_ft[0] + cw.max_ft[0]) / 2.0;
                let cy = (cw.min_ft[1] + cw.max_ft[1]) / 2.0;
                assert!(
                    cx >= rm.min_ft[0] - 1.0 && cx <= rm.max_ft[0] + 1.0 && cy >= rm.min_ft[1] - 1.0 && cy <= rm.max_ft[1] + 1.0,
                    "seed {seed}: casework {} ({}) at ({cx:.1}, {cy:.1}) is outside room {} ({:.1}..{:.1}, {:.1}..{:.1})",
                    cw.id,
                    cw.kind,
                    cw.room,
                    rm.min_ft[0],
                    rm.max_ft[0],
                    rm.min_ft[1],
                    rm.max_ft[1]
                );
            }
        }
    }
}

/// Break rooms always have base cabinets and a counter.
#[test]
fn break_rooms_have_cabinets_and_counter() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        let break_rooms: Vec<_> = t.building.rooms.iter().filter(|r| r.name == "BREAK ROOM").collect();
        for rm in &break_rooms {
            let room_cw: Vec<_> = t.casework.iter().filter(|c| c.room == rm.number && c.level == rm.level).collect();
            let has_base = room_cw.iter().any(|c| c.kind == "base_cabinet");
            let has_counter = room_cw.iter().any(|c| c.kind == "counter");
            assert!(has_base, "seed {seed}: break room {} on level {} has no base cabinet", rm.number, rm.level);
            assert!(has_counter, "seed {seed}: break room {} on level {} has no counter", rm.number, rm.level);
        }
    }
}

/// Toilet rooms have vanities.
#[test]
fn toilet_rooms_have_vanities() {
    for seed in 1..=10 {
        let t = truth_for(seed);
        let toilets: Vec<_> = t
            .building
            .rooms
            .iter()
            .filter(|r| r.name.contains("TOILET") || r.name.contains("REST") || r.name.contains("MEN") || r.name.contains("WOMEN"))
            .collect();
        for rm in &toilets {
            let has_vanity = t.casework.iter().any(|c| c.room == rm.number && c.level == rm.level && c.kind == "vanity");
            assert!(has_vanity, "seed {seed}: toilet room {} ({}) on level {} has no vanity", rm.number, rm.name, rm.level);
        }
    }
}
