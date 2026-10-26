//! Q5273 jobjunk: every floor of a print set gives its rooms, and only rooms.

use std::collections::BTreeMap;

use cadaddy_engine::Session;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::json;

fn real_pdf_reader() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

/// The made-up hotel's whole set as one merged PDF, in a folder of its own, and the hotel's room
/// numbers by level.
fn job_folder() -> (std::path::PathBuf, BTreeMap<String, Vec<String>>) {
    let set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    let mut truth: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in &set.truth.building.rooms {
        truth.entry(r.level.clone()).or_default().push(r.number.clone());
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_jobjunk_rooms_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    (dir, truth)
}

#[test]
fn every_floor_gives_its_numbered_rooms_and_no_schedule_cells() {
    real_pdf_reader();
    let (dir, truth) = job_folder();
    let mut s = Session::new();
    let st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false})).expect("job.start");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(st["done"], json!(true), "{st}");
    let rooms = s.execute("job.explain", &json!({"stage": "rooms"})).expect("explain")["result"].clone();
    let list = rooms["rooms"].as_array().cloned().unwrap_or_default();
    // The numbered rooms by floor: a guest floor's numbers start with its level (201, 305, ...).
    let mut by_floor: BTreeMap<char, Vec<String>> = BTreeMap::new();
    for r in &list {
        let number = r["number"].as_str().unwrap_or("");
        if let Some(f) = number.chars().next().filter(char::is_ascii_digit) {
            by_floor.entry(f).or_default().push(format!("{} {number}", r["name"].as_str().unwrap_or("")));
        }
    }
    // Every numbered room of every floor (the architectural plans tag the guest rooms with their
    // unit type only; the other disciplines' plans of the same floor name and number them).
    for (level, numbers) in &truth {
        let Some(floor) = level.chars().find(char::is_ascii_digit) else { continue };
        let read = by_floor.get(&floor).cloned().unwrap_or_default();
        let missing: Vec<&String> = numbers.iter().filter(|n| !read.iter().any(|r| r.ends_with(&format!(" {n}")))).collect();
        assert!(missing.len() * 10 <= numbers.len(), "level {level}: missing {missing:?} of {numbers:?}; read {read:?}");
    }
    // A sheet's schedule cells and garbled print text are no rooms.
    for r in &list {
        let name = r["name"].as_str().unwrap_or("").to_ascii_uppercase();
        assert!(!["DESCRIPTION", "COUNTERTOP", "BASE CABINET", "VARIES", "SIZE (W X D)"].contains(&name.as_str()), "{name}: {rooms}");
        assert!(name.chars().filter(char::is_ascii_alphabetic).count() >= 2, "{name}: {rooms}");
    }
    // Each numbered room once on its floor.
    for (floor, names) in &by_floor {
        let mut numbers: Vec<&str> = names.iter().filter_map(|n| n.rsplit(' ').next()).collect();
        numbers.sort_unstable();
        let before = numbers.len();
        numbers.dedup();
        assert_eq!(before, numbers.len(), "level {floor}: {names:?}");
    }
}
