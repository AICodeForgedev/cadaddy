//! A print set one sheet at a time (Q6416, `cadaddy_engine::sheetnav`): the sheets come from the
//! job's records in the set's order; with one open, only its objects are drawn, picked, selected
//! and snapped to, while every command still reads the whole drawing.

use cadaddy_engine::doc::{EntityKind, entity_bounds};
use cadaddy_engine::geom::{Bounds2, Vec2};
use cadaddy_engine::snap::tracking::CursorQuery;
use cadaddy_engine::{Session, sample};
use serde_json::{Value, json};

use crate::helper::{hooks, run, scratch, session};

/// A session holding the made-up print set of `sheets` sheets of about `per` objects each.
fn job_print_set(sheets: usize, per: usize) -> Session {
    let mut s = session();
    s.open_drawing(sample::job_print_set(sheets, per), "Sample Set", None);
    s
}

/// The model-space objects whose bounds touch `rect`.
fn on_sheet(s: &Session, rect: &Bounds2) -> Vec<cadaddy_engine::doc::Handle> {
    let d = s.doc().unwrap();
    d.model.iter().filter(|e| entity_bounds(d, e, 0).intersects(rect)).map(|e| e.handle).collect()
}

/// A wall line lying wholly on `rect`, as its two ends.
fn a_line_on(s: &Session, rect: &Bounds2) -> (Vec2, Vec2) {
    let d = s.doc().unwrap();
    d.model
        .iter()
        .find_map(|e| match &e.kind {
            EntityKind::Line(l)
                if e.common.layer == "A-WALL" && rect.contains(l.a.xy()) && rect.contains(l.b.xy()) && l.a.xy().dist(l.b.xy()) > 30.0 =>
            {
                Some((l.a.xy(), l.b.xy()))
            }
            _ => None,
        })
        .unwrap()
}

#[test]
fn sheets_come_from_the_job_records_in_the_sets_order() {
    let mut s = session();
    let mut d = cadaddy_engine::doc::Drawing::new_imperial();
    // Imported pages, side by side; a sheet printing two levels' plans is one region per view.
    let r = |x: f64| json!([x, 0.0, x + 1000.0, 700.0]);
    d.agent = Some(
        json!({"guided_job": {
            "sheet_regions": [
                {"number": "Q-002", "rect": r(1100.0)},
                {"number": "Q-001", "rect": r(0.0)},
                {"number": "Q-003", "rect": r(2200.0), "view": {"title": "LEVEL 1", "rect": [2200, 0, 2700, 700]}},
                {"number": "Q-003", "rect": r(2200.0), "view": {"title": "LEVEL 2", "rect": [2700, 0, 3200, 700]}},
                {"number": "", "rect": r(3300.0)},
                {"number": "Q-008", "rect": [5000.0, 0.0, 5000.0, 700.0]},
            ],
            // The set's index: the order and the titles.
            "index": {"sheets": [{"number": "Q-001", "title": "First Floor"}, {"number": "Q-003/1", "title": "Upper Floors"}, {"number": "Q-002", "title": "Second Floor"}]},
        }})
        .to_string(),
    )
    .into();
    // A spec link with a place the job didn't record, and one it did.
    d.building_spec = Some(
        json!({"sheets": [{"number": "Q-009", "title": "Spec Only", "rect": [4400, 0, 5400, 700]}, {"number": "Q-001", "rect": [0, 0, 1000, 700]}]})
            .to_string(),
    )
    .into();
    s.open_drawing(d, "Records", None);
    let got: Vec<(String, String, f64)> = s.print_sheets().iter().map(|sh| (sh.number.clone(), sh.title.clone(), sh.rect.min.x)).collect();
    let want = [
        ("Q-001", "First Floor", 0.0),
        ("Q-003", "Upper Floors", 2200.0),
        ("Q-002", "Second Floor", 1100.0),
        // Not in the index: in page order (left to right) after it; the degenerate one is no sheet.
        ("", "", 3300.0),
        ("Q-009", "Spec Only", 4400.0),
    ];
    assert_eq!(got, want.map(|(n, t, x)| (n.to_string(), t.to_string(), x)), "{got:?}");
    let list = run(&mut s, "sheet.list", json!({}));
    assert_eq!(list["sheets"].as_array().unwrap().len(), 5, "{list}");
    assert_eq!(list["sheets"][1]["index"], 2);
    assert_eq!(list["active"], Value::Null, "nothing is open until the app or a command opens a sheet");
}

#[test]
fn an_ordinary_drawing_has_no_sheets_and_shows_itself_whole() {
    let mut s = session();
    run(&mut s, "line", json!({"points": [[0, 0], [10, 0]]}));
    run(&mut s, "line", json!({"points": [[5000, 0], [5010, 0]]}));
    assert!(s.print_sheets().is_empty());
    assert!(!s.open_first_sheet());
    assert!(s.sheet_scope().is_none());
    assert!(std::sync::Arc::ptr_eq(&s.view_doc().unwrap(), &s.state().unwrap().doc), "the screen shows the drawing itself");
    assert!(s.execute("sheet.open", &json!({"sheet": 1})).is_err());
    assert_eq!(run(&mut s, "sheet.next", json!({}))["sheets"], json!([]));
    run(&mut s, "selectall", json!({}));
    assert_eq!(s.selection().len(), 2);
    run(&mut s, "zoom", json!({"mode": "extents"}));
    let v = s.state().unwrap().view();
    assert!(v.height * 1.5 >= 5010.0 && v.center.near(Vec2::new(2505.0, 0.0), 1e-6), "zoom extents fits all of the drawing: {v:?}");
}

#[test]
fn a_print_set_opens_on_its_first_sheet_and_steps_through_them() {
    let mut s = job_print_set(4, 60);
    assert_eq!(s.print_sheets().len(), 4);
    assert!(s.open_first_sheet(), "a print set with no sheet open opens on its first");
    assert!(!s.open_first_sheet(), "once");
    let first = s.print_sheets()[0].rect;
    assert_eq!(s.active_sheet().map(|(i, sh)| (i, sh.number)), Some((0, "Q-001".to_string())));
    let v = s.state().unwrap().view();
    assert!(v.center.near(first.center(), 1e-6), "the view fits the sheet: {v:?}");
    assert!(v.height >= first.height() && v.height < first.height() * 1.6, "{v:?}");
    // Page Down / Page Up, stopping at the ends.
    assert_eq!(run(&mut s, "sheet.next", json!({}))["active"], 2);
    assert_eq!(run(&mut s, "sheet.previous", json!({}))["active"], 1);
    assert_eq!(run(&mut s, "sheet.previous", json!({}))["active"], 1);
    assert_eq!(run(&mut s, "sheet.open", json!({"step": "last"}))["active"], 4);
    assert_eq!(run(&mut s, "sheet.next", json!({}))["active"], 4);
    assert_eq!(run(&mut s, "sheet.open", json!({"sheet": "q-002"}))["active"], 2);
    let r = run(&mut s, "sheet.open", json!({"sheet": 3}));
    assert_eq!((r["active"].clone(), r["message"].clone()), (json!(3), json!("Sheet Q-003 Sample Sheet 3")));
    for bad in [json!({"sheet": 0}), json!({"sheet": 5}), json!({"sheet": "Q-404"}), json!({"step": "sideways"}), json!({})] {
        assert!(s.execute("sheet.open", &bad).is_err(), "{bad}");
    }
    // ZOOM Extents fits the open sheet: there is no view of the whole set.
    run(&mut s, "zoom", json!({"mode": "extents"}));
    let third = s.print_sheets()[2].rect;
    assert!(s.state().unwrap().view().center.near(third.center(), 1e-6));
    // A 3D view shows the building, not a sheet; opening a sheet comes back to plan.
    s.state_mut().unwrap().view_dir = cadaddy_engine::geom::Vec3::new(1.0, 1.0, 1.0);
    assert!(s.sheet_scope().is_none());
    run(&mut s, "sheet.open", json!({"sheet": 1}));
    assert_eq!(s.sheet_scope(), Some(first));
}

#[test]
fn an_open_sheet_is_all_that_is_drawn_picked_selected_and_snapped_to() {
    let mut s = job_print_set(3, 200);
    s.open_sheet(1).unwrap();
    let sheets = s.print_sheets();
    let (here, there) = (sheets[1].rect, sheets[2].rect);
    // Drawn: exactly the objects touching the sheet.
    let shown = s.view_doc().unwrap();
    let mut want = on_sheet(&s, &here);
    let mut got: Vec<_> = shown.model.iter().map(|e| e.handle).collect();
    want.sort();
    got.sort();
    assert_eq!(got, want);
    assert!(got.len() >= 200 && got.len() < s.doc().unwrap().model.len() / 2, "{} of {}", got.len(), s.doc().unwrap().model.len());
    let list = run(&mut s, "sheet.list", json!({}));
    assert_eq!((list["shown"].as_u64(), list["objects"].as_u64()), (Some(got.len() as u64), Some(s.doc().unwrap().model.len() as u64)), "{list}");
    // The same view while nothing changes; the drawing changing makes it again.
    assert!(std::sync::Arc::ptr_eq(&shown, &s.view_doc().unwrap()));
    // Picked: a line on the next sheet isn't, one on this sheet is.
    let (a, b) = a_line_on(&s, &there);
    s.idle_click((a + b) * 0.5, false).unwrap();
    assert!(s.selection().is_empty(), "nothing on another sheet is picked");
    assert!(s.pick_candidates((a + b) * 0.5).unwrap().is_empty());
    s.pending_window = None;
    let (c, e) = a_line_on(&s, &here);
    s.idle_click((c + e) * 0.5, false).unwrap();
    assert_eq!(s.selection().len(), 1, "an object on the open sheet is");
    s.set_selection(Vec::new());
    // Selected: a window over the whole set takes the open sheet's objects.
    s.idle_click(Vec2::new(-500.0, -500.0), false).unwrap();
    s.idle_click(Vec2::new(1e6, 1e5), false).unwrap();
    let mut sel = s.selection();
    sel.sort();
    assert_eq!(sel, want, "a window takes only the open sheet's objects");
    run(&mut s, "selectall", json!({}));
    assert_eq!(s.selection().len(), want.len(), "Select All: what is shown");
    s.set_selection(Vec::new());
    // Snapped to: an end on another sheet isn't, one on this sheet is.
    s.settings.osmode = cadaddy_engine::snap::mode::END;
    let q = CursorQuery { base: None, deferred: false, corner: false, aperture: 2.0, tol: 1.0, now: 0.0, shift: false };
    assert!(s.snap_cursor(a + Vec2::new(0.5, 0.5), &q).snap.is_none(), "no snap on another sheet");
    let hit = s.snap_cursor(c + Vec2::new(0.5, 0.5), &q).snap.expect("a snap on the open sheet");
    assert!(hit.point.near(c, 1e-9));
    // An edit on the sheet shows at once.
    run(&mut s, "line", json!({"points": [[c.x, c.y], [c.x + 50.0, c.y + 50.0]]}));
    assert_eq!(s.view_doc().unwrap().model.len(), want.len() + 1);
}

/// What the commands that read the drawing report of it.
fn readings(s: &mut Session) -> Vec<Value> {
    vec![
        run(s, "entities", json!({"limit": 1}))["count"].clone(),
        run(s, "takeoff", json!({})),
        run(s, "takeoff", json!({"mode": "area"})),
        run(s, "bim.tree", json!({}))["total"].clone(),
        run(s, "count", json!({"block": "SYM-OUTLET", "space": "model"})),
    ]
}

#[test]
fn commands_read_the_whole_drawing_with_a_sheet_open() {
    hooks();
    let mut s = job_print_set(3, 150);
    let total = s.doc().unwrap().model.len();
    let before = readings(&mut s);
    s.open_sheet(1).unwrap();
    assert!(s.sheet_scope().is_some());
    assert_eq!(readings(&mut s), before, "takeoff, counts and queries are the same with a sheet open");
    assert_eq!(s.doc().unwrap().model.len(), total);
    // Saved whole.
    let dir = scratch("sheetnav");
    let path = dir.join("set.cdy").display().to_string();
    run(&mut s, "saveas", json!({"path": path}));
    let mut o = Session::empty();
    run(&mut o, "open", json!({"path": path}));
    assert_eq!(o.doc().unwrap().model.len(), total, "the saved drawing holds every sheet");
    assert_eq!(o.print_sheets().len(), 3, "and its records");
    assert!(o.sheet_scope().is_none(), "the open sheet is the screen's, never saved");
    let _ = std::fs::remove_dir_all(dir);
}

/// The open sheet's view follows every edit: the same objects, in the drawing's order, as a view
/// made anew.
#[test]
fn the_sheet_view_follows_edits_and_undo() {
    let mut s = job_print_set(3, 150);
    s.open_sheet(1).unwrap();
    let sheets = s.print_sheets();
    let (here, there) = (sheets[1].rect, sheets[2].rect);
    let check = |s: &Session, what: &str| {
        let d = s.doc().unwrap();
        let want: Vec<_> = d.model.iter().filter(|e| entity_bounds(d, e, 0).intersects(&here)).map(|e| e.handle).collect();
        let got: Vec<_> = s.view_doc().unwrap().model.iter().map(|e| e.handle).collect();
        assert_eq!(got, want, "after {what}");
    };
    check(&s, "opening");
    let c = here.center();
    run(&mut s, "line", json!({"points": [[c.x, c.y], [c.x + 40.0, c.y]]}));
    check(&s, "a line on the sheet");
    let t = there.center();
    run(&mut s, "line", json!({"points": [[t.x, t.y], [t.x + 40.0, t.y]]}));
    check(&s, "a line on another sheet");
    let (a, b) = a_line_on(&s, &here);
    let h = s.doc().unwrap().model.iter().find(|e| matches!(&e.kind, EntityKind::Line(l) if l.a.xy() == a && l.b.xy() == b)).unwrap().handle;
    run(&mut s, "move", json!({"handles": [h.hex()], "from": [0, 0], "to": [5, 5]}));
    check(&s, "a move on the sheet");
    run(&mut s, "move", json!({"handles": [h.hex()], "from": [0, 0], "to": [there.min.x - here.min.x, 0]}));
    check(&s, "a move off the sheet");
    run(&mut s, "move", json!({"handles": [h.hex()], "from": [0, 0], "to": [here.min.x - there.min.x, 0]}));
    check(&s, "a move back onto the sheet");
    let (e, _) = a_line_on(&s, &here);
    let g = s.doc().unwrap().model.iter().find(|x| matches!(&x.kind, EntityKind::Line(l) if l.a.xy() == e)).unwrap().handle;
    run(&mut s, "erase", json!({"handles": [g.hex()]}));
    check(&s, "an erase");
    for i in 0..3 {
        run(&mut s, "u", json!({}));
        check(&s, &format!("undo {i}"));
    }
    run(&mut s, "redo", json!({}));
    check(&s, "redo");
}
