//! The Purge dialog's Find Non-Purgeable Items (PURGE `{list, reasons}`): why each item stays,
//! with the objects that hold it; and Purge nested items.

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn held<'a>(list: &'a Value, kind: &str, name: &str) -> &'a Vec<Value> {
    list["nonPurgeable"][kind]
        .as_array()
        .and_then(|a| a.iter().find(|i| i["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name))))
        .and_then(|i| i["reasons"].as_array())
        .unwrap_or_else(|| panic!("{name} is not listed as non-purgeable: {list}"))
}

fn texts(r: &[Value]) -> Vec<String> {
    r.iter().map(|x| x["text"].as_str().unwrap_or_default().to_string()).collect()
}

#[test]
fn a_layer_used_only_inside_a_block_names_the_block_and_its_references() {
    let mut s = Session::new();
    s.execute("layer.new", &json!({ "name": "Hidden", "current": true })).unwrap();
    s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    s.execute("layer.current", &json!({ "name": "0" })).unwrap();
    s.execute("selectall", &json!({})).unwrap();
    s.execute("block", &json!({ "name": "Door", "base": [0, 0], "keep": "delete" })).unwrap();
    let ins = s.execute("insert", &json!({ "name": "Door", "at": [5, 5] })).unwrap();
    let list = s.execute("purge", &json!({ "list": true, "reasons": true })).unwrap();
    assert!(!list["layers"].as_array().unwrap().iter().any(|n| n == "Hidden"));
    let why = held(&list, "layers", "Hidden");
    assert_eq!(texts(why), vec!["Used in the definition of block Door"]);
    let h = ins["handle"].as_str().map(str::to_string).unwrap_or_default();
    assert_eq!(why[0]["handles"], json!([h]), "zoom to the reference that shows it");
    // Layer 0: kept, current, and used by the reference itself.
    let zero = texts(held(&list, "layers", "0"));
    assert!(zero.contains(&"Layer 0 is always kept".to_string()) && zero.contains(&"The current layer".to_string()), "{zero:?}");
    assert!(zero.iter().any(|t| t == "Used by a Block Reference object on layer 0 in Model"), "{zero:?}");
    // The block itself is held by its reference.
    assert!(texts(held(&list, "blocks", "Door")).iter().any(|t| t.starts_with("Used by a Block Reference")));
    // Purging changes nothing that the list held.
    s.execute("purge", &json!({})).unwrap();
    assert!(s.doc().unwrap().layer("Hidden").is_some());
}

#[test]
fn a_text_style_used_by_a_dimension_style_says_so() {
    let mut s = Session::new();
    s.execute("style", &json!({ "name": "DimText", "current": false })).unwrap();
    s.execute("dimstyle", &json!({ "name": "Arch", "current": false })).unwrap();
    s.doc_mut().unwrap().dim_styles.iter_mut().find(|d| d.name == "Arch").unwrap().text_style = "DimText".into();
    let list = s.execute("purge", &json!({ "list": true, "reasons": true })).unwrap();
    assert!(list["textstyles"].as_array().unwrap().is_empty());
    assert_eq!(texts(held(&list, "textstyles", "DimText")), vec!["Referenced by dimension style Arch"]);
    assert!(texts(held(&list, "textstyles", "Standard")).contains(&"The Standard style is always kept".to_string()));
    // Arch itself is purgeable; purging it frees DimText.
    assert_eq!(list["dimstyles"], json!(["Arch"]));
    s.execute("purge", &json!({ "type": "dimstyles" })).unwrap();
    let list = s.execute("purge", &json!({ "list": true })).unwrap();
    assert_eq!(list["textstyles"], json!(["DimText"]));
    assert!(list.get("nonPurgeable").is_none(), "only with reasons");
}

#[test]
fn nested_purging_repeats_until_nothing_more_goes() {
    let mut s = Session::new();
    // Inner sits in Outer; Outer is unused. Purging blocks by name pattern "Outer" first frees Inner.
    s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap();
    s.execute("selectall", &json!({})).unwrap();
    s.execute("block", &json!({ "name": "Inner", "base": [0, 0], "keep": "delete" })).unwrap();
    s.execute("insert", &json!({ "name": "Inner", "at": [0, 0] })).unwrap();
    s.execute("selectall", &json!({})).unwrap();
    s.execute("block", &json!({ "name": "Outer", "base": [0, 0], "keep": "delete" })).unwrap();
    s.execute("layer.new", &json!({ "name": "Spare" })).unwrap();
    s.execute("purge", &json!({ "type": "blocks", "nested": true })).unwrap();
    let d = s.doc().unwrap();
    assert!(d.block("Inner").is_none() && d.block("Outer").is_none());
    // A group with live members can't go, and says how many.
    s.execute("line", &json!({ "points": [[0, 0], [2, 2]] })).unwrap();
    s.execute("selectall", &json!({})).unwrap();
    s.execute("group", &json!({ "name": "G1" })).unwrap();
    let list = s.execute("purge", &json!({ "list": true, "reasons": true })).unwrap();
    let g = held(&list, "groups", "G1");
    assert_eq!(texts(g), vec!["Has 1 member object(s)"]);
    assert_eq!(g[0]["handles"].as_array().map(Vec::len), Some(1));
}

#[test]
fn exact_names_purge_only_those_even_with_wildcard_characters() {
    let mut s = Session::new();
    // `#` is a wildcard (any digit) and a valid character in a name.
    for n in ["A#B", "A1B", "Other"] {
        s.execute("layer.new", &json!({ "name": n })).unwrap();
    }
    s.execute("purge", &json!({ "type": "layers", "exact": ["a#b"] })).unwrap();
    let d = s.doc().unwrap();
    assert!(d.layer("A#B").is_none() && d.layer("A1B").is_some() && d.layer("Other").is_some());
}
