//! The website's claims about CADaddy stay true (Q232): every checked claim in
//! `docs/website-claims.json` holds against the facts the code gives (`facts.rs`), and
//! `docs/compare-facts.json` is up to date (`cargo xtask compare-facts` rewrites it).

#[path = "../facts.rs"]
mod facts;

use std::path::PathBuf;

use serde_json::{Value, json};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn every_website_claim_is_backed_and_the_facts_file_is_current() {
    let root = root();
    let f = facts::compute(&root);
    let claims: Value = serde_json::from_str(&std::fs::read_to_string(root.join("docs/website-claims.json")).unwrap()).unwrap();
    let list = claims["claims"].as_array().unwrap();
    assert!(list.len() >= 20, "the claims file lists the site's claims");
    let mut failures = Vec::new();
    for c in list {
        let text = c["claim"].as_str().unwrap_or_default();
        match (c.get("check"), c.get("manual")) {
            (Some(chk), None) => {
                if let Err(e) = facts::check(&f, chk) {
                    failures.push(format!("\"{text}\": {e}"));
                }
            }
            (None, Some(why)) => assert!(why.as_str().is_some_and(|w| !w.is_empty()), "\"{text}\": say why it can't be checked"),
            _ => failures.push(format!("\"{text}\": needs exactly one of `check` or `manual`")),
        }
    }
    assert!(
        failures.is_empty(),
        "claims the code no longer backs (fix the code or the site, then docs/website-claims.json):\n{}",
        failures.join("\n")
    );
    // The committed facts are what the code says now.
    let committed: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("docs/compare-facts.json")).unwrap_or_default()).unwrap_or(Value::Null);
    assert!(committed == f, "docs/compare-facts.json is out of date: run `cargo xtask compare-facts`");
    let llms = std::fs::read_to_string(root.join("docs/compare-facts-llms.txt")).unwrap_or_default();
    assert!(llms.contains(&facts::llms_lines(&f)), "docs/compare-facts-llms.txt is out of date: run `cargo xtask compare-facts`");
}

#[test]
fn facts_are_worked_out_not_typed() {
    let f = facts::compute(&root());
    // Formats are proved by writing and reading back.
    let open: Vec<&str> = f["formats"]["open"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(open.contains(&"dxf") && open.contains(&"cdy"), "{open:?}");
    assert!(f["versions"]["dxf"].as_array().unwrap().len() >= 6, "{}", f["versions"]);
    assert!(f["commands"]["count"].as_u64().unwrap() > 100);
    assert_eq!(f["commands"]["count"], f["commands"]["ids"].as_array().unwrap().len());
    // The same code gives the same facts.
    assert_eq!(f, facts::compute(&root()));
}

#[test]
fn a_claim_that_no_longer_holds_is_reported() {
    let f = json!({ "commands": { "count": 350 }, "formats": { "open": ["dxf"] }, "version": "0.7.8" });
    assert!(facts::check(&f, &json!({ "fact": "commands.count", "atLeast": 400 })).unwrap_err().contains("at least 400"));
    assert!(facts::check(&f, &json!({ "fact": "formats.open", "includes": ["dxf", "dwg"] })).unwrap_err().contains("dwg"));
    assert!(facts::check(&f, &json!({ "fact": "version", "equals": "0.7.9" })).is_err());
    assert!(facts::check(&f, &json!({ "fact": "no.such" })).is_err());
    assert!(facts::check(&f, &json!({ "fact": "commands.count", "atLeast": 300 })).is_ok());
}
