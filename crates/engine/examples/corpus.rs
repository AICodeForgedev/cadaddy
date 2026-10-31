//! The synthetic corpus at size (`cargo xtask corpus`): made-up floor plans of 10k–500k objects,
//! seeded, each round-tripped through DXF, DWG and .cdy with an exact-difference report and the
//! time to save, open and regen. A time above its budget (`corpus_budget.json`, seconds per 100k
//! objects) is a warning, not a failure, unless `--strict` is given: then a DWG open or save whose
//! 500k time is more than 7 times its 100k time, or a macOS arm64 500k DWG open over 15 s, exits 1.
//!
//! `cargo run --release -p cadaddy-engine --example corpus -- [--sizes 10000,100000] [--seed N]
//! [--out DIR] [--keep] [--strict]`. The report goes to `DIR/report.md` and `DIR/results.json` (default
//! `target/corpus`); `--keep` keeps the drawings written.

#[path = "../tests/corpus/mod.rs"]
mod corpus;

use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::{Value, json};

const FORMATS: &[&str] = &["dxf", "dwg", "dwg-native", "cdy"];

/// Largest allowed 500k/100k time ratio for a DWG open or save (5x the objects, so 7 leaves little
/// room above linear).
const MAX_SCALE_RATIO: f64 = 7.0;
/// Largest allowed 500k DWG open on macOS arm64, in seconds.
const MAX_OPEN_500K_MAC: f64 = 15.0;

/// One measured time: (requested size, format, operation, seconds).
type Timing = (usize, String, &'static str, f64);

/// Scaling flags over the measured times: for each DWG format (`dwg`, `dwg-native`) and operation
/// (open, save) with both a 100k and a 500k run, a 500k/100k ratio over [`MAX_SCALE_RATIO`]; and on
/// macOS arm64 a 500k `dwg` open over [`MAX_OPEN_500K_MAC`] s.
fn scale_flags(timings: &[Timing], platform: &str) -> Vec<String> {
    let find = |size: usize, format: &str, op: &str| timings.iter().find(|(s, f, o, _)| *s == size && f == format && *o == op).map(|t| t.3);
    let mut flags = Vec::new();
    for format in ["dwg", "dwg-native"] {
        for op in ["open", "save"] {
            let (Some(small), Some(big)) = (find(100_000, format, op), find(500_000, format, op)) else { continue };
            // A floor keeps timer noise on a tiny 100k time from flagging.
            let ratio = big / small.max(0.05);
            if ratio > MAX_SCALE_RATIO {
                flags.push(format!("{format} {op}: 500k {big:.2} s is {ratio:.1}x the 100k {small:.2} s (limit {MAX_SCALE_RATIO}x)"));
            }
        }
    }
    if platform == "macos-aarch64"
        && let Some(open) = find(500_000, "dwg", "open")
        && open > MAX_OPEN_500K_MAC
    {
        flags.push(format!("dwg open: 500k took {open:.2} s (limit {MAX_OPEN_500K_MAC} s on macos-aarch64)"));
    }
    flags
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned();
    let sizes: Vec<usize> = value("--sizes")
        .unwrap_or_else(|| "10000,50000,100000,250000,500000".into())
        .split(',')
        .filter_map(|v| v.trim().replace('_', "").parse().ok())
        .filter(|n| (1..=2_000_000).contains(n))
        .collect();
    let seed: u64 = value("--seed").and_then(|v| v.parse().ok()).unwrap_or(1);
    let out = value("--out").map_or_else(|| PathBuf::from("target/corpus"), PathBuf::from);
    let keep = args.iter().any(|a| a == "--keep");
    let strict = args.iter().any(|a| a == "--strict");
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("corpus: {}: {e}", out.display());
        std::process::exit(2);
    }
    let budget: Value = serde_json::from_str(include_str!("corpus_budget.json")).unwrap_or(Value::Null);
    let platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let rates = budget.get(&platform);

    let mut report = format!("# Corpus round trips ({platform}, seed {seed})\n\n");
    let mut results = Vec::new();
    let mut warnings = Vec::new();
    let mut timings: Vec<Timing> = Vec::new();
    for &size in &sizes {
        let made = corpus::generate(size, seed);
        let kinds = corpus::kinds(&made.drawing);
        let _ = writeln!(report, "## {} objects (asked {size}), made in {:.1} s\n", made.objects, made.seconds);
        let _ = writeln!(report, "Kinds: {}\n", kinds.iter().map(|(k, n)| format!("{k} {n}")).collect::<Vec<_>>().join(", "));
        let _ = writeln!(report, "| format | save s | size MB | open s | regen s | differences |\n|---|---|---|---|---|---|");
        let mut details = String::new();
        for &format in FORMATS {
            let trip = match corpus::round_trip(&made.drawing, format) {
                Ok(t) => t,
                Err(e) => {
                    let _ = writeln!(report, "| {format} | **{e}** | | | | |");
                    warnings.push(format!("{size}: {e}"));
                    continue;
                }
            };
            let mb = trip.bytes as f64 / 1_048_576.0;
            let _ = writeln!(report, "| {format} | {:.2} | {mb:.1} | {:.2} | {:.2} | {} |", trip.save, trip.open, trip.regen, trip.differences.len());
            for (op, secs) in [("save", trip.save), ("open", trip.open), ("regen", trip.regen)] {
                timings.push((size, format.to_string(), op, secs));
                let Some(rate) = rates.and_then(|r| r[format][op].as_f64()) else { continue };
                // The budget grows with the size, with a floor for small drawings.
                let allowed = (rate * made.objects as f64 / 100_000.0).max(0.25);
                if secs > allowed {
                    warnings.push(format!("{} objects, {format} {op}: {secs:.2} s, budget {allowed:.2} s", made.objects));
                }
            }
            if !trip.differences.is_empty() {
                let _ = writeln!(details, "\n{format} differences after reading back:\n");
                for d in &trip.differences {
                    let _ = writeln!(details, "- {d}");
                }
            }
            if keep {
                let name = out.join(format!("corpus-{size}-seed{seed}.{format}"));
                if let Ok(bytes) = cadaddy_io::write(&made.drawing, &name.to_string_lossy()) {
                    let _ = std::fs::write(&name, bytes);
                }
            }
            results.push(json!({
                "size": size, "objects": made.objects, "format": format, "save": trip.save, "bytes": trip.bytes,
                "open": trip.open, "regen": trip.regen, "differences": trip.differences,
            }));
        }
        report.push_str(&details);
        report.push('\n');
        eprintln!("corpus: {} objects done", made.objects);
    }
    let _ = writeln!(report, "## Over budget ({})\n", warnings.len());
    if rates.is_none() {
        let _ = writeln!(report, "No budgets for {platform} in corpus_budget.json: timings recorded only.\n");
    }
    for w in &warnings {
        let _ = writeln!(report, "- WARNING {w}");
    }
    let flags = scale_flags(&timings, &platform);
    let _ = writeln!(report, "\n## Scaling flags ({})\n", flags.len());
    for f in &flags {
        let _ = writeln!(report, "- FLAG {f}");
    }
    let _ = std::fs::write(out.join("report.md"), &report);
    let _ = std::fs::write(
        out.join("results.json"),
        serde_json::to_string_pretty(&json!({ "platform": platform, "seed": seed, "runs": results })).unwrap_or_default(),
    );
    println!("{report}");
    if strict && !flags.is_empty() {
        eprintln!("corpus: --strict: {} scaling flag(s)", flags.len());
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(format: &str, op: &'static str, small: f64, big: f64) -> Vec<Timing> {
        vec![(100_000, format.into(), op, small), (500_000, format.into(), op, big)]
    }

    #[test]
    fn a_superlinear_open_is_flagged_and_a_linear_one_is_not() {
        let bad = scale_flags(&pair("dwg", "open", 1.6, 47.5), "linux-x86_64");
        assert_eq!(bad.len(), 1, "{bad:?}");
        assert!(scale_flags(&pair("dwg", "open", 1.6, 8.0), "linux-x86_64").is_empty());
    }

    #[test]
    fn both_dwg_engines_and_save_are_checked_but_other_formats_are_not() {
        assert_eq!(scale_flags(&pair("dwg-native", "save", 1.0, 20.0), "linux-x86_64").len(), 1);
        assert_eq!(scale_flags(&pair("dwg", "save", 1.0, 20.0), "linux-x86_64").len(), 1);
        assert!(scale_flags(&pair("dxf", "open", 1.0, 20.0), "linux-x86_64").is_empty());
    }

    #[test]
    fn the_mac_500k_open_cap_is_15_seconds() {
        // 3.0 s -> 16.0 s is within the ratio (5.3x) but over the cap.
        let over = pair("dwg", "open", 3.0, 16.0);
        assert_eq!(scale_flags(&over, "macos-aarch64").len(), 1);
        assert!(scale_flags(&over, "linux-x86_64").is_empty());
        assert!(scale_flags(&pair("dwg", "open", 2.4, 14.0), "macos-aarch64").is_empty());
    }

    #[test]
    fn missing_sizes_are_not_flagged() {
        assert!(scale_flags(&[(100_000, "dwg".into(), "open", 9.0)], "macos-aarch64").is_empty());
    }
}
