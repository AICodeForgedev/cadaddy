//! End to end: the Drawing Properties dialog (DWGPROPS typed) shows the drawing's properties on
//! its tabs; a title typed on Summary and a custom property added on Custom are applied by OK;
//! Cancel changes nothing. The System Variable Monitor opens and lists what it watches.

mod e2e_common;

use e2e_common::E2e;
use egui::{Event, Key, Modifiers};
use serde_json::{Value, json};

fn key(e: &mut E2e, k: Key, modifiers: Modifiers) {
    e.events.push(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers });
    e.events.push(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers });
    e.frame();
}

fn type_into(e: &mut E2e, field: &Value, text: &str) {
    let r: Vec<f64> = field["rect"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
    e.ok("ui.click", json!({ "x": r[0] + r[2] / 2.0, "y": r[1] + r[3] / 2.0, "space": "screen" }));
    e.pump();
    key(e, Key::A, Modifiers::COMMAND);
    e.events.push(Event::Text(text.into()));
    e.frame();
    e.pump();
}

#[test]
fn drawing_properties_are_edited_in_the_dialog() {
    let mut e = E2e::new();
    e.exec("dwgprops", json!({ "title": "Old title", "author": "Project A team" }));
    e.typed("dwgprops");
    assert_eq!(e.dialog().as_deref(), Some("dwgprops"));
    assert!(e.click("Summary"));
    e.pump();
    let title = e.elements().into_iter().find(|x| x["role"] == "textedit" && x["label"] == "Old title").expect("the title field");
    assert!(e.shows("Project A team"), "the author is shown");
    type_into(&mut e, &title, "Sample Floor Plan");
    assert!(e.click("Custom"));
    e.pump();
    assert!(e.click("Add..."));
    e.pump();
    assert!(e.shows("Property 1"), "a new row");
    assert!(e.click("OK"));
    e.pump();
    assert_eq!(e.dialog(), None);
    let p = e.exec("dwgprops", json!({}));
    assert_eq!(p["summary"]["title"], "Sample Floor Plan", "{p}");
    assert_eq!(p["summary"]["author"], "Project A team");
    assert_eq!(p["custom"][0]["name"], "Property 1");

    // Cancel: nothing changes.
    e.typed("dwgprops");
    assert!(e.click("Summary"));
    e.pump();
    let title = e.elements().into_iter().find(|x| x["role"] == "textedit" && x["label"] == "Sample Floor Plan").unwrap();
    type_into(&mut e, &title, "Not kept");
    assert!(e.click("Cancel"));
    e.pump();
    assert_eq!(e.exec("dwgprops", json!({}))["summary"]["title"], "Sample Floor Plan");
    // Statistics show facts (objects counted).
    e.exec("line", json!({ "points": [[0, 0], [10, 0]] }));
    e.typed("dwgprops");
    assert!(e.click("Statistics"));
    e.pump();
    assert!(e.click("Cancel"));
}

#[test]
fn the_variable_monitor_lists_what_it_watches() {
    let mut e = E2e::new();
    e.exec("sysvarmonitor", json!({ "variables": ["FILLETRAD"], "notify": true }));
    e.typed("sysvarmonitor");
    assert_eq!(e.dialog().as_deref(), Some("sysvarmonitor"));
    assert!(e.shows("Remove") && e.shows("Reset All"));
    e.exec("setvar", json!({ "name": "FILLETRAD", "value": 3.5 }));
    assert!(e.history().iter().any(|l| l.contains("SYSVARMONITOR: FILLETRAD changed")), "{:?}", e.history());
    assert!(e.click("Reset All"));
    e.pump();
    let v = e.exec("getvar", json!({ "name": "FILLETRAD" }));
    assert!(v["value"].as_f64().is_some_and(|f| f != 3.5), "{v}");
    assert!(e.click("Close"));
    e.pump();
    assert_eq!(e.dialog(), None);
}
