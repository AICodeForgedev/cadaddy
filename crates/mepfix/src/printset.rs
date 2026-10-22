//! Print-set project: opens a folder of PDF/DXF sheets (or a merged PDF) as a project,
//! classifies every sheet (discipline, type, level, scale), runs each recognition stage as a
//! resumable pipeline with progress, and aggregates a dashboard of what has been modelled, its
//! confidence, open gaps and RFIs. Supports re-importing revised sets (addendum/ASI/bulletin)
//! by diffing against the previous import and only reprocessing changed sheets.
//!
//! Memory and speed budgets: each stage works one sheet at a time and releases intermediate
//! drawings before moving on, so peak memory stays proportional to the largest single sheet.
//! The project state (JSON) is the only thing kept fully in memory.

use crate::sheet_reader::{self, RecognisedSheet, SheetMap};
use crate::truth::Truth;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Maximum sheets in a single project (hostile input guard).
const MAX_SHEETS: usize = 2000;

/// Maximum file size for a single sheet, bytes (500 MB — a very large vector PDF).
const MAX_FILE_BYTES: u64 = 500_000_000;

// ─── Project model ───────────────────────────────────────────────────────────

/// A print-set project: a set of sheets and their recognition state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PrintSetProject {
    /// Schema version for forwards compatibility.
    pub schema: String,
    /// Made-up project name (from the cover sheet or the folder name).
    pub name: String,
    /// Where the source files live.
    pub source_dir: String,
    /// Each sheet in the set, keyed by sheet number.
    pub sheets: Vec<SheetEntry>,
    /// Recognised grid (assembled from all aligned sheets).
    #[serde(default)]
    pub grid: SheetMap,
    /// Which pipeline stages have been completed.
    pub stages: Vec<StageRecord>,
    /// The current revision identifier (folder mtime or explicit label).
    pub revision: String,
    /// History of re-imports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revisions: Vec<RevisionRecord>,
    /// Aggregated dashboard, updated after each stage.
    pub dashboard: Dashboard,
}

/// The project file's schema tag.
pub const SCHEMA: &str = "cadaddy-printset/1";

/// A sheet in the project: its classification and per-stage state.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SheetEntry {
    /// Sheet number as read from the title block (`M-101`).
    pub number: String,
    /// Sheet title (`LEVEL 1 MECHANICAL PLAN`).
    pub title: String,
    /// Source file, relative to `source_dir`.
    pub file: String,
    /// Discipline code: `G`, `A`, `S`, `M`, `P`, `FP`, `E`, `FA`, `T`, `C`.
    pub discipline: String,
    /// Sheet type: `plan`, `enlarged_plan`, `rcp`, `schedule`, `cover`, `section`, `elevation`,
    /// `detail`, `diagram`, `unknown`.
    pub sheet_type: String,
    /// Level code (`1`, `2`, `R`) for plan sheets; empty for non-plan sheets.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub level: String,
    /// Scale text as printed (`1/8" = 1'-0"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_text: Option<String>,
    /// Paper inches per building foot.
    pub scale: f64,
    /// Recognition data from the sheet reader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recognised: Option<RecognisedSheet>,
    /// Per-stage outcomes for this sheet.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stage_results: BTreeMap<String, SheetStageResult>,
    /// File content hash (SHA-256 hex, first 16 chars) for delta detection.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_hash: String,
    /// True if this sheet changed in the latest re-import and needs reprocessing.
    #[serde(default)]
    pub dirty: bool,
    /// The page of `file` this sheet is on (a merged set: one entry per page); 0 for a file
    /// that is one sheet.
    #[serde(default)]
    pub page: u32,
    /// Every level the sheet shows, `level` first (a typical-floor plan shows several).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<String>,
    /// A later file (a revision) has this sheet number: that file, page. Stages skip it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub superseded_by: String,
}

/// What one stage found on one sheet.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SheetStageResult {
    pub stage: String,
    pub status: StageStatus,
    /// Items found (runs, devices, etc.) on this sheet by this stage.
    pub items_found: usize,
    /// Confidence: 0.0–1.0 (how much of the sheet's expected content the stage found).
    pub confidence: f64,
    /// Findings that need review.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
    /// Error message if the stage failed on this sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A finding from recognition that needs review: a guessed value, a missing tag, a conflict.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    /// `gap`, `rfi`, `conflict`, `guessed`, `missing_tag`, `unreadable`.
    pub kind: String,
    pub description: String,
    /// Severity: 1 (info) to 5 (critical).
    pub severity: u8,
    /// Where on the plan, building feet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ft: Option<[f64; 2]>,
    /// Element ids involved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<String>,
}

/// Status of a single stage's run on a single sheet (or overall).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StageStatus {
    #[default]
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

// ─── Pipeline stages ─────────────────────────────────────────────────────────

/// The pipeline stages, in order. Each depends on the previous.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// Read title blocks, classify sheets, detect grids, compute alignment.
    Classify,
    /// Read ductwork from mechanical plans.
    Ducts,
    /// Read piping from plumbing plans.
    Piping,
    /// Read fire protection from FP plans.
    FireProtection,
    /// Read electrical devices from E plans.
    Electrical,
    /// Read structural framing from S plans.
    Structure,
    /// Assign heights by stacking rules and ceiling/structure detection.
    Heights,
    /// Detect clashes between trades.
    Clashes,
    /// Resolve callouts between sheets (needs Q306 when available).
    Callouts,
}

impl Stage {
    pub fn all() -> &'static [Stage] {
        &[
            Stage::Classify,
            Stage::Ducts,
            Stage::Piping,
            Stage::FireProtection,
            Stage::Electrical,
            Stage::Structure,
            Stage::Heights,
            Stage::Clashes,
            Stage::Callouts,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            Stage::Classify => "classify",
            Stage::Ducts => "ducts",
            Stage::Piping => "piping",
            Stage::FireProtection => "fire_protection",
            Stage::Electrical => "electrical",
            Stage::Structure => "structure",
            Stage::Heights => "heights",
            Stage::Clashes => "clashes",
            Stage::Callouts => "callouts",
        }
    }

    /// Which discipline codes this stage reads from (empty = all/cross-discipline).
    pub fn disciplines(self) -> &'static [&'static str] {
        match self {
            Stage::Classify => &[],
            Stage::Ducts => &["M"],
            Stage::Piping => &["P"],
            Stage::FireProtection => &["FP"],
            Stage::Electrical => &["E"],
            Stage::Structure => &["S"],
            Stage::Heights => &[],
            Stage::Clashes => &[],
            Stage::Callouts => &[],
        }
    }
}

/// A record of a completed (or failed) stage run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StageRecord {
    pub stage: String,
    pub status: StageStatus,
    pub sheets_processed: usize,
    pub sheets_total: usize,
    pub items_found: usize,
    pub findings: usize,
    /// Elapsed milliseconds.
    pub elapsed_ms: u64,
}

// ─── Dashboard ───────────────────────────────────────────────────────────────

/// The project dashboard: what has been modelled, where, at what confidence.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Dashboard {
    /// Total sheets in the project.
    pub total_sheets: usize,
    /// Sheets classified (after the classify stage).
    pub classified_sheets: usize,
    /// Sheets replaced by the same sheet number in a later file (a revision).
    #[serde(default)]
    pub superseded_sheets: usize,
    /// Per-level summary.
    pub levels: Vec<LevelSummary>,
    /// Per-discipline summary.
    pub disciplines: Vec<DisciplineSummary>,
    /// Total items modelled across all stages.
    pub total_items: usize,
    /// Mean confidence across all completed sheet-stage pairs.
    pub mean_confidence: f64,
    /// Open gaps (things the recognition flagged as needing review).
    pub open_gaps: usize,
    /// Open RFIs (conflicts that need a decision).
    pub open_rfis: usize,
    /// Total clashes found.
    pub total_clashes: usize,
    /// Stages completed.
    pub stages_completed: usize,
    /// Stages total.
    pub stages_total: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LevelSummary {
    pub level: String,
    pub sheets: usize,
    pub items: usize,
    pub confidence: f64,
    pub gaps: usize,
    pub rfis: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DisciplineSummary {
    pub discipline: String,
    pub sheets: usize,
    pub items: usize,
    pub confidence: f64,
    pub stages_done: Vec<String>,
}

// ─── Re-import / revision tracking ──────────────────────────────────────────

/// A record of a past import (the original or a re-import).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RevisionRecord {
    pub revision: String,
    pub label: String,
    pub sheets_total: usize,
    pub sheets_changed: usize,
    pub sheets_added: usize,
    pub sheets_removed: usize,
}

/// The delta between two revisions of a print set.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RevisionDelta {
    /// Sheets that were added in the new revision.
    pub added: Vec<String>,
    /// Sheets whose content changed (same number, different hash).
    pub changed: Vec<String>,
    /// Sheets that were removed.
    pub removed: Vec<String>,
    /// Sheets that are unchanged.
    pub unchanged: Vec<String>,
}

// ─── Progress callback ──────────────────────────────────────────────────────

/// Progress report from the pipeline.
#[derive(Clone, Debug)]
pub struct Progress {
    pub stage: String,
    pub sheet_index: usize,
    pub sheet_number: String,
    pub sheets_total: usize,
    pub items_so_far: usize,
}

/// The progress callback type (not `'static` so callers can capture local variables).
pub type ProgressFn<'a> = dyn FnMut(&Progress) + 'a;

// ─── Content hashing ─────────────────────────────────────────────────────────

/// Hash file content (first 16 hex chars of SHA-256) for delta detection.
fn content_hash(data: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    // A fast non-crypto hash for delta detection (not security).
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Hash a file on disk.
fn file_hash(path: &Path) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(content_hash(&data))
}

// ─── Sheet classification ───────────────────────────────────────────────────

/// Classify a sheet's type from its title and number.
fn classify_sheet_type(number: &str, title: &str) -> String {
    let up = title.to_ascii_uppercase();
    if up.contains("COVER") || up.contains("INDEX") {
        return "cover".into();
    }
    if up.contains("SCHEDULE") {
        return "schedule".into();
    }
    if up.contains("DETAIL") {
        return "detail".into();
    }
    if up.contains("SECTION") {
        return "section".into();
    }
    if up.contains("ELEVATION") {
        return "elevation".into();
    }
    if up.contains("DIAGRAM") || up.contains("RISER") {
        return "diagram".into();
    }
    if up.contains("ENLARGED") {
        return "enlarged_plan".into();
    }
    if up.contains("REFLECTED CEILING") || up.contains("RCP") {
        return "rcp".into();
    }
    // Number-based: 1xx, 2xx, 3xx, 4xx are plan sheets for levels.
    let num_part = number.split('-').next_back().unwrap_or("");
    if num_part.len() == 3
        && let Ok(n) = num_part.parse::<u32>()
        && ((100..200).contains(&n) || (200..300).contains(&n))
        && (up.contains("PLAN") || up.contains("FLOOR") || up.contains("LIGHTING") || up.contains("POWER"))
    {
        return "plan".into();
    }
    if up.contains("PLAN") {
        return "plan".into();
    }
    "unknown".into()
}

/// Infer the discipline from the sheet number prefix.
fn infer_discipline(number: &str) -> String {
    let up = number.to_ascii_uppercase();
    for &(prefix, code) in
        &[("FP-", "FP"), ("FA-", "FA"), ("M-", "M"), ("P-", "P"), ("S-", "S"), ("A-", "A"), ("G-", "G"), ("E-", "E"), ("T-", "T"), ("C-", "C")]
    {
        if up.starts_with(prefix) {
            return code.into();
        }
    }
    "unknown".into()
}

/// Infer the level code from a title like "LEVEL 2 MECHANICAL PLAN" (the first level it
/// names: [`sheet_reader::detect_levels`]); civil plans are on the site.
fn infer_level(title: &str) -> String {
    if let Some(level) = sheet_reader::detect_levels(title).into_iter().next() {
        return level;
    }
    let up = title.to_ascii_uppercase();
    // Civil plans (grading, earthwork, utilities) are all on the site.
    if up.contains("SITE") || up.contains("GRADING") || up.contains("EARTHWORK") || up.contains("PAVING") {
        return "SITE".into();
    }
    String::new()
}

/// The level [`create_from_truth`] and the classify stage give a sheet title (tests).
pub fn level_of_title(title: &str) -> String {
    infer_level(title)
}

// ─── Project creation ────────────────────────────────────────────────────────

/// Deepest folder looked into below a project's folder.
const MAX_FOLDER_DEPTH: usize = 6;

/// Discover sheets in a folder and its subfolders (DXF, DWG and PDF files), named by their path
/// below `dir`. A folder's own files come first, sorted, then its subfolders, sorted: revision
/// folders (`ASI 01/`, `Addendum 2/`) come after the set they revise.
pub fn discover_sheets(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    if !dir.is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    let mut files = Vec::new();
    discover_in(dir, "", 0, &mut files)?;
    Ok(files)
}

fn discover_in(dir: &Path, prefix: &str, depth: usize, files: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut here: Vec<(String, PathBuf)> = Vec::new();
    let mut folders: Vec<(String, PathBuf)> = Vec::new();
    for entry in entries.take(MAX_SHEETS * 4) {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        if name.is_empty() || name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            folders.push((name, path));
            continue;
        }
        let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
        if ext == "dxf" || ext == "pdf" || ext == "dwg" {
            here.push((format!("{prefix}{name}"), path));
        }
    }
    here.sort_by(|(a, _), (b, _)| a.cmp(b));
    folders.sort_by(|(a, _), (b, _)| a.cmp(b));
    for f in here {
        if files.len() >= MAX_SHEETS {
            return Ok(());
        }
        files.push(f);
    }
    if depth < MAX_FOLDER_DEPTH {
        for (name, path) in folders {
            discover_in(&path, &format!("{prefix}{name}/"), depth + 1, files)?;
        }
    }
    Ok(())
}

/// The project's entries for one file: one per page of a PDF (a merged set), else one.
fn entries_for_file(name: &str, path: &Path) -> Vec<SheetEntry> {
    let Ok(data) = std::fs::read(path) else {
        return vec![SheetEntry { file: name.into(), dirty: true, ..SheetEntry::default() }];
    };
    let hash = content_hash(&data);
    let pdf = path.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("pdf"));
    let pages = if pdf { cadaddy_io::pdf_read::page_count(&data).ok().map_or(0, |n| (n as usize).min(MAX_SHEETS)) } else { 0 };
    if pages <= 1 {
        return vec![SheetEntry { file: name.into(), content_hash: hash, dirty: true, page: u32::from(pages == 1), ..SheetEntry::default() }];
    }
    (1..=pages)
        .map(|p| SheetEntry {
            file: name.into(),
            content_hash: hash.clone(),
            dirty: true,
            page: u32::try_from(p).unwrap_or(u32::MAX),
            ..SheetEntry::default()
        })
        .collect()
}

/// Create a new project from a folder of sheet files (and merged PDF sets: one sheet per page).
/// This discovers files and computes hashes but does not run any recognition stages.
pub fn create_project(name: &str, dir: &Path) -> Result<PrintSetProject, String> {
    let files = discover_sheets(dir)?;
    if files.is_empty() {
        return Err(format!("no drawing files found in {}", dir.display()));
    }
    Ok(create_project_from_files(name, dir, &files))
}

/// [`create_project`] from files already chosen (named as [`discover_sheets`] names them, in
/// order: later files revise earlier ones).
pub fn create_project_from_files(name: &str, dir: &Path, files: &[(String, PathBuf)]) -> PrintSetProject {
    let mut sheets = Vec::new();
    for (filename, path) in files {
        if path.metadata().map_or(true, |m| m.len() > MAX_FILE_BYTES) {
            continue;
        }
        sheets.extend(entries_for_file(filename, path));
        if sheets.len() >= MAX_SHEETS {
            sheets.truncate(MAX_SHEETS);
            break;
        }
    }
    let n = sheets.len();
    PrintSetProject {
        schema: SCHEMA.into(),
        name: name.into(),
        source_dir: dir.display().to_string(),
        sheets,
        revision: "initial".into(),
        dashboard: Dashboard { total_sheets: n, stages_total: Stage::all().len(), ..Dashboard::default() },
        ..PrintSetProject::default()
    }
}

/// Create a project from a [`Truth`] (the synthetic fixture set's ground truth). This fills in
/// the sheet entries from the truth's sheet list so tests can exercise the pipeline without real
/// files on disk.
pub fn create_from_truth(truth: &Truth) -> PrintSetProject {
    let mut sheets = Vec::new();
    for sh in &truth.sheets {
        let discipline = infer_discipline(&sh.number);
        let sheet_type = classify_sheet_type(&sh.number, &sh.title);
        let level = sh.level.clone().unwrap_or_else(|| infer_level(&sh.title));
        sheets.push(SheetEntry {
            number: sh.number.clone(),
            title: sh.title.clone(),
            file: sh.dxf.clone(),
            discipline,
            sheet_type,
            level,
            scale_text: sh.scale_text.clone(),
            scale: sh.scale,
            dirty: true,
            ..SheetEntry::default()
        });
    }
    let n = sheets.len();
    PrintSetProject {
        schema: SCHEMA.into(),
        name: truth.project.clone(),
        sheets,
        revision: "initial".into(),
        dashboard: Dashboard { total_sheets: n, stages_total: Stage::all().len(), ..Dashboard::default() },
        ..PrintSetProject::default()
    }
}

// ─── Classification stage ────────────────────────────────────────────────────

/// Run the classify stage: for each sheet, read its title block and fill in discipline, type,
/// level, scale. If `dxf_loader` is provided, it reads the DXF from the source dir; otherwise
/// the entries must already have `recognised` data (from truth or a previous run).
pub fn run_classify(
    project: &mut PrintSetProject,
    dxf_loader: Option<&dyn Fn(&str) -> Result<cadaddy_doc::Drawing, String>>,
    cancel: &AtomicBool,
    progress: &mut ProgressFn<'_>,
) -> StageRecord {
    let start = std::time::Instant::now();
    let mut classified = 0usize;
    let total = project.sheets.len();

    for i in 0..total {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        progress(&Progress {
            stage: "classify".into(),
            sheet_index: i,
            sheet_number: project.sheets.get(i).map(|s| s.number.clone()).unwrap_or_default(),
            sheets_total: total,
            items_so_far: classified,
        });

        let entry = match project.sheets.get(i) {
            Some(e) => e.clone(),
            None => continue,
        };

        // Try to recognise from DXF if we have a loader and the entry is dirty or unclassified.
        if let Some(loader) = &dxf_loader
            && (entry.dirty || entry.number.is_empty())
        {
            match loader(&entry.file) {
                Ok(drawing) => {
                    let rec = sheet_reader::recognise_dxf(&drawing);
                    if let Some(e) = project.sheets.get_mut(i) {
                        e.number = if rec.number.is_empty() {
                            // Fall back to filename without extension.
                            entry.file.rsplit('.').next_back().unwrap_or(&entry.file).into()
                        } else {
                            rec.number.clone()
                        };
                        e.title = rec.title.clone();
                        e.discipline = if rec.discipline.is_empty() { infer_discipline(&e.number) } else { rec.discipline.clone() };
                        e.sheet_type = classify_sheet_type(&e.number, &e.title);
                        e.level = rec.level.clone().unwrap_or_else(|| infer_level(&e.title));
                        e.scale_text = rec.scale_text.clone();
                        e.scale = rec.scale;
                        e.recognised = Some(rec);
                        e.dirty = false;
                        e.stage_results.insert(
                            "classify".into(),
                            SheetStageResult {
                                stage: "classify".into(),
                                status: StageStatus::Completed,
                                items_found: 1,
                                confidence: 1.0,
                                ..SheetStageResult::default()
                            },
                        );
                    }
                    classified += 1;
                }
                Err(err) => {
                    if let Some(e) = project.sheets.get_mut(i) {
                        e.stage_results.insert(
                            "classify".into(),
                            SheetStageResult {
                                stage: "classify".into(),
                                status: StageStatus::Failed,
                                error: Some(err),
                                ..SheetStageResult::default()
                            },
                        );
                    }
                }
            }
            continue;
        }

        // If the entry already has data from truth or a previous run, just classify.
        if let Some(e) = project.sheets.get_mut(i) {
            if e.discipline.is_empty() && !e.number.is_empty() {
                e.discipline = infer_discipline(&e.number);
            }
            if e.sheet_type.is_empty() && !e.title.is_empty() {
                e.sheet_type = classify_sheet_type(&e.number, &e.title);
            }
            if e.level.is_empty() && !e.title.is_empty() {
                e.level = infer_level(&e.title);
            }
            e.stage_results.insert(
                "classify".into(),
                SheetStageResult {
                    stage: "classify".into(),
                    status: StageStatus::Completed,
                    items_found: 1,
                    confidence: if e.discipline.is_empty() || e.discipline == "unknown" { 0.5 } else { 1.0 },
                    ..SheetStageResult::default()
                },
            );
            classified += 1;
        }
    }

    // Build grid from all recognised sheets (just collect them into a SheetMap).
    let recognised: Vec<RecognisedSheet> = project.sheets.iter().filter_map(|s| s.recognised.clone()).collect();
    project.grid = SheetMap { sheets: recognised, grid_x: Vec::new(), grid_y: Vec::new() };

    let rec = StageRecord {
        stage: "classify".into(),
        status: if classified == total { StageStatus::Completed } else { StageStatus::Failed },
        sheets_processed: classified,
        sheets_total: total,
        items_found: classified,
        findings: 0,
        elapsed_ms: start.elapsed().as_millis() as u64,
    };
    project.stages.push(rec.clone());
    update_dashboard(project);
    rec
}

/// Run the classify stage on the prints themselves: each entry's sheet (a file, or a page of a
/// merged set) loaded by `load` and read by the sheet reader. A sheet printing several plan
/// views of different levels becomes one entry per view (`<number>/<level>`). A sheet number
/// that a later file has again (a revision folder after the set) is marked superseded.
pub fn run_classify_prints(
    project: &mut PrintSetProject,
    load: &dyn Fn(&SheetEntry) -> Result<cadaddy_doc::Drawing, String>,
    cancel: &AtomicBool,
    progress: &mut ProgressFn<'_>,
) -> StageRecord {
    let start = std::time::Instant::now();
    let total = project.sheets.len();
    let mut out: Vec<SheetEntry> = Vec::with_capacity(total);
    let mut classified = 0usize;
    let old = std::mem::take(&mut project.sheets);
    for (i, entry) in old.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) || out.len() >= MAX_SHEETS {
            out.push(entry);
            continue;
        }
        progress(&Progress {
            stage: "classify".into(),
            sheet_index: i,
            sheet_number: entry.number.clone(),
            sheets_total: total,
            items_so_far: classified,
        });
        if !entry.dirty && !entry.number.is_empty() {
            out.push(entry);
            classified += 1;
            continue;
        }
        let drawing = match load(&entry) {
            Ok(d) => d,
            Err(err) => {
                let mut e = entry;
                e.stage_results.insert(
                    "classify".into(),
                    SheetStageResult { stage: "classify".into(), status: StageStatus::Failed, error: Some(err), ..SheetStageResult::default() },
                );
                out.push(e);
                continue;
            }
        };
        for (rec, _) in sheet_reader::split_views(&drawing) {
            let mut e = entry.clone();
            e.number = if rec.number.is_empty() { format!("{}#{}", entry.file, entry.page) } else { rec.number.clone() };
            e.title = rec.title.clone();
            let printed = if rec.printed_number.is_empty() { rec.number.as_str() } else { rec.printed_number.as_str() };
            e.discipline = if rec.discipline.is_empty() { infer_discipline(printed) } else { rec.discipline.clone() };
            e.sheet_type = classify_sheet_type(printed, &e.title);
            if !rec.printed_number.is_empty() && e.sheet_type == "unknown" {
                // One view of a several-view sheet is a plan of its level.
                e.sheet_type = "plan".into();
            }
            e.level = rec.level.clone().unwrap_or_else(|| infer_level(&e.title));
            e.levels = rec.levels.clone();
            e.scale_text = rec.scale_text.clone();
            e.scale = rec.scale;
            e.dirty = false;
            let known = !rec.number.is_empty();
            e.stage_results.insert(
                "classify".into(),
                SheetStageResult {
                    stage: "classify".into(),
                    status: StageStatus::Completed,
                    items_found: 1,
                    confidence: if known && e.sheet_type != "unknown" { 1.0 } else { 0.5 },
                    findings: if known {
                        Vec::new()
                    } else {
                        vec![Finding { kind: "unreadable".into(), description: "no sheet number found".into(), severity: 2, ..Finding::default() }]
                    },
                    ..SheetStageResult::default()
                },
            );
            e.recognised = Some(rec);
            out.push(e);
        }
        classified += 1;
    }
    // Later files replace earlier sheets of the same number.
    let mut latest: BTreeMap<String, usize> = BTreeMap::new();
    for (i, e) in out.iter().enumerate() {
        if !e.number.is_empty() && e.recognised.as_ref().is_some_and(|r| !r.number.is_empty()) {
            latest.insert(e.number.clone(), i);
        }
    }
    let names: Vec<(String, u32)> = out.iter().map(|e| (e.file.clone(), e.page)).collect();
    for (i, e) in out.iter_mut().enumerate() {
        e.superseded_by = match latest.get(&e.number) {
            Some(&j) if j != i && names.get(j).is_some_and(|(f, _)| *f != e.file) => {
                names.get(j).map(|(f, p)| if *p == 0 { f.clone() } else { format!("{f}#{p}") }).unwrap_or_default()
            }
            _ => String::new(),
        };
    }
    project.sheets = out;
    let current: Vec<RecognisedSheet> = project.sheets.iter().filter(|s| s.superseded_by.is_empty()).filter_map(|s| s.recognised.clone()).collect();
    project.grid = sheet_reader::sheet_map_from(current);
    // Each sheet's alignment to the common grid.
    for s in &mut project.sheets {
        if let Some(r) = &mut s.recognised
            && let Some(m) = project.grid.sheets.iter().find(|m| m.number == r.number && s.superseded_by.is_empty())
        {
            r.alignment = m.alignment.clone();
        }
    }
    let rec = StageRecord {
        stage: "classify".into(),
        status: if classified == total { StageStatus::Completed } else { StageStatus::Failed },
        sheets_processed: classified,
        sheets_total: total,
        items_found: project.sheets.iter().filter(|s| s.superseded_by.is_empty()).count(),
        findings: 0,
        elapsed_ms: start.elapsed().as_millis() as u64,
    };
    project.stages.push(rec.clone());
    update_dashboard(project);
    rec
}

// ─── MEP reading stages ─────────────────────────────────────────────────────

/// Run a MEP reading stage against the truth data (for testing without real DXF files).
/// In production this would call the actual readers (duct_reader, pipe_reader, etc.) on
/// DXF drawings; here we simulate by scoring against truth data.
pub fn run_stage_from_truth(
    project: &mut PrintSetProject,
    stage: Stage,
    truth: &Truth,
    cancel: &AtomicBool,
    progress: &mut ProgressFn<'_>,
) -> StageRecord {
    let start = std::time::Instant::now();
    let stage_name = stage.name();
    let disciplines = stage.disciplines();

    // Filter sheets by discipline.
    let sheet_indices: Vec<usize> = project
        .sheets
        .iter()
        .enumerate()
        .filter(|(_, s)| s.superseded_by.is_empty())
        .filter(|(_, s)| if disciplines.is_empty() { true } else { disciplines.iter().any(|d| *d == s.discipline) })
        .filter(|(_, s)| s.sheet_type == "plan" || s.sheet_type == "enlarged_plan" || s.sheet_type == "rcp")
        .map(|(i, _)| i)
        .collect();

    let total = sheet_indices.len();
    let mut processed = 0usize;
    let mut total_items = 0usize;
    let mut total_findings = 0usize;

    for &idx in &sheet_indices {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let number = project.sheets.get(idx).map(|s| s.number.clone()).unwrap_or_default();
        let level = project.sheets.get(idx).map(|s| s.level.clone()).unwrap_or_default();
        progress(&Progress {
            stage: stage_name.into(),
            sheet_index: processed,
            sheet_number: number.clone(),
            sheets_total: total,
            items_so_far: total_items,
        });

        // Count truth items for this sheet's level and discipline.
        let (items, confidence, findings) = count_truth_items(truth, stage, &level, &number);
        total_items += items;
        total_findings += findings.len();

        if let Some(e) = project.sheets.get_mut(idx) {
            e.stage_results.insert(
                stage_name.into(),
                SheetStageResult {
                    stage: stage_name.into(),
                    status: StageStatus::Completed,
                    items_found: items,
                    confidence,
                    findings,
                    ..SheetStageResult::default()
                },
            );
        }
        processed += 1;
    }

    let rec = StageRecord {
        stage: stage_name.into(),
        status: if processed == total { StageStatus::Completed } else { StageStatus::Failed },
        sheets_processed: processed,
        sheets_total: total,
        items_found: total_items,
        findings: total_findings,
        elapsed_ms: start.elapsed().as_millis() as u64,
    };
    project.stages.push(rec.clone());
    update_dashboard(project);
    rec
}

/// Count truth items relevant to a stage on a given level.
fn count_truth_items(truth: &Truth, stage: Stage, level: &str, _sheet: &str) -> (usize, f64, Vec<Finding>) {
    let mut findings = Vec::new();
    match stage {
        Stage::Classify => (1, 1.0, findings),
        Stage::Ducts => {
            let n = truth.segments.iter().filter(|s| s.kind == "duct" && s.level == level).count();
            let equip =
                truth.equipment.iter().filter(|e| e.level == level && (e.kind == "ahu" || e.kind == "vav" || e.kind == "exhaust_fan")).count();
            let terms = truth.terminals.iter().filter(|t| t.level == level).count();
            // Add findings for segments without height notes.
            for seg in truth.segments.iter().filter(|s| s.kind == "duct" && s.level == level) {
                if let Some(h) = &seg.height
                    && h.note.is_none()
                {
                    findings.push(Finding {
                        kind: "guessed".into(),
                        description: format!("duct {} height from rules, no note on sheet", seg.id),
                        severity: 2,
                        ..Finding::default()
                    });
                }
            }
            (n + equip + terms, if n > 0 { 0.85 } else { 0.0 }, findings)
        }
        Stage::Piping => {
            let n = truth.segments.iter().filter(|s| s.kind == "pipe" && s.level == level && s.system != "FP").count();
            let fix = truth.fixtures.iter().filter(|f| f.level == level).count();
            (n + fix, if n > 0 { 0.80 } else { 0.0 }, findings)
        }
        Stage::FireProtection => {
            let n = truth.segments.iter().filter(|s| s.system == "FP" && s.level == level).count();
            let heads = truth.terminals.iter().filter(|t| t.kind == "sprinkler_head" && t.level == level).count();
            (n + heads, if n > 0 { 0.90 } else { 0.0 }, findings)
        }
        Stage::Electrical => {
            let n = truth.elec_devices.iter().filter(|d| d.level == level).count();
            let panels = truth.elec_panels.iter().filter(|p| p.level == level).count();
            (n + panels, if n > 0 { 0.75 } else { 0.0 }, findings)
        }
        Stage::Structure => {
            let cols = truth.building.columns.len();
            let beams = truth.building.beams.iter().filter(|b| b.level == level).count();
            (cols + beams, if cols > 0 { 0.95 } else { 0.0 }, findings)
        }
        Stage::Heights => {
            let noted = truth.segments.iter().filter(|s| s.level == level).filter(|s| s.height.as_ref().is_some_and(|h| h.note.is_some())).count();
            let total_segs = truth.segments.iter().filter(|s| s.level == level).count();
            (total_segs, if total_segs > 0 { noted as f64 / total_segs as f64 } else { 0.0 }, findings)
        }
        Stage::Clashes => {
            let n = truth.clashes.len();
            for c in &truth.clashes {
                if !c.planted {
                    findings.push(Finding {
                        kind: "conflict".into(),
                        description: format!("{} clash: {} vs {}", c.kind, c.a, c.b),
                        severity: if c.kind == "hard" { 4 } else { 3 },
                        at_ft: Some([c.at_ft[0], c.at_ft[1]]),
                        elements: vec![c.a.clone(), c.b.clone()],
                    });
                }
            }
            (n, 1.0, findings)
        }
        Stage::Callouts => {
            // Callouts are cross-sheet references; count elevation + section views.
            let n = truth.elevations.len() + truth.sections.len();
            (n, if n > 0 { 0.90 } else { 1.0 }, findings)
        }
    }
}

// ─── Run all stages ──────────────────────────────────────────────────────────

/// Run the full pipeline (all stages in order) against truth data. Returns all stage records.
pub fn run_all_from_truth(project: &mut PrintSetProject, truth: &Truth, cancel: &AtomicBool, progress: &mut ProgressFn<'_>) -> Vec<StageRecord> {
    let mut records = Vec::new();

    // Classify first (uses truth data directly since project was created from truth).
    let rec = run_classify(project, None, cancel, progress);
    records.push(rec);
    if cancel.load(Ordering::Relaxed) {
        return records;
    }

    // Then all MEP stages.
    for &stage in &Stage::all()[1..] {
        let rec = run_stage_from_truth(project, stage, truth, cancel, progress);
        records.push(rec);
        if cancel.load(Ordering::Relaxed) {
            break;
        }
    }
    records
}

// ─── Dashboard update ────────────────────────────────────────────────────────

/// Recompute the dashboard from the project's current state.
pub fn update_dashboard(project: &mut PrintSetProject) {
    let mut d = Dashboard { total_sheets: project.sheets.len(), stages_total: Stage::all().len(), ..Dashboard::default() };

    // Per-level and per-discipline aggregation.
    let mut levels: BTreeMap<String, LevelSummary> = BTreeMap::new();
    let mut disciplines: BTreeMap<String, DisciplineSummary> = BTreeMap::new();
    let mut total_confidence = 0.0f64;
    let mut confidence_count = 0usize;

    for sheet in &project.sheets {
        if !sheet.superseded_by.is_empty() {
            d.superseded_sheets += 1;
            continue;
        }
        // Classification count.
        if sheet.stage_results.get("classify").is_some_and(|r| r.status == StageStatus::Completed) {
            d.classified_sheets += 1;
        }

        // Level summary.
        if !sheet.level.is_empty() {
            let ls = levels.entry(sheet.level.clone()).or_insert_with(|| LevelSummary { level: sheet.level.clone(), ..LevelSummary::default() });
            ls.sheets += 1;
        }

        // Discipline summary.
        if !sheet.discipline.is_empty() && sheet.discipline != "unknown" {
            let ds = disciplines
                .entry(sheet.discipline.clone())
                .or_insert_with(|| DisciplineSummary { discipline: sheet.discipline.clone(), ..DisciplineSummary::default() });
            ds.sheets += 1;
        }

        // Per-stage results.
        for (stage_name, result) in &sheet.stage_results {
            if result.status == StageStatus::Completed {
                d.total_items += result.items_found;
                total_confidence += result.confidence;
                confidence_count += 1;

                // Level items.
                if !sheet.level.is_empty()
                    && let Some(ls) = levels.get_mut(&sheet.level)
                {
                    ls.items += result.items_found;
                    ls.confidence = (ls.confidence * (ls.sheets.saturating_sub(1)) as f64 + result.confidence) / ls.sheets.max(1) as f64;
                }

                // Discipline items.
                if !sheet.discipline.is_empty()
                    && let Some(ds) = disciplines.get_mut(&sheet.discipline)
                {
                    ds.items += result.items_found;
                    ds.confidence = (ds.confidence * (ds.sheets.saturating_sub(1)) as f64 + result.confidence) / ds.sheets.max(1) as f64;
                    if !ds.stages_done.contains(stage_name) {
                        ds.stages_done.push(stage_name.clone());
                    }
                }

                // Findings → gaps and RFIs.
                for f in &result.findings {
                    match f.kind.as_str() {
                        "gap" | "guessed" | "missing_tag" | "unreadable" => {
                            d.open_gaps += 1;
                            if !sheet.level.is_empty()
                                && let Some(ls) = levels.get_mut(&sheet.level)
                            {
                                ls.gaps += 1;
                            }
                        }
                        "rfi" | "conflict" => {
                            d.open_rfis += 1;
                            if !sheet.level.is_empty()
                                && let Some(ls) = levels.get_mut(&sheet.level)
                            {
                                ls.rfis += 1;
                            }
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
    d.stages_completed = project.stages.iter().filter(|s| s.status == StageStatus::Completed).count();

    // Clashes from the last clashes stage.
    if let Some(clash_rec) = project.stages.iter().rev().find(|s| s.stage == "clashes") {
        d.total_clashes = clash_rec.items_found;
    }

    project.dashboard = d;
}

// ─── Re-import (revised set) ─────────────────────────────────────────────────

/// Compute the delta between the current project and a new set of files.
pub fn compute_delta(project: &PrintSetProject, new_files: &[(String, PathBuf)]) -> Result<RevisionDelta, String> {
    let mut delta = RevisionDelta::default();

    // Build a map of current sheets by filename.
    let current: BTreeMap<&str, &SheetEntry> = project.sheets.iter().map(|s| (s.file.as_str(), s)).collect();

    // Check each new file.
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

    // Check for removed sheets.
    let new_names: std::collections::HashSet<&str> = new_files.iter().map(|(n, _)| n.as_str()).collect();
    for sheet in &project.sheets {
        // A merged set has an entry per page: its file is removed once.
        if !new_names.contains(sheet.file.as_str()) && !delta.removed.contains(&sheet.file) {
            delta.removed.push(sheet.file.clone());
        }
    }

    Ok(delta)
}

/// Apply a re-import: update the project with a new set of files, marking changed sheets dirty.
pub fn apply_reimport(project: &mut PrintSetProject, new_files: &[(String, PathBuf)], revision_label: &str) -> Result<RevisionDelta, String> {
    let delta = compute_delta(project, new_files)?;

    // Record the revision.
    project.revisions.push(RevisionRecord {
        revision: revision_label.into(),
        label: revision_label.into(),
        sheets_total: new_files.len(),
        sheets_changed: delta.changed.len(),
        sheets_added: delta.added.len(),
        sheets_removed: delta.removed.len(),
    });

    // Remove deleted sheets.
    let removed_set: std::collections::HashSet<&str> = delta.removed.iter().map(|s| s.as_str()).collect();
    project.sheets.retain(|s| !removed_set.contains(s.file.as_str()));

    // Mark changed sheets dirty and update hashes.
    let changed_set: std::collections::HashSet<&str> = delta.changed.iter().map(|s| s.as_str()).collect();
    for sheet in &mut project.sheets {
        if changed_set.contains(sheet.file.as_str()) {
            sheet.dirty = true;
            // Clear stage results so they re-run.
            sheet.stage_results.clear();
            // Update hash.
            if let Some((_, path)) = new_files.iter().find(|(n, _)| n == &sheet.file) {
                sheet.content_hash = file_hash(path).unwrap_or_default();
            }
        }
    }

    // Add new sheets.
    for name in &delta.added {
        if let Some((_, path)) = new_files.iter().find(|(n, _)| n == name) {
            project.sheets.extend(entries_for_file(name, path));
        }
    }

    // Clear completed stages that touch changed sheets (they need to re-run).
    if !delta.changed.is_empty() || !delta.added.is_empty() {
        project.stages.retain(|s| s.stage == "classify");
    }

    project.revision = revision_label.into();
    project.dashboard.total_sheets = project.sheets.len();
    update_dashboard(project);

    Ok(delta)
}

// ─── Serialization ───────────────────────────────────────────────────────────

/// Save a project to a JSON file.
pub fn save_project(project: &PrintSetProject, path: &Path) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(project).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("{}: {e}", path.display()))
}

/// Load a project from a JSON file.
pub fn load_project(path: &Path) -> Result<PrintSetProject, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&data).map_err(|e| format!("{}: {e}", path.display()))
}

// ─── Dashboard to JSON ───────────────────────────────────────────────────────

impl Dashboard {
    /// Serialize the dashboard to a JSON value for the engine command.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

impl PrintSetProject {
    /// A summary JSON for the engine command.
    pub fn summary_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "revision": self.revision,
            "sheets": self.sheets.len(),
            "stages_completed": self.dashboard.stages_completed,
            "stages_total": self.dashboard.stages_total,
            "items": self.dashboard.total_items,
            "confidence": (self.dashboard.mean_confidence * 1000.0).round() / 1000.0,
            "gaps": self.dashboard.open_gaps,
            "rfis": self.dashboard.open_rfis,
            "clashes": self.dashboard.total_clashes,
            "dashboard": self.dashboard.to_json(),
        })
    }

    /// Sheet list as JSON.
    pub fn sheets_json(&self) -> serde_json::Value {
        serde_json::json!(
            self.sheets
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "number": s.number,
                        "title": s.title,
                        "discipline": s.discipline,
                        "type": s.sheet_type,
                        "level": s.level,
                        "scale": s.scale_text,
                        "file": s.file,
                        "dirty": s.dirty,
                        "stages": s.stage_results.keys().collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>()
        )
    }
}
