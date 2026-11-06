//! Geometric tolerance: the frame string taken apart for the dialog and put back, TOLERANCE with a
//! whole frame string, TOLERANCE.EDIT, and the Tolerance annotation of LEADER and QLEADER.

use cadaddy_engine::Session;
use cadaddy_engine::cmd::tolerance::frame_from_text;
use cadaddy_engine::doc::{EntityKind, GdtDatum, GdtFrame, GdtLine, GdtSymbol, GdtTol, Tolerance, compose_frame};
use serde_json::json;

fn tolerances(s: &Session) -> Vec<Tolerance> {
    s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Tolerance(t) = &e.kind { Some(t.clone()) } else { None }).collect()
}

fn leaders(s: &Session) -> usize {
    s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Leader(_))).count()
}

fn full_frame() -> GdtFrame {
    GdtFrame {
        lines: vec![
            GdtLine {
                symbol: Some(GdtSymbol::Position),
                tol1: Some(GdtTol { diameter: true, value: "0.05".into(), material: Some("M".into()) }),
                tol2: None,
                datums: vec![GdtDatum { letter: "A".into(), material: None }, GdtDatum { letter: "B".into(), material: Some("L".into()) }],
            },
            GdtLine {
                symbol: Some(GdtSymbol::Flatness),
                tol1: Some(GdtTol { diameter: false, value: "0.01".into(), material: None }),
                tol2: None,
                datums: vec![],
            },
        ],
        height: "12".into(),
        projected: true,
        datum_id: "C".into(),
    }
}

#[test]
fn a_frame_string_comes_apart_into_the_dialog_fields() {
    let f = full_frame();
    assert_eq!(frame_from_text(&compose_frame(&f)), f);
    // One line, the height alone, the datum identifier alone.
    for f in [
        GdtFrame { lines: vec![GdtLine { symbol: Some(GdtSymbol::Symmetry), ..Default::default() }], ..Default::default() },
        GdtFrame { lines: vec![GdtLine { symbol: Some(GdtSymbol::Position), ..Default::default() }], height: "5".into(), ..Default::default() },
        GdtFrame { lines: vec![], datum_id: "D".into(), ..Default::default() },
        GdtFrame {
            lines: vec![GdtLine { symbol: None, tol1: Some(GdtTol { diameter: false, value: "1".into(), material: None }), ..Default::default() }],
            ..Default::default()
        },
    ] {
        assert_eq!(frame_from_text(&compose_frame(&f)), f, "{}", compose_frame(&f));
    }
    // Hostile text never panics.
    let _ = frame_from_text("{\\Fgdt;%%v^J^J%%v{");
}

#[test]
fn tolerance_takes_a_whole_frame_string_and_asks_the_location() {
    let mut s = Session::new();
    let text = compose_frame(&full_frame());
    s.cmdline("TOLERANCE").unwrap();
    s.cmdline(&text).unwrap();
    assert_eq!(s.current_prompt().unwrap().display(), "Enter tolerance location:");
    s.cmdline("4,5").unwrap();
    let t = tolerances(&s);
    assert_eq!(t.len(), 1);
    assert_eq!((t[0].text.as_str(), t[0].insert.x, t[0].insert.y), (text.as_str(), 4.0, 5.0));
    // The questions one by one still work.
    s.script("TOLERANCE\nFlatness\n0.02\n\n\n1,1\n").unwrap();
    assert_eq!(tolerances(&s).len(), 2);
}

#[test]
fn tolerance_edit_changes_the_contents_in_place() {
    let mut s = Session::new();
    let r = s.execute("tolerance", &json!({ "at": [1, 2], "symbol": "position", "tol1": "0.1" })).unwrap();
    let h = r["handle"].as_str().unwrap().to_string();
    let r = s.execute("tolerance.edit", &json!({ "handle": h, "symbol": "flatness", "tol1": "0.3" })).unwrap();
    let t = &tolerances(&s)[0];
    assert_eq!(t.text, r["text"].as_str().unwrap());
    assert!(t.text.contains("0.3") && (t.insert.x, t.insert.y) == (1.0, 2.0));
    assert_eq!(frame_from_text(&t.text).lines[0].symbol, Some(GdtSymbol::Flatness));
    // By frame string; undo puts the old one back.
    let text = compose_frame(&full_frame());
    s.execute("tolerance.edit", &json!({ "handle": h, "text": text })).unwrap();
    assert_eq!(tolerances(&s)[0].text, text);
    s.undo().unwrap();
    assert!(tolerances(&s)[0].text.contains("0.3"));
    // Errors: empty frame, not a tolerance.
    assert!(s.execute("tolerance.edit", &json!({ "handle": h, "text": "" })).is_err());
    let line = s.execute("line", &json!({ "points": [[0, 0], [1, 0]] })).unwrap();
    let lh = line["handle"].as_str().or(line["handles"][0].as_str()).unwrap().to_string();
    assert!(s.execute("tolerance.edit", &json!({ "handle": lh, "text": text })).is_err());
}

#[test]
fn leaders_end_with_a_feature_control_frame() {
    // JSON: a frame string or fields.
    let mut s = Session::new();
    let r = s
        .execute("leader", &json!({ "points": [[0, 0], [5, 5]], "tolerance": { "symbol": "position", "tol1": "%%c0.1", "datums": ["A"] } }))
        .unwrap();
    assert_eq!(r["handles"].as_array().unwrap().len(), 2);
    let t = &tolerances(&s)[0];
    assert_eq!((t.insert.x, t.insert.y), (5.0, 5.0), "at the end of the leader");
    assert!(s.execute("qleader", &json!({ "points": [[0, 0], [5, 5]], "tolerance": "" })).is_err());
    assert_eq!(leaders(&s), 1, "nothing added for an empty frame");

    // LEADER typed: Enter at the first text line, then the Tolerance option and the questions.
    let mut s = Session::new();
    s.script("LEADER 0,0 4,4\n\n\nTolerance\nPosition\n0.2\n\nA\n\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_eq!((leaders(&s), tolerances(&s).len()), (1, 1));
    // None: the leader alone.
    let mut s = Session::new();
    s.script("LEADER 0,0 4,4\n\n\nNone\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_eq!((leaders(&s), tolerances(&s).len()), (1, 0));

    // QLEADER: Settings, Tolerance, the points, then the questions.
    let mut s = Session::new();
    s.cmdline("QLEADER").unwrap();
    assert_eq!(s.current_prompt().unwrap().display(), "Specify first leader point, or [Settings]:");
    s.script("S\nT\n0,0 3,3 6,3\nFlatness\n0.05\n\n\n").unwrap();
    assert!(s.running.is_none(), "{}", s.prompt_text());
    assert_eq!((leaders(&s), tolerances(&s).len()), (1, 1));
    assert_eq!(frame_from_text(&tolerances(&s)[0].text).lines[0].symbol, Some(GdtSymbol::Flatness));
}
