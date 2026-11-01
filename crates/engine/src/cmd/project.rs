//! Multiple projects: open one, shelve the rest (Q325).
//!
//! A project is a self-contained folder with a `project.json` manifest listing documents,
//! a type catalog, a rules file, overrides, review/RFI lists, validation results, notes and
//! derived caches. Only the open project costs anything: startup, memory and the UI touch no
//! other project; closed projects are just folders.
//!
//! Commands:
//! - `project.open {path}`: open a project folder (reads its `project.json`); another open
//!   project is saved and closed first, with its print set and drawings;
//! - `project.list {}`: the projects on this machine ([`project_hub`]);
//! - `project.drawings {}`: the drawings of the open project;
//! - `project.info {}`: active project metadata;
//! - `project.activate {name}`: open a drawing into the session, saving and closing the current one;
//! - `project.shelve {}`: save and close the current drawing;
//! - `project.create {path, name?, metric?, template?}`: create a new project folder;
//! - `project.close {}`: save and close the project's drawings and drop its state;
//! - `project.add {path}`: add an existing drawing to the project;
//! - `project.remove {name}`: remove a drawing from the manifest (file untouched);
//! - `project.shelve_project {path?}`: pack the project into one archive;
//! - `project.restore_project {path}`: unpack a shelved project, byte for byte;
//! - `project.duplicate`, `project.delete`, `project.company.*`: see [`project_hub`];
//! - `project.standards.list {}`: the effective standards (company library, project overrides);
//! - `project.standards.set {domain, key, value}`: set a project override;
//! - `project.standards.import {from, keys?}`: copy standards from another project;
//! - `project.import_manifest {path}`: import a folder + `manifest.json` (docs/project-manifest.md).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

use super::project_hub as hub;
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
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
    /// Review status of a submittal (approved / approved as noted / revise and resubmit /
    /// rejected) or an RFI (open / answered), from the manifest; empty for other kinds.
    #[serde(default)]
    pub review_status: String,
    /// Set by a manifest import: the entry follows the manifest (a re-import can drop it).
    #[serde(default)]
    pub from_manifest: bool,
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
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
    /// The archive holding a shelved project's files (empty when not shelved).
    #[serde(default)]
    pub archive: String,
    /// How many files the archive holds.
    #[serde(default)]
    pub archived_files: u64,
    /// Paths a manifest import added or changed: what is left to look at.
    #[serde(default)]
    pub queue: Vec<String>,
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
pub(super) const MAX_STANDARDS: usize = 10_000;

/// Now as an ISO 8601 UTC time (`2026-10-31T14:05:09Z`).
pub(super) fn timestamp() -> String {
    // Use web_time for wasm compat.
    let d = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).unwrap_or_default();
    iso8601(d.as_secs())
}

/// Seconds since 1970 as an ISO 8601 UTC time (days from civil, proleptic Gregorian).
fn iso8601(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
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

pub(super) fn read_manifest(dir: &str) -> Result<ProjectManifest> {
    let path = format!("{dir}/project.json");
    let data = std::fs::read(&path).map_err(|e| crate::EngineError::Other(format!("cannot read {path}: {e}")))?;
    let mut m: ProjectManifest = serde_json::from_slice(&data).map_err(|e| crate::EngineError::Other(format!("invalid project.json: {e}")))?;
    m.dir = dir.to_string();
    Ok(m)
}

pub(super) fn write_manifest(m: &ProjectManifest) -> Result<()> {
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

pub(super) fn manifest_json(m: &ProjectManifest) -> Value {
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
        "reviewStatus": d.review_status,
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
            .params("{path, name?, metric?: bool, template?: project folder} → {project}: create a new project folder; from a template it gets the template's standards and support files (rules, overrides, types), not its drawings")
            .enabled(always)
            .noundo(),
        CommandSpec::new("project.open", "Open Project", run_open)
            .files()
            .params("{path} → {project, drawings}: open a project folder (a shelved one is unpacked first); the project open before is saved and closed")
            .enabled(always)
            .noundo(),
        CommandSpec::new("project.close", "Close Project", run_close)
            .files()
            .params("{save?: bool (default true)} → {closed: [drawings], saved: [drawings]}: save and close the project's drawings and drop the project's state (its print set)")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.info", "Project Info", run_info)
            .files()
            .params("{} → {project}: active project metadata")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.drawings", "Project Drawings", run_drawings)
            .files()
            .params("{discipline?, status?, kind?} → {drawings, total}: list the drawings in the open project")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.activate", "Activate Drawing", run_activate)
            .files()
            .params("{name, save?: bool (default true)} → {activated, index, entities, shelved?}: open a drawing of the project into the session; the drawing active before is saved and closed")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.shelve", "Shelve Drawing", run_shelve)
            .files()
            .params("{save?: bool (default true)} → {shelved, saved}: save and close the active drawing of the project")
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
            .params("{path?: project folder (default: the open project)} → {path, beforeBytes, afterBytes, savedBytes, files}: drop the caches, pack everything else into one archive and leave a stub project.json; the project is closed first")
            .enabled(always)
            .noundo(),
        CommandSpec::new("project.restore_project", "Restore Project", run_restore_project)
            .files()
            .params("{path} → {project, files}: unpack a shelved project (every file as it was) and open it")
            .enabled(always)
            .noundo(),
        CommandSpec::new("project.disk", "Project Disk Use", run_disk)
            .files()
            .params("{} → {totalBytes, drawings: [{name, bytes}], caches}: disk use per drawing")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.standards.list", "Standards", run_standards_list)
            .files()
            .params("{domain?} → {standards: [{domain, key, value, source: company|project, overrides?}]}: the effective standards: the company library with this project's overrides")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.standards.set", "Set Standard", run_standards_set)
            .files()
            .params("{domain, key, value, source?} → {standard}: set this project's own value (it overrides the company's)")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.standards.remove", "Remove Standard", run_standards_remove)
            .files()
            .params("{domain, key} → {removed}: drop this project's override (the company value applies again)")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.standards.import", "Import Standards", run_standards_import)
            .files()
            .params("{from, domain?, keys?} → {imported}: copy standards from another project")
            .enabled(has_project)
            .noundo(),
        CommandSpec::new("project.import_manifest", "Import Manifest", run_import_manifest)
            .files()
            .params("{path} → {project, added, changed, removed, errors, files: {added, changed: [{path, fields}], removed, missing}, queued}: import/update a project from a folder + manifest.json (docs/project-manifest.md); only added and changed files are queued")
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

fn other(msg: impl Into<String>) -> crate::EngineError {
    crate::EngineError::Other(msg.into())
}

/// The folder name of a path (the project's name when none is given).
fn base_name(path: &str, fallback: &'static str) -> String {
    path.trim_end_matches('/').rsplit('/').find(|s| !s.is_empty()).unwrap_or(fallback).to_string()
}

// ---- which documents belong to the project ----

fn canon(path: &str) -> String {
    std::fs::canonicalize(path).ok().and_then(|p| p.to_str().map(str::to_string)).unwrap_or_else(|| path.to_string())
}

fn in_project(dir: &str, doc_path: &str) -> bool {
    let (d, p) = (canon(dir), canon(doc_path));
    p.starts_with(&format!("{}/", d.trim_end_matches('/')))
}

/// Save (when changed and `save`) and close the open drawing at `abs`. Returns whether it was open.
fn shelve_doc(s: &mut Session, abs: &str, save: bool) -> Result<(bool, bool)> {
    let Some(i) = super::file::already_open(s, abs) else { return Ok((false, false)) };
    let mut saved = false;
    if save && s.docs.get(i).is_some_and(|d| d.is_dirty()) {
        let keep = s.active;
        s.active = i;
        let r = s.execute("qsave", &json!({}));
        s.active = keep.min(s.docs.len().saturating_sub(1));
        r?;
        saved = true;
    }
    if let Some(i) = super::file::already_open(s, abs) {
        s.execute("close", &json!({ "index": i }))?;
    }
    Ok((true, saved))
}

/// Close the open project: its drawings are saved (when `save`) and closed, its print set and
/// framing dropped, its manifest written. What was open of it is returned (closed, saved titles).
pub(super) fn close_project_state(s: &mut Session, save: bool) -> Result<(Vec<String>, Vec<String>)> {
    let Some(ps) = s.project.as_ref() else { return Ok((Vec::new(), Vec::new())) };
    let dir = ps.manifest.dir.clone();
    let mut closed = Vec::new();
    let mut saved = Vec::new();
    while let Some((title, path)) =
        s.docs.iter().find_map(|d| d.path.as_deref().filter(|p| in_project(&dir, p)).map(|p| (d.title.clone(), p.to_string())))
    {
        let (_, was_saved) = shelve_doc(s, &path, save)?;
        if was_saved {
            saved.push(title.clone());
        }
        closed.push(title);
        // A drawing that could not be closed would loop for ever.
        if s.docs.iter().any(|d| d.path.as_deref() == Some(path.as_str())) {
            return Err(other(format!("cannot close `{path}`")));
        }
    }
    if let Some(ps) = s.project.as_mut() {
        for d in &mut ps.manifest.drawings {
            d.status = DrawingStatus::Shelved;
        }
        ps.active_drawing = None;
        if std::path::Path::new(&dir).join("project.json").is_file() {
            write_manifest(&ps.manifest)?;
        }
    }
    s.project = None;
    s.printset_project = None;
    s.framing = None;
    Ok((closed, saved))
}

/// Open review items of the active drawing, when it belongs to the open project.
pub(super) fn open_review_count(s: &mut Session) -> Option<usize> {
    let dir = s.project.as_ref()?.manifest.dir.clone();
    let path = s.state().ok()?.path.clone()?;
    if !in_project(&dir, &path) {
        return None;
    }
    super::agentready::open_review_count(s)
}

/// Make `m` the open project (the one before is closed first unless it is the same folder).
fn set_open(s: &mut Session, m: ProjectManifest) -> Result<()> {
    if s.project.as_ref().is_some_and(|ps| canon(&ps.manifest.dir) != canon(&m.dir)) {
        close_project_state(s, true)?;
    }
    hub::register(s, &m, true);
    s.project = Some(ProjectState { manifest: m, active_drawing: None });
    Ok(())
}

// ---- project.create ----

fn run_create(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.create";
    let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;
    let name = str_param(p, "name").map_or_else(|| base_name(path, "Project"), str::to_string);
    let template = str_param(p, "template");
    if let Some(t) = template {
        let t = t.trim_end_matches('/');
        if canon(t) == canon(path) {
            return Err(bad(cmd, "`template` must be another project"));
        }
        if read_manifest(t).is_ok_and(|m| m.shelved) {
            return Err(bad(cmd, "the template is shelved; restore it first (project.restore_project)"));
        }
    }
    if std::path::Path::new(path).join("project.json").exists() {
        return Err(bad(cmd, format!("`{path}` is already a project; use project.open")));
    }

    std::fs::create_dir_all(path).map_err(|e| other(format!("cannot create {path}: {e}")))?;

    let now = timestamp();
    let mut m = ProjectManifest { name, dir: path.to_string(), created: now.clone(), modified: now, ..Default::default() };
    if let Some(t) = template {
        let t = t.trim_end_matches('/');
        let drawings: Vec<String> = read_manifest(t)?.drawings.into_iter().map(|d| d.path).collect();
        m.standards = hub::apply_template(t, path, &drawings)?;
    }
    write_manifest(&m)?;
    set_open(s, m.clone())?;

    Ok(json!({ "project": manifest_json(&m) }))
}

// ---- project.open ----

fn run_open(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.open";
    let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?.trim_end_matches('/');

    let mut m = read_manifest(path)?;
    if m.shelved {
        hub::restore_folder(path)?;
        m = read_manifest(path)?;
    }
    if m.drawings.len() > MAX_DRAWINGS {
        return Err(other(format!("manifest has {} drawings (cap is {MAX_DRAWINGS})", m.drawings.len())));
    }

    let drawings: Vec<Value> = m.drawings.iter().map(drawing_json).collect();
    let info = manifest_json(&m);
    set_open(s, m)?;

    Ok(json!({
        "project": info,
        "drawings": drawings,
    }))
}

// ---- project.close ----

fn run_close(s: &mut Session, p: &Value) -> Result<Value> {
    require_project(s)?;
    let (closed, saved) = close_project_state(s, bool_or(p, "save", true))?;
    Ok(json!({ "closed": closed, "saved": saved }))
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
    info["queue"] = json!(ps.manifest.queue);
    Ok(json!({ "project": info }))
}

// ---- project.drawings ----

fn run_drawings(s: &mut Session, p: &Value) -> Result<Value> {
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
    let save = bool_or(p, "save", true);

    let ps = require_project(s)?;
    let idx = find_drawing(&ps.manifest, name).ok_or_else(|| bad(cmd, format!("drawing `{name}` not found in the project")))?;
    let d = ps.manifest.drawings.get(idx).ok_or_else(|| bad(cmd, "drawing index out of range"))?;
    let abs = drawing_abs_path(&ps.manifest, d);
    let prev = ps.active_drawing.and_then(|i| ps.manifest.drawings.get(i)).map(|d| (d.title.clone(), drawing_abs_path(&ps.manifest, d)));
    if !std::path::Path::new(&abs).is_file() {
        return Err(bad(cmd, format!("`{name}` has no file at {abs}")));
    }

    // Open the new drawing first: a drawing that cannot be read leaves the current one open.
    let opened = s.execute("open", &json!({ "path": abs })).map_err(|e| bad(cmd, format!("`{name}` cannot be opened as a drawing: {e}")))?;
    let mut shelved_name = None;
    if let Some((title, prev_abs)) = prev
        && super::file::already_open(s, &prev_abs) != super::file::already_open(s, &abs)
    {
        shelve_doc(s, &prev_abs, save)?;
        shelved_name = Some(title);
        // Closing the earlier drawing may have moved the new one: make it current again.
        if let Some(i) = super::file::already_open(s, &abs) {
            s.active = i;
        }
    }

    let ps = require_project_mut(s)?;
    for (i, d) in ps.manifest.drawings.iter_mut().enumerate() {
        d.status = if i == idx {
            DrawingStatus::Active
        } else if d.status == DrawingStatus::Active {
            DrawingStatus::Shelved
        } else {
            d.status.clone()
        };
    }
    ps.active_drawing = Some(idx);
    write_manifest(&ps.manifest)?;
    let activated = ps.manifest.drawings.get(idx).map(drawing_json).unwrap_or(Value::Null);

    let mut result = json!({ "activated": activated, "index": opened["index"], "entities": opened["entities"] });
    if result["entities"].is_null() {
        result["entities"] = json!(s.state().ok().map(|d| d.doc.entity_count()));
    }
    if let Some(name) = shelved_name {
        result["shelved"] = json!(name);
    }
    Ok(result)
}

// ---- project.shelve ----

fn run_shelve(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.shelve";
    let save = bool_or(p, "save", true);
    let ps = require_project(s)?;
    let idx = ps.active_drawing.ok_or_else(|| bad(cmd, "no drawing is active in the project"))?;
    let d = ps.manifest.drawings.get(idx).ok_or_else(|| bad(cmd, "no drawing is active in the project"))?;
    let (title, abs) = (d.title.clone(), drawing_abs_path(&ps.manifest, d));

    let (_, saved) = shelve_doc(s, &abs, save)?;

    let ps = require_project_mut(s)?;
    if let Some(d) = ps.manifest.drawings.get_mut(idx) {
        d.status = DrawingStatus::Shelved;
    }
    ps.active_drawing = None;
    write_manifest(&ps.manifest)?;

    Ok(json!({ "shelved": title, "saved": saved }))
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
        return Err(other(format!("project already has {MAX_DRAWINGS} drawings")));
    }

    // Make path relative to project dir if it's inside it.
    let rel = if path.starts_with(&ps.manifest.dir) { path.strip_prefix(&ps.manifest.dir).unwrap_or(path).trim_start_matches('/') } else { path };

    // Check for duplicates.
    if ps.manifest.drawings.iter().any(|d| d.path == rel) {
        return Err(bad(cmd, format!("`{rel}` is already in the project")));
    }

    // Read file size and hash if the file exists.
    let abs = drawing_abs_path(&ps.manifest, &ProjectDrawing { path: rel.to_string(), ..Default::default() });
    let (file_size, hash) = match std::fs::read(&abs) {
        Ok(data) => (data.len() as u64, content_hash(&data)),
        Err(_) => (0, String::new()),
    };

    let drawing = ProjectDrawing {
        path: rel.to_string(),
        title: title.to_string(),
        discipline: discipline.to_string(),
        content_hash: hash,
        kind: kind.to_string(),
        file_size,
        ..Default::default()
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
    forget_index(ps, idx);

    ps.manifest.modified = timestamp();
    write_manifest(&ps.manifest)?;

    Ok(json!({ "removed": removed.title }))
}

/// A drawing left the manifest at `idx`: keep `active_drawing` pointing at the same drawing.
fn forget_index(ps: &mut ProjectState, idx: usize) {
    match ps.active_drawing {
        Some(a) if a == idx => ps.active_drawing = None,
        Some(a) if a > idx => ps.active_drawing = Some(a - 1),
        _ => {}
    }
}

/// Bytes of the files under `path` (links are not followed).
pub(super) fn dir_size(path: &str) -> u64 {
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

// ---- project.shelve_project ----

fn run_shelve_project(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.shelve_project";
    let dir = match str_param(p, "path") {
        Some(d) => d.trim_end_matches('/').to_string(),
        None => require_project(s).map(|ps| ps.manifest.dir.clone()).map_err(|_| bad(cmd, "`path` is required when no project is open"))?,
    };
    // A project in use is saved and closed first, so what is packed is what was being worked on.
    if s.project.as_ref().is_some_and(|ps| canon(&ps.manifest.dir) == canon(&dir)) {
        close_project_state(s, true)?;
    }
    let (before, after, files) = hub::shelve_folder(&dir)?;
    if let Ok(m) = read_manifest(&dir) {
        hub::register(s, &m, false);
    }
    Ok(json!({
        "path": dir,
        "beforeBytes": before,
        "afterBytes": after,
        "savedBytes": before.saturating_sub(after),
        "files": files,
    }))
}

// ---- project.restore_project ----

fn run_restore_project(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.restore_project";
    let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?.trim_end_matches('/');

    let files = hub::restore_folder(path)?;
    let m = read_manifest(path)?;
    let info = manifest_json(&m);
    set_open(s, m)?;

    Ok(json!({ "project": info, "files": files }))
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

    let mut cache_bytes: u64 = 0;
    for d in &hub::CACHE_DIRS {
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
    let filtered = hub::effective_standards(s, &ps.manifest, str_param(p, "domain"));
    let total = filtered.len();
    Ok(json!({ "standards": filtered, "total": total }))
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

const MANIFEST_KINDS: [&str; 11] = ["drawing", "sheet", "spec", "submittal", "rfi", "asi", "addendum", "photo", "markup", "bulletin", "other"];
const SUBMITTAL_STATUSES: [&str; 4] = ["approved", "approved as noted", "revise and resubmit", "rejected"];
const RFI_STATUSES: [&str; 2] = ["open", "answered"];
/// Longest text field a manifest entry may carry.
const MAX_FIELD: usize = 4096;
/// Largest manifest.json read.
const MAX_MANIFEST_BYTES: u64 = 64 << 20;

/// A status as the manifest wrote it (any case, `_` or `-` between words) in its canonical
/// spelling for `kind`; `None` when the kind has no such status.
fn canonical_status(kind: &str, raw: &str) -> Option<&'static str> {
    let norm = raw.trim().to_ascii_lowercase().replace(['_', '-'], " ");
    let norm = norm.split_whitespace().collect::<Vec<_>>().join(" ");
    let list: &[&'static str] = match kind {
        "submittal" => &SUBMITTAL_STATUSES,
        "rfi" => &RFI_STATUSES,
        _ => &[],
    };
    list.iter().find(|c| **c == norm).copied()
}

fn read_capped(path: &str) -> Result<Vec<u8>> {
    let md = std::fs::metadata(path).map_err(|e| other(format!("cannot read {path}: {e}")))?;
    if md.len() > MAX_MANIFEST_BYTES {
        return Err(other(format!("{path} is {} bytes (the most read is {MAX_MANIFEST_BYTES})", md.len())));
    }
    std::fs::read(path).map_err(|e| other(format!("cannot read {path}: {e}")))
}

/// Validate a manifest.json schema (docs/project-manifest.md). Never panics on any input.
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
    if v.get("name").is_some_and(|n| !n.is_string()) {
        errors.push("`name` must be a string".into());
    }

    let Some(files) = v.get("files").and_then(Value::as_array) else {
        errors.push("`files` array is required".into());
        return errors;
    };
    if files.len() > MAX_DRAWINGS {
        errors.push(format!("`files` has {} entries (cap is {MAX_DRAWINGS})", files.len()));
        return errors;
    }
    let mut seen_paths = std::collections::HashSet::new();
    for (i, f) in files.iter().enumerate() {
        let Some(f) = f.as_object() else {
            errors.push(format!("files[{i}] must be an object"));
            continue;
        };
        match f.get("path").and_then(Value::as_str) {
            None | Some("") => errors.push(format!("files[{i}].path is required (a string)")),
            Some(path) => {
                if !hub::safe_rel(path) {
                    errors.push(format!(
                        "files[{i}].path `{}` must be a relative path inside the folder (no `..`, no leading `/`, no drive)",
                        cut(path)
                    ));
                } else if !seen_paths.insert(path.to_string()) {
                    errors.push(format!("duplicate path: `{}`", cut(path)));
                }
            }
        }
        for key in ["title", "discipline", "revision", "date", "kind", "status"] {
            match f.get(key) {
                None | Some(Value::Null) => {}
                Some(Value::String(t)) if t.len() > MAX_FIELD => errors.push(format!("files[{i}].{key} is longer than {MAX_FIELD} bytes")),
                Some(Value::String(_)) => {}
                Some(_) => errors.push(format!("files[{i}].{key} must be a string")),
            }
        }
        let kind = f.get("kind").and_then(Value::as_str).unwrap_or("drawing");
        if f.get("kind").is_some_and(Value::is_string) && !MANIFEST_KINDS.contains(&kind) {
            errors.push(format!("files[{i}].kind `{}` is not recognized", cut(kind)));
        }
        if let Some(status) = f.get("status").and_then(Value::as_str)
            && !status.trim().is_empty()
            && canonical_status(kind, status).is_none()
        {
            let allowed = match kind {
                "submittal" => SUBMITTAL_STATUSES.join(" / "),
                "rfi" => RFI_STATUSES.join(" / "),
                _ => String::new(),
            };
            errors.push(if allowed.is_empty() {
                format!("files[{i}].status is only used for submittals and RFIs (this entry is a `{kind}`)")
            } else {
                format!("files[{i}].status `{}` is not one of: {allowed}", cut(status))
            });
        }
        match f.get("links") {
            None | Some(Value::Null) => {}
            Some(Value::Object(o)) => {
                for (k, l) in o {
                    if !l.as_array().is_some_and(|a| a.iter().all(Value::is_string)) {
                        errors.push(format!("files[{i}].links.{} must be an array of strings", cut(k)));
                    }
                }
            }
            Some(_) => errors.push(format!("files[{i}].links must be an object of string arrays")),
        }
    }

    errors
}

/// A value for an error message, cut short.
fn cut(s: &str) -> String {
    s.chars().take(60).collect()
}

fn run_validate_manifest(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.validate_manifest";
    let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;
    let _ = s; // keep the signature, don't need session

    let data = read_capped(path)?;

    let errors = validate_manifest_data(&data);
    Ok(json!({
        "valid": errors.is_empty(),
        "errors": errors,
    }))
}

fn str_field(f: &Value, key: &str) -> String {
    f.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn run_import_manifest(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "project.import_manifest";
    let path = str_param(p, "path").ok_or_else(|| bad(cmd, "`path` is required"))?;

    // Read manifest.json from the given path.
    let manifest_path = if path.ends_with("manifest.json") { path.to_string() } else { format!("{}/manifest.json", path.trim_end_matches('/')) };

    let data = read_capped(&manifest_path)?;

    let errors = validate_manifest_data(&data);
    if !errors.is_empty() {
        return Ok(json!({
            "valid": false,
            "errors": errors,
        }));
    }

    let v: Value = serde_json::from_slice(&data).map_err(|e| other(format!("invalid manifest: {e}")))?;

    let source_dir = if path.ends_with("manifest.json") {
        path.rsplit_once('/').map_or(".", |(d, _)| d).to_string()
    } else {
        path.trim_end_matches('/').to_string()
    };

    let files = v.get("files").and_then(Value::as_array).cloned().unwrap_or_default();

    // With no project open: the folder's own project when it has one (a re-import), else a new one.
    if s.project.is_none() {
        let m = match read_manifest(&source_dir) {
            Ok(m) if !m.shelved => m,
            Ok(m) => return Err(bad(cmd, format!("project `{}` is shelved; restore it first (project.restore_project {{path}})", m.name))),
            Err(_) => {
                let name = v.get("name").and_then(Value::as_str).map_or_else(|| base_name(&source_dir, "Imported"), str::to_string);
                let now = timestamp();
                ProjectManifest { name, dir: source_dir.clone(), created: now.clone(), modified: now, ..Default::default() }
            }
        };
        hub::register(s, &m, true);
        s.project = Some(ProjectState { manifest: m, active_drawing: None });
    }

    let ps = require_project_mut(s)?;
    let mut added: Vec<String> = Vec::new();
    let mut changed: Vec<Value> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut import_errors: Vec<String> = Vec::new();
    let mut in_manifest: std::collections::HashSet<String> = std::collections::HashSet::new();

    for f in &files {
        // Validation made `path` a safe relative path.
        let Some(file_path) = f.get("path").and_then(Value::as_str) else { continue };
        in_manifest.insert(file_path.to_string());
        let abs = format!("{source_dir}/{file_path}");

        // A file named but not there yet is kept; it can arrive later.
        let (file_size, hash) = match std::fs::read(&abs) {
            Ok(data) => (data.len() as u64, content_hash(&data)),
            Err(_) => {
                missing.push(file_path.to_string());
                import_errors.push(format!("missing file: `{file_path}`"));
                (0, String::new())
            }
        };

        let kind = f.get("kind").and_then(Value::as_str).unwrap_or("drawing").to_string();
        let review_status = f.get("status").and_then(Value::as_str).and_then(|st| canonical_status(&kind, st)).unwrap_or("").to_string();
        let title = f.get("title").and_then(Value::as_str).map_or_else(|| base_name(file_path, "Untitled"), str::to_string);
        let mut links = BTreeMap::new();
        if let Some(link_obj) = f.get("links").and_then(Value::as_object) {
            for (k, v) in link_obj {
                if let Some(arr) = v.as_array() {
                    links.insert(k.clone(), arr.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>());
                }
            }
        }
        let fresh = ProjectDrawing {
            path: file_path.to_string(),
            title,
            discipline: str_field(f, "discipline"),
            status: DrawingStatus::Shelved,
            content_hash: hash,
            kind,
            revision: str_field(f, "revision"),
            date: str_field(f, "date"),
            links,
            notes: String::new(),
            file_size,
            review_status,
            from_manifest: true,
        };

        if let Some(existing) = ps.manifest.drawings.iter_mut().find(|d| d.path == file_path) {
            // What differs, by field: a status change alone (same bytes) counts.
            let mut fields: Vec<&str> = Vec::new();
            if !fresh.content_hash.is_empty() && existing.content_hash != fresh.content_hash {
                fields.push("content");
            }
            if existing.review_status != fresh.review_status {
                fields.push("status");
            }
            if existing.revision != fresh.revision {
                fields.push("revision");
            }
            if existing.date != fresh.date {
                fields.push("date");
            }
            if existing.discipline != fresh.discipline {
                fields.push("discipline");
            }
            if existing.kind != fresh.kind {
                fields.push("kind");
            }
            if f.get("title").is_some() && existing.title != fresh.title {
                fields.push("title");
            }
            if existing.links != fresh.links {
                fields.push("links");
            }
            existing.from_manifest = true;
            if fields.is_empty() {
                continue;
            }
            // Notes and the drawing's own status are the user's: only manifest fields move.
            if !fresh.content_hash.is_empty() {
                existing.content_hash = fresh.content_hash;
                existing.file_size = fresh.file_size;
            }
            existing.review_status = fresh.review_status;
            existing.revision = fresh.revision;
            existing.date = fresh.date;
            existing.discipline = fresh.discipline;
            existing.kind = fresh.kind;
            existing.links = fresh.links;
            if f.get("title").is_some() {
                existing.title = fresh.title;
            }
            changed.push(json!({ "path": file_path, "fields": fields }));
        } else {
            if ps.manifest.drawings.len() >= MAX_DRAWINGS {
                import_errors.push(format!("cap of {MAX_DRAWINGS} drawings reached; skipping `{file_path}`"));
                continue;
            }
            ps.manifest.drawings.push(fresh);
            added.push(file_path.to_string());
        }
    }

    // Entries an earlier import made that the manifest no longer lists.
    let gone: Vec<usize> =
        ps.manifest.drawings.iter().enumerate().filter(|(_, d)| d.from_manifest && !in_manifest.contains(&d.path)).map(|(i, _)| i).collect();
    let mut removed: Vec<String> = Vec::new();
    for &i in gone.iter().rev() {
        let d = ps.manifest.drawings.remove(i);
        forget_index(ps, i);
        removed.push(d.path);
    }
    removed.reverse();

    // Only what changed is queued: it replaces older entries for the same file, and a removed
    // file leaves the queue.
    let touched: Vec<String> = added.iter().cloned().chain(changed.iter().filter_map(|c| c["path"].as_str().map(str::to_string))).collect();
    ps.manifest.queue.retain(|q| !removed.contains(q) && !touched.contains(q));
    ps.manifest.queue.extend(touched.iter().cloned());
    ps.manifest.queue.truncate(MAX_DRAWINGS);

    ps.manifest.modified = timestamp();
    write_manifest(&ps.manifest)?;

    Ok(json!({
        "project": manifest_json(&ps.manifest),
        "added": added.len(),
        "changed": changed.len(),
        "removed": removed.len(),
        "errors": import_errors,
        "files": { "added": added, "changed": changed, "removed": removed, "missing": missing },
        "queued": touched,
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

    /// A session whose registry and company library live in a folder of the test's own.
    fn session() -> Session {
        let mut s = Session::empty();
        s.project_config = Some(std::path::PathBuf::from(format!("/tmp/cadaddy_project_cfg_{}", std::process::id())));
        s
    }

    fn cleanup(dir: &str) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn create_and_open_project() {
        let dir = temp_dir("create_open");
        let mut s = session();

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
        let mut s = session();

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
        let r = s.execute("project.drawings", &json!({}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 2);

        // Filter by discipline.
        let r = s.execute("project.drawings", &json!({"discipline": "Electrical"}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 1);

        // Duplicate add fails.
        let r = s.execute("project.add", &json!({"path": "plan.dxf"}));
        assert!(r.is_err());

        // Remove one.
        let r = s.execute("project.remove", &json!({"name": "Floor Plan"}));
        assert!(r.is_ok());

        let r = s.execute("project.drawings", &json!({}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 1);

        cleanup(&dir);
    }

    #[test]
    fn activate_of_a_file_that_is_not_a_drawing_is_refused_and_changes_nothing() {
        // The real open/close of drawings is tested with .cdy files in tests/projects.rs.
        let dir = temp_dir("activate_refused");
        let mut s = session();

        s.execute("project.create", &json!({"path": &dir})).unwrap();
        fs::write(format!("{dir}/a.dxf"), b"not a drawing").unwrap();
        s.execute("project.add", &json!({"path": "a.dxf", "title": "Drawing A"})).unwrap();

        let e = s.execute("project.activate", &json!({"name": "Drawing A"})).unwrap_err().to_string();
        assert!(e.contains("Drawing A"), "{e}");
        assert_eq!(s.project.as_ref().map(|p| p.active_drawing), Some(None));
        assert!(s.docs.is_empty());
        let e = s.execute("project.shelve", &json!({})).unwrap_err().to_string();
        assert!(e.contains("no drawing is active"), "{e}");
        let e = s.execute("project.activate", &json!({"name": "nope"})).unwrap_err().to_string();
        assert!(e.contains("not found"), "{e}");
        let e = s.execute("project.activate", &json!({})).unwrap_err().to_string();
        assert!(e.contains("`name` is required"), "{e}");

        cleanup(&dir);
    }

    #[test]
    fn two_projects_stay_isolated() {
        let dir_a = temp_dir("iso_a");
        let dir_b = temp_dir("iso_b");
        let mut s = session();

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
        let r = s.execute("project.drawings", &json!({}));
        let v = r.unwrap_or(Value::Null);
        assert_eq!(v["total"], 1);
        assert_eq!(v["drawings"][0]["title"], "Floor");
        s.execute("project.close", &json!({})).ok();

        // Open B — should not see A's drawings.
        s.execute("project.open", &json!({"path": &dir_b})).ok();
        let r = s.execute("project.drawings", &json!({}));
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
        let mut s = session();

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
        let mut s = session();

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
        let mut s = session();

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
        let mut s = session();

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
        let mut s = session();

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
        let mut s = session();
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
        let mut s = session();
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
        let mut s = session();
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
        let mut s = session();

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
        let mut s = session();

        // All project commands that require an open project should fail with a clear message.
        let cmds = [
            ("project.drawings", json!({})),
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
        let mut s = session();
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
