//! DIMBREAK follows its objects (gaps F7): automatic breaks are worked out again when the
//! crossing objects or the dimension move, change or go; their size comes from the dimension's
//! style; manual breaks stay; DIMBREAK Remove works on several dimensions; the followed objects
//! survive a DXF round trip.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Dimension, EntityKind, Handle};
use serde_json::{Value, json};

fn dim(s: &Session, h: &str) -> Dimension {
    match &s.doc().unwrap().entity(Handle::parse_hex(h).unwrap()).unwrap().kind {
        EntityKind::Dimension(dm) => dm.clone(),
        k => panic!("not a dimension: {k:?}"),
    }
}

fn handle(r: &Value) -> String {
    r["handle"].as_str().or_else(|| r["handles"][0].as_str()).unwrap().to_string()
}

/// A linear dimension along y = 3 from x = 0 to 10, and a vertical line through it at x = 4.
fn setup() -> (Session, String, String) {
    let mut s = Session::new();
    let h = handle(&s.execute("dimlinear", &json!({ "p1": [0, 0], "p2": [10, 0], "at": [5, 3] })).unwrap());
    let l = handle(&s.execute("line", &json!({ "points": [[4, -1], [4, 6]] })).unwrap());
    s.execute("dimbreak", &json!({ "handles": [h] })).unwrap();
    (s, h, l)
}

/// The x of the dimension's automatic breaks on its dimension line (y = 3).
fn auto_xs(s: &Session, h: &str) -> Vec<f64> {
    let mut xs: Vec<f64> =
        dim(s, h).marks.breaks.iter().filter(|b| b.auto && (b.at.y - 3.0).abs() < 1e-9).map(|b| (b.at.x * 1e6).round() / 1e6).collect();
    xs.sort_by(f64::total_cmp);
    xs
}

#[test]
fn breaks_follow_a_moved_object() {
    let (mut s, h, l) = setup();
    assert_eq!(auto_xs(&s, &h), [4.0]);
    assert_eq!(dim(&s, &h).marks.break_objects.iter().map(|x| x.hex()).collect::<Vec<_>>(), std::slice::from_ref(&l));
    s.execute("move", &json!({ "handles": [l], "delta": [3, 0] })).unwrap();
    assert_eq!(auto_xs(&s, &h), [7.0]);
    // Moved off the dimension: no break, but it is still followed and breaks again coming back.
    s.execute("move", &json!({ "handles": [l], "delta": [20, 0] })).unwrap();
    assert!(auto_xs(&s, &h).is_empty());
    // (Clear of the text in the middle, where the dimension line has a gap.)
    s.execute("move", &json!({ "handles": [l], "delta": [-24, 0] })).unwrap();
    assert_eq!(auto_xs(&s, &h), [3.0]);
    // One undo step puts the break back with the object.
    s.undo().unwrap();
    assert!(auto_xs(&s, &h).is_empty());
}

#[test]
fn breaks_follow_a_moved_dimension_and_an_erased_object() {
    let (mut s, h, l) = setup();
    s.execute("move", &json!({ "handles": [h], "delta": [-2, 0] })).unwrap();
    assert_eq!(auto_xs(&s, &h), [4.0], "the line stays put; the break stays at the crossing");
    s.execute("erase", &json!({ "handles": [l] })).unwrap();
    let dm = dim(&s, &h);
    assert!(dm.marks.breaks.is_empty());
    assert!(dm.marks.break_objects.is_empty());
}

#[test]
fn manual_breaks_stay_and_the_style_sets_the_size() {
    let (mut s, h, l) = setup();
    s.execute("dimbreak", &json!({ "handles": [h], "mode": "manual", "first": [8, 3], "second": [9, 3] })).unwrap();
    s.execute("move", &json!({ "handles": [l], "delta": [-1, 0] })).unwrap();
    let dm = dim(&s, &h);
    assert_eq!(dm.marks.breaks.iter().filter(|b| !b.auto).count(), 1);
    assert!(dm.marks.breaks.iter().any(|b| !b.auto && (b.at.x - 8.5).abs() < 1e-9));
    // The style's break size: changing it resizes the automatic breaks.
    let auto_size = |s: &Session| dim(s, &h).marks.breaks.iter().find(|b| b.auto).map(|b| b.size).unwrap();
    assert!((auto_size(&s) - 0.125).abs() < 1e-12);
    s.execute("dimstyle", &json!({ "name": "Standard", "breakSize": 0.4 })).unwrap();
    assert!((auto_size(&s) - 0.4).abs() < 1e-9, "{}", auto_size(&s));
    // A size given to DIMBREAK stays while things move.
    s.execute("dimbreak", &json!({ "handles": [h], "size": 1.0 })).unwrap();
    s.execute("move", &json!({ "handles": [l], "delta": [-1, 0] })).unwrap();
    assert!((auto_size(&s) - 1.0).abs() < 1e-12);
}

#[test]
fn only_the_chosen_objects_are_followed() {
    let (mut s, h, l) = setup();
    let other = handle(&s.execute("line", &json!({ "points": [[6, -1], [6, 6]] })).unwrap());
    s.execute("dimbreak", &json!({ "handles": [h], "objects": [other] })).unwrap();
    assert_eq!(auto_xs(&s, &h), [6.0]);
    // Moving the line that isn't followed changes nothing.
    s.execute("move", &json!({ "handles": [l], "delta": [1, 0] })).unwrap();
    assert_eq!(auto_xs(&s, &h), [6.0]);
    s.execute("move", &json!({ "handles": [other], "delta": [1, 0] })).unwrap();
    assert_eq!(auto_xs(&s, &h), [7.0]);
}

#[test]
fn remove_works_on_several_dimensions() {
    let (mut s, h, _) = setup();
    let h2 = handle(&s.execute("dimlinear", &json!({ "p1": [0, 0], "p2": [10, 0], "at": [5, 5] })).unwrap());
    s.execute("dimbreak", &json!({ "handles": [h2] })).unwrap();
    assert!(!dim(&s, &h2).marks.breaks.is_empty());
    let r = s.execute("dimbreak", &json!({ "handles": [h, h2], "mode": "remove" })).unwrap();
    assert_eq!(r["changed"], 2);
    for x in [&h, &h2] {
        let dm = dim(&s, x);
        assert!(dm.marks.breaks.is_empty() && dm.marks.break_objects.is_empty());
    }
    // Typed: Multiple, both dimensions, Remove.
    s.execute("dimbreak", &json!({ "handles": [h, h2] })).unwrap();
    s.cmdline("dimbreak").unwrap();
    s.cmdline("m").unwrap();
    s.input(cadaddy_engine::Input::Pick(vec![Handle::parse_hex(&h).unwrap(), Handle::parse_hex(&h2).unwrap()])).unwrap();
    s.input(cadaddy_engine::Input::Enter).unwrap();
    s.cmdline("r").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert!(dim(&s, &h).marks.breaks.is_empty() && dim(&s, &h2).marks.breaks.is_empty());
}

#[test]
fn followed_objects_survive_dxf() {
    let (s, h, l) = setup();
    let text = cadaddy_io::write_dxf(s.doc().unwrap());
    assert!(text.contains("BREAKOBJ"));
    let back = cadaddy_io::read_dxf(text.as_bytes()).unwrap();
    let mut s2 = Session::new();
    s2.open_drawing(back, "x", None);
    assert_eq!(dim(&s2, &h).marks.break_objects.iter().map(|x| x.hex()).collect::<Vec<_>>(), std::slice::from_ref(&l));
    s2.execute("move", &json!({ "handles": [l], "delta": [2, 0] })).unwrap();
    assert_eq!(auto_xs(&s2, &h), [6.0]);
}
