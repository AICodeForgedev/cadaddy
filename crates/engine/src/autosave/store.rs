//! Autosave into a key/value store instead of files: the web build's browser storage (the host
//! installs it with [`set_store`]). Drawings are kept as DXF text under the same names autosave
//! files get, each app (browser tab) refreshes a lock entry while it has autosaves, and the
//! entries of a tab that stopped (closed, crashed, reloaded) are offered back by Drawing Recovery,
//! as autosave files left behind are on the desktop. Store paths read `browser:<name>`.

use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};

use super::{Autosaver, Job, Recoverable, STALE_LOCK_SECS, parse_file_name, savetime};
use crate::Session;

/// A key/value store of strings, and the time (seconds since 1970).
pub struct Store {
    pub get: fn(&str) -> Option<String>,
    /// `false` when the value wasn't stored (the store is full).
    pub set: fn(&str, &str) -> bool,
    pub remove: fn(&str),
    pub keys: fn() -> Vec<String>,
    pub now: fn() -> f64,
}

static STORE: OnceLock<Store> = OnceLock::new();

pub fn set_store(s: Store) {
    let _ = STORE.set(s);
}

pub fn store() -> Option<&'static Store> {
    STORE.get()
}

/// The prefix of store paths.
pub const SCHEME: &str = "browser:";
/// Keys of autosaved drawings and of the apps' lock entries.
const FILES: &str = "cadaddy.autosave/";
const LOCKS: &str = "cadaddy.autosave.lock/";
/// Largest autosave kept (browser storage holds a few megabytes per site).
pub const MAX_TEXT: usize = 4_500_000;
/// How often an app with autosaves refreshes its lock entry.
const HEARTBEAT_SECS: f64 = 60.0;
/// Most entries looked at.
const MAX_KEYS: usize = 10_000;

/// Whether `path` names an autosave in the store.
pub fn is_store_path(path: &str) -> bool {
    path.starts_with(SCHEME)
}

/// The autosave name in a store path (`browser:<name>`, or `browser:/<name>` as plans build it).
fn name_of(path: &str) -> Option<&str> {
    let n = path.strip_prefix(SCHEME)?.trim_start_matches(['/', '\\']);
    parse_file_name(n).map(|_| n)
}

/// An app instance id for the store (any target): `<random digits>-<start time in hex>`.
pub fn instance_id(random: u32, now_secs: f64) -> String {
    let t = if now_secs.is_finite() && now_secs > 0.0 { now_secs as u64 } else { 0 };
    format!("{}-{t:x}", random % 1_000_000_000 + 1)
}

impl Autosaver {
    /// Autosave what is due into the store and remove entries no longer needed; returns how many
    /// drawings were written. A drawing that couldn't be stored (too large, store full) waits
    /// SAVETIME again.
    pub fn tick_store(&mut self, s: &Session) -> usize {
        let Some(st) = store() else { return 0 };
        let now = (st.now)();
        let jobs = self.plan(s, now, savetime(), SCHEME);
        let mut written = 0;
        for job in jobs {
            match job {
                Job::Delete { path } => discard_store(&path),
                Job::Write { uid, path, doc } => {
                    let title = s.docs.iter().find(|d| d.uid == uid).map(|d| d.title.clone()).unwrap_or_default();
                    if store_write(st, &path, &title, &doc, now) {
                        written += 1;
                    } else if let Some(t) = self.docs.get_mut(&uid) {
                        t.written = None;
                        t.host = None;
                        t.since = Some(now);
                    }
                }
            }
        }
        let has_files = self.docs.values().any(|t| t.file.is_some());
        if has_files && self.store_beat.is_none_or(|b| now - b >= HEARTBEAT_SECS) {
            self.store_beat = Some(now);
            (st.set)(&format!("{LOCKS}{}", self.instance), &format!("{}", now.max(0.0) as u64));
        }
        written
    }

    /// The app closes normally: its store entries and lock go.
    pub fn shutdown_store(&mut self) {
        let Some(st) = store() else { return };
        for path in self.files() {
            discard_store(&path);
        }
        self.docs.clear();
        (st.remove)(&format!("{LOCKS}{}", self.instance));
    }
}

fn store_write(st: &Store, path: &str, title: &str, doc: &Arc<cadaddy_doc::Drawing>, now: f64) -> bool {
    let Some(name) = name_of(path) else { return false };
    let Some(hooks) = crate::cmd::file::io() else { return false };
    let Ok(bytes) = (hooks.write)(doc, "autosave.dxf") else { return false };
    let Ok(text) = String::from_utf8(bytes) else { return false };
    if text.len() > MAX_TEXT {
        return false;
    }
    let v = json!({ "drawing": title, "modified": now.max(0.0) as u64, "dxf": text });
    (st.set)(&format!("{FILES}{name}"), &v.to_string())
}

/// Autosaves of apps that stopped: entries whose app's lock is missing or older than
/// [`STALE_LOCK_SECS`], other than `own` instance's.
pub fn recoverable_store(own: &str) -> Vec<Recoverable> {
    let Some(st) = store() else { return Vec::new() };
    let now = (st.now)().max(0.0) as u64;
    let mut out = Vec::new();
    for key in (st.keys)().into_iter().take(MAX_KEYS) {
        let Some(name) = key.strip_prefix(FILES) else { continue };
        let Some((title, instance, _)) = parse_file_name(name) else { continue };
        if instance == own {
            continue;
        }
        let beat = (st.get)(&format!("{LOCKS}{instance}")).and_then(|t| t.trim().parse::<u64>().ok());
        if beat.is_some_and(|b| now.saturating_sub(b) < STALE_LOCK_SECS) {
            continue;
        }
        let Some(v) = (st.get)(&key).and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        let bytes = v.get("dxf").and_then(Value::as_str).map_or(0, str::len) as u64;
        out.push(Recoverable {
            path: format!("{SCHEME}{name}"),
            drawing: v.get("drawing").and_then(Value::as_str).filter(|d| !d.is_empty()).unwrap_or(title).to_string(),
            modified: v.get("modified").and_then(Value::as_u64).unwrap_or(0),
            bytes,
        });
    }
    out.sort_by_key(|r| std::cmp::Reverse(r.modified));
    out
}

/// The DXF bytes of a store autosave.
pub fn read_store(path: &str) -> Option<Vec<u8>> {
    let st = store()?;
    let name = name_of(path)?;
    let v: Value = serde_json::from_str(&(st.get)(&format!("{FILES}{name}"))?).ok()?;
    v.get("dxf").and_then(Value::as_str).map(|t| t.as_bytes().to_vec())
}

/// Remove a store autosave (only names of CADaddy's own form).
pub fn discard_store(path: &str) {
    if let (Some(st), Some(name)) = (store(), name_of(path)) {
        (st.remove)(&format!("{FILES}{name}"));
    }
}

/// Remove lock entries of apps that stopped and have no autosaves left.
pub fn prune_store_locks() {
    let Some(st) = store() else { return };
    let now = (st.now)().max(0.0) as u64;
    let keys: Vec<String> = (st.keys)().into_iter().take(MAX_KEYS).collect();
    for k in &keys {
        let Some(instance) = k.strip_prefix(LOCKS) else { continue };
        let has_files = keys.iter().any(|f| f.strip_prefix(FILES).and_then(parse_file_name).is_some_and(|(_, i, _)| i == instance));
        let stale = (st.get)(k).and_then(|t| t.trim().parse::<u64>().ok()).is_none_or(|b| now.saturating_sub(b) >= STALE_LOCK_SECS);
        if !has_files && stale {
            (st.remove)(k);
        }
    }
}
