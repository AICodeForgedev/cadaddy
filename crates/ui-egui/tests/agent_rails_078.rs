//! Agent safety rails over the control channel (Q211): a dry run changes nothing, batches
//! included; saving over a file waits for the user's answer, inside a batch too, and runs once
//! allowed; the change journal lists each request with what it did.

use cadaddy_engine::Session;
use cadaddy_ui_egui::control::{ControlRequest, Outcome, handle};
use cadaddy_ui_egui::{CadApp, Services, agentdlg};
use serde_json::{Value, json};

fn call(app: &mut CadApp, method: &str, params: Value) -> Value {
    let (req, _reply) = ControlRequest::new(method, params);
    match handle(app, &egui::Context::default(), &req) {
        Outcome::Done(v) => v,
        Outcome::Screenshot { .. } => Value::Null,
    }
}

fn exec(app: &mut CadApp, command: &str, params: Value) -> Value {
    call(app, "engine.execute", json!({ "command": command, "params": params }))
}

fn many(app: &mut CadApp, commands: Value) -> Value {
    call(app, "engine.executeMany", json!({ "commands": commands }))
}

fn app() -> CadApp {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    CadApp::new(Session::new(), Services::default())
}

fn objects(app: &CadApp) -> usize {
    app.session.doc().unwrap().model.len()
}

fn folder(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-rails078-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn circles(n: usize) -> Value {
    Value::Array((0..n).map(|i| json!({ "command": "circle", "params": { "center": [i as f64 * 3.0, 0.0], "radius": 1.0 } })).collect())
}

#[test]
fn a_dry_run_changes_nothing_batches_included() {
    let mut app = app();
    let dir = folder("dry");
    let before = objects(&app);
    let undo = app.session.state().unwrap().undo.len();
    exec(&mut app, "agent.dryrun", json!({ "on": true }));
    let r = exec(&mut app, "circle", json!({ "center": [0, 0], "radius": 1 }));
    assert_eq!(r["result"]["dryRun"], true, "{r}");
    let r = many(&mut app, circles(1000));
    assert_eq!(objects(&app), before, "a batch in a dry run changes nothing: {}", r.to_string().chars().take(300).collect::<String>());
    let path = dir.join("dry.dxf");
    many(&mut app, json!([{ "command": "saveas", "params": { "path": path } }]));
    assert!(!path.exists(), "no file written");
    assert_eq!(app.session.state().unwrap().undo.len(), undo);
    exec(&mut app, "agent.dryrun", json!({ "on": false }));
    many(&mut app, circles(10));
    assert_eq!(objects(&app), before + 10, "and off again, batches change the drawing");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn saving_over_a_file_waits_for_the_user_inside_a_batch_too_and_runs_once_allowed() {
    let mut app = app();
    let dir = folder("confirm");
    let path = dir.join("theirs.dxf");
    std::fs::write(&path, b"the user's file").unwrap();
    exec(&mut app, "circle", json!({ "center": [0, 0], "radius": 1 }));
    // Inside a batch: the save waits; nothing is written.
    let r = many(
        &mut app,
        json!([{ "command": "circle", "params": { "center": [5, 5], "radius": 1 } }, { "command": "saveas", "params": { "path": path } }]),
    );
    assert!(r.to_string().contains("Waiting for the user"), "{}", r.to_string().chars().take(300).collect::<String>());
    assert_eq!(std::fs::read(&path).unwrap(), b"the user's file");
    // On its own: the same, and the question stays until the user answers.
    let r = exec(&mut app, "saveas", json!({ "path": path }));
    assert!(r["error"].as_str().unwrap_or_default().contains("Waiting for the user"), "{r}");
    assert!(app.session.agent.pending.is_some());
    assert_eq!(std::fs::read(&path).unwrap(), b"the user's file", "still waiting");
    // The user allows it: it runs, as the user's own.
    agentdlg::answer_pending(&mut app, true);
    assert!(app.session.agent.pending.is_none());
    assert_ne!(std::fs::read(&path).unwrap(), b"the user's file", "saved once allowed");
    // Denied: nothing happens.
    std::fs::write(&path, b"again").unwrap();
    exec(&mut app, "saveas", json!({ "path": path }));
    agentdlg::answer_pending(&mut app, false);
    assert_eq!(std::fs::read(&path).unwrap(), b"again");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_journal_lists_each_request_with_what_it_did() {
    let mut app = app();
    for (label, n) in [("Columns", 3usize), ("Grid", 5)] {
        exec(&mut app, "agent.begin", json!({ "label": label }));
        many(&mut app, circles(n));
        exec(&mut app, "agent.end", json!({}));
    }
    let j = exec(&mut app, "agent.journal", json!({}));
    let entries = j["result"]["entries"].as_array().unwrap();
    let got: Vec<(String, u64)> = entries.iter().map(|e| (e["label"].as_str().unwrap().to_string(), e["counts"][0].as_u64().unwrap())).collect();
    assert_eq!(got, [("Grid".to_string(), 5), ("Columns".to_string(), 3)], "newest first: {j}");
}
