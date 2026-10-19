//! Every command in the menus and on the ribbon (buttons, their flyouts and the panels'
//! slide-outs), started from its menu item and typed at the command line in a headless app on the
//! sample drawing: whatever it asks first is cancelled with Escape (dialogs
//! close the same way). Afterwards nothing has panicked, no dialog is open and the command line is
//! back at "Command:" (no prompt, dialog, mode or question waiting, and no panic the engine's
//! guard caught). Problems are collected and reported together; `CADADDY_SMOKE_ONLY=a,b` runs
//! just those commands.

use std::panic::{AssertUnwindSafe, catch_unwind};

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services, menus, ribbon};
use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;

/// Commands that end the program or the drawing session on purpose; started, they are checked
/// for not panicking only.
const ENDS_SESSION: &[&str] = &["quit", "exit", "close", "closeall"];

fn harness() -> Harness<'static, CadApp> {
    let mut app = CadApp::new(Session::default(), Services::default());
    let _ = app.run("ui.sample", json!({}));
    app.ui.in_window_menu = false;
    let mut h = Harness::builder().with_size(vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(2);
    h
}

fn escape(h: &mut Harness<'_, CadApp>) {
    h.key_press(egui::Key::Escape);
    h.run_steps(2);
}

/// What is still open or waiting: a dialog, a prompt, a mode (walk), a question.
fn open_things(app: &CadApp, id: &str) -> Vec<String> {
    let mut wrong = Vec::new();
    if let Some(d) = &app.ui.dialog {
        wrong.push(format!("dialog \"{d}\" still open"));
    }
    if app.session.prompting() && !ENDS_SESSION.contains(&id) {
        wrong.push(format!("still prompting: {}", app.session.prompt_text()));
    }
    // Dialogs and modes kept outside `ui.dialog`.
    let open = [
        ("PDF import dialog", app.pdf.is_some()),
        ("tolerance dialog", app.tol_dlg.is_some()),
        ("hatch dialog", app.hatch_dlg.is_some()),
        ("walk mode", app.walk.is_some()),
        ("layer-off question", app.layer_off_ask.is_some()),
        ("close question", app.closing.is_some() && !ENDS_SESSION.contains(&id)),
    ];
    for (what, on) in open {
        if on {
            wrong.push(format!("{what} still open"));
        }
    }
    wrong
}

/// What is wrong after starting `id` one way (`how`), if anything.
fn check(id: &str, how: &str) -> Option<String> {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mut h = harness();
        match how {
            "menu" => menus::activate(h.state_mut(), id),
            _ => {
                h.state_mut().cmdline(id);
            }
        }
        h.run_steps(3);
        // Cancel what it asks (a prompt, a nested prompt, a dialog), as a person would.
        for _ in 0..4 {
            if open_things(h.state(), id).is_empty() {
                break;
            }
            escape(&mut h);
        }
        let app = h.state();
        let mut wrong = open_things(app, id);
        // The engine's guard turns a panic in a command into an "internal error" message.
        if let Some(l) = app.session.log.iter().find(|l| l.to_ascii_lowercase().contains("internal error")) {
            wrong.push(format!("caught panic: {l}"));
        }
        if !app.cmd.buffer.is_empty() {
            wrong.push(format!("command line holds \"{}\"", app.cmd.buffer));
        }
        wrong
    }));
    match r {
        Ok(w) if w.is_empty() => None,
        Ok(w) => Some(format!("{id} ({how}): {}", w.join("; "))),
        Err(p) => {
            let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
            Some(format!("{id} ({how}): PANIC {msg}"))
        }
    }
}

/// The commands on the ribbon: every button, its flyout's variants and the panels' slide-outs.
fn ribbon_commands() -> Vec<&'static str> {
    let mut out = Vec::new();
    for tab in ribbon::TABS {
        for panel in tab.panels {
            out.extend(panel.more.iter().map(|f| f.1));
            for item in panel.items {
                let btns: &[ribbon::Btn] = match item {
                    ribbon::Item::Large(b) => std::slice::from_ref(b),
                    ribbon::Item::Smalls(bs) => bs,
                    ribbon::Item::Layers => &[],
                };
                for b in btns {
                    out.push(b.cmd);
                    out.extend(b.flyout.iter().map(|f| f.1));
                }
            }
        }
    }
    out
}

#[test]
fn ribbon_commands_are_known() {
    let ribbon = ribbon_commands();
    assert!(ribbon.len() > 100, "only {} ribbon commands", ribbon.len());
    let unknown: Vec<&str> =
        ribbon.iter().copied().filter(|id| cadaddy_engine::find_command(id).is_none() && !menus::UI_COMMANDS.iter().any(|c| c.0 == *id)).collect();
    assert!(unknown.is_empty(), "ribbon buttons that run no command: {unknown:?}");
}

#[test]
fn every_menu_command_starts_and_cancels_cleanly() {
    // Commands may write files next to the drawing (or the working folder): keep them out of the
    // source tree.
    let dir = std::env::temp_dir().join(format!("cadaddy-every-command-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_current_dir(&dir).unwrap();
    let mut ids: Vec<&str> = cadaddy_engine::command_specs().iter().filter(|s| !s.menu.is_empty()).map(|s| s.id).collect();
    ids.extend(menus::UI_COMMANDS.iter().filter(|c| !c.2.is_empty()).map(|c| c.0));
    ids.extend(ribbon_commands());
    ids.sort_unstable();
    ids.dedup();
    let only = std::env::var("CADADDY_SMOKE_ONLY").ok();
    let mut problems = Vec::new();
    let mut n = 0;
    for id in ids.iter().filter(|id| only.as_deref().is_none_or(|o| o.split(',').any(|x| x == **id))) {
        for how in ["menu", "typed"] {
            n += 1;
            if let Some(p) = check(id, how) {
                problems.push(p);
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(only.is_some() || n > 400, "only {n} runs: the catalog looks empty");
    assert!(problems.is_empty(), "{} of {n} runs left something wrong:\n{}", problems.len(), problems.join("\n"));
}
