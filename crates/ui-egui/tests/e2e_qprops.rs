//! End to end: Quick Properties. With QPMODE on, selecting a circle shows the Quick Properties
//! window near it (layer, centre, radius…, apart from the Properties palette); typing a new radius
//! there changes the circle, and the window shows the new value; with nothing selected it goes.

mod e2e_common;

use e2e_common::E2e;
use egui::{Event, Key, Modifiers};
use serde_json::{Value, json};

/// The text fields of the Quick Properties window: those left of the docked Properties palette.
fn quick_fields(e: &mut E2e, palette_left: f64) -> Vec<Value> {
    e.elements().into_iter().filter(|x| x["role"] == "textedit" && x["rect"][0].as_f64().is_some_and(|l| l < palette_left)).collect()
}

fn key(e: &mut E2e, k: Key, modifiers: Modifiers) {
    e.events.push(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers });
    e.events.push(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers });
    e.frame();
}

#[test]
fn quick_properties_show_and_change_the_selected_circle() {
    let mut e = E2e::new();
    e.exec("qpmode", json!({ "on": true }));
    let c = e.exec("circle", json!({ "center": [10, 10], "radius": 5 }));
    let h = c["handle"].clone();
    // The Properties palette's own fields (the docked one) start at its left edge.
    let palette_left = f64::from(e2e_common::W) * 0.8;
    assert!(quick_fields(&mut e, palette_left).is_empty(), "nothing selected: no Quick Properties");

    e.exec("select", json!({ "handles": [h] }));
    let fields = quick_fields(&mut e, palette_left);
    let labels: Vec<&str> = fields.iter().filter_map(|x| x["label"].as_str()).collect();
    assert!(labels.contains(&"5.0000") && labels.iter().filter(|l| **l == "10.0000").count() >= 3, "centre, radius, diameter: {labels:?}");
    assert!(e.shows("Circle") || fields.len() >= 4, "the circle's rows");

    // Type a new radius in its field: click it, select what's there, type, Enter.
    let radius = fields.iter().find(|x| x["label"] == "5.0000").cloned().unwrap();
    let r: Vec<f64> = radius["rect"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
    e.ok("ui.click", json!({ "x": r[0] + r[2] / 2.0, "y": r[1] + r[3] / 2.0, "space": "screen" }));
    key(&mut e, Key::A, Modifiers::COMMAND);
    e.events.push(Event::Text("8".into()));
    e.frame();
    key(&mut e, Key::Enter, Modifiers::NONE);
    e.pump();

    let ents = e.exec("entities", json!({ "type": "Circle" }));
    let radius_now = ents["entities"][0]["geometry"]["radius"].as_f64();
    assert_eq!(radius_now, Some(8.0), "{ents}");
    let labels: Vec<String> = quick_fields(&mut e, palette_left).iter().filter_map(|x| x["label"].as_str().map(str::to_string)).collect();
    assert!(
        labels.contains(&"8.0000".to_string()) && labels.contains(&"16.0000".to_string()),
        "the window shows the new radius and diameter: {labels:?}"
    );

    e.exec("select", json!({ "handles": [] }));
    assert!(quick_fields(&mut e, palette_left).is_empty(), "deselected: gone");
}
