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
}

impl JobGoal {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "cdy" => Some(Self::Cdy),
            "bim" => Some(Self::Bim),
            "takeoff" => Some(Self::Takeoff),
            "estimate" => Some(Self::Estimate),
            "clash" => Some(Self::Clash),
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
            Self::Clash => StageName::Clash,
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
    /// Answered questions of stages being run again, to carry their answers over.
    #[serde(skip)]
    carry: Vec<Question>,
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
}

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
}

/// Why a sheet is never built into a building level: a civil, landscape or site sheet, or a
/// plan whose title and number name no level (it was only given the lowest, to confirm).
pub fn not_built_reason(discipline: &str, title: &str, level: &str, level_source: &str) -> &'static str {
    if level.eq_ignore_ascii_case("SITE") || cadaddy_mepfix::sheet_facts::is_site_sheet(discipline, title) {
        "site/civil"
    } else if level.trim().is_empty() || level_source == "default" {
        "no level"
    } else {
        ""
    }
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
    pub fn level_at(&self, at: [f64; 2]) -> Option<&SheetRegion> {
        self.sheet_regions.iter().find(|r| at[0] >= r.rect[0] && at[0] <= r.rect[2] && at[1] >= r.rect[1] && at[1] <= r.rect[3])
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
            level_answers: BTreeMap::new(),
            rerun_held: None,
            spec_at_start: BTreeMap::new(),
            carry: Vec::new(),
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
    if sheet_count == 0 {
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
        // Every plan gets a level: one whose title names none was given the lowest; confirm it.
        for sheet in sheets.iter().filter(|sh| sh.get("level_source").and_then(Value::as_str) == Some("default")).take(MAX_QUESTIONS_PER_STAGE) {
            let number = sheet.get("number").and_then(Value::as_str).unwrap_or("?");
            let title = sheet.get("title").and_then(Value::as_str).unwrap_or("");
            let level = sheet.get("level").and_then(Value::as_str).unwrap_or("1");
            let q = Question {
                id: job.next_qid(StageName::Classify),
                stage: StageName::Classify,
                question: format!("Plan sheet {number} \"{title}\" names no level — is it level {level}?"),
                why: "Neither the sheet title, its view titles nor its number say which level the plan shows".into(),
                choices: vec![format!("Level {level}"), "Another level".into(), "Not a level plan".into()],
                evidence: Some(QuestionEvidence {
                    crop_png_path: None,
                    sheet: Some(number.to_string()),
                    at: None,
                    values: Some(json!({"number": number, "title": title, "level": level, "sources": sheet.get("sources")})),
                    handles: Vec::new(),
                    tiled_crops: None,
                    leftovers: Vec::new(),
                }),
                default: Some(format!("Level {level}")),
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

    // MUST 3 (typicalfloors): a level with an architectural plan but no plan for a trade
    // gets a finding.
    if let Some(ref sheets) = sheets_vec {
        check_missing_trade_plans(sheets, job);
    }

    job.audit("stage_run", "pipeline", None, Some(StageName::Classify), Some(json!({"sheets": sheet_count})));
    Ok(classified)
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
            let sheets: Vec<&str> =
                l.get("sheets").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            title_levels.push(json!({
                "name": name,
                "sheet": sheets.first().copied().unwrap_or("?"),
                "sheets": sheets,
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
    let effective_levels = match levels.filter(|l| !l.is_empty()) {
        Some(lvls) if title_levels.is_empty() => lvls.clone(),
        _ => title_levels,
    };
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
    let mut plan_sheets: Vec<&Value> = sheet_arr.iter().filter(current).filter(|sh| matches!(kind(sh).as_str(), "plan" | "enlarged_plan")).collect();
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
            let path = if std::path::Path::new(file).is_absolute() { file.to_string() } else { format!("{}/{}", job.pdfs_path, file) };
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
    let results = super::exchange::import_pdf_pages(s, &pages, true, true, &mut on_page)?;
    let mut imported = 0usize;
    let mut same = 0usize;
    let mut placed = Vec::new();
    let mut regions = Vec::new();
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
                    regions.push(SheetRegion {
                        number: base.to_string(),
                        level,
                        discipline: discipline.to_ascii_uppercase(),
                        kind: kind(sh),
                        rect,
                        area: None,
                        title_block: None,
                        not_built: not_built.to_string(),
                        levels,
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
    if save_job(s, job).is_err() {
        return;
    }
    let Ok(d) = s.doc() else { return };
    let written = (hooks.write)(d, &path).and_then(|bytes| {
        let p = std::path::Path::new(&path);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        // Written aside, then moved over the last one: a crash mid-write keeps the last checkpoint.
        let part = p.with_extension("cdy.part");
        std::fs::write(&part, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&part, p).map_err(|e| e.to_string())
    });
    if let Err(e) = written {
        s.echo(format!("guided: the job's checkpoint couldn't be written to {path}: {e}"));
    }
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

/// An unlisted tag printed by fewer symbols than this is stray lettering, not a question.
const MIN_REPEATED_TAG: usize = 3;
/// Most crops drawn for unlisted tags.
const MAX_TAG_CROPS: usize = 12;
/// Most handles kept per unlisted tag.
const MAX_TAG_HANDLES: usize = 20;

/// Sort the drawing's device symbols sheet by sheet: each sheet's legends and schedules say
/// which symbols are samples; all of them together say which tags are device types.
fn device_census(s: &Session, job: &GuidedJob) -> DeviceCensus {
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
    let mut symbols: Vec<Vec<(String, cadaddy_geom::Vec2, cadaddy_doc::Handle)>> = vec![Vec::new(); groups];
    for e in store.iter() {
        let g = group_of(e.handle.0);
        match &e.kind {
            cadaddy_doc::EntityKind::Insert(ins) if ins.block.starts_with("PDF-SYM-") => {
                let tag = ins.attribs.iter().find(|a| a.tag == "TAG").map(|a| a.text.value.trim().to_string()).unwrap_or_default();
                if let Some(v) = symbols.get_mut(g) {
                    v.push((tag, ins.insert.xy(), e.handle));
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
    let learned: std::collections::BTreeSet<String> = job
        .rules
        .iter()
        .filter(|r| r.value.starts_with("device:"))
        .filter_map(|r| r.key.strip_prefix("symbol:"))
        .map(|t| t.trim().to_string())
        .collect();
    // Tags an answer said are not devices.
    let refused: BTreeSet<&str> = job.rules.iter().filter(|r| r.value == "not_device").filter_map(|r| r.key.strip_prefix("tag:")).collect();
    let none = Legend::default();
    for (g, syms) in symbols.iter().enumerate() {
        let legend = legends.get(g).unwrap_or(&none);
        let circuits: Vec<&TextItem> = texts.get(g).into_iter().flatten().filter(|t| circuit_text(&t.value, &sheet_numbers)).collect();
        let sheet = spans.get(g).map_or_else(String::new, |sp| sp.sheet.clone());
        let span = spans.get(g).map_or((0, u64::MAX), |sp| (sp.lo, sp.hi));
        for (tag, at, h) in syms {
            if legend.covers(*at) {
                c.legend_samples += 1;
            } else if tag.is_empty() {
                c.untagged += 1;
            } else if is_circuit_tag(tag) {
                c.circuit += 1;
            } else if refused.contains(tag.as_str()) {
                *c.not_device.entry("not a device (answered)").or_default() += 1;
            } else {
                match device_types::tag_verdict(tag, &c.legend_types, &learned, &sheet_numbers) {
                    TagVerdict::Device(t) => {
                        c.placed.push(super::readers::ReadDevice {
                            tag: t.clone(),
                            at: [at.x, at.y],
                            sheet: sheet.clone(),
                            level: job.level_at([at.x, at.y]).map_or_else(String::new, |r| r.level.clone()),
                            circuit: super::readers::circuit_beside(&circuits, *at),
                            handle: h.hex(),
                            kind: String::new(),
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

/// A 256-pixel PNG of the drawing around `at` (the first symbol printing an unlisted tag),
/// written into `dir`; its path.
fn tag_crop(s: &Session, at: [f64; 2], span: (u64, u64), handle: Option<&str>, dir: &std::path::Path, name: &str) -> Option<String> {
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
    let half = if r.is_finite() && r > 0.0 { 4.0 * r } else { 24.0 };
    let window = cadaddy_geom::Bounds2::new(centre, centre).expand(half);
    // The sheets overlap in model space: draw only this sheet's objects.
    let near: Vec<&cadaddy_doc::Entity> = store
        .iter()
        .filter(|e| (span.0..span.1).contains(&e.handle.0) && cadaddy_doc::entity_bounds(d, e, 0).intersects(&window))
        .map(|e| e.as_ref())
        .take(20_000)
        .collect();
    let list = cadaddy_render::build_entities(d, near, &cadaddy_render::Options::default());
    let view = cadaddy_render::raster::View::fit(&window, 256, 256, 0.0);
    let ro = cadaddy_render::raster::RasterOptions { background: cadaddy_color::Rgb(255, 255, 255), hairline: 1.0, px_per_mm: 0.0, antialias: true };
    let png = cadaddy_render::raster::render_png(&list, &view, &ro)?;
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).take(24).collect();
    let path = dir.join(format!("tag-{safe}.png"));
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
fn run_rooms(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    // Load plan-sheet PDFs into the drawing so rooms.list finds entities.
    ensure_sheets_loaded(s, job)?;
    // Each floor plan's labels read in full, sheet by sheet (a whole set's labels are more than
    // one reading traces: the last floors were left out).
    let regions: Vec<[f64; 4]> =
        job.sheet_regions.iter().filter(|r| r.kind == "plan" && r.not_built.is_empty()).map(|r| r.area.unwrap_or(r.rect)).collect();
    let mut params = json!({"outline": false});
    if !regions.is_empty() {
        params["regions"] = json!(regions);
    }
    let mut rooms_result = s.execute("rooms.list", &params)?;
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
    let duplicates = one_room_per_number(job, &mut rooms_result);
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
    let before = list.len();
    list.retain(|r| rules.get(key(r).as_str()) != Some(&"remove"));
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
    let census = device_census(s, job);
    results["circuit_tagged_symbols"] = json!(census.circuit);
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
                let crop = if i < MAX_TAG_CROPS { tag_crop(s, u.at, u.span, u.handles.first().map(String::as_str), &crop_dir, tag) } else { None };
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
        for (class_key, class_label) in &device_classes {
            let count = devices.get("by_type").and_then(|bt| bt.get(*class_key)).and_then(Value::as_u64).unwrap_or(0);
            // MUST 9: Always ask — even for 0 ("I counted 0 X — anything not counted?").
            let qid = job.next_qid(StageName::Devices);
            let q = Question {
                id: qid,
                stage: StageName::Devices,
                question: format!("I counted {count} {class_label}. Are there any {class_label} I did not count?",),
                why: format!("Completeness checkpoint for {class_label}"),
                choices: vec!["Count is complete".into(), "There are uncounted devices".into()],
                evidence: Some(QuestionEvidence {
                    crop_png_path: None,
                    sheet: None,
                    at: None,
                    values: Some(json!({"class": class_key, "count": count})),
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

        // MUST 10: Unknown/foreign symbols — ask to learn them.
        if let Some(unknowns) = devices.get("unknowns").and_then(Value::as_array) {
            for unk in unknowns.iter().take(MAX_QUESTIONS_PER_STAGE) {
                let desc = unk.get("description").and_then(Value::as_str).unwrap_or("unknown symbol");
                let count = unk.get("count").and_then(Value::as_u64).unwrap_or(1);
                let tagged = unk.get("tag").and_then(Value::as_str).is_some();
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
                    choices: Vec::new(),
                    evidence: Some(QuestionEvidence {
                        crop_png_path: unk.get("crop_path").and_then(Value::as_str).map(String::from),
                        sheet: unk.get("sheet").and_then(Value::as_str).map(String::from),
                        at: unk.get("at").and_then(|a| {
                            let arr = a.as_array()?;
                            Some([arr.first()?.as_f64()?, arr.get(1)?.as_f64()?])
                        }),
                        values: Some(json!({"description": desc, "count": count})),
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
            let span = job
                .sheet_spans
                .iter()
                .enumerate()
                .find(|(i, sp)| !used.get(*i).copied().unwrap_or(true) && base(&sp.sheet).eq_ignore_ascii_case(&r.number));
            let span = span.map(|(i, sp)| {
                if let Some(u) = used.get_mut(i) {
                    *u = true;
                }
                (sp.lo, sp.hi)
            });
            super::printmep::SheetRef {
                number: r.number.clone(),
                level: r.level.clone(),
                discipline,
                kind: kind.filter(|k| !k.is_empty()).unwrap_or_else(|| "plan".into()),
                rect: r.rect,
                span,
                scale_text: entry.and_then(|c| c.get("scale")).and_then(Value::as_str).map(str::to_string),
                levels: r.levels.clone(),
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
fn run_bim_build(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    // Ensure sheet content is in the drawing so bspec.draft can read geometry.
    ensure_sheets_loaded(s, job)?;
    // MUST 1: Draft the spec first — bim.build needs it. Without the spec, it errors.
    // With architectural floor plans, the building is theirs: one level per plan (its sheet's
    // rectangle), stacked; the other disciplines' plans add no walls.
    let levels = building_levels(job);
    let draft_params = if levels.is_empty() { json!({"store": true}) } else { json!({"store": true, "levels": levels, "stack": true}) };
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
                // The circuits' estimated routes just above each level's ceiling.
                super::printcond::seat_heights(&mut sp);
                s.doc_mut()?.building_spec = Some(sp.to_json());
                job.audit("mep_merged", "pipeline", None, Some(StageName::BimBuild), Some(json!({"runs": runs})));
            }
            // Where each sheet of the set sits, so what lies on the other disciplines' sheets
            // is theirs (not "outside every level's region").
            if !job.sheet_regions.is_empty()
                && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
            {
                add_sheet_links(&mut sp, &job.sheet_regions);
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

    let mut placed: Option<Value> = None;
    // The devices stage's devices, on their levels (bspec.draft started a new spec), before the
    // build: it makes each one an element carrying what was read about it (Q6002).
    if !job.devices.is_empty() {
        match super::readers::place_devices(s, &job.devices, "guided job") {
            Ok(v) => placed = Some(v),
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
        // The architect locates, the engineer circuits (Q6026): each device moves to where the
        // architectural elevations and plans put it, keeping its circuit.
        let located = super::archlocate::run_for_job(s, job);
        for (what, examples) in super::archlocate::job_findings(&located) {
            job.add_finding(
                StageName::BimBuild,
                &what,
                &examples,
                2,
                vec!["Check the device on the architectural elevation and the E plan; edit its location to settle it".into()],
                None,
                false,
            );
        }
        result["arch_locate"] = located;
    }

    // Use model.build directly — it returns walls/doors/rooms/levels counts.
    // (bim.build wraps model.build but constructs its own result without those counts.)
    let mut result = s.execute("model.build", &json!({}))?;
    if let Some(v) = placed {
        result["devices"] = v;
    }
    // The branch circuits (their runs went in with the MEP parts): one tag each, for the takeoff.
    if !job.circuits.is_empty()
        && let Ok((mut sp, true)) = super::bspec::current(s, "bim_build")
    {
        super::printcond::store_circuits(&mut sp, &job.circuits);
        s.doc_mut()?.building_spec = Some(sp.to_json());
        result["circuits"] = json!(job.circuits.len());
    }
    // The model's rooms are the job's rooms on the levels it built.
    if let Some(by_level) = rooms_from_model(s, job)
        && let Some(o) = result.as_object_mut()
    {
        o.insert("rooms_by_level".into(), by_level);
    }
    // Q6002: the rooms as elements, then what the readers found on every element.
    match s.execute("rooms.build", &json!({})) {
        Ok(v) => result["room_volumes"] = v.get("built").cloned().unwrap_or(Value::Null),
        Err(e) => result["room_volumes_error"] = json!(e.to_string()),
    }
    match s.execute("bim.props.fill", &json!({})) {
        Ok(v) => result["props"] = v,
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
fn level_name(code: &str) -> String {
    match code {
        "R" => "Roof".to_string(),
        l if !l.is_empty() && l.chars().all(|c| c.is_ascii_digit()) => format!("Level {l}"),
        l => l.to_string(),
    }
}

/// Most sheets the job lists in the spec.
const MAX_SHEET_LINKS: usize = 2_000;

/// The job's imported sheets into the spec's sheet list, each with where it sits in model space
/// and its level (a sheet listed already keeps its entry and gains its place).
pub(crate) fn add_sheet_links(sp: &mut cadaddy_walk3d::BuildingSpec, regions: &[SheetRegion]) {
    for r in regions.iter().take(MAX_SHEET_LINKS) {
        if !r.rect.iter().all(|v| v.is_finite()) {
            continue;
        }
        let level = Some(r.level.trim()).filter(|l| !l.is_empty() && !l.eq_ignore_ascii_case("SITE")).map(level_name);
        let level = level.map(|n| sp.level(&n).map_or(n.clone(), |l| l.name.clone()));
        match sp.sheets.iter_mut().find(|x| x.number.eq_ignore_ascii_case(&r.number) && x.rect.is_none_or(|q| q == r.rect)) {
            Some(x) => {
                x.rect = Some(r.rect);
                if x.level.is_none() {
                    x.level = level;
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
            }),
        }
    }
}

fn building_levels(job: &GuidedJob) -> Vec<Value> {
    // Site, civil and landscape plans, and plans that name no level, are no storey (MUST 3).
    let rank = |l: &str| (cadaddy_walk3d::levels::code_rank(l).unwrap_or(i32::MAX - 1), l.to_string());
    let floor_plan =
        |r: &&SheetRegion| r.kind == "plan" && !r.level.trim().is_empty() && !r.level.eq_ignore_ascii_case("SITE") && r.not_built.is_empty();
    let mut plans: Vec<&SheetRegion> = job.sheet_regions.iter().filter(floor_plan).filter(|r| r.discipline == "A").collect();
    if plans.is_empty() {
        // No architectural floor plans: one discipline's plans give the floors (their
        // background shows the walls), the one with the most levels.
        let mut by: BTreeMap<&str, std::collections::BTreeSet<&str>> = BTreeMap::new();
        for r in job.sheet_regions.iter().filter(floor_plan) {
            by.entry(r.discipline.as_str()).or_default().insert(r.level.as_str());
        }
        let best = by.iter().filter(|(_, l)| l.len() > 1).max_by_key(|(d, l)| (l.len(), std::cmp::Reverse(*d))).map(|(d, _)| (*d).to_string());
        if let Some(d) = best {
            plans = job.sheet_regions.iter().filter(floor_plan).filter(|r| r.discipline == d).collect();
        }
    }
    plans.sort_by_key(|r| rank(&r.level));
    plans.dedup_by(|a, b| a.level.eq_ignore_ascii_case(&b.level));
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
        let mut level = json!({"name": name, "elevation_ft": elevation, "height_ft": height, "ceiling_ft": (height - 2.0).clamp(7.0, 9.0), "region": r.area.unwrap_or(r.rect)});
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
            let room = legible_room_name(&name) && super::autospec::room_label(&name) && !name.trim().eq_ignore_ascii_case("ROOM");
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

/// What [`load_plan_pages`] put in the drawing.
pub(crate) struct PlanPages {
    /// The sheets in the set's index (0 when classify read none).
    pub indexed: usize,
    /// One per imported page: `{sheet, level, discipline, type}`.
    pub pages: Vec<Value>,
    /// The building's levels for `bspec.draft` ([`building_levels`]; empty without
    /// architectural floor plans).
    pub levels: Vec<Value>,
    /// Where each imported sheet sits ([`add_sheet_links`]).
    pub regions: Vec<SheetRegion>,
}

/// The guided job's classify → plan pages path, for one-call PDF to BIM (`bim.from_pdf`): the
/// print set `pdfs` (a folder, or one PDF of any number of pages) is opened and classified, and
/// its current plan sheets (every page when the set has none) are imported into the current
/// drawing side by side, exactly as the job's build stage does.
pub(crate) fn load_plan_pages(s: &mut Session, pdfs: &std::path::Path) -> Result<PlanPages> {
    s.execute("printset.open", &json!({"path": pdfs.to_string_lossy()}))?;
    // The sheets' files are named from the set's folder (the PDF's own folder for one file).
    let dir = if pdfs.is_dir() { pdfs.to_path_buf() } else { pdfs.parent().map(std::path::Path::to_path_buf).unwrap_or_default() };
    let dir = if dir.as_os_str().is_empty() { std::path::PathBuf::from(".") } else { dir };
    let mut job = GuidedJob::new(&dir.to_string_lossy(), JobGoal::Bim);
    run_classify(s, &mut job)?;
    let indexed = job.index.as_ref().and_then(|i| i.get("sheets")).and_then(Value::as_array).map_or(0, Vec::len);
    ensure_sheets_loaded(s, &mut job)?;
    let pages = job.sheet_regions.iter().map(|r| json!({"sheet": r.number, "level": r.level, "discipline": r.discipline, "type": r.kind})).collect();
    Ok(PlanPages { indexed, pages, levels: building_levels(&job), regions: job.sheet_regions })
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

/// Run clash detection stage.
fn run_clash(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    // Clash detection through BIM validation.
    let result = s.execute("validate.clash", &json!({}));
    match result {
        Ok(v) => {
            job.audit("stage_run", "pipeline", None, Some(StageName::Clash), None);
            Ok(v)
        }
        Err(e) => {
            // Clash detection may not be available.
            job.audit("stage_error", "pipeline", None, Some(StageName::Clash), Some(json!({"error": e.to_string()})));
            Ok(json!({"error": e.to_string(), "note": "Clash detection not available; skipping."}))
        }
    }
}

/// Take the defaults of the questions the job doesn't ask: in auto mode every question with a
/// default; else only the verify samples after the first [`VERIFY_ASKED`] (a spot check). A
/// question in [`GuidedJob::always_ask`] is never taken by default. How many were answered.
fn auto_answer(job: &mut GuidedJob, name: StageName) -> usize {
    let (auto, always) = (job.auto, std::mem::take(&mut job.always_ask));
    let mut answered = Vec::new();
    if let Some(st) = job.stage_mut(name) {
        let asked = st.questions.iter_mut().chain(st.completeness.iter_mut()).map(|q| (q, !auto));
        let samples = st.verify.iter_mut().enumerate().map(|(i, q)| (q, !auto && i < VERIFY_ASKED));
        for (q, asked) in asked.chain(samples) {
            if q.answer.is_none()
                && !asked
                && !always.contains(&q.id)
                && let Some(def) = &q.default
            {
                q.answer = Some(Value::String(def.clone()));
                q.answered_by = Some("default".into());
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

/// How the job's questions were answered: (by default, by the agent or user, still open).
fn answer_counts(job: &GuidedJob) -> (usize, usize, usize) {
    let (mut automatic, mut by_you, mut open) = (0, 0, 0);
    for q in job.stages.iter().flat_map(|st| st.questions.iter().chain(st.verify.iter()).chain(st.completeness.iter())) {
        match (&q.answer, q.answered_by.as_deref()) {
            (None, _) => open += 1,
            (Some(_), Some("default")) => automatic += 1,
            _ => by_you += 1,
        }
    }
    (automatic, by_you, open)
}

/// The job's mode and how its questions were answered, in words too.
fn answers_value(job: &GuidedJob) -> Value {
    let (automatic, by_you, open) = answer_counts(job);
    let mut note = format!("{automatic} answered automatically, {by_you} by you");
    if open > 0 {
        note.push_str(&format!(", {open} waiting for an answer"));
    }
    if job.auto {
        note.push_str(" (auto mode: every question with a default took it)");
    }
    json!({"mode": if job.auto { "auto" } else { "interactive" }, "automatic": automatic, "by_you": by_you, "open": open, "note": note})
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
                if other && !level.is_empty() && level.len() <= 8 {
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
            "level" => r.level = value.to_string(),
            _ => r.kind = value.to_string(),
        }
    }
}

/// Advance the pipeline: run the current stage and generate questions. A finished job's
/// checkpoint is removed.
fn advance_pipeline(s: &mut Session, job: &mut GuidedJob) -> Result<()> {
    let r = advance_stages(s, job);
    if job.is_done()
        && let Some(path) = &job.checkpoint
    {
        let _ = std::fs::remove_file(path);
    }
    r
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
        let outcome = match &result {
            Ok(_) => "done".to_string(),
            Err(e) => format!("error: {e}"),
        };
        report_progress(
            s,
            json!({"stage": stage_name.label(), "step": "end", "elapsed_ms": ms, "ok": result.is_ok()}),
            &format!("{tag}: {outcome} in {:.1} s", ms as f64 / 1000.0),
        );

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
                }),
            );
            rule.auto_applied = rule.auto_applied.saturating_add(1);
        }
    }
    Ok(())
}

// ─── Command implementations ────────────────────────────────────────────────

/// `job.start {pdfs, goal}` — create and start a guided job.
fn cmd_start(s: &mut Session, p: &Value) -> Result<Value> {
    let pdfs = str_param(p, "pdfs").ok_or_else(|| bad("job.start", "`pdfs` (path) is required"))?;
    let goal_str = p.get("goal").and_then(Value::as_str).unwrap_or("estimate");
    let goal = JobGoal::from_str(goal_str)
        .ok_or_else(|| bad("job.start", format!("unknown goal '{goal_str}' — use cdy, bim, takeoff, estimate or clash")))?;

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
    // The checkpoint: a path, false (none), else the default in the temporary folder.
    job.checkpoint = match p.get("checkpoint") {
        Some(Value::Bool(false)) => None,
        Some(Value::String(path)) if !path.trim().is_empty() => Some(path.trim().to_string()),
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
        saved.auto = job.auto;
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
fn cmd_next(s: &mut Session, _p: &Value) -> Result<Value> {
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
fn cmd_answer(s: &mut Session, p: &Value) -> Result<Value> {
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
fn cmd_set_mode(s: &mut Session, p: &Value) -> Result<Value> {
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

    Ok(json!({
        "job_id": job.id,
        "goal": job.goal,
        "created": job.created,
        "pdfs": job.pdfs_path,
        "done": job.is_done(),
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
        "not_built": not_built_sheets(&job),
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

    let mut v = json!({
        "job_id": job.id,
        "goal": job.goal,
        "progress": job.progress(),
        "done": job.is_done(),
        "current_stage": job.current_stage().map(|s| s.name.label()),
        "stages": stages,
        "open_questions": job.stages.iter().map(|s| s.unanswered().len()).sum::<usize>(),
        "findings": job.findings.len(),
        "worklist_pending": pending_worklist,
        "rules_learned": job.rules.len(),
        "elapsed_ms": job.stages.iter().filter_map(|st| st.elapsed_ms).sum::<u64>(),
        "checkpoint": job.checkpoint.as_ref().filter(|_| !job.is_done()),
        "sheets": index_count(job, "sheets"),
        "levels": levels,
        "elements": elements,
        "answers": answers_value(job),
    });
    if let Some(from) = job.rerun_held {
        v["rerun_held"] = json!(from.label());
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
            .params("{pdfs: path (folder of PDFs or DWGs), goal?: \"cdy\"|\"bim\"|\"takeoff\"|\"estimate\" (default)|\"clash\", auto?: bool (default false: the job stops at each checkpoint — a low-confidence reading, a count to confirm, a spot check — until job.answer; true: every question with a default takes it and the job runs to the end, except a stage that found nothing where it should, which is always asked), resume?: bool (default true: a job of the same prints and goal cut off before it finished goes on from its checkpoint), checkpoint?: path | false (the job's drawing, saved after each stage; default a .cdy in the temporary folder, removed when the job is done)} → {job_id, goal, progress, done, current_stage, stages: [{stage, status, questions, unanswered, elapsed_ms?}], elapsed_ms, open_questions, rules_learned, answers: {mode, automatic, by_you, open, note}, question?: the question the job waits on, resumed?}; progress: job.progress events (and lines on the CLI's stderr) per stage and per sheet imported"),
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
        let stage = job.stage_mut(StageName::Devices).map(|st| st.completeness.clone()).unwrap_or_default();
        let q = stage.iter().find(|q| q.question.contains("'ZZ'")).unwrap_or_else(|| panic!("no ZZ question"));
        assert!(q.question.contains("no legend or schedule lists it"), "{}", q.question);
        assert_eq!(q.evidence.as_ref().and_then(|e| e.crop_png_path.as_deref()), Some(crop));
        assert_eq!(q.evidence.as_ref().map_or(0, |e| e.handles.len()), 4);
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
            // Junk: a circuit number, the sheet's own number, a switch leg, a keynote, an untagged symbol.
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
            });
            want_by_level.insert(format!("Level {level}"), a + b);
        }
        let n: i32 = want_by_level.values().sum();
        let r = run_devices(&mut s, &mut job).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(r["devices"]["count"], json!(n), "{r:#}");
        assert_eq!(r["devices"]["by_type"], json!({"A": 9, "B": 5}));
        // No building model yet: the bim_build stage puts them in.
        assert_eq!(r["model"]["pending"], json!(n), "{:#}", r["model"]);
        assert_eq!(job.devices.len(), n as usize);
        let first = job.devices.iter().find(|d| d.sheet == "Q-1" && d.at == [100.0, 300.0]).unwrap_or_else(|| panic!("{:?}", job.devices));
        assert_eq!((first.level.as_str(), first.circuit.as_str()), ("1", "D-7"));
        assert_eq!(job.devices.iter().filter(|d| !d.circuit.is_empty()).count(), 2, "one circuit is printed per sheet");

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
        assert_eq!((units("Level 1"), units("Level 2")), (8.0, 6.0), "{lines:#?}");
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
        }
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
}
