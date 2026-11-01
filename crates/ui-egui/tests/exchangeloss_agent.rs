//! An agent's DWG save through the app (background checks on, as shipped) gets the exchange-loss
//! list in its reply and leaves no notice for a later command of the user's to show.

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services};
use serde_json::json;

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

#[test]
fn an_agent_save_in_the_app_returns_the_losses_and_leaves_no_dialog() {
    io_hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-exchangeloss-agent-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = CadApp::new(Session::new(), Services::default());
    app.session.background_checks = true;
    let line = app.run("line", json!({"points": [[0, 0], [10, 0]]})).unwrap()["handles"][0].as_str().unwrap().to_string();
    app.run("chprop", json!({"handles": [line], "transparency": 50})).unwrap();

    let path = dir.join("Sample Plan.dwg").display().to_string();
    let r = app.run("saveas", json!({"path": path, "version": 2000})).unwrap();
    assert!(r.get("notKeptPending").is_none(), "{r}");
    let detail = r["notKeptDetail"].as_array().unwrap_or_else(|| panic!("no notKeptDetail: {r}"));
    assert!(detail.iter().any(|l| l["feature"] == "object transparency" && l["handles"] == json!([line])), "{r}");
    assert!(app.session.background_checks, "the app's setting is restored");

    // A user command afterwards shows no dialog for the agent's save.
    app.start("regen");
    assert!(app.ui.dialog.is_none(), "{:?}", app.ui.dialog);
    assert!(app.exchange_notice.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
