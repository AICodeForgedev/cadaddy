//! Job speed (Q5266): the whole set in minutes, with progress, resumable.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use cadaddy_engine::Session;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
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

/// The sheets the tests' sets hold (a quick job; the timing probe takes them all).
const FEW: &[&str] = &["G-001", "A-101", "A-102", "M-101", "M-102"];

/// A folder like a real job's: the full set as one merged PDF, plus a discipline set that
/// prints the mechanical sheets again. `only`: the sheets it holds (all when empty).
fn job_folder(tag: &str, size: HotelSize, only: &[&str]) -> std::path::PathBuf {
    let mut set = generate_hotel(42, size).expect("generate hotel");
    if !only.is_empty() {
        set.truth.sheets.retain(|s| only.contains(&s.number.as_str()));
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_jobspeed_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(dir.join("Mechanical")).expect("folders");
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    for sh in set.truth.sheets.iter().filter(|s| s.discipline == "M") {
        std::fs::write(dir.join("Mechanical").join(&sh.pdf), set.file(&sh.pdf).expect("sheet pdf")).expect("write sheet");
    }
    dir
}

/// The tests here set the process-wide progress sink and run whole jobs: one at a time.
static SERIAL: Mutex<()> = Mutex::new(());
static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// (checkpoint, copy): the sink copies the checkpoint aside when the rooms stage starts, as a
/// crash there would leave it.
static COPY_AT_ROOMS: Mutex<Option<(PathBuf, PathBuf)>> = Mutex::new(None);

fn sink(line: &str) {
    if let Ok(mut l) = LINES.lock() {
        l.push(line.to_string());
    }
    if line.contains(" rooms: Detect rooms")
        && let Ok(c) = COPY_AT_ROOMS.lock()
        && let Some((from, to)) = c.as_ref()
    {
        std::fs::copy(from, to).expect("the checkpoint exists when the rooms stage starts");
    }
}

/// Where the cancel sink asks the job to stop: at a line holding this text.
static CANCEL_AT: Mutex<Option<&'static str>> = Mutex::new(None);

/// A progress sink that asks the running job to stop (as an MCP client's cancel would) once,
/// at the line [`CANCEL_AT`] names.
fn cancel_sink(line: &str) {
    if let Ok(mut l) = LINES.lock() {
        l.push(line.to_string());
    }
    if let Ok(mut at) = CANCEL_AT.lock()
        && at.is_some_and(|t| line.contains(t))
    {
        *at = None;
        cadaddy_engine::request_cancel();
    }
}

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn job_state(s: &Session) -> Value {
    serde_json::from_str(s.doc().ok().and_then(|d| d.agent.clone()).as_deref().unwrap_or("{}")).expect("job state")
}

fn audit<'a>(job: &'a Value, event: &str) -> Option<&'a Value> {
    job["guided_job"]["audit"].as_array()?.iter().find(|a| a["event"] == event)
}

/// The plan sheets of a [`FEW`] folder (the job imports plans, not the cover sheet), and how many
/// sheets the discipline set prints again.
fn sheet_count(dir: &Path) -> (usize, usize) {
    assert!(dir.join("Full Set.pdf").exists());
    let mech = std::fs::read_dir(dir.join("Mechanical")).map_or(0, |d| d.count());
    (FEW.iter().filter(|n| !n.starts_with("G-")).count(), mech)
}

#[test]
fn each_page_is_imported_once_side_by_side() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("once", HotelSize::Small, FEW);
    let (sheets, mech) = sheet_count(&dir);
    assert!(mech > 0);
    let mut s = Session::new();
    let builds = cadaddy_engine::cmd::wallgen::builds_so_far();
    let st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false})).expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    // The model is built once: when job.start ends, the model its own step built is current
    // (the associative rebuild used to build it again, as if the whole plan had just been drawn).
    assert_eq!(cadaddy_engine::cmd::wallgen::builds_so_far() - builds, 1);
    let job = job_state(&s);
    let loaded = audit(&job, "sheets_loaded").expect("sheets loaded")["details"].clone();
    // Every sheet of the set once: the discipline set's copies of the mechanical sheets are not
    // imported again (the sheet index marks them superseded, or the import sees the same page).
    let pages = loaded["pages"].as_u64().expect("pages");
    assert!(pages >= sheets as u64 && pages <= (sheets + mech) as u64, "{loaded}");
    assert_eq!(loaded["imported"].as_u64(), Some(sheets as u64), "{loaded}");
    assert_eq!(loaded["same_pages"].as_u64(), Some(pages - sheets as u64), "{loaded}");
    // Side by side: no two sheets on top of each other.
    let rects: Vec<[f64; 4]> = loaded["sheets"]
        .as_array()
        .expect("sheets")
        .iter()
        .map(|p| {
            let r = p["rect"].as_array().expect("rect");
            [0, 1, 2, 3].map(|i| r.get(i).and_then(Value::as_f64).expect("number"))
        })
        .collect();
    assert_eq!(rects.len(), sheets);
    for (i, a) in rects.iter().enumerate() {
        assert!(a[2] > a[0] && a[3] > a[1], "{a:?}");
        for b in rects.iter().skip(i + 1) {
            let overlap = a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3];
            assert!(!overlap, "sheets overlap: {a:?} {b:?}");
        }
    }
    // Each stage says how long it took.
    for stage in st["stages"].as_array().expect("stages").iter().filter(|x| x["status"] == "Done") {
        assert!(stage["elapsed_ms"].as_u64().is_some(), "{stage}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_running_job_reports_each_stage_and_sheet() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("progress", HotelSize::Small, FEW);
    let (sheets, mech) = sheet_count(&dir);
    if let Ok(mut l) = LINES.lock() {
        l.clear();
    }
    cadaddy_engine::cmd::guided::set_progress_sink(Some(sink));
    let mut s = Session::new();
    s.events_subscribe(true);
    let st = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false}));
    cadaddy_engine::cmd::guided::set_progress_sink(None);
    let st = st.expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    let lines = LINES.lock().map(|l| l.clone()).unwrap_or_default();
    // import, classify, levels, rooms, devices, mep_read, bim_build: a start and an end line each.
    for (i, stage) in ["import", "classify", "levels", "rooms", "devices", "mep_read", "bim_build"].iter().enumerate() {
        let tag = format!("job {}/7 {stage}: ", i + 1);
        assert!(lines.iter().any(|l| l.starts_with(&tag) && !l.contains(" done in ")), "start of {stage}: {lines:?}");
        assert!(lines.iter().any(|l| l.starts_with(&tag) && l.contains(" done in ")), "end of {stage}: {lines:?}");
    }
    // A line per page the job imports: each sheet once (the discipline copies are dropped by
    // the sheet index or reported "not imported again").
    let sheet_lines: Vec<&String> = lines.iter().filter(|l| l.contains(": sheet ")).collect();
    let total = sheet_lines.len();
    assert!(total >= sheets && total <= sheets + mech, "a line per page: {sheet_lines:?}");
    assert!(sheet_lines.iter().all(|l| l.contains(&format!("/{total} "))), "{sheet_lines:?}");
    let imported = sheet_lines.iter().filter(|l| l.contains(" imported (")).count();
    assert_eq!(imported, sheets, "{sheet_lines:?}");
    // The same as events, for a control channel or MCP client.
    let ev = s.events_poll(0);
    let progress: Vec<&Value> = ev["events"].as_array().expect("events").iter().filter(|e| e["type"] == "job.progress").collect();
    assert!(progress.iter().any(|e| e["step"] == "sheets" && e["total"].as_u64() == Some(total as u64)), "{progress:?}");
    assert!(progress.iter().any(|e| e["step"] == "end" && e["stage"] == "bim_build" && e["elapsed_ms"].as_u64().is_some()), "{progress:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_job_cut_off_goes_on_from_its_checkpoint() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("resume", HotelSize::Small, FEW);
    let pdfs = dir.display().to_string();
    let checkpoint = dir.with_extension("job.cdy");
    let copy = dir.with_extension("job-copy.cdy");
    *COPY_AT_ROOMS.lock().unwrap_or_else(|e| e.into_inner()) = Some((checkpoint.clone(), copy.clone()));
    cadaddy_engine::cmd::guided::set_progress_sink(Some(sink));
    let mut first = Session::new();
    let r = first.execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "auto": true, "checkpoint": checkpoint.display().to_string()}));
    cadaddy_engine::cmd::guided::set_progress_sink(None);
    *COPY_AT_ROOMS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let st = r.expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    // A checkpoint the caller named stays, the finished model in it (B7-002).
    assert!(checkpoint.exists(), "the caller's checkpoint is kept");
    assert_eq!(st["checkpoint"], json!(checkpoint.display().to_string()), "{st}");
    assert!(copy.exists(), "the checkpoint as a crash in the rooms stage leaves it");
    let rooms_of =
        |s: &mut Session| s.execute("job.explain", &json!({"stage": "rooms"})).expect("explain")["result"]["rooms"].as_array().map_or(0, Vec::len);
    let rooms = rooms_of(&mut first);
    assert!(rooms > 0);
    let before = first.execute("job.status", &json!({})).expect("status");

    // The crash: a new session, the checkpoint left from the rooms stage.
    std::fs::copy(&copy, &checkpoint).expect("restore");
    let mut s = Session::new();
    let st =
        s.execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "auto": true, "checkpoint": checkpoint.display().to_string()})).expect("resume");
    assert_eq!(st["resumed"], json!(true), "{st}");
    assert_eq!(st["done"], json!(true), "{st}");
    assert!(audit(&job_state(&s), "job_resumed").is_some());
    // The stages before rooms are not run again (their times are the first run's); the rest are.
    for (a, b) in before["stages"].as_array().expect("stages").iter().zip(st["stages"].as_array().expect("stages")) {
        if ["import", "classify", "levels"].contains(&a["stage"].as_str().unwrap_or("")) {
            assert_eq!(a["elapsed_ms"], b["elapsed_ms"], "{a} {b}");
        }
    }
    assert_eq!(rooms_of(&mut s), rooms, "the same rooms as the uncut run");
    assert!(checkpoint.exists());

    // Other prints, a damaged checkpoint, or resume off: a fresh job.
    std::fs::copy(&copy, &checkpoint).expect("restore");
    let mut s = Session::new();
    let st = s
        .execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "auto": true, "checkpoint": checkpoint.display().to_string(), "resume": false}))
        .expect("fresh");
    assert!(st["resumed"].is_null(), "{st}");
    // A file there that is not a job's checkpoint (the caller's drawing, or anything else) is
    // refused and left as it was: never written over, never removed (B7-002).
    std::fs::write(&checkpoint, b"not a drawing").expect("damage");
    let mut s = Session::new();
    let e = s
        .execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "auto": true, "checkpoint": checkpoint.display().to_string()}))
        .expect_err("an existing non-checkpoint file is refused");
    assert!(e.to_string().contains("not a job checkpoint"), "{e}");
    assert_eq!(std::fs::read(&checkpoint).expect("still there"), b"not a drawing");
    let drawing = dir.with_extension("mine.dxf");
    std::fs::write(&drawing, b"0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n").expect("a drawing");
    let mut s = Session::new();
    assert!(s.execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "auto": true, "checkpoint": drawing.display().to_string()})).is_err());
    assert!(drawing.exists(), "the caller's drawing is untouched");
    let _ = std::fs::remove_file(&drawing);
    std::fs::copy(&copy, &checkpoint).expect("restore");
    std::fs::write(dir.join("Added Sheet.pdf"), std::fs::read(dir.join("Full Set.pdf")).expect("set")).expect("add");
    let mut s = Session::new();
    let st =
        s.execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "auto": true, "checkpoint": checkpoint.display().to_string()})).expect("fresh");
    assert!(st["resumed"].is_null(), "the prints changed: {st}");
    // The job's own checkpoint (none named: in the temporary folder) is removed when it is done.
    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "auto": true, "resume": false})).expect("default checkpoint");
    assert_eq!(st["done"], json!(true), "{st}");
    assert!(st["checkpoint"].is_null(), "{st}");
    let own = std::env::temp_dir().join("cadaddy-jobs").join(format!("{}.cdy", st["job_id"].as_str().unwrap_or("?")));
    assert!(!own.exists(), "{}", own.display());
    let _ = std::fs::remove_file(&copy);
    let _ = std::fs::remove_file(&checkpoint);
    let _ = std::fs::remove_dir_all(&dir);
}

/// B7-015: a cancel (MCP `notifications/cancelled`, the `cancel` tool) stops a running job at
/// its next sheet or stage, not when it ends; job.next goes on from there, and the sheets cut
/// off mid-import are imported once.
#[test]
fn a_cancelled_job_stops_at_the_next_sheet_and_goes_on_from_there() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("cancel", HotelSize::Small, FEW);
    let pdfs = dir.display().to_string();
    // The uncut run, for its counts.
    let mut whole = Session::new();
    let st = whole.execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "auto": true, "checkpoint": false})).expect("job.start");
    assert_eq!(st["done"], json!(true), "{st}");
    let rooms_of =
        |s: &mut Session| s.execute("job.explain", &json!({"stage": "rooms"})).expect("explain")["result"]["rooms"].as_array().map_or(0, Vec::len);
    let rooms = rooms_of(&mut whole);
    // No job running: a cancel is dropped, not kept for the next job.
    cadaddy_engine::request_cancel();

    for (at, stage) in [(" classify: Classify", "classify"), (": sheet 1/", "rooms")] {
        *CANCEL_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some(at);
        if let Ok(mut l) = LINES.lock() {
            l.clear();
        }
        cadaddy_engine::cmd::guided::set_progress_sink(Some(cancel_sink));
        let mut s = Session::new();
        let st = s.execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "auto": true, "checkpoint": false}));
        cadaddy_engine::cmd::guided::set_progress_sink(None);
        let st = st.expect("job.start");
        assert_eq!(st["done"], json!(false), "{st}");
        assert_eq!(st["status"], json!("cancelled"), "{st}");
        assert_eq!(st["cancelled"]["stage"], json!(stage), "{st}");
        assert_eq!(st["current_stage"], json!(stage), "{st}");
        let lines = LINES.lock().map(|l| l.clone()).unwrap_or_default();
        assert!(!lines.iter().any(|l| l.contains("bim_build: Build")), "stopped before the build: {lines:?}");
        // Going on: the rest of the job, the same model as the uncut run.
        let mut done = s.execute("job.next", &json!({})).expect("job.next");
        for _ in 0..20 {
            if done["done"] == json!(true) {
                break;
            }
            done = s.execute("job.next", &json!({})).expect("job.next");
        }
        assert_eq!(done["done"], json!(true), "{done}");
        assert_eq!(rooms_of(&mut s), rooms, "cancelled at {stage}");
        let elements = |s: &mut Session| s.execute("job.status", &json!({})).expect("status")["elements"].clone();
        assert_eq!(elements(&mut s), elements(&mut whole), "the same model as the uncut run (cancelled at {stage})");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// B4-015: a print set none of whose files can be read is the job's error (which the CLI exits
/// non-zero on), naming the files; never `done`.
#[test]
fn a_print_set_of_unreadable_files_is_an_error_naming_them() {
    let _g = serial();
    real_pdf_reader();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_jobspeed_junk_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    for (name, bytes) in [("A-101.pdf", &b"%PDF-1.4\n garbage"[..]), ("E-101.pdf", &b"not a pdf at all"[..]), ("M-101.pdf", &b""[..])] {
        std::fs::write(dir.join(name), bytes).expect("write");
    }
    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false}));
    match st {
        // Refused outright is fine too: never a job that says it is done.
        Err(e) => assert!(!e.to_string().is_empty()),
        Ok(st) => {
            assert_eq!(st["done"], json!(false), "{st}");
            assert_eq!(st["status"], json!("error"), "{st}");
            assert_eq!(st["error"]["stage"], json!("classify"), "{st}");
            let msg = st["error"]["message"].as_str().unwrap_or("");
            assert!(msg.contains("could be read"), "{st}");
            let findings = s.execute("job.status", &json!({})).expect("status");
            assert_eq!(findings["status"], json!("error"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Timing probe on a full-size made-up job (run by hand: `--ignored --nocapture`).
#[test]
#[ignore]
fn time_a_full_job() {
    let _g = serial();
    real_pdf_reader();
    cadaddy_engine::cmd::guided::set_progress_sink(Some(|l| eprintln!("{l}")));
    let size = if std::env::var_os("JOBSPEED_SMALL").is_some() { HotelSize::Small } else { HotelSize::Full };
    let dir = job_folder("time", size, &[]);
    let mut s = Session::new();
    let t = std::time::Instant::now();
    let st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false})).expect("job.start");
    eprintln!("job.start {:.1} s: {st}", t.elapsed().as_secs_f64());
    let _ = std::fs::remove_dir_all(&dir);
}
