//! `cargo xtask perf [--full] [--update-budget]`: the performance budgets.
//!
//! Runs the engine's `perfbench` example (the dev profile CI builds anyway) over generated
//! drawings — 1k and 10k objects, and with `--full` also 100k, 500k and a million — timing open
//! and save as DWG/DXF/.cdy, regen, select all, move all, undo, zoom extents, 3D build, plot to
//! PDF and close, with each size's peak memory (`peak_mb@size`, MB); with `--full`, from 100k
//! objects on, also memory measured in fresh processes: the bytes one object holds in the
//! drawing (`mem_bytes_per_object`) and with its session and display list
//! (`mem_session_bytes_per_object`), and opening the size's DWG: the most memory the open took
//! (`mem_open_dwg_peak_mb`, MB) and what the open drawing holds (`mem_open_dwg_live_mb`). Each
//! number is compared with `perf/budget.json` (today's numbers plus 25 %, from `--update-budget`), under
//! this machine's platform (`macos-aarch64`, `linux-x86_64`…: each records its own; a platform
//! with none only gets "no budget" until `--update-budget` runs there): over the budget is a
//! warning, over twice the budget fails. Times under 5 ms are compared with 5 ms
//! (timer noise). With `--full`, a step whose million-object time is over twice ten times its
//! 100k time grows faster than the drawing: reported, and a failure. Every run is appended to
//! `target/perf/history.jsonl` (the trend).
//!
//! The assembled BIM pipeline runs too (`cadaddy-mepfix`'s `bimbench` example): every stage on
//! the synthetic hotel's print set — project import, PDF to .cdy, sheet classification, each
//! print reader, the whole job's model build, validation, review, takeoff, IFC and BCF export,
//! rendering, .cdy save and open, spec intake, the viewer, an agent-session soak, the MCP tools
//! and a revision re-import — as `bim_<stage>@full` (with `--full` also
//! `@double`, twice the hotel), with the run's peak memory, .cdy and IFC sizes and the largest
//! command answer an agent reads (`resp_kb`), all budgeted like the rest. The whole job is built
//! as one model (every level and discipline, its element counts as `elems_*`), then validated,
//! reviewed, taken off, exported, viewed, soaked for leaks (`leak_mb`) and driven through the MCP
//! tools (`mcp_<tool>_ms`/`_kb`). Every stage is checked as well as timed: a stage whose command
//! errors or whose key result is empty, a revision that doesn't change exactly one sheet, or a
//! double hotel under 1.8x the full hotel's elements fails the run ([`bim_outcome`]). Its
//! `cpu_cores` (how many cores were busy) is reported, not judged. `--bim` runs only the
//! pipeline; with `--update-budget` it re-baselines only the `bim` keys (see `docs/bim-perf.md`).
//!
//! `cargo xtask perf --vs [--sizes 10000,100000,1000000]`: CADaddy's side of the comparison with
//! other programs (`docs/perf-vs.md`): writes the 2D plans (`target/perf/vs/plan-<n>.dxf`), then
//! opens each in a fresh process (`perfbench --file`) and prints open, select-all, zoom-extents
//! and save times with the process's peak memory, as the table's rows; then measures pan frame
//! time per size (`perfbench --pan`, 100 steps rasterised on the CPU at 1600 x 900).
//! The competitor scripts are in `tools/perf-vs/` (QCAD, FreeCAD).

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

/// Steps whose time at a million objects is over twice ten times their time at 100k (growing
/// faster than the drawing does), as (step, ratio of the 1M time to ten times the 100k time).
/// Times at 100k under the noise floor are compared as the floor.
pub fn superlinear(results: &BTreeMap<String, f64>) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for (k, big) in results {
        let Some(op) = k.strip_suffix("@1000000") else { continue };
        // Memory isn't time (bytes per object stay flat; an open's memory grows with the file).
        if op.starts_with("peak_mb") || op.starts_with("mem_") {
            continue;
        }
        let Some(small) = results.get(&format!("{op}@100000")) else { continue };
        let ratio = big / (10.0 * small.max(FLOOR_MS));
        if ratio > 2.0 {
            out.push((op.to_string(), (ratio * 100.0).round() / 100.0));
        }
    }
    out
}

/// The double hotel's model has at least this many times the full hotel's elements.
pub const BIM_DOUBLE_MIN_RATIO: f64 = 1.8;

/// Whether the BIM pipeline run passed: bimbench exits non-zero when a stage's command errors or
/// its key result is empty (its FAILED lines say which), it must have written results, and the
/// double hotel's model must have nearly twice the full hotel's elements.
pub fn bim_outcome(ran: Result<(), String>, results: &BTreeMap<String, f64>) -> Result<(), String> {
    ran.map_err(|e| format!("{e}: a BIM pipeline stage failed its check (see the FAILED lines above)"))?;
    if results.is_empty() {
        return Err("bimbench wrote no results".into());
    }
    if let (Some(full), Some(double)) = (results.get("elems_total@bim_full"), results.get("elems_total@bim_double"))
        && *double < BIM_DOUBLE_MIN_RATIO * full
    {
        return Err(format!("the double hotel's model has {double} elements, under {BIM_DOUBLE_MIN_RATIO}x the full hotel's {full}"));
    }
    Ok(())
}

/// Whether `key` is one of the BIM pipeline's results for hotel `size` (`bim_<stage>@full`,
/// `peak_mb@bim_full`).
pub fn is_bim_key(key: &str, size: &str) -> bool {
    (key.starts_with("bim_") && key.ends_with(&format!("@{size}"))) || key.ends_with(&format!("@bim_{size}"))
}

/// Whether a result has a budget: everything but the cores-busy figure.
pub fn judged(key: &str) -> bool {
    !key.starts_with("cpu_cores@")
}

/// This machine's key in the budget file.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

pub fn run(root: &Path, rest: &[&str]) -> Result<(), String> {
    if rest.contains(&"--vs") {
        return run_vs(root, rest);
    }
    let full = rest.contains(&"--full");
    let update = rest.contains(&"--update-budget");
    let bim_only = rest.contains(&"--bim");
    if let Some(bad) = rest.iter().find(|a| !["--full", "--update-budget", "--bim"].contains(*a)) {
        return Err(format!("unknown option `{bad}` (perf [--full] [--bim] [--update-budget] | perf --vs [--sizes N,…])"));
    }
    let sizes = if full { "1000,10000,100000,500000,1000000" } else { "1000,10000" };
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), std::path::PathBuf::from);
    let dir = target.join("perf");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut results = BTreeMap::new();
    if !bim_only {
        let last = dir.join("last.json");
        let mut c = crate::cargo();
        c.args(["run", "-q", "-p", "cadaddy-engine", "--example", "perfbench", "--", "--sizes", sizes, "--out"]).arg(&last);
        crate::run(c, &format!("perfbench --sizes {sizes}"))?;
        results = read_json(&last);
        if results.is_empty() {
            return Err("perfbench wrote no results".into());
        }
    }
    // The assembled BIM pipeline on the synthetic hotel (and twice it with --full).
    let bim_sizes = if full { "full,double" } else { "full" };
    let bim = dir.join("bim.json");
    let _ = std::fs::remove_file(&bim);
    let mut c = crate::cargo();
    c.args(["run", "-q", "-p", "cadaddy-mepfix", "--example", "bimbench", "--", "--sizes", bim_sizes, "--out"]).arg(&bim);
    let ran = crate::run(c, &format!("bimbench --sizes {bim_sizes}"));
    let bim_results = read_json(&bim);
    bim_outcome(ran, &bim_results)?;
    results.extend(bim_results);
    let budget_path = root.join("perf").join("budget.json");
    let mut all: BTreeMap<String, BTreeMap<String, f64>> =
        std::fs::read_to_string(&budget_path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let here = platform();
    let mut budget = all.remove(&here).unwrap_or_default();
    let (mut warn, mut fail) = (Vec::new(), Vec::new());
    println!("\nbudget: {here}\n{:<24} {:>10} {:>10}  verdict", "step", "ms", "budget");
    for (k, ms) in &results {
        let b = budget.get(k).copied();
        // How many cores were busy: more is not worse.
        let v = if judged(k) { verdict(*ms, b) } else { Verdict::Ok };
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
        // BIM keys the pipeline no longer emits (a stage renamed or merged) go with the re-baseline.
        for size in bim_sizes.split(',') {
            budget.retain(|k, _| results.contains_key(k) || !is_bim_key(k, size));
        }
        for (k, ms) in results.iter().filter(|(k, _)| judged(k)) {
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
    // A million against 100k: anything growing more than twice as fast as the drawing.
    let steep = superlinear(&results);
    for (op, ratio) in &steep {
        println!("finding: {op} at 1M takes {ratio}x ten times its 100k time");
    }
    fail.extend(steep.into_iter().map(|(op, _)| format!("{op} (grows faster than the drawing)")));
    if fail.is_empty() { Ok(()) } else { Err(format!("over twice the budget: {}", fail.join(", "))) }
}

/// The peak resident memory (MB) in `/usr/bin/time`'s report: `-l` on macOS and the BSDs prints
/// bytes ("maximum resident set size"), GNU `-v` kilobytes ("Maximum resident set size (kbytes)").
pub fn peak_mb(report: &str) -> Option<f64> {
    report.lines().find_map(|l| {
        let l = l.trim();
        if let Some(kb) = l.strip_prefix("Maximum resident set size (kbytes):") {
            return kb.trim().parse::<f64>().ok().map(|k| k / 1024.0);
        }
        let n = l.strip_suffix("maximum resident set size")?;
        n.trim().parse::<f64>().ok().map(|b| b / 1_048_576.0)
    })
}

fn run_vs(root: &Path, rest: &[&str]) -> Result<(), String> {
    let sizes = rest.iter().position(|a| *a == "--sizes").and_then(|i| rest.get(i + 1)).copied().unwrap_or("10000,100000,1000000");
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), std::path::PathBuf::from);
    let dir = target.join("perf").join("vs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut c = crate::cargo();
    c.args(["build", "-q", "--release", "-p", "cadaddy-engine", "--example", "perfbench"]);
    crate::run(c, "build perfbench")?;
    let bench = target.join("release").join("examples").join(if cfg!(windows) { "perfbench.exe" } else { "perfbench" });
    let mut c = std::process::Command::new(&bench);
    c.args(["--sizes", sizes, "--plans"]).arg(&dir);
    crate::run(c, "perfbench --plans")?;
    let flag = if cfg!(target_os = "linux") { "-v" } else { "-l" };
    println!("\n| objects | open (ms) | select all (ms) | zoom extents (ms) | save DXF (ms) | peak MB |\n|---|---|---|---|---|---|");
    let mut pan_rows = Vec::new();
    for n in sizes.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let plan = dir.join(format!("plan-{n}.dxf"));
        let out = std::process::Command::new("/usr/bin/time")
            .arg(flag)
            .arg(&bench)
            .arg("--file")
            .arg(&plan)
            .output()
            .map_err(|e| format!("/usr/bin/time: {e}"))?;
        let r: Value = serde_json::from_slice(String::from_utf8_lossy(&out.stdout).trim().as_bytes()).unwrap_or(Value::Null);
        let ms = |k: &str| r[k].as_f64().map_or_else(|| "-".into(), |v| format!("{v:.0}"));
        let peak = peak_mb(&String::from_utf8_lossy(&out.stderr)).map_or_else(|| "-".into(), |m| format!("{m:.0}"));
        println!("| {n} | {} | {} | {} | {} | {peak} |", ms("open"), ms("select_all"), ms("zoom_extents"), ms("save_dxf"));
        // Pan frame time: rasterise 100 pan steps on the CPU (conservative upper bound; GPU is
        // faster since it only changes a uniform).
        let pan_out = std::process::Command::new(&bench)
            .arg("--pan")
            .arg(&plan)
            .output()
            .map_err(|e| format!("perfbench --pan: {e}"))?;
        let pr: Value = serde_json::from_slice(String::from_utf8_lossy(&pan_out.stdout).trim().as_bytes()).unwrap_or(Value::Null);
        let frame = pr["pan_frame_ms"].as_f64().map_or_else(|| "-".into(), |v| format!("{v:.2}"));
        let fps = pr["pan_fps"].as_f64().map_or_else(|| "-".into(), |v| format!("{v:.0}"));
        pan_rows.push((n.to_string(), frame, fps));
    }
    println!("\n| objects | program | pan frame (ms) | pan FPS |\n|---|---|---|---|");
    for (n, frame, fps) in &pan_rows {
        println!("| {n} | **CADaddy** | {frame} | {fps} |");
    }
    println!("| any | QCAD | not measured (by hand) | |");
    println!("| any | FreeCAD | not measured (by hand) | |");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_memory_from_both_time_reports() {
        let bsd = "        7.21 real         6.10 user\n          3116335104  maximum resident set size\n";
        assert_eq!(peak_mb(bsd).map(f64::round), Some(2972.0));
        let gnu = "\tMaximum resident set size (kbytes): 524288\n";
        assert_eq!(peak_mb(gnu), Some(512.0));
        assert_eq!(peak_mb("nothing"), None);
    }

    #[test]
    fn busy_cores_are_reported_not_judged() {
        assert!(!judged("cpu_cores@bim_full"));
        assert!(judged("bim_import@bim_full") && judged("peak_mb@bim_full") && judged("resp_kb@bim_full") && judged("regen@1000"));
    }

    #[test]
    fn a_failed_bim_stage_fails_perf() {
        let ok: BTreeMap<String, f64> = [("bim_build@full", 50.0), ("elems_total@bim_full", 1000.0), ("elems_total@bim_double", 1900.0)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        assert_eq!(bim_outcome(Ok(()), &ok), Ok(()));
        // bimbench exits non-zero when a stage errors or comes back empty (its results are still
        // written): perf fails.
        let failed = bim_outcome(Err("bimbench --sizes full: exited with exit status: 1".into()), &ok);
        assert!(failed.as_ref().is_err_and(|e| e.contains("failed its check")), "{failed:?}");
        assert!(bim_outcome(Ok(()), &BTreeMap::new()).is_err(), "no results");
        let mut small = ok.clone();
        small.insert("elems_total@bim_double".into(), 1200.0);
        assert!(bim_outcome(Ok(()), &small).is_err_and(|e| e.contains("1200")));
        small.remove("elems_total@bim_double");
        assert_eq!(bim_outcome(Ok(()), &small), Ok(()), "full only: nothing to compare");
    }

    #[test]
    fn bim_keys_are_told_by_size() {
        assert!(is_bim_key("bim_build@full", "full") && is_bim_key("elems_mep@bim_full", "full") && is_bim_key("leak_mb@bim_full", "full"));
        assert!(!is_bim_key("bim_build@double", "full") && !is_bim_key("regen@1000", "full") && !is_bim_key("peak_mb@1000", "full"));
    }

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

    #[test]
    fn steps_growing_faster_than_the_drawing_are_found() {
        let r: BTreeMap<String, f64> = [
            ("regen@100000", 40.0),
            ("regen@1000000", 420.0),
            ("move_all@100000", 100.0),
            ("move_all@1000000", 2500.0),
            ("zoom_extents@100000", 0.5),
            ("zoom_extents@1000000", 60.0),
            ("peak_mb@100000", 100.0),
            ("peak_mb@1000000", 5000.0),
            ("mem_open_dwg_peak_mb@100000", 200.0),
            ("mem_open_dwg_peak_mb@1000000", 4500.0),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
        // 2.5x is a finding; 1.05x is not; 60 ms against the 5 ms floor is 1.2x; memory isn't time.
        assert_eq!(superlinear(&r), [("move_all".to_string(), 2.5)]);
    }
}
