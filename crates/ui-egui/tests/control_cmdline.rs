//! The app's control channel answers `cmdline.input` and `app.open` with the same reply shape as
//! the headless MCP backend (`cadaddy_mcp::Headless`, tested in `crates/mcp/tests/cmdline_parity.rs`).

use cadaddy_engine::Session;
use cadaddy_ui_egui::control::{ControlRequest, Outcome, handle};
use cadaddy_ui_egui::{CadApp, Services};
use serde_json::{Value, json};

const STATE_KEYS: [&str; 11] =
    ["prompt", "message", "running", "keywords", "default", "accept", "busy", "closePrompt", "buffer", "history", "historyExpanded"];

fn call(app: &mut CadApp, method: &str, params: Value) -> Value {
    let (req, _reply) = ControlRequest::new(method, params);
    match handle(app, &egui::Context::default(), &req) {
        Outcome::Done(v) => v,
        Outcome::Screenshot { .. } => Value::Null,
    }
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    k.sort();
    k
}

fn want(extra: &[&str]) -> Vec<String> {
    let mut k: Vec<String> = STATE_KEYS.iter().chain(extra).map(|s| s.to_string()).collect();
    k.sort();
    k
}

fn app() -> CadApp {
    CadApp::new(Session::new(), Services::default())
}

#[test]
fn refused_input_is_echoed_to_output_and_reported_in_error() {
    let mut app = app();
    let v = call(&mut app, "cmdline.input", json!({"text": "nosuchcommand"}));
    assert_eq!(v["ok"], true, "{v}");
    let r = &v["result"];
    assert_eq!(keys(r), want(&["output", "error"]), "{r}");
    let e = r["error"].as_str().unwrap();
    assert!(!e.is_empty());
    assert!(r["output"].as_array().unwrap().iter().any(|l| l.as_str() == Some(e)), "{r}");
    assert!(r["history"].as_array().unwrap().iter().any(|l| l.as_str() == Some(e)), "{r}");
}

#[test]
fn accepted_input_has_no_error_and_reports_the_prompt() {
    let mut app = app();
    let v = call(&mut app, "cmdline.input", json!({"text": "circle"}));
    let r = &v["result"];
    assert_eq!(keys(r), want(&["output"]), "{r}");
    assert_eq!(r["running"], "circle");
    assert!(!r["accept"].is_null(), "{r}");
    assert_eq!(keys(&call(&mut app, "cmdline.state", json!({}))["result"]), want(&[]));
}

#[test]
fn open_without_a_path_is_refused_before_the_engine() {
    let mut app = app();
    assert_eq!(call(&mut app, "app.open", json!({})), json!({"ok": false, "error": "missing path"}));
    assert_eq!(call(&mut app, "app.open", json!({"path": 3})), json!({"ok": false, "error": "missing path"}));
}

/// An agent's typing never becomes input to a command the person at the window is running
/// (battle test 0.7.7c: `circle 80,80 3` added two segments to the user's LINE); a command the
/// agent started itself takes its typing as before.
#[test]
fn agent_typing_stays_out_of_the_users_command() {
    let mut app = app();
    let count = |app: &CadApp| app.session.doc().map(|d| d.model.len()).unwrap_or(0);
    let before = count(&app);
    // The user starts LINE in the app.
    app.cmdline("line");
    let v = call(&mut app, "cmdline.input", json!({"text": "circle 80,80 3"}));
    assert_eq!(v["ok"], false, "{v}");
    assert!(v["error"].as_str().unwrap_or("").contains("LINE"), "{v}");
    assert_eq!(count(&app), before, "the agent's text went into the user's LINE");
    assert_eq!(call(&mut app, "cmdline.key", json!({"key": "enter"}))["ok"], false, "the agent's Enter went into the user's LINE");
    assert_eq!(app.session.running.as_ref().map(|r| r.id.as_str()), Some("line"));
    // Escape is always allowed (tester 0.7.7g): it feeds nothing into the prompts.
    assert_eq!(call(&mut app, "cmdline.key", json!({"key": "escape"}))["ok"], true);
    assert!(app.session.running.is_none());
    // The agent's own command takes its points.
    call(&mut app, "cmdline.input", json!({"text": "line"}));
    let v = call(&mut app, "cmdline.input", json!({"text": "0,0 10,0 "}));
    assert_eq!(v["ok"], true, "{v}");
    call(&mut app, "cmdline.key", json!({"key": "enter"}));
    assert_eq!(count(&app), before + 1);
}

/// A command an agent starts from a menu is the agent's: its input answers the prompts (tester
/// 0.7.7g: menu-started CIRCLE refused `5,5` as "the user is running CIRCLE").
#[test]
fn a_menu_command_the_agent_started_takes_its_input() {
    let mut app = app();
    let count = |app: &CadApp| app.session.doc().map(|d| d.model.len()).unwrap_or(0);
    let before = count(&app);
    call(&mut app, "ui.menu.invoke", json!({"command": "circle"}));
    assert_eq!(app.session.running.as_ref().map(|r| r.id.as_str()), Some("circle"));
    let v = call(&mut app, "cmdline.input", json!({"text": "5,5"}));
    assert_eq!(v["ok"], true, "{v}");
    let v = call(&mut app, "cmdline.input", json!({"text": "2"}));
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(count(&app), before + 1);
}

/// An agent's request never opens a native file dialog: it opened on the user's screen and
/// blocked the app, every later agent call timing out (menu sweep 0.7.7e: Insert ▸ PDF Import…).
/// The reply says why nothing was picked.
#[test]
fn agent_requests_never_open_file_dialogs() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static OPENED: AtomicBool = AtomicBool::new(false);
    let services = cadaddy_ui_egui::Services {
        pick_pdf: Some(Box::new(|| {
            if cadaddy_ui_egui::file_dialogs_allowed() {
                OPENED.store(true, Ordering::Relaxed);
                Some("x.pdf".into())
            } else {
                None
            }
        })),
        ..Default::default()
    };
    let mut app = CadApp::new(Session::new(), services);
    let ctx = egui::Context::default();
    let (req, reply) = ControlRequest::new("ui.menu.invoke", json!({"command": "ui.pdfimport"}));
    app.serve_control_request(&ctx, req);
    let outcome = reply.wait(std::time::Duration::from_secs(2));
    assert!(!OPENED.load(Ordering::Relaxed), "a file dialog opened for an agent");
    assert!(outcome["warning"].as_str().unwrap_or("").contains("file dialog"), "{outcome}");
}
