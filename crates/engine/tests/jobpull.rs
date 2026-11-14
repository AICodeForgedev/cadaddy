//! A job asks for the project's latest documents (Q6618): at its start and before its takeoff
//! when the last pull is too old, saying what to pull and how to hand it back; a finding newer
//! documents could answer carries the request; a pulled manifest re-reads only the sheets it
//! changed, closes the findings they answer and names the sheet revision; "skip" (and auto mode)
//! goes on with the report saying the latest is not confirmed; a project with no source never
//! asks. A program of its own: it installs the PDF reader (process-wide hooks).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use cadaddy_engine::Session;
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};

const PULL: &str = "Pull the latest documents";

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

/// Whole jobs, one at a time (the PDF reader hooks are process-wide).
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Seconds since 1970 as an ISO 8601 UTC time.
fn iso(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// A made-up project: the hotel's two floor plans and the level 1 lighting plan in `drawings/`,
/// listed in a manifest from a source pulled two days ago. The level 2 lighting plan is kept
/// aside: the "newer document" a pull brings.
struct Fixture {
    dir: PathBuf,
    prints: PathBuf,
    cfg: PathBuf,
    later: Vec<u8>,
    pulled: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        let _ = std::fs::remove_dir_all(&self.cfg);
    }
}

fn fixture(tag: &str, kind: &str) -> Fixture {
    let set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_jobpull_{tag}_{}_{nanos}", std::process::id()));
    let prints = dir.join("drawings");
    std::fs::create_dir_all(&prints).expect("folder");
    let mut later = Vec::new();
    let mut found = Vec::new();
    for sh in &set.truth.sheets {
        if !["A-101", "A-102", "E-101", "E-102"].contains(&sh.number.as_str()) {
            continue;
        }
        let bytes = set.file(&sh.pdf).expect("sheet pdf").to_vec();
        if sh.number == "E-102" {
            later = bytes;
        } else {
            std::fs::write(prints.join(format!("{}.pdf", sh.number)), bytes).expect("write sheet");
        }
        found.push(sh.number.clone());
    }
    assert_eq!(found.len(), 4, "the made-up hotel has both floor plans and both lighting plans: {found:?}");
    let pulled = iso(now() - 2 * 86_400);
    let f = Fixture { cfg: dir.with_extension("cfg"), dir, prints, later, pulled };
    write_manifest(&f.dir, "Issued set 1", kind, &f.pulled, &[("A-101", "0"), ("A-102", "0"), ("E-101", "0")]);
    f
}

fn write_manifest(dir: &Path, name: &str, kind: &str, pulled: &str, sheets: &[(&str, &str)]) {
    let files: Vec<Value> = sheets
        .iter()
        .map(|(n, rev)| {
            let mut f = json!({
                "path": format!("drawings/{n}.pdf"), "kind": "sheet", "title": n, "revision": rev, "date": "2026-10-01",
                "discipline": if n.starts_with('E') { "Electrical" } else { "Architectural" },
                "sourceId": format!("doc-{n}"), "pulled": pulled,
            });
            if *n == "E-102" {
                f["links"] = json!({"asi": ["ASI-1"]});
            }
            f
        })
        .collect();
    let m = json!({
        "name": name,
        "source": {"kind": kind, "platform": "Example CM", "reference": "P-100", "pulled": pulled},
        "files": files,
    });
    std::fs::write(dir.join("manifest.json"), serde_json::to_vec_pretty(&m).expect("json")).expect("write manifest");
}

fn session(f: &Fixture) -> Session {
    let mut s = Session::new();
    s.project_config = Some(f.cfg.clone());
    let r = s.execute("project.import_manifest", &json!({"path": f.dir.display().to_string()})).expect("import");
    assert_eq!(r["added"], json!(3), "{r}");
    s
}

/// The answer the script gives every other question: 12' floor to floor, else the default.
fn script_answer(q: &Value) -> Value {
    if q["question"].as_str().is_some_and(|t| t.contains("floor-to-floor")) {
        return json!("12'-0\"");
    }
    q["default"].as_str().map_or_else(|| q["choices"][0].clone(), |d| json!(d))
}

/// Answer every question but a pull request with its default: the pull request it stops at, or
/// null once the job is done.
fn answer_until_pull(s: &mut Session) -> Value {
    answer_counting(s, &mut Vec::new())
}

/// As [`answer_until_pull`], noting each question answered (its text).
fn answer_counting(s: &mut Session, asked: &mut Vec<String>) -> Value {
    for _ in 0..400 {
        let n = s.execute("job.next", &json!({})).expect("job.next");
        if n["done"] == json!(true) {
            return Value::Null;
        }
        let q = n["question"].clone();
        assert!(q.is_object(), "a job that isn't done has a question: {n}");
        if q["question"].as_str().is_some_and(|t| t.starts_with(PULL)) {
            return q;
        }
        let id = q["id"].as_str().expect("id").to_string();
        asked.push(q["question"].as_str().unwrap_or("").to_string());
        s.execute("job.answer", &json!({"id": id, "answer": script_answer(&q)})).expect("job.answer");
    }
    panic!("the job asked more than 400 questions");
}

fn stage<'a>(st: &'a Value, name: &str) -> &'a Value {
    st["stages"].as_array().and_then(|a| a.iter().find(|x| x["stage"] == name)).unwrap_or(&Value::Null)
}

/// The finding that level 2 has no electrical plan, from the worklist.
fn level_two_finding(s: &mut Session) -> Option<Value> {
    let w = s.execute("job.worklist", &json!({})).expect("worklist");
    w["items"]
        .as_array()?
        .iter()
        .find(|i| i["kind"] == "finding" && i["summary"].as_str().is_some_and(|t| t.contains("2 has no electrical (E) plan")))
        .cloned()
}

fn classify_result(s: &mut Session) -> Value {
    s.execute("job.explain", &json!({"stage": "classify"})).expect("explain")["result"].clone()
}

#[test]
fn a_stale_project_is_asked_before_the_build_and_a_pull_rereads_only_the_new_sheet() {
    let _g = serial();
    real_pdf_reader();
    let f = fixture("pull", "platform");
    let mut s = session(&f);

    // job.start stops for the pull before any sheet is read, and says what to pull.
    let pdfs = f.prints.display().to_string();
    let st = s.execute("job.start", &json!({"pdfs": pdfs, "goal": "takeoff", "checkpoint": false, "sheet_check": false})).expect("job.start");
    let q = st["question"].clone();
    assert!(q["question"].as_str().is_some_and(|t| t.starts_with(PULL) && t.contains("Example CM") && t.contains("P-100")), "{q}");
    assert_eq!(stage(&st, "classify")["status"], "Pending", "nothing is read before the answer: {st}");
    let req = &q["evidence"]["values"];
    let what: Vec<&str> = req["what"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    for want in ["drawings", "ASIs", "RFIs with their responses", "submittals with their review status", "specifications"] {
        assert!(what.iter().any(|w| w.contains(want)), "{want} in {what:?}");
    }
    assert_eq!(req["project_reference"], "P-100", "{req}");
    assert_eq!(req["changed_since"], json!(f.pulled), "{req}");
    assert!(req["hand_back"]["manifest"].as_str().is_some_and(|m| m.ends_with("manifest.json")), "{req}");
    assert_eq!(q["default"], "skip");
    assert_eq!(st["documents"]["latest_confirmed"], json!(false), "{st}");

    // Skip: the job goes on with what it has.
    let r = s.execute("job.answer", &json!({"id": q["id"], "answer": "skip"})).expect("skip");
    assert!(r["effect"].as_str().is_some_and(|e| e.contains("not confirmed")), "{r}");

    // Still stale before the takeoff: asked again there, once the model is built.
    let mut before_pull = Vec::new();
    let q2 = answer_counting(&mut s, &mut before_pull);
    assert!(q2["question"].as_str().is_some_and(|t| t.contains("before the takeoff")), "{q2}");
    assert_ne!(q2["id"], q["id"]);
    let before = classify_result(&mut s);
    assert_eq!(before["read"], json!(3), "the first classify read every page: {before}");

    // The plans draw level 2; no electrical sheet does: the finding asks to check the source,
    // naming the level's sheets.
    let finding = level_two_finding(&mut s).expect("level 2 has no electrical plan");
    let cs = &finding["check_source"];
    assert!(cs["request"].as_str().is_some_and(|t| t.contains("newer sheet") && t.contains("RFI response")), "{finding}");
    assert!(cs["sheets"].as_array().is_some_and(|a| a.contains(&json!("A-102"))), "{finding}");

    // The pull brings the level 2 lighting plan (with an ASI): only that sheet is read again.
    std::fs::write(f.prints.join("E-102.pdf"), &f.later).expect("write E-102");
    let today = iso(now());
    write_manifest(&f.dir, "ASI 1", "platform", &today, &[("A-101", "0"), ("A-102", "0"), ("E-101", "0"), ("E-102", "1")]);
    let r = s.execute("job.answer", &json!({"id": q2["id"], "answer": "pulled"})).expect("pulled");
    assert!(r["rerun"].as_array().is_some_and(|a| a.contains(&json!("classify"))), "{r}");
    let effect = r["effect"].as_str().unwrap_or("");
    assert!(effect.contains("1 added") && effect.contains("E-102"), "{r}");
    let after = classify_result(&mut s);
    assert_eq!(after["read"], json!(1), "only the new sheet is read: {after}");
    assert!(after["read_sources"].as_array().is_some_and(|a| a.len() == 1 && a[0].as_str().is_some_and(|t| t.starts_with("E-102.pdf"))), "{after}");

    // The source is fresh now: the job runs to the end without asking again, and what was
    // answered before the pull stays answered (only what the new sheet raises is asked).
    let mut after_pull = Vec::new();
    assert_eq!(answer_counting(&mut s, &mut after_pull), Value::Null, "no pull asked after a pull");
    let again: Vec<&String> = after_pull.iter().filter(|q| before_pull.contains(q)).collect();
    eprintln!("asked before the pull: {}, after: {}, asked again: {}", before_pull.len(), after_pull.len(), again.len());
    assert!(again.is_empty(), "answers given before the pull are kept: asked again {again:?}");
    assert!(level_two_finding(&mut s).is_none(), "the new sheet answered it");
    let report = s.execute("job.report", &json!({})).expect("report");
    let docs = &report["documents"];
    assert_eq!(docs["latest_confirmed"], json!(true), "{docs}");
    assert!(docs["line"].as_str().is_some_and(|l| l.contains(&today[..10]) && l.contains("ASI 1") && !l.contains("not confirmed")), "{docs}");
    let closed = docs["closed_findings"].as_array().cloned().unwrap_or_default();
    let mine = closed.iter().find(|c| c["finding"].as_str().is_some_and(|t| t.contains("2 has no electrical (E) plan"))).expect("closed");
    assert!(mine["line"].as_str().is_some_and(|l| l.contains("E-102 rev 1")), "{mine}");
    assert_eq!(docs["checks"].as_array().map(Vec::len), Some(2), "{docs}");
    eprintln!("closed: {mine}\nline: {}", docs["line"]);

    // The project keeps where every document came from and when.
    let project = std::fs::read_to_string(f.dir.join("project.json")).expect("project.json");
    let p: Value = serde_json::from_str(&project).expect("json");
    assert_eq!(p["source"]["issue"], "ASI 1", "{p}");
    let e102 = p["drawings"].as_array().and_then(|a| a.iter().find(|d| d["path"] == "drawings/E-102.pdf")).cloned().unwrap_or_default();
    assert_eq!(
        (e102["sourceId"].as_str(), e102["pulled"].as_str(), e102["revision"].as_str()),
        (Some("doc-E-102"), Some(today.as_str()), Some("1")),
        "{e102}"
    );
}

#[test]
fn an_unattended_job_never_stops_for_the_pull_and_says_the_latest_is_not_confirmed() {
    let _g = serial();
    real_pdf_reader();
    let f = fixture("auto", "platform");
    let mut s = session(&f);
    let st = s
        .execute(
            "job.start",
            &json!({"pdfs": f.prints.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false, "sheet_check": false}),
        )
        .expect("auto job");
    // It never stops for the pull: what it stops at (always asked) is something else.
    let mut st = st;
    for _ in 0..50 {
        if st["done"] == json!(true) {
            break;
        }
        let q = st["question"].clone();
        assert!(!q["question"].as_str().is_some_and(|t| t.starts_with(PULL)), "auto mode stopped for the pull: {q}");
        st = s.execute("job.answer", &json!({"id": q["id"], "answer": script_answer(&q)})).expect("answer");
        if st.get("next").is_some() {
            st["question"] = st["next"].clone();
        }
    }
    assert_eq!(st["done"], json!(true), "{st}");
    let report = s.execute("job.report", &json!({})).expect("report");
    let docs = &report["documents"];
    let day = &f.pulled[..10];
    assert!(docs["line"].as_str().is_some_and(|l| l.contains(&format!("last pulled on {day}")) && l.contains("latest not confirmed")), "{docs}");
    let checks = docs["checks"].as_array().cloned().unwrap_or_default();
    assert!(!checks.is_empty() && checks.iter().all(|c| c["answer"] == "skip" && c["by"] == "default"), "{docs}");
    // A takeoff on those documents names them, and asks to pull before it.
    let t = s.execute("bim.takeoff", &json!({})).expect("takeoff");
    assert_eq!(t["documents"]["latest_confirmed"], json!(false), "{}", t["documents"]);
    assert_eq!(t["pull_request"]["project_reference"], "P-100", "{}", t["pull_request"]);
}

#[test]
fn a_project_with_no_source_never_asks() {
    let _g = serial();
    real_pdf_reader();
    let f = fixture("none", "none");
    let mut s = session(&f);
    let st = s
        .execute("job.start", &json!({"pdfs": f.prints.display().to_string(), "goal": "takeoff", "checkpoint": false, "sheet_check": false}))
        .expect("job.start");
    assert!(!st["question"]["question"].as_str().is_some_and(|t| t.starts_with(PULL)), "{st}");
    assert_eq!(answer_until_pull(&mut s), Value::Null, "a project with source none is never asked to pull");
    let report = s.execute("job.report", &json!({})).expect("report");
    assert!(report["documents"]["line"].as_str().is_some_and(|l| l.contains("names no source")), "{}", report["documents"]);
}

#[test]
fn a_resumed_job_asks_for_the_latest_before_it_goes_on() {
    let _g = serial();
    real_pdf_reader();
    let f = fixture("resume", "shared drive");
    let ckpt = f.dir.join("job-checkpoint.cdy").display().to_string();
    let params = json!({"pdfs": f.prints.display().to_string(), "goal": "bim", "checkpoint": ckpt, "sheet_check": false});
    {
        let mut s = session(&f);
        let st = s.execute("job.start", &params).expect("job.start");
        let q = st["question"].clone();
        assert!(q["question"].as_str().is_some_and(|t| t.starts_with(PULL)), "{q}");
        let r = s.execute("job.answer", &json!({"id": q["id"], "answer": "skip"})).expect("skip");
        assert!(r["next"].is_object(), "the job stops at a later checkpoint: {r}");
    }
    // Another session goes on from the checkpoint: the documents are still two days old.
    let mut s = Session::new();
    s.project_config = Some(f.cfg.clone());
    let st = s.execute("job.start", &params).expect("resume");
    assert_eq!(st["resumed"], json!(true), "{st}");
    let q = st["question"].clone();
    assert!(q["question"].as_str().is_some_and(|t| t.starts_with(PULL) && t.contains("resumed")), "{q}");
    let r = s.execute("job.answer", &json!({"id": q["id"], "answer": "nothing newer"})).expect("nothing newer");
    assert!(r["effect"].as_str().is_some_and(|e| e.contains("nothing newer")), "{r}");
    let p: Value = serde_json::from_str(&std::fs::read_to_string(f.dir.join("project.json")).expect("project")).expect("json");
    assert!(p["source"]["lastChecked"].as_str().is_some_and(|t| t.starts_with(&iso(now())[..10])), "{p}");
    assert_eq!(p["source"]["kind"], "shared_drive", "{p}");
}
