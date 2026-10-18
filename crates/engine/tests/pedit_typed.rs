//! PEDIT typed as in AutoCAD scripts: a selection word at "Select polyline", and Multiple.

use cadaddy_engine::Session;
use cadaddy_engine::doc::EntityKind;
use serde_json::json;

fn polylines(s: &Session) -> Vec<cadaddy_engine::doc::LwPolyline> {
    s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::LwPolyline(p) = &e.kind { Some(p.clone()) } else { None }).collect()
}

#[test]
fn pedit_multiple_joins_lines_into_one_polyline() {
    let mut s = Session::new();
    for (a, b) in [([0, 0], [10, 0]), ([10, 0], [10, 10]), ([10, 10], [0, 10])] {
        s.execute("line", &json!({"points": [a, b]})).unwrap();
    }
    s.script("pedit m all  y j 0 \n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    let p = polylines(&s);
    assert_eq!(p.len(), 1, "{:?}", s.log.iter().rev().take(4).collect::<Vec<_>>());
    assert_eq!(p[0].vertices.len(), 4);
    assert_eq!(s.doc().unwrap().model.iter().count(), 1);
}

#[test]
fn pedit_last_then_width() {
    let mut s = Session::new();
    s.execute("pline", &json!({"points": [[0, 0], [5, 0], [5, 5]]})).unwrap();
    s.script("pedit l w 0.5 \n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_eq!(polylines(&s)[0].const_width, 0.5);
}
