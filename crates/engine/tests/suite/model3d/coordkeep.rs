//! Data kept beside the building spec in the same JSON (saved coordination views, viewpoints,
//! colour schemes, the BIM look) survives every command that writes the typed spec: a command
//! changes only the data it is about.

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// A one-level building with a duct, built to 3D.
fn building() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
    s.execute("pline", &json!({"vertices": [[0, 0], [480, 0], [480, 360], [0, 360]], "closed": true})).unwrap();
    s.execute(
        "bspec.set",
        &json!({"replace": true, "spec": {
            "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9}],
            "layers": [{"layer": "A-WALL", "role": "wall"}],
            "rooms": [{"name": "Office", "ceiling_ft": 8}],
            "mep": {
                "systems": [{"name": "M-SUPPLY", "layer": "M-SUPPLY", "kind": "duct", "size_in": [12, 8], "bottom_ft": 10}],
                "runs": [{"system": "M-SUPPLY", "level": "Level 1", "points": [[24, 60], [264, 60]], "size_in": [12, 8], "bottoms_ft": [10]}]
            }
        }}),
    )
    .unwrap();
    s.execute("model.build", &json!({})).unwrap();
    s
}

/// Save one coordination view, one viewpoint and one colour scheme.
fn save_coordination(s: &mut Session) {
    s.execute("coordination.views", &json!({"save": {"name": "v1"}})).unwrap();
    s.execute("coordination.viewpoints", &json!({"save": {"name": "vp1"}})).unwrap();
    s.execute("coordination.colors", &json!({"save": {"name": "c1", "colors": {"M-SUPPLY": [255, 0, 0]}}})).unwrap();
}

fn names(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter().filter_map(|x| x.as_str().map(str::to_string).or_else(|| x.get("name").and_then(Value::as_str).map(str::to_string))).collect()
        })
        .unwrap_or_default()
}

/// The saved (views, viewpoints, colour schemes), as the commands list them.
fn saved(s: &mut Session) -> (Vec<String>, Vec<String>, Vec<String>) {
    let views = s.execute("coordination.views", &json!({"list": true})).unwrap();
    let vps = s.execute("coordination.viewpoints", &json!({})).unwrap();
    let schemes = s.execute("coordination.colors", &json!({"list": true})).unwrap();
    (names(&views, "views"), names(&vps, "viewpoints"), names(&schemes, "schemes"))
}

fn want() -> (Vec<String>, Vec<String>, Vec<String>) {
    (vec!["v1".to_string()], vec!["vp1".to_string()], vec!["c1".to_string()])
}

#[test]
fn coordination_data_survives_every_spec_writer() {
    let writers: Vec<(&str, Value)> = vec![
        ("estimate.settings", json!({"tax_pct": 5})),
        ("estimate.set_price", json!({"item": "Door", "unit": "each", "material": 777, "date": "2026-01-02", "source": "t"})),
        ("bim.estimate", json!({"revision": "r1"})),
        ("schedule.build", json!({"start": "2026-12-01"})),
        ("bspec.set", json!({"spec": {"rooms": [{"name": "Office", "ceiling_ft": 9}]}})),
        // Replacing the building spec replaces the spec, not the data kept beside it.
        ("bspec.set", json!({"replace": true, "spec": {"levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12}]}})),
    ];
    let mut s = building();
    save_coordination(&mut s);
    assert_eq!(saved(&mut s), want(), "saved before the writers run");
    for (cmd, p) in writers {
        s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd} {p}: {e}"));
        assert_eq!(saved(&mut s), want(), "after {cmd} {p}");
    }
}

#[test]
fn the_writers_still_change_what_they_are_about() {
    let mut s = building();
    save_coordination(&mut s);
    s.execute("estimate.settings", &json!({"tax_pct": 5})).unwrap();
    s.execute("bspec.set", &json!({"spec": {"rooms": [{"name": "Office", "ceiling_ft": 9}]}})).unwrap();
    let text = s.doc().unwrap().building_spec.as_deref().unwrap_or_default().to_string();
    let spec: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(spec["rooms"][0]["ceiling_ft"], json!(9.0), "{text}");
    assert!(spec["estimate"].to_string().contains('5'), "the tax setting is kept: {}", spec["estimate"]);
    // Kept once, as written, not nested or duplicated.
    assert_eq!(text.matches("\"coord_views\"").count(), 1, "{text}");
}

#[test]
fn deleting_a_saved_view_still_deletes_it_after_a_spec_write() {
    let mut s = building();
    save_coordination(&mut s);
    s.execute("estimate.settings", &json!({"tax_pct": 5})).unwrap();
    s.execute("coordination.views", &json!({"delete": "v1"})).unwrap();
    s.execute("bspec.set", &json!({"spec": {"rooms": [{"name": "Office", "ceiling_ft": 9}]}})).unwrap();
    let (views, vps, schemes) = saved(&mut s);
    assert!(views.is_empty(), "{views:?}");
    assert_eq!((vps, schemes), (want().1, want().2));
}
