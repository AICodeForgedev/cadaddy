//! Table leftovers (gaps F11): cell text wraps to its column (drawn and in the table's block),
//! rows grow to fit wrapped text, and plain number cells show in their cell style's number
//! format (formula results already did).

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle, Table};
use cadaddy_engine::render;
use serde_json::json;

fn table(s: &Session, h: &str) -> Table {
    match &s.doc().unwrap().entity(Handle::parse_hex(h).unwrap()).unwrap().kind {
        EntityKind::Table(t) => t.clone(),
        _ => panic!("not a table"),
    }
}

fn new_table(s: &mut Session, width: f64) -> String {
    let r = s.execute("table", &json!({ "at": [0, 0], "header": ["Item", "Cost"], "cells": [["a", "1"], ["b", "2.5"]], "colWidth": width })).unwrap();
    r["handle"].as_str().unwrap().to_string()
}

const LONG: &str = "the quick brown fox jumps over the lazy dog again and again";

#[test]
fn cell_text_wraps_to_its_column_and_the_row_grows() {
    let mut s = Session::new();
    let h = new_table(&mut s, 2.0);
    let before = table(&s, &h);
    s.execute("table.set", &json!({ "handle": h, "row": 1, "col": 0, "text": LONG })).unwrap();
    let t = table(&s, &h);
    // The row grew to fit the wrapped lines; the other rows didn't.
    assert!(t.row_heights[1] > before.row_heights[1] * 2.0, "{:?} → {:?}", before.row_heights, t.row_heights);
    assert_eq!(t.row_heights[0], before.row_heights[0]);
    assert_eq!(t.row_heights[2], before.row_heights[2]);
    // The text is laid out to the column width less the margins.
    let g = render::table_geometry(s.doc().unwrap(), &t);
    let tx = g.texts.iter().find(|x| x.row == 1 && x.col == 0).unwrap();
    assert!(tx.width > 0.0 && tx.width < 2.0, "{}", tx.width);
    // Drawn, it stays inside its column.
    let d = s.doc().unwrap();
    let l = render::build(d, &cadaddy_engine::doc::Space::Model, &render::Options::default());
    let xs: Vec<f64> = l.prims.iter().flat_map(|p| l.points(p).to_vec()).map(|p| p.x).collect();
    let text_xs: Vec<f64> = xs.iter().copied().filter(|x| *x > 0.0 && *x < 2.0).collect();
    assert!(!text_xs.is_empty());
    assert!(xs.iter().all(|x| *x <= 4.0 + 1e-6), "something drawn past the table");
    // One undo step with the edit.
    s.undo().unwrap();
    assert_eq!(table(&s, &h).row_heights, before.row_heights);
}

#[test]
fn rows_only_grow() {
    let mut s = Session::new();
    let h = new_table(&mut s, 2.0);
    s.execute("table.set", &json!({ "handle": h, "row": 1, "col": 0, "text": LONG })).unwrap();
    let tall = table(&s, &h).row_heights[1];
    s.execute("table.set", &json!({ "handle": h, "row": 1, "col": 0, "text": "x" })).unwrap();
    assert_eq!(table(&s, &h).row_heights[1], tall);
    // A wider column doesn't need more height: unchanged too.
    let r = s.execute("table", &json!({ "at": [0, -20], "cells": [[LONG]], "colWidth": 100.0 })).unwrap();
    let wide = table(&s, r["handle"].as_str().unwrap());
    assert!(wide.row_heights[0] < tall);
}

#[test]
fn plain_number_cells_show_in_their_number_format() {
    let mut s = Session::new();
    let h = new_table(&mut s, 4.0);
    s.execute("tablestyle.cellstyle", &json!({ "name": "Data", "format": "decimal", "precision": 2 })).unwrap();
    let t = table(&s, &h);
    // Stored as typed…
    assert_eq!(t.cells[1][1].text, "1");
    // …shown formatted; text stays text.
    let g = render::table_geometry(s.doc().unwrap(), &t);
    let shown = |r: usize, c: usize| g.texts.iter().find(|x| x.row == r && x.col == c).map(|x| x.text.clone()).unwrap();
    assert_eq!(shown(1, 1), "1.00");
    assert_eq!(shown(2, 1), "2.50");
    assert_eq!(shown(1, 0), "a");
    s.execute("tablestyle.cellstyle", &json!({ "name": "Data", "format": "percent", "precision": 0 })).unwrap();
    let t = table(&s, &h);
    let g = render::table_geometry(s.doc().unwrap(), &t);
    assert_eq!(g.texts.iter().find(|x| x.row == 2 && x.col == 1).map(|x| x.text.as_str()), Some("250%"));
    // General shows it as typed.
    s.execute("tablestyle.cellstyle", &json!({ "name": "Data", "format": "general", "precision": 4 })).unwrap();
    let t = table(&s, &h);
    let g = render::table_geometry(s.doc().unwrap(), &t);
    assert_eq!(g.texts.iter().find(|x| x.row == 1 && x.col == 1).map(|x| x.text.as_str()), Some("1"));
    // A formula result keeps the text its recalculation formatted.
    s.execute("tablestyle.cellstyle", &json!({ "name": "Data", "format": "decimal", "precision": 1 })).unwrap();
    s.execute("table.set", &json!({ "handle": h, "row": 2, "col": 1, "text": "=B2*2" })).unwrap();
    let t = table(&s, &h);
    assert_eq!(t.cells[2][1].text, "2.0");
    let g = render::table_geometry(s.doc().unwrap(), &t);
    assert_eq!(g.texts.iter().find(|x| x.row == 2 && x.col == 1).map(|x| x.text.as_str()), Some("2.0"));
}
