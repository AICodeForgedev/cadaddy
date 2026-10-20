//! The first DWG save off the critical path (Q133): with background checks on (the app), the
//! first save of a drawing into DWG returns once the file is written, about as fast as later
//! saves, and what DWG didn't keep arrives afterwards as the drawing's exchange notice and a
//! command-line line. With them off (scripts, the CLI), the save's result holds it, as before.

use std::time::{Duration, Instant};

use cadaddy_engine::Session;
use serde_json::{Value, json};

fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    cadaddy_engine::cmd::file::set_write_as(|d, n, o| {
        cadaddy_io::write_with(d, n, &cadaddy_io::SaveOptions { version: o.version, binary: o.binary }).map_err(|e| e.to_string())
    });
}

/// A made-up plan of `n` lines, one of them see-through (DWG 2000 has no transparency).
fn drawing(n: usize) -> Session {
    let mut s = Session::new();
    let d = s.doc_mut().unwrap();
    for i in 0..n {
        let y = (i / 100) as f64;
        let x = (i % 100) as f64;
        let mut e = cadaddy_engine::doc::Entity::new(
            d.new_handle(),
            cadaddy_engine::doc::EntityKind::Line(cadaddy_engine::doc::Line {
                a: cadaddy_engine::geom::Vec3::new(x, y, 0.0),
                b: cadaddy_engine::geom::Vec3::new(x + 0.8, y + 0.5, 0.0),
            }),
        );
        if i == 0 {
            e.common.transparency = cadaddy_engine::doc::Transparency::Percent(50);
        }
        d.model.push(e);
    }
    s
}

fn save(s: &mut Session, path: &std::path::Path) -> (Value, Duration) {
    let t = Instant::now();
    let r = s.execute("saveas", &json!({ "path": path, "version": "2000" })).unwrap();
    (r, t.elapsed())
}

fn folder(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-keptasync-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_first_dwg_save_returns_at_once_and_the_notice_follows() {
    hooks();
    let dir = folder("bg");
    let mut s = drawing(20_000);
    s.background_checks = true;
    let path = dir.join("Sample Floor Plan.dwg");
    let (r, first) = save(&mut s, &path);
    assert_eq!(r["notKeptPending"], true, "{r}");
    assert!(r.get("notKeptDetail").is_none(), "not in the result: it comes later");
    assert!(path.is_file(), "the file is written");

    // The notice arrives when the check finishes.
    let start = Instant::now();
    let notice = loop {
        s.poll_background();
        if let Some(n) = s.state_mut().unwrap().exchange_notice.take() {
            break n;
        }
        assert!(start.elapsed() < Duration::from_secs(120), "the check never finished");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(notice["format"], "DWG");
    assert!(notice["losses"].to_string().contains("object transparency"), "{notice}");
    assert!(s.log.iter().any(|l| l.contains("Not kept in the DWG file") && l.contains("transparency")), "{:?}", s.log);
    assert!(s.state().unwrap().kept_job.is_none(), "taken");

    // Later saves don't check again; the first was about as fast.
    let (r, later) = save(&mut s, &path);
    assert!(r.get("notKeptPending").is_none() && r.get("notKeptDetail").is_none(), "checked once per drawing: {r}");
    let (_, later2) = save(&mut s, &path);
    let later = later.min(later2);
    eprintln!("first DWG save {first:?}, later saves {later:?}");
    assert!(first <= later * 2 + Duration::from_millis(300), "first save {first:?} vs later {later:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_background_checks_the_result_holds_the_losses() {
    hooks();
    let dir = folder("sync");
    let mut s = drawing(20_000);
    let (r, took) = save(&mut s, &dir.join("Project A.dwg"));
    eprintln!("first DWG save with the check in it: {took:?}");
    assert!(r.get("notKeptPending").is_none());
    assert!(r["notKeptDetail"].to_string().contains("object transparency"), "{r}");
    assert!(s.state().unwrap().exchange_notice.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A check still running when the next command comes: that command reports it (execute polls),
/// and closing the drawing first loses nothing else.
#[test]
fn a_finished_check_reports_on_the_next_command() {
    hooks();
    let dir = folder("next");
    let mut s = drawing(200);
    s.background_checks = true;
    save(&mut s, &dir.join("Project A.dwg"));
    let start = Instant::now();
    while s.state().unwrap().kept_job.as_ref().is_some_and(|_| s.log.iter().all(|l| !l.contains("Not kept"))) {
        std::thread::sleep(Duration::from_millis(20));
        s.execute("line", &json!({ "points": [[0, 0], [1, 1]] })).unwrap();
        assert!(start.elapsed() < Duration::from_secs(60));
    }
    assert!(s.log.iter().any(|l| l.contains("Not kept in the DWG file")), "{:?}", s.log);
    // Closing a drawing with a check running is fine.
    let mut t = drawing(5_000);
    t.background_checks = true;
    save(&mut t, &dir.join("Closed.dwg"));
    t.execute("close", &json!({ "save": false })).ok();
    std::thread::sleep(Duration::from_millis(50));
    t.poll_background();
    let _ = std::fs::remove_dir_all(&dir);
}
