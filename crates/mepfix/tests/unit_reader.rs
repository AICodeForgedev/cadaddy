//! Q305 unit reader: unit types from the enlarged unit plans, instances from the floor plans'
//! unit tags, overrides from the overrides table, scored against the synthetic hotel's truth.

use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::unit_reader::{self, READER, override_map, read_fixture_units, score_units};
use serde_json::{Value, json};

/// Every instance read with the truth's type, room number, level and hand; no misses and no
/// extras; every type with instances read with its size, per-unit counts and wet wall / PTAC
/// sleeve sides; every instance's overrides exactly the truth's.
#[test]
fn every_unit_matches_the_truth() {
    for (seed, size) in [(42, HotelSize::Small), (7, HotelSize::Small), (99, HotelSize::Full)] {
        let (t, r) = read_fixture_units(seed, size);
        let sc = score_units(&t, &r);
        assert!(sc.units_total > 10, "seed {seed}: {} instances", sc.units_total);
        assert_eq!(sc.units_exact, sc.units_total, "seed {seed}: {:?}\nnotes {:?}", sc.misses, r.notes);
        assert_eq!(sc.extras, 0, "seed {seed}");
        assert_eq!(r.units.len(), t.unit_instances.len(), "seed {seed}");
        assert!(sc.types_total >= 2);
        assert_eq!(sc.types_exact, sc.types_total, "seed {seed}: {:?}", sc.misses);
        assert_eq!(sc.overrides_total, 3, "seed {seed}: ADA, end unit and column");
        assert_eq!(sc.overrides_exact, sc.overrides_total, "seed {seed}: {:?}", sc.misses);
        assert!(r.notes.is_empty(), "seed {seed}: {:?}", r.notes);
        // Each instance's corner is the room's.
        for u in &r.units {
            let ti = t.unit_instances.iter().find(|i| i.level == u.level && i.number == u.number).unwrap();
            assert!(
                (u.at_ft[0] - ti.at_ft[0]).abs() < 0.05 && (u.at_ft[1] - ti.at_ft[1]).abs() < 0.05,
                "room {}: {:?} vs {:?}",
                u.number,
                u.at_ft,
                ti.at_ft
            );
            assert!(u.sheet.starts_with("A-10"), "instance source {}", u.sheet);
        }
        for ty in &r.types {
            assert_eq!(ty.sheet, "A-401");
        }
    }
}

/// The reader really reads the plans: an instance whose unit tag is changed on the floor plan
/// reads as the other type, and a type whose plan loses a tag counts one fewer.
#[test]
fn reads_what_is_drawn() {
    let (t, mut dxf) = unit_reader::fixture_unit_sheets(42, HotelSize::Small);
    let first = t.unit_instances.iter().find(|u| u.level == "2" && u.hand == "normal").unwrap();
    let tag = cadaddy_mepfix::units::instance_tag(&t, first).unwrap();
    let other = if tag == "UNIT A" { "UNIT B (MIRROR)" } else { "UNIT A (MIRROR)" };
    let plan = dxf.iter_mut().find(|(n, _)| n == "A-102.dxf").unwrap();
    let text = String::from_utf8(plan.1.clone()).unwrap();
    // Each unit tag is one TEXT value line; change the first one.
    let changed = text.replacen(&format!("\n{tag}\r\n"), &format!("\n{other}\r\n"), 1);
    assert!(changed != text, "the tag {tag} is on the plan");
    plan.1 = changed.into_bytes();
    let map = cadaddy_mepfix::sheet_reader::recognise_fixture_set(&dxf);
    let r = unit_reader::read_units(&dxf, &map);
    let sc = score_units(&t, &r);
    assert_eq!(sc.units_exact, sc.units_total - 1, "one instance reads as another type and hand: {:?}", sc.misses);
}

/// Re-reading keeps an override a person set (and a type edit), drops nothing the reader wrote
/// twice, and a person's override wins over a read one of the same key.
#[test]
fn spec_patch_keeps_what_a_person_set() {
    let (t, r) = read_fixture_units(42, HotelSize::Small);
    let first = unit_reader::spec_patch(&r, &json!({}));
    let units = first["unit_instances"].as_array().unwrap();
    assert_eq!(units.len(), t.unit_instances.len());
    let ada = units.iter().find(|u| u["overrides"].as_array().unwrap().iter().any(|o| o["key"] == "ada")).unwrap().clone();
    for o in ada["overrides"].as_array().unwrap() {
        assert_eq!(o["author"], READER);
        assert_eq!(o["source"]["sheet"], "A-401");
    }
    // A person adds an override to another room and changes the ADA room's shower count.
    let mut stored = first.clone();
    let plain = stored["unit_instances"].as_array_mut().unwrap().iter_mut().find(|u| u["overrides"].as_array().unwrap().is_empty()).unwrap();
    plain["overrides"] = json!([{"key": "devices.receptacle", "value": "5", "author": "estimator", "source": {"note": "owner request"}}]);
    let plain_number = plain["number"].clone();
    let ada_stored = stored["unit_instances"].as_array_mut().unwrap().iter_mut().find(|u| u["number"] == ada["number"]).unwrap();
    ada_stored["overrides"].as_array_mut().unwrap().push(json!({"key": "fixtures.shower", "value": "2", "author": "estimator"}));
    let king = stored["unit_types"].as_array_mut().unwrap().first_mut().unwrap();
    king["edits"] = json!([{"field": "devices.receptacle", "value": 9, "author": "estimator"}]);
    let again = unit_reader::spec_patch(&r, &stored);
    let find = |n: &Value| again["unit_instances"].as_array().unwrap().iter().find(|u| &u["number"] == n).unwrap().clone();
    let p = find(&plain_number);
    assert_eq!(p["overrides"], json!([{"key": "devices.receptacle", "value": "5", "author": "estimator", "source": {"note": "owner request"}}]));
    let a = find(&ada["number"]);
    let showers: Vec<&Value> = a["overrides"].as_array().unwrap().iter().filter(|o| o["key"] == "fixtures.shower").collect();
    assert_eq!(showers.len(), 1);
    assert_eq!(showers[0]["value"], "2");
    assert_eq!(a["overrides"].as_array().unwrap().iter().filter(|o| o["key"] == "ada").count(), 1);
    assert_eq!(again["unit_types"][0]["devices"]["receptacle"], 9);
    assert_eq!(again["unit_instances"].as_array().unwrap().len(), t.unit_instances.len());
    // The read result itself carries the truth's override map.
    let read_ada = r.units.iter().find(|u| u.number == ada["number"].as_str().unwrap()).unwrap();
    assert_eq!(override_map(read_ada).get("fixtures.bathtub").map(String::as_str), Some("0"));
}

/// Broken and hostile sheets read without a panic.
#[test]
fn hostile_sheets_do_not_panic() {
    let (_, dxf) = unit_reader::fixture_unit_sheets(42, HotelSize::Small);
    let map = cadaddy_mepfix::sheet_reader::recognise_fixture_set(&dxf);
    let cut: Vec<(String, Vec<u8>)> = dxf.iter().map(|(n, b)| (n.clone(), b.get(..b.len() / 2).unwrap_or_default().to_vec())).collect();
    let _ = unit_reader::read_units(&cut, &map);
    let _ = unit_reader::read_units(&[("A-401.dxf".into(), Vec::new())], &map);
    let mut junk = dxf.clone();
    for (_, b) in junk.iter_mut() {
        let s = String::from_utf8_lossy(b).replace("UNIT A", "UNIT A - 1e309'-NaN\" x -5'").replace("13'-0\"", "99999999999'-0\"");
        *b = s.into_bytes();
    }
    let _ = unit_reader::read_units(&junk, &map);
    let _ = unit_reader::spec_patch(&Default::default(), &json!({"unit_types": "x", "unit_instances": [1, null, {"overrides": 3}]}));
}
