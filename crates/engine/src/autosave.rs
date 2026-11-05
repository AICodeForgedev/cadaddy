//! Autosave (SAVETIME, SAVEFILEPATH) and the files Drawing Recovery offers after a crash.
//!
//! Every SAVETIME minutes after a change, each drawing with unsaved changes made since its last
//! autosave is written to `<SAVEFILEPATH>/<drawing>_<instance>_<uid>.sv$`: a CADaddy drawing (.cdy
//! content, which keeps everything the drawing holds), else DWG, else DXF when those writers fail
//! (readers tell them apart by their first bytes, so Drawing Recovery opens any). The file goes
//! away when the drawing is saved or closed and when the app exits normally, so a file left behind
//! means the app (or the machine) went down; the next start offers it for recovery. A running app
//! keeps a lock file (`<instance>.lock`) fresh, so another copy running at the same time doesn't
//! offer its files. SAVEFILEPATH may be a folder other programs use too, so only names built from
//! an instance id ([`is_instance_id`]) are ever read as ours or deleted.
//!
//! [`Autosaver::plan`] decides what to write and delete (no I/O, so it is testable); on the desktop
//! [`Autosaver::tick`] carries it out, writing on a background thread. SAVETIME and SAVEFILEPATH
//! are profile settings: process-wide, not saved in the drawing.
//!
//! The journal: where the .cdy saver is installed (the desktop), the autosave file is a .cdy file
//! written in full at the drawing's first change, and after that each change is appended to it as
//! a .cdy save appends (only the chunks that changed, then a new index), on the background thread;
//! every SAVETIME minutes it is written in full again, which keeps it compact. A crash then loses
//! at most the command being done, not up to SAVETIME minutes; Drawing Recovery opens the file as
//! any .cdy file (the last whole index is the drawing as of the last finished append).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};

use cadaddy_doc::Drawing;
use serde::Serialize;

use crate::Session;

pub mod store;

/// The autosave file extension.
pub const EXT: &str = "sv$";

/// The writer the io hooks provide: a drawing and a file name (its extension picks the format).
pub type WriteHook = fn(&Drawing, &str) -> std::result::Result<Vec<u8>, String>;

/// An autosave file's contents: a CADaddy drawing (.cdy, lossless), else DWG, else DXF, each tried
/// when the one before fails or writes nothing. Returns the bytes and the format used ("cdy",
/// "dwg" or "dxf").
pub fn encode(write: WriteHook, doc: &Drawing) -> std::result::Result<(Vec<u8>, &'static str), String> {
    for format in ["cdy", "dwg"] {
        if let Ok(bytes) = write(doc, &format!("autosave.{format}"))
            && !bytes.is_empty()
        {
            return Ok((bytes, format));
        }
    }
    write(doc, "autosave.dxf").map(|bytes| (bytes, "dxf"))
}
/// SAVETIME's default, in minutes.
pub const DEFAULT_SAVETIME: u32 = 10;
/// SAVETIME's largest value, in minutes.
pub const MAX_SAVETIME: u32 = 600;
/// A lock file not refreshed for this long belongs to an app that is no longer running.
pub const STALE_LOCK_SECS: u64 = 5 * 60;
/// How often a running app refreshes its lock file.
#[cfg(not(target_arch = "wasm32"))]
const HEARTBEAT_SECS: f64 = 60.0;

static SAVETIME: AtomicU32 = AtomicU32::new(DEFAULT_SAVETIME);
static ISAVEBAK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// ISAVEBAK: whether saving over a drawing keeps the previous file as `<name>.bak` (on by
/// default, as in AutoCAD). A profile setting like SAVETIME.
pub fn isavebak() -> bool {
    ISAVEBAK.load(Ordering::Relaxed)
}

/// Turn ISAVEBAK on or off.
pub fn set_isavebak(on: bool) {
    ISAVEBAK.store(on, Ordering::Relaxed);
}
static SAVEFILEPATH: Mutex<String> = Mutex::new(String::new());

/// SAVETIME: minutes between autosaves; 0 turns autosave off.
pub fn savetime() -> u32 {
    SAVETIME.load(Ordering::Relaxed)
}

/// Set SAVETIME (clamped to 0–600 minutes).
pub fn set_savetime(minutes: u32) {
    SAVETIME.store(minutes.min(MAX_SAVETIME), Ordering::Relaxed);
}

/// SAVEFILEPATH: the folder autosave files go to (the host sets a per-user default at start).
pub fn savefilepath() -> String {
    SAVEFILEPATH.lock().map(|p| p.clone()).unwrap_or_default()
}

pub fn set_savefilepath(path: &str) {
    if let Ok(mut p) = SAVEFILEPATH.lock() {
        *p = path.trim().to_string();
    }
}

/// The autosave file name for drawing `uid` titled `title`, written by app instance `instance`.
pub fn file_name(title: &str, instance: &str, uid: u64) -> String {
    // File systems limit a name to 255 bytes, so the stem gets what the suffix leaves, cut at a character boundary.
    let suffix = format!("_{instance}_{uid}.{EXT}");
    let budget = 255usize.saturating_sub(suffix.len());
    let mut used = 0;
    let stem: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || " .-_()".contains(c) { c } else { '-' })
        .take(80)
        .take_while(|c| {
            used += c.len_utf8();
            used <= budget
        })
        .collect();
    let stem = stem.trim().trim_matches('.');
    let stem = if stem.is_empty() { "Drawing" } else { stem };
    format!("{stem}{suffix}")
}

/// Whether `s` has the form of an app instance id (`<process id>-<start time in hex>`, see
/// `instance_id`). Only file names built from one are treated as CADaddy's own, so files of
/// other programs in the autosave folder are never offered for recovery or deleted.
pub fn is_instance_id(s: &str) -> bool {
    let Some((pid, time)) = s.split_once('-') else { return false };
    (1..=10).contains(&pid.len())
        && pid.bytes().all(|b| b.is_ascii_digit())
        && (1..=32).contains(&time.len())
        && time.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// An autosave file name's parts: the drawing's title, the instance and the drawing uid.
pub fn parse_file_name(name: &str) -> Option<(&str, &str, u64)> {
    let base = name.strip_suffix(&format!(".{EXT}"))?;
    let (rest, uid) = base.rsplit_once('_')?;
    let (title, instance) = rest.rsplit_once('_')?;
    let uid_ok = !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit());
    (!title.is_empty() && is_instance_id(instance) && uid_ok).then_some(())?;
    Some((title, instance, uid.parse().ok()?))
}

/// The instance whose lock file is named `name` (`<instance>.lock`), if it is one of ours.
pub fn parse_lock_name(name: &str) -> Option<&str> {
    name.strip_suffix(".lock").filter(|i| is_instance_id(i))
}

/// What to do with the file system.
#[derive(Clone)]
pub enum Job {
    /// Write `doc` (drawing `uid`) to `path`.
    Write {
        uid: u64,
        path: String,
        doc: Arc<Drawing>,
    },
    /// Append `doc`'s changes to the .cdy autosave at `path`, which holds `held` and is `from`
    /// bytes long (the journal).
    Append {
        uid: u64,
        path: String,
        doc: Arc<Drawing>,
        held: crate::cmd::file::CdyState,
        from: u64,
    },
    Delete {
        path: String,
    },
}

#[derive(Default)]
struct Tracked {
    /// The drawing as last autosaved (a weak pointer: compared, never kept alive).
    written: Option<Weak<Drawing>>,
    /// The drawing outside the Block Editor as last autosaved (the same as `written` when the
    /// editor is closed): saving the editor's changes into the block changes only this.
    host: Option<Weak<Drawing>>,
    /// Its autosave file, once written (or adopted).
    file: Option<String>,
    /// When a change not yet autosaved was first seen (seconds).
    since: Option<f64>,
    /// The journal: what the .cdy autosave file holds and its length, after the last write.
    held: Option<(crate::cmd::file::CdyState, u64)>,
    /// When the autosave was last written in full (seconds).
    full_at: Option<f64>,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Job::Write { uid, path, .. } => write!(f, "Write {{ uid: {uid}, path: {path:?} }}"),
            Job::Append { uid, path, from, .. } => write!(f, "Append {{ uid: {uid}, path: {path:?}, from: {from} }}"),
            Job::Delete { path } => write!(f, "Delete {{ path: {path:?} }}"),
        }
    }
}

/// Autosave state for one running app.
pub struct Autosaver {
    /// This app instance's id (`<process id>-<start time>`), part of every file name.
    pub instance: String,
    docs: HashMap<u64, Tracked>,
    /// Drawings whose autosave is being written right now.
    busy: HashSet<u64>,
    /// Why the last autosave of each drawing failed, until one succeeds or the drawing no longer
    /// needs it. Ordered, so [`Autosaver::failure`] doesn't change between frames.
    failed: std::collections::BTreeMap<u64, String>,
    /// Writes in progress; each ends with why its file wasn't written, if it wasn't.
    #[cfg(not(target_arch = "wasm32"))]
    running: Vec<(u64, std::thread::JoinHandle<std::result::Result<native::Held, String>>)>,
    #[cfg(not(target_arch = "wasm32"))]
    start: web_time::Instant,
    #[cfg(not(target_arch = "wasm32"))]
    last_beat: Option<f64>,
    /// When the store lock entry was last refreshed ([`store`]).
    store_beat: Option<f64>,
}

impl Autosaver {
    pub fn new(instance: impl Into<String>) -> Self {
        Autosaver {
            instance: instance.into(),
            docs: HashMap::new(),
            busy: HashSet::new(),
            failed: std::collections::BTreeMap::new(),
            #[cfg(not(target_arch = "wasm32"))]
            running: Vec::new(),
            #[cfg(not(target_arch = "wasm32"))]
            start: web_time::Instant::now(),
            #[cfg(not(target_arch = "wasm32"))]
            last_beat: None,
            store_beat: None,
        }
    }

    /// Take over an existing autosave file for drawing `uid` (a drawing opened from Drawing
    /// Recovery, whose current state it holds): later autosaves overwrite it, and it is deleted
    /// once the drawing is saved or closed.
    pub fn adopt(&mut self, s: &Session, uid: u64, path: &str) {
        let st = s.docs.iter().find(|d| d.uid == uid);
        let (written, host) = (st.map(|d| Arc::downgrade(&d.doc)), st.map(|d| Arc::downgrade(d.drawing())));
        self.docs.insert(uid, Tracked { written, host, file: Some(path.to_string()), since: None, held: None, full_at: None });
    }

    /// The autosave files this app has written or adopted.
    pub fn files(&self) -> Vec<String> {
        self.docs.values().filter_map(|t| t.file.clone()).collect()
    }

    /// The drawings with no up-to-date recovery copy and why, for the host to show the user.
    pub fn failures(&self) -> impl Iterator<Item = (u64, &str)> {
        self.failed.iter().map(|(uid, why)| (*uid, why.as_str()))
    }

    /// What to do at `now` (seconds, any fixed origin) with SAVETIME `minutes` and autosave folder
    /// `dir`: write drawings changed `minutes` ago or earlier and not autosaved since, delete the
    /// files of drawings that were saved (no unsaved changes left) or closed.
    pub fn plan(&mut self, s: &Session, now: f64, minutes: u32, dir: &str) -> Vec<Job> {
        self.plan_with(s, now, minutes, dir, false)
    }

    /// [`Autosaver::plan`], with `urgent` writing every drawing with changes not yet autosaved
    /// now instead of `minutes` after the change (autosave still off when `minutes` is 0).
    fn plan_with(&mut self, s: &Session, now: f64, minutes: u32, dir: &str, urgent: bool) -> Vec<Job> {
        let mut jobs = Vec::new();
        let live: HashSet<u64> = s.docs.iter().map(|d| d.uid).collect();
        let busy = &self.busy;
        self.docs.retain(|uid, t| {
            // A file being written is deleted once the write is done.
            if live.contains(uid) || busy.contains(uid) {
                return true;
            }
            if let Some(path) = t.file.take() {
                jobs.push(Job::Delete { path });
            }
            false
        });
        for st in &s.docs {
            if self.busy.contains(&st.uid) {
                continue;
            }
            let t = self.docs.entry(st.uid).or_default();
            if !st.is_dirty() {
                if let Some(path) = t.file.take() {
                    jobs.push(Job::Delete { path });
                }
                *t = Tracked::default();
                self.failed.remove(&st.uid);
                continue;
            }
            let same = |w: &Option<Weak<Drawing>>, d: &Arc<Drawing>| w.as_ref().is_some_and(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(d)));
            let changed = !same(&t.written, &st.doc) || !same(&t.host, st.drawing());
            if !changed {
                t.since = None;
                continue;
            }
            let since = *t.since.get_or_insert(now);
            // The journal (files on disk with the .cdy saver): written at once, appended to after
            // that, in full again every SAVETIME minutes.
            let journal = minutes > 0 && !dir.trim().is_empty() && !dir.contains("://") && crate::cmd::file::cdy_hooks().is_some();
            // In the Block Editor: the drawing with the editor's unsaved changes in the block. When
            // they can't be put there, nothing is written and nothing is marked as covered, so the
            // next pass tries again instead of keeping the host without them.
            let Ok(doc) = st.recovery_drawing() else {
                continue;
            };
            if journal && let Some(path) = t.file.clone() {
                let due_full = t.full_at.is_none_or(|f| now - f >= f64::from(minutes) * 60.0);
                if let (Some((held, from)), false) = (&t.held, due_full) {
                    t.written = Some(Arc::downgrade(&st.doc));
                    t.host = Some(Arc::downgrade(st.drawing()));
                    t.since = None;
                    jobs.push(Job::Append { uid: st.uid, path, doc: doc.clone(), held: held.clone(), from: *from });
                    continue;
                }
            }
            if minutes == 0 || dir.trim().is_empty() || (!journal && !urgent && now - since < f64::from(minutes) * 60.0) {
                continue;
            }
            t.held = None;
            t.full_at = Some(now);
            let path = t
                .file
                .clone()
                .unwrap_or_else(|| std::path::Path::new(dir).join(file_name(&st.title, &self.instance, st.uid)).to_string_lossy().to_string());
            t.file = Some(path.clone());
            t.written = Some(Arc::downgrade(&st.doc));
            t.host = Some(Arc::downgrade(st.drawing()));
            t.since = None;
            jobs.push(Job::Write { uid: st.uid, path, doc });
        }
        let docs = &self.docs;
        self.failed.retain(|uid, _| docs.contains_key(uid));
        jobs
    }

    /// Seconds from `now` until the next autosave is due, if one is waiting.
    pub fn next_due(&self, now: f64, minutes: u32) -> Option<f64> {
        if minutes == 0 {
            return None;
        }
        self.docs.values().filter_map(|t| t.since).map(|since| (since + f64::from(minutes) * 60.0 - now).max(0.0)).reduce(f64::min)
    }
}

/// A file Drawing Recovery offers: an autosave left behind by an app that is no longer running.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recoverable {
    pub path: String,
    /// The drawing's title when it was autosaved.
    pub drawing: String,
    /// When it was written, in seconds since 1970.
    pub modified: u64,
    pub bytes: u64,
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    use super::*;

    /// A new instance id: the process id and the start time.
    pub fn instance_id() -> String {
        let t = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        format!("{}-{t:x}", std::process::id())
    }

    fn lock_path(dir: &Path, instance: &str) -> std::path::PathBuf {
        dir.join(format!("{instance}.lock"))
    }

    /// Whether the app instance that owns a lock file modified `age` ago (`None`: no lock file)
    /// may still be running.
    pub fn instance_alive(age: Option<Duration>) -> bool {
        age.is_some_and(|a| a < Duration::from_secs(STALE_LOCK_SECS))
    }

    /// Whether a CADaddy process `pid` is running (the name must match, so a process id reused
    /// by another program doesn't count): `None` when this system can't tell.
    fn pid_running(pid: u32) -> Option<bool> {
        let ours = |name: &str| {
            let n = name.to_ascii_lowercase();
            n.contains("cadaddy") || n.contains("cadaddy")
        };
        if cfg!(any(target_os = "linux", target_os = "android")) {
            return Some(std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|n| ours(&n)));
        }
        if cfg!(windows) {
            #[allow(unused_mut)]
            let mut c = std::process::Command::new("tasklist");
            c.args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"]);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            let out = c.output().ok()?;
            let text = String::from_utf8_lossy(&out.stdout);
            // "cadaddy.exe","7152",...
            return Some(text.lines().any(|l| l.contains(&format!("\"{pid}\"")) && ours(l)));
        }
        if cfg!(unix) {
            let out = std::process::Command::new("/bin/ps").args(["-p", &pid.to_string(), "-o", "comm="]).output().ok()?;
            return Some(out.status.success() && ours(&String::from_utf8_lossy(&out.stdout)));
        }
        None
    }

    /// Whether the app that owns `instance` may still be running. Its process (the id's first
    /// part) decides when this system can tell: a crashed app's files are offered right away, and
    /// an idle one whose lock went stale keeps them. Otherwise the lock's age decides.
    fn instance_running(dir: &Path, instance: &str) -> bool {
        let pid = instance.split('-').next().and_then(|p| p.parse::<u32>().ok());
        match pid {
            Some(pid) if pid == std::process::id() => true,
            Some(pid) => pid_running(pid).unwrap_or_else(|| instance_alive(lock_age(dir, instance))),
            None => instance_alive(lock_age(dir, instance)),
        }
    }

    fn lock_age(dir: &Path, instance: &str) -> Option<Duration> {
        let m = std::fs::metadata(lock_path(dir, instance)).and_then(|m| m.modified()).ok()?;
        // A time in the future (clock changes) counts as fresh.
        Some(SystemTime::now().duration_since(m).unwrap_or_default())
    }

    /// The autosave files in `dir` left behind by apps that are no longer running (never those of
    /// `own`, this app's instance), oldest first.
    pub fn recoverable(dir: &Path, own: &str) -> Vec<Recoverable> {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut alive: HashMap<String, bool> = HashMap::new();
        let mut out = Vec::new();
        for e in entries.flatten().take(10_000) {
            let name = e.file_name().to_string_lossy().to_string();
            let Some((title, instance, _)) = parse_file_name(&name) else { continue };
            if instance == own || *alive.entry(instance.to_string()).or_insert_with(|| instance_running(dir, instance)) {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let modified = meta.modified().ok().and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
            out.push(Recoverable { path: e.path().to_string_lossy().to_string(), drawing: title.to_string(), modified, bytes: meta.len() });
        }
        out.sort_by(|a, b| a.modified.cmp(&b.modified).then_with(|| a.path.cmp(&b.path)));
        out
    }

    /// Delete a recovery file the user discarded (or that was recovered and then saved), and lock
    /// files of stopped apps that have no files left. Only an autosave file is deleted: a path
    /// whose name doesn't parse as one ([`parse_file_name`]) is left alone.
    pub fn discard(path: &str) {
        let ours = Path::new(path).file_name().and_then(|n| n.to_str()).is_some_and(|n| parse_file_name(n).is_some());
        if !ours {
            log::warn!("autosave: not deleting {path}: not an autosave file");
            return;
        }
        if let Err(e) = std::fs::remove_file(path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("autosave: can't delete {path}: {e}");
        }
        if let Some(dir) = Path::new(path).parent() {
            prune_locks(dir);
        }
    }

    /// Delete the lock files of apps that are no longer running and left no autosave files. Only
    /// names of the form `<instance id>.lock` are ours ([`parse_lock_name`]): SAVEFILEPATH may
    /// point at a folder shared with other programs, whose `Cargo.lock` or `foo.lock` stay.
    pub fn prune_locks(dir: &Path) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        let names: Vec<String> = entries.flatten().take(10_000).map(|e| e.file_name().to_string_lossy().to_string()).collect();
        // The instances that own autosave files, parsed once rather than once per lock file.
        let with_files: HashSet<&str> = names.iter().filter_map(|f| parse_file_name(f)).map(|(_, i, _)| i).collect();
        for n in &names {
            let Some(instance) = parse_lock_name(n) else { continue };
            if !with_files.contains(instance) && !instance_running(dir, instance) {
                let _ = std::fs::remove_file(dir.join(n));
            }
        }
    }

    /// What a write leaves: for the journal, what the .cdy file holds and its length.
    pub(super) type Held = Option<(crate::cmd::file::CdyState, u64)>;

    /// Create the autosave folder `dir` and any missing parents. On Unix the folders made here are
    /// owner-only (`0o700`): autosave files are unsaved drawings, and with the default mode other
    /// accounts on the machine could read them. A folder that already exists keeps its mode, since
    /// SAVEFILEPATH can be any folder the user picked.
    fn create_folder(dir: &Path) -> std::io::Result<()> {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(dir)
    }

    fn write_file(write: WriteHook, doc: &Drawing, path: &str) -> std::result::Result<Held, String> {
        // Through the .cdy saver when there is one: its state is the journal's start.
        let (bytes, held) = match crate::cmd::file::cdy_hooks().map(|h| (h.save)(doc, None)) {
            Some(Ok((crate::cmd::file::CdyWrite::Full(bytes), state))) => {
                let len = bytes.len() as u64;
                (bytes, Some((state, len)))
            }
            _ => (encode(write, doc)?.0, None),
        };
        if let Some(dir) = Path::new(path).parent() {
            create_folder(dir).map_err(|e| e.to_string())?;
        }
        // The file holds unsaved work: owner-only from its creation, whatever the umask.
        write_private(Path::new(path), &bytes).map_err(|e| e.to_string())?;
        Ok(held)
    }

    /// Owner read/write only (`None` where file modes don't exist).
    fn private_mode() -> Option<std::fs::Permissions> {
        #[cfg(unix)]
        return Some(std::os::unix::fs::PermissionsExt::from_mode(0o600));
        #[cfg(not(unix))]
        None
    }

    /// Replace the file at `path` with `bytes`, owner-only: an exclusive temporary file renamed
    /// into place, so a link planted at `path` is replaced, never written through (CWE-59).
    fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        crate::writefile::check(path)?;
        crate::atomic_write::write_atomic_with(path, private_mode(), |f| std::io::Write::write_all(f, bytes))
    }

    /// Append `doc`'s changes to the .cdy autosave at `path` (holding `held`, `from` bytes long).
    fn append_file(write: WriteHook, doc: &Drawing, path: &str, held: &crate::cmd::file::CdyState, from: u64) -> std::result::Result<Held, String> {
        let hooks = crate::cmd::file::cdy_hooks().ok_or("no .cdy saver")?;
        let (out, state) = (hooks.save)(doc, Some(held))?;
        match out {
            crate::cmd::file::CdyWrite::Append { from: at, bytes, index_at } if at == from => {
                crate::cmd::file::append_cdy(path, at, &bytes, index_at).map_err(|e| e.to_string())?;
                Ok(Some((state, at + bytes.len() as u64)))
            }
            // The saver wants it whole (or the lengths disagree): a full write.
            _ => write_file(write, doc, path),
        }
    }

    impl Autosaver {
        /// Autosave what is due (writing in the background) and delete files no longer needed.
        /// Returns how long until the next autosave is due, so the host can wake up for it.
        pub fn tick(&mut self, s: &Session) -> Option<Duration> {
            let now = self.start.elapsed().as_secs_f64();
            self.reap();
            let dir = savefilepath();
            let minutes = savetime();
            let jobs = self.plan(s, now, minutes, &dir);
            self.perform(jobs);
            self.heartbeat(&dir, now);
            // While this app has autosave files, wake for the next heartbeat too: an idle window
            // must keep its lock fresh, or another CADaddy would offer its files for recovery.
            let beat = self.docs.values().any(|t| t.file.is_some()).then_some(HEARTBEAT_SECS);
            match (self.next_due(now, minutes), beat) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
            .map(Duration::from_secs_f64)
        }

        /// Autosave every drawing with changes not yet autosaved right away, without waiting for
        /// SAVETIME (the app may not survive what just happened, e.g. a GPU error). Returns how
        /// many drawings are being written.
        pub fn save_now(&mut self, s: &Session) -> usize {
            let now = self.start.elapsed().as_secs_f64();
            let jobs = self.plan_with(s, now, savetime(), &savefilepath(), true);
            let writes = jobs.iter().filter(|j| matches!(j, Job::Write { .. })).count();
            self.perform(jobs);
            // The lock file marks the files as this running app's (Drawing Recovery skips them).
            self.last_beat = None;
            self.heartbeat(&savefilepath(), now);
            writes
        }

        /// No write is in progress (each finished write taken in by [`Autosaver::reap`]).
        pub fn idle(&mut self) -> bool {
            self.reap();
            self.running.is_empty()
        }

        /// Whether a background write hasn't finished yet. Its file can exist before it finishes
        /// (the folder is synced after the rename), and until [`Autosaver::reap`] takes it in, the
        /// drawing is left out of planning.
        pub fn writing(&self) -> bool {
            self.running.iter().any(|(_, h)| !h.is_finished())
        }

        /// Whether a write's result hasn't been taken in yet, finished or not: the host needs
        /// another [`Autosaver::tick`] to learn whether it failed.
        pub fn results_pending(&self) -> bool {
            !self.running.is_empty()
        }

        /// Take in the writes that have finished ([`Autosaver::tick`] does this first). A drawing
        /// whose write failed is no longer counted as autosaved: the next plan restarts its clock,
        /// so it is tried again SAVETIME later even if it doesn't change.
        pub fn reap(&mut self) {
            let (done, running) = std::mem::take(&mut self.running).into_iter().partition::<Vec<_>, _>(|(_, h)| h.is_finished());
            self.running = running;
            for (uid, h) in done {
                self.settle(uid, h);
            }
        }

        fn settle(&mut self, uid: u64, h: std::thread::JoinHandle<std::result::Result<Held, String>>) {
            self.busy.remove(&uid);
            match h.join().unwrap_or_else(|_| Err("the autosave writer stopped unexpectedly".to_string())) {
                Ok(held) => {
                    self.failed.remove(&uid);
                    if let Some(t) = self.docs.get_mut(&uid) {
                        t.held = held;
                    }
                }
                Err(e) => self.unwritten(uid, e),
            }
        }

        /// Write every drawing with changes not yet autosaved and wait until it is done, because
        /// the app has gone wrong and may not get another frame. Unlike [`Autosaver::save_now`]
        /// this also runs when SAVETIME is 0: unsaved work comes before that preference. Returns
        /// how many drawings were written.
        pub fn emergency_save(&mut self, s: &Session) -> usize {
            for (uid, h) in std::mem::take(&mut self.running) {
                self.settle(uid, h);
            }
            let now = self.start.elapsed().as_secs_f64();
            let dir = savefilepath();
            let jobs = self.plan_with(s, now, savetime().max(1), &dir, true);
            let uids: Vec<u64> = jobs.iter().filter_map(|j| match j {
                Job::Write { uid, .. } | Job::Append { uid, .. } => Some(*uid),
                Job::Delete { .. } => None,
            }).collect();
            self.perform(jobs);
            self.last_beat = None;
            self.heartbeat(&dir, now);
            for (uid, h) in std::mem::take(&mut self.running) {
                self.settle(uid, h);
            }
            uids.iter().filter(|uid| !self.failed.contains_key(uid)).count()
        }

        fn unwritten(&mut self, uid: u64, why: String) {
            if let Some(t) = self.docs.get_mut(&uid) {
                t.written = None;
                // The file's state is unknown now: the next write is a whole one.
                t.held = None;
                t.full_at = None;
            }
            self.failed.insert(uid, why);
        }

        /// Carry out jobs from [`Autosaver::plan`]: writes on a background thread, deletes now.
        pub fn perform(&mut self, jobs: Vec<Job>) {
            for job in jobs {
                match job {
                    Job::Delete { path } => discard(&path),
                    Job::Write { uid, .. } | Job::Append { uid, .. } => {
                        let Some(write) = crate::cmd::file::io().map(|h| h.write) else {
                            log::warn!("autosave: file formats are not available in this build");
                            self.unwritten(uid, "file formats are not available in this build".to_string());
                            continue;
                        };
                        let spawned = std::thread::Builder::new().name("autosave".into()).spawn(move || {
                            let (r, path) = match job {
                                Job::Write { path, doc, .. } => (write_file(write, &doc, &path), path),
                                Job::Append { path, doc, held, from, .. } => (append_file(write, &doc, &path, &held, from), path),
                                Job::Delete { path } => (Ok(None), path),
                            };
                            match r {
                                Ok(held) => {
                                    log::debug!("autosaved {path}");
                                    Ok(held)
                                }
                                Err(e) => {
                                    log::warn!("autosave {path}: {e}");
                                    Err(format!("{path}: {e}"))
                                }
                            }
                        });
                        match spawned {
                            Ok(h) => {
                                self.busy.insert(uid);
                                self.running.push((uid, h));
                            }
                            Err(e) => {
                                log::warn!("autosave: {e}");
                                self.unwritten(uid, e.to_string());
                            }
                        }
                    }
                }
            }
        }

        /// Keep this app's lock file fresh while it has autosave files.
        fn heartbeat(&mut self, dir: &str, now: f64) {
            if dir.is_empty() || self.docs.values().all(|t| t.file.is_none()) || self.last_beat.is_some_and(|b| now - b < HEARTBEAT_SECS) {
                return;
            }
            self.last_beat = Some(now);
            let dir = Path::new(dir);
            // Replaced, never written through: another account sharing the folder could plant a
            // link at the lock's name and have it truncate a file of this user's.
            let r = create_folder(dir).and_then(|()| write_private(&lock_path(dir, &self.instance), std::process::id().to_string().as_bytes()));
            if let Err(e) = r {
                log::warn!("autosave: lock file in {}: {e}", dir.display());
            }
        }

        /// The app is closing normally: finish writes in progress, then delete this app's autosave
        /// files and its lock file.
        pub fn shutdown(&mut self) {
            for (_, h) in self.running.drain(..) {
                let _ = h.join();
            }
            self.busy.clear();
            for path in self.files() {
                discard(&path);
            }
            self.docs.clear();
            let dir = savefilepath();
            if !dir.is_empty() && is_instance_id(&self.instance) {
                let _ = std::fs::remove_file(lock_path(Path::new(&dir), &self.instance));
            }
        }
    }

    #[cfg(all(test, unix))]
    mod link_tests {
        use super::*;

        /// A symlink planted at the lock path is replaced, never written through, and autosave
        /// files are owner-only whatever the umask.
        #[test]
        fn lock_writes_do_not_follow_a_planted_link_and_files_are_private() {
            use std::os::unix::fs::PermissionsExt;
            let dir = std::env::temp_dir().join(format!("cadaddy-lock-link-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let victim = dir.join("victim.txt");
            std::fs::write(&victim, b"precious").unwrap();
            let link = dir.join("1-a.lock");
            std::os::unix::fs::symlink(&victim, &link).unwrap();
            write_private(&link, b"42").unwrap();
            assert_eq!(std::fs::read(&victim).unwrap(), b"precious");
            assert!(!std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
            assert_eq!(std::fs::read(&link).unwrap(), b"42");
            assert_eq!(std::fs::metadata(&link).unwrap().permissions().mode() & 0o777, 0o600);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_fits_255_bytes_for_multibyte_titles() {
        let instance = "12345-abcdef0123456789";
        for title in ["图".repeat(100), "😀".repeat(100), "aé图😀".repeat(40), "a".repeat(300)] {
            let name = file_name(&title, instance, u64::MAX);
            assert!(name.len() <= 255, "{} bytes", name.len());
            let (t, i, u) = parse_file_name(&name).expect("round trips");
            assert_eq!((i, u), (instance, u64::MAX));
            assert!(!t.is_empty());
        }
    }
}
