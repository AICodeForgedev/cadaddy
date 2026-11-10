//! Opening a large .cdy file: where the time and the memory go, and that opening never holds the
//! drawing's data twice (Q6398). Before, every chunk of the file was inflated first and all of
//! it was held while the objects were built from it: on a print-set model the inflated element
//! data (about 1.1 KB an object) sat beside the finished objects until the read was over.
//!
//! The model is synthetic, in the mix a large job model has: plan linework and text, block
//! definitions and their references, and BIM elements (3D boxes carrying element properties).
//!
//! A program of its own: it counts every allocation (a global allocator).
//!
//! `cargo test -p cadaddy-io --test cdy_open_memory -- --ignored --nocapture` runs the
//! million-object reproduction and prints its breakdown.

use std::sync::{Mutex, MutexGuard};

use cadaddy_doc::Drawing;

#[path = "support/synthetic_model.rs"]
mod synthetic_model;
use synthetic_model::model;

#[global_allocator]
static ALLOC: cadaddy_memcount::Counting = cadaddy_memcount::Counting::new();

/// One test at a time: they read the process's allocation peak.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Megabyte.
const MB: usize = 1 << 20;
/// `f`'s result, the most bytes it held at once above what was held before it, and the bytes
/// still held after it above that (what it returned).
fn measured<T>(f: impl FnOnce() -> T) -> (T, usize, usize) {
    let before = ALLOC.live();
    ALLOC.reset_peak();
    let r = f();
    (r, ALLOC.peak().saturating_sub(before), ALLOC.live().saturating_sub(before))
}

/// The file of a model of `n` objects, and how many bytes its parts inflate to.
fn file(n: usize) -> (Vec<u8>, u64) {
    let bytes = cadaddy_io::cdy::write_cdy_checkpoint(&model(n)).unwrap();
    let v = cadaddy_io::cdy::verify(&bytes).unwrap();
    let raw = v["sizes"].as_object().unwrap().values().map(|s| s["raw"].as_u64().unwrap()).sum();
    (bytes, raw)
}

/// Read `bytes` on `threads` cores: the drawing, its report, peak and held bytes, and the time.
fn open(bytes: &[u8], threads: usize) -> ((Drawing, cadaddy_io::cdy::ReadReport), usize, usize, std::time::Duration) {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let t = std::time::Instant::now();
    let (r, peak, held) = measured(|| pool.install(|| cadaddy_io::cdy::read_cdy_report(bytes).unwrap()));
    (r, peak, held, t.elapsed())
}

/// The drawing's objects are read once: the peak while opening is the opened drawing plus a few
/// chunks in flight, not the drawing plus the whole file inflated.
#[test]
fn opening_holds_the_drawing_once_not_the_file_inflated_beside_it() {
    let _g = serial();
    let n = 60_000;
    let (bytes, raw) = file(n);
    // On one core: one chunk in flight. Each more core adds one chunk's value tree (about 6 MB
    // here, ~12 times its inflated bytes), which is the same however large the file is.
    let ((d, rep), peak, held, _) = open(&bytes, 1);
    assert!(rep.notes.is_empty(), "{:?}", rep.notes);
    assert_eq!(d.model.len(), n);
    assert!(rep.profile.raw_peak < raw / 20, "{} KB inflated held at once", rep.profile.raw_peak / 1024);
    // Inflated, the objects are several times the file: holding all of it is the fault.
    assert!(raw as usize > 8 * MB, "the parts inflate to {} MB", raw / MB as u64);
    // Before, it held more than the whole file inflated over the drawing (33 MB over a 39 MB
    // drawing whose parts inflate to 26 MB); now one chunk in flight (7 MB, its value tree).
    let over = peak.saturating_sub(held);
    assert!(
        over < raw as usize / 2,
        "opening held {} MB over the {} MB drawing it made (the parts inflate to {} MB)",
        over / MB,
        held / MB,
        raw / MB as u64
    );
}

/// Four times the objects: what opening holds over the drawing stays a few chunks in flight.
#[test]
fn what_opening_holds_over_the_drawing_does_not_grow_with_the_file() {
    let _g = serial();
    let mut over = Vec::new();
    for n in [15_000, 60_000] {
        let (bytes, _) = file(n);
        let (_, peak, held, _) = open(&bytes, 2);
        over.push(peak.saturating_sub(held));
    }
    let [small, big] = over[..] else { panic!("two sizes") };
    assert!(big < small + 16 * MB, "{} MB over the drawing for the small file, {} MB for four times it", small / MB, big / MB);
}

/// The reproduction at the size of a print-set model: a million objects. Prints the breakdown.
#[test]
#[ignore = "a million objects: several GB and a minute; run on demand"]
fn a_million_object_model_opens_with_its_breakdown() {
    let _g = serial();
    let n = 1_000_000;
    let t = std::time::Instant::now();
    let (bytes, raw) = file(n);
    let made = t.elapsed();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let ((d, rep), peak, held, wall) = open(&bytes, threads);
    assert!(rep.notes.is_empty(), "{:?}", rep.notes);
    assert_eq!(d.model.len(), n);
    println!(
        "model: {n} objects, file {} MB, parts inflate to {} MB ({} bytes an object); made and written in {made:.1?}",
        bytes.len() / MB,
        raw / MB as u64,
        raw / n as u64
    );
    println!(
        "open on {threads} cores: {wall:.2?}; peak {} MB over before, drawing once open {} MB, peak over the drawing {} MB",
        peak / MB,
        held / MB,
        peak.saturating_sub(held) / MB
    );
    println!("{:#?}", rep.profile);
    assert!(peak.saturating_sub(held) < raw as usize / 4, "opening held {} MB over the drawing", peak.saturating_sub(held) / MB);
}
