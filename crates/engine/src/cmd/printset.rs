//! Print-set project commands: open a folder of sheets as a project, run the classification
//! and stage pipeline, show the dashboard, and re-import revised sets.
//!
//! Delegates to [`cadaddy_mepfix::printset`] for all project operations (create, classify,
//! reimport, dashboard). Project state is owned by the [`Session`] (not a process-global static),
//! so two sessions can each have their own project open.
//!
//! Commands:
//! - `printset.open`: create or load a project from a folder of sheets.
//! - `printset.classify`: classify sheets by discipline, type, level, scale.
//! - `printset.run`: run recognition stages on the project's sheets.
//! - `printset.dashboard`: show what has been modelled, confidence, gaps, RFIs.
//! - `printset.sheets`: list the project's sheets with their classification.
//! - `printset.reimport`: re-import a revised set, only reprocessing changed sheets.
//! - `printset.save`: save the project state to disk.
//! - `printset.keep`: keep the whole job (print set, drawing with its spec, model and job state)
//!   in the print folder's `.cadaddy` folder, and note it as the last print set.
//! - `printset.resume`: open a kept job again (a folder's, else the last one kept): a new
//!   session picks up where the last one stopped.

use cadaddy_mepfix::printset::{self as mep_printset, PrintSetProject};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::{CommandSpec, always, bool_or, str_param};
use crate::{EngineError, Result, Session};

// ─── Per-session project access ─────────────────────────────────────────────

fn get_project(s: &Session) -> Result<&PrintSetProject> {
    s.printset_project.as_deref().ok_or_else(|| EngineError::Other("no print-set project open; use printset.open first".into()))
}

fn get_project_mut(s: &mut Session) -> Result<&mut PrintSetProject> {
    s.printset_project.as_deref_mut().ok_or_else(|| EngineError::Other("no print-set project open; use printset.open first".into()))
}

/// Access the current printset project from other modules (readers).
pub(crate) fn get_project_handle(s: &Session) -> Result<PrintSetProject> {
    get_project(s).cloned()
}

fn set_project(s: &mut Session, p: PrintSetProject) {
    s.printset_project = Some(Box::new(p));
}

// ─── Command specs ───────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("printset.open", "Open Print Set", run_open)
            .files()
            .params("{path: folder, one PDF (its pages only) or .printset.json, name?} → {name, sheets, revision, dashboard}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.classify", "Classify Print Set", run_classify)
            .files()
            .params("{} → {classified: pages read, total: sheets in the index, pages, elapsed_ms, sheets: [{number, title, discipline, type, level, levels, level_source, sources: [file#page], superseded?, issued?}], levels: [{name, sheets, elevation_ft?, height_ft?}], unidentified, plans_without_level}: one entry per real sheet (copies in several files merged by number, the newer revision date current), from each page's title block")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.run", "Run Print Set Stages", run_stages)
            .files()
            .params("{stages?: [name], resume?: bool, force?: bool (run even when the import check failed)} → {stages_run, total_items, elapsed_ms}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.dashboard", "Print Set Dashboard", run_dashboard).files().params("{} → {dashboard}").enabled(always).noundo(),
        CommandSpec::new("printset.sheets", "Print Set Sheets", run_sheets)
            .files()
            .params("{discipline?, level?, current?: bool (leave out sheets a later copy supersedes)} → {sheets: [{number, title, discipline, type, level, levels, scale, scale_factor, file, page, dirty, superseded_by, stages}], total}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.reimport", "Re-import Revised Set", run_reimport)
            .files()
            .params("{path: folder, revision?: label, full_set?: bool} → {delta, sheets}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.save", "Save Print Set", run_save).files().params("{path?} → {saved}").enabled(always).noundo(),
        CommandSpec::new("printset.keep", "Keep Print Set Job", run_keep)
            .files()
            .params("{} → {kept: folder, sheets}: the print set and the drawing (its building spec, model, review and job state) kept in the print folder's .cadaddy folder; the next session's printset.resume {} opens it")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.resume", "Resume Print Set Job", run_resume)
            .files()
            .params("{path?: print folder (default: the last one kept), auto?: bool (only when no print set is open and the drawing is empty)} → {resumed, name?, sheets?, folder?, why?}: the kept print set and its drawing opened again")
            .enabled(always)
            .noundo(),
    ]
}

// ─── printset.open ───────────────────────────────────────────────────────────

fn run_open(s: &mut Session, p: &Value) -> Result<Value> {
    let path_str = str_param(p, "path").ok_or_else(|| EngineError::Other("path is required".into()))?;
    let path = PathBuf::from(path_str);

    let project = if path.extension().and_then(|e| e.to_str()) == Some("json") && path.is_file() {
        mep_printset::load_project(&path).map_err(EngineError::Other)?
    } else if path.is_dir() {
        let name = str_param(p, "name").unwrap_or_else(|| path.file_name().and_then(|n| n.to_str()).unwrap_or("Print Set"));
        mep_printset::create_project(name, &path).map_err(EngineError::Other)?
    } else if path.is_file() && path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        // One PDF (often a merged set): a project of that file alone, not of its whole folder.
        let file = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let stem = path.file_stem().and_then(|n| n.to_str()).unwrap_or("Print Set");
        let name = str_param(p, "name").unwrap_or(stem);
        mep_printset::create_project_from_files(name, &dir, &[(file, path.clone())])
    } else {
        return Err(EngineError::Other(format!("{path_str} is not a folder, a PDF or a .json project file")));
    };

    let result = json!({
        "name": project.name,
        "sheets": project.sheets.len(),
        "revision": project.revision,
        "dashboard": project.dashboard,
    });
    set_project(s, project);
    Ok(result)
}

// ─── printset.classify ───────────────────────────────────────────────────────

/// Classify reads every sheet's own print (each page of a merged set): the title block's number,
/// title and discipline, never the file name. The result is the set's sheet index.
fn run_classify(s: &mut Session, _p: &Value) -> Result<Value> {
    let project = get_project_mut(s)?;
    let cancel = AtomicBool::new(false);
    let mut noop = |_: &mep_printset::Progress| {};
    let dir = PathBuf::from(&project.source_dir);
    let rec = if !project.source_dir.is_empty() && dir.is_dir() {
        let loader = mep_printset::PrintLoader::new(&dir);
        mep_printset::run_classify_prints(project, &|e| loader.load(e), &cancel, &mut noop)
    } else {
        // A project made without files (from a saved truth): classify what it already holds.
        mep_printset::run_classify(project, None, &cancel, &mut noop)
    };
    let index = mep_printset::sheet_index(project);
    Ok(json!({
        "classified": rec.sheets_processed,
        "total": index.sheets.len(),
        "pages": rec.sheets_total,
        "elapsed_ms": rec.elapsed_ms,
        "sheets": index.sheets,
        "levels": index.levels,
        "unidentified": index.unidentified,
        "plans_without_level": index.plans_without_level,
        "level_warnings": index.level_warnings,
    }))
}

// ─── printset.run ────────────────────────────────────────────────────────────

fn run_stages(s: &mut Session, p: &Value) -> Result<Value> {
    // Gate: refuse to run stages if the current drawing's import check failed, unless force.
    let force = bool_or(p, "force", false);
    if !force
        && let Ok(d) = s.doc()
        && !super::import_check::passes(d)
    {
        let fails = super::import_check::failed_checks(d);
        let names: Vec<&str> = fails.iter().map(|(id, _)| id.as_str()).collect();
        return Err(EngineError::Other(format!(
            "printset.run: import check failed ({}); fix the import or pass `force: true` to override",
            names.join(", ")
        )));
    }
    let resume = bool_or(p, "resume", false);

    // Parse optional stage filter.
    let stage_filter: Option<Vec<mep_printset::Stage>> = p.get("stages").and_then(|v| v.as_array()).map(|arr| {
        arr.iter().filter_map(|v| v.as_str()).filter_map(|name| mep_printset::Stage::all().iter().find(|s| s.name() == name).copied()).collect()
    });

    let stages = stage_filter.as_deref().unwrap_or(&mep_printset::Stage::all()[1..]);
    let project = get_project_mut(s)?;
    let cancel = AtomicBool::new(false);
    let mut noop = |_: &mep_printset::Progress| {};

    let records = mep_printset::run_stages(project, stages, &mut mep_printset::null_stage_runner, &cancel, &mut noop, resume);

    let total_items: usize = records.iter().map(|r| r.items_found).sum();
    let elapsed_ms: u64 = records.iter().map(|r| r.elapsed_ms).sum();

    Ok(json!({
        "stages_run": records.len(),
        "total_items": total_items,
        "elapsed_ms": elapsed_ms,
        "stages": records.iter().map(|r| json!({
            "stage": r.stage,
            "status": r.status,
            "sheets_processed": r.sheets_processed,
            "items_found": r.items_found,
        })).collect::<Vec<_>>(),
    }))
}

// ─── printset.dashboard ──────────────────────────────────────────────────────

fn run_dashboard(s: &mut Session, _p: &Value) -> Result<Value> {
    let project = get_project(s)?;
    Ok(json!({
        "name": project.name,
        "revision": project.revision,
        "sheets": project.sheets.len(),
        "dashboard": project.dashboard,
    }))
}

// ─── printset.sheets ─────────────────────────────────────────────────────────

fn run_sheets(s: &mut Session, p: &Value) -> Result<Value> {
    let project = get_project(s)?;
    let filter_discipline = str_param(p, "discipline");
    let filter_level = str_param(p, "level");
    let current = bool_or(p, "current", false);

    let sheets: Vec<Value> = project
        .sheets
        .iter()
        .filter(|sh| filter_discipline.is_none_or(|d| sh.discipline == d))
        .filter(|sh| filter_level.is_none_or(|l| sh.level == l))
        .filter(|sh| !current || sh.superseded_by.is_empty())
        .map(|sh| {
            json!({
                "number": sh.number, "title": sh.title, "discipline": sh.discipline,
                "type": sh.sheet_type, "level": sh.level, "levels": sh.levels, "scale": sh.scale_text,
                "scale_factor": sh.scale, "file": sh.file, "page": sh.page, "dirty": sh.dirty,
                "superseded_by": sh.superseded_by,
                "stages": sh.stage_results.keys().collect::<Vec<_>>(),
            })
        })
        .collect();
    let total = sheets.len();
    Ok(json!({ "sheets": sheets, "total": total }))
}

// ─── printset.reimport ───────────────────────────────────────────────────────

fn run_reimport(s: &mut Session, p: &Value) -> Result<Value> {
    let path_str = str_param(p, "path").ok_or_else(|| EngineError::Other("path is required".into()))?;
    let dir = PathBuf::from(path_str);
    if !dir.is_dir() {
        return Err(EngineError::Other(format!("{path_str} is not a directory")));
    }
    let label = str_param(p, "revision").unwrap_or("revised");
    let full_set = bool_or(p, "full_set", false);
    let files = mep_printset::discover_sheets(&dir).map_err(EngineError::Other)?;
    let project = get_project_mut(s)?;
    let delta = mep_printset::apply_reimport(project, &files, label, full_set).map_err(EngineError::Other)?;

    Ok(json!({
        "delta": { "added": delta.added, "changed": delta.changed, "removed": delta.removed, "unchanged": delta.unchanged },
        "revision": label, "sheets": project.sheets.len(),
    }))
}

// ─── printset.save ───────────────────────────────────────────────────────────

fn run_save(s: &mut Session, p: &Value) -> Result<Value> {
    let project = get_project(s)?;
    let path = str_param(p, "path").map(PathBuf::from).unwrap_or_else(|| {
        let dir = PathBuf::from(&project.source_dir);
        dir.join(format!("{}.printset.json", project.name.replace(' ', "_")))
    });
    // Sanitize: don't allow path separators in the name portion.
    let name_part = path.file_name().and_then(|n| n.to_str()).unwrap_or("project.json");
    if name_part.contains("..") {
        return Err(EngineError::Other("path must not contain '..'".into()));
    }
    mep_printset::save_project(project, &path).map_err(EngineError::Other)?;
    Ok(json!({ "saved": path.display().to_string() }))
}

// ─── printset.keep / printset.resume ─────────────────────────────────────────

/// The folder in a print folder where its job is kept (sheet discovery skips dot folders).
const KEEP_DIR: &str = ".cadaddy";
const KEEP_SET: &str = "printset.json";
const KEEP_DRAWING: &str = "project.cdy";
/// A kept drawing larger than this isn't read back (hostile files).
const KEEP_MAX_BYTES: u64 = 2_000_000_000;

/// Where the last print set kept is noted.
fn pointer_file(s: &Session) -> Option<PathBuf> {
    s.printset_pointer.clone().or_else(|| crate::control_auth::config_dir().map(|d| d.join("last-printset.txt")))
}

/// Written aside, then moved over the last copy: a crash mid-write keeps the last one.
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    std::fs::write(&part, bytes)?;
    std::fs::rename(&part, path)
}

fn run_keep(s: &mut Session, _p: &Value) -> Result<Value> {
    let project = get_project(s)?;
    if project.source_dir.is_empty() {
        return Err(EngineError::Other("this print set has no folder to keep its job in".into()));
    }
    let folder = PathBuf::from(&project.source_dir);
    let dir = folder.join(KEEP_DIR);
    let hooks = super::file::io().ok_or_else(|| EngineError::Other("no file writer is installed".into()))?;
    let set = serde_json::to_vec(project).map_err(|e| EngineError::Other(e.to_string()))?;
    let sheets = project.sheets.len();
    let drawing = (hooks.write)(s.doc()?, KEEP_DRAWING).map_err(EngineError::Other)?;
    let fail = |e: std::io::Error| EngineError::Other(format!("{}: {e}", dir.display()));
    std::fs::create_dir_all(&dir).map_err(fail)?;
    write_atomic(&dir.join(KEEP_DRAWING), &drawing).map_err(fail)?;
    write_atomic(&dir.join(KEEP_SET), &set).map_err(fail)?;
    // The pointer is a convenience: a settings folder that can't be written isn't an error.
    if let Some(pointer) = pointer_file(s) {
        if let Some(parent) = pointer.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = write_atomic(&pointer, folder.display().to_string().as_bytes());
    }
    Ok(json!({"kept": dir.display().to_string(), "sheets": sheets}))
}

/// Whether the active drawing holds nothing: no objects, building spec or agent state.
fn drawing_is_empty(s: &Session) -> bool {
    s.doc()
        .is_ok_and(|d| d.model.is_empty() && d.building_spec.is_none() && d.agent.as_deref().is_none_or(|a| a.trim().is_empty() || a.trim() == "{}"))
}

fn run_resume(s: &mut Session, p: &Value) -> Result<Value> {
    if bool_or(p, "auto", false) && (s.printset_project.is_some() || !drawing_is_empty(s)) {
        return Ok(json!({"resumed": false, "why": "a print set or a drawing with work is open already"}));
    }
    let folder = match str_param(p, "path") {
        Some(path) => PathBuf::from(path),
        None => match pointer_file(s).and_then(|f| std::fs::read_to_string(f).ok()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
            Some(last) => PathBuf::from(last),
            None => {
                return Ok(json!({"resumed": false, "why": "no job kept yet: open the prints with printset.open {path} (bim_job import {path})"}));
            }
        },
    };
    let dir = if folder.file_name().is_some_and(|n| n == KEEP_DIR) { folder.clone() } else { folder.join(KEEP_DIR) };
    let (set_path, drawing_path) = (dir.join(KEEP_SET), dir.join(KEEP_DRAWING));
    if !set_path.is_file() || !drawing_path.is_file() {
        return Ok(json!({"resumed": false, "why": format!("no job is kept in {}", folder.display())}));
    }
    let hooks = super::file::io().ok_or_else(|| EngineError::Other("no file reader is installed".into()))?;
    let fail = |e: String| EngineError::Other(format!("the kept job in {} can't be read: {e}", dir.display()));
    if std::fs::metadata(&drawing_path).map_or(0, |m| m.len()) > KEEP_MAX_BYTES {
        return Err(fail("the drawing is too large".into()));
    }
    let project = mep_printset::load_project(&set_path).map_err(fail)?;
    let bytes = std::fs::read(&drawing_path).map_err(|e| fail(e.to_string()))?;
    let drawing = (hooks.read)(&bytes, KEEP_DRAWING).map_err(fail)?;
    let result = json!({"resumed": true, "name": project.name, "sheets": project.sheets.len(), "folder": project.source_dir});
    // An empty drawing is replaced; one with work stays open beside the kept one.
    if drawing_is_empty(s) && s.state().is_ok_and(|st| st.path.is_none()) {
        *s.doc_mut()? = drawing;
        if let Ok(st) = s.state_mut() {
            st.path = Some(drawing_path.display().to_string());
            st.title = project.name.clone();
        }
    } else {
        s.open_drawing(drawing, &project.name, Some(drawing_path.display().to_string()));
    }
    set_project(s, project);
    Ok(result)
}
