//! One `validate.trend` call costs the same however long the project's trend file is (Q6479).
//!
//! A trend file grows by one row per measured build for the life of a job; a call that appends a
//! row or reads the table used to read and parse the whole file, so each call grew slower with
//! the history (17 ms -> 114 ms over 1,500 rows in a soak). A synthetic trend file of 1,000,
//! 10,000 and 100,000 rows is timed here: after the first call (which builds the index beside the
//! file once), an append and a read answer within a fixed bound at every size.
//!
//! A program of its own: it asserts wall-clock times.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cadaddy_engine::Session;
use serde_json::{Value, json};

/// The fixed bound one call (append or read) answers in at 100,000 rows, in a debug build.
const BOUND: Duration = Duration::from_millis(20);

fn row(i: usize) -> Value {
    json!({
        "schema": "cadaddy.accuracy/1",
        "commit": format!("{:07x}{i:08}", i % 0xfff_ffff),
        "at": "2026-01-01T00:00:00Z",
        "label": "made-up job",
        "metrics": {
            "gate_a.host_pass_pct": 90.0 + (i % 7) as f64,
            "gate_a.host_fail_far": (i % 13) as f64,
            "gate_a.height_pass_pct": 80.0 + (i % 5) as f64,
            "elements.total": 1000.0 + (i % 11) as f64,
            "elements.receptacle": 200.0,
            "takeoff.electrical/power.ft": 5000.0 + (i % 17) as f64,
        },
        "detail": {"errors": []},
    })
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-trendspeed-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// The median time of an append and of a read on a trend file of `n` rows, after one warm call.
fn time_calls(n: usize) -> (Duration, Duration, Value) {
    let dir = scratch(&n.to_string());
    let trend = dir.join("trend.jsonl");
    let mut text = String::with_capacity(n * 400);
    for i in 0..n {
        text.push_str(&row(i).to_string());
        text.push('\n');
    }
    std::fs::write(&trend, text).unwrap();
    let mut s = Session::new();
    let warm = s.execute("validate.trend", &json!({"path": trend})).unwrap();
    assert_eq!(warm["rows"].as_u64(), Some(n as u64), "{}", warm["note"]);
    let mut appends = Vec::new();
    let mut reads = Vec::new();
    let mut last = Value::Null;
    for k in 0..5 {
        let t = Instant::now();
        let r = s.execute("validate.trend", &json!({"path": trend, "row": row(n + k)})).unwrap();
        appends.push(t.elapsed());
        assert_eq!(r["rows"].as_u64(), Some((n + k + 1) as u64));
        let t = Instant::now();
        last = s.execute("validate.trend", &json!({"path": trend})).unwrap();
        reads.push(t.elapsed());
    }
    let _ = std::fs::remove_dir_all(&dir);
    (median(appends), median(reads), last)
}

#[test]
fn one_call_costs_the_same_at_any_length_of_history() {
    let mut table = Vec::new();
    for n in [1_000usize, 10_000, 100_000] {
        let (append, read, last) = time_calls(n);
        table.push(format!("{n:>7} rows: append {append:?}, read {read:?}, index {}", last["index"]));
        if n == 100_000 {
            eprintln!("{}", table.join("\n"));
            assert!(append < BOUND && read < BOUND, "at 100,000 rows one call is over {BOUND:?}:\n{}", table.join("\n"));
        }
    }
}
