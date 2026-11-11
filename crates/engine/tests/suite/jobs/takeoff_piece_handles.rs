//! Q6529: every takeoff and BOM line names the objects it counts. The lines counted from the
//! model's piece index (boxes, conduit fittings, wire, breakers) list their pieces' handles, and
//! the BOM's elements are object handles, never a spec device's id or a build name, so
//! `bim.takeoff.show` and `bim.props.get` resolve every line. Made-up building only.

use std::collections::BTreeSet;

use cadaddy_engine::Session;
use cadaddy_engine::doc::Handle;
use serde_json::{Value, json};

use super::mepatoms::building;

fn ids(l: &Value) -> Vec<String> {
    l["elements"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

fn is_handle(s: &mut Session, h: &str) -> bool {
    let d = s.doc().expect("doc");
    Handle::parse_hex(h).is_some_and(|x| d.entity(x).is_some())
}

/// The model's pieces of an atom kind, by handle.
fn pieces(s: &mut Session, kind: &str) -> BTreeSet<String> {
    let d = s.doc().expect("doc");
    d.model
        .iter()
        .filter(|e| cadaddy_engine::doc::props::read(&e.common.xdata).get("atom").map(|p| p.text()).as_deref() == Some(kind))
        .map(|e| e.handle.hex())
        .collect()
}

#[test]
fn takeoff_lines_counted_from_the_piece_index_list_their_pieces() {
    let mut s = building(None);
    let t = s.execute("bim.takeoff", &json!({})).expect("takeoff");
    let lines = t["lines"].as_array().expect("lines").clone();
    let of = |cat: &str| -> Vec<&Value> { lines.iter().filter(|l| l["category"] == cat).collect() };
    for cat in ["boxes", "conduit fittings", "wire", "conduit"] {
        assert!(!of(cat).is_empty(), "a {cat} line: {t}");
        for l in of(cat) {
            assert!(l["element_count"].as_u64().unwrap_or(0) > 0, "{cat} line lists no elements: {l}");
            for h in ids(l) {
                assert!(is_handle(&mut s, &h), "{h} on {l} is not an object handle");
            }
        }
    }
    // The box lines counted in the model hold exactly the model's boxes (a plate's line, by
    // rule, lists the device it is for); every coupling and connector is on a fittings line.
    let counted = |l: &&&Value| l["status"] != "rule" || l["item"].as_str().is_some_and(|i| i.starts_with("Box"));
    let boxes: BTreeSet<String> = of("boxes").iter().filter(counted).flat_map(|l| ids(l)).collect();
    assert_eq!(boxes, pieces(&mut s, "box"), "{t}");
    // Every line lists elements, rule-made ones what their rule counted from (a plate: its
    // device; hangers: the runs they are spaced along).
    for l in &lines {
        assert!(l["element_count"].as_u64().unwrap_or(0) > 0, "a line lists no elements: {l}");
    }
    let fittings: BTreeSet<String> = of("conduit fittings").iter().flat_map(|l| ids(l)).collect();
    let want: BTreeSet<String> = pieces(&mut s, "coupling").into_iter().chain(pieces(&mut s, "connector")).collect();
    assert!(want.is_subset(&fittings), "{want:?} not all on {fittings:?}");
    // The wire lines hold the wire pieces.
    let wire: BTreeSet<String> = of("wire").iter().flat_map(|l| ids(l)).collect();
    assert_eq!(wire, pieces(&mut s, "wire"), "{t}");
    // takeoff.show selects a fittings line's elements.
    let l = of("conduit fittings").into_iter().next().expect("fittings line");
    let shown = s.execute("bim.takeoff.show", &json!({"line": l["key"]})).expect("show");
    assert_eq!(shown["selected"], l["element_count"], "{shown}");
}

#[test]
fn bom_elements_are_object_handles() {
    let mut s = building(None);
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    let lines = bom["lines"].as_array().expect("lines").clone();
    let mut devices = 0;
    for l in &lines {
        for h in ids(l) {
            assert!(is_handle(&mut s, &h), "{h} on {l} is not an object handle");
            s.execute("bim.props.get", &json!({"handles": [h]})).unwrap_or_else(|e| panic!("props.get {h}: {e}"));
        }
        if l["category"] == "devices" {
            devices += ids(l).len();
        }
    }
    assert!(devices > 0, "the receptacle's device line lists it: {bom}");
    // The box line counted from the pieces lists them; the device's own ids stay beside its
    // handle, and every line lists its elements.
    let boxes: BTreeSet<String> =
        lines.iter().filter(|l| l["category"] == "boxes" && l["item"].as_str().is_some_and(|i| i.starts_with("Box"))).flat_map(ids).collect();
    assert_eq!(boxes, pieces(&mut s, "box"), "{bom}");
    let dev = lines.iter().find(|l| l["category"] == "devices").expect("device line");
    assert_eq!(dev["sources"], json!(["device:L1:20.7,10.0"]), "{dev}");
    for l in &lines {
        assert!(!ids(l).is_empty() && l["element_count"].as_u64() == Some(ids(l).len() as u64), "{l}");
        assert!(l.get("elements_unresolved").is_none(), "{l}");
    }
}

/// A wall mark's lines (length and area) list the built walls of its type on its level, not
/// the job's first walls.
#[test]
fn a_wall_marks_lines_list_the_walls_of_its_type() {
    let mut s = super::drywall_views::building();
    let bom = s.execute("bim.bom", &json!({})).expect("bom");
    let type_of = |s: &mut Session, h: &str| -> String {
        let d = s.doc().expect("doc");
        let e = Handle::parse_hex(h).and_then(|x| d.entity(x)).unwrap_or_else(|| panic!("{h}"));
        e.common.xdata.iter().find_map(|t| t.value.strip_prefix("type:")).unwrap_or_default().to_string()
    };
    for item in ["Wall P1", "Wall area P1"] {
        let l = bom["lines"].as_array().expect("lines").iter().find(|l| l["item"] == item).unwrap_or_else(|| panic!("no {item}: {bom}")).clone();
        let hs = ids(&l);
        assert!(!hs.is_empty(), "{item} lists no walls: {l}");
        for h in &hs {
            assert_eq!(type_of(&mut s, h), "P1", "{h} on {l}");
        }
    }
}
