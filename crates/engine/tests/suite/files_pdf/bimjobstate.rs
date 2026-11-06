//! Q5276: an import followed by a build works without anything in between, the job is kept in
//! the print folder and a new session resumes it, and every `next` hint the briefing, the guided
//! job and the print set give runs as given.

use cadaddy_engine::Session;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};
use std::path::PathBuf;

/// The real readers and writers (PDF pages are imported for the build).
fn install_io() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
}

/// A folder of prints: the made-up hotel's `only` sheets, one PDF each.
fn prints(tag: &str, only: &[&str]) -> PathBuf {
    let set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_bimjobstate_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    for sh in set.truth.sheets.iter().filter(|s| only.contains(&s.number.as_str())) {
        std::fs::write(dir.join(&sh.pdf), set.file(&sh.pdf).expect("sheet pdf")).expect("write sheet");
    }
    dir
}

/// A session whose last-print-set note is its own (never the user's settings folder).
fn session(dir: &std::path::Path) -> Session {
    let mut s = Session::new();
    s.printset_pointer = Some(dir.join("pointer.txt"));
    s
}

const SHEETS: [&str; 3] = ["A-101", "A-102", "M-101"];

#[test]
fn a_build_right_after_the_import_drafts_the_spec_itself() {
    install_io();
    let dir = prints("import", &SHEETS);
    let mut s = session(&dir);
    s.execute("printset.open", &json!({"path": dir.display().to_string()})).expect("open");
    s.execute("printset.classify", &json!({})).expect("classify");
    let built = s.execute("bim.build", &json!({})).expect("bim.build after the import");
    assert!(built["drafted"].is_object(), "the spec was drafted: {built}");
    assert!(built["built"].as_object().is_some_and(|m| !m.is_empty()), "levels built: {built}");
    assert!(s.doc().expect("doc").building_spec.is_some(), "the drafted spec is stored");
    // Building again uses the stored spec: nothing is drafted twice.
    let again = s.execute("bim.build", &json!({})).expect("build again");
    assert!(again["drafted"].is_null(), "{again}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_build_right_after_printset_open_classifies_and_drafts() {
    install_io();
    let dir = prints("open", &SHEETS);
    let mut s = session(&dir);
    s.execute("printset.open", &json!({"path": dir.display().to_string()})).expect("open");
    let built = s.execute("bim.build", &json!({})).expect("bim.build right after printset.open");
    assert!(built["drafted"].is_object(), "{built}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_build_with_nothing_open_says_what_to_open() {
    let mut s = Session::new();
    let e = s.execute("bim.build", &json!({})).expect_err("nothing to draft from").to_string();
    assert!(e.contains("printset.open {path}"), "{e}");
}

/// The briefing's next call, run as given (`{path}` is the folder of prints).
fn run_next(s: &mut Session, dir: &std::path::Path, call: &str) -> Value {
    let (command, rest) = call.split_once(' ').unwrap_or((call, "{}"));
    let params = if rest.starts_with("{path}") { json!({"path": dir.display().to_string()}) } else { json!({}) };
    s.execute(command, &params).unwrap_or_else(|e| panic!("`{call}` as given failed: {e}"))
}

#[test]
fn every_next_hint_of_the_briefing_runs_as_given() {
    install_io();
    let dir = prints("hints", &SHEETS);
    let mut s = session(&dir);
    let mut seen = Vec::new();
    for _ in 0..6 {
        let b = s.execute("project.briefing", &json!({})).expect("briefing");
        let call = b["next"]["call"].as_str().expect("a next call").to_string();
        // `takeoff {} or export {…}`: each alternative is a call of its own.
        let first = call.split(" or ").next().unwrap_or(&call).split(" (").next().unwrap_or(&call).to_string();
        run_next(&mut s, &dir, &first);
        seen.push(first.split(' ').next().unwrap_or_default().to_string());
        if first.starts_with("bim.takeoff") || first.starts_with("takeoff") {
            break;
        }
        if first.starts_with("review.next") {
            // The queue is the agent's to work; the hint itself ran. Defer the rest so the
            // briefing moves on.
            let list = s.execute("review.list", &json!({"limit": 50})).expect("list");
            for i in list["items"].as_array().into_iter().flatten() {
                s.execute("review.defer", &json!({"id": i["id"], "reason": "field to verify"})).expect("defer");
            }
        }
    }
    assert_eq!(seen.first().map(String::as_str), Some("printset.open"), "{seen:?}");
    assert_eq!(seen.get(1).map(String::as_str), Some("bim.build"), "{seen:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_job_is_kept_in_the_print_folder_and_a_new_session_resumes_it() {
    install_io();
    let dir = prints("keep", &SHEETS);
    let mut s = session(&dir);
    s.execute("printset.open", &json!({"path": dir.display().to_string()})).expect("open");
    s.execute("bim.build", &json!({})).expect("build");
    s.execute("project.note", &json!({"text": "Level 1 walls checked; ducts next"})).expect("note");
    let before = s.execute("project.briefing", &json!({})).expect("briefing");
    let kept = s.execute("printset.keep", &json!({})).expect("keep");
    assert!(dir.join(".cadaddy").join("project.cdy").is_file() && dir.join(".cadaddy").join("printset.json").is_file(), "{kept}");
    // The kept folder isn't read as prints.
    let files = cadaddy_mepfix::printset::discover_sheets(&dir).expect("discover");
    assert!(files.iter().all(|(n, _)| !n.contains(".cadaddy")), "{files:?}");

    // A new session: nothing open until it resumes the last job kept.
    let mut fresh = session(&dir);
    let b = fresh.execute("project.briefing", &json!({})).expect("briefing");
    assert!(b["text"].as_str().is_some_and(|t| t.contains("Nothing built yet")), "{b}");
    let r = fresh.execute("printset.resume", &json!({"auto": true})).expect("resume");
    assert_eq!(r["resumed"], json!(true), "{r}");
    let after = fresh.execute("project.briefing", &json!({})).expect("briefing");
    assert_eq!(after["revision"], before["revision"], "{after}");
    assert_eq!(after["built"], before["built"], "{after}");
    assert!(after["notes"].to_string().contains("ducts next"), "{after}");
    assert!(fresh.printset_project.is_some(), "the print set is open again");
    assert!(fresh.doc().expect("doc").building_spec.is_some(), "the spec came back");
    // Resumed: the build goes on from the kept spec.
    let built = fresh.execute("bim.build", &json!({})).expect("build");
    assert!(built["drafted"].is_null(), "{built}");

    // Resuming by folder works too; an automatic resume leaves a session with work alone.
    let mut other = session(&dir);
    other.execute("line", &json!({"points": [[0, 0], [10, 0]]})).expect("line");
    let r = other.execute("printset.resume", &json!({"auto": true})).expect("resume");
    assert_eq!(r["resumed"], json!(false), "{r}");
    let r = other.execute("printset.resume", &json!({"path": dir.display().to_string()})).expect("resume");
    assert_eq!(r["resumed"], json!(true), "{r}");
    assert!(other.docs.len() >= 2, "the drawing with work stays open beside the kept one");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn nothing_kept_resumes_nothing() {
    let dir = std::env::temp_dir().join(format!("cadaddy_bimjobstate_none_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let mut s = session(&dir);
    let r = s.execute("printset.resume", &json!({})).expect("resume");
    assert_eq!(r["resumed"], json!(false), "{r}");
    let r = s.execute("printset.resume", &json!({"path": dir.display().to_string()})).expect("resume");
    assert_eq!(r["resumed"], json!(false), "{r}");
    // A pointer naming a folder that holds nothing kept (or garbage) is no error either.
    std::fs::write(dir.join("pointer.txt"), "\u{0}not a folder\n").expect("pointer");
    let r = s.execute("printset.resume", &json!({})).expect("resume");
    assert_eq!(r["resumed"], json!(false), "{r}");
    // Without a print set open there is nothing to keep.
    assert!(s.execute("printset.keep", &json!({})).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_kept_job_is_an_error_not_a_crash() {
    install_io();
    let dir = std::env::temp_dir().join(format!("cadaddy_bimjobstate_bad_{}", std::process::id()));
    let kept = dir.join(".cadaddy");
    std::fs::create_dir_all(&kept).expect("folder");
    std::fs::write(kept.join("printset.json"), b"{not json").expect("set");
    std::fs::write(kept.join("project.cdy"), b"garbage").expect("drawing");
    let mut s = session(&dir);
    assert!(s.execute("printset.resume", &json!({"path": dir.display().to_string()})).is_err());
    assert!(s.printset_project.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_guided_job_hints_run_as_given() {
    install_io();
    let dir = prints("guided", &["A-101"]);
    let mut s = session(&dir);
    let st = s.execute("job.status", &json!({})).expect("status");
    let hint = st["hint"].as_str().expect("hint");
    assert!(hint.contains("job.start {pdfs, goal}"), "{hint}");
    let note = s.execute("job.explain", &json!({})).expect("explain")["note"].as_str().unwrap_or_default().to_string();
    assert!(note.contains("job.start {pdfs}"), "{note}");
    // `job.start {pdfs}`, as the note gives it.
    let started = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "checkpoint": false})).expect("job.start {pdfs}");
    assert!(started.is_object(), "{started}");
    // `job.start {pdfs, goal}`, as the hint gives it, in a session of its own.
    let mut other = session(&dir);
    other.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "checkpoint": false})).expect("job.start {pdfs, goal}");
    // The guided job's state is kept with the drawing and comes back in a new session.
    other.execute("printset.keep", &json!({})).expect("keep");
    let mut fresh = session(&dir);
    fresh.execute("printset.resume", &json!({})).expect("resume");
    let st = fresh.execute("job.status", &json!({})).expect("status");
    assert!(st.get("hint").is_none(), "the job came back: {st}");
    let _ = std::fs::remove_dir_all(&dir);
}
