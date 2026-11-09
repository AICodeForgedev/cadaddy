//! `bim.takeoff`'s device lines (Q6373): every built device is on a line by its type, a line's
//! elements are exactly the devices counted on it (`bim.takeoff.show` selects those), another
//! trade's devices are on their own "by others" lines (out of our totals), and that holds when
//! the drawing's print linework comes before the built model. Made-up plan only.

use std::collections::BTreeSet;

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// A made-up level: 3 receptacles of mark RA, 2 of mark RB and a thermostat, as device tags.
fn spec(s: &mut Session) {
    let dev = |id: &str, kind: &str, ty: &str, mark: &str, x: f64, y: f64| {
        json!({"id": format!("device:Level 1:{id}"), "kind": kind, "level": "Level 1", "at_ft": [x, y],
            "fields": {"type": {"value": mark, "source": "plan"}, "device_type": {"value": ty, "source": "legend"},
                "symbol": {"value": "", "source": "plan"}}})
    };
    let spec = json!({
        "units_per_foot": 12,
        "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9}],
        "tags": [
            dev("A1", "receptacle", "duplex receptacle", "RA", 10.0, 10.0),
            dev("A2", "receptacle", "duplex receptacle", "RA", 20.0, 10.0),
            dev("A3", "receptacle", "duplex receptacle", "RA", 30.0, 10.0),
            dev("B1", "receptacle", "GFCI receptacle", "RB", 10.0, 30.0),
            dev("B2", "receptacle", "GFCI receptacle", "RB", 20.0, 30.0),
            dev("T1", "thermostat", "thermostat", "T", 30.0, 30.0),
        ]
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    // The thermostat is the mechanical contractor's (a person's decision).
    s.execute(
        "bim.scope",
        &json!({"sheets": false, "set": [{"class": "thermostat", "party": "mechanical contractor", "ours": "none", "reason": "test: the mechanical contractor's"}]}),
    )
    .expect("scope");
    s.execute("model.build", &json!({"route": false})).expect("build");
}

fn device_lines(t: &Value) -> Vec<&Value> {
    t["lines"].as_array().into_iter().flatten().filter(|l| l["category"] == "devices").collect()
}

fn handles(l: &Value) -> BTreeSet<String> {
    l["elements"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

fn line<'a>(t: &'a Value, item: &str) -> &'a Value {
    device_lines(t).into_iter().find(|l| l["item"] == item).unwrap_or_else(|| panic!("no device line {item}: {t}"))
}

/// The device schedule counts the six built devices.
fn six_built(s: &mut Session) {
    let r = s.execute("bim.schedule", &json!({"kind": "device"})).expect("schedule");
    let n: u64 = r["rows"].as_array().into_iter().flatten().filter_map(|r| r["count"].as_u64().or_else(|| r["count"].as_str()?.parse().ok())).sum();
    assert_eq!(n, 6, "six devices built: {r}");
}

fn check(s: &mut Session) {
    let t = s.execute("bim.takeoff", &json!({})).expect("takeoff");
    let ra = line(&t, "duplex receptacle");
    let rb = line(&t, "GFCI receptacle");
    // Each line's elements are its own devices: as many as it counts, none of the other line's.
    assert_eq!((ra["qty"].as_f64(), ra["element_count"].as_u64()), (Some(3.0), Some(3)), "{ra}");
    assert_eq!((rb["qty"].as_f64(), rb["element_count"].as_u64()), (Some(2.0), Some(2)), "{rb}");
    assert!(handles(ra).is_disjoint(&handles(rb)), "{ra}\n{rb}");
    // The thermostat is another trade's: on its own line, by others, out of our totals.
    let th = line(&t, "thermostat");
    assert_eq!((th["qty"].as_f64(), th["element_count"].as_u64()), (Some(1.0), Some(1)), "{th}");
    assert_eq!(th["status"], "by others", "{th}");
    assert!(th["scope"].as_str().unwrap_or_default().contains("mechanical"), "{th}");
    assert!(handles(th).is_disjoint(&handles(ra)) && handles(th).is_disjoint(&handles(rb)));
    // Every built device is on a line, once.
    let on: Vec<String> = device_lines(&t).iter().flat_map(|l| handles(l)).collect();
    let uniq: BTreeSet<&String> = on.iter().collect();
    assert_eq!((on.len(), uniq.len()), (6, 6), "{t}");
    assert_eq!(t["devices"]["built"], 6, "{}", t["devices"]);
    assert_eq!(t["devices"]["on_lines"], 6, "{}", t["devices"]);
    assert_eq!(t["devices"]["by_others"], 1, "{}", t["devices"]);
    assert_eq!(t["devices"]["not_on_a_line"], 0, "{}", t["devices"]);
    // Our count totals leave the other trade's thermostat out.
    let ours: f64 = t["lines"].as_array().unwrap().iter().filter(|l| l["status"] != "by others").filter_map(|l| l["count"].as_f64()).sum();
    assert!((t["totals"]["count"].as_f64().unwrap() - ours).abs() < 1e-6, "{}", t["totals"]);
    assert_eq!(t["totals"]["by_others"]["count"].as_f64(), Some(1.0), "{}", t["totals"]);
    // takeoff.show selects exactly a line's elements.
    let key = ra["key"].as_str().unwrap();
    let shown = s.execute("bim.takeoff.show", &json!({"line": key})).expect("show");
    assert_eq!(shown["selected"], 3, "{shown}");
    // The estimate never prices the other trade's line.
    let e = s.execute("bim.estimate", &json!({"rows": [{"item": "thermostat", "unit": "each", "material": 50, "labor_hrs": 1}]})).expect("estimate");
    assert!(!e["lines"].as_array().unwrap().iter().any(|l| l["item"] == "thermostat"), "{e}");
}

#[test]
fn each_device_line_carries_only_its_own_devices_and_other_trades_are_by_others() {
    let mut s = Session::new();
    spec(&mut s);
    six_built(&mut s);
    check(&mut s);
}

/// A print set's linework comes before the built model (a large job has well over a million
/// entities): the device lines still find their own devices.
#[test]
fn device_lines_find_their_devices_behind_a_large_print_set() {
    let mut s = Session::new();
    {
        let d = s.doc_mut().expect("doc");
        let space = cadaddy_doc::Space::Model;
        for i in 0..520_000u32 {
            let x = f64::from(i % 1000);
            let y = f64::from(i / 1000);
            let kind = cadaddy_doc::EntityKind::Line(cadaddy_doc::Line {
                a: cadaddy_geom::Vec3::new(x, y + 2000.0, 0.0),
                b: cadaddy_geom::Vec3::new(x + 0.5, y + 2000.0, 0.0),
            });
            d.add(&space, cadaddy_doc::Common::default(), kind).expect("linework");
        }
    }
    spec(&mut s);
    six_built(&mut s);
    check(&mut s);
}
