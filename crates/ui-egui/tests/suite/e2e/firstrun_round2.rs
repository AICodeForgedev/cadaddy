//! Friction a new user met in round 2 of the first ten minutes (Q282): a floor plan, a block with
//! attributes, a layout plotted, an xref attached, a hatch, a table. Each fixed one stays fixed.

use crate::e2e_common::*;
use serde_json::json;

/// The app as it starts: one new drawing whose layouts have their viewports.
fn app() -> E2e {
    let mut e = E2e::new();
    e.app.session = cadaddy_engine::Session::empty();
    e.app.session.new_drawing_with_viewports(false);
    e.pump();
    e
}

/// A person typing a line at the command line (keystrokes, then Enter).
fn keys(e: &mut E2e, text: &str) {
    if !text.is_empty() {
        e.ok("ui.text", json!({ "text": text }));
    }
    e.ok("ui.key", json!({ "key": "Enter" }));
}

fn prompt(e: &mut E2e) -> String {
    e.ok("cmdline.state", json!({}))["prompt"].as_str().unwrap_or_default().to_string()
}

fn table_cells(e: &mut E2e) -> Vec<Vec<String>> {
    let d = e.ok("drawing.inspect", json!({ "entities": true }));
    let t = d["entities"].as_array().unwrap().iter().find(|x| x["type"] == "Table").expect("a table").clone();
    t["geometry"]["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_array().unwrap().iter().map(|c| c["text"].as_str().unwrap_or_default().to_string()).collect())
        .collect()
}

/// TABLE left an empty grid with no way into its cells (no double-click, nothing on the Table
/// Cell tab): a table a person inserts now opens on its title cell, and Enter after each cell's
/// text goes on to the next one, past the cells the title's merge covers.
#[test]
fn a_table_a_person_inserts_opens_on_its_title_cell_and_walks_on() {
    let mut e = app();
    keys(&mut e, "table");
    keys(&mut e, "3");
    keys(&mut e, "2");
    e.ok("ui.pointer", json!({ "x": 0, "y": 0 }));
    assert_eq!(prompt(&mut e), "TABLEDIT Enter text for cell A1 (=formula, Enter to finish):");
    for text in ["Room schedule", "Room", "Area", "Finish", "101"] {
        keys(&mut e, text);
    }
    assert_eq!(prompt(&mut e), "TABLEDIT Enter text for cell B3 (=formula, Enter to finish):");
    keys(&mut e, "");
    assert_eq!(prompt(&mut e), "Command:");
    let cells = table_cells(&mut e);
    assert_eq!(cells[0][0], "Room schedule");
    assert_eq!(cells[1], ["Room", "Area", "Finish"]);
    assert_eq!(cells[2][0], "101");
    assert_eq!(cells[2][1], "");
}

/// An agent's or a script's TABLE is done when the table is placed: nothing waits for cell text
/// (their next line must not land in a cell).
#[test]
fn an_agents_table_doesnt_wait_for_cell_text() {
    let mut e = app();
    e.typed("table");
    e.typed("3");
    e.typed("2");
    e.typed("0,0");
    assert_eq!(prompt(&mut e), "Command:");
    e.typed("line 0,0 10,10");
    e.typed("");
    assert!(table_cells(&mut e).iter().flatten().all(String::is_empty));
}

/// Insert ▸ Attach asked for the X scale, the Y scale and the rotation after the point (three
/// Enters nobody needed); as AutoCAD's Attach dialog defaults, it now asks only where.
#[test]
fn xattach_from_the_menu_asks_only_where() {
    let dir = folder("round2-xref");
    let site = dir.join("site.dxf");
    let mut other = app();
    other.typed("rectang 0,0 @100,50");
    other.exec("saveas", json!({ "path": site.display().to_string() }));
    let mut e = app();
    let picked = site.display().to_string();
    e.app.services.pick_open = Some(Box::new(move || Some(picked.clone())));
    e.menu("xattach");
    assert_eq!(prompt(&mut e), "XATTACH Specify insertion point or [Basepoint/Scale/Rotate]:");
    e.ok("ui.pointer", json!({ "x": 500, "y": 0 }));
    assert_eq!(prompt(&mut e), "Command:", "one pick and it's placed");
    let d = e.ok("drawing.inspect", json!({ "entities": true }));
    let x = d["entities"].as_array().unwrap().iter().find(|x| x["type"] == "Block Reference").expect("the xref").clone();
    let g = &x["geometry"];
    assert_eq!(g["block"], "site");
    assert_eq!(
        (g["insert"]["x"].as_f64(), g["scale"]["x"].as_f64(), g["scale"]["y"].as_f64(), g["rotation"].as_f64()),
        (Some(500.0), Some(1.0), Some(1.0), Some(0.0)),
        "{g}"
    );
}

/// With ANGBASE = 90° and ANGDIR = 1 (clockwise, a common site/survey setup), Insert ▸ Attach
/// must still place the xref at rotation 0 and scale 1/1. Before the fix, the rotation preset
/// sent `Text("0")` which `direction()` offset by ANGBASE; now it sends `Enter` which
/// PresetRotation maps to absolute 0.0.
#[test]
fn xattach_rotation_is_absolute_zero_regardless_of_angbase_and_angdir() {
    let dir = folder("round2-xref-angbase");
    let site = dir.join("survey.dxf");
    let mut other = app();
    other.typed("rectang 0,0 @100,50");
    other.exec("saveas", json!({ "path": site.display().to_string() }));
    let mut e = app();
    // Set ANGBASE = 90° and ANGDIR = 1 (clockwise).
    e.exec("setvar", json!({ "name": "ANGBASE", "value": 90 }));
    e.exec("setvar", json!({ "name": "ANGDIR", "value": 1 }));
    let picked = site.display().to_string();
    e.app.services.pick_open = Some(Box::new(move || Some(picked.clone())));
    e.menu("xattach");
    assert_eq!(prompt(&mut e), "XATTACH Specify insertion point or [Basepoint/Scale/Rotate]:");
    e.ok("ui.pointer", json!({ "x": 200, "y": 0 }));
    assert_eq!(prompt(&mut e), "Command:", "one pick and it's placed");
    let d = e.ok("drawing.inspect", json!({ "entities": true }));
    let x = d["entities"].as_array().unwrap().iter().find(|x| x["type"] == "Block Reference").expect("the xref").clone();
    let g = &x["geometry"];
    assert_eq!(g["block"], "survey");
    assert_eq!(
        (g["insert"]["x"].as_f64(), g["scale"]["x"].as_f64(), g["scale"]["y"].as_f64(), g["rotation"].as_f64()),
        (Some(200.0), Some(1.0), Some(1.0), Some(0.0)),
        "rotation must be exactly 0.0 even with ANGBASE 90° ANGDIR 1: {g}"
    );
}

/// ATTDEF's default height (0.2) on a 40-foot plan makes a tag no one can see: it now gets the
/// same hint as TEXT and MTEXT.
#[test]
fn a_tiny_attribute_definition_gets_the_readability_hint() {
    let mut e = app();
    keys(&mut e, "rectang");
    keys(&mut e, "0,0");
    keys(&mut e, "@40',30'");
    for line in ["attdef", "ROOM", "Room name", "OFFICE", "100,100", "", ""] {
        keys(&mut e, line);
    }
    assert_eq!(prompt(&mut e), "Command:");
    let h = e.history();
    assert!(h.iter().any(|l| l.contains("too small to read")), "{h:?}");
}
