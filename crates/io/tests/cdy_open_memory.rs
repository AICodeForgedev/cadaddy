//! Opening and saving a large .cdy file: where the time and the memory go. Opening never holds
//! the drawing's data twice (Q6398); a full save's memory over the drawing's is bounded, whatever
//! the drawing's size (Q6635, at the end).
//!
//! Opening (Q6398). Before, every chunk of the file was inflated first and all of
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

/// A chunk's objects are built as its tokens are read (Q6399): opening holds the objects and the
/// chunk's inflated bytes, not a token list and a value tree of the chunk beside them. Before,
/// one chunk in flight held about twelve times its inflated bytes.
#[test]
fn a_chunk_is_built_from_its_tokens_without_a_value_tree_beside_it() {
    let _g = serial();
    // Four model space chunks, read on one core: one chunk in flight at a time.
    let n = 4 * 1024;
    let (bytes, raw) = file(n);
    let ((d, rep), peak, held, _) = open(&bytes, 1);
    assert!(rep.notes.is_empty(), "{:?}", rep.notes);
    assert_eq!(d.model.len(), n);
    let over = peak.saturating_sub(held);
    let chunk = raw as usize / 4;
    println!("{n} objects: drawing {} KB, peak over it {} KB, one chunk inflates to about {} KB", held / 1024, over / 1024, chunk / 1024);
    assert!(over < 2 * chunk, "opening held {} KB over the {} KB drawing; one chunk inflates to about {} KB", over / 1024, held / 1024, chunk / 1024);
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

/// Any .cdy file opened the same way, named by `CADADDY_PROFILE_CDY` (a real model kept outside
/// the repository): prints its open time, peak memory and breakdown. Does nothing without it.
#[test]
#[ignore = "opens a file named by CADADDY_PROFILE_CDY; run on demand"]
fn a_model_file_opens_with_its_breakdown() {
    let _g = serial();
    let Some(path) = std::env::var_os("CADADDY_PROFILE_CDY") else { return };
    let bytes = std::fs::read(path).unwrap();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let ((d, rep), peak, held, wall) = open(&bytes, threads);
    println!(
        "file {} MB, {} model space objects, {} notes; open on {threads} cores: {wall:.2?}; peak {} MB, drawing once open {} MB, peak over the drawing {} MB",
        bytes.len() / MB,
        d.model.len(),
        rep.notes.len(),
        peak / MB,
        held / MB,
        peak.saturating_sub(held) / MB
    );
    println!("{:#?}", rep.profile);
    // What was read, as a fingerprint of the drawing saved again (the writer is deterministic):
    // equal before and after a reader change means the same drawing.
    use std::hash::{Hash, Hasher};
    let mut h = std::hash::DefaultHasher::new();
    cadaddy_io::cdy::write_cdy_checkpoint(&d).unwrap().hash(&mut h);
    println!("drawing read, saved again: fingerprint {:016x}", h.finish());
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

// Saving (Q6635). A print set's full save with its preview took 1-2 GB over the model: the preview
// built the whole display list for a 256 × 192 image; each compressed chunk kept room for half its
// raw size until the file was made; each chunk being encoded was its objects as one value tree.

/// A full save holds the file it makes and a few chunks being encoded: four times the model adds
/// the bigger file and nothing more.
#[test]
fn a_full_save_holds_a_bounded_working_set_besides_the_file_it_makes() {
    let _g = serial();
    let mut runs = Vec::new();
    for n in [10_000, 40_000] {
        let d = model(n);
        let (bytes, peak, _) = measured(|| cadaddy_io::cdy::write_cdy_checkpoint(&d).unwrap());
        // What the save held besides the file it returns.
        let extra = peak.saturating_sub(bytes.capacity());
        assert_eq!(cadaddy_io::cdy::read_cdy(&bytes).unwrap().model.len(), d.model.len());
        runs.push((bytes.len(), extra));
    }
    let [(small_file, small), (big_file, big)] = runs[..] else { panic!("two sizes") };
    eprintln!("full save: {small_file} B file, {small} B besides it; {big_file} B file, {big} B besides it");
    assert!(big_file > 3 * small_file, "the bigger model's file: {big_file} bytes, the smaller's {small_file}");
    assert!(big < small + 4 * MB, "{} MB besides the file for a model, {} MB for four times it", small / MB, big / MB);
    // The few chunks in flight as tokens: as value trees, four of them held ~50 MB here.
    assert!(big < 16 * MB, "the save held {} MB besides its file", big / MB);
}

/// The preview is drawn a part of the display list at a time: four times the model adds a little
/// (the drawing's object list, and the stroke path of a run of polylines drawn alike, which the
/// whole list's image joins too), not four times the list (the whole list of the bigger model here
/// is over 50 MB).
#[test]
fn a_saves_preview_is_drawn_without_the_whole_display_list() {
    let _g = serial();
    let mut held = Vec::new();
    for n in [10_000, 40_000] {
        let d = model(n);
        let (png, peak, _) = measured(|| cadaddy_io::cdy::preview(&d));
        assert!(png.unwrap().starts_with(b"\x89PNG"));
        held.push(peak);
    }
    let [small, big] = held[..] else { panic!("two sizes") };
    eprintln!("preview: {small} B for a model, {big} B for four times it");
    assert!(big < small + 8 * MB, "{} MB for a model, {} MB for four times it", small / MB, big / MB);
}

/// Drawn a part at a time, the preview is the image the whole display list draws.
#[test]
fn the_preview_is_the_one_the_whole_display_list_draws() {
    let _g = serial();
    let d = model(20_000);
    let png = cadaddy_io::cdy::preview(&d).unwrap();
    let (w, h) = cadaddy_io::cdy::THUMBNAIL_SIZE;
    let ext = d.extents(&cadaddy_doc::Space::Model);
    let px = (ext.width() / f64::from(w)).max(ext.height() / f64::from(h)) * 1.1;
    let opts = cadaddy_render::Options { transparency: true, tolerance: px * 0.5, min_dash: px * 2.0, ..cadaddy_render::Options::default() };
    let whole = cadaddy_io::png_with(&d, &cadaddy_doc::Space::Model, w, h, &opts).unwrap();
    assert!(png == whole, "the preview drawn a part at a time differs from the whole list's");
}
