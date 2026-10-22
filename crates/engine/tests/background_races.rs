//! Stress tests for background-thread races (Q242).
//!
//! Each test hammers a pair of concurrent operations: save while opening, close while
//! a background check is pending, quit during autosave, open the same file twice fast,
//! and agent commands during a save.  Nothing may lose data, deadlock, or show a result
//! for the wrong drawing.

use std::time::{Duration, Instant};

use cadaddy_engine::Session;
use cadaddy_engine::autosave::{self, Autosaver, Job};
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use serde_json::json;

// ─── helpers ───────────────────────────────────────────────────────────────

fn hooks() {
    set_io(IoHooks {
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

fn draw(s: &mut Session) {
    s.execute("circle", &json!({"center": [0, 0], "radius": 1})).unwrap();
}

fn writes(jobs: &[Job]) -> Vec<String> {
    jobs.iter()
        .filter_map(|j| match j {
            Job::Write { path, .. } | Job::Append { path, .. } => Some(path.clone()),
            Job::Delete { .. } => None,
        })
        .collect()
}

fn deletes(jobs: &[Job]) -> Vec<String> {
    jobs.iter()
        .filter_map(|j| match j {
            Job::Delete { path } => Some(path.clone()),
            Job::Write { .. } | Job::Append { .. } => None,
        })
        .collect()
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-races-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn wait_idle(a: &mut Autosaver, limit: Duration) {
    let t = Instant::now();
    while !a.idle() {
        assert!(t.elapsed() < limit, "autosaver did not become idle within {limit:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ─── autosave + close: the busy set must prevent premature file deletion ───

/// Closing a drawing while its autosave write is in progress must not delete the
/// autosave file until the write has finished.  The autosaver's `busy` set protects
/// this: `plan()` keeps the entry while it is busy, and the delete comes on the first
/// `plan()` after `reap()` takes in the finished write.
#[test]
fn close_during_autosave_write_does_not_lose_the_file() {
    hooks();
    let dir = temp_dir("close-during-write");
    let mut s = Session::new();
    draw(&mut s);
    let mut a = Autosaver::new(autosave::instance_id());
    // Start the clock.
    a.plan(&s, 0.0, 1, &dir.to_string_lossy());
    // Time up: plan a write.
    let jobs = a.plan(&s, 60.0, 1, &dir.to_string_lossy());
    assert_eq!(writes(&jobs).len(), 1);
    let path = writes(&jobs)[0].clone();
    a.perform(jobs);

    // Close the drawing while the write is in progress (or just finished).
    s.execute("close", &json!({"save": false})).ok();
    assert!(s.docs.is_empty(), "drawing is closed");

    // The autosaver must not delete the file yet if the write is still running.
    let jobs = a.plan(&s, 61.0, 1, &dir.to_string_lossy());
    // While busy, the entry is retained and no Delete is emitted.
    if !a.idle() {
        assert!(deletes(&jobs).is_empty(), "busy write must not emit a Delete");
    }

    // Wait for the write to finish and reap.
    wait_idle(&mut a, Duration::from_secs(30));

    // Now plan again: the drawing is closed and the write is done, so the file is deleted.
    let jobs = a.plan(&s, 62.0, 1, &dir.to_string_lossy());
    assert_eq!(deletes(&jobs), vec![path.clone()]);
    a.perform(jobs);

    a.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Hammering close + autosave in a tight loop: 20 drawings, each made dirty, an
/// autosave planned, then closed while the write may still be in flight.  No panics,
/// no deadlocks, no leaked threads.
#[test]
fn rapid_close_during_autosave_never_panics() {
    hooks();
    let dir = temp_dir("rapid-close");
    let mut a = Autosaver::new(autosave::instance_id());

    for i in 0..20 {
        let mut s = Session::new();
        draw(&mut s);
        a.plan(&s, (i * 100) as f64, 1, &dir.to_string_lossy());
        let jobs = a.plan(&s, (i * 100 + 60) as f64, 1, &dir.to_string_lossy());
        a.perform(jobs);
        // Close immediately: the write is barely started or not yet.
        s.execute("close", &json!({"save": false})).ok();
        // Reap what finished and plan deletions.
        a.reap();
        let jobs = a.plan(&s, (i * 100 + 61) as f64, 1, &dir.to_string_lossy());
        a.perform(jobs);
    }

    // All writes must finish.
    wait_idle(&mut a, Duration::from_secs(60));
    a.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

// ─── quit during autosave: shutdown must join all writes ───────────────────

/// Calling shutdown() while autosave threads are running must not deadlock and must
/// clean up all autosave files.
#[test]
fn shutdown_during_autosave_joins_all_threads() {
    hooks();
    let dir = temp_dir("shutdown-during");
    let mut s = Session::new();
    for _ in 0..5 {
        s.new_drawing(false);
        draw(&mut s);
    }

    let mut a = Autosaver::new(autosave::instance_id());
    // Start the clock for all drawings.
    a.plan(&s, 0.0, 1, &dir.to_string_lossy());
    // Time up: plan writes for all.
    let jobs = a.plan(&s, 60.0, 1, &dir.to_string_lossy());
    assert!(writes(&jobs).len() >= 4, "multiple drawings should write");
    a.perform(jobs);

    // Shutdown while writes are in progress: must not deadlock.
    let t = Instant::now();
    a.shutdown();
    let took = t.elapsed();
    assert!(took < Duration::from_secs(30), "shutdown took too long: {took:?}");
    assert!(a.idle());

    // All autosave files must be deleted.
    assert!(a.files().is_empty());
    // The lock file must be deleted too.
    let locks: Vec<_> =
        std::fs::read_dir(&dir).into_iter().flatten().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".lock")).collect();
    assert!(locks.is_empty(), "lock file not cleaned up: {locks:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── save_now + close: an urgent autosave racing a close ───────────────────

/// save_now() triggers an immediate write (e.g. after a GPU error); closing the drawing
/// right after must not lose the autosave or deadlock.
#[test]
fn save_now_then_close_does_not_deadlock_or_lose_data() {
    hooks();
    let dir = temp_dir("savenow-close");
    // save_now reads the process-wide SAVEFILEPATH and SAVETIME.
    autosave::set_savefilepath(&dir.to_string_lossy());
    autosave::set_savetime(1);
    let mut s = Session::new();
    draw(&mut s);

    let mut a = Autosaver::new(autosave::instance_id());
    a.plan(&s, 0.0, 1, &dir.to_string_lossy());
    let n = a.save_now(&s);
    assert_eq!(n, 1, "one drawing should be written");

    // Close immediately.
    s.execute("close", &json!({"save": false})).ok();

    // The write must finish without hanging.
    wait_idle(&mut a, Duration::from_secs(30));

    // Clean up.
    a.reap();
    let jobs = a.plan(&s, 100.0, 1, &dir.to_string_lossy());
    a.perform(jobs);
    a.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

// ─── kept_async + close: background DWG check racing drawing close ─────────

/// Save a drawing to DWG with background checks, then close the drawing before the
/// check thread finishes.  The session must not panic or hang when polled.
#[test]
fn close_during_kept_check_does_not_crash() {
    hooks();
    let dir = temp_dir("kept-close");
    let mut s = Session::new();
    s.background_checks = true;

    // Add enough geometry to make the check take a moment.
    let d = s.doc_mut().unwrap();
    for i in 0..500 {
        let y = (i / 50) as f64;
        let x = (i % 50) as f64;
        let h = d.new_handle();
        d.model.push(cadaddy_engine::doc::Entity::new(
            h,
            cadaddy_engine::doc::EntityKind::Line(cadaddy_engine::doc::Line {
                a: cadaddy_engine::geom::Vec3::new(x, y, 0.0),
                b: cadaddy_engine::geom::Vec3::new(x + 0.8, y + 0.5, 0.0),
            }),
        ));
    }

    let path = dir.join("Sample Floor Plan.dwg");
    let r = s.execute("saveas", &json!({ "path": &path, "version": "2000" })).unwrap();
    assert_eq!(r.get("notKeptPending"), Some(&json!(true)), "background check started");

    // Close before the check finishes.
    s.execute("close", &json!({"save": false})).ok();
    assert!(s.docs.is_empty());

    // Polling must not crash even after the drawing is gone.
    for _ in 0..50 {
        s.poll_background();
        std::thread::sleep(Duration::from_millis(10));
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Close and then keep polling: the kept_job result must not appear on a different
/// drawing.
#[test]
fn kept_result_does_not_leak_to_another_drawing() {
    hooks();
    let dir = temp_dir("kept-leak");
    let mut s = Session::new();
    s.background_checks = true;

    // Drawing 1: save to DWG with a lossy property.
    let d = s.doc_mut().unwrap();
    let h = d.new_handle();
    let mut e = cadaddy_engine::doc::Entity::new(
        h,
        cadaddy_engine::doc::EntityKind::Line(cadaddy_engine::doc::Line {
            a: cadaddy_engine::geom::Vec3::ZERO,
            b: cadaddy_engine::geom::Vec3::new(1.0, 0.0, 0.0),
        }),
    );
    e.common.transparency = cadaddy_engine::doc::Transparency::Percent(50);
    d.model.push(e);
    let path = dir.join("First.dwg");
    s.execute("saveas", &json!({ "path": &path, "version": "2000" })).unwrap();

    // Open a second drawing.
    s.new_drawing(false);
    assert!(s.docs.len() >= 2);

    // Close drawing 1.
    s.execute("document.switch", &json!({ "index": 0 })).unwrap();
    s.execute("close", &json!({"save": false})).ok();

    // Poll: the result must not appear on drawing 2.
    for _ in 0..100 {
        s.poll_background();
        std::thread::sleep(Duration::from_millis(10));
    }
    for st in &s.docs {
        assert!(st.exchange_notice.is_none(), "drawing {} (uid {}) got an exchange_notice it shouldn't have", st.title, st.uid);
    }

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── multiple drawings: autosave must not mix up files ─────────────────────

/// Multiple drawings changed at different times: each gets its own autosave file, and
/// closing one leaves the others' files intact.
#[test]
fn autosave_tracks_multiple_drawings_independently() {
    hooks();
    let dir = temp_dir("multi-track");
    let mut s = Session::new();
    s.new_drawing(false);
    s.new_drawing(false);
    // Draw in each.
    for i in 0..s.docs.len() {
        s.execute("document.switch", &json!({ "index": i })).unwrap();
        draw(&mut s);
    }
    let uids: Vec<u64> = s.docs.iter().map(|d| d.uid).collect();
    assert_eq!(uids.len(), 3);

    let mut a = Autosaver::new(autosave::instance_id());
    a.plan(&s, 0.0, 1, &dir.to_string_lossy());
    let jobs = a.plan(&s, 60.0, 1, &dir.to_string_lossy());
    let paths = writes(&jobs);
    assert_eq!(paths.len(), 3, "each drawing gets a write");
    a.perform(jobs);
    wait_idle(&mut a, Duration::from_secs(30));

    // Verify each file exists.
    for p in &paths {
        assert!(std::path::Path::new(p).exists(), "autosave file {p} missing");
    }

    // Close the middle drawing.
    s.execute("document.switch", &json!({ "index": 1 })).unwrap();
    s.execute("close", &json!({"save": false})).ok();
    assert_eq!(s.docs.len(), 2);

    a.reap();
    let jobs = a.plan(&s, 61.0, 1, &dir.to_string_lossy());
    let deleted = deletes(&jobs);
    assert_eq!(deleted.len(), 1, "only the closed drawing's file is deleted");
    a.perform(jobs);

    // The other two files must still exist.
    let remaining = a.files();
    assert_eq!(remaining.len(), 2);
    for p in &remaining {
        assert!(std::path::Path::new(p).exists(), "file should still exist: {p}");
    }

    a.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

// ─── hammer: many concurrent autosave + close + save_now cycles ────────────

/// 50 iterations of: create drawing, dirty it, plan an autosave, save_now, close, reap.
/// No panics, no deadlocks, no thread leaks.
#[test]
fn hammer_autosave_close_cycle() {
    hooks();
    let dir = temp_dir("hammer");
    let mut a = Autosaver::new(autosave::instance_id());

    for i in 0..50 {
        let mut s = Session::new();
        draw(&mut s);
        let t = (i * 200) as f64;
        a.plan(&s, t, 1, &dir.to_string_lossy());
        a.save_now(&s);
        // Sometimes close immediately, sometimes wait a bit.
        if i % 3 != 0 {
            std::thread::sleep(Duration::from_millis(2));
        }
        s.execute("close", &json!({"save": false})).ok();
        a.reap();
        let jobs = a.plan(&s, t + 1.0, 1, &dir.to_string_lossy());
        a.perform(jobs);
    }

    wait_idle(&mut a, Duration::from_secs(60));
    a.shutdown();
    assert!(a.files().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

// ─── agent commands during save: commands must not corrupt state ────────────

/// Running drawing commands while a kept_async background check is in progress must
/// not corrupt the session or produce wrong results.
#[test]
fn commands_during_background_check_do_not_corrupt() {
    hooks();
    let dir = temp_dir("agent-save");
    let mut s = Session::new();
    s.background_checks = true;

    draw(&mut s);
    let path = dir.join("Agent Test.dwg");
    s.execute("saveas", &json!({ "path": &path, "version": "2000" })).unwrap();

    // While the check runs, execute many drawing commands.
    let start = Instant::now();
    let mut commands_run = 0;
    while s.state().unwrap().kept_job.is_some() && start.elapsed() < Duration::from_secs(30) {
        s.execute("line", &json!({ "points": [[0, 0], [commands_run as f64, 1.0]] })).unwrap();
        s.poll_background();
        commands_run += 1;
        if commands_run > 200 {
            break; // enough hammering
        }
    }

    // The drawing must have all the lines we added.
    let entities = s.doc().unwrap().model.len();
    // Original circle + the lines.
    assert!(entities >= commands_run, "expected at least {commands_run} entities, got {entities}");

    // Undo must work: the state is not corrupted.
    for _ in 0..commands_run.min(10) {
        s.undo().unwrap();
    }

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── open same file twice fast ─────────────────────────────────────────────

/// Opening the same DXF file twice (synchronous OPEN) switches to the already-open
/// drawing (AutoCAD behaviour), not creating a duplicate.  Opening two *different*
/// files in rapid succession produces two drawings with distinct uids.
#[test]
fn open_same_file_switches_and_two_different_files_produce_two_drawings() {
    hooks();
    let dir = temp_dir("double-open");
    let s0 = Session::new();
    let bytes = cadaddy_io::write(s0.doc().unwrap(), "x.dxf").unwrap();
    let a_path = dir.join("Plan A.dxf");
    let b_path = dir.join("Plan B.dxf");
    std::fs::write(&a_path, &bytes).unwrap();
    std::fs::write(&b_path, &bytes).unwrap();

    let mut s = Session::new();
    let before = s.docs.len();
    s.execute("open", &json!({ "path": a_path.to_string_lossy() })).unwrap();
    // Opening the same file again switches to it (no new drawing).
    s.execute("open", &json!({ "path": a_path.to_string_lossy() })).unwrap();
    assert_eq!(s.docs.len(), before + 1, "same file: only one drawing");

    // A different file adds a second drawing.
    s.execute("open", &json!({ "path": b_path.to_string_lossy() })).unwrap();
    assert_eq!(s.docs.len(), before + 2, "two different files: two drawings");

    let uids: Vec<u64> = s.docs.iter().map(|d| d.uid).collect();
    let unique: std::collections::HashSet<u64> = uids.iter().copied().collect();
    assert_eq!(uids.len(), unique.len(), "uids must be unique: {uids:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

// ─── autosave + re-open: a drawing closed and reopened must not conflict ───

/// Close a drawing whose autosave was running, then open a new drawing: the autosaver
/// must not confuse the new drawing with the closed one.
#[test]
fn autosave_does_not_confuse_closed_and_reopened_drawings() {
    hooks();
    let dir = temp_dir("reopen");
    let mut s = Session::new();
    draw(&mut s);
    let uid1 = s.state().unwrap().uid;

    let mut a = Autosaver::new(autosave::instance_id());
    a.plan(&s, 0.0, 1, &dir.to_string_lossy());
    let jobs = a.plan(&s, 60.0, 1, &dir.to_string_lossy());
    a.perform(jobs);

    // Close the drawing.
    s.execute("close", &json!({"save": false})).ok();

    // Open a new drawing (gets a new uid).
    s.new_drawing(false);
    draw(&mut s);
    let uid2 = s.state().unwrap().uid;
    assert_ne!(uid1, uid2, "new drawing must have a different uid");

    // Reap the old write and plan for the new drawing.
    wait_idle(&mut a, Duration::from_secs(30));
    a.reap();
    let jobs = a.plan(&s, 120.0, 1, &dir.to_string_lossy());
    // The old drawing's file should be deleted; the new one should get its own write.
    let del = deletes(&jobs);
    let wr = writes(&jobs);
    assert!(!del.is_empty() || !wr.is_empty(), "old file deleted or new write planned");

    a.perform(jobs);
    wait_idle(&mut a, Duration::from_secs(30));

    // The new drawing's autosave file must exist and not be the same as the old one.
    let files = a.files();
    for f in &files {
        assert!(!f.contains(&format!("_{uid1}.")), "file name must not contain closed drawing's uid: {f}");
    }

    a.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

// ─── autosave write + a second change: the busy flag skips the write ───────

/// While an autosave write is in flight, a second change to the same drawing must not
/// produce a second concurrent write; the busy set skips it.  The change is picked up
/// on the next cycle.
#[test]
fn second_change_during_write_does_not_duplicate_writes() {
    hooks();
    let dir = temp_dir("dup-write");
    let mut s = Session::new();
    draw(&mut s);

    let mut a = Autosaver::new(autosave::instance_id());
    a.plan(&s, 0.0, 1, &dir.to_string_lossy());
    let jobs = a.plan(&s, 60.0, 1, &dir.to_string_lossy());
    a.perform(jobs);

    // Make a second change while the first write is running.
    draw(&mut s);
    let jobs = a.plan(&s, 61.0, 1, &dir.to_string_lossy());
    // The drawing is busy: no write should be emitted.
    assert!(writes(&jobs).is_empty(), "a busy drawing must not get a second write");

    // After the write finishes, the new change is picked up.
    wait_idle(&mut a, Duration::from_secs(30));
    a.reap();
    a.plan(&s, 62.0, 1, &dir.to_string_lossy()); // restart clock
    let jobs = a.plan(&s, 122.0, 1, &dir.to_string_lossy());
    assert_eq!(writes(&jobs).len(), 1, "new change is written on next cycle");

    a.perform(jobs);
    wait_idle(&mut a, Duration::from_secs(30));
    a.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
