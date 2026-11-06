//! ABOUT and QUIT typed at the command line (AutoCAD names) reach the About box and quitting.

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services};

#[test]
fn about_and_quit_are_commands() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.cmdline("about");
    assert_eq!(app.ui.dialog.as_deref(), Some("about"));
    app.cmdline("quit");
    assert!(app.quit_requested);
    // QQUIT (Express Tools) quits the same way: every drawing closes, asking about unsaved ones.
    let mut app = CadApp::new(Session::new(), Services::default());
    app.cmdline("qquit");
    assert!(app.quit_requested);
}

/// -DIMSTYLE typed in the app prompts at the command line; it doesn't open the dialog.
#[test]
fn dash_dimstyle_prompts_without_a_dialog() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.cmdline("-dimstyle");
    assert_eq!(app.ui.dialog, None);
    assert!(app.session.prompt_text().contains("dimension style option"), "{}", app.session.prompt_text());
}
