//! Q6168: a job finds the project's specification itself (in the prints folder, beside it or
//! beside the submittals': named like one, or a PDF whose pages carry CSI section headers), says
//! where, and reads its product divisions; with none it asks at classify for the path or "none"
//! (auto mode: goes on with a high finding); the report lists the sections read.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

const FEW: &[&str] = &["G-001", "A-101", "A-102", "M-101", "M-102"];

/// A made-up wiring devices section.
const WIRING: &str = "SECTION 262726 - WIRING DEVICES\nPART 1 - GENERAL\nPART 2 - PRODUCTS\n2.2 STRAIGHT-BLADE RECEPTACLES\n\
A. Duplex Convenience Receptacles: 125 V, 20 A.\n1. Manufacturers: Subject to compliance with requirements, provide products by one of the following:\n\
a. Maker One.\nb. Maker Two.\n2.4 TOGGLE SWITCHES\nB. Switches, 120/277 V, 20 A:\n1. Single Pole:\n\
a. Manufacturers: Subject to compliance with requirements, provide products by one of the following:\n1) Maker One.\nPART 3 - EXECUTION\n";

/// A made-up project manual not named like one: a cover, then painting and wiring devices.
fn manual() -> String {
    format!(
        "SAMPLE OFFICE BUILDING\nTABLE OF CONTENTS\n099123 Interior Painting\n262726 Wiring Devices\n\
         SECTION 099123 - INTERIOR PAINTING\nPART 1 - GENERAL\nPART 2 - PRODUCTS\n2.1 PAINT\nA. Manufacturers: Subject to compliance with requirements, provide products by one of the following:\n1. Paint Maker.\nPART 3 - EXECUTION\n{WIRING}"
    )
}

fn unique(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("cadaddy_jobspec_{tag}_{}_{nanos}", std::process::id()))
}

/// A made-up job folder: a few sheets of the sample set as one PDF.
fn prints(dir: &Path) {
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    set.truth.sheets.retain(|s| FEW.contains(&s.number.as_str()));
    std::fs::create_dir_all(dir).expect("folder");
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("folder");
    std::fs::write(path, text).expect("write");
}

fn job_state(s: &Session) -> Value {
    serde_json::from_str::<Value>(s.doc().ok().and_then(|d| d.agent.clone()).as_deref().unwrap_or("{}")).expect("job state")["guided_job"].clone()
}

fn start(pdfs: &Path, more: Value) -> (Session, cadaddy_engine::Result<Value>) {
    let mut s = Session::new();
    let mut p = json!({"pdfs": pdfs.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false});
    if let (Some(p), Some(m)) = (p.as_object_mut(), more.as_object()) {
        p.extend(m.clone());
    }
    let r = s.execute("job.start", &p);
    (s, r)
}

fn sections_read(s: &mut Session) -> Vec<String> {
    let r = s.execute("job.report", &json!({})).expect("report");
    r["specifications"]["sections_read"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

fn no_spec_findings(job: &Value) -> Vec<Value> {
    job["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["description"].as_str().is_some_and(|d| d.starts_with("No specification")))
        .cloned()
        .collect()
}

#[test]
fn a_job_finds_the_specification_beside_its_prints_and_reads_it() {
    real_pdf_reader();
    let root = unique("beside");
    prints(&root.join("01 Drawings"));
    write(&root.join("03 Specifications").join("26 Electrical").join("26 27 26 - Wiring Devices.txt"), WIRING);
    write(&root.join("03 Specifications").join("09 Finishes").join("09 91 23 - Interior Painting.txt"), "SECTION 099123 - INTERIOR PAINTING\n");
    let (mut s, r) = start(&root.join("01 Drawings"), json!({}));
    let r = r.expect("job.start");
    assert_eq!(r["done"], json!(true), "{r:#}");
    let job = job_state(&s);
    assert_eq!(job["specs_found"], json!([root.join("03 Specifications").display().to_string()]), "{}", job["specs_how"]);
    assert!(job["specs_how"][0].as_str().is_some_and(|h| h.contains("named like a specification")), "{}", job["specs_how"]);
    assert!(no_spec_findings(&job).is_empty());
    assert_eq!(sections_read(&mut s), ["26 27 26 WIRING DEVICES"], "the product divisions' sections only");
    let report = s.execute("job.report", &json!({})).expect("report");
    assert_eq!(report["specifications"]["note"], json!("1 specification sections read"), "{report:#}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_project_manual_not_named_one_is_told_by_its_section_headers() {
    real_pdf_reader();
    let root = unique("sniff");
    prints(&root.join("Plans"));
    write(&root.join("Owner Documents").join("Book.txt"), &manual());
    write(&root.join("Owner Documents").join("Notes.txt"), "GENERAL NOTES\n1. Made-up note.\n");
    let (mut s, r) = start(&root.join("Plans"), json!({}));
    assert_eq!(r.expect("job.start")["done"], json!(true));
    let job = job_state(&s);
    assert_eq!(job["specs_found"], json!([root.join("Owner Documents").join("Book.txt").display().to_string()]));
    assert!(job["specs_how"][0].as_str().is_some_and(|h| h.contains("CSI section headers")), "{}", job["specs_how"]);
    assert_eq!(sections_read(&mut s), ["26 27 26 WIRING DEVICES"], "the manual's painting section is not a product division");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn without_a_specification_auto_mode_goes_on_with_a_high_finding() {
    real_pdf_reader();
    let root = unique("auto");
    prints(&root.join("Plans"));
    let (mut s, r) = start(&root.join("Plans"), json!({}));
    assert_eq!(r.expect("job.start")["done"], json!(true));
    let job = job_state(&s);
    let found = no_spec_findings(&job);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["severity"], json!(4));
    assert_eq!(found[0]["stage"], json!("classify"));
    let report = s.execute("job.report", &json!({})).expect("report");
    assert!(report["specifications"]["how"][0].as_str().is_some_and(|h| h.starts_with("none: auto mode")), "{report:#}");
    assert_eq!(report["specifications"]["note"], json!("no specification read"));
    let _ = std::fs::remove_dir_all(&root);
}

/// The job's question about the specification, and its id.
fn spec_question(job: &Value) -> Option<(String, Value)> {
    job["stages"].as_array().into_iter().flatten().flat_map(|st| st["questions"].as_array().into_iter().flatten()).find_map(|q| {
        q["question"].as_str().filter(|t| t.starts_with("No specification found")).map(|_| (q["id"].as_str().unwrap_or("").to_string(), q.clone()))
    })
}

#[test]
fn without_a_specification_the_job_asks_for_its_path_or_none() {
    real_pdf_reader();
    let root = unique("ask");
    prints(&root.join("Plans"));
    let elsewhere = unique("ask_manual");
    write(&elsewhere.join("26 27 26 - Wiring Devices.txt"), WIRING);

    // Asked at classify; a path that isn't there is refused (the question stays open).
    let (mut s, r) = start(&root.join("Plans"), json!({"auto": false}));
    assert_eq!(r.expect("job.start")["done"], json!(false));
    let job = job_state(&s);
    let (id, q) = spec_question(&job).expect("the job asks for the specification");
    assert_eq!(q["stage"], json!("classify"));
    assert_eq!(q["default"], json!("none"));
    let missing = root.join("not here");
    let e = s.execute("job.answer", &json!({"id": id, "answer": missing.display().to_string()})).expect_err("no such path");
    assert!(e.to_string().contains("no folder or PDF/text file"), "{e}");
    // The path: the build reads it.
    let a = s.execute("job.answer", &json!({"id": id, "answer": elsewhere.display().to_string()})).expect("answer the path");
    assert!(a.to_string().contains("reads the specification"), "{a}");
    s.execute("job.set_mode", &json!({"auto": true})).expect("run on");
    assert_eq!(s.execute("job.status", &json!({})).expect("status")["done"], json!(true));
    assert_eq!(sections_read(&mut s), ["26 27 26 WIRING DEVICES"]);
    let job = job_state(&s);
    assert!(no_spec_findings(&job).is_empty());
    assert!(job["specs_how"][0].as_str().is_some_and(|h| h.starts_with("given in answer")), "{}", job["specs_how"]);

    // "none": on without it, said by a finding.
    let (mut s, _) = start(&root.join("Plans"), json!({"auto": false}));
    let (id, _) = spec_question(&job_state(&s)).expect("asked");
    s.execute("job.answer", &json!({"id": id, "answer": "none"})).expect("answer none");
    s.execute("job.set_mode", &json!({"auto": true})).expect("run on");
    let job = job_state(&s);
    assert_eq!(job["specs_declined"], json!(true));
    assert_eq!(no_spec_findings(&job).len(), 1);
    assert!(sections_read(&mut s).is_empty());
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&elsewhere);
}

#[test]
fn started_with_specs_false_the_job_asks_nothing_and_says_so() {
    real_pdf_reader();
    let root = unique("declined");
    prints(&root.join("Plans"));
    for none in [json!(false), json!("none")] {
        let (s, r) = start(&root.join("Plans"), json!({"auto": false, "specs": none}));
        r.expect("job.start");
        let job = job_state(&s);
        assert!(spec_question(&job).is_none(), "no question: {none}");
        assert_eq!(job["specs_declined"], json!(true));
        assert_eq!(no_spec_findings(&job).len(), 1);
        assert!(job["specs_how"][0].as_str().is_some_and(|h| h.contains("started with specs: none")), "{}", job["specs_how"]);
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// (checkpoint, copy): the sink copies the checkpoint aside when the rooms stage starts.
static COPY_AT_ROOMS: Mutex<Option<(PathBuf, PathBuf)>> = Mutex::new(None);

fn sink(line: &str) {
    if line.contains(" rooms: Detect rooms")
        && let Ok(c) = COPY_AT_ROOMS.lock()
        && let Some((from, to)) = c.as_ref()
    {
        std::fs::copy(from, to).expect("the checkpoint exists when the rooms stage starts");
    }
}

#[test]
fn a_resumed_job_looks_again_for_a_specification_it_had_not_found() {
    real_pdf_reader();
    let root = unique("resume");
    let plans = root.join("Plans");
    prints(&plans);
    let (checkpoint, cut) = (root.join("job.cdy"), root.join("job-cut.cdy"));
    *COPY_AT_ROOMS.lock().unwrap_or_else(|e| e.into_inner()) = Some((checkpoint.clone(), cut.clone()));
    cadaddy_engine::cmd::guided::set_progress_sink(Some(sink));
    let (_, r) = start(&plans, json!({"checkpoint": checkpoint.display().to_string()}));
    cadaddy_engine::cmd::guided::set_progress_sink(None);
    *COPY_AT_ROOMS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    assert_eq!(r.expect("job.start")["done"], json!(true));
    // The specification arrives beside the prints; the job cut off after classify goes on.
    write(&root.join("Specifications").join("26 27 26 - Wiring Devices.txt"), WIRING);
    std::fs::copy(&cut, &checkpoint).expect("restore");
    let (mut s, r) = start(&plans, json!({"checkpoint": checkpoint.display().to_string()}));
    let st = r.expect("resume");
    assert_eq!(st["resumed"], json!(true), "{st}");
    let job = job_state(&s);
    assert_eq!(job["specs_found"], json!([root.join("Specifications").display().to_string()]));
    assert!(no_spec_findings(&job).is_empty(), "the earlier look's finding goes: {:?}", no_spec_findings(&job));
    assert_eq!(sections_read(&mut s), ["26 27 26 WIRING DEVICES"]);
    let _ = std::fs::remove_dir_all(&root);
}
