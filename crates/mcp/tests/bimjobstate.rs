//! Q5276 through the MCP tools, as the blind test met it: `bim_job import` then the `next` it
//! suggests (`bim_job build {}`) works, every step is kept in the print folder, and a new MCP
//! session resumes the job with no memory of the last one.

use cadaddy_mcp::{Headless, Server};
use cadaddy_mepfix::generate_hotel;
use cadaddy_mepfix::hotel::HotelSize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn rpc(s: &mut Server, method: &str, params: Value) -> Value {
    let line = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }).to_string();
    serde_json::from_str(&s.handle_line(&line).expect("a reply")).expect("JSON")
}

/// A tool's answer, or its error text.
fn call(s: &mut Server, name: &str, args: Value) -> Result<Value, String> {
    let r = rpc(s, "tools/call", json!({ "name": name, "arguments": args }));
    let text = r["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string();
    if r.get("error").is_some() || r["result"]["isError"] == true {
        return Err(if text.is_empty() { r["error"].to_string() } else { text });
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

/// A new MCP session (a new in-process CADaddy), whose last-print-set note is the test's own.
fn server(dir: &Path) -> Server {
    use cadaddy_engine::cmd::file::{IoHooks, set_io};
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
        read_dwf: None,
        write_dwfx: None,
    });
    let mut h = Headless::default();
    h.session.printset_pointer = Some(dir.join("pointer.txt"));
    let mut s = Server::new(Box::new(h));
    rpc(&mut s, "initialize", json!({ "protocolVersion": "2025-06-18" }));
    s
}

/// A folder of the made-up hotel's prints (a few sheets, one PDF each), and a folder beside it
/// for the session's own settings.
fn prints(tag: &str) -> (PathBuf, PathBuf) {
    let set = generate_hotel(42, HotelSize::Small).expect("generate hotel");
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let root = std::env::temp_dir().join(format!("cadaddy_mcp_bimjobstate_{tag}_{}_{nanos}", std::process::id()));
    let dir = root.join("prints");
    std::fs::create_dir_all(&dir).expect("folder");
    for sh in set.truth.sheets.iter().filter(|s| ["A-101", "A-102", "M-101"].contains(&s.number.as_str())) {
        std::fs::write(dir.join(&sh.pdf), set.file(&sh.pdf).expect("sheet pdf")).expect("write sheet");
    }
    (root, dir)
}

/// `"bim_job build {}"` as a tool call.
fn as_call(hint: &str) -> (String, Value) {
    let mut words = hint.splitn(3, ' ');
    let tool = words.next().unwrap_or_default().to_string();
    let action = words.next().unwrap_or_default().to_string();
    (tool, json!({"action": action, "params": {}}))
}

#[test]
fn import_then_the_suggested_build_works_and_a_new_session_resumes_the_job() {
    let (root, dir) = prints("resume");
    let mut s = server(&root);
    let imported = call(&mut s, "bim_job", json!({"action": "import", "params": {"path": dir.display().to_string()}})).expect("import");
    let hint = imported["next"].as_str().expect("next").to_string();
    assert_eq!(hint, "bim_job build {}");
    // The suggested next step, as given.
    let (tool, args) = as_call(&hint);
    let built = call(&mut s, &tool, args).expect("the suggested build");
    assert!(built["drafted"].is_object(), "{built}");
    assert!(built.get("not_kept").is_none(), "{built}");
    call(&mut s, "bim_job", json!({"action": "note", "params": {"text": "Level 1 walls checked; ducts next"}})).expect("note");
    let before = call(&mut s, "bim_job", json!({"action": "briefing"})).expect("briefing");
    assert!(dir.join(".cadaddy").join("project.cdy").is_file(), "kept in the print folder");
    drop(s);

    // A new MCP session: the agent remembers nothing and starts with the briefing.
    let mut s = server(&root);
    let after = call(&mut s, "bim_job", json!({"action": "briefing"})).expect("briefing");
    assert!(after["resumed"].is_object(), "{after}");
    assert!(!after["text"].as_str().unwrap_or_default().contains("Nothing built yet"), "{after}");
    assert_eq!(after["revision"], before["revision"]);
    assert_eq!(after["built"], before["built"]);
    assert!(after["notes"].to_string().contains("ducts next"), "{after}");
    // A third session after explicit `resume {path}` gets the same.
    let mut s3 = server(&root);
    let r = call(&mut s3, "bim_job", json!({"action": "resume", "params": {"path": dir.display().to_string()}})).expect("resume");
    assert_eq!(r["resumed"], json!(true), "{r}");

    // Every `next` the briefing gives runs as given through bim_job.
    let mut ran = Vec::new();
    let mut brief = after;
    for _ in 0..6 {
        let next = brief["next"]["bim_job"].clone();
        assert!(next.is_object(), "a runnable next: {brief}");
        let action = next["action"].as_str().unwrap_or_default().to_string();
        call(&mut s, "bim_job", next.clone()).unwrap_or_else(|e| panic!("{next} as given: {e}"));
        ran.push(action.clone());
        if action == "takeoff" {
            break;
        }
        if action == "review_next" {
            // The queue is the agent's to work: defer it so the briefing moves on.
            let list = call(&mut s, "bim_job", json!({"action": "review_list", "params": {"limit": 50}})).expect("list");
            for i in list["items"].as_array().into_iter().flatten() {
                call(&mut s, "bim_job", json!({"action": "defer", "params": {"id": i["id"], "reason": "field to verify"}})).expect("defer");
            }
        }
        brief = call(&mut s, "bim_job", json!({"action": "briefing"})).expect("briefing");
    }
    assert!(!ran.is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_build_right_after_printset_open_works() {
    let (root, dir) = prints("open");
    let mut s = server(&root);
    // A new session with nothing kept: the briefing's next is to open the prints.
    let b = call(&mut s, "bim_job", json!({"action": "briefing"})).expect("briefing");
    assert_eq!(b["next"]["bim_job"]["action"], "import", "{b}");
    call(&mut s, "execute", json!({"command": "printset.open", "params": {"path": dir.display().to_string()}})).expect("printset.open");
    let b = call(&mut s, "bim_job", json!({"action": "briefing"})).expect("briefing");
    assert_eq!(b["next"]["bim_job"]["action"], "build", "{b}");
    let built = call(&mut s, "bim_job", json!({"action": "build"})).expect("build right after printset.open");
    assert!(built["drafted"].is_object(), "{built}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_guided_job_is_kept_and_a_new_session_resumes_it() {
    let (root, dir) = prints("guided");
    let mut s = server(&root);
    // With no job, the status hint names the call that starts one: run it as given.
    let st = call(&mut s, "guided_job", json!({"action": "status"})).expect("status");
    assert!(st["hint"].as_str().is_some_and(|h| h.contains("job.start {pdfs, goal}")), "{st}");
    call(&mut s, "guided_job", json!({"action": "start", "params": {"pdfs": dir.display().to_string(), "goal": "bim", "checkpoint": false}}))
        .expect("start");
    let status = call(&mut s, "guided_job", json!({"action": "status"})).expect("status");
    drop(s);
    let mut s = server(&root);
    let again = call(&mut s, "guided_job", json!({"action": "status"})).expect("status");
    assert!(again["resumed"].is_object(), "{again}");
    assert_eq!(again["job_id"], status["job_id"], "the same job: {again}");
    let _ = std::fs::remove_dir_all(&root);
}
