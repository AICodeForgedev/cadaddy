//! The field viewer's package for the whole synthetic hotel (Q312): every room, wall, run,
//! terminal, fixture, device, column and the planted clashes and print gaps, loaded within the
//! budget a tablet or phone can take, then searched, picked and annotated the way the viewer does.

use std::collections::BTreeSet;
use std::time::Instant;

use cadaddy_mepfix::fieldpkg::field_package;
use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_walk3d::field::{FieldElement, FieldNote, FieldNotes, FieldPackage, FieldRoom, MatchRank, NoteKind, merge_notes};

/// Package size a phone downloads and keeps offline.
const BUDGET_BYTES: usize = 4 << 20;
/// Reading and indexing it (a debug build; a release build in the browser is several times faster).
const BUDGET_LOAD_MS: u128 = 1_500;

#[test]
fn the_full_hotel_loads_within_budget_and_works_offline() {
    let t = cadaddy_mepfix::hotel_truth_for(7, HotelSize::Full);
    let built = field_package(&t);
    let text = built.to_json().unwrap();
    assert!(text.len() <= BUDGET_BYTES, "package {} bytes > budget {BUDGET_BYTES}", text.len());
    let started = Instant::now();
    let mut p = FieldPackage::from_json(text.as_bytes()).unwrap();
    p.assign_rooms();
    let load_ms = started.elapsed().as_millis();
    eprintln!(
        "hotel package: {} KB, {} levels, {} rooms, {} elements, {} types, {} issues, loaded in {load_ms} ms",
        text.len() >> 10,
        p.levels.len(),
        p.rooms.len(),
        p.elements.len(),
        p.types.len(),
        p.issues.len()
    );
    assert!(load_ms <= BUDGET_LOAD_MS, "loading took {load_ms} ms > {BUDGET_LOAD_MS} ms");
    // Everything came across.
    assert_eq!(p.rooms.len(), t.building.rooms.len());
    assert!(p.levels.len() >= 4);
    assert!(p.elements.len() > 1_000, "{}", p.elements.len());
    for trade in ["arch", "mech", "plumb", "elec"] {
        assert!(p.trades.iter().any(|x| x.key == trade), "{trade} missing: {:?}", p.trade_counts());
    }

    // Room number search: every guest room is found first by its own number.
    let guest: Vec<&FieldRoom> = p.rooms.iter().filter(|r| !r.unit.is_empty()).collect();
    assert_eq!(guest.len(), t.unit_instances.len(), "every unit instance names its room");
    for r in &guest {
        let hits = p.search_rooms(&r.number);
        let first = hits.first().and_then(|(i, _)| p.rooms.get(*i));
        assert_eq!(first.map(|x| (&x.number, &x.level)), Some((&r.number, &r.level)), "search {}", r.number);
        assert_eq!(hits.first().map(|h| h.1), Some(MatchRank::Number));
    }
    // A room's view: its devices and fixtures, each with its type; hiding a trade hides them.
    let room =
        guest.iter().copied().find(|r| p.elements.iter().any(|e| e.room == r.number && e.trade == "elec" && !e.mark.is_empty())).unwrap().clone();
    let room = &room;
    let none = BTreeSet::new();
    let started = Instant::now();
    let inside: Vec<usize> = p.elements_in_room(room, &none).collect();
    let picked = inside.iter().filter_map(|i| p.elements.get(*i)).find(|e| e.trade == "elec" && !e.mark.is_empty()).unwrap().clone();
    let hit = p.pick_plan(&room.level, [(picked.min[0] + picked.max[0]) * 0.5, (picked.min[1] + picked.max[1]) * 0.5], 0.05, &none);
    let room_ms = started.elapsed().as_millis();
    assert!(room_ms <= 250, "a room view took {room_ms} ms");
    let hit = hit.and_then(|i| p.elements.get(i)).unwrap();
    assert_eq!(hit.trade, "elec", "{hit:?}");
    assert!(p.type_of(hit).is_some_and(|ty| ty.fields.iter().any(|f| f.key == "type_mark" && !f.source.is_empty())));
    let elec_hidden: BTreeSet<String> = ["elec".to_string()].into();
    assert!(p.elements_in_room(room, &elec_hidden).all(|i| p.elements.get(i).is_some_and(|e| e.trade != "elec")));
    assert!(p.elements_in_room(room, &elec_hidden).count() < inside.len());
    // Clashes and gaps land in rooms; a room with one lists it as open.
    let with_issue = p.rooms.iter().find(|r| p.open_issues(r).next().is_some());
    assert!(with_issue.is_some(), "{} issues, none in a room", p.issues.len());

    // Offline: notes made with no connection are kept in the package the device stores, come
    // back with it, and sync into the drawing's notes once, by id.
    let at = [room.at[0], room.at[1], p.level(&room.level).map_or(0.0, |l| l.elevation_ft) + 1.5];
    let id = p.add_note(
        "tab1",
        FieldNote { kind: NoteKind::VerifiedPoint, level: room.level.clone(), at, text: "box at 18\" AFF verified".into(), ..FieldNote::default() },
    );
    assert_eq!(p.notes.last().map(|n| n.room.clone()), Some(room.number.clone()));
    let stored = p.to_json().unwrap();
    let mut reopened = FieldPackage::from_json(stored.as_bytes()).unwrap();
    assert_eq!(reopened.unsynced().first().map(|n| Some(n.id.clone())), Some(id));
    let file = serde_json::to_vec(&FieldNotes::new(&reopened.project, reopened.unsynced())).unwrap();
    let mut drawing = Vec::new();
    let notes = FieldNotes::from_json(&file).unwrap();
    assert_eq!(merge_notes(&mut drawing, &notes.notes).added, 1);
    assert_eq!(merge_notes(&mut drawing, &notes.notes).added, 0);
    let ids: Vec<String> = reopened.unsynced().iter().map(|n| n.id.clone()).collect();
    reopened.mark_synced(&ids);
    assert!(reopened.unsynced().is_empty());
}

#[test]
fn a_hotel_twenty_times_larger_still_loads_within_budget() {
    let t = cadaddy_mepfix::hotel_truth_for(11, HotelSize::Full);
    let mut p = field_package(&t);
    let base = p.elements.clone();
    for k in 1..20 {
        p.elements.extend(base.iter().map(|e| FieldElement { id: format!("{}~{k}", e.id), ..e.clone() }));
    }
    let text = p.to_json().unwrap();
    assert!(text.len() <= 5 * BUDGET_BYTES, "{} bytes", text.len());
    let started = Instant::now();
    let mut back = FieldPackage::from_json(text.as_bytes()).unwrap();
    back.assign_rooms();
    let ms = started.elapsed().as_millis();
    eprintln!("20x hotel: {} KB, {} elements, loaded in {ms} ms", text.len() >> 10, back.elements.len());
    assert_eq!(back.elements.len(), base.len() * 20);
    assert!(ms <= 5 * BUDGET_LOAD_MS, "{ms} ms");
}
