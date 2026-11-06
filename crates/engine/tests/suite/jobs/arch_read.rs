//! Q307 rework (Q5107), MUST 1: `arch.read` runs the architecture reader on the open job's
//! sheets: a preview whose counts equal the made-up office's truth exactly (doors, marks,
//! handing, rooms, finishes), `apply` merging it into the building spec as one undo step, and a
//! guiding message (no error) when there is nothing architectural to read.

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// The office seed's DXF sheets written to a folder of their own.
fn sheets_folder(seed: u64, tag: &str) -> (std::path::PathBuf, cadaddy_mepfix::truth::Truth) {
    let set = cadaddy_mepfix::generate(seed).expect("generate");
    let dir = std::env::temp_dir().join(format!("cadaddy-arch-read-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    for (name, bytes) in set.files.iter().filter(|(n, _)| n.ends_with(".dxf")) {
        std::fs::write(dir.join(name), bytes).expect("write");
    }
    (dir, set.truth)
}

fn opened(seed: u64, tag: &str) -> (Session, cadaddy_mepfix::truth::Truth, std::path::PathBuf) {
    let (dir, t) = sheets_folder(seed, tag);
    let mut s = Session::new();
    s.execute("printset.open", &json!({"path": dir.to_string_lossy()})).expect("printset.open");
    (s, t, dir)
}

fn list(v: &Value, k: &str) -> Vec<Value> {
    v[k].as_array().cloned().unwrap_or_default()
}

#[test]
fn arch_read_preview_counts_equal_the_truth() {
    let (mut s, t, dir) = opened(3, "preview");
    let a = t.arch.as_ref().expect("arch truth");
    let r = s.execute("arch.read", &json!({"limit": 200})).expect("arch.read");
    assert_eq!(r["source"], "print set");
    assert_eq!(r["applied"], false);
    let c = &r["counts"];
    assert_eq!(c["doors"], json!(a.doors.len()), "{:#}", r["misses"]);
    assert_eq!(c["marked"], json!(a.doors.len()));
    assert_eq!(c["windows"], json!(a.windows.len()));
    assert_eq!((c["stairs"].clone(), c["elevators"].clone()), (json!(1), json!(1)));
    assert_eq!(c["rooms"], json!(a.finishes.len()));
    assert_eq!(c["rooms_with_finishes"], json!(a.finishes.len()));
    assert_eq!(r["misses_total"], json!(0), "{:#}", r["misses"]);
    // Every door as the truth has it: hand, the rooms either side, card reader; read off its
    // level's plan with its row from the door schedule.
    let doors = list(&r, "doors");
    assert_eq!(doors.len(), a.doors.len());
    for d in &doors {
        let mark = d["mark"].as_str().expect("mark");
        let td = a.doors.iter().find(|x| x.mark == mark).unwrap_or_else(|| panic!("door {mark} not in the truth"));
        assert_eq!(d["hand"], json!(td.hand), "door {mark}");
        assert_eq!(d["level"], json!(td.level), "door {mark}");
        assert_eq!((d["swings_into"].clone(), d["from_room"].clone()), (json!(td.swings_into), json!(td.from_room)), "door {mark}");
        let card = a.hardware.iter().any(|g| g.group == td.hardware && g.card_reader);
        assert_eq!(d["card_reader"], json!(card), "door {mark}");
        assert_eq!(d["source"]["status"], "schedule");
        assert!(d["source"]["sheet"].as_str().is_some_and(|x| x.starts_with("A-1")), "{}", d["source"]);
        assert_eq!(d["source"]["schedule"], r["schedules"]["doors"]);
    }
    // Every room with its finishes and plates.
    let rooms = list(&r, "rooms");
    assert_eq!(rooms.len(), a.finishes.len());
    for f in &a.finishes {
        let rm = rooms.iter().find(|x| x["number"] == json!(f.number) && x["level"] == json!(f.level)).unwrap_or_else(|| panic!("room {}", f.number));
        assert_eq!(rm["name"], json!(f.name));
        assert_eq!(
            (rm["floor"].clone(), rm["base"].clone(), rm["wall"].clone(), rm["ceiling"].clone()),
            (json!(f.floor), json!(f.base), json!(f.wall), json!(f.ceiling))
        );
        let plates = a.finish_codes.iter().find(|x| x.code == f.wall).and_then(|x| x.plates.clone());
        assert_eq!(rm["plates"], json!(plates), "room {}", f.number);
        assert_eq!(rm["source"]["schedule"], r["schedules"]["finishes"]);
    }
    let st = list(&r, "stairs");
    assert_eq!(st.first().map(|x| x["risers"].clone()), a.stairs.first().map(|x| json!(x.risers)));
    let el = list(&r, "elevators");
    assert_eq!(el.first().map(|x| x["capacity_lb"].clone()), a.elevators.first().map(|x| json!(x.capacity_lb)));

    // Paged: the first five doors and rooms, then the rest.
    let p1 = s.execute("arch.read", &json!({"limit": 5})).expect("page 1");
    assert_eq!((list(&p1, "doors").len(), list(&p1, "rooms").len(), p1["next"].clone()), (5, 5, json!(5)));
    let p2 = s.execute("arch.read", &json!({"limit": 5, "offset": 5})).expect("page 2");
    assert_ne!(list(&p1, "doors").first(), list(&p2, "doors").first());
    // One level.
    let l2 = s.execute("arch.read", &json!({"level": "Level 2", "limit": 200})).expect("level 2");
    assert_eq!(l2["counts"]["doors"], json!(a.doors.iter().filter(|d| d.level == "2").count()));
    assert!(list(&l2, "rooms").iter().all(|x| x["level"] == "2"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn arch_read_apply_merges_into_the_building_spec_and_undoes() {
    let (mut s, t, dir) = opened(3, "apply");
    let a = t.arch.as_ref().expect("arch truth");
    let f2f = t.building.levels.iter().find(|l| l.code == "2").map_or(0.0, |l| l.elevation_ft);
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {"version": 1, "levels": [
            {"name": "Level 1", "elevation_ft": 0.0, "height_ft": f2f, "ceiling_ft": 9.0},
            {"name": "Level 2", "elevation_ft": f2f, "height_ft": 13.0, "ceiling_ft": 9.0}]}}),
    )
    .expect("levels");
    let before = s.doc().expect("doc").building_spec.clone();
    let r = s.execute("arch.read", &json!({"apply": true, "author": "agent"})).expect("apply");
    assert_eq!(r["applied"], true, "{r:#}");
    assert!(list(&r, "warnings").is_empty(), "{:#}", r["warnings"]);
    let sp = cadaddy_walk3d::BuildingSpec::from_json(s.doc().expect("doc").building_spec.as_deref().expect("stored")).expect("spec");
    // Every room on its level with its finishes, base and plates.
    assert_eq!(sp.rooms.len(), a.finishes.len());
    for f in &a.finishes {
        let level = format!("Level {}", f.level);
        let rm = sp.rooms.iter().find(|x| x.number.as_deref() == Some(f.number.as_str()) && x.level.as_deref() == Some(level.as_str()));
        let rm = rm.unwrap_or_else(|| panic!("room {} on {level}", f.number));
        assert!(rm.materials.floor.as_deref().is_some_and(|m| m.starts_with(f.floor.as_str())), "room {}: {:?}", f.number, rm.materials);
        assert!(rm.materials.wall.is_some() && rm.materials.ceiling.is_some() && rm.base.is_some());
        let plates = a.finish_codes.iter().find(|x| x.code == f.wall).and_then(|x| x.plates.clone());
        assert_eq!(rm.plates, plates);
    }
    // The door and window rows, and a tag per door with how it swings.
    let door_rows = sp.schedule.rows.iter().filter(|x| x.kind == cadaddy_walk3d::FamilyKind::Door).count();
    let window_rows = sp.schedule.rows.iter().filter(|x| x.kind == cadaddy_walk3d::FamilyKind::Window).count();
    assert_eq!((door_rows, window_rows), (a.doors.len(), a.window_types.len()));
    for d in &a.doors {
        let tag = sp.schedule.tags.iter().find(|g| g.mark == d.mark && g.hand.is_some()).unwrap_or_else(|| panic!("tag {}", d.mark));
        assert_eq!(tag.hand.as_deref(), Some(d.hand.as_str()));
        assert_eq!(tag.level.as_deref(), Some(format!("Level {}", d.level).as_str()));
        assert_eq!(tag.card_reader, a.hardware.iter().any(|g| g.group == d.hardware && g.card_reader));
        assert!(tag.hinge.is_some() && tag.latch.is_some() && tag.source.as_deref().is_some_and(|x| x.starts_with("A-1")));
    }
    // The stair climbs floor to floor; the hoistway has its pit.
    let st = sp.stairs.first().expect("stair");
    let (risers, riser_in, _, _) = sp.stair_measures(st).expect("measures");
    assert_eq!((st.level.as_str(), st.to_level.as_deref()), ("Level 1", Some("Level 2")));
    assert!((riser_in * f64::from(risers) - f2f * 12.0).abs() < 1e-6);
    assert!(sp.elevators.first().is_some_and(|e| e.pit_ft.is_some() && e.levels == ["Level 1", "Level 2"]));
    // Who applied it, from which sheets.
    assert!(sp.sources.get("arch.read").is_some_and(|x| x.starts_with("applied by agent") && x.contains("A-1")), "{:?}", sp.sources);
    let errors: Vec<_> = sp.check().into_iter().filter(|i| i.error).collect();
    assert!(errors.is_empty(), "{errors:?}");

    // Applying again changes nothing more (rooms by level and number, rows and tags by mark).
    let r = s.execute("arch.read", &json!({"apply": true, "author": "agent"})).expect("again");
    assert_eq!((r["applied"].clone(), r["unchanged"].clone()), (json!(true), json!(true)));
    let again = cadaddy_walk3d::BuildingSpec::from_json(s.doc().expect("doc").building_spec.as_deref().expect("stored")).expect("spec");
    assert_eq!(
        (again.rooms.len(), again.schedule.rows.len(), again.schedule.tags.len()),
        (sp.rooms.len(), sp.schedule.rows.len(), sp.schedule.tags.len())
    );
    assert_eq!((again.stairs.len(), again.elevators.len()), (1, 1));
    // One undo step: the spec as it was.
    s.execute("undo", &json!({})).expect("undo");
    assert_eq!(s.doc().expect("doc").building_spec, before);
    // A preview stores nothing.
    s.execute("arch.read", &json!({})).expect("preview");
    assert_eq!(s.doc().expect("doc").building_spec, before);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn arch_read_without_architectural_sheets_says_what_to_load() {
    // An empty drawing: no print set, nothing to read.
    let mut s = Session::new();
    let r = s.execute("arch.read", &json!({})).expect("no error");
    assert_eq!(r["source"], "current drawing");
    assert_eq!(r["counts"]["doors"], json!(0));
    let msg = r["message"].as_str().expect("message");
    assert!(msg.contains("printset.open") && msg.contains("floor plans"), "{msg}");
    let r = s.execute("arch.read", &json!({"apply": true})).expect("apply does nothing");
    assert_eq!(r["applied"], false);
    // A print set of mechanical sheets only.
    let (dir, _) = sheets_folder(2, "mech");
    for e in std::fs::read_dir(&dir).expect("dir").flatten() {
        if !e.file_name().to_string_lossy().starts_with("M-") {
            let _ = std::fs::remove_file(e.path());
        }
    }
    s.execute("printset.open", &json!({"path": dir.to_string_lossy()})).expect("open");
    let r = s.execute("arch.read", &json!({})).expect("no error");
    assert_eq!(r["source"], "print set");
    assert!(r["sheets_read"].as_u64().is_some_and(|n| n > 0));
    assert!(r["message"].as_str().is_some_and(|m| m.contains("no architectural (A) sheet")), "{r:#}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn arch_read_reads_the_open_floor_plan_alone() {
    // One floor plan open as the drawing: its doors and rooms, with no schedule to name them,
    // are guesses that say why (and the message-free answer lists them as misses).
    let set = cadaddy_mepfix::generate(1).expect("generate");
    let (name, bytes) = set.files.iter().find(|(n, _)| n == "A-101.dxf").expect("A-101");
    let d = cadaddy_io::read_dxf(bytes).expect("dxf");
    let mut s = Session::new();
    s.open_drawing(d, name, None);
    let r = s.execute("arch.read", &json!({"limit": 200})).expect("arch.read");
    assert_eq!(r["source"], "current drawing");
    assert!(r["message"].is_null(), "{r:#}");
    let doors = list(&r, "doors");
    assert!(!doors.is_empty());
    assert!(doors.iter().all(|d| d["source"]["sheet"] == "A-101" && d["source"]["status"] == "guess" && d["mark"].is_null()));
    assert!(r["misses_total"].as_u64().is_some_and(|n| n as usize >= doors.len()));
}
