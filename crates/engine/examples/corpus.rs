//! The synthetic corpus at size (`cargo xtask corpus`): made-up floor plans of 10k–500k objects,
//! seeded, each round-tripped through DXF, DWG and .cdy with an exact-difference report and the
//! time to save, open and regen. A time above its budget (`corpus_budget.json`, seconds per 100k
//! objects) is a warning, not a failure.
//!
//! `cargo run --release -p cadaddy-engine --example corpus -- [--sizes 10000,100000] [--seed N]
//! [--out DIR] [--keep]`. The report goes to `DIR/report.md` and `DIR/results.json` (default
//! `target/corpus`); `--keep` keeps the drawings written.

#[path = "../tests/corpus/mod.rs"]
mod corpus;

use std::fmt::Write as _;
use std::path::PathBuf;

use serde_json::{Value, json};

const FORMATS: &[&str] = &["dxf", "dwg", "cdy"];

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
    let _ = std::fs::write(out.join("report.md"), &report);
    let _ = std::fs::write(
        out.join("results.json"),
        serde_json::to_string_pretty(&json!({ "platform": platform, "seed": seed, "runs": results })).unwrap_or_default(),
    );
    println!("{report}");
}
