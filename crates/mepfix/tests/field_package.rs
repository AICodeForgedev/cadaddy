//! The field viewer's package for the whole synthetic hotel (Q312): every room, wall, run,
//! terminal, fixture, device, column and the planted clashes and print gaps, loaded within the
//! budget a tablet or phone can take, then searched, picked and annotated the way the viewer does.

use std::collections::BTreeSet;
use std::time::Instant;

use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::truth::Truth;
use cadaddy_walk3d::field::{
    FieldElement, FieldIssue, FieldLevel, FieldNote, FieldNotes, FieldPackage, FieldRoom, FieldSpec, FieldType, IssueKind, MatchRank, NoteKind,
    merge_notes, trade_info, trade_of,
};

/// Package size a phone downloads and keeps offline.
const BUDGET_BYTES: usize = 4 << 20;
/// Reading and indexing it (a debug build; a release build in the browser is several times faster).
const BUDGET_LOAD_MS: u128 = 1_500;

fn bx(id: &str, trade: &str, kind: &str, level: &str, min: [f64; 3], max: [f64; 3]) -> FieldElement {
    FieldElement { id: id.into(), trade: trade.into(), kind: kind.into(), level: level.into(), min, max, ..FieldElement::default() }
}

fn around(at: [f64; 3], half: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    ([at[0] - half[0], at[1] - half[1], at[2] - half[2]], [at[0] + half[0], at[1] + half[1], at[2] + half[2]])
}

/// The hotel's model as `fieldview.export` lays it out (made from the truth here).
fn package(t: &Truth) -> FieldPackage {
    let mut p = FieldPackage::new(&t.building.name);
    let lv = &t.building.levels;
    let elev = |name: &str| lv.iter().find(|l| l.name == name || l.code == name).map_or(0.0, |l| l.elevation_ft);
    let name_of = |name: &str| lv.iter().find(|l| l.name == name || l.code == name).map_or(name.to_string(), |l| l.name.clone());
    for (i, l) in lv.iter().enumerate() {
        let next = lv.get(i + 1).map_or(l.elevation_ft + 12.0, |n| n.elevation_ft);
        p.levels.push(FieldLevel { name: l.name.clone(), elevation_ft: l.elevation_ft, height_ft: next - l.elevation_ft });
    }
    let height = |name: &str| p.levels.iter().find(|l| l.name == name_of(name)).map_or(10.0, |l| l.height_ft);
    let units: std::collections::BTreeMap<(String, String), String> = t
        .unit_instances
        .iter()
        .map(|u| {
            let ty = t.unit_types.iter().find(|x| x.id == u.unit_type).map_or(u.unit_type.clone(), |x| x.name.clone());
            ((name_of(&u.level), u.number.clone()), format!("{ty} ({})", u.hand))
        })
        .collect();
    for r in &t.building.rooms {
        let (a, b) = (r.min_ft, r.max_ft);
        let level = name_of(&r.level);
        p.rooms.push(FieldRoom {
            number: r.number.clone(),
            name: r.name.clone(),
            unit: units.get(&(level.clone(), r.number.clone())).cloned().unwrap_or_default(),
            level,
            outline: vec![[a[0], a[1]], [b[0], a[1]], [b[0], b[1]], [a[0], b[1]]],
            at: [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5],
        });
    }
    for w in &t.building.walls {
        let z = elev(&w.level);
        let h = w.thickness_in / 24.0;
        let (f, to) = (w.from_ft, w.to_ft);
        let mut e = bx(
            &w.id,
            "arch",
            "wall",
            &name_of(&w.level),
            [f[0].min(to[0]) - h, f[1].min(to[1]) - h, z],
            [f[0].max(to[0]) + h, f[1].max(to[1]) + h, z + height(&w.level)],
        );
        let hh = height(&w.level);
        e.path = vec![[f[0], f[1], z + hh * 0.5], [to[0], to[1], z + hh * 0.5]];
        e.size_in = Some([w.thickness_in, hh * 12.0]);
        e.source = format!("{}\" stud wall", w.thickness_in);
        p.elements.push(e);
    }
    for c in &t.building.columns {
        let (min, max) = around([c.at_ft[0], c.at_ft[1], 0.0], [c.footprint_in[0] / 24.0, c.footprint_in[1] / 24.0, 0.0]);
        let level = lv.iter().rev().find(|l| l.elevation_ft <= c.bottom_ft + 1e-6).map_or(String::new(), |l| l.name.clone());
        p.elements.push(bx(&c.id, "struct", "column", &level, [min[0], min[1], c.bottom_ft], [max[0], max[1], c.top_ft]));
    }
    let kinds: std::collections::BTreeMap<&str, &str> = t.systems.iter().map(|s| (s.name.as_str(), s.kind.as_str())).collect();
    for s in &t.segments {
        let [a, b] = s.path_ft;
        let kind = kinds.get(s.system.as_str()).copied().unwrap_or(s.kind.as_str());
        let kind = if kind.contains("duct") || kind == "supply" || kind == "return" || kind == "exhaust" { "duct" } else { "pipe" };
        let r = s.size_in[0].max(s.size_in[1]) / 24.0;
        let mut e = bx(
            &s.id,
            trade_of(kind, &s.system),
            kind,
            &name_of(&s.level),
            [a[0].min(b[0]) - r, a[1].min(b[1]) - r, a[2].min(b[2]) - r],
            [a[0].max(b[0]) + r, a[1].max(b[1]) + r, a[2].max(b[2]) + r],
        );
        e.path = vec![a, b];
        e.size_in = Some(s.size_in);
        e.source = format!("{} {}", s.sheet, s.system);
        p.elements.push(e);
    }
    for c in &t.elec_conduits {
        let z = elev(&c.level) + 9.5;
        let (f, to) = (c.from_ft, c.to_ft);
        let r = c.size_in / 24.0;
        let mut e = bx(
            &c.id,
            "elec",
            "conduit",
            &name_of(&c.level),
            [f[0].min(to[0]) - r, f[1].min(to[1]) - r, z - r],
            [f[0].max(to[0]) + r, f[1].max(to[1]) + r, z + r],
        );
        e.path = vec![[f[0], f[1], z], [to[0], to[1], z]];
        e.size_in = Some([c.size_in, 0.0]);
        e.mark = c.conduit_type.clone();
        e.source = c.sheet.clone();
        p.elements.push(e);
    }
    for x in &t.terminals {
        let (min, max) = around(x.at_ft, [x.face_in[0] / 24.0, x.face_in[1] / 24.0, 0.25]);
        let mut e = bx(&x.id, "mech", "terminal", &name_of(&x.level), min, max);
        e.room = x.room.clone();
        e.mark = x.type_tag.clone();
        p.elements.push(e);
    }
    for x in &t.equipment {
        let mut e = bx(&x.id, trade_of("equipment", &x.kind), "equipment", &name_of(&x.level), x.min_ft, x.max_ft);
        e.source = x.tag.clone();
        p.elements.push(e);
    }
    for x in &t.fixtures {
        let (min, max) = around(x.at_ft, [1.0, 1.0, 1.25]);
        let mut e = bx(&x.id, "plumb", "fixture", &name_of(&x.level), min, max);
        e.mark = x.tag.clone();
        p.elements.push(e);
    }
    for x in &t.elec_devices {
        let (min, max) = around(x.at_ft, [0.2, 0.2, 0.2]);
        let mut e = bx(&x.id, "elec", "device", &name_of(&x.level), min, max);
        e.mark = x.type_tag.clone();
        e.source = format!("{} circuit {}-{}", x.sheet, x.panel, x.circuit);
        p.elements.push(e);
    }
    for x in &t.fire_alarm_devices {
        let (min, max) = around(x.at_ft, [0.25, 0.25, 0.25]);
        let mut e = bx(&x.id, "fire", "device", &name_of(&x.level), min, max);
        e.mark = x.tag.clone();
        p.elements.push(e);
    }
    for x in &t.casework {
        let z = elev(&x.level);
        let mut e =
            bx(&x.id, "arch", "casework", &name_of(&x.level), [x.min_ft[0], x.min_ft[1], z], [x.max_ft[0], x.max_ft[1], z + x.height_in / 12.0]);
        e.mark = x.tag.clone();
        p.elements.push(e);
    }
    // Types: the device type tags, with the spec items they point to as their source.
    let mut marks: BTreeSet<String> = p.elements.iter().filter(|e| !e.mark.is_empty()).map(|e| e.mark.clone()).collect();
    marks.retain(|m| m.len() <= 16);
    for m in marks {
        let spec = t.spec_items.iter().find(|s| s.referenced_by.iter().any(|r| r == &m));
        let mut fields = vec![FieldSpec { key: "type_mark".into(), value: m.clone(), source: "plans".into(), confidence: "stated".into() }];
        if let Some(s) = spec {
            fields.push(FieldSpec {
                key: "manufacturer".into(),
                value: s.manufacturer.clone(),
                source: format!("spec {}", s.section),
                confidence: "stated".into(),
            });
            fields.push(FieldSpec {
                key: "model".into(),
                value: s.model.clone(),
                source: format!("spec {}", s.section),
                confidence: "stated".into(),
            });
        }
        p.types.push(FieldType { mark: m, category: String::new(), fields });
    }
    for c in &t.clashes {
        let level = lv.iter().rev().find(|l| l.elevation_ft <= c.at_ft[2] + 1e-6).map_or(String::new(), |l| l.name.clone());
        p.issues.push(FieldIssue {
            id: c.id.clone(),
            kind: IssueKind::Clash,
            title: c.description.clone().unwrap_or_else(|| format!("{} vs {}", c.a, c.b)),
            status: "new".into(),
            level,
            room: String::new(),
            at: c.at_ft,
            elements: vec![c.a.clone(), c.b.clone()],
        });
    }
    for g in &t.gaps {
        let level = name_of(&g.level);
        let z = elev(&g.level);
        p.issues.push(FieldIssue {
            id: g.id.clone(),
            kind: IssueKind::Rfi,
            title: g.description.clone(),
            status: "open".into(),
            level,
            room: String::new(),
            at: [g.at_ft[0], g.at_ft[1], z],
            elements: g.elements.clone(),
        });
    }
    let counts = p.trade_counts();
    p.trades =
        ["arch", "struct", "mech", "plumb", "elec", "fire", "lowvolt"].iter().filter(|k| counts.contains_key(**k)).map(|k| trade_info(k)).collect();
    p
}

#[test]
fn the_full_hotel_loads_within_budget_and_works_offline() {
    let t = cadaddy_mepfix::hotel_truth_for(7, HotelSize::Full);
    let built = package(&t);
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
    let mut p = package(&t);
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
