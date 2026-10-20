//! Activity history: a private, local log per drawing of what changed — each undo step's command,
//! when, and how many objects it added, changed and erased (no user names). Kept in memory with
//! the drawing, saved in CADaddy's own settings folder (`activity/`, one file per drawing file,
//! named by a hash of its path) when the drawing is saved, and read back when it is opened. Never
//! beside the drawing: a folder of saved or exported files holds only those files (battle test
//! 0.7.8 #8), so the history doesn't travel with a drawing that is sent on. A log an older
//! CADaddy wrote beside a drawing (`<file>.activity.json`) is still read; it isn't written again.
//! The objects a step changed can be shown while they still exist. ACTIVITYLOG 0 (Options) turns
//! it off: nothing is recorded, read or written.

use std::sync::atomic::{AtomicBool, Ordering};

use cadaddy_doc::{Drawing, Handle};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Session;

/// Entries kept per drawing (the oldest go first).
pub const MAX_ENTRIES: usize = 10_000;
/// Changed objects remembered per entry (to show them).
pub const MAX_HANDLES: usize = 1000;
/// The largest log read.
const MAX_SIDECAR_BYTES: u64 = 64 << 20;
/// Most drawings' logs kept in the settings folder (the least recently saved go first).
pub const MAX_LOGS: usize = 2000;

/// Where the logs are kept instead of the settings folder's `activity/` (tests).
static STORE: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// Keep the logs in `dir` instead of the settings folder (`None`: back to it). For tests.
pub fn set_store_dir(dir: Option<std::path::PathBuf>) {
    if let Ok(mut s) = STORE.lock() {
        *s = dir;
    }
}

/// The folder the logs are kept in.
#[cfg(not(target_arch = "wasm32"))]
fn store_dir() -> Option<std::path::PathBuf> {
    STORE.lock().ok().and_then(|s| s.clone()).or_else(|| crate::control_auth::config_dir().map(|d| d.join("activity")))
}

/// The log file of the drawing saved at `path`: named by a hash of the full path (the path itself
/// isn't a file name), in [`store_dir`].
#[cfg(not(target_arch = "wasm32"))]
pub fn log_file(path: &str) -> Option<std::path::PathBuf> {
    let full = std::path::absolute(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
    let mut key = full.to_string_lossy().into_owned();
    // Names that differ only in case are one file where the file system ignores case.
    if cfg!(any(windows, target_os = "macos")) {
        key = key.to_lowercase();
    }
    let h = key.bytes().fold(0xCBF2_9CE4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3));
    Some(store_dir()?.join(format!("{h:016x}.json")))
}

/// Drop the least recently saved logs past [`MAX_LOGS`].
#[cfg(not(target_arch = "wasm32"))]
fn prune(dir: &std::path::Path) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut logs: Vec<(std::time::SystemTime, std::path::PathBuf)> = read
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .take(MAX_LOGS * 4)
        .collect();
    if logs.len() <= MAX_LOGS {
        return;
    }
    logs.sort();
    for (_, p) in logs.iter().take(logs.len() - MAX_LOGS) {
        let _ = std::fs::remove_file(p);
    }
}

static ENABLED: AtomicBool = AtomicBool::new(true);

/// ACTIVITYLOG: whether activity is recorded (process-wide, like the other Options settings).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Entry {
    pub seq: u64,
    /// Seconds since 1970 (0 where the clock can't be read).
    pub time: u64,
    pub command: String,
    pub added: usize,
    pub modified: usize,
    pub erased: usize,
    /// Block definitions added, changed or removed.
    pub blocks: usize,
    /// Objects the step added or changed (at most [`MAX_HANDLES`]), as hex handles.
    pub handles: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Activity {
    pub entries: Vec<Entry>,
    next: u64,
}

fn now() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// What changed from `before` to `after`: (added, modified, erased, block definitions, changed
/// handles now in the drawing).
pub fn diff(before: &Drawing, after: &Drawing) -> (usize, usize, usize, usize, Vec<Handle>) {
    let (mut a, mut m, mut e) = (0usize, 0usize, 0usize);
    let mut hs: Vec<Handle> = Vec::new();
    let mut take = |(added, modified, erased): (Vec<Handle>, Vec<Handle>, Vec<Handle>)| {
        a += added.len();
        m += modified.len();
        e += erased.len();
        hs.extend(added.into_iter().chain(modified).take(MAX_HANDLES.saturating_sub(hs.len())));
    };
    take(before.model.changes(&after.model));
    for l in &after.layouts {
        let empty = cadaddy_doc::EntityStore::new();
        let old = before.layouts.iter().find(|o| o.name == l.name).map_or(&empty, |o| &o.entities);
        take(old.changes(&l.entities));
    }
    for l in before.layouts.iter().filter(|o| !after.layouts.iter().any(|n| n.name == o.name)) {
        take(l.entities.changes(&cadaddy_doc::EntityStore::new()));
    }
    let mut blocks = after.blocks.iter().filter(|(k, b)| before.blocks.get(*k).is_none_or(|o| !std::sync::Arc::ptr_eq(o, b))).count();
    blocks += before.blocks.keys().filter(|k| !after.blocks.contains_key(*k)).count();
    (a, m, e, blocks, hs)
}

impl Activity {
    /// Log a step: the command `label` changed `before` into `after`.
    pub fn record(&mut self, label: &str, before: &Drawing, after: &Drawing) {
        if !enabled() {
            return;
        }
        let (added, modified, erased, blocks, hs) = diff(before, after);
        self.next = self.next.saturating_add(1);
        self.entries.push(Entry {
            seq: self.next,
            time: now(),
            command: label.chars().take(200).collect(),
            added,
            modified,
            erased,
            blocks,
            handles: hs.iter().map(|h| h.hex()).collect(),
        });
        if self.entries.len() > MAX_ENTRIES {
            let extra = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..extra);
        }
    }

    fn from_json(text: &str) -> Option<Activity> {
        let v: Value = serde_json::from_str(text).ok()?;
        let mut entries: Vec<Entry> = serde_json::from_value(v.get("entries")?.clone()).ok()?;
        if entries.len() > MAX_ENTRIES {
            let extra = entries.len() - MAX_ENTRIES;
            entries.drain(..extra);
        }
        for e in &mut entries {
            e.command = e.command.chars().take(200).collect();
            e.handles.truncate(MAX_HANDLES);
        }
        let next = entries.iter().map(|e| e.seq).max().unwrap_or(0);
        Some(Activity { entries, next })
    }

    fn to_json(&self) -> String {
        serde_json::to_string(&json!({ "version": 1, "entries": self.entries })).unwrap_or_default()
    }
}

/// The file an older CADaddy kept the log of the drawing at `path` in, beside it (read, never
/// written).
pub fn sidecar(path: &str) -> String {
    format!("{path}.activity.json")
}

/// After opening drawing `i`: its log from the sidecar, if there is one.
pub fn after_open(s: &mut Session, i: usize) {
    if !enabled() {
        return;
    }
    let Some(st) = s.docs.get_mut(i) else { return };
    let Some(path) = st.path.clone() else { return };
    #[cfg(not(target_arch = "wasm32"))]
    {
        // The settings folder's log, else one an older CADaddy left beside the drawing.
        let files = [log_file(&path), Some(std::path::PathBuf::from(sidecar(&path)))];
        for file in files.into_iter().flatten() {
            if std::fs::metadata(&file).is_ok_and(|m| m.is_file() && m.len() <= MAX_SIDECAR_BYTES)
                && let Ok(text) = std::fs::read_to_string(&file)
                && let Some(a) = Activity::from_json(&text)
            {
                st.activity = a;
                break;
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = path;
}

/// After saving the active drawing to `path`: write its log to the settings folder, under that
/// path's name. A log that can't be written is reported, never an error of the save.
pub fn after_save(s: &mut Session, path: &str) {
    if !enabled() {
        return;
    }
    let Ok(st) = s.state() else { return };
    if st.activity.entries.is_empty() {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let text = st.activity.to_json();
        let Some(file) = log_file(path) else { return };
        let written = file.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&file, text));
        match written {
            Ok(()) => {
                if let Some(dir) = file.parent() {
                    prune(dir);
                }
            }
            Err(e) => s.echo(format!("The activity log couldn't be saved in CADaddy's settings folder: {e}")),
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = path;
}
