//! `cargo xtask mep-fixtures [--seed N] [--count K] [--out DIR]`: made-up MEP print sets
//! (mechanical, plumbing, fire protection, ceiling and structural sheets as vector PDF and DXF)
//! with a JSON ground truth per set, for scoring print readers (crates/mepfix).

use std::path::Path;

pub fn run(root: &Path, rest: &[&str]) -> Result<(), String> {
    let mut args: Vec<String> = rest.iter().map(|a| (*a).to_string()).collect();
    if !args.iter().any(|a| a == "--out") {
        let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), std::path::PathBuf::from);
        args.push("--out".into());
        args.push(target.join("mep-fixtures").display().to_string());
    }
    let mut c = crate::cargo();
    c.args(["run", "-q", "-p", "cadaddy-mepfix", "--bin", "mep-fixtures", "--"]).args(&args);
    crate::run(c, &format!("mep-fixtures {}", args.join(" ")))
}
