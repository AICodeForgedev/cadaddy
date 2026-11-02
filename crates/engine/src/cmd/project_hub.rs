//! The project hub (Q325): the list of projects on this machine, shelving into one archive and
//! restoring byte for byte, duplicate / delete / template, and the company standards library the
//! projects inherit from.
//!
//! The registry is `projects.json` and the company library `standards.json`, both in the settings
//! folder ([`Session::project_config`] first, else `CADADDY_CONFIG_DIR` / the user's settings
//! folder). Both are read leniently: a missing or damaged file is an empty list, never a panic.
//!
//! Commands: `project.list`, `project.shelve_project`, `project.restore_project` (with the
//! helpers in [`project`]), `project.duplicate`, `project.delete`, `project.company.list`,
//! `project.company.set`, `project.company.remove`. Schema and layout: `docs/project-manifest.md`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use super::project::{self, ProjectManifest, Standard};
use super::*;
use crate::{EngineError, Result, Session};

/// The archive a shelved project keeps (next to its stub `project.json`).
pub(super) const ARCHIVE: &str = "project.shelved";
const ARCHIVE_TMP: &str = "project.shelved.tmp";
/// Folders at the top of a project that hold only what can be rebuilt.
pub(super) const CACHE_DIRS: [&str; 4] = ["cache", "tiles", "temp", "indexes"];
const MAGIC: &[u8; 8] = b"CDYPRJ01";
/// Caps on what a folder or an archive may hold (input-sized allocations are bounded).
const MAX_FILES: usize = 1_000_000;
const MAX_DEPTH: usize = 64;
const MAX_ENTRY_BYTES: u64 = 8 << 30;
const MAX_REGISTRY: usize = 10_000;
const MAX_MANIFEST_BYTES: u64 = 64 << 20;

fn other(msg: impl Into<String>) -> EngineError {
    EngineError::Other(msg.into())
}

// ---------------------------------------------------------------------------
// Settings folder, registry, company library
// ---------------------------------------------------------------------------

fn config_root(s: &Session) -> Result<PathBuf> {
    s.project_config.clone().or_else(crate::control_auth::config_dir).ok_or_else(|| other("no settings folder is available for the project registry"))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegEntry {
    name: String,
    path: String,
    #[serde(default)]
    last_opened: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    projects: Vec<RegEntry>,
}

/// The same folder under one spelling, so a project is listed once however it was typed.
fn canon(path: &str) -> String {
    std::fs::canonicalize(path).ok().and_then(|p| p.to_str().map(str::to_string)).unwrap_or_else(|| path.trim_end_matches('/').to_string())
}

fn read_json_file(path: &Path) -> Option<Vec<u8>> {
    let md = std::fs::metadata(path).ok()?;
    if md.len() > MAX_MANIFEST_BYTES {
        return None;
    }
    std::fs::read(path).ok()
}

fn load_registry(s: &Session) -> Registry {
    let Ok(root) = config_root(s) else { return Registry::default() };
    let mut r: Registry = read_json_file(&root.join("projects.json")).and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
    r.projects.truncate(MAX_REGISTRY);
    r
}

fn save_registry(s: &Session, r: &Registry) -> Result<()> {
    let root = config_root(s)?;
    std::fs::create_dir_all(&root).map_err(|e| other(format!("cannot create {}: {e}", root.display())))?;
    let json = serde_json::to_vec_pretty(r).map_err(|e| other(format!("cannot serialize the project registry: {e}")))?;
    let tmp = root.join("projects.json.tmp");
    crate::writefile::write(&tmp, json).map_err(|e| other(format!("cannot write {}: {e}", tmp.display())))?;
    crate::writefile::rename(&tmp, root.join("projects.json")).map_err(|e| other(format!("cannot write the project registry: {e}")))
}

/// Note a project in the registry (best effort: a project works without a settings folder).
pub(super) fn register(s: &Session, m: &ProjectManifest, opened: bool) {
    let mut r = load_registry(s);
    let path = canon(&m.dir);
    let now = project::timestamp();
    if let Some(e) = r.projects.iter_mut().find(|e| e.path == path) {
        e.name = m.name.clone();
        if opened {
            e.last_opened = now;
        }
    } else if r.projects.len() < MAX_REGISTRY {
        r.projects.push(RegEntry { name: m.name.clone(), path, last_opened: if opened { now } else { String::new() } });
    }
    let _ = save_registry(s, &r);
}

fn unregister(s: &Session, dir: &str) {
    let mut r = load_registry(s);
    let path = canon(dir);
    let before = r.projects.len();
    r.projects.retain(|e| e.path != path);
    if r.projects.len() != before {
        let _ = save_registry(s, &r);
    }
}

fn company_path(s: &Session) -> Result<PathBuf> {
    Ok(config_root(s)?.join("standards.json"))
}

#[derive(Default, Serialize, Deserialize)]
struct Company {
    #[serde(default)]
    standards: Vec<Standard>,
}

pub(super) fn load_company(s: &Session) -> Vec<Standard> {
    let Ok(path) = company_path(s) else { return Vec::new() };
    let mut c: Company = read_json_file(&path).and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
    c.standards.truncate(project::MAX_STANDARDS);
    c.standards
}

fn save_company(s: &Session, standards: Vec<Standard>) -> Result<()> {
    let path = company_path(s)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| other(format!("cannot create {}: {e}", dir.display())))?;
    }
    let json = serde_json::to_vec_pretty(&Company { standards }).map_err(|e| other(format!("cannot serialize the company standards: {e}")))?;
    crate::writefile::write(&path, json).map_err(|e| other(format!("cannot write {}: {e}", path.display())))
}

fn same(a: &Standard, domain: &str, key: &str) -> bool {
    a.domain.eq_ignore_ascii_case(domain) && a.key.eq_ignore_ascii_case(key)
}

/// A project's standards as they apply: the company's, each replaced by the project's own value
/// where the project sets one. `source` is `company` or `project`.
pub(super) fn effective_standards(s: &Session, m: &ProjectManifest, domain: Option<&str>) -> Vec<Value> {
    let company = load_company(s);
    let mut out: Vec<Value> = Vec::new();
    for c in &company {
        if m.standards.iter().any(|p| same(p, &c.domain, &c.key)) {
            continue;
        }
        out.push(json!({"domain": c.domain, "key": c.key, "value": c.value, "source": "company"}));
    }
    for p in &m.standards {
        let mut e = json!({"domain": p.domain, "key": p.key, "value": p.value, "source": "project"});
        if let Some(c) = company.iter().find(|c| same(c, &p.domain, &p.key)) {
            e["overrides"] = json!({"source": "company", "value": c.value});
        }
        if !p.source.is_empty() && p.source != "project" {
            e["origin"] = json!(p.source);
        }
        out.push(e);
    }
    if let Some(d) = domain {
        out.retain(|e| e["domain"].as_str().is_some_and(|x| x.eq_ignore_ascii_case(d)));
    }
    out.sort_by(|a, b| (a["domain"].as_str(), a["key"].as_str()).cmp(&(b["domain"].as_str(), b["key"].as_str())));
    out
}

// ---------------------------------------------------------------------------
// Folder walking and the archive
// ---------------------------------------------------------------------------

/// Every file under `dir` as (path relative to `dir`, absolute path), except the top-level
/// cache folders and the archive files. A symbolic link is refused: it could point outside.
fn collect(dir: &Path, skip_caches: bool) -> Result<Vec<(String, PathBuf)>> {
    fn walk(base: &Path, cur: &Path, top: bool, skip_caches: bool, depth: usize, out: &mut Vec<(String, PathBuf)>) -> Result<()> {
        if depth > MAX_DEPTH {
            return Err(other(format!("{} is nested more than {MAX_DEPTH} folders deep", cur.display())));
        }
        let rd = std::fs::read_dir(cur).map_err(|e| other(format!("cannot read {}: {e}", cur.display())))?;
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            let md = std::fs::symlink_metadata(&path).map_err(|e| other(format!("cannot read {}: {e}", path.display())))?;
            if md.file_type().is_symlink() {
                return Err(other(format!("{} is a symbolic link; a project folder must not contain links", path.display())));
            }
            if top && (name == ARCHIVE || name == ARCHIVE_TMP || (skip_caches && md.is_dir() && CACHE_DIRS.contains(&name.as_str()))) {
                continue;
            }
            if md.is_dir() {
                walk(base, &path, false, skip_caches, depth + 1, out)?;
            } else if md.is_file() {
                if out.len() >= MAX_FILES {
                    return Err(other(format!("more than {MAX_FILES} files in the project")));
                }
                let rel = path.strip_prefix(base).map_err(|_| other("path outside the project"))?;
                let rel = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
                out.push((rel, path));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, true, skip_caches, 0, &mut out)?;
    Ok(out)
}

/// A relative path that stays inside its folder: no `..`, not absolute, no drive, no backslash.
pub(super) fn safe_rel(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 4096
        && !p.starts_with('/')
        && !p.contains('\\')
        && !p.contains(':')
        && !p.contains('\0')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

fn fnv(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn put_u64(w: &mut impl std::io::Write, v: u64) -> std::io::Result<()> {
    w.write_all(&v.to_le_bytes())
}

fn get_u64(r: &mut impl std::io::Read) -> Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b).map_err(|e| other(format!("project archive is cut short: {e}")))?;
    Ok(u64::from_le_bytes(b))
}

/// Read every entry of an archive, checking its size and hash, and hand it to `f`.
fn read_archive(path: &Path, mut f: impl FnMut(&str, Vec<u8>) -> Result<()>) -> Result<u64> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| other(format!("cannot open {}: {e}", path.display())))?;
    let mut r = std::io::BufReader::new(file);
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic).map_err(|e| other(format!("project archive is cut short: {e}")))?;
    if &magic != MAGIC {
        return Err(other("not a project archive (wrong header)"));
    }
    let mut n = 0u64;
    loop {
        let plen = get_u64(&mut r)?;
        if plen == 0 {
            break;
        }
        if plen > 4096 || n as usize >= MAX_FILES {
            return Err(other("project archive is damaged (entry header)"));
        }
        let mut name = vec![0u8; plen as usize];
        r.read_exact(&mut name).map_err(|e| other(format!("project archive is cut short: {e}")))?;
        let name = String::from_utf8(name).map_err(|_| other("project archive is damaged (path)"))?;
        let (raw, packed, hash) = (get_u64(&mut r)?, get_u64(&mut r)?, get_u64(&mut r)?);
        if raw > MAX_ENTRY_BYTES || packed > MAX_ENTRY_BYTES {
            return Err(other(format!("project archive entry `{name}` is too large")));
        }
        let mut buf = Vec::new();
        r.by_ref().take(packed).read_to_end(&mut buf).map_err(|e| other(format!("project archive is cut short: {e}")))?;
        if buf.len() as u64 != packed {
            return Err(other(format!("project archive is cut short in `{name}`")));
        }
        let data =
            miniz_oxide::inflate::decompress_to_vec_with_limit(&buf, raw as usize).map_err(|e| other(format!("`{name}` does not unpack: {e:?}")))?;
        if data.len() as u64 != raw || fnv(&data) != hash {
            return Err(other(format!("`{name}` does not match its recorded hash; the archive is damaged")));
        }
        if !safe_rel(&name) {
            return Err(other(format!("project archive entry `{name}` leaves the project folder")));
        }
        f(&name, data)?;
        n += 1;
    }
    Ok(n)
}

/// Pack `files` into `<dir>/project.shelved` (written aside, read back and checked first).
fn write_archive(dir: &Path, files: &[(String, PathBuf)]) -> Result<u64> {
    use std::io::Write;
    let tmp = dir.join(ARCHIVE_TMP);
    let res = (|| -> Result<()> {
        let file = crate::writefile::create(&tmp).map_err(|e| other(format!("cannot create {}: {e}", tmp.display())))?;
        let mut w = std::io::BufWriter::new(file);
        let io = |e: std::io::Error| other(format!("cannot write the project archive: {e}"));
        w.write_all(MAGIC).map_err(io)?;
        for (rel, path) in files {
            let data = std::fs::read(path).map_err(|e| other(format!("cannot read {}: {e}", path.display())))?;
            let packed = miniz_oxide::deflate::compress_to_vec(&data, 6);
            put_u64(&mut w, rel.len() as u64).map_err(io)?;
            w.write_all(rel.as_bytes()).map_err(io)?;
            put_u64(&mut w, data.len() as u64).map_err(io)?;
            put_u64(&mut w, packed.len() as u64).map_err(io)?;
            put_u64(&mut w, fnv(&data)).map_err(io)?;
            w.write_all(&packed).map_err(io)?;
        }
        put_u64(&mut w, 0).map_err(io)?;
        let file = w.into_inner().map_err(|e| other(format!("cannot write the project archive: {e}")))?;
        file.sync_all().map_err(io)?;
        let n = read_archive(&tmp, |_, _| Ok(()))?;
        if n as usize != files.len() {
            return Err(other("the project archive does not hold every file"));
        }
        Ok(())
    })();
    if let Err(e) = res {
        let _ = crate::writefile::remove(&tmp);
        return Err(e);
    }
    crate::writefile::rename(&tmp, dir.join(ARCHIVE)).map_err(|e| other(format!("cannot finish the project archive: {e}")))?;
    std::fs::metadata(dir.join(ARCHIVE)).map(|m| m.len()).map_err(|e| other(format!("cannot read the project archive: {e}")))
}

fn remove_empty_dirs(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                remove_empty_dirs(&e.path());
                let _ = std::fs::remove_dir(e.path());
            }
        }
    }
}

/// Shelve the project in `dir`: drop the caches, pack everything else into one archive, leave a
/// stub `project.json`. Returns (bytes before, bytes after, files packed).
pub(super) fn shelve_folder(dir: &str) -> Result<(u64, u64, u64)> {
    let d = Path::new(dir);
    let mut m = project::read_manifest(dir)?;
    if m.shelved {
        return Err(other(format!("project `{}` is already shelved; project.restore_project {{path}} brings it back", m.name)));
    }
    let before = project::dir_size(dir);
    let files = collect(d, true)?;
    write_archive(d, &files)?;
    // The archive is checked; now the originals go. A failure here leaves the archive, which
    // restore reads, so nothing is lost.
    for (rel, path) in &files {
        if rel == "project.json" {
            continue;
        }
        crate::writefile::remove(path).map_err(|e| {
            other(format!("cannot remove {}: {e} (the archive {ARCHIVE} is complete; restore the project to get everything back)", path.display()))
        })?;
    }
    for c in CACHE_DIRS {
        let p = d.join(c);
        if p.is_dir() {
            std::fs::remove_dir_all(&p).map_err(|e| other(format!("cannot remove {}: {e}", p.display())))?;
        }
    }
    remove_empty_dirs(d);
    m.shelved = true;
    m.archive = ARCHIVE.to_string();
    m.archived_files = files.len() as u64;
    m.modified = project::timestamp();
    project::write_manifest(&m)?;
    Ok((before, project::dir_size(dir), files.len() as u64))
}

/// Unpack a shelved project in `dir` (every file as it was). A project that isn't shelved is left alone.
pub(super) fn restore_folder(dir: &str) -> Result<u64> {
    let d = Path::new(dir);
    let m = project::read_manifest(dir)?;
    if !m.shelved {
        return Ok(0);
    }
    let archive = d.join(ARCHIVE);
    // Check the whole archive before touching anything.
    read_archive(&archive, |_, _| Ok(()))?;
    let n = read_archive(&archive, |rel, data| {
        let target = d.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| other(format!("cannot create {}: {e}", parent.display())))?;
        }
        crate::writefile::write(&target, data).map_err(|e| other(format!("cannot write {}: {e}", target.display())))
    })?;
    crate::writefile::remove(&archive).map_err(|e| other(format!("cannot remove {}: {e}", archive.display())))?;
    Ok(n)
}

fn copy_tree(src: &Path, dst: &Path) -> Result<u64> {
    let files = collect(src, true)?;
    let mut n = 0;
    for (rel, path) in files {
        let target = dst.join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| other(format!("cannot create {}: {e}", parent.display())))?;
        }
        crate::writefile::copy(&path, &target).map_err(|e| other(format!("cannot copy {}: {e}", path.display())))?;
        n += 1;
    }
    Ok(n)
}

/// A new project from a template project: its standards and its support files (rules, overrides,
/// types), not its drawings. Returns the template's standards.
pub(super) fn apply_template(template: &str, dir: &str, drawings: &[String]) -> Result<Vec<Standard>> {
    let t = project::read_manifest(template)?;
    if t.shelved {
        return Err(other(format!("template `{}` is shelved; restore it first (project.restore_project)", t.name)));
    }
    let skip: std::collections::HashSet<&str> = drawings.iter().map(String::as_str).chain(["project.json"]).collect();
    for (rel, path) in collect(Path::new(template), true)? {
        if skip.contains(rel.as_str()) {
            continue;
        }
        let target = Path::new(dir).join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| other(format!("cannot create {}: {e}", parent.display())))?;
        }
        crate::writefile::copy(&path, &target).map_err(|e| other(format!("cannot copy {}: {e}", path.display())))?;
    }
    Ok(t.standards)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("project.list", "Projects", run_list)
            .files()
            .params("{status?: active|shelved} → {projects: [{name, path, status: active|shelved|missing, open, lastOpened, diskBytes, drawingCount, openReviewItems (null + openReviewItemsNote when not counted), validationScore (null + validationScoreNote when not available)}], total}: the projects on this machine (the registry); a project's drawings are project.drawings")
            .enabled(always)
            .noundo(),
        CommandSpec::new("project.duplicate", "Duplicate Project", run_duplicate)
            .files()
            .params("{to: new folder, path?: project folder (default: the open project), name?} → {project}: an independent copy without the caches (the source stays as it was)")
            .enabled(always)
            .noundo(),
        CommandSpec::new("project.delete", "Delete Project", run_delete)
            .files()
            .params("{path?: project folder (default: the open project), confirm: true, discard?: true (delete although a drawing of it has unsaved changes), deleteUnmanaged?: true (an agent: also the files its manifest doesn't list)} → {deleted, bytes}: removes the folder and everything in it for good; refused without confirm: true, and (a dry run says what it would take: wouldDelete) while a drawing of it has unsaved changes or, for an agent, the folder holds files the project doesn't manage")
            .enabled(always)
            .noundo(),
        CommandSpec::new("project.company.list", "Company Standards", run_company_list)
            .files()
            .params("{domain?} → {standards: [{domain, key, value, source: company}], total}: the company library every project inherits")
            .enabled(always)
            .noundo(),
        CommandSpec::new("project.company.set", "Set Company Standard", run_company_set)
            .files()
            .params("{domain, key, value} → {standard}: set a company standard; projects that don't override it follow")
            .enabled(always)
            .noundo(),
        CommandSpec::new("project.company.remove", "Remove Company Standard", run_company_remove)
            .files()
            .params("{domain, key} → {removed}")
            .enabled(always)
            .noundo(),
    ]
}

fn project_dir(s: &Session, p: &Value, cmd: &str) -> Result<String> {
    match str_param(p, "path") {
        Some(d) => Ok(d.trim_end_matches('/').to_string()),
        None => s.project.as_ref().map(|ps| ps.manifest.dir.clone()).ok_or_else(|| bad(cmd, "`path` is required when no project is open")),
    }
}

fn is_open(s: &Session, dir: &str) -> bool {
    s.project.as_ref().is_some_and(|ps| canon(&ps.manifest.dir) == canon(dir))
}

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let want = str_param(p, "status");
    let reg = load_registry(s);
    let review = project::open_review_count(s);
    let open_dir = s.project.as_ref().map(|ps| canon(&ps.manifest.dir));
    let mut out = Vec::new();
    for e in &reg.projects {
        let open = open_dir.as_deref() == Some(e.path.as_str());
        let (name, status, drawings) = match project::read_manifest(&e.path) {
            Ok(m) => (m.name.clone(), if m.shelved { "shelved" } else { "active" }, m.drawings.len()),
            Err(_) => (e.name.clone(), "missing", 0),
        };
        if want.is_some_and(|w| !w.eq_ignore_ascii_case(status)) {
            continue;
        }
        let mut v = json!({
            "name": name, "path": e.path, "status": status, "open": open,
            "lastOpened": e.last_opened, "diskBytes": project::dir_size(&e.path), "drawingCount": drawings,
            "validationScore": Value::Null,
            "validationScoreNote": "no validation run is recorded in the project folder",
        });
        match (open, review) {
            (true, Some(n)) => v["openReviewItems"] = json!(n),
            _ => {
                v["openReviewItems"] = Value::Null;
                v["openReviewItemsNote"] = json!(if status == "shelved" {
                    "shelved: restore and open the project to count"
                } else if open {
                    "no drawing of the project is open"
                } else {
                    "open the project to count its review items"
                });
            }
        }
        out.push(v);
    }
    let total = out.len();
    Ok(json!({ "projects": out, "total": total }))
}

fn run_duplicate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.duplicate";
    let to = str_param(p, "to").ok_or_else(|| bad(cmd, "`to` (the new project folder) is required"))?.trim_end_matches('/');
    let src = project_dir(s, p, cmd)?;
    let m = project::read_manifest(&src)?;
    if m.shelved {
        return Err(bad(cmd, format!("project `{}` is shelved; restore it first (project.restore_project {{path}})", m.name)));
    }
    let (cs, ct) = (canon(&src), canon(to));
    if ct == cs || ct.starts_with(&format!("{cs}/")) {
        return Err(bad(cmd, "`to` must be a folder outside the source project"));
    }
    if std::fs::read_dir(to).is_ok_and(|mut r| r.next().is_some()) {
        return Err(bad(cmd, format!("`{to}` already has files in it; pick a new folder")));
    }
    std::fs::create_dir_all(to).map_err(|e| other(format!("cannot create {to}: {e}")))?;
    copy_tree(Path::new(&src), Path::new(to))?;
    let mut copy = project::read_manifest(to)?;
    let now = project::timestamp();
    copy.name = str_param(p, "name").map_or_else(|| format!("{} copy", m.name), str::to_string);
    copy.dir = to.to_string();
    copy.created = now.clone();
    copy.modified = now;
    for d in &mut copy.drawings {
        d.status = project::DrawingStatus::Shelved;
    }
    project::write_manifest(&copy)?;
    register(s, &copy, false);
    Ok(json!({ "project": project::manifest_json(&copy) }))
}

fn run_delete(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.delete";
    let dir = project_dir(s, p, cmd)?;
    let m = project::read_manifest(&dir).map_err(|e| bad(cmd, format!("`{dir}` is not a project folder ({e}); nothing was deleted")))?;
    if p.get("confirm").and_then(Value::as_bool) != Some(true) && crate::agent::writes_allowed() {
        return Err(bad(
            cmd,
            format!(
                "deleting `{}` removes the folder {dir} and everything in it for good; call again with `confirm: true` to go ahead (project.shelve_project keeps it in one archive instead)",
                m.name
            ),
        ));
    }
    let discard = p.get("discard").and_then(Value::as_bool) == Some(true);
    let unsaved: Vec<String> =
        s.docs.iter().filter(|d| d.is_dirty() && d.path.as_deref().is_some_and(|x| project::in_project(&dir, x))).map(|d| d.title.clone()).collect();
    let (foreign, foreign_more) = unmanaged_files(&dir, &m);
    let bytes = project::dir_size(&dir);
    if !crate::agent::writes_allowed() {
        // A dry run says what it would take, and takes nothing.
        return Ok(
            json!({ "wouldDelete": { "path": dir, "bytes": bytes, "unsavedDrawings": unsaved, "unmanagedFiles": foreign, "unmanagedMore": foreign_more } }),
        );
    }
    if !unsaved.is_empty() && !discard {
        return Err(bad(
            cmd,
            format!(
                "unsaved changes in {}: save them (document.close save: true), or pass discard: true to delete the project without them; nothing was deleted",
                unsaved.join(", ")
            ),
        ));
    }
    if (!foreign.is_empty() || foreign_more > 0) && s.agent.from_agent && p.get("deleteUnmanaged").and_then(Value::as_bool) != Some(true) {
        let more = if foreign_more > 0 { format!(" and {foreign_more} more") } else { String::new() };
        return Err(bad(
            cmd,
            format!(
                "the folder holds files the project doesn't manage ({}{more}); nothing was deleted: move them out, or pass deleteUnmanaged: true to delete them too",
                foreign.join(", ")
            ),
        ));
    }
    if is_open(s, &dir) {
        project::close_project_state(s, false)?;
    }
    std::fs::remove_dir_all(&dir).map_err(|e| other(format!("cannot delete {dir}: {e}")))?;
    unregister(s, &dir);
    Ok(json!({ "deleted": m.name, "path": dir, "bytes": bytes }))
}

/// Files under a project folder that its manifest doesn't manage (not `project.json`, not a
/// listed drawing or file, nothing in a hidden folder): the first few, and how many more.
fn unmanaged_files(dir: &str, m: &project::ProjectManifest) -> (Vec<String>, usize) {
    const LISTED: usize = 20;
    const WALKED: usize = 100_000;
    let managed: std::collections::HashSet<String> = m.drawings.iter().map(|d| d.path.replace('\\', "/")).collect();
    let root = std::path::Path::new(dir);
    let mut stack = vec![root.to_path_buf()];
    let (mut listed, mut more, mut seen) = (Vec::new(), 0usize, 0usize);
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > WALKED {
                return (listed, more.saturating_add(1));
            }
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                stack.push(e.path());
                continue;
            }
            let rel = e.path().strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or(name);
            if rel == "project.json"
                || managed.contains(&rel)
                || rel.strip_suffix(".bak").is_some_and(|r| managed.iter().any(|m| m.rsplit_once('.').is_some_and(|(stem, _)| stem == r)))
            {
                continue;
            }
            if listed.len() < LISTED {
                listed.push(rel);
            } else {
                more += 1;
            }
        }
    }
    listed.sort();
    (listed, more)
}

fn std_params<'a>(p: &'a Value, cmd: &str) -> Result<(&'a str, &'a str)> {
    let domain = str_param(p, "domain").ok_or_else(|| bad(cmd, "`domain` required"))?;
    let key = str_param(p, "key").ok_or_else(|| bad(cmd, "`key` required"))?;
    Ok((domain, key))
}

fn run_company_list(s: &mut Session, p: &Value) -> Result<Value> {
    let domain = str_param(p, "domain");
    let list: Vec<Value> = load_company(s)
        .iter()
        .filter(|c| domain.is_none_or(|d| c.domain.eq_ignore_ascii_case(d)))
        .map(|c| json!({"domain": c.domain, "key": c.key, "value": c.value, "source": "company"}))
        .collect();
    let total = list.len();
    Ok(json!({ "standards": list, "total": total }))
}

fn run_company_set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.company.set";
    let (domain, key) = std_params(p, cmd)?;
    let value = p.get("value").cloned().ok_or_else(|| bad(cmd, "`value` required"))?;
    let mut list = load_company(s);
    if let Some(c) = list.iter_mut().find(|c| same(c, domain, key)) {
        c.value = value.clone();
    } else if list.len() >= project::MAX_STANDARDS {
        return Err(other(format!("the company library already has {} standards", project::MAX_STANDARDS)));
    } else {
        list.push(Standard { domain: domain.to_string(), key: key.to_string(), value: value.clone(), source: "company".into() });
    }
    save_company(s, list)?;
    Ok(json!({ "standard": { "domain": domain, "key": key, "value": value, "source": "company" } }))
}

fn run_company_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.company.remove";
    let (domain, key) = std_params(p, cmd)?;
    let mut list = load_company(s);
    let before = list.len();
    list.retain(|c| !same(c, domain, key));
    if list.len() == before {
        return Err(bad(cmd, format!("company standard `{domain}/{key}` not found")));
    }
    save_company(s, list)?;
    Ok(json!({ "removed": format!("{domain}/{key}") }))
}

/// Add `"project": <name>` to what a project-scoped command answered (print set, briefing, BIM
/// job, review): an agent always knows which project it is hearing about.
pub(crate) fn tag_project(s: &Session, id: &str, mut v: Value) -> Value {
    let scoped =
        id.starts_with("printset.") || id.starts_with("bim.") || id.starts_with("review.") || id == "project.briefing" || id == "project.note";
    if scoped
        && let Some(ps) = s.project.as_ref()
        && let Some(o) = v.as_object_mut()
    {
        o.insert("project".into(), json!(ps.manifest.name));
    }
    v
}
