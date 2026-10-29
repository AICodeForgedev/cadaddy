//! TRIM and BREAK modify the object in place (as AutoCAD does): the original handle keeps the
//! piece holding the start point with its layer and xdata, further pieces are new objects that
//! copy both, associative dimensions follow, and UNDO / REDO restore and re-apply the exact edit.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{DimAssoc, Dimension, EntityKind, Handle, RawTag};
use serde_json::{Value, json};

fn hd(v: &Value) -> Handle {
    let s = v.get("handle").or_else(|| v["handles"].get(0)).and_then(Value::as_str).unwrap();
    Handle::parse_hex(s).unwrap()
}

fn kind(s: &Session, h: Handle) -> Option<EntityKind> {
    s.doc().unwrap().entity(h).map(|e| e.kind.clone())
}

fn count(s: &Session) -> usize {
    s.doc().unwrap().model.iter().count()
}

fn dim(s: &Session, h: Handle) -> Dimension {
    match kind(s, h) {
        Some(EntityKind::Dimension(d)) => *d,
        other => panic!("not a dimension: {other:?}"),
    }
}

/// Put the object on layer RED and give it xdata, as an application or the BIM would.
fn mark(s: &mut Session, h: Handle) {
    let _ = s.execute("layer.new", &json!({ "name": "RED", "color": "red" }));
    s.execute("chprop", &json!({ "handles": [h.hex()], "layer": "RED" })).unwrap();
    s.doc_mut().unwrap().modify_entity(h, |e| e.common.xdata.push(RawTag { code: 1000, value: "tag:keep-me".into() })).unwrap();
}

fn assert_marked(s: &Session, h: Handle) {
    let e = s.doc().unwrap().entity(h).unwrap_or_else(|| panic!("{} is gone", h.hex()));
    assert_eq!(&*e.common.layer, "RED");
    assert!(e.common.xdata.iter().any(|t| t.value == "tag:keep-me"), "xdata lost on {}", h.hex());
}

/// Run `cmd` with `params` on `h` and check the handle, properties, undo and redo.
/// `pieces`: how many objects the edit leaves. Returns the handles it reports.
fn edit_checks(s: &mut Session, cmd: &str, params: Value, h: Handle, pieces: usize) -> Vec<Handle> {
    mark(s, h);
    let before = kind(s, h).unwrap();
    let n0 = count(s);
    let r = s.execute(cmd, &params).unwrap();
    let hs: Vec<Handle> = r["handles"].as_array().unwrap().iter().map(|v| Handle::parse_hex(v.as_str().unwrap()).unwrap()).collect();
    assert_eq!(hs.len(), pieces, "{cmd}: {r}");
    assert_eq!(hs[0], h, "{cmd}: the original handle keeps the first piece");
    let after = kind(s, h).unwrap();
    assert_ne!(after, before, "{cmd}: the object changed");
    for p in &hs {
        assert_marked(s, *p);
    }
    assert_eq!(count(s), n0 + pieces - 1);
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(kind(s, h), Some(before), "{cmd}: undo restores the exact original");
    assert_eq!(count(s), n0);
    assert_marked(s, h);
    s.execute("redo", &json!({})).unwrap();
    assert_eq!(kind(s, h), Some(after), "{cmd}: redo re-applies");
    assert_eq!(count(s), n0 + pieces - 1);
    hs
}

fn cutter(s: &mut Session, x: f64) -> String {
    hd(&s.execute("line", &json!({ "points": [[x, -50], [x, 50]] })).unwrap()).hex()
}

fn trim_case(make: impl FnOnce(&mut Session) -> Handle, cutters: &[f64], pick: [f64; 2], pieces: usize) {
    let mut s = Session::new();
    let h = make(&mut s);
    let edges: Vec<String> = cutters.iter().map(|x| cutter(&mut s, *x)).collect();
    edit_checks(&mut s, "trim", json!({ "handle": h.hex(), "pick": pick, "edges": edges }), h, pieces);
}

#[test]
fn trim_a_line_keeps_the_handle() {
    trim_case(|s| hd(&s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap()), &[5.0], [8.0, 0.0], 1);
}

#[test]
fn trim_the_middle_of_a_line_leaves_the_start_piece_on_the_handle() {
    let mut s = Session::new();
    let h = hd(&s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap());
    let edges = [cutter(&mut s, 3.0), cutter(&mut s, 7.0)];
    let hs = edit_checks(&mut s, "trim", json!({ "handle": h.hex(), "pick": [5, 0], "edges": edges }), h, 2);
    let EntityKind::Line(a) = kind(&s, hs[0]).unwrap() else { panic!("line") };
    let EntityKind::Line(b) = kind(&s, hs[1]).unwrap() else { panic!("line") };
    assert!((a.a.x - 0.0).abs() < 1e-9 && (a.b.x - 3.0).abs() < 1e-9);
    assert!((b.a.x - 7.0).abs() < 1e-9 && (b.b.x - 10.0).abs() < 1e-9);
}

#[test]
fn trim_an_arc_keeps_the_handle() {
    trim_case(|s| hd(&s.execute("arc", &json!({ "center": [0, 0], "radius": 5.0, "start": 0.0, "end": 180.0 })).unwrap()), &[3.0], [4.7, 1.7], 1);
}

#[test]
fn trim_a_circle_into_an_arc_keeps_the_handle() {
    let mut s = Session::new();
    let h = hd(&s.execute("circle", &json!({ "center": [0, 0], "radius": 5.0 })).unwrap());
    let edges = [cutter(&mut s, 3.0), cutter(&mut s, -3.0)];
    edit_checks(&mut s, "trim", json!({ "handle": h.hex(), "pick": [5, 0], "edges": edges }), h, 1);
    assert!(matches!(kind(&s, h), Some(EntityKind::Arc(_))));
}

#[test]
fn trim_a_polyline_keeps_the_handle() {
    trim_case(|s| hd(&s.execute("pline", &json!({ "vertices": [[0, 0], [10, 0], [10, 10]] })).unwrap()), &[5.0], [8.0, 0.0], 1);
}

#[test]
fn trim_a_spline_keeps_the_handle() {
    trim_case(|s| hd(&s.execute("spline", &json!({ "fit": [[0, 0], [4, 4], [8, 0], [12, 4]] })).unwrap()), &[6.0], [10.0, 2.0], 1);
}

#[test]
fn trim_an_ellipse_keeps_the_handle() {
    let mut s = Session::new();
    let h = hd(&s.execute("ellipse", &json!({ "center": [0, 0], "major": [10, 0], "ratio": 0.5 })).unwrap());
    let edges = [cutter(&mut s, 5.0), cutter(&mut s, -5.0)];
    edit_checks(&mut s, "trim", json!({ "handle": h.hex(), "pick": [10, 0], "edges": edges }), h, 1);
}

#[test]
fn break_keeps_the_handle_for_every_shape() {
    let mut s = Session::new();
    let l = hd(&s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap());
    edit_checks(&mut s, "break", json!({ "handle": l.hex(), "p1": [3, 0], "p2": [6, 0] }), l, 2);
    let a = hd(&s.execute("arc", &json!({ "center": [0, 20], "radius": 5.0, "start": 0.0, "end": 180.0 })).unwrap());
    edit_checks(&mut s, "break", json!({ "handle": a.hex(), "p1": [3, 24], "p2": [-3, 24] }), a, 2);
    let c = hd(&s.execute("circle", &json!({ "center": [0, 40], "radius": 5.0 })).unwrap());
    edit_checks(&mut s, "break", json!({ "handle": c.hex(), "p1": [5, 40], "p2": [-5, 40] }), c, 1);
    let p = hd(&s.execute("pline", &json!({ "vertices": [[0, 60], [10, 60], [10, 70]] })).unwrap());
    edit_checks(&mut s, "break", json!({ "handle": p.hex(), "p1": [4, 60], "p2": [7, 60] }), p, 2);
    let e = hd(&s.execute("ellipse", &json!({ "center": [0, 90], "major": [10, 0], "ratio": 0.5 })).unwrap());
    edit_checks(&mut s, "break", json!({ "handle": e.hex(), "p1": [10, 90], "p2": [-10, 90] }), e, 1);
    let sp = hd(&s.execute("spline", &json!({ "fit": [[0, 110], [4, 114], [8, 110], [12, 114]] })).unwrap());
    edit_checks(&mut s, "break", json!({ "handle": sp.hex(), "p1": [4, 114], "p2": [8, 110] }), sp, 2);
}

#[test]
fn breakatpoint_keeps_the_handle() {
    let mut s = Session::new();
    let l = hd(&s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap());
    edit_checks(&mut s, "breakatpoint", json!({ "handle": l.hex(), "at": [4, 0] }), l, 2);
}

/// The MCP repro: a dimension on a trimmed line reads the new length, the line keeps its handle.
#[test]
fn the_dimension_follows_a_trimmed_line() {
    let mut s = Session::new();
    let h = hd(&s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap());
    let e = hd(&s.execute("line", &json!({ "points": [[5, -5], [5, 5]] })).unwrap());
    let d = hd(&s.execute("dimlinear", &json!({ "object": h.hex(), "at": [0, -3] })).unwrap());
    let before = dim(&s, d);
    assert_eq!(before.assoc.len(), 2);
    let r = s.execute("trim", &json!({ "handle": h.hex(), "pick": [8, 0], "edges": [e.hex()] })).unwrap();
    assert_eq!(hd(&r), h);
    let after = dim(&s, d);
    assert!(after.marks.orphaned.is_empty(), "{:?}", after.marks.orphaned);
    assert_eq!(after.assoc.len(), 2);
    assert!((after.p14.x - 5.0).abs() < 1e-9 && (after.p13.x).abs() < 1e-9, "p13 {:?} p14 {:?}", after.p13, after.p14);
    let EntityKind::Line(l) = kind(&s, h).unwrap() else { panic!("line") };
    assert!((l.b.x - 5.0).abs() < 1e-9);
    // Undo: the original handle and the old value; redo: the new one again.
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(dim(&s, d), before);
    s.execute("redo", &json!({})).unwrap();
    assert_eq!(dim(&s, d).p14.x, after.p14.x);
}

/// Cut the line in the middle: the dimension's far end now sits on the new piece and follows it.
#[test]
fn the_dimension_moves_to_the_piece_that_holds_its_point() {
    let mut s = Session::new();
    let h = hd(&s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap());
    let d = hd(&s.execute("dimlinear", &json!({ "object": h.hex(), "at": [0, -3] })).unwrap());
    let r = s.execute("break", &json!({ "handle": h.hex(), "p1": [4, 0], "p2": [6, 0] })).unwrap();
    let hs: Vec<String> = r["handles"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    assert_eq!(hs.len(), 2);
    let dm = dim(&s, d);
    assert!(dm.marks.orphaned.is_empty());
    let owners: Vec<&DimAssoc> = dm.assoc.iter().collect();
    assert_eq!(owners.len(), 2);
    assert!(owners.iter().any(|a| a.handle == h) && owners.iter().any(|a| a.handle.hex() == hs[1]), "{owners:?}");
    assert!((dm.p13.x).abs() < 1e-9 && (dm.p14.x - 10.0).abs() < 1e-9);
}

/// A point that cannot follow (the midpoint, now cut away) lets go with a note, never stale.
#[test]
fn a_dimension_point_on_the_removed_part_becomes_non_associative() {
    let mut s = Session::new();
    let h = hd(&s.execute("line", &json!({ "points": [[0, 0], [10, 0]] })).unwrap());
    let d = hd(&s.execute("dimlinear", &json!({ "p1": [0, 0], "p2": [5, 0], "at": [0, -3] })).unwrap());
    // Attach p14 to the line's midpoint.
    s.doc_mut()
        .unwrap()
        .modify_entity(d, |e| {
            if let EntityKind::Dimension(dm) = &mut e.kind {
                dm.assoc = vec![DimAssoc { point: "p14".into(), handle: h, snap: cadaddy_engine::doc::AssocSnap::Mid }];
            }
        })
        .unwrap();
    let cut = hd(&s.execute("line", &json!({ "points": [[2, -5], [2, 5]] })).unwrap());
    s.execute("trim", &json!({ "handle": h.hex(), "pick": [8, 0], "edges": [cut.hex()] })).unwrap();
    let dm = dim(&s, d);
    assert!(dm.assoc.is_empty());
    assert_eq!(dm.marks.orphaned, vec!["p14".to_string()]);
    assert!(s.log.iter().any(|l| l.contains("no longer associative")), "{:?}", s.log);
}
