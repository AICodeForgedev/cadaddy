//! `mep-fixtures [--seed N] [--count K] [--out DIR]`: write fixture sets `DIR/seed-N/` (sheets as
//! PDF and DXF, `set.pdf`, `truth.json`). `cargo xtask mep-fixtures` runs it.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut seed = 1u64;
    let mut count = 1u64;
    let mut out = PathBuf::from("target/mep-fixtures");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || args.next().unwrap_or_default();
        match a.as_str() {
            "--seed" => seed = value().parse().unwrap_or(seed),
            "--count" => count = value().parse().unwrap_or(count).clamp(1, 1000),
            "--out" => out = PathBuf::from(value()),
            "-h" | "--help" => {
                println!("mep-fixtures [--seed N (1)] [--count K (1)] [--out DIR (target/mep-fixtures)]");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("mep-fixtures: unknown argument `{other}` (see --help)");
                return ExitCode::FAILURE;
            }
        }
    }
    for s in seed..seed.saturating_add(count) {
        let set = match cadaddy_mepfix::generate(s) {
            Ok(set) => set,
            Err(e) => {
                eprintln!("mep-fixtures: seed {s}: {e}");
                return ExitCode::FAILURE;
            }
        };
        let dir = out.join(format!("seed-{s}"));
        if let Err(e) = set.write_to(&dir) {
            eprintln!("mep-fixtures: {}: {e}", dir.display());
            return ExitCode::FAILURE;
        }
        let t = &set.truth;
        println!(
            "seed {s}: {} sheets, {} duct and {} pipe segments, {} fittings, {} terminals, {} equipment, {} clashes ({} planted) -> {}",
            t.sheets.len(),
            t.segments_of("duct").count(),
            t.segments_of("pipe").count(),
            t.nodes.iter().filter(|n| ["elbow", "tee", "cross", "transition", "reducer"].contains(&n.kind.as_str())).count(),
            t.terminals.len(),
            t.equipment.len(),
            t.clashes.len(),
            t.clashes.iter().filter(|c| c.planted).count(),
            dir.display()
        );
    }
    ExitCode::SUCCESS
}
