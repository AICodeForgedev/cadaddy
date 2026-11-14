//! Q6578 fix-job-build-tail-working-set: the BIM build lets go of each working set it reads in
//! several steps (the legend, panel schedule and electrical sheets' pages, the sheet placer with
//! the sheets' linework it fits) once the last step that reads it has run, not at the end of the
//! stage. Held to the end, they kept about 0.8 GB of a four-storey print set's pages through the
//! steps after their last reader (conduit routes final, value engineering, the print inventory),
//! where the job's memory peaks. Each build step's progress names what the build still holds,
//! beside the memory it left.

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

/// A made-up job folder: the sample set's cover, a level's architectural plan and its electrical
/// plans, as one PDF.
fn prints(dir: &std::path::Path) {
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    set.truth.sheets.retain(|s| s.number == "G-001" || s.number.starts_with("A-10") || s.number.starts_with("E-"));
    std::fs::create_dir_all(dir).expect("folder");
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
}

/// The build's steps in order, each with the working sets its progress says are still held
/// (`None`: it says nothing of them).
fn build_steps(s: &mut Session) -> Vec<(String, Option<Vec<String>>, Option<u64>)> {
    let ev = s.events_poll(0);
    ev["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["type"] == "job.progress" && e["stage"] == "bim_build" && e.get("elapsed_s").is_some())
        .map(|e| {
            let held = e["held"].as_array().map(|h| h.iter().filter_map(Value::as_str).map(str::to_string).collect());
            (e["step"].as_str().unwrap_or_default().to_string(), held, e["memory_mb"].as_u64())
        })
        .collect()
}

#[test]
fn the_build_lets_go_of_its_sheet_working_sets_once_their_last_reader_ran() {
    real_pdf_reader();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_jobtailheld_{}_{nanos}", std::process::id()));
    prints(&dir);
    let mut s = Session::new();
    s.events_subscribe(true);
    let r = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false}));
    let _ = std::fs::remove_dir_all(&dir);
    let r = r.expect("job.start");
    assert_eq!(r["done"], json!(true), "the job didn't finish: {r}");
    let steps = build_steps(&mut s);
    let names: Vec<&str> = steps.iter().map(|(n, _, _)| n.as_str()).collect();
    let at = |step: &str| names.iter().position(|n| *n == step).unwrap_or_else(|| panic!("no build step {step:?} in {names:?}"));
    let held =
        |i: usize| steps.get(i).and_then(|(_, h, _)| h.clone()).unwrap_or_else(|| panic!("step {:?} says nothing of what it holds", names.get(i)));
    // Every step says what the build still holds (and, where the system says it, the memory).
    for i in 0..steps.len() {
        held(i);
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(steps.get(i).and_then(|(_, _, m)| *m).is_some(), "step {:?} says no memory", names.get(i));
        }
    }
    // Each set is held through the last step that reads it (its progress says so) and gone by
    // the next one.
    for (what, last, next) in [
        ("legend pages", "devices placed", "model built"),
        ("panel schedule pages", "panel schedules", "electrical gear"),
        ("sheet placer", "documented objects", "feeder routes"),
        ("electrical pages", "lighting", "devices placed on their wall faces"),
    ] {
        assert!(held(at(last)).iter().any(|h| h == what), "{what} aren't held through {last:?}: {steps:?}");
        assert!(!held(at(next)).iter().any(|h| h == what), "{what} are still held at {next:?}: {steps:?}");
    }
    for step in ["devices placed on their wall faces", "conduit routes final", "breakers", "value engineering", "load calculation", "print inventory"]
    {
        let h = held(at(step));
        assert!(h.is_empty(), "at {step:?} the build still holds {h:?}");
    }
}
