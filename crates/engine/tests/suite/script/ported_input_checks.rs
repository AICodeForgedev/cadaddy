//! Input checks upstream added that CADaddy already had its own way; their tests, kept here so the
//! behaviour stays (ported with the upstream fixes for SETVAR units, OFFSET and CHAMFER).

use cadaddy_engine::Session;
use serde_json::json;

#[test]
fn setvar_rejects_out_of_range_units() {
    let mut s = Session::new();
    for (name, v) in [("LUPREC", 100), ("LUNITS", 9), ("LUNITS", 0), ("AUPREC", -5), ("AUNITS", 5)] {
        assert!(s.execute("setvar", &json!({"name": name, "value": v})).is_err(), "{name} {v}");
    }
    s.execute("setvar", &json!({"name": "LUPREC", "value": 8})).unwrap();
    assert_eq!(s.doc().unwrap().header.i64("LUPREC", 0), 8);
    assert_ne!(s.doc().unwrap().header.i64("LUNITS", 2), 9);
}

#[test]
fn offset_zero_distance_is_rejected() {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    let h = s.doc().unwrap().model.handles()[0].hex();
    s.execute("offset", &json!({"handle": h, "distance": 1, "side": [1, 1]})).expect("a real distance offsets");
    s.execute("undo", &json!({})).unwrap();
    let r = s.execute("offset", &json!({"handle": h, "distance": 0, "side": [1, 1]}));
    assert!(r.is_err(), "offset distance 0 made a coincident copy: {r:?}");
    assert_eq!(s.doc().unwrap().model.len(), 1);
}

#[test]
fn chamfer_rejects_negative_distances() {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [10, 0]]})).unwrap();
    s.execute("line", &json!({"points": [[0, 0], [0, 10]]})).unwrap();
    let hs = s.doc().unwrap().model.handles();
    let (h1, h2) = (hs[0].hex(), hs[1].hex());
    let before = format!("{:?}", s.doc().unwrap().model.iter().map(|e| e.kind.clone()).collect::<Vec<_>>());
    let r = s.execute("chamfer", &json!({"h1": h1, "h2": h2, "p1": [5, 0], "p2": [0, 5], "d1": -3, "d2": 2}));
    assert!(r.is_err(), "negative chamfer distance accepted: {r:?}");
    let after = format!("{:?}", s.doc().unwrap().model.iter().map(|e| e.kind.clone()).collect::<Vec<_>>());
    assert_eq!(before, after);
}
