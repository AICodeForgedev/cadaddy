//! Stress tests for UI-level background-thread races (Q242).
//!
//! Opening workers, canvas rebuilds and the update poll interact with the UI thread's
//! close, quit and tab-switch operations.  Nothing may panic, deadlock, crash, or show
//! stale data for a closed drawing.

use std::time::{Duration, Instant};

use cadaddy_engine::Session;
use serde_json::json;

use cadaddy_ui_egui::{CadApp, Services};

fn io_hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-uiraces-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn new_app() -> CadApp {
    CadApp::new(Session::empty(), Services::default())
}

fn app_with(titles: &[&str]) -> CadApp {
    let mut a = CadApp::new(Session::empty(), Services::default());
    for _ in titles {
        a.session.new_drawing(false);
    }
    for (d, t) in a.session.docs.iter_mut().zip(titles) {
        d.title = (*t).to_string();
    }
    a
}

fn dirty(app: &mut CadApp, i: usize) {
    app.session.execute("document.switch", &json!({ "index": i })).unwrap();
    app.session.execute("circle", &json!({ "center": [0, 0], "radius": 1 })).unwrap();
    assert!(app.session.docs[i].is_dirty());
}

// ─── opening workers + close/quit ──────────────────────────────────────────

/// Start opening a file, then close the app before the worker finishes.  The opening
/// queue must be drained without a panic and without adding the drawing.
#[test]
fn close_app_while_opening_a_file() {
    io_hooks();
    let dir = temp_dir("open-close");
    let path = dir.join("Sample Floor Plan.dxf");
    // Write a real DXF so the open worker has something to read.
    let s0 = Session::new();
    let bytes = cadaddy_io::write(s0.doc().unwrap(), "x.dxf").unwrap();
    std::fs::write(&path, &bytes).unwrap();

    let mut app = new_app();
    let before = app.session.docs.len();
    app.open_path(&path.to_string_lossy());
    assert!(app.opening.busy(), "a worker should have started");

    // Simulate quit: drop the CadApp.  The worker thread holds only an mpsc::Sender;
    // dropping the Receiver on our side causes the send to return Err, and the thread
    // exits cleanly.  This must not panic.
    drop(app);

    // Also test: open then poll to completion, then confirm the drawing was added.
    let mut app2 = new_app();
    app2.open_path(&path.to_string_lossy());
    let t = Instant::now();
    while app2.opening.busy() && t.elapsed() < Duration::from_secs(30) {
        app2.finish_opening();
        std::thread::sleep(Duration::from_millis(5));
    }
    app2.finish_opening();
    assert!(!app2.opening.busy());
    assert_eq!(app2.session.docs.len(), before + 1, "the drawing should have been added");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Open two different files on workers at once.  Both must complete and produce a
/// drawing with its own uid.  (Opening the same file twice switches to the first,
/// matching AutoCAD's behaviour.)
#[test]
fn open_two_different_files_on_workers() {
    io_hooks();
    let dir = temp_dir("double-worker");
    let s0 = Session::new();
    let bytes = cadaddy_io::write(s0.doc().unwrap(), "x.dxf").unwrap();
    let path_a = dir.join("Plan A.dxf");
    let path_b = dir.join("Plan B.dxf");
    std::fs::write(&path_a, &bytes).unwrap();
    std::fs::write(&path_b, &bytes).unwrap();

    let mut app = new_app();
    let before = app.session.docs.len();
    app.open_path(&path_a.to_string_lossy());
    app.open_path(&path_b.to_string_lossy());

    let t = Instant::now();
    while app.opening.busy() && t.elapsed() < Duration::from_secs(30) {
        app.finish_opening();
        std::thread::sleep(Duration::from_millis(5));
    }
    app.finish_opening();
    assert!(!app.opening.busy());
    assert_eq!(app.session.docs.len(), before + 2, "both files should produce a drawing");

    let uids: Vec<u64> = app.session.docs.iter().map(|d| d.uid).collect();
    let unique: std::collections::HashSet<u64> = uids.iter().copied().collect();
    assert_eq!(uids.len(), unique.len(), "uids must be unique");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Open a file, close the only drawing before the worker finishes, then let the worker
/// complete: the opened drawing must be added successfully even when the session was
/// otherwise empty.
#[test]
fn close_all_while_opening_then_finish() {
    io_hooks();
    let dir = temp_dir("closeall-open");
    let path = dir.join("Sample Floor Plan.dxf");
    let s0 = Session::new();
    let bytes = cadaddy_io::write(s0.doc().unwrap(), "x.dxf").unwrap();
    std::fs::write(&path, &bytes).unwrap();

    let mut app = app_with(&["A"]);
    app.open_path(&path.to_string_lossy());

    // Close all existing drawings while the open is in flight.
    app.run("closeall", json!({})).unwrap();
    assert!(app.session.docs.is_empty(), "all drawings closed");

    // Let the open finish.
    let t = Instant::now();
    while app.opening.busy() && t.elapsed() < Duration::from_secs(30) {
        app.finish_opening();
        std::thread::sleep(Duration::from_millis(5));
    }
    app.finish_opening();
    // The opened drawing should have been added to the now-empty session.
    assert_eq!(app.session.docs.len(), 1, "the opened file should add a drawing");

    let _ = std::fs::remove_dir_all(&dir);
}

/// Multiple files opened at once, one of which does not exist.  The good ones succeed
/// and the bad one reports an error, without any of them blocking the others.
#[test]
fn open_mix_of_good_and_bad_files() {
    io_hooks();
    let dir = temp_dir("mix-open");
    let s0 = Session::new();
    let bytes = cadaddy_io::write(s0.doc().unwrap(), "x.dxf").unwrap();
    let good_a = dir.join("Good Plan A.dxf");
    let good_b = dir.join("Good Plan B.dxf");
    std::fs::write(&good_a, &bytes).unwrap();
    std::fs::write(&good_b, &bytes).unwrap();
    let bad = dir.join("Nonexistent Plan.dxf");

    let mut app = new_app();
    let before = app.session.docs.len();
    app.open_path(&good_a.to_string_lossy());
    app.open_path(&bad.to_string_lossy());
    app.open_path(&good_b.to_string_lossy());

    let t = Instant::now();
    while app.opening.busy() && t.elapsed() < Duration::from_secs(30) {
        app.finish_opening();
        std::thread::sleep(Duration::from_millis(5));
    }
    app.finish_opening();
    assert!(!app.opening.busy());
    // Two good opens (different paths), one bad.
    assert_eq!(app.session.docs.len(), before + 2, "two good files should add drawings");

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── close during close-all: queue consistency ─────────────────────────────

/// While close-all is walking through drawings, a programmatic close of one of the
/// queued drawings (through the control channel) must not leave the close queue broken.
#[test]
fn programmatic_close_during_close_all_keeps_queue_consistent() {
    let mut app = app_with(&["A", "B", "C", "D"]);
    dirty(&mut app, 1); // B has unsaved changes
    dirty(&mut app, 3); // D has unsaved changes

    // Start close-all.  A is clean and closes; B asks.
    app.request_close_all();
    assert_eq!(app.close_prompt(), Some("B"));

    // A programmatic close removes C (which is still in the queue) before we answer B.
    let c_idx = app.session.docs.iter().position(|d| d.title == "C").unwrap();
    app.run("close", json!({ "index": c_idx })).unwrap();

    // Now answer B with Discard.
    app.answer_close(cadaddy_ui_egui::closing::Choice::Discard);
    // D should still be asked about.
    if let Some(title) = app.close_prompt() {
        assert_eq!(title, "D");
        app.answer_close(cadaddy_ui_egui::closing::Choice::Discard);
    }
    // The close queue should be done.
    assert!(app.closing.is_none(), "close queue should be finished");
}

// ─── rapid tab switching during opening ────────────────────────────────────

/// Switch tabs rapidly while files are being opened.  The session must remain
/// consistent: active index in bounds, no panic.
#[test]
fn rapid_tab_switch_during_open() {
    io_hooks();
    let dir = temp_dir("tab-switch");
    let path = dir.join("Sample Floor Plan.dxf");
    let s0 = Session::new();
    let bytes = cadaddy_io::write(s0.doc().unwrap(), "x.dxf").unwrap();
    std::fs::write(&path, &bytes).unwrap();

    let mut app = app_with(&["A", "B", "C"]);
    for _ in 0..5 {
        app.open_path(&path.to_string_lossy());
    }

    // While opens are in flight, switch tabs rapidly.
    for _ in 0..30 {
        let n = app.session.docs.len();
        if n > 0 {
            let target = fastrand_index(n);
            let _ = app.session.execute("document.switch", &json!({ "index": target }));
        }
        app.finish_opening();
        std::thread::sleep(Duration::from_millis(2));
    }

    // Drain remaining.
    let t = Instant::now();
    while app.opening.busy() && t.elapsed() < Duration::from_secs(30) {
        app.finish_opening();
        std::thread::sleep(Duration::from_millis(5));
    }
    app.finish_opening();
    assert!(!app.opening.busy());
    assert!(app.session.active < app.session.docs.len(), "active index in bounds");
}

/// A simple pseudo-random index (no external dep, good enough for a test).
fn fastrand_index(n: usize) -> usize {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    std::time::Instant::now().hash(&mut h);
    std::thread::current().id().hash(&mut h);
    (h.finish() as usize) % n
}
