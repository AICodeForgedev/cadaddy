//! Where a project's documents come from, and asking for the latest (Q6618).
//!
//! A project names its document source (`project.source`, or a manifest's `source` block): a
//! construction-management platform, a shared drive, a plan room, email, or none. The kinds are
//! rule data (`docsource.json`); the platform's own name is the project's data. CADaddy never
//! signs in anywhere and makes no network call: the pull is done outside the program (an agent,
//! a connector or a script with the owner's own read-only access), and CADaddy ASKS for it, at
//! the right moments, and takes the answer:
//!
//! - a guided job asks at its start (and on resume) when the last pull or check is older than
//!   the project's age (default a day) or never happened, and again before its takeoff and
//!   estimate; the question names what to pull, for which project reference, changed since
//!   when, and how to hand it back (a folder with `manifest.json`, docs/project-manifest.md).
//!   `job.answer` takes `pulled` (with the manifest), `nothing newer` or `skip` (the default:
//!   an unattended job goes on and its report says the latest was not confirmed);
//! - a finding newer documents could answer (documents that disagree, a missing sheet or
//!   schedule, an unanswered callout, a product with no submittal) carries the request to
//!   check the source for a newer sheet, an ASI, an RFI response or a submittal, naming the
//!   sheets and specification sections involved;
//! - a pulled manifest is imported, says what changed, and re-runs only what that touches: the
//!   revised and new sheets are read again (the others keep their reading), a specification or
//!   submittal reruns the build; the findings the new documents answer are closed naming them;
//!   answers and corrections carry over;
//! - every job report, takeoff, estimate, bid and AR export answer carries a `documents` header:
//!   the issue it was built from and when the source was last checked.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::guided::{GuidedJob, Question, QuestionEvidence, QuestionKind, StageName, StageStatus};
use super::project::{DocSource, ProjectManifest, ProjectState};
use super::{CommandSpec, bad, str_param};
use crate::{Result, Session};

const RULES: &str = include_str!("docsource.json");

/// The command that reads and sets a project's source.
const CMD: &str = "project.source";
/// Most findings one pull keeps to check again.
const MAX_RECHECK: usize = 2_000;
/// Most closed findings and pulls a job keeps.
const MAX_KEPT: usize = 2_000;
/// Most sheets or sections a finding's request names.
const MAX_NAMED: usize = 40;
/// Most parent folders looked in for a job's project.
const MAX_PARENTS: usize = 4;
/// The age a source is asked again after when the rules give none (hours).
const DEFAULT_MAX_AGE_H: f64 = 24.0;
/// The longest age a project may set (ten years, in hours).
const MAX_AGE_LIMIT_H: f64 = 87_600.0;

/// The question every pull request starts with.
pub const PULL_QUESTION: &str = "Pull the latest documents";

// ─── Rules ──────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct KindRule {
    kind: String,
    label: String,
    asks: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PullRule {
    what: String,
    kinds: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct AnswerWords {
    pulled: Vec<String>,
    nothing_newer: Vec<String>,
    skip: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RereadKinds {
    drawings: Vec<String>,
    build: Vec<String>,
    noted: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Answerable {
    class: String,
    check: Vec<String>,
    words: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rules {
    kinds: Vec<KindRule>,
    aliases: BTreeMap<String, String>,
    max_age_hours: f64,
    pull: Vec<PullRule>,
    answers: AnswerWords,
    reread_kinds: RereadKinds,
    answerable: Vec<Answerable>,
    headers: Vec<String>,
    asks_before: Vec<String>,
}

fn rules() -> &'static Rules {
    static R: OnceLock<Rules> = OnceLock::new();
    R.get_or_init(|| serde_json::from_str(RULES).unwrap_or_default())
}

/// The source kinds the rules know.
pub fn kinds() -> Vec<&'static str> {
    rules().kinds.iter().map(|k| k.kind.as_str()).collect()
}

/// A kind as the rules spell it (any case, `-` or a space for `_`, or one of its aliases).
pub fn canonical_kind(raw: &str) -> Option<&'static str> {
    let t = raw.trim().to_lowercase();
    let under = t.replace(['-', ' '], "_");
    let r = rules();
    if let Some(k) = r.kinds.iter().find(|k| k.kind == t || k.kind == under) {
        return Some(k.kind.as_str());
    }
    let alias = r.aliases.get(&t).or_else(|| r.aliases.get(&t.replace(['-', '_'], " ")))?;
    r.kinds.iter().find(|k| &k.kind == alias).map(|k| k.kind.as_str())
}

fn kind_rule(kind: &str) -> Option<&'static KindRule> {
    let k = canonical_kind(kind)?;
    rules().kinds.iter().find(|r| r.kind == k)
}

/// Whether a source of this kind is asked for its latest (`none` never is).
fn asks(kind: &str) -> bool {
    kind_rule(kind).is_some_and(|k| k.asks)
}

/// The age a source is asked again after (hours): the project's, else the rules'.
fn max_age_h(src: &DocSource) -> f64 {
    let rule = rules().max_age_hours;
    let default = if rule.is_finite() && rule > 0.0 { rule } else { DEFAULT_MAX_AGE_H };
    src.max_age_hours.filter(|h| h.is_finite() && *h > 0.0).unwrap_or(default).min(MAX_AGE_LIMIT_H)
}

// ─── Times ──────────────────────────────────────────────────────────────────

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn digits(t: &str) -> Option<i64> {
    (!t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())).then(|| t.parse().ok()).flatten()
}

/// Seconds since 1970 of an ISO 8601 date (`2026-11-13`) or date and time
/// (`2026-11-13T09:30:00Z`, `2026-11-13 09:30`, `…+02:00`).
pub fn parse_iso(t: &str) -> Option<u64> {
    let t = t.trim();
    let date = t.get(..10)?;
    let mut parts = date.split('-');
    let (y, m, d) = (digits(parts.next()?)?, digits(parts.next()?)?, digits(parts.next()?)?);
    if parts.next().is_some() || !(1970..=9999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut secs = days_from_civil(y, m, d).checked_mul(86_400)?;
    let rest = t.get(10..)?;
    if !rest.is_empty() {
        let rest = rest.strip_prefix('T').or_else(|| rest.strip_prefix(' '))?;
        // The offset: `Z`, `+hh:mm` / `-hh:mm`, or none (UTC).
        let (clock, offset) = match rest.find(['Z', 'z', '+', '-']) {
            Some(i) => (rest.get(..i)?, rest.get(i..)?),
            None => (rest, ""),
        };
        let clock = clock.split('.').next()?;
        let mut hms = clock.split(':');
        let h = digits(hms.next()?)?;
        let mi = digits(hms.next()?)?;
        let s = hms.next().map_or(Some(0), digits)?;
        if hms.next().is_some() || h > 23 || mi > 59 || s > 60 {
            return None;
        }
        secs += h * 3600 + mi * 60 + s;
        if !offset.eq_ignore_ascii_case("z") && !offset.is_empty() {
            let sign = if offset.starts_with('-') { -1 } else { 1 };
            let mut oh = offset.get(1..)?.split(':');
            let (h, m) = (digits(oh.next()?)?, oh.next().map_or(Some(0), digits)?);
            if h > 23 || m > 59 {
                return None;
            }
            secs -= sign * (h * 3600 + m * 60);
        }
    }
    u64::try_from(secs).ok()
}

/// Now, in seconds since 1970.
pub fn now_secs() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The date of an ISO time, for plain words (`2026-11-13`).
fn day(t: &str) -> String {
    t.trim().get(..10).filter(|_| parse_iso(t).is_some()).unwrap_or(t.trim()).to_string()
}

// ─── Freshness ──────────────────────────────────────────────────────────────

/// How fresh a project's documents are.
#[derive(Clone, Debug, PartialEq)]
pub struct Fresh {
    /// The source is one that is asked (not `none`).
    pub asks: bool,
    /// The last pull or confirmation (ISO), if any.
    pub since: Option<String>,
    /// Hours since then.
    pub age_h: Option<f64>,
    /// The age it is asked again after (hours).
    pub max_h: f64,
    /// Older than that, or never pulled: ask.
    pub stale: bool,
}

/// How fresh `src` is at `now` (seconds since 1970).
pub fn freshness(src: &DocSource, now: u64) -> Fresh {
    let times = [src.last_pulled.as_str(), src.last_checked.as_str()];
    let latest = times.iter().filter_map(|t| parse_iso(t).map(|s| (s, *t))).max_by_key(|(s, _)| *s);
    let age_h = latest.map(|(s, _)| now.saturating_sub(s) as f64 / 3600.0);
    let max_h = max_age_h(src);
    let asks = asks(&src.kind);
    Fresh { asks, since: latest.map(|(_, t)| t.to_string()), age_h, max_h, stale: asks && age_h.is_none_or(|a| a > max_h) }
}

/// What a source is called in a request: its platform's name, else its kind's words.
fn source_name(src: &DocSource) -> String {
    if !src.platform.trim().is_empty() {
        return src.platform.trim().to_string();
    }
    kind_rule(&src.kind).map_or_else(|| src.kind.clone(), |k| k.label.clone())
}

/// A source as answers give it.
pub fn source_json(src: &DocSource) -> Value {
    let f = freshness(src, now_secs());
    json!({
        "kind": src.kind,
        "label": kind_rule(&src.kind).map(|k| k.label.as_str()),
        "platform": src.platform,
        "reference": src.reference,
        "maxAgeHours": f.max_h,
        "lastPulled": src.last_pulled,
        "lastChecked": src.last_checked,
        "issue": src.issue,
        "asks": f.asks,
        "stale": f.stale,
        "ageHours": f.age_h.map(|a| (a * 10.0).round() / 10.0),
    })
}

/// A manifest's `source` block taken into the project: its kind, platform and reference when
/// given, the pull's time and the issue it brought.
pub fn take_manifest_source(m: &mut ProjectManifest, x: &Value, pulled_at: &str, issue: &str) {
    let mut src = m.source.clone().unwrap_or_default();
    if let Some(k) = x.get("kind").and_then(Value::as_str).and_then(canonical_kind) {
        src.kind = k.to_string();
    }
    for (key, field) in [("platform", &mut src.platform), ("reference", &mut src.reference)] {
        if let Some(v) = x.get(key).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty()) {
            *field = v.chars().take(4096).collect();
        }
    }
    if let Some(h) = x.get("maxAgeHours").or_else(|| x.get("max_age_hours")).and_then(Value::as_f64).filter(|h| h.is_finite() && *h > 0.0) {
        src.max_age_hours = Some(h.min(MAX_AGE_LIMIT_H));
    }
    if !pulled_at.is_empty() {
        src.last_pulled = pulled_at.to_string();
    }
    let issue = x.get("issue").and_then(Value::as_str).filter(|t| !t.trim().is_empty()).unwrap_or(issue);
    if !issue.trim().is_empty() {
        src.issue = issue.trim().chars().take(4096).collect();
    }
    m.source = Some(src);
}

// ─── The request ────────────────────────────────────────────────────────────

/// What to pull and how to hand it back: the request a question or an answer carries.
pub fn request(src: &DocSource, dir: &str, moment: &str) -> Value {
    let f = freshness(src, now_secs());
    let what: Vec<&str> = rules().pull.iter().map(|p| p.what.as_str()).collect();
    let kinds: Vec<&str> = rules().pull.iter().flat_map(|p| p.kinds.iter().map(String::as_str)).collect();
    let manifest = format!("{}/manifest.json", dir.trim_end_matches('/'));
    json!({
        "moment": moment,
        "source": source_name(src),
        "kind": src.kind,
        "project_reference": src.reference,
        "what": what,
        "changed_since": f.since,
        "last_pulled": (!src.last_pulled.is_empty()).then_some(src.last_pulled.as_str()),
        "age_hours": f.age_h.map(|a| (a * 10.0).round() / 10.0),
        "ask_after_hours": f.max_h,
        "hand_back": {
            "folder": dir,
            "manifest": manifest,
            "schema": "docs/project-manifest.md",
            "kinds": kinds,
            "how": "put the files in the project folder and list each in manifest.json with its kind, revision, date issued, sourceId and pulled time; give the manifest a source block {kind, reference, pulled}",
        },
        "answers": {
            "pulled": format!("{{\"answer\": \"pulled\", \"manifest\": \"{manifest}\"}} (or \"pulled\": the project folder's manifest.json)"),
            "nothing newer": "the source holds nothing newer than what the project has",
            "skip": "go on with the documents as they are; the report says the latest is not confirmed",
        },
        "note": "CADaddy signs in nowhere and fetches nothing: pull with your own access, then answer",
    })
}

// ─── The project's source command ───────────────────────────────────────────

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new(CMD, "Document Source", run_source)
            .files()
            .params("{kind?: platform|shared_drive|plan_room|email|none (docsource.json), platform?: its name, reference?: the project reference the puller understands, max_age_hours?: ask again after (default 24)} → {source: {kind, label, platform, reference, maxAgeHours, lastPulled, lastChecked, issue, asks, stale, ageHours}, request?: what to pull and how to hand it back, when stale}: the open project's document source (Q6618); with no parameters it is read. CADaddy signs in nowhere: the pull is done outside it, the answer comes back as a manifest import or a job answer")
            .enabled(|s: &Session| if s.project.is_some() { Ok(()) } else { Err("no project open".into()) })
            .noundo(),
    ]
}

fn run_source(s: &mut Session, p: &Value) -> Result<Value> {
    let ps = s.project.as_mut().ok_or_else(|| bad(CMD, "no project open: project.open {path} first"))?;
    let setting = ["kind", "platform", "reference", "max_age_hours"].iter().any(|k| p.get(*k).is_some());
    if setting {
        let mut src = ps.manifest.source.clone().unwrap_or_default();
        match str_param(p, "kind") {
            Some(k) => {
                src.kind = canonical_kind(k)
                    .ok_or_else(|| bad(CMD, format!("kind `{}` is not one of {}", k.chars().take(60).collect::<String>(), kinds().join(", "))))?
                    .to_string();
            }
            None if src.kind.is_empty() => return Err(bad(CMD, format!("`kind` is required: one of {}", kinds().join(", ")))),
            None => {}
        }
        for (key, field) in [("platform", &mut src.platform), ("reference", &mut src.reference)] {
            match p.get(key) {
                None | Some(Value::Null) => {}
                Some(Value::String(t)) if t.len() <= 4096 => *field = t.trim().to_string(),
                Some(_) => return Err(bad(CMD, format!("`{key}` must be text of at most 4096 bytes"))),
            }
        }
        match p.get("max_age_hours") {
            None | Some(Value::Null) => {}
            Some(h) => match h.as_f64() {
                Some(h) if h.is_finite() && h > 0.0 && h <= MAX_AGE_LIMIT_H => src.max_age_hours = Some(h),
                _ => return Err(bad(CMD, format!("`max_age_hours` is a number of hours above 0 (at most {MAX_AGE_LIMIT_H})"))),
            },
        }
        ps.manifest.source = Some(src);
        ps.manifest.modified = super::project::timestamp();
        super::project::write_manifest(&ps.manifest)?;
    }
    let Some(src) = ps.manifest.source.as_ref() else {
        return Ok(json!({"source": null, "note": "no document source set: project.source {kind, platform?, reference?} sets one"}));
    };
    let mut out = json!({"source": source_json(src)});
    if freshness(src, now_secs()).stale {
        out["request"] = request(src, &ps.manifest.dir, "now");
    }
    Ok(out)
}

// ─── A job's project ────────────────────────────────────────────────────────

/// What a guided job keeps about its project's documents.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct JobDocuments {
    /// The project folder (its `project.json`).
    pub project_dir: String,
    /// The project's name.
    #[serde(default)]
    pub project: String,
    /// Findings a pull may answer: checked again when their stage has run again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recheck: Vec<Recheck>,
    /// Findings a pull's documents answered, each with the documents.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closed: Vec<Value>,
    /// Every pull the job took: what came in and what it re-ran.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pulls: Vec<Value>,
    /// Every pull request answered: when it was asked and the answer (kept when a re-run drops
    /// the question it was).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Value>,
}

/// A finding a pull may answer, waiting for its stage to run again.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recheck {
    pub description: String,
    pub stage: StageName,
    #[serde(default)]
    pub sheets: Vec<String>,
    #[serde(default)]
    pub levels: Vec<String>,
    /// The documents the pull brought (`path`, `title`, `kind`, `revision`, `change`).
    #[serde(default)]
    pub docs: Vec<Value>,
}

/// The project the job's prints belong to: the open project when they are in its folder, else
/// a `project.json` in the prints folder or the folders above it.
fn project_for(s: &Session, pdfs: &str) -> Option<ProjectManifest> {
    if let Some(ps) = s.project.as_ref()
        && (super::project::in_project(&ps.manifest.dir, pdfs) || same_dir(&ps.manifest.dir, pdfs))
    {
        return Some(ps.manifest.clone());
    }
    let start = std::path::Path::new(pdfs);
    let mut dir = if start.is_file() { start.parent() } else { Some(start) };
    for _ in 0..=MAX_PARENTS {
        let d = dir?;
        if d.join("project.json").is_file()
            && let Some(m) = d.to_str().and_then(|t| super::project::read_manifest(t).ok()).filter(|m| !m.shelved)
        {
            return Some(m);
        }
        dir = d.parent().filter(|p| !p.as_os_str().is_empty());
    }
    None
}

fn same_dir(a: &str, b: &str) -> bool {
    let c = |p: &str| std::fs::canonicalize(p).ok();
    a.trim_end_matches('/') == b.trim_end_matches('/') || c(a).is_some_and(|x| c(b).as_ref() == Some(&x))
}

/// The job's project as it is now (the open project's copy when it is that one).
fn manifest_of(s: Option<&Session>, docs: &JobDocuments) -> Option<ProjectManifest> {
    if let Some(ps) = s.and_then(|s| s.project.as_ref())
        && same_dir(&ps.manifest.dir, &docs.project_dir)
    {
        return Some(ps.manifest.clone());
    }
    super::project::read_manifest(&docs.project_dir).ok()
}

/// Change the job's project's source and write it (the open project's copy too).
fn update_source(s: &mut Session, docs: &JobDocuments, f: impl Fn(&mut DocSource)) -> Result<()> {
    if let Some(ps) = s.project.as_mut()
        && same_dir(&ps.manifest.dir, &docs.project_dir)
    {
        if let Some(src) = ps.manifest.source.as_mut() {
            f(src);
        }
        return super::project::write_manifest(&ps.manifest);
    }
    let mut m = super::project::read_manifest(&docs.project_dir)?;
    if let Some(src) = m.source.as_mut() {
        f(src);
    }
    super::project::write_manifest(&m)
}

/// Note the job's project (when its prints are in one): the job asks it for the latest and its
/// reports name the issue they were built from.
pub fn attach(s: &Session, job: &mut GuidedJob) {
    let Some(m) = project_for(s, &job.pdfs_path) else { return };
    match job.documents.as_mut() {
        Some(d) if same_dir(&d.project_dir, &m.dir) => d.project = m.name,
        _ => job.documents = Some(JobDocuments { project_dir: m.dir, project: m.name, ..Default::default() }),
    }
}

// ─── The job's question ─────────────────────────────────────────────────────

/// Whether `q` is a pull request.
pub fn is_pull_question(q: &Question) -> bool {
    q.question.starts_with(PULL_QUESTION)
}

/// Ask for the latest at `moment` (added to `at`'s questions) when the job's project has a source
/// that asks and its last pull or check is too old. Asked once per moment. Returns whether it asked.
pub fn ask_if_stale(s: Option<&Session>, job: &mut GuidedJob, at: StageName, moment: &str) -> bool {
    let Some(docs) = job.documents.as_ref() else { return false };
    let Some(m) = manifest_of(s, docs) else { return false };
    let Some(src) = m.source.as_ref() else { return false };
    let f = freshness(src, now_secs());
    if !f.stale {
        return false;
    }
    let text = format!(
        "{PULL_QUESTION} from {} ({}) {moment}? Answer 'pulled' once they are in the project folder with manifest.json, 'nothing newer', or 'skip'",
        source_name(src),
        if src.reference.trim().is_empty() { "no project reference set".to_string() } else { format!("project {}", src.reference.trim()) }
    );
    let asked = job.stages.iter().flat_map(|st| st.questions.iter()).any(|q| q.question == text);
    if asked {
        return false;
    }
    let why = match (&f.since, f.age_h) {
        (Some(t), Some(a)) => format!(
            "The documents were last pulled or checked on {} ({:.0} hours ago; this project asks again after {:.0}): a newer sheet, addendum, ASI, RFI response or approved submittal may change what is built",
            day(t),
            a,
            f.max_h
        ),
        _ => "The documents were never pulled from the project's source: a newer sheet, addendum, ASI, RFI response or approved submittal may change what is built".into(),
    };
    let q = Question {
        id: job.next_qid(at),
        stage: at,
        question: text,
        why,
        choices: vec!["pulled".into(), "nothing newer".into(), "skip".into()],
        evidence: Some(QuestionEvidence {
            crop_png_path: None,
            sheet: None,
            at: None,
            values: Some(request(src, &m.dir, moment)),
            handles: Vec::new(),
            tiled_crops: None,
            leftovers: Vec::new(),
        }),
        default: Some("skip".into()),
        answer_schema: Some(json!({"type": "object", "properties": {"answer": {"type": "string"}, "manifest": {"type": "string"}}})),
        answer: None,
        answered_by: None,
        answered_at: None,
        kind: QuestionKind::Doubt,
    };
    job.add_question_to_worklist(&q);
    if let Some(st) = job.stage_mut(at) {
        st.questions.push(q);
    }
    job.audit("pull_requested", "pipeline", None, Some(at), Some(json!({"moment": moment, "since": f.since})));
    true
}

/// A resumed job asks before it goes on: its last finished stage waits for the answer.
pub fn ask_on_resume(s: &Session, job: &mut GuidedJob) {
    let Some(last) = job.stages.iter().filter(|st| st.status == StageStatus::Done).map(|st| st.name).max() else { return };
    // Every stage before the first not done is done (the resume picks up there).
    if job.current_stage().is_some_and(|c| c.name < last) {
        return;
    }
    if ask_if_stale(Some(s), job, last, "before the job goes on (resumed)")
        && let Some(st) = job.stage_mut(last)
    {
        st.status = StageStatus::Paused;
    }
}

/// After a stage ran: its findings newer documents could answer carry the request, findings a
/// pull may have answered are checked again, and the moments to ask come (the job's start, before
/// its takeoff and estimate).
pub fn after_stage(job: &mut GuidedJob, stage: StageName) {
    if job.documents.is_none() {
        return;
    }
    mark_findings(job, stage);
    recheck(job, stage);
    let next = job.stages.iter().find(|st| st.name > stage && st.status != StageStatus::Skipped).map(|st| st.name);
    let estimate = job.stage(StageName::Estimate).is_some_and(|st| st.status != StageStatus::Skipped);
    match next {
        Some(StageName::Classify) if stage == StageName::Import => {
            ask_if_stale(None, job, stage, "at the job's start, before the sheets are read");
        }
        Some(StageName::Takeoff) => {
            let moment = if estimate { "before the takeoff and estimate" } else { "before the takeoff" };
            ask_if_stale(None, job, stage, moment);
        }
        Some(StageName::Estimate) => {
            ask_if_stale(None, job, stage, "before the estimate");
        }
        _ => {}
    }
}

// ─── Findings newer documents could answer ──────────────────────────────────

/// Whether `phrase` stands in `text` (both lower case) as whole words.
fn has_phrase(text: &str, phrase: &str) -> bool {
    if phrase.is_empty() {
        return false;
    }
    let edge = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric());
    let mut from = 0;
    while let Some(i) = text.get(from..).and_then(|t| t.find(phrase)).map(|i| i + from) {
        let before = text.get(..i).and_then(|t| t.chars().next_back());
        let after = text.get(i + phrase.len()..).and_then(|t| t.chars().next());
        if edge(before) && edge(after) {
            return true;
        }
        from = i + phrase.len().max(1);
        if from >= text.len() {
            break;
        }
    }
    false
}

/// The class of question a finding is, when newer documents could answer it: the class whose
/// matching words are the most specific (the longest phrase: "no submittal" over "have no").
fn answerable(description: &str, why: &str) -> Option<&'static Answerable> {
    let best = |text: &str| -> Option<&'static Answerable> {
        let t = text.to_lowercase();
        rules()
            .answerable
            .iter()
            .filter_map(|a| a.words.iter().filter(|p| has_phrase(&t, &p.to_lowercase())).map(String::len).max().map(|n| (n, a)))
            .max_by_key(|(n, _)| *n)
            .map(|(_, a)| a)
    };
    // Its own words first; its reason only when they say nothing.
    best(description).or_else(|| best(why))
}

/// A level as the index and the findings name it, for comparing (`Level 2`, `level 2`, `2`).
fn norm_level(l: &str) -> String {
    let t = l.trim().to_lowercase();
    t.strip_prefix("level").map_or(t.as_str(), str::trim).trim_matches(|c: char| !c.is_alphanumeric()).to_string()
}

/// The levels a text names (`Level 2`, `levels 1 and 2`).
fn levels_named(text: &str) -> BTreeSet<String> {
    let lower = text.to_lowercase();
    let mut out = BTreeSet::new();
    let mut from = 0;
    while let Some(i) = lower.get(from..).and_then(|t| t.find("level")).map(|i| i + from) {
        let rest = lower.get(i + 5..).unwrap_or("");
        let rest = rest.strip_prefix('s').unwrap_or(rest);
        let token: String = rest.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '-').collect();
        if !token.is_empty() && token.chars().any(|c| c.is_ascii_digit()) {
            out.insert(norm_level(&token));
        }
        from = i + 5;
        if out.len() >= MAX_NAMED {
            break;
        }
    }
    out
}

/// The specification sections a text names (`26 05 19`).
fn sections_named(text: &str) -> Vec<String> {
    let b = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let d = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    let sp = |i: usize| b.get(i) == Some(&b' ');
    let mut i = 0;
    while i + 8 <= b.len() {
        let starts = i == 0 || !b.get(i - 1).is_some_and(u8::is_ascii_alphanumeric);
        if starts && d(i) && d(i + 1) && sp(i + 2) && d(i + 3) && d(i + 4) && sp(i + 5) && d(i + 6) && d(i + 7) && !d(i + 8) {
            if let Some(t) = text.get(i..i + 8)
                && !out.iter().any(|o| o == t)
            {
                out.push(t.to_string());
            }
            i += 8;
        } else {
            i += 1;
        }
        if out.len() >= MAX_NAMED {
            break;
        }
    }
    out
}

/// The job's sheet index: (number, normalized level, its files).
fn index_sheets(job: &GuidedJob) -> Vec<(String, String, Vec<String>)> {
    let Some(sheets) = job.index.as_ref().and_then(|i| i.get("sheets")).and_then(Value::as_array) else { return Vec::new() };
    sheets
        .iter()
        .filter_map(|sh| {
            let number = sh.get("number").and_then(Value::as_str)?.to_string();
            let level = sh.get("level").and_then(Value::as_str).map(norm_level).unwrap_or_default();
            let files = sh
                .get("sources")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(|t| t.split('#').next().unwrap_or(t).to_string()).collect())
                .unwrap_or_default();
            Some((number, level, files))
        })
        .collect()
}

/// The sheets and levels a finding is about: the sheet its evidence names, the index's sheet
/// numbers its words name, and the sheets of the levels it names.
fn finding_scope(job: &GuidedJob, description: &str, why: &str, sheet: Option<&str>) -> (Vec<String>, Vec<String>) {
    let text = format!("{description} {why}");
    let upper = text.to_uppercase();
    let index = index_sheets(job);
    let levels = levels_named(description);
    let mut sheets: Vec<String> = sheet.map(str::to_string).into_iter().collect();
    for (number, level, _) in &index {
        let named = has_phrase(&upper, &number.to_uppercase());
        if (named || (!level.is_empty() && levels.contains(level))) && !sheets.contains(number) {
            sheets.push(number.clone());
        }
        if sheets.len() >= MAX_NAMED {
            break;
        }
    }
    (sheets, levels.into_iter().collect())
}

/// Every finding of `stage` that newer documents could answer carries the request to check the
/// source, naming its sheets and specification sections (once).
fn mark_findings(job: &mut GuidedJob, stage: StageName) {
    let source = job.documents.as_ref().map(|d| d.project.clone()).unwrap_or_default();
    let mut marked: Vec<(String, String)> = Vec::new();
    let pending: Vec<(usize, String, String, Option<String>)> = job
        .findings
        .iter()
        .enumerate()
        .filter(|(_, f)| f.stage == stage && f.check_source.is_none() && f.severity >= 2 && !f.blocking)
        .map(|(i, f)| (i, f.description.clone(), f.why.clone(), f.evidence.as_ref().and_then(|e| e.sheet.clone())))
        .collect();
    for (i, description, why, sheet) in pending {
        let Some(class) = answerable(&description, &why) else { continue };
        let (sheets, levels) = finding_scope(job, &description, &why, sheet.as_deref());
        let sections = sections_named(&format!("{description} {why}"));
        let check = class.check.join(", ");
        let mut action = format!("Check the project's source for {check}");
        if !sheets.is_empty() {
            action.push_str(&format!(" (sheets {})", sheets.join(", ")));
        }
        if !sections.is_empty() {
            action.push_str(&format!(" (sections {})", sections.join(", ")));
        }
        if let Some(f) = job.findings.get_mut(i) {
            f.check_source = Some(json!({
                "request": "check the source for a newer sheet / ASI / RFI response / submittal",
                "class": class.class,
                "check": class.check,
                "sheets": sheets,
                "levels": levels,
                "spec_sections": sections,
                "project": source,
            }));
            if !f.actions.contains(&action) {
                f.actions.push(action.clone());
            }
            marked.push((f.id.clone(), action));
        }
    }
    for (id, action) in marked {
        if let Some(w) = job.worklist.iter_mut().find(|w| w.id == id)
            && !w.choices.contains(&action)
        {
            w.choices.push(action);
        }
    }
}

/// What a finding is, without what it counts: its words before its list (the first `:`), a
/// leading count left out. `3 devices …` and `12 devices …` are one finding; `Level 2 …` and
/// `Level 3 …` are two; `No panel schedule read: 1 sheets searched (Z-7)` is the same finding
/// with two sheets searched.
fn finding_key(description: &str) -> String {
    let head = description.split(':').next().unwrap_or(description).trim();
    let rest = head.trim_start_matches(|c: char| c.is_ascii_digit());
    if rest.len() == head.len() { head.to_string() } else { format!("#{rest}") }
}

/// The sheets and levels a pulled document is in the index (by its file).
fn doc_sheets(index: &[(String, String, Vec<String>)], path: &str) -> (Vec<String>, Vec<String>) {
    let mut sheets = Vec::new();
    let mut levels = Vec::new();
    for (number, level, files) in index {
        if files.iter().any(|f| !f.is_empty() && (path.ends_with(f.as_str()) || f.ends_with(path))) {
            sheets.push(number.clone());
            if !level.is_empty() {
                levels.push(level.clone());
            }
        }
    }
    (sheets, levels)
}

/// The findings a pull may answer whose stage ran again: not raised again ⟹ closed, naming the
/// documents that answered it; raised again ⟹ said on the finding.
fn recheck(job: &mut GuidedJob, stage: StageName) {
    let Some(docs) = job.documents.as_mut() else { return };
    let (mine, rest): (Vec<Recheck>, Vec<Recheck>) = std::mem::take(&mut docs.recheck).into_iter().partition(|r| r.stage == stage);
    docs.recheck = rest;
    if mine.is_empty() {
        return;
    }
    let index = index_sheets(job);
    let when = super::project::timestamp();
    let mut closed = Vec::new();
    for r in mine {
        let key = finding_key(&r.description);
        let still = job.findings.iter_mut().find(|f| f.stage == stage && finding_key(&f.description) == key);
        let names: Vec<String> = r.docs.iter().map(doc_label).collect();
        if let Some(f) = still {
            let note = format!("still raised after the pull of {}", names.join(", "));
            match f.check_source.as_mut() {
                Some(c) => c["after_pull"] = json!(note),
                None => f.check_source = Some(json!({"after_pull": note})),
            }
            continue;
        }
        // The documents that answered it: those on its sheets or levels, else the drawings the
        // pull brought, else all it brought.
        let scoped: Vec<Value> = r
            .docs
            .iter()
            .filter_map(|d| {
                let path = d.get("path").and_then(Value::as_str).unwrap_or("");
                let (sheets, levels) = doc_sheets(&index, path);
                let links: Vec<&str> = d
                    .get("links")
                    .and_then(Value::as_object)
                    .map(|o| o.values().filter_map(Value::as_array).flatten().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                let related = sheets.iter().any(|s| r.sheets.contains(s))
                    || levels.iter().any(|l| r.levels.contains(l))
                    || links.iter().any(|l| r.sheets.iter().any(|s| s.eq_ignore_ascii_case(l)));
                related.then(|| with_sheets(d, &sheets))
            })
            .collect();
        let by = if !scoped.is_empty() {
            scoped
        } else {
            let drawings: Vec<Value> = r
                .docs
                .iter()
                .filter(|d| rules().reread_kinds.drawings.iter().any(|k| d.get("kind").and_then(Value::as_str) == Some(k.as_str())))
                .map(|d| with_sheets(d, &doc_sheets(&index, d.get("path").and_then(Value::as_str).unwrap_or("")).0))
                .collect();
            if drawings.is_empty() { r.docs.clone() } else { drawings }
        };
        let named: Vec<String> = by.iter().map(doc_label).collect();
        closed.push(json!({
            "finding": r.description,
            "stage": r.stage,
            "closed_by": by,
            "line": format!("answered by {}", named.join(", ")),
            "at": when,
        }));
    }
    if let Some(docs) = job.documents.as_mut() {
        for c in closed {
            if docs.closed.len() < MAX_KEPT {
                docs.closed.push(c);
            }
        }
    }
}

/// A pulled document with the sheet numbers it is in the index.
fn with_sheets(d: &Value, sheets: &[String]) -> Value {
    let mut d = d.clone();
    if !sheets.is_empty() {
        d["sheets"] = json!(sheets);
    }
    d
}

/// A document in words: `Z-7 rev 2 (drawings/Z-7.pdf)`.
fn doc_label(d: &Value) -> String {
    let path = d.get("path").and_then(Value::as_str).unwrap_or("?");
    let name = d
        .get("sheets")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("/"))
        .filter(|t| !t.is_empty())
        .or_else(|| d.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string))
        .unwrap_or_else(|| path.to_string());
    match d.get("revision").and_then(Value::as_str).filter(|r| !r.is_empty()) {
        Some(r) => format!("{name} rev {r} ({path})"),
        None => format!("{name} ({path})"),
    }
}

// ─── The answer ─────────────────────────────────────────────────────────────

/// What a pull question's answer says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PullAnswer {
    /// Pulled: the manifest (a path, or the project folder's).
    Pulled(Option<String>),
    NothingNewer,
    Skip,
}

/// Read an answer: text (`pulled`, `pulled: <manifest>`, `nothing newer`, `skip`) or
/// `{answer, manifest?}`.
pub fn read_answer(answer: &Value) -> Option<PullAnswer> {
    let (text, manifest) = match answer {
        Value::String(t) => (t.trim().to_string(), None),
        Value::Object(o) => (
            o.get("answer").and_then(Value::as_str).unwrap_or(if o.contains_key("manifest") { "pulled" } else { "" }).trim().to_string(),
            o.get("manifest").and_then(Value::as_str).map(str::trim).filter(|m| !m.is_empty()).map(str::to_string),
        ),
        _ => return None,
    };
    let lower = text.to_lowercase();
    let words = &rules().answers;
    let is = |list: &[String]| list.contains(&lower);
    if is(&words.nothing_newer) {
        return Some(PullAnswer::NothingNewer);
    }
    if is(&words.skip) {
        return Some(PullAnswer::Skip);
    }
    if is(&words.pulled) {
        return Some(PullAnswer::Pulled(manifest));
    }
    // `pulled: <manifest>` / `pulled <manifest>`
    let pulled = words.pulled.iter().find(|w| lower.starts_with(w.as_str()))?;
    let rest = text.get(pulled.len()..)?.trim_start_matches([':', ' ']).trim();
    (!rest.is_empty()).then(|| PullAnswer::Pulled(Some(manifest.unwrap_or_else(|| rest.to_string()))))
}

/// The manifest a `pulled` answer points at (relative to the project folder; none: its own).
fn manifest_path(docs: &JobDocuments, given: Option<&str>) -> String {
    let dir = docs.project_dir.trim_end_matches('/');
    match given {
        None => format!("{dir}/manifest.json"),
        Some(p) if p.starts_with('/') || p.contains(':') => p.to_string(),
        Some(p) => format!("{dir}/{p}"),
    }
}

/// Whether an answer fits a pull question: one of its words, and a pulled manifest that is there.
pub fn check_answer(job: &GuidedJob, q: &Question, answer: &Value) -> std::result::Result<(), String> {
    let id = q.id.as_str();
    match read_answer(answer) {
        None => Err(format!("question {id}: answer 'pulled' (with the manifest), 'nothing newer' or 'skip'")),
        Some(PullAnswer::Pulled(given)) => {
            let Some(docs) = job.documents.as_ref() else { return Err(format!("question {id}: the job has no project")) };
            let path = manifest_path(docs, given.as_deref());
            let file = if path.ends_with("manifest.json") { path.clone() } else { format!("{}/manifest.json", path.trim_end_matches('/')) };
            if std::path::Path::new(&file).is_file() {
                Ok(())
            } else {
                Err(format!(
                    "question {id}: no manifest at {file}: put the pulled files and their manifest.json in the project folder (docs/project-manifest.md), or give its path"
                ))
            }
        }
        Some(_) => Ok(()),
    }
}

/// What taking an answer does to the job.
#[derive(Debug, Default)]
pub struct Taken {
    /// The first stage it runs again.
    pub rerun_from: Option<StageName>,
    pub effect: String,
    /// The print files changed: the job's record of them is made again.
    pub prints_changed: bool,
    /// The answer could not be taken (a manifest with errors): the question stays open.
    pub reopen: bool,
}

/// Take a pull question's answer. `pulled` imports the manifest into the project, says what
/// changed and re-runs only what that touches: revised and new sheets are read again (the
/// others keep their reading) from classify on; a specification or submittal re-runs the build;
/// RFIs, ASIs and the like are listed on the findings they speak to. `nothing newer` notes the
/// check on the project; `skip` goes on, the report saying the latest is not confirmed.
pub fn take_answer(s: &mut Session, job: &mut GuidedJob, q: &Question) -> Result<Taken> {
    let Some(docs) = job.documents.clone() else { return Ok(Taken { effect: "recorded: the job has no project".into(), ..Default::default() }) };
    let answer = q.answer.clone().unwrap_or(Value::Null);
    record_check(job, q);
    match read_answer(&answer) {
        None => Ok(Taken { effect: "recorded; answer 'pulled', 'nothing newer' or 'skip'".into(), reopen: true, ..Default::default() }),
        Some(PullAnswer::Skip) => {
            let since = manifest_of(Some(s), &docs).and_then(|m| m.source).map(|src| freshness(&src, now_secs()).since);
            let when = since.flatten().map_or_else(|| "never pulled".to_string(), |t| format!("last pulled on {}", day(&t)));
            Ok(Taken {
                effect: format!("the job goes on with the documents {when}; its report says the latest is not confirmed"),
                ..Default::default()
            })
        }
        Some(PullAnswer::NothingNewer) => {
            let now = super::project::timestamp();
            update_source(s, &docs, |src| src.last_checked = now.clone())?;
            Ok(Taken { effect: format!("noted: the source holds nothing newer (checked {}); the job goes on", day(&now)), ..Default::default() })
        }
        Some(PullAnswer::Pulled(given)) => take_pull(s, job, &docs, &manifest_path(&docs, given.as_deref())),
    }
}

fn take_pull(s: &mut Session, job: &mut GuidedJob, docs: &JobDocuments, manifest: &str) -> Result<Taken> {
    // The import goes into the project as the session holds it (the open one), else as its folder does.
    let open = s.project.as_ref().is_some_and(|ps| same_dir(&ps.manifest.dir, &docs.project_dir));
    let mut state = match s.project.as_ref() {
        Some(ps) if open => ps.clone(),
        _ => ProjectState { manifest: super::project::read_manifest(&docs.project_dir)?, active_drawing: None },
    };
    let before: BTreeMap<String, super::project::ProjectDrawing> = state.manifest.drawings.iter().map(|d| (d.path.clone(), d.clone())).collect();
    let imported = super::project::import_manifest_into(&mut state, manifest)?;
    if imported.get("valid") == Some(&Value::Bool(false)) {
        let errors: Vec<&str> =
            imported.get("errors").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).take(10).collect()).unwrap_or_default();
        return Ok(Taken {
            effect: format!("the manifest {manifest} has errors, nothing was imported: {}", errors.join("; ")),
            reopen: true,
            ..Default::default()
        });
    }
    // A manifest with no source block: the answer is the pull, now.
    let now = super::project::timestamp();
    if let Some(src) = state.manifest.source.as_mut() {
        if src.last_pulled.is_empty() || imported.pointer("/project/source/lastPulled").is_none() {
            src.last_pulled.clone_from(&now);
        }
        src.last_checked.clone_from(&now);
        super::project::write_manifest(&state.manifest)?;
    }
    let after: BTreeMap<String, super::project::ProjectDrawing> = state.manifest.drawings.iter().map(|d| (d.path.clone(), d.clone())).collect();
    if open && let Some(ps) = s.project.as_mut() {
        ps.manifest = state.manifest.clone();
    }

    // What came in, by document.
    let files = imported.get("files").cloned().unwrap_or(Value::Null);
    let list = |k: &str| -> Vec<String> {
        files.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
    };
    let changed: Vec<(String, Vec<String>)> = files
        .get("changed")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let path = c.get("path").and_then(Value::as_str)?.to_string();
                    let fields = c
                        .get("fields")
                        .and_then(Value::as_array)
                        .map(|f| f.iter().filter_map(Value::as_str).map(str::to_string).collect())
                        .unwrap_or_default();
                    Some((path, fields))
                })
                .collect()
        })
        .unwrap_or_default();
    let doc = |path: &str, change: &str, fields: &[String]| -> Value {
        let d = after.get(path).or_else(|| before.get(path));
        let was = before.get(path).map(|b| b.revision.clone()).filter(|r| !r.is_empty());
        json!({
            "path": path,
            "change": change,
            "fields": fields,
            "kind": d.map(|d| d.kind.as_str()),
            "title": d.map(|d| d.title.as_str()),
            "revision": d.map(|d| d.revision.as_str()),
            "was_revision": if change == "changed" { was } else { None },
            "date": d.map(|d| d.date.as_str()),
            "status": d.map(|d| d.review_status.as_str()).filter(|t| !t.is_empty()),
            "links": d.map(|d| json!(d.links)),
        })
    };
    let mut pulled: Vec<Value> = list("added").iter().map(|p| doc(p, "added", &[])).collect();
    pulled.extend(changed.iter().map(|(p, f)| doc(p, "changed", f)));
    pulled.extend(list("removed").iter().map(|p| doc(p, "removed", &[])));
    let kind_of = |d: &Value| d.get("kind").and_then(Value::as_str).unwrap_or("drawing").to_string();
    let in_list = |list: &[String], k: &str| list.iter().any(|x| x == k);
    let rk = &rules().reread_kinds;
    // A drawing whose bytes changed or that is new (or gone) is read again; a field alone
    // (a revision label, a status) is noted.
    let content = |d: &Value| {
        d.get("change").and_then(Value::as_str) != Some("changed")
            || d.get("fields").and_then(Value::as_array).is_some_and(|f| f.iter().any(|x| x == "content"))
    };
    let drawings: Vec<&Value> = pulled.iter().filter(|d| in_list(&rk.drawings, &kind_of(d)) && content(d)).collect();
    let build: Vec<&Value> = pulled.iter().filter(|d| in_list(&rk.build, &kind_of(d)) && content(d)).collect();
    let noted: Vec<&Value> = pulled.iter().filter(|d| !drawings.iter().chain(build.iter()).any(|x| std::ptr::eq(*x, *d))).collect();

    // The job's prints are the project's drawings under its prints folder.
    let prints = &job.pdfs_path;
    let prints_dir = if std::path::Path::new(prints).is_file() {
        std::path::Path::new(prints).parent().and_then(|p| p.to_str()).unwrap_or(prints).to_string()
    } else {
        prints.clone()
    };
    let under_prints = |d: &Value| {
        let path = d.get("path").and_then(Value::as_str).unwrap_or("");
        let abs = format!("{}/{path}", docs.project_dir.trim_end_matches('/'));
        super::project::in_project(&prints_dir, &abs) || same_dir(&prints_dir, &docs.project_dir) && !path.contains('/')
    };
    let sheet_files: Vec<&Value> = drawings.iter().copied().filter(|d| under_prints(d)).collect();
    let mut rerun_from = None;
    let mut reread = Value::Null;
    let mut prints_changed = false;
    if !sheet_files.is_empty() {
        // Only the sheets whose files changed are read again; the rest keep their reading.
        let label = state.manifest.source.as_ref().map(|s| s.issue.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| "pulled".into());
        let delta = match s.execute("printset.reimport", &json!({"path": prints_dir, "revision": label, "full_set": false})) {
            Ok(d) => d,
            // No print set held (a job resumed in another session): opened again, read whole.
            Err(_) => {
                s.execute("printset.open", &json!({"path": prints}))?;
                json!({"delta": {"note": "the print set was opened again: every sheet is read"}})
            }
        };
        reread = delta.get("delta").cloned().unwrap_or(Value::Null);
        prints_changed = true;
        job.reimport = job.sheets_loaded_flag();
        rerun_from = Some(StageName::Classify);
    }
    if !build.is_empty() && job.stage(StageName::BimBuild).is_some_and(|st| !matches!(st.status, StageStatus::Pending | StageStatus::Skipped)) {
        rerun_from = Some(rerun_from.map_or(StageName::BimBuild, |r: StageName| r.min(StageName::BimBuild)));
    }

    // The findings newer documents could answer, checked again when their stage runs again.
    let mut recheck_n = 0;
    if let Some(from) = rerun_from {
        let mut rc: Vec<Recheck> = job
            .findings
            .iter()
            .filter(|f| f.stage >= from)
            .filter_map(|f| {
                let c = f.check_source.as_ref()?;
                let strings = |k: &str| -> Vec<String> {
                    c.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
                };
                Some(Recheck {
                    description: f.description.clone(),
                    stage: f.stage,
                    sheets: strings("sheets"),
                    levels: strings("levels"),
                    docs: pulled.clone(),
                })
            })
            .take(MAX_RECHECK)
            .collect();
        recheck_n = rc.len();
        if let Some(d) = job.documents.as_mut() {
            d.recheck.retain(|r| r.stage < from);
            d.recheck.append(&mut rc);
        }
    }
    // Documents that are read by people (RFIs, ASIs …) go on the findings whose sheets they name.
    let mut noted_on = 0;
    for d in &noted {
        let links: Vec<String> = d
            .get("links")
            .and_then(Value::as_object)
            .map(|o| o.values().filter_map(Value::as_array).flatten().filter_map(Value::as_str).map(str::to_uppercase).collect())
            .unwrap_or_default();
        if links.is_empty() {
            continue;
        }
        for f in job.findings.iter_mut() {
            let Some(c) = f.check_source.as_mut() else { continue };
            let sheets: Vec<String> = c
                .get("sheets")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_uppercase).collect())
                .unwrap_or_default();
            if sheets.iter().any(|s| links.contains(s)) {
                let entry = json!(doc_label(d));
                match c.get_mut("newer_documents").and_then(Value::as_array_mut) {
                    Some(a) if !a.contains(&entry) => a.push(entry),
                    Some(_) => {}
                    None => c["newer_documents"] = json!([entry]),
                }
                noted_on += 1;
            }
        }
    }

    let names = |v: &[&Value]| -> Vec<String> { v.iter().map(|d| doc_label(d)).collect() };
    let mut effect = format!("pulled {}: {} added, {} changed, {} removed", manifest, list("added").len(), changed.len(), list("removed").len());
    if !sheet_files.is_empty() {
        effect.push_str(&format!("; the sheets read again: {} (the others keep their reading), from classify on", names(&sheet_files).join(", ")));
    }
    if !build.is_empty() {
        effect.push_str(&format!("; the build reads again: {}", names(&build).join(", ")));
    }
    if !noted.is_empty() {
        effect.push_str(&format!("; to read: {}", names(&noted).join(", ")));
    }
    if rerun_from.is_none() {
        effect.push_str("; nothing to read again");
    }
    let record = json!({
        "at": now,
        "manifest": manifest,
        "issue": state.manifest.source.as_ref().map(|s| s.issue.clone()),
        "documents": pulled,
        "reread": {"sheets": names(&sheet_files), "build": names(&build), "printset": reread},
        "noted": names(&noted),
        "noted_on_findings": noted_on,
        "rerun_from": rerun_from.map(StageName::label_str),
        "findings_to_recheck": recheck_n,
        "missing": list("missing"),
    });
    if let Some(d) = job.documents.as_mut()
        && d.pulls.len() < MAX_KEPT
    {
        d.pulls.push(record);
    }
    job.audit(
        "pull_taken",
        "agent",
        Some(&q_id(job, manifest)),
        rerun_from,
        Some(json!({"manifest": manifest, "rerun_from": rerun_from.map(StageName::label_str)})),
    );
    Ok(Taken { rerun_from, effect, prints_changed, reopen: false })
}

/// The audit names the pull by its manifest.
fn q_id(_job: &GuidedJob, manifest: &str) -> String {
    format!("pull:{}", manifest.chars().take(200).collect::<String>())
}

// ─── The header ─────────────────────────────────────────────────────────────

/// A pull question as the report lists it: when it was asked and how it was answered.
fn check_value(q: &Question) -> Value {
    let moment = q.evidence.as_ref().and_then(|e| e.values.as_ref()).and_then(|v| v.get("moment")).cloned().unwrap_or(Value::Null);
    let answer = q.answer.as_ref().and_then(read_answer).map(|a| match a {
        PullAnswer::Pulled(_) => "pulled",
        PullAnswer::NothingNewer => "nothing newer",
        PullAnswer::Skip => "skip",
    });
    json!({"id": q.id, "moment": moment, "answer": answer, "by": q.answered_by, "at": q.answered_at})
}

/// Keep an answered pull question in the job's record (its stage may run again without it).
fn record_check(job: &mut GuidedJob, q: &Question) {
    let v = check_value(q);
    if let Some(d) = job.documents.as_mut() {
        d.checks.retain(|c| c.get("id") != Some(&json!(q.id)));
        if d.checks.len() < MAX_KEPT {
            d.checks.push(v);
        }
    }
}

/// The job's pull requests: those answered (kept), and those its stages hold (defaults taken,
/// open ones).
fn checks(job: &GuidedJob) -> Vec<Value> {
    let mut out: Vec<Value> = job.documents.as_ref().map(|d| d.checks.clone()).unwrap_or_default();
    for q in job.stages.iter().flat_map(|st| st.questions.iter()).filter(|q| is_pull_question(q)) {
        if !out.iter().any(|c| c.get("id") == Some(&json!(q.id))) {
            out.push(check_value(q));
        }
    }
    out
}

/// Which document issue an answer was built from and when the source was last checked, with a
/// line in plain words (the job's own, or the open project's).
pub fn header(s: Option<&Session>, job: Option<&GuidedJob>) -> Option<Value> {
    let docs = job.and_then(|j| j.documents.as_ref());
    let m = match docs {
        Some(d) => manifest_of(s, d),
        None => s.and_then(|s| s.project.as_ref()).map(|ps| ps.manifest.clone()),
    };
    let Some(m) = m else {
        let j = job?;
        return Some(json!({
            "project": null,
            "latest_confirmed": false,
            "line": format!("built from the files in {}; no project names a document source", j.pdfs_path),
        }));
    };
    let mut out = json!({"project": m.name});
    match m.source.as_ref() {
        Some(src) if asks(&src.kind) => {
            let f = freshness(src, now_secs());
            let pulled = (!src.last_pulled.is_empty()).then(|| day(&src.last_pulled));
            let issue = (!src.issue.is_empty()).then(|| src.issue.clone());
            let confirmed = !f.stale;
            let issue_words = issue.as_ref().map_or_else(String::new, |i| format!(" (issue {i})"));
            let line = match (&pulled, confirmed) {
                (Some(p), true) => format!(
                    "built from documents pulled on {p}{issue_words}; the source was last checked on {}",
                    f.since.as_deref().map_or_else(|| p.clone(), day)
                ),
                (Some(p), false) => format!("built from documents last pulled on {p}{issue_words}; latest not confirmed"),
                (None, true) => format!(
                    "built from the project's documents{issue_words}; the source was last checked on {}",
                    f.since.as_deref().map_or_else(String::new, day)
                ),
                (None, false) => {
                    format!("built from the project's documents{issue_words}, never pulled from {}; latest not confirmed", source_name(src))
                }
            };
            out["source"] = source_json(src);
            out["issue"] = json!(issue);
            out["last_pulled"] = json!(src.last_pulled);
            out["last_checked"] = json!(f.since);
            out["latest_confirmed"] = json!(confirmed);
            out["line"] = json!(line);
        }
        other => {
            out["source"] = other.map_or(Value::Null, source_json);
            out["latest_confirmed"] = Value::Null;
            out["line"] = json!("built from the documents in the project folder; the project names no source to pull from");
        }
    }
    if let Some(j) = job {
        let c = checks(j);
        if !c.is_empty() {
            out["checks"] = json!(c);
        }
        if let Some(d) = docs {
            if !d.pulls.is_empty() {
                out["pulls"] = json!(d.pulls);
            }
            if !d.closed.is_empty() {
                out["closed_findings"] = json!(d.closed);
            }
        }
        let to_check = j.findings.iter().filter(|f| f.check_source.is_some()).count();
        out["findings_to_check_at_source"] = json!(to_check);
    }
    Some(out)
}

/// Every takeoff, estimate, bid, AR export and briefing answer names the documents it was built
/// from (`headers` in the rules); one produced on stale documents carries the request to pull.
pub(crate) fn tag_documents(s: &Session, id: &str, mut v: Value) -> Value {
    let r = rules();
    if !r.headers.iter().any(|h| h == id) || !v.is_object() || v.get("documents").is_some() {
        return v;
    }
    let job = super::guided::load_job(s).ok().flatten();
    let Some(head) = header(Some(s), job.as_ref()) else { return v };
    let stale = head.get("latest_confirmed") == Some(&Value::Bool(false)) && head.get("source").is_some_and(Value::is_object);
    if let Some(o) = v.as_object_mut() {
        o.insert("documents".into(), head);
        if stale && r.asks_before.iter().any(|a| a == id) {
            let m = match job.as_ref().and_then(|j| j.documents.as_ref()) {
                Some(d) => manifest_of(Some(s), d),
                None => s.project.as_ref().map(|ps| ps.manifest.clone()),
            };
            if let Some(m) = m
                && let Some(src) = m.source.as_ref()
            {
                o.insert("pull_request".into(), request(src, &m.dir, &format!("before {id}")));
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_read_in_the_iso_forms() {
        assert_eq!(parse_iso("1970-01-02"), Some(86_400));
        assert_eq!(parse_iso("2026-11-13T00:00:00Z"), Some(days_from_civil(2026, 11, 13) as u64 * 86_400));
        assert_eq!(parse_iso("1970-01-01T01:30:00+01:00"), Some(1800));
        assert_eq!(parse_iso("1970-01-01 02:00"), Some(7200));
        assert_eq!(parse_iso("1970-01-01T00:00:10.5Z"), Some(10));
        for bad in ["", "2026-13-01", "2026-11-13T25:00", "yesterday", "2026-11-13Tnoon", "1969-12-31"] {
            assert_eq!(parse_iso(bad), None, "{bad}");
        }
        let now = super::super::project::iso8601(1_800_000_000);
        assert_eq!(parse_iso(&now), Some(1_800_000_000));
    }

    #[test]
    fn kinds_come_from_the_rules_with_their_aliases() {
        assert_eq!(canonical_kind("Platform"), Some("platform"));
        assert_eq!(canonical_kind("shared drive"), Some("shared_drive"));
        assert_eq!(canonical_kind("plan-room"), Some("plan_room"));
        assert_eq!(canonical_kind("construction management"), Some("platform"));
        assert_eq!(canonical_kind("NONE"), Some("none"));
        assert_eq!(canonical_kind("ftp"), None);
        assert!(!asks("none") && asks("platform"));
    }

    #[test]
    fn a_source_is_stale_past_its_age_or_never_pulled_and_none_never_is() {
        let now = 1_800_000_000;
        let hours_ago = |h: u64| super::super::project::iso8601(now - h * 3600);
        let src = |kind: &str, pulled: String| DocSource { kind: kind.into(), last_pulled: pulled, ..Default::default() };
        assert!(freshness(&src("platform", hours_ago(48)), now).stale);
        assert!(!freshness(&src("platform", hours_ago(2)), now).stale);
        assert!(freshness(&src("platform", String::new()), now).stale);
        assert!(!freshness(&src("none", String::new()), now).stale);
        // A confirmation since counts as fresh; the project's own age wins over the rules'.
        let mut checked = src("platform", hours_ago(48));
        checked.last_checked = hours_ago(1);
        assert!(!freshness(&checked, now).stale);
        let mut weekly = src("shared_drive", hours_ago(48));
        weekly.max_age_hours = Some(168.0);
        assert!(!freshness(&weekly, now).stale);
    }

    #[test]
    fn answers_are_read_as_text_or_an_object() {
        assert_eq!(read_answer(&json!("skip")), Some(PullAnswer::Skip));
        assert_eq!(read_answer(&json!("Nothing newer")), Some(PullAnswer::NothingNewer));
        assert_eq!(read_answer(&json!("pulled")), Some(PullAnswer::Pulled(None)));
        assert_eq!(read_answer(&json!("pulled: issue2/manifest.json")), Some(PullAnswer::Pulled(Some("issue2/manifest.json".into()))));
        assert_eq!(
            read_answer(&json!({"answer": "pulled", "manifest": "/x/manifest.json"})),
            Some(PullAnswer::Pulled(Some("/x/manifest.json".into())))
        );
        assert_eq!(read_answer(&json!({"manifest": "/x"})), Some(PullAnswer::Pulled(Some("/x".into()))));
        assert_eq!(read_answer(&json!("maybe")), None);
        assert_eq!(read_answer(&json!(3)), None);
    }

    #[test]
    fn findings_newer_documents_could_answer_are_told_apart_by_their_words() {
        assert_eq!(answerable("Level 2 has no electrical (E) plan", "").map(|a| a.class.as_str()), Some("missing sheet or schedule"));
        assert_eq!(answerable("3 device types have no submittal product: X1", "").map(|a| a.class.as_str()), Some("product with no submittal"));
        assert_eq!(
            answerable("Sprinkler heads drawn differ from the head table on 2 page(s)", "").map(|a| a.class.as_str()),
            Some("documents disagree")
        );
        assert_eq!(answerable("4 devices in no room", "").map(|a| a.class.as_str()), None);
        assert_eq!(answerable("Building spec draft failed: io", "").map(|a| a.class.as_str()), None);
        // whole words only
        assert!(!has_phrase("the differential", "differ"));
        assert!(has_phrase("they differ.", "differ"));
    }

    #[test]
    fn levels_sections_and_counts_are_read_from_a_findings_words() {
        assert_eq!(levels_named("Level 2 has no electrical plan").into_iter().collect::<Vec<_>>(), vec!["2".to_string()]);
        assert_eq!(levels_named("levels 3 and roof").into_iter().collect::<Vec<_>>(), vec!["3".to_string()]);
        assert_eq!(sections_named("see 26 05 19 and 26 51 00, not 126 05 190"), vec!["26 05 19".to_string(), "26 51 00".to_string()]);
        assert_eq!(finding_key("3 device(s) have no circuit"), finding_key("12 device(s) have no circuit"));
        assert_ne!(finding_key("Level 2 has no electrical (E) plan"), finding_key("Level 3 has no electrical (E) plan"));
        assert_eq!(
            finding_key("No panel schedule read: 1 sheets searched (Z-7)"),
            finding_key("No panel schedule read: 2 sheets searched (Z-7, Z-8)")
        );
        assert_eq!(norm_level("Level 2"), "2");
        assert_eq!(norm_level("2"), "2");
    }
}
