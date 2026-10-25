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

use cadaddy_mepfix::printset::{self as mep_printset, PrintSetProject};
use serde_json::{Value, json};
use std::path::PathBuf;
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
            .params("{path: folder or .printset.json, name?} → {name, sheets, revision, dashboard}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.classify", "Classify Print Set", run_classify)
            .files()
            .params("{} → {classified, total, elapsed_ms}")
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
            .params("{discipline?, level?} → {sheets, total}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.reimport", "Re-import Revised Set", run_reimport)
            .files()
            .params("{path: folder, revision?: label, full_set?: bool} → {delta, sheets}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.save", "Save Print Set", run_save).files().params("{path?} → {saved}").enabled(always).noundo(),
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
    } else {
        return Err(EngineError::Other(format!("{path_str} is not a folder or .json project file")));
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

fn run_classify(s: &mut Session, _p: &Value) -> Result<Value> {
    let project = get_project_mut(s)?;
    let cancel = AtomicBool::new(false);
    let mut noop = |_: &mep_printset::Progress| {};
    let rec = mep_printset::run_classify(project, None, &cancel, &mut noop);

    Ok(json!({
        "classified": rec.sheets_processed,
        "total": rec.sheets_total,
        "elapsed_ms": rec.elapsed_ms,
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

    let sheets: Vec<Value> = project
        .sheets
        .iter()
        .filter(|sh| filter_discipline.is_none_or(|d| sh.discipline == d))
        .filter(|sh| filter_level.is_none_or(|l| sh.level == l))
        .map(|sh| {
            json!({
                "number": sh.number, "title": sh.title, "discipline": sh.discipline,
                "type": sh.sheet_type, "level": sh.level, "scale": sh.scale_text,
                "file": sh.file, "page": sh.page, "dirty": sh.dirty,
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
