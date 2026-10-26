//! Q307: doors (size, swing, frame, hardware group, rating), windows and storefront, the stair
//! and elevator with their shafts and pit, slab edges, the roof and its curbs, and rooms with
//! their finishes, read from the made-up architectural sheets and scored against the truth.

use cadaddy_mepfix::arch::{hand, outside_side};
use cadaddy_mepfix::arch_reader::{self, ArchScore, read_fixture_arch, score_arch};
use cadaddy_mepfix::truth::Truth;
use cadaddy_mepfix::{generate, truth_for};
use std::collections::BTreeSet;

/// Seeds 1–6, and 17 (its plans are turned 90° on the sheets).
const SEEDS: [u64; 7] = [1, 2, 3, 4, 5, 6, 17];

fn scored(seed: u64) -> (Truth, ArchScore) {
    let (t, r) = read_fixture_arch(seed).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
    let sc = score_arch(&t, &r);
    (t, sc)
}

#[test]
fn the_truth_hangs_together() {
    for seed in 1..=12u64 {
        let t = truth_for(seed);
        let a = t.arch.as_ref().expect("arch truth");
        assert!(!a.doors.is_empty() && !a.windows.is_empty() && !a.finishes.is_empty(), "seed {seed}");
        let marks: BTreeSet<&str> = a.doors.iter().map(|d| d.mark.as_str()).collect();
        assert_eq!(marks.len(), a.doors.len(), "seed {seed}: door marks repeat");
        // Every wall opening is a door, and every door is in its wall's opening.
        let openings: usize = t.plan_walls.iter().map(|w| w.openings.len()).sum();
        assert_eq!(openings, a.doors.len(), "seed {seed}");
        for d in &a.doors {
            assert!(t.plan_walls.iter().any(|w| w.id == d.wall), "seed {seed}: door {} wall {}", d.mark, d.wall);
            assert!(["LH", "RH", "LHR", "RHR"].contains(&d.hand.as_str()), "{}", d.hand);
            assert!(a.hardware.iter().any(|g| g.group == d.hardware), "seed {seed}: {} has no group {}", d.mark, d.hardware);
            assert!(((d.hinge_ft[0] - d.latch_ft[0]).hypot(d.hinge_ft[1] - d.latch_ft[1]) * 12.0 - d.width_in).abs() < 0.5);
        }
        // The entrance swings out (reverse handing seen from outside) and has the card reader.
        let entrance: Vec<_> = a.doors.iter().filter(|d| d.swings_into.is_none()).collect();
        assert_eq!(entrance.len(), 1, "seed {seed}");
        assert!(entrance.iter().all(|d| d.hand.ends_with('R') && a.hardware.iter().any(|g| g.group == d.hardware && g.card_reader)));
        for w in &a.windows {
            assert!(a.window_types.iter().any(|x| x.mark == w.mark), "seed {seed}: window type {}", w.mark);
        }
        // The core: a stair room on both floors, the stair and elevator inside it.
        let s = a.stairs.first().expect("stair");
        assert_eq!(s.rooms.len(), 2);
        for num in &s.rooms {
            let rm = t.building.rooms.iter().find(|r| &r.number == num).expect("stair room");
            assert_eq!(rm.name, "STAIR");
            assert!(
                s.min_ft[0] > rm.min_ft[0] && s.max_ft[0] < rm.max_ft[0] && s.min_ft[1] > rm.min_ft[1] && s.max_ft[1] < rm.max_ft[1],
                "seed {seed}"
            );
            let e = a.elevators.first().expect("elevator");
            assert!(
                e.min_ft[0] > rm.min_ft[0] && e.max_ft[0] < s.min_ft[0] && e.max_ft[1] < rm.max_ft[1],
                "seed {seed}: elevator clear of the stair"
            );
        }
        assert!(
            (s.riser_in * s.risers as f64 - 12.0 * t.building.levels.iter().find(|l| l.code == "2").map_or(0.0, |l| l.elevation_ft)).abs() < 1e-3
        );
        assert!(s.riser_in <= 7.0 + 1e-9 && s.tread_in >= 11.0);
        assert_eq!(a.finishes.len(), t.building.rooms.len());
        for f in &a.finishes {
            for code in [&f.floor, &f.base, &f.wall, &f.ceiling] {
                assert!(a.finish_codes.iter().any(|c| &c.code == code), "seed {seed}: code {code} not in the legend");
            }
        }
        assert!(a.finish_codes.iter().filter(|c| c.kind == "wall").all(|c| c.plates.is_some()));
        // The full set draws the same building.
        if seed <= 3 {
            assert_eq!(generate(seed).unwrap().truth.arch, t.arch, "seed {seed}");
        }
    }
}

#[test]
fn handing_follows_the_hardware_rule() {
    // A door in a wall running east, seen from the south (outside), swinging north (away).
    let (o, n) = ([0.0, -1.0], [0.0, 1.0]);
    assert_eq!(hand([3.0, 0.0], [0.0, 0.0], n, o), "RH");
    assert_eq!(hand([0.0, 0.0], [3.0, 0.0], n, o), "LH");
    // Swinging towards the viewer: reverse.
    assert_eq!(hand([3.0, 0.0], [0.0, 0.0], o, o), "RHR");
    assert_eq!(hand([0.0, 0.0], [3.0, 0.0], o, o), "LHR");
    // Outside: the exterior first, then the corridor, else away from the swing.
    assert_eq!(outside_side(None, Some("CORRIDOR"), 1.0), 1.0);
    assert_eq!(outside_side(Some("OFFICE"), Some("CORRIDOR"), 1.0), -1.0);
    assert_eq!(outside_side(Some("OFFICE"), Some("STORAGE"), 1.0), -1.0);
}

#[test]
fn doors_marks_swings_and_schedule_rows() {
    for seed in SEEDS {
        let (_, s) = scored(seed);
        let why = s.misses.join("\n");
        assert!(s.doors_total > 0);
        assert_eq!(s.doors_found, s.doors_total, "seed {seed}\n{why}");
        assert_eq!(s.door_marks, s.doors_total, "seed {seed}\n{why}");
        assert_eq!(s.door_hands, s.doors_total, "seed {seed}: hinge side and handing\n{why}");
        assert_eq!(s.door_rooms, s.doors_total, "seed {seed}: rooms either side\n{why}");
        assert_eq!(s.door_rows, s.doors_total, "seed {seed}: size, type, material, frame, rating, hardware\n{why}");
        assert_eq!(s.door_card_readers, s.doors_total, "seed {seed}");
        assert_eq!(s.hardware_correct, s.hardware_total, "seed {seed}: hardware groups");
    }
}

#[test]
fn windows_and_storefront() {
    let mut storefront = false;
    for seed in SEEDS {
        let (t, s) = scored(seed);
        let why = s.misses.join("\n");
        assert!(s.windows_total > 0, "seed {seed}");
        assert_eq!(s.windows_found, s.windows_total, "seed {seed}\n{why}");
        assert_eq!(s.window_marks, s.windows_total, "seed {seed}\n{why}");
        assert_eq!(s.window_types_correct, s.window_types_total, "seed {seed}\n{why}");
        storefront |= t.arch.as_ref().is_some_and(|a| a.window_types.iter().any(|w| w.storefront && w.sill_in == 0.0));
    }
    assert!(storefront, "no seed has a storefront");
}

#[test]
fn stair_and_elevator_with_shaft_and_pit() {
    for seed in SEEDS {
        let (_, s) = scored(seed);
        let why = s.misses.join("\n");
        assert_eq!((s.stairs_correct, s.stairs_total), (1, 1), "seed {seed}: footprint, width, risers, riser and tread, levels\n{why}");
        assert_eq!((s.elevators_correct, s.elevators_total), (1, 1), "seed {seed}: hoistway, pit, capacity, levels\n{why}");
    }
}

#[test]
fn slab_edges_and_roof() {
    let mut parapet = false;
    for seed in SEEDS {
        let (t, s) = scored(seed);
        let why = s.misses.join("\n");
        assert_eq!(s.slab_edges_correct, s.slab_edges_total, "seed {seed}\n{why}");
        assert_eq!(s.slab_edges_total, 3);
        assert!(s.roof_correct, "seed {seed}: edge, parapet, slope\n{why}");
        assert_eq!(s.drains_found, s.drains_total, "seed {seed}");
        // Equipment curbs, the roof hatch and the elevator overrun.
        assert!(s.curbs_total >= 3, "seed {seed}");
        assert_eq!(s.curbs_correct, s.curbs_total, "seed {seed}\n{why}");
        parapet |= t.arch.as_ref().is_some_and(|a| a.roof.parapet_in.is_some());
    }
    assert!(parapet, "no seed has a parapet");
}

#[test]
fn rooms_and_finishes() {
    for seed in SEEDS {
        let (_, s) = scored(seed);
        let why = s.misses.join("\n");
        assert_eq!(s.rooms_found, s.rooms_total, "seed {seed}: names and numbers\n{why}");
        assert_eq!(s.finishes_correct, s.rooms_total, "seed {seed}: floor, base, wall, ceiling, height, plates\n{why}");
        assert_eq!(s.codes_correct, s.codes_total, "seed {seed}: finish legend\n{why}");
    }
}

#[test]
fn hostile_sheets_never_panic() {
    let set = generate(2).unwrap();
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = cadaddy_mepfix::sheet_reader::recognise_fixture_set(&dxf);
    let ceilings = cadaddy_mepfix::ceiling_reader::read_ceilings(&dxf, &map, None);
    let junk = |s: &str| {
        s.replace("3'-0\"", "99999999999999999999'-1/0\"")
            .replace("7'-0\"", "-/-'")
            .replace("20 MIN", "1e309 MIN")
            .replace("PIT ", "PIT :::")
            .replace(" LB", "999999999999 LB")
            .replace("CURB", "\"\"\" CURB")
            .replace("SLOPE ", "SLOPE /")
            .replace("UP ", "UP 99999999999999999999R @ ")
    };
    let hostile: Vec<(String, Vec<u8>)> = dxf
        .iter()
        .map(|(n, b)| {
            if n.starts_with("A-") {
                let s = String::from_utf8_lossy(b);
                (n.clone(), junk(&s).into_bytes())
            } else {
                (n.clone(), b.clone())
            }
        })
        .collect();
    let r = arch_reader::read_arch(&hostile, &map, &ceilings);
    let _ = score_arch(&set.truth, &r);
    // Truncated and empty files, and an empty map.
    let cut: Vec<(String, Vec<u8>)> = dxf.iter().map(|(n, b)| (n.clone(), b.get(..b.len() / 3).unwrap_or_default().to_vec())).collect();
    let _ = arch_reader::read_arch(&cut, &map, &ceilings);
    let _ = arch_reader::read_arch(&[("A-101.dxf".into(), Vec::new())], &Default::default(), &Default::default());
}

#[test]
fn the_spec_patch_is_a_valid_building_spec() {
    let (t, r) = read_fixture_arch(3).unwrap();
    let patch = arch_reader::spec_patch(&r, &|level, p| (level == "1").then_some([p[0] * 12.0, p[1] * 12.0]));
    // Level 2 too, for the stair to reach (its plan isn't built here).
    let f2f = t.building.levels.iter().find(|l| l.code == "2").map_or(0.0, |l| l.elevation_ft);
    let mut spec = serde_json::json!({"version": 1, "levels": [
        {"name": "1", "elevation_ft": 0.0, "height_ft": f2f, "ceiling_ft": 9.0},
        {"name": "2", "elevation_ft": f2f, "height_ft": 14.0, "ceiling_ft": 9.0}]});
    cadaddy_walk3d::merge(&mut spec, &patch);
    let sp = cadaddy_walk3d::BuildingSpec::from_json(&spec.to_string()).unwrap_or_else(|e| panic!("{e}\n{spec:#}"));
    let errors: Vec<_> = sp.check().into_iter().filter(|i| i.error).collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(sp.rooms.len(), r.rooms.iter().filter(|rm| rm.level == "1").count());
    assert!(sp.rooms.iter().all(|rm| rm.number.is_some() && rm.materials.floor.is_some() && rm.materials.wall.is_some()));
    let doors = sp.schedule.rows.iter().filter(|x| x.kind == cadaddy_walk3d::FamilyKind::Door).count();
    assert_eq!(doors, r.door_rows.len());
    assert!(sp.schedule.rows.iter().any(|x| x.kind == cadaddy_walk3d::FamilyKind::Window && x.sill_ft.is_some()));
    let level1 = r.doors.iter().filter(|d| d.level == "1" && d.mark.is_some()).count()
        + r.windows.iter().filter(|w| w.level == "1" && w.mark.is_some()).count();
    assert_eq!(sp.schedule.tags.len(), level1);
    let row = sp.schedule.rows.iter().find(|x| x.fire_rating.is_some()).expect("a rated door");
    assert!(row.fire_rating.as_deref().is_some_and(|f| f.ends_with("MIN")));
    // Q5107: what matters downstream. Each level-1 door tag carries its hand, rooms, hinge,
    // latch and card reader as read.
    let a = t.arch.as_ref().expect("arch truth");
    let door_tags: Vec<_> = sp.schedule.tags.iter().filter(|g| g.hand.is_some()).collect();
    assert_eq!(door_tags.len(), r.doors.iter().filter(|d| d.level == "1" && d.mark.is_some()).count());
    assert!(door_tags.iter().any(|g| g.card_reader), "no level-1 door has a card reader");
    for g in &door_tags {
        let d = r.doors.iter().find(|d| d.level == "1" && d.mark.as_deref() == Some(g.mark.as_str())).expect("its door");
        assert_eq!(g.hand.as_deref(), Some(d.hand.as_str()), "door {}", g.mark);
        assert_eq!(g.card_reader, d.card_reader, "door {}", g.mark);
        assert_eq!((&g.swings_into, &g.from_room), (&d.swings_into, &d.from_room), "door {}", g.mark);
        assert_eq!(g.hinge, Some([d.hinge_ft[0] * 12.0, d.hinge_ft[1] * 12.0]), "door {}", g.mark);
        assert_eq!(g.latch, Some([d.latch_ft[0] * 12.0, d.latch_ft[1] * 12.0]), "door {}", g.mark);
        assert_eq!(g.source.as_deref(), Some(d.source.sheet.as_str()));
        // And as the truth has it.
        let td = a.doors.iter().find(|x| x.mark == g.mark).expect("truth door");
        assert_eq!(g.hand.as_deref(), Some(td.hand.as_str()), "door {}", g.mark);
    }
    // Every level-1 room's base and device plates as the finish schedule and legend give them.
    for rm in &sp.rooms {
        let num = rm.number.as_deref().unwrap_or_default();
        let fin = a.finishes.iter().find(|f| f.level == "1" && f.number == num).expect("its finish row");
        let plates = a.finish_codes.iter().find(|c| c.code == fin.wall).and_then(|c| c.plates.clone());
        assert!(plates.is_some(), "room {num}: the legend gives its plates");
        assert_eq!(rm.plates, plates, "room {num}");
        assert!(rm.base.as_deref().is_some_and(|b| b.starts_with(fin.base.as_str())), "room {num}: base {:?} not {}", rm.base, fin.base);
    }
    // One stair, from level 1 to 2: its risers times the riser height is floor to floor.
    assert_eq!(sp.stairs.len(), 1, "{:?}", sp.stairs);
    let st = sp.stairs.first().expect("stair");
    let ts = a.stairs.first().expect("truth stair");
    assert_eq!((st.name.as_str(), st.level.as_str(), st.to_level.as_deref()), (ts.tag.as_str(), "1", Some("2")));
    assert!((st.width_in - ts.width_in).abs() < 1.0);
    let (risers, riser_in, tread_in, _) = sp.stair_measures(st).expect("measures");
    assert_eq!(risers, ts.risers);
    assert!((riser_in * f64::from(risers) - f2f * 12.0).abs() < 1e-6, "{risers} × {riser_in} vs {f2f} ft");
    assert!((tread_in - ts.tread_in).abs() < 0.25, "tread {tread_in}");
    // The elevator hoistway with its pit.
    assert_eq!(sp.elevators.len(), 1);
    let (e, te) = (sp.elevators.first().expect("hoistway"), a.elevators.first().expect("truth elevator"));
    assert_eq!((e.name.as_str(), e.levels.clone()), (te.tag.as_str(), vec!["1".to_string(), "2".to_string()]));
    assert!(e.pit_ft.is_some_and(|p| (p - te.pit_ft).abs() < 0.01));
    assert_eq!(e.capacity_lb, Some(te.capacity_lb));
    assert!((e.min[0] - te.min_ft[0] * 12.0).abs() < 1.0 && (e.max[1] - te.max_ft[1] * 12.0).abs() < 1.0);
}

/// Seed 3's sheets, their map and the ceilings read off them.
fn seed3() -> (Truth, Vec<(String, Vec<u8>)>, cadaddy_mepfix::sheet_reader::SheetMap, cadaddy_mepfix::ceiling_reader::CeilingResult) {
    let set = generate(3).unwrap();
    let dxf: Vec<(String, Vec<u8>)> = set.files.iter().filter(|(n, _)| n.ends_with(".dxf")).cloned().collect();
    let map = cadaddy_mepfix::sheet_reader::recognise_fixture_set(&dxf);
    let ceilings = cadaddy_mepfix::ceiling_reader::read_ceilings(&dxf, &map, None);
    (set.truth, dxf, map, ceilings)
}

#[test]
fn every_item_says_where_it_came_from() {
    let (_, dxf, map, ceilings) = seed3();
    let r = arch_reader::read_arch(&dxf, &map, &ceilings);
    let plans: Vec<&str> = r.plans.iter().map(|(s, _)| s.as_str()).collect();
    assert!(plans.len() >= 2, "{plans:?}");
    let sheet = |s: &Option<String>| s.clone().expect("schedule sheet");
    let (doors, windows, finishes) = (sheet(&r.sheets.doors), sheet(&r.sheets.windows), sheet(&r.sheets.finishes));
    assert!(doors.starts_with("A-") && finishes.starts_with("A-") && !plans.contains(&doors.as_str()));
    assert!(!r.doors.is_empty() && !r.windows.is_empty() && !r.rooms.is_empty());
    for d in &r.doors {
        assert!(plans.contains(&d.source.sheet.as_str()), "{:?}", d.source);
        assert_eq!((d.source.schedule.as_deref(), d.source.status.as_str()), (Some(doors.as_str()), "schedule"), "{:?}", d.mark);
        // The plan sheet is the one for its level.
        assert!(r.plans.iter().any(|(s, l)| *s == d.source.sheet && *l == d.level));
    }
    for w in &r.windows {
        assert!(plans.contains(&w.source.sheet.as_str()));
        assert_eq!((w.source.schedule.as_deref(), w.source.status.as_str()), (Some(windows.as_str()), "schedule"));
    }
    for rm in &r.rooms {
        assert!(plans.contains(&rm.source.sheet.as_str()));
        assert_eq!((rm.source.schedule.as_deref(), rm.source.status.as_str()), (Some(finishes.as_str()), "schedule"), "room {}", rm.number);
    }
    for s in r.stairs.iter().map(|s| &s.source).chain(r.elevators.iter().map(|e| &e.source)) {
        assert!(plans.contains(&s.sheet.as_str()) && s.status == "drawing" && s.reason.is_none(), "{s:?}");
    }
    assert!(r.roof.as_ref().is_some_and(|f| f.source.status == "drawing" && Some(&f.source.sheet) == r.sheets.roof.as_ref()));
    assert!(r.misses().is_empty(), "{:?}", r.misses());

    // One door's tag and one room's finish row taken off the sheets: both are still read, as
    // guesses that say why, and listed in the misses.
    let d0 = r.doors.first().expect("a door").clone();
    let mark = d0.mark.clone().expect("marked");
    let rm0 = r.rooms.iter().find(|rm| r.doors.iter().all(|d| d.mark.as_deref() != Some(rm.number.as_str()))).expect("a room").clone();
    let edit = |bytes: &[u8], drop: &dyn Fn(&cadaddy_doc::Entity) -> bool| {
        let mut d = cadaddy_io::read_dxf(bytes).expect("dxf");
        let keep: Vec<cadaddy_doc::Entity> = d.model.iter().filter(|e| !drop(e)).map(|e| (**e).clone()).collect();
        assert!(keep.len() < d.model.len(), "nothing dropped");
        let mut store = cadaddy_doc::EntityStore::new();
        store.push_many(keep);
        d.model = store;
        cadaddy_io::write_dxf(&d).into_bytes()
    };
    let text_is = |e: &cadaddy_doc::Entity, v: &str| matches!(&e.kind, cadaddy_doc::EntityKind::Text(t) if t.value.trim() == v);
    let cut: Vec<(String, Vec<u8>)> = dxf
        .iter()
        .map(|(n, b)| {
            let stem = n.trim_end_matches(".dxf");
            if stem == d0.source.sheet {
                (n.clone(), edit(b, &|e| e.common.layer == "A-DOOR-IDEN" && text_is(e, &mark)))
            } else if stem == finishes {
                (n.clone(), edit(b, &|e| text_is(e, &rm0.number)))
            } else {
                (n.clone(), b.clone())
            }
        })
        .collect();
    let r2 = arch_reader::read_arch(&cut, &map, &ceilings);
    assert_eq!(r2.doors.len(), r.doors.len(), "the unmarked door is kept");
    let un = r2.doors.iter().find(|d| d.mark.is_none()).expect("the unmarked door");
    assert!((un.at_ft[0] - d0.at_ft[0]).abs() < 1e-6 && (un.at_ft[1] - d0.at_ft[1]).abs() < 1e-6);
    assert_eq!((un.source.status.as_str(), un.source.schedule.as_deref()), ("guess", None));
    assert!(un.source.reason.as_deref().is_some_and(|w| w.contains("no door mark")));
    assert_eq!(r2.rooms.len(), r.rooms.len(), "the room without finishes is kept");
    let bare = r2.rooms.iter().find(|x| x.number == rm0.number && x.level == rm0.level).expect("the room");
    assert_eq!((bare.source.status.as_str(), bare.floor.as_deref()), ("guess", None));
    let misses = r2.misses();
    assert!(misses.iter().any(|m| m.starts_with("door at ") && m.contains("no door mark")), "{misses:?}");
    assert!(misses.iter().any(|m| m.starts_with(&format!("room {} ", rm0.number)) && m.contains("finish")), "{misses:?}");
}

#[test]
fn styles_vary_by_seed() {
    let mut marks = BTreeSet::new();
    let mut codes = BTreeSet::new();
    let mut hands = BTreeSet::new();
    for seed in 1..=16u64 {
        let t = truth_for(seed);
        let a = t.arch.as_ref().expect("arch");
        for d in &a.doors {
            marks.insert(d.mark.chars().filter(|c| !c.is_ascii_digit()).collect::<String>());
            hands.insert(d.hand.clone());
        }
        codes.insert(a.finish_codes.iter().any(|c| c.code.contains('-')));
    }
    assert!(marks.contains("") && marks.contains("A") && marks.contains("D"), "{marks:?}");
    assert_eq!(hands.len(), 4, "{hands:?}");
    assert_eq!(codes.len(), 2);
}
