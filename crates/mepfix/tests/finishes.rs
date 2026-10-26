//! Q338: Finish takeoff — regression tests.
//!
//! Verifies quantities by room and trade from the architecture truth, with waste and pattern
//! rules, per-trade takeoff ready for pricing.

use cadaddy_mepfix::finishes::{FinishTakeoff, aggregate_by_description, aggregate_by_trade, compute_takeoff};
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
            // Ceiling: an installed ceiling is a ceilings line; exposed structure is only painted
            // (Q5121: no ceilings-trade line for it).
            let exposed = arch.finish_codes.iter().any(|c| c.code == rf.ceiling && c.description.contains("EXPOSED"));
            if exposed {
                assert!(
                    items.iter().any(|i| i.trade == "painting" && i.finish_code.as_deref() == Some(&rf.ceiling)),
                    "seed {seed}: room {key} missing exposed ceiling paint"
                );
                assert!(!items.iter().any(|i| i.trade == "ceilings"), "seed {seed}: room {key}: exposed structure has a ceilings line");
            } else {
                assert!(
                    items.iter().any(|i| i.trade == "ceilings" && i.finish_code.as_deref() == Some(&rf.ceiling)),
                    "seed {seed}: room {key} missing ceiling"
                );
            }
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
        // Each room's floor line is its width times its depth, worked out here from the room's
        // corners (not through the takeoff's own area function).
        for r in &t.building.rooms {
            let expect = (r.max_ft[0] - r.min_ft[0]) * (r.max_ft[1] - r.min_ft[1]);
            let got: Vec<f64> = tk
                .items
                .iter()
                .filter(|i| i.trade == "flooring" && i.unit == "SF" && i.level == r.level && i.room == r.number)
                .map(|i| i.quantity)
                .collect();
            assert_eq!(got.len(), 1, "seed {seed}: room {}-{} floor lines {got:?}", r.level, r.number);
            assert!((got[0] - expect).abs() < 0.5, "seed {seed}: room {}-{}: floor {} vs {expect}", r.level, r.number, got[0]);
        }
    }
}

#[test]
fn base_is_the_perimeter_less_the_door_widths() {
    for seed in SEEDS {
        let (t, tk) = takeoff_for(seed);
        let arch = t.arch.as_ref().expect("arch truth");
        for r in &t.building.rooms {
            let perimeter = 2.0 * ((r.max_ft[0] - r.min_ft[0]) + (r.max_ft[1] - r.min_ft[1]));
            let doors: f64 = arch
                .doors
                .iter()
                .filter(|d| d.level == r.level && (d.swings_into.as_deref() == Some(&r.number) || d.from_room.as_deref() == Some(&r.number)))
                .map(|d| d.width_in / 12.0)
                .sum();
            let got: Vec<f64> = tk
                .items
                .iter()
                .filter(|i| i.trade == "flooring" && i.unit == "LF" && i.level == r.level && i.room == r.number)
                .map(|i| i.quantity)
                .collect();
            assert_eq!(got.len(), 1, "seed {seed}: room {}-{} base lines {got:?}", r.level, r.number);
            assert!(
                (got[0] - (perimeter - doors)).abs() < 0.5,
                "seed {seed}: room {}-{}: base {} vs {}",
                r.level,
                r.number,
                got[0],
                perimeter - doors
            );
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
        assert_eq!(glazing_items.len(), expected, "seed {seed}: expected {expected} glazing items, got {}", glazing_items.len());
        // Total window count matches.
        let total_windows: f64 = glazing_items.iter().filter(|i| i.category == "window").map(|i| i.quantity).sum();
        assert_eq!(total_windows as usize, arch.windows.len(), "seed {seed}");
    }
}

#[test]
fn wet_rooms_get_partitions_and_accessories() {
    for seed in SEEDS {
        let (t, tk) = takeoff_for(seed);
        let arch = t.arch.as_ref().expect("arch truth");
        let wet: Vec<_> = arch.finishes.iter().filter(|f| matches!(f.name.as_str(), "MEN" | "WOMEN" | "TOILET" | "RESTROOM")).collect();
        for rf in &wet {
            let key = format!("{}-{}", rf.level, rf.number);
            let items = tk.by_room.get(&key).expect(&key);
            assert!(items.iter().any(|i| i.description.contains("TOILET PARTITION")), "seed {seed}: wet room {key} missing toilet partitions");
            assert!(items.iter().any(|i| i.description.contains("GRAB BAR")), "seed {seed}: wet room {key} missing grab bars");
            assert!(items.iter().any(|i| i.description.contains("SOAP DISPENSER")), "seed {seed}: wet room {key} missing soap dispensers");
            assert!(items.iter().any(|i| i.description.contains("PAPER TOWEL")), "seed {seed}: wet room {key} missing paper towels");
            assert!(items.iter().any(|i| i.description.contains("MIRROR")), "seed {seed}: wet room {key} missing mirror");
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
            let has_door = arch
                .doors
                .iter()
                .any(|d| d.level == rf.level && (d.swings_into.as_deref() == Some(&rf.number) || d.from_room.as_deref() == Some(&rf.number)));
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
                    item.description,
                    item.with_waste,
                    expected
                );
                assert!(item.with_waste >= item.quantity, "seed {seed}: {}: with_waste < quantity", item.description);
            } else {
                // with_waste should equal quantity when waste is 0.
                assert!(
                    (item.with_waste - item.quantity).abs() < 0.02,
                    "seed {seed}: {}: zero waste but with_waste {} != quantity {}",
                    item.description,
                    item.with_waste,
                    item.quantity
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
                assert!(items.iter().any(|i| i.description.contains("CEILING GRID")), "seed {seed}: ACT room {key} missing grid");
                assert!(items.iter().any(|i| i.description.contains("WALL ANGLE")), "seed {seed}: ACT room {key} missing wall angle");
                assert!(items.iter().any(|i| i.description.contains("SUSPENSION WIRE")), "seed {seed}: ACT room {key} missing wire");
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
                assert!(items.iter().any(|i| i.description.contains("WALL TILE")), "seed {seed}: wet room {key} should have wall tile item");
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

// ---- Q5121: the takeoff from what was read ------------------------------------------------------

use cadaddy_mepfix::finishes::{
    RoomRule, TakeoffDoor, TakeoffInput, TakeoffItem, TakeoffRoom, TakeoffWall, compute, default_room_rules, read_input, rule_for,
};
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Seed 3's answer key and the takeoff input read off its DXF sheets (read once).
fn seed3() -> &'static (Truth, TakeoffInput) {
    static S: OnceLock<(Truth, TakeoffInput)> = OnceLock::new();
    S.get_or_init(|| {
        let set = cadaddy_mepfix::generate(3).expect("seed 3");
        let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
        (set.truth, read_input(&dxf))
    })
}

fn read3() -> FinishTakeoff {
    compute(&seed3().1, &default_room_rules())
}

fn lines<'a>(tk: &'a FinishTakeoff, level: &str, room: &str) -> Vec<&'a TakeoffItem> {
    tk.items.iter().filter(|i| i.level == level && i.room == room).collect()
}

/// The one line of a room whose description starts with `what` (and has `unit`).
fn one<'a>(tk: &'a FinishTakeoff, room: &str, what: &str, unit: &str) -> &'a TakeoffItem {
    let l: Vec<_> = lines(tk, "1", room).into_iter().filter(|i| i.description.starts_with(what) && i.unit == unit).collect();
    assert_eq!(l.len(), 1, "room 1-{room}: lines {what} {unit}: {l:#?}");
    l[0]
}

fn near(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() <= 0.5, "{what}: {got} vs hand count {want}");
}

#[test]
fn read_takeoff_equals_the_truth_takeoff_on_seed_3() {
    let (t, _) = seed3();
    let truth = compute_takeoff(t);
    let read = read3();
    type Key = (String, String, String, String, String);
    let index = |tk: &FinishTakeoff| {
        let mut m: BTreeMap<Key, Vec<f64>> = BTreeMap::new();
        for i in &tk.items {
            m.entry((i.trade.clone(), i.level.clone(), i.room.clone(), i.description.clone(), i.unit.clone())).or_default().push(i.quantity);
        }
        for v in m.values_mut() {
            v.sort_by(f64::total_cmp);
        }
        m
    };
    let (ti, ri) = (index(&truth), index(&read));
    let only_truth: Vec<_> = ti.keys().filter(|k| !ri.contains_key(*k)).collect();
    let only_read: Vec<_> = ri.keys().filter(|k| !ti.contains_key(*k)).collect();
    assert!(only_truth.is_empty() && only_read.is_empty(), "lines only in truth {only_truth:#?}\nlines only in the read {only_read:#?}");
    for (k, tq) in &ti {
        let rq = &ri[k];
        assert_eq!(tq.len(), rq.len(), "{k:?}: {tq:?} vs {rq:?}");
        for (a, b) in tq.iter().zip(rq) {
            assert!((a - b).abs() <= 0.5, "{k:?}: truth {a} read {b}");
        }
    }
    assert_eq!(truth.items.len(), read.items.len());
    assert!(read.items.len() > 400, "seed 3 should give a full takeoff, got {}", read.items.len());
    // The read found two unmarked door symbols outside the building: reported, not counted.
    assert!(read.missing.iter().any(|m| m.contains("unmarked door symbol")), "{:?}", read.missing);
}

#[test]
fn every_read_line_has_a_source_and_status_and_rule_lines_say_why() {
    let tk = read3();
    for i in &tk.items {
        assert!(!i.source.trim().is_empty(), "no source: {i:?}");
        assert!(matches!(i.status.as_str(), "drawing" | "schedule" | "rule" | "guess"), "status {:?}: {i:?}", i.status);
        if i.status == "rule" || i.status == "guess" {
            assert!(i.rule.as_deref().is_some_and(|r| !r.is_empty()), "no rule text: {i:?}");
        }
        if i.trade == "specialties" {
            assert_eq!(i.status, "rule", "{i:?}");
            assert!(i.rule.as_deref().is_some_and(|r| r.contains("standard")), "{i:?}");
        }
    }
    // Measured room lines: the plan sheet and the finish schedule's row.
    let floor = one(&tk, "105", "CPT-1", "SF");
    assert_eq!(floor.status, "drawing");
    assert_eq!(floor.source, "A-101 plan; A-603 ROOM FINISHES row 105");
    assert_eq!(floor.schedule_row.as_deref(), Some("105"));
    let floor2 = tk.items.iter().find(|i| i.level == "2" && i.room == "205" && i.unit == "SF" && i.trade == "flooring").expect("205 floor");
    assert!(floor2.source.starts_with("A-102 plan"), "{}", floor2.source);
    // Doors: the plan and the door schedule's row.
    let door = tk.items.iter().find(|i| i.category == "door" && i.description.starts_with("DOOR 105A")).expect("door 105A");
    assert_eq!(door.status, "schedule");
    assert_eq!(door.source, "A-101 plan door 105A; A-602 DOOR SCHEDULE row 105A");
    // Windows: the plans and the window schedule's row.
    let win = tk.items.iter().find(|i| i.category == "window").expect("window type");
    assert_eq!(win.status, "schedule");
    assert_eq!(win.source, "A-101, A-102 plans; A-602 WINDOW SCHEDULE row 1");
    // Rule lines: suspension wire, signs, shades, accessories.
    for what in ["SUSPENSION WIRE", "ROOM IDENTIFICATION SIGN", "WINDOW TREATMENT", "TOILET PARTITION", "CORNER GUARD", "FIRE EXTINGUISHER"] {
        let i = tk.items.iter().find(|i| i.description.starts_with(what)).unwrap_or_else(|| panic!("no {what} line"));
        assert_eq!(i.status, "rule", "{what}");
    }
}

/// A synthetic room of `w` x `d` feet at the origin, walls `h` high on all four sides.
fn boxed_room(name: &str, number: &str, w: f64, d: f64, h: f64) -> (TakeoffRoom, Vec<TakeoffWall>) {
    let room = TakeoffRoom {
        level: "1".into(),
        number: number.into(),
        name: name.into(),
        min_ft: [0.0, 0.0],
        max_ft: [w, d],
        floor: Some("F".into()),
        base: Some("B".into()),
        wall: Some("W".into()),
        ceiling: Some("C".into()),
        plan_sheet: "A-1".into(),
        ..Default::default()
    };
    let wall = |a: [f64; 2], b: [f64; 2]| TakeoffWall { level: "1".into(), a_ft: a, b_ft: b, height_ft: Some(h), sheet: "A-1".into() };
    let walls = vec![wall([0.0, 0.0], [w, 0.0]), wall([w, 0.0], [w, d]), wall([w, d], [0.0, d]), wall([0.0, d], [0.0, 0.0])];
    (room, walls)
}

fn code(code: &str, kind: &str, description: &str) -> cadaddy_mepfix::truth::FinishCode {
    cadaddy_mepfix::truth::FinishCode { code: code.into(), kind: kind.into(), description: description.into(), plates: None }
}

#[test]
fn room_types_come_from_the_standards_table_with_hotel_types() {
    let rules: Vec<RoomRule> = default_room_rules();
    for hotel in ["GUEST ROOM", "GUEST BATH", "CORRIDOR"] {
        assert!(rule_for(hotel, &rules).is_some(), "the standards table has no {hotel} row");
    }
    let (bath, mut walls) = boxed_room("GUEST BATH", "301B", 8.0, 6.0, 9.0);
    let (corr, w2) = boxed_room("CORRIDOR", "300", 40.0, 6.0, 9.0);
    walls.extend(w2.into_iter().map(|mut w| {
        w.level = "2".into();
        w
    }));
    let mut corr = corr;
    corr.level = "2".into();
    let input = TakeoffInput { rooms: vec![bath, corr], walls, ..Default::default() };
    let tk = compute(&input, &rules);
    let row = rule_for("GUEST BATH", &rules).expect("guest bath row");
    let got: Vec<(String, f64)> =
        tk.items.iter().filter(|i| i.room == "301B" && i.trade == "specialties").map(|i| (i.description.clone(), i.quantity)).collect();
    let want: Vec<(String, f64)> = row.items.iter().map(|i| (i.description.clone(), i.quantity)).collect();
    assert_eq!(got, want, "the guest bath gets exactly its table row");
    assert!(!got.iter().any(|(d, _)| d.contains("PARTITION")), "a guest bath has no toilet partitions");
    assert!(
        tk.items
            .iter()
            .filter(|i| i.room == "301B" && i.trade == "specialties")
            .all(|i| i.status == "rule" && i.rule.as_deref() == Some(row.rule.as_str()))
    );
    // A company can change its standards: its table, not the code, decides.
    let mut mine = rules.clone();
    if let Some(r) = mine.iter_mut().find(|r| r.names.iter().any(|n| n == "GUEST BATH")) {
        r.items.retain(|i| i.description.starts_with("ROBE HOOK"));
        r.items[0].quantity = 2.0;
    }
    let tk = compute(&input, &mine);
    let got: Vec<(String, f64)> =
        tk.items.iter().filter(|i| i.room == "301B" && i.trade == "specialties").map(|i| (i.description.clone(), i.quantity)).collect();
    assert_eq!(got, vec![("ROBE HOOK".to_string(), 2.0)]);
    // The men's room no longer gets the women's room's dispensers.
    let men = rule_for("MEN", &rules).expect("men");
    assert!(!men.items.iter().any(|i| i.description.contains("FEMININE") || i.description.contains("NAPKIN")));
    assert!(rule_for("WOMEN", &rules).expect("women").items.iter().any(|i| i.description.contains("NAPKIN")));
}

/// Seed 3, level 1, counted by hand from the plan (room boxes on the wall centrelines; walls to
/// structure 13'-0½" except the 8'-6" partitions W-4; doors 3'-0" x 7'-0"; tiles 2x2):
/// - 105 OFFICE, 30' x 26', ACT: floor 780 SF; base 112 − 3 = 109 LF; walls 30 × 13.041667 × 2 +
///   26 × 8.5 × 2 = 1224.5 − 21 (door 105A) = 1203.5 SF, all paint; grid: mains along the 30'
///   side, 13 rows × 30 + crosses 15 rows × 26 = 780 LF; wall angle 112 LF; wire 780 / 16 → 49.
/// - 114 RESTROOM, 15' x 26', gypsum ceiling, tile to 7'-0": floor 390; base 82 − 3 = 79; walls
///   82 × 13.041667 = 1069.417 − 21 = 1048.417 SF; tile 82 × 7 − 21 = 553 SF; paint above 495.417.
/// - 100 CORRIDOR, 210' x 8', ACT: floor 1680; 16 doors open off it: base 436 − 48 = 388 LF; walls
///   436 × 13.041667 = 5686.167 − 16 × 21 = 5350.167 SF; grid 4 × 210 + 105 × 8 = 1680 LF; wall angle
///   436; wire 1680 / 16 = 105.
#[test]
fn quantities_match_a_hand_count_of_three_rooms() {
    let (t, _) = seed3();
    for (tk, what) in [(read3(), "read"), (compute_takeoff(t), "truth")] {
        // 105 OFFICE
        near(one(&tk, "105", "CPT-1", "SF").quantity, 780.0, &format!("{what} 105 floor"));
        near(one(&tk, "105", "RB-1", "LF").quantity, 109.0, &format!("{what} 105 base"));
        near(one(&tk, "105", "PT-1", "SF").quantity, 1203.5, &format!("{what} 105 net wall"));
        near(one(&tk, "105", "CEILING GRID", "LF").quantity, 780.0, &format!("{what} 105 grid"));
        near(one(&tk, "105", "WALL ANGLE", "LF").quantity, 112.0, &format!("{what} 105 wall angle"));
        near(one(&tk, "105", "SUSPENSION WIRE", "EA").quantity, 49.0, &format!("{what} 105 wire"));
        near(one(&tk, "105", "ACT-2", "SF").quantity, 780.0, &format!("{what} 105 ceiling"));
        // 114 RESTROOM
        near(one(&tk, "114", "CT-1", "SF").quantity, 390.0, &format!("{what} 114 floor"));
        near(one(&tk, "114", "CTB-1", "LF").quantity, 79.0, &format!("{what} 114 base"));
        near(one(&tk, "114", "WALL TILE", "SF").quantity, 553.0, &format!("{what} 114 tile"));
        near(one(&tk, "114", "PAINT ABOVE TILE", "SF").quantity, 495.417, &format!("{what} 114 paint above"));
        let tile = one(&tk, "114", "WALL TILE", "SF").quantity;
        near(tile + one(&tk, "114", "PAINT ABOVE TILE", "SF").quantity, 1048.417, &format!("{what} 114 net wall"));
        // 100 CORRIDOR
        near(one(&tk, "100", "LVT-1", "SF").quantity, 1680.0, &format!("{what} 100 floor"));
        near(one(&tk, "100", "RB-1", "LF").quantity, 388.0, &format!("{what} 100 base"));
        near(one(&tk, "100", "PT-1", "SF").quantity, 5350.167, &format!("{what} 100 net wall"));
        near(one(&tk, "100", "CEILING GRID", "LF").quantity, 1680.0, &format!("{what} 100 grid"));
        near(one(&tk, "100", "WALL ANGLE", "LF").quantity, 436.0, &format!("{what} 100 wall angle"));
        near(one(&tk, "100", "SUSPENSION WIRE", "EA").quantity, 105.0, &format!("{what} 100 wire"));
    }
}

#[test]
fn per_trade_totals_on_seed_3() {
    let tk = read3();
    let mut totals: BTreeMap<(String, String), f64> = BTreeMap::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for i in &tk.items {
        *totals.entry((i.trade.clone(), i.unit.clone())).or_default() += i.quantity;
        *counts.entry(i.trade.clone()).or_default() += 1;
    }
    let got: Vec<(String, String, f64)> = totals.into_iter().map(|((t, u), q)| (t, u, (q * 100.0).round() / 100.0)).collect();
    let counts: Vec<(String, usize)> = counts.into_iter().collect();
    println!("{got:?}\n{counts:?}");
    assert_eq!(got, PER_TRADE_TOTALS.iter().map(|(t, u, q)| (t.to_string(), u.to_string(), *q)).collect::<Vec<_>>());
    assert_eq!(counts, PER_TRADE_LINES.iter().map(|(t, n)| (t.to_string(), *n)).collect::<Vec<_>>());
}

#[test]
fn paint_has_coats_sheen_and_a_primer_line() {
    let tk = read3();
    let paint = one(&tk, "105", "PT-1", "SF");
    assert_eq!(paint.coats, Some(2));
    assert_eq!(paint.sheen.as_deref(), Some("EGGSHELL"));
    assert!(paint.rule.as_deref().is_some_and(|r| r.contains("2 finish coats") && r.contains("standard")), "{paint:?}");
    let primer = one(&tk, "105", "PRIMER", "SF");
    assert_eq!(primer.status, "rule");
    assert_eq!(primer.coats, Some(1));
    near(primer.quantity, paint.quantity, "primer area = painted wall area");
    // The epoxy paint names its sheen.
    let mech = tk.items.iter().find(|i| i.room == "101" && i.level == "1" && i.description.starts_with("PT-2")).expect("101 epoxy");
    assert_eq!(mech.sheen.as_deref(), Some("SEMI-GLOSS"));
    // Coats given in the legend win over the default.
    let (mut room, walls) = boxed_room("OFFICE", "1", 10.0, 10.0, 10.0);
    room.floor = None;
    room.base = None;
    room.ceiling = None;
    let input = TakeoffInput { rooms: vec![room], walls, codes: vec![code("W", "wall", "PAINT, SATIN, 3 COATS")], ..Default::default() };
    let tk = compute(&input, &[]);
    let p = tk.items.iter().find(|i| i.description.starts_with("W:")).expect("paint");
    assert_eq!((p.coats, p.sheen.as_deref(), p.rule.as_deref()), (Some(3), Some("SATIN"), None));
    assert_eq!(p.quantity, 400.0);
    assert!(tk.items.iter().any(|i| i.description.starts_with("PRIMER")));
}

#[test]
fn frp_wallcovering_gets_its_own_line_with_15_percent_waste() {
    let tk = read3();
    // 110 BREAK ROOM, 30' x 26': walls 30 × 13.041667 × 2 + 26 × 8.5 + 26 × 13.041667 = 1342.583,
    // less door 110A (3' x 7') = 1321.583 SF.
    let frp = one(&tk, "110", "FRP-1", "SF");
    assert_eq!(frp.category, "wallcovering");
    assert_eq!(frp.waste_factor, 0.15);
    near(frp.quantity, 1321.583, "110 FRP");
    near(frp.with_waste, 1321.583 * 1.15, "110 FRP with waste");
    assert!(frp.coats.is_none());
    assert!(!lines(&tk, "1", "110").iter().any(|i| i.description.starts_with("PRIMER")), "no primer under FRP");
}

#[test]
fn quantities_roll_up_by_unit_type() {
    // Two hotel units of one type, each a guest room and its bath.
    let mut rooms = Vec::new();
    let mut walls = Vec::new();
    for (k, x) in [("301", 0.0), ("302", 40.0)] {
        for (name, n, w) in [("GUEST ROOM", k.to_string(), 14.0), ("GUEST BATH", format!("{k}B"), 8.0)] {
            let (mut r, ws) = boxed_room(name, &n, w, 10.0, 9.0);
            let dx = if name == "GUEST BATH" { x + 20.0 } else { x };
            r.min_ft[0] += dx;
            r.max_ft[0] += dx;
            r.unit_type = Some("K1".into());
            rooms.push(r);
            walls.extend(ws.into_iter().map(|mut w| {
                w.a_ft[0] += dx;
                w.b_ft[0] += dx;
                w
            }));
        }
    }
    let input = TakeoffInput { rooms, walls, ..Default::default() };
    let tk = compute(&input, &default_room_rules());
    let k1 = tk.by_unit_type.get("K1").expect("K1 roll-up");
    let floor = k1.iter().find(|l| l.trade == "flooring" && l.unit == "SF").expect("K1 floor");
    assert_eq!((floor.quantity, floor.rooms), (2.0 * (140.0 + 80.0), 4));
    let rod = k1.iter().find(|l| l.description == "WARDROBE ROD AND SHELF").expect("rod");
    assert_eq!((rod.quantity, rod.rooms), (2.0, 2));
    let towel = k1.iter().find(|l| l.description.starts_with("TOWEL BAR")).expect("towel bar");
    assert_eq!((towel.quantity, towel.rooms), (2.0, 2));
    assert!(tk.items.iter().all(|i| i.unit_type.as_deref() == Some("K1")));
    // Every line of the type sums to the rooms' own lines.
    let rooms_total: f64 = tk.items.iter().filter(|i| i.unit == "SF").map(|i| i.quantity).sum();
    let type_total: f64 = k1.iter().filter(|l| l.unit == "SF").map(|l| l.quantity).sum();
    assert!((rooms_total - type_total).abs() < 0.01);
}

#[test]
fn one_transition_per_door_typed_by_the_floor_pair() {
    let (t, _) = seed3();
    let arch = t.arch.as_ref().expect("arch");
    let floor =
        |level: &str, n: Option<&String>| n.and_then(|n| arch.finishes.iter().find(|f| f.level == level && &f.number == n)).map(|f| f.floor.clone());
    let changing: Vec<&String> = arch
        .doors
        .iter()
        .filter(|d| matches!((floor(&d.level, d.swings_into.as_ref()), floor(&d.level, d.from_room.as_ref())), (Some(a), Some(b)) if a != b))
        .map(|d| &d.mark)
        .collect();
    let tk = read3();
    let trans: Vec<&TakeoffItem> = tk.items.iter().filter(|i| i.description.starts_with("FLOOR TRANSITION")).collect();
    assert_eq!(trans.len(), changing.len(), "one per door between two floor materials");
    for m in &changing {
        assert_eq!(trans.iter().filter(|i| i.source.contains(&format!("door {m};"))).count(), 1, "door {m}");
    }
    // 105A: carpet office off the LVT corridor.
    let t105 = trans.iter().find(|i| i.source.contains("door 105A;")).expect("105A");
    assert_eq!(t105.description, "FLOOR TRANSITION, CARPET EDGE REDUCER: CPT-1 TO LVT-1");
    assert_eq!(t105.room, "105");
    // 114A: tile restroom off the corridor; 110A: LVT on both sides, so none.
    assert!(
        trans.iter().any(|i| i.source.contains("door 114A;") && i.description == "FLOOR TRANSITION, TILE TO RESILIENT TRANSITION: CT-1 TO LVT-1")
    );
    assert!(!trans.iter().any(|i| i.source.contains("door 110A;")));
}

#[test]
fn a_patterned_floor_raises_its_waste() {
    let tk = read3();
    let conf = one(&tk, "107", "CPT-2", "SF");
    assert!(conf.description.contains("HERRINGBONE"));
    assert!((conf.waste_factor - 0.10).abs() < 1e-9, "carpet tile 5% + pattern 5%: {}", conf.waste_factor);
    near(conf.with_waste, 780.0 * 1.10, "107 with waste");
    assert!(conf.rule.as_deref().is_some_and(|r| r.contains("herringbone")));
    let plain = one(&tk, "105", "CPT-1", "SF");
    assert_eq!(plain.waste_factor, 0.05);
    assert!(plain.rule.is_none());
}

#[test]
fn gypsum_ceilings_get_board_waste() {
    let tk = read3();
    let gyp = one(&tk, "114", "GYP-1", "SF");
    assert_eq!(gyp.trade, "ceilings");
    assert_eq!(gyp.category, "gypsum_board");
    assert_eq!(gyp.waste_factor, 0.10);
}

#[test]
fn exposed_structure_has_no_ceiling_line_only_paint() {
    let tk = read3();
    let mech = lines(&tk, "1", "101");
    assert!(!mech.iter().any(|i| i.trade == "ceilings"), "{mech:#?}");
    let paint = mech.iter().find(|i| i.description.starts_with("CEILING PAINT")).expect("exposed ceiling paint");
    assert_eq!(paint.quantity, 780.0);
}

#[test]
fn openings_use_the_door_height() {
    let (room, walls) = boxed_room("OFFICE", "1", 10.0, 10.0, 10.0);
    let door = TakeoffDoor {
        level: "1".into(),
        mark: Some("1A".into()),
        width_in: 36.0,
        height_in: Some(96.0),
        scheduled: true,
        swings_into: Some("1".into()),
        ..Default::default()
    };
    let input = TakeoffInput { rooms: vec![room], walls, doors: vec![door], codes: vec![code("W", "wall", "PAINT, EGGSHELL")], ..Default::default() };
    let tk = compute(&input, &[]);
    let wall = tk.items.iter().find(|i| i.description.starts_with("W:")).expect("wall paint");
    // 40' × 10' less a 3'-0" x 8'-0" door: 376 SF (a 7'-0" door would leave 379).
    assert_eq!(wall.quantity, 376.0);
    assert!(tk.items.iter().any(|i| i.description == "DOOR 1A — 3'-0\"x8'-0\"  TYPE "), "{:#?}", tk.items);
}

#[test]
fn no_rooms_read_means_an_empty_takeoff_with_no_panic() {
    let tk = compute(&TakeoffInput::default(), &default_room_rules());
    assert!(tk.items.is_empty());
    let tk = compute(&read_input(&[]), &default_room_rules());
    assert!(tk.items.is_empty());
    // A room with no finish row is reported, not dropped.
    let (mut room, walls) = boxed_room("OFFICE", "9", 10.0, 10.0, 10.0);
    room.floor = None;
    room.base = None;
    room.wall = None;
    room.ceiling = None;
    let tk = compute(&TakeoffInput { rooms: vec![room], walls, ..Default::default() }, &default_room_rules());
    assert!(tk.missing.iter().any(|m| m.contains("room 9 OFFICE") && m.contains("floor/base/wall/ceiling")), "{:?}", tk.missing);
}

/// Seed 3's per-trade totals (quantity before waste) by unit. Floor SF is the two 210' x 60'
/// floors; ceiling SF the same less the two exposed 30' x 26' mechanical rooms; glazing 24 type-1
/// windows of 6'-0" x 4'-6"; 31 doors with a door, frame and hardware set each.
const PER_TRADE_TOTALS: &[(&str, &str, f64)] = &[
    ("ceilings", "EA", 1190.0),
    ("ceilings", "LF", 22072.0),
    ("ceilings", "SF", 23640.0),
    ("doors_frames_hardware", "EA", 93.0),
    ("flooring", "EA", 27.0),
    ("flooring", "LF", 3929.0),
    ("flooring", "SF", 25200.0),
    ("glazing", "EA", 24.0),
    ("glazing", "SF", 648.0),
    ("painting", "SF", 100017.02),
    ("specialties", "EA", 136.0),
];
/// Seed 3's lines per trade (flooring: 32 floors, 32 bases, 27 transitions).
const PER_TRADE_LINES: &[(&str, usize)] =
    &[("ceilings", 96), ("doors_frames_hardware", 93), ("flooring", 91), ("glazing", 2), ("painting", 75), ("specialties", 107)];
