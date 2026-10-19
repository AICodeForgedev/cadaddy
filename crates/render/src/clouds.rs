//! The point clouds drawings show, read once by the host's loader (the file formats live in
//! `io`) and kept while drawings use them.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use cadaddy_doc::pointcloud::{Cloud, CloudLoader};

/// Most clouds kept in memory (the least recently used goes).
const MAX_CLOUDS: usize = 8;
/// Most points drawn for one cloud in one view.
pub const DRAW_BUDGET: usize = 200_000;

static LOADER: OnceLock<CloudLoader> = OnceLock::new();

type Cache = Vec<(String, Option<Arc<Cloud>>)>;

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

/// Install the reader of point cloud files (once; later calls are ignored).
pub fn set_cloud_loader(f: CloudLoader) {
    let _ = LOADER.set(f);
}

/// The points of the scan file `path` (read and sorted into the octree on first use); `None`
/// when it can't be read. A file that failed isn't tried again until [`forget_cloud`].
pub fn cloud(path: &str) -> Option<Arc<Cloud>> {
    let mut c = cache().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(i) = c.iter().position(|(p, _)| p == path) {
        let hit = c.remove(i);
        let out = hit.1.clone();
        c.push(hit);
        return out;
    }
    let loaded = LOADER.get().and_then(|f| f(path));
    if c.len() >= MAX_CLOUDS {
        c.remove(0);
    }
    c.push((path.to_string(), loaded.clone()));
    loaded
}

/// Read `path` again on next use (POINTCLOUDATTACH of a changed file).
pub fn forget_cloud(path: &str) {
    cache().lock().unwrap_or_else(PoisonError::into_inner).retain(|(p, _)| p != path);
}
