//! Pre-cover hold points as commands (Q5119): a record made from a print set's truth.json,
//! items checked, an AHJ phase signed per level, `cover.ready` refused with what is open and then
//! accepted, in-wall photos filed by where they were taken, a wall tap showing the tapped face's
//! photos, a hand-edited record cleared on load, the record kept through a .cdy round trip, and
//! hostile parameters answered without a panic.

use std::path::PathBuf;

use cadaddy_engine::Session;
use cadaddy_mepfix::covercheck::STANDARD_CATEGORIES;
use cadaddy_mepfix::truth::{Ceiling, Level, PlanWall, Room, Truth};
use serde_json::{Value, json};

/// Two floors of an office (101 / 201, south) and a corridor (102 / 202, north) split by two
/// walls along y = 15; level codes `1` and `2`, title-block names `LEVEL 1` and `LEVEL 2`.
fn truth() -> Truth {
    let mut t = Truth::default();
    for (code, z) in [("1", 0.0), ("2", 12.0)] {
        t.building.levels.push(Level { name: format!("LEVEL {code}"), code: code.into(), elevation_ft: z });
        for (n, name, min, max, ceiling) in [("01", "OFFICE", [0.0, 0.0], [20.0, 15.0], true), ("02", "CORRIDOR", [0.0, 15.0], [20.0, 20.0], false)] {
            t.building.rooms.push(Room {
                id: format!("R{code}{n}"),
                level: code.into(),
                number: format!("{code}{n}"),
                name: name.into(),
                min_ft: min,
                max_ft: max,
                ceiling: ceiling.then(|| Ceiling { height_ft: 9.0, kind: "ACT".into(), ..Ceiling::default() }),
                plenum_ft: None,
            });
        }
        for (i, a, b) in [(1, [0.0, 15.0], [10.0, 15.0]), (2, [10.0, 15.0], [20.0, 15.0])] {
            t.plan_walls.push(PlanWall {
                id: format!("W{code}-00{i}"),
                level: code.into(),
                a_ft: a,
                b_ft: b,
                wall_type: "1A".into(),
                tagged: true,
                height_ft: 9.0,
                rooms: vec![format!("{code}01"), format!("{code}02")],
                openings: Vec::new(),
                tags: Vec::new(),
                layout_from: None,
                plan_tag: None,
                enlarged_tag: None,
            });
        }
    }
    t
}

/// The truth written to its own folder.
fn model(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-cover-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("truth.json"), serde_json::to_vec(&truth()).expect("json")).expect("written");
    dir
}

fn ex(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd} {p}: {e}"))
}

fn err(s: &mut Session, cmd: &str, p: Value) -> String {
    match s.execute(cmd, &p) {
        Ok(v) => panic!("{cmd} {p} should fail, gave {v}"),
        Err(e) => e.to_string(),
    }
}

fn started(name: &str) -> (Session, PathBuf) {
    let dir = model(name);
    let mut s = Session::new();
    let r = ex(&mut s, "cover.init", json!({ "model": dir.display().to_string(), "project": "Office" }));
    // 2 levels × (2 walls × 2 faces + 1 ceiling); 6 phases per level.
    assert_eq!(
        (r["areas"].as_u64(), r["wall_faces"].as_u64(), r["ceilings"].as_u64(), r["phases"].as_u64()),
        (Some(10), Some(8), Some(2), Some(12)),
        "{r}"
    );
    assert_eq!(r["levels"], json!(["1", "2"]));
    (s, dir)
}

fn check_all(s: &mut Session, area: &str) {
    for (cat, _) in STANDARD_CATEGORIES {
        ex(s, "cover.check", json!({ "area": area, "item": cat, "by": "Electrician", "date": "2026-10-21" }));
    }
}

#[test]
fn ready_is_refused_with_what_is_open_then_accepted() {
    let (mut s, dir) = started("ready");
    // Nothing done: refused, nothing changes, and the answer says what is open.
    let r = ex(&mut s, "cover.ready", json!({ "area": "W1-001/B", "by": "Foreman" }));
    assert_eq!(r["ready"], false);
    assert_eq!(r["open_items"].as_array().map(Vec::len), Some(STANDARD_CATEGORIES.len()));
    assert_eq!(r["unsigned_phases"].as_array().map(Vec::len), Some(6), "{r}");
    let reason = r["reason"].as_str().unwrap_or_default();
    assert!(reason.contains("7 of 7 items open") && reason.contains("Rough-In Electrical (1)"), "{reason}");
    // The face rooms follow the geometry: B (south of an eastward wall) is the office.
    let st = ex(&mut s, "cover.status", json!({ "level": "1", "kind": "wall" }));
    let face = |st: &Value, id: &str| st["areas"].as_array().and_then(|a| a.iter().find(|x| x["id"] == id).cloned()).unwrap_or(Value::Null);
    assert_eq!(face(&st, "W1-001/B")["rooms"], json!(["101"]));
    assert_eq!(face(&st, "W1-001/A")["rooms"], json!(["102"]));
    assert_eq!(st["total"], 4);
    // Every item done; still refused until the phases are signed.
    check_all(&mut s, "W1-001/B");
    let r = ex(&mut s, "cover.ready", json!({ "area": "W1-001/B" }));
    assert_eq!((r["ready"].as_bool(), r["open_items"].as_array().map(Vec::len)), (Some(false), Some(0)));
    // Signed per level: level 2's sign-off doesn't release level 1.
    for (id, _) in cadaddy_mepfix::covercheck::STANDARD_PHASES {
        let r = ex(&mut s, "cover.sign", json!({ "phase": id, "level": "2", "inspector": "AHJ", "date": "2026-10-22" }));
        assert_eq!(r["signed"].as_array().map(Vec::len), Some(1), "{r}");
    }
    assert_eq!(ex(&mut s, "cover.ready", json!({ "area": "W1-001/B" }))["ready"], false);
    for (id, name) in cadaddy_mepfix::covercheck::STANDARD_PHASES {
        let phase = if *id == "rough_elec" { *name } else { *id };
        ex(&mut s, "cover.sign", json!({ "phase": phase, "level": "1", "inspector": "AHJ", "permit": "E-1234" }));
    }
    let r = ex(&mut s, "cover.ready", json!({ "area": "W1-001/B", "by": "Foreman", "date": "2026-10-23" }));
    assert_eq!(r["ready"], true, "{r}");
    let st = ex(&mut s, "cover.status", json!({ "level": "1", "ready": true }));
    assert_eq!(st["total"], 1);
    assert_eq!(st["levels"][0]["phases_signed"], 6);
    // Reopening an item takes ready off.
    let r = ex(&mut s, "cover.uncheck", json!({ "area": "W1-001/B", "item": "firestop" }));
    assert_eq!((r["was_ready"].as_bool(), r["ready"].as_bool()), (Some(true), Some(false)));
    assert_eq!(ex(&mut s, "cover.status", json!({ "ready": true }))["total"], 0);
    // An L2 area with its items done is ready now (its phases were signed above).
    check_all(&mut s, "W2-002/A");
    assert_eq!(ex(&mut s, "cover.ready", json!({ "area": "w2-002/a" }))["ready"], true);
    // Errors that say what to give.
    assert!(err(&mut s, "cover.sign", json!({ "phase": "Rough-In Electrical", "level": "9" })).contains("levels: 1, 2"));
    assert!(err(&mut s, "cover.check", json!({ "area": "W1-001/B", "item": "paint" })).contains("protection_plates"));
    assert!(err(&mut s, "cover.ready", json!({ "area": "W9-001/A" })).contains("cover.status"));
    assert!(err(&mut s, "cover.init", json!({ "model": dir.display().to_string() })).contains("replace: true"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn photos_are_filed_by_position_and_a_wall_tap_shows_the_tapped_face() {
    let (mut s, dir) = started("photos");
    // Taken south of W1-001 at x = 4: the office face B, 4 ft along.
    let r = ex(
        &mut s,
        "cover.photo",
        json!({ "action": "add", "at": [4.0, 14.6], "level": "1", "uri": "photos/b.jpg", "caption": "boxes at 18 in", "author": "Foreman" }),
    );
    assert_eq!((r["area"].as_str(), r["face"].as_str(), r["id"].as_str()), (Some("W1-001/B"), Some("B"), Some("PH-0001")), "{r}");
    assert_eq!(r["along_ft"], 4.0);
    let r = ex(&mut s, "cover.photo", json!({ "at": [6.0, 15.4], "level": "1", "uri": "photos/a.jpg", "face": "A" }));
    assert_eq!((r["area"].as_str(), r["id"].as_str()), (Some("W1-001/A"), Some("PH-0002")));
    // A stated face its position contradicts, a far photo, a bad action: refused.
    assert!(err(&mut s, "cover.photo", json!({ "at": [6.0, 15.4], "level": "1", "uri": "x.jpg", "face": "B" })).contains("face A side"));
    assert!(err(&mut s, "cover.photo", json!({ "at": [6.0, 40.0], "level": "1", "uri": "x.jpg" })).contains("no wall face within"));
    assert!(err(&mut s, "cover.photo", json!({ "action": "delete", "at": [6.0, 15.4], "level": "1", "uri": "x.jpg" })).contains("add"));
    // Tapping the south side lists the face B photo; both faces come with their checklists.
    let r = ex(&mut s, "cover.wall", json!({ "at": [5.0, 14.8], "level": "1" }));
    assert_eq!((r["wall"].as_str(), r["tapped"].as_str()), (Some("W1-001"), Some("W1-001/B")), "{r}");
    assert_eq!(r["photos"].as_array().map(Vec::len), Some(1));
    assert_eq!(r["photos"][0]["uri"], "photos/b.jpg");
    assert_eq!(r["faces"].as_array().map(Vec::len), Some(2));
    assert_eq!(r["faces"][0]["checklist"].as_array().map(Vec::len), Some(STANDARD_CATEGORIES.len()));
    // The north side: face A's photo.
    let r = ex(&mut s, "cover.wall", json!({ "at": [5.0, 15.2], "level": "1" }));
    assert_eq!((r["tapped"].as_str(), r["photos"][0]["uri"].as_str()), (Some("W1-001/A"), Some("photos/a.jpg")));
    // By id: both faces, no tapped face.
    let r = ex(&mut s, "cover.wall", json!({ "wall": "W1-001" }));
    assert!(r.get("tapped").is_none() && r["faces"].as_array().map(Vec::len) == Some(2));
    assert!(err(&mut s, "cover.wall", json!({ "at": [5.0, 14.8], "level": "2", "max_distance_ft": 0.1 })).contains("no wall within"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_record_claiming_ready_is_cleared_on_load_and_the_record_survives_a_cdy_round_trip() {
    let (mut s, dir) = started("load");
    ex(&mut s, "cover.check", json!({ "area": "W1-002/A", "item": "boxes" }));
    // Hand-edit the kept record: an area claims ready with six items open.
    let mut rec: Value = serde_json::from_slice(&s.doc().expect("doc").embedded["covercheck.json"]).expect("record");
    for a in rec["areas"].as_array_mut().expect("areas") {
        if a["id"] == "W1-002/A" {
            a["ready_to_cover"] = json!(true);
        }
    }
    s.doc_mut().expect("doc").embedded.insert("covercheck.json".into(), std::sync::Arc::new(serde_json::to_vec(&rec).expect("json")));
    let st = ex(&mut s, "cover.status", json!({ "ready": true }));
    assert_eq!(st["total"], 0, "not ready: {st}");
    assert_eq!(st["cleared"][0]["area"], "W1-002/A");
    assert_eq!(st["cleared"][0]["open_items"].as_array().map(Vec::len), Some(6));
    assert_eq!(st["cleared"][0]["unsigned_phases"].as_array().map(Vec::len), Some(6));
    // The next change keeps the cleared record.
    ex(&mut s, "cover.check", json!({ "area": "W1-002/A", "item": "cable" }));
    assert!(ex(&mut s, "cover.status", json!({})).get("cleared").is_none());
    // Saved and opened again (.cdy): the record and its plan come back.
    ex(&mut s, "cover.photo", json!({ "at": [15.0, 15.3], "level": "1", "uri": "p.jpg" }));
    let bytes = cadaddy_io::write(s.doc().expect("doc"), "job.cdy").expect("written");
    let back = cadaddy_io::read(&bytes, "job.cdy").expect("read");
    let mut s2 = Session::new();
    s2.open_drawing(back, "job", None);
    let r = ex(&mut s2, "cover.wall", json!({ "at": [15.0, 15.3], "level": "1" }));
    assert_eq!((r["tapped"].as_str(), r["photos"][0]["uri"].as_str()), (Some("W1-002/A"), Some("p.jpg")));
    let st = ex(&mut s2, "cover.status", json!({ "level": "1", "kind": "wall", "limit": 1 }));
    assert_eq!((st["total"].as_u64(), st["next_offset"].as_u64()), (Some(4), Some(1)));
    let page = ex(&mut s2, "cover.status", json!({ "level": "1", "kind": "wall", "offset": 2, "limit": 1 }));
    assert_eq!(page["areas"][0]["id"], "W1-002/A");
    assert_eq!(page["areas"][0]["done"], 2);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn hostile_params_never_panic() {
    let mut empty = Session::new();
    // No record yet: every command says to run cover.init.
    assert!(ex(&mut empty, "cover.status", json!({}))["hint"].as_str().is_some_and(|h| h.contains("cover.init")));
    for cmd in ["cover.check", "cover.uncheck", "cover.sign", "cover.ready", "cover.wall", "cover.photo"] {
        assert!(err(&mut empty, cmd, json!({})).contains("cover.init"), "{cmd}");
    }
    assert!(err(&mut empty, "cover.init", json!({ "model": "/nonexistent/prints" })).contains("truth.json"));
    let (mut s, dir) = started("hostile");
    let long = format!("{}{}", "a".repeat(1999), "é".repeat(600));
    for (cmd, p) in [
        ("cover.check", json!({ "area": "W1-001/A", "item": "boxes", "by": long, "date": long, "notes": long })),
        ("cover.sign", json!({ "phase": "firestop", "level": "1", "inspector": long, "date": long, "permit": long, "notes": long })),
        ("cover.ready", json!({ "area": "W1-001/A", "by": long })),
        ("cover.photo", json!({ "at": [5.0, 15.5], "level": "1", "uri": long, "caption": long, "author": long, "id": long })),
    ] {
        ex(&mut s, cmd, p);
    }
    for (cmd, p) in [
        ("cover.status", json!({ "level": 5, "offset": -1, "limit": "all", "ready": "yes" })),
        ("cover.check", json!({ "area": long, "item": long })),
        ("cover.uncheck", json!({ "area": [1], "item": {} })),
        ("cover.sign", json!({ "phase": "", "level": null })),
        ("cover.ready", json!({ "area": 7 })),
        ("cover.wall", json!({ "at": [1e308, -1e308], "level": "1", "max_distance_ft": f64::MAX })),
        ("cover.wall", json!({ "at": "here" })),
        ("cover.wall", json!({ "wall": long })),
        ("cover.photo", json!({ "at": [5.0, 15.5], "level": "1", "uri": "x", "elevation_ft": "high" })),
        ("cover.photo", json!({ "at": [5.0], "level": "1", "uri": "x" })),
        ("cover.photo", json!({ "at": [5.0, 15.5], "level": "1", "uri": "x", "max_distance_ft": -4 })),
        ("cover.init", json!({ "model": 5, "replace": "yes" })),
    ] {
        let _ = s.execute(cmd, &p);
    }
    std::fs::remove_dir_all(&dir).ok();
}
