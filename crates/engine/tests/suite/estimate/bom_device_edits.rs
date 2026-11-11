//! `bim.bom` and `bim.takeoff` device lines follow edits to the device elements
//! (fix-bom-device-lines-ignore-element-edits): an erased device is off its line, a copied one
//! is on it, a moved one stays, a person's type edit moves it to its new type's line, and each
//! line lists exactly the element handles it counts (the same set `bim.count` sees). Made-up
//! plan only.

use std::collections::BTreeSet;

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// A made-up level: 10 duplex receptacles of mark RA and 4 luminaires of type LA, as the
/// readers put device tags in the spec; built.
fn built(s: &mut Session) {
    let tag = |id: String, kind: &str, mark: &str, ty: Option<&str>, x: f64, y: f64| {
        let mut fields = json!({"type": {"value": mark, "source": "plan"}, "symbol": {"value": "", "source": "plan"}});
        if let Some(t) = ty {
            fields["device_type"] = json!({"value": t, "source": "legend"});
        }
        json!({"id": id, "kind": kind, "level": "Level 1", "at_ft": [x, y], "fields": fields})
    };
    let mut tags: Vec<Value> =
        (0..10).map(|i| tag(format!("device:Level 1:R{i}"), "receptacle", "RA", Some("duplex receptacle"), 5.0 + 4.0 * f64::from(i), 10.0)).collect();
    tags.extend((0..4).map(|i| tag(format!("device:Level 1:L{i}"), "luminaire", "LA", None, 8.0 + 8.0 * f64::from(i), 25.0)));
    let spec = json!({
        "units_per_foot": 12,
        "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9}],
        "tags": tags,
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    s.execute("model.build", &json!({"route": false})).expect("build");
}

/// The built elements of a type mark, by handle.
fn of_mark(s: &mut Session, mark: &str) -> Vec<String> {
    let r = s.execute("bim.props.find", &json!({"where": {"type_mark": mark}, "limit": 1000})).expect("find");
    r["handles"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

fn count(s: &mut Session, mark: &str) -> u64 {
    let r = s.execute("bim.count", &json!({"type": mark})).expect("count");
    r["count"].as_u64().unwrap_or_else(|| panic!("count: {r}"))
}

fn handles(l: &Value) -> Vec<String> {
    l["elements"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

/// A BOM's (or takeoff's) lines of one item: their quantity and their elements.
fn item(r: &Value, item: &str, qty: &str) -> (f64, Vec<String>) {
    let lines: Vec<&Value> = r["lines"].as_array().into_iter().flatten().filter(|l| l["item"] == item).collect();
    (lines.iter().filter_map(|l| l[qty].as_f64()).sum(), lines.iter().flat_map(|l| handles(l)).collect())
}

#[test]
fn bom_and_takeoff_device_lines_follow_erase_copy_move_and_type_edits() {
    let mut s = Session::new();
    built(&mut s);
    let lum = of_mark(&mut s, "LA");
    let rec = of_mark(&mut s, "RA");
    assert_eq!((lum.len(), rec.len()), (4, 10), "the built devices");
    let bom = s.execute("bim.bom", &json!({"all_elements": true})).expect("bom");
    assert_eq!(item(&bom, "Luminaire type LA", "net").0, 4.0, "{bom}");
    assert_eq!(item(&bom, "duplex receptacle", "net").0, 10.0, "{bom}");

    // Erase two luminaires, copy three receptacles 10' over, move one luminaire, and a person
    // sets one receptacle's type.
    s.execute("erase", &json!({"handles": [lum[0], lum[1]]})).expect("erase");
    s.execute("copy", &json!({"handles": [rec[0], rec[1], rec[2]], "delta": [120.0, 0.0]})).expect("copy");
    s.execute("3dmove", &json!({"handles": [lum[2]], "delta": [24.0, 0.0, 0.0]})).expect("3dmove");
    s.execute("bim.props.set", &json!({"handle": rec[9], "props": {"type_mark": "RB", "type": "GFCI receptacle"}})).expect("props.set");

    // The model's elements: 2 luminaires; 12 RA receptacles (10 + 3 copies - 1 retyped), 1 RB.
    let (lum_now, ra_now, rb_now) = (of_mark(&mut s, "LA"), of_mark(&mut s, "RA"), of_mark(&mut s, "RB"));
    assert_eq!((lum_now.len(), ra_now.len(), rb_now.len()), (2, 12, 1), "the model after the edits");
    assert_eq!((count(&mut s, "LA"), count(&mut s, "RA"), count(&mut s, "RB")), (2, 12, 1), "bim.count after the edits");
    let copies: Vec<&String> = ra_now.iter().filter(|h| !rec.contains(h)).collect();
    assert_eq!(copies.len(), 3, "{ra_now:?}");

    // The BOM counts those elements, each line listing exactly the handles it counts.
    let bom = s.execute("bim.bom", &json!({"all_elements": true})).expect("bom");
    let (n, el) = item(&bom, "Luminaire type LA", "net");
    assert_eq!(n, 2.0, "erased luminaires are off the BOM: {bom}");
    assert_eq!(el.iter().collect::<BTreeSet<_>>(), lum_now.iter().collect::<BTreeSet<_>>(), "its elements are the luminaires standing");
    let (n, el) = item(&bom, "duplex receptacle", "net");
    assert_eq!(n, 12.0, "copies are on the BOM, the retyped one is not: {bom}");
    assert_eq!(el.iter().collect::<BTreeSet<_>>(), ra_now.iter().collect::<BTreeSet<_>>());
    let (n, el) = item(&bom, "GFCI receptacle", "net");
    assert_eq!((n, el), (1.0, rb_now.clone()), "a person's type edit moves it to its type's line: {bom}");
    // Every device id the BOM lists is an object handle of the model.
    for h in
        bom["lines"].as_array().into_iter().flatten().filter(|l| matches!(l["category"].as_str(), Some("devices" | "fixtures"))).flat_map(handles)
    {
        s.execute("bim.props.get", &json!({"handle": h})).unwrap_or_else(|e| panic!("BOM element {h} is no element: {e}"));
    }

    // The takeoff: the same quantities, each line's elements exactly the ones counted on it.
    let t = s.execute("bim.takeoff", &json!({"keep": false})).expect("takeoff");
    for (name, want) in [("Luminaire type LA", &lum_now), ("duplex receptacle", &ra_now), ("GFCI receptacle", &rb_now)] {
        let (q, el) = item(&t, name, "qty");
        assert_eq!(q, want.len() as f64, "{name}: {t}");
        assert_eq!(el.iter().collect::<BTreeSet<_>>(), want.iter().collect::<BTreeSet<_>>(), "{name}'s elements: {t}");
    }
    assert_eq!(t["devices"]["not_on_a_line"], 0, "every built device on a line: {}", t["devices"]);
}

/// A model whose spec tags were never built as elements (a takeoff from the readers alone)
/// still counts its tags: only a built device's line follows the elements.
#[test]
fn unbuilt_device_tags_still_count() {
    let mut s = Session::new();
    let spec = json!({
        "units_per_foot": 12,
        "levels": [{"name": "Level 1", "elevation_ft": 0, "height_ft": 12, "ceiling_ft": 9}],
        "tags": [
            {"id": "device:Level 1:A", "kind": "receptacle", "level": "Level 1", "at_ft": [1.0, 1.0],
                "fields": {"type": {"value": "RA", "source": "plan"}, "device_type": {"value": "duplex receptacle", "source": "legend"}, "symbol": {"value": "", "source": "plan"}}},
            {"id": "device:Level 1:B", "kind": "receptacle", "level": "Level 1", "at_ft": [5.0, 1.0],
                "fields": {"type": {"value": "RA", "source": "plan"}, "device_type": {"value": "duplex receptacle", "source": "legend"}, "symbol": {"value": "", "source": "plan"}}},
        ],
    });
    s.execute("bspec.set", &json!({"replace": true, "spec": spec})).expect("spec");
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    assert_eq!(item(&bom, "duplex receptacle", "net").0, 2.0, "{bom}");
}
