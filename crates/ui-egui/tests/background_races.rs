//! Stress tests for UI-level background-thread races (Q242).
//!
//! Opening workers, DWG saves with background checks and agent requests over the control
//! channel interact with the UI thread's save, close, quit and tab-switch operations.
//! Nothing may panic, deadlock, lose data, or act on or show data for the wrong drawing.

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
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

/// A DXF holding `circles` circles.
fn dxf_with(circles: usize) -> Vec<u8> {
    let mut s = Session::new();
    for i in 0..circles {
        s.execute("circle", &json!({ "center": [i as f64 * 3.0, 0], "radius": 1 })).unwrap();
    }
    cadaddy_io::write(s.doc().unwrap(), "x.dxf").unwrap()
}

/// The entity count of the drawing in `path`, read back through the file formats.
fn file_count(path: &std::path::Path) -> usize {
    let bytes = std::fs::read(path).unwrap();
    cadaddy_io::read(&bytes, &path.to_string_lossy()).unwrap().entity_count()
}

/// The entity count of the open drawing titled `title` (exactly one must have it).
fn count_titled(app: &CadApp, title: &str) -> usize {
    let found: Vec<_> = app.session.docs.iter().filter(|d| d.title == title).collect();
    assert_eq!(found.len(), 1, "exactly one drawing titled {title}: {:?}", titles(app));
    found[0].doc.entity_count()
}

fn titles(app: &CadApp) -> Vec<String> {
    app.session.docs.iter().map(|d| d.title.clone()).collect()
}

/// Let the open workers finish and add their drawings (bounded).
fn drain_opening(app: &mut CadApp) {
    let t = Instant::now();
    while app.opening.busy() {
        assert!(t.elapsed() < Duration::from_secs(30), "open workers did not finish");
        app.finish_opening();
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Let every background DWG check finish and be taken in (bounded).
fn drain_checks(app: &mut CadApp) {
    let t = Instant::now();
    while app.session.docs.iter().any(|d| d.kept_job.is_some()) {
        assert!(t.elapsed() < Duration::from_secs(30), "background checks did not finish");
        app.session.poll_background();
        std::thread::sleep(Duration::from_millis(5));
    }
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

    // C is gone; A closed already; B is still asked about, D still waits.
    assert_eq!(titles(&app), ["B", "D"]);
    assert_eq!(app.close_prompt(), Some("B"));

    // Now answer B with Discard: D must be asked about next (the queue skipped C, not D).
    app.answer_close(cadaddy_ui_egui::closing::Choice::Discard);
    assert_eq!(titles(&app), ["D"]);
    assert_eq!(app.close_prompt(), Some("D"), "D still has to be asked about");
    app.answer_close(cadaddy_ui_egui::closing::Choice::Discard);
    // The close queue is done and every drawing is closed.
    assert!(app.closing.is_none(), "close queue should be finished");
    assert_eq!(app.close_prompt(), None);
    assert!(titles(&app).is_empty(), "every drawing closed: {:?}", titles(&app));
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

    // Changed drawings (an opened file replaces untouched new drawings, as in AutoCAD).
    let mut app = app_with(&["A", "B", "C"]);
    for i in 0..3 {
        dirty(&mut app, i);
    }
    for _ in 0..5 {
        app.open_path(&path.to_string_lossy());
    }

    // While opens are in flight, switch tabs rapidly (a fixed seed: the same run every time).
    let mut rng = 0x2545_f491_4f6c_dd1d_u64;
    for _ in 0..30 {
        let n = app.session.docs.len();
        if n > 0 {
            let target = next_index(&mut rng, n);
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
    // Five opens of one file add it once: the three drawings plus one.
    assert_eq!(app.session.docs.len(), 3 + 1, "{:?}", titles(&app));
    assert_eq!(titles(&app), ["A", "B", "C", "Sample Floor Plan.dxf"]);
    let opened: Vec<_> = app.session.docs.iter().filter(|d| d.path.as_deref() == Some(&*path.to_string_lossy())).collect();
    assert_eq!(opened.len(), 1, "the file is open once");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A seeded pseudo-random index (xorshift64; no external dep).
fn next_index(state: &mut u64, n: usize) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state % n as u64) as usize
}

// ─── save while opening ────────────────────────────────────────────────────

/// Start opening a file on a worker, then save the active drawing (DXF, then DWG with a
/// background check) before the open is taken in.  Each saved file reads back with the
/// saved drawing's exact entity count, and the opened drawing arrives with its own source
/// content and title, not the saved one's.
#[test]
fn save_while_opening_keeps_both_drawings_whole() {
    io_hooks();
    let dir = temp_dir("save-open");
    let src = dir.join("Opened Plan.dxf");
    std::fs::write(&src, dxf_with(5)).unwrap();
    assert_eq!(file_count(&src), 5);

    let mut app = app_with(&["Saved Plan"]);
    app.session.background_checks = true;
    for i in 0..3 {
        app.session.execute("circle", &json!({ "center": [i as f64 * 3.0, 9], "radius": 2 })).unwrap();
    }
    assert_eq!(count_titled(&app, "Saved Plan"), 3);

    app.open_path(&src.to_string_lossy());
    assert!(app.opening.busy(), "a worker should have started");

    // DWG save with a background check (the drawing's first save, so its check runs) while
    // the open is in flight.
    let dwg = dir.join("Saved Plan.dwg");
    let r = app.session.execute("saveas", &json!({ "path": &dwg, "version": "2000" })).unwrap();
    assert_eq!(r.get("notKeptPending"), Some(&json!(true)), "background check started: {r}");
    assert!(app.opening.busy(), "the open is not taken in yet");
    assert_eq!(file_count(&dwg), 3, "the saved DWG holds the saved drawing");

    // DXF save, still before the open is taken in.
    let dxf = dir.join("Saved Plan.dxf");
    app.run("saveas", json!({ "path": &dxf })).unwrap();
    assert!(app.opening.busy(), "the open is not taken in yet");
    assert_eq!(file_count(&dxf), 3, "the saved DXF holds the saved drawing");

    // Saved as DWG again, so the drawing's file is the DWG.
    app.run("saveas", json!({ "path": &dwg, "version": "2000" })).unwrap();
    assert!(app.opening.busy(), "the open is not taken in yet");

    drain_opening(&mut app);
    drain_checks(&mut app);

    assert_eq!(titles(&app), ["Saved Plan.dwg", "Opened Plan.dxf"]);
    let saved = &app.session.docs[0];
    assert_eq!(saved.path.as_deref(), Some(&*dwg.to_string_lossy()));
    assert_eq!(saved.doc.entity_count(), 3);
    let opened = &app.session.docs[1];
    assert_eq!(opened.path.as_deref(), Some(&*src.to_string_lossy()));
    assert_eq!(opened.doc.entity_count(), 5, "the opened drawing has its source's content");
    assert!(opened.exchange_notice.is_none(), "the saved drawing's check result is not on the opened one");
    // The files on disk are unchanged by the open.
    assert_eq!(file_count(&dwg), 3);
    assert_eq!(file_count(&src), 5);

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── agent channel during save and open ────────────────────────────────────

/// An agent sends control requests from its own thread (as the control server does) while
/// a DWG save's background check and an open worker are in flight.  Every reply arrives
/// within its time limit, in order, each request acts on the drawing it names, and the
/// final entity counts are exact.
#[test]
fn agent_requests_during_dwg_save_and_open_reach_the_named_drawing() {
    use cadaddy_ui_egui::ControlRequest;
    const LINES: usize = 20_000;
    const N: usize = 40;
    io_hooks();
    let dir = temp_dir("agent-channel");
    let src = dir.join("Opened Plan.dxf");
    std::fs::write(&src, dxf_with(7)).unwrap();

    let mut app = app_with(&["Agent A", "Agent B"]);
    app.session.background_checks = true;
    // Enough geometry in A for its check to take a while; B has one change (an opened file
    // replaces untouched new drawings).
    app.session.execute("document.switch", &json!({ "index": 0 })).unwrap();
    let d = app.session.doc_mut().unwrap();
    for i in 0..LINES {
        let (x, y) = ((i % 100) as f64, (i / 100) as f64);
        let h = d.new_handle();
        d.model.push(cadaddy_engine::doc::Entity::new(
            h,
            cadaddy_engine::doc::EntityKind::Line(cadaddy_engine::doc::Line {
                a: cadaddy_engine::geom::Vec3::new(x, y, 0.0),
                b: cadaddy_engine::geom::Vec3::new(x + 0.8, y + 0.5, 0.0),
            }),
        ));
    }
    dirty(&mut app, 1);
    app.session.execute("document.switch", &json!({ "index": 0 })).unwrap();
    let r = app.session.execute("saveas", &json!({ "path": dir.join("Agent A.dwg"), "version": "2000" })).unwrap();
    assert_eq!(r.get("notKeptPending"), Some(&json!(true)), "background check started: {r}");
    let a = "Agent A.dwg".to_string();
    let b = "Agent B".to_string();
    assert_eq!(titles(&app), [a.as_str(), b.as_str()]);
    let (a0, b0) = (count_titled(&app, &a), count_titled(&app, &b));
    assert_eq!((a0, b0), (LINES, 1));

    app.open_path(&src.to_string_lossy());
    assert!(app.opening.busy(), "a worker should have started");

    // The agent: 40 requests, alternating between the two drawings by title.
    let (tx, rx) = std::sync::mpsc::channel::<ControlRequest>();
    let targets = [a.clone(), b.clone()];
    let agent = std::thread::spawn(move || {
        let mut replies = Vec::new();
        for i in 0..N {
            let title = targets[i % 2].clone();
            let (req, pending) = ControlRequest::new(
                "engine.executeMany",
                json!({ "stopOnError": true, "commands": [
                    { "command": "document.switch", "params": { "title": title } },
                    { "command": "circle", "params": { "center": [i as f64, 50], "radius": 1 } },
                ] }),
            );
            if tx.send(req).is_err() {
                break;
            }
            let t = Instant::now();
            let reply = pending.wait(Duration::from_secs(20));
            replies.push((i, title, reply, t.elapsed()));
        }
        replies
    });

    // The UI thread: serve requests between "frames".  For the first half nothing else is
    // taken in, so the check and the open are certainly still pending; then both are polled
    // as the app does every frame.
    let ctx = egui::Context::default();
    let start = Instant::now();
    let mut served = 0;
    // How many requests ran while A's check was still pending, and whether the open was
    // still pending half way.
    let mut during_check = 0;
    let mut mid = None;
    while !agent.is_finished() {
        assert!(start.elapsed() < Duration::from_secs(60), "agent requests stalled ({served} served)");
        while let Ok(req) = rx.try_recv() {
            if app.session.docs.iter().any(|d| d.kept_job.is_some()) {
                during_check += 1;
            }
            app.serve_control_request(&ctx, req);
            served += 1;
            if served == N / 2 {
                mid = Some(app.opening.busy());
            }
        }
        if served >= N / 2 {
            app.finish_opening();
            app.session.poll_background();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let replies = agent.join().expect("agent thread");
    assert_eq!(mid, Some(true), "half the requests ran with the open pending");
    assert!(during_check >= 1, "requests ran while the DWG check was pending");
    assert_eq!(served, N);

    // Every reply arrived, in order, in time, and succeeded.
    assert_eq!(replies.len(), N);
    for (k, (i, title, reply, took)) in replies.iter().enumerate() {
        assert_eq!(*i, k, "replies in order");
        assert!(*took < Duration::from_secs(20), "request {i} took {took:?}");
        assert_eq!(reply.get("ok"), Some(&json!(true)), "request {i} ({title}): {reply}");
    }

    drain_opening(&mut app);
    drain_checks(&mut app);

    // Each circle landed in the drawing its request named; the opened drawing is untouched.
    assert_eq!(titles(&app), [a.as_str(), b.as_str(), "Opened Plan.dxf"]);
    assert_eq!(count_titled(&app, &a), a0 + N / 2, "drawing {a}");
    assert_eq!(count_titled(&app, &b), b0 + N / 2, "drawing {b}");
    assert_eq!(count_titled(&app, "Opened Plan.dxf"), 7, "the opened drawing has only its own content");

    let _ = std::fs::remove_dir_all(&dir);
}
