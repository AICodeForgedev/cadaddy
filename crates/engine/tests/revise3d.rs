//! model.revise: the plan compared with the one the model was built from (or another version of
//! it), the model rebuilt, hand edits kept where they don't conflict and listed where they do.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::wallgen::made_as;
use cadaddy_engine::doc::{Drawing, Entity, EntityKind, Handle, Line};
use cadaddy_engine::geom::Vec3;
use serde_json::{Value, json};

/// Feet → a line in inches on A-WALL.
fn line(d: &mut Drawing, a: (f64, f64), b: (f64, f64)) {
    let p = |(x, y): (f64, f64)| Vec3 { x: x * 12.0, y: y * 12.0, z: 0.0 };
    let h = Handle(d.handseed.max(0x100));
    d.handseed = h.0 + 1;
    let mut e = Entity::new(h, EntityKind::Line(Line { a: p(a), b: p(b) }));
    e.common.layer = "A-WALL".into();
    d.model.push(e);
}

fn rules() -> Value {
    json!({
        "units_per_foot": 12,
        "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 9.5, "ceiling_ft": 8, "base": [0, 0]}],
        "layers": [{"layer": "A-WALL", "kind": "wall"}]
    })
}

/// A 20' × 10' room of 6" walls, built (not associative unless `auto`).
fn built(auto: bool) -> Session {
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))] {
        line(&mut d, a, b);
    }
    for (a, b) in [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))] {
        line(&mut d, a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    s.execute("wallgen", &json!({"rules": rules(), "auto": auto})).unwrap();
    s
}

/// The plan lines on the east side (x > 200").
fn east(s: &Session) -> Vec<String> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).is_none() && matches!(&e.kind, EntityKind::Line(l) if l.a.x > 200.0 && l.b.x > 200.0))
        .map(|e| e.handle.hex())
        .collect()
}

/// The generated wall whose vertices all lie south (y ≤ 6"), and the one east (x ≥ 234").
fn wall(s: &Session, south: bool) -> Handle {
    s.doc()
        .unwrap()
        .model
        .iter()
        .find(|e| {
            made_as(e).as_deref() == Some("wall")
                && matches!(&e.kind, EntityKind::Mesh(m) if m.vertices.iter().all(|v| if south { v.y <= 6.0 + 1e-6 } else { v.x >= 234.0 - 1e-6 }))
        })
        .map(|e| e.handle)
        .expect("that wall")
}

fn recolor(s: &mut Session, h: Handle, aci: u8) {
    s.doc_mut().unwrap().modify_entity(h, |e| e.common.color = cadaddy_doc::color::Color::Index(aci)).unwrap();
}

#[test]
fn an_unchanged_plan_changes_nothing_and_a_moved_wall_is_reported_and_rebuilt() {
    let mut s = built(false);
    let r = s.execute("model.revise", &json!({})).unwrap();
    assert_eq!(
        (r["added"].as_array().unwrap().len(), r["removed"].as_array().unwrap().len(), r["moved"].as_array().unwrap().len()),
        (0, 0, 0),
        "{r}"
    );
    assert_eq!(r["rebuilt"], false);
    assert!(r["unchanged"].as_u64().unwrap() >= 8);
    // Move the east wall 2' further east.
    s.execute("move", &json!({"handles": east(&s), "from": [0, 0], "to": [24, 0]})).unwrap();
    let r = s.execute("model.revise", &json!({})).unwrap();
    let moved = r["moved"].as_array().unwrap();
    assert_eq!(moved.len(), 2, "both east wall lines: {r}");
    assert!(moved.iter().all(|m| m["kind"] == "wall" && m["at"][0].as_f64().unwrap() > m["from"][0].as_f64().unwrap()));
    assert_eq!(r["rebuilt"], true);
    // The rebuilt east wall reaches the new line, and is selected to show it.
    let changed = r["changed"].as_array().unwrap();
    assert!(!changed.is_empty());
    assert_eq!(s.selection().len(), changed.len());
    let max_x = s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).flat_map(|e| match &e.kind {
        EntityKind::Mesh(m) => m.vertices.iter().map(|v| v.x).collect::<Vec<_>>(),
        _ => vec![],
    });
    assert!(max_x.fold(f64::MIN, f64::max) >= 264.0 - 1e-6);
    // Revised again: nothing more.
    let r = s.execute("model.revise", &json!({})).unwrap();
    assert_eq!(r["rebuilt"], false, "{r}");
}

#[test]
fn hand_edits_stay_unless_their_plan_objects_changed() {
    let mut s = built(false);
    // Recolour the south wall (its plan lines won't change) and the east wall (they will).
    let (south, eastw) = (wall(&s, true), wall(&s, false));
    recolor(&mut s, south, 1);
    recolor(&mut s, eastw, 3);
    s.execute("move", &json!({"handles": east(&s), "from": [0, 0], "to": [24, 0]})).unwrap();
    let r = s.execute("model.revise", &json!({})).unwrap();
    let kept: Vec<&str> = r["kept"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert_eq!(kept, vec![south.hex().as_str()], "{r}");
    let e = s.doc().unwrap().entity(south).unwrap().clone();
    assert_eq!(e.common.color, cadaddy_doc::color::Color::Index(1), "the edit is kept");
    // Only one south wall: the rebuilt twin gave way.
    let souths = s
        .doc()
        .unwrap()
        .model
        .iter()
        .filter(|x| made_as(x).as_deref() == Some("wall") && matches!(&x.kind, EntityKind::Mesh(m) if m.vertices.iter().all(|v| v.y <= 6.0 + 1e-6)))
        .count();
    assert_eq!(souths, 1);
    let conflicts = r["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1, "{r}");
    assert_eq!(conflicts[0]["change"], "edited");
}

#[test]
fn a_part_deleted_by_hand_stays_deleted() {
    let mut s = built(false);
    let south = wall(&s, true);
    let walls = |s: &Session| s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).count();
    let before = walls(&s);
    s.doc_mut().unwrap().remove_entity(south);
    s.execute("move", &json!({"handles": east(&s), "from": [0, 0], "to": [24, 0]})).unwrap();
    s.execute("model.revise", &json!({})).unwrap();
    assert_eq!(walls(&s), before - 1, "the deleted south wall isn't rebuilt");
}

#[test]
fn another_version_of_the_plan_and_associative_rebuilds() {
    // Against a saved earlier version: the line added since shows as added.
    let mut s = built(false);
    let dir = std::env::temp_dir().join(format!("cadaddy-revise-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let earlier = dir.join("earlier.dxf");
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    std::fs::write(&earlier, cadaddy_io::write(s.doc().unwrap(), "x.dxf").unwrap()).unwrap();
    line(s.doc_mut().unwrap(), (5.0, 0.5), (5.0, 9.5));
    line(s.doc_mut().unwrap(), (5.5, 0.5), (5.5, 9.5));
    let r = s.execute("model.revise", &json!({"against": earlier.display().to_string(), "rebuild": false})).unwrap();
    assert_eq!(r["added"].as_array().unwrap().len(), 2, "{r}");
    assert_eq!(r["moved"].as_array().unwrap().len(), 0);
    assert!(s.execute("model.revise", &json!({"against": dir.join("none.dxf").display().to_string()})).is_err());
    // Associative: a plan edit rebuilds at once, keeping a hand edit elsewhere.
    let mut a = built(true);
    let south = wall(&a, true);
    recolor(&mut a, south, 5);
    a.execute("move", &json!({"handles": east(&a), "from": [0, 0], "to": [24, 0]})).unwrap();
    assert_eq!(a.doc().unwrap().entity(south).map(|e| e.common.color), Some(cadaddy_doc::color::Color::Index(5)), "kept through the rebuild");
    // No model yet: an error.
    let mut n = Session::new();
    assert!(n.execute("model.revise", &json!({})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- the rework (Q5168): every kind, rooms, the associative path, paging, handles ----------

/// Feet → a drawing point (inches).
fn ft(x: f64, y: f64) -> [f64; 2] {
    [x * 12.0, y * 12.0]
}

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        s.execute("layer.current", &json!({"name": name})).unwrap();
    }
}

/// The last object drawn.
fn last(s: &Session) -> Handle {
    s.doc().unwrap().model.iter().last().unwrap().handle
}

/// A door (`DOOR36`) or window (`WIN48`) block inserted at `at` (feet) on A-DOOR / A-GLAZ.
fn opening(s: &mut Session, name: &str, at: (f64, f64)) -> Handle {
    if !s.doc().unwrap().blocks.contains_key(name) {
        layer(s, "0");
        s.execute("line", &json!({"points": [[-18, 0], [18, 0]]})).unwrap();
        let h = last(s).hex();
        s.execute("block", &json!({"name": name, "base": [0, 0], "handles": [h], "keep": "delete"})).unwrap();
    }
    layer(s, if name.starts_with("DOOR") { "A-DOOR" } else { "A-GLAZ" });
    s.execute("insert", &json!({"name": name, "at": ft(at.0, at.1)})).unwrap();
    last(s)
}

/// A room: a closed outline `w` × `h` feet at `at` (its lower left) on A-AREA, its name inside.
/// Returns (outline, label).
fn room(s: &mut Session, name: &str, at: (f64, f64), w: f64, h: f64) -> (Handle, Handle) {
    layer(s, "A-AREA");
    let (x, y) = at;
    s.execute("pline", &json!({"vertices": [ft(x, y), ft(x + w, y), ft(x + w, y + h), ft(x, y + h)], "closed": true})).unwrap();
    let outline = last(s);
    layer(s, "A-ANNO");
    s.execute("text", &json!({"at": ft(x + w / 3.0, y + h / 2.0), "text": name, "height": 6})).unwrap();
    (outline, last(s))
}

/// The 20' × 10' room of [`built`], drawn by `setup` first, then built.
fn built_with(auto: bool, setup: impl FnOnce(&mut Session)) -> Session {
    let mut d = Drawing::new_imperial();
    for (a, b) in [((0., 0.), (20., 0.)), ((20., 0.), (20., 10.)), ((20., 10.), (0., 10.)), ((0., 10.), (0., 0.))] {
        line(&mut d, a, b);
    }
    for (a, b) in [((0.5, 0.5), (19.5, 0.5)), ((19.5, 0.5), (19.5, 9.5)), ((19.5, 9.5), (0.5, 9.5)), ((0.5, 9.5), (0.5, 0.5))] {
        line(&mut d, a, b);
    }
    let mut s = Session::empty();
    s.open_drawing(d, "Plan", None);
    setup(&mut s);
    s.execute("wallgen", &json!({"rules": rules(), "auto": auto})).unwrap();
    s
}

fn shift(s: &mut Session, handles: &[Handle], by: (f64, f64)) {
    let hs: Vec<String> = handles.iter().map(|h| h.hex()).collect();
    s.execute("move", &json!({"handles": hs, "from": [0, 0], "to": ft(by.0, by.1)})).unwrap();
}

fn erase(s: &mut Session, handles: &[Handle]) {
    let hs: Vec<String> = handles.iter().map(|h| h.hex()).collect();
    s.execute("erase", &json!({"handles": hs})).unwrap();
}

/// How many of each kind a report lists in `list`.
fn kinds(r: &Value, list: &str) -> std::collections::BTreeMap<String, usize> {
    let mut m = std::collections::BTreeMap::new();
    for x in r[list].as_array().unwrap() {
        *m.entry(x["kind"].as_str().unwrap().to_string()).or_insert(0) += 1;
    }
    m
}

fn counted(pairs: &[(&str, usize)]) -> std::collections::BTreeMap<String, usize> {
    pairs.iter().map(|(k, n)| (k.to_string(), *n)).collect()
}

/// The plan lines on the west side (x < 12").
fn west(s: &Session) -> Vec<Handle> {
    s.doc()
        .unwrap()
        .model
        .iter()
        .filter(|e| made_as(e).is_none() && matches!(&e.kind, EntityKind::Line(l) if l.a.x < 12.0 && l.b.x < 12.0))
        .map(|e| e.handle)
        .collect()
}

fn set_io() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

/// A scratch folder for one test.
fn scratch(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-revise-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn every_kind_added_moved_and_removed_is_counted_exactly_from_the_record_and_against_a_file() {
    set_io();
    let dir = scratch("kinds");
    // Doors at 5' and 15' on the south wall, windows at 3' and 17' on the north wall; an office
    // 10' × 10' east of the building.
    let mut ids = (Handle(0), Handle(0), Handle(0), (Handle(0), Handle(0)));
    let mut s = built_with(false, |s| {
        let d1 = opening(s, "DOOR36", (5.0, 0.25));
        let d2 = opening(s, "DOOR36", (15.0, 0.25));
        opening(s, "WIN48", (3.0, 9.75));
        let w2 = opening(s, "WIN48", (17.0, 9.75));
        ids = (d1, d2, w2, room(s, "OFFICE", (40.0, 0.0), 10.0, 10.0));
    });
    let (d1, d2, w2, (outline, label)) = ids;
    let r = s.execute("model.revise", &json!({"rebuild": false})).unwrap();
    assert_eq!((kinds(&r, "added").len(), kinds(&r, "removed").len(), kinds(&r, "moved").len()), (0, 0, 0), "{r}");
    let earlier = dir.join("earlier.dxf");
    std::fs::write(&earlier, cadaddy_io::write(s.doc().unwrap(), "x.dxf").unwrap()).unwrap();
    // The revision: a door added on the north wall, the 5' door moved 1' east, the 15' door
    // removed; a window added on the south wall, the 17' window removed; the west wall (both its
    // lines) removed; the office moved 1' north.
    opening(&mut s, "DOOR36", (8.0, 9.75));
    shift(&mut s, &[d1], (1.0, 0.0));
    erase(&mut s, &[d2, w2]);
    opening(&mut s, "WIN48", (11.0, 0.25));
    let wl = west(&s);
    assert_eq!(wl.len(), 2);
    erase(&mut s, &wl);
    shift(&mut s, &[outline, label], (0.0, 1.0));
    let expect = |r: &Value| {
        assert_eq!(kinds(r, "added"), counted(&[("door", 1), ("window", 1)]), "added: {r}");
        assert_eq!(kinds(r, "removed"), counted(&[("door", 1), ("wall", 2), ("window", 1)]), "removed: {r}");
        assert_eq!(kinds(r, "moved"), counted(&[("door", 1), ("room", 1)]), "moved: {r}");
        let room = r["moved"].as_array().unwrap().iter().find(|m| m["kind"] == "room").unwrap();
        assert_eq!(room["name"], "OFFICE");
        assert!((room["at"][1].as_f64().unwrap() - room["from"][1].as_f64().unwrap() - 12.0).abs() < 1e-6, "{room}");
        let door = r["moved"].as_array().unwrap().iter().find(|m| m["kind"] == "door").unwrap();
        assert!((door["at"][0].as_f64().unwrap() - door["from"][0].as_f64().unwrap() - 12.0).abs() < 1e-6, "{door}");
    };
    // Against the earlier file (no rebuild: the record stays as built).
    let r = s.execute("model.revise", &json!({"against": earlier.display().to_string(), "rebuild": false})).unwrap();
    expect(&r);
    assert_eq!(r["rebuilt"], false);
    // From the record the model was built from: the same changes, and the model rebuilt.
    let r = s.execute("model.revise", &json!({})).unwrap();
    expect(&r);
    assert_eq!(r["rebuilt"], true);
    assert_eq!(r["auto"], false);
    // Now current: nothing more (the room's new place is in the record too).
    let r = s.execute("model.revise", &json!({})).unwrap();
    assert_eq!((kinds(&r, "added").len(), kinds(&r, "removed").len(), kinds(&r, "moved").len()), (0, 0, 0), "{r}");
    assert_eq!(r["rebuilt"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_associative_rebuild_keeps_its_conflicts_for_model_revise() {
    let mut s = built(true);
    let (south, eastw) = (wall(&s, true), wall(&s, false));
    recolor(&mut s, eastw, 3);
    s.log.clear();
    // Moving the east wall's plan lines leaves the model behind; the update rebuilds it: the
    // recoloured east wall conflicts (its plan lines changed) and the rebuilt wall replaces it.
    s.execute("move", &json!({"handles": east(&s), "from": [0, 0], "to": [24, 0]})).unwrap();
    s.execute("model.update", &json!({})).unwrap();
    assert!(s.log.iter().any(|l| l.contains("2 moved") && l.contains("1 conflict(s)")), "{:?}", s.log);
    let r = s.execute("model.revise", &json!({"last": true})).unwrap();
    assert_eq!(r["auto"], true, "{r}");
    assert_eq!(r["rebuilt"], true);
    assert_eq!(kinds(&r, "moved"), counted(&[("wall", 2)]), "{r}");
    let conflicts = r["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1, "{r}");
    assert_eq!(conflicts[0]["handle"], eastw.hex());
    assert_eq!(conflicts[0]["part"], "wall");
    assert_eq!(conflicts[0]["change"], "edited");
    assert_eq!(r["totals"]["conflicts"], 1);
    // The rebuilt parts of the moved lines are selected, as model.revise selects them.
    let changed: Vec<&str> = r["changed"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(!changed.is_empty());
    let mut sel: Vec<String> = s.selection().iter().map(|h| h.hex()).collect();
    sel.sort();
    let mut ch: Vec<String> = changed.iter().map(|h| h.to_string()).collect();
    ch.sort();
    assert_eq!(sel, ch);
    // The untouched south wall kept its handle through the rebuild.
    assert_eq!(wall(&s, true), south);
    // A plain model.revise finds the model current and still points at that revision.
    let r = s.execute("model.revise", &json!({})).unwrap();
    assert_eq!(r["rebuilt"], false, "{r}");
    assert_eq!(r["last"]["totals"]["conflicts"], 1, "{r}");
    assert_eq!(r["last"]["auto"], true);
}

#[test]
fn rooms_past_the_first_two_hundred_are_compared() {
    // 260 offices, 10' × 10' on a 15' grid east of the building; the 250th moves 1' east.
    let mut rooms = Vec::new();
    let mut s = built_with(false, |s| {
        for i in 0..260 {
            let (c, r) = (f64::from(i % 20), f64::from(i / 20));
            rooms.push(room(s, "OFFICE", (40.0 + c * 15.0, r * 15.0), 10.0, 10.0));
        }
    });
    let r = s.execute("model.revise", &json!({"rebuild": false})).unwrap();
    assert_eq!((kinds(&r, "added").len(), kinds(&r, "removed").len(), kinds(&r, "moved").len()), (0, 0, 0), "{r}");
    assert!(r["unchanged"].as_u64().unwrap() >= 8 + 260, "every room compared: {}", r["unchanged"]);
    let (outline, label) = rooms[249];
    shift(&mut s, &[outline, label], (1.0, 0.0));
    let r = s.execute("model.revise", &json!({"rebuild": false})).unwrap();
    assert_eq!(kinds(&r, "moved"), counted(&[("room", 1)]), "{r}");
    assert_eq!((kinds(&r, "added").len(), kinds(&r, "removed").len()), (0, 0), "{r}");
    let m = &r["moved"][0];
    // Room 250 (index 249): column 9, row 12; its label's room centre moved 12" east.
    let (cx, cy) = ((40.0 + 9.0 * 15.0 + 5.0) * 12.0, (12.0 * 15.0 + 5.0) * 12.0);
    assert!((m["from"][0].as_f64().unwrap() - cx).abs() < 24.0 && (m["from"][1].as_f64().unwrap() - cy).abs() < 24.0, "{m}");
    assert!((m["at"][0].as_f64().unwrap() - m["from"][0].as_f64().unwrap() - 12.0).abs() < 1e-6, "{m}");
}

#[test]
fn rooms_moved_added_and_removed_between_two_files() {
    set_io();
    let dir = scratch("rooms");
    let mut ids = Vec::new();
    let mut s = built_with(false, |s| {
        ids.push(room(s, "OFFICE", (40.0, 0.0), 10.0, 10.0));
        ids.push(room(s, "LOBBY", (55.0, 0.0), 10.0, 10.0));
        ids.push(room(s, "STORAGE", (70.0, 0.0), 10.0, 10.0));
    });
    let earlier = dir.join("earlier.dxf");
    std::fs::write(&earlier, cadaddy_io::write(s.doc().unwrap(), "x.dxf").unwrap()).unwrap();
    // OFFICE moved 1' north, LOBBY removed, a KITCHEN added; STORAGE as it was.
    let (o, l) = ids[0];
    shift(&mut s, &[o, l], (0.0, 1.0));
    let (o, l) = ids[1];
    erase(&mut s, &[o, l]);
    room(&mut s, "KITCHEN", (40.0, 20.0), 10.0, 10.0);
    let r = s.execute("model.revise", &json!({"against": earlier.display().to_string(), "rebuild": false})).unwrap();
    let names =
        |list: &str| r[list].as_array().unwrap().iter().map(|x| (x["kind"].as_str().unwrap().to_string(), x["name"].clone())).collect::<Vec<_>>();
    assert_eq!(names("moved"), vec![("room".to_string(), json!("OFFICE"))], "{r}");
    assert_eq!(names("removed"), vec![("room".to_string(), json!("LOBBY"))], "{r}");
    assert_eq!(names("added"), vec![("room".to_string(), json!("KITCHEN"))], "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_thousand_moved_items_come_back_a_page_at_a_time() {
    let mut doors = Vec::new();
    let mut s = built_with(false, |s| {
        for i in 0..1000 {
            let (c, r) = (f64::from(i % 40), f64::from(i / 40));
            doors.push(opening(s, "DOOR36", (40.0 + c * 5.0, r * 5.0)));
        }
    });
    shift(&mut s, &doors, (0.5, 0.0));
    let r = s.execute("model.revise", &json!({"rebuild": false, "limit": 100})).unwrap();
    assert_eq!(r["totals"]["moved"], 1000, "{}", r["totals"]);
    assert_eq!((r["totals"]["added"].as_u64(), r["totals"]["removed"].as_u64()), (Some(0), Some(0)));
    assert_eq!(r["moved"].as_array().unwrap().len(), 100);
    assert_eq!(r["next"], 100);
    // Every page, from the stored report, until there is no next: each item once.
    let mut seen = std::collections::HashSet::new();
    let mut offset = Some(0u64);
    let mut pages = 0;
    while let Some(o) = offset {
        let p = s.execute("model.revise", &json!({"last": true, "offset": o, "limit": 100})).unwrap();
        let list = p["moved"].as_array().unwrap();
        assert!(list.len() <= 100);
        assert_eq!(p["totals"]["moved"], 1000);
        for m in list {
            assert!(seen.insert(m["handle"].as_str().unwrap().to_string()), "{m}");
        }
        offset = p["next"].as_u64();
        pages += 1;
    }
    assert_eq!((pages, seen.len()), (10, 1000));
    // The default page and the cap.
    let r = s.execute("model.revise", &json!({"last": true})).unwrap();
    assert_eq!((r["moved"].as_array().unwrap().len(), r["limit"].as_u64()), (100, Some(100)));
    let r = s.execute("model.revise", &json!({"last": true, "limit": 100_000})).unwrap();
    assert_eq!((r["moved"].as_array().unwrap().len(), r["limit"].as_u64()), (500, Some(500)));
    // Compared again (no rebuild yet): the same totals, a later page.
    let r = s.execute("model.revise", &json!({"rebuild": false, "offset": 950, "limit": 100})).unwrap();
    assert_eq!((r["moved"].as_array().unwrap().len(), r["totals"]["moved"].as_u64(), r.get("next")), (50, Some(1000), None));
}

#[test]
fn untouched_walls_keep_their_handles_when_another_wall_moves() {
    let mut s = built(false);
    let made = |s: &Session| {
        s.doc().unwrap().model.iter().filter(|e| made_as(e).as_deref() == Some("wall")).map(|e| e.handle).collect::<std::collections::BTreeSet<_>>()
    };
    let south = wall(&s, true);
    let eastw = wall(&s, false);
    let before = made(&s);
    s.execute("move", &json!({"handles": east(&s), "from": [0, 0], "to": [24, 0]})).unwrap();
    let r = s.execute("model.revise", &json!({})).unwrap();
    assert_eq!(r["rebuilt"], true);
    assert_eq!(wall(&s, true), south, "the south wall's handle survives the rebuild");
    // Every wall but the moved east one keeps its handle; the east wall is rebuilt anew.
    let after = made(&s);
    let kept: Vec<_> = before.intersection(&after).collect();
    assert_eq!(kept.len(), before.len() - 1, "before {before:?}, after {after:?}");
    assert!(!after.contains(&eastw));
    assert_eq!(after.len(), before.len());
}
