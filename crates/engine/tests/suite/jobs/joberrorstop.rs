//! Q6600: a guided job that a stage's error stopped, with no question open, has ended there. Its
//! `job.next` used to answer `waiting: true` for ever (a client polling until the job ends never
//! stopped); now `job.next`, `job.status` and `job.answer` say it stopped (terminal), at which stage
//! and why, and never that it is waiting or done. Made-up folders only.

use crate::helper::{scratch, serial};
use cadaddy_engine::Session;
use serde_json::{Value, json};

/// The terminal answer: stopped at `stage`, with its error, never waiting or done.
fn assert_stopped(v: &Value, stage: &str, why: &str, what: &str) {
    assert_ne!(v["waiting"], json!(true), "{what}: never waiting with nothing open: {v}");
    assert_ne!(v["done"], json!(true), "{what}: nothing was built after the error, not done: {v}");
    assert_eq!(v["status"], json!("error"), "{what}: {v}");
    assert_eq!(v["stopped"]["stage"], json!(stage), "{what}: {v}");
    assert_eq!(v["stopped"]["terminal"], json!(true), "{what}: {v}");
    assert!(v["stopped"]["message"].as_str().is_some_and(|m| m.contains(why)), "{what}: says why: {v}");
    assert!(v["stopped"]["how_to_go_on"].as_array().is_some_and(|a| !a.is_empty()), "{what}: how to go on: {v}");
}

/// A prints folder with no drawing file stops the job at its import: for every goal and both
/// modes, `job.next` asked again and again answers the same terminal stop, `job.status` says it,
/// and another `job.start` in the drawing says the job stopped (not that one is running).
#[test]
fn a_job_an_error_stopped_has_ended_and_never_waits() {
    let _g = serial();
    for goal in ["bim", "takeoff", "estimate"] {
        for auto in [true, false] {
            let what = format!("{goal}, auto {auto}");
            let dir = scratch(&format!("joberrorstop-{goal}-{auto}"));
            let mut s = Session::new();
            let params = json!({"pdfs": dir.display().to_string(), "goal": goal, "auto": auto, "checkpoint": false, "resume": false});
            let start = s.execute("job.start", &params).unwrap_or_else(|e| panic!("{what}: job.start: {e}"));
            assert_stopped(&start, "import", "no drawing files found", &format!("{what}: job.start"));
            assert_eq!(start["done"], json!(false), "{what}: {start}");
            // Every next answers the same: stopped, with the error (as a string, as before).
            for poll in 0..5 {
                let n = s.execute("job.next", &json!({})).unwrap_or_else(|e| panic!("{what}: job.next: {e}"));
                assert_stopped(&n, "import", "no drawing files found", &format!("{what}: job.next #{poll}"));
                assert!(n["error"].as_str().is_some_and(|e| e.contains("no drawing files found")), "{what}: {n}");
                assert!(n.get("question").is_none(), "{what}: {n}");
            }
            let st = s.execute("job.status", &json!({})).unwrap_or_else(|e| panic!("{what}: job.status: {e}"));
            assert_stopped(&st, "import", "no drawing files found", &format!("{what}: job.status"));
            assert_eq!(st["open_questions"], json!(0), "{what}: {st}");
            assert_eq!(st["error"]["stage"], json!("import"), "{what}: {st}");
            // Another job here: refused, saying the job stopped and where.
            let e = s.execute("job.start", &params).expect_err("one job a drawing");
            assert!(e.to_string().contains("stopped at import"), "{what}: {e}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// A stage that ends on an error while its question is open still asks it (the job waits for the
/// answer, not for ever); a job still at a question is not stopped.
#[test]
fn a_job_at_a_question_is_neither_stopped_nor_waiting() {
    use cadaddy_engine::cmd::guided::*;
    let mut job = GuidedJob::new("/tmp/made-up-prints", JobGoal::Bim);
    if let Some(st) = job.stage_mut(StageName::Import) {
        st.status = StageStatus::Error;
        st.error = Some("made-up failure".into());
    }
    assert_eq!(job.stopped_at().map(|st| st.name), Some(StageName::Import));
    assert!(stopped_value(&job).is_some_and(|v| v["stage"] == "import" && v["message"] == "made-up failure"));
    // Earlier stages done, an open question in the stage: not stopped.
    let qid = job.next_qid(StageName::Import);
    if let Some(st) = job.stage_mut(StageName::Import) {
        st.questions.push(Question {
            id: qid,
            stage: StageName::Import,
            question: "Is this right?".into(),
            why: "made-up".into(),
            choices: vec!["Yes".into(), "No".into()],
            evidence: None,
            default: Some("Yes".into()),
            answer_schema: None,
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: QuestionKind::Doubt,
        });
    }
    assert!(job.stopped_at().is_none() && stopped_value(&job).is_none());
    // A running stage isn't stopped either.
    let mut job = GuidedJob::new("/tmp/made-up-prints", JobGoal::Bim);
    if let Some(st) = job.stage_mut(StageName::Import) {
        st.status = StageStatus::Running;
    }
    assert!(job.stopped_at().is_none());
}
