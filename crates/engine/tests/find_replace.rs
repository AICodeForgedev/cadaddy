//! FIND across the drawing: every kind of text it looks in (text, multiline text, attributes,
//! dimension overrides, tables, multileaders, hyperlinks, layer and block names), each option
//! (match case, whole words, wildcards, regular expressions, scope, replace one, selection),
//! across layouts, in one undo step.

use cadaddy_engine::Session;
use cadaddy_engine::doc::{EntityKind, Handle};
use serde_json::{Value, json};

fn h(v: &Value) -> String {
    v["handle"].as_str().unwrap().to_string()
}

/// The handles of every object and what was made.
struct Fixture {
    s: Session,
    text: String,
    mtext: String,
    tag: String,
    dim: String,
    table: String,
    leader: String,
    line: String,
}

fn fixture() -> Fixture {
    let mut s = Session::new();
    let text = h(&s.execute("text", &json!({ "at": [0, 0], "height": 1, "text": "DOOR 1" })).unwrap());
    let mtext = h(&s.execute("mtext", &json!({ "at": [0, 5], "text": "Door frame and doorstop" })).unwrap());
    let def = h(&s.execute("attdef", &json!({ "tag": "NUM", "prompt": "Number", "default": "D-000", "at": [0, 0] })).unwrap());
    s.execute("block", &json!({ "name": "DOORTAG", "base": [0, 0], "handles": [def], "keep": "delete" })).unwrap();
    let tag = h(&s.execute("insert", &json!({ "name": "DOORTAG", "at": [20, 0], "attribs": { "NUM": "D-101" } })).unwrap());
    let dim = h(&s.execute("dimlinear", &json!({ "p1": [0, 10], "p2": [30, 10], "at": [15, 14], "text": "DOOR WIDTH <>" })).unwrap());
    let table = h(&s.execute("table", &json!({ "at": [50, 0], "rows": 1, "cols": 2, "cells": [["Door", "Window"]] })).unwrap());
    let leader = h(&s.execute("mleader", &json!({ "points": [[0, 30], [5, 35]], "text": "Fire door" })).unwrap());
    let line = s.execute("line", &json!({ "points": [[0, 40], [10, 40]] })).unwrap();
    let line = line["handle"].as_str().or_else(|| line["handles"][0].as_str()).unwrap().to_string();
    s.execute("hyperlink", &json!({ "handles": [line], "url": "http://example.com/door-spec", "description": "Door spec" })).unwrap();
    s.doc_mut().unwrap().layers.push(cadaddy_engine::doc::Layer::new("DOORS"));
    // A layout with its own text.
    s.execute("layout", &json!({ "option": "new", "name": "Sheet" })).unwrap();
    s.execute("layout", &json!({ "option": "set", "name": "Sheet" })).unwrap();
    s.execute("text", &json!({ "at": [1, 1], "height": 1, "text": "door schedule" })).unwrap();
    s.execute("layout", &json!({ "option": "set", "name": "Model" })).unwrap();
    Fixture { s, text, mtext, tag, dim, table, leader, line }
}

fn handles(r: &Value) -> Vec<String> {
    r["matches"].as_array().unwrap().iter().filter_map(|m| m["handle"].as_str().map(str::to_string)).collect()
}

fn kind(s: &Session, hex: &str) -> EntityKind {
    s.doc().unwrap().entity(Handle::parse_hex(hex).unwrap()).unwrap().kind.clone()
}

#[test]
fn every_kind_is_found_across_layouts() {
    let mut f = fixture();
    let r = f.s.execute("find", &json!({ "find": "door" })).unwrap();
    let hs = handles(&r);
    for (what, hx) in
        [("text", &f.text), ("mtext", &f.mtext), ("dimension", &f.dim), ("table", &f.table), ("mleader", &f.leader), ("hyperlink", &f.line)]
    {
        assert!(hs.contains(hx), "{what} not found: {r}");
    }
    assert!(r["matches"].as_array().unwrap().iter().any(|m| m["space"] == "Sheet"), "the layout's text");
    assert!(!r["matches"].as_array().unwrap().iter().any(|m| m["name"].is_string()), "names only when asked");
    // The hits in model space are selected.
    let sel: Vec<String> = f.s.state().unwrap().selection.iter().map(|h| h.hex()).collect();
    assert!(sel.contains(&f.text) && sel.len() == hs.len() - 1, "{sel:?}");

    let r = f.s.execute("find", &json!({ "find": "D-101" })).unwrap();
    assert_eq!(handles(&r), [f.tag.clone()], "the attribute value");
    let r = f.s.execute("find", &json!({ "find": "door", "include": ["names"] })).unwrap();
    let names: Vec<&str> = r["matches"].as_array().unwrap().iter().filter_map(|m| m["name"].as_str()).collect();
    assert!(names.contains(&"DOORS") && names.contains(&"DOORTAG"), "{names:?}");
    let r = f.s.execute("find", &json!({ "find": "door", "in": "space" })).unwrap();
    assert!(!r["matches"].as_array().unwrap().iter().any(|m| m["space"] == "Sheet"));
}

#[test]
fn options_narrow_the_search() {
    let mut f = fixture();
    let r = f.s.execute("find", &json!({ "find": "DOOR", "matchCase": true, "include": ["text", "dimensions"] })).unwrap();
    let mut hs = handles(&r);
    hs.sort();
    let mut want = vec![f.text.clone(), f.dim.clone()];
    want.sort();
    assert_eq!(hs, want);
    let r = f.s.execute("find", &json!({ "find": "door", "wholeWord": true, "include": ["text"], "in": "space" })).unwrap();
    // "doorstop" isn't the word; "Door" in the mtext is.
    assert_eq!(r["total"], 2, "{r}");
    let r = f.s.execute("find", &json!({ "find": "D-###", "mode": "wildcard" })).unwrap();
    assert_eq!(handles(&r), [f.tag.clone()]);
    let r = f.s.execute("find", &json!({ "find": "d[o]+r\\b", "mode": "regex" })).unwrap();
    assert!(handles(&r).contains(&f.text));
    assert!(f.s.execute("find", &json!({ "find": "(", "mode": "regex" })).is_err());
    assert!(f.s.execute("find", &json!({ "find": "x", "include": ["widgets"] })).is_err());
    // In the selection only.
    f.s.set_selection(vec![Handle::parse_hex(&f.mtext).unwrap()]);
    let r = f.s.execute("find", &json!({ "find": "door", "in": "selection", "select": false })).unwrap();
    assert_eq!(handles(&r), [f.mtext.clone()]);
}

#[test]
fn replacing_one_all_names_and_links_in_one_undo_step() {
    let mut f = fixture();
    // Regular expression with a group, attributes only.
    f.s.execute("find", &json!({ "find": "D-(\\d+)", "replace": "R-$1", "mode": "regex", "include": ["attributes"] })).unwrap();
    let EntityKind::Insert(i) = kind(&f.s, &f.tag) else { panic!() };
    assert_eq!(i.attribs[0].text.value, "R-101");
    // Replace one: only the chosen object.
    f.s.execute("find", &json!({ "find": "door", "replace": "gate", "handles": [f.text] })).unwrap();
    let EntityKind::Text(t) = kind(&f.s, &f.text) else { panic!() };
    assert_eq!(t.value, "gate 1");
    let EntityKind::MText(m) = kind(&f.s, &f.mtext) else { panic!() };
    assert!(m.contents.starts_with("Door"), "untouched");

    // Replace all, names and hyperlinks too, in one step.
    let before = f.s.doc().unwrap().clone();
    let r =
        f.s.execute(
            "find",
            &json!({ "find": "door", "replace": "gate", "include": ["text", "dimensions", "tables", "mleaders", "hyperlinks", "names"] }),
        )
        .unwrap();
    assert!(r["total"].as_u64().unwrap() >= 8, "{r}");
    let d = f.s.doc().unwrap();
    assert!(d.layer("gateS").is_some() && d.layer("DOORS").is_none(), "layer renamed");
    assert!(d.block("gateTAG").is_some(), "block renamed");
    let EntityKind::Insert(i) = kind(&f.s, &f.tag) else { panic!() };
    assert_eq!(i.block, "gateTAG", "references follow the block");
    let EntityKind::Dimension(dm) = kind(&f.s, &f.dim) else { panic!() };
    assert_eq!(dm.text, "gate WIDTH <>");
    let EntityKind::Table(tb) = kind(&f.s, &f.table) else { panic!() };
    assert!(tb.cells.iter().flatten().any(|c| c.text == "gate"));
    let EntityKind::MLeader(ml) = kind(&f.s, &f.leader) else { panic!() };
    assert_eq!(ml.text.as_ref().unwrap().contents, "Fire gate");
    let link = f.s.execute("hyperlink", &json!({ "handles": [f.line] })).unwrap();
    assert_eq!(link["links"][0]["description"], "gate spec");
    assert!(
        f.s.doc().unwrap().layouts.iter().any(|l| l.entities.iter().any(|e| matches!(&e.kind, EntityKind::Text(t) if t.value == "gate schedule")))
    );

    f.s.execute("undo", &json!({})).unwrap();
    assert_eq!(f.s.doc().unwrap().layers.len(), before.layers.len());
    assert!(f.s.doc().unwrap().layer("DOORS").is_some(), "one undo step");
    let EntityKind::Dimension(dm) = kind(&f.s, &f.dim) else { panic!() };
    assert_eq!(dm.text, "DOOR WIDTH <>");
}
