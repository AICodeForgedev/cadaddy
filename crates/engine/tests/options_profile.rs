//! OPTIONS: the JSON form reads and sets the user-profile variables without a dialog.

use cadcraft_engine::Session;
use cadcraft_engine::cmd::options::{PROFILE_VARS, profile, restore};
use serde_json::json;

#[test]
fn options_json_reports_sets_and_resets_the_profile() {
    let mut s = Session::new();
    let all = s.execute("options", &json!({})).unwrap();
    for name in PROFILE_VARS {
        assert!(all.get(*name).is_some(), "{name} is reported");
    }
    assert_eq!(all["PICKBOX"], json!(3.0));

    // Names in any case; the reply is the whole profile.
    let r = s.execute("options", &json!({"pickbox": 6, "GRIPSIZE": 9, "PICKFIRST": 0, "CursorSize": 25})).unwrap();
    assert_eq!(
        (r["PICKBOX"].clone(), r["GRIPSIZE"].clone(), r["PICKFIRST"].clone(), r["CURSORSIZE"].clone()),
        (json!(6.0), json!(9.0), json!(0), json!(25.0))
    );
    assert_eq!(s.execute("getvar", &json!({"name": "PICKBOX"})).unwrap()["value"], json!(6.0));

    // All or nothing: a bad name or value changes nothing.
    assert!(s.execute("options", &json!({"PICKBOX": 2, "LUNITS": 4})).is_err(), "drawing variables are not profile settings");
    assert!(s.execute("options", &json!({"APERTURE": 20, "SNAPUNIT": "wide"})).is_err());
    assert!(s.execute("options", &json!({"reset": "yes"})).is_err());
    assert!(s.execute("options", &json!([1, 2])).is_err());
    let now = s.execute("options", &json!({})).unwrap();
    assert_eq!((now["PICKBOX"].clone(), now["APERTURE"].clone()), (json!(6.0), json!(10.0)));

    // Reset, then apply in the same call.
    let r = s.execute("options", &json!({"reset": true, "APERTURE": 15})).unwrap();
    assert_eq!((r["PICKBOX"].clone(), r["GRIPSIZE"].clone(), r["APERTURE"].clone()), (json!(3.0), json!(5.0), json!(15.0)));
}

#[test]
fn saved_profile_restores_and_hostile_entries_are_skipped() {
    let mut a = Session::new();
    a.execute("options", &json!({"OSMODE": 515, "ORTHOMODE": 1, "POLARANG": 15, "GRIDUNIT": [2, 2], "DYNMODE": 0, "QPMODE": 1})).unwrap();
    let saved = profile(&a);
    let mut b = Session::new();
    restore(&mut b, &saved);
    assert_eq!(profile(&b), saved);

    // Unknown names, drawing variables, bad values and non-objects are ignored, never panic.
    let mut c = Session::new();
    restore(&mut c, &json!({"PICKBOX": "huge", "LUNITS": 4, "NOPE": 1, "POLARANG": -5, "SNAPUNIT": ["x"], "GRIPSIZE": 8}));
    restore(&mut c, &json!("not an object"));
    restore(&mut c, &json!(null));
    let p = profile(&c);
    assert_eq!((p["PICKBOX"].clone(), p["GRIPSIZE"].clone()), (json!(3.0), json!(8.0)));
    assert_ne!(c.execute("getvar", &json!({"name": "LUNITS"})).ok().map(|v| v["value"].clone()), Some(json!(4)));
}

#[test]
fn typed_options_without_a_dialog_reports_how_to_change_settings() {
    let mut s = Session::new();
    s.cmdline("OP").unwrap();
    assert!(s.log.iter().any(|l| l.contains("SETVAR")), "{:?}", s.log);
}
