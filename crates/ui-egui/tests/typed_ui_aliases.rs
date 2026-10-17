//! ABOUT and QUIT typed at the command line (AutoCAD names) reach the About box and quitting.

use cadcraft_engine::Session;
use cadcraft_ui_egui::{CadApp, Services};

#[test]
fn about_and_quit_are_commands() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.cmdline("about");
    assert_eq!(app.ui.dialog.as_deref(), Some("about"));
    app.cmdline("quit");
    assert!(app.quit_requested);
}
