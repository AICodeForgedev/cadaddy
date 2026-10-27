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
    // Formats are proved by writing and reading back (with entities).
    let open: Vec<&str> = f["formats"]["open"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for ext in ["cdy", "dwg", "dxf", "dgn", "ifc"] {
        assert!(open.contains(&ext), "open includes {ext}: {open:?}");
    }
    // DWG bridge reads R2000..R2018.
    let bridge_read: Vec<&str> = f["versions"]["dwg"]["bridge"]["read"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for v in ["R2000", "R2004", "R2007", "R2010", "R2013", "R2018"] {
        assert!(bridge_read.contains(&v), "bridge reads {v}");
    }
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
    // The site shows the last published release; the code may be ahead of it, never behind.
    assert!(facts::check(&f, &json!({ "fact": "version", "versionAtLeast": "0.7.8" })).is_ok());
    assert!(facts::check(&f, &json!({ "fact": "version", "versionAtLeast": "0.7.7" })).is_ok());
    assert!(facts::check(&f, &json!({ "fact": "version", "versionAtLeast": "0.7.9" })).unwrap_err().contains("below"));
    assert!(
        facts::check(&json!({ "version": "0.7.10" }), &json!({ "fact": "version", "versionAtLeast": "0.7.9" })).is_ok(),
        "numeric, not text, order"
    );
    assert!(facts::check(&f, &json!({ "fact": "version", "versionAtLeast": "seven" })).is_err());
    assert!(facts::check(&f, &json!({ "fact": "no.such" })).is_err());
    assert!(facts::check(&f, &json!({ "fact": "commands.count", "atLeast": 300 })).is_ok());
}

/// MUST 1: DWG versions report both engines; bridge covers R2000..R2018; env vars don't change
/// the output. Env independence: `versions()` passes `DwgEngine::Native` / `DwgEngine::Bridge`
/// explicitly and calls `dwg_write::write` / `dwg_read::read` (native) or
/// `write_dwg`/`read_dwg` with an explicit engine (bridge) — neither path consults
/// `CADADDY_DWG*`. The `formats()` path for DWG still goes through `dwg_engine()`, but
/// `write_dwg` / `read_dwg` fall back across engines, so the save/open sets are stable.
/// Verified below by a subprocess run with `CADADDY_DWG=native`.
#[test]
fn dwg_versions_per_engine() {
    let f = facts::compute(&root());
    // Both engines present.
    assert!(f["versions"]["dwg"]["native"].is_object(), "native engine reported");
    assert!(f["versions"]["dwg"]["bridge"].is_object(), "bridge engine reported");
    // Bridge supports R2000..R2018 for both write and read.
    let bridge_write: Vec<&str> = f["versions"]["dwg"]["bridge"]["write"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    let bridge_read: Vec<&str> = f["versions"]["dwg"]["bridge"]["read"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for v in ["R2000", "R2004", "R2007", "R2010", "R2013", "R2018"] {
        assert!(bridge_write.contains(&v), "bridge writes {v}");
        assert!(bridge_read.contains(&v), "bridge reads {v}");
    }
    // Determinism: two runs give the same facts.
    assert_eq!(f, facts::compute(&root()), "compute() is deterministic");
}

/// Env-var independence: `CADADDY_DWG=native` must not change the facts. Runs
/// `facts_are_worked_out_not_typed` in a subprocess with the env var set.
#[test]
fn dwg_facts_ignore_cadaddy_dwg_env() {
    let out = std::process::Command::new(env!("CARGO"))
        .args(["test", "-p", "cadaddy-cli", "--test", "compare_facts", "--", "facts_are_worked_out_not_typed", "--exact", "-q"])
        .env("CADADDY_DWG", "native")
        .current_dir(root())
        .output()
        .expect("spawn cargo test");
    assert!(
        out.status.success(),
        "facts_are_worked_out_not_typed must pass with CADADDY_DWG=native:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// MUST 2: the format list is derived from `cadaddy_io::SAVE_EXTENSIONS`, not hand-typed.
/// Every extension the io write dispatch handles is in `formats.save` or explicitly excluded.
#[test]
fn format_list_derives_from_io() {
    let f = facts::compute(&root());
    let save: Vec<&str> = f["formats"]["save"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    // No exclusions today: every SAVE_EXTENSIONS entry writes the sample drawing.
    let excluded: &[&str] = &[];
    for ext in cadaddy_io::SAVE_EXTENSIONS {
        assert!(save.contains(ext) || excluded.contains(ext), "io SAVE_EXTENSIONS has `{ext}` but it is not in formats.save and not excluded");
    }
    // DGN, DWF and DWFx must appear in save.
    for ext in ["dgn", "dwf", "dwfx"] {
        assert!(save.contains(&ext), "`{ext}` must be in formats.save");
    }
}

/// MUST 3: a format counts as "open" only when the read-back drawing has entities.
#[test]
fn open_formats_require_entities() {
    let f = facts::compute(&root());
    let open: Vec<&str> = f["formats"]["open"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    for ext in ["cdy", "dwg", "dxf", "dgn", "ifc"] {
        assert!(open.contains(&ext), "format `{ext}` must be openable with entities");
    }
}
