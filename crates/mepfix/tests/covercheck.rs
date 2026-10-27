//! Tests for pre-cover hold points and in-wall photos (Q334).

use cadaddy_mepfix::covercheck::*;
use cadaddy_mepfix::truth::{Ceiling, PlanWall, Room};

/// A minimal truth with two walls and one room with a ceiling, for testing.
fn test_truth() -> cadaddy_mepfix::truth::Truth {
    let mut t = cadaddy_mepfix::truth::Truth::default();
    t.building.levels.push(cadaddy_mepfix::truth::Level { name: "1".into(), code: "1".into(), elevation_ft: 0.0 });
    t.building.rooms.push(Room {
        id: "R1".into(),
        level: "1".into(),
        number: "101".into(),
        name: "OFFICE".into(),
        min_ft: [0.0, 0.0],
        max_ft: [20.0, 15.0],
        ceiling: Some(Ceiling { height_ft: 9.0, kind: "ACT".into(), ..Ceiling::default() }),
        plenum_ft: Some(3.0),
    });
    t.building.rooms.push(Room {
        id: "R2".into(),
        level: "1".into(),
        number: "102".into(),
        name: "CORRIDOR".into(),
        min_ft: [0.0, 15.0],
        max_ft: [20.0, 20.0],
        ceiling: None,
        plenum_ft: None,
    });
    // Wall type.
    t.wall_types.push(cadaddy_mepfix::truth::WallType {
        tag: "1A".into(),
        name: "Interior Partition".into(),
        layers: vec![
            cadaddy_mepfix::truth::WallTypeLayer { material: "gyp".into(), thickness_in: 0.625, side: "1".into(), ..Default::default() },
            cadaddy_mepfix::truth::WallTypeLayer {
                material: "stud".into(),
                thickness_in: 3.625,
                side: "core".into(),
                spacing_in: Some(16.0),
                ..Default::default()
            },
            cadaddy_mepfix::truth::WallTypeLayer { material: "gyp".into(), thickness_in: 0.625, side: "2".into(), ..Default::default() },
        ],
        thickness_in: 4.875,
        height: "ceiling".into(),
        fire_rating: Some("1 HR".into()),
        stc: Some(45),
        external: false,
        head: None,
        bottom_track: true,
    });
    // Two plan walls along y=15 (separating room 101 from corridor 102).
    t.plan_walls.push(PlanWall {
        id: "W1-001".into(),
        level: "1".into(),
        a_ft: [0.0, 15.0],
        b_ft: [10.0, 15.0],
        wall_type: "1A".into(),
        tagged: true,
        height_ft: 9.0,
        rooms: vec!["101".into(), "102".into()],
        openings: vec![[3.0, 6.0]],
        tags: Vec::new(),
        layout_from: None,
        plan_tag: None,
        enlarged_tag: None,
    });
    t.plan_walls.push(PlanWall {
        id: "W1-002".into(),
        level: "1".into(),
        a_ft: [10.0, 15.0],
        b_ft: [20.0, 15.0],
        wall_type: "1A".into(),
        tagged: true,
        height_ft: 9.0,
        rooms: vec!["101".into(), "102".into()],
        openings: Vec::new(),
        tags: Vec::new(),
        layout_from: None,
        plan_tag: None,
        enlarged_tag: None,
    });
    t
}

#[test]
fn areas_from_truth_creates_wall_faces_and_ceilings() {
    let t = test_truth();
    let areas = areas_from_truth(&t);
    // 2 walls × 2 faces + 1 ceiling = 5 areas.
    assert_eq!(areas.len(), 5, "expected 5 areas, got {}", areas.len());
    // Check wall faces.
    let wall_faces: Vec<_> = areas.iter().filter(|a| a.kind == AreaKind::WallFace).collect();
    assert_eq!(wall_faces.len(), 4);
    // Each wall has face A and B.
    assert!(areas.iter().any(|a| a.id == "W1-001/A"));
    assert!(areas.iter().any(|a| a.id == "W1-001/B"));
    assert!(areas.iter().any(|a| a.id == "W1-002/A"));
    assert!(areas.iter().any(|a| a.id == "W1-002/B"));
    // Ceiling area.
    let clg: Vec<_> = areas.iter().filter(|a| a.kind == AreaKind::Ceiling).collect();
    assert_eq!(clg.len(), 1);
    assert_eq!(clg[0].id, "CLG-1-101");
    assert_eq!(clg[0].rooms, vec!["101"]);
}

#[test]
fn default_checklist_has_standard_items() {
    let cl = default_checklist("test");
    assert_eq!(cl.len(), STANDARD_CATEGORIES.len());
    for (item, (cat, _)) in cl.iter().zip(STANDARD_CATEGORIES) {
        assert_eq!(item.category, *cat);
        assert!(!item.done);
    }
}

#[test]
fn hold_point_blocks_cover_until_checklist_complete() {
    let t = test_truth();
    let mut record = record_from_truth(&t, "Test Project");
    // Sign all inspection phases so only the checklist matters.
    for ph in record.phases.iter_mut() {
        ph.sign("Inspector", "2026-10-21");
    }
    let area_id = "W1-001/A";
    // Not ready yet: checklist incomplete.
    {
        let area = record.area(area_id).unwrap();
        assert!(!area.checklist_complete());
        assert!(!area.can_mark_ready(&record.phases));
    }
    // Try to mark ready — must fail.
    let phases_copy = record.phases.clone();
    {
        let area = record.area_mut(area_id).unwrap();
        assert!(!area.mark_ready("Foreman", "2026-10-21", &phases_copy));
        assert!(!area.ready_to_cover);
    }
    // Complete all checklist items.
    let item_ids: Vec<String> = record.area(area_id).unwrap().checklist.iter().map(|c| c.id.clone()).collect();
    let phases_copy2 = record.phases.clone();
    {
        let area = record.area_mut(area_id).unwrap();
        for id in &item_ids {
            assert!(area.check_item(id, "Electrician", "2026-10-21"));
        }
        assert!(area.checklist_complete());
        // Now marking ready should succeed.
        assert!(area.mark_ready("Foreman", "2026-10-21", &phases_copy2));
        assert!(area.ready_to_cover);
        assert_eq!(area.ready_by.as_deref(), Some("Foreman"));
    }
}

#[test]
fn hold_point_blocks_cover_until_inspections_signed() {
    let t = test_truth();
    let mut record = record_from_truth(&t, "Test Project");
    let area_id = "W1-001/A";
    // Complete all checklist items.
    let item_ids: Vec<String> = record.area(area_id).unwrap().checklist.iter().map(|c| c.id.clone()).collect();
    {
        let area = record.area_mut(area_id).unwrap();
        for id in &item_ids {
            area.check_item(id, "Electrician", "2026-10-21");
        }
        assert!(area.checklist_complete());
    }
    // Inspections not signed — can't mark ready.
    let phases_copy = record.phases.clone();
    {
        let area = record.area_mut(area_id).unwrap();
        assert!(!area.can_mark_ready(&phases_copy));
        assert!(!area.mark_ready("Foreman", "2026-10-21", &phases_copy));
    }
    // Sign all inspection phases.
    for ph in record.phases.iter_mut() {
        ph.sign("Inspector", "2026-10-21");
    }
    let phases_copy2 = record.phases.clone();
    {
        let area = record.area_mut(area_id).unwrap();
        assert!(area.can_mark_ready(&phases_copy2));
        assert!(area.mark_ready("Foreman", "2026-10-21", &phases_copy2));
    }
}

#[test]
fn uncheck_item_revokes_ready_status() {
    let t = test_truth();
    let mut record = record_from_truth(&t, "Test Project");
    for ph in record.phases.iter_mut() {
        ph.sign("Inspector", "2026-10-21");
    }
    let area_id = "W1-001/A";
    let item_ids: Vec<String> = record.area(area_id).unwrap().checklist.iter().map(|c| c.id.clone()).collect();
    let phases_copy = record.phases.clone();
    {
        let area = record.area_mut(area_id).unwrap();
        for id in &item_ids {
            area.check_item(id, "Electrician", "2026-10-21");
        }
        area.mark_ready("Foreman", "2026-10-21", &phases_copy);
        assert!(area.ready_to_cover);
        // Uncheck one item.
        area.uncheck_item(&item_ids[0]);
        assert!(!area.ready_to_cover, "unchecking an item should revoke ready status");
        assert!(!area.checklist_complete());
    }
}

#[test]
fn photos_resolve_to_correct_wall_face() {
    let t = test_truth();
    let mut record = record_from_truth(&t, "Test Project");
    // Add a photo to face A of W1-001, taken on its north side (face A = left of a→b = north).
    let photo = InWallPhoto {
        id: "P001".into(),
        face: "A".into(),
        elevation_ft: 4.0,
        along_ft: 5.0,
        uri: "photos/P001.jpg".into(),
        author: "Foreman".into(),
        date: "2026-10-21".into(),
        caption: "Boxes installed".into(),
        at_ft: [5.0, 15.5],
        level: "1".into(),
    };
    {
        let area = record.area_mut("W1-001/A").unwrap();
        assert!(area.add_photo(photo));
        assert_eq!(area.photos.len(), 1);
        assert_eq!(area.photos[0].id, "P001");
    }
    // Tap on the south side (below y=15) of wall W1-001 at x=5.
    // Wall goes east: a→b = [0,15]→[10,15], left of a→b = north (y+), so face A = north,
    // face B = south.
    let tap = [5.0, 14.5]; // slightly below the wall at y=15 → face B (right of a→b)
    let result = resolve_wall_tap(&record, &t.plan_walls, tap, "1", 2.0);
    assert!(result.is_some(), "should resolve a wall tap");
    let (area_id, dist) = result.unwrap();
    assert_eq!(area_id, "W1-001/B");
    assert!(dist < 1.0, "distance should be small: {dist}");
    // The photo is on face A but the tap resolved to face B — verify the photo on the correct face.
    let area_b = record.area(&area_id).unwrap();
    assert_eq!(area_b.photos.len(), 0, "face B has no photos");
    let area_a = record.area("W1-001/A").unwrap();
    assert_eq!(area_a.photos.len(), 1);
    assert_eq!(area_a.photos[0].id, "P001");
}

#[test]
fn tap_on_other_face_resolves_to_face_b() {
    let t = test_truth();
    let record = record_from_truth(&t, "Test Project");
    // Tap on the north side (left of a→b = face A) of wall W1-001 at x=5.
    let tap = [5.0, 15.5]; // slightly above the wall at y=15 → face A
    let result = resolve_wall_tap(&record, &t.plan_walls, tap, "1", 2.0);
    assert!(result.is_some());
    let (area_id, _) = result.unwrap();
    assert_eq!(area_id, "W1-001/A");
}

#[test]
fn ceiling_tap_resolves_to_correct_room() {
    let t = test_truth();
    let record = record_from_truth(&t, "Test Project");
    // Tap inside room 101 (has a ceiling).
    let result = resolve_ceiling_tap(&record, &t.building.rooms, [10.0, 7.5], "1");
    assert_eq!(result, Some("CLG-1-101".into()));
    // Tap in the corridor (no ceiling) — no match.
    let result2 = resolve_ceiling_tap(&record, &t.building.rooms, [10.0, 17.5], "1");
    assert_eq!(result2, None);
}

#[test]
fn wall_contents_shows_both_faces() {
    let t = test_truth();
    let mut record = record_from_truth(&t, "Test Project");
    // Add photos to each face.
    // Face A is north of the wall (y > 15), face B south.
    for (face, along, y) in [("A", 3.0, 16.0), ("B", 3.0, 14.0)] {
        let area_id = format!("W1-001/{face}");
        let photo = InWallPhoto {
            id: format!("P-{face}"),
            face: face.into(),
            elevation_ft: 4.0,
            along_ft: along,
            uri: format!("photos/{face}.jpg"),
            author: "Foreman".into(),
            date: "2026-10-21".into(),
            caption: String::new(),
            at_ft: [3.0, y],
            level: "1".into(),
        };
        record.area_mut(&area_id).unwrap().add_photo(photo);
    }
    let contents = record.wall_contents("W1-001");
    assert_eq!(contents.faces.len(), 2);
    assert_eq!(contents.photo_count, 2);
    assert!(!contents.all_ready);
    // Each face has the standard checklist.
    for f in &contents.faces {
        assert_eq!(f.checklist_total, STANDARD_CATEGORIES.len());
        assert_eq!(f.checklist_done, 0);
    }
}

#[test]
fn progress_tracks_whole_project() {
    let t = test_truth();
    let record = record_from_truth(&t, "Test Project");
    let prog = record.progress();
    assert_eq!(prog.total, 5); // 4 wall faces + 1 ceiling
    assert_eq!(prog.ready, 0);
    assert_eq!(prog.photos, 0);
    assert_eq!(prog.phases_total, STANDARD_PHASES.len());
    assert_eq!(prog.phases_signed, 0);
}

#[test]
fn fire_rating_propagates_to_area() {
    let t = test_truth();
    let record = record_from_truth(&t, "Test Project");
    let area = record.area("W1-001/A").unwrap();
    assert_eq!(area.fire_rating.as_deref(), Some("1 HR"));
    assert_eq!(area.type_tag, "1A");
}

#[test]
fn summary_by_level_counts_correctly() {
    let t = test_truth();
    let record = record_from_truth(&t, "Test Project");
    let summaries = summary_by_level(&record);
    assert_eq!(summaries.len(), 1); // only level "1"
    let s = &summaries[0];
    assert_eq!(s.level, "1");
    assert_eq!(s.walls_total, 4); // 2 walls × 2 faces
    assert_eq!(s.ceilings_total, 1);
    assert_eq!(s.walls_ready, 0);
    assert_eq!(s.ceilings_ready, 0);
}

#[test]
fn tap_far_from_wall_returns_none() {
    let t = test_truth();
    let record = record_from_truth(&t, "Test Project");
    // Tap far from any wall.
    let result = resolve_wall_tap(&record, &t.plan_walls, [100.0, 100.0], "1", 2.0);
    assert!(result.is_none());
}

#[test]
fn tap_wrong_level_returns_none() {
    let t = test_truth();
    let record = record_from_truth(&t, "Test Project");
    // Tap at correct position but wrong level.
    let result = resolve_wall_tap(&record, &t.plan_walls, [5.0, 14.5], "2", 2.0);
    assert!(result.is_none());
}

#[test]
fn fixture_set_cover_record() {
    // Build from the real fixture set to ensure it integrates with the full truth.
    let t = cadaddy_mepfix::truth_for(42);
    let record = record_from_truth(&t, "Fixture Seed 42");
    // Should have areas for every plan wall (2 faces each) plus ceilings.
    let wall_count = t.plan_walls.len() * 2;
    let ceiling_count = t.building.rooms.iter().filter(|r| r.ceiling.is_some()).count();
    assert_eq!(record.areas.len(), wall_count + ceiling_count, "wall faces={wall_count}, ceilings={ceiling_count}, areas={}", record.areas.len());
    // Every area has a non-empty checklist.
    for a in &record.areas {
        assert!(!a.checklist.is_empty(), "area {} has empty checklist", a.id);
        assert!(!a.ready_to_cover);
    }
    // Phases exist.
    assert!(!record.phases.is_empty());
}

// ---- Rework Q5119 (covercheck-rw1) -------------------------------------------------------------

fn photo(id: &str, at: [f64; 2], level: &str, face: &str) -> InWallPhoto {
    InWallPhoto {
        id: id.into(),
        face: face.into(),
        elevation_ft: 4.0,
        along_ft: 0.0,
        uri: format!("photos/{id}.jpg"),
        author: "Foreman".into(),
        date: "2026-10-21".into(),
        caption: String::new(),
        at_ft: at,
        level: level.into(),
    }
}

/// Text just over the cap, with a two-byte letter straddling byte 2000.
fn long_text() -> String {
    let mut s = "a".repeat(1999);
    s.push_str(&"é".repeat(40));
    s
}

#[test]
fn long_multibyte_text_never_panics() {
    let t = test_truth();
    let mut record = record_from_truth(&t, "Test Project");
    let text = long_text();
    assert!(text.len() > 2000 && !text.is_char_boundary(2000));
    assert!(clamp_text(&text).len() <= 2000);
    for ph in record.phases.iter_mut() {
        ph.sign(&text, &text);
        assert!(ph.inspector.as_ref().is_some_and(|i| i.len() <= 2000 && i.starts_with("aaa")));
    }
    let ids: Vec<String> = record.area("W1-001/A").unwrap().checklist.iter().map(|c| c.id.clone()).collect();
    let phases = record.phases.clone();
    let area = record.area_mut("W1-001/A").unwrap();
    for id in &ids {
        assert!(area.check_item(id, &text, &text));
    }
    assert!(area.mark_ready(&text, &text, &phases));
    assert!(area.ready_by.as_ref().is_some_and(|b| b.len() <= 2000));
    // Every text field of a photo is cut on a character boundary.
    let mut p = photo(&text, [5.0, 15.5], &text, &text);
    p.uri = text.clone();
    p.author = text.clone();
    p.date = text.clone();
    p.caption = text.clone();
    assert!(area.add_photo(p));
    let kept = area.photos.last().unwrap();
    for f in [&kept.id, &kept.face, &kept.uri, &kept.author, &kept.date, &kept.caption, &kept.level] {
        assert!(f.len() <= 2000 && f.ends_with('a'), "{}", f.len());
    }
    // Numbers that aren't finite are refused.
    for (elev, along, at) in
        [(f64::NAN, 0.0, [1.0, 1.0]), (1.0, f64::INFINITY, [1.0, 1.0]), (1.0, 0.0, [f64::NAN, 1.0]), (1.0, 0.0, [1.0, f64::NEG_INFINITY])]
    {
        let mut p = photo("bad", at, "1", "A");
        p.elevation_ft = elev;
        p.along_ft = along;
        assert!(!area.add_photo(p));
    }
    assert_eq!(area.photos.len(), 1);
}

#[test]
fn face_rooms_follow_the_geometry() {
    let mut t = test_truth();
    let record = record_from_truth(&t, "Test Project");
    // W1-001 runs east along y=15: face A (left) is north, the corridor; face B the office.
    assert_eq!(record.area("W1-001/A").unwrap().rooms, vec!["102"]);
    assert_eq!(record.area("W1-001/B").unwrap().rooms, vec!["101"]);
    // Drawn the other way, the faces swap.
    t.plan_walls[1].a_ft = [20.0, 15.0];
    t.plan_walls[1].b_ft = [10.0, 15.0];
    // An exterior wall (west side of the office, drawn north to south): one room, on one face.
    t.plan_walls.push(cadaddy_mepfix::truth::PlanWall {
        id: "W1-003".into(),
        a_ft: [0.0, 15.0],
        b_ft: [0.0, 0.0],
        rooms: vec!["101".into()],
        openings: Vec::new(),
        ..t.plan_walls[0].clone()
    });
    let record = record_from_truth(&t, "Test Project");
    assert_eq!(record.area("W1-002/A").unwrap().rooms, vec!["101"]);
    assert_eq!(record.area("W1-002/B").unwrap().rooms, vec!["102"]);
    // South-going: left (A) is east, inside the office; the outside face has no room.
    assert_eq!(record.area("W1-003/A").unwrap().rooms, vec!["101"]);
    assert!(record.area("W1-003/B").unwrap().rooms.is_empty());
}

#[test]
fn photos_are_filed_by_position() {
    let t = test_truth();
    let mut record = record_from_truth(&t, "Test Project");
    // South of W1-001 (y=14.5): the office face B, 5 ft from its a end.
    let area = add_photo_at(&mut record, &t.plan_walls, photo("P1", [5.0, 14.5], "1", ""), PHOTO_REACH_FT).unwrap();
    assert_eq!(area, "W1-001/B");
    let p = &record.area("W1-001/B").unwrap().photos[0];
    assert_eq!(p.face, "B");
    assert!((p.along_ft - 5.0).abs() < 1e-9, "{}", p.along_ft);
    // North (y=15.5): face A, the corridor side; a stated face that agrees is fine.
    assert_eq!(add_photo_at(&mut record, &t.plan_walls, photo("P2", [5.0, 15.5], "1", "a"), PHOTO_REACH_FT).unwrap(), "W1-001/A");
    // W1-002 starts at x=10: along is from its own a end.
    assert_eq!(add_photo_at(&mut record, &t.plan_walls, photo("P3", [13.0, 14.0], "1", "B"), PHOTO_REACH_FT).unwrap(), "W1-002/B");
    assert!((record.area("W1-002/B").unwrap().photos[0].along_ft - 3.0).abs() < 1e-9);
    // Far from every wall: refused, with the reason.
    let far = add_photo_at(&mut record, &t.plan_walls, photo("P4", [5.0, 40.0], "1", ""), PHOTO_REACH_FT).unwrap_err();
    assert!(far.contains("no wall face within"), "{far}");
    // Right position, wrong level: refused too.
    assert!(add_photo_at(&mut record, &t.plan_walls, photo("P5", [5.0, 14.5], "2", ""), PHOTO_REACH_FT).is_err());
    // It says face A but stands on the B side: refused, nothing filed.
    let wrong = add_photo_at(&mut record, &t.plan_walls, photo("P6", [5.0, 14.5], "1", "A"), PHOTO_REACH_FT).unwrap_err();
    assert!(wrong.contains("face A") && wrong.contains("W1-001/B"), "{wrong}");
    // Not a number: refused.
    assert!(add_photo_at(&mut record, &t.plan_walls, photo("P7", [f64::NAN, 14.5], "1", ""), PHOTO_REACH_FT).is_err());
    // The same photo twice is filed once.
    assert!(add_photo_at(&mut record, &t.plan_walls, photo("P1", [5.0, 14.5], "1", ""), PHOTO_REACH_FT).is_err());
    assert_eq!(record.progress().photos, 3);
    // The tap and the photo agree: tapping where P1 was taken shows P1.
    let (tapped, _) = resolve_wall_tap(&record, &t.plan_walls, [5.0, 14.5], "1", 2.0).unwrap();
    assert_eq!(record.area(&tapped).unwrap().photos[0].id, "P1");
}

/// The fixture with a second floor: the same walls and rooms on level 2.
fn two_level_truth() -> cadaddy_mepfix::truth::Truth {
    let mut t = test_truth();
    t.building.levels.push(cadaddy_mepfix::truth::Level { name: "2".into(), code: "2".into(), elevation_ft: 12.0 });
    let rooms: Vec<Room> = t
        .building
        .rooms
        .iter()
        .map(|r| Room { id: format!("{}-2", r.id), level: "2".into(), number: format!("2{}", &r.number[1..]), ..r.clone() })
        .collect();
    t.building.rooms.extend(rooms);
    let walls: Vec<PlanWall> = t
        .plan_walls
        .iter()
        .map(|w| PlanWall { id: w.id.replace("W1", "W2"), level: "2".into(), rooms: vec!["201".into(), "202".into()], ..w.clone() })
        .collect();
    t.plan_walls.extend(walls);
    t
}

#[test]
fn phases_are_signed_per_level() {
    let t = two_level_truth();
    let mut record = record_from_truth(&t, "Test Project");
    assert_eq!(record.phases.len(), STANDARD_PHASES.len() * 2, "one per phase and level");
    assert!(record.phases.iter().all(|p| p.levels.len() == 1));
    assert_eq!(record.area("W2-001/A").unwrap().rooms, vec!["202"]);
    // Every other phase signed on both floors; Rough-In Electrical on L1 only.
    for (id, name) in STANDARD_PHASES.iter().filter(|(id, _)| *id != "rough_elec") {
        assert_eq!(record.sign_phase(id, "1", "Inspector", "2026-10-21"), 1, "{name}");
        assert_eq!(record.sign_phase(name, "2", "Inspector", "2026-10-21"), 1, "{name}");
    }
    assert_eq!(record.sign_phase("Rough-In Electrical", "1", "Inspector", "2026-10-22"), 1);
    assert_eq!(record.sign_phase("rough_elec", "9", "Inspector", "2026-10-22"), 0, "no such level");
    let phases = record.phases.clone();
    for id in ["W1-001/A", "W2-001/A"] {
        let ids: Vec<String> = record.area(id).unwrap().checklist.iter().map(|c| c.id.clone()).collect();
        let area = record.area_mut(id).unwrap();
        for c in &ids {
            area.check_item(c, "Electrician", "2026-10-21");
        }
    }
    assert!(record.area_mut("W1-001/A").unwrap().mark_ready("Foreman", "2026-10-22", &phases), "L1 is signed");
    let l2 = record.area_mut("W2-001/A").unwrap();
    assert!(!l2.mark_ready("Foreman", "2026-10-22", &phases), "L1's signature doesn't release L2");
    let (items, unsigned) = l2.open_reasons(&phases);
    assert!(items.is_empty());
    assert_eq!(unsigned, vec!["Rough-In Electrical (2)"]);
    // Per level in the summary too.
    let s = summary_by_level(&record);
    assert_eq!((s[0].phases_signed, s[0].phases_total), (6, 6));
    assert_eq!((s[1].phases_signed, s[1].phases_total), (5, 6));
}

#[test]
fn validate_clears_ready_claimed_by_a_loaded_record() {
    let t = test_truth();
    let mut record = record_from_truth(&t, "Test Project");
    for ph in record.phases.iter_mut() {
        ph.sign("Inspector", "2026-10-21");
    }
    let ids: Vec<String> = record.area("W1-002/A").unwrap().checklist.iter().map(|c| c.id.clone()).collect();
    let phases = record.phases.clone();
    let good = record.area_mut("W1-002/A").unwrap();
    for c in &ids {
        good.check_item(c, "Electrician", "2026-10-21");
    }
    assert!(good.mark_ready("Foreman", "2026-10-21", &phases));
    // A hand-edited file: W1-001/A claims ready with its whole checklist open, and the
    // ceiling claims ready with one item open and its phase unsigned.
    let mut v = serde_json::to_value(&record).unwrap();
    for a in v["areas"].as_array_mut().unwrap() {
        if a["id"] == "W1-001/A" || a["id"] == "CLG-1-101" {
            a["ready_to_cover"] = serde_json::json!(true);
            a["ready_by"] = serde_json::json!("someone");
        }
        if a["id"] == "CLG-1-101" {
            for c in a["checklist"].as_array_mut().unwrap() {
                c["done"] = serde_json::json!(true);
            }
            a["checklist"][2]["done"] = serde_json::json!(false);
        }
    }
    v["phases"][0]["signed"] = serde_json::json!(false);
    let mut loaded: CoverRecord = serde_json::from_value(v).unwrap();
    let cleared = loaded.validate();
    let names: Vec<&str> = cleared.iter().map(|c| c.area.as_str()).collect();
    assert_eq!(names, vec!["W1-001/A", "W1-002/A", "CLG-1-101"], "{cleared:?}");
    assert_eq!(cleared[0].open_items.len(), STANDARD_CATEGORIES.len());
    assert_eq!(cleared[2].open_items, vec!["CLG-1-101-CK03: protection_plates"]);
    assert_eq!(cleared[1].unsigned_phases, vec!["Rough-In Electrical (1)"]);
    assert!(loaded.areas.iter().all(|a| !a.ready_to_cover && a.ready_by.is_none()));
    // A valid record keeps its ready areas.
    assert!(record.validate().is_empty());
    assert!(record.area("W1-002/A").unwrap().ready_to_cover);
}

#[test]
fn fixture_set_phases_apply_to_every_area() {
    // Generated sets name levels `LEVEL 1` with code `1`; walls and rooms carry the code.
    let t = cadaddy_mepfix::truth_for(42);
    let record = record_from_truth(&t, "Fixture Seed 42");
    assert_eq!(record.phases.len() % STANDARD_PHASES.len(), 0);
    for a in &record.areas {
        let n = record.phases.iter().filter(|p| p.applies_to(a)).count();
        assert_eq!(n, STANDARD_PHASES.len(), "area {} on level {} has {n} phases", a.id, a.level);
    }
    // Signing one level's electrical rough-in signs exactly that level's.
    let mut record = record;
    let level = record.areas[0].level.clone();
    assert_eq!(record.sign_phase("Rough-In Electrical", &level, "Inspector", "2026-10-21"), 1);
}
