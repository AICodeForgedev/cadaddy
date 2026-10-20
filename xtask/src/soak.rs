//! `cargo xtask soak [--minutes N] [--mode headless|window|both] [--app PATH] [--cli PATH] [--out DIR]
//! [--profile release|dev]`:
//! a long-session soak. A scripted mixed workload runs for N minutes (default 2) against the
//! headless app (`cadaddy-cli mcp`) and a real window (`cadaddy --control PORT`): each round opens a
//! new drawing, draws, edits, undoes and redoes, saves, reopens and closes drawings, plots, and in
//! the window turns Walk on and off and floats and docks palettes; then it closes the drawing, so a
//! steady session holds the same memory round after round.
//!
//! Every interval (a minute; a tenth of a short run) it records the process's memory (RSS), its open
//! handles (file descriptors), and in the window the canvas's GPU buffer bytes and the frame time.
//! It fails on steady growth (a leak: up through the second half of the run, and by more than a
//! slack) or a stall (a call or a frame over 15 s). The report goes to `--out` (default
//! `target/soak`) as `report.md` and `samples.json`. CI runs the headless mode for 2 minutes; longer
//! runs and the window are manual.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::smoke::{Control, Mcp, free_port};

/// A call or a frame slower than this is a stall.
const STALL: Duration = Duration::from_secs(15);

#[derive(Debug, PartialEq)]
pub struct Opts {
    pub minutes: f64,
    pub headless: bool,
    pub window: bool,
    pub app: Option<PathBuf>,
    pub cli: Option<PathBuf>,
    pub out: Option<PathBuf>,
    /// The cargo profile the app is built with when not given: release, or dev (CI's: it reuses
    /// the test build's dependencies).
    pub release: bool,
}

pub fn parse(rest: &[&str]) -> Result<Opts, String> {
    let mut o = Opts { minutes: 2.0, headless: true, window: false, app: None, cli: None, out: None, release: true };
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().copied().ok_or_else(|| format!("{a} needs a value"));
        match *a {
            "--minutes" => {
                o.minutes = value()?.parse().map_err(|_| "--minutes needs a number".to_string())?;
                if !(0.1..=24.0 * 60.0).contains(&o.minutes) {
                    return Err("--minutes: 0.1 to 1440".into());
                }
            }
            "--mode" => {
                (o.headless, o.window) = match value()? {
                    "headless" => (true, false),
                    "window" => (false, true),
                    "both" => (true, true),
                    m => return Err(format!("--mode {m}: headless, window or both")),
                }
            }
            "--app" => o.app = Some(PathBuf::from(value()?)),
            "--cli" => o.cli = Some(PathBuf::from(value()?)),
            "--out" => o.out = Some(PathBuf::from(value()?)),
            "--profile" => {
                o.release = match value()? {
                    "release" => true,
                    "dev" => false,
                    p => return Err(format!("--profile {p}: release or dev")),
                }
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(o)
}

/// One measurement.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sample {
    pub minute: f64,
    pub rss_mb: f64,
    pub handles: u64,
    pub gpu_mb: Option<f64>,
    pub frame_ms: Option<f64>,
    pub rounds: u64,
    pub slowest_call_ms: f64,
}

/// A process's resident memory (MB) and open file descriptors.
fn process(pid: u32) -> (f64, u64) {
    let rss = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0);
    let fds = std::fs::read_dir(format!("/proc/{pid}/fd")).map(|d| d.count() as u64).ok().or_else(|| {
        let o = Command::new("lsof").args(["-n", "-P", "-p", &pid.to_string()]).stderr(Stdio::null()).output().ok()?;
        // Header line apart; memory-mapped files (txt, mem) aren't handles the app opened.
        Some(String::from_utf8_lossy(&o.stdout).lines().skip(1).filter(|l| !l.contains(" txt ") && !l.contains(" mem ")).count() as u64)
    });
    (rss, fds.unwrap_or(0))
}

/// Whether a series grows steadily: up from its first half to its second, the second half's
/// least-squares slope positive, and the growth over the run above `slack`. Short noisy series and
/// a one-off step (a cache filling once) don't count.
pub fn grows(values: &[f64], slack: f64) -> Option<String> {
    if values.len() < 6 {
        return None;
    }
    let half = values.len() / 2;
    let (first, second) = values.split_at(half);
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let n = second.len() as f64;
    let xm = (n - 1.0) / 2.0;
    let ym = mean(second);
    let (num, den) = second.iter().enumerate().fold((0.0, 0.0), |(a, b), (i, y)| (a + (i as f64 - xm) * (y - ym), b + (i as f64 - xm).powi(2)));
    let slope = if den > 0.0 { num / den } else { 0.0 };
    let growth = second.last().copied().unwrap_or(0.0) - first.first().copied().unwrap_or(0.0);
    let second_min = second.iter().copied().fold(f64::INFINITY, f64::min);
    let first_max = first.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    (slope > 0.0 && growth > slack && second_min > first_max).then(|| format!("grew {growth:.1} (+{slope:.2} per sample in the second half)"))
}

/// The workload's transport: the window's control channel or the headless MCP server.
trait Driver {
    fn exec(&mut self, command: &str, params: Value) -> Result<Value, String>;
    fn typed(&mut self, text: &str) -> Result<(), String>;
    fn save(&mut self, path: &Path) -> Result<(), String>;
    fn open(&mut self, path: &Path) -> Result<(), String>;
    fn plot(&mut self, path: &Path) -> Result<(), String>;
    /// Window only: Walk on and off, palettes floated and docked, a frame's time and GPU bytes.
    fn window_work(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn gpu(&mut self) -> (Option<f64>, Option<f64>) {
        (None, None)
    }
    fn pid(&self) -> u32;
}

struct Window {
    c: Control,
    pid: u32,
}

impl Driver for Window {
    fn exec(&mut self, command: &str, params: Value) -> Result<Value, String> {
        self.c.exec(command, params)
    }
    fn typed(&mut self, text: &str) -> Result<(), String> {
        self.c.call("cmdline.input", json!({ "text": text })).map(|_| ())
    }
    fn save(&mut self, path: &Path) -> Result<(), String> {
        self.c.exec("saveas", json!({ "path": path })).map(|_| ())
    }
    fn open(&mut self, path: &Path) -> Result<(), String> {
        self.c.exec("open", json!({ "path": path })).map(|_| ())
    }
    fn plot(&mut self, path: &Path) -> Result<(), String> {
        self.c.exec("plot", json!({ "path": path })).map(|_| ())
    }
    fn window_work(&mut self) -> Result<(), String> {
        self.c.exec("ui.walk", json!({}))?;
        std::thread::sleep(Duration::from_millis(300));
        self.c.exec("ui.walk", json!({}))?;
        for panel in ["ui.float.layers", "ui.float.properties"] {
            self.c.exec(panel, json!({}))?;
            std::thread::sleep(Duration::from_millis(150));
            self.c.exec(panel, json!({}))?;
        }
        Ok(())
    }
    fn gpu(&mut self) -> (Option<f64>, Option<f64>) {
        let perf = self.c.call("ui.inspect", json!({})).map(|v| v["perf"].clone()).unwrap_or(Value::Null);
        (perf["gpuBytes"].as_f64().map(|b| b / 1_048_576.0), perf["frameMs"].as_f64())
    }
    fn pid(&self) -> u32 {
        self.pid
    }
}

struct Headless(Mcp);

impl Driver for Headless {
    fn exec(&mut self, command: &str, params: Value) -> Result<Value, String> {
        self.0.tool("execute", json!({ "command": command, "params": params }))
    }
    fn typed(&mut self, text: &str) -> Result<(), String> {
        self.0.tool("command_line", json!({ "text": text })).map(|_| ())
    }
    fn save(&mut self, path: &Path) -> Result<(), String> {
        self.0.tool("save", json!({ "path": path })).map(|_| ())
    }
    fn open(&mut self, path: &Path) -> Result<(), String> {
        self.0.tool("open", json!({ "path": path })).map(|_| ())
    }
    fn plot(&mut self, path: &Path) -> Result<(), String> {
        self.0.tool("plot", json!({ "path": path, "layout": "Model" })).map(|_| ())
    }
    fn pid(&self) -> u32 {
        self.0.pid()
    }
}

/// One round of the workload, in a drawing of its own that is closed at the end.
fn round(d: &mut dyn Driver, n: u64, dir: &Path, slowest: &mut Duration) -> Result<(), String> {
    let mut timed = |d: &mut dyn Driver, what: &str, f: &mut dyn FnMut(&mut dyn Driver) -> Result<(), String>| -> Result<(), String> {
        let t = Instant::now();
        f(d).map_err(|e| format!("round {n}, {what}: {e}"))?;
        let took = t.elapsed();
        *slowest = (*slowest).max(took);
        if took > STALL { Err(format!("round {n}, {what}: stalled {:.1} s", took.as_secs_f64())) } else { Ok(()) }
    };
    timed(d, "new", &mut |d| d.exec("new", json!({})).map(|_| ()))?;
    timed(d, "draw", &mut |d| {
        for i in 0..40 {
            let (x, y) = (f64::from(i % 8) * 12.0, f64::from(i / 8) * 9.0);
            d.exec("line", json!({ "points": [[x, y], [x + 10.0, y], [x + 10.0, y + 6.0]] }))?;
            d.exec("circle", json!({ "center": [x + 5.0, y + 3.0], "radius": 2 }))?;
        }
        d.exec("hatch", json!({ "points": [[5.0, 3.0]], "pattern": "ANSI31", "scale": 2 }))?;
        d.exec("text", json!({ "at": [0, -5], "text": format!("Soak round {n}"), "height": 1.5 }))?;
        // The work really happened: 40 polylines' worth of lines, 40 circles, a hatch and a note.
        let drawn = d.exec("drawing.inspect", json!({ "entities": false }))?["entityCount"].as_u64().unwrap_or(0);
        if drawn < 82 { Err(format!("{drawn} objects drawn, expected at least 82")) } else { Ok(()) }
    })?;
    timed(d, "edit", &mut |d| {
        d.typed("MOVE L  0,0 3,3")?;
        d.typed("COPY ALL  0,0 0,60")?;
        d.typed("ERASE L ")?;
        d.typed("ROTATE L  0,0 15")?;
        Ok(())
    })?;
    timed(d, "undo/redo", &mut |d| {
        for _ in 0..6 {
            d.typed("U")?;
        }
        for _ in 0..3 {
            d.typed("REDO")?;
        }
        Ok(())
    })?;
    // A new file each round: an agent saving over a file waits for the person at the window to
    // confirm (and only they can turn that off).
    let file = dir.join(format!("soak-{n}.{}", ["dxf", "dwg", "cdy"][(n % 3) as usize]));
    timed(d, "save", &mut |d| d.save(&file))?;
    timed(d, "close and reopen", &mut |d| {
        d.exec("close", json!({}))?;
        d.open(&file)?;
        Ok(())
    })?;
    if n.is_multiple_of(3) {
        let pdf = dir.join(format!("soak-{n}.pdf"));
        timed(d, "plot", &mut |d| d.plot(&pdf))?;
        let _ = std::fs::remove_file(&pdf);
    }
    timed(d, "window", &mut |d| d.window_work())?;
    timed(d, "close", &mut |d| d.exec("close", json!({})).map(|_| ()))?;
    let _ = std::fs::remove_file(&file);
    Ok(())
}

/// Runs the workload against `d` for `minutes`, sampling; the samples and what went wrong.
fn soak(d: &mut dyn Driver, minutes: f64, dir: &Path) -> (Vec<Sample>, Vec<String>) {
    let start = Instant::now();
    let total = Duration::from_secs_f64(minutes * 60.0);
    let every = Duration::from_secs_f64((minutes * 60.0 / 10.0).clamp(5.0, 60.0));
    let mut samples = Vec::new();
    let mut problems = Vec::new();
    let mut next = Duration::ZERO;
    let mut rounds = 0;
    let mut slowest = Duration::ZERO;
    while start.elapsed() < total {
        if let Err(e) = round(d, rounds, dir, &mut slowest) {
            problems.push(e);
            break;
        }
        rounds += 1;
        if start.elapsed() >= next {
            let (rss_mb, handles) = process(d.pid());
            let (gpu_mb, frame_ms) = d.gpu();
            if frame_ms.is_some_and(|f| f > STALL.as_secs_f64() * 1000.0) {
                problems.push(format!("a frame took {:.1} s", frame_ms.unwrap_or_default() / 1000.0));
            }
            samples.push(Sample {
                minute: start.elapsed().as_secs_f64() / 60.0,
                rss_mb,
                handles,
                gpu_mb,
                frame_ms,
                rounds,
                slowest_call_ms: slowest.as_secs_f64() * 1000.0,
            });
            slowest = Duration::ZERO;
            next += every;
        }
    }
    let rss: Vec<f64> = samples.iter().map(|s| s.rss_mb).collect();
    // A leak: steady growth by more than 64 MB or a quarter of where it started.
    let base = rss.first().copied().unwrap_or(0.0);
    if let Some(g) = grows(&rss, (base * 0.25).max(64.0)) {
        problems.push(format!("memory {g} MB: a leak"));
    }
    let fds: Vec<f64> = samples.iter().map(|s| s.handles as f64).collect();
    if let Some(g) = grows(&fds, 16.0) {
        problems.push(format!("open handles {g}: a leak"));
    }
    let gpu: Vec<f64> = samples.iter().filter_map(|s| s.gpu_mb).collect();
    if let Some(g) = grows(&gpu, 32.0) {
        problems.push(format!("GPU buffers {g} MB: a leak"));
    }
    (samples, problems)
}

fn table(name: &str, samples: &[Sample], problems: &[String]) -> String {
    let mut s = format!("## {name}\n\n");
    let rounds = samples.last().map_or(0, |x| x.rounds);
    let _ = writeln!(s, "{rounds} rounds. {}\n", if problems.is_empty() { "PASS".to_string() } else { format!("FAIL: {}", problems.join("; ")) });
    s.push_str("| minute | rounds | RSS MB | handles | GPU MB | frame ms | slowest call ms |\n|---|---|---|---|---|---|---|\n");
    for x in samples {
        let opt = |v: Option<f64>| v.map_or("-".to_string(), |v| format!("{v:.1}"));
        let _ = writeln!(
            s,
            "| {:.1} | {} | {:.1} | {} | {} | {} | {:.0} |",
            x.minute,
            x.rounds,
            x.rss_mb,
            x.handles,
            opt(x.gpu_mb),
            opt(x.frame_ms),
            x.slowest_call_ms
        );
    }
    s.push('\n');
    s
}

/// A release build of `package`'s binary `bin`, made when not given.
fn built(root: &Path, given: &Option<PathBuf>, release: bool, package: &str, bin: &str) -> Result<PathBuf, String> {
    if let Some(p) = given {
        return Ok(p.clone());
    }
    let mut cargo = Command::new(env!("CARGO"));
    cargo.current_dir(root).args(["build", "-p", package, "--bin", bin]);
    if release {
        cargo.arg("--release");
    }
    let status = cargo.status().map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err(format!("building {bin}: {status}"));
    }
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from);
    Ok(target.join(if release { "release" } else { "debug" }).join(format!("{bin}{}", std::env::consts::EXE_SUFFIX)))
}

pub fn run(root: &Path, rest: &[&str]) -> Result<(), String> {
    let o = parse(rest)?;
    let out = o.out.clone().unwrap_or_else(|| root.join("target/soak"));
    let _ = std::fs::remove_dir_all(&out);
    let files = out.join("files");
    std::fs::create_dir_all(&files).map_err(|e| format!("{}: {e}", files.display()))?;
    let mut report = format!("# Soak, {} minute(s)\n\n", o.minutes);
    let mut all = json!({});
    let mut failed = Vec::new();
    if o.headless {
        let exe = built(root, &o.cli, o.release, "cadaddy-cli", "cadaddy-cli")?;
        let cfg = out.join("config-headless");
        std::fs::create_dir_all(&cfg).map_err(|e| e.to_string())?;
        let mut m = Mcp::start(&exe, &cfg)?;
        m.send("initialize", json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "xtask-soak", "version": "1" } }))?;
        let mut d = Headless(m);
        eprintln!("soak: headless ({}) for {} minute(s)…", exe.display(), o.minutes);
        let (samples, problems) = soak(&mut d, o.minutes, &files);
        report.push_str(&table("Headless (cadaddy-cli mcp)", &samples, &problems));
        all["headless"] = samples_json(&samples, &problems);
        failed.extend(problems.into_iter().map(|p| format!("headless: {p}")));
    }
    if o.window {
        let exe = built(root, &o.app, o.release, "cadaddy", "cadaddy")?;
        let cfg = out.join("config-window");
        std::fs::create_dir_all(&cfg).map_err(|e| e.to_string())?;
        let port = free_port()?;
        let log = std::fs::File::create(out.join("app.log")).map_err(|e| e.to_string())?;
        let mut child = Command::new(&exe)
            .arg("--control")
            .arg(port.to_string())
            .env("CADADDY_CONFIG_DIR", &cfg)
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .spawn()
            .map_err(|e| format!("{}: {e}", exe.display()))?;
        let result = Control::connect(port, &cfg, &mut child, Duration::from_secs(90)).map(|c| {
            let mut d = Window { c, pid: child.id() };
            eprintln!("soak: window ({}) for {} minute(s)…", exe.display(), o.minutes);
            soak(&mut d, o.minutes, &files)
        });
        let _ = child.kill();
        let _ = child.wait();
        let (samples, problems) = result.unwrap_or_else(|e| (Vec::new(), vec![e]));
        report.push_str(&table("Window (cadaddy --control)", &samples, &problems));
        all["window"] = samples_json(&samples, &problems);
        failed.extend(problems.into_iter().map(|p| format!("window: {p}")));
    }
    let _ = std::fs::write(out.join("report.md"), &report);
    let _ = std::fs::write(out.join("samples.json"), serde_json::to_string_pretty(&all).unwrap_or_default());
    let _ = std::fs::remove_dir_all(&files);
    println!("{report}");
    if failed.is_empty() { Ok(()) } else { Err(format!("soak failed:\n  {}", failed.join("\n  "))) }
}

fn samples_json(samples: &[Sample], problems: &[String]) -> Value {
    json!({
        "problems": problems,
        "samples": samples.iter().map(|s| json!({
            "minute": s.minute, "rounds": s.rounds, "rssMb": s.rss_mb, "handles": s.handles,
            "gpuMb": s.gpu_mb, "frameMs": s.frame_ms, "slowestCallMs": s.slowest_call_ms,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options() {
        assert_eq!(parse(&[]).unwrap(), Opts { minutes: 2.0, headless: true, window: false, app: None, cli: None, out: None, release: true });
        assert!(!parse(&["--profile", "dev"]).unwrap().release && parse(&["--profile", "x"]).is_err());
        let o = parse(&["--minutes", "30", "--mode", "both"]).unwrap();
        assert!(o.minutes == 30.0 && o.headless && o.window);
        assert!(parse(&["--minutes", "0"]).is_err() && parse(&["--mode", "x"]).is_err() && parse(&["--bogus"]).is_err());
    }

    #[test]
    fn steady_growth_is_a_leak_and_a_plateau_is_not() {
        let leak: Vec<f64> = (0..12).map(|i| 200.0 + f64::from(i) * 12.0).collect();
        assert!(grows(&leak, 64.0).is_some());
        // Warm-up then flat (a cache filling once), with noise.
        let flat = [150.0, 210.0, 240.0, 251.0, 249.0, 252.0, 250.0, 253.0, 251.0, 252.0, 250.0, 251.0];
        assert!(grows(&flat, 64.0).is_none());
        // Growing, but by less than the slack.
        let small: Vec<f64> = (0..12).map(|i| 200.0 + f64::from(i)).collect();
        assert!(grows(&small, 64.0).is_none());
        assert!(grows(&[1.0, 500.0], 1.0).is_none(), "too few samples to say");
    }
}
