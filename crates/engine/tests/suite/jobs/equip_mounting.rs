//! Q6494: mechanical equipment a plan tags with no height is mounted where its kind goes (rules
//! data `M-EQUIP-MOUNT`), never a 6 ft box on the floor: a ceiling exhaust fan's housing above
//! the ceiling, a PTAC on its sub-base, a kind no rule names on the floor at its usual height.

use crate::helper::run;
use cadaddy_engine::Session;
use serde_json::{Value, json};

fn layer(s: &mut Session, name: &str) {
    if s.execute("layer.new", &json!({"name": name, "current": true})).is_err() {
        run(s, "layer.current", json!({"name": name}));
    }
}

/// A unit's outline on the equipment layer with its tag inside it (inches).
fn unit(s: &mut Session, lo: [f64; 2], hi: [f64; 2], tag: &str) {
    layer(s, "EQUIP");
    run(s, "pline", json!({"vertices": [lo, [hi[0], lo[1]], hi, [lo[0], hi[1]]], "closed": true}));
    layer(s, "TEXT");
    run(s, "text", json!({"at": [lo[0] + 4.0, (lo[1] + hi[1]) / 2.0], "text": tag, "height": 4}));
}

/// A mechanical plan of one room: a supply duct, a 14 in ceiling exhaust fan in the room, a
/// PTAC under the window in the outside wall, a rooftop unit's outline, no heights anywhere.
fn plan(bands: Value) -> Session {
    let mut s = Session::new();
    layer(&mut s, "A-WALL");
    run(&mut s, "pline", json!({"vertices": [[-12, -12], [492, -12], [492, 372], [-12, 372]], "closed": true}));
    run(&mut s, "pline", json!({"vertices": [[-6, -6], [486, -6], [486, 366], [-6, 366]], "closed": true}));
    layer(&mut s, "M-DUCT-SUPP");
    run(&mut s, "pline", json!({"vertices": [[24, 200], [300, 200]]}));
    layer(&mut s, "TEXT");
    run(&mut s, "text", json!({"at": [120.0, 210.0], "text": "12x8 SA", "height": 4}));
    unit(&mut s, [200.0, 100.0], [214.0, 114.0], "EF-1");
    unit(&mut s, [380.0, -4.0], [422.0, 12.0], "PTAC-1");
    unit(&mut s, [380.0, 250.0], [480.0, 320.0], "RTU-1");
    run(
        &mut s,
        "bspec.set",
        json!({"replace": true, "spec": {
            "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9,
                        "region": [-100, -100, 1000, 1000], "base": [0, 0]}],
            "layers": [{"layer": "A-WALL", "role": "wall"}],
            "mep": {"elevation_bands": bands},
        }}),
    );
    s
}

fn equipment(s: &mut Session) -> Vec<Value> {
    run(s, "bspec.get", json!({}))["spec"]["mep"]["equipment"].as_array().cloned().unwrap_or_default()
}

fn find<'a>(eq: &'a [Value], tag: &str) -> &'a Value {
    eq.iter().find(|e| e["tag"].as_str().is_some_and(|t| t.starts_with(tag))).unwrap_or_else(|| panic!("no {tag} in {eq:#?}"))
}

fn ft(e: &Value, key: &str) -> f64 {
    e[key].as_f64().unwrap_or(0.0)
}

#[test]
fn tagged_equipment_with_no_height_is_mounted_by_its_kind_not_a_box_on_the_floor() {
    let mut s = plan(json!([]));
    run(&mut s, "ducts.read", json!({"apply": true}));
    let eq = equipment(&mut s);
    let near = |a: f64, b: f64| (a - b).abs() < 1e-6;

    let ef = find(&eq, "EF-1");
    assert_eq!(ef["kind"], "exhaust_fan", "{ef:#}");
    assert!(near(ft(ef, "bottom_ft"), 9.0) && near(ft(ef, "height_ft"), 10.0 / 12.0), "housing above the 9 ft ceiling: {ef:#}");
    assert!(ef["sources"]["elevation"].as_str().is_some_and(|b| b.starts_with("M-EQUIP-MOUNT")), "the rule is its source: {ef:#}");

    let ptac = find(&eq, "PTAC-1");
    assert!(near(ft(ptac, "bottom_ft"), 4.0 / 12.0) && near(ft(ptac, "height_ft"), 16.0 / 12.0), "on its sub-base, 16 in high: {ptac:#}");

    // No mounting rule names a rooftop unit's kind: it stands on its floor at its kind's usual height.
    let rtu = find(&eq, "RTU-1");
    assert!(near(ft(rtu, "bottom_ft"), 0.0) && near(ft(rtu, "height_ft"), 4.5), "{rtu:#}");

    // Nothing is the old 6 ft floor box.
    for e in &eq {
        assert!(!(near(ft(e, "bottom_ft"), 0.0) && near(ft(e, "height_ft"), 6.0)), "a 6 ft box on the floor: {e:#}");
    }

    // The model builds them where the spec has them.
    run(&mut s, "model.build", json!({}));
    let after = equipment(&mut s);
    assert!(near(ft(find(&after, "EF-1"), "bottom_ft"), 9.0));
}

#[test]
fn the_equipment_band_is_where_floor_equipment_stands_never_its_height() {
    let mut s = plan(json!([{"trade": "equipment", "bottom_ft": 1.5}]));
    run(&mut s, "ducts.read", json!({"apply": true}));
    let eq = equipment(&mut s);
    let rtu = find(&eq, "RTU-1");
    assert!((ft(rtu, "bottom_ft") - 1.5).abs() < 1e-6 && (ft(rtu, "height_ft") - 4.5).abs() < 1e-6, "{rtu:#}");
    // A kind with its own mounting keeps it.
    assert!((ft(find(&eq, "EF-1"), "bottom_ft") - 9.0).abs() < 1e-6);
}
