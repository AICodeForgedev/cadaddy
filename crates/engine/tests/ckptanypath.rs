//! Q6156: a job's checkpoint goes on from another folder or machine when the prints are the
//! same by content (names, sizes, contents; not the path or the change times), and other prints
//! are refused naming the file that differs. Q6264: a checkpoint's stages another build read run
//! again, so a rebuild from it reads the prints as a whole job of this build does.

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

fn unique(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("cadaddy_ckptany_{tag}_{}_{nanos}", std::process::id()))
}

/// A made-up job folder: the set as one merged PDF, plus the mechanical sheets again in a
/// subfolder.
fn job_folder(dir: &Path) {
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    set.truth.sheets.retain(|s| FEW.contains(&s.number.as_str()));
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
}

/// A copy of the folder elsewhere, every file written now and then dated two hours later (as a
/// copy to another machine leaves them).
fn copy_folder(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("folder");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(7200);
    for entry in std::fs::read_dir(from).expect("read") {
        let p = entry.expect("entry").path();
        let target = to.join(p.file_name().expect("name"));
        if p.is_dir() {
            copy_folder(&p, &target);
        } else {
            std::fs::write(&target, std::fs::read(&p).expect("read file")).expect("write file");
            std::fs::File::options().write(true).open(&target).expect("open").set_modified(later).expect("mtime");
        }
    }
}

/// The tests set the one progress sink: one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

/// (progress line, checkpoint, copy): the sink copies the checkpoint aside when the stage of the
/// line starts, as a crash there would leave it.
static COPY_AT: Mutex<Option<(&'static str, PathBuf, PathBuf)>> = Mutex::new(None);

fn sink(line: &str) {
    if let Ok(c) = COPY_AT.lock()
        && let Some((at, from, to)) = c.as_ref()
        && line.contains(at)
        && !to.exists()
    {
        std::fs::copy(from, to).expect("the checkpoint exists when the stage starts");
    }
}

/// Run a job as far as it goes with the progress sink copying its checkpoint aside to `cut` when
/// the stage of `at` starts.
fn run_cut(pdfs: &Path, checkpoint: &Path, cut: &Path, at: &'static str) -> (Session, Value) {
    *COPY_AT.lock().unwrap_or_else(|e| e.into_inner()) = Some((at, checkpoint.to_path_buf(), cut.to_path_buf()));
    cadaddy_engine::cmd::guided::set_progress_sink(Some(sink));
    let (s, r) = start(pdfs, checkpoint, json!(true));
    cadaddy_engine::cmd::guided::set_progress_sink(None);
    *COPY_AT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let st = r.expect("job.start");
    assert!(cut.exists(), "the checkpoint was copied when the stage started");
    (s, st)
}

fn job_state(s: &Session) -> Value {
    serde_json::from_str(s.doc().ok().and_then(|d| d.agent.as_ref().cloned()).as_deref().unwrap_or("{}")).expect("job state")
}

fn rooms_of(s: &mut Session) -> usize {
    s.execute("job.explain", &json!({"stage": "rooms"})).expect("explain")["result"]["rooms"].as_array().map_or(0, Vec::len)
}

fn start(pdfs: &Path, checkpoint: &Path, resume: Value) -> (Session, cadaddy_engine::Result<Value>) {
    let mut s = Session::new();
    let r = s.execute(
        "job.start",
        &json!({"pdfs": pdfs.display().to_string(), "goal": "bim", "auto": true, "checkpoint": checkpoint.display().to_string(), "resume": resume}),
    );
    (s, r)
}

#[test]
fn a_checkpoint_resumes_from_another_folder_when_the_prints_are_the_same() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    real_pdf_reader();
    let root = unique("move");
    let (made, moved, changed) = (root.join("made here"), root.join("elsewhere").join("prints"), root.join("changed"));
    job_folder(&made);
    let checkpoint = root.join("job.cdy");
    let cut = root.join("job-cut.cdy");

    // The first run, its checkpoint copied aside as a crash in the rooms stage leaves it.
    let (mut first, st) = run_cut(&made, &checkpoint, &cut, " rooms: Detect rooms");
    assert_eq!(st["done"], json!(true), "{st}");
    let rooms = rooms_of(&mut first);
    assert!(rooms > 0);
    let files = job_state(&first)["guided_job"]["prints_files"].as_array().map_or(0, Vec::len);
    assert_eq!(files, 1 + std::fs::read_dir(made.join("Mechanical")).map_or(0, |d| d.count()), "each print file is stamped");

    // The same prints in another folder, written later: it goes on from the checkpoint, the
    // print path remapped and said.
    copy_folder(&made, &moved);
    std::fs::copy(&cut, &checkpoint).expect("restore");
    let (mut s, r) = start(&moved, &checkpoint, json!(true));
    let st = r.expect("resume from another folder");
    assert_eq!(st["resumed"], json!(true), "{st}");
    assert_eq!(st["prints_check"], json!("same"), "{st}");
    assert_eq!(st["prints_moved_from"], json!(made.display().to_string()), "{st}");
    assert!(st["resume_note"].as_str().is_some_and(|n| n.contains("moved from")), "{st}");
    assert_eq!(st["done"], json!(true), "{st}");
    assert_eq!(rooms_of(&mut s), rooms, "the same rooms as the uncut run");
    let job = job_state(&s);
    assert_eq!(job["guided_job"]["pdfs_path"], json!(moved.display().to_string()), "the job reads the prints where they are now");
    let resumed = job["guided_job"]["audit"].as_array().and_then(|a| a.iter().find(|e| e["event"] == "job_resumed").cloned());
    assert_eq!(resumed.map(|a| a["details"]["prints_moved_from"].clone()), Some(json!(made.display().to_string())));

    // One PDF changed (same size, one byte): refused, the file named; nothing written over.
    copy_folder(&made, &changed);
    let mech = std::fs::read_dir(changed.join("Mechanical")).expect("mech").filter_map(|e| e.ok().map(|e| e.path())).min().expect("a sheet");
    let mut bytes = std::fs::read(&mech).expect("read");
    if let Some(b) = bytes.iter_mut().rev().find(|b| b.is_ascii_digit()) {
        *b = if *b == b'0' { b'1' } else { b'0' };
    }
    std::fs::write(&mech, &bytes).expect("change");
    let name = format!("Mechanical/{}", mech.file_name().and_then(|n| n.to_str()).unwrap_or("?"));
    std::fs::copy(&cut, &checkpoint).expect("restore");
    let before = std::fs::read(&checkpoint).expect("checkpoint");
    let (_, r) = start(&changed, &checkpoint, json!(true));
    let e = r.expect_err("other prints are refused");
    assert!(e.to_string().contains(&format!("{name} changed (same size, other content)")), "{e}");
    assert!(e.to_string().contains("resume: \"any\""), "{e}");
    assert_eq!(std::fs::read(&checkpoint).expect("checkpoint"), before, "the checkpoint is left as it was");

    // resume: "any" goes on from it anyway, saying what differs.
    let (_, r) = start(&changed, &checkpoint, json!("any"));
    let st = r.expect("resume any");
    assert_eq!(st["resumed"], json!(true), "{st}");
    assert_eq!(st["prints_check"], json!("differs"), "{st}");
    assert!(st["resume_note"].as_str().is_some_and(|n| n.contains(&name)), "{st}");

    // Anything else for resume is an error, not a guess.
    let (_, r) = start(&changed, &checkpoint, json!("maybe"));
    assert!(r.is_err());
    let _ = std::fs::remove_dir_all(&root);
}

/// A made-up job folder with a plumbing plan: its sheets one PDF each.
fn plumbing_folder(dir: &Path) {
    let mut set = generate_hotel(7, HotelSize::Small).expect("generate hotel");
    set.truth.sheets.retain(|s| ["G-001", "A-101", "P-101"].contains(&s.number.as_str()));
    std::fs::create_dir_all(dir).expect("folder");
    for sh in &set.truth.sheets {
        std::fs::write(dir.join(&sh.pdf), set.file(&sh.pdf).expect("sheet pdf")).expect("write sheet");
    }
}

/// The sanitary runs the job's MEP reading holds.
fn sanitary_runs(job: &Value) -> usize {
    job["guided_job"]["mep"]["runs"].as_array().map_or(0, |r| r.iter().filter(|r| r["system"] == "SAN").count())
}

/// Write the checkpoint at `path` with its job state changed by `edit`.
fn edit_checkpoint(path: &Path, edit: impl Fn(&mut Value)) {
    let name = path.display().to_string();
    let mut d = cadaddy_io::read(&std::fs::read(path).expect("checkpoint"), &name).expect("read checkpoint");
    let mut agent: Value = serde_json::from_str(d.agent.as_deref().unwrap_or("{}")).expect("job state");
    edit(&mut agent);
    d.agent = Some(agent.to_string()).into();
    std::fs::write(path, cadaddy_io::write(&d, &name).expect("write checkpoint")).expect("save checkpoint");
}

/// The checkpoint as an older build leaves it: `stages` stamped by another build (all of them
/// when empty), and its MEP reading without the sanitary runs (a reader that read none).
fn as_older_build(path: &Path, stages: &'static [&'static str]) {
    edit_checkpoint(path, |agent| {
        let job = &mut agent["guided_job"];
        for st in job["stages"].as_array_mut().into_iter().flatten() {
            let ran = st["status"] == "done" || st["status"] == "paused";
            if ran && (stages.is_empty() || st["name"].as_str().is_some_and(|n| stages.contains(&n))) {
                st["build"] = json!("0.0.1+src.0000000000000000");
            }
        }
        if let Some(runs) = job["mep"]["runs"].as_array_mut() {
            runs.retain(|r| r["system"] != "SAN");
        }
    });
}

fn audit_events(job: &Value, event: &str) -> usize {
    job["guided_job"]["audit"].as_array().map_or(0, |a| a.iter().filter(|e| e["event"] == event).count())
}

fn start_refresh(pdfs: &Path, checkpoint: &Path, refresh: Value) -> (Session, cadaddy_engine::Result<Value>) {
    let mut s = Session::new();
    let r = s.execute(
        "job.start",
        &json!({"pdfs": pdfs.display().to_string(), "goal": "bim", "auto": true, "checkpoint": checkpoint.display().to_string(), "refresh": refresh}),
    );
    (s, r)
}

#[test]
fn a_checkpoint_another_build_read_is_read_again() {
    let _one = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    real_pdf_reader();
    let root = unique("stale");
    let prints = root.join("prints");
    plumbing_folder(&prints);
    let checkpoint = root.join("job.cdy");
    let cut = root.join("job-cut.cdy");

    // The whole job, its checkpoint copied aside when the build starts (the pipes read). One
    // level's plans: the build goes as far as the spec lets it, which is all this needs.
    let (whole, _) = run_cut(&prints, &checkpoint, &cut, " bim_build: Build");
    let san = sanitary_runs(&job_state(&whole));
    assert!(san > 0, "the plumbing plan's sanitary pipe is read");

    // This build's own checkpoint goes on from it: nothing is read again.
    std::fs::copy(&cut, &checkpoint).expect("restore");
    let (s, r) = start(&prints, &checkpoint, json!(true));
    let st = r.expect("resume");
    assert_eq!(st["resumed"], json!(true), "{st}");
    assert!(st.get("refreshed").is_none(), "{st}");
    let job = job_state(&s);
    assert_eq!(sanitary_runs(&job), san);
    assert_eq!(audit_events(&job, "stage_rerun") + audit_events(&job, "job_refreshed"), 0);

    // An older build's checkpoint (it read no sanitary pipe): the job reads the prints again as
    // a whole job of this build does, rather than building from that reading.
    std::fs::copy(&cut, &checkpoint).expect("restore");
    as_older_build(&checkpoint, &[]);
    let (s, r) = start(&prints, &checkpoint, json!(true));
    let st = r.expect("refreshed");
    let job = job_state(&s);
    assert_eq!(sanitary_runs(&job), san, "the sanitary pipe a whole job reads");
    assert_eq!(st["refreshed"]["started_over"], json!(true), "{st}");
    assert_eq!(st["refreshed"]["from"], json!("import"), "{st}");
    assert!(st["resume_note"].as_str().is_some_and(|n| n.contains("another build")), "{st}");
    assert_eq!(audit_events(&job, "job_refreshed"), 1);

    // Only the pipe reading from another build: it and the stages after it run again; the
    // stages before are kept.
    std::fs::copy(&cut, &checkpoint).expect("restore");
    as_older_build(&checkpoint, &["mep_read"]);
    let (s, r) = start(&prints, &checkpoint, json!(true));
    let st = r.expect("refreshed from mep_read");
    assert_eq!(sanitary_runs(&job_state(&s)), san);
    assert_eq!(st["resumed"], json!(true), "{st}");
    assert_eq!(st["refreshed"]["from"], json!("mep_read"), "{st}");
    assert_eq!(st["refreshed"]["started_over"], json!(false), "{st}");
    assert_eq!(st["refreshed"]["stages"], json!(["mep_read"]), "{st}");
    let job = job_state(&s);
    assert_eq!(sanitary_runs(&job), san);
    assert_eq!(audit_events(&job, "stage_rerun"), 1);

    // refresh: a stage runs again from it, whoever read it.
    std::fs::copy(&cut, &checkpoint).expect("restore");
    let (_, r) = start_refresh(&prints, &checkpoint, json!("mep_read"));
    let st = r.expect("refresh from a stage");
    assert_eq!(st["refreshed"]["from"], json!("mep_read"), "{st}");

    // refresh: "none" keeps the older reading, as asked, and says so (a note and a finding).
    std::fs::copy(&cut, &checkpoint).expect("restore");
    as_older_build(&checkpoint, &["mep_read"]);
    let (s, r) = start_refresh(&prints, &checkpoint, json!("none"));
    let st = r.expect("kept");
    assert!(st.get("refreshed").is_none(), "{st}");
    assert!(st["resume_note"].as_str().is_some_and(|n| n.contains("kept from a checkpoint another build read: mep_read")), "{st}");
    let job = job_state(&s);
    assert_eq!(sanitary_runs(&job), 0, "kept as asked");
    let said = job["guided_job"]["findings"]
        .as_array()
        .is_some_and(|f| f.iter().any(|f| f["description"].as_str().is_some_and(|d| d.contains("another build"))));
    assert!(said, "the kept reading is a finding");

    // Anything else is an error, not a guess.
    let (_, r) = start_refresh(&prints, &checkpoint, json!("sometimes"));
    assert!(r.is_err());

    // Each stage that ran records the build that ran it.
    let stamped = job_state(&whole)["guided_job"]["stages"].as_array().map_or(0, |a| a.iter().filter(|st| st["build"].is_string()).count());
    assert!(stamped >= 6, "{stamped} stages stamped");
    let _ = std::fs::remove_dir_all(&root);
}
