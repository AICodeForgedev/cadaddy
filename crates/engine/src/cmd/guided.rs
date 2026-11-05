//! Guided pipeline: software does the legwork, the agent checks work and answers questions at
//! checkpoints (Q5245). Each stage is deterministic code; the agent never sequences commands.
//!
//! A **job** runs a fixed pipeline on a print set: import → classify → levels/scale → rooms →
//! devices/symbols → bim.build → takeoff → estimate|clash. Between stages (and within them),
//! the pipeline pauses with **questions** when confidence is low or a check fails. The agent
//! (or a human) answers; answers become project rules for future runs.
//!
//! API: `job.start`, `job.next`, `job.answer`, `job.status`, `job.explain`, `job.rules`,
//! `job.report`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

use super::{CommandSpec, bad, str_param};
use crate::{Result, Session};

// ─── Limits ─────────────────────────────────────────────────────────────────

/// Most questions one stage may produce (hostile inputs).
const MAX_QUESTIONS_PER_STAGE: usize = 500;
/// Most verify samples per stage.
const MAX_VERIFY_SAMPLES: usize = 20;
/// Most texts read per sheet when looking for its legends and schedules.
const MAX_SHEET_TEXTS: usize = 200_000;
/// Most rules a job may learn.
const MAX_RULES: usize = 5_000;
/// Most audit entries kept (oldest trimmed).
const MAX_AUDIT: usize = 50_000;
/// Block timeout for `job.next` (ms) before returning progress (reserved for future use).
const _DEFAULT_WAIT_MS: u64 = 5_000;
/// Most worklist items kept.
const MAX_WORKLIST: usize = 10_000;
/// Verify samples a stage asks about when the job asks (the rest take their default): a spot
/// check, not every room.
const VERIFY_ASKED: usize = 3;
/// Most numbers kept when comparing a stage's result before and after an answer.
const MAX_DIGEST: usize = 400;

// ─── Types ──────────────────────────────────────────────────────────────────

/// The goals a guided job can pursue; each stops at a different stage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobGoal {
    Cdy,
    Bim,
    Takeoff,
    Estimate,
    Clash,
    /// Everything through clash detection, then the construction sequence (`schedule.build`).
    Sequence,
}

impl JobGoal {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "cdy" => Some(Self::Cdy),
            "bim" => Some(Self::Bim),
            "takeoff" => Some(Self::Takeoff),
            "estimate" => Some(Self::Estimate),
            "clash" => Some(Self::Clash),
            "sequence" => Some(Self::Sequence),
            _ => None,
        }
    }

    /// The last stage this goal needs.
    fn last_stage(&self) -> StageName {
        match self {
            Self::Cdy => StageName::Import,
            Self::Bim => StageName::BimBuild,
            Self::Takeoff => StageName::Takeoff,
            Self::Estimate => StageName::Estimate,
            Self::Clash | Self::Sequence => StageName::Clash,
        }
    }
}

/// Fixed pipeline stages in execution order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageName {
    Import,
    Classify,
    Levels,
    Rooms,
    Devices,
    MepRead,
    BimBuild,
    Takeoff,
    Estimate,
    Clash,
}

impl StageName {
    pub const ALL: &'static [StageName] = &[
        StageName::Import,
        StageName::Classify,
        StageName::Levels,
        StageName::Rooms,
        StageName::Devices,
        StageName::MepRead,
        StageName::BimBuild,
        StageName::Takeoff,
        StageName::Estimate,
        StageName::Clash,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Classify => "classify",
            Self::Levels => "levels",
            Self::Rooms => "rooms",
            Self::Devices => "devices",
            Self::MepRead => "mep_read",
            Self::BimBuild => "bim_build",
            Self::Takeoff => "takeoff",
            Self::Estimate => "estimate",
            Self::Clash => "clash",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Import => "Import print set (PDFs/DWGs)",
            Self::Classify => "Classify sheets by discipline and level",
            Self::Levels => "Detect levels, scales and elevations",
            Self::Rooms => "Detect rooms, names and areas",
            Self::Devices => "Detect devices, symbols and counts",
            Self::MepRead => "Read MEP ductwork, piping and equipment",
            Self::BimBuild => "Build 3D BIM model from spec",
            Self::Takeoff => "Quantity takeoff",
            Self::Estimate => "Estimate (prices and labor)",
            Self::Clash => "Clash detection",
        }
    }
}

/// Status of a pipeline stage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageStatus {
    /// Not started yet.
    Pending,
    /// Running (in the middle of deterministic work).
    Running,
    /// Paused: has unanswered questions.
    Paused,
    /// Done: all questions answered, stage complete.
    Done,
    /// Skipped: not needed for this goal.
    Skipped,
    /// Error: stage failed (message stored in result).
    Error,
}

/// State of one pipeline stage.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StageState {
    pub name: StageName,
    pub status: StageStatus,
    /// Questions this stage raised.
    pub questions: Vec<Question>,
    /// Verify samples (spot-check after stage completes).
    pub verify: Vec<Question>,
    /// Completeness checkpoint questions (MUST 9-11).
    pub completeness: Vec<Question>,
    /// Stage result data (counts, summaries).
    pub result: Option<Value>,
    /// Error message if status is Error.
    pub error: Option<String>,
    /// How long the stage's work took (ms), once it has run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
}

impl StageState {
    pub fn new(name: StageName) -> Self {
        Self {
            name,
            status: StageStatus::Pending,
            questions: Vec::new(),
            verify: Vec::new(),
            completeness: Vec::new(),
            result: None,
            error: None,
            elapsed_ms: None,
        }
    }

    /// All unanswered questions across regular, verify and completeness.
    pub fn unanswered(&self) -> Vec<&Question> {
        self.questions.iter().chain(self.verify.iter()).chain(self.completeness.iter()).filter(|q| q.answer.is_none()).collect()
    }
}

/// A question the pipeline asks the agent or user.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Question {
    /// Unique id: `Q-<stage>-<seq>`.
    pub id: String,
    /// Which stage raised it.
    pub stage: StageName,
    /// The question text.
    pub question: String,
    /// Why the pipeline is asking (confidence too low, check failed, verify sample).
    pub why: String,
    /// Offered choices (may be empty for free-form).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// Evidence supporting the question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<QuestionEvidence>,
    /// Default answer if the agent accepts without choosing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// JSON schema the answer must match (or null for any string).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_schema: Option<Value>,
    /// The answer, once provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Value>,
    /// Who answered (agent, user, default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<String>,
    /// When answered (ISO 8601).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<String>,
    /// Kind of question: doubt, verify, completeness.
    #[serde(default)]
    pub kind: QuestionKind,
}

/// Evidence attached to a question: crops, values, references.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuestionEvidence {
    /// Path to a crop image (PNG).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop_png_path: Option<String>,
    /// Sheet name or number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    /// Location on the sheet (drawing coordinates).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    /// Values the pipeline read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Value>,
    /// Handle references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handles: Vec<String>,
    /// For completeness: tiled crops with every counted item boxed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiled_crops: Option<Vec<String>>,
    /// Leftover candidates not matched to any library symbol.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub leftovers: Vec<LeftoverCandidate>,
}

/// An unmatched symbol-like cluster found during completeness checking.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeftoverCandidate {
    /// Description of what was found.
    pub description: String,
    /// Crop image path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop_path: Option<String>,
    /// Location on the sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<[f64; 2]>,
    /// Sheet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    /// Count of similar clusters.
    pub count: usize,
}

/// What kind of question this is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    /// Confidence doubt or check failure.
    #[default]
    Doubt,
    /// Spot-check verify sample.
    Verify,
    /// Completeness: "what did I miss?"
    Completeness,
}

/// A finding from a stage that needs attention: empty result, error, or other issue.
/// Unlike a question (which pauses the pipeline), a finding is logged on the worklist
/// and may or may not block the pipeline.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobFinding {
    /// Unique id: `F-<stage>-<seq>`.
    pub id: String,
    pub stage: StageName,
    /// What was found (or not found).
    pub description: String,
    /// Why this is a finding.
    pub why: String,
    /// Severity: 1 (info) to 5 (critical).
    pub severity: u8,
    /// Suggested actions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
    /// Evidence supporting the finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<QuestionEvidence>,
    /// Whether this blocks the pipeline from advancing past this stage.
    #[serde(default)]
    pub blocking: bool,
}

/// A work item on the job worklist: a question, finding, or action with undo support.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkItem {
    /// Unique id.
    pub id: String,
    /// `question`, `finding`, `action`.
    pub kind: String,
    pub stage: StageName,
    pub summary: String,
    /// Status: `pending`, `resolved`, `deferred`.
    #[serde(default = "default_pending")]
    pub status: String,
    /// Evidence (crop, values, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<QuestionEvidence>,
    /// Available actions / choices.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
    /// The resolution (answer or action taken).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Value>,
    /// Delta for undo: what command to run to reverse the resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_delta: Option<Value>,
}

fn default_pending() -> String {
    "pending".into()
}

/// An entry in the audit trail.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditEntry {
    /// ISO 8601 timestamp.
    pub when: String,
    /// What happened.
    pub event: String,
    /// Who did it (agent, user, pipeline, default).
    pub who: String,
    /// Question id, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question_id: Option<String>,
    /// Stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<StageName>,
    /// Details.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

/// A learned project rule from an answer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LearnedRule {
    /// Rule key: `symbol:HEXAGON_WP -> device:weatherproof_receptacle`.
    pub key: String,
    /// Rule value.
    pub value: String,
    /// Which question taught it.
    pub from_question: String,
    /// Which stage.
    pub stage: StageName,
    /// How many times it has been applied automatically.
    #[serde(default)]
    pub auto_applied: usize,
}

/// The full state of a guided job (serialized to JSON alongside the .cdy).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GuidedJob {
    /// Unique job id.
    pub id: String,
    /// The goal: how far the pipeline goes.
    pub goal: JobGoal,
    /// ISO 8601 creation time.
    pub created: String,
    /// The print-set path (folder of PDFs).
    pub pdfs_path: String,
    /// Pipeline stages in order.
    pub stages: Vec<StageState>,
    /// Learned project rules from answers.
    pub rules: Vec<LearnedRule>,
    /// Audit trail.
    pub audit: Vec<AuditEntry>,
    /// Findings from stages (empty results, errors, etc.).
    #[serde(default)]
    pub findings: Vec<JobFinding>,
    /// Worklist: questions, findings, and actions unified for the UI.
    #[serde(default)]
    pub worklist: Vec<WorkItem>,
    /// Classified sheet info cached after classify stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheets: Option<Value>,
    /// Global question counter (for unique ids).
    question_seq: u32,
    /// Global finding counter.
    #[serde(default)]
    finding_seq: u32,
    /// True once plan-sheet PDFs have been imported into the session's drawing(s).
    #[serde(default)]
    sheets_loaded: bool,
    /// Where the job's drawing is saved after each stage (none: no checkpoints).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    /// The checkpoint is a path the caller gave: it stays when the job is done (only the job's
    /// own one in the temporary folder is removed).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub checkpoint_given: bool,
    /// Submittal PDFs (or folders of them) whose products go onto the built elements.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub submittals: Vec<String>,
    /// Specification section PDFs (or folders of them): what each kind of product must be.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub specs: Vec<String>,
    /// The type marks the submittals give products for: device types as the legends' are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub submittal_types: Vec<String>,
    /// Those of them that are luminaires (their cut sheets): they hang at the ceiling.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub luminaire_types: Vec<String>,
    /// The luminaires that go on a wall (an exit sign, a sconce, a wall pack).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wall_luminaire_types: Vec<String>,
    /// What the print folder held when the job started ([`prints_stamp`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prints_stamp: String,
    /// The set's sheet index from classify: one entry per real sheet, and the levels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<Value>,
    /// Where each imported plan sheet sits in the drawing (side by side, a level's sheets
    /// together), so what is read on it carries the sheet's level.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sheet_regions: Vec<SheetRegion>,
    /// The objects each imported sheet added (the sheets overlap in model space, so each
    /// sheet's legend applies to its own symbols only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sheet_spans: Vec<SheetSpan>,
    /// The devices the devices stage confirmed (type, where, sheet, level, circuit): the
    /// bim_build stage puts them in the model after it drafts the building spec.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<super::readers::ReadDevice>,
    /// What the mechanical and plumbing plans read (mep_read), each run on its sheet's level:
    /// the BIM build puts it into the spec it drafts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mep: Option<cadaddy_walk3d::Mep>,
    /// The branch circuits the electrical plans and the devices give (mep_read, Q6007): one
    /// `circuit` tag each, whose conduit runs are in [`Self::mep`]; the BIM build puts them in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub circuits: Vec<cadaddy_walk3d::tags::ElementTag>,
    /// Auto mode (`job.start {auto: true}`): every question with a default takes it and the job
    /// runs on. Off (the default) the job stops at each checkpoint until it is answered.
    #[serde(default)]
    pub auto: bool,
    /// Questions asked even in auto mode: a stage that should find things found none.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub always_ask: BTreeSet<String>,
    /// Floor to floor (feet) given as an answer, over the sections' and the 10' guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storey_ft: Option<f64>,
    /// The levels an answer named when the drawings gave none (`1, 2, 3, R`), bottom up: the
    /// levels stage makes them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels_given: Vec<String>,
    /// What answers said about each level (its elevation, its name, or that it is right), by
    /// the level as the questions name it, upper case.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub level_answers: BTreeMap<String, LevelAnswer>,
    /// The first stage answers given with `rerun: false` changed: it runs again on the next
    /// `job.answer` or `job.next`, once for all of them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerun_held: Option<StageName>,
    /// The drawing's building spec when each stage started (stages store their own, and the
    /// readers use it): a stage that runs again starts from the one it had.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    spec_at_start: BTreeMap<StageName, Option<String>>,
    /// Stopped by a cancel (MCP `notifications/cancelled`, the `cancel` tool) at the stage it
    /// names; job.next goes on from there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<StageName>,
    /// Answered questions of stages being run again, to carry their answers over.
    #[serde(skip)]
    carry: Vec<Question>,
    /// The sheet map read off the loaded sheets, in short ([`super::printset::links_summary`]):
    /// callouts resolved, enlarged plans registered, match lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Value>,
}

/// A level as an answer gave it: the elevation and name the user typed, or "Correct".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LevelAnswer {
    /// Feet, as the user gave it (over every reading).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation_ft: Option<f64>,
    /// The level's name in the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The user confirmed what the job read.
    #[serde(default)]
    pub confirmed: bool,
}

/// The handles one sheet's import created: `lo..hi`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SheetSpan {
    /// The sheet's number (`E-4`), or its file when it has none.
    pub sheet: String,
    pub lo: u64,
    pub hi: u64,
}

/// An imported sheet's rectangle in the job's drawing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SheetRegion {
    pub number: String,
    pub level: String,
    /// The sheet's discipline letter (`A`, `E`, `M`…) and type (`plan`, `enlarged_plan`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub discipline: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// `[x0, y0, x1, y1]`, drawing units.
    pub rect: [f64; 4],
    /// Where the sheet's views are: inside its border, off its title block (the whole `rect`
    /// when no frame was found). Nothing outside it is building.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<[f64; 4]>,
    /// The title-block strip, when one was found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_block: Option<[f64; 4]>,
    /// Why the sheet is never built into a building level (`site/civil`, `no level`); empty
    /// for a sheet of a storey.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub not_built: String,
    /// Every level this sheet covers (a typical-floor plan covers several).  Empty means
    /// only `level`.  When non-empty, `level` equals `levels[0]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub levels: Vec<String>,
    /// For a sheet printing several plans of different levels (`3RD FLOOR PLAN` beside `4TH
    /// FLOOR PLAN`): the one view this region is, so its level takes only what lies in it. The
    /// sheet is then one region per view, all with the same `rect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<SheetView>,
}

pub use cadaddy_walk3d::levels::SheetView;

impl SheetRegion {
    /// Every level this sheet covers. Falls back to `[level]` when `levels` is empty.
    pub fn all_levels(&self) -> Vec<String> {
        if self.levels.is_empty() { if self.level.is_empty() { Vec::new() } else { vec![self.level.clone()] } } else { self.levels.clone() }
    }

    /// Whether `at` is in the sheet's drawing area (off its border and title block).
    pub fn in_area(&self, at: [f64; 2]) -> bool {
        let r = self.area.unwrap_or(self.rect);
        at[0] >= r[0].min(r[2]) && at[0] <= r[0].max(r[2]) && at[1] >= r[1].min(r[3]) && at[1] <= r[1].max(r[3])
    }

    /// Whether `at` is in this region's view (the whole sheet for a sheet of one view).
    pub fn in_view(&self, at: [f64; 2]) -> bool {
        self.view.as_ref().is_none_or(|v| in_rect(&v.rect, at))
    }

    /// Where the level's plan is: its view, else the sheet's drawing area, else the sheet.
    pub fn plan_rect(&self) -> [f64; 4] {
        self.view.as_ref().map_or_else(|| self.area.unwrap_or(self.rect), |v| v.rect)
    }
}

fn in_rect(r: &[f64; 4], at: [f64; 2]) -> bool {
    at[0] >= r[0].min(r[2]) && at[0] <= r[0].max(r[2]) && at[1] >= r[1].min(r[3]) && at[1] <= r[1].max(r[3])
}

/// Why a sheet is never built into a building level: a civil, landscape or site sheet, or a
/// plan whose title and number name no level (it was only given the lowest, to confirm).
pub fn not_built_reason(discipline: &str, title: &str, level: &str, level_source: &str) -> &'static str {
    if cadaddy_walk3d::levels::is_datum_name(level) || cadaddy_mepfix::sheet_facts::is_site_sheet(discipline, title) {
        "site/civil"
    } else if level.trim().is_empty() || level_source == "default" {
        "no level"
    } else {
        ""
    }
}

/// The job's plan sheets that print several plans of different levels (`3RD FLOOR PLAN` beside
/// `4TH FLOOR PLAN`), each split into one region per view: the view's level, its title and its
/// part of the sheet ([`super::printmep::sheet_views`]). A sheet of one view keeps its region, a
/// typical-floor plan its several levels: it is the one case where one view feeds several levels.
/// `typical[i]`: region `i`'s sheet is titled as a typical plan. Any other sheet that shows
/// several levels is split even when its views carry no title the readers find (by its blocks
/// of linework), so no floor-specific plan feeds two levels.
pub(crate) fn split_view_regions(d: &cadaddy_doc::Drawing, regions: Vec<SheetRegion>, typical: &[bool]) -> Vec<SheetRegion> {
    let candidate = |r: &SheetRegion| matches!(r.kind.as_str(), "plan" | "enlarged_plan") && r.view.is_none();
    let sheets: Vec<Option<([f64; 4], [f64; 4], Vec<String>)>> = regions
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let levels = if typical.get(i).copied().unwrap_or(false) { Vec::new() } else { r.levels.clone() };
            candidate(r).then(|| (r.rect, r.area.unwrap_or(r.rect), levels))
        })
        .collect();
    let mut views = super::printmep::sheet_views(d, &sheets);
    let mut out = Vec::with_capacity(regions.len());
    for (i, r) in regions.into_iter().enumerate() {
        let found = views.get_mut(i).map(std::mem::take).unwrap_or_default();
        if found.is_empty() {
            out.push(r);
            continue;
        }
        for v in found {
            let not_built = if r.not_built == "site/civil" { r.not_built.clone() } else { String::new() };
            out.push(SheetRegion {
                level: v.level,
                levels: v.levels,
                not_built,
                view: Some(SheetView { title: v.title, rect: v.rect }),
                ..r.clone()
            });
        }
    }
    out
}

/// Straight lines longer than this share of a sheet's shorter side are measured for its frame.
const FRAME_RULE_SHARE: f64 = 0.25;

/// Each sheet's frame ([`cadaddy_mepfix::sheet_facts::sheet_frame`]): its long straight lines
/// (one pass over the drawing), then its border and title block.
fn sheet_frames(d: &cadaddy_doc::Drawing, rects: &[[f64; 4]]) -> Vec<cadaddy_mepfix::sheet_facts::SheetFrame> {
    use cadaddy_doc::EntityKind;
    use cadaddy_geom::Vec2;
    let shortest =
        rects.iter().map(|r| (r[2] - r[0]).abs().min((r[3] - r[1]).abs())).filter(|v| v.is_finite() && *v > 0.0).fold(f64::INFINITY, f64::min);
    let mut per: Vec<Vec<(Vec2, Vec2)>> = vec![Vec::new(); rects.len()];
    if shortest.is_finite() {
        let min_len = FRAME_RULE_SHARE * shortest;
        let mut put = |a: Vec2, b: Vec2| {
            if !(a.is_finite() && b.is_finite()) || a.dist(b) < min_len {
                return;
            }
            let m = (a + b) * 0.5;
            let inside = |r: &[f64; 4]| m.x >= r[0].min(r[2]) && m.x <= r[0].max(r[2]) && m.y >= r[1].min(r[3]) && m.y <= r[1].max(r[3]);
            if let Some(i) = rects.iter().position(inside)
                && let Some(v) = per.get_mut(i)
            {
                v.push((a, b));
            }
        };
        for e in d.model.iter() {
            match &e.kind {
                EntityKind::Line(l) => put(l.a.xy(), l.b.xy()),
                EntityKind::LwPolyline(p) => {
                    for w in p.vertices.windows(2) {
                        if let [a, b] = w {
                            put(a.p, b.p);
                        }
                    }
                    if p.closed
                        && let (Some(a), Some(b)) = (p.vertices.last(), p.vertices.first())
                    {
                        put(a.p, b.p);
                    }
                }
                _ => {}
            }
        }
    }
    rects.iter().zip(&per).map(|(r, segs)| cadaddy_mepfix::sheet_facts::sheet_frame(segs, *r)).collect()
}

impl GuidedJob {
    /// The level of the imported sheet holding `at`.
    /// A sheet printing several levels' plans is one region per view: the view holding `at`
    /// (outside every view, its notes and title block: the sheet's first region).
    pub fn level_at(&self, at: [f64; 2]) -> Option<&SheetRegion> {
        let on_sheet = |r: &&SheetRegion| at[0] >= r.rect[0] && at[0] <= r.rect[2] && at[1] >= r.rect[1] && at[1] <= r.rect[3];
        let mut here = self.sheet_regions.iter().filter(on_sheet);
        let first = here.next()?;
        if first.view.is_none() || first.in_view(at) {
            return Some(first);
        }
        Some(here.take_while(|r| r.rect == first.rect).find(|r| r.in_view(at)).unwrap_or(first))
    }
}

impl GuidedJob {
    pub fn new(pdfs: &str, goal: JobGoal) -> Self {
        let stages: Vec<StageState> = StageName::ALL
            .iter()
            .map(|&n| {
                let mut st = StageState::new(n);
                if n > goal.last_stage() {
                    st.status = StageStatus::Skipped;
                }
                st
            })
            .collect();
        Self {
            id: format!("job-{:08x}", {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut h = DefaultHasher::new();
                pdfs.hash(&mut h);
                // Mix in something unique enough without std::time on wasm.
                goal.last_stage().label().hash(&mut h);
                (h.finish() & 0xFFFF_FFFF) as u32
            }),
            goal,
            created: now_iso(),
            pdfs_path: pdfs.to_string(),
            stages,
            rules: Vec::new(),
            audit: Vec::new(),
            findings: Vec::new(),
            worklist: Vec::new(),
            sheets: None,
            question_seq: 0,
            finding_seq: 0,
            sheets_loaded: false,
            checkpoint: None,
            checkpoint_given: false,
            submittals: Vec::new(),
            specs: Vec::new(),
            submittal_types: Vec::new(),
            luminaire_types: Vec::new(),
            wall_luminaire_types: Vec::new(),
            prints_stamp: String::new(),
            index: None,
            sheet_regions: Vec::new(),
            sheet_spans: Vec::new(),
            devices: Vec::new(),
            mep: None,
            circuits: Vec::new(),
            auto: false,
            always_ask: BTreeSet::new(),
            storey_ft: None,
            levels_given: Vec::new(),
            level_answers: BTreeMap::new(),
            rerun_held: None,
            spec_at_start: BTreeMap::new(),
            cancelled: None,
            carry: Vec::new(),
            links: None,
        }
    }

    /// The current stage (first non-done, non-skipped).
    pub fn current_stage(&self) -> Option<&StageState> {
        self.stages.iter().find(|s| !matches!(s.status, StageStatus::Done | StageStatus::Skipped))
    }

    pub fn stage(&self, name: StageName) -> Option<&StageState> {
        self.stages.iter().find(|s| s.name == name)
    }

    pub fn stage_mut(&mut self, name: StageName) -> Option<&mut StageState> {
        self.stages.iter_mut().find(|s| s.name == name)
    }

    /// The next unanswered question across all stages.
    pub fn next_question(&self) -> Option<&Question> {
        for stage in &self.stages {
            for q in stage.questions.iter().chain(stage.verify.iter()).chain(stage.completeness.iter()) {
                if q.answer.is_none() {
                    return Some(q);
                }
            }
        }
        None
    }

    /// Is the pipeline finished?
    pub fn is_done(&self) -> bool {
        self.stages.iter().all(|s| matches!(s.status, StageStatus::Done | StageStatus::Skipped))
    }

    /// Fraction complete (0.0–1.0).
    pub fn progress(&self) -> f64 {
        let active: Vec<_> = self.stages.iter().filter(|s| s.status != StageStatus::Skipped).collect();
        if active.is_empty() {
            return 1.0;
        }
        let done = active.iter().filter(|s| s.status == StageStatus::Done).count();
        done as f64 / active.len() as f64
    }

    /// Generate a question id.
    pub fn next_qid(&mut self, stage: StageName) -> String {
        self.question_seq = self.question_seq.saturating_add(1);
        format!("Q-{}-{}", stage.label(), self.question_seq)
    }

    /// Add an audit entry.
    pub fn audit(&mut self, event: &str, who: &str, question_id: Option<&str>, stage: Option<StageName>, details: Option<Value>) {
        if self.audit.len() >= MAX_AUDIT {
            // Trim oldest 10%.
            let trim = MAX_AUDIT / 10;
            self.audit.drain(..trim);
        }
        self.audit.push(AuditEntry {
            when: now_iso(),
            event: event.to_string(),
            who: who.to_string(),
            question_id: question_id.map(str::to_string),
            stage,
            details,
        });
    }

    /// Generate a finding id.
    pub fn next_fid(&mut self, stage: StageName) -> String {
        self.finding_seq = self.finding_seq.saturating_add(1);
        format!("F-{}-{}", stage.label(), self.finding_seq)
    }

    /// Add a finding and a corresponding worklist item.
    pub fn add_finding(
        &mut self,
        stage: StageName,
        description: &str,
        why: &str,
        severity: u8,
        actions: Vec<String>,
        evidence: Option<QuestionEvidence>,
        blocking: bool,
    ) {
        let fid = self.next_fid(stage);
        self.findings.push(JobFinding {
            id: fid.clone(),
            stage,
            description: description.to_string(),
            why: why.to_string(),
            severity,
            actions: actions.clone(),
            evidence: evidence.clone(),
            blocking,
        });
        if self.worklist.len() < MAX_WORKLIST {
            self.worklist.push(WorkItem {
                id: fid,
                kind: "finding".into(),
                stage,
                summary: description.to_string(),
                status: if blocking { "pending".into() } else { "info".into() },
                evidence,
                choices: actions,
                resolution: None,
                undo_delta: None,
            });
        }
    }

    /// Add a worklist item for a question.
    pub fn add_question_to_worklist(&mut self, q: &Question) {
        if self.worklist.len() < MAX_WORKLIST {
            self.worklist.push(WorkItem {
                id: q.id.clone(),
                kind: "question".into(),
                stage: q.stage,
                summary: q.question.clone(),
                status: if q.answer.is_some() { "resolved".into() } else { "pending".into() },
                evidence: q.evidence.clone(),
                choices: q.choices.clone(),
                resolution: q.answer.clone(),
                undo_delta: None,
            });
        }
    }

    /// Learn a rule from an answer.
    pub fn learn_rule(&mut self, key: &str, value: &str, question_id: &str, stage: StageName) {
        if self.rules.len() >= MAX_RULES {
            return;
        }
        // Update existing or add new.
        if let Some(r) = self.rules.iter_mut().find(|r| r.key == key) {
            r.value = value.to_string();
            r.from_question = question_id.to_string();
        } else {
            self.rules.push(LearnedRule {
                key: key.to_string(),
                value: value.to_string(),
                from_question: question_id.to_string(),
                stage,
                auto_applied: 0,
            });
        }
    }
}

// ─── Persistence ────────────────────────────────────────────────────────────

/// Job state is stored in `Drawing.agent` under the key `"guided_job"`.
pub(crate) fn load_job(s: &Session) -> Result<Option<GuidedJob>> {
    let agent_text = match &s.doc()?.agent {
        Some(t) if !t.trim().is_empty() => t.clone(),
        _ => return Ok(None),
    };
    let agent: Value = serde_json::from_str(&agent_text).map_err(|e| bad("job", format!("corrupt agent state: {e}")))?;
    match agent.get("guided_job") {
        Some(v) if !v.is_null() => {
            let job: GuidedJob = serde_json::from_value(v.clone()).map_err(|e| bad("job", format!("corrupt job state: {e}")))?;
            Ok(Some(job))
        }
        _ => Ok(None),
    }
}

fn save_job(s: &mut Session, job: &GuidedJob) -> Result<()> {
    let doc = s.doc_mut()?;
    let mut agent: Value = doc.agent.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(json!({}));
    agent["guided_job"] = serde_json::to_value(job).unwrap_or(json!(null));
    doc.agent = Some(agent.to_string());
    Ok(())
}

// ─── Stage runners ──────────────────────────────────────────────────────────
//
// Each stage runner:
// 1. Executes the deterministic commands for its stage
// 2. Inspects results for low-confidence items
// 3. Generates questions (doubt, verify, completeness)
// 4. Sets stage status to Paused (if questions) or Done (if none)

/// Run the import stage: open the print set.
fn run_import(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    let opened = s.execute("printset.open", &json!({"path": job.pdfs_path}))?;
    job.audit("stage_run", "pipeline", None, Some(StageName::Import), Some(json!({"result": "opened"})));
    Ok(opened)
}

/// Run the classify stage: the set's sheet index (MUST 2). Each page's title block names its
/// sheet (number, title, discipline — never the file name); the same sheet in several files is one
/// entry; plans carry the level their titles name. Asks when a discipline or level is unsure.
fn run_classify(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    let classified = s.execute("printset.classify", &json!({}))?;

    // The current copy of every sheet, with its file and page, for the stages that import them.
    let sheets_result = s.execute("printset.sheets", &json!({"current": true}))?;

    // Cache the sheet list for later stages (MUST 4).
    job.sheets = Some(sheets_result);
    job.index =
        Some(json!({"sheets": classified.get("sheets"), "levels": classified.get("levels"), "level_warnings": classified.get("level_warnings")}));

    // MUST 1: Check for empty results.
    // The index: one entry per real sheet (questions are asked once per sheet, not per copy).
    let sheets_vec: Option<Vec<Value>> = classified
        .get("sheets")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| job.sheets.as_ref().and_then(|v| v.get("sheets")).and_then(Value::as_array).cloned());
    let sheet_count = sheets_vec.as_ref().map_or(0, Vec::len);
    // Files (or pages) that couldn't be read: named; none read at all is the job's error.
    let unreadable = classified.get("unreadable").and_then(Value::as_array).cloned().unwrap_or_default();
    if !unreadable.is_empty() {
        let pages = classified.get("pages").and_then(Value::as_u64).map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX));
        let none_read = unreadable.len() >= pages.max(1);
        let named: Vec<String> = unreadable
            .iter()
            .take(20)
            .map(|u| {
                let file = u.get("file").and_then(Value::as_str).unwrap_or("?");
                let error = u.get("error").and_then(Value::as_str).unwrap_or("not readable");
                format!("{file}: {}", error.chars().take(120).collect::<String>())
            })
            .collect();
        let more = unreadable.len().saturating_sub(named.len());
        job.add_finding(
            StageName::Classify,
            &if none_read {
                format!("None of the print set's {} file(s) could be read", unreadable.len())
            } else {
                format!("{} of the print set's files could not be read", unreadable.len())
            },
            &format!("{}{}", named.join("; "), if more > 0 { format!("; and {more} more") } else { String::new() }),
            if none_read { 5 } else { 4 },
            vec!["Check that each file is a PDF that opens".into(), "Replace the damaged files and start the job again".into()],
            None,
            none_read,
        );
    }
    if sheet_count == 0 && unreadable.is_empty() {
        job.add_finding(
            StageName::Classify,
            "Classify found 0 sheets in the print set",
            "A classify stage that yields 0 sheets cannot proceed",
            5,
            vec!["Check that the PDF folder is correct".into(), "Re-import with a different path".into()],
            None,
            true,
        );
    }

    // MUST 2: Check each sheet's discipline; ask when unknown.
    if let Some(ref sheets) = sheets_vec {
        let mut unknown_count = 0usize;
        for sheet in sheets.iter().take(MAX_QUESTIONS_PER_STAGE) {
            let number = sheet.get("number").and_then(Value::as_str).unwrap_or("?");
            let title = sheet.get("title").and_then(Value::as_str).unwrap_or("");
            let discipline = sheet.get("discipline").and_then(Value::as_str).unwrap_or("unknown");

            if discipline == "unknown" {
                unknown_count += 1;
                // Try to infer from title text.
                let inferred = infer_discipline_from_title(title);
                if let Some(disc) = inferred {
                    // Low confidence — ask to confirm.
                    let qid = job.next_qid(StageName::Classify);
                    let q = Question {
                        id: qid.clone(),
                        stage: StageName::Classify,
                        question: format!("Sheet {number} titled \"{title}\" — discipline looks like {disc} from the title. Confirm?",),
                        why: format!("Sheet number has no recognized prefix; title suggests {disc}"),
                        choices: vec![
                            format!("Yes, {disc}"),
                            "A (Architectural)".into(),
                            "E (Electrical)".into(),
                            "M (Mechanical)".into(),
                            "P (Plumbing)".into(),
                            "Other".into(),
                        ],
                        evidence: Some(QuestionEvidence {
                            crop_png_path: None,
                            sheet: Some(number.to_string()),
                            at: None,
                            values: Some(json!({"number": number, "title": title, "inferred": disc})),
                            handles: Vec::new(),
                            tiled_crops: None,
                            leftovers: Vec::new(),
                        }),
                        default: Some(format!("Yes, {disc}")),
                        answer_schema: Some(json!({"type": "string"})),
                        answer: None,
                        answered_by: None,
                        answered_at: None,
                        kind: QuestionKind::Doubt,
                    };
                    job.add_question_to_worklist(&q);
                    if let Some(st) = job.stage_mut(StageName::Classify) {
                        st.questions.push(q);
                    }
                } else {
                    // Cannot infer at all — ask.
                    let qid = job.next_qid(StageName::Classify);
                    let q = Question {
                        id: qid.clone(),
                        stage: StageName::Classify,
                        question: format!("Sheet {number} titled \"{title}\" — what discipline is this sheet?",),
                        why: "Cannot determine discipline from sheet number or title".into(),
                        choices: vec![
                            "A (Architectural)".into(),
                            "E (Electrical)".into(),
                            "M (Mechanical)".into(),
                            "P (Plumbing)".into(),
                            "S (Structural)".into(),
                            "Other".into(),
                        ],
                        evidence: Some(QuestionEvidence {
                            crop_png_path: None,
                            sheet: Some(number.to_string()),
                            at: None,
                            values: Some(json!({"number": number, "title": title})),
                            handles: Vec::new(),
                            tiled_crops: None,
                            leftovers: Vec::new(),
                        }),
                        default: Some("Other".into()),
                        answer_schema: Some(json!({"type": "string"})),
                        answer: None,
                        answered_by: None,
                        answered_at: None,
                        kind: QuestionKind::Doubt,
                    };
                    job.add_question_to_worklist(&q);
                    if let Some(st) = job.stage_mut(StageName::Classify) {
                        st.questions.push(q);
                    }
                }
            }
        }
        if unknown_count > 0 {
            job.add_finding(
                StageName::Classify,
                &format!("{unknown_count} sheet(s) have unknown discipline"),
                "Sheets without a recognized discipline prefix or title keyword need manual classification",
                3,
                Vec::new(),
                None,
                false,
            );
        }
        // A plan whose title names no level: in a one-level building it was given that level,
        // to confirm; with several levels it has none (it is read as nothing) until the answer
        // says which, so auto mode never puts it on a level.
        let level_codes: Vec<String> = classified
            .get("levels")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|l| l.get("name").and_then(Value::as_str)).map(str::to_string).collect())
            .unwrap_or_default();
        let uncertain = |sh: &&Value| matches!(sh.get("level_source").and_then(Value::as_str), Some("default" | "ask"));
        for sheet in sheets.iter().filter(uncertain).take(MAX_QUESTIONS_PER_STAGE) {
            let q = plan_level_question(job, sheet, &level_codes);
            job.add_question_to_worklist(&q);
            if let Some(st) = job.stage_mut(StageName::Classify) {
                st.questions.push(q);
            }
        }
    }

    // MUST 3 (typicalfloors): a level with an architectural plan but no plan for a trade
    // gets a finding.
    if let Some(ref sheets) = sheets_vec {
        check_missing_trade_plans(sheets, job);
    }

    job.audit("stage_run", "pipeline", None, Some(StageName::Classify), Some(json!({"sheets": sheet_count})));
    Ok(classified)
}

/// The question for a plan whose title names no level (`level_source` `default` or `ask`): with
/// a level given (a one-level building), is it that one; with none, which of the set's levels
/// (`level_codes`) it shows, "Not a level plan" by default (auto mode never puts it on a level).
fn plan_level_question(job: &mut GuidedJob, sheet: &Value, level_codes: &[String]) -> Question {
    let number = sheet.get("number").and_then(Value::as_str).unwrap_or("?");
    let title = sheet.get("title").and_then(Value::as_str).unwrap_or("");
    let level = sheet.get("level").and_then(Value::as_str).unwrap_or("");
    let (question, choices, default) = if level.is_empty() {
        let mut choices: Vec<String> = level_codes.iter().take(20).map(|c| level_name(c)).collect();
        choices.push("Not a level plan".into());
        (format!("Plan sheet {number} \"{title}\" names no level — which level does it show?"), choices, "Not a level plan".to_string())
    } else {
        (
            format!("Plan sheet {number} \"{title}\" names no level — is it {}?", level_name(level)),
            vec![level_name(level), "Another level".into(), "Not a level plan".into()],
            level_name(level),
        )
    };
    Question {
        id: job.next_qid(StageName::Classify),
        stage: StageName::Classify,
        question,
        why: "Neither the sheet title, its view titles nor its number say which level the plan shows".into(),
        choices,
        evidence: Some(QuestionEvidence {
            crop_png_path: None,
            sheet: Some(number.to_string()),
            at: None,
            values: Some(json!({"number": number, "title": title, "level": level, "levels": level_codes, "sources": sheet.get("sources")})),
            handles: Vec::new(),
            tiled_crops: None,
            leftovers: Vec::new(),
        }),
        default: Some(default),
        answer_schema: Some(json!({"type": "string"})),
        answer: None,
        answered_by: None,
        answered_at: None,
        kind: QuestionKind::Doubt,
    }
}

/// Check for levels that have an architectural plan but no MEP plan for a discipline.
fn check_missing_trade_plans(sheets: &[Value], job: &mut GuidedJob) {
    let is_plan = |sh: &Value| {
        let t = sh.get("type").or_else(|| sh.get("sheet_type")).and_then(Value::as_str).unwrap_or("");
        matches!(t, "plan" | "enlarged_plan")
    };
    let disc_of = |sh: &Value| sh.get("discipline").and_then(Value::as_str).unwrap_or("").to_ascii_uppercase();
    let levels_of = |sh: &Value| -> Vec<String> {
        let mut out: Vec<String> =
            sh.get("levels").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        if out.is_empty()
            && let Some(l) = sh.get("level").and_then(Value::as_str).filter(|l| !l.is_empty())
        {
            out.push(l.to_string());
        }
        out
    };
    // Collect levels that have architectural plans.
    let mut arch_levels: Vec<String> = Vec::new();
    for sh in sheets.iter().filter(|s| is_plan(s) && disc_of(s) == "A") {
        for l in levels_of(sh) {
            if l.parse::<u32>().is_ok() && !arch_levels.contains(&l) {
                arch_levels.push(l.clone());
            }
        }
    }
    arch_levels.sort_by_key(|l| l.parse::<u32>().unwrap_or(u32::MAX));
    if arch_levels.len() < 2 {
        return; // Only meaningful when there are multiple levels.
    }
    // For each trade, check which levels have coverage.
    for disc in ["E", "M", "P"] {
        let mut covered: Vec<String> = Vec::new();
        for sh in sheets.iter().filter(|s| is_plan(s) && disc_of(s) == disc) {
            for l in levels_of(sh) {
                if !covered.contains(&l) {
                    covered.push(l.clone());
                }
            }
        }
        let missing: Vec<&str> = arch_levels.iter().filter(|l| !covered.contains(l)).map(String::as_str).collect();
        if !missing.is_empty() && !covered.is_empty() {
            let trade = match disc {
                "E" => "electrical",
                "M" => "mechanical",
                "P" => "plumbing",
                _ => disc,
            };
            for l in &missing {
                job.add_finding(
                    StageName::Classify,
                    &format!("Level {l} has no {trade} ({disc}) plan"),
                    &format!("Architectural plans exist for levels {arch_levels:?} but no {disc} plan covers level {l}; devices and MEP on this level will be empty unless a typical-floor sheet is added"),
                    2,
                    vec![format!("Check if a {disc} sheet covers this level (TYPICAL or multi-level title)")],
                    None,
                    false,
                );
            }
        }
    }
}

/// Infer discipline from title text (MUST 2 fallback).
pub fn infer_discipline_from_title(title: &str) -> Option<&'static str> {
    let upper = title.to_uppercase();
    // Check for discipline keywords in title or keynote headers.
    let patterns: &[(&[&str], &str)] = &[
        (&["ELECTRICAL", "ELEC PLAN", "ELEC. PLAN", "ELECTRICAL KEYNOTES", "POWER PLAN", "LIGHTING PLAN"], "E"),
        (&["MECHANICAL", "MECH PLAN", "MECH. PLAN", "HVAC", "MECHANICAL KEYNOTES"], "M"),
        (&["PLUMBING", "PLUMB PLAN", "PLUMB. PLAN", "PLUMBING KEYNOTES"], "P"),
        (&["ARCHITECTURAL", "ARCH PLAN", "FLOOR PLAN", "REFLECTED CEILING"], "A"),
        (&["STRUCTURAL", "STRUCT PLAN", "FOUNDATION"], "S"),
        (&["FIRE PROTECTION", "FIRE ALARM", "SPRINKLER"], "FP"),
        (&["CIVIL", "SITE PLAN", "GRADING", "PAVING"], "G"),
        (&["TELECOM", "TECHNOLOGY", "LOW VOLTAGE"], "T"),
    ];
    for (keywords, disc) in patterns {
        if keywords.iter().any(|kw| upper.contains(kw)) {
            return Some(disc);
        }
    }
    None
}

/// Run the levels stage: detect levels, scales, elevations (MUST 1, 3).
/// Levels from: (1) levels.read, (2) plan titles ("ELECTRICAL - 3RD FLOOR").
/// Multiple plans on one sheet → split into levels. Ask floor-to-floor if no level notes.
fn run_levels(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    let levels_result = s.execute("levels.read", &json!({"apply": true}))?;
    let levels = levels_result.get("levels").and_then(Value::as_array);
    let level_count = levels.map_or(0, Vec::len);

    // The sheet index's levels come first: the levels the plan titles name, with elevations
    // where the sections and elevations mark them. levels.read's notes (which may include a
    // site sheet's spot elevations) are used only when the index has none.
    let mut title_levels: Vec<Value> = Vec::new();
    if let Some(index_levels) = job.index.as_ref().and_then(|i| i.get("levels")).and_then(Value::as_array) {
        for l in index_levels.iter().take(MAX_QUESTIONS_PER_STAGE) {
            let Some(name) = l.get("name").and_then(Value::as_str) else { continue };
            // Q6018: sheets may be strings or {sheet, typical} objects.
            let raw_sheets = l.get("sheets").and_then(Value::as_array);
            let sheet_name = |v: &Value| v.as_str().map(str::to_string).or_else(|| v.get("sheet").and_then(Value::as_str).map(str::to_string));
            let first_sheet = raw_sheets.and_then(|a| a.first()).and_then(sheet_name).unwrap_or_else(|| "?".into());
            // Pass through the sheets as-is (objects with typical flags when present).
            let sheets_val = l.get("sheets").cloned().unwrap_or(json!([]));
            title_levels.push(json!({
                "name": name,
                "sheet": first_sheet,
                "sheets": sheets_val,
                // Marked on the sections: no title guess to confirm.
                "source": if l.get("elevation_ft").and_then(Value::as_f64).is_some() { "sheet index" } else { "title" },
                "elevation_ft": l.get("elevation_ft"),
                "height_ft": l.get("height_ft"),
            }));
        }
    }
    if level_count == 0
        && title_levels.is_empty()
        && let Some(sheets) = &job.sheets
        && let Some(sheet_arr) = sheets.get("sheets").and_then(Value::as_array)
    {
        for sheet in sheet_arr {
            let number = sheet.get("number").and_then(Value::as_str).unwrap_or("?");
            let title = sheet.get("title").and_then(Value::as_str).unwrap_or("");
            let extracted = extract_levels_from_title(title);
            for lvl in extracted {
                title_levels.push(json!({
                    "name": lvl,
                    "sheet": number,
                    "source": "title",
                    "elevation_ft": null,
                }));
            }
        }
    }

    // Deduplicate title-extracted levels by name before consuming.
    {
        let mut seen = std::collections::HashSet::new();
        title_levels.retain(|v| {
            let name = v.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            seen.insert(name)
        });
    }
    let title_level_count = title_levels.len();
    let from_index = !title_levels.is_empty();
    let mut effective_levels = match levels.filter(|l| !l.is_empty()) {
        Some(lvls) if title_levels.is_empty() => lvls.clone(),
        _ => title_levels,
    };
    // None read: the levels an answer named, a storey apart, into the spec.
    if effective_levels.is_empty() && !job.levels_given.is_empty() {
        effective_levels = levels_from_answer(s, job)?;
    }
    let effective_count = effective_levels.len();

    // Readings levels.read refused (they break the levels' order) or set aside (site sheets).
    let index_warnings = job.index.as_ref().and_then(|i| i.get("level_warnings")).and_then(Value::as_array);
    let refused: Vec<String> = ["warnings", "ignored"]
        .iter()
        .filter_map(|k| levels_result.get(*k).and_then(Value::as_array))
        .chain(index_warnings)
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .take(MAX_QUESTIONS_PER_STAGE)
        .collect();
    if !refused.is_empty() {
        job.add_finding(
            StageName::Levels,
            &format!("{} level reading(s) not used: {}", refused.len(), refused.join("; ")),
            "Level elevations rise with the level number, and a civil or site sheet's elevations are spot elevations on the site datum",
            2,
            vec!["Check the sections' level marks".into(), "Set a level's elevation with levels.set".into()],
            None,
            false,
        );
    }

    // MUST 1: No empty success.
    if effective_count == 0 {
        job.add_finding(
            StageName::Levels,
            "No levels found from scale notes or plan titles",
            "The levels stage exists to find building levels; 0 means the pipeline cannot assign rooms or devices to floors",
            4,
            vec!["Provide level names manually".into(), "Check that the print set contains plan sheets".into()],
            None,
            false,
        );
        // Ask for levels.
        let qid = job.next_qid(StageName::Levels);
        let q = Question {
            id: qid,
            stage: StageName::Levels,
            question: "No levels detected. What are the building levels? (e.g. \"1, 2, 3, R\")".into(),
            why: "No level notes found in the plans and no floor names in titles".into(),
            choices: Vec::new(),
            evidence: None,
            default: Some("1".into()),
            answer_schema: Some(json!({"type": "string"})),
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: QuestionKind::Doubt,
        };
        job.add_question_to_worklist(&q);
        if let Some(st) = job.stage_mut(StageName::Levels) {
            st.questions.push(q);
        }
    }

    // Process each detected level.
    for (i, level) in effective_levels.iter().take(MAX_QUESTIONS_PER_STAGE).enumerate() {
        let name = level.get("name").and_then(Value::as_str).unwrap_or("?");
        let elevation = level.get("elevation_ft").and_then(Value::as_f64);
        let source = level.get("source").and_then(Value::as_str).unwrap_or("detected");

        // If from title extraction, ask to confirm.
        if source == "title" {
            let sheet = level.get("sheet").and_then(Value::as_str).unwrap_or("?");
            let qid = job.next_qid(StageName::Levels);
            let q = Question {
                id: qid,
                stage: StageName::Levels,
                question: format!("Sheet {sheet} title suggests level \"{name}\" — confirm and provide elevation?",),
                why: "Level extracted from plan title; no scale or level note found".into(),
                choices: vec![format!("Yes, \"{name}\""), "Different name".into(), "Not a level".into()],
                evidence: Some(QuestionEvidence {
                    crop_png_path: None,
                    sheet: Some(sheet.to_string()),
                    at: None,
                    values: Some(json!({"level": name, "source": "title"})),
                    handles: Vec::new(),
                    tiled_crops: None,
                    leftovers: Vec::new(),
                }),
                default: Some(format!("Yes, \"{name}\"")),
                answer_schema: None,
                answer: None,
                answered_by: None,
                answered_at: None,
                kind: QuestionKind::Doubt,
            };
            job.add_question_to_worklist(&q);
            if let Some(st) = job.stage_mut(StageName::Levels) {
                st.questions.push(q);
            }
        }

        // Check for low confidence scale matches.
        if let Some(matches) = level.get("matches").and_then(Value::as_array) {
            for m in matches {
                let confidence = m.get("confidence").and_then(Value::as_f64).unwrap_or(1.0);
                if confidence < 0.85 {
                    let scale_text = m.get("scale").and_then(Value::as_str).unwrap_or("unknown");
                    let sheet = m.get("sheet").and_then(Value::as_str).unwrap_or("?");
                    let qid = job.next_qid(StageName::Levels);
                    let q = Question {
                        id: qid,
                        stage: StageName::Levels,
                        question: format!("Sheet {sheet} scale reads {scale_text} — confirm?"),
                        why: format!("Scale note detected with confidence {:.0}%", confidence * 100.0),
                        choices: vec![format!("Yes, {scale_text}"), "No, different scale".into()],
                        evidence: Some(QuestionEvidence {
                            crop_png_path: None,
                            sheet: Some(sheet.to_string()),
                            at: m.get("at").and_then(|a| {
                                let arr = a.as_array()?;
                                Some([arr.first()?.as_f64()?, arr.get(1)?.as_f64()?])
                            }),
                            values: Some(json!({"scale": scale_text, "confidence": confidence})),
                            handles: Vec::new(),
                            tiled_crops: None,
                            leftovers: Vec::new(),
                        }),
                        default: Some(format!("Yes, {scale_text}")),
                        answer_schema: Some(json!({"type": "string"})),
                        answer: None,
                        answered_by: None,
                        answered_at: None,
                        kind: QuestionKind::Doubt,
                    };
                    if let Some(st) = job.stage_mut(StageName::Levels) {
                        st.questions.push(q);
                    }
                }
            }
        }

        // Verify sample: for the first N levels, ask the agent to confirm.
        if i < MAX_VERIFY_SAMPLES && source != "title" {
            let qid = job.next_qid(StageName::Levels);
            let q = Question {
                id: qid,
                stage: StageName::Levels,
                question: format!("Level {name}: elevation {:.1} ft — name and elevation correct?", elevation.unwrap_or(0.0)),
                why: "Verify sample: spot-checking detected levels".into(),
                choices: vec!["Correct".into(), "Wrong name".into(), "Wrong elevation".into()],
                evidence: Some(QuestionEvidence {
                    crop_png_path: None,
                    sheet: None,
                    at: None,
                    values: Some(json!({"level": name, "elevation_ft": elevation})),
                    handles: Vec::new(),
                    tiled_crops: None,
                    leftovers: Vec::new(),
                }),
                default: Some("Correct".into()),
                answer_schema: None,
                answer: None,
                answered_by: None,
                answered_at: None,
                kind: QuestionKind::Verify,
            };
            if let Some(st) = job.stage_mut(StageName::Levels) {
                st.verify.push(q);
            }
        }
    }

    // Level conflicts.
    if let Some(conflicts) = levels_result.get("conflicts").and_then(Value::as_array)
        && conflicts.len() > 1
    {
        let qid = job.next_qid(StageName::Levels);
        let q = Question {
            id: qid,
            stage: StageName::Levels,
            question: format!("{} level conflicts found — review them?", conflicts.len()),
            why: "Multiple levels have conflicting elevations across sheets".into(),
            choices: vec!["Review each".into(), "Accept detected values".into()],
            evidence: Some(QuestionEvidence {
                crop_png_path: None,
                sheet: None,
                at: None,
                values: Some(json!({"conflicts": conflicts})),
                handles: Vec::new(),
                tiled_crops: None,
                leftovers: Vec::new(),
            }),
            default: Some("Accept detected values".into()),
            answer_schema: None,
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: QuestionKind::Doubt,
        };
        if let Some(st) = job.stage_mut(StageName::Levels) {
            st.questions.push(q);
        }
    }

    // MUST 3: Multiple plans on one sheet → split into levels and ask to confirm.
    {
        let mut by_sheet: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for level in &effective_levels {
            if let Some(sheet) = level.get("sheet").and_then(Value::as_str) {
                let name = level.get("name").and_then(Value::as_str).unwrap_or("?");
                by_sheet.entry(sheet.to_string()).or_default().push(name.to_string());
            }
        }
        for (sheet, names) in &by_sheet {
            if names.len() > 1 {
                let qid = job.next_qid(StageName::Levels);
                let q = Question {
                    id: qid,
                    stage: StageName::Levels,
                    question: format!("Sheet {sheet} contains {} plans ({}) — confirm these are separate levels?", names.len(), names.join(", "),),
                    why: "Multiple plan areas detected on one sheet; each needs its own level".into(),
                    choices: vec!["Yes, assign as shown".into(), "No, different assignment".into()],
                    evidence: Some(QuestionEvidence {
                        crop_png_path: None,
                        sheet: Some(sheet.clone()),
                        at: None,
                        values: Some(json!({"plans": names})),
                        handles: Vec::new(),
                        tiled_crops: None,
                        leftovers: Vec::new(),
                    }),
                    default: Some("Yes, assign as shown".into()),
                    answer_schema: None,
                    answer: None,
                    answered_by: None,
                    answered_at: None,
                    kind: QuestionKind::Doubt,
                };
                if let Some(st) = job.stage_mut(StageName::Levels) {
                    st.questions.push(q);
                }
            }
        }
    }

    // MUST 3: Ask floor-to-floor height if no level notes provide elevations.
    let has_elevations = effective_levels.iter().any(|l| l.get("elevation_ft").and_then(Value::as_f64).is_some());
    if effective_count > 0 && !has_elevations {
        let qid = job.next_qid(StageName::Levels);
        let level_names: Vec<&str> = effective_levels.iter().filter_map(|l| l.get("name").and_then(Value::as_str)).collect();
        let q = Question {
            id: qid,
            stage: StageName::Levels,
            question: format!("No level notes with elevations found. What is the floor-to-floor height for levels {}?", level_names.join(", "),),
            why: "Elevations are needed for 3D model placement; no level notes found on sheets".into(),
            choices: vec!["10'-0\" (typical commercial)".into(), "12'-0\"".into(), "9'-0\" (typical residential)".into()],
            evidence: Some(QuestionEvidence {
                crop_png_path: None,
                sheet: None,
                at: None,
                values: Some(json!({"levels": level_names, "has_elevations": false})),
                handles: Vec::new(),
                tiled_crops: None,
                leftovers: Vec::new(),
            }),
            default: Some("10'-0\" (typical commercial)".into()),
            answer_schema: Some(json!({"type": "string"})),
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: QuestionKind::Doubt,
        };
        job.add_question_to_worklist(&q);
        if let Some(st) = job.stage_mut(StageName::Levels) {
            st.questions.push(q);
        }
    }

    job.audit(
        "stage_run",
        "pipeline",
        None,
        Some(StageName::Levels),
        Some(json!({"levels_detected": level_count, "levels_from_titles": title_level_count, "effective": effective_count})),
    );
    let mut out = levels_result;
    if (level_count == 0 || from_index)
        && effective_count > 0
        && let Some(o) = out.as_object_mut()
    {
        // The levels the job goes on with: the sheet index's (the plan titles').
        o.insert("levels_read".into(), json!(level_count));
        o.insert("levels".into(), json!(effective_levels));
    }
    Ok(out)
}

/// The levels an answer names (`1, 2, 3, R`, `Level 1; Level 2; Roof`), bottom up: a number is
/// `Level n`, `R` the roof. None when the answer names none.
fn parse_level_list(text: &str) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for t in text.split([',', ';', '\n']).map(str::trim).filter(|t| !t.is_empty()).take(MAX_GIVEN_LEVELS + 1) {
        let name = if t.chars().all(|c| c.is_ascii_digit()) {
            format!("Level {t}")
        } else if t.eq_ignore_ascii_case("R") || t.eq_ignore_ascii_case("roof") {
            "Roof".to_string()
        } else if t.chars().count() <= 32 && t.chars().all(|c| c.is_alphanumeric() || " -_.".contains(c)) {
            t.to_string()
        } else {
            return None;
        };
        if !out.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            out.push(name);
        }
    }
    (!out.is_empty() && out.len() <= MAX_GIVEN_LEVELS).then_some(out)
}

/// Most levels one answer may name.
const MAX_GIVEN_LEVELS: usize = 60;

/// Make the levels an answer named in the building spec (none there yet), a storey apart (the
/// floor to floor answered, else 10'), and the levels stage's list of them.
fn levels_from_answer(s: &mut Session, job: &GuidedJob) -> Result<Vec<Value>> {
    let (mut sp, _) = super::bspec::current(s, "job levels")?;
    let storey = job.storey_ft.filter(|f| f.is_finite() && *f > 0.0).unwrap_or(10.0);
    let mut out = Vec::new();
    for (i, name) in job.levels_given.iter().enumerate() {
        let elevation = i as f64 * storey;
        if sp.level(name).is_none() {
            let level: cadaddy_walk3d::Level = serde_json::from_value(
                json!({"name": name, "elevation_ft": elevation, "height_ft": storey, "ceiling_ft": (storey - 2.0).max(storey * 0.75)}),
            )
            .map_err(|e| bad("job levels", e.to_string()))?;
            sp.levels.push(level);
        }
        out.push(json!({"name": name, "elevation_ft": elevation, "source": "answer"}));
    }
    sp.levels.sort_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft));
    s.doc_mut()?.building_spec = Some(sp.to_json());
    Ok(out)
}

/// Extract level names from a sheet title (MUST 3).
/// Recognises patterns: "ELECTRICAL - 3RD FLOOR", "4TH FLOOR PLAN", "LEVEL 2", etc.
pub fn extract_levels_from_title(title: &str) -> Vec<String> {
    let upper = title.to_uppercase();
    let mut levels = Vec::new();

    // Pattern: ordinal + FLOOR: "3RD FLOOR", "FOURTH FLOOR"
    let ordinals: &[(&str, &str)] = &[
        ("FIRST", "1"),
        ("1ST", "1"),
        ("SECOND", "2"),
        ("2ND", "2"),
        ("THIRD", "3"),
        ("3RD", "3"),
        ("FOURTH", "4"),
        ("4TH", "4"),
        ("FIFTH", "5"),
        ("5TH", "5"),
        ("SIXTH", "6"),
        ("6TH", "6"),
        ("SEVENTH", "7"),
        ("7TH", "7"),
        ("EIGHTH", "8"),
        ("8TH", "8"),
        ("NINTH", "9"),
        ("9TH", "9"),
        ("TENTH", "10"),
        ("10TH", "10"),
        ("BASEMENT", "B1"),
        ("GROUND", "G"),
        ("MEZZANINE", "M"),
        ("PENTHOUSE", "PH"),
        ("ROOF", "R"),
    ];
    for (word, code) in ordinals {
        if upper.contains(word) && (upper.contains("FLOOR") || upper.contains("LEVEL") || upper.contains("PLAN")) {
            levels.push(code.to_string());
        }
    }

    // Pattern: "LEVEL <N>" or "FLOOR <N>"
    for token in upper.split_whitespace().collect::<Vec<_>>().windows(2) {
        if (token[0] == "LEVEL" || token[0] == "FLOOR") && !token[1].is_empty() {
            let num = token[1].trim_matches(|c: char| !c.is_alphanumeric());
            if !num.is_empty() && num.len() <= 4 && !levels.contains(&num.to_string()) {
                levels.push(num.to_string());
            }
        }
    }

    levels
}

/// Import the print set's plan-sheet PDFs into the session's drawing so that
/// stage commands (`rooms.list`, `bspec.draft`, `bim.build`, …) find entities.
///
/// `printset.open` only creates metadata; this bridges to actual drawing content.
/// Each current plan-type sheet (plan / enlarged_plan, its own page of its file) is imported
/// once, side by side (never on top of each other): a level's sheets together, lowest level
/// first, each in its own rectangle (`job.sheet_regions`), so what is read on a sheet carries the
/// sheet's level. A page showing the same sheet as one already imported (the full set and a
/// discipline set) is not imported again. The pages are converted on every core, each file read
/// once, small fills merged ([`super::exchange::import_pdf_pages`]).
fn ensure_sheets_loaded(s: &mut Session, job: &mut GuidedJob) -> Result<()> {
    if job.sheets_loaded {
        return Ok(());
    }
    let sheets_val = match &job.sheets {
        Some(v) => v.clone(),
        None => return Err(bad("guided", "no sheets cached — classify must run first")),
    };
    let sheet_arr = sheets_val.get("sheets").and_then(Value::as_array).cloned().unwrap_or_default();
    let current = |sh: &&Value| sh.get("superseded_by").and_then(Value::as_str).is_none_or(str::is_empty);
    let kind = |sh: &Value| sh.get("type").or_else(|| sh.get("sheet_type")).and_then(Value::as_str).unwrap_or("").to_string();
    // The plans, and the reflected ceiling plans (a sheet of enlarged RCPs and details too): the
    // ceilings are set per area from them before the MEP goes in.
    let rcp_title = |sh: &Value| {
        let t = sh.get("title").and_then(Value::as_str).unwrap_or("").to_ascii_uppercase();
        t.contains("REFLECTED CEILING") || t.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "RCP")
    };
    let legend_title = |sh: &Value| wall_legend_title(sh.get("title").and_then(Value::as_str).unwrap_or(""));
    // And the partition (wall type) legend: how each wall the plans tag is built.
    let mut plan_sheets: Vec<&Value> = sheet_arr
        .iter()
        .filter(current)
        .filter(|sh| matches!(kind(sh).as_str(), "plan" | "enlarged_plan" | "rcp") || rcp_title(sh) || legend_title(sh))
        .collect();
    // The index's level (a plan whose title names none was given one) over the page's own.
    let level_of = |sh: &Value| -> String {
        let number = sh.get("number").and_then(Value::as_str).unwrap_or("");
        let base = number.split('/').next().unwrap_or(number);
        let own = sh.get("level").and_then(Value::as_str).unwrap_or("");
        if !own.is_empty() && number.contains('/') {
            return own.to_string();
        }
        job.index
            .as_ref()
            .and_then(|i| i.get("sheets"))
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|x| x.get("number").and_then(Value::as_str) == Some(base)))
            .and_then(|x| x.get("level"))
            .and_then(Value::as_str)
            .filter(|l| !l.is_empty())
            .unwrap_or(own)
            .to_string()
    };
    // The sheet's entry in the index (its title and where its level came from).
    let index_entry = |base: &str| -> Option<&Value> {
        job.index
            .as_ref()
            .and_then(|i| i.get("sheets"))
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|x| x.get("number").and_then(Value::as_str) == Some(base)))
    };
    let level_rank = |l: &str| (l.is_empty(), l == "R", l.parse::<u32>().unwrap_or(u32::MAX), l.to_string());
    plan_sheets.sort_by_key(|sh| level_rank(&level_of(sh)));
    let mut all: Vec<&Value> = if plan_sheets.is_empty() { sheet_arr.iter().filter(current).collect() } else { plan_sheets };
    // A several-view sheet (3RD and 4TH FLOOR PLANS) is one entry per view but one page:
    // import each page once, with its lowest level.
    let page_of = |sh: &Value| sh.get("page").and_then(Value::as_u64).filter(|p| *p >= 1).unwrap_or(1);
    let mut seen = std::collections::HashSet::new();
    all.retain(|sh| match sh.get("file").and_then(Value::as_str) {
        Some(f) if !f.is_empty() => seen.insert((f.to_string(), page_of(sh))),
        _ => false,
    });
    let pages: Vec<super::exchange::PdfPage> = all
        .iter()
        .map(|sh| {
            let file = sh.get("file").and_then(Value::as_str).unwrap_or_default();
            // The set's files are named from its folder (a one-PDF set: the PDF's folder).
            let dir = std::path::Path::new(&job.pdfs_path);
            let dir =
                if dir.is_file() { dir.parent().map_or_else(|| ".".to_string(), |p| p.to_string_lossy().to_string()) } else { job.pdfs_path.clone() };
            let path = if std::path::Path::new(file).is_absolute() { file.to_string() } else { format!("{dir}/{file}") };
            // Images (logos, scans) aren't used by the job, and must not land in the user's print
            // folder; pattern-sized fills are merged (they are most of a print set's objects).
            let mut params = json!({"page": page_of(sh), "images": false, "fills": "merge"});
            if let Some(k) = sh.get("scale").and_then(Value::as_f64).filter(|k| k.is_finite() && *k > 1.0) {
                params["scale"] = json!(k);
            }
            super::exchange::PdfPage { path, params, at: None }
        })
        .collect();
    let names: Vec<String> = all
        .iter()
        .map(|sh| {
            let number = sh.get("number").and_then(Value::as_str).unwrap_or("");
            if number.is_empty() { format!("{}#{}", sh.get("file").and_then(Value::as_str).unwrap_or("?"), page_of(sh)) } else { number.to_string() }
        })
        .collect();
    let stage = job.current_stage().map_or(StageName::Rooms, |st| st.name);
    let tag = progress_tag(job, stage);
    let total = pages.len();
    // The objects each sheet's import made (`lo..hi`): each sheet's legend applies to its own
    // symbols only (pages are added one after another, so their handles don't interleave).
    let mut spans: Vec<SheetSpan> = Vec::new();
    let mut last_seed = s.doc().map_or(0, |d| d.handseed);
    let mut on_page = |s: &mut Session, i: usize, r: &super::exchange::PageImport| {
        let name = names.get(i).map_or("?", String::as_str);
        let seed = s.doc().map_or(0, |d| d.handseed);
        if matches!(r, super::exchange::PageImport::Done(_)) && seed > last_seed {
            spans.push(SheetSpan { sheet: name.to_string(), lo: last_seed, hi: seed });
        }
        last_seed = seed;
        let what = match r {
            super::exchange::PageImport::Done(rep) => format!("imported ({} objects)", rep.get("entities").and_then(Value::as_u64).unwrap_or(0)),
            super::exchange::PageImport::Same(j) => format!("same as {}, not imported again", names.get(*j).map_or("?", String::as_str)),
            super::exchange::PageImport::Failed(e) => format!("could not import: {e}"),
        };
        report_progress(
            s,
            json!({"stage": stage.label(), "step": "sheets", "done": i + 1, "total": total, "sheet": name}),
            &format!("{tag}: sheet {}/{total} {name} {what}", i + 1),
        );
    };
    // A cancel stops the import at its next page: the drawing goes back to before it, so the
    // pages are imported once when the job goes on.
    let before = s.doc()?.clone();
    let results = match super::exchange::import_pdf_pages(s, &pages, true, true, &mut on_page) {
        Err(e) if cancel_requested() => {
            *s.doc_mut()? = before;
            return Err(e);
        }
        r => r?,
    };
    drop(before);
    let mut imported = 0usize;
    let mut same = 0usize;
    let mut placed = Vec::new();
    let mut regions = Vec::new();
    let mut typical = Vec::new();
    for ((r, name), sh) in results.iter().zip(&names).zip(&all) {
        match r {
            super::exchange::PageImport::Done(rep) => {
                imported += 1;
                let rect = rep.get("rect").and_then(Value::as_array).and_then(|r| {
                    let v: Vec<f64> = r.iter().filter_map(Value::as_f64).collect();
                    <[f64; 4]>::try_from(v).ok()
                });
                if let Some(rect) = rect {
                    let number = sh.get("number").and_then(Value::as_str).unwrap_or("");
                    let discipline = sh
                        .get("discipline")
                        .and_then(Value::as_str)
                        .filter(|d| !d.is_empty())
                        .map_or_else(|| number.chars().take_while(char::is_ascii_alphabetic).take(2).collect(), str::to_string);
                    let base = number.split('/').next().unwrap_or(number);
                    let level = level_of(sh);
                    let field =
                        |k: &str| index_entry(base).and_then(|x| x.get(k)).or_else(|| sh.get(k)).and_then(Value::as_str).unwrap_or("").to_string();
                    let not_built = not_built_reason(&discipline, &field("title"), &level, &field("level_source"));
                    let levels: Vec<String> = sh
                        .get("levels")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                        .unwrap_or_default();
                    typical.push(cadaddy_mepfix::sheet_reader::says_typical(&field("title")));
                    regions.push(SheetRegion {
                        number: base.to_string(),
                        level,
                        discipline: discipline.to_ascii_uppercase(),
                        // A ceiling plan (by its kind or its title) is read for its ceiling heights only.
                        kind: if rcp_title(sh) {
                            "rcp".to_string()
                        } else if legend_title(sh) {
                            "legend".to_string()
                        } else {
                            kind(sh)
                        },
                        rect,
                        area: None,
                        title_block: None,
                        not_built: not_built.to_string(),
                        levels,
                        view: None,
                    });
                }
                if placed.len() < MAX_QUESTIONS_PER_STAGE {
                    placed.push(json!({"sheet": name, "rect": rep.get("rect")}));
                }
            }
            super::exchange::PageImport::Same(_) => same += 1,
            // Non-fatal: log the error but continue with other sheets.
            super::exchange::PageImport::Failed(e) => s.echo(format!("guided: could not import sheet {name}: {e}")),
        }
    }
    job.sheet_spans.extend(spans);
    // Each sheet's border and title block: what is read there is never building.
    let rects: Vec<[f64; 4]> = regions.iter().map(|r| r.rect).collect();
    for (r, f) in regions.iter_mut().zip(sheet_frames(s.doc()?, &rects)) {
        r.area = Some(f.area);
        r.title_block = f.title_block;
    }
    // A sheet printing two levels' plans: each level takes only its own view.
    let regions = split_view_regions(s.doc()?, regions, &typical);
    job.sheet_regions = regions;
    job.sheets_loaded = true;
    job.audit(
        "sheets_loaded",
        "pipeline",
        None,
        None,
        Some(json!({
            "plan_sheets": total,
            "pages": total,
            "imported": imported,
            "same_pages": same,
            "total_sheets": sheet_arr.len(),
            "sheets": placed,
        })),
    );
    Ok(())
}

// ─── Progress and checkpoints ───────────────────────────────────────────────

/// Where a running job's progress lines go besides the `job.progress` events (the CLI prints
/// them on stderr while the job runs).
static PROGRESS_SINK: std::sync::RwLock<Option<fn(&str)>> = std::sync::RwLock::new(None);

/// A job is running (job.start, job.next, job.answer…): a cancel asked for now stops it.
static JOB_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// A cancel the running job takes at its next sheet or stage.
static JOB_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Ask the running job (if any) to stop at its next sheet or stage. Safe to call from any
/// thread (the MCP server's input thread calls it). Whether a job was running to take it.
pub fn request_cancel() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    let running = JOB_RUNNING.load(Relaxed);
    if running {
        JOB_CANCEL.store(true, Relaxed);
    }
    running
}

/// Whether the running job has been asked to stop.
pub(crate) fn cancel_requested() -> bool {
    JOB_CANCEL.load(std::sync::atomic::Ordering::Relaxed)
}

/// The flag a job's long steps (classify's sheets) stop on.
pub(crate) fn cancel_flag() -> &'static std::sync::atomic::AtomicBool {
    &JOB_CANCEL
}

/// While it lives, the job counts as running (a cancel goes to it); a cancel left from before
/// is dropped.
struct Running;

impl Running {
    fn start() -> Self {
        use std::sync::atomic::Ordering::Relaxed;
        JOB_CANCEL.store(false, Relaxed);
        JOB_RUNNING.store(true, Relaxed);
        Running
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        JOB_RUNNING.store(false, Relaxed);
        JOB_CANCEL.store(false, Relaxed);
    }
}

/// Send a running job's progress lines (one per stage and per sheet imported) to `sink`.
pub fn set_progress_sink(sink: Option<fn(&str)>) {
    if let Ok(mut g) = PROGRESS_SINK.write() {
        *g = sink;
    }
}

/// `job 3/8 rooms`: the stage and where it is in the job's pipeline.
fn progress_tag(job: &GuidedJob, stage: StageName) -> String {
    let active: Vec<StageName> = job.stages.iter().filter(|st| st.status != StageStatus::Skipped).map(|st| st.name).collect();
    let at = active.iter().position(|n| *n == stage).map_or(0, |i| i + 1);
    format!("job {at}/{} {}", active.len(), stage.label())
}

/// A line to the progress sink only (a step inside a stage, e.g. the model build's).
pub(crate) fn progress_line(line: &str) {
    if let Ok(g) = PROGRESS_SINK.read()
        && let Some(sink) = *g
    {
        sink(line);
    }
}

/// A `job.progress` event, and the line to the progress sink.
fn report_progress(s: &mut Session, data: Value, line: &str) {
    s.event("job.progress", data);
    if let Ok(g) = PROGRESS_SINK.read()
        && let Some(sink) = *g
    {
        sink(line);
    }
}

/// The default checkpoint of a job: the job's drawing (with the job's state) after each stage,
/// in the temporary folder, so a job cut off by a crash picks up where it was (`job.start` with
/// the same prints and goal). Never in the print folder.
fn default_checkpoint(job_id: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(std::env::temp_dir().join("cadaddy-jobs").join(format!("{job_id}.cdy")).to_string_lossy().to_string())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = job_id;
        None
    }
}

/// What the print folder holds (names, sizes, change times): a checkpoint of other prints isn't
/// resumed.
fn prints_stamp(pdfs: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if let Ok(files) = cadaddy_mepfix::printset::discover_sheets(std::path::Path::new(pdfs)) {
        for (name, path) in files {
            name.hash(&mut h);
            if let Ok(m) = std::fs::metadata(&path) {
                m.len().hash(&mut h);
                if let Ok(t) = m.modified()
                    && let Ok(d) = t.duration_since(std::time::UNIX_EPOCH)
                {
                    d.as_secs().hash(&mut h);
                }
            }
        }
    }
    format!("{:016x}", h.finish())
}

/// Save the job's drawing (its state inside) to the job's checkpoint; a failure is a note, not
/// a stop.
fn write_checkpoint(s: &mut Session, job: &GuidedJob) {
    let Some(path) = job.checkpoint.clone() else { return };
    let Some(hooks) = super::file::io() else { return };
    let started = web_time::Instant::now();
    if save_job(s, job).is_err() {
        return;
    }
    let Ok(d) = s.doc() else { return };
    // A .cdy file without its preview (`ckpt`), else as the path says.
    let written = (hooks.write)(d, "checkpoint.ckpt").or_else(|_| (hooks.write)(d, &path)).and_then(|bytes| {
        let p = std::path::Path::new(&path);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        // Written aside, then moved over the last one: a crash mid-write keeps the last checkpoint.
        let part = p.with_extension("cdy.part");
        crate::writefile::write(&part, bytes).map_err(|e| e.to_string())?;
        crate::writefile::rename(&part, p).map_err(|e| e.to_string())
    });
    // How long it took, and the peak memory after it (a checkpoint per stage must not be the
    // job's peak: compare with the stage's line).
    let peak = super::memstat::process_mb().map_or_else(String::new, |(_, p)| format!(" (peak memory {p} MB)"));
    progress_line(&format!("  checkpoint written in {:.1} s{peak}", started.elapsed().as_secs_f64()));
    if let Err(e) = written {
        s.echo(format!("guided: the job's checkpoint couldn't be written to {path}: {e}"));
    }
}

/// Whether the file at `path` is a job's checkpoint (a drawing holding a job's state): one the
/// caller names may be written over; any other file there is the caller's and is never touched.
fn is_job_checkpoint(path: &str) -> bool {
    let Some(hooks) = super::file::io() else { return false };
    let Ok(bytes) = std::fs::read(path) else { return false };
    let Ok(d) = (hooks.read)(&bytes, path) else { return false };
    d.agent.as_deref().and_then(|a| serde_json::from_str::<Value>(a).ok()).is_some_and(|a| a.get("guided_job").is_some())
}

/// The unfinished job a checkpoint holds for these prints and this goal, with its drawing.
fn read_checkpoint(path: &str, pdfs: &str, goal: &JobGoal, stamp: &str) -> Option<(cadaddy_doc::Drawing, GuidedJob)> {
    let hooks = super::file::io()?;
    let bytes = std::fs::read(path).ok()?;
    let d = (hooks.read)(&bytes, path).ok()?;
    let agent: Value = serde_json::from_str(d.agent.as_deref()?).ok()?;
    let job: GuidedJob = serde_json::from_value(agent.get("guided_job")?.clone()).ok()?;
    (job.pdfs_path == pdfs && job.goal == *goal && job.prints_stamp == stamp && !job.is_done()).then_some((d, job))
}

/// An unlisted tag printed by several symbols: a question with a crop.
#[derive(Clone, Debug, Default)]
struct UnlistedTag {
    count: usize,
    /// The first symbol printing it, and its sheet.
    at: [f64; 2],
    sheet: String,
    /// That sheet's handles (`lo..hi`), where the crop looks.
    span: (u64, u64),
    handles: Vec<String>,
}

/// The job's device symbols (pdfnative's repeated clusters, `PDF-SYM-N` inserts with the
/// nearby mark in a TAG attribute), sorted by what their tags say.
#[derive(Clone, Debug, Default)]
struct DeviceCensus {
    /// Devices per type tag.
    by_type: BTreeMap<String, usize>,
    /// Symbols tagged with a circuit or panel reference (`D-1`): wiring labels, not types.
    circuit: usize,
    /// Of those, the ones whose type mark is printed by them (`H` over a downlight): that type.
    retyped: usize,
    /// Luminaires read from their type mark and circuit label alone (no symbol made of them).
    lone_marks: usize,
    /// Symbols with no tag beside them.
    untagged: usize,
    /// Symbols drawn inside a legend or schedule: samples, not devices.
    legend_samples: usize,
    /// Symbols whose "tag" is lettering or a sheet number, by reason.
    not_device: BTreeMap<&'static str, usize>,
    /// Tag-shaped marks no legend lists, by tag.
    unlisted: BTreeMap<String, UnlistedTag>,
    /// Every type the loaded legends and schedules list, and how many tables listed them.
    legend_types: std::collections::BTreeSet<String>,
    legend_tables: usize,
    /// Each device counted in `by_type`, for the model.
    placed: Vec<super::readers::ReadDevice>,
}

/// The disciplines whose sheets draw devices (electrical, mechanical, plumbing, fire protection
/// and alarm, technology).
const DEVICE_DISCIPLINES: &[&str] = &["E", "M", "P", "FP", "FA", "F", "T", "LV", "EL", "ME", "MEP"];
/// An unlisted tag printed by fewer symbols than this is stray lettering, not a question.
const MIN_REPEATED_TAG: usize = 3;
/// Most crops drawn for unlisted tags.
const MAX_TAG_CROPS: usize = 12;
/// Most handles kept per unlisted tag.
const MAX_TAG_HANDLES: usize = 20;

/// Sort the drawing's device symbols sheet by sheet: each sheet's legends and schedules say
/// which symbols are samples; all of them together say which tags are device types.
fn device_census(s: &Session, job: &GuidedJob, bubbles: &[([f64; 2], f64)]) -> DeviceCensus {
    use cadaddy_mepfix::device_types::{self, Legend, TagVerdict};
    use cadaddy_mepfix::sheet_facts::{self, TextItem};
    let mut c = DeviceCensus::default();
    let Ok(d) = s.doc() else { return c };
    let Some(store) = d.space(&s.space()) else { return c };
    // Group objects by the sheet whose import made them; anything else is one more group.
    let mut spans = job.sheet_spans.clone();
    spans.sort_by_key(|sp| sp.lo);
    let group_of = |h: u64| -> usize {
        let i = spans.partition_point(|sp| sp.lo <= h);
        match i.checked_sub(1).and_then(|k| spans.get(k).map(|sp| (k, sp))) {
            Some((k, sp)) if h < sp.hi => k,
            _ => spans.len(),
        }
    };
    let groups = spans.len() + 1;
    let mut texts: Vec<Vec<TextItem>> = vec![Vec::new(); groups];
    // Each symbol: its tag, where it is, its handle, and where its tag is printed (and how tall).
    type Symbol = (String, cadaddy_geom::Vec2, cadaddy_doc::Handle, Option<(cadaddy_geom::Vec2, f64)>);
    let mut symbols: Vec<Vec<Symbol>> = vec![Vec::new(); groups];
    for e in store.iter() {
        let g = group_of(e.handle.0);
        match &e.kind {
            cadaddy_doc::EntityKind::Insert(ins) if ins.block.starts_with("PDF-SYM-") => {
                let attr = ins.attribs.iter().find(|a| a.tag == "TAG");
                let tag = attr.map(|a| a.text.value.trim().to_string()).unwrap_or_default();
                // A tag at the origin is an import that didn't keep where it was printed.
                let tag_at = attr
                    .map(|a| (a.text.insert.xy(), a.text.height))
                    .filter(|(p, h)| p.x.is_finite() && p.y.is_finite() && (p.x != 0.0 || p.y != 0.0) && h.is_finite() && *h > 0.0);
                if let Some(v) = symbols.get_mut(g) {
                    v.push((tag, ins.insert.xy(), e.handle, tag_at));
                }
            }
            cadaddy_doc::EntityKind::Text(t) => {
                if let Some(v) = texts.get_mut(g)
                    && v.len() < MAX_SHEET_TEXTS
                    && t.height.is_finite()
                    && t.height > 0.0
                    && !t.value.trim().is_empty()
                {
                    v.push(TextItem { value: t.value.trim().to_string(), at: t.insert.xy(), height: t.height, rotation: t.rotation });
                }
            }
            _ => {}
        }
    }
    // A circuit, a switch's leg or a receptacle's designation the import took for a symbol's tag
    // ("A-2" by a stair's lines, the "e" by a sensor, the "GFCI" by a receptacle) is still printed
    // there: back among its sheet's texts, where it was printed.
    for (g, syms) in symbols.iter().enumerate() {
        for (tag, _, _, tag_at) in syms {
            if let (Some((at, h)), Some(v)) = (tag_at, texts.get_mut(g))
                && v.len() < MAX_SHEET_TEXTS
                && (sheet_facts::is_circuit_label(tag)
                    || super::readers::switch_designation(tag).is_some()
                    || !receptacle_designations(&[tag.as_str()]).is_empty())
            {
                v.push(TextItem { value: tag.clone(), at: *at, height: *h, rotation: 0.0 });
            }
        }
    }
    // Sheet numbers: the set's, and each loaded sheet's own title block.
    let mut sheet_numbers = std::collections::BTreeSet::new();
    let listed = job.sheets.as_ref().and_then(|v| v.get("sheets")).and_then(Value::as_array);
    for n in listed.into_iter().flatten().filter_map(|sh| sh.get("number").and_then(Value::as_str)) {
        sheet_numbers.extend(device_types::sheet_number_forms(n));
    }
    let mut legends = Vec::with_capacity(groups);
    for t in &texts {
        if let Some(id) = sheet_facts::identify_sheet(t) {
            sheet_numbers.extend(device_types::sheet_number_forms(&id.number));
        }
        let l = device_types::read_legend(t);
        c.legend_tables += l.regions.len();
        c.legend_types.extend(l.types.iter().cloned());
        legends.push(l);
    }
    // Tags an answer made device types.
    let learned: std::collections::BTreeSet<String> = job
        .rules
        .iter()
        .filter(|r| r.value.starts_with("device:"))
        .filter_map(|r| r.key.strip_prefix("symbol:"))
        .map(|t| t.trim().to_string())
        .collect();
    // On the electrical sheets, the types the submittals give products for too (a set whose
    // fixture schedule is only in the lighting submittal lists its types there); another trade's
    // `H` is its own mark.
    let learned_electrical: std::collections::BTreeSet<String> = learned.iter().cloned().chain(job.submittal_types.iter().cloned()).collect();
    // A fire alarm plan's marks are its devices' (an `S` is a smoke detector there, not a switch).
    let fire_alarm: BTreeSet<String> = job
        .index
        .as_ref()
        .or(job.sheets.as_ref())
        .and_then(|i| i.get("sheets"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|sh| sh.get("title").and_then(Value::as_str).is_some_and(|t| t.to_ascii_uppercase().contains("FIRE ALARM")))
        .filter_map(|sh| sh.get("number").and_then(Value::as_str).map(|n| n.trim().to_ascii_uppercase()))
        .collect();
    let electrical = |sheet: &str| {
        let number = sheet.split('/').next().unwrap_or(sheet);
        job.sheet_regions
            .iter()
            .find(|r| r.number.eq_ignore_ascii_case(number))
            .map_or_else(|| number.trim().to_ascii_uppercase().starts_with('E'), |r| r.discipline.eq_ignore_ascii_case("E"))
    };
    // Tags an answer said are not devices.
    let refused: BTreeSet<&str> = job.rules.iter().filter(|r| r.value == "not_device").filter_map(|r| r.key.strip_prefix("tag:")).collect();
    let none = Legend::default();
    // A reflected ceiling plan is loaded for its ceiling heights: its fixtures are the electrical
    // plans' (counted there), and it isn't registered onto the plans.
    let ceiling_plans: BTreeSet<&str> = job.sheet_regions.iter().filter(|r| r.kind == "rcp").map(|r| r.number.as_str()).collect();
    // A trade's devices are on its own sheets: another trade's (architectural, civil, landscape,
    // structural) letters wall types and keys with the same marks, unless the sheet has a legend
    // of devices of its own.
    let device_trade = |sheet: &str| {
        let number = sheet.split('/').next().unwrap_or(sheet);
        job.sheet_regions.iter().find(|r| r.number.eq_ignore_ascii_case(number)).is_none_or(|r| {
            let d = r.discipline.trim().to_ascii_uppercase();
            d.is_empty() || DEVICE_DISCIPLINES.contains(&d.as_str())
        })
    };
    for (g, syms) in symbols.iter().enumerate() {
        if spans.get(g).is_some_and(|sp| ceiling_plans.contains(sp.sheet.split('/').next().unwrap_or(&sp.sheet))) {
            continue;
        }
        let legend = legends.get(g).unwrap_or(&none);
        let sheet_of_g = spans.get(g).map_or_else(String::new, |sp| sp.sheet.clone());
        let fire_alarm_g = fire_alarm.contains(&sheet_of_g.split('/').next().unwrap_or(&sheet_of_g).trim().to_ascii_uppercase());
        if !device_trade(&sheet_of_g) && !fire_alarm_g && legend.regions.is_empty() {
            *c.not_device.entry("on another trade's sheet (no device legend of its own)").or_default() += syms.len();
            continue;
        }
        let circuits: Vec<&TextItem> = texts.get(g).into_iter().flatten().filter(|t| circuit_text(&t.value, &sheet_numbers)).collect();
        let sheet = spans.get(g).map_or_else(String::new, |sp| sp.sheet.clone());
        let span = spans.get(g).map_or((0, u64::MAX), |sp| (sp.lo, sp.hi));
        let learned = if electrical(&sheet) { &learned_electrical } else { &learned };
        let fire_alarm_sheet = fire_alarm.contains(&sheet.split('/').next().unwrap_or(&sheet).trim().to_ascii_uppercase());
        let kind = if fire_alarm_sheet { "fire alarm device".to_string() } else { String::new() };
        // A luminaire by its submittal (on an electrical plan): it hangs at the ceiling.
        let kind_of_tag = |t: &str| {
            let listed = |types: &[String]| types.iter().any(|l| l.eq_ignore_ascii_case(t));
            match (kind.is_empty() && electrical(&sheet), listed(&job.luminaire_types), listed(&job.wall_luminaire_types)) {
                (true, true, _) => "luminaire".to_string(),
                (true, _, true) => "wall luminaire".to_string(),
                _ => kind.clone(),
            }
        };
        // A symbol the import tagged with the circuit label beside it may still have its type
        // mark printed by it (`H` over a downlight whose `G-1,a` the import took): the nearest
        // device mark within reach that no other symbol is nearer.
        let type_mark_by = |at: cadaddy_geom::Vec2, h: cadaddy_doc::Handle, height: f64| -> Option<String> {
            texts
                .get(g)
                .into_iter()
                .flatten()
                .filter(|t| {
                    t.at.dist(at) <= TYPE_MARK_REACH_H * height.max(t.height) && !circuit_text(&t.value, &sheet_numbers) && !is_circuit_tag(&t.value)
                })
                .filter_map(|t| match device_types::tag_verdict(&t.value, &c.legend_types, learned, &sheet_numbers) {
                    TagVerdict::Device(m) => Some((t, m)),
                    _ => None,
                })
                .filter(|(t, _)| !syms.iter().any(|(_, o, oh, _)| *oh != h && o.dist(t.at) < at.dist(t.at)))
                .min_by(|a, b| a.0.at.dist(at).total_cmp(&b.0.at.dist(at)))
                .map(|(_, m)| m)
        };
        // A fire alarm symbol's subscript (a small `R` at an `S`'s lower right: a resident room's
        // smoke detector with sounder base) is part of its mark, not a device of its own: the
        // import may hang the letter on another symbol nearby.
        let (subscript_of, compound) =
            if fire_alarm_sheet { fire_alarm_subscripts(syms, texts.get(g).map_or(&[][..], Vec::as_slice)) } else { Default::default() };
        for (tag, at, h, tag_at) in syms {
            if subscript_of.contains(h) {
                *c.not_device.entry("a fire alarm symbol's subscript (part of its mark)").or_default() += 1;
                continue;
            }
            let tag = compound.get(h).unwrap_or(tag);
            // (On an electrical plan only: another trade's tags read like circuits, `P-3`. And a
            // luminaire's mark only, one the lighting submittals list: a note by a receptacle,
            // `HOR`, is no type.)
            let marked = if electrical(&sheet) && kind.is_empty() && is_circuit_tag(tag) && !legend.covers(*at) && !refused.contains(tag.as_str()) {
                type_mark_by(*at, *h, tag_at.map_or(DEFAULT_MARK_HEIGHT, |(_, ht)| ht))
                    .filter(|m| job.luminaire_types.iter().chain(&job.wall_luminaire_types).any(|l| l.eq_ignore_ascii_case(m)))
            } else {
                None
            };
            if let Some(t) = marked.filter(|_| job.level_at([at.x, at.y]).is_none_or(|r| super::printmep::building_level(&r.level))) {
                c.placed.push(super::readers::ReadDevice {
                    tag: t.clone(),
                    at: [at.x, at.y],
                    sheet: sheet.clone(),
                    level: job.level_at([at.x, at.y]).map_or_else(String::new, |r| r.level.clone()),
                    circuit: tag.clone(),
                    handle: h.hex(),
                    kind: kind_of_tag(&t),
                    ..super::readers::ReadDevice::default()
                });
                *c.by_type.entry(t).or_default() += 1;
                c.retyped += 1;
                continue;
            }
            if legend.covers(*at) {
                c.legend_samples += 1;
            } else if tag.is_empty() {
                c.untagged += 1;
            } else if is_circuit_tag(tag) {
                c.circuit += 1;
                // On an electrical plan a symbol only its circuit labels is a receptacle on that
                // circuit (a GFCI one when GFCI is printed by it): receptacles carry no type mark.
                let building = job.level_at([at.x, at.y]).is_none_or(|r| super::printmep::building_level(&r.level));
                let bubble = bubbles.iter().any(|(c, r)| r.is_finite() && (at.x - c[0]).hypot(at.y - c[1]) <= 1.5 * r.max(0.0));
                if electrical(&sheet) && kind.is_empty() && building && !bubble && circuit_text(tag, &sheet_numbers) {
                    // Its designations printed by it (GFCI, WP, USB…), else a plain one.
                    let near: Vec<&str> = tag_at
                        .map(|(p, h)| {
                            texts.get(g).into_iter().flatten().filter(|t| t.at.dist(p) <= RECEPTACLE_NOTE_H * h).map(|t| t.value.as_str()).collect()
                        })
                        .unwrap_or_default();
                    let marks = receptacle_designations(&near);
                    let mark = if marks.is_empty() { "REC".to_string() } else { marks.join("/") };
                    let mark = mark.as_str();
                    c.placed.push(super::readers::ReadDevice {
                        tag: mark.into(),
                        at: [at.x, at.y],
                        sheet: sheet.clone(),
                        level: job.level_at([at.x, at.y]).map_or_else(String::new, |r| r.level.clone()),
                        circuit: tag.clone(),
                        handle: h.hex(),
                        kind: "receptacle".into(),
                        leg: String::new(),
                        room: String::new(),
                        aff_in: None,
                        aff_note: String::new(),
                        kind_source: String::new(),
                        ifc_class: String::new(),
                    });
                    *c.by_type.entry(mark.to_string()).or_default() += 1;
                }
            } else if refused.contains(tag.as_str()) {
                *c.not_device.entry("not a device (answered)").or_default() += 1;
            } else {
                // A mark with its subscript is the device its base mark says.
                let verdict = if compound.contains_key(h) {
                    TagVerdict::Device(tag.clone())
                } else {
                    device_types::tag_verdict(tag, &c.legend_types, learned, &sheet_numbers)
                };
                match verdict {
                    // A sheet of no level (uncertain, not answered) or on the site datum is a
                    // reference sheet: nothing on it is a model element.
                    TagVerdict::Device(_) if job.level_at([at.x, at.y]).is_some_and(|r| !super::printmep::building_level(&r.level)) => {
                        *c.not_device.entry("on a sheet of no building level").or_default() += 1;
                    }
                    TagVerdict::Device(t) => {
                        c.placed.push(super::readers::ReadDevice {
                            tag: t.clone(),
                            at: [at.x, at.y],
                            sheet: sheet.clone(),
                            level: job.level_at([at.x, at.y]).map_or_else(String::new, |r| r.level.clone()),
                            circuit: super::readers::circuit_of(texts.get(g).map_or(&[][..], Vec::as_slice), &circuits, tag, *tag_at, *at),
                            handle: h.hex(),
                            kind: kind_of_tag(&t),
                            leg: String::new(),
                            room: String::new(),
                            aff_in: None,
                            aff_note: String::new(),
                            kind_source: String::new(),
                            ifc_class: String::new(),
                        });
                        *c.by_type.entry(t).or_default() += 1;
                    }
                    TagVerdict::NotDevice(why) => *c.not_device.entry(why).or_default() += 1,
                    TagVerdict::Unlisted(t) => {
                        let u = c.unlisted.entry(t).or_insert_with(|| UnlistedTag {
                            at: [at.x, at.y],
                            sheet: sheet.clone(),
                            span,
                            ..UnlistedTag::default()
                        });
                        u.count += 1;
                        if u.handles.len() < MAX_TAG_HANDLES {
                            u.handles.push(h.hex());
                        }
                    }
                }
            }
        }
        // A luminaire the import made no symbol of: its type mark (one the lighting submittals
        // list) printed alone with a circuit label just under it (`H` over `G-1,a`), no device
        // read beside it: a luminaire there on that circuit.
        if electrical(&sheet) && kind.is_empty() {
            let luminaire = |v: &str| job.luminaire_types.iter().chain(&job.wall_luminaire_types).any(|l| l.eq_ignore_ascii_case(v));
            let sheet_texts: &[TextItem] = texts.get(g).map_or(&[][..], Vec::as_slice);
            let mut lone: Vec<super::readers::ReadDevice> = Vec::new();
            for t in sheet_texts.iter().filter(|t| luminaire(t.value.trim()) && !legend.covers(t.at)) {
                let reach = LONE_MARK_REACH_H * t.height;
                if job.level_at([t.at.x, t.at.y]).is_some_and(|r| !super::printmep::building_level(&r.level)) {
                    continue;
                }
                // Its circuit: a label under it, within reach.
                let Some(label) = sheet_texts
                    .iter()
                    .filter(|o| circuit_text(&o.value, &sheet_numbers) && o.at.y < t.at.y && o.at.dist(t.at) <= reach)
                    .min_by(|a, b| a.at.dist(t.at).total_cmp(&b.at.dist(t.at)))
                else {
                    continue;
                };
                // Read already: a luminaire of this sheet by the mark (its symbol, beside or under
                // it), or any device where the symbol would be drawn (between the mark and its
                // circuit label). A receptacle beside the mark is another device.
                let spot = cadaddy_geom::Vec2::new((t.at.x + label.at.x) / 2.0, (t.at.y + label.at.y) / 2.0);
                let read = |dv: &super::readers::ReadDevice| {
                    let p = cadaddy_geom::Vec2::new(dv.at[0], dv.at[1]);
                    (dv.kind.contains("luminaire") && p.dist(t.at) <= reach) || p.dist(spot) <= LONE_SYMBOL_H * t.height
                };
                if c.placed.iter().filter(|dv| dv.sheet == sheet).any(read) || lone.iter().any(read) {
                    continue;
                }
                let tag = t.value.trim().to_ascii_uppercase();
                let circuit = label.value.split(',').next().unwrap_or(&label.value).trim().to_string();
                lone.push(super::readers::ReadDevice {
                    tag: tag.clone(),
                    // Between the mark and its circuit label: where the symbol is drawn.
                    at: [(t.at.x + label.at.x) / 2.0, (t.at.y + label.at.y) / 2.0],
                    sheet: sheet.clone(),
                    level: job.level_at([t.at.x, t.at.y]).map_or_else(String::new, |r| r.level.clone()),
                    circuit,
                    kind: kind_of_tag(&tag),
                    ..super::readers::ReadDevice::default()
                });
            }
            for dv in lone.into_iter().take(MAX_LONE_MARKS) {
                *c.by_type.entry(dv.tag.clone()).or_default() += 1;
                c.lone_marks += 1;
                c.placed.push(dv);
            }
        }
        // Switches, by the designation printed by the switch symbol on an electrical sheet
        // (`D,b` a dimmer on leg b, `3,a` a 3-way on leg a, a lone `c`): the switch and the leg
        // of fixtures it controls. A lone leg letter by an occupancy sensor is the sensor's leg.
        if electrical(&sheet) && kind.is_empty() {
            for t in texts.get(g).into_iter().flatten() {
                let Some((tag, leg)) = super::readers::switch_designation(&t.value) else { continue };
                let level = job.level_at([t.at.x, t.at.y]);
                if legend.covers(t.at) || level.is_some_and(|r| !super::printmep::building_level(&r.level)) {
                    continue;
                }
                let reach = SENSOR_LEG_H * t.height;
                let gap = |dv: &super::readers::ReadDevice| cadaddy_geom::Vec2::new(dv.at[0], dv.at[1]).dist(t.at);
                if tag.is_empty() {
                    let sensor = c
                        .placed
                        .iter_mut()
                        .filter(|dv| dv.sheet == sheet && dv.leg.is_empty() && super::readers::device_kind(&dv.tag) == "occupancy sensor")
                        .filter(|dv| gap(dv) <= reach)
                        .min_by(|a, b| gap(a).total_cmp(&gap(b)));
                    if let Some(dv) = sensor {
                        dv.leg = leg;
                        continue;
                    }
                }
                let tag = if tag.is_empty() { "S".to_string() } else { tag };
                c.placed.push(super::readers::ReadDevice {
                    tag: tag.clone(),
                    at: [t.at.x, t.at.y],
                    sheet: sheet.clone(),
                    level: level.map_or_else(String::new, |r| r.level.clone()),
                    leg,
                    ..super::readers::ReadDevice::default()
                });
                *c.by_type.entry(tag).or_default() += 1;
            }
        }
    }
    // Unlisted marks printed once or twice are stray lettering.
    let stray: usize = c.unlisted.values().filter(|u| u.count < MIN_REPEATED_TAG).map(|u| u.count).sum();
    c.unlisted.retain(|_, u| u.count >= MIN_REPEATED_TAG);
    if stray > 0 {
        *c.not_device.entry("unlisted, printed once or twice").or_default() += stray;
    }
    // Replicate devices on typical-floor sheets: a sheet covering levels [2,3,4] places its
    // devices on each of those levels (the primary is already placed; clone for the rest).
    let mut extras: Vec<super::readers::ReadDevice> = Vec::new();
    for dv in &c.placed {
        if let Some(reg) = job.level_at(dv.at) {
            let all = reg.all_levels();
            if all.len() > 1 {
                for extra_level in all.iter().skip(1) {
                    let mut copy = dv.clone();
                    copy.level = extra_level.clone();
                    extras.push(copy);
                }
            }
        }
    }
    if !extras.is_empty() {
        for e in &extras {
            *c.by_type.entry(e.tag.clone()).or_default() += 1;
        }
        c.placed.extend(extras);
    }
    c
}

/// The job's sheet map, read off the sheets it loaded: each sheet's own objects (its handles)
/// as a page, a few pages at a time.
fn job_links(s: &Session, job: &GuidedJob) -> Option<cadaddy_mepfix::printlinks::SheetLinks> {
    use cadaddy_mepfix::printlinks::{self, LinkPage, SetSheet};
    let d = s.doc().ok()?;
    let store = d.space(&s.space())?;
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let sheets: Vec<SetSheet> = job
        .index
        .as_ref()
        .and_then(|i| i.get("sheets"))
        .and_then(Value::as_array)?
        .iter()
        .map(|v| SetSheet {
            number: text(v, "number"),
            title: text(v, "title"),
            discipline: text(v, "discipline"),
            sheet_type: text(v, "type"),
            level: text(v, "level"),
        })
        .collect();
    let mut spans = job.sheet_spans.clone();
    spans.sort_by_key(|sp| sp.lo);
    if spans.is_empty() {
        return None;
    }
    #[cfg(not(target_arch = "wasm32"))]
    let batch = rayon::current_num_threads().clamp(1, super::exchange::MAX_PAGES_AT_ONCE);
    #[cfg(target_arch = "wasm32")]
    let batch = 1usize;
    let mut pages: Vec<LinkPage> = Vec::with_capacity(spans.len());
    for chunk in spans.chunks(batch) {
        let mut parts: Vec<Vec<cadaddy_doc::Entity>> = vec![Vec::new(); chunk.len()];
        for e in store.iter() {
            if let Some(i) = chunk.iter().position(|sp| (sp.lo..sp.hi).contains(&e.handle.0))
                && let Some(v) = parts.get_mut(i)
            {
                v.push((**e).clone());
            }
        }
        let read = |(sp, ents): (&SheetSpan, Vec<cadaddy_doc::Entity>)| -> LinkPage {
            let base = sp.sheet.split('/').next().unwrap_or(&sp.sheet).to_string();
            let mut sub = cadaddy_doc::Drawing { blocks: d.blocks.clone(), ..cadaddy_doc::Drawing::default() };
            sub.model.push_many(ents);
            let k = printlinks::norm_sheet(&base);
            let sheet = sheets
                .iter()
                .find(|x| printlinks::norm_sheet(&x.number) == k)
                .cloned()
                .unwrap_or(SetSheet { number: base.clone(), ..SetSheet::default() });
            LinkPage { read: printlinks::read_page(&sub, &base), sheet }
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            pages.extend(chunk.par_iter().zip(parts.into_par_iter()).map(read).collect::<Vec<_>>());
        }
        #[cfg(target_arch = "wasm32")]
        pages.extend(chunk.iter().zip(parts).map(read));
    }
    Some(printlinks::link_set(&sheets, &pages))
}

/// A view of a typical room or of one room (stamped into its rooms, not placed as a part of the
/// floor).
fn room_view(title: &str) -> bool {
    let up = title.to_ascii_uppercase();
    ["QUEEN", "KING", "SUITE", "UNIT", "TYPICAL", "TYP.", "ROOM"].iter().any(|w| up.contains(w))
}

/// An enlarged view the sheet map registered on its overall plan (by its grids, tags, boundary
/// box or walls): its sheet, where its items are on that sheet, and the fit onto the overall
/// plan's sheet.
struct EnlargedFit {
    sheet: String,
    reach: [f64; 4],
    inner: Vec<[f64; 4]>,
    fit: cadaddy_mepfix::tagid::Sim,
}

/// The job's registered enlarged views of a part of a floor (not a typical room's), from its sheet
/// map.
fn enlarged_fits(job: &GuidedJob) -> Vec<EnlargedFit> {
    let rect = |v: &Value| -> Option<[f64; 4]> {
        let a: Vec<f64> = v.as_array()?.iter().filter_map(Value::as_f64).collect();
        <[f64; 4]>::try_from(a).ok().filter(|r| r.iter().all(|x| x.is_finite()))
    };
    job.links
        .as_ref()
        .and_then(|l| l.get("enlarged"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|e| e.get("registered").and_then(Value::as_bool) == Some(true))
        .filter(|e| !room_view(e.get("title").and_then(Value::as_str).unwrap_or("")))
        .filter_map(|e| {
            let f = rect(e.get("fit")?)?;
            let fit = cadaddy_mepfix::tagid::Sim { a: f[0], b: f[1], tx: f[2], ty: f[3] };
            Some(EnlargedFit {
                sheet: e.get("on_sheet").and_then(Value::as_str)?.to_string(),
                reach: rect(e.get("reach")?)?,
                inner: e.get("inner").and_then(Value::as_array).map(|v| v.iter().filter_map(rect).collect()).unwrap_or_default(),
                fit,
            })
        })
        .filter(|e| e.fit.scale() > 0.0)
        .take(MAX_ENLARGED_FITS)
        .collect()
}

/// Registered enlarged views the placer takes at most.
const MAX_ENLARGED_FITS: usize = 500;

/// The enlarged sheets are the authority inside their boundaries (one count per device): each
/// registered enlarged plan's devices are moved to where they sit on its overall plan (and take
/// that plan's level), and the overall plan's devices inside the boundary are left out. An
/// enlarged plan that shows no devices leaves the overall plan's alone. Returns (moved, left out).
fn enlarged_authority(job: &GuidedJob, links: &cadaddy_mepfix::printlinks::SheetLinks, census: &mut DeviceCensus) -> (usize, usize) {
    let mut moves: BTreeMap<&str, &cadaddy_mepfix::printlinks::Enlarged> = BTreeMap::new();
    let mut ignored: BTreeSet<&str> = BTreeSet::new();
    // (Not a typical room's or one room's view: those are stamped into their rooms, every room of
    // the type; moved onto the one room a callout points at, the rest would go without them.)
    for e in links.enlarged.iter().filter(|e| e.registered && e.fit.is_some() && e.devices > 0 && !room_view(&e.title)) {
        for h in &e.device_handles {
            moves.insert(h.as_str(), e);
        }
        ignored.extend(e.ignored_handles.iter().map(String::as_str));
    }
    // A registered view's other devices, by where they are: the types its own sheet's legend
    // doesn't list (a submittal's), the switches read at their designations. Not a typical room's
    // or one room's view: those are stamped into their rooms.
    let views: Vec<&cadaddy_mepfix::printlinks::Enlarged> =
        links.enlarged.iter().filter(|e| e.registered && e.fit.is_some() && e.devices > 0 && e.reach.is_some() && !room_view(&e.title)).collect();
    let inside = |r: &[f64; 4], p: [f64; 2]| p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3];
    let sheet_of = |n: &str| cadaddy_mepfix::printlinks::norm_sheet(n.split('/').next().unwrap_or(n));
    let by_place = |dv: &super::readers::ReadDevice| {
        views.iter().copied().find(|e| {
            sheet_of(&dv.sheet) == sheet_of(&e.sheet) && e.reach.is_some_and(|r| inside(&r, dv.at)) && !e.inner.iter().any(|r| inside(r, dv.at))
        })
    };
    let (mut moved, mut dropped) = (0usize, 0usize);
    let mut keep = Vec::with_capacity(census.placed.len());
    for mut dv in std::mem::take(&mut census.placed) {
        if let Some(e) = moves.get(dv.handle.as_str()).copied().or_else(|| by_place(&dv))
            && let Some(at) = e.place(dv.at)
        {
            dv.at = at;
            dv.level = job.level_at(at).map_or_else(|| e.level.clone(), |r| r.level.clone());
            moved += 1;
        } else if ignored.contains(dv.handle.as_str()) {
            if let Some(n) = census.by_type.get_mut(&dv.tag) {
                *n = n.saturating_sub(1);
            }
            dropped += 1;
            continue;
        }
        keep.push(dv);
    }
    census.by_type.retain(|_, n| *n > 0);
    census.placed = keep;
    (moved, dropped)
}

/// Leave out the census's devices another sheet already shows (`identity::duplicate_devices`);
/// how many.
fn device_identity(s: &Session, job: &GuidedJob, census: &mut DeviceCensus) -> usize {
    let Ok(d) = s.doc() else { return 0 };
    let sheets: Vec<super::identity::SheetBox> = job
        .sheet_regions
        .iter()
        .filter(|r| r.kind == "plan" && !r.level.trim().is_empty() && r.not_built.is_empty() && r.rect.iter().all(|v| v.is_finite()))
        .map(|r| super::identity::SheetBox::new(&r.number, &r.discipline, &r.level, r.rect))
        .collect();
    let upf = 12.0 * super::perceive::units_per_inch(d);
    let (drop, _) = super::identity::duplicate_devices(d, &sheets, upf, &census.placed);
    if drop.is_empty() {
        return 0;
    }
    let mut keep = Vec::with_capacity(census.placed.len());
    for (i, dv) in std::mem::take(&mut census.placed).into_iter().enumerate() {
        if drop.contains(&i) {
            if let Some(n) = census.by_type.get_mut(&dv.tag) {
                *n = n.saturating_sub(1);
            }
            continue;
        }
        keep.push(dv);
    }
    census.by_type.retain(|_, n| *n > 0);
    census.placed = keep;
    drop.len()
}

/// An 800-pixel PNG of the drawing around `at` (the first symbol printing an unlisted tag),
/// written into `dir`; its path.  Returns `None` when the render is blank after retries.
fn tag_crop(s: &Session, at: [f64; 2], span: (u64, u64), handle: Option<&str>, dir: &std::path::Path, name: &str, sheet: &str) -> Option<String> {
    let d = s.doc().ok()?;
    let store = d.space(&s.space())?;
    let centre = cadaddy_geom::Vec2::new(at[0], at[1]);
    // Size the window from the symbol itself: a few symbol widths each way.
    let sym = handle
        .and_then(|h| u64::from_str_radix(h, 16).ok())
        .and_then(|h| store.iter().find(|e| e.handle.0 == h))
        .map(|e| cadaddy_doc::entity_bounds(d, e, 0))
        .filter(|b| !b.is_empty());
    let r = sym.map_or(0.0, |b| b.width().max(b.height()));
    let base_half = if r.is_finite() && r > 0.0 { 4.0 * r } else { 24.0 };
    let ro = cadaddy_render::raster::RasterOptions { background: cadaddy_color::Rgb(255, 255, 255), hairline: 1.0, px_per_mm: 0.0, antialias: true };
    // Try progressively wider windows if the render is blank.
    let mut png_bytes: Option<Vec<u8>> = None;
    for zoom in &[1.0_f64, 2.0, 4.0] {
        let half = base_half * zoom;
        let window = cadaddy_geom::Bounds2::new(centre, centre).expand(half);
        // The sheets overlap in model space: draw only this sheet's objects.
        let near: Vec<&cadaddy_doc::Entity> = store
            .iter()
            .filter(|e| (span.0..span.1).contains(&e.handle.0) && cadaddy_doc::entity_bounds(d, e, 0).intersects(&window))
            .map(|e| e.as_ref())
            .take(20_000)
            .collect();
        let list = cadaddy_render::build_entities(d, near, &cadaddy_render::Options::default());
        let view = cadaddy_render::raster::View::fit(&window, 800, 800, 0.0);
        let Some(pm) = cadaddy_render::raster::render(&list, &view, &ro) else { continue };
        let blank = pm.data().chunks_exact(4).all(|px| px[0] == 255 && px[1] == 255 && px[2] == 255);
        if let Ok(encoded) = pm.encode_png() {
            if !blank {
                png_bytes = Some(encoded);
                break;
            }
            // Keep the last render even if blank — we'll save it if nothing better comes.
            png_bytes = Some(encoded);
        }
    }
    let png = png_bytes?;
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).take(24).collect();
    let sheet_safe: String = sheet.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).take(16).collect();
    let path = dir.join(format!("tag-{safe}-{sheet_safe}.png"));
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::create_dir_all(dir).ok()?;
        crate::writefile::write(&path, png).ok()?;
        Some(path.to_string_lossy().into_owned())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (png, path);
        None
    }
}

/// The imported plan sheets of a discipline (`A`, `E`…): their numbers.
fn plan_sheets(job: &GuidedJob, discipline: &str) -> Vec<String> {
    let mut out: Vec<String> = job
        .sheet_regions
        .iter()
        .filter(|r| r.discipline.eq_ignore_ascii_case(discipline) && matches!(r.kind.as_str(), "plan" | "enlarged_plan"))
        .map(|r| r.number.clone())
        .collect();
    out.dedup();
    out.truncate(MAX_VERIFY_SAMPLES);
    out
}

/// Evidence for a stage that read nothing: the plans it read (the first is the sheet to look at).
fn zero_evidence(plans: &[String]) -> Option<QuestionEvidence> {
    let first = plans.first()?;
    Some(QuestionEvidence {
        crop_png_path: None,
        sheet: Some(first.clone()),
        at: None,
        values: Some(json!({"plans": plans, "found": 0})),
        handles: Vec::new(),
        tiled_crops: None,
        leftovers: Vec::new(),
    })
}

/// Run the rooms stage: detect rooms from labels (MUST 1, 4).
/// Uses the same readers (rooms.list) that work directly on the job's sheets.
/// Finish and material codes of the finish legend (gypsum board, carpet, vinyl and ceramic tile,
/// acoustic tile, paint, rubber base, wood, epoxy, sealed concrete): a tag of one is no room.
const FINISH_CODES: [&str; 14] = ["GYP", "GWB", "CPT", "VCT", "LVT", "LVP", "ACT", "PT", "RB", "WD", "CT", "QT", "EPX", "SC"];

/// A finish or material tag (GYP 01, CPT-1, VCT 2, ACT-1) traced in its room: a room read with a
/// finish code for a name and a short number, or none.
fn finish_tag(r: &Value) -> bool {
    let name = r.get("name").and_then(Value::as_str).unwrap_or("").trim().to_ascii_uppercase();
    let first = name.split(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or("");
    let short_number = r.get("number").and_then(Value::as_str).is_none_or(|n| n.trim().len() <= 2);
    short_number && FINISH_CODES.contains(&first)
}

fn run_rooms(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    // Load plan-sheet PDFs into the drawing so rooms.list finds entities.
    ensure_sheets_loaded(s, job)?;
    // Each floor plan's labels read in full, sheet by sheet (a whole set's labels are more than
    // one reading traces: the last floors were left out).
    let plans = |arch_only: bool| -> Vec<[f64; 4]> {
        job.sheet_regions
            .iter()
            .filter(|r| r.kind == "plan" && r.not_built.is_empty() && (!arch_only || r.discipline.eq_ignore_ascii_case("A")))
            .map(SheetRegion::plan_rect)
            .collect()
    };
    // Rooms come from the architectural floor plans: other trades' plans show the same rooms
    // again (and phantom ones). Any plan when the set has no architectural ones.
    let regions = Some(plans(true)).filter(|r| !r.is_empty()).unwrap_or_else(|| plans(false));
    // With outlines: the typical-room plans' devices are stamped into each room of the type.
    let mut params = json!({"outline": true});
    if !regions.is_empty() {
        params["regions"] = json!(regions);
    }
    let mut rooms_result = s.execute("rooms.list", &params)?;
    // Architecture gives the rooms; the other disciplines' plans of a floor may give their numbers
    // (an architectural plan can tag guest rooms with the unit type only). From those plans only a
    // numbered room whose number the architectural plans didn't give is taken: a room they'd add
    // without one is a phantom (their notes, their equipment rooms drawn again).
    // Added after the checks below: they are where their own sheet draws them, not on the
    // architectural plan the building's outline is read from.
    let mut extra_rooms: Vec<Value> = Vec::new();
    let others: Vec<[f64; 4]> = job
        .sheet_regions
        .iter()
        .filter(|r| r.kind == "plan" && r.not_built.is_empty() && !r.discipline.eq_ignore_ascii_case("A"))
        .map(SheetRegion::plan_rect)
        .collect();
    if !others.is_empty()
        && plans(true).len() == regions.len()
        && let Ok(more) = s.execute("rooms.list", &json!({"outline": false, "regions": others}))
    {
        let extra: Vec<Value> = more
            .get("rooms")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|r| r.get("number").and_then(Value::as_str).is_some_and(|n| !n.trim().is_empty()))
            // On its floor by the floor's numbering: a guest floor's rooms start with its
            // number (205 on level 2); a note's or a schedule's number doesn't.
            .filter_map(|r| {
                let at = r.get("at").and_then(|a| Some([a.get(0)?.as_f64()?, a.get(1)?.as_f64()?]));
                // The floor of the sheet it was read on (the whole sheet: a room by its edge is
                // outside the plan area).
                let sheet = at.and_then(|at| job.sheet_regions.iter().filter(|r| r.kind == "plan").find(|r| in_rect(&r.rect, at)))?;
                let floor = sheet.level.chars().find(char::is_ascii_digit)?;
                let number = r.get("number").and_then(Value::as_str).unwrap_or("").trim();
                if number.len() < 3 || !number.starts_with(floor) {
                    return None;
                }
                // On that sheet's level, not the one the read guessed (a third-floor number
                // numbered a first-floor room).
                let mut r = r.clone();
                if let Some(o) = r.as_object_mut() {
                    o.insert("level".into(), json!(sheet.level));
                    o.insert("sheet".into(), json!(sheet.number));
                }
                Some(r)
            })
            .collect();
        // Each number once (every discipline's plan of the floor gives it).
        let mut seen = BTreeSet::new();
        extra_rooms = extra
            .into_iter()
            .filter(|r| seen.insert(r.get("number").and_then(Value::as_str).unwrap_or("").trim().to_ascii_uppercase()))
            .map(|mut r| {
                if let Some(o) = r.as_object_mut() {
                    o.insert("other_trade".into(), json!(true));
                }
                r
            })
            .collect();
    }
    // A room carries the level of the sheet it was read on (Q5291 MUST 2): the sheet's level
    // wins over a spec's guess.
    if let Some(list) = rooms_result.get_mut("rooms").and_then(Value::as_array_mut) {
        for room in list.iter_mut() {
            let at = room.get("at").and_then(|a| Some([a.get(0)?.as_f64()?, a.get(1)?.as_f64()?]));
            let Some(region) = at.and_then(|at| job.level_at(at)) else { continue };
            if let Some(o) = room.as_object_mut() {
                if !region.level.is_empty() {
                    o.insert("level".into(), json!(region.level));
                }
                o.insert("sheet".into(), json!(region.number));
            }
        }
    }
    let dropped = rooms_off_the_building(job, &mut rooms_result);
    if let Some(o) = rooms_result.as_object_mut() {
        o.insert("not_rooms".into(), dropped);
        if let Some(n) = o.get("rooms").and_then(Value::as_array).map(Vec::len) {
            o.insert("count".into(), json!(n));
        }
    }
    if let Some(list) = rooms_result.get_mut("rooms").and_then(Value::as_array_mut) {
        list.retain(|r| !finish_tag(r));
    }
    let duplicates = one_room_per_number(job, &mut rooms_result);
    if !extra_rooms.is_empty()
        && let Some(list) = rooms_result.get_mut("rooms").and_then(Value::as_array_mut)
    {
        // Only numbers the architectural plans' rooms (as checked) don't have.
        let key = |r: &Value| r.get("number").and_then(Value::as_str).unwrap_or("").trim().to_ascii_uppercase();
        let have: BTreeSet<String> = list.iter().map(key).filter(|n| !n.is_empty()).collect();
        list.extend(extra_rooms.into_iter().filter(|r| !have.contains(&key(r))));
        let n = list.len();
        if let Some(o) = rooms_result.as_object_mut() {
            o.insert("count".into(), json!(n));
        }
    }
    let (removed, renamed) = apply_room_answers(job, &mut rooms_result);
    if let Some(o) = rooms_result.as_object_mut() {
        if duplicates > 0 {
            o.insert("duplicates_merged".into(), json!(duplicates));
        }
        if removed > 0 {
            o.insert("removed_by_answers".into(), json!(removed));
        }
        if renamed > 0 {
            o.insert("renamed_by_answers".into(), json!(renamed));
        }
        if duplicates + removed > 0
            && let Some(n) = o.get("rooms").and_then(Value::as_array).map(Vec::len)
        {
            o.insert("count".into(), json!(n));
        }
    }

    let rooms = rooms_result.get("rooms").and_then(Value::as_array);
    let room_count = rooms.map_or(0, Vec::len);
    {
        let mut by: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        for r in rooms.into_iter().flatten() {
            let e = by.entry(r.get("level").and_then(Value::as_str).unwrap_or("?").to_string()).or_default();
            e.0 += 1;
            if r.get("number").and_then(Value::as_str).is_some_and(|n| !n.trim().is_empty()) {
                e.1 += 1;
            }
        }
        progress_line(&format!("  rooms per level (rooms, with a number): {by:?}"));
    }

    // MUST 1: No empty success — 0 rooms is a finding.
    if room_count == 0 {
        job.add_finding(
            StageName::Rooms,
            "Rooms stage found 0 rooms",
            "The rooms reader found no room labels or outlines in the loaded sheets",
            4,
            vec!["Check that the loaded sheets contain room tags".into(), "Try rooms.list directly on individual sheets".into()],
            None,
            false,
        );
        // Raise a question rather than silently proceeding. With architectural plans in the set
        // there should be rooms: that is always asked, never taken as the default.
        let plans = plan_sheets(job, "A");
        let qid = job.next_qid(StageName::Rooms);
        if !plans.is_empty() {
            job.always_ask.insert(qid.clone());
        }
        let q = Question {
            id: qid,
            stage: StageName::Rooms,
            question: if plans.is_empty() {
                "No rooms detected on any sheet. Are there room tags on the plans?".into()
            } else {
                format!("No rooms read on the architectural plans ({}). Are there room tags on them?", plans.join(", "))
            },
            why: "The rooms reader found 0 rooms; the building model needs room boundaries".into(),
            choices: vec!["There should be rooms — re-check".into(), "No room tags on these sheets".into()],
            evidence: zero_evidence(&plans),
            default: Some("No room tags on these sheets".into()),
            answer_schema: Some(json!({"type": "string"})),
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: QuestionKind::Completeness,
        };
        job.add_question_to_worklist(&q);
        if let Some(st) = job.stage_mut(StageName::Rooms) {
            st.completeness.push(q);
        }
    }

    if let Some(rooms) = rooms {
        // Check for equipment labels.
        let equipment_patterns = ["PNLBD", "PANEL", "XFMR", "TRANS", "MCC", "SWG", "ATS", "GEN", "MDP", "CDP"];
        for room in rooms.iter().take(MAX_QUESTIONS_PER_STAGE) {
            let name = room.get("name").and_then(Value::as_str).unwrap_or("?");
            let number = room.get("number").and_then(Value::as_str).unwrap_or("");

            let looks_like_equipment = equipment_patterns.iter().any(|p| name.to_uppercase().contains(p));
            if looks_like_equipment {
                let qid = job.next_qid(StageName::Rooms);
                let q = Question {
                    id: qid,
                    stage: StageName::Rooms,
                    question: format!("Room {number} label '{name}' looks like equipment, not a room name — use a different name?"),
                    why: "Label matches an equipment pattern; may not be a room name".into(),
                    choices: vec![format!("Keep '{name}' as room name"), "Use a different name".into(), "Not a room — remove it".into()],
                    evidence: Some(QuestionEvidence {
                        crop_png_path: None,
                        sheet: None,
                        at: room.get("at").and_then(|a| {
                            let arr = a.as_array()?;
                            Some([arr.first()?.as_f64()?, arr.get(1)?.as_f64()?])
                        }),
                        values: Some(json!({"room_name": name, "room_number": number})),
                        handles: Vec::new(),
                        tiled_crops: None,
                        leftovers: Vec::new(),
                    }),
                    default: Some(format!("Keep '{name}' as room name")),
                    answer_schema: None,
                    answer: None,
                    answered_by: None,
                    answered_at: None,
                    kind: QuestionKind::Doubt,
                };
                if let Some(st) = job.stage_mut(StageName::Rooms) {
                    st.questions.push(q);
                }
            }
        }

        // Verify samples.
        let sample_count = rooms.len().min(MAX_VERIFY_SAMPLES);
        for room in rooms.iter().take(sample_count) {
            let name = room.get("name").and_then(Value::as_str).unwrap_or("?");
            let number = room.get("number").and_then(Value::as_str).unwrap_or("");
            let area = room.get("area_sqft").and_then(Value::as_f64).unwrap_or(0.0);
            let qid = job.next_qid(StageName::Rooms);
            let q = Question {
                id: qid,
                stage: StageName::Rooms,
                question: format!("Room {number} '{name}': {area:.0} sq ft — name and area right?"),
                why: "Verify sample: spot-checking detected rooms".into(),
                choices: vec!["Correct".into(), "Wrong name".into(), "Wrong area".into(), "Not a room".into()],
                evidence: Some(QuestionEvidence {
                    crop_png_path: None,
                    sheet: None,
                    at: None,
                    values: Some(json!({"room": name, "number": number, "area_sqft": area})),
                    handles: Vec::new(),
                    tiled_crops: None,
                    leftovers: Vec::new(),
                }),
                default: Some("Correct".into()),
                answer_schema: None,
                answer: None,
                answered_by: None,
                answered_at: None,
                kind: QuestionKind::Verify,
            };
            if let Some(st) = job.stage_mut(StageName::Rooms) {
                st.verify.push(q);
            }
        }
    }

    job.audit("stage_run", "pipeline", None, Some(StageName::Rooms), Some(json!({"rooms_found": room_count})));
    Ok(rooms_result)
}

/// Take out what was read as rooms but is not the building's (MUST 1-3): labels in a sheet's
/// title block or border (firm names, addresses, review stamps), rooms read on a site, civil or
/// level-less sheet, the other disciplines' unnumbered labels on a level the architectural plan
/// shows (legends, notes), and labels no room is named (notes, dates, view titles). How many
/// went, by why.
fn rooms_off_the_building(job: &GuidedJob, rooms_result: &mut Value) -> Value {
    let Some(list) = rooms_result.get_mut("rooms").and_then(Value::as_array_mut) else { return json!({}) };
    let a_levels: Vec<String> = job
        .sheet_regions
        .iter()
        .filter(|r| r.discipline == "A" && r.kind == "plan" && r.not_built.is_empty())
        .map(|r| r.level.trim().to_ascii_uppercase())
        .collect();
    let (mut sheet_furniture, mut site, mut other_discipline, mut not_a_name) = (0usize, 0usize, 0usize, 0usize);
    list.retain(|room| {
        let at = room.get("at").and_then(|a| Some([a.get(0)?.as_f64()?, a.get(1)?.as_f64()?]));
        let name = room.get("name").and_then(Value::as_str).unwrap_or("");
        if !name.trim().is_empty() && (!super::autospec::room_like_name(name) || !legible_room_name(name)) {
            not_a_name += 1;
            return false;
        }
        let Some(region) = at.and_then(|at| job.level_at(at)) else { return true };
        if at.is_some_and(|at| !region.in_area(at)) {
            sheet_furniture += 1;
            return false;
        }
        if !region.not_built.is_empty() {
            site += 1;
            return false;
        }
        // Another discipline's copy of a level the architectural plan shows: only a numbered
        // room (the copy of one the architectural reading missed; one per number is kept
        // below), never its legends and notes.
        let numbered = room.get("number").and_then(Value::as_str).is_some_and(|n| n.chars().any(|c| c.is_ascii_digit()));
        if region.discipline != "A" && !numbered && a_levels.contains(&region.level.trim().to_ascii_uppercase()) {
            other_discipline += 1;
            return false;
        }
        true
    });
    json!({"title_block_or_border": sheet_furniture, "site_or_no_level": site, "other_discipline": other_discipline, "not_a_room_name": not_a_name})
}

/// Whether a room name reads as words: a run of three letters (`LOBBY`, `IDF`, `CORR`) or a
/// two-letter code (`WC`, `CL`). Text read from a print in a font the reader can't map comes out
/// as `-4--P- -I--Y-`, `Xb` or `D D`, and a dimension string as `22 40 OO`.
fn legible_room_name(name: &str) -> bool {
    let t = name.trim();
    if t.len() == 2 && t.bytes().all(|b| b.is_ascii_uppercase()) {
        return true;
    }
    let mut run = 0;
    for c in t.chars() {
        run = if c.is_alphabetic() { run + 1 } else { 0 };
        if run >= 3 {
            return true;
        }
    }
    false
}

/// The rule key of a room answer: `room:<number>|<name>`, upper case.
fn room_key(number: &str, name: &str) -> String {
    format!("room:{}|{}", number.trim().to_ascii_uppercase(), name.trim().to_ascii_uppercase())
}

/// The answers given about rooms ("not a room", a new name): rooms removed and renamed.
fn apply_room_answers(job: &GuidedJob, rooms_result: &mut Value) -> (usize, usize) {
    let rules: BTreeMap<&str, &str> = job.rules.iter().filter(|r| r.key.starts_with("room:")).map(|r| (r.key.as_str(), r.value.as_str())).collect();
    if rules.is_empty() {
        return (0, 0);
    }
    let Some(list) = rooms_result.get_mut("rooms").and_then(Value::as_array_mut) else { return (0, 0) };
    let key = |r: &Value| room_key(r.get("number").and_then(Value::as_str).unwrap_or(""), r.get("name").and_then(Value::as_str).unwrap_or(""));
    // A room said not to be one (`room:<number>|<name>`) goes under any name its number reads,
    // and as the same name with no number (the model's copy of it: a corridor built from the
    // circulation came back after "Not a room").
    let gone: Vec<(String, String)> = rules
        .iter()
        .filter(|(_, v)| **v == "remove")
        .filter_map(|(k, _)| k.strip_prefix("room:")?.split_once('|').map(|(n, m)| (n.to_string(), m.to_string())))
        .collect();
    let removed_room = |r: &Value| {
        let up = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").trim().to_ascii_uppercase();
        let (number, name) = (up("number"), up("name"));
        rules.get(key(r).as_str()) == Some(&"remove")
            || gone.iter().any(|(n, m)| (!n.is_empty() && *n == number) || (number.is_empty() && !m.is_empty() && *m == name))
    };
    let before = list.len();
    list.retain(|r| !removed_room(r));
    let removed = before - list.len();
    let mut renamed = 0;
    for r in list.iter_mut() {
        if let Some(name) = rules.get(key(r).as_str()).and_then(|v| v.strip_prefix("name:"))
            && let Some(o) = r.as_object_mut()
        {
            o.insert("name".into(), json!(name));
            renamed += 1;
        }
    }
    (removed, renamed)
}

/// One room per number and level: the electrical, mechanical and plumbing plans tag the same
/// rooms as the architectural plan of their level, so a numbered room read on several sheets of
/// one level is one room (the architectural sheet's reading, else the first). Rooms with no
/// number or no level are kept as read. How many copies were dropped.
fn one_room_per_number(job: &GuidedJob, rooms_result: &mut Value) -> usize {
    let Some(list) = rooms_result.get_mut("rooms").and_then(Value::as_array_mut) else { return 0 };
    let text = |r: &Value, k: &str| r.get(k).and_then(Value::as_str).map(|v| v.trim().to_ascii_uppercase()).unwrap_or_default();
    let architectural = |r: &Value| {
        let sheet = text(r, "sheet");
        job.sheet_regions.iter().any(|g| g.number.eq_ignore_ascii_case(&sheet) && g.discipline == "A")
    };
    // The reading kept for each (level, number): its index in the list.
    let mut best: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (i, r) in list.iter().enumerate() {
        let (level, number) = (text(r, "level"), text(r, "number"));
        if level.is_empty() || number.is_empty() {
            continue;
        }
        match best.get(&(level.clone(), number.clone())).copied() {
            Some(j) if !architectural(r) || list.get(j).is_some_and(architectural) => {}
            _ => {
                best.insert((level, number), i);
            }
        }
    }
    let before = list.len();
    let mut i = 0;
    list.retain(|r| {
        let keep = {
            let (level, number) = (text(r, "level"), text(r, "number"));
            level.is_empty() || number.is_empty() || best.get(&(level, number)) == Some(&i)
        };
        i += 1;
        keep
    });
    before - list.len()
}

/// Circuit and panel references end in a dash and a number (`D-1`, `DP1-13`, `B-12`); device
/// marks don't (`A`, `OS`, `EF`, `L4`, `XA`).
/// Whether a text printed beside a device is its circuit: `G-12`, `LP-3,5` (see
/// [`cadaddy_mepfix::sheet_facts::is_circuit_label`]) and the panel-and-number forms `LP1-3`, `2LA-12`, `H1/7`
/// (a panel name with a letter, then one to three digits), never one of the set's sheet numbers.
pub fn circuit_text(t: &str, sheet_numbers: &BTreeSet<String>) -> bool {
    let t = t.trim();
    if sheet_numbers.contains(t) || sheet_numbers.contains(&t.to_ascii_uppercase()) {
        return false;
    }
    if cadaddy_mepfix::sheet_facts::is_circuit_label(t) {
        return true;
    }
    let Some((panel, n)) = t.rsplit_once(['-', '/']) else { return false };
    (2..=6).contains(&panel.len())
        && panel.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && panel.chars().any(|c| c.is_ascii_uppercase())
        && panel.chars().any(|c| c.is_ascii_digit())
        && (1..=3).contains(&n.len())
        && n.chars().all(|c| c.is_ascii_digit())
}

pub(crate) fn is_circuit_tag(tag: &str) -> bool {
    tag.rsplit_once('-').is_some_and(|(head, n)| !head.is_empty() && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// How far from an occupancy sensor its leg letter is printed, in the letter's heights.
const SENSOR_LEG_H: f64 = 4.0;
/// How far from a receptacle's circuit label a note on it (GFCI) is printed, in its heights.
const RECEPTACLE_NOTE_H: f64 = 4.0;
/// How far a symbol's type mark is printed from it, in the mark's text heights (over or beside
/// it, its circuit label on the other side).
const TYPE_MARK_REACH_H: f64 = 3.0;
/// A tag's text height where the import didn't keep it (drawing units).
const DEFAULT_MARK_HEIGHT: f64 = 6.0;
/// A lone luminaire mark's circuit label is within this of it, in its text heights; a device read
/// that close is the mark's.
const LONE_MARK_REACH_H: f64 = 4.0;
/// A device read within this of where a lone mark's symbol would be drawn (in its text heights)
/// is that symbol.
const LONE_SYMBOL_H: f64 = 2.0;
/// Most luminaires one sheet's lone marks add.
const MAX_LONE_MARKS: usize = 2_000;

/// A receptacle's designations among the texts printed by it, in a fixed order: `GFCI` (or
/// `GFI`), `WP` weatherproof, `WR` weather-resistant, `TR` tamper-resistant, `USB`.
fn receptacle_designations(texts: &[&str]) -> Vec<&'static str> {
    const DESIGNATIONS: &[(&str, &[&str])] = &[("GFCI", &["GFCI", "GFI"]), ("WP", &["WP"]), ("WR", &["WR"]), ("TR", &["TR"]), ("USB", &["USB"])];
    let words: BTreeSet<String> =
        texts.iter().flat_map(|t| t.split(|c: char| !c.is_ascii_alphanumeric())).filter(|w| !w.is_empty()).map(str::to_ascii_uppercase).collect();
    DESIGNATIONS.iter().filter(|(_, said)| said.iter().any(|w| words.contains(*w))).map(|(d, _)| *d).collect()
}

/// Run the devices stage: detect devices, symbols and counts (MUST 1, 4, 9-11).
/// MUST 9: count then "anything not counted?" with leftover clusters.
/// MUST 10: learn foreign symbols. MUST 11: recount after learning.
fn run_devices(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    // Load sheets so the drawing has PDF-SYM block inserts from pdfnative symbol detection.
    ensure_sheets_loaded(s, job)?;

    let mut results = json!({});
    let mut total_items = 0usize;
    let mut reader_errors = Vec::new();

    // ── Primary source: PDF-SYM block inserts (pdfnative-rw2 symbol detection) ──
    // After pdfimport, repeated symbol clusters become block inserts named PDF-SYM-N
    // with a TAG attribute holding the nearby device-mark text.
    // Device types come from the sheets' legends and schedules (plus the marks every set
    // uses); notes, sheet numbers, keynotes, switch legs and dimensions never count.
    // The submittals' product types, read once: device types too.
    if !job.submittals.is_empty() && job.submittal_types.is_empty() {
        let started = web_time::Instant::now();
        let marks = super::submittals::product_marks(s, &job.submittals);
        (job.submittal_types, job.luminaire_types, job.wall_luminaire_types) = (marks.types, marks.ceiling, marks.wall);
        progress_line(&format!(
            "  submittal product types (device types too): {} in {:.1} s",
            job.submittal_types.join(", "),
            started.elapsed().as_secs_f64()
        ));
    }
    // The sheet map: what the sheets' callouts point at, where the enlarged plans sit. Its bubbles
    // are no devices (a bubble's sheet reference reads like a circuit).
    let links = job_links(s, job);
    let bubbles: Vec<([f64; 2], f64)> = links.iter().flat_map(|l| l.callouts.iter()).map(|c| (c.at, c.radius)).collect();
    let mut census = device_census(s, job, &bubbles);
    // Inside a registered enlarged plan's boundary its sheet is the authority: its devices move
    // onto the overall plan there, and the overall plan's own copies there are left out.
    if let Some(links) = links {
        let summary = super::printset::links_summary(&links);
        results["links"] = summary.clone();
        job.links = Some(summary);
        let (moved, dropped) = enlarged_authority(job, &links, &mut census);
        if moved + dropped > 0 {
            results["enlarged_devices"] = json!({"moved_onto_overall_plans": moved, "overall_copies_left_out": dropped});
        }
    }
    // Tags are identities: a device another sheet of its level shows at the same spot (once the
    // sheets are put onto the level's plan by the door and room numbers they share) is counted
    // once.
    let merged = device_identity(s, job, &mut census);
    if merged > 0 {
        results["identity"] = json!({"devices_merged": merged});
    }
    results["circuit_tagged_symbols"] = json!(census.circuit);
    results["circuit_tagged_retyped"] = json!(census.retyped);
    results["luminaires_by_mark_alone"] = json!(census.lone_marks);
    let not_devices: usize = census.not_device.values().sum();
    results["non_device_tags"] = json!(not_devices);
    results["device_types"] = json!({
        "legend_tables": census.legend_tables,
        "legend_types": census.legend_types,
        "legend_samples": census.legend_samples,
        "untagged_symbols": census.untagged,
        "not_devices": census.not_device,
        "unlisted_repeated": census.unlisted.iter().map(|(t, u)| (t.clone(), json!(u.count))).collect::<serde_json::Map<String, Value>>(),
    });
    let sym_count: usize = census.by_type.values().sum();
    if sym_count > 0 || !census.unlisted.is_empty() {
        total_items += sym_count;
        let items: Vec<Value> =
            census.by_type.iter().map(|(tag, &count)| json!({"tag": tag, "type": tag, "count": count, "source": "pdf_symbol"})).collect();
        let by_type: serde_json::Map<String, Value> = census.by_type.iter().map(|(k, v)| (k.clone(), json!(*v))).collect();
        // Unlisted tags printed again and again: ask which device each is, with a crop.
        let mut unlisted: Vec<(&String, &UnlistedTag)> = census.unlisted.iter().collect();
        unlisted.sort_by(|a, b| b.1.count.cmp(&a.1.count).then_with(|| a.0.cmp(b.0)));
        let crop_dir = std::env::temp_dir().join(format!("cadaddy-{}", job.id)).join("device-tags");
        let unknowns: Vec<Value> = unlisted
            .iter()
            .enumerate()
            .map(|(i, (tag, u))| {
                let crop =
                    if i < MAX_TAG_CROPS { tag_crop(s, u.at, u.span, u.handles.first().map(String::as_str), &crop_dir, tag, &u.sheet) } else { None };
                json!({"description": tag, "tag": tag, "count": u.count, "at": u.at, "sheet": u.sheet, "handles": u.handles, "crop_path": crop})
            })
            .collect();
        results["devices"] = json!({
            "items": items,
            "count": sym_count,
            "by_type": by_type,
            "unknowns": unknowns,
            "source": "pdf_symbol_blocks",
        });
    }

    // Every confirmed device goes in the model on its sheet's level (MUST 1): now when there is
    // a building model, and again by the bim_build stage after it drafts the spec.
    job.devices = census.placed.clone();
    let has_model = s.doc().is_ok_and(|d| d.building_spec.as_deref().is_some_and(|t| !t.trim().is_empty()));
    results["model"] = if job.devices.is_empty() {
        json!({"inserted": 0})
    } else if has_model {
        super::readers::place_devices(s, &job.devices, "guided job").unwrap_or_else(|e| json!({"inserted": 0, "why_not": e.to_string()}))
    } else {
        json!({"inserted": 0, "pending": job.devices.len(), "why_not": "no building model yet: the bim_build stage puts them in after drafting it"})
    };

    // ── Secondary: schedules.read for device marks in schedule tables ──
    match s.execute("schedules.read", &json!({})) {
        Ok(v) => {
            if let Some(items) = v.get("items").and_then(Value::as_array) {
                total_items += items.len();
            }
            results["schedules"] = v;
        }
        Err(e) => {
            reader_errors.push(format!("schedules.read: {e}"));
        }
    }

    // ── Tertiary: existing readers (may be stubs but call them for completeness) ──
    for (cmd, key) in [("mep.read_devices", "mep_devices"), ("conduit.read", "conduit"), ("gear.read", "gear")] {
        match s.execute(cmd, &json!({})) {
            Ok(v) => {
                if let Some(items) = v.get("items").and_then(Value::as_array) {
                    total_items += items.len();
                }
                if results.get(key).is_none() {
                    results[key] = v;
                }
            }
            Err(e) => {
                let msg = e.to_string();
                reader_errors.push(format!("{cmd}: {msg}"));
                results[key] = json!({"error": msg});
            }
        }
    }

    // MUST 1: No empty success — 0 devices found is a finding. On electrical plans no device
    // symbol at all is one too, and always asked.
    let e_plans = plan_sheets(job, "E");
    let no_symbols = sym_count == 0 && census.unlisted.is_empty() && !e_plans.is_empty();
    if total_items == 0 || no_symbols {
        let msg = if reader_errors.is_empty() {
            "Devices stage found 0 devices on all sheets".to_string()
        } else {
            format!("Devices stage found 0 devices; reader errors: {}", reader_errors.join("; "))
        };
        job.add_finding(
            StageName::Devices,
            &msg,
            "No E-sheets found in the project or readers returned empty; check that classify tagged electrical sheets",
            4,
            vec!["Check that classify identified E-discipline sheets".into(), "Try mep.read_devices directly".into()],
            None,
            false,
        );
        let qid = job.next_qid(StageName::Devices);
        if !e_plans.is_empty() {
            job.always_ask.insert(qid.clone());
        }
        let q = Question {
            id: qid,
            stage: StageName::Devices,
            question: if e_plans.is_empty() {
                "No devices detected. Are there device symbols on the electrical sheets?".into()
            } else {
                format!("No device symbols counted on the electrical plans ({}). Are there devices on them?", e_plans.join(", "))
            },
            why: "The device reader found 0 items; the takeoff needs device counts".into(),
            choices: vec!["There should be devices — re-check".into(), "No devices on these sheets".into()],
            evidence: zero_evidence(&e_plans),
            default: Some("No devices on these sheets".into()),
            answer_schema: Some(json!({"type": "string"})),
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: QuestionKind::Completeness,
        };
        job.add_question_to_worklist(&q);
        if let Some(st) = job.stage_mut(StageName::Devices) {
            st.completeness.push(q);
        }
    }

    // MUST 9: Completeness checkpoints per device class.
    let device_classes = [
        ("duplex_receptacle", "duplex receptacles"),
        ("gfci", "GFCI receptacles"),
        ("ef", "exhaust fans"),
        ("light", "light fixtures"),
        ("switch", "switches"),
        ("data", "data outlets"),
        ("fire_alarm", "fire alarm devices"),
    ];

    if let Some(devices) = results.get("devices") {
        // Collect unlisted tag names for "they are drawn as <tag>" choices.
        let unlisted_tags: Vec<String> = census.unlisted.keys().take(10).cloned().collect();
        for (class_key, class_label) in &device_classes {
            let count = devices.get("by_type").and_then(|bt| bt.get(*class_key)).and_then(Value::as_u64).unwrap_or(0);
            // MUST 9: Always ask — even for 0 ("I counted 0 X — anything not counted?").
            // Build choices: "count is complete", "none on this sheet", or "they are drawn as <tag>"
            // for each unlisted tag, plus "there are uncounted devices".
            let mut choices = vec!["Count is complete".into()];
            if count == 0 {
                choices.push("None on these sheets".into());
            }
            for tag in &unlisted_tags {
                choices.push(format!("They are drawn as '{tag}'"));
            }
            choices.push("There are uncounted devices".into());
            let qid = job.next_qid(StageName::Devices);
            let q = Question {
                id: qid,
                stage: StageName::Devices,
                question: format!("I counted {count} {class_label}. Are there any {class_label} I did not count?",),
                why: format!("Completeness checkpoint for {class_label}"),
                choices,
                evidence: Some(QuestionEvidence {
                    crop_png_path: None,
                    sheet: None,
                    at: None,
                    values: Some(json!({
                        "class": class_key,
                        "count": count,
                        "legend_entry": census.legend_types.contains(*class_key),
                    })),
                    handles: Vec::new(),
                    // MUST 9: tiled_crops with every counted item boxed.
                    tiled_crops: devices
                        .get("tiled_crops")
                        .and_then(|tc| tc.get(*class_key))
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()),
                    // MUST 9: leftover clusters that might be uncounted.
                    leftovers: devices.get("unknowns").and_then(Value::as_array).map_or_else(Vec::new, |unknowns| {
                        unknowns
                            .iter()
                            .take(10)
                            .map(|u| LeftoverCandidate {
                                description: u.get("description").and_then(Value::as_str).unwrap_or("?").to_string(),
                                crop_path: u.get("crop_path").and_then(Value::as_str).map(String::from),
                                at: u.get("at").and_then(|a| {
                                    let arr = a.as_array()?;
                                    Some([arr.first()?.as_f64()?, arr.get(1)?.as_f64()?])
                                }),
                                sheet: u.get("sheet").and_then(Value::as_str).map(String::from),
                                count: u.get("count").and_then(Value::as_u64).unwrap_or(1) as usize,
                            })
                            .collect()
                    }),
                }),
                default: Some("Count is complete".into()),
                answer_schema: None,
                answer: None,
                answered_by: None,
                answered_at: None,
                kind: QuestionKind::Completeness,
            };
            job.add_question_to_worklist(&q);
            if let Some(st) = job.stage_mut(StageName::Devices) {
                st.completeness.push(q);
            }
        }

        // MUST 10: Unknown/foreign symbols — ask to learn them, with choices from the legend.
        // Q6021 MUST 2: never create a question with empty text; skip symbols with no tag.
        if let Some(unknowns) = devices.get("unknowns").and_then(Value::as_array) {
            for unk in unknowns.iter().take(MAX_QUESTIONS_PER_STAGE) {
                let desc = unk.get("description").and_then(Value::as_str).unwrap_or("");
                // Q6021 MUST 2: skip if the description/tag is empty or whitespace-only.
                if desc.trim().is_empty() {
                    continue;
                }
                let count = unk.get("count").and_then(Value::as_u64).unwrap_or(1);
                let tagged = unk.get("tag").and_then(Value::as_str).is_some_and(|t| !t.trim().is_empty());
                let sheet = unk.get("sheet").and_then(Value::as_str).map(String::from);
                // Build choices: legend device types (near-matching first, then the rest),
                // plus non-device and skip.
                let mut choices: Vec<String> = Vec::new();
                let desc_lower = desc.to_ascii_lowercase();
                // Near-matches first: types whose name shares a prefix or substring with the tag.
                let mut near: Vec<&String> = Vec::new();
                let mut rest: Vec<&String> = Vec::new();
                for lt in &census.legend_types {
                    let lt_lower = lt.to_ascii_lowercase();
                    if lt_lower.contains(&desc_lower) || desc_lower.contains(&lt_lower) {
                        near.push(lt);
                    } else {
                        rest.push(lt);
                    }
                }
                for lt in near.iter().take(8) {
                    choices.push(lt.to_string());
                }
                for lt in rest.iter().take(12usize.saturating_sub(choices.len())) {
                    choices.push(lt.to_string());
                }
                choices.push("a window/door/room/equipment tag (not a device)".into());
                choices.push("skip".into());
                let qid = job.next_qid(StageName::Devices);
                let q = Question {
                    id: qid,
                    stage: StageName::Devices,
                    question: if tagged {
                        format!("Unknown symbol tag '{desc}' printed by {count} symbols, but no legend or schedule lists it — which device is it?")
                    } else {
                        format!("Unknown symbol: '{desc}' found {count} time(s) — which device is it?")
                    },
                    why: if tagged {
                        "Device types come from the legend and schedules; answering adds this tag's symbols to the count".into()
                    } else {
                        "Foreign/unknown symbol not matched to any library entry; learning it adds to future counts".into()
                    },
                    choices,
                    evidence: Some(QuestionEvidence {
                        crop_png_path: unk.get("crop_path").and_then(Value::as_str).map(String::from),
                        // Q6021 MUST 2: always include the sheet reference.
                        sheet: sheet.clone(),
                        at: unk.get("at").and_then(|a| {
                            let arr = a.as_array()?;
                            Some([arr.first()?.as_f64()?, arr.get(1)?.as_f64()?])
                        }),
                        values: Some(json!({"description": desc, "count": count, "sheet": sheet})),
                        handles: unk
                            .get("handles")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect())
                            .unwrap_or_default(),
                        tiled_crops: None,
                        leftovers: Vec::new(),
                    }),
                    default: Some("skip".into()),
                    answer_schema: Some(
                        json!({"type": "object", "properties": {"device_type": {"type": "string"}, "symbol_name": {"type": "string"}}}),
                    ),
                    answer: None,
                    answered_by: None,
                    answered_at: None,
                    kind: QuestionKind::Completeness,
                };
                job.add_question_to_worklist(&q);
                if let Some(st) = job.stage_mut(StageName::Devices) {
                    st.completeness.push(q);
                }
            }
        }
    }

    // Verify samples for detected devices.
    if let Some(devices) = results.get("devices")
        && let Some(items) = devices.get("items").and_then(Value::as_array)
    {
        for item in items.iter().take(MAX_VERIFY_SAMPLES) {
            let dev_type = item.get("type").and_then(Value::as_str).unwrap_or("?");
            let tag = item.get("tag").and_then(Value::as_str).unwrap_or("?");
            let qid = job.next_qid(StageName::Devices);
            let q = Question {
                id: qid,
                stage: StageName::Devices,
                question: format!("Device '{tag}' detected as {dev_type} — type right?"),
                why: "Verify sample: spot-checking device detection".into(),
                choices: vec!["Correct".into(), "Wrong type".into(), "Not a device".into()],
                evidence: Some(QuestionEvidence {
                    crop_png_path: None,
                    sheet: None,
                    at: item.get("at").and_then(|a| {
                        let arr = a.as_array()?;
                        Some([arr.first()?.as_f64()?, arr.get(1)?.as_f64()?])
                    }),
                    values: Some(item.clone()),
                    handles: Vec::new(),
                    tiled_crops: None,
                    leftovers: Vec::new(),
                }),
                default: Some("Correct".into()),
                answer_schema: None,
                answer: None,
                answered_by: None,
                answered_at: None,
                kind: QuestionKind::Verify,
            };
            if let Some(st) = job.stage_mut(StageName::Devices) {
                st.verify.push(q);
            }
        }
    }

    job.audit(
        "stage_run",
        "pipeline",
        None,
        Some(StageName::Devices),
        Some(json!({"total_items": total_items, "reader_errors": reader_errors.len()})),
    );
    Ok(results)
}

/// The job's imported sheets for the print-set readers: where each sits, the handles its
/// import made, and its discipline, type and printed scale from classify.
pub(crate) fn job_sheet_refs(job: &GuidedJob) -> Vec<super::printmep::SheetRef> {
    let classified: Vec<Value> = job.sheets.as_ref().and_then(|v| v.get("sheets")).and_then(Value::as_array).cloned().unwrap_or_default();
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut used = vec![false; job.sheet_spans.len()];
    let mut prev: Option<(String, [f64; 4], Option<(u64, u64)>)> = None;
    job.sheet_regions
        .iter()
        .map(|r| {
            let base = |n: &str| n.split('/').next().unwrap_or(n).trim().to_string();
            let entry = classified.iter().find(|c| base(&text(c, "number")).eq_ignore_ascii_case(&r.number));
            let discipline = entry
                .map(|c| text(c, "discipline"))
                .filter(|d| !d.is_empty())
                .unwrap_or_else(|| cadaddy_mepfix::sheet_facts::discipline_of(&r.number));
            let kind = entry.map(|c| {
                let t = text(c, "type");
                if t.is_empty() { text(c, "sheet_type") } else { t }
            });
            // The views of a several-view sheet share its import's objects.
            let same_page = r.view.is_some() && prev.as_ref().is_some_and(|p| p.0 == r.number && p.1 == r.rect);
            let span = if same_page {
                prev.as_ref().and_then(|(_, _, sp)| *sp)
            } else {
                let span = job
                    .sheet_spans
                    .iter()
                    .enumerate()
                    .find(|(i, sp)| !used.get(*i).copied().unwrap_or(true) && base(&sp.sheet).eq_ignore_ascii_case(&r.number));
                span.map(|(i, sp)| {
                    if let Some(u) = used.get_mut(i) {
                        *u = true;
                    }
                    (sp.lo, sp.hi)
                })
            };
            prev = Some((r.number.clone(), r.rect, span));
            super::printmep::SheetRef {
                number: r.number.clone(),
                level: r.level.clone(),
                discipline,
                kind: kind.filter(|k| !k.is_empty()).unwrap_or_else(|| "plan".into()),
                rect: r.rect,
                span,
                scale_text: entry.and_then(|c| c.get("scale")).and_then(Value::as_str).map(str::to_string),
                levels: r.levels.clone(),
                view: r.view.as_ref().map(|v| v.rect),
            }
        })
        .collect()
}

/// Drawing units per foot of the job's drawing: the stored spec's, else inches.
fn job_units_per_foot(s: &Session) -> f64 {
    match super::bspec::current(s, "mep_read") {
        Ok((sp, true)) if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 => sp.units_per_foot,
        _ => 12.0,
    }
}

/// Run the MEP read stage: ducts on each mechanical plan and pipes on each plumbing plan of
/// the set (never on the other sheets), each on its sheet's level; a per-trade summary and a
/// question for each plan that read nothing.
fn run_mep_read_stage(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    ensure_sheets_loaded(s, job)?;
    let refs = job_sheet_refs(job);
    let upf = job_units_per_foot(s);
    let read = super::printmep::read_sheets(s.doc()?, &refs, upf, None);
    // The electrical plans: drawn conduit, homeruns, panels, and an estimated route for each
    // circuit the devices print (Q6007).
    let cond = super::printcond::read_sheets(s.doc()?, &refs, upf, &job.devices);
    let mut read_sheets: Vec<String> = read.sheets.iter().map(|r| r.sheet.clone()).collect();
    read_sheets.extend(cond.sheets.iter().map(|r| r.sheet.clone()));
    let mut mep = read.mep.clone();
    for sy in &cond.mep.systems {
        if !mep.systems.iter().any(|m| m.name == sy.name) {
            mep.systems.push(sy.clone());
        }
    }
    mep.runs.extend(cond.mep.runs.iter().cloned());
    // A spec already stored (a re-run) takes the reading now; the BIM build puts it into the
    // spec it drafts in any case.
    if let Ok((mut sp, true)) = super::bspec::current(s, "mep_read") {
        super::printmep::merge_mep(&mut sp, mep.clone(), &read_sheets);
        super::printcond::seat_heights(&mut sp);
        super::mep::seat_in_plenum(&mut sp);
        super::printcond::store_circuits(&mut sp, &cond.circuits);
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    job.mep = Some(mep);
    job.circuits = cond.circuits.clone();
    if !cond.unrouted.is_empty() {
        job.add_finding(
            StageName::MepRead,
            &format!("{} circuit(s) have no panel on their level's electrical plans", cond.unrouted.len()),
            "their home runs to the panel are not counted: the panel's tag (PANEL LA, or its name by its box) was not found on the level's plans",
            2,
            vec!["Check the panel tags on the electrical plans".into()],
            None,
            false,
        );
    }
    if cond.no_circuit > 0 {
        job.add_finding(
            StageName::MepRead,
            &format!("{} device(s) on the electrical plans have no circuit printed beside them", cond.no_circuit),
            "no conduit or wire is estimated for a device whose circuit is not known",
            1,
            vec!["Check the circuit numbers beside the devices".into()],
            None,
            false,
        );
    }

    let duct_ft = read.length_ft("duct");
    let pipe_ft = read.length_ft("pipe");
    let (duct_runs, pipe_runs) = (read.runs("duct"), read.runs("pipe"));
    let terminals = read.mep.terminals.len();
    let equipment = read.mep.equipment.len();
    let m_sheets = read.sheets.iter().filter(|r| r.trade == "duct").count();
    let p_sheets = read.sheets.iter().filter(|r| r.trade == "pipe").count();
    let mut trade_summary = format!(
        "Found {duct_runs} duct run(s), {:.0} ft on {m_sheets} mechanical plan(s); {pipe_runs} pipe run(s), {:.0} ft on {p_sheets} plumbing plan(s); {terminals} terminal(s), {equipment} equipment",
        duct_ft, pipe_ft
    );
    if !cond.sheets.is_empty() {
        trade_summary.push_str(&format!(
            "; {} circuit(s) on {} electrical plan(s): {:.0} ft of conduit ({:.0} ft drawn, {:.0} ft {})",
            cond.circuits.len(),
            cond.sheets.len(),
            cond.drawn_ft() + cond.estimated_ft(),
            cond.drawn_ft(),
            cond.estimated_ft(),
            super::printcond::ESTIMATED
        ));
    }

    // Per-trade summary question: present the summary and ask if anything was missed.
    if duct_runs + pipe_runs + cond.circuits.len() + cond.mep.runs.len() > 0 {
        let qid = job.next_qid(StageName::MepRead);
        let q = Question {
            id: qid,
            stage: StageName::MepRead,
            question: format!("{trade_summary} — anything not counted?"),
            why: "per-trade summary of MEP elements found".into(),
            choices: vec!["Looks correct".into(), "Missing items".into()],
            evidence: Some(QuestionEvidence {
                crop_png_path: None,
                sheet: None,
                at: None,
                values: Some(read.to_json()),
                handles: Vec::new(),
                tiled_crops: None,
                leftovers: Vec::new(),
            }),
            default: Some("Looks correct".into()),
            answer_schema: None,
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: QuestionKind::Completeness,
        };
        if let Some(st) = job.stage_mut(StageName::MepRead) {
            st.questions.push(q);
        }
    }
    if m_sheets + p_sheets == 0 {
        job.add_finding(
            StageName::MepRead,
            "No mechanical or plumbing plans in the set",
            "ducts are read on M plan sheets and pipes on P plan sheets; classify found none",
            2,
            vec!["Check the sheet classification (printset.classify)".into()],
            None,
            false,
        );
    }

    // Each plan that read nothing: a question with the sheet.
    for r in read.sheets.iter().filter(|r| r.runs == 0).take(MAX_QUESTIONS_PER_STAGE) {
        let qid = job.next_qid(StageName::MepRead);
        let q = Question {
            id: qid,
            stage: StageName::MepRead,
            question: format!("{} ({}) read no {} runs — is this expected?", r.sheet, r.level, r.trade),
            why: "zero MEP results on a sheet that should have them".into(),
            choices: vec!["Yes, expected (schedule/detail sheet)".into(), "No, re-check this sheet".into()],
            evidence: Some(QuestionEvidence {
                crop_png_path: None,
                sheet: Some(r.sheet.clone()),
                at: None,
                values: Some(r.to_json()),
                handles: Vec::new(),
                tiled_crops: None,
                leftovers: Vec::new(),
            }),
            default: Some("Yes, expected (schedule/detail sheet)".into()),
            answer_schema: None,
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: QuestionKind::Doubt,
        };
        if let Some(st) = job.stage_mut(StageName::MepRead) {
            st.questions.push(q);
        }
    }
    job.audit(
        "stage_run",
        "pipeline",
        None,
        Some(StageName::MepRead),
        Some(json!({"duct_runs": duct_runs, "pipe_runs": pipe_runs, "sheets": read_sheets})),
    );

    let mut out = read.to_json();
    out["runs"] = json!(duct_runs + pipe_runs);
    out["length_ft"] = json!(super::printmep::r1(duct_ft + pipe_ft));
    out["fittings"] = json!(0);
    out["conduit"] = cond.to_json();
    out["trade_summary"] = json!(trade_summary);
    Ok(out)
}

/// Run the BIM build stage (MUST 1).
/// Must draft/store the building spec itself (bspec.draft store) before building.
/// One `job N/M bim_build: <step> done in X s` line per sub-step of the build, so a slow step
/// shows where it is (Q6033: on Windows the stage ran for an hour without a line).
fn bim_step(s: &mut Session, job: &GuidedJob, step: &str, started: web_time::Instant) {
    let secs = started.elapsed().as_secs_f64();
    let line = format!("{}: {step} done in {secs:.1} s", progress_tag(job, StageName::BimBuild));
    report_progress(s, json!({"stage": "bim_build", "step": step, "elapsed_s": secs}), &line);
}

fn run_bim_build(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    // A general note's panelboard height to top (`PANELBOARDS 6'-6" AFF TO TOP`), for the panels
    // once their properties are filled.
    let mut panel_top: Option<(f64, String)> = None;
    // Where a sheet point goes on its level's plan: made once (its view fits are the slow part)
    // for the MEP and the devices both.
    let mut placer: Option<Box<dyn Fn([f64; 2], &str) -> Option<cadaddy_mepfix::tagid::Sim>>> = None;
    // The building's shell first: each storey's outline from its exterior walls, before anything
    // is placed in it (rooms, MEP and devices go inside it).
    let mut shells: Option<Vec<(String, Vec<[cadaddy_geom::Vec2; 2]>)>> = None;
    // Then the corridors, stairs and elevator hoistways inside it, before the rooms.
    let mut circulation: Vec<cadaddy_walk3d::Circulation> = Vec::new();
    // Ensure sheet content is in the drawing so bspec.draft can read geometry.
    let t = web_time::Instant::now();
    ensure_sheets_loaded(s, job)?;
    bim_step(s, job, "sheets loaded", t);
    let t = web_time::Instant::now();
    // MUST 1: Draft the spec first — bim.build needs it. Without the spec, it errors.
    // With architectural floor plans, the building is theirs: one level per plan (its sheet's
    // rectangle), stacked; the other disciplines' plans add no walls.
    let levels = building_levels(job);
    let draft_params = if levels.is_empty() { json!({"store": true}) } else { json!({"store": true, "levels": levels, "stack": true}) };
    // The circuits whose estimated routes were all in typical-room views (their tags not stored).
    let mut room_view_circuits: BTreeSet<String> = BTreeSet::new();
    let draft_result = s.execute("bspec.draft", &draft_params);
    match &draft_result {
        Ok(spec) => {
            // Check for guesses that should be flagged.
            if let Some(guesses) = spec.get("guesses").and_then(Value::as_array) {
                for g in guesses.iter().take(MAX_QUESTIONS_PER_STAGE) {
                    let key = g.get("key").and_then(Value::as_str).unwrap_or("?");
                    let why = g.get("why").and_then(Value::as_str).unwrap_or("unknown reason");
                    let qid = job.next_qid(StageName::BimBuild);
                    let q = Question {
                        id: qid,
                        stage: StageName::BimBuild,
                        question: format!("Building spec guessed {key} ({why}) — accept?"),
                        why: "The building spec was drafted from plan features; some parts are guesses".into(),
                        choices: vec!["Accept guess".into(), "Provide correct value".into()],
                        evidence: Some(QuestionEvidence {
                            crop_png_path: None,
                            sheet: None,
                            at: None,
                            values: Some(g.clone()),
                            handles: Vec::new(),
                            tiled_crops: None,
                            leftovers: Vec::new(),
                        }),
                        default: Some("Accept guess".into()),
                        answer_schema: None,
                        answer: None,
                        answered_by: None,
                        answered_at: None,
                        kind: QuestionKind::Doubt,
                    };
                    if let Some(st) = job.stage_mut(StageName::BimBuild) {
                        st.questions.push(q);
                    }
                }
            }
            job.audit("bspec_draft", "pipeline", None, Some(StageName::BimBuild), Some(json!({"stored": true})));
            // The drafted spec is the plans' alone: the M and P plans' runs go back in, each on
            // its level, so the build makes them.
            if let Some(mep) = job.mep.clone().filter(|m| !m.runs.is_empty() || !m.terminals.is_empty() || !m.equipment.is_empty())
                && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
            {
                let runs = mep.runs.len();
                super::printmep::merge_mep(&mut sp, mep, &[]);
                // The circuits' estimated routes just above each level's ceiling; the runs the
                // drawings give no height in their level's plenum, under the floor above.
                super::printcond::seat_heights(&mut sp);
                let seated = super::mep::seat_in_plenum(&mut sp);
                if seated > 0 {
                    progress_line(&format!("  MEP runs with no height of their own seated in their level's plenum: {seated}"));
                }
                s.doc_mut()?.building_spec = Some(sp.to_json());
                job.audit("mep_merged", "pipeline", None, Some(StageName::BimBuild), Some(json!({"runs": runs})));
            }
            // The rooms stage's rooms go in too: the drafted spec had none, so the build made no
            // room elements (and every later room read traced the whole print set again).
            let read: Vec<Value> = job
                .stage(StageName::Rooms)
                .and_then(|st| st.result.as_ref())
                .and_then(|r| r.get("rooms"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            // How many rooms of a level carry each name: a name several carry is a unit type
            // (every DOUBLE Q of the floor), one a single room carries is that room's (LOBBY).
            let mut named: BTreeMap<(String, String), usize> = BTreeMap::new();
            for r in &read {
                let level = r.get("level").and_then(Value::as_str).unwrap_or("").to_string();
                let name = r.get("name").and_then(Value::as_str).unwrap_or("").trim().to_ascii_uppercase();
                *named.entry((level, name)).or_default() += 1;
            }
            let found: Vec<cadaddy_walk3d::SpecRoom> = read
                .iter()
                .filter_map(|r| {
                    let name = r.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
                    let number = r.get("number").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).map(String::from);
                    // A numbered room with a unit type for a name goes by its number alone: a type
                    // label would claim the first room so named. The traced room takes its name
                    // from the labels in its outline.
                    let level = r.get("level").and_then(Value::as_str).unwrap_or("").to_string();
                    let type_name = named.get(&(level, name.to_ascii_uppercase())).is_some_and(|n| *n > 1);
                    let name = if number.is_some() && type_name { String::new() } else { name };
                    (!name.is_empty() || number.is_some()).then(|| cadaddy_walk3d::SpecRoom { name, number, ..Default::default() })
                })
                .collect();
            if !found.is_empty()
                && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
            {
                for r in found {
                    if !sp.rooms.iter().any(|x| x.name == r.name && x.number == r.number) {
                        sp.rooms.push(r);
                    }
                }
                s.doc_mut()?.building_spec = Some(sp.to_json());
            }
            // Each level's region is its architectural floor plan's view: the draft guessed the
            // upper floors' regions from stray objects (slivers), so their walls weren't built.
            let arch_plans: Vec<(String, [f64; 4])> = job
                .sheet_regions
                .iter()
                .filter(|r| r.kind == "plan" && r.not_built.is_empty() && r.discipline.eq_ignore_ascii_case("A"))
                .map(|r| (r.level.clone(), r.plan_rect()))
                .collect();
            if !arch_plans.is_empty()
                && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
            {
                let key = |l: &str| {
                    let l = l.trim().to_ascii_uppercase();
                    let digits: String = l.chars().filter(char::is_ascii_digit).collect();
                    if digits.is_empty() { l.trim_start_matches("LEVEL").trim().chars().take(1).collect() } else { digits }
                };
                for level in &mut sp.levels {
                    let want = key(&level.name);
                    let rects: Vec<[f64; 4]> = arch_plans.iter().filter(|(l, _)| !l.is_empty() && key(l) == want).map(|(_, r)| *r).collect();
                    // The largest view of the level (a several-view sheet's floor plan, not its enlarged plans).
                    if let Some(r) = rects
                        .into_iter()
                        .max_by(|a, b| ((a[2] - a[0]).abs() * (a[3] - a[1]).abs()).total_cmp(&((b[2] - b[0]).abs() * (b[3] - b[1]).abs())))
                    {
                        progress_line(&format!("  level {}: region from its architectural plan {:?}", level.name, r.map(|v| v.round())));
                        level.region = Some(r);
                    }
                }
                // A level without a floor plan (the roof) or with a placeholder region takes the
                // region of the level below: a far-off placeholder put its devices a billion units
                // away and the whole-building view framed a dot.
                let mut below: Option<[f64; 4]> = None;
                for level in &mut sp.levels {
                    let placeholder = level.region.is_none_or(|r| r.iter().any(|v| !v.is_finite() || v.abs() > 1.0e8));
                    if placeholder && let Some(r) = below {
                        progress_line(&format!("  level {}: no plan of its own, uses the region below", level.name));
                        level.region = Some(r);
                    }
                    below = level.region.filter(|r| r.iter().all(|v| v.is_finite() && v.abs() <= 1.0e8)).or(below);
                }
                s.doc_mut()?.building_spec = Some(sp.to_json());
            }
            // Where each sheet of the set sits, so what lies on the other disciplines' sheets
            // is theirs (not "outside every level's region").
            if !job.sheet_regions.is_empty()
                && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
            {
                add_sheet_links(&mut sp, &job.sheet_regions);
                s.doc_mut()?.building_spec = Some(sp.to_json());
            }
            // Runs whose size wasn't read: one larger than any size the system's tags give on its
            // level is two unrelated lines measured; it takes the system's usual tagged size. And a
            // run drawn over the same segment again (traced back and forth) is one run.
            if let Ok((mut sp, true)) = super::bspec::current(s, "bim_build") {
                let upf = sp.units_per_foot;
                let ducts: BTreeSet<String> =
                    sp.mep.systems.iter().filter(|y| y.kind == cadaddy_walk3d::MepKind::Duct).map(|y| y.name.clone()).collect();
                let (sized, dropped) = tidy_runs(&mut sp.mep.runs, upf, &ducts);
                if sized + dropped > 0 {
                    progress_line(&format!("  MEP runs tidied: {sized} unread sizes set from the tags, {dropped} repeated runs dropped"));
                    s.doc_mut()?.building_spec = Some(sp.to_json());
                }
            }
            // The MEP plans' runs, terminals and equipment were read in their own sheet's place
            // (the sheets sit side by side): each is put onto its level's architectural plan by
            // its sheet's registration (shared door and room numbers), so it is in the building.
            // With no architectural plan in the set there is nothing to place onto: the items stay
            // where their sheets draw them.
            let has_arch = job.sheet_regions.iter().any(|r| r.kind == "plan" && r.discipline.eq_ignore_ascii_case("A"));
            if has_arch && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build") {
                let shell = shells
                    .get_or_insert_with(|| {
                        let (shells, found) = building_shells(s, &sp, job);
                        circulation = found;
                        shells
                    })
                    .clone();
                let sim = placer.insert(Box::new(arch_placer(s.doc()?, &sp, &shell, enlarged_fits(job))));
                // Architecture is the rule: what can't be put onto the plans isn't drawn (a finding).
                let before = (sp.mep.runs.len(), sp.mep.terminals.len(), sp.mep.equipment.len());
                // Which sheets' runs can't be placed (both ends, or one end off the plan), and how
                // much of them: where a registration is missing.
                let upf_runs = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
                let mut unplaced: BTreeMap<String, (usize, f64)> = BTreeMap::new();
                // A circuit's estimated route drawn in a view of rooms (a typical-room plan) that
                // can't be placed: each room's own circuits go in instead (with the devices
                // stamped into the rooms), so it isn't a missing registration.
                let estimated = |r: &cadaddy_walk3d::MepRun| r.system == super::printcond::CIRCUIT_SYSTEM;
                let lost: BTreeSet<String> = sp
                    .mep
                    .runs
                    .iter()
                    .filter(|r| estimated(r) && r.points.first().is_none_or(|p| sim(*p, &r.level).is_none()))
                    .map(|r| sheet_key(&r.source_sheet))
                    .collect();
                let views = if lost.is_empty() { BTreeMap::new() } else { room_view_regions(s.doc()?, job, &lost) };
                let mut in_views = 0usize;
                // The runs no registration places, with what the report says of them: a typical
                // room's plan's go into its rooms below.
                let mut lost_runs: Vec<(String, f64, cadaddy_walk3d::MepRun)> = Vec::new();
                sp.mep.runs.retain_mut(|r| {
                    let start = r.points.first().and_then(|p| sim(*p, &r.level));
                    let end = start.and_then(|_| r.points.last().and_then(|p| sim(*p, &r.level)));
                    match (start, end) {
                        (Some(t), Some(_)) => {
                            r.points.iter_mut().for_each(|p| *p = t.apply(*p));
                            true
                        }
                        _ if in_room_view(r, &views) => {
                            in_views += 1;
                            if let Some(c) = super::printcond::circuit_of_run(r) {
                                room_view_circuits.insert(c.to_string());
                            }
                            false
                        }
                        _ => {
                            let why = if start.is_some() { " one end" } else { "" };
                            let ft = r.points.windows(2).map(|w| (w[1][0] - w[0][0]).hypot(w[1][1] - w[0][1])).sum::<f64>() / upf_runs;
                            let key = format!("{} on {}{why}", if r.source_sheet.is_empty() { "?" } else { &r.source_sheet }, r.level);
                            if lost_runs.len() < MAX_STAMPED {
                                lost_runs.push((key, ft, r.clone()));
                            } else {
                                let e = unplaced.entry(key).or_default();
                                e.0 += 1;
                                e.1 += ft;
                            }
                            false
                        }
                    }
                });
                // A typical room's plan's pipes and ducts (ENLARGED GUESTROOM - SANITARY): into each
                // room of its type, as its devices are.
                let lost: Vec<cadaddy_walk3d::MepRun> = lost_runs.iter().map(|(_, _, r)| r.clone()).collect();
                let typical = if lost.is_empty() { StampedRuns::default() } else { stamp_typical_runs(s.doc()?, job, &sp.mep.runs, &lost) };
                for ((key, ft, _), taken) in lost_runs.iter().zip(typical.taken.iter().copied().chain(std::iter::repeat(false))) {
                    if !taken {
                        let e = unplaced.entry(key.clone()).or_default();
                        e.0 += 1;
                        e.1 += ft;
                    }
                }
                if !typical.runs.is_empty() {
                    progress_line(&format!(
                        "  MEP runs of typical-room plans stamped into the rooms of their type: {} from {} runs ({:?})",
                        typical.runs.len(),
                        typical.taken.iter().filter(|t| **t).count(),
                        typical.report
                    ));
                    sp.mep.runs.extend(typical.runs);
                }
                if !unplaced.is_empty() {
                    let list: Vec<String> = unplaced.iter().map(|(k, (n, ft))| format!("{k}: {n} ({ft:.0} ft)")).collect();
                    progress_line(&format!("  MEP runs not placeable by sheet: {}", list.join(", ")));
                }
                if in_views > 0 {
                    progress_line(&format!("  estimated circuit routes in typical-room views (each room's own circuits instead): {in_views}"));
                }
                // A circuit some of whose route was placed keeps its tag.
                for r in &sp.mep.runs {
                    if let Some(c) = super::printcond::circuit_of_run(r) {
                        room_view_circuits.remove(c);
                    }
                }
                sp.mep.terminals.retain_mut(|t| match sim(t.at, &t.level) {
                    Some(m) => {
                        t.at = m.apply(t.at);
                        true
                    }
                    None => false,
                });
                sp.mep.equipment.retain_mut(|e| match sim(e.min, &e.level) {
                    Some(m) => {
                        let (a, b) = (m.apply(e.min), m.apply(e.max));
                        e.min = [a[0].min(b[0]), a[1].min(b[1])];
                        e.max = [a[0].max(b[0]), a[1].max(b[1])];
                        true
                    }
                    None => false,
                });
                let dropped = (before.0 - sp.mep.runs.len() - in_views, before.1 - sp.mep.terminals.len(), before.2 - sp.mep.equipment.len());
                progress_line(&format!(
                    "  MEP onto the plans: {} runs, {} terminals, {} equipment placed; not placeable (no fit to the architecture): {} runs, {} terminals, {} equipment",
                    sp.mep.runs.len(),
                    sp.mep.terminals.len(),
                    sp.mep.equipment.len(),
                    dropped.0,
                    dropped.1,
                    dropped.2
                ));
                // An elevator hoistway is closed floor to roof: a duct, pipe or conduit through
                // one is a clash.
                let through = runs_through_hoistways(&sp.mep.runs, &circulation, upf_runs);
                if !through.is_empty() {
                    let mut by: BTreeMap<String, usize> = BTreeMap::new();
                    for r in through.iter().filter_map(|i| sp.mep.runs.get(*i)) {
                        *by.entry(format!("{} on {}", r.system, r.level)).or_default() += 1;
                    }
                    progress_line(&format!("  MEP runs through an elevator hoistway: {} {by:?}", through.len()));
                    job.add_finding(
                        StageName::BimBuild,
                        "MEP through an elevator hoistway",
                        &format!(
                            "{} duct, pipe or conduit runs pass through an elevator shaft, which nothing may run through: {by:?}",
                            through.len()
                        ),
                        3,
                        vec!["Route them round the shaft".into()],
                        None,
                        false,
                    );
                }
                // The reflected ceiling plans set each area's ceiling (a lobby higher, back of
                // house lower): read now the floor's spaces are known, then each run with no
                // height of its own sits over the highest ceiling it passes under, and each
                // diffuser is in its area's ceiling.
                sp.circulation = circulation.clone();
                let rcp_sheets: Vec<[f64; 4]> = job.sheet_regions.iter().filter(|r| r.kind == "rcp").map(|r| r.rect).collect();
                let (mut areas, read) = rcp_ceiling_areas(s.doc()?, &sp, &rcp_sheets);
                for line in &read {
                    progress_line(&format!("  RCP {line}"));
                }
                // And the typical rooms' own ceiling plans, into every room of their type.
                let (typical, tags) = typical_rcp_areas(s.doc()?, &sp, job);
                if tags > 0 {
                    progress_line(&format!("  RCP typical rooms: {tags} height tags read, {} areas set", typical.len()));
                }
                areas.extend(typical);
                if !areas.is_empty() {
                    // Each area's room takes its height (its ceiling is built there; the RCP's areas
                    // are not the level's whole ceiling, so the rooms' ceilings stay): the spec
                    // room of its number, or of its circulation space's name.
                    let mut rooms_set = 0usize;
                    for a in &areas {
                        let Some(label) = a.room.as_deref().map(str::trim).filter(|l| !l.is_empty()) else { continue };
                        for r in sp.rooms.iter_mut().filter(|r| {
                            r.number.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(label)) || r.name.trim().eq_ignore_ascii_case(label)
                        }) {
                            r.ceiling_ft = Some(a.height_ft);
                            r.ceiling_source.clone_from(&a.source);
                            rooms_set += 1;
                        }
                    }
                    let seated = super::mep::seat_in_plenum_under(&mut sp, &areas);
                    let levels: BTreeSet<&str> = areas.iter().map(|a| a.level.as_str()).collect();
                    progress_line(&format!(
                        "  ceilings set per area from the RCPs: {} areas on {levels:?}, {rooms_set} rooms at their height; {seated} MEP runs and terminals reseated under them",
                        areas.len()
                    ));
                }
                // A typical room whose ceiling plan prints no height: its elevations' (measured).
                let measured = typical_elevation_heights(s.doc()?, job);
                if !measured.is_empty() {
                    let mut set = 0usize;
                    // A room's type: the rooms stage's room of its number, else its own name.
                    let by_number: BTreeMap<String, BTreeSet<String>> = job_rooms(job)
                        .iter()
                        .filter(|(_, _, _, n)| !n.trim().is_empty())
                        .map(|(name, _, _, n)| (n.trim().to_ascii_uppercase(), type_words(name, None)))
                        .collect();
                    for r in sp.rooms.iter_mut().filter(|r| r.ceiling_ft.is_none()) {
                        let words = r
                            .number
                            .as_deref()
                            .and_then(|n| by_number.get(&n.trim().to_ascii_uppercase()))
                            .cloned()
                            .unwrap_or_else(|| type_words(&r.name, None));
                        if let Some((_, ft, source)) = measured.iter().find(|(w, _, _)| *w == words) {
                            r.ceiling_ft = Some(*ft);
                            r.ceiling_source = Some(source.clone());
                            set += 1;
                        }
                    }
                    let said: Vec<String> = measured
                        .iter()
                        .map(|(w, ft, _)| format!("{} {}", w.iter().cloned().collect::<Vec<_>>().join(" "), ft_in_label(*ft)))
                        .collect();
                    progress_line(&format!("  ceilings measured on the typical rooms' elevations: {said:?}; {set} rooms take theirs"));
                }
                // The typical rooms' mechanical views: each room's PTAC and exhaust fan (with its
                // duct), in their rooms' ceilings now those are known. A PTAC's face grille read as
                // a return grille isn't one.
                let mech = stamp_typical_mech(s.doc()?, &sp, job);
                if !mech.equipment.is_empty() {
                    let ptacs: Vec<([f64; 2], [f64; 2], String)> = sp
                        .mep
                        .equipment
                        .iter()
                        .chain(mech.equipment.iter())
                        .filter(|e| e.kind == "ptac")
                        .map(|e| (e.min, e.max, e.level.clone()))
                        .collect();
                    let reach = PTAC_GRILLE_FT * upf_runs;
                    let before = sp.mep.terminals.len();
                    sp.mep.terminals.retain(|t| {
                        !(t.terminal_kind == "return_grille"
                            && ptacs.iter().any(|(lo, hi, lv)| {
                                *lv == t.level
                                    && t.at[0] >= lo[0] - reach
                                    && t.at[0] <= hi[0] + reach
                                    && t.at[1] >= lo[1] - reach
                                    && t.at[1] <= hi[1] + reach
                            }))
                    });
                    let mut by: BTreeMap<(String, String), usize> = BTreeMap::new();
                    for e in &mech.equipment {
                        *by.entry((e.level.clone(), e.kind.clone())).or_default() += 1;
                    }
                    progress_line(&format!(
                        "  typical rooms' mechanical: {} equipment, {} exhaust ducts stamped {by:?}; {} PTAC faces read as return grilles left out ({:?})",
                        mech.equipment.len(),
                        mech.runs.len(),
                        before - sp.mep.terminals.len(),
                        mech.report
                    ));
                    sp.mep.equipment.extend(mech.equipment);
                    sp.mep.runs.extend(mech.runs);
                }
                // Nothing more than 5' outside its storey's shell: past it, it isn't in the
                // building (site work, or a reading put in the wrong place). Out, and said.
                let before_out = (sp.mep.runs.len(), sp.mep.terminals.len(), sp.mep.equipment.len());
                sp.mep.runs.retain(|r| !r.points.iter().any(|p| outside_shell(&shell, &r.level, *p, upf_runs)));
                sp.mep.terminals.retain(|t| !outside_shell(&shell, &t.level, t.at, upf_runs));
                sp.mep.equipment.retain(|e| !outside_shell(&shell, &e.level, e.min, upf_runs) && !outside_shell(&shell, &e.level, e.max, upf_runs));
                let out = (before_out.0 - sp.mep.runs.len(), before_out.1 - sp.mep.terminals.len(), before_out.2 - sp.mep.equipment.len());
                if out.0 + out.1 + out.2 > 0 {
                    progress_line(&format!(
                        "  MEP outside the building (more than {OUTSIDE_SHELL_FT}' past its storey's shell), left out: {} runs, {} terminals, {} equipment",
                        out.0, out.1, out.2
                    ));
                    job.add_finding(
                        StageName::BimBuild,
                        "MEP outside the building",
                        &format!(
                            "{} runs, {} terminals and {} pieces of equipment land more than {OUTSIDE_SHELL_FT} ft outside their storey's walls (site work, or a sheet placed wrong); they are not in the model",
                            out.0, out.1, out.2
                        ),
                        2,
                        vec!["Check those sheets' registration onto the plans".into()],
                        None,
                        false,
                    );
                }
                if dropped.0 + dropped.1 + dropped.2 > 0 {
                    job.add_finding(
                        StageName::BimBuild,
                        "MEP items not placed",
                        &format!("{} runs, {} terminals and {} pieces of equipment are on sheets that don't line up with an architectural plan; they are not drawn", dropped.0, dropped.1, dropped.2),
                        2,
                        vec!["Register those sheets onto their level's plan".into()],
                        None,
                        false,
                    );
                }
                s.doc_mut()?.building_spec = Some(sp.to_json());
            }
        }
        Err(e) => {
            // Spec draft failed — this is a finding, not a silent error.
            job.add_finding(
                StageName::BimBuild,
                &format!("Building spec draft failed: {e}"),
                "bspec.draft must succeed before bim.build can run",
                5,
                vec!["Check that rooms and levels were detected".into()],
                None,
                true,
            );
            return Err(bad("bim_build", format!("bspec.draft failed: {e}")));
        }
    }

    bim_step(s, job, "spec drafted (with MEP and sheet links)", t);
    let t = web_time::Instant::now();
    let mut placed: Option<Value> = None;
    // The devices stage's devices, on their levels (bspec.draft started a new spec), before the
    // build: it makes each one an element carrying what was read about it (Q6002).
    // The plumbing fixtures the plumbing plans tag (named by the set's fixture schedule) go in
    // with the devices: placed by their sheets the same way.
    let schedule = if job.sheet_spans.iter().any(|sp| cadaddy_mepfix::sheet_facts::discipline_of(&sp.sheet).eq_ignore_ascii_case("P")) {
        plumbing_schedule(s.doc()?, job)
    } else {
        None
    };
    let mut all_devices = job.devices.clone();
    if let Some(sch) = &schedule {
        let fixtures = plumbing_fixtures(s.doc()?, job, sch);
        progress_line(&format!("  plumbing fixture schedule: {} tags; fixtures the plumbing plans tag: {}", sch.rows.len(), fixtures.len()));
        all_devices.extend(fixtures);
    }
    if schedule.is_some() || job.sheet_spans.iter().any(|sp| cadaddy_mepfix::sheet_facts::discipline_of(&sp.sheet).eq_ignore_ascii_case("P")) {
        let stacks = plumbing_stacks(s.doc()?, job);
        if !stacks.is_empty() {
            progress_line(&format!("  waste and vent stacks the plumbing plans letter: {}", stacks.len()));
            all_devices.extend(stacks);
        }
    }
    if !all_devices.is_empty() {
        // Onto the architecture like the MEP: a device that can't be placed is left out.
        let has_arch = job.sheet_regions.iter().any(|r| r.kind == "plan" && r.discipline.eq_ignore_ascii_case("A"));
        // (Not placed onto the architecture, stamped from typical-room plans.)
        let mut counts = (0usize, 0usize);
        // The stamped rooms' panels (where a typical room's plan letters them), for their circuits.
        let mut room_panels: BTreeMap<String, Vec<(String, [f64; 2])>> = BTreeMap::new();
        let mut panel_rooms: BTreeMap<String, (String, Vec<cadaddy_geom::Vec2>, bool)> = BTreeMap::new();
        let mut sleeping: Vec<(String, Vec<cadaddy_geom::Vec2>)> = Vec::new();
        let devices: Vec<super::readers::ReadDevice> = match super::bspec::current(s, "bim_build") {
            Ok((sp, true)) if has_arch => {
                if placer.is_none() {
                    let shell = shells
                        .get_or_insert_with(|| {
                            let (shells, found) = building_shells(s, &sp, job);
                            circulation = found;
                            shells
                        })
                        .clone();
                    placer = Some(Box::new(arch_placer(s.doc()?, &sp, &shell, enlarged_fits(job))));
                }
                let Some(sim_sheet) = placer.as_deref() else { return Err(bad("bim_build", "device placer missing".to_string())) };
                // A device in a typical room's view (or one room's) is that view's: stamped into the
                // rooms, never put where its sheet's registration (another view's, the core plan
                // beside it) would take it.
                let device_sheets: BTreeSet<String> = all_devices.iter().map(|dv| sheet_key(&dv.sheet)).collect();
                let views = room_view_regions(s.doc()?, job, &device_sheets);
                let in_view = |dv: &super::readers::ReadDevice| {
                    views
                        .get(&sheet_key(&dv.sheet))
                        .is_some_and(|rs| rs.iter().any(|r| dv.at[0] >= r[0] && dv.at[0] <= r[2] && dv.at[1] >= r[1] && dv.at[1] <= r[3]))
                };
                let sim = |dv: &super::readers::ReadDevice| if in_view(dv) { None } else { sim_sheet(dv.at, &dv.level) };
                let out: Vec<_> = all_devices
                    .iter()
                    .filter_map(|dv| {
                        let m = sim(dv)?;
                        let mut dv = dv.clone();
                        dv.at = m.apply(dv.at);
                        Some(dv)
                    })
                    .collect();
                progress_line(&format!("  devices onto the plans: {} placed, {} not placeable", out.len(), all_devices.len() - out.len()));
                // Nothing more than 5' outside its storey's shell (as for the MEP).
                let upf_dev = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
                let placed_n = out.len();
                let out: Vec<_> = out.into_iter().filter(|dv| !outside_shell(shells.as_deref().unwrap_or(&[]), &dv.level, dv.at, upf_dev)).collect();
                if out.len() < placed_n {
                    let n = placed_n - out.len();
                    progress_line(&format!("  devices outside the building (more than {OUTSIDE_SHELL_FT}' past its storey's shell), left out: {n}"));
                    job.add_finding(
                        StageName::BimBuild,
                        "Devices outside the building",
                        &format!("{n} devices land more than {OUTSIDE_SHELL_FT} ft outside their storey's walls (site lighting, or a sheet placed wrong); they are not in the model"),
                        2,
                        vec!["Check those sheets' registration onto the plans".into()],
                        None,
                        false,
                    );
                }
                let mut why: BTreeMap<String, usize> = BTreeMap::new();
                for dv in all_devices.iter().filter(|dv| sim(dv).is_none()) {
                    *why.entry(format!(
                        "{} on {}",
                        if dv.sheet.is_empty() { "?" } else { &dv.sheet },
                        if dv.level.is_empty() { "?" } else { &dv.level }
                    ))
                    .or_default() += 1;
                }
                progress_line(&format!("  devices not placeable by sheet: {why:?}"));
                let unplaced: Vec<&super::readers::ReadDevice> = all_devices.iter().filter(|dv| sim(dv).is_none()).collect();
                let StampedRooms { devices: mut stamped, report, authority, panels, rooms, sleeping: guest, footprints, duplicates } =
                    stamp_typical_rooms(s.doc()?, job, &out, &unplaced);
                // One room's two copies of a tag at one spot are one device (its plan read it twice).
                let twice = one_per_spot(&mut stamped, upf_dev / 4.0);
                if twice > 0 {
                    progress_line(&format!("  typical-room devices read twice at one spot, one kept: {twice}"));
                }
                // The heights the typical rooms' plans print by their devices.
                let (notes, heights) = typical_heights(s.doc()?, job, &mut stamped);
                if notes > 0 {
                    progress_line(&format!("  typical rooms' height notes: {notes} read, {heights} stamped devices took theirs"));
                }
                // An electrical tag naming mechanical equipment (EF, PTAC) is its connection: at
                // the equipment the room's mechanical plan puts there, not a device of its own.
                let reach = CONNECT_REACH_FT * upf_dev;
                // Equipment in a unit room no plan connects (its feed only a keynote): its
                // connection on the circuit the room's panel schedule names it on.
                // (The panel schedules are read into the spec later in the build: read here too.)
                let schedules = if sp.mep.panel_schedules.is_empty() {
                    panel_schedules(&electrical_pages(s.doc()?, job))
                } else {
                    sp.mep.panel_schedules.clone()
                };
                let scheduled = scheduled_connections(&schedules, &sp.mep.equipment, &stamped);
                if !scheduled.is_empty() {
                    progress_line(&format!("  equipment connections from the unit panel's schedule: {}", scheduled.len()));
                    stamped.extend(scheduled);
                }
                let mut connected = connect_to_equipment(&sp.mep.equipment, &mut stamped, reach);
                room_panels = panels;
                panel_rooms = rooms;
                sleeping = guest;
                // A room an enlarged plan of its own shows: that plan's devices, not the overall
                // plan's copies of some of them.
                let mut out = out;
                // The floor plans' copies of devices a typical plan stamped where they are.
                if !duplicates.is_empty() {
                    let mut i = 0usize;
                    out.retain(|_| {
                        let keep = !duplicates.contains(&i);
                        i += 1;
                        keep
                    });
                    progress_line(&format!("  floor plans' devices a typical plan stamped at their spot (left out): {}", duplicates.len()));
                }
                let before = out.len();
                // A device of the plans in no room but in a unit's footprint (its bath, its entry:
                // the unit's label's room leaves them out) is that unit's.
                // (What a unit's own plans show: its stamped tags; and plumbing, which its plumbing
                // plans give. A corridor's sconce by the unit's door is the corridor's.)
                let mut own: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                // The panels the unit's own devices are on (its load center).
                let mut fed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                for dv in &stamped {
                    own.entry(dv.room.clone()).or_default().insert(format!("{}@{}", dv.tag.trim().to_ascii_uppercase(), units_trade(&dv.sheet)));
                    if let Some(p) = super::propfill::panel_of(&dv.circuit) {
                        fed.entry(dv.room.clone()).or_default().insert(p.to_ascii_uppercase());
                    }
                }
                let booked = book_into_units(&mut out, &footprints, &own, &fed, NEIGHBOUR_FIXTURE_FT * upf_dev);
                if booked > 0 {
                    progress_line(&format!("  devices booked to their unit by its plan's footprint (bath, entry): {booked}"));
                }
                connected += connect_to_equipment(&sp.mep.equipment, &mut out, reach);
                // One connection per piece of equipment: the electrical plan's and the mechanical
                // plan's tags of one fan, both put at it, are one.
                let doubled = one_connection_each(&mut stamped, &mut out, upf_dev);
                if doubled > 0 {
                    progress_line(&format!("  equipment connections two plans tag, one kept: {doubled}"));
                }
                // A fixture both an enlarged plumbing plan and the floor's plan tag is one fixture.
                let near = DUPLICATE_FIXTURE_FT * upf_dev;
                // (And one tag in one unit from two sheets, the enlarged plan's and the floor's,
                // is one fixture however far apart their registrations put it.)
                // Two plans' tags of one fixture (the sanitary plan's and the water plan's) on two
                // sheets register a little apart: within [`CROSS_SHEET_FIXTURE_FT`] they are one.
                // Of the two, the one in a unit that has no other of its tag stays (the neighbour's
                // water closet across the wall they share is not this unit's second).
                let across = CROSS_SHEET_FIXTURE_FT * upf_dev;
                let before_dupes = out.len();
                let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
                let mut kept: Vec<usize> = Vec::new();
                let mut gone: BTreeSet<usize> = BTreeSet::new();
                for i in 0..out.len() {
                    let Some(dv) = out.get(i) else { break };
                    if dv.kind != "plumbing fixture" && dv.kind != "plumbing stack" {
                        continue;
                    }
                    let same = |o: &super::readers::ReadDevice| {
                        let d = (o.at[0] - dv.at[0]).hypot(o.at[1] - dv.at[1]);
                        o.tag == dv.tag
                            && digits(&o.level) == digits(&dv.level)
                            && (d <= near
                                || (d <= across && o.sheet != dv.sheet)
                                || (!dv.room.is_empty() && o.room == dv.room && o.sheet != dv.sheet))
                    };
                    let Some(pos) = kept.iter().position(|&k| out.get(k).is_some_and(same)) else {
                        kept.push(i);
                        continue;
                    };
                    let k = kept.get(pos).copied().unwrap_or(i);
                    let others = |room: &str, but: usize| {
                        kept.iter().filter(|&&j| j != but && out.get(j).is_some_and(|o| o.room == room && o.tag == dv.tag)).count()
                    };
                    let theirs = out.get(k).map_or_else(String::new, |o| o.room.clone());
                    if !dv.room.is_empty() && dv.room != theirs && others(&theirs, k) >= 1 && others(&dv.room, k) == 0 {
                        gone.insert(k);
                        if let Some(slot) = kept.get_mut(pos) {
                            *slot = i;
                        }
                    } else {
                        gone.insert(i);
                    }
                }
                let mut i = 0usize;
                out.retain(|_| {
                    let keep = !gone.contains(&i);
                    i += 1;
                    keep
                });
                if out.len() < before_dupes {
                    progress_line(&format!("  plumbing fixtures tagged on two plans, one kept: {}", before_dupes - out.len()));
                }
                // The guest rooms the plumbing plans don't show take the fixture set of a room of
                // their type that the plans do.
                let (fixture_copies, fixture_report) = copy_room_fixtures(s.doc()?, job, &out);
                if !fixture_copies.is_empty() {
                    progress_line(&format!("  plumbing fixtures laid into the rooms of their type: {} ({fixture_report:?})", fixture_copies.len()));
                    stamped.extend(fixture_copies);
                }
                if connected > 0 {
                    progress_line(&format!("  equipment connections (EF, PTAC tags) put at their equipment: {connected}"));
                }
                out.retain(|dv| {
                    !authority.iter().any(|(level, outline)| {
                        dv.level == *level && cadaddy_geom::point_in_polygon(outline, cadaddy_geom::Vec2::new(dv.at[0], dv.at[1]))
                    })
                });
                if out.len() < before {
                    progress_line(&format!("  rooms an enlarged plan shows: {} of the overall plans' devices in them left out", before - out.len()));
                }
                if !report.is_empty() {
                    progress_line(&format!("  typical-room devices stamped into rooms of their type: {} ({report:?})", stamped.len()));
                }
                let not_placed = all_devices.len() - out.len();
                if not_placed > 0 {
                    job.add_finding(
                        StageName::BimBuild,
                        "Devices not placed",
                        &format!("{not_placed} devices are on sheets or views that don't line up with an architectural plan (and no typical-room plan); they are not drawn"),
                        2,
                        vec!["Register those sheets onto their level's plan".into()],
                        None,
                        false,
                    );
                }
                counts = (not_placed, stamped.len());
                out.extend(stamped);
                out
            }
            _ => all_devices.clone(),
        };
        // A stamped room's circuits are its own (a guest room's own panel, in the room's wall):
        // their routes in the room, where the sheet's reading can't put them.
        if let Ok((mut sp, true)) = super::bspec::current(s, "bim_build") {
            let (gear, room_panels) = super::printcond::room_panel_equipment(&room_panels, &panel_rooms, sp.units_per_foot);
            let (runs, circuits) = super::printcond::room_circuits(&devices, &room_panels, sp.units_per_foot);
            if !gear.is_empty() || !circuits.is_empty() {
                progress_line(&format!("  stamped rooms' panels: {}; circuits: {} ({} runs)", gear.len(), circuits.len(), runs.len()));
                super::printcond::store_room_panels(&mut sp, gear);
                super::printcond::store_room_circuits(&mut sp, runs, &circuits);
                s.doc_mut()?.building_spec = Some(sp.to_json());
            }
        }
        // A "receptacle" that is only a circuit-like name (a panel's feeder, DP2-11) by a keynote's
        // box (its number drawn in it) is no device: every receptacle symbol is round.
        let mut devices = devices;
        let before_boxes = devices.len();
        {
            let d = s.doc()?;
            let mut shapes: BTreeMap<String, bool> = BTreeMap::new();
            devices.retain(|dv| {
                if dv.kind != "receptacle" {
                    return true;
                }
                let Some(cadaddy_doc::EntityKind::Insert(ins)) =
                    cadaddy_doc::Handle::parse_hex(&dv.handle).and_then(|h| d.model.get(h)).map(|e| &e.kind)
                else {
                    return true;
                };
                !*shapes.entry(ins.block.clone()).or_insert_with(|| symbol_shape(d, &ins.block).is_some_and(|sh| plain_box(&sh)))
            });
        }
        if devices.len() < before_boxes {
            progress_line(&format!(
                "  circuit names by a keynote's box (its number in it), not receptacles: {} left out",
                before_boxes - devices.len()
            ));
        }
        // What each device is, by the legend row its symbol is.
        let legend = legend_symbols(s.doc()?, job);
        if !legend.is_empty() {
            let n = kinds_from_legend(s.doc()?, &legend, &mut devices);
            let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
            for dv in &devices {
                *kinds.entry(if dv.kind.is_empty() { super::readers::device_kind(&dv.tag).to_string() } else { dv.kind.clone() }).or_default() += 1;
            }
            progress_line(&format!(
                "  device kinds from the legends: {} symbols in {} rows; {n} devices took theirs; by kind {kinds:?}",
                legend.len(),
                legend.values().collect::<BTreeSet<_>>().len()
            ));
            // The ones still of no kind: their marks, each a question for the legend.
            let mut unknown: BTreeMap<&str, usize> = BTreeMap::new();
            for dv in devices.iter().filter(|dv| dv.kind.is_empty() && super::readers::device_kind(&dv.tag) == "device") {
                *unknown.entry(dv.tag.as_str()).or_default() += 1;
            }
            if !unknown.is_empty() {
                progress_line(&format!("  devices of no kind (their mark is in no legend row or known list): {unknown:?}"));
            }
        }
        // The stacks are pipes through the storey, not devices.
        let (stacks, devices): (Vec<super::readers::ReadDevice>, Vec<super::readers::ReadDevice>) =
            devices.into_iter().partition(|dv| dv.kind == "plumbing stack");
        if !stacks.is_empty()
            && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
        {
            let n = stacks_as_risers(&mut sp, &stacks);
            if n > 0 {
                s.doc_mut()?.building_spec = Some(sp.to_json());
            }
            let mut by: BTreeMap<(String, String), usize> = BTreeMap::new();
            for st in &stacks {
                *by.entry((st.level.clone(), st.leg.clone())).or_default() += 1;
            }
            progress_line(&format!("  waste and vent stacks as risers: {n} ({by:?})"));
        }
        match super::readers::place_devices(s, &devices, "guided job") {
            Ok(mut v) => {
                // What the devices stage read and the model doesn't hold, and what typical-room
                // plans added: read = inserted + not placed - stamped.
                v["not_placed"] = json!(counts.0);
                v["stamped"] = json!(counts.1);
                placed = Some(v);
                // What the legend says each switch and sensor is (its designations).
                let mut designations = legend_designations(s.doc()?, job);
                for (k, v) in unimported_designations(job) {
                    designations.entry(k).or_insert(v);
                }
                if !designations.is_empty() {
                    let keys: Vec<&str> = designations.keys().map(String::as_str).collect();
                    progress_line(&format!("  the legends' designations: {}", keys.join(" ")));
                }
                if !designations.is_empty()
                    && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
                {
                    let n = super::readers::legend_onto_devices(&mut sp, &designations);
                    if n > 0 {
                        s.doc_mut()?.building_spec = Some(sp.to_json());
                        progress_line(&format!("  switches and sensors named by the legend's designations: {n}"));
                    }
                }
                // The set's typical mounting heights (a general note: "DEVICE MOUNTED HEIGHTS (UNO):
                // TOGGLE SWITCH 4'-0\" AFF TO TOP…") onto each device of those kinds with none of its own.
                let pages = electrical_pages(s.doc()?, job);
                // The fire alarm symbol list (SYMBOL, DESCRIPTION, MOUNTING) onto the fire alarm
                // devices: what each mark is, and its ceiling or its height.
                let rows = symbol_list_rows(&pages);
                if !rows.is_empty()
                    && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
                {
                    let n = apply_symbol_list(&mut sp, &rows);
                    if n > 0 {
                        s.doc_mut()?.building_spec = Some(sp.to_json());
                    }
                    progress_line(&format!(
                        "  fire alarm symbol list: {} marks ({}); {n} devices named",
                        rows.len(),
                        rows.iter().map(|r| r.mark.as_str()).collect::<Vec<_>>().join(", ")
                    ));
                }
                let typical = typical_mounting_heights(&pages);
                panel_top = typical.iter().find(|t| t.kind == "panel" && t.to == "top").map(|t| (t.given_in, t.doc.clone()));
                if !typical.is_empty()
                    && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
                {
                    let n = apply_typical_heights(&mut sp, &typical);
                    if n > 0 {
                        s.doc_mut()?.building_spec = Some(sp.to_json());
                    }
                    progress_line(&format!(
                        "  typical mounting heights (general note): {} kinds ({}); {n} devices took theirs",
                        typical.len(),
                        typical.iter().map(|t| format!("{} {:.1}\"", t.kind, t.inches)).collect::<Vec<_>>().join(", ")
                    ));
                }
                // The typical rooms as unit types: their views and each room's hand and fit, for the
                // packets and the unit takeoff (written unless the spec has its own).
                if let Ok((mut sp, true)) = super::bspec::current(s, "bim_build") {
                    let empty = |v: &Value| v.is_null() || v.as_array().is_some_and(Vec::is_empty);
                    if empty(&sp.unit_types) && empty(&sp.unit_instances) {
                        let (types, instances) = job_unit_types(s.doc()?, job);
                        if !types.is_empty() {
                            let mirrored = instances.iter().filter(|i| i["hand"] == "mirrored").count();
                            let fitted = instances.iter().filter(|i| i.get("fit").is_some()).count();
                            progress_line(&format!(
                                "  unit types: {} ({} views); {} rooms ({fitted} with their layout fit, {mirrored} mirrored)",
                                types.len(),
                                types.iter().map(|t| t["views"].as_array().map_or(0, Vec::len)).sum::<usize>(),
                                instances.len()
                            ));
                            // Each room's level as the spec names it (the job's rooms carry its codes).
                            let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
                            let mut instances = instances;
                            for inst in &mut instances {
                                let code = inst["level"].as_str().unwrap_or_default().to_string();
                                let name = sp
                                    .level(&code)
                                    .or_else(|| sp.levels.iter().find(|l| !digits(&code).is_empty() && digits(&l.name) == digits(&code)))
                                    .map(|l| l.name.clone());
                                if let Some(n) = name {
                                    inst["level"] = json!(n);
                                }
                            }
                            sp.unit_types = Value::Array(types);
                            sp.unit_instances = Value::Array(instances);
                            s.doc_mut()?.building_spec = Some(sp.to_json());
                        }
                    }
                }
                // The plumbing fixtures named by their schedule.
                if let Some(sch) = &schedule
                    && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
                {
                    let n = super::readers::plumbing_onto_fixtures(&mut sp, &sch.rows);
                    if n > 0 {
                        s.doc_mut()?.building_spec = Some(sp.to_json());
                        progress_line(&format!("  plumbing fixtures named by the schedule: {n}"));
                    }
                }
                // The code's devices the plans don't mark: a guest room's receptacles are
                // tamper-resistant.
                if !sleeping.is_empty()
                    && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
                {
                    let (n, rooms) = super::readers::tamper_resistant_in_rooms(&mut sp, &sleeping);
                    if n > 0 {
                        s.doc_mut()?.building_spec = Some(sp.to_json());
                        progress_line(&format!("  tamper-resistant receptacles (NEC 406.12): {n} in {rooms} guest rooms"));
                        job.add_finding(
                            StageName::BimBuild,
                            "Tamper-resistant receptacles in the guest rooms",
                            &format!(
                                "{n} receptacles in {rooms} guest rooms are taken off as tamper-resistant: NEC 406.12 requires them in hotel guest rooms and dwelling units; the plans don't mark them TR"
                            ),
                            2,
                            vec!["Confirm the TR devices with the engineer (the wiring devices section lists them)".into()],
                            None,
                            false,
                        );
                    }
                }
                // A receptacle by a sink is a GFCI device (non-feed-through GFCIs).
                if schedule.is_some()
                    && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
                {
                    let (n, sinks) = super::readers::gfci_by_sinks(&mut sp);
                    if n > 0 {
                        s.doc_mut()?.building_spec = Some(sp.to_json());
                        progress_line(&format!("  receptacles within 6' of a sink taken off as GFCI (NEC 210.8): {n} by {sinks} sinks"));
                        job.add_finding(
                            StageName::BimBuild,
                            "Receptacles by sinks taken off as GFCI",
                            &format!(
                                "{n} receptacles within 6' of {sinks} sinks and lavatories aren't marked GFCI on the plans: NEC 210.8 protects them, and GFCI receptacles that don't feed through make each one a GFCI device; measured from where the plumbing plans tag each sink"
                            ),
                            3,
                            vec!["Check them on the plans: a GFCI breaker on the circuit would protect them instead".into()],
                            None,
                            false,
                        );
                    }
                }
            }
            Err(e) => job.add_finding(
                StageName::BimBuild,
                &format!("The devices could not be put in the model: {e}"),
                "the devices stage found devices but the model has none",
                4,
                vec!["Run mep.read_devices {preview: false} once the model is built".into()],
                None,
                false,
            ),
        }
    }

    // Use model.build directly — it returns walls/doors/rooms/levels counts.
    // (bim.build wraps model.build but constructs its own result without those counts.)
    // Q6029 MUST 3: pass a configurable time budget (default 20 min).
    bim_step(s, job, "devices placed", t);
    let t = web_time::Instant::now();
    if let Some(place) = placer.as_deref() {
        let n = anchor_other_trade_rooms(s, job, place)?;
        if n > 0 {
            progress_line(&format!("  rooms traced from other trades' tags: {n}"));
        }
    }
    // The circulation goes in the spec before the rooms are traced: they are traced in what it
    // leaves.
    if !circulation.is_empty()
        && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
    {
        sp.circulation = std::mem::take(&mut circulation);
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    // Each storey stacks by its shell's lower-left corner (the building's outline), not by the
    // lower-left of all its wall linework: a stray line or a site note there moved a storey
    // off the ones above and below it (B4-016). A level given a base keeps it.
    if let Some(sh) = shells.as_ref()
        && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
    {
        let mut set = Vec::new();
        for (name, rects) in sh {
            let Some(l) = sp.levels.iter_mut().find(|l| l.name.eq_ignore_ascii_case(name) && l.base.is_none()) else { continue };
            let corner = rects.iter().fold([f64::INFINITY; 2], |c, [a, b]| [c[0].min(a.x).min(b.x), c[1].min(a.y).min(b.y)]);
            if corner.iter().all(|v| v.is_finite()) {
                l.base = Some(corner);
                set.push(l.name.clone());
            }
        }
        if !set.is_empty() {
            progress_line(&format!("  storeys stacked by their shells' corners: {}", set.join(", ")));
        }
        // Then each storey onto the one under it by the rooms they share (a typical floor's
        // 3NN over 2NN): a shell drawn a foot off on one sheet no longer moves that storey.
        let (moved, not) = register_storeys_by_rooms(s.doc()?, &mut sp);
        if !moved.is_empty() {
            progress_line(&format!("  storeys lined up on the rooms of the one under them: {}", moved.join("; ")));
        }
        if !not.is_empty() {
            progress_line(&format!("  storeys not lined up by their rooms: {}", not.join("; ")));
        }
        // The shells go to the build: a wall line well past its storey's is none of its walls.
        // Without their slivers (a strip a wall thick: a stray wall line traced into the shell,
        // L4's ran 7' past the building), so the stray line is measured from the floor.
        let sliver = SHELL_SLIVER_FT * if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
        sp.shells = sh
            .iter()
            .map(|(n, rects)| (n.clone(), rects.iter().filter(|[a, b]| (b.x - a.x).abs() >= sliver).map(|[a, b]| [a.x, a.y, b.x, b.y]).collect()))
            .collect();
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    // How every wall is built: the partition legend's types, the walls the plans' tags type.
    if job.sheet_regions.iter().any(|r| r.kind == "legend")
        && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
    {
        for line in apply_wall_legend(s, &mut sp, job)? {
            progress_line(&format!("  wall types {line}"));
        }
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    // The stairs in the circulation's stair spaces, found from their treads on any layer: the
    // build makes their steps, landings and rails. Confirmed stairs stay.
    if let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
        && !sp.circulation.is_empty()
    {
        let found = super::stairs::detect_in_circulation(s, &sp).unwrap_or_default();
        if !found.is_empty() {
            let names: Vec<String> = found.iter().map(|st| format!("{} ({} flights)", st.name, st.runs.len())).collect();
            progress_line(&format!("  stairs found in their stair spaces: {}", names.join(", ")));
            sp.stairs.retain(|x| x.confirmed);
            // A stair no code lets be built as read (a misread flight: risers of 31", or 30 of
            // 3½") is left out and asked about, never built.
            let mut unbuildable = Vec::new();
            for st in found {
                if sp.stairs.iter().any(|x| x.name.eq_ignore_ascii_case(&st.name)) {
                    continue;
                }
                match sp.stair_unbuildable(&st) {
                    Some(why) => unbuildable.push(format!("{} on {}: {why}", st.name, st.level)),
                    None => sp.stairs.push(st),
                }
            }
            if !unbuildable.is_empty() {
                progress_line(&format!("  stairs left out (can't be built as read): {}", unbuildable.join("; ")));
                job.add_finding(
                    StageName::BimBuild,
                    "Stairs left out: they can't be built as read",
                    &format!("{} stairs found in their stair spaces measure risers no code allows: {}", unbuildable.len(), unbuildable.join("; ")),
                    2,
                    vec!["Check the stair's treads and the levels it joins on the plans, then add it with stairs.detect or the building spec".into()],
                    None,
                    false,
                );
            }
            s.doc_mut()?.building_spec = Some(sp.to_json());
        }
    }
    let budget_s = super::bspec::BUILD_BUDGET_S;
    // The job merges walls read off several sheets after the build: its own line says the counts
    // the model ends with (= bim.tree), the build's would be a second, earlier set.
    let mut result = s.execute("model.build", &json!({"budget_s": budget_s, "echo": false}))?;
    bim_step(s, job, "model built", t);
    if let Some(place) = placer.as_deref() {
        let (n, missed) = number_rooms_from_other_trades(s, job, place)?;
        if n > 0 || !missed.is_empty() {
            progress_line(&format!("  rooms numbered from other trades' plans and the plans' stray numbers: {n}; not numbered: {missed:?}"));
        }
    }
    let scraps = drop_numbered_scraps(s)?;
    if scraps > 0 {
        progress_line(&format!("  room scraps dropped (under {SCRAP_SF} sf, a bigger room of the level has their number): {scraps}"));
    }
    if let Some(shell) = shells.as_ref() {
        let outside = drop_rooms_outside_shell(s, shell)?;
        if outside > 0 {
            progress_line(&format!("  rooms outside the building's shell dropped: {outside}"));
            job.add_finding(
                StageName::BimBuild,
                "Rooms outside the building",
                &format!("{outside} rooms traced outside their level's exterior walls were left out"),
                1,
                vec!["Check the plan's labels outside the building".into()],
                None,
                false,
            );
        }
    }
    let stacked = number_stacked_rooms(s)?;
    if stacked > 0 {
        progress_line(&format!("  rooms numbered from the same room on another floor: {stacked}"));
    }
    let t = web_time::Instant::now();
    if let Some(v) = placed {
        result["devices"] = v;
    }
    // The branch circuits (their runs went in with the MEP parts): one tag each, for the takeoff;
    // not the typical-room views' (each room's own circuits are those).
    let circuits: Vec<_> = job.circuits.iter().filter(|c| !room_view_circuits.contains(&c.id)).cloned().collect();
    if !circuits.is_empty()
        && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
    {
        super::printcond::store_circuits(&mut sp, &circuits);
        s.doc_mut()?.building_spec = Some(sp.to_json());
        result["circuits"] = json!(circuits.len());
    }
    // The model's rooms are the job's rooms on the levels it built.
    if let Some(by_level) = rooms_from_model(s, job)
        && let Some(o) = result.as_object_mut()
    {
        o.insert("rooms_by_level".into(), by_level);
    }
    // Q6002: the rooms as elements, then what the readers found on every element.
    bim_step(s, job, "circuits and model rooms", t);
    let t = web_time::Instant::now();
    // The architectural floor plans only: tracing the whole print set took over 10 min.
    let arch: Vec<[f64; 4]> = job
        .sheet_regions
        .iter()
        .filter(|r| r.kind == "plan" && r.not_built.is_empty() && r.discipline.eq_ignore_ascii_case("A"))
        .map(SheetRegion::plan_rect)
        .collect();
    let volumes = if arch.is_empty() { json!({}) } else { json!({"regions": arch}) };
    // A print set's room volumes: after the build the stacked model sits on the lowest level's
    // plan and the trace ran for 18+ min on Windows. The rooms are already model elements.
    let no_rooms = s.doc().map_or(true, |d| super::bspec::room_elements(d).is_empty());
    let volumes = if job.sheet_regions.len() > 8 && no_rooms { json!({"clear": true}) } else { volumes };
    match s.execute("rooms.build", &volumes) {
        Ok(v) => result["room_volumes"] = v.get("built").cloned().unwrap_or(Value::Null),
        Err(e) => result["room_volumes_error"] = json!(e.to_string()),
    }
    bim_step(s, job, "room volumes built", t);
    // The panel schedules (each circuit's breaker and its type), for the devices' props.
    let t = web_time::Instant::now();
    let pages = s.doc().map(|d| electrical_pages(d, job)).unwrap_or_default();
    let schedules = panel_schedules(&pages);
    if !schedules.is_empty()
        && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
    {
        let typed = schedules.iter().flat_map(|p| &p.circuits).filter(|c| !c.protection.is_empty()).count();
        progress_line(&format!(
            "  panel schedules: {} ({} circuits, {typed} with their breaker's type): {}",
            schedules.len(),
            schedules.iter().map(|p| p.circuits.len()).sum::<usize>(),
            schedules.iter().map(|p| p.panel.as_str()).collect::<Vec<_>>().join(", ")
        ));
        result["panel_schedules"] = json!(schedules.len());
        sp.mep.panel_schedules = schedules;
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    bim_step(s, job, "panel schedules", t);
    // The electrical gear the plans don't place, from the schedules, the one-line and the gear
    // submittals, in its rooms; the feeders between; the gear checked (entry, lugs) and reviewed.
    let t = web_time::Instant::now();
    if let Err(e) = gear_step(s, job, &pages, &mut result) {
        result["gear_error"] = json!(e.to_string());
    }
    bim_step(s, job, "electrical gear", t);
    // What the electrical keynotes ask built (a BDA system's boxes and conduit).
    let t = web_time::Instant::now();
    if let Err(e) = keynote_step(s, job, &pages, &mut result) {
        result["keynotes_error"] = json!(e.to_string());
    }
    bim_step(s, job, "keynotes", t);
    // The feeders' conduit, gear to gear (proposed).
    let t = web_time::Instant::now();
    if let Err(e) = feeder_step(s, job, &mut result) {
        result["feeders_error"] = json!(e.to_string());
    }
    bim_step(s, job, "feeder routes", t);
    // Buried work the set issues no plan for: its underground sheets generated.
    let t = web_time::Instant::now();
    if let Err(e) = underground_step(s, job, &pages, &mut result) {
        result["underground_error"] = json!(e.to_string());
    }
    bim_step(s, job, "underground", t);
    // The wiring-method alternatives the code and the spec allow, priced from the model.
    let t = web_time::Instant::now();
    if let Err(e) = value_step(s, job, &mut result) {
        result["value_engineering_error"] = json!(e.to_string());
    }
    bim_step(s, job, "value engineering", t);
    let t = web_time::Instant::now();
    let fill = match &panel_top {
        Some((top, doc)) => json!({"panel_top_in": top, "panel_top_doc": doc}),
        None => json!({}),
    };
    match s.execute("bim.props.fill", &fill) {
        Ok(v) => {
            if let Some(n) = v.get("panels_heighted").and_then(Value::as_u64) {
                progress_line(&format!("  panels at the general note's height to top: {n}"));
            }
            // Devices in no room (outside every room, not on a room's wall): per level.
            if let Some(m) = v.get("devices_in_no_room").and_then(Value::as_object).filter(|m| !m.is_empty()) {
                let per: Vec<String> = m.iter().map(|(l, n)| format!("{}: {n}", if l.is_empty() { "no level" } else { l })).collect();
                progress_line(&format!("  devices in no room: {}", per.join(", ")));
            }
            // Boxes the partition legend gives no ring for: their wall's type is unknown.
            let untyped = v.get("wall_boxes_untyped").and_then(Value::as_u64).unwrap_or(0);
            if untyped > 0 {
                job.add_finding(
                    StageName::BimBuild,
                    &format!("{untyped} wall boxes on walls of unknown build-up: no mud ring"),
                    "No tag on the plans types their walls, and no tagged wall of the same thickness says what they are",
                    2,
                    vec!["Tag the walls (or check the partition legend's marks) so their boxes get rings".into()],
                    None,
                    false,
                );
            }
            result["props"] = v;
        }
        Err(e) => job.add_finding(
            StageName::BimBuild,
            &format!("The elements' properties could not be filled: {e}"),
            "bim.props.fill failed after the build",
            3,
            vec!["Run bim.props.fill once the model is built".into()],
            None,
            false,
        ),
    }
    // The plans kept out of the building (MUST 3), for the job report.
    let skipped = not_built_sheets(job);
    if !skipped.is_empty()
        && let Some(o) = result.as_object_mut()
    {
        o.insert("not_built".into(), json!(skipped));
    }
    // Tags are identities: one element per door, room and device however many sheets show it,
    // each door between its two rooms, and the schedules checked against the plans. Never stops
    // the job: a failure is a finding.
    bim_step(s, job, "properties filled", t);
    let t = web_time::Instant::now();
    identity_step(s, job, &mut result);
    bim_step(s, job, "tag identity", t);
    if !job.submittals.is_empty() {
        let t = web_time::Instant::now();
        submittals_step(s, job, &mut result);
        bim_step(s, job, "submittals applied", t);
    }
    // The code rules on the devices first (their product follows the decision: a GFCI built standard is a receptacle,
    // a guest room's is the tamper-resistant one; tamper-resistant in guest rooms, GFCI by circuit), the
    // decisions the documents can't make left `undecided` and asked.
    let t = web_time::Instant::now();
    match s.execute("bim.coderules", &json!({})) {
        Ok(v) => {
            let line = format!(
                "  code rules: {} receptacles; tamper-resistant {} (TR stated in the documents for {}); GFCI by circuit {}; receptacle circuits in GFCI places on breakers that aren't: {}",
                v.get("receptacles").and_then(Value::as_u64).unwrap_or(0),
                v.get("tamper_resistant").and_then(Value::as_u64).unwrap_or(0),
                v.pointer("/tr_documented/stated").and_then(Value::as_u64).unwrap_or(0),
                v.get("gfci").map(Value::to_string).unwrap_or_default(),
                gfci_places(&v).join("; ")
            );
            report_progress(s, json!({"step": "code rules"}), &line);
            s.echo(line);
            let places = gfci_places(&v);
            if !places.is_empty() {
                // Its cost: a GFCI receptacle for a standard one on each.
                let n = places.len() as f64;
                let change = s
                    .execute(
                        "estimate.change",
                        &json!({"lines": [{"item": "GFCI receptacle", "qty": n}, {"item": "Duplex receptacle", "qty": -n}], "why": "GFCI places"}),
                    )
                    .ok();
                let cost = change.as_ref().map_or_else(String::new, |c| {
                    format!(
                        " (change: ${:.2} material, {:.2} labor hours)",
                        c.pointer("/totals/material").and_then(Value::as_f64).unwrap_or(0.0),
                        c.pointer("/totals/labor_hours").and_then(Value::as_f64).unwrap_or(0.0)
                    )
                });
                job.add_finding(
                    StageName::BimBuild,
                    &format!(
                        "{} receptacle circuits in places needing GFCI have breakers that aren't GFCI: {}{cost}",
                        places.len(),
                        places.join("; ")
                    ),
                    "a receptacle in an elevator pit, car or hoistway, on a roof, outdoors, in a garage, bathroom or laundry needs GFCI (NEC 620.6, 210.8); the breaker isn't, so the first receptacle on the circuit is a GFCI device",
                    3,
                    vec!["Build (and buy) a GFCI receptacle first on each, or confirm the breaker is GFCI".into()],
                    None,
                    false,
                );
            }
            let undecided = v.pointer("/gfci/undecided").and_then(Value::as_u64).unwrap_or(0);
            if undecided > 0 {
                job.add_finding(
                    StageName::BimBuild,
                    &format!("{undecided} GFCI receptacles are undecided: their breaker's type isn't in the panel schedule, or which is first on the circuit isn't known"),
                    "GFCI is judged per circuit: a receptacle under a GFCI breaker is standard; else the first on the circuit is the GFCI",
                    2,
                    vec!["Give the panel schedules' breaker types, or mark which receptacle is first on each circuit".into()],
                    None,
                    false,
                );
            }
            result["code_rules"] = v;
        }
        Err(e) => result["code_rules_error"] = json!(e.to_string()),
    }
    bim_step(s, job, "code rules", t);
    // The specifications' product data (and the submittals' as applied) onto the elements.
    let t = web_time::Instant::now();
    products_step(s, job, &mut result);
    bim_step(s, job, "product data", t);
    // The light fixture schedule on each luminaire, and the lighting documents checked against
    // each other (plans, schedule, submittal).
    let t = web_time::Instant::now();
    if let Err(e) = lighting_step(s, job, &pages, &mut result) {
        result["lighting_error"] = json!(e.to_string());
    }
    bim_step(s, job, "lighting", t);
    // Check for warnings.
    if let Some(warnings) = result.get("warnings").and_then(Value::as_array) {
        for w in warnings.iter().take(MAX_QUESTIONS_PER_STAGE) {
            let msg = w.get("message").and_then(Value::as_str).or_else(|| w.as_str()).unwrap_or("Build warning");
            let qid = job.next_qid(StageName::BimBuild);
            let q = Question {
                id: qid,
                stage: StageName::BimBuild,
                question: format!("BIM build warning: {msg} — accept?"),
                why: "Build produced a warning that may indicate an issue".into(),
                choices: vec!["Accept".into(), "Investigate".into()],
                evidence: Some(QuestionEvidence {
                    crop_png_path: None,
                    sheet: None,
                    at: None,
                    values: Some(w.clone()),
                    handles: Vec::new(),
                    tiled_crops: None,
                    leftovers: Vec::new(),
                }),
                default: Some("Accept".into()),
                answer_schema: None,
                answer: None,
                answered_by: None,
                answered_at: None,
                kind: QuestionKind::Doubt,
            };
            if let Some(st) = job.stage_mut(StageName::BimBuild) {
                st.questions.push(q);
            }
        }
    }

    // MUST 1: Check if build produced 0 elements.
    let built_count = result.get("counts").and_then(Value::as_object).map_or(0, |c| c.len());
    if built_count == 0 {
        job.add_finding(
            StageName::BimBuild,
            "BIM build produced 0 elements",
            "The build returned no geometry; the building spec may be empty or incorrect",
            5,
            vec!["Review the building spec".into(), "Check that rooms and levels were found".into()],
            None,
            false,
        );
    }

    // Plausibility check.
    if let Some(counts) = result.get("counts").and_then(Value::as_object) {
        for (key, val) in counts {
            if let Some(n) = val.as_f64()
                && n > 100_000.0
            {
                let qid = job.next_qid(StageName::BimBuild);
                let q = Question {
                    id: qid,
                    stage: StageName::BimBuild,
                    question: format!("{key}: {n:.0} is unusually high — accept, or re-run detection?"),
                    why: format!("{key} count exceeds plausible range"),
                    choices: vec!["Accept".into(), "Re-run detection".into()],
                    evidence: Some(QuestionEvidence {
                        crop_png_path: None,
                        sheet: None,
                        at: None,
                        values: Some(json!({key: n})),
                        handles: Vec::new(),
                        tiled_crops: None,
                        leftovers: Vec::new(),
                    }),
                    default: None,
                    answer_schema: None,
                    answer: None,
                    answered_by: None,
                    answered_at: None,
                    kind: QuestionKind::Doubt,
                };
                if let Some(st) = job.stage_mut(StageName::BimBuild) {
                    st.questions.push(q);
                }
            }
        }
    }

    // Q5278: persist devices detected by the Devices stage into sp.tags, AFTER bspec.draft
    // stored the spec (which would overwrite earlier tag inserts). Only when the stage placed no
    // devices of its own (`job.devices`, put in the model above): counting both doubled them.
    if job.devices.is_empty()
        && let Some(dev_stage) = job.stage(StageName::Devices)
        && let Some(devices) = dev_stage.result.as_ref().and_then(|r| r.get("devices"))
        && let Some(items) = devices.get("items").and_then(Value::as_array)
    {
        let level_name = super::bspec::current(s, "guided").ok().and_then(|(sp, _)| sp.levels.first().map(|l| l.name.clone())).unwrap_or_default();
        let mut new_tags = Vec::new();
        for item in items.iter().take(50_000) {
            let tag = item.get("tag").and_then(Value::as_str).unwrap_or("unknown");
            let count = item.get("count").and_then(Value::as_u64).unwrap_or(0);
            for i in 0..count {
                let id = format!("device:{tag}:{i}");
                let kind =
                    if tag.to_ascii_lowercase().contains("light") || tag.to_ascii_lowercase().contains("lum") { "luminaire" } else { "receptacle" };
                new_tags.push(cadaddy_walk3d::tags::ElementTag {
                    id,
                    kind: kind.into(),
                    level: level_name.clone(),
                    at_ft: [0.0, 0.0],
                    fields: {
                        let mut f = std::collections::BTreeMap::new();
                        f.insert("device_type".into(), cadaddy_walk3d::tags::Field { value: tag.to_string(), source: "plan".into() });
                        f
                    },
                    ..Default::default()
                });
            }
        }
        if !new_tags.is_empty()
            && let Ok((mut sp, _)) = super::bspec::current(s, "guided")
        {
            sp.tags.extend(new_tags);
            if let Ok(spec_val) = serde_json::to_value(&sp) {
                let _ = s.execute("bspec.set", &json!({"replace": true, "spec": spec_val}));
            }
        }
    }

    // The levels the user named or confirmed keep their answers in the new model.
    let level_changes = apply_level_answers(s, job);
    if !level_changes.is_empty()
        && let Some(o) = result.as_object_mut()
    {
        o.insert("level_answers".into(), json!(level_changes));
    }

    job.audit("stage_run", "pipeline", None, Some(StageName::BimBuild), Some(json!({"built_count": built_count})));
    // The counts are the model's as the stage ends (B4-022: tag identity merges a wall or door
    // read off several sheets into one after the build; the summary kept the build's 2,977
    // walls of 2,848).
    super::bspec::recount_built(s.doc()?, &mut result);
    progress_line(&format!("  the model as built: {} walls, {} doors, {} windows", result["walls"], result["doors"], result["windows"]));
    Ok(result)
}

/// The plan sheets never built into a building level, as `A010: not built: site/civil`.
fn not_built_sheets(job: &GuidedJob) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for r in job.sheet_regions.iter().filter(|r| !r.not_built.is_empty()).take(MAX_QUESTIONS_PER_STAGE) {
        let line = format!("{}: not built: {}", if r.number.is_empty() { "?" } else { &r.number }, r.not_built);
        if !out.contains(&line) {
            out.push(line);
        }
    }
    out
}

/// Floor to floor when the sections give none, feet.
const DEFAULT_STOREY_FT: f64 = 10.0;

/// The building's levels for `bspec.draft`: one per level that has an architectural floor plan
/// (discipline A, type plan; the first such sheet of each level), lowest first, its region the
/// plan's drawing area on the job's drawing (inside the border, off the title block, so sheet
/// furniture builds nothing). Elevations and floor-to-floor heights from the sheet
/// index (read off the sections); else stacked at [`DEFAULT_STOREY_FT`] and marked as guessed,
/// so the build stage asks. Empty when the set has no architectural floor plans (the build reads
/// whatever was imported, as for one sheet).
/// A plan's level as the building's levels name it: `Level 3`, `Roof`, `B1`.
/// Uses the canonical level mapping so every alias resolves to one name:
/// R/RF/ROOF/T.O. → Roof, B/B1 → Basement, PH → Penthouse, M/MEZZ → Mezzanine, etc.
fn level_name(code: &str) -> String {
    // Try canonical_level first; fall back to common short codes that the canonical
    // function doesn't handle as bare single-word inputs.
    let canonical = cadaddy_walk3d::levels::canonical_level(code).map(|(c, _)| c).or_else(|| {
        let u = code.trim().to_ascii_uppercase();
        match u.as_str() {
            "PH" => Some("PENTHOUSE".into()),
            "RF" | "T.O." => Some("ROOF".into()),
            "G" => Some("LEVEL 1".into()),
            "M" | "MEZZ" => Some("MEZZANINE".into()),
            "MPH" => Some("PENTHOUSE".into()),
            _ => None,
        }
    });
    if let Some(canonical) = canonical {
        // Title-case the canonical name: "ROOF" → "Roof", "LEVEL 3" → "Level 3", etc.
        let mut out = String::with_capacity(canonical.len());
        for (i, word) in canonical.split_whitespace().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                out.push(first);
                for c in chars {
                    out.push(c.to_ascii_lowercase());
                }
            }
        }
        out
    } else if !code.is_empty() && code.chars().all(|c| c.is_ascii_digit()) {
        format!("Level {code}")
    } else {
        code.to_string()
    }
}

/// Most sheets the job lists in the spec.
const MAX_SHEET_LINKS: usize = 2_000;

/// The job's imported sheets into the spec's sheet list, each with where it sits in model space
/// and its level (a sheet listed already keeps its entry and gains its place).
/// The build's identity pass (`bim.identity` then `rooms.graph`): merged copies leave the counts,
/// findings go on the build stage, and the answer's summary goes in the result as `identity`.
/// The specifications the job reads: those given, else a project's specifications folder beside
/// its submittals (`…\\03_Specifications` next to `…\\02_Submittals`).
fn job_specs(job: &GuidedJob) -> Vec<String> {
    if !job.specs.is_empty() {
        return job.specs.clone();
    }
    let mut out: Vec<String> = Vec::new();
    for given in &job.submittals {
        let Some(parent) = std::path::Path::new(given.trim()).parent() else { continue };
        let Ok(rd) = std::fs::read_dir(parent) else { continue };
        let mut found: Vec<String> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_uppercase().contains("SPEC")))
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        found.sort();
        for f in found {
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

/// The specifications' and submittals' product data onto the built elements (`bim.products`,
/// after the submittals): each device, fixture and equipment element's `product` record. Never
/// stops the job: kinds no section or submittal speaks for are a finding.
fn products_step(s: &mut Session, job: &mut GuidedJob, result: &mut Value) {
    let specs = job_specs(job);
    if specs.is_empty() && job.submittals.is_empty() {
        return;
    }
    let v = match s.execute("bim.products", &json!({"specs": specs})) {
        Ok(v) => v,
        Err(e) => {
            job.add_finding(
                StageName::BimBuild,
                &format!("The product data could not be applied: {e}"),
                "bim.products failed after the build",
                2,
                Vec::new(),
                None,
                false,
            );
            return;
        }
    };
    let line = format!(
        "  product data: {} spec sections ({} requirements, kinds {}); records on {} elements; by kind {}",
        v.get("sections").and_then(Value::as_array).map_or(0, Vec::len),
        v.get("requirements").and_then(Value::as_u64).unwrap_or(0),
        v.get("kinds_specified").map(Value::to_string).unwrap_or_default(),
        v.get("elements").and_then(Value::as_u64).unwrap_or(0),
        v.get("by_kind").map(Value::to_string).unwrap_or_default()
    );
    report_progress(s, json!({"step": "products", "elements": v.get("elements")}), &line);
    s.echo(line);
    let none: Vec<String> = v
        .get("no_product_data")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(k, why)| format!("{k}: {}", why.as_str().unwrap_or("")))
        .collect();
    if !none.is_empty() {
        job.add_finding(
            StageName::BimBuild,
            &format!("{} kinds of element carry no product data: {}", none.len(), none.join("; ")),
            "No specification section read and no submittal speaks for them",
            1,
            vec!["Give the job their specification sections or submittals".into()],
            None,
            false,
        );
    }
    result["products"] = v;
}

/// The submittals' products onto the built elements (BUILD ORDER: after the architecture and the
/// MEP objects; `bim.submittals`). Never stops the job: what they leave open is a finding.
fn submittals_step(s: &mut Session, job: &mut GuidedJob, result: &mut Value) {
    let v = match s.execute("bim.submittals", &json!({"paths": job.submittals})) {
        Ok(v) => v,
        Err(e) => {
            job.add_finding(
                StageName::BimBuild,
                &format!("The submittals could not be read: {e}"),
                "bim.submittals failed after the build",
                3,
                vec!["Check the submittal paths given to job.start".into()],
                None,
                false,
            );
            return;
        }
    };
    let applied = v.get("applied").and_then(Value::as_object).cloned().unwrap_or_default();
    let elements: u64 = applied.values().filter_map(Value::as_u64).sum();
    let line = format!(
        "  submittals: {} files read, products onto {elements} elements of {} types ({}); dimming {}",
        v.get("files").and_then(Value::as_array).map_or(0, Vec::len),
        applied.len(),
        applied.keys().cloned().collect::<Vec<_>>().join(", "),
        v.get("dimming").map(Value::to_string).unwrap_or_default()
    );
    report_progress(s, json!({"step": "submittals", "elements": elements, "types": applied.len()}), &line);
    s.echo(line);
    // The model's device types no submittal covers (switches, boxes and the like are by spec).
    let missing: Vec<String> = v
        .get("no_submittal")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(m, _)| super::propfill::kind_of(m) == "device")
        .map(|(m, n)| format!("{m} ({n})"))
        .collect();
    if !missing.is_empty() {
        job.add_finding(
            StageName::BimBuild,
            &format!("{} device types have no submittal product: {}", missing.len(), missing.join(", ")),
            "No submittal given to the job has a data sheet for them, so their elements carry no maker or catalog number",
            1,
            vec!["Add their submittals, or confirm they are by others".into()],
            None,
            false,
        );
    }
    // A type the model holds whose submittal leaves a question (its dimming, say).
    for q in v.get("questions").and_then(Value::as_array).into_iter().flatten() {
        let mark = q.get("mark").and_then(Value::as_str).unwrap_or("").to_ascii_uppercase();
        let text = q.get("text").and_then(Value::as_str).unwrap_or("");
        if applied.contains_key(&mark) && !text.is_empty() {
            job.add_finding(
                StageName::BimBuild,
                text,
                "The submittal does not settle it for the product ordered",
                2,
                vec!["Ask the supplier which option was ordered".into()],
                None,
                false,
            );
        }
    }
    for u in v.get("unread").and_then(Value::as_array).into_iter().flatten() {
        let file = u.get("file").and_then(Value::as_str).unwrap_or("");
        let error = u.get("error").and_then(Value::as_str).unwrap_or("");
        job.add_finding(
            StageName::BimBuild,
            &format!("Submittal {file} could not be read: {error}"),
            "bim.spec_import failed on it",
            2,
            Vec::new(),
            None,
            false,
        );
    }
    result["submittals"] = v;
    if applied.is_empty() {
        return;
    }
    // CHECKS on the submitted products: what each switch leg dims.
    match s.execute("bim.dimming", &json!({})) {
        Ok(dv) => {
            let line = format!(
                "  dimming: {} switch legs, {} with phase and 0-10V fixtures together; 0-10V pairs on {} legs to {} fixtures",
                dv.get("legs").and_then(Value::as_array).map_or(0, Vec::len),
                dv.get("mixed").and_then(Value::as_u64).unwrap_or(0),
                dv.pointer("/pairs/legs").and_then(Value::as_u64).unwrap_or(0),
                dv.pointer("/pairs/fixtures").and_then(Value::as_u64).unwrap_or(0)
            );
            report_progress(s, json!({"step": "dimming", "mixed": dv.get("mixed")}), &line);
            s.echo(line);
            for f in dv.get("findings").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                // What the check found: a leg mixing phase and 0-10V fixtures; a dimmer on a
                // 0-10V leg whose kind the plans don't say; 0-10V fixtures only switched; the pair.
                let (why, severity, todo) = if f.contains("on one control") {
                    (
                        "The fixtures' submittals give them different dimming on one switch leg",
                        4,
                        "Split the leg, or change one product so both dim the same way",
                    )
                } else if f.contains("has to be a 0-10V dimmer") {
                    (
                        "The plans name the dimmer by a designation only; the fixtures on its leg are 0-10V",
                        3,
                        "Ask which dimmer it is (the lighting controls schedule): a 0-10V dimmer with its pair",
                    )
                } else if f.contains("no dimmer") {
                    ("A 0-10V fixture on a plain switch never dims", 1, "Confirm the fixtures are meant not to dim")
                } else {
                    (
                        "A 0-10V fixture dims over its own 2-wire control pair from the dimmer or sensor",
                        1,
                        "Check each leg's dimmer or sensor: the pair's run to it is an allowance where the plans don't place it",
                    )
                };
                job.add_finding(StageName::BimBuild, f, why, severity, vec![todo.into()], None, false);
            }
            result["dimming"] = dv;
        }
        Err(e) => result["dimming_error"] = json!(e.to_string()),
    }
}

fn identity_step(s: &mut Session, job: &mut GuidedJob, result: &mut Value) {
    let numbers: Vec<String> = job
        .stage(StageName::Rooms)
        .and_then(|st| st.result.as_ref())
        .and_then(|r| r.get("rooms"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|r| r.get("number").and_then(Value::as_str)).map(str::to_string).collect())
        .unwrap_or_default();
    let run = match super::identity::identity(s, true, (!numbers.is_empty()).then_some(numbers.as_slice())) {
        Ok(r) => r,
        Err(e) => {
            job.add_finding(
                StageName::BimBuild,
                &format!("Element identities were not checked: {e}"),
                "the same door or device may be counted once per sheet that shows it",
                3,
                vec!["Run bim.identity once the model is built".into()],
                None,
                false,
            );
            return;
        }
    };
    // The build's counts are the model's after the merge.
    for (kind, key) in [("door", "doors"), ("window", "windows"), ("wall", "walls")] {
        let gone = run.removed.get(kind).copied().unwrap_or(0) as u64;
        if gone > 0
            && let Some(n) = result.get(key).and_then(Value::as_u64)
        {
            result[key] = json!(n.saturating_sub(gone));
        }
    }
    let graph = super::identity::graph(s, true).ok();
    let findings: Vec<Value> = run.answer.get("findings").and_then(Value::as_array).cloned().unwrap_or_default();
    for f in findings.iter().take(MAX_QUESTIONS_PER_STAGE) {
        let msg = f.get("message").and_then(Value::as_str).unwrap_or("element identity finding");
        let kind = f.get("kind").and_then(Value::as_str).unwrap_or("");
        job.add_finding(
            StageName::BimBuild,
            msg,
            match kind {
                "same_tag_apart" => "one number names one thing: two places on a level is a misread or a print error",
                "schedule_not_placed" | "not_in_schedule" | "doors_vs_schedule" => "the door schedule and the plans disagree",
                _ => "the finish schedule names a room the plans do not show",
            },
            2,
            vec!["Check the sheets named in the finding".into()],
            None,
            false,
        );
    }
    let sheets = run.answer.get("sheets").and_then(Value::as_array);
    let registered = sheets.map_or(0, |a| a.iter().filter(|t| t.get("registered").and_then(Value::as_bool) == Some(true)).count());
    if let Some(o) = result.as_object_mut() {
        o.insert(
            "identity".into(),
            json!({
                "merged": run.answer.get("merged"),
                "by_kind": run.answer.get("by_kind"),
                "numbered_doors": run.answer.get("numbered_doors"),
                "sheets": sheets.map_or(0, Vec::len),
                "sheets_registered": registered,
                "findings": findings.len(),
                "door_connections": graph.as_ref().and_then(|g| g.get("connected")),
                "room_edges": graph.as_ref().and_then(|g| g.get("edges")).and_then(Value::as_array).map(Vec::len),
            }),
        );
    }
    job.audit("identity", "pipeline", None, Some(StageName::BimBuild), Some(json!({"merged": run.answer.get("merged"), "findings": findings.len()})));
}

/// [`run_bim_build`]'s tidy of the runs read: a duct run whose size wasn't read (`size_source`
/// not a tag) and is larger than any tagged run of its system on its level takes the median
/// tagged size; a run over the same two points as one before it (either way, within a foot) is
/// dropped.
/// (Sizes set, runs dropped.)
fn tidy_runs(runs: &mut Vec<cadaddy_walk3d::MepRun>, upf: f64, ducts: &BTreeSet<String>) -> (usize, usize) {
    let mut tagged: BTreeMap<(String, String), Vec<[f64; 2]>> = BTreeMap::new();
    for r in runs.iter().filter(|r| r.size_source == "tag") {
        tagged.entry((r.system.clone(), r.level.clone())).or_default().push(r.size_in);
    }
    let mut sized = 0;
    for r in runs.iter_mut().filter(|r| r.size_source != "tag") {
        let Some(sizes) = tagged.get(&(r.system.clone(), r.level.clone())) else { continue };
        let biggest = sizes.iter().map(|s| s[0].max(s[1])).fold(0.0, f64::max);
        if r.size_in[0].max(r.size_in[1]) > biggest {
            let mut by_area: Vec<[f64; 2]> = sizes.clone();
            by_area.sort_by(|a, b| (a[0] * a[1]).total_cmp(&(b[0] * b[1])));
            r.size_in = by_area[by_area.len() / 2];
            sized += 1;
        }
    }
    let before = runs.len();
    super::runtidy::drop_repeats(runs, upf, &|r| ducts.contains(&r.system));
    (sized, before - runs.len())
}

/// The rooms only the other disciplines' plans tag (rooms stage `other_trade`): each spec room
/// of such a number gets its tag's place on its level's architectural plan (`at`), so the build
/// traces the room there, and the tag's name when it reads as one (a unit type). A door's tag
/// is no room's. How many were anchored.
fn anchor_other_trade_rooms(s: &mut Session, job: &GuidedJob, place: &dyn Fn([f64; 2], &str) -> Option<cadaddy_mepfix::tagid::Sim>) -> Result<usize> {
    let Ok((mut sp, true)) = super::bspec::current(s, "bim_build") else { return Ok(0) };
    let extras: Vec<(String, String, [f64; 2], String)> = job
        .stage(StageName::Rooms)
        .and_then(|st| st.result.as_ref())
        .and_then(|r| r.get("rooms"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| r.get("other_trade").and_then(Value::as_bool) == Some(true))
        .filter_map(|r| {
            let n = r.get("number").and_then(Value::as_str)?.trim().to_string();
            let name = r.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
            let at = [r.get("at")?.get(0)?.as_f64()?, r.get("at")?.get(1)?.as_f64()?];
            Some((n, name, at, r.get("level").and_then(Value::as_str).unwrap_or("").to_string()))
        })
        .collect();
    let mut anchored = 0;
    for (number, name, at, level) in extras {
        if name.to_ascii_uppercase().contains("DOOR") {
            continue;
        }
        let Some(m) = place(at, &level) else { continue };
        let Some(r) = sp.rooms.iter_mut().find(|r| r.at.is_none() && r.number.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(&number)))
        else {
            continue;
        };
        r.at = Some(m.apply(at));
        if r.name.trim().is_empty() && legible_room_name(&name) && super::perceive::geometry::room_name_score(&name) > 0 {
            r.name = name;
        }
        anchored += 1;
    }
    if anchored > 0 {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    Ok(anchored)
}

/// The numbers the other disciplines' plans gave rooms the architectural plans left unnumbered
/// (rooms stage `other_trade`): each put onto its level's plan, then onto the stacked building,
/// numbers the unnumbered room element there. How many were numbered.
fn number_rooms_from_other_trades(
    s: &mut Session,
    job: &GuidedJob,
    place: &dyn Fn([f64; 2], &str) -> Option<cadaddy_mepfix::tagid::Sim>,
) -> Result<(usize, BTreeMap<String, usize>)> {
    let mut missed: BTreeMap<String, usize> = BTreeMap::new();
    let extras: Vec<(String, [f64; 2], String)> = job
        .stage(StageName::Rooms)
        .and_then(|st| st.result.as_ref())
        .and_then(|r| r.get("rooms"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| r.get("other_trade").and_then(Value::as_bool) == Some(true))
        .filter_map(|r| {
            let n = r.get("number").and_then(Value::as_str)?.trim().to_string();
            let at = [r.get("at")?.get(0)?.as_f64()?, r.get("at")?.get(1)?.as_f64()?];
            Some((n, at, r.get("level").and_then(Value::as_str).unwrap_or("").to_string()))
        })
        .collect();
    let Ok((sp, true)) = super::bspec::current(s, "bim_build") else { return Ok((0, missed)) };
    let key = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    // Each level's plan corner; the stack moved each level's model by the lowest's minus its own.
    let corner = |l: &cadaddy_walk3d::Level| l.region.map(|r| cadaddy_geom::Vec2::new(r[0].min(r[2]), r[1].min(r[3])));
    let lowest = sp.levels.iter().filter(|l| corner(l).is_some()).min_by(|a, b| a.elevation_ft.total_cmp(&b.elevation_ft)).and_then(corner);
    let mut rooms = super::bspec::room_elements(s.doc()?);
    // And the architectural plans' own numbers no room took: a number printed by a label the trace
    // couldn't start from (an elevator's, under its ELEV. on the shaft's wall). Not a callout's
    // sheet number, nor one a room of the level has.
    let mut extras = extras;
    extras.extend(stray_plan_numbers(s.doc()?, job, &rooms));
    if extras.is_empty() {
        return Ok((0, missed));
    }
    let mut numbered = 0;
    for (number, at, level) in extras {
        let Some(m) = place(at, &level) else {
            *missed.entry(format!("{level}: not placeable")).or_default() += 1;
            continue;
        };
        let q = m.apply(at);
        let Some(lv) = sp.levels.iter().find(|l| !key(&l.name).is_empty() && key(&l.name) == key(&level)) else {
            *missed.entry(format!("{level}: no such level")).or_default() += 1;
            continue;
        };
        let off = match (lowest, corner(lv)) {
            (Some(a), Some(b)) if sp.stack => a - b,
            _ => cadaddy_geom::Vec2::new(0.0, 0.0),
        };
        let q = cadaddy_geom::Vec2::new(q[0], q[1]) + off;
        // In the room, or (a tag by the room's door, in the corridor) the nearest unnumbered room
        // of the level within 6 feet, when the point is in no room.
        let here = |r: &super::bspec::RoomElement| key(&r.level) == key(&level) && r.outline.len() >= 3;
        let inside = rooms.iter().position(|r| here(r) && cadaddy_geom::point_in_polygon(&r.outline, q));
        let near = || {
            rooms
                .iter()
                .enumerate()
                .filter(|(_, r)| r.number.is_none() && here(r))
                .map(|(i, r)| (i, edge_distance(&r.outline, q)))
                .filter(|(_, d)| *d <= 6.0 * upf)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        };
        let pick = match inside {
            Some(i) if rooms[i].number.is_none() => Some(i),
            Some(_) => None,
            None => near(),
        };
        if number_taken(&rooms, &level, &number) {
            *missed.entry(format!("{level}: number already on the level")).or_default() += 1;
            continue;
        }
        let Some(room) = pick.and_then(|i| rooms.get_mut(i)) else {
            // Already numbered there (its own label), or between rooms.
            let numbered_here = rooms
                .iter()
                .any(|r| r.number.is_some() && key(&r.level) == key(&level) && r.outline.len() >= 3 && cadaddy_geom::point_in_polygon(&r.outline, q));
            // How far the nearest unnumbered room of the level was (whole feet).
            let nearest = rooms
                .iter()
                .filter(|r| r.number.is_none() && key(&r.level) == key(&level) && r.outline.len() >= 3)
                .map(|r| edge_distance(&r.outline, q))
                .fold(f64::INFINITY, f64::min)
                / upf;
            let why = if numbered_here {
                "room already numbered".to_string()
            } else if nearest.is_finite() {
                format!("in no room (an unnumbered one {nearest:.0} ft away)")
            } else {
                "in no room (none unnumbered)".to_string()
            };
            *missed.entry(format!("{level}: {why}")).or_default() += 1;
            continue;
        };
        room.number = Some(number.clone());
        let h = room.handle;
        let lk = key(&room.level);
        s.doc_mut()?.modify_entity(h, |e| {
            let mut x = e.common.xdata.to_vec();
            x.push(cadaddy_doc::RawTag { code: 1000, value: format!("number:{number}") });
            e.common.xdata = x.into();
        })?;
        // The number is that room's: a room of the level known by the number alone ("Room 302",
        // traced from the same label) is the same room again.
        let bare = |r: &super::bspec::RoomElement| {
            r.handle != h
                && key(&r.level) == lk
                && r.number.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(number.trim()))
                && (r.name.trim().eq_ignore_ascii_case("Room") || r.name.trim().eq_ignore_ascii_case(&format!("Room {}", number.trim())))
        };
        let gone: Vec<cadaddy_doc::Handle> = rooms.iter().filter(|r| bare(r)).map(|r| r.handle).collect();
        for g in &gone {
            s.doc_mut()?.remove_entity(*g);
        }
        rooms.retain(|r| !gone.contains(&r.handle));
        numbered += 1;
    }
    Ok((numbered, missed))
}

/// Whether a room of the level (digits of its name) already has the number: one number per level
/// (the other piece of a corridor walls cut in two doesn't take it again). A "Room NNN" traced
/// from the same label doesn't count: it is the room the number goes to, again.
/// The room numbers printed on each level's architectural plan that no room of the level has:
/// (number, where, level), each once.
fn stray_plan_numbers(d: &cadaddy_doc::Drawing, job: &GuidedJob, rooms: &[super::bspec::RoomElement]) -> Vec<(String, [f64; 2], String)> {
    let sheets: BTreeSet<String> = job.sheet_regions.iter().map(|r| sheet_key(&r.number)).collect();
    let plans: Vec<([f64; 4], String)> = job
        .sheet_regions
        .iter()
        .filter(|r| r.discipline.eq_ignore_ascii_case("A") && r.kind == "plan" && r.not_built.is_empty() && !r.level.is_empty())
        .map(|r| (r.plan_rect(), r.level.clone()))
        .collect();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut out = Vec::new();
    if plans.is_empty() {
        return out;
    }
    for e in d.model.iter().take(4_000_000) {
        let cadaddy_doc::EntityKind::Text(t) = &e.kind else { continue };
        let v = t.value.trim();
        if !super::perceive::geometry::room_number(v) || sheets.contains(&v.to_ascii_uppercase()) {
            continue;
        }
        let p = t.insert.xy();
        let Some((_, level)) = plans.iter().find(|(r, _)| p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3]) else { continue };
        if number_taken(rooms, level, v) || !seen.insert((level.clone(), v.to_ascii_uppercase())) {
            continue;
        }
        out.push((v.to_string(), [p.x, p.y], level.clone()));
        if out.len() >= MAX_STAMPED {
            break;
        }
    }
    out
}

fn number_taken(rooms: &[super::bspec::RoomElement], level: &str, number: &str) -> bool {
    let key = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let number = number.trim();
    rooms.iter().any(|r| {
        key(&r.level) == key(level)
            && r.number.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(number))
            && !(r.name.trim().eq_ignore_ascii_case("Room") || r.name.trim().eq_ignore_ascii_case(&format!("Room {number}")))
    })
}

/// How much of a level view's linework (its plan's background and its trade's own lines) must lie on
/// the level's architectural plan, both ways (F1), for the view to be laid there.
const LEVEL_VIEW_F1: f64 = 0.5;

/// A typical-room plan's room type, as the words its title and a room's type name share: QUEEN
/// is Q, ACC./ACCESSIBLE is ADA, SINGLE is dropped (a QUEEN room is the single one), the rest
/// kept for a room name and, for a title, only the words some room type uses.
fn type_words(text: &str, vocabulary: Option<&BTreeSet<String>>) -> BTreeSet<String> {
    let upper = text.to_ascii_uppercase().replace("ACC.", "ADA ");
    upper
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| match w {
            "QUEEN" => "Q".to_string(),
            "ACC" | "ACCESSIBLE" => "ADA".to_string(),
            w => w.to_string(),
        })
        .filter(|w| w != "SINGLE")
        .filter(|w| vocabulary.is_none_or(|v| v.contains(w)))
        .collect()
}

/// Where `p` of a typical-room view (its block of linework `view`) falls in a room whose outline
/// spans `room` (both `[x0, y0, x1, y1]`): the same share across and up, the view turned a quarter
/// when the room runs the other way.
fn into_room(p: [f64; 2], view: [f64; 4], room: [f64; 4]) -> Option<[f64; 2]> {
    let (vw, vh) = (view[2] - view[0], view[3] - view[1]);
    let (rw, rh) = (room[2] - room[0], room[3] - room[1]);
    let k = if (vw > vh) != (rw > rh) { 1 } else { 0 };
    into_room_turned(p, view, room, k)
}

/// [`into_room`] with the view turned `k % 4` quarters and, for `k >= 4`, mirrored first: a
/// typical room's plan drawn as one unit and the rooms of its type facing every way along the
/// corridors (turned half round across the corridor, mirrored either side of a shared wall).
fn into_room_turned(p: [f64; 2], view: [f64; 4], room: [f64; 4], k: u8) -> Option<[f64; 2]> {
    let (vw, vh) = (view[2] - view[0], view[3] - view[1]);
    let (rw, rh) = (room[2] - room[0], room[3] - room[1]);
    if !(vw > 0.0 && vh > 0.0 && rw > 0.0 && rh > 0.0) {
        return None;
    }
    let (u, v) = (((p[0] - view[0]) / vw).clamp(0.0, 1.0), ((p[1] - view[1]) / vh).clamp(0.0, 1.0));
    let u = if k >= 4 { 1.0 - u } else { u };
    let (u, v) = match k % 4 {
        1 => (v, 1.0 - u),
        2 => (1.0 - u, 1.0 - v),
        3 => (1.0 - v, u),
        _ => (u, v),
    };
    Some([room[0] + u * rw, room[1] + v * rh])
}

/// How a typical-room view goes into one room: the turn (and mirror) of [`into_room_turned`] that
/// lays the view's door swings (their hinges, `view_hinges`) nearest the swings the floor plan
/// draws in and round the room (`room_hinges`); the plain quarter-turn rule of [`into_room`]
/// with fewer than two swings to go by.
fn room_turn(view: [f64; 4], room: [f64; 4], view_hinges: &[cadaddy_geom::Vec2], room_hinges: &[cadaddy_geom::Vec2]) -> u8 {
    room_turn_by(view, room, view_hinges, room_hinges, &[], &[], 0.0)
}

/// [`room_turn`], first by the view's own wall devices (`on_walls`: where it draws them): the turn
/// that puts the most of them within `reach` of the room's outline (`outline`, on its walls), then
/// the door swings, then the plain way.
fn room_turn_by(
    view: [f64; 4],
    room: [f64; 4],
    view_hinges: &[cadaddy_geom::Vec2],
    room_hinges: &[cadaddy_geom::Vec2],
    on_walls: &[[f64; 2]],
    outline: &[cadaddy_geom::Vec2],
    reach: f64,
) -> u8 {
    let (vw, vh) = (view[2] - view[0], view[3] - view[1]);
    let (rw, rh) = (room[2] - room[0], room[3] - room[1]);
    let plain = if (vw > vh) != (rw > rh) { 1 } else { 0 };
    let use_devices = on_walls.len() >= 2 && outline.len() >= 3;
    let use_swings = view_hinges.len() >= 2 && room_hinges.len() >= 2;
    if !use_devices && !use_swings {
        return plain;
    }
    let cap = 0.5 * rw.max(rh);
    // Devices off the walls (fewer is better), then the swings' distances.
    let score = |k: u8| -> (usize, f64) {
        let off = if use_devices {
            on_walls
                .iter()
                .filter_map(|p| into_room_turned(*p, view, room, k))
                .filter(|q| edge_distance(outline, cadaddy_geom::Vec2::new(q[0], q[1])) > reach)
                .count()
        } else {
            0
        };
        let swings = if use_swings {
            view_hinges
                .iter()
                .filter_map(|h| into_room_turned([h.x, h.y], view, room, k))
                .map(|q| room_hinges.iter().map(|r| r.dist(cadaddy_geom::Vec2::new(q[0], q[1]))).fold(cap, f64::min))
                .sum()
        } else {
            0.0
        };
        (off, swings)
    };
    // The plain way wins a tie (the same score: a symmetric room).
    let mut best = (score(plain), plain);
    for k in 0..8u8 {
        let sc = score(k);
        if sc.0 < best.0.0 || (sc.0 == best.0.0 && sc.1 < best.0.1 - 1e-6) {
            best = (sc, k);
        }
    }
    best.1
}

/// How a typical-room view goes into one room: laid at its printed scale (`rigid`: turned `k`,
/// its middle `from` onto `to`, both sheets read at their scale notes), else by shares of its box
/// ([`into_room_turned`] with `k`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct RoomFit {
    k: u8,
    from: cadaddy_geom::Vec2,
    to: cadaddy_geom::Vec2,
    rigid: bool,
}

/// `v` turned `k % 4` quarters counter-clockwise, mirrored across first for `k >= 4`.
fn turn_vec(v: cadaddy_geom::Vec2, k: u8) -> cadaddy_geom::Vec2 {
    let v = if k >= 4 { cadaddy_geom::Vec2::new(-v.x, v.y) } else { v };
    match k % 4 {
        1 => cadaddy_geom::Vec2::new(-v.y, v.x),
        2 => cadaddy_geom::Vec2::new(-v.x, -v.y),
        3 => cadaddy_geom::Vec2::new(v.y, -v.x),
        _ => v,
    }
}

/// [`turn_vec`] undone.
fn unturn_vec(v: cadaddy_geom::Vec2, k: u8) -> cadaddy_geom::Vec2 {
    let back = turn_vec(v, (4 - k % 4) % 4);
    if k >= 4 { cadaddy_geom::Vec2::new(-back.x, back.y) } else { back }
}

/// Where a point `q` of the room came from in the view, for a rigid `fit` ([`place_in_room`]
/// undone).
fn view_point(q: [f64; 2], fit: RoomFit) -> Option<[f64; 2]> {
    let p = fit.from + unturn_vec(cadaddy_geom::Vec2::new(q[0], q[1]) - fit.to, fit.k);
    (fit.rigid && p.is_finite()).then_some([p.x, p.y])
}

/// Where `p` of the view `view` goes in a room spanning `room` by `fit`.
fn place_in_room(p: [f64; 2], view: [f64; 4], room: [f64; 4], fit: RoomFit) -> Option<[f64; 2]> {
    if fit.rigid {
        let q = fit.to + turn_vec(cadaddy_geom::Vec2::new(p[0], p[1]) - fit.from, fit.k);
        q.is_finite().then_some([q.x, q.y])
    } else {
        into_room_turned(p, view, room, fit.k)
    }
}

/// A floor plan's linework samples by 20' cells, for laying the typical-room views on the rooms.
struct FloorLines {
    cell: f64,
    cells: std::collections::HashMap<(i64, i64), Vec<cadaddy_geom::Vec2>>,
}

impl FloorLines {
    /// The samples (every `step`) of the plan objects of `d` whose middles lie in one of `rooms`
    /// (boxes) grown by `margin`.
    fn new(d: &cadaddy_doc::Drawing, rooms: &[[f64; 4]], margin: f64, step: f64) -> Self {
        let cell = 240.0 * super::perceive::units_per_inch(d);
        let key = |p: cadaddy_geom::Vec2| ((p.x / cell).floor() as i64, (p.y / cell).floor() as i64);
        let mut wanted: BTreeSet<(i64, i64)> = BTreeSet::new();
        for r in rooms.iter().take(MAX_STAMPED) {
            let (a, b) = (key(cadaddy_geom::Vec2::new(r[0] - margin, r[1] - margin)), key(cadaddy_geom::Vec2::new(r[2] + margin, r[3] + margin)));
            if (b.0 - a.0 + 1) * (b.1 - a.1 + 1) > 400 {
                continue;
            }
            for x in a.0..=b.0 {
                for y in a.1..=b.1 {
                    wanted.insert((x, y));
                }
            }
        }
        let mut cells: std::collections::HashMap<(i64, i64), Vec<cadaddy_geom::Vec2>> = std::collections::HashMap::new();
        if !wanted.is_empty() {
            let ents: Vec<&std::sync::Arc<cadaddy_doc::Entity>> =
                super::bspec::plan_objects(d).into_iter().filter(|e| super::bspec::centre(d, e).is_some_and(|c| wanted.contains(&key(c)))).collect();
            for p in super::grids::line_samples(&ents, step) {
                cells.entry(key(p)).or_default().push(p);
            }
        }
        FloorLines { cell, cells }
    }

    fn in_box(&self, r: [f64; 4]) -> Vec<cadaddy_geom::Vec2> {
        let (x0, y0, x1, y1) = (
            (r[0] / self.cell).floor() as i64,
            (r[1] / self.cell).floor() as i64,
            (r[2] / self.cell).floor() as i64,
            (r[3] / self.cell).floor() as i64,
        );
        if (x1 - x0 + 1) * (y1 - y0 + 1) > 400 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(v) = self.cells.get(&(x, y)) {
                    out.extend(v.iter().filter(|p| p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3]));
                }
            }
        }
        out
    }
}

/// The view's linework (`view_pts`, samples of its block `view`) laid on the floor's around a room
/// (`room`, its outline's box) at the printed scale: each of the eight turns, the offset most
/// samples agree on (to `tol`), the view's middle kept over the room. `None` when too few agree
/// (another scale, or no linework to go by).
fn rigid_room_fit(
    view_pts: &[cadaddy_geom::Vec2],
    view: [f64; 4],
    room: [f64; 4],
    floor: &FloorLines,
    margin: f64,
    tol: f64,
) -> Option<(RoomFit, usize)> {
    if view_pts.len() < MIN_FIT_SAMPLES || tol.is_nan() || tol <= 0.0 {
        return None;
    }
    let from = cadaddy_geom::Vec2::new((view[0] + view[2]) / 2.0, (view[1] + view[3]) / 2.0);
    let near = floor.in_box([room[0] - margin, room[1] - margin, room[2] + margin, room[3] + margin]);
    if near.len() < MIN_FIT_SAMPLES {
        return None;
    }
    let stride = (view_pts.len() / MAX_FIT_SAMPLES).max(1);
    let few: Vec<cadaddy_geom::Vec2> = view_pts.iter().step_by(stride).copied().collect();
    // Each turn's likeliest offsets (the most sample/line pairs agreeing), then the turn and
    // offset whose lines match both ways: the view's samples on the floor's lines and the room's
    // own lines explained by the view, each counted once. Pair counts alone can't tell a turn from
    // its mirror when the walls are alike and only the bath and the door differ.
    let cell = |p: cadaddy_geom::Vec2| ((p.x / tol).floor() as i64, (p.y / tol).floor() as i64);
    let grid_of = |pts: &[cadaddy_geom::Vec2]| {
        let mut g: std::collections::HashMap<(i64, i64), Vec<cadaddy_geom::Vec2>> = std::collections::HashMap::new();
        for p in pts {
            g.entry(cell(*p)).or_default().push(*p);
        }
        g
    };
    let hit = |g: &std::collections::HashMap<(i64, i64), Vec<cadaddy_geom::Vec2>>, p: cadaddy_geom::Vec2| {
        let (cx, cy) = cell(p);
        (-1..=1).any(|dx| (-1..=1).any(|dy| g.get(&(cx + dx, cy + dy)).is_some_and(|v| v.iter().any(|q| q.dist(p) <= tol))))
    };
    let floor_grid = grid_of(&near);
    let inside: Vec<cadaddy_geom::Vec2> =
        near.iter().filter(|f| f.x >= room[0] && f.x <= room[2] && f.y >= room[1] && f.y <= room[3]).copied().take(4 * MAX_FIT_SAMPLES).collect();
    let mut best: Option<(RoomFit, usize, usize)> = None;
    for k in 0..8u8 {
        let turned: Vec<cadaddy_geom::Vec2> = few.iter().map(|p| turn_vec(*p - from, k)).collect();
        let mut votes: std::collections::HashMap<(i64, i64), usize> = std::collections::HashMap::new();
        for t in &turned {
            for f in &near {
                let to = *f - *t;
                if to.x < room[0] || to.x > room[2] || to.y < room[1] || to.y > room[3] {
                    continue;
                }
                *votes.entry(((to.x / tol).round() as i64, (to.y / tol).round() as i64)).or_default() += 1;
            }
        }
        // Its best few offsets (ties to the smaller key: a hash map's order differs run to run).
        let mut ranked: Vec<((i64, i64), usize)> = votes.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (o, votes) in ranked.into_iter().take(FIT_OFFSETS_TRIED) {
            let to = cadaddy_geom::Vec2::new(o.0 as f64 * tol, o.1 as f64 * tol);
            let placed: Vec<cadaddy_geom::Vec2> = turned.iter().map(|t| to + *t).collect();
            let on_lines = placed.iter().filter(|p| hit(&floor_grid, **p)).count();
            let placed_grid = grid_of(&placed);
            let explained = inside.iter().filter(|f| hit(&placed_grid, **f)).count();
            let score = on_lines + explained * few.len() / inside.len().max(1);
            // Only an offset enough samples agree on counts (the vote the fit was always accepted
            // by): the two-way score ranks those, it never turns a fit away.
            let enough = votes * 100 >= few.len() * MIN_FIT_SHARE_PCT;
            if enough && best.as_ref().is_none_or(|b| score > b.2) {
                best = Some((RoomFit { k, from, to, rigid: true }, votes, score));
            }
        }
    }
    best.map(|(f, on, _)| (f, on))
}

/// The offsets each turn of a view is tried at (its most agreed-on), for the two-way match.
const FIT_OFFSETS_TRIED: usize = 3;

/// How far round a room its floor's linework is looked at to lay a typical view in it, feet.
const ROOM_FIT_MARGIN_FT: f64 = 3.0;

/// A typical view's samples looked at for its fit into a room, at most; at least this many on each
/// side; and the share of them (%) that must land on the floor's lines.
const MAX_FIT_SAMPLES: usize = 600;
const MIN_FIT_SAMPLES: usize = 20;
const MIN_FIT_SHARE_PCT: usize = 35;

/// How far round a room its floor plan's door swings are looked for, feet (a door's hinge is on
/// its wall; a neighbour's entry beside it is no further than that wall).
const ROOM_SWING_FT: f64 = 1.0;

/// A typical room's device this close to the room's outline (feet) is on its wall.
const ROOM_WALL_FT: f64 = 1.5;

/// The door swings' hinges of a drawing's model space (`k` units per inch), for turning the
/// typical-room views into their rooms.
fn plan_hinges(d: &cadaddy_doc::Drawing) -> Vec<cadaddy_geom::Vec2> {
    let k = super::perceive::units_per_inch(d);
    let ents: Vec<&std::sync::Arc<cadaddy_doc::Entity>> =
        d.model.iter().filter(|e| matches!(e.kind, cadaddy_doc::EntityKind::Arc(_) | cadaddy_doc::EntityKind::LwPolyline(_))).collect();
    super::wallgen::swing_hinges(&ents, k)
}

/// The hinges among `all` inside `r` (`[x0, y0, x1, y1]`) grown by `m`.
fn hinges_in(all: &[cadaddy_geom::Vec2], r: [f64; 4], m: f64) -> Vec<cadaddy_geom::Vec2> {
    all.iter().filter(|h| h.x >= r[0] - m && h.x <= r[2] + m && h.y >= r[1] - m && h.y <= r[3] + m).copied().collect()
}

/// The devices of the typical-room plans (UNIT ELECTRICAL PLAN - DOUBLE QUEEN, ENLARGED ACC.
/// QUEEN) that no plan registration placed: each put into every room of the type its view's
/// title names, at the same place in the room. A room the floor's own plans already put devices
/// in is left alone (they show its devices: no second count). With the stamped count per view.
fn stamp_typical_rooms(
    d: &cadaddy_doc::Drawing,
    job: &GuidedJob,
    placed: &[super::readers::ReadDevice],
    unplaced: &[&super::readers::ReadDevice],
) -> StampedRooms {
    let mut out = Vec::new();
    let mut report: BTreeMap<String, usize> = BTreeMap::new();
    // The rooms a one-room plan is the authority in (its level and outline).
    let mut authority: Vec<(String, Vec<cadaddy_geom::Vec2>)> = Vec::new();
    // Each stamped room's panels, where its plan letters them ("PNLBD 'G'"), by the room.
    let mut panels: BTreeMap<String, Vec<(String, [f64; 2])>> = BTreeMap::new();
    let mut panel_rooms: BTreeMap<String, (String, Vec<cadaddy_geom::Vec2>, bool)> = BTreeMap::new();
    let mut sleeping: BTreeMap<usize, (String, Vec<cadaddy_geom::Vec2>)> = BTreeMap::new();
    if unplaced.is_empty() {
        return StampedRooms {
            devices: out,
            report,
            authority,
            panels,
            rooms: panel_rooms,
            sleeping: sleeping.into_values().collect(),
            footprints: Vec::new(),
            duplicates: BTreeSet::new(),
        };
    }
    // The rooms stage's rooms with outlines; a name several rooms carry is a unit type.
    let rooms = job_rooms(job);
    let (types, vocabulary) = room_types(&rooms);
    let bbox = |o: &[cadaddy_geom::Vec2]| {
        o.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
            [b[0].min(q.x), b[1].min(q.y), b[2].max(q.x), b[3].max(q.y)]
        })
    };
    // A room the floor's plans already put devices in: two or more.
    let furnished = |level: &str, o: &[cadaddy_geom::Vec2]| {
        placed.iter().filter(|dv| dv.level == level && cadaddy_geom::point_in_polygon(o, cadaddy_geom::Vec2::new(dv.at[0], dv.at[1]))).take(2).count()
            >= 2
    };
    // A device of a tag the floor's plans already put where a typical plan's copy lands (within
    // [`STAND_IN_FT`]) is that device: the typical plan's copy is the unit's (its room, its
    // circuit), and the floor plan's is left out as its duplicate (one each, so two units'
    // devices back to back on their shared wall keep one each whichever is stamped first).
    let stand_in_reach = STAND_IN_FT * 12.0 * super::perceive::units_per_inch(d);
    // Not one on another panel's circuit (a corridor's receptacle on a house panel by the unit's
    // entry receptacle).
    let panel = |c: &str| super::propfill::panel_of(c).map(|p| p.to_ascii_uppercase());
    let stand_in = |level: &str, typ: &super::readers::ReadDevice, at: [f64; 2], taken: &BTreeSet<usize>| {
        let theirs = panel(&typ.circuit);
        placed
            .iter()
            .enumerate()
            .filter(|(j, dv)| !taken.contains(j) && dv.level == level && dv.tag.eq_ignore_ascii_case(&typ.tag))
            .filter(|(_, dv)| match (panel(&dv.circuit), &theirs) {
                (Some(a), Some(b)) => a == *b,
                _ => true,
            })
            .map(|(j, dv)| (j, (dv.at[0] - at[0]).hypot(dv.at[1] - at[1])))
            .filter(|(_, dd)| *dd <= stand_in_reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(j, _)| j)
    };
    let mut stood: BTreeSet<usize> = BTreeSet::new();
    let mut footprints: Vec<Footprint> = Vec::new();
    let refs = job_sheet_refs(job);
    // The floor plans' door swings and linework round the rooms, to lay each typical view into
    // each room the way it faces.
    let floor_hinges = plan_hinges(d);
    let k_in = super::perceive::units_per_inch(d);
    let near = ROOM_SWING_FT * 12.0 * k_in;
    let room_boxes: Vec<[f64; 4]> = rooms.iter().map(|(_, _, o, _)| bbox(o)).collect();
    let floor_lines = FloorLines::new(d, &room_boxes, ROOM_FIT_MARGIN_FT * 12.0 * k_in, 12.0 * k_in);
    let mut by_sheet: BTreeMap<String, Vec<&super::readers::ReadDevice>> = BTreeMap::new();
    for dv in unplaced {
        by_sheet.entry(sheet_key(&dv.sheet)).or_default().push(dv);
    }
    for (sheet, devices) in by_sheet.into_iter().filter(|(_, v)| v.len() >= 3).take(MAX_STAMP_SHEETS) {
        let Some(sh) = refs.iter().find(|r| sheet_key(&r.number) == sheet && r.view.is_none()) else { continue };
        let sub = super::printmep::sheet_drawing(d, sh);
        let sheet_hinges = plan_hinges(&sub);
        let mut turns: BTreeMap<([u64; 4], usize), RoomFit> = BTreeMap::new();
        // The sheet's devices (where each view draws its own), and each view's linework.
        let drawn: Vec<[f64; 2]> = devices.iter().map(|dv| dv.at).collect();
        let reach = ROOM_WALL_FT * 12.0 * k_in;
        let sheet_objects = super::bspec::plan_objects(&sub);
        let mut view_lines: BTreeMap<[u64; 4], Vec<cadaddy_geom::Vec2>> = BTreeMap::new();
        // Each view's unit: its plan's linework, the stray few (a leader, a title's rule) left off.
        let mut unit_boxes: BTreeMap<[u64; 4], Option<[f64; 4]>> = BTreeMap::new();
        let mut unit_box = |view: &[f64; 4]| -> Option<[f64; 4]> {
            *unit_boxes.entry(view.map(f64::to_bits)).or_insert_with(|| {
                let mine: Vec<&std::sync::Arc<cadaddy_doc::Entity>> = sheet_objects
                    .iter()
                    .filter(|e| super::bspec::centre(&sub, e).is_some_and(|c| c.x >= view[0] && c.x <= view[2] && c.y >= view[1] && c.y <= view[3]))
                    .copied()
                    .collect();
                let pts = super::grids::line_samples(&mine, 12.0 * k_in);
                if pts.len() < 8 {
                    return None;
                }
                let cut = |mut v: Vec<f64>| {
                    v.sort_by(f64::total_cmp);
                    let k = v.len() / 40;
                    (v.get(k).copied().unwrap_or(0.0), v.get(v.len().saturating_sub(1 + k)).copied().unwrap_or(0.0))
                };
                let (x0, x1) = cut(pts.iter().map(|p| p.x).collect());
                let (y0, y1) = cut(pts.iter().map(|p| p.y).collect());
                (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
            })
        };
        let mut turn_of = |view: &[f64; 4], ri: usize, outline: &[cadaddy_geom::Vec2]| -> RoomFit {
            *turns.entry((view.map(f64::to_bits), ri)).or_insert_with(|| {
                let lines = view_lines.entry(view.map(f64::to_bits)).or_insert_with(|| {
                    let mine: Vec<&std::sync::Arc<cadaddy_doc::Entity>> = sheet_objects
                        .iter()
                        .filter(|e| {
                            super::bspec::centre(&sub, e).is_some_and(|c| c.x >= view[0] && c.x <= view[2] && c.y >= view[1] && c.y <= view[3])
                        })
                        .copied()
                        .collect();
                    super::grids::line_samples(&mine, 12.0 * k_in)
                });
                if let Some((fit, _)) = rigid_room_fit(lines, *view, bbox(outline), &floor_lines, ROOM_FIT_MARGIN_FT * 12.0 * k_in, 6.0 * k_in) {
                    return fit;
                }
                let mine: Vec<[f64; 2]> =
                    drawn.iter().filter(|p| p[0] >= view[0] && p[0] <= view[2] && p[1] >= view[1] && p[1] <= view[3]).copied().collect();
                let k = room_turn_by(
                    *view,
                    bbox(outline),
                    &hinges_in(&sheet_hinges, *view, 0.0),
                    &hinges_in(&floor_hinges, bbox(outline), near),
                    &mine,
                    outline,
                    reach,
                );
                RoomFit { k, from: cadaddy_geom::Vec2::new(0.0, 0.0), to: cadaddy_geom::Vec2::new(0.0, 0.0), rigid: false }
            })
        };
        // The typical-room views (a room type), and the views of one room (its number or its name
        // in the title: "ENLARGED BOH ROOM #237", "1ST FLOOR - MEP ROOM").
        let sheet_texts = cadaddy_mepfix::sheet_facts::texts(&sub);
        let lettered: Vec<(String, [f64; 2])> =
            sheet_texts.iter().filter_map(|t| Some((super::printcond::panel_lettering(&t.value)?, [t.at.x, t.at.y]))).take(MAX_STAMPED).collect();
        let panels_in = |view: &[f64; 4]| -> Vec<(String, [f64; 2])> {
            lettered.iter().filter(|(_, p)| p[0] >= view[0] && p[0] <= view[2] && p[1] >= view[1] && p[1] <= view[3]).cloned().collect()
        };
        let panel_into = |room: String,
                          typical: bool,
                          level: &str,
                          view: &[f64; 4],
                          outline: &[cadaddy_geom::Vec2],
                          turn: RoomFit,
                          panels: &mut BTreeMap<String, Vec<(String, [f64; 2])>>,
                          panel_rooms: &mut BTreeMap<String, (String, Vec<cadaddy_geom::Vec2>, bool)>| {
            if panels.contains_key(&room) {
                return;
            }
            let at: Vec<(String, [f64; 2])> =
                panels_in(view).into_iter().filter_map(|(n, p)| Some((n, place_in_room(p, *view, bbox(outline), turn)?))).collect();
            if !at.is_empty() {
                panel_rooms.insert(room.clone(), (level.to_string(), outline.to_vec(), typical));
            }
            panels.insert(room, at);
        };
        let (views, one_room) = room_views(&sub, &sheet_texts, &types, &vocabulary, &rooms);
        let area = |r: &[f64; 4]| (r[2] - r[0]) * (r[3] - r[1]);
        for dv in devices {
            let inside = |r: &[f64; 4]| dv.at[0] >= r[0] && dv.at[0] <= r[2] && dv.at[1] >= r[1] && dv.at[1] <= r[3];
            // A view of one room (the smallest holding it, unless a typical-room view is smaller):
            // into that room, and the authority there (the overall plan's copies there go).
            let typical = views.iter().filter(|(_, r, _)| inside(r)).map(|(_, r, _)| area(r)).fold(f64::INFINITY, f64::min);
            if let Some((title, view, i)) =
                one_room.iter().filter(|(_, r, _)| inside(r) && area(r) < typical).min_by(|a, b| area(&a.1).total_cmp(&area(&b.1)))
            {
                if let Some((name, level, outline, number)) = rooms.get(*i)
                    && out.len() < MAX_STAMPED
                    && let turn = turn_of(view, *i, outline)
                    && let Some(at) = place_in_room(dv.at, *view, bbox(outline), turn)
                {
                    if furnished(level, outline) && !authority.iter().any(|(l, o)| l == level && o == outline) {
                        authority.push((level.clone(), outline.clone()));
                    }
                    let mut copy = (*dv).clone();
                    copy.at = at;
                    copy.level.clone_from(level);
                    copy.room = stamped_room(name, number, *i);
                    panel_into(copy.room.clone(), false, level, view, outline, turn, &mut panels, &mut panel_rooms);
                    out.push(copy);
                    *report.entry(format!("{sheet} {title}")).or_default() += 1;
                }
                continue;
            }
            let Some((title, view, words)) = views
                .iter()
                .filter(|(_, r, _)| inside(r))
                .min_by(|a, b| ((a.1[2] - a.1[0]) * (a.1[3] - a.1[1])).total_cmp(&((b.1[2] - b.1[0]) * (b.1[3] - b.1[1]))))
            else {
                if !views.is_empty() || !one_room.is_empty() {
                    *report.entry(format!("{sheet}: in no typical-room view")).or_default() += 1;
                }
                continue;
            };
            for (ri, (name, level, outline, number)) in rooms.iter().enumerate().filter(|(_, (n, _, _, _))| type_words(n, None) == *words) {
                if out.len() >= MAX_STAMPED {
                    continue;
                }
                let turn = turn_of(view, ri, outline);
                let Some(at) = place_in_room(dv.at, *view, bbox(outline), turn) else { continue };
                if let Some(j) = stand_in(level, dv, at, &stood) {
                    stood.insert(j);
                }
                let room = stamped_room(name, number, ri);
                if !footprints.iter().any(|(l, r, _, _)| l == level && *r == room)
                    && let Some(b) = unit_box(view)
                {
                    let corners: Vec<cadaddy_geom::Vec2> = [[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]
                        .iter()
                        .filter_map(|c| place_in_room(*c, *view, bbox(outline), turn))
                        .map(|q| cadaddy_geom::Vec2::new(q[0], q[1]))
                        .collect();
                    if corners.len() == 4 {
                        footprints.push((level.clone(), room.clone(), corners, outline.clone()));
                    }
                }
                let mut copy = (*dv).clone();
                copy.at = at;
                copy.level.clone_from(level);
                copy.room = room;
                panel_into(copy.room.clone(), true, level, view, outline, turn, &mut panels, &mut panel_rooms);
                if sleeping_room(words) {
                    sleeping.entry(ri).or_insert_with(|| (level.clone(), outline.clone()));
                }
                out.push(copy);
                *report.entry(format!("{sheet} {title}")).or_default() += 1;
            }
        }
    }
    StampedRooms {
        devices: out,
        report,
        authority,
        panels,
        rooms: panel_rooms,
        sleeping: sleeping.into_values().collect(),
        footprints,
        duplicates: stood,
    }
}

/// The mounting heights the typical rooms' plans print by their devices ("44\" AFF",
/// "MICROWAVE 65\" AFF" on a leader, "10\" AFF HORIZ") onto the devices stamped into the rooms
/// of each type: each note's device (where its leader ends, else the round symbol beside it, else
/// the note's own place) laid into every room of the view's type as the view goes into it, the
/// device stamped into that room nearest it (within [`AFF_DEVICE_FT`]) takes the height. The notes
/// read and the devices that took one.
fn typical_heights(d: &cadaddy_doc::Drawing, job: &GuidedJob, devices: &mut [super::readers::ReadDevice]) -> (usize, usize) {
    let rooms = job_rooms(job);
    let (types, vocabulary) = room_types(&rooms);
    if types.is_empty() || devices.is_empty() {
        return (0, 0);
    }
    let k_in = super::perceive::units_per_inch(d);
    let bbox = |o: &[cadaddy_geom::Vec2]| {
        o.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
            [b[0].min(q.x), b[1].min(q.y), b[2].max(q.x), b[3].max(q.y)]
        })
    };
    let room_boxes: Vec<[f64; 4]> = rooms.iter().map(|(_, _, o, _)| bbox(o)).collect();
    let floor_lines = FloorLines::new(d, &room_boxes, ROOM_FIT_MARGIN_FT * 12.0 * k_in, 12.0 * k_in);
    let (mut notes_read, mut set) = (0usize, 0usize);
    for sh in job_sheet_refs(job).iter().filter(|r| r.view.is_none()).take(MAX_STAMP_SHEETS) {
        let sub = super::printmep::sheet_drawing(d, sh);
        let texts = cadaddy_mepfix::sheet_facts::texts(&sub);
        let notes: Vec<(&cadaddy_mepfix::sheet_facts::TextItem, f64)> = texts
            .iter()
            .filter_map(|t| Some((t, super::propfill::mount_height_in(&t.value)?)))
            .filter(|(_, h)| *h > 0.0 && *h < 240.0)
            .take(MAX_STAMPED)
            .collect();
        if notes.is_empty() {
            continue;
        }
        let (views, _) = room_views(&sub, &texts, &types, &vocabulary, &rooms);
        if views.is_empty() {
            continue;
        }
        // Every straight piece (a leader ends on its symbol: no reader of linework keeps those).
        let mut segs: Vec<[[f64; 2]; 2]> = Vec::new();
        for e in sub.model.iter() {
            if segs.len() >= MAX_LEADER_PIECES {
                break;
            }
            match &e.kind {
                cadaddy_doc::EntityKind::Line(l) => segs.push([[l.a.x, l.a.y], [l.b.x, l.b.y]]),
                cadaddy_doc::EntityKind::LwPolyline(pl) if !pl.closed && pl.vertices.len() <= 8 => {
                    segs.extend(pl.vertices.windows(2).filter_map(|w| Some([[w.first()?.p.x, w.first()?.p.y], [w.get(1)?.p.x, w.get(1)?.p.y]])));
                }
                _ => {}
            }
        }
        let rounds = cadaddy_mepfix::sheet_reader::round_shapes(&sub);
        let objects = super::bspec::plan_objects(&sub);
        let mut view_lines: BTreeMap<[u64; 4], Vec<cadaddy_geom::Vec2>> = BTreeMap::new();
        let mut fits: BTreeMap<([u64; 4], usize), Option<RoomFit>> = BTreeMap::new();
        let inside = |r: &[f64; 4], p: cadaddy_geom::Vec2| p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3];
        for (t, h) in &notes {
            let Some((_, view, words)) = views
                .iter()
                .filter(|(_, r, _)| inside(r, t.at))
                .min_by(|a, b| ((a.1[2] - a.1[0]) * (a.1[3] - a.1[1])).total_cmp(&((b.1[2] - b.1[0]) * (b.1[3] - b.1[1]))))
            else {
                continue;
            };
            notes_read += 1;
            // The note's box (its lettering along its direction).
            let th = t.height.max(1e-9);
            let dir = cadaddy_geom::Vec2::from_angle(t.rotation);
            let end = t.at + dir * (NOTE_CHAR_W * th * t.value.chars().count() as f64);
            let near_note = |p: cadaddy_geom::Vec2| {
                let ab = end - t.at;
                let len2 = ab.x * ab.x + ab.y * ab.y;
                let u = if len2 > 0.0 { (((p - t.at).x * ab.x + (p - t.at).y * ab.y) / len2).clamp(0.0, 1.0) } else { 0.0 };
                p.dist(t.at + ab * u) <= 1.5 * th
            };
            // A leader from the note: its far end. Else the round symbol nearest the note.
            let leader = segs
                .iter()
                .filter_map(|sg| {
                    let (a, b) = (cadaddy_geom::Vec2::new(sg[0][0], sg[0][1]), cadaddy_geom::Vec2::new(sg[1][0], sg[1][1]));
                    if a.dist(b) < 2.0 * th {
                        return None;
                    }
                    if near_note(a) && !near_note(b) {
                        Some(b)
                    } else if near_note(b) && !near_note(a) {
                        Some(a)
                    } else {
                        None
                    }
                })
                .max_by(|x, y| x.dist(t.at).total_cmp(&y.dist(t.at)));
            let reach = AFF_SYMBOL_FT * 12.0 * k_in;
            let symbol = |p: cadaddy_geom::Vec2, r: f64| {
                rounds
                    .iter()
                    .map(|(c, _)| cadaddy_geom::Vec2::new(c[0], c[1]))
                    .filter(|c| c.dist(p) <= r)
                    .min_by(|a, b| a.dist(p).total_cmp(&b.dist(p)))
            };
            let at = match leader {
                Some(tip) => symbol(tip, 0.5 * reach).unwrap_or(tip),
                None => symbol(t.at, reach).unwrap_or(t.at),
            };
            let note = format!("{} on {}", t.value.trim(), sh.number);
            for (ri, (name, level, outline, number)) in rooms.iter().enumerate().filter(|(_, (n, _, _, _))| type_words(n, None) == *words) {
                let b = bbox(outline);
                let fit = *fits.entry((view.map(f64::to_bits), ri)).or_insert_with(|| {
                    let lines = view_lines.entry(view.map(f64::to_bits)).or_insert_with(|| {
                        let mine: Vec<&std::sync::Arc<cadaddy_doc::Entity>> =
                            objects.iter().filter(|e| super::bspec::centre(&sub, e).is_some_and(|c| inside(view, c))).copied().collect();
                        super::grids::line_samples(&mine, 12.0 * k_in)
                    });
                    rigid_room_fit(lines, *view, b, &floor_lines, ROOM_FIT_MARGIN_FT * 12.0 * k_in, 6.0 * k_in).map(|f| f.0)
                });
                // Only where the view lies on the room by its lines: a share of its box is no place to
                // tell one device from the next.
                let Some(fit) = fit else { continue };
                let Some(q) = place_in_room([at.x, at.y], *view, b, fit) else { continue };
                let q = cadaddy_geom::Vec2::new(q[0], q[1]);
                let label = stamped_room(name, number, ri);
                let best = devices
                    .iter_mut()
                    .filter(|dv| dv.room == label && dv.level == *level && dv.aff_in.is_none())
                    .map(|dv| {
                        let dd = q.dist(cadaddy_geom::Vec2::new(dv.at[0], dv.at[1]));
                        (dd, dv)
                    })
                    .filter(|(dd, _)| *dd <= AFF_DEVICE_FT * 12.0 * k_in)
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                if let Some((_, dv)) = best {
                    dv.aff_in = Some(*h);
                    dv.aff_note.clone_from(&note);
                    set += 1;
                }
            }
        }
    }
    (notes_read, set)
}

/// A height note's device is the round symbol within this of the note (feet; half of it from a
/// leader's end); a stamped device within [`AFF_DEVICE_FT`] of where that lands in a room takes it.
const AFF_SYMBOL_FT: f64 = 2.5;
const AFF_DEVICE_FT: f64 = 2.5;
/// A capital's width, in text heights (a note's box along its line).
const NOTE_CHAR_W: f64 = 0.8;
/// Straight pieces of a sheet looked at for height notes' leaders, at most.
const MAX_LEADER_PIECES: usize = 400_000;

/// The ceilings the typical rooms' reflected ceiling plans print ("UNIT RCP - SINGLE QUEEN": a
/// height tag in each area, the bath lower than the room): each view laid into every room of its
/// type by its linework, each tag's area of the floor there (the floor's traced space round it)
/// at its height, the room's own when the tag lands in the room's outline. The areas and the tags
/// read.
fn typical_rcp_areas(d: &cadaddy_doc::Drawing, sp: &cadaddy_walk3d::BuildingSpec, job: &GuidedJob) -> (Vec<cadaddy_walk3d::CeilingArea>, usize) {
    let mut floors: BTreeMap<String, Option<super::wallgen::Floors>> = BTreeMap::new();
    typical_rcp_areas_on(d, sp, job, &mut |level, q| {
        let grid = floors.entry(level.to_string()).or_insert_with(|| super::wallgen::room_floors(d, sp, level));
        let Some(Ok((_, rects))) = grid.as_mut().map(|g| g.room_at(q)) else { return None };
        Some(rects.iter().map(|[a, c]| [[a.x, a.y], [c.x, c.y]]).collect())
    })
}

/// [`typical_rcp_areas`], the floor's traced space round a point (its rectangles) from `area_at`.
fn typical_rcp_areas_on(
    d: &cadaddy_doc::Drawing,
    sp: &cadaddy_walk3d::BuildingSpec,
    job: &GuidedJob,
    area_at: &mut dyn FnMut(&str, cadaddy_geom::Vec2) -> Option<Vec<[[f64; 2]; 2]>>,
) -> (Vec<cadaddy_walk3d::CeilingArea>, usize) {
    let rooms = job_rooms(job);
    let (types, vocabulary) = room_types(&rooms);
    let mut out: Vec<cadaddy_walk3d::CeilingArea> = Vec::new();
    if types.is_empty() {
        return (out, 0);
    }
    let k_in = super::perceive::units_per_inch(d);
    let bbox = |o: &[cadaddy_geom::Vec2]| {
        o.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
            [b[0].min(q.x), b[1].min(q.y), b[2].max(q.x), b[3].max(q.y)]
        })
    };
    let room_boxes: Vec<[f64; 4]> = rooms.iter().map(|(_, _, o, _)| bbox(o)).collect();
    let floor_lines = FloorLines::new(d, &room_boxes, ROOM_FIT_MARGIN_FT * 12.0 * k_in, 12.0 * k_in);
    let mut read = 0usize;
    let inside = |r: &[f64; 4], p: cadaddy_geom::Vec2| p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3];
    for sh in job_sheet_refs(job).iter().filter(|r| r.view.is_none()).take(MAX_STAMP_SHEETS) {
        let sub = super::printmep::sheet_drawing(d, sh);
        let texts = cadaddy_mepfix::sheet_facts::texts(&sub);
        // A height in a ceiling tag: its type letter just over it (a dimension string has none).
        let typed = |t: &cadaddy_mepfix::sheet_facts::TextItem| {
            let h = t.height.max(1e-6);
            texts.iter().take(MAX_TAG_TEXTS).any(|o| {
                let v = o.value.trim();
                (1..=3).contains(&v.len())
                    && v.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                    && v.chars().all(|c| c.is_ascii_alphanumeric())
                    && (o.at.x - t.at.x).abs() <= CEILING_TAG_REACH * h
                    && o.at.y > t.at.y
                    && o.at.y - t.at.y <= CEILING_TAG_REACH * h
            })
        };
        let tags: Vec<(cadaddy_geom::Vec2, f64)> = texts
            .iter()
            .filter_map(|t| Some((t, ceiling_height_tag(&t.value)?)))
            .filter(|(t, _)| typed(t))
            .map(|(t, ft)| (t.at, ft))
            .take(MAX_STAMPED)
            .collect();
        if tags.is_empty() {
            continue;
        }
        // The typical rooms' ceiling plans: a room type and RCP (or REFLECTED CEILING) in the title.
        let views: Vec<([f64; 4], BTreeSet<String>, String)> = cadaddy_mepfix::sheet_facts::view_titles(&sub, &texts)
            .into_iter()
            .filter_map(|v| {
                let up = v.title.to_ascii_uppercase();
                let rcp = up.contains("REFLECTED CEILING") || up.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "RCP");
                let words = type_words(&v.title, Some(&vocabulary));
                (rcp && types.contains(&words)).then_some((v.region?, words, v.title.clone()))
            })
            .collect();
        if views.is_empty() {
            continue;
        }
        let objects = super::bspec::plan_objects(&sub);
        for (view, words, title) in &views {
            let mine: Vec<(cadaddy_geom::Vec2, f64)> = tags.iter().filter(|(p, _)| inside(view, *p)).copied().collect();
            if mine.is_empty() {
                continue;
            }
            read += mine.len();
            let ents: Vec<&std::sync::Arc<cadaddy_doc::Entity>> =
                objects.iter().filter(|e| super::bspec::centre(&sub, e).is_some_and(|c| inside(view, c))).copied().collect();
            let lines = super::grids::line_samples(&ents, 12.0 * k_in);
            for (ri, (name, level, outline, number)) in rooms.iter().enumerate().filter(|(_, (n, _, _, _))| type_words(n, None) == *words) {
                let Some(lv) = sp.level(level) else { continue };
                let b = bbox(outline);
                let Some((fit, _)) = rigid_room_fit(&lines, *view, b, &floor_lines, ROOM_FIT_MARGIN_FT * 12.0 * k_in, 6.0 * k_in) else { continue };
                for (p, ft) in &mine {
                    if lv.height_ft.is_finite() && lv.height_ft > 0.0 && *ft > lv.height_ft - CEILING_UNDER_FLOOR_FT {
                        continue;
                    }
                    let Some(q) = place_in_room([p.x, p.y], *view, b, fit) else { continue };
                    let q = cadaddy_geom::Vec2::new(q[0], q[1]);
                    let Some(rects) = area_at(&lv.name, q) else { continue };
                    let area = cadaddy_walk3d::Circulation { rects, ..Default::default() }.outline();
                    if area.len() < 3 || out.iter().any(|a| a.level == lv.name && a.outline == area) {
                        continue;
                    }
                    let room = cadaddy_geom::point_in_polygon(outline, q).then(|| stamped_room(name, number, ri));
                    out.push(cadaddy_walk3d::CeilingArea {
                        level: lv.name.clone(),
                        room,
                        outline: area,
                        height_ft: *ft,
                        grid: None,
                        ceiling_type: None,
                        guess: None,
                        source: Some(format!("{} ceiling tag on {} {title}", ft_in_label(*ft), sh.number)),
                    });
                    if out.len() >= MAX_STAMPED {
                        return (main_areas_only(out), read);
                    }
                }
            }
        }
    }
    (main_areas_only(out), read)
}

/// How far (in its text heights) a ceiling tag's type letter is over its height.
const CEILING_TAG_REACH: f64 = 3.0;
/// Texts of a sheet looked through for a tag's type letter, at most.
const MAX_TAG_TEXTS: usize = 200_000;
/// How far apart (feet, across) an elevation's ceiling and floor level markers may be.
const ELEVATION_MARKER_FT: f64 = 3.0;

/// Each typical room type's ceiling height measured on its interior elevations ("SINGLE QUEEN -
/// BED WALL": the F.F. level line to the B.O. CEILING one, the view at its printed scale), for a
/// type whose ceiling plan prints none: the type's words, the height (feet, the middle of what
/// its elevations measure) and where it was measured.
fn typical_elevation_heights(d: &cadaddy_doc::Drawing, job: &GuidedJob) -> Vec<(BTreeSet<String>, f64, String)> {
    use cadaddy_mepfix::sheet_facts::TextItem;
    let rooms = job_rooms(job);
    let (types, vocabulary) = room_types(&rooms);
    let mut found: BTreeMap<BTreeSet<String>, Vec<(f64, String)>> = BTreeMap::new();
    if types.is_empty() {
        return Vec::new();
    }
    let upf = 12.0 * super::perceive::units_per_inch(d);
    let inside = |r: &[f64; 4], p: cadaddy_geom::Vec2| p.x >= r[0] && p.x <= r[2] && p.y >= r[1] && p.y <= r[3];
    let squash = |v: &str| -> String { v.to_ascii_uppercase().chars().filter(|c| !c.is_whitespace()).collect() };
    let is_ceiling = |v: &str| {
        let u = squash(v);
        u.starts_with("B.O.CEILING") || u.starts_with("BOTTOMOFCEILING") || u.starts_with("B.O.CLG")
    };
    let is_floor = |v: &str| {
        let u = squash(v);
        u == "F.F." || u == "FF" || u.starts_with("FIN.FLOOR") || u.starts_with("FINISHFLOOR")
    };
    for sh in job_sheet_refs(job).iter().filter(|r| r.view.is_none()).take(MAX_STAMP_SHEETS) {
        let sub = super::printmep::sheet_drawing(d, sh);
        let texts = cadaddy_mepfix::sheet_facts::texts(&sub);
        if !texts.iter().any(|t| is_ceiling(&t.value)) {
            continue;
        }
        // The sheet came in at its plans' scale (its typical plans lie on the floor one to one): a
        // view at another is that much bigger or smaller. The scale most of its plan views print,
        // else the one the set's classification gives the sheet.
        let scale_of = |v: &cadaddy_mepfix::sheet_facts::ViewTitle| {
            v.scale_text.as_deref().and_then(cadaddy_mepfix::sheet_reader::parse_scale).filter(|x| x.is_finite() && *x > 0.0)
        };
        let views = cadaddy_mepfix::sheet_facts::view_titles(&sub, &texts);
        let mut plan_scales: BTreeMap<u64, usize> = BTreeMap::new();
        // (A title wrapped onto two lines reads as its second, "DOUBLE QUEEN - ADA (TUB)": a view
        // naming a room type that isn't an elevation, section or detail is one of its plans too.)
        for v in views.iter().filter(|v| {
            let up = v.title.to_ascii_uppercase();
            let side_view = [" WALL", "ELEVATION", "SECTION", "DETAIL"].iter().any(|w| up.contains(w));
            up.contains("PLAN")
                || up.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "RCP")
                || (!side_view && types.contains(&type_words(&v.title, Some(&vocabulary))))
        }) {
            if let Some(x) = scale_of(v) {
                *plan_scales.entry(x.to_bits()).or_default() += 1;
            }
        }
        let sheet_scale = plan_scales
            .iter()
            .max_by_key(|(_, n)| **n)
            .map(|(b, _)| f64::from_bits(*b))
            .or_else(|| sh.scale_text.as_deref().and_then(cadaddy_mepfix::sheet_reader::parse_scale).filter(|x| x.is_finite() && *x > 0.0));
        for v in views {
            let view_scale = scale_of(&v);
            let factor = match (sheet_scale, view_scale) {
                (Some(a), Some(b)) => a / b,
                _ => 1.0,
            };
            let up = v.title.to_ascii_uppercase();
            let elevation = up.contains(" WALL") || up.contains("ELEVATION");
            let words = type_words(&v.title, Some(&vocabulary));
            let Some(r) = v.region.filter(|_| elevation && !up.contains("RCP") && types.contains(&words)) else { continue };
            // Its level markers' labels stand over (and under) their lines, past the linework.
            let m = ELEVATION_MARKER_FT * upf;
            let region = [r[0] - m, r[1] - m, r[2] + m, r[3] + m];
            let ceilings: Vec<&TextItem> = texts.iter().filter(|t| inside(&region, t.at) && is_ceiling(&t.value)).collect();
            let floors: Vec<&TextItem> = texts.iter().filter(|t| inside(&region, t.at) && is_floor(&t.value)).collect();
            for c in ceilings.iter().take(8) {
                // Its floor line: the F.F. marker under it, at its side of the view.
                let Some(f) = floors
                    .iter()
                    .filter(|f| f.at.y < c.at.y && (f.at.x - c.at.x).abs() <= ELEVATION_MARKER_FT * upf)
                    .min_by(|a, b| (c.at.y - a.at.y).total_cmp(&(c.at.y - b.at.y)))
                else {
                    continue;
                };
                let ft = ((c.at.y - f.at.y) * factor / upf * 12.0).round() / 12.0;
                if (6.5..=20.0).contains(&ft) {
                    let source = format!("{} measured on {} {} (F.F. to B.O. CEILING)", ft_in_label(ft), sh.number, v.title);
                    found.entry(words.clone()).or_default().push((ft, source));
                }
            }
        }
    }
    found
        .into_iter()
        .filter_map(|(words, mut hs)| {
            hs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let (ft, source) = hs.get(hs.len() / 2)?.clone();
            Some((words, ft, source))
        })
        .collect()
}

/// Of each room's ceiling areas, only its largest (the room itself, not its bath or closet)
/// keeps the room's name: the room takes that one's height.
fn main_areas_only(mut areas: Vec<cadaddy_walk3d::CeilingArea>) -> Vec<cadaddy_walk3d::CeilingArea> {
    let size = |a: &cadaddy_walk3d::CeilingArea| {
        let poly: Vec<cadaddy_geom::Vec2> = a.outline.iter().map(|p| cadaddy_geom::Vec2::new(p[0], p[1])).collect();
        cadaddy_geom::shoelace(&poly).abs()
    };
    let mut best: BTreeMap<(String, String), (usize, f64)> = BTreeMap::new();
    for (i, a) in areas.iter().enumerate() {
        let Some(room) = &a.room else { continue };
        let sz = size(a);
        let e = best.entry((a.level.clone(), room.clone())).or_insert((i, sz));
        if sz > e.1 {
            *e = (i, sz);
        }
    }
    let keep: BTreeSet<usize> = best.values().map(|(i, _)| *i).collect();
    for (i, a) in areas.iter_mut().enumerate() {
        if !keep.contains(&i) {
            a.room = None;
        }
    }
    areas
}

/// `9'-6"` for 9.5 (a ceiling height, feet).
fn ft_in_label(ft: f64) -> String {
    if !ft.is_finite() {
        return "?".into();
    }
    let total = (ft.abs() * 12.0).round() as i64;
    format!("{}'-{}\"", total / 12, total % 12)
}

/// A typical room's mechanical equipment as its view draws it (view coordinates).
#[derive(Clone, Debug, PartialEq)]
struct MechItem {
    /// `ptac` or `exhaust_fan`.
    kind: &'static str,
    tag: String,
    lo: [f64; 2],
    hi: [f64; 2],
    /// Where the exhaust fan's duct ends (its wall cap), view coordinates.
    cap: Option<[f64; 2]>,
    /// How it was found (`tag PTAC`, `symbol: no tag`).
    how: String,
}

/// What [`stamp_typical_mech`] made.
#[derive(Default)]
struct StampedMech {
    equipment: Vec<cadaddy_walk3d::MepEquipment>,
    runs: Vec<cadaddy_walk3d::MepRun>,
    /// Per view: what it showed and how many rooms took it.
    report: BTreeMap<String, String>,
}

/// A PTAC's plan size (inches): long side, short side, from…to.
const PTAC_LONG_IN: (f64, f64) = (30.0, 48.0);
const PTAC_SHORT_IN: (f64, f64) = (8.0, 20.0);
/// An exhaust fan's square, inches a side.
const EF_SIDE_IN: (f64, f64) = (8.0, 24.0);
/// How far from its tag (feet) an item's symbol may be.
const MECH_TAG_REACH_FT: f64 = 10.0;
/// A wall cap: at least this many small circles (radius up to 3") within 2' of each other.
const WALL_CAP_CIRCLES: usize = 3;
/// A PTAC's height and an exhaust fan's housing (feet) when no schedule gives them.
const PTAC_HEIGHT_FT: f64 = 16.0 / 12.0;
const EF_HEIGHT_FT: f64 = 10.0 / 12.0;
/// A PTAC's face grille read as a return grille: a return grille this close (feet) to a PTAC.
const PTAC_GRILLE_FT: f64 = 3.0;

/// The PTAC and the exhaust fan (with its wall cap) a typical room's mechanical view draws.
/// `labels`: the view's words (text and block attributes) and where; `shapes`: its polylines'
/// boxes (and whether closed); `circles`: centre and radius. Sizes in inches at `k_in` units.
fn mech_items(labels: &[(String, [f64; 2])], shapes: &[([f64; 2], [f64; 2], bool)], circles: &[([f64; 2], f64)], k_in: f64) -> Vec<MechItem> {
    let mid = |lo: [f64; 2], hi: [f64; 2]| [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
    let dist = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]);
    let sides = |lo: [f64; 2], hi: [f64; 2]| {
        let (w, h) = ((hi[0] - lo[0]) / k_in, (hi[1] - lo[1]) / k_in);
        (w.max(h), w.min(h))
    };
    let reach = MECH_TAG_REACH_FT * 12.0 * k_in;
    let mut out: Vec<MechItem> = Vec::new();
    // Its type number: digits printed by the tag (in the hexagon under the PTAC).
    let type_of = |at: [f64; 2]| {
        labels
            .iter()
            .filter(|(v, p)| (1..=2).contains(&v.len()) && v.chars().all(|c| c.is_ascii_digit()) && dist(*p, at) <= 24.0 * k_in)
            .min_by(|a, b| dist(a.1, at).total_cmp(&dist(b.1, at)))
            .map(|(v, _)| v.clone())
    };
    for (v, at) in labels.iter().filter(|(v, _)| v.trim().eq_ignore_ascii_case("PTAC")) {
        let body = shapes
            .iter()
            .filter(|(lo, hi, _)| {
                let (l, s) = sides(*lo, *hi);
                (PTAC_LONG_IN.0..=PTAC_LONG_IN.1).contains(&l)
                    && (PTAC_SHORT_IN.0..=PTAC_SHORT_IN.1).contains(&s)
                    && dist(mid(*lo, *hi), *at) <= reach
            })
            .min_by(|a, b| dist(mid(a.0, a.1), *at).total_cmp(&dist(mid(b.0, b.1), *at)));
        let Some((lo, hi, _)) = body else { continue };
        let tag = type_of(*at).map_or_else(|| v.trim().to_ascii_uppercase(), |n| format!("PTAC-{n}"));
        if !out.iter().any(|m| m.kind == "ptac" && m.lo == *lo) {
            out.push(MechItem { kind: "ptac", tag, lo: *lo, hi: *hi, cap: None, how: "tag PTAC".into() });
        }
    }
    let squarish = |lo: [f64; 2], hi: [f64; 2]| {
        let (l, s) = sides(lo, hi);
        (EF_SIDE_IN.0..=EF_SIDE_IN.1).contains(&l) && s >= EF_SIDE_IN.0 && l <= 1.4 * s
    };
    let is_ef = |v: &str| {
        let u = v.trim().to_ascii_uppercase();
        u == "EF"
            || u.strip_prefix("EF-")
                .or_else(|| u.strip_prefix("EF"))
                .is_some_and(|r| !r.is_empty() && r.len() <= 3 && r.chars().all(|c| c.is_ascii_alphanumeric()))
    };
    let mut fans: Vec<(MechItem, [f64; 2])> = Vec::new();
    for (v, at) in labels.iter().filter(|(v, _)| is_ef(v)) {
        if let Some((lo, hi, _)) = shapes
            .iter()
            .filter(|(lo, hi, _)| squarish(*lo, *hi) && dist(mid(*lo, *hi), *at) <= reach)
            .min_by(|a, b| dist(mid(a.0, a.1), *at).total_cmp(&dist(mid(b.0, b.1), *at)))
        {
            // One fan tagged twice (a tag and a keynote) is one fan.
            if !fans.iter().any(|(f, _)| dist(mid(f.lo, f.hi), mid(*lo, *hi)) <= 3.0 * k_in) {
                fans.push((
                    MechItem {
                        kind: "exhaust_fan",
                        tag: v.trim().to_ascii_uppercase(),
                        lo: *lo,
                        hi: *hi,
                        cap: None,
                        how: format!("tag {}", v.trim()),
                    },
                    mid(*lo, *hi),
                ));
            }
        }
    }
    if fans.is_empty() {
        // No tag: the fan drawn as a box in a box (its housing and grille), the closed square
        // most others nest in.
        let closed: Vec<([f64; 2], [f64; 2])> = shapes.iter().filter(|(lo, hi, c)| *c && squarish(*lo, *hi)).map(|(lo, hi, _)| (*lo, *hi)).collect();
        let nested = |c: [f64; 2]| closed.iter().filter(|(lo, hi)| dist(mid(*lo, *hi), c) <= 3.0 * k_in).count();
        if let Some((lo, hi)) =
            closed.iter().filter(|(lo, hi)| nested(mid(*lo, *hi)) >= 2).max_by(|a, b| nested(mid(a.0, a.1)).cmp(&nested(mid(b.0, b.1))))
        {
            fans.push((
                MechItem { kind: "exhaust_fan", tag: "EF".into(), lo: *lo, hi: *hi, cap: None, how: "symbol (box in a box), no tag".into() },
                mid(*lo, *hi),
            ));
        }
    }
    // Its wall cap: the nearest cluster of small circles (the louvre drawn at the wall).
    let small: Vec<[f64; 2]> = circles.iter().filter(|(_, r)| *r > 0.0 && *r <= 3.0 * k_in).map(|(c, _)| *c).take(2_000).collect();
    let clusters: Vec<[f64; 2]> = small
        .iter()
        .filter_map(|c| {
            let near: Vec<&[f64; 2]> = small.iter().filter(|o| dist(**o, *c) <= 24.0 * k_in).collect();
            (near.len() >= WALL_CAP_CIRCLES).then(|| {
                let n = near.len() as f64;
                [near.iter().map(|p| p[0]).sum::<f64>() / n, near.iter().map(|p| p[1]).sum::<f64>() / n]
            })
        })
        .collect();
    for (mut f, c) in fans {
        f.cap = clusters.iter().copied().filter(|q| dist(*q, c) <= 2.0 * reach).min_by(|a, b| dist(*a, c).total_cmp(&dist(*b, c)));
        out.push(f);
    }
    out
}

/// The typical rooms' mechanical views (MECH ENLARGED DOUBLE QUEEN): each one's PTAC and
/// exhaust fan (with its duct to the wall cap) laid into every room of its type by its linework,
/// as the rooms' devices are. A room that already has that kind of equipment gets no second.
/// The fan sits in its room's ceiling (`ceiling_ft` of the room, else the level's).
fn stamp_typical_mech(d: &cadaddy_doc::Drawing, sp: &cadaddy_walk3d::BuildingSpec, job: &GuidedJob) -> StampedMech {
    let mut out = StampedMech::default();
    let rooms = job_rooms(job);
    let (types, vocabulary) = room_types(&rooms);
    if types.is_empty() {
        return out;
    }
    let k_in = super::perceive::units_per_inch(d);
    let upf = 12.0 * k_in;
    let bbox = |o: &[cadaddy_geom::Vec2]| {
        o.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
            [b[0].min(q.x), b[1].min(q.y), b[2].max(q.x), b[3].max(q.y)]
        })
    };
    let room_boxes: Vec<[f64; 4]> = rooms.iter().map(|(_, _, o, _)| bbox(o)).collect();
    let floor_lines = FloorLines::new(d, &room_boxes, ROOM_FIT_MARGIN_FT * upf, upf);
    let inside = |r: &[f64; 4], p: [f64; 2]| p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3];
    for sh in job_sheet_refs(job).iter().filter(|r| r.view.is_none()).take(MAX_STAMP_SHEETS) {
        let sub = super::printmep::sheet_drawing(d, sh);
        let texts = cadaddy_mepfix::sheet_facts::texts(&sub);
        let mech_sheet = sh.discipline.eq_ignore_ascii_case("M");
        let views: Vec<([f64; 4], BTreeSet<String>, String)> = cadaddy_mepfix::sheet_facts::view_titles(&sub, &texts)
            .into_iter()
            .filter_map(|v| {
                let up = v.title.to_ascii_uppercase();
                let mech = mech_sheet || up.contains("MECH") || up.contains("HVAC");
                let words = type_words(&v.title, Some(&vocabulary));
                (mech && types.contains(&words)).then_some((v.region?, words, v.title.clone()))
            })
            .collect();
        if views.is_empty() {
            continue;
        }
        // The sheet's words (text and block attributes), shapes and circles.
        let mut labels: Vec<(String, [f64; 2])> = texts.iter().map(|t| (t.value.trim().to_string(), [t.at.x, t.at.y])).collect();
        let mut shapes: Vec<([f64; 2], [f64; 2], bool)> = Vec::new();
        let mut circles: Vec<([f64; 2], f64)> = Vec::new();
        for e in sub.model.iter().take(MAX_LEADER_PIECES) {
            match &e.kind {
                cadaddy_doc::EntityKind::Insert(ins) => {
                    for a in ins.attribs.iter().take(8) {
                        let v = a.text.value.trim();
                        if !v.is_empty() {
                            labels.push((v.to_string(), [a.text.insert.x, a.text.insert.y]));
                        }
                    }
                }
                cadaddy_doc::EntityKind::LwPolyline(pl) if pl.vertices.len() >= 2 => {
                    let b = pl.vertices.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, v| {
                        [b[0].min(v.p.x), b[1].min(v.p.y), b[2].max(v.p.x), b[3].max(v.p.y)]
                    });
                    if b.iter().all(|x| x.is_finite()) {
                        shapes.push(([b[0], b[1]], [b[2], b[3]], pl.closed));
                    }
                }
                cadaddy_doc::EntityKind::Circle(c) if c.center.is_finite() && c.radius.is_finite() => {
                    circles.push(([c.center.x, c.center.y], c.radius))
                }
                _ => {}
            }
        }
        let objects = super::bspec::plan_objects(&sub);
        for (view, words, title) in &views {
            let mine = |p: [f64; 2]| inside(view, p);
            let l: Vec<(String, [f64; 2])> = labels.iter().filter(|(_, p)| mine(*p)).cloned().collect();
            let sh_v: Vec<([f64; 2], [f64; 2], bool)> = shapes.iter().filter(|(lo, hi, _)| mine(*lo) && mine(*hi)).copied().collect();
            let c_v: Vec<([f64; 2], f64)> = circles.iter().filter(|(c, _)| mine(*c)).copied().collect();
            let items = mech_items(&l, &sh_v, &c_v, k_in);
            if items.is_empty() {
                continue;
            }
            let ents: Vec<&std::sync::Arc<cadaddy_doc::Entity>> =
                objects.iter().filter(|e| super::bspec::centre(&sub, e).is_some_and(|c| inside(view, [c.x, c.y]))).copied().collect();
            let lines = super::grids::line_samples(&ents, upf);
            let mut took = 0usize;
            for (ri, (name, level, outline, number)) in rooms.iter().enumerate().filter(|(_, (n, _, _, _))| type_words(n, None) == *words) {
                let Some(lv) = sp.level(level) else { continue };
                let b = bbox(outline);
                let Some((fit, _)) = rigid_room_fit(&lines, *view, b, &floor_lines, ROOM_FIT_MARGIN_FT * upf, 6.0 * k_in) else { continue };
                let room = stamped_room(name, number, ri);
                let ceiling = sp
                    .rooms
                    .iter()
                    .find(|r| r.number.as_deref().is_some_and(|n| n.trim().eq_ignore_ascii_case(number.trim())) && !number.trim().is_empty())
                    .and_then(|r| r.ceiling_ft)
                    .unwrap_or(lv.ceiling_ft);
                let has = |kind: &str| {
                    sp.mep.equipment.iter().chain(out.equipment.iter()).any(|e| {
                        e.kind == kind
                            && e.level == lv.name
                            && cadaddy_geom::point_in_polygon(
                                outline,
                                cadaddy_geom::Vec2::new((e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0),
                            )
                    })
                };
                let had: Vec<bool> = items.iter().map(|it| has(it.kind)).collect();
                let mut any = false;
                for (it, _) in items.iter().zip(&had).filter(|(_, h)| !**h) {
                    let corners = [it.lo, [it.hi[0], it.lo[1]], it.hi, [it.lo[0], it.hi[1]]];
                    let placed: Vec<[f64; 2]> = corners.iter().filter_map(|c| place_in_room(*c, *view, b, fit)).collect();
                    if placed.len() != 4 {
                        continue;
                    }
                    let lo = [placed.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min), placed.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min)];
                    let hi = [
                        placed.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max),
                        placed.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max),
                    ];
                    let fan = it.kind == "exhaust_fan";
                    let mut sources = BTreeMap::new();
                    sources.insert("sheet".to_string(), sh.number.clone());
                    sources.insert("position".to_string(), format!("typical room view {title} laid into room {room}"));
                    sources.insert("outline".to_string(), it.how.clone());
                    sources.insert(
                        "height".to_string(),
                        if fan { format!("in the room's ceiling ({ceiling:.2} ft)") } else { "rule: on the floor, 16\" high".into() },
                    );
                    let mut properties = BTreeMap::new();
                    properties.insert("room".to_string(), room.clone());
                    out.equipment.push(cadaddy_walk3d::MepEquipment {
                        tag: it.tag.clone(),
                        kind: it.kind.to_string(),
                        level: lv.name.clone(),
                        min: lo,
                        max: hi,
                        bottom_ft: if fan { ceiling } else { 0.0 },
                        height_ft: if fan { EF_HEIGHT_FT } else { PTAC_HEIGHT_FT },
                        systems: vec![if fan { "EA".into() } else { "M-DUCT".into() }],
                        title: title.clone(),
                        properties,
                        sources,
                        confidence: 0.7,
                        ..Default::default()
                    });
                    any = true;
                    // The fan's duct, over the ceiling, to its wall cap.
                    if let (true, Some(cap)) = (fan, it.cap.and_then(|c| place_in_room(c, *view, b, fit))) {
                        let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
                        let mut points = vec![c];
                        let (dx, dy) = ((cap[0] - c[0]).abs(), (cap[1] - c[1]).abs());
                        if dx > k_in && dy > k_in {
                            points.push(if dx >= dy { [c[0], cap[1]] } else { [cap[0], c[1]] });
                        }
                        points.push(cap);
                        let bottoms = vec![ceiling + 0.25; points.len().saturating_sub(1)];
                        if let Ok(run) = serde_json::from_value::<cadaddy_walk3d::MepRun>(json!({
                            "system": "EA", "level": lv.name, "points": points, "size_in": [4.0, 0.0], "bottoms_ft": bottoms,
                            "height_source": "typical room view: over the ceiling", "size_source": "rule", "source_sheet": sh.number,
                        })) {
                            out.runs.push(run);
                        }
                    }
                }
                took += usize::from(any);
                if out.equipment.len() >= MAX_STAMPED {
                    return out;
                }
            }
            let what: Vec<String> = items.iter().map(|i| format!("{} ({})", i.tag, i.how)).collect();
            out.report.insert(format!("{} {title}", sh.number), format!("{} into {took} rooms", what.join(", ")));
        }
    }
    out
}

/// Each stamped device whose tag names mechanical equipment (`EF`, `PTAC`) moved to that
/// equipment in its room: an exhaust fan's connection in the ceiling at the fan, a PTAC's at the
/// unit (its printed height kept, else a foot up). How many moved.
fn connect_to_equipment(equipment: &[cadaddy_walk3d::MepEquipment], devices: &mut [super::readers::ReadDevice], reach: f64) -> usize {
    let mut moved = 0usize;
    let centre = |e: &cadaddy_walk3d::MepEquipment| [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0];
    for dv in devices.iter_mut() {
        let tag = dv.tag.trim().to_ascii_uppercase();
        let kind = if tag == "EF" || tag.starts_with("EF-") {
            "exhaust_fan"
        } else if tag == "PTAC" || tag.starts_with("PTAC-") {
            "ptac"
        } else {
            continue;
        };
        // Its room's, else (a tag on the overall plan, in no room yet) the nearest within reach.
        // (A device read off its sheet may still carry the sheet's level code: `3` is Level 3.)
        let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
        let same_level = |l: &str| l == dv.level || (!digits(l).is_empty() && digits(l) == digits(&dv.level));
        let same = |e: &&cadaddy_walk3d::MepEquipment| e.kind == kind && same_level(&e.level);
        let d2 = |e: &cadaddy_walk3d::MepEquipment| {
            let c = centre(e);
            (c[0] - dv.at[0]).hypot(c[1] - dv.at[1])
        };
        let found = if dv.room.is_empty() {
            equipment.iter().filter(same).filter(|e| d2(e) <= reach).min_by(|a, b| d2(a).total_cmp(&d2(b)))
        } else {
            equipment.iter().filter(same).find(|e| e.properties.get("room").is_some_and(|r| *r == dv.room))
        };
        let Some(e) = found else { continue };
        dv.at = centre(e);
        let sheet = e.sources.get("sheet").cloned().unwrap_or_default();
        if kind == "exhaust_fan" {
            dv.aff_in = Some(e.bottom_ft * 12.0);
            dv.aff_note = format!("connection to {} in the room's ceiling ({sheet})", e.tag);
        } else if dv.aff_in.is_none() {
            dv.aff_in = Some(PTAC_CONNECTION_IN);
            dv.aff_note = format!("connection at {} ({sheet})", e.tag);
        }
        moved += 1;
    }
    moved
}

/// Each unit room's equipment (PTAC, exhaust fan) that no device connects: a connection on the
/// circuit the typical panel's schedule names it on (`GUESTROOM PTAC-1` on 13, two poles:
/// `G-13,15`), placed at the equipment by [`connect_to_equipment`].
fn scheduled_connections(
    schedules: &[cadaddy_walk3d::PanelSchedule],
    equipment: &[cadaddy_walk3d::MepEquipment],
    devices: &[super::readers::ReadDevice],
) -> Vec<super::readers::ReadDevice> {
    // The typical panel (one schedule for many rooms) when there is one.
    let typical: Vec<&cadaddy_walk3d::PanelSchedule> = schedules.iter().filter(|p| p.typical_of.is_some()).collect();
    let [panel] = typical.as_slice() else { return Vec::new() };
    let words = |kind: &str| -> &'static [&'static str] {
        match kind {
            "ptac" => &["PTAC"],
            "exhaust_fan" => &["EXHAUST FAN", "BATH FAN"],
            _ => &[],
        }
    };
    let mut out = Vec::new();
    for e in equipment.iter().take(MAX_STAMPED) {
        let Some(room) = e.properties.get("room").filter(|r| !r.is_empty()) else { continue };
        let tag = match e.kind.as_str() {
            "ptac" => "PTAC",
            "exhaust_fan" => "EF",
            _ => continue,
        };
        // Connected already: a connection of its kind in its room.
        if devices.iter().chain(&out).any(|dv| dv.room == *room && dv.tag.to_ascii_uppercase().starts_with(tag)) {
            continue;
        }
        let Some(c) = panel.circuits.iter().find(|c| {
            let up = c.description.to_ascii_uppercase();
            words(&e.kind).iter().any(|w| up.contains(w)) && !c.number.trim().is_empty()
        }) else {
            continue;
        };
        let Ok(n) = c.number.trim().parse::<u32>() else { continue };
        let poles: Vec<String> = (0..c.poles.clamp(1, 3)).map(|i| (n + 2 * i).to_string()).collect();
        out.push(super::readers::ReadDevice {
            tag: tag.into(),
            at: [(e.min[0] + e.max[0]) / 2.0, (e.min[1] + e.max[1]) / 2.0],
            sheet: panel.source.clone(),
            level: e.level.clone(),
            circuit: format!("{}-{}", panel.panel, poles.join(",")),
            room: room.clone(),
            kind_source: format!("panel {} schedule circuit {}: {}", panel.panel, c.number, c.description),
            ..super::readers::ReadDevice::default()
        });
    }
    out
}

/// A stamped unit's footprint: level, room, the footprint, the room's own (label-traced) outline.
type Footprint = (String, String, Vec<cadaddy_geom::Vec2>, Vec<cadaddy_geom::Vec2>);

/// Books each device in no room that stands in a unit's footprint to that unit, when it is a
/// plumbing fixture or fire alarm device (another system's plan of the unit) or a tag the unit's
/// own plans show (`own`: by room, the tags stamped into it)
/// (its level by the digits of its name: a device read off its sheet may carry the sheet's level
/// code); in two
/// footprints (units back to back, their plans' linework past the wall they share), the one whose
/// own room is nearer. How many.
fn book_into_units(
    devices: &mut [super::readers::ReadDevice],
    footprints: &[Footprint],
    own: &BTreeMap<String, BTreeSet<String>>,
    panels: &BTreeMap<String, BTreeSet<String>>,
    reach: f64,
) -> usize {
    let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let mut n = 0;
    let mut booked: BTreeSet<usize> = BTreeSet::new();
    // The tags each unit was booked so far, and those it holds already.
    let mut held: BTreeSet<(String, String)> = devices
        .iter()
        .filter(|dv| !dv.room.trim().is_empty())
        .map(|dv| (dv.room.clone(), format!("{}@{}", dv.tag.trim().to_ascii_uppercase(), units_trade(&dv.sheet))))
        .collect();
    // Devices in one unit's footprint first, then those where two overlap (at the wall they
    // share): by then each unit holds its own, and a fixture there goes to the one lacking it.
    for pass in 0..2 {
        for (i, dv) in devices.iter_mut().enumerate().filter(|(_, dv)| dv.room.trim().is_empty()) {
            // Another system's plan of the unit gives these: its plumbing, its fire alarm devices.
            let plumbing = dv.kind.starts_with("plumbing") || dv.kind.starts_with("fire alarm");
            // A tag the unit's plans show, off a sheet of their trade (an `F` on a mechanical plan is
            // not the electrical plan's wall light).
            let mine = format!("{}@{}", dv.tag.trim().to_ascii_uppercase(), units_trade(&dv.sheet));
            // On another panel's circuit than the unit's devices (a corridor's receptacle on a house
            // panel, across the unit's entry wall): not the unit's.
            let panel = super::propfill::panel_of(&dv.circuit).map(|p| p.to_ascii_uppercase());
            let fed_here = |room: &str| match (&panel, panels.get(room)) {
                (Some(p), Some(ps)) if !ps.is_empty() => ps.contains(p),
                _ => true,
            };
            let p = cadaddy_geom::Vec2::new(dv.at[0], dv.at[1]);
            let here = |l: &str| l == dv.level || (!digits(l).is_empty() && digits(l) == digits(&dv.level));
            // Two units' footprints overlap past the wall they share: the one it stands deepest in
            // (the bath on its side of the wall).
            let fits: Vec<&Footprint> = footprints
                .iter()
                .filter(|(l, room, poly, _)| {
                    here(l)
                        && (plumbing || own.get(room).is_some_and(|t| t.contains(&mine)))
                        && fed_here(room)
                        && cadaddy_geom::point_in_polygon(poly, p)
                })
                .collect();
            if pass == 0 && fits.len() != 1 {
                continue;
            }
            if let Some((_, room, _, _)) = fits.into_iter().max_by(|a, b| {
                // A plumbing fixture: a unit that has none of its tag yet first (two units' water
                // closets back to back are one each).
                let lacks = |room: &str| plumbing && !held.contains(&(room.to_string(), mine.clone()));
                lacks(&a.1).cmp(&lacks(&b.1)).then(edge_distance(&a.2, p).total_cmp(&edge_distance(&b.2, p)))
            }) {
                held.insert((room.clone(), mine.clone()));
                dv.room = room.clone();
                booked.insert(i);
                n += 1;
            }
        }
    }
    // A unit booked two of a fixture while the unit across the wall, its footprint fitted a
    // little short of its own bath, has none within `reach`: the one nearer that unit is its.
    let key = |dv: &super::readers::ReadDevice| (dv.room.clone(), format!("{}@{}", dv.tag.trim().to_ascii_uppercase(), units_trade(&dv.sheet)));
    for _ in 0..devices.len().min(MAX_STAMPED) {
        let mut count: BTreeMap<(String, String), usize> = BTreeMap::new();
        for dv in devices.iter().filter(|dv| !dv.room.trim().is_empty()) {
            *count.entry(key(dv)).or_default() += 1;
        }
        let mut best: Option<(usize, String, f64)> = None;
        for &i in &booked {
            let Some(dv) = devices.get(i).filter(|dv| dv.kind.starts_with("plumbing")) else { continue };
            let (room, mine) = key(dv);
            if count.get(&(room.clone(), mine.clone())).copied().unwrap_or(0) < 2 {
                continue;
            }
            let p = cadaddy_geom::Vec2::new(dv.at[0], dv.at[1]);
            for (l, other, poly, _) in footprints {
                let level = *l == dv.level || (!digits(l).is_empty() && digits(l) == digits(&dv.level));
                if !level || *other == room || count.contains_key(&(other.clone(), mine.clone())) || poly.len() < 3 {
                    continue;
                }
                let d = if cadaddy_geom::point_in_polygon(poly, p) { 0.0 } else { edge_distance(poly, p) };
                if d <= reach && best.as_ref().is_none_or(|b| d < b.2) {
                    best = Some((i, other.clone(), d));
                }
            }
        }
        let Some((i, other, _)) = best else { break };
        if let Some(dv) = devices.get_mut(i) {
            dv.room = other;
        }
    }
    n
}

/// The trade of the sheet a device was read from (its number's discipline letter: `E-6` E), for
/// what a unit's plans show; unknown: empty.
fn units_trade(sheet: &str) -> String {
    cadaddy_mepfix::sheet_facts::discipline_of(sheet.split('/').next().unwrap_or(sheet)).to_ascii_uppercase()
}

/// Leaves one equipment connection (EF, PTAC) per room and tag, the first (stamped before placed),
/// where two plans tag one piece of equipment; how many went.
fn one_connection_each(stamped: &mut Vec<super::readers::ReadDevice>, placed: &mut Vec<super::readers::ReadDevice>, eps: f64) -> usize {
    let connection = |dv: &super::readers::ReadDevice| {
        let t = dv.tag.trim().to_ascii_uppercase();
        t == "EF" || t.starts_with("EF-") || t == "PTAC" || t.starts_with("PTAC-")
    };
    let mut seen: BTreeSet<(String, String, String)> = BTreeSet::new();
    // Where each kept one is: another plan's tag put at the same equipment (in no room) is it.
    let mut at: Vec<(String, String, [f64; 2])> = Vec::new();
    let before = stamped.len() + placed.len();
    for list in [&mut *stamped, &mut *placed] {
        list.retain(|dv| {
            if !connection(dv) {
                return true;
            }
            let tag = dv.tag.trim().to_ascii_uppercase();
            let here = at.iter().any(|(l, t, p)| *l == dv.level && *t == tag && (p[0] - dv.at[0]).hypot(p[1] - dv.at[1]) <= eps);
            let keep = if dv.room.trim().is_empty() { !here } else { seen.insert((dv.level.clone(), dv.room.clone(), tag.clone())) };
            if keep {
                at.push((dv.level.clone(), tag, dv.at));
            }
            keep
        });
    }
    before - stamped.len() - placed.len()
}

/// Leaves one device per room, tag and spot (within `eps`) of a room's stamped devices, unless
/// their circuits differ (two counter receptacles side by side on G-5 and G-6 are two); how many
/// went.
fn one_per_spot(devices: &mut Vec<super::readers::ReadDevice>, eps: f64) -> usize {
    let mut kept: BTreeMap<(String, String, String), Vec<([f64; 2], String)>> = BTreeMap::new();
    let before = devices.len();
    devices.retain(|dv| {
        if dv.room.trim().is_empty() {
            return true;
        }
        let circuit = dv.circuit.trim().to_ascii_uppercase();
        let spots = kept.entry((dv.level.clone(), dv.room.clone(), dv.tag.trim().to_ascii_uppercase())).or_default();
        if spots.iter().any(|(p, c)| (p[0] - dv.at[0]).hypot(p[1] - dv.at[1]) <= eps && (c.is_empty() || circuit.is_empty() || *c == circuit)) {
            return false;
        }
        spots.push((dv.at, circuit));
        true
    });
    before - devices.len()
}

/// A floor plan's device this near where a typical plan's copy of the same tag lands is that
/// device (feet).
const STAND_IN_FT: f64 = 3.0;

/// A subscript letter's text at its base mark's lower right: this far right and below the base
/// mark's text, in its text heights.
const SUBSCRIPT_REACH_H: (f64, f64) = (0.3, 2.0);

/// The fire alarm symbols whose mark is another's subscript (a single letter whose text is at
/// that mark's text's lower right, about its size: the `R` of a resident room's smoke detector
/// `S` with sounder base), and each base's mark with it (`SR`), by symbol.
#[allow(clippy::type_complexity)]
fn fire_alarm_subscripts(
    syms: &[(String, cadaddy_geom::Vec2, cadaddy_doc::Handle, Option<(cadaddy_geom::Vec2, f64)>)],
    free: &[cadaddy_mepfix::sheet_facts::TextItem],
) -> (BTreeSet<cadaddy_doc::Handle>, BTreeMap<cadaddy_doc::Handle, String>) {
    let mut subs: BTreeSet<cadaddy_doc::Handle> = BTreeSet::new();
    let mut compound: BTreeMap<cadaddy_doc::Handle, String> = BTreeMap::new();
    let letters = |t: &str, n: usize| (1..=n).contains(&t.chars().count()) && t.chars().all(|c| c.is_ascii_uppercase());
    for (t, _, h, at) in syms.iter().take(MAX_STAMPED) {
        let Some((p, ht)) = at.filter(|(_, ht)| ht.is_finite() && *ht > 0.0) else { continue };
        if !letters(t, 1) {
            continue;
        }
        let base = syms
            .iter()
            .filter(|(bt, _, bh, _)| bh != h && letters(bt, 2) && !subs.contains(bh) && !compound.contains_key(bh))
            .filter_map(|(bt, _, bh, bat)| {
                let (q, bht) = (*bat)?;
                let (dx, dy) = ((p.x - q.x) / bht, (q.y - p.y) / bht);
                let size = ht / bht;
                ((SUBSCRIPT_REACH_H.0..=SUBSCRIPT_REACH_H.1).contains(&dx)
                    && (SUBSCRIPT_REACH_H.0..=SUBSCRIPT_REACH_H.1).contains(&dy)
                    && (0.6..=1.5).contains(&size))
                .then_some((bt, *bh, dx.hypot(dy)))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2));
        if let Some((bt, bh, _)) = base
            && !subs.contains(h)
            && !compound.contains_key(h)
        {
            subs.insert(*h);
            compound.insert(bh, format!("{bt}{t}"));
        }
    }
    // A subscript the import hung on no symbol is still printed there: a lone letter among the
    // sheet's texts at a base's lower right (each letter once).
    let at_subscript = |q: cadaddy_geom::Vec2, bht: f64, t: &cadaddy_mepfix::sheet_facts::TextItem| {
        let (dx, dy) = ((t.at.x - q.x) / bht, (q.y - t.at.y) / bht);
        (SUBSCRIPT_REACH_H.0..=SUBSCRIPT_REACH_H.1).contains(&dx)
            && (SUBSCRIPT_REACH_H.0..=SUBSCRIPT_REACH_H.1).contains(&dy)
            && (0.6..=1.5).contains(&(t.height / bht))
    };
    let mut used: BTreeSet<usize> = BTreeSet::new();
    for (bt, _, bh, bat) in syms.iter().take(MAX_STAMPED) {
        let Some((q, bht)) = bat.filter(|(_, ht)| ht.is_finite() && *ht > 0.0) else { continue };
        if !letters(bt, 2) || subs.contains(bh) || compound.contains_key(bh) {
            continue;
        }
        let lone = free
            .iter()
            .enumerate()
            .take(MAX_SHEET_TEXTS)
            .filter(|(i, t)| !used.contains(i) && letters(&t.value, 1) && at_subscript(q, bht, t))
            .min_by(|a, b| a.1.at.dist(q).total_cmp(&b.1.at.dist(q)));
        if let Some((i, t)) = lone {
            used.insert(i);
            compound.insert(*bh, format!("{bt}{}", t.value));
        }
    }
    (subs, compound)
}

/// A row of a symbol list (`FIRE ALARM SYMBOL LIST`: SYMBOL, DESCRIPTION, MOUNTING): the mark in
/// its symbol (with its subscript: `SR`), what it is, and where it mounts.
#[derive(Clone, Debug, PartialEq)]
struct SymbolRow {
    mark: String,
    description: String,
    /// On the ceiling (`CEIL./WALL`), else at this centre height (inches) when the row gives one.
    ceiling: bool,
    inches: Option<f64>,
    /// The row's mounting words and its sheet, for the device's notes.
    doc: String,
}

/// A symbol list's rows at most (per table).
const MAX_SYMBOL_ROWS: usize = 60;

/// The symbol lists on the sheets: a table headed SYMBOL, DESCRIPTION and MOUNTING, each row's
/// letters in the symbol column joined left to right (an `S` with its subscript `R` is `SR`). A
/// mark two rows give (`H` a heat detector and a horn) is left out: which one is not known.
fn symbol_list_rows(pages: &[(String, cadaddy_doc::Drawing)]) -> Vec<SymbolRow> {
    let mut out: Vec<SymbolRow> = Vec::new();
    let mut twice: BTreeSet<String> = BTreeSet::new();
    for (sheet, sub) in pages {
        let texts = cadaddy_mepfix::sheet_facts::texts(sub);
        let upper = |t: &cadaddy_mepfix::sheet_facts::TextItem| t.value.trim().to_ascii_uppercase();
        // Heights to the bottom of the item unless noted, when the sheet says so.
        let to_bottom = texts.iter().any(|t| {
            let u = upper(t);
            u.contains("MOUNTING HEIGHTS TO BOTTOM")
        });
        for head in texts.iter().filter(|t| upper(t) == "MOUNTING") {
            let h = head.height.max(1e-6);
            let on_row = |t: &&cadaddy_mepfix::sheet_facts::TextItem, y: f64| (t.at.y - y).abs() <= 0.5 * h;
            let Some(desc_head) = texts
                .iter()
                .filter(|t| on_row(t, head.at.y) && upper(t) == "DESCRIPTION" && t.at.x < head.at.x)
                .max_by(|a, b| a.at.x.total_cmp(&b.at.x))
            else {
                continue;
            };
            let Some(sym_head) = texts
                .iter()
                .filter(|t| on_row(t, head.at.y) && upper(t) == "SYMBOL" && t.at.x < desc_head.at.x)
                .max_by(|a, b| a.at.x.total_cmp(&b.at.x))
            else {
                continue;
            };
            // The descriptions: words between the symbol column and halfway to the mounting
            // column's heading, below the heading, row after row (a gap of more than 4 rows ends
            // the table).
            let mut descs: Vec<&cadaddy_mepfix::sheet_facts::TextItem> = texts
                .iter()
                .filter(|t| {
                    t.at.y < head.at.y - 0.5 * h
                        && t.at.x > sym_head.at.x + h
                        && t.at.x < (desc_head.at.x + head.at.x) / 2.0
                        && t.value.trim().chars().count() > 3
                })
                .collect();
            descs.sort_by(|a, b| b.at.y.total_cmp(&a.at.y));
            let mut last_y = head.at.y;
            for d in descs.into_iter().take(MAX_SYMBOL_ROWS) {
                if last_y - d.at.y > 4.0 * h * 1.5 {
                    break;
                }
                last_y = d.at.y;
                let dh = d.height.max(1e-6);
                let mut letters: Vec<&cadaddy_mepfix::sheet_facts::TextItem> = texts
                    .iter()
                    .filter(|t| {
                        let v = t.value.trim();
                        v.chars().count() <= 2
                            && v.chars().all(|c| c.is_ascii_uppercase())
                            && (t.at.y - d.at.y).abs() <= 0.6 * dh
                            && t.at.x < d.at.x
                            && t.at.x > sym_head.at.x - 3.0 * h
                            && (0.6..=1.5).contains(&(t.height / dh))
                    })
                    .collect();
                if letters.is_empty() {
                    continue;
                }
                letters.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
                let mark: String = letters.iter().map(|t| t.value.trim()).collect();
                let mounting = texts
                    .iter()
                    .filter(|t| on_row(t, d.at.y) && t.at.x >= head.at.x - 3.0 * h)
                    .min_by(|a, b| a.at.x.total_cmp(&b.at.x))
                    .map(|t| t.value.trim().to_string())
                    .unwrap_or_default();
                let mu = mounting.to_ascii_uppercase();
                let ceiling = mu.contains("CEIL");
                let inches = (!ceiling)
                    .then(|| super::propfill::mount_height_in(&format!("{mounting} AFF")))
                    .flatten()
                    .map(|i| {
                        if mu.contains("TO TOP") {
                            i - HALF_BOX_IN
                        } else if mu.contains("TO BOTTOM") || to_bottom {
                            i + HALF_BOX_IN
                        } else {
                            i
                        }
                    })
                    .filter(|i| *i > 0.0 && *i < 240.0);
                if out.iter().any(|r| r.mark == mark) || twice.contains(&mark) {
                    out.retain(|r| r.mark != mark);
                    twice.insert(mark);
                    continue;
                }
                out.push(SymbolRow {
                    mark,
                    description: d.value.trim().to_string(),
                    ceiling,
                    inches,
                    doc: format!("{sheet} symbol list: {}", if mounting.is_empty() { d.value.trim() } else { mounting.as_str() }),
                });
            }
        }
    }
    out
}

/// Each fire alarm device whose mark a symbol list row gives: its description (`device_type`),
/// and its ceiling or its height when it has none. How many.
fn apply_symbol_list(sp: &mut cadaddy_walk3d::BuildingSpec, rows: &[SymbolRow]) -> usize {
    let mut n = 0;
    for t in sp.tags.iter_mut().filter(|t| t.id.starts_with("device:") && t.kind == "fire_alarm_device").take(MAX_STAMPED * 4) {
        let mark = t.fields.get("type").map(|f| f.value.trim().to_ascii_uppercase()).unwrap_or_default();
        let Some(row) = rows.iter().find(|r| r.mark == mark) else { continue };
        t.set("device_type", format!("{} {mark}", row.description.to_ascii_lowercase()), "legend");
        t.set("legend", row.description.clone(), "legend");
        let has = |t: &cadaddy_walk3d::tags::ElementTag, k: &str| t.fields.get(k).is_some_and(|f| !f.value.trim().is_empty());
        if row.ceiling && !has(t, "mount") {
            t.set("mount", "ceiling", "legend");
        } else if let Some(i) = row.inches
            && !has(t, "mount_height_in")
            && !has(t, "mount")
        {
            t.set("mount_height_in", format!("{}", (i * 100.0).round() / 100.0), "legend");
            t.set("mount_height_note", row.doc.clone(), "legend");
        }
        n += 1;
    }
    n
}

/// A kind of device's typical mounting height a general note gives.
#[derive(Clone, Debug, PartialEq)]
struct TypicalHeight {
    /// The device kind it is for (`switch`, `receptacle`, `low_voltage_outlet`, `thermostat`,
    /// `emergency light`: luminaires the fixture schedule calls emergency lights; `panel`).
    kind: &'static str,
    /// The height as the note gives it, inches, and to what: `top`, `bottom` or `centre` (a
    /// panel's centre depends on its box).
    given_in: f64,
    to: &'static str,
    /// Its centre over the floor, inches.
    inches: f64,
    /// The note's words for it, with its sheet.
    doc: String,
}

/// Half a device box's height, inches: a height "to top" or "to bottom" is this from its centre.
const HALF_BOX_IN: f64 = 2.25;

/// The kind a typical-height item names (`TOGGLE SWITCH`, `RECEPTACLES`, `VOICE/DATA OUTLETS`).
fn typical_kind(words: &str) -> Option<&'static str> {
    let up = words.to_ascii_uppercase();
    if up.contains("PANEL") {
        Some("panel")
    } else if up.contains("EMERGENCY") {
        Some("emergency light")
    } else if up.contains("SWITCH") {
        Some("switch")
    } else if up.contains("RECEPT") {
        Some("receptacle")
    } else if ["VOICE", "DATA", "TELEPHONE", "TEL/", "TV", "CATV"].iter().any(|w| up.contains(w)) {
        Some("low_voltage_outlet")
    } else if up.contains("THERMOSTAT") {
        Some("thermostat")
    } else {
        None
    }
}

/// One typical-height item (`TOGGLE SWITCH 4'-0" AFF TO TOP`): its kind and centre height.
fn typical_item(item: &str, sheet: &str) -> Option<TypicalHeight> {
    let kind = typical_kind(item.split(|c: char| c.is_ascii_digit()).next().unwrap_or(item))?;
    let inches = super::propfill::mount_height_in(&format!("{item} AFF"))?;
    let up = item.to_ascii_uppercase();
    let (centre, to) = if up.contains("TO TOP") {
        (inches - HALF_BOX_IN, "top")
    } else if up.contains("TO BOTTOM") {
        (inches + HALF_BOX_IN, "bottom")
    } else {
        (inches, "centre")
    };
    (centre > 0.0 && centre < 240.0).then(|| TypicalHeight {
        kind,
        inches: centre,
        given_in: inches,
        to,
        doc: format!("{sheet} general note: {}", item.trim()),
    })
}

/// The typical mounting heights a set's general note gives (`ELECTRICAL DEVICE MOUNTED HEIGHTS
/// (UNO): • PANELBOARDS 6'-6" AFF TO TOP • TOGGLE SWITCH 4'-0" AFF TO TOP • RECEPTACLES 1'-6"
/// CENTERLINE…`): the note's rows from its heading down to the next numbered note, each read left
/// to right (an item's name and its height are often two columns), item by item.
fn typical_mounting_heights(pages: &[(String, cadaddy_doc::Drawing)]) -> Vec<TypicalHeight> {
    let mut out: Vec<TypicalHeight> = Vec::new();
    for (sheet, sub) in pages {
        let texts = cadaddy_mepfix::sheet_facts::texts(sub);
        for head in texts.iter().filter(|t| {
            let up = t.value.to_ascii_uppercase();
            (up.contains("MOUNTED HEIGHT") || up.contains("MOUNTING HEIGHT"))
                && (up.contains("UNO") || up.contains("UNLESS") || up.contains("TYPICAL"))
        }) {
            let h = head.height.max(1e-6);
            let mut lines: Vec<&cadaddy_mepfix::sheet_facts::TextItem> = texts
                .iter()
                .filter(|t| {
                    t.at.y <= head.at.y + 0.5 * h && head.at.y - t.at.y <= 12.0 * h && t.at.x >= head.at.x - 3.0 * h && t.at.x - head.at.x <= 80.0 * h
                })
                .collect();
            lines.sort_by(|a, b| b.at.y.total_cmp(&a.at.y).then(a.at.x.total_cmp(&b.at.x)));
            // Its rows, each read left to right (an item's name and its height are two columns).
            let mut rows: Vec<(f64, Vec<&cadaddy_mepfix::sheet_facts::TextItem>)> = Vec::new();
            for l in lines {
                match rows.last_mut() {
                    Some((y, row)) if (*y - l.at.y).abs() <= 0.5 * h => row.push(l),
                    _ => rows.push((l.at.y, vec![l])),
                }
            }
            let mut items: Vec<String> = Vec::new();
            for (i, (_, row)) in rows.iter_mut().enumerate() {
                row.sort_by(|a, b| a.at.x.total_cmp(&b.at.x));
                let text = row.iter().map(|t| t.value.trim()).collect::<Vec<_>>().join(" ");
                // The next numbered note ends this one.
                let numbered = text.split_once('.').is_some_and(|(n, _)| !n.is_empty() && n.len() <= 2 && n.chars().all(|c| c.is_ascii_digit()));
                if i > 0 && numbered {
                    break;
                }
                let text = if i == 0 { text.split_once(':').map_or(String::new(), |(_, rest)| rest.to_string()) } else { text };
                items.extend(text.split(['\u{2022}', ';']).map(str::trim).filter(|i| !i.is_empty()).map(str::to_string));
            }
            for item in &items {
                if let Some(t) = typical_item(item, sheet)
                    && !out.iter().any(|o| o.kind == t.kind)
                {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// Each device of a kind the typical heights give with no height of its own (no AFF note by it)
/// takes the note's: its `mount_height_in`, with the note. How many.
fn apply_typical_heights(sp: &mut cadaddy_walk3d::BuildingSpec, typical: &[TypicalHeight]) -> usize {
    let emergency: BTreeSet<String> = sp
        .mep
        .fixture_schedule
        .iter()
        .filter(|f| f.description.to_ascii_uppercase().contains("EMERGENCY LIGHT"))
        .map(|f| f.tag.to_ascii_uppercase())
        .collect();
    let mut n = 0;
    for t in sp.tags.iter_mut().filter(|t| t.id.starts_with("device:")).take(MAX_STAMPED * 4) {
        if t.fields.get("mount_height_in").is_some_and(|f| !f.value.trim().is_empty()) {
            continue;
        }
        let mark = t.fields.get("type").map(|f| f.value.trim().to_ascii_uppercase()).unwrap_or_default();
        let kind = if emergency.contains(&mark) { "emergency light" } else { t.kind.as_str() };
        let Some(h) = typical.iter().find(|h| h.kind == kind) else { continue };
        t.set("mount_height_in", format!("{}", (h.inches * 100.0).round() / 100.0), "note");
        t.set("mount_height_note", h.doc.clone(), "note");
        n += 1;
    }
    n
}

/// Two tags of one plumbing fixture (on the enlarged plan and the floor's), at most this far apart (feet).
const DUPLICATE_FIXTURE_FT: f64 = 2.0;
/// Two sheets' tags of one plumbing fixture (the sanitary and the water plans'), at most this far
/// apart (feet): less than two water closets back to back across a wall.
const CROSS_SHEET_FIXTURE_FT: f64 = 2.75;
/// A unit's second fixture of a tag this near (feet) a unit with none is that unit's: its own,
/// across the wall they share, where its footprint was fitted a little short.
const NEIGHBOUR_FIXTURE_FT: f64 = 3.0;
/// How far (feet) a connection tag in no room may be from the equipment it is for.
const CONNECT_REACH_FT: f64 = 15.0;
/// A PTAC's connection height when the plans print none (inches).
const PTAC_CONNECTION_IN: f64 = 12.0;

/// The plumbing fixtures of the guest rooms the plumbing plans show (the enlarged sanitary
/// views draw one pair of rooms a floor, P-1 water closet, P-2 lavatory...): each room of the
/// same type with none gets the fixture set of the room of its type that has the most, laid in
/// the way a typical view of that type goes into each of the two (turned and mirrored as each
/// room is). `placed`: the devices put onto the plans. The copies, and per type how many rooms.
fn copy_room_fixtures(
    d: &cadaddy_doc::Drawing,
    job: &GuidedJob,
    placed: &[super::readers::ReadDevice],
) -> (Vec<super::readers::ReadDevice>, BTreeMap<String, usize>) {
    let mut out = Vec::new();
    let mut report: BTreeMap<String, usize> = BTreeMap::new();
    let rooms = job_rooms(job);
    let (types, vocabulary) = room_types(&rooms);
    let fixtures: Vec<&super::readers::ReadDevice> =
        placed.iter().filter(|dv| dv.kind == "plumbing fixture" || dv.kind == "plumbing stack").collect();
    if types.is_empty() || fixtures.is_empty() {
        return (out, report);
    }
    let k_in = super::perceive::units_per_inch(d);
    let upf = 12.0 * k_in;
    let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let bbox = |o: &[cadaddy_geom::Vec2]| {
        o.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
            [b[0].min(q.x), b[1].min(q.y), b[2].max(q.x), b[3].max(q.y)]
        })
    };
    // Each typed room's fixtures: in its outline, or booked to it (its bath, outside the outline
    // its label traced, by the unit's footprint).
    let in_room: Vec<Vec<&super::readers::ReadDevice>> = rooms
        .iter()
        .enumerate()
        .map(|(i, (name, level, outline, number))| {
            let booked = stamped_room(name, number, i);
            fixtures
                .iter()
                .filter(|f| {
                    digits(&f.level) == digits(level)
                        && (f.room == booked || cadaddy_geom::point_in_polygon(outline, cadaddy_geom::Vec2::new(f.at[0], f.at[1])))
                })
                .copied()
                .collect()
        })
        .collect();
    // The typical views of each type, with their linework.
    let mut views: BTreeMap<BTreeSet<String>, Vec<([f64; 4], Vec<cadaddy_geom::Vec2>)>> = BTreeMap::new();
    let inside = |r: &[f64; 4], c: cadaddy_geom::Vec2| c.x >= r[0] && c.x <= r[2] && c.y >= r[1] && c.y <= r[3];
    for sh in job_sheet_refs(job).iter().filter(|r| r.view.is_none()).take(MAX_STAMP_SHEETS) {
        let sub = super::printmep::sheet_drawing(d, sh);
        let texts = cadaddy_mepfix::sheet_facts::texts(&sub);
        let titled: Vec<([f64; 4], BTreeSet<String>)> = cadaddy_mepfix::sheet_facts::view_titles(&sub, &texts)
            .into_iter()
            .filter_map(|v| {
                let words = type_words(&v.title, Some(&vocabulary));
                (types.contains(&words) && !v.title.to_ascii_uppercase().contains(" WALL")).then_some((v.region?, words))
            })
            .collect();
        if titled.is_empty() {
            continue;
        }
        let objects = super::bspec::plan_objects(&sub);
        for (region, words) in titled {
            let list = views.entry(words).or_default();
            if list.len() >= 4 {
                continue;
            }
            let ents: Vec<&std::sync::Arc<cadaddy_doc::Entity>> =
                objects.iter().filter(|e| super::bspec::centre(&sub, e).is_some_and(|c| inside(&region, c))).copied().collect();
            list.push((region, super::grids::line_samples(&ents, upf)));
        }
    }
    let room_boxes: Vec<[f64; 4]> = rooms.iter().map(|(_, _, o, _)| bbox(o)).collect();
    let floor_lines = FloorLines::new(d, &room_boxes, ROOM_FIT_MARGIN_FT * upf, upf);
    let fit = |vi: &([f64; 4], Vec<cadaddy_geom::Vec2>), ri: usize| {
        let b = room_boxes.get(ri)?;
        rigid_room_fit(&vi.1, vi.0, *b, &floor_lines, ROOM_FIT_MARGIN_FT * upf, 6.0 * k_in).map(|(f, _)| f).filter(|f| f.rigid)
    };
    for words in &types {
        let Some(vs) = views.get(words) else { continue };
        let of_type: Vec<usize> = (0..rooms.len()).filter(|i| rooms.get(*i).is_some_and(|r| type_words(&r.0, None) == *words)).collect();
        // The type's fixture set: one of each tag the plans letter in the rooms of this type, the
        // rooms with the most first (a tag lettered across the wall, in the room behind, would
        // put a second where there is none), taken back into the view that goes into its room.
        let mut set: Vec<([f64; 2], &super::readers::ReadDevice)> = Vec::new();
        let mut v_used: Option<&([f64; 4], Vec<cadaddy_geom::Vec2>)> = None;
        let mut sources = 0usize;
        let mut by_count: Vec<usize> = of_type.iter().copied().filter(|i| in_room.get(*i).is_some_and(|f| !f.is_empty())).collect();
        by_count.sort_by_key(|i| std::cmp::Reverse(in_room.get(*i).map_or(0, Vec::len)));
        for src in by_count {
            let Some((v, fit_src)) = vs.iter().filter(|v| v_used.is_none_or(|u| std::ptr::eq(u, *v))).find_map(|v| fit(v, src).map(|f| (v, f)))
            else {
                continue;
            };
            v_used = Some(v);
            sources += 1;
            for f in in_room.get(src).into_iter().flatten() {
                let Some(p) = view_point(f.at, fit_src) else { continue };
                if !set.iter().any(|(_, g)| g.tag == f.tag) {
                    set.push((p, f));
                }
            }
        }
        let Some(v) = v_used else { continue };
        // Each room of the type: the set's fixtures it hasn't (by tag, about there).
        let mut took = 0usize;
        for ri in of_type.iter().copied() {
            let Some(fit_t) = fit(v, ri) else { continue };
            let Some((name, level, _, number)) = rooms.get(ri) else { continue };
            let b = room_boxes.get(ri).copied().unwrap_or_default();
            let have = in_room.get(ri).cloned().unwrap_or_default();
            let mut added = false;
            for (p, f) in &set {
                let Some(at) = place_in_room(*p, v.0, b, fit_t) else { continue };
                // The set is one of each tag: a room with that tag already (where the plans put it,
                // however its fit lays the set in) takes none.
                if have.iter().any(|h| h.tag == f.tag) {
                    continue;
                }
                out.push(super::readers::ReadDevice { at, level: level.clone(), room: stamped_room(name, number, ri), ..(*f).clone() });
                added = true;
            }
            took += usize::from(added);
            if out.len() >= MAX_STAMPED {
                break;
            }
        }
        let label = words.iter().cloned().collect::<Vec<_>>().join(" ");
        let tags: Vec<&str> = set.iter().map(|(_, f)| f.tag.as_str()).collect();
        report.insert(format!("{label}: set {tags:?} from {sources} rooms the plans tag"), took);
    }
    (out, report)
}

/// The typical rooms as unit types (`unittypes`): each type of room the enlarged views draw,
/// with every view of it (sheet, title, where: plans, RCP, power, mechanical, plumbing,
/// elevations), and each room of the type as an instance with its hand and how its type's plan
/// lies in it (turned, mirrored), as the stamping laid the views in. `(unit_types,
/// unit_instances)`.
fn job_unit_types(d: &cadaddy_doc::Drawing, job: &GuidedJob) -> (Vec<Value>, Vec<Value>) {
    let rooms = job_rooms(job);
    let (types, vocabulary) = room_types(&rooms);
    if types.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let k_in = super::perceive::units_per_inch(d);
    let upf = 12.0 * k_in;
    let inside = |r: &[f64; 4], c: cadaddy_geom::Vec2| c.x >= r[0] && c.x <= r[2] && c.y >= r[1] && c.y <= r[3];
    // Each type's views; its plan's linework (the first plan view: not an RCP, elevation or detail).
    let mut views: BTreeMap<BTreeSet<String>, Vec<Value>> = BTreeMap::new();
    let mut plan: BTreeMap<BTreeSet<String>, ([f64; 4], Vec<cadaddy_geom::Vec2>, String)> = BTreeMap::new();
    // The guest-room views of no one type (seen before a type's own views are: added after).
    let mut generic: Vec<Value> = Vec::new();
    // Each sheet once, whole: its entry, else (a sheet only of several levels' views, P-7) one
    // spanning all its views.
    let refs = job_sheet_refs(job);
    let base = |n: &str| n.split('/').next().unwrap_or(n).trim().to_ascii_uppercase();
    let mut sheets: Vec<super::printmep::SheetRef> = Vec::new();
    for r in &refs {
        let b = base(&r.number);
        match sheets.iter_mut().find(|s| base(&s.number) == b) {
            // The sheet's own entry wins; views of it widen a stand-in to the whole sheet.
            Some(s) if r.view.is_none() && r.number.trim().eq_ignore_ascii_case(&b) => *s = r.clone(),
            Some(s) if s.view.is_none() && s.number.trim().eq_ignore_ascii_case(&b) => {}
            Some(s) => {
                s.rect = [s.rect[0].min(r.rect[0]), s.rect[1].min(r.rect[1]), s.rect[2].max(r.rect[2]), s.rect[3].max(r.rect[3])];
                s.span = s.span.or(r.span);
            }
            None => sheets.push(super::printmep::SheetRef {
                view: None,
                number: if r.view.is_none() { r.number.clone() } else { b.clone() },
                ..r.clone()
            }),
        }
    }
    for sh in sheets.iter().take(MAX_STAMP_SHEETS) {
        let sub = super::printmep::sheet_drawing(d, sh);
        let texts = cadaddy_mepfix::sheet_facts::texts(&sub);
        let mut objects = None;
        let titled = cadaddy_mepfix::sheet_facts::view_titles(&sub, &texts);
        let titles: Vec<[f64; 2]> = titled.iter().map(|t| t.at).collect();
        for v in titled.iter().cloned() {
            let words = type_words(&v.title, Some(&vocabulary));
            // A view of "the guest room" naming no type and a floor (ENLARGED GUESTROOM - 3RD
            // FLOOR - SANITARY): every guest room type's, on that floor.
            let up = v.title.to_ascii_uppercase();
            let enlarged = up.contains("ENLARGED") || up.contains("PLAN");
            if !types.contains(&words) && enlarged && (up.contains("GUESTROOM") || up.contains("GUEST ROOM")) {
                let Some(region) = v.region.or_else(|| title_cell(v.at, &titles, sh.rect)) else {
                    progress_line(&format!("  unit types: {} {:?} at {:?}: no part of the sheet found for it", sh.number, v.title, v.at));
                    continue;
                };
                let levels = cadaddy_mepfix::sheet_reader::detect_levels(&v.title);
                for (_, list) in views.iter_mut().filter(|(w, _)| sleeping_room(w)) {
                    if list.len() < MAX_UNIT_VIEWS {
                        list.push(json!({"sheet": sh.number, "title": v.title, "rect": region, "sheet_rect": sh.rect, "scale": v.scale_text, "levels": levels}));
                    }
                }
                generic.push(
                    json!({"sheet": sh.number, "title": v.title, "rect": region, "sheet_rect": sh.rect, "scale": v.scale_text, "levels": levels}),
                );
                continue;
            }
            let Some(region) = v.region.filter(|_| types.contains(&words)) else { continue };
            let list = views.entry(words.clone()).or_default();
            if list.len() < MAX_UNIT_VIEWS {
                list.push(json!({"sheet": sh.number, "title": v.title, "rect": region, "sheet_rect": sh.rect, "scale": v.scale_text}));
            }
            let up = v.title.to_ascii_uppercase();
            let side = [" WALL", "ELEVATION", "SECTION", "DETAIL", "RCP", "REFLECTED"].iter().any(|w| up.contains(w));
            if !side && !plan.contains_key(&words) {
                let objs = objects.get_or_insert_with(|| super::bspec::plan_objects(&sub));
                let ents: Vec<&std::sync::Arc<cadaddy_doc::Entity>> =
                    objs.iter().filter(|e| super::bspec::centre(&sub, e).is_some_and(|c| inside(&region, c))).copied().collect();
                plan.insert(words, (region, super::grids::line_samples(&ents, upf), format!("{} {}", sh.number, v.title)));
            }
        }
    }
    if !generic.is_empty() {
        let names: Vec<String> =
            generic.iter().map(|g| format!("{} {}", g["sheet"].as_str().unwrap_or(""), g["title"].as_str().unwrap_or(""))).collect();
        progress_line(&format!("  unit types: guest-room views of no one type, each floor's: {names:?}"));
    }
    for (_, list) in views.iter_mut().filter(|(w, _)| sleeping_room(w)) {
        for g in &generic {
            if list.len() < MAX_UNIT_VIEWS && !list.contains(g) {
                list.push(g.clone());
            }
        }
    }
    let bbox = |o: &[cadaddy_geom::Vec2]| {
        o.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
            [b[0].min(q.x), b[1].min(q.y), b[2].max(q.x), b[3].max(q.y)]
        })
    };
    let room_boxes: Vec<[f64; 4]> = rooms.iter().map(|(_, _, o, _)| bbox(o)).collect();
    let floor_lines = FloorLines::new(d, &room_boxes, ROOM_FIT_MARGIN_FT * upf, upf);
    let id_of = |w: &BTreeSet<String>| w.iter().cloned().collect::<Vec<_>>().join("-");
    // A room the rooms stage left unnumbered: the number printed in it on its floor's plan.
    let numbers = super::identity::number_texts(d);
    let printed = |outline: &[cadaddy_geom::Vec2]| -> Option<String> {
        let b = bbox(outline);
        let mid = [(b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0];
        numbers
            .iter()
            .filter(|(_, at)| cadaddy_geom::point_in_polygon(outline, cadaddy_geom::Vec2::new(at[0], at[1])))
            .min_by(|a, b| (a.1[0] - mid[0]).hypot(a.1[1] - mid[1]).total_cmp(&(b.1[0] - mid[0]).hypot(b.1[1] - mid[1])))
            .map(|(n, _)| n.clone())
    };
    let mut unit_types = Vec::new();
    let mut instances = Vec::new();
    // Rooms to live in (guest rooms, dwelling units): not a corridor or a back-of-house room that
    // an enlarged view happens to show.
    for words in types.iter().filter(|w| sleeping_room(w)) {
        let of_type: Vec<usize> = (0..rooms.len()).filter(|i| rooms.get(*i).is_some_and(|r| type_words(&r.0, None) == *words)).collect();
        let Some(vs) = views.get(words) else { continue };
        // Its name: what its rooms are called most.
        let mut names: BTreeMap<&str, usize> = BTreeMap::new();
        for i in &of_type {
            if let Some(r) = rooms.get(*i) {
                *names.entry(r.0.trim()).or_default() += 1;
            }
        }
        let name = names.iter().max_by_key(|(_, n)| **n).map_or_else(|| id_of(words), |(n, _)| (*n).to_string());
        let p = plan.get(words);
        unit_types.push(json!({
            "id": id_of(words), "name": name, "views": vs,
            "source": {"sheet": p.map_or_else(|| vs.first().and_then(|v| v.get("sheet")).cloned().unwrap_or(Value::Null), |p| json!(p.2.split(' ').next().unwrap_or(""))), "by": "guided job: typical-room views"},
        }));
        for i in of_type.iter().copied().take(MAX_STAMPED) {
            let Some((rname, level, outline, number)) = rooms.get(i) else { continue };
            let fit = p.and_then(|(region, lines, _)| {
                let b = room_boxes.get(i)?;
                rigid_room_fit(lines, *region, *b, &floor_lines, ROOM_FIT_MARGIN_FT * upf, 6.0 * k_in).map(|(f, _)| f).filter(|f| f.rigid)
            });
            let mut inst = json!({
                "number": if number.trim().is_empty() { printed(outline).unwrap_or_else(|| stamped_room(rname, number, i)) } else { number.trim().to_string() },
                "level": level, "type": id_of(words), "unit_type": id_of(words),
                "hand": if fit.is_some_and(|f| f.k >= 4) { "mirrored" } else { "normal" },
                "source": "guided job",
            });
            if let (Some(f), Some(p)) = (fit, p) {
                inst["fit"] =
                    json!({"rotation_deg": f64::from(f.k % 4) * 90.0, "mirrored": f.k >= 4, "view": p.2, "by": "linework (as the stamping)"});
            }
            instances.push(inst);
        }
    }
    (unit_types, instances)
}

/// A view's part of its sheet when its linework can't be told from its neighbours' (views in a
/// row sharing grid lines): from its title right to the next title of its row and up to the next
/// title over it (else to the sheet's edge, less the title block's strip).
fn title_cell(at: [f64; 2], titles: &[[f64; 2]], sheet: [f64; 4]) -> Option<[f64; 4]> {
    let (w, h) = (sheet[2] - sheet[0], sheet[3] - sheet[1]);
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let row = 0.05 * h;
    let left = (at[0] - 0.02 * w).max(sheet[0]);
    let right = titles
        .iter()
        .filter(|t| (t[1] - at[1]).abs() <= row && t[0] > at[0] + 0.02 * w)
        .map(|t| t[0] - 0.02 * w)
        .fold(sheet[2] - TITLE_BLOCK_SHARE * w, f64::min);
    let bottom = (at[1] - 0.02 * h).max(sheet[1]);
    let top = titles.iter().filter(|t| t[1] > at[1] + row && t[0] >= left && t[0] <= right).map(|t| t[1] - 0.02 * h).fold(sheet[3], f64::min);
    (right > left && top > bottom).then_some([left, bottom, right, top])
}

/// The strip of a sheet its title block takes (a share of its width, at the right).
const TITLE_BLOCK_SHARE: f64 = 0.12;

/// The views of one unit type kept at most.
const MAX_UNIT_VIEWS: usize = 40;

/// A legend row's device kind and IFC class from its description (`DUPLEX RECEPTACLE - NEMA
/// 5-20R`: a receptacle, `IfcOutlet`). None for a row that names no device (a mounting height
/// note, a circuit designation, a fixture that the luminaire schedule types).
pub(crate) fn legend_kind(description: &str) -> Option<(&'static str, &'static str)> {
    let u = description.to_ascii_uppercase();
    let has = |w: &str| u.contains(w);
    if ["INSTALL DEVICE AT", "DESIGNATION", "MOUNT 6", "LOWER CASE", "INDICATES", "LIGHTING FIXTURE", "CIRCUITING"].iter().any(|w| has(w)) {
        return None;
    }
    Some(if has("GFCI") || has("GROUND FAULT") || has("GFI ") {
        ("gfci_receptacle", "IfcOutlet")
    } else if has("CEILING MOUNTED") && has("RECEPTACLE") {
        ("ceiling_receptacle", "IfcOutlet")
    } else if has("QUAD") && has("RECEPTACLE") {
        ("quad_receptacle", "IfcOutlet")
    } else if has("RECEPTACLE") {
        ("receptacle", "IfcOutlet")
    } else if has("FLOOR BOX") {
        ("floor_box", "IfcOutlet")
    } else if has("3-WAY") || has("THREE WAY") {
        ("3way_switch", "IfcSwitchingDevice")
    } else if has("OCCUPANCY") || has("VACANCY") {
        ("occupancy_sensor", "IfcSensor")
    } else if has("SWITCH") && !has("DISCONNECT") {
        ("switch", "IfcSwitchingDevice")
    } else if has("DISCONNECT") {
        ("disconnect", "IfcSwitchingDevice")
    } else if has("SMOKE") {
        ("smoke_detector", "IfcSensor")
    } else if has("HEAT SENSOR") || has("HEAT DETECTOR") {
        ("heat_detector", "IfcSensor")
    } else if has("PULL STATION") {
        ("pull_station", "IfcAlarm")
    } else if has("HORN") || has("STROBE") {
        ("fire_alarm_notification", "IfcAlarm")
    } else if has("TELEVISION") || has("TV OUTLET") || has("CATV") {
        ("tv_outlet", "IfcOutlet")
    } else if has("DATA") || has("TELEPHONE") {
        ("data_outlet", "IfcOutlet")
    } else if has("AUDIO") {
        ("av_outlet", "IfcOutlet")
    } else if has("WIRELESS ACCESS") {
        ("wireless_access_point", "IfcCommunicationsAppliance")
    } else if has("CAMERA") {
        ("camera", "IfcCommunicationsAppliance")
    } else if has("SPEAKER") {
        ("speaker", "IfcAudioVisualAppliance")
    } else if has("THERMOSTAT") {
        ("thermostat", "IfcUnitaryControlElement")
    } else if has("EXHAUST FAN") {
        ("exhaust_fan", "IfcFan")
    } else if has("JUNCTION BOX") {
        ("junction_box", "IfcJunctionBox")
    } else if has("PANELBOARD") || has("PANEL BOARD") {
        ("panel", "IfcElectricDistributionBoard")
    } else if has("EXIT SIGN") {
        ("exit_sign", "IfcLightFixture")
    } else if has("DAMPER") {
        ("motorized_damper", "IfcDamper")
    } else if has("DOOR HOLD") {
        ("door_holder", "IfcElectricAppliance")
    } else if ["CARD READER", "INTERCOM", "BELL", "PUSH BUTTON", "VOLUME CONTROL", "DOOR CONTACT", "DOOR POWER", "POWER PACK"].iter().any(|w| has(w))
    {
        ("low_voltage_device", "IfcCommunicationsAppliance")
    } else {
        return None;
    })
}

/// A symbol block's shape, whatever its size and name (a PDF import names the repeated drawings
/// of each page apart): its lines, circles, arcs and polyline corners, put in its box scaled to
/// one, rounded to a 16th, sorted. None for an empty block or one of more than 64 parts.
pub(crate) fn symbol_shape(d: &cadaddy_doc::Drawing, block: &str) -> Option<Vec<(u8, i32, i32, i32)>> {
    let b = d.block(block)?;
    if b.entities.len() > 64 {
        return None;
    }
    shape_of(b.entities.iter().map(|e| &e.kind))
}

/// [`symbol_shape`] of some drawn objects (a symbol drawn once, as linework, in a legend row).
fn shape_of<'a>(objects: impl Iterator<Item = &'a cadaddy_doc::EntityKind>) -> Option<Vec<(u8, i32, i32, i32)>> {
    let mut parts: Vec<(u8, [f64; 2], f64)> = Vec::new();
    for kind in objects.take(64) {
        match kind {
            cadaddy_doc::EntityKind::Line(l) => {
                parts.push((0, [l.a.x, l.a.y], 0.0));
                parts.push((0, [l.b.x, l.b.y], 0.0));
            }
            cadaddy_doc::EntityKind::Circle(c) => parts.push((1, [c.center.x, c.center.y], c.radius)),
            cadaddy_doc::EntityKind::Arc(a) => parts.push((2, [a.center.x, a.center.y], a.radius)),
            cadaddy_doc::EntityKind::LwPolyline(pl) => parts.extend(pl.vertices.iter().take(64).map(|v| (3, [v.p.x, v.p.y], 0.0))),
            _ => {}
        }
    }
    let (lo, hi) = parts.iter().fold(([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]), |(lo, hi), (_, p, r)| {
        ([lo[0].min(p[0] - r), lo[1].min(p[1] - r)], [hi[0].max(p[0] + r), hi[1].max(p[1] + r)])
    });
    let size = (hi[0] - lo[0]).max(hi[1] - lo[1]);
    if parts.is_empty() || !(size.is_finite() && size > 0.0) {
        return None;
    }
    let q = |v: f64| (v / size * 16.0).round() as i32;
    let mut sig: Vec<(u8, i32, i32, i32)> = parts.iter().map(|(k, p, r)| (*k, q(p[0] - lo[0]), q(p[1] - lo[1]), q(*r))).collect();
    sig.sort_unstable();
    sig.dedup();
    Some(sig)
}

/// A symbol's shape ([`symbol_shape`]).
type Shape = Vec<(u8, i32, i32, i32)>;

/// Whether a shape is a keynote's box: straight pieces only, drawn into all four corners of its
/// extent (square or eased), with its number's strokes inside. A receptacle's symbol is round (a
/// circle never comes near its extent's corners); an empty box (furniture, equipment drawn by a
/// receptacle whose label it was taken for) has nothing inside.
fn plain_box(shape: &Shape) -> bool {
    let straight = !shape.is_empty() && shape.iter().all(|(k, _, _, r)| (*k == 0 || *k == 3) && *r == 0);
    let (x1, y1) = shape.iter().fold((0, 0), |(x, y), (_, px, py, _)| (x.max(*px), y.max(*py)));
    let corner = |x: i32, y: i32| shape.iter().any(|(_, px, py, _)| (px - x).abs() <= 1 && (py - y).abs() <= 1);
    let inside = shape.iter().any(|(_, px, py, _)| *px > 1 && *px < x1 - 1 && *py > 1 && *py < y1 - 1);
    straight && x1 > 2 && y1 > 2 && corner(0, 0) && corner(x1, 0) && corner(0, y1) && corner(x1, y1) && inside
}

/// The electrical legends' symbols: each row's symbol (by its shape) with its description and
/// sheet. Read off the SYMBOL | DESCRIPTION tables of the E sheets: those in the drawing and the
/// legend sheets the set wasn't imported for (re-read from their PDF pages).
fn legend_symbols(d: &cadaddy_doc::Drawing, job: &GuidedJob) -> BTreeMap<Shape, (String, String)> {
    let mut out: BTreeMap<Shape, (String, String)> = BTreeMap::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut pages: Vec<(String, cadaddy_doc::Drawing)> = job_sheet_refs(job)
        .iter()
        .filter(|r| r.discipline.eq_ignore_ascii_case("E") && seen.insert(r.number.split('/').next().unwrap_or("").to_string()))
        .take(MAX_LEGEND_SHEETS)
        .map(|sh| {
            (
                sh.number.split('/').next().unwrap_or("").to_string(),
                super::printmep::sheet_drawing(d, &super::printmep::SheetRef { view: None, ..sh.clone() }),
            )
        })
        .collect();
    pages.extend(non_plan_pages(job, "E").into_iter().filter(|(n, _)| !seen.contains(n)));
    for (number, sub) in &pages {
        legend_rows(sub, number, &mut out);
    }
    out
}

/// The gear words a submittal's file name carries when it is a gear submittal.
const GEAR_SUBMITTAL_WORDS: [&str; 4] = ["GEAR", "SWITCHGEAR", "PANELBOARD", "SWITCHBOARD"];

/// The electrical gear from the documents ([`super::gearbuild`]): feeder types (the feeder
/// schedule), each panel's incoming type (the one-line), the gear's box and lugs (the gear
/// submittals); the gear laid out in its rooms (proposed), built, and checked by `gear.review`.
fn gear_step(s: &mut Session, job: &mut GuidedJob, pages: &[(String, cadaddy_doc::Drawing)], result: &mut Value) -> crate::Result<()> {
    let Ok((mut sp, true)) = super::bspec::current(s, "bim_build") else { return Ok(()) };
    if sp.mep.panel_schedules.is_empty() {
        return Ok(());
    }
    let tags: Vec<String> = sp.mep.panel_schedules.iter().map(|p| p.panel.clone()).collect();
    let mut types: Vec<super::gearbuild::FeederType> = Vec::new();
    for (number, d) in pages {
        for f in super::gearbuild::feeder_types(&cadaddy_mepfix::sheet_facts::texts(d), number) {
            if !types.iter().any(|t| t.tag.eq_ignore_ascii_case(&f.tag)) {
                types.push(f);
            }
        }
    }
    let type_tags: Vec<String> = types.iter().map(|t| t.tag.clone()).collect();
    let mut incoming: BTreeMap<String, String> = BTreeMap::new();
    for (_, d) in pages {
        for (k, v) in super::gearbuild::oneline_incoming(&cadaddy_mepfix::sheet_facts::texts(d), &tags, &type_tags) {
            incoming.entry(k).or_insert(v);
        }
    }
    let mut gear: BTreeMap<String, super::gearbuild::SubmittedGear> = BTreeMap::new();
    // The gear submittals read (file, name, text): their drawing pages are read again by position.
    let mut read_files: Vec<(String, String, String)> = Vec::new();
    for f in super::submittals::pdf_files(&job.submittals).into_iter().take(MAX_GEAR_SUBMITTALS) {
        let name = std::path::Path::new(&f).file_stem().map_or_else(|| f.clone(), |n| n.to_string_lossy().to_string());
        let up = name.to_ascii_uppercase();
        if !GEAR_SUBMITTAL_WORDS.iter().any(|w| up.contains(w)) {
            continue;
        }
        let Ok(text) =
            crate::readfile::read(&f).map_err(|e| e.to_string()).and_then(|b| super::spec_import::pdf_text(&b, &f).map_err(|e| e.to_string()))
        else {
            continue;
        };
        for (k, v) in super::gearbuild::submittal_gear(&text, &tags, &name) {
            gear.entry(k).or_insert(v);
        }
        read_files.push((f.clone(), name, text));
    }
    // Gear the text gives no size for: its elevation drawing (a page naming it and its sections),
    // read by position; else an agent's answer; else a question for the agent.
    let wanted: Vec<String> = sp
        .mep
        .panel_schedules
        .iter()
        .filter(|p| p.typical_of.is_none() && !sp.mep.equipment.iter().any(|e| e.tag.eq_ignore_ascii_case(&p.panel)))
        .map(|p| p.panel.clone())
        .filter(|t| gear.get(t).is_none_or(|g| g.size_in == [0.0; 3]))
        .collect();
    let read_pdf = super::file::io().and_then(|h| h.read_pdf);
    let mut looked: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for tag in &wanted {
        let up = tag.to_ascii_uppercase();
        'files: for (f, name, text) in &read_files {
            for page in text.split("--- page ").skip(1) {
                let n: u32 = page.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
                let words: Vec<String> = page.split(|c: char| !(c.is_ascii_alphanumeric() || c == '#')).map(str::to_ascii_uppercase).collect();
                if n == 0 || !words.iter().any(|w| w == "SECTION#") || !words.contains(&up) {
                    continue;
                }
                looked.entry(tag.clone()).or_default().push(format!("{name} p.{n}"));
                let Some(read) = read_pdf else { continue };
                let Ok(bytes) = crate::readfile::read(f) else { continue };
                let Ok((d, _)) = read(&bytes, &json!({"page": n, "text": true, "images": false})) else { continue };
                if let Some(e) = super::gearbuild::elevation_at(&cadaddy_mepfix::sheet_facts::texts(&d)) {
                    let g = gear.entry(tag.clone()).or_default();
                    g.size_in = e.size_in;
                    g.top_plate_in = e.top_plate_in;
                    g.bottom_plate_in = e.bottom_plate_in;
                    if g.kind.is_empty() {
                        g.kind = e.kind;
                    }
                    g.source = format!("{name} p.{n} (elevation drawing)");
                    break 'files;
                }
            }
        }
    }
    for tag in &wanted {
        if gear.get(tag).is_some_and(|g| g.size_in != [0.0; 3]) {
            continue;
        }
        // An agent's answer to an earlier question.
        if let Some(a) = job
            .rules
            .iter()
            .find(|r| r.key == format!("gear:{}", tag.to_ascii_uppercase()))
            .and_then(|r| serde_json::from_str::<Value>(&r.value).ok())
        {
            let n = |k: &str| a.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v > 0.0);
            let pair = |k: &str| a.get(k).and_then(Value::as_array).and_then(|v| Some([v.first()?.as_f64()?, v.get(1)?.as_f64()?]));
            if let (Some(h), Some(w), Some(dp)) = (n("height_in"), n("width_in"), n("depth_in")) {
                let g = gear.entry(tag.clone()).or_default();
                g.size_in = [h, w, dp];
                g.top_plate_in = pair("top_entry_in").or(g.top_plate_in);
                g.bottom_plate_in = pair("bottom_entry_in").or(g.bottom_plate_in);
                if let Some(feed) = a.get("feed").and_then(Value::as_str) {
                    g.feed = feed.to_ascii_lowercase();
                }
                g.source = "an agent's reading of the submittal (job answer)".into();
                continue;
            }
        }
        gear_question(job, tag, looked.get(tag).cloned().unwrap_or_default(), &read_files.iter().map(|(_, n, _)| n.clone()).collect::<Vec<_>>());
    }
    let rooms: Vec<super::gearbuild::RoomBox> = {
        let d = s.doc()?;
        let offsets: BTreeMap<cadaddy_doc::Handle, cadaddy_geom::Vec2> = d
            .model
            .iter()
            .filter(|e| super::wallgen::made_as(e).as_deref() == Some("room"))
            .map(|e| (e.handle, super::bspec::stacked_by(e)))
            .collect();
        super::bspec::room_elements(d)
            .into_iter()
            .map(|r| super::gearbuild::RoomBox {
                offset: offsets.get(&r.handle).copied().unwrap_or(cadaddy_geom::Vec2::ZERO),
                level: r.level,
                number: r.number.unwrap_or_default(),
                name: r.name,
                outline: r.outline,
            })
            .collect()
    };
    // The schedules' breakers left to the one-line (`OL`): the one-line's breaker and feeder.
    let mut feeds: Vec<super::equipsub::OnelineLoad> = Vec::new();
    for (_, d) in pages {
        for f in super::equipsub::oneline_loads(&cadaddy_mepfix::sheet_facts::texts(d), &[], &type_tags) {
            if !feeds.iter().any(|x| x.tag.eq_ignore_ascii_case(&f.tag)) {
                feeds.push(f);
            }
        }
    }
    let from_oneline = super::panelsched::fill_from_oneline(&mut sp, &feeds, &types);
    let planned = super::gearbuild::plan(&sp, &types, &incoming, &gear, &rooms);
    let new_tags: BTreeSet<String> = planned.equipment.iter().map(|e| e.tag.to_ascii_uppercase()).collect();
    let placed: Vec<String> =
        planned.equipment.iter().map(|e| format!("{} ({})", e.tag, e.sources.get("position").cloned().unwrap_or_default())).collect();
    sp.mep.equipment.extend(planned.equipment);
    // The one-line's feeders replace those read the same way before (a rerun).
    sp.mep.feeders.retain(|f| !f.source.contains("one-line tag"));
    let feeders = planned.feeders.len();
    sp.mep.feeders.extend(planned.feeders);
    s.doc_mut()?.building_spec = Some(sp.to_json());
    // The new gear built, then moved as the build moved its level to stack it.
    let d = s.doc_mut()?;
    let built = super::mep::build_equipment(d, &sp, Some(&new_tags));
    for h in built {
        let level = d.model.iter().find(|e| e.handle == h).and_then(|e| super::bspec::made_level(e));
        let off = level.and_then(|l| rooms.iter().find(|r| r.level.eq_ignore_ascii_case(&l)).map(|r| r.offset)).unwrap_or(cadaddy_geom::Vec2::ZERO);
        if off.len() > 1e-9 {
            let m = cadaddy_geom::Mat3::translate(off);
            let _ = d.modify_entity(h, |e| {
                e.kind.transform(&m);
                e.common.xdata = super::bspec::with_stacked(&e.common.xdata, off).into();
            });
        }
    }
    s.touch();
    // The gear checked against what lands on it; each failure an RFI.
    let review = s.execute("gear.review", &json!({})).unwrap_or_else(|e| json!({"error": e.to_string()}));
    let findings: Vec<&Value> = review.get("findings").and_then(Value::as_array).into_iter().flatten().collect();
    let line = format!(
        "  electrical gear: feeder types {} ({}); one-line feeds {}; schedule circuits given the one-line's breaker/feeder {from_oneline}; gear submittals give {} panels; laid out {}: {}; feeders {feeders}; unplaced {:?}; gear review: {} findings",
        types.len(),
        type_tags.join(", "),
        incoming.len(),
        gear.len(),
        placed.len(),
        placed.join("; "),
        planned.unplaced,
        findings.len()
    );
    report_progress(s, json!({"step": "electrical gear"}), &line);
    s.echo(line);
    for f in findings.iter().take(MAX_QUESTIONS_PER_STAGE) {
        let msg = f.get("message").and_then(Value::as_str).unwrap_or_default();
        let fix = f.get("fix").and_then(Value::as_str).unwrap_or_default();
        job.add_finding(
            StageName::BimBuild,
            &format!("Gear {}: {msg}", f.get("gear").and_then(Value::as_str).unwrap_or_default()),
            "gear.check against the feeders that land on it",
            2,
            vec![fix.to_string()],
            None,
            false,
        );
    }
    result["gear"] = json!({"placed": placed, "feeders": feeders, "unplaced": planned.unplaced, "review": review});
    equipment_step(s, job, pages, &types, &tags, result)
}

/// The lighting step ([`super::lightsched`]): the fixture schedule (by its grid) into the spec and
/// onto each luminaire of its type; RFIs where the plans, the schedule and the submittal disagree.
fn lighting_step(s: &mut Session, job: &mut GuidedJob, pages: &[(String, cadaddy_doc::Drawing)], result: &mut Value) -> crate::Result<()> {
    use super::lightsched;
    use cadaddy_doc::props::{self, Prop, Source, SourceKind};
    let mut types: Vec<cadaddy_walk3d::FixtureType> = Vec::new();
    let mut no_subs = false;
    for (number, d) in pages {
        let texts = cadaddy_mepfix::sheet_facts::texts(d);
        if !texts
            .iter()
            .any(|t| t.value.to_ascii_uppercase().contains("FIXTURE SCHEDULE") || t.value.to_ascii_uppercase().contains("LUMINAIRE SCHEDULE"))
        {
            continue;
        }
        let segs = super::gridtable::segments(d);
        for title in ["LIGHT FIXTURE SCHEDULE", "LIGHTING FIXTURE SCHEDULE", "LUMINAIRE SCHEDULE"] {
            for tb in super::gridtable::read_titled(&texts, &segs, title, number) {
                let (rows, ns) = lightsched::fixture_types(&tb);
                no_subs |= ns;
                for r in rows {
                    if !types.iter().any(|t| t.tag == r.tag) {
                        types.push(r);
                    }
                }
            }
        }
    }
    if types.is_empty() {
        return Ok(());
    }
    // Each luminaire: its type's row; what was submitted for it.
    let d = s.doc()?;
    let mut changes: Vec<(cadaddy_doc::Handle, props::Props)> = Vec::new();
    let mut on_plans: BTreeMap<String, usize> = BTreeMap::new();
    let mut submitted: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    for (e, base) in super::elemprops::elements(d).take(2_000_000) {
        let kind = super::elemprops::reported_kind(e, base.clone());
        if !matches!(kind.as_str(), "luminaire" | "wall_luminaire" | "light_fixture") {
            continue;
        }
        let mut store = props::read(&e.common.xdata);
        let text = |st: &props::Props, k: &str| st.get(k).filter(|p| p.has_value()).map(|p| p.text().trim().to_string()).unwrap_or_default();
        let mark = {
            let m = text(&store, "type_mark");
            if m.is_empty() { text(&store, "type") } else { m }
        };
        let ty = lightsched::type_of(&mark);
        if ty.is_empty() {
            continue;
        }
        *on_plans.entry(ty.clone()).or_default() += 1;
        let (maker, cat) = (text(&store, "manufacturer"), text(&store, "catalog_number"));
        if !maker.is_empty() || !cat.is_empty() {
            submitted.entry(ty.clone()).or_default().insert((maker, cat));
        }
        let Some(row) = types.iter().find(|t| t.tag == ty) else { continue };
        let before = store.clone();
        let src = Source::from_doc(SourceKind::Schedule, &format!("{} type {}", row.source, row.tag));
        for (k, v) in [
            ("basis_of_design", format!("{} {}", row.manufacturer, row.model).trim().to_string()),
            ("lamp", row.lamp.clone()),
            ("voltage", row.voltage.clone()),
            ("installation", row.installation.clone()),
            ("fixture_description", row.description.clone()),
            ("schedule_notes", row.notes.clone()),
        ] {
            let keep = store.get(k).is_some_and(|old| old.source.kind == SourceKind::User);
            if !keep && !v.is_empty() {
                store.set(k, Prop::new(v, src.clone()));
            }
        }
        if let Some(va) = row.va
            && !store.get("fixture_va").is_some_and(|o| o.source.kind == SourceKind::User)
        {
            store.set("fixture_va", Prop::new(va, src.clone()).with_unit("VA"));
        }
        if store != before {
            changes.push((e.handle, store));
        }
    }
    let d = s.doc_mut()?;
    for (h, store) in changes.iter() {
        let _ = d.modify_entity(*h, |e| e.common.xdata = props::write(&e.common.xdata, store).into());
    }
    // The documents against each other.
    let mut issues: Vec<(String, String, String)> = Vec::new();
    for (ty, n) in &on_plans {
        if !types.iter().any(|t| &t.tag == ty) {
            issues.push((
                ty.clone(),
                format!("{n} luminaires of type {ty} on the plans; the light fixture schedule has no type {ty}"),
                "Add the type to the schedule (basis of design, lamp, voltage, mounting) or correct the plans' tags".into(),
            ));
        }
    }
    for t in &types {
        let Some(subs) = submitted.get(&t.tag) else { continue };
        for (maker, cat) in subs {
            let other_maker = !maker.is_empty() && !t.manufacturer.is_empty() && !lightsched::same_maker(&t.manufacturer, maker);
            let other_product = !cat.is_empty() && !t.model.is_empty() && !lightsched::same_product(&t.model, cat);
            if other_maker || other_product {
                let note = if no_subs { "; the schedule's notes allow no substitutions unless approved" } else { "" };
                issues.push((
                    t.tag.clone(),
                    format!(
                        "type {}: the schedule's basis of design is {} {}; the submittal gives {} {}{note}",
                        t.tag, t.manufacturer, t.model, maker, cat
                    ),
                    "Confirm the substitution is approved (or resubmit the basis of design)".into(),
                ));
            }
        }
    }
    let unshown: Vec<String> = types.iter().filter(|t| !on_plans.contains_key(&t.tag)).map(|t| t.tag.clone()).collect();
    let Ok((mut sp, true)) = super::bspec::current(s, "bim_build") else { return Ok(()) };
    let mut rfis = Vec::new();
    for (ty, issue, fix) in &issues {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in issue.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        let id = format!("RFI-L{:08x}", h as u32);
        if !sp.rfis.iter().any(|r| r.id == id) && sp.rfis.len() < cadaddy_walk3d::rfi::MAX_RFIS {
            let kind = cadaddy_walk3d::RfiKind::Lighting;
            sp.rfis.push(cadaddy_walk3d::Rfi {
                id: id.clone(),
                kind: Some(kind),
                severity: kind.severity(),
                about: format!("lighting type {ty}"),
                issue: issue.clone(),
                assumed: "The model keeps the plans' type and the submitted product until answered.".into(),
                proposed: Some(fix.clone()),
                ..cadaddy_walk3d::Rfi::default()
            });
            rfis.push(id);
        }
        job.add_finding(StageName::BimBuild, issue, "the lighting documents disagree", 1, vec![fix.clone()], None, false);
    }
    sp.mep.fixture_schedule = types.clone();
    s.doc_mut()?.building_spec = Some(sp.to_json());
    s.touch();
    let line = format!(
        "  lighting: fixture schedule {} types ({}){}; luminaires given their row {}; on the plans {:?}; scheduled but not on the plans {:?}; conflicts {}",
        types.len(),
        types.iter().map(|t| t.tag.as_str()).collect::<Vec<_>>().join(", "),
        if no_subs { ", no substitutions" } else { "" },
        changes.len(),
        on_plans,
        unshown,
        issues.len()
    );
    report_progress(s, json!({"step": "lighting"}), &line);
    s.echo(line);
    result["lighting"] = json!({"types": types.len(), "luminaires": changes.len(), "conflicts": issues.len(), "rfis": rfis, "unshown": unshown});
    controls_step(s, job, &types, result)
}

/// Lighting controls: each luminaire's control by its room and switch leg (the plans' letters on
/// fixtures and switches), else its circuit's (a contactor or time switch per the panel schedule's
/// codes), else none; emergency and exit fixtures unswitched by design. Conflicts: a leg no
/// switch in the room has, a switch leg no fixture has, a 3-way without its pair, a dimmer over
/// a fixture whose driver doesn't dim (or dims another way), a fixture with no control at all.
fn controls_step(s: &mut Session, job: &mut GuidedJob, types: &[cadaddy_walk3d::FixtureType], result: &mut Value) -> crate::Result<()> {
    use cadaddy_doc::props::{self, Prop, Source, SourceKind};
    let Ok((mut sp, true)) = super::bspec::current(s, "bim_build") else { return Ok(()) };
    // Each circuit's codes (C, TS, EM...) by panel and number.
    let mut codes: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for p in &sp.mep.panel_schedules {
        for c in &p.circuits {
            codes.insert((p.panel.to_ascii_uppercase(), c.number.clone()), c.protection.clone());
        }
    }
    struct Dev {
        h: cadaddy_doc::Handle,
        room: String,
        leg: String,
        mark: String,
        kind: String,
        dimming: String,
        product: String,
        circuit: (String, String),
        level: String,
        at: Option<[f64; 2]>,
    }
    let d = s.doc()?;
    let mut lums: Vec<Dev> = Vec::new();
    let mut ctrls: Vec<Dev> = Vec::new();
    // Other loads a switch leg may switch (a half-switched receptacle, a fan): those with a leg.
    let mut others: Vec<Dev> = Vec::new();
    for (e, base) in super::elemprops::elements(d).take(2_000_000) {
        let kind = super::elemprops::reported_kind(e, base.clone());
        let is_lum = matches!(kind.as_str(), "luminaire" | "wall_luminaire" | "light_fixture");
        let is_ctl = matches!(kind.as_str(), "switch" | "3way_switch" | "dimmer" | "occupancy_sensor");
        if !is_lum && !is_ctl && !matches!(base.as_str(), "device" | "equipment" | "fixture") {
            continue;
        }
        let st = props::read(&e.common.xdata);
        let text = |k: &str| st.get(k).filter(|p| p.has_value()).map(|p| p.text().trim().to_string()).unwrap_or_default();
        let mark = {
            let m = text("type_mark");
            if m.is_empty() { super::lightsched::type_of(&text("type")) } else { m.to_ascii_uppercase() }
        };
        let circuit = text("circuit");
        let number = circuit.rsplit_once(['-', '/', '.']).map_or(String::new(), |(_, n)| n.trim().to_string());
        let dv = Dev {
            h: e.handle,
            room: text("room").to_ascii_uppercase(),
            leg: text("switch_leg").to_ascii_lowercase(),
            mark,
            kind,
            dimming: text("dimming").to_ascii_uppercase(),
            product: format!("{} {} {}", text("product"), text("configuration"), text("required_features")).to_ascii_uppercase(),
            circuit: (text("panel").to_ascii_uppercase(), number),
            level: text("level").to_ascii_uppercase(),
            at: {
                let v: Vec<f64> = text("location").split(',').filter_map(|x| x.trim().parse::<f64>().ok()).collect();
                (v.len() >= 2).then(|| [v[0], v[1]])
            },
        };
        if is_lum {
            lums.push(dv);
        } else if is_ctl {
            ctrls.push(dv);
        } else if !dv.leg.is_empty() {
            others.push(dv);
        }
    }
    let emergency = |mark: &str| {
        types.iter().find(|t| t.tag == mark).is_some_and(|t| {
            let u = format!("{} {}", t.description, t.notes).to_ascii_uppercase();
            u.contains("EXIT") || u.contains("EMERGENCY")
        })
    };
    let is_dimmer = |c: &Dev| c.mark.starts_with("SD") || c.mark.starts_with("D") && c.mark.len() <= 3 || c.product.contains("DIMMER");
    let dim_kind = |text: &str| -> &'static str {
        if text.contains("0-10") {
            "0-10V"
        } else if ["ELV", "MLV", "PHASE", "FORWARD", "REVERSE", "TRIAC"].iter().any(|w| text.contains(w)) {
            "phase"
        } else if text.contains("NON-DIM") || text == "NONE" || text.contains("NO DIM") {
            "none"
        } else {
            ""
        }
    };
    // Within reach on one level (a leg's switch is often across a door, in the next space).
    let near = |a: &Dev, b: &Dev, reach: f64| {
        let same_level = a.level.is_empty() || b.level.is_empty() || a.level == b.level;
        same_level
            && match (a.at, b.at) {
                (Some(p), Some(q)) => ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt() <= reach,
                _ => !a.room.is_empty() && a.room == b.room,
            }
    };
    let mut changes: Vec<(cadaddy_doc::Handle, props::Props)> = Vec::new();
    let mut issues: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let mut tally: BTreeMap<&'static str, usize> = BTreeMap::new();
    for l in &lums {
        let in_room: Vec<&Dev> = ctrls.iter().filter(|c| !l.room.is_empty() && c.room == l.room).collect();
        let by_leg: Vec<&Dev> = ctrls.iter().filter(|c| !l.leg.is_empty() && c.leg == l.leg && near(l, c, CONTROL_REACH_FT)).collect();
        let circuit_codes = codes.get(&l.circuit).cloned().unwrap_or_default();
        let (control, src, how): (String, Source, &'static str) = if !by_leg.is_empty() {
            (
                format!("switch leg {}: {}", l.leg, by_leg.iter().map(|c| format!("{} {}", c.mark, c.h.hex())).collect::<Vec<_>>().join(", ")),
                Source::from_doc(SourceKind::Print, "switch leg letters on the plan"),
                "leg",
            )
        } else if circuit_codes.iter().any(|c| c == "C" || c == "TS") {
            (
                format!(
                    "{} (panel {} circuit {}: {})",
                    if circuit_codes.iter().any(|c| c == "TS") { "time switch" } else { "contactor" },
                    l.circuit.0,
                    l.circuit.1,
                    circuit_codes.join("/")
                ),
                Source::from_doc(SourceKind::Schedule, "panel schedule codes"),
                "contactor",
            )
        } else if emergency(&l.mark) || circuit_codes.iter().any(|c| c == "EM") {
            ("unswitched: emergency / exit".to_string(), Source::from_doc(SourceKind::Schedule, "light fixture schedule"), "emergency")
        } else if !in_room.is_empty() && l.leg.is_empty() {
            (
                format!("the room's switches: {}", in_room.iter().map(|c| format!("{} {}", c.mark, c.h.hex())).collect::<Vec<_>>().join(", ")),
                Source::computed(),
                "room",
            )
        } else {
            ("none found".to_string(), Source::computed(), "none")
        };
        *tally.entry(how).or_default() += 1;
        if how == "none" {
            let what = if l.leg.is_empty() { "fixture with no control" } else { "switch leg with no switch" };
            issues.entry(what).or_default().push(format!("{} {} in {}", l.mark, l.h.hex(), if l.room.is_empty() { "no room" } else { &l.room }));
        }
        // A dimmer over a fixture that doesn't dim, or dims another way.
        for c in by_leg.iter().filter(|c| is_dimmer(c)) {
            let (dk, lk) = (dim_kind(&c.product), dim_kind(&l.dimming));
            if lk == "none" || (!dk.is_empty() && !lk.is_empty() && dk != lk) {
                issues.entry("dimmer over a fixture that doesn't dim that way").or_default().push(format!(
                    "{} {} ({}) under {} {} ({}) in {}",
                    l.mark,
                    l.h.hex(),
                    if lk.is_empty() { "?" } else { lk },
                    c.mark,
                    c.h.hex(),
                    if dk.is_empty() { "?" } else { dk },
                    l.room
                ));
            }
        }
        if let Some(e) = d.model.iter().find(|e| e.handle == l.h) {
            let mut st = props::read(&e.common.xdata);
            if !st.get("control").is_some_and(|o| o.source.kind == SourceKind::User) {
                st.set("control", Prop::new(control, src));
                changes.push((l.h, st));
            }
        }
    }
    // Switches: a leg no fixture near it has; a 3-way with no other 3-way of its leg within reach
    // (its pair is usually at the other door or the other end of a corridor or stair).
    let three = |c: &Dev| c.mark.starts_with("S3") || c.kind == "3way_switch" || c.product.contains("THREE WAY");
    for c in &ctrls {
        if c.kind == "occupancy_sensor" {
            continue;
        }
        // Its leg's loads: lettered fixtures or other loads near it; else unlettered fixtures near
        // it (a stamped unit's fixtures carry no letters), presumed its.
        let lettered = lums.iter().chain(others.iter()).any(|l| l.leg == c.leg && near(l, c, CONTROL_REACH_FT));
        let presumed = lums.iter().any(|l| l.leg.is_empty() && near(l, c, CONTROL_REACH_FT));
        if !c.leg.is_empty() && !lettered && !presumed {
            issues.entry("switch leg with no fixture").or_default().push(format!("{} {} leg {} in {}", c.mark, c.h.hex(), c.leg, c.room));
        }
        if three(c) && !ctrls.iter().any(|o| o.h != c.h && three(o) && o.leg == c.leg && near(o, c, THREE_WAY_REACH_FT)) {
            issues.entry("3-way switch without its pair").or_default().push(format!(
                "{} {} leg {} in {}",
                c.mark,
                c.h.hex(),
                if c.leg.is_empty() { "-" } else { &c.leg },
                c.room
            ));
        }
    }
    let d = s.doc_mut()?;
    for (h, st) in &changes {
        let _ = d.modify_entity(*h, |e| e.common.xdata = props::write(&e.common.xdata, st).into());
    }
    let mut rfis = Vec::new();
    for (what, list) in &issues {
        let shown: Vec<&String> = list.iter().take(12).collect();
        let more = if list.len() > shown.len() { format!(" and {} more", list.len() - shown.len()) } else { String::new() };
        let issue = format!("Lighting controls: {} {what}: {}{more}", list.len(), shown.iter().map(|x| x.as_str()).collect::<Vec<_>>().join("; "));
        let fix = match *what {
            "fixture with no control" => "Show its switch leg (or its contactor / sensor), or note it unswitched",
            "switch leg with no switch" => "Add the switch for that leg, or correct the fixture's leg letter",
            "switch leg with no fixture" => "Correct the switch's leg letter, or add the fixtures it controls",
            "3-way switch without its pair" => "Show the second 3-way switch, or make it single pole",
            _ => "Match the dimmer to the driver (0-10V dimmer for 0-10V drivers, phase dimmer for phase-dimmable), or the fixture to the dimmer",
        };
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in issue.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        let id = format!("RFI-L{:08x}", h as u32);
        if !sp.rfis.iter().any(|r| r.id == id) && sp.rfis.len() < cadaddy_walk3d::rfi::MAX_RFIS {
            let kind = cadaddy_walk3d::RfiKind::Lighting;
            sp.rfis.push(cadaddy_walk3d::Rfi {
                id: id.clone(),
                kind: Some(kind),
                severity: kind.severity(),
                about: format!("lighting controls ({what})"),
                issue: issue.clone(),
                assumed: "The model links what the plans' letters link; the rest is held.".into(),
                proposed: Some(fix.into()),
                ..cadaddy_walk3d::Rfi::default()
            });
            rfis.push(id);
        }
        job.add_finding(StageName::BimBuild, &issue, "lighting controls on the plans", 1, vec![fix.into()], None, false);
    }
    s.doc_mut()?.building_spec = Some(sp.to_json());
    s.touch();
    let line = format!(
        "  lighting controls: {} luminaires by {:?}; {} controls; conflicts {}",
        lums.len(),
        tally,
        ctrls.len(),
        issues.iter().map(|(k, v)| format!("{k}: {}", v.len())).collect::<Vec<_>>().join(", ")
    );
    report_progress(s, json!({"step": "lighting controls"}), &line);
    s.echo(line);
    result["lighting_controls"] =
        json!({"by": tally, "issues": issues.iter().map(|(k, v)| (k.to_string(), v.len())).collect::<BTreeMap<_, _>>(), "rfis": rfis});
    Ok(())
}

/// How far a switch leg's switch may be from its fixtures, and a 3-way from its pair, feet.
const CONTROL_REACH_FT: f64 = 40.0;
const THREE_WAY_REACH_FT: f64 = 150.0;

/// `bim.coderules`' receptacle circuits in GFCI places on breakers that aren't: `A-13 RCPT - ELEVATOR PIT (NEC 620.6 …)`.
fn gfci_places(v: &Value) -> Vec<String> {
    v.get("gfci_circuits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| {
            let g = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or_default();
            format!("{}-{} {} ({})", g("panel"), g("circuit"), g("description"), g("rule"))
        })
        .collect()
}

/// How far a keynote's tag may be from the symbol it points at (its leader), in its text heights.
const KEYNOTE_REACH: f64 = 8.0;
/// Two levels' riser boxes this close in plan (feet) are one riser.
const RISER_REACH_FT: f64 = 15.0;
/// How far a stub goes into the space it is stubbed into, feet.
const STUB_INTO_FT: f64 = 1.5;
/// Buried conduit a keynote calls for runs this far under the slab to its top, feet, and comes up
/// this far in the wall it reaches.
const UNDER_SLAB_FT: f64 = 1.5;
const STUB_UP_FT: f64 = 1.5;
/// Most boxes the keynotes add.
const MAX_KEYNOTE_BOXES: usize = 400;

/// One box a keynote asks for, where it is.
struct KeynoteBox {
    sheet: String,
    number: String,
    text: String,
    work: super::keynotes::Work,
    level: String,
    /// On its level's plan (drawing units).
    plan: cadaddy_geom::Vec2,
    /// The device (its index in the spec's tags) it is, when its tag points at one.
    device: Option<usize>,
}

/// The electrical keynotes' work ([`super::keynotes`]): each actionable note's boxes where its
/// tags point (a junction box the plan shows takes its size and system; else a box of its size
/// at the tag), and its conduit: between each of its boxes floor to floor (a riser), stubbed into
/// the nearest room so named, or run to its system's riser box on the level. All at the level's
/// ceiling; what the documents leave to a vendor ("as directed by the BDA system vendor") said so.
fn keynote_step(s: &mut Session, job: &mut GuidedJob, pages: &[(String, cadaddy_doc::Drawing)], result: &mut Value) -> crate::Result<()> {
    use super::keynotes::{self, Route};
    use cadaddy_geom::Vec2;
    let Ok((mut sp, true)) = super::bspec::current(s, "bim_build") else { return Ok(()) };
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    // A sheet by its number (`E-4/3`: E-4's view of level 3).
    let norm = |v: &str| cadaddy_mepfix::printlinks::norm_sheet(v.split('/').next().unwrap_or(v));
    // The devices: where each one's symbol is on its sheet, and where it is on its level.
    struct Dev {
        i: usize,
        sheet: String,
        level: String,
        doc: Vec2,
        plan: Vec2,
        junction: bool,
    }
    let devs: Vec<Dev> = {
        let d = s.doc()?;
        sp.tags
            .iter()
            .enumerate()
            .filter(|(_, t)| t.id.starts_with("device:"))
            .filter_map(|(i, t)| {
                let h = cadaddy_doc::Handle::parse_hex(&t.fields.get("symbol")?.value)?;
                let b = cadaddy_doc::entity_bounds(d, d.model.get(h)?, 0);
                (!b.is_empty()).then(|| Dev {
                    i,
                    sheet: norm(&t.fields.get("sheet").map(|f| f.value.clone()).unwrap_or_default()),
                    level: t.level.clone(),
                    doc: b.center(),
                    plan: Vec2::new(t.at_ft[0] * upf, t.at_ft[1] * upf),
                    junction: t.kind.contains("junction"),
                })
            })
            .collect()
    };
    let mut boxes: Vec<KeynoteBox> = Vec::new();
    for (number, sub) in pages {
        let texts = cadaddy_mepfix::sheet_facts::texts(sub);
        let lists = keynotes::lists(&texts);
        let Some(list) = lists.iter().find(|l| l.heading.to_ascii_uppercase().contains("ELECTRICAL")).or_else(|| lists.first()) else { continue };
        let actionable: Vec<(&keynotes::Note, keynotes::Work)> = list.notes.iter().filter_map(|n| keynotes::work(&n.text).map(|w| (n, w))).collect();
        if actionable.is_empty() {
            continue;
        }
        let rect = texts.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |r, t| {
            [r[0].min(t.at.x), r[1].min(t.at.y), r[2].max(t.at.x), r[3].max(t.at.y)]
        });
        let segs = keynotes::sheet_segments(sub, rect);
        let mut shapes: BTreeMap<String, bool> = BTreeMap::new();
        let symbol_boxes: Vec<[f64; 4]> = sub
            .model
            .iter()
            .filter(|e| match &e.kind {
                cadaddy_doc::EntityKind::Insert(ins) => {
                    *shapes.entry(ins.block.clone()).or_insert_with(|| symbol_shape(sub, &ins.block).is_some_and(|sh| plain_box(&sh)))
                }
                _ => false,
            })
            .map(|e| cadaddy_doc::entity_bounds(sub, e, 0))
            .filter(|b| !b.is_empty())
            .map(|b| [b.min.x, b.min.y, b.max.x, b.max.y])
            .collect();
        let sheet = norm(number);
        let on_sheet: Vec<&Dev> = devs.iter().filter(|d| d.sheet == sheet).collect();
        // Circles, and symbols (not keynote boxes, radius -1: a number right over one's middle is
        // its own: a callout bubble the import made a block).
        let rounds: Vec<(Vec2, f64)> = sub
            .model
            .iter()
            .filter_map(|e| match &e.kind {
                cadaddy_doc::EntityKind::Circle(c) => Some((c.center.xy(), c.radius)),
                cadaddy_doc::EntityKind::Insert(ins) if !shapes.get(&ins.block).copied().unwrap_or(false) => Some((ins.insert.xy(), -1.0)),
                _ => None,
            })
            .collect();
        let me = keynotes::subject(list);
        let others: Vec<String> = lists.iter().map(keynotes::subject).filter(|w| *w != me && !w.is_empty()).collect();
        for (num, at) in keynotes::tags(&texts, &segs, &symbol_boxes, &rounds, &others, list) {
            let Some((note, w)) = actionable.iter().find(|(n, _)| n.number == num) else { continue };
            if boxes.len() >= MAX_KEYNOTE_BOXES {
                break;
            }
            let at = Vec2::new(at[0], at[1]);
            let reach = KEYNOTE_REACH * list.number_h;
            let nearest = |junction: bool| {
                on_sheet.iter().filter(|d| !junction || d.junction).map(|d| ((d.doc - at).len(), *d)).min_by(|a, b| a.0.total_cmp(&b.0))
            };
            // A box note's tag points at its junction box; a buried note's at what it serves (a floor box).
            let anchor = if w.box_in.is_some() {
                nearest(true)
            } else if w.buried {
                nearest(false)
            } else {
                None
            };
            let (level, plan, device) = match anchor.filter(|(dd, _)| *dd <= reach) {
                Some((_, dv)) => (dv.level.clone(), dv.plan, Some(dv.i)),
                None => {
                    // The tag's place on its level: the nearest devices' offset from sheet to plan.
                    let mut near: Vec<(f64, &&Dev)> = on_sheet.iter().map(|d| ((d.doc - at).len(), d)).collect();
                    near.sort_by(|a, b| a.0.total_cmp(&b.0));
                    let Some((_, first)) = near.first() else { continue };
                    let same: Vec<Vec2> = near.iter().filter(|(_, d)| d.level == first.level).take(3).map(|(_, d)| d.plan - d.doc).collect();
                    let off = same.iter().fold(Vec2::ZERO, |a, b| a + *b) * (1.0 / same.len().max(1) as f64);
                    (first.level.clone(), at + off, None)
                }
            };
            boxes.push(KeynoteBox { sheet: number.clone(), number: num.clone(), text: note.text.clone(), work: w.clone(), level, plan, device });
        }
    }
    if boxes.is_empty() {
        return Ok(());
    }
    // One box per spot (a tag on two sheets of one level).
    boxes.dedup_by(|a, b| a.level == b.level && (a.plan - b.plan).len() < 1.0 * upf && a.work.system == b.work.system);
    let level_of = |name: &str| sp.levels.iter().find(|l| l.name.eq_ignore_ascii_case(name)).cloned();
    let mount = |name: &str| level_of(name).map(|l| if l.ceiling_ft.is_finite() && l.ceiling_ft > 0.0 { l.ceiling_ft } else { 9.0 }).unwrap_or(9.0);
    let fmt_in = |b: [f64; 3]| format!("{}\"x{}\"x{}\"", b[0], b[1], b[2]);
    let mut new_equipment: BTreeSet<String> = BTreeSet::new();
    let (mut on_devices, mut at_tags) = (0usize, 0usize);
    for (k, b) in boxes.iter().enumerate() {
        let why = format!("{} keynote {}: {}", b.sheet, b.number, b.text);
        let size = b.work.box_in.map(fmt_in).unwrap_or_default();
        match b.device.and_then(|i| sp.tags.get_mut(i)) {
            Some(t) => {
                t.set("keynote", why.clone(), "note");
                if !b.work.system.is_empty() {
                    t.set("system", b.work.system.clone(), "note");
                }
                if !size.is_empty() {
                    t.set("box_size", size.clone(), "note");
                }
                on_devices += 1;
            }
            None => {
                let Some(bx) = b.work.box_in else { continue };
                let (w, dpt) = (bx[1] / 12.0 * upf, bx[2] / 12.0 * upf);
                let tag = format!("{}-JB-{}", if b.work.system.is_empty() { "KN" } else { b.work.system.as_str() }, k + 1);
                let mut props = BTreeMap::new();
                props.insert("size".to_string(), size.clone());
                props.insert("keynote".to_string(), why.clone());
                let mut sources = BTreeMap::new();
                sources.insert("position".to_string(), format!("{} keynote {} tag (the symbol it points at not read)", b.sheet, b.number));
                sources.insert("size".to_string(), format!("{} keynote {}", b.sheet, b.number));
                sources.insert("elevation".to_string(), "rule: at the level's ceiling".to_string());
                sp.mep.equipment.push(cadaddy_walk3d::MepEquipment {
                    tag: tag.clone(),
                    kind: "junction_box".into(),
                    level: b.level.clone(),
                    min: [b.plan.x - w / 2.0, b.plan.y - dpt / 2.0],
                    max: [b.plan.x + w / 2.0, b.plan.y + dpt / 2.0],
                    bottom_ft: (mount(&b.level) - bx[0] / 12.0).max(0.0),
                    height_ft: bx[0] / 12.0,
                    systems: vec![b.work.system.clone()],
                    title: why,
                    properties: props,
                    sources,
                    confidence: 0.6,
                    ..Default::default()
                });
                new_equipment.insert(tag.to_ascii_uppercase());
                at_tags += 1;
            }
        }
    }
    // The conduit.
    let mut runs: Vec<cadaddy_walk3d::MepRun> = Vec::new();
    let rooms = {
        let d = s.doc()?;
        let offsets: BTreeMap<cadaddy_doc::Handle, Vec2> = d
            .model
            .iter()
            .filter(|e| super::wallgen::made_as(e).as_deref() == Some("room"))
            .map(|e| (e.handle, super::bspec::stacked_by(e)))
            .collect();
        super::bspec::room_elements(d)
            .into_iter()
            .map(|r| {
                let off = offsets.get(&r.handle).copied().unwrap_or(Vec2::ZERO);
                (r.level, r.name.to_ascii_uppercase(), r.outline.iter().map(|p| *p - off).collect::<Vec<Vec2>>())
            })
            .collect::<Vec<_>>()
    };
    let system_of = |w: &keynotes::Work| format!("E-{}", if w.system.is_empty() { "KEYNOTE" } else { w.system.as_str() });
    let mut risers = 0usize;
    for b in &boxes {
        let sys = system_of(&b.work);
        let h = mount(&b.level);
        for (size, route) in &b.work.conduits {
            let mut r = super::printcond::run(&sys, &b.level, &b.sheet, Vec::new(), *size, h);
            r.size_source = "note".into();
            r.height_source = "rule".into();
            match route {
                Route::Riser => {
                    // Up to the same note's box on the next level, when it is above this one.
                    let Some(lv) = level_of(&b.level) else { continue };
                    // Each level's plan has its own place in the drawing: compared from its corner.
                    let corner = |l: &cadaddy_walk3d::Level| l.region.map_or(Vec2::ZERO, |r| Vec2::new(r[0], r[1]));
                    let here = b.plan - corner(&lv);
                    let above = boxes
                        .iter()
                        .filter(|o| o.text == b.text || (o.work.system == b.work.system && o.work.conduits.iter().any(|c| c.1 == Route::Riser)))
                        .filter_map(|o| Some((o, level_of(&o.level)?)))
                        .filter(|(o, ol)| ol.elevation_ft > lv.elevation_ft + 1.0 && (o.plan - corner(ol) - here).len() <= RISER_REACH_FT * upf)
                        .min_by(|a, b| a.1.elevation_ft.total_cmp(&b.1.elevation_ft));
                    let Some((o, ol)) = above else { continue };
                    r.points = vec![[b.plan.x, b.plan.y]];
                    r.bottoms_ft = vec![h];
                    r.top_ft = Some(ol.elevation_ft - lv.elevation_ft + mount(&o.level));
                    risers += 1;
                }
                Route::Stub(room) => {
                    // To the nearest such room's edge on its level, and a little into it.
                    let near = rooms
                        .iter()
                        .filter(|(l, n, o)| l.eq_ignore_ascii_case(&b.level) && n.contains(room.as_str()) && o.len() >= 3)
                        .flat_map(|(_, _, o)| (0..o.len()).filter_map(move |i| Some((*o.get(i)?, *o.get((i + 1) % o.len())?))))
                        .map(|(a, c)| {
                            let ab = c - a;
                            let t = if ab.len() > 1e-9 { ((b.plan - a).dot(ab) / ab.dot(ab)).clamp(0.0, 1.0) } else { 0.0 };
                            a + ab * t
                        })
                        .min_by(|p, q| (*p - b.plan).len().total_cmp(&(*q - b.plan).len()));
                    let Some(edge) = near else { continue };
                    let dir = edge - b.plan;
                    if dir.len() < 1e-6 {
                        continue;
                    }
                    let end = edge + dir * (STUB_INTO_FT * upf / dir.len());
                    r.points = vec![[b.plan.x, b.plan.y], [end.x, end.y]];
                }
                Route::UnderSlab => {
                    // Under the slab from it to the nearest wall of its room (else of any room on
                    // its level), into it from below and stubbed up in the wall.
                    let on_level: Vec<&Vec<Vec2>> =
                        rooms.iter().filter(|(l, _, o)| l.eq_ignore_ascii_case(&b.level) && o.len() >= 3).map(|(_, _, o)| o).collect();
                    let own: Vec<&Vec<Vec2>> = on_level.iter().copied().filter(|o| cadaddy_geom::point_in_polygon(o, b.plan)).collect();
                    let near = (if own.is_empty() { &on_level } else { &own })
                        .iter()
                        .flat_map(|o| (0..o.len()).filter_map(move |i| Some((*o.get(i)?, *o.get((i + 1) % o.len())?))))
                        .map(|(a, c)| {
                            let ab = c - a;
                            let t = if ab.len() > 1e-9 { ((b.plan - a).dot(ab) / ab.dot(ab)).clamp(0.0, 1.0) } else { 0.0 };
                            a + ab * t
                        })
                        .min_by(|p, q| (*p - b.plan).len().total_cmp(&(*q - b.plan).len()));
                    let Some(wall) = near else { continue };
                    if (wall - b.plan).len() < 1e-6 {
                        continue;
                    }
                    let called = format!("note: {} keynote {} (under the slab)", b.sheet, b.number);
                    let size_from = if b.work.size_assumed { "rule: smallest trade size (the note gives none)" } else { "note" };
                    r.points = vec![[b.plan.x, b.plan.y], [wall.x, wall.y]];
                    r.bottoms_ft = vec![-UNDER_SLAB_FT];
                    r.height_source = called.clone();
                    r.size_source = size_from.into();
                    for (at, top) in [(b.plan, 0.0), (wall, STUB_UP_FT)] {
                        let mut up = super::printcond::run(&sys, &b.level, &b.sheet, vec![[at.x, at.y]], *size, -UNDER_SLAB_FT);
                        up.top_ft = Some(top);
                        up.height_source = called.clone();
                        up.size_source = size_from.into();
                        runs.push(up);
                    }
                }
                Route::ToRiser => {
                    // To its system's riser box on its level.
                    let riser = boxes
                        .iter()
                        .filter(|o| o.level == b.level && o.work.system == b.work.system && o.work.conduits.iter().any(|c| c.1 == Route::Riser))
                        .min_by(|p, q| (p.plan - b.plan).len().total_cmp(&(q.plan - b.plan).len()));
                    let Some(o) = riser else { continue };
                    let corner = [o.plan.x, b.plan.y];
                    r.points = vec![[b.plan.x, b.plan.y], corner, [o.plan.x, o.plan.y]];
                    r.points.dedup();
                    r.bottoms_ft = vec![h; r.points.len().saturating_sub(1).max(1)];
                }
            }
            if !r.points.is_empty() {
                runs.push(r);
            }
        }
    }
    let run_count = runs.len();
    build_new_mep(s, sp, runs, &new_equipment)?;
    let systems: BTreeSet<&str> = boxes.iter().map(|b| b.work.system.as_str()).filter(|v| !v.is_empty()).collect();
    let line = format!(
        "  keynotes: {} boxes ({} on the junction boxes the plans show, {at_tags} added at their tags; systems {:?}); conduit runs {run_count} ({risers} risers)",
        boxes.len(),
        on_devices,
        systems
    );
    report_progress(s, json!({"step": "keynotes"}), &line);
    s.echo(line);
    if boxes.iter().any(|b| b.text.to_ascii_uppercase().contains("VENDOR")) {
        job.add_finding(
            StageName::BimBuild,
            &format!("{} system boxes and conduit built from the keynotes; their notes leave locations and requirements to the system vendor", systems.into_iter().collect::<Vec<_>>().join(", ")),
            "keynotes as the sheets print them: boxes where their tags point, conduit as the notes say (risers floor to floor, stubs, runs to the riser), at the level's ceiling",
            3,
            vec!["Confirm the box locations and the conduit with the system vendor (and where the corridor's ceiling is hard lid)".into()],
            None,
            false,
        );
    }
    result["keynotes"] = json!({"boxes": boxes.len(), "on_devices": on_devices, "added": at_tags, "runs": run_count, "risers": risers});
    Ok(())
}

/// Value engineering (Q6154, [`super::valueeng`]): the building's construction type from its
/// code sheet (the general sheets', else the architectural ones'), the specs' wiring-methods
/// tables, each alternative tiered and priced from the model; one needing an RFI gets its draft.
fn value_step(s: &mut Session, job: &mut GuidedJob, result: &mut Value) -> crate::Result<()> {
    if !matches!(super::bspec::current(s, "bim_build"), Ok((_, true))) {
        return Ok(());
    }
    let specs = job_specs(job);
    let mut construction: Option<(String, String)> = None;
    'find: for disc in ["G", "A"] {
        for (number, sub) in non_plan_pages(job, disc) {
            let texts = cadaddy_mepfix::sheet_facts::texts(&sub);
            let words = super::valueeng::reading_order(texts);
            if let Some(c) = super::valueeng::construction_type(&words) {
                construction = Some((c, number));
                break 'find;
            }
        }
    }
    let v = s.execute("ve.study", &json!({"specs": specs, "construction_type": construction.as_ref().map(|c| c.0.clone())}))?;
    let Ok((mut sp, true)) = super::bspec::current(s, "bim_build") else { return Ok(()) };
    let scenarios = v["scenarios"].as_array().cloned().unwrap_or_default();
    let mut lines = Vec::new();
    let mut rfis = Vec::new();
    for sc in &scenarios {
        let tier = sc["tier"].as_str().unwrap_or_default();
        let name = sc["name"].as_str().unwrap_or_default();
        let q = &sc["quantity"];
        let what = if q["circuits"].is_number() { format!("{} branch circuits", q["circuits"]) } else { format!("{} feeders", q["feeders"]) };
        let over = format!("{what}, {:.0} ft", q["feet"].as_f64().unwrap_or(0.0));
        let unpriced: Vec<&str> = sc["unpriced"].as_array().map(|u| u.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let money = match (sc["saves"]["cost"].as_f64(), sc["saves"]["labor_hours"].as_f64()) {
            (Some(c), Some(h)) => format!("; saves ${c:.0} and {h:.0} labor hours over {over}"),
            _ if sc["as_designed"].is_object() => format!(
                "; over {over}: savings not priced (no price for {}); of what is priced, as designed ${:.0} and {:.0} h, the alternative ${:.0} and {:.0} h",
                unpriced.join(", "),
                sc["as_designed"]["cost"].as_f64().unwrap_or(0.0),
                sc["as_designed"]["labor_hours"].as_f64().unwrap_or(0.0),
                sc["alternative"]["cost"].as_f64().unwrap_or(0.0),
                sc["alternative"]["labor_hours"].as_f64().unwrap_or(0.0),
            ),
            _ => String::new(),
        };
        lines.push(format!("{name}: {tier}{money}"));
        if !matches!(tier, "allowed_now" | "needs_rfi") {
            continue;
        }
        if let (true, Some(issue)) = (tier == "needs_rfi", sc["rfi"]["issue"].as_str()) {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for b in sc["id"].as_str().unwrap_or_default().bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
            let id = format!("RFI-V{:08x}", h as u32);
            sp.rfis.retain(|r| r.id != id);
            if sp.rfis.len() < cadaddy_walk3d::rfi::MAX_RFIS {
                let kind = cadaddy_walk3d::RfiKind::MethodChoice;
                sp.rfis.push(cadaddy_walk3d::Rfi {
                    id: id.clone(),
                    kind: Some(kind),
                    severity: kind.severity(),
                    about: name.to_string(),
                    issue: issue.to_string(),
                    assumed: "The model stays as designed; the alternative is a priced scenario until the RFI is approved.".into(),
                    proposed: sc["rfi"]["proposed"].as_str().map(str::to_string),
                    ..cadaddy_walk3d::Rfi::default()
                });
                rfis.push(id);
            }
        }
        job.add_finding(
            StageName::BimBuild,
            &format!("Value engineering ({}): {name}{money}", tier.replace('_', " ")),
            "a wiring method the code allows saves material and labor; the model stays as designed until approved",
            3,
            vec![if tier == "needs_rfi" { "Send the drafted RFI".into() } else { "Price it into the bid as allowed".into() }],
            None,
            false,
        );
    }
    if !rfis.is_empty() {
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    let line = format!(
        "  value engineering: construction type {}; {} wiring-method applications read; {}",
        construction.as_ref().map(|(c, n)| format!("{c} ({n})")).unwrap_or_else(|| "not found".into()),
        v["applications"].as_array().map_or(0, Vec::len),
        lines.join("; ")
    );
    report_progress(s, json!({"step": "value engineering"}), &line);
    s.echo(line);
    result["value_engineering"] = json!({"construction_type": construction.map(|c| c.0), "scenarios": scenarios, "rfis": rfis});
    Ok(())
}

/// The sheet number the generated underground sheets start at.
const UNDERGROUND_SHEET: &str = "E-UG1";

/// Buried work (Q6148). When the model has conduit under the slab (the keynotes' called runs,
/// the feeders proposed under it) or the documents' notes call for buried work, and the set
/// issues no plan of it (no title names buried work and a plan), the underground sheets are
/// generated (`ugsheets.build`): called runs solid, proposed ones dashed, every note calling for
/// buried work that no plan locates listed. The runs CADaddy proposes under the slab get an RFI
/// before the pour.
fn underground_step(s: &mut Session, job: &mut GuidedJob, pages: &[(String, cadaddy_doc::Drawing)], result: &mut Value) -> crate::Result<()> {
    let Ok((mut sp, true)) = super::bspec::current(s, "bim_build") else { return Ok(()) };
    let mut issued: Option<String> = None;
    let mut called: Vec<Value> = Vec::new();
    for (number, sub) in pages {
        let texts = cadaddy_mepfix::sheet_facts::texts(sub);
        // A view title: short, taller than the sheet's usual text, naming buried work and a plan.
        let mut hs: Vec<f64> = texts.iter().map(|t| t.height).filter(|h| h.is_finite() && *h > 0.0).collect();
        hs.sort_by(f64::total_cmp);
        let usual = hs.get(hs.len() / 2).copied().unwrap_or(0.0);
        if issued.is_none()
            && texts.iter().any(|t| {
                let up = t.value.to_ascii_uppercase();
                t.height > usual * 1.2
                    && up.len() <= 60
                    && super::keynotes::buried(&up)
                    && up.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "PLAN")
            })
        {
            issued = Some(number.clone());
        }
        for (n, text) in super::keynotes::buried_notes(&texts) {
            // A keynote the keynotes step built is located; the rest are listed.
            if let Some(n) = &n {
                let built = format!("{number} keynote {n} ");
                if sp.mep.runs.iter().any(|r| r.height_source.contains(&built)) {
                    continue;
                }
            }
            let text = match &n {
                Some(n) => format!("KEYNOTE {n}: {text}"),
                None => text,
            };
            if !called.iter().any(|c| c["text"] == text.as_str()) {
                called.push(json!({"sheet": number, "text": text}));
            }
        }
    }
    let below = |r: &&cadaddy_walk3d::MepRun| r.points.len() >= 2 && r.bottoms_ft.iter().any(|b| b.is_finite() && *b < 0.0);
    let buried: Vec<&cadaddy_walk3d::MepRun> = sp.mep.runs.iter().filter(below).collect();
    let proposed: BTreeSet<String> =
        buried.iter().filter(|r| r.height_source.eq_ignore_ascii_case("proposed")).map(|r| r.source_sheet.clone()).collect();
    let called_runs = buried.len() - buried.iter().filter(|r| r.height_source.eq_ignore_ascii_case("proposed")).count();
    if buried.is_empty() && called.is_empty() {
        return Ok(());
    }
    if let Some(sheet) = &issued {
        let line = format!("  underground: the set issues its plan ({sheet}); {} buried runs in the model", buried.len());
        report_progress(s, json!({"step": "underground"}), &line);
        s.echo(line);
        result["underground"] = json!({"issued": sheet, "runs": buried.len()});
        return Ok(());
    }
    // An RFI for what CADaddy proposes under the slab, before the pour.
    let mut rfis = Vec::new();
    if !proposed.is_empty() {
        let list = proposed.iter().take(20).cloned().collect::<Vec<_>>().join("; ");
        let issue = format!(
            "The documents issue no underground plan. CADaddy proposes these under the slab (shortest from gear that leaves its bottom, or rising to another level): {list}."
        );
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in issue.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        let id = format!("RFI-U{:08x}", h as u32);
        if !sp.rfis.iter().any(|r| r.id == id) && sp.rfis.len() < cadaddy_walk3d::rfi::MAX_RFIS {
            let kind = cadaddy_walk3d::RfiKind::PourHold;
            sp.rfis.push(cadaddy_walk3d::Rfi {
                id: id.clone(),
                kind: Some(kind),
                severity: kind.severity(),
                about: "underground plan".into(),
                issue: issue.clone(),
                assumed: "The generated underground sheet shows them dashed, as proposed, until answered.".into(),
                proposed: Some("Confirm each run under the slab (or route it overhead) before the pour".into()),
                ..cadaddy_walk3d::Rfi::default()
            });
            rfis.push(id);
        }
        s.doc_mut()?.building_spec = Some(sp.to_json());
    }
    let built = s.execute("ugsheets.build", &json!({"number": UNDERGROUND_SHEET, "called": called}));
    let _ = s.execute("layout.set", &json!({"name": "Model"}));
    let built = built?;
    let sheets: Vec<String> = built["sheets"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| Some(format!("{} {}", x["number"].as_str()?, x["level"].as_str().unwrap_or_default()))).collect())
        .unwrap_or_default();
    let line = format!(
        "  underground: no plan issued; generated {} ({called_runs} called runs solid, {} proposed feeders dashed, {} notes calling for buried work listed{})",
        if sheets.is_empty() { "nothing".to_string() } else { sheets.join(", ") },
        proposed.len(),
        called.len(),
        if rfis.is_empty() { String::new() } else { format!("; RFI {}", rfis.join(", ")) }
    );
    report_progress(s, json!({"step": "underground"}), &line);
    s.echo(line.clone());
    job.add_finding(
        StageName::BimBuild,
        line.trim(),
        "work the documents call for under the slab or in the ground needs a plan before the pour; none is issued",
        2,
        vec!["Have the engineer review the generated sheet; answer the RFI".into()],
        None,
        false,
    );
    result["underground"] = json!({"sheets": sheets, "called_runs": called_runs, "proposed": proposed, "called_notes": called, "rfis": rfis});
    Ok(())
}

/// The feeders' conduit ([`super::gearbuild::feeder_runs`]): proposed routes gear to gear, built.
fn feeder_step(s: &mut Session, job: &mut GuidedJob, result: &mut Value) -> crate::Result<()> {
    let Ok((mut sp, true)) = super::bspec::current(s, "bim_build") else { return Ok(()) };
    if sp.mep.feeders.is_empty() {
        return Ok(());
    }
    // A rerun: the feeders' earlier routes go (their 3D with them when the whole model rebuilds).
    sp.mep.runs.retain(|r| r.system != super::gearbuild::FEEDER_SYSTEM);
    let (runs, left) = super::gearbuild::feeder_runs(&sp);
    let routed: BTreeSet<String> = runs.iter().map(|r| r.source_sheet.clone()).collect();
    let feet: f64 = runs
        .iter()
        .map(|r| {
            let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
            let flat: f64 = r.points.windows(2).map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt() / upf).sum();
            flat + r.top_ft.map_or(0.0, |t| (t - r.bottoms_ft.first().copied().unwrap_or(t)).abs())
        })
        .sum();
    let line = format!(
        "  feeder routes (proposed): {} feeders routed, {} conduit runs, {feet:.0} ft of conduit; not routed {}: {}",
        routed.len(),
        runs.len(),
        left.len(),
        super::gearbuild::tally(&left).into_iter().take(6).collect::<Vec<_>>().join("; ")
    );
    let short: Vec<String> = super::gearbuild::typical_placed(&sp)
        .into_iter()
        .filter(|(_, placed, of)| *placed < *of as usize)
        .map(|(tag, placed, of)| format!("{tag}: {placed} of the {of} its schedule is typical of are placed"))
        .collect();
    let line = if short.is_empty() { line } else { format!("{line}; {}", short.join("; ")) };
    let n = runs.len();
    build_new_mep(s, sp, runs, &BTreeSet::new())?;
    report_progress(s, json!({"step": "feeder routes"}), &line);
    s.echo(line);
    if !left.is_empty() {
        job.add_finding(
            StageName::BimBuild,
            &format!("{} feeders not routed: {}", left.len(), super::gearbuild::tally(&left).into_iter().take(10).collect::<Vec<_>>().join("; ")),
            "a feeder is routed gear to gear when each end is one placed piece of gear",
            3,
            vec!["Route them from the riser diagram, or name the panel each one feeds".into()],
            None,
            false,
        );
    }
    result["feeder_routes"] = json!({"routed": routed.len(), "runs": n, "feet": feet, "not_routed": left});
    Ok(())
}

/// New MEP runs and equipment into the spec and built (the rest of the model kept), moved as the
/// build moved their level to stack it.
fn build_new_mep(
    s: &mut Session,
    mut sp: cadaddy_walk3d::BuildingSpec,
    runs: Vec<cadaddy_walk3d::MepRun>,
    new_equipment: &BTreeSet<String>,
) -> crate::Result<()> {
    use cadaddy_geom::Vec2;
    for name in runs.iter().map(|r| r.system.clone()).collect::<BTreeSet<_>>() {
        if !sp.mep.systems.iter().any(|y| y.name == name) {
            sp.mep.systems.push(cadaddy_walk3d::MepSystem {
                name: name.clone(),
                layer: name,
                kind: cadaddy_walk3d::MepKind::Conduit,
                size_in: [1.0, 0.0],
                bottom_ft: 9.0,
                show: true,
                duct_type: None,
                color_index: 0,
            });
        }
    }
    sp.mep.runs.extend(runs.iter().cloned());
    s.doc_mut()?.building_spec = Some(sp.to_json());
    let offsets: BTreeMap<String, Vec2> = {
        let d = s.doc()?;
        let mut m = BTreeMap::new();
        for e in d.model.iter().filter(|e| super::wallgen::made_as(e).as_deref() == Some("room")) {
            if let Some(l) = super::bspec::made_level(e) {
                m.entry(l.to_ascii_lowercase()).or_insert_with(|| super::bspec::stacked_by(e));
            }
        }
        m
    };
    let d = s.doc_mut()?;
    let before: BTreeSet<cadaddy_doc::Handle> = d.model.handles().into_iter().collect();
    if !new_equipment.is_empty() {
        let _ = super::mep::build_equipment(d, &sp, Some(new_equipment));
    }
    let mut part = sp.clone();
    part.mep.runs = runs;
    part.mep.terminals.clear();
    part.mep.equipment.clear();
    let _ = super::mep::build(d, &part);
    let made: Vec<cadaddy_doc::Handle> = d.model.handles().into_iter().filter(|h| !before.contains(h)).collect();
    for h in made {
        let level = d.model.iter().find(|e| e.handle == h).and_then(|e| super::bspec::made_level(e));
        let off = level.and_then(|l| offsets.get(&l.to_ascii_lowercase()).copied()).unwrap_or(Vec2::ZERO);
        if off.len() > 1e-9 {
            let m = cadaddy_geom::Mat3::translate(off);
            let _ = d.modify_entity(h, |e| {
                e.kind.transform(&m);
                e.common.xdata = super::bspec::with_stacked(&e.common.xdata, off).into();
            });
        }
    }
    s.touch();
    Ok(())
}

/// What an equipment question begins with (its tag follows).
const EQUIP_QUESTION: &str = "Equipment electrical data: ";

/// The loads the one-line feeds (not panels) against their submittals ([`super::equipsub`]):
/// each one's data from its latest submittal's text, else an agent's reading (a job answer), else
/// a question; the checks' failures RFIs and findings; the data kept in `mep.equipment_electrical`.
fn equipment_step(
    s: &mut Session,
    job: &mut GuidedJob,
    pages: &[(String, cadaddy_doc::Drawing)],
    types: &[super::gearbuild::FeederType],
    panels: &[String],
    result: &mut Value,
) -> crate::Result<()> {
    use super::equipsub;
    let type_tags: Vec<String> = types.iter().map(|t| t.tag.clone()).collect();
    let mut loads: Vec<equipsub::OnelineLoad> = Vec::new();
    for (_, d) in pages {
        for l in equipsub::oneline_loads(&cadaddy_mepfix::sheet_facts::texts(d), panels, &type_tags) {
            if !loads.iter().any(|x| x.tag.eq_ignore_ascii_case(&l.tag)) {
                loads.push(l);
            }
        }
    }
    let Ok((mut sp, true)) = super::bspec::current(s, "bim_build") else { return Ok(()) };
    let rule = super::coderules::rules().into_iter().find(|r| r.id == "E-EQUIP-ELEC").unwrap_or_default();
    // Every submittal PDF, in any folder under those given (revision folders: `Rev 1-1`).
    let mut files: Vec<String> = Vec::new();
    for root in &job.submittals {
        all_pdfs(std::path::Path::new(root.trim()), &mut files, 0);
    }
    // And the equipment a panel schedule feeds by tag (`GUESTROOM PTAC-1` on 20A/2P, #12) that has
    // a submittal: its breaker the drawings' protection, its branch wire the conductors.
    let mut branch_wire: BTreeMap<String, String> = BTreeMap::new();
    let has_submittal = |tag: &str| {
        equipsub::stem(tag).is_some_and(|st| {
            files.iter().any(|f| std::path::Path::new(f).file_name().is_some_and(|n| n.to_string_lossy().to_ascii_uppercase().contains(&st)))
        })
    };
    for p in sp.mep.panel_schedules.iter().take(MAX_EQUIPMENT_LOADS) {
        for c in &p.circuits {
            let up = c.description.to_ascii_uppercase();
            let Some(tag) = up.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-').find(|w| {
                w.split_once('-').is_some_and(|(a, n)| {
                    a.len() >= 2 && a.chars().all(|ch| ch.is_ascii_alphabetic()) && !n.is_empty() && n.chars().all(|ch| ch.is_ascii_digit())
                })
            }) else {
                continue;
            };
            let breaker = c.breaker.trim().trim_end_matches(['A', 'a']).trim();
            if breaker.parse::<f64>().is_err() || c.poles == 0 || loads.iter().any(|x| x.tag.eq_ignore_ascii_case(tag)) || !has_submittal(tag) {
                continue;
            }
            loads.push(equipsub::OnelineLoad { tag: tag.to_string(), feeder: String::new(), ocpd: format!("{breaker}A/{}P", c.poles) });
            branch_wire.insert(tag.to_string(), c.wire.trim().trim_start_matches('#').to_string());
        }
    }
    if loads.is_empty() {
        return Ok(());
    }
    let mut rows: Vec<cadaddy_walk3d::EquipmentElectrical> = Vec::new();
    let mut asked = 0usize;
    let mut conflicts_all: Vec<(String, equipsub::Conflict)> = Vec::new();
    for l in loads.iter().take(MAX_EQUIPMENT_LOADS) {
        let (description, source_panel) = equipsub::schedule_row(&sp, &l.tag).unwrap_or_default();
        let mut e = cadaddy_walk3d::EquipmentElectrical {
            tag: l.tag.clone(),
            description,
            source_panel,
            drawing_ocpd: l.ocpd.clone(),
            feeder: l.feeder.clone(),
            ..Default::default()
        };
        // Its submittals, the latest revision first.
        let Some(st) = equipsub::stem(&l.tag) else { continue };
        let mut mine: Vec<&String> = files
            .iter()
            .filter(|f| std::path::Path::new(f).file_name().is_some_and(|n| n.to_string_lossy().to_ascii_uppercase().contains(&st)))
            .collect();
        mine.sort_by(|a, b| equipsub::revision(b).cmp(&equipsub::revision(a)).then(a.cmp(b)));
        let mut scanned: Vec<String> = Vec::new();
        for f in mine.iter().take(MAX_GEAR_SUBMITTALS) {
            let name = std::path::Path::new(f.as_str()).file_name().map_or_else(|| f.to_string(), |n| n.to_string_lossy().to_string());
            let Ok(text) = crate::readfile::read(f.as_str())
                .map_err(|e| e.to_string())
                .and_then(|b| super::spec_import::pdf_text(&b, f).map_err(|e| e.to_string()))
            else {
                continue;
            };
            if e.status.is_empty() {
                e.status = equipsub::review_status(&text);
            }
            // A submittal of several models: the tag's own (its item line), else the text's data.
            let got = equipsub::submittal_for_model(&text, &l.tag).filter(equipsub::has_data).unwrap_or_else(|| equipsub::text_electrical(&text));
            if equipsub::has_data(&got) {
                equipsub::merge(&mut e, &got);
                e.source = format!("submittal {name}");
                break;
            }
            scanned.push(name);
        }
        if !equipsub::has_data(&e) {
            let key = format!("equip:{}", l.tag.to_ascii_uppercase());
            match job.rules.iter().find(|r| r.key == key).and_then(|r| serde_json::from_str::<Value>(&r.value).ok()) {
                Some(a) => equipsub::merge(&mut e, &equipsub::from_answer(&a)),
                None if !scanned.is_empty() => {
                    equipment_question(job, &l.tag, &e, &scanned);
                    asked += 1;
                }
                None => {}
            }
        }
        // The checks: the drawings' feeder conductors and the source's fault current.
        let ft = types.iter().find(|t| t.tag.eq_ignore_ascii_case(&l.feeder));
        let afc = sp
            .mep
            .panel_schedules
            .iter()
            .find(|p| p.panel.eq_ignore_ascii_case(&e.source_panel))
            .and_then(|p| p.header.get("afc value"))
            .and_then(|v| v.replace([',', 'A', 'a'], "").trim().parse::<f64>().ok());
        // A branch circuit's wire (copper) where a panel schedule feeds it.
        let wire = branch_wire.get(&l.tag).filter(|w| !w.is_empty());
        let conductors = ft.map(|t| (t.size.as_str(), t.material.as_str())).or(wire.map(|w| (w.as_str(), "cu")));
        for c in equipsub::check(&e, conductors, afc, &rule) {
            conflicts_all.push((l.tag.clone(), c));
        }
        rows.push(e);
    }
    // Each failure an RFI (once) and a finding.
    let mut rfis = Vec::new();
    for (tag, c) in &conflicts_all {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in format!("{tag}|{}|{}", c.check, c.message).bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        let id = format!("RFI-E{:08x}", h as u32);
        if !sp.rfis.iter().any(|r| r.id == id) && sp.rfis.len() < cadaddy_walk3d::rfi::MAX_RFIS {
            let kind = cadaddy_walk3d::RfiKind::EquipmentElectrical;
            sp.rfis.push(cadaddy_walk3d::Rfi {
                id: id.clone(),
                kind: Some(kind),
                severity: kind.severity(),
                about: format!("{tag} ({})", c.check),
                issue: format!("Submittal against the drawings: {}", c.message),
                assumed: "The model follows the submittal; the drawings' value is kept beside it until answered.".into(),
                proposed: Some(c.fix.clone()),
                ..cadaddy_walk3d::Rfi::default()
            });
            rfis.push(id);
        }
        job.add_finding(
            StageName::BimBuild,
            &format!("Equipment {tag} ({}): {}", c.check, c.message),
            "its approved submittal differs from the drawings",
            2,
            vec![c.fix.clone()],
            None,
            false,
        );
    }
    let line = format!(
        "  equipment connections: {} on the one-line and the panel schedules ({}); submittal data for {}; asked {asked}; conflicts {}: {}",
        rows.len(),
        rows.iter().map(|r| format!("{} {} {}", r.tag, r.drawing_ocpd, r.feeder)).collect::<Vec<_>>().join(", "),
        rows.iter().filter(|r| super::equipsub::has_data(r)).count(),
        conflicts_all.len(),
        conflicts_all.iter().map(|(t, c)| format!("{t} {}", c.check)).collect::<Vec<_>>().join(", ")
    );
    sp.mep.equipment_electrical = rows;
    s.doc_mut()?.building_spec = Some(sp.to_json());
    report_progress(s, json!({"step": "equipment connections"}), &line);
    s.echo(line);
    result["equipment_electrical"] = json!({"conflicts": conflicts_all.len(), "rfis": rfis, "asked": asked});
    Ok(())
}

/// Equipment connections looked at, at most.
const MAX_EQUIPMENT_LOADS: usize = 200;

/// The PDFs under `p` (a file, or a folder searched down a few levels), at most a few thousand.
fn all_pdfs(p: &std::path::Path, out: &mut Vec<String>, depth: usize) {
    if out.len() >= 5_000 || depth > 6 {
        return;
    }
    if p.is_file() {
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
            out.push(p.to_string_lossy().to_string());
        }
        return;
    }
    let Ok(rd) = std::fs::read_dir(p) else { return };
    let mut entries: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for e in entries {
        all_pdfs(&e, out, depth + 1);
    }
}

/// A question for the agent: the electrical data of a load whose submittal is scanned (no text),
/// with the files to read; answered, the build checks the drawings against it. Its default keeps
/// the drawings' values, so a job on its own goes on.
fn equipment_question(job: &mut GuidedJob, tag: &str, e: &cadaddy_walk3d::EquipmentElectrical, files: &[String]) {
    let text = format!("{EQUIP_QUESTION}{tag}");
    if job.stages.iter().flat_map(|st| &st.questions).any(|q| q.question.starts_with(&text)) {
        return;
    }
    let qid = job.next_qid(StageName::BimBuild);
    let q = Question {
        id: qid,
        stage: StageName::BimBuild,
        question: format!(
            "{text}: its submittal is scanned (no text). Read its electrical data from {} (the latest approved revision; the data sheet: volts/phase, nameplate or full-load amps, minimum circuit amps, the most protection it allows and fuse class, acceleration amps, HP/kW, the controller's SCCR) and answer {{volts, phase, nameplate_a, fla, mca, max_ocpd_a, fuse_class, accel_a, hp, kw, sccr_a, source: \"<file> p.<n>\"}}. The drawings give it {} on feeder {} from {}.",
            files.join("; "),
            if e.drawing_ocpd.is_empty() { "no protection read" } else { &e.drawing_ocpd },
            if e.feeder.is_empty() { "?" } else { &e.feeder },
            if e.source_panel.is_empty() { "?" } else { &e.source_panel }
        ),
        why: "An equipment connection follows its approved submittal: protection, conductors and the controller's short-circuit rating are checked against it".into(),
        choices: vec!["Keep the drawings' values".into()],
        evidence: Some(QuestionEvidence {
            crop_png_path: None,
            sheet: None,
            at: None,
            values: Some(json!({"equipment": tag, "files": files, "drawings": {"ocpd": e.drawing_ocpd, "feeder": e.feeder, "source": e.source_panel}})),
            handles: Vec::new(),
            tiled_crops: None,
            leftovers: Vec::new(),
        }),
        default: Some("Keep the drawings' values".into()),
        answer_schema: Some(json!({"type": "object", "properties": {
            "volts": {"type": "number"}, "phase": {"type": "number"}, "nameplate_a": {"type": "number"}, "fla": {"type": "number"}, "mca": {"type": "number"},
            "max_ocpd_a": {"type": "number"}, "fuse_class": {"type": "string"}, "accel_a": {"type": "number"}, "hp": {"type": "number"}, "kw": {"type": "number"},
            "sccr_a": {"type": "number"}, "source": {"type": "string"}}})),
        answer: None,
        answered_by: None,
        answered_at: None,
        kind: QuestionKind::Completeness,
    };
    job.add_question_to_worklist(&q);
    if let Some(st) = job.stage_mut(StageName::BimBuild) {
        st.questions.push(q);
    }
}

/// Gear submittals read, at most.
const MAX_GEAR_SUBMITTALS: usize = 40;

/// What a gear question begins with (its tag follows), for its answer to be learned.
const GEAR_QUESTION: &str = "Gear size: ";

/// A question for the agent: the size (and conduit entry) of gear the documents give but the job
/// couldn't read, with where to look; answered, the build lays the gear out with it. Its default
/// leaves the gear unplaced, so a job running on its own goes on.
fn gear_question(job: &mut GuidedJob, tag: &str, pages: Vec<String>, files: &[String]) {
    let text = format!("{GEAR_QUESTION}{tag}");
    let asked = job.stages.iter().flat_map(|st| &st.questions).any(|q| q.question.starts_with(&text));
    if asked {
        return;
    }
    let qid = job.next_qid(StageName::BimBuild);
    let look = if pages.is_empty() { files.join("; ") } else { pages.join("; ") };
    let q = Question {
        id: qid,
        stage: StageName::BimBuild,
        question: format!(
            "{text}: its box size isn't in a table the job reads. Read it from {look} (its elevation or dimension drawing) and answer {{height_in, width_in, depth_in, feed: \"top\"|\"bottom\", top_entry_in?: [w, d], bottom_entry_in?: [w, d]}}"
        ),
        why: "The electrical room is laid out and each piece of gear checked (entry plates, lugs, working space) from its size; without it the gear stays out of the model".into(),
        choices: vec!["Leave unplaced".into()],
        evidence: Some(QuestionEvidence {
            crop_png_path: None,
            sheet: None,
            at: None,
            values: Some(json!({"panel": tag, "pages": pages, "files": files})),
            handles: Vec::new(),
            tiled_crops: None,
            leftovers: Vec::new(),
        }),
        default: Some("Leave unplaced".into()),
        answer_schema: Some(json!({"type": "object", "properties": {
            "height_in": {"type": "number"}, "width_in": {"type": "number"}, "depth_in": {"type": "number"},
            "feed": {"type": "string"}, "top_entry_in": {"type": "array"}, "bottom_entry_in": {"type": "array"}}})),
        answer: None,
        answered_by: None,
        answered_at: None,
        kind: QuestionKind::Completeness,
    };
    job.add_question_to_worklist(&q);
    if let Some(st) = job.stage_mut(StageName::BimBuild) {
        st.questions.push(q);
    }
}

/// The electrical sheets' pages: the job's drawing's E sheets and those it didn't import.
fn electrical_pages(d: &cadaddy_doc::Drawing, job: &GuidedJob) -> Vec<(String, cadaddy_doc::Drawing)> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut pages: Vec<(String, cadaddy_doc::Drawing)> = job_sheet_refs(job)
        .iter()
        .filter(|r| r.discipline.eq_ignore_ascii_case("E") && seen.insert(r.number.split('/').next().unwrap_or("").to_string()))
        .take(MAX_LEGEND_SHEETS)
        .map(|sh| {
            (
                sh.number.split('/').next().unwrap_or("").to_string(),
                super::printmep::sheet_drawing(d, &super::printmep::SheetRef { view: None, ..sh.clone() }),
            )
        })
        .collect();
    pages.extend(non_plan_pages(job, "E").into_iter().filter(|(n, _)| !seen.contains(n)));
    pages
}

/// The panel schedules on the electrical sheets, each panel once (a panel named twice: the
/// schedule with more breakers read).
fn panel_schedules(pages: &[(String, cadaddy_doc::Drawing)]) -> Vec<cadaddy_walk3d::PanelSchedule> {
    let mut out: Vec<cadaddy_walk3d::PanelSchedule> = Vec::new();
    for (number, sub) in pages {
        for p in super::panelsched::read(&cadaddy_mepfix::sheet_facts::texts(sub), number) {
            // A panel named twice: the schedule with more breakers read.
            let read = |x: &cadaddy_walk3d::PanelSchedule| x.circuits.iter().filter(|c| !c.breaker.is_empty()).count();
            match out.iter_mut().find(|o| o.panel.eq_ignore_ascii_case(&p.panel)) {
                Some(o) if read(&p) > read(o) => *o = p,
                Some(_) => {}
                None => out.push(p),
            }
        }
    }
    out
}

/// One sheet's legend rows into `out` ([`legend_symbols`]).
fn legend_rows(sub: &cadaddy_doc::Drawing, number: &str, out: &mut BTreeMap<Shape, (String, String)>) {
    let texts = cadaddy_mepfix::sheet_facts::texts(sub);
    let heads: Vec<&cadaddy_mepfix::sheet_facts::TextItem> =
        texts.iter().filter(|t| t.value.trim().eq_ignore_ascii_case("SYMBOL")).take(20).collect();
    if heads.is_empty() {
        return;
    }
    let symbols: Vec<(String, cadaddy_geom::Vec2)> = sub
        .model
        .iter()
        .filter_map(|e| {
            let cadaddy_doc::EntityKind::Insert(ins) = &e.kind else { return None };
            let b = cadaddy_doc::entity_bounds(sub, e, 0);
            (!b.is_empty()).then(|| (ins.block.clone(), b.center()))
        })
        .take(MAX_LEGEND_SYMBOLS)
        .collect();
    for head in heads {
        let h = head.height.max(1e-6);
        // Its description column: the DESCRIPTION header beside it, on its row.
        let Some(desc) = texts
            .iter()
            .filter(|t| t.value.trim().to_ascii_uppercase().starts_with("DESCRIPTION") && (t.at.y - head.at.y).abs() <= h && t.at.x > head.at.x)
            .min_by(|a, b| (a.at.x - head.at.x).total_cmp(&(b.at.x - head.at.x)))
        else {
            continue;
        };
        // The table: down to the next SYMBOL header under this one (or a long way).
        let below = texts
            .iter()
            .filter(|t| t.value.trim().eq_ignore_ascii_case("SYMBOL") && (t.at.x - head.at.x).abs() <= 2.0 * h && t.at.y < head.at.y - h)
            .map(|t| t.at.y)
            .fold(head.at.y - LEGEND_ROWS_H * h, f64::max);
        let rows: Vec<&cadaddy_mepfix::sheet_facts::TextItem> =
            texts.iter().filter(|t| (t.at.x - desc.at.x).abs() <= 1.5 * h && t.at.y < head.at.y - 0.5 * h && t.at.y > below).collect();
        for (block, c) in symbols.iter().filter(|(_, c)| c.x > head.at.x - 3.0 * h && c.x < desc.at.x && c.y < head.at.y && c.y > below) {
            let Some(shape) = symbol_shape(sub, block) else { continue };
            if out.contains_key(&shape) {
                continue;
            }
            // Its row: the description line nearest its middle.
            let Some(row) = rows
                .iter()
                .filter(|t| (t.at.y - c.y).abs() <= 2.0 * t.height.max(h))
                .min_by(|a, b| (a.at.y - c.y).abs().total_cmp(&(b.at.y - c.y).abs()))
            else {
                continue;
            };
            out.insert(shape, (row.value.trim().to_string(), number.to_string()));
        }
        // A symbol drawn once (the import blocks only what repeats on a page): the linework in
        // the row's symbol cell, left of its description, on its first line.
        let x0 = head.at.x - 3.0 * h;
        let x1 = desc.at.x - 0.5 * h;
        let drawn: Vec<(cadaddy_geom::Bounds2, &cadaddy_doc::EntityKind)> = sub
            .model
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    cadaddy_doc::EntityKind::Line(_)
                        | cadaddy_doc::EntityKind::Circle(_)
                        | cadaddy_doc::EntityKind::Arc(_)
                        | cadaddy_doc::EntityKind::LwPolyline(_)
                )
            })
            .filter_map(|e| {
                let b = cadaddy_doc::entity_bounds(sub, e, 0);
                (!b.is_empty() && b.min.x >= x0 && b.max.x <= x1 && b.min.y > below && b.max.y < head.at.y).then_some((b, &e.kind))
            })
            .take(MAX_LEGEND_SYMBOLS)
            .collect();
        for row in &rows {
            let th = row.height.max(1e-6);
            let (y0, y1) = (row.at.y - 1.0 * th, row.at.y + 1.8 * th);
            let cell: Vec<&cadaddy_doc::EntityKind> = drawn.iter().filter(|(b, _)| b.min.y >= y0 && b.max.y <= y1).map(|(_, k)| *k).collect();
            if cell.len() < 2 {
                continue;
            }
            let Some(shape) = shape_of(cell.into_iter()) else { continue };
            out.entry(shape).or_insert_with(|| (row.value.trim().to_string(), number.to_string()));
        }
    }
}

/// A legend table's rows reach at most this many header heights down.
const LEGEND_ROWS_H: f64 = 150.0;
/// Symbols looked at on one legend sheet, at most.
const MAX_LEGEND_SYMBOLS: usize = 50_000;

/// Each device whose symbol is a legend row's: that row's kind and IFC class, the row its source.
/// How many took one.
fn kinds_from_legend(d: &cadaddy_doc::Drawing, legend: &BTreeMap<Shape, (String, String)>, devices: &mut [super::readers::ReadDevice]) -> usize {
    let mut n = 0usize;
    let mut shapes: BTreeMap<String, Option<Shape>> = BTreeMap::new();
    for dv in devices.iter_mut() {
        if !(dv.kind.is_empty() || dv.kind == "device") {
            continue;
        }
        let Some(cadaddy_doc::EntityKind::Insert(ins)) = cadaddy_doc::Handle::parse_hex(&dv.handle).and_then(|h| d.model.get(h)).map(|e| &e.kind)
        else {
            continue;
        };
        let shape = shapes.entry(ins.block.clone()).or_insert_with(|| symbol_shape(d, &ins.block));
        let Some((desc, sheet)) = shape.as_ref().and_then(|sh| legend.get(sh)) else { continue };
        let Some((kind, ifc)) = legend_kind(desc) else { continue };
        dv.kind = kind.into();
        dv.ifc_class = ifc.into();
        dv.kind_source = format!("{sheet} legend: {desc}");
        n += 1;
    }
    n
}

/// What [`stamp_typical_runs`] made: the stamped runs, which of the runs it was given lie in a
/// typical room's plan (each `true` went into the rooms), and the count per sheet and view.
#[derive(Default)]
struct StampedRuns {
    runs: Vec<cadaddy_walk3d::MepRun>,
    taken: Vec<bool>,
    report: BTreeMap<String, usize>,
}

/// The pipes and ducts of the typical-room plans (ENLARGED GUESTROOM - SANITARY's DOUBLE QUEEN
/// view) that no registration placed (`lost`): each, when all of it lies in a view whose title
/// names a room type, into every room of that type at the same place in the room, as their devices
/// are ([`stamp_typical_rooms`]). A room the floor's own plans already run that system through
/// (`placed`, in the model's coordinates) is left alone: no second count.
fn stamp_typical_runs(d: &cadaddy_doc::Drawing, job: &GuidedJob, placed: &[cadaddy_walk3d::MepRun], lost: &[cadaddy_walk3d::MepRun]) -> StampedRuns {
    let mut out = StampedRuns { taken: vec![false; lost.len()], ..StampedRuns::default() };
    let rooms = job_rooms(job);
    let (types, vocabulary) = room_types(&rooms);
    if types.is_empty() {
        return out;
    }
    let refs = job_sheet_refs(job);
    let floor_hinges = plan_hinges(d);
    let k_in = super::perceive::units_per_inch(d);
    let near = ROOM_SWING_FT * 12.0 * k_in;
    let bbox = |o: &[cadaddy_geom::Vec2]| {
        o.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
            [b[0].min(q.x), b[1].min(q.y), b[2].max(q.x), b[3].max(q.y)]
        })
    };
    let room_boxes: Vec<[f64; 4]> = rooms.iter().map(|(_, _, o, _)| bbox(o)).collect();
    let floor_lines = FloorLines::new(d, &room_boxes, ROOM_FIT_MARGIN_FT * 12.0 * k_in, 12.0 * k_in);
    let inside = |v: &[f64; 4], p: &[f64; 2]| p[0] >= v[0] && p[0] <= v[2] && p[1] >= v[1] && p[1] <= v[3];
    let area = |r: &[f64; 4]| (r[2] - r[0]) * (r[3] - r[1]);
    // The systems the floor's plans run through each room already.
    let run_through = |level: &str, outline: &[cadaddy_geom::Vec2], system: &str| {
        placed.iter().any(|q| {
            q.level == level
                && q.system == system
                && q.points.iter().any(|p| cadaddy_geom::point_in_polygon(outline, cadaddy_geom::Vec2::new(p[0], p[1])))
        })
    };
    let sheets: BTreeSet<String> = lost.iter().map(|r| sheet_key(&r.source_sheet)).collect();
    for sheet in sheets.iter().take(MAX_STAMP_SHEETS) {
        let Some(sh) = refs.iter().find(|r| sheet_key(&r.number) == *sheet && r.view.is_none()) else { continue };
        let sub = super::printmep::sheet_drawing(d, sh);
        let texts = cadaddy_mepfix::sheet_facts::texts(&sub);
        let (views, _) = room_views(&sub, &texts, &types, &vocabulary, &rooms);
        if views.is_empty() {
            continue;
        }
        let sheet_hinges = plan_hinges(&sub);
        let sheet_objects = super::bspec::plan_objects(&sub);
        let mut view_lines: BTreeMap<[u64; 4], Vec<cadaddy_geom::Vec2>> = BTreeMap::new();
        let mut turns: BTreeMap<([u64; 4], usize), RoomFit> = BTreeMap::new();
        for (i, r) in lost.iter().enumerate().filter(|(_, r)| sheet_key(&r.source_sheet) == *sheet) {
            if r.points.is_empty() {
                continue;
            }
            // The smallest typical view holding all of it.
            let Some((title, view, words)) =
                views.iter().filter(|(_, v, _)| r.points.iter().all(|p| inside(v, p))).min_by(|a, b| area(&a.1).total_cmp(&area(&b.1)))
            else {
                continue;
            };
            if let Some(t) = out.taken.get_mut(i) {
                *t = true;
            }
            for (ri, (_, level, outline, _)) in rooms.iter().enumerate().filter(|(_, (n, _, _, _))| type_words(n, None) == *words) {
                if out.runs.len() >= MAX_STAMPED {
                    break;
                }
                if run_through(level, outline, &r.system) {
                    continue;
                }
                let b = bbox(outline);
                let turn = *turns.entry((view.map(f64::to_bits), ri)).or_insert_with(|| {
                    let lines = view_lines.entry(view.map(f64::to_bits)).or_insert_with(|| {
                        let mine: Vec<&std::sync::Arc<cadaddy_doc::Entity>> = sheet_objects
                            .iter()
                            .filter(|e| {
                                super::bspec::centre(&sub, e).is_some_and(|c| c.x >= view[0] && c.x <= view[2] && c.y >= view[1] && c.y <= view[3])
                            })
                            .copied()
                            .collect();
                        super::grids::line_samples(&mine, 12.0 * k_in)
                    });
                    rigid_room_fit(lines, *view, b, &floor_lines, ROOM_FIT_MARGIN_FT * 12.0 * k_in, 6.0 * k_in).map(|f| f.0).unwrap_or(RoomFit {
                        k: room_turn(*view, b, &hinges_in(&sheet_hinges, *view, 0.0), &hinges_in(&floor_hinges, b, near)),
                        from: cadaddy_geom::Vec2::new(0.0, 0.0),
                        to: cadaddy_geom::Vec2::new(0.0, 0.0),
                        rigid: false,
                    })
                });
                let Some(points) = r.points.iter().map(|p| place_in_room(*p, *view, b, turn)).collect::<Option<Vec<[f64; 2]>>>() else { continue };
                let mut copy = r.clone();
                copy.points = points;
                copy.level.clone_from(level);
                copy.id = String::new();
                copy.objects = Vec::new();
                out.runs.push(copy);
                *out.report.entry(format!("{sheet} {title}")).or_default() += 1;
            }
        }
    }
    out
}

/// A room of the rooms stage with its outline: name, level, outline, number.
type JobRoom = (String, String, Vec<cadaddy_geom::Vec2>, String);

/// The rooms stage's rooms with outlines (not the ones traced from other trades' tags).
fn job_rooms(job: &GuidedJob) -> Vec<JobRoom> {
    let mut rooms: Vec<JobRoom> = job
        .stage(StageName::Rooms)
        .and_then(|st| st.result.as_ref())
        .and_then(|r| r.get("rooms"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| r.get("other_trade").and_then(Value::as_bool) != Some(true))
        .filter_map(|r| {
            let name = r.get("name").and_then(Value::as_str)?.trim().to_string();
            let level = r.get("level").and_then(Value::as_str).unwrap_or("").to_string();
            let outline: Vec<cadaddy_geom::Vec2> = r
                .get("outline")?
                .as_array()?
                .iter()
                .filter_map(|q| Some(cadaddy_geom::Vec2::new(q.get(0)?.as_f64()?, q.get(1)?.as_f64()?)))
                .collect();
            let number = r.get("number").and_then(Value::as_str).unwrap_or("").trim().to_string();
            (outline.len() >= 3 && !name.is_empty()).then_some((name, level, outline, number))
        })
        .collect();
    // A trace that leaked through a wall into its neighbour, cut back.
    trim_swallowed(&mut rooms);
    rooms
}

/// How far (sheet units in feet times this) a room's box may stick out of one it is inside.
const SWALLOW_TOL_FT: f64 = 1.0;

/// A room whose traced outline leaked through a wall into its neighbour (unit 328's trace took
/// 326 in as well): where a smaller room of its level lies inside its box, spanning the box's
/// whole depth at one end, that end is cut off (its outline clipped to what is left).
fn trim_swallowed(rooms: &mut [JobRoom]) {
    let bbox = |o: &[cadaddy_geom::Vec2]| {
        o.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
            [b[0].min(q.x), b[1].min(q.y), b[2].max(q.x), b[3].max(q.y)]
        })
    };
    let boxes: Vec<[f64; 4]> = rooms.iter().map(|(_, _, o, _)| bbox(o)).collect();
    // Sheet units per foot: the rooms' outlines are in the plan's units (inches on these sets).
    let tol = SWALLOW_TOL_FT * 12.0;
    let area = |b: &[f64; 4]| (b[2] - b[0]) * (b[3] - b[1]);
    for i in 0..rooms.len().min(MAX_STAMPED) {
        let Some(mut a) = boxes.get(i).copied() else { continue };
        let level = rooms.get(i).map(|r| r.1.clone()).unwrap_or_default();
        let mut cut = false;
        for (j, b) in boxes.iter().enumerate() {
            if j == i || rooms.get(j).is_none_or(|r| r.1 != level) || area(b) >= area(&a) * 0.9 {
                continue;
            }
            let inside = b[0] >= a[0] - tol && b[2] <= a[2] + tol && b[1] >= a[1] - tol && b[3] <= a[3] + tol;
            if !inside {
                continue;
            }
            let full_y = b[1] <= a[1] + tol && b[3] >= a[3] - tol;
            let full_x = b[0] <= a[0] + tol && b[2] >= a[2] - tol;
            if full_y && b[0] <= a[0] + tol && b[2] < a[2] - tol {
                a[0] = b[2];
                cut = true;
            } else if full_y && b[2] >= a[2] - tol && b[0] > a[0] + tol {
                a[2] = b[0];
                cut = true;
            } else if full_x && b[1] <= a[1] + tol && b[3] < a[3] - tol {
                a[1] = b[3];
                cut = true;
            } else if full_x && b[3] >= a[3] - tol && b[1] > a[1] + tol {
                a[3] = b[1];
                cut = true;
            }
        }
        if cut && let Some(r) = rooms.get_mut(i) {
            let clipped = clip_to_box(&r.2, a);
            if clipped.len() >= 3 {
                r.2 = clipped;
            }
        }
    }
}

/// `poly` clipped to the box `b` (`[x0, y0, x1, y1]`).
fn clip_to_box(poly: &[cadaddy_geom::Vec2], b: [f64; 4]) -> Vec<cadaddy_geom::Vec2> {
    let mut out: Vec<cadaddy_geom::Vec2> = poly.to_vec();
    // Each side: inside test and the crossing on the side's line.
    for side in 0..4 {
        let inside = |p: &cadaddy_geom::Vec2| match side {
            0 => p.x >= b[0],
            1 => p.x <= b[2],
            2 => p.y >= b[1],
            _ => p.y <= b[3],
        };
        let cross = |p: cadaddy_geom::Vec2, q: cadaddy_geom::Vec2| {
            let t = match side {
                0 => (b[0] - p.x) / (q.x - p.x),
                1 => (b[2] - p.x) / (q.x - p.x),
                2 => (b[1] - p.y) / (q.y - p.y),
                _ => (b[3] - p.y) / (q.y - p.y),
            };
            p + (q - p) * if t.is_finite() { t } else { 0.0 }
        };
        let input = std::mem::take(&mut out);
        let n = input.len();
        for k in 0..n {
            let (Some(&p), Some(&q)) = (input.get(k), input.get((k + 1) % n)) else { continue };
            match (inside(&p), inside(&q)) {
                (true, true) => out.push(q),
                (true, false) => out.push(cross(p, q)),
                (false, true) => {
                    out.push(cross(p, q));
                    out.push(q);
                }
                (false, false) => {}
            }
        }
    }
    out
}

/// The room types (the words of a name several rooms carry) and every word they use.
fn room_types(rooms: &[JobRoom]) -> (BTreeSet<BTreeSet<String>>, BTreeSet<String>) {
    let mut count: BTreeMap<BTreeSet<String>, usize> = BTreeMap::new();
    for (name, _, _, _) in rooms {
        *count.entry(type_words(name, None)).or_default() += 1;
    }
    let types: BTreeSet<BTreeSet<String>> = count.into_iter().filter(|(k, n)| *n > 1 && !k.is_empty()).map(|(k, _)| k).collect();
    let vocabulary: BTreeSet<String> = types.iter().flatten().cloned().collect();
    (types, vocabulary)
}

/// A sheet number without its view (`E-6/2` is on `E-6`), upper case.
fn sheet_key(n: &str) -> String {
    n.split('/').next().unwrap_or(n).trim().to_ascii_uppercase()
}

/// The plan views on a sheet that show rooms, not a floor: the typical-room views (title, region,
/// the room type's words) and the views of one room (title, region, the room's index in `rooms`).
#[allow(clippy::type_complexity)]
fn room_views(
    sub: &cadaddy_doc::Drawing,
    texts: &[cadaddy_mepfix::sheet_facts::TextItem],
    types: &BTreeSet<BTreeSet<String>>,
    vocabulary: &BTreeSet<String>,
    rooms: &[JobRoom],
) -> (Vec<(String, [f64; 4], BTreeSet<String>)>, Vec<(String, [f64; 4], usize)>) {
    let mut views = Vec::new();
    let mut one_room = Vec::new();
    for v in cadaddy_mepfix::sheet_facts::view_titles(sub, texts) {
        // A plan of the room, not an elevation of one of its walls or a section.
        let upper = v.title.to_ascii_uppercase();
        let plan = !["WALL", "ELEVATION", "SECTION", "DETAIL", "RCP", "CEILING"].iter().any(|w| upper.contains(w));
        let Some(region) = v.region.filter(|_| plan) else { continue };
        let words = type_words(&v.title, Some(vocabulary));
        if types.contains(&words) {
            views.push((v.title, region, words));
        } else if let Some(i) = room_of_view(&upper, rooms) {
            one_room.push((v.title, region, i));
        }
    }
    (views, one_room)
}

/// Whether a run is a circuit's estimated route that starts in one of its sheet's views of rooms
/// (`views`: by [`sheet_key`]).
fn in_room_view(r: &cadaddy_walk3d::MepRun, views: &BTreeMap<String, Vec<[f64; 4]>>) -> bool {
    r.system == super::printcond::CIRCUIT_SYSTEM
        && views
            .get(&sheet_key(&r.source_sheet))
            .is_some_and(|vs| r.points.first().is_some_and(|p| vs.iter().any(|v| p[0] >= v[0] && p[0] <= v[2] && p[1] >= v[1] && p[1] <= v[3])))
}

/// Where `sheets` (by [`sheet_key`]) show rooms rather than a floor (their typical-room and
/// one-room plans), in the job's drawing.
fn room_view_regions(d: &cadaddy_doc::Drawing, job: &GuidedJob, sheets: &BTreeSet<String>) -> BTreeMap<String, Vec<[f64; 4]>> {
    let rooms = job_rooms(job);
    let (types, vocabulary) = room_types(&rooms);
    let refs = job_sheet_refs(job);
    let mut out = BTreeMap::new();
    for sheet in sheets.iter().take(MAX_STAMP_SHEETS) {
        let Some(sh) = refs.iter().find(|r| sheet_key(&r.number) == *sheet && r.view.is_none()) else { continue };
        let sub = super::printmep::sheet_drawing(d, sh);
        let texts = cadaddy_mepfix::sheet_facts::texts(&sub);
        let (views, one_room) = room_views(&sub, &texts, &types, &vocabulary, &rooms);
        let regions: Vec<[f64; 4]> = views.iter().map(|v| v.1).chain(one_room.iter().map(|v| v.1)).collect();
        if !regions.is_empty() {
            out.insert(sheet.clone(), regions);
        }
    }
    out
}

/// What stamping the typical and one-room plans gave: the devices, how many by view, the rooms
/// a one-room plan is the authority in (level, outline), each stamped room's panels, the level
/// and outline of each room with panels, and the guest rooms stamped.
struct StampedRooms {
    devices: Vec<super::readers::ReadDevice>,
    report: BTreeMap<String, usize>,
    authority: Vec<(String, Vec<cadaddy_geom::Vec2>)>,
    panels: BTreeMap<String, Vec<(String, [f64; 2])>>,
    rooms: BTreeMap<String, (String, Vec<cadaddy_geom::Vec2>, bool)>,
    /// The rooms stamped from a typical plan of a room to sleep in (a guest room): level, outline.
    sleeping: Vec<(String, Vec<cadaddy_geom::Vec2>)>,
    /// Each stamped unit's whole footprint (its plan's linework laid into the room: the bath and
    /// entry its label's room leaves out): level, room, footprint, the room's own outline.
    footprints: Vec<Footprint>,
    /// The floor plans' devices (by their index in `placed`) where a typical plan's copy landed:
    /// the same devices, left out.
    duplicates: BTreeSet<usize>,
}

/// The electrical legends' designations (`3=3-WAY`, `OS2=WALL MOUNTED OCCUPANCY SENSOR …`) on the
/// job's electrical sheets (each imported sheet's objects: its legend and note sheets too).
fn legend_designations(d: &cadaddy_doc::Drawing, job: &GuidedJob) -> BTreeMap<String, String> {
    let spans: Vec<(u64, u64)> = job
        .sheet_spans
        .iter()
        .filter(|sp| cadaddy_mepfix::sheet_facts::discipline_of(sp.sheet.split('/').next().unwrap_or(&sp.sheet)).eq_ignore_ascii_case("E"))
        .map(|sp| (sp.lo, sp.hi))
        .collect();
    if spans.is_empty() {
        return BTreeMap::new();
    }
    let texts: Vec<cadaddy_mepfix::sheet_facts::TextItem> = d
        .model
        .iter()
        .filter(|e| spans.iter().any(|(lo, hi)| (*lo..*hi).contains(&e.handle.0)))
        .filter_map(|e| match &e.kind {
            cadaddy_doc::EntityKind::Text(t) if t.value.contains('=') => Some(cadaddy_mepfix::sheet_facts::TextItem {
                value: t.value.trim().to_string(),
                at: t.insert.xy(),
                height: t.height,
                rotation: t.rotation,
            }),
            _ => None,
        })
        .take(MAX_LEGEND_TEXTS)
        .collect();
    cadaddy_mepfix::device_types::designations(&texts)
}

/// Most `KEY=MEANING` texts looked at for the legends' designations.
const MAX_LEGEND_TEXTS: usize = 5_000;
/// Most electrical sheets read again for a legend the job's drawing doesn't hold.
const MAX_LEGEND_SHEETS: usize = 6;

/// The designations on the set's electrical sheets that aren't plans (a legend on the one-line
/// diagram's sheet, or the schedules'; the job's drawing keeps the plans).
fn unimported_designations(job: &GuidedJob) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (_, d) in non_plan_pages(job, "E") {
        for (k, v) in cadaddy_mepfix::device_types::designations(&cadaddy_mepfix::sheet_facts::texts(&d)) {
            out.entry(k).or_insert(v);
        }
    }
    out
}

/// The pages of a discipline's sheets that aren't plans (legends, schedules, diagrams: the job's
/// drawing keeps the plans), each read again from its file, numbered sheets in sheet order.
fn non_plan_pages(job: &GuidedJob, discipline: &str) -> Vec<(String, cadaddy_doc::Drawing)> {
    let mut out = Vec::new();
    let Some(read_pdf) = super::file::io().and_then(|h| h.read_pdf) else {
        progress_line("  the sheets' legends and schedules: no PDF reader in this build");
        return out;
    };
    let classified: Vec<Value> = job.sheets.as_ref().and_then(|v| v.get("sheets")).and_then(Value::as_array).cloned().unwrap_or_default();
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    // The numbered ones (a page known only by its file isn't a sheet of the set), in sheet order.
    let mut sheets: Vec<(String, &Value)> = classified
        .iter()
        .filter_map(|c| {
            let text = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
            let kind = if text("type").is_empty() { text("sheet_type") } else { text("type") };
            let number = c.get("number").and_then(Value::as_str).unwrap_or("").trim().to_string();
            (text("discipline").eq_ignore_ascii_case(discipline) && !kind.contains("plan") && cadaddy_mepfix::sheet_facts::is_sheet_number(&number))
                .then_some((number, c))
        })
        .collect();
    sheets.sort_by(|a, b| a.0.cmp(&b.0));
    let mut missed: Vec<String> = Vec::new();
    for (number, c) in sheets.into_iter().take(MAX_LEGEND_SHEETS) {
        // Its current copy's file and page (`printset.sheets`).
        let (Some(file), Some(page)) = (c.get("file").and_then(Value::as_str), c.get("page").and_then(Value::as_u64)) else { continue };
        if !files.contains_key(file) {
            match crate::readfile::read(std::path::Path::new(&job.pdfs_path).join(file)) {
                Ok(bytes) => {
                    files.insert(file.to_string(), bytes);
                }
                Err(e) => {
                    missed.push(format!("{number} ({file}: {e})"));
                    continue;
                }
            }
        }
        let Some(bytes) = files.get(file) else { continue };
        match read_pdf(bytes, &json!({"page": page, "text": true, "images": false})) {
            Ok((d, _)) => out.push((number, d)),
            Err(e) => missed.push(format!("{number} ({file} page {page}: {e})")),
        }
    }
    if !missed.is_empty() {
        progress_line(&format!("  the sheets' legends and schedules not read: {}", missed.join(", ")));
    }
    out
}

/// The set's plumbing fixture schedule: on a plumbing sheet that isn't a plan, else on a plan.
fn plumbing_schedule(d: &cadaddy_doc::Drawing, job: &GuidedJob) -> Option<cadaddy_mepfix::fixture_schedule::FixtureSchedule> {
    if let Some(s) = non_plan_pages(job, "P").iter().find_map(|(_, page)| cadaddy_mepfix::fixture_schedule::read(page)) {
        return Some(s);
    }
    job_sheet_refs(job)
        .iter()
        .filter(|r| r.view.is_none() && r.discipline.eq_ignore_ascii_case("P"))
        .take(MAX_LEGEND_SHEETS)
        .find_map(|r| cadaddy_mepfix::fixture_schedule::read(&super::printmep::sheet_drawing(d, r)))
}

/// Most plumbing fixtures read from a set's plans.
const MAX_FIXTURES: usize = 20_000;

/// The plumbing fixtures the plumbing plans tag: each tag the schedule lists, lettered on a
/// plumbing sheet, as a device of kind `plumbing fixture` where it is printed. Not the sheet's own
/// number in its title block (P-2 is a sheet and a lavatory), nor lettering larger than a tag's.
fn plumbing_fixtures(
    d: &cadaddy_doc::Drawing,
    job: &GuidedJob,
    schedule: &cadaddy_mepfix::fixture_schedule::FixtureSchedule,
) -> Vec<super::readers::ReadDevice> {
    let tags: BTreeSet<&str> = schedule.rows.iter().map(|r| r.tag.as_str()).collect();
    let spans: Vec<(&str, u64, u64)> = job
        .sheet_spans
        .iter()
        .filter(|sp| cadaddy_mepfix::sheet_facts::discipline_of(sp.sheet.split('/').next().unwrap_or(&sp.sheet)).eq_ignore_ascii_case("P"))
        .map(|sp| (sp.sheet.as_str(), sp.lo, sp.hi))
        .collect();
    // A detail callout's bubble (`7` over `P-7`) names a sheet of the set under its view number: not
    // a fixture, though the schedule has a P-7 and the bubble is the tags' own hexagon.
    let sheet_numbers: BTreeSet<String> = job
        .sheet_regions
        .iter()
        .map(|r| r.number.split('/').next().unwrap_or(&r.number).trim().to_ascii_uppercase())
        .chain(
            job.sheets
                .as_ref()
                .and_then(|v| v.get("sheets"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|c| c.get("number").and_then(Value::as_str).map(|n| n.trim().to_ascii_uppercase())),
        )
        .collect();
    let view_numbers: Vec<(cadaddy_geom::Vec2, &str)> = d
        .model
        .iter()
        .filter(|e| spans.iter().any(|(_, lo, hi)| (*lo..*hi).contains(&e.handle.0)))
        .filter_map(|e| match &e.kind {
            cadaddy_doc::EntityKind::Text(t) if (1..=2).contains(&t.value.trim().len()) && t.value.trim().chars().all(|c| c.is_ascii_digit()) => {
                Some((t.insert.xy(), sheet_of_handle(&spans, e.handle.0)))
            }
            _ => None,
        })
        .take(MAX_FIXTURES)
        .collect();
    let callout = |value: &str, t: &cadaddy_doc::Text, sheet: &str| {
        let h = t.height.max(1e-6);
        sheet_numbers.contains(&value.to_ascii_uppercase())
            && view_numbers.iter().any(|(p, s)| *s == sheet && (p.x - t.insert.x).abs() <= 3.0 * h && p.y > t.insert.y && p.y - t.insert.y <= 4.0 * h)
    };
    let mut found: Vec<(String, [f64; 2], f64, String)> = Vec::new();
    for e in d.model.iter() {
        // Lettered, or a tag symbol's attribute (the hexagon P-1 by a water closet).
        let lettered: Vec<&cadaddy_doc::Text> = match &e.kind {
            cadaddy_doc::EntityKind::Text(t) => vec![t],
            cadaddy_doc::EntityKind::Insert(ins) => ins.attribs.iter().take(8).map(|a| &a.text).collect(),
            _ => continue,
        };
        let Some((sheet, _, _)) = spans.iter().find(|(_, lo, hi)| (*lo..*hi).contains(&e.handle.0)) else { continue };
        for t in lettered {
            let value = t.value.trim();
            if !(tags.contains(value) && t.height.is_finite() && t.height > 0.0 && t.insert.is_finite())
                || sheet_key(sheet) == value
                || callout(value, t, sheet)
            {
                continue;
            }
            found.push((value.to_string(), [t.insert.x, t.insert.y], t.height, sheet.to_string()));
        }
        if found.len() >= MAX_FIXTURES {
            break;
        }
    }
    let mut heights: Vec<f64> = found.iter().map(|f| f.2).collect();
    heights.sort_by(f64::total_cmp);
    let usual = heights.get(heights.len() / 2).copied().unwrap_or(0.0);
    found
        .into_iter()
        .filter(|(_, _, h, _)| *h <= 2.0 * usual)
        .map(|(tag, at, _, sheet)| super::readers::ReadDevice {
            level: job.level_at(at).map_or_else(String::new, |r| r.level.clone()),
            tag,
            at,
            sheet,
            kind: "plumbing fixture".into(),
            ..Default::default()
        })
        .collect()
}

/// The waste and vent stacks the plumbing plans letter (`3" WASTE` over `STACK VENT`, `2" VENT
/// STACK`): each where its leader ends, as an item of kind `plumbing stack` (tag: its size and
/// words, leg: its system, `SAN` or `V`), placed and laid into the rooms like the fixtures.
fn plumbing_stacks(d: &cadaddy_doc::Drawing, job: &GuidedJob) -> Vec<super::readers::ReadDevice> {
    let spans: Vec<(&str, u64, u64)> = job
        .sheet_spans
        .iter()
        .filter(|sp| cadaddy_mepfix::sheet_facts::discipline_of(sp.sheet.split('/').next().unwrap_or(&sp.sheet)).eq_ignore_ascii_case("P"))
        .map(|sp| (sp.sheet.as_str(), sp.lo, sp.hi))
        .collect();
    let sheet_of = |h: u64| spans.iter().find(|(_, lo, hi)| (*lo..*hi).contains(&h)).map(|(s, _, _)| *s);
    let mut texts: Vec<(String, cadaddy_geom::Vec2, f64, &str)> = Vec::new();
    let mut segs: Vec<(cadaddy_geom::Vec2, cadaddy_geom::Vec2, &str)> = Vec::new();
    for e in d.model.iter() {
        let Some(sheet) = sheet_of(e.handle.0) else { continue };
        match &e.kind {
            cadaddy_doc::EntityKind::Text(t) if t.height.is_finite() && t.height > 0.0 && t.insert.is_finite() && texts.len() < MAX_FIXTURES => {
                texts.push((t.value.trim().to_ascii_uppercase(), t.insert.xy(), t.height, sheet));
            }
            cadaddy_doc::EntityKind::Line(l) if segs.len() < MAX_LEADER_PIECES => segs.push((l.a.xy(), l.b.xy(), sheet)),
            cadaddy_doc::EntityKind::LwPolyline(pl) if !pl.closed && pl.vertices.len() <= 8 && segs.len() < MAX_LEADER_PIECES => {
                segs.extend(pl.vertices.windows(2).filter_map(|w| Some((w.first()?.p, w.get(1)?.p, sheet))));
            }
            _ => {}
        }
    }
    // `3"`, `1 1/2"`, `4"` at the start: the pipe size.
    let size_of = |t: &str| -> Option<f64> {
        let (num, _) = t.split_once('"')?;
        let num = num.trim().replace('-', " ");
        let mut total = 0.0;
        for part in num.split_whitespace().take(2) {
            total += match part.split_once('/') {
                Some((a, b)) => a.parse::<f64>().ok()? / b.parse::<f64>().ok().filter(|b| *b > 0.0)?,
                None => part.parse::<f64>().ok()?,
            };
        }
        (total > 0.0 && total <= 12.0).then_some(total)
    };
    let mut out = Vec::new();
    for (value, at, h, sheet) in &texts {
        let Some(size) = size_of(value) else { continue };
        if !(value.contains("WASTE") || value.contains("VENT") || value.contains("SOIL")) {
            continue;
        }
        // Its second line (`STACK VENT` under `3" WASTE`).
        let under = texts.iter().find(|(v, p, _, s2)| {
            s2 == sheet && v.starts_with("STACK") && (p.x - at.x).abs() <= 2.0 * h && at.y - p.y > 0.0 && at.y - p.y <= 2.2 * h
        });
        let label = match under {
            Some((v, ..)) => format!("{value} {v}"),
            None => value.clone(),
        };
        if !label.contains("STACK") {
            continue;
        }
        let width = 0.8 * h * label.chars().count().min(24) as f64;
        let lo = cadaddy_geom::Vec2::new(at.x - 1.5 * h, under.map_or(at.y, |u| u.1.y) - 1.5 * h);
        let hi = cadaddy_geom::Vec2::new(at.x + width + 1.5 * h, at.y + 2.5 * h);
        let near = |p: cadaddy_geom::Vec2| p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y;
        // Its leader: a straight piece from the label out, to its far end.
        let Some(end) = segs
            .iter()
            .filter(|(_, _, s2)| s2 == sheet)
            .filter_map(|(a, b, _)| match (near(*a), near(*b)) {
                (true, false) => Some(*b),
                (false, true) => Some(*a),
                _ => None,
            })
            .max_by(|p, q| p.dist(*at).total_cmp(&q.dist(*at)))
            .filter(|e| e.dist(*at) <= 40.0 * h)
        else {
            continue;
        };
        let system = if label.contains("WASTE") || label.contains("SOIL") { "SAN" } else { "V" };
        out.push(super::readers::ReadDevice {
            tag: format!("{label} ({size}\")"),
            at: [end.x, end.y],
            sheet: (*sheet).to_string(),
            level: job.level_at([end.x, end.y]).map_or_else(String::new, |r| r.level.clone()),
            kind: "plumbing stack".into(),
            leg: system.into(),
            ..Default::default()
        });
        if out.len() >= MAX_FIXTURES {
            break;
        }
    }
    out
}

/// The stacks ([`plumbing_stacks`]), placed, as risers through their storey: the system (a
/// sanitary one made when the spec has none) and size their label gives. How many.
fn stacks_as_risers(sp: &mut cadaddy_walk3d::BuildingSpec, stacks: &[super::readers::ReadDevice]) -> usize {
    if stacks.iter().any(|st| st.leg == "SAN")
        && !sp.mep.systems.iter().any(|y| y.name == "SAN")
        && let Ok(sys) = serde_json::from_value(
            json!({"name": "SAN", "kind": "pipe", "layer": "P-SAN", "size_in": [3.0, 0.0], "bottom_ft": 0.0, "color_index": 30, "show": true}),
        )
    {
        sp.mep.systems.push(sys);
    }
    let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let mut n = 0usize;
    for st in stacks {
        let Some(lv) =
            sp.level(&st.level).or_else(|| sp.levels.iter().find(|l| !digits(&st.level).is_empty() && digits(&l.name) == digits(&st.level)))
        else {
            continue;
        };
        let (name, top) = (lv.name.clone(), lv.height_ft);
        if !(sp.mep.systems.iter().any(|y| y.name == st.leg) && top.is_finite() && top > 0.0) {
            continue;
        }
        let size = st.tag.rsplit_once('(').and_then(|(_, r)| r.trim_end_matches(')').trim_end_matches('"').parse::<f64>().ok()).unwrap_or(3.0);
        let Ok(run) = serde_json::from_value::<cadaddy_walk3d::MepRun>(json!({
            "system": st.leg, "level": name, "points": [st.at], "size_in": [size, 0.0], "bottoms_ft": [0.0], "top_ft": top,
            "height_source": "stack through the storey", "size_source": "tag", "source_sheet": st.sheet,
        })) else {
            continue;
        };
        sp.mep.runs.push(run);
        n += 1;
    }
    n
}

/// The sheet of the set an object (by its handle) was imported for, among `spans`.
fn sheet_of_handle<'a>(spans: &[(&'a str, u64, u64)], h: u64) -> &'a str {
    spans.iter().find(|(_, lo, hi)| (*lo..*hi).contains(&h)).map_or("", |(s, _, _)| *s)
}

/// Whether a typical room's type (its words, [`type_words`]) is a room to sleep in: a hotel's
/// guest room (QUEEN, KING, SUITE…) or a dwelling unit.
fn sleeping_room(words: &BTreeSet<String>) -> bool {
    const SLEEPING: &[&str] = &["Q", "KING", "K", "SUITE", "STUDIO", "GUEST", "BEDROOM", "BED", "UNIT", "APT", "APARTMENT", "DWELLING"];
    words.iter().any(|w| SLEEPING.contains(&w.as_str()))
}

/// A stamped room's name for its own circuits: its number, else its name and place in the list.
fn stamped_room(name: &str, number: &str, i: usize) -> String {
    if number.trim().is_empty() { format!("{} {i}", name.trim()) } else { number.trim().to_string() }
}

/// The one room a view's title (upper case) names: its number ("ROOM #237", "ROOM 237") when one
/// room has it, else its name ("1ST FLOOR - MEP ROOM": the room called MEP), on the floor the
/// title names when several are.
fn room_of_view(title: &str, rooms: &[JobRoom]) -> Option<usize> {
    if !title.contains("ROOM") {
        return None;
    }
    let tokens: Vec<&str> = title.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let numbered: Vec<usize> = tokens
        .iter()
        .filter(|t| (2..=5).contains(&t.len()) && t.chars().any(|c| c.is_ascii_digit()) && t.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .flat_map(|t| rooms.iter().enumerate().filter(move |(_, r)| r.3.eq_ignore_ascii_case(t)).map(|(i, _)| i))
        .collect();
    if let [i] = numbered[..] {
        return Some(i);
    }
    // The name: the title's words but the view's own (ENLARGED, PLAN, the trade, the floor).
    const VIEW_WORDS: &[&str] =
        &["ENLARGED", "PLAN", "PLANS", "ELECTRICAL", "MECHANICAL", "PLUMBING", "LIGHTING", "POWER", "FLOOR", "LEVEL", "ROOM", "SCALE"];
    let ordinal =
        |w: &str| w.len() >= 3 && w.chars().next().is_some_and(|c| c.is_ascii_digit()) && ["ST", "ND", "RD", "TH"].iter().any(|e| w.ends_with(e));
    // An equipment room by any of its names (the trades' plans call it MEP, the architect MECH).
    let same = |w: &str| -> String {
        match w {
            "MEP" | "MECH" | "MECHANICAL" | "ELEC" | "EQUIPMENT" => "MEP".to_string(),
            w => w.to_string(),
        }
    };
    let name: Vec<String> =
        tokens.iter().copied().filter(|w| !VIEW_WORDS.contains(w) && !ordinal(w) && !w.chars().all(|c| c.is_ascii_digit())).map(same).collect();
    if name.is_empty() {
        return None;
    }
    let floor: Option<String> = tokens.iter().find(|w| ordinal(w)).map(|w| w.chars().take_while(char::is_ascii_digit).collect());
    let named: Vec<usize> = rooms
        .iter()
        .enumerate()
        .filter(|(_, r)| {
            let words: Vec<String> =
                r.0.to_ascii_uppercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty() && *w != "ROOM").map(same).collect();
            !words.is_empty() && words.len() == name.len() && name.iter().all(|n| words.iter().any(|w| w == n))
        })
        .map(|(i, _)| i)
        .collect();
    match named[..] {
        [i] => Some(i),
        _ => {
            let on_floor: Vec<usize> =
                named.into_iter().filter(|i| floor.as_deref().is_some_and(|f| rooms.get(*i).is_some_and(|r| r.1.trim() == f))).collect();
            if let [i] = on_floor[..] { Some(i) } else { None }
        }
    }
}

/// The wall tags of the typical-room plans (unit plans): each legend mark a typical-room view
/// prints, put at the same place in every room of the type the view's title names (as the
/// devices are stamped). By level (its digits): (mark, where on the level's plan).
fn typical_wall_tags(d: &cadaddy_doc::Drawing, job: &GuidedJob, marks: &BTreeSet<String>) -> BTreeMap<String, Vec<(String, cadaddy_geom::Vec2)>> {
    let mut out: BTreeMap<String, Vec<(String, cadaddy_geom::Vec2)>> = BTreeMap::new();
    let rooms: Vec<(String, String, Vec<cadaddy_geom::Vec2>)> = job
        .stage(StageName::Rooms)
        .and_then(|st| st.result.as_ref())
        .and_then(|r| r.get("rooms"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| r.get("other_trade").and_then(Value::as_bool) != Some(true))
        .filter_map(|r| {
            let name = r.get("name").and_then(Value::as_str)?.trim().to_string();
            let level = r.get("level").and_then(Value::as_str).unwrap_or("").to_string();
            let outline: Vec<cadaddy_geom::Vec2> = r
                .get("outline")?
                .as_array()?
                .iter()
                .filter_map(|q| Some(cadaddy_geom::Vec2::new(q.get(0)?.as_f64()?, q.get(1)?.as_f64()?)))
                .collect();
            (outline.len() >= 3 && !name.is_empty()).then_some((name, level, outline))
        })
        .collect();
    let mut count: BTreeMap<BTreeSet<String>, usize> = BTreeMap::new();
    for (name, _, _) in &rooms {
        *count.entry(type_words(name, None)).or_default() += 1;
    }
    let types: BTreeSet<BTreeSet<String>> = count.into_iter().filter(|(k, n)| *n > 1 && !k.is_empty()).map(|(k, _)| k).collect();
    let vocabulary: BTreeSet<String> = types.iter().flatten().cloned().collect();
    if types.is_empty() || marks.is_empty() {
        return out;
    }
    let bbox = |o: &[cadaddy_geom::Vec2]| {
        o.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, q| {
            [b[0].min(q.x), b[1].min(q.y), b[2].max(q.x), b[3].max(q.y)]
        })
    };
    let key = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let refs = job_sheet_refs(job);
    for sh in refs.iter().filter(|r| r.view.is_none() && r.discipline.eq_ignore_ascii_case("A") && r.kind == "enlarged_plan").take(MAX_STAMP_SHEETS) {
        let sub = super::printmep::sheet_drawing(d, sh);
        let texts = cadaddy_mepfix::sheet_facts::texts(&sub);
        let views: Vec<([f64; 4], BTreeSet<String>)> = cadaddy_mepfix::sheet_facts::view_titles(&sub, &texts)
            .into_iter()
            .filter_map(|v| {
                let upper = v.title.to_ascii_uppercase();
                let plan = !["ELEVATION", "SECTION", "DETAIL", "RCP", "CEILING"].iter().any(|w| upper.contains(w));
                let words = type_words(&v.title, Some(&vocabulary));
                (plan && types.contains(&words)).then_some((v.region?, words))
            })
            .collect();
        if views.is_empty() {
            continue;
        }
        for t in texts.iter() {
            let mark = t.value.trim().to_ascii_uppercase();
            if !marks.contains(&mark) {
                continue;
            }
            let p = [t.at.x, t.at.y];
            let inside = |r: &[f64; 4]| p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3];
            let area = |r: &[f64; 4]| (r[2] - r[0]) * (r[3] - r[1]);
            let Some((view, words)) = views.iter().filter(|(r, _)| inside(r)).min_by(|a, b| area(&a.0).total_cmp(&area(&b.0))) else {
                continue;
            };
            for (_, level, outline) in rooms.iter().filter(|(n, _, _)| type_words(n, None) == *words) {
                if let Some(q) = into_room(p, *view, bbox(outline)) {
                    out.entry(key(level)).or_default().push((mark.clone(), cadaddy_geom::Vec2::new(q[0], q[1])));
                }
            }
        }
    }
    out
}

/// At most this many sheets are looked over for typical-room plans, and this many devices stamped.
const MAX_STAMP_SHEETS: usize = 40;
const MAX_STAMPED: usize = 20_000;

/// A room smaller than this (square feet) is no room of its own when a bigger room of its level
/// carries its number (or, unnumbered, its name): a sliver of that room traced from its label (a
/// tub, a closet's corner).
const SCRAP_SF: f64 = 15.0;

/// Numbers each unnumbered room from the same room on another floor (typical floors: room 305
/// over 205 and under 405); how many.
fn number_stacked_rooms(s: &mut Session) -> Result<usize> {
    let upf = match super::bspec::current(s, "bim_build") {
        Ok((sp, true)) if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 => sp.units_per_foot,
        _ => 12.0,
    };
    let numbers = stacked_numbers(&super::bspec::room_elements(s.doc()?), upf);
    for (h, n) in &numbers {
        s.doc_mut()?.modify_entity(*h, |e| {
            let mut x = e.common.xdata.to_vec();
            x.push(cadaddy_doc::RawTag { code: 1000, value: format!("number:{n}") });
            e.common.xdata = x.into();
        })?;
    }
    Ok(numbers.len())
}

/// The numbers [`number_stacked_rooms`] gives: an unnumbered room of a level takes the number of a
/// room stacked on it on another level (the same name, or that room known by its number alone; its
/// middle within 2 feet; its area within a third: the floors' outlines of one room differ), that
/// level's floor (the digits of its name, the number's first characters) swapped for its own.
/// Only when every such room agrees and the number is new on its level.
fn stacked_numbers(rooms: &[super::bspec::RoomElement], upf: f64) -> Vec<(cadaddy_doc::Handle, String)> {
    let floor = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let norm = |n: &str| n.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_uppercase();
    // Known by its number alone ("Room 305"): no name to tell it from another.
    let bare = |o: &super::bspec::RoomElement| {
        let n = norm(&o.name);
        n == "ROOM" || o.number.as_deref().is_some_and(|num| n == format!("ROOM {}", num.trim().to_ascii_uppercase()))
    };
    let mid = |r: &super::bspec::RoomElement| {
        let b = cadaddy_geom::Bounds2::from_points(r.outline.iter().copied());
        (!b.is_empty()).then(|| b.center())
    };
    let mut out: Vec<(cadaddy_doc::Handle, String, String)> = Vec::new();
    for r in rooms.iter().filter(|r| r.number.is_none()) {
        let (name, here) = (norm(&r.name), floor(&r.level));
        let Some(m) = mid(r) else { continue };
        if name.is_empty() || name == "ROOM" || here.is_empty() || r.area_sf.is_nan() || r.area_sf <= 0.0 {
            continue;
        }
        let proposed: BTreeSet<String> = rooms
            .iter()
            .filter(|o| o.level != r.level && (norm(&o.name) == name || bare(o)) && (o.area_sf - r.area_sf).abs() <= 0.35 * r.area_sf.max(o.area_sf))
            // Over it: the middles within 2', or each in the other (one floor's trace takes in its
            // bathroom, another's doesn't).
            .filter(|o| {
                mid(o).is_some_and(|c| {
                    c.dist(m) <= 2.0 * upf || (cadaddy_geom::point_in_polygon(&r.outline, c) && cadaddy_geom::point_in_polygon(&o.outline, m))
                })
            })
            .filter_map(|o| {
                let (n, there) = (o.number.as_deref()?.trim(), floor(&o.level));
                let rest = n.strip_prefix(there.as_str()).filter(|_| !there.is_empty() && n.len() >= 3)?;
                (!rest.is_empty()).then(|| format!("{here}{rest}"))
            })
            .collect();
        if proposed.len() == 1
            && let Some(n) = proposed.into_iter().next()
        {
            out.push((r.handle, r.level.clone(), n));
        }
    }
    // New on its level: neither a room of the level has it nor two rooms are given it.
    let taken =
        |level: &str, n: &str| rooms.iter().any(|o| o.level == level && o.number.as_deref().is_some_and(|m| m.trim().eq_ignore_ascii_case(n)));
    let twice = |level: &str, n: &str| out.iter().filter(|(_, l, m)| l == level && m == n).count() > 1;
    out.iter().filter(|(_, l, n)| !taken(l, n) && !twice(l, n)).map(|(h, _, n)| (*h, n.clone())).collect()
}

/// Removes the scraps ([`SCRAP_SF`]); how many.
fn drop_numbered_scraps(s: &mut Session) -> Result<usize> {
    let gone = numbered_scraps(&super::bspec::room_elements(s.doc()?));
    for h in &gone {
        s.doc_mut()?.remove_entity(*h);
    }
    Ok(gone.len())
}

/// The scraps ([`SCRAP_SF`]) among `rooms`.
fn numbered_scraps(rooms: &[super::bspec::RoomElement]) -> Vec<cadaddy_doc::Handle> {
    let norm = |n: &str| n.trim().to_ascii_uppercase();
    rooms
        .iter()
        .filter(|r| r.area_sf < SCRAP_SF)
        .filter(|r| {
            let bigger = |o: &&super::bspec::RoomElement| o.handle != r.handle && o.level == r.level && o.area_sf >= SCRAP_SF;
            match r.number.as_deref() {
                Some(n) => rooms.iter().filter(bigger).any(|o| o.number.as_deref().is_some_and(|m| norm(m) == norm(n))),
                // Unnumbered: by its name, a real one (not the bare "Room").
                None => !norm(&r.name).is_empty() && norm(&r.name) != "ROOM" && rooms.iter().filter(bigger).any(|o| norm(&o.name) == norm(&r.name)),
            }
        })
        .map(|r| r.handle)
        .collect()
}

/// How far `p` is from the nearest edge of the closed `outline`.
pub(crate) fn edge_distance(outline: &[cadaddy_geom::Vec2], p: cadaddy_geom::Vec2) -> f64 {
    let n = outline.len();
    (0..n)
        .map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            let ab = b - a;
            let len2 = ab.x * ab.x + ab.y * ab.y;
            let t = if len2 > 0.0 { (((p.x - a.x) * ab.x + (p.y - a.y) * ab.y) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let c = cadaddy_geom::Vec2::new(a.x + ab.x * t, a.y + ab.y * t);
            (p.x - c.x).hypot(p.y - c.y)
        })
        .fold(f64::INFINITY, f64::min)
}

/// Where a sheet point goes on its level's architectural plan (architecture is the rule), by the
/// first that holds: its view's grid intersections; the sheet's registration by shared door and
/// room numbers; the frame of a same-size architectural sheet; its view's
/// screened background laid over the floor plan (a fire alarm or low-voltage plan). `None` when
/// none holds, or the point would land off the level's plan.
fn arch_placer(
    d: &cadaddy_doc::Drawing,
    sp: &cadaddy_walk3d::BuildingSpec,
    shells: &[(String, Vec<[cadaddy_geom::Vec2; 2]>)],
    enlarged: Vec<EnlargedFit>,
) -> impl Fn([f64; 2], &str) -> Option<cadaddy_mepfix::tagid::Sim> + use<> {
    let sheets = super::identity::sheets_of(Some(sp));
    let sheet_rects: Vec<(String, [f64; 4])> = sheets.iter().map(|b| (cadaddy_mepfix::printlinks::norm_sheet(&b.number), b.rect)).collect();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let clock = web_time::Instant::now();
    let lap = |what: &str| progress_line(&format!("  placer: {what} at {:.1} s", clock.elapsed().as_secs_f64()));
    let reg = super::identity::register(d, &sheets, upf);
    lap("sheets registered by tags");
    // Each level's architectural plan (the sheet the registration takes as its reference) and
    // the other sheets: their grid intersections, for views no tag fit reaches.
    let level_key0 = |l: &str| {
        format!("{}{}", l.chars().filter(char::is_ascii_digit).collect::<String>(), {
            let u = l.trim().to_ascii_uppercase();
            u.contains("ROOF") || u == "R"
        })
    };
    let plans: Vec<(String, [f64; 4])> = sp
        .levels
        .iter()
        .filter_map(|l| Some((level_key0(&l.name), l.region.filter(|r| r.iter().all(|v| v.is_finite() && v.abs() <= 1.0e8))?)))
        .collect();
    let others: Vec<[f64; 4]> = sheets.iter().filter(|b| !b.discipline.eq_ignore_ascii_case("A")).map(|b| b.rect).collect();
    let inside = |r: &[f64; 4], p: [f64; 2]| p[0] >= r[0] && p[0] <= r[2] && p[1] >= r[1] && p[1] <= r[3];
    let level_key = |l: &str| {
        format!("{}{}", l.chars().filter(char::is_ascii_digit).collect::<String>(), {
            let u = l.trim().to_ascii_uppercase();
            u.contains("ROOF") || u == "R"
        })
    };
    // Architecture is the rule: a placed point must land on its level's plan (5% margin).
    let regions: Vec<(String, [f64; 4])> = sp
        .levels
        .iter()
        .filter_map(|l| {
            let r = l.region.filter(|r| r.iter().all(|v| v.is_finite() && v.abs() <= 1.0e8))?;
            let (mx, my) = (0.05 * (r[2] - r[0]).abs(), 0.05 * (r[3] - r[1]).abs());
            Some((level_key(&l.name), [r[0].min(r[2]) - mx, r[1].min(r[3]) - my, r[0].max(r[2]) + mx, r[1].max(r[3]) + my]))
        })
        .collect();
    // Views that aren't plans (an elevation, a section, a riser diagram): what they draw is no plan
    // geometry, and their grid bubbles would place it on the plan all the same.
    // A view's region can take in its neighbour's linework: a point in any plan view's region stays
    // a plan's.
    let views = cadaddy_mepfix::sheet_facts::view_titles(d, &cadaddy_mepfix::sheet_facts::texts(d));
    let plan_word = |t: &str| t.to_ascii_uppercase().contains("PLAN");
    let not_plan_word =
        |t: &str| ["ELEVATION", "SECTION", "DETAIL", "RISER", "DIAGRAM", "SCHEDULE", "ISOMETRIC"].iter().any(|w| t.to_ascii_uppercase().contains(w));
    // A view not titled as something else is a plan ("MECHANICAL - 1ST FLOOR CORE AREA").
    let plan_views: Vec<[f64; 4]> = views.iter().filter(|v| plan_word(&v.title) || !not_plan_word(&v.title)).filter_map(|v| v.region).collect();
    let not_plan: Vec<[f64; 4]> = views.iter().filter(|v| !plan_word(&v.title) && not_plan_word(&v.title)).filter_map(|v| v.region).collect();
    lap(&format!("views read ({} plans, {} not plans)", plan_views.len(), not_plan.len()));
    // Each view's own grids (a sheet of enlarged views shows the same grids in each), its views
    // read off its sheet alone: over the whole job's drawing a cell of the view finder's grid is a
    // good part of a sheet, and a sheet's views run together.
    // And the views the job gave a level (a sheet's view of one floor), which the titles may miss.
    let view_rects: Vec<(String, [f64; 4])> =
        sp.sheets.iter().filter_map(|l| Some((level_key(l.level.as_deref()?), l.view.as_ref()?.rect))).collect();
    let mut grid_views = sheet_plan_views(d, &others);
    grid_views.extend(view_rects.iter().map(|(_, r)| *r));
    let grid = super::grids::GridPlacer::new(d, upf, &plans, &others, &grid_views);
    lap("grid intersections read");
    progress_line(&format!("  grid intersections: {}", grid.summary()));
    let place = move |p: [f64; 2], level: &str| -> Option<cadaddy_mepfix::tagid::Sim> {
        if not_plan.iter().any(|r| inside(r, p)) && !plan_views.iter().any(|r| inside(r, p)) {
            return None;
        }
        let sheet = sheets.iter().find(|b| inside(&b.rect, p))?;
        let on = |t: &&cadaddy_mepfix::tagid::SheetTransform| t.registered && t.sheet.eq_ignore_ascii_case(&sheet.number);
        let ts: Vec<_> = reg.transforms.iter().filter(on).collect();
        // Another level's fit, but never for a point in this level's own view of a sheet of
        // several levels' plans: that fit is the other view's.
        let own_view = view_rects.iter().any(|(k, r)| *k == level_key(level) && inside(r, p));
        let fit = ts.iter().find(|t| level_key(&t.level) == level_key(level)).or(ts.first().filter(|_| !own_view));
        // The grid lines its own view shows first: a sheet of enlarged plans has views at their
        // own scales, and one fit of the whole sheet by its tags puts at most one of them right.
        if let Some((k, t)) = grid.place(p, &level_key(level)) {
            return Some(cadaddy_mepfix::tagid::Sim::scale_offset(k, t));
        }
        // Else the sheet's fit by its tags; one on a few tags or off 1:1 is no fit.
        if let Some(t) = fit.filter(|t| t.matched >= 4 && (0.85..=1.18).contains(&t.transform.scale())) {
            // The fit puts the sheet onto its reference plan, which for a several-level sheet
            // may be another level's: from there onto this level's own plan, by their frames.
            let mut m = t.transform;
            let own = |b: &&super::identity::SheetBox| {
                b.discipline.eq_ignore_ascii_case("A")
                    && level_key(&b.level) == level_key(level)
                    && reg.transforms.iter().any(|x| x.sheet.eq_ignore_ascii_case(&b.number) && x.method == "reference")
            };
            if let (Some(from), Some(to)) = (sheets.iter().find(|b| b.number.eq_ignore_ascii_case(&t.reference)), sheets.iter().find(own))
                && !from.number.eq_ignore_ascii_case(&to.number)
            {
                m.tx += to.rect[0] - from.rect[0];
                m.ty += to.rect[1] - from.rect[1];
            }
            return Some(m);
        }
        // Else the frame: a sheet the size of the level's architectural plan sheet has
        // its plan in the same place on the page.
        let size = |b: &super::identity::SheetBox| [b.rect[2] - b.rect[0], b.rect[3] - b.rect[1]];
        let alike = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() <= 0.01 * b[0] && (a[1] - b[1]).abs() <= 0.01 * b[1];
        let arch = sheets.iter().find(|b| {
            b.discipline.eq_ignore_ascii_case("A")
                && level_key(&b.level) == level_key(level)
                && alike(size(b), size(sheet))
                && reg.transforms.iter().any(|t| t.sheet.eq_ignore_ascii_case(&b.number) && t.method == "reference")
        })?;
        Some(cadaddy_mepfix::tagid::Sim::scale_offset(1.0, [arch.rect[0] - sheet.rect[0], arch.rect[1] - sheet.rect[1]]))
    };
    // A view with neither grids nor tags: its screened background laid over the floor plan.
    let backgrounds = super::grids::Backgrounds::new(d, &others);
    lap("backgrounds grouped");
    // Each level's floor plan linework (points every foot), for laying a view's background on it.
    let k_in = super::perceive::units_per_inch(d);
    let plan_lines: Vec<(String, Vec<cadaddy_geom::Vec2>)> = plans
        .iter()
        .map(|(key, r)| {
            let mine: Vec<&std::sync::Arc<cadaddy_doc::Entity>> = super::bspec::plan_objects(d)
                .into_iter()
                .filter(|e| {
                    super::bspec::centre(d, e)
                        .is_some_and(|c| c.x >= r[0].min(r[2]) && c.x <= r[0].max(r[2]) && c.y >= r[1].min(r[3]) && c.y <= r[1].max(r[3]))
                })
                .collect();
            (key.clone(), super::grids::line_samples(&mine, 12.0 * k_in))
        })
        .collect();
    lap("floor plan linework sampled");
    // The level views of sheets that print several levels' plans (each its own level's part of
    // the sheet): their linework, for laying a view onto its level's plan when nothing else does.
    let level_views: Vec<(String, [f64; 4], Vec<cadaddy_geom::Vec2>)> = {
        let views: Vec<(String, [f64; 4])> = sp.sheets.iter().filter_map(|l| Some((level_key(l.level.as_deref()?), l.view.as_ref()?.rect))).collect();
        let mut mine: Vec<Vec<&std::sync::Arc<cadaddy_doc::Entity>>> = vec![Vec::new(); views.len()];
        if !views.is_empty() {
            for e in super::bspec::plan_objects(d) {
                if let Some(c) = super::bspec::centre(d, e)
                    && let Some(i) = views.iter().position(|(_, r)| inside(r, [c.x, c.y]))
                {
                    mine[i].push(e);
                }
            }
        }
        // Its plan: the biggest block of its lines (a legend, notes and the title stand apart).
        views
            .into_iter()
            .zip(mine)
            .map(|((k, r), es)| (k, r, super::grids::largest_block(&super::grids::line_samples(&es, 12.0 * k_in), 48.0 * k_in)))
            .collect()
    };
    lap("level views sampled");
    let view_fits: std::sync::Mutex<std::collections::HashMap<usize, Option<cadaddy_mepfix::tagid::Sim>>> = Default::default();
    // Each level's base point (where the building's origin is on its plan), once the build has
    // registered the storeys (shell corners alone are feet off: a fit moved by them lost devices).
    let bases: Vec<(String, [f64; 2])> =
        sp.levels.iter().filter_map(|l| Some((level_key(&l.name), l.base.filter(|b| b.iter().all(|v| v.is_finite()))?))).collect();
    let plan_lines_v = plan_lines.clone();
    let fits: std::sync::Mutex<std::collections::HashMap<(i64, i64, String), Option<cadaddy_mepfix::tagid::Sim>>> = Default::default();
    let place = move |p: [f64; 2], level: &str, lands: &dyn Fn(&cadaddy_mepfix::tagid::Sim) -> bool| {
        place(p, level)
            .filter(|m| lands(m))
            .or_else(|| {
                let bg = backgrounds.around(p)?;
                let key = level_key(level);
                let id = (bg.min.x.round() as i64, bg.min.y.round() as i64, key.clone());
                if let Some(f) = fits.lock().ok()?.get(&id) {
                    return f.filter(|m| lands(m));
                }
                // The same view fitted for another level: the levels' plans lie apart by their
                // base points (a view of the typical floor is asked for each of its levels).
                let base = |k: &str| bases.iter().find(|(b, _)| b == k).map(|(_, v)| *v);
                let fitted =
                    fits.lock().ok()?.iter().find(|((x, y, k), _)| *x == id.0 && *y == id.1 && *k != key).map(|((_, _, k), f)| (k.clone(), *f));
                if let Some((k, f)) = fitted
                    && let (Some(from), Some(to)) = (base(&k), base(&key))
                {
                    let got = f.map(|m| cadaddy_mepfix::tagid::Sim { tx: m.tx + to[0] - from[0], ty: m.ty + to[1] - from[1], ..m });
                    fits.lock().ok()?.insert(id, got);
                    return got.filter(|m| lands(m));
                }
                let tgt = plan_lines.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_slice()).unwrap_or(&[]);
                let started = web_time::Instant::now();
                let pts = backgrounds.points_in(&bg);
                // A whole floor's background at any scale; else a part of one (an enlarged view of
                // a few rooms) at its sheet's scale.
                let fit = super::grids::linework_fit(&pts, tgt, k_in)
                    .filter(|f| f.2 >= 0.9 && f.3 >= 0.03)
                    .or_else(|| part_of_plan(&pts, tgt).then(|| super::grids::linework_fit_part(&pts, tgt, k_in, 0.95, 1.05)).flatten());
                progress_line(&format!("  placer: a view's background fitted in {:.1} s", started.elapsed().as_secs_f64()));
                if let Some((s, _, score, margin)) = fit {
                    progress_line(&format!(
                        "  background view at {:?} onto level {level}: scale {s:.3}, {:.0}% on the plan, {:.0} points over the next best",
                        [bg.min.x.round(), bg.min.y.round()],
                        score * 100.0,
                        margin * 100.0
                    ));
                }
                // 90% of both ways, and 3 points over the best placement 10' or more away: a floor of
                // repeating bays one bay over scores close behind (only its ends differ).
                let part = part_of_plan(&pts, tgt);
                // A part of the floor fitted a few percent off one to one: its sampling's error (a
                // few rooms can't tell 3% apart), so one to one about the view's middle.
                let mid = pts.iter().fold(cadaddy_geom::Vec2::new(0.0, 0.0), |a, q| a + *q) * (1.0 / pts.len().max(1) as f64);
                let got = fit.filter(|f| (f.2 >= 0.9 || (f.2 >= LEVEL_VIEW_F1 && part)) && f.3 >= 0.03).map(|(s, t, _, _)| {
                    let (s, t) = if part && (s - 1.0).abs() < 0.05 { (1.0, t + mid * (s - 1.0)) } else { (s, t) };
                    cadaddy_mepfix::tagid::Sim::scale_offset(s, [t.x, t.y])
                });
                fits.lock().ok()?.insert(id, got);
                got.filter(|m| lands(m))
            })
            .or_else(|| {
                // A level's view on a sheet of several levels' plans: its whole linework laid on the
                // level's plan.
                let key = level_key(level);
                let i = level_views.iter().position(|(k, r, _)| *k == key && inside(r, p))?;
                if let Some(f) = view_fits.lock().ok()?.get(&i) {
                    return f.filter(|m| lands(m));
                }
                let tgt = plan_lines_v.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_slice()).unwrap_or(&[]);
                // Sheets are imported at their printed scales: a level's view lies on its plan one to
                // one (another scale is another drawing that happens to share lines with it).
                let fit = super::grids::linework_fit_within(&level_views[i].2, tgt, k_in, 0.97, 1.03).or_else(|| {
                    // An enlarged view of a part of the floor (a few guest rooms) lies on its part.
                    part_of_plan(&level_views[i].2, tgt).then(|| super::grids::linework_fit_part(&level_views[i].2, tgt, k_in, 0.97, 1.03)).flatten()
                });
                if fit.is_none() {
                    progress_line(&format!(
                        "  level view {:?} onto level {level}: no fit ({} view points, {} plan points)",
                        level_views[i].1.map(f64::round),
                        level_views[i].2.len(),
                        tgt.len()
                    ));
                }
                if let Some((sc, t, score, margin)) = fit {
                    progress_line(&format!(
                        "  level view {:?} onto level {level}: scale {sc:.3}, offset {:?}, {:.0}% on the plan, {:.0} points over the next best",
                        level_views[i].1.map(f64::round),
                        [t.x.round(), t.y.round()],
                        score * 100.0,
                        margin * 100.0
                    ));
                }
                // A fit a few percent off one to one is its sampling's error (lengths would shrink with
                // it): one to one, about the view's middle.
                let pts = &level_views[i].2;
                let mid = pts.iter().fold(cadaddy_geom::Vec2::new(0.0, 0.0), |a, q| a + *q) * (1.0 / pts.len().max(1) as f64);
                let got = fit.filter(|f| f.2 >= LEVEL_VIEW_F1 && f.3 >= 0.03).map(|(s, t, _, _)| {
                    let (s, t) = if (s - 1.0).abs() < 0.03 { (1.0, t + mid * (s - 1.0)) } else { (s, t) };
                    cadaddy_mepfix::tagid::Sim::scale_offset(s, [t.x, t.y])
                });
                view_fits.lock().ok()?.insert(i, got);
                got.filter(|m| lands(m))
            })
    };
    // Architecture is the rule, the building's shell first: a placed point lands inside its
    // level's shell (within reach of its walls: a wall pack, a condenser by the wall), else
    // (a level with no shell: the roof) on its level's plan.
    let shells: Vec<(String, Vec<[cadaddy_geom::Vec2; 2]>)> = shells.iter().map(|(l, r)| (level_key(l), r.clone())).collect();
    let reach = SHELL_REACH_FT * upf;
    // Each way of placing it is tried in turn until one lands there (a tag fit that puts a
    // second level's view of the sheet off the building leaves it to the view's own fit).
    let placed = move |p: [f64; 2], level: &str| {
        let key = level_key(level);
        let lands = |m: &cadaddy_mepfix::tagid::Sim| {
            let q = m.apply(p);
            if let Some((_, rects)) = shells.iter().find(|(k, _)| *k == key) {
                return rects.iter().any(|[a, b]| q[0] >= a.x - reach && q[0] <= b.x + reach && q[1] >= a.y - reach && q[1] <= b.y + reach);
            }
            regions.iter().filter(|(k, _)| *k == key).any(|(_, r)| inside(r, q)) || regions.iter().all(|(k, _)| *k != key)
        };
        place(p, level, &lands).or_else(|| {
            // An enlarged view the sheet map registered on its overall plan (by the box drawn
            // round its part of the floor): onto that plan, then from there as its points go.
            let sheet = sheet_rects.iter().find(|(_, r)| inside(r, p)).map(|(n, _)| n.as_str())?;
            let e = enlarged.iter().find(|e| {
                cadaddy_mepfix::printlinks::norm_sheet(&e.sheet) == sheet && inside(&e.reach, p) && !e.inner.iter().any(|r| inside(r, p))
            })?;
            let q = e.fit.apply(p);
            let lands_q = |m: &cadaddy_mepfix::tagid::Sim| lands(&e.fit.then(m));
            place(q, level, &lands_q).map(|m| e.fit.then(&m))
        })
    };
    // A point is placed once: the build asks again (to place it, to say why not, to stamp).
    let memo: std::sync::Mutex<std::collections::HashMap<(u64, u64, String), Option<cadaddy_mepfix::tagid::Sim>>> = Default::default();
    move |p: [f64; 2], level: &str| {
        let id = (p[0].to_bits(), p[1].to_bits(), level.to_string());
        if let Some(m) = memo.lock().ok().and_then(|c| c.get(&id).copied()) {
            return m;
        }
        let m = placed(p, level);
        if let Ok(mut c) = memo.lock()
            && c.len() < MAX_PLACER_MEMO
        {
            c.insert(id, m);
        }
        m
    }
}

/// The regions of the plan views (not elevations, sections, details, risers, diagrams,
/// schedules) on each of the sheets `rects`, each sheet read on its own.
fn sheet_plan_views(d: &cadaddy_doc::Drawing, rects: &[[f64; 4]]) -> Vec<[f64; 4]> {
    let inside = |r: &[f64; 4], x: f64, y: f64| x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3];
    let mut on: Vec<Vec<cadaddy_doc::Entity>> = vec![Vec::new(); rects.len()];
    for e in d.model.iter() {
        let b = cadaddy_doc::entity_bounds(d, e, 0);
        if b.is_empty() {
            continue;
        }
        let c = b.center();
        if let Some(i) = rects.iter().position(|r| inside(r, c.x, c.y)) {
            on[i].push((**e).clone());
        }
    }
    let not_plan = |t: &str| {
        let u = t.to_ascii_uppercase();
        !u.contains("PLAN") && ["ELEVATION", "SECTION", "DETAIL", "RISER", "DIAGRAM", "SCHEDULE", "ISOMETRIC"].iter().any(|w| u.contains(w))
    };
    let mut out = Vec::new();
    for mine in on.into_iter().filter(|m| !m.is_empty()) {
        let mut sub = d.clone();
        let mut store = cadaddy_doc::EntityStore::new();
        store.push_many(mine);
        sub.model = store;
        let texts = cadaddy_mepfix::sheet_facts::texts(&sub);
        out.extend(cadaddy_mepfix::sheet_facts::view_titles(&sub, &texts).into_iter().filter(|v| !not_plan(&v.title)).filter_map(|v| v.region));
    }
    out
}

/// Whether linework `view` is a part of a floor next to `plan` (an enlarged view of a few
/// rooms): its longer side under half the plan's.
fn part_of_plan(view: &[cadaddy_geom::Vec2], plan: &[cadaddy_geom::Vec2]) -> bool {
    let (v, p) = (cadaddy_geom::Bounds2::from_points(view.iter().copied()), cadaddy_geom::Bounds2::from_points(plan.iter().copied()));
    !v.is_empty() && !p.is_empty() && v.width().max(v.height()) < 0.5 * p.width().max(p.height())
}

/// Points a job's placer remembers at most.
const MAX_PLACER_MEMO: usize = 500_000;

/// Each storey's shell ([`super::wallgen::shells`]) the rooms stage's rooms confirm: one holding
/// fewer than [`SHELL_ROOMS`] of its level's rooms is walls the plan didn't close round the
/// building (a part of it), no shell. And the corridors, stairs and hoistways traced inside the
/// shells. With a progress line per level.
fn building_shells(
    s: &Session,
    sp: &cadaddy_walk3d::BuildingSpec,
    job: &GuidedJob,
) -> (Vec<(String, Vec<[cadaddy_geom::Vec2; 2]>)>, Vec<cadaddy_walk3d::Circulation>) {
    let Ok(d) = s.doc() else { return (Vec::new(), Vec::new()) };
    let started = web_time::Instant::now();
    let key = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let rooms: Vec<(String, [f64; 2])> = job
        .stage(StageName::Rooms)
        .and_then(|st| st.result.as_ref())
        .and_then(|r| r.get("rooms"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|r| r.get("other_trade").and_then(Value::as_bool) != Some(true))
        .filter_map(|r| Some((key(r.get("level")?.as_str()?), [r.get("at")?.get(0)?.as_f64()?, r.get("at")?.get(1)?.as_f64()?])))
        .collect();
    let storeys = super::wallgen::shells(d, sp);
    let mut untrusted = Vec::new();
    let mut circulation = Vec::new();
    let mut missed = Vec::new();
    let mut shells = Vec::new();
    for storey in storeys {
        let (l, rects) = (&storey.level, &storey.shell);
        // The level's rooms on its own plan (the rooms stage also reads its other sheets: a
        // schedule's cells, an elevation's notes).
        let region = sp.levels.iter().find(|x| x.name.eq_ignore_ascii_case(l)).and_then(|x| x.region);
        let on_plan = |at: &[f64; 2]| {
            region.is_some_and(|r| at[0] >= r[0].min(r[2]) && at[0] <= r[0].max(r[2]) && at[1] >= r[1].min(r[3]) && at[1] <= r[1].max(r[3]))
        };
        let mine: Vec<[f64; 2]> = rooms.iter().filter(|(k, at)| !k.is_empty() && *k == key(l) && on_plan(at)).map(|(_, at)| *at).collect();
        let (ok, held) = shell_holds_rooms(rects, &mine);
        if !ok {
            untrusted.push(format!("{l} (holds {held} of {} rooms)", mine.len()));
            continue;
        }
        circulation.extend(storey.circulation);
        if !storey.missed.is_empty() {
            missed.push(format!("{} {}", storey.level, storey.missed.join(", ")));
        }
        shells.push((storey.level, storey.shell));
    }
    if !untrusted.is_empty() {
        progress_line(&format!("  building shell not used (the walls don't close round the building): {}", untrusted.join(", ")));
    }
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let each: Vec<String> =
        shells.iter().map(|(l, r)| format!("{l} {:.0} sf", r.iter().map(|[a, b]| (b.x - a.x) * (b.y - a.y)).sum::<f64>() / (upf * upf))).collect();
    progress_line(&format!("  building shell (exterior walls) in {:.1} s: {}", started.elapsed().as_secs_f64(), each.join(", ")));
    let each: Vec<String> = circulation
        .iter()
        .map(|c| {
            let b = c.rects.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |m, [a, z]| {
                [m[0].min(a[0]), m[1].min(a[1]), m[2].max(z[0]), m[3].max(z[1])]
            });
            format!("{} {:?} '{}' {:.0} sf {:?}", c.level, c.kind, c.name, c.area() / (upf * upf), b.map(|v| v.round()))
        })
        .collect();
    progress_line(&format!("  circulation (corridors, stairs, hoistways) traced: {}: {}", circulation.len(), each.join("; ")));
    if !missed.is_empty() {
        progress_line(&format!("  circulation labels not traced: {}", missed.join("; ")));
    }
    (shells, circulation)
}

/// Removes the room elements whose middle is outside their level's shell (the levels stacked:
/// back to the level's own plan first); how many.
fn drop_rooms_outside_shell(s: &mut Session, shells: &[(String, Vec<[cadaddy_geom::Vec2; 2]>)]) -> Result<usize> {
    if shells.is_empty() {
        return Ok(0);
    }
    let d = s.doc()?;
    let offsets = super::bimlook::stack_offsets(d);
    let gone: Vec<cadaddy_doc::Handle> = super::bspec::room_elements(d)
        .iter()
        .filter(|r| {
            let Some((_, rects)) = shells.iter().find(|(l, _)| l.eq_ignore_ascii_case(&r.level)) else { return false };
            let b = cadaddy_geom::Bounds2::from_points(r.outline.iter().copied());
            if b.is_empty() {
                return false;
            }
            let off = offsets.iter().find(|(l, _)| l.eq_ignore_ascii_case(&r.level)).map_or(cadaddy_geom::Vec2::new(0.0, 0.0), |o| o.1);
            let c = b.center() - off;
            !rects.iter().any(|[a, z]| c.x >= a.x && c.x <= z.x && c.y >= a.y && c.y <= z.y)
        })
        .map(|r| r.handle)
        .collect();
    for h in &gone {
        s.doc_mut()?.remove_entity(*h);
    }
    Ok(gone.len())
}

/// A ceiling height tag of a reflected ceiling plan, feet: `8' - 0"`, `7'-6" A.F.F.`, `8'-6" AFF`,
/// between 6'-6" and 20' and nothing else in the text. Not a stair's `7'-6" ABOVE LANDING` (its
/// height is from the landing) or a dimension's note.
fn ceiling_height_tag(t: &str) -> Option<f64> {
    let u = t.trim().to_ascii_uppercase();
    if u.contains("ABOVE") {
        return None;
    }
    let core = u.trim_end_matches("A.F.F.").trim_end_matches("AFF").trim();
    if core.chars().any(|c| !(c.is_ascii_digit() || " '\"-/.".contains(c))) || !core.contains('\'') {
        return None;
    }
    let ft = super::perceive::feet_inches(core)? / 12.0;
    (6.5..=20.0).contains(&ft).then_some(ft)
}

/// A shell strip narrower than this is a sliver (a wall's thickness), feet.
const SHELL_SLIVER_FT: f64 = 2.0;

/// Rooms two storeys must share for one to be lined up on the other.
const MIN_REGISTER_ROOMS: usize = 10;
/// The most a storey is moved onto the one under it, feet (more is a different floor, not a
/// sheet drawn off).
const MAX_REGISTER_FT: f64 = 3.0;

/// B4-016: each storey with a base, from the second up, lined up on the storey under it by the
/// room numbers both print (`312` over `212`: the same room the floor below has, same number
/// after the level's digit): the middle of the paired rooms' shifts, where at least
/// [`MIN_REGISTER_ROOMS`] pair up and most of them agree within a foot, of at most
/// [`MAX_REGISTER_FT`]. The levels moved (and by how much), and why the others weren't.
fn register_storeys_by_rooms(d: &cadaddy_doc::Drawing, sp: &mut cadaddy_walk3d::BuildingSpec) -> (Vec<String>, Vec<String>) {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let numbers = super::identity::number_texts(d);
    let inside = |r: &[f64; 4], p: [f64; 2]| p[0] >= r[0].min(r[2]) && p[0] <= r[0].max(r[2]) && p[1] >= r[1].min(r[3]) && p[1] <= r[1].max(r[3]);
    let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    // A level's rooms by their number after its digit (`12` for `312` on Level 3).
    let rooms = |region: &[f64; 4], code: &str| -> Vec<(String, [f64; 2])> {
        numbers
            .iter()
            .filter(|(k, at)| k.len() >= 3 && k.starts_with(code) && inside(region, *at))
            .map(|(k, at)| (k[code.len()..].to_string(), *at))
            .collect()
    };
    let median = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(0.0)
    };
    let mut floors: Vec<usize> = (0..sp.levels.len())
        .filter(|i| {
            let l = &sp.levels[*i];
            l.sheet.is_none()
                && l.region.is_some()
                && l.base.is_some()
                && digits(&l.name).len() == 1
                && !super::wallgen::roof_level(&sp.levels, &l.name)
        })
        .collect();
    floors.sort_by(|a, b| sp.levels[*a].elevation_ft.total_cmp(&sp.levels[*b].elevation_ft));
    let (mut out, mut not) = (Vec::new(), Vec::new());
    for w in floors.windows(2) {
        let (lo, hi) = (&sp.levels[w[0]], &sp.levels[w[1]]);
        let (Some(rlo), Some(rhi), Some(blo), Some(bhi)) = (lo.region, hi.region, lo.base, hi.base) else { continue };
        let (mine, theirs) = (rooms(&rhi, &digits(&hi.name)), rooms(&rlo, &digits(&lo.name)));
        let (name, below) = (hi.name.clone(), lo.name.clone());
        // The rooms both print, paired by the fit (its scale near 1; a pair is the places nearest
        // each other after it).
        let Some(fit) = cadaddy_mepfix::tagid::fit_by_tags(&mine, &theirs, 2.0 * upf, (0.98, 1.02)) else {
            not.push(format!("{name} onto {below}: no fit ({} and {} room numbers)", mine.len(), theirs.len()));
            continue;
        };
        let mut pairs: Vec<[f64; 2]> = Vec::new();
        for k in &fit.matched {
            let best =
                mine.iter().filter(|(m, _)| m == k).flat_map(|(_, a)| theirs.iter().filter(|(t, _)| t == k).map(move |(_, b)| (*a, *b))).min_by(
                    |x, y| {
                        let dist = |(a, b): &([f64; 2], [f64; 2])| {
                            let q = fit.sim.apply(*a);
                            (q[0] - b[0]).hypot(q[1] - b[1])
                        };
                        dist(x).total_cmp(&dist(y))
                    },
                );
            if let Some((a, b)) = best {
                pairs.push([b[0] - a[0], b[1] - a[1]]);
            }
        }
        // A plain shift: the middle of the pairs' shifts, most of them within a foot of it.
        let t = [median(pairs.iter().map(|p| p[0]).collect()), median(pairs.iter().map(|p| p[1]).collect())];
        let agree = pairs.iter().filter(|p| (p[0] - t[0]).hypot(p[1] - t[1]) <= upf).count();
        if pairs.len() < MIN_REGISTER_ROOMS || (agree as f64) < 0.7 * pairs.len() as f64 {
            not.push(format!("{name} onto {below}: {} rooms paired, {agree} agree on a shift", pairs.len()));
            continue;
        }
        // Same room, same place in the building: p_hi - base_hi = p_lo - base_lo.
        let base = [blo[0] - t[0], blo[1] - t[1]];
        let moved = (base[0] - bhi[0]).hypot(base[1] - bhi[1]) / upf;
        if moved > MAX_REGISTER_FT {
            not.push(format!("{name} onto {below}: {moved:.1}' off, more than {MAX_REGISTER_FT}' (another floor plan)"));
            continue;
        }
        if moved <= 0.05 {
            continue;
        }
        let (dx, dy) = ((base[0] - bhi[0]) / upf, (base[1] - bhi[1]) / upf);
        sp.levels[w[1]].base = Some(base);
        out.push(format!("{name} by ({dx:.2}', {dy:.2}') onto {below} ({agree} of {} rooms)", pairs.len()));
    }
    (out, not)
}

/// How far under the next floor a ceiling is at most (the slab and a plenum's least), feet.
const CEILING_UNDER_FLOOR_FT: f64 = 0.5;

/// The ceiling areas the reflected ceiling plans' views give (a view titled REFLECTED CEILING or
/// RCP, its levels from its title: `SECOND-FOURTH FLOOR` is levels 2 to 4): each view registered
/// onto its level's floor plan by the room numbers both print (a typical floor's view by its
/// numbers with that floor's digit), each height tag's area the floor around it on the level's
/// floor grid (or the corridor, stair or hoistway it is on), at that height. And what was read.
fn rcp_ceiling_areas(
    d: &cadaddy_doc::Drawing,
    sp: &cadaddy_walk3d::BuildingSpec,
    sheets: &[[f64; 4]],
) -> (Vec<cadaddy_walk3d::CeilingArea>, Vec<String>) {
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let texts = cadaddy_mepfix::sheet_facts::texts(d);
    let views = cadaddy_mepfix::sheet_facts::view_titles(d, &texts);
    let numbers = super::identity::number_texts(d);
    let inside = |r: &[f64; 4], p: [f64; 2]| p[0] >= r[0].min(r[2]) && p[0] <= r[0].max(r[2]) && p[1] >= r[1].min(r[3]) && p[1] <= r[1].max(r[3]);
    let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let mut out: Vec<cadaddy_walk3d::CeilingArea> = Vec::new();
    let mut report = Vec::new();
    let mut floors: BTreeMap<String, Option<super::wallgen::Floors>> = BTreeMap::new();
    let is_rcp = |title: &str| {
        let t = title.to_ascii_uppercase();
        t.contains("REFLECTED CEILING") || t.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == "RCP")
    };
    let rcp_views: Vec<&cadaddy_mepfix::sheet_facts::ViewTitle> = views.iter().take(200).filter(|v| v.region.is_some() && is_rcp(&v.title)).collect();
    // A point's view: of the views whose block holds it, the one whose title is nearest under it
    // (a sheet of two stacked RCPs gives both the whole sheet as their block).
    let owner = |p: [f64; 2]| -> Option<usize> {
        rcp_views
            .iter()
            .enumerate()
            .filter(|(_, v)| v.region.is_some_and(|r| inside(&r, p)) && v.at[1] < p[1])
            .min_by(|a, b| (p[1] - a.1.at[1]).total_cmp(&(p[1] - b.1.at[1])))
            .map(|(i, _)| i)
    };
    for (vi, v) in rcp_views.iter().enumerate() {
        let Some(region) = v.region else { continue };
        let levels = cadaddy_mepfix::sheet_reader::detect_levels(&v.title);
        // On the ceiling plan's own sheet (a view's block can take in the sheets laid beside it).
        let on_sheet = |p: [f64; 2]| sheets.is_empty() || sheets.iter().any(|r| inside(r, p));
        let mine_only = |p: [f64; 2]| inside(&region, p) && on_sheet(p) && owner(p).is_none_or(|o| o == vi);
        let tags: Vec<(String, [f64; 2])> = numbers.iter().filter(|(_, at)| mine_only(*at)).cloned().collect();
        let heights: Vec<([f64; 2], f64)> =
            texts.iter().filter(|x| mine_only([x.at.x, x.at.y])).filter_map(|x| Some(([x.at.x, x.at.y], ceiling_height_tag(&x.value)?))).collect();
        for code in &levels {
            let Some(level) = sp.levels.iter().find(|l| digits(&l.name) == *code && l.sheet.is_none()) else { continue };
            let Some(plan) = level.region else { continue };
            // The view's room numbers as this floor prints them.
            let mine: Vec<(String, [f64; 2])> = tags
                .iter()
                .map(|(k, at)| {
                    let swapped = if k.len() >= 3 && k.chars().next().is_some_and(|c| c.is_ascii_digit()) && code.len() == 1 {
                        format!("{code}{}", &k[1..])
                    } else {
                        k.clone()
                    };
                    (swapped, *at)
                })
                .collect();
            let theirs: Vec<(String, [f64; 2])> = numbers.iter().filter(|(_, at)| inside(&plan, *at)).cloned().collect();
            let Some(fit) = cadaddy_mepfix::tagid::fit_by_tags(&mine, &theirs, 2.0 * upf, (0.9, 1.1)).filter(|f| f.matched.len() >= 4) else {
                report.push(format!("{} on {}: not registered (room numbers don't match the floor plan)", v.title, level.name));
                continue;
            };
            let grid = floors.entry(level.name.clone()).or_insert_with(|| super::wallgen::room_floors(d, sp, &level.name));
            let mut found = 0usize;
            // Where the other tags went: off the plan, in no room or corridor, in an area set.
            let (mut off, mut neither, mut again, mut above) = (0usize, 0usize, 0usize, 0usize);
            let mut landed = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
            for (at, ft) in &heights {
                let q = fit.sim.apply(*at);
                landed = [landed[0].min(q[0]), landed[1].min(q[1]), landed[2].max(q[0]), landed[3].max(q[1])];
                if !inside(&plan, q) {
                    off += 1;
                    continue;
                }
                // A ceiling at or over the next floor (less its slab) is none this storey can
                // have: a misread tag, or another level's.
                if level.height_ft.is_finite() && level.height_ft > 0.0 && *ft > level.height_ft - CEILING_UNDER_FLOOR_FT {
                    above += 1;
                    continue;
                }
                let p = cadaddy_geom::Vec2::new(q[0], q[1]);
                // The floor area round the tag, and its room: the circulation space's name, else the
                // room number printed inside it.
                let (rects, mut room): (Option<Vec<[[f64; 2]; 2]>>, Option<String>) = match grid.as_mut().map(|g| g.room_at(p)) {
                    Some(Ok((_, rects))) => (Some(rects.iter().map(|[a, b]| [[a.x, a.y], [b.x, b.y]]).collect()), None),
                    _ => sp
                        .circulation
                        .iter()
                        .find(|c| c.level.eq_ignore_ascii_case(&level.name) && c.holds(q))
                        .map_or((None, None), |c| (Some(c.rects.clone()), Some(c.name.clone()))),
                };
                let Some(rects) = rects else {
                    neither += 1;
                    continue;
                };
                let outline = cadaddy_walk3d::Circulation { rects, ..Default::default() }.outline();
                if outline.len() < 3 {
                    neither += 1;
                    continue;
                }
                // A second tag in an area already set: the first stands.
                if out.iter().any(|c| {
                    c.level == level.name
                        && cadaddy_geom::point_in_polygon(&c.outline.iter().map(|p| cadaddy_geom::Vec2::new(p[0], p[1])).collect::<Vec<_>>(), p)
                }) {
                    again += 1;
                    continue;
                }
                if room.is_none() {
                    let poly: Vec<cadaddy_geom::Vec2> = outline.iter().map(|p| cadaddy_geom::Vec2::new(p[0], p[1])).collect();
                    room = numbers
                        .iter()
                        .find(|(_, at)| inside(&plan, *at) && cadaddy_geom::point_in_polygon(&poly, cadaddy_geom::Vec2::new(at[0], at[1])))
                        .map(|(n, _)| n.clone());
                }
                out.push(cadaddy_walk3d::CeilingArea {
                    level: level.name.clone(),
                    room,
                    outline,
                    height_ft: *ft,
                    grid: None,
                    ceiling_type: None,
                    guess: None,
                    source: Some(format!("{} ceiling tag on {}", ft_in_label(*ft), v.title)),
                });
                found += 1;
            }
            // Tags off the plan: where the fit put them, against the plan, to see why.
            let why = if off > 0 {
                let r = |v: f64| v.round();
                format!(
                    " [fit scale {:.3}, shift ({}, {}); plan [{}, {}, {}, {}]; tags landed [{}, {}, {}, {}]]",
                    fit.sim.a.hypot(fit.sim.b),
                    r(fit.sim.tx),
                    r(fit.sim.ty),
                    r(plan[0]),
                    r(plan[1]),
                    r(plan[2]),
                    r(plan[3]),
                    r(landed[0]),
                    r(landed[1]),
                    r(landed[2]),
                    r(landed[3])
                )
            } else {
                String::new()
            };
            report.push(format!(
                "{} on {}: {found} ceiling areas from {} height tags ({} room numbers matched; {again} more in an area already set, {neither} in no room or corridor, {off} off the plan, {above} at or over the next floor){why}",
                v.title,
                level.name,
                heights.len(),
                fit.matched.len()
            ));
        }
    }
    (out, report)
}

/// How far outside its storey's shell (feet) a run, terminal, piece of equipment or device may
/// land and still be in the building.
const OUTSIDE_SHELL_FT: f64 = 5.0;

/// Whether `p` lands more than [`OUTSIDE_SHELL_FT`] outside its level's shell (by the level's
/// digits). A level with no shell: never.
fn outside_shell(shells: &[(String, Vec<[cadaddy_geom::Vec2; 2]>)], level: &str, p: [f64; 2], upf: f64) -> bool {
    let key = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let k = key(level);
    let m = OUTSIDE_SHELL_FT * upf;
    let mut mine = shells.iter().filter(|(l, r)| !k.is_empty() && key(l) == k && !r.is_empty()).peekable();
    mine.peek().is_some()
        && !mine.any(|(_, rects)| {
            rects.iter().any(|[a, b]| p[0] >= a.x.min(b.x) - m && p[0] <= a.x.max(b.x) + m && p[1] >= a.y.min(b.y) - m && p[1] <= a.y.max(b.y) + m)
        })
}

/// A sheet title naming a wall type (partition) legend: `WALL AND FLOOR TYPES`, `PARTITION TYPES`.
fn wall_legend_title(title: &str) -> bool {
    let t = title.to_ascii_uppercase();
    t.contains("PARTITION") || (t.contains("WALL") && t.contains("TYPE"))
}

/// The partition legend sheets' wall types into the spec, and the walls each level's plan tags
/// type with them (a tag that is a legend mark types the wall nearest it and the walls on its
/// line): how every wall is built, so a box on it gets its mud ring. What was read.
fn apply_wall_legend(s: &Session, sp: &mut cadaddy_walk3d::BuildingSpec, job: &GuidedJob) -> Result<Vec<String>> {
    let d = s.doc()?;
    let mut report = Vec::new();
    let mut types: Vec<cadaddy_walk3d::WallType> = Vec::new();
    for r in job.sheet_regions.iter().filter(|r| r.kind == "legend") {
        let lg = super::walltypes::read_legend(d, Some(r.rect), &r.number);
        report.push(format!(
            "{}: {} wall types{}",
            r.number,
            lg.types.len(),
            if lg.report.is_empty() { String::new() } else { format!(" ({})", lg.report.join("; ")) }
        ));
        for t in lg.types {
            if !types.iter().any(|x| x.name.eq_ignore_ascii_case(&t.name)) {
                types.push(t);
            }
        }
    }
    // A type the spec refuses (a layer it can't hold) is left out and said: one legend entry
    // never stops the build.
    let refused: Vec<String> = types
        .iter()
        .filter(|t| {
            let alone = cadaddy_walk3d::BuildingSpec { wall_types: vec![(*t).clone()], ..Default::default() };
            alone.check().iter().any(|i| i.error)
        })
        .map(|t| t.name.clone())
        .collect();
    if !refused.is_empty() {
        report.push(format!("not used (the spec can't hold them): {}", refused.join(", ")));
        types.retain(|t| !refused.contains(&t.name));
    }
    if types.is_empty() {
        return Ok(report);
    }
    sp.wall_types.retain(|w| !types.iter().any(|t| t.name.eq_ignore_ascii_case(&w.name)));
    sp.wall_types.extend(types.iter().cloned());
    let marks: BTreeSet<String> = types.iter().map(|t| t.name.to_ascii_uppercase()).collect();
    let upf = if sp.units_per_foot.is_finite() && sp.units_per_foot > 0.0 { sp.units_per_foot } else { 12.0 };
    let texts = cadaddy_mepfix::sheet_facts::texts(d);
    let inside =
        |r: &[f64; 4], p: cadaddy_geom::Vec2| p.x >= r[0].min(r[2]) && p.x <= r[0].max(r[2]) && p.y >= r[1].min(r[3]) && p.y <= r[1].max(r[3]);
    let levels: Vec<(String, [f64; 4])> = sp
        .levels
        .iter()
        .filter(|l| l.sheet.is_none() && !super::wallgen::roof_level(&sp.levels, &l.name))
        .filter_map(|l| Some((l.name.clone(), l.region?)))
        .collect();
    // The unit plans' tags, put into every room of their type.
    let stamped = typical_wall_tags(d, job, &marks);
    let digits = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let mut typed = Vec::new();
    for (level, region) in levels {
        let mut tags: Vec<(String, cadaddy_geom::Vec2)> = texts
            .iter()
            .filter(|t| inside(&region, t.at))
            .filter_map(|t| {
                let v = t.value.trim().to_ascii_uppercase();
                marks.contains(&v).then_some((v, t.at))
            })
            .collect();
        let own = tags.len();
        tags.extend(stamped.get(&digits(&level)).into_iter().flatten().cloned());
        if tags.is_empty() {
            continue;
        }
        let walls = super::wallgen::planned_walls(d, sp, &level);
        let sheet = job
            .sheet_regions
            .iter()
            .find(|r| r.kind == "plan" && r.rect.iter().zip(region).all(|(a, b)| (a - b).abs() < 1e-6 * upf.max(1.0)))
            .map_or_else(|| format!("{level} plan"), |r| r.number.clone());
        let (new, lost) = super::walltypes::plan_marks(&tags, &walls, &level, upf, &sheet);
        // The walls no tag reached: the type the tagged walls of their thickness carry (a guess).
        let guessed = super::walltypes::guess_marks(&walls, &new, &level, upf, &sheet);
        typed.push(format!(
            "{level} {} walls from {own} plan tags + {} unit-plan tags, {} more guessed by thickness{}",
            new.len(),
            tags.len() - own,
            guessed.len(),
            if lost > 0 { format!(" ({lost} tags near no wall)") } else { String::new() }
        ));
        sp.walls.retain(|m| !m.level.eq_ignore_ascii_case(&level));
        sp.walls.extend(new);
        sp.walls.extend(guessed);
    }
    report.push(format!("walls typed by the plans' tags: {}", if typed.is_empty() { "none".to_string() } else { typed.join(", ") }));
    Ok(report)
}

/// How far inside a hoistway's walls (feet) a run must pass to go through it: one along the
/// face of its wall runs beside it.
const HOISTWAY_CLEAR_FT: f64 = 0.25;

/// The MEP runs (indices) that pass through an elevator hoistway on their level.
fn runs_through_hoistways(runs: &[cadaddy_walk3d::MepRun], circulation: &[cadaddy_walk3d::Circulation], upf: f64) -> Vec<usize> {
    runs.iter().enumerate().filter(|(_, r)| hoistway_crossed(r, circulation, upf).is_some()).map(|(i, _)| i).collect()
}

/// The elevator hoistway of its level (by the level's digits) a run passes through, if any.
pub(crate) fn hoistway_crossed<'a>(
    run: &cadaddy_walk3d::MepRun,
    circulation: &'a [cadaddy_walk3d::Circulation],
    upf: f64,
) -> Option<&'a cadaddy_walk3d::Circulation> {
    let key = |l: &str| l.chars().filter(char::is_ascii_digit).collect::<String>();
    let inset = HOISTWAY_CLEAR_FT * upf;
    let level = key(&run.level);
    circulation.iter().filter(|c| c.kind == cadaddy_walk3d::CirculationKind::Hoistway && !level.is_empty() && key(&c.level) == level).find(|c| {
        c.rects
            .iter()
            .map(|[a, b]| [a[0] + inset, a[1] + inset, b[0] - inset, b[1] - inset])
            .filter(|r| r[2] > r[0] && r[3] > r[1])
            .any(|rect| run.points.windows(2).any(|w| crosses_rect(w[0], w[1], &rect)))
    })
}

/// Whether the segment a–b runs through the rectangle `[x0, y0, x1, y1]` (Liang–Barsky): some
/// length of it inside.
fn crosses_rect(a: [f64; 2], b: [f64; 2], r: &[f64; 4]) -> bool {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-d[0], a[0] - r[0]), (d[0], r[2] - a[0]), (-d[1], a[1] - r[1]), (d[1], r[3] - a[1])] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else if p < 0.0 {
            t0 = t0.max(q / p);
        } else {
            t1 = t1.min(q / p);
        }
    }
    t0 < t1
}

/// The share of its level's rooms a shell must hold to be the building's.
const SHELL_ROOMS: f64 = 0.8;

/// Whether a shell (rectangles) holds [`SHELL_ROOMS`] of its level's rooms (their label points),
/// and how many it holds. A level with no rooms read takes its shell as it is.
fn shell_holds_rooms(rects: &[[cadaddy_geom::Vec2; 2]], rooms: &[[f64; 2]]) -> (bool, usize) {
    let held = rooms.iter().filter(|at| rects.iter().any(|[a, b]| at[0] >= a.x && at[0] <= b.x && at[1] >= a.y && at[1] <= b.y)).count();
    (rooms.is_empty() || held as f64 >= SHELL_ROOMS * rooms.len() as f64, held)
}

/// How far outside its level's shell (feet) a placed point may still be: what is mounted on the
/// building's outside walls or stands by them.
const SHELL_REACH_FT: f64 = 5.0;
pub(crate) fn add_sheet_links(sp: &mut cadaddy_walk3d::BuildingSpec, regions: &[SheetRegion]) {
    for r in regions.iter().take(MAX_SHEET_LINKS) {
        // A ceiling plan or a sheet of details is no level's plan: the sheets are registered onto
        // the floor plans without it (it prints every room's number, and its level took it for
        // its reference); the ceilings stage registers it itself.
        if !r.rect.iter().all(|v| v.is_finite()) || matches!(r.kind.as_str(), "rcp" | "detail" | "legend") {
            continue;
        }
        let level = Some(r.level.trim()).filter(|l| super::printmep::building_level(l)).map(level_name);
        let level = level.map(|n| sp.level(&n).map_or(n.clone(), |l| l.name.clone()));
        // The views of a several-view sheet: one link each, on its own level.
        let same = |x: &cadaddy_walk3d::levels::SheetLink| {
            x.number.eq_ignore_ascii_case(&r.number)
                && x.rect.is_none_or(|q| q == r.rect)
                && (r.view.is_none() || x.view.is_none() || x.view == r.view)
        };
        match sp.sheets.iter_mut().find(|x| same(x)) {
            Some(x) => {
                x.rect = Some(r.rect);
                if x.level.is_none() || (r.view.is_some() && x.view.is_none()) {
                    x.level = level;
                }
                if r.view.is_some() {
                    x.view.clone_from(&r.view);
                    x.levels = r.levels.iter().map(|c| level_name(c)).collect();
                }
            }
            None => sp.sheets.push(cadaddy_walk3d::levels::SheetLink {
                number: r.number.clone(),
                title: String::new(),
                discipline: Some(r.discipline.clone()).filter(|d| !d.is_empty()),
                level,
                source: "model".into(),
                rect: Some(r.rect),
                levels: r.levels.iter().map(|c| level_name(c)).collect(),
                registration: None,
                view: r.view.clone(),
            }),
        }
    }
}

fn building_levels(job: &GuidedJob) -> Vec<Value> {
    // Site, civil and landscape plans, and plans that name no level, are no storey (MUST 3).
    let rank = |l: &str| (cadaddy_walk3d::levels::code_rank(l).unwrap_or(i32::MAX - 1), l.to_string());
    let floor_plan = |r: &&SheetRegion| r.kind == "plan" && super::printmep::building_level(&r.level) && r.not_built.is_empty();
    let mut plans: Vec<&SheetRegion> = job.sheet_regions.iter().filter(floor_plan).filter(|r| r.discipline == "A").collect();
    if plans.is_empty() {
        // No architectural floor plans: one discipline's plans give the floors (their
        // background shows the walls), the one with the most levels.
        // Q6037: exclude disciplines that don't carry building walls/doors (life safety,
        // fire protection, fire alarm, general/civil, structural, telecom).
        const NON_BUILDING: &[&str] = &["SL", "FP", "FA", "G", "C", "S", "T"];
        let building_plan = |r: &&SheetRegion| floor_plan(r) && !NON_BUILDING.contains(&r.discipline.as_str());
        let mut by: BTreeMap<&str, std::collections::BTreeSet<&str>> = BTreeMap::new();
        for r in job.sheet_regions.iter().filter(|r| building_plan(r)) {
            by.entry(r.discipline.as_str()).or_default().insert(r.level.as_str());
        }
        let best = by.iter().filter(|(_, l)| l.len() > 1).max_by_key(|(d, l)| (l.len(), std::cmp::Reverse(*d))).map(|(d, _)| (*d).to_string());
        if let Some(d) = best {
            plans = job.sheet_regions.iter().filter(|r| building_plan(r)).filter(|r| r.discipline == d).collect();
        }
    }
    plans.sort_by_key(|r| rank(&r.level));
    // Dedup by canonical level name so aliases (R/RF/ROOF/T.O.) merge into one level.
    plans.dedup_by(|a, b| level_name(&a.level).eq_ignore_ascii_case(&level_name(&b.level)));
    let index_levels: Vec<Value> = job.index.as_ref().and_then(|i| i.get("levels")).and_then(Value::as_array).cloned().unwrap_or_default();
    let read = |level: &str, key: &str| {
        index_levels
            .iter()
            .find(|l| l.get("name").and_then(Value::as_str).is_some_and(|n| n.eq_ignore_ascii_case(level)))
            .and_then(|l| l.get(key))
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite())
    };
    // Elevations rise with the level: the readings that break that (a site spot elevation
    // taken for a floor) are no readings; the most that agree win.
    let reads: Vec<Option<f64>> = plans.iter().map(|r| read(&r.level, "elevation_ft")).collect();
    let items: Vec<(Option<i32>, f64, u32)> = plans.iter().zip(&reads).map(|(r, e)| (Some(rank(&r.level).0), e.unwrap_or(f64::NAN), 1)).collect();
    let in_order = cadaddy_walk3d::levels::in_order(&items);
    // An elevation the user gave is the level's, in order or not.
    let given: Vec<Option<f64>> = plans.iter().map(|r| answered_level(job, &r.level).and_then(|a| a.elevation_ft)).collect();
    let read_good = |i: usize| reads.get(i).copied().flatten().filter(|_| in_order.get(i).copied().unwrap_or(false));
    let good = |i: usize| given.get(i).copied().flatten().or_else(|| read_good(i));
    let mut out = Vec::new();
    let mut next_elevation = 0.0f64;
    for (i, r) in plans.iter().enumerate() {
        let name = level_name(&r.level);
        let read_el = reads.get(i).copied().flatten();
        let out_of_order = read_el.is_some() && good(i).is_none();
        if i == 0 {
            // The lowest with no good reading: a storey under the first level read, if any (an
            // elevation the user gave a level above doesn't move it off 0).
            next_elevation = (1..plans.len()).find_map(|j| read_good(j).map(|e| e - DEFAULT_STOREY_FT * j as f64)).unwrap_or(0.0);
        }
        let (elevation, read_elevation) = match good(i) {
            Some(e) => (e, true),
            None => (next_elevation, false),
        };
        // A floor to floor given as an answer is the building's.
        // The user put the level above at an elevation: this one reaches it.
        let storey = |h: &f64| *h > 0.0 && *h <= 100.0;
        let to_given = given.get(i + 1).copied().flatten().map(|up| up - elevation).filter(storey);
        let (height, read_height) = match to_given.or_else(|| job.storey_ft.or_else(|| read(&r.level, "height_ft")).filter(storey)) {
            Some(h) => (h, true),
            None => (DEFAULT_STOREY_FT, false),
        };
        next_elevation = elevation + height;
        let mut level = json!({"name": name, "elevation_ft": elevation, "height_ft": height, "ceiling_ft": (height - 2.0).clamp(7.0, 9.0), "region": r.plan_rect()});
        if let (true, Some(e)) = (out_of_order, read_el) {
            level["guess"] = json!(format!(
                "{} plan {}: the elevation read for it, {}, breaks the levels' order, so it is not used; stacked at {}",
                name,
                r.number,
                cadaddy_walk3d::feet(e),
                cadaddy_walk3d::feet(elevation)
            ));
        } else if !(read_height && (read_elevation || job.storey_ft.is_some())) {
            // Stacked at a floor to floor the user gave is no guess either.
            level["guess"] = json!(format!("{} plan {}: floor to floor not read from the sections; {}' assumed", name, r.number, DEFAULT_STOREY_FT));
        }
        out.push(level);
    }
    out
}

/// The job's rooms after the build (MUST 2: a room is a label inside a closed boundary of real
/// walls): the rooms stage's reading (each label traced inside its closed outline on its sheet,
/// the title block, border and other sheets' notes left out), and the rooms the model traced
/// inside a level's walls that the reading has not: neither its number (on any level: the
/// model's tag can be read off another floor's sheet) nor its name on its level. The rooms
/// stage's result takes them, its reading's count kept as `read_count`. The model's rooms per
/// level.
fn rooms_from_model(s: &mut Session, job: &mut GuidedJob) -> Option<Value> {
    let model = super::rooms3d::model_rooms(s).ok().flatten().filter(|l| !l.is_empty())?;
    let text = |r: &Value, k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").trim().to_ascii_uppercase();
    let mut by_level: BTreeMap<String, usize> = BTreeMap::new();
    for r in &model {
        *by_level.entry(r.get("level").and_then(Value::as_str).unwrap_or("").to_string()).or_default() += 1;
    }
    let mut read: Vec<Value> = job
        .stage(StageName::Rooms)
        .and_then(|st| st.result.as_ref())
        .and_then(|r| r.get("rooms"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // A re-run of the build: the reading is the one before the last build added to it.
    read.retain(|r| r.get("from_model").and_then(Value::as_bool) != Some(true));
    let read_count = read.len();
    let numbers: BTreeSet<String> = read.iter().map(|r| text(r, "number")).filter(|n| !n.is_empty()).collect();
    let mut names: BTreeSet<(String, String)> = read.iter().map(|r| (level_name(&text(r, "level")).to_ascii_uppercase(), text(r, "name"))).collect();
    let added: Vec<Value> = model
        .into_iter()
        .filter_map(|mut m| {
            let (number, name) = (text(&m, "number"), m.get("name").and_then(Value::as_str).unwrap_or("").to_string());
            // A room known only by a one- or two-character number ("Room 01", no name of its own) is
            // a finish or keynote tag traced in a room.
            let bare_short = name.trim().eq_ignore_ascii_case(&format!("Room {number}")) && number.trim().chars().count() <= 2;
            let room = legible_room_name(&name)
                && super::autospec::room_label(&name)
                && !name.trim().eq_ignore_ascii_case("ROOM")
                && !finish_tag(&m)
                && !bare_short;
            let new = (number.is_empty() || !numbers.contains(&number)) && names.insert((text(&m, "level"), name.trim().to_ascii_uppercase()));
            if let Some(o) = m.as_object_mut() {
                o.insert("from_model".into(), json!(true));
            }
            (room && new).then_some(m)
        })
        .collect();
    let from_model = added.len();
    let mut rooms = json!({"rooms": read.into_iter().chain(added).collect::<Vec<_>>()});
    let (removed, renamed) = apply_room_answers(job, &mut rooms);
    let list = rooms.get_mut("rooms").map(Value::take).unwrap_or_else(|| json!([]));
    let count = list.as_array().map_or(0, Vec::len);
    if let Some(o) = job.stage_mut(StageName::Rooms).and_then(|st| st.result.as_mut()).and_then(Value::as_object_mut) {
        o.insert("read_count".into(), json!(read_count));
        o.insert("from_model".into(), json!(from_model));
        o.insert("rooms".into(), list);
        o.insert("count".into(), json!(count));
        if removed > 0 {
            o.insert("removed_by_answers".into(), json!(removed));
        }
        if renamed > 0 {
            o.insert("renamed_by_answers".into(), json!(renamed));
        }
    }
    job.audit(
        "rooms_from_model",
        "pipeline",
        None,
        Some(StageName::Rooms),
        Some(json!({"read": read_count, "from_model": from_model, "rooms": count})),
    );
    Some(json!(by_level))
}

/// The building spec drafted and stored from the open print set, as the job's build stage drafts
/// it: the set classified (when it isn't yet), its current plan sheets imported side by side and
/// one level per architectural floor plan. For `bim.build` when there is no spec yet (Q5276: an
/// import followed by a build must work without a guided job).
pub(crate) fn draft_from_prints(s: &mut Session) -> Result<Value> {
    let project = super::printset::get_project_handle(s)?;
    if project.sheets.iter().all(|e| e.recognised.is_none()) {
        s.execute("printset.classify", &json!({}))?;
    }
    let project = super::printset::get_project_handle(s)?;
    let index = cadaddy_mepfix::printset::sheet_index(&project);
    if index.sheets.is_empty() {
        return Err(bad(
            "bim.build",
            format!("no sheets were found in {} — check the folder holds the prints (PDF, DXF or DWG)", project.source_dir),
        ));
    }
    let mut job = GuidedJob::new(&project.source_dir, JobGoal::Bim);
    job.sheets = Some(s.execute("printset.sheets", &json!({"current": true}))?);
    job.index = Some(json!({"sheets": index.sheets, "levels": index.levels}));
    ensure_sheets_loaded(s, &mut job)?;
    let levels = building_levels(&job);
    let params = if levels.is_empty() { json!({"store": true}) } else { json!({"store": true, "levels": levels, "stack": true}) };
    s.execute("bspec.draft", &params)
}

/// The plan sheets the job built from, `{sheet, level, discipline, type}` each (`bim.from_pdf`).
pub(crate) fn job_plan_pages(s: &Session) -> Vec<Value> {
    load_job(s).ok().flatten().map_or_else(Vec::new, |job| {
        job.sheet_regions.iter().map(|r| json!({"sheet": r.number, "level": r.level, "discipline": r.discipline, "type": r.kind})).collect()
    })
}

/// Run takeoff stage — calls the model takeoff (by level and trade), not the 2D layer count.
/// The 2D layer count remains available as an explicit `takeoff.layers` tool.
fn run_takeoff(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    ensure_sheets_loaded(s, job)?;
    let mut result = s.execute("bim.takeoff", &json!({"group_by": "level"}))?;

    // Collect the trades and levels reported by the model takeoff.
    let lines_arr = result.get("lines").and_then(Value::as_array);
    let line_count = lines_arr.map_or(0, Vec::len);
    let trades: std::collections::BTreeSet<String> =
        lines_arr.into_iter().flat_map(|a| a.iter()).filter_map(|l| l["trade"].as_str().filter(|t| !t.is_empty()).map(str::to_string)).collect();
    let levels: Vec<Value> = result.get("levels").and_then(Value::as_array).cloned().unwrap_or_default();

    result["summary"] = json!({
        "line_count": line_count,
        "trades": trades,
        "levels": levels,
    });
    // Conduit, wire, boxes and fittings by level and circuit (Q6007).
    let by_circuit = s
        .execute("bim.takeoff", &json!({"group_by": "circuit", "filter": {"trade": "electrical"}, "keep": false}))
        .ok()
        .and_then(|t| t.get("lines").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let (circuits, conduit_ft, wire_ft) = circuit_summary(&by_circuit);
    result["summary"]["conduit_ft"] = json!(super::printmep::r1(conduit_ft));
    result["summary"]["wire_ft"] = json!(super::printmep::r1(wire_ft));
    result["circuits"] = json!(circuits);
    if !job.circuits.is_empty() && conduit_ft <= 0.0 {
        job.add_finding(
            StageName::Takeoff,
            "The circuits read have no conduit in the takeoff",
            "the electrical plans gave circuits but the model's conduit came to 0 ft",
            3,
            vec!["Re-run the mep_read and bim_build stages".into()],
            None,
            false,
        );
    }

    // MUST 4: takeoff with 0 lines is a finding, never silently Done.
    if line_count == 0 {
        job.add_finding(
            StageName::Takeoff,
            "Takeoff produced 0 line items",
            "A takeoff with no line items indicates that earlier stages (rooms, devices, BIM) \
             may not have detected enough data to generate quantities",
            4,
            vec!["Review rooms and devices stages for 0-count findings".into(), "Check that BIM build produced elements".into()],
            None,
            false,
        );
    }

    job.audit("stage_run", "pipeline", None, Some(StageName::Takeoff), Some(json!({"line_count": line_count, "trades": &trades, "levels": &levels})));
    Ok(result)
}

/// The takeoff's electrical lines (grouped by circuit) per level and circuit: conduit feet,
/// wire feet by size, boxes and fittings; and the conduit and wire totals (every line's).
fn circuit_summary(lines: &[Value]) -> (Vec<Value>, f64, f64) {
    let mut by: BTreeMap<(String, String), Value> = BTreeMap::new();
    let (mut conduit, mut wire) = (0.0, 0.0);
    for l in lines {
        let (cat, qty) = (l["category"].as_str().unwrap_or_default(), l["qty"].as_f64().unwrap_or(0.0));
        match cat {
            "conduit" => conduit += qty,
            "wire" => wire += qty,
            _ => {}
        }
        let Some(c) = l["circuit"].as_str().filter(|c| !c.is_empty()) else { continue };
        let level = l["level"].as_str().unwrap_or_default().to_string();
        let e = by.entry((level.clone(), c.to_string())).or_insert_with(
            || json!({"level": level, "circuit": c, "conduit_ft": 0.0, "wire_ft": {}, "boxes": 0.0, "fittings": 0.0, "devices": 0.0}),
        );
        let add = |v: &Value| json!(super::printmep::r1(v.as_f64().unwrap_or(0.0) + qty));
        match cat {
            "conduit" => e["conduit_ft"] = add(&e["conduit_ft"]),
            "wire" => {
                let size = l["size"].as_str().unwrap_or_default().to_string();
                e["wire_ft"][&size] = add(&e["wire_ft"][&size]);
            }
            "boxes" => e["boxes"] = add(&e["boxes"]),
            "conduit fittings" => e["fittings"] = add(&e["fittings"]),
            "devices" | "fixtures" => e["devices"] = add(&e["devices"]),
            _ => {}
        }
    }
    (by.into_values().collect(), conduit, wire)
}

/// Run estimate stage.
fn run_estimate(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    let result = s.execute("estimate.status", &json!({}))?;
    job.audit("stage_run", "pipeline", None, Some(StageName::Estimate), None);
    Ok(result)
}

/// Run clash detection stage; for the `sequence` goal, the construction sequence after it.
fn run_clash(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    // Clash detection through BIM validation.
    let result = s.execute("validate.clash", &json!({}));
    let mut out = match result {
        Ok(v) => {
            job.audit("stage_run", "pipeline", None, Some(StageName::Clash), None);
            v
        }
        Err(e) => {
            // Clash detection may not be available.
            job.audit("stage_error", "pipeline", None, Some(StageName::Clash), Some(json!({"error": e.to_string()})));
            json!({"error": e.to_string(), "note": "Clash detection not available; skipping."})
        }
    };
    if job.goal == JobGoal::Sequence {
        let seq = s.execute("schedule.build", &json!({}))?;
        let conflicts = s.execute("schedule.conflicts", &json!({})).unwrap_or_else(|e| json!({"error": e.to_string()}));
        job.audit("stage_run", "pipeline", None, Some(StageName::Clash), Some(json!({"sequence": seq["activities"], "span": seq["span"]})));
        let mut summary = seq;
        if let Some(m) = summary.as_object_mut() {
            m.remove("list");
        }
        if !out.is_object() {
            out = json!({"clash": out});
        }
        out["sequence"] = summary;
        out["sequence_conflicts"] = json!({"by_kind": conflicts["by_kind"], "ok": conflicts["ok"]});
    }
    Ok(out)
}

/// Q6021 MUST 1: a default is plausible only when the thing it confirms is real.
/// Rooms: area > 0 and a room-like name (not a material, height label, or note fragment).
/// Devices: a known type (unknown symbols with default "skip" are not plausible).
/// When there is no evidence at all, assume plausible (the question was asked for a reason).
fn is_plausible_default(q: &Question) -> bool {
    match (q.stage, &q.kind) {
        // Room verify: "name and area right?" — only when the evidence has values with
        // area_sqft.  A room at 0 sf or with a junk name is implausible; no evidence means
        // the question predates the guard and keeps its default.
        (StageName::Rooms, QuestionKind::Verify) => {
            let vals = match q.evidence.as_ref().and_then(|e| e.values.as_ref()) {
                Some(v) => v,
                None => return true, // no evidence → assume plausible
            };
            // Only gate rooms whose evidence carries area_sqft (the room-verify pattern).
            let Some(area) = vals.get("area_sqft").and_then(Value::as_f64) else {
                return true;
            };
            if area <= 0.0 {
                return false;
            }
            let name = vals.get("room").or_else(|| vals.get("room_name")).and_then(Value::as_str).unwrap_or("");
            is_room_like_name(name)
        }
        // Device unknown-symbol questions default "skip": not plausible (unknown type).
        (StageName::Devices, QuestionKind::Completeness) if q.default.as_deref() == Some("skip") => false,
        _ => true,
    }
}

/// Names that look like rooms (not junk labels read as rooms).
fn is_room_like_name(name: &str) -> bool {
    let upper = name.trim().to_uppercase();
    if upper.is_empty() {
        return false;
    }
    // A room number alone (digits, possibly with a letter suffix) is fine.
    if upper.chars().all(|c| c.is_ascii_alphanumeric()) && upper.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    // Reject names that are clearly not rooms: building materials, notes, single words that are
    // structural/finish labels, or very short fragments.
    const NOT_ROOMS: &[&str] = &[
        "WALL",
        "ROOF",
        "ROOFING",
        "CEILING",
        "FLOOR",
        "SLAB",
        "HEIGHT",
        "APPLICABLE",
        "TYP",
        "TYPICAL",
        "NIC",
        "N.I.C",
        "EXIST",
        "EXISTING",
        "SIM",
        "SIMILAR",
        "ABV",
        "ABOVE",
        "BLW",
        "BELOW",
        "TPO",
        "EPDM",
        "CMU",
        "GYP",
        "ACT",
        "VCT",
        "CPT",
        "CONC",
        "CLG",
        "FLR",
        "DEMO",
        "NEW",
        "FINISH",
        "PAINT",
        "BASE",
    ];
    for bad_name in NOT_ROOMS {
        // Exact match or the entire name is one of these words (possibly with punctuation).
        let cleaned: String = upper.chars().filter(|c| c.is_ascii_alphanumeric() || *c == ' ').collect();
        if cleaned.split_whitespace().all(|w| NOT_ROOMS.contains(&w)) {
            return false;
        }
        if cleaned == *bad_name {
            return false;
        }
    }
    // Very short single-word names without a number are suspect.
    if !upper.contains(' ') && upper.len() <= 3 && !upper.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    true
}

/// Take the defaults of the questions the job doesn't ask: in auto mode every question with a
/// default; else only the verify samples after the first [`VERIFY_ASKED`] (a spot check). A
/// question in [`GuidedJob::always_ask`] is never taken by default.
///
/// Q6021 MUST 1: in auto mode, implausible defaults are **rejected** (not confirmed): the
/// answer is `"Rejected"` with `answered_by = "auto_guard"`, and the question is listed in
/// the report.  How many were answered (accepted + rejected).
fn auto_answer(job: &mut GuidedJob, name: StageName) -> usize {
    let (auto, always) = (job.auto, std::mem::take(&mut job.always_ask));
    let mut answered = Vec::new();
    if let Some(st) = job.stage_mut(name) {
        let asked = st.questions.iter_mut().chain(st.completeness.iter_mut()).map(|q| (q, !auto));
        let samples = st.verify.iter_mut().enumerate().map(|(i, q)| (q, !auto && i < VERIFY_ASKED));
        for (q, asked) in asked.chain(samples) {
            if q.answer.is_none() && !asked && !always.contains(&q.id) && q.default.is_some() {
                if is_plausible_default(q) {
                    let def = q.default.clone().unwrap_or_default();
                    q.answer = Some(Value::String(def));
                    q.answered_by = Some("default".into());
                } else {
                    q.answer = Some(Value::String("Rejected".into()));
                    q.answered_by = Some("auto_guard".into());
                }
                answered.push(q.id.clone());
            }
        }
    }
    job.always_ask = always;
    sync_worklist(job);
    answered.len()
}

/// Every question is on the worklist (the app's palette answers from it), resolved once answered
/// (by default too: Q5278 MUST 4, the worklist agrees with the open questions).
pub fn sync_worklist(job: &mut GuidedJob) {
    let mut at: std::collections::HashMap<String, usize> = job.worklist.iter().enumerate().map(|(i, w)| (w.id.clone(), i)).collect();
    let questions: Vec<Question> =
        job.stages.iter().flat_map(|st| st.questions.iter().chain(st.verify.iter()).chain(st.completeness.iter())).cloned().collect();
    for q in &questions {
        match at.get(&q.id).and_then(|&i| job.worklist.get_mut(i)) {
            Some(w) => {
                if let Some(a) = &q.answer
                    && w.status != "resolved"
                {
                    w.status = "resolved".into();
                    w.resolution = Some(a.clone());
                }
            }
            None => {
                job.add_question_to_worklist(q);
                at.insert(q.id.clone(), job.worklist.len().saturating_sub(1));
            }
        }
    }
}

/// How the job's questions were answered: (by default, by the agent or user, rejected, open).
fn answer_counts(job: &GuidedJob) -> (usize, usize, usize, usize) {
    let (mut automatic, mut by_you, mut rejected, mut open) = (0, 0, 0, 0);
    for q in job.stages.iter().flat_map(|st| st.questions.iter().chain(st.verify.iter()).chain(st.completeness.iter())) {
        match (&q.answer, q.answered_by.as_deref()) {
            (None, _) => open += 1,
            (Some(_), Some("default")) => automatic += 1,
            (Some(_), Some("auto_guard")) => rejected += 1,
            _ => by_you += 1,
        }
    }
    (automatic, by_you, rejected, open)
}

/// The job's mode and how its questions were answered, in words too.
fn answers_value(job: &GuidedJob) -> Value {
    let (automatic, by_you, rejected, open) = answer_counts(job);
    let mut note = format!("{automatic} answered automatically, {by_you} by you");
    if rejected > 0 {
        note.push_str(&format!(", {rejected} rejected by auto-guard (implausible defaults)"));
    }
    if open > 0 {
        note.push_str(&format!(", {open} waiting for an answer"));
    }
    if job.auto {
        if rejected > 0 {
            note.push_str(" (auto mode with guard: plausible defaults accepted, implausible rejected)");
        } else {
            note.push_str(" (auto mode: every question with a default took it)");
        }
    }
    json!({"mode": if job.auto { "auto" } else { "interactive" }, "automatic": automatic, "by_you": by_you, "rejected": rejected, "open": open, "note": note})
}

/// A question as `job.next` gives it: with the sheet it is about and its picture, when known.
fn question_value(job: &GuidedJob, q: &Question) -> Value {
    let ev = q.evidence.as_ref();
    let sheet = ev.and_then(|e| e.sheet.clone()).or_else(|| ev.and_then(|e| e.at).and_then(|at| job.level_at(at)).map(|r| r.number.clone()));
    let crop = ev.and_then(|e| e.crop_png_path.clone().or_else(|| e.leftovers.iter().find_map(|l| l.crop_path.clone())));
    json!({
        "id": q.id,
        "stage": q.stage,
        "question": q.question,
        "why": q.why,
        "choices": q.choices,
        "evidence": q.evidence,
        "sheet": sheet,
        "crop": crop,
        "default": q.default,
        "answer_schema": q.answer_schema,
        "kind": q.kind,
        "always_asked": job.always_ask.contains(&q.id),
        "if_changed": if_changed(job, q),
    })
}

/// The first stage an answer other than the default runs again ([`answer_effect`]'s), by the
/// kind of question.
fn rerun_stage_of(q: &Question) -> Option<StageName> {
    match q.stage {
        StageName::Classify if q.question.contains("discipline") || q.question.contains("names no level") => Some(StageName::Rooms),
        StageName::Levels if q.question.contains("floor-to-floor") || question_level(q).is_some() => Some(StageName::BimBuild),
        StageName::Rooms => Some(StageName::Rooms),
        StageName::Devices if q.question.contains("Unknown symbol") || q.question.starts_with("No device") || q.kind == QuestionKind::Verify => {
            Some(StageName::Devices)
        }
        _ => None,
    }
}

/// What answering a question with something other than its default runs again, and about how
/// long that takes (the stages' last run times): `{reruns, about_s, note}`; `{applies_to}` when
/// the stage hasn't run yet; null when no answer runs anything again.
fn if_changed(job: &GuidedJob, q: &Question) -> Value {
    let Some(from) = rerun_stage_of(q) else { return Value::Null };
    let ran: Vec<&StageState> =
        job.stages.iter().filter(|st| st.name >= from && !matches!(st.status, StageStatus::Skipped | StageStatus::Pending)).collect();
    if ran.is_empty() {
        return json!({"applies_to": from.label(), "note": format!("read when {} runs", from.label())});
    }
    let names: Vec<&str> = ran.iter().map(|st| st.name.label()).collect();
    let ms: u64 = ran.iter().filter_map(|st| st.elapsed_ms).fold(0u64, u64::saturating_add);
    let secs = (ms as f64 / 1000.0).ceil();
    json!({
        "reruns": names,
        "about_s": secs,
        "note": format!(
            "an answer other than the default runs {} again (about {secs:.0} s); answers sent together in job.answer {{answers: [...]}} run it once",
            names.join(", ")
        ),
    })
}

/// The numbers in a stage's result (counts, lengths, list sizes), flattened to `a.b` keys: what
/// an answer's re-run is compared by.
fn digest(v: &Value) -> BTreeMap<String, Value> {
    fn walk(v: &Value, key: String, depth: usize, out: &mut BTreeMap<String, Value>) {
        if out.len() >= MAX_DIGEST {
            return;
        }
        match v {
            Value::Number(_) | Value::Bool(_) if !key.is_empty() => {
                out.insert(key, v.clone());
            }
            Value::Array(a) if !key.is_empty() => {
                out.insert(format!("{key}.len"), json!(a.len()));
            }
            Value::Object(o) if depth < 3 => {
                for (k, x) in o {
                    if k.ends_with("_ms") {
                        continue;
                    }
                    walk(x, if key.is_empty() { k.clone() } else { format!("{key}.{k}") }, depth + 1, out);
                }
            }
            _ => {}
        }
    }
    let mut out = BTreeMap::new();
    walk(v, String::new(), 0, &mut out);
    out
}

/// What changed between two digests of a stage's result.
fn digest_changes(stage: StageName, before: &BTreeMap<String, Value>, after: &BTreeMap<String, Value>) -> Vec<Value> {
    let keys: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    keys.into_iter()
        .filter(|k| before.get(*k) != after.get(*k))
        .take(MAX_VERIFY_SAMPLES * 2)
        .map(|k| json!({"stage": stage.label(), "what": k, "before": before.get(k), "after": after.get(k)}))
        .collect()
}

/// Set `from` and the stages after it to run again: their questions are kept aside so the
/// answers carry over to the same questions asked again, and their findings are dropped (the
/// re-run finds them again). The stages reset, with their result digests.
fn reset_from(job: &mut GuidedJob, from: StageName) -> Vec<(StageName, BTreeMap<String, Value>)> {
    let mut reset = Vec::new();
    let mut carry = Vec::new();
    for st in job.stages.iter_mut().filter(|st| st.name >= from && !matches!(st.status, StageStatus::Skipped | StageStatus::Pending)) {
        reset.push((st.name, st.result.as_ref().map(digest).unwrap_or_default()));
        carry.extend(std::mem::take(&mut st.questions));
        carry.extend(std::mem::take(&mut st.verify));
        carry.extend(std::mem::take(&mut st.completeness));
        st.status = StageStatus::Pending;
        st.result = None;
        st.error = None;
    }
    let names: BTreeSet<StageName> = reset.iter().map(|(n, _)| *n).collect();
    let dropped: BTreeSet<String> = job.findings.iter().filter(|f| names.contains(&f.stage)).map(|f| f.id.clone()).collect();
    job.findings.retain(|f| !dropped.contains(&f.id));
    job.worklist.retain(|w| !dropped.contains(&w.id));
    job.carry.extend(carry);
    reset
}

/// After a stage ran again: the questions it asks again (same kind and text) take their earlier
/// id and answer; earlier answered questions it doesn't ask again are kept, answered; earlier
/// open ones leave the worklist.
fn carry_answers(job: &mut GuidedJob, name: StageName) {
    let (old, keep): (Vec<Question>, Vec<Question>) = std::mem::take(&mut job.carry).into_iter().partition(|q| q.stage == name);
    job.carry = keep;
    if old.is_empty() {
        return;
    }
    let mut renamed: Vec<(String, String)> = Vec::new();
    let mut used = vec![false; old.len()];
    if let Some(st) = job.stage_mut(name) {
        for q in st.questions.iter_mut().chain(st.verify.iter_mut()).chain(st.completeness.iter_mut()) {
            let found = old.iter().enumerate().find(|(i, o)| !used.get(*i).copied().unwrap_or(true) && o.kind == q.kind && o.question == q.question);
            if let Some((i, o)) = found {
                if let Some(u) = used.get_mut(i) {
                    *u = true;
                }
                renamed.push((q.id.clone(), o.id.clone()));
                q.id = o.id.clone();
                if o.answer.is_some() {
                    q.answer = o.answer.clone();
                    q.answered_by = o.answered_by.clone();
                    q.answered_at = o.answered_at.clone();
                }
            }
        }
    }
    for (new, old_id) in &renamed {
        job.worklist.retain(|w| w.id != *new);
        if job.always_ask.remove(new) {
            job.always_ask.insert(old_id.clone());
        }
    }
    let mut gone = BTreeSet::new();
    for (o, u) in old.into_iter().zip(used) {
        if u {
            continue;
        }
        if o.answer.is_some() {
            if let Some(st) = job.stage_mut(name) {
                match o.kind {
                    QuestionKind::Verify => st.verify.push(o),
                    QuestionKind::Completeness => st.completeness.push(o),
                    QuestionKind::Doubt => st.questions.push(o),
                }
            }
        } else {
            gone.insert(o.id);
        }
    }
    job.worklist.retain(|w| !gone.contains(&w.id));
}

/// A floor to floor height in feet from an answer: `12'-0"`, `11'-6"`, `12 ft`, `12`.
pub fn parse_feet(text: &str) -> Option<f64> {
    let t = text.trim();
    let num = |s: &str| -> Option<f64> {
        let digits: String = s.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        digits.parse::<f64>().ok().filter(|v| v.is_finite())
    };
    let feet = match t.split_once('\'') {
        Some((ft, rest)) => {
            let inches = rest.trim_start_matches(['-', ' ']).split('"').next().and_then(num).unwrap_or(0.0);
            num(ft)? + inches / 12.0
        }
        None => num(t)?,
    };
    Some(feet).filter(|f| *f > 0.0 && *f <= 100.0)
}

/// A level's elevation in feet from an answer: `12'-6"`, `-4'-0"`, `0`, `EL. 112'-0"`, `112.5 ft`.
pub fn parse_elevation(text: &str) -> Option<f64> {
    let t = text.trim();
    let t = t.strip_prefix("EL.").or_else(|| t.strip_prefix("EL")).or_else(|| t.strip_prefix("el.")).unwrap_or(t).trim();
    let (sign, t) = match t.strip_prefix('-') {
        Some(rest) => (-1.0, rest.trim()),
        None => (1.0, t.strip_prefix('+').unwrap_or(t).trim()),
    };
    if !t.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    // "0" is an elevation (parse_feet takes heights only).
    let zero = t.trim_end_matches(['\'', '"', ' ', '-']).trim_end_matches("ft").trim().trim_end_matches(['0', '.']).is_empty();
    let feet = if zero { 0.0 } else { parse_feet_any(t)? };
    Some(sign * feet).filter(|f| f.is_finite() && f.abs() <= 10_000.0)
}

/// [`parse_feet`] without its floor-to-floor range.
fn parse_feet_any(t: &str) -> Option<f64> {
    let num = |s: &str| -> Option<f64> {
        let digits: String = s.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        digits.parse::<f64>().ok().filter(|v| v.is_finite())
    };
    match t.split_once('\'') {
        Some((ft, rest)) => {
            let inches = rest.trim_start_matches(['-', ' ']).split('"').next().and_then(num).unwrap_or(0.0);
            Some(num(ft)? + inches / 12.0)
        }
        None => num(t),
    }
}

/// What a level answer says: `{elevation_ft?, name?}` (`elevation_ft` a number or feet text,
/// `rename` for `name`), or text — `Correct` / `Yes, "2"` (it's right), `12'-6"` or
/// `Wrong elevation: 12'-6"` (the elevation), `Wrong name: Mezzanine` (the name),
/// `Mezzanine at 12'-6"` (both). `None` for an answer that says neither.
fn level_answer(answer: &Value) -> Option<LevelAnswer> {
    let clean = |n: &str| -> Option<String> {
        let n = n.trim().trim_matches('"').trim();
        (!n.is_empty() && n.chars().count() <= 40).then(|| n.to_string())
    };
    if let Some(o) = answer.as_object() {
        let elevation = o.get("elevation_ft").or_else(|| o.get("elevation")).and_then(|v| match v {
            Value::Number(n) => n.as_f64().filter(|f| f.is_finite() && f.abs() <= 10_000.0),
            Value::String(t) => parse_elevation(t),
            _ => None,
        });
        let name = o.get("name").or_else(|| o.get("rename")).and_then(Value::as_str).and_then(clean);
        let confirmed = o.get("correct").and_then(Value::as_bool).unwrap_or(false);
        return (elevation.is_some() || name.is_some() || confirmed).then_some(LevelAnswer { elevation_ft: elevation, name, confirmed });
    }
    let text = answer.as_str()?.trim();
    let lower = text.to_ascii_lowercase();
    if lower == "correct" || lower.starts_with("yes") {
        return Some(LevelAnswer { confirmed: true, ..LevelAnswer::default() });
    }
    // "Wrong elevation: 12'-6"", "Different name: Mezzanine": what follows the colon.
    let (label, body) = match text.split_once(':') {
        Some((l, b)) => (l.to_ascii_lowercase(), b.trim()),
        None => (String::new(), text),
    };
    if let Some(e) = parse_elevation(body) {
        return Some(LevelAnswer { elevation_ft: Some(e), ..LevelAnswer::default() });
    }
    // "Mezzanine at 12'-6"" / "Mezzanine @ 12'-6"".
    let lb = body.to_ascii_lowercase();
    if let Some(i) = lb.rfind(" at ").map(|i| (i, 4)).or_else(|| lb.rfind(" @ ").map(|i| (i, 3)))
        && let (Some(head), Some(tail)) = (body.get(..i.0), body.get(i.0 + i.1..))
        && let Some(e) = parse_elevation(tail)
    {
        return Some(LevelAnswer { elevation_ft: Some(e), name: clean(head), confirmed: false });
    }
    if label.contains("name") {
        return clean(body).map(|n| LevelAnswer { name: Some(n), ..LevelAnswer::default() });
    }
    None
}

/// What an answer changes in the job: the rule or setting it adds (or the sheet facts it
/// corrects), and the first stage that has to run again for it. `None` when the answer keeps
/// what the job had, or when nothing in the job reads it.
fn answer_effect(job: &mut GuidedJob, q: &Question) -> (Option<StageName>, String) {
    let Some(answer) = &q.answer else { return (None, String::new()) };
    let text = answer.as_str().map(str::trim).unwrap_or("");
    // The levels the drawings didn't give: the levels stage makes them.
    if q.stage == StageName::Levels && q.question.starts_with("No levels detected") {
        return match parse_level_list(text) {
            Some(names) => {
                let said = names.join(", ");
                job.levels_given = names;
                (Some(StageName::Levels), format!("levels {said}"))
            }
            None => (None, "recorded; name the levels bottom up, like \"1, 2, 3, R\"".into()),
        };
    }
    // A level's elevation or name (or "Correct"), over what the job read.
    if q.stage == StageName::Levels
        && let Some(level) = question_level(q)
    {
        if let Some(a) = level_answer(answer) {
            return level_effect(job, &level, a);
        }
        if text == "Not a level" {
            return (None, "recorded".into());
        }
        return (None, format!("recorded; answer with an elevation (\"12'-6\\\"\") or {{\"name\": \"…\"}} to change level {level}"));
    }
    if !text.is_empty() && q.default.as_deref() == Some(text) {
        return (None, "kept what the job read".into());
    }
    if q.stage == StageName::BimBuild && q.question.starts_with(EQUIP_QUESTION) && answer.is_object() {
        return (Some(StageName::BimBuild), "the build checks the drawings against the submittal's data".into());
    }
    if q.stage == StageName::BimBuild && q.question.starts_with(GEAR_QUESTION) && answer.get("height_in").is_some() {
        return (Some(StageName::BimBuild), "the build lays the gear out with its size".into());
    }
    let values = q.evidence.as_ref().and_then(|e| e.values.clone()).unwrap_or(Value::Null);
    let value = |k: &str| values.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let other = !text.is_empty() && !q.choices.iter().any(|c| c == text);
    match q.stage {
        StageName::Classify => {
            let number = q.evidence.as_ref().and_then(|e| e.sheet.clone()).unwrap_or_default();
            if q.question.contains("discipline") {
                // "E (Electrical)", "Yes, E" → E.
                let letter: String = text.trim_start_matches("Yes,").trim().chars().take_while(char::is_ascii_alphabetic).collect();
                if letter.is_empty() || letter.eq_ignore_ascii_case("Other") || letter.len() > 2 {
                    return (None, "recorded; the sheet keeps no discipline".into());
                }
                let letter = letter.to_ascii_uppercase();
                set_sheet_fact(job, &number, "discipline", &letter);
                return (Some(StageName::Rooms), format!("sheet {number} is discipline {letter}"));
            }
            if q.question.contains("names no level") {
                if text == "Not a level plan" {
                    set_sheet_fact(job, &number, "type", "other");
                    return (Some(StageName::Rooms), format!("sheet {number} is not read as a level plan"));
                }
                let level = text.trim_start_matches("Level").trim().to_string();
                // A level the sheet didn't have: typed, or one of the set's levels offered.
                let named = text != "Another level" && !level.is_empty() && level.len() <= 8;
                if named && (other || text.starts_with("Level")) && !cadaddy_walk3d::levels::is_datum_name(&level) {
                    set_sheet_fact(job, &number, "level", &level);
                    return (Some(StageName::Rooms), format!("sheet {number} is level {level}"));
                }
                return (None, "recorded; answer with the level (\"Level 3\") to move the sheet".into());
            }
            (None, "recorded".into())
        }
        StageName::Levels if q.question.contains("floor-to-floor") => match parse_feet(text) {
            Some(ft) => {
                job.storey_ft = Some(ft);
                (Some(StageName::BimBuild), format!("floor to floor {ft:.2} ft"))
            }
            None => (None, "recorded; give a height like 12'-0\" to build with it".into()),
        },
        StageName::Rooms => {
            if job.always_ask.contains(&q.id) || q.question.starts_with("No rooms") {
                return if text.starts_with("There should be") {
                    (Some(StageName::Rooms), "rooms read again".into())
                } else {
                    (None, "the plans have no room tags".into())
                };
            }
            let (number, name) = match q.kind {
                QuestionKind::Verify => (value("number"), value("room")),
                _ => (value("room_number"), value("room_name")),
            };
            let key = room_key(&number, &name);
            if text.starts_with("Not a room") {
                job.learn_rule(&key, "remove", &q.id, q.stage);
                (Some(StageName::Rooms), format!("room {number} '{name}' is not a room"))
            } else if other && text.len() <= 80 {
                job.learn_rule(&key, &format!("name:{text}"), &q.id, q.stage);
                (Some(StageName::Rooms), format!("room {number} is named '{text}'"))
            } else {
                (None, "recorded; answer with the room's name to rename it".into())
            }
        }
        StageName::Devices => {
            if job.always_ask.contains(&q.id) || q.question.starts_with("No device") {
                return if text.starts_with("There should be") {
                    (Some(StageName::Devices), "devices counted again".into())
                } else {
                    (None, "the sheets have no devices".into())
                };
            }
            if q.question.contains("Unknown symbol") {
                let tag = value("description");
                let kind = answer
                    .get("device_type")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| (other && text != "skip").then(|| text.to_string()));
                return match kind.filter(|k| !k.trim().is_empty() && k.len() <= 80) {
                    Some(k) => {
                        job.learn_rule(&format!("symbol:{tag}"), &format!("device:{k}"), &q.id, q.stage);
                        (Some(StageName::Devices), format!("symbols tagged '{tag}' count as {k}"))
                    }
                    None => (None, format!("symbols tagged '{tag}' stay uncounted")),
                };
            }
            if q.kind == QuestionKind::Verify && text == "Not a device" {
                let tag = value("tag");
                job.learn_rule(&format!("tag:{tag}"), "not_device", &q.id, q.stage);
                return (Some(StageName::Devices), format!("symbols tagged '{tag}' are not devices"));
            }
            (None, "recorded".into())
        }
        _ => (None, "recorded; nothing in the job reads this answer yet".into()),
    }
}

/// The level a levels-stage question is about (its evidence's `level`).
fn question_level(q: &Question) -> Option<String> {
    let v = q.evidence.as_ref()?.values.as_ref()?.get("level")?.as_str()?.trim();
    (!v.is_empty() && v != "?").then(|| v.to_string())
}

/// Whether `code` (a sheet's level: `2`, `R`) or its model name (`Level 2`) is `key`.
fn same_level(code: &str, key: &str) -> bool {
    let key = key.trim();
    code.trim().eq_ignore_ascii_case(key) || level_name(code.trim()).eq_ignore_ascii_case(key)
}

/// The answer the user gave for the level `code` (a sheet's level), if any.
fn answered_level<'a>(job: &'a GuidedJob, code: &str) -> Option<&'a LevelAnswer> {
    job.level_answers.iter().find(|(k, _)| same_level(code, k)).map(|(_, a)| a)
}

/// A level answer into the job: an elevation builds the model again with it; a name renames the
/// model's level (now when the model is built, else when it is); "Correct" marks the level the
/// user's.
fn level_effect(job: &mut GuidedJob, level: &str, a: LevelAnswer) -> (Option<StageName>, String) {
    // "2" and "Level 2" are one level.
    let key = level_name(level.trim()).to_ascii_uppercase();
    if job.level_answers.len() >= MAX_QUESTIONS_PER_STAGE && !job.level_answers.contains_key(&key) {
        return (None, "recorded; too many level answers".into());
    }
    let entry = job.level_answers.entry(key).or_default();
    entry.confirmed = true;
    let mut said = Vec::new();
    if let Some(n) = &a.name {
        entry.name = Some(n.clone());
        said.push(format!("level {level} is named {n}"));
    }
    if let Some(e) = a.elevation_ft {
        entry.elevation_ft = Some(e);
        said.push(format!("level {level} is at {}", cadaddy_walk3d::feet(e)));
        return (Some(StageName::BimBuild), said.join("; "));
    }
    if said.is_empty() {
        return (None, format!("level {level} confirmed by you (source user)"));
    }
    (None, said.join("; "))
}

/// The level answers onto the built model's levels: their names, and the user as their source.
/// Returns what changed.
fn apply_level_answers(s: &mut Session, job: &GuidedJob) -> Vec<String> {
    let mut changes = Vec::new();
    if job.level_answers.is_empty() {
        return changes;
    }
    let Ok((mut sp, true)) = super::bspec::current(s, "guided") else { return changes };
    for (key, a) in &job.level_answers {
        let found = sp
            .levels
            .iter()
            .find(|l| same_level(key, &l.name) || a.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&l.name)))
            .map(|l| l.name.clone());
        let Some(mut current) = found else { continue };
        if let Some(n) = a.name.as_deref().filter(|n| !n.eq_ignore_ascii_case(&current))
            && sp.rename_level(&current, n).is_ok()
        {
            changes.push(format!("{current} renamed {n}"));
            current = n.to_string();
        }
        if sp.level_source(&current) != Some("user") {
            sp.set_level_source(&current, "user");
            changes.push(format!("{current}: source user"));
        }
    }
    if !changes.is_empty()
        && let Ok(spec) = serde_json::to_value(&sp)
        && s.execute("bspec.set", &json!({"replace": true, "spec": spec})).is_err()
    {
        changes.clear();
    }
    changes
}

/// Correct a sheet's fact (`discipline`, `level`, `type`) in the job's sheet list and index, and
/// on its imported rectangle.
fn set_sheet_fact(job: &mut GuidedJob, number: &str, key: &str, value: &str) {
    if number.is_empty() {
        return;
    }
    let same = |sh: &Value| sh.get("number").and_then(Value::as_str).is_some_and(|n| n.split('/').next().unwrap_or(n).eq_ignore_ascii_case(number));
    for list in [job.sheets.as_mut(), job.index.as_mut()].into_iter().flatten() {
        if let Some(a) = list.get_mut("sheets").and_then(Value::as_array_mut) {
            for sh in a.iter_mut().filter(|sh| same(sh)) {
                sh[key] = json!(value);
                if key == "level" {
                    sh["level_source"] = json!("answer");
                }
            }
        }
    }
    for r in job.sheet_regions.iter_mut().filter(|r| r.number.eq_ignore_ascii_case(number)) {
        match key {
            "discipline" => r.discipline = value.to_string(),
            // A view of a several-view sheet keeps the level its own title names.
            "level" if r.view.is_none() => {
                r.level = value.to_string();
                r.levels.clear();
                // The answer is the plan's level: it is built (unless it is a site plan).
                if r.not_built == "no level" {
                    r.not_built.clear();
                }
            }
            "level" => {}
            _ => r.kind = value.to_string(),
        }
    }
}

/// Advance the pipeline: run the current stage and generate questions. A finished job's own
/// checkpoint (in the temporary folder) is removed; one the caller named stays, the finished
/// model in it.
fn advance_pipeline(s: &mut Session, job: &mut GuidedJob) -> Result<()> {
    let _running = Running::start();
    job.cancelled = None;
    let r = advance_stages(s, job);
    if job.is_done()
        && !job.checkpoint_given
        && let Some(path) = &job.checkpoint
    {
        let _ = crate::writefile::remove(path);
    }
    r
}

/// Stop the job at `stage` for a cancel: the stage waits (pending) and job.next runs it.
fn stop_cancelled(s: &mut Session, job: &mut GuidedJob, stage: StageName) {
    JOB_CANCEL.store(false, std::sync::atomic::Ordering::Relaxed);
    if let Some(st) = job.stage_mut(stage) {
        st.status = StageStatus::Pending;
        st.result = None;
    }
    job.cancelled = Some(stage);
    job.audit("job_cancelled", "agent", None, Some(stage), None);
    let line = format!("job: cancelled at {}; job.next goes on from there", stage.label());
    report_progress(s, json!({"stage": stage.label(), "step": "cancelled"}), &line);
    s.echo(line);
}

fn advance_stages(s: &mut Session, job: &mut GuidedJob) -> Result<()> {
    loop {
        let stage_name = match job.current_stage() {
            Some(st) if st.status == StageStatus::Paused => {
                let name = st.name;
                // Take the defaults of the questions the job doesn't ask.
                auto_answer(job, name);
                // Still have unanswered questions with no default — pause.
                if let Some(st) = job.stage(name)
                    && !st.unanswered().is_empty()
                {
                    return Ok(());
                }
                // All questions answered — mark done, move to next.
                if let Some(st) = job.stage_mut(name) {
                    st.status = StageStatus::Done;
                    job.audit("stage_done", "pipeline", None, Some(name), None);
                }
                continue;
            }
            Some(st) if st.status == StageStatus::Pending => st.name,
            Some(st) if st.status == StageStatus::Running => st.name,
            _ => return Ok(()), // All done or error.
        };

        // A cancel stops the job here, before the stage.
        if cancel_requested() {
            stop_cancelled(s, job, stage_name);
            return Ok(());
        }

        // Apply any learned rules before running the stage.
        apply_rules_for_stage(s, job, stage_name)?;

        // Mark running.
        if let Some(st) = job.stage_mut(stage_name) {
            st.status = StageStatus::Running;
        }

        // Run the stage.
        let tag = progress_tag(job, stage_name);
        report_progress(
            s,
            json!({"stage": stage_name.label(), "step": "start", "progress": job.progress()}),
            &format!("{tag}: {}", stage_name.description()),
        );
        let started = web_time::Instant::now();
        if let Ok(d) = s.doc() {
            job.spec_at_start.insert(stage_name, d.building_spec.clone());
        }
        // What the job held before the stage (a stage a cancel cuts short leaves none of its
        // findings and questions behind).
        let held = (job.findings.len(), job.worklist.len());
        let result = match stage_name {
            StageName::Import => run_import(s, job),
            StageName::Classify => run_classify(s, job),
            StageName::Levels => run_levels(s, job),
            StageName::Rooms => run_rooms(s, job),
            StageName::Devices => run_devices(s, job),
            StageName::MepRead => run_mep_read_stage(s, job),
            StageName::BimBuild => run_bim_build(s, job),
            StageName::Takeoff => run_takeoff(s, job),
            StageName::Estimate => run_estimate(s, job),
            StageName::Clash => run_clash(s, job),
        };
        let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(st) = job.stage_mut(stage_name) {
            st.elapsed_ms = Some(ms);
        }
        // Cut short by a cancel (classify stops at its next sheet, the sheet import at its next
        // page): the stage runs again in full when the job goes on. A stage the cancel didn't
        // cut keeps what it did, and the job stops before the next one.
        if cancel_requested() && (result.is_err() || stage_name == StageName::Classify) {
            if let Some(spec) = job.spec_at_start.get(&stage_name).cloned()
                && let Ok(d) = s.doc_mut()
            {
                d.building_spec = spec;
            }
            job.findings.truncate(held.0);
            job.worklist.truncate(held.1);
            if let Some(st) = job.stage_mut(stage_name) {
                st.questions.clear();
                st.verify.clear();
                st.completeness.clear();
            }
            stop_cancelled(s, job, stage_name);
            return Ok(());
        }
        let outcome = match &result {
            Ok(_) => "done".to_string(),
            Err(e) => format!("error: {e}"),
        };
        // With the memory the stage left and the peak so far (the stage where the peak jumps).
        let memory = super::memstat::process_mb();
        let mut end = json!({"stage": stage_name.label(), "step": "end", "elapsed_ms": ms, "ok": result.is_ok()});
        if let Some((now, peak)) = memory {
            end["memory_mb"] = json!(now);
            end["peak_mb"] = json!(peak);
        }
        let memory_text = memory.map_or_else(String::new, |(now, peak)| format!(" (memory {now} MB, peak {peak} MB)"));
        report_progress(s, end, &format!("{tag}: {outcome} in {:.1} s{memory_text}", ms as f64 / 1000.0));

        // Stop (the job pauses or ends in an error) or go on with the next stage.
        let stop = match result {
            Ok(v) => {
                // Check for blocking findings before marking done.
                let has_blocking = job.findings.iter().any(|f| f.stage == stage_name && f.blocking);
                // Store the result; questions asked again keep their earlier answers, and the
                // ones the job doesn't ask take their defaults.
                match job.stage_mut(stage_name) {
                    Some(st) => st.result = Some(v),
                    None => continue,
                }
                carry_answers(job, stage_name);
                auto_answer(job, stage_name);
                let unanswered_count = job.stage(stage_name).map_or(0, |st| st.unanswered().len());
                let all_answered = unanswered_count == 0;
                if all_answered && !has_blocking {
                    if let Some(st) = job.stage_mut(stage_name) {
                        st.status = StageStatus::Done;
                    }
                    job.audit("stage_done", "pipeline", None, Some(stage_name), None);
                    false
                } else if has_blocking {
                    // MUST 1: blocking finding → Error, not Done.
                    let msg = job
                        .findings
                        .iter()
                        .filter(|f| f.stage == stage_name && f.blocking)
                        .map(|f| f.description.as_str())
                        .collect::<Vec<_>>()
                        .join("; ");
                    if let Some(st) = job.stage_mut(stage_name) {
                        st.status = StageStatus::Error;
                        st.error = Some(msg.clone());
                    }
                    job.audit("stage_blocked", "pipeline", None, Some(stage_name), Some(json!({"blocking_findings": msg})));
                    true
                } else {
                    if let Some(st) = job.stage_mut(stage_name) {
                        st.status = StageStatus::Paused;
                    }
                    job.audit("stage_paused", "pipeline", None, Some(stage_name), Some(json!({"unanswered": unanswered_count})));
                    true
                }
            }
            Err(e) => {
                carry_answers(job, stage_name);
                let msg = e.to_string();
                // RW2 MUST 2: Error → worklist finding with evidence + actions
                // so the job never just stops silently.
                let actions = stage_error_actions(stage_name);
                let evidence = Some(QuestionEvidence {
                    crop_png_path: None,
                    sheet: None,
                    at: None,
                    values: Some(json!({"error": &msg, "stage": stage_name.label()})),
                    handles: Vec::new(),
                    tiled_crops: None,
                    leftovers: Vec::new(),
                });
                job.add_finding(
                    stage_name,
                    &format!("{} error: {msg}", stage_name.label()),
                    "Stage encountered an error and could not complete",
                    5,
                    actions,
                    evidence,
                    true,
                );
                if let Some(st) = job.stage_mut(stage_name) {
                    st.status = StageStatus::Error;
                    st.error = Some(msg.clone());
                }
                job.audit("stage_error", "pipeline", None, Some(stage_name), Some(json!({"error": msg})));
                true
            }
        };
        // A crash from here on loses no more than the next stage.
        if !job.is_done() {
            write_checkpoint(s, job);
        }
        if stop {
            return Ok(());
        }
    }
}

/// Recovery actions for a stage error (RW2 MUST 2).
pub fn stage_error_actions(stage: StageName) -> Vec<String> {
    let mut actions = vec!["Retry this stage".into(), "Skip this stage and continue".into()];
    match stage {
        StageName::Classify => {
            actions.push("Re-import with a different path".into());
            actions.push("Set discipline manually".into());
        }
        StageName::Levels => {
            actions.push("Set levels manually".into());
        }
        StageName::Rooms => {
            actions.push("Re-classify sheets and retry".into());
        }
        StageName::Devices => {
            actions.push("Re-classify sheets and retry".into());
        }
        _ => {}
    }
    actions
}

/// Apply learned rules (from previous answers) before running a stage.
fn apply_rules_for_stage(s: &mut Session, job: &mut GuidedJob, _stage: StageName) -> Result<()> {
    // Apply symbol → device rules as project rules.
    for rule in &mut job.rules {
        if rule.key.starts_with("symbol:") {
            // Set as project rule so the readers use it.
            let parts: Vec<&str> = rule.value.splitn(2, ':').collect();
            if parts.len() == 2 {
                let _ = s.execute(
                    "project.rules.set",
                    &json!({
                        "domain": "custom",
                        "key": &rule.key,
                        "value": &rule.value,
                        "source": "guided_job",
                        "tier": "project_rule",
                        "reason": "symbol rule from the guided job",
                    }),
                );
                rule.auto_applied = rule.auto_applied.saturating_add(1);
            }
        }
        // Apply scale overrides.
        if rule.key.starts_with("scale:") {
            let _ = s.execute(
                "project.rules.set",
                &json!({
                    "domain": "custom",
                    "key": &rule.key,
                    "value": &rule.value,
                    "source": "guided_job",
                    "tier": "project_rule",
                    "reason": "scale override from the guided job",
                }),
            );
            rule.auto_applied = rule.auto_applied.saturating_add(1);
        }
    }
    Ok(())
}

// ─── Command implementations ────────────────────────────────────────────────

/// `job.start {pdfs, goal}` — create and start a guided job.
/// One undo step however many commands the job runs: its stages draw through hundreds of
/// commands, and a step of each kept a copy of the drawing (gigabytes on a large set).
fn cmd_start(s: &mut Session, p: &Value) -> Result<Value> {
    s.as_one_undo_step(|s| cmd_start_inner(s, p))
}

fn cmd_start_inner(s: &mut Session, p: &Value) -> Result<Value> {
    let pdfs = str_param(p, "pdfs").ok_or_else(|| bad("job.start", "`pdfs` (path) is required"))?;
    let goal_str = p.get("goal").and_then(Value::as_str).unwrap_or("estimate");
    let goal = JobGoal::from_str(goal_str)
        .ok_or_else(|| bad("job.start", format!("unknown goal '{goal_str}' — use cdy, bim, takeoff, estimate, clash or sequence")))?;

    // Check if a job already exists.
    if let Some(existing) = load_job(s)?
        && !existing.is_done()
    {
        let hint = if p.get("auto").and_then(Value::as_bool) == Some(true) && !existing.auto {
            "a job is already running — job.set_mode {auto: true} runs it on unattended (answers given are kept); job.status shows it"
        } else {
            "a job is already running — job.status shows it, job.set_mode {auto: true} runs it on unattended, or start a new drawing"
        };
        return Err(bad("job.start", hint));
    }

    let mut job = GuidedJob::new(pdfs, goal);
    // Interactive unless asked: the job stops at each checkpoint for an answer.
    job.auto = p.get("auto").and_then(Value::as_bool).unwrap_or(false);
    job.submittals = match p.get("submittals") {
        Some(Value::String(one)) if !one.trim().is_empty() => vec![one.trim().to_string()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).take(64).collect(),
        _ => Vec::new(),
    };
    job.specs = match p.get("specs") {
        Some(Value::String(one)) if !one.trim().is_empty() => vec![one.trim().to_string()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).take(64).collect(),
        _ => Vec::new(),
    };
    // The checkpoint: a path, false (none), else the default in the temporary folder.
    job.checkpoint = match p.get("checkpoint") {
        Some(Value::Bool(false)) => None,
        Some(Value::String(path)) if !path.trim().is_empty() => {
            let path = path.trim().to_string();
            // The caller's own drawing (or any file that isn't a job's checkpoint) is never
            // written over or removed.
            if std::path::Path::new(&path).exists() && !is_job_checkpoint(&path) {
                return Err(bad(
                    "job.start",
                    format!(
                        "checkpoint {path} is an existing file that is not a job checkpoint: the job would write over it. Give a new path (or a job's checkpoint to resume)"
                    ),
                ));
            }
            job.checkpoint_given = true;
            Some(path)
        }
        _ => default_checkpoint(&job.id),
    };
    job.prints_stamp = prints_stamp(pdfs);
    // A job of these prints cut off before it finished goes on from its last stage.
    let resume = p.get("resume").and_then(Value::as_bool).unwrap_or(true);
    if resume
        && let Some(path) = job.checkpoint.clone()
        && let Some((d, mut saved)) = read_checkpoint(&path, pdfs, &job.goal, &job.prints_stamp)
    {
        *s.doc_mut()? = d;
        // The print set's project lives in the session, not the drawing.
        s.execute("printset.open", &json!({"path": pdfs}))?;
        for st in saved.stages.iter_mut().filter(|st| st.status == StageStatus::Running) {
            st.status = StageStatus::Pending;
        }
        saved.checkpoint = Some(path.clone());
        saved.checkpoint_given = job.checkpoint_given;
        saved.auto = job.auto;
        if !job.submittals.is_empty() {
            if saved.submittals != job.submittals {
                saved.submittal_types.clear();
                saved.luminaire_types.clear();
                saved.wall_luminaire_types.clear();
            }
            saved.submittals = job.submittals.clone();
        }
        let done: Vec<&str> = saved.stages.iter().filter(|st| st.status == StageStatus::Done).map(|st| st.name.label()).collect();
        let line = format!("job: resumed from {path} ({} stage(s) done: {})", done.len(), done.join(", "));
        let data = json!({"step": "resumed", "checkpoint": path, "stages_done": done});
        report_progress(s, data, &line);
        s.echo(line);
        saved.audit("job_resumed", "pipeline", None, None, Some(json!({"checkpoint": path})));
        advance_pipeline(s, &mut saved)?;
        save_job(s, &saved)?;
        let stats = model_stats_from_session(s);
        let mut status = job_status_value(&saved, stats.as_ref());
        status["resumed"] = json!(true);
        return Ok(status);
    }
    job.audit("job_start", "pipeline", None, None, Some(json!({"pdfs": pdfs, "goal": goal_str, "auto": job.auto})));

    // Run the pipeline until it pauses or finishes.
    advance_pipeline(s, &mut job)?;
    save_job(s, &job)?;

    let stats = model_stats_from_session(s);
    Ok(job_status_value(&job, stats.as_ref()))
}

/// `job.next` — return the next question, or "done".
/// One undo step however many commands the job runs: its stages draw through hundreds of
/// commands, and a step of each kept a copy of the drawing (gigabytes on a large set).
fn cmd_next(s: &mut Session, p: &Value) -> Result<Value> {
    s.as_one_undo_step(|s| cmd_next_inner(s, p))
}

fn cmd_next_inner(s: &mut Session, _p: &Value) -> Result<Value> {
    let mut job = load_job(s)?.ok_or_else(|| bad("job.next", "no job running — call job.start first"))?;

    // Answers given with `rerun: false`: their stages run again now, once.
    let mut rerun = Vec::new();
    if let Some(from) = job.rerun_held.take() {
        let reset = rerun_from_stage(s, &mut job, from, &json!("held answers"), None)?;
        advance_pipeline(s, &mut job)?;
        save_job(s, &job)?;
        rerun = reset;
    }
    // Try to advance if no questions pending.
    if job.next_question().is_none() && !job.is_done() {
        advance_pipeline(s, &mut job)?;
        save_job(s, &job)?;
    }
    let changed = rerun_changes(&job, &rerun);
    let rerun_value = (!rerun.is_empty()).then(|| json!({"stages": rerun.iter().map(|(n, _)| n.label()).collect::<Vec<_>>(), "changed": changed}));
    let mut out = cmd_next_value(s, &job);
    if let (Some(r), Some(o)) = (rerun_value, out.as_object_mut()) {
        o.insert("rerun".into(), r);
    }
    Ok(out)
}

/// What `job.next` says: the next question, done, or waiting.
fn cmd_next_value(s: &mut Session, job: &GuidedJob) -> Value {
    match job.next_question() {
        Some(q) => json!({
            "question": question_value(job, q),
            "progress": job.progress(),
            "stage": job.current_stage().map(|s| s.name.label()),
            "answers": answers_value(job),
        }),
        None if job.is_done() => {
            let stats = model_stats_from_session(s);
            json!({
                "done": true,
                "progress": 1.0,
                "summary": job_summary(s, job, stats.as_ref()),
            })
        }
        None => json!({
            "waiting": true,
            "progress": job.progress(),
            "stage": job.current_stage().map(|s| s.name.label()),
            "status": job.current_stage().map(|s| format!("{:?}", s.status)),
        }),
    }
}

/// Most answers one `job.answer` takes.
const MAX_ANSWERS_PER_CALL: usize = 500;

/// One answer set on its question: what it changes and the stage it re-runs.
struct Applied {
    qid: String,
    stage: Option<StageName>,
    rerun_from: Option<StageName>,
    effect: String,
    question: Question,
}

/// Where a question is: (stage index, list, index in it).
fn find_question(job: &GuidedJob, qid: &str) -> Option<(usize, u8, usize)> {
    for (si, stage) in job.stages.iter().enumerate() {
        for (list, qs) in [(0u8, &stage.questions), (1, &stage.verify), (2, &stage.completeness)] {
            if let Some(qi) = qs.iter().position(|q| q.id == qid) {
                return Some((si, list, qi));
            }
        }
    }
    None
}

fn question_mut(job: &mut GuidedJob, (si, list, qi): (usize, u8, usize)) -> Option<&mut Question> {
    let stage = job.stages.get_mut(si)?;
    match list {
        0 => stage.questions.get_mut(qi),
        1 => stage.verify.get_mut(qi),
        _ => stage.completeness.get_mut(qi),
    }
}

/// Why `qid` can't be answered: never asked, or asked and gone (an earlier answer ran its stage
/// again and it wasn't asked again, or it was merged into another).
fn unknown_question(job: &GuidedJob, qid: &str) -> String {
    let seen = job.audit.iter().any(|a| a.question_id.as_deref() == Some(qid)) || job.worklist.iter().any(|w| w.id == qid);
    let shown: String = qid.chars().take(60).collect();
    if seen {
        format!(
            "question '{shown}' is no longer open — an earlier answer ran its stage again and it was not asked again (or was merged into another); job.next gives the open ones"
        )
    } else {
        format!("question '{shown}' not found — ids look like Q-rooms-12; job.next gives the open ones")
    }
}

/// Set one answer on its question, learn from it and work out its effect. The stage is not run
/// again here: the caller runs it once for every answer.
fn apply_answer(job: &mut GuidedJob, qid: &str, answer: Value) -> std::result::Result<Applied, String> {
    if answer.is_null() {
        return Err(format!("question '{}': `answer` is required", qid.chars().take(60).collect::<String>()));
    }
    let loc = find_question(job, qid).ok_or_else(|| unknown_question(job, qid))?;
    let question = {
        let Some(q) = question_mut(job, loc) else { return Err(unknown_question(job, qid)) };
        check_answer(q, &answer)?;
        q.answer = Some(answer.clone());
        q.answered_by = Some("agent".into());
        q.answered_at = Some(now_iso());
        q.clone()
    };
    let stage = job.stages.get(loc.0).map(|s| s.name);
    learn_from_answer(job, &question);
    let (rerun_from, effect) = answer_effect(job, &question);
    job.audit("answer", "agent", Some(qid), stage, Some(json!({"answer": answer, "effect": effect})));
    // MUST 5 REFRAME: Sync worklist — mark this item resolved.
    if let Some(wi) = job.worklist.iter_mut().find(|w| w.id == *qid) {
        wi.status = "resolved".into();
        wi.resolution = Some(answer);
    }
    Ok(Applied { qid: qid.to_string(), stage, rerun_from, effect, question })
}

/// Whether an answer fits its question: what its `answer_schema` asks for (text, an object with
/// its fields), never empty; the levels question a list of levels.
fn check_answer(q: &Question, answer: &Value) -> std::result::Result<(), String> {
    let id = q.id.as_str();
    let empty = match answer {
        Value::String(t) => t.trim().is_empty(),
        Value::Object(o) => o.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Null => true,
        _ => false,
    };
    if empty {
        return Err(format!("question {id}: the answer is empty"));
    }
    // One of its choices, or its default, always fits.
    if answer.as_str().is_some_and(|t| q.default.as_deref() == Some(t.trim()) || q.choices.iter().any(|c| c == t.trim())) {
        return Ok(());
    }
    let want = q.answer_schema.as_ref().and_then(|s| s.get("type")).and_then(Value::as_str);
    match want {
        Some("string") if !answer.is_string() => return Err(format!("question {id}: the answer must be text ({answer} given)")),
        // An object with its fields, or text naming it.
        Some("object") if !answer.is_string() => {
            let Some(o) = answer.as_object() else { return Err(format!("question {id}: the answer must be an object or text ({answer} given)")) };
            let props = q.answer_schema.as_ref().and_then(|s| s.get("properties")).and_then(Value::as_object);
            if let Some(props) = props {
                if !o.keys().any(|k| props.contains_key(k)) {
                    let names: Vec<&String> = props.keys().collect();
                    return Err(format!("question {id}: the answer names none of {names:?}"));
                }
                for (k, v) in o {
                    if props.get(k).and_then(|p| p.get("type")).and_then(Value::as_str) == Some("string") && !v.is_string() {
                        return Err(format!("question {id}: `{k}` must be text"));
                    }
                }
            }
        }
        _ => {}
    }
    if q.stage == StageName::Levels && q.question.starts_with("No levels detected") && answer.as_str().and_then(parse_level_list).is_none() {
        return Err(format!("question {id}: name the levels bottom up, like \"1, 2, 3, R\""));
    }
    Ok(())
}

/// Run `from` and the stages after it that already ran again (the answers changed what they
/// read). Returns the stages reset, with their result digests from before.
fn rerun_from_stage(
    s: &mut Session,
    job: &mut GuidedJob,
    from: StageName,
    reason: &Value,
    qid: Option<&str>,
) -> Result<Vec<(StageName, BTreeMap<String, Value>)>> {
    let reset = reset_from(job, from);
    // The built model is in the drawing: stages before the build would read it as more
    // plan. It goes, and the build runs again after them. The stages start from the
    // building spec they had (later stages store their own, and the readers use it).
    if from < StageName::BimBuild && reset.iter().any(|(n, _)| *n == StageName::BimBuild) {
        s.execute("model.build", &json!({"clear": true}))?;
    }
    if let Some(spec) = reset.first().and_then(|(n, _)| job.spec_at_start.get(n)).cloned() {
        s.doc_mut()?.building_spec = spec;
    }
    if !reset.is_empty() {
        let names: Vec<&str> = reset.iter().map(|(n, _)| n.label()).collect();
        job.audit("stage_rerun", "pipeline", qid, Some(from), Some(json!({"reason": reason, "stages": names})));
    }
    Ok(reset)
}

/// What changed in the stages that ran again.
fn rerun_changes(job: &GuidedJob, reset: &[(StageName, BTreeMap<String, Value>)]) -> Vec<Value> {
    let mut changed = Vec::new();
    for (name, before) in reset {
        if let Some(after) = job.stage(*name).and_then(|st| st.result.as_ref()).map(digest) {
            changed.extend(digest_changes(*name, before, &after));
        }
    }
    changed
}

/// `job.answer {id, answer} | {answers: [{id, answer}…]}` — answer one question or many. Each
/// answer stands alone: one whose question is gone (merged, or not asked again after a re-run)
/// is a warning and the others are kept and saved. The stages the answers change run again
/// once, from the first of them (`rerun: false` holds that until the next answer or job.next).
/// One undo step however many commands the job runs: its stages draw through hundreds of
/// commands, and a step of each kept a copy of the drawing (gigabytes on a large set).
fn cmd_answer(s: &mut Session, p: &Value) -> Result<Value> {
    s.as_one_undo_step(|s| cmd_answer_inner(s, p))
}

fn cmd_answer_inner(s: &mut Session, p: &Value) -> Result<Value> {
    let mut items: Vec<(String, Value)> = Vec::new();
    match p.get("answers") {
        Some(Value::Array(list)) => {
            if list.len() > MAX_ANSWERS_PER_CALL {
                return Err(bad("job.answer", format!("at most {MAX_ANSWERS_PER_CALL} answers per call ({} given)", list.len())));
            }
            for (i, a) in list.iter().enumerate() {
                let id = a.get("id").and_then(Value::as_str).map(str::trim).unwrap_or("");
                items.push((if id.is_empty() { format!("#{i}") } else { id.to_string() }, a.get("answer").cloned().unwrap_or(Value::Null)));
            }
            if items.is_empty() {
                return Err(bad("job.answer", "`answers` is empty — give [{id, answer}…]"));
            }
        }
        Some(_) => return Err(bad("job.answer", "`answers` must be a list of {id, answer}")),
        None => {
            let qid = str_param(p, "id").ok_or_else(|| bad("job.answer", "`id` (question id from job.next, e.g. Q-rooms-12) is required"))?;
            let answer = p.get("answer").cloned().unwrap_or(Value::Null);
            if answer.is_null() {
                return Err(bad("job.answer", "`answer` is required"));
            }
            items.push((qid.to_string(), answer));
        }
    }
    let batch = p.get("answers").is_some();
    let run_now = p.get("rerun").and_then(Value::as_bool).unwrap_or(true);

    let mut job = load_job(s)?.ok_or_else(|| bad("job.answer", "no job running"))?;

    let mut applied: Vec<Applied> = Vec::new();
    let mut warnings: Vec<Value> = Vec::new();
    for (qid, answer) in items {
        match apply_answer(&mut job, &qid, answer) {
            Ok(a) => applied.push(a),
            Err(w) => warnings.push(json!({"id": qid, "warning": w})),
        }
    }
    // Not one answer taken because of an unknown question or an answer that doesn't fit: an
    // error, the job as it was. (A question no longer open — an earlier answer's re-run dropped
    // it — stays a warning: a script's later lines still run.)
    let fatal: Vec<&str> = warnings.iter().filter_map(|w| w["warning"].as_str()).filter(|t| !t.contains("is no longer open")).collect();
    if applied.is_empty() && !fatal.is_empty() {
        return Err(bad("job.answer", fatal.join("; ")));
    }

    // The answers' re-runs (and one held by an earlier `rerun: false`), as one.
    let wanted = applied.iter().filter_map(|a| a.rerun_from).chain(job.rerun_held).min();
    let mut reset = Vec::new();
    let mut held = None;
    match wanted {
        Some(from) if run_now => {
            job.rerun_held = None;
            let reasons: Vec<&str> = applied.iter().filter(|a| a.rerun_from.is_some()).map(|a| a.effect.as_str()).collect();
            let first = applied.iter().find(|a| a.rerun_from == Some(from)).map(|a| a.qid.clone());
            reset = rerun_from_stage(s, &mut job, from, &json!(reasons.join("; ")), first.as_deref())?;
        }
        Some(from) => {
            // Held: the stage runs when the answers are all in.
            let ran = job.stages.iter().any(|st| st.name >= from && !matches!(st.status, StageStatus::Skipped | StageStatus::Pending));
            if ran {
                job.rerun_held = Some(from);
                held = Some(from);
            }
        }
        None => {}
    }
    // A stage whose questions are all answered is done.
    let touched: BTreeSet<StageName> = applied.iter().filter_map(|a| a.stage).collect();
    for sn in touched {
        if let Some(st) = job.stage_mut(sn)
            && st.status == StageStatus::Paused
            && st.unanswered().is_empty()
        {
            st.status = StageStatus::Done;
            job.audit("stage_done", "pipeline", None, Some(sn), None);
        }
    }
    // A held re-run waits: the stages after it would only run again.
    if held.is_none() {
        advance_pipeline(s, &mut job)?;
    }

    let changed = rerun_changes(&job, &reset);
    // Read again and still nothing: the reader misses what the answer says is there.
    for a in &applied {
        if a.question.answer.as_ref().and_then(Value::as_str).is_some_and(|t| t.starts_with("There should be"))
            && let Some(sn) = a.stage
            && reset.iter().any(|(n, _)| *n == sn)
            && !changed.iter().any(|c| c["stage"] == sn.label())
        {
            job.add_finding(
                sn,
                &format!("{}: read again after your answer and still nothing found", sn.label()),
                "The answer says the sheets have them; the reader doesn't find them on this set",
                5,
                vec!["Look at the sheets named in the question".into()],
                a.question.evidence.clone(),
                false,
            );
        }
    }
    // Level names and confirmations reach a model already built.
    let mut level_changes = Vec::new();
    if applied.iter().any(|a| a.stage == Some(StageName::Levels))
        && job.stage(StageName::BimBuild).is_some_and(|st| st.status == StageStatus::Done)
        && !reset.iter().any(|(n, _)| *n == StageName::BimBuild)
    {
        level_changes = apply_level_answers(s, &job);
    }
    sync_worklist(&mut job);
    save_job(s, &job)?;

    let rerun: Vec<&str> = reset.iter().map(|(n, _)| n.label()).collect();
    let applies_to = wanted.filter(|_| reset.is_empty() && held.is_none()).map(StageName::label);
    let ids: Vec<&str> = applied.iter().map(|a| a.qid.as_str()).collect();
    let answered = match (batch, ids.as_slice()) {
        (false, [one]) => json!(one),
        (false, _) => Value::Null,
        _ => json!(ids),
    };
    let mut out = match job.next_question() {
        Some(q) => json!({
            "answered": answered,
            "next": question_value(&job, q),
            "progress": job.progress(),
        }),
        None if job.is_done() => {
            let stats = model_stats_from_session(s);
            json!({
                "answered": answered,
                "done": true,
                "progress": 1.0,
                "summary": job_summary(s, &job, stats.as_ref()),
            })
        }
        None => json!({
            "answered": answered,
            "progress": job.progress(),
            "stage": job.current_stage().map(|s| s.name.label()),
        }),
    };
    let effects: Vec<&str> = applied.iter().map(|a| a.effect.as_str()).collect();
    out["effect"] = match (batch, effects.as_slice()) {
        (false, [one]) => json!(one),
        (false, []) => json!(""),
        _ => json!(applied.iter().map(|a| json!({"id": a.qid, "effect": a.effect})).collect::<Vec<_>>()),
    };
    out["rerun"] = json!(rerun);
    out["changed"] = json!(changed);
    if let Some(stage) = applies_to {
        out["applies_to"] = json!(stage);
    }
    if let Some(from) = held {
        out["rerun_held"] = json!({
            "from": from.label(),
            "note": "held (rerun: false): it runs once on the next job.answer or job.next",
        });
    }
    if !level_changes.is_empty() {
        out["levels_changed"] = json!(level_changes);
    }
    if !warnings.is_empty() {
        out["warnings"] = json!(warnings);
    }
    out["answers"] = answers_value(&job);
    Ok(out)
}

/// `job.set_mode {auto}` — switch a job between interactive and unattended. Answers already
/// given are kept; going to auto takes the defaults of the open questions and runs on.
/// One undo step however many commands the job runs: its stages draw through hundreds of
/// commands, and a step of each kept a copy of the drawing (gigabytes on a large set).
fn cmd_set_mode(s: &mut Session, p: &Value) -> Result<Value> {
    s.as_one_undo_step(|s| cmd_set_mode_inner(s, p))
}

fn cmd_set_mode_inner(s: &mut Session, p: &Value) -> Result<Value> {
    let auto = p.get("auto").and_then(Value::as_bool).ok_or_else(|| bad("job.set_mode", "`auto` (true or false) is required"))?;
    let mut job = load_job(s)?.ok_or_else(|| bad("job.set_mode", "no job running — job.start {pdfs, auto: true} starts one unattended"))?;
    let was = job.auto;
    job.auto = auto;
    job.audit("set_mode", "agent", None, None, Some(json!({"auto": auto, "was": was})));
    let mut reset = Vec::new();
    if auto && !job.is_done() {
        // A held re-run runs first: the defaults go to the questions it asks.
        if let Some(from) = job.rerun_held.take() {
            reset = rerun_from_stage(s, &mut job, from, &json!("held answers"), None)?;
        }
        advance_pipeline(s, &mut job)?;
    }
    save_job(s, &job)?;
    let stats = model_stats_from_session(s);
    let mut status = job_status_value(&job, stats.as_ref());
    status["mode"] = json!({"from": if was { "auto" } else { "interactive" }, "to": if auto { "auto" } else { "interactive" }});
    if !reset.is_empty() {
        status["rerun"] = json!(reset.iter().map(|(n, _)| n.label()).collect::<Vec<_>>());
    }
    Ok(status)
}

/// `job.status` — current job status (returns `{running: false}` when no job exists).
fn cmd_status(s: &mut Session, _p: &Value) -> Result<Value> {
    match load_job(s)? {
        Some(job) => {
            let stats = model_stats_from_session(s);
            Ok(job_status_value(&job, stats.as_ref()))
        }
        None => Ok(json!({"running": false, "hint": "no job running — start one with job.start {pdfs, goal}"})),
    }
}

/// `job.explain {stage}` — explain what a stage does and its current state.
fn cmd_explain(s: &mut Session, p: &Value) -> Result<Value> {
    let stage_label = p.get("stage").and_then(Value::as_str).unwrap_or("");
    // Before a job starts, an agent can still ask what the stages do.
    let Some(job) = load_job(s)? else {
        let pick = |n: &StageName| json!({"stage": n.label(), "description": n.description(), "status": "NotStarted"});
        return Ok(match StageName::ALL.iter().find(|n| n.label() == stage_label) {
            Some(n) => {
                let mut v = pick(n);
                v["note"] = json!("no job running yet — job.start {pdfs} starts one");
                v
            }
            None => {
                json!({"stages": StageName::ALL.iter().map(pick).collect::<Vec<_>>(), "note": "no job running yet — job.start {pdfs} starts one"})
            }
        });
    };

    let stage = job.stages.iter().find(|st| st.name.label() == stage_label);
    match stage {
        Some(st) => Ok(json!({
            "stage": st.name.label(),
            "description": st.name.description(),
            "status": format!("{:?}", st.status),
            "questions_total": st.questions.len() + st.verify.len() + st.completeness.len(),
            "questions_answered": st.questions.iter().chain(st.verify.iter()).chain(st.completeness.iter()).filter(|q| q.answer.is_some()).count(),
            "questions_pending": st.unanswered().len(),
            "result": st.result,
            "error": st.error,
        })),
        None => {
            let stages: Vec<Value> = job
                .stages
                .iter()
                .map(|st| {
                    json!({
                        "stage": st.name.label(),
                        "description": st.name.description(),
                        "status": format!("{:?}", st.status),
                    })
                })
                .collect();
            Ok(json!({"stages": stages, "note": "Provide a stage name to get details."}))
        }
    }
}

/// `job.rules` — list learned project rules.
fn cmd_rules(s: &mut Session, _p: &Value) -> Result<Value> {
    let job = load_job(s)?.ok_or_else(|| bad("job.rules", "no job running"))?;
    let rules: Vec<Value> = job
        .rules
        .iter()
        .map(|r| {
            json!({
                "key": r.key,
                "value": r.value,
                "from_question": r.from_question,
                "stage": r.stage,
                "auto_applied": r.auto_applied,
            })
        })
        .collect();
    Ok(json!({"rules": rules, "count": rules.len()}))
}

/// Q6021 MUST 3: warnings the report raises when the job is done but something is off.
fn job_warnings(job: &GuidedJob, stats: Option<&ModelStats>) -> Vec<Value> {
    let mut w: Vec<Value> = Vec::new();

    // 0 rooms.
    let rooms_stage = job.stage(StageName::Rooms);
    let room_count = rooms_stage.and_then(|st| st.result.as_ref()).and_then(|r| r.get("count")).and_then(Value::as_u64).unwrap_or(0);
    if rooms_stage.is_some_and(|st| st.status == StageStatus::Done) && room_count == 0 {
        w.push(json!({"warning": "0 rooms", "detail": "The rooms stage finished with 0 rooms in the model"}));
    }

    // Devices not placed in the model.
    let devices_stage = job.stage(StageName::Devices);
    if devices_stage.is_some_and(|st| st.status == StageStatus::Done) && job.devices.is_empty() {
        w.push(json!({"warning": "devices not in model", "detail": "The devices stage ran but 0 devices were placed in the model"}));
    }

    // Estimate $0 or unpriced.
    let estimate_stage = job.stage(StageName::Estimate);
    if let Some(est) = estimate_stage.filter(|st| st.status == StageStatus::Done) {
        let total = est.result.as_ref().and_then(|r| r.get("total")).and_then(Value::as_f64).unwrap_or(0.0);
        let unpriced = est.result.as_ref().and_then(|r| r.get("unpriced")).and_then(Value::as_u64).unwrap_or(0);
        if total <= 0.0 {
            w.push(json!({"warning": "estimate $0", "detail": "The estimate total is $0"}));
        }
        if unpriced > 0 {
            w.push(json!({"warning": "unpriced items", "detail": format!("{unpriced} line(s) have no price")}));
        }
    }

    // MEP sheets not registered (plan sheets not loaded/built).
    let not_built = not_built_sheets(job);
    if !not_built.is_empty() {
        w.push(json!({"warning": "sheets not built", "detail": not_built.join("; ")}));
    }

    // Auto-guard rejections (Q6021).
    let rejected: Vec<String> = job
        .stages
        .iter()
        .flat_map(|st| st.questions.iter().chain(st.verify.iter()).chain(st.completeness.iter()))
        .filter(|q| q.answered_by.as_deref() == Some("auto_guard"))
        .map(|q| q.question.clone())
        .collect();
    if !rejected.is_empty() {
        w.push(json!({"warning": "auto-guard rejections", "count": rejected.len(), "detail": format!("{} rejected: {}", rejected.len(), rejected.join("; "))}));
    }

    // Elements from the saved model.
    if let Some(ms) = stats
        && (ms.elements_by_kind.is_empty() || ms.elements_by_kind.values().sum::<usize>() == 0)
    {
        w.push(json!({"warning": "0 elements in model", "detail": "The building model has 0 elements"}));
    }
    w
}

/// Q6021 MUST 3: "done" vs "done_with_warnings".
fn done_status(job: &GuidedJob, warnings: &[Value]) -> &'static str {
    if job.stages.iter().any(|st| st.status == StageStatus::Error) {
        "error"
    } else if job.cancelled.is_some() {
        "cancelled"
    } else if !job.is_done() {
        "in_progress"
    } else if warnings.is_empty() {
        "done"
    } else {
        "done_with_warnings"
    }
}

/// `job.report` — final report (md + json).
fn cmd_report(s: &mut Session, _p: &Value) -> Result<Value> {
    let job = load_job(s)?.ok_or_else(|| bad("job.report", "no job running"))?;

    let mut automatic = 0usize;
    let mut confirmed = 0usize;
    let mut assumed = 0usize;

    for stage in &job.stages {
        for q in stage.questions.iter().chain(stage.verify.iter()).chain(stage.completeness.iter()) {
            if q.answer.is_some() {
                if q.answered_by.as_deref() == Some("default") {
                    automatic += 1;
                } else {
                    confirmed += 1;
                }
            } else {
                assumed += 1;
            }
        }
    }

    let stages_summary: Vec<Value> = job
        .stages
        .iter()
        .filter(|s| s.status != StageStatus::Skipped)
        .map(|st| {
            json!({
                "stage": st.name.label(),
                "status": format!("{:?}", st.status),
                "questions": st.questions.len() + st.verify.len() + st.completeness.len(),
                "answered": st.questions.iter().chain(st.verify.iter()).chain(st.completeness.iter()).filter(|q| q.answer.is_some()).count(),
            })
        })
        .collect();

    let stats = model_stats_from_session(s);
    let warnings = job_warnings(&job, stats.as_ref());
    let status = done_status(&job, &warnings);

    Ok(json!({
        "job_id": job.id,
        "goal": job.goal,
        "created": job.created,
        "pdfs": job.pdfs_path,
        "done": job.is_done(),
        "status": status,
        "warnings": warnings,
        "progress": job.progress(),
        "stages": stages_summary,
        "totals": {
            "automatic": automatic,
            "confirmed": confirmed,
            "assumed": assumed,
        },
        "answers": answers_value(&job),
        "rules_learned": job.rules.len(),
        "audit_entries": job.audit.len(),
        "not_built": not_built_sheets(&job).join("; "),
        "property_coverage": s.doc().map(super::propuse::coverage).unwrap_or(Value::Null),
    }))
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn job_status_value(job: &GuidedJob, stats: Option<&ModelStats>) -> Value {
    let stages: Vec<Value> = job
        .stages
        .iter()
        .map(|st| {
            json!({
                "stage": st.name.label(),
                "status": format!("{:?}", st.status),
                "questions": st.questions.len() + st.verify.len() + st.completeness.len(),
                "unanswered": st.unanswered().len(),
                "elapsed_ms": st.elapsed_ms,
            })
        })
        .collect();

    let pending_worklist = job.worklist.iter().filter(|w| w.status == "pending").count();

    // Q5278 MUST 1: levels from the saved model when available.
    let levels: Value = match stats {
        Some(ms) => json!(ms.levels),
        None => index_count(job, "levels").map_or(Value::Null, |n| json!(n)),
    };
    let elements: Value = match stats {
        Some(ms) if !ms.elements_by_kind.is_empty() => {
            let total: usize = ms.elements_by_kind.values().sum();
            json!({"total": total, "by_kind": ms.elements_by_kind})
        }
        _ => Value::Null,
    };

    // Q6021 MUST 3: warnings and done_with_warnings status.
    let warnings = job_warnings(job, stats);
    let status = done_status(job, &warnings);

    // Q6037: auto-rejected questions count as open (they need resolution).
    let unanswered: usize = job.stages.iter().map(|s| s.unanswered().len()).sum();
    let (_, _, rejected_count, _) = answer_counts(job);
    let open_questions = unanswered + rejected_count;

    let mut v = json!({
        "job_id": job.id,
        "goal": job.goal,
        "progress": job.progress(),
        "done": job.is_done(),
        "status": status,
        "current_stage": job.current_stage().map(|s| s.name.label()),
        "stages": stages,
        "open_questions": open_questions,
        "findings": job.findings.len(),
        "worklist_pending": pending_worklist,
        "rules_learned": job.rules.len(),
        "elapsed_ms": job.stages.iter().filter_map(|st| st.elapsed_ms).sum::<u64>(),
        "checkpoint": job.checkpoint.as_ref().filter(|_| job.checkpoint_given || !job.is_done()),
        "sheets": index_count(job, "sheets"),
        "levels": levels,
        "elements": elements,
        "answers": answers_value(job),
        "links": job.links,
    });
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
    // Q6037: auto-rejected questions are listed as open with their reason.
    let rejected_qs: Vec<Value> = job
        .stages
        .iter()
        .flat_map(|st| st.questions.iter().chain(st.verify.iter()).chain(st.completeness.iter()))
        .filter(|q| q.answered_by.as_deref() == Some("auto_guard"))
        .map(|q| json!({"id": q.id, "question": q.question, "reason": "implausible default rejected by auto-guard", "default": q.default}))
        .collect();
    if !rejected_qs.is_empty() {
        v["rejected_questions"] = json!(rejected_qs);
    }
    if let Some(from) = job.rerun_held {
        v["rerun_held"] = json!(from.label());
    }
    // The stage that failed and why: the CLI exits non-zero on it.
    if let Some(st) = job.stages.iter().find(|st| st.status == StageStatus::Error) {
        v["error"] = json!({"stage": st.name.label(), "message": st.error.clone().unwrap_or_default()});
    }
    if let Some(at) = job.cancelled {
        v["cancelled"] = json!({"stage": at.label(), "note": "job.next goes on from there"});
    }
    // A paused job says what it is asking.
    if let Some(q) = job.next_question() {
        v["question"] = question_value(job, q);
    }
    v
}

/// How many sheets or levels the job's sheet index holds (none before classify).
fn index_count(job: &GuidedJob, key: &str) -> Option<usize> {
    job.index.as_ref().and_then(|i| i.get(key)).and_then(Value::as_array).map(Vec::len)
}

/// Model statistics from the saved drawing (Q5278 MUST 1).
struct ModelStats {
    levels: usize,
    elements_by_kind: BTreeMap<String, usize>,
}

/// Extract model statistics from the session's saved model.
fn model_stats_from_session(s: &Session) -> Option<ModelStats> {
    let (sp, stored) = super::bspec::current(s, "job").ok()?;
    if !stored {
        return None;
    }
    let d = s.doc().ok()?;
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for e in d.model.iter().take(500_000) {
        if let Some(part) = super::wallgen::made_as(e) {
            *by_kind.entry(part).or_default() += 1;
        }
    }
    Some(ModelStats { levels: sp.levels.len(), elements_by_kind: by_kind })
}

fn job_summary(s: &Session, job: &GuidedJob, stats: Option<&ModelStats>) -> Value {
    let mut results = json!({});
    for stage in &job.stages {
        if stage.status != StageStatus::Skipped {
            results[stage.name.label()] = json!({
                "status": format!("{:?}", stage.status),
                "questions_asked": stage.questions.len() + stage.verify.len() + stage.completeness.len(),
            });
        }
    }
    // Q5278 MUST 1: levels and element counts from the saved model, not session-local index.
    let levels: Value = match stats {
        Some(ms) => json!(ms.levels),
        None => index_count(job, "levels").map_or(Value::Null, |n| json!(n)),
    };
    let elements: Value = match stats {
        Some(ms) if !ms.elements_by_kind.is_empty() => {
            json!(ms.elements_by_kind.iter().map(|(k, v)| json!({"kind": k, "count": v})).collect::<Vec<_>>())
        }
        _ => Value::Null,
    };
    json!({
        "goal": job.goal,
        "stages_completed": job.stages.iter().filter(|s| s.status == StageStatus::Done).count(),
        "total_questions": job.stages.iter().map(|s| s.questions.len() + s.verify.len() + s.completeness.len()).sum::<usize>(),
        "rules_learned": job.rules.len(),
        "sheets": index_count(job, "sheets"),
        "levels": levels,
        "elements": elements,
        "answers": answers_value(job),
        // From the saved model, not the stage's own count.
        "devices": s.doc().map_or(Value::Null, super::readers::model_device_counts),
        "results": results,
    })
}

/// Learn project rules from a question's answer.
pub fn learn_from_answer(job: &mut GuidedJob, q: &Question) {
    let answer = match &q.answer {
        Some(v) => v,
        None => return,
    };

    // Learn symbol → device mappings from completeness answers.
    if q.kind == QuestionKind::Completeness
        && q.question.contains("Unknown symbol")
        && let Some(device_type) = answer.get("device_type").and_then(Value::as_str)
        && let Some(ev) = &q.evidence
        && let Some(desc) = ev.values.as_ref().and_then(|v| v.get("description")).and_then(Value::as_str)
    {
        job.learn_rule(&format!("symbol:{desc}"), &format!("device:{device_type}"), &q.id, q.stage);
    }

    // Equipment data an agent read off a scanned submittal.
    if q.stage == StageName::BimBuild
        && q.question.starts_with(EQUIP_QUESTION)
        && answer.is_object()
        && let Some(tag) = q.evidence.as_ref().and_then(|e| e.values.as_ref()).and_then(|v| v.get("equipment")).and_then(Value::as_str)
    {
        job.learn_rule(&format!("equip:{}", tag.to_ascii_uppercase()), &answer.to_string(), &q.id, q.stage);
    }

    // Gear sizes an agent read off a drawing.
    if q.stage == StageName::BimBuild
        && q.question.starts_with(GEAR_QUESTION)
        && answer.get("height_in").is_some()
        && let Some(tag) = q.evidence.as_ref().and_then(|e| e.values.as_ref()).and_then(|v| v.get("panel")).and_then(Value::as_str)
    {
        job.learn_rule(&format!("gear:{}", tag.to_ascii_uppercase()), &answer.to_string(), &q.id, q.stage);
    }

    // Learn scale overrides.
    if q.stage == StageName::Levels
        && q.question.contains("scale reads")
        && let Some(ans) = answer.as_str()
        && let Some(ev) = &q.evidence
        && let Some(sheet) = &ev.sheet
    {
        job.learn_rule(&format!("scale:{sheet}"), ans, &q.id, q.stage);
    }

    // Learn room name overrides.
    if q.stage == StageName::Rooms
        && q.question.contains("looks like equipment")
        && let Some(ans) = answer.as_str()
        && let Some(ev) = &q.evidence
        && let Some(room_name) = ev.values.as_ref().and_then(|v| v.get("room_name")).and_then(Value::as_str)
    {
        job.learn_rule(&format!("label:{room_name}"), &format!("room_override:{ans}"), &q.id, q.stage);
    }
}

/// Check if any rejected verify samples mean the stage needs to re-run.
pub fn check_rejected_samples(stage: &StageState) -> bool {
    for q in &stage.verify {
        if let Some(ans) = &q.answer {
            let text = ans.as_str().unwrap_or("");
            if text.contains("Wrong") || text.contains("Not a") {
                return true;
            }
        }
    }
    false
}

/// ISO 8601 timestamp (UTC).
fn now_iso() -> String {
    // Use web-time for wasm compat.
    let epoch = web_time::SystemTime::now().duration_since(web_time::SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = epoch.as_secs();
    // Simple ISO format without pulling in chrono.
    let days = secs / 86400;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let sec = rem % 60;
    // Approximate date from days since epoch.
    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{sec:02}Z")
}

/// Convert days since Unix epoch to (year, month, day).
pub fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Civil calendar from days: Algorithms from Howard Hinnant.
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64 + era * 400) as u64;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// `job.worklist` — the unified worklist of questions, findings, and actions (MUST 5 REFRAME).
fn cmd_worklist(s: &mut Session, p: &Value) -> Result<Value> {
    let job = load_job(s)?.ok_or_else(|| bad("job.worklist", "no job running"))?;
    let status_filter = p.get("status").and_then(Value::as_str);
    let stage_filter = p.get("stage").and_then(Value::as_str);

    let items: Vec<Value> = job
        .worklist
        .iter()
        .filter(|w| status_filter.is_none_or(|sf| w.status == sf))
        .filter(|w| stage_filter.is_none_or(|sf| w.stage.label() == sf))
        .map(|w| {
            json!({
                "id": w.id,
                "kind": w.kind,
                "stage": w.stage.label(),
                "summary": w.summary,
                "status": w.status,
                "choices": w.choices,
                "evidence": w.evidence,
                "resolution": w.resolution,
                "has_undo": w.undo_delta.is_some(),
            })
        })
        .collect();

    let pending = job.worklist.iter().filter(|w| w.status == "pending").count();
    let resolved = job.worklist.iter().filter(|w| w.status == "resolved").count();
    let total = job.worklist.len();

    Ok(json!({
        "items": items,
        "total": total,
        "pending": pending,
        "resolved": resolved,
    }))
}

/// `job.undo {id}` — undo a resolved worklist item if it has a delta (MUST 5 REFRAME).
fn cmd_undo(s: &mut Session, p: &Value) -> Result<Value> {
    let wid = str_param(p, "id").ok_or_else(|| bad("job.undo", "`id` (worklist item id) is required"))?;

    let mut job = load_job(s)?.ok_or_else(|| bad("job.undo", "no job running"))?;

    let wi = job.worklist.iter_mut().find(|w| w.id == *wid).ok_or_else(|| bad("job.undo", format!("worklist item '{wid}' not found")))?;

    if wi.status != "resolved" {
        return Err(bad("job.undo", format!("item '{wid}' is not resolved (status: {})", wi.status)));
    }

    if let Some(delta) = wi.undo_delta.take() {
        // Execute the undo command.
        if let Some(cmd) = delta.get("command").and_then(Value::as_str) {
            let params = delta.get("params").cloned().unwrap_or(json!({}));
            s.execute(cmd, &params)?;
        }
    }

    wi.status = "pending".into();
    wi.resolution = None;
    job.audit("undo", "agent", Some(wid), None, None);

    save_job(s, &job)?;
    Ok(json!({"undone": wid}))
}

// ─── Command specs ──────────────────────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("job.start", "Start Guided Job", cmd_start)
            .files()
            .params("{pdfs: path (folder of PDFs or DWGs), goal?: \"cdy\"|\"bim\"|\"takeoff\"|\"estimate\" (default)|\"clash\"|\"sequence\" (clash, then the construction sequence), auto?: bool (default false: the job stops at each checkpoint — a low-confidence reading, a count to confirm, a spot check — until job.answer; true: every question with a default takes it and the job runs to the end, except a stage that found nothing where it should, which is always asked), resume?: bool (default true: a job of the same prints and goal cut off before it finished goes on from its checkpoint), submittals?: path | [path] (submittal PDFs, or folders of them: the types they give products for are device types on the electrical sheets, and after the build each device, fixture and equipment element gets its type's product — maker, catalog number, dimming — with the submittal page as the source), specs?: path | [path] (specification section PDFs, or folders of them; default a *Spec* folder beside the submittals: each element's product record — the section's makers, rating, configuration, colour, features — with section and page as the source), checkpoint?: path | false (the job's drawing, saved after each stage; default a .cdy in the temporary folder, removed when the job is done; a path you give stays, with the finished model, and must be new or a job's checkpoint: an existing drawing is refused, never written over)} → {job_id, goal, progress, done, current_stage, stages: [{stage, status, questions, unanswered, elapsed_ms?}], elapsed_ms, open_questions, rules_learned, answers: {mode, automatic, by_you, open, note}, question?: the question the job waits on, resumed?}; progress: job.progress events (and lines on the CLI's stderr) per stage and per sheet imported"),
        CommandSpec::new("job.next", "Next Job Question", cmd_next)
            .params("{} → {question?: {id (e.g. Q-classify-1, Q-levels-4, Q-rooms-23), stage, question, why, choices, evidence, sheet?, crop?, default, kind, always_asked, if_changed: {reruns, about_s, note} | {applies_to} | null (what an answer other than the default runs again, and about how long)}, answers, done?, progress, stage?, rerun?: {stages, changed} (answers held with rerun: false ran now)}")
            .noundo(),
        CommandSpec::new("job.answer", "Answer Job Question", cmd_answer)
            .params("{id: \"Q-rooms-23\" (the question id job.next gives: Q-stage-number, as Q-levels-3; not needed with answers), answer: \"Correct\" | text | object (a choice, the default, your own value; a level question takes an elevation in feet — 12.5, or feet and inches — and a name: {elevation_ft: 12.5, name: \"Mezzanine\"}; \"Correct\" confirms it as yours), answers?: [{id, answer}] (many at once: each stands alone — an unknown or merged id is a warning and the rest are kept — and the stages they change run again once), rerun?: bool (default true; false holds the re-run until the next job.answer or job.next, to answer several questions of one stage first)} → {answered: id | [ids], effect: what the answer changes | [{id, effect}], rerun: [stages run again], changed: [{stage, what, before, after}], applies_to?: a stage still to run that will read it, rerun_held?: {from, note}, levels_changed?, warnings?: [{id, warning}], next?: question, done?, progress, answers}"),
        CommandSpec::new("job.set_mode", "Set Job Mode", cmd_set_mode)
            .params("{auto: bool (true: the running job goes on unattended, every open question with a default takes it; false: it stops at each checkpoint again)} → {mode: {from, to}, rerun?, …job.status}"),
        CommandSpec::new("job.status", "Job Status", cmd_status)
            .params("{} → {job_id, goal, progress, done, current_stage, stages: [{stage, status, questions, unanswered, elapsed_ms?}], elapsed_ms, checkpoint?, open_questions, findings, worklist_pending, rules_learned}")
            .noundo(),
        CommandSpec::new("job.explain", "Explain Job Stage", cmd_explain)
            .params("{stage?: name} → {stage, description, status, questions_total, questions_answered, result} or {stages}")
            .noundo(),
        CommandSpec::new("job.rules", "Job Learned Rules", cmd_rules)
            .params("{} → {rules: [{key, value, from_question, stage, auto_applied}], count}")
            .noundo(),
        CommandSpec::new("job.report", "Job Report", cmd_report)
            .params("{} → {job_id, goal, done, progress, stages, totals, rules_learned, audit_entries}")
            .noundo(),
        CommandSpec::new("job.worklist", "Job Worklist", cmd_worklist)
            .params("{status?: \"pending\"|\"resolved\"|\"info\", stage?: name} → {items, total, pending, resolved}")
            .noundo(),
        CommandSpec::new("job.undo", "Undo Job Action", cmd_undo)
            .params("{id: worklist item id} → {undone}"),
    ]
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    /// Q6003 MUST 4: the job's report lists the property coverage per kind.
    #[test]
    fn job_report_lists_property_coverage_per_kind() {
        use cadaddy_doc::props::{Prop, Props, Source};
        let mut s = Session::new();
        let mut d = cadaddy_doc::Drawing::new_imperial();
        for (i, number) in [Some("101A"), None].into_iter().enumerate() {
            let h = d.new_handle();
            let a = cadaddy_geom::Vec3 { x: i as f64 * 48.0, y: 0.0, z: 0.0 };
            let mut e = cadaddy_doc::Entity::new(h, EntityKind::Line(cadaddy_doc::Line { a, b: cadaddy_geom::Vec3 { x: a.x + 36.0, ..a } }));
            let tag = vec![
                cadaddy_doc::RawTag { code: 1001, value: crate::cmd::wallgen::APP.into() },
                cadaddy_doc::RawTag { code: 1000, value: "door".into() },
                cadaddy_doc::RawTag { code: 1000, value: "L1".into() },
            ];
            let mut store = Props::default();
            if let Some(n) = number {
                store.set("number", Prop::new(n, Source::print("Sample Sheet A-101", None)));
            }
            e.common.xdata = cadaddy_doc::props::write(&tag, &store).into();
            d.model.push(e);
        }
        s.open_drawing(d, "Sample Building", None);
        save_job(&mut s, &GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff)).unwrap();
        let r = s.execute("job.report", &json!({})).unwrap();
        let doors = &r["property_coverage"]["door"];
        assert_eq!(doors["elements"], 2, "{r}");
        // 10 standard door keys each: one number and two levels are filled.
        assert_eq!(doors["filled"], 3, "{doors}");
        assert_eq!(doors["possible"], 20);
        assert_eq!(doors["percent"], 15.0);
        assert!(doors["keys_with_no_value"].as_array().unwrap().iter().any(|k| k == "fire_rating"));
    }

    #[test]
    fn explain_works_before_a_job_starts() {
        let mut s = Session::new();
        let one = cmd_explain(&mut s, &json!({"stage": "classify"})).unwrap();
        assert_eq!((one["stage"].as_str(), one["status"].as_str()), (Some("classify"), Some("NotStarted")));
        assert!(one["description"].as_str().is_some_and(|d| !d.is_empty()));
        let all = cmd_explain(&mut s, &json!({})).unwrap();
        assert_eq!(all["stages"].as_array().map(Vec::len), Some(StageName::ALL.len()));
    }

    #[test]
    fn junk_tags_are_not_device_types() {
        assert!(is_circuit_tag("D-1") && is_circuit_tag("DP1-13"));
        let (none, sheets) = (std::collections::BTreeSet::new(), std::collections::BTreeSet::new());
        for t in ["A", "XA", "L4", "EF", "DP2"] {
            assert_eq!(cadaddy_mepfix::device_types::tag_verdict(t, &none, &none, &sheets), TagVerdict::Device(t.into()), "{t}");
        }
        for t in ["S2004", "s", "t", "u", "GFCI", "TYP", "a1", "ABCD"] {
            assert!(matches!(cadaddy_mepfix::device_types::tag_verdict(t, &none, &none, &sheets), TagVerdict::NotDevice(_)), "{t}");
        }
    }

    use cadaddy_doc::{Attrib, AttribModes, AttribProps, Block, Circle, Common, EntityKind, HAlign, Insert, Space, Text, VAlign};
    use cadaddy_geom::Vec3;
    use cadaddy_mepfix::device_types::TagVerdict;

    fn text(v: &str, x: f64, y: f64, h: f64) -> Text {
        Text {
            insert: Vec3::new(x, y, 0.0),
            align_pt: None,
            height: h,
            value: v.into(),
            rotation: 0.0,
            width_factor: 1.0,
            oblique: 0.0,
            style: "Standard".into(),
            halign: HAlign::default(),
            valign: VAlign::default(),
        }
    }

    /// One sheet's import: its texts and its device symbols (`PDF-SYM-1` inserts, `tag` in TAG).
    fn add_sheet(s: &mut crate::Session, texts: &[(&str, f64, f64, f64)], symbols: &[(&str, f64, f64)]) {
        let Ok(d) = s.doc_mut() else { panic!("no drawing") };
        let mut b = Block::new("PDF-SYM-1");
        b.entities.push(cadaddy_doc::Entity {
            handle: d.new_handle(),
            common: Common::default(),
            kind: EntityKind::Circle(Circle { center: Vec3::ZERO, radius: 3.0 }),
        });
        d.blocks.insert("PDF-SYM-1".into(), std::sync::Arc::new(b));
        let common = || Common { layer: "PDF".into(), ..Common::default() };
        for (v, x, y, h) in texts {
            d.add(&Space::Model, common(), EntityKind::Text(text(v, *x, *y, *h))).ok();
        }
        for (tag, x, y) in symbols {
            let attribs = if tag.is_empty() {
                Vec::new()
            } else {
                vec![Attrib {
                    tag: "TAG".into(),
                    text: text(tag, *x, *y, 2.0),
                    invisible: false,
                    constant: false,
                    prompt: String::new(),
                    props: AttribProps::default(),
                    multiline: None,
                    modes: AttribModes::default(),
                }]
            };
            let ins = Insert {
                block: "PDF-SYM-1".into(),
                insert: Vec3::new(*x, *y, 0.0),
                scale: Vec3::new(1.0, 1.0, 1.0),
                rotation: 0.0,
                attribs,
                cols: 1,
                rows: 1,
                col_spacing: 0.0,
                row_spacing: 0.0,
                clip: None,
            };
            d.add(&Space::Model, common(), EntityKind::Insert(ins)).ok();
        }
    }

    /// An enlarged guest-room plan as the hotel's E-6 prints it: a downlight's symbol the import
    /// tagged with its circuit label (`H` printed over it), a downlight the import made no symbol
    /// of (its `H` over `G-1,a` alone), a receptacle with only its circuit, one with `HOR` by it;
    /// and a plumbing sheet's fixture tag (`P-3`, like a circuit) by a grid letter.
    #[test]
    fn a_luminaires_type_mark_by_its_circuit_tagged_symbol_or_alone_is_read() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        job.sheets = Some(json!({"sheets": [{"number": "E-1", "sheet_type": "plan"}, {"number": "P-1", "sheet_type": "plan"}]}));
        job.sheets_loaded = true;
        job.luminaire_types = vec!["H".into()];
        let region = |n: &str, d: &str, x0: f64| SheetRegion {
            number: n.into(),
            level: "1".into(),
            discipline: d.into(),
            kind: "plan".into(),
            rect: [x0, 0.0, x0 + 2000.0, 1000.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        };
        // E-1: texts 4 high. The downlight at (100, 500): `H` over it, `G-1` its tag. The lone
        // downlight: `H` at (300, 506), `G-1,a` at (300, 494). Receptacles at (500, 500) and
        // (700, 500), `HOR` by the second.
        let texts: Vec<(&str, f64, f64, f64)> =
            vec![("H", 98.0, 507.0, 4.0), ("H", 300.0, 506.0, 4.0), ("G-1,a", 300.0, 494.0, 4.0), ("HOR", 704.0, 506.0, 4.0)];
        // A GFCI receptacle beside the lone downlight (13 off where its symbol would be): another
        // device, not the downlight's symbol.
        let syms: Vec<(&str, f64, f64)> = vec![("G-1", 100.0, 500.0), ("G-9", 500.0, 500.0), ("G-9", 700.0, 500.0), ("GFCI", 313.0, 497.0)];
        let lo = s.doc().map_or(0, |d| d.handseed);
        add_sheet(&mut s, &texts, &syms);
        let hi = s.doc().map_or(0, |d| d.handseed);
        job.sheet_spans.push(SheetSpan { sheet: "E-1".into(), lo, hi });
        job.sheet_regions.push(region("E-1", "E", 0.0));
        // P-1: a fixture tag `P-3` by grid letter `C`.
        let lo = s.doc().map_or(0, |d| d.handseed);
        add_sheet(&mut s, &[("C", 3002.0, 506.0, 4.0)], &[("P-3", 3000.0, 500.0)]);
        let hi = s.doc().map_or(0, |d| d.handseed);
        job.sheet_spans.push(SheetSpan { sheet: "P-1".into(), lo, hi });
        job.sheet_regions.push(region("P-1", "P", 3000.0));
        // L-1: a landscape plan's key `H` by a symbol (no device legend of its own): no device.
        let lo = s.doc().map_or(0, |d| d.handseed);
        add_sheet(&mut s, &[], &[("H", 6000.0, 500.0)]);
        let hi = s.doc().map_or(0, |d| d.handseed);
        job.sheet_spans.push(SheetSpan { sheet: "L-1".into(), lo, hi });
        job.sheet_regions.push(region("L-1", "L", 6000.0));
        let r = run_devices(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        let on = |sheet: &str| -> Vec<(String, String)> {
            let mut v: Vec<(String, String)> =
                job.devices.iter().filter(|d| d.sheet == sheet && d.tag != "GFCI").map(|d| (d.tag.clone(), d.circuit.clone())).collect();
            v.sort();
            v
        };
        assert_eq!(
            on("E-1"),
            vec![
                ("H".to_string(), "G-1".to_string()),
                ("H".to_string(), "G-1".to_string()),
                ("REC".into(), "G-9".into()),
                ("REC".into(), "G-9".into())
            ],
            "{r:#}"
        );
        assert_eq!((r["circuit_tagged_retyped"].clone(), r["luminaires_by_mark_alone"].clone()), (json!(1), json!(1)), "{r:#}");
        assert!(on("P-1").iter().all(|(t, _)| t != "C"), "a plumbing tag isn't retyped: {:?}", on("P-1"));
        assert!(on("L-1").is_empty(), "another trade's sheet: {:?}", on("L-1"));
    }

    /// A registered enlarged view's devices move onto the overall plan where its callout points;
    /// a typical room's view's don't: they are stamped into every room of the type.
    #[test]
    fn a_typical_rooms_view_keeps_its_devices_for_every_room_of_its_type() {
        let job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        let view = |title: &str, h: &str| cadaddy_mepfix::printlinks::Enlarged {
            sheet: "E-6".into(),
            title: title.into(),
            registered: true,
            devices: 1,
            device_handles: vec![h.into()],
            fit: Some(cadaddy_mepfix::tagid::Sim { tx: 1000.0, ..cadaddy_mepfix::tagid::Sim::IDENTITY }),
            ..Default::default()
        };
        let links = cadaddy_mepfix::printlinks::SheetLinks {
            enlarged: vec![view("ENLARGED DOUBLE QUEEN", "A1"), view("ENLARGED FLOOR PLAN - LOBBY", "B2")],
            ..Default::default()
        };
        let dev = |h: &str| super::super::readers::ReadDevice {
            tag: "H".into(),
            at: [10.0, 10.0],
            sheet: "E-6".into(),
            handle: h.into(),
            ..Default::default()
        };
        let mut census = DeviceCensus { placed: vec![dev("A1"), dev("B2")], ..Default::default() };
        let (moved, _) = enlarged_authority(&job, &links, &mut census);
        assert_eq!(moved, 1);
        let at = |h: &str| census.placed.iter().find(|d| d.handle == h).map(|d| d.at);
        assert_eq!((at("A1"), at("B2")), (Some([10.0, 10.0]), Some([1010.0, 10.0])));
    }

    /// A unit's PTAC no plan connects (its feed only a keynote) takes its connection on the circuit
    /// the typical panel's schedule names it on; a fan the plans connect gets no second one.
    #[test]
    fn a_unit_ptac_is_connected_on_its_panel_schedules_circuit() {
        let mut sp = cadaddy_walk3d::BuildingSpec::default();
        sp.mep.panel_schedules = vec![cadaddy_walk3d::PanelSchedule {
            panel: "G".into(),
            typical_of: Some(124),
            source: "E-2".into(),
            circuits: vec![
                cadaddy_walk3d::PanelCircuit { number: "11".into(), description: "RANGE EXHAUST (ADA ONLY)".into(), poles: 1, ..Default::default() },
                cadaddy_walk3d::PanelCircuit { number: "13".into(), description: "GUESTROOM PTAC-1".into(), poles: 2, ..Default::default() },
            ],
            ..Default::default()
        }];
        let unit = |kind: &str, at: f64| {
            let mut e = cadaddy_walk3d::MepEquipment {
                kind: kind.into(),
                level: "Level 2".into(),
                min: [at, 0.0],
                max: [at + 40.0, 12.0],
                ..Default::default()
            };
            e.properties.insert("room".into(), "202".into());
            e
        };
        sp.mep.equipment = vec![unit("ptac", 100.0), unit("exhaust_fan", 300.0)];
        let fan = super::super::readers::ReadDevice { tag: "EF".into(), room: "202".into(), ..Default::default() };
        let got = scheduled_connections(&sp.mep.panel_schedules, &sp.mep.equipment, &[fan]);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!((got[0].tag.as_str(), got[0].circuit.as_str(), got[0].room.as_str(), got[0].at), ("PTAC", "G-13,15", "202", [120.0, 6.0]));
        assert!(got[0].kind_source.contains("GUESTROOM PTAC-1"));
    }

    /// A water closet in no room but inside a unit's footprint (its bath: the unit's label's room
    /// leaves it out) is the unit's; one outside every footprint, or on another floor, stays out.
    #[test]
    fn a_device_in_a_units_bath_is_booked_to_the_unit() {
        let v = |x: f64, y: f64| cadaddy_geom::Vec2::new(x, y);
        let quad = |x0: f64, y0: f64, x1: f64, y1: f64| vec![v(x0, y0), v(x1, y0), v(x1, y1), v(x0, y1)];
        // Two units back to back at x = 144: their footprints overlap 12 past the shared wall; their
        // bedrooms (the label-traced rooms) north of the baths.
        let foot = vec![
            ("2".to_string(), "202".to_string(), quad(0.0, 0.0, 156.0, 348.0), quad(0.0, 100.0, 144.0, 348.0)),
            ("2".to_string(), "204".to_string(), quad(132.0, 0.0, 300.0, 348.0), quad(144.0, 100.0, 300.0, 348.0)),
        ];
        let wc = |x: f64, y: f64, level: &str| super::super::readers::ReadDevice {
            tag: "P-1".into(),
            kind: "plumbing fixture".into(),
            sheet: "P-2".into(),
            at: [x, y],
            level: level.into(),
            ..Default::default()
        };
        let mut devices = vec![
            wc(40.0, 30.0, "Level 2"),
            wc(400.0, 30.0, "Level 2"),
            wc(40.0, 30.0, "Level 3"),
            wc(150.0, 60.0, "Level 2"),
            wc(138.0, 60.0, "Level 2"),
        ];
        let own: BTreeMap<String, BTreeSet<String>> =
            [("202".to_string(), ["P-1@P".to_string()].into()), ("204".to_string(), ["P-1@P".to_string()].into())].into();
        let none = BTreeMap::new();
        assert_eq!(book_into_units(&mut devices, &foot, &own, &none, 0.0), 3);
        // Read before the unit's own (only in its footprint): a water closet at the shared wall
        // still goes to the unit that has none, whatever order the plans list them in.
        let mut order = vec![wc(138.0, 60.0, "Level 2"), wc(40.0, 30.0, "Level 2")];
        assert_eq!(book_into_units(&mut order, &foot, &own, &none, 0.0), 2);
        assert_eq!(order.iter().map(|d| d.room.as_str()).collect::<Vec<_>>(), vec!["204", "202"]);
        // The second unit's footprint fitted short of its own bath (from 160): its water closet at
        // 150, only in the first's footprint, is still its own, not the first's second.
        let short = vec![foot[0].clone(), ("2".to_string(), "204".to_string(), quad(160.0, 0.0, 300.0, 348.0), quad(160.0, 100.0, 300.0, 348.0))];
        let mut back = vec![wc(150.0, 60.0, "Level 2"), wc(138.0, 60.0, "Level 2")];
        assert_eq!(book_into_units(&mut back, &short, &own, &none, 36.0), 2);
        assert_eq!(back.iter().map(|d| d.room.as_str()).collect::<Vec<_>>(), vec!["204", "202"]);
        // Out of reach it stays where it was booked.
        let mut far = vec![wc(150.0, 60.0, "Level 2"), wc(138.0, 60.0, "Level 2")];
        assert_eq!(book_into_units(&mut far, &short, &own, &none, 6.0), 2);
        assert_eq!(far.iter().map(|d| d.room.as_str()).collect::<Vec<_>>(), vec!["202", "202"]);
        // A smoke detector off the fire alarm plan is the unit's; a corridor's sconce by the door,
        // no tag the unit's plans show, stays out.
        let mut fa = vec![super::super::readers::ReadDevice {
            tag: "S".into(),
            kind: "fire alarm device".into(),
            at: [40.0, 30.0],
            level: "Level 2".into(),
            ..Default::default()
        }];
        assert_eq!(book_into_units(&mut fa, &foot, &own, &none, 0.0), 1);
        let mut g = vec![super::super::readers::ReadDevice { tag: "G".into(), at: [40.0, 30.0], level: "Level 2".into(), ..Default::default() }];
        assert_eq!(book_into_units(&mut g, &foot, &own, &none, 0.0), 0);
        // A receptacle on the unit's panel is the unit's; one on a house panel's circuit (the
        // corridor's, across the entry wall) is not.
        let own: BTreeMap<String, BTreeSet<String>> = [("202".to_string(), ["REC@E".to_string(), "F@E".to_string()].into())].into();
        let fed: BTreeMap<String, BTreeSet<String>> = [("202".to_string(), ["G".to_string()].into())].into();
        let rec = |circuit: &str| super::super::readers::ReadDevice {
            tag: "REC".into(),
            circuit: circuit.into(),
            sheet: "E-3".into(),
            at: [40.0, 30.0],
            level: "Level 2".into(),
            ..Default::default()
        };
        // An `F` off a mechanical plan isn't the unit's electrical wall light.
        let f_on = |sheet: &str| super::super::readers::ReadDevice {
            tag: "F".into(),
            sheet: sheet.into(),
            at: [40.0, 30.0],
            level: "Level 2".into(),
            ..Default::default()
        };
        let mut recs = vec![rec("G-9"), rec("A-3"), rec(""), f_on("M-2"), f_on("E-3")];
        assert_eq!(book_into_units(&mut recs, &foot, &own, &fed, 0.0), 3);
        assert_eq!(recs.iter().map(|d| d.room.as_str()).collect::<Vec<_>>(), vec!["202", "", "202", "", "202"]);
        assert_eq!(devices.iter().map(|d| d.room.as_str()).collect::<Vec<_>>(), vec!["202", "", "", "204", "202"]);
    }

    /// A room's two copies of a tag at one spot are one; the same tag a foot away, another room's
    /// at that spot, or one on another circuit, stay.
    #[test]
    fn one_device_per_room_tag_and_spot() {
        let dv = |tag: &str, room: &str, x: f64| super::super::readers::ReadDevice {
            tag: tag.into(),
            room: room.into(),
            at: [x, 0.0],
            level: "Level 2".into(),
            ..Default::default()
        };
        let mut d = vec![dv("S", "219", 10.0), dv("S", "219", 10.5), dv("S", "219", 22.0), dv("S", "221", 10.0), dv("SD", "219", 10.0)];
        assert_eq!(one_per_spot(&mut d, 3.0), 1);
        assert_eq!(d.len(), 4);
        // Two receptacles side by side on two circuits are two.
        let on = |mut d: super::super::readers::ReadDevice, c: &str| {
            d.circuit = c.into();
            d
        };
        let mut r = vec![on(dv("REC", "219", 10.0), "G-5"), on(dv("REC", "219", 10.2), "G-6"), on(dv("REC", "219", 10.1), "G-5")];
        assert_eq!(one_per_spot(&mut r, 3.0), 1);
        assert_eq!(r.iter().map(|d| d.circuit.as_str()).collect::<Vec<_>>(), vec!["G-5", "G-6"]);
    }

    /// A fan two plans tag (the electrical plan's EF and the mechanical plan's) is one connection
    /// per room; two rooms' fans are two.
    #[test]
    fn one_connection_per_rooms_equipment() {
        let dv = |tag: &str, room: &str| super::super::readers::ReadDevice {
            tag: tag.into(),
            room: room.into(),
            level: "Level 1".into(),
            ..Default::default()
        };
        let mut stamped = vec![dv("EF", "103"), dv("EF", "105"), dv("REC", "103"), dv("REC", "103")];
        let mut placed = vec![dv("EF", "103"), dv("PTAC", "103")];
        assert_eq!(one_connection_each(&mut stamped, &mut placed, 12.0), 1);
        assert_eq!((stamped.len(), placed.len()), (4, 1));
        // The floor plans' tags of a unit's fan, put at it (in no room), are its one; a corridor's
        // fan elsewhere stays.
        let at = |mut d: super::super::readers::ReadDevice, x: f64| {
            d.at = [x, 0.0];
            d
        };
        let mut stamped = vec![at(dv("EF", "103"), 100.0)];
        let mut placed = vec![at(dv("EF", ""), 100.0), at(dv("EF", ""), 104.0), at(dv("EF", ""), 400.0)];
        assert_eq!(one_connection_each(&mut stamped, &mut placed, 12.0), 2);
        assert_eq!(placed.iter().map(|d| d.at[0]).collect::<Vec<_>>(), vec![400.0]);
    }

    /// The note as a sheet prints it: its heading, then each item's name and height in two
    /// columns, down to the next numbered note.
    #[test]
    fn a_two_column_mounting_height_note_is_read_row_by_row() {
        let mut d = cadaddy_doc::Drawing::default();
        let rows: [(&str, f64, f64); 9] = [
            ("17.ELECTRICAL DEVICE MOUNTED HEIGHTS (UNO):", 34.6, 17.5),
            ("PANELBOARDS", 35.1, 17.4),
            ("6'-6\" AFF TO TOP", 36.8, 17.4),
            ("TOGGLE SWITCH", 35.1, 17.2),
            ("4'-0\" AFF TO TOP", 36.8, 17.2),
            ("RECEPTACLES", 35.1, 17.1),
            ("1'-6\" CENTERLINE", 36.8, 17.1),
            ("18.THERMOSTATS SHALL BE 5'-0\" AFF", 34.6, 16.9),
            ("TV OUTLETS 6'-0\" AFF", 35.1, 16.7),
        ];
        for (v, x, y) in rows {
            let t = cadaddy_doc::Text {
                insert: cadaddy_geom::Vec3::new(x, y, 0.0),
                align_pt: None,
                height: 0.1,
                value: v.into(),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: cadaddy_doc::HAlign::default(),
                valign: cadaddy_doc::VAlign::default(),
            };
            d.add(&cadaddy_doc::Space::Model, cadaddy_doc::Common::default(), cadaddy_doc::EntityKind::Text(t)).ok();
        }
        let got: Vec<(&str, f64)> = typical_mounting_heights(&[("E-1".into(), d)]).iter().map(|t| (t.kind, t.inches)).collect();
        // The next note's thermostat and what follows it are not this note's.
        assert_eq!(got, vec![("panel", 75.75), ("switch", 45.75), ("receptacle", 18.0)]);
    }

    /// A detector's subscript `R` the import hung on no symbol (a lone letter among the sheet's
    /// texts at its `S`'s lower right) still makes it `SR`; a letter at its left (`P` by a pull
    /// station) or one already used does not.
    #[test]
    fn a_subscript_left_as_free_text_still_joins_its_mark() {
        use cadaddy_geom::Vec2;
        use cadaddy_mepfix::sheet_facts::TextItem;
        let sym = |t: &str, x: f64, n: u64| (t.to_string(), Vec2::new(x, 0.0), cadaddy_doc::Handle(n), Some((Vec2::new(x, 0.0), 1.0)));
        let syms = vec![sym("S", 0.0, 1), sym("S", 10.0, 2), sym("S", 20.0, 3), sym("S", 30.0, 4), sym("R", 30.9, 5)];
        let text = |v: &str, x: f64, y: f64| TextItem { value: v.into(), at: Vec2::new(x, y), height: 1.0, rotation: 0.0 };
        // An R at the first S, a P at the left of the second, nothing by the third; the fourth's R
        // is a symbol's tag (at its lower right).
        let free = vec![text("R", 0.9, -0.8), text("P", 8.5, -0.8)];
        let syms = {
            let mut v = syms;
            v[4].3 = Some((Vec2::new(30.9, -0.8), 1.0));
            v
        };
        let (subs, compound) = fire_alarm_subscripts(&syms, &free);
        assert_eq!(compound.get(&cadaddy_doc::Handle(1)).map(String::as_str), Some("SR"));
        assert_eq!(compound.get(&cadaddy_doc::Handle(4)).map(String::as_str), Some("SR"));
        assert!(!compound.contains_key(&cadaddy_doc::Handle(2)) && !compound.contains_key(&cadaddy_doc::Handle(3)));
        assert_eq!(subs.into_iter().collect::<Vec<_>>(), vec![cadaddy_doc::Handle(5)]);
    }

    /// A fire alarm symbol list as E-7 prints it: each row's letters (an `S` with its subscript
    /// `R`), description and mounting; a mark two rows give is left out.
    #[test]
    fn a_fire_alarm_symbol_list_names_marks_and_mounts() {
        let mut d = cadaddy_doc::Drawing::default();
        let h = 18.3;
        let rows: Vec<(&str, f64, f64, f64)> = vec![
            ("MOUNTING HEIGHTS TO BOTTOM OF ITEM UNLESS NOTED OTHERWISE.", 30000.0, 3500.0, h),
            ("SYMBOL", 34300.8, 2958.7, h),
            ("DESCRIPTION", 34677.8, 2958.7, h),
            ("MOUNTING", 35303.0, 2958.7, h),
            ("P", 34343.4, 2889.6, h),
            ("FIRE ALARM MANUAL PULL STATION", 34454.7, 2890.2, h),
            ("4'-0\" TO TOP", 35281.0, 2890.2, h),
            ("COMBINATION F.A. HORN & STROBE SIGNAL", 34454.7, 2836.5, h),
            ("WALL 80\" A.F.F.", 35281.0, 2836.5, h),
            ("S", 34343.4, 2727.4, h),
            ("AREA SMOKE DETECTOR", 34454.7, 2728.3, h),
            ("CEIL./WALL", 35281.0, 2728.3, h),
            ("S", 34340.5, 2619.5, h),
            ("R", 34372.8, 2613.1, h),
            ("RESIDENT ROOM SMOKE DETECTOR AND SOUNDER BASE", 34454.7, 2620.5, h),
            ("CEIL./WALL", 35281.0, 2620.5, h),
            ("H", 34338.6, 2566.4, h),
            ("AREA HEAT DETECTOR", 34454.7, 2566.4, h),
            ("CEIL./WALL", 35281.0, 2566.4, h),
            ("H", 34342.4, 2511.4, h),
            ("HEARING IMPAIRED HORN & STROBE SIGNAL", 34454.7, 2512.3, h),
            ("WALL 80\" A.F.F.", 35281.0, 2512.3, h),
            ("WS", 34343.4, 2457.0, h),
            ("WEATHERPROOF STROBE", 34454.7, 2458.0, h),
            ("WALL 80\" A.F.F.", 35281.0, 2458.0, h),
        ];
        for (v, x, y, ht) in rows {
            let t = cadaddy_doc::Text {
                insert: cadaddy_geom::Vec3::new(x, y, 0.0),
                align_pt: None,
                height: ht,
                value: v.into(),
                rotation: 0.0,
                width_factor: 1.0,
                oblique: 0.0,
                style: "Standard".into(),
                halign: cadaddy_doc::HAlign::default(),
                valign: cadaddy_doc::VAlign::default(),
            };
            d.add(&cadaddy_doc::Space::Model, cadaddy_doc::Common::default(), cadaddy_doc::EntityKind::Text(t)).ok();
        }
        let rows = symbol_list_rows(&[("E-7".into(), d)]);
        let got: Vec<(&str, bool, Option<f64>)> = rows.iter().map(|r| (r.mark.as_str(), r.ceiling, r.inches)).collect();
        assert_eq!(got, vec![("P", false, Some(45.75)), ("S", true, None), ("SR", true, None), ("WS", false, Some(82.25))], "{rows:#?}");
        let mut sp = cadaddy_walk3d::BuildingSpec::default();
        let tag = |id: &str, mark: &str| {
            let mut t = cadaddy_walk3d::tags::ElementTag { id: id.into(), kind: "fire_alarm_device".into(), ..Default::default() };
            t.set("type", mark, "plan");
            t
        };
        sp.tags = vec![tag("device:p", "P"), tag("device:r", "SR"), tag("device:h", "H")];
        assert_eq!(apply_symbol_list(&mut sp, &rows), 2, "H is either of two rows");
        let f = |id: &str, k: &str| sp.tags.iter().find(|t| t.id == id).and_then(|t| t.fields.get(k)).map(|f| f.value.clone());
        assert_eq!(f("device:p", "mount_height_in").as_deref(), Some("45.75"));
        assert_eq!(f("device:r", "mount").as_deref(), Some("ceiling"));
        assert_eq!(f("device:r", "device_type").as_deref(), Some("resident room smoke detector and sounder base SR"));
        assert!(f("device:h", "device_type").is_none());
    }

    /// A unit's trace that leaked through the wall into its neighbour (two units wide) is cut back
    /// to its own half; the neighbour, and a room no smaller one is inside, stay.
    #[test]
    fn a_room_trace_that_took_in_its_neighbour_is_cut_back() {
        let v = |x: f64, y: f64| cadaddy_geom::Vec2::new(x, y);
        let rect = |x0: f64, x1: f64| vec![v(x0, 0.0), v(x1, 0.0), v(x1, 360.0), v(x0, 360.0)];
        let mut rooms: Vec<JobRoom> = vec![
            ("DOUBLE Q".into(), "3".into(), rect(0.0, 144.0), "326".into()),
            ("DOUBLE Q".into(), "3".into(), rect(6.0, 294.0), "328".into()),
            ("DOUBLE Q".into(), "3".into(), rect(300.0, 444.0), "330".into()),
            ("LOBBY".into(), "1".into(), rect(0.0, 900.0), "141".into()),
        ];
        trim_swallowed(&mut rooms);
        let xs = |i: usize| rooms.get(i).map(|r| r.2.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, p| (a.0.min(p.x), a.1.max(p.x))));
        assert_eq!(xs(1), Some((144.0, 294.0)), "328 cut back to its own half");
        assert_eq!(xs(0), Some((0.0, 144.0)));
        assert_eq!(xs(2), Some((300.0, 444.0)));
        assert_eq!(xs(3), Some((0.0, 900.0)), "another level's room");
    }

    /// A general note's typical mounting heights, item by item, to each one's centre.
    #[test]
    fn a_general_notes_typical_heights_are_read_item_by_item() {
        assert_eq!(typical_item("TOGGLE SWITCH 4'-0\" AFF TO TOP", "E-1").map(|t| (t.kind, t.inches)), Some(("switch", 45.75)));
        assert_eq!(typical_item("RECEPTACLES 1'-6\" CENTERLINE", "E-1").map(|t| (t.kind, t.inches)), Some(("receptacle", 18.0)));
        assert_eq!(typical_item("VOICE/DATA OUTLETS 1'-6\" CENTERLINE", "E-1").map(|t| t.kind), Some("low_voltage_outlet"));
        assert_eq!(typical_item("EMERGENCY LIGHTS 7'-6\" AFF TO BOTTOM", "E-1").map(|t| (t.kind, t.inches)), Some(("emergency light", 92.25)));
        assert_eq!(typical_item("PANELBOARDS 6'-6\" AFF TO TOP", "E-1").map(|t| (t.kind, t.given_in, t.to)), Some(("panel", 78.0, "top")));
        let mut sp = cadaddy_walk3d::BuildingSpec::default();
        let tag = |id: &str, kind: &str, mark: &str| {
            let mut t = cadaddy_walk3d::tags::ElementTag { id: id.into(), kind: kind.into(), ..Default::default() };
            t.set("type", mark, "plan");
            t
        };
        let mut noted = tag("device:b", "receptacle", "REC");
        noted.set("mount_height_in", "44", "note");
        sp.tags = vec![tag("device:a", "receptacle", "REC"), noted, tag("device:c", "switch", "S"), tag("device:d", "device", "E")];
        sp.mep.fixture_schedule = vec![cadaddy_walk3d::FixtureType { tag: "E".into(), description: "EMERGENCY LIGHT".into(), ..Default::default() }];
        let typical: Vec<TypicalHeight> =
            ["RECEPTACLES 1'-6\" CENTERLINE", "TOGGLE SWITCH 4'-0\" AFF TO TOP", "EMERGENCY LIGHTS 7'-6\" AFF TO BOTTOM"]
                .iter()
                .filter_map(|i| typical_item(i, "E-1"))
                .collect();
        assert_eq!(apply_typical_heights(&mut sp, &typical), 3);
        let h = |id: &str| sp.tags.iter().find(|t| t.id == id).and_then(|t| t.fields.get("mount_height_in")).map(|f| f.value.clone());
        assert_eq!(
            (h("device:a"), h("device:b"), h("device:c"), h("device:d")),
            (Some("18".into()), Some("44".into()), Some("45.75".into()), Some("92.25".into()))
        );
    }

    /// A made-up two-sheet job: sheet Q-1 has a fixture schedule (types A, B) beside its plan;
    /// sheet Q-2 lies over the same paper area with no legend. Its symbols over Q-1's schedule
    /// are devices; Q-1's own sample symbol in the schedule isn't.
    #[test]
    fn device_types_come_from_the_legend_and_schedules() {
        let tmp = std::env::temp_dir();
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        job.sheets = Some(
            json!({"sheets": [{"number": "Q-1", "sheet_type": "plan"}, {"number": "Q-2", "sheet_type": "plan"}, {"number": "E1", "sheet_type": "schedule"}]}),
        );
        job.sheets_loaded = true;
        let schedule: Vec<(&str, f64, f64, f64)> = vec![
            ("LIGHTING FIXTURE SCHEDULE", 1000.0, 800.0, 8.0),
            ("TYPE", 1000.0, 785.0, 5.0),
            ("DESCRIPTION", 1030.0, 785.0, 5.0),
            ("A", 1000.0, 772.0, 5.0),
            ("2X4 LED TROFFER", 1030.0, 772.0, 5.0),
            ("B", 1000.0, 760.0, 5.0),
            ("LED DOWNLIGHT", 1030.0, 760.0, 5.0),
        ];
        let mut q1: Vec<(&str, f64, f64)> = vec![("A", 990.0, 772.0)]; // the schedule's sample symbol
        q1.extend((0..5).map(|i| ("A", 100.0 + 40.0 * f64::from(i), 300.0)));
        q1.extend((0..3).map(|i| ("B", 100.0 + 40.0 * f64::from(i), 200.0)));
        q1.extend((0..4).map(|i| ("ZZ", 100.0 + 40.0 * f64::from(i), 100.0))); // unlisted, repeated
        q1.extend([("QQ", 500.0, 100.0), ("s", 520.0, 100.0), ("s", 540.0, 100.0), ("12", 560.0, 100.0)]); // stray, switch legs, keynote
        q1.extend([("D-1", 600.0, 100.0), ("E1", 620.0, 100.0), ("", 640.0, 100.0), ("OS", 660.0, 100.0)]); // circuit, sheet number, untagged, known mark
        for (i, (texts, syms, name)) in
            [(schedule, q1, "Q-1"), (Vec::new(), vec![("A", 1000.0, 772.0), ("A", 1010.0, 760.0)], "Q-2")].into_iter().enumerate()
        {
            let lo = s.doc().map_or(0, |d| d.handseed);
            add_sheet(&mut s, &texts, &syms);
            let hi = s.doc().map_or(0, |d| d.handseed);
            assert!(hi > lo, "sheet {i}");
            job.sheet_spans.push(SheetSpan { sheet: name.into(), lo, hi });
        }
        let r = run_devices(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        let dev = &r["devices"];
        assert_eq!(dev["by_type"], json!({"A": 7, "B": 3, "OS": 1}), "{r:#}");
        assert_eq!(dev["count"], json!(11));
        let dt = &r["device_types"];
        assert_eq!(dt["legend_types"], json!(["A", "B"]));
        assert_eq!(dt["legend_samples"], json!(1));
        assert_eq!(dt["untagged_symbols"], json!(1));
        assert_eq!(dt["not_devices"], json!({"lettering": 3, "sheet number": 1, "unlisted, printed once or twice": 1}), "{dt:#}");
        assert_eq!(r["circuit_tagged_symbols"], json!(1));
        // ZZ: a question with a crop of the first symbol printing it.
        let unk = dev["unknowns"].as_array().cloned().unwrap_or_default();
        assert_eq!(unk.len(), 1, "{unk:?}");
        assert_eq!(unk[0]["tag"], json!("ZZ"));
        assert_eq!(unk[0]["count"], json!(4));
        assert_eq!(unk[0]["sheet"], json!("Q-1"));
        let crop = unk[0]["crop_path"].as_str().unwrap_or_else(|| panic!("no crop: {unk:?}"));
        assert!(std::path::Path::new(crop).starts_with(&tmp), "{crop}");
        let png = std::fs::read(crop).unwrap_or_else(|e| panic!("{crop}: {e}"));
        assert!(png.starts_with(b"\x89PNG"), "{crop}");
        // Q6031: crop is >= 800 px.
        assert!(png.len() >= 24, "PNG too short for IHDR");
        let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        assert!(w >= 800 && h >= 800, "crop too small: {w}x{h}");
        // Q6031: crop filename includes sheet info.
        assert!(crop.contains("Q_1") || crop.contains("Q-1"), "crop should mention sheet: {crop}");
        let stage = job.stage_mut(StageName::Devices).map(|st| st.completeness.clone()).unwrap_or_default();
        let q = stage.iter().find(|q| q.question.contains("'ZZ'")).unwrap_or_else(|| panic!("no ZZ question"));
        assert!(q.question.contains("no legend or schedule lists it"), "{}", q.question);
        assert_eq!(q.evidence.as_ref().and_then(|e| e.crop_png_path.as_deref()), Some(crop));
        assert_eq!(q.evidence.as_ref().map_or(0, |e| e.handles.len()), 4);
        // Q6031: unknown-tag questions always have choices.
        assert!(!q.choices.is_empty(), "unknown-tag question must have choices");
        assert!(q.choices.iter().any(|c| c == "skip"), "must offer 'skip'");
        assert!(q.choices.iter().any(|c| c.contains("not a device")), "must offer 'not a device'");
        // Q6031: unknown-tag evidence includes sheet info.
        assert_eq!(q.evidence.as_ref().and_then(|e| e.sheet.as_deref()), Some("Q-1"));
        // Answering it makes ZZ a device type on the recount.
        let mut answered = q.clone();
        answered.answer = Some(json!({"device_type": "wall_sconce"}));
        learn_from_answer(&mut job, &answered);
        let r = run_devices(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r["devices"]["by_type"]["ZZ"], json!(4), "{r:#}");
        assert_eq!(r["devices"]["count"], json!(15));
        assert_eq!(r["devices"]["unknowns"], json!([]));
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("cadaddy-{}", job.id)));
    }

    /// Q6007: a made-up electrical plan (level 1, inches) with a fixture schedule, three A
    /// fixtures on circuit D-7 (a homerun arrow at the last one), two B fixtures on D-9 (no
    /// homerun), and panel D drawn as a box lettered D. The job reads the circuits, routes
    /// each (estimated), and the takeoff has their conduit, wire and boxes by level and circuit.
    #[test]
    fn circuits_come_out_of_the_job_as_conduit_and_wire() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        job.sheets = Some(json!({"sheets": [{"number": "E-1", "sheet_type": "plan", "level": "1", "discipline": "E"}]}));
        job.sheets_loaded = true;
        let mut texts: Vec<(&str, f64, f64, f64)> = vec![
            ("LIGHTING FIXTURE SCHEDULE", 1400.0, 800.0, 8.0),
            ("TYPE", 1400.0, 785.0, 5.0),
            ("DESCRIPTION", 1430.0, 785.0, 5.0),
            ("A", 1400.0, 772.0, 5.0),
            ("2X4 LED TROFFER", 1430.0, 772.0, 5.0),
            ("B", 1400.0, 760.0, 5.0),
            ("LED DOWNLIGHT", 1430.0, 760.0, 5.0),
            // The homerun's label, and panel D's lettering above its box.
            ("D-7", 532.0, 328.0, 6.0),
            ("D", 996.0, 110.0, 6.0),
        ];
        let mut syms: Vec<(&str, f64, f64)> = vec![("A", 1390.0, 772.0)];
        for x in [100.0, 300.0, 500.0] {
            syms.push(("A", x, 300.0));
            texts.push(("D-7", x + 8.0, 296.0, 2.0));
        }
        for x in [100.0, 300.0] {
            syms.push(("B", x, 600.0));
            texts.push(("D-9", x + 8.0, 596.0, 2.0));
        }
        let lo = s.doc().map_or(0, |d| d.handseed);
        add_sheet(&mut s, &texts, &syms);
        s.execute("line", &json!({"points": [[500, 300], [530, 330]]})).unwrap_or_else(|e| panic!("{e}"));
        s.execute("pline", &json!({"points": [[988, 94], [1012, 94], [1012, 106], [988, 106]], "closed": true})).unwrap_or_else(|e| panic!("{e}"));
        let hi = s.doc().map_or(0, |d| d.handseed);
        job.sheet_spans.push(SheetSpan { sheet: "E-1".into(), lo, hi });
        job.sheet_regions.push(SheetRegion {
            number: "E-1".into(),
            level: "1".into(),
            discipline: "E".into(),
            kind: "plan".into(),
            rect: [0.0, 0.0, 2000.0, 1000.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        });
        let r = run_devices(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r["devices"]["count"], json!(5), "{r:#}");
        assert_eq!(job.devices.iter().filter(|d| d.circuit == "D-7").count(), 3, "{:?}", job.devices);

        let m = run_mep_read_stage(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        let c = &m["conduit"];
        assert_eq!((c["circuits"].clone(), c["homeruns"].clone(), c["sheets"][0]["panels"].clone()), (json!(2), json!(1), json!(["D"])), "{c:#}");
        assert!(m["trade_summary"].as_str().unwrap_or_default().contains("2 circuit(s) on 1 electrical plan(s)"), "{m:#}");
        // D-7: 400 units of chain + 700 of home run; D-9: 200 + 1200 (12 units a foot).
        assert!((c["estimated_ft"].as_f64().unwrap_or(0.0) - 2500.0 / 12.0).abs() < 0.1, "{c:#}");

        let built = run_bim_build(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(built["circuits"], json!(2), "{built:#}");
        let d = s.doc().unwrap_or_else(|e| panic!("{e}"));
        let sp = cadaddy_walk3d::BuildingSpec::from_json(d.building_spec.as_deref().unwrap_or("")).unwrap_or_else(|e| panic!("{e}"));
        let tags: Vec<&cadaddy_walk3d::tags::ElementTag> = sp.tags.iter().filter(|t| t.kind == "circuit").collect();
        assert_eq!(tags.len(), 2);
        assert!(tags.iter().all(|t| t.fields["route"].value == super::super::printcond::ESTIMATED && t.fields["panel_at"].value == "E-1"));
        let runs: Vec<&cadaddy_walk3d::MepRun> = sp.mep.runs.iter().filter(|r| r.system == super::super::printcond::CIRCUIT_SYSTEM).collect();
        assert_eq!(runs.len(), 5, "two home runs and three legs: {runs:#?}");
        let ceiling = sp.level("Level 1").map_or(0.0, |l| l.ceiling_ft);
        assert!(runs.iter().all(|r| r.bottoms_ft.iter().all(|b| (b - ceiling - 0.25).abs() < 1e-9)), "just above the ceiling: {runs:#?}");

        // The BOM by circuit: conduit (an estimated route, marked), wire, boxes.
        let bom = s.execute("bim.bom", &json!({"group_by": "circuit"})).unwrap_or_else(|e| panic!("{e}"));
        let lines: Vec<&Value> = bom["lines"].as_array().into_iter().flatten().filter(|l| l["circuit"] == "D-7").collect();
        let get = |item: &str| lines.iter().find(|l| l["item"] == item).copied().unwrap_or(&Value::Null);
        let conduit = get("EMT conduit");
        assert!((conduit["net"].as_f64().unwrap_or(0.0) - 1100.0 / 12.0).abs() < 0.01, "{lines:#?}");
        assert_eq!(conduit["status"], json!("rule"));
        assert!(conduit["rule"].as_str().unwrap_or_default().contains("estimated route"));
        // 2#12 + 1#12G (the default) over 91.67' plus 1' at each end of 3 pulls.
        let wire = lines.iter().find(|l| l["item"] == "THHN copper" && l["size"] == "#12").copied().unwrap_or(&Value::Null);
        assert!((wire["net"].as_f64().unwrap_or(0.0) - 2.0 * (1100.0 / 12.0 + 6.0)).abs() < 0.01, "{lines:#?}");
        assert_eq!(get("Box, 4\" octagon")["net"], json!(3.0), "one box per fixture on the circuit");
        assert!(get("EMT connector")["net"].as_f64().unwrap_or(0.0) >= 6.0);

        // The takeoff stage: by level and circuit.
        let t = run_takeoff(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        let d9 = t["circuits"].as_array().into_iter().flatten().find(|c| c["circuit"] == "D-9").unwrap_or(&Value::Null);
        assert_eq!(d9["level"], json!("Level 1"), "{t:#}");
        assert!((d9["conduit_ft"].as_f64().unwrap_or(0.0) - 1400.0 / 12.0).abs() < 0.1, "{d9:#}");
        assert!(d9["wire_ft"]["#12"].as_f64().unwrap_or(0.0) > 0.0 && d9["boxes"] == json!(2.0), "{d9:#}");
        assert!((t["summary"]["conduit_ft"].as_f64().unwrap_or(0.0) - 2500.0 / 12.0).abs() < 0.1, "{:#}", t["summary"]);
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("cadaddy-{}", job.id)));
    }

    /// A made-up job of two plan sheets side by side, levels 1 and 2, each with a fixture
    /// schedule (types A, B) and junk tags. Every device the devices stage counts becomes one
    /// model element on its sheet's level and one BOM unit (Q5271 MUSTS 1, 2, 4, 5).
    #[test]
    fn a_typical_room_plans_title_names_its_room_type() {
        let words = |t: &str| type_words(t, None);
        let vocabulary: BTreeSet<String> = ["DOUBLE Q", "SINGLE Q", "SINGLE Q ADA"].iter().flat_map(|n| words(n)).collect();
        let title = |t: &str| type_words(t, Some(&vocabulary));
        assert_eq!(title("ENLARGED DOUBLE QUEEN"), words("DOUBLE Q"));
        assert_eq!(title("UNIT ELECTRICAL PLAN - SINGLE QUEEN"), words("SINGLE Q"));
        assert_eq!(title("ENLARGED QUEEN"), words("SINGLE Q"), "a QUEEN room is the single one");
        assert_eq!(title("ENLARGED ACC. QUEEN"), words("SINGLE Q ADA"));
        assert_ne!(title("ENLARGED ACC. DOUBLE QUEEN"), words("DOUBLE Q"));
        assert!(title("ELECTRICAL - 1ST FLOOR - MEP ROOM").is_empty());
    }

    #[test]
    fn a_typical_rooms_device_lands_at_the_same_place_in_each_room() {
        // A 12' x 24' view (inches) and a room the same way round, then one turned a quarter.
        let view = [0.0, 0.0, 144.0, 288.0];
        assert_eq!(into_room([36.0, 72.0], view, [1000.0, 0.0, 1144.0, 288.0]), Some([1036.0, 72.0]));
        assert_eq!(into_room([36.0, 72.0], view, [0.0, 0.0, 288.0, 144.0]), Some([72.0, 108.0]));
        assert_eq!(into_room([36.0, 72.0], [0.0, 0.0, 0.0, 288.0], view), None);
    }

    #[test]
    fn a_typical_view_goes_into_each_room_the_way_its_doors_face() {
        use cadaddy_geom::Vec2;
        // A 12' x 24' unit drawn entry-down: its entry door's hinge at (24, 6), the bath door's at
        // (110, 60), a receptacle at (6, 200) on the west wall by the window end.
        let view = [0.0, 0.0, 144.0, 288.0];
        let swings = [Vec2::new(24.0, 6.0), Vec2::new(110.0, 60.0)];
        // Across the corridor the same unit faces the other way: turned half round.
        let room = [1000.0, 0.0, 1144.0, 288.0];
        let turned: Vec<Vec2> = swings.iter().map(|h| Vec2::new(room[0] + room[2] - room[0] - h.x, room[1] + 288.0 - h.y)).collect();
        let k = room_turn(view, room, &swings, &turned);
        assert_eq!(k, 2);
        assert_eq!(into_room_turned([6.0, 200.0], view, room, k), Some([1138.0, 88.0]), "on the east wall, by the window end");
        // Its mirror twin beside it (the entry hinge on the other side): mirrored.
        let mirrored: Vec<Vec2> = swings.iter().map(|h| Vec2::new(room[0] + 144.0 - h.x, h.y)).collect();
        let k = room_turn(view, room, &swings, &mirrored);
        assert_eq!(into_room_turned([24.0, 6.0], view, room, k), Some([1120.0, 6.0]), "turn {k}");
        // No swings to go by: the plain way.
        assert_eq!(room_turn(view, room, &[], &turned), 0);
    }

    #[test]
    fn a_typical_floors_room_takes_the_number_of_the_same_room_above_it() {
        let v = cadaddy_geom::Vec2::new;
        let room = |h: u64, level: &str, name: &str, number: Option<&str>, x: f64| super::super::bspec::RoomElement {
            handle: cadaddy_doc::Handle(h),
            level: level.into(),
            name: name.into(),
            number: number.map(String::from),
            area_sf: 200.0,
            ceiling_ft: 9.0,
            finish: None,
            source: cadaddy_doc::Handle(0),
            outline: vec![v(x, 0.0), v(x + 120.0, 0.0), v(x + 120.0, 240.0), v(x, 240.0)],
            z: 0.0,
        };
        let rooms = [
            room(1, "Level 3", "SINGLE Q", Some("305"), 0.0),
            room(2, "Level 2", "SINGLE Q", None, 6.0),
            // Another kind of room over it, and one a bay over: no number from them.
            room(3, "Level 4", "DOUBLE Q", None, 0.0),
            room(4, "Level 4", "SINGLE Q", None, 400.0),
            // 205 is already a room of level 2's: never twice.
            room(5, "Level 3", "SINGLE Q", Some("306"), 1000.0),
            room(6, "Level 2", "SINGLE Q", None, 1000.0),
            room(7, "Level 2", "STORAGE", Some("206"), 3000.0),
            // Known by its number alone above; the same room traced a little bigger below.
            room(8, "Level 3", "Room 309", Some("309"), 2000.0),
            room(9, "Level 2", "SINGLE Q", None, 2004.0),
            // Traced without its bathroom (60 shorter) below: the middles 2.5' apart, each in the other.
            room(10, "Level 3", "SINGLE Q", Some("331"), 4000.0),
            room(11, "Level 2", "SINGLE Q", None, 4000.0),
        ];
        let mut rooms = rooms.to_vec();
        rooms[8].area_sf = 250.0;
        rooms[10].outline = vec![v(4000.0, 0.0), v(4120.0, 0.0), v(4120.0, 180.0), v(4000.0, 180.0)];
        rooms[10].area_sf = 150.0;
        assert_eq!(
            stacked_numbers(&rooms, 12.0),
            vec![
                (cadaddy_doc::Handle(2), "205".to_string()),
                (cadaddy_doc::Handle(9), "209".to_string()),
                (cadaddy_doc::Handle(11), "231".to_string())
            ]
        );
    }

    #[test]
    fn a_number_a_room_of_the_level_has_is_not_given_again() {
        let room = |level: &str, name: &str, number: Option<&str>| super::super::bspec::RoomElement {
            handle: cadaddy_doc::Handle(1),
            level: level.into(),
            name: name.into(),
            number: number.map(String::from),
            area_sf: 100.0,
            ceiling_ft: 9.0,
            finish: None,
            source: cadaddy_doc::Handle(0),
            outline: Vec::new(),
            z: 0.0,
        };
        let rooms = [room("Level 1", "CORRIDOR", Some("100")), room("Level 1", "Room 102", Some("102")), room("Level 2", "CORRIDOR", None)];
        // The corridor's other piece doesn't take 100 again; level 2's corridor may.
        assert!(number_taken(&rooms, "1", "100") && number_taken(&rooms, "Level 1", " 100 "));
        assert!(!number_taken(&rooms, "Level 2", "100") && !number_taken(&rooms, "Level 1", "101"));
        // A "Room 102" traced from the label is the room 102 goes to.
        assert!(!number_taken(&rooms, "Level 1", "102"));
    }

    #[test]
    fn a_run_through_the_elevator_shaft_is_a_clash_and_one_beside_it_is_not() {
        let shaft = cadaddy_walk3d::Circulation {
            kind: cadaddy_walk3d::CirculationKind::Hoistway,
            level: "Level 2".into(),
            name: "ELEV.".into(),
            at: [100.0, 100.0],
            rects: vec![[[60.0, 60.0], [156.0, 156.0]]],
        };
        let corridor = cadaddy_walk3d::Circulation {
            kind: cadaddy_walk3d::CirculationKind::Corridor,
            rects: vec![[[0.0, 0.0], [600.0, 60.0]]],
            ..shaft.clone()
        };
        let run = |level: &str, points: Vec<[f64; 2]>| -> cadaddy_walk3d::MepRun {
            serde_json::from_value(json!({"system": "SA", "level": level, "points": points, "size_in": [12, 8], "bottoms_ft": [9]})).unwrap()
        };
        let runs = [
            // Straight through the shaft.
            run("2", vec![[0.0, 100.0], [300.0, 100.0]]),
            // Along the face of its wall (inside the clearance), and past it.
            run("Level 2", vec![[0.0, 59.0], [300.0, 59.0], [300.0, 300.0]]),
            run("Level 2", vec![[0.0, 160.0], [300.0, 160.0]]),
            // Through it, but on another floor.
            run("Level 3", vec![[100.0, 0.0], [100.0, 300.0]]),
            // A corner of it cut by the last leg.
            run("Level 2", vec![[0.0, 0.0], [40.0, 40.0], [140.0, 200.0]]),
            // Down the corridor: allowed.
            run("Level 2", vec![[0.0, 30.0], [600.0, 30.0]]),
        ];
        assert_eq!(runs_through_hoistways(&runs, &[shaft, corridor], 12.0), vec![0, 4]);
    }

    #[test]
    fn a_shell_holding_half_its_floors_rooms_is_not_the_building() {
        let v = cadaddy_geom::Vec2::new;
        // The west half of a 100' x 50' floor enclosed; rooms along its whole length.
        let half = [[v(0.0, 0.0), v(600.0, 600.0)]];
        let whole = [[v(0.0, 0.0), v(1200.0, 600.0)]];
        let rooms: Vec<[f64; 2]> = (0..10).map(|i| [60.0 + f64::from(i) * 120.0, 300.0]).collect();
        assert_eq!(shell_holds_rooms(&half, &rooms), (false, 5));
        assert_eq!(shell_holds_rooms(&whole, &rooms), (true, 10));
        assert_eq!(shell_holds_rooms(&half, &[]), (true, 0));
    }

    #[test]
    fn a_tiny_room_with_a_bigger_rooms_number_is_a_scrap() {
        let room = |h: u64, level: &str, number: Option<&str>, area_sf: f64| super::super::bspec::RoomElement {
            handle: cadaddy_doc::Handle(h),
            level: level.into(),
            name: "Room".into(),
            number: number.map(String::from),
            area_sf,
            ceiling_ft: 9.0,
            finish: None,
            source: cadaddy_doc::Handle(0),
            outline: Vec::new(),
            z: 0.0,
        };
        let rooms = [
            room(1, "Level 1", Some("119"), 206.0),
            room(2, "Level 1", Some("119"), 9.0),
            // A closet of its own number, and another floor's 119-sized sliver: rooms.
            room(3, "Level 1", Some("120A"), 6.0),
            room(4, "Level 2", Some("119"), 4.0),
            room(5, "Level 1", None, 3.0),
            // A 13 sf sliver of 119; an unnumbered sliver named as a bigger room of its level.
            room(6, "Level 1", Some("119"), 13.0),
        ];
        assert_eq!(numbered_scraps(&rooms), vec![cadaddy_doc::Handle(2), cadaddy_doc::Handle(6)]);
        let mut named = rooms.to_vec();
        named[4].name = "SINGLE Q ADA".into();
        named[0].name = "SINGLE Q ADA".into();
        assert!(numbered_scraps(&named).contains(&cadaddy_doc::Handle(5)), "an unnumbered sliver named as room 119");
    }

    #[test]
    fn a_tag_by_a_rooms_door_is_near_that_room() {
        let v = cadaddy_geom::Vec2::new;
        let room = [v(0.0, 0.0), v(120.0, 0.0), v(120.0, 120.0), v(0.0, 120.0)];
        // Two feet outside its door wall, and inside (the nearest wall a foot away).
        assert!((edge_distance(&room, v(60.0, -24.0)) - 24.0).abs() < 1e-9);
        assert!((edge_distance(&room, v(60.0, 12.0)) - 12.0).abs() < 1e-9);
        assert!(edge_distance(&[], v(0.0, 0.0)).is_infinite());
    }

    #[test]
    fn unread_duct_sizes_and_retraced_runs_are_tidied() {
        let run = |sys: &str, pts: [[f64; 2]; 2], size: [f64; 2], source: &str| cadaddy_walk3d::MepRun {
            system: sys.into(),
            level: "Level 1".into(),
            points: pts.to_vec(),
            size_in: size,
            bottoms_ft: vec![10.0],
            objects: Vec::new(),
            id: String::new(),
            encased: false,
            size_source: source.into(),
            height_source: String::new(),
            reviewed: false,
            edited_by: String::new(),
            top_ft: None,
            source_sheet: "M-1".into(),
        };
        let mut runs = vec![
            run("SA", [[0.0, 0.0], [100.0, 0.0]], [12.0, 10.0], "tag"),
            run("SA", [[0.0, 50.0], [100.0, 50.0]], [14.0, 10.0], "tag"),
            run("SA", [[0.0, 90.0], [100.0, 90.0]], [10.0, 8.0], "tag"),
            // Two unrelated lines measured as one duct, then the same segment traced back.
            run("SA", [[200.0, 0.0], [300.0, 0.0]], [60.0, 12.0], "default"),
            run("SA", [[301.0, 2.0], [199.0, -1.0]], [60.0, 12.0], "default"),
            // A sensible unread size stays.
            run("SA", [[400.0, 0.0], [500.0, 0.0]], [10.0, 8.0], "default"),
        ];
        let (sized, dropped) = tidy_runs(&mut runs, 12.0, &BTreeSet::from(["SA".to_string()]));
        assert_eq!((sized, dropped), (2, 1));
        assert_eq!(runs.len(), 5);
        assert_eq!(runs[3].size_in, [12.0, 10.0], "the median tagged size");
        assert_eq!(runs[4].size_in, [10.0, 8.0]);
    }

    #[test]
    fn a_registered_views_devices_move_by_where_they_are() {
        use cadaddy_mepfix::printlinks::{Enlarged, SheetLinks};
        let view = |title: &str, reach: [f64; 4], handles: &[&str]| Enlarged {
            sheet: "E-6".into(),
            title: title.into(),
            level: "1".into(),
            registered: true,
            devices: handles.len(),
            device_handles: handles.iter().map(|h| h.to_string()).collect(),
            fit: Some(cadaddy_mepfix::tagid::Sim::scale_offset(1.0, [1000.0, 0.0])),
            reach: Some(reach),
            ..Enlarged::default()
        };
        let links = SheetLinks {
            enlarged: vec![
                view("1ST FLOOR - CORE AREA", [0.0, 0.0, 100.0, 100.0], &["10"]),
                view("ENLARGED QUEEN", [200.0, 0.0, 300.0, 100.0], &["20"]),
            ],
            ..SheetLinks::default()
        };
        let dv = |tag: &str, handle: &str, at: [f64; 2]| super::super::readers::ReadDevice {
            tag: tag.into(),
            at,
            sheet: "E-6".into(),
            level: "1".into(),
            handle: handle.into(),
            ..Default::default()
        };
        let mut census = DeviceCensus {
            placed: vec![
                dv("A", "10", [10.0, 10.0]),
                // A switch read at its designation (no symbol), in the core area: it moves too.
                dv("SD", "", [20.0, 20.0]),
                // Outside every view: stays.
                dv("J", "", [150.0, 50.0]),
                // In a typical room's view: left for the stamping.
                dv("SD", "", [250.0, 50.0]),
            ],
            ..DeviceCensus::default()
        };
        let job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        let (moved, _) = enlarged_authority(&job, &links, &mut census);
        assert_eq!(moved, 2);
        let at: Vec<[f64; 2]> = census.placed.iter().map(|d| d.at).collect();
        assert_eq!(at, [[1010.0, 10.0], [1020.0, 20.0], [150.0, 50.0], [250.0, 50.0]]);
    }

    #[test]
    fn a_views_title_names_its_one_room() {
        use cadaddy_geom::Vec2;
        let sq = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)];
        let rooms = vec![
            ("MECH".to_string(), "1".to_string(), sq.clone(), "130".to_string()),
            ("BOH".to_string(), "2".to_string(), sq.clone(), "237".to_string()),
            ("STORAGE".to_string(), "1".to_string(), sq.clone(), "131".to_string()),
            ("STORAGE".to_string(), "2".to_string(), sq.clone(), "231".to_string()),
        ];
        assert_eq!(room_of_view("ENLARGED BOH ROOM #237", &rooms), Some(1), "by its number");
        assert_eq!(room_of_view("ELECTRICAL - 1ST FLOOR - MEP ROOM", &rooms), Some(0), "by its name");
        assert_eq!(room_of_view("ELECTRICAL - 2ND FLOOR - STORAGE ROOM", &rooms), Some(3), "two of the name: the floor says which");
        assert_eq!(room_of_view("ELECTRICAL - 1ST FLOOR - CORE AREA", &rooms), None, "no room named");
        assert_eq!(room_of_view("ENLARGED QUEEN", &rooms), None);
    }

    #[test]
    fn a_typical_queen_or_suite_is_a_room_to_sleep_in() {
        for (title, sleeping) in
            [("ENLARGED QUEEN", true), ("ENLARGED ACC. DOUBLE QUEEN", true), ("TYPICAL SUITE", true), ("TYPICAL RESTROOM", false)]
        {
            assert_eq!(sleeping_room(&type_words(title, None)), sleeping, "{title}");
        }
    }

    #[test]
    fn a_receptacles_designations_are_the_words_printed_by_it() {
        assert_eq!(receptacle_designations(&["G-3", "GFCI"]), ["GFCI"]);
        assert_eq!(receptacle_designations(&["WP/GFI"]), ["GFCI", "WP"], "in a fixed order");
        assert_eq!(receptacle_designations(&["USB", "TR", "A-1"]), ["TR", "USB"]);
        assert!(receptacle_designations(&["WPT", "TRUCK", "A-1"]).is_empty(), "whole words only");
    }

    /// Q6099: an enlarged RCP sheet imported before an enlarged plan (its view titled with the
    /// same room type, every later sheet a sheet's width further right) takes none of the plan's
    /// devices and moves none: each room of the type gets them, the floor plan's own of a tag in a
    /// room standing for that many.
    #[test]
    fn a_typical_rooms_ceiling_plan_sets_each_rooms_ceiling_with_its_source() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        let line = |s: &mut crate::Session, a: [f64; 2], b: [f64; 2]| {
            s.execute("line", &json!({"points": [a, b]})).unwrap();
        };
        let unit = |s: &mut crate::Session, o: [f64; 2], turned: bool| {
            let p = |x: f64, y: f64| if turned { [o[0] + 144.0 - x, o[1] + 288.0 - y] } else { [o[0] + x, o[1] + y] };
            let mut segs =
                vec![((0.0, 0.0), (144.0, 0.0)), ((144.0, 0.0), (144.0, 288.0)), ((144.0, 288.0), (0.0, 288.0)), ((0.0, 288.0), (0.0, 0.0))];
            for k in 1..6 {
                segs.push(((0.0, 48.0 * f64::from(k)), (144.0, 48.0 * f64::from(k))));
            }
            segs.push(((0.0, 100.0), (60.0, 100.0)));
            segs.push(((100.0, 210.0), (144.0, 210.0)));
            for ((ax, ay), (bx, by)) in segs {
                line(s, p(ax, ay), p(bx, by));
            }
        };
        // The A-6 sheet: the unit's ceiling plan, 9'-0" in the room, 8'-0" in the bath (y < 100).
        let x0 = 4234.0;
        let lo = s.doc().unwrap().handseed;
        for (a, b) in [([0.0, 0.0], [2000.0, 0.0]), ([2000.0, 0.0], [2000.0, 1400.0]), ([2000.0, 1400.0], [0.0, 1400.0]), ([0.0, 1400.0], [0.0, 0.0])]
        {
            line(&mut s, [x0 + a[0], a[1]], [x0 + b[0], b[1]]);
        }
        let v = [x0 + 300.0, 200.0];
        unit(&mut s, v, false);
        // Each in its ceiling tag (the type letter over it); a dimension string is no height.
        s.execute("text", &json!({"at": [v[0] + 70.0, v[1] + 230.0], "text": "9'-0\"", "height": 4.0})).unwrap();
        s.execute("text", &json!({"at": [v[0] + 72.0, v[1] + 236.0], "text": "D", "height": 4.0})).unwrap();
        s.execute("text", &json!({"at": [v[0] + 20.0, v[1] + 30.0], "text": "8'-0\" AFF", "height": 4.0})).unwrap();
        s.execute("text", &json!({"at": [v[0] + 22.0, v[1] + 36.0], "text": "A", "height": 4.0})).unwrap();
        s.execute("text", &json!({"at": [v[0] + 30.0, v[1] + 160.0], "text": "10' - 1\"", "height": 4.0})).unwrap();
        s.execute("text", &json!({"at": [v[0], v[1] - 19.0], "text": "UNIT RCP - DOUBLE QUEEN", "height": 7.4})).unwrap();
        s.execute("text", &json!({"at": [v[0], v[1] - 28.0], "text": "SCALE: 1/4\" = 1'-0\"", "height": 4.6})).unwrap();
        let hi = s.doc().unwrap().handseed;
        unit(&mut s, [10_000.0, 0.0], false);
        unit(&mut s, [10_400.0, 0.0], true);
        job.sheet_regions = vec![SheetRegion {
            number: "A-6".into(),
            level: "T".into(),
            discipline: "A".into(),
            kind: "enlarged_plan".into(),
            rect: [x0, 0.0, x0 + 2000.0, 1400.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        }];
        job.sheet_spans = vec![SheetSpan { sheet: "A-6".into(), lo, hi }];
        let room = |n: &str, x: f64| json!({"name": "DOUBLE QUEEN", "number": n, "level": "Level 2", "outline": [[x, 0], [x + 144.0, 0], [x + 144.0, 288], [x, 288]]});
        if let Some(st) = job.stage_mut(StageName::Rooms) {
            st.result = Some(json!({"rooms": [room("201", 10_000.0), room("202", 10_400.0)]}));
        }
        let sp: cadaddy_walk3d::BuildingSpec = serde_json::from_value(json!({
            "units_per_foot": 12.0,
            "levels": [{"name": "Level 2", "elevation_ft": 12.0, "height_ft": 10.0, "ceiling_ft": 8.5}]
        }))
        .unwrap();
        // The floor's spaces: each room's bath (its end with the stub at 100") and the rest.
        let mut area_at = |_: &str, q: cadaddy_geom::Vec2| -> Option<Vec<[[f64; 2]; 2]>> {
            let r = |x0: f64, y0: f64, x1: f64, y1: f64| Some(vec![[[x0, y0], [x1, y1]]]);
            match (q.x, q.y) {
                (x, y) if (10_000.0..=10_144.0).contains(&x) && y < 100.0 => r(10_000.0, 0.0, 10_144.0, 100.0),
                (x, _) if (10_000.0..=10_144.0).contains(&x) => r(10_000.0, 100.0, 10_144.0, 288.0),
                (x, y) if (10_400.0..=10_544.0).contains(&x) && y > 188.0 => r(10_400.0, 188.0, 10_544.0, 288.0),
                (x, _) if (10_400.0..=10_544.0).contains(&x) => r(10_400.0, 0.0, 10_544.0, 188.0),
                _ => None,
            }
        };
        let (areas, read) = typical_rcp_areas_on(s.doc().unwrap(), &sp, &job, &mut area_at);
        assert_eq!((read, areas.len()), (2, 4), "{areas:?}");
        for n in ["201", "202"] {
            let mine: Vec<_> = areas.iter().filter(|a| a.room.as_deref() == Some(n)).collect();
            assert_eq!(mine.len(), 1, "room {n}: its one main area {areas:?}");
            assert!((mine[0].height_ft - 9.0).abs() < 1e-9, "room {n}: {:?}", mine[0]);
            assert_eq!(mine[0].source.as_deref(), Some("9'-0\" ceiling tag on A-6 UNIT RCP - DOUBLE QUEEN"));
        }
        // The baths: 8', in no room's name (the room keeps its own height).
        let baths: Vec<_> = areas.iter().filter(|a| a.room.is_none()).collect();
        assert!(baths.len() == 2 && baths.iter().all(|a| (a.height_ft - 8.0).abs() < 1e-9), "{areas:?}");
        let turned_bath = baths
            .iter()
            .any(|a| a.outline.iter().any(|p| p[0] >= 10_400.0 && p[1] >= 188.0 - 1e-6) && a.outline.iter().all(|p| p[1] >= 188.0 - 1e-6));
        assert!(turned_bath, "the turned room's bath is at its other end: {baths:?}");
    }

    #[test]
    fn a_typical_rooms_elevations_measure_its_ceiling() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        let line = |s: &mut crate::Session, a: [f64; 2], b: [f64; 2]| {
            s.execute("line", &json!({"points": [a, b]})).unwrap();
        };
        // A-8 (its plans at 3/8"): "DOUBLE QUEEN - BED WALL" at 1/2", so a third bigger: the wall
        // 12' wide, its floor line and its ceiling line 7'-10" over it run out to their markers.
        let z = 4.0 / 3.0;
        let x0 = 4234.0;
        let lo = s.doc().unwrap().handseed;
        for (a, b) in [([0.0, 0.0], [2000.0, 0.0]), ([2000.0, 0.0], [2000.0, 1400.0]), ([2000.0, 1400.0], [0.0, 1400.0]), ([0.0, 1400.0], [0.0, 0.0])]
        {
            line(&mut s, [x0 + a[0], a[1]], [x0 + b[0], b[1]]);
        }
        let v = [x0 + 300.0, 200.0];
        for (a, b) in [
            ((0.0, 0.0), (144.0, 0.0)),
            ((144.0, 0.0), (144.0, 94.0)),
            ((144.0, 94.0), (0.0, 94.0)),
            ((0.0, 94.0), (0.0, 0.0)),
            ((144.0, 0.0), (180.0, 0.0)),
            ((144.0, 94.0), (180.0, 94.0)),
            ((20.0, 30.0), (60.0, 30.0)),
        ] {
            line(&mut s, [v[0] + z * a.0, v[1] + z * a.1], [v[0] + z * b.0, v[1] + z * b.1]);
        }
        let label = |s: &mut crate::Session, x: f64, y: f64, t: &str| {
            s.execute("text", &json!({"at": [v[0] + z * x, v[1] + z * y], "text": t, "height": 3.0})).unwrap();
        };
        label(&mut s, 150.0, 96.0, "B.O. CEILING");
        label(&mut s, 150.0, 91.0, "RE: PLANS");
        label(&mut s, 150.0, 2.0, "F.F.");
        label(&mut s, 150.0, -3.0, "VARIES");
        let p = [x0 + 1000.0, 300.0];
        for (a, b) in [
            ((0.0, 0.0), (144.0, 0.0)),
            ((144.0, 0.0), (144.0, 288.0)),
            ((144.0, 288.0), (0.0, 288.0)),
            ((0.0, 288.0), (0.0, 0.0)),
            ((0.0, 100.0), (60.0, 100.0)),
        ] {
            line(&mut s, [p[0] + a.0, p[1] + a.1], [p[0] + b.0, p[1] + b.1]);
        }
        // Its title wrapped: only its second line reads as the title.
        s.execute("text", &json!({"at": [p[0], p[1] - 19.0], "text": "DOUBLE QUEEN", "height": 7.4})).unwrap();
        s.execute("text", &json!({"at": [p[0], p[1] - 28.0], "text": "SCALE: 3/8\" = 1'-0\"", "height": 4.6})).unwrap();
        s.execute("text", &json!({"at": [v[0], v[1] - 19.0], "text": "DOUBLE QUEEN - BED WALL", "height": 7.4})).unwrap();
        s.execute("text", &json!({"at": [v[0], v[1] - 28.0], "text": "SCALE: 1/2\" = 1'-0\"", "height": 4.6})).unwrap();
        let hi = s.doc().unwrap().handseed;
        job.sheet_regions = vec![SheetRegion {
            number: "A-8".into(),
            level: "T".into(),
            discipline: "A".into(),
            kind: "enlarged_plan".into(),
            rect: [x0, 0.0, x0 + 2000.0, 1400.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        }];
        job.sheet_spans = vec![SheetSpan { sheet: "A-8".into(), lo, hi }];
        // The set's classification read one of the elevations' scales: the sheet's plan says 3/8".
        job.sheets = Some(json!({"sheets": [{"number": "A-8", "discipline": "A", "type": "enlarged_plan", "scale": "1/2\" = 1'-0\""}]}));
        let room = |n: &str, x: f64| json!({"name": "DOUBLE QUEEN", "number": n, "level": "Level 2", "outline": [[x, 0], [x + 144.0, 0], [x + 144.0, 288], [x, 288]]});
        if let Some(st) = job.stage_mut(StageName::Rooms) {
            st.result = Some(json!({"rooms": [room("201", 10_000.0), room("202", 10_400.0)]}));
        }
        let m = typical_elevation_heights(s.doc().unwrap(), &job);
        assert_eq!(m.len(), 1, "{m:?}");
        let (words, ft, source) = &m[0];
        assert_eq!(*words, type_words("DOUBLE QUEEN", None));
        assert!((ft - 94.0 / 12.0).abs() < 1e-9, "{m:?}");
        assert_eq!(source, "7'-10\" measured on A-8 DOUBLE QUEEN - BED WALL (F.F. to B.O. CEILING)");
    }

    #[test]
    fn a_typical_rooms_mechanical_view_gives_its_ptac_and_exhaust_fan() {
        // As M-5's MECH ENLARGED DOUBLE QUEEN draws them (inches): the PTAC tag and its type, the
        // PTAC (42" x 12") at the window, a nightstand-sized box, the exhaust fan with no tag (a
        // box in a box) and its wall cap's four louvre circles; a toilet's two circles.
        let labels = vec![
            ("PTAC".to_string(), [52316.0, 1323.0]),
            ("1".to_string(), [52326.0, 1318.0]),
            ("T".to_string(), [52427.0, 1111.0]),
            ("10".to_string(), [52333.0, 1013.0]),
        ];
        let shape = |c: [f64; 2], w: f64, h: f64, closed: bool| ([c[0] - w / 2.0, c[1] - h / 2.0], [c[0] + w / 2.0, c[1] + h / 2.0], closed);
        let shapes = vec![
            shape([52362.0, 1333.0], 41.9, 12.3, false),
            shape([52302.0, 1273.0], 16.0, 26.0, true),
            shape([52308.0, 1067.0], 13.8, 11.5, true),
            shape([52308.0, 1067.0], 13.8, 11.5, true),
            shape([52308.0, 1067.0], 14.9, 13.3, true),
            shape([52308.0, 1112.0], 23.7, 26.8, true),
        ];
        let circles = vec![
            ([52288.0, 1023.0], 2.0),
            ([52288.0, 1028.0], 2.0),
            ([52287.0, 1034.0], 2.0),
            ([52287.0, 1040.0], 2.0),
            ([52329.0, 1111.0], 3.5),
            ([52329.0, 1103.0], 3.5),
        ];
        let items = mech_items(&labels, &shapes, &circles, 1.0);
        assert_eq!(items.len(), 2, "{items:?}");
        let ptac = items.iter().find(|i| i.kind == "ptac").unwrap();
        assert_eq!(ptac.tag, "PTAC-1");
        assert!((ptac.hi[0] - ptac.lo[0] - 41.9).abs() < 1e-6, "{ptac:?}");
        let ef = items.iter().find(|i| i.kind == "exhaust_fan").unwrap();
        assert!(((ef.lo[0] + ef.hi[0]) / 2.0 - 52308.0).abs() < 1e-6 && ((ef.lo[1] + ef.hi[1]) / 2.0 - 1067.0).abs() < 1e-6, "{ef:?}");
        let cap = ef.cap.unwrap();
        assert!((cap[0] - 52287.5).abs() < 0.6 && (cap[1] - 1031.25).abs() < 0.6, "{cap:?}");
        // With an EF tag the fan is the square by it.
        let mut tagged = labels.clone();
        tagged.push(("EF-3".to_string(), [52320.0, 1080.0]));
        tagged.push(("EF".to_string(), [52296.0, 1050.0]));
        let items = mech_items(&tagged, &shapes, &circles, 1.0);
        assert!(items.iter().any(|i| i.kind == "exhaust_fan" && i.tag == "EF-3"), "{items:?}");
        assert_eq!(items.iter().filter(|i| i.kind == "exhaust_fan").count(), 1, "one fan tagged twice: {items:?}");
        // A typical room's connection tags go to its equipment.
        let eq = cadaddy_walk3d::MepEquipment {
            tag: "EF".into(),
            kind: "exhaust_fan".into(),
            level: "Level 3".into(),
            min: [10.0, 10.0],
            max: [24.0, 22.0],
            bottom_ft: 7.0 + 10.0 / 12.0,
            properties: [("room".to_string(), "315".to_string())].into_iter().collect(),
            ..Default::default()
        };
        let mut devs = vec![super::super::readers::ReadDevice {
            tag: "EF".into(),
            at: [100.0, 100.0],
            level: "Level 3".into(),
            room: "315".into(),
            ..Default::default()
        }];
        assert_eq!(connect_to_equipment(std::slice::from_ref(&eq), &mut devs, 180.0), 1);
        assert_eq!(devs[0].at, [17.0, 16.0]);
        assert_eq!(devs[0].aff_in, Some(94.0));
        // A tag on the overall plan (no room yet): the nearest fan within reach, none past it.
        let mut far = vec![super::super::readers::ReadDevice { tag: "EF".into(), at: [17.0, 300.0], level: "Level 3".into(), ..Default::default() }];
        assert_eq!(connect_to_equipment(std::slice::from_ref(&eq), &mut far, 180.0), 0);
        let mut near = vec![super::super::readers::ReadDevice { tag: "EF".into(), at: [17.0, 120.0], level: "Level 3".into(), ..Default::default() }];
        assert_eq!(connect_to_equipment(&[eq], &mut near, 180.0), 1);
        assert_eq!(near[0].at, [17.0, 16.0]);
    }

    #[test]
    fn a_rooms_point_goes_back_to_the_view_for_every_turn() {
        // Room to view to another room: each of the eight turns undone exactly.
        let view = [0.0, 0.0, 144.0, 288.0];
        for k in 0..8u8 {
            let fit = RoomFit { k, from: cadaddy_geom::Vec2::new(72.0, 144.0), to: cadaddy_geom::Vec2::new(10_072.0, 144.0), rigid: true };
            for p in [[10.0, 20.0], [130.0, 270.0], [72.0, 0.0]] {
                let q = place_in_room(p, view, [10_000.0, 0.0, 10_144.0, 288.0], fit).unwrap();
                let back = view_point(q, fit).unwrap();
                assert!((back[0] - p[0]).abs() < 1e-9 && (back[1] - p[1]).abs() < 1e-9, "k {k}: {p:?} -> {q:?} -> {back:?}");
            }
        }
        // A fit by shares of the box can't be undone.
        let loose = RoomFit { k: 0, from: cadaddy_geom::Vec2::new(0.0, 0.0), to: cadaddy_geom::Vec2::new(0.0, 0.0), rigid: false };
        assert!(view_point([1.0, 1.0], loose).is_none());
    }

    #[test]
    fn the_plumbing_plans_stacks_are_risers_where_their_leaders_end() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        let lo = s.doc().unwrap().handseed;
        // P-7: a two-line label with its leader to the stack, a one-line one, and a label with no
        // leader (no place to put it).
        let t = |s: &mut crate::Session, at: [f64; 2], v: &str| {
            s.execute("text", &json!({"at": at, "text": v, "height": 4.6})).unwrap();
        };
        t(&mut s, [100.0, 200.0], "3\" WASTE");
        t(&mut s, [100.0, 193.0], "STACK VENT");
        s.execute("line", &json!({"points": [[98.0, 196.0], [70.0, 160.0]]})).unwrap();
        t(&mut s, [300.0, 200.0], "2\" VENT STACK");
        s.execute("line", &json!({"points": [[298.0, 202.0], [280.0, 240.0]]})).unwrap();
        t(&mut s, [500.0, 200.0], "4\" WASTE STACK");
        let hi = s.doc().unwrap().handseed;
        job.sheet_regions = vec![SheetRegion {
            number: "P-7".into(),
            level: "3".into(),
            discipline: "P".into(),
            kind: "plan".into(),
            rect: [0.0, 0.0, 2000.0, 1400.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        }];
        job.sheet_spans = vec![SheetSpan { sheet: "P-7".into(), lo, hi }];
        let stacks = plumbing_stacks(s.doc().unwrap(), &job);
        assert_eq!(stacks.len(), 2, "{stacks:?}");
        let san = stacks.iter().find(|st| st.leg == "SAN").unwrap();
        assert_eq!(san.at, [70.0, 160.0]);
        assert_eq!(san.tag, "3\" WASTE STACK VENT (3\")");
        let vent = stacks.iter().find(|st| st.leg == "V").unwrap();
        assert_eq!(vent.at, [280.0, 240.0]);
        // As risers through the storey: a sanitary system made, each its size, floor to floor.
        let mut sp: cadaddy_walk3d::BuildingSpec = serde_json::from_value(json!({
            "units_per_foot": 12.0,
            "levels": [{"name": "Level 3", "elevation_ft": 24.0, "height_ft": 10.0, "ceiling_ft": 8.0}],
            "mep": {"systems": [{"name": "V", "kind": "pipe", "layer": "P-V", "size_in": [2.0, 0.0], "bottom_ft": 10.0}]}
        }))
        .unwrap();
        let placed: Vec<super::super::readers::ReadDevice> = stacks
            .into_iter()
            .map(|mut st| {
                st.level = "Level 3".into();
                st
            })
            .collect();
        assert_eq!(stacks_as_risers(&mut sp, &placed), 2);
        assert!(sp.mep.systems.iter().any(|y| y.name == "SAN"));
        let r = sp.mep.runs.iter().find(|r| r.system == "SAN").unwrap();
        assert_eq!((r.points.len(), r.size_in[0], r.top_ft), (1, 3.0, Some(10.0)));
        assert!(sp.check().iter().all(|i| !i.path.starts_with("mep.runs")), "{:?}", sp.check());
    }

    #[test]
    fn the_typical_rooms_become_unit_types_with_their_views_and_each_rooms_hand() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        let line = |s: &mut crate::Session, a: [f64; 2], b: [f64; 2]| {
            s.execute("line", &json!({"points": [a, b]})).unwrap();
        };
        // The unit's linework at `o`: as drawn, turned half round, or mirrored (east to west).
        let unit = |s: &mut crate::Session, o: [f64; 2], how: u8| {
            let p = |x: f64, y: f64| match how {
                1 => [o[0] + 144.0 - x, o[1] + 288.0 - y],
                2 => [o[0] + 144.0 - x, o[1] + y],
                3 => [o[0] + x, o[1] + 288.0 - y],
                _ => [o[0] + x, o[1] + y],
            };
            let mut segs =
                vec![((0.0, 0.0), (144.0, 0.0)), ((144.0, 0.0), (144.0, 288.0)), ((144.0, 288.0), (0.0, 288.0)), ((0.0, 288.0), (0.0, 0.0))];
            for k in 1..6 {
                segs.push(((0.0, 48.0 * f64::from(k)), (144.0, 48.0 * f64::from(k))));
            }
            segs.push(((0.0, 100.0), (60.0, 100.0)));
            segs.push(((100.0, 210.0), (144.0, 210.0)));
            for ((ax, ay), (bx, by)) in segs {
                line(s, p(ax, ay), p(bx, by));
            }
        };
        let x0 = 4234.0;
        let lo = s.doc().unwrap().handseed;
        for (a, b) in [([0.0, 0.0], [2000.0, 0.0]), ([2000.0, 0.0], [2000.0, 1400.0]), ([2000.0, 1400.0], [0.0, 1400.0]), ([0.0, 1400.0], [0.0, 0.0])]
        {
            line(&mut s, [x0 + a[0], a[1]], [x0 + b[0], b[1]]);
        }
        let v = [x0 + 300.0, 200.0];
        unit(&mut s, v, 0);
        s.execute("text", &json!({"at": [v[0], v[1] - 19.0], "text": "UNIT PLAN - DOUBLE QUEEN", "height": 7.4})).unwrap();
        s.execute("text", &json!({"at": [v[0], v[1] - 28.0], "text": "SCALE: 1/4\" = 1'-0\"", "height": 4.6})).unwrap();
        let e = [x0 + 1000.0, 300.0];
        for (a, b) in [((0.0, 0.0), (144.0, 0.0)), ((144.0, 0.0), (144.0, 94.0)), ((144.0, 94.0), (0.0, 94.0)), ((0.0, 94.0), (0.0, 0.0))] {
            line(&mut s, [e[0] + a.0, e[1] + a.1], [e[0] + b.0, e[1] + b.1]);
        }
        s.execute("text", &json!({"at": [e[0], e[1] - 19.0], "text": "DOUBLE QUEEN - BED WALL", "height": 7.4})).unwrap();
        s.execute("text", &json!({"at": [e[0], e[1] - 28.0], "text": "SCALE: 1/4\" = 1'-0\"", "height": 4.6})).unwrap();
        let hi = s.doc().unwrap().handseed;
        unit(&mut s, [10_000.0, 0.0], 0);
        unit(&mut s, [10_400.0, 0.0], 1);
        unit(&mut s, [10_800.0, 0.0], 2);
        unit(&mut s, [11_600.0, 0.0], 3);
        job.sheet_regions = vec![SheetRegion {
            number: "A-8".into(),
            level: "T".into(),
            discipline: "A".into(),
            kind: "enlarged_plan".into(),
            rect: [x0, 0.0, x0 + 2000.0, 1400.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        }];
        job.sheet_spans = vec![SheetSpan { sheet: "A-8".into(), lo, hi }];
        let room = |n: &str, x: f64, name: &str| json!({"name": name, "number": n, "level": "2", "outline": [[x, 0], [x + 144.0, 0], [x + 144.0, 288], [x, 288]]});
        if let Some(st) = job.stage_mut(StageName::Rooms) {
            st.result = Some(
                json!({"rooms": [room("201", 10_000.0, "DOUBLE QUEEN"), room("202", 10_400.0, "DOUBLE QUEEN"), room("203", 10_800.0, "DOUBLE QUEEN"), room("200", 11_200.0, "CORRIDOR"), room("204", 11_600.0, "DOUBLE QUEEN")]}),
            );
        }
        let (types, instances) = job_unit_types(s.doc().unwrap(), &job);
        assert_eq!(types.len(), 1, "{types:?}");
        let views: Vec<&str> = types[0]["views"].as_array().unwrap().iter().filter_map(|v| v["title"].as_str()).collect();
        assert!(views.contains(&"UNIT PLAN - DOUBLE QUEEN") && views.contains(&"DOUBLE QUEEN - BED WALL"), "{views:?}");
        let of = |n: &str| instances.iter().find(|i| i["number"] == n).unwrap().clone();
        assert_eq!((of("201")["hand"].clone(), of("201")["fit"]["rotation_deg"].clone()), (json!("normal"), json!(0.0)), "{instances:?}");
        assert_eq!((of("202")["hand"].clone(), of("202")["fit"]["rotation_deg"].clone()), (json!("normal"), json!(180.0)), "{instances:?}");
        assert_eq!(of("203")["hand"], json!("mirrored"), "{instances:?}");
        // Flipped top to bottom (the hotel's 101 and 427): mirrored and turned half round, not
        // only turned.
        assert_eq!((of("204")["hand"].clone(), of("204")["fit"]["rotation_deg"].clone()), (json!("mirrored"), json!(180.0)), "{instances:?}");
        assert!(instances.iter().all(|i| i["type"] == types[0]["id"]), "no corridor unit: {instances:?}");
    }

    #[test]
    fn a_view_whose_linework_runs_into_its_neighbours_takes_its_titles_cell() {
        // P-7: four sanitary views in a row along the bottom, two water views over them.
        let sheet = [0.0, 0.0, 2000.0, 1400.0];
        let titles = [[100.0, 50.0], [550.0, 50.0], [1000.0, 50.0], [1450.0, 50.0], [550.0, 600.0], [1000.0, 600.0]];
        let c = title_cell([550.0, 50.0], &titles, sheet).unwrap();
        assert!((c[0] - 510.0).abs() < 1e-9 && (c[2] - 960.0).abs() < 1e-9, "to the next title of its row: {c:?}");
        assert!((c[1] - 22.0).abs() < 1e-9 && (c[3] - 572.0).abs() < 1e-9, "up to the title over it: {c:?}");
        // The last of the row stops at the title block; one with nothing over it runs to the top.
        let last = title_cell([1450.0, 50.0], &titles, sheet).unwrap();
        assert!((last[2] - 1760.0).abs() < 1e-9 && (last[3] - 1400.0).abs() < 1e-9, "{last:?}");
        assert!(title_cell([1.0, 1.0], &titles, [0.0, 0.0, f64::NAN, 1.0]).is_none());
    }

    #[test]
    fn a_symbol_is_known_by_its_shape_not_its_size_or_name() {
        let mut s = crate::Session::new();
        // A duplex receptacle (a circle, two short lines through it) at two sizes, and a keynote
        // box (a square), each made a block.
        let block = |s: &mut crate::Session, name: &str, k: f64, square: bool| {
            let lo = s.doc().unwrap().handseed;
            if square {
                s.execute("pline", &json!({"vertices": [[0, 0], [k * 6.0, 0], [k * 6.0, k * 6.0], [0, k * 6.0]], "closed": true})).unwrap();
            } else {
                s.execute("circle", &json!({"center": [0, 0], "radius": k * 3.0})).unwrap();
                s.execute("line", &json!({"points": [[-k * 4.5, k], [k * 4.5, k]]})).unwrap();
                s.execute("line", &json!({"points": [[-k * 4.5, -k], [k * 4.5, -k]]})).unwrap();
            }
            let handles: Vec<String> = s.doc().unwrap().model.iter().filter(|e| e.handle.0 >= lo).map(|e| e.handle.hex()).collect();
            s.execute("block", &json!({"name": name, "base": [0, 0], "handles": handles})).unwrap();
        };
        block(&mut s, "PDF-SYM-12", 1.0, false);
        block(&mut s, "PDF-SYM-301", 4.0, false);
        block(&mut s, "PDF-SYM-7", 1.0, true);
        let d = s.doc().unwrap();
        let (a, b, c) = (symbol_shape(d, "PDF-SYM-12"), symbol_shape(d, "PDF-SYM-301"), symbol_shape(d, "PDF-SYM-7"));
        assert!(a.is_some() && a == b, "{a:?} {b:?}");
        assert_ne!(a, c);
        assert_eq!(symbol_shape(d, "NO-SUCH-BLOCK"), None);
        // An empty box is no keynote (furniture by a receptacle); the receptacle isn't one either.
        assert!(!plain_box(&c.unwrap()) && !plain_box(&a.unwrap()));
        let keynote: Shape = vec![(3, 0, 0, 0), (3, 16, 0, 0), (3, 16, 16, 0), (3, 0, 16, 0), (0, 8, 4, 0), (0, 8, 12, 0)];
        assert!(plain_box(&keynote));
        let round: Shape = vec![
            (3, 8, 0, 0),
            (3, 16, 8, 0),
            (3, 8, 16, 0),
            (3, 0, 8, 0),
            (3, 3, 2, 0),
            (3, 13, 2, 0),
            (3, 3, 14, 0),
            (3, 13, 14, 0),
            (0, 2, 6, 0),
            (0, 14, 6, 0),
        ];
        assert!(!plain_box(&round), "a circle drawn as a polygon is round");
        // A box with rounded corners, quantized onto its edges.
        let rounded: Shape =
            vec![(3, 0, 1, 0), (3, 0, 11, 0), (3, 1, 0, 0), (3, 1, 12, 0), (3, 15, 0, 0), (3, 15, 12, 0), (3, 16, 1, 0), (3, 16, 11, 0)];
        assert!(!plain_box(&rounded), "an eased box with nothing in it");
        // PDF-SYM-108 itself: an eased box with its "1" drawn inside.
        let keynote_1: Shape = vec![
            (0, 4, 11, 0),
            (0, 6, 0, 0),
            (3, 0, 0, 0),
            (3, 0, 1, 0),
            (3, 0, 10, 0),
            (3, 0, 11, 0),
            (3, 1, 0, 0),
            (3, 1, 11, 0),
            (3, 12, 2, 0),
            (3, 12, 7, 0),
            (3, 14, 0, 0),
            (3, 14, 11, 0),
        ];
        assert!(plain_box(&keynote_1));
    }

    #[test]
    fn a_legend_rows_description_names_its_devices_kind_and_ifc_class() {
        // The hotel's E-1 legend rows, as printed.
        for (row, want) in [
            ("DUPLEX RECEPTACLE - NEMA 5-20R, UNO", Some(("receptacle", "IfcOutlet"))),
            ("CEILING MOUNTED DUPLEX RECEPTACLE", Some(("ceiling_receptacle", "IfcOutlet"))),
            ("QUAD RECEPTACLE - NEMA 5-20R, UNO", Some(("quad_receptacle", "IfcOutlet"))),
            ("SINGLE POLE WALL SWITCH", Some(("switch", "IfcSwitchingDevice"))),
            ("SMOKE SENSOR", Some(("smoke_detector", "IfcSensor"))),
            ("FIRE ALARM HORN/STROBE", Some(("fire_alarm_notification", "IfcAlarm"))),
            ("TELEVISION OUTLET", Some(("tv_outlet", "IfcOutlet"))),
            ("TELEPHONE/DATA OUTLET", Some(("data_outlet", "IfcOutlet"))),
            ("OCCUPANCY SENSOR", Some(("occupancy_sensor", "IfcSensor"))),
            ("JUNCTION BOX", Some(("junction_box", "IfcJunctionBox"))),
            ("DISCONNECT SWITCH", Some(("disconnect", "IfcSwitchingDevice"))),
            ("ELECTRICAL PANELBOARD", Some(("panel", "IfcElectricDistributionBoard"))),
            ("INSTALL DEVICE AT HEIGHT INDICATED (+60\" = MOUNT AT 60\"AFF)", None),
            ("RECEPTACLE DESIGNATION - REFER TO ABBREVIATIONS ON THIS SHEET", None),
            ("LIGHTING FIXTURE - REFER TO LIGHT FIXTURE SCHEDULE", None),
        ] {
            assert_eq!(legend_kind(row), want, "{row}");
        }
    }

    #[test]
    fn a_detail_callouts_bubble_is_no_plumbing_fixture() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        let lo = s.doc().unwrap().handseed;
        let t = |s: &mut crate::Session, at: [f64; 2], v: &str| {
            s.execute("text", &json!({"at": at, "text": v, "height": 4.6})).unwrap();
        };
        // P-4: a mop sink tagged P-7, and the bubble of detail 7 on sheet P-7 (its view number over it).
        t(&mut s, [100.0, 100.0], "P-7");
        t(&mut s, [300.0, 100.0], "P-7");
        t(&mut s, [301.0, 108.0], "7");
        t(&mut s, [500.0, 100.0], "P-1");
        let hi = s.doc().unwrap().handseed;
        let region = |n: &str, x: f64| SheetRegion {
            number: n.into(),
            level: "3".into(),
            discipline: "P".into(),
            kind: "plan".into(),
            rect: [x, 0.0, x + 2000.0, 1400.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        };
        job.sheet_regions = vec![region("P-4", 0.0), region("P-7", 5000.0)];
        job.sheet_spans = vec![SheetSpan { sheet: "P-4".into(), lo, hi }];
        let row = |tag: &str| cadaddy_mepfix::fixture_schedule::ScheduledFixture { tag: tag.into(), ..Default::default() };
        let schedule = cadaddy_mepfix::fixture_schedule::FixtureSchedule { rows: vec![row("P-1"), row("P-7")], extent: [0.0; 4] };
        let found = plumbing_fixtures(s.doc().unwrap(), &job, &schedule);
        let mut tags: Vec<(&str, f64)> = found.iter().map(|f| (f.tag.as_str(), f.at[0])).collect();
        tags.sort_by(|a, b| a.1.total_cmp(&b.1));
        assert_eq!(tags, [("P-7", 100.0), ("P-1", 500.0)], "{found:?}");
    }

    #[test]
    fn a_typical_rooms_height_notes_go_to_its_devices_in_every_room() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        let line = |s: &mut crate::Session, a: [f64; 2], b: [f64; 2]| {
            s.execute("line", &json!({"points": [a, b]})).unwrap();
        };
        // The unit's linework (12' x 24', a cross wall every 4', a stub that tells its ends apart),
        // at `o`, turned half round with `turned`.
        let unit = |s: &mut crate::Session, o: [f64; 2], turned: bool| {
            let p = |x: f64, y: f64| if turned { [o[0] + 144.0 - x, o[1] + 288.0 - y] } else { [o[0] + x, o[1] + y] };
            let mut segs =
                vec![((0.0, 0.0), (144.0, 0.0)), ((144.0, 0.0), (144.0, 288.0)), ((144.0, 288.0), (0.0, 288.0)), ((0.0, 288.0), (0.0, 0.0))];
            for k in 1..6 {
                segs.push(((0.0, 48.0 * f64::from(k)), (144.0, 48.0 * f64::from(k))));
            }
            segs.push(((0.0, 100.0), (60.0, 100.0)));
            segs.push(((100.0, 210.0), (144.0, 210.0)));
            for ((ax, ay), (bx, by)) in segs {
                line(s, p(ax, ay), p(bx, by));
            }
        };
        // The E-6 sheet: its border, the view, a receptacle's circle with "44\" AFF" beside it, a
        // switch's circle with a leader from "MICROWAVE 65\" AFF".
        let x0 = 4234.0;
        let lo = s.doc().unwrap().handseed;
        for (a, b) in [([0.0, 0.0], [2000.0, 0.0]), ([2000.0, 0.0], [2000.0, 1400.0]), ([2000.0, 1400.0], [0.0, 1400.0]), ([0.0, 1400.0], [0.0, 0.0])]
        {
            line(&mut s, [x0 + a[0], a[1]], [x0 + b[0], b[1]]);
        }
        let v = [x0 + 300.0, 200.0];
        unit(&mut s, v, false);
        s.execute("circle", &json!({"center": [v[0] + 20.0, v[1] + 20.0], "radius": 4.0})).unwrap();
        s.execute("text", &json!({"at": [v[0] + 27.0, v[1] + 18.0], "text": "44\" AFF", "height": 4.0})).unwrap();
        s.execute("circle", &json!({"center": [v[0] + 130.0, v[1] + 120.0], "radius": 4.0})).unwrap();
        s.execute("text", &json!({"at": [v[0] + 40.0, v[1] + 150.0], "text": "MICROWAVE 65\" AFF", "height": 4.0})).unwrap();
        line(&mut s, [v[0] + 86.0, v[1] + 152.0], [v[0] + 130.0, v[1] + 124.0]);
        s.execute("text", &json!({"at": [v[0], v[1] - 19.0], "text": "ENLARGED DOUBLE QUEEN", "height": 7.4})).unwrap();
        s.execute("text", &json!({"at": [v[0], v[1] - 28.0], "text": "SCALE: 1/4\" = 1'-0\"", "height": 4.6})).unwrap();
        let hi = s.doc().unwrap().handseed;
        // The floor: room 201 as drawn, room 202 across the corridor (turned half round).
        unit(&mut s, [10_000.0, 0.0], false);
        unit(&mut s, [10_400.0, 0.0], true);
        job.sheet_regions = vec![SheetRegion {
            number: "E-6".into(),
            level: "T".into(),
            discipline: "E".into(),
            kind: "enlarged_plan".into(),
            rect: [x0, 0.0, x0 + 2000.0, 1400.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        }];
        job.sheet_spans = vec![SheetSpan { sheet: "E-6".into(), lo, hi }];
        let room = |n: &str, x: f64| json!({"name": "DOUBLE QUEEN", "number": n, "level": "Level 2", "outline": [[x, 0], [x + 144.0, 0], [x + 144.0, 288], [x, 288]]});
        if let Some(st) = job.stage_mut(StageName::Rooms) {
            st.result = Some(json!({"rooms": [room("201", 10_000.0), room("202", 10_400.0)]}));
        }
        let device = |tag: &str, at: [f64; 2]| super::super::readers::ReadDevice {
            tag: tag.into(),
            at,
            sheet: "E-6".into(),
            level: "T".into(),
            ..Default::default()
        };
        let read = [device("A", [v[0] + 20.0, v[1] + 20.0]), device("S", [v[0] + 130.0, v[1] + 120.0]), device("A", [v[0] + 120.0, v[1] + 270.0])];
        let unplaced: Vec<&super::super::readers::ReadDevice> = read.iter().collect();
        let mut stamped = stamp_typical_rooms(s.doc().unwrap(), &job, &[], &unplaced).devices;
        assert_eq!(stamped.len(), 6, "{stamped:?}");
        let (notes, set) = typical_heights(s.doc().unwrap(), &job, &mut stamped);
        assert_eq!((notes, set), (2, 4), "{stamped:?}");
        for room in ["201", "202"] {
            let h = |tag: &str| -> Vec<Option<f64>> { stamped.iter().filter(|d| d.room == room && d.tag == tag).map(|d| d.aff_in).collect() };
            assert_eq!(h("S"), [Some(65.0)], "room {room}: {stamped:?}");
            let mut a = h("A");
            a.sort_by(|x, y| x.unwrap_or(0.0).total_cmp(&y.unwrap_or(0.0)));
            assert_eq!(a, [None, Some(44.0)], "room {room}: the other receptacle has no note");
        }
    }

    #[test]
    fn an_enlarged_plan_after_an_enlarged_rcp_stamps_every_room_of_its_type() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        // One view per sheet: a 144 x 288 plan, its title over a SCALE line under it.
        let sheet = |s: &mut crate::Session, x0: f64, title: &str| -> (u64, u64) {
            let lo = s.doc().unwrap().handseed;
            let line = |s: &mut crate::Session, a: [f64; 2], b: [f64; 2]| {
                s.execute("line", &json!({"points": [a, b]})).unwrap();
            };
            for (a, b) in
                [([0.0, 0.0], [2000.0, 0.0]), ([2000.0, 0.0], [2000.0, 1400.0]), ([2000.0, 1400.0], [0.0, 1400.0]), ([0.0, 1400.0], [0.0, 0.0])]
            {
                line(s, [x0 + a[0], a[1]], [x0 + b[0], b[1]]);
            }
            let (vx, vy) = (x0 + 300.0, 200.0);
            for (a, b) in [([0.0, 0.0], [144.0, 0.0]), ([144.0, 0.0], [144.0, 288.0]), ([144.0, 288.0], [0.0, 288.0]), ([0.0, 288.0], [0.0, 0.0])] {
                line(s, [vx + a[0], vy + a[1]], [vx + b[0], vy + b[1]]);
            }
            for k in 1..6 {
                line(s, [vx, vy + 48.0 * f64::from(k)], [vx + 144.0, vy + 48.0 * f64::from(k)]);
            }
            s.execute("text", &json!({"at": [vx, vy - 19.0], "text": title, "height": 7.4})).unwrap();
            s.execute("text", &json!({"at": [vx, vy - 28.0], "text": "SCALE: 1/4\" = 1'-0\"", "height": 4.6})).unwrap();
            (lo, s.doc().unwrap().handseed)
        };
        let rcp = sheet(&mut s, 0.0, "ENLARGED DOUBLE QUEEN RCP");
        let plan = sheet(&mut s, 4234.0, "ENLARGED DOUBLE QUEEN");
        let region = |number: &str, x0: f64| SheetRegion {
            number: number.into(),
            level: "T".into(),
            discipline: number.chars().take(1).collect(),
            kind: "enlarged_plan".into(),
            rect: [x0, 0.0, x0 + 2000.0, 1400.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        };
        job.sheet_regions = vec![SheetRegion { kind: "rcp".into(), ..region("A621", 0.0) }, region("E-6", 4234.0)];
        job.sheet_spans = vec![SheetSpan { sheet: "A621".into(), lo: rcp.0, hi: rcp.1 }, SheetSpan { sheet: "E-6".into(), lo: plan.0, hi: plan.1 }];
        // Two DOUBLE QUEEN rooms on Level 2, the floor plan's own receptacle in the first.
        let room = |n: &str, x: f64| json!({"name": "DOUBLE QUEEN", "number": n, "level": "Level 2", "outline": [[x, 0], [x + 144.0, 0], [x + 144.0, 288], [x, 288]]});
        if let Some(st) = job.stage_mut(StageName::Rooms) {
            st.result = Some(json!({"rooms": [room("201", 10_000.0), room("202", 10_200.0)]}));
        }
        let device = |tag: &str, at: [f64; 2], sheet: &str, level: &str| super::super::readers::ReadDevice {
            tag: tag.into(),
            at,
            sheet: sheet.into(),
            level: level.into(),
            ..Default::default()
        };
        // The floor plan's own copy of one receptacle in room 201, where the enlarged plan's lands
        // (on the room's panel): it stands for it. A corridor's on a house panel by another: not.
        let on = |mut d: super::super::readers::ReadDevice, circuit: &str| {
            d.circuit = circuit.into();
            d
        };
        let placed = vec![on(device("A", [10_020.0, 20.0], "E-2", "Level 2"), "G-5"), on(device("A", [10_120.0, 20.0], "E-2", "Level 2"), "H-3")];
        // The enlarged plan's three receptacles and a switch, in its view.
        let read: Vec<_> =
            [("A", [4534.0 + 20.0, 220.0]), ("A", [4534.0 + 120.0, 220.0]), ("A", [4534.0 + 20.0, 460.0]), ("S", [4534.0 + 130.0, 300.0])]
                .iter()
                .map(|(t, at)| on(device(t, *at, "E-6", "T"), if *t == "A" { "G-5" } else { "" }))
                .collect();
        let unplaced: Vec<&super::super::readers::ReadDevice> = read.iter().collect();
        let stamped = stamp_typical_rooms(s.doc().unwrap(), &job, &placed, &unplaced);
        let in_room = |n: &str, tag: &str| stamped.devices.iter().filter(|d| d.room.contains(n) && d.tag == tag).count();
        assert_eq!((in_room("201", "A"), in_room("202", "A")), (3, 3), "{:?}", stamped.report);
        assert_eq!((in_room("201", "S"), in_room("202", "S")), (1, 1));
        assert_eq!(stamped.report.get("E-6 ENLARGED DOUBLE QUEEN"), Some(&8), "{:?}", stamped.report);
        // The floor plan's copy where one landed is its duplicate; the corridor's is not.
        assert_eq!(stamped.duplicates, BTreeSet::from([0]), "{:?}", stamped.duplicates);
        assert!(!stamped.report.keys().any(|k| k.contains("A621") || k.contains("no typical-room view")), "{:?}", stamped.report);
        // Each one inside its room.
        for d in &stamped.devices {
            let x0 = if d.room.contains("201") { 10_000.0 } else { 10_200.0 };
            assert!(d.at[0] >= x0 && d.at[0] <= x0 + 144.0 && d.at[1] >= 0.0 && d.at[1] <= 288.0, "{d:?}");
        }
    }

    /// A typical room's plan's pipe no registration placed goes into each room of its type, at
    /// its place in the room; a room the floor's own plans already run that system through gets
    /// none (no second count); a run outside every typical view stays unplaced.
    #[test]
    fn a_typical_rooms_pipe_goes_into_each_room_of_its_type() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        // One view per sheet: a 144 x 288 plan, its title over a SCALE line under it.
        let sheet = |s: &mut crate::Session, x0: f64, title: &str| -> (u64, u64) {
            let lo = s.doc().unwrap().handseed;
            let line = |s: &mut crate::Session, a: [f64; 2], b: [f64; 2]| {
                s.execute("line", &json!({"points": [a, b]})).unwrap();
            };
            for (a, b) in
                [([0.0, 0.0], [2000.0, 0.0]), ([2000.0, 0.0], [2000.0, 1400.0]), ([2000.0, 1400.0], [0.0, 1400.0]), ([0.0, 1400.0], [0.0, 0.0])]
            {
                line(s, [x0 + a[0], a[1]], [x0 + b[0], b[1]]);
            }
            let (vx, vy) = (x0 + 300.0, 200.0);
            for (a, b) in [([0.0, 0.0], [144.0, 0.0]), ([144.0, 0.0], [144.0, 288.0]), ([144.0, 288.0], [0.0, 288.0]), ([0.0, 288.0], [0.0, 0.0])] {
                line(s, [vx + a[0], vy + a[1]], [vx + b[0], vy + b[1]]);
            }
            for k in 1..6 {
                line(s, [vx, vy + 48.0 * f64::from(k)], [vx + 144.0, vy + 48.0 * f64::from(k)]);
            }
            s.execute("text", &json!({"at": [vx, vy - 19.0], "text": title, "height": 7.4})).unwrap();
            s.execute("text", &json!({"at": [vx, vy - 28.0], "text": "SCALE: 1/4\" = 1'-0\"", "height": 4.6})).unwrap();
            (lo, s.doc().unwrap().handseed)
        };
        let rcp = sheet(&mut s, 0.0, "ENLARGED DOUBLE QUEEN RCP");
        let plan = sheet(&mut s, 4234.0, "ENLARGED DOUBLE QUEEN");
        let region = |number: &str, x0: f64| SheetRegion {
            number: number.into(),
            level: "T".into(),
            discipline: number.chars().take(1).collect(),
            kind: "enlarged_plan".into(),
            rect: [x0, 0.0, x0 + 2000.0, 1400.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        };
        job.sheet_regions = vec![SheetRegion { kind: "rcp".into(), ..region("A621", 0.0) }, region("E-6", 4234.0)];
        job.sheet_spans = vec![SheetSpan { sheet: "A621".into(), lo: rcp.0, hi: rcp.1 }, SheetSpan { sheet: "E-6".into(), lo: plan.0, hi: plan.1 }];
        // Two DOUBLE QUEEN rooms on Level 2, the floor plan's own receptacle in the first.
        let room = |n: &str, x: f64| json!({"name": "DOUBLE QUEEN", "number": n, "level": "Level 2", "outline": [[x, 0], [x + 144.0, 0], [x + 144.0, 288], [x, 288]]});
        if let Some(st) = job.stage_mut(StageName::Rooms) {
            st.result = Some(json!({"rooms": [room("201", 10_000.0), room("202", 10_200.0)]}));
        }
        let run = |system: &str, level: &str, sheet: &str, points: Vec<[f64; 2]>| cadaddy_walk3d::MepRun {
            system: system.into(),
            level: level.into(),
            points,
            size_in: [2.0, 0.0],
            bottoms_ft: vec![0.0],
            objects: vec!["1A".into()],
            id: "R7".into(),
            encased: false,
            size_source: "tag".into(),
            height_source: "default".into(),
            reviewed: false,
            edited_by: String::new(),
            top_ft: None,
            source_sheet: sheet.into(),
        };
        // The sanitary line along the plan's long side; a vent line outside every view.
        let lost = vec![
            run("SAN", "T", "E-6", vec![[4534.0 + 20.0, 220.0], [4534.0 + 20.0, 460.0]]),
            run("V", "T", "E-6", vec![[4234.0 + 1500.0, 900.0], [4234.0 + 1600.0, 900.0]]),
        ];
        // The floor's plan already runs SAN through room 202.
        let placed = vec![run("SAN", "Level 2", "P-2", vec![[10_250.0, 50.0], [10_250.0, 150.0]])];
        let st = stamp_typical_runs(s.doc().unwrap(), &job, &placed, &lost);
        assert_eq!(st.taken, vec![true, false]);
        assert_eq!(st.runs.len(), 1, "room 201 only: {:?}", st.report);
        let r = &st.runs[0];
        assert_eq!((r.level.as_str(), r.system.as_str(), r.id.as_str()), ("Level 2", "SAN", ""));
        assert!(r.points.iter().all(|p| p[0] >= 10_000.0 && p[0] <= 10_144.0 && p[1] >= 0.0 && p[1] <= 288.0), "{:?}", r.points);
        assert!((r.points[1][1] - r.points[0][1]).abs() > 100.0, "it runs along the room: {:?}", r.points);
    }

    #[test]
    fn a_storey_drawn_a_foot_off_lines_up_on_the_rooms_of_the_one_under_it() {
        // Level 2's twelve rooms at their places; Level 3's sheet the same rooms 12 units (1')
        // right and 6 up of where its shell corner (its base) puts them.
        let mut s = crate::Session::new();
        for k in 0..12 {
            let (x, y) = (100.0 + 150.0 * f64::from(k % 6), 100.0 + 300.0 * f64::from(k / 6));
            s.execute("text", &json!({"at": [x, y], "text": format!("2{:02}", k + 1), "height": 8})).unwrap();
            s.execute("text", &json!({"at": [x + 5000.0 + 12.0, y + 6.0], "text": format!("3{:02}", k + 1), "height": 8})).unwrap();
        }
        // A stray number on Level 3 (a key note) doesn't move it.
        s.execute("text", &json!({"at": [5400.0, 900.0], "text": "399", "height": 8})).unwrap();
        let mut sp = cadaddy_walk3d::BuildingSpec::from_json(
            r#"{"version": 1, "units_per_foot": 12, "levels": [
                {"name": "Level 2", "elevation_ft": 10, "height_ft": 10, "region": [0, 0, 1200, 1000], "base": [0, 0]},
                {"name": "Level 3", "elevation_ft": 20, "height_ft": 10, "region": [5000, 0, 6200, 1000], "base": [5000, 0]}
            ]}"#,
        )
        .expect("spec");
        let (moved, _) = register_storeys_by_rooms(s.doc().unwrap(), &mut sp);
        assert_eq!(moved.len(), 1, "{moved:?}");
        let b = sp.level("Level 3").and_then(|l| l.base).expect("base");
        assert!((b[0] - 5012.0).abs() < 1e-6 && (b[1] - 6.0).abs() < 1e-6, "{b:?}");
        assert_eq!(sp.level("Level 2").and_then(|l| l.base), Some([0.0, 0.0]), "the one under it stays");
        // Lined up already: nothing moves; and a shift past 3' is a different floor, left alone.
        assert!(register_storeys_by_rooms(s.doc().unwrap(), &mut sp).0.is_empty());
        if let Some(l) = sp.levels.iter_mut().find(|l| l.name == "Level 3") {
            l.base = Some([5000.0 - 60.0, 0.0]);
        }
        let (moved, not) = register_storeys_by_rooms(s.doc().unwrap(), &mut sp);
        assert!(moved.is_empty() && not.iter().any(|n| n.contains("more than")), "{moved:?} {not:?}");
    }

    #[test]
    fn a_circuits_route_in_a_typical_rooms_view_is_left_to_the_rooms_own_circuits() {
        use cadaddy_doc::{Drawing, Line};
        use cadaddy_geom::Vec2;
        let mut d = Drawing::default();
        let line = |d: &mut Drawing, a: (f64, f64), b: (f64, f64)| {
            d.add(&Space::Model, Common::default(), EntityKind::Line(Line { a: Vec3::new(a.0, a.1, 0.0), b: Vec3::new(b.0, b.1, 0.0) })).ok();
        };
        // A sheet's border, and two views side by side on it: a room's walls with a title and a
        // scale under it.
        line(&mut d, (0.0, 0.0), (2400.0, 0.0));
        line(&mut d, (2400.0, 0.0), (2400.0, 1600.0));
        line(&mut d, (2400.0, 1600.0), (0.0, 1600.0));
        line(&mut d, (0.0, 1600.0), (0.0, 0.0));
        for (x0, title) in [(100.0, "ENLARGED QUEEN"), (900.0, "ENLARGED BOH ROOM #237")] {
            line(&mut d, (x0, 90.0), (x0 + 600.0, 90.0));
            line(&mut d, (x0 + 600.0, 90.0), (x0 + 600.0, 510.0));
            line(&mut d, (x0 + 600.0, 510.0), (x0, 510.0));
            line(&mut d, (x0, 510.0), (x0, 90.0));
            line(&mut d, (x0 + 300.0, 90.0), (x0 + 300.0, 510.0));
            d.add(&Space::Model, Common::default(), EntityKind::Text(text(title, x0 + 13.0, 71.0, 7.4))).ok();
            d.add(&Space::Model, Common::default(), EntityKind::Text(text("SCALE:  1/4\" = 1'-0\"", x0 + 13.0, 62.0, 4.6))).ok();
        }
        let sq = vec![Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 10.0)];
        let rooms: Vec<JobRoom> = vec![
            ("QUEEN".to_string(), "2".to_string(), sq.clone(), "201".to_string()),
            ("QUEEN".to_string(), "3".to_string(), sq.clone(), "301".to_string()),
            ("BOH".to_string(), "2".to_string(), sq.clone(), "237".to_string()),
        ];
        let (types, vocabulary) = room_types(&rooms);
        let texts = cadaddy_mepfix::sheet_facts::texts(&d);
        let (views, one_room) = room_views(&d, &texts, &types, &vocabulary, &rooms);
        assert_eq!(views.iter().map(|v| v.0.as_str()).collect::<Vec<_>>(), ["ENLARGED QUEEN"], "{views:?}");
        assert_eq!(one_room.iter().map(|v| (v.0.as_str(), v.2)).collect::<Vec<_>>(), [("ENLARGED BOH ROOM #237", 2)]);
        // An estimated route starting in either view is the rooms'; one elsewhere on the sheet, or
        // drawn conduit, is the floor's.
        let regions: BTreeMap<String, Vec<[f64; 4]>> =
            [("E-6".to_string(), views.iter().map(|v| v.1).chain(one_room.iter().map(|v| v.1)).collect())].into_iter().collect();
        let run =
            |system: &str, sheet: &str, at: [f64; 2]| super::super::printcond::run(system, "2", sheet, vec![at, [at[0] + 10.0, at[1]]], 0.75, 9.0);
        let est = super::super::printcond::CIRCUIT_SYSTEM;
        assert!(in_room_view(&run(est, "E-6", [300.0, 300.0]), &regions));
        assert!(in_room_view(&run(est, "E-6/1", [1000.0, 300.0]), &regions), "the sheet's view number aside");
        assert!(!in_room_view(&run(est, "E-6", [800.0, 300.0]), &regions), "between the views");
        assert!(!in_room_view(&run(est, "E-3", [300.0, 300.0]), &regions), "another sheet");
        assert!(!in_room_view(&run(super::super::printcond::DRAWN_SYSTEM, "E-6", [300.0, 300.0]), &regions), "drawn conduit");
    }

    #[test]
    fn a_fire_alarm_plans_marks_are_fire_alarm_devices() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        job.sheets = Some(json!({"sheets": [{"number": "Q-7", "sheet_type": "plan", "level": "1", "title": "FIRE ALARM - 1ST FLOOR"}]}));
        job.sheets_loaded = true;
        let texts: Vec<(&str, f64, f64, f64)> = vec![
            ("DEVICE SCHEDULE", 1000.0, 800.0, 8.0),
            ("TYPE", 1000.0, 785.0, 5.0),
            ("DESCRIPTION", 1030.0, 785.0, 5.0),
            ("S", 1000.0, 772.0, 5.0),
            ("SMOKE DETECTOR", 1030.0, 772.0, 5.0),
            // A lone letter on a fire alarm plan is no switch's leg.
            ("c", 400.0, 100.0, 2.0),
        ];
        let syms: Vec<(&str, f64, f64)> = (0..3).map(|k| ("S", 100.0 + 40.0 * f64::from(k), 300.0)).collect();
        let lo = s.doc().map_or(0, |d| d.handseed);
        add_sheet(&mut s, &texts, &syms);
        let hi = s.doc().map_or(0, |d| d.handseed);
        job.sheet_spans.push(SheetSpan { sheet: "Q-7".into(), lo, hi });
        job.sheet_regions.push(SheetRegion {
            number: "Q-7".into(),
            level: "1".into(),
            discipline: "E".into(),
            kind: "plan".into(),
            rect: [0.0, 0.0, 2000.0, 1000.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        });
        run_devices(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(job.devices.len(), 3, "{:?}", job.devices);
        assert!(job.devices.iter().all(|d| d.tag == "S" && d.kind == "fire alarm device" && d.leg.is_empty()), "{:?}", job.devices);
    }

    /// A fire alarm plan's resident room detector (`S` with a subscript `R` at its lower right) is
    /// one device marked `SR`; the corridor's plain `S` stays an `S`.
    #[test]
    fn a_fire_alarm_marks_subscript_is_part_of_its_mark() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        job.sheets = Some(json!({"sheets": [{"number": "Q-7", "sheet_type": "plan", "level": "1", "title": "FIRE ALARM - 1ST FLOOR"}]}));
        job.sheets_loaded = true;
        let syms: Vec<(&str, f64, f64)> = vec![("S", 100.0, 300.0), ("R", 102.4, 297.6), ("S", 200.0, 300.0)];
        let lo = s.doc().map_or(0, |d| d.handseed);
        add_sheet(&mut s, &[], &syms);
        let hi = s.doc().map_or(0, |d| d.handseed);
        job.sheet_spans.push(SheetSpan { sheet: "Q-7".into(), lo, hi });
        job.sheet_regions.push(SheetRegion {
            number: "Q-7".into(),
            level: "1".into(),
            discipline: "E".into(),
            kind: "plan".into(),
            rect: [0.0, 0.0, 2000.0, 1000.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        });
        run_devices(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        let mut tags: Vec<&str> = job.devices.iter().map(|d| d.tag.as_str()).collect();
        tags.sort_unstable();
        assert_eq!(tags, vec!["S", "SR"], "{:?}", job.devices);
    }

    #[test]
    fn devices_read_reach_the_model_and_the_bom() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Takeoff);
        job.sheets =
            Some(json!({"sheets": [{"number": "Q-1", "sheet_type": "plan", "level": "1"}, {"number": "Q-2", "sheet_type": "plan", "level": "2"}]}));
        job.sheets_loaded = true;
        let mut want_by_level = BTreeMap::new();
        for (i, (name, level, a, b)) in [("Q-1", "1", 5, 3), ("Q-2", "2", 4, 2)].into_iter().enumerate() {
            let x0 = 3000.0 * i as f64;
            let texts: Vec<(&str, f64, f64, f64)> = vec![
                ("LIGHTING FIXTURE SCHEDULE", x0 + 1000.0, 800.0, 8.0),
                ("TYPE", x0 + 1000.0, 785.0, 5.0),
                ("DESCRIPTION", x0 + 1030.0, 785.0, 5.0),
                ("A", x0 + 1000.0, 772.0, 5.0),
                ("2X4 LED TROFFER", x0 + 1030.0, 772.0, 5.0),
                ("B", x0 + 1000.0, 760.0, 5.0),
                ("LED DOWNLIGHT", x0 + 1030.0, 760.0, 5.0),
                // The circuit printed beside the first A.
                ("D-7", x0 + 108.0, 296.0, 2.0),
            ];
            let mut syms: Vec<(&str, f64, f64)> = vec![("A", x0 + 990.0, 772.0)]; // the schedule's sample
            syms.extend((0..a).map(|k| ("A", x0 + 100.0 + 40.0 * f64::from(k), 300.0)));
            syms.extend((0..b).map(|k| ("B", x0 + 100.0 + 40.0 * f64::from(k), 200.0)));
            // Junk: the sheet's own number, a keynote, an untagged symbol; a receptacle, its
            // circuit printed by its symbol; and a switch, its leg letter printed by its symbol.
            syms.extend([
                ("D-1", x0 + 600.0, 100.0),
                (name, x0 + 620.0, 100.0),
                ("s", x0 + 640.0, 100.0),
                ("12", x0 + 660.0, 100.0),
                ("", x0 + 680.0, 100.0),
            ]);
            let lo = s.doc().map_or(0, |d| d.handseed);
            add_sheet(&mut s, &texts, &syms);
            let hi = s.doc().map_or(0, |d| d.handseed);
            job.sheet_spans.push(SheetSpan { sheet: name.into(), lo, hi });
            job.sheet_regions.push(SheetRegion {
                number: name.into(),
                level: level.into(),
                discipline: "E".into(),
                kind: "plan".into(),
                rect: [x0, 0.0, x0 + 2000.0, 1000.0],
                area: None,
                title_block: None,
                not_built: String::new(),
                levels: Vec::new(),
                view: None,
            });
            want_by_level.insert(format!("Level {level}"), a + b + 2);
        }
        let n: i32 = want_by_level.values().sum();
        let r = run_devices(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r["devices"]["count"], json!(n), "{r:#}");
        assert_eq!(r["devices"]["by_type"], json!({"A": 9, "B": 5, "REC": 2, "S": 2}));
        assert_eq!(job.devices.iter().filter(|d| d.kind == "receptacle" && d.circuit == "D-1").count(), 2, "a receptacle on D-1 a sheet");
        assert_eq!(job.devices.iter().filter(|d| d.tag == "S" && d.leg == "s").count(), 2, "a switch on leg s a sheet");
        // No building model yet: the bim_build stage puts them in.
        assert_eq!(r["model"]["pending"], json!(n), "{:#}", r["model"]);
        assert_eq!(job.devices.len(), n as usize);
        let first = job.devices.iter().find(|d| d.sheet == "Q-1" && d.at == [100.0, 300.0]).unwrap_or_else(|| panic!("{:?}", job.devices));
        assert_eq!((first.level.as_str(), first.circuit.as_str()), ("1", "D-7"));
        assert_eq!(
            job.devices.iter().filter(|d| !d.circuit.is_empty() && d.kind != "receptacle").count(),
            2,
            "one circuit is printed by an A a sheet"
        );

        let built = run_bim_build(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(built["devices"]["inserted"], json!(n), "{:#}", built["devices"]);
        assert_eq!(built["devices"]["by_level"], json!(want_by_level));
        let d = s.doc().unwrap_or_else(|e| panic!("{e}"));
        let sp = cadaddy_walk3d::BuildingSpec::from_json(d.building_spec.as_deref().unwrap_or("")).unwrap_or_else(|e| panic!("{e}"));
        let devs: Vec<&cadaddy_walk3d::tags::ElementTag> = sp.tags.iter().filter(|t| t.id.starts_with("device:")).collect();
        assert_eq!(devs.len(), n as usize);
        let e = devs.iter().find(|t| t.fields.contains_key("circuit")).unwrap_or_else(|| panic!("no circuit on any device"));
        assert_eq!((e.fields["circuit"].value.as_str(), e.fields["type"].value.as_str()), ("D-7", "A"));
        assert!(e.level.starts_with("Level ") && e.fields["sheet"].value.starts_with("Q-"), "{e:?}");
        assert_eq!(crate::cmd::readers::model_device_counts(d)["count"], json!(n));

        // The BOM: one electrical device line per type and level, with as many units.
        let bom = s.execute("bim.bom", &json!({"group_by": "level"})).unwrap_or_else(|e| panic!("{e}"));
        let lines: Vec<&Value> = bom["lines"].as_array().into_iter().flatten().filter(|l| l["category"] == "devices").collect();
        let units = |lv: &str| lines.iter().filter(|l| l["level"] == lv).filter_map(|l| l["net"].as_f64()).sum::<f64>();
        // The draft spec has one level, `Level 1`; sheet level 2 is named alike.
        assert_eq!((units("Level 1"), units("Level 2")), (10.0, 8.0), "{lines:#?}");
        assert!(lines.iter().any(|l| l["item"] == "duplex receptacle"), "{lines:#?}");
        assert!(lines.iter().all(|l| l["trade"] == "electrical"));
        assert!(lines.iter().any(|l| l["item"] == "device type A" && l["level"] == "Level 2" && l["net"] == json!(4.0)), "{lines:#?}");
        // The takeoff stage uses the model takeoff (bim.takeoff) and reports a summary.
        let t = run_takeoff(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        let summary = &t["summary"];
        assert!(summary["line_count"].as_u64().unwrap_or(0) > 0, "takeoff should have lines: {t:#}");
        assert!(summary["trades"].as_array().is_some_and(|a| a.iter().any(|v| v == "electrical")), "trades should include electrical: {summary:#}");
        assert!(summary["levels"].as_array().is_some_and(|a| !a.is_empty()), "levels should be reported: {summary:#}");
        // The device lines in bim.takeoff should account for at least the device count.
        let device_count: f64 =
            t["lines"].as_array().into_iter().flatten().filter(|l| l["category"] == "devices").filter_map(|l| l["qty"].as_f64()).sum();
        assert!(device_count >= f64::from(n), "device count {device_count} should be >= {n}: {:#}", t["lines"]);
        // Built again: the same devices, not twice as many.
        let again = run_bim_build(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(again["devices"]["inserted"], json!(n));
        assert_eq!(s.doc().map(crate::cmd::readers::model_device_counts).unwrap_or_default()["count"], json!(n));
    }

    /// `mep.read_devices` on an open drawing: a preview by default, says why it can't insert
    /// without a model, and with one inserts the devices but never the junk (Q5271 MUSTS 3, 4).
    #[test]
    fn read_devices_inserts_into_the_open_model() {
        let mut s = crate::Session::new();
        let texts: Vec<(&str, f64, f64, f64)> = vec![
            ("LIGHTING FIXTURE SCHEDULE", 1000.0, 800.0, 8.0),
            ("TYPE", 1000.0, 785.0, 5.0),
            ("DESCRIPTION", 1030.0, 785.0, 5.0),
            ("A", 1000.0, 772.0, 5.0),
            ("2X4 LED TROFFER", 1030.0, 772.0, 5.0),
            ("B", 1000.0, 760.0, 5.0),
            ("LED DOWNLIGHT", 1030.0, 760.0, 5.0),
        ];
        let mut syms: Vec<(&str, f64, f64)> = vec![("A", 990.0, 772.0)]; // the schedule's sample
        syms.extend((0..3).map(|k| ("A", 100.0 + 40.0 * f64::from(k), 300.0)));
        syms.extend((0..2).map(|k| ("B", 100.0 + 40.0 * f64::from(k), 200.0)));
        syms.extend([("ZZ", 600.0, 100.0), ("D-1", 620.0, 100.0), ("s", 640.0, 100.0)]);
        add_sheet(&mut s, &texts, &syms);
        let read = |s: &mut crate::Session, p: Value| s.execute("mep.read_devices", &p).unwrap_or_else(|e| panic!("{e}"));
        let r = read(&mut s, json!({}));
        assert_eq!((&r["model"]["inserted"], &r["model"]["would_insert"]), (&json!(0), &json!(5)), "{:#}", r["model"]);
        let r = read(&mut s, json!({"preview": false}));
        assert_eq!(r["model"]["inserted"], json!(0));
        let why = r["model"]["why_not"].as_str().unwrap_or_default();
        assert!(why.contains("no building model") && why.contains("bspec.draft"), "{why}");
        s.execute("bspec.draft", &json!({"store": true})).unwrap_or_else(|e| panic!("{e}"));
        let r = read(&mut s, json!({"preview": false}));
        assert_eq!(r["model"]["inserted"], json!(5), "{:#}", r["model"]);
        assert_eq!(r["model"]["by_type"], json!({"A": 3, "B": 2}));
        assert_eq!(r["model"]["not_inserted"]["legend or schedule sample"], json!(2), "{:#}", r["model"]);
        assert_eq!(r["model"]["not_inserted"]["not in the legend (the guided job asks about it)"], json!(1));
        // Read again: the same five, not ten; the BOM has five units.
        let r = read(&mut s, json!({"preview": false}));
        assert_eq!((&r["model"]["inserted"], &r["model"]["replaced"]), (&json!(5), &json!(5)));
        let bom = s.execute("bim.bom", &json!({})).unwrap_or_else(|e| panic!("{e}"));
        let units: f64 = bom["lines"].as_array().into_iter().flatten().filter(|l| l["category"] == "devices").filter_map(|l| l["net"].as_f64()).sum();
        assert_eq!(units, 5.0, "{:#}", bom["lines"]);
    }

    #[test]
    fn a_device_takes_the_circuit_stacked_under_its_own_tag() {
        use cadaddy_geom::Vec2;
        use cadaddy_mepfix::sheet_facts::TextItem;
        let text = |v: &str, x: f64, y: f64| TextItem { value: v.into(), at: Vec2::new(x, y), height: 1.0, rotation: 0.0 };
        // Another device's pair above the symbol, this one's tag and circuit below it.
        let texts = vec![text("A", 0.0, 4.0), text("A-3,e", 0.0, 2.6), text("L4", 1.0, -2.0), text("A-2", 1.0, -3.4)];
        let circuits: Vec<&TextItem> = texts.iter().filter(|t| circuit_text(&t.value, &BTreeSet::new())).collect();
        let symbol = Vec2::new(1.5, 0.8);
        assert_eq!(super::super::readers::circuit_of(&texts, &circuits, "L4", None, symbol), "A-2");
        // The import keeps the tag on the symbol, where it was printed.
        let kept: Vec<TextItem> = texts.iter().filter(|t| t.value != "L4").cloned().collect();
        let circuits: Vec<&TextItem> = kept.iter().filter(|t| circuit_text(&t.value, &BTreeSet::new())).collect();
        assert_eq!(super::super::readers::circuit_of(&kept, &circuits, "L4", Some((Vec2::new(1.0, -2.0), 1.0)), symbol), "A-2");
        // No tag text found: the circuit nearest the symbol.
        assert_eq!(super::super::readers::circuit_of(&kept, &circuits, "L4", None, symbol), "A-3,e");
    }

    #[test]
    fn circuit_tags_are_not_device_types() {
        for t in ["D-1", "DP1-13", "DP2-4", "B-12", "A-2"] {
            assert!(super::is_circuit_tag(t), "{t}");
        }
        for t in ["A", "OS", "EF", "L4", "XA", "DH", "REC", "-1", "A-", "D-1A"] {
            assert!(!super::is_circuit_tag(t), "{t}");
        }
    }

    use super::*;

    #[test]
    fn guided_job_new_sets_stages() {
        let job = GuidedJob::new("/tmp/prints", JobGoal::Estimate);
        assert_eq!(job.stages.len(), StageName::ALL.len());
        // Clash should be skipped for estimate goal.
        let clash = job.stages.iter().find(|s| s.name == StageName::Clash).unwrap();
        assert_eq!(clash.status, StageStatus::Skipped);
        // Import should be pending.
        let import = job.stages.iter().find(|s| s.name == StageName::Import).unwrap();
        assert_eq!(import.status, StageStatus::Pending);
    }

    #[test]
    fn guided_job_new_bim_skips_late_stages() {
        let job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        let takeoff = job.stages.iter().find(|s| s.name == StageName::Takeoff).unwrap();
        assert_eq!(takeoff.status, StageStatus::Skipped);
        let estimate = job.stages.iter().find(|s| s.name == StageName::Estimate).unwrap();
        assert_eq!(estimate.status, StageStatus::Skipped);
        let bim = job.stages.iter().find(|s| s.name == StageName::BimBuild).unwrap();
        assert_eq!(bim.status, StageStatus::Pending);
    }

    #[test]
    fn guided_job_progress() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        assert!(job.progress() < 0.01); // Nothing done.
        // Mark import and classify done.
        if let Some(st) = job.stage_mut(StageName::Import) {
            st.status = StageStatus::Done;
        }
        if let Some(st) = job.stage_mut(StageName::Classify) {
            st.status = StageStatus::Done;
        }
        let p = job.progress();
        assert!(p > 0.1 && p < 0.9);
    }

    #[test]
    fn guided_job_question_lifecycle() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Estimate);
        let qid = job.next_qid(StageName::Levels);
        assert_eq!(qid, "Q-levels-1");
        let q = Question {
            id: qid.clone(),
            stage: StageName::Levels,
            question: "Scale reads 1/8\"=1'-0\" — confirm?".into(),
            why: "Low confidence".into(),
            choices: vec!["Yes".into(), "No".into()],
            evidence: None,
            default: Some("Yes".into()),
            answer_schema: None,
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: QuestionKind::Doubt,
        };
        if let Some(st) = job.stage_mut(StageName::Levels) {
            st.questions.push(q);
            st.status = StageStatus::Paused;
        }
        // Should find the unanswered question.
        assert!(job.next_question().is_some());
        assert_eq!(job.next_question().unwrap().id, qid);

        // Answer it.
        if let Some(st) = job.stage_mut(StageName::Levels)
            && let Some(q) = st.questions.first_mut()
        {
            q.answer = Some(json!("Yes"));
            q.answered_by = Some("agent".into());
        }
        // No more questions from that stage.
        assert!(job.stage_mut(StageName::Levels).unwrap().unanswered().is_empty());
    }

    #[test]
    fn guided_job_learn_rule() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Estimate);
        job.learn_rule("symbol:HEXAGON_WP", "device:weatherproof_receptacle", "Q-devices-1", StageName::Devices);
        assert_eq!(job.rules.len(), 1);
        assert_eq!(job.rules[0].key, "symbol:HEXAGON_WP");
        // Update same key.
        job.learn_rule("symbol:HEXAGON_WP", "device:wet_location_receptacle", "Q-devices-2", StageName::Devices);
        assert_eq!(job.rules.len(), 1);
        assert_eq!(job.rules[0].value, "device:wet_location_receptacle");
    }

    #[test]
    fn guided_job_audit_trim() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Estimate);
        for i in 0..MAX_AUDIT + 100 {
            job.audit("test", "test", None, None, Some(json!({"i": i})));
        }
        assert!(job.audit.len() <= MAX_AUDIT);
    }

    #[test]
    fn days_to_ymd_epoch() {
        let (y, m, d) = days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_date() {
        // 2026-10-23 = day 20749 since epoch.
        let (y, m, d) = days_to_ymd(20749);
        assert_eq!((y, m, d), (2026, 10, 23));
    }

    #[test]
    fn stage_name_ordering() {
        assert!(StageName::Import < StageName::Classify);
        assert!(StageName::Classify < StageName::Levels);
        assert!(StageName::Levels < StageName::BimBuild);
        assert!(StageName::BimBuild < StageName::Takeoff);
    }

    #[test]
    fn job_goal_last_stage() {
        assert_eq!(JobGoal::Bim.last_stage(), StageName::BimBuild);
        assert_eq!(JobGoal::Takeoff.last_stage(), StageName::Takeoff);
        assert_eq!(JobGoal::Estimate.last_stage(), StageName::Estimate);
        assert_eq!(JobGoal::Clash.last_stage(), StageName::Clash);
        assert_eq!(JobGoal::Sequence.last_stage(), StageName::Clash);
        assert_eq!(JobGoal::from_str("sequence"), Some(JobGoal::Sequence));
    }

    /// The `sequence` goal runs everything through clash detection, then builds the
    /// construction sequence onto the model (Q6014 MUST 5).
    #[test]
    fn sequence_goal_builds_the_sequence_after_clash() {
        let mut s = Session::new();
        s.execute(
            "bspec.set",
            &json!({"replace": true, "spec": {
                "levels": [{"name": "L1", "elevation_ft": 0, "height_ft": 13, "ceiling_ft": 9, "region": [-1000, -1000, 1000, 1000], "base": [0, 0]}],
                "mep": {
                    "systems": [{"name": "M-SUPPLY", "layer": "M-DUCT-SUPP", "kind": "duct", "size_in": [12, 12], "bottom_ft": 10}],
                    "runs": [{"system": "M-SUPPLY", "level": "L1", "points": [[0, 120], [480, 120]], "size_in": [12, 12], "bottoms_ft": [10]}]
                }
            }}),
        )
        .unwrap();
        s.execute("model.build", &json!({})).unwrap();
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Sequence);
        assert!(job.stages.iter().all(|st| st.status != StageStatus::Skipped), "every stage runs for a sequence");
        let out = run_clash(&mut s, &mut job).unwrap();
        assert!(out["sequence"]["activities"].as_u64().unwrap_or(0) >= 1, "{out}");
        assert!(out["sequence"].get("list").is_none(), "the summary, not every activity");
        assert!(out["sequence_conflicts"]["ok"].is_boolean(), "{out}");
        assert!(!s.doc().unwrap().schedule4d.as_deref().unwrap_or_default().is_empty(), "the sequence is the drawing's 4D schedule");
        // Other goals don't sequence.
        let mut clash = GuidedJob::new("/tmp/prints", JobGoal::Clash);
        assert!(run_clash(&mut s, &mut clash).unwrap().get("sequence").is_none());
    }

    #[test]
    fn guided_job_serialization() {
        let job = GuidedJob::new("/tmp/test-prints", JobGoal::Estimate);
        let json = serde_json::to_string(&job).unwrap();
        let back: GuidedJob = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, job.id);
        assert_eq!(back.goal, job.goal);
        assert_eq!(back.stages.len(), job.stages.len());
    }

    #[test]
    fn question_evidence_serialization() {
        let ev = QuestionEvidence {
            crop_png_path: Some("/tmp/crop.png".into()),
            sheet: Some("E-4".into()),
            at: Some([100.0, 200.0]),
            values: Some(json!({"scale": "1/8\"=1'-0\""})),
            handles: vec!["10A".into()],
            tiled_crops: None,
            leftovers: vec![LeftoverCandidate {
                description: "hexagon with WP".into(),
                crop_path: None,
                at: Some([50.0, 60.0]),
                sheet: Some("E-4".into()),
                count: 6,
            }],
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["sheet"], "E-4");
        assert_eq!(json["leftovers"][0]["count"], 6);
    }

    #[test]
    fn check_rejected_samples_detects_rejection() {
        let mut stage = StageState::new(StageName::Rooms);
        stage.verify.push(Question {
            id: "Q-rooms-1".into(),
            stage: StageName::Rooms,
            question: "Room correct?".into(),
            why: "verify".into(),
            choices: vec!["Correct".into(), "Wrong name".into()],
            evidence: None,
            default: None,
            answer_schema: None,
            answer: Some(json!("Correct")),
            answered_by: Some("agent".into()),
            answered_at: None,
            kind: QuestionKind::Verify,
        });
        assert!(!check_rejected_samples(&stage));

        stage.verify.push(Question {
            id: "Q-rooms-2".into(),
            stage: StageName::Rooms,
            question: "Room correct?".into(),
            why: "verify".into(),
            choices: vec!["Correct".into(), "Wrong name".into()],
            evidence: None,
            default: None,
            answer_schema: None,
            answer: Some(json!("Wrong name")),
            answered_by: Some("agent".into()),
            answered_at: None,
            kind: QuestionKind::Verify,
        });
        assert!(check_rejected_samples(&stage));
    }

    fn region(number: &str, level: &str, discipline: &str, kind: &str, x: f64) -> SheetRegion {
        SheetRegion {
            number: number.into(),
            level: level.into(),
            discipline: discipline.into(),
            kind: kind.into(),
            rect: [x, 0.0, x + 100.0, 80.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        }
    }

    /// Sheet `number` printing level 1's plan under level 2's: one region per view.
    fn two_views(number: &str, discipline: &str, x: f64) -> Vec<SheetRegion> {
        let view = |level: &str, y0: f64, y1: f64| SheetRegion {
            view: Some(SheetView { title: format!("LEVEL {level} PLAN"), rect: [x, y0, x + 100.0, y1] }),
            ..region(number, level, discipline, "plan", x)
        };
        vec![view("1", 0.0, 40.0), view("2", 40.0, 80.0)]
    }

    #[test]
    fn a_two_plan_sheet_gives_each_level_its_own_view() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        job.sheet_regions = two_views("A-1A", "A", 0.0);
        job.sheet_regions.extend(two_views("M-1A", "M", 200.0));
        job.sheet_regions.push(region("A-103", "3", "A", "plan", 400.0));
        // What is read on the sheet carries the level of the view it is in.
        let level = |at: [f64; 2]| job.level_at(at).map(|r| r.level.clone());
        assert_eq!(level([50.0, 10.0]).as_deref(), Some("1"));
        assert_eq!(level([50.0, 70.0]).as_deref(), Some("2"));
        assert_eq!(level([250.0, 70.0]).as_deref(), Some("2"));
        assert_eq!(level([450.0, 70.0]).as_deref(), Some("3"));
        // Each level's plan region is its view, not the whole sheet.
        let levels = building_levels(&job);
        let names: Vec<&str> = levels.iter().filter_map(|l| l["name"].as_str()).collect();
        assert_eq!(names, ["Level 1", "Level 2", "Level 3"], "{levels:?}");
        let region_of = |i: usize| [0, 1, 2, 3].map(|k| levels[i]["region"][k].as_f64().unwrap_or(f64::NAN));
        assert_eq!(region_of(0), [0.0, 0.0, 100.0, 40.0]);
        assert_eq!(region_of(1), [0.0, 40.0, 100.0, 80.0]);
        // The readers read each view on its own level, sharing the sheet's imported objects.
        job.sheet_spans = vec![SheetSpan { sheet: "A-1A".into(), lo: 1, hi: 10 }, SheetSpan { sheet: "M-1A".into(), lo: 10, hi: 20 }];
        let refs = job_sheet_refs(&job);
        let m: Vec<_> = refs.iter().filter(|r| r.number == "M-1A").collect();
        assert_eq!(m.len(), 2);
        assert!(m.iter().all(|r| r.span == Some((10, 20)) && r.levels.is_empty()), "{m:?}");
        assert_eq!(m.iter().map(|r| r.view.map(|v| v[1])).collect::<Vec<_>>(), [Some(0.0), Some(40.0)]);
        // The spec's sheet links: one per view, each on its level with its view.
        let mut sp = cadaddy_walk3d::BuildingSpec::default();
        add_sheet_links(&mut sp, &job.sheet_regions);
        let links: Vec<_> = sp.sheets.iter().filter(|l| l.number == "M-1A").collect();
        assert_eq!(links.len(), 2, "{:?}", sp.sheets);
        assert_eq!(links.iter().map(|l| l.level.clone().unwrap_or_default()).collect::<Vec<_>>(), ["Level 1", "Level 2"]);
        assert!(links.iter().all(|l| l.view.is_some()));
        // A typical plan (one view of several levels) stays one region feeding each level.
        let typical = SheetRegion { levels: vec!["2".into(), "3".into()], ..region("M-201", "2", "M", "plan", 600.0) };
        let kept = split_view_regions(&cadaddy_doc::Drawing::new_imperial(), vec![typical.clone()], &[true]);
        assert_eq!(kept, vec![typical]);
    }

    #[test]
    fn a_plan_of_uncertain_level_is_asked_never_put_on_level_1() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        let sheet = json!({"number": "P-201", "title": "PLUMBING PLAN", "level": "", "level_source": "ask"});
        job.index = Some(json!({"sheets": [sheet.clone()]}));
        job.sheets = Some(json!({"sheets": [sheet.clone()]}));
        let mut r = region("P-201", "", "P", "plan", 0.0);
        r.not_built = not_built_reason("P", "PLUMBING PLAN", "", "ask").to_string();
        assert_eq!(r.not_built, "no level");
        job.sheet_regions = vec![r];
        let codes: Vec<String> = ["1", "2", "R"].iter().map(|c| c.to_string()).collect();
        let mut q = plan_level_question(&mut job, &sheet, &codes);
        assert!(q.question.contains("which level"), "{}", q.question);
        assert_eq!(q.choices, ["Level 1", "Level 2", "Roof", "Not a level plan"]);
        assert_eq!(q.default.as_deref(), Some("Not a level plan"), "auto mode keeps it off every level");
        // Auto mode takes the default: the sheet stays a reference sheet, read as nothing.
        q.answer = Some(json!("Not a level plan"));
        answer_effect(&mut job, &q);
        assert_eq!((job.sheet_regions[0].level.as_str(), job.sheet_regions[0].not_built.as_str()), ("", "no level"));
        assert!(job_sheet_refs(&job).iter().all(|r| !r.builds()));
        // The site is no level a plan can be put on.
        q.answer = Some(json!("Level SITE"));
        answer_effect(&mut job, &q);
        assert_eq!(job.sheet_regions[0].level, "");
        // An answer puts it on that level, and it is built.
        q.answer = Some(json!("Level 2"));
        let (rerun, said) = answer_effect(&mut job, &q);
        assert_eq!((rerun, said.as_str()), (Some(StageName::Rooms), "sheet P-201 is level 2"));
        assert_eq!((job.sheet_regions[0].level.as_str(), job.sheet_regions[0].not_built.as_str()), ("2", ""));
        assert_eq!(job.index.as_ref().and_then(|i| i.pointer("/sheets/0/level_source")).and_then(Value::as_str), Some("answer"));
        // In a one-level building it was given that level, to confirm.
        let one = json!({"number": "P-101", "title": "PLUMBING PLAN", "level": "1", "level_source": "default"});
        let q = plan_level_question(&mut job, &one, &codes[..1]);
        assert_eq!(q.default.as_deref(), Some("Level 1"));
    }

    /// Q6037: level_name canonicalises aliases to one name per real level.
    #[test]
    fn level_name_uses_canonical_names() {
        assert_eq!(level_name("R"), "Roof");
        assert_eq!(level_name("1"), "Level 1");
        assert_eq!(level_name("2"), "Level 2");
        assert_eq!(level_name("B"), "Basement");
        assert_eq!(level_name("B1"), "Basement");
        assert_eq!(level_name("B2"), "Basement 2");
        assert_eq!(level_name("PH"), "Penthouse");
        assert_eq!(level_name("M"), "Mezzanine");
        // Unknown codes pass through.
        assert_eq!(level_name("X"), "X");
    }

    /// Q6037: warnings are strings, never char lists.
    #[test]
    fn job_warnings_are_strings() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        job.sheet_regions = vec![{
            let mut r = region("E-101", "1", "E", "plan", 0.0);
            r.not_built = "no level".to_string();
            r
        }];
        // Add an auto-guard rejected question.
        let q = Question {
            id: "test-q".into(),
            stage: StageName::Classify,
            question: "Is this the right room?".into(),
            why: String::new(),
            choices: vec!["Yes".into(), "No".into()],
            evidence: None,
            default: Some("Yes".into()),
            answer_schema: None,
            answer: Some(Value::String("Rejected".into())),
            answered_by: Some("auto_guard".into()),
            answered_at: None,
            kind: QuestionKind::Doubt,
        };
        if let Some(st) = job.stage_mut(StageName::Classify) {
            st.questions.push(q);
        }
        let warnings = job_warnings(&job, None);
        for w in &warnings {
            if let Some(detail) = w.get("detail") {
                assert!(detail.is_string(), "warning detail must be a string, got: {detail}");
            }
        }
    }

    /// Q6037: auto-rejected questions are listed as open.
    #[test]
    fn auto_rejected_questions_count_as_open() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        let qid = job.next_qid(StageName::Classify);
        let q = Question {
            id: qid.clone(),
            stage: StageName::Classify,
            question: "Is this plausible?".into(),
            why: String::new(),
            choices: Vec::new(),
            evidence: None,
            default: Some("Yes".into()),
            answer_schema: None,
            answer: Some(Value::String("Rejected".into())),
            answered_by: Some("auto_guard".into()),
            answered_at: None,
            kind: QuestionKind::Doubt,
        };
        if let Some(st) = job.stage_mut(StageName::Classify) {
            st.questions.push(q);
        }
        let (_automatic, _by_you, rejected, open) = answer_counts(&job);
        assert_eq!(rejected, 1, "one rejected question");
        assert_eq!(open, 0, "no truly unanswered questions");
        // open_questions in status includes rejected.
        let unanswered: usize = job.stages.iter().map(|s| s.unanswered().len()).sum();
        assert_eq!(unanswered + rejected, 1, "open_questions = unanswered + rejected");
    }

    /// Q6037: building_levels excludes non-building disciplines in the fallback.
    #[test]
    fn building_levels_exclude_life_safety_and_general() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        // No A plans — fallback should NOT pick Life Safety (SL) or General (G) plans.
        job.sheet_regions = vec![
            region("SL-111", "1", "SL", "plan", 0.0),
            region("SL-112", "2", "SL", "plan", 200.0),
            region("E-101", "1", "E", "plan", 400.0),
            region("E-102", "2", "E", "plan", 600.0),
        ];
        let levels = building_levels(&job);
        // Should pick E (Electrical), not SL (Life Safety).
        for lv in &levels {
            let name = lv.get("name").and_then(Value::as_str).unwrap_or("");
            assert!(!name.is_empty(), "level has a name");
        }
        // Verify SL sheets are not used as building level sources.
        let sheets: Vec<_> = levels.iter().filter_map(|l| l.get("sheet").and_then(Value::as_str)).collect();
        assert!(sheets.iter().all(|s| !s.starts_with("SL-")), "no SL sheets in building levels: {sheets:?}");
    }

    #[test]
    fn devices_on_a_sheet_of_no_level_or_the_site_are_not_model_elements() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        job.sheet_regions =
            vec![region("E-101", "1", "E", "plan", 0.0), region("E-201", "", "E", "plan", 200.0), region("E-001", "SITE", "E", "plan", 400.0)];
        let refs = job_sheet_refs(&job);
        assert_eq!(refs.iter().map(|r| r.builds()).collect::<Vec<_>>(), [true, false, false]);
        let mut sp = cadaddy_walk3d::BuildingSpec::default();
        add_sheet_links(&mut sp, &job.sheet_regions);
        assert_eq!(sp.sheets.iter().map(|x| x.level.clone()).collect::<Vec<_>>(), [Some("Level 1".to_string()), None, None]);
    }

    #[test]
    fn the_building_levels_are_the_architectural_floor_plans() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        job.sheet_regions = vec![
            region("E-102", "2", "E", "plan", 0.0),
            region("A112", "2", "A", "plan", 200.0),
            region("A111", "1", "A", "plan", 400.0),
            region("A121", "1", "A", "enlarged_plan", 600.0),
            region("A111B", "1", "A", "plan", 800.0),
            region("A113", "3", "A", "plan", 1000.0),
            region("M-101", "1", "M", "plan", 1200.0),
        ];
        job.index = Some(json!({"levels": [
            {"name": "1", "elevation_ft": 0.0, "height_ft": 12.0},
            {"name": "2", "elevation_ft": 12.0, "height_ft": 10.5},
            {"name": "3", "elevation_ft": 22.5}
        ]}));
        let levels = building_levels(&job);
        let names: Vec<&str> = levels.iter().filter_map(|l| l["name"].as_str()).collect();
        assert_eq!(names, ["Level 1", "Level 2", "Level 3"], "{levels:?}");
        // Each level's region is its A plan's rectangle (the first of the level), never E/M.
        let x0: Vec<f64> = levels.iter().filter_map(|l| l["region"][0].as_f64()).collect();
        assert_eq!(x0, [400.0, 200.0, 1000.0]);
        // Heights from the index; the top level's floor to floor is not read, so it is a guess.
        assert_eq!((levels[0]["elevation_ft"].as_f64(), levels[0]["height_ft"].as_f64()), (Some(0.0), Some(12.0)));
        assert_eq!((levels[1]["elevation_ft"].as_f64(), levels[1]["height_ft"].as_f64()), (Some(12.0), Some(10.5)));
        assert!(levels[0].get("guess").is_none() && levels[1].get("guess").is_none(), "{levels:?}");
        assert_eq!(levels[2]["elevation_ft"].as_f64(), Some(22.5));
        assert!(levels[2]["guess"].as_str().is_some_and(|g| g.contains("A113")), "{levels:?}");

        // No index heights: stacked at the default storey, every one a guess.
        job.index = None;
        let levels = building_levels(&job);
        let el: Vec<f64> = levels.iter().filter_map(|l| l["elevation_ft"].as_f64()).collect();
        assert_eq!(el, [0.0, DEFAULT_STOREY_FT, 2.0 * DEFAULT_STOREY_FT]);
        assert!(levels.iter().all(|l| l["guess"].is_string()));

        // Only E/M plans: no building levels (the build reads what was imported, as before).
        job.sheet_regions = vec![region("E-104", "4", "E", "plan", 0.0)];
        assert!(building_levels(&job).is_empty());
    }

    #[test]
    fn building_levels_keep_their_order_and_come_from_any_floor_plans() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        job.sheet_regions = vec![
            region("A111", "1", "A", "plan", 0.0),
            region("A112", "2", "A", "plan", 200.0),
            region("A113", "3", "A", "plan", 400.0),
            region("A114", "4", "A", "plan", 600.0),
            region("C-101", "SITE", "C", "plan", 800.0),
        ];
        // A site spot elevation read as the first floor (147'-0") is over Levels 2-4.
        job.index = Some(json!({"levels": [
            {"name": "1", "elevation_ft": 147.0},
            {"name": "2", "elevation_ft": 12.0, "height_ft": 11.0},
            {"name": "3", "elevation_ft": 23.0, "height_ft": 11.0},
            {"name": "4", "elevation_ft": 34.0}
        ]}));
        let levels = building_levels(&job);
        let names: Vec<&str> = levels.iter().filter_map(|l| l["name"].as_str()).collect();
        assert_eq!(names, ["Level 1", "Level 2", "Level 3", "Level 4"], "{levels:?}");
        let el: Vec<f64> = levels.iter().filter_map(|l| l["elevation_ft"].as_f64()).collect();
        // The one reading out of order goes; Level 1 sits a storey under Level 2.
        assert_eq!(el, [12.0 - DEFAULT_STOREY_FT, 12.0, 23.0, 34.0], "{levels:?}");
        assert!(levels[0]["guess"].as_str().is_some_and(|g| g.contains("147'-0\", breaks the levels' order")), "{levels:?}");
        assert!(levels[1].get("guess").is_none() && levels[2].get("guess").is_none(), "{levels:?}");

        // Electrical and mechanical plans only: the discipline with the most floors gives the
        // levels, each on its own plan's region (never one level for the whole drawing).
        job.index = None;
        job.sheet_regions = vec![
            region("E-101", "1", "E", "plan", 0.0),
            region("E-102", "2", "E", "plan", 200.0),
            region("E-103", "3", "E", "plan", 400.0),
            region("E-104", "4", "E", "plan", 600.0),
            region("M-101", "1", "M", "plan", 800.0),
            region("M-102", "2", "M", "plan", 1000.0),
            region("E-001", "SITE", "E", "plan", 1200.0),
        ];
        let levels = building_levels(&job);
        let names: Vec<&str> = levels.iter().filter_map(|l| l["name"].as_str()).collect();
        assert_eq!(names, ["Level 1", "Level 2", "Level 3", "Level 4"], "{levels:?}");
        let x0: Vec<f64> = levels.iter().filter_map(|l| l["region"][0].as_f64()).collect();
        assert_eq!(x0, [0.0, 200.0, 400.0, 600.0]);
    }

    #[test]
    fn a_room_read_on_several_sheets_of_a_level_is_one_room() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        job.sheet_regions = vec![region("E-103", "3", "E", "plan", 0.0), region("A113", "3", "A", "plan", 200.0)];
        let mut rooms = json!({"rooms": [
            {"name": "KING", "number": "301", "level": "3", "sheet": "E-103", "area": 1.0},
            {"name": "KING", "number": "301", "level": "3", "sheet": "A113", "area": 2.0},
            {"name": "KING", "number": "302", "level": "3", "sheet": "A113"},
            {"name": "KING", "number": "301", "level": "4", "sheet": "A114"},
            {"name": "CORRIDOR", "level": "3", "sheet": "E-103"},
            {"name": "CORRIDOR", "level": "3", "sheet": "A113"}
        ]});
        assert_eq!(one_room_per_number(&job, &mut rooms), 1);
        let list = rooms["rooms"].as_array().cloned().unwrap_or_default();
        assert_eq!(list.len(), 5, "{list:?}");
        // The architectural sheet's reading is the one kept.
        let r301: Vec<&Value> = list.iter().filter(|r| r["number"] == "301" && r["level"] == "3").collect();
        assert_eq!(r301.len(), 1);
        assert_eq!(r301[0]["sheet"], "A113");
    }

    // ─── Sheet furniture and site sheets build nothing (Q5273) ───

    fn line(s: &mut crate::Session, a: [f64; 2], b: [f64; 2]) {
        s.execute("line", &json!({"points": [a, b]})).unwrap();
    }

    fn rect(s: &mut crate::Session, x0: f64, y0: f64, x1: f64, y1: f64) {
        line(s, [x0, y0], [x1, y0]);
        line(s, [x1, y0], [x1, y1]);
        line(s, [x1, y1], [x0, y1]);
        line(s, [x0, y1], [x0, y0]);
    }

    /// The made-up sheet's rectangle (inches, a 1/8" plan's sheet units).
    const SHEET: [f64; 4] = [-200.0, -200.0, 1000.0, 700.0];

    /// A plan sheet as a print set plots it, everything on the wall pen: a 40' × 30' floor (6"
    /// walls, a 4" partition with a door, OFFICE and LOBBY), a double ruled border, a title-block
    /// strip on the right (two full-height rules, rows ruled in pairs that close boxes around the
    /// firm's name and the project name) and a round review stamp in the strip.
    fn furnished_sheet() -> crate::Session {
        let mut s = crate::Session::new();
        s.execute("layer.new", &json!({"name": "A-WALL", "current": true})).unwrap();
        rect(&mut s, 0.0, 0.0, 480.0, 360.0);
        rect(&mut s, 6.0, 6.0, 474.0, 354.0);
        for x in [238.0, 242.0] {
            line(&mut s, [x, 6.0], [x, 150.0]);
            line(&mut s, [x, 186.0], [x, 354.0]);
        }
        // The border.
        rect(&mut s, -185.0, -185.0, 985.0, 685.0);
        rect(&mut s, -179.0, -179.0, 979.0, 679.0);
        // The title block: its rules, and rows in pairs 8" apart.
        line(&mut s, [880.0, -179.0], [880.0, 679.0]);
        line(&mut s, [886.0, -179.0], [886.0, 679.0]);
        for y in [0.0, 100.0, 200.0, 300.0] {
            line(&mut s, [886.0, y], [979.0, y]);
            line(&mut s, [886.0, y + 8.0], [979.0, y + 8.0]);
        }
        // The stamp.
        s.execute("circle", &json!({"center": [930.0, 450.0], "radius": 30.0})).unwrap();
        s.execute("circle", &json!({"center": [930.0, 450.0], "radius": 24.0})).unwrap();
        s.execute("layer.new", &json!({"name": "A-DOOR", "current": true})).unwrap();
        s.execute("arc", &json!({"center": [240.0, 150.0], "radius": 36.0, "start": 0.0, "end": 90.0})).unwrap();
        s.execute("layer.new", &json!({"name": "A-AREA-IDEN", "current": true})).unwrap();
        for (t, x, y, h) in [
            ("OFFICE", 100.0, 180.0, 8.0),
            ("101", 100.0, 160.0, 6.0),
            ("LOBBY", 340.0, 180.0, 8.0),
            ("102", 340.0, 160.0, 6.0),
            ("ACME DESIGN GROUP", 895.0, 150.0, 6.0),
            ("SAMPLE INN", 895.0, 250.0, 6.0),
            ("OWNER COPY", 905.0, 450.0, 5.0),
        ] {
            s.execute("text", &json!({"at": [x, y], "text": t, "height": h})).unwrap();
        }
        s
    }

    fn a_plan(area: Option<[f64; 4]>, title_block: Option<[f64; 4]>) -> SheetRegion {
        SheetRegion {
            number: "A111".into(),
            level: "1".into(),
            discipline: "A".into(),
            kind: "plan".into(),
            rect: SHEET,
            area,
            title_block,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        }
    }

    /// Built walls: how many, and how many have their middle off the floor's footprint.
    fn walls_built(s: &crate::Session) -> (usize, usize) {
        let d = s.doc().unwrap();
        let walls: Vec<_> = d.model.iter().filter(|e| super::super::wallgen::made_as(e).as_deref() == Some("wall")).collect();
        let off = walls
            .iter()
            .filter(|e| {
                let c = cadaddy_doc::entity_bounds(d, e, 0).center();
                !(c.x >= -12.0 && c.x <= 492.0 && c.y >= -12.0 && c.y <= 372.0)
            })
            .count();
        (walls.len(), off)
    }

    fn build_job(s: &mut crate::Session, job: &GuidedJob) -> Value {
        let levels = building_levels(job);
        assert_eq!(levels.len(), 1, "{levels:?}");
        s.execute("bspec.draft", &json!({"store": true, "levels": levels, "stack": true})).unwrap();
        s.execute("model.build", &json!({})).unwrap()
    }

    #[test]
    fn the_sheet_frame_is_found_and_the_plan_is_inside_it() {
        let s = furnished_sheet();
        let frames = sheet_frames(s.doc().unwrap(), &[SHEET]);
        let f = frames.first().copied().unwrap();
        assert!(f.border, "{f:?}");
        let tb = f.title_block.unwrap();
        assert!((tb[0] - 880.0).abs() < 1.0, "the strip starts at its first full-height rule: {f:?}");
        // Inside the border, left of the title block, and the whole floor in it.
        let a = f.area;
        assert!(a[0] > -179.0 && a[1] > -179.0 && a[2] < 880.0 && a[3] < 679.0, "{f:?}");
        assert!(a[0] < 0.0 && a[1] < 0.0 && a[2] > 480.0 && a[3] > 360.0, "{f:?}");
        // A region with nothing drawn: the frame is the whole region.
        let none = sheet_frames(s.doc().unwrap(), &[[5000.0, 5000.0, 6000.0, 6000.0]]);
        assert_eq!(none.first().map(|f| (f.area, f.border, f.title_block)), Some(([5000.0, 5000.0, 6000.0, 6000.0], false, None)));
    }

    #[test]
    fn the_title_block_border_and_stamp_build_no_walls_or_rooms() {
        // Without the frame (the whole sheet a level), the title block's boxes are traced around
        // its texts, but no room is built from them: they run along no built wall (Q5291).
        let mut s = furnished_sheet();
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        job.sheet_regions = vec![a_plan(None, None)];
        let r = build_job(&mut s, &job);
        let spec: cadaddy_walk3d::BuildingSpec = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
        assert!(
            spec.rooms.iter().any(|r| !["OFFICE", "LOBBY"].contains(&r.name.to_ascii_uppercase().as_str())),
            "the test sheet's title block must name rooms: {r}"
        );
        assert!(r["notes"].to_string().contains("not inside built walls"), "the title block's boxes trace: {r}");
        assert_eq!(r["rooms"].as_u64(), Some(1), "the floor's room only: {r}");

        // With it: only the floor's walls, its door and its two rooms.
        let mut s = furnished_sheet();
        let f = sheet_frames(s.doc().unwrap(), &[SHEET]).first().copied().unwrap();
        job.sheet_regions = vec![a_plan(Some(f.area), f.title_block)];
        let r = build_job(&mut s, &job);
        let (n, off) = walls_built(&s);
        assert!(n >= 4, "{r}");
        assert_eq!(off, 0, "no wall from the border, title block or stamp: {r}");
        // (OFFICE and LOBBY trace as one space through the door's gap: one room.)
        assert_eq!(r["rooms"].as_u64(), Some(1), "the floor's room only: {r}");
        assert_eq!(r["doors"].as_u64(), Some(1), "{r}");
        let spec: cadaddy_walk3d::BuildingSpec = serde_json::from_str(s.doc().unwrap().building_spec.as_deref().unwrap()).unwrap();
        let names: Vec<String> = spec.rooms.iter().map(|r| r.name.to_ascii_uppercase()).collect();
        assert!(!names.is_empty() && names.iter().all(|n| n == "OFFICE" || n == "LOBBY"), "{names:?}");
    }

    #[test]
    fn the_jobs_rooms_are_the_reading_and_what_only_the_model_traced() {
        // The partition's jambs drawn and its door shut, so OFFICE and LOBBY are two rooms
        // inside the floor's walls.
        let mut s = furnished_sheet();
        s.execute("layer.current", &json!({"name": "A-WALL"})).unwrap();
        line(&mut s, [238.0, 150.0], [242.0, 150.0]);
        line(&mut s, [238.0, 186.0], [242.0, 186.0]);
        s.execute("layer.current", &json!({"name": "A-DOOR"})).unwrap();
        line(&mut s, [240.0, 150.0], [240.0, 186.0]);
        let f = sheet_frames(s.doc().unwrap(), &[SHEET]).first().copied().unwrap();
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        job.sheet_regions = vec![a_plan(Some(f.area), f.title_block)];
        let r = build_job(&mut s, &job);
        assert_eq!(r["rooms"].as_u64(), Some(2), "{r}");
        // The rooms stage read OFFICE 101 and LAB 103 (its door not built, so the model traced
        // no lab) on level 1, and STORAGE on level 2 (no plan built); not the lobby.
        if let Some(st) = job.stage_mut(StageName::Rooms) {
            st.result = Some(json!({"count": 3, "rooms": [
                {"name": "OFFICE", "number": "101", "level": "1"},
                {"name": "LAB", "number": "103", "level": "1"},
                {"name": "STORAGE", "number": "201", "level": "2"}
            ]}));
        }
        let by_level = rooms_from_model(&mut s, &mut job).expect("the model's rooms");
        assert_eq!(by_level, json!({"Level 1": 2}));
        let names_of = |job: &GuidedJob| {
            let result = job.stage(StageName::Rooms).and_then(|st| st.result.clone()).unwrap_or_default();
            let mut names: Vec<(String, String, String)> = result["rooms"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    (r["name"].as_str().unwrap_or("").into(), r["number"].as_str().unwrap_or("").into(), r["level"].as_str().unwrap_or("").into())
                })
                .collect();
            names.sort_unstable();
            (names, result)
        };
        let (names, result) = names_of(&job);
        // The model's Office is the OFFICE read (its number); its Lobby, not read, is added.
        let want = [("LAB", "103", "1"), ("Lobby", "102", "Level 1"), ("OFFICE", "101", "1"), ("STORAGE", "201", "2")];
        assert_eq!(names, want.map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string())), "{result}");
        assert_eq!((result["count"].as_u64(), result["read_count"].as_u64(), result["from_model"].as_u64()), (Some(4), Some(3), Some(1)));
        // Run again (a re-run of the build): the same rooms, none twice.
        rooms_from_model(&mut s, &mut job).expect("the model's rooms");
        let (again, result) = names_of(&job);
        assert_eq!(again, names, "{result}");
        assert_eq!(result["read_count"].as_u64(), Some(3));

        // A model with no rooms leaves the reading as it was.
        let mut empty = crate::Session::new();
        let mut job2 = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        if let Some(st) = job2.stage_mut(StageName::Rooms) {
            st.result = Some(json!({"count": 1, "rooms": [{"name": "OFFICE", "level": "1"}]}));
        }
        assert!(rooms_from_model(&mut empty, &mut job2).is_none());
        assert_eq!(
            job2.stage(StageName::Rooms).and_then(|st| st.result.clone()),
            Some(json!({"count": 1, "rooms": [{"name": "OFFICE", "level": "1"}]}))
        );
    }

    #[test]
    fn site_civil_and_levelless_plans_are_never_a_storey() {
        assert_eq!(not_built_reason("C", "DIMENSION CONTROL PLAN", "1", "default"), "site/civil");
        assert_eq!(not_built_reason("L", "PLANTING PLAN", "1", "title"), "site/civil");
        assert_eq!(not_built_reason("A", "ARCHITECTURAL SITE PLAN", "1", "default"), "site/civil");
        assert_eq!(not_built_reason("A", "OVERALL PLAN", "SITE", "title"), "site/civil");
        assert_eq!(not_built_reason("A", "ENLARGED STAIR PLAN", "1", "default"), "no level");
        assert_eq!(not_built_reason("A", "FIRST FLOOR PLAN", "1", "title"), "");

        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        let mut site = region("A010", "1", "A", "plan", 0.0);
        site.not_built = "site/civil".into();
        let mut civil = region("C101", "1", "C", "plan", 200.0);
        civil.not_built = "site/civil".into();
        let mut nolevel = region("A100", "1", "A", "plan", 400.0);
        nolevel.not_built = "no level".into();
        job.sheet_regions = vec![site, civil, nolevel, region("A111", "1", "A", "plan", 600.0), region("A112", "2", "A", "plan", 800.0)];
        let levels = building_levels(&job);
        let x0: Vec<f64> = levels.iter().filter_map(|l| l["region"][0].as_f64()).collect();
        assert_eq!(x0, [600.0, 800.0], "Level 1 is A111's, never the site plan's: {levels:?}");
        let report = not_built_sheets(&job);
        assert_eq!(report, ["A010: not built: site/civil", "C101: not built: site/civil", "A100: not built: no level"]);
    }

    #[test]
    fn rooms_in_title_blocks_on_site_sheets_and_other_disciplines_go() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        let mut a = region("A111", "1", "A", "plan", 0.0);
        a.area = Some([2.0, 2.0, 80.0, 78.0]);
        a.title_block = Some([82.0, 2.0, 98.0, 78.0]);
        let mut site = region("C101", "", "C", "plan", 200.0);
        site.not_built = "site/civil".into();
        job.sheet_regions = vec![a, region("E101", "1", "E", "plan", 400.0), region("E102", "2", "E", "plan", 600.0), site];
        let mut rooms = json!({"rooms": [
            {"name": "OFFICE", "number": "101", "at": [20.0, 20.0]},
            {"name": "ACME DESIGN GROUP", "at": [90.0, 40.0]},
            {"name": "LOBBY", "at": [99.5, 79.5]},
            {"name": "PLAZA", "at": [250.0, 40.0]},
            {"name": "OFFICE", "at": [420.0, 40.0]},
            {"name": "STORAGE", "at": [620.0, 40.0]},
            {"name": "FIRST FLOOR PLAN", "at": [30.0, 10.0]},
            {"name": "DRAWN BY: JD", "at": [30.0, 30.0]},
            {"name": "KING SUITE", "number": "102", "at": [40.0, 40.0]},
            {"name": "KING SUITE", "number": "104", "at": [430.0, 40.0]},
            {"name": "-4--P- -I--Y-", "number": "135", "at": [440.0, 40.0]},
            {"name": "22 40 OO", "at": [50.0, 50.0]}
        ]});
        let dropped = rooms_off_the_building(&job, &mut rooms);
        let names: Vec<&str> = rooms["rooms"].as_array().unwrap().iter().filter_map(|r| r["name"].as_str()).collect();
        // Level 2 has no A plan here: its electrical plan's rooms stay. On level 1 the
        // electrical plan's numbered room the A plan's reading missed stays (one per number is
        // kept later); its unnumbered OFFICE (a legend's, a note's) goes.
        assert_eq!(names, ["OFFICE", "STORAGE", "KING SUITE", "KING SUITE"], "{dropped}");
        assert_eq!(dropped, json!({"title_block_or_border": 2, "site_or_no_level": 1, "other_discipline": 1, "not_a_room_name": 4}));
    }

    #[test]
    fn a_room_name_reads_as_words() {
        for good in ["LOBBY", "IDF", "WC", "CORR", "KING ROOM", "Office", "STAIR 2"] {
            assert!(legible_room_name(good), "{good}");
        }
        for bad in ["-4-----P- -I-----Y-", "Xb", "D D", "22 40 OO", "-J--L---", "G L", "", "12"] {
            assert!(!legible_room_name(bad), "{bad}");
        }
    }

    /// A question for the jobask tests.
    fn ask(job: &mut GuidedJob, stage: StageName, kind: QuestionKind, text: &str, default: Option<&str>) -> String {
        let id = job.next_qid(stage);
        let q = Question {
            id: id.clone(),
            stage,
            question: text.into(),
            why: "test".into(),
            choices: vec!["Correct".into(), "Not a room".into()],
            evidence: None,
            default: default.map(str::to_string),
            answer_schema: None,
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: kind.clone(),
        };
        if let Some(st) = job.stage_mut(stage) {
            match kind {
                QuestionKind::Verify => st.verify.push(q),
                QuestionKind::Completeness => st.completeness.push(q),
                QuestionKind::Doubt => st.questions.push(q),
            }
        }
        id
    }

    /// Interactive: doubts and completeness checks wait for an answer, and the first few verify
    /// samples (a spot check); the rest take their defaults. Auto: every default is taken.
    #[test]
    fn an_interactive_job_asks_and_auto_takes_the_defaults() {
        for auto in [false, true] {
            let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
            job.auto = auto;
            ask(&mut job, StageName::Rooms, QuestionKind::Doubt, "label looks like equipment", Some("Keep"));
            ask(&mut job, StageName::Rooms, QuestionKind::Completeness, "anything not counted?", Some("Count is complete"));
            for i in 0..5 {
                ask(&mut job, StageName::Rooms, QuestionKind::Verify, &format!("Room {i} right?"), Some("Correct"));
            }
            let taken = auto_answer(&mut job, StageName::Rooms);
            let open = job.stage(StageName::Rooms).map_or(0, |st| st.unanswered().len());
            if auto {
                assert_eq!((taken, open), (7, 0));
                assert_eq!(answers_value(&job)["note"], "7 answered automatically, 0 by you (auto mode: every question with a default took it)");
            } else {
                assert_eq!((taken, open), (5 - VERIFY_ASKED, 2 + VERIFY_ASKED));
                assert_eq!(answers_value(&job)["mode"], "interactive");
            }
            // Every question is on the worklist; the ones taken by default are resolved.
            let resolved = job.worklist.iter().filter(|w| w.kind == "question" && w.status == "resolved").count();
            assert_eq!((job.worklist.len(), resolved), (7, taken));
        }
    }

    /// MUST 3: no rooms on architectural plans, or no device symbols on electrical plans, is
    /// asked even in auto mode; with no such plans in the set it takes its default as before.
    #[test]
    fn a_stage_that_found_nothing_on_its_plans_is_always_asked() {
        for with_plans in [true, false] {
            let mut s = crate::Session::new();
            let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Bim);
            job.auto = true;
            job.sheets = Some(json!({"sheets": []}));
            job.sheets_loaded = true;
            job.sheet_regions = if with_plans {
                vec![region("A-101", "1", "A", "plan", 0.0), region("E-101", "1", "E", "plan", 200.0), region("A-501", "", "A", "details", 400.0)]
            } else {
                vec![region("M-101", "1", "M", "plan", 0.0)]
            };
            run_rooms(&mut s, &mut job).expect("rooms");
            run_devices(&mut s, &mut job).expect("devices");
            for stage in [StageName::Rooms, StageName::Devices] {
                auto_answer(&mut job, stage);
                let open: Vec<String> = job.stage(stage).map(|st| st.unanswered().iter().map(|q| q.question.clone()).collect()).unwrap_or_default();
                if with_plans {
                    assert_eq!(open.len(), 1, "{stage:?}: {open:?}");
                    let sheet = if stage == StageName::Rooms { "A-101" } else { "E-101" };
                    assert!(open[0].contains(&format!("({sheet})")), "{open:?}");
                    let q = job.next_question().map(|q| question_value(&job, q)).unwrap_or_default();
                    assert_eq!(q["always_asked"], true, "{q}");
                    assert_eq!(q["sheet"], "A-101", "the sheet to look at: {q}");
                } else {
                    assert!(open.is_empty(), "{stage:?}: {open:?}");
                }
            }
        }
    }

    /// A stage run again asks its questions again: the same ones keep their id and answer; an
    /// answered one it no longer asks is kept; an open one it no longer asks leaves the worklist.
    #[test]
    fn answers_carry_over_when_a_stage_runs_again() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        let same = ask(&mut job, StageName::Rooms, QuestionKind::Verify, "Room 101 right?", Some("Correct"));
        let gone = ask(&mut job, StageName::Rooms, QuestionKind::Verify, "Room 102 right?", Some("Correct"));
        let open = ask(&mut job, StageName::Rooms, QuestionKind::Doubt, "Room 103 looks like equipment", None);
        sync_worklist(&mut job);
        for st in job.stages.iter_mut().filter(|st| st.name == StageName::Rooms) {
            st.status = StageStatus::Paused;
            st.result = Some(json!({"count": 3}));
            for q in st.verify.iter_mut() {
                q.answer = Some(json!("Not a room"));
                q.answered_by = Some("agent".into());
            }
        }
        job.add_finding(StageName::Rooms, "a finding", "test", 2, Vec::new(), None, false);
        let reset = reset_from(&mut job, StageName::Rooms);
        assert_eq!(reset.len(), 1);
        assert_eq!(reset[0].1.get("count"), Some(&json!(3)));
        assert!(job.findings.is_empty(), "the re-run finds its findings again");
        // The re-run asks about room 101 again, and a new question.
        let new_same = ask(&mut job, StageName::Rooms, QuestionKind::Verify, "Room 101 right?", Some("Correct"));
        let fresh = ask(&mut job, StageName::Rooms, QuestionKind::Verify, "Room 104 right?", Some("Correct"));
        sync_worklist(&mut job);
        carry_answers(&mut job, StageName::Rooms);
        sync_worklist(&mut job);
        let st = job.stage(StageName::Rooms).expect("rooms");
        let q = st.verify.iter().find(|q| q.question == "Room 101 right?").expect("asked again");
        assert_eq!((q.id.as_str(), q.answer.clone()), (same.as_str(), Some(json!("Not a room"))));
        assert!(st.verify.iter().any(|q| q.id == gone && q.answer.is_some()), "kept, answered");
        assert!(st.verify.iter().any(|q| q.id == fresh && q.answer.is_none()));
        let ids: Vec<&str> = job.worklist.iter().map(|w| w.id.as_str()).collect();
        assert!(!ids.contains(&open.as_str()) && !ids.contains(&new_same.as_str()), "{ids:?}");
        assert!(ids.contains(&same.as_str()) && ids.contains(&fresh.as_str()), "{ids:?}");
        let changes = digest_changes(StageName::Rooms, &reset[0].1, &digest(&json!({"count": 2})));
        assert_eq!(changes, vec![json!({"stage": "rooms", "what": "count", "before": 3, "after": 2})]);
    }

    #[test]
    fn floor_to_floor_answers_are_read_in_feet() {
        assert_eq!(parse_feet("12'-0\""), Some(12.0));
        assert_eq!(parse_feet("11'-6\" (typical)"), Some(11.5));
        assert_eq!(parse_feet("10'-0\" (typical commercial)"), Some(10.0));
        assert_eq!(parse_feet("13 ft"), Some(13.0));
        assert_eq!(parse_feet("9.5"), Some(9.5));
        for bad in ["", "tall", "0", "-3", "1000", "NaN'"] {
            assert_eq!(parse_feet(bad), None, "{bad}");
        }
        // A floor to floor answer is the building's.
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        job.sheet_regions = vec![region("A-101", "1", "A", "plan", 0.0), region("A-102", "2", "A", "plan", 200.0)];
        job.storey_ft = Some(12.0);
        let levels = building_levels(&job);
        assert_eq!(levels.get(1).and_then(|l| l["elevation_ft"].as_f64()), Some(12.0));
        assert!(levels.iter().all(|l| l.get("guess").is_none()), "{levels:?}");
    }

    /// Answers about rooms and devices become rules the re-run applies.
    #[test]
    fn room_and_device_answers_become_rules() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        let room = |n: &str, name: &str| json!({"number": n, "name": name});
        let mut q = |stage, kind, text: &str, values: Value, answer: Value| {
            let id = ask(&mut job, stage, kind, text, Some("Correct"));
            let mut q = job
                .stages
                .iter()
                .flat_map(|st| st.questions.iter().chain(&st.verify).chain(&st.completeness))
                .find(|q| q.id == id)
                .cloned()
                .expect("q");
            q.evidence = zero_evidence(&[]).or(Some(QuestionEvidence {
                crop_png_path: None,
                sheet: None,
                at: None,
                values: Some(values),
                handles: Vec::new(),
                tiled_crops: None,
                leftovers: Vec::new(),
            }));
            q.answer = Some(answer);
            answer_effect(&mut job, &q)
        };
        let (rerun, _) =
            q(StageName::Rooms, QuestionKind::Verify, "Room 101 'STAIR' right?", json!({"number": "101", "room": "STAIR"}), json!("Not a room"));
        assert_eq!(rerun, Some(StageName::Rooms));
        let (rerun, _) =
            q(StageName::Rooms, QuestionKind::Verify, "Room 102 'KNG' right?", json!({"number": "102", "room": "KNG"}), json!("KING SUITE"));
        assert_eq!(rerun, Some(StageName::Rooms));
        let (rerun, effect) = q(StageName::Rooms, QuestionKind::Verify, "Room 103 right?", json!({"number": "103", "room": "X"}), json!("Correct"));
        assert_eq!((rerun, effect.as_str()), (None, "kept what the job read"));
        let (rerun, _) = q(
            StageName::Devices,
            QuestionKind::Completeness,
            "Unknown symbol tag 'XQ' printed by 4 symbols",
            json!({"description": "XQ"}),
            json!({"device_type": "occupancy sensor"}),
        );
        assert_eq!(rerun, Some(StageName::Devices));
        let (rerun, _) =
            q(StageName::Devices, QuestionKind::Verify, "Device 'L4' detected as L4 — type right?", json!({"tag": "L4"}), json!("Not a device"));
        assert_eq!(rerun, Some(StageName::Devices));
        let (rerun, _) = q(StageName::Levels, QuestionKind::Doubt, "What is the floor-to-floor height for levels 1, 2?", json!({}), json!("12'-0\""));
        assert_eq!((rerun, job.storey_ft), (Some(StageName::BimBuild), Some(12.0)));

        let mut rooms = json!({"rooms": [room("101", "STAIR"), room("102", "KNG"), room("103", "X")]});
        assert_eq!(apply_room_answers(&job, &mut rooms), (1, 1));
        assert_eq!(rooms["rooms"], json!([room("102", "KING SUITE"), room("103", "X")]));
        let keys: Vec<(&str, &str)> = job.rules.iter().map(|r| (r.key.as_str(), r.value.as_str())).collect();
        assert!(keys.contains(&("symbol:XQ", "device:occupancy sensor")) && keys.contains(&("tag:L4", "not_device")), "{keys:?}");
    }

    #[test]
    fn level_answers_take_an_elevation_and_a_name() {
        assert_eq!(parse_elevation("12'-6\""), Some(12.5));
        assert_eq!(parse_elevation("0'-0\""), Some(0.0));
        assert_eq!(parse_elevation("0"), Some(0.0));
        assert_eq!(parse_elevation("-4'-0\""), Some(-4.0));
        assert_eq!(parse_elevation("EL. 112'-0\""), Some(112.0));
        assert_eq!(parse_elevation("112.5 ft"), Some(112.5));
        for bad in ["", "Level 2", "Correct", "high", "-", "99999999"] {
            assert_eq!(parse_elevation(bad), None, "{bad}");
        }
        let at = |e: f64| Some(LevelAnswer { elevation_ft: Some(e), ..LevelAnswer::default() });
        let yes = Some(LevelAnswer { confirmed: true, ..LevelAnswer::default() });
        assert_eq!(level_answer(&json!("Correct")), yes);
        assert_eq!(level_answer(&json!("Yes, \"2\"")), yes);
        assert_eq!(level_answer(&json!("14'-0\"")), at(14.0));
        assert_eq!(level_answer(&json!("Wrong elevation: 14'-0\"")), at(14.0));
        assert_eq!(level_answer(&json!({"elevation_ft": 14.5})), at(14.5));
        assert_eq!(level_answer(&json!({"elevation_ft": "14'-6\""})), at(14.5));
        let named = |n: &str, e: Option<f64>| Some(LevelAnswer { elevation_ft: e, name: Some(n.into()), confirmed: false });
        assert_eq!(level_answer(&json!("Wrong name: Mezzanine")), named("Mezzanine", None));
        assert_eq!(level_answer(&json!("Mezzanine at 14'-0\"")), named("Mezzanine", Some(14.0)));
        assert_eq!(level_answer(&json!({"name": "Mezzanine", "elevation_ft": 14})), named("Mezzanine", Some(14.0)));
        // A choice that says neither, or nonsense: no level answer.
        for none in [json!("Wrong elevation"), json!("Different name"), json!("Not a level"), json!({"x": 1}), json!(3), json!({"name": ""})] {
            assert_eq!(level_answer(&none), None, "{none}");
        }
    }

    fn level_question(job: &mut GuidedJob, level: &str, elevation: Option<f64>) -> Question {
        let id =
            ask(job, StageName::Levels, QuestionKind::Verify, &format!("Level {level}: elevation — name and elevation correct?"), Some("Correct"));
        let st = job.stage_mut(StageName::Levels).expect("levels");
        let q = st.verify.iter_mut().find(|q| q.id == id).expect("q");
        q.evidence = Some(QuestionEvidence {
            crop_png_path: None,
            sheet: None,
            at: None,
            values: Some(json!({"level": level, "elevation_ft": elevation})),
            handles: Vec::new(),
            tiled_crops: None,
            leftovers: Vec::new(),
        });
        q.clone()
    }

    /// A level answer is no longer "kept what the job read": an elevation builds the model at
    /// it (over the readings, in order or not), a name renames, "Correct" makes it the user's.
    #[test]
    fn a_level_answer_moves_the_level_and_correct_confirms_it() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        job.sheet_regions =
            vec![region("A-101", "1", "A", "plan", 0.0), region("A-102", "2", "A", "plan", 200.0), region("A-103", "3", "A", "plan", 400.0)];
        let mut q = level_question(&mut job, "2", Some(10.0));
        q.answer = Some(json!("Wrong elevation: 14'-0\""));
        let (rerun, effect) = answer_effect(&mut job, &q);
        assert_eq!(rerun, Some(StageName::BimBuild), "{effect}");
        assert!(effect.contains("14'"), "{effect}");
        let levels = building_levels(&job);
        let el: Vec<f64> = levels.iter().filter_map(|l| l["elevation_ft"].as_f64()).collect();
        assert_eq!(el, vec![0.0, 14.0, 24.0], "{levels:?}");
        // Level 1 stays at 0 and reaches the elevation given for level 2.
        assert_eq!(levels.first().and_then(|l| l["height_ft"].as_f64()), Some(14.0), "{levels:?}");
        let mut q = level_question(&mut job, "Level 1", None);
        q.answer = Some(json!("-2'-0\""));
        assert_eq!(answer_effect(&mut job, &q).0, Some(StageName::BimBuild));
        let first = building_levels(&job).first().cloned().unwrap_or_default();
        assert_eq!((first["elevation_ft"].as_f64(), first["height_ft"].as_f64()), (Some(-2.0), Some(16.0)));

        let mut q = level_question(&mut job, "Level 3", Some(20.0));
        q.answer = Some(json!("Correct"));
        let (rerun, effect) = answer_effect(&mut job, &q);
        assert_eq!((rerun, effect.as_str()), (None, "level Level 3 confirmed by you (source user)"));
        assert!(job.level_answers.get("LEVEL 3").is_some_and(|a| a.confirmed));

        let mut q = level_question(&mut job, "1", None);
        q.answer = Some(json!({"name": "Ground Floor"}));
        // "1" and "Level 1" are the same level: one answer, the elevation kept.
        let (rerun, effect) = answer_effect(&mut job, &q);
        assert_eq!((rerun, effect.as_str()), (None, "level 1 is named Ground Floor"));
        assert_eq!(
            job.level_answers.get("LEVEL 1"),
            Some(&LevelAnswer { elevation_ft: Some(-2.0), name: Some("Ground Floor".into()), confirmed: true })
        );
        assert_eq!(building_levels(&job).first().and_then(|l| l["elevation_ft"].as_f64()), Some(-2.0));
        // A level answer that says nothing usable says how to answer.
        let mut q = level_question(&mut job, "1", None);
        q.answer = Some(json!("Wrong elevation"));
        let (rerun, effect) = answer_effect(&mut job, &q);
        assert!(rerun.is_none() && effect.contains("answer with an elevation"), "{effect}");
    }

    /// One unknown id among good answers is a warning: the good ones are set and saved.
    #[test]
    fn an_unknown_id_in_a_batch_is_a_warning_and_the_rest_are_kept() {
        let mut s = Session::new();
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Cdy);
        let a = ask(&mut job, StageName::Rooms, QuestionKind::Verify, "Room 101 'OFFICE' right?", Some("Correct"));
        let b = ask(&mut job, StageName::Rooms, QuestionKind::Verify, "Room 102 'LOBBY' right?", Some("Correct"));
        // An earlier answer's re-run dropped this one: it is in the audit, not in a stage.
        job.audit("answer", "agent", Some("Q-rooms-99"), Some(StageName::Rooms), None);
        if let Some(st) = job.stage_mut(StageName::Rooms) {
            st.status = StageStatus::Paused;
        }
        save_job(&mut s, &job).unwrap();
        let r = cmd_answer(
            &mut s,
            &json!({"answers": [{"id": a, "answer": "Correct"}, {"id": "Q-rooms-99", "answer": "Correct"}, {"id": "Q-bogus-1", "answer": "x"}, {"id": b, "answer": "Correct"}]}),
        )
        .unwrap();
        assert_eq!(r["answered"], json!([a, b]), "{r}");
        let w = r["warnings"].as_array().expect("warnings");
        assert_eq!(w.len(), 2, "{r}");
        assert!(w[0]["warning"].as_str().is_some_and(|t| t.contains("no longer open")), "{r}");
        assert!(w[1]["warning"].as_str().is_some_and(|t| t.contains("not found")), "{r}");
        let saved = load_job(&s).unwrap().unwrap();
        let answered: Vec<&str> = saved.stages.iter().flat_map(|st| st.verify.iter()).filter(|q| q.answer.is_some()).map(|q| q.id.as_str()).collect();
        assert_eq!(answered, vec![a.as_str(), b.as_str()]);
        // One id alone that is gone: a warning too, not an error (a script's later lines and its
        // --save still run).
        let r = cmd_answer(&mut s, &json!({"id": "Q-rooms-99", "answer": "Correct"})).unwrap();
        assert!(r["answered"].is_null() && r["warnings"].as_array().is_some_and(|w| w.len() == 1), "{r}");
        // Bad shapes are errors.
        assert!(cmd_answer(&mut s, &json!({"answers": "x"})).is_err());
        assert!(cmd_answer(&mut s, &json!({"answers": []})).is_err());
        assert!(cmd_answer(&mut s, &json!({"id": a})).is_err());
        let many: Vec<Value> = (0..=MAX_ANSWERS_PER_CALL).map(|i| json!({"id": format!("Q-x-{i}"), "answer": "y"})).collect();
        assert!(cmd_answer(&mut s, &json!({"answers": many})).is_err());
    }

    /// Q6031: question payloads always have choices, crops are >= 800 px, and completeness
    /// questions offer "drawn as" alternatives from unlisted tags.
    #[test]
    fn askcrops_questions_have_choices_and_readable_crops() {
        let mut s = crate::Session::new();
        let mut job = GuidedJob::new("/nonexistent/askcrops-test", JobGoal::Takeoff);
        job.sheets = Some(json!({"sheets": [{"number": "E-1", "sheet_type": "plan", "discipline": "E"}]}));
        job.sheets_loaded = true;
        // A legend with types A (lights) and B, then 3 x A, 3 x B, plus 4 x unlisted "W1"
        // and 4 x unlisted "JL".
        let schedule: Vec<(&str, f64, f64, f64)> = vec![
            ("LIGHTING FIXTURE SCHEDULE", 1000.0, 800.0, 8.0),
            ("TYPE", 1000.0, 785.0, 5.0),
            ("DESCRIPTION", 1030.0, 785.0, 5.0),
            ("A", 1000.0, 772.0, 5.0),
            ("2X4 LED TROFFER", 1030.0, 772.0, 5.0),
            ("B", 1000.0, 760.0, 5.0),
            ("LED DOWNLIGHT", 1030.0, 760.0, 5.0),
        ];
        let mut syms: Vec<(&str, f64, f64)> = vec![("A", 990.0, 772.0)]; // legend sample
        // 3 x A and 3 x B on the plan
        for i in 0..3 {
            syms.push(("A", 100.0 + 40.0 * f64::from(i), 300.0));
        }
        for i in 0..3 {
            syms.push(("B", 100.0 + 40.0 * f64::from(i), 200.0));
        }
        // 4 x unlisted "W1" and 4 x "JL"
        for i in 0..4 {
            syms.push(("W1", 100.0 + 40.0 * f64::from(i), 500.0));
        }
        for i in 0..4 {
            syms.push(("JL", 100.0 + 40.0 * f64::from(i), 600.0));
        }
        let lo = s.doc().map_or(0, |d| d.handseed);
        add_sheet(&mut s, &schedule, &syms);
        let hi = s.doc().map_or(0, |d| d.handseed);
        job.sheet_spans.push(SheetSpan { sheet: "E-1".into(), lo, hi });
        job.sheet_regions.push(SheetRegion {
            number: "E-1".into(),
            level: "1".into(),
            discipline: "E".into(),
            kind: "plan".into(),
            rect: [0.0, 0.0, 2000.0, 1000.0],
            area: None,
            title_block: None,
            not_built: String::new(),
            levels: Vec::new(),
            view: None,
        });
        let _r = run_devices(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        let completeness = job.stage(StageName::Devices).map(|st| st.completeness.clone()).unwrap_or_default();

        // ── Unknown-tag questions: every one must have choices ──
        let unkn_qs: Vec<&Question> = completeness.iter().filter(|q| q.question.contains("Unknown")).collect();
        assert!(unkn_qs.len() >= 2, "expected questions for W1 and JL: {}", unkn_qs.len());
        for q in &unkn_qs {
            assert!(!q.choices.is_empty(), "unknown-tag question must have choices: {}", q.question);
            assert!(q.choices.iter().any(|c| c == "skip"), "missing 'skip' in: {:?}", q.choices);
            assert!(q.choices.iter().any(|c| c.contains("not a device")), "missing 'not a device' in: {:?}", q.choices);
            // Should include legend types as choices.
            assert!(q.choices.iter().any(|c| c == "A" || c == "B"), "missing legend types in choices: {:?}", q.choices);
            // Crop evidence: sheet info present.
            let ev = q.evidence.as_ref().expect("evidence");
            assert_eq!(ev.sheet.as_deref(), Some("E-1"), "evidence should name the sheet");
        }

        // ── Completeness questions: must have choices ──
        let comp_qs: Vec<&Question> = completeness.iter().filter(|q| q.question.starts_with("I counted")).collect();
        assert!(!comp_qs.is_empty(), "no completeness questions");
        for q in &comp_qs {
            assert!(q.choices.len() >= 2, "completeness question needs choices: {} -> {:?}", q.question, q.choices);
            // Should offer "drawn as" alternatives for the unlisted tags.
            let has_drawn_as = q.choices.iter().any(|c| c.starts_with("They are drawn as"));
            assert!(has_drawn_as, "completeness question should offer 'drawn as' alternatives from unlisted tags: {:?}", q.choices);
        }

        // ── Crop size check for unknown tags ──
        for q in &unkn_qs {
            if let Some(ev) = &q.evidence
                && let Some(path) = &ev.crop_png_path
            {
                let png = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
                assert!(png.starts_with(b"\x89PNG"), "not PNG: {path}");
                assert!(png.len() >= 24, "PNG too short");
                let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
                let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
                assert!(w >= 800 && h >= 800, "crop {path} too small: {w}x{h}");
            }
        }

        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("cadaddy-{}", job.id)));
    }

    #[test]
    fn a_question_says_what_an_answer_runs_again() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        let id = ask(&mut job, StageName::Rooms, QuestionKind::Verify, "Room 101 'OFFICE' right?", Some("Correct"));
        let q = job.stage(StageName::Rooms).and_then(|st| st.verify.first()).cloned().expect("q");
        assert_eq!(q.id, id);
        // The rooms stage hasn't run: the answer is read when it does.
        assert_eq!(if_changed(&job, &q)["applies_to"], "rooms");
        for (name, ms) in [(StageName::Rooms, 98_400), (StageName::Devices, 3_100)] {
            if let Some(st) = job.stage_mut(name) {
                st.status = StageStatus::Done;
                st.elapsed_ms = Some(ms);
            }
        }
        let v = if_changed(&job, &q);
        assert_eq!((v["reruns"].clone(), v["about_s"].as_f64()), (json!(["rooms", "devices"]), Some(102.0)), "{v}");
        assert!(v["note"].as_str().is_some_and(|n| n.contains("about 102 s")), "{v}");
        // A question nothing re-reads.
        let other = ask(&mut job, StageName::Takeoff, QuestionKind::Doubt, "Totals look right?", Some("Yes"));
        let q = job.stage(StageName::Takeoff).and_then(|st| st.questions.iter().find(|q| q.id == other)).cloned().expect("q");
        assert!(if_changed(&job, &q).is_null());
    }

    // ─── Q6021: auto-guard tests ─────────────────────────────────────────────

    /// Helper: a room-verify question with area and name in the evidence.
    fn room_verify(job: &mut GuidedJob, name: &str, number: &str, area: f64) -> String {
        let id = job.next_qid(StageName::Rooms);
        let q = Question {
            id: id.clone(),
            stage: StageName::Rooms,
            question: format!("Room {number} '{name}': {area:.0} sq ft — name and area right?"),
            why: "Verify sample".into(),
            choices: vec!["Correct".into(), "Not a room".into()],
            evidence: Some(QuestionEvidence {
                crop_png_path: None,
                sheet: None,
                at: None,
                values: Some(json!({"room": name, "number": number, "area_sqft": area})),
                handles: Vec::new(),
                tiled_crops: None,
                leftovers: Vec::new(),
            }),
            default: Some("Correct".into()),
            answer_schema: None,
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: QuestionKind::Verify,
        };
        if let Some(st) = job.stage_mut(StageName::Rooms) {
            st.verify.push(q);
        }
        id
    }

    /// Helper: an unknown-symbol device question (default "skip").
    fn device_unknown(job: &mut GuidedJob, tag: &str, count: u64) -> String {
        let id = job.next_qid(StageName::Devices);
        let q = Question {
            id: id.clone(),
            stage: StageName::Devices,
            question: format!("Unknown symbol tag '{tag}' printed by {count} symbols — which device is it?"),
            why: "unknown symbol".into(),
            choices: Vec::new(),
            evidence: Some(QuestionEvidence {
                crop_png_path: None,
                sheet: Some("E-1".into()),
                at: None,
                values: Some(json!({"description": tag, "count": count})),
                handles: Vec::new(),
                tiled_crops: None,
                leftovers: Vec::new(),
            }),
            default: Some("skip".into()),
            answer_schema: None,
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: QuestionKind::Completeness,
        };
        if let Some(st) = job.stage_mut(StageName::Devices) {
            st.completeness.push(q);
        }
        id
    }

    #[test]
    fn is_room_like_name_rejects_junk() {
        // Good room names.
        assert!(is_room_like_name("101"));
        assert!(is_room_like_name("LOBBY"));
        assert!(is_room_like_name("GUEST ROOM"));
        assert!(is_room_like_name("ELECTRICAL ROOM"));
        assert!(is_room_like_name("101A"));
        assert!(is_room_like_name("BATHROOM 3"));
        // Junk names from the real prints.
        assert!(!is_room_like_name("WALL"));
        assert!(!is_room_like_name("TPO ROOFING"));
        assert!(!is_room_like_name("HEIGHT"));
        assert!(!is_room_like_name("APPLICABLE."));
        assert!(!is_room_like_name("CEILING"));
        assert!(!is_room_like_name("FLOOR"));
        assert!(!is_room_like_name("SLAB"));
        assert!(!is_room_like_name("TYP"));
        assert!(!is_room_like_name(""));
        assert!(!is_room_like_name("CMU"));
    }

    /// Q6021 MUST 1: auto mode rejects rooms at 0 sf or with junk names.
    #[test]
    fn auto_guard_rejects_implausible_rooms() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        job.auto = true;
        // Good room: 350 sf, real name.
        room_verify(&mut job, "GUEST ROOM", "101", 350.0);
        // Bad room: 0 sf.
        room_verify(&mut job, "GUEST ROOM", "102", 0.0);
        // Bad room: junk name.
        room_verify(&mut job, "WALL", "103", 50.0);
        // Bad room: junk name AND 0 sf.
        room_verify(&mut job, "TPO ROOFING", "104", 0.0);

        let taken = auto_answer(&mut job, StageName::Rooms);
        assert_eq!(taken, 4, "all 4 should be answered (1 accepted + 3 rejected)");

        let st = job.stage(StageName::Rooms).expect("rooms stage");
        let accepted: Vec<_> = st.verify.iter().filter(|q| q.answered_by.as_deref() == Some("default")).collect();
        let rejected: Vec<_> = st.verify.iter().filter(|q| q.answered_by.as_deref() == Some("auto_guard")).collect();
        assert_eq!(accepted.len(), 1, "only the good room accepted");
        assert_eq!(rejected.len(), 3, "3 junk rooms rejected");
        assert!(accepted[0].question.contains("101"), "room 101 accepted");
        for q in &rejected {
            assert_eq!(q.answer.as_ref().and_then(Value::as_str), Some("Rejected"));
        }

        let (auto_count, _, rej_count, _) = answer_counts(&job);
        assert_eq!((auto_count, rej_count), (1, 3));
    }

    /// Q6021 MUST 1: auto mode rejects unknown device "skip" defaults.
    #[test]
    fn auto_guard_rejects_unknown_device_skip() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        job.auto = true;
        // A regular completeness question (not "skip") is accepted.
        ask(&mut job, StageName::Devices, QuestionKind::Completeness, "I counted 5 duplex receptacles. Any uncounted?", Some("Count is complete"));
        // Unknown symbol questions default "skip" are rejected.
        device_unknown(&mut job, "XY", 10);
        device_unknown(&mut job, "QQ", 4);

        let taken = auto_answer(&mut job, StageName::Devices);
        assert_eq!(taken, 3);

        let st = job.stage(StageName::Devices).expect("devices stage");
        let all_comp: Vec<_> = st.completeness.iter().collect();
        // First: "Count is complete" → accepted.
        assert_eq!(all_comp[0].answered_by.as_deref(), Some("default"));
        // XY and QQ: rejected.
        assert_eq!(all_comp[1].answered_by.as_deref(), Some("auto_guard"));
        assert_eq!(all_comp[2].answered_by.as_deref(), Some("auto_guard"));
    }

    /// Q6021 MUST 3: job warnings and done_with_warnings status.
    #[test]
    fn job_report_warns_on_zero_rooms_and_empty_devices() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        // Rooms stage done with 0 rooms.
        if let Some(st) = job.stage_mut(StageName::Rooms) {
            st.status = StageStatus::Done;
            st.result = Some(json!({"count": 0, "rooms": []}));
        }
        // Devices stage done with 0 devices.
        if let Some(st) = job.stage_mut(StageName::Devices) {
            st.status = StageStatus::Done;
            st.result = Some(json!({"count": 0}));
        }
        // Mark other stages skipped so is_done() returns true.
        for st in &mut job.stages {
            if st.status == StageStatus::Pending {
                st.status = StageStatus::Skipped;
            }
        }
        assert!(job.is_done());

        let warnings = job_warnings(&job, None);
        assert!(warnings.iter().any(|w| w["warning"] == "0 rooms"), "expected 0-rooms warning: {warnings:?}");
        assert!(warnings.iter().any(|w| w["warning"] == "devices not in model"), "expected devices warning: {warnings:?}");
        assert_eq!(done_status(&job, &warnings), "done_with_warnings");

        // Without warnings: plain "done".
        let empty_warnings: Vec<Value> = Vec::new();
        assert_eq!(done_status(&job, &empty_warnings), "done");
    }

    /// Q6021 MUST 3: auto-guard rejections appear in the warnings.
    #[test]
    fn auto_guard_rejections_show_in_warnings() {
        let mut job = GuidedJob::new("/tmp/prints", JobGoal::Bim);
        job.auto = true;
        room_verify(&mut job, "WALL", "999", 0.0);
        auto_answer(&mut job, StageName::Rooms);
        // Mark everything done/skipped.
        for st in &mut job.stages {
            if st.status == StageStatus::Pending || st.status == StageStatus::Paused {
                st.status = StageStatus::Skipped;
            }
        }
        if let Some(st) = job.stage_mut(StageName::Rooms) {
            st.status = StageStatus::Done;
            st.result = Some(json!({"count": 1, "rooms": [{"name": "WALL"}]}));
        }

        let warnings = job_warnings(&job, None);
        assert!(warnings.iter().any(|w| w["warning"] == "auto-guard rejections"), "expected auto-guard warning: {warnings:?}");
    }

    /// B7-005: the levels question's answer makes the levels; an answer that doesn't fit its
    /// question (a number for text, an empty one, an unknown id) is refused and the job is as
    /// it was.
    #[test]
    fn a_levels_answer_makes_the_levels_and_answers_must_fit_their_question() {
        let mut s = Session::new();
        let mut job = GuidedJob::new("/nonexistent/prints", JobGoal::Bim);
        let q = Question {
            id: job.next_qid(StageName::Levels),
            stage: StageName::Levels,
            question: "No levels detected. What are the building levels? (e.g. \"1, 2, 3, R\")".into(),
            why: String::new(),
            choices: Vec::new(),
            evidence: None,
            default: Some("1".into()),
            answer_schema: Some(json!({"type": "string"})),
            answer: None,
            answered_by: None,
            answered_at: None,
            kind: QuestionKind::Doubt,
        };
        let qid = q.id.clone();
        if let Some(st) = job.stage_mut(StageName::Levels) {
            st.status = StageStatus::Paused;
            st.questions.push(q);
        }
        save_job(&mut s, &job).unwrap();
        for bad_answer in [json!(12345), json!({}), json!(""), json!("   "), json!("<>!")] {
            let e = s.execute("job.answer", &json!({"id": qid, "answer": bad_answer})).expect_err("refused");
            assert!(e.to_string().contains(&qid), "{bad_answer}: {e}");
        }
        let e = s.execute("job.answer", &json!({"id": "Q-levels-999", "answer": "1, 2"})).expect_err("unknown id");
        assert!(e.to_string().contains("not found"), "{e}");
        let job = load_job(&s).unwrap().unwrap();
        assert!(job.stage(StageName::Levels).is_some_and(|st| st.questions.iter().all(|q| q.answer.is_none())), "nothing taken");

        assert_eq!(parse_level_list("1, 2, 3, R"), Some(vec!["Level 1".into(), "Level 2".into(), "Level 3".into(), "Roof".into()]));
        assert_eq!(parse_level_list("Basement; Level 1;Level 1"), Some(vec!["Basement".into(), "Level 1".into()]));
        assert_eq!(parse_level_list(" , "), None);
        let mut job = load_job(&s).unwrap().unwrap();
        let a = apply_answer(&mut job, &qid, json!("1, 2, R")).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(a.rerun_from, Some(StageName::Levels), "{}", a.effect);
        assert_eq!(job.levels_given, vec!["Level 1", "Level 2", "Roof"]);
        job.storey_ft = Some(12.0);
        let made = levels_from_answer(&mut s, &job).unwrap();
        assert_eq!(made.len(), 3);
        let (sp, _) = super::super::bspec::current(&s, "test").unwrap();
        let levels: Vec<(String, f64)> = sp.levels.iter().map(|l| (l.name.clone(), l.elevation_ft)).collect();
        assert_eq!(levels, vec![("Level 1".into(), 0.0), ("Level 2".into(), 12.0), ("Roof".into(), 24.0)]);
    }
}
