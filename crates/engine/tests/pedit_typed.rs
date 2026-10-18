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

/// Many separate chains, picked in a scrambled order and drawn in both directions, each join into
/// one polyline, and quickly: the join looks ends up in a grid instead of comparing every object
/// with every other (a 10,000-line selection took minutes).
#[test]
fn pedit_multiple_join_scales_to_thousands_of_segments() {
    let mut s = Session::new();
    let (chains, segs) = (200usize, 50usize);
    let mut order: Vec<(usize, usize)> = (0..chains).flat_map(|c| (0..segs).map(move |k| (c, k))).collect();
    // A fixed scramble (no rng dependency): stride through the list.
    let n = order.len();
    order = (0..n).map(|i| order[(i * 7919) % n]).collect();
    for (c, k) in order {
        let y = c as f64 * 10.0;
        let (a, b) = ([k as f64, y + (k % 2) as f64], [(k + 1) as f64, y + ((k + 1) % 2) as f64]);
        let (a, b) = if k % 3 == 0 { (b, a) } else { (a, b) };
        s.add_entity(EntityKind::Line(cadaddy_engine::doc::Line {
            a: cadaddy_engine::geom::Vec3::new(a[0], a[1], 0.0),
            b: cadaddy_engine::geom::Vec3::new(b[0], b[1], 0.0),
        }))
        .unwrap();
    }
    let t = std::time::Instant::now();
    s.script("pedit m all  y j 0 \n").unwrap();
    let took = t.elapsed();
    let p = polylines(&s);
    assert_eq!(p.len(), chains, "{:?}", s.log.iter().rev().take(4).collect::<Vec<_>>());
    assert!(p.iter().all(|p| p.vertices.len() == segs + 1 && !p.closed));
    assert!(took.as_secs_f64() < 20.0, "join took {took:?}");
}

#[test]
fn pedit_last_then_width() {
    let mut s = Session::new();
    s.execute("pline", &json!({"points": [[0, 0], [5, 0], [5, 5]]})).unwrap();
    s.script("pedit l w 0.5 \n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_eq!(polylines(&s)[0].const_width, 0.5);
}
