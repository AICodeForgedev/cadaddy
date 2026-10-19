//! Associative path arrays and array item editing: ARRAYPATH makes one array that follows its
//! path curve when the curve is edited, ARRAYEDIT changes the method, spacing, count and
//! alignment and its grips work on it; items can be deleted from any array (kept out through a
//! save and load, back with RESet) and replaced; levels stand at their elevation in a 3D view.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{ArrayKind, Drawing, Entity, EntityKind, Handle};
use cadaddy_engine::geom::{Mat3, Vec2};
use serde_json::{Value, json};

fn h(v: &Value) -> String {
    v.as_str().unwrap().to_string()
}

fn handle_of(v: &str) -> Handle {
    Handle(u64::from_str_radix(v, 16).unwrap())
}

/// Circle centres in model space, inside arrays too.
fn circle_centers(d: &Drawing) -> Vec<Vec2> {
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
    for e in d.model.iter() {
        walk(d, e, Mat3::IDENTITY, 0, &mut out);
    }
    out
}

fn has(pts: &[Vec2], x: f64, y: f64) -> bool {
    pts.iter().any(|p| p.near(Vec2::new(x, y), 1e-6))
}

/// A circle at the origin arrayed along a 20-unit line along X: (array, path).
fn line_array(s: &mut Session, params: Value) -> (String, String) {
    let c = s.execute("circle", &json!({"center": [0, 0], "radius": 0.5})).unwrap();
    let path = s.execute("line", &json!({"points": [[0, 0], [20, 0]]})).unwrap();
    let path = h(&path["handles"][0]);
    let mut p = json!({"handles": [c["handle"]], "path": path});
    if let (Some(o), Some(extra)) = (p.as_object_mut(), params.as_object()) {
        o.extend(extra.clone());
    }
    let r = s.execute("arraypath", &p).unwrap();
    assert_eq!(r["associative"], true, "{r}");
    (h(&r["handle"]), path)
}

#[test]
fn arraypath_is_one_associative_array_that_follows_its_path() {
    let mut s = Session::new();
    let (a, path) = line_array(&mut s, json!({"count": 5}));
    let d = s.doc().unwrap();
    let e = d.entity(handle_of(&a)).unwrap();
    let p = d.array_params(e).unwrap();
    assert!(matches!(&p.kind, ArrayKind::Path(k) if k.divide && k.items == 5 && k.curve == Some(handle_of(&path))));
    let c = circle_centers(d);
    assert_eq!(c.len(), 5);
    assert!((0..5).all(|i| has(&c, 5.0 * f64::from(i), 0.0)), "{c:?}");

    // The path turns a quarter about its start: the items go with it.
    s.execute("rotate", &json!({"handles": [path], "base": [0, 0], "angle": 90})).unwrap();
    let c = circle_centers(s.doc().unwrap());
    assert_eq!(c.len(), 5);
    assert!((0..5).all(|i| has(&c, 0.0, 5.0 * f64::from(i))), "{c:?}");
    // Undo puts the path and the items back together.
    s.execute("undo", &json!({})).unwrap();
    let c = circle_centers(s.doc().unwrap());
    assert!(has(&c, 20.0, 0.0) && !has(&c, 0.0, 20.0), "{c:?}");

    // Moving the array and its path together keeps them together.
    s.execute("move", &json!({"handles": [a, path], "delta": [3, 4]})).unwrap();
    let c = circle_centers(s.doc().unwrap());
    assert!((0..5).all(|i| has(&c, 3.0 + 5.0 * f64::from(i), 4.0)), "{c:?}");

    // Erasing the path leaves the array where it is.
    s.execute("erase", &json!({"handles": [path]})).unwrap();
    assert_eq!(circle_centers(s.doc().unwrap()).len(), 5);
}

#[test]
fn a_longer_path_holds_more_measured_items_and_divide_spreads_them() {
    let mut s = Session::new();
    let (a, path) = line_array(&mut s, json!({"spacing": 4}));
    // Measure: every 4 units, 6 items on 20.
    assert_eq!(circle_centers(s.doc().unwrap()).len(), 6);
    // A shorter path (scaled to half) holds 3.
    s.execute("scale", &json!({"handles": [path], "base": [0, 0], "factor": 0.5})).unwrap();
    let c = circle_centers(s.doc().unwrap());
    assert_eq!(c.len(), 3, "{c:?}");
    assert!(has(&c, 8.0, 0.0));
    // Divide: the 6 items spread over the 10 units.
    s.execute("arrayedit", &json!({"handle": a, "method": "divide", "items": 6})).unwrap();
    let c = circle_centers(s.doc().unwrap());
    assert_eq!(c.len(), 6);
    assert!(has(&c, 2.0, 0.0) && has(&c, 10.0, 0.0), "{c:?}");
    // Measure again with a spacing; Align off keeps the items' orientation.
    s.execute("arrayedit", &json!({"handle": a, "spacing": 2.5, "align": false})).unwrap();
    let d = s.doc().unwrap();
    let p = d.array_params(d.entity(handle_of(&a)).unwrap()).unwrap();
    assert!(matches!(&p.kind, ArrayKind::Path(k) if !k.divide && k.spacing == 2.5 && !k.align));
    assert_eq!(circle_centers(d).len(), 5);
    assert!(s.execute("arrayedit", &json!({"handle": a, "spacing": -1})).is_err());
    assert!(s.execute("arrayedit", &json!({"handle": a, "method": "sideways"})).is_err());
}

#[test]
fn path_array_items_turn_along_an_arc() {
    let mut s = Session::new();
    s.execute("line", &json!({"points": [[9, 0], [11, 0]]})).unwrap();
    let tick = h(&s.execute("line", &json!({"points": [[9, 0], [11, 0]]})).unwrap()["handles"][0]);
    let arc = s.execute("arc", &json!({"center": [0, 0], "radius": 10, "start": 0, "end": 180})).unwrap();
    let arc = h(&arc["handle"]);
    let r = s.execute("arraypath", &json!({"handles": [tick], "path": arc, "count": 3})).unwrap();
    let a = h(&r["handle"]);
    // The middle item is at the top, turned a quarter: a vertical tick at (0, 9..11).
    let d = s.doc().unwrap();
    let ins = d.entity(handle_of(&a)).unwrap();
    let EntityKind::Insert(top) = &ins.kind else { panic!() };
    let blk = d.block(&top.block).unwrap();
    let m = top.transform(blk.base.xy());
    let mut ends = Vec::new();
    for item in blk.entities.iter() {
        let EntityKind::Insert(it) = &item.kind else { continue };
        let sb = d.block(&it.block).unwrap();
        let im = m.then_before(it.transform(sb.base.xy()));
        for e in sb.entities.iter() {
            if let EntityKind::Line(l) = &e.kind {
                ends.push((im.apply(l.a.xy()), im.apply(l.b.xy())));
            }
        }
    }
    assert_eq!(ends.len(), 3);
    assert!(ends.iter().any(|(p, q)| p.near(Vec2::new(0.0, 9.0), 1e-6) && q.near(Vec2::new(0.0, 11.0), 1e-6)), "{ends:?}");
}

#[test]
fn path_array_grips_set_spacing_and_count_along_the_path() {
    let mut s = Session::new();
    let (a, _) = line_array(&mut s, json!({"count": 5}));
    let grips = s.execute("array.grips", &json!({"handle": a})).unwrap();
    let names: Vec<&str> = grips.as_array().unwrap().iter().filter_map(|g| g["grip"].as_str()).collect();
    assert!(names.contains(&"itemSpacing") && names.contains(&"itemCount") && names.contains(&"base"), "{names:?}");
    // Drag the spacing grip to 3 units along: items every 3 (Measure), still 5.
    s.execute("array.grip", &json!({"handle": a, "grip": "itemSpacing", "to": [3, 0.4]})).unwrap();
    let c = circle_centers(s.doc().unwrap());
    assert!(c.len() == 5 && has(&c, 12.0, 0.0), "{c:?}");
    // Type 7 at the count grip: 7 items, as long as they fit (18 of 20).
    s.execute("array.grip", &json!({"handle": a, "grip": "itemCount", "value": 7})).unwrap();
    let c = circle_centers(s.doc().unwrap());
    assert!(c.len() == 7 && has(&c, 18.0, 0.0), "{c:?}");
}

#[test]
fn deleted_items_stay_deleted_through_save_and_load_and_reset_brings_them_back() {
    let mut s = Session::new();
    s.execute("circle", &json!({"center": [0, 0], "radius": 0.5})).unwrap();
    let r = s.execute("arrayrect", &json!({"rows": 2, "cols": 3, "rowSpacing": 10, "colSpacing": 10, "handles": [s.doc().unwrap().model.iter().next().unwrap().handle.hex()]})).unwrap();
    let a = h(&r["handle"]);
    s.execute("arrayedit", &json!({"handle": a, "remove": [[1, 0, 0], [2, 1, 0]]})).unwrap();
    let c = circle_centers(s.doc().unwrap());
    assert_eq!(c.len(), 4);
    assert!(!has(&c, 10.0, 0.0) && !has(&c, 20.0, 10.0) && has(&c, 20.0, 0.0));

    let bytes = cadaddy_io::write(s.doc().unwrap(), "items.dxf").unwrap();
    let back = cadaddy_io::read(&bytes, "items.dxf").unwrap();
    let arr = back.model.iter().find(|e| back.array_params(e).is_some()).expect("still an array after a reload");
    assert_eq!(back.array_params(arr).unwrap().removed, vec![[1, 0, 0], [2, 1, 0]]);
    assert_eq!(circle_centers(&back).len(), 4);

    // Grid edits keep the deleted items out; RESet brings them back.
    s.execute("arrayedit", &json!({"handle": a, "rowSpacing": 12})).unwrap();
    assert_eq!(circle_centers(s.doc().unwrap()).len(), 4);
    s.execute("arrayedit", &json!({"handle": a, "reset": true})).unwrap();
    assert_eq!(circle_centers(s.doc().unwrap()).len(), 6);

    // Hostile: no such item, not a list, the last item.
    assert!(s.execute("arrayedit", &json!({"handle": a, "remove": [[9, 9, 9]]})).is_err());
    assert!(s.execute("arrayedit", &json!({"handle": a, "remove": "all"})).is_err());
    let all: Vec<Value> = (0..3).flat_map(|i| (0..2).map(move |j| json!([i, j, 0]))).collect();
    assert!(s.execute("arrayedit", &json!({"handle": a, "remove": all})).is_err());
    assert_eq!(circle_centers(s.doc().unwrap()).len(), 6, "a refused edit changes nothing");
}

#[test]
fn arrayedit_deletes_and_replaces_path_items_at_the_command_line() {
    let mut s = Session::new();
    let (a, _) = line_array(&mut s, json!({"count": 5}));
    s.set_selection(vec![handle_of(&a)]);
    s.cmdline("arrayedit").unwrap();
    assert!(
        s.prompt_text().contains("Method") && s.prompt_text().contains("ALign items") && s.prompt_text().contains("DElete items"),
        "{}",
        s.prompt_text()
    );
    for t in ["de", "10,0", "15,0", ""] {
        s.cmdline(t).unwrap();
    }
    let c = circle_centers(s.doc().unwrap());
    assert_eq!(c.len(), 3, "{c:?}");
    assert!(!has(&c, 10.0, 0.0) && !has(&c, 15.0, 0.0));
    // Method ▸ Measure, items every 2.
    for t in ["m", "measure", "2"] {
        s.cmdline(t).unwrap();
    }
    for t in ["res", "x"] {
        s.cmdline(t).unwrap();
    }
    assert!(s.current_prompt().is_none(), "{}", s.prompt_text());
    let c = circle_centers(s.doc().unwrap());
    assert_eq!(c.len(), 5, "{c:?}");
    assert!(has(&c, 8.0, 0.0), "{c:?}");

    // REPlace one item of the path array with another circle.
    let other = s.execute("circle", &json!({"center": [50, 50], "radius": 2})).unwrap();
    s.execute("arrayedit", &json!({"handle": a, "replace": {"handles": [other["handle"]], "base": [50, 50], "items": [[2, 0, 0]]}})).unwrap();
    let d = s.doc().unwrap();
    let p = d.array_params(d.entity(handle_of(&a)).unwrap()).unwrap();
    assert_eq!(p.replaced.len(), 1);
}

#[test]
fn hostile_path_arrays_are_refused() {
    let mut s = Session::new();
    let c = s.execute("circle", &json!({"center": [0, 0], "radius": 0.5})).unwrap();
    let path = h(&s.execute("line", &json!({"points": [[0, 0], [20, 0]]})).unwrap()["handles"][0]);
    for p in [
        json!({"handles": [c["handle"]], "path": path, "spacing": 0}),
        json!({"handles": [c["handle"]], "path": path, "spacing": -3}),
        json!({"handles": [c["handle"]], "path": path, "spacing": 1e-9}),
        json!({"handles": [c["handle"]], "path": c["handle"].clone(), "count": 3}),
        json!({"handles": [path], "path": path, "count": 3}),
    ] {
        assert!(s.execute("arraypath", &p).is_err(), "{p}");
    }
    // Copies when not associative, as before.
    let r = s.execute("arraypath", &json!({"handles": [c["handle"]], "path": path, "count": 3, "associative": false})).unwrap();
    assert_eq!(r["handles"].as_array().unwrap().len(), 2);
}

#[test]
fn levels_stand_at_their_elevation_in_a_3d_view() {
    use cadaddy_engine::doc::Space;
    use cadaddy_engine::geom::Vec3;
    let mut s = Session::new();
    let c = s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
    s.execute(
        "arrayrect",
        &json!({"handles": [c["handle"]], "rows": 1, "cols": 1, "rowSpacing": 1, "colSpacing": 1, "levels": 3, "levelSpacing": 10}),
    )
    .unwrap();
    let d = s.doc().unwrap();
    let front = cadaddy_render::Options {
        projection: cadaddy_engine::geom::view3::Projection::new(Vec3::new(0.0, -1.0, 0.0)),
        ..cadaddy_render::Options::default()
    };
    let list = cadaddy_render::build(d, &Space::Model, &front);
    // Seen from the front the circles are edge-on, at elevations 0, 10 and 20.
    assert!((list.bounds.height() - 20.0).abs() < 0.1, "{:?}", list.bounds);
    let plan = cadaddy_render::build(d, &Space::Model, &cadaddy_render::Options::default());
    assert!((plan.bounds.height() - 2.0).abs() < 0.1, "{:?}", plan.bounds);
}
