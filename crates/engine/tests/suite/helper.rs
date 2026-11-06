//! The one shared helper of the suite programs (`tests/suite/<program>/main.rs` includes it as
//! `crate::helper`): a session, the standard file hooks, a program-wide turn for heavy tests, running
//! a command, and a scratch folder.
//!
//! Where a new test goes: a module of the suite program for its area (add `mod my_file;` to that
//! program's `main.rs`), not a new top-level file. A new top-level file is a new program, linked
//! against the whole engine once more. A test that changes process-wide state (IoHooks other than
//! the program's, environment variables, the current folder, global hooks, the job cancel or
//! progress sink) is the exception: it stays a top-level file, so it gets a process of its own.

use cadaddy_engine::Session;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

/// A session whose view is 1200 x 800 pixels (pick and zoom tests measure in pixels).
pub fn session() -> Session {
    let mut s = Session::new();
    s.viewport_px = (1200.0, 800.0);
    s
}

/// Install cadaddy-io's DXF/DWG reader and writer as the file hooks. Only the `files` program's
/// modules call it: the hooks are set once per process, and every module of a program has to agree on
/// them.
pub fn hooks() {
    cadaddy_engine::cmd::file::set_io(cadaddy_engine::cmd::file::IoHooks {
        read: |b, n| cadaddy_io::read(b, n).map_err(|e| e.to_string()),
        write: |d, n| cadaddy_io::write(d, n).map_err(|e| e.to_string()),
        plot: None,
        read_pdf: None,
        read_dwf: None,
        write_dwfx: None,
    });
}

/// One heavy test at a time in this program (whole jobs, large drawings): hold the guard for
/// the test's length. When each test file was its own program, its tests took turns on a lock of
/// its own; the modules of one program now take turns on this one.
static SERIAL: Mutex<()> = Mutex::new(());

pub fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run a command's JSON form; a failure fails the test with the command, its params and the error.
pub fn run(s: &mut Session, cmd: &str, p: Value) -> Value {
    s.execute(cmd, &p).unwrap_or_else(|e| panic!("{cmd} {p}: {e}"))
}

/// An empty scratch folder of this process for `tag` (made fresh; any older one is removed).
pub fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cadaddy-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("scratch folder {}: {e}", d.display()));
    d
}
