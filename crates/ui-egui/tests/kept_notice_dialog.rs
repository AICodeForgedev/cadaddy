//! The "not kept in the DWG file" notice reaches the dialog and the agents when the first-save
//! check runs in the background (`Session::background_checks`).

use std::time::{Duration, Instant};

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Entity, EntityKind, Line, Transparency};
use cadaddy_engine::geom::Vec3;
use cadaddy_ui_egui::control::{ControlRequest, Outcome, handle};
use cadaddy_ui_egui::{CadApp, Services, exchangeloss};
use serde_json::{Value, json};

fn io_hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-keptdlg-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// An app with background checks on and a drawing of one 50%-transparent line (DWG 2000 has no
/// transparency).
fn app() -> CadApp {
    io_hooks();
    let mut s = Session::new();
    let d = s.doc_mut().unwrap();
    let mut e = Entity::new(d.new_handle(), EntityKind::Line(Line { a: Vec3::new(0.0, 0.0, 0.0), b: Vec3::new(4.0, 2.0, 0.0) }));
    e.common.transparency = Transparency::Percent(50);
    d.model.push(e);
    s.background_checks = true;
    CadApp::new(s, Services::default())
}

#[test]
fn a_finished_background_check_opens_the_dialog_even_after_a_programmatic_run() {
    let dir = temp_dir("dialog");
    let path = dir.join("Project A.dwg");
    let mut app = app();
    let ctx = egui::Context::default();

    // The user's save: typed at the command line, so the check runs on a worker.
    app.cmdline(&format!("saveas {{\"path\": {:?}, \"version\": \"2000\"}}", path.to_string_lossy()));
    assert!(path.is_file(), "the save wrote the file");
    // A programmatic run in between must not discard the notice the worker leaves.
    app.run("regen", json!({})).unwrap();

    let t = Instant::now();
    while app.ui.dialog.as_deref() != Some(exchangeloss::DIALOG) {
        assert!(t.elapsed() < Duration::from_secs(30), "the dialog never opened");
        app.run("regen", json!({})).unwrap();
        app.logic(&ctx);
        std::thread::sleep(Duration::from_millis(10));
    }
    let notice = app.exchange_notice.clone().expect("notice held for the dialog");
    assert!(notice["losses"].to_string().contains("object transparency"), "{notice}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_finished_check_is_not_dropped_by_a_run_that_finds_it() {
    let dir = temp_dir("run");
    let path = dir.join("Project B.dwg");
    let mut app = app();
    app.session.execute("saveas", &json!({ "path": &path, "version": "2000" })).unwrap();
    let t = Instant::now();
    // Only `run` polls here: the worker's result is handed to the dialog by it.
    while app.ui.dialog.as_deref() != Some(exchangeloss::DIALOG) {
        assert!(t.elapsed() < Duration::from_secs(30), "the dialog never opened");
        app.run("regen", json!({})).unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.exchange_notice.as_ref().is_some_and(|n| n["losses"].to_string().contains("object transparency")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// An agent's save through the control channel, with background checks on, gets the losses in
/// the reply.
#[test]
fn an_agent_save_through_the_control_channel_gets_the_losses() {
    let dir = temp_dir("agent");
    let path = dir.join("Project C.dwg");
    let mut app = app();
    let ctx = egui::Context::default();
    let (req, _reply) = ControlRequest::new("app.save", json!({ "path": path.to_string_lossy(), "version": "2000" }));
    let Outcome::Done(v): Outcome = handle(&mut app, &ctx, &req) else { panic!("no reply") };
    let v: Value = v;
    assert_eq!(v["ok"], true, "{v}");
    assert!(v["result"].get("notKeptPending").is_none(), "{v}");
    assert!(v["result"]["notKeptDetail"].to_string().contains("object transparency"), "{v}");
    assert!(app.ui.dialog.is_none(), "programmatic saves never open dialogs");
    let _ = std::fs::remove_dir_all(&dir);
}
