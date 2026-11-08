//! The desktop app's logger: `log::` records go to standard error and to a log file.
//!
//! Without a logger every `log::warn!`/`log::error!`/`log::info!` in the workspace (a broken app
//! icon, the GPU canvas set-up, the control server) vanished. A launch from a desktop menu has no
//! terminal, so the file is what a bug report can attach: `<settings dir>/logs/cadaddy.log`
//! (`CADADDY_CONFIG_DIR`, or the platform settings directory; see `config_dir` in `main.rs`).
//! Each start moves the previous log to `cadaddy.1.log` (and that one to `.2`), so the log of a
//! run that crashed survives the next launch.
//!
//! Levels: `info` for CADaddy's own crates, `warn` for everything else (wgpu and naga are
//! chatty). `RUST_LOG` replaces that with env_logger-style directives: `debug`,
//! `warn,cadaddy_io=trace`, `wgpu_core=info`. A directive ending in `*` matches every target
//! that starts with it (`cadaddy*=debug`). `RUST_LOG=cadaddy_engine=debug` also logs every
//! command-line message (`Session::echo`).
//!
//! Records logged before the settings directory is known are kept (up to [`MAX_PENDING`]) and
//! written once the file is attached. Writing never panics. A record takes the sink lock only for
//! one `write_all` of an already formatted line (standard error is written outside the lock).
//!
//! No single message can take the log over ([`Repeats`]): a message that keeps coming (the same
//! target, level and text once numbers are set aside) is written the first
//! [`REPEAT_FIRST_OWN`] times from CADaddy's crates, [`REPEAT_FIRST_OTHER`] times from other
//! crates, then counted: a `[repeated]` line at 100, 1,000, 10,000 … copies, and a final count when
//! the message goes quiet for [`REPEAT_QUIET`] or the log is flushed (at exit). A message that
//! comes back after going quiet (the next file) starts its count again. `CADADDY_LOG_REPEATS=all`
//! writes every copy.
//!
//! When the file reaches [`MAX_FILE_BYTES`] it is moved to [`EARLIER_FILE`] and a fresh
//! `cadaddy.log` continues, so the newest records are always kept and the disk use stays bounded
//! (the earlier half of a run that overflowed is dropped at the next start). When the settings
//! directory can't hold a log, [`fallback_dir`] (in the temporary folder) is tried: a desktop
//! build launched from a menu has no console, so standard error alone would lose everything.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use log::LevelFilter;

/// The current log file's name inside the log directory.
pub const LOG_FILE: &str = "cadaddy.log";
/// The log directory inside the settings directory.
pub const LOG_DIR: &str = "logs";
/// How many previous logs are kept (`cadaddy.1.log` … `cadaddy.<KEEP>.log`).
pub const KEEP: usize = 2;
/// Past this size the log file is set aside as [`EARLIER_FILE`] and a fresh one continues (a
/// runaway warning can't fill the disk, and the newest records are kept).
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// The earlier part of a run whose log reached [`MAX_FILE_BYTES`].
pub const EARLIER_FILE: &str = "cadaddy.earlier.log";
/// Copies of one message from CADaddy's own crates written before the rest are only counted.
pub const REPEAT_FIRST_OWN: u64 = 50;
/// Copies of one message from any other crate (libraries repeat a warning per object) written
/// before the rest are only counted.
pub const REPEAT_FIRST_OTHER: u64 = 5;
/// A repeating message silent this long gets its final count, and starts over when it returns.
pub const REPEAT_QUIET: Duration = Duration::from_secs(30);
/// Distinct messages followed at once; past this every count is written out and the list restarts.
pub const MAX_REPEAT_KEYS: usize = 4096;
/// Characters of a message used to tell messages apart (and quoted in a `[repeated]` line).
const REPEAT_KEY_CHARS: usize = 200;
/// Records kept in memory until the log file is attached.
pub const MAX_PENDING: usize = 512;
/// The built-in filter when `RUST_LOG` is unset or empty.
pub const DEFAULT_FILTER: &str = "warn,cadaddy*=info";

/// Per-target level filter parsed from env_logger-style directives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    default: LevelFilter,
    directives: Vec<(String, LevelFilter)>,
}

impl Filter {
    /// Parse `spec`; unknown levels and empty parts are skipped. Without a bare level, targets no
    /// directive names log errors only.
    pub fn parse(spec: &str) -> Filter {
        let mut f = Filter { default: LevelFilter::Error, directives: Vec::new() };
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part.split_once('=') {
                Some((name, level)) => {
                    let name = name.trim();
                    if let (false, Some(level)) = (name.is_empty(), parse_level(level)) {
                        f.directives.push((name.to_owned(), level));
                    }
                }
                None => match parse_level(part) {
                    Some(level) => f.default = level,
                    // A bare target name: everything from it (env_logger does the same).
                    None => f.directives.push((part.to_owned(), LevelFilter::Trace)),
                },
            }
        }
        f
    }

    /// The level that applies to `target` (the most specific matching directive wins).
    pub fn level_for(&self, target: &str) -> LevelFilter {
        self.directives.iter().filter(|(name, _)| matches(name, target)).max_by_key(|(name, _)| name.len()).map_or(self.default, |(_, level)| *level)
    }

    /// The most verbose level any target can reach (for `log::set_max_level`).
    pub fn max(&self) -> LevelFilter {
        self.directives.iter().map(|(_, level)| *level).fold(self.default, Ord::max)
    }
}

fn parse_level(s: &str) -> Option<LevelFilter> {
    s.trim().parse().ok()
}

/// `name` matches `target` itself and its submodules (`a` matches `a` and `a::b`, not `ab`);
/// `name*` matches every target starting with `name`.
fn matches(name: &str, target: &str) -> bool {
    match name.strip_suffix('*') {
        Some(prefix) => target.starts_with(prefix),
        None => target.strip_prefix(name).is_some_and(|rest| rest.is_empty() || rest.starts_with("::")),
    }
}

/// `2026-10-08T07:59:17.728Z` (UTC, milliseconds). A clock before 1970 reads as the epoch.
pub fn timestamp(t: SystemTime) -> String {
    let since = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z", rem / 3600, rem % 3600 / 60, rem % 60, since.subsec_millis())
}

/// Days since 1970-01-01 to a proleptic Gregorian (year, month, day) (Howard Hinnant's
/// `civil_from_days`, unsigned because `timestamp` clamps at the epoch; saturating so an absurd
/// clock can't overflow).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days.saturating_add(719_468);
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = era.saturating_mul(400).saturating_add(yoe).saturating_add(u64::from(m <= 2));
    (y, m, d)
}

fn numbered(dir: &Path, n: usize) -> PathBuf {
    if n == 0 { dir.join(LOG_FILE) } else { dir.join(format!("cadaddy.{n}.log")) }
}

/// Shift the previous logs up one (`.log` → `.1.log` → … → `.<KEEP>.log`, the oldest dropped)
/// and create a fresh, empty log file in `dir` (created if missing). The earlier part of the
/// previous run ([`EARLIER_FILE`], left when its log overflowed) is removed: its newest part is
/// the one kept.
pub fn rotate(dir: &Path) -> std::io::Result<(PathBuf, File)> {
    std::fs::create_dir_all(dir)?;
    match std::fs::remove_file(dir.join(EARLIER_FILE)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    for n in (1..=KEEP).rev() {
        match std::fs::rename(numbered(dir, n - 1), numbered(dir, n)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    let path = numbered(dir, 0);
    let file = File::create(&path)?;
    Ok((path, file))
}

/// One formatted log line: `<timestamp> <LEVEL> [<thread>] <target>: <message>\n`.
pub fn format_line(ts: &str, level: log::Level, thread: &str, target: &str, message: &str) -> String {
    let message = message.strip_suffix('\n').unwrap_or(message);
    format!("{ts} {level:<5} [{thread}] {target}: {message}\n")
}

/// Where formatted lines go besides standard error: kept in memory until a file is attached,
/// then appended to it. At its size cap the file is set aside ([`EARLIER_FILE`], when the
/// directory is known; otherwise emptied) and writing continues in a fresh one.
pub struct Sink {
    file: Option<File>,
    dir: Option<PathBuf>,
    pending: Vec<String>,
    dropped: usize,
    written: u64,
    max_bytes: u64,
    closed: bool,
}

impl Sink {
    pub fn new(max_bytes: u64) -> Sink {
        Sink { file: None, dir: None, pending: Vec::new(), dropped: 0, written: 0, max_bytes, closed: false }
    }

    /// Append `line` to the file, or keep it until [`Sink::attach`].
    pub fn write(&mut self, line: &str) {
        if self.file.is_none() {
            if self.closed {
                return;
            }
            if self.pending.len() < MAX_PENDING {
                self.pending.push(line.to_owned());
            } else {
                self.dropped = self.dropped.saturating_add(1);
            }
            return;
        }
        self.append(line);
    }

    /// Start writing to `file`, first the lines kept so far.
    pub fn attach(&mut self, file: File) {
        self.file = Some(file);
        self.written = 0;
        self.closed = false;
        for line in std::mem::take(&mut self.pending) {
            self.append(&line);
        }
        if self.dropped > 0 {
            let note = format!("{} earlier log lines were dropped before the log file was opened\n", self.dropped);
            self.dropped = 0;
            self.append(&note);
        }
    }

    /// Start writing to `file`, which is `dir`'s `cadaddy.log`: at the size cap it is moved to
    /// [`EARLIER_FILE`] there.
    pub fn attach_in(&mut self, dir: &Path, file: File) {
        self.dir = Some(dir.to_path_buf());
        self.attach(file);
    }

    /// No file will come: drop the kept lines and keep none from now on.
    pub fn close(&mut self) {
        self.file = None;
        self.pending = Vec::new();
        self.closed = true;
    }

    fn append(&mut self, line: &str) {
        let len = u64::try_from(line.len()).unwrap_or(u64::MAX);
        if self.written.saturating_add(len) > self.max_bytes && self.written > 0 {
            self.start_over();
        }
        let Some(file) = self.file.as_mut() else { return };
        if file.write_all(line.as_bytes()).is_ok() {
            self.written = self.written.saturating_add(len);
            return;
        }
        // A full disk or a vanished file must not take the app down: try a fresh file once, else
        // drop the file sink (and stop buffering, since no file will come).
        if self.reopen() && self.file.as_mut().is_some_and(|f| f.write_all(line.as_bytes()).is_ok()) {
            self.written = self.written.saturating_add(len);
        } else {
            self.close();
        }
    }

    /// The file is full: set it aside (or empty it when there is nowhere to move it) and go on in
    /// a fresh one that says where the earlier records went.
    fn start_over(&mut self) {
        let note = match &self.dir {
            Some(dir) => {
                let current = dir.join(LOG_FILE);
                // Unix moves an open file; Windows needs it closed first.
                self.file = None;
                match std::fs::rename(&current, dir.join(EARLIER_FILE)).and_then(|()| File::create(&current)) {
                    Ok(file) => {
                        self.file = Some(file);
                        format!("log file reached {} bytes; the earlier records of this run are in {EARLIER_FILE}\n", self.max_bytes)
                    }
                    Err(e) => match File::create(&current) {
                        // Couldn't set it aside: start it over, so the newest records still land.
                        Ok(file) => {
                            self.file = Some(file);
                            format!(
                                "log file reached {} bytes and couldn't be set aside ({e}); the earlier records of this run are gone\n",
                                self.max_bytes
                            )
                        }
                        Err(_) => {
                            self.close();
                            return;
                        }
                    },
                }
            }
            None => {
                let Some(file) = self.file.as_mut() else { return };
                if file.set_len(0).and_then(|()| file.rewind()).is_err() {
                    self.close();
                    return;
                }
                format!("log file reached {} bytes; the earlier records of this run were dropped\n", self.max_bytes)
            }
        };
        self.written = 0;
        if let Some(file) = self.file.as_mut()
            && file.write_all(note.as_bytes()).is_ok()
        {
            self.written = u64::try_from(note.len()).unwrap_or(u64::MAX);
        }
    }

    /// After a failed write: a fresh `cadaddy.log` in the same directory.
    fn reopen(&mut self) -> bool {
        self.file = None;
        let Some(dir) = &self.dir else { return false };
        match File::create(dir.join(LOG_FILE)) {
            Ok(file) => {
                self.file = Some(file);
                self.written = 0;
                true
            }
            Err(_) => false,
        }
    }
}

/// A message that came before: how often, and how many copies were not written yet.
struct Seen {
    level: log::Level,
    target: String,
    sample: String,
    count: u64,
    unwritten: u64,
    last: Instant,
}

impl Seen {
    fn note(&self) -> Note {
        let text = format!("[repeated] {} more copies not written ({} in all): {}", self.unwritten, self.count, self.sample);
        Note { level: self.level, target: self.target.clone(), text }
    }
}

/// A line the logger writes about repeated messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub level: log::Level,
    pub target: String,
    pub text: String,
}

/// Collapses a message that keeps coming: the first copies are written, the rest counted (see
/// the module docs).
pub struct Repeats {
    seen: HashMap<(log::Level, String, String), Seen>,
    first_own: u64,
    first_other: u64,
    quiet: Duration,
    last_sweep: Option<Instant>,
    enabled: bool,
}

impl Repeats {
    pub fn new(first_own: u64, first_other: u64, quiet: Duration) -> Repeats {
        Repeats { seen: HashMap::new(), first_own, first_other, quiet, last_sweep: None, enabled: true }
    }

    /// Every copy is written (`CADADDY_LOG_REPEATS=all`).
    pub fn off() -> Repeats {
        Repeats { enabled: false, ..Repeats::new(REPEAT_FIRST_OWN, REPEAT_FIRST_OTHER, REPEAT_QUIET) }
    }

    /// Whether this copy of the message is written; `notes` gets the `[repeated]` lines due now.
    pub fn admit(&mut self, level: log::Level, target: &str, message: &str, now: Instant, notes: &mut Vec<Note>) -> bool {
        if !self.enabled {
            return true;
        }
        if self.last_sweep.is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(1)) {
            self.last_sweep = Some(now);
            self.sweep(now, notes);
        }
        let key = (level, target.to_owned(), shape(message));
        if !self.seen.contains_key(&key) && self.seen.len() >= MAX_REPEAT_KEYS {
            self.drain(notes);
            self.seen.clear();
        }
        let first = if is_own(target) { self.first_own } else { self.first_other };
        let seen = self.seen.entry(key).or_insert_with(|| Seen {
            level,
            target: target.to_owned(),
            sample: message.chars().take(REPEAT_KEY_CHARS).collect(),
            count: 0,
            unwritten: 0,
            last: now,
        });
        seen.count = seen.count.saturating_add(1);
        seen.last = now;
        if seen.count <= first {
            return true;
        }
        seen.unwritten = seen.unwritten.saturating_add(1);
        if seen.count == first.saturating_add(1) {
            let text = format!("[repeated] this message keeps coming; further copies are counted, not written: {}", seen.sample);
            notes.push(Note { level, target: target.to_owned(), text });
        } else if is_milestone(seen.count) {
            notes.push(seen.note());
            seen.unwritten = 0;
        }
        false
    }

    /// The final count of every message not heard from for the quiet time; they start over.
    fn sweep(&mut self, now: Instant, notes: &mut Vec<Note>) {
        let quiet = self.quiet;
        self.seen.retain(|_, seen| {
            if now.saturating_duration_since(seen.last) < quiet {
                return true;
            }
            if seen.unwritten > 0 {
                notes.push(seen.note());
            }
            false
        });
    }

    /// The count of every message with copies not written yet (at flush and exit).
    pub fn drain(&mut self, notes: &mut Vec<Note>) {
        for seen in self.seen.values_mut().filter(|s| s.unwritten > 0) {
            notes.push(seen.note());
            seen.unwritten = 0;
        }
    }
}

/// CADaddy's own crates (the `cadaddy*` targets of [`DEFAULT_FILTER`]).
fn is_own(target: &str) -> bool {
    target.starts_with("cadaddy")
}

/// 100, 1,000, 10,000 …
fn is_milestone(n: u64) -> bool {
    n >= 100 && n.checked_ilog10().is_some_and(|e| 10u64.checked_pow(e) == Some(n))
}

/// A message's shape: runs of digits stand for one number (offsets and counts differ between
/// copies of the same message), at most [`REPEAT_KEY_CHARS`] characters.
fn shape(message: &str) -> String {
    let mut out = String::new();
    let mut digits = false;
    for c in message.chars().take(REPEAT_KEY_CHARS) {
        if c.is_ascii_digit() {
            if !digits {
                out.push('#');
            }
            digits = true;
        } else {
            digits = false;
            out.push(c);
        }
    }
    out
}

/// Where the log goes when the settings directory can't hold it: a folder in the temporary folder.
pub fn fallback_dir() -> PathBuf {
    std::env::temp_dir().join("CADaddy").join(LOG_DIR)
}

/// The installed logger: a [`Filter`], [`Repeats`], standard error and a [`Sink`].
pub struct AppLogger {
    filter: Filter,
    stderr: bool,
    repeats: Mutex<Repeats>,
    sink: Mutex<Sink>,
    dir: Mutex<Option<PathBuf>>,
}

impl AppLogger {
    pub fn new(filter: Filter, stderr: bool) -> AppLogger {
        AppLogger::with(filter, stderr, Repeats::new(REPEAT_FIRST_OWN, REPEAT_FIRST_OTHER, REPEAT_QUIET), MAX_FILE_BYTES)
    }

    /// A logger with its own repeat rule and size cap.
    pub fn with(filter: Filter, stderr: bool, repeats: Repeats, max_bytes: u64) -> AppLogger {
        AppLogger { filter, stderr, repeats: Mutex::new(repeats), sink: Mutex::new(Sink::new(max_bytes)), dir: Mutex::new(None) }
    }

    /// Rotate the logs in `dir` and send records to the fresh file; returns its path.
    /// An unusable directory is an error and changes nothing (see [`AppLogger::attach_first`]).
    pub fn attach_dir(&self, dir: &Path) -> Result<PathBuf, String> {
        match rotate(dir) {
            Ok((path, file)) => {
                self.sink.lock().unwrap_or_else(PoisonError::into_inner).attach_in(dir, file);
                *self.dir.lock().unwrap_or_else(PoisonError::into_inner) = Some(dir.to_path_buf());
                Ok(path)
            }
            Err(e) => Err(format!("{}: {e}", dir.display())),
        }
    }

    /// The first of `dirs` that can hold the log (the settings directory, then [`fallback_dir`]).
    /// The errors of the ones that couldn't come back beside the path; when none can,
    /// standard error is the only sink ([`AppLogger::stderr_only`]).
    pub fn attach_first(&self, dirs: &[PathBuf]) -> (Option<PathBuf>, Vec<String>) {
        let mut errors = Vec::new();
        for dir in dirs {
            match self.attach_dir(dir) {
                Ok(path) => return (Some(path), errors),
                Err(e) => errors.push(e),
            }
        }
        self.stderr_only();
        (None, errors)
    }

    /// The directory the log file is in, once attached.
    pub fn dir(&self) -> Option<PathBuf> {
        self.dir.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// No log file this run (no settings directory, or it can't be used): stop keeping records for one.
    pub fn stderr_only(&self) {
        self.sink.lock().unwrap_or_else(PoisonError::into_inner).close();
    }

    fn emit(&self, level: log::Level, target: &str, message: &str) {
        let thread = std::thread::current();
        let line = format_line(&timestamp(SystemTime::now()), level, thread.name().unwrap_or("?"), target, message);
        if self.stderr {
            let _ = std::io::stderr().write_all(line.as_bytes());
        }
        self.sink.lock().unwrap_or_else(PoisonError::into_inner).write(&line);
    }

    fn emit_notes(&self, notes: Vec<Note>) {
        for note in notes {
            self.emit(note.level, &note.target, &note.text);
        }
    }
}

impl log::Log for AppLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= self.filter.level_for(metadata.target())
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // Format before taking the sink lock, so that lock covers one write only.
        let message = record.args().to_string();
        let mut notes = Vec::new();
        let write =
            self.repeats.lock().unwrap_or_else(PoisonError::into_inner).admit(record.level(), record.target(), &message, Instant::now(), &mut notes);
        self.emit_notes(notes);
        if write {
            self.emit(record.level(), record.target(), &message);
        }
    }

    /// Writes the counts of repeated messages not written yet (called at exit).
    fn flush(&self) {
        let mut notes = Vec::new();
        self.repeats.lock().unwrap_or_else(PoisonError::into_inner).drain(&mut notes);
        self.emit_notes(notes);
    }
}

/// Install the logger (filter from `RUST_LOG`, else [`DEFAULT_FILTER`]; `override_filter`, when
/// set, replaces the default when `RUST_LOG` is unset — `--debug` uses it for verbose logging).
/// `None` when another logger was installed first. Call [`AppLogger::attach_dir`] once the
/// settings directory is known.
pub fn install(override_filter: Option<&str>) -> Option<&'static AppLogger> {
    static LOGGER: OnceLock<AppLogger> = OnceLock::new();
    let spec = std::env::var("RUST_LOG").ok().filter(|s| !s.trim().is_empty());
    let effective = spec.as_deref().or(override_filter).unwrap_or(DEFAULT_FILTER);
    let filter = Filter::parse(effective);
    let max = filter.max();
    let every_copy = std::env::var("CADADDY_LOG_REPEATS").is_ok_and(|v| v.trim().eq_ignore_ascii_case("all"));
    let logger =
        LOGGER.get_or_init(|| if every_copy { AppLogger::with(filter, true, Repeats::off(), MAX_FILE_BYTES) } else { AppLogger::new(filter, true) });
    log::set_logger(logger).ok()?;
    log::set_max_level(max);
    Some(logger)
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!("cadaddy-logging-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).expect("read log")
    }

    fn record(logger: &AppLogger, level: log::Level, target: &'static str, msg: &'static str) {
        logger.log(&log::Record::builder().level(level).target(target).args(format_args!("{msg}")).build());
    }

    #[test]
    fn the_default_filter_shows_cadaddy_info_and_other_crates_warnings() {
        let f = Filter::parse(DEFAULT_FILTER);
        assert_eq!(f.level_for("cadaddy"), LevelFilter::Info);
        assert_eq!(f.level_for("cadaddy::control_server"), LevelFilter::Info);
        assert_eq!(f.level_for("cadaddy_ui_egui::gpu"), LevelFilter::Info);
        assert_eq!(f.level_for("cadaddy_engine"), LevelFilter::Info);
        assert_eq!(f.level_for("wgpu_core::device"), LevelFilter::Warn);
        assert_eq!(f.level_for("naga"), LevelFilter::Warn);
        assert_eq!(f.max(), LevelFilter::Info);
    }

    #[test]
    fn directives_follow_env_logger_and_the_most_specific_one_wins() {
        let f = Filter::parse("info,wgpu_core=error,cadaddy_io=trace,cadaddy_io::dxf_ext=off");
        assert_eq!(f.level_for("eframe"), LevelFilter::Info);
        assert_eq!(f.level_for("wgpu_core::instance"), LevelFilter::Error);
        assert_eq!(f.level_for("cadaddy_io::svg"), LevelFilter::Trace);
        assert_eq!(f.level_for("cadaddy_io::dxf_ext"), LevelFilter::Off);
        assert_eq!(f.max(), LevelFilter::Trace);
        // A module name is matched at `::` boundaries, not as a bare prefix.
        assert_eq!(f.level_for("wgpu_core_extra"), LevelFilter::Info);
        // A trailing `*` is a prefix.
        assert_eq!(Filter::parse("warn,cadaddy_ui*=debug").level_for("cadaddy_ui_egui::gpu"), LevelFilter::Debug);
        assert_eq!(Filter::parse("warn,cadaddy_ui*=debug").level_for("cadaddy_engine"), LevelFilter::Warn);
        // A bare target name sets that target to the most verbose level, as env_logger does.
        assert_eq!(Filter::parse("naga").level_for("naga::front"), LevelFilter::Trace);
        assert_eq!(Filter::parse("naga").level_for("eframe"), LevelFilter::Error);
        // Levels are case-insensitive; the last bare level wins.
        assert_eq!(Filter::parse("debug,WARN").level_for("x"), LevelFilter::Warn);
    }

    #[test]
    fn hostile_specs_never_panic_and_fall_back_sensibly() {
        let long = "a".repeat(100_000);
        let specs = [
            "",
            ",,,",
            "=",
            "==",
            "=debug",
            "cadaddy=",
            "nonsense=loud",
            "🦀=info",
            "*",
            "*=debug",
            "a=b=c",
            "warn,,cadaddy=DEBUG",
            " \t ",
            "\u{0}",
            "é*=trace",
            &long,
        ];
        for spec in specs {
            let f = Filter::parse(spec);
            for target in ["cadaddy", "", "🦀", "é", "a::b"] {
                let _ = f.level_for(target);
            }
            let _ = f.max();
        }
        assert_eq!(Filter::parse("warn,,cadaddy=DEBUG").level_for("cadaddy"), LevelFilter::Debug);
        assert_eq!(Filter::parse("nonsense=loud").level_for("nonsense"), LevelFilter::Error);
        assert_eq!(Filter::parse("=debug").max(), LevelFilter::Error, "a directive without a name is skipped");
        assert_eq!(Filter::parse("*=debug").level_for("anything"), LevelFilter::Debug, "a lone `*` matches every target");
        assert_eq!(Filter::parse("é*=trace").level_for("éa"), LevelFilter::Trace);
    }

    #[test]
    fn timestamps_are_utc_with_milliseconds() {
        assert_eq!(timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_millis(1_791_446_357_728)), "2026-10-08T07:59:17.728Z");
        // Leap day, and the day after it.
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_709_164_800)), "2024-02-29T00:00:00.000Z");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_709_251_199)), "2024-02-29T23:59:59.000Z");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(1_709_251_200)), "2024-03-01T00:00:00.000Z");
        // 2000 is a leap year (divisible by 400), 2100 is not.
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29T00:00:00.000Z");
        assert_eq!(timestamp(UNIX_EPOCH + Duration::from_secs(4_107_542_400)), "2100-03-01T00:00:00.000Z");
        // Before the epoch (a clock set wrong) clamps instead of panicking.
        assert_eq!(timestamp(UNIX_EPOCH - Duration::from_secs(5)), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn rotation_keeps_the_previous_logs_and_starts_an_empty_file() {
        let dir = temp_dir("rotate");
        for run in 1..=4 {
            let (path, _file) = rotate(&dir).expect("rotate");
            assert_eq!(path, dir.join(LOG_FILE));
            assert_eq!(read(&path), "");
            std::fs::write(&path, format!("run {run}")).expect("write");
        }
        assert_eq!(read(&dir.join(LOG_FILE)), "run 4");
        assert_eq!(read(&dir.join("cadaddy.1.log")), "run 3");
        assert_eq!(read(&dir.join("cadaddy.2.log")), "run 2");
        assert!(!dir.join("cadaddy.3.log").exists(), "only {KEEP} old logs are kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_into_an_unusable_directory_is_an_error_not_a_panic() {
        let dir = temp_dir("blocked");
        std::fs::create_dir_all(dir.parent().expect("parent")).expect("tmp");
        std::fs::write(&dir, "a file where the directory should be").expect("block");
        assert!(rotate(&dir).is_err());
        assert!(rotate(&dir.join("below")).is_err());
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn a_line_carries_time_level_thread_target_and_message() {
        assert_eq!(
            format_line("2026-10-08T07:59:17.728Z", log::Level::Warn, "main", "cadaddy", "app icon: invalid PNG signature"),
            "2026-10-08T07:59:17.728Z WARN  [main] cadaddy: app icon: invalid PNG signature\n"
        );
        // A multi-line message keeps its lines; the record still ends in exactly one newline.
        assert!(format_line("t", log::Level::Error, "w", "x", "a\nb\n").ends_with("x: a\nb\n"));
    }

    #[test]
    fn lines_logged_before_the_file_exists_are_written_when_it_is_attached() {
        let dir = temp_dir("pending");
        let (path, file) = rotate(&dir).expect("rotate");
        let mut sink = Sink::new(MAX_FILE_BYTES);
        sink.write("early 1\n");
        sink.write("early 2\n");
        sink.attach(file);
        sink.write("late\n");
        assert_eq!(read(&path), "early 1\nearly 2\nlate\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_pending_buffer_is_bounded_and_says_how_much_it_dropped() {
        let dir = temp_dir("bounded");
        let (path, file) = rotate(&dir).expect("rotate");
        let mut sink = Sink::new(MAX_FILE_BYTES);
        for i in 0..MAX_PENDING + 5 {
            sink.write(&format!("line {i}\n"));
        }
        sink.attach(file);
        let text = read(&path);
        assert_eq!(text.lines().filter(|l| l.starts_with("line ")).count(), MAX_PENDING);
        assert_eq!(text.matches("5 earlier log lines were dropped").count(), 1, "{text}");
        assert!(text.contains("line 0\n") && !text.contains(&format!("line {MAX_PENDING}\n")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exactly_max_pending_lines_drop_nothing() {
        let dir = temp_dir("exact");
        let (path, file) = rotate(&dir).expect("rotate");
        let mut sink = Sink::new(MAX_FILE_BYTES);
        for i in 0..MAX_PENDING {
            sink.write(&format!("line {i}\n"));
        }
        sink.attach(file);
        let text = read(&path);
        assert_eq!(text.lines().count(), MAX_PENDING);
        assert!(!text.contains("dropped"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_full_file_is_set_aside_and_the_newest_records_go_on_in_a_fresh_one() {
        let dir = temp_dir("cap");
        let (path, file) = rotate(&dir).expect("rotate");
        let mut sink = Sink::new(100);
        sink.attach_in(&dir, file);
        for i in 0..50 {
            sink.write(&format!("line {i:02} with some padding\n"));
        }
        let text = read(&path);
        assert!(text.ends_with("line 49 with some padding\n"), "the newest record is kept: {text}");
        assert!(text.len() <= 100 + 120, "the fresh file stays near its cap: {}", text.len());
        let earlier = read(&dir.join(EARLIER_FILE));
        // The part before the newest one (itself a continued file after the first overflow).
        assert!(earlier.contains("line ") && earlier.len() <= 100 + 120, "{earlier}");
        assert!(text.lines().next().is_some_and(|l| l.contains(EARLIER_FILE)), "the fresh file says where the earlier records are: {text}");
        // Bounded on disk: the current file and one earlier part, however long the run.
        let files: Vec<_> = std::fs::read_dir(&dir).expect("dir").flatten().map(|e| e.file_name()).collect();
        assert_eq!(files.len(), 2, "{files:?}");
        // The next start keeps the newest part of this run as `.1` and drops its earlier part.
        let (_, _file) = rotate(&dir).expect("rotate again");
        assert_eq!(read(&dir.join("cadaddy.1.log")), text);
        assert!(!dir.join(EARLIER_FILE).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_full_file_with_no_known_directory_starts_over_keeping_the_newest_records() {
        let dir = temp_dir("cap-nodir");
        let (path, file) = rotate(&dir).expect("rotate");
        let mut sink = Sink::new(100);
        sink.attach(file);
        for i in 0..50 {
            sink.write(&format!("line {i:02} with some padding\n"));
        }
        let text = read(&path);
        assert!(text.ends_with("line 49 with some padding\n"), "{text}");
        assert!(text.contains("earlier records of this run were dropped"), "{text}");
        assert!(text.len() <= 100 + 120, "{}", text.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The test finding: one library warning repeated per inline image filled the 16 MB log in two
    /// minutes and nothing after it was recorded.
    #[test]
    fn one_repeated_library_warning_cannot_crowd_out_the_rest_of_the_log() {
        let dir = temp_dir("flood");
        let logger = AppLogger::new(Filter::parse(DEFAULT_FILTER), false);
        let path = logger.attach_dir(&dir).expect("attach");
        record(&logger, log::Level::Info, "cadaddy", "job started");
        for i in 0..200_000u32 {
            let args = format_args!("Skipping an unreadable part at byte {i}: a long explanation of what went wrong");
            logger.log(&log::Record::builder().level(log::Level::Warn).target("some_pdf_library::parser").args(args).build());
            if i == 100_000 {
                record(&logger, log::Level::Warn, "cadaddy_engine::jobs", "a real warning in the middle");
            }
        }
        record(&logger, log::Level::Info, "cadaddy", "job finished");
        logger.flush();
        let text = read(&path);
        let library = text.lines().filter(|l| l.contains("some_pdf_library::parser: Skipping")).count();
        assert!(library <= 10, "{library} copies of one warning were written");
        assert!(text.contains("a real warning in the middle") && text.contains("job finished"), "later records are kept");
        assert!(text.len() < 64 * 1024, "the log stays small: {} bytes", text.len());
        assert!(text.contains("[repeated]") && text.contains("200000 in all"), "the count is reported: {text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeats_write_the_first_copies_then_count_and_start_over_after_a_quiet_time() {
        let t0 = Instant::now();
        let mut r = Repeats::new(3, 2, Duration::from_secs(30));
        let mut notes = Vec::new();
        let other: Vec<bool> = (0..4).map(|i| r.admit(log::Level::Warn, "lib", &format!("bad object {i}"), t0, &mut notes)).collect();
        assert_eq!(other, [true, true, false, false], "two copies from another crate, numbers aside");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes.first().is_some_and(|n| n.text.contains("keeps coming") && n.target == "lib" && n.level == log::Level::Warn), "{notes:?}");
        let own: Vec<bool> = (0..4).map(|_| r.admit(log::Level::Warn, "cadaddy_io", "same", t0, &mut notes)).collect();
        assert_eq!(own, [true, true, true, false], "three from CADaddy's own crates");
        // A different level or target is a different message.
        assert!(r.admit(log::Level::Error, "lib", "bad object 9", t0, &mut notes));
        assert!(r.admit(log::Level::Warn, "lib2", "bad object 9", t0, &mut notes));
        // A count line at 100 copies.
        notes.clear();
        for i in 4..100 {
            assert!(!r.admit(log::Level::Warn, "lib", &format!("bad object {i}"), t0, &mut notes));
        }
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes.first().is_some_and(|n| n.text.contains("98 more copies not written (100 in all)")), "{notes:?}");
        // Quiet for longer than the quiet time: the final count, then the message starts over.
        notes.clear();
        assert!(!r.admit(log::Level::Warn, "lib", "bad object 100", t0 + Duration::from_secs(1), &mut notes));
        let later = t0 + Duration::from_secs(60);
        assert!(r.admit(log::Level::Warn, "lib", "bad object 101", later, &mut notes), "the next file's first copies are written");
        assert!(notes.iter().any(|n| n.text.contains("1 more copies not written (101 in all)")), "{notes:?}");
        // The rest at flush.
        notes.clear();
        let third: Vec<bool> = (0..3).map(|_| r.admit(log::Level::Warn, "lib3", "same", later, &mut notes)).collect();
        assert_eq!(third, [true, true, false]);
        r.drain(&mut notes);
        assert!(notes.iter().any(|n| n.target == "lib3" && n.text.contains("1 more copies not written (3 in all)")), "{notes:?}");
        notes.clear();
        r.drain(&mut notes);
        assert!(notes.is_empty(), "a count is written once: {notes:?}");
    }

    #[test]
    fn repeats_follow_a_bounded_number_of_messages() {
        let t0 = Instant::now();
        let mut r = Repeats::new(1, 1, Duration::from_secs(30));
        let mut notes = Vec::new();
        for i in 0..MAX_REPEAT_KEYS * 2 {
            // Letters, not digits, so every message is a different one.
            let msg: String = format!("{i:x}")
                .chars()
                .map(|c| char::from(b'a'.saturating_add(c.to_digit(16).and_then(|d| u8::try_from(d).ok()).unwrap_or(0))))
                .collect();
            r.admit(log::Level::Warn, "lib", &msg, t0, &mut notes);
            r.admit(log::Level::Warn, "lib", &msg, t0, &mut notes);
            assert!(r.seen.len() <= MAX_REPEAT_KEYS);
        }
        // Hostile messages: huge, multi-byte, empty.
        let long = "é".repeat(100_000);
        for msg in [long.as_str(), "", "🦀🦀", "0123456789"] {
            r.admit(log::Level::Warn, "lib", msg, t0, &mut notes);
        }
        assert!(notes.iter().all(|n| n.text.chars().count() < REPEAT_KEY_CHARS + 120));
    }

    #[test]
    fn every_copy_is_written_when_repeats_are_off() {
        let mut r = Repeats::off();
        let mut notes = Vec::new();
        assert!((0..1000).all(|_| r.admit(log::Level::Warn, "lib", "same", Instant::now(), &mut notes)));
        assert!(notes.is_empty());
    }

    #[test]
    fn a_message_shape_sets_numbers_aside() {
        assert_eq!(shape("byte 12 of 3456"), "byte # of #");
        assert_eq!(shape("x1y22z"), "x#y#z");
        assert_eq!(shape(""), "");
        assert_eq!(shape(&"a".repeat(1000)).len(), REPEAT_KEY_CHARS);
        assert!([100, 1000, 10_000, 1_000_000].iter().all(|&n| is_milestone(n)));
        assert!([0, 1, 10, 99, 101, 200, 999, 1001].iter().all(|&n| !is_milestone(n)));
    }

    #[test]
    fn the_logger_filters_records_and_writes_them_to_the_attached_file() {
        let dir = temp_dir("logger");
        let logger = AppLogger::new(Filter::parse(DEFAULT_FILTER), false);
        record(&logger, log::Level::Info, "cadaddy", "CADaddy starting");
        record(&logger, log::Level::Info, "wgpu_core::device", "too chatty");
        let path = logger.attach_dir(&dir).expect("attach");
        assert_eq!(path, dir.join(LOG_FILE));
        record(&logger, log::Level::Warn, "wgpu_hal::vulkan", "a real warning");
        record(&logger, log::Level::Debug, "cadaddy_engine", "a command-line echo");
        let text = read(&path);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines.first().is_some_and(|l| l.contains(" INFO  [") && l.ends_with("cadaddy: CADaddy starting")), "{text}");
        assert!(lines.get(1).is_some_and(|l| l.ends_with("wgpu_hal::vulkan: a real warning")), "{text}");
        assert!(logger.enabled(&log::Metadata::builder().level(log::Level::Info).target("cadaddy_ui_egui::gpu").build()));
        assert!(!logger.enabled(&log::Metadata::builder().level(log::Level::Info).target("naga").build()));
        // A second launch rotates: the first log becomes `.1`.
        let second = AppLogger::new(Filter::parse(DEFAULT_FILTER), false);
        assert_eq!(second.attach_dir(&dir).expect("attach again"), dir.join(LOG_FILE));
        assert_eq!(read(&dir.join("cadaddy.1.log")), text);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn records_from_many_threads_are_whole_lines() {
        let dir = temp_dir("threads");
        // Every copy written: the same line 400 times is the point here.
        let logger = AppLogger::with(Filter::parse("info"), false, Repeats::off(), MAX_FILE_BYTES);
        let path = logger.attach_dir(&dir).expect("attach");
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..100 {
                        record(&logger, log::Level::Info, "cadaddy", "from a worker");
                    }
                });
            }
        });
        let text = read(&path);
        assert_eq!(text.lines().count(), 400);
        assert!(text.lines().all(|l| l.ends_with("cadaddy: from a worker")), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn attaching_to_an_unusable_directory_reports_an_error_and_keeps_logging() {
        let dir = temp_dir("unusable");
        std::fs::write(&dir, "not a directory").expect("block");
        let logger = AppLogger::new(Filter::parse("info"), false);
        let e = logger.attach_dir(&dir).expect_err("a file is not a log directory");
        assert!(e.contains(&dir.display().to_string()), "{e}");
        let (path, errors) = logger.attach_first(std::slice::from_ref(&dir));
        assert!(path.is_none() && errors.len() == 1, "{errors:?}");
        record(&logger, log::Level::Error, "x", "still fine");
        let sink = logger.sink.lock().expect("sink");
        assert!(sink.file.is_none() && sink.pending.is_empty(), "nothing is kept for a file that will not come");
        drop(sink);
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn an_unusable_settings_directory_falls_back_to_the_next_one_with_the_early_records() {
        let bad = temp_dir("bad-settings");
        std::fs::write(&bad, "not a directory").expect("block");
        let good = temp_dir("fallback");
        let logger = AppLogger::new(Filter::parse("info"), false);
        record(&logger, log::Level::Info, "cadaddy", "before the file");
        let (path, errors) = logger.attach_first(&[bad.clone(), good.clone()]);
        assert_eq!(path, Some(good.join(LOG_FILE)));
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(logger.dir(), Some(good.clone()));
        record(&logger, log::Level::Info, "cadaddy", "after");
        let text = read(&good.join(LOG_FILE));
        assert!(text.contains("before the file") && text.contains("after"), "{text}");
        assert!(fallback_dir().starts_with(std::env::temp_dir()));
        let _ = std::fs::remove_file(&bad);
        let _ = std::fs::remove_dir_all(&good);
    }

    #[test]
    fn stderr_only_stops_keeping_records_for_a_file() {
        let logger = AppLogger::new(Filter::parse("info"), false);
        record(&logger, log::Level::Info, "cadaddy", "before");
        logger.stderr_only();
        record(&logger, log::Level::Info, "cadaddy", "after");
        let sink = logger.sink.lock().expect("sink");
        assert!(sink.file.is_none() && sink.pending.is_empty(), "no memory is held for a file that will not come");
    }

    /// Measure the log a real PDF import writes (not part of the normal run: it needs a PDF and
    /// installs the process-wide logger). `CADADDY_LOG_PROBE_PDF=<file> [CADADDY_LOG_PROBE_PAGES=n]
    /// cargo test -p cadaddy log_probe -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement: needs CADADDY_LOG_PROBE_PDF"]
    fn log_probe_real_pdf() {
        let Some(pdf) = std::env::var_os("CADADDY_LOG_PROBE_PDF") else { return };
        let pages: u32 = std::env::var("CADADDY_LOG_PROBE_PAGES").ok().and_then(|s| s.parse().ok()).unwrap_or(u32::MAX);
        let dir = temp_dir("probe");
        let logger: &'static AppLogger = Box::leak(Box::new(AppLogger::new(Filter::parse(DEFAULT_FILTER), false)));
        log::set_logger(logger).expect("first logger");
        log::set_max_level(Filter::parse(DEFAULT_FILTER).max());
        let path = logger.attach_dir(&dir).expect("attach");
        let bytes = std::fs::read(&pdf).expect("read pdf");
        let total = cadaddy_io::pdf_read::page_count(&bytes).expect("pages").min(pages);
        let start = std::time::Instant::now();
        for page in 1..=total {
            let opts = cadaddy_io::pdf_read::PdfOptions { page, ..Default::default() };
            let _ = cadaddy_io::pdf_read::read(&bytes, &opts);
            log::info!(target: "cadaddy", "probe: page {page} read");
        }
        log::info!(target: "cadaddy", "probe: finished {total} pages");
        log::logger().flush();
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("dir").flatten() {
            let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
            let other = text.lines().filter(|l| !l.contains("] cadaddy")).count();
            let pages_logged = text.lines().filter(|l| l.contains("probe: page ")).count();
            files.push(format!(
                "{}: {} bytes, {} lines, {} from other crates, {} page lines, finished={}",
                entry.file_name().to_string_lossy(),
                text.len(),
                text.lines().count(),
                other,
                pages_logged,
                text.contains("probe: finished")
            ));
        }
        files.sort();
        println!("PROBE {total} pages in {:.1}s; current log {}", start.elapsed().as_secs_f64(), path.display());
        for f in files {
            println!("PROBE {f}");
        }
    }

    #[test]
    fn a_poisoned_lock_does_not_stop_logging() {
        let dir = temp_dir("poison");
        let logger = AppLogger::new(Filter::parse("info"), false);
        let path = logger.attach_dir(&dir).expect("attach");
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = logger.sink.lock();
            panic!("poison the sink");
        }));
        assert!(logger.sink.is_poisoned());
        record(&logger, log::Level::Info, "cadaddy", "after the panic");
        assert!(read(&path).contains("after the panic"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
