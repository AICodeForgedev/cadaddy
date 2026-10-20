//! End to end: the Arabic interface. Switched to Arabic through the control channel, the ribbon's
//! tabs run from the right, the Layers palette moves to the left, the labels are Arabic; back to
//! English, everything returns to where it was.

mod e2e_common;

use e2e_common::E2e;
use serde_json::{Value, json};

/// Left edge of the control with egui id `id`.
fn left(els: &[Value], id: &Value) -> f64 {
    els.iter().find(|e| &e["id"] == id).and_then(|e| e["rect"][0].as_f64()).unwrap_or(f64::NAN)
}

fn id_of(els: &[Value], label: &str) -> Value {
    els.iter().find(|e| e["label"] == label).map(|e| e["id"].clone()).unwrap_or_else(|| panic!("no `{label}` on screen"))
}

#[test]
fn arabic_mirrors_the_interface_and_english_restores_it() {
    let mut e = E2e::new();
    let en = e.elements();
    let (home, layers) = (id_of(&en, "Home"), id_of(&en, "Lock Layer"));
    let w = f64::from(e2e_common::W);
    assert!(left(&en, &home) < w / 2.0 && left(&en, &layers) > w / 2.0);

    let r = e.exec("ui.language", json!({ "lang": "ar" }));
    assert_eq!(r["shown"], "ar", "{r}");
    let ar = e.elements();
    assert!(left(&ar, &home) > w / 2.0, "the Home tab on the right: {}", left(&ar, &home));
    assert!(left(&ar, &layers) < w / 2.0, "the Layers palette on the left: {}", left(&ar, &layers));
    let label = ar.iter().find(|x| x["id"] == home).and_then(|x| x["label"].as_str()).unwrap_or_default().to_string();
    assert!(
        label != "Home" && label.chars().any(|c| ('\u{0600}'..='\u{06FF}').contains(&c) || ('\u{FE70}'..='\u{FEFF}').contains(&c)),
        "Arabic label: {label}"
    );

    e.exec("ui.language", json!({ "lang": "en" }));
    let back = e.elements();
    assert_eq!(left(&back, &home), left(&en, &home));
    assert_eq!(left(&back, &layers), left(&en, &layers));
    assert!(back.iter().any(|x| x["label"] == "Home"));
}
