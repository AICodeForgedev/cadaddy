//! Tests for pre-cover hold points and in-wall photos (Q334).

use cadaddy_mepfix::covercheck::*;
use cadaddy_mepfix::truth::{PlanWall, Room, Ceiling};

/// A minimal truth with two walls and one room with a ceiling, for testing.
fn test_truth() -> cadaddy_mepfix::truth::Truth {
    let mut t = cadaddy_mepfix::truth::Truth::default();
    t.building.levels.push(cadaddy_mepfix::truth::Level {
        name: "1".into(),
        code: "1".into(),
        elevation_ft: 0.0,
    });
    t.building.rooms.push(Room {
        id: "R1".into(),
        level: "1".into(),
        number: "101".into(),
        name: "OFFICE".into(),
        min_ft: [0.0, 0.0],
        max_ft: [20.0, 15.0],
        ceiling: Some(Ceiling {
            height_ft: 9.0,
            kind: "ACT".into(),
            ..Ceiling::default()
        }),
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
            cadaddy_mepfix::truth::WallTypeLayer {
                material: "gyp".into(),
                thickness_in: 0.625,
                side: "1".into(),
                ..Default::default()
            },
            cadaddy_mepfix::truth::WallTypeLayer {
                material: "stud".into(),
                thickness_in: 3.625,
                side: "core".into(),
                spacing_in: Some(16.0),
                ..Default::default()
            },
            cadaddy_mepfix::truth::WallTypeLayer {
                material: "gyp".into(),
                thickness_in: 0.625,
                side: "2".into(),
                ..Default::default()
            },
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
    // Add a photo to face A of W1-001.
    let photo = InWallPhoto {
        id: "P001".into(),
        face: "A".into(),
        elevation_ft: 4.0,
        along_ft: 5.0,
        uri: "photos/P001.jpg".into(),
        author: "Foreman".into(),
        date: "2026-10-21".into(),
        caption: "Boxes installed".into(),
        at_ft: [5.0, 14.5],
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
    for (face, along, y) in [("A", 3.0, 14.0), ("B", 3.0, 16.0)] {
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
    assert_eq!(
        record.areas.len(),
        wall_count + ceiling_count,
        "wall faces={wall_count}, ceilings={ceiling_count}, areas={}",
        record.areas.len()
    );
    // Every area has a non-empty checklist.
    for a in &record.areas {
        assert!(!a.checklist.is_empty(), "area {} has empty checklist", a.id);
        assert!(!a.ready_to_cover);
    }
    // Phases exist.
    assert!(!record.phases.is_empty());
}
