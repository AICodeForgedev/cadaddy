//! Q6269: a job started from the app runs off the UI thread. The command line hands it over and
//! returns; frames keep coming with a card that shows the job and a Cancel; the control channel
//! answers `ping`, `cmdline.state` and `job.status` and refuses edits as busy; Esc cancels; and the
//! session (the job's state, the history) comes back when it ends. An agent's `engine.execute` of
//! a job gets its reply when the job ends, and the window answers meanwhile.
//!
//! A program of its own: it installs the job progress sink (process-wide). The sink holds the job
//! at its first progress line, as a long sheet import does, until the test lets it go.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cadaddy_engine::Session;
use cadaddy_ui_egui::control::ControlRequest;
use cadaddy_ui_egui::{CadApp, Services};
use serde_json::{Value, json};

static SERIAL: Mutex<()> = Mutex::new(());
static HOLD: AtomicBool = AtomicBool::new(false);
static HELD: AtomicBool = AtomicBool::new(false);

/// The job's progress lines: while HOLD is on, the first one holds the job (at most 20 s).
fn holding_sink(_line: &str) {
    if HOLD.load(Ordering::SeqCst) {
        HELD.store(true, Ordering::SeqCst);
        let t = Instant::now();
        while HOLD.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn prints_folder(tag: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("cadaddy_offui_{tag}_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().replace('\\', "/")
}

/// One frame of the whole window; how long it took.
fn frame(app: &mut CadApp, ctx: &egui::Context) -> Duration {
    let t = Instant::now();
    let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))), ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| {
        app.logic(ui.ctx());
        app.ui(ui);
    });
    out.textures_delta.clear();
    t.elapsed()
}

/// A control request as the app's server hands it over; the reply, or the transport's timeout.
fn call(app: &mut CadApp, ctx: &egui::Context, method: &str, params: Value) -> Value {
    let (req, pending) = ControlRequest::new(method, params);
    app.serve_control_request(ctx, req);
    pending.wait(Duration::from_millis(500))
}

fn hold_until_reached() {
    let t = Instant::now();
    while !HELD.load(Ordering::SeqCst) {
        assert!(t.elapsed() < Duration::from_secs(20), "the job never reached its first stage");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn start_holding() -> std::sync::MutexGuard<'static, ()> {
    let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    HELD.store(false, Ordering::SeqCst);
    HOLD.store(true, Ordering::SeqCst);
    cadaddy_engine::cmd::guided::set_progress_sink(Some(holding_sink));
    g
}

#[test]
fn a_job_typed_at_the_command_line_leaves_the_window_live() {
    let _g = start_holding();
    let mut app = CadApp::new(Session::new(), Services::default());
    let ctx = egui::Context::default();
    frame(&mut app, &ctx);
    let uid = app.session.state().unwrap().uid;
    let line = format!("job.start {{\"pdfs\": \"{}\", \"auto\": true, \"checkpoint\": false}}", prints_folder("typed"));

    // The command line hands the job over and returns (it held the window for the whole job).
    let t = Instant::now();
    let refused = app.cmdline(&line);
    let took = t.elapsed();
    assert!(refused.is_none(), "{refused:?}");
    assert!(took < Duration::from_secs(2), "the command line held the window {took:?}");
    assert_eq!(app.work_running(), Some("job.start"));
    hold_until_reached();

    // Frames keep coming, with the card and its Cancel.
    for _ in 0..5 {
        let ms = frame(&mut app, &ctx);
        assert!(ms < Duration::from_secs(2), "a frame took {ms:?} while the job runs");
    }
    let cancel = call(&mut app, &ctx, "ui.elements", json!({"label": "Cancel"}));
    assert!(cancel.to_string().contains("\"Cancel\""), "the card's Cancel is on screen: {cancel}");

    // The control channel answers while the job runs.
    let pong = call(&mut app, &ctx, "ping", json!({}));
    assert_eq!(pong["result"], "pong", "{pong}");
    let st = call(&mut app, &ctx, "cmdline.state", json!({}));
    assert_eq!(st["result"]["busy"], true, "{st}");
    assert_eq!(st["result"]["running"], "job.start", "{st}");
    let status = call(&mut app, &ctx, "engine.execute", json!({"command": "job.status", "params": {}}));
    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(status["result"]["busy"], true, "{status}");
    assert_eq!(status["result"]["running"], "job.start", "{status}");
    assert!(status["result"]["current_stage"].is_string(), "the job's stage: {status}");
    assert!(status["result"]["progress"].is_number(), "the job's progress: {status}");
    // Edits are refused as busy, never run on the copy the window shows.
    let edit = call(&mut app, &ctx, "engine.execute", json!({"command": "line", "params": {"points": [[0, 0], [10, 0]]}}));
    assert_eq!(edit["ok"], false, "{edit}");
    assert!(edit["error"].as_str().unwrap_or("").starts_with("busy:"), "{edit}");
    assert_eq!(edit["state"], "not-run", "{edit}");
    let typed = app.cmdline("line 0,0 10,0").unwrap_or_default();
    assert!(typed.contains("busy"), "{typed}");

    // Esc cancels: the job stops at its next stage.
    let esc = call(&mut app, &ctx, "cmdline.key", json!({"key": "escape"}));
    assert_eq!(esc["ok"], true, "{esc}");
    assert_eq!(cadaddy_engine::cmd::guided::live_progress().map(|p| p["cancel_requested"].clone()), Some(json!(true)));
    HOLD.store(false, Ordering::SeqCst);
    assert!(app.wait_for_work(Duration::from_secs(60)), "the job ended");
    frame(&mut app, &ctx);

    // The session is back: the same drawing, with the job's state and its history.
    assert_eq!(app.work_running(), None);
    assert_eq!(app.session.state().unwrap().uid, uid, "the same drawing");
    let st = app.run("job.status", json!({})).expect("job.status after the job");
    assert!(st["job_id"].is_string(), "{st}");
    assert_ne!(st["done"], true, "cancelled before the end: {st}");
    assert!(app.session.log.iter().any(|l| l.contains("cancelled at")), "{:?}", app.session.log.iter().rev().take(10).collect::<Vec<_>>());
    let st = call(&mut app, &ctx, "cmdline.state", json!({}));
    assert_eq!(st["result"]["busy"], false, "{st}");
    // And it takes edits again.
    app.run("line", json!({"points": [[0, 0], [10, 0]]})).expect("an edit after the job");
    cadaddy_engine::cmd::guided::set_progress_sink(None);
}

#[test]
fn an_agents_job_replies_when_it_ends_and_the_window_answers_meanwhile() {
    let _g = start_holding();
    let mut app = CadApp::new(Session::new(), Services::default());
    let ctx = egui::Context::default();
    frame(&mut app, &ctx);
    let params = json!({"pdfs": prints_folder("agent"), "auto": true, "checkpoint": false});
    let (req, pending) = ControlRequest::new("engine.execute", json!({"command": "job.start", "params": params}));
    let t = Instant::now();
    app.serve_control_request(&ctx, req);
    assert!(t.elapsed() < Duration::from_secs(2), "the request held the window {:?}", t.elapsed());
    let reply = std::thread::spawn(move || pending.wait(Duration::from_secs(60)));
    hold_until_reached();
    for _ in 0..3 {
        frame(&mut app, &ctx);
    }
    let pong = call(&mut app, &ctx, "ping", json!({}));
    assert_eq!(pong["result"], "pong", "the window answers while the agent's job runs: {pong}");
    assert!(!reply.is_finished(), "the reply waits for the job");
    HOLD.store(false, Ordering::SeqCst);
    let t = Instant::now();
    while app.work_running().is_some() {
        assert!(t.elapsed() < Duration::from_secs(60), "the job never ended");
        frame(&mut app, &ctx);
        std::thread::sleep(Duration::from_millis(10));
    }
    let v = reply.join().unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert!(v["result"]["job_id"].is_string(), "the job's own answer: {v}");
    // The agent's rails were the job's, and are off again at rest.
    assert!(!app.session.agent.from_agent);
    cadaddy_engine::cmd::guided::set_progress_sink(None);
}
