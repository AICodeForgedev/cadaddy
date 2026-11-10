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
    /// The number as printed when another page of the set prints it too on a different sheet (its
    /// title and contents differ, so it is no copy): this page keeps its own entry, numbered
    /// `<number>~2`, `~3` in page order, and the clash is reported. Empty otherwise.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub printed_as: String,
}

/// A level's elevation as a section or elevation sheet marks it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LevelMark {
    /// The level code (`1`, `2`, `R`), as plan titles give levels; [`MARK_FINISH_FLOOR`],
    /// [`MARK_PLATE`] or [`MARK_PARAPET`] for a finished floor, a top of plate (or truss bearing)
    /// or a top of parapet marked with no level number.
    pub level: String,
    pub elevation_ft: f64,
    /// For a top of parapet on an exterior elevation: the facade that elevation shows
    /// (`north`, `south`, `east`, `west`), from the view title nearest the mark; none when no
    /// title names one or two are as near.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facade: Option<String>,
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

/// The words that name a floor plan in a title.
fn floor_plan_words() -> &'static [&'static str] {
    crate::wordrules::words("sheets.floor_plan_title_words")
}

/// Classify a sheet's type from its title and number.
///
/// A title that lists parts (`FLOOR PLAN/ DETAILS/ SCHEDULES`, `ROOF PLAN & DETAILS`) names
/// everything on the sheet: when a part names a plan the sheet is that plan, and the other parts
/// are other views on it ([`title_views`]). The part that gives the kind is the one naming the
/// floor plan, else the first plan part (Q6514).
fn classify_sheet_type(number: &str, title: &str) -> String {
    let up = title.to_ascii_uppercase();
    if up.contains("COVER") || up.contains("INDEX") {
        return "cover".into();
    }
    if let Some(lead) = title_lead(number, title) {
        return lead.kind;
    }
    classify_one_part(number, &up)
}

/// The type a title of one part names (upper case).
fn classify_one_part(number: &str, up: &str) -> String {
    if up.contains("COVER") || up.contains("INDEX") {
        return "cover".into();
    }
    if up.contains("SCHEDULE") || up.contains("NOTES") || up.contains("LEGEND") || up.contains("ABBREVIATION") || up.contains("SYMBOLS") {
        return "schedule".into();
    }
    // A specification printed on the drawings (`ELECTRICAL SPECIFICATIONS`): read as a
    // specification, never a plan.
    let spec_words = crate::wordrules::words("sheets.specification_title_words");
    if up.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| spec_words.contains(&w)) {
        return "specification".into();
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
    // A sheet that also prints the floor plan is that floor's plan first.
    if (up.contains("REFLECTED CEILING") || up.contains("RCP")) && !sheet_reader::title_names(up, floor_plan_words()) {
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

/// Most parts a listed title is split into.
const MAX_TITLE_PARTS: usize = 16;

/// The parts a sheet title lists (upper case, trimmed), split at `/`, `,`, `&`, `+`, `;` and the
/// word `AND`. A slash beside a digit (`1/2`, a scale or a date; `1/E-6`, a callout) splits
/// nothing. One part: the title names one thing.
fn title_parts(title: &str) -> Vec<String> {
    let up = title.to_ascii_uppercase();
    let chars: Vec<char> = up.chars().collect();
    let mut spaced = String::with_capacity(up.len() + 8);
    for (i, c) in chars.iter().enumerate() {
        let digit = |j: Option<usize>| j.and_then(|j| chars.get(j)).is_some_and(char::is_ascii_digit);
        if matches!(c, '&' | '+' | ',' | ';') || (*c == '/' && !digit(i.checked_sub(1)) && !digit(i.checked_add(1))) {
            spaced.push_str(" & ");
        } else {
            spaced.push(*c);
        }
    }
    let mut parts: Vec<String> = Vec::new();
    for piece in spaced.split(" & ") {
        let words: Vec<&str> = piece.split_whitespace().collect();
        let mut part: Vec<&str> = Vec::new();
        for w in words.iter().chain(std::iter::once(&"AND")) {
            if *w == "AND" {
                if !part.is_empty() && parts.len() < MAX_TITLE_PARTS {
                    parts.push(part.join(" "));
                }
                part.clear();
            } else {
                part.push(w);
            }
        }
    }
    parts.retain(|p| p.chars().any(|c| c.is_ascii_alphanumeric()));
    parts
}

/// The part of a listed title that names the sheet's plan, with the kind it gives.
struct TitleLead {
    /// `plan`, `rcp` or `enlarged_plan`.
    kind: String,
    /// The part's index among [`title_parts`].
    index: usize,
}

/// Each part's own type (a part naming no kind of drawing is `unknown`).
fn part_kinds(number: &str, parts: &[String]) -> Vec<String> {
    parts.iter().map(|p| if is_key_plan(p) { "key_plan".to_string() } else { classify_one_part(number, p) }).collect()
}

/// The plan a title that lists parts names (`None` for a title of one part, or one that lists no
/// plan): the part naming the floor plan, else the first part naming a plan (a key plan names
/// none). An enlarged plan beside a part that names no kind of drawing (`EQUIPMENT/ ENLARGED
/// STORAGE`) is a detail of the floor that part draws whole: the sheet is the floor's plan.
fn title_lead(number: &str, title: &str) -> Option<TitleLead> {
    let parts = title_parts(title);
    if parts.len() < 2 {
        return None;
    }
    let kinds = part_kinds(number, &parts);
    let plan_part = |k: &str| is_plan_type(k);
    let floor = parts.iter().zip(&kinds).position(|(p, k)| plan_part(k) && sheet_reader::title_names(p, floor_plan_words()));
    let index = floor.or_else(|| kinds.iter().position(|k| plan_part(k)))?;
    let mut kind = kinds.get(index).cloned().unwrap_or_default();
    if kind == "enlarged_plan"
        && let Some(bare) = parts.iter().zip(&kinds).position(|(p, k)| k == "unknown" && names_a_subject(p))
    {
        return Some(TitleLead { kind: "plan".into(), index: bare });
    }
    if kind.is_empty() {
        kind = "plan".into();
    }
    Some(TitleLead { kind, index })
}

/// Fewest letters of a word that names what a part of a title draws (`EQUIPMENT`, `SAFETY`; not
/// `TYP.` or a callout's `E-6`).
const SUBJECT_WORD_LETTERS: usize = 5;

/// Whether a part of a title that names no kind of drawing names what one draws: words only (a
/// part with digits is a callout or a mark), one of them a whole word of
/// [`SUBJECT_WORD_LETTERS`] letters or more.
fn names_a_subject(part: &str) -> bool {
    !part.chars().any(|c| c.is_ascii_digit()) && part.split(|c: char| !c.is_ascii_alphabetic()).any(|w| w.len() >= SUBJECT_WORD_LETTERS)
}

/// The other views a sheet's title lists beside the one that types it (`FLOOR PLAN/ DETAILS/
/// SCHEDULES`: `detail`, `schedule`): each other part's kind, once, in the title's order (a part
/// naming no kind of drawing, `FIRST` of `FIRST AND SECOND FLOOR PLANS`, adds none). Empty for a
/// title of one part.
pub fn title_views(number: &str, title: &str) -> Vec<String> {
    let Some(lead) = title_lead(number, title) else { return Vec::new() };
    let parts = title_parts(title);
    let mut out: Vec<String> = Vec::new();
    for (i, k) in part_kinds(number, &parts).into_iter().enumerate() {
        if i != lead.index && k != "unknown" && !out.contains(&k) {
            out.push(k);
        }
    }
    out
}

/// Whether a sheet is a reference one (life safety, occupancy load, code analysis: no plan of a
/// level, [`sheet_facts::is_reference_sheet`]). A title listing parts is one by the part that
/// types it: `EQUIPMENT/ LIFE SAFETY` is the floor's equipment plan, which also prints its
/// life safety.
fn reference_title(number: &str, title: &str) -> bool {
    match title_lead(number, title) {
        Some(lead) => title_parts(title).get(lead.index).is_some_and(|p| sheet_facts::is_reference_sheet(p)),
        None => sheet_facts::is_reference_sheet(title),
    }
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
        let valid_prefix = crate::wordrules::words("sheets.valid_prefixes").contains(&prefix);
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
                        if reference_title(&e.number, &e.title) {
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
            if reference_title(&e.number, &e.title) {
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
/// that a later file has again (a revision folder after the set) is marked superseded. Several
/// pages are read at once (`load` is called from several threads), a batch at a time, and taken
/// in the set's order, so the result is the one a page-by-page read gives.
pub fn run_classify_prints(
    project: &mut PrintSetProject,
    load: &(dyn Fn(&SheetEntry) -> Result<cadaddy_doc::Drawing, String> + Sync),
    cancel: &AtomicBool,
    progress: &mut ProgressFn<'_>,
) -> StageRecord {
    let start = web_time::Instant::now();
    let total = project.sheets.len();
    let mut classified = 0usize;
    // First every page is read on its own (its title block, its views, every text that reads as a
    // sheet number); then the set decides each page's number by where the set prints it; then the
    // entries are made.
    let old = std::mem::take(&mut project.sheets);
    let mut pages: Vec<ClassifyPage> = Vec::with_capacity(total);
    let mut entries = 0usize;
    let mut rest = old.into_iter().enumerate();
    loop {
        let batch: Vec<(usize, SheetEntry)> = rest.by_ref().take(CLASSIFY_BATCH).collect();
        if batch.is_empty() {
            break;
        }
        // Past the cap nothing more is read (the batch's own entries are counted below).
        let capped = entries >= MAX_SHEETS;
        let readings = par_map(&batch, |(_, entry)| {
            if capped || cancel.load(Ordering::Relaxed) {
                PageReading::Skipped
            } else if !entry.dirty && !entry.number.is_empty() {
                PageReading::Known
            } else {
                match load(entry) {
                    Ok(drawing) => PageReading::Read(Box::new(read_page(&drawing))),
                    Err(err) => PageReading::Failed(err),
                }
            }
        });
        for ((i, entry), reading) in batch.into_iter().zip(readings) {
            if entries >= MAX_SHEETS || matches!(reading, PageReading::Skipped) {
                entries += 1;
                pages.push(ClassifyPage::Kept(Box::new(entry)));
                continue;
            }
            progress(&Progress {
                stage: "classify".into(),
                sheet_index: i,
                sheet_number: entry.number.clone(),
                sheets_total: total,
                items_so_far: classified,
            });
            match reading {
                PageReading::Skipped | PageReading::Known => {
                    entries += 1;
                    pages.push(ClassifyPage::Kept(Box::new(entry)));
                    classified += 1;
                }
                PageReading::Failed(err) => {
                    let mut e = entry;
                    e.stage_results.insert(
                        "classify".into(),
                        SheetStageResult { stage: "classify".into(), status: StageStatus::Failed, error: Some(err), ..SheetStageResult::default() },
                    );
                    entries += 1;
                    pages.push(ClassifyPage::Kept(Box::new(e)));
                }
                PageReading::Read(r) => {
                    let PageRead { facts, recs, candidates, contents } = *r;
                    let read = ReadPage { entry, facts, recs, candidates, contents, note: String::new() };
                    entries += read.recs.len().max(1);
                    pages.push(ClassifyPage::Read(Box::new(read)));
                    classified += 1;
                }
            }
        }
    }
    set_numbers(&mut pages);
    let mut out: Vec<SheetEntry> = Vec::with_capacity(entries.min(MAX_SHEETS));
    // Each entry's page among the ones read now (its contents and title, to tell copies apart).
    let mut page_of: Vec<Option<usize>> = Vec::with_capacity(entries.min(MAX_SHEETS));
    let mut read_pages: Vec<PageSummary> = Vec::new();
    for page in pages {
        let read = match page {
            ClassifyPage::Kept(e) => {
                out.push(*e);
                page_of.push(None);
                continue;
            }
            ClassifyPage::Read(r) => *r,
        };
        let ReadPage { entry, facts, recs, contents, note, .. } = read;
        let k = read_pages.len();
        read_pages.push(PageSummary {
            title: facts.ident.as_ref().filter(|id| ident_strength(&id.source) == 2).map(|id| id.title.clone()).unwrap_or_default(),
            contents,
            strength: page_strength(&facts),
        });
        for mut rec in recs {
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
            if !view && e.sheet_type == "unknown" {
                // A title block that names no kind of sheet (`BUILDING A - OVERALL`): a view under
                // it titled as a plan of a level (`THIRD FLOOR PLAN`) makes it that plan.
                if let Some(t) = facts.view_titles.iter().find(|t| {
                    !is_key_plan(t)
                        && !sheet_reader::names_no_plan(t)
                        && !sheet_reader::detect_levels(t).is_empty()
                        && is_plan_type(&classify_sheet_type("", t))
                }) {
                    e.sheet_type = classify_sheet_type("", t);
                }
            }
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
                    for t in facts.view_titles.iter().filter(|t| !is_key_plan(t) && !sheet_reader::names_no_plan(t)) {
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
                // A sheet printing the floor plan beside the roof plan is the floor's plan: its
                // title naming the roof doesn't put it on the roof.
                if e.level == "R" && std::iter::once(&e.title).chain(&facts.view_titles).any(|t| sheet_reader::title_names(t, floor_plan_words())) {
                    e.level.clear();
                    e.levels.clear();
                }
            }
            // Reference sheets (life safety, occupancy load, code analysis) mention a floor
            // in their title but should not be on a building level.
            if reference_title(&e.number, &e.title) {
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
                    findings: match (known, note.is_empty()) {
                        (true, true) => Vec::new(),
                        (true, false) => vec![Finding { kind: "guessed".into(), description: note.clone(), severity: 1, ..Finding::default() }],
                        (false, true) => {
                            vec![Finding {
                                kind: "unreadable".into(),
                                description: "no sheet number found".into(),
                                severity: 2,
                                ..Finding::default()
                            }]
                        }
                        (false, false) => vec![Finding { kind: "unreadable".into(), description: note.clone(), severity: 2, ..Finding::default() }],
                    },
                    ..SheetStageResult::default()
                },
            );
            e.recognised = Some(rec);
            out.push(e);
            page_of.push(Some(k));
        }
    }
    // Pages that print one number on different sheets are no copies of each other: each keeps
    // its own entry and the clash is reported.
    split_number_clashes(&mut out, &page_of, &read_pages);
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
/// or layout: only what the title block and views say), or a drawing file. The files read last
/// stay in memory, so a merged set's pages are read from one copy, also by several threads at
/// once.
pub struct PrintLoader {
    dir: PathBuf,
    /// The files read last, the latest at the end.
    files: std::sync::Mutex<Vec<(String, std::sync::Arc<Vec<u8>>)>>,
}

/// Most files [`PrintLoader`] keeps: classify's threads read pages next to each other in the
/// set, so a few files at most at a time.
const MAX_KEPT_FILES: usize = 2 * CLASSIFY_THREADS;

impl PrintLoader {
    pub fn new(dir: &Path) -> PrintLoader {
        PrintLoader { dir: dir.to_path_buf(), files: std::sync::Mutex::new(Vec::new()) }
    }

    /// `e`'s file, read once while it stays among the last ones read (the lock is held while a
    /// file is read: the other threads wait for it instead of reading it again).
    fn bytes(&self, e: &SheetEntry) -> Result<std::sync::Arc<Vec<u8>>, String> {
        let mut files = self.files.lock().map_err(|_| format!("{}: the file cache failed", e.file))?;
        if let Some((_, b)) = files.iter().find(|(f, _)| *f == e.file) {
            return Ok(b.clone());
        }
        let path = self.dir.join(&e.file);
        if path.metadata().map_or(true, |m| m.len() > MAX_FILE_BYTES) {
            return Err(format!("{}: missing or too large", e.file));
        }
        let b = std::sync::Arc::new(std::fs::read(&path).map_err(|err| format!("{}: {err}", e.file))?);
        files.push((e.file.clone(), b.clone()));
        // at most a big file's worth kept besides the latest
        while files.len() > 1
            && (files.len() > MAX_KEPT_FILES || files.iter().rev().skip(1).map(|(_, b)| b.len() as u64).sum::<u64>() > MAX_FILE_BYTES)
        {
            files.remove(0);
        }
        Ok(b)
    }

    pub fn load(&self, e: &SheetEntry) -> Result<cadaddy_doc::Drawing, String> {
        if e.file.split(['/', '\\']).any(|part| part == "..") {
            return Err(format!("{}: outside the project folder", e.file));
        }
        let bytes = self.bytes(e)?;
        if e.file.to_ascii_lowercase().ends_with(".pdf") {
            let opts = cadaddy_io::pdf_read::PdfOptions { page: e.page.max(1), images: false, markups: false, layout: false, ..Default::default() };
            return cadaddy_io::pdf_read::read(&bytes, &opts).map(|i| i.drawing).map_err(|err| err.to_string());
        }
        cadaddy_io::read(&bytes, &e.file).map_err(|err| err.to_string())
    }
}

/// Pages classify reads before taking them in order: enough to keep its threads busy, few enough
/// that a cancel or the cap stops it soon.
const CLASSIFY_BATCH: usize = 32;
/// Pages classify reads at once. Each holds its drawing while it is read (a dense page's is
/// hundreds of MB), so this bounds the memory, not the core count: the lettering of each page is
/// read on all cores anyway.
const CLASSIFY_THREADS: usize = 4;

/// `f` over `items` on up to [`CLASSIFY_THREADS`] threads, the results in order. In the browser
/// (no threads), or for what no thread took (none could be started), one at a time here.
fn par_map<T: Sync, U: Send>(items: &[T], f: impl Fn(&T) -> U + Sync) -> Vec<U> {
    #[cfg(not(target_arch = "wasm32"))]
    let done = on_threads(items, &f);
    #[cfg(target_arch = "wasm32")]
    let done: Vec<Option<U>> = Vec::new();
    let mut done = done.into_iter();
    items.iter().map(|item| done.next().flatten().unwrap_or_else(|| f(item))).collect()
}

/// `f` over `items` on up to [`CLASSIFY_THREADS`] threads, each taking the next item when done:
/// each item's result, `None` for one no thread took.
#[cfg(not(target_arch = "wasm32"))]
fn on_threads<T: Sync, U: Send>(items: &[T], f: &(impl Fn(&T) -> U + Sync)) -> Vec<Option<U>> {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, CLASSIFY_THREADS).min(items.len());
    if threads < 2 {
        return Vec::new();
    }
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<U>>> = items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            let work = || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let (Some(item), Some(slot)) = (items.get(i), slots.get(i)) else { break };
                    let out = f(item);
                    if let Ok(mut slot) = slot.lock() {
                        *slot = Some(out);
                    }
                }
            };
            if std::thread::Builder::new().name("classify".into()).spawn_scoped(scope, work).is_err() {
                break;
            }
        }
    });
    slots.into_iter().map(|slot| slot.into_inner().ok().flatten()).collect()
}

/// What became of one page of a classify batch.
enum PageReading {
    /// Not read: cancelled, or past the cap.
    Skipped,
    /// Read before and not changed since: kept as it is.
    Known,
    Failed(String),
    Read(Box<PageRead>),
}

/// What classify reads on a page (its drawing is dropped once read).
struct PageRead {
    facts: PageFacts,
    recs: Vec<RecognisedSheet>,
    candidates: Vec<crate::sheet_facts::NumberCandidate>,
    contents: Vec<u64>,
}

fn read_page(drawing: &cadaddy_doc::Drawing) -> PageRead {
    let texts = sheet_facts::texts(drawing);
    PageRead {
        facts: PageFacts::read_texts(drawing, &texts),
        recs: sheet_reader::recognise_views(drawing),
        candidates: sheet_facts::number_candidates(drawing, &texts),
        contents: content_signature(&texts),
    }
}

/// Short texts looked at as level marks per page (a guard against hostile files).
const MAX_MARK_TEXTS: usize = 400;
/// Most readings of one level reported as not used for disagreeing with the others.
const MAX_LEVEL_WARNINGS: usize = 8;
/// Highest a top of plate can stand above the top floor and still be its roof's (feet).
const MAX_STOREY_FT: f64 = 30.0;
/// A letter's width in its text's height, about (the run of a marker's name: drafting fonts are
/// 0.6 to 0.9 as wide as tall).
const CHAR_WIDTH: f64 = 0.75;
/// Most parapet heights a roof keeps (hostile files).
const MAX_PARAPETS: usize = 16;

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
    fn read_texts(d: &cadaddy_doc::Drawing, texts: &[crate::sheet_facts::TextItem]) -> PageFacts {
        let ident = crate::sheet_facts::identify_sheet(texts);
        let view_titles = crate::sheet_facts::view_titles(d, texts).into_iter().map(|v| v.title).collect();
        let issued = texts.iter().filter_map(|t| latest_date(&t.value)).max().unwrap_or_default();
        PageFacts { ident, view_titles, issued, level_marks: level_marks(texts) }
    }
}

/// A page of the set during classify: read now, or kept as it was (read before, unreadable, or
/// past the cap).
enum ClassifyPage {
    Kept(Box<SheetEntry>),
    Read(Box<ReadPage>),
}

/// What classify read on a page before the set decides its number.
struct ReadPage {
    entry: SheetEntry,
    facts: PageFacts,
    /// The sheet reader's reading, one per view.
    recs: Vec<RecognisedSheet>,
    /// Every text that reads as a sheet number, with where it sits.
    candidates: Vec<crate::sheet_facts::NumberCandidate>,
    contents: Vec<u64>,
    /// How the set changed the page's number (a finding on its entries), empty when it didn't.
    note: String,
}

/// A read page as the copy test needs it.
struct PageSummary {
    /// The title its labelled title block gives (a title guessed from the largest lettering can be
    /// the project's name: empty then, and the contents decide).
    title: String,
    contents: Vec<u64>,
    strength: u8,
}

/// How a page's number was read: 3 by its place in the set's title block, 2 from a labelled
/// title block, 1 the largest sheet number at the sheet's edge, 0 the sheet reader's fallback (the
/// largest sheet-number lettering along the right or bottom of the sheet).
fn ident_strength(source: &str) -> u8 {
    match source {
        "set_position" => 3,
        "title_block" | "number_label" => 2,
        "largest_number" => 1,
        _ => 0,
    }
}

fn page_strength(facts: &PageFacts) -> u8 {
    facts.ident.as_ref().map_or(0, |id| ident_strength(&id.source))
}

/// The number the page's own reading gave: its title block's, else the sheet reader's.
fn page_number(r: &ReadPage) -> Option<(String, u8)> {
    if let Some(id) = &r.facts.ident {
        return Some((id.number.clone(), ident_strength(&id.source)));
    }
    let rec = r.recs.first()?;
    let n = if rec.printed_number.is_empty() { &rec.number } else { &rec.printed_number };
    (!n.is_empty()).then(|| (n.clone(), 0))
}

/// Distinct texts on a page (a fingerprint of its contents, hashed): two prints of one sheet
/// share nearly all of them, two sheets of one set little more than the title block.
fn content_signature(texts: &[crate::sheet_facts::TextItem]) -> Vec<u64> {
    use std::hash::{Hash as _, Hasher as _};
    let mut out: Vec<u64> = texts
        .iter()
        .take(MAX_SIGNATURE_TEXTS)
        .map(|t| {
            let mut h = std::hash::DefaultHasher::new();
            t.value.trim().to_ascii_uppercase().hash(&mut h);
            h.finish()
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Texts read into a page's content fingerprint.
const MAX_SIGNATURE_TEXTS: usize = 20_000;

/// Shared over the shorter of two sorted, distinct lists (0 to 1): a print that splits its lines
/// into more texts (a PDF beside its drawing file) still holds the other's.
fn overlap(a: &[u64], b: &[u64]) -> f64 {
    let (mut i, mut j, mut both) = (0usize, 0usize, 0usize);
    while let (Some(x), Some(y)) = (a.get(i), b.get(j)) {
        match x.cmp(y) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                both += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let fewer = a.len().min(b.len());
    if fewer == 0 { 1.0 } else { both as f64 / fewer as f64 }
}

/// Words shared over all words of two titles (0 to 1).
fn title_overlap(a: &str, b: &str) -> f64 {
    let words = |t: &str| -> std::collections::BTreeSet<String> {
        t.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
    };
    let (a, b) = (words(a), words(b));
    let all = a.union(&b).count();
    if all == 0 { 1.0 } else { a.intersection(&b).count() as f64 / all as f64 }
}

/// Fewest distinct texts on both pages for their contents to tell them apart.
const MIN_COMPARED_TEXTS: usize = 8;

/// Whether two pages that print one number are copies of one sheet: their titles match (a
/// revision may change a word) and their contents don't say otherwise, or their contents are
/// nearly the same print (a title read differently). With neither to compare, the number decides,
/// as before.
fn same_sheet(a: &PageSummary, b: &PageSummary) -> bool {
    let contents = (a.contents.len() >= MIN_COMPARED_TEXTS && b.contents.len() >= MIN_COMPARED_TEXTS).then(|| overlap(&a.contents, &b.contents));
    let titles = (!a.title.trim().is_empty() && !b.title.trim().is_empty()).then(|| title_overlap(&a.title, &b.title));
    match (titles, contents) {
        (Some(t), Some(c)) if t >= SAME_TITLE => c >= 0.3,
        (Some(t), None) => t >= SAME_TITLE,
        (_, Some(c)) => c >= if titles.is_some() { 0.9 } else { 0.85 },
        (None, None) => true,
    }
}

/// Title word overlap of two prints of one sheet.
const SAME_TITLE: f64 = 0.8;

/// Share of a spot's numbers that are different numbers (copies of a sheet in a set repeat a few).
const DISTINCT_AT_SPOT: f64 = 0.6;

/// How close (a share of the page's width and height) a page's number is to the set's spot for it.
const SPOT_RADIUS: f64 = 0.03;

/// The set's sheet-number spots: where its pages' numbers sit (a set prints every sheet's number at
/// one place of its title block, a set of several firms' title blocks at a few). Each spot is its
/// centre and its numbers' lettering size; a spot holds at least 3 pages' numbers and 3 % of them,
/// and they are different numbers (a tag the pages repeat at one place is no sheet number).
fn number_spots(picks: &[(String, [f64; 2], f64)]) -> Vec<([f64; 2], f64)> {
    let mut clusters: Vec<([f64; 2], Vec<f64>, Vec<&str>)> = Vec::new();
    for (value, at, size) in picks {
        match clusters.iter_mut().find(|(c, _, _)| (c[0] - at[0]).abs() <= SPOT_RADIUS && (c[1] - at[1]).abs() <= SPOT_RADIUS) {
            Some((c, sizes, values)) => {
                let n = sizes.len() as f64;
                *c = [(c[0] * n + at[0]) / (n + 1.0), (c[1] * n + at[1]) / (n + 1.0)];
                sizes.push(*size);
                values.push(value);
            }
            None => clusters.push((*at, vec![*size], vec![value])),
        }
    }
    let least = 3usize.max((picks.len() as f64 * 0.03).ceil() as usize);
    clusters
        .into_iter()
        .filter(|(_, sizes, values)| {
            let distinct = values.iter().collect::<std::collections::BTreeSet<_>>().len();
            sizes.len() >= least && distinct as f64 >= DISTINCT_AT_SPOT * values.len() as f64
        })
        .map(|(c, mut sizes, _)| {
            sizes.sort_by(f64::total_cmp);
            (c, sizes.get(sizes.len() / 2).copied().unwrap_or(1.0))
        })
        .collect()
}

/// Different numbers that start with one series of letters, at least, for the letters to be the
/// set's own designator (a set numbering one discipline's sheets `AJ101`, `AJ102` ...).
const SERIES_MIN_PAGES: usize = 5;

/// The set's own series: letters ([`sheet_facts::discipline_of`]) that start
/// [`SERIES_MIN_PAGES`] different numbers or more among the pages' number-like lettering, one or
/// two letters whose first is a discipline (rule data `sheets.discipline_prefix`). A mark that
/// repeats page to page is one number, not a series; and where a series sits still has to agree
/// page to page before it gives the set a number spot ([`number_spots`]).
fn set_series(numbers: impl Iterator<Item = String>) -> std::collections::BTreeSet<String> {
    let mut by_letters: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    for n in numbers.take(MAX_SHEETS * 64) {
        let n = n.trim().to_ascii_uppercase();
        let letters = sheet_facts::discipline_of(&n);
        if (1..=2).contains(&letters.len()) {
            by_letters.entry(letters).or_default().insert(n);
        }
    }
    let known = crate::wordrules::pairs("sheets.discipline_prefix");
    by_letters
        .into_iter()
        .filter(|(letters, numbers)| {
            let first: String = letters.chars().take(1).collect();
            numbers.len() >= SERIES_MIN_PAGES && known.iter().any(|(p, _)| *p == first)
        })
        .map(|(letters, _)| letters)
        .collect()
}

/// Each page's number as the set prints it. A page's own reading can take a tag in the drawing
/// area (an equipment or schedule mark that reads like a sheet number) when its title block's
/// number has no label or doesn't stand out. The set shows where its numbers are: where most pages'
/// numbers sit. A page whose reading isn't there takes the number at that spot when it has one; a
/// page with no number reading takes it too; a fallback reading whose letters name no discipline,
/// with nothing at the spot, is dropped (the page is left unnumbered, never named by a tag).
fn set_numbers(pages: &mut [ClassifyPage]) {
    let at_of =
        |r: &ReadPage, n: &str| r.candidates.iter().filter(|c| c.value == n.trim()).max_by(|a, b| a.size.total_cmp(&b.size)).map(|c| (c.at, c.size));
    let near = |at: [f64; 2], c: [f64; 2]| (at[0] - c[0]).abs() <= SPOT_RADIUS && (at[1] - c[1]).abs() <= SPOT_RADIUS;
    // The set's own series (letters its numbers start with that the designator list doesn't
    // name: `AJ101`, `AJ102` ... one discipline's sheets) count as a designator in it, and
    // where the series prints its numbers is a spot of the set's.
    let series = set_series(
        pages
            .iter()
            .filter_map(|p| match p {
                ClassifyPage::Read(r) => Some(r.candidates.iter().map(|c| c.value.clone())),
                ClassifyPage::Kept(_) => None,
            })
            .flatten(),
    );
    let in_series = |n: &str| series.contains(&sheet_facts::discipline_of(n));
    let designated = |n: &str| sheet_facts::designator_prefix(n) || in_series(n);
    let largest_in_series = |r: &ReadPage| r.candidates.iter().filter(|c| in_series(&c.value)).max_by(|a, b| a.size.total_cmp(&b.size)).cloned();
    let series_picks: Vec<(String, [f64; 2], f64)> = pages
        .iter()
        .filter_map(|p| match p {
            ClassifyPage::Read(r) => largest_in_series(r).map(|c| (c.value, c.at, c.size)),
            ClassifyPage::Kept(_) => None,
        })
        .collect();
    let series_spots = number_spots(&series_picks);
    // A page whose own reading is weak (no labelled title block: the largest number-like
    // lettering) takes the series' number at the series' spot when it is as large.
    for p in pages.iter_mut() {
        let ClassifyPage::Read(r) = p else { continue };
        let own = page_number(r);
        if own.as_ref().is_some_and(|(n, strength)| *strength >= 2 || in_series(n)) {
            continue;
        }
        let own_size = own.as_ref().and_then(|(n, _)| at_of(r, n)).map_or(0.0, |(_, size)| size);
        let Some(c) = r
            .candidates
            .iter()
            .filter(|c| in_series(&c.value) && c.size >= 0.9 * own_size && series_spots.iter().any(|(s, _)| near(c.at, *s)))
            .max_by(|a, b| a.size.total_cmp(&b.size))
            .cloned()
        else {
            continue;
        };
        r.note = match &own {
            Some((was, _)) => format!("sheet number {} read at the set's sheet-number spot, not {was} (lettering elsewhere on the sheet)", c.value),
            None => format!("sheet number {} read at the set's sheet-number spot", c.value),
        };
        let title = r.facts.ident.as_ref().map(|id| id.title.clone()).unwrap_or_default();
        r.facts.ident =
            Some(sheet_facts::SheetIdent { discipline: sheet_facts::discipline_of(&c.value), number: c.value, title, source: "set_position".into() });
    }
    let picks: Vec<(String, [f64; 2], f64)> = pages
        .iter()
        .filter_map(|p| match p {
            ClassifyPage::Read(r) => page_number(r).filter(|(n, _)| designated(n)).and_then(|(n, _)| at_of(r, &n).map(|(at, size)| (n, at, size))),
            ClassifyPage::Kept(_) => None,
        })
        .collect();
    let spots = number_spots(&picks);
    for p in pages.iter_mut() {
        let ClassifyPage::Read(r) = p else { continue };
        let pick = page_number(r);
        let pick_at = pick.as_ref().and_then(|(n, _)| at_of(r, n));
        if pick_at.is_some_and(|(at, _)| spots.iter().any(|(c, _)| near(at, *c))) {
            continue;
        }
        // The largest number at one of the set's spots, lettered like the set's numbers there.
        let at_spot = r
            .candidates
            .iter()
            .filter_map(|c| {
                spots.iter().find(|(s, size)| near(c.at, *s) && c.size >= 0.5 * size).map(|(s, _)| (c, (c.at[0] - s[0]).hypot(c.at[1] - s[1])))
            })
            .max_by(|a, b| a.0.size.total_cmp(&b.0.size).then(b.1.total_cmp(&a.1)))
            .map(|(c, _)| c.value.clone());
        let pick_ok = pick.as_ref().is_some_and(|(n, strength)| *strength >= 2 && designated(n));
        match (at_spot, pick) {
            (Some(n), pick) if !pick_ok => {
                let title = r.facts.ident.as_ref().map(|id| id.title.clone()).unwrap_or_default();
                r.note = match &pick {
                    Some((was, _)) if *was != n => {
                        format!("sheet number {n} read at the set's sheet-number spot, not {was} (lettering elsewhere on the sheet)")
                    }
                    Some(_) => String::new(),
                    None => format!("sheet number {n} read at the set's sheet-number spot"),
                };
                r.facts.ident =
                    Some(sheet_facts::SheetIdent { discipline: sheet_facts::discipline_of(&n), number: n, title, source: "set_position".into() });
            }
            (None, Some((was, strength))) if strength < 2 && !designated(&was) => {
                r.note = format!("no sheet number found: {was} names no discipline and is not where the set prints its numbers (a tag on the sheet)");
                r.facts.ident = None;
                for rec in &mut r.recs {
                    rec.number.clear();
                    rec.printed_number.clear();
                }
            }
            _ => {}
        }
    }
}

/// Entries of different pages that share a number but are different sheets (their titles and
/// contents differ): the page read best keeps the number, the others become `<number>~2`, `~3`
/// in page order, each with `printed_as` and a finding. Copies of one sheet keep one number.
fn split_number_clashes(out: &mut [SheetEntry], page_of: &[Option<usize>], pages: &[PageSummary]) {
    let base = |e: &SheetEntry| -> Option<String> {
        let r = e.recognised.as_ref()?;
        let n = if r.printed_number.is_empty() { &r.number } else { &r.printed_number };
        (!n.is_empty()).then(|| n.clone())
    };
    let mut by_number: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, e) in out.iter().enumerate() {
        if let (Some(n), Some(Some(_))) = (base(e), page_of.get(i)) {
            by_number.entry(n).or_default().push(i);
        }
    }
    for (number, members) in by_number {
        // The distinct pages, in set order, then the pages as copies of one sheet.
        let mut distinct: Vec<usize> = members.iter().filter_map(|&i| page_of.get(i).copied().flatten()).collect();
        distinct.dedup();
        if distinct.len() < 2 {
            continue;
        }
        let mut sheets: Vec<Vec<usize>> = Vec::new();
        for k in distinct {
            let Some(page) = pages.get(k) else { continue };
            match sheets.iter_mut().find(|s| s.first().and_then(|&f| pages.get(f)).is_some_and(|f| same_sheet(f, page))) {
                Some(s) => s.push(k),
                None => sheets.push(vec![k]),
            }
        }
        if sheets.len() < 2 {
            continue;
        }
        let strength = |s: &Vec<usize>| s.iter().filter_map(|&k| pages.get(k)).map(|p| p.strength).max().unwrap_or(0);
        let keeper = sheets.iter().enumerate().max_by(|a, b| strength(a.1).cmp(&strength(b.1)).then(b.0.cmp(&a.0))).map_or(0, |(i, _)| i);
        let sources: Vec<String> = sheets
            .iter()
            .filter_map(|s| s.first())
            .filter_map(|&k| members.iter().find(|&&i| page_of.get(i).copied().flatten() == Some(k)).and_then(|&i| out.get(i)))
            .map(source_of)
            .collect();
        let mut suffix = 1usize;
        for (si, s) in sheets.iter().enumerate() {
            if si == keeper {
                continue;
            }
            suffix += 1;
            let renamed = format!("{number}~{suffix}");
            for &i in &members {
                if !page_of.get(i).copied().flatten().is_some_and(|k| s.contains(&k)) {
                    continue;
                }
                let Some(e) = out.get_mut(i) else { continue };
                if let Some(r) = e.recognised.as_mut() {
                    if r.printed_number.is_empty() {
                        r.number = renamed.clone();
                    } else {
                        r.number = r.number.replacen(&r.printed_number, &renamed, 1);
                        r.printed_number = renamed.clone();
                    }
                    e.number = r.number.clone();
                }
                e.printed_as = number.clone();
                let why = format!(
                    "sheet number {number} is printed on {} different sheets ({}): this one is kept as {renamed}, no copy of the other",
                    sheets.len(),
                    sources.join(", ")
                );
                if let Some(res) = e.stage_results.get_mut("classify") {
                    res.findings.push(Finding { kind: "conflict".into(), description: why, severity: 3, ..Finding::default() });
                }
            }
        }
    }
}

/// The discipline code a sheet number's letters stand for (`A` from `A111`, `FP` from `FP-1`).
/// Letters the rule data lists as a discipline of their own (`FP`, `FA`, `SL`) stand for it;
/// else a designator with a modifier (`AE111`, `AD-201`, `EP1.1`: the first letter names the
/// discipline, the next ones the kind of sheet within it) stands for its first letter when that
/// is a discipline. Other letters stay as they are.
fn canonical_discipline(letters: &str) -> String {
    let up = letters.trim().to_ascii_uppercase();
    let known = crate::wordrules::pairs("sheets.discipline_prefix");
    if let Some((_, d)) = known.iter().find(|(p, _)| *p == up) {
        return (*d).to_string();
    }
    let first: String = up.chars().take(1).collect();
    match known.iter().find(|(p, _)| *p == first) {
        Some((_, d)) if up.len() > 1 && up.chars().all(|c| c.is_ascii_uppercase()) => (*d).to_string(),
        _ => up,
    }
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
fn level_key(c: &str) -> (i32, String) {
    sheet_reader::level_order(c)
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

/// The [`LevelMark::level`] of a section's finished floor marked with no level number
/// (`FIN. FLR. EL. 100'-0"`): a building of one storey's floor.
pub const MARK_FINISH_FLOOR: &str = "FF";
/// The [`LevelMark::level`] of a top-of-plate marker (`T.O. PLATE 112'-0"`, `TRUSS BEARING`):
/// where the roof bears.
pub const MARK_PLATE: &str = "PLATE";
/// The [`LevelMark::level`] of a top-of-parapet marker (`T.O. PARAPET (LOW) 124'-0"`): how high
/// the roof's parapets stand.
pub const MARK_PARAPET: &str = "PARAPET";

/// A text's words for comparing with a datum label list: upper case, punctuation dropped, the
/// elevation keywords (`EL`, `ELEV`) left out.
fn label_words(s: &str) -> String {
    s.to_ascii_uppercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty() && !matches!(*w, "EL" | "ELEV" | "ELEVATION"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// What an unnumbered level marker names (rule data `levels.finish_floor_mark_words`,
/// `levels.plate_mark_words`): [`MARK_FINISH_FLOOR`], [`MARK_PLATE`] or none.
fn datum_label(name: &str) -> Option<&'static str> {
    let n = label_words(name);
    if n.is_empty() {
        return None;
    }
    let is = |list: &[&str]| list.iter().any(|w| label_words(w) == n);
    if is(crate::wordrules::words("levels.finish_floor_mark_words")) {
        Some(MARK_FINISH_FLOOR)
    } else if is(crate::wordrules::words("levels.plate_mark_words")) {
        Some(MARK_PLATE)
    } else {
        None
    }
}

/// Whether a marker's name is a bare code (`3`, `26`, `R`, `B1`, `L1`): a detail callout, a grid
/// bubble, or a fixture, lintel or keynote tag beside a dimension, never a level marker, which
/// names its level in words (`SECOND FLOOR`, `LEVEL 2`, `ROOF`).
fn bare_code(name: &str) -> bool {
    let words: Vec<&str> = name.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let digits = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit());
    let code = |w: &str| w.len() == 1 || digits(w) || ['B', 'b', 'L', 'l'].iter().any(|p| w.strip_prefix(*p).is_some_and(digits));
    match words.as_slice() {
        [w] => code(w),
        // `L 1`, `B 2`: a tag's letter and number apart.
        [a, n] => matches!(a.to_ascii_uppercase().as_str(), "L" | "B") && digits(n),
        _ => false,
    }
}

/// Level marks on a section or elevation: a level note in one text (`SECOND FLOOR EL. 110'-8"`)
/// or a level name with its elevation right beside it (the two lines of a level marker). A
/// marker names its level: a bare number or letter beside a dimension is a callout. A finished
/// floor or a top of plate marked with no level number is kept as [`MARK_FINISH_FLOOR`] or
/// [`MARK_PLATE`], for the index to place.
fn level_marks(texts: &[crate::sheet_facts::TextItem]) -> Vec<LevelMark> {
    use cadaddy_walk3d::levels::{canonical_level, parse_elevation, parse_level_note};
    let mut out = Vec::new();
    let short: Vec<&crate::sheet_facts::TextItem> =
        texts.iter().filter(|t| t.value.len() <= 40 && !t.value.contains('=')).take(MAX_MARK_TEXTS * 50).collect();
    // How far a text is from a marker's name: from anywhere along the name's line of lettering
    // (a long name, `T.O. PARAPET (LOW)`, prints its elevation under its far end).
    let from_name = |t: &crate::sheet_facts::TextItem, u: &crate::sheet_facts::TextItem| {
        let run = cadaddy_geom::Vec2::new(t.rotation.cos(), t.rotation.sin()) * (t.value.chars().count() as f64 * t.height * CHAR_WIDTH);
        let (a, d) = (t.at, run);
        let l2 = d.dot(d);
        let k = if l2 > 1e-18 { ((u.at - a).dot(d) / l2).clamp(0.0, 1.0) } else { 0.0 };
        u.at.dist(a + d * k)
    };
    // The elevation printed right beside a marker's name (the marker's second line).
    let beside = |t: &crate::sheet_facts::TextItem| {
        short
            .iter()
            .filter(|u| {
                u.at != t.at
                    && from_name(t, u) <= 4.0 * t.height.max(u.height)
                    && canonical_level(&u.value).is_none()
                    && datum_label(&u.value).is_none()
                    && parse_elevation(&u.value).is_some()
            })
            .min_by(|a, b| from_name(t, a).total_cmp(&from_name(t, b)))
            .copied()
    };
    // The exterior elevations' view titles (`NORTH ELEVATION`), for the facade a top of parapet
    // is marked on.
    let titles: Vec<(cadaddy_geom::Vec2, &'static str)> =
        short.iter().filter_map(|t| facade_title(&t.value).map(|f| (t.at, f))).take(MAX_MARK_TEXTS).collect();
    let mut looked = 0usize;
    for t in &short {
        // The marker's name: the text but its elevation.
        let own = parse_elevation(&t.value);
        let name = match own {
            Some((_, a, z)) => format!("{} {}", t.value.get(..a).unwrap_or_default(), t.value.get(z..).unwrap_or_default()),
            None => t.value.clone(),
        };
        let datum = datum_label(&name);
        if datum.is_none() && (canonical_level(&t.value).is_none() || bare_code(&name)) {
            continue;
        }
        looked += 1;
        if looked > MAX_MARK_TEXTS {
            break;
        }
        if let Some(kind) = datum {
            let el = own.map(|(v, _, _)| v).or_else(|| beside(t).and_then(|u| parse_elevation(&u.value)).map(|(v, _, _)| v));
            if let Some(v) = el.filter(|v| v.is_finite() && (-1000.0..=5000.0).contains(v)) {
                out.push(LevelMark { level: kind.into(), elevation_ft: v, facade: None });
            }
            continue;
        }
        let note = parse_level_note(&t.value).or_else(|| beside(t).and_then(|u| parse_level_note(&format!("{} {}", t.value, u.value))));
        if let Some(n) = note {
            // A parapet is no storey: its top is kept for the roof it stands round.
            let level = if n.kind == cadaddy_walk3d::levels::DatumKind::Parapet { Some(MARK_PARAPET.to_string()) } else { code_of_level(&n.name) };
            if let Some(level) = level.filter(|_| n.elevation_ft.is_finite() && (-1000.0..=5000.0).contains(&n.elevation_ft)) {
                let facade = if level == MARK_PARAPET { facade_near(&titles, t.at) } else { None };
                out.push(LevelMark { level, elevation_ft: n.elevation_ft, facade });
            }
        }
    }
    out
}

/// The facade an exterior elevation's view title names (`NORTH ELEVATION`, `EAST EXTERIOR
/// ELEVATION`): one compass side and the word elevation; none otherwise.
fn facade_title(t: &str) -> Option<&'static str> {
    let up = t.to_ascii_uppercase();
    let words: Vec<&str> = up.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    if !words.iter().any(|w| matches!(*w, "ELEVATION" | "ELEV")) {
        return None;
    }
    let sides: Vec<&'static str> = words
        .iter()
        .filter_map(|w| match *w {
            "NORTH" => Some("north"),
            "SOUTH" => Some("south"),
            "EAST" => Some("east"),
            "WEST" => Some("west"),
            _ => None,
        })
        .collect();
    match sides.as_slice() {
        [one] => Some(one),
        _ => None,
    }
}

/// The facade of the elevation a mark at `at` stands on: its view's title is printed under the
/// view, so the title below the mark nearest it (height first: elevations stacked one over the
/// other; then sideways: elevations side by side), when it is clearly the nearest (the next
/// scores at least a quarter more). None when no title is below it or two are as near.
fn facade_near(titles: &[(cadaddy_geom::Vec2, &'static str)], at: cadaddy_geom::Vec2) -> Option<String> {
    let mut by: Vec<(f64, &str)> = titles.iter().filter(|(p, _)| p.y < at.y).map(|(p, f)| ((at.y - p.y) + 0.25 * (at.x - p.x).abs(), *f)).collect();
    by.sort_by(|a, b| a.0.total_cmp(&b.0));
    match by.as_slice() {
        [] => None,
        [(_, f)] => Some((*f).to_string()),
        [(d0, f0), (d1, f1), ..] => (f0 == f1 || *d1 >= 1.25 * d0).then(|| (*f0).to_string()),
    }
}

/// Plan sheets of a numbered series whose titles name no level take it from their number: in a
/// series of two or more (`A111`…`A114`) the last digit is the level. A series is one kind of
/// plan, floor after floor: the same title but for its level words. Sheets numbered in a row that
/// are different plans of one floor (`A101` floor plan, `A102` equipment plan, `A105` ceiling
/// plan) are no levels.
fn level_by_number(sheets: &mut [SheetEntry]) {
    let series = |e: &SheetEntry| -> Option<(String, u32)> {
        if !e.level.is_empty() || !e.superseded_by.is_empty() || !is_plan_type(&e.sheet_type) || e.number.contains('/') {
            return None;
        }
        let letters: String = e.number.chars().take_while(char::is_ascii_uppercase).collect();
        let digits = e.number.get(letters.len()..)?.trim_start_matches('-');
        let level = digits.chars().nth(2).and_then(|c| c.to_digit(10))?;
        (!letters.is_empty() && digits.len() == 3 && digits.starts_with('1') && digits.chars().all(|c| c.is_ascii_digit()) && level >= 1)
            .then(|| (format!("{letters}{}|{}", digits.get(..2).unwrap_or_default(), sheet_reader::title_kind(&e.title)), level))
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
    /// The other views a title listing parts names beside the one that types the sheet
    /// (`FLOOR PLAN/ DETAILS/ SCHEDULES`, a plan: `detail`, `schedule`; [`title_views`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<String>,
}

/// A sheet whose title names a floor plan that is filed as something else, while a level has no
/// sheet titled as its floor plan (Q6514): to ask, never to build on silently.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FloorPlanFiledOtherwise {
    /// The sheet's index entry.
    pub sheet: String,
    pub title: String,
    /// What it is filed as (`schedule`, `detail`, …).
    #[serde(rename = "type")]
    pub sheet_type: String,
    /// The level with no titled floor plan (empty: the set has no level at all).
    pub level: String,
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
    /// What gave the elevation: `marks` (the level's own markers), `finish floor` (a section's
    /// unnumbered finished floor, in a building of one storey) or `top of plate` (where the roof
    /// bears, when no roof is marked). Empty with no elevation.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub elevation_source: String,
    /// The roof's parapets: each top of parapet the sections and elevations mark, as a height
    /// above the roof's elevation (where it bears), lowest first. Only the roof level has them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parapets_ft: Vec<f64>,
    /// Each top of parapet with the facade whose elevation marks it (none: a section's mark, or
    /// a title that names none), lowest first: a parapet that steps keeps a top per facade.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parapet_tops: Vec<ParapetTop>,
}

/// A top of parapet the index keeps for the roof: its height above the roof's elevation and
/// the facade marking it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ParapetTop {
    pub height_ft: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facade: Option<String>,
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
    /// Sheet numbers printed on different sheets (their titles and contents differ, so they are no
    /// copies): each sheet has its own entry (`<number>~2`, …) and the clash is to be resolved.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub number_conflicts: Vec<NumberConflict>,
    /// Sheets titled as a floor plan but filed as something else while a level has no sheet
    /// titled as its floor plan ([`FloorPlanFiledOtherwise`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub floor_plans_filed_otherwise: Vec<FloorPlanFiledOtherwise>,
}

/// Most [`FloorPlanFiledOtherwise`] entries an index lists.
const MAX_FILED_OTHERWISE: usize = 20;

/// One sheet number printed on different sheets.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NumberConflict {
    /// The number as printed.
    pub number: String,
    /// The index entries it became (the number itself for the sheet that kept it).
    pub sheets: Vec<String>,
    /// Each entry's page (`file#page`), in the same order.
    pub sources: Vec<String>,
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
            views: title_views(&first.number, &first.title),
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
    // A floor that has plans is a level, even when all its plans are enlarged or partial ones
    // (`ENLARGED FIRST FLOOR PLAN - NORTH`): they are then the plans that fill it.
    let mut enlarged_only: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    for s in out.sheets.iter().filter(|s| s.sheet_type == "enlarged_plan" && s.level != "SITE") {
        for l in s.levels.iter().chain(std::iter::once(&s.level)).filter(|l| !l.is_empty() && !cadaddy_walk3d::levels::is_datum_name(l)) {
            if by_level.contains_key(l) {
                continue;
            }
            let v = enlarged_only.entry(l.clone()).or_default();
            if !v.iter().any(|x| x.get("sheet").and_then(serde_json::Value::as_str) == Some(&s.number)) {
                v.push(serde_json::json!({"sheet": s.number, "typical": false}));
            }
        }
    }
    by_level.extend(enlarged_only);
    let mut codes: Vec<String> = by_level.keys().cloned().collect();
    codes.sort_by_key(|c| level_key(c));
    // The storeys are the levels but the roof (a roof plan shows the top of the building, no
    // floor). A building of one storey may name it nowhere: its plans are titled `FLOOR PLAN`
    // and only the roof plan names a level. Its plans that name none are on that one floor,
    // which is a level of the building even though no title names it.
    // A mezzanine is inside a storey, no storey of its own: a building of one storey with a
    // mezzanine is still of one storey.
    let floors: Vec<&String> = codes.iter().filter(|c| c.as_str() != "R" && !cadaddy_walk3d::levels::is_mezzanine(c)).collect();
    let one_level = floors.len() <= 1;
    let lowest = floors.iter().find(|c| c.parse::<u32>().is_ok()).map_or_else(|| "1".to_string(), |c| (*c).clone());
    if one_level && !codes.contains(&lowest) {
        let unnamed: Vec<serde_json::Value> = out
            .sheets
            .iter()
            .filter(|s| is_level_plan_type(&s.sheet_type) && s.level.is_empty())
            .map(|s| serde_json::json!({"sheet": s.number, "typical": false}))
            .collect();
        if !unnamed.is_empty() {
            by_level.insert(lowest.clone(), unnamed);
            codes.push(lowest.clone());
            codes.sort_by_key(|c| level_key(c));
        }
    }
    // Elevations: the value most marks give per level, above the first floor (whatever datum the
    // sections use: `100'-0"` or the true elevation). A civil or site sheet's marks are spot
    // elevations on the site datum, never the levels'.
    let mut marks: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut finish: Vec<f64> = Vec::new();
    let mut plate: Vec<f64> = Vec::new();
    let mut parapet: Vec<f64> = Vec::new();
    let mut parapet_marks: Vec<(f64, Option<String>)> = Vec::new();
    let site = |e: &SheetEntry| e.level.eq_ignore_ascii_case("SITE") || cadaddy_walk3d::levels::site_sheet(&e.discipline, &e.title);
    for e in project.sheets.iter().filter(|e| e.superseded_by.is_empty() && !site(e)) {
        for m in e.level_marks.iter().filter(|m| m.elevation_ft.is_finite()) {
            match m.level.as_str() {
                MARK_FINISH_FLOOR => finish.push(m.elevation_ft),
                MARK_PLATE => plate.push(m.elevation_ft),
                MARK_PARAPET => {
                    parapet.push(m.elevation_ft);
                    if parapet_marks.len() < MAX_PARAPETS * 8 {
                        parapet_marks.push((m.elevation_ft, m.facade.clone()));
                    }
                }
                _ => marks.entry(m.level.clone()).or_default().push(m.elevation_ft),
            }
        }
    }
    let same = |a: f64, b: f64| (a - b).abs() < 1.0 / 96.0;
    // The value most marks give, and how many give it.
    let most = |vals: &[f64]| -> Option<(f64, usize)> {
        let mut best: Option<(f64, usize)> = None;
        for v in vals {
            let n = vals.iter().filter(|w| same(**w, *v)).count();
            if best.is_none_or(|(_, bn)| n > bn) {
                best = Some((*v, n));
            }
        }
        best
    };
    let mut elevation: BTreeMap<String, f64> = BTreeMap::new();
    let mut source: BTreeMap<String, &str> = BTreeMap::new();
    for (l, vals) in &marks {
        let Some((v, n)) = most(vals) else { continue };
        elevation.insert(l.clone(), v);
        source.insert(l.clone(), "marks");
        // Marks giving the level another elevation: reported, not used.
        let mut other: Vec<f64> = Vec::new();
        for w in vals.iter().filter(|w| !same(**w, v)) {
            if !other.iter().any(|o| same(*o, *w)) {
                other.push(*w);
            }
        }
        for w in other.iter().take(MAX_LEVEL_WARNINGS) {
            out.level_warnings.push(format!("level {l}: {} not used: {n} other mark(s) give {}", cadaddy_walk3d::feet(*w), cadaddy_walk3d::feet(v)));
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
            source.remove(l);
            out.level_warnings
                .push(format!("level {l}: {} not used: it breaks the levels' order (elevations rise with the level)", cadaddy_walk3d::feet(*e)));
        }
    }
    // A finished floor marked with no level number is the floor of a building of one storey.
    if one_level
        && !elevation.contains_key(&lowest)
        && let Some((v, _)) = most(&finish)
    {
        elevation.insert(lowest.clone(), v);
        source.insert(lowest.clone(), "finish floor");
    }
    // The roof bears on the top of plate: with no roof marked, the plate above the top floor
    // (within a storey of it) is the roof's elevation.
    if codes.iter().any(|c| c == "R") && !elevation.contains_key("R") {
        let top = codes.iter().filter(|c| c.as_str() != "R").filter_map(|c| elevation.get(c)).copied().fold(f64::NEG_INFINITY, f64::max);
        let roof =
            plate.iter().copied().filter(|p| top.is_finite() && *p > top + 1e-6 && *p <= top + MAX_STOREY_FT).fold(f64::NEG_INFINITY, f64::max);
        if roof.is_finite() {
            elevation.insert("R".into(), roof);
            source.insert("R".into(), "top of plate");
        }
    }
    // The parapets round the roof: each top of parapet marked above the roof (within a storey of
    // it), as a height above it, lowest first. A building whose parapet steps (low, mid, high)
    // has several.
    let mut parapets: Vec<f64> = Vec::new();
    if let Some(roof) = elevation.get("R").copied() {
        for p in parapet.iter().copied().filter(|p| *p > roof + 1e-6 && *p <= roof + MAX_STOREY_FT) {
            let h = ((p - roof) * 96.0).round() / 96.0;
            if !parapets.iter().any(|q| same(*q, h)) {
                parapets.push(h);
            }
        }
        parapets.sort_by(f64::total_cmp);
        parapets.truncate(MAX_PARAPETS);
    }
    // The same tops by the facade marking each one.
    let mut parapet_tops: Vec<ParapetTop> = Vec::new();
    if let Some(roof) = elevation.get("R").copied() {
        for (p, facade) in parapet_marks.iter().filter(|(p, _)| *p > roof + 1e-6 && *p <= roof + MAX_STOREY_FT) {
            let h = ((p - roof) * 96.0).round() / 96.0;
            if !parapet_tops.iter().any(|t| same(t.height_ft, h) && t.facade == *facade) {
                parapet_tops.push(ParapetTop { height_ft: h, facade: facade.clone() });
            }
        }
        parapet_tops.sort_by(|a, b| a.height_ft.total_cmp(&b.height_ft).then(a.facade.cmp(&b.facade)));
        parapet_tops.truncate(MAX_PARAPETS * 4);
    }
    // Above the first floor: its own mark is the datum (`100'-0"`, or the true elevation).
    let datum = elevation.get("1").copied().filter(|e| e.is_finite()).unwrap_or(0.0);
    for (i, c) in codes.iter().enumerate() {
        let el = elevation.get(c).map(|e| e - datum);
        // Floor to floor reaches the next storey: a mezzanine over a floor is inside its storey.
        let mezzanine = cadaddy_walk3d::levels::is_mezzanine(c);
        let next = codes
            .get(i + 1..)
            .unwrap_or_default()
            .iter()
            .find(|n| mezzanine || !cadaddy_walk3d::levels::is_mezzanine(n))
            .and_then(|n| elevation.get(n))
            .map(|e| e - datum);
        out.levels.push(IndexLevel {
            name: c.clone(),
            sheets: by_level.get(c).cloned().unwrap_or_default(),
            elevation_ft: el,
            height_ft: el.zip(next).map(|(a, b)| b - a).filter(|h| *h > 0.0),
            elevation_source: if el.is_some() { source.get(c).map(|s| (*s).to_string()).unwrap_or_default() } else { String::new() },
            parapets_ft: if c == "R" { parapets.clone() } else { Vec::new() },
            parapet_tops: if c == "R" { parapet_tops.clone() } else { Vec::new() },
        });
    }
    // A plan whose title names no level: in a building of one level it can only be that one
    // (given it, to be confirmed); with several the level is uncertain, so the sheet stays off
    // every level until someone says which (`ask`), never put on the lowest. An enlarged plan
    // with no level belongs to its parent floor's region, found by where it is drawn.
    for s in out.sheets.iter_mut().filter(|s| is_level_plan_type(&s.sheet_type) && s.level.is_empty()) {
        if one_level {
            s.level = lowest.clone();
            s.level_source = "default".into();
        } else {
            s.level_source = "ask".into();
        }
        out.plans_without_level.push(s.number.clone());
    }
    // Numbers printed on different sheets: the sheet that kept the number and the ones renamed.
    let base = |e: &SheetEntry| e.recognised.as_ref().map(|r| if r.printed_number.is_empty() { r.number.clone() } else { r.printed_number.clone() });
    let mut clashes: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    for e in project.sheets.iter().take(MAX_SHEETS).filter(|e| !e.printed_as.is_empty()) {
        clashes.entry(e.printed_as.as_str()).or_default();
    }
    for e in project.sheets.iter().take(MAX_SHEETS) {
        let Some(b) = base(e) else { continue };
        let key = if e.printed_as.is_empty() { b.as_str() } else { e.printed_as.as_str() };
        if let Some(v) = clashes.get_mut(key)
            && !v.iter().any(|(n, _)| *n == b)
        {
            v.push((b.clone(), source_of(e)));
        }
    }
    out.number_conflicts = clashes
        .into_iter()
        .map(|(number, v)| NumberConflict {
            number: number.to_string(),
            sheets: v.iter().map(|(n, _)| n.clone()).collect(),
            sources: v.into_iter().map(|(_, s)| s).collect(),
        })
        .collect();
    out.floor_plans_filed_otherwise = floor_plans_filed_otherwise(&out);
    out
}

/// The sheets of `index` titled as a floor plan but filed as something else (`FLOOR PLAN NOTES`, a
/// schedule) while a storey has no plan sheet titled as its floor plan, each with the first such
/// storey (Q6514). Architecture's sheets when the set has any, else every discipline's; the site
/// and the roof need no floor plan. Empty when every storey has its floor plan.
fn floor_plans_filed_otherwise(index: &SheetIndex) -> Vec<FloorPlanFiledOtherwise> {
    let arch = index.sheets.iter().any(|s| s.discipline.eq_ignore_ascii_case("A"));
    let ours = |s: &&IndexSheet| !arch || s.discipline.eq_ignore_ascii_case("A");
    let site = |s: &IndexSheet| s.level == "SITE" || crate::sheet_facts::is_site_sheet(&s.discipline, &s.title);
    let misfiled: Vec<&IndexSheet> = index
        .sheets
        .iter()
        .filter(ours)
        .filter(|s| !is_plan_type(&s.sheet_type) && !site(s) && sheet_reader::names_floor_plan(&s.title))
        .take(MAX_FILED_OTHERWISE)
        .collect();
    if misfiled.is_empty() {
        return Vec::new();
    }
    let titled_floor = |s: &&IndexSheet| s.sheet_type == "plan" && sheet_reader::names_floor_plan(&s.title);
    let has_floor_plan = |l: &str| index.sheets.iter().filter(ours).filter(titled_floor).any(|s| s.level == l || s.levels.iter().any(|x| x == l));
    let storeys: Vec<&str> =
        index.levels.iter().map(|l| l.name.as_str()).filter(|l| *l != "R" && !cadaddy_walk3d::levels::is_datum_name(l)).collect();
    let without = if storeys.is_empty() {
        // No storey read at all: one with no floor plan, unless a sheet is titled as one.
        (!index.sheets.iter().filter(ours).any(|s| titled_floor(&s))).then(String::new)
    } else {
        storeys.into_iter().find(|l| !has_floor_plan(l)).map(str::to_string)
    };
    let Some(level) = without else { return Vec::new() };
    misfiled
        .into_iter()
        .map(|s| FloorPlanFiledOtherwise { sheet: s.number.clone(), title: s.title.clone(), sheet_type: s.sheet_type.clone(), level: level.clone() })
        .collect()
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

#[cfg(test)]
mod classify_tests {
    use super::{SheetEntry, canonical_discipline, classify_sheet_type, level_by_number, set_series};

    #[test]
    fn a_series_is_five_numbers_of_a_disciplines_letters() {
        let s = |v: &[&str]| set_series(v.iter().map(|x| x.to_string())).into_iter().collect::<Vec<_>>();
        assert_eq!(s(&["AJ101", "AJ102", "AJ103", "AJ104.1", "aj104.2", "M-101"]), ["AJ"]);
        // a mark repeated page to page is one number; four numbers are no series
        assert!(s(&["L9", "L9", "L9", "L9", "L9", "L9"]).is_empty());
        assert!(s(&["AJ101", "AJ102", "AJ103", "AJ104"]).is_empty());
        // letters naming no discipline, or three of them, make no series
        assert!(s(&["XQ1", "XQ2", "XQ3", "XQ4", "XQ5"]).is_empty());
        assert!(s(&["RTU1", "RTU2", "RTU3", "RTU4", "RTU5"]).is_empty());
        assert!(s(&[]).is_empty());
    }

    #[test]
    fn a_designator_with_a_modifier_is_its_discipline() {
        // the first letter names the discipline, the next the kind of sheet within it
        for (letters, want) in [("A", "A"), ("AC", "A"), ("AE", "A"), ("ep", "E"), ("MP", "M"), ("SD", "S")] {
            assert_eq!(canonical_discipline(letters), want, "{letters}");
        }
        // two letters that are a discipline of their own stay it; letters naming none stay
        for (letters, want) in [("FP", "FP"), ("FA", "FA"), ("SL", "SL"), ("LP", "LP"), ("XY", "XY"), ("", ""), ("A1", "A1")] {
            assert_eq!(canonical_discipline(letters), want, "{letters}");
        }
    }

    fn plan(number: &str, title: &str) -> SheetEntry {
        SheetEntry { number: number.into(), title: title.into(), sheet_type: "plan".into(), ..SheetEntry::default() }
    }

    #[test]
    fn a_numbered_series_is_levels_only_when_it_is_one_kind_of_plan() {
        // Different plans of one floor numbered in a row: no levels from their numbers.
        let mut one = vec![plan("A-101", "FLOOR PLAN"), plan("A-102", "EQUIPMENT PLAN"), plan("A-105", "FINISH PLAN")];
        level_by_number(&mut one);
        assert!(one.iter().all(|e| e.level.is_empty()), "{:?}", one.iter().map(|e| &e.level).collect::<Vec<_>>());
        // One kind of plan floor after floor: the last digit is the level.
        let mut floors = vec![plan("A-111", "OVERALL PLAN"), plan("A-112", "OVERALL PLAN"), plan("A-113", "OVERALL PLAN")];
        level_by_number(&mut floors);
        assert_eq!(floors.iter().map(|e| e.level.as_str()).collect::<Vec<_>>(), ["1", "2", "3"]);
    }

    #[test]
    fn a_sheet_also_printing_the_floor_plan_is_a_plan_not_a_ceiling_plan() {
        assert_eq!(classify_sheet_type("A-301", "SECOND FLOOR PLAN, ROOF PLAN AND RCP"), "plan");
        assert_eq!(classify_sheet_type("A221", "FIRST FLOOR REFLECTED CEILING PLAN"), "rcp", "a floor's ceiling plan isn't a floor plan");
        assert_eq!(classify_sheet_type("A222", "RCP"), "rcp");
        assert_eq!(classify_sheet_type("A223", "REFLECTED CEILING PLAN"), "rcp");
        assert_eq!(classify_sheet_type("Q41", "ELECTRICAL SPECIFICATIONS"), "specification");
        assert_eq!(classify_sheet_type("Q702", "PLUMBING SPECIFICATION"), "specification");
        assert_eq!(classify_sheet_type("Q703", "GENERAL NOTES AND SPECIFICATIONS"), "schedule", "notes stay notes");
        assert_eq!(classify_sheet_type("Q201", "FLOOR PLAN"), "plan");
    }

    #[test]
    fn a_title_listing_parts_is_filed_by_the_plan_it_lists() {
        // Q6514: a part naming a plan makes the sheet that plan, whatever comes after it.
        for (title, want) in [
            ("FLOOR PLAN/ DETAILS/ SCHEDULES", "plan"),
            ("REFLECTED CEILING PLAN/ CEILING DETAILS", "rcp"),
            ("ROOF PLAN/ ROOF DETAILS", "plan"),
            ("CANOPY PLAN/ ELEVATIONS", "plan"),
            ("DETAILS, SCHEDULES & FLOOR PLAN", "plan"),
            ("CEILING DETAILS AND REFLECTED CEILING PLAN", "rcp"),
            ("ENLARGED PLANS / DETAILS", "enlarged_plan"),
            ("POWER PLAN / RISER DIAGRAM", "plan"),
            ("LIFE SAFETY PLAN / CODE ANALYSIS", "plan"),
            ("OVERALL ROOF PLAN & DETAILS", "plan"),
            ("FLOOR PLAN AND SECTIONS", "plan"),
            ("FIRST AND SECOND FLOOR PLANS", "plan"),
            // a floor plan part outranks the ceiling plan listed first
            ("REFLECTED CEILING PLAN / FLOOR PLAN", "plan"),
            // an enlarged plan beside a part that names no kind of drawing: that part is the floor's plan
            ("EQUIPMENT/ ENLARGED STORAGE/ LIFE SAFETY", "plan"),
        ] {
            assert_eq!(classify_sheet_type("A1", title), want, "{title}");
        }
        // No part names a plan: as before.
        for (title, want) in [
            ("DOOR/ WINDOW SCHEDULES", "schedule"),
            ("WALL SECTIONS & PLAN DETAILS", "detail"),
            ("LEGEND, NOTES & KEY PLAN", "schedule"),
            ("GENERAL NOTES AND SPECIFICATIONS", "schedule"),
            ("COVER SHEET / SITE PLAN", "cover"),
            ("BUILDING SECTIONS", "section"),
            ("EXTERIOR ELEVATIONS", "elevation"),
            ("ENLARGED PLAN SECTIONS", "section"),
            ("FLOOR PLAN NOTES", "schedule"),
            ("SECTION 1/2", "section"),
            // a callout or an abbreviation beside an enlarged plan names no floor plan
            ("SEE ENLARGED PLAN 1/E-6", "enlarged_plan"),
            ("ENLARGED PLAN, TYP.", "enlarged_plan"),
        ] {
            assert_eq!(classify_sheet_type("A1", title), want, "{title}");
        }
    }
}
