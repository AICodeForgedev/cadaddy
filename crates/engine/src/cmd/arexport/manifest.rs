//! `bim.export.ar.manifest {dir}` (Q6218): writes `manifest.json` for a folder of AR exports, so
//! any static web server can serve the folder and the field app's Update button can fetch only
//! what changed. Format: `docs/ar-export.md` "Serving exports to the field app".
//!
//! Read-only apart from `manifest.json`: the exports are read and hashed, never changed. The
//! manifest goes through the same output guard as every data-writing command (not a drawing, not
//! an open drawing; an existing file that is not an earlier manifest is replaced only with
//! `force: true`).

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::super::{bad, bool_param, str_param};
use super::SCHEMA;
use crate::{Result, Session};

pub(super) const CMD: &str = "bim.export.ar.manifest";
/// The manifest's format name and version.
pub const FORMAT: &str = "cadaddy-ar-manifest";
pub const VERSION: u64 = 1;
/// The manifest's file name in the folder.
pub const FILE: &str = "manifest.json";
/// Entries looked at in the folder (and its subfolders), subfolder depth, packages listed.
const MAX_ENTRIES: usize = 10_000;
const MAX_DEPTH: usize = 2;
const MAX_PACKAGES: usize = 2_000;
/// Largest sidecar read, and largest file hashed.
const MAX_SIDECAR_BYTES: u64 = 32 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub(super) fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let dir = str_param(p, "dir").map(str::trim).filter(|x| !x.is_empty()).ok_or_else(|| bad(CMD, "`dir` is required: the folder of AR exports"))?;
    if p.get("dir").is_some_and(|v| !v.is_string()) {
        return Err(bad(CMD, "`dir` must be a folder path"));
    }
    let force = bool_param(CMD, p, "force")?.unwrap_or(false);
    let root = PathBuf::from(dir);
    if !root.is_dir() {
        return Err(bad(CMD, format!("{dir} is not a folder")));
    }
    let out = root.join(FILE);
    let out_text = out.to_string_lossy().into_owned();
    // An earlier manifest is ours to replace; any other file of that name needs force: true.
    let ours = !force && is_manifest(&out);
    let guard_p = if ours { json!({"force": true}) } else { p.clone() };
    super::super::exchange::guard_output(s, CMD, &guard_p, &out_text)?;

    let mut sidecars = Vec::new();
    let mut seen = 0usize;
    scan(&root, "", 0, &mut seen, &mut sidecars)?;
    sidecars.sort();
    let mut packages = Vec::new();
    let mut skipped = Vec::new();
    let mut total_bytes = 0u64;
    let mut total_files = 0usize;
    for rel in &sidecars {
        if packages.len() >= MAX_PACKAGES {
            skipped.push(json!({"name": rel, "why": format!("over {MAX_PACKAGES} packages")}));
            continue;
        }
        match package(&root, rel) {
            Ok(Some((pkg, bytes, files))) => {
                total_bytes = total_bytes.saturating_add(bytes);
                total_files += files;
                packages.push(pkg);
            }
            Ok(None) => {}
            Err(why) => skipped.push(json!({"name": rel, "why": why})),
        }
    }
    let manifest = json!({
        "format": FORMAT,
        "version": VERSION,
        "generator": "CADaddy bim.export.ar.manifest",
        "generated": super::super::project::timestamp(),
        "packages": packages,
    });
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| bad(CMD, e.to_string()))?;
    crate::writefile::write_atomic(&out, text.as_bytes()).map_err(|e| bad(CMD, format!("{out_text}: {e}")))?;
    s.echo(format!("AR manifest: {} packages, {} files → {out_text}", packages.len(), total_files));
    Ok(json!({
        "path": out_text,
        "packages": packages.len(),
        "files": total_files,
        "bytes": total_bytes,
        "names": packages.iter().filter_map(|p| p.get("name").cloned()).collect::<Vec<_>>(),
        "skipped": skipped,
    }))
}

/// True when `path` is a manifest this command wrote (so re-running it replaces it).
fn is_manifest(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    if !meta.is_file() || meta.len() > MAX_SIDECAR_BYTES {
        return false;
    }
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .is_some_and(|v| v.get("format").and_then(Value::as_str) == Some(FORMAT))
}

/// The `.json` files under `dir` (relative names with `/`), at most [`MAX_DEPTH`] folders down.
/// Hidden entries and symbolic links are skipped, so nothing outside the folder is served.
fn scan(dir: &Path, prefix: &str, depth: usize, seen: &mut usize, out: &mut Vec<String>) -> Result<()> {
    let rd = std::fs::read_dir(dir).map_err(|e| bad(CMD, format!("{}: {e}", dir.display())))?;
    for entry in rd {
        let Ok(entry) = entry else { continue };
        *seen += 1;
        if *seen > MAX_ENTRIES {
            return Err(bad(CMD, format!("the folder holds more than {MAX_ENTRIES} entries: point `dir` at the exports folder")));
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !plain_name(&name) {
            continue;
        }
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        if ft.is_dir() {
            if depth < MAX_DEPTH {
                scan(&entry.path(), &rel, depth + 1, seen, out)?;
            }
        } else if ft.is_file() && name.to_ascii_lowercase().ends_with(".json") && !(prefix.is_empty() && name == FILE) {
            out.push(rel);
        }
    }
    Ok(())
}

/// A file name the phone accepts: not empty, no separators, not `.`/`..`, no control characters.
fn plain_name(n: &str) -> bool {
    !n.is_empty() && n != "." && n != ".." && n.len() <= 255 && !n.chars().any(|c| c == '/' || c == '\\' || c.is_control())
}

/// One package from its sidecar `rel`: `Ok(None)` for a `.json` that is not an AR sidecar.
fn package(root: &Path, rel: &str) -> std::result::Result<Option<(Value, u64, usize)>, String> {
    let path = root.join(rel);
    let len = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    if len > MAX_SIDECAR_BYTES {
        return Ok(None);
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let Ok(side) = serde_json::from_slice::<Value>(&bytes) else { return Ok(None) };
    let is_ar =
        side.get("schema").and_then(Value::as_str) == Some(SCHEMA) || side.get("format").and_then(Value::as_str).is_some_and(|f| f == "cadaddy-ar");
    if !is_ar {
        return Ok(None);
    }
    let (folder, file) = match rel.rsplit_once('/') {
        Some((f, n)) => (format!("{f}/"), n.to_string()),
        None => (String::new(), rel.to_string()),
    };
    let stem = file.rsplit_once('.').map_or(file.as_str(), |(a, _)| a).to_string();
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut models = 0usize;
    for role in ["glb", "usdz"] {
        let Some(n) = side.get(role).and_then(Value::as_str) else { continue };
        // The sidecar names its models beside it; anything else (a path, `..`) is not served.
        if !plain_name(n) {
            return Err(format!("its {role} `{n}` is not a plain file name beside the sidecar"));
        }
        let fp = path.with_file_name(n);
        if !fp.is_file() {
            return Err(format!("its {role} {n} is missing"));
        }
        let (size, sha) = hash(&fp)?;
        total = total.saturating_add(size);
        models += 1;
        files.push(json!({"name": format!("{folder}{n}"), "role": role, "bytes": size, "sha256": sha}));
    }
    if models == 0 {
        return Err("it names no .glb or .usdz".into());
    }
    let (size, sha) = hash(&path)?;
    total = total.saturating_add(size);
    files.push(json!({"name": rel, "role": "sidecar", "bytes": size, "sha256": sha}));
    let mut pkg = Map::new();
    pkg.insert("name".into(), json!(format!("{folder}{stem}")));
    pkg.insert("label".into(), json!(label(&side, &stem)));
    if let Some(l) = level(&side) {
        pkg.insert("level".into(), json!(l));
    }
    pkg.insert("exported".into(), json!(exported(&side, &path)));
    let n = files.len();
    pkg.insert("files".into(), Value::Array(files));
    Ok(Some((Value::Object(pkg), total, n)))
}

/// What the phone shows: the sidecar's title, else its rooms' labels, else the anchor's room or
/// level, else the file stem.
fn label(side: &Value, stem: &str) -> String {
    if let Some(t) = side.get("title").and_then(Value::as_str).filter(|t| !t.trim().is_empty()) {
        return t.trim().to_string();
    }
    let rooms: Vec<&str> = side
        .pointer("/scope/rooms")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|r| r.get("label").and_then(Value::as_str)).collect())
        .unwrap_or_default();
    match rooms.as_slice() {
        [] => {}
        [r] => return (*r).to_string(),
        [r, rest @ ..] => return format!("{r} and {} more", rest.len()),
    }
    for key in ["/anchor/room", "/scope/level", "/anchor/level"] {
        if let Some(v) = side.pointer(key).and_then(Value::as_str).filter(|v| !v.trim().is_empty()) {
            return v.trim().to_string();
        }
    }
    stem.to_string()
}

fn level(side: &Value) -> Option<String> {
    ["/anchor/level", "/scope/level"]
        .iter()
        .find_map(|k| side.pointer(k).and_then(Value::as_str).filter(|v| !v.trim().is_empty()))
        .map(|v| v.trim().to_string())
}

/// The sidecar's own export time, else the sidecar file's modification time.
fn exported(side: &Value, path: &Path) -> String {
    if let Some(t) = side.get("exported").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        return t.to_string();
    }
    let secs =
        std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    super::super::project::iso8601(secs)
}

/// Size and lowercase hex SHA-256 of a file, read in pieces.
fn hash(path: &Path) -> std::result::Result<(u64, String), String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > MAX_FILE_BYTES {
        return Err(format!("{} is over {} bytes", path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()), MAX_FILE_BYTES));
    }
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        size += n as u64;
        h.update(buf.get(..n).unwrap_or_default());
    }
    let digest = h.finalize();
    Ok((size, digest.iter().map(|b| format!("{b:02x}")).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_phone_refuses_are_not_plain() {
        for n in ["", ".", "..", "a/b", "a\\b", "../x", "/abs", "x\u{0}y"] {
            assert!(!plain_name(n), "{n:?}");
        }
        assert!(plain_name("Plan-ar-12.glb"));
    }

    #[test]
    fn labels_fall_back_in_order() {
        assert_eq!(label(&json!({"title": " A "}), "s"), "A");
        assert_eq!(label(&json!({"scope": {"rooms": [{"label": "R1"}]}}), "s"), "R1");
        assert_eq!(label(&json!({"scope": {"rooms": [{"label": "R1"}, {"label": "R2"}, {"label": "R3"}]}}), "s"), "R1 and 2 more");
        assert_eq!(label(&json!({"anchor": {"level": "L9"}}), "s"), "L9");
        assert_eq!(label(&json!({}), "stem"), "stem");
    }
}
