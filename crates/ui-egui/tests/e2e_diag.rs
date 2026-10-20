//! End to end: Help ▸ Report a Problem. The dialog opens from its menu item, Save Report writes the
//! ZIP where the user picks, the dialog closes, and the report holds the system details and the
//! commands but none of the drawing's names (its file path, a layer name, typed text).

mod e2e_common;

use e2e_common::{E2e, folder};
use serde_json::json;

/// Keys as a user types them, then Enter.
fn type_line(e: &mut E2e, text: &str) {
    if !text.is_empty() {
        e.events.push(egui::Event::Text(text.into()));
    }
    e.events.push(egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() });
    e.frame();
    e.events.push(egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: false, repeat: false, modifiers: Default::default() });
    e.pump();
}

#[test]
fn the_report_is_saved_without_names() {
    let mut e = E2e::new();
    let dir = folder("diag");
    // A drawing with private names: its file, a layer, typed text — typed on the keyboard.
    for line in ["-layer make Zorblat", "", "line 0,0 10,10", ""] {
        type_line(&mut e, line);
    }
    assert!(e.app.session.doc().unwrap().layer("Zorblat").is_some(), "{:?}", e.history());
    let plan = dir.join("Quixotic Plan.dxf");
    e.exec("saveas", json!({ "path": plan }));

    // The user picks where the report goes.
    let zip_path = dir.join("report.zip");
    let pick = zip_path.to_string_lossy().into_owned();
    e.app.services.pick_save = Some(Box::new(move |_| Some(pick.clone())));
    e.menu("ui.diag");
    assert_eq!(e.dialog().as_deref(), Some("diag"));
    assert!(e.click("Save Report..."), "the Save Report button");
    assert_ne!(e.dialog().as_deref(), Some("diag"), "the dialog closes once saved");
    assert!(e.history().iter().any(|l| l.contains("Problem report saved")), "{:?}", e.history());

    let zip = std::fs::read(&zip_path).expect("the report");
    let files = cadaddy_engine::cmd::file::zip::read(&zip).expect("a ZIP");
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"README.txt") && names.contains(&"system.txt") && names.contains(&"commands.txt"), "{names:?}");
    let all: String = files.iter().map(|(_, b)| String::from_utf8_lossy(b).into_owned()).collect();
    assert!(all.contains("operating system"), "system details");
    let cmds = files.iter().find(|(n, _)| n == "commands.txt").map(|(_, b)| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
    assert!(cmds.contains("\nline\n") && cmds.contains("\n-layer\n"), "the command names: {cmds:?}");
    for private in ["Zorblat", "Quixotic", dir.to_string_lossy().as_ref()] {
        assert!(!all.contains(private), "`{private}` in the report");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
