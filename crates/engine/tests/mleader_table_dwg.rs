//! Native MULTILEADER and ACAD_TABLE through DWG, with their styles: every multileader content
//! kind (text, block with an attribute, no leader lines, two leaders, overrides) and table cell
//! type (title, header, text, numbers, a formula, merged cells, a custom cell style, row styles, a
//! broken table) — kept whole in a DWG of 2007 or later, as the exchange check sees it and as
//! opening the file gives it.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Drawing, EntityKind};
use serde_json::{Value, json};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

fn handle(v: &Value) -> String {
    v["handle"].as_str().or_else(|| v["handles"][0].as_str()).unwrap().to_string()
}

/// The drawing and its formula table's handle.
fn drawing() -> (Session, String) {
    let mut s = Session::new();
    let mut x = |id: &str, p: Value| s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"));
    let c = handle(&x("circle", json!({"center": [0, 0], "radius": 1})));
    let a = handle(&x("attdef", json!({"tag": "NO", "default": "1", "at": [0, 0], "height": 0.5})));
    x("block", json!({"name": "BUBBLE", "base": [0, 0], "handles": [c, a]}));
    x("mleaderstyle", json!({"name": "Notes", "arrowSize": 0.3, "textHeight": 0.4, "landingGap": 0.1, "dogleg": 0.8}));
    x("mleaderstyle", json!({"name": "Tags", "block": "BUBBLE", "blockAttach": "center", "leaderType": "spline"}));
    let m1 = handle(&x("mleader", json!({"points": [[0, 0], [5, 5]], "text": "Text content\\Psecond line"})));
    let m2 = handle(&x("mleader", json!({"points": [[10, 0], [15, 5]], "leaderType": "spline", "block": "BUBBLE", "blockAttach": "center"})));
    x("mleader", json!({"points": [[20, 0]], "leaderType": "none", "text": "No leader"}));
    x("mleaderedit.add", json!({"handle": m1, "points": [[0, 3]]}));
    x("mleaderedit", json!({"handle": m2, "mode": "set", "leaderColor": "1", "arrow": "_Dot", "textFrame": true}));
    x("tablestyle", json!({"name": "Schedule", "textHeight": 0.25, "margin": 0.05, "title": true, "header": true}));
    x(
        "tablestyle.cellstyle",
        json!({"style": "Schedule", "name": "Money", "fill": "3", "align": "MR", "format": "decimal", "precision": 2, "borders": {"outside": {"visible": true, "color": "5"}}}),
    );
    let t = handle(&x(
        "table",
        json!({"at": [0, 20], "rows": 4, "cols": 3, "title": "Costs", "header": ["Item", "Qty", "Price"], "cells": [["A", "2", "10.5"], ["B", "3", "4.25"], ["C", "1", "100"], ["", "", ""]]}),
    ));
    x("table.formula", json!({"handle": t, "row": 5, "col": 2, "function": "sum", "range": "C3:C5"}));
    x("table.merge", json!({"handle": t, "row": 5, "col": 0, "rows": 1, "cols": 2}));
    x("table.cellstyle", json!({"handle": t, "style": "Money", "row": 2, "col": 2, "rows": 4, "cols": 1}));
    x("table.set", json!({"handle": t, "row": 5, "col": 0, "text": "Total"}));
    let t2 = handle(&x("table", json!({"at": [20, 20], "rows": 20, "cols": 2})));
    x("table.break", json!({"handle": t2, "enabled": true, "height": 3.0}));
    (s, t)
}

#[test]
fn every_multileader_and_table_kind_goes_through_dwg() {
    hooks();
    let (mut s, _) = drawing();
    for version in [Value::Null, json!(2007), json!(2010), json!(2013), json!(2018)] {
        let mut p = json!({"format": "dwg"});
        if !version.is_null() {
            p["version"] = version.clone();
        }
        let r = s.execute("checkexchange", &p).unwrap();
        assert_eq!(r["kept"], true, "DWG {version}: {r}");
    }
}

#[test]
fn opening_the_dwg_shows_the_formula_value_and_the_cell_styles() {
    hooks();
    let (s, t) = drawing();
    let d = s.doc().unwrap().clone();
    let dwg = cadaddy_io::write(&d, "x.dwg").unwrap();
    let mut o = Session::new();
    o.execute("open", &json!({"data": cadaddy_engine::cmd::file::base64_encode(&dwg), "name": "x.dwg"})).unwrap();
    let back: &Drawing = o.doc().unwrap();
    // The formula's value (the DWG writer loses what it keeps in the FIELD), worked out on opening.
    let h = cadaddy_engine::doc::Handle::parse_hex(&t).unwrap();
    let cell = back.model.iter().find(|e| e.handle == h).and_then(|e| match &e.kind {
        EntityKind::Table(t) => t.cells.get(5).and_then(|r| r.get(2)).cloned(),
        _ => None,
    });
    let cell = cell.unwrap();
    assert_eq!((cell.formula.as_deref(), cell.text.as_str(), cell.style.as_str()), (Some("=Sum(C3:C5)"), "114.75", "Money"));
    assert!(!o.state().unwrap().is_dirty(), "the worked-out value isn't an edit");
    // The custom cell style (kept in an XRECORD: the DWG writer keeps no xdata on TABLESTYLE).
    let style = back.table_styles.iter().find(|s| s.name == "Schedule").unwrap();
    assert!(style.cell_styles.iter().any(|c| c.name == "Money" && c.precision == 2), "{:?}", style.cell_styles);
    // The multileaders: text, block and none content, two leaders, the overrides.
    let ml: Vec<_> = back.model.iter().filter_map(|e| if let EntityKind::MLeader(m) = &e.kind { Some(m.clone()) } else { None }).collect();
    assert_eq!(ml.len(), 3);
    assert!(ml.iter().any(|m| m.leaders.len() == 2 && m.text.as_ref().is_some_and(|t| t.contents.contains("second line"))));
    assert!(ml.iter().any(|m| m.spline && m.block.as_ref().is_some_and(|b| b.block == "BUBBLE") && m.text_frame && m.arrow.is_some()));
    assert!(ml.iter().any(|m| m.leaders.is_empty()));
}

#[test]
fn older_dwg_gets_leaders_and_text_as_older_dxf_does() {
    hooks();
    let (mut s, _) = drawing();
    for f in ["dwg", "dxf"] {
        let r = s.execute("checkexchange", &json!({"format": f, "version": 2004})).unwrap();
        let ml = r["losses"].as_array().unwrap().iter().find(|l| l["feature"] == "mleader objects").cloned().unwrap_or_default();
        assert_eq!((ml["what"].as_str(), ml["count"].as_u64()), (Some("dropped"), Some(3)), "{f} 2004: {r}");
    }
    // What the file has in their place: the leaders and their text.
    let d = s.doc().unwrap().clone();
    let bytes =
        cadaddy_io::write_with(&d, "x.dwg", &cadaddy_io::SaveOptions { version: Some(cadaddy_engine::cmd::file::Version::R2004), binary: false })
            .unwrap();
    let back = cadaddy_io::read(&bytes, "x.dwg").unwrap();
    let count = |f: fn(&EntityKind) -> bool| back.model.iter().filter(|e| f(&e.kind)).count();
    assert!(count(|k| matches!(k, EntityKind::Leader(_))) >= 3, "a LEADER per leader line");
    assert!(count(|k| matches!(k, EntityKind::MText(m) if m.contents.contains("No leader"))) == 1);
}
