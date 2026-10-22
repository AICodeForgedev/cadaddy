//! Print-set project commands: open a folder of sheets as a project, run the classification
//! pipeline, show the dashboard, and re-import revised sets.
//!
//! Delegates to [`cadaddy_mepfix::printset`] for all project operations (create, classify,
//! reimport, dashboard). The engine commands are thin wrappers that manage the global project
//! state and convert between engine types and mepfix types.
//!
//! Commands:
//! - `printset.open`: create or load a project from a folder of sheets.
//! - `printset.classify`: classify sheets by discipline, type, level, scale.
//! - `printset.dashboard`: show what has been modelled, confidence, gaps, RFIs.
//! - `printset.sheets`: list the project's sheets with their classification.
//! - `printset.reimport`: re-import a revised set, only reprocessing changed sheets.
//! - `printset.save`: save the project state to disk.

use cadaddy_mepfix::printset::{self as mep_printset, PrintSetProject};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::{CommandSpec, always, str_param};
use crate::{EngineError, Result, Session};

// ─── Global project state ────────────────────────────────────────────────────

static PROJECT: std::sync::Mutex<Option<PrintSetProject>> = std::sync::Mutex::new(None);

fn get_project() -> Result<PrintSetProject> {
    PROJECT
        .lock()
        .map_err(|_| EngineError::Other("project lock poisoned".into()))?
        .clone()
        .ok_or_else(|| EngineError::Other("no print-set project open; use printset.open first".into()))
}

/// Access the current printset project from other modules (readers).
pub(crate) fn get_project_handle() -> Result<PrintSetProject> {
    get_project()
}

fn set_project(p: PrintSetProject) -> Result<()> {
    *PROJECT.lock().map_err(|_| EngineError::Other("project lock poisoned".into()))? = Some(p);
    Ok(())
}

// ─── Serialization helpers ──────────────────────────────────────────────────

fn save_project(project: &PrintSetProject, path: &Path) -> std::result::Result<(), String> {
    let json = serde_json::to_vec_pretty(project).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("{}: {e}", path.display()))
}

fn load_project(path: &Path) -> std::result::Result<PrintSetProject, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&data).map_err(|e| format!("{}: {e}", path.display()))
}

// ─── Command specs ───────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("printset.open", "Open Print Set", run_open)
            .files()
            .params("{path: folder or .printset.json, name?} → {name, sheets, revision, dashboard}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.classify", "Classify Print Set", run_classify)
            .files()
            .params("{} → {classified, total, elapsed_ms}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.dashboard", "Print Set Dashboard", run_dashboard).files().params("{} → {dashboard}").enabled(always).noundo(),
        CommandSpec::new("printset.sheets", "Print Set Sheets", run_sheets)
            .files()
            .params("{discipline?, level?} → {sheets, total}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.reimport", "Re-import Revised Set", run_reimport)
            .files()
            .params("{path: folder, revision?: label} → {delta, sheets}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.save", "Save Print Set", run_save).files().params("{path?} → {saved}").enabled(always).noundo(),
    ]
}

// ─── printset.open ───────────────────────────────────────────────────────────

fn run_open(_s: &mut Session, p: &Value) -> Result<Value> {
    let path_str = str_param(p, "path").ok_or_else(|| EngineError::Other("path is required".into()))?;
    let path = PathBuf::from(path_str);

    let project = if path.extension().and_then(|e| e.to_str()) == Some("json") && path.is_file() {
        load_project(&path).map_err(EngineError::Other)?
    } else if path.is_dir() {
        let name = str_param(p, "name").unwrap_or_else(|| path.file_name().and_then(|n| n.to_str()).unwrap_or("Print Set"));
        mep_printset::create_project(name, &path).map_err(EngineError::Other)?
    } else {
        return Err(EngineError::Other(format!("{path_str} is not a folder or .json project file")));
    };

    let result = json!({
        "name": project.name,
        "sheets": project.sheets.len(),
        "revision": project.revision,
        "dashboard": project.dashboard,
    });
    set_project(project)?;
    Ok(result)
}

// ─── printset.classify ───────────────────────────────────────────────────────

fn run_classify(_s: &mut Session, _p: &Value) -> Result<Value> {
    let mut project = get_project()?;
    let cancel = AtomicBool::new(false);
    let mut noop = |_: &mep_printset::Progress| {};
    let rec = mep_printset::run_classify(&mut project, None, &cancel, &mut noop);

    let result = json!({
        "classified": rec.sheets_processed,
        "total": rec.sheets_total,
        "elapsed_ms": rec.elapsed_ms,
    });
    set_project(project)?;
    Ok(result)
}

// ─── printset.dashboard ──────────────────────────────────────────────────────

fn run_dashboard(_s: &mut Session, _p: &Value) -> Result<Value> {
    let project = get_project()?;
    Ok(json!({
        "name": project.name,
        "revision": project.revision,
        "sheets": project.sheets.len(),
        "dashboard": project.dashboard,
    }))
}

// ─── printset.sheets ─────────────────────────────────────────────────────────

fn run_sheets(_s: &mut Session, p: &Value) -> Result<Value> {
    let project = get_project()?;
    let filter_discipline = str_param(p, "discipline");
    let filter_level = str_param(p, "level");

    let sheets: Vec<Value> = project
        .sheets
        .iter()
        .filter(|s| filter_discipline.is_none_or(|d| s.discipline == d))
        .filter(|s| filter_level.is_none_or(|l| s.level == l))
        .map(|s| {
            json!({
                "number": s.number, "title": s.title, "discipline": s.discipline,
                "type": s.sheet_type, "level": s.level, "scale": s.scale_text,
                "file": s.file, "dirty": s.dirty,
                "stages": s.stage_results.keys().collect::<Vec<_>>(),
            })
        })
        .collect();
    let total = sheets.len();
    Ok(json!({ "sheets": sheets, "total": total }))
}

// ─── printset.reimport ───────────────────────────────────────────────────────

fn run_reimport(_s: &mut Session, p: &Value) -> Result<Value> {
    let path_str = str_param(p, "path").ok_or_else(|| EngineError::Other("path is required".into()))?;
    let dir = PathBuf::from(path_str);
    if !dir.is_dir() {
        return Err(EngineError::Other(format!("{path_str} is not a directory")));
    }
    let label = str_param(p, "revision").unwrap_or("revised");
    let files = mep_printset::discover_sheets(&dir).map_err(EngineError::Other)?;
    let mut project = get_project()?;
    let delta = mep_printset::apply_reimport(&mut project, &files, label).map_err(EngineError::Other)?;

    let result = json!({
        "delta": { "added": delta.added, "changed": delta.changed, "removed": delta.removed, "unchanged": delta.unchanged },
        "revision": label, "sheets": project.sheets.len(),
    });
    set_project(project)?;
    Ok(result)
}

// ─── printset.save ───────────────────────────────────────────────────────────

fn run_save(_s: &mut Session, p: &Value) -> Result<Value> {
    let project = get_project()?;
    let path = str_param(p, "path").map(PathBuf::from).unwrap_or_else(|| {
        let dir = PathBuf::from(&project.source_dir);
        dir.join(format!("{}.printset.json", project.name.replace(' ', "_")))
    });
    save_project(&project, &path).map_err(EngineError::Other)?;
    Ok(json!({ "saved": path.display().to_string() }))
}
