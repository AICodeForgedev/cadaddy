//! Associative arrays: ARRAYRECT and ARRAYPOLAR make one block reference whose block keeps the
//! array parameters (ASsociative, Levels, polar ROWs); EXPLODE gives plain objects; MOVE and COPY
//! keep the array.

use cadcraft_engine::Session;
use cadcraft_engine::doc::{ArrayKind, ArrayParams, Drawing, Entity, EntityKind, Handle};
use cadcraft_engine::geom::{Mat3, Vec2};
use serde_json::{Value, json};

fn typed(s: &mut Session, inputs: &[&str]) {
    for t in inputs {
        s.cmdline(t).unwrap();
    }
}

fn circle(s: &mut Session, x: f64, y: f64) -> Value {
    s.execute("circle", &json!({"center": [x, y], "radius": 1})).unwrap()["handle"].clone()
}

fn model(s: &Session) -> Vec<Entity> {
    s.doc().unwrap().model.iter().map(|e| (**e).clone()).collect()
}

/// The only object in model space, which must be an associative array, and its parameters.
fn the_array(s: &Session) -> (Entity, ArrayParams) {
    let m = model(s);
    assert_eq!(m.len(), 1, "{m:?}");
    let e = m[0].clone();
    let p = s.doc().unwrap().array_params(&e).expect("an associative array");
    (e, p)
}

/// World positions of the circles drawn by `e` (through nested block references).
fn circles(d: &Drawing, e: &Entity) -> Vec<(Vec2, f64)> {
    fn walk(d: &Drawing, e: &Entity, m: Mat3, z: f64, depth: usize, out: &mut Vec<(Vec2, f64)>) {
        match &e.kind {
            EntityKind::Circle(c) => out.push((m.apply(c.center.xy()), z + c.center.z)),
            EntityKind::Insert(i) if depth < 8 => {
                let b = d.block(&i.block).unwrap();
                let im = m.then_before(i.transform(b.base.xy()));
                for be in b.entities.iter() {
                    walk(d, be, im, z + i.insert.z, depth + 1, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(d, e, Mat3::IDENTITY, 0.0, 0, &mut out);
    out
}

fn has(pts: &[(Vec2, f64)], x: f64, y: f64, z: f64) -> bool {
    pts.iter().any(|(p, pz)| p.near(Vec2::new(x, y), 1e-6) && (pz - z).abs() < 1e-6)
}

#[test]
fn json_arrays_are_associative_by_default() {
    let mut s = Session::new();
    let h = circle(&mut s, 0.0, 0.0);
    let r = s
        .execute(
            "arrayrect",
            &json!({"handles": [h], "rows": 2, "cols": 3, "rowSpacing": 4, "colSpacing": 2, "levels": 2, "levelSpacing": 10, "rowElevation": 1}),
        )
        .unwrap();
    assert_eq!(r["associative"], true);
    assert_eq!(r["created"], 11);
    let (e, p) = the_array(&s);
    assert_eq!(r["handle"], json!(e.handle.hex()));
    assert_eq!(p.levels, 2);
    assert!(matches!(&p.kind, ArrayKind::Rect(k) if k.cols == 3 && k.rows == 2 && k.row_elevation == 1.0));
    // The source circle moved into the array; the items are where the copies would be.
    let c = circles(s.doc().unwrap(), &e);
    assert_eq!(c.len(), 12);
    assert!(has(&c, 0.0, 0.0, 0.0) && has(&c, 4.0, 4.0, 1.0) && has(&c, 4.0, 4.0, 11.0), "{c:?}");

    // associative: false makes separate copies, as before.
    let mut s = Session::new();
    let h = circle(&mut s, 0.0, 0.0);
    let r = s.execute("arraypolar", &json!({"handles": [h], "center": [10, 0], "count": 4, "associative": false})).unwrap();
    assert_eq!(r["created"], 3);
    assert!(r.get("handle").is_none());
    assert_eq!(model(&s).len(), 4);
}

#[test]
fn typed_levels_and_polar_rows() {
    // ARRAYRECT Levels: count, then spacing.
    let mut s = Session::new();
    circle(&mut s, 0.0, 0.0);
    typed(&mut s, &["arrayrect", "l", "", "cou", "2", "1", "s", "5", "5", "l"]);
    assert!(s.prompt_text().contains("Enter the number of levels"), "{}", s.prompt_text());
    typed(&mut s, &["3", "7", "x"]);
    assert!(s.current_prompt().is_none(), "{}", s.prompt_text());
    let (e, p) = the_array(&s);
    assert_eq!((p.levels, p.level_spacing), (3, 7.0));
    let c = circles(s.doc().unwrap(), &e);
    assert_eq!(c.len(), 6);
    assert!(has(&c, 5.0, 0.0, 14.0), "{c:?}");

    // ARRAYPOLAR ROWs: rings further out from the centre, with an elevation per row.
    let mut s = Session::new();
    circle(&mut s, 10.0, 0.0);
    typed(&mut s, &["arraypolar", "l", "", "0,0", "i", "4", "row"]);
    assert!(s.prompt_text().contains("Enter the number of rows"), "{}", s.prompt_text());
    typed(&mut s, &["2", "3", "0.5", "x"]);
    assert!(s.current_prompt().is_none(), "{}", s.prompt_text());
    let (e, p) = the_array(&s);
    assert!(matches!(&p.kind, ArrayKind::Polar(k) if k.items == 4 && k.rows == 2 && k.row_spacing == 3.0));
    let c = circles(s.doc().unwrap(), &e);
    assert_eq!(c.len(), 8);
    assert!(has(&c, 0.0, 10.0, 0.0) && has(&c, 13.0, 0.0, 0.5) && has(&c, 0.0, -13.0, 0.5), "{c:?}");

    // ASsociative No (or ARRAYASSOCIATIVITY 0) makes copies; ROWs work there too.
    let mut s = Session::new();
    s.execute("setvar", &json!({"name": "ARRAYASSOCIATIVITY", "value": 0})).unwrap();
    circle(&mut s, 10.0, 0.0);
    typed(&mut s, &["arraypolar", "l", "", "0,0"]);
    assert!(s.log.iter().any(|l| l.contains("Associative = No")), "{:?}", s.log);
    typed(&mut s, &["i", "4", "row", "2", "3", "", "x"]);
    assert_eq!(model(&s).len(), 8);
    assert!(model(&s).iter().all(|e| matches!(e.kind, EntityKind::Circle(_))));
}

#[test]
fn explode_gives_plain_objects_and_edits_move_the_array() {
    let mut s = Session::new();
    let h = circle(&mut s, 10.0, 0.0);
    let r = s.execute("arraypolar", &json!({"handles": [h], "center": [0, 0], "count": 4})).unwrap();
    let arr = r["handle"].clone();

    // MOVE and COPY transform the block reference; the copy is an array too.
    s.execute("move", &json!({"handles": [arr], "delta": [100, 0]})).unwrap();
    let r = s.execute("copy", &json!({"handles": [arr], "delta": [0, 100]})).unwrap();
    let d = s.doc().unwrap();
    assert_eq!(d.model.len(), 2, "{r}");
    for e in d.model.iter() {
        assert!(d.array_params(e).is_some());
    }
    let e = d.entity(Handle::parse_hex(arr.as_str().unwrap()).unwrap()).unwrap().as_ref().clone();
    let c = circles(d, &e);
    assert!(has(&c, 100.0, 10.0, 0.0) && has(&c, 90.0, 0.0, 0.0), "{c:?}");

    // EXPLODE: the items become circles, not block references.
    let r = s.execute("explode", &json!({"handles": [arr]})).unwrap();
    assert_eq!(r["handles"].as_array().unwrap().len(), 4);
    let d = s.doc().unwrap();
    let plain: Vec<Vec2> = d
        .model
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Circle(c) => Some(c.center.xy()),
            _ => None,
        })
        .collect();
    assert_eq!(plain.len(), 4);
    assert!(plain.iter().any(|p| p.near(Vec2::new(100.0, -10.0), 1e-6)), "{plain:?}");

    // UNDO brings the array back.
    s.execute("undo", &json!({})).unwrap();
    assert!(s.doc().unwrap().entity(Handle::parse_hex(arr.as_str().unwrap()).unwrap()).is_some());
}

#[test]
fn arrays_of_arrays_and_limits() {
    // An array of an array: exploding the outer one gives the inner arrays.
    let mut s = Session::new();
    let h = circle(&mut s, 0.0, 0.0);
    let inner = s.execute("arrayrect", &json!({"handles": [h], "rows": 1, "cols": 3, "colSpacing": 2})).unwrap()["handle"].clone();
    let outer = s.execute("arrayrect", &json!({"handles": [inner], "rows": 2, "cols": 1, "rowSpacing": 5})).unwrap()["handle"].clone();
    let (e, _) = the_array(&s);
    assert_eq!(circles(s.doc().unwrap(), &e).len(), 6);
    let r = s.execute("explode", &json!({"handles": [outer]})).unwrap();
    assert_eq!(r["handles"].as_array().unwrap().len(), 2);
    let d = s.doc().unwrap();
    assert!(d.model.iter().all(|e| d.array_params(e).is_some()));

    // Levels count toward MAXARRAY; nothing is created when refused.
    let mut s = Session::new();
    let h = circle(&mut s, 0.0, 0.0);
    let e = s.execute("arrayrect", &json!({"handles": [h], "rows": 1000, "cols": 1000, "levels": 1000})).unwrap_err();
    assert!(e.to_string().contains("MAXARRAY"), "{e}");
    let e = s.execute("arraypolar", &json!({"handles": [h], "center": [5, 5], "count": 1000, "rows": 1000})).unwrap_err();
    assert!(e.to_string().contains("MAXARRAY"), "{e}");
    assert_eq!(model(&s).len(), 1);
}

#[test]
fn pasted_arrays_get_their_own_blocks() {
    let mut s = Session::new();
    let h = circle(&mut s, 0.0, 0.0);
    let arr = s.execute("arrayrect", &json!({"handles": [h], "rows": 1, "cols": 3, "colSpacing": 2})).unwrap()["handle"].clone();
    s.execute("copyclip", &json!({"handles": [arr]})).unwrap();
    s.execute("pasteclip", &json!({"at": [0, 50]})).unwrap();
    let d = s.doc().unwrap();
    let params: Vec<ArrayParams> = d.model.iter().filter_map(|e| d.array_params(e)).collect();
    assert_eq!(params.len(), 2);
    // The pasted array's blocks were renamed (the names were taken); its parameters follow.
    assert_ne!(params[0].source, params[1].source);
    assert!(params.iter().all(|p| d.block(&p.source).is_some()));
}

#[test]
fn json_arrays_follow_arrayassociativity() {
    // With ARRAYASSOCIATIVITY 0 the JSON form makes copies unless `associative` asks otherwise.
    let mut s = Session::new();
    s.execute("setvar", &json!({"name": "ARRAYASSOCIATIVITY", "value": 0})).unwrap();
    let h = circle(&mut s, 0.0, 0.0);
    let r = s.execute("arrayrect", &json!({"handles": [h], "rows": 1, "cols": 3, "rowSpacing": 4, "colSpacing": 2})).unwrap();
    assert_ne!(r["associative"], true);
    assert_eq!(model(&s).iter().filter(|e| matches!(e.kind, EntityKind::Circle(_))).count(), 3);
    let r = s.execute("arraypolar", &json!({"handles": [h], "center": [10, 0], "count": 4, "associative": true})).unwrap();
    assert_eq!(r["associative"], true);
}
