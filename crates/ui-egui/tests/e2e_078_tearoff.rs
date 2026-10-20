//! End to end (0.7.8): tear-off drawing windows. Two drawings; the second torn off into its own
//! window: it leaves the tab bar, its window has a Dock button, it can be edited there while the
//! main window keeps the first drawing, and docked again it is back in the tab bar with its edit.
//! A drawing shows in one window at a time (tearing it off again opens no second window).

mod e2e_common;

use e2e_common::E2e;
use serde_json::json;

fn entities(e: &mut E2e) -> u64 {
    e.entity_count()
}

fn active(e: &E2e) -> usize {
    e.app.session.active
}

#[test]
fn a_torn_off_drawing_is_edited_in_its_window_and_docks_again() {
    let mut e = E2e::new();
    e.exec("line", json!({ "points": [[0, 0], [10, 0]] }));
    let first = active(&e);
    e.exec("new", json!({}));
    let second = active(&e);
    assert_ne!(first, second);
    e.exec("circle", json!({ "center": [0, 0], "radius": 5 }));
    let tabs_before = e.elements().iter().filter(|x| x["role"] == "tab").count();

    // Tear the second drawing off.
    let r = e.exec("ui.window.new", json!({ "index": second }));
    assert_eq!(r["windows"], 1, "{r}");
    assert_eq!(e.app.torn.len(), 1);
    assert!(cadaddy_ui_egui::tearoff::is_torn(&e.app, second));
    assert!(e.shows("Dock"), "its window shows a Dock button");
    let tabs_after = e.elements().iter().filter(|x| x["role"] == "tab").count();
    assert!(tabs_after < tabs_before, "it left the tab bar ({tabs_before} → {tabs_after})");
    assert_eq!(active(&e), first, "the main window shows the other drawing");

    // The same drawing torn off again: still one window.
    e.exec("ui.window.new", json!({ "index": second }));
    assert_eq!(e.app.torn.len(), 1, "one window per drawing");

    // Edit it in its window, and the main drawing on its own.
    e.exec("document.switch", json!({ "index": second }));
    assert_eq!(active(&e), second);
    let n = entities(&mut e);
    e.typed("line 0,20 30,20");
    assert_eq!(entities(&mut e), n + 1, "drawn in the torn-off drawing");
    e.exec("document.switch", json!({ "index": first }));
    let m = entities(&mut e);
    e.exec("circle", json!({ "center": [5, 5], "radius": 1 }));
    assert_eq!(entities(&mut e), m + 1);
    e.exec("document.switch", json!({ "index": second }));
    assert_eq!(entities(&mut e), n + 1, "the main drawing's edit didn't go to it");

    // Dock it: back in the tab bar, current, with its edit.
    assert!(e.click("Dock"));
    e.pump();
    assert!(e.app.torn.is_empty(), "docked");
    assert!(!e.shows("Dock"));
    assert_eq!(e.elements().iter().filter(|x| x["role"] == "tab").count(), tabs_before, "back in the tab bar");
    assert_eq!(active(&e), second, "the docked drawing is current");
    assert_eq!(entities(&mut e), n + 1);

    // Dock all: every torn window comes back.
    e.exec("ui.window.new", json!({ "index": second }));
    e.exec("ui.window.new", json!({ "index": first }));
    assert!(!e.app.torn.is_empty());
    e.exec("ui.window.dockall", json!({}));
    assert!(e.app.torn.is_empty(), "all docked");
}
