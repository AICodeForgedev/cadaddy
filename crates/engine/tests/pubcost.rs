//! Tests for the public MEP cost data pack (Q331).
//!
//! Enforced checks:
//! 1. Every row in every CSV has `source` and `licence` columns with non-empty values.
//! 2. No row has electrical labor (man-hours/labor units with an electrical trade class).
//! 3. The generated baseline/ folder is valid for `estimate.import`.
//! 4. The P-405 importer splits electrical and non-electrical correctly.

use cadaddy_engine::Session;
use serde_json::json;
use std::collections::BTreeMap;

/// Parse CSV the same way as the xtask and the engine.
fn csv_rows(text: &str) -> Vec<BTreeMap<String, String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for line in text.lines().take(200_000) {
        let mut cells = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match (c, quoted) {
                ('"', true) if chars.peek() == Some(&'"') => {
                    cur.push('"');
                    chars.next();
                }
                ('"', true) => quoted = false,
                ('"', false) if cur.is_empty() => quoted = true,
                (',', false) => cells.push(std::mem::take(&mut cur)),
                _ => cur.push(c),
            }
        }
        cells.push(cur);
        if cells.iter().any(|c| !c.trim().is_empty()) {
            rows.push(cells.into_iter().map(|c| c.trim().to_string()).collect());
        }
    }
    let Some(header) = rows.first().cloned() else { return Vec::new() };
    let header: Vec<String> = header.iter().map(|h| h.to_ascii_lowercase()).collect();
    rows.into_iter().skip(1).map(|r| header.iter().cloned().zip(r).collect()).collect()
}

/// Find the repo root (the test binary is in target/<profile>/deps/).
fn repo_root() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap().parent().unwrap().to_path_buf()
}

fn pack_dir() -> std::path::PathBuf {
    repo_root().join("assets/public-cost-data")
}

// --------------- enforced checks on the shipped data ---------------

#[test]
fn every_row_has_source_and_licence() {
    let pack = pack_dir();
    let mut violations = Vec::new();
    for entry in std::fs::read_dir(&pack).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("csv") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        for (i, row) in csv_rows(&text).iter().enumerate() {
            let line = i + 2;
            let source = row.get("source").map(String::as_str).unwrap_or("").trim();
            let licence = row
                .get("licence")
                .or(row.get("license"))
                .map(String::as_str)
                .unwrap_or("")
                .trim();
            if source.is_empty() {
                violations.push(format!("{name}:{line}: missing source"));
            }
            if licence.is_empty() {
                violations.push(format!("{name}:{line}: missing licence"));
            }
        }
    }
    assert!(violations.is_empty(), "rows without source or licence:\n{}", violations.join("\n"));
}

#[test]
fn no_electrical_labor_in_public_pack() {
    let pack = pack_dir();
    let electrical_trades = ["electrical", "electric", "elec"];
    let mut violations = Vec::new();
    for entry in std::fs::read_dir(&pack).unwrap().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("csv") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        for (i, row) in csv_rows(&text).iter().enumerate() {
            let line = i + 2;
            let trade = row.get("trade_class").map(String::as_str).unwrap_or("");
            let is_elec = electrical_trades.iter().any(|t| trade.eq_ignore_ascii_case(t));
            let has_labor = row.get("labor_normal").filter(|v| !v.trim().is_empty()).is_some()
                || row.get("labor_difficult").filter(|v| !v.trim().is_empty()).is_some()
                || row.get("labor_very_difficult").filter(|v| !v.trim().is_empty()).is_some()
                || row.get("labor_hours").filter(|v| !v.trim().is_empty()).is_some();
            if is_elec && has_labor {
                let item = row.get("item").map(String::as_str).unwrap_or("?");
                violations.push(format!("{name}:{line}: electrical labor (trade={trade}, item={item})"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "electrical labor rows in the public pack (must not ship):\n{}",
        violations.join("\n")
    );
}

#[test]
fn p405_labor_has_representative_trades() {
    let path = pack_dir().join("p405_labor.csv");
    let text = std::fs::read_to_string(&path).unwrap();
    let rows = csv_rows(&text);
    assert!(rows.len() >= 50, "expected at least 50 P-405 labor rows, got {}", rows.len());

    let trades: Vec<String> = rows.iter().filter_map(|r| r.get("trade_class").cloned()).collect();
    let unique: std::collections::BTreeSet<&str> = trades.iter().map(String::as_str).collect();
    // Must have at least: earthwork, concrete, masonry, carpentry, drywall, roofing, plumbing, mechanical, site
    for required in &["earthwork", "concrete", "masonry", "carpentry", "drywall", "roofing", "plumbing", "mechanical", "site"] {
        assert!(unique.contains(required), "P-405 data missing required trade: {required}");
    }
    // Must NOT have electrical
    assert!(!unique.contains("electrical"), "P-405 public data must not contain electrical trade");
}

#[test]
fn p405_labor_rows_have_difficulty_tiers() {
    let path = pack_dir().join("p405_labor.csv");
    let text = std::fs::read_to_string(&path).unwrap();
    for (i, row) in csv_rows(&text).iter().enumerate() {
        let line = i + 2;
        let item = row.get("item").map(String::as_str).unwrap_or("?");
        let ln = row.get("labor_normal").filter(|v| !v.trim().is_empty());
        let ld = row.get("labor_difficult").filter(|v| !v.trim().is_empty());
        let lv = row.get("labor_very_difficult").filter(|v| !v.trim().is_empty());
        assert!(ln.is_some(), "p405_labor.csv:{line}: {item} missing labor_normal");
        assert!(ld.is_some(), "p405_labor.csv:{line}: {item} missing labor_difficult");
        assert!(lv.is_some(), "p405_labor.csv:{line}: {item} missing labor_very_difficult");
        // Difficulty tiers should be increasing.
        let n: f64 = ln.unwrap().parse().unwrap();
        let d: f64 = ld.unwrap().parse().unwrap();
        let v: f64 = lv.unwrap().parse().unwrap();
        assert!(d >= n, "p405_labor.csv:{line}: {item} difficult ({d}) < normal ({n})");
        assert!(v >= d, "p405_labor.csv:{line}: {item} very_difficult ({v}) < difficult ({d})");
    }
}

#[test]
fn baseline_folder_is_valid_for_import() {
    let baseline = pack_dir().join("baseline");
    if !baseline.is_dir() {
        // The xtask needs to have been run first.
        panic!("baseline/ folder not found — run `cargo xtask fetch-pubcost` first");
    }
    // Check all required files exist.
    for name in &["item_labor_units.csv", "assemblies.csv", "assembly_parts.csv", "prices.csv", "rates.json", "benchmarks.json"] {
        assert!(baseline.join(name).exists(), "baseline/{name} missing");
    }
    // item_labor_units.csv should have rows from P-405.
    let text = std::fs::read_to_string(baseline.join("item_labor_units.csv")).unwrap();
    let rows = csv_rows(&text);
    assert!(rows.len() >= 50, "baseline item_labor_units.csv has only {} rows", rows.len());
    // prices.csv should have rows from material and bid data.
    let text = std::fs::read_to_string(baseline.join("prices.csv")).unwrap();
    let rows = csv_rows(&text);
    assert!(rows.len() >= 10, "baseline prices.csv has only {} rows", rows.len());
}

#[test]
fn electrical_material_has_no_labor() {
    let path = pack_dir().join("electrical_material.csv");
    let text = std::fs::read_to_string(&path).unwrap();
    let rows = csv_rows(&text);
    assert!(!rows.is_empty(), "electrical_material.csv is empty");
    for (i, row) in rows.iter().enumerate() {
        let line = i + 2;
        let item = row.get("item").map(String::as_str).unwrap_or("?");
        // No labor columns should have values.
        for col in &["labor_normal", "labor_difficult", "labor_very_difficult", "labor_hours", "labor_dollars"] {
            let val = row.get(*col).map(String::as_str).unwrap_or("").trim();
            assert!(val.is_empty(), "electrical_material.csv:{line}: {item} has {col}={val} (electrical material must not include labor)");
        }
    }
}

// --------------- estimate.import with label ---------------

#[test]
fn import_labels_public_data() {
    let mut s = Session::new();
    // Create a minimal spec so estimate commands work.
    s.execute("bspec.set", &json!({ "replace": true, "spec": { "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}] } })).unwrap();

    let baseline = pack_dir().join("baseline");
    if !baseline.is_dir() {
        return; // Skip if baseline not generated.
    }
    let r = s.execute("estimate.import", &json!({
        "folder": baseline.to_string_lossy(),
        "label": "public data"
    })).unwrap();
    assert_eq!(r["label"].as_str(), Some("public data"), "import should echo the label: {r}");
    assert!(r["items"].as_u64().unwrap_or(0) >= 50, "expected at least 50 items from public pack: {r}");
}

// --------------- P-405 split importer ---------------

#[test]
fn p405_importer_splits_electrical_to_private() {
    let mut s = Session::new();
    s.execute("bspec.set", &json!({ "replace": true, "spec": { "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}] } })).unwrap();

    // Create a test CSV with both electrical and non-electrical rows.
    let dir = std::env::temp_dir().join(format!("cadaddy-p405-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let priv_dir = dir.join("private");
    std::fs::create_dir_all(&priv_dir).unwrap();

    let csv = "\
item,unit,labor_normal,p405_table,trade_class
Concrete footing forms erect,sq ft,0.09,6-1,concrete
Rebar place #3-#5,ton,20.0,6-2,concrete
EMT conduit 3/4 in,ft,0.05,13-1,electrical
Romex NM-B 12/2,ft,0.03,13-1,electrical
Copper pipe 3/4 in solder,ft,0.09,11-1,plumbing
";
    std::fs::write(dir.join("p405_labor_units.csv"), csv).unwrap();

    let r = s.execute("estimate.import_p405", &json!({
        "file": dir.join("p405_labor_units.csv").to_string_lossy(),
        "private_folder": priv_dir.to_string_lossy()
    })).unwrap();

    assert_eq!(r["public_rows"].as_u64(), Some(3), "3 non-electrical rows: {r}");
    assert_eq!(r["private_rows"].as_u64(), Some(2), "2 electrical rows: {r}");

    // The private file should exist with the electrical rows.
    let priv_file = priv_dir.join("p405_electrical_labor.tsv");
    assert!(priv_file.exists(), "private electrical labor file not written");
    let priv_text = std::fs::read_to_string(&priv_file).unwrap();
    assert!(priv_text.contains("EMT conduit"), "private file should contain electrical items: {priv_text}");
    assert!(!priv_text.contains("Concrete"), "private file should NOT contain non-electrical items: {priv_text}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn manifest_exists_and_has_required_fields() {
    let manifest_path = pack_dir().join("manifest.json");
    let text = std::fs::read_to_string(&manifest_path).expect("manifest.json should exist");
    let v: serde_json::Value = serde_json::from_str(&text).expect("manifest.json should be valid JSON");
    assert!(v["name"].is_string(), "manifest.json missing name");
    assert!(v["version"].is_string(), "manifest.json missing version");
    assert!(v["sources"].is_object(), "manifest.json missing sources");
    assert!(v["excluded"].is_object(), "manifest.json missing excluded section");
    assert!(v["excluded"]["electrical_labor"].is_string(), "manifest.json must document electrical_labor exclusion");
}

#[test]
fn condition_factors_exist() {
    let path = pack_dir().join("p405_condition_factors.json");
    let text = std::fs::read_to_string(&path).expect("p405_condition_factors.json should exist");
    let v: serde_json::Value = serde_json::from_str(&text).expect("condition factors should be valid JSON");
    assert!(v["difficulty_tiers"]["normal"]["factor"].as_f64() == Some(1.0));
    assert!(v["difficulty_tiers"]["difficult"]["factor"].as_f64() == Some(1.4));
    assert!(v["difficulty_tiers"]["very_difficult"]["factor"].as_f64() == Some(1.8));
    assert!(v["adjustment_factors"].is_object());
}
