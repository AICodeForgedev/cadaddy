//! A guided job's own folder in the temporary folder (Q6433): the pictures its questions show
//! (the "which device is this tag?" crops, the architect's open objects, the symbols to name,
//! the stroke-lettered sheets' tables), in `cadaddy-<job id>/<kind>/`.
//!
//! They live only as long as something can still show them:
//! - the job finished with nothing open: the folder goes ([`remove`]);
//! - the drawing holding the job closes (or the program ends): the folder goes, unless the job
//!   can still be picked up and ask again — its checkpoint is on disk, or the drawing was saved
//!   to a file with the job in it ([`after_close`]);
//! - a job cut off (the program killed, a crash) leaves its folder behind: when a job starts, the
//!   `cadaddy-job-*` folders nothing has written for [`STALE_AFTER`] go ([`sweep_stale`]), except
//!   the open drawings' own.
//!
//! A question whose picture went shows none ([`still_there`]).

// The web build has no folders: only the names and the early returns are left of it there.
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::{DocState, Session};

/// A job folder nothing has written for this long is a left-over: its job was cut off and the
/// drawing that held it is gone (a job going on writes its checkpoint after every stage).
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(3 * 24 * 3600);

/// How often a job start looks for left-overs: a worker running job after job reads the
/// temporary folder once in this while, not at every start.
const SWEEP_EVERY: Duration = Duration::from_secs(3600);

/// Every job folder (and a job's sheet-index packet in the temporary folder) is named so.
const PREFIX: &str = "cadaddy-job-";

/// The most temporary-folder entries a sweep looks at, and the most entries it reads in one job
/// folder (with its sub-folders) for their times.
const MAX_ENTRIES: usize = 100_000;
const MAX_INSIDE: usize = 20_000;

/// The job's id as a folder name part: letters, digits, `-` and `_` only, so a job id read from a
/// drawing can't name a folder outside the temporary folder.
fn safe_id(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(48).collect()
}

/// The job's own folder in `root` (the temporary folder).
fn dir_in(root: &Path, job_id: &str) -> PathBuf {
    root.join(format!("cadaddy-{}", safe_id(job_id)))
}

/// The job's own folder in the temporary folder: its pictures go in sub-folders of it.
pub(crate) fn dir(job_id: &str) -> PathBuf {
    dir_in(&std::env::temp_dir(), job_id)
}

/// Remove the job's own folder (nothing left to show its pictures). True when one was removed.
pub(crate) fn remove(job_id: &str) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        remove_in(&std::env::temp_dir(), job_id)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = job_id;
        false
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn remove_in(root: &Path, job_id: &str) -> bool {
    if safe_id(job_id).is_empty() {
        return false;
    }
    let d = dir_in(root, job_id);
    // Never through a link: only a real folder of ours goes.
    d.symlink_metadata().is_ok_and(|m| m.is_dir()) && std::fs::remove_dir_all(&d).is_ok()
}

/// A picture path a question names, when the file is still there.
pub(crate) fn still_there(path: Option<String>) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        path.filter(|p| Path::new(p).is_file())
    }
    #[cfg(target_arch = "wasm32")]
    {
        path
    }
}

/// The guided job a drawing holds, as stored (`Drawing.agent["guided_job"]`).
fn job_of(d: &cadaddy_doc::Drawing) -> Option<Value> {
    let text = d.agent.as_deref()?;
    if text.trim().is_empty() {
        return None;
    }
    let mut agent: Value = serde_json::from_str(text).ok()?;
    let job = agent.get_mut("guided_job").map(Value::take).filter(|j| !j.is_null())?;
    Some(job)
}

fn id_of(job: &Value) -> Option<&str> {
    job.get("id").and_then(Value::as_str).filter(|id| !safe_id(id).is_empty())
}

fn done(job: &Value) -> bool {
    job.get("stages").and_then(Value::as_array).is_some_and(|stages| {
        stages
            .iter()
            .all(|st| st.get("status").and_then(Value::as_str).is_some_and(|v| v.eq_ignore_ascii_case("done") || v.eq_ignore_ascii_case("skipped")))
    })
}

/// A drawing is closing (`open`: the drawings staying open): its job's folder goes, unless the
/// job can still be picked up and ask again — another open drawing holds the same job, the job's
/// checkpoint is on disk, or the drawing was saved to a file holding the unfinished job. A job's
/// sheet-index packet in the temporary folder (a job with no checkpoint) goes with it.
pub(crate) fn after_close(closing: &DocState, open: &[DocState]) {
    if cfg!(target_arch = "wasm32") {
        return;
    }
    let Some(job) = job_of(&closing.doc) else { return };
    let Some(id) = id_of(&job) else { return };
    if open.iter().any(|o| job_of(&o.doc).as_ref().and_then(id_of) == Some(id)) {
        return;
    }
    let checkpoint = job.get("checkpoint").and_then(Value::as_str).map(str::to_string);
    let finished = done(&job);
    #[cfg(not(target_arch = "wasm32"))]
    let resumable = !finished && checkpoint.as_deref().is_some_and(|c| Path::new(c).is_file());
    #[cfg(target_arch = "wasm32")]
    let resumable = false;
    let saved_with_it = closing.path.is_some() && job_of(&closing.saved).is_some_and(|j| id_of(&j) == Some(id) && !done(&j));
    if resumable || saved_with_it {
        return;
    }
    remove(id);
    if checkpoint.is_none() {
        super::sheetcheck::remove_packet_dir(&super::sheetcheck::packet_dir_for(id, None));
    }
}

/// When a job starts: remove the left-over job folders in the temporary folder (see the module
/// notes), at most once in [`SWEEP_EVERY`]; `starting` is the new job's id. The number removed.
pub(crate) fn sweep_stale(s: &Session, starting: &str) -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
        let Ok(mut last) = LAST.lock() else { return 0 };
        if last.is_some_and(|t| t.elapsed() < SWEEP_EVERY) {
            return 0;
        }
        *last = Some(std::time::Instant::now());
        drop(last);
        let root = std::env::temp_dir();
        let mut keep: Vec<String> = Vec::new();
        let ids = s.docs.iter().filter_map(|st| job_of(&st.doc)).filter_map(|j| id_of(&j).map(str::to_string));
        for id in ids.chain(std::iter::once(starting.to_string())) {
            keep.extend(dir_in(&root, &id).file_name().map(|n| n.to_string_lossy().into_owned()));
            keep.extend(super::sheetcheck::packet_dir_for(&id, None).file_name().map(|n| n.to_string_lossy().into_owned()));
        }
        sweep_in(&root, SystemTime::now(), STALE_AFTER, &keep)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, starting);
        0
    }
}

/// Remove the `cadaddy-job-*` folders in `root` that nothing has written for `max_age` before
/// `now` (their newest file or sub-folder, and the job's default checkpoint), except those named
/// in `keep`. The number removed.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn sweep_in(root: &Path, now: SystemTime, max_age: Duration, keep: &[String]) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else { return 0 };
    let mut removed = 0;
    for entry in entries.flatten().take(MAX_ENTRIES) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(PREFIX) || keep.iter().any(|k| k == name) {
            continue;
        }
        // Never through a link (the entry's own type, not its target's).
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let path = entry.path();
        let mut budget = MAX_INSIDE;
        let mut newest = newest_time(&path, 2, &mut budget);
        // A job cut off keeps its checkpoint fresh while it goes on: its folder stays with it.
        let checkpoint = name.strip_prefix("cadaddy-").map(|id| root.join("cadaddy-jobs").join(format!("{id}.cdy")));
        if let Some(t) = checkpoint.and_then(|c| c.metadata().ok()).and_then(|m| m.modified().ok()) {
            newest = newest.max(Some(t));
        }
        // A folder whose times can't be read (or are in the future) stays.
        let stale = newest.and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > max_age);
        if stale && std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The newest modification time of `path` and what is in it, `depth` folders down (links not
/// followed), reading at most `budget` entries.
#[cfg(not(target_arch = "wasm32"))]
fn newest_time(path: &Path, depth: u32, budget: &mut usize) -> Option<SystemTime> {
    let meta = path.symlink_metadata().ok()?;
    let mut newest = meta.modified().ok();
    if depth == 0 || !meta.is_dir() {
        return newest;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return newest };
    for entry in entries.flatten() {
        if *budget == 0 {
            break;
        }
        *budget -= 1;
        let t = newest_time(&entry.path(), depth - 1, budget);
        newest = newest.max(t);
    }
    newest
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::panic)]
#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cadaddy-jobtemp-test-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap_or_else(|e| panic!("{e}"));
        d
    }

    fn put(dir: &Path, rel: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap_or(dir)).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(&p, b"x").unwrap_or_else(|e| panic!("{e}"));
    }

    #[test]
    fn a_job_id_names_a_folder_in_the_temporary_folder_only() {
        let root = Path::new("/scratch");
        assert_eq!(dir_in(root, "job-0123abcd"), root.join("cadaddy-job-0123abcd"));
        assert_eq!(dir_in(root, "../../etc/job"), root.join("cadaddy-etcjob"));
        // An id with nothing usable in it removes nothing.
        assert!(!remove_in(root, "../.."));
    }

    #[test]
    fn the_sweep_removes_left_over_job_folders_only() {
        let root = scratch("sweep");
        put(&root, "cadaddy-job-00000001/device-tags/tag-ZZ-Q_1.png");
        put(&root, "cadaddy-job-00000002/symbols/tag-symbol-Q_1.png");
        put(&root, "cadaddy-job-job00000003 sheet index/sheet-index.json");
        put(&root, "cadaddy-job-00000004/device-tags/tag-A-Q_1.png");
        put(&root, "cadaddy-jobs/job-00000005.cdy");
        put(&root, "cadaddy-job-00000005/device-tags/tag-B-Q_1.png");
        put(&root, "cadaddy-other/keep.png");
        put(&root, "cadaddy-job-file.txt");
        let now = SystemTime::now();
        // Nothing is old yet: nothing goes.
        assert_eq!(sweep_in(&root, now, STALE_AFTER, &[]), 0);
        // Four days on: every job folder but the kept one and the one whose checkpoint is fresh.
        let later = now + Duration::from_secs(4 * 24 * 3600);
        std::fs::write(root.join("cadaddy-jobs/job-00000005.cdy"), b"y").unwrap_or_else(|e| panic!("{e}"));
        let fresh = std::fs::File::options().write(true).open(root.join("cadaddy-jobs/job-00000005.cdy")).unwrap_or_else(|e| panic!("{e}"));
        fresh.set_modified(later).unwrap_or_else(|e| panic!("{e}"));
        let keep = vec!["cadaddy-job-00000004".to_string()];
        assert_eq!(sweep_in(&root, later, STALE_AFTER, &keep), 3);
        assert!(!root.join("cadaddy-job-00000001").exists());
        assert!(!root.join("cadaddy-job-00000002").exists());
        assert!(!root.join("cadaddy-job-job00000003 sheet index").exists());
        assert!(root.join("cadaddy-job-00000004/device-tags/tag-A-Q_1.png").is_file(), "an open drawing's job keeps its folder");
        assert!(root.join("cadaddy-job-00000005/device-tags/tag-B-Q_1.png").is_file(), "a job still writing its checkpoint keeps its folder");
        assert!(root.join("cadaddy-other/keep.png").is_file(), "not a job folder");
        assert!(root.join("cadaddy-job-file.txt").is_file(), "not a folder");
        assert!(root.join("cadaddy-jobs/job-00000005.cdy").is_file(), "checkpoints are not swept");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_picture_that_went_is_not_offered() {
        let root = scratch("there");
        put(&root, "a.png");
        let a = root.join("a.png").to_string_lossy().into_owned();
        assert_eq!(still_there(Some(a.clone())), Some(a));
        assert_eq!(still_there(Some(root.join("gone.png").to_string_lossy().into_owned())), None);
        assert_eq!(still_there(None), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
