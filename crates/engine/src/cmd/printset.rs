//! Print-set project commands: open a folder of sheets as a project, run the classification
//! pipeline, show the dashboard, and re-import revised sets.
//!
//! The project state is a JSON file (`*.printset.json`) stored alongside the sheet files.
//! The heavy recognition pipeline (MEP readers, height assignment, clash detection) runs in
//! the `cadaddy-mepfix` crate's `printset` module; these engine commands manage the project
//! file, run the classify stage (which only needs the DXF reader from `cadaddy-dxf`), and
//! present the dashboard.
//!
//! Commands:
//! - `printset.open`: create or load a project from a folder of sheets.
//! - `printset.classify`: classify sheets by discipline, type, level, scale.
//! - `printset.dashboard`: show what has been modelled, confidence, gaps, RFIs.
//! - `printset.sheets`: list the project's sheets with their classification.
//! - `printset.reimport`: re-import a revised set, only reprocessing changed sheets.
//! - `printset.save`: save the project state to disk.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{CommandSpec, always, str_param};
use crate::{EngineError, Result, Session};

/// Maximum sheets in a single project (hostile input guard).
const MAX_SHEETS: usize = 2000;

/// Maximum file size for a single sheet, bytes.
const MAX_FILE_BYTES: u64 = 500_000_000;

// ─── Project model (self-contained, serialized as JSON) ──────────────────────

const SCHEMA: &str = "cadaddy-printset/1";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct PrintSetProject {
    schema: String,
    name: String,
    source_dir: String,
    sheets: Vec<SheetEntry>,
    stages: Vec<StageRecord>,
    revision: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    revisions: Vec<RevisionRecord>,
    dashboard: Dashboard,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct SheetEntry {
    number: String,
    title: String,
    file: String,
    discipline: String,
    sheet_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    level: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scale_text: Option<String>,
    scale: f64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    stage_results: BTreeMap<String, SheetStageResult>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    content_hash: String,
    #[serde(default)]
    dirty: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct SheetStageResult {
    stage: String,
    #[serde(default)]
    status: String,
    items_found: usize,
    confidence: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    findings: Vec<Finding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Finding {
    kind: String,
    description: String,
    severity: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    at_ft: Option<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    elements: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct StageRecord {
    stage: String,
    status: String,
    sheets_processed: usize,
    sheets_total: usize,
    items_found: usize,
    findings: usize,
    elapsed_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Dashboard {
    total_sheets: usize,
    classified_sheets: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    levels: Vec<LevelSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    disciplines: Vec<DisciplineSummary>,
    total_items: usize,
    mean_confidence: f64,
    open_gaps: usize,
    open_rfis: usize,
    total_clashes: usize,
    stages_completed: usize,
    stages_total: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct LevelSummary {
    level: String,
    sheets: usize,
    items: usize,
    confidence: f64,
    gaps: usize,
    rfis: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct DisciplineSummary {
    discipline: String,
    sheets: usize,
    items: usize,
    confidence: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    stages_done: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct RevisionRecord {
    revision: String,
    label: String,
    sheets_total: usize,
    sheets_changed: usize,
    sheets_added: usize,
    sheets_removed: usize,
}

// ─── Global project state ────────────────────────────────────────────────────

static PROJECT: std::sync::Mutex<Option<PrintSetProject>> = std::sync::Mutex::new(None);

fn get_project() -> Result<PrintSetProject> {
    PROJECT
        .lock()
        .map_err(|_| EngineError::Other("project lock poisoned".into()))?
        .clone()
        .ok_or_else(|| EngineError::Other("no print-set project open; use printset.open first".into()))
}

fn set_project(p: PrintSetProject) -> Result<()> {
    *PROJECT
        .lock()
        .map_err(|_| EngineError::Other("project lock poisoned".into()))? = Some(p);
    Ok(())
}

// ─── Classification helpers ──────────────────────────────────────────────────

fn infer_discipline(number: &str) -> String {
    let up = number.to_ascii_uppercase();
    for &(prefix, code) in &[
        ("FP-", "FP"), ("FA-", "FA"), ("M-", "M"), ("P-", "P"), ("S-", "S"),
        ("A-", "A"), ("G-", "G"), ("E-", "E"), ("T-", "T"), ("C-", "C"),
    ] {
        if up.starts_with(prefix) {
            return code.into();
        }
    }
    "unknown".into()
}

fn classify_sheet_type(number: &str, title: &str) -> String {
    let up = title.to_ascii_uppercase();
    if up.contains("COVER") || up.contains("INDEX") { return "cover".into(); }
    if up.contains("SCHEDULE") { return "schedule".into(); }
    if up.contains("DETAIL") { return "detail".into(); }
    if up.contains("SECTION") { return "section".into(); }
    if up.contains("ELEVATION") { return "elevation".into(); }
    if up.contains("DIAGRAM") || up.contains("RISER") { return "diagram".into(); }
    if up.contains("ENLARGED") { return "enlarged_plan".into(); }
    if up.contains("REFLECTED CEILING") || up.contains("RCP") { return "rcp".into(); }
    if up.contains("PLAN") { return "plan".into(); }
    let num_part = number.split('-').next_back().unwrap_or("");
    if let Ok(n) = num_part.parse::<u32>() {
        if (100..500).contains(&n) {
            return "plan".into();
        }
        if (600..700).contains(&n) {
            return "schedule".into();
        }
    }
    "unknown".into()
}

fn infer_level(title: &str) -> String {
    let up = title.to_ascii_uppercase();
    for prefix in &["LEVEL ", "FLOOR "] {
        if let Some(pos) = up.find(prefix) {
            let after = &up[pos + prefix.len()..];
            let code: String = after.chars().take_while(|c| c.is_alphanumeric()).collect();
            if !code.is_empty() {
                return code;
            }
        }
    }
    if up.contains("ROOF") { return "R".into(); }
    if up.contains("SITE") { return "SITE".into(); }
    String::new()
}

fn content_hash(data: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn file_hash(path: &Path) -> std::result::Result<String, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(content_hash(&data))
}

// ─── Sheet number parser from filename ───────────────────────────────────────

/// Try to extract a sheet number from a filename like "M-101 Level 1 Mech Plan.dxf".
fn number_from_filename(filename: &str) -> String {
    let stem = filename.rsplit('.').next_back().unwrap_or(filename);
    // Try to match a sheet number pattern at the start: "M-101", "FP-201", etc.
    let up = stem.to_ascii_uppercase();
    for prefix in &["FP-", "FA-", "M-", "P-", "S-", "A-", "G-", "E-", "T-", "C-"] {
        if let Some(rest) = up.strip_prefix(prefix) {
            // Take the number part.
            let num: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !num.is_empty() {
                return format!("{}{num}", prefix);
            }
        }
    }
    // Fall back to the whole stem.
    stem.to_string()
}

/// Try to extract a title from a filename like "M-101 Level 1 Mech Plan.dxf".
fn title_from_filename(filename: &str) -> String {
    let stem = filename.rsplit('.').next_back().unwrap_or(filename);
    // Strip the leading number pattern.
    let up = stem.to_ascii_uppercase();
    for prefix in &["FP-", "FA-", "M-", "P-", "S-", "A-", "G-", "E-", "T-", "C-"] {
        if let Some(rest) = up.strip_prefix(prefix) {
            let after_num: String = rest.chars().skip_while(|c| c.is_ascii_digit()).collect();
            let trimmed = after_num.trim_start_matches([' ', '-', '_']);
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    String::new()
}

// ─── Project operations ──────────────────────────────────────────────────────

fn discover_sheets(dir: &Path) -> std::result::Result<Vec<(String, PathBuf)>, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let mut files = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() { continue; }
        let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
        if ext == "dxf" || ext == "pdf" || ext == "dwg" {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            files.push((name, path));
        }
        if files.len() >= MAX_SHEETS { break; }
    }
    files.sort_by(|(a, _), (b, _)| a.cmp(b));
    Ok(files)
}

fn create_project(name: &str, dir: &Path) -> std::result::Result<PrintSetProject, String> {
    let files = discover_sheets(dir)?;
    if files.is_empty() {
        return Err(format!("no drawing files found in {}", dir.display()));
    }
    let mut sheets = Vec::new();
    for (filename, path) in &files {
        let meta = path.metadata().map_err(|e| format!("{filename}: {e}"))?;
        if meta.len() > MAX_FILE_BYTES { continue; }
        let hash = file_hash(path).unwrap_or_default();
        sheets.push(SheetEntry {
            file: filename.clone(),
            content_hash: hash,
            dirty: true,
            ..SheetEntry::default()
        });
    }
    let n = sheets.len();
    Ok(PrintSetProject {
        schema: SCHEMA.into(),
        name: name.into(),
        source_dir: dir.display().to_string(),
        sheets,
        revision: "initial".into(),
        dashboard: Dashboard { total_sheets: n, stages_total: 9, ..Dashboard::default() },
        ..PrintSetProject::default()
    })
}

fn update_dashboard(project: &mut PrintSetProject) {
    let mut d = Dashboard { total_sheets: project.sheets.len(), stages_total: 9, ..Dashboard::default() };
    let mut levels: BTreeMap<String, LevelSummary> = BTreeMap::new();
    let mut disciplines: BTreeMap<String, DisciplineSummary> = BTreeMap::new();
    let mut total_confidence = 0.0f64;
    let mut confidence_count = 0usize;

    for sheet in &project.sheets {
        if sheet.stage_results.get("classify").is_some_and(|r| r.status == "completed") {
            d.classified_sheets += 1;
        }
        if !sheet.level.is_empty() {
            let ls = levels.entry(sheet.level.clone()).or_insert_with(|| LevelSummary {
                level: sheet.level.clone(), ..LevelSummary::default()
            });
            ls.sheets += 1;
        }
        if !sheet.discipline.is_empty() && sheet.discipline != "unknown" {
            let ds = disciplines.entry(sheet.discipline.clone()).or_insert_with(|| DisciplineSummary {
                discipline: sheet.discipline.clone(), ..DisciplineSummary::default()
            });
            ds.sheets += 1;
        }
        for (stage_name, result) in &sheet.stage_results {
            if result.status == "completed" {
                d.total_items += result.items_found;
                total_confidence += result.confidence;
                confidence_count += 1;
                if !sheet.level.is_empty()
                    && let Some(ls) = levels.get_mut(&sheet.level) {
                        ls.items += result.items_found;
                    }
                if !sheet.discipline.is_empty()
                    && let Some(ds) = disciplines.get_mut(&sheet.discipline) {
                        ds.items += result.items_found;
                        if !ds.stages_done.contains(stage_name) {
                            ds.stages_done.push(stage_name.clone());
                        }
                    }
                for f in &result.findings {
                    match f.kind.as_str() {
                        "gap" | "guessed" | "missing_tag" | "unreadable" => {
                            d.open_gaps += 1;
                            if let Some(ls) = levels.get_mut(&sheet.level) { ls.gaps += 1; }
                        }
                        "rfi" | "conflict" => {
                            d.open_rfis += 1;
                            if let Some(ls) = levels.get_mut(&sheet.level) { ls.rfis += 1; }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    d.mean_confidence = if confidence_count > 0 { total_confidence / confidence_count as f64 } else { 0.0 };
    d.levels = levels.into_values().collect();
    d.disciplines = disciplines.into_values().collect();
    d.stages_completed = project.stages.iter().filter(|s| s.status == "completed").count();
    project.dashboard = d;
}

fn compute_delta(project: &PrintSetProject, new_files: &[(String, PathBuf)]) -> std::result::Result<RevisionDelta, String> {
    let current: BTreeMap<&str, &SheetEntry> = project.sheets.iter().map(|s| (s.file.as_str(), s)).collect();
    let mut delta = RevisionDelta::default();
    for (name, path) in new_files {
        let new_hash = file_hash(path)?;
        if let Some(existing) = current.get(name.as_str()) {
            if existing.content_hash == new_hash {
                delta.unchanged.push(name.clone());
            } else {
                delta.changed.push(name.clone());
            }
        } else {
            delta.added.push(name.clone());
        }
    }
    let new_names: std::collections::HashSet<&str> = new_files.iter().map(|(n, _)| n.as_str()).collect();
    for sheet in &project.sheets {
        if !new_names.contains(sheet.file.as_str()) {
            delta.removed.push(sheet.file.clone());
        }
    }
    Ok(delta)
}

#[derive(Clone, Debug, Default)]
struct RevisionDelta {
    added: Vec<String>,
    changed: Vec<String>,
    removed: Vec<String>,
    unchanged: Vec<String>,
}

fn apply_reimport(project: &mut PrintSetProject, new_files: &[(String, PathBuf)], label: &str) -> std::result::Result<RevisionDelta, String> {
    let delta = compute_delta(project, new_files)?;
    project.revisions.push(RevisionRecord {
        revision: label.into(), label: label.into(),
        sheets_total: new_files.len(), sheets_changed: delta.changed.len(),
        sheets_added: delta.added.len(), sheets_removed: delta.removed.len(),
    });
    let removed_set: std::collections::HashSet<&str> = delta.removed.iter().map(|s| s.as_str()).collect();
    project.sheets.retain(|s| !removed_set.contains(s.file.as_str()));
    let changed_set: std::collections::HashSet<&str> = delta.changed.iter().map(|s| s.as_str()).collect();
    for sheet in &mut project.sheets {
        if changed_set.contains(sheet.file.as_str()) {
            sheet.dirty = true;
            sheet.stage_results.clear();
            if let Some((_, path)) = new_files.iter().find(|(n, _)| n == &sheet.file) {
                sheet.content_hash = file_hash(path).unwrap_or_default();
            }
        }
    }
    for name in &delta.added {
        if let Some((_, path)) = new_files.iter().find(|(n, _)| n == name) {
            let hash = file_hash(path).unwrap_or_default();
            project.sheets.push(SheetEntry { file: name.clone(), content_hash: hash, dirty: true, ..SheetEntry::default() });
        }
    }
    if !delta.changed.is_empty() || !delta.added.is_empty() {
        project.stages.retain(|s| s.stage == "classify");
    }
    project.revision = label.into();
    update_dashboard(project);
    Ok(delta)
}

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
            .params("{path: folder or .printset.json, name?} → {name, sheets, revision, dashboard}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.classify", "Classify Print Set", run_classify)
            .params("{} → {classified, total, elapsed_ms}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.dashboard", "Print Set Dashboard", run_dashboard)
            .params("{} → {dashboard}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.sheets", "Print Set Sheets", run_sheets)
            .params("{discipline?, level?} → {sheets, total}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.reimport", "Re-import Revised Set", run_reimport)
            .params("{path: folder, revision?: label} → {delta, sheets}")
            .enabled(always)
            .noundo(),
        CommandSpec::new("printset.save", "Save Print Set", run_save)
            .params("{path?} → {saved}")
            .enabled(always)
            .noundo(),
    ]
}

// ─── printset.open ───────────────────────────────────────────────────────────

fn run_open(_s: &mut Session, p: &Value) -> Result<Value> {
    let path_str = str_param(p, "path")
        .ok_or_else(|| EngineError::Other("path is required".into()))?;
    let path = PathBuf::from(path_str);

    let project = if path.extension().and_then(|e| e.to_str()) == Some("json") && path.is_file() {
        load_project(&path).map_err(EngineError::Other)?
    } else if path.is_dir() {
        let name = str_param(p, "name").unwrap_or_else(|| {
            path.file_name().and_then(|n| n.to_str()).unwrap_or("Print Set")
        });
        create_project(name, &path).map_err(EngineError::Other)?
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
    let start = web_time::Instant::now();
    let total = project.sheets.len();
    let mut classified = 0usize;

    for i in 0..total {
        let entry = match project.sheets.get(i) {
            Some(e) => e.clone(),
            None => continue,
        };
        if !entry.dirty && !entry.number.is_empty() {
            classified += 1;
            continue;
        }

        // Classify from filename and any existing data.
        if let Some(sheet) = project.sheets.get_mut(i) {
            if sheet.number.is_empty() {
                sheet.number = number_from_filename(&entry.file);
            }
            if sheet.title.is_empty() {
                sheet.title = title_from_filename(&entry.file);
            }
            sheet.discipline = infer_discipline(&sheet.number);
            sheet.sheet_type = classify_sheet_type(&sheet.number, &sheet.title);
            sheet.level = infer_level(&sheet.title);
            sheet.dirty = false;
            let conf = if sheet.discipline != "unknown" && !sheet.title.is_empty() { 0.7 } else if sheet.discipline != "unknown" { 0.5 } else { 0.1 };
            sheet.stage_results.insert("classify".into(), SheetStageResult {
                stage: "classify".into(), status: "completed".into(),
                items_found: 1, confidence: conf, ..SheetStageResult::default()
            });
            classified += 1;
        }
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;
    project.stages.push(StageRecord {
        stage: "classify".into(),
        status: if classified == total { "completed" } else { "partial" }.into(),
        sheets_processed: classified, sheets_total: total,
        items_found: classified, findings: 0, elapsed_ms,
    });
    update_dashboard(&mut project);
    let result = json!({ "classified": classified, "total": total, "elapsed_ms": elapsed_ms });
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

    let sheets: Vec<Value> = project.sheets.iter()
        .filter(|s| filter_discipline.is_none_or(|d| s.discipline == d))
        .filter(|s| filter_level.is_none_or(|l| s.level == l))
        .map(|s| json!({
            "number": s.number, "title": s.title, "discipline": s.discipline,
            "type": s.sheet_type, "level": s.level, "scale": s.scale_text,
            "file": s.file, "dirty": s.dirty,
            "stages": s.stage_results.keys().collect::<Vec<_>>(),
        }))
        .collect();
    let total = sheets.len();
    Ok(json!({ "sheets": sheets, "total": total }))
}

// ─── printset.reimport ───────────────────────────────────────────────────────

fn run_reimport(_s: &mut Session, p: &Value) -> Result<Value> {
    let path_str = str_param(p, "path")
        .ok_or_else(|| EngineError::Other("path is required".into()))?;
    let dir = PathBuf::from(path_str);
    if !dir.is_dir() {
        return Err(EngineError::Other(format!("{path_str} is not a directory")));
    }
    let label = str_param(p, "revision").unwrap_or("revised");
    let files = discover_sheets(&dir).map_err(EngineError::Other)?;
    let mut project = get_project()?;
    let delta = apply_reimport(&mut project, &files, label).map_err(EngineError::Other)?;

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
