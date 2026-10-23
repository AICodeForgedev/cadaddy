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

/// The entity count of the drawing in autosave file `path`, read back through the file
/// formats (the file holds a .cdy, DWG or DXF; the format comes from the content).
fn autosaved_count(path: &str) -> usize {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("autosave file {path} unreadable: {e}"));
    cadaddy_io::read(&bytes, path).unwrap_or_else(|e| panic!("autosave file {path} is not a drawing: {e}")).entity_count()
}

/// `save_now` reads the process-wide SAVEFILEPATH, so the tests that use it take turns.
static SAVE_NOW: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    let expected = s.doc().unwrap().entity_count();
    assert_eq!(expected, 1, "one circle");
    a.perform(jobs);

    // Close the drawing while the write is in progress (or just finished).
    s.execute("close", &json!({})).ok();
    assert!(s.docs.is_empty(), "drawing is closed");

    // The autosaver must not delete the file yet if the write is still running.
    let jobs = a.plan(&s, 61.0, 1, &dir.to_string_lossy());
    // While busy, the entry is retained and no Delete is emitted.
    if !a.idle() {
        assert!(deletes(&jobs).is_empty(), "busy write must not emit a Delete");
    }

    // Wait for the write to finish and reap.
    wait_idle(&mut a, Duration::from_secs(30));

    // The finished file holds the closed drawing, whole.
    assert_eq!(autosaved_count(&path), expected, "autosave file {path} holds the drawing");

    // Now plan again: the drawing is closed and the write is done, so the file is deleted.
    let jobs = a.plan(&s, 62.0, 1, &dir.to_string_lossy());
    assert_eq!(deletes(&jobs), vec![path.clone()]);
    a.perform(jobs);
    assert!(!std::path::Path::new(&path).exists(), "deleted after the write finished");

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
        s.execute("close", &json!({})).ok();
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
    let _turn = SAVE_NOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = temp_dir("savenow-close");
    // save_now reads the process-wide SAVEFILEPATH and SAVETIME.
    autosave::set_savefilepath(&dir.to_string_lossy());
    autosave::set_savetime(1);
    let mut s = Session::new();
    draw(&mut s);
    draw(&mut s);
    let expected = s.doc().unwrap().entity_count();
    assert_eq!(expected, 2, "two circles");

    let mut a = Autosaver::new(autosave::instance_id());
    a.plan(&s, 0.0, 1, &dir.to_string_lossy());
    let n = a.save_now(&s);
    assert_eq!(n, 1, "one drawing should be written");

    // Close immediately.
    s.execute("close", &json!({})).ok();

    // The write must finish without hanging.
    wait_idle(&mut a, Duration::from_secs(30));

    // The urgent autosave holds the closed drawing, whole, until its delete.
    let files = a.files();
    assert_eq!(files.len(), 1, "one autosave file: {files:?}");
    assert_eq!(autosaved_count(&files[0]), expected, "autosave file {} holds the drawing", files[0]);

    // The drawing is closed: the next plan deletes exactly that file.
    a.reap();
    let jobs = a.plan(&s, 100.0, 1, &dir.to_string_lossy());
    assert_eq!(deletes(&jobs), files);
    a.perform(jobs);
    assert!(!std::path::Path::new(&files[0]).exists());
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
    s.execute("close", &json!({})).ok();
    assert!(s.docs.is_empty());

    // Polling must not crash even after the drawing is gone.
    for _ in 0..50 {
        s.poll_background();
        std::thread::sleep(Duration::from_millis(10));
    }

    let _ = std::fs::remove_dir_all(&dir);
}

/// Drawing 1 has a property DWG 2000 can't keep (50% transparency) and is saved to DWG with
/// background checks; a second drawing is opened after it.
fn lossy_save_then_second_drawing(dir: &std::path::Path, name: &str) -> Session {
    let mut s = Session::new();
    s.background_checks = true;
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
    let r = s.execute("saveas", &json!({ "path": dir.join(name), "version": "2000" })).unwrap();
    assert_eq!(r.get("notKeptPending"), Some(&json!(true)), "background check started");
    s.new_drawing(false);
    assert_eq!(s.docs.len(), 2);
    s
}

/// Close and then keep polling: the kept_job result must not appear on a different
/// drawing.  Positive control first: the same save without the close puts the notice on
/// drawing 1 (and only there), so the negative half can't pass just because no notice
/// would ever come.
#[test]
fn kept_result_does_not_leak_to_another_drawing() {
    hooks();
    let dir = temp_dir("kept-leak");

    // Positive control: drawing 1 stays open and gets its notice.
    let mut s = lossy_save_then_second_drawing(&dir, "Control.dwg");
    let uid1 = s.docs[0].uid;
    let t = Instant::now();
    while s.docs[0].exchange_notice.is_none() {
        assert!(t.elapsed() < Duration::from_secs(30), "drawing 1 never got its notice");
        s.poll_background();
        std::thread::sleep(Duration::from_millis(10));
    }
    let took = t.elapsed();
    assert_eq!(s.docs[0].uid, uid1);
    assert!(s.docs[1].exchange_notice.is_none(), "the notice went to drawing 1 only");

    // The race: drawing 1 is closed while its check runs.
    let mut s = lossy_save_then_second_drawing(&dir, "First.dwg");
    s.execute("document.switch", &json!({ "index": 0 })).unwrap();
    s.execute("close", &json!({})).ok();
    assert_eq!(s.docs.len(), 1);

    // Poll well past the time the control took: the result must not appear on drawing 2.
    let wait = (took * 3).max(Duration::from_secs(1));
    let t = Instant::now();
    while t.elapsed() < wait {
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
    s.execute("close", &json!({})).ok();
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
/// No panics, no deadlocks, no thread leaks; every autosave file is read back before its
/// delete and holds its own drawing's exact entity count (drawing i has i % 4 + 1 circles).
#[test]
fn hammer_autosave_close_cycle() {
    hooks();
    let _turn = SAVE_NOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = temp_dir("hammer");
    autosave::set_savefilepath(&dir.to_string_lossy());
    autosave::set_savetime(1);
    let mut a = Autosaver::new(autosave::instance_id());
    // Each drawing's uid → its entity count, and the files read back.
    let mut expected = std::collections::HashMap::new();
    let mut checked = std::collections::HashSet::new();
    let mut check = |jobs: &[Job], expected: &std::collections::HashMap<u64, usize>| {
        for p in deletes(jobs) {
            let name = std::path::Path::new(&p).file_name().unwrap().to_string_lossy().to_string();
            let (_, _, uid) = autosave::parse_file_name(&name).unwrap_or_else(|| panic!("not an autosave name: {name}"));
            assert_eq!(autosaved_count(&p), expected[&uid], "autosave file {p} holds drawing {uid}");
            assert!(checked.insert(p.clone()), "{p} deleted twice");
        }
    };

    for i in 0..50 {
        let mut s = Session::new();
        for _ in 0..i % 4 + 1 {
            draw(&mut s);
        }
        expected.insert(s.state().unwrap().uid, s.doc().unwrap().entity_count());
        let t = (i * 200) as f64;
        // The last drawing's write (closed mid-write) finishes; its delete comes now, so it is
        // checked here rather than inside save_now.
        wait_idle(&mut a, Duration::from_secs(30));
        let jobs = a.plan(&s, t, 1, &dir.to_string_lossy());
        check(&jobs, &expected);
        a.perform(jobs);
        assert_eq!(a.save_now(&s), 1, "iteration {i}: one drawing written");
        // Sometimes close immediately, sometimes wait a bit.
        if i % 3 != 0 {
            std::thread::sleep(Duration::from_millis(2));
        }
        s.execute("close", &json!({})).ok();
        a.reap();
        let jobs = a.plan(&s, t + 1.0, 1, &dir.to_string_lossy());
        check(&jobs, &expected);
        a.perform(jobs);
    }

    // The writes still running finish; their files are read back before their deletes too.
    wait_idle(&mut a, Duration::from_secs(60));
    let jobs = a.plan(&Session::empty(), 50.0 * 200.0, 1, &dir.to_string_lossy());
    check(&jobs, &expected);
    a.perform(jobs);
    assert_eq!(checked.len(), 50, "every drawing's autosave file was read back");
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
    let base = s.doc().unwrap().entity_count();
    assert_eq!(base, 1, "one circle");
    let path = dir.join("Agent Test.dwg");
    let r = s.execute("saveas", &json!({ "path": &path, "version": "2000" })).unwrap();
    assert_eq!(r.get("notKeptPending"), Some(&json!(true)), "background check started");

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

    // The check was pending when the loop began, so at least one line ran during it.
    assert!(commands_run >= 1);
    // The drawing has the circle and exactly the lines we added.
    let entities = s.doc().unwrap().entity_count();
    assert_eq!(entities, commands_run + 1, "circle + {commands_run} lines");

    // Undo works and takes off exactly one line each: the state is not corrupted.
    let undos = commands_run.min(10);
    for _ in 0..undos {
        s.undo().unwrap();
    }
    assert_eq!(s.doc().unwrap().entity_count(), commands_run + 1 - undos, "after {undos} undos");

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
    s.execute("close", &json!({})).ok();

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
