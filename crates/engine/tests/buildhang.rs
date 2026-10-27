//! Q6029 buildhang: the interactive-then-auto path finishes, reports progress per level, and
//! respects the time budget.

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

fn job_folder(tag: &str, size: HotelSize) -> std::path::PathBuf {
    let set = generate_hotel(42, size).expect("generate hotel");
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_buildhang_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folders");
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    dir
}

static SERIAL: Mutex<()> = Mutex::new(());

/// Q6029 MUST 4: the interactive-then-auto path on a made-up multi-level set finishes within a
/// test time limit. Start interactively, answer the classify/levels questions with defaults, then
/// switch to auto and verify the job completes.
#[test]
fn interactive_then_auto_finishes() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    real_pdf_reader();
    let dir = job_folder("int_auto", HotelSize::Full);
    let mut s = Session::new();
    s.events_subscribe(true);

    // Start INTERACTIVELY (auto: false) — the job pauses at each stage's questions.
    let st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": false, "checkpoint": false})).expect("job.start");
    assert_ne!(st["done"], json!(true), "interactive job should pause, not finish immediately");

    // Answer questions with defaults until bim_build, then switch to auto.
    let mut answered = 0;
    for _ in 0..200 {
        let next = s.execute("job.next", &json!({})).expect("job.next");
        if next.get("done").and_then(Value::as_bool) == Some(true) {
            break;
        }
        // If there's a question, answer with the default (or "Correct").
        if let Some(q) = next.get("question").or_else(|| next.get("next")).filter(|q| q.get("id").is_some()) {
            let qid = q["id"].as_str().unwrap_or("");
            let default = q.get("default").cloned().unwrap_or(json!("Correct"));
            let _ = s.execute("job.answer", &json!({"id": qid, "answer": default}));
            answered += 1;
        }
        // After answering a few questions from classify/levels, switch to auto.
        if answered >= 3 {
            break;
        }
    }

    // Switch to auto — this triggers the remaining pipeline.
    let started = web_time::Instant::now();
    let mode = s.execute("job.set_mode", &json!({"auto": true})).expect("job.set_mode");
    let elapsed_ms = started.elapsed().as_millis();

    eprintln!("buildhang interactive_then_auto: answered {answered} questions, set_mode auto");
    eprintln!("  done={}, elapsed={elapsed_ms} ms", mode.get("done").unwrap_or(&json!(null)));

    // Verify the job finished.
    let status = s.execute("job.status", &json!({})).expect("job.status");
    let done = status.get("done").and_then(Value::as_bool).unwrap_or(false);

    // The bim_build stage must have completed (not errored, not timed out).
    let bim_status = status["stages"].as_array().into_iter().flatten().find(|st| st["name"] == "bim_build").cloned().unwrap_or(json!(null));
    let bim_ok = bim_status["status"].as_str() == Some("Done") || bim_status["status"].as_str() == Some("done");
    eprintln!("  bim_build status: {}", bim_status["status"]);

    assert!(done || bim_ok, "interactive-then-auto job must complete; status: {status}");

    // Check for per-level progress events (Q6029 MUST 2).
    let ev = s.events_poll(0);
    let progress_events: Vec<&Value> =
        ev["events"].as_array().into_iter().flatten().filter(|e| e["type"] == "build.progress" && e["step"] == "level").collect();
    eprintln!("  per-level progress events: {}", progress_events.len());
    assert!(!progress_events.is_empty(), "must report per-level progress events");

    // The elapsed time must be reasonable (under the 20 min budget).
    assert!(elapsed_ms < 1_200_000, "interactive-then-auto path took {elapsed_ms} ms, exceeds 20 min budget");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Q6029 MUST 2: build.progress events include per-level info with ETA.
#[test]
fn build_reports_per_level_progress() {
    let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    real_pdf_reader();
    let dir = job_folder("progress", HotelSize::Full);
    let mut s = Session::new();
    s.events_subscribe(true);

    let st =
        s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false})).expect("job.start");
    assert_eq!(st["done"], json!(true), "auto job did not finish: {st}");

    let ev = s.events_poll(0);
    let events: Vec<&Value> = ev["events"].as_array().into_iter().flatten().collect();

    // Must have trace_rooms progress.
    let trace_rooms = events.iter().any(|e| e["type"] == "build.progress" && e["step"] == "trace_rooms");
    assert!(trace_rooms, "must emit trace_rooms progress event");

    // Must have per-level progress.
    let level_events: Vec<&&Value> = events.iter().filter(|e| e["type"] == "build.progress" && e["step"] == "level").collect();
    eprintln!("Level progress events: {}", level_events.len());
    for e in &level_events {
        eprintln!("  level={}, {}/{}, walls_so_far={}, eta_s={}", e["level"], e["level_num"], e["total_levels"], e["walls_so_far"], e["eta_s"]);
    }
    assert!(!level_events.is_empty(), "must report per-level progress");

    // Must have walls done.
    let walls_done = events.iter().any(|e| e["type"] == "build.progress" && e["step"] == "walls_done");
    assert!(walls_done, "must emit walls_done progress event");

    let _ = std::fs::remove_dir_all(&dir);
}
