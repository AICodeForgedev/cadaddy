//! End to end (0.7.8): Arabic dialogs mirrored. The Purge and Options dialogs opened in English
//! and in Arabic: in Arabic their rows of buttons run from the right (the first is rightmost),
//! their labels are Arabic, and back in English they are as they were.

mod e2e_common;

use e2e_common::E2e;
use serde_json::{Value, json};

fn left(els: &[Value], label: &str) -> f64 {
    els.iter().find(|x| x["label"] == label).and_then(|x| x["rect"][0].as_f64()).unwrap_or_else(|| panic!("no `{label}` on screen"))
}

fn ar(text: &str) -> String {
    cadaddy_ui_egui::i18n::with_language("ar", || cadaddy_ui_egui::i18n::t(text).to_string())
}

/// The left edges of `labels` (English) in the open dialog, in `lang`.
fn edges(e: &mut E2e, lang: &str, labels: &[&str]) -> Vec<f64> {
    let els = e.elements();
    labels.iter().map(|l| left(&els, &if lang == "ar" { ar(l) } else { (*l).to_string() })).collect()
}

fn open(e: &mut E2e, command: &str, id: &str) {
    e.menu(command);
    e.pump();
    assert_eq!(e.dialog().as_deref(), Some(id));
}

fn close(e: &mut E2e) {
    e.ok("ui.set", json!({ "dialog": null }));
    e.pump();
}

#[test]
fn arabic_dialogs_run_from_the_right() {
    let mut e = E2e::new();
    e.exec("layer", json!({ "option": "new", "name": "Spare" }));
    let purge = ["Purge Checked Items", "Purge All", "Close"];
    {
        let (command, id, row) = ("purge", "purge", &purge[..]);
        open(&mut e, command, id);
        let en = edges(&mut e, "en", row);
        assert!(en.windows(2).all(|w| w[0] < w[1]), "English: left to right {en:?}");
        close(&mut e);

        e.exec("ui.language", json!({ "lang": "ar" }));
        open(&mut e, command, id);
        assert_ne!(ar(row[0]), row[0], "{} has an Arabic translation", row[0]);
        let a = edges(&mut e, "ar", row);
        assert!(a.windows(2).all(|w| w[0] > w[1]), "Arabic: right to left {a:?}");
        close(&mut e);

        e.exec("ui.language", json!({ "lang": "en" }));
        open(&mut e, command, id);
        assert_eq!(edges(&mut e, "en", row), en, "English again: as before");
        close(&mut e);
    }
    // Options: its tabs run from the right too.
    let tabs = ["Open and Save", "User Preferences", "Profiles"];
    open(&mut e, "options", "options");
    let en = edges(&mut e, "en", &tabs);
    assert!(en.windows(2).all(|w| w[0] < w[1]), "English tabs: {en:?}");
    close(&mut e);
    e.exec("ui.language", json!({ "lang": "ar" }));
    open(&mut e, "options", "options");
    let a = edges(&mut e, "ar", &tabs);
    assert!(a.windows(2).all(|w| w[0] > w[1]), "Arabic tabs from the right: {a:?}");
    close(&mut e);
    e.exec("ui.language", json!({ "lang": "en" }));
}
