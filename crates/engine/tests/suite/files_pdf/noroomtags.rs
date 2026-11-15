//! A set whose architectural plans print no room names (Q6668). The made-up hotel's level 1
//! floor plan and its lighting plan with their room lettering taken off: the rooms
//! stage reads none, and asks. An unattended job takes the question's default ("no room tags":
//! the walls, slab and MEP are still built) as an assumption, listed in the report, and runs to
//! the end on its own; attended, the question is asked as before.

use super::paperscale::{FLOOR, LIGHTING, pdf_reader, set_lettering};
use crate::helper::serial;
use cadaddy_engine::Session;
use cadaddy_engine::doc::{Entity, EntityKind};
use serde_json::{Value, json};

/// The room names and numbers on the floor plan.
fn room_lettering(e: &Entity) -> bool {
    matches!(e.kind, EntityKind::Text(_)) && e.common.layer.as_str() == "ARCH-TEXT"
}

fn start(dir: &std::path::Path, auto: bool) -> Value {
    json!({"pdfs": dir.display().to_string(), "goal": "bim", "auto": auto, "checkpoint": false, "specs": false, "sheet_pictures": false})
}

fn is_rooms_question(q: &Value) -> bool {
    q["question"].as_str().is_some_and(|t| t.starts_with("No rooms read on the architectural plans"))
}

#[test]
fn an_unattended_job_assumes_no_room_tags_and_finishes() {
    let _g = serial();
    pdf_reader();
    let (dir, _) = set_lettering("noroomtags", &[FLOOR, LIGHTING], &[FLOOR, LIGHTING], room_lettering);

    // Unattended: no answer given, the job runs to the end.
    let mut s = Session::new();
    s.execute("job.start", &start(&dir, true)).expect("job.start");
    let st = s.execute("job.status", &json!({})).expect("status");
    assert_eq!(st["done"], true, "finished on its own: {st}");
    let assumed = st["assumed_questions"].as_array().cloned().unwrap_or_default();
    let rooms = assumed.iter().find(|q| is_rooms_question(q)).unwrap_or_else(|| panic!("the rooms question is listed as assumed: {st}"));
    assert_eq!(rooms["assumed"], "No room tags on these sheets", "{rooms}");
    assert!(rooms["question"].as_str().is_some_and(|t| t.contains(FLOOR)), "names the plan: {rooms}");
    let report = s.execute("job.report", &json!({})).expect("report");
    let warned = report["warnings"].as_array().and_then(|w| w.iter().find(|w| w["warning"] == "assumed in auto mode").cloned());
    assert!(warned.is_some_and(|w| w["detail"].as_str().is_some_and(|d| d.contains(FLOOR))), "the report says what was assumed: {report}");
    assert!(report["answers"]["assumed"].as_u64().is_some_and(|n| n >= 1), "{}", report["answers"]);
    // The building is still built: its walls stand, its rooms are empty.
    let doc = s.doc().expect("doc");
    let walls = doc.model.iter().filter(|e| cadaddy_engine::cmd::wallgen::made_as(e).as_deref() == Some("wall")).count();
    assert!(walls > 10, "walls built: {walls}");
    let _ = std::fs::remove_dir_all(&dir);

    // Attended: the same question stops the job, always asked, never taken by default.
    let (dir, _) = set_lettering("noroomtags_asked", &[FLOOR, LIGHTING], &[FLOOR, LIGHTING], room_lettering);
    let mut s = Session::new();
    s.execute("job.start", &start(&dir, false)).expect("job.start");
    let mut asked = None;
    for _ in 0..40 {
        let st = s.execute("job.status", &json!({})).expect("status");
        if st["done"] == true {
            break;
        }
        let q = st["question"].clone();
        let Some(id) = q["id"].as_str().map(str::to_string) else { break };
        if is_rooms_question(&q) {
            asked = Some(q);
            break;
        }
        let answer =
            q["default"].as_str().map(str::to_string).or_else(|| q["choices"][0].as_str().map(str::to_string)).unwrap_or_else(|| "Correct".into());
        s.execute("job.answer", &json!({"id": id, "answer": answer})).expect("answer");
    }
    let q = asked.unwrap_or_else(|| panic!("attended, the rooms question is asked"));
    assert_eq!(q["always_asked"], true, "{q}");
    let _ = std::fs::remove_dir_all(&dir);
}
