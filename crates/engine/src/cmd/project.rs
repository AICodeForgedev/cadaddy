//! Multiple projects: open one, shelve the rest (Q325).
//!
//! A project is a self-contained folder with a `project.json` manifest listing documents,
//! a type catalog, a rules file, overrides, review/RFI lists, validation results, notes and
//! derived caches. Only the open project costs anything: startup, memory and the UI touch no
//! other project; closed projects are just folders.
//!
//! Commands:
//! - `project.open {path}`: open a project folder (reads its `project.json`);
//! - `project.list {}`: list drawings in the active project;
//! - `project.info {}`: active project metadata;
//! - `project.activate {name}`: open a drawing, shelving the current one;
//! - `project.shelve {}`: save and close the current drawing within the project;
//! - `project.restore {name}`: un-shelve an archived project on disk;
//! - `project.create {path, name?, metric?}`: create a new project folder;
//! - `project.close {}`: close the active project;
//! - `project.add {path}`: add an existing drawing to the project;
//! - `project.remove {name}`: remove a drawing from the manifest (file untouched);
//! - `project.shelve_project {path?}`: archive the project (drop caches, compress);
//! - `project.restore_project {path}`: restore a shelved project archive;
//! - `project.standards.list {}`: shared standards library;
//! - `project.standards.set {domain, key, value}`: set a standard;
//! - `project.standards.import {from, keys?}`: copy standards from another project.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::*;
use crate::{Result, Session};

// ---------------------------------------------------------------------------
// Manifest types
// ---------------------------------------------------------------------------

/// Status of a drawing within a project.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DrawingStatus {
    Active,
    #[default]
    Shelved,
    Archived,
}

/// A single document (drawing) in the project manifest.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDrawing {
    /// Relative path from the project root.
    pub path: String,
    /// Display name.
    pub title: String,
    /// Discipline ("Architectural", "Electrical", "Mechanical", …).
    #[serde(default)]
    pub discipline: String,
    /// Status within the project.
    #[serde(default)]
    pub status: DrawingStatus,
    /// Content hash for change detection.
    #[serde(default)]
    pub content_hash: String,
    /// Kind (drawing, spec, submittal, rfi, photo, markup, …).
    #[serde(default)]
    pub kind: String,
    /// Revision label.
    #[serde(default)]
    pub revision: String,
    /// Date string (ISO 8601).
    #[serde(default)]
    pub date: String,
    /// Linked items (e.g. "rfi" → sheet numbers, "submittal" → type marks).
    #[serde(default)]
    pub links: BTreeMap<String, Vec<String>>,
    /// Notes.
    #[serde(default)]
    pub notes: String,
    /// File size in bytes (cached at last scan).
    #[serde(default)]
    pub file_size: u64,
}

/// A standard (shared across projects or per-project override).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Standard {
    pub domain: String,
    pub key: String,
    pub value: Value,
    /// Where this standard came from ("built_in", "company", "project").
    #[serde(default)]
    pub source: String,
}

/// The project manifest (`project.json` in the project folder).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectManifest {
    pub name: String,
    /// Absolute path to the project folder.
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub modified: String,
    #[serde(default)]
    pub drawings: Vec<ProjectDrawing>,
    /// Project-level standards (layer schemes, mounting heights, etc.).
    #[serde(default)]
    pub standards: Vec<Standard>,
    /// Whether the project is archived.
    #[serde(default)]
    pub shelved: bool,
    /// Disk usage summary (bytes).
    #[serde(default)]
    pub disk_bytes: u64,
}

/// The active project state held in Session.
#[derive(Clone, Debug)]
pub struct ProjectState {
    pub manifest: ProjectManifest,
    /// Index of the currently active drawing (open) in `manifest.drawings`, or None.
    pub active_drawing: Option<usize>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Cap on drawings per project manifest.
const MAX_DRAWINGS: usize = 10_000;
/// Cap on standards per project.
const MAX_STANDARDS: usize = 10_000;

fn timestamp() -> String {
    // Use web_time for wasm compat.
    let d = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    // Approximate ISO 8601 (good enough for display; no chrono dependency).
    format!("{secs}")
}

fn content_hash(data: &[u8]) -> String {
    // Simple FNV-1a 64-bit hash as a hex string — not cryptographic, just for change detection.
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn read_manifest(dir: &str) -> Result<ProjectManifest> {
    let path = format!("{dir}/project.json");
    let data = std::fs::read(&path).map_err(|e| crate::EngineError::Other(format!("cannot read {path}: {e}")))?;
    let mut m: ProjectManifest = serde_json::from_slice(&data).map_err(|e| crate::EngineError::Other(format!("invalid project.json: {e}")))?;
    m.dir = dir.to_string();
    Ok(m)
}

fn write_manifest(m: &ProjectManifest) -> Result<()> {
    let path = format!("{}/project.json", m.dir);
    let json = serde_json::to_vec_pretty(m).map_err(|e| crate::EngineError::Other(format!("cannot serialize manifest: {e}")))?;
    crate::writefile::write(&path, &json).map_err(|e| crate::EngineError::Other(format!("cannot write {path}: {e}")))?;
    Ok(())
}

fn require_project(s: &Session) -> Result<&ProjectState> {
    s.project.as_ref().ok_or_else(|| crate::EngineError::Other("no project open".into()))
}

fn require_project_mut(s: &mut Session) -> Result<&mut ProjectState> {
    s.project.as_mut().ok_or_else(|| crate::EngineError::Other("no project open".into()))
}

fn manifest_json(m: &ProjectManifest) -> Value {
    json!({
        "name": m.name,
        "dir": m.dir,
        "created": m.created,
        "modified": m.modified,
        "drawings": m.drawings.len(),
        "shelved": m.shelved,
        "diskBytes": m.disk_bytes,
        "active": m.drawings.iter().filter(|d| d.status == DrawingStatus::Active).count(),
        "shelvedDrawings": m.drawings.iter().filter(|d| d.status == DrawingStatus::Shelved).count(),
        "archived": m.drawings.iter().filter(|d| d.status == DrawingStatus::Archived).count(),
        "standards": m.standards.len(),
    })
}

fn drawing_json(d: &ProjectDrawing) -> Value {
    json!({
        "path": d.path,
        "title": d.title,
        "discipline": d.discipline,
        "status": d.status,
        "kind": d.kind,
        "revision": d.revision,
        "date": d.date,
        "notes": d.notes,
        "fileSize": d.file_size,
        "contentHash": d.content_hash,
    })
}

/// Resolve a drawing name (title or relative path) to its index in the manifest.
fn find_drawing(m: &ProjectManifest, name: &str) -> Option<usize> {
    m.drawings.iter().position(|d| d.title.eq_ignore_ascii_case(name) || d.path.eq_ignore_ascii_case(name))
}

/// Absolute path to a drawing file inside the project.
fn drawing_abs_path(m: &ProjectManifest, d: &ProjectDrawing) -> String {
    if d.path.starts_with('/') || d.path.contains(':') { d.path.clone() } else { format!("{}/{}", m.dir, d.path) }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("project.create", "New Project", run_create)
            .files()
            .params("{path, name?, metric?: bool} → {project}: create a new project folder with an empty manifest")
            .enabled(always)
            .noundo(),
        CommandSpec::new("project.open", "Open Project", run_open)
            .files()
            .params("{path} → {project, drawings}: open a project folder")
            .enabled(always)
            .noundo(),
        CommandSpec::new("project.close", "Close Project", run_close)
            .files()
            .params("{} → {}: close the active project (drawings stay open)")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.info", "Project Info", run_info)
            .files()
            .params("{} → {project}: active project metadata")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.list", "Project Drawings", run_list)
            .files()
            .params("{discipline?, status?, kind?} → {drawings, total}: list drawings in the project")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.activate", "Activate Drawing", run_activate)
            .files()
            .params("{name} → {activated, shelved?}: open a drawing from the project, shelving the current one")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.shelve", "Shelve Drawing", run_shelve)
            .files()
            .params("{save?: bool} → {shelved}: shelve the active drawing (save and close it within the project)")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.add", "Add to Project", run_add)
            .files()
            .params("{path, discipline?, kind?, title?} → {drawing}: add an existing drawing to the project")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.remove", "Remove from Project", run_remove)
            .files()
            .params("{name} → {removed}: remove a drawing from the manifest (file untouched)")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.shelve_project", "Shelve Project", run_shelve_project)
            .files()
            .params("{} → {path, savedBytes}: archive the project (drop caches, mark shelved)")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.restore_project", "Restore Project", run_restore_project)
            .files()
            .params("{path} → {project}: restore a shelved project")
            .enabled(always)
            .noundo(),
        CommandSpec::new("project.disk", "Project Disk Use", run_disk)
            .files()
            .params("{} → {totalBytes, drawings: [{name, bytes}], caches}: disk use per drawing")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.standards.list", "Standards Library", run_standards_list)
            .files()
            .params("{domain?} → {standards}: list shared standards")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.standards.set", "Set Standard", run_standards_set)
            .files()
            .params("{domain, key, value, source?} → {standard}: set a standard")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.standards.remove", "Remove Standard", run_standards_remove)
            .files()
            .params("{domain, key} → {removed}")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.standards.import", "Import Standards", run_standards_import)
            .files()
            .params("{from, domain?, keys?} → {imported}: copy standards from another project")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.import_manifest", "Import Manifest", run_import_manifest)
            .files()
            .params("{path} → {project, added, changed, errors}: import/update a project from a folder + manifest.json")
            .enabled(always)
            .noundo(),
        CommandSpec::new("project.validate_manifest", "Validate Manifest", run_validate_manifest)
            .params("{path} → {valid, errors}: validate a manifest.json without importing")
            .enabled(always)
            .noundo(),
    ]
}

fn has_project(s: &Session) -> std::result::Result<(), String> {
    if s.project.is_some() { Ok(()) } else { Err("no project open".into()) }
}

// ---- project.create ----

fn run_create(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.create";
    let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;
    let name = str_param(p, "name").unwrap_or_else(|| path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("Project"));

    std::fs::create_dir_all(path).map_err(|e| crate::EngineError::Other(format!("cannot create {path}: {e}")))?;

    let now = timestamp();
    let m = ProjectManifest {
        name: name.to_string(),
        dir: path.to_string(),
        created: now.clone(),
        modified: now,
        drawings: Vec::new(),
        standards: Vec::new(),
        shelved: false,
        disk_bytes: 0,
    };
    write_manifest(&m)?;

    s.project = Some(ProjectState { manifest: m.clone(), active_drawing: None });

    Ok(json!({ "project": manifest_json(&m) }))
}

// ---- project.open ----

fn run_open(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.open";
    let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;

    let m = read_manifest(path)?;
    if m.drawings.len() > MAX_DRAWINGS {
        return Err(crate::EngineError::Other(format!("manifest has {} drawings (cap is {MAX_DRAWINGS})", m.drawings.len())));
    }

    let drawings: Vec<Value> = m.drawings.iter().map(drawing_json).collect();

    s.project = Some(ProjectState { manifest: m.clone(), active_drawing: None });

    Ok(json!({
        "project": manifest_json(&m),
        "drawings": drawings,
    }))
}

// ---- project.close ----

fn run_close(s: &mut Session, _p: &Value) -> Result<Value> {
    let ps = require_project(s)?;
    // Save manifest before closing.
    write_manifest(&ps.manifest)?;
    s.project = None;
    Ok(json!({}))
}

// ---- project.info ----

fn run_info(s: &mut Session, _p: &Value) -> Result<Value> {
    let ps = require_project(s)?;
    let mut info = manifest_json(&ps.manifest);
    if let Some(idx) = ps.active_drawing
        && let Some(d) = ps.manifest.drawings.get(idx)
    {
        info["activeDrawing"] = drawing_json(d);
    }
    Ok(json!({ "project": info }))
}

// ---- project.list ----

fn run_list(s: &mut Session, p: &Value) -> Result<Value> {
    let ps = require_project(s)?;
    let discipline = str_param(p, "discipline");
    let status = str_param(p, "status");
    let kind = str_param(p, "kind");

    let filtered: Vec<Value> = ps
        .manifest
        .drawings
        .iter()
        .filter(|d| {
            discipline.is_none_or(|f| d.discipline.eq_ignore_ascii_case(f))
                && status.is_none_or(|f| {
                    let ds = serde_json::to_value(&d.status).ok().and_then(|v| v.as_str().map(str::to_string));
                    ds.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(f))
                })
                && kind.is_none_or(|f| d.kind.eq_ignore_ascii_case(f))
        })
        .map(drawing_json)
        .collect();

    Ok(json!({
        "drawings": filtered,
        "total": filtered.len(),
    }))
}

// ---- project.activate ----

fn run_activate(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.activate";
    let name = str_param(p, "name").ok_or_else(|| bad(cmd, "`name` is required"))?;

    let ps = require_project_mut(s)?;
    let idx = find_drawing(&ps.manifest, name).ok_or_else(|| bad(cmd, format!("drawing `{name}` not found in the project")))?;

    // Mark the currently active drawing as shelved.
    let mut shelved_name = None;
    if let Some(prev) = ps.active_drawing
        && let Some(d) = ps.manifest.drawings.get_mut(prev)
    {
        d.status = DrawingStatus::Shelved;
        shelved_name = Some(d.title.clone());
    }

    // Mark the requested drawing as active.
    if let Some(d) = ps.manifest.drawings.get_mut(idx) {
        d.status = DrawingStatus::Active;
    }
    ps.active_drawing = Some(idx);

    // Save the updated manifest.
    write_manifest(&ps.manifest)?;

    let activated = ps.manifest.drawings.get(idx).map(drawing_json).unwrap_or(Value::Null);

    let mut result = json!({ "activated": activated });
    if let Some(name) = shelved_name {
        result["shelved"] = json!(name);
    }
    Ok(result)
}

// ---- project.shelve ----

fn run_shelve(s: &mut Session, _p: &Value) -> Result<Value> {
    let cmd = "project.shelve";
    let ps = require_project_mut(s)?;
    let idx = ps.active_drawing.ok_or_else(|| bad(cmd, "no drawing is active in the project"))?;

    if let Some(d) = ps.manifest.drawings.get_mut(idx) {
        d.status = DrawingStatus::Shelved;
    }
    let shelved = ps.manifest.drawings.get(idx).map(|d| d.title.clone()).unwrap_or_default();
    ps.active_drawing = None;

    write_manifest(&ps.manifest)?;

    Ok(json!({ "shelved": shelved }))
}

// ---- project.add ----

fn run_add(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.add";
    let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;
    let discipline = str_param(p, "discipline").unwrap_or("");
    let kind = str_param(p, "kind").unwrap_or("drawing");
    let title = str_param(p, "title").unwrap_or_else(|| path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("Untitled"));

    let ps = require_project_mut(s)?;
    if ps.manifest.drawings.len() >= MAX_DRAWINGS {
        return Err(crate::EngineError::Other(format!("project already has {MAX_DRAWINGS} drawings")));
    }

    // Make path relative to project dir if it's inside it.
    let rel = if path.starts_with(&ps.manifest.dir) { path.strip_prefix(&ps.manifest.dir).unwrap_or(path).trim_start_matches('/') } else { path };

    // Check for duplicates.
    if ps.manifest.drawings.iter().any(|d| d.path == rel) {
        return Err(bad(cmd, format!("`{rel}` is already in the project")));
    }

    // Read file size and hash if the file exists.
    let abs = drawing_abs_path(
        &ps.manifest,
        &ProjectDrawing {
            path: rel.to_string(),
            title: String::new(),
            discipline: String::new(),
            status: DrawingStatus::Shelved,
            content_hash: String::new(),
            kind: String::new(),
            revision: String::new(),
            date: String::new(),
            links: BTreeMap::new(),
            notes: String::new(),
            file_size: 0,
        },
    );
    let (file_size, hash) = match std::fs::read(&abs) {
        Ok(data) => (data.len() as u64, content_hash(&data)),
        Err(_) => (0, String::new()),
    };

    let drawing = ProjectDrawing {
        path: rel.to_string(),
        title: title.to_string(),
        discipline: discipline.to_string(),
        status: DrawingStatus::Shelved,
        content_hash: hash,
        kind: kind.to_string(),
        revision: String::new(),
        date: String::new(),
        links: BTreeMap::new(),
        notes: String::new(),
        file_size,
    };

    ps.manifest.drawings.push(drawing.clone());
    ps.manifest.modified = timestamp();
    write_manifest(&ps.manifest)?;

    Ok(json!({ "drawing": drawing_json(&drawing) }))
}

// ---- project.remove ----

fn run_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.remove";
    let name = str_param(p, "name").ok_or_else(|| bad(cmd, "`name` is required"))?;

    let ps = require_project_mut(s)?;
    let idx = find_drawing(&ps.manifest, name).ok_or_else(|| bad(cmd, format!("drawing `{name}` not found in the project")))?;

    let removed = ps.manifest.drawings.remove(idx);

    // Adjust active_drawing index.
    match ps.active_drawing {
        Some(a) if a == idx => ps.active_drawing = None,
        Some(a) if a > idx => ps.active_drawing = Some(a - 1),
        _ => {}
    }

    ps.manifest.modified = timestamp();
    write_manifest(&ps.manifest)?;

    Ok(json!({ "removed": removed.title }))
}

// ---- project.shelve_project ----

fn run_shelve_project(s: &mut Session, _p: &Value) -> Result<Value> {
    let ps = require_project_mut(s)?;

    // Drop rebuildable caches.
    let dir = ps.manifest.dir.clone();
    let cache_dirs = ["cache", "tiles", "temp", "indexes"];
    let mut saved_bytes: u64 = 0;
    for d in &cache_dirs {
        let p = format!("{dir}/{d}");
        if let Ok(md) = std::fs::metadata(&p)
            && md.is_dir()
        {
            saved_bytes += dir_size(&p);
            let _ = std::fs::remove_dir_all(&p);
        }
    }

    ps.manifest.shelved = true;
    ps.manifest.modified = timestamp();
    write_manifest(&ps.manifest)?;

    Ok(json!({
        "path": dir,
        "savedBytes": saved_bytes,
    }))
}

fn dir_size(path: &str) -> u64 {
    let mut total: u64 = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(md) = entry.metadata() {
                if md.is_file() {
                    total = total.saturating_add(md.len());
                } else if md.is_dir()
                    && let Some(p) = entry.path().to_str()
                {
                    total = total.saturating_add(dir_size(p));
                }
            }
        }
    }
    total
}

// ---- project.restore_project ----

fn run_restore_project(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.restore_project";
    let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;

    let mut m = read_manifest(path)?;
    m.shelved = false;
    m.modified = timestamp();
    write_manifest(&m)?;

    let info = manifest_json(&m);
    s.project = Some(ProjectState { manifest: m, active_drawing: None });

    Ok(json!({ "project": info }))
}

// ---- project.disk ----

fn run_disk(s: &mut Session, _p: &Value) -> Result<Value> {
    let ps = require_project(s)?;
    let dir = &ps.manifest.dir;

    let mut drawings_info: Vec<Value> = Vec::new();
    let mut total: u64 = 0;

    for d in &ps.manifest.drawings {
        let abs = drawing_abs_path(&ps.manifest, d);
        let size = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
        total = total.saturating_add(size);
        drawings_info.push(json!({"name": d.title, "bytes": size}));
    }

    let cache_dirs = ["cache", "tiles", "temp", "indexes"];
    let mut cache_bytes: u64 = 0;
    for d in &cache_dirs {
        let p = format!("{dir}/{d}");
        cache_bytes = cache_bytes.saturating_add(dir_size(&p));
    }
    total = total.saturating_add(cache_bytes);

    Ok(json!({
        "totalBytes": total,
        "drawings": drawings_info,
        "caches": cache_bytes,
    }))
}

// ---- project.standards.list ----

fn run_standards_list(s: &mut Session, p: &Value) -> Result<Value> {
    let ps = require_project(s)?;
    let domain = str_param(p, "domain");

    let filtered: Vec<Value> = ps
        .manifest
        .standards
        .iter()
        .filter(|st| domain.is_none_or(|d| st.domain.eq_ignore_ascii_case(d)))
        .map(|st| {
            json!({
                "domain": st.domain,
                "key": st.key,
                "value": st.value,
                "source": st.source,
            })
        })
        .collect();

    Ok(json!({ "standards": filtered, "total": filtered.len() }))
}

// ---- project.standards.set ----

fn run_standards_set(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.standards.set";
    let domain = str_param(p, "domain").ok_or_else(|| bad(cmd, "`domain` required"))?;
    let key = str_param(p, "key").ok_or_else(|| bad(cmd, "`key` required"))?;
    let value = p.get("value").cloned().ok_or_else(|| bad(cmd, "`value` required"))?;
    let source = str_param(p, "source").unwrap_or("project");

    let ps = require_project_mut(s)?;
    if ps.manifest.standards.len() >= MAX_STANDARDS {
        return Err(crate::EngineError::Other(format!("project already has {MAX_STANDARDS} standards")));
    }

    // Upsert.
    if let Some(st) = ps.manifest.standards.iter_mut().find(|st| st.domain.eq_ignore_ascii_case(domain) && st.key.eq_ignore_ascii_case(key)) {
        st.value = value.clone();
        st.source = source.to_string();
    } else {
        ps.manifest.standards.push(Standard { domain: domain.to_string(), key: key.to_string(), value: value.clone(), source: source.to_string() });
    }

    ps.manifest.modified = timestamp();
    write_manifest(&ps.manifest)?;

    Ok(json!({ "standard": { "domain": domain, "key": key, "value": value, "source": source } }))
}

// ---- project.standards.remove ----

fn run_standards_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.standards.remove";
    let domain = str_param(p, "domain").ok_or_else(|| bad(cmd, "`domain` required"))?;
    let key = str_param(p, "key").ok_or_else(|| bad(cmd, "`key` required"))?;

    let ps = require_project_mut(s)?;
    let before = ps.manifest.standards.len();
    ps.manifest.standards.retain(|st| !(st.domain.eq_ignore_ascii_case(domain) && st.key.eq_ignore_ascii_case(key)));

    if ps.manifest.standards.len() == before {
        return Err(bad(cmd, format!("standard `{domain}/{key}` not found")));
    }

    ps.manifest.modified = timestamp();
    write_manifest(&ps.manifest)?;

    Ok(json!({ "removed": format!("{domain}/{key}") }))
}

// ---- project.standards.import ----

fn run_standards_import(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.standards.import";
    let from_path = str_param(p, "from").ok_or_else(|| bad(cmd, "`from` (project path) required"))?;
    let domain_filter = str_param(p, "domain");
    let keys_filter: Option<Vec<&str>> = p.get("keys").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect());

    let source = read_manifest(from_path)?;

    let ps = require_project_mut(s)?;
    let mut imported = 0u64;

    for st in &source.standards {
        if domain_filter.is_some_and(|d| !st.domain.eq_ignore_ascii_case(d)) {
            continue;
        }
        if keys_filter.as_ref().is_some_and(|ks| !ks.iter().any(|k| k.eq_ignore_ascii_case(&st.key))) {
            continue;
        }
        if ps.manifest.standards.len() >= MAX_STANDARDS {
            break;
        }

        // Upsert into the target project.
        if let Some(existing) =
            ps.manifest.standards.iter_mut().find(|x| x.domain.eq_ignore_ascii_case(&st.domain) && x.key.eq_ignore_ascii_case(&st.key))
        {
            existing.value = st.value.clone();
            existing.source = format!("imported from {}", source.name);
        } else {
            ps.manifest.standards.push(Standard {
                domain: st.domain.clone(),
                key: st.key.clone(),
                value: st.value.clone(),
                source: format!("imported from {}", source.name),
            });
        }
        imported += 1;
    }

    ps.manifest.modified = timestamp();
    write_manifest(&ps.manifest)?;

    Ok(json!({ "imported": imported, "from": source.name }))
}

// ---- project.import_manifest (Q314 merged) ----

/// Validate a manifest.json schema.
fn validate_manifest_data(data: &[u8]) -> Vec<String> {
    let mut errors = Vec::new();

    let v: Value = match serde_json::from_slice(data) {
        Ok(v) => v,
        Err(e) => {
            errors.push(format!("invalid JSON: {e}"));
            return errors;
        }
    };

    if !v.is_object() {
        errors.push("manifest must be a JSON object".into());
        return errors;
    }

    // Check required fields.
    if v.get("files").and_then(Value::as_array).is_none() {
        errors.push("`files` array is required".into());
    }

    if let Some(files) = v.get("files").and_then(Value::as_array) {
        let mut seen_paths = std::collections::HashSet::new();
        for (i, f) in files.iter().enumerate() {
            if !f.is_object() {
                errors.push(format!("files[{i}] must be an object"));
                continue;
            }
            let path = f.get("path").and_then(Value::as_str).unwrap_or("");
            if path.is_empty() {
                errors.push(format!("files[{i}].path is required"));
            }
            if !seen_paths.insert(path.to_string()) {
                errors.push(format!("duplicate path: `{path}`"));
            }
            // Validate kind if present.
            if let Some(kind) = f.get("kind").and_then(Value::as_str) {
                let valid = ["drawing", "sheet", "spec", "submittal", "rfi", "asi", "addendum", "photo", "markup", "bulletin", "other"];
                if !valid.contains(&kind) {
                    errors.push(format!("files[{i}].kind `{kind}` is not recognized"));
                }
            }
        }
    }

    errors
}

fn run_validate_manifest(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.validate_manifest";
    let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;
    let _ = s; // keep the signature, don't need session

    let data = std::fs::read(path).map_err(|e| crate::EngineError::Other(format!("cannot read {path}: {e}")))?;

    let errors = validate_manifest_data(&data);
    Ok(json!({
        "valid": errors.is_empty(),
        "errors": errors,
    }))
}

fn run_import_manifest(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.import_manifest";
    let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;

    // Read manifest.json from the given path.
    let manifest_path = if path.ends_with("manifest.json") { path.to_string() } else { format!("{path}/manifest.json") };

    let data = std::fs::read(&manifest_path).map_err(|e| crate::EngineError::Other(format!("cannot read {manifest_path}: {e}")))?;

    let errors = validate_manifest_data(&data);
    if !errors.is_empty() {
        return Ok(json!({
            "valid": false,
            "errors": errors,
        }));
    }

    let v: Value = serde_json::from_slice(&data).map_err(|e| crate::EngineError::Other(format!("invalid manifest: {e}")))?;

    let source_dir = if path.ends_with("manifest.json") { path.rsplit_once('/').map_or(path, |(d, _)| d).to_string() } else { path.to_string() };

    let files = v.get("files").and_then(Value::as_array).cloned().unwrap_or_default();

    // If no project is open, create one from the manifest.
    if s.project.is_none() {
        let name = v.get("name").and_then(Value::as_str).unwrap_or_else(|| source_dir.rsplit('/').find(|s| !s.is_empty()).unwrap_or("Imported"));
        let now = timestamp();
        let m = ProjectManifest {
            name: name.to_string(),
            dir: source_dir.clone(),
            created: now.clone(),
            modified: now,
            drawings: Vec::new(),
            standards: Vec::new(),
            shelved: false,
            disk_bytes: 0,
        };
        s.project = Some(ProjectState { manifest: m, active_drawing: None });
    }

    let ps = require_project_mut(s)?;
    let mut added = 0u64;
    let mut changed = 0u64;
    let mut import_errors: Vec<String> = Vec::new();

    for f in &files {
        let file_path = match f.get("path").and_then(Value::as_str) {
            Some(p) => p,
            None => continue,
        };

        // Check the file exists.
        let abs = if file_path.starts_with('/') || file_path.contains(':') { file_path.to_string() } else { format!("{source_dir}/{file_path}") };

        if std::fs::metadata(&abs).is_err() {
            import_errors.push(format!("missing file: `{file_path}`"));
            // Still add to manifest — the file can arrive later.
        }

        let kind = f.get("kind").and_then(Value::as_str).unwrap_or("drawing");
        let discipline = f.get("discipline").and_then(Value::as_str).unwrap_or("");
        let revision = f.get("revision").and_then(Value::as_str).unwrap_or("");
        let date = f.get("date").and_then(Value::as_str).unwrap_or("");
        let status_str = f.get("status").and_then(Value::as_str).unwrap_or("");
        let title = f.get("title").and_then(Value::as_str).unwrap_or_else(|| file_path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("Untitled"));

        // Parse links.
        let mut links = BTreeMap::new();
        if let Some(link_obj) = f.get("links").and_then(Value::as_object) {
            for (k, v) in link_obj {
                if let Some(arr) = v.as_array() {
                    let vals: Vec<String> = arr.iter().filter_map(Value::as_str).map(str::to_string).collect();
                    links.insert(k.clone(), vals);
                }
            }
        }

        let rel = file_path.to_string();

        // File size and hash.
        let (file_size, hash) = match std::fs::read(&abs) {
            Ok(data) => (data.len() as u64, content_hash(&data)),
            Err(_) => (0, String::new()),
        };

        // Check if already in the manifest.
        if let Some(existing) = ps.manifest.drawings.iter_mut().find(|d| d.path == rel) {
            // Update if hash changed.
            if existing.content_hash != hash && !hash.is_empty() {
                existing.content_hash = hash;
                existing.revision = revision.to_string();
                existing.date = date.to_string();
                existing.file_size = file_size;
                existing.links = links;
                existing.notes = status_str.to_string();
                changed += 1;
            }
        } else {
            if ps.manifest.drawings.len() >= MAX_DRAWINGS {
                import_errors.push(format!("cap of {MAX_DRAWINGS} drawings reached; skipping `{file_path}`"));
                continue;
            }
            ps.manifest.drawings.push(ProjectDrawing {
                path: rel,
                title: title.to_string(),
                discipline: discipline.to_string(),
                status: DrawingStatus::Shelved,
                content_hash: hash,
                kind: kind.to_string(),
                revision: revision.to_string(),
                date: date.to_string(),
                links,
                notes: status_str.to_string(),
                file_size,
            });
            added += 1;
        }
    }

    ps.manifest.modified = timestamp();
    write_manifest(&ps.manifest)?;

    Ok(json!({
        "project": manifest_json(&ps.manifest),
        "added": added,
        "changed": changed,
        "errors": import_errors,
    }))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use std::fs;

    fn temp_dir(name: &str) -> String {
        let dir = format!("/tmp/cadaddy_project_test_{name}_{}", std::process::id());
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).ok();
        dir
    }

    fn cleanup(dir: &str) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn create_and_open_project() {
        let dir = temp_dir("create_open");
        let mut s = Session::empty();

        // Create a project.
        let r = s.execute("project.create", &json!({"path": &dir, "name": "Test Project"}));
        assert!(r.is_ok(), "create failed: {r:?}");
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["project"]["name"], "Test Project");
        assert!(s.project.is_some());

        // Close it.
        let r = s.execute("project.close", &json!({}));
        assert!(r.is_ok());
        assert!(s.project.is_none());

        // Re-open it.
        let r = s.execute("project.open", &json!({"path": &dir}));
        assert!(r.is_ok());
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["project"]["name"], "Test Project");

        cleanup(&dir);
    }

    #[test]
    fn add_remove_drawings() {
        let dir = temp_dir("add_remove");
        let mut s = Session::empty();

        // Create project and a dummy drawing file.
        s.execute("project.create", &json!({"path": &dir})).ok();
        fs::write(format!("{dir}/plan.dxf"), b"dummy dxf").ok();
        fs::write(format!("{dir}/elec.dxf"), b"dummy elec").ok();

        // Add drawings.
        let r = s.execute("project.add", &json!({"path": "plan.dxf", "title": "Floor Plan", "discipline": "Architectural"}));
        assert!(r.is_ok());

        let r = s.execute("project.add", &json!({"path": "elec.dxf", "title": "Electrical", "discipline": "Electrical"}));
        assert!(r.is_ok());

        // List them.
        let r = s.execute("project.list", &json!({}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 2);

        // Filter by discipline.
        let r = s.execute("project.list", &json!({"discipline": "Electrical"}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 1);

        // Duplicate add fails.
        let r = s.execute("project.add", &json!({"path": "plan.dxf"}));
        assert!(r.is_err());

        // Remove one.
        let r = s.execute("project.remove", &json!({"name": "Floor Plan"}));
        assert!(r.is_ok());

        let r = s.execute("project.list", &json!({}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 1);

        cleanup(&dir);
    }

    #[test]
    fn activate_and_shelve() {
        let dir = temp_dir("activate_shelve");
        let mut s = Session::empty();

        s.execute("project.create", &json!({"path": &dir})).ok();
        fs::write(format!("{dir}/a.dxf"), b"a").ok();
        fs::write(format!("{dir}/b.dxf"), b"b").ok();
        s.execute("project.add", &json!({"path": "a.dxf", "title": "Drawing A"})).ok();
        s.execute("project.add", &json!({"path": "b.dxf", "title": "Drawing B"})).ok();

        // Activate A.
        let r = s.execute("project.activate", &json!({"name": "Drawing A"}));
        assert!(r.is_ok());
        assert_eq!(s.project.as_ref().map(|p| p.active_drawing), Some(Some(0)));

        // Activate B — A becomes shelved.
        let r = s.execute("project.activate", &json!({"name": "Drawing B"}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["shelved"], "Drawing A");
        assert_eq!(s.project.as_ref().map(|p| p.active_drawing), Some(Some(1)));

        // Check A is shelved.
        let ps = s.project.as_ref().map(|p| &p.manifest.drawings[0].status);
        assert_eq!(ps, Some(&DrawingStatus::Shelved));

        // Shelve B.
        let r = s.execute("project.shelve", &json!({}));
        assert!(r.is_ok());
        assert_eq!(s.project.as_ref().map(|p| p.active_drawing), Some(None));

        cleanup(&dir);
    }

    #[test]
    fn two_projects_stay_isolated() {
        let dir_a = temp_dir("iso_a");
        let dir_b = temp_dir("iso_b");
        let mut s = Session::empty();

        // Create project A with a drawing.
        s.execute("project.create", &json!({"path": &dir_a, "name": "Project A"})).ok();
        fs::write(format!("{dir_a}/floor.dxf"), b"floor").ok();
        s.execute("project.add", &json!({"path": "floor.dxf", "title": "Floor"})).ok();
        s.execute("project.close", &json!({})).ok();

        // Create project B with a different drawing.
        s.execute("project.create", &json!({"path": &dir_b, "name": "Project B"})).ok();
        fs::write(format!("{dir_b}/elec.dxf"), b"elec").ok();
        s.execute("project.add", &json!({"path": "elec.dxf", "title": "Elec"})).ok();
        s.execute("project.close", &json!({})).ok();

        // Open A — should not see B's drawings.
        s.execute("project.open", &json!({"path": &dir_a})).ok();
        let r = s.execute("project.list", &json!({}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 1);
        assert_eq!(v["drawings"][0]["title"], "Floor");
        s.execute("project.close", &json!({})).ok();

        // Open B — should not see A's drawings.
        s.execute("project.open", &json!({"path": &dir_b})).ok();
        let r = s.execute("project.list", &json!({}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 1);
        assert_eq!(v["drawings"][0]["title"], "Elec");
        s.execute("project.close", &json!({})).ok();

        cleanup(&dir_a);
        cleanup(&dir_b);
    }

    #[test]
    fn shelve_project_drops_caches() {
        let dir = temp_dir("shelve_proj");
        let mut s = Session::empty();

        s.execute("project.create", &json!({"path": &dir})).ok();

        // Create a cache directory with content.
        fs::create_dir_all(format!("{dir}/cache")).ok();
        fs::write(format!("{dir}/cache/tile.bin"), vec![0u8; 1024]).ok();

        let r = s.execute("project.shelve_project", &json!({}));
        let v = r.unwrap_or(Value::Null);
        assert!(v["savedBytes"].as_u64().unwrap_or(0) > 0);

        // Cache dir should be gone.
        assert!(!std::path::Path::new(&format!("{dir}/cache")).exists());

        // Manifest should say shelved.
        let m = read_manifest(&dir);
        assert!(m.is_ok());
        assert!(m.ok().map(|m| m.shelved).unwrap_or(false));

        cleanup(&dir);
    }

    #[test]
    fn restore_project() {
        let dir = temp_dir("restore_proj");
        let mut s = Session::empty();

        s.execute("project.create", &json!({"path": &dir})).ok();
        s.execute("project.shelve_project", &json!({})).ok();
        s.execute("project.close", &json!({})).ok();

        // Restore it.
        let r = s.execute("project.restore_project", &json!({"path": &dir}));
        assert!(r.is_ok());
        let ps = s.project.as_ref();
        assert!(ps.is_some());
        assert!(!ps.map(|p| p.manifest.shelved).unwrap_or(true));

        cleanup(&dir);
    }

    #[test]
    fn shelved_project_adds_nothing_to_startup() {
        // A shelved project is just a folder — opening it shouldn't load any drawings into memory.
        let dir = temp_dir("startup_cost");
        let mut s = Session::empty();

        s.execute("project.create", &json!({"path": &dir})).ok();
        fs::write(format!("{dir}/big.dxf"), vec![0u8; 100_000]).ok();
        s.execute("project.add", &json!({"path": "big.dxf", "title": "Big Drawing"})).ok();
        s.execute("project.close", &json!({})).ok();

        // Open the project: no docs should be loaded.
        s.execute("project.open", &json!({"path": &dir})).ok();
        assert!(s.docs.is_empty(), "opening a project must not load drawings into Session::docs");

        cleanup(&dir);
    }

    #[test]
    fn standards_crud() {
        let dir = temp_dir("standards");
        let mut s = Session::empty();

        s.execute("project.create", &json!({"path": &dir})).ok();

        // Set standards.
        s.execute(
            "project.standards.set",
            &json!({
                "domain": "electrical",
                "key": "mounting_height_switch",
                "value": 48
            }),
        )
        .ok();

        s.execute(
            "project.standards.set",
            &json!({
                "domain": "electrical",
                "key": "mounting_height_outlet",
                "value": 18
            }),
        )
        .ok();

        s.execute(
            "project.standards.set",
            &json!({
                "domain": "mechanical",
                "key": "duct_insulation",
                "value": "R-8"
            }),
        )
        .ok();

        // List all.
        let r = s.execute("project.standards.list", &json!({}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 3);

        // Filter by domain.
        let r = s.execute("project.standards.list", &json!({"domain": "electrical"}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 2);

        // Update existing.
        s.execute(
            "project.standards.set",
            &json!({
                "domain": "electrical",
                "key": "mounting_height_switch",
                "value": 44
            }),
        )
        .ok();

        let r = s.execute("project.standards.list", &json!({"domain": "electrical"}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 2); // Still 2, not 3.

        // Remove.
        s.execute(
            "project.standards.remove",
            &json!({
                "domain": "electrical",
                "key": "mounting_height_switch"
            }),
        )
        .ok();

        let r = s.execute("project.standards.list", &json!({}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 2);

        cleanup(&dir);
    }

    #[test]
    fn standards_import() {
        let dir_a = temp_dir("std_imp_a");
        let dir_b = temp_dir("std_imp_b");
        let mut s = Session::empty();

        // Project A with standards.
        s.execute("project.create", &json!({"path": &dir_a, "name": "Source"})).ok();
        s.execute("project.standards.set", &json!({"domain": "electrical", "key": "wire_method", "value": "EMT"})).ok();
        s.execute("project.standards.set", &json!({"domain": "mechanical", "key": "duct_type", "value": "rectangular"})).ok();
        s.execute("project.close", &json!({})).ok();

        // Project B imports from A.
        s.execute("project.create", &json!({"path": &dir_b, "name": "Target"})).ok();
        let r = s.execute("project.standards.import", &json!({"from": &dir_a}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["imported"], 2);

        let r = s.execute("project.standards.list", &json!({}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 2);

        cleanup(&dir_a);
        cleanup(&dir_b);
    }

    #[test]
    fn validate_manifest_good() {
        let dir = temp_dir("validate_good");
        let mut s = Session::empty();
        fs::create_dir_all(&dir).ok();

        let manifest = json!({
            "files": [
                {"path": "plan.dxf", "kind": "drawing", "discipline": "Architectural"},
                {"path": "elec.dxf", "kind": "sheet", "discipline": "Electrical"}
            ]
        });
        fs::write(format!("{dir}/manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default()).ok();

        let r = s.execute("project.validate_manifest", &json!({"path": format!("{dir}/manifest.json")}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["valid"], true);
        assert!(v["errors"].as_array().is_some_and(|a| a.is_empty()));

        cleanup(&dir);
    }

    #[test]
    fn validate_manifest_bad() {
        let dir = temp_dir("validate_bad");
        let mut s = Session::empty();
        fs::create_dir_all(&dir).ok();

        // Missing files array.
        let manifest = json!({"name": "broken"});
        fs::write(format!("{dir}/manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default()).ok();

        let r = s.execute("project.validate_manifest", &json!({"path": format!("{dir}/manifest.json")}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["valid"], false);

        // Duplicate paths.
        let manifest = json!({
            "files": [
                {"path": "a.dxf"},
                {"path": "a.dxf"}
            ]
        });
        fs::write(format!("{dir}/manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default()).ok();

        let r = s.execute("project.validate_manifest", &json!({"path": format!("{dir}/manifest.json")}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["valid"], false);
        assert!(v["errors"].as_array().is_some_and(|a| a.iter().any(|e| e.as_str().unwrap_or("").contains("duplicate"))));

        cleanup(&dir);
    }

    #[test]
    fn import_manifest_with_missing_files() {
        let dir = temp_dir("import_missing");
        let mut s = Session::empty();
        fs::create_dir_all(&dir).ok();

        // Only create one of two listed files.
        fs::write(format!("{dir}/plan.dxf"), b"plan data").ok();

        let manifest = json!({
            "name": "Hotel Job",
            "files": [
                {"path": "plan.dxf", "kind": "drawing", "title": "Floor Plan"},
                {"path": "missing.dxf", "kind": "drawing", "title": "Missing Sheet"}
            ]
        });
        fs::write(format!("{dir}/manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default()).ok();

        let r = s.execute("project.import_manifest", &json!({"path": &dir}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["added"], 2);
        assert!(v["errors"].as_array().is_some_and(|a| a.iter().any(|e| e.as_str().unwrap_or("").contains("missing"))));

        cleanup(&dir);
    }

    #[test]
    fn shelve_restore_roundtrip_identical() {
        let dir = temp_dir("roundtrip");
        let mut s = Session::empty();

        s.execute("project.create", &json!({"path": &dir, "name": "Roundtrip"})).ok();
        fs::write(format!("{dir}/a.dxf"), b"drawing A data").ok();
        fs::write(format!("{dir}/b.dxf"), b"drawing B data").ok();
        s.execute("project.add", &json!({"path": "a.dxf", "title": "A", "discipline": "Arch"})).ok();
        s.execute("project.add", &json!({"path": "b.dxf", "title": "B", "discipline": "Elec"})).ok();
        s.execute("project.standards.set", &json!({"domain": "elec", "key": "voltage", "value": 120})).ok();

        // Snapshot the state.
        let before_drawings: Vec<String> =
            s.project.as_ref().map(|p| p.manifest.drawings.iter().map(|d| d.title.clone()).collect()).unwrap_or_default();
        let before_standards = s.project.as_ref().map(|p| p.manifest.standards.len()).unwrap_or(0);

        // Shelve and restore.
        s.execute("project.shelve_project", &json!({})).ok();
        s.execute("project.close", &json!({})).ok();
        s.execute("project.restore_project", &json!({"path": &dir})).ok();

        // Compare.
        let after_drawings: Vec<String> =
            s.project.as_ref().map(|p| p.manifest.drawings.iter().map(|d| d.title.clone()).collect()).unwrap_or_default();
        let after_standards = s.project.as_ref().map(|p| p.manifest.standards.len()).unwrap_or(0);

        assert_eq!(before_drawings, after_drawings, "drawings differ after shelve→restore");
        assert_eq!(before_standards, after_standards, "standards differ after shelve→restore");

        cleanup(&dir);
    }

    #[test]
    fn no_project_commands_fail_gracefully() {
        let mut s = Session::empty();

        // All project commands that require an open project should fail with a clear message.
        let cmds = [
            ("project.list", json!({})),
            ("project.info", json!({})),
            ("project.activate", json!({"name": "x"})),
            ("project.shelve", json!({})),
            ("project.add", json!({"path": "x.dxf"})),
            ("project.remove", json!({"name": "x"})),
            ("project.disk", json!({})),
            ("project.standards.list", json!({})),
        ];

        for (id, params) in &cmds {
            let r = s.execute(id, params);
            assert!(r.is_err(), "{id} should fail with no project open");
        }
    }

    #[test]
    fn hostile_manifest_never_panics() {
        let dir = temp_dir("hostile");
        let mut s = Session::empty();
        fs::create_dir_all(&dir).ok();

        // Truncated JSON.
        fs::write(format!("{dir}/project.json"), b"{\"name\": \"tr").ok();
        let r = s.execute("project.open", &json!({"path": &dir}));
        assert!(r.is_err());

        // Empty file.
        fs::write(format!("{dir}/project.json"), b"").ok();
        let r = s.execute("project.open", &json!({"path": &dir}));
        assert!(r.is_err());

        // Array instead of object.
        fs::write(format!("{dir}/project.json"), b"[1,2,3]").ok();
        let r = s.execute("project.open", &json!({"path": &dir}));
        assert!(r.is_err());

        // Huge drawings array (just many entries).
        let huge = json!({
            "name": "huge",
            "drawings": (0..MAX_DRAWINGS + 1).map(|i| json!({"path": format!("f{i}.dxf"), "title": format!("D{i}")})).collect::<Vec<_>>()
        });
        fs::write(format!("{dir}/project.json"), serde_json::to_vec(&huge).unwrap_or_default()).ok();
        let r = s.execute("project.open", &json!({"path": &dir}));
        assert!(r.is_err(), "should reject manifest with too many drawings");

        cleanup(&dir);
    }
}
