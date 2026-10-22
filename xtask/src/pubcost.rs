//! `cargo xtask fetch-pubcost`: generate, validate and package the public MEP cost data pack.
//!
//! The pack lives in `assets/public-cost-data/` and ships with CADaddy. Every row must have a
//! `source` and `licence` column. **No electrical labor** (man-hours, labor units, production
//! rates) may appear — that data is private.
//!
//! This xtask:
//! 1. Validates every CSV in the pack (source+licence present, no electrical labor).
//! 2. Generates a baseline-format folder (`assets/public-cost-data/baseline/`) that
//!    `estimate.import` can read directly, labelled "public data".
//! 3. Reports the pack's contents and any violations.
//!
//! A future version will fetch live data from BLS and SAM.gov APIs; for now it validates and
//! packages the hand-curated seed data.

use std::collections::BTreeMap;
use std::path::Path;

/// Trade classes that are electrical.
const ELECTRICAL_TRADES: &[&str] = &["electrical", "electric", "elec"];

pub fn run(root: &Path) -> Result<(), String> {
    let pack = root.join("assets/public-cost-data");
    if !pack.is_dir() {
        return Err(format!("{}: not found", pack.display()));
    }

    let mut errors: Vec<String> = Vec::new();
    let mut stats: BTreeMap<String, usize> = BTreeMap::new();

    // Validate every CSV in the pack.
    for entry in std::fs::read_dir(&pack).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("csv") {
            continue;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))?;
        let rows = csv_rows(&text);
        stats.insert(name.clone(), rows.len());

        for (i, row) in rows.iter().enumerate() {
            let line = i + 2; // 1-indexed, skip header
            // Every row must have source and licence.
            let source = row.get("source").map(String::as_str).unwrap_or("").trim();
            let licence = row.get("licence").or(row.get("license")).map(String::as_str).unwrap_or("").trim();
            if source.is_empty() {
                errors.push(format!("{name}:{line}: missing `source`"));
            }
            if licence.is_empty() {
                errors.push(format!("{name}:{line}: missing `licence`"));
            }

            // No electrical labor: a row with a labor value and an electrical trade is forbidden.
            let trade = row.get("trade_class").map(String::as_str).unwrap_or("");
            let is_elec_trade = ELECTRICAL_TRADES.iter().any(|t| trade.eq_ignore_ascii_case(t));
            let has_labor = row.get("labor_normal").filter(|v| !v.trim().is_empty()).is_some()
                || row.get("labor_difficult").filter(|v| !v.trim().is_empty()).is_some()
                || row.get("labor_very_difficult").filter(|v| !v.trim().is_empty()).is_some()
                || row.get("labor_hours").filter(|v| !v.trim().is_empty()).is_some();
            if is_elec_trade && has_labor {
                let item = row.get("item").map(String::as_str).unwrap_or("?");
                errors.push(format!("{name}:{line}: ELECTRICAL LABOR found (trade={trade}, item={item}) — this must not ship in the public pack"));
            }
        }
    }

    // Generate the baseline-format folder for estimate.import.
    let baseline_dir = pack.join("baseline");
    std::fs::create_dir_all(&baseline_dir).map_err(|e| format!("mkdir baseline: {e}"))?;
    generate_baseline(&pack, &baseline_dir)?;

    // Report.
    eprintln!("--- public cost data pack ---");
    for (name, count) in &stats {
        eprintln!("  {name}: {count} rows");
    }
    eprintln!("  baseline/ generated for estimate.import");

    if errors.is_empty() {
        eprintln!("  OK: all rows have source+licence, no electrical labor found");
        Ok(())
    } else {
        for e in &errors {
            eprintln!("  ERROR: {e}");
        }
        Err(format!("{} violations in the public cost data pack", errors.len()))
    }
}

/// Generate `item_labor_units.csv` and `prices.csv` in the baseline format from the pack's
/// CSVs, so `estimate.import {folder: "assets/public-cost-data/baseline"}` works.
fn generate_baseline(pack: &Path, out: &Path) -> Result<(), String> {
    let mut items: Vec<String> = vec!["item,unit,material,labor_normal,labor_difficult,labor_very_difficult".into()];
    let mut prices: Vec<String> = vec!["item,size,unit,material,labor_hours,date,source".into()];

    // P-405 labor → item_labor_units.csv
    if let Ok(text) = std::fs::read_to_string(pack.join("p405_labor.csv")) {
        for row in csv_rows(&text) {
            let item = row.get("item").map(String::as_str).unwrap_or("");
            let unit = row.get("unit").map(String::as_str).unwrap_or("");
            let ln = row.get("labor_normal").map(String::as_str).unwrap_or("");
            let ld = row.get("labor_difficult").map(String::as_str).unwrap_or("");
            let lv = row.get("labor_very_difficult").map(String::as_str).unwrap_or("");
            if !item.is_empty() && !ln.is_empty() {
                items.push(format!("{},{},{},{},{},{}", csv_escape(item), csv_escape(unit), "", ln, ld, lv));
            }
        }
    }

    // Concrete costs → items (labor rows) and prices (material rows)
    if let Ok(text) = std::fs::read_to_string(pack.join("concrete_costs.csv")) {
        for row in csv_rows(&text) {
            let item = row.get("item").map(String::as_str).unwrap_or("");
            let unit = row.get("unit").map(String::as_str).unwrap_or("");
            let mat = row.get("material").map(String::as_str).unwrap_or("");
            let ln = row.get("labor_normal").map(String::as_str).unwrap_or("");
            if !item.is_empty() {
                if !ln.is_empty() {
                    let ld = row.get("labor_difficult").map(String::as_str).unwrap_or("");
                    let lv = row.get("labor_very_difficult").map(String::as_str).unwrap_or("");
                    items.push(format!("{},{},{},{},{},{}", csv_escape(item), csv_escape(unit), mat, ln, ld, lv));
                }
                if !mat.is_empty() {
                    prices.push(format!("{},,{},{},,,public data (concrete_costs.csv)", csv_escape(item), csv_escape(unit), mat));
                }
            }
        }
    }

    // Electrical material → prices (material only, no labor)
    if let Ok(text) = std::fs::read_to_string(pack.join("electrical_material.csv")) {
        for row in csv_rows(&text) {
            let item = row.get("item").map(String::as_str).unwrap_or("");
            let size = row.get("size").map(String::as_str).unwrap_or("");
            let unit = row.get("unit").map(String::as_str).unwrap_or("each");
            let mat = row.get("material").map(String::as_str).unwrap_or("");
            let date = row.get("date").map(String::as_str).unwrap_or("");
            if !item.is_empty() && !mat.is_empty() {
                prices.push(format!(
                    "{},{},{},{},,{},public data (electrical_material.csv)",
                    csv_escape(item),
                    csv_escape(size),
                    csv_escape(unit),
                    mat,
                    date
                ));
            }
        }
    }

    // Public bid prices → prices
    if let Ok(text) = std::fs::read_to_string(pack.join("public_bid_prices.csv")) {
        for row in csv_rows(&text) {
            let item = row.get("item").map(String::as_str).unwrap_or("");
            let unit = row.get("unit").map(String::as_str).unwrap_or("");
            let price = row.get("median_price").map(String::as_str).unwrap_or("");
            if !item.is_empty() && !price.is_empty() {
                prices.push(format!("{},,{},{},,2025,public data (public_bid_prices.csv)", csv_escape(item), csv_escape(unit), price));
            }
        }
    }

    std::fs::write(out.join("item_labor_units.csv"), items.join("\n") + "\n").map_err(|e| format!("write item_labor_units.csv: {e}"))?;
    std::fs::write(out.join("prices.csv"), prices.join("\n") + "\n").map_err(|e| format!("write prices.csv: {e}"))?;

    // Empty but valid files so estimate.import doesn't warn.
    std::fs::write(out.join("assemblies.csv"), "assembly,unit,description,labor_hours\n").map_err(|e| format!("write assemblies.csv: {e}"))?;
    std::fs::write(out.join("assembly_parts.csv"), "assembly,item,qty\n").map_err(|e| format!("write assembly_parts.csv: {e}"))?;
    std::fs::write(out.join("rates.json"), "{}\n").map_err(|e| format!("write rates.json: {e}"))?;
    std::fs::write(out.join("benchmarks.json"), "{}\n").map_err(|e| format!("write benchmarks.json: {e}"))?;

    Ok(())
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

/// Parse CSV text into header-keyed rows (same as estimate.rs).
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
