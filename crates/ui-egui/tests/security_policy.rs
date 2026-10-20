//! How to report a security problem is easy to find: SECURITY.md says where and what to expect,
//! the README points to it, and Help ▸ About links to it.

use cadaddy_engine::Session;
use cadaddy_ui_egui::{CadApp, Services, support};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

fn root(file: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(file)).unwrap()
}

#[test]
fn security_md_says_where_what_and_when() {
    let s = root("SECURITY.md");
    assert!(s.contains(support::FEEDBACK_EMAIL) && s.contains("SECURITY:"), "where to write");
    assert!(s.contains("## Supported versions") && s.contains("latest release"), "which versions get fixes");
    assert!(s.contains("## What happens next") && s.contains("7 days") && s.contains("90 days"), "what to expect");
    assert!(s.contains("no bug bounty"), "no bounty, said plainly");
    assert!(support::SECURITY_URL.ends_with("/SECURITY.md"));
    let readme = root("README.md");
    assert!(readme.contains("## Security") && readme.contains("(SECURITY.md)"), "the README points to it");
}

#[test]
fn about_links_to_the_security_policy() {
    let mut app = CadApp::new(Session::new(), Services::default());
    app.ui.dialog = Some("about".into());
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_ui_state(
        |ui, app: &mut CadApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h.get_by_label("Report a security problem").click();
    h.run_steps(1);
    let urls: Vec<String> = h
        .output()
        .platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::OpenUrl(u) => Some(u.url.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(urls, [support::SECURITY_URL]);
}
