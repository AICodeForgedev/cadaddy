//! Q6353 raceway supports: raceways run side by side go on rod-hung strut trapezes (strut, a
//! strut strap per raceway, threaded rod, anchors, nuts) in place of their own straps, a raceway
//! run alone keeps its straps, and a riser gets a riser clamp at the floor it passes. Each is a
//! counted model piece and a takeoff line under the raceway supports.

use cadaddy_engine::Session;
use cadaddy_engine::doc::Drawing;
use serde_json::{Value, json};

/// A made-up two-storey building (12 ft storeys): three 1" EMT runs 4" apart centre to centre,
/// 40 ft along x at 10 ft; a fourth 40 ft run alone 50 ft away; and a 2" riser from 4 ft up to
/// the floor above. The build's report beside it.
fn building() -> (Session, Value) {
    let mut s = Session::empty();
    s.open_drawing(Drawing::new_imperial(), "Sample Floor Plan", None);
    let bank = |y: f64| json!({"system": "E-COND", "level": "L1", "points": [[0, y], [480, y]], "size_in": [1, 0], "bottoms_ft": [10], "size_source": "tag", "height_source": "tag"});
    let spec = json!({
        "version": 1, "units_per_foot": 12,
        "levels": [
            {"name": "L1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]},
            {"name": "L2", "elevation_ft": 12, "height_ft": 12, "ceiling_ft": 9, "base": [0, 0]}
        ],
        "mep": {
            "systems": [
                {"name": "E-COND", "layer": "E-COND", "kind": "conduit", "size_in": [1, 0], "bottom_ft": 10}
            ],
            "runs": [
                bank(600.0), bank(604.0), bank(608.0),
                {"system": "E-COND", "level": "L1", "points": [[0, 1200], [480, 1200]], "size_in": [1, 0], "bottoms_ft": [10], "size_source": "tag", "height_source": "tag"},
                {"system": "E-COND", "level": "L1", "points": [[700, 300]], "size_in": [2, 0], "bottoms_ft": [4], "top_ft": 12, "size_source": "tag", "height_source": "tag"}
            ]
        }
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let built = s.execute("model.build", &json!({"route": false})).expect("build");
    assert!(built["atoms"].as_u64().unwrap_or(0) > 0, "{built}");
    (s, built)
}

fn count(s: &mut Session, q: Value) -> Value {
    s.execute("bim.count", &q).expect("count")
}

/// Every model piece of `kind`: its properties as text.
fn pieces(s: &Session, kind: &str) -> Vec<cadaddy_engine::doc::props::Props> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .map(|e| cadaddy_engine::doc::props::read(&e.common.xdata))
        .filter(|p| p.get("atom").map(|x| x.text()).as_deref() == Some(kind))
        .collect()
}

fn text(p: &cadaddy_engine::doc::props::Props, k: &str) -> String {
    p.get(k).map(|x| x.text()).unwrap_or_default()
}

#[test]
fn raceways_side_by_side_hang_on_strut_trapezes_and_a_lone_run_keeps_its_straps() {
    let (mut s, built) = building();
    let sup = &built["supports"];
    // 40 ft together, supports at most 10 ft apart (NEC 358.30) and one at each end: five.
    assert_eq!(sup["trapezes"], json!(5), "{sup}");
    assert_eq!(count(&mut s, json!({"kind": "strut"}))["count"], json!(5), "{sup}");
    assert_eq!(count(&mut s, json!({"kind": "strut_strap", "size": "1\""}))["count"], json!(15), "one per raceway per support: {sup}");
    assert_eq!(count(&mut s, json!({"kind": "rod"}))["count"], json!(10), "two rods per trapeze: {sup}");
    assert_eq!(count(&mut s, json!({"kind": "support_anchor"}))["count"], json!(10), "an anchor per rod: {sup}");
    assert_eq!(count(&mut s, json!({"kind": "strut_nut"}))["count"], json!(20), "two nuts per rod: {sup}");
    // The struts and rods are counted in feet.
    let strut_ft = count(&mut s, json!({"kind": "strut"}))["length_ft"].as_f64().unwrap_or(0.0);
    assert!((strut_ft - 5.0 * 15.0 / 12.0).abs() < 0.05, "five 15 in struts: {strut_ft}");
    let rod_ft = count(&mut s, json!({"kind": "rod"}))["length_ft"].as_f64().unwrap_or(0.0);
    assert!(rod_ft > 5.0, "each rod from the strut up to the structure: {rod_ft}");
    // The bank's own straps are gone; the lone run keeps its five (within 3 ft of each end, at
    // most 10 ft apart).
    assert_eq!(sup["strapsReplaced"], json!(15), "{sup}");
    let straps = count(&mut s, json!({"kind": "strap", "size": "1\""}))["count"].as_u64().unwrap_or(0);
    assert_eq!(straps, 5, "{sup}");
    assert_eq!(sup["byKind"]["strap"].as_u64(), Some(count(&mut s, json!({"kind": "strap"}))["count"].as_u64().unwrap_or(0)), "{sup}");
    // Each strut strap holds its stick and is on its strut; each piece is inferred, with its rule.
    let d = s.doc().expect("doc");
    let kind_of = |h: &str| {
        d.model
            .iter()
            .find(|e| e.handle.hex().eq_ignore_ascii_case(h))
            .map(|e| text(&cadaddy_engine::doc::props::read(&e.common.xdata), "atom"))
            .unwrap_or_default()
    };
    for p in pieces(&s, "strut_strap") {
        let cn: Vec<String> = text(&p, "connects").split(',').map(|h| kind_of(h.trim())).collect();
        assert!(cn.iter().any(|k| k == "conduit") && cn.iter().any(|k| k == "strut"), "{cn:?}");
        assert!(!text(&p, "run").is_empty(), "a strut strap is its raceway's piece");
    }
    for p in pieces(&s, "strut") {
        assert!(text(&p, "source_rule").contains("strut channel"), "{p:?}");
    }
    assert_eq!(count(&mut s, json!({"kind": "strut", "made": "inferred"}))["count"], json!(5), "built by rule, never passing for drawn");
}

#[test]
fn a_riser_through_the_floor_takes_a_riser_clamp() {
    let (mut s, built) = building();
    assert_eq!(built["supports"]["riserClamps"], json!(1), "{}", built["supports"]);
    let c = count(&mut s, json!({"kind": "riser_clamp", "size": "2\""}));
    assert_eq!(c["count"], json!(1), "{c}");
    // At the floor above, holding the riser.
    let p = pieces(&s, "riser_clamp");
    let clamp = p.first().expect("clamp");
    let z: f64 = text(clamp, "elevation").parse().unwrap_or(0.0);
    assert!(z > 11.5 && z < 12.0, "on the floor at 12 ft: {z}");
    assert!(!text(clamp, "connects").is_empty(), "{clamp:?}");
}

#[test]
fn the_takeoff_lists_the_supports() {
    let (mut s, _) = building();
    let bom = s.execute("bim.bom", &json!({"group_by": "job"})).expect("bom");
    let lines: Vec<&Value> = bom["lines"].as_array().expect("lines").iter().filter(|l| l["category"] == "raceway supports").collect();
    let line = |item: &str| lines.iter().find(|l| l["item"].as_str().is_some_and(|i| i.contains(item))).copied();
    let strut = line("Strut channel").unwrap_or_else(|| panic!("a strut line: {lines:?}"));
    assert_eq!(strut["unit"], json!("ft"));
    assert!(strut["net"].as_f64().unwrap_or(0.0) > 6.0, "{strut}");
    assert_eq!(line("Threaded rod").map(|l| l["unit"].clone()), Some(json!("ft")), "{lines:?}");
    assert_eq!(line("Strut strap").map(|l| l["net"].clone()), Some(json!(15.0)), "{lines:?}");
    assert_eq!(line("Riser clamp").map(|l| l["net"].clone()), Some(json!(1.0)), "{lines:?}");
    assert!(line("anchor").is_some() && line("nut").is_some(), "{lines:?}");
    // Every support line lists its model elements.
    assert!(lines.iter().all(|l| l["elements"].as_array().is_some_and(|e| !e.is_empty())), "{lines:?}");
}
