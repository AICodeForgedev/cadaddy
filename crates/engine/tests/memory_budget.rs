//! Memory comes back: opening and closing 200 drawings (each drawn, edited, saved as DXF, DWG or
//! CDY, opened again and plotted now and then) and running 10,000 commands leaves the process
//! within a budget of where it was after the same work once before, and nothing process-wide
//! keeps a closed drawing's objects (Q240).
//!
//! The work runs in passes of 100 drawings and 5,000 commands. The first three passes are the
//! warm-up: they load fonts, patterns and codecs. Freed memory isn't always handed back to the
//! system: on Windows, once in a while a 5,000-command drawing closed leaves about 45 MB with the
//! allocator, which every later pass reuses (a step, once). So after four more passes the memory
//! must be within [`STEP_MB`] of where the warm-up left it, and within [`BUDGET_MB`] of where the
//! first measured pass left it: a leak grows with every pass.
//!
//! One test in its own binary: the process's memory is the measurement, and no other test may run
//! beside it. The process memory is its private bytes on Windows, anonymous resident memory on
//! Linux and resident memory on macOS; where none can be read, the structural checks still run.

use std::path::{Path, PathBuf};
use std::sync::Weak;

use cadaddy_engine::Session;
use cadaddy_engine::doc::{Entity, Space};
use serde_json::json;

/// Growth allowed from the first measured pass to the last, in MB.
const BUDGET_MB: f64 = 16.0;
/// Growth allowed from the warm-up's end to the last pass, in MB (the allocator's step included).
const STEP_MB: f64 = 64.0;
/// Drawings opened and closed, and commands run, in one pass.
const DRAWINGS: usize = 100;
const COMMANDS: usize = 5_000;
/// Passes of warm-up, and passes measured.
const WARM_UP: usize = 3;
const PASSES: usize = 4;

fn hooks() {
    use cadaddy_engine::cmd::file::{CdyHooks, CdyState, CdyWrite, IoHooks, set_cdy_hooks, set_io, set_pdf_merge};
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: Some(|d, space, opts| cadaddy_io::plotter::plot(d, space, opts).map_err(|e| e.to_string())),
        read_pdf: Some(cadaddy_io::pdf_read::read_json),
    });
    set_cdy_hooks(CdyHooks {
        save: |d, prev| {
            let prev = prev.and_then(|p| p.as_ref().downcast_ref::<cadaddy_io::cdy::SaveState>());
            let (out, state) = cadaddy_io::cdy::save(d, prev, &|| cadaddy_io::cdy::preview(d))?;
            let out = match out {
                cadaddy_io::cdy::Output::Full(bytes) => CdyWrite::Full(bytes),
                cadaddy_io::cdy::Output::Append { from, bytes, index_at } => CdyWrite::Append { from, bytes, index_at },
            };
            Ok((out, std::sync::Arc::new(state)))
        },
        seed: |d, bytes| cadaddy_io::cdy::seed(d, bytes).map(|s| std::sync::Arc::new(s) as CdyState),
    });
    set_pdf_merge(|parts| cadaddy_io::pdf_merge::merge(parts).map_err(|e| e.to_string()));
}

/// This process's memory in MB, when the system says.
fn memory_mb() -> Option<f64> {
    let pid = std::process::id();
    if cfg!(windows) {
        let script = format!("(Get-Process -Id {pid}).PrivateMemorySize64");
        let out = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().ok()?;
        return String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok().map(|b| b / 1_048_576.0);
    }
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        let kb = status.lines().find_map(|l| l.strip_prefix("RssAnon:"))?.trim().trim_end_matches("kB").trim().parse::<f64>().ok()?;
        return Some(kb / 1024.0);
    }
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok().map(|kb| kb / 1024.0)
}

fn first_entity(s: &Session) -> Weak<Entity> {
    let store = s.doc().unwrap().space(&Space::Model).unwrap();
    std::sync::Arc::downgrade(store.iter().next().unwrap())
}

/// One drawing's life: drawn, edited, saved, closed, opened again, plotted (every fifth), closed.
/// A weak reference to one of its objects as opened from the file, to show nothing keeps it.
fn cycle(s: &mut Session, n: usize, dir: &Path) -> Weak<Entity> {
    s.execute("new", &json!({})).unwrap();
    for i in 0..20 {
        let (x, y) = ((i % 5) as f64 * 12.0, (i / 5) as f64 * 9.0);
        s.execute("line", &json!({ "points": [[x, y], [x + 10.0, y], [x + 10.0, y + 6.0]] })).unwrap();
        s.execute("circle", &json!({ "center": [x + 5.0, y + 3.0], "radius": 2 })).unwrap();
    }
    s.execute("hatch", &json!({ "points": [[5.0, 3.0]], "pattern": "ANSI31", "scale": 2 })).unwrap();
    s.execute("text", &json!({ "at": [0, -5], "text": format!("Cycle {n}"), "height": 1.5 })).unwrap();
    s.execute("mtext", &json!({ "at": [0, -10], "text": "Two\\Plines", "height": 1 })).ok();
    s.cmdline("MOVE L  0,0 3,3").ok();
    s.cmdline("U").ok();
    let file = dir.join(format!("budget-{n}.{}", ["dxf", "dwg", "cdy"][n % 3]));
    s.execute("saveas", &json!({ "path": file })).unwrap();
    s.execute("close", &json!({})).unwrap();
    s.execute("open", &json!({ "path": file })).unwrap();
    let weak = first_entity(s);
    if n.is_multiple_of(5) {
        let pdf = dir.join(format!("budget-{n}.pdf"));
        s.execute("plot", &json!({ "path": pdf })).unwrap();
        let _ = std::fs::remove_file(pdf);
    }
    s.execute("close", &json!({})).unwrap();
    let _ = std::fs::remove_file(file);
    weak
}

/// `count` commands in one drawing (drawing, moving, undoing), then the drawing closed.
fn commands(s: &mut Session, count: usize) {
    s.execute("new", &json!({})).unwrap();
    for i in 0..count {
        let (x, y) = ((i % 50) as f64 * 3.0, (i / 50 % 50) as f64 * 3.0);
        match i % 5 {
            0 | 1 => drop(s.execute("line", &json!({ "points": [[x, y], [x + 2.0, y + 1.0]] })).unwrap()),
            2 => drop(s.execute("circle", &json!({ "center": [x, y], "radius": 1 })).unwrap()),
            3 => s.cmdline("MOVE L  0,0 0.5,0").unwrap(),
            _ => s.cmdline("U").unwrap(),
        }
    }
    s.execute("close", &json!({})).unwrap();
}

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-memory-budget-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn memory_returns_after_drawings_open_and_close() {
    hooks();
    let dir = temp_dir();
    let mut s = Session::new();
    let pass = |s: &mut Session, p: usize, kept: &mut Vec<Weak<Entity>>| {
        for n in p * DRAWINGS..(p + 1) * DRAWINGS {
            let weak = cycle(s, n, &dir);
            if n % 25 == 0 {
                kept.push(weak);
            }
        }
        commands(s, COMMANDS);
        memory_mb()
    };
    let mut kept = Vec::new();
    let base = (0..WARM_UP).map(|p| pass(&mut s, p, &mut kept)).last().flatten();
    let mut after = Vec::new();
    for p in WARM_UP..WARM_UP + PASSES {
        after.push(pass(&mut s, p, &mut kept));
    }
    let _ = std::fs::remove_dir_all(&dir);
    // Nothing process-wide holds a closed drawing's objects, or the journal more than its budget.
    assert!(kept.iter().all(|w| w.upgrade().is_none()), "a closed drawing's objects are still held");
    assert_eq!(cadaddy_render::images::copies_held(), 0, "image copies outlived their drawings");
    assert!(s.journal.bytes() <= cadaddy_engine::journal::MAX_BYTES);
    if let (Some(base), Some(Some(first)), Some(Some(end))) = (base, after.first(), after.last()) {
        eprintln!("memory: {base:.1} MB after the warm-up, then {after:.1?} MB after each pass of {DRAWINGS} drawings and {COMMANDS} commands");
        let work = format!("{} drawings opened and closed and {} commands", (PASSES - 1) * DRAWINGS, (PASSES - 1) * COMMANDS);
        assert!(end - first < BUDGET_MB, "memory grew {:.1} MB (from {first:.1} to {end:.1}) over {work}; the budget is {BUDGET_MB} MB", end - first);
        assert!(end - base < STEP_MB, "memory ended {:.1} MB above where the warm-up left it ({base:.1} MB); at most {STEP_MB} MB", end - base);
    }
}
