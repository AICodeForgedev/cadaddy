//! Answering the guided job's questions is safe and does what it says (Q6008): answers sent
//! together stand alone and run their stages once, a running job switches to auto, level
//! questions take an elevation and a name, and each question says what an answer re-runs.

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
    let dir = std::env::temp_dir().join(format!("cadaddy_jobanswers_{tag}_{}_{nanos}", std::process::id()));
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

/// The open questions of `stage`, in the order the job asks them.
fn open_questions(s: &mut Session, stage: &str) -> Vec<String> {
    let w = s.execute("job.worklist", &json!({"status": "pending", "stage": stage})).expect("job.worklist");
    w["items"]
        .as_array()
        .map(|a| a.iter().filter(|i| i["kind"] == "question").filter_map(|i| i["id"].as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

/// The text of question `id`.
fn question_text(s: &mut Session, id: &str) -> String {
    let w = s.execute("job.worklist", &json!({})).expect("job.worklist");
    w["items"].as_array().and_then(|a| a.iter().find(|i| i["id"] == id)).and_then(|i| i["summary"].as_str()).unwrap_or("").to_string()
}

fn level(levels: &Value, name: &str) -> Value {
    levels["levels"].as_array().and_then(|a| a.iter().find(|l| l["name"] == name)).cloned().unwrap_or(Value::Null)
}

#[test]
fn answers_stand_alone_run_their_stage_once_and_the_job_can_go_on_unattended() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("batch");
    let pdfs = dir.display().to_string();
    let mut s = Session::new();
    let st = s.execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "checkpoint": false})).expect("job.start");
    assert_eq!(st["current_stage"], "levels", "{st}");

    // MUST 1 + 3: the level questions in one call, with an id that isn't there. Level 2 takes a
    // typed elevation, level 1 is confirmed, the floor to floor is 12'.
    let ids = open_questions(&mut s, "levels");
    assert_eq!(ids.len(), 3, "{ids:?}");
    let mut answers = vec![json!({"id": "Q-levels-999", "answer": "Correct"})];
    for id in &ids {
        let text = question_text(&mut s, id);
        let answer = if text.contains("floor-to-floor") {
            json!("12'-0\"")
        } else if text.contains("\"2\"") {
            json!("Level 2 at 13'-0\"")
        } else {
            json!("Correct")
        };
        answers.push(json!({"id": id, "answer": answer}));
    }
    let r = s.execute("job.answer", &json!({"answers": answers})).expect("a bad id doesn't fail the call");
    assert_eq!(r["answered"].as_array().map(Vec::len), Some(3), "{r}");
    let w = r["warnings"].as_array().expect("warnings");
    assert_eq!((w.len(), w[0]["id"].as_str()), (1, Some("Q-levels-999")), "{r}");
    assert!(r["effect"].as_array().is_some_and(|e| e.iter().any(|x| x["effect"].as_str().is_some_and(|t| t.contains("level 2 is at 13'")))), "{r}");
    assert_eq!(r["applies_to"], "bim_build", "the build hasn't run: it reads the answers when it does: {r}");
    let st = s.execute("job.status", &json!({})).expect("status");
    assert_eq!(st["current_stage"], "rooms", "{st}");
    assert_eq!(st["answers"]["by_you"].as_u64(), Some(3), "the three good answers are kept: {st}");

    // MUST 4: a rooms question says what an answer re-runs and about how long.
    let rooms = open_questions(&mut s, "rooms");
    assert!(rooms.len() >= 3, "{rooms:?}");
    let next = s.execute("job.next", &json!({})).expect("job.next");
    let ic = &next["question"]["if_changed"];
    assert_eq!(ic["reruns"], json!(["rooms"]), "{next}");
    assert!(ic["about_s"].as_f64().is_some_and(|t| t >= 0.0) && ic["note"].as_str().is_some_and(|n| n.contains("run it once")), "{next}");

    // Two renames and the rest confirmed in one call: the rooms stage runs again once.
    let mut answers = Vec::new();
    for (i, id) in rooms.iter().enumerate() {
        let answer = match i {
            0 => json!("SUITE ONE"),
            1 => json!("SUITE TWO"),
            _ => json!("Correct"),
        };
        answers.push(json!({"id": id, "answer": answer}));
    }
    let r = s.execute("job.answer", &json!({"answers": answers})).expect("rooms answers");
    assert_eq!(r["rerun"], json!(["rooms"]), "{r}");
    assert!(r.get("warnings").is_none(), "{r}");
    let rules = s.execute("job.rules", &json!({})).expect("rules");
    let values: Vec<&str> = rules["rules"].as_array().map(|a| a.iter().filter_map(|r| r["value"].as_str()).collect()).unwrap_or_default();
    assert!(values.contains(&"name:SUITE ONE") && values.contains(&"name:SUITE TWO"), "{rules}");
    // The rooms asked again keep their answers: the job goes on to the devices.
    for _ in 0..20 {
        let st = s.execute("job.status", &json!({})).expect("status");
        if st["current_stage"] != "rooms" {
            break;
        }
        let rest = open_questions(&mut s, "rooms");
        let all: Vec<Value> = rest.iter().map(|id| json!({"id": id, "answer": "Correct"})).collect();
        s.execute("job.answer", &json!({"answers": all})).expect("rest of rooms");
    }
    let st = s.execute("job.status", &json!({})).expect("status");
    assert_eq!(st["current_stage"], "devices", "{st}");

    // rerun: false holds the re-run; job.next runs it.
    let devices = open_questions(&mut s, "devices");
    let verify = devices.iter().find(|id| question_text(&mut s, id).contains("type right?")).expect("a device spot check").clone();
    let r = s.execute("job.answer", &json!({"id": verify, "answer": "Not a device", "rerun": false})).expect("held");
    assert_eq!((r["rerun"].clone(), r["rerun_held"]["from"].clone()), (json!([]), json!("devices")), "{r}");
    assert_eq!(s.execute("job.status", &json!({})).expect("status")["rerun_held"], "devices");
    let n = s.execute("job.next", &json!({})).expect("job.next");
    assert_eq!(n["rerun"]["stages"].as_array().and_then(|a| a.first()).cloned(), Some(json!("devices")), "{n}");
    assert!(s.execute("job.status", &json!({})).expect("status").get("rerun_held").is_none());

    // MUST 2: a running job can't be started again in auto mode; it switches.
    let e = s.execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "auto": true, "checkpoint": false})).expect_err("running");
    assert!(e.to_string().contains("job.set_mode {auto: true}"), "{e}");
    let by_you = s.execute("job.status", &json!({})).expect("status")["answers"]["by_you"].as_u64().expect("by_you");
    let st = s.execute("job.set_mode", &json!({"auto": true})).expect("job.set_mode");
    assert_eq!(st["done"], json!(true), "{st}");
    assert_eq!(st["mode"], json!({"from": "interactive", "to": "auto"}), "{st}");
    assert_eq!(st["answers"]["mode"], "auto");
    assert_eq!(st["answers"]["by_you"].as_u64(), Some(by_you), "the answers given are kept: {st}");
    assert!(s.execute("job.set_mode", &json!({})).is_err(), "auto is required");

    // The level answers reached the model: level 2 at the typed 13', level 1 reaching it, both
    // the user's.
    let levels = s.execute("levels.list", &json!({})).expect("levels.list");
    let (l1, l2) = (level(&levels, "Level 1"), level(&levels, "Level 2"));
    assert_eq!((l1["elevation_ft"].as_f64(), l2["elevation_ft"].as_f64()), (Some(0.0), Some(13.0)), "{levels}");
    assert_eq!(l1["height_ft"].as_f64(), Some(13.0), "{levels}");
    assert_eq!((l1["source"].as_str(), l2["source"].as_str()), (Some("user"), Some("user")), "{levels}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A level renamed after the model is built is renamed in the model at once, and stays renamed
/// when the build runs again.
#[test]
fn a_level_name_answer_renames_the_built_level() {
    let _g = serial();
    real_pdf_reader();
    let dir = job_folder("rename");
    let pdfs = dir.display().to_string();
    let mut s = Session::new();
    s.execute("job.start", &json!({"pdfs": pdfs, "goal": "bim", "checkpoint": false})).expect("job.start");
    let ids = open_questions(&mut s, "levels");
    let level_two = ids.iter().find(|id| question_text(&mut s, id).contains("\"2\"")).expect("level 2's question").clone();
    // Run to the end on the defaults, then name level 2.
    let st = s.execute("job.set_mode", &json!({"auto": true})).expect("auto");
    assert_eq!(st["done"], json!(true), "{st}");
    let r = s.execute("job.answer", &json!({"id": level_two, "answer": {"name": "Mezzanine"}})).expect("rename");
    assert_eq!(r["effect"], "level 2 is named Mezzanine", "{r}");
    assert!(r["levels_changed"].as_array().is_some_and(|c| c.iter().any(|x| x == "Level 2 renamed Mezzanine")), "{r}");
    let levels = s.execute("levels.list", &json!({})).expect("levels.list");
    assert_eq!(level(&levels, "Mezzanine")["source"], "user", "{levels}");
    assert!(level(&levels, "Level 2").is_null(), "{levels}");
    let _ = std::fs::remove_dir_all(&dir);
}
