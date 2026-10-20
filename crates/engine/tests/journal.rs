//! The autosave journal: a session killed mid-way (a child process, SIGKILL) is recovered from its
//! autosave file as it was after its last finished command — at most the command in progress is
//! lost, not up to SAVETIME minutes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cadaddy_engine::Session;
use cadaddy_engine::autosave::{self, Autosaver};
use cadaddy_engine::cmd::file::{CdyHooks, CdyState, CdyWrite, IoHooks, set_cdy_hooks, set_io};
use cadaddy_engine::doc::EntityKind;
use serde_json::json;

const CHILD: &str = "CADADDY_JOURNAL_CHILD";

fn hooks() {
    set_io(IoHooks {
        read: |b, name| cadaddy_io::read(b, name).map_err(|e| e.to_string()),
        write: |d, name| cadaddy_io::write(d, name).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
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
        seed: |d, bytes| cadaddy_io::cdy::seed(d, bytes).map(|s| Arc::new(s) as CdyState),
    });
}

/// The child: a session drawing a circle per command, the journal written after each; the number
/// of commands whose change is in the file goes to `progress` (written whole, then renamed).
fn child(dir: &Path) {
    hooks();
    autosave::set_savetime(10);
    autosave::set_savefilepath(&dir.to_string_lossy());
    let mut s = Session::new();
    // Something big enough that each append is far smaller than the file.
    let pts: Vec<_> = (0..3000).map(|i| json!([[i, -10], [i, -11]])).collect();
    for p in pts {
        s.execute("line", &json!({ "points": p })).unwrap();
    }
    let mut a = Autosaver::new(autosave::instance_id());
    for i in 1..=10_000u32 {
        s.execute("circle", &json!({ "center": [i, 0], "radius": 1 })).unwrap();
        a.tick(&s);
        while !a.idle() {
            std::thread::sleep(Duration::from_millis(1));
        }
        let tmp = dir.join("progress.tmp");
        std::fs::write(&tmp, i.to_string()).unwrap();
        std::fs::rename(&tmp, dir.join("progress")).unwrap();
    }
    // Wait to be killed.
    std::thread::sleep(Duration::from_secs(600));
}

#[test]
fn a_killed_session_loses_at_most_the_command_in_progress() {
    if let Some(dir) = std::env::var_os(CHILD) {
        child(Path::new(&dir));
        return;
    }
    hooks();
    let dir: PathBuf = std::env::temp_dir().join(format!("cadaddy-journal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut kid = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["a_killed_session_loses_at_most_the_command_in_progress", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, &dir)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let progress = |d: &Path| std::fs::read_to_string(d.join("progress")).ok().and_then(|t| t.trim().parse::<u32>().ok()).unwrap_or(0);
    let started = Instant::now();
    while progress(&dir) < 40 {
        assert!(started.elapsed() < Duration::from_secs(120), "the child made no progress");
        std::thread::sleep(Duration::from_millis(5));
    }
    // Killed between two commands, at no particular moment.
    std::thread::sleep(Duration::from_millis(7));
    kid.kill().unwrap();
    kid.wait().unwrap();
    let done = progress(&dir);
    // The autosave left behind, read as Drawing Recovery reads it.
    let file = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == autosave::EXT))
        .expect("an autosave file left behind");
    let d = cadaddy_io::read(&std::fs::read(&file).unwrap(), &file.to_string_lossy()).unwrap();
    let mut circles: Vec<u32> =
        d.model.iter().filter_map(|e| if let EntityKind::Circle(c) = &e.kind { Some(c.center.x as u32) } else { None }).collect();
    circles.sort_unstable();
    let n = circles.len() as u32;
    assert!(n == done || n == done + 1, "recovered {n} circles; {done} commands were journalled before the kill");
    assert_eq!(circles, (1..=n).collect::<Vec<_>>(), "exactly the drawing after command {n}");
    assert_eq!(d.model.iter().filter(|e| matches!(e.kind, EntityKind::Line(_))).count(), 3000);
    // Each command added a little to the file, not a copy of the drawing.
    let size = std::fs::metadata(&file).unwrap().len() as usize;
    let whole = cadaddy_io::write(&d, "x.cdy").unwrap().len();
    eprintln!("journal: {n} commands, file {size} bytes, the drawing whole {whole} bytes");
    assert!(size < whole * 10, "{size} bytes for {n} appends of a {whole}-byte drawing: appended, not rewritten each time");
    std::fs::remove_dir_all(&dir).ok();
}
