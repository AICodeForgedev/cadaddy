//! Temporary tracking (#441): the `TT` temporary tracking point and the `TK` tracking modifier,
//! typed at any point prompt and driven without the UI.

use cadcraft_doc::EntityKind;
use cadcraft_engine::Session;
use cadcraft_engine::geom::Vec2;
use cadcraft_engine::snap::tracking::{Acquired, CursorQuery, PathKind};
use serde_json::json;

fn v(x: f64, y: f64) -> Vec2 {
    Vec2::new(x, y)
}

/// The last line drawn as (start, end).
fn last_line(s: &Session) -> (Vec2, Vec2) {
    s.doc().unwrap().model.iter().filter_map(|e| if let EntityKind::Line(l) = &e.kind { Some((l.a.xy(), l.b.xy())) } else { None }).last().unwrap()
}

fn near(p: Vec2, x: f64, y: f64) -> bool {
    p.near(v(x, y), 1e-9)
}

/// Move the cursor to `raw` as the canvas does: snap it and remember where it ended up.
fn hover(s: &mut Session, raw: Vec2, base: Option<Vec2>) -> Vec2 {
    let q = CursorQuery { base, deferred: false, aperture: 0.5, tol: 0.1, now: 100.0, shift: false };
    let r = s.snap_cursor(raw, &q);
    s.cursor = r.point;
    r.point
}

#[test]
fn tt_acquires_a_point_that_tracks_until_the_next_point() {
    let mut s = Session::new();
    // Even with object snap tracking off, a temporary tracking point sends its paths.
    s.execute("otrack", &json!({ "on": false })).unwrap();
    s.cmdline("line tt").unwrap();
    assert!(s.prompt_text().ends_with("Specify temporary OTRACK point:"), "{}", s.prompt_text());
    s.cmdline("5,5").unwrap();
    assert!(s.prompt_text().ends_with("Specify first point:"), "{}", s.prompt_text());
    assert_eq!(s.tracking.acquired, vec![Acquired::Temporary { at: v(5.0, 5.0) }]);
    // The cursor slides along the horizontal path from 5,5 …
    let p = hover(&mut s, v(8.0, 5.04), None);
    assert!(near(p, 8.0, 5.0), "{p:?}");
    let t = s.tracking.last.clone().unwrap();
    assert_eq!((t.paths[0].kind, t.paths[0].name), (PathKind::Alignment, "Temporary track point"));
    // … and a distance typed there is measured from the tracking point, with no base point.
    s.cmdline("4").unwrap();
    assert!(near(s.last_point, 9.0, 5.0), "{:?}", s.last_point);
    assert!(s.tracking.acquired.is_empty(), "released once the command took a point");
    s.cmdline("@0,3").unwrap();
    s.cmdline("").unwrap();
    let (a, b) = last_line(&s);
    assert!(near(a, 9.0, 5.0) && near(b, 9.0, 8.0), "{a:?} {b:?}");
}

#[test]
fn tt_point_is_the_last_point_and_takes_snap_overrides() {
    let mut s = Session::new();
    s.script("LINE 0,0 10,0\n\n").unwrap();
    s.cmdline("line 0,5 _tt end 9.97,0.04").unwrap();
    assert_eq!(s.tracking.acquired, vec![Acquired::Temporary { at: v(10.0, 0.0) }]);
    // `@` is measured from the temporary point.
    s.cmdline("@0,2").unwrap();
    s.cmdline("").unwrap();
    let (a, b) = last_line(&s);
    assert!(near(a, 0.0, 5.0) && near(b, 10.0, 2.0), "{a:?} {b:?}");
}

#[test]
fn tk_runs_orthogonal_tracking_points_until_enter() {
    let mut s = Session::new();
    s.cmdline("line 0,0 tk").unwrap();
    assert!(s.prompt_text().ends_with("First tracking point:"), "{}", s.prompt_text());
    s.cmdline("10,0").unwrap();
    assert!(s.prompt_text().ends_with("Next point (Press ENTER to end tracking):"), "{}", s.prompt_text());
    assert_eq!(s.current_prompt().and_then(|p| p.base), Some(v(10.0, 0.0)));
    // Picked off-axis, the point goes straight up from the last tracking point.
    s.cmdline("12,3").unwrap();
    assert!(near(s.last_point, 10.0, 3.0), "{:?}", s.last_point);
    s.cmdline("@5,0").unwrap();
    // A distance follows the cursor's axis from the last tracking point.
    s.cursor = v(15.2, 9.0);
    s.cmdline("2").unwrap();
    assert!(near(s.last_point, 15.0, 5.0), "{:?}", s.last_point);
    // Enter ends tracking: the line runs to the last tracking point.
    s.cmdline("").unwrap();
    assert!(s.prompt_text().ends_with("Specify next point or [Undo]:"), "{}", s.prompt_text());
    let (a, b) = last_line(&s);
    assert!(near(a, 0.0, 0.0) && near(b, 15.0, 5.0), "{a:?} {b:?}");
}

#[test]
fn tk_takes_snaps_and_nests_under_from() {
    let mut s = Session::new();
    s.script("LINE 0,0 10,0\n\n").unwrap();
    // FROM, base point by tracking from the snapped endpoint, then the offset.
    s.cmdline("line from tracking endp 9.96,0.03 @0,4").unwrap();
    s.cmdline("").unwrap();
    assert!(s.prompt_text().ends_with("<Offset>:"), "{}", s.prompt_text());
    s.cmdline("@1,1").unwrap();
    s.cmdline("@1,0").unwrap();
    s.cmdline("").unwrap();
    let (a, b) = last_line(&s);
    assert!(near(a, 11.0, 5.0) && near(b, 12.0, 5.0), "{a:?} {b:?}");
}

#[test]
fn enter_at_the_first_tracking_point_and_hostile_input_are_harmless() {
    let mut s = Session::new();
    s.cmdline("line tk").unwrap();
    s.cmdline("").unwrap();
    assert!(s.prompt_text().ends_with("Specify first point:"), "{}", s.prompt_text());
    for _ in 0..40 {
        s.cmdline("tk").unwrap();
        s.cmdline("tt").unwrap();
    }
    s.cmdline("nan,1").unwrap();
    s.cmdline("1e308").unwrap();
    s.cancel();
    assert!(s.running.is_none());
    assert!(s.tracking.acquired.len() <= cadcraft_engine::snap::tracking::MAX_ACQUIRED);
}
