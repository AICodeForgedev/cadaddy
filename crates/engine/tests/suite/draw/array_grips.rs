//! Grip editing of associative arrays (count, spacing, base, centre, level grips; one undo step
//! each) and the Key point base points of ARRAYRECT, ARRAYPOLAR and ARRAYEDIT.

use cadaddy_engine::cmd::array::grips::GripId;
use cadaddy_engine::doc::{ArrayKind, ArrayParams, Drawing, Entity, EntityKind, Handle};
use cadaddy_engine::geom::{Mat3, Vec2};
use cadaddy_engine::{Input, Session};
use serde_json::{Value, json};

fn circle(s: &mut Session, x: f64, y: f64) -> Value {
    s.execute("circle", &json!({"center": [x, y], "radius": 1})).unwrap()["handle"].clone()
}

fn handle(v: &Value) -> Handle {
    Handle::parse_hex(v.as_str().unwrap()).unwrap()
}

fn entity(s: &Session, h: Handle) -> Entity {
    (**s.doc().unwrap().entity(h).unwrap()).clone()
}

fn params(s: &Session, h: Handle) -> ArrayParams {
    s.doc().unwrap().array_params(&entity(s, h)).expect("an associative array")
}

fn rect(p: &ArrayParams) -> (u32, u32, f64, f64) {
    match &p.kind {
        ArrayKind::Rect(r) => (r.cols, r.rows, r.col_spacing, r.row_spacing),
        k => panic!("not rectangular: {k:?}"),
    }
}

fn polar(p: &ArrayParams) -> cadaddy_engine::doc::PolarArray {
    match &p.kind {
        ArrayKind::Polar(k) => k.clone(),
        k => panic!("not polar: {k:?}"),
    }
}

/// World centres of the circles array `e` draws.
fn circles(d: &Drawing, e: &Entity) -> Vec<Vec2> {
    fn walk(d: &Drawing, e: &Entity, m: Mat3, depth: usize, out: &mut Vec<Vec2>) {
        match &e.kind {
            EntityKind::Circle(c) => out.push(m.apply(c.center.xy())),
            EntityKind::Insert(i) if depth < 8 => {
                let b = d.block(&i.block).unwrap();
                let im = m.then_before(i.transform(b.base.xy()));
                for be in b.entities.iter() {
                    walk(d, be, im, depth + 1, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(d, e, Mat3::IDENTITY, 0, &mut out);
    out
}

fn has(pts: &[Vec2], x: f64, y: f64) -> bool {
    pts.iter().any(|p| p.near(Vec2::new(x, y), 1e-6))
}

/// A 3 × 3 rectangular array of a circle at the origin, 5 apart along X and 4 along Y.
fn rect3() -> (Session, Handle) {
    let mut s = Session::new();
    let c = circle(&mut s, 0.0, 0.0);
    let r = s.execute("arrayrect", &json!({"handles": [c], "cols": 3, "rows": 3, "colSpacing": 5, "rowSpacing": 4, "base": [0, 0]})).unwrap();
    let h = handle(&r["handle"]);
    (s, h)
}

fn grips(s: &mut Session, h: Handle) -> Vec<Value> {
    s.execute("array.grips", &json!({"handle": h.hex()})).unwrap().as_array().unwrap().clone()
}

fn grip_at(gs: &[Value], name: &str) -> Option<Vec2> {
    gs.iter().find(|g| g["grip"] == name).map(|g| Vec2::new(g["at"][0].as_f64().unwrap(), g["at"][1].as_f64().unwrap()))
}

fn grip(s: &mut Session, h: Handle, p: Value) -> cadaddy_engine::Result<Value> {
    let mut p = p;
    p["handle"] = json!(h.hex());
    s.execute("array.grip", &p)
}

#[test]
fn a_rectangular_array_shows_base_count_and_spacing_grips() {
    let (mut s, h) = rect3();
    let gs = grips(&mut s, h);
    let at = |n: &str| grip_at(&gs, n).unwrap_or_else(|| panic!("no {n} grip: {gs:?}"));
    assert!(at("base").near(Vec2::ZERO, 1e-9));
    assert!(at("columnSpacing").near(Vec2::new(5.0, 0.0), 1e-9));
    assert!(at("columnCount").near(Vec2::new(10.0, 0.0), 1e-9));
    assert!(at("rowSpacing").near(Vec2::new(0.0, 4.0), 1e-9));
    assert!(at("rowCount").near(Vec2::new(0.0, 8.0), 1e-9));
    assert!(at("rowColumnCount").near(Vec2::new(10.0, 8.0), 1e-9));
    assert!(grip_at(&gs, "levelCount").is_none(), "one level: no level grips");
    let cc = gs.iter().find(|g| g["grip"] == "columnCount").unwrap();
    assert_eq!(cc["look"], "arrow");
    assert_eq!(cc["option"][0], "Total column spacing");
    // Not an array: no grips.
    let c = circle(&mut s, 50.0, 50.0);
    assert_eq!(s.execute("array.grips", &json!({"handle": c})).unwrap(), json!([]));
}

#[test]
fn count_grips_keep_the_spacing_and_total_changes_it() {
    let (mut s, h) = rect3();
    grip(&mut s, h, json!({"grip": "columnCount", "to": [20.4, 3]})).unwrap();
    assert_eq!(rect(&params(&s, h)), (5, 3, 5.0, 4.0), "20 along at 5 apart: 5 columns");
    grip(&mut s, h, json!({"grip": "columnCount", "to": [40, 0], "option": "total"})).unwrap();
    assert_eq!(rect(&params(&s, h)), (5, 3, 10.0, 4.0), "the last column at 40: 10 apart");
    grip(&mut s, h, json!({"grip": "rowCount", "to": [0, 11.9]})).unwrap();
    assert_eq!(rect(&params(&s, h)).1, 4);
    grip(&mut s, h, json!({"grip": "rowColumnCount", "to": [20, 4]})).unwrap();
    assert_eq!(rect(&params(&s, h)), (3, 2, 10.0, 4.0));
    grip(&mut s, h, json!({"grip": "rowColumnCount", "to": [30, 12], "option": "total"})).unwrap();
    assert_eq!(rect(&params(&s, h)), (3, 2, 15.0, 12.0));
    // Dragged back past the base: one column.
    grip(&mut s, h, json!({"grip": "columnCount", "to": [-50, 0]})).unwrap();
    assert_eq!(rect(&params(&s, h)).0, 1);
    // With one column the count grip waits one spacing out.
    let gs = grips(&mut s, h);
    assert!(grip_at(&gs, "columnCount").unwrap().near(Vec2::new(15.0, 0.0), 1e-9));
    assert!(grip_at(&gs, "columnSpacing").is_none());
}

#[test]
fn spacing_grips_change_the_distance_and_the_items_follow() {
    let (mut s, h) = rect3();
    grip(&mut s, h, json!({"grip": "columnSpacing", "to": [7, 1]})).unwrap();
    grip(&mut s, h, json!({"grip": "rowSpacing", "to": [3, -6]})).unwrap();
    assert_eq!(rect(&params(&s, h)), (3, 3, 7.0, -6.0));
    let pts = circles(s.doc().unwrap(), &entity(&s, h));
    assert_eq!(pts.len(), 9);
    assert!(has(&pts, 14.0, -12.0), "{pts:?}");
    assert!(grip(&mut s, h, json!({"grip": "columnSpacing", "to": [0, 9]})).is_err(), "a zero spacing");
}

#[test]
fn the_base_grip_moves_the_array() {
    let (mut s, h) = rect3();
    grip(&mut s, h, json!({"grip": "base", "to": [100, 50]})).unwrap();
    let pts = circles(s.doc().unwrap(), &entity(&s, h));
    assert!(has(&pts, 100.0, 50.0) && has(&pts, 110.0, 58.0), "{pts:?}");
    assert_eq!(rect(&params(&s, h)), (3, 3, 5.0, 4.0));
    assert!(grip_at(&grips(&mut s, h), "columnCount").unwrap().near(Vec2::new(110.0, 50.0), 1e-9));
}

#[test]
fn a_grip_drag_is_one_undo_step_and_takes_a_typed_value() {
    let (mut s, h) = rect3();
    s.start_array_grip(h, GripId::ColumnCount).unwrap();
    assert!(s.current_prompt().unwrap().message.contains("COLUMN COUNT"));
    // The preview shows the array as it would be.
    assert!(!s.preview(Vec2::new(30.0, 0.0)).is_empty());
    s.input(Input::Point(Vec2::new(30.0, 0.0))).unwrap();
    assert!(s.running.is_none());
    assert_eq!(rect(&params(&s, h)).0, 7);
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(rect(&params(&s, h)).0, 3, "one undo step");
    // A number typed at the grip.
    s.start_array_grip(h, GripId::RowCount).unwrap();
    s.cmdline("6").unwrap();
    assert_eq!(rect(&params(&s, h)).1, 6);
    // The option keyword switches the grip to Total, and back.
    s.start_array_grip(h, GripId::ColumnCount).unwrap();
    s.cmdline("T").unwrap();
    assert!(s.current_prompt().unwrap().message.contains("TOTAL COLUMN SPACING"));
    s.cmdline("20").unwrap();
    assert_eq!(rect(&params(&s, h)), (3, 6, 10.0, 4.0));
    // Esc leaves the array as it was.
    s.start_array_grip(h, GripId::ColumnSpacing).unwrap();
    s.input(Input::Cancel).unwrap();
    assert_eq!(rect(&params(&s, h)), (3, 6, 10.0, 4.0));
}

#[test]
fn level_grips_take_typed_values() {
    let mut s = Session::new();
    let c = circle(&mut s, 0.0, 0.0);
    let r = s
        .execute(
            "arrayrect",
            &json!({"handles": [c], "cols": 2, "rows": 1, "colSpacing": 5, "rowSpacing": 4, "levels": 3, "levelSpacing": 2, "base": [0, 0]}),
        )
        .unwrap();
    let h = handle(&r["handle"]);
    let gs = grips(&mut s, h);
    let lc = gs.iter().find(|g| g["grip"] == "levelCount").unwrap();
    assert_eq!((lc["plan"].clone(), lc["z"].clone()), (json!(false), json!(4.0)));
    grip(&mut s, h, json!({"grip": "levelCount", "value": 5})).unwrap();
    grip(&mut s, h, json!({"grip": "levelSpacing", "value": 3})).unwrap();
    let p = params(&s, h);
    assert_eq!((p.levels, p.level_spacing), (5, 3.0));
    grip(&mut s, h, json!({"grip": "levelCount", "value": 8, "option": "total"})).unwrap();
    assert_eq!(params(&s, h).level_spacing, 2.0);
    assert!(grip(&mut s, h, json!({"grip": "levelCount", "to": [1, 1]})).is_err(), "a plan view can't drag along Z");
    assert!(grip(&mut s, h, json!({"grip": "levelCount", "value": 2.5})).is_err());
}

/// A polar array of a circle at (10, 0) around the origin.
fn polar_of(count: u32, angle: f64, rows: u32) -> (Session, Handle) {
    let mut s = Session::new();
    let c = circle(&mut s, 10.0, 0.0);
    let r = s
        .execute(
            "arraypolar",
            &json!({"handles": [c], "center": [0, 0], "count": count, "angle": angle, "rows": rows, "rowSpacing": 2, "base": [10, 0]}),
        )
        .unwrap();
    let h = handle(&r["handle"]);
    (s, h)
}

#[test]
fn a_polar_array_shows_base_centre_item_and_row_grips() {
    let (mut s, h) = polar_of(4, 360.0, 2);
    let gs = grips(&mut s, h);
    let at = |n: &str| grip_at(&gs, n).unwrap_or_else(|| panic!("no {n} grip: {gs:?}"));
    assert!(at("base").near(Vec2::new(10.0, 0.0), 1e-9));
    assert!(at("center").near(Vec2::ZERO, 1e-9));
    assert!(at("itemSpacing").near(Vec2::new(0.0, 10.0), 1e-9));
    assert!(at("itemCount").near(Vec2::new(0.0, -10.0), 1e-9));
    assert!(at("rowSpacing").near(Vec2::new(12.0, 0.0), 1e-9));
    assert!(at("rowCount").near(Vec2::new(12.0, 0.0), 1e-9));
    let base = gs.iter().find(|g| g["grip"] == "base").unwrap();
    assert_eq!(base["option"][0], "Stretch radius");
}

#[test]
fn the_polar_item_count_grip_adds_items() {
    // A full circle: the last item one step short of it. Dragged to 315°: eight items.
    let (mut s, h) = polar_of(4, 360.0, 1);
    let to = Vec2::from_angle(315f64.to_radians()) * 10.0;
    grip(&mut s, h, json!({"grip": "itemCount", "to": [to.x, to.y]})).unwrap();
    let k = polar(&params(&s, h));
    assert_eq!((k.items, k.fill), (8, 360.0));
    // A half circle, 90° apart: dragged to 270°, four items filling 270°.
    let (mut s, h) = polar_of(3, 180.0, 1);
    grip(&mut s, h, json!({"grip": "itemCount", "to": [0, -10]})).unwrap();
    let k = polar(&params(&s, h));
    assert_eq!(k.items, 4);
    assert!((k.fill - 270.0).abs() < 1e-9);
    // Fill angle: the count stays, the last item goes to 120°.
    let (mut s, h) = polar_of(3, 180.0, 1);
    let to = Vec2::from_angle(120f64.to_radians()) * 10.0;
    grip(&mut s, h, json!({"grip": "itemCount", "to": [to.x, to.y], "option": "total"})).unwrap();
    let k = polar(&params(&s, h));
    assert_eq!(k.items, 3);
    assert!((k.fill - 120.0).abs() < 1e-9);
    // Never past a full circle.
    grip(&mut s, h, json!({"grip": "itemCount", "value": 400})).unwrap();
    assert!(polar(&params(&s, h)).fill.abs() <= 360.0);
}

#[test]
fn the_polar_item_spacing_grip_sets_the_angle_between_items() {
    let (mut s, h) = polar_of(3, 180.0, 1);
    let to = Vec2::from_angle(60f64.to_radians()) * 7.0;
    grip(&mut s, h, json!({"grip": "itemSpacing", "to": [to.x, to.y]})).unwrap();
    let k = polar(&params(&s, h));
    assert!((k.fill - 120.0).abs() < 1e-9, "{}", k.fill);
    let pts = circles(s.doc().unwrap(), &entity(&s, h));
    let p = Vec2::from_angle(120f64.to_radians()) * 10.0;
    assert!(has(&pts, p.x, p.y), "{pts:?}");
    grip(&mut s, h, json!({"grip": "itemSpacing", "value": 45})).unwrap();
    assert!((polar(&params(&s, h)).fill - 90.0).abs() < 1e-9);
}

#[test]
fn the_polar_centre_and_base_grips() {
    // The centre grip moves the centre; the base item stays.
    let (mut s, h) = polar_of(4, 360.0, 1);
    grip(&mut s, h, json!({"grip": "center", "to": [10, 5]})).unwrap();
    let pts = circles(s.doc().unwrap(), &entity(&s, h));
    assert!(has(&pts, 10.0, 0.0) && has(&pts, 10.0, 10.0), "{pts:?}");
    // The base grip moves the array.
    let (mut s, h) = polar_of(4, 360.0, 1);
    grip(&mut s, h, json!({"grip": "base", "to": [15, 5]})).unwrap();
    let pts = circles(s.doc().unwrap(), &entity(&s, h));
    assert!(has(&pts, 15.0, 5.0) && has(&pts, 5.0, 15.0), "{pts:?}");
    // Stretch radius: the items move out from the same centre.
    let (mut s, h) = polar_of(4, 360.0, 1);
    grip(&mut s, h, json!({"grip": "base", "to": [0, 20], "option": "stretchRadius"})).unwrap();
    let pts = circles(s.doc().unwrap(), &entity(&s, h));
    assert!(has(&pts, 20.0, 0.0) && has(&pts, 0.0, 20.0) && has(&pts, -20.0, 0.0), "{pts:?}");
    grip(&mut s, h, json!({"grip": "base", "value": 5, "option": "stretchRadius"})).unwrap();
    let pts = circles(s.doc().unwrap(), &entity(&s, h));
    assert!(has(&pts, 5.0, 0.0) && has(&pts, 0.0, -5.0), "{pts:?}");
    assert!(grip(&mut s, h, json!({"grip": "center", "option": "total", "to": [1, 1]})).is_err(), "no such option");
}

#[test]
fn polar_row_grips() {
    let (mut s, h) = polar_of(4, 360.0, 2);
    grip(&mut s, h, json!({"grip": "rowCount", "to": [16, 1]})).unwrap();
    assert_eq!(polar(&params(&s, h)).rows, 4, "6 out at 2 apart: 4 rows");
    grip(&mut s, h, json!({"grip": "rowSpacing", "to": [13, 0]})).unwrap();
    assert_eq!(polar(&params(&s, h)).row_spacing, 3.0);
    grip(&mut s, h, json!({"grip": "rowCount", "to": [22, 0], "option": "total"})).unwrap();
    assert_eq!(polar(&params(&s, h)).row_spacing, 4.0);
}

#[test]
fn hostile_grip_edits_are_refused_and_never_panic() {
    let (mut s, h) = rect3();
    for p in [
        json!({"grip": "columnCount"}),
        json!({"grip": "columnCount", "to": ["x", 1]}),
        json!({"grip": "nosuch", "to": [1, 1]}),
        json!({"grip": "center", "to": [1, 1]}),
        json!({"grip": "columnCount", "value": 1e300}),
        json!({"grip": "columnCount", "value": -3}),
        json!({"grip": "columnCount", "to": [1e300, 0]}),
        json!({"grip": "rowColumnCount", "to": [1e12, 1e12]}),
        json!({"grip": "columnSpacing", "option": "sideways", "to": [1, 1]}),
    ] {
        assert!(grip(&mut s, h, p.clone()).is_err(), "{p}");
    }
    assert_eq!(rect(&params(&s, h)), (3, 3, 5.0, 4.0), "nothing changed");
    let c = circle(&mut s, 50.0, 50.0);
    assert!(s.execute("array.grip", &json!({"handle": c, "grip": "base", "to": [1, 1]})).is_err());
    assert!(s.start_array_grip(handle(&c), GripId::Base).is_err());
    assert!(s.execute("array.grips", &json!({})).is_err());
}

#[test]
fn arrayrect_grips_edit_the_array_while_it_is_made() {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [4, 0]]})).unwrap();
    // 4 columns 6 apart (1.5 × the width) from the centre (2, 0): the column count grip at (20, 0).
    for t in ["arrayrect", "l", ""] {
        s.cmdline(t).unwrap();
    }
    s.input(Input::Point(Vec2::new(20.0, 0.0))).unwrap();
    assert!(s.current_prompt().unwrap().message.contains("COLUMN COUNT"), "{:?}", s.current_prompt());
    s.input(Input::Point(Vec2::new(32.0, 0.0))).unwrap();
    assert!(s.current_prompt().unwrap().message.contains("Select grip to edit array"));
    s.cmdline("x").unwrap();
    let h = s.doc().unwrap().model.last().unwrap().handle;
    assert_eq!(rect(&params(&s, h)).0, 6);
    // A point that is no grip is refused, and the command goes on.
    for t in ["arrayrect", "l", ""] {
        s.cmdline(t).unwrap();
    }
    let _ = s.input(Input::Point(Vec2::new(500.0, 500.0)));
    assert!(s.running.is_some());
    assert!(s.current_prompt().unwrap().message.contains("Select grip to edit array"));
}

#[test]
fn arraypolar_grips_edit_the_array_while_it_is_made() {
    let mut s = Session::new();
    circle(&mut s, 10.0, 0.0);
    for t in ["arraypolar", "l", "", "0,0", "i", "4"] {
        s.cmdline(t).unwrap();
    }
    // Four items around the origin: the last one (the item count grip) at (0, -10).
    s.input(Input::Point(Vec2::new(0.0, -10.0))).unwrap();
    assert!(s.current_prompt().unwrap().message.contains("ITEM COUNT"), "{:?}", s.current_prompt());
    let to = Vec2::from_angle(315f64.to_radians()) * 10.0;
    s.input(Input::Point(to)).unwrap();
    s.cmdline("x").unwrap();
    let h = s.doc().unwrap().model.last().unwrap().handle;
    assert_eq!(polar(&params(&s, h)).items, 8);
}

#[test]
fn key_points_set_the_base_point() {
    // ARRAYRECT: Base point, Key point, then a pick near the line's end takes the end.
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[0, 0], [4, 0]]})).unwrap();
    for t in ["arrayrect", "l", "", "b", "k"] {
        s.cmdline(t).unwrap();
    }
    assert_eq!(s.current_prompt().unwrap().message, "Specify source objects key point");
    s.input(Input::Point(Vec2::new(3.8, 0.3))).unwrap();
    s.cmdline("x").unwrap();
    let h = s.doc().unwrap().model.last().unwrap().handle;
    let EntityKind::Insert(ins) = &entity(&s, h).kind else { panic!("not an array") };
    assert!(ins.insert.xy().near(Vec2::new(4.0, 0.0), 1e-9), "{:?}", ins.insert);

    // ARRAYEDIT: Base point, Key point on the source objects: the midpoint.
    s.cmdline("arrayedit").unwrap();
    s.input(Input::Pick(vec![h])).unwrap();
    for t in ["b", "k"] {
        s.cmdline(t).unwrap();
    }
    s.input(Input::Point(Vec2::new(2.2, 0.4))).unwrap();
    s.cmdline("x").unwrap();
    let EntityKind::Insert(ins) = &entity(&s, h).kind else { panic!("not an array") };
    assert!(ins.insert.xy().near(Vec2::new(2.0, 0.0), 1e-9), "{:?}", ins.insert);
    // The items didn't move.
    assert_eq!(rect(&params(&s, h)).0, 4);

    // ARRAYPOLAR: Base point from the centre prompt, Key point at the circle's centre.
    let mut s = Session::new();
    circle(&mut s, 10.0, 0.0);
    for t in ["arraypolar", "l", "", "b", "k", "10.1,0.2", "0,0", "x"] {
        s.cmdline(t).unwrap();
    }
    let h = s.doc().unwrap().model.last().unwrap().handle;
    let EntityKind::Insert(ins) = &entity(&s, h).kind else { panic!("not an array") };
    assert!(ins.insert.xy().near(Vec2::new(10.0, 0.0), 1e-9), "{:?}", ins.insert);
}
