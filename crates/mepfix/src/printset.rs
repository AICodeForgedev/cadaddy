//! Print-set project: opens a folder of PDF/DXF sheets (or a merged PDF) as a project,
//! classifies every sheet (discipline, type, level, scale), runs each recognition stage as a
//! resumable pipeline with progress, and aggregates a dashboard of what has been modelled, its
//! confidence, open gaps and RFIs. Supports re-importing revised sets (addendum/ASI/bulletin)
//! by diffing against the previous import and only reprocessing changed sheets.
//!
//! Memory and speed budgets: each stage works one sheet at a time and releases intermediate
//! drawings before moving on, so peak memory stays proportional to the largest single sheet.
//! The project state (JSON) is the only thing kept fully in memory.

use crate::sheet_facts;
use crate::sheet_reader::{self, RecognisedSheet, SheetMap};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Maximum sheets in a single project (hostile input guard).
const MAX_SHEETS: usize = 2000;

/// Maximum file size for a single sheet, bytes (500 MB — a very large vector PDF).
const MAX_FILE_BYTES: u64 = 500_000_000;

/// Buffer size for streaming hash (64 KB).
const HASH_BUF: usize = 65_536;

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
    /// Sheet index from the cover/index sheet: listed sheet numbers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sheet_index: Vec<String>,
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
    /// Discipline code: `G`, `A`, `S`, `M`, `P`, `FP`, `E`, `FA`, `T`, `C`, `SL`.
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
    /// File content hash (FNV-1a hex) for delta detection.
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
    /// Why `superseded_by` replaces this sheet (which order decided it).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub superseded_reason: String,
    /// Where `level` came from: `title` (the sheet or a view title names it), `number` (the
    /// sheet's place in a numbered plan series: A111…A114), empty when there is none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub level_source: String,
    /// The latest date printed on the sheet (`YYYY-MM-DD`: issue or revision dates), empty when
    /// none reads. The same sheet in two files: the later date is the current one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub issued: String,
    /// Level marks a building section or elevation prints (`SECOND FLOOR` over `110'-8"`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub level_marks: Vec<LevelMark>,
}

/// A level's elevation as a section or elevation sheet marks it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LevelMark {
    /// The level code (`1`, `2`, `R`), as plan titles give levels.
    pub level: String,
    pub elevation_ft: f64,
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

    /// True for stages whose items are project-level (not per-sheet).
    pub fn is_cross_discipline(self) -> bool {
        matches!(self, Stage::Clashes | Stage::Callouts)
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
    /// Sheet numbers on the index that have no file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_sheets: Vec<String>,
    /// Sheet numbers present but not on the index.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unlisted_sheets: Vec<String>,
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

// ─── Stage runner callback ──────────────────────────────────────────────────

/// Callback that runs one stage on one sheet. Returns the result for that sheet.
/// The production implementation loads DXF and calls readers; tests can inject a truth scorer.
pub type StageRunnerFn<'a> = dyn FnMut(Stage, &SheetEntry) -> SheetStageResult + 'a;

/// A null stage runner that returns Completed with 0 items (for when readers are stubs).
pub fn null_stage_runner(_stage: Stage, _sheet: &SheetEntry) -> SheetStageResult {
    SheetStageResult { stage: _stage.name().into(), status: StageStatus::Completed, items_found: 0, confidence: 0.0, ..SheetStageResult::default() }
}

// ─── Content hashing (streaming FNV-1a) ─────────────────────────────────────

/// Stream-hash a file on disk (FNV-1a, first 16 hex chars). Never reads the whole file at once.
/// Stable across runs, machines and builds (a job's checkpoint keeps it to tell its prints).
pub fn file_hash(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis
    let mut buf = [0u8; HASH_BUF];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x100000001b3); // FNV prime
        }
    }
    Ok(format!("{hash:016x}"))
}

/// Hash in-memory bytes (for tests and small data).
#[allow(dead_code)]
fn content_hash(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

// ─── Sheet classification ───────────────────────────────────────────────────

/// Classify a sheet's type from its title and number.
fn classify_sheet_type(number: &str, title: &str) -> String {
    let up = title.to_ascii_uppercase();
    if up.contains("COVER") || up.contains("INDEX") {
        return "cover".into();
    }
    if up.contains("SCHEDULE") || up.contains("NOTES") || up.contains("LEGEND") || up.contains("ABBREVIATION") || up.contains("SYMBOLS") {
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
    for &(prefix, code) in &[
        ("FP-", "FP"),
        ("FA-", "FA"),
        ("SL-", "SL"),
        ("M-", "M"),
        ("P-", "P"),
        ("S-", "S"),
        ("A-", "A"),
        ("G-", "G"),
        ("E-", "E"),
        ("T-", "T"),
        ("C-", "C"),
    ] {
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

// ─── Sheet index reading ────────────────────────────────────────────────────

/// Parse sheet numbers from a cover/index sheet's drawing list. Looks for text entities
/// that match sheet number patterns (e.g. "M-101", "E-201", "S-100").
pub fn read_sheet_index(drawing: &cadaddy_doc::Drawing) -> Vec<String> {
    let mut numbers = Vec::new();
    let is_sheet_number = |s: &str| -> bool {
        // Match patterns like G-001, A-100, S-201, M-101, P-101, FP-101, E-201, etc.
        let up = s.trim().to_ascii_uppercase();
        if up.len() < 3 {
            return false;
        }
        let parts: Vec<&str> = up.splitn(2, '-').collect();
        if parts.len() != 2 {
            return false;
        }
        let prefix = parts[0];
        let suffix = parts[1];
        let valid_prefix = matches!(prefix, "G" | "A" | "S" | "M" | "P" | "FP" | "E" | "FA" | "T" | "C" | "SL");
        let valid_suffix = suffix.len() >= 2 && suffix.chars().next().is_some_and(|c| c.is_ascii_digit());
        valid_prefix && valid_suffix
    };

    for entity in drawing.model.iter() {
        let text = match &entity.kind {
            cadaddy_doc::EntityKind::Text(t) => &t.value,
            cadaddy_doc::EntityKind::MText(t) => &t.contents,
            _ => continue,
        };
        // A drawing list may have multiple numbers per line, or one per row.
        for word in text.split_whitespace() {
            if is_sheet_number(word) && !numbers.contains(&word.to_ascii_uppercase()) {
                numbers.push(word.to_ascii_uppercase());
            }
        }
    }
    numbers.sort();
    numbers
}

/// Compare the sheet index against actual sheets. Returns (missing, unlisted).
pub fn compare_index(project: &PrintSetProject) -> (Vec<String>, Vec<String>) {
    let present: std::collections::HashSet<String> = project.sheets.iter().map(|s| s.number.to_ascii_uppercase()).collect();
    let indexed: std::collections::HashSet<String> = project.sheet_index.iter().map(|s| s.to_ascii_uppercase()).collect();

    let missing: Vec<String> = indexed.difference(&present).cloned().collect();
    let unlisted: Vec<String> = present.difference(&indexed).filter(|n| !n.is_empty()).cloned().collect();
    (missing, unlisted)
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
    here.sort_by(|(a, _), (b, _)| natural_cmp(a, b));
    folders.sort_by(|(a, _), (b, _)| natural_cmp(a, b));
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

/// Compare names the way people number them: runs of digits by value (`Revision 2` before
/// `Revision 10`, `ASI 02` with `ASI 2`), the rest ignoring case; exact text breaks ties.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    while i < ab.len() && j < bb.len() {
        let (ca, cb) = (ab.get(i).copied().unwrap_or(0), bb.get(j).copied().unwrap_or(0));
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let si = i;
            while ab.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            let sj = j;
            while bb.get(j).is_some_and(u8::is_ascii_digit) {
                j += 1;
            }
            let da = trim_zeros(ab.get(si..i).unwrap_or_default());
            let db = trim_zeros(bb.get(sj..j).unwrap_or_default());
            // Longer digit runs (leading zeros dropped) are bigger; same length compares by digit.
            let ord = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
            if ord != Ordering::Equal {
                return ord;
            }
            continue;
        }
        let ord = ca.to_ascii_lowercase().cmp(&cb.to_ascii_lowercase());
        if ord != Ordering::Equal {
            return ord;
        }
        i += 1;
        j += 1;
    }
    (ab.len() - i.min(ab.len())).cmp(&(bb.len() - j.min(bb.len()))).then_with(|| a.cmp(b))
}

fn trim_zeros(d: &[u8]) -> &[u8] {
    let n = d.iter().take_while(|c| **c == b'0').count();
    d.get(n..).unwrap_or_default()
}

/// The order [`discover_sheets`] lists two project paths in (`Revision 1/M-102.pdf`): folder by
/// folder in [`natural_cmp`] order, a folder's own files before its subfolders.
pub fn path_order(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (pa, pb): (Vec<&str>, Vec<&str>) = (a.split('/').collect(), b.split('/').collect());
    for k in 0..pa.len().max(pb.len()) {
        let (Some(x), Some(y)) = (pa.get(k), pb.get(k)) else { break };
        let (x_file, y_file) = (k + 1 == pa.len(), k + 1 == pb.len());
        let ord = match (x_file, y_file) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => natural_cmp(x, y),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    pa.len().cmp(&pb.len())
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
pub fn create_from_truth(truth: &crate::truth::Truth) -> PrintSetProject {
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
    let start = web_time::Instant::now();
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
                    // Read the sheet index from cover/index sheets.
                    if rec.title.to_ascii_uppercase().contains("INDEX")
                        || rec.title.to_ascii_uppercase().contains("COVER")
                        || rec.title.to_ascii_uppercase().contains("DRAWING LIST")
                    {
                        let idx = read_sheet_index(&drawing);
                        if !idx.is_empty() {
                            project.sheet_index = idx;
                        }
                    }
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
                        // Reference sheets (life safety, occupancy load, etc.) mention a
                        // floor in their title but are not architectural floor plans — strip
                        // the building level so they don't drive the BIM model.
                        if sheet_facts::is_reference_sheet(&e.title) {
                            e.level = String::new();
                        }
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
            if sheet_facts::is_reference_sheet(&e.title) {
                e.level = String::new();
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
    let start = web_time::Instant::now();
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
        let facts = PageFacts::read(&drawing);
        for (mut rec, _) in sheet_reader::split_views(&drawing) {
            let mut e = entry.clone();
            let view = !rec.printed_number.is_empty();
            // The title block names the sheet (sheet facts); the sheet reader's own number is the
            // fallback, and a view of a several-view sheet keeps its `<number>/<level>`.
            if let Some(id) = &facts.ident {
                if view {
                    if rec.printed_number != id.number {
                        rec.number = rec.number.replacen(&rec.printed_number, &id.number, 1);
                        rec.printed_number = id.number.clone();
                    }
                } else {
                    rec.number = id.number.clone();
                }
                if !id.title.is_empty() && !view {
                    rec.title = id.title.clone();
                }
                if !id.discipline.is_empty() {
                    rec.discipline = canonical_discipline(&id.discipline);
                }
            }
            e.number = if rec.number.is_empty() { format!("{}#{}", entry.file, entry.page) } else { rec.number.clone() };
            e.title = rec.title.clone();
            let printed = if rec.printed_number.is_empty() { rec.number.as_str() } else { rec.printed_number.as_str() };
            e.discipline = if rec.discipline.is_empty() { infer_discipline(printed) } else { rec.discipline.clone() };
            // A consultant's title block may carry no title: the view titles say what is drawn.
            let type_text = if e.title.is_empty() { facts.view_titles.join(" / ") } else { e.title.clone() };
            e.sheet_type = classify_sheet_type(printed, &type_text);
            if view && e.sheet_type == "unknown" {
                // One view of a several-view sheet is a plan of its level.
                e.sheet_type = "plan".into();
            }
            if view {
                e.level = rec.level.clone().unwrap_or_else(|| infer_level(&e.title));
                e.levels = rec.levels.clone();
            } else {
                // The sheet title's levels, else the plan views' titles' (`FIRST FLOOR POWER PLAN`).
                // Q6018: multi-level assignment (typical floors) only from the plan title, not
                // from notes or different view titles on the sheet — a "TYP." note elsewhere
                // must not make a floor-specific sheet cover several levels.
                let mut levels = sheet_reader::detect_levels(&e.title);
                if levels.is_empty() && is_plan_type(&e.sheet_type) {
                    // A key plan (the small floor plan marking where an enlarged plan or detail
                    // is) names its floor, not the sheet's.
                    for t in facts.view_titles.iter().filter(|t| !is_key_plan(t)) {
                        let vl = sheet_reader::detect_levels(t);
                        if !vl.is_empty() {
                            // Use the first view title's level only; multi-level comes from
                            // the plan title or the typical-floor sentinel, not from combining
                            // different views' titles (which are already split by recognise_views
                            // when they name different levels).
                            levels = if sheet_reader::is_typical_sentinel(&vl) {
                                vl
                            } else if let Some(first) = vl.into_iter().next() {
                                vec![first]
                            } else {
                                continue;
                            };
                            break;
                        }
                    }
                }
                if levels.is_empty() {
                    levels = rec.levels.clone();
                }
                e.level = levels.first().cloned().or_else(|| rec.level.clone()).unwrap_or_else(|| infer_level(&type_text));
                e.levels = levels;
            }
            // Reference sheets (life safety, occupancy load, code analysis) mention a floor
            // in their title but should not be on a building level.
            if sheet_facts::is_reference_sheet(&e.title) {
                e.level = String::new();
                e.levels = Vec::new();
            }
            e.level_source = if e.level.is_empty() { String::new() } else { "title".into() };
            e.issued = facts.issued.clone();
            e.level_marks = if matches!(e.sheet_type.as_str(), "section" | "elevation") { facts.level_marks.clone() } else { Vec::new() };
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
    // Later files replace earlier sheets of the same number. "Later" is folder order with
    // numbers by value (`Revision 10/` after `Revision 2/`), not the order entries were added
    // (a re-import appends), so the same folder always gives the same current sheets.
    // A later printed date (an issue or revision date) decides first: the same sheet in a full
    // set and a discipline set keeps the newer print.
    let mut latest: BTreeMap<String, usize> = BTreeMap::new();
    for (i, e) in out.iter().enumerate() {
        if !e.number.is_empty() && e.recognised.as_ref().is_some_and(|r| !r.number.is_empty()) {
            let later = match latest.get(&e.number).and_then(|&j| out.get(j)) {
                Some(prev) if by_date(e, prev) => e.issued > prev.issued,
                Some(prev) => path_order(&e.file, &prev.file).then(e.page.cmp(&prev.page)).is_ge(),
                None => true,
            };
            if later {
                latest.insert(e.number.clone(), i);
            }
        }
    }
    let names: Vec<(String, u32, String)> = out.iter().map(|e| (e.file.clone(), e.page, e.issued.clone())).collect();
    for (i, e) in out.iter_mut().enumerate() {
        (e.superseded_by, e.superseded_reason) = match latest.get(&e.number) {
            Some(&j) if j != i && names.get(j).is_some_and(|(f, _, _)| *f != e.file) => {
                let by = names.get(j).map(|(f, p, _)| if *p == 0 { f.clone() } else { format!("{f}#{p}") }).unwrap_or_default();
                let (later, issued) = names.get(j).map(|(f, _, d)| (f.as_str(), d.as_str())).unwrap_or_default();
                let why = if !issued.is_empty() && !e.issued.is_empty() && issued != e.issued {
                    format!("revision date: {later} is dated {issued}, {} {}", e.file, e.issued)
                } else {
                    format!(
                        "path order: {later} comes after {} (folders and files in name order, numbers by value; revision marks are not compared)",
                        e.file
                    )
                };
                (by, why)
            }
            _ => (String::new(), String::new()),
        };
    }
    level_by_number(&mut out);
    // Expand "typical floor" sentinels: a sheet with levels == ["T"] gets the levels of every
    // numeric floor that no other sheet of the same discipline already covers.
    expand_typical_levels(&mut out);
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

// ─── Sheet index ─────────────────────────────────────────────────────────────

/// Reads the project's prints for [`run_classify_prints`]: a page of a PDF (no images, markups
/// or layout: only what the title block and views say), or a drawing file. The file read last
/// stays in memory, so a merged set's pages are read from one copy.
pub struct PrintLoader {
    dir: PathBuf,
    last: std::cell::RefCell<Option<(String, std::rc::Rc<Vec<u8>>)>>,
}

impl PrintLoader {
    pub fn new(dir: &Path) -> PrintLoader {
        PrintLoader { dir: dir.to_path_buf(), last: std::cell::RefCell::new(None) }
    }

    pub fn load(&self, e: &SheetEntry) -> Result<cadaddy_doc::Drawing, String> {
        if e.file.split(['/', '\\']).any(|part| part == "..") {
            return Err(format!("{}: outside the project folder", e.file));
        }
        let cached = self.last.borrow().as_ref().filter(|(f, _)| *f == e.file).map(|(_, b)| b.clone());
        let bytes = match cached {
            Some(b) => b,
            None => {
                let path = self.dir.join(&e.file);
                if path.metadata().map_or(true, |m| m.len() > MAX_FILE_BYTES) {
                    return Err(format!("{}: missing or too large", e.file));
                }
                let b = std::rc::Rc::new(std::fs::read(&path).map_err(|err| format!("{}: {err}", e.file))?);
                *self.last.borrow_mut() = Some((e.file.clone(), b.clone()));
                b
            }
        };
        if e.file.to_ascii_lowercase().ends_with(".pdf") {
            let opts = cadaddy_io::pdf_read::PdfOptions { page: e.page.max(1), images: false, markups: false, layout: false, ..Default::default() };
            return cadaddy_io::pdf_read::read(&bytes, &opts).map(|i| i.drawing).map_err(|err| err.to_string());
        }
        cadaddy_io::read(&bytes, &e.file).map_err(|err| err.to_string())
    }
}

/// Short texts looked at as level marks per page (a guard against hostile files).
const MAX_MARK_TEXTS: usize = 400;

/// What a page says about itself, read once for all of its entries.
struct PageFacts {
    /// The title block's number, title and discipline ([`crate::sheet_facts`]).
    ident: Option<crate::sheet_facts::SheetIdent>,
    /// The titles of the views drawn on it.
    view_titles: Vec<String>,
    /// The latest date printed on it (`YYYY-MM-DD`).
    issued: String,
    level_marks: Vec<LevelMark>,
}

impl PageFacts {
    fn read(d: &cadaddy_doc::Drawing) -> PageFacts {
        let texts = crate::sheet_facts::texts(d);
        let ident = crate::sheet_facts::identify_sheet(&texts);
        let view_titles = crate::sheet_facts::view_titles(d, &texts).into_iter().map(|v| v.title).collect();
        let issued = texts.iter().filter_map(|t| latest_date(&t.value)).max().unwrap_or_default();
        PageFacts { ident, view_titles, issued, level_marks: level_marks(&texts) }
    }
}

/// The discipline code a sheet number's letters stand for (`A` from `A111`, `FP` from `FP-1`).
fn canonical_discipline(letters: &str) -> String {
    letters.trim().to_ascii_uppercase()
}

fn is_plan_type(t: &str) -> bool {
    matches!(t, "plan" | "enlarged_plan" | "rcp")
}

/// Whether a sheet of this type shows a whole level and so defines and fills it (a floor plan,
/// a reflected ceiling plan, a trade's plan): an enlarged plan shows part of its parent floor,
/// and details, sections, elevations, schedules, legends, notes and covers are of no level.
pub fn is_level_plan_type(t: &str) -> bool {
    matches!(t, "plan" | "rcp")
}

/// Whether a view title is a key plan's (`KEY PLAN`, `KEYPLAN - LEVEL 2`).
fn is_key_plan(title: &str) -> bool {
    let up = title.to_ascii_uppercase();
    up.contains("KEY PLAN") || up.contains("KEYPLAN") || up.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "KEY")
}

/// Level codes in building order: basements, numbered floors, then the roof.
fn level_key(c: &str) -> (bool, u32, String) {
    (c == "R", c.parse::<u32>().unwrap_or(u32::MAX), c.to_string())
}

/// Whether two copies of a sheet are told apart by their printed dates.
fn by_date(a: &SheetEntry, b: &SheetEntry) -> bool {
    !a.issued.is_empty() && !b.issued.is_empty() && a.issued != b.issued
}

/// The latest date in a text (`10/24/2026`, `10-24-26`, `2026-10-24`, `10.24.2026`) as
/// `YYYY-MM-DD`.
pub fn latest_date(text: &str) -> Option<String> {
    let mut best: Option<String> = None;
    for word in text.split(|c: char| !(c.is_ascii_digit() || matches!(c, '/' | '-' | '.'))).take(200) {
        let w = word.trim_matches(|c: char| !c.is_ascii_digit());
        let Some(sep) = w.chars().find(|c| matches!(c, '/' | '-' | '.')) else { continue };
        let parts: Vec<&str> = w.split(sep).collect();
        let [a, b, c] = parts.as_slice() else { continue };
        let num = |s: &str| (!s.is_empty() && s.len() <= 4 && s.chars().all(|c| c.is_ascii_digit())).then(|| s.parse::<u32>().ok()).flatten();
        let (Some(x), Some(y), Some(z)) = (num(a), num(b), num(c)) else { continue };
        let (year, month, day) = if a.len() == 4 {
            (x, y, z)
        } else if c.len() == 4 || c.len() == 2 {
            (if c.len() == 2 { 2000 + z } else { z }, x, y)
        } else {
            continue;
        };
        if !(1980..=2100).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) || (a.len() != 4 && a.len() > 2) || b.len() > 2 {
            continue;
        }
        let d = format!("{year:04}-{month:02}-{day:02}");
        if best.as_ref().is_none_or(|b| d > *b) {
            best = Some(d);
        }
    }
    best
}

/// The level code of a canonical level name (`LEVEL 2` → `2`, `ROOF` → `R`); none for a datum
/// that is no storey of its own (a parapet, a low roof).
fn code_of_level(name: &str) -> Option<String> {
    if let Some(n) = name.strip_prefix("LEVEL ") {
        return Some(n.to_string());
    }
    match name {
        "ROOF" => Some("R".into()),
        "BASEMENT" => Some("B1".into()),
        "MEZZANINE" => Some("M".into()),
        "PENTHOUSE" => Some("PH".into()),
        _ => name.strip_prefix("BASEMENT ").map(|n| format!("B{n}")),
    }
}

/// Level marks on a section or elevation: a level note in one text (`SECOND FLOOR EL. 110'-8"`)
/// or a level name with its elevation right beside it (the two lines of a level marker).
fn level_marks(texts: &[crate::sheet_facts::TextItem]) -> Vec<LevelMark> {
    use cadaddy_walk3d::levels::{canonical_level, parse_elevation, parse_level_note};
    let mut out = Vec::new();
    let short: Vec<&crate::sheet_facts::TextItem> =
        texts.iter().filter(|t| t.value.len() <= 40 && !t.value.contains('=')).take(MAX_MARK_TEXTS * 50).collect();
    let mut looked = 0usize;
    for t in &short {
        if canonical_level(&t.value).is_none() {
            continue;
        }
        looked += 1;
        if looked > MAX_MARK_TEXTS {
            break;
        }
        let note = parse_level_note(&t.value).or_else(|| {
            short
                .iter()
                .filter(|u| {
                    u.at != t.at
                        && u.at.dist(t.at) <= 4.0 * t.height.max(u.height)
                        && canonical_level(&u.value).is_none()
                        && parse_elevation(&u.value).is_some()
                })
                .min_by(|a, b| a.at.dist(t.at).total_cmp(&b.at.dist(t.at)))
                .and_then(|u| parse_level_note(&format!("{} {}", t.value, u.value)))
        });
        if let Some(n) = note
            && let Some(level) = code_of_level(&n.name)
        {
            out.push(LevelMark { level, elevation_ft: n.elevation_ft });
        }
    }
    out
}

/// Plan sheets of a numbered series whose titles name no level take it from their number: in a
/// series of two or more (`A111`…`A114`) the last digit is the level.
fn level_by_number(sheets: &mut [SheetEntry]) {
    let series = |e: &SheetEntry| -> Option<(String, u32)> {
        if !e.level.is_empty() || !e.superseded_by.is_empty() || !is_plan_type(&e.sheet_type) || e.number.contains('/') {
            return None;
        }
        let letters: String = e.number.chars().take_while(char::is_ascii_uppercase).collect();
        let digits = e.number.get(letters.len()..)?.trim_start_matches('-');
        let level = digits.chars().nth(2).and_then(|c| c.to_digit(10))?;
        (!letters.is_empty() && digits.len() == 3 && digits.starts_with('1') && digits.chars().all(|c| c.is_ascii_digit()) && level >= 1)
            .then(|| (format!("{letters}{}", digits.get(..2).unwrap_or_default()), level))
    };
    let mut groups: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for e in sheets.iter() {
        if let Some((g, l)) = series(e) {
            groups.entry(g).or_default().push(l);
        }
    }
    for e in sheets.iter_mut() {
        let Some((g, l)) = series(e) else { continue };
        let mut distinct = groups.get(&g).cloned().unwrap_or_default();
        distinct.sort_unstable();
        distinct.dedup();
        if distinct.len() >= 2 {
            e.level = l.to_string();
            e.levels = vec![e.level.clone()];
            e.level_source = "number".into();
        }
    }
}

/// Expand the "typical floor" sentinel (`levels == ["T"]`): for each such sheet, determine which
/// numeric levels it covers by looking at all plan sheets of the same discipline that already
/// have a dedicated level. The typical sheet gets every other numeric level in the set.
fn expand_typical_levels(sheets: &mut [SheetEntry]) {
    // Collect all known numeric levels from non-superseded plan sheets.
    let mut all_numeric: Vec<String> = Vec::new();
    for e in sheets.iter().filter(|s| s.superseded_by.is_empty() && is_plan_type(&s.sheet_type)) {
        for l in &e.levels {
            if l != "T" && l.parse::<u32>().is_ok() && !all_numeric.contains(l) {
                all_numeric.push(l.clone());
            }
        }
        // Also check the primary level.
        if !e.level.is_empty() && e.level != "T" && e.level.parse::<u32>().is_ok() && !all_numeric.contains(&e.level) {
            all_numeric.push(e.level.clone());
        }
    }
    all_numeric.sort_by_key(|l| l.parse::<u32>().unwrap_or(u32::MAX));
    all_numeric.dedup();
    if all_numeric.is_empty() {
        return;
    }
    // For each "typical" sheet, expand against the discipline's dedicated levels.
    let typical_indices: Vec<usize> = sheets
        .iter()
        .enumerate()
        .filter(|(_, e)| e.superseded_by.is_empty() && is_plan_type(&e.sheet_type) && sheet_reader::is_typical_sentinel(&e.levels))
        .map(|(i, _)| i)
        .collect();
    for idx in typical_indices {
        let disc = sheets[idx].discipline.clone();
        // Levels that already have a dedicated plan of this discipline.
        let dedicated: Vec<String> = sheets
            .iter()
            .filter(|e| {
                e.superseded_by.is_empty()
                    && is_plan_type(&e.sheet_type)
                    && e.discipline.eq_ignore_ascii_case(&disc)
                    && !sheet_reader::is_typical_sentinel(&e.levels)
            })
            .flat_map(|e| e.levels.iter().cloned())
            .filter(|l| l.parse::<u32>().is_ok())
            .collect();
        let expanded = sheet_reader::expand_typical(&all_numeric, |l| dedicated.iter().any(|d| d == l));
        if expanded.is_empty() {
            // No expansion possible: fall back to all numeric levels.
            sheets[idx].levels = all_numeric.clone();
        } else {
            sheets[idx].levels = expanded;
        }
        // Primary level = first of the expanded list.
        if let Some(first) = sheets[idx].levels.first() {
            sheets[idx].level = first.clone();
        }
        sheets[idx].level_source = "typical".into();
    }
}

/// One real sheet of the set: every copy of it (the full set and a discipline set print the same
/// sheet) is one entry.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct IndexSheet {
    pub number: String,
    pub title: String,
    pub discipline: String,
    #[serde(rename = "type")]
    pub sheet_type: String,
    /// The level the sheet shows (the first when it shows several); empty for a sheet of no one
    /// level (cover, schedules, details).
    pub level: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<String>,
    /// `title`, `number`, `typical`, `site` (a civil, landscape or site plan: level `SITE`, a
    /// datum), `default` (a plan whose title names no level in a building of one level: that
    /// level, to be confirmed) or `ask` (the same in a building of several levels: no level
    /// until someone says which).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub level_source: String,
    /// Every copy as `file#page` (the file alone for a one-sheet file), the current one first.
    pub sources: Vec<String>,
    /// Copies an other copy replaces (a later revision date, else a later file).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub superseded: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub issued: String,
}

/// A building level the plans show.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct IndexLevel {
    /// Level code: `1`, `2`, `R`.
    pub name: String,
    /// The plan sheets of the level: `[{sheet, typical}]` (Q6018).
    pub sheets: Vec<serde_json::Value>,
    /// Finished floor elevation from the sections' and elevations' level marks (above the
    /// first floor when the prints put it at 100'-0").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elevation_ft: Option<f64>,
    /// Floor to floor: the next level's elevation less this one's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height_ft: Option<f64>,
}

/// The set as sheets: one entry per real sheet, the levels its plans show.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SheetIndex {
    pub sheets: Vec<IndexSheet>,
    pub levels: Vec<IndexLevel>,
    /// Pages and files read.
    pub pages: usize,
    /// Entries whose title block gave no sheet number.
    pub unidentified: usize,
    /// Plan sheets whose title and number name no level (`default` or `ask`, to confirm).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plans_without_level: Vec<String>,
    /// Level marks not used: they break the levels' order (elevations rise with the level).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub level_warnings: Vec<String>,
}

fn source_of(e: &SheetEntry) -> String {
    if e.page == 0 { e.file.clone() } else { format!("{}#{}", e.file, e.page) }
}

/// The project's sheet index (after [`run_classify_prints`]): copies of a sheet merged by
/// number, the views of a several-view sheet back into their sheet, each plan with its level,
/// and the levels with their elevations where sections or elevations mark them.
pub fn sheet_index(project: &PrintSetProject) -> SheetIndex {
    let base = |e: &SheetEntry| -> String {
        match e.recognised.as_ref().map(|r| r.printed_number.as_str()).filter(|p| !p.is_empty()) {
            Some(p) => p.to_string(),
            None if e.number.is_empty() => source_of(e),
            None => e.number.clone(),
        }
    };
    let mut order: Vec<String> = Vec::new();
    let mut groups: BTreeMap<String, Vec<&SheetEntry>> = BTreeMap::new();
    for e in project.sheets.iter().take(MAX_SHEETS) {
        let k = base(e);
        if !groups.contains_key(&k) {
            order.push(k.clone());
        }
        groups.entry(k).or_default().push(e);
    }
    let mut out = SheetIndex { pages: project.sheets.len(), ..SheetIndex::default() };
    for k in &order {
        let Some(members) = groups.get(k) else { continue };
        let current: Vec<&SheetEntry> = members.iter().copied().filter(|e| e.superseded_by.is_empty()).collect();
        let Some(first) = current.first().or_else(|| members.first()).copied() else { continue };
        let views = current.len() > 1 && current.iter().any(|e| e.recognised.as_ref().is_some_and(|r| !r.printed_number.is_empty()));
        let mut levels: Vec<String> = Vec::new();
        for e in &current {
            levels.extend(e.levels.iter().cloned());
            if !e.level.is_empty() {
                levels.push(e.level.clone());
            }
        }
        levels.sort_by_key(|c| level_key(c));
        levels.dedup();
        let mut sources: Vec<String> = Vec::new();
        for e in current.iter().copied().chain(members.iter().copied().filter(|e| !e.superseded_by.is_empty())) {
            let s = source_of(e);
            if !sources.contains(&s) {
                sources.push(s);
            }
        }
        let mut superseded: Vec<String> = members.iter().filter(|e| !e.superseded_by.is_empty()).map(|e| source_of(e)).collect();
        superseded.dedup();
        if first.recognised.as_ref().is_none_or(|r| r.number.is_empty()) {
            out.unidentified += 1;
        }
        let mut level = if first.level.is_empty() { levels.first().cloned().unwrap_or_default() } else { first.level.clone() };
        let sheet_type = if views { "plan".to_string() } else { first.sheet_type.clone() };
        // Only plans attach to a level: a detail, section, schedule, legend, cover or notes
        // sheet that names one (`LEVEL 1 DETAILS`, a key plan) is a reference sheet.
        if !is_plan_type(&sheet_type) {
            level.clear();
            levels.clear();
        }
        out.sheets.push(IndexSheet {
            number: k.clone(),
            title: first.title.clone(),
            discipline: first.discipline.clone(),
            sheet_type,
            level_source: if level.is_empty() {
                String::new()
            } else {
                current.iter().map(|e| e.level_source.clone()).find(|s| !s.is_empty()).unwrap_or_else(|| "title".into())
            },
            level,
            levels,
            sources,
            superseded,
            issued: first.issued.clone(),
        });
    }
    // A civil, landscape or site plan is on the site (a datum, no storey of the building), even
    // when its title names a floor.
    for s in out.sheets.iter_mut().filter(|s| is_plan_type(&s.sheet_type) && crate::sheet_facts::is_site_sheet(&s.discipline, &s.title)) {
        s.level = "SITE".into();
        s.levels.clear();
        s.level_source = "site".into();
    }
    // The building's levels: what the floor plans show (the site is no storey; an enlarged
    // plan is part of its parent floor and defines no level).
    // Q6018: each sheet entry is {sheet, typical} so the gate can distinguish a
    // multi-level typical sheet from a floor-specific one.
    let mut by_level: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    for s in out.sheets.iter().filter(|s| is_level_plan_type(&s.sheet_type)) {
        let typical = s.levels.len() > 1;
        for l in s.levels.iter().chain(std::iter::once(&s.level)).filter(|l| !l.is_empty() && !cadaddy_walk3d::levels::is_datum_name(l)) {
            let v = by_level.entry(l.clone()).or_default();
            if !v.iter().any(|x| x.get("sheet").and_then(serde_json::Value::as_str) == Some(&s.number)) {
                v.push(serde_json::json!({"sheet": s.number, "typical": typical}));
            }
        }
    }
    let mut codes: Vec<String> = by_level.keys().cloned().collect();
    codes.sort_by_key(|c| level_key(c));
    // Elevations: the value most marks give per level; above the first floor at 100'-0". A
    // civil or site sheet's marks are spot elevations on the site datum, never the levels'.
    let mut marks: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let site = |e: &SheetEntry| e.level.eq_ignore_ascii_case("SITE") || cadaddy_walk3d::levels::site_sheet(&e.discipline, &e.title);
    for e in project.sheets.iter().filter(|e| e.superseded_by.is_empty() && !site(e)) {
        for m in &e.level_marks {
            marks.entry(m.level.clone()).or_default().push(m.elevation_ft);
        }
    }
    let mut elevation: BTreeMap<String, f64> = BTreeMap::new();
    for (l, vals) in &marks {
        let mut best: Option<(f64, usize)> = None;
        for v in vals {
            let n = vals.iter().filter(|w| (*w - v).abs() < 1.0 / 96.0).count();
            if best.is_none_or(|(_, bn)| n > bn) {
                best = Some((*v, n));
            }
        }
        if let Some((v, _)) = best {
            elevation.insert(l.clone(), v);
        }
    }
    // Elevations rise with the level: the most marks that agree win; the others are dropped.
    let marked: Vec<(String, f64)> = elevation.iter().map(|(l, e)| (l.clone(), *e)).collect();
    let items: Vec<(Option<i32>, f64, u32)> = marked
        .iter()
        .map(|(l, e)| (cadaddy_walk3d::levels::code_rank(l), *e, u32::try_from(marks.get(l).map_or(1, Vec::len)).unwrap_or(u32::MAX)))
        .collect();
    for ((l, e), keep) in marked.iter().zip(cadaddy_walk3d::levels::in_order(&items)) {
        if !keep {
            elevation.remove(l);
            out.level_warnings
                .push(format!("level {l}: {} not used: it breaks the levels' order (elevations rise with the level)", cadaddy_walk3d::feet(*e)));
        }
    }
    let datum = if elevation.get("1").is_some_and(|e| (e - 100.0).abs() < 1e-6) { 100.0 } else { 0.0 };
    for (i, c) in codes.iter().enumerate() {
        let el = elevation.get(c).map(|e| e - datum);
        let next = codes.get(i + 1).and_then(|n| elevation.get(n)).map(|e| e - datum);
        out.levels.push(IndexLevel {
            name: c.clone(),
            sheets: by_level.get(c).cloned().unwrap_or_default(),
            elevation_ft: el,
            height_ft: el.zip(next).map(|(a, b)| b - a).filter(|h| *h > 0.0),
        });
    }
    // A plan whose title names no level: in a building of one level it can only be that one
    // (given it, to be confirmed); with several the level is uncertain, so the sheet stays off
    // every level until someone says which (`ask`), never put on the lowest. An enlarged plan
    // with no level belongs to its parent floor's region, found by where it is drawn.
    let lowest = codes.iter().find(|c| c.parse::<u32>().is_ok()).cloned().unwrap_or_else(|| "1".into());
    let one_level = codes.len() <= 1;
    for s in out.sheets.iter_mut().filter(|s| is_level_plan_type(&s.sheet_type) && s.level.is_empty()) {
        if one_level {
            s.level = lowest.clone();
            s.level_source = "default".into();
        } else {
            s.level_source = "ask".into();
        }
        out.plans_without_level.push(s.number.clone());
    }
    out
}

// ─── Run stages (production API — no Truth) ──────────────────────────────────

/// Run recognition stages on the project's sheets via `runner`. Each stage processes sheets that
/// match its discipline filter; cross-discipline stages (clashes, callouts) run once for the
/// whole project, not per-sheet. If `resume` is true, completed non-dirty sheets are skipped.
/// The project is updated in place after each sheet (for persistence/progress).
pub fn run_stages(
    project: &mut PrintSetProject,
    stages: &[Stage],
    runner: &mut StageRunnerFn<'_>,
    cancel: &AtomicBool,
    progress: &mut ProgressFn<'_>,
    resume: bool,
) -> Vec<StageRecord> {
    let mut records = Vec::new();

    for &stage in stages {
        if stage == Stage::Classify {
            continue; // Classify uses run_classify, not run_stages.
        }
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let start = web_time::Instant::now();
        let stage_name = stage.name();
        let disciplines = stage.disciplines();

        // Filter sheets by discipline and type.
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

        // For cross-discipline stages, items are counted once for the whole project.
        // We call the runner on each sheet but only count items on the FIRST sheet processed
        // (to avoid duplicate counting of project-level items like clashes).
        let mut cross_counted = false;

        for &idx in &sheet_indices {
            if cancel.load(Ordering::Relaxed) {
                break;
            }

            let entry = match project.sheets.get(idx) {
                Some(e) => e.clone(),
                None => continue,
            };

            // Resume: skip completed non-dirty sheets.
            if resume
                && !entry.dirty
                && let Some(existing) = entry.stage_results.get(stage_name)
                && existing.status == StageStatus::Completed
            {
                processed += 1;
                if !stage.is_cross_discipline() || !cross_counted {
                    total_items += existing.items_found;
                    if stage.is_cross_discipline() {
                        cross_counted = true;
                    }
                }
                total_findings += existing.findings.len();
                continue;
            }

            progress(&Progress {
                stage: stage_name.into(),
                sheet_index: processed,
                sheet_number: entry.number.clone(),
                sheets_total: total,
                items_so_far: total_items,
            });

            let result = runner(stage, &entry);

            // For cross-discipline stages, only count items once for the whole project.
            if !stage.is_cross_discipline() || !cross_counted {
                total_items += result.items_found;
                if stage.is_cross_discipline() {
                    cross_counted = true;
                }
            }
            total_findings += result.findings.len();

            if let Some(e) = project.sheets.get_mut(idx) {
                e.stage_results.insert(stage_name.into(), result);
                if !e.dirty {
                    // Stage ran successfully, mark clean.
                } else {
                    e.dirty = false;
                }
            }
            processed += 1;
        }

        let status = if cancel.load(Ordering::Relaxed) {
            StageStatus::Failed
        } else if processed == 0 {
            // 0 sheets processed ⟹ skipped/failed, never completed.
            if total == 0 { StageStatus::Skipped } else { StageStatus::Failed }
        } else {
            StageStatus::Completed
        };
        let rec = StageRecord {
            stage: stage_name.into(),
            status,
            sheets_processed: processed,
            sheets_total: total,
            items_found: total_items,
            findings: total_findings,
            elapsed_ms: start.elapsed().as_millis() as u64,
        };
        project.stages.push(rec.clone());
        update_dashboard(project);
        records.push(rec);
    }
    records
}

// ─── Truth-based stage running (test helper) ─────────────────────────────────

/// Run a MEP reading stage against truth data. This is a **test helper** — production code
/// uses [`run_stages`] with a real reader callback.
pub fn run_stage_from_truth(
    project: &mut PrintSetProject,
    stage: Stage,
    truth: &crate::truth::Truth,
    cancel: &AtomicBool,
    progress: &mut ProgressFn<'_>,
) -> StageRecord {
    let mut runner = |st: Stage, sh: &SheetEntry| -> SheetStageResult {
        let (items, confidence, findings) = count_truth_items(truth, st, &sh.level, &sh.number);
        SheetStageResult {
            stage: st.name().into(),
            status: StageStatus::Completed,
            items_found: items,
            confidence,
            findings,
            ..SheetStageResult::default()
        }
    };
    let recs = run_stages(project, &[stage], &mut runner, cancel, progress, false);
    recs.into_iter().next().unwrap_or_default()
}

/// Count truth items relevant to a stage on a given level (test helper for scoring).
pub fn count_truth_items(truth: &crate::truth::Truth, stage: Stage, level: &str, _sheet: &str) -> (usize, f64, Vec<Finding>) {
    let mut findings = Vec::new();
    match stage {
        Stage::Classify => (1, 1.0, findings),
        Stage::Ducts => {
            let n = truth.segments.iter().filter(|s| s.kind == "duct" && s.level == level).count();
            let equip =
                truth.equipment.iter().filter(|e| e.level == level && (e.kind == "ahu" || e.kind == "vav" || e.kind == "exhaust_fan")).count();
            let terms = truth.terminals.iter().filter(|t| t.level == level).count();
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
            // Clashes are project-level, not per-sheet. Return all clashes.
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

/// Run the full pipeline (all stages in order) against truth data (test helper).
pub fn run_all_from_truth(
    project: &mut PrintSetProject,
    truth: &crate::truth::Truth,
    cancel: &AtomicBool,
    progress: &mut ProgressFn<'_>,
) -> Vec<StageRecord> {
    let mut records = Vec::new();

    // Classify first (uses truth data directly since project was created from truth).
    let rec = run_classify(project, None, cancel, progress);
    records.push(rec);
    if cancel.load(Ordering::Relaxed) {
        return records;
    }

    // Then all MEP stages via run_stages with a truth runner.
    let mut runner = |st: Stage, sh: &SheetEntry| -> SheetStageResult {
        let (items, confidence, findings) = count_truth_items(truth, st, &sh.level, &sh.number);
        SheetStageResult {
            stage: st.name().into(),
            status: StageStatus::Completed,
            items_found: items,
            confidence,
            findings,
            ..SheetStageResult::default()
        }
    };
    let stage_recs = run_stages(project, &Stage::all()[1..], &mut runner, cancel, progress, false);
    records.extend(stage_recs);
    records
}

// ─── Dashboard update ────────────────────────────────────────────────────────

/// Recompute the dashboard from the project's current state.
pub fn update_dashboard(project: &mut PrintSetProject) {
    let mut d = Dashboard { total_sheets: project.sheets.len(), stages_total: Stage::all().len(), ..Dashboard::default() };

    // Per-level and per-discipline aggregation.
    let mut levels: BTreeMap<String, LevelSummary> = BTreeMap::new();
    let mut disciplines: BTreeMap<String, DisciplineSummary> = BTreeMap::new();

    // Confidence: collect (sum, count) for true mean.
    let mut level_conf: BTreeMap<String, (f64, usize)> = BTreeMap::new();
    let mut disc_conf: BTreeMap<String, (f64, usize)> = BTreeMap::new();
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

        // Per-stage results — only count per-discipline stages (not cross-discipline).
        for (stage_name, result) in &sheet.stage_results {
            if result.status != StageStatus::Completed {
                continue;
            }

            // Look up the Stage to check if it's cross-discipline.
            let is_cross = Stage::all().iter().find(|s| s.name() == stage_name).is_some_and(|s| s.is_cross_discipline());

            if !is_cross {
                d.total_items += result.items_found;
                total_confidence += result.confidence;
                confidence_count += 1;

                // Level items.
                if !sheet.level.is_empty() {
                    let ls =
                        levels.entry(sheet.level.clone()).or_insert_with(|| LevelSummary { level: sheet.level.clone(), ..LevelSummary::default() });
                    ls.items += result.items_found;
                    let (sum, cnt) = level_conf.entry(sheet.level.clone()).or_insert((0.0, 0));
                    *sum += result.confidence;
                    *cnt += 1;
                }

                // Discipline items.
                if !sheet.discipline.is_empty() && sheet.discipline != "unknown" {
                    let ds = disciplines
                        .entry(sheet.discipline.clone())
                        .or_insert_with(|| DisciplineSummary { discipline: sheet.discipline.clone(), ..DisciplineSummary::default() });
                    ds.items += result.items_found;
                    let (sum, cnt) = disc_conf.entry(sheet.discipline.clone()).or_insert((0.0, 0));
                    *sum += result.confidence;
                    *cnt += 1;
                    if !ds.stages_done.contains(stage_name) {
                        ds.stages_done.push(stage_name.clone());
                    }
                }
            }

            // Findings → gaps and RFIs (for all stages including cross-discipline).
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

    // Add cross-discipline stage items from StageRecords (counted once, not per-sheet).
    for rec in &project.stages {
        let is_cross = Stage::all().iter().find(|s| s.name() == rec.stage).is_some_and(|s| s.is_cross_discipline());
        if is_cross && rec.status == StageStatus::Completed {
            d.total_items += rec.items_found;
        }
    }

    // Clashes from the last clashes stage record (already the correct unique count).
    if let Some(clash_rec) = project.stages.iter().rev().find(|s| s.stage == "clashes") {
        d.total_clashes = clash_rec.items_found;
    }

    // Compute true mean confidence for levels and disciplines.
    for (level, ls) in &mut levels {
        if let Some(&(sum, cnt)) = level_conf.get(level) {
            ls.confidence = if cnt > 0 { sum / cnt as f64 } else { 0.0 };
        }
    }
    for (disc, ds) in &mut disciplines {
        if let Some(&(sum, cnt)) = disc_conf.get(disc) {
            ds.confidence = if cnt > 0 { sum / cnt as f64 } else { 0.0 };
        }
    }

    d.mean_confidence = if confidence_count > 0 { total_confidence / confidence_count as f64 } else { 0.0 };
    d.levels = levels.into_values().collect();
    d.disciplines = disciplines.into_values().collect();
    d.stages_completed = project.stages.iter().filter(|s| s.status == StageStatus::Completed).count();

    // Sheet index comparison.
    if !project.sheet_index.is_empty() {
        let (missing, unlisted) = compare_index(project);
        d.missing_sheets = missing;
        d.unlisted_sheets = unlisted;
    }

    project.dashboard = d;
}

// ─── Re-import (revised set) ─────────────────────────────────────────────────

/// Compute the delta between the current project and new files, keyed by **sheet number**
/// (from the title block, not the file name). Falls back to file name if the sheet has no number.
pub fn compute_delta(project: &PrintSetProject, new_files: &[(String, PathBuf)]) -> Result<RevisionDelta, String> {
    let mut delta = RevisionDelta::default();

    // Build a map of current sheets by number (primary) and file (fallback).
    let mut by_number: BTreeMap<String, &SheetEntry> = BTreeMap::new();
    let mut by_file: BTreeMap<&str, &SheetEntry> = BTreeMap::new();
    for s in &project.sheets {
        if !s.number.is_empty() {
            by_number.insert(s.number.to_ascii_uppercase(), s);
        }
        by_file.insert(&s.file, s);
    }

    // For each new file, try to match by sheet number first (requires reading DXF or
    // using a cached number). For now, match by hash + filename + number.
    for (name, path) in new_files {
        let new_hash = file_hash(path)?;
        // Try filename match first (most common).
        if let Some(existing) = by_file.get(name.as_str()) {
            if existing.content_hash == new_hash {
                delta.unchanged.push(existing.number_or_file());
            } else {
                delta.changed.push(existing.number_or_file());
            }
        } else {
            // New file — could be a reissue under a different name. Check by hash among
            // all sheets to see if content matches but name differs.
            let reissue = project.sheets.iter().find(|s| {
                !s.number.is_empty() && s.content_hash != new_hash && s.file != *name && name.contains(&s.number) // heuristic: new name contains old number
            });
            if let Some(existing) = reissue {
                delta.changed.push(existing.number_or_file());
            } else {
                delta.added.push(name.clone());
            }
        }
    }

    // Check for removed sheets: only if the caller marks this as a full set.
    // For addendum/partial imports, we do NOT mark absent sheets as removed.
    let new_names: std::collections::HashSet<&str> = new_files.iter().map(|(n, _)| n.as_str()).collect();
    // The caller decides full_set via apply_reimport's flag.
    // compute_delta alone does not remove — it reports what's missing from the new set.
    for sheet in &project.sheets {
        // A merged set has an entry per page: its file is removed once.
        let multi = project.sheets.iter().filter(|s| s.file == sheet.file).nth(1).is_some();
        let key = if multi { sheet.file.clone() } else { sheet.number_or_file() };
        if !new_names.contains(sheet.file.as_str()) && !delta.removed.contains(&key) {
            delta.removed.push(key);
        }
    }

    Ok(delta)
}

/// Apply a re-import. If `full_set` is true, sheets absent from the new folder are removed.
/// If `full_set` is false (addendum/partial), absent sheets are kept — only sheets whose content
/// changed are marked dirty, and new sheets are added.
pub fn apply_reimport(
    project: &mut PrintSetProject,
    new_files: &[(String, PathBuf)],
    revision_label: &str,
    full_set: bool,
) -> Result<RevisionDelta, String> {
    let mut delta = RevisionDelta::default();

    // Current entries by filename (a merged set has one entry per page).
    let mut current_files: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, s) in project.sheets.iter().enumerate() {
        current_files.entry(s.file.clone()).or_default().push(i);
    }
    let folder = |name: &str| name.rsplit_once('/').map_or("", |(d, _)| d).to_string();

    let mut seen_files: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (name, path) in new_files {
        let new_hash = file_hash(path)?;
        seen_files.insert(name.clone());

        if let Some(idxs) = current_files.get(name) {
            let key = match idxs.as_slice() {
                [one] => project.sheets.get(*one).map(SheetEntry::number_or_file).unwrap_or_else(|| name.clone()),
                _ => name.clone(),
            };
            let same = idxs.iter().filter_map(|&i| project.sheets.get(i)).all(|e| e.content_hash == new_hash);
            if same {
                delta.unchanged.push(key);
            } else {
                // Changed: update hash, mark dirty, clear stage results.
                delta.changed.push(key);
            }
            for &i in idxs {
                if let Some(e) = project.sheets.get_mut(i) {
                    if same {
                        // Unchanged sheets match the new set.
                        e.dirty = false;
                    } else {
                        e.content_hash = new_hash.clone();
                        e.dirty = true;
                        e.stage_results.clear();
                    }
                }
            }
        } else {
            // A reissue: a single-sheet file of the same folder whose new name carries its sheet
            // number. A file in another folder (a revision folder) is a new sheet; classify marks
            // the one it replaces as superseded.
            let up = name.to_ascii_uppercase();
            let reissue_idx = project.sheets.iter().position(|s| {
                !s.number.is_empty()
                    && s.page <= 1
                    && s.file != *name
                    && folder(&s.file) == folder(name)
                    && up.contains(&s.number.to_ascii_uppercase())
            });
            if let Some(e) = reissue_idx.and_then(|i| project.sheets.get_mut(i)) {
                // Reissue under a new file name = "changed".
                delta.changed.push(e.number_or_file());
                e.file = name.clone();
                e.content_hash = new_hash;
                e.dirty = true;
                e.stage_results.clear();
            } else {
                // Truly new sheet (one entry per page of a merged set).
                delta.added.push(name.clone());
                project.sheets.extend(entries_for_file(name, path));
            }
        }
    }

    // Only remove sheets if this is a full set import.
    if full_set {
        let mut removed = Vec::new();
        project.sheets.retain(|s| {
            if seen_files.contains(&s.file) {
                true
            } else {
                let key = s.number_or_file();
                if !removed.contains(&key) {
                    removed.push(key);
                }
                false
            }
        });
        delta.removed = removed;
    }
    // For partial/addendum: no removals. Absent sheets are kept.

    // Record the revision.
    project.revisions.push(RevisionRecord {
        revision: revision_label.into(),
        label: revision_label.into(),
        sheets_total: new_files.len(),
        sheets_changed: delta.changed.len(),
        sheets_added: delta.added.len(),
        sheets_removed: delta.removed.len(),
    });

    // Clear completed stages that touch changed sheets (they need to re-run).
    if !delta.changed.is_empty() || !delta.added.is_empty() {
        project.stages.retain(|s| s.stage == "classify");
    }

    project.revision = revision_label.into();
    project.dashboard.total_sheets = project.sheets.len();
    update_dashboard(project);

    Ok(delta)
}

impl SheetEntry {
    /// The sheet number if available, otherwise the filename.
    pub fn number_or_file(&self) -> String {
        if self.number.is_empty() { self.file.clone() } else { self.number.clone() }
    }
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
                        "page": s.page,
                        "levels": s.levels,
                        "level_source": s.level_source,
                        "issued": s.issued,
                        "superseded_by": s.superseded_by,
                        "superseded_reason": s.superseded_reason,
                        "stages": s.stage_results.keys().collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>()
        )
    }
}
