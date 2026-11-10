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
