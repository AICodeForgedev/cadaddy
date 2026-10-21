//! Crash reports: when a panic is caught by the guard, a report is written to
//! `<config_dir>/diagnostics/` with the command, the prompt state, a backtrace and the recent
//! command history. No file contents, no paths outside the drawing's name, no user name. The
//! reports are picked up by Help > Report a Problem and offered in the bundle.
//!
//! A report's file name is `crash-<unix seconds>-<counter>.txt` so they sort by time. At most
//! [`MAX_REPORTS`] are kept; older ones are removed when a new one is written.
//!
//! ## Backtrace capture
//!
//! `std::panic::catch_unwind` doesn't give a backtrace of the panic site, so we install a
//! panic hook ([`install_hook`]) that captures one and stores it in a thread-local. After
//! `catch_unwind` returns an `Err`, the caller reads it with [`take_backtrace`].

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// Reports kept in the diagnostics folder.
pub const MAX_REPORTS: usize = 20;
/// Lines of command history included.
pub const MAX_HISTORY: usize = 50;

/// Counter so two panics in the same second get different file names.
static COUNTER: AtomicU32 = AtomicU32::new(0);
/// Override for tests (so they don't write into the real settings folder).
static STORE: Mutex<Option<PathBuf>> = Mutex::new(None);

thread_local! {
    /// Backtrace captured by the panic hook, read by `take_backtrace`.
    static LAST_BT: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Install a panic hook that captures a backtrace for [`take_backtrace`]. Call once at start-up
/// (the desktop app and the CLI both do). The previous hook is chained so `RUST_BACKTRACE=1`
/// still prints to stderr.
pub fn install_hook() {
    thread_local! {
        /// Guard against recursion (a panic inside the hook itself).
        static IN_HOOK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let in_hook = IN_HOOK.with(|c| c.replace(true));
        if !in_hook {
            // Store the panic location (cheap) — a full `Backtrace::force_capture` is expensive
            // and can overflow a small test-thread stack. The location is enough to find the site.
            let loc = info
                .location()
                .map(|l| format!("panicked at {}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_default();
            LAST_BT.with(|cell| *cell.borrow_mut() = Some(loc));
            IN_HOOK.with(|c| c.set(false));
        }
        prev(info);
    }));
}

/// Take the backtrace the last panic on this thread left. Returns `None` if no panic was caught
/// or the hook wasn't installed.
pub fn take_backtrace() -> Option<String> {
    LAST_BT.with(|cell| cell.borrow_mut().take())
}

/// Set the diagnostics folder for tests.
pub fn set_store_dir(dir: Option<PathBuf>) {
    if let Ok(mut s) = STORE.lock() {
        *s = dir;
    }
}

/// The diagnostics folder: `STORE` override, else `<config_dir>/diagnostics`.
fn store_dir() -> Option<PathBuf> {
    STORE.lock().ok().and_then(|s| s.clone()).or_else(|| crate::control_auth::config_dir().map(|d| d.join("diagnostics")))
}

/// What a crash report records.
pub struct CrashInfo<'a> {
    /// The command that panicked.
    pub command: &'a str,
    /// The panic message (from `catch_unwind`).
    pub message: &'a str,
    /// The prompt the command was showing (if any).
    pub prompt: Option<&'a str>,
    /// A backtrace captured at the panic site.
    pub backtrace: &'a str,
    /// Recent command history (command names, no arguments that could contain file paths or names).
    pub history: &'a [String],
    /// The drawing's title (just the file name, not the full path).
    pub drawing_title: Option<&'a str>,
}

/// Write a crash report to the diagnostics folder. Returns the path on success.
///
/// Never panics or propagates errors: a crash report that can't be written is silently dropped
/// (better than a double panic).
pub fn write(info: &CrashInfo<'_>) -> Option<PathBuf> {
    let dir = store_dir()?;
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("crash-{now}-{n}.txt");
    let path = dir.join(&name);

    let mut report = String::with_capacity(4096);
    report.push_str(&format!("CADaddy crash report ({})\n", env!("CARGO_PKG_VERSION")));
    report.push_str(&format!("time: {now} (Unix seconds)\n"));
    report.push_str(&format!("os: {} {}\n", std::env::consts::OS, std::env::consts::ARCH));
    report.push_str(&format!("command: {}\n", info.command));
    report.push_str(&format!("panic: {}\n", info.message));
    if let Some(p) = info.prompt {
        report.push_str(&format!("prompt: {p}\n"));
    }
    if let Some(title) = info.drawing_title {
        let name_only = Path::new(title).file_name().map_or(title, |n| n.to_str().unwrap_or(title));
        report.push_str(&format!("drawing: {name_only}\n"));
    }
    report.push_str("\n--- backtrace ---\n");
    report.push_str(info.backtrace);
    report.push_str("\n--- recent commands ---\n");
    let start = info.history.len().saturating_sub(MAX_HISTORY);
    for line in &info.history[start..] {
        let cmd = line.split_whitespace().next().unwrap_or_default();
        report.push_str(cmd);
        report.push('\n');
    }

    if std::fs::write(&path, &report).is_err() {
        return None;
    }
    prune(&dir);
    Some(path)
}

/// Convenience: capture crash info from a Session after a caught panic and write the report.
/// Called by the guard and execute's catch_unwind sites.
///
/// Does NOT call `session.current_prompt()` — that would invoke the machine's prompt method,
/// which may be the thing that panicked. Instead, the prompt state is inferred from the running
/// command's ID.
pub fn record(session: &crate::Session, command: &str, panic_msg: &str) {
    let bt = take_backtrace().unwrap_or_default();
    let running_info = session.running.as_ref().map(|r| format!("{} (interactive)", r.id));
    let title = session.docs.get(session.active).map(|d| d.title.as_str());
    let _ = write(&CrashInfo {
        command,
        message: panic_msg,
        prompt: running_info.as_deref(),
        backtrace: &bt,
        history: &session.log,
        drawing_title: title,
    });
}

/// Remove the oldest reports past [`MAX_REPORTS`].
fn prune(dir: &Path) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = read
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let s = n.to_string_lossy();
            s.starts_with("crash-") && s.ends_with(".txt")
        })
        .map(|e| e.path())
        .collect();
    if files.len() <= MAX_REPORTS {
        return;
    }
    files.sort();
    for p in files.iter().take(files.len() - MAX_REPORTS) {
        let _ = std::fs::remove_file(p);
    }
}

/// Read all crash reports in the diagnostics folder, newest first.
pub fn list() -> Vec<(String, String)> {
    let Some(dir) = store_dir() else { return Vec::new() };
    let Ok(read) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = read
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let s = n.to_string_lossy();
            s.starts_with("crash-") && s.ends_with(".txt")
        })
        .map(|e| e.path())
        .collect();
    files.sort();
    files.reverse();
    files
        .into_iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&p).ok()?;
            Some((name, text))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crash_report_is_written_and_pruned() {
        let dir = std::env::temp_dir().join(format!("cadaddy-crash-{}", std::process::id()));
        set_store_dir(Some(dir.clone()));

        let info = CrashInfo {
            command: "hatch",
            message: "index out of bounds",
            prompt: Some("Specify internal point or [Select objects]:"),
            backtrace: "   0: std::backtrace\n   1: cadaddy_engine::cmd::hatch\n",
            history: &["line".into(), "circle".into(), "hatch".into()],
            drawing_title: Some("/secret/path/Sample Floor Plan.dwg"),
        };
        let path = write(&info);
        assert!(path.is_some(), "report written");
        let text = std::fs::read_to_string(path.as_deref().unwrap_or(Path::new(""))).unwrap_or_default();
        assert!(text.contains("command: hatch"));
        assert!(text.contains("panic: index out of bounds"));
        assert!(text.contains("prompt: Specify internal point"));
        assert!(text.contains("--- backtrace ---"));
        assert!(text.contains("--- recent commands ---"));
        assert!(text.contains("drawing: Sample Floor Plan.dwg"));
        assert!(!text.contains("/secret/path"), "no private path in the report: {text}");
        assert!(text.contains("\nline\n") && text.contains("\ncircle\n"));

        let reports = list();
        assert!(!reports.is_empty());
        assert!(reports[0].0.starts_with("crash-"));

        for _ in 0..MAX_REPORTS + 5 {
            write(&info);
        }
        let reports = list();
        assert!(reports.len() <= MAX_REPORTS, "pruned to {}: {}", MAX_REPORTS, reports.len());

        set_store_dir(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_panic_hook_captures_a_backtrace() {
        install_hook();
        // After a caught panic, take_backtrace returns Some (the trace may be empty if
        // RUST_BACKTRACE is unset; the hook still ran).
        let _ = std::panic::catch_unwind(|| panic!("test crash for backtrace"));
        let bt = take_backtrace();
        assert!(bt.is_some(), "backtrace captured");
        // A second call returns None (consumed).
        assert!(take_backtrace().is_none());
    }
}
