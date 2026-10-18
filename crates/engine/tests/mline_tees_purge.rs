//! MLEDIT tees end each element of the first multiline where it meets the second (not square
//! across), the Merged tee joins the inner elements to the second's centre element, and PURGE
//! keeps multiline styles that are used anywhere, current or Standard.

use cadaddy_doc::{EntityKind, Handle, MLine};
use cadaddy_engine::Session;
use serde_json::{Value, json};

fn h(v: &Value) -> Handle {
    Handle::parse_hex(v["handle"].as_str().unwrap()).unwrap()
}

fn mline(s: &Session, hd: Handle) -> MLine {
    match &s.doc().unwrap().entity(hd).unwrap().kind {
        EntityKind::MLine(m) => m.clone(),
        k => panic!("not a multiline: {k:?}"),
    }
}

fn r(v: f64) -> f64 {
    (v * 1e6).round() / 1e6 + 0.0
}

/// Element pieces of a multiline, rounded, sorted: (x1, y1, x2, y2).
fn pieces(m: &MLine) -> Vec<[f64; 4]> {
    let mut v: Vec<[f64; 4]> = m.geometry().lines.iter().map(|(_, a, b)| [r(a.x), r(a.y), r(b.x), r(b.y)]).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v
}

fn add(s: &mut Session, pts: Value, scale: f64, style: Option<&str>) -> Handle {
    let mut p = json!({ "points": pts, "justification": "zero", "scale": scale });
    if let (Some(st), Some(o)) = (style, p.as_object_mut()) {
        o.insert("style".into(), json!(st));
    }
    h(&s.execute("mline", &p).unwrap())
}

/// A horizontal multiline at y = 5 with elements at y = 6 and y = 4, and a 45° one running up
/// into it from below with elements at ±1 across its path y = x − 10.
fn slanted(s: &mut Session) -> (Handle, Handle) {
    let b = add(s, json!([[0, 5], [20, 5]]), 2.0, None);
    let a = add(s, json!([[0, -10], [20, 10]]), 2.0, None);
    (a, b)
}

#[test]
fn closed_tee_ends_each_element_on_the_second_multiline() {
    let mut s = Session::new();
    let (a, b) = slanted(&mut s);
    s.execute("mledit", &json!({"tool": "CT", "first": a.hex(), "p1": [2, -8], "second": b.hex(), "p2": [18, 5]})).unwrap();
    let k = 2f64.sqrt();
    // Element lines y = x − 10 ± √2 reach y = 4 at x = 14 ∓ √2.
    let ends: Vec<[f64; 2]> = pieces(&mline(&s, a)).iter().map(|p| [p[2], p[3]]).collect();
    assert_eq!(ends, vec![[r(14.0 - k), 4.0], [r(14.0 + k), 4.0]], "not cut square");
    assert!(mline(&s, a).no_end_caps);
    // Closed: the second multiline is untouched.
    assert_eq!(pieces(&mline(&s, b)), vec![[0.0, 4.0, 20.0, 4.0], [0.0, 6.0, 20.0, 6.0]]);
}

#[test]
fn open_tee_opens_the_second_between_the_slanted_elements() {
    let mut s = Session::new();
    let (a, b) = slanted(&mut s);
    s.execute("mledit", &json!({"tool": "OT", "first": a.hex(), "p1": [2, -8], "second": b.hex(), "p2": [18, 5]})).unwrap();
    let k = 2f64.sqrt();
    assert_eq!(pieces(&mline(&s, b)), vec![[0.0, 4.0, r(14.0 - k), 4.0], [0.0, 6.0, 20.0, 6.0], [r(14.0 + k), 4.0, 20.0, 4.0]]);
}

#[test]
fn tee_keeping_the_far_part_starts_each_element_on_the_second() {
    let mut s = Session::new();
    let (a, b) = slanted(&mut s);
    // Picked above the second multiline: the part after it is kept, starting at y = 6.
    s.execute("mledit", &json!({"tool": "CT", "first": a.hex(), "p1": [19, 9], "second": b.hex(), "p2": [2, 5]})).unwrap();
    let k = 2f64.sqrt();
    let starts: Vec<[f64; 2]> = pieces(&mline(&s, a)).iter().map(|p| [p[0], p[1]]).collect();
    assert_eq!(starts, vec![[r(16.0 - k), 6.0], [r(16.0 + k), 6.0]]);
    assert!(mline(&s, a).no_start_caps);
}

#[test]
fn merged_tee_joins_inner_elements_to_the_centre() {
    let mut s = Session::new();
    s.execute("mlstyle", &json!({"name": "T3", "elements": [{"offset": 1}, {"offset": 0}, {"offset": -1}], "current": false})).unwrap();
    let b = add(&mut s, json!([[0, 5], [20, 5]]), 1.0, Some("T3"));
    let a = add(&mut s, json!([[10, -10], [10, 10]]), 1.0, Some("T3"));
    s.execute("mledit", &json!({"tool": "MT", "first": a.hex(), "p1": [10, -8], "second": b.hex(), "p2": [2, 5]})).unwrap();
    // The outer elements stop at the near side; the centre element runs on to the centre.
    assert_eq!(pieces(&mline(&s, a)), vec![[9.0, -10.0, 9.0, 4.0], [10.0, -10.0, 10.0, 5.0], [11.0, -10.0, 11.0, 4.0]]);
    // The near side opens between them; the centre and far side stay whole.
    assert_eq!(pieces(&mline(&s, b)), vec![[0.0, 4.0, 9.0, 4.0], [0.0, 5.0, 20.0, 5.0], [0.0, 6.0, 20.0, 6.0], [11.0, 4.0, 20.0, 4.0]]);

    // Five elements: the one between the near side and the centre opens too.
    let mut s = Session::new();
    s.execute(
        "mlstyle",
        &json!({"name": "T5", "elements": [{"offset": 2}, {"offset": 1}, {"offset": 0}, {"offset": -1}, {"offset": -2}], "current": false}),
    )
    .unwrap();
    let b = add(&mut s, json!([[0, 5], [20, 5]]), 1.0, Some("T5"));
    let a = add(&mut s, json!([[10, -10], [10, 10]]), 1.0, Some("T5"));
    s.execute("mledit", &json!({"tool": "MT", "first": a.hex(), "p1": [10, -8], "second": b.hex(), "p2": [2, 5]})).unwrap();
    let bp = pieces(&mline(&s, b));
    assert!(bp.contains(&[0.0, 3.0, 8.0, 3.0]) && bp.contains(&[0.0, 4.0, 8.0, 4.0]) && bp.contains(&[12.0, 4.0, 20.0, 4.0]), "{bp:?}");
    assert!(bp.contains(&[0.0, 5.0, 20.0, 5.0]) && bp.contains(&[0.0, 7.0, 20.0, 7.0]), "{bp:?}");
    let ap = pieces(&mline(&s, a));
    assert!(ap.contains(&[8.0, -10.0, 8.0, 3.0]) && ap.contains(&[12.0, -10.0, 12.0, 3.0]) && ap.contains(&[10.0, -10.0, 10.0, 5.0]), "{ap:?}");

    // With two elements a merged tee is an open tee.
    let mut s = Session::new();
    let (a, b) = slanted(&mut s);
    s.execute("mledit", &json!({"tool": "MT", "first": a.hex(), "p1": [2, -8], "second": b.hex(), "p2": [18, 5]})).unwrap();
    let k = 2f64.sqrt();
    assert_eq!(pieces(&mline(&s, b)), vec![[0.0, 4.0, r(14.0 - k), 4.0], [0.0, 6.0, 20.0, 6.0], [r(14.0 + k), 4.0, 20.0, 4.0]]);
    // One undo step brings both back.
    s.undo().unwrap();
    assert_eq!(pieces(&mline(&s, b)), vec![[0.0, 4.0, 20.0, 4.0], [0.0, 6.0, 20.0, 6.0]]);
    assert_eq!(mline(&s, a).points().len(), 2);
}

#[test]
fn typed_merged_tee() {
    let mut s = Session::new();
    let (_, b) = slanted(&mut s);
    s.cmdline("-MLEDIT MT").unwrap();
    assert_eq!(s.current_prompt().unwrap().display(), "Select first mline:");
    s.cmdline("2,-6.5857864").unwrap();
    s.cmdline("18,6").unwrap();
    s.cmdline("").unwrap();
    assert!(s.current_prompt().is_none());
    let k = 2f64.sqrt();
    assert_eq!(pieces(&mline(&s, b)), vec![[0.0, 4.0, r(14.0 - k), 4.0], [0.0, 6.0, 20.0, 6.0], [r(14.0 + k), 4.0, 20.0, 4.0]]);
}

#[test]
fn purge_keeps_multiline_styles_used_in_layouts_and_purges_with_all() {
    let mut s = Session::new();
    for n in ["InLayout", "Spare"] {
        s.execute("mlstyle", &json!({"name": n, "current": false})).unwrap();
    }
    s.execute("layout.set", &json!({"name": "Layout1"})).unwrap();
    s.execute("mline", &json!({"points": [[0, 0], [5, 0]], "style": "inlayout"})).unwrap();
    let r = s.execute("purge", &json!({})).unwrap();
    assert_eq!(r["purged"]["mlinestyles"], json!(["Spare"]));
    let names: Vec<String> = s.doc().unwrap().mline_styles.iter().map(|m| m.name.clone()).collect();
    assert!(names.iter().any(|n| n == "InLayout") && names.iter().any(|n| n.eq_ignore_ascii_case("Standard")), "{names:?}");
}
