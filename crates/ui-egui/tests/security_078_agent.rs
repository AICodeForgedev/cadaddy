//! Security regression suite, agent rails (Q207): what an agent on the control channel can't do
//! without the user, sent as real control requests. Saving over a file waits for the user even
//! when the agent claims it was confirmed; SECURELOAD can't be lowered, TRUSTEDPATHS changed or a
//! file allowed by an agent, whether by `engine.execute`, OPTIONS or a typed command line; an
//! untrusted LISP file an agent loads doesn't run. The user's own calls still can.

use cadaddy_engine::Session;
use cadaddy_ui_egui::control::{ControlRequest, Outcome, handle};
use cadaddy_ui_egui::{CadApp, Services};
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

fn error(v: &Value) -> String {
    v["error"].as_str().map(str::to_string).or_else(|| v["result"]["error"].as_str().map(str::to_string)).unwrap_or_default()
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

fn folder(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-sec078-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn an_agent_cannot_claim_its_own_confirmation() {
    let mut app = app();
    let dir = folder("confirm");
    let path = dir.join("theirs.dxf");
    std::fs::write(&path, b"the user's file").unwrap();
    exec(&mut app, "circle", json!({ "center": [0, 0], "radius": 1 }));
    // `confirmed: true` from the agent is not the user's answer.
    for params in [json!({ "path": path }), json!({ "path": path, "confirmed": true })] {
        let v = exec(&mut app, "saveas", params);
        assert!(error(&v).contains("Waiting for the user"), "{v}");
        assert_eq!(std::fs::read(&path).unwrap(), b"the user's file", "not saved over");
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_agent_cannot_loosen_what_loads_code() {
    let mut app = app();
    // Raising is fine; lowering, changing the trusted folders or allowing a file are not.
    assert_eq!(exec(&mut app, "setvar", json!({ "name": "SECURELOAD", "value": 2 }))["ok"], true);
    for (command, params) in [
        ("setvar", json!({ "name": "SECURELOAD", "value": 0 })),
        ("setvar", json!({ "name": "SECURELOAD", "value": 1 })),
        ("setvar", json!({ "name": "TRUSTEDPATHS", "value": "/tmp/..." })),
        ("options", json!({ "SECURELOAD": 0 })),
        ("options", json!({ "TRUSTEDPATHS": "/" })),
        ("secureload.allow", json!({ "path": "/tmp/x.lsp" })),
        ("secureload.allow", json!({ "path": "/tmp/x.lsp", "always": true })),
    ] {
        let v = exec(&mut app, command, params.clone());
        assert!(error(&v).contains("Only the user"), "{command} {params}: {v}");
    }
    // Typed at the command line by the agent: refused the same way.
    for line in ["SECURELOAD 0", "setvar SECURELOAD 1", "TRUSTEDPATHS /"] {
        call(&mut app, "cmdline.input", json!({ "text": line }));
        call(&mut app, "cmdline.key", json!({ "key": "escape" }));
    }
    assert_eq!(app.session.settings.secureload, 2);
    assert_eq!(app.session.settings.trustedpaths, "");
    // The user's own calls still change them.
    app.session.execute("setvar", &json!({ "name": "SECURELOAD", "value": 1 })).unwrap();
    assert_eq!(app.session.settings.secureload, 1);
}

#[test]
fn an_agent_cannot_open_the_control_channel_wider_or_stop_updates() {
    let mut app = app();
    let (token, agents) = (app.ui.control_token, app.ui.agent_control);
    for params in [json!({ "controlToken": !token }), json!({ "agentControl": !agents }), json!({ "checkForUpdates": false })] {
        let v = exec(&mut app, "options", params.clone());
        assert!(error(&v).contains("Only the user"), "{params}: {v}");
    }
    assert_eq!((app.ui.control_token, app.ui.agent_control), (token, agents));
    // Other preferences an agent may set; the user may set these too.
    assert_eq!(exec(&mut app, "options", json!({ "theme": "dark" }))["ok"], true);
    app.run("options", json!({ "controlToken": !token })).unwrap();
    assert_eq!(app.ui.control_token, !token);
}

#[test]
fn untrusted_lisp_an_agent_loads_does_not_run() {
    let mut app = app();
    let dir = folder("lisp");
    let file = dir.join("tool.lsp");
    std::fs::write(&file, "(setq agent-marker 42)").unwrap();
    let v = exec(&mut app, "appload", json!({ "path": file }));
    assert!(error(&v).contains("isn't in a trusted folder"), "{v}");
    // A programmatic call gets the refusal only (no dialog pops up for the user to click by
    // mistake), and the agent can't allow the file itself.
    assert!(app.session.secureload_ask.is_none() && app.ui.dialog.is_none(), "{v}");
    assert!(error(&exec(&mut app, "secureload.allow", json!({ "path": file }))).contains("Only the user"));
    let marker = exec(&mut app, "lisp", json!({ "code": "agent-marker" }));
    assert_eq!(marker["result"]["value"], "nil", "nothing ran: {marker}");
    std::fs::remove_dir_all(&dir).ok();
}
