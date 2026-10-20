//! OFFSET of a polyline that closes on itself inward (two rooms joined by a narrow passage) gives
//! a polyline per room, as AutoCAD does; each piece is exact and clipped where the raw offset
//! crossed itself.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle};
use serde_json::json;

#[test]
fn a_polyline_that_splits_gives_every_piece() {
    let mut s = Session::new();
    let pts = json!([[0, 0], [10, 0], [10, 4.5], [14, 4.5], [14, 0], [24, 0], [24, 10], [14, 10], [14, 5.5], [10, 5.5], [10, 10], [0, 10]]);
    let pl = s.execute("pline", &json!({"vertices": pts, "closed": true})).unwrap()["handle"].as_str().unwrap().to_string();
    let r = s.execute("offset", &json!({"handle": pl, "distance": 1, "side": [5, 5]})).unwrap();
    let hs: Vec<Handle> = r["handles"].as_array().unwrap().iter().map(|h| Handle::parse_hex(h.as_str().unwrap()).unwrap()).collect();
    assert_eq!(hs.len(), 2, "{r}");
    assert_eq!(r["handle"], r["handles"][0]);
    for h in &hs {
        let e = s.doc().unwrap().entity(*h).unwrap().clone();
        let EntityKind::LwPolyline(p) = &e.kind else { panic!("{:?}", e.kind) };
        assert!(p.closed && p.vertices.len() == 4, "{p:?}");
    }
    // Less than half the passage: still one.
    let r = s.execute("offset", &json!({"handle": pl, "distance": 0.25, "side": [5, 5]})).unwrap();
    assert_eq!(r["handles"].as_array().unwrap().len(), 1);
    // Undo takes the pieces of one OFFSET together.
    let n = s.doc().unwrap().model.len();
    s.undo().unwrap();
    s.undo().unwrap();
    assert_eq!(s.doc().unwrap().model.len(), n - 3);
}
