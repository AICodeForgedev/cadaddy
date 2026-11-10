//! OPEN of a large .cdy file, step by step (Q6398): reading the file, the .cdy read (inflating,
//! decoding, building objects, assembling the drawing), matching the file for incremental saves,
//! everything the session does once the drawing is read (styles, table values, the view,
//! fields, xrefs, the activity log), and the first full display list, each with its time and the
//! memory it holds at its peak. Opening must not hold the drawing's data twice: its peak stays
//! close to the drawing once open.
//!
//! A program of its own: it installs the file hooks and counts every allocation (a global
//! allocator).
//!
//! `cargo test -p cadaddy-engine --test cdy_open_profile -- --ignored --nocapture` prints the
//! breakdown for a million objects.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{CdyHooks, CdyState, CdyWrite, IoHooks, read_file, set_cdy_hooks, set_io};
use cadaddy_engine::doc::Space;

#[path = "../../io/tests/support/synthetic_model.rs"]
mod synthetic_model;

#[global_allocator]
static ALLOC: cadaddy_memcount::Counting = cadaddy_memcount::Counting::new();

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

const MB: usize = 1 << 20;

/// As the apps install them.
fn hooks() {
    set_io(IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    set_cdy_hooks(CdyHooks {
        save: |d, prev| {
            let prev = prev.and_then(|p| p.as_ref().downcast_ref::<cadaddy_io::cdy::SaveState>());
            let (out, state) = cadaddy_io::cdy::save(d, prev, &|| None)?;
            let out = match out {
                cadaddy_io::cdy::Output::Full(bytes) => CdyWrite::Full(bytes),
                cadaddy_io::cdy::Output::Append { from, bytes, index_at } => CdyWrite::Append { from, bytes, index_at },
            };
            Ok((out, Arc::new(state)))
        },
        seed: |d, bytes| cadaddy_io::cdy::seed(d, bytes).map(|s| Arc::new(s) as CdyState),
    });
}

/// One step of opening: its time, its peak above what was held before it, and what it still
/// holds after.
struct Step {
    name: &'static str,
    time: Duration,
    peak: usize,
    held: isize,
}

fn step<T>(steps: &mut Vec<Step>, name: &'static str, f: impl FnOnce() -> T) -> T {
    let before = ALLOC.live();
    ALLOC.reset_peak();
    let t = Instant::now();
    let r = f();
    let time = t.elapsed();
    steps.push(Step { name, time, peak: ALLOC.peak().saturating_sub(before), held: ALLOC.live() as isize - before as isize });
    r
}

/// What opening a model of `n` objects costs: the steps, the drawing's bytes once open, and the
/// peak while opening (file read to drawing open in the session) above what was held before.
struct Opened {
    steps: Vec<Step>,
    drawing: usize,
    peak: usize,
    profile: cadaddy_io::cdy::OpenProfile,
}

/// Open a model of `n` objects with the parts done chunk by chunk on `threads` cores.
fn open(n: usize, tag: &str, threads: usize) -> Opened {
    hooks();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let dir = std::env::temp_dir().join(format!("cadaddy-cdy-open-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Synthetic Model.cdy");
    std::fs::write(&path, cadaddy_io::cdy::write_cdy_checkpoint(&synthetic_model::model(n)).unwrap()).unwrap();
    let path = path.to_string_lossy().into_owned();
    let mut s = Session::new();
    let mut steps = Vec::new();

    // The read alone, for its own breakdown (then let go).
    let bytes = std::fs::read(&path).unwrap();
    let (d, rep) = step(&mut steps, "cdy read alone (inflate, decode, construct, assemble)", || {
        pool.install(|| cadaddy_io::cdy::read_cdy_report(&bytes).unwrap())
    });
    let profile = rep.profile.clone();
    step(&mut steps, "matching the file for incremental saves (seed)", || drop(cadaddy_io::cdy::seed(&d, &bytes)));
    drop((d, rep, bytes));

    // OPEN as the app does it: the slow half off the session, then the fast half.
    let before = ALLOC.live();
    let read = step(&mut steps, "OPEN, read half (file, cdy read, seed)", || pool.install(|| read_file(&path).unwrap()));
    let read_peak = steps.last().map_or(0, |st| st.peak);
    let mid = ALLOC.live().saturating_sub(before);
    let opened = step(&mut steps, "OPEN, session half (styles, values, view, fields, xrefs, log)", || s.finish_open(read).unwrap());
    let peak = read_peak.max(mid + steps.last().map_or(0, |st| st.peak));
    let drawing = ALLOC.live().saturating_sub(before);
    assert!(opened["warnings"].as_array().is_none_or(Vec::is_empty), "{opened}");
    assert_eq!(s.doc().unwrap().model.len(), n);
    let list = step(&mut steps, "first display list, full detail (the UI builds its own at screen size)", || {
        cadaddy_engine::render::build(s.doc().unwrap(), &Space::Model, &cadaddy_engine::render::Options::default())
    });
    drop(list);
    let _ = std::fs::remove_dir_all(&dir);
    Opened { steps, drawing, peak, profile }
}

fn print(n: usize, o: &Opened) {
    println!(
        "OPEN of a {n}-object synthetic .cdy ({} chunks, {} MB inflated): peak {} MB above before, drawing once open {} MB",
        o.profile.chunks,
        o.profile.raw_bytes / MB as u64,
        o.peak / MB,
        o.drawing / MB
    );
    println!("| step | time | peak MB | held after MB |");
    for st in &o.steps {
        println!("| {} | {:.2?} | {} | {} |", st.name, st.time, st.peak / MB, st.held / MB as isize);
    }
    println!("{:#?}", o.profile);
}

/// The whole OPEN, file to drawing in the session, peaks close to the drawing once open (before:
/// the file inflated was held beside the objects built from it, twice the drawing).
#[test]
fn opening_a_cdy_peaks_close_to_the_drawing_once_open() {
    let _g = serial();
    let n = 60_000;
    // On one core: one chunk in flight (each more core adds one chunk's value tree, a few MB
    // however large the file is).
    let o = open(n, "small", 1);
    print(n, &o);
    assert!(o.peak < o.drawing + o.drawing / 3, "opening peaked at {} MB for a {} MB drawing", o.peak / MB, o.drawing / MB);
}

/// The reproduction at the size of a print-set model: a million objects. Prints the breakdown.
#[test]
#[ignore = "a million objects: several GB and a minute; run on demand"]
fn a_million_object_model_opens_with_its_breakdown() {
    let _g = serial();
    let n = 1_000_000;
    let o = open(n, "large", std::thread::available_parallelism().map_or(4, |n| n.get()));
    print(n, &o);
    assert!(o.peak < o.drawing + o.drawing / 4, "opening peaked at {} MB for a {} MB drawing", o.peak / MB, o.drawing / MB);
}

// ─── A print set opened one sheet at a time (Q6418) ──────────────────────────────────────────

/// What the interface held and took to open a print set, before (every sheet read and kept, a
/// sheet open on screen: Q6416) and after (sheets on demand), and that every count is the same.
///
/// `cargo test -p cadaddy-engine --test cdy_open_profile print_set_on_demand -- --ignored --nocapture`
/// (`SHEETS`, `PER`: the made-up set's size, 100 x 10 000 by default; `PRINT_SET_CDY`: a .cdy print
/// set of your own instead, read only: it is copied and saved again with sheet sections first).
#[test]
#[ignore = "a measurement: run by hand (see the doc comment)"]
fn print_set_on_demand_breakdown() {
    let _g = serial();
    hooks();
    let n = |k: &str, d: usize| std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
    let dir = std::env::temp_dir().join(format!("cadaddy-ondemand-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("set.cdy").to_string_lossy().into_owned();
    // The set saved as a headless session (a job) saves it: with its sheet sections.
    let t = Instant::now();
    {
        let mut s = Session::new();
        match std::env::var("PRINT_SET_CDY").ok() {
            Some(src) => {
                s.execute("open", &serde_json::json!({ "path": src })).unwrap();
            }
            None => {
                s.open_drawing(cadaddy_engine::sample::job_print_set(n("SHEETS", 100), n("PER", 10_000)), "Sample Set", None);
            }
        }
        s.execute("saveas", &serde_json::json!({ "path": path })).unwrap();
    }
    let saved = t.elapsed();
    let bytes = std::fs::metadata(&path).unwrap().len();
    println!("\nset saved with sheet sections in {saved:.2?}: {:.1} MB", bytes as f64 / MB as f64);
    let readings = |s: &mut Session| -> Vec<serde_json::Value> {
        let mut run = |c: &str, p: serde_json::Value| s.execute(c, &p).unwrap_or_else(|e| panic!("{c}: {e}"));
        vec![
            run("entities", serde_json::json!({"limit": 1}))["count"].clone(),
            run("count", serde_json::json!({"block": "SYM-OUTLET", "space": "model"})),
            run("bim.tree", serde_json::json!({}))["total"].clone(),
        ]
    };
    // BEFORE: read whole (as the interface read every file), first sheet open.
    let base = ALLOC.live();
    ALLOC.reset_peak();
    let t = Instant::now();
    let mut before = Session::new();
    before.viewport_px = (1600.0, 1000.0);
    before.finish_open(read_file(&path).unwrap()).unwrap();
    before.open_first_sheet();
    let (b_time, b_peak, b_held) = (t.elapsed(), ALLOC.peak().saturating_sub(base), ALLOC.live().saturating_sub(base));
    let b_objects = before.doc().unwrap().model.len();
    let sheets = before.print_sheets().len();
    let t = Instant::now();
    for _ in 0..3 {
        before.execute("sheet.next", &serde_json::json!({})).unwrap();
    }
    let b_step = t.elapsed() / 3;
    let want = readings(&mut before);
    drop(before);
    // AFTER: sheets on demand, as the interface opens it now.
    let base = ALLOC.live();
    ALLOC.reset_peak();
    let t = Instant::now();
    let mut after = Session::new();
    after.viewport_px = (1600.0, 1000.0);
    after.sheets_on_demand = true;
    after.finish_open(cadaddy_engine::cmd::file::read_file_on_demand(&path).unwrap()).unwrap();
    after.open_first_sheet();
    let (a_time, a_peak, a_held) = (t.elapsed(), ALLOC.peak().saturating_sub(base), ALLOC.live().saturating_sub(base));
    let a_objects = after.doc().unwrap().model.len();
    let parked = after.doc().unwrap().sheet_store.parked_objects();
    let t = Instant::now();
    for _ in 0..3 {
        after.execute("sheet.next", &serde_json::json!({})).unwrap();
    }
    let a_step = t.elapsed() / 3;
    let a_step_held = ALLOC.live().saturating_sub(base);
    // A one-line edit on the open sheet, saved.
    let c = after.active_sheet().map(|(_, s)| s.rect.center()).unwrap();
    after.execute("line", &serde_json::json!({ "points": [[c.x, c.y], [c.x + 40.0, c.y]] })).unwrap();
    let t = Instant::now();
    let r = after.execute("qsave", &serde_json::json!({})).unwrap();
    let a_save = t.elapsed();
    let grew = std::fs::metadata(&path).unwrap().len().saturating_sub(bytes);
    // The whole set, for a command that reads it (loaded in place by the engine's guard).
    let t = Instant::now();
    after.execute("undo", &serde_json::json!({})).unwrap();
    let got = readings(&mut after);
    let a_whole = t.elapsed();
    assert_eq!(got, want, "the same counts with the sheets on demand");
    println!("{sheets} sheets; counts identical before and after: {want:?}");
    println!("| | before (whole set held, one sheet shown) | after (sheets on demand) |");
    println!("|---|---|---|");
    println!("| open to the first sheet | {b_time:.2?} | {a_time:.2?} |");
    println!("| peak memory while opening | {} MB | {} MB |", b_peak / MB, a_peak / MB);
    println!("| held once open | {} MB | {} MB |", b_held / MB, a_held / MB);
    println!("| objects in memory | {b_objects} | {a_objects} (+ {parked} parked as stored) |");
    println!("| Page Down (next sheet) | {b_step:.2?} | {a_step:.2?} (held after 3: {} MB) |", a_step_held / MB);
    println!("| QSAVE after a one-line edit | n/a | {a_save:.2?}, file +{grew} bytes, incremental {} |", r["incremental"]);
    println!("| a whole-set command (load every sheet + counts) | n/a | {a_whole:.2?} |");
    let _ = std::fs::remove_dir_all(&dir);
}
