//! A print set open one sheet at a time (Q6418) gives back what a whole-set load took once its
//! sheets are out of memory again (Q6571: every Count dialog open on a large set left about
//! 1.1 GB behind, 2.6 GB grew to 6.6 GB in four opens and never levelled off).
//!
//! Counted in the bytes the program holds (the process's resident size is the allocator's free
//! lists as much as the program's bytes): after each round of load, whole-set command, put-out,
//! the bytes held come back to about what they were before the first round.
//!
//! The counting allocator is the whole test binary's, so this is one test that runs its parts in
//! turn.
#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{CdyHooks, CdyWrite, IoHooks, set_cdy_hooks, set_io};
use cadaddy_engine::doc::Space;
use cadaddy_engine::geom::Vec2;
use serde_json::json;

#[global_allocator]
static ALLOC: cadaddy_memcount::Counting = cadaddy_memcount::Counting::new();

/// Sheets of the set and objects a sheet: large enough that a round's leftovers stand out from
/// the session's own bounded bookkeeping (the command log, the replay journal).
const SHEETS: usize = 8;
const PER_SHEET: usize = 6000;
/// Rounds of load and put-out.
const ROUNDS: usize = 5;

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
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
        seed: |d, bytes| cadaddy_io::cdy::seed(d, bytes).map(|s| Arc::new(s) as cadaddy_engine::cmd::file::CdyState),
    });
}

/// The bytes held once background threads (the undo thread frees dropped steps off the command
/// path) have let go, waiting up to a few seconds for them to come down to `want`.
fn settled(want: usize) -> usize {
    let mut live = ALLOC.live();
    for _ in 0..200 {
        if live <= want {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        live = ALLOC.live();
    }
    live
}

/// The session's own history, bounded and meant to outlive any load: not what this counts.
fn quiet(s: &mut Session) {
    s.log.clear();
    s.journal.clear();
}

/// Work on a worker thread as the interface runs it: a copy of the session on screen meanwhile
/// ([`Session::view_copy`], its frames going on: the cursor over the drawing), the worked-on
/// session put back after and the copy dropped.
fn on_worker(s: &mut Session, work: impl FnOnce(&mut Session) + Send + 'static) {
    let copy = s.view_copy();
    let mut w = std::mem::replace(s, copy);
    let worker = std::thread::spawn(move || {
        work(&mut w);
        w
    });
    hover(s);
    *s = worker.join().unwrap();
}

fn opened(path: &str) -> Session {
    let mut s = Session::new();
    s.viewport_px = (1200.0, 800.0);
    s.sheets_on_demand = true;
    let f = cadaddy_engine::cmd::file::read_file_on_demand(path).unwrap();
    s.finish_open(f).unwrap();
    assert!(s.open_first_sheet());
    assert!(s.has_parked_sheets());
    s
}

/// One round as the Count dialog runs it in the interface: the set loaded (off the UI thread
/// when `worker`), the counts read, frames with the cursor over the drawing, and once idle the
/// sheets put out again (off the UI thread when `worker`, the caches let go of once the session
/// is back, as the app does it). With `edit`, a line drawn while the set is in memory: an undo
/// step of the whole set.
fn round(s: &mut Session, worker: bool, edit: Option<usize>) {
    if worker {
        on_worker(s, |w| {
            w.load_all_sheets().unwrap();
        });
    }
    s.execute("count", &json!({"space": "model", "similar": true})).unwrap();
    s.execute("entities", &json!({"limit": 1})).unwrap();
    assert!(s.whole_set_in_memory());
    if let Some(k) = edit {
        let y = -50.0 - k as f64;
        s.execute("line", &json!({"points": [[0.0, y], [5.0, y]]})).unwrap();
    }
    // The frames meanwhile: the cursor picks through the spatial index of what is in memory,
    // the whole set now.
    hover(s);
    if worker {
        on_worker(s, |w| {
            assert!(w.park_idle_sheets_off_thread().unwrap());
        });
        s.release_unheld();
    } else {
        assert!(s.park_idle_sheets().unwrap());
    }
    assert!(s.has_parked_sheets() && !s.whole_set_in_memory());
    // And the frames after: the open sheet's objects.
    hover(s);
}

/// A frame with the cursor over the drawing: what is under it, picked.
fn hover(s: &Session) {
    let d = s.doc().unwrap();
    let _ = cadaddy_engine::select::pick(d, &Space::Model, Vec2::new(10.0, 10.0), 0.5);
}

/// Objects in the drawing, in memory or out.
fn total(s: &Session) -> usize {
    let d = s.doc().unwrap();
    d.model.len() + d.sheet_store.parked_objects()
}

/// Undo and redo across a put-out: the steps recorded while the whole set was in memory come
/// back with the sheets out of memory as the drawing has them, and a drawing undone to its
/// saved state has no change to save.
fn undo_after_put_out(p: &str) {
    let mut s = opened(p);
    let all = total(&s);
    s.load_all_sheets().unwrap();
    assert!(!s.state().unwrap().is_dirty(), "loading is no change");
    s.execute("line", &json!({"points": [[0, -40], [5, -40]]})).unwrap();
    s.execute("line", &json!({"points": [[0, -41], [5, -41]]})).unwrap();
    assert!(s.park_idle_sheets().unwrap());
    let parked = |s: &Session| s.doc().unwrap().sheet_store.parked_objects();
    let out = parked(&s);
    assert!(out > 0 && total(&s) == all + 2);
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(total(&s), all + 1, "undo took the last line away");
    assert!(s.has_parked_sheets() && parked(&s) == out, "undone, the other sheets are still out of memory");
    s.execute("undo", &json!({})).unwrap();
    assert_eq!(total(&s), all);
    assert!(s.has_parked_sheets() && parked(&s) == out);
    assert!(!s.state().unwrap().is_dirty(), "undone to the drawing as saved: no change to save");
    s.execute("redo", &json!({"count": 2})).unwrap();
    assert_eq!(total(&s), all + 2, "redone");
    assert!(s.has_parked_sheets() && parked(&s) == out);
    // Every object back where it was: the set loaded again holds them all, once.
    s.load_all_sheets().unwrap();
    assert_eq!(s.doc().unwrap().model.len(), all + 2);
    assert!(!s.has_parked_sheets());
}

#[test]
fn a_whole_set_load_gives_its_memory_back_once_the_sheets_are_out_again() {
    hooks();
    let dir = std::env::temp_dir().join(format!("cadaddy-wholeset-mem-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("set.cdy").to_string_lossy().into_owned();
    {
        let mut s = Session::new();
        s.open_drawing(cadaddy_engine::sample::job_print_set(SHEETS, PER_SHEET), "Sample Set", None);
        s.execute("saveas", &json!({ "path": p })).unwrap();
    }

    let mb = |b: usize| b as f64 / 1_048_576.0;
    let mut loaded = 0usize;
    for worker in [false, true] {
        let mut s = opened(&p);
        hover(&s);
        quiet(&mut s);
        let opened_at = settled(0);
        // One round first: one-time state (command tables, thread-local buffers, caches that
        // live as long as the program).
        round(&mut s, worker, None);
        quiet(&mut s);
        let base = settled(0);
        let mut held = Vec::new();
        for _ in 0..ROUNDS {
            ALLOC.reset_peak();
            round(&mut s, worker, None);
            quiet(&mut s);
            loaded = loaded.max(ALLOC.peak().saturating_sub(base));
            held.push(settled(base).saturating_sub(base));
        }
        let last = held.last().copied().unwrap_or(0);
        eprintln!(
            "worker {worker}: held opened {:.1} MB, after the first round {:.1} MB; a load took {:.1} MB more; held after each round, above that: {:?} MB",
            mb(opened_at),
            mb(base),
            mb(loaded),
            held.iter().map(|b| (mb(*b) * 10.0).round() / 10.0).collect::<Vec<_>>()
        );
        assert!(loaded > 0, "the rounds loaded the set");
        // The rule: about what it held before the load (within a few percent of what a load
        // takes), however many times it is repeated.
        assert!(
            last <= loaded / 20,
            "worker {worker}: after {ROUNDS} loads and put-outs {:.1} MB more is held than before ({:.1} MB a load): {held:?}",
            mb(last),
            mb(loaded)
        );
        // Nor did the first round keep the set (what it set up once is far less than a load).
        assert!(
            base.saturating_sub(opened_at) <= loaded / 10,
            "worker {worker}: the first load and put-out kept {:.1} MB ({:.1} MB a load)",
            mb(base.saturating_sub(opened_at)),
            mb(loaded)
        );
        drop(s);
    }

    // An edit while the set is in memory: its undo step holds what it changed and its own
    // handle index (the sheets it doesn't keep are out of memory as in the drawing), far less
    // than a load, and the undo budget counts what it holds.
    // (The spatial indexes of what is held are let go of before each count: the cache keeps a
    // few, whatever undo holds.)
    let mut s = opened(&p);
    round(&mut s, false, None);
    quiet(&mut s);
    cadaddy_engine::spatial::clear_cache();
    let base = settled(0);
    let said = |s: &Session| s.state().unwrap().undo.iter().map(|x| x.bytes).sum::<usize>();
    let said_before = said(&s);
    for k in 1..=ROUNDS {
        round(&mut s, false, Some(k));
        quiet(&mut s);
    }
    cadaddy_engine::spatial::clear_cache();
    let held = settled(base).saturating_sub(base);
    let counted = said(&s).saturating_sub(said_before);
    eprintln!(
        "{ROUNDS} edits while the set was in memory: {:.1} MB held after, {:.1} MB a step; undo counts {:.1} MB",
        mb(held),
        mb(held / ROUNDS),
        mb(counted)
    );
    assert!(
        held / ROUNDS <= loaded / 10,
        "an undo step of an edit while the set was in memory holds {:.1} MB ({:.1} MB a load)",
        mb(held / ROUNDS),
        mb(loaded)
    );
    assert!(counted >= held / 2, "undo says its steps hold {counted} bytes, they hold {held}: the budget can't see the memory");
    drop(s);
    undo_after_put_out(&p);
    let _ = std::fs::remove_dir_all(&dir);
}
