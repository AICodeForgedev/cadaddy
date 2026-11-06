//! `cadaddy-cli accuracy` (Q6193): a model measured into one row keyed by its commit, appended to
//! the trend file and compared with the row before; exit status 2 when a metric regressed.

use std::path::PathBuf;
use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cadaddy-cli")).args(args).output().unwrap()
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cc-accuracy-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn s(p: &std::path::Path) -> String {
    p.display().to_string()
}

#[test]
fn a_worse_build_exits_2_and_the_same_build_again_exits_0() {
    let d = dir("trend");
    let dxf = d.join("plan.dxf");
    let model = d.join("plan.cdy");
    assert!(cli(&["sample", "floorplan", &s(&dxf)]).status.success());
    assert!(cli(&["convert", &s(&dxf), &s(&model)]).status.success());
    // The build before had far more elements than this one: a stable count that fell.
    let trend = d.join("trend.jsonl");
    std::fs::write(&trend, "{\"schema\":\"cadaddy.accuracy/1\",\"commit\":\"1111111aaaa\",\"metrics\":{\"elements.total\":100000}}\n").unwrap();
    let config = d.join("config.json");
    std::fs::write(&config, "{\"label\": \"Project A\", \"rooms\": [\"901\"], \"last\": 5}").unwrap();
    let table = d.join("trend.md");
    let out = cli(&[
        "accuracy",
        &s(&model),
        "--config",
        &s(&config),
        "--commit",
        "2222222bbbb",
        "--job-s",
        "60",
        "--trend",
        &s(&trend),
        "--table",
        &s(&table),
    ]);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(2), "{stdout}\n{stderr}");
    let row: serde_json::Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    assert_eq!((row["commit"].as_str(), row["label"].as_str()), (Some("2222222bbbb"), Some("Project A")));
    assert_eq!(row["metrics"]["build.job_s"], 60.0);
    assert!(stderr.contains("REGRESSION elements.total: fell 100000 -> "), "{stderr}");
    assert!(std::fs::read_to_string(&table).unwrap().contains("| 2222222b |"));
    // The same build again: nothing worse (the table exists, so --force).
    let out =
        cli(&["accuracy", &s(&model), "--config", &s(&config), "--commit", "3333333cccc", "--trend", &s(&trend), "--table", &s(&table), "--force"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(std::fs::read_to_string(&trend).unwrap().lines().count(), 3);
    // Without --trend: only the row.
    let out = cli(&["accuracy", &s(&model), "--commit", "4444444"]);
    assert!(out.status.success() && String::from_utf8_lossy(&out.stdout).lines().count() == 1);
    // Mistakes are refused, never a panic.
    let m = s(&model);
    let m = m.as_str();
    for bad in [vec!["accuracy"], vec!["accuracy", m, "--typo"], vec!["accuracy", m, "--job-s", "fast"], vec!["accuracy", m, "other.cdy"]] {
        let out = cli(&bad);
        assert_eq!(out.status.code(), Some(1), "{bad:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    let _ = std::fs::remove_dir_all(&d);
}
