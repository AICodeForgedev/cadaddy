//! Q6156: a job's checkpoint goes on from another folder or machine when the prints are the
//! same by content (names, sizes, contents; not the path or the change times), and other prints
//! are refused naming the file that differs.

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

/// (checkpoint, copy): the sink copies the checkpoint aside when the rooms stage starts, as a
/// crash there would leave it.
static COPY_AT_ROOMS: Mutex<Option<(PathBuf, PathBuf)>> = Mutex::new(None);

fn sink(line: &str) {
    if line.contains(" rooms: Detect rooms")
        && let Ok(c) = COPY_AT_ROOMS.lock()
        && let Some((from, to)) = c.as_ref()
    {
        std::fs::copy(from, to).expect("the checkpoint exists when the rooms stage starts");
    }
}

fn job_state(s: &Session) -> Value {
    serde_json::from_str(s.doc().ok().and_then(|d| d.agent.clone()).as_deref().unwrap_or("{}")).expect("job state")
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
    real_pdf_reader();
    let root = unique("move");
    let (made, moved, changed) = (root.join("made here"), root.join("elsewhere").join("prints"), root.join("changed"));
    job_folder(&made);
    let checkpoint = root.join("job.cdy");
    let cut = root.join("job-cut.cdy");

    // The first run, its checkpoint copied aside as a crash in the rooms stage leaves it.
    *COPY_AT_ROOMS.lock().unwrap_or_else(|e| e.into_inner()) = Some((checkpoint.clone(), cut.clone()));
    cadaddy_engine::cmd::guided::set_progress_sink(Some(sink));
    let (mut first, r) = start(&made, &checkpoint, json!(true));
    cadaddy_engine::cmd::guided::set_progress_sink(None);
    *COPY_AT_ROOMS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    assert_eq!(r.expect("job.start")["done"], json!(true));
    assert!(cut.exists());
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
