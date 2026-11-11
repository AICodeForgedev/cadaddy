//! Q6529: the mechanical and plumbing lines of a built model name the objects they count:
//! duct and pipe lines their runs' segments, fitting lines the fittings of their kind,
//! terminal lines the terminals of their kind, hanger lines the runs they are spaced along.
//! `bim.takeoff.show` selects them. Made-up model only (the `mep.qty` fixture).

use cadaddy_engine::Session;
use cadaddy_engine::doc::Handle;
use serde_json::{Value, json};

use super::mepqty::model;

fn ids(l: &Value) -> Vec<String> {
    l["elements"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

/// An element's build part (`segment`, `fitting:elbow`, `terminal:diffuser`).
fn part(s: &mut Session, h: &str) -> String {
    let d = s.doc().expect("doc");
    let e = Handle::parse_hex(h).and_then(|x| d.entity(x)).unwrap_or_else(|| panic!("{h} is not an object"));
    let x = &e.common.xdata;
    x.iter().find_map(|t| t.value.strip_prefix("part:")).unwrap_or_default().to_string()
}

#[test]
fn mechanical_takeoff_lines_list_the_objects_they_count() {
    let mut s = model();
    s.execute("model.build", &json!({})).expect("build");
    let t = s.execute("bim.takeoff", &json!({})).expect("takeoff");
    let lines: Vec<Value> = t["lines"].as_array().expect("lines").iter().filter(|l| l["trade"] != "electrical").cloned().collect();
    for cat in ["duct", "pipe", "fittings", "terminals", "hangers"] {
        let of: Vec<&Value> = lines.iter().filter(|l| l["category"] == cat).collect();
        assert!(!of.is_empty(), "a {cat} line: {t}");
        for l in of {
            let hs = ids(l);
            assert!(!hs.is_empty(), "{cat} line lists no elements: {l}");
            for h in &hs {
                let p = part(&mut s, h);
                let item = l["item"].as_str().unwrap_or_default();
                match cat {
                    // A fitting line: fittings of the kind its item names (a pipe's offset at a
                    // corner is built as its drop); the build made the main's size change at its
                    // end its elbow, so the transition's line lists the system's fittings there.
                    "fittings" => {
                        let kind = p.strip_prefix("fitting:").unwrap_or_else(|| panic!("{h} ({p}) on {l}"));
                        let kind = if kind == "drop" && !item.contains("drop") { "offset" } else { kind };
                        assert!(item.contains(kind) || item == "SA transition", "{h} is a {kind}, the line {item}");
                    }
                    "terminals" => assert_eq!(p, format!("terminal:{item}"), "{l}"),
                    _ => assert_eq!(p, "segment", "{h} on {l}"),
                }
            }
        }
    }
    // takeoff.show selects a fitting line's objects.
    let l = lines.iter().find(|l| l["category"] == "fittings").expect("fittings");
    let shown = s.execute("bim.takeoff.show", &json!({"line": l["key"]})).expect("show");
    assert_eq!(shown["selected"], l["element_count"], "{shown}");
}
