//! The canvas's right-click menu (#325): Repeat, the Clipboard commands with AutoCAD's rules for
//! what is available, and the selection commands.

use cadcraft_engine::Session;
use cadcraft_ui_egui::menus::Entry;
use cadcraft_ui_egui::{CadApp, Services, context_menu};
use serde_json::json;

/// (label, id, enabled) of the items, with submenu items prefixed by "Sub/".
fn items(app: &CadApp) -> Vec<(String, String, bool)> {
    fn walk(prefix: &str, es: &[Entry], out: &mut Vec<(String, String, bool)>) {
        for e in es {
            match e {
                Entry::Item { label, id, enabled, .. } => out.push((format!("{prefix}{label}"), id.clone(), *enabled)),
                Entry::Sub { label, children } => walk(&format!("{prefix}{label}/"), children, out),
            }
        }
    }
    let mut out = Vec::new();
    for g in context_menu::entries(app) {
        walk("", &g, &mut out);
    }
    out
}

fn enabled(app: &CadApp, id: &str) -> bool {
    items(app).iter().find(|(_, i, _)| i == id).is_some_and(|(_, _, e)| *e)
}

#[test]
fn context_menu_offers_the_clipboard() {
    let mut app = CadApp::new(Session::new(), Services::default());
    let labels: Vec<String> = items(&app).into_iter().map(|(l, _, _)| l).collect();
    for l in [
        "Clipboard/Cut",
        "Clipboard/Copy",
        "Clipboard/Copy with Base Point",
        "Clipboard/Paste",
        "Clipboard/Paste as Block",
        "Clipboard/Paste to Original Coordinates",
        "Undo",
        "Redo",
        "Pan",
        "Zoom",
    ] {
        assert!(labels.iter().any(|x| x == l), "{l} missing from {labels:?}");
    }
    assert!(!labels.iter().any(|l| l.starts_with("Repeat")), "no command yet");
    assert!(!enabled(&app, "pasteclip"), "nothing to paste");

    app.cmdline("LINE 0,0 10,0");
    app.cmdline("");
    app.run("copyclip", json!({ "handles": app.session.doc().unwrap().model.handles().iter().map(|h| h.hex()).collect::<Vec<_>>() })).unwrap();
    let repeat = items(&app).into_iter().next().unwrap();
    assert_eq!((repeat.0.as_str(), repeat.1.as_str()), ("Repeat LINE", "line"));
    assert!(enabled(&app, "pasteclip") && enabled(&app, "pasteblock"));
    assert!(!enabled(&app, "pasteorig"), "Paste to Original Coordinates pastes into another drawing");

    app.run("new", json!({})).unwrap();
    assert!(enabled(&app, "pasteorig"));
    context_menu_selection(&mut app);
}

/// With a selection, the editing commands and Deselect All.
fn context_menu_selection(app: &mut CadApp) {
    assert!(!items(app).iter().any(|(_, id, _)| id == "ai_deselect"));
    app.run("pasteorig", json!({})).unwrap();
    assert_eq!(app.session.selection().len(), 1, "pasted objects are selected");
    let ids: Vec<String> = items(app).into_iter().map(|(_, id, _)| id).collect();
    for id in ["erase", "move", "copy", "ai_deselect", "properties"] {
        assert!(ids.iter().any(|x| x == id), "{id} missing from {ids:?}");
    }
    app.run("ai_deselect", json!({})).unwrap();
    assert!(app.session.selection().is_empty());
}

/// egui delivers Cmd/Ctrl+C, X and V as clipboard events, not key presses: they run the
/// clipboard commands.
#[test]
fn clipboard_events_run_the_clipboard_commands() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.run("line", json!({ "points": [[0, 0], [10, 0]] })).unwrap();
    let h = app.session.doc().unwrap().model.handles();
    app.session.set_selection(h);
    let ctx = egui::Context::default();
    let frame = |app: &mut CadApp, events: Vec<egui::Event>| {
        ctx.begin_pass(egui::RawInput { events, ..Default::default() });
        cadcraft_ui_egui::menus::shortcuts(app, &ctx);
        ctx.end_pass().textures_delta.clear();
    };
    frame(&mut app, vec![egui::Event::Copy]);
    assert_eq!(app.session.clipboard.len(), 1, "Copy with a selection copies it");
    frame(&mut app, vec![egui::Event::Paste("text from another program".into())]);
    assert_eq!(app.session.running.as_ref().map(|r| r.id.as_str()), Some("pasteclip"));
    assert!(app.session.prompt_text().contains("insertion point"));
}
