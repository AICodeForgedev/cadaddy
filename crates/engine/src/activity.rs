//! Activity history: a private, local log per drawing of what changed — each undo step's command,
//! when, and how many objects it added, changed and erased (no user names). Kept in memory with
//! the drawing, saved beside its file as `<file>.activity.json` when the drawing is saved, and read
//! back when it is opened. The objects a step changed can be shown while they still exist.
//! ACTIVITYLOG 0 (Options) turns it off: nothing is recorded, read or written.

use std::sync::atomic::{AtomicBool, Ordering};

use cadaddy_doc::{Drawing, Handle};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Session;

/// Entries kept per drawing (the oldest go first).
pub const MAX_ENTRIES: usize = 10_000;
/// Changed objects remembered per entry (to show them).
pub const MAX_HANDLES: usize = 1000;
/// The largest sidecar read.
const MAX_SIDECAR_BYTES: u64 = 64 << 20;

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

/// The sidecar file of the drawing saved at `path`.
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
        let file = sidecar(&path);
        if std::fs::metadata(&file).is_ok_and(|m| m.is_file() && m.len() <= MAX_SIDECAR_BYTES)
            && let Ok(text) = std::fs::read_to_string(&file)
            && let Some(a) = Activity::from_json(&text)
        {
            st.activity = a;
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = path;
}

/// After saving the active drawing to `path`: write its log beside it. A log that can't be
/// written is reported, never an error of the save.
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
        if let Err(e) = std::fs::write(sidecar(path), text) {
            s.echo(format!("The activity log couldn't be saved beside the drawing: {e}"));
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = path;
}
