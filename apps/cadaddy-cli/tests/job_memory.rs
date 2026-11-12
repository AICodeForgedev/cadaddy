//! A job's peak memory is what it holds, not what the allocator keeps for the machine's threads
//! (Q6363). glibc's malloc gives allocating threads arenas of their own (up to 8 per core) and
//! keeps what a thread frees in its arena, so the same job on a many-core Linux machine peaked at
//! 2.6 times what it did on Windows, most of it gained in the stages that read the sheets on the
//! worker threads. Here a made-up set, its sheets as dense as a real print set's (small fills and
//! hatch strokes are most of a real sheet's objects), goes through the job's read stages twice on
//! the same worker threads: as the CLI runs, and with glibc held to one arena
//! (`MALLOC_ARENA_MAX=1`, which other allocators ignore). One arena keeps no memory for other
//! threads, so its peak is what the job holds; the CLI's may not be much above it.
//!
//! Run by hand (3-4 minutes in release; about 10 with glibc's malloc):
//! `cargo test --release -p cadaddy-cli --test job_memory -- --ignored --nocapture`.

#![cfg(all(target_os = "linux", target_env = "gnu"))]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use cadaddy_engine::doc::{Common, Drawing, EntityKind, Line, Solid, Space};
use cadaddy_engine::geom::Vec3;
use cadaddy_mepfix::hotel::HotelSize;
use cadaddy_mepfix::{draw, hotel_planned, layout, pdf_options};

/// The worker threads of both runs (the machines the finding was measured on had 12-24).
const THREADS: usize = 16;
/// How far the CLI's peak may be above the one-arena peak: a factor and a few MB.
const FACTOR: f64 = 1.3;
const SLACK_MB: u64 = 64;
/// Sheets in the set, and the small fills and hatch strokes added to each (a real sheet holds
/// 10,000-60,000 objects).
const SHEETS: usize = 24;
const FILLS: usize = 40_000;
const STROKES: usize = 20_000;

/// `n` marks in a grid over `lo..hi`, each `mark(x, y, dx, dy)` in its cell.
fn grid(d: &mut Drawing, layer: &str, n: usize, lo: [f64; 2], hi: [f64; 2], mark: impl Fn(f64, f64, f64, f64) -> EntityKind) {
    let side = (n as f64).sqrt().ceil() as usize;
    let (dx, dy) = ((hi[0] - lo[0]) / side as f64, (hi[1] - lo[1]) / side as f64);
    let common = Common { layer: layer.into(), ..Common::default() };
    for i in 0..side {
        for j in 0..side {
            d.add(&Space::Model, common.clone(), mark(lo[0] + dx * i as f64, lo[1] + dy * j as f64, dx, dy)).unwrap();
        }
    }
}

/// A made-up job folder: the first sheets of a made-up hotel, each its own PDF, with a field of
/// small fills and short 45° strokes (as hatching comes out of a PDF) beside the plan.
fn job_folder(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    let (t, st) = hotel_planned(7, HotelSize::Small);
    let kinds = layout::hotel_sheet_list();
    for sh in t.sheets.iter().take(SHEETS) {
        let Some((.., kind)) = kinds.iter().find(|k| k.0 == sh.number) else { continue };
        let (mut d, ..) = draw::draw(&t, &st, sh, kind);
        let [w, h] = sh.paper_in;
        // inside the frame and clear of the title block
        let (lo, hi) = (layout::paper_to_model(sh, [w * 0.1, h * 0.15]), layout::paper_to_model(sh, [w * 0.7, h * 0.85]));
        grid(&mut d, "A-FILL", FILLS, lo, hi, |x, y, dx, dy| {
            let (a, b) = (dx * 0.5, dy * 0.5);
            let p = |u: f64, v: f64| Vec3::new(x + u, y + v, 0.0);
            EntityKind::Solid(Solid { corners: [p(0.0, 0.0), p(a, 0.0), p(0.0, b), p(a, b)] })
        });
        grid(&mut d, "A-PATT", STROKES, lo, hi, |x, y, dx, dy| {
            EntityKind::Line(Line { a: Vec3::new(x, y, 0.0), b: Vec3::new(x + dx * 0.6, y + dy * 0.6, 0.0) })
        });
        let pdf = cadaddy_io::pdf::pdf(&d, &Space::Model, &pdf_options(sh, &st)).unwrap();
        std::fs::write(dir.join(&sh.pdf), pdf).unwrap();
    }
}

/// The job on `prints` on [`THREADS`] worker threads with `env` set, stopped once its rooms stage
/// is done; the most resident memory it had by then, MB: the most of its VmHWM read while it runs
/// and what its own stage line says.
fn read_stages_peak_mb(work: &Path, prints: &Path, label: &str, env: &[(&str, &str)]) -> u64 {
    let cmds = work.join(format!("job-{label}.jsonl"));
    let params = serde_json::json!({"pdfs": prints.display().to_string(), "goal": "takeoff", "auto": true, "checkpoint": false});
    std::fs::write(&cmds, format!("job.start {params}\njob.status {{}}\n")).unwrap();
    let err = work.join(format!("job-{label}.err"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_cadaddy-cli"))
        .args(["run", "--cmd-file"])
        .arg(&cmds)
        .env("RAYON_NUM_THREADS", THREADS.to_string())
        .env_remove("MALLOC_ARENA_MAX")
        .envs(env.iter().copied())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&err).unwrap())
        .spawn()
        .unwrap();
    let status = format!("/proc/{}/status", child.id());
    let mut hwm = 0;
    let mut log = String::new();
    while child.try_wait().unwrap().is_none() {
        if let Some(kb) = std::fs::read_to_string(&status)
            .ok()
            .and_then(|s| s.lines().find_map(|l| l.strip_prefix("VmHWM:")).and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok()))
        {
            hwm = hwm.max(kb / 1024);
        }
        log = std::fs::read_to_string(&err).unwrap_or_default();
        if log.contains("rooms: done") {
            child.kill().unwrap();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.wait();
    let done =
        log.lines().find(|l| l.contains("rooms: done")).unwrap_or_else(|| panic!("the {label} job stopped before its rooms stage was done:\n{log}"));
    let imported = log.matches(" imported (").count();
    assert!(imported >= SHEETS / 2, "the {label} job imported {imported} sheets:\n{log}");
    let said = done.split("peak ").nth(1).and_then(|r| r.split(" MB").next()?.parse::<u64>().ok()).unwrap_or(0);
    eprintln!("{label}: {done} (VmHWM {hwm} MB, {imported} sheets imported)");
    hwm.max(said)
}

#[test]
#[ignore = "reads a made-up print set twice (3-10 minutes in release); run by hand"]
fn job_peak_is_what_it_holds_not_the_thread_arenas() {
    let work: PathBuf = std::env::temp_dir().join(format!("cadaddy-jobmem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let prints = work.join("prints");
    job_folder(&prints);
    let cli = read_stages_peak_mb(&work, &prints, "as-run", &[]);
    let one_arena = read_stages_peak_mb(&work, &prints, "one-arena", &[("MALLOC_ARENA_MAX", "1")]);
    let bound = (one_arena as f64 * FACTOR) as u64 + SLACK_MB;
    assert!(cli <= bound, "the job's read stages peaked at {cli} MB on {THREADS} threads, {one_arena} MB in one malloc arena (bound {bound} MB)");
    let _ = std::fs::remove_dir_all(&work);
}
