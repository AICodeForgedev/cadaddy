//! The guided job really asks (Q5274): it stops at its checkpoints until they are answered, and
//! an answer runs the stages it affects again and says what changed.

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

/// The made-up hotel's cover, two floor plans and a mechanical plan, as one merged PDF.
fn job_folder(tag: &str) -> std::path::PathBuf {
    let mut set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    set.truth.sheets.retain(|s| ["G-001", "A-101", "A-102", "M-101"].contains(&s.number.as_str()));
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_jobask_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("folder");
    let pages: Vec<(Vec<u8>, String, String)> = set
        .truth
        .sheets
        .iter()
        .map(|sh| (set.file(&sh.pdf).expect("sheet pdf").to_vec(), format!("{} {}", sh.number, sh.title), String::new()))
        .collect();
    std::fs::write(dir.join("Full Set.pdf"), cadaddy_io::pdf_merge::merge(&pages).expect("merge")).expect("write set");
    dir
}

/// Whole jobs, one at a time (the PDF reader hooks are process-wide).
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// The built model's levels: (name, elevation in feet).
fn levels(s: &mut Session) -> Vec<(String, f64)> {
    let spec = s.execute("bspec.get", &json!({})).expect("bspec.get");
    spec["spec"]["levels"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|l| {
                    let elevation = l["elevation_ft"].as_f64().or_else(|| l["elevation"].as_f64()).unwrap_or(f64::NAN);
                    (l["name"].as_str().unwrap_or("").to_string(), elevation)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The answer the script gives: 12' floor to floor, else what the job offers.
fn script_answer(q: &Value) -> Value {
    let text = q["question"].as_str().unwrap_or("");
    if text.contains("floor-to-floor") {
        return json!("12'-0\"");
    }
    q["default"].as_str().map_or_else(|| q["choices"][0].clone(), |d| json!(d))
}

#[test]
fn a_scripted_job_stops_at_each_checkpoint_and_its_answers_change_the_model() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("script");
    let pdfs = dir.display().to_string();

    // Auto mode: today's behaviour, and it says so.
    let mut auto = Session::new();
    let st = auto.execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "auto": true, "checkpoint": false})).expect("auto job");
    assert_eq!(st["done"], json!(true), "{st}");
    let a = &st["answers"];
    assert_eq!((a["mode"].as_str(), a["by_you"].as_u64()), (Some("auto"), Some(0)), "{a}");
    let automatic = a["automatic"].as_u64().expect("automatic");
    assert!(automatic > 0 && a["note"].as_str().is_some_and(|n| n.contains(&format!("{automatic} answered automatically, 0 by you"))), "{a}");
    let auto_levels = levels(&mut auto);
    assert!(auto_levels.len() >= 2, "{auto_levels:?}");

    // Interactive (the default): job.start stops at the first checkpoint, and job.next gives it.
    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "checkpoint": false})).expect("job.start");
    assert_eq!(st["done"], json!(false), "the job waits for an answer: {st}");
    assert_eq!(st["answers"]["mode"], "interactive", "{st}");
    let first = st["question"]["id"].as_str().expect("job.start says what it asks").to_string();
    let next = s.execute("job.next", &json!({})).expect("job.next");
    assert_eq!(next["question"]["id"].as_str(), Some(first.as_str()), "{next}");
    // Waiting is waiting: asking again doesn't move the job on.
    let again = s.execute("job.next", &json!({})).expect("job.next");
    assert_eq!(again["question"]["id"], next["question"]["id"]);

    let mut asked = Vec::new();
    let mut floor = None;
    for _ in 0..400 {
        let n = s.execute("job.next", &json!({})).expect("job.next");
        if n["done"] == json!(true) {
            break;
        }
        let q = n["question"].clone();
        assert!(q.is_object(), "a job that isn't done has a question: {n}");
        let id = q["id"].as_str().expect("id").to_string();
        assert!(!asked.contains(&id), "{id} asked twice");
        asked.push(id.clone());
        let r = s.execute("job.answer", &json!({"id": id, "answer": script_answer(&q)})).expect("job.answer");
        assert!(r["effect"].is_string(), "{r}");
        if q["question"].as_str().is_some_and(|t| t.contains("floor-to-floor")) {
            floor = Some(r);
        }
    }
    let st = s.execute("job.status", &json!({})).expect("status");
    assert_eq!(st["done"], json!(true), "{st}");
    let a = &st["answers"];
    assert_eq!(a["by_you"].as_u64(), Some(asked.len() as u64), "every question it stopped at was answered by the script: {a}");
    assert!(a["automatic"].as_u64().is_some_and(|n| n < automatic), "fewer defaults than auto mode: {a} vs {automatic}");
    eprintln!("interactive: {} (auto mode: {automatic} automatic)", a["note"]);

    // The 12' floor to floor reached the model: the levels stack at 12', not the 10' guess.
    let floor = floor.expect("the job asked for the floor to floor height");
    assert_eq!(floor["applies_to"], "bim_build", "{floor}");
    let got = levels(&mut s);
    assert_eq!(got.len(), auto_levels.len(), "{got:?}");
    assert_eq!(got.get(1).map(|l| l.1), Some(12.0), "{got:?} vs auto {auto_levels:?}");
    assert_eq!(auto_levels.get(1).map(|l| l.1), Some(10.0), "{auto_levels:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_answer_to_a_stage_that_ran_runs_it_again_and_says_what_changed() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("rerun");
    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": true, "checkpoint": false})).expect("job");
    assert_eq!(st["done"], json!(true), "{st}");
    let count =
        |s: &mut Session| s.execute("job.explain", &json!({"stage": "rooms"})).expect("explain")["result"]["rooms"].as_array().map_or(0, Vec::len);
    let rooms = count(&mut s);
    assert!(rooms > 1, "rooms read: {rooms}");
    // A room spot check the job took by default: the user says it isn't a room.
    let list = s.execute("job.worklist", &json!({"stage": "rooms"})).expect("worklist");
    let item = list["items"]
        .as_array()
        .and_then(|a| a.iter().find(|i| i["kind"] == "question" && i["summary"].as_str().is_some_and(|t| t.contains("name and area right"))))
        .cloned()
        .expect("a rooms spot check on the worklist");
    assert_eq!(item["status"], "resolved", "taken by default: {item}");
    let r = s.execute("job.answer", &json!({"id": item["id"], "answer": "Not a room"})).expect("answer");
    let rerun: Vec<&str> = r["rerun"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    assert_eq!(rerun.first(), Some(&"rooms"), "{r}");
    assert!(rerun.contains(&"bim_build"), "the stages after it run again too: {r}");
    let changed = r["changed"].as_array().cloned().unwrap_or_default();
    assert!(
        changed.iter().any(|c| c["stage"] == "rooms" && c["what"] == "rooms.len" && c["before"] == json!(rooms) && c["after"] == json!(rooms - 1)),
        "{r}"
    );
    assert_eq!(count(&mut s), rooms - 1);
    assert_eq!(r["done"], json!(true), "an auto job goes on to the end: {r}");
    // The answer is the user's, and stays answered through the re-run.
    assert_eq!(r["answers"]["by_you"].as_u64(), Some(1), "{r}");
    let after = s.execute("job.worklist", &json!({"status": "pending"})).expect("worklist");
    assert!(after["items"].as_array().is_some_and(|a| a.iter().all(|i| i["kind"] != "question")), "{after}");
    let _ = std::fs::remove_dir_all(&dir);
}
