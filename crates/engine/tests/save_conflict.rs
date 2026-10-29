//! A save never silently overwrites a file something else changed since the drawing was opened or
//! last saved: two sessions on one file, a file replaced from outside, `force`, our own earlier
//! save, a file deleted meanwhile.

use cadaddy_doc::Drawing;
use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{IoHooks, set_io};
use serde_json::json;

/// A reader and a writer whose bytes say how many entities the drawing had.
fn install() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_io(IoHooks {
        read: |_, _| Ok(Drawing::new_metric()),
        write: |d, _| Ok(format!("entities={}", d.entity_count()).into_bytes()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
    guard
}

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-save-conflict-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn opened(path: &str) -> Session {
    let mut s = Session::new();
    s.execute("open", &json!({ "path": path })).unwrap();
    s
}

/// A bumped modification time, so the test doesn't depend on the file system's clock grain.
fn later(path: &std::path::Path) {
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
}

#[test]
fn two_sessions_on_one_file_second_save_is_refused() {
    let _t = install();
    let d = dir("two");
    let path = d.join("plan.dxf");
    std::fs::write(&path, b"seed").unwrap();
    let p = path.to_string_lossy().to_string();
    let mut a = opened(&p);
    let mut b = opened(&p);
    a.cmdline("circle 0,0 5").unwrap();
    b.cmdline("line 0,0 3,3").unwrap();
    b.cmdline("line 1,1 4,4").unwrap();
    a.execute("qsave", &json!({})).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"entities=1");
    let e = b.execute("qsave", &json!({})).unwrap_err().to_string();
    assert!(e.contains(&p) && e.contains("changed on disk") && e.contains("force"), "{e}");
    // The file keeps the first writer's content, and the refused session still has its changes.
    assert_eq!(std::fs::read(&path).unwrap(), b"entities=1");
    assert!(b.state().unwrap().is_dirty());
}

#[test]
fn a_file_replaced_from_outside_is_not_saved_over() {
    let _t = install();
    let d = dir("outside");
    let path = d.join("plan.dxf");
    std::fs::write(&path, b"seed").unwrap();
    let p = path.to_string_lossy().to_string();
    let mut s = opened(&p);
    // Replaced by another drawing of another size.
    std::fs::write(&path, b"another drawing, longer").unwrap();
    s.cmdline("circle 0,0 5").unwrap();
    for cmd in ["qsave", "saveas"] {
        let e = s.execute(cmd, &json!({ "path": p })).unwrap_err().to_string();
        assert!(e.contains("changed on disk") && e.contains("size"), "{cmd}: {e}");
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"another drawing, longer");
    // Same size, other contents, newer stamp: caught by the contents.
    std::fs::write(&path, b"SEED").unwrap();
    let mut s = opened(&p);
    std::fs::write(&path, b"sEeD").unwrap();
    later(&path);
    s.cmdline("circle 0,0 5").unwrap();
    let e = s.execute("qsave", &json!({})).unwrap_err().to_string();
    assert!(e.contains("contents changed"), "{e}");
}

#[test]
fn force_overwrites_and_says_so() {
    let _t = install();
    let d = dir("force");
    let path = d.join("plan.dxf");
    std::fs::write(&path, b"seed").unwrap();
    let p = path.to_string_lossy().to_string();
    let mut s = opened(&p);
    std::fs::write(&path, b"changed by someone else").unwrap();
    s.cmdline("circle 0,0 5").unwrap();
    assert!(s.execute("qsave", &json!({})).is_err());
    let r = s.execute("qsave", &json!({ "force": true })).unwrap();
    assert!(r["note"].as_str().is_some_and(|n| n.contains("force")), "{r}");
    assert_eq!(std::fs::read(&path).unwrap(), b"entities=1");
    // Forcing is for that one save: the next needs nothing.
    s.cmdline("circle 0,0 6").unwrap();
    s.execute("qsave", &json!({})).unwrap();
    // And SAVEAS onto the opened path takes it too.
    std::fs::write(&path, b"changed again, differently").unwrap();
    assert!(s.execute("saveas", &json!({ "path": p })).is_err());
    s.execute("saveas", &json!({ "path": p, "force": true })).unwrap();
}

#[test]
fn a_save_after_our_own_save_needs_no_force() {
    let _t = install();
    let d = dir("own");
    let path = d.join("plan.dxf");
    std::fs::write(&path, b"seed").unwrap();
    let p = path.to_string_lossy().to_string();
    let mut s = opened(&p);
    for n in 1..=3 {
        s.cmdline(&format!("circle 0,0 {n}")).unwrap();
        let r = s.execute("qsave", &json!({})).unwrap();
        assert!(r.get("note").is_none(), "{r}");
        assert_eq!(std::fs::read(&path).unwrap(), format!("entities={n}").into_bytes());
    }
    // A file only touched (same contents, newer stamp) is not a change.
    later(&path);
    s.cmdline("circle 0,0 9").unwrap();
    s.execute("qsave", &json!({})).unwrap();
    // A new drawing saved to a name nobody has yet, then saved again.
    let mut n = Session::new();
    let q = d.join("fresh.dxf").to_string_lossy().to_string();
    n.execute("saveas", &json!({ "path": q })).unwrap();
    n.execute("qsave", &json!({})).unwrap();
}

#[test]
fn a_file_deleted_since_open_is_saved_with_a_note() {
    let _t = install();
    let d = dir("gone");
    let path = d.join("plan.dxf");
    std::fs::write(&path, b"seed").unwrap();
    let p = path.to_string_lossy().to_string();
    let mut s = opened(&p);
    std::fs::remove_file(&path).unwrap();
    s.cmdline("circle 0,0 5").unwrap();
    let r = s.execute("qsave", &json!({})).unwrap();
    assert!(r["note"].as_str().is_some_and(|n| n.contains("deleted")), "{r}");
    assert_eq!(std::fs::read(&path).unwrap(), b"entities=1");
}

#[test]
fn the_agent_question_names_a_changed_file() {
    let _t = install();
    let d = dir("ask");
    let path = d.join("plan.dxf");
    std::fs::write(&path, b"seed").unwrap();
    let p = path.to_string_lossy().to_string();
    let s = opened(&p);
    assert_eq!(s.changed_on_disk(&p), None);
    std::fs::write(&path, b"seed plus more").unwrap();
    assert!(s.changed_on_disk(&p).is_some_and(|how| how.contains("size")));
}
