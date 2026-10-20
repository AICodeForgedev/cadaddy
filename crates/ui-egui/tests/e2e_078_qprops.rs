//! End to end (0.7.8): Quick Properties with a mixed selection. Two circles of different radii
//! and a line on another layer: the window offers "All (3)" with a row per type, shows *VARIES*
//! where they differ, and narrowed to the circles a diameter typed there changes both circles and
//! not the line (the selection stays all three).

mod e2e_common;

use e2e_common::E2e;
use egui::{Event, Key, Modifiers};
use serde_json::{Value, json};

/// The Quick Properties window's controls: those left of the docked Properties palette.
fn quick(e: &mut E2e) -> Vec<Value> {
    let palette_left = f64::from(e2e_common::W) * 0.8;
    e.elements()
        .into_iter()
        .filter(|x| x["rect"][0].as_f64().is_some_and(|l| l < palette_left) && x["rect"][1].as_f64().is_some_and(|t| t > 120.0))
        .collect()
}

fn labels(els: &[Value], role: &str) -> Vec<String> {
    els.iter().filter(|x| x["role"] == role).filter_map(|x| x["label"].as_str().map(str::to_string)).collect()
}

fn click_el(e: &mut E2e, el: &Value) {
    let r: Vec<f64> = el["rect"].as_array().into_iter().flatten().filter_map(Value::as_f64).collect();
    e.ok("ui.click", json!({ "x": r[0] + r[2] / 2.0, "y": r[1] + r[3] / 2.0, "space": "screen" }));
    e.pump();
}

fn key(e: &mut E2e, k: Key, modifiers: Modifiers) {
    e.events.push(Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers });
    e.events.push(Event::Key { key: k, physical_key: None, pressed: false, repeat: false, modifiers });
    e.frame();
}

fn geometry(e: &mut E2e, handle: &Value) -> Value {
    let all = e.exec("entities", json!({}));
    all["entities"].as_array().unwrap().iter().find(|x| x["handle"] == *handle).cloned().unwrap_or_else(|| panic!("{handle}: {all}"))
}

#[test]
fn a_mixed_selection_shows_varies_and_edits_only_the_shown_type() {
    let mut e = E2e::new();
    e.exec("qpmode", json!({ "on": true }));
    e.exec("layer.new", json!({ "name": "WALLS" }));
    let a = e.exec("circle", json!({ "center": [10, 10], "radius": 5 }))["handle"].clone();
    let b = e.exec("circle", json!({ "center": [40, 10], "radius": 7 }))["handle"].clone();
    let l = e.exec("line", json!({ "points": [[0, 30], [50, 30]] }))["handles"][0].clone();
    e.exec("chprop", json!({ "handles": [l], "layer": "WALLS" }));
    let line_before = geometry(&mut e, &l)["geometry"].clone();
    e.exec("select", json!({ "handles": [a, b, l] }));

    // All three: the type filter and the shared rows only.
    let els = quick(&mut e);
    let combos = labels(&els, "combobox");
    assert!(combos.contains(&"All (3)".to_string()), "the type filter: {combos:?}");
    assert!(combos.contains(&"*VARIES*".to_string()), "the layer varies: {combos:?}");
    assert!(!labels(&els, "textedit").iter().any(|t| t == "5.0000"), "no geometry rows for mixed types");

    // The filter lists each type with its count.
    let filter = els.iter().find(|x| x["role"] == "combobox" && x["label"] == "All (3)").cloned().unwrap();
    click_el(&mut e, &filter);
    assert!(e.shows("Circle (2)") && e.shows("Line (1)"), "a row per type: {:?}", labels(&e.elements(), "button"));

    // Narrowed to the circles: their rows, the radius *VARIES*.
    assert!(e.click("Circle (2)"));
    e.pump();
    let els = quick(&mut e);
    let fields = labels(&els, "textedit");
    assert!(fields.iter().filter(|f| *f == "*VARIES*").count() >= 2, "centre X and radius vary: {fields:?}");
    assert!(fields.contains(&"10.0000".to_string()), "centre Y shared: {fields:?}");

    // Type 18 in the diameter field (the lowest *VARIES*: centre X, radius, diameter).
    let diameter = els
        .iter()
        .filter(|x| x["role"] == "textedit" && x["label"] == "*VARIES*")
        .max_by(|p, q| p["rect"][1].as_f64().unwrap_or(0.0).total_cmp(&q["rect"][1].as_f64().unwrap_or(0.0)))
        .cloned()
        .unwrap();
    click_el(&mut e, &diameter);
    key(&mut e, Key::A, Modifiers::COMMAND);
    e.events.push(Event::Text("18".into()));
    e.frame();
    key(&mut e, Key::Enter, Modifiers::NONE);
    e.pump();

    let ra = geometry(&mut e, &a)["geometry"]["radius"].as_f64();
    let rb = geometry(&mut e, &b)["geometry"]["radius"].as_f64();
    assert_eq!((ra, rb), (Some(9.0), Some(9.0)), "both circles");
    assert_eq!(geometry(&mut e, &l)["geometry"], line_before, "not the line");
    let sel = e.ok("ui.inspect", json!({}))["session"].clone();
    let n = sel["selection"].as_u64().or_else(|| sel["selected"].as_u64()).or_else(|| sel["selection"].as_array().map(|v| v.len() as u64));
    assert!(n.is_none_or(|n| n == 3), "the selection stays all three: {sel}");
    let fields = labels(&quick(&mut e), "textedit");
    assert!(fields.contains(&"9.0000".to_string()) && fields.contains(&"18.0000".to_string()), "the window shows radius 9, diameter 18: {fields:?}");
}
