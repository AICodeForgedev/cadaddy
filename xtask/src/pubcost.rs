//! `cargo xtask fetch-pubcost`: generate, validate and package the public MEP cost data pack.
//!
//! The pack lives in `assets/public-cost-data/` and ships with CADaddy. Every row must have a
//! `source` and `licence` column. **No electrical labor** (man-hours, labor units, production
//! rates) may appear — that data is private.
//!
//! This xtask:
//! 1. Validates every CSV in the pack ([`validate`]: rectangular rows, numbers that parse,
//!    source, licence and status, no electrical labor — judged fail closed on every file).
//! 2. Generates a baseline-format folder (`assets/public-cost-data/baseline/`) that
//!    `estimate.import` can read directly, labelled "public data" (its `pack.json`), with a crew
//!    rate from the wages (`--area <BLS area code>` default Phoenix 38060, `--occupation <SOC>`
//!    default 47-2061 construction laborers).
//! 3. Reports the pack's contents and any violations.
//!
//! A future version will fetch live data from BLS and SAM.gov APIs; for now it validates and
//! packages the hand-curated seed data.

use std::collections::BTreeMap;
use std::path::Path;

/// Trade classes and trades that are electrical (P-405 writes "CE", Construction Electrician).
const ELECTRICAL_TRADES: &[&str] = &["electrical", "electric", "elec", "electrician", "ce", "construction electrician"];
/// Words that make an item, title or description electrical work.
const ELECTRICAL_WORDS: &[&str] = &[
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
/// Phrases with those words that aren't electrical (reinforcing, fencing, rigging).
const NOT_ELECTRICAL: &[&str] =
    &["welded wire", "wire fabric", "wire mesh", "tie wire", "barbed wire", "wire rope", "chicken wire", "plumbing fixture"];
/// Columns that are numbers wherever they appear.
const NUMERIC: &[&str] = &[
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
/// Every seed row's source says it is a placeholder.
pub const SEED_MARK: &str = "seed placeholder";
/// The area and occupation whose wage is the generated crew rate (`--area`, `--occupation`):
/// a BLS metropolitan area code and SOC code. Electricians aren't offered (no electrical labor
/// in the public pack).
pub const DEFAULT_AREA: &str = "38060";
pub const DEFAULT_OCCUPATION: &str = "47-2061";

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let pack = root.join("assets/public-cost-data");
    if !pack.is_dir() {
        return Err(format!("{}: not found", pack.display()));
    }
    let arg = |k: &str| args.windows(2).find(|w| w[0] == k).map(|w| w[1].clone());
    let area = arg("--area").unwrap_or_else(|| DEFAULT_AREA.into());
    let occupation = arg("--occupation").unwrap_or_else(|| DEFAULT_OCCUPATION.into());
    let (errors, stats) = validate(&pack)?;
    // Generate the baseline-format folder for estimate.import.
    let baseline_dir = pack.join("baseline");
    std::fs::create_dir_all(&baseline_dir).map_err(|e| format!("mkdir baseline: {e}"))?;
    generate_baseline(&pack, &baseline_dir, &area, &occupation)?;

    // Report.
    eprintln!("--- public cost data pack ---");
    for (name, count) in &stats {
        eprintln!("  {name}: {count} rows");
    }
    eprintln!("  baseline/ generated for estimate.import (crew rate: area {area}, occupation {occupation})");

    if errors.is_empty() {
        eprintln!("  OK: every row rectangular with numbers where numbers go, source, licence and status, no electrical labor");
        Ok(())
    } else {
        for e in &errors {
            eprintln!("  ERROR: {e}");
        }
        Err(format!("{} violations in the public cost data pack", errors.len()))
    }
}

/// Every CSV of the pack checked: each row has the header's number of cells; number columns
/// hold finite numbers; source, licence and status (`seed` with the placeholder mark in its
/// source, or `fetched` with its date); and no electrical labor, judged on every file whether it
/// has a trade column or not. The violations, and each file's row count.
pub fn validate(pack: &Path) -> Result<(Vec<String>, BTreeMap<String, usize>), String> {
    let mut errors: Vec<String> = Vec::new();
    let mut stats: BTreeMap<String, usize> = BTreeMap::new();
    let mut files: Vec<_> = std::fs::read_dir(pack).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    files.sort();
    for path in files {
        if path.extension().and_then(|e| e.to_str()) != Some("csv") {
            continue;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))?;
        let records = csv_records(&text);
        let Some(header) = records.first() else {
            errors.push(format!("{name}: empty"));
            continue;
        };
        let header: Vec<String> = header.iter().map(|h| h.to_ascii_lowercase()).collect();
        stats.insert(name.clone(), records.len() - 1);
        for col in ["source", "licence", "status", "fetched"] {
            if !header.iter().any(|h| h == col) {
                errors.push(format!("{name}: no `{col}` column"));
            }
        }
        for (i, cells) in records.iter().enumerate().skip(1) {
            let line = i + 1;
            if cells.len() != header.len() {
                errors.push(format!("{name}:{line}: {} cells under {} columns", cells.len(), header.len()));
                continue;
            }
            let row: BTreeMap<&str, &str> = header.iter().map(String::as_str).zip(cells.iter().map(|c| c.trim())).collect();
            let get = |k: &str| row.get(k).copied().unwrap_or("");
            for col in NUMERIC {
                let v = get(col);
                if !v.is_empty() && !v.parse::<f64>().is_ok_and(f64::is_finite) {
                    errors.push(format!("{name}:{line}: `{col}` is `{v}`, not a number"));
                }
            }
            if get("source").is_empty() {
                errors.push(format!("{name}:{line}: missing `source`"));
            }
            if get("licence").is_empty() && get("license").is_empty() {
                errors.push(format!("{name}:{line}: missing `licence`"));
            }
            match get("status") {
                "seed" if !get("source").contains(SEED_MARK) => {
                    errors.push(format!("{name}:{line}: a seed row whose source doesn't say \"{SEED_MARK}\""))
                }
                "seed" => {}
                "fetched" if get("fetched").is_empty() => errors.push(format!("{name}:{line}: fetched, but no `fetched` date")),
                "fetched" => {}
                other => errors.push(format!("{name}:{line}: status `{other}` (seed or fetched)")),
            }
            if let Some(why) = electrical_labor(&row) {
                errors.push(format!("{name}:{line}: ELECTRICAL LABOR ({why}) — this must not ship in the public pack"));
            }
        }
    }
    Ok((errors, stats))
}

/// Why a row is electrical labor: it has a labor value and its trade, its item (title,
/// description) or its P-405 table is electrical. `None` when it isn't.
fn electrical_labor(row: &BTreeMap<&str, &str>) -> Option<String> {
    let has_labor = ["labor_normal", "labor_difficult", "labor_very_difficult", "labor_hours", "man-hours/unit", "labor_dollars"]
        .iter()
        .any(|k| row.get(k).is_some_and(|v| !v.is_empty()));
    if !has_labor {
        return None;
    }
    for k in ["trade_class", "trade class", "trade"] {
        let t = row.get(k).copied().unwrap_or("").trim();
        if ELECTRICAL_TRADES.iter().any(|e| t.eq_ignore_ascii_case(e)) {
            return Some(format!("{k} {t}"));
        }
    }
    for k in ["item", "work element", "description", "p405_title", "title"] {
        let text = row.get(k).copied().unwrap_or("");
        if let Some(w) = electrical_word(text) {
            return Some(format!("{k} `{text}` ({w})"));
        }
    }
    None
}

/// The electrical word `text` holds, as a whole word or phrase (`wire`, not `welded wire`).
pub fn electrical_word(text: &str) -> Option<&'static str> {
    let mut t = format!(" {} ", text.to_ascii_lowercase().replace(|c: char| !c.is_ascii_alphanumeric(), " "));
    for n in NOT_ELECTRICAL {
        t = t.replace(&format!(" {n} "), " ");
    }
    ELECTRICAL_WORDS.iter().copied().find(|w| t.contains(&format!(" {w} ")))
}

/// Generate `item_labor_units.csv` and `prices.csv` in the baseline format from the pack's
/// CSVs, so `estimate.import {folder: "assets/public-cost-data/baseline"}` works.
fn generate_baseline(pack: &Path, out: &Path, area: &str, occupation: &str) -> Result<(), String> {
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
    // The crew rate: the area's hourly mean wage for the occupation; every non-electrical
    // occupation's for the area beside it.
    let wages = csv_rows(&std::fs::read_to_string(pack.join("bls_wages.csv")).map_err(|e| format!("bls_wages.csv: {e}"))?);
    let in_area: Vec<&BTreeMap<String, String>> = wages
        .iter()
        .filter(|r| r.get("area_code").map(String::as_str) == Some(area) && r.get("occupation_code").map(String::as_str) != Some("47-2111"))
        .collect();
    let rate = |r: &BTreeMap<String, String>| r.get("hourly_mean_wage").and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite() && *v > 0.0);
    if occupation == "47-2111" {
        return Err("electricians' wages aren't a public crew rate (electrical labor is private)".into());
    }
    let crew = in_area
        .iter()
        .find(|r| r.get("occupation_code").map(String::as_str) == Some(occupation))
        .and_then(|r| rate(r))
        .ok_or_else(|| format!("bls_wages.csv has no hourly wage for occupation {occupation} in area {area}"))?;
    let trades: serde_json::Map<String, serde_json::Value> =
        in_area.iter().filter_map(|r| Some((r.get("occupation_title")?.clone(), serde_json::json!(rate(r)?)))).collect();
    let area_title = in_area.first().and_then(|r| r.get("area_title")).cloned().unwrap_or_default();
    let rates = serde_json::json!({
        "crew_rate": crew,
        "area": format!("{area} {area_title}"),
        "occupation": occupation,
        "trade_rates": trades,
        "source": "bls_wages.csv (hourly mean wage; status as the rows there)",
    });
    std::fs::write(out.join("rates.json"), serde_json::to_string_pretty(&rates).map_err(|e| e.to_string())? + "\n")
        .map_err(|e| format!("write rates.json: {e}"))?;
    // What estimate.import labels it.
    std::fs::write(out.join("pack.json"), "{\n  \"label\": \"public data\"\n}\n").map_err(|e| format!("write pack.json: {e}"))?;
    std::fs::write(out.join("benchmarks.json"), "{}\n").map_err(|e| format!("write benchmarks.json: {e}"))?;

    Ok(())
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

/// CSV text's records: each line's cells (quotes honoured), blank lines left out.
fn csv_records(text: &str) -> Vec<Vec<String>> {
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
    rows
}

/// Parse CSV text into header-keyed rows (same as estimate.rs).
fn csv_rows(text: &str) -> Vec<BTreeMap<String, String>> {
    let rows = csv_records(text);
    let Some(header) = rows.first().cloned() else { return Vec::new() };
    let header: Vec<String> = header.iter().map(|h| h.to_ascii_lowercase()).collect();
    rows.into_iter().skip(1).map(|r| header.iter().cloned().zip(r).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/public-cost-data")
    }

    /// A copy of the pack in a temporary folder, `edit` applied to one file.
    fn copy_with(tag: &str, file: &str, edit: impl Fn(&str) -> String) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cadaddy-pubcost-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for e in std::fs::read_dir(pack()).unwrap().flatten() {
            if e.path().is_file() {
                std::fs::copy(e.path(), d.join(e.file_name())).unwrap();
            }
        }
        let p = d.join(file);
        std::fs::write(&p, edit(&std::fs::read_to_string(&p).unwrap())).unwrap();
        d
    }

    #[test]
    fn the_shipped_pack_is_valid() {
        let (errors, stats) = validate(&pack()).unwrap();
        assert!(errors.is_empty(), "{errors:#?}");
        assert_eq!(stats.len(), 7, "{stats:?}");
    }

    /// Electrical labor is found in a file with no trade column (concrete_costs.csv), by its item.
    #[test]
    fn a_planted_electrical_labor_row_fails_the_pack() {
        let d = copy_with("planted", "concrete_costs.csv", |t| {
            format!("{t}EMT conduit 3/4 in install,ft,,0.05,0.07,0.09,{SEED_MARK} — test,placeholder,test row,seed,\n")
        });
        let (errors, _) = validate(&d).unwrap();
        assert!(errors.iter().any(|e| e.contains("concrete_costs.csv") && e.contains("ELECTRICAL LABOR") && e.contains("conduit")), "{errors:#?}");
        // By its trade, too: a P-405 row of a Construction Electrician.
        let d = copy_with("trade", "p405_labor.csv", |t| {
            format!("{t}Pull cable in raceway,ft,0.02,0.03,0.04,99-9,Misc,CE,{SEED_MARK} — test,placeholder,test,seed,\n")
        });
        let (errors, _) = validate(&d).unwrap();
        assert!(errors.iter().any(|e| e.contains("trade_class CE")), "{errors:#?}");
    }

    #[test]
    fn short_rows_bad_numbers_and_unmarked_seeds_fail() {
        let d = copy_with("shape", "electrical_material.csv", |t| {
            format!("{t}THHN wire #14,ft,0.09,2025-06,seed placeholder — x,placeholder,note,seed,\n")
        });
        let (errors, _) = validate(&d).unwrap();
        assert!(errors.iter().any(|e| e.contains("electrical_material.csv") && e.contains("9 cells under 10 columns")), "{errors:#?}");
        let d = copy_with("number", "public_bid_prices.csv", |t| t.replacen("8.50", "eight", 1));
        let (errors, _) = validate(&d).unwrap();
        assert!(errors.iter().any(|e| e.contains("`median_price` is `eight`")), "{errors:#?}");
        let d = copy_with("seed", "bls_wages.csv", |t| t.replacen(SEED_MARK, "BLS OEWS", 1));
        let (errors, _) = validate(&d).unwrap();
        assert!(errors.iter().any(|e| e.contains("a seed row whose source doesn't say")), "{errors:#?}");
    }

    #[test]
    fn electrical_words_are_whole_words() {
        assert_eq!(electrical_word("THHN wire #12"), Some("wire"));
        assert_eq!(electrical_word("Fire alarm pull station"), Some("fire alarm"));
        assert_eq!(electrical_word("Welded wire fabric place"), None);
        assert_eq!(electrical_word("Plumbing fixture rough-in"), None);
        assert_eq!(electrical_word("Panelized roofing"), None, "a whole word");
    }
}
