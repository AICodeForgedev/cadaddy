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
use std::collections::BTreeMap;

use super::{CommandSpec, bad, str_param};
use crate::{Result, Session};

// ─── Limits ─────────────────────────────────────────────────────────────────

/// Most questions one stage may produce (hostile inputs).
const MAX_QUESTIONS_PER_STAGE: usize = 500;
/// Most verify samples per stage.
const MAX_VERIFY_SAMPLES: usize = 20;
/// Most rules a job may learn.
const MAX_RULES: usize = 5_000;
/// Most audit entries kept (oldest trimmed).
const MAX_AUDIT: usize = 50_000;
/// Block timeout for `job.next` (ms) before returning progress (reserved for future use).
const _DEFAULT_WAIT_MS: u64 = 5_000;
/// Most worklist items kept.
const MAX_WORKLIST: usize = 10_000;

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
        }
    }

    /// All unanswered questions across regular, verify and completeness.
    pub fn unanswered(&self) -> Vec<&Question> {
        self.questions
            .iter()
            .chain(self.verify.iter())
            .chain(self.completeness.iter())
            .filter(|q| q.answer.is_none())
            .collect()
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

fn default_pending() -> String { "pending".into() }

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
}

impl GuidedJob {
    pub fn new(pdfs: &str, goal: JobGoal) -> Self {
        let stages: Vec<StageState> = StageName::ALL.iter().map(|&n| {
            let mut st = StageState::new(n);
            if n > goal.last_stage() {
                st.status = StageStatus::Skipped;
            }
            st
        }).collect();
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
        }
    }

    /// The current stage (first non-done, non-skipped).
    pub fn current_stage(&self) -> Option<&StageState> {
        self.stages.iter().find(|s| {
            !matches!(s.status, StageStatus::Done | StageStatus::Skipped)
        })
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
        self.stages.iter().all(|s| {
            matches!(s.status, StageStatus::Done | StageStatus::Skipped)
        })
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
    pub fn add_finding(&mut self, stage: StageName, description: &str, why: &str,
                       severity: u8, actions: Vec<String>, evidence: Option<QuestionEvidence>,
                       blocking: bool) {
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
fn load_job(s: &Session) -> Result<Option<GuidedJob>> {
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

/// Run the classify stage: classify sheets by discipline (MUST 2).
/// Discipline comes from: (1) sheet number prefix (E-4 → E), (2) title block text
/// ("ELECTRICAL PLANS"), (3) keynote headers ("ELECTRICAL KEYNOTES"). Asks when unsure.
fn run_classify(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    let classified = s.execute("printset.classify", &json!({}))?;

    // Fetch the actual sheet list (printset.classify returns {classified, total,
    // elapsed_ms} — no "sheets" array). printset.sheets returns {sheets: [...], total}.
    let sheets_result = s.execute("printset.sheets", &json!({}))?;

    // Cache the sheet list for later stages (MUST 4).
    job.sheets = Some(sheets_result);

    // MUST 1: Check for empty results.
    // Clone the sheets array out of job.sheets so we don't hold an immutable
    // borrow on `job` while later code needs `&mut job`.
    let sheets_vec: Option<Vec<Value>> = job.sheets.as_ref()
        .and_then(|v| v.get("sheets"))
        .and_then(Value::as_array)
        .cloned();
    let sheet_count = sheets_vec.as_ref().map_or(0, Vec::len);
    if sheet_count == 0 {
        job.add_finding(StageName::Classify,
            "Classify found 0 sheets in the print set",
            "A classify stage that yields 0 sheets cannot proceed",
            5,
            vec!["Check that the PDF folder is correct".into(), "Re-import with a different path".into()],
            None, true);
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
                        question: format!(
                            "Sheet {number} titled \"{title}\" — discipline looks like {disc} from the title. Confirm?",
                        ),
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
                        question: format!(
                            "Sheet {number} titled \"{title}\" — what discipline is this sheet?",
                        ),
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
            job.add_finding(StageName::Classify,
                &format!("{unknown_count} sheet(s) have unknown discipline"),
                "Sheets without a recognized discipline prefix or title keyword need manual classification",
                3, Vec::new(), None, false);
        }
    }

    job.audit("stage_run", "pipeline", None, Some(StageName::Classify),
        Some(json!({"sheets": sheet_count})));
    Ok(classified)
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

    // MUST 3: If levels.read found 0 levels, try extracting from sheet titles.
    let mut title_levels: Vec<Value> = Vec::new();
    if level_count == 0
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
    let effective_levels = if let Some(lvls) = levels.filter(|l| !l.is_empty()) {
        lvls.clone()
    } else {
        title_levels
    };
    let effective_count = effective_levels.len();

    // MUST 1: No empty success.
    if effective_count == 0 {
        job.add_finding(StageName::Levels,
            "No levels found from scale notes or plan titles",
            "The levels stage exists to find building levels; 0 means the pipeline cannot assign rooms or devices to floors",
            4,
            vec!["Provide level names manually".into(), "Check that the print set contains plan sheets".into()],
            None, false);
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
                question: format!(
                    "Sheet {sheet} title suggests level \"{name}\" — confirm and provide elevation?",
                ),
                why: "Level extracted from plan title; no scale or level note found".into(),
                choices: vec![
                    format!("Yes, \"{name}\""),
                    "Different name".into(),
                    "Not a level".into(),
                ],
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
                        choices: vec![
                            format!("Yes, {scale_text}"),
                            "No, different scale".into(),
                        ],
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
                question: format!(
                    "Level {name}: elevation {:.1} ft — name and elevation correct?",
                    elevation.unwrap_or(0.0)
                ),
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
                    question: format!(
                        "Sheet {sheet} contains {} plans ({}) — confirm these are separate levels?",
                        names.len(), names.join(", "),
                    ),
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
    let has_elevations = effective_levels.iter().any(|l|
        l.get("elevation_ft").and_then(Value::as_f64).is_some()
    );
    if effective_count > 0 && !has_elevations {
        let qid = job.next_qid(StageName::Levels);
        let level_names: Vec<&str> = effective_levels.iter()
            .filter_map(|l| l.get("name").and_then(Value::as_str))
            .collect();
        let q = Question {
            id: qid,
            stage: StageName::Levels,
            question: format!(
                "No level notes with elevations found. What is the floor-to-floor height for levels {}?",
                level_names.join(", "),
            ),
            why: "Elevations are needed for 3D model placement; no level notes found on sheets".into(),
            choices: vec![
                "10'-0\" (typical commercial)".into(),
                "12'-0\"".into(),
                "9'-0\" (typical residential)".into(),
            ],
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

    job.audit("stage_run", "pipeline", None, Some(StageName::Levels),
        Some(json!({"levels_detected": level_count, "levels_from_titles": title_level_count, "effective": effective_count})));
    Ok(levels_result)
}

/// Extract level names from a sheet title (MUST 3).
/// Recognises patterns: "ELECTRICAL - 3RD FLOOR", "4TH FLOOR PLAN", "LEVEL 2", etc.
pub fn extract_levels_from_title(title: &str) -> Vec<String> {
    let upper = title.to_uppercase();
    let mut levels = Vec::new();

    // Pattern: ordinal + FLOOR: "3RD FLOOR", "FOURTH FLOOR"
    let ordinals: &[(&str, &str)] = &[
        ("FIRST", "1"), ("1ST", "1"), ("SECOND", "2"), ("2ND", "2"),
        ("THIRD", "3"), ("3RD", "3"), ("FOURTH", "4"), ("4TH", "4"),
        ("FIFTH", "5"), ("5TH", "5"), ("SIXTH", "6"), ("6TH", "6"),
        ("SEVENTH", "7"), ("7TH", "7"), ("EIGHTH", "8"), ("8TH", "8"),
        ("NINTH", "9"), ("9TH", "9"), ("TENTH", "10"), ("10TH", "10"),
        ("BASEMENT", "B1"), ("GROUND", "G"), ("MEZZANINE", "M"),
        ("PENTHOUSE", "PH"), ("ROOF", "R"),
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
/// Each plan-type sheet (plan / enlarged_plan) is imported via `pdfimport`.
fn ensure_sheets_loaded(s: &mut Session, job: &mut GuidedJob) -> Result<()> {
    if job.sheets_loaded {
        return Ok(());
    }
    let sheets_val = match &job.sheets {
        Some(v) => v.clone(),
        None => return Err(bad("guided", "no sheets cached — classify must run first")),
    };
    let sheet_arr = sheets_val.get("sheets").and_then(Value::as_array).cloned().unwrap_or_default();
    let plan_sheets: Vec<&Value> = sheet_arr.iter().filter(|sh| {
        let st = sh.get("sheet_type").and_then(Value::as_str).unwrap_or("");
        st == "plan" || st == "enlarged_plan"
    }).collect();
    if plan_sheets.is_empty() {
        // Fall back to all sheets if no plans classified yet.
        for sh in &sheet_arr {
            import_one_sheet(s, &job.pdfs_path, sh)?;
        }
    } else {
        for sh in &plan_sheets {
            import_one_sheet(s, &job.pdfs_path, sh)?;
        }
    }
    job.sheets_loaded = true;
    job.audit("sheets_loaded", "pipeline", None, None,
        Some(json!({"plan_sheets": plan_sheets.len(), "total_sheets": sheet_arr.len()})));
    Ok(())
}

/// Import a single sheet's file into the current drawing via `pdfimport`.
///
/// First imports at default scale (sheet size) to detect the architectural scale note,
/// then re-imports at full scale if needed so that geometry is at building dimensions
/// (walls are 6" thick, not 0.06" at sheet scale).
fn import_one_sheet(s: &mut Session, base_dir: &str, sheet: &Value) -> Result<()> {
    let file = match sheet.get("file").and_then(Value::as_str) {
        Some(f) if !f.is_empty() => f,
        _ => return Ok(()),
    };
    let abs_path = if std::path::Path::new(file).is_absolute() {
        file.to_string()
    } else {
        format!("{}/{}", base_dir, file)
    };
    let page = sheet.get("page").and_then(Value::as_u64).unwrap_or(1);
    // Use the sheet's known scale if available; otherwise let pdfimport detect it.
    // Passing "scale": "auto" lets the reader use the detected architectural scale
    // and override the DO NOT SCALE note (guided runs always need real dimensions).
    let scale_val = sheet.get("scale").and_then(Value::as_f64)
        .filter(|k| k.is_finite() && *k > 1.0);
    let params = if let Some(k) = scale_val {
        json!({"path": abs_path, "page": page, "scale": k})
    } else {
        json!({"path": abs_path, "page": page})
    };
    match s.execute("pdfimport", &params) {
        Ok(report) => {
            // If the import was at sheet scale (DO NOT SCALE) and a scale was detected,
            // re-import at full scale so wall/room detection works at building dimensions.
            let needs_reg = report.get("needs_registration").and_then(Value::as_bool).unwrap_or(false);
            if needs_reg && scale_val.is_none() {
                // Extract the detected scale factor from the scaleSource string
                // e.g. "DO NOT SCALE (detected 1/8" = 1'-0", scale 96x — sheet kept at sheet size)"
                let detected = report.get("scaleSource").and_then(Value::as_str)
                    .and_then(|src| {
                        // Look for "scale NNx" pattern
                        src.find("scale ").and_then(|i| {
                            let rest = &src[i + 6..];
                            let end = rest.find('x').unwrap_or(rest.len());
                            rest[..end].trim().parse::<f64>().ok()
                        })
                    });
                if let Some(k) = detected.filter(|k| *k > 1.5) {
                    // Undo the sheet-scale import and redo at full scale.
                    let _ = s.execute("u", &json!({}));
                    let full_params = json!({"path": abs_path, "page": page, "scale": k});
                    match s.execute("pdfimport", &full_params) {
                        Ok(_) => {}
                        Err(e) => {
                            s.echo(format!("guided: full-scale re-import of {file} failed: {e}"));
                        }
                    }
                }
            }
            Ok(())
        }
        Err(e) => {
            // Non-fatal: log the error but continue with other sheets.
            s.echo(format!("guided: could not import sheet {file}: {e}"));
            Ok(())
        }
    }
}

/// Count device symbols detected by pdfnative's symbol-cluster detection (PDF-SYM-N blocks).
/// Returns a map of device-tag → count.
fn count_symbol_block_devices(s: &Session) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let Ok(d) = s.doc() else { return counts };
    let space = s.space();
    let store = match d.space(&space) {
        Some(st) => st,
        None => return counts,
    };
    for e in store.iter() {
        if let cadaddy_doc::EntityKind::Insert(ins) = &e.kind {
            if ins.block.starts_with("PDF-SYM-") {
                // The TAG attribute holds the device mark text.
                let tag = ins.attribs.iter()
                    .find(|a| a.tag == "TAG")
                    .map(|a| a.text.value.trim().to_string())
                    .unwrap_or_default();
                if !tag.is_empty() {
                    *counts.entry(tag).or_default() += 1;
                } else {
                    // Symbol without a tag — count as "unknown".
                    *counts.entry("unknown_symbol".to_string()).or_default() += 1;
                }
            }
        }
    }
    counts
}

/// Run the rooms stage: detect rooms from labels (MUST 1, 4).
/// Uses the same readers (rooms.list) that work directly on the job's sheets.
fn run_rooms(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    // Load plan-sheet PDFs into the drawing so rooms.list finds entities.
    ensure_sheets_loaded(s, job)?;
    let rooms_result = s.execute("rooms.list", &json!({"outline": false}))?;

    let rooms = rooms_result.get("rooms").and_then(Value::as_array);
    let room_count = rooms.map_or(0, Vec::len);

    // MUST 1: No empty success — 0 rooms is a finding.
    if room_count == 0 {
        job.add_finding(StageName::Rooms,
            "Rooms stage found 0 rooms",
            "The rooms reader found no room labels or outlines in the loaded sheets",
            4,
            vec![
                "Check that the loaded sheets contain room tags".into(),
                "Try rooms.list directly on individual sheets".into(),
            ],
            None, false);
        // Raise a question rather than silently proceeding.
        let qid = job.next_qid(StageName::Rooms);
        let q = Question {
            id: qid,
            stage: StageName::Rooms,
            question: "No rooms detected on any sheet. Are there room tags on the plans?".into(),
            why: "The rooms reader found 0 rooms; the building model needs room boundaries".into(),
            choices: vec![
                "There should be rooms — re-check".into(),
                "No room tags on these sheets".into(),
            ],
            evidence: None,
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
                    choices: vec![
                        format!("Keep '{name}' as room name"),
                        "Use a different name".into(),
                        "Not a room — remove it".into(),
                    ],
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

    job.audit("stage_run", "pipeline", None, Some(StageName::Rooms),
        Some(json!({"rooms_found": room_count})));
    Ok(rooms_result)
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
    let sym_devices = count_symbol_block_devices(s);
    let sym_count: usize = sym_devices.values().sum();
    if sym_count > 0 {
        total_items += sym_count;
        let items: Vec<Value> = sym_devices.iter().map(|(tag, &count)| {
            json!({"tag": tag, "type": tag, "count": count, "source": "pdf_symbol"})
        }).collect();
        let by_type: serde_json::Map<String, Value> = sym_devices.iter()
            .map(|(k, v)| (k.clone(), json!(*v))).collect();
        results["devices"] = json!({
            "items": items,
            "count": sym_count,
            "by_type": by_type,
            "source": "pdf_symbol_blocks",
        });
    }

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
    for (cmd, key) in [
        ("mep.read_devices", "mep_devices"),
        ("conduit.read", "conduit"),
        ("gear.read", "gear"),
    ] {
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

    // MUST 1: No empty success — 0 devices found is a finding.
    if total_items == 0 {
        let msg = if reader_errors.is_empty() {
            "Devices stage found 0 devices on all sheets".to_string()
        } else {
            format!("Devices stage found 0 devices; reader errors: {}", reader_errors.join("; "))
        };
        job.add_finding(StageName::Devices,
            &msg,
            "No E-sheets found in the project or readers returned empty; check that classify tagged electrical sheets",
            4,
            vec![
                "Check that classify identified E-discipline sheets".into(),
                "Try mep.read_devices directly".into(),
            ],
            None, false);
        let qid = job.next_qid(StageName::Devices);
        let q = Question {
            id: qid,
            stage: StageName::Devices,
            question: "No devices detected. Are there device symbols on the electrical sheets?".into(),
            why: "The device reader found 0 items; the takeoff needs device counts".into(),
            choices: vec![
                "There should be devices — re-check".into(),
                "No devices on these sheets".into(),
            ],
            evidence: None,
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
            let count = devices.get("by_type")
                .and_then(|bt| bt.get(*class_key))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            // MUST 9: Always ask — even for 0 ("I counted 0 X — anything not counted?").
            let qid = job.next_qid(StageName::Devices);
            let q = Question {
                id: qid,
                stage: StageName::Devices,
                question: format!(
                    "I counted {count} {class_label}. Are there any {class_label} I did not count?",
                ),
                why: format!("Completeness checkpoint for {class_label}"),
                choices: vec![
                    "Count is complete".into(),
                    "There are uncounted devices".into(),
                ],
                evidence: Some(QuestionEvidence {
                    crop_png_path: None,
                    sheet: None,
                    at: None,
                    values: Some(json!({"class": class_key, "count": count})),
                    handles: Vec::new(),
                    // MUST 9: tiled_crops with every counted item boxed.
                    tiled_crops: devices.get("tiled_crops")
                        .and_then(|tc| tc.get(*class_key))
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()),
                    // MUST 9: leftover clusters that might be uncounted.
                    leftovers: devices.get("unknowns")
                        .and_then(Value::as_array)
                        .map_or_else(Vec::new, |unknowns| {
                            unknowns.iter().take(10).map(|u| LeftoverCandidate {
                                description: u.get("description").and_then(Value::as_str).unwrap_or("?").to_string(),
                                crop_path: u.get("crop_path").and_then(Value::as_str).map(String::from),
                                at: u.get("at").and_then(|a| {
                                    let arr = a.as_array()?;
                                    Some([arr.first()?.as_f64()?, arr.get(1)?.as_f64()?])
                                }),
                                sheet: u.get("sheet").and_then(Value::as_str).map(String::from),
                                count: u.get("count").and_then(Value::as_u64).unwrap_or(1) as usize,
                            }).collect()
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
                let qid = job.next_qid(StageName::Devices);
                let q = Question {
                    id: qid,
                    stage: StageName::Devices,
                    question: format!("Unknown symbol: '{desc}' found {count} time(s) — which device is it?"),
                    why: "Foreign/unknown symbol not matched to any library entry; learning it adds to future counts".into(),
                    choices: Vec::new(),
                    evidence: Some(QuestionEvidence {
                        crop_png_path: unk.get("crop_path").and_then(Value::as_str).map(String::from),
                        sheet: unk.get("sheet").and_then(Value::as_str).map(String::from),
                        at: unk.get("at").and_then(|a| {
                            let arr = a.as_array()?;
                            Some([arr.first()?.as_f64()?, arr.get(1)?.as_f64()?])
                        }),
                        values: Some(json!({"description": desc, "count": count})),
                        handles: Vec::new(),
                        tiled_crops: None,
                        leftovers: Vec::new(),
                    }),
                    default: Some("skip".into()),
                    answer_schema: Some(json!({"type": "object", "properties": {"device_type": {"type": "string"}, "symbol_name": {"type": "string"}}})),
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

    job.audit("stage_run", "pipeline", None, Some(StageName::Devices),
        Some(json!({"total_items": total_items, "reader_errors": reader_errors.len()})));
    Ok(results)
}

/// Run the BIM build stage (MUST 1).
/// Must draft/store the building spec itself (bspec.draft store) before building.
fn run_bim_build(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    // Ensure sheet content is in the drawing so bspec.draft can read geometry.
    ensure_sheets_loaded(s, job)?;
    // MUST 1: Draft the spec first — bim.build needs it. Without the spec, it errors.
    let draft_result = s.execute("bspec.draft", &json!({"store": true}));
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
            job.audit("bspec_draft", "pipeline", None, Some(StageName::BimBuild),
                Some(json!({"stored": true})));
        }
        Err(e) => {
            // Spec draft failed — this is a finding, not a silent error.
            job.add_finding(StageName::BimBuild,
                &format!("Building spec draft failed: {e}"),
                "bspec.draft must succeed before bim.build can run",
                5,
                vec!["Check that rooms and levels were detected".into()],
                None, true);
            return Err(bad("bim_build", format!("bspec.draft failed: {e}")));
        }
    }

    // Use model.build directly — it returns walls/doors/rooms/levels counts.
    // (bim.build wraps model.build but constructs its own result without those counts.)
    let result = s.execute("model.build", &json!({}))?;

    // Check for warnings.
    if let Some(warnings) = result.get("warnings").and_then(Value::as_array) {
        for w in warnings.iter().take(MAX_QUESTIONS_PER_STAGE) {
            let msg = w.get("message").and_then(Value::as_str)
                .or_else(|| w.as_str())
                .unwrap_or("Build warning");
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
        job.add_finding(StageName::BimBuild,
            "BIM build produced 0 elements",
            "The build returned no geometry; the building spec may be empty or incorrect",
            5,
            vec!["Review the building spec".into(), "Check that rooms and levels were found".into()],
            None, false);
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

    job.audit("stage_run", "pipeline", None, Some(StageName::BimBuild),
        Some(json!({"built_count": built_count})));
    Ok(result)
}

/// Run takeoff stage.
fn run_takeoff(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    ensure_sheets_loaded(s, job)?;
    let result = s.execute("takeoff", &json!({}))?;

    // MUST 4: takeoff with 0 lines is a finding, never silently Done.
    let line_count = result.get("lines").and_then(Value::as_array).map_or(0, Vec::len)
        .max(result.get("count").and_then(Value::as_u64).unwrap_or(0) as usize);
    if line_count == 0 {
        job.add_finding(StageName::Takeoff,
            "Takeoff produced 0 line items",
            "A takeoff with no line items indicates that earlier stages (rooms, devices, BIM) \
             may not have detected enough data to generate quantities",
            4,
            vec![
                "Review rooms and devices stages for 0-count findings".into(),
                "Check that BIM build produced elements".into(),
            ],
            None, false);
    }

    job.audit("stage_run", "pipeline", None, Some(StageName::Takeoff),
        Some(json!({"line_count": line_count})));
    Ok(result)
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

/// Auto-answer questions that have a default value, so the pipeline keeps moving.
/// Questions are still recorded (for the agent to review later) but don't block.
fn auto_answer_defaults(stage: &mut StageState) -> usize {
    let mut answered = 0usize;
    for q in stage.questions.iter_mut()
        .chain(stage.verify.iter_mut())
        .chain(stage.completeness.iter_mut())
    {
        if q.answer.is_none()
            && let Some(ref def) = q.default
        {
            q.answer = Some(Value::String(def.clone()));
            q.answered_by = Some("default".into());
            answered += 1;
        }
    }
    answered
}

/// Advance the pipeline: run the current stage and generate questions.
fn advance_pipeline(s: &mut Session, job: &mut GuidedJob) -> Result<()> {
    loop {
        let stage_name = match job.current_stage() {
            Some(st) if st.status == StageStatus::Paused => {
                let name = st.name;
                // Auto-answer defaulted questions so the pipeline can proceed.
                if let Some(st) = job.stage_mut(name) {
                    auto_answer_defaults(st);
                }
                // Still have unanswered questions with no default — pause.
                if let Some(st) = job.stage(name) {
                    if !st.unanswered().is_empty() {
                        return Ok(());
                    }
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
        let result = match stage_name {
            StageName::Import => run_import(s, job),
            StageName::Classify => run_classify(s, job),
            StageName::Levels => run_levels(s, job),
            StageName::Rooms => run_rooms(s, job),
            StageName::Devices => run_devices(s, job),
            StageName::BimBuild => run_bim_build(s, job),
            StageName::Takeoff => run_takeoff(s, job),
            StageName::Estimate => run_estimate(s, job),
            StageName::Clash => run_clash(s, job),
        };

        match result {
            Ok(v) => {
                // Check for blocking findings before marking done.
                let has_blocking = job.findings.iter().any(|f| f.stage == stage_name && f.blocking);
                // Store result and auto-answer defaults so the pipeline keeps moving.
                let (all_answered, unanswered_count) = {
                    let st = match job.stage_mut(stage_name) {
                        Some(st) => { st.result = Some(v); st }
                        None => continue,
                    };
                    auto_answer_defaults(st);
                    let ua = st.unanswered().len();
                    (ua == 0, ua)
                };
                if all_answered && !has_blocking {
                    if let Some(st) = job.stage_mut(stage_name) { st.status = StageStatus::Done; }
                    job.audit("stage_done", "pipeline", None, Some(stage_name), None);
                } else if has_blocking {
                    // MUST 1: blocking finding → Error, not Done.
                    let msg = job.findings.iter()
                        .filter(|f| f.stage == stage_name && f.blocking)
                        .map(|f| f.description.as_str())
                        .collect::<Vec<_>>().join("; ");
                    if let Some(st) = job.stage_mut(stage_name) {
                        st.status = StageStatus::Error;
                        st.error = Some(msg.clone());
                    }
                    job.audit("stage_blocked", "pipeline", None, Some(stage_name),
                        Some(json!({"blocking_findings": msg})));
                    return Ok(());
                } else {
                    if let Some(st) = job.stage_mut(stage_name) { st.status = StageStatus::Paused; }
                    job.audit("stage_paused", "pipeline", None, Some(stage_name),
                        Some(json!({"unanswered": unanswered_count})));
                    return Ok(());
                }
            }
            Err(e) => {
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
                job.add_finding(stage_name,
                    &format!("{} error: {msg}", stage_name.label()),
                    "Stage encountered an error and could not complete",
                    5, actions, evidence, true);
                if let Some(st) = job.stage_mut(stage_name) {
                    st.status = StageStatus::Error;
                    st.error = Some(msg.clone());
                }
                job.audit("stage_error", "pipeline", None, Some(stage_name),
                    Some(json!({"error": msg})));
                return Ok(());
            }
        }
    }
}

/// Recovery actions for a stage error (RW2 MUST 2).
pub fn stage_error_actions(stage: StageName) -> Vec<String> {
    let mut actions = vec![
        "Retry this stage".into(),
        "Skip this stage and continue".into(),
    ];
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
                let _ = s.execute("project.rules.set", &json!({
                    "domain": "custom",
                    "key": &rule.key,
                    "value": &rule.value,
                    "source": "guided_job",
                    "tier": "project_rule",
                }));
                rule.auto_applied = rule.auto_applied.saturating_add(1);
            }
        }
        // Apply scale overrides.
        if rule.key.starts_with("scale:") {
            let _ = s.execute("project.rules.set", &json!({
                "domain": "custom",
                "key": &rule.key,
                "value": &rule.value,
                "source": "guided_job",
                "tier": "project_rule",
            }));
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
        return Err(bad("job.start", "a job is already running — use job.status to see it, or start a new drawing"));
    }

    let mut job = GuidedJob::new(pdfs, goal);
    job.audit("job_start", "pipeline", None, None, Some(json!({"pdfs": pdfs, "goal": goal_str})));

    // Run the pipeline until it pauses or finishes.
    advance_pipeline(s, &mut job)?;
    save_job(s, &job)?;

    let status = job_status_value(&job);
    Ok(status)
}

/// `job.next` — return the next question, or "done".
fn cmd_next(s: &mut Session, _p: &Value) -> Result<Value> {
    let mut job = load_job(s)?.ok_or_else(|| bad("job.next", "no job running — call job.start first"))?;

    // Try to advance if no questions pending.
    if job.next_question().is_none() && !job.is_done() {
        advance_pipeline(s, &mut job)?;
        save_job(s, &job)?;
    }

    match job.next_question() {
        Some(q) => Ok(json!({
            "question": {
                "id": q.id,
                "stage": q.stage,
                "question": q.question,
                "why": q.why,
                "choices": q.choices,
                "evidence": q.evidence,
                "default": q.default,
                "answer_schema": q.answer_schema,
                "kind": q.kind,
            },
            "progress": job.progress(),
            "stage": job.current_stage().map(|s| s.name.label()),
        })),
        None if job.is_done() => Ok(json!({
            "done": true,
            "progress": 1.0,
            "summary": job_summary(&job),
        })),
        None => Ok(json!({
            "waiting": true,
            "progress": job.progress(),
            "stage": job.current_stage().map(|s| s.name.label()),
            "status": job.current_stage().map(|s| format!("{:?}", s.status)),
        })),
    }
}

/// `job.answer {id, answer}` — answer a question.
fn cmd_answer(s: &mut Session, p: &Value) -> Result<Value> {
    let qid = str_param(p, "id").ok_or_else(|| bad("job.answer", "`id` (question id) is required"))?;
    let answer = p.get("answer").cloned().unwrap_or(Value::Null);
    if answer.is_null() {
        return Err(bad("job.answer", "`answer` is required"));
    }

    let mut job = load_job(s)?.ok_or_else(|| bad("job.answer", "no job running"))?;

    // Find the question by index (to avoid borrow issues with learn_from_answer).
    let mut found_loc: Option<(usize, &'static str, usize)> = None; // (stage_idx, list, q_idx)
    'outer: for (si, stage) in job.stages.iter().enumerate() {
        for (qi, q) in stage.questions.iter().enumerate() {
            if q.id == qid { found_loc = Some((si, "questions", qi)); break 'outer; }
        }
        for (qi, q) in stage.verify.iter().enumerate() {
            if q.id == qid { found_loc = Some((si, "verify", qi)); break 'outer; }
        }
        for (qi, q) in stage.completeness.iter().enumerate() {
            if q.id == qid { found_loc = Some((si, "completeness", qi)); break 'outer; }
        }
    }
    let (si, list, qi) = found_loc.ok_or_else(|| bad("job.answer", format!("question '{qid}' not found")))?;
    let stage_name = job.stages.get(si).map(|s| s.name);

    // Set the answer on the question.
    {
        let stage = job.stages.get_mut(si).ok_or_else(|| bad("job.answer", "internal error"))?;
        let q = match list {
            "questions" => stage.questions.get_mut(qi),
            "verify" => stage.verify.get_mut(qi),
            "completeness" => stage.completeness.get_mut(qi),
            _ => None,
        }.ok_or_else(|| bad("job.answer", "internal error"))?;
        q.answer = Some(answer.clone());
        q.answered_by = Some("agent".into());
        q.answered_at = Some(now_iso());
    }

    // Learn rules from the answer (needs &mut job, so done after the borrow above).
    {
        let stage = job.stages.get(si).ok_or_else(|| bad("job.answer", "internal error"))?;
        let q = match list {
            "questions" => stage.questions.get(qi),
            "verify" => stage.verify.get(qi),
            "completeness" => stage.completeness.get(qi),
            _ => None,
        }.ok_or_else(|| bad("job.answer", "internal error"))?;
        // Clone the question data we need for learning.
        let q_clone = q.clone();
        learn_from_answer(&mut job, &q_clone);
    }

    job.audit("answer", "agent", Some(qid), stage_name, Some(json!({"answer": answer})));

    // MUST 5 REFRAME: Sync worklist — mark this item resolved.
    if let Some(wi) = job.worklist.iter_mut().find(|w| w.id == *qid) {
        wi.status = "resolved".into();
        wi.resolution = Some(answer.clone());
    }

    // Check if the stage's questions are all answered — if so, advance.
    if let Some(sn) = stage_name
        && let Some(st) = job.stage_mut(sn)
    {
        if st.unanswered().is_empty() && st.status == StageStatus::Paused {
            // A rejected verify sample may need to re-run the stage.
            let needs_rerun = check_rejected_samples(st);
            if needs_rerun {
                st.status = StageStatus::Pending; // Re-run the stage.
                job.audit("stage_rerun", "pipeline", None, Some(sn),
                    Some(json!({"reason": "rejected verify sample"})));
            } else {
                st.status = StageStatus::Done;
                job.audit("stage_done", "pipeline", None, Some(sn), None);
            }
        }
        // Try to advance.
        advance_pipeline(s, &mut job)?;
    }

    save_job(s, &job)?;

    // Return the next question or done.
    match job.next_question() {
        Some(q) => Ok(json!({
            "answered": qid,
            "next": {
                "id": q.id,
                "stage": q.stage,
                "question": q.question,
                "why": q.why,
                "choices": q.choices,
                "evidence": q.evidence,
                "default": q.default,
                "kind": q.kind,
            },
            "progress": job.progress(),
        })),
        None if job.is_done() => Ok(json!({
            "answered": qid,
            "done": true,
            "progress": 1.0,
            "summary": job_summary(&job),
        })),
        None => Ok(json!({
            "answered": qid,
            "progress": job.progress(),
            "stage": job.current_stage().map(|s| s.name.label()),
        })),
    }
}

/// `job.status` — current job status.
fn cmd_status(s: &mut Session, _p: &Value) -> Result<Value> {
    let job = load_job(s)?.ok_or_else(|| bad("job.status", "no job running"))?;
    Ok(job_status_value(&job))
}

/// `job.explain {stage}` — explain what a stage does and its current state.
fn cmd_explain(s: &mut Session, p: &Value) -> Result<Value> {
    let job = load_job(s)?.ok_or_else(|| bad("job.explain", "no job running"))?;
    let stage_label = p.get("stage").and_then(Value::as_str).unwrap_or("");

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
            let stages: Vec<Value> = job.stages.iter().map(|st| json!({
                "stage": st.name.label(),
                "description": st.name.description(),
                "status": format!("{:?}", st.status),
            })).collect();
            Ok(json!({"stages": stages, "note": "Provide a stage name to get details."}))
        }
    }
}

/// `job.rules` — list learned project rules.
fn cmd_rules(s: &mut Session, _p: &Value) -> Result<Value> {
    let job = load_job(s)?.ok_or_else(|| bad("job.rules", "no job running"))?;
    let rules: Vec<Value> = job.rules.iter().map(|r| json!({
        "key": r.key,
        "value": r.value,
        "from_question": r.from_question,
        "stage": r.stage,
        "auto_applied": r.auto_applied,
    })).collect();
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

    let stages_summary: Vec<Value> = job.stages.iter().filter(|s| s.status != StageStatus::Skipped).map(|st| json!({
        "stage": st.name.label(),
        "status": format!("{:?}", st.status),
        "questions": st.questions.len() + st.verify.len() + st.completeness.len(),
        "answered": st.questions.iter().chain(st.verify.iter()).chain(st.completeness.iter()).filter(|q| q.answer.is_some()).count(),
    })).collect();

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
        "rules_learned": job.rules.len(),
        "audit_entries": job.audit.len(),
    }))
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn job_status_value(job: &GuidedJob) -> Value {
    let stages: Vec<Value> = job.stages.iter().map(|st| json!({
        "stage": st.name.label(),
        "status": format!("{:?}", st.status),
        "questions": st.questions.len() + st.verify.len() + st.completeness.len(),
        "unanswered": st.unanswered().len(),
    })).collect();

    let pending_worklist = job.worklist.iter().filter(|w| w.status == "pending").count();

    json!({
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
    })
}

fn job_summary(job: &GuidedJob) -> Value {
    let mut results = json!({});
    for stage in &job.stages {
        if stage.status != StageStatus::Skipped {
            results[stage.name.label()] = json!({
                "status": format!("{:?}", stage.status),
                "questions_asked": stage.questions.len() + stage.verify.len() + stage.completeness.len(),
            });
        }
    }
    json!({
        "goal": job.goal,
        "stages_completed": job.stages.iter().filter(|s| s.status == StageStatus::Done).count(),
        "total_questions": job.stages.iter().map(|s| s.questions.len() + s.verify.len() + s.completeness.len()).sum::<usize>(),
        "rules_learned": job.rules.len(),
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
        job.learn_rule(
            &format!("symbol:{desc}"),
            &format!("device:{device_type}"),
            &q.id,
            q.stage,
        );
    }

    // Learn scale overrides.
    if q.stage == StageName::Levels
        && q.question.contains("scale reads")
        && let Some(ans) = answer.as_str()
        && let Some(ev) = &q.evidence
        && let Some(sheet) = &ev.sheet
    {
        job.learn_rule(
            &format!("scale:{sheet}"),
            ans,
            &q.id,
            q.stage,
        );
    }

    // Learn room name overrides.
    if q.stage == StageName::Rooms
        && q.question.contains("looks like equipment")
        && let Some(ans) = answer.as_str()
        && let Some(ev) = &q.evidence
        && let Some(room_name) = ev.values.as_ref().and_then(|v| v.get("room_name")).and_then(Value::as_str)
    {
        job.learn_rule(
            &format!("label:{room_name}"),
            &format!("room_override:{ans}"),
            &q.id,
            q.stage,
        );
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
    let epoch = web_time::SystemTime::now()
        .duration_since(web_time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
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

    let items: Vec<Value> = job.worklist.iter()
        .filter(|w| status_filter.is_none_or(|sf| w.status == sf))
        .filter(|w| stage_filter.is_none_or(|sf| w.stage.label() == sf))
        .map(|w| json!({
            "id": w.id,
            "kind": w.kind,
            "stage": w.stage.label(),
            "summary": w.summary,
            "status": w.status,
            "choices": w.choices,
            "evidence": w.evidence,
            "resolution": w.resolution,
            "has_undo": w.undo_delta.is_some(),
        }))
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

    let wi = job.worklist.iter_mut().find(|w| w.id == *wid)
        .ok_or_else(|| bad("job.undo", format!("worklist item '{wid}' not found")))?;

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
            .params("{pdfs: path (folder of PDFs or DWGs), goal?: \"cdy\"|\"bim\"|\"takeoff\"|\"estimate\" (default)|\"clash\"} → {job_id, goal, progress, done, current_stage, stages, open_questions, rules_learned}"),
        CommandSpec::new("job.next", "Next Job Question", cmd_next)
            .params("{} → {question?: {id, stage, question, why, choices, evidence, default, kind}, done?, progress, stage?}")
            .noundo(),
        CommandSpec::new("job.answer", "Answer Job Question", cmd_answer)
            .params("{id: question id, answer: value} → {answered, next?: question, done?, progress}"),
        CommandSpec::new("job.status", "Job Status", cmd_status)
            .params("{} → {job_id, goal, progress, done, current_stage, stages, open_questions, findings, worklist_pending, rules_learned}")
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
        if let Some(st) = job.stage_mut(StageName::Import) { st.status = StageStatus::Done; }
        if let Some(st) = job.stage_mut(StageName::Classify) { st.status = StageStatus::Done; }
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
}
