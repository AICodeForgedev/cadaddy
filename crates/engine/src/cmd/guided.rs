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
    /// Global question counter (for unique ids).
    question_seq: u32,
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
            question_seq: 0,
        }
    }

    /// The current stage (first non-done, non-skipped).
    pub fn current_stage(&self) -> Option<&StageState> {
        self.stages.iter().find(|s| {
            !matches!(s.status, StageStatus::Done | StageStatus::Skipped)
        })
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

/// Run the classify stage: classify sheets by discipline.
fn run_classify(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    let classified = s.execute("printset.classify", &json!({}))?;
    job.audit("stage_run", "pipeline", None, Some(StageName::Classify), Some(json!({"result": "classified"})));
    Ok(classified)
}

/// Run the levels stage: detect levels, scales, elevations.
fn run_levels(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    let levels_result = s.execute("levels.read", &json!({"apply": true}))?;

    // Check each level for low confidence — generate questions.
    if let Some(levels) = levels_result.get("levels").and_then(Value::as_array) {
        for (i, level) in levels.iter().take(MAX_QUESTIONS_PER_STAGE).enumerate() {
            let name = level.get("name").and_then(Value::as_str).unwrap_or("?");
            let elevation = level.get("elevation_ft").and_then(Value::as_f64);

            // Check for scale notes that need confirmation.
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

            // If level conflicts exist, ask.
            if let Some(conflicts) = levels_result.get("conflicts").and_then(Value::as_array) {
                // Already handled above per-level, but generate a summary question if many.
                if i == 0 && conflicts.len() > 1 {
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
            }

            // Verify sample: for the first N levels, ask the agent to confirm.
            if i < MAX_VERIFY_SAMPLES {
                let qid = job.next_qid(StageName::Levels);
                let q = Question {
                    id: qid.clone(),
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
    }

    // Check for two plans on one sheet (MUST 2 example).
    if let Some(levels) = levels_result.get("levels").and_then(Value::as_array) {
        // Group by sheet to find multiple levels per sheet.
        let mut by_sheet: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for level in levels {
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
                        "Two plans on sheet {sheet} titled {} — make them {}?",
                        names.join(" and "),
                        names.iter().enumerate().map(|(i, _)| format!("Level {}", i + 1)).collect::<Vec<_>>().join(" and ")
                    ),
                    why: "Multiple plan areas detected on one sheet".into(),
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

    job.audit("stage_run", "pipeline", None, Some(StageName::Levels), Some(json!({"result": "levels_read"})));
    Ok(levels_result)
}

/// Run the rooms stage: detect rooms from labels.
fn run_rooms(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    let rooms_result = s.execute("rooms.list", &json!({"outline": false}))?;

    if let Some(rooms) = rooms_result.get("rooms").and_then(Value::as_array) {
        // Check for equipment labels inside rooms (MUST 2 example).
        let equipment_patterns = ["PNLBD", "PANEL", "XFMR", "TRANS", "MCC", "SWG", "ATS", "GEN", "MDP", "CDP"];
        for room in rooms.iter().take(MAX_QUESTIONS_PER_STAGE) {
            let name = room.get("name").and_then(Value::as_str).unwrap_or("?");
            let number = room.get("number").and_then(Value::as_str).unwrap_or("");

            // Check if room name looks like equipment.
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
                    default: None,
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

        // Verify samples: spot-check random rooms.
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

    job.audit("stage_run", "pipeline", None, Some(StageName::Rooms), None);
    Ok(rooms_result)
}

/// Run the devices stage: detect devices, symbols and counts (MUST 9-11).
fn run_devices(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    // Run readers in dependency order.
    let mut results = json!({});

    // Run each device reader and collect results.
    for (cmd, key) in [
        ("mep.read_devices", "devices"),
        ("conduit.read", "conduit"),
        ("gear.read", "gear"),
    ] {
        match s.execute(cmd, &json!({})) {
            Ok(v) => { results[key] = v; }
            Err(e) => { results[key] = json!({"error": e.to_string()}); }
        }
    }

    // MUST 9: Strong deterministic counting, then completeness checkpoint.
    // For each device class, generate a completeness question: "I counted N — anything not counted?"
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
            if count > 0 {
                let qid = job.next_qid(StageName::Devices);
                let q = Question {
                    id: qid,
                    stage: StageName::Devices,
                    question: format!(
                        "I counted {count} {class_label}. Are there any devices I did not count?"
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
                        tiled_crops: None,
                        leftovers: Vec::new(),
                    }),
                    default: Some("Count is complete".into()),
                    answer_schema: None,
                    answer: None,
                    answered_by: None,
                    answered_at: None,
                    kind: QuestionKind::Completeness,
                };
                if let Some(st) = job.stage_mut(StageName::Devices) {
                    st.completeness.push(q);
                }
            }
        }

        // MUST 10: Unknown/foreign symbols — leftover candidates.
        if let Some(unknowns) = devices.get("unknowns").and_then(Value::as_array) {
            for unk in unknowns.iter().take(MAX_QUESTIONS_PER_STAGE) {
                let desc = unk.get("description").and_then(Value::as_str).unwrap_or("unknown symbol");
                let count = unk.get("count").and_then(Value::as_u64).unwrap_or(1);
                let qid = job.next_qid(StageName::Devices);
                let q = Question {
                    id: qid,
                    stage: StageName::Devices,
                    question: format!("Unknown symbol: '{desc}' found {count} time(s) — which device is it?"),
                    why: "Foreign/unknown symbol not matched to any library entry".into(),
                    choices: Vec::new(), // Free-form answer.
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
                    default: None,
                    answer_schema: Some(json!({"type": "object", "properties": {"device_type": {"type": "string"}, "symbol_name": {"type": "string"}}})),
                    answer: None,
                    answered_by: None,
                    answered_at: None,
                    kind: QuestionKind::Completeness,
                };
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

    job.audit("stage_run", "pipeline", None, Some(StageName::Devices), None);
    Ok(results)
}

/// Run the BIM build stage.
fn run_bim_build(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    let result = s.execute("bim.build", &json!({}))?;

    // Check for warnings that need confirmation.
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

    // Plausibility check: total quantities.
    if let Some(counts) = result.get("counts").and_then(Value::as_object) {
        for (key, val) in counts {
            if let Some(n) = val.as_f64() {
                // Flag suspiciously large quantities.
                if n > 100_000.0 {
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
    }

    job.audit("stage_run", "pipeline", None, Some(StageName::BimBuild), None);
    Ok(result)
}

/// Run takeoff stage.
fn run_takeoff(s: &mut Session, job: &mut GuidedJob) -> Result<Value> {
    let result = s.execute("takeoff", &json!({}))?;
    job.audit("stage_run", "pipeline", None, Some(StageName::Takeoff), None);
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

/// Advance the pipeline: run the current stage and generate questions.
fn advance_pipeline(s: &mut Session, job: &mut GuidedJob) -> Result<()> {
    loop {
        let stage_name = match job.current_stage() {
            Some(st) if st.status == StageStatus::Paused => {
                // Still have unanswered questions — don't advance.
                if !st.unanswered().is_empty() {
                    return Ok(());
                }
                // All questions answered — mark done, move to next.
                let name = st.name;
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
                let (all_answered, unanswered_count) = {
                    let st = match job.stage_mut(stage_name) {
                        Some(st) => { st.result = Some(v); st }
                        None => continue,
                    };
                    let ua = st.unanswered().len();
                    (ua == 0, ua)
                };
                if all_answered {
                    if let Some(st) = job.stage_mut(stage_name) { st.status = StageStatus::Done; }
                    job.audit("stage_done", "pipeline", None, Some(stage_name), None);
                } else {
                    if let Some(st) = job.stage_mut(stage_name) { st.status = StageStatus::Paused; }
                    job.audit("stage_paused", "pipeline", None, Some(stage_name),
                        Some(json!({"unanswered": unanswered_count})));
                    return Ok(());
                }
            }
            Err(e) => {
                let msg = e.to_string();
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

    json!({
        "job_id": job.id,
        "goal": job.goal,
        "progress": job.progress(),
        "done": job.is_done(),
        "current_stage": job.current_stage().map(|s| s.name.label()),
        "stages": stages,
        "open_questions": job.stages.iter().map(|s| s.unanswered().len()).sum::<usize>(),
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
            .params("{} → {job_id, goal, progress, done, current_stage, stages, open_questions, rules_learned}")
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
        if let Some(st) = job.stage_mut(StageName::Levels) {
            if let Some(q) = st.questions.first_mut() {
                q.answer = Some(json!("Yes"));
                q.answered_by = Some("agent".into());
            }
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
