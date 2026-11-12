//! A person's types and finishes reach the takeoff, the BOM and the estimate
//! (fix-takeoff-ignores-user-type-and-finish), by the same source priority the schedules use
//! (the user's, then the documents', then a default): a device whose mark nothing resolved takes
//! the type a person sets on it (`bim.props.set`), else the type a person gives its mark in the
//! type catalog (`bim.set_type`, `bim.set_spec`) with that type's maker and catalog number; a
//! room takes the floor finish a person sets on it. Made-up plan only.

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// A made-up level: 6 devices of mark QA that no legend resolved, as the readers put them in the
/// spec; built.
fn unresolved_devices(s: &mut Session) {
    let tags: Vec<Value> = (0..6)
        .map(|i| {
            json!({"id": format!("device:Level 1:Q{i}"), "kind": "device", "level": "Level 1", "at_ft": [5.0 + 4.0 * f64::from(i), 10.0],
                "fields": {"type": {"value": "QA", "source": "plan"}, "device_type": {"value": "UNRESOLVED device mark QA", "source": "plan"},
                    "symbol": {"value": "", "source": "plan"}}})
        })
        .collect();
    let spec = json!({
        "units_per_foot": 12,
        "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9}],
        "tags": tags,
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": false})).expect("build");
}

fn device_handles(s: &mut Session) -> Vec<String> {
    let r = s.execute("bim.props.find", &json!({"where": {"type_mark": "QA"}, "limit": 1000})).expect("find");
    r["handles"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

/// The quantity on the lines of `item` (`net` in the BOM, `qty` in the takeoff).
fn qty(r: &Value, item: &str, key: &str) -> f64 {
    r["lines"].as_array().into_iter().flatten().filter(|l| l["item"] == item).filter_map(|l| l[key].as_f64()).sum()
}

fn line<'a>(r: &'a Value, item: &str) -> &'a Value {
    r["lines"].as_array().into_iter().flatten().find(|l| l["item"] == item).unwrap_or_else(|| panic!("no line {item}: {r}"))
}

fn items(r: &Value) -> Vec<String> {
    r["lines"].as_array().into_iter().flatten().filter_map(|l| l["item"].as_str()).map(str::to_string).collect()
}

#[test]
fn a_person_types_unresolved_devices_on_the_element_or_in_the_type_catalog() {
    let mut s = Session::new();
    unresolved_devices(&mut s);
    let devs = device_handles(&mut s);
    assert_eq!(devs.len(), 6, "the built devices");
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    assert_eq!(qty(&bom, "UNRESOLVED device mark QA", "net"), 6.0, "{bom}");

    // A person types two of them on the elements: the schedule and the takeoff agree.
    let set = s
        .execute("bim.props.set", &json!({"handles": [devs[0], devs[1]], "props": {"type": "duplex receptacle", "circuit": "LP1-1", "panel": "LP1"}}))
        .expect("props.set");
    assert_eq!(set["set"], 2, "{set}");
    let sched = s.execute("bim.schedule", &json!({"kind": "device", "by": "circuit"})).expect("schedule");
    assert!(sched.to_string().contains("duplex receptacle"), "{sched}");
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    assert_eq!((qty(&bom, "duplex receptacle", "net"), qty(&bom, "UNRESOLVED device mark QA", "net")), (2.0, 4.0), "{bom}");

    // A person gives the mark a type in the type catalog and assigns it: the other four take its
    // description, maker and catalog number (either spelling of the catalog key); the two typed
    // on the element keep the element's type.
    s.execute(
        "bim.set_type",
        &json!({"mark": "QA", "category": "receptacle",
            "fields": {"description": "Duplex receptacle 20A", "manufacturer": "Sample Maker", "catalog_number": "SM-20"}}),
    )
    .expect("set_type");
    s.execute("bim.set_spec", &json!({"handles": devs, "mark": "QA"})).expect("set_spec");
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    assert_eq!(qty(&bom, "UNRESOLVED device mark QA", "net"), 0.0, "no device is unresolved now: {:?}", items(&bom));
    assert_eq!(qty(&bom, "Duplex receptacle 20A", "net"), 4.0, "{:?}", items(&bom));
    assert_eq!(qty(&bom, "duplex receptacle", "net"), 2.0, "the element's own type wins: {:?}", items(&bom));
    let typed = line(&bom, "Duplex receptacle 20A");
    assert_eq!((typed["manufacturer"].as_str(), typed["catalog"].as_str()), (Some("Sample Maker"), Some("SM-20")), "{typed}");
    assert_eq!(typed["status"], "firm", "a person's type is firm: {typed}");

    // The takeoff and the estimate follow.
    let t = s.execute("bim.takeoff", &json!({"keep": false})).expect("takeoff");
    assert_eq!((qty(&t, "Duplex receptacle 20A", "qty"), qty(&t, "duplex receptacle", "qty")), (4.0, 2.0), "{:?}", items(&t));
    assert_eq!(qty(&t, "UNRESOLVED device mark QA", "qty"), 0.0, "{:?}", items(&t));
    let e = s.execute("bim.estimate", &json!({})).expect("estimate");
    let text = e.to_string();
    assert!(text.contains("Duplex receptacle 20A") && !text.contains("UNRESOLVED device mark QA"), "{e}");

    // A person assigns one device another mark of the catalog: it moves to that type's line.
    s.execute("bim.set_type", &json!({"mark": "QB", "category": "receptacle", "fields": {"description": "GFCI receptacle 20A"}})).expect("set_type");
    s.execute("bim.set_spec", &json!({"handles": [devs[5]], "mark": "QB"})).expect("set_spec");
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    assert_eq!((qty(&bom, "Duplex receptacle 20A", "net"), qty(&bom, "GFCI receptacle 20A", "net")), (3.0, 1.0), "{:?}", items(&bom));
    // The schedule shows the mark the person assigned.
    let sched = s.execute("bim.schedule", &json!({"kind": "device"})).expect("schedule");
    assert!(sched.to_string().contains("\"QB\""), "{sched}");
}

/// A catalog type a document gave (its fields cite the document) names an unresolved device's
/// line, but never outranks a type its legend read: only a person's type does.
#[test]
fn a_documents_catalog_type_fills_an_unresolved_device_but_not_a_legend_reading() {
    let mut s = Session::new();
    let tag = |id: &str, mark: &str, ty: &str, src: &str, x: f64| {
        json!({"id": format!("device:Level 1:{id}"), "kind": "device", "level": "Level 1", "at_ft": [x, 10.0],
            "fields": {"type": {"value": mark, "source": "plan"}, "device_type": {"value": ty, "source": src}, "symbol": {"value": "", "source": "plan"}}})
    };
    let spec = json!({
        "units_per_foot": 12,
        "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9}],
        "tags": [tag("A", "QC", "UNRESOLVED device mark QC", "plan", 5.0), tag("B", "QD", "floor box", "legend", 10.0)],
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": false})).expect("build");
    for (mark, desc) in [("QC", "Data outlet"), ("QD", "Floor box, 4 gang")] {
        s.execute(
            "bim.set_type",
            &json!({"mark": mark, "category": "device", "fields": {"description": {"value": desc, "source": "sample submittal p.3"}}}),
        )
        .expect("set_type");
    }
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    assert_eq!(qty(&bom, "Data outlet", "net"), 1.0, "{:?}", items(&bom));
    assert_eq!(qty(&bom, "floor box", "net"), 1.0, "the legend's reading stays: {:?}", items(&bom));
}

fn rect(s: &mut Session, x0: f64, y0: f64, x1: f64, y1: f64) {
    for (a, b) in [([x0, y0], [x1, y0]), ([x1, y0], [x1, y1]), ([x1, y1], [x0, y1]), ([x0, y1], [x0, y0])] {
        s.execute("line", &json!({"points": [a, b]})).expect("line");
    }
}

/// A 40' × 30' made-up floor (inches): 6" outside walls and a partition at 20', two tagged rooms.
fn two_rooms() -> Session {
    let mut s = Session::new();
    s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).expect("layer");
    s.execute("layer.new", &json!({"name": "A-AREA-IDEN"})).expect("layer");
    rect(&mut s, 0.0, 0.0, 480.0, 360.0);
    rect(&mut s, 6.0, 6.0, 474.0, 354.0);
    for x in [238.0, 242.0] {
        s.execute("line", &json!({"points": [[x, 6.0], [x, 354.0]]})).expect("line");
    }
    s.execute("layer.current", &json!({"name": "A-AREA-IDEN"})).expect("layer");
    for (at, t) in [([100.0, 220.0], "OFFICE"), ([100.0, 190.0], "101"), ([340.0, 220.0], "STORAGE"), ([340.0, 190.0], "102")] {
        s.execute("text", &json!({"at": at, "text": t, "height": 6})).expect("text");
    }
    let levels = json!([{"name": "Level 1", "elevation_ft": 0.0, "height_ft": 10.0, "ceiling_ft": 9.0, "region": [-200.0, -200.0, 800.0, 600.0]}]);
    s.execute("bspec.draft", &json!({"store": true, "levels": levels})).expect("draft");
    let r = s.execute("model.build", &json!({})).expect("build");
    assert_eq!(r["rooms"], 2, "{r}");
    s
}

fn room_handle(s: &mut Session, number: &str) -> String {
    let r = s.execute("bim.props.find", &json!({"kind": "room", "where": {"number": number}})).expect("find");
    r["handles"][0].as_str().unwrap_or_else(|| panic!("room {number}: {r}")).to_string()
}

/// The floor area on the lines of finish `f` (a room floored as pieces is on its flooring line,
/// one without on its room's floor finish line).
fn floor_sf(r: &Value, f: Option<&str>, key: &str) -> f64 {
    match f {
        Some(f) => qty(r, &format!("flooring {f}"), key) + qty(r, &format!("Floor finish {f}"), key),
        None => qty(r, "flooring (no finish given)", key) + qty(r, "Floor finish (not given)", key),
    }
}

#[test]
fn a_person_set_floor_finish_is_the_rooms_takeoff_line() {
    let mut s = two_rooms();
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    let all = floor_sf(&bom, None, "net");
    assert!(all > 500.0, "both rooms' floors, no finish given: {:?}", items(&bom));
    let office = room_handle(&mut s, "101");
    s.execute("bim.props.set", &json!({"handle": office, "props": {"floor_finish": "VCT"}})).expect("props.set");
    let sched = s.execute("bim.schedule", &json!({"kind": "room_finish"})).expect("schedule");
    assert!(sched.to_string().contains("VCT"), "{sched}");

    let one = 232.0 * 348.0 / 144.0;
    let check = |s: &mut Session, when: &str| {
        let bom = s.execute("bim.bom", &json!({})).expect("bom");
        let vct = floor_sf(&bom, Some("VCT"), "net");
        assert!((vct - one).abs() < 2.0, "{when}: the office's floor is VCT: {vct} {:?}", items(&bom));
        assert!((floor_sf(&bom, None, "net") + vct - all).abs() < 0.5, "{when}: the rest stays not given: {:?}", items(&bom));
        let t = s.execute("bim.takeoff", &json!({"keep": false})).expect("takeoff");
        assert!((floor_sf(&t, Some("VCT"), "qty") - vct).abs() < 0.5, "{when}: {:?}", items(&t));
        let e = s.execute("bim.estimate", &json!({})).expect("estimate");
        assert!(e.to_string().contains("VCT"), "{when}: {e}");
    };
    check(&mut s, "after the edit");
    // The person's finish is kept through a rebuild, and the flooring is laid as it.
    s.execute("model.build", &json!({"force": true})).expect("rebuild");
    let office_now = room_handle(&mut s, "101");
    let got = s.execute("bim.props.get", &json!({"handle": office_now})).expect("get");
    assert_eq!(got["elements"][0]["props"]["floor_finish"]["value"], "VCT", "{got}");
    check(&mut s, "after a rebuild");
    let pieces = s.execute("bim.count", &json!({"trade": "finishes", "kind": "flooring", "group_by": "type"})).expect("count");
    assert!(pieces.to_string().contains("VCT"), "the rebuilt flooring pieces are VCT: {pieces}");
}
