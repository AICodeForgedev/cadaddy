//! Tests for the public MEP cost data pack (Q331).
//!
//! Enforced checks:
//! 1. Every row in every CSV has `source` and `licence` columns with non-empty values, and the
//!    header's number of cells, numbers where numbers go, and a status (seed rows say they are
//!    placeholders).
//! 2. No row has electrical labor: judged on every file (trade, item, P-405 table), fail closed.
//! 3. The generated baseline/ folder imports as "public data", with its units, prices and crew rate.
//! 4. The P-405 importer keeps electrical (and unknown-trade) rows private, or refuses.

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

/// Raw records: every line's cells (to count them against the header).
fn csv_cells(text: &str) -> Vec<Vec<String>> {
    let header_and_rows: Vec<Vec<String>> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
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
            cells
        })
        .collect();
    header_and_rows
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
            let licence = row.get("licence").or(row.get("license")).map(String::as_str).unwrap_or("").trim();
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

/// Electrical labor, judged as the xtask does: a labor value on a row whose trade, item, title or
/// description is electrical, on every file whether it has a trade column or not.
fn electrical_labor(row: &BTreeMap<String, String>) -> bool {
    let has_labor = ["labor_normal", "labor_difficult", "labor_very_difficult", "labor_hours", "man-hours/unit", "labor_dollars"]
        .iter()
        .any(|k| row.get(*k).is_some_and(|v| !v.trim().is_empty()));
    let trades = ["electrical", "electric", "elec", "electrician", "ce", "construction electrician"];
    let trade =
        ["trade_class", "trade class", "trade"].iter().any(|k| row.get(*k).is_some_and(|t| trades.iter().any(|e| t.trim().eq_ignore_ascii_case(e))));
    let words = [
        "conduit",
        "emt",
        "imc",
        "rmc",
        "wire",
        "wiring",
        "thhn",
        "xhhw",
        "romex",
        "receptacle",
        "breaker",
        "panel",
        "panelboard",
        "luminaire",
        "fixture",
        "fire alarm",
        "switchgear",
        "switchboard",
        "transformer",
        "cable tray",
        "junction box",
        "disconnect",
        "lighting",
        "electric",
        "electrical",
        "outlet",
        "gfci",
    ];
    let not = ["welded wire", "wire fabric", "wire mesh", "tie wire", "barbed wire", "wire rope", "chicken wire", "plumbing fixture"];
    let text = ["item", "work element", "description", "p405_title", "title"].iter().any(|k| {
        let mut t = format!(" {} ", row.get(*k).cloned().unwrap_or_default().to_ascii_lowercase().replace(|c: char| !c.is_ascii_alphanumeric(), " "));
        for n in not {
            t = t.replace(&format!(" {n} "), " ");
        }
        words.iter().any(|w| t.contains(&format!(" {w} ")))
    });
    has_labor && (trade || text)
}

/// The pack's CSV files: (name, text).
fn pack_csvs() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::fs::read_dir(pack_dir())
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("csv"))
        .map(|p| (p.file_name().unwrap().to_string_lossy().to_string(), std::fs::read_to_string(&p).unwrap()))
        .collect();
    out.sort();
    assert_eq!(out.len(), 7);
    out
}

#[test]
fn no_electrical_labor_in_public_pack() {
    let mut violations = Vec::new();
    for (name, text) in pack_csvs() {
        for (i, row) in csv_rows(&text).iter().enumerate() {
            if electrical_labor(row) {
                violations.push(format!("{name}:{}: {:?}", i + 2, row.get("item")));
            }
        }
    }
    assert!(violations.is_empty(), "electrical labor rows in the public pack (must not ship):\n{}", violations.join("\n"));
    // The check itself: a labor row planted in a file without a trade column is caught by its item,
    // and a P-405 row by its CE trade; welded wire fabric isn't electrical.
    let planted = "item,unit,material,labor_normal\nEMT conduit 3/4 in install,ft,,0.05\n";
    assert!(csv_rows(planted).iter().any(electrical_labor));
    let ce = "item,unit,labor_normal,trade class\nPull cable,ft,0.02,CE\n";
    assert!(csv_rows(ce).iter().any(electrical_labor));
    let wwf = "item,unit,labor_normal\nWelded wire fabric place,sq ft,0.004\n";
    assert!(!csv_rows(wwf).iter().any(electrical_labor));
}

/// Every row has the header's number of cells, and number columns hold finite numbers.
#[test]
fn pack_csvs_are_rectangular_with_numbers() {
    let numeric = [
        "material",
        "labor_normal",
        "labor_difficult",
        "labor_very_difficult",
        "labor_hours",
        "median_price",
        "n",
        "low",
        "high",
        "annual_mean_wage",
        "hourly_mean_wage",
        "basic_hourly_rate",
        "fringe",
        "total",
        "value",
    ];
    let mut bad = Vec::new();
    for (name, text) in pack_csvs() {
        let width = csv_cells(&text).first().map_or(0, Vec::len);
        for (i, cells) in csv_cells(&text).iter().enumerate().skip(1) {
            if cells.len() != width {
                bad.push(format!("{name}:{}: {} cells under {width}", i + 1, cells.len()));
            }
        }
        for (i, row) in csv_rows(&text).iter().enumerate() {
            for col in numeric {
                if let Some(v) = row.get(col).filter(|v| !v.is_empty())
                    && !v.parse::<f64>().is_ok_and(f64::is_finite)
                {
                    bad.push(format!("{name}:{}: {col} `{v}`", i + 2));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Every row has a status; a seed row says it's a placeholder and claims no agency's licence;
/// no fabricated identifiers; the right PPI series for sheet metal.
#[test]
fn every_row_has_a_status_and_seeds_say_so() {
    for (name, text) in pack_csvs() {
        assert!(!text.contains("TX20260001"), "{name}: a made-up determination number");
        for (i, row) in csv_rows(&text).iter().enumerate() {
            let line = i + 2;
            let status = row.get("status").map(String::as_str).unwrap_or("");
            assert!(status == "seed" || status == "fetched", "{name}:{line}: status `{status}`");
            let source = row.get("source").cloned().unwrap_or_default();
            if status == "seed" {
                assert!(source.contains("seed placeholder — not from"), "{name}:{line}: {source}");
                assert!(!row.get("licence").unwrap().contains("17 USC"), "{name}:{line}: a seed isn't a US government work");
            } else {
                assert!(!row.get("fetched").unwrap().is_empty(), "{name}:{line}: fetched without a date");
            }
            let state_rows = source.to_ascii_lowercase().contains("state") || source.to_ascii_lowercase().contains("bid tab");
            assert!(!(state_rows && row.get("licence").unwrap().contains("17 USC")), "{name}:{line}: state work isn't 17 USC §105");
        }
    }
    let idx = std::fs::read_to_string(pack_dir().join("escalation_indexes.csv")).unwrap();
    let duct = csv_rows(&idx).into_iter().find(|r| r["material_family"] == "ductwork_sheet_metal").unwrap();
    assert_eq!((duct["series_id"].as_str(), duct["series_title"].as_str()), ("PCU332322332322", "Sheet metal work manufacturing"));
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

/// The estimate state's baseline after an import (the drawing's building spec).
fn baseline_of(s: &Session) -> serde_json::Value {
    let spec: serde_json::Value = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
    spec["estimate"]["baseline"].clone()
}

/// The pack imports labelled "public data" without being told, every price with its CSV unit and
/// a numeric material (THHN wire #12: ft, $0.12).
#[test]
fn import_labels_public_data() {
    let mut s = Session::new();
    s.execute("bspec.set", &json!({ "replace": true, "spec": { "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}] } })).unwrap();
    let r = s.execute("estimate.import", &json!({ "folder": pack_dir().join("baseline").to_string_lossy() })).unwrap();
    assert_eq!(r["label"].as_str(), Some("public data"), "{r}");
    assert!(r["items"].as_u64().unwrap_or(0) >= 50, "{r}");
    let base = baseline_of(&s);
    assert_eq!(base["label"], json!("public data"));
    let prices = base["prices"].as_array().unwrap();
    // Every material row of the three price files.
    let with_material =
        |f: &str, col: &str| csv_rows(&std::fs::read_to_string(pack_dir().join(f)).unwrap()).iter().filter(|r| !r[col].is_empty()).count();
    let want = with_material("electrical_material.csv", "material")
        + with_material("concrete_costs.csv", "material")
        + with_material("public_bid_prices.csv", "median_price");
    assert_eq!(prices.len(), want);
    for p in prices {
        assert!(p["material"].as_f64().is_some_and(f64::is_finite), "material: {p}");
        assert!(!p["unit"].as_str().unwrap_or_default().is_empty(), "unit: {p}");
    }
    let thhn = prices.iter().find(|p| p["item"] == json!("THHN wire #12")).unwrap();
    assert_eq!((thhn["unit"].clone(), thhn["material"].clone()), (json!("ft"), json!(0.12)), "{thhn}");
    // Each price's unit is its CSV row's.
    let csv = csv_rows(&std::fs::read_to_string(pack_dir().join("electrical_material.csv")).unwrap());
    for row in &csv {
        let p = prices.iter().find(|p| p["item"] == json!(row["item"]) && p["size"] == json!(row["size"])).unwrap();
        assert_eq!(p["unit"], json!(row["unit"]), "{p}");
        assert_eq!(p["material"].as_f64(), row["material"].parse::<f64>().ok(), "{p}");
    }
}

/// Public labor is priced: a line takes its hours from P-405 and the crew rate is the pack's
/// wage for the area (rates.json from bls_wages.csv).
#[test]
fn public_labor_is_priced_at_the_packs_wage() {
    let mut s = Session::new();
    let terminal =
        |x: f64| json!({"system": "M-SA", "level": "1", "at": [x, 0], "size_in": [24, 24], "bottom_ft": 9, "terminal_kind": "Diffuser ceiling 2x2"});
    s.execute(
        "bspec.set",
        &json!({ "replace": true, "spec": { "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}], "mep": { "terminals": [terminal(0.0), terminal(120.0), terminal(240.0)] } } }),
    )
    .unwrap();
    let r = s.execute("estimate.import", &json!({ "folder": pack_dir().join("baseline").to_string_lossy() })).unwrap();
    assert_eq!(r["label"], json!("public data"));
    s.execute(
        "estimate.set_price",
        &json!({ "item": "Diffuser ceiling 2x2", "unit": "each", "material": 40, "date": "2026-10-01", "source": "test quote" }),
    )
    .unwrap();
    // The truth from the CSVs: P-405's hours, the wage of 47-2061 in area 38060.
    let p405 = csv_rows(&std::fs::read_to_string(pack_dir().join("p405_labor.csv")).unwrap());
    let hours: f64 = p405.iter().find(|r| r["item"] == "Diffuser ceiling 2x2").unwrap()["labor_normal"].parse().unwrap();
    let wages = csv_rows(&std::fs::read_to_string(pack_dir().join("bls_wages.csv")).unwrap());
    let rate: f64 =
        wages.iter().find(|r| r["area_code"] == "38060" && r["occupation_code"] == "47-2061").unwrap()["hourly_mean_wage"].parse().unwrap();
    let q = s.execute("estimate.query", &json!({ "trade": "mechanical" })).unwrap();
    let line = q["lines"].as_array().unwrap().iter().find(|l| l["item"] == json!("Diffuser ceiling 2x2")).unwrap_or_else(|| panic!("{q}"));
    let labor_hours = hours * 3.0;
    assert_eq!(line["labor_hours"].as_f64(), Some(labor_hours), "{line}");
    assert_eq!(line["labor_cost"].as_f64(), Some(labor_hours * rate), "{line}");
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

    let r = s
        .execute(
            "estimate.import_p405",
            &json!({
                "file": dir.join("p405_labor_units.csv").to_string_lossy(),
                "private_folder": priv_dir.to_string_lossy()
            }),
        )
        .unwrap();

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

/// The real header form; Construction Electrician (CE), Electrician, a blank trade and an
/// unknown one go private; a missing or bad private folder is an error with nothing kept.
#[test]
fn p405_import_keeps_ce_electrician_and_unknown_rows_private() {
    let dir = std::env::temp_dir().join(format!("cadaddy-p405-ce-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("private")).unwrap();
    let csv = "\
table,title,work element,unit,man-hours/unit,P-405 code,trade class
6-1,Formwork,Footing forms erect,sq ft,0.09,06110,concrete
11-2,Piping,Copper pipe 3/4 in solder,lin ft,0.09,15110,plumbing
16-1,Raceways,Rigid steel 1 in install,lin ft,0.07,16110,CE
16-2,Boxes,Device box set,each,0.30,16130,Electrician
16-3,Interior Lighting,Troffer hang,each,1.2,16510,
9-9,Misc,Mystery work,each,0.5,99999,zz
";
    let file = dir.join("p405.csv");
    std::fs::write(&file, csv).unwrap();
    let items = |s: &Session| baseline_of(s)["items"].as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
    let mut s = Session::new();
    s.execute("bspec.set", &json!({ "replace": true, "spec": { "levels": [{"name": "1", "elevation_ft": 0, "height_ft": 12}] } })).unwrap();
    // No private folder, or one that isn't a folder: refused, nothing kept.
    let e = s.execute("estimate.import_p405", &json!({ "file": file.to_string_lossy() })).unwrap_err().to_string();
    assert!(e.contains("private_folder"), "{e}");
    let not_dir = dir.join("p405.csv");
    let e = s
        .execute("estimate.import_p405", &json!({ "file": file.to_string_lossy(), "private_folder": not_dir.to_string_lossy() }))
        .unwrap_err()
        .to_string();
    assert!(e.contains("is not a folder"), "{e}");
    assert!(items(&s).is_empty(), "nothing kept: {:?}", items(&s));
    // With it: two public rows, four private.
    let r = s
        .execute("estimate.import_p405", &json!({ "file": file.to_string_lossy(), "private_folder": dir.join("private").to_string_lossy() }))
        .unwrap();
    assert_eq!((r["public_rows"].as_u64(), r["private_rows"].as_u64()), (Some(2), Some(4)), "{r}");
    let mut kept = items(&s);
    kept.sort();
    assert_eq!(kept, ["Copper pipe 3/4 in solder", "Footing forms erect"]);
    let private = std::fs::read_to_string(dir.join("private/p405_electrical_labor.tsv")).unwrap();
    for w in ["Rigid steel 1 in install", "Device box set", "Troffer hang", "Mystery work"] {
        assert!(private.contains(w), "{w} in {private}");
    }
    assert!(!private.contains("Footing") && !private.contains("Copper"), "{private}");
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
