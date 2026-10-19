//! Incremental .cdy saves (Q99): QSAVE appends only what changed to the file the drawing was saved
//! to or read from, a file changed by anything else is written in full, a save killed partway
//! leaves the previous save readable, and autosave writes .cdy without blocking.

use std::path::PathBuf;
use std::sync::Arc;

use cadaddy_engine::Session;
use cadaddy_engine::cmd::file::{CdyHooks, CdyState, CdyWrite, IoHooks, set_cdy_hooks, set_io};
use serde_json::json;

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
    });
    // As the apps install it.
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

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cadaddy-cdyfast-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A session with `n` lines drawn.
fn drawn(n: usize) -> Session {
    hooks();
    let mut s = Session::new();
    let pts: Vec<_> = (0..n).map(|i| json!([[i, 0], [i, 1]])).collect();
    for p in pts {
        s.execute("line", &json!({ "points": p })).unwrap();
    }
    s
}

fn len(p: &str) -> u64 {
    std::fs::metadata(p).unwrap().len()
}

#[test]
fn qsave_appends_only_what_changed() {
    let dir = folder("qsave");
    let p = dir.join("Big.cdy").to_string_lossy().into_owned();
    let mut s = drawn(3000);
    let r = s.execute("saveas", &json!({ "path": p })).unwrap();
    assert!(r.get("incremental").is_none(), "the first save is a full one: {r}");
    let first = len(&p);
    s.execute("line", &json!({ "points": [[0, 5], [9, 5]] })).unwrap();
    let r = s.execute("qsave", &json!({})).unwrap();
    assert_eq!(r["incremental"], json!(true), "{r}");
    assert!(r["bytes"].as_u64().unwrap() < first / 2, "{r} (file was {first})");
    assert_eq!(len(&p), first + r["bytes"].as_u64().unwrap(), "appended, not rewritten");
    // Read back exactly; and the drawing opened from it saves incrementally too.
    let mut t = Session::new();
    t.execute("open", &json!({ "path": p })).unwrap();
    assert_eq!(t.doc().unwrap().model, s.doc().unwrap().model);
    t.execute("erase", &json!({ "handles": [t.doc().unwrap().model.iter().next().unwrap().handle.hex()] })).unwrap();
    let r = t.execute("qsave", &json!({})).unwrap();
    assert_eq!(r["incremental"], json!(true), "{r}");
    let mut u = Session::new();
    u.execute("open", &json!({ "path": p })).unwrap();
    assert_eq!(u.doc().unwrap().model, t.doc().unwrap().model);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_file_changed_by_something_else_is_written_in_full() {
    let dir = folder("changed");
    let p = dir.join("Shared.cdy").to_string_lossy().into_owned();
    let mut s = drawn(50);
    s.execute("saveas", &json!({ "path": p })).unwrap();
    // Another program (or copy) replaces the file.
    let mut other = drawn(5);
    other.execute("saveas", &json!({ "path": dir.join("other.cdy").to_string_lossy() })).unwrap();
    std::fs::copy(dir.join("other.cdy"), &p).unwrap();
    s.execute("line", &json!({ "points": [[0, 5], [9, 5]] })).unwrap();
    let r = s.execute("qsave", &json!({})).unwrap();
    assert!(r.get("incremental").is_none(), "{r}");
    let mut t = Session::new();
    t.execute("open", &json!({ "path": p })).unwrap();
    assert_eq!(t.doc().unwrap().model, s.doc().unwrap().model);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Run by [`a_writer_killed_mid_save_leaves_a_readable_file`] in a child process: keep editing
/// every chunk of a large drawing and saving it, until killed.
#[test]
fn child_keeps_saving() {
    let Ok(p) = std::env::var("CADADDY_CDY_CHILD") else { return };
    let mut s = Session::new();
    hooks();
    s.execute("open", &json!({ "path": p })).unwrap();
    let handles: Vec<String> = s.doc().unwrap().model.iter().step_by(512).map(|e| e.handle.hex()).collect();
    for i in 0..1_000_000u64 {
        for h in &handles {
            s.execute("move", &json!({ "handles": [h], "from": [0, 0], "to": [0, (i % 7) as f64] })).unwrap();
        }
        s.execute("qsave", &json!({})).unwrap();
        std::fs::write(format!("{p}.saves"), i.to_string()).unwrap();
    }
}

#[test]
fn a_writer_killed_mid_save_leaves_a_readable_file() {
    let dir = folder("kill");
    let p = dir.join("Killed.cdy").to_string_lossy().into_owned();
    let mut s = drawn(30_000);
    s.execute("saveas", &json!({ "path": p })).unwrap();
    let n = s.doc().unwrap().model.len();
    let exe = std::env::current_exe().unwrap();
    for (round, ms) in [150u64, 400, 700, 1100, 1700].into_iter().enumerate() {
        let mut child = std::process::Command::new(&exe)
            .args(["child_keeps_saving", "--exact", "--nocapture", "--test-threads", "1"])
            .env("CADADDY_CDY_CHILD", &p)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(ms));
        child.kill().unwrap();
        let _ = child.wait();
        // Whatever the moment, the file opens whole: no part left out, every object there.
        let bytes = std::fs::read(&p).unwrap();
        let (d, rep) = cadaddy_io::cdy::read_cdy_report(&bytes).unwrap_or_else(|e| panic!("round {round}: {e}"));
        assert!(rep.notes.iter().all(|n| n.contains("index")), "round {round}: {:?}", rep.notes);
        assert_eq!(d.model.len(), n, "round {round}");
    }
    let saves = std::fs::read_to_string(format!("{p}.saves")).unwrap_or_default();
    assert!(!saves.is_empty(), "the child saved at least once");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn autosave_writes_cdy_without_blocking() {
    use cadaddy_engine::autosave::{Autosaver, Job};
    let dir = folder("autosave");
    let s = drawn(1);
    let mut big = s.doc().unwrap().clone();
    for i in 0..150_000u64 {
        let x = i as f64;
        let _ = big.add(
            &cadaddy_engine::doc::Space::Model,
            Default::default(),
            cadaddy_engine::doc::EntityKind::Point(cadaddy_engine::doc::Point { p: cadaddy_engine::doc::geom::Vec3::new(x, x, 0.0), angle: 0.0 }),
        );
    }
    let path = dir.join("Big_1-a_1.sv$").to_string_lossy().into_owned();
    let mut saver = Autosaver::new("1-a");
    let t = std::time::Instant::now();
    saver.perform(vec![Job::Write { uid: 1, path: path.clone(), doc: Arc::new(big.clone()) }]);
    let returned = t.elapsed();
    let mut waited = 0;
    while !std::path::Path::new(&path).exists() && waited < 600 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        saver.reap();
        waited += 1;
    }
    let written = t.elapsed();
    assert!(returned * 4 < written, "perform returned in {returned:?}, the write took {written:?}");
    let back = cadaddy_io::read(&std::fs::read(&path).unwrap(), &path).unwrap();
    assert!(cadaddy_io::cdy::is_cdy(&std::fs::read(&path).unwrap()));
    assert_eq!(back.model, big.model);
    let _ = std::fs::remove_dir_all(&dir);
}
