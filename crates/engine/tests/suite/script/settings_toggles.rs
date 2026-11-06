//! Status-bar drafting toggles.

use cadaddy_engine::Session;
use serde_json::json;

/// Ortho and polar tracking exclude each other, however they're turned on (AutoCAD).
#[test]
fn ortho_and_polar_turn_each_other_off() {
    let mut s = Session::new();
    s.execute("polar", &json!({"on": true})).unwrap();
    s.execute("ortho", &json!({"on": true})).unwrap();
    assert!(s.settings.orthomode && !s.settings.polarmode);
    s.execute("polar", &json!({})).unwrap();
    assert!(s.settings.polarmode && !s.settings.orthomode);
    s.execute("setvar", &json!({"name": "ORTHOMODE", "value": 1})).unwrap();
    assert!(s.settings.orthomode && !s.settings.polarmode);
    s.execute("setvar", &json!({"name": "AUTOSNAP", "value": 63})).unwrap();
    assert!(s.settings.polarmode && !s.settings.orthomode);
    // Turning one off leaves the other off.
    s.execute("polar", &json!({"on": false})).unwrap();
    assert!(!s.settings.polarmode && !s.settings.orthomode);
}
