//! A job whose plans give no room (Q6362): the steps after its rooms stage (the build's plan
//! record, the wall-face placement, the takeoff and the report) reuse what the rooms stage read
//! and never trace the plans' rooms again.
//!
//! A program of its own: it installs the PDF reader hooks and bounds a stage's time.

use std::collections::HashSet;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, EntityStore, Space};
use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::printed::{OneViewOptions, one_view_sheet};
use serde_json::{Value, json};

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

const SEED: u64 = 42;

fn folder(tag: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_roomlessbuild_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    dir
}

/// The made-up building's floor plans `numbers`, each printed as its own one-view sheet; without
/// `tagged`, every room tag (each room's name and number) left off: plans that name no room.
fn write_plans(dir: &std::path::Path, numbers: &[&str], tagged: bool) {
    let (t, st) = cadaddy_mepfix::hotel_planned(SEED, HotelSize::Small);
    let tags: HashSet<String> = t
        .building
        .rooms
        .iter()
        .flat_map(|r| [r.name.trim().to_ascii_uppercase(), r.number.trim().to_ascii_uppercase()])
        .filter(|x| !x.is_empty())
        .collect();
    for number in numbers {
        let sheet = one_view_sheet(&t, &st, number, "", &OneViewOptions::default()).expect("one-view sheet");
        let mut d = sheet.drawing;
        let untagged = |e: &cadaddy_engine::doc::Entity| {
            tagged
                || match &e.kind {
                    EntityKind::Text(x) => !tags.contains(&x.value.trim().to_ascii_uppercase()),
                    EntityKind::MText(x) => !x.contents.lines().any(|l| tags.contains(&l.trim().to_ascii_uppercase())),
                    _ => true,
                }
        };
        let mut kept = EntityStore::new();
        kept.push_many(d.model.iter().filter(|e| untagged(e)).map(|e| (**e).clone()).collect::<Vec<_>>());
        d.model = kept;
        let ext = d.extents(&Space::Model);
        let printed = t.sheets.iter().find(|x| x.number == *number).and_then(|x| x.scale_text.clone()).unwrap_or_default();
        let k = cadaddy_mepfix::sheet_reader::parse_scale(&printed).unwrap_or(0.125);
        let (w, h) = ((ext.max.x - ext.min.x) * k / 12.0 * 25.4, (ext.max.y - ext.min.y) * k / 12.0 * 25.4);
        let opts = cadaddy_io::pdf::PdfOptions {
            paper_mm: Some((w.min(h), w.max(h))),
            landscape: Some(w > h),
            fit: Some(true),
            window: Some(ext),
            margins: Some([0.0; 4]),
            ..cadaddy_io::pdf::PdfOptions::default()
        };
        let pdf = cadaddy_io::pdf::pdf(&d, &Space::Model, &opts).expect("plot");
        std::fs::write(dir.join(format!("{number}.pdf")), pdf).expect("write sheet");
    }
}

fn job_state(s: &Session) -> Value {
    serde_json::from_str(s.doc().ok().and_then(|d| d.agent.as_ref().cloned()).as_deref().unwrap_or("{}")).expect("job state")
}

fn stage<'a>(job: &'a Value, name: &str) -> &'a Value {
    job["guided_job"]["stages"].as_array().and_then(|a| a.iter().find(|x| x["name"] == name)).expect("stage")
}

/// Runs a takeoff job on the plans in `dir` (auto mode; the questions it always asks answered
/// with their defaults, as the job that found the slow traces took them) and returns its state.
fn run_job(s: &mut Session, dir: &std::path::Path) -> Value {
    let st = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false}));
    let _ = std::fs::remove_dir_all(dir);
    let mut st = st.expect("job.start");
    for _ in 0..20 {
        if st["done"] == json!(true) {
            break;
        }
        let q = &st["question"];
        let (Some(id), Some(answer)) = (q["id"].as_str(), q["default"].as_str()) else { break };
        s.execute("job.answer", &json!({"id": id, "answer": answer})).expect("job.answer");
        st = s.execute("job.status", &json!({})).expect("job.status");
    }
    assert_eq!(st["done"], json!(true), "{st}");
    job_state(s)
}

#[test]
fn a_roomless_job_traces_no_rooms_after_its_rooms_stage() {
    real_pdf_reader();
    let dir = folder("untagged");
    write_plans(&dir, &["A-101", "A-102"], false);
    let mut s = Session::new();
    let job = run_job(&mut s, &dir);
    let rooms = &stage(&job, "rooms")["result"];
    assert_eq!(rooms["count"].as_u64(), Some(0), "the untagged plans give no room: {rooms}");
    let bim = stage(&job, "bim_build");
    assert_eq!(bim["status"], json!("done"), "{bim}");
    // The rooms stage reads the plans' rooms once (architectural plans only); nothing after it
    // (the build's plan record, the wall-face placement, the takeoff) reads them again.
    let traces = s.room_traces();
    eprintln!("roomless job: {traces} room trace(s), bim_build {} ms", bim["elapsed_ms"]);
    assert!(traces <= 1, "the plans were read for a room trace {traces} times: the steps after the rooms stage traced them again");
    // And the build is not held up by them (it took several times the rooms stage's trace).
    let ms = bim["elapsed_ms"].as_u64().unwrap_or(u64::MAX);
    assert!(ms < 600_000, "bim_build took {ms} ms");
    // Once the job has stopped, a room trace is as asked again: the whole drawing read.
    s.execute("rooms.list", &json!({"outline": false})).expect("rooms.list");
    assert_eq!(s.room_traces(), traces + 1, "rooms.list after the job traces the drawing");
}

#[test]
fn a_job_with_rooms_reads_its_rooms_stage_trace_after_it() {
    real_pdf_reader();
    let dir = folder("tagged");
    write_plans(&dir, &["A-101", "A-102"], true);
    let mut s = Session::new();
    let job = run_job(&mut s, &dir);
    let rooms = &stage(&job, "rooms")["result"];
    assert!(rooms["count"].as_u64().is_some_and(|n| n > 0), "the tagged plans give rooms: {rooms}");
    assert_eq!(stage(&job, "bim_build")["status"], json!("done"));
    // The plan record after the build reads the rooms the stage traced on its plan views (not
    // the whole drawing traced again).
    let traces = s.room_traces();
    eprintln!("job with rooms: {traces} room trace(s), {} rooms", rooms["count"]);
    assert!(traces <= 1, "the plans were read for a room trace {traces} times: the steps after the rooms stage traced them again");
}
