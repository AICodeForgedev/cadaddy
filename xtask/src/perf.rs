//! `cargo xtask perf [--full] [--update-budget]`: the performance budgets.
//!
//! Runs the engine's `perfbench` example (the dev profile CI builds anyway) over generated
//! drawings — 1k and 10k objects, and with `--full` also 100k and 500k — timing open and save as
//! DWG/DXF/.cdy, regen, select all, move all, zoom extents, 3D build and plot to PDF. Each time is
//! compared with `perf/budget.json` (today's numbers plus 25 %, from `--update-budget`), under
//! this machine's platform (`macos-aarch64`, `linux-x86_64`…: each records its own; a platform
//! with none only gets "no budget" until `--update-budget` runs there): over the budget is a
//! warning, over twice the budget fails. Times under 5 ms are compared with 5 ms
//! (timer noise). Every run is appended to `target/perf/history.jsonl` (the trend).

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

/// Times below this many milliseconds are compared as this (scheduling noise).
const FLOOR_MS: f64 = 5.0;
/// The budget is today's time plus this share.
const MARGIN: f64 = 1.25;

/// How a time compares with its budget.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    Ok,
    Warn,
    Fail,
    NoBudget,
}

pub fn verdict(ms: f64, budget: Option<f64>) -> Verdict {
    let Some(b) = budget else { return Verdict::NoBudget };
    let limit = b.max(FLOOR_MS);
    if ms > 2.0 * limit {
        Verdict::Fail
    } else if ms > limit {
        Verdict::Warn
    } else {
        Verdict::Ok
    }
}

fn read_json(path: &Path) -> BTreeMap<String, f64> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<BTreeMap<String, f64>>(&t).ok()).unwrap_or_default()
}

/// This machine's key in the budget file.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

pub fn run(root: &Path, rest: &[&str]) -> Result<(), String> {
    let full = rest.contains(&"--full");
    let update = rest.contains(&"--update-budget");
    if let Some(bad) = rest.iter().find(|a| !["--full", "--update-budget"].contains(*a)) {
        return Err(format!("unknown option `{bad}` (perf [--full] [--update-budget])"));
    }
    let sizes = if full { "1000,10000,100000,500000" } else { "1000,10000" };
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), std::path::PathBuf::from);
    let dir = target.join("perf");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let last = dir.join("last.json");
    let mut c = crate::cargo();
    c.args(["run", "-q", "-p", "cadaddy-engine", "--example", "perfbench", "--", "--sizes", sizes, "--out"]).arg(&last);
    crate::run(c, &format!("perfbench --sizes {sizes}"))?;
    let results = read_json(&last);
    if results.is_empty() {
        return Err("perfbench wrote no results".into());
    }
    let budget_path = root.join("perf").join("budget.json");
    let mut all: BTreeMap<String, BTreeMap<String, f64>> =
        std::fs::read_to_string(&budget_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let here = platform();
    let mut budget = all.remove(&here).unwrap_or_default();
    let (mut warn, mut fail) = (Vec::new(), Vec::new());
    println!("\nbudget: {here}\n{:<24} {:>10} {:>10}  verdict", "step", "ms", "budget");
    for (k, ms) in &results {
        let b = budget.get(k).copied();
        let v = verdict(*ms, b);
        let mark = match v {
            Verdict::Ok => "ok",
            Verdict::Warn => "OVER BUDGET",
            Verdict::Fail => "FAIL (over 2x)",
            Verdict::NoBudget => "no budget",
        };
        println!("{k:<24} {ms:>10.1} {:>10}  {mark}", b.map_or_else(|| "-".into(), |b| format!("{b:.1}")));
        match v {
            Verdict::Warn => warn.push(k.clone()),
            Verdict::Fail => fail.push(k.clone()),
            _ => {}
        }
    }
    // The trend: one line per run.
    let rev = std::process::Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let line = json!({ "time": when, "rev": rev, "platform": here, "results": results }).to_string();
    let history = dir.join("history.jsonl");
    let mut text = std::fs::read_to_string(&history).unwrap_or_default();
    text.push_str(&line);
    text.push('\n');
    let _ = std::fs::write(&history, text);
    if update {
        for (k, ms) in &results {
            budget.insert(k.clone(), (ms * MARGIN * 10.0).round() / 10.0);
        }
        all.insert(here.clone(), budget);
        let v: Value = serde_json::to_value(&all).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(budget_path.parent().unwrap_or(root)).map_err(|e| e.to_string())?;
        std::fs::write(&budget_path, serde_json::to_string_pretty(&v).map_err(|e| e.to_string())? + "\n").map_err(|e| e.to_string())?;
        println!("budget for {here} updated: {}", budget_path.display());
        return Ok(());
    }
    if !warn.is_empty() {
        println!("warning: over budget: {}", warn.join(", "));
    }
    if fail.is_empty() { Ok(()) } else { Err(format!("over twice the budget: {}", fail.join(", "))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts() {
        assert_eq!(verdict(10.0, Some(10.0)), Verdict::Ok);
        assert_eq!(verdict(12.0, Some(10.0)), Verdict::Warn);
        assert_eq!(verdict(21.0, Some(10.0)), Verdict::Fail);
        assert_eq!(verdict(4.0, Some(0.5)), Verdict::Ok, "under the 5 ms floor");
        assert_eq!(verdict(9.0, Some(0.5)), Verdict::Warn, "over the floor, under twice it");
        assert_eq!(verdict(11.0, Some(0.5)), Verdict::Fail, "over twice the floor");
        assert_eq!(verdict(1.0, None), Verdict::NoBudget);
    }
}
