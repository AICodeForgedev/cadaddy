//! Count tables at the drawing's scale (battle test 0.7.8 #5): a count table scaled with SCALE
//! keeps its size when it updates (rows, text and columns together; only the numbers change), and
//! Insert Count Table takes a scale, by default the annotation scale, so its text reads at the
//! drawing's scale.

use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::doc::geom::Vec3;
use cadaddy_engine::doc::{Block, Circle, Entity, EntityKind, Handle, Insert, Table};
use serde_json::json;

fn recep(s: &mut Session, x: f64) {
    s.add_entity(EntityKind::Insert(Insert {
        block: "RECEP".into(),
        insert: Vec3::new(x, 0.0, 0.0),
        scale: Vec3::new(1.0, 1.0, 1.0),
        rotation: 0.0,
        attribs: Vec::new(),
        cols: 1,
        rows: 1,
        col_spacing: 0.0,
        row_spacing: 0.0,
        clip: None,
    }))
    .unwrap();
}

fn plan() -> Session {
    let mut s = Session::new();
    let d = s.doc_mut().unwrap();
    let mut b = Block::new("RECEP");
    let h = d.new_handle();
    b.entities.push(Entity::new(h, EntityKind::Circle(Circle { center: Vec3::new(0.0, 0.0, 0.0), radius: 3.0 })));
    d.blocks.insert("RECEP".into(), Arc::new(b));
    for i in 0..3 {
        recep(&mut s, f64::from(i) * 50.0);
    }
    s
}

fn table(s: &Session, h: Handle) -> Table {
    match &s.doc().unwrap().entity(h).unwrap().kind {
        EntityKind::Table(t) => (**t).clone(),
        _ => panic!("not a table"),
    }
}

fn count_of(t: &Table) -> String {
    t.cells.iter().find(|r| r.first().is_some_and(|c| c.text == "RECEP")).and_then(|r| r.get(1)).map(|c| c.text.clone()).unwrap_or_default()
}

#[test]
fn a_scaled_count_table_keeps_its_size_when_it_updates() {
    let mut s = plan();
    let h = Handle::parse_hex(s.execute("counttable", &json!({ "at": [0, -50], "scale": 1 })).unwrap()["handle"].as_str().unwrap()).unwrap();
    let small = table(&s, h);
    s.execute("scale", &json!({ "handles": [h.hex()], "base": [0, -50], "factor": 40 })).unwrap();
    let big = table(&s, h);
    assert!((big.text_height - small.text_height * 40.0).abs() < 1e-9, "SCALE scaled it");
    // One more receptacle: REGEN updates the count, and only the count.
    recep(&mut s, 400.0);
    s.execute("regen", &json!({})).unwrap();
    let after = table(&s, h);
    assert_eq!(count_of(&after), "4");
    assert_eq!(
        (after.text_height, &after.row_heights, &after.col_widths),
        (big.text_height, &big.row_heights, &big.col_widths),
        "kept its scaled size"
    );
    // A new block name adds a row like the others.
    {
        let d = s.doc_mut().unwrap();
        let mut b = Block::new("SWITCH");
        let hh = d.new_handle();
        b.entities.push(Entity::new(hh, EntityKind::Circle(Circle { center: Vec3::new(0.0, 0.0, 0.0), radius: 1.0 })));
        d.blocks.insert("SWITCH".into(), Arc::new(b));
    }
    let mut ins = match &s.doc().unwrap().model.iter().find(|e| matches!(e.kind, EntityKind::Insert(_))).unwrap().kind {
        EntityKind::Insert(i) => i.clone(),
        _ => unreachable!(),
    };
    ins.block = "SWITCH".into();
    s.add_entity(EntityKind::Insert(ins)).unwrap();
    s.execute("counttable", &json!({ "update": true })).unwrap();
    let grown = table(&s, h);
    assert_eq!(grown.row_heights.len(), after.row_heights.len() + 1);
    assert_eq!(grown.row_heights.last(), after.row_heights.last(), "the new row is the size of the others");
    assert_eq!(grown.col_widths, big.col_widths);
}

#[test]
fn a_new_count_table_is_sized_for_the_drawing_scale() {
    let mut s = plan();
    // A scale given: everything that size times.
    let h1 = Handle::parse_hex(s.execute("counttable", &json!({ "at": [0, -50], "scale": 1 })).unwrap()["handle"].as_str().unwrap()).unwrap();
    let one = table(&s, h1);
    let h = Handle::parse_hex(s.execute("counttable", &json!({ "at": [0, -500], "scale": 48 })).unwrap()["handle"].as_str().unwrap()).unwrap();
    let t = table(&s, h);
    assert!((t.text_height - one.text_height * 48.0).abs() < 1e-9 && (t.col_widths[0] - one.col_widths[0] * 48.0).abs() < 1e-9);
    // None given: the annotation scale (1/4" = 1'-0" is 48 drawing units per paper unit).
    s.execute("setvar", &json!({ "name": "CANNOSCALE", "value": "1/4\" = 1'-0\"" })).unwrap();
    let h = Handle::parse_hex(s.execute("counttable", &json!({ "at": [0, -900] })).unwrap()["handle"].as_str().unwrap()).unwrap();
    assert!((table(&s, h).text_height - one.text_height * 48.0).abs() < 1e-9);
    // Typed: Scale first, then the point.
    for l in ["counttable", "s", "10", "0,-1500"] {
        s.cmdline(l).unwrap();
    }
    let last = s.doc().unwrap().model.iter().filter(|e| matches!(e.kind, EntityKind::Table(_))).last().unwrap().handle;
    assert!((table(&s, last).text_height - one.text_height * 10.0).abs() < 1e-9);
    // Hostile scales are refused.
    for k in [json!(0), json!(-2), json!("big"), json!(1e9)] {
        assert!(s.execute("counttable", &json!({ "at": [0, 0], "scale": k })).is_err(), "{k}");
    }
}
