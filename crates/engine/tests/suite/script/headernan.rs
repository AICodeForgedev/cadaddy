//! A header variable can't take 'nan' / 'inf' on any route, and a saved .cdy never loses its
//! other settings to a bad one (battle test 6, B9-016).

use cadaddy_engine::Session;
use cadaddy_engine::doc::HVal;
use serde_json::json;

fn session() -> Session {
    Session::new()
}

#[test]
fn nonfinite_text_is_refused_for_numeric_variables() {
    let mut s = session();
    for (name, bad) in
        [("THICKNESS", "nan"), ("PDSIZE", "inf"), ("SNAPANG", "-inf"), ("ELEVATION", "NaN"), ("LTSCALE", "Infinity"), ("TEXTSIZE", "nan")]
    {
        let before = s.doc().unwrap().header.get(name).cloned();
        let r = s.execute("setvar", &json!({"name": name, "value": bad}));
        assert!(r.is_err(), "{name}={bad} was accepted");
        // Variables with a range of their own say that; the rest ask for a number.
        assert!(format!("{r:?}").contains("Requires a number") || format!("{r:?}").contains("must be"), "{r:?}");
        assert_eq!(s.doc().unwrap().header.get(name).cloned(), before, "{name} changed");
    }
    // Typed at the command line, the same.
    let _ = s.cmdline("setvar");
    let _ = s.cmdline("THICKNESS");
    let _ = s.cmdline("nan");
    assert!(s.doc().unwrap().header.f64("THICKNESS", 0.0).is_finite());
    // A text variable may still hold the word.
    s.execute("setvar", &json!({"name": "USERS1", "value": "nan"})).unwrap();
    s.execute("setvar", &json!({"name": "THICKNESS", "value": 2.5})).unwrap();
    assert_eq!(s.doc().unwrap().header.f64("THICKNESS", 0.0), 2.5);
}

#[test]
fn a_saved_header_with_a_nonfinite_variable_still_reads_back() {
    let mut s = session();
    s.execute("setvar", &json!({"name": "DIMSCALE", "value": 48})).unwrap();
    let d = s.doc_mut().unwrap();
    d.header.set("THICKNESS", HVal::Real(f64::NAN));
    d.header.set("PDSIZE", HVal::Real(f64::INFINITY));
    let bytes = cadaddy_io::cdy::write_cdy(d).unwrap();
    let back = cadaddy_io::cdy::read_cdy(&bytes).unwrap();
    assert_eq!(back.header.f64("DIMSCALE", 1.0), 48.0);
    assert!(back.header.get("THICKNESS").is_none());
    let (_, rep) = cadaddy_io::cdy::read_cdy_report(&bytes).unwrap();
    assert!(rep.notes.is_empty(), "{:?}", rep.notes);
}
